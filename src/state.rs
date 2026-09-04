//! 共享数据面 `Shared`：sampler 线程写、UI 读；UI 写、config 落盘。
//!
//! UI 目前是单窗口（主卡 + 下方抽屉页），页面绘制需要 `&mut` 编辑缓冲，
//! 全部可变状态集中在这里，用 `Arc<Mutex<Shared>>` 传递，避免跨线程（sampler）抢锁。

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
    /// 三个计时按钮共用的时分选择页。
    TimePick(TimePickKind),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimePickKind {
    Punch,
    Merchant,
    Boss,
}

/// 时分选择器（999打卡/神秘商人/BOSS）的编辑缓冲：当前选中的 小时/分钟。
/// app.rs 在打开时按类型预填默认值；点"确定"后据此写入 cfg 并落盘。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PickState {
    pub h: u32,
    pub m: u32,
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
            page: Page::None,
            report: ReportState::default(),
            pick: PickState::default(),
            uid: cfg.uid_cache.clone(),
            uid_error: None,
            token: None,
            token_exp: None,
            token_error: None,
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
