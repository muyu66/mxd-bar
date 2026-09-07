//! 「关于」抽屉页（点主卡工具格 ⓘ 打开）：只显示工具名称/版本与官网，保持干净。
//! 官网一行可点：点开用默认浏览器打开官网，并顺手收起本页（与上报成功页"前往查看"一致）。
//! 高度固定 `ABOUT_PAGE_H`，从底部随抽屉滑出；关闭 = 再点主卡「关于」图标（或其它按钮收起）。

use std::sync::{Arc, Mutex};

use eframe::egui;
use egui::{pos2, vec2, Align2, Color32, FontId, Rect, Sense, Stroke};

use crate::net;
use crate::state::{Page, Shared};
use crate::theme;

/// 「关于」页内容区固定高（逻辑像素）。整页高度因此恒定。
const ABOUT_PAGE_H: f32 = 150.0;
/// 官网链接悬停时提亮（比主题 `ORANGE` 更亮一档的橘黄）。
const HOVER_ORANGE: Color32 = Color32::from_rgb(255, 196, 120);

/// 在抽屉区画整页「关于」，由 app.rs 在 `shared.page==About` 时调用。
pub fn page(ui: &mut egui::Ui, shared: &Arc<Mutex<Shared>>) {
    let pal = theme::Palette::dark();
    let painter = ui.painter().clone();
    // 占满固定高：让 app.rs 量到的抽屉高度恒定，展开/收起不随内容跳。
    let (rect, _) =
        ui.allocate_exact_size(vec2(ui.available_width(), ABOUT_PAGE_H), Sense::hover());

    // —— 标题行：工具名 + 版本（取 Cargo.toml version）——
    let title = format!("mxd-bar  v{}", env!("CARGO_PKG_VERSION"));
    let title_font = FontId::proportional(16.0);
    painter.text(
        pos2(rect.center().x, rect.center().y - 30.0),
        Align2::CENTER_CENTER,
        &title,
        title_font,
        pal.btn_text,
    );

    // —— 官网行：整行文本矩形可点，悬停提亮 + 下划线 ——
    let site = net::official_site();
    let site_font = FontId::proportional(13.0);
    let site_size = painter
        .layout_no_wrap(site.to_owned(), site_font.clone(), Color32::WHITE)
        .size();
    let site_center = pos2(rect.center().x, rect.center().y + 24.0);
    let link_rect = Rect::from_center_size(site_center, site_size);
    let resp = ui.interact(link_rect, ui.id().with("about_site_link"), Sense::click());
    let hovered = resp.hovered() || resp.is_pointer_button_down_on();
    let color = if hovered { HOVER_ORANGE } else { theme::ORANGE };
    painter.text(site_center, Align2::CENTER_CENTER, site, site_font, color);
    if hovered {
        // 悬停下划线（画在文字框下沿 +1px）。
        painter.line_segment(
            [
                pos2(link_rect.left(), link_rect.bottom() + 1.0),
                pos2(link_rect.right(), link_rect.bottom() + 1.0),
            ],
            Stroke::new(1.0, color),
        );
    }

    if resp.clicked() {
        net::open_url(site);
        // 顺手收起本页：浏览器前台打开后，悬浮卡不再挡着游戏画面。
        if let Ok(mut g) = shared.lock()
            && g.page == Page::About
        {
            g.page = Page::None;
        }
        ui.ctx().request_repaint();
    }
}
