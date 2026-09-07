//! 999打卡 / BOSS 共用的"时分选择"（抽屉内页面）。
//!
//! 与上报页一样画在主卡下方的抽屉里，主卡保持可见。两种类型只是"确定后写进 cfg 的
//! 字段"不同，编辑缓冲统一放在 `Shared.pick`：
//! - Punch：一次性"下一个 HH:MM"打卡目标（带日期，过点变红待打卡）。
//! - Boss：秒表起算时刻（带日期，超过 99 分钟自动作废）。
//!
//! 页面就一行：左边 时分输入(HH : MM)，右边 确定(橘黄)/取消；不放标题/提示/预览文字。

use std::sync::{Arc, Mutex};

use chrono::Local;
use eframe::egui;
use egui::{vec2, Align, DragValue, Layout, RichText};

use crate::state::{Page, Shared, TimePickKind};

/// 在抽屉区里画该类型的时分选择，由 app.rs 在 `shared.page==TimePick(kind)` 时调用。
pub fn page(ui: &mut egui::Ui, shared: &Arc<Mutex<Shared>>, kind: TimePickKind) {
    let mut confirm = false;
    let mut cancel = false;
    {
        let mut g = shared.lock().unwrap();
        let cur = g.page; // 若 app 已切走本类型则不再画（防御，正常不会到）
        if cur != Page::TimePick(kind) {
            return;
        }

        // —— 一整行就够：左边 HH : MM 输入，右边 确定/取消（确定在最右）——
        // 整行用 allocate_ui_with_layout 圈定单行高度(避免占满测量区)；再内嵌一个右对齐子区
        // 把两个按钮推到行尾。不放标题/提示/预览等文字。
        // 页面上下留白统一由 app.rs 的 DRAWER_MARGIN_Y 给出，这里不再另加外圈空距。
        let row_h = 34.0;
        ui.allocate_ui_with_layout(
            vec2(ui.available_width(), row_h),
            Layout::left_to_right(Align::Center),
            |ui| {
                // 左：HH : MM（范围在控件里就锁死，不用再校验）
                let h_resp = ui.add(DragValue::new(&mut g.pick.h).range(0..=23u32).speed(1.0));
                h_resp.on_hover_text("小时");
                ui.label(RichText::new(":").size(18.0));
                let m_resp = ui.add(DragValue::new(&mut g.pick.m).range(0..=59u32).speed(1.0));
                m_resp.on_hover_text("分钟");

                // 右：确定/取消 推到行尾（先加的确定在最右）。所在子区高度被上行圈定，
                // 不会像顶层 with_layout 那样吃掉测量区整高。
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if ui
                        .add_sized(
                            [72.0, 28.0],
                            egui::Button::new(RichText::new("确定").strong())
                                .fill(crate::theme::ORANGE_DEEP),
                        )
                        .clicked()
                    {
                        confirm = true;
                    }
                    if ui.add_sized([72.0, 28.0], egui::Button::new("取消")).clicked() {
                        cancel = true;
                    }
                });
            },
        );
    }

    if confirm {
        apply(shared, kind);
    }
    if confirm || cancel {
        if let Ok(mut g) = shared.lock() {
            g.page = Page::None;
        }
        ui.ctx().request_repaint(); // 立刻让抽屉收回动画开跑
    }
}

/// 点"确定"：按类型把编辑值写进 cfg，并落盘。
fn apply(shared: &Arc<Mutex<Shared>>, kind: TimePickKind) {
    let mut g = shared.lock().unwrap();
    let t = match chrono::NaiveTime::from_hms_opt(g.pick.h % 24, g.pick.m % 60, 0) {
        Some(t) => t,
        None => return, // 理论上到不了（DragValue 已锁范围）
    };
    let now = Local::now().naive_local();
    match kind {
        TimePickKind::Punch => {
            // 999：总是落到“下一个还没到的 HH:MM”（今天没到→今天；已到→明天）。
            // 这样到点后（红色待打卡）再次点确定，就会自动滚到下一天同一时分，红色清除。
            let nxt = crate::util::next_occurrence(now, t);
            g.cfg.punch = Some((nxt.date(), nxt.time()));
        }
        TimePickKind::Boss => {
            // BOSS：正向秒表，记录当天的设定时分，从该时刻累计（现在时分 − 记录时分）。
            g.cfg.boss = Some((Local::now().date_naive(), t));
        }
    }
    g.persist_cfg();
}
