//! mxd-bar 程序入口。
//!
//! 创建一个无边框、置顶、透明的悬浮小卡片窗口。
//! UI 与样式详见 `app.rs` / `theme.rs`；共享状态在 `state.rs`。

// release 构建下隐藏控制台窗口。
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod config;
mod data;
mod net;
mod sensor;
mod state;
mod theme;
mod ui;
mod util;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use eframe::egui;

use crate::state::Shared;

#[cfg(debug_assertions)]
mod digitcheck;

fn main() -> eframe::Result {
    // —— debug：内嵌 0-9 模板分类器离线回归（--digit [img...]）——
    // 对离线帧跑与实机同一 `digits::read_exp_value`，与文件名真值对照（test-img/*.png + here_full/cap）。
    #[cfg(debug_assertions)]
    {
        let args: Vec<String> = std::env::args().collect();
        if args.len() >= 2 && args[1] == "--digit" {
            return digitcheck::run();
        }
    }

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("mxd-bar")
            .with_app_id("mxd-bar")
            // 无边框
            .with_decorations(false)
            // 置顶
            .with_always_on_top()
            // 透明窗口，用于画出真正的圆角卡片
            .with_transparent(true)
            .with_resizable(false)
            .with_taskbar(false)
            // 初始占位尺寸，首帧会按内容自动修正宽度
            .with_inner_size([app::INITIAL_WIDTH, app::BAR_HEIGHT]),
        ..Default::default()
    };

    // —— 启动期一次性加载：ini 配置 + 两张 JSON 数据表（职业组/打怪地图）——
    // 经验速率只靠内嵌 0-9 模板分类器读 EXP 整数差分，不再需要 exp-table（等级表）。
    // 数据表缺失时降级为空，UI 依旧可用（只是没有可选项），并打印原因。
    let cfg = config::load();
    let jobs = load_or_else(data::load_jobs, "jobs.json");
    // 进 Shared 前就按 scene 过滤成打怪地图（state.rs 的 Shared.maps 契约）。
    // maps.json 共 705 张，town/other 占 468 张——不滤的话地图搜索建议会混进城镇等不可练级图。
    let maps = data::filter_hunting(&load_or_else(data::load_maps, "maps.json"));

    #[cfg(debug_assertions)]
    eprintln!(
        "[mxd-bar] 启动: 职业组 {} 个, 打怪地图 {} 张",
        jobs.len(),
        maps.len()
    );

    let shared = Arc::new(Mutex::new(Shared::new(cfg, jobs, maps)));

    // debug：联调自测 `--nettest`——对本地服务跑一遍 换 token + 上报。
    // 只允许走本地地址（data.ini [net] base=local），避免误往生产库塞假数据。
    #[cfg(debug_assertions)]
    {
        let args: Vec<String> = std::env::args().collect();
        if args.iter().any(|a| a == "--nettest") {
            let cfg = shared.lock().unwrap().cfg.clone();
            let base = crate::net::api_base(&cfg);
            eprintln!("[nettest] base={base}");
            if !crate::net::local_enabled(&cfg) {
                eprintln!("[nettest] 只限本地联调：请设环境变量 MXD_BAR_API=local，或在 data.ini 写 [net] base=local");
                return Ok(());
            }
            let uid = shared
                .lock()
                .unwrap()
                .uid
                .clone()
                .or_else(crate::net::compute_uid)
                .unwrap_or_default();
            eprintln!("[nettest] uid={uid}");
            if uid.is_empty() {
                eprintln!("[nettest] 无法取得设备标识(主板序列号)，中止");
            } else {
                match crate::net::request_token(base, &uid) {
                    Ok((tok, exp)) => {
                        eprintln!("[nettest] token OK (len={}, {}s)", tok.len(), exp);
                        let p = serde_json::json!({
                            "exp_per_hour": 0, "job": "测试", "level": 1,
                            "map": "测试地图", "mode": "solo", "power": 1,
                            "note": "mxd-bar --nettest", "vip": false, "test_seconds": 5,
                        });
                        match crate::net::post_report(base, &tok, &p) {
                            Ok(id) => eprintln!(
                                "[nettest] report OK id={id} 分享={}",
                                crate::net::share_url(base, &id)
                            ),
                            Err(e) => eprintln!("[nettest] report 失败: {e:?}"),
                        }
                    }
                    Err(e) => eprintln!("[nettest] token 失败: {e}"),
                }
            }
            return Ok(());
        }
    }

    // debug：启动即展开某个抽屉页，便于离线观察/自测布局（`--page-report` / `--page-pick`）。
    #[cfg(debug_assertions)]
    {
        use crate::state::{Page, PickState, TimePickKind};
        let force = std::env::args().find_map(|a| match a.as_str() {
            "--page-report" => Some(1),
            "--page-pick" => Some(2),
            _ => None,
        });
        let mut g = shared.lock().unwrap();
        match force {
            Some(1) => {
                g.init_report();
                g.page = Page::Report;
            }
            Some(2) => {
                g.pick = PickState { h: 5, m: 59 };
                g.page = Page::TimePick(TimePickKind::Boss);
            }
            _ => {}
        }
        drop(g);
    }


    // —— 采样线程：仅当 Maplestory_Classic.exe 位于前台时才整窗截图 + OCR 读全量 EXP ——
    // 每 5s 按"活动时钟"（扣除切走/最小化等暂停墙钟）算经验速率；切走即暂停、回来续走，暂停
    // 的空隙不会把速率拉低。不取等级/百分比/条宽，只差分 EXP 整数；读失败不记录；单次读数递减
    // 当作瞬时误读忽略、连续 3 条低于此前平均才判定升级并重置（细节见 sampler.rs）。
    // 游戏没开或不在前台时线程停在对应状态，不影响卡片。
    let stop = Arc::new(AtomicBool::new(false));
    let sampler_handle = {
        let shared2 = Arc::clone(&shared);
        let st = Arc::clone(&stop);
        std::thread::spawn(move || sensor::sampler::run(shared2, st))
    };

    // —— UID（主板序列号 → MD5 32hex）——它是 v2 换 token 用的 deviceId。
    // 未命中缓存时后台算一次并写进 data.ini；token_keeper 会等它到位后再换 token。
    {
        let need = shared.lock().unwrap().uid.is_none();
        if need {
            let shared2 = Arc::clone(&shared);
            std::thread::spawn(move || match crate::net::compute_uid() {
                Some(u) => {
                    let mut g = shared2.lock().unwrap();
                    g.uid = Some(u.clone());
                    g.cfg.uid_cache = Some(u);
                    g.persist_cfg();
                }
                None => {
                    shared2.lock().unwrap().uid_error = Some("无法读取主板序列号".into());
                }
            });
        }
    }

    // —— v2 token 预取与刷新：后台线程维护 JWT（等 uid 算好后再换），UI 不碰联网 ——
    crate::net::spawn_token_keeper(Arc::clone(&shared));

    let result = eframe::run_native(
        "mxd-bar",
        options,
        Box::new(|cc| {
            // 注入中文字体（微软雅黑等），避免中文显示为方块
            theme::install_fonts(&cc.egui_ctx);
            Ok(Box::new(app::MxdBarApp::new(shared.clone())))
        }),
    );

    // —— 退出清理：让采样线程退出并等它收尾（释放 WinRT/截图句柄）——
    stop.store(true, Ordering::SeqCst);
    let _ = sampler_handle.join();

    result
}

/// 加载失败时打印原因并返回空值（保持程序可用）。
fn load_or_else<T: Default>(f: impl FnOnce() -> Result<T, std::io::Error>, name: &str) -> T {
    f().unwrap_or_else(|e| {
        eprintln!("[mxd-bar] 警告: 读取 {name} 失败: {e}");
        T::default()
    })
}
