//! 上报数据页（抽屉内）：表单页 ⇄ 成功页（同一块内容里切 `ReportPhase`）。
//!
//! 页面自身不做固定高度/滚动：整块内容自上而下自然排布，由 app.rs 量出实际高度
//! 去适配窗口（"高度自适应"）。表单排成固定两行：
//! 第一行 = 等级｜职业｜地图｜攻击力(或魔法力)｜模式（除备注、按钮外的全部字段）；
//! 第二行 = 备注 ＋ 取消/确认提交（按钮右对齐）。
//! 地图联想列在地图栏正下方，点选即填入（仅输入中短暂出现，不常驻）。
//! 只有「确认提交」是橘黄重点按钮；成功页里只有「前往查看」是橘黄重点按钮。
//! 成功页没有图标：顶部是绿色分享网址，下方「返回 / 前往查看」两钮；
//! 点网址或「前往查看」会开浏览器并**同时收起本页**。没有标题条与右上 ✕；
//! 收起 = 再点主卡按钮、或"取消"/"返回"（以及上面的"前往查看/点网址"）。
//! 布局与行为见根目录 `开发.md`。
//!
//! 确认提交后把载荷交给**后台线程**真正联网（v2 上报），期间按钮显示"提交中…"并防重；
//! 成功 → 切到成功页并展示分享网址；失败 → 留在表单、红字给原因，可直接改后重试。

use std::sync::{Arc, Mutex};

use eframe::egui;
use egui::{vec2, Align, ComboBox, Layout, RichText, ScrollArea, Sense, TextEdit};

use crate::data::MAGIC_GROUP;
use crate::net;
use crate::state::{Page, ReportPayload, ReportPhase, ReportSuccess, Shared};
use crate::theme;

/// 在抽屉区里画整页内容（表单或成功页），由 app.rs 在 `shared.page==Report` 时调用。
/// 关闭 = 置回 `page=None`，抽屉随即收回。
pub fn page(ui: &mut egui::Ui, shared: &Arc<Mutex<Shared>>) {
    let mut close = false;
    let mut open: Option<String> = None; // 点外部链接 / 管理数据 → 打开浏览器
    let mut kick: Option<u64> = None; // 确认提交通过校验，要去后台联网（记下 req）
    {
        let mut g = shared.lock().unwrap();
        let phase = g.report.phase;

        // 页面上/下的统一留白由 app.rs 的 DRAWER_MARGIN_Y 给出，这里不再额外加外圈空距，
        // 保证两页间距一致、内容不顶边。
        match phase {
            ReportPhase::Form => draw_form(ui, &mut g, &mut close, &mut kick),
            ReportPhase::Success => draw_success(ui, &mut g, &mut close, &mut open),
        }
    } // 锁先放掉，再执行副作用

    if let Some(req) = kick {
        start_submit(Arc::clone(shared), req); // 联网放后台，UI 不卡
    }
    if let Some(url) = open {
        net::open_url(&url);
    }
    if close {
        if let Ok(mut g) = shared.lock() {
            if g.page == Page::Report {
                g.page = Page::None;
            }
        }
        ui.ctx().request_repaint(); // 立刻让抽屉收回动画开跑
    }
}

// ---------------------------------------------------------------------------
// 表单页
// ---------------------------------------------------------------------------

fn draw_form(ui: &mut egui::Ui, g: &mut Shared, close: &mut bool, kick: &mut Option<u64>) {
    let total = ui.available_width();
    let gap = 8.0;

    // —— 第一行：除备注、按钮外的全部字段，一行内加权分栏 ——
    // 权重：数字栏窄、职业中等、地图最宽、模式最小，让地图有搜索余地且整行一次排完。
    let weights = [0.62f32, 1.0, 1.7, 1.0, 0.9];
    let sum: f32 = weights.iter().sum();
    let unit = ((total - gap * (weights.len() - 1) as f32) / sum).max(1.0);
    let mut ws = [0.0f32; 5];
    for (i, w) in weights.iter().enumerate() {
        ws[i] = w * unit;
    }
    // "地图"列相对行首的 x（联想列表对齐用）。
    let map_x = ws[0] + gap + ws[1] + gap;

    let magic = g
        .jobs
        .get(g.report.job_group)
        .map_or(false, |gr| gr.group == MAGIC_GROUP);

    let _ = ui.allocate_ui_with_layout(
        vec2(total, 200.0),
        Layout::left_to_right(Align::Min),
        |row| {
            row.spacing_mut().item_spacing = vec2(gap, 0.0);
            form_cell(row, ws[0], "等级", |c| {
                c.add(
                    TextEdit::singleline(&mut g.report.level)
                        .desired_width(f32::INFINITY)
                        .hint_text("正整数"),
                );
            });
            form_cell(row, ws[1], "职业", |c| job_combo(c, g));
            form_cell(row, ws[2], "地图", |c| {
                c.add(
                    TextEdit::singleline(&mut g.report.map_query)
                        .desired_width(f32::INFINITY)
                        .hint_text("搜索或直填"),
                );
            });
            let label = if magic { "魔法力" } else { "攻击力" };
            form_cell(row, ws[3], label, |c| {
                c.add(
                    TextEdit::singleline(&mut g.report.power)
                        .desired_width(f32::INFINITY)
                        .hint_text("正整数"),
                );
            });
            form_cell(row, ws[4], "模式", |c| mode_pick(c, g));
        },
    );

    // 地图联想：紧跟第一行、左对齐"地图"列；点选即填入（选中后联想自然消失，
    // 平时不占位，保证整页稳定在两行）。
    map_suggestions(ui, g, map_x);

    // —— 第二行：备注（占左）+ 取消/确认提交（右对齐）——
    ui.add_space(12.0);
    let busy = g.report.submitting;
    let confirm_w = 124.0;
    let cancel_w = 88.0;
    // 备注编辑宽 = 总宽 − "备注"标签 − 取消 − 确认 − 3 个横向自动间距。
    let lab_w = ui
        .painter()
        .layout_no_wrap(
            "备注".to_owned(),
            egui::FontId::proportional(12.0),
            egui::Color32::WHITE,
        )
        .size()
        .x;
    let note_w = (total - lab_w - cancel_w - confirm_w - 3.0 * gap).max(120.0);
    ui.horizontal(|ui| {
        ui.label(RichText::new("备注").weak());
        note_edit(ui, g, note_w);
        if ui.add_sized([cancel_w, 30.0], egui::Button::new("取消")).clicked() {
            *close = true;
        }
        let label = if busy { "提交中…" } else { "确认提交" };
        let resp = ui.add_sized(
            [confirm_w, 30.0],
            egui::Button::new(RichText::new(label).strong()).fill(theme::ORANGE),
        );
        if busy {
            resp.on_hover_text("正在联网上报，请稍候");
        } else if resp.clicked() {
            match build_payload(g) {
                Ok(payload) => {
                    g.report.err.clear();
                    g.report.submitting = true;
                    g.report.req = g.report.req.wrapping_add(1);
                    let req = g.report.req;
                    g.report.pending = Some(payload);
                    *kick = Some(req);
                }
                Err(e) => g.report.err = e,
            }
        }
    });

    // 校验/提交出错时红字提示（仅在出错时出现的第三行）。
    if !g.report.err.is_empty() {
        ui.add_space(6.0);
        ui.colored_label(theme::DANGER, &g.report.err);
    }
}

/// 一个"标签在上、控件在下"的固定宽度栏。`w` 由外层分栏给定。
fn form_cell(ui: &mut egui::Ui, w: f32, label: &str, ctl: impl FnOnce(&mut egui::Ui)) {
    ui.allocate_ui_with_layout(vec2(w, 200.0), Layout::top_down(Align::Min), |c| {
        c.spacing_mut().item_spacing = vec2(0.0, 5.0);
        c.label(RichText::new(label).size(12.0).weak());
        ctl(c);
    });
}

/// 地图联想：对齐"地图"列的一小片候选列表，点选即填入；无查询或已精确匹配则不出现。
fn map_suggestions(ui: &mut egui::Ui, g: &mut Shared, indent: f32) {
    let q = g.report.map_query.trim().to_owned();
    if q.is_empty() {
        return;
    }
    let maps = Arc::clone(&g.maps);
    let hits = crate::data::map_search(&maps, &q);
    let exact = hits.iter().any(|m| m.name == q);
    if hits.is_empty() || exact {
        return;
    }

    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
        ui.add_space(indent);
        let avail = ui.available_width();
        let sw = avail.clamp(200.0, 360.0);
        ScrollArea::vertical()
            .id_salt("map_hits")
            .max_height(150.0)
            .min_scrolled_width(sw)
            .show(ui, |ui| {
                for m in hits.iter().take(10) {
                    let name = m.name.clone();
                    if ui
                        .add_sized([sw, 24.0], egui::Button::new(name))
                        .on_hover_text("点击填入")
                        .clicked()
                    {
                        g.report.map_query = m.name.clone();
                    }
                }
            });
    });
}

/// 校验 + 保存默认值 + 打包本次上报载荷。出错时返回消息（调用方写进 `report.err`）。
/// 只在"进页面那一刻冻结的 exp 快照"上打包，见 state.rs `init_report`。
fn build_payload(g: &mut Shared) -> Result<ReportPayload, String> {
    let level: u32 = match g.report.level.trim().parse() {
        Ok(v) if v >= 1 => v,
        _ => return Err("等级需为 ≥1 的正整数".into()),
    };

    let job = g.job_name(g.report.job_group, g.report.job);
    if job.is_empty() {
        return Err("请选择一个职业（data/jobs.json 缺失？）".into());
    }

    let map = g.report.map_query.trim();
    if map.is_empty() {
        return Err("请填写或选择一个地图".into());
    }

    let power: u64 = match g.report.power.trim().parse() {
        Ok(v) if v >= 1 => v,
        _ => {
            let label = if g
                .jobs
                .get(g.report.job_group)
                .map_or(false, |gr| gr.group == MAGIC_GROUP)
            {
                "魔法力"
            } else {
                "攻击力"
            };
            return Err(format!("{label} 需为 ≥1 的正整数"));
        }
    };

    // 保存默认值到 data.ini（下次打开自动带出）
    g.cfg.level = level;
    g.cfg.job = job.clone();
    g.cfg.map = map.to_owned();
    g.cfg.mode_solo = g.report.mode_solo;
    g.cfg.power = power;
    g.persist_cfg();

    Ok(ReportPayload {
        level,
        job,
        map: map.to_owned(),
        mode_solo: g.report.mode_solo,
        power,
        note: g.report.note.clone(),
        exp_per_hour: g.report.exp_per_hour,
        exp_seconds: g.report.exp_seconds,
    })
}

/// 职业：ComboBox，组内做小标题分组。
fn job_combo(ui: &mut egui::Ui, g: &mut Shared) {
    let mut gi = g.report.job_group;
    let mut ji = g.report.job;
    let cur = g
        .jobs
        .get(gi)
        .and_then(|gr| gr.jobs.get(ji))
        .cloned()
        .unwrap_or_else(|| "请选择职业".to_owned());
    ComboBox::from_id_salt("job_combo")
        .width(ui.available_width())
        .selected_text(cur)
        .show_ui(ui, |ui| {
            for (cgi, grp) in g.jobs.iter().enumerate() {
                if grp.jobs.is_empty() {
                    continue;
                }
                ui.label(RichText::new(format!("—— {} ——", grp.group)).weak().small());
                for (cji, job) in grp.jobs.iter().enumerate() {
                    let sel = cgi == gi && cji == ji;
                    if ui.selectable_label(sel, job.clone()).clicked() {
                        gi = cgi;
                        ji = cji;
                    }
                }
            }
        });
    g.report.job_group = gi;
    g.report.job = ji;
}

/// 模式：组队 / 单人。
fn mode_pick(ui: &mut egui::Ui, g: &mut Shared) {
    ui.horizontal(|ui| {
        if ui.selectable_label(g.report.mode_solo, "单人").clicked() {
            g.report.mode_solo = true;
        }
        if ui.selectable_label(!g.report.mode_solo, "组队").clicked() {
            g.report.mode_solo = false;
        }
    });
}

/// 备注：≤20 字符，输入途中超限立刻截断。`width` = 编辑框宽度（第二行左区，占满到按钮前）。
fn note_edit(ui: &mut egui::Ui, g: &mut Shared, width: f32) {
    let cap = g.report.note.chars().take(20).collect::<String>();
    if cap != g.report.note {
        g.report.note = cap;
    }
    let resp = ui.add(
        TextEdit::singleline(&mut g.report.note)
            .desired_width(width)
            .hint_text("选填，最多 20 字"),
    );
    if resp.changed() {
        // IME 组合中的临时超长下一帧再处理
        let cap = g.report.note.chars().take(20).collect::<String>();
        if cap != g.report.note {
            g.report.note = cap;
        }
    }
}

// ---------------------------------------------------------------------------
// 联网提交（后台线程）
// ---------------------------------------------------------------------------

/// v2 上报请求体：snake_case，每小时值直接给，服务端以 token 的 sub 落设备。
fn report_json(p: &ReportPayload) -> serde_json::Value {
    serde_json::json!({
        "exp_per_hour": p.exp_per_hour,
        "job": p.job,
        "level": p.level,
        "map": p.map,
        "mode": if p.mode_solo { "solo" } else { "party" },
        "power": p.power,
        "note": p.note,
        "test_seconds": p.exp_seconds,
    })
}

/// 上报流程：换/取 token → POST。遇 401（token 失效）强制换新重试一次。
fn send_report(shared: &Arc<Mutex<Shared>>, base: &str, payload: &ReportPayload) -> Result<String, net::ApiError> {
    let to_api = |e: String| net::ApiError { status: None, message: e };
    let json = report_json(payload);
    let token = net::ensure_device_token(shared).map_err(to_api)?;
    match net::post_report(base, &token, &json) {
        Ok(id) => Ok(id),
        Err(ae) if ae.status == Some(401) => {
            // 缓存里的 token 被拒：清掉换新的再试一次
            let token2 = net::refresh_token(shared).map_err(to_api)?;
            net::post_report(base, &token2, &json)
        }
        Err(ae) => Err(ae),
    }
}

/// 起一个后台线程做真正联网。成功后写 success/切成功页；失败留在表单红字原因。
/// 回写前比对 `req`，若用户已重新打开/切换页面则旧结果作废（不覆盖新会话）。
fn start_submit(shared: Arc<Mutex<Shared>>, req: u64) {
    std::thread::spawn(move || {
        let (base, payload) = {
            let g = shared.lock().unwrap();
            if g.report.submitting && g.report.req == req {
                let base = net::api_base(&g.cfg);
                (base.to_owned(), g.report.pending.clone())
            } else {
                (String::new(), None)
            }
        };
        let Some(payload) = payload else { return };

        #[cfg(debug_assertions)]
        eprintln!(
            "[mxd-bar] 上报 {} → {base}",
            report_json(&payload)
        );

        let result = send_report(&shared, &base, &payload);

        let mut g = shared.lock().unwrap();
        if !(g.report.submitting && g.report.req == req) {
            return; // 页面已被重新打开/切换，别拿旧结果覆盖
        }
        match result {
            Ok(id) => {
                g.report.success = Some(ReportSuccess { url: net::share_url(&base, &id) });
                g.report.submitting = false;
                g.report.phase = ReportPhase::Success;
            }
            Err(ae) => {
                g.report.err = format!("上报失败：{}", ae.message);
                g.report.submitting = false;
            }
        }
    });
}

// ---------------------------------------------------------------------------
// 成功页
// ---------------------------------------------------------------------------

fn draw_success(ui: &mut egui::Ui, g: &mut Shared, close: &mut bool, open: &mut Option<String>) {
    let Some(s) = g.report.success.clone() else {
        *close = true; // 理论到不了
        return;
    };
    let report_url = s.url.clone();

    ui.add_space(2.0);
    // 绿色分享网址：点它 = 开浏览器看这条记录；开完顺手收起本页（等同"前往查看"）
    ui.vertical_centered(|ui| {
        let link = ui.add(
            egui::Label::new(RichText::new(&report_url).color(theme::SUCCESS).underline())
                .sense(Sense::click()),
        );
        if link.clicked() {
            *open = Some(report_url.clone());
            *close = true;
        }
        if link.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
    });

    // 底部两按钮一行：返回(中性) / 前往查看(橘黄重点)。定宽、居中，不铺满整行。
    // 前往查看开完浏览器也收起本页。
    ui.add_space(22.0);
    let bw = 130.0;
    let gap = 12.0;
    let lead = ((ui.available_width() - (2.0 * bw + gap)) * 0.5).max(0.0);
    ui.horizontal(|ui| {
        ui.add_space(lead);
        if ui.add_sized([bw, 30.0], egui::Button::new("返回")).clicked() {
            *close = true;
        }
        ui.add_space(gap);
        if ui
            .add_sized(
                [bw, 30.0],
                egui::Button::new(RichText::new("前往查看").strong()).fill(theme::ORANGE),
            )
            .clicked()
        {
            *open = Some(report_url.clone());
            *close = true;
        }
    });
}
