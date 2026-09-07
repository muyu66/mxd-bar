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
//! - **实时EXP/分** = 最近**恰好 60s** 尾随窗的实测净增；**预估EXP/时** = 最近**恰好 1h** 尾随窗的
//!   实测净增。窗起点精确落在 `now − win`，若落在两样本之间按折线线性插值（同心电图口径），
//!   因此边界那一跳不会被整段剔出窗外而低估（见 `window_gain`）。样本链还没攒满一个窗口时，
//!   退化为"窗内两样本速率 × 窗时长"外推，启动初期照常出数；样本不足 2 条/跨度太短 → 显示 `-`。
//!   用真实时间戳而不是"5s×条数"，因此读失败的间隙不会把速率算错。
//!
//! 反外挂约束：只做整窗截图 + 像素分析，绝不读内存/色块。稳态不需要 WinRT。
//!
//! **前台门控**：只有 `Maplestory_Classic.exe` 位于前台时才截图/OCR；否则（切走/最小化/
//! 点了悬浮卡）整段当作"暂停"——不截图、不计速。暂停时长通过 `PauseClock` 从墙钟里扣掉，
//! 样本时间轴用的是"活动时钟"（真实时钟 − 累计暂停时长），因此切走再回来的空隙
//! 不会被算进经验速率（否则会把这个窗口期当成在刷怪，速率被拉低）。

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::state::{
    ExpLogLine, ExpMetrics, SamplerState, Shared, SPARK_BUCKETS, EXP_LOG_CAP,
};

use super::capture::{
    capture_window, find_game_hwnd, frame_is_blank, is_game_foreground, window_geo,
};
use super::digits;

/// 采样间隔（秒）。另供上报页把"窗内采样条数"换算成实测秒数用，故 pub。
pub(crate) const SAMPLE_INTERVAL_SECS: u64 = 5;
/// 采样间隔。
const SAMPLE_INTERVAL: Duration = Duration::from_secs(SAMPLE_INTERVAL_SECS);
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

/// 前台暂停记账：把"游戏不在前台/进程不在"的墙钟时间累积成 `lag`。
/// 采样时刻统一用**活动时钟** `active_now = 真实时钟 − lag`——切走/关游戏的那段墙钟
/// 不会进入样本时间轴，经验速率只按"真正在前台可采样"的时间算（见模块顶部文档）。
struct PauseClock {
    /// 至今累计的暂停墙钟时长。
    lag: Duration,
    /// 上一 tick 的真实时刻（`step` 前移它并返回本 tick 耗时）。
    prev: Option<Instant>,
}

impl PauseClock {
    fn new() -> Self {
        PauseClock { lag: Duration::ZERO, prev: None }
    }

    /// 自上一 tick（或线程启动）以来经过的墙钟时长，并把游标前移到 `now`。
    fn step(&mut self, now: Instant) -> Duration {
        match self.prev {
            None => {
                self.prev = Some(now);
                Duration::ZERO
            }
            Some(p) => {
                let d = now.saturating_duration_since(p);
                self.prev = Some(now);
                d
            }
        }
    }

    /// 把一段"暂停"墙钟记入累计（本 tick 判定为不采样时调用）。
    fn pause(&mut self, elapsed: Duration) {
        self.lag += elapsed;
    }

    /// 当前活动时刻 = 真实时刻 − 已累计暂停时长（下界 clamp，不会早于时钟原点）。
    fn active_now(&self, now: Instant) -> Instant {
        now.checked_sub(self.lag).unwrap_or(now)
    }
}

/// 采样线程主循环。`stop=true` 时尽快退出（退出清理阶段调用方负责 join）。
pub fn run(shared: Arc<Mutex<Shared>>, stop: Arc<AtomicBool>) {
    // (时间戳, EXP) 样本序列，同级内单调不减。
    let mut recs: VecDeque<Sample> = VecDeque::with_capacity(730);
    // 判定升级用的"连续低于原平均"缓冲（尚未确认，先不记入 recs）。
    let mut pending: Vec<Sample> = Vec::with_capacity(LEVELUP_RUN);
    // 当前"段"的起点（累计经验/测试基准）：点刷新或升级时重置；独立于 recs，1h 剪除不影响它。
    let mut base: Option<Sample> = None;

    // 连续未读到数据的次数；连续 MISS_LIMIT 次(≈20s)后才清掉旧数值显示 `-`。
    let mut miss: u32 = 0;
    let mut pc = PauseClock::new();
    while !stop.load(Ordering::Relaxed) {
        let t0 = Instant::now();
        drain_clear(&shared, &mut recs, &mut pending, &mut miss, &mut base);
        tick(&shared, &mut recs, &mut pending, &mut miss, &mut pc, &mut base, t0);
        // 剩余时间分段睡，stop 能及时打断（最长 100ms 响应）。
        let mut waited = t0.elapsed();
        while waited < SAMPLE_INTERVAL && !stop.load(Ordering::Relaxed) {
            // 清空请求不等到下一 tick（否则暂停/无游戏时最长要 5s 才生效）。
            drain_clear(&shared, &mut recs, &mut pending, &mut miss, &mut base);
            let step = (SAMPLE_INTERVAL - waited).min(Duration::from_millis(100));
            std::thread::sleep(step);
            waited = t0.elapsed();
        }
    }
}

/// 消费 UI 的"清空经验采样队列"请求（主卡刷新图标）。命中后清掉已积累的样本链、
/// 升级观望缓冲与连续失败计数，并把读数置回 `-`（无样本 → n_hour==0）。段起点一并重置。
fn drain_clear(
    shared: &Arc<Mutex<Shared>>,
    recs: &mut VecDeque<Sample>,
    pending: &mut Vec<Sample>,
    miss: &mut u32,
    base: &mut Option<Sample>,
) {
    let mut g = shared.lock().unwrap();
    if !g.clear_queue {
        return;
    }
    g.clear_queue = false;
    g.exp = ExpMetrics::default();
    g.exp_log.clear(); // 点刷新 = 清空 EXP 日志（"刷新清空"，见 UI 决策）
    drop(g); // 本地缓冲只有本线程会碰，放锁后再清，减少持锁时间
    recs.clear();
    pending.clear();
    *base = None;
    *miss = 0;
}

/// 一次完整采样：截图 → 模板分类器读全量 EXP → 单调/升级判定 → 记录 → 按时间窗算速率。
/// 读取失败只在状态栏提示，**不记录任何样本**；但连续 `MISS_LIMIT` 次(≈20s)失败会把
/// 实时/预估 EXP 清成 `-`（见 `miss_step`），任何一次成功读数都会把计数归零。
fn tick(
    shared: &Arc<Mutex<Shared>>,
    recs: &mut VecDeque<Sample>,
    pending: &mut Vec<Sample>,
    miss: &mut u32,
    pc: &mut PauseClock,
    base: &mut Option<Sample>,
    now_real: Instant,
) {
    let wall = pc.step(now_real);
    let Some(hwnd) = find_game_hwnd() else {
        // 游戏进程不在：无法采样、也不会涨经验，这段墙钟按"暂停"扣掉，回来时速率才连续。
        pc.pause(wall);
        #[cfg(debug_assertions)]
        dbg("未检测到游戏窗口", None, "");
        set_status(shared, SamplerState::NoGame, "未检测到游戏窗口".into());
        miss_step(shared, miss);
        return;
    };
    if !is_game_foreground(hwnd) {
        // 前台门控：只有 Maplestory_Classic.exe 位于前台才截图/OCR，否则经验计时暂停。
        // 暂停 ≠ 失败：不截图、不清掉已算出的数值（冻结显示），且这段墙钟不进速率。
        pc.pause(wall);
        #[cfg(debug_assertions)]
        dbg("游戏不在前台，经验计时暂停", None, "");
        set_status(shared, SamplerState::Paused, "游戏不在前台，经验计时暂停".into());
        return;
    }
    // 至此前台可采样。样本用"活动时钟"：扣除此前累计的暂停墙钟，时间轴不因切走而稀释。
    let now = pc.active_now(now_real);
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
    let accept = accept_exp(recs, pending, now, exp_now);
    // 推进"段起点"并算累计经验（最新 EXP − 段起点；升级/刷新会重置段起点，见 next_base）。
    *base = next_base(accept, recs, *base);
    let cum = cum_gain(recs, *base);
    #[cfg(debug_assertions)]
    let note = match accept {
        Accept::Recorded => "",
        Accept::Ignored => " 经验递减(疑似瞬时误读)，未记录",
        Accept::LevelUp => " 判定升级，已重置",
    };

    let (per_min, per_hour, n_min, n_hour) = metrics(recs, now);

    let mut g = shared.lock().unwrap();
    // 控制台 EXP 日志：记入样本或升级都镜像一行（墙钟时间，不是"活动时钟"——
    // 暂停期不采样自然不写；刷新清空交给 drain_clear）。
    if matches!(accept, Accept::Recorded | Accept::LevelUp) {
        g.exp_log.push_back(ExpLogLine {
            hms: chrono::Local::now().format("%H:%M:%S").to_string(),
            exp: exp_now,
            seg: accept == Accept::LevelUp,
        });
        while g.exp_log.len() > EXP_LOG_CAP {
            g.exp_log.pop_front();
        }
    }
    g.exp = ExpMetrics {
        per_min,
        per_hour,
        n_min,
        n_hour,
        level: None,
        updated: Some(Instant::now()),
        spark: spark_gains(recs, now),
        cum_gain: cum,
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

/// 一段采样后推进"段起点"（累计经验的分母）：升级判定已把链重置成单条低值锚点 → 以它为起点；
/// 否则保持已有起点；还没有起点（启动 / 点刷新之后）且记下了样本 → 以首条样本为段首。
/// 段起点独立于样本链保存，`prune` 丢弃老样本不会挪动它，长会话（>1h）累计不截断。
fn next_base(accept: Accept, recs: &VecDeque<Sample>, base: Option<Sample>) -> Option<Sample> {
    match accept {
        Accept::LevelUp => recs.back().copied(),
        _ => base.or_else(|| recs.front().copied()),
    }
}

/// 累计经验 = 当前最新 EXP − 段起点 EXP（段内单调递增，理论非负）。
fn cum_gain(recs: &VecDeque<Sample>, base: Option<Sample>) -> i64 {
    base.and_then(|b| recs.back().map(|l| l.exp.saturating_sub(b.exp).max(0))).unwrap_or(0)
}

/// 取时间窗内首尾两点的平均速率 (EXP/秒) 与窗内样本条数。
/// 供 `window_gain` 在"样本链尚未盖满整窗"时作速率外推回退；窗口内不足 2 条时退化为最近两条
/// （已保留 ≤1h）；样本 <2 或首尾跨度 <MIN_SPAN 返回 None。
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

/// 最近"恰好 `win` 时长"尾随窗的实测净增 EXP（分钟窗 → 实时EXP/分、小时窗 → 预估EXP/时）
/// 与窗内样本条数。窗起点精确落在 `now − win`：
/// - **盖满**（已运行 ≥ win 且链上最早样本 ≤ now−win）：起点落点用 `exp_at` 折线插值取值，
///   净增 = 末样本 EXP − exp_at(now − win)。边界那一跳只按其落在窗内的比例计入，不会整段被
///   剔出窗外而低估（此前窗口起点吸附样本点，60s 边界整段丢失）。
/// - **未盖满**（启动初期/运行不足一个窗口）：退化为 `window_rate` 两样本速率 × 窗时长外推，
///   保证刚开刷也能照常出数。
/// 两者都无法计算 → None（`metrics` 归 0 → UI 显示 `-`）。
fn window_gain(recs: &VecDeque<Sample>, now: Instant, win: Duration) -> Option<(i64, usize)> {
    if recs.len() < 2 {
        return None;
    }
    let cutoff = now.checked_sub(win);
    let covered = cutoff.is_some_and(|c| recs.front().is_some_and(|f| f.at <= c));
    if covered {
        let c = cutoff.unwrap(); // 已保证 Some
        let gained = (recs.back().unwrap().exp - exp_at(recs, c)).max(0);
        let n = recs.iter().filter(|s| s.at >= c).count(); // 与旧 `n−lo` 口径一致
        return Some((gained, n));
    }
    // 未盖满：两样本速率 × 窗时长外推。
    let (rate, n) = window_rate(recs, now, win)?;
    Some(((rate * win.as_secs_f64()).round() as i64, n))
}

/// 换算两个展示指标。样本 <2 / 无法估算（启动、重置初期）→ 全 0，UI 用 n_hour==0 显示 `-`。
fn metrics(recs: &VecDeque<Sample>, now: Instant) -> (i64, i64, u32, u32) {
    let (Some((per_min, n_min)), Some((per_hour, n_hour))) = (
        window_gain(recs, now, MIN_WINDOW),
        window_gain(recs, now, HOUR_WINDOW),
    ) else {
        return (0, 0, 0, 0);
    };
    (per_min, per_hour, n_min as u32, n_hour as u32)
}

/// 样本链 (at,exp) 在时刻 `t` 的 EXP 值：落在相邻两样本之间时做线性插值。
/// 早于最老样本 / 晚于最新样本分别取两端值。样本为空返回 0。
/// 心电图的 5s 桶可能落在"漏读造成的空隙"里，插值能把这段收益均匀摊到每个 5s 桶，
/// 与整卡"用真实时间戳差分算速率"的口径一致（样本是逐段已知的折线，中间线性估计）。
fn exp_at(recs: &VecDeque<Sample>, t: Instant) -> i64 {
    let Some(first) = recs.front() else { return 0 };
    if t <= first.at {
        return first.exp;
    }
    let last = recs.back().unwrap();
    if t >= last.at {
        return last.exp;
    }
    let mut a = first;
    for b in recs.iter().skip(1) {
        if b.at >= t {
            let dt = b.at.duration_since(a.at).as_secs_f64();
            if dt <= 0.0 {
                return a.exp;
            }
            let f = t.duration_since(a.at).as_secs_f64() / dt;
            return (a.exp as f64 + (b.exp - a.exp) as f64 * f).round() as i64;
        }
        a = b;
    }
    last.exp
}

/// 主卡"心电图"数据：把最近 1 分钟（= 12×5s）切成 `SPARK_BUCKETS` 个 5s 桶，
/// 每桶 = 该 5s 内净增的 EXP（最新在末尾）。样本不足 1 分钟时更早的桶为 0。
/// 各桶都是非负的；UI 再相对这 1 分钟均值画"波动"折线。
fn spark_gains(recs: &VecDeque<Sample>, now: Instant) -> [i64; SPARK_BUCKETS] {
    let mut out = [0i64; SPARK_BUCKETS];
    if recs.is_empty() {
        return out;
    }
    let win = MIN_WINDOW; // 60s
    let step = SAMPLE_INTERVAL; // 5s
    let t0 = now
        .checked_sub(win)
        .unwrap_or_else(|| recs.front().map(|f| f.at).unwrap_or(now));
    for (i, g) in out.iter_mut().enumerate() {
        let l = t0 + step * i as u32;
        let r = l + step;
        *g = exp_at(recs, r).saturating_sub(exp_at(recs, l)).max(0);
    }
    out
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

    #[test]
    fn pause_clock_excludes_background_wall_time() {
        // 模拟每 5s 一次的 tick：两次前台、一次暂停、一次前台。
        let t0 = Instant::now();
        let mut pc = PauseClock::new();
        assert_eq!(pc.step(t0), Duration::ZERO); // 启动 tick 不算耗时
        // 前台 tick（5s、5s）：只前移游标，不累加暂停。
        let t1 = t0 + Duration::from_secs(5);
        let t2 = t0 + Duration::from_secs(10);
        assert_eq!(pc.step(t1), Duration::from_secs(5));
        assert_eq!(pc.step(t2), Duration::from_secs(5));
        assert_eq!(pc.active_now(t2), t2, "无暂停时活动时钟 == 真实时钟");
        // 第 3 个 tick 判定游戏不在前台：把这一 tick 的 5s 记入暂停。
        let t3 = t0 + Duration::from_secs(15);
        let d = pc.step(t3);
        assert_eq!(d, Duration::from_secs(5));
        pc.pause(d);
        assert_eq!(pc.active_now(t3), t2, "暂停期间活动时钟冻结（停表）");
        // 第 4 个 tick 回到前台：活动时钟比真实时钟慢 5s（暂停的 5s 不算）。
        let t4 = t0 + Duration::from_secs(20);
        assert_eq!(pc.step(t4), Duration::from_secs(5));
        assert_eq!(pc.active_now(t4), t3, "暂停墙钟被扣除，不稀释速率");
    }

    // -----------------------------------------------------------------------
    // "截图读到的 EXP → 实时EXP/分、预估EXP/时" 的准确性
    //
    // 以下测试回放"每 5s 从截图读到一个全量 EXP"的读数流（真机由 digits::read_exp_value
    // 产出），走与实机同一段 accept_exp → metrics 逻辑，断言算出的速率等于设定真值。
    // 每张"截图"的 EXP 已在 digits.rs 自测里验证能 1:1 读回，这里验证换算本身。
    // -----------------------------------------------------------------------

    /// 回放一条读数流：`(活动时钟相对秒, Option<EXP>)`，Some=本帧读出（走 accept_exp），
    /// None=本帧没读出（不记录，但活动时钟照走，EXP 在后续帧里把这段时间的收益补回来）。
    /// 返回 Ignored(单条误读，不入列)与 LevelUp 的次数，便于断言发生了什么。
    fn replay(
        base: Instant,
        seq: &[(u64, Option<i64>)],
        recs: &mut VecDeque<Sample>,
        pending: &mut Vec<Sample>,
    ) -> (usize, usize) {
        let (mut ignored, mut levelup) = (0, 0);
        for (s, e) in seq {
            if let Some(exp) = *e {
                match accept_exp(recs, pending, base + Duration::from_secs(*s), exp) {
                    Accept::Ignored => ignored += 1,
                    Accept::LevelUp => levelup += 1,
                    Accept::Recorded => {}
                }
            }
        }
        (ignored, levelup)
    }

    /// 每 5s 一条、EXP 线性递增的读数流：k 从 1..=ticks，t=k×5s，EXP=exp0+per_tick×k。
    /// `miss` 里的序号表示该帧截图没读出（None）。
    fn linear_reads(ticks: u32, per_tick: i64, exp0: i64, miss: &[u32]) -> Vec<(u64, Option<i64>)> {
        (1..=ticks)
            .map(|k| {
                let exp = exp0 + per_tick * k as i64;
                (k as u64 * 5, (!miss.contains(&k)).then_some(exp))
            })
            .collect()
    }

    fn rates(recs: &VecDeque<Sample>, base: Instant, end_sec: u64) -> (i64, i64, u32, u32) {
        metrics(recs, base + Duration::from_secs(end_sec))
    }

    /// 匀速刷怪：500 EXP/5s = 6000/min = 360,000/h，跑满 2 小时。
    #[test]
    fn steady_farming_rates_are_exact() {
        let base = Instant::now();
        let mut recs = VecDeque::new();
        let mut pend = Vec::new();
        replay(base, &linear_reads(1440, 500, 0, &[]), &mut recs, &mut pend);
        let (pm, ph, nm, nh) = rates(&recs, base, 1440 * 5);
        assert_eq!(pm, 6000, "实时EXP/分应 = 6000/min");
        assert_eq!(ph, 360_000, "预估EXP/时应 = 360,000/h");
        assert!(nm >= 12, "实时窗应有 ≥12 条样本，实得 {nm}");
        assert!(nh >= 700, "预估窗应有 1h 内的样本，实得 {nh}");
        // 2h 数据经过 prune，只保留最近 1h。
        let front = recs.front().unwrap();
        assert!(
            (front.exp - 360_000).abs() <= 500,
            "最老样本应约在 1h 前(EXP≈360000)，实得 {}",
            front.exp
        );
    }

    /// 读取失败/漏帧：中间整段漏读、结尾连续漏读，EXP 却在照常涨——
    /// 端到端按真实时间戳差分，平均速率不应被空隙拉偏。
    #[test]
    fn missed_reads_do_not_bias_endpoint_rate() {
        let base = Instant::now();
        let mut recs = VecDeque::new();
        let mut pend = Vec::new();
        // 30 分钟匀速；中间 (600s..720s) 与结尾 (840s..900s) 各有一整段漏读，
        // 另在第 10、17、100 个序号随机丢几帧。
        let miss = [10u32, 17, 100, 120, 121, 122, 123, 124, 168, 169, 170, 171, 172, 173, 174, 175, 176, 177, 178, 179];
        let (ign, lv) = replay(base, &linear_reads(180, 500, 0, &miss), &mut recs, &mut pend);
        assert_eq!(ign, 0);
        assert_eq!(lv, 0);
        let (pm, ph, ..) = rates(&recs, base, 180 * 5);
        assert_eq!(pm, 6000, "漏读不应拉偏实时速率");
        assert_eq!(ph, 360_000, "漏读不应拉偏预估速率");
    }

    /// 单次读到更小的值（OCR 瞬时误读）：该条被忽略不入列，历史与速率不受污染。
    #[test]
    fn single_bad_read_is_ignored_and_rates_continue() {
        let base = Instant::now();
        let mut seq = linear_reads(180, 500, 0, &[]);
        // 把第 60 个序号那一帧的读数改成"比上一条还小"（误读成低值）。
        let k = 60;
        let dip_t = k as u64 * 5;
        seq[(k - 1) as usize] = (dip_t, Some(500 * (k as i64 - 1) - 7));
        let mut recs = VecDeque::new();
        let mut pend = Vec::new();
        let (ign, lv) = replay(base, &seq, &mut recs, &mut pend);
        assert_eq!(ign, 1, "这一条低值应被当作瞬时误读忽略");
        assert_eq!(lv, 0);
        assert_eq!(
            recs.len(),
            179,
            "只应少记一条(误读不入列)，实得 {}",
            recs.len()
        );
        let (pm, ph, ..) = rates(&recs, base, 180 * 5);
        assert_eq!(pm, 6000, "单条误读后实时速率不变");
        assert_eq!(ph, 360_000, "单条误读后预估速率不变");
    }

    /// 升级：EXP 一次大跌、连续 3 条低于此前平均 → 判定升级并重置；
    /// 之后以新等级低值重新累积，速率回到新等级的真实斜率，不出现负值/失真。
    #[test]
    fn levelup_resets_then_new_level_rate_is_true() {
        let base = Instant::now();
        let mut recs = VecDeque::new();
        let mut pend = Vec::new();
        let tick = |secs: u64| base + Duration::from_secs(secs);

        // 阶段 1：旧等级匀速刷 30 分钟，500 EXP/5s。
        for k in 1..=360 {
            assert_eq!(accept_exp(&mut recs, &mut pend, tick(5 * k), 500 * k as i64), Accept::Recorded);
        }
        let old_last = recs.back().unwrap().exp; // 180,000
        // 阶段 2：升级——读数瞬间大跌到新等级的头部低值，连续 3 条仍低于旧平均。
        assert_eq!(accept_exp(&mut recs, &mut pend, tick(1805), 1500), Accept::Ignored);
        assert_eq!(accept_exp(&mut recs, &mut pend, tick(1810), 2000), Accept::Ignored);
        assert_eq!(accept_exp(&mut recs, &mut pend, tick(1815), 2500), Accept::LevelUp);
        assert_eq!(recs.len(), 1, "升级后旧段作废，只留新起点");
        assert_eq!(recs[0].exp, 2500);
        assert!(old_last > 2500);
        // 刚重置只有 1 条 → 先显示 `-`（全 0），与 UI 的 n_hour==0 一致。
        assert_eq!(metrics(&recs, tick(1815)), (0, 0, 0, 0));
        // 阶段 3：新等级继续按 500 EXP/5s 爬升，跑 ~3.5 分钟后速率应回到 6000/min。
        for k in 364..=404 {
            // k=364 → t=1820，exp 从 2500+500 继续
            let t = 5 * k;
            let exp = 2500 + 500 * (k - 363) as i64;
            assert_eq!(accept_exp(&mut recs, &mut pend, tick(t), exp), Accept::Recorded);
        }
        let (pm, ph, ..) = rates(&recs, base, 5 * 404);
        assert_eq!(pm, 6000, "升级重置后，实时速率应反映新等级斜率");
        assert_eq!(ph, 360_000, "升级重置后，预估速率应反映新等级斜率");
    }

    /// 近期提速：实时(60s 窗)应贴住最近的速度，预估(≤1h 窗)则是整段平均——两者都对，
    /// 只是口径不同。rateA=100/s(6000/min) 40 分钟，之后提速到 200/s(12000/min) 30 分钟。
    #[test]
    fn recent_speedup_realtime_vs_hourly_average_differ() {
        let base = Instant::now();
        let mut seq: Vec<(u64, Option<i64>)> = Vec::with_capacity(840);
        let mut exp = 0i64;
        for k in 1..=840 {
            // t 5..2400s: +500/tick；2405..4200s: +1000/tick
            let per = if k <= 480 { 500 } else { 1000 };
            exp += per;
            seq.push((k as u64 * 5, Some(exp)));
        }
        let mut recs = VecDeque::new();
        let mut pend = Vec::new();
        replay(base, &seq, &mut recs, &mut pend);
        let (pm, ph, ..) = rates(&recs, base, 840 * 5);
        assert_eq!(pm, 12_000, "最近 60s 全在提速后，实时应 ≈ 12,000/min");
        assert_eq!(ph, 540_000, "最近 1h 含两段(100/s 与 200/s 各 30min)，预估 = 平均 540,000/h");
        assert_ne!(pm, ph / 60, "实时≠每小时折算均值，说明两窗口径不同属预期");
    }

    /// 刚启动/刚重置、样本不足（<2 条）→ 全 0，UI 显示 `-`，绝不显示编造的速率。
    #[test]
    fn too_early_shows_dash_not_bogus_rate() {
        let base = Instant::now();
        let mut recs = VecDeque::new();
        let mut pend = Vec::new();
        assert_eq!(rates(&recs, base, 5), (0, 0, 0, 0), "一条都还没有 → `-`");
        replay(base, &linear_reads(1, 500, 0, &[]), &mut recs, &mut pend);
        assert_eq!(rates(&recs, base, 5), (0, 0, 0, 0), "只有 1 条 → `-`");
    }

    /// 端到端：把每帧"截图"(合成像素)喂给真正的 read_exp_value，得到 EXP 整数，
    /// 再过 accept_exp→metrics——证明"截图→EXP→实时/预估"整条链路没有断点。
    #[test]
    fn synthetic_screenshot_pixels_to_rates_e2e() {
        let base = Instant::now();
        let mut recs = VecDeque::new();
        let mut pend = Vec::new();
        // 从一段真实的当前 EXP 起步（跨 6 位、含 0），每 5s +500（6000/min）。
        let mut exp = 100_000i64;
        for k in 1..=40 {
            exp += 500;
            let f = crate::sensor::digits::synth_value_frame(&exp.to_string(), 758, 900, 800);
            let read = crate::sensor::digits::read_exp_value(&f)
                .unwrap_or_else(|| panic!("第 {k} 帧应读出 EXP，画面值={exp}"));
            assert_eq!(read, exp, "第 {k} 帧截图应 1:1 读回 EXP");
            accept_exp(&mut recs, &mut pend, base + Duration::from_secs(5 * k), read);
        }
        let (pm, ph, ..) = rates(&recs, base, 5 * 40);
        assert_eq!(pm, 6000, "合成截图像素流换算出的 实时EXP/分 应=6000/min");
        assert_eq!(ph, 360_000, "合成截图像素流换算出的 预估EXP/时 应=360,000/h");
    }

    /// 回归：用户实测这批每 5s 全量 EXP（01:15:32→01:16:32，恰好 13 条/60s），实时EXP/分
    /// 应 = 这一整分钟的实测净增 476,193−472,974 = 3,219，而不是因边界被截短成 2,769。
    #[test]
    fn bursty_minute_exact_alignment_is_minute_sum() {
        let base = Instant::now();
        let vals = [
            472_974, 473_655, 473_899, 474_066, 474_156, 474_554, 474_785, //
            475_196, 475_196, 475_273, 475_387, 475_693, 476_193,
        ];
        let mut recs = VecDeque::new();
        for (i, v) in vals.iter().enumerate() {
            recs.push_back(Sample { at: base + Duration::from_secs(i as u64 * 5), exp: *v });
        }
        let (pm, _, nm, _) = metrics(&recs, base + Duration::from_secs(60));
        assert_eq!(pm, 3219, "整 60s 对齐时 实时EXP/分 应 = 这一分钟实测净增");
        assert_eq!(nm, 13);
    }

    /// 回归：真实 tick 略超 5s（t0→t1 = 5.3s，末样本距 t0 = 60.3s），60s 边界落到两样本之间。
    /// 旧算法窗起点吸附到 t1，把 +681 整段丢出窗外 → (476,193−473,655)/55s×60 ≈ 2,769（低估）；
    /// 新算法在 now−60s=0.3s 处插值，这段只按其落在窗内的比例计入 → 476,193 − 473,013 = 3,180。
    #[test]
    fn bursty_minute_drifted_boundary_not_under_counted() {
        let base = Instant::now();
        let vals = [
            472_974, 473_655, 473_899, 474_066, 474_156, 474_554, 474_785, //
            475_196, 475_196, 475_273, 475_387, 475_693, 476_193,
        ];
        // 时刻(相对秒)：t0=0，t1=5.3，其后每 5.0 → t12=60.3。EXP 与上条同序列。
        let mut recs = VecDeque::new();
        for (i, v) in vals.iter().enumerate() {
            let t = if i == 0 { 0.0 } else { 5.3 + (i as f64 - 1.0) * 5.0 };
            recs.push_back(Sample { at: base + Duration::from_secs_f64(t), exp: *v });
        }
        let (pm, _, nm, _) = metrics(&recs, base + Duration::from_secs_f64(60.3));
        assert_eq!(pm, 3180, "边界漂移时 实时EXP/分 应插值接近整分钟(3,219)，而非旧算法的 2,769");
        assert!(pm > 3000, "无论如何不应再低估到 2,769");
        assert_eq!(nm, 12);
    }

    // —— 心电图数据：近 1 分钟逐 5s 净增 EXP ——

    /// 造一条严格递增的样本链：t=5s 起每 5s +500（100 EXP/s）。
    fn steady_13(base: Instant) -> VecDeque<Sample> {
        let mut recs = VecDeque::new();
        for k in 1..=13 {
            let t = 5 * k as u64;
            recs.push_back(Sample { at: base + Duration::from_secs(t), exp: 500 * k as i64 });
        }
        recs
    }

    #[test]
    fn spark_steady_60s_all_buckets_equal() {
        let base = Instant::now();
        let recs = steady_13(base); // 覆盖 5..=65s = 恰好 12 个 5s 桶
        let s = spark_gains(&recs, base + Duration::from_secs(65));
        assert_eq!(s.len(), SPARK_BUCKETS);
        for (i, g) in s.iter().enumerate() {
            assert_eq!(*g, 500, "第 {i} 个 5s 桶净增应=500");
        }
    }

    #[test]
    fn spark_splits_read_gap_evenly() {
        let base = Instant::now();
        // 抽掉 t=15s 那一条（漏读 10s：t=10 与 t=20 之间净增 +1000）。
        // 插值应把这段收益均摊到 [10,15) 与 [15,20) 两个 5s 桶，各 +500，速率不偏。
        let mut recs = VecDeque::new();
        for t in [5u64, 10, 20, 25, 30, 35, 40, 45, 50, 55, 60, 65] {
            let exp = 500 * (t / 5) as i64; // t=20 → 2000（t=15 的 +500 已并入）
            recs.push_back(Sample { at: base + Duration::from_secs(t), exp });
        }
        let s = spark_gains(&recs, base + Duration::from_secs(65));
        assert_eq!(s, [500; SPARK_BUCKETS], "漏读间隙的收益应被均摊，每 5s 桶仍=500");
    }

    #[test]
    fn spark_before_data_buckets_are_zero() {
        let base = Instant::now();
        // 最近才有数据：t=45..65s 共 5 条（+500/条），now=65s。
        // 窗口 [5,65] 里 [5,45) 在首个样本之前 → 净增 0；[45,65) 才有 +500。
        let mut recs = VecDeque::new();
        for k in 9..=13 {
            let t = 5 * k as u64;
            recs.push_back(Sample { at: base + Duration::from_secs(t), exp: 500 * k as i64 });
        }
        let s = spark_gains(&recs, base + Duration::from_secs(65));
        assert_eq!(&s[0..8], &[0; 8], "最早 8 个桶在首个样本前，应为 0");
        assert_eq!(&s[8..], &[500; 4], "进入有数据区间后每桶=500");
    }

    // —— 累计经验（段起点 + 不随 1h 剪除）——

    #[test]
    fn base_starts_at_first_sample_and_cum_grows() {
        let now = Instant::now();
        let mut recs = VecDeque::new();
        let mut pend = Vec::new();
        let mut base = None;
        let a1 = accept_exp(&mut recs, &mut pend, now - Duration::from_secs(10), 1000);
        assert_eq!(a1, Accept::Recorded);
        base = next_base(a1, &recs, base);
        assert_eq!(base.map(|b| b.exp), Some(1000), "段首 = 第一条样本");
        assert_eq!(cum_gain(&recs, base), 0);
        let a2 = accept_exp(&mut recs, &mut pend, now - Duration::from_secs(5), 1100);
        assert_eq!(a2, Accept::Recorded);
        base = next_base(a2, &recs, base);
        assert_eq!(cum_gain(&recs, base), 100);
        let a3 = accept_exp(&mut recs, &mut pend, now, 1300);
        base = next_base(a3, &recs, base);
        assert_eq!(cum_gain(&recs, base), 300);
    }

    #[test]
    fn levelup_rebases_and_cum_restarts() {
        let now = Instant::now();
        let mut recs = plateau(now, 1_000_000, 0); // 平台：均值=末值=1,000,000
        let base_old = recs.front().copied(); // 旧段起点（最老样本）
        let mut base = base_old;
        let mut pend = Vec::new();
        assert_eq!(accept_exp(&mut recs, &mut pend, now, 3000), Accept::Ignored);
        assert_eq!(
            accept_exp(&mut recs, &mut pend, now + Duration::from_secs(5), 3100),
            Accept::Ignored
        );
        let r3 = accept_exp(&mut recs, &mut pend, now + Duration::from_secs(10), 3200);
        assert_eq!(r3, Accept::LevelUp);
        assert_eq!(recs.len(), 1, "升级后旧段作废，只留新起点");
        base = next_base(r3, &recs, base);
        assert_eq!(base.map(|b| b.exp), Some(3200), "升级后段起点应更新为新锚点");
        assert_eq!(cum_gain(&recs, base), 0);
        // 新等级再记一条：累计只算新等级增量，不含旧等级。
        let r4 = accept_exp(&mut recs, &mut pend, now + Duration::from_secs(15), 3700);
        assert_eq!(r4, Accept::Recorded);
        base = next_base(r4, &recs, base);
        assert_eq!(cum_gain(&recs, base), 500);
    }

    #[test]
    fn cum_gain_ignores_hourly_prune() {
        let now = Instant::now();
        // 2 小时匀速：k 1..=1440，每 5s +500，最早样本 exp=500，最新 exp=720,000。
        let mut recs = VecDeque::new();
        for k in 1..=1440 {
            let t = now - Duration::from_secs((1440 - k) as u64 * 5);
            recs.push_back(Sample {
                at: t,
                exp: 500 * k as i64,
            });
        }
        let base = recs.front().copied(); // 段起点 = 全程最早样本
        let full_gain = cum_gain(&recs, base); // ≈720,000 − 500
        prune(&mut recs, now); // 1h 剪除：front 前移到 ~1h 前
        assert!(
            recs.front().unwrap().exp > base.unwrap().exp,
            "老样本应被剪除"
        );
        // 段起点保持最老锚点不变 → 累计仍 = 全程净获
        assert_eq!(cum_gain(&recs, base), full_gain, "累计不应随 1h 剪除回退");
        // 若按剪除后链首尾差则不足全程（说明确实需要独立段起点）
        let window_gain = recs.back().unwrap().exp - recs.front().unwrap().exp;
        assert!(window_gain < full_gain);
    }
}
