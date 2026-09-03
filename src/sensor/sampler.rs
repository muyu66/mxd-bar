//! 采样线程：每 5s 对游戏窗口整窗截图 → **内嵌 0-9 字形模板分类器**读"全量整数 EXP" → 记录
//! `(时间戳, EXP)` 序列 → 按真实时间窗求平均速率 → 写 Shared。不要等级/百分比/经验条。
//!
//! EXP 来源（用户 2026-09-04 拍板）：digits.rs 的 `read_exp_value`。值区数字是"亮环字形"、
//! 0-9 每字固定唯一位图，故逐 6px 格取 84-bit 二值 mask 与内嵌模板按 Hamming 硬匹配（不依赖
//! tesseract/WinRT）。离线 20/20 全对（test-img/*.png 18 张 + here_full/cap）。代价：格子
//! 几何固定按 1382×807 标定，窗口尺寸/布局变了要重标（见 digits.rs 顶部）。
//!
//! 速率算法（用户 2026-09-04 定稿）：
//! - 内部只存 **(时间戳, EXP)** 样本，每 5s 一个；读取失败（未读到 EXP）**不记录**；
//! - 记录应单调递增，但升级时 EXP 会**递减一次**、随后新等级从低位一路递增。为区分"升级"与
//!   "OCR 偶尔一次误读"，用**之前平均值**（最近 ≤12 条已记录样本的均值）作参照：
//!   - 某样本比上一条小、且 ≥ 原平均 → 只是瞬时抖动（值没离开原水平），**忽略这一条**；
//!   - 若**连续 3 条都低于原平均**（≈15s；只有升级后从低位爬升才会如此）→ **判定升级，重置**
//!     —— 清掉旧段，以这一串低值的最新一条为新的起点重新累积；
//!   - 期间若回到 ≥ 原平均 → 前面的低值当误读作废，恢复正常记录；
//! - **实时EXP/分** = 最近 60s 时间窗内 (ΔEXP)/(Δt) ×60；
//!   **预估EXP/时** = 最近 ≤1h 时间窗内 (ΔEXP)/(Δt) ×3600。窗口不足 2 条/时长太短 → 显示 `-`。
//!   用真实时间戳而不是"5s×条数"，因此读失败的间隙不会把速率算错。
//!
//! 反外挂约束：只做整窗截图 + 像素分析，绝不读内存/色块。稳态不需要 WinRT。

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::state::{ExpMetrics, SamplerState, Shared};

use super::capture::{capture_window, find_game_hwnd, frame_is_blank, window_geo};
use super::digits;

/// 采样间隔。
const SAMPLE_INTERVAL: Duration = Duration::from_secs(5);
/// 连续多少次(每 5s 一次)检测不到数据后，才把 实时/预估 EXP 置为 `-`。
/// 4 次 ≈ 20 秒。期间保留最近一次算出的数值，避免一次抓屏抖动就把读数清成 `-`。
const MISS_LIMIT: u32 = 4;
/// 实时窗口：最近 60 秒（正常 ≈12 次采样）。
const MIN_WINDOW: Duration = Duration::from_secs(60);
/// 预估窗口：最近 1 小时（正常 ≈720 次采样）。
const HOUR_WINDOW: Duration = Duration::from_secs(3600);
/// 首尾两点时间跨度小于此值不算一段速率（正常 5s；留余量避免除零/采样抖动）。
const MIN_SPAN: Duration = Duration::from_secs(1);
/// 连续多少条"低于原平均"判定为升级（3 条 ×5s ≈ 15s 内）。
const LEVELUP_RUN: usize = 3;
/// "之前的平均值"取最近多少条已记录样本。
const AVG_N: usize = 12;

/// 一条记录：(采样时刻, 当时的全量 EXP)。
#[derive(Debug, Clone, Copy)]
struct Sample {
    at: Instant,
    exp: i64,
}

/// 一条新样本该何去何从。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Accept {
    /// 记入样本链。
    Recorded,
    /// 瞬时低值：忽略不记录（历史照常保留）。
    Ignored,
    /// 连续 N 条低于原平均 → 判定升级：重置并以此为新起点。
    LevelUp,
}

/// debug 构建的每 5s 心跳日志，方便对游戏画面核验。
/// 成功: [DEBUG] 14:02:31 模板 EXP=518,921
/// 失败: [DEBUG] 14:02:31 未读到 EXP EXP=--
/// 误读: [DEBUG] 14:02:31 模板 EXP=518,120 经验递减(疑似误读)，未记录
/// 升级: [DEBUG] 14:02:31 模板 EXP=3,120 判定升级，已重置
/// release 不编入。
#[cfg(debug_assertions)]
fn dbg(verb: &str, exp: Option<i64>, note: &str) {
    let t = chrono::Local::now().format("%H:%M:%S");
    let ex = exp.map(crate::util::thousands).unwrap_or_else(|| "--".into());
    eprintln!("[DEBUG] {t} {verb} EXP={ex}{note}");
}

/// 采样线程主循环。`stop=true` 时尽快退出（退出清理阶段调用方负责 join）。
pub fn run(shared: Arc<Mutex<Shared>>, stop: Arc<AtomicBool>) {
    // (时间戳, EXP) 样本序列，同级内单调不减。
    let mut recs: VecDeque<Sample> = VecDeque::with_capacity(730);
    // 判定升级用的"连续低于原平均"缓冲（尚未确认，先不记入 recs）。
    let mut pending: Vec<Sample> = Vec::with_capacity(LEVELUP_RUN);

    // 连续未读到数据的次数；连续 MISS_LIMIT 次(≈20s)后才清掉旧数值显示 `-`。
    let mut miss: u32 = 0;
    while !stop.load(Ordering::Relaxed) {
        let t0 = Instant::now();
        tick(&shared, &mut recs, &mut pending, &mut miss);
        // 剩余时间分段睡，stop 能及时打断（最长 100ms 响应）。
        let mut waited = t0.elapsed();
        while waited < SAMPLE_INTERVAL && !stop.load(Ordering::Relaxed) {
            let step = (SAMPLE_INTERVAL - waited).min(Duration::from_millis(100));
            std::thread::sleep(step);
            waited = t0.elapsed();
        }
    }
}

/// 一次完整采样：截图 → 模板分类器读全量 EXP → 单调/升级判定 → 记录 → 按时间窗算速率。
/// 读取失败只在状态栏提示，**不记录任何样本**；但连续 `MISS_LIMIT` 次(≈20s)失败会把
/// 实时/预估 EXP 清成 `-`（见 `miss_step`），任何一次成功读数都会把计数归零。
fn tick(
    shared: &Arc<Mutex<Shared>>,
    recs: &mut VecDeque<Sample>,
    pending: &mut Vec<Sample>,
    miss: &mut u32,
) {
    let Some(hwnd) = find_game_hwnd() else {
        #[cfg(debug_assertions)]
        dbg("未检测到游戏窗口", None, "");
        set_status(shared, SamplerState::NoGame, "未检测到游戏窗口".into());
        miss_step(shared, miss);
        return;
    };
    let Some(frame) = capture_window(hwnd) else {
        #[cfg(debug_assertions)]
        dbg("无法抓取游戏窗口", None, "");
        set_status(shared, SamplerState::NoGame, "无法抓取游戏窗口".into());
        miss_step(shared, miss);
        return;
    };
    if frame_is_blank(&frame) {
        // 说明可能的原因：最小化时 PrintWindow 与屏幕 BitBlt 都拿不到内容。
        let why = match window_geo(hwnd) {
            Some(g) if g.minimized => "游戏窗口最小化，抓不到画面",
            _ => "抓到的画面为空（PrintWindow/反外挂拦截，或窗口被遮挡）",
        };
        #[cfg(debug_assertions)]
        dbg(why, None, "");
        set_status(shared, SamplerState::OcrFailed, why.into());
        miss_step(shared, miss);
        return;
    }

    let Some(exp_now) = digits::read_exp_value(&frame) else {
        // 本帧没读出 EXP（值区被盖/切屏/窗口几何不符）：提示但不记录。
        #[cfg(debug_assertions)]
        dbg("未读到 EXP", None, "");
        set_status(shared, SamplerState::OcrFailed, "未读到 EXP".into());
        miss_step(shared, miss);
        return;
    };
    *miss = 0; // 成功读到数据 → 连续失败计数归零
    let now = Instant::now();
    let accept = accept_exp(recs, pending, now, exp_now);
    #[cfg(debug_assertions)]
    let note = match accept {
        Accept::Recorded => "",
        Accept::Ignored => " 经验递减(疑似瞬时误读)，未记录",
        Accept::LevelUp => " 判定升级，已重置",
    };

    let (per_min, per_hour, n_min, n_hour) = metrics(recs, now);

    let mut g = shared.lock().unwrap();
    g.exp = ExpMetrics {
        per_min,
        per_hour,
        n_min,
        n_hour,
        level: None,
        updated: Some(Instant::now()),
    };
    g.sampler.state = SamplerState::Running;
    g.sampler.msg = match accept {
        Accept::Ignored => format!("EXP 递减(疑似瞬时误读)，该条未记录（当前 EXP {exp_now}）"),
        Accept::LevelUp => format!("判定升级，已重置（当前 EXP {exp_now}）"),
        Accept::Recorded => format!("EXP {exp_now}"),
    };

    #[cfg(debug_assertions)]
    dbg("模板", Some(exp_now), note);
}

/// 接受/拒收一条新读到的 EXP（决定升级 vs 误读的判据在模块顶部文档）。
///
/// - 高于等于最近一条已记录 → 正常记入；
/// - 低于最近一条已记录：与"之前平均值"（最近 ≤AVG_N 条已记录均值）比
///   - ≥ 平均值 → 瞬时抖动（值没离开原水平），忽略；
///   - < 平均值 → 记入 pending 观察；连续 LEVELUP_RUN 条都低于平均 → 升级 → 重置，
///     以该串最新一条为新起点；中途回到 ≥ 平均 → 前面的低值当误读清空。
fn accept_exp(
    recs: &mut VecDeque<Sample>,
    pending: &mut Vec<Sample>,
    now: Instant,
    exp: i64,
) -> Accept {
    if recs.is_empty() {
        recs.push_back(Sample { at: now, exp });
        return Accept::Recorded;
    }
    let last = recs.back().unwrap().exp;
    if exp >= last {
        // 正常（含持平）：前面如果有观望中的低值，说明它们只是误读，作废。
        pending.clear();
        recs.push_back(Sample { at: now, exp });
        prune(recs, now);
        return Accept::Recorded;
    }
    // 比上一条小：看是否真的离开了原来的平均水平。
    match recent_avg(recs) {
        Some(avg) if exp < avg => {
            pending.push(Sample { at: now, exp });
            if pending.len() >= LEVELUP_RUN {
                // 连续 N 条都低于原平均 → 升级。旧段作废，以最新一条低值为新起点。
                let anchor = pending.last().copied().unwrap();
                recs.clear();
                recs.push_back(anchor);
                pending.clear();
                return Accept::LevelUp;
            }
            Accept::Ignored
        }
        _ => {
            // 值仍在原平均附近（未离开原水平）→ 单条瞬时抖动，忽略；观望清空。
            pending.clear();
            Accept::Ignored
        }
    }
}

/// 最近 ≤AVG_N 条已记录样本的 EXP 均值（判定升级的参照"之前的平均值"）。
fn recent_avg(recs: &VecDeque<Sample>) -> Option<i64> {
    if recs.is_empty() {
        return None;
    }
    let take = recs.len().min(AVG_N);
    let sum: i64 = recs.iter().rev().take(take).map(|s| s.exp).sum();
    Some(sum / take as i64)
}

/// 丢弃超过 1 小时的最老样本（至少保留最新一条）。
fn prune(recs: &mut VecDeque<Sample>, now: Instant) {
    while recs.len() > 1 {
        let keep = match recs.front() {
            Some(f) => now.saturating_duration_since(f.at) <= HOUR_WINDOW,
            None => true,
        };
        if keep {
            break;
        }
        recs.pop_front();
    }
}

/// 取时间窗内首尾两点的平均速率 (EXP/秒) 与窗内样本条数。
/// 窗口内不足 2 条时退化为最近两条（已保留 ≤1h）；样本 <2 或首尾跨度 <MIN_SPAN 返回 None。
fn window_rate(recs: &VecDeque<Sample>, now: Instant, win: Duration) -> Option<(f64, usize)> {
    if recs.len() < 2 {
        return None;
    }
    let cutoff = now
        .checked_sub(win)
        .unwrap_or_else(|| recs.front().map(|f| f.at).unwrap_or(now));
    let n = recs.len();
    // 窗内起点（最老一条 at>=cutoff）。
    let mut lo = n;
    for (i, s) in recs.iter().enumerate() {
        if s.at >= cutoff {
            lo = i;
            break;
        }
    }
    if n - lo < 2 {
        lo = n.saturating_sub(2); // 窗内不足 → 退化为最近两条
    }
    let first = &recs[lo];
    let last = recs.back().unwrap();
    let dt = last.at.saturating_duration_since(first.at);
    if dt < MIN_SPAN {
        return None;
    }
    let de = (last.exp - first.exp).max(0) as f64; // 同级单调，理论上非负
    Some((de / dt.as_secs_f64(), n - lo))
}

/// 换算两个展示指标。样本 <2（启动/重置初期）→ 全 0，UI 用 n_hour==0 显示 `-`。
fn metrics(recs: &VecDeque<Sample>, now: Instant) -> (i64, i64, u32, u32) {
    match (
        window_rate(recs, now, MIN_WINDOW),
        window_rate(recs, now, HOUR_WINDOW),
    ) {
        (Some((rm, cm)), Some((rh, ch))) => {
            let per_min = (rm * 60.0).round() as i64;
            let per_hour = (rh * 3600.0).round() as i64;
            (per_min, per_hour, cm as u32, ch as u32)
        }
        _ => (0, 0, 0, 0),
    }
}

/// 写入状态（不拿锁太久）。
fn set_status(shared: &Arc<Mutex<Shared>>, state: SamplerState, msg: String) {
    let mut g = shared.lock().unwrap();
    g.sampler.state = state;
    g.sampler.msg = msg;
}

/// 记一次"检测不到数据"。累计达到 `MISS_LIMIT`(≈20s) 才真正清空速率——
/// 在此之前继续显示最近一次算出的数值，等读数恢复即可，不至于抓屏一闪就变 `-`。
fn miss_step(shared: &Arc<Mutex<Shared>>, miss: &mut u32) {
    if *miss >= MISS_LIMIT {
        return; // 已经显示 `-`，无需反复清零
    }
    *miss += 1;
    if *miss >= MISS_LIMIT {
        let mut g = shared.lock().unwrap();
        g.exp = ExpMetrics::default(); // n_hour==0 → UI 显示 `-`
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn smp(base: Instant, ago_secs: u64, exp: i64) -> Sample {
        Sample { at: base - Duration::from_secs(ago_secs), exp }
    }

    /// 造一段"原平均"基线：12 条、每 5s、缓升，便于构造参照平均值。
    fn plateau(now: Instant, start: i64, step: i64) -> VecDeque<Sample> {
        let mut r = VecDeque::new();
        for i in 0..AVG_N {
            r.push_back(smp(now, (AVG_N - 1 - i) as u64 * 5, start + i as i64 * step));
        }
        r
    }

    #[test]
    fn normal_and_equal_recorded() {
        let now = Instant::now();
        let mut recs = VecDeque::new();
        let mut pend = Vec::new();
        assert_eq!(accept_exp(&mut recs, &mut pend, now - Duration::from_secs(5), 100), Accept::Recorded);
        assert_eq!(accept_exp(&mut recs, &mut pend, now, 100), Accept::Recorded); // 持平
        assert_eq!(recs.len(), 2);
    }

    #[test]
    fn single_ocr_dip_is_ignored_history_kept() {
        let now = Instant::now();
        let mut recs = plateau(now, 1_000_000, 100); // 均值≈1,000,600，末值 1,001,100
        let n = recs.len();
        let mut pend = Vec::new();
        // 一条误读，低得离谱但只出现一次。
        assert_eq!(accept_exp(&mut recs, &mut pend, now, 800_000), Accept::Ignored);
        assert_eq!(recs.len(), n, "忽略不入列、不清历史");
        // 下一条回到 ≥ 末值 → 正常记录，观望清空。
        assert_eq!(accept_exp(&mut recs, &mut pend, now + Duration::from_secs(5), 1_001_200), Accept::Recorded);
        assert_eq!(recs.len(), n + 1);
        assert!(pend.is_empty());
    }

    #[test]
    fn three_below_average_is_levelup_reset() {
        let now = Instant::now();
        let mut recs = plateau(now, 1_000_000, 0); // 平台：均值=末值=1,000,000
        let mut pend = Vec::new();
        assert_eq!(accept_exp(&mut recs, &mut pend, now, 3_000), Accept::Ignored);
        assert_eq!(accept_exp(&mut recs, &mut pend, now + Duration::from_secs(5), 3_100), Accept::Ignored);
        // 第 3 条仍低于原平均 → 升级。
        assert_eq!(accept_exp(&mut recs, &mut pend, now + Duration::from_secs(10), 3_200), Accept::LevelUp);
        assert_eq!(recs.len(), 1);
        assert_eq!(recs[0].exp, 3_200, "以最新一条低值为新起点");
        assert!(pend.is_empty());
    }

    #[test]
    fn dip_not_repeated_does_not_reset() {
        let now = Instant::now();
        let mut recs = plateau(now, 1_000_000, 0);
        let n = recs.len();
        let mut pend = Vec::new();
        assert_eq!(accept_exp(&mut recs, &mut pend, now, 900_000), Accept::Ignored);
        // 没到 3 条就回平台 → 只作误读，重置不发生。
        assert_eq!(accept_exp(&mut recs, &mut pend, now + Duration::from_secs(5), 1_000_000), Accept::Recorded);
        assert_eq!(recs.len(), n + 1);
        assert!(pend.is_empty());
    }

    #[test]
    fn rate_uses_real_timestamps() {
        let now = Instant::now();
        let mut recs = VecDeque::new();
        recs.push_back(smp(now, 10, 1000));
        recs.push_back(smp(now, 0, 1100));
        let (r, c) = window_rate(&recs, now, HOUR_WINDOW).unwrap();
        assert!((r - 10.0).abs() < 1e-9, "r={r}");
        assert_eq!(c, 2);
    }

    #[test]
    fn minute_gap_still_correct_rate() {
        let now = Instant::now();
        let mut recs = VecDeque::new();
        recs.push_back(smp(now, 90, 1000));
        recs.push_back(smp(now, 0, 1450)); // 90s +450 → 5 EXP/s
        let (r, _) = window_rate(&recs, now, HOUR_WINDOW).unwrap();
        assert!((r - 5.0).abs() < 1e-9, "r={r}");
    }

    #[test]
    fn too_few_samples_is_none() {
        let now = Instant::now();
        let mut recs = VecDeque::new();
        recs.push_back(smp(now, 5, 1000));
        assert!(window_rate(&recs, now, HOUR_WINDOW).is_none());
        assert_eq!(metrics(&recs, now), (0, 0, 0, 0));
    }

    #[test]
    fn metrics_minute_and_hour() {
        let now = Instant::now();
        let mut recs = VecDeque::new();
        for i in 0..5 {
            recs.push_back(smp(now, (4 - i) * 5, 1000 + i as i64 * 10));
        }
        let (pm, ph, nm, nh) = metrics(&recs, now);
        assert_eq!(pm, 120);
        assert_eq!(ph, 7200);
        assert_eq!(nm, 5);
        assert_eq!(nh, 5);
    }

    #[test]
    fn prune_keeps_last_hour() {
        let now = Instant::now();
        let mut recs = VecDeque::new();
        recs.push_back(smp(now, 2 * 3600, 100));
        recs.push_back(smp(now, 3600 + 1, 200));
        recs.push_back(smp(now, 0, 300));
        prune(&mut recs, now);
        assert_eq!(recs.len(), 1);
        assert_eq!(recs[0].exp, 300);
    }
}
