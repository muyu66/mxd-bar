//! mxd-bar 的全局主题：配色方案、中文字体加载、以及子窗体 egui 控件的深色样式。

use std::borrow::Cow;
use std::sync::Arc;

use eframe::egui;
use egui::{Color32, FontData, FontDefinitions, FontFamily, Stroke};

/// 主按钮橘黄（用作纯文字/小强调，亮度高的场合）。
pub const ORANGE: Color32 = Color32::from_rgb(255, 152, 51);
/// 深橘黄实心底：白字压在橘黄上的大面积底色（选中态 / 重点按钮实心）用这个。
/// 白字对比约 4.8:1，不像亮橘（~2.1:1）那样发糊刺眼；橘黄纯文字仍用 `ORANGE`。
pub const ORANGE_DEEP: Color32 = Color32::from_rgb(180, 83, 9);
/// 成功/已刷新绿。
pub const SUCCESS: Color32 = Color32::from_rgb(92, 235, 150);
/// 危险/待打卡红。
pub const DANGER: Color32 = Color32::from_rgb(255, 96, 96);

/// 整套深色 HUD 配色，方便后续统一调整。
#[derive(Clone, Copy, Debug)]
pub struct Palette {
    /// 卡片底色（半透明深蓝黑）
    pub bg: Color32,
    /// 卡片描边
    pub border: Color32,
    /// 区域分隔线
    pub divider: Color32,
    /// 指标标签文字（如 “实时EXP/分”）
    pub label: Color32,
    /// “实时EXP/分” 数值颜色
    pub exp_per_min: Color32,
    /// “预估EXP/时” 数值颜色
    pub exp_per_hour: Color32,
    /// “测试时间” 数值颜色
    pub test_time: Color32,
    /// “累计经验” 数值颜色
    pub exp_cum: Color32,
    /// 主卡"心电图"折线颜色（相对均值波动）。
    pub wave: Color32,
    /// 心电图中线（0 = 窗口均值）的颜色，比折线淡。
    pub wave_axis: Color32,
    /// 按钮底色
    pub btn_bg: Color32,
    /// 按钮悬停底色
    pub btn_hover: Color32,
    /// 按钮按下底色
    pub btn_active: Color32,
    /// 按钮描边
    pub btn_border: Color32,
    /// 按钮文字
    pub btn_text: Color32,
    /// 重点（橘黄）按钮底色/悬停/按下/描边
    pub prim_bg: Color32,
    pub prim_hover: Color32,
    pub prim_active: Color32,
    pub prim_border: Color32,
    /// 成功/已刷新（商人）
    pub success: Color32,
    /// 危险/待打卡（999 过点）
    pub danger: Color32,
}

impl Default for Palette {
    fn default() -> Self {
        Self::dark()
    }
}

impl Palette {
    pub fn dark() -> Self {
        Self {
            bg: Color32::from_rgba_unmultiplied(16, 22, 34, 226),
            border: Color32::from_rgba_unmultiplied(255, 152, 51, 90),
            divider: Color32::from_rgba_unmultiplied(140, 170, 235, 45),
            label: Color32::from_rgba_unmultiplied(148, 165, 200, 255),
            exp_per_min: Color32::from_rgb(110, 226, 255),
            exp_per_hour: ORANGE,
            // 测试时间：淡堇紫（区别于 实时/预估 的青与橘）。累计经验：成功绿，呼应"增长"。
            test_time: Color32::from_rgb(196, 181, 253),
            exp_cum: SUCCESS,
            // 心电图：折线用淡绿，中线再淡一档（表达"0 = 这 1 分钟的均值"）。
            wave: Color32::from_rgb(120, 222, 172),
            wave_axis: Color32::from_rgba_unmultiplied(120, 222, 172, 70),
            // 普通按钮 = 中性暗底（胶囊式）；只有重点按钮（主卡"上报数据"、页面"确认/确定"）用橘黄
            btn_bg: Color32::from_rgb(46, 56, 76),
            btn_hover: Color32::from_rgb(64, 77, 103),
            btn_active: Color32::from_rgb(87, 103, 135),
            btn_border: Color32::from_rgba_unmultiplied(160, 180, 220, 70),
            btn_text: Color32::from_rgb(242, 245, 250),
            // 橘黄重点按钮：整段用深橘渐变，白字不再压亮橘（悬停略提亮、按下最深）
            prim_bg: Color32::from_rgb(180, 83, 9),
            prim_hover: Color32::from_rgb(204, 104, 14),
            prim_active: Color32::from_rgb(150, 66, 8),
            prim_border: Color32::from_rgba_unmultiplied(255, 180, 100, 140),
            success: SUCCESS,
            danger: DANGER,
        }
    }
}

/// 尝试从系统字体目录加载一款中文字体（.ttc 使用 collection index）。
/// 优先微软雅黑，其次黑体。
fn load_cjk_font() -> Option<(Vec<u8>, u32)> {
    // (路径, ttc 集合序号)。msyh.ttc 序号 0 = Microsoft YaHei。
    const CANDIDATES: &[(&str, u32)] = &[
        ("C:\\Windows\\Fonts\\msyh.ttc", 0),
        ("C:\\Windows\\Fonts\\msyh.ttc", 1),
        ("C:\\Windows\\Fonts\\msyhbd.ttc", 0),
        ("C:\\Windows\\Fonts\\simhei.ttf", 0),
        ("C:\\Windows\\Fonts\\simsun.ttc", 0),
    ];
    for (path, index) in CANDIDATES {
        if let Ok(bytes) = std::fs::read(path) {
            // 简单校验一下文件是否可用（读到了内容就算成功，字体在启动时会再被解析）。
            if !bytes.is_empty() {
                return Some((bytes, *index));
            }
        }
    }
    None
}

/// 把中文字体注入 egui 全局字体表，作为拉丁字体的后备字体。
/// 这样中文不会变成 “豆腐块”。
pub fn install_fonts(ctx: &egui::Context) {
    let (bytes, index) = load_cjk_font().unwrap_or_else(|| {
        panic!(
            "未找到可用的中文字体，请确认系统中存在微软雅黑或黑体 \
             (C:\\Windows\\Fonts\\msyh.ttc 或 simhei.ttf)"
        )
    });

    let mut defs = FontDefinitions::default();
    // skrifa 支持 TrueType Collection，index 指定用哪个字面。
    defs.font_data.insert(
        "mxd-cjk".to_owned(),
        Arc::new(FontData {
            font: Cow::Owned(bytes),
            index,
            tweak: Default::default(),
        }),
    );

    // 将中文字体追加到常用字体族的回退列表末尾，
    // 拉丁字符仍用内置字体，遇到 CJK 时回退到中文。
    for family in [FontFamily::Proportional, FontFamily::Monospace] {
        let names = defs.families.entry(family).or_default();
        if !names.iter().any(|n| n == "mxd-cjk") {
            names.push("mxd-cjk".to_owned());
        }
    }

    ctx.set_fonts(defs);
}

/// 给子窗体（上报页 / 计时小窗等）的标准 egui 控件套深色 + 橘黄强调的样式。
pub fn apply_ui_style(ctx: &egui::Context) {
    let mut v = egui::Visuals::dark();
    v.panel_fill = Color32::from_rgb(22, 27, 40);
    v.window_fill = Color32::from_rgb(20, 25, 37);
    v.extreme_bg_color = Color32::from_rgb(12, 15, 22);
    v.faint_bg_color = Color32::from_rgb(28, 34, 48);
    v.override_text_color = Some(Color32::from_rgb(226, 230, 238));
    // 光标/选区：选中底用深橘黄（白字能看清），描边用柔和的浅橘
    v.selection.bg_fill = ORANGE_DEEP;
    v.selection.stroke = Stroke::new(1.0, Color32::from_rgb(255, 190, 120));
    v.hyperlink_color = ORANGE;

    for w in [
        &mut v.widgets.noninteractive,
        &mut v.widgets.inactive,
        &mut v.widgets.hovered,
        &mut v.widgets.active,
        &mut v.widgets.open,
    ] {
        w.bg_stroke.color = Color32::from_rgb(64, 72, 92);
        w.weak_bg_fill = Color32::from_rgb(30, 37, 52);
        w.bg_fill = Color32::from_rgb(38, 46, 64);
        w.fg_stroke.color = Color32::from_rgb(226, 230, 238);
    }
    // 按钮统一中性色，白字。橘黄"重点按钮"由调用方显式 .fill(theme::ORANGE_DEEP) 给出
    //（egui 标准 Button：确认提交/确定；主卡的"上报数据"用自绘重点配色）。
    v.widgets.inactive.fg_stroke.color = Color32::WHITE;
    v.widgets.hovered.fg_stroke.color = Color32::WHITE;
    v.widgets.active.fg_stroke.color = Color32::WHITE;

    ctx.set_visuals(v);
}
