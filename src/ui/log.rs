//! EXP 日志抽屉页（点主卡工具格「日志」图标打开）：控制台式逐行打印采样经验。
//!
//! 格式：`HH:MM:SS EXP 233,222 [+223]` —— 时间 / 截图经验值 / 比上一条记录的增量。
//! 升级（EXP 大跌重置）那一条不带 `[+..]`，直接开新段（其上方画一条细分隔线），
//! 段内每行恢复 `[+Δ]`；点「重置队列」会把整页清空。除此之外不做任何其它功能，
//! 保持页面干净。关闭 = 再点主卡「日志」图标（或其它按钮收起抽屉）。
//!
//! 页面自身是一个**固定高度**（LOG_PAGE_H）的可滚动控制台：内容不足也占满整高，
//! 超过则内部滚动并始终贴底（最新一条在底部）。这样采样每 5s 追加一行也不会让
//! 窗口高度抖动。

use std::sync::{Arc, Mutex};

use eframe::egui;
use egui::{Align2, Color32, FontId, RichText, ScrollArea, Sense};

use crate::state::{exp_log_rows, ExpLogRow, Shared};
use crate::theme;
use crate::util::thousands;

/// 日志页内容区固定高（逻辑像素）。整页高度因此恒定，不随行数伸缩。
const LOG_PAGE_H: f32 = 300.0;
/// 时间列颜色（比正文暗一档，弱化它）。
const TIME_COLOR: Color32 = Color32::from_rgb(128, 144, 176);

/// 在抽屉区画整页日志，由 app.rs 在 `shared.page==Log` 时调用。
pub fn page(ui: &mut egui::Ui, shared: &Arc<Mutex<Shared>>) {
    // 一次快照（拷贝行结构）后立刻放锁，再慢慢画，避免持锁。
    let rows = { exp_log_rows(&shared.lock().unwrap().exp_log) };

    if rows.is_empty() {
        // 固定高的空态提示（与有内容时整页同高，抽屉不跳）。
        let (rect, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), LOG_PAGE_H), Sense::hover());
        ui.painter().text(
            rect.center(),
            Align2::CENTER_CENTER,
            "（暂无记录：采样成功 / 升级时写入；点 ↻ 会清空）",
            FontId::proportional(12.0),
            Color32::from_rgba_unmultiplied(150, 165, 195, 120),
        );
        return;
    }

    ScrollArea::vertical()
        .id_salt("exp_log_scroll")
        .auto_shrink([false, false])
        .max_height(LOG_PAGE_H)
        .stick_to_bottom(true)
        .show(ui, |v| {
            v.spacing_mut().item_spacing = egui::vec2(0.0, 3.0);
            for (i, row) in rows.iter().enumerate() {
                // 升级段头：上方画一条细分隔线，视觉上"开新段"。
                if row.seg && i > 0 {
                    v.add_space(2.0);
                    v.separator();
                    v.add_space(2.0);
                }
                draw_row(v, row);
            }
        });
}

/// 一行：`HH:MM:SS  EXP  233,222  [+223]`。时间暗、增量绿、升级段头那条 EXP 用橘黄。
fn draw_row(ui: &mut egui::Ui, row: &ExpLogRow) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing = egui::vec2(6.0, 0.0);
        ui.monospace(RichText::new(&row.t).color(TIME_COLOR));
        ui.monospace(RichText::new("EXP").weak());
        if row.seg {
            ui.monospace(RichText::new(thousands(row.exp)).color(theme::ORANGE));
        } else {
            ui.monospace(RichText::new(thousands(row.exp)));
        }
        if let Some(d) = row.delta {
            ui.monospace(RichText::new(format!("[+{}]", thousands(d))).color(theme::SUCCESS));
        }
    });
}
