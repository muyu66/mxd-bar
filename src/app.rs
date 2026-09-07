//! mxd-bar 应用主体：一个置顶、无边框、透明的悬浮卡片。
//!
//! 主卡固定 72px 高、固定 `BAR_W`(=872px) 宽；当某页打开（上报数据 / 999打卡·BOSS
//! 的时分选择 / EXP 日志 / 关于）时，卡片在**同一窗口**内向下方平滑展开，页面内容滑出在主卡下方（不再是
//! 独立弹框，主卡也始终可见）。高度按页面实际内容自动适配（测量后动画趋近）；**宽度固定**，
//! 内部的数字/倒计时等 live 文案在各自固定区域内自适应（居中，必要时缩小字号），
//! 因此外框宽度不会随内容长短抖动。
//!
//! 布局：主卡从左到右 —— ①实时效率PK ②实时EXP/分 ③预估EXP/时 ④测试时间 ⑤累计经验 ⑥心电区 ⑦按钮区 ⑧工具格
//! 心电区画近 1 分钟"每 5s 净增 EXP"相对该分钟均值的波动折线（见 paint_wave）。
//! 按钮区分两行（上报数据/管理数据 / BOSS、999打卡），其中只有「上报数据」
//! 是橘黄重点按钮；最右是 **2×2 工具格**（PNG 图标，见 icons.rs）：上行 日志 · 关闭，
//! 下行 关于 · 重置（用户 2026-09-07 定稿：close 与 info 互换）。999/BOSS 文本实时变化。
//! 数据源统一走 `Shared`（见 state.rs），
//! 本模块只读取与绘制。
//!
//! 主卡有两种**布局模式**（config.rs `PanelMode`，**只作运行时态、不落盘**）：
//! - `Normal` 常规：悬浮、可随意拖动；位置在拖动停稳后写回 ini。
//! - `Auto` 收缩/展开：把常规卡片**拖动到屏幕顶部**松手即进入 —— 窗体立刻贴到顶部(y=0)、
//!   收成一条细线。细线固定、**不可拖动**；鼠标悬停 → 展开成完整主卡；鼠标离开满 5s → 收回。
//!   悬停展开态下可拖动卡片：**拖离屏幕顶部松手 = 切回常规**（停在被拖到的位置）；仍贴着
//!   顶部松手则留在 Auto（只是换了横位）。999 打卡转「待打卡」时，收线/展开的卡片描边红
//!   色呼吸脉冲。
//!
//! **模式完全由"位置"推导**：ini `[panel]` 只记主卡**上次退出时的位置**（x/y，不含 mode）。
//! 启动时 y 落在顶部吸附带(≤ `TOP_SNAP`)内 → 进入 Auto 贴顶收起；带外 → 常规悬浮、不吸附。
//! 拖动松手也按最终落点进/出 Auto（见 `end_drag`）。因本窗口置顶/无边框/透明，
//! 实测 OS 标题栏拖拽(`ViewportCommand::StartDrag`)拖不动窗体，故拖动是**自己每帧用
//! `OuterPosition` 跟随光标**实现的（`drag_start`/`drag_follow`/`end_drag`），
//! 落点始终精确已知、无 OS 模态拖拽的滞后问题。

use std::sync::{Arc, Mutex};
use std::time::Duration;

use chrono::{Local, Timelike};
use eframe::egui;
use egui::{
    pos2, vec2, Align2, Color32, FontId, Rect, Sense, Stroke, StrokeKind, UiBuilder,
    ViewportCommand,
};

use crate::config::PanelMode;
use crate::icons::Icons;
use crate::state::{ExpMetrics, Page, PickState, Shared, TimePickKind, SPARK_BUCKETS};
use crate::theme::Palette;
use crate::ui::{about, log, pickers, report};
use crate::util::{fmt_clock, thousands, TimerUi};

/// 卡片总高（逻辑像素）。
pub const BAR_HEIGHT: f32 = 72.0;

/// 主卡固定总宽（逻辑像素）。外框恒定、内部文案自适应。用户 2026-09-07：最右工具列改 2×2
/// 工具格（→~872），同日再加最左「实时效率 PK」区（+PK_W 及其分隔槽）后到 ~974。
pub const BAR_W: f32 = 974.0;
/// 首次创建窗口的占位宽度（首帧后即按 BAR_W 修正）。
pub(crate) const INITIAL_WIDTH: f32 = BAR_W;

// —— 布局间距（逻辑像素）——
const PAD_X: f32 = 14.0; // 卡片左右内边距
const COL_GAP: f32 = 9.0; // 分隔线两侧的间距
const VAL_GAP: f32 = 8.0; // 指标区 标签→数值 间距
const COLPAD: f32 = 6.0; // 指标区文字两侧留白

const BTN_H: f32 = 26.0; // 按钮高
const BTN_RADIUS: f32 = 6.0; // 按钮圆角（用户 2026-09-04 定稿：比全胶囊小，呈圆角矩形）
const BTN_ROW_GAP: f32 = 6.0; // 按钮行距
const BTN_COL_GAP: f32 = 8.0; // 按钮列距
const BTN_PAD_X: f32 = 14.0; // 每个按钮横向额外留白(两侧合计)

const CARD_RADIUS: u8 = 14;

// —— 最右 2×2 工具格（PNG 图标，icons.rs）——
const ICON_C: f32 = 26.0; // 单格边长（方形，图标按格内接绘制）
const ICON_GAP_X: f32 = 5.0; // 格内左右两列的列距
const ICON_W: f32 = 2.0 * ICON_C + ICON_GAP_X; // 工具格总宽（行距不另设：两行居中于按钮两行，天然对齐）
const RAIL_GAP: f32 = 8.0; // 工具格与按钮区之间留的空隙

/// 单个指标列的最低宽度（防止按钮区过宽把指标区挤没了）。
const METRIC_MIN_W: f32 = 78.0;
/// 指标列个数：实时EXP/分 ｜ 预估EXP/时 ｜ 测试时间 ｜ 累计经验（等宽均分剩余宽度）。
const METRIC_N: usize = 4;
/// 主卡最左「实时效率PK」固定区宽（用户 2026-09-07 新增：上方标题"实时效率PK"、
/// 下方名次标签 `第x名`）。数值由后台 pk 线程每 10s 上报换回，见 state.rs PkState / pk.rs。
const PK_W: f32 = 84.0;
/// 心电区固定宽（用户 2026-09-04 定稿：预估列后 ~70px）。画近 1 分钟的"每 5s 经验波动"。
const WAVE_W: f32 = 70.0;

/// 抽屉开着时窗口的最小宽度（避免卡片过窄时页面挤压）。
const DRAWER_MIN_W: f32 = 320.0;
/// 高度动画趋近速率（每秒向目标靠近的比例，值越大越跟手）。
const PANEL_K: f32 = 14.0;
/// 测量抽屉内容时给子 Ui 的最大高度：只要足够大让内容按自然高度排布即可，
/// 真正的高度随后由"内容实际占用高度 + 上下留白"决定（被窗口裁剪也无妨）。
const MEASURE_H: f32 = 3000.0;

/// 抽屉页内容相对窗口左右的内边距（用户选"整幅通排但加大留白"：比主卡 PAD_X 更厚）。
const DRAWER_PAD_X: f32 = 22.0;
/// 抽屉内容相对主卡下沿 / 窗口底边的一致上下留白。各页面自身不再另加外圈空距，
/// 上下留白都由这里统一给，保证两页观感一致、不再顶边缩角。
const DRAWER_MARGIN_Y: f32 = 14.0;

// —— 收缩/展开（Auto）模式的几何与节奏 ——
/// 收成一条线时的高度（逻辑像素）。实测窗口贴近顶边后仍留有约 8px 的去不掉的透明余量，
/// 只开 8px 会被它挤得几乎看不见，故整窗取 20 让收线有可见高度。
const AUTO_STRIP_H: f32 = 20.0;
/// 悬停展开后，鼠标离开窗口满这么久(秒)再收线。
const AUTO_LEAVE: f64 = 5.0;
/// 展开/收起高度动画趋近速率（指数趋近系数，值越大越跟手）。
const AUTO_K: f32 = 16.0;
/// "顶部吸附带"宽度（逻辑像素）：窗体左上角 y ≤ 此值即视为贴在屏幕顶部。
/// 既是**拖动松手判进 Auto** 的阈值，也是**启动时据 ini 记的退出位置反推模式**的阈值——
/// 上次退出 y 落进此带 → 启动即 Auto（贴顶收起）；带外 → 常规悬浮不吸附。
/// 与收线高度 `AUTO_STRIP_H` 保持一致：收线本身就在这条带里，"拖到这条带"才算贴顶进 Auto，
/// 进入后由贴顶逻辑吸到 y=0。
const TOP_SNAP: f32 = 20.0;
/// 收线高度到此仍算"线态"：≤ 此值给收线画经验进度条；再高是展开动画中途、只画底卡。
/// 取 收线高 + 6，让收线/刚起手那几帧都覆盖到，又不至于占进展开卡的高度。
const STRIP_WAVE_MAX: f32 = AUTO_STRIP_H + 6.0;
/// 收线进度条长度缓动速率（指数趋近系数，越小收得越"软"，越大越跟手）。
const METER_K: f32 = 6.0;

/// 主卡片上的动作。Report..Boss 在按钮行里；Exit/Refresh/Log/About 走最右 2×2 工具格。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Report,
    Manage,
    Punch,
    Boss,
    /// 关闭程序（工具格 close）。
    Exit,
    /// 清空 实时/预估经验队列（工具格 reload）。
    Refresh,
    /// 打开/收起 控制台式 EXP 日志页（工具格 debug）。
    Log,
    /// 打开/收起「关于」页：工具版本 + 官网（工具格 info）。
    About,
}

impl Action {
    /// 两行按钮（不含工具格动作）。每行靠左排布，行宽各自适应。
    fn rows() -> Vec<Vec<Action>> {
        vec![
            vec![Action::Report, Action::Manage],
            vec![Action::Boss, Action::Punch],
        ]
    }

    /// 固定标签（计时按钮的显示文本见 `button_text`，随状态变化）。
    fn label(self) -> &'static str {
        match self {
            Action::Report => "上报数据",
            Action::Manage => "管理数据",
            Action::Punch => "999打卡",
            Action::Boss => "BOSS",
            Action::Exit => "关闭",
            Action::Refresh => "重置",
            Action::Log => "日志",
            Action::About => "关于",
        }
    }

    /// 是否橘黄重点按钮。主卡上只有「上报数据」。
    fn primary(self) -> bool {
        matches!(self, Action::Report)
    }
}

/// 一帧内主卡片需要显示的全部值（快照，避免持锁绘制）。
pub struct BarData {
    pub exp_min: String,
    pub exp_hour: String,
    /// 测试时间：窗内采样条数 × 5s（与上报 test_seconds 同算法），时钟式格式。无样本显示 `-`。
    pub test_time: String,
    /// 累计经验：当前段（自刷新/升级起）最新 EXP − 段起点 EXP。无样本显示 `-`。
    pub exp_cum: String,
    /// 实时效率 PK 名次（服务端原样返回、未截断；显示时 >999 封顶见 util::pk_rank_text）。
    /// None = 还没上报成功 / 已离线被清 → 显示 `-`。
    pub pk: Option<u32>,
    /// 近 1 分钟逐 5s 净增 EXP（心电图数据源）。无样本时 None → 不画心电图。
    pub spark: Option<[i64; SPARK_BUCKETS]>,
    pub punch: TimerUi,
    pub boss: TimerUi,
}

pub struct MxdBarApp {
    pub shared: Arc<Mutex<Shared>>,
    /// 窗口宽度是否已与内容对齐
    sized: bool,
    /// 常规模式：是否已把窗口放到保存位置/顶部居中（Auto 的贴顶用自己的 pinned，见下）。
    positioned: bool,
    /// 首帧需安装中文字体 + 控件样式（需要 ctx，故延迟到 ui()）
    fonts_installed: bool,
    /// 当前抽屉展开高度（0=完全收起；动画逼近当页内容测得的高度）
    panel_h: f32,
    /// 工具图标纹理（需要 ctx，首帧惰性加载；见 icons.rs）。
    icons: Option<Icons>,
    /// 布局模式：常规 / 收缩-展开。构造时由 ini 记忆的退出位置反推（y≤TOP_SNAP→Auto）；
    /// 只作内存态，不进 ini。
    mode: PanelMode,
    // —— Auto（收缩/展开）运行时子状态 ——
    /// 指针在窗内（悬停）= 展开；离开 = 待收线。
    auto_open: bool,
    /// 指针离开窗口的时刻（ctx.time）；离开满 AUTO_LEAVE 收线，期间回来即复位。
    auto_leave_at: Option<f64>,
    /// 当前高度（向 展开=BAR_HEIGHT / 收线=AUTO_STRIP_H 缓动）。
    auto_h: f32,
    /// Auto 段内贴顶的横向 x（= 进入 Auto 时的常规位置 x；首帧拿到显示器信息后定）。
    auto_x: Option<f32>,
    /// 本 Auto 段内是否已发过"贴到 (auto_x, 0)"的 OuterPosition（每个切换点复位一次）。
    pinned: bool,
    // —— 收线进度条 ——
    /// 当前显示的长度比例（0..1，向"最新 5s 值/窗内动态最大"缓动，见 ui_auto / meter_frac）。
    meter: f32,
    // —— 手动拖动（自实现，见 drag_start/drag_follow/end_drag）——
    /// 一次拖动进行中（bar 上 drag_started → drag_stopped）。期间 Auto 不贴顶、不收起。
    dragging: bool,
    /// 拖动开始那帧的抓取点（窗内坐标）。拖动中每帧把窗口"钉"在抓取点随光标走。
    drag_grab: Option<egui::Pos2>,
    /// 拖动开始那帧的窗体左上角（松手时比它位移 >1px 才算真拖动；只在 bar 上"点一下"不动 → 不吸附不收线）。
    drag_origin: Option<egui::Pos2>,
    // —— 常规模式位置心跳 ——
    /// 上次写盘/记录的窗体左上角（逻辑像素）；None = 还没记录。
    last_pos: Option<[f32; 2]>,
    /// 上次心跳写盘的时刻（ctx.time），用于节流（≥0.8s 才写一次）。
    last_save_at: Option<f64>,
}

impl MxdBarApp {
    pub fn new(shared: Arc<Mutex<Shared>>) -> Self {
        // 模式不由 ini 存——由记忆的**退出位置**反推：上次退出 y 落在顶部吸附带内 → Auto
        // 收起贴顶；带外 → 常规。（位置本身在首帧摆放处再读 cfg。）
        let y = shared.lock().unwrap().cfg.panel_y;
        // y<0 是"未记录"哨兵，天然不在 0..=TOP_SNAP 内。
        let near_top = (0.0..=TOP_SNAP).contains(&y);
        Self {
            shared,
            sized: false,
            positioned: false,
            fonts_installed: false,
            panel_h: 0.0,
            icons: None,
            mode: if near_top { PanelMode::Auto } else { PanelMode::Normal },
            auto_open: false,
            auto_leave_at: None,
            auto_h: if near_top { AUTO_STRIP_H } else { BAR_HEIGHT },
            auto_x: None,
            pinned: false,
            meter: 0.0,
            dragging: false,
            drag_grab: None,
            drag_origin: None,
            last_pos: None,
            last_save_at: None,
        }
    }

    /// 当前窗口外框左上角（egui 逻辑坐标）。Windows 下布局后即为 Some；最小化/初始化时可能为 None。
    fn outer_min(&self, ctx: &egui::Context) -> Option<egui::Pos2> {
        ctx.input(|i| i.viewport().outer_rect).map(|r| r.min)
    }

    /// 进入 Auto（常规卡片拖到屏幕顶部松手 / 启动时位置本就在顶部带内）：关抽屉、把"退出位置"
    /// 记成贴顶 (x, 0) 落盘、收成细线。`x` = 松手时的横位。
    fn enter_auto(&mut self, ctx: &egui::Context, x: f32) {
        {
            let mut g = self.shared.lock().unwrap();
            g.page = Page::None; // 收缩态不开抽屉
            // ini 只记退出位置：贴在顶部的 (x, 0) → 下次启动据此再进 Auto。
            g.cfg.panel_x = x;
            g.cfg.panel_y = 0.0;
            g.persist_cfg();
        }
        self.mode = PanelMode::Auto;
        self.auto_open = false; // 先收线，等鼠标悬停再展开
        self.auto_leave_at = None;
        self.auto_h = AUTO_STRIP_H;
        self.pinned = false; // 下一帧贴到顶部
        self.auto_x = Some(x); // 直接用松手横位贴顶
        self.panel_h = 0.0;
        self.last_pos = None;
        self.last_save_at = None;
        ctx.request_repaint();
    }

    /// 退出 Auto 回常规（Auto 展开态点了开抽屉页等需常规态的动作）：把窗口挪到**吸附带外**
    /// 的常规位置。顶部带内没法停常规卡——否则下帧随手一拖又判成"贴顶进 Auto"。优先用 ini 里
    /// 记的位置；它若也落在带内（此前就是在 Auto 里退出、位置是贴顶 (x,0)），就下挪到 y=40
    /// 给抽屉留出空间。模式不再落盘。
    fn exit_auto_restore(&mut self, ctx: &egui::Context) {
        self.mode = PanelMode::Normal;
        self.auto_open = false;
        self.auto_leave_at = None;
        self.auto_h = BAR_HEIGHT;
        self.pinned = false;
        self.auto_x = None;
        // 直接把窗口挪到带外的常规位置——若丢给常规分支等"尺寸稳定再首摆"，开抽屉时会先露
        // 出贴在顶部的那一帧再跳走。拿不到显示器信息（极少）才退回常规分支的首摆兜底。
        let (cfgx, cfgy) = {
            let g = self.shared.lock().unwrap();
            (g.cfg.panel_x, g.cfg.panel_y)
        };
        match ctx.input(|i| i.viewport().monitor_size) {
            Some(mon) => {
                let x = if cfgx >= 0.0 {
                    cfgx
                } else {
                    ((mon.x - BAR_W) * 0.5).max(0.0)
                };
                // 常规卡绝不放进顶部吸附带（≤TOP_SNAP 的位置留给出 Auto 用）。
                let y = if cfgy > TOP_SNAP { cfgy } else { 40.0 };
                ctx.send_viewport_cmd(ViewportCommand::OuterPosition(pos2(x, y)));
                self.positioned = true; // 已亲自摆放，常规分支不再重摆
                self.last_pos = Some([x, y]);
                self.last_save_at = None;
                self.persist_pos(x, y); // 记成新的"退出位置"
            }
            None => {
                self.positioned = false;
                self.sized = false;
            }
        }
        ctx.request_repaint();
    }

    /// 把窗体左上角写进 ini `[panel]`（唯一持久化的面板状态：退出位置）。
    fn persist_pos(&mut self, x: f32, y: f32) {
        let mut g = self.shared.lock().unwrap();
        g.cfg.panel_x = x;
        g.cfg.panel_y = y;
        g.persist_cfg();
    }

    /// 拖动开始：记下抓取点（窗内坐标）与此刻窗位，由 `drag_follow` 每帧把窗口钉住抓取点随光标走。
    fn drag_start(&mut self, ctx: &egui::Context) {
        self.dragging = true;
        self.drag_grab = ctx.pointer_latest_pos();
        self.drag_origin = self.outer_min(ctx);
        #[cfg(debug_assertions)]
        eprintln!("[mxd-bar] 开始拖动卡片");
        ctx.request_repaint();
    }

    /// 拖动中每帧：把窗口左上角挪到"让抓取点仍压在光标下"的位置。这是**位置伺服**而非位移
    /// 累加——每次用 eframe 如实上报的当前窗位 outer_min 重新对准目标，丢帧/漏事件也不会漂移；
    /// 落点也因此始终精确可知（end_drag 据此进/出 Auto）。
    fn drag_follow(&mut self, ctx: &egui::Context) {
        let Some(grab) = self.drag_grab else { return };
        let Some(win) = self.outer_min(ctx) else { return };
        let Some(p) = ctx.pointer_latest_pos() else { return };
        let t = win + (p - grab);
        ctx.send_viewport_cmd(ViewportCommand::OuterPosition(pos2(
            t.x.round().max(0.0),
            t.y.round().max(0.0),
        )));
        ctx.request_repaint_after(Duration::from_millis(16)); // 拖动中持续跟上，别让光标甩出窗
    }

    /// 拖动松手：最后一次对准落点，然后**按最终位置**决定进/出 Auto——
    /// - 落点落进顶部吸附带 → 进 Auto（贴顶收线；Auto 内横向换位松手也走这里，换横位保持 Auto）；
    /// - 落在带外 → 常规：Auto 展开态拖离顶就停在这儿，常规态拖拽也就此停稳；把落点写回 ini
    ///   作"退出位置"。
    fn end_drag(&mut self, ctx: &egui::Context) {
        if !self.dragging {
            return;
        }
        self.dragging = false;
        let grab = self.drag_grab.take();
        let origin = self.drag_origin.take();
        // 松手帧再对准一次（OuterPosition 帧末才生效，这里直接算出目标用于判定，不读滞后窗位）。
        let mut final_pos: Option<egui::Pos2> = None;
        if let (Some(g), Some(win), Some(p)) = (grab, self.outer_min(ctx), ctx.pointer_latest_pos()) {
            let t = win + (p - g);
            let t = pos2(t.x.round().max(0.0), t.y.round().max(0.0));
            ctx.send_viewport_cmd(ViewportCommand::OuterPosition(t));
            final_pos = Some(t);
        }
        let m = final_pos.or_else(|| self.outer_min(ctx));
        let Some(m) = m else { return };
        // 只在 bar 上"点一下"（没真正位移）→ 不算拖动：Auto 不收线、不吸附，常规不落盘。
        let moved = origin.is_none_or(|o| (o.x - m.x).abs() > 1.0 || (o.y - m.y).abs() > 1.0);
        #[cfg(debug_assertions)]
        if moved {
            eprintln!("[mxd-bar] 拖动落点 y={:.1}（顶部吸附阈值 {}）", m.y, TOP_SNAP);
        }
        if !moved {
            return;
        }

        if m.y <= TOP_SNAP {
            // 落进顶部带（常规拖到顶 / Auto 内横向换位）→ 进 Auto 贴顶收起。
            self.enter_auto(ctx, m.x);
            return;
        }
        if self.mode == PanelMode::Auto {
            // Auto 展开态被拖离顶 → 回常规，停在松手处；不再贴顶锁定。
            self.mode = PanelMode::Normal;
            self.auto_open = false;
            self.auto_leave_at = None;
            self.auto_h = BAR_HEIGHT;
            self.pinned = false;
            self.auto_x = None;
        }
        // 常规落点：已亲手摆好，常规分支首帧别再重摆；落点写回 ini 记作"退出位置"。
        self.positioned = true;
        self.last_pos = Some([m.x, m.y]);
        self.last_save_at = None;
        self.persist_pos(m.x, m.y);
        ctx.request_repaint();
    }

    /// 常规位置心跳：窗体离上次记录 >0.5px 且距上次写盘 ≥0.8s 时，把落点写回 cfg 并落盘。
    /// 常规模式拖动停稳后，下一帧就会走到这里记下最终位置（end_drag 也会立即写一次）。
    fn heartbeat_save_pos(&mut self, ctx: &egui::Context) {
        if self.mode != PanelMode::Normal || !self.positioned {
            return;
        }
        let now = ctx.input(|i| i.time);
        let Some(min) = self.outer_min(ctx) else { return };
        let cur = [min.x, min.y];
        let changed = match self.last_pos {
            Some(p) => (p[0] - cur[0]).abs() > 0.5 || (p[1] - cur[1]).abs() > 0.5,
            None => true,
        };
        if !changed {
            return;
        }
        // 节流：写盘别太密。
        if self.last_save_at.is_some_and(|t| now - t < 0.8) {
            return;
        }
        self.last_pos = Some(cur);
        self.last_save_at = Some(now);
        let mut g = self.shared.lock().unwrap();
        g.cfg.panel_x = cur[0];
        g.cfg.panel_y = cur[1];
        g.persist_cfg();
    }

    /// 取一帧绘制所需的当前页 + 全部显示值（一次加锁快照）。
    fn snapshot(&self) -> (Page, BarData) {
        let g = self.shared.lock().unwrap();
        let page = g.page;
        let now = Local::now().naive_local();
        (
            page,
            BarData {
                // 无样本 / 最近 20s 读不到数据（连续 4 次）→ 显示 `-`，不显示陈旧的 0。
                exp_min: if g.exp.n_hour > 0 {
                    thousands(g.exp.per_min)
                } else {
                    "-".to_owned()
                },
                exp_hour: if g.exp.n_hour > 0 {
                    thousands(g.exp.per_hour)
                } else {
                    "-".to_owned()
                },
                // 测试时间：同上报 test_seconds 算法（窗内采样条数 × 5s），时钟式格式。
                test_time: if g.exp.n_hour > 0 {
                    fmt_clock(
                        g.exp.n_hour as i64 * crate::sensor::sampler::SAMPLE_INTERVAL_SECS as i64,
                    )
                } else {
                    "-".to_owned()
                },
                // 累计经验（当前段净获）：无样本 / 被清空显示 `-`，与其它指标一致。
                exp_cum: if g.exp.n_hour > 0 {
                    thousands(g.exp.cum_gain)
                } else {
                    "-".to_owned()
                },
                // 实时效率 PK：后台 pk 线程填的服务端原样名次；None → 主卡 `-`。
                pk: g.pk.rank,
                // 无样本（含启动/清空后）不画心电图。
                spark: (g.exp.n_hour > 0).then_some(g.exp.spark),
                punch: crate::util::punch_view(now, g.cfg.punch),
                boss: crate::util::boss_view(now, g.cfg.boss),
            },
        )
    }

    /// Auto（收缩/展开）这一帧：指针进出推进状态、贴顶固定、按 auto_open 展开/收线、画卡。
    /// 不在主分支里做：Auto 不开抽屉页，只画 72px 主卡带（或收成 16px 细线）。
    fn ui_auto(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, data: &BarData) {
        let painter = ui.painter().clone();
        let pal = Palette::dark();
        let time = ctx.input(|i| i.time);

        // —— 指针进出（事件驱动：PointerMoved=在窗内、PointerGone=离开窗口）——
        // 用事件而非某控件 hovered()：既管收起态那条细线"悬停到即展开"，也避免指针停在
        // 图标/按钮上时把"还在窗内"误判成"离开"。egui-winit 只在指针跨入/跨出窗口时给这两类
        // 事件，因此在窗内静止不动不会误触发收线计时。
        if self.dragging {
            // 拖动中（Auto 展开态被抓住在拖）：始终保持在展开态，别因指针瞬时出窗/停顿而收线。
            self.auto_open = true;
            self.auto_leave_at = None;
        }
        let moved_in = ctx
            .input(|i| i.events.iter().any(|e| matches!(e, egui::Event::PointerMoved(_))));
        let gone_out = ctx
            .input(|i| i.events.iter().any(|e| matches!(e, egui::Event::PointerGone)));
        if moved_in {
            self.auto_open = true;
            self.auto_leave_at = None;
        }
        if gone_out && self.auto_leave_at.is_none() {
            self.auto_leave_at = Some(time);
        }
        if self.auto_leave_at.is_some_and(|t0| time - t0 >= AUTO_LEAVE) {
            self.auto_open = false;
            self.auto_leave_at = None;
        }

        // —— 高度：向 展开=BAR_HEIGHT / 收线=AUTO_STRIP_H 缓动 ——
        let target_h = if self.auto_open { BAR_HEIGHT } else { AUTO_STRIP_H };
        let dt = ctx.input(|i| i.stable_dt).clamp(0.0, 0.1);
        if dt > 0.0 {
            let k = 1.0 - (-AUTO_K * dt).exp();
            self.auto_h += (target_h - self.auto_h) * k;
            if (target_h - self.auto_h).abs() < 0.3 {
                self.auto_h = target_h;
            }
        }

        // —— 进度条长度动画：目标 = 最新 5s 值 ÷ 窗内动态最大值（仅线态才跟；展开态退回 0）——
        let meter_on = self.auto_h <= STRIP_WAVE_MAX;
        let meter_target = if meter_on {
            data.spark.map_or(0.0, |g| meter_frac(&g))
        } else {
            0.0
        };
        let meter_moving = (self.meter - meter_target).abs() > 0.004;
        if dt > 0.0 {
            let k = 1.0 - (-METER_K * dt).exp();
            self.meter += (meter_target - self.meter) * k;
            if (meter_target - self.meter).abs() < 0.004 {
                self.meter = meter_target;
            }
        }

        // —— 画卡片：全高圆角底 + 描边；999 待打卡时描边红色呼吸脉冲 ——
        // 圆角随高度收敛：还不到整卡一半高(40)时算"细线"，用 4 的小圆角，避免 CARD_RADIUS(14)
        // 在矮条上超过半高、把上下边整个顶成胶囊形；接近整卡高才用整卡圆角。
        let win = ui.max_rect();
        let radius = if self.auto_h < 40.0 { 4 } else { CARD_RADIUS };
        // 底卡 = 收线原来的颜色：给下方绿色进度条当**轨道/背景**（见 paint_strip_meter）。
        painter.rect_filled(win, radius, pal.bg);
        let pulse = data.punch.red;
        let border = if pulse {
            let k = (0.5 + 0.5 * (time * 4.2).sin()) as f32; // 0..1 正弦
            let a = (0.35 + 0.45 * k) as u8; // alpha 35%..80% 呼吸
            Color32::from_rgba_unmultiplied(
                pal.danger.r(),
                pal.danger.g(),
                pal.danger.b(),
                (pal.danger.a() as u32 * a as u32 / 255) as u8,
            )
        } else {
            pal.border
        };
        let stroke_w = if pulse { 2.0 } else { 1.0 };
        painter.rect_stroke(win, radius, Stroke::new(stroke_w, border), StrokeKind::Inside);

        // —— 线态：画"会呼吸、泛光的渐变能量条"。条长已在上方缓动进 self.meter
        // （目标 = 最新 5s 值 ÷ 窗内动态最大，见 meter_frac / METER_K），m≈0 就露轨道不画 ——
        if self.auto_h <= STRIP_WAVE_MAX && self.meter > 0.001 {
            paint_strip_meter(&painter, &win, self.meter, time);
        }

        // —— 高度基本到满格才画主卡内容（图标/按钮这时才可点）；收线/展开过程中只画底卡 ——
        if self.auto_h >= BAR_HEIGHT - 2.0 {
            let band = Rect::from_min_size(win.min, vec2(win.width(), BAR_HEIGHT));
            self.show_bar(ui, ctx, data, band);
        }

        // 若这一帧内有动作把模式切回了常规（Auto 展开态点了开抽屉页等），Auto 的贴顶/尺寸
        // 到此为止，交给下一帧常规分支接管，避免本帧再把窗口收细、造成跳变。
        if self.mode != PanelMode::Auto {
            ctx.request_repaint();
            return;
        }

        // —— 贴到顶部：x 沿用进入 Auto 时的常规位置 x（没记录则主屏居中）——
        let mon = ctx.input(|i| i.viewport().monitor_size);
        if let (None, Some(mon)) = (self.auto_x, mon) {
            let cfgx = self.shared.lock().unwrap().cfg.panel_x;
            self.auto_x = Some(if cfgx >= 0.0 {
                cfgx
            } else {
                ((mon.x - BAR_W) * 0.5).max(0.0)
            });
        }
        if let Some(x) = self.auto_x {
            let cur_min = self.outer_min(ctx);
            let drifted = cur_min.is_none_or(|m| (m.x - x).abs() > 1.5 || m.y.abs() > 1.5);
            // 拖动进行中绝不贴顶：本窗口拖动是自己用 OuterPosition 挪的，贴顶会把刚拖离的
            // 窗口立刻拽回 y=0。
            if !self.dragging && (!self.pinned || drifted) {
                // 重新贴一次顶部（x 固定、y=0）；随后 OS 已就位就不再重发。
                ctx.send_viewport_cmd(ViewportCommand::OuterPosition(pos2(x, 0.0)));
                self.pinned = true;
            }
        }

        // —— 尺寸下发：宽度固定 BAR_W，高度随 auto_h ——
        let cur = win.size();
        let want_h = self.auto_h.round();
        if (BAR_W - cur.x).abs() > 0.5 || (want_h - cur.y).abs() > 0.5 {
            ctx.send_viewport_cmd(ViewportCommand::InnerSize(vec2(BAR_W, want_h)));
        }

        // —— 重绘节奏：条长还在缓动/高度动画 → 高刷跟手；收线条还活着（有样本）→ ~20fps
        // 维持"呼吸 + 光斑漂移"的波动感；展开悬停 → 稍频；彻底静止 → 低频（进窗靠事件唤醒）——
        let animating = (self.auto_h - target_h).abs() > 0.5;
        let meter_alive = meter_on && self.meter > 0.001;
        let interval = if pulse {
            Duration::from_millis(33)
        } else if animating || meter_moving {
            Duration::from_millis(16)
        } else if meter_alive {
            Duration::from_millis(50)
        } else if self.auto_open {
            Duration::from_millis(120)
        } else {
            Duration::from_millis(250)
        };
        ctx.request_repaint_after(interval);
    }

    /// 绘制主卡内容（返回固定总宽 BAR_W）。外框恒定，内部文案在各自固定区域内自适应。
    /// 只画 72px 高的主卡带；整卡背景与下方的抽屉由 `ui()` 统一处理。
    fn show_bar(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, data: &BarData, bar: Rect) -> f32 {
        let painter = ui.painter().clone();
        let pal = Palette::dark();
        let time = ctx.input(|i| i.time);

        // —— 字体 ——
        let font_label = FontId::proportional(13.0);
        let font_value = FontId::monospace(20.0);
        let font_btn = FontId::proportional(12.0);

        // 测量文本（颜色对尺寸无影响，统一传白）。
        let measure = |text: &str, font: &FontId| {
            painter
                .layout_no_wrap(text.to_owned(), font.clone(), Color32::WHITE)
                .size()
        };

        // —— 指标区：4 个"标签在上、数值在下"的等宽列，标签/取值/取色一一对应 ——
        let labels = ["实时EXP/分", "预估EXP/时", "测试时间", "累计经验"];
        let val_strs = [
            data.exp_min.as_str(),
            data.exp_hour.as_str(),
            data.test_time.as_str(),
            data.exp_cum.as_str(),
        ];
        let val_colors = [
            pal.exp_per_min,
            pal.exp_per_hour,
            pal.test_time,
            pal.exp_cum,
        ];
        let mut lsz = [egui::Vec2::ZERO; METRIC_N];
        for i in 0..METRIC_N {
            lsz[i] = measure(labels[i], &font_label); // 标签宽度恒定：只跟固定文案与字号有关
        }

        // —— 按钮区：四个按钮共用同一个固定格宽（用户 2026-09-07：宽度按"999打卡 11h"一档统一）。
        // 基准 = 各按钮可能最宽文案（sizing_text）的最大者，作为统一格宽的文字宽度；四格等宽、
        // 主卡总宽因此恒定，不再随 live 数字/倒计时变化。真正显示的文案另测，仅在格内居中。
        let rows = Action::rows();
        let base_w = rows
            .iter()
            .flatten()
            .map(|a| measure(sizing_text(*a), &font_btn).x)
            .fold(0.0, f32::max);
        let cell_w = base_w + BTN_PAD_X; // 四格共用同一格宽（两侧留白合计 BTN_PAD_X）
        let btns_w = cell_w * rows[0].len() as f32 + BTN_COL_GAP * (rows[0].len() - 1) as f32;
        // live 文案尺寸：只用于各自按钮格内居中，不影响格子宽度。
        let live_sizes: Vec<Vec<egui::Vec2>> = rows
            .iter()
            .map(|row| row.iter().map(|a| measure(button_text(*a, data), &font_btn)).collect())
            .collect();
        let btns_h = 2.0 * BTN_H + BTN_ROW_GAP;
        let btns_top = (BAR_HEIGHT - btns_h) * 0.5;

        // —— 指标列宽：由固定总宽 BAR_W 反推（宽度恒定，不随数值位数变）。
        // 总宽 = 2*PAD_X + PK_W（最左 PK 区）+ METRIC_N*col_w
        //      + 2*COL_GAP*(METRIC_N+2)（PK/指标、指标列间、指标/心电 的分隔线槽）
        //      + WAVE_W + btns_w + RAIL_GAP + ICON_W（最右 2×2 工具格）。
        let div_slots = (METRIC_N + 2) as f32;
        let col_w = ((BAR_W - 2.0 * PAD_X - PK_W - 2.0 * COL_GAP * div_slots
            - WAVE_W - btns_w - RAIL_GAP - ICON_W)
            / METRIC_N as f32)
            .max(METRIC_MIN_W)
            .floor();

        // —— 指标数值：字号收敛到列宽内完整放下（默认 20px；数字过长自动缩小，宽度不受影响）——
        let max_val_w = col_w - COLPAD;
        let mut vfont: [FontId; METRIC_N] = std::array::from_fn(|_| font_value.clone());
        let mut vsz = [egui::Vec2::ZERO; METRIC_N];
        for i in 0..METRIC_N {
            vfont[i] = fit_font(&painter, val_strs[i], &font_value, max_val_w);
            vsz[i] = measure(val_strs[i], &vfont[i]);
        }

        let label_h = lsz.iter().map(|s| s.y).fold(0.0, f32::max);
        let value_h = vsz.iter().map(|s| s.y).fold(0.0, f32::max);
        let content_h = label_h + VAL_GAP + value_h;
        let content_top = ((BAR_HEIGHT - content_h) * 0.5).max(4.0);

        // —— 横向坐标：PK 区 → METRIC_N 个指标列（列间各一条分隔线）→ 心电区 → 按钮区 → 工具格 ——
        // 分隔线位置全收进 divs：0 = PK区|指标0，1..=N-1 = 指标列间，N = 末指标|心电，N+1 = 心电|按钮。
        let mut cols = [Rect::ZERO; METRIC_N];
        let mut divs = [0.0f32; METRIC_N + 2];
        let mut cursor = bar.left() + PAD_X;
        let pk_rect = Rect::from_min_size(pos2(cursor, bar.top()), vec2(PK_W, BAR_HEIGHT));
        cursor = pk_rect.right() + COL_GAP;
        divs[0] = cursor; // PK 区 / 指标区 分隔线
        cursor += COL_GAP;
        for i in 0..METRIC_N {
            cols[i] = Rect::from_min_size(pos2(cursor, bar.top()), vec2(col_w, BAR_HEIGHT));
            cursor = cols[i].right() + COL_GAP;
            divs[i + 1] = cursor; // 分隔线（居中于自己的间隙槽）
            cursor += COL_GAP;
        }
        let wave_rect = Rect::from_min_size(pos2(cursor, bar.top()), vec2(WAVE_W, BAR_HEIGHT));
        cursor = wave_rect.right() + COL_GAP;
        divs[METRIC_N + 1] = cursor; // 心电区 / 按钮区 分隔线
        cursor += COL_GAP;
        let btns_left = cursor;
        let grid_left = btns_left + btns_w + RAIL_GAP;
        // 总宽固定（外框以 BAR_W 为准）。
        let total_w = BAR_W;

        // —— 指标区文字 ——
        for i in 0..METRIC_N {
            paint_metric(&painter, &cols[i], content_top, &lsz[i], labels[i], font_label.clone(), &vsz[i], val_strs[i], vfont[i].clone(), pal.label, val_colors[i]);
        }

        // —— PK 区：上方标题"实时效率PK"（与指标标签同顶、居中于本区），下方名次标签 ——
        let pk_label = "实时效率PK";
        let pk_cx = pk_rect.center().x;
        let pk_lsz = measure(pk_label, &font_label);
        painter.text(
            pos2(pk_cx - pk_lsz.x * 0.5, content_top),
            Align2::LEFT_TOP,
            pk_label,
            font_label.clone(),
            pal.label,
        );
        paint_pk_value(&painter, pk_cx, content_top + label_h + VAL_GAP, data.pk, &pal, PK_W);

        // —— 心电区：近 1 分钟"每 5s 净增 EXP"相对该分钟均值的波动折线（无样本不画）——
        if let Some(gains) = data.spark {
            paint_wave(&painter, &wave_rect, &gains, &pal);
        }

        // —— 分隔线 ——
        let div_top = bar.top() + 12.0;
        let div_bot = bar.bottom() - 12.0;
        let div_stroke = Stroke::new(1.0, pal.divider);
        for &dx in &divs {
            painter.line_segment([pos2(dx, div_top), pos2(dx, div_bot)], div_stroke);
        }

        // —— 交互 ——
        // 先注册"整条拖动"（只主卡带可拖动，抽屉内容不抢）：按住空白处拖动窗口。
        // 两种模式都能拖，进/出 Auto 完全由**松手落点**决定（见 end_drag）。OS 标题栏拖拽
        // (StartDrag) 在此置顶/无边框/透明窗上实测拖不动，故自己用 drag_follow 每帧发
        // OuterPosition 跟随光标。Auto 收缩细线态不画主卡带、无 drag → 天然固定不可拖动。
        let drag_id = ui.id().with("bar_drag");
        let drag = ui.interact(bar, drag_id, Sense::drag());
        if drag.drag_started() {
            self.drag_start(ctx);
        }
        if drag.dragged() {
            self.drag_follow(ctx);
        }
        if drag.drag_stopped() {
            self.end_drag(ctx);
        }

        // 两行按钮（后注册的在顶层，点击优先交给按钮）。
        for (r, row) in rows.iter().enumerate() {
            let mut cx = btns_left;
            for (c, act) in row.iter().enumerate() {
                let text = button_text(*act, data);
                let tsize = &live_sizes[r][c];
                let cell = Rect::from_min_size(
                    pos2(cx, bar.top() + btns_top + r as f32 * (BTN_H + BTN_ROW_GAP)),
                    vec2(cell_w, BTN_H),
                );
                let id = ui.id().with(("mxd_btn", r, c));
                let resp = ui.interact(cell, id, Sense::click());
                if resp.clicked() {
                    self.on_action(ctx, *act);
                }
                let timer = timer_for(*act, data);
                paint_button(&painter, &cell, &resp, text, tsize, &pal, timer, time, act.primary());
                cx += cell_w + BTN_COL_GAP;
            }
        }

        // —— 最右 2×2 工具格：上行 日志·关闭，下行 关于·重置（用户 2026-09-07：close 与 info 互换）——
        // 图标 PNG 先拷成局部数组（TextureId 是 Copy），免得借用 self.icons 时再可变调 self.on_action。
        let grid: [[(Action, egui::TextureId); 2]; 2] = {
            let icons = self.icons.as_ref().expect("工具图标已在 ui() 首帧加载");
            [
                [(Action::Log, icons.debug.id()), (Action::Exit, icons.close.id())],
                [(Action::About, icons.info.id()), (Action::Refresh, icons.reload.id())],
            ]
        };
        let row0_y = bar.top() + btns_top;
        let row1_y = row0_y + BTN_H + BTN_ROW_GAP;
        let row_centers = [row0_y + BTN_H * 0.5, row1_y + BTN_H * 0.5];
        let uv = Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0));
        for (r, row) in grid.iter().enumerate() {
            for (c, &(act, tex)) in row.iter().enumerate() {
                let cx = grid_left + c as f32 * (ICON_C + ICON_GAP_X) + ICON_C * 0.5;
                let cell = Rect::from_center_size(pos2(cx, row_centers[r]), vec2(ICON_C, ICON_C));
                let resp = ui.interact(cell, ui.id().with(("mxd_tool", r, c)), Sense::click());
                if resp.clicked() {
                    self.on_action(ctx, act);
                }
                paint_icon_hover(&painter, &cell, &resp);
                painter.image(tex, cell, uv, Color32::WHITE);
            }
        }

        total_w
    }

    /// 按钮点击入口。
    fn on_action(&mut self, ctx: &egui::Context, action: Action) {
        // Auto（悬停展开）态下，除 关闭/重置/管理（只管开外链，无需常规窗口）外，其余动作
        // 都要先退回常规再执行：开抽屉页（上报/计时/日志/关于）需要一个完整的常规窗口；
        // 这也隐含"Auto 里点这些 = 退出 Auto"。
        let wants_normal = !matches!(action, Action::Exit | Action::Refresh | Action::Manage);
        if self.mode == PanelMode::Auto && wants_normal {
            self.exit_auto_restore(ctx);
        }
        match action {
            Action::Report => {
                let mut g = self.shared.lock().unwrap();
                if g.page == Page::Report {
                    g.page = Page::None; // 再点一次 = 收起抽屉
                } else {
                    g.init_report(); // 从 data.ini 带出默认值 + 缓存当前 预估EXP/时
                    g.page = Page::Report;
                }
                drop(g);
                ctx.request_repaint();
            }
            Action::Log => {
                // 控制台式 EXP 日志页：与 Report 同款 toggle（开/关抽屉）。
                let mut g = self.shared.lock().unwrap();
                if g.page == Page::Log {
                    g.page = Page::None;
                } else {
                    g.page = Page::Log;
                }
                drop(g);
                ctx.request_repaint();
            }
            Action::Exit => {
                #[cfg(debug_assertions)]
                eprintln!("[mxd-bar] 点击 X，退出");
                ctx.send_viewport_cmd(ViewportCommand::Close);
            }
            Action::Refresh => {
                // 清空 实时/预估经验队列：立即把读数置 `-`，并置请求标志让采样线程清掉
                // 内部积累的 (时间戳,EXP) 样本链（见 state.rs clear_queue / sampler 消费）。
                #[cfg(debug_assertions)]
                eprintln!("[mxd-bar] 清空经验采样队列");
                let mut g = self.shared.lock().unwrap();
                g.clear_queue = true;
                g.exp = ExpMetrics::default();
                drop(g);
                ctx.request_repaint();
            }
            Action::About => {
                // 「关于」页：版本 + 官网，与 Report/Log 同款 toggle（开/关抽屉）。
                let mut g = self.shared.lock().unwrap();
                if g.page == Page::About {
                    g.page = Page::None;
                } else {
                    g.page = Page::About;
                }
                drop(g);
                ctx.request_repaint();
            }
            Action::Punch | Action::Boss => {
                let kind = match action {
                    Action::Punch => TimePickKind::Punch,
                    Action::Boss => TimePickKind::Boss,
                    _ => unreachable!(),
                };
                let now = Local::now();
                let nt = now.time();
                let mut g = self.shared.lock().unwrap();
                if g.page == Page::TimePick(kind) {
                    g.page = Page::None; // 再点一次 = 收起
                } else {
                    // 默认值 = 点击按钮那一刻的时分。
                    g.pick = PickState { h: nt.hour(), m: nt.minute() };
                    g.page = Page::TimePick(kind);
                }
                drop(g);
                ctx.request_repaint();
            }
            Action::Manage => {
                // 打开管理数据页：带当前设备 token（v2，网页据此授权本设备编辑）
                let url = {
                    let g = self.shared.lock().unwrap();
                    let base = crate::net::api_base(&g.cfg);
                    crate::net::manage_url(base, g.token.as_deref())
                };
                crate::net::open_url(&url);
            }
        }
    }
}

impl eframe::App for MxdBarApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        // 拖动兜底：正常 egui 会在松手那帧给 drag_stopped → end_drag 已把 dragging 复位。
        // 若光标短暂甩出窗外导致 egui 没收到松手、而主键已抬起（拖动中我们在持续重绘、能看到
        // 按键状态），这里也结束拖动，避免窗口卡在"跟手"状态。
        if self.dragging && !ctx.input(|i| i.pointer.primary_down()) {
            self.end_drag(&ctx);
        }
        if !self.fonts_installed {
            crate::theme::install_fonts(&ctx);
            crate::theme::apply_ui_style(&ctx);
            self.fonts_installed = true;
        }
        // 工具图标纹理需要 ctx，首帧建一次。
        if self.icons.is_none() {
            self.icons = Some(Icons::load(&ctx));
        }

        // —— 当前页 + 一帧要显示的全部值（一次快照）——
        let (page, data) = self.snapshot();
        let open = page != Page::None;

        // —— Auto：贴顶收缩/悬停展开，不走下方普通分支 ——
        if self.mode == PanelMode::Auto {
            self.ui_auto(ui, &ctx, &data);
            return;
        }

        // ---- Normal 常规模式 ----
        let win = ui.max_rect(); // 当前实际窗口内容区
        let painter = ui.painter().clone();
        let pal = Palette::dark();

        // —— 整卡背景：随窗口高度一起圆角（下方抽屉展开时卡片自然变高）——
        painter.rect_filled(win, CARD_RADIUS, pal.bg);
        painter.rect_stroke(win, CARD_RADIUS, Stroke::new(1.0, pal.border), StrokeKind::Inside);

        // —— 主卡内容（顶部 72px 带）——
        let band = Rect::from_min_size(win.min, vec2(win.width(), BAR_HEIGHT));
        let total_w = self.show_bar(ui, &ctx, &data, band);

        // —— 抽屉内容：有页面时先真正排布一次，量出自然高度 ——
        // 放在一个"够高"的子 Ui 里（MEASURE_H），把内容完整铺出来；egui 会按窗口裁剪，
        // 窗口暂不够高时看不见的部分下一帧随面板长出来即可。量到的 min_rect 高度即内容高。
        let mut used_h = 0.0f32;
        if open {
            // 抽屉整幅通排但加大对称留白：左右 DRAWER_PAD_X、上下 DRAWER_MARGIN_Y 各一致。
            let col_x = win.left() + DRAWER_PAD_X;
            let col_w = (win.width() - 2.0 * DRAWER_PAD_X).max(0.0);
            let start_y = win.top() + BAR_HEIGHT + DRAWER_MARGIN_Y;
            let shared = Arc::clone(&self.shared);
            let _ = ui.scope_builder(
                UiBuilder::new()
                    .id_salt("drawer_page")
                    .max_rect(Rect::from_min_size(pos2(col_x, start_y), vec2(col_w, MEASURE_H)))
                    .layout(egui::Layout::top_down(egui::Align::Min)),
                |ui| {
                    match page {
                        Page::None => {}
                        Page::Report => report::page(ui, &shared),
                        Page::TimePick(kind) => pickers::page(ui, &shared, kind),
                        Page::Log => log::page(ui, &shared),
                        Page::About => about::page(ui, &shared),
                    }
                    used_h = ui.min_rect().height();
                },
            );
            // 主卡与抽屉之间一条浅分隔线
            painter.line_segment(
                [
                    pos2(win.left(), win.top() + BAR_HEIGHT),
                    pos2(win.right(), win.top() + BAR_HEIGHT),
                ],
                Stroke::new(1.0, pal.divider),
            );
        }

        // —— 面板高度目标：页面上沿在 BAR_HEIGHT + DRAWER_MARGIN_Y（顶留白），
        // 窗口还需 used_h 加上对称的底留白 DRAWER_MARGIN_Y。总 = used_h + 2 * DRAWER_MARGIN_Y。
        let target_h = if open {
            (used_h + 2.0 * DRAWER_MARGIN_Y).max(0.0)
        } else {
            0.0
        };

        // —— 抽屉高度动画：指数趋近目标 ——
        let dt = ctx.input(|i| i.stable_dt).clamp(0.0, 0.1);
        if dt > 0.0 {
            let k = 1.0 - (-PANEL_K * dt).exp();
            self.panel_h += (target_h - self.panel_h) * k;
            if (target_h - self.panel_h).abs() < 0.05 {
                self.panel_h = target_h;
            }
        }

        // —— 宽度自适应 / 尺寸下发（取整像素，避免小数来回抖动）——
        let want_w = if open { total_w.max(DRAWER_MIN_W) } else { total_w };
        let want_h = (BAR_HEIGHT + self.panel_h).round();
        let want_w = want_w.round();
        let cur = win.size();
        if (want_w - cur.x).abs() > 0.6 || (want_h - cur.y).abs() > 0.6 {
            ctx.send_viewport_cmd(ViewportCommand::InnerSize(vec2(want_w, want_h)));
            self.sized = false;
        } else {
            self.sized = true;
        }

        // —— 首帧/切回常规后：把窗口放到保存的常规位置（没记录过 → 主屏顶部居中 y=40）——
        if self.sized && !self.positioned {
            if let Some(mon) = ctx.input(|i| i.viewport().monitor_size) {
                let (cfgx, cfgy) = {
                    let g = self.shared.lock().unwrap();
                    (g.cfg.panel_x, g.cfg.panel_y)
                };
                let x = if cfgx >= 0.0 {
                    cfgx
                } else {
                    ((mon.x - want_w) * 0.5).max(0.0)
                };
                let y = if cfgy >= 0.0 { cfgy } else { 40.0 };
                ctx.send_viewport_cmd(ViewportCommand::OuterPosition(pos2(x, y)));
                self.positioned = true;
                self.last_pos = Some([x, y]);
                self.last_save_at = None;
            }
        }

        // 常规位置心跳：拖动停稳后把最终落点写回 ini。
        self.heartbeat_save_pos(&ctx);

        // —— 心跳：动画期间高频重绘让它"滑"出来；其余 1s 一次刷计时/速率 ——
        let animating = (self.panel_h - target_h).abs() > 0.5;
        let interval = if animating {
            Duration::from_millis(16)
        } else {
            Duration::from_secs(1)
        };
        ctx.request_repaint_after(interval);
    }

    /// 返回清屏颜色。全透明才能让圆角卡片外的窗口区域“消失”。
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        [0.0, 0.0, 0.0, 0.0]
    }

    /// 退出兜底：模式/位置在切换点已各自落盘，这里再把当前 cfg 写一次收尾。
    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        let g = self.shared.lock().unwrap();
        let _ = crate::config::save(&g.cfg);
    }
}

/// 按钮显示文本：两个计时按钮实时变化，其余用固定标签。
fn button_text(act: Action, data: &BarData) -> &str {
    match act {
        Action::Punch => &data.punch.text,
        Action::Boss => &data.boss.text,
        _ => act.label(),
    }
}

fn timer_for(act: Action, data: &BarData) -> Option<&TimerUi> {
    match act {
        Action::Punch => Some(&data.punch),
        Action::Boss => Some(&data.boss),
        _ => None,
    }
}

/// 按钮排版基准文案：布局以"四个按钮中最宽的这一档"为准定**统一格宽**（四格等宽、恒定，
/// 不随 live 变）。因此这套文案须覆盖各按钮会出现的较宽状态，缺了才可能溢出格子。
fn sizing_text(act: Action) -> &'static str {
    match act {
        Action::Report => "上报数据",
        Action::Manage => "管理数据",
        // 999 的基准用"11h"这一档（用户 2026-09-07：四格宽度按它统一）——比"无"略宽，
        // 覆盖 999 大多数倒计时形态；真正最宽文案由四者取最大兜底，不会溢出。
        Action::Punch => "999打卡 11h",
        Action::Boss => "BOSS 无",
        // 不在按钮行里（最右工具格），不参与排版。
        Action::Exit | Action::Refresh | Action::Log | Action::About => "",
    }
}

/// 字号收敛：从基准字号往下（下限 12px）找让 `text` 在 `max_w` 内完整放下的字号。
/// 用于固定列宽下容纳超长数值，宽度不受文本长短影响。
fn fit_font(painter: &egui::Painter, text: &str, base: &FontId, max_w: f32) -> FontId {
    let mut size = base.size;
    loop {
        let font = FontId::new(size, base.family.clone());
        let w = painter
            .layout_no_wrap(text.to_owned(), font.clone(), Color32::WHITE)
            .size()
            .x;
        if w <= max_w || size <= 12.0 {
            return font;
        }
        size -= 1.0;
    }
}

/// 画一个指标区：上标签、下数值，水平居中于 rect，从 content_top 起排布。
#[allow(clippy::too_many_arguments)]
fn paint_metric(
    painter: &egui::Painter,
    rect: &Rect,
    content_top: f32,
    label_size: &egui::Vec2,
    label: &str,
    font_label: FontId,
    value_size: &egui::Vec2,
    value: &str,
    font_value: FontId,
    label_color: Color32,
    value_color: Color32,
) {
    let cx = rect.center().x;
    painter.text(
        pos2(cx - label_size.x * 0.5, content_top),
        Align2::LEFT_TOP,
        label,
        font_label,
        label_color,
    );
    painter.text(
        pos2(cx - value_size.x * 0.5, content_top + label_size.y + VAL_GAP),
        Align2::LEFT_TOP,
        value,
        font_value,
        value_color,
    );
}

/// 名次标签片的配色：第1金（主题橘）→ 第2银 → 第3铜 → 其它走中性按钮色。
fn pk_chip_colors(rank: u32, pal: &Palette) -> (Color32, Color32, Color32) {
    // (片底, 描边, 文字)：前三名片底用同色的极淡半透明，让档位一眼可辨又不抢底。
    let accent = |r: u8, g: u8, b: u8| {
        (
            Color32::from_rgba_unmultiplied(r, g, b, 30),
            Color32::from_rgb(r, g, b),
            Color32::from_rgb(r, g, b),
        )
    };
    match rank {
        1 => accent(255, 152, 51), // 金 = 主题橘
        2 => accent(203, 214, 235), // 银
        3 => accent(222, 156, 96), // 铜
        _ => (pal.btn_bg, pal.btn_border, pal.btn_text),
    }
}

/// 画主卡 PK 区的名次标签（值区）：无名次 → 一枚与其它指标同尺寸的灰 `-`（空态）；
/// 有名次 → 一枚圆角标签片，文字随名次档位上色（第1金/第2银/第3铜/其它中性），
/// 文案超区自动缩字号。名次 >999 的显示封顶见 util::pk_rank_text。
/// `value_top` = 指标数值区顶线（4 个指标列的数值都从这条线起排），片上垂直居中于
/// 20px 数值行（≈value_top+13），视觉上与左右指标数值对齐。
fn paint_pk_value(
    painter: &egui::Painter,
    cx: f32,
    value_top: f32,
    rank: Option<u32>,
    pal: &Palette,
    region_w: f32,
) {
    // 标签片可用宽：区域内左右各留 4px。
    let max_w = region_w - 8.0;

    let Some(rank) = rank else {
        // 无名次：与其它指标一致的 20px 等宽灰 `-`（弱化，作为空态占位）。
        let font = FontId::monospace(20.0);
        let sz = painter
            .layout_no_wrap("-".to_owned(), font.clone(), Color32::WHITE)
            .size();
        painter.text(
            pos2(cx - sz.x * 0.5, value_top),
            Align2::LEFT_TOP,
            "-",
            font,
            Color32::from_rgba_unmultiplied(150, 165, 200, 170),
        );
        return;
    };

    // 字号收敛：文案超区自动缩小，宽度不受文字长短影响（与数值 fit_font 同思路）。
    let text = crate::util::pk_rank_text(Some(rank));
    let mut font = FontId::proportional(14.0);
    loop {
        let w = painter
            .layout_no_wrap(text.clone(), font.clone(), Color32::WHITE)
            .size()
            .x;
        if w <= max_w - 16.0 || font.size <= 11.0 {
            break;
        }
        font.size -= 1.0;
    }
    let sz = painter
        .layout_no_wrap(text.clone(), font.clone(), Color32::WHITE)
        .size();
    let chip_h = 22.0;
    let pad_x = 9.0;
    let chip_w = (sz.x + 2.0 * pad_x).max(chip_h);
    // 垂直：片心放在 20px 数值行的中部（value_top + 13 ≈ 行中心）。
    let chip = Rect::from_center_size(pos2(cx, value_top + 13.0), vec2(chip_w, chip_h));
    let (fill, border, fg) = pk_chip_colors(rank, pal);
    let radius = chip_h * 0.5;
    painter.rect_filled(chip, radius, fill);
    painter.rect_stroke(chip, radius, Stroke::new(1.0, border), StrokeKind::Inside);
    painter.text(chip.center(), Align2::CENTER_CENTER, text, font, fg);
}

/// 画心电图折线：输入为近 1 分钟 12 个"每 5s 净增 EXP"。先减均值（0 落在中线），
/// 再把最大偏离幅度贴到上下边界，因此匀速刷怪时是一条贴中线的平线，某个 5s 波动
/// 明显（BOSS/爆发）就向上凸、不足时向下凹——只表达"相对该分钟的波动"，不带绝对量。
/// `gains` 恒为固定 12 点；调用方在无样本时根本不给 Some，就不会画。
fn paint_wave(painter: &egui::Painter, rect: &Rect, gains: &[i64; SPARK_BUCKETS], pal: &Palette) {
    let n = gains.len();
    let center = rect.center().y;
    let half_amp = (rect.height() * 0.5 - 12.0).max(8.0);

    // 该分钟均值（所有桶取平均，含尚未来得及填数据的更早桶）。
    let mean = gains.iter().map(|&g| g as f64).sum::<f64>() / n as f64;
    // 最大偏离幅度；全为 0/平坦时归 1，避免除零，此时整条线贴中线。
    let maxd = gains
        .iter()
        .map(|&g| (g as f64 - mean).abs() as i64)
        .max()
        .unwrap_or(0)
        .max(1);

    // 中线：0 = 窗口均值（淡一点，示意"波动以这条线为基准"）。
    painter.line_segment(
        [pos2(rect.left() + 2.0, center), pos2(rect.right() - 2.0, center)],
        Stroke::new(1.0, pal.wave_axis),
    );

    let pad = 2.5;
    let x0 = rect.left() + pad;
    let x1 = rect.right() - pad;
    let step_x = if n > 1 { (x1 - x0) / (n - 1) as f32 } else { 0.0 };
    let pts: Vec<egui::Pos2> = gains
        .iter()
        .enumerate()
        .map(|(i, &g)| {
            let dev = (g as f64 - mean) / maxd as f64; // ∈ [-1,1]
            let y = center - (dev as f32) * half_amp; // 高于均值 → 上凸
            pos2(x0 + i as f32 * step_x, y)
        })
        .collect();
    if pts.len() >= 2 {
        painter.add(egui::Shape::line(pts, Stroke::new(1.6, pal.wave)));
    }
}

/// 收线进度条的长度比例（0..1）= 最新一个 5s 增量 ÷ 整个 60s 窗内的**动态最大值**，
/// 供 ui_auto 每帧当作目标去缓动（见 METER_K），避免值跳变时条长一格格地蹦。
/// 例：5 →100%（自身即峰）→ 100 仍 100%（刷新峰）→ 20 时 20% → 30 时 30% ——
/// 高峰只要还活在 60s 窗里就压住比例，滑出窗外（或最新值追平它）才把满格让出来。
fn meter_frac(gains: &[i64; SPARK_BUCKETS]) -> f32 {
    let latest = *gains.last().unwrap_or(&0);
    let dmax = *gains.iter().max().unwrap_or(&0);
    if dmax <= 0 {
        return 0.0; // 无正增量 → 0，露出轨道
    }
    (latest as f64 / dmax as f64).clamp(0.0, 1.0) as f32
}

/// 把 0..255 的三通道（可能带小数）取整成不透明色；越界先夹回，防 u8 环绕。
fn meter_rgb(c: [f32; 3]) -> Color32 {
    Color32::from_rgb(
        c[0].clamp(0.0, 255.0).round() as u8,
        c[1].clamp(0.0, 255.0).round() as u8,
        c[2].clamp(0.0, 255.0).round() as u8,
    )
}

/// 收线进度条**整条**的颜色（不随条上的位置变，整条同一色）：由当前波动值的高低决定——
/// 值越低越红、稍高转黄、接近/追平窗内巅峰才绿。输入比例 = 最新 5s 值 ÷ 窗内动态最大
/// （就是 `frac` 本身），所以条越长颜色也越偏绿，短红条 = 这 5s 偏弱、长绿条 = 正在巅峰，
/// 一眼可辨。
fn meter_hue(frac: f32) -> [f32; 3] {
    let red = [238.0, 84.0, 66.0];
    let yellow = [252.0, 206.0, 66.0];
    let green = [92.0, 226.0, 158.0];
    let t = frac.clamp(0.0, 1.0);
    let (a, b, tt) = if t < 0.5 {
        (red, yellow, t * 2.0)
    } else {
        (yellow, green, (t - 0.5) * 2.0)
    };
    [
        a[0] + (b[0] - a[0]) * tt,
        a[1] + (b[1] - a[1]) * tt,
        a[2] + (b[2] - a[2]) * tt,
    ]
}

/// 收线(线态)专用：把**最新一个 5s 的经验增量**（已缓动到 `frac`）画成一条从左侧生长、
/// 会"呼吸 + 泛光"的能量条。整条颜色由值的高低统一给出（见 meter_hue）：
/// 低 = 红、中 = 黄、高 = 绿；条长 = 该值 ÷ 窗内动态最大，二者同源同步。画面要素——
///  - 立体感：每列下缘再压暗 ~0.84 倍，截面带一点"柱/管"味。
///  - 呼吸感：整条亮度按 sin(时间×2.6) 明灭。
///  - 波动感：一道白色"光斑"每 ~1.4s 从条头漂向条尾（高斯软峰**叠加**在底色上，暗尾也看得清）。
///
/// 背景 = 收线原底色，当"轨道"（见 ui_auto 底卡绘制）。长度 0（frac≤0）→ 不画、露轨道。
/// 例：5 →100%（自身即峰）→ 100 仍 100%（刷新峰）→ 20 时 20%（红）→ 30 时 30%（红）。
fn paint_strip_meter(painter: &egui::Painter, rect: &Rect, frac: f32, time: f64) {
    if frac <= 0.0 {
        return; // 最小长度 0
    }
    // 轨道内缩出边距、别顶到描边/圆角；条从最左起、长度按比例向右。
    let rail = rect.shrink2(vec2(3.0, 2.5));
    let fill_w = (rail.width() * frac).max(1.0);
    let top = rail.top();
    let bottom = rail.bottom();

    // 整条颜色一次算好（跟随缓动中的 frac 一起变，变色也是平滑推过去的）。
    let base = meter_hue(frac);
    // 呼吸亮度：整条在 ~0.6..1.0 间明灭（呼吸感）。
    let breathe = 0.80 + 0.20 * (time * 2.6).sin() as f32;
    // 光斑相位 0..1：0 在条头、1 在条尾，每 ~1.4s 从条头漂完一整条到条尾。
    let phase = (time / 1.4).fract() as f32;

    let n = ((fill_w / 8.0).ceil() as usize).clamp(2, 200);
    let mut mesh = egui::epaint::Mesh::default();
    for i in 0..n {
        let xn = (i as f32 + 0.5) / n as f32; // 本列中心：0=条尾..1=条头（仅决定 x，不影响色）
        let x0 = rail.left() + fill_w * i as f32 / n as f32;
        let x1 = rail.left() + fill_w * (i + 1) as f32 / n as f32;

        // 光斑以"离条头的距离"定位：r=0 头..1 尾，软峰**加法**叠白——暗尾处也看得见亮斑扫过。
        let r = 1.0 - xn;
        let dist = (r - phase).abs();
        let gauss = (-(dist * dist) * 22.0).exp(); // 0..1，峰在光斑中心
        let sheen = 85.0 * gauss;
        let mut top_c = [0.0_f32; 3];
        let mut bot_c = [0.0_f32; 3];
        for ci in 0..3 {
            let v = (base[ci] * breathe + sheen).clamp(0.0, 255.0);
            top_c[ci] = v;
            bot_c[ci] = v * 0.84; // 下缘压暗 → 截面"柱"感
        }
        let i0 = mesh.vertices.len() as u32;
        mesh.colored_vertex(pos2(x0, top), meter_rgb(top_c));
        mesh.colored_vertex(pos2(x0, bottom), meter_rgb(bot_c));
        mesh.colored_vertex(pos2(x1, bottom), meter_rgb(bot_c));
        mesh.colored_vertex(pos2(x1, top), meter_rgb(top_c));
        mesh.add_triangle(i0, i0 + 1, i0 + 2);
        mesh.add_triangle(i0, i0 + 2, i0 + 3);
    }
    painter.add(mesh);
}

/// 画一个按钮，按悬停/按下状态变色；`emph` 决定是否用橘黄重点配色。
/// 计时按钮可叠加红/绿与抖动（文字/描边变色）。
fn paint_button(
    painter: &egui::Painter,
    rect: &Rect,
    resp: &egui::Response,
    text: &str,
    text_size: &egui::Vec2,
    pal: &Palette,
    timer: Option<&TimerUi>,
    time: f64,
    emph: bool,
) {
    // 状态底色：重点=橘黄系，普通=中性暗底。描边也随重点与否变化。
    let (base_bg, base_hover, base_active) = if emph {
        (pal.prim_bg, pal.prim_hover, pal.prim_active)
    } else {
        (pal.btn_bg, pal.btn_hover, pal.btn_active)
    };
    let base_border = if emph { pal.prim_border } else { pal.btn_border };
    let (mut fill, border, text_color) = if resp.is_pointer_button_down_on() {
        (base_active, base_border, pal.btn_text)
    } else if resp.hovered() {
        (base_hover, base_border, pal.btn_text)
    } else {
        (base_bg, base_border, pal.btn_text)
    };
    let mut border = border;
    let mut text_color = text_color;
    // 计时状态覆盖文字/边框颜色（红 = 待打卡/BOSS 就绪）
    let mut jitter = false;
    if let Some(tu) = timer {
        if tu.red {
            border = pal.danger;
            text_color = pal.danger;
            jitter = tu.jitter;
            if resp.hovered() {
                fill = pal.danger; // 待打卡悬停时底色也红一点，更醒目
            }
        }
    }

    // 抖动：横向做小幅正弦偏移，视觉“晃”起来。
    let dx = if jitter {
        ((time * 28.0).sin() * 2.0) as f32
    } else {
        0.0
    };
    let rect = rect.translate(vec2(dx, 0.0));

    // 圆角矩形（圆角 = BTN_RADIUS，不再全胶囊）。常态再叠一条贴顶的淡高光，呈轻微"凸起"。
    let radius = BTN_RADIUS;
    painter.rect_filled(rect, radius, fill);
    painter.rect_stroke(rect, radius, Stroke::new(1.0, border), StrokeKind::Inside);
    if !resp.is_pointer_button_down_on() {
        // 顶部淡高光（白色 ~7%），只在圆弧之间的平直段，避免戳出弧外。
        let gloss = Color32::from_rgba_unmultiplied(255, 255, 255, 18);
        let inner = rect.shrink(1.0);
        painter.line_segment(
            [
                pos2(inner.left() + radius * 0.6, inner.top() + 1.0),
                pos2(inner.right() - radius * 0.6, inner.top() + 1.0),
            ],
            Stroke::new(1.0, gloss),
        );
    }
    painter.text(
        pos2(rect.center().x - text_size.x * 0.5, rect.center().y - text_size.y * 0.5),
        Align2::LEFT_TOP,
        text,
        FontId::proportional(12.0),
        text_color,
    );
}

/// 工具图标悬停时画一点很淡的圆形反馈，让人知道它可点。
fn paint_icon_hover(painter: &egui::Painter, rect: &Rect, resp: &egui::Response) {
    if resp.hovered() || resp.is_pointer_button_down_on() {
        let r = rect.height().min(rect.width()) * 0.5;
        painter.circle_filled(rect.center(), r, Color32::from_rgba_unmultiplied(120, 130, 150, 40));
    }
}
