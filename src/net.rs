//! 联网/系统集成：本机 UID、v2 上报接口（token + report）、浏览器打开。
//!
//! 对接 exp-api.md 的 **v2** 协议，用了三个接口（PATCH 编辑能力不做）：
//! - `POST /api/v2/exp/token`：用固定设备密钥换 2h 的 JWT（`X-Exp-Device-Secret: zhuzhu`）；
//! - `POST /api/v2/exp/report`：带 `Authorization: Bearer <token>` 上报一帧快照；
//! - `POST /api/v2/exp/pk`：带同一 token 上报预估 EXP/h 换"实时效率 PK"名次（见 pk-api.md）。
//!
//! 服务地址二选一（见 `api_base`）：生产 `https://mxd.zhuzhu.website`，本地联调
//! `http://127.0.0.1:3001`，由 data.ini `[net] base=local` 切到本地、缺省走生产。
//!
//! HTTP 客户端用系统自带 WinHTTP（`winhttp.dll`），同步阻塞，跑在后台线程；
//! https 由系统 Schannel 校验，无需额外 TLS 依赖（反外挂合规与本机环境都不影响）。
//!
//! token 生命周期：启动后由 `spawn_token_keeper` 在后台预换一次并缓存到 `Shared`，
//! 距过期 <10 分钟自动重换；上报遇 401 会再换一次重试。token 由 JWT 的 sub 指认设备，
//! 服务端不信任 body 里的设备字段，故 deviceId（主板 MD5 uid）只出现在换 token 请求里。

use std::fmt::Write as _;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use md5::{Digest, Md5};
use windows::core::PCWSTR;
use windows::Win32::Networking::WinHttp::{
    WinHttpAddRequestHeaders, WinHttpCloseHandle, WinHttpConnect, WinHttpOpen, WinHttpOpenRequest,
    WinHttpQueryDataAvailable, WinHttpQueryHeaders, WinHttpReadData, WinHttpReceiveResponse,
    WinHttpSendRequest, WinHttpSetTimeouts, WINHTTP_ACCESS_TYPE_NO_PROXY, WINHTTP_ADDREQ_FLAG_ADD,
    WINHTTP_FLAG_SECURE, WINHTTP_OPEN_REQUEST_FLAGS, WINHTTP_QUERY_FLAG_NUMBER,
    WINHTTP_QUERY_STATUS_CODE,
};

use crate::config::AppConfig;
use crate::state::Shared;

// ---------------------------------------------------------------------------
// 服务地址与端点
// ---------------------------------------------------------------------------

/// 生产服务端基址。
const PROD_BASE: &str = "https://mxd.zhuzhu.website";
/// 本地联调基址。
const LOCAL_BASE: &str = "http://127.0.0.1:3001";
/// 换 token 的固定设备密钥（服务端 EXP_DEVICE_SECRET）。
const DEVICE_SECRET: &str = "zhuzhu";

const TOKEN_PATH: &str = "/api/v2/exp/token";
const REPORT_PATH: &str = "/api/v2/exp/report";
/// 实时效率 PK：每 10s 上报预估 EXP/h，服务端返回名次（见 pk-api.md）。
const PK_PATH: &str = "/api/v2/exp/pk";
/// token 距过期不足此秒数即视为"快过期"，提前重换。
const TOKEN_MARGIN: Duration = Duration::from_secs(600);

/// 环境变量 `MXD_BAR_API=local` 是否要求走本地（联调/自测用，优先级最高）。
fn env_local() -> bool {
    std::env::var_os("MXD_BAR_API").is_some_and(|v| v.eq_ignore_ascii_case("local"))
}

/// 当前应使用的服务端基址：本地 = 环境变量 `MXD_BAR_API=local` 或 data.ini `[net] base=local`；
/// 否则生产。
pub fn api_base(cfg: &AppConfig) -> &'static str {
    if cfg.net_local || env_local() {
        LOCAL_BASE
    } else {
        PROD_BASE
    }
}

/// 官网地址（「关于」页展示/点开用）。即生产服务端基址——联调模式下官网也照指生产站。
pub fn official_site() -> &'static str {
    PROD_BASE
}

/// 是否正在走本地基址（用于 `--nettest` 等只限本地操作的防护；仅 debug 用）。
#[cfg(debug_assertions)]
pub fn local_enabled(cfg: &AppConfig) -> bool {
    cfg.net_local || env_local()
}

/// 上报成功页的分享链接（打开只显示这一条记录）。
pub fn share_url(base: &str, id: &str) -> String {
    format!("{base}/exp.html?id={id}")
}

/// 管理数据页：带 token 打开，网页据此识别出本设备并点亮其编辑/管理入口；
/// token 还没有时退化为不带 token 的只读数据页（查看、不授权）。
pub fn manage_url(base: &str, token: Option<&str>) -> String {
    match token {
        Some(t) => format!("{base}/exp.html?token={t}"),
        None => format!("{base}/exp.html"),
    }
}

// ---------------------------------------------------------------------------
// 低层 HTTP：同步 POST JSON（WinHTTP）
// ---------------------------------------------------------------------------

/// 把 `scheme://host[:port]` 拆成 (host, port, secure)。仅支持 http/https。
fn base_parts(base: &str) -> Result<(String, u16, bool), String> {
    let (auth, secure, def_port) = if let Some(r) = base.strip_prefix("https://") {
        (r, true, 443u16)
    } else if let Some(r) = base.strip_prefix("http://") {
        (r, false, 80u16)
    } else {
        return Err("服务地址必须以 http:// 或 https:// 开头".into());
    };
    let hp = auth.split(['/', '?']).next().unwrap_or("").trim_end_matches('/');
    if hp.is_empty() {
        return Err("服务地址缺少主机名".into());
    }
    if let Some(i) = hp.rfind(':') {
        let (host, port) = (&hp[..i], &hp[i + 1..]);
        if host.is_empty() {
            return Err("服务地址缺少主机名".into());
        }
        let port: u16 = port.parse().map_err(|_| "服务地址端口非法".to_string())?;
        Ok((host.to_owned(), port, secure))
    } else {
        Ok((hp.to_owned(), def_port, secure))
    }
}

/// 转成带结尾 NUL 的 UTF-16 缓冲区（WinHTTP 的 C 接口要的是以 NUL 结尾字符串）。
fn wcs(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// WinHTTP 句柄的 RAII 关闭（session/connect/request 三档都适用）。
struct H(*mut core::ffi::c_void);
impl H {
    fn new(p: *mut core::ffi::c_void) -> Self {
        H(p)
    }
}
impl Drop for H {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe {
                let _ = WinHttpCloseHandle(self.0);
            }
        }
    }
}

/// 把 windows-core 错误转成一句人能看的话；常见的连接类错误给中文提示，其余带 0x 码。
fn winhttp_msg(ctx: &str, e: &windows::core::Error) -> String {
    let hr = e.code().0 as u32;
    let hint = match hr {
        0x80072ee7 => "服务器域名无法解析",
        0x80072efd => "无法连接到服务器",
        0x80072ee2 => "连接超时",
        0x80072ef5 => "连接被重置",
        _ => "",
    };
    if hint.is_empty() {
        format!("{ctx}（0x{hr:08x}）")
    } else {
        format!("{ctx}：{hint}")
    }
}

/// 同步发一个 POST JSON 请求，返回 (HTTP 状态码, 响应体文本)。
/// `headers` 是额外要加的键值（Content-Type 等在此传）。
fn http_post_json(
    base: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: &str,
) -> Result<(u16, String), String> {
    let (host, port, secure) = base_parts(base)?;

    let agent = wcs("mxd-bar/1.0");
    let session = H::new(unsafe {
        WinHttpOpen(
            PCWSTR(agent.as_ptr()),
            WINHTTP_ACCESS_TYPE_NO_PROXY,
            PCWSTR(std::ptr::null()),
            PCWSTR(std::ptr::null()),
            0,
        )
    });
    if session.0.is_null() {
        return Err("WinHTTP 初始化失败".into());
    }
    // 失败则用保守超时兜底；忽略返回值，只是尽力而为。
    unsafe {
        let _ = WinHttpSetTimeouts(session.0, 6000, 6000, 12000, 20000);
    }

    let srv = wcs(&host);
    let conn = H::new(unsafe { WinHttpConnect(session.0, PCWSTR(srv.as_ptr()), port, 0) });
    if conn.0.is_null() {
        return Err("无法建立到服务器的连接".into());
    }

    let verb = wcs("POST");
    let obj = wcs(path);
    let flags = if secure {
        WINHTTP_FLAG_SECURE
    } else {
        WINHTTP_OPEN_REQUEST_FLAGS(0)
    };
    let req = H::new(unsafe {
        WinHttpOpenRequest(
            conn.0,
            PCWSTR(verb.as_ptr()),
            PCWSTR(obj.as_ptr()),
            PCWSTR(std::ptr::null()),
            PCWSTR(std::ptr::null()),
            std::ptr::null(),
            flags,
        )
    });
    if req.0.is_null() {
        return Err("创建请求失败".into());
    }

    if !headers.is_empty() {
        let mut hs = String::new();
        for (k, v) in headers {
            hs.push_str(k);
            hs.push_str(": ");
            hs.push_str(v);
            hs.push_str("\r\n");
        }
        let hw: Vec<u16> = hs.encode_utf16().collect();
        unsafe {
            WinHttpAddRequestHeaders(req.0, &hw, WINHTTP_ADDREQ_FLAG_ADD)
                .map_err(|e| winhttp_msg("添加请求头失败", &e))?;
        }
    }

    let bytes = body.as_bytes();
    unsafe {
        WinHttpSendRequest(
            req.0,
            None,
            Some(bytes.as_ptr() as *const core::ffi::c_void),
            bytes.len() as u32,
            bytes.len() as u32,
            0,
        )
        .map_err(|e| winhttp_msg("发送请求失败", &e))?;
        WinHttpReceiveResponse(req.0, std::ptr::null_mut())
            .map_err(|e| winhttp_msg("接收响应失败", &e))?;
    }

    let mut code: u32 = 0;
    let mut csz: u32 = std::mem::size_of::<u32>() as u32;
    unsafe {
        WinHttpQueryHeaders(
            req.0,
            WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
            PCWSTR(std::ptr::null()),
            Some(&mut code as *mut u32 as *mut core::ffi::c_void),
            &mut csz,
            std::ptr::null_mut(),
        )
        .map_err(|e| winhttp_msg("读取状态码失败", &e))?;
    }

    // 分段读完整响应体（上限 1MB 兜底，正常一帧 ~1KB）。
    let mut out: Vec<u8> = Vec::new();
    loop {
        let mut avail: u32 = 0;
        unsafe {
            WinHttpQueryDataAvailable(req.0, &mut avail)
                .map_err(|e| winhttp_msg("读取响应长度失败", &e))?;
        }
        if avail == 0 {
            break;
        }
        let mut buf = vec![0u8; avail as usize];
        let mut got: u32 = 0;
        unsafe {
            WinHttpReadData(req.0, buf.as_mut_ptr() as *mut core::ffi::c_void, avail, &mut got)
                .map_err(|e| winhttp_msg("读取响应内容失败", &e))?;
        }
        if got == 0 {
            break;
        }
        out.extend_from_slice(&buf[..got as usize]);
        if out.len() > 1 << 20 {
            break;
        }
    }

    Ok((code as u16, String::from_utf8_lossy(&out).into_owned()))
}

// ---------------------------------------------------------------------------
// v2 端点
// ---------------------------------------------------------------------------

/// `POST /api/v2/exp/token`：拿固定密钥换 JWT。返回 (token, expiresIn 秒)。
pub fn request_token(base: &str, device_id: &str) -> Result<(String, u32), String> {
    let body = serde_json::json!({ "deviceId": device_id }).to_string();
    let headers = [
        ("Content-Type", "application/json"),
        ("X-Exp-Device-Secret", DEVICE_SECRET),
    ];
    let (status, text) = http_post_json(base, TOKEN_PATH, &headers, &body)?;
    let v: serde_json::Value =
        serde_json::from_str(&text).map_err(|_| format!("token 响应不是合法 JSON（HTTP {status}）"))?;
    if status == 200 {
        let tok = v["token"]
            .as_str()
            .ok_or("token 响应缺少 token 字段")?;
        let exp = v["expiresIn"]
            .as_u64()
            .ok_or("token 响应缺少 expiresIn 字段")?;
        Ok((tok.to_owned(), exp as u32))
    } else {
        let err = v["error"].as_str().unwrap_or("未知原因");
        if status == 403 {
            Err(format!("设备密钥错误（HTTP 403）：{err}"))
        } else {
            Err(format!("换取 token 失败（HTTP {status}）：{err}"))
        }
    }
}

/// 一次上报的网络结果。
#[derive(Debug)]
pub struct ApiError {
    /// HTTP 状态码（网络层失败为 None）。
    pub status: Option<u16>,
    /// 可直接展示给用户的原因。
    pub message: String,
}

/// `POST /api/v2/exp/report`：带 token 上报。成功返回服务端生成的记录 id。
pub fn post_report(base: &str, token: &str, payload: &serde_json::Value) -> Result<String, ApiError> {
    let auth = format!("Bearer {token}");
    let headers = [("Content-Type", "application/json"), ("Authorization", &auth)];
    let (status, text) = http_post_json(base, REPORT_PATH, &headers, &payload.to_string())
        .map_err(|e| ApiError { status: None, message: e })?;
    let v: serde_json::Value = serde_json::from_str(&text).unwrap_or(serde_json::Value::Null);
    if status == 200 {
        return v["id"]
            .as_str()
            .map(str::to_owned)
            .ok_or_else(|| ApiError {
                status: Some(status),
                message: "上报成功但响应缺少 id 字段".into(),
            });
    }
    let err = v["error"].as_str().unwrap_or("服务器未返回具体原因");
    let message = match status {
        401 => "令牌无效或已过期".to_string(),
        400 => format!("数据校验未通过：{err}"),
        413 => "请求体过大".to_string(),
        429 => "上报过于频繁（同一设备两次成功上报需间隔≥5秒），请稍后再试".to_string(),
        _ => format!("上报失败（HTTP {status}）：{err}"),
    };
    Err(ApiError { status: Some(status), message })
}

/// `POST /api/v2/exp/pk` 的成功结果：服务端算出的名次（1 起）与参与总数。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PkRank {
    /// 名次，1 = 全场第一。服务端最多留 999 个对象，理论上 ≤999；
    /// 若服务端放宽/返回更大值，UI 显示时按 999 封顶（见 util::pk_rank_text）。
    pub rank: u32,
    /// 本次参与（数组内）总数；服务端没给时按 0，仅留作诊断。
    pub total: u32,
}

/// `POST /api/v2/exp/pk`：带 token 上报"当前预估 EXP/h"，换实时名次。
/// 请求体由调用方构造（见 pk.rs：`{ board_id, ts, exp_per_hour }`），鉴权与 v2 report 相同。
pub fn post_pk(base: &str, token: &str, payload: &serde_json::Value) -> Result<PkRank, ApiError> {
    let auth = format!("Bearer {token}");
    let headers = [("Content-Type", "application/json"), ("Authorization", &auth)];
    let (status, text) = http_post_json(base, PK_PATH, &headers, &payload.to_string())
        .map_err(|e| ApiError { status: None, message: e })?;
    let v: serde_json::Value = serde_json::from_str(&text).unwrap_or(serde_json::Value::Null);
    if status == 200 {
        return v["rank"]
            .as_u64()
            .map(|r| PkRank {
                rank: r as u32,
                total: v["total"].as_u64().unwrap_or(0) as u32,
            })
            .ok_or_else(|| ApiError {
                status: Some(status),
                message: "PK 成功但响应缺少 rank 字段".into(),
            });
    }
    let err = v["error"].as_str().unwrap_or("服务器未返回具体原因");
    let message = match status {
        401 => "令牌无效或已过期".to_string(),
        400 => format!("数据校验未通过：{err}"),
        429 => "上报过于频繁（同一设备两次成功上报需间隔≥5秒），请稍后再试".to_string(),
        _ => format!("PK 上报失败（HTTP {status}）：{err}"),
    };
    Err(ApiError { status: Some(status), message })
}

// ---------------------------------------------------------------------------
// token 生命周期（后台线程维护，结果缓存在 Shared）
// ---------------------------------------------------------------------------

/// 确保手上有没过期（且距过期 ≥10 分钟）的 token；没有就现换。
/// 设备标识（主板 MD5 uid）缺失时会当场现算并缓存。
pub fn ensure_device_token(shared: &Arc<Mutex<Shared>>) -> Result<String, String> {
    // 快路径：已缓存且还够新鲜 → 直接用。
    {
        let g = shared.lock().unwrap();
        if let (Some(t), Some(exp)) = (&g.token, g.token_exp) {
            if Instant::now() + TOKEN_MARGIN < exp {
                return Ok(t.clone());
            }
        }
    }

    // 设备标识：优先缓存，否则现算（PowerShell 主板序列号 → MD5）。
    let device = {
        let g = shared.lock().unwrap();
        g.uid.clone()
    };
    let device = match device {
        Some(u) => u,
        None => {
            let u = compute_uid()
                .ok_or_else(|| "无法读取设备标识（主板序列号），无法上报".to_string())?;
            let mut g = shared.lock().unwrap();
            g.uid = Some(u.clone());
            g.cfg.uid_cache = Some(u.clone());
            drop(g); // persist 前别握着 UI 锁太久
            let _ = shared.lock().unwrap().persist_cfg();
            u
        }
    };

    let base = {
        let g = shared.lock().unwrap();
        api_base(&g.cfg)
    };
    let (tok, exp_s) = request_token(base, &device)?;
    let mut g = shared.lock().unwrap();
    g.token = Some(tok.clone());
    g.token_exp = Some(Instant::now() + Duration::from_secs(exp_s as u64));
    g.token_error = None;
    Ok(tok)
}

/// 强制换一个新 token：清掉缓存再走一遍 `ensure_device_token`（上报遇 401 时用）。
pub fn refresh_token(shared: &Arc<Mutex<Shared>>) -> Result<String, String> {
    {
        let mut g = shared.lock().unwrap();
        g.token = None;
        g.token_exp = None;
    }
    ensure_device_token(shared)
}

/// 后台线程：启动后预换一次 token 并保持刷新（UI 不阻塞）。进程退出随主线程一起结束。
pub fn spawn_token_keeper(shared: Arc<Mutex<Shared>>) {
    std::thread::spawn(move || loop {
        let (has_uid, fresh) = {
            let g = shared.lock().unwrap();
            let fresh = g.token.is_some()
                && g.token_exp
                    .is_some_and(|e| Instant::now() + TOKEN_MARGIN < e);
            (g.uid.is_some(), fresh)
        };
        if has_uid && !fresh {
            if let Err(e) = ensure_device_token(&shared) {
                shared.lock().unwrap().token_error = Some(e);
            }
        }
        // uid 还在算（启动瞬间）就高频轮询等它到位；之后就 20s 检查一次即可。
        std::thread::sleep(if has_uid {
            Duration::from_secs(20)
        } else {
            Duration::from_millis(500)
        });
    });
}

// ---------------------------------------------------------------------------
// 打开浏览器
// ---------------------------------------------------------------------------

/// 用系统默认浏览器打开链接。子窗体（egui deferred viewport）里不能直接用
/// eframe 的 URL 打开机制，这里走 `cmd /C start "" url`，最省事可靠。
pub fn open_url(url: &str) {
    #[cfg(debug_assertions)]
    eprintln!("[mxd-bar] 打开网址: {url}");
    let _ = std::process::Command::new("cmd")
        .args(["/C", "start", "", url])
        .spawn();
}

// ---------------------------------------------------------------------------
// UID：主板序列号 → MD5(32hex 小写)；读不到主板回退到 MachineGuid
// ---------------------------------------------------------------------------

fn is_placeholder(s: &str) -> bool {
    let t = s.trim().to_lowercase();
    t.is_empty()
        || matches!(t.as_str(), "default string" | "none" | "not specified" | "unknown" | "default")
        || t.contains("to be filled")
}

/// 跑一段 PowerShell，取其 stdout 首行文本；失败返回 None。
fn ps(query: &str) -> Option<String> {
    let out = std::process::Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", query])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&out.stdout).into_owned())
}

fn hex_md5(s: &str) -> String {
    let d = Md5::digest(s.as_bytes());
    let mut h = String::with_capacity(32);
    for b in d {
        let _ = write!(h, "{b:02x}");
    }
    h
}

/// 计算本机 UID：主板序列号（去掉占位值后）做 MD5；主板读不到就退回
/// `HKLM\...\Cryptography\MachineGuid`。两者皆失败返回 None（上层会报错提示）。
pub fn compute_uid() -> Option<String> {
    let board = ps("(Get-CimInstance Win32_BaseBoard).SerialNumber")?.trim().to_owned();
    if !is_placeholder(&board) {
        return Some(hex_md5(&board));
    }
    let guid = ps("(Get-ItemProperty -Path 'HKLM:\\SOFTWARE\\Microsoft\\Cryptography').MachineGuid")?
        .trim()
        .to_owned();
    if guid.is_empty() {
        return None;
    }
    Some(hex_md5(&guid))
}
