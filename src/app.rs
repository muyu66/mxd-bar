//! mxd-bar 应用主体：一个置顶、无边框、透明的悬浮卡片。
//!
//! 主卡固定 72px 高；当某页打开（上报数据 / 999打卡·神秘商人·BOSS 的时分选择）时，
//! 卡片在**同一窗口**内向下方平滑展开，页面内容滑出在主卡下方（不再是独立弹框，
//! 主卡也始终可见）。高度按页面实际内容自动适配（测量后动画趋近），宽度按内容自适应。
//!
//! 布局：主卡从左到右 —— ①实时EXP/分 ②预估EXP/时 ③按钮区
//! 按钮区分两行（上报数据/管理数据/BOSS、神秘商人/999打卡），其中只有「上报数据」
//! 是橘黄重点按钮；退出 X 单独放在最右、整带垂直居中——无边框，就是个大一点的 X。
//! 999/商人/BOSS 文本实时变化。数据源统一走 `Shared`（见 state.rs），本模块只读取与绘制。

use std::sync::{Arc, Mutex};
use std::time::Duration;

use chrono::{Local, Timelike};
use eframe::egui;
use egui::{
    pos2, vec2, Align2, Color32, FontId, Rect, Sense, Stroke, StrokeKind, UiBuilder,
    ViewportCommand,
};

use crate::state::{Page, PickState, Shared, TimePickKind};
use crate::theme::Palette;
use crate::ui::{pickers, report};
use crate::util::{thousands, TimerUi};

/// 卡片总高（逻辑像素）。
pub const BAR_HEIGHT: f32 = 72.0;

/// 首次创建窗口的占位宽度，首帧会被自动修正为内容宽度。
pub(crate) const INITIAL_WIDTH: f32 = 480.0;

// —— 布局间距（逻辑像素）——
const PAD_X: f32 = 14.0; // 卡片左右内边距
const COL_GAP: f32 = 9.0; // 分隔线两侧的间距
const VAL_GAP: f32 = 3.0; // 指标区 标签→数值 间距
const COLPAD: f32 = 6.0; // 指标区文字两侧留白

const BTN_H: f32 = 22.0; // 按钮高
const BTN_ROW_GAP: f32 = 6.0; // 按钮行距
const BTN_COL_GAP: f32 = 8.0; // 按钮列距
const BTN_PAD_X: f32 = 14.0; // 每个按钮横向额外留白(两侧合计)
const BTN_RADIUS: u8 = 6;

const CARD_RADIUS: u8 = 14;

/// 退出 X 所在窄列的宽度（X 本身只是文字，无边框/底色）。
const EXIT_W: f32 = 30.0;
/// 退出 X 与按钮区之间留的空隙。
const EXIT_GAP: f32 = 8.0;

/// 抽屉开着时窗口的最小宽度（避免卡片过窄时页面挤压）。
const DRAWER_MIN_W: f32 = 320.0;
/// 高度动画趋近速率（每秒向目标靠近的比例，值越大越跟手）。
const PANEL_K: f32 = 14.0;
/// 测量抽屉内容时给子 Ui 的最大高度：只要足够大让内容按自然高度排布即可，
/// 真正的高度随后由"内容实际占用高度 + 上下留白"决定（被窗口裁剪也无妨）。
const MEASURE_H: f32 = 3000.0;

/// 主卡片上的按钮动作。退出 X 不在按钮行里，单独画在最右。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Report,
    Manage,
    Punch,
    Merchant,
    Boss,
    Exit,
}

impl Action {
    /// 两行按钮（不含退出 X）。每行靠左排布，行宽各自适应。
    /// BOSS 与 999打卡 互换了位置：第一行右列是 BOSS，第二行右列是 999打卡。
    fn rows() -> Vec<Vec<Action>> {
        vec![
            vec![Action::Report, Action::Manage, Action::Boss],
            vec![Action::Merchant, Action::Punch],
        ]
    }

    /// 固定标签（计时按钮的显示文本见 `button_text`，随状态变化）。
    fn label(self) -> &'static str {
        match self {
            Action::Report => "上报数据",
            Action::Manage => "管理数据",
            Action::Punch => "999打卡",
            Action::Merchant => "神秘商人",
            Action::Boss => "BOSS",
            Action::Exit => "X",
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
    pub punch: TimerUi,
    pub merchant: TimerUi,
    pub boss: TimerUi,
}

pub struct MxdBarApp {
    pub shared: Arc<Mutex<Shared>>,
    /// 窗口宽度是否已与内容对齐
    sized: bool,
    /// 是否已把窗口放到屏幕顶部中央
    positioned: bool,
    /// 首帧需安装中文字体 + 控件样式（需要 ctx，故延迟到 ui()）
    fonts_installed: bool,
    /// 当前抽屉展开高度（0=完全收起；动画逼近当页内容测得的高度）
    panel_h: f32,
}

impl MxdBarApp {
    pub fn new(shared: Arc<Mutex<Shared>>) -> Self {
        Self {
            shared,
            sized: false,
            positioned: false,
            fonts_installed: false,
            panel_h: 0.0,
        }
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
                punch: crate::util::punch_view(now, g.cfg.punch),
                merchant: crate::util::merchant_view(now, g.cfg.merchant_time, g.cfg.merchant_ref),
                boss: crate::util::boss_view(now, g.cfg.boss),
            },
        )
    }

    /// 绘制主卡内容（返回测量出的内容总宽）。
    /// 只画 72px 高的主卡带；整卡背景与下方的抽屉由 `ui()` 统一处理。
    fn show_bar(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, data: &BarData, bar: Rect) -> f32 {
        let painter = ui.painter().clone();
        let pal = Palette::dark();
        let time = ctx.input(|i| i.time);

        // —— 字体 ——
        let font_label = FontId::proportional(11.0);
        let font_value = FontId::monospace(20.0);
        let font_btn = FontId::proportional(12.0);

        // 测量文本（颜色对尺寸无影响，统一传白）。
        let measure = |text: &str, font: &FontId| {
            painter
                .layout_no_wrap(text.to_owned(), font.clone(), Color32::WHITE)
                .size()
        };

        // —— 指标区内容 ——
        let label_1 = "实时EXP/分";
        let label_2 = "预估EXP/时";
        let (l1, l2) = (measure(label_1, &font_label), measure(label_2, &font_label));
        let (v1, v2) = (measure(&data.exp_min, &font_value), measure(&data.exp_hour, &font_value));

        // 两个指标区做成同宽，视觉上更整齐。
        let col_w = l1.x.max(v1.x).max(l2.x).max(v2.x) + COLPAD;

        let content_h = l1.y.max(l2.y) + VAL_GAP + v1.y.max(v2.y);
        let content_top = ((BAR_HEIGHT - content_h) * 0.5).max(4.0);

        // —— 按钮区：两行靠左排布、右端对齐（窄行的按钮拉宽补足，见 cells_w）——
        let rows = Action::rows();
        let row_sizes: Vec<Vec<egui::Vec2>> = rows
            .iter()
            .map(|row| row.iter().map(|a| measure(&button_text(*a, data), &font_btn)).collect())
            .collect();
        // 每行各按钮宽度 = 文本 + 横向留白；行宽 = 行内和。
        let row_widths: Vec<f32> = row_sizes
            .iter()
            .map(|sizes| {
                sizes.iter().map(|s| s.x).sum::<f32>()
                    + BTN_PAD_X * sizes.len() as f32
                    + BTN_COL_GAP * (sizes.len().saturating_sub(1)) as f32
            })
            .collect();
        // 两行共用同一行宽（右端对齐）：以最宽的自然行为准（通常就是文字恒定的"上报数据"那行）。
        // 较窄的行多出来的宽度平均分给该行每个按钮 → 神秘商人/999打卡 那行右端与上行对齐，
        // 按钮变宽、文字仍居中；live 文案变长时整区随之加宽，两行依然等宽对齐。
        let btns_w = row_widths.iter().cloned().fold(0.0, f32::max);
        let cells_w: Vec<Vec<f32>> = rows
            .iter()
            .enumerate()
            .map(|(r, row)| {
                let extra = (btns_w - row_widths[r]) / row.len() as f32;
                row.iter()
                    .enumerate()
                    .map(|(c, _)| row_sizes[r][c].x + BTN_PAD_X + extra)
                    .collect()
            })
            .collect();
        let btns_h = 2.0 * BTN_H + BTN_ROW_GAP;
        let btns_top = (BAR_HEIGHT - btns_h) * 0.5;

        // —— 横向坐标：指标区 / 分隔线 / 指标区 / 分隔线 / 按钮区 / 退出 ——
        let x = bar.left() + PAD_X;
        let z1 = Rect::from_min_size(pos2(x, bar.top()), vec2(col_w, BAR_HEIGHT));
        let d1 = z1.right() + COL_GAP; // 分隔线1（居中于自己的间隙槽）
        let z2 = Rect::from_min_size(pos2(d1 + COL_GAP, bar.top()), vec2(col_w, BAR_HEIGHT));
        let d2 = z2.right() + COL_GAP; // 分隔线2
        let btns_left = d2 + COL_GAP;

        // 退出 X：放在最右，占一个 EXIT_W 宽的窄列（X 仅文字，垂直居中于整带）。
        let exit_left = btns_left + btns_w + EXIT_GAP;
        let total_w = exit_left + EXIT_W + PAD_X - bar.left();

        // —— 指标区文字 ——
        paint_metric(&painter, &z1, content_top, &l1, label_1, font_label.clone(), &v1, &data.exp_min, font_value.clone(), pal.label, pal.exp_per_min);
        paint_metric(&painter, &z2, content_top, &l2, label_2, font_label.clone(), &v2, &data.exp_hour, font_value.clone(), pal.label, pal.exp_per_hour);

        // —— 分隔线 ——
        let div_top = bar.top() + 12.0;
        let div_bot = bar.bottom() - 12.0;
        let div_stroke = Stroke::new(1.0, pal.divider);
        painter.line_segment([pos2(d1, div_top), pos2(d1, div_bot)], div_stroke);
        painter.line_segment([pos2(d2, div_top), pos2(d2, div_bot)], div_stroke);

        // —— 交互 ——
        // 先注册"整条拖动"（只主卡带可拖动，抽屉内容不抢）：按住空白处拖动窗口。
        let drag_id = ui.id().with("bar_drag");
        let drag = ui.interact(bar, drag_id, Sense::drag());
        if drag.drag_started() {
            ctx.send_viewport_cmd(ViewportCommand::StartDrag);
        }

        // 两行按钮（后注册的在顶层，点击优先交给按钮）。
        for (r, row) in rows.iter().enumerate() {
            let mut cx = btns_left;
            for (c, act) in row.iter().enumerate() {
                let text = button_text(*act, data);
                let tsize = &row_sizes[r][c];
                let cell_w = cells_w[r][c];
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

        // 退出 X：单独最右一个窄列、整带垂直居中；无边框底色，就是大一点的 X（白字）。
        let exit_cx = exit_left + EXIT_W * 0.5;
        let exit_rect = Rect::from_center_size(pos2(exit_cx, bar.center().y), vec2(EXIT_W, 40.0));
        let exit_id = ui.id().with("mxd_exit");
        let resp = ui.interact(exit_rect, exit_id, Sense::click());
        if resp.clicked() {
            self.on_action(ctx, Action::Exit);
        }
        paint_exit(&painter, &exit_rect, &resp);

        total_w
    }

    /// 按钮点击入口。
    fn on_action(&mut self, ctx: &egui::Context, action: Action) {
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
            Action::Exit => {
                #[cfg(debug_assertions)]
                eprintln!("[mxd-bar] 点击 X，退出");
                ctx.send_viewport_cmd(ViewportCommand::Close);
            }
            Action::Punch | Action::Merchant | Action::Boss => {
                let kind = match action {
                    Action::Punch => TimePickKind::Punch,
                    Action::Merchant => TimePickKind::Merchant,
                    Action::Boss => TimePickKind::Boss,
                    _ => unreachable!(),
                };
                let now = Local::now();
                let nt = now.time();
                let mut g = self.shared.lock().unwrap();
                if g.page == Page::TimePick(kind) {
                    g.page = Page::None; // 再点一次 = 收起
                } else {
                    // 默认值：Punch/BOSS=点击按钮那一刻的时分；Merchant=已有周期或 05:59。
                    let (h, m) = match kind {
                        TimePickKind::Punch | TimePickKind::Boss => (nt.hour(), nt.minute()),
                        TimePickKind::Merchant => g
                            .cfg
                            .merchant_time
                            .map(|t| (t.hour(), t.minute()))
                            .unwrap_or((5, 59)),
                    };
                    g.pick = PickState { h, m };
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
        if !self.fonts_installed {
            crate::theme::install_fonts(&ctx);
            crate::theme::apply_ui_style(&ctx);
            self.fonts_installed = true;
        }

        // —— 当前页 + 一帧要显示的全部值（一次快照）——
        let (page, data) = self.snapshot();
        let open = page != Page::None;

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
            let col_x = win.left() + PAD_X;
            let col_w = (win.width() - 2.0 * PAD_X).max(0.0);
            let start_y = win.top() + BAR_HEIGHT;
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

        // —— 面板高度目标：页面上沿到内容底(used_h) + 顶部/底部呼吸留白 ——
        // 页面上沿在 BAR_HEIGHT，故额外高度 ≈ used_h + 上下留白。
        let target_h = if open { (used_h + 14.0).max(0.0) } else { 0.0 };

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

        // —— 首次对齐后，把窗口放到主屏顶部中央 ——
        if self.sized && !self.positioned {
            if let Some(mon) = ctx.input(|i| i.viewport().monitor_size) {
                let x = ((mon.x - want_w) * 0.5).max(0.0);
                let y = 40.0;
                ctx.send_viewport_cmd(ViewportCommand::OuterPosition(pos2(x, y)));
                self.positioned = true;
            }
        }

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
}

/// 按钮显示文本：三个计时按钮实时变化，其余用固定标签。
fn button_text(act: Action, data: &BarData) -> &str {
    match act {
        Action::Punch => &data.punch.text,
        Action::Merchant => &data.merchant.text,
        Action::Boss => &data.boss.text,
        _ => act.label(),
    }
}

fn timer_for(act: Action, data: &BarData) -> Option<&TimerUi> {
    match act {
        Action::Punch => Some(&data.punch),
        Action::Merchant => Some(&data.merchant),
        Action::Boss => Some(&data.boss),
        _ => None,
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
    // 计时状态覆盖文字/边框颜色（红/绿）
    let mut jitter = false;
    if let Some(tu) = timer {
        if tu.red {
            border = pal.danger;
            text_color = pal.danger;
            jitter = tu.jitter;
            if resp.hovered() {
                fill = pal.danger; // 待打卡悬停时底色也红一点，更醒目
            }
        } else if tu.green {
            border = pal.success;
            text_color = pal.success;
        }
    }

    // 抖动：横向做小幅正弦偏移，视觉“晃”起来。
    let dx = if jitter {
        ((time * 28.0).sin() * 2.0) as f32
    } else {
        0.0
    };
    let rect = rect.translate(vec2(dx, 0.0));

    painter.rect_filled(rect, BTN_RADIUS, fill);
    painter.rect_stroke(rect, BTN_RADIUS, Stroke::new(1.0, border), StrokeKind::Inside);
    painter.text(
        pos2(rect.center().x - text_size.x * 0.5, rect.center().y - text_size.y * 0.5),
        Align2::LEFT_TOP,
        text,
        FontId::proportional(12.0),
        text_color,
    );
}

/// 退出 X：无边框、无底色，就是一个大一点的 X（白字）。悬停时略微加亮并画一点
/// 很淡的圆形反馈，让人知道它可点。
fn paint_exit(painter: &egui::Painter, rect: &Rect, resp: &egui::Response) {
    if resp.hovered() || resp.is_pointer_button_down_on() {
        let r = rect.height().min(rect.width()) * 0.5;
        painter.circle_filled(rect.center(), r, Color32::from_rgba_unmultiplied(120, 130, 150, 40));
    }
    let glyph = if resp.hovered() || resp.is_pointer_button_down_on() {
        Color32::WHITE
    } else {
        Color32::from_rgb(205, 211, 224)
    };
    painter.text(
        rect.center() - vec2(0.0, 1.0),
        Align2::CENTER_CENTER,
        "X",
        FontId::proportional(17.0),
        glyph,
    );
}
