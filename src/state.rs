//! 共享数据面 `Shared`：sampler 线程写、UI 读；UI 写、config 落盘。
//!
//! UI 目前是单窗口（主卡 + 下方抽屉页），页面绘制需要 `&mut` 编辑缓冲，
//! 全部可变状态集中在这里，用 `Arc<Mutex<Shared>>` 传递，避免跨线程（sampler）抢锁。

use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Instant;

use crate::config::AppConfig;
use crate::data::{JobGroup, MapInfo};

/// 当前打开的是哪一页（决定主卡下方抽屉展开的内容）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Page {
    None,
    /// 上报数据（或其后继的成功页，看 report.phase）。
    Report,
    /// 两个计时按钮（999打卡/BOSS）共用的时分选择页。
    TimePick(TimePickKind),
    /// 控制台式 EXP 日志页。
    Log,
    /// 「关于」页：工具版本 + 官网。
    About,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimePickKind {
    Punch,
    Boss,
}

/// 时分选择器（999打卡/BOSS）的编辑缓冲：当前选中的 小时/分钟。
/// app.rs 在打开时按类型预填默认值；点"确定"后据此写入 cfg 并落盘。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PickState {
    pub h: u32,
    pub m: u32,
}

// ---------------------------------------------------------------------------
// EXP 日志（主卡「日志」抽屉页的数据源：sampler 写、UI 只读渲染）
// ---------------------------------------------------------------------------

/// EXP 日志保留的最大行数（超过丢最老）。
pub(crate) const EXP_LOG_CAP: usize = 1000;

/// 一条原始 EXP 日志（sampler 线程写入）。`seg=true` = 升级重置那一条「段头」。
#[derive(Debug, Clone)]
pub struct ExpLogLine {
    /// 记录时刻的墙钟 "%H:%M:%S"。
    pub hms: String,
    /// 当时的全量 EXP。
    pub exp: i64,
    /// true = 升级段头：此行走「无 [..] 括号」，其后各行的 +Δ 相对它（也就是相对上一条）累积。
    pub seg: bool,
}

/// 渲染用的一行（脱离 egui 可测）。`delta=None` = 段头 / 刷新后首条 / 无前一条（不显示 [ +Δ ]）。
#[derive(Debug, Clone)]
pub struct ExpLogRow {
    /// 墙钟 "%H:%M:%S"。
    pub t: String,
    /// 当时的全量 EXP。
    pub exp: i64,
    /// 比上一条记录的增量。`None` 表示本行不带 [ +Δ ]。
    pub delta: Option<i64>,
    /// 是否为升级段头（页面在其上方画一条细分隔线开新段）。
    pub seg: bool,
}

/// 把日志缓冲按「比上一条」换算成控制台行。规则：
/// - 升级段头行（`seg`）→ 新起一段，`delta=None`（EXP 大跌那一条不显示 [ -负数 ]）；
/// - 其余行 → `delta = 本行 EXP − 上一条 EXP`（段内单调，理论 ≥0）；
/// - 没有上一条（刷新清空后首条 / cap 剪掉段头）→ 当作新起点，`delta=None`。
pub fn exp_log_rows(log: &VecDeque<ExpLogLine>) -> Vec<ExpLogRow> {
    let mut out = Vec::with_capacity(log.len());
    let mut last: Option<i64> = None;
    for ln in log {
        let delta = match (ln.seg, last) {
            (true, _) => None,                       // 升级段头：无括号
            (false, Some(prev)) => Some(ln.exp.saturating_sub(prev)),
            (false, None) => None,                   // 刷新后首条 / 被 cap 剪掉上一条
        };
        out.push(ExpLogRow { t: ln.hms.clone(), exp: ln.exp, delta, seg: ln.seg });
        last = Some(ln.exp);
    }
    out
}

/// 主卡"心电图"的采样点数：近 1 分钟按 5s 分桶 = 12 点。
pub(crate) const SPARK_BUCKETS: usize = 12;

/// sampler 线程写给 UI 看的最新换算结果。
#[derive(Debug, Clone, Default)]
pub struct ExpMetrics {
    pub per_min: i64,
    pub per_hour: i64,
    /// 计算所用的样本条数：实时 = 最近 60s 窗内条数(n_min)、预估 = 最近 ≤1h 窗内条数(n_hour)。
    /// UI 只用 n_hour>0 判定"有足够样本可显示"（启动/重置初期无样本 → 显示 `-`）；n_min 供日后
    /// 展示实时窗口大小时用。
    #[allow(dead_code)]
    pub n_min: u32,
    pub n_hour: u32,
    /// 等级：已弃用——采样只差分模板分类器读出的 EXP 整数，不取等级；
    /// 升级/瞬时误读的判定在 sampler.rs（连续 3 条低于平均才重置）。字段保留兼容旧 UI。
    #[allow(dead_code)]
    pub level: Option<u32>,
    /// 最近一次成功采样距当前有多久。预留：卡片"采样中/已过期"指示用。
    #[allow(dead_code)]
    pub updated: Option<std::time::Instant>,
    /// 近 1 分钟逐 5s 净增 EXP（`SPARK_BUCKETS` 点，最新在末尾）——主卡"心电图"数据源。
    /// 无样本 / 被清空时全 0；UI 在 `n_hour==0` 时隐藏整条心电图。
    pub spark: [i64; SPARK_BUCKETS],
    /// 累计经验：当前"段"（自最近一次 刷新/升级 起）最新 EXP − 段起点 EXP。
    /// 段起点独立于样本链保留、不受 1h 剪除影响，因此长会话（>1h）也持续累计。
    pub cum_gain: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SamplerState {
    #[default]
    Init,
    Running,
    NoGame,
    /// 游戏在运行但窗口不在前台（被切走/最小化/点了悬浮卡）：暂停截图与经验计时。
    /// 与 NoGame 不同：这是"暂停"而非"失败"，已算出的数值保持冻结、不清成 `-`。
    Paused,
    OcrFailed,
}

#[derive(Debug, Clone, Default)]
pub struct SamplerStatus {
    pub state: SamplerState,
    pub msg: String,
}

// ---------------------------------------------------------------------------
// 实时效率 PK（主卡最左新区域；后台上报线程写、UI 只读渲染）
// ---------------------------------------------------------------------------

/// PK 上报线程写给 UI 看的最新状态。UI 只取 `rank` 画名次标签：
/// `None` → 主卡显示 `-`；`Some(r)` → 显示 `第{r}名`（r>999 显示 `第999+名`）。
#[derive(Debug, Clone, Default)]
pub struct PkState {
    /// 服务端返回的名次（1 起，原样未截断）。None = 还没上报成功 / 已离线被清 →
    /// 显示 `-`。UI 显示时的 999 封顶另算（见 util::pk_rank_text），这里存原始值。
    pub rank: Option<u32>,
    /// 服务端返回的参与总数（可选字段；当前暂无展示位，留作诊断）。
    pub total: Option<u32>,
    /// 最近一次上报成功时刻（Instant::now()）；UI 可据此判断名次新鲜度（暂无展示）。
    pub updated: Option<Instant>,
    /// 最近一次上报失败的原因（暂无展示位，留作调试/日志）。
    pub error: Option<String>,
}

/// 上报流程的子页。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReportPhase {
    Form,
    Success,
}
impl Default for ReportPhase {
    fn default() -> Self {
        ReportPhase::Form
    }
}

/// 确认提交后跳到成功页要展示的内容。
#[derive(Debug, Clone, Default)]
pub struct ReportSuccess {
    /// 分享网址：`<base>/exp.html?id=<服务端返回的 id>`（点开只显示这一条记录）。
    pub url: String,
}

/// 一次"确认提交"打包好的整份上报载荷（在 UI 校验通过那一刻冻结，
/// 交给后台线程真正联网；open 后再改表单不影响这次提交）。
#[derive(Debug, Clone)]
pub struct ReportPayload {
    pub level: u32,
    pub job: String,
    pub map: String,
    pub mode_solo: bool,
    pub power: u64,
    pub note: String,
    /// 有会员 = true；无会员 = false。
    pub vip: bool,
    /// 打开上报页那一刻缓存的 预估EXP/时。
    pub exp_per_hour: i64,
    /// 对应的"实测刷怪秒数"（窗内采样条数 × 5s）。
    pub exp_seconds: i64,
}

/// 上报表单的编辑态。egui 文本编辑只能经共享态缓冲，故全用字符串/索引。
#[derive(Debug, Clone, Default)]
pub struct ReportState {
    pub phase: ReportPhase,
    /// 等级（正整数文本）。
    pub level: String,
    /// 职业选择：job_group 组下标 + 该组内 job 下标。
    pub job_group: usize,
    pub job: usize,
    /// 地图：输入+搜索二合一。
    pub map_query: String,
    /// true=单人。
    pub mode_solo: bool,
    /// 有会员 = true；无会员 = false（打开页面默认无会员）。
    pub vip: bool,
    /// 攻击力/魔法力（正整数文本；标签由 job 组决定）。
    pub power: String,
    /// 备注 ≤20 字符。
    pub note: String,
    /// 表单校验错误。
    pub err: String,
    /// 成功页内容（phase==Success 时有值）。
    pub success: Option<ReportSuccess>,
    /// 正在提交（联网中）：置灰「确认提交」并显示"提交中…"，防重复点。
    pub submitting: bool,
    /// 打开上报页那一刻缓存的 预估EXP/时 与 实测秒数（提交载荷的一部分）。
    pub exp_per_hour: i64,
    pub exp_seconds: i64,
    /// 递增序号：每次打开页面或发起提交 +1。后台线程提交完成回写前比对它，
    /// 不符说明页面已被重新打开/切换，旧结果不再写入（防止覆盖新会话状态）。
    pub req: u64,
    /// 校验通过、待联网提交的载荷（submitting=true 时有值）。
    pub pending: Option<ReportPayload>,
}

/// 全部共享状态。
pub struct Shared {
    // —— sampler 写、UI 读 ——
    pub exp: ExpMetrics,
    pub sampler: SamplerStatus,
    /// UI 请求清空经验采样队列（主卡最右的刷新图标）。sampler 线程每轮循环消费后复位 false。
    /// 置位后 sampler 会清掉已积累的 (时间戳,EXP) 样本链、升级观望缓冲与连续失败计数，
    /// 并把读数置回 `-`（ExpMetrics 清零）；经验速率随后从新样本重新累积。
    pub clear_queue: bool,
    /// 控制台式 EXP 日志（每一条 记入样本/升级 都镜像一条；点刷新清空）。sampler 线程追加。
    pub exp_log: VecDeque<ExpLogLine>,
    // —— UI 导航 ——
    pub page: Page,
    pub report: ReportState,
    /// 时分选择器的编辑值（Page::TimePick(_) 打开时使用）。
    pub pick: PickState,
    // —— net 后台算完回填 ——
    pub uid: Option<String>,
    pub uid_error: Option<String>,
    /// v2 JWT（POST /api/v2/exp/token 换来，2h 有效）。token_keeper 线程维护。
    pub token: Option<String>,
    /// token 过期时刻。
    pub token_exp: Option<Instant>,
    /// 最近一次换 token 失败的原因（供 UI/调试提示，暂无展示位）。
    pub token_error: Option<String>,
    // —— PK 后台上报线程写 ——
    pub pk: PkState,
    // —— 只读数据表 ——
    pub jobs: Arc<Vec<JobGroup>>,
    /// 只含 `scene=="hunting"` 的地图。
    pub maps: Arc<Vec<MapInfo>>,
    // —— ini 镜像（计时目标/上报表单默认值/uid 缓存都在这里）——
    pub cfg: AppConfig,
}

impl Shared {
    pub fn new(cfg: AppConfig, jobs: Vec<JobGroup>, maps: Vec<MapInfo>) -> Self {
        Shared {
            exp: ExpMetrics::default(),
            sampler: SamplerStatus::default(),
            clear_queue: false,
            exp_log: VecDeque::new(),
            page: Page::None,
            report: ReportState::default(),
            pick: PickState::default(),
            uid: cfg.uid_cache.clone(),
            uid_error: None,
            token: None,
            token_exp: None,
            token_error: None,
            pk: PkState::default(),
            jobs: Arc::new(jobs),
            maps: Arc::new(maps),
            cfg,
        }
    }

    /// 把当前 cfg 落盘 data.ini（计时器/表单默认值修改后调用）。
    pub fn persist_cfg(&self) {
        let _ = crate::config::save(&self.cfg);
    }

    /// 每次打开"上报数据"页时调用：把表单编辑态重置为 cfg 里的默认值。
    pub fn init_report(&mut self) {
        let (level, job, map, solo, power) = self.cfg.report_defaults();
        // 职业默认：cfg 存的是职业名，反查回 (组,组内) 下标；找不到或为空 → 第一组第一个职业。
        let first = self
            .jobs
            .iter()
            .position(|g| !g.jobs.is_empty())
            .map(|gi| (gi, 0))
            .unwrap_or((0, 0));
        let sel = if job.is_empty() {
            first
        } else {
            crate::data::find_job(&self.jobs, &job).unwrap_or(first)
        };

        // 打开上报页那一刻冻结"预估EXP/时"与其实测秒数：填表期间即使采样继续，
        // 提交的仍是进页面这一刻的值（否则填了十分钟再提交，rate 会被这十分钟稀释）。
        let secs = self.exp.n_hour as i64 * crate::sensor::sampler::SAMPLE_INTERVAL_SECS as i64;
        self.report = ReportState {
            phase: ReportPhase::Form,
            level: (level > 0).then(|| level.to_string()).unwrap_or_default(),
            job_group: sel.0,
            job: sel.1,
            map_query: map,
            mode_solo: solo,
            vip: false,
            power: (power > 0).then(|| power.to_string()).unwrap_or_default(),
            note: String::new(),
            err: String::new(),
            success: None,
            submitting: false,
            exp_per_hour: self.exp.per_hour,
            exp_seconds: secs,
            req: self.report.req.wrapping_add(1),
            pending: None,
        };
    }

    /// 由 (group_idx, job_idx) 得出职业名。
    pub fn job_name(&self, gi: usize, ji: usize) -> String {
        self.jobs
            .get(gi)
            .and_then(|g| g.jobs.get(ji))
            .cloned()
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ln(hms: &str, exp: i64, seg: bool) -> ExpLogLine {
        ExpLogLine { hms: hms.into(), exp, seg }
    }

    #[test]
    fn rows_delta_vs_previous_record() {
        let mut log = VecDeque::new();
        log.push_back(ln("11:14:22", 232_999, false));
        log.push_back(ln("11:14:27", 233_222, false));
        log.push_back(ln("11:14:32", 233_445, false));
        let rows = exp_log_rows(&log);
        // 首条没有前一条 → 无括号；其后 = 比上一条的增量（用户示例 [+223]）。
        assert_eq!(rows[0].delta, None);
        assert_eq!(rows[1].delta, Some(223));
        assert_eq!(rows[2].delta, Some(223));
        assert_eq!(rows[1].t, "11:14:27");
        assert_eq!(rows[1].exp, 233_222);
    }

    #[test]
    fn levelup_seg_head_has_no_negative_bracket() {
        // 刷满后升级：EXP 一次大跌成新等级低值，那一条是段头（无括号）；其后 +Δ 正常为正。
        let mut log = VecDeque::new();
        log.push_back(ln("11:20:00", 500_000, false));
        log.push_back(ln("11:20:05", 500_200, false));
        log.push_back(ln("11:20:10", 3_120, true)); // 升级段头
        log.push_back(ln("11:20:15", 3_350, false));
        log.push_back(ln("11:20:20", 3_580, false));
        let rows = exp_log_rows(&log);
        assert_eq!(rows[2].delta, None, "升级大跌行不得显示 [ -负数 ]");
        assert_eq!(rows[3].delta, Some(230), "段头后第一行相对段头(上一条)为正");
        assert_eq!(rows[4].delta, Some(230));
    }

    #[test]
    fn trimmed_head_is_new_baseline() {
        // cap 剪掉段头后，保留下来的第一行退化为"新起点"（无括号），后续仍比上一条。
        let mut log = VecDeque::new();
        log.push_back(ln("11:20:10", 3_120, true)); // 这段原本会被剪掉
        log.push_back(ln("11:20:15", 3_350, false));
        let rows = exp_log_rows(&log);
        assert_eq!(rows[0].delta, None);
        assert_eq!(rows[1].delta, Some(3_350 - 3_120));
    }
}
