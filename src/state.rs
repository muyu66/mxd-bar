//! 共享数据面 `Shared`：sampler 线程写、UI 读；UI 写、config 落盘。
//!
//! UI 目前是单窗口（主卡 + 下方抽屉页），页面绘制需要 `&mut` 编辑缓冲，
//! 全部可变状态集中在这里，用 `Arc<Mutex<Shared>>` 传递，避免跨线程（sampler）抢锁。

use std::sync::Arc;

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
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SamplerState {
    #[default]
    Init,
    Running,
    NoGame,
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
    /// 绿色网址（远程返回的占位）。
    pub url: String,
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
}

/// 全部共享状态。
pub struct Shared {
    // —— sampler 写、UI 读 ——
    pub exp: ExpMetrics,
    pub sampler: SamplerStatus,
    // —— UI 导航 ——
    pub page: Page,
    pub report: ReportState,
    /// 时分选择器的编辑值（Page::TimePick(_) 打开时使用）。
    pub pick: PickState,
    // —— net 后台算完回填 ——
    pub uid: Option<String>,
    pub uid_error: Option<String>,
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
            page: Page::None,
            report: ReportState::default(),
            pick: PickState::default(),
            uid: cfg.uid_cache.clone(),
            uid_error: None,
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
