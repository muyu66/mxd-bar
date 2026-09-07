//! 主卡右上角 2×2 工具图标：把 `icons/` 里的 PNG 随 exe 内嵌（include_bytes!），
//! 首帧用 `image` crate 解码成 RGBA 上传为 egui 纹理（见 `load`）。
//!
//! 每张图都是 200×200 方形圆底图标（IHDR 已核），绘制时按正方形 cell 等比缩放即可，
//! 无需处理非方形容差。绘制统一 tint 白 —— 白与纹理颜色相乘 = 原色，保留 PNG 自带配色。
//!
//! 四枚图标对应主卡最右工具格的四个动作：close=退出程序、reload=重置队列、
//! debug=EXP 日志抽屉、info=「关于」（版本 + 官网）。旧的 close-circle/up-circle/
//! down-circle/info-circle 一批已被替换删除，这里按新文件名内嵌。
//! 用户 2026-09-07 定稿排布：上行 日志(debug)·关闭(close)，下行 关于(info)·重置(reload)。

use eframe::egui;
use egui::{ColorImage, Context, TextureHandle, TextureOptions};

/// 右上角 2×2 工具区的四枚图标（关闭 / 重置 / 日志 / 关于）。
pub(crate) struct Icons {
    /// 退出程序（工具格右上，close）。
    pub close: TextureHandle,
    /// 重置队列（工具格右下，reload）。
    pub reload: TextureHandle,
    /// 打开 EXP 日志抽屉页（工具格左上，debug）。
    pub debug: TextureHandle,
    /// 打开「关于」抽屉页——版本 + 官网（工具格左下，info）。
    pub info: TextureHandle,
}

impl Icons {
    /// 需要 `ctx` 才能上传纹理，故在 `ui()` 首帧惰性创建一次（存进 `MxdBarApp.icons`）。
    pub fn load(ctx: &Context) -> Self {
        Icons {
            close: load(ctx, "close", include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/icons/close.png"))),
            reload: load(ctx, "reload", include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/icons/reload.png"))),
            debug: load(ctx, "debug", include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/icons/debug.png"))),
            info: load(ctx, "info", include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/icons/info.png"))),
        }
    }
}

/// 解码一枚内嵌 PNG 并上传为纹理。源文件缺失/损坏属于打包问题，启动即 panic 报明。
fn load(ctx: &Context, name: &str, bytes: &[u8]) -> TextureHandle {
    let img = image::load_from_memory(bytes)
        .unwrap_or_else(|e| panic!("icons/{name}.png 解码失败: {e}"))
        .into_rgba8();
    let (w, h) = img.dimensions();
    let color = ColorImage::from_rgba_unmultiplied([w as usize, h as usize], &img);
    ctx.load_texture(format!("mxd_{name}"), color, TextureOptions::LINEAR)
}
