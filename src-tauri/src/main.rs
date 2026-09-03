// 冒险岛怀旧服经验记录器 — Tauri 主进程
// 与旧 Electron 版保持同一 IPC 语义(桥接见 src/renderer/mxd-api.js)
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use base64::Engine;
use include_dir::{include_dir, Dir};
use serde_json::{json, Map, Value};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Mutex;
use std::time::Duration;
use tauri::{
    Emitter, Manager, PhysicalPosition, PhysicalSize, WebviewUrl, WebviewWindowBuilder, WindowEvent,
};
use tauri_plugin_opener::OpenerExt;

// 数据(地图/药水/经验表/图标,约 250KB)直接内嵌进 exe → 单文件便携,无外部资源
static DATA: Dir = include_dir!("$CARGO_MANIFEST_DIR/../data");

const SETTINGS_FILE: &str = "settings.ini";          // 设置以 INI 存储,便于手工查看/修改
const SETTINGS_JSON_LEGACY: &str = "settings.json";   // 旧 Electron 版格式,迁移完成后改名为 settings.json.migrated 留档
const APP_ID_FILE: &str = "device-id.json";

const SNAP_THRESHOLD: i32 = 24;     // 窗口顶边距工作区顶 ≤24px 视为"拖到顶部附近" → 吸顶
const COLLAPSED_H_CSS: f64 = 6.0;   // 收缩后的粗线高度(CSS px,按 DPI 换算物理像素)

struct AppState {
    base_dir: PathBuf,
    device_id: String,
    settings: Mutex<Value>,
    drag_origin: Mutex<Option<(i32, i32)>>, // JS 拖动起点(窗口位置)
    popup_ready: AtomicBool,
    popup_focusable: AtomicBool, // 时间选择器面板需要键盘输入,临时可聚焦
    pending_render: Mutex<Option<Value>>,
    load_heal: AtomicU32, // 启动竞态自愈次数(页面被代理/DNS 劫持时重新导航,最多 3 次)
    snap_on: AtomicBool,                 // 吸顶模式:窗口贴合屏幕工作区顶部
    collapsed: AtomicBool,               // 已收缩成粗线(仅吸顶模式下存在)
    expanded_height: Mutex<Option<u32>>, // 收缩前外框高度,还原用
}

// ---------- 启动竞态自愈 ----------
// WebView2 的首次导航偶尔会抢在自定义协议(tauri.localhost)注册之前完成,
// 请求漏到网络层后被系统代理/DNS 劫持成错误页(白屏)。检测到页面没落在 tauri.localhost 上就重新导航,
// 此时协议已注册,必然成功。每个窗口最多重试 3 次,避免死循环。
fn is_tauri_url(url: &str) -> bool {
    url.starts_with("http://tauri.localhost") || url.starts_with("https://tauri.localhost")
}

fn heal_page_load(webview: &tauri::WebviewWindow, url: &str, target: &str) {
    if is_tauri_url(url) || url.starts_with("about:blank") {
        return; // 正常加载或尚未开始导航
    }
    let state = webview.app_handle().state::<AppState>();
    if state.load_heal.fetch_add(1, Ordering::SeqCst) >= 3 {
        return;
    }
    println!("[自愈] 页面被劫持({url}) → 重新导航 {target}");
    // 自定义协议的正常形态为 http://tauri.localhost/<path>(wry 的 workaround 前缀),此刻协议已注册
    if let Ok(u) = tauri::Url::parse(&format!("http://tauri.localhost/{target}")) {
        let _ = webview.navigate(u);
    }
}

// ---------- 数据目录 ----------
// 沿用旧 Electron 版的 userData 目录(%APPDATA%\mxd-exp-recorder),设置/本地记录无缝衔接
fn base_dir() -> PathBuf {
    std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .unwrap_or_default()
        .join("mxd-exp-recorder")
}

// ---------- 唯一机器 ID ----------
// 优先取 Windows 注册表 MachineGuid(同机恒定);取不到时生成一次 UUID 并持久化
fn load_device_id(base: &Path) -> String {
    if let Ok(key) = winreg::RegKey::predef(winreg::enums::HKEY_LOCAL_MACHINE)
        .open_subkey(r"SOFTWARE\Microsoft\Cryptography")
    {
        if let Ok(v) = key.get_value::<String, _>("MachineGuid") {
            return v.to_lowercase();
        }
    }
    let file = base.join(APP_ID_FILE);
    if let Ok(s) = std::fs::read_to_string(&file) {
        if let Ok(j) = serde_json::from_str::<Value>(&s) {
            if let Some(id) = j.get("deviceId").and_then(|v| v.as_str()) {
                return id.to_string();
            }
        }
    }
    let id = uuid::Uuid::new_v4().to_string();
    let _ = std::fs::write(&file, json!({ "deviceId": id }).to_string());
    id
}

// ---------- 用户设置持久化(%APPDATA%\mxd-exp-recorder\settings.ini) ----------
// 设置以 INI 格式落盘(999打卡/神秘商人时间戳等可直接手工编辑);内存中仍为原 JSON 结构,渲染层无感知
// settings.json 存在即视为"未迁移"。早期 Tauri 版可能已用默认值落盘 settings.ini,
// 仅凭"ini 不存在"判断会永远跳过迁移,把老数据挡在门外(等级/999打卡显示默认值)。
// 规则:ini 缺失或解析结果是纯默认值空壳 → 老数据整体接管;否则只补 ini 中缺失/为空的字段。
// 迁移成功后把 settings.json 改名为 .migrated 留档,避免后续每次启动都用旧值覆盖新改的值。
fn load_settings(base: &Path) -> Value {
    let ini = base.join(SETTINGS_FILE);
    let legacy = base.join(SETTINGS_JSON_LEGACY);
    let legacy_val = std::fs::read_to_string(&legacy)
        .ok()
        .and_then(|t| serde_json::from_str::<Value>(&t).ok());
    let mut current = std::fs::read_to_string(&ini)
        .ok()
        .map(|t| ini_to_settings(&t))
        .unwrap_or_else(|| json!({}));
    let Some(lv) = legacy_val else { return current };
    if let Value::Object(dst) = &mut current {
        let src = lv.as_object().cloned().unwrap_or_default();
        if ini_is_defaults(dst) {
            // ini 是早期版本以默认值落盘的空壳,老数据整体接管
            *dst = src;
        } else {
            // ini 已有用户新数据,只补缺失/空字段(空壳时被默认值挡住的字段由上面分支兜底)
            for (k, v) in &src {
                if dst.get(k).map_or(true, is_blank) && !is_blank(v) {
                    dst.insert(k.clone(), v.clone());
                }
            }
        }
    }
    let _ = std::fs::write(&ini, settings_to_ini(&current));
    let _ = std::fs::rename(&legacy, base.join(format!("{SETTINGS_JSON_LEGACY}.migrated")));
    current
}

// ini 解析结果是否为纯默认值空壳(无任何真实用户数据,不含 window:窗口位置随时会被移动事件重写)
fn ini_is_defaults(v: &Map<String, Value>) -> bool {
    v.get("level").and_then(|x| x.as_i64()) == Some(1)
        && v.get("outLevel").and_then(|x| x.as_i64()) == Some(1)
        && v.get("job").and_then(|x| x.as_str()).unwrap_or("").is_empty()
        && v.get("checkin999").map_or(true, |x| x.is_null())
        && v.get("merchant").map_or(true, |x| x.is_null())
        && v.get("boss").map_or(true, |x| x.is_null())
        && v.get("potions").and_then(|x| x.as_object()).map_or(true, |o| o.is_empty())
        && v.get("map").and_then(|x| x.as_object()).map_or(true, |o| o.is_empty())
}

// 值为空(Null/空串/空对象/全零数组/等级默认值 1)视为"未设置"
fn is_blank(v: &Value) -> bool {
    match v {
        Value::Null => true,
        Value::String(s) => s.is_empty(),
        Value::Object(o) => o.is_empty(),
        Value::Array(a) => a.is_empty() || a.iter().all(|x| x.as_i64() == Some(0)),
        Value::Number(n) => n.as_i64() == Some(1),
        _ => false,
    }
}

fn save_settings_patch(app: &tauri::AppHandle, patch: Value) {
    let state = app.state::<AppState>();
    let mut s = state.settings.lock().unwrap();
    if let (Value::Object(a), Value::Object(b)) = (&mut *s, &patch) {
        for (k, v) in b {
            a.insert(k.clone(), v.clone());
        }
    }
    let _ = std::fs::write(state.base_dir.join(SETTINGS_FILE), settings_to_ini(&s));
}

// JSON 设置 → INI 文本
fn settings_to_ini(v: &Value) -> String {
    let o = v.as_object().cloned().unwrap_or_default();
    let mut s = String::new();
    let mut section = |name: &str, entries: Vec<(String, String)>| {
        if entries.is_empty() {
            return;
        }
        s.push_str(&format!("[{name}]\n"));
        for (k, val) in entries {
            s.push_str(&format!("{k}={val}\n"));
        }
        s.push('\n');
    };
    let str_of = |v: &Value| -> String {
        match v {
            Value::String(x) => x.clone(),
            other => other.to_string(),
        }
    };
    if let Some(arr) = o.get("window").and_then(|w| w.as_array()) {
        if arr.len() == 2 {
            section("window", vec![
                ("x".into(), arr[0].as_i64().unwrap_or(0).to_string()),
                ("y".into(), arr[1].as_i64().unwrap_or(0).to_string()),
            ]);
        }
    }
    section("player", ["level", "outLevel", "job", "expMode", "partyMode"]
        .iter()
        .filter_map(|k| o.get(*k).map(|v| ((*k).to_string(), str_of(v))))
        .collect());
    if let Some(pots) = o.get("potions").and_then(|p| p.as_object()) {
        section("potions", pots.iter()
            .map(|(k, v)| (k.clone(), v.as_i64().unwrap_or(0).to_string()))
            .collect());
    }
    section("checkin", ["checkin999", "merchant", "boss"]
        .iter()
        .filter_map(|k| o.get(*k).and_then(|v| v.as_i64()).map(|n| ((*k).to_string(), n.to_string())))
        .collect());
    if let Some(m) = o.get("map").and_then(|m| m.as_object()) {
        section("map", m.iter()
            .map(|(k, v)| (k.clone(), str_of(v)))
            .collect());
    }
    // Windows 记事本友好:换行用 CRLF
    s.replace('\n', "\r\n")
}

// INI 文本 → JSON 设置(结构与原 settings.json 一致)
fn ini_to_settings(text: &str) -> Value {
    let mut window = (0i64, 0i64);
    let mut player = Map::new();
    let mut potions = Map::new();
    let mut checkin = Map::new();
    let mut map = Map::new();
    let mut section = "";
    for line in text.lines() {
        let t = line.trim();
        if t.is_empty() || t.starts_with(';') || t.starts_with('#') {
            continue;
        }
        if t.starts_with('[') && t.ends_with(']') {
            section = &t[1..t.len() - 1];
            continue;
        }
        let Some((k, v)) = t.split_once('=') else { continue };
        let (k, v) = (k.trim(), v.trim());
        match section {
            "window" => {
                if k == "x" {
                    window.0 = v.parse().unwrap_or(0);
                } else if k == "y" {
                    window.1 = v.parse().unwrap_or(0);
                }
            }
            "player" => {
                if k == "level" || k == "outLevel" {
                    player.insert(k.into(), json!(v.parse::<i64>().unwrap_or(1)));
                } else {
                    player.insert(k.into(), json!(v));
                }
            }
            "potions" => {
                potions.insert(k.into(), json!(v.parse::<i64>().unwrap_or(0)));
            }
            "checkin" => {
                checkin.insert(k.into(), json!(v.parse::<i64>().unwrap_or(0)));
            }
            "map" => {
                if k == "mapid" {
                    map.insert(k.into(), json!(v.parse::<i64>().unwrap_or(0)));
                } else {
                    map.insert(k.into(), json!(v));
                }
            }
            _ => {}
        }
    }
    let mut out = Map::new();
    out.insert("window".into(), json!([window.0, window.1]));
    out.insert("level".into(), player.get("level").cloned().unwrap_or(json!(1)));
    out.insert("outLevel".into(), player.get("outLevel").cloned().unwrap_or(json!(1)));
    out.insert("job".into(), player.get("job").cloned().unwrap_or(json!("")));
    out.insert("expMode".into(), player.get("expMode").cloned().unwrap_or(json!("value")));
    out.insert("partyMode".into(), player.get("partyMode").cloned().unwrap_or(json!("solo")));
    out.insert("potions".into(), Value::Object(potions));
    out.insert("checkin999".into(), checkin.get("checkin999").cloned().unwrap_or(Value::Null));
    out.insert("merchant".into(), checkin.get("merchant").cloned().unwrap_or(Value::Null));
    out.insert("boss".into(), checkin.get("boss").cloned().unwrap_or(Value::Null));
    out.insert("map".into(), Value::Object(map));
    Value::Object(out)
}

// ---------- 收益上报 ----------
// 本地开发默认 http://127.0.0.1:3001,打包后默认生产域名;均可用环境变量覆盖
fn api_base() -> (String, String) {
    let packaged = !cfg!(debug_assertions);
    let api = std::env::var("EXP_API").unwrap_or_else(|_| {
        if packaged {
            "https://mxd.zhuzhu.website/api/exp/report".into()
        } else {
            "http://127.0.0.1:3001/api/exp/report".into()
        }
    });
    let site = std::env::var("EXP_SITE").unwrap_or_else(|_| {
        if packaged {
            "https://mxd.zhuzhu.website".into()
        } else {
            "http://127.0.0.1:3001".into()
        }
    });
    (api, site)
}

fn save_record(base: &Path, payload: &Value) -> String {
    let dir = base.join("records");
    let _ = std::fs::create_dir_all(&dir);
    // 同 Electron 的 toISOString().replace(/[:.]/g,'-') → 2026-08-25T14-41-04-870Z
    let stamp = chrono::Utc::now()
        .format("%Y-%m-%dT%H:%M:%S%.3fZ")
        .to_string()
        .replace([':', '.'], "-");
    let file = dir.join(format!("rec-{stamp}.json"));
    let _ = std::fs::write(&file, serde_json::to_string_pretty(payload).unwrap_or_default());
    file.display().to_string()
}

// 上报重试:首次立即,失败退避 5s → 10s;400/403/413 数据或密钥问题,重试无意义
fn report_to_server(api: &str, site: &str, payload: &Value) -> Value {
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(15))
        .timeout(Duration::from_secs(15))
        .build();
    let delays = [0u64, 5000, 10000];
    let mut last: Option<Value> = None;
    for (i, d) in delays.iter().enumerate() {
        if i > 0 {
            std::thread::sleep(Duration::from_millis(*d));
        }
        match agent
            .post(api)
            .set("Content-Type", "application/json")
            .send_json(payload)
        {
            Ok(resp) => {
                let body: Value = resp.into_json().unwrap_or(Value::Null);
                // 服务端返参 id(即 deviceId)→ 拼本设备分享链接 exp.html?id=<id>
                let id = body
                    .get("id")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| {
                        payload
                            .get("deviceId")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string()
                    });
                return json!({
                    "ok": true,
                    "report": body.get("report").cloned(),
                    "id": id,
                    "shareUrl": format!("{site}/exp.html?id={id}") // id 为 hex+'-',无需转义
                });
            }
            Err(ureq::Error::Status(code, resp)) => {
                let body: Value = resp.into_json().unwrap_or(Value::Null);
                last = Some(json!({
                    "ok": false,
                    "status": code,
                    "error": body.get("error").and_then(|v| v.as_str()).map(|s| s.to_string()).unwrap_or(format!("HTTP {code}"))
                }));
                if matches!(code, 400 | 403 | 413) {
                    break;
                }
            }
            Err(e) => {
                last = Some(json!({ "ok": false, "status": 0, "error": e.to_string() })); // 网络错误 → 退避重试
            }
        }
    }
    last.unwrap_or(json!({ "ok": false, "status": 0, "error": "未知错误" }))
}

// ---------- 内嵌数据读取 ----------
fn read_json(name: &str) -> Value {
    let f = DATA.get_file(name).expect("内嵌数据缺失");
    serde_json::from_str(f.contents_utf8().expect("数据非 UTF-8")).expect("数据 JSON 无效")
}

#[tauri::command]
async fn get_data() -> Result<Value, String> {
    // 药水图标编码为 data URL(58 张共约 150KB,启动时一次注入)
    let mut icons = Map::new();
    if let Ok(files) = DATA.find("potion-icons/*.png") {
        for f in files {
            if let Some(file) = f.as_file() {
                let id = f.path().file_stem().and_then(|s| s.to_str()).unwrap_or("");
                icons.insert(
                    id.to_string(),
                    json!(format!(
                        "data:image/png;base64,{}",
                        base64::engine::general_purpose::STANDARD.encode(file.contents())
                    )),
                );
            }
        }
    }
    Ok(json!({
        "expTable": read_json("exp-table.json"),
        "maps": read_json("maps.json"),
        "potions": read_json("potions.json"),
        "jobs": read_json("jobs.json"),
        "icons": icons,
        "potionIconDir": null, // 仅 Electron 版使用(拼 file:// 路径),此处图标已内嵌
    }))
}

// ---------- 悬浮面板(外置于主条的独立小窗,不改变主条布局) ----------
static POPUP_CREATE: Mutex<()> = Mutex::new(());

fn ensure_popup(app: &tauri::AppHandle) -> tauri::Result<tauri::WebviewWindow> {
    if let Some(w) = app.get_webview_window("popup") {
        return Ok(w);
    }
    // 串行化创建:并发 build 同 label 窗口会产生两个窗口(页面加载两遍,空白面板覆盖有数据的面板)
    let _guard = POPUP_CREATE.lock().unwrap();
    if let Some(w) = app.get_webview_window("popup") {
        return Ok(w);
    }
    let popup = WebviewWindowBuilder::new(app, "popup", WebviewUrl::App("popup.html".into()))
        .inner_size(240.0, 200.0)
        .decorations(false)
        .resizable(false)
        .maximizable(false)
        .minimizable(false)
        .transparent(true) // 圆角面板
        .always_on_top(true)
        .skip_taskbar(true)
        .focused(false) // 默认不抢主条焦点
        .visible(false)
        // WebView 无真实网络请求(数据内嵌、上报走 Rust),禁用系统代理,
        // 避免代理软件未运行/劫持 tauri.localhost 时页面加载失败
        .additional_browser_args("--no-proxy-server")
        // 首次导航可能抢在自定义协议注册前被劫持 → 异常页面时重新导航自愈
        .on_page_load(|webview, payload| heal_page_load(&webview, payload.url().as_str(), "popup.html"))
        .build()?;
    // 面板失焦(点回主条或游戏)时收起
    let popup2 = popup.clone();
    popup.on_window_event(move |event| {
        if let WindowEvent::Focused(false) = event {
            let app = popup2.app_handle();
            let state = app.state::<AppState>();
            if state.popup_focusable.load(Ordering::SeqCst) && !popup2.is_focused().unwrap_or(false) {
                hide_raw(&popup2);
                // 时间面板关闭后把焦点还给主条(点游戏关闭时不抢焦点)
                if let Some(main) = app.get_webview_window("main") {
                    let _ = main.set_focus();
                }
            }
        }
    });
    Ok(popup)
}

// 悬浮面板显隐一律走原生 ShowWindow,绕过 tao 的可见性状态管理:
// 面板用 SW_SHOWNOACTIVATE 显示(不抢游戏/主条焦点,与 Electron showInactive 等价),
// tao 内部始终认为它是隐藏的,若混用 tauri 的 hide() 会因状态无变化而跳过 → 面板关不掉
#[cfg(windows)]
fn show_raw(popup: &tauri::WebviewWindow, activate: bool) {
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::SetFocus;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        SetForegroundWindow, SetWindowPos, ShowWindow, HWND_TOPMOST, SWP_NOACTIVATE, SWP_NOMOVE,
        SWP_NOSIZE, SW_SHOW, SW_SHOWNOACTIVATE,
    };
    if let Ok(hwnd) = popup.hwnd() {
        let hwnd = hwnd.0 as *mut _; // raw-window-handle 的 HWND 元组 → windows-sys 指针
        unsafe {
            ShowWindow(hwnd, if activate { SW_SHOW } else { SW_SHOWNOACTIVATE });
            SetWindowPos(
                hwnd,
                HWND_TOPMOST,
                0,
                0,
                0,
                0,
                SWP_NOACTIVATE | SWP_NOMOVE | SWP_NOSIZE,
            );
            if activate {
                SetForegroundWindow(hwnd); // 时间选择器需要键盘输入 → 激活收键
                SetFocus(hwnd);
            }
        }
    }
}

#[cfg(windows)]
fn hide_raw(popup: &tauri::WebviewWindow) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{ShowWindow, SW_HIDE};
    if let Ok(hwnd) = popup.hwnd() {
        unsafe { ShowWindow(hwnd.0 as *mut _, SW_HIDE); }
    }
}

// ---------- 吸顶 / 收缩 ----------
// 吸顶:窗口拖到工作区顶部 24px 内 → 贴顶吸附,阈值内拖动被吸住;拖离 → 退出。
// 收缩:吸顶且鼠标不在窗口内 3 秒(由渲染层计时)→ 高度收成一根粗线,悬停还原。
// 粗线模式下 999 打卡 / 神秘商人的提醒由渲染层在线上闪亮发光呈现。
static COLLAPSE_LOCK: Mutex<()> = Mutex::new(()); // 串行化收缩/还原,防并发竞态

fn collapse_line_height(window: &tauri::Window) -> u32 {
    let scale = window.scale_factor().unwrap_or(1.0);
    ((COLLAPSED_H_CSS * scale).round() as u32).max(4)
}

// 鼠标此刻是否停在窗口内:收缩前复核,防"计时器刚触发、鼠标恰好进入"竞态
#[cfg(windows)]
fn cursor_over(window: &tauri::Window) -> bool {
    use windows_sys::Win32::Foundation::POINT;
    use windows_sys::Win32::UI::WindowsAndMessaging::GetCursorPos;
    let (Ok(p), Ok(s)) = (window.outer_position(), window.outer_size()) else {
        return false;
    };
    let mut pt = POINT { x: 0, y: 0 };
    if unsafe { GetCursorPos(&mut pt) } == 0 {
        return false;
    }
    pt.x >= p.x && pt.x < p.x + s.width as i32 && pt.y >= p.y && pt.y < p.y + s.height as i32
}
#[cfg(not(windows))]
fn cursor_over(_window: &tauri::Window) -> bool {
    false
}

// 还原收缩前高度;未收缩时无操作。发事件让渲染层同步样式
fn restore_height(window: &tauri::WebviewWindow, state: &AppState) {
    if !state.collapsed.swap(false, Ordering::SeqCst) {
        return;
    }
    if let Ok(size) = window.inner_size() {
        let h = state.expanded_height.lock().unwrap().unwrap_or(size.height);
        let _ = window.set_size(PhysicalSize::new(size.width, h));
    }
    let _ = window.emit("collapsed-changed", json!({ "collapsed": false }));
}

// Moved 时判定吸顶;贴顶/还原高度(左上角锚定 → 窗口始终向下展开,顶边不动)
fn eval_snap(window: &tauri::WebviewWindow) {
    let state = window.app_handle().state::<AppState>();
    let Ok(pos) = window.outer_position() else { return };
    let Ok(Some(mon)) = window.current_monitor() else { return };
    let top = mon.work_area().position.y;
    let near_top = pos.y - top <= SNAP_THRESHOLD;
    let snapped = state.snap_on.load(Ordering::SeqCst);
    if near_top && !snapped {
        state.snap_on.store(true, Ordering::SeqCst);
        if pos.y != top {
            let _ = window.set_position(PhysicalPosition::new(pos.x, top));
        }
        let _ = window.emit("snap-changed", json!({ "snapped": true }));
    } else if !near_top && snapped {
        state.snap_on.store(false, Ordering::SeqCst);
        restore_height(window, &state); // 离开吸顶必须还原(粗线只在吸顶态存在)
        let _ = window.emit("snap-changed", json!({ "snapped": false }));
    } else if near_top && snapped && pos.y != top {
        let _ = window.set_position(PhysicalPosition::new(pos.x, top)); // 阈值内拖回贴顶
    }
}

// ---------- IPC 命令(与原 Electron 版同名语义) ----------
// 注意:同步命令跑在主线程,而窗口 API(outer_position/建窗等)要等事件循环回包,
// 从主线程调用会死锁 → 全部命令一律 async,跑在 tokio 工作线程
#[tauri::command]
async fn get_device_id(state: tauri::State<'_, AppState>) -> Result<String, String> {
    Ok(state.device_id.clone())
}

#[tauri::command]
async fn get_settings(state: tauri::State<'_, AppState>) -> Result<Value, String> {
    Ok(state.settings.lock().unwrap().clone())
}

#[tauri::command]
async fn save_settings(app: tauri::AppHandle, patch: Value) -> Result<(), String> {
    save_settings_patch(&app, patch);
    Ok(())
}

#[tauri::command]
async fn submit_record(state: tauri::State<'_, AppState>, payload: Value) -> Result<Value, String> {
    let base = state.base_dir.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let (api, site) = api_base();
        let file = save_record(&base, &payload); // 本地存档兜底,无论上报成败
        println!("[submit] 本地存档: {file}");
        let r = report_to_server(&api, &site, &payload);
        if r["ok"] == true {
            println!("[submit] 上报成功: {}", r["report"]["profit"]);
        } else {
            eprintln!("[submit] 上报失败 [{}]: {}", r["status"], r["error"]);
        }
        r
    })
    .await
    .map_err(|e| e.to_string())
}

#[tauri::command]
async fn open_site(app: tauri::AppHandle, url: Option<String>) -> Result<(), String> {
    let url = url.unwrap_or_else(|| api_base().1);
    app.opener().open_url(url, None::<&str>).map_err(|e| e.to_string())
}

#[tauri::command]
async fn close_app(window: tauri::Window) -> Result<(), String> {
    window.close().map_err(|e| e.to_string())
}

// JS 拖动:渲染层发起点/位移,主进程移动窗口
#[tauri::command]
async fn drag_start(window: tauri::Window, state: tauri::State<'_, AppState>) -> Result<(), String> {
    if let Ok(p) = window.outer_position() {
        *state.drag_origin.lock().unwrap() = Some((p.x, p.y));
    }
    Ok(())
}

#[tauri::command]
async fn drag_move(window: tauri::Window, state: tauri::State<'_, AppState>, dx: f64, dy: f64) -> Result<(), String> {
    let origin = state.drag_origin.lock().unwrap();
    if let Some((x, y)) = *origin {
        let _ = window.set_position(PhysicalPosition::new(
            x + dx.round() as i32,
            y + dy.round() as i32,
        ));
    }
    Ok(())
}

#[tauri::command]
async fn drag_end(state: tauri::State<'_, AppState>) -> Result<(), String> {
    *state.drag_origin.lock().unwrap() = None;
    Ok(())
}

#[tauri::command]
async fn popup_show(app: tauri::AppHandle, state: tauri::State<'_, AppState>, opts: Value) -> Result<(), String> {
    let focusable = opts.get("focusable").and_then(|v| v.as_bool()).unwrap_or(false);
    let w = opts.get("width").and_then(|v| v.as_f64()).unwrap_or(240.0);
    let h = opts.get("height").and_then(|v| v.as_f64()).unwrap_or(240.0);
    let anchor_x = opts.get("anchorX").and_then(|v| v.as_f64()).unwrap_or(0.0);
    let popup = ensure_popup(&app).map_err(|e| e.to_string())?;
    let main = app.get_webview_window("main").ok_or("主窗口不存在")?;
    let (mx, my) = {
        let p = main.outer_position().map_err(|e| e.to_string())?;
        (p.x, p.y)
    };
    let mh = main.outer_size().map_err(|e| e.to_string())?.height as i32;
    let monitor = main
        .current_monitor()
        .map_err(|e| e.to_string())?
        .ok_or("当前屏幕不可用")?;
    let wa = monitor.work_area();
    let (wa_x, wa_y, wa_w, wa_h) = (
        wa.position.x as f64,
        wa.position.y as f64,
        wa.size.width as f64,
        wa.size.height as f64,
    );
    let width = w.min(wa_w - 8.0);
    let height = (h.min(wa_h - ((my + mh + 2) as f64 - wa_y) - 4.0)).max(40.0);
    let x = (mx as f64 + anchor_x).clamp(wa_x + 4.0, wa_x + wa_w - width - 4.0);
    popup
        .set_size(PhysicalSize::new(width, height))
        .map_err(|e| e.to_string())?;
    popup
        .set_position(PhysicalPosition::new(x as i32, my + mh + 2))
        .map_err(|e| e.to_string())?;
    state.popup_focusable.store(focusable, Ordering::SeqCst);
    popup.set_focusable(focusable).map_err(|e| e.to_string())?;
    // 需要键盘输入(时间选择器)→ 显示并激活;其余面板不抢主条焦点
    show_raw(&popup, focusable);
    Ok(())
}

#[tauri::command]
async fn popup_render(app: tauri::AppHandle, state: tauri::State<'_, AppState>, payload: Value) -> Result<(), String> {
    // 不在此创建窗口(窗口只由 popup_show 创建):并发 ensure_popup 会产生同 label 双窗口,
    // 页面加载两遍,空白窗口覆盖有数据窗口。窗口未就绪时存 pending,popup.js 加载后主动拉取
    if state.popup_ready.load(Ordering::SeqCst) {
        if let Some(popup) = app.get_webview_window("popup") {
            let _ = popup.emit("popup-render", &payload);
            return Ok(());
        }
    }
    *state.pending_render.lock().unwrap() = Some(payload);
    Ok(())
}

// 面板加载完成后调用:标记就绪并取走期间积压的渲染请求
#[tauri::command]
async fn popup_ready(state: tauri::State<'_, AppState>) -> Result<Option<Value>, String> {
    state.popup_ready.store(true, Ordering::SeqCst);
    Ok(state.pending_render.lock().unwrap().take())
}

#[tauri::command]
async fn popup_close(app: tauri::AppHandle, state: tauri::State<'_, AppState>) -> Result<(), String> {
    if let Some(popup) = app.get_webview_window("popup") {
        let was_focused = state.popup_focusable.load(Ordering::SeqCst)
            && popup.is_focused().unwrap_or(false);
        hide_raw(&popup);
        if was_focused {
            if let Some(main) = app.get_webview_window("main") {
                let _ = main.set_focus();
            }
        }
    }
    Ok(())
}

#[tauri::command]
async fn popup_pick(app: tauri::AppHandle, data: Value) -> Result<(), String> {
    if let Some(main) = app.get_webview_window("main") {
        let _ = main.emit("popup-picked", &data);
    }
    Ok(())
}

// 面板宽度贴合内容:渲染层量取控件总宽(CSS px)上报,按 DPI 缩放换算为物理像素后调整窗口
#[tauri::command]
async fn set_window_width(window: tauri::Window, width: f64) -> Result<(), String> {
    let scale = window.scale_factor().map_err(|e| e.to_string())?;
    let inner = window.inner_size().map_err(|e| e.to_string())?;
    window
        .set_size(PhysicalSize::new((width * scale).round() as u32, inner.height))
        .map_err(|e| e.to_string())
}

// 吸顶收缩:仅吸顶模式可收缩;还原任意时刻可调(未收缩时忽略)
#[tauri::command]
async fn set_collapsed(
    window: tauri::Window,
    state: tauri::State<'_, AppState>,
    collapsed: bool,
) -> Result<(), String> {
    let _guard = COLLAPSE_LOCK.lock().unwrap(); // 与收缩/还原互斥,保证先后顺序
    let app = window.app_handle();
    let main = app.get_webview_window("main").ok_or("主窗口不存在")?;
    if collapsed {
        // 非吸顶 / 已收缩 / 鼠标正悬停 → 忽略(悬停复核防计时器竞态)
        if !state.snap_on.load(Ordering::SeqCst)
            || state.collapsed.load(Ordering::SeqCst)
            || cursor_over(&window)
        {
            return Ok(());
        }
        // 存内尺寸而非外尺寸:外尺寸(GetWindowRect)含 DWM 阴影,还原时会把阴影算进真实高度
        let size = window.inner_size().map_err(|e| e.to_string())?;
        *state.expanded_height.lock().unwrap() = Some(size.height);
        window
            .set_size(PhysicalSize::new(size.width, collapse_line_height(&window)))
            .map_err(|e| e.to_string())?;
        // 左上角锚定:变矮后顶边不动 → 向下收成细线,仍贴合顶部
        if let (Ok(p), Ok(Some(m))) = (window.outer_position(), window.current_monitor()) {
            let top = m.work_area().position.y;
            if p.y != top {
                let _ = window.set_position(PhysicalPosition::new(p.x, top));
            }
        }
        // 主条没了,锚在其下方的悬浮面板一并收起
        if let Some(popup) = app.get_webview_window("popup") {
            hide_raw(&popup);
        }
        state.collapsed.store(true, Ordering::SeqCst);
        let _ = main.emit("collapsed-changed", json!({ "collapsed": true }));
    } else {
        restore_height(&main, &state);
    }
    Ok(())
}

// 渲染层初始化时拉取吸顶/收缩现状(启动即吸顶时页面可能错过事件)
#[tauri::command]
async fn get_snap_state(state: tauri::State<'_, AppState>) -> Result<Value, String> {
    Ok(json!({
        "snapped": state.snap_on.load(Ordering::SeqCst),
        "collapsed": state.collapsed.load(Ordering::SeqCst),
    }))
}

fn main() {
    // 单实例:双击/多开会产生共享同一 user-data-dir 的僵尸 WebView2 浏览器进程,
    // 导致加载失败、悬浮面板定位异常等互扰 → 已运行时直接退出
    #[cfg(windows)]
    {
        use windows_sys::Win32::Foundation::{GetLastError, ERROR_ALREADY_EXISTS};
        use windows_sys::Win32::System::Threading::CreateMutexW;
        let name: Vec<u16> = format!(
            "mxd-bar-single-instance-{}",
            if cfg!(debug_assertions) { "dev" } else { "rel" }
        )
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
        let handle = unsafe { CreateMutexW(std::ptr::null(), 0, name.as_ptr()) };
        if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
            std::process::exit(0);
        }
        // HANDLE 是裸指针无析构,句柄随进程存续,互斥在进程生命周期内一直有效
        let _ = handle;
    }
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            let app_base = base_dir();
            let device_id = load_device_id(&app_base);
            let settings = load_settings(&app_base);
            app.manage(AppState {
                base_dir: app_base,
                device_id: device_id.clone(),
                settings: Mutex::new(settings),
                drag_origin: Mutex::new(None),
                popup_ready: AtomicBool::new(false),
                popup_focusable: AtomicBool::new(false),
                pending_render: Mutex::new(None),
                load_heal: AtomicU32::new(0),
                snap_on: AtomicBool::new(false),
                collapsed: AtomicBool::new(false),
                expanded_height: Mutex::new(None),
            });

            // 主窗口运行时创建(配置窗口无法挂 on_page_load 启动竞态自愈)
            let main = WebviewWindowBuilder::new(app, "main", WebviewUrl::App("index.html".into()))
                .title("冒险岛怀旧服经验记录器")
                .inner_size(1600.0, 36.0)
                .position(100.0, 100.0)
                .decorations(false)
                .resizable(false)
                .maximizable(false)
                .minimizable(false)
                .always_on_top(true)
                .skip_taskbar(true)
                // 无边框窗口默认带 DWM 阴影+隐形边框(GetWindowRect 比客户区大一圈),
                // 收缩成 6px 粗线时下方会拖出一片半透明阴影 → 关掉
                .shadow(false)
                .additional_browser_args("--no-proxy-server")
                .on_page_load(|webview, payload| heal_page_load(&webview, payload.url().as_str(), "index.html"))
                .build()?;
            // 关阴影后 Win11 不再默认圆角 → 显式请求圆角(老系统不支持时忽略)
            #[cfg(windows)]
            {
                use windows_sys::Win32::Graphics::Dwm::{
                    DwmSetWindowAttribute, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND,
                };
                if let Ok(hwnd) = main.hwnd() {
                    let pref = DWMWCP_ROUND;
                    unsafe {
                        DwmSetWindowAttribute(
                            hwnd.0 as *mut _,
                            DWMWA_WINDOW_CORNER_PREFERENCE as u32,
                            &pref as *const _ as *const _,
                            std::mem::size_of_val(&pref) as u32,
                        );
                    }
                }
            }
            {
                // 恢复上次窗口位置(须仍在屏幕工作区内,否则忽略)
                if let Some(wp) = app.state::<AppState>().settings.lock().unwrap().get("window").and_then(|v| v.as_array()).cloned() {
                    if wp.len() == 2 {
                        if let (Some(x), Some(y)) = (wp[0].as_i64(), wp[1].as_i64()) {
                            let on_screen = main
                                .available_monitors()
                                .unwrap_or_default()
                                .iter()
                                .any(|m| {
                                    let wa = m.work_area();
                                    let (wx, wy, ww, wh) = (
                                        wa.position.x,
                                        wa.position.y,
                                        wa.size.width as i32,
                                        wa.size.height as i32,
                                    );
                                    x >= (wx - 16) as i64
                                        && y >= (wy - 16) as i64
                                        && x <= (wx + ww - 16) as i64
                                        && y <= (wy + wh - 16) as i64
                                });
                            if on_screen {
                                let _ = main.set_position(PhysicalPosition::new(x as i32, y as i32));
                            }
                        }
                    }
                }
                // 失焦(点回游戏)或拖动主条时收起悬浮面板(可聚焦面板打开期间除外);关闭时记住窗口位置
                let main2 = main.clone();
                main.on_window_event(move |event| {
                    let app = main2.app_handle();
                    let state = app.state::<AppState>();
                    match event {
                        WindowEvent::Focused(false) => {
                            let popup_focused = app
                                .get_webview_window("popup")
                                .map(|p| p.is_focused().unwrap_or(false))
                                .unwrap_or(false);
                            let focusable = state.popup_focusable.load(Ordering::SeqCst);
                            if !(focusable && popup_focused) {
                                if let Some(popup) = app.get_webview_window("popup") {
                                    hide_raw(&popup);
                                }
                            }
                        }
                        WindowEvent::Moved(_) => {
                            if let Some(popup) = app.get_webview_window("popup") {
                                hide_raw(&popup);
                            }
                            eval_snap(&main2); // 拖动贴顶/拖离的吸顶切换
                        }
                        WindowEvent::CloseRequested { .. } => {
                            if let Ok(p) = main2.outer_position() {
                                save_settings_patch(&app, json!({ "window": [p.x, p.y] }));
                            }
                        }
                        // 悬浮面板只是隐藏不销毁,主窗关闭时需显式退出
                        WindowEvent::Destroyed => {
                            let _ = app.exit(0);
                        }
                        _ => {}
                    }
                });
            }

            // 上次位置贴近顶部 → 启动即恢复吸顶(事件此时无页面接收,渲染层用 get_snap_state 同步)
            eval_snap(&main);

            // 开发冒烟测试:验证 saveRecord 写入、deviceId 与内嵌数据完整性,随后退出
            if std::env::args().any(|a| a == "--smoke-test") {
                let d = tauri::async_runtime::block_on(get_data()).expect("内嵌数据读取失败");
                let file = save_record(
                    &base_dir(),
                    &json!({
                        "deviceId": device_id,
                        "smoke": true,
                        "time": chrono::Utc::now().to_rfc3339(),
                        "dataCheck": {
                            "maps": d["maps"].as_array().map(|a| a.len()).unwrap_or(0),
                            "potions": d["potions"].as_array().map(|a| a.len()).unwrap_or(0),
                            "expLevels": d["expTable"]["perLevel"].as_array().map(|a| a.len()).unwrap_or(0),
                            "jobs": d["jobs"].as_array().map(|a| a.len()).unwrap_or(0),
                        }
                    }),
                );
                println!("[smoke-test] saved to: {file}");
                std::process::exit(0);
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_data,
            get_device_id,
            get_settings,
            save_settings,
            submit_record,
            open_site,
            close_app,
            drag_start,
            drag_move,
            drag_end,
            popup_show,
            popup_render,
            popup_close,
            popup_pick,
            popup_ready,
            set_window_width,
            set_collapsed,
            get_snap_state
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
