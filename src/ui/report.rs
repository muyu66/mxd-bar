//! 上报数据页（抽屉内）：表单页 ⇄ 成功页（同一块内容里切 `ReportPhase`）。
//!
//! 页面自身不做固定高度/滚动：整块内容自上而下自然排布，由 app.rs 量出实际高度
//! 去适配窗口（"高度自适应"）。表单按"标签在上、输入在下"排成：
//! 等级｜职业｜地图 / 攻击力或魔法力｜模式 / 备注(整行)。
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
use egui::{ComboBox, RichText, ScrollArea, Sense, TextEdit};

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

        ui.add_space(6.0);
        match phase {
            ReportPhase::Form => draw_form(ui, &mut g, &mut close, &mut kick),
            ReportPhase::Success => draw_success(ui, &mut g, &mut close, &mut open),
        }
        ui.add_space(8.0); // 底部留白（窗口高度已按内容自动适配）
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

/// 一个小节标签（标签在上、控件在下）。
fn field_label(ui: &mut egui::Ui, text: &str) {
    ui.add_space(6.0);
    ui.label(RichText::new(text).strong());
}

// ---------------------------------------------------------------------------
// 表单页
// ---------------------------------------------------------------------------

fn draw_form(ui: &mut egui::Ui, g: &mut Shared, close: &mut bool, kick: &mut Option<u64>) {
    // —— 第 1 行：等级 | 职业 | 地图 ——
    ui.columns(3, |cols| {
        field_label(&mut cols[0], "等级");
        cols[0].add(
            TextEdit::singleline(&mut g.report.level)
                .desired_width(f32::INFINITY)
                .hint_text("正整数，如 120"),
        );

        field_label(&mut cols[1], "职业");
        job_combo(&mut cols[1], g);

        map_field(&mut cols[2], g);
    });

    // —— 第 2 行：攻击力/魔法力 | 模式 ——
    let magic = g
        .jobs
        .get(g.report.job_group)
        .map_or(false, |gr| gr.group == MAGIC_GROUP);
    ui.add_space(6.0);
    ui.columns(2, |cols| {
        field_label(&mut cols[0], if magic { "魔法力" } else { "攻击力" });
        cols[0].add(
            TextEdit::singleline(&mut g.report.power)
                .desired_width(f32::INFINITY)
                .hint_text("正整数"),
        );

        field_label(&mut cols[1], "模式");
        mode_pick(&mut cols[1], g);
    });

    // —— 第 3 行：备注（整行）——
    ui.add_space(6.0);
    field_label(ui, "备注");
    note_edit(ui, g);

    ui.add_space(12.0);
    ui.separator();

    // 错误提示
    if !g.report.err.is_empty() {
        ui.colored_label(theme::DANGER, &g.report.err);
    }

    // 确认（橘黄重点）/ 取消。提交中按钮显示"提交中…"并禁点（防重复）。
    let busy = g.report.submitting;
    ui.horizontal(|ui| {
        let label = if busy { "提交中…" } else { "确认提交" };
        let resp = ui.add_sized(
            [ui.available_width() * 0.6, 28.0],
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
        ui.add_space(8.0);
        if ui
            .add_sized([ui.available_width(), 28.0], egui::Button::new("取消"))
            .clicked()
        {
            *close = true;
        }
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

/// 地图：输入 + 搜索二合一（选择建议列在下方，点选即填入）。
fn map_field(ui: &mut egui::Ui, g: &mut Shared) {
    field_label(ui, "地图");
    ui.add(
        TextEdit::singleline(&mut g.report.map_query)
            .desired_width(f32::INFINITY)
            .hint_text("输入关键字选择，或直接填地图名"),
    );
    let q = g.report.map_query.trim().to_owned();
    if !q.is_empty() {
        let maps_arc = Arc::clone(&g.maps);
        let hits = crate::data::map_search(&maps_arc, &q);
        let exact = hits.iter().any(|m| m.name == q);
        if !hits.is_empty() && !exact {
            ui.add_space(2.0);
            ScrollArea::vertical()
                .id_salt("map_hits")
                .max_height(72.0)
                .show(ui, |ui| {
                    for m in hits.iter().take(12) {
                        if ui
                            .selectable_label(m.name == q, m.name.clone())
                            .on_hover_text("点击填入")
                            .clicked()
                        {
                            g.report.map_query = m.name.clone();
                        }
                    }
                });
        }
    }
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

/// 备注：≤20 字符，输入途中超限立刻截断。
fn note_edit(ui: &mut egui::Ui, g: &mut Shared) {
    let cap = g.report.note.chars().take(20).collect::<String>();
    if cap != g.report.note {
        g.report.note = cap;
    }
    let resp = ui.add(
        TextEdit::singleline(&mut g.report.note)
            .desired_width(f32::INFINITY)
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

    ui.add_space(18.0);
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

    // 底部两按钮一行：返回(中性) / 前往查看(橘黄重点)。前往查看开完浏览器也收起本页。
    ui.add_space(24.0);
    ui.horizontal(|ui| {
        let gap = 8.0;
        let bw = (ui.available_width() - gap) / 2.0;
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
    ui.add_space(8.0);
}
