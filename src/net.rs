//! 联网/系统集成占位：主板序列号 → UID(MD5) 、本地占位网页、浏览器打开。
//!
//! 远程上报接口 / 真实管理网址都未实现（见 开发.md），三处外链
//! （上报成功页的绿色网址、"前往查看"、"管理数据"）一律打开一个**本地占位网页**
//! `%APPDATA%\mxd-bar\view.html`，网页会把 URL 上的 query（uid 等）显示出来，
//! 以便离线确认「打开浏览器」整条链路是通的。日后把这里的两个 URL 构造函数
//! 换成真实远程模板即可。

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use md5::{Digest, Md5};

// ---------------------------------------------------------------------------
// 本地占位网页
// ---------------------------------------------------------------------------

const VIEW_HTML: &str = r#"<!doctype html>
<html lang="zh-CN">
<head>
<meta charset="utf-8">
<title>mxd-bar 占位页</title>
<style>
  body { font-family: "Microsoft YaHei", "Segoe UI", sans-serif; background:#141414; color:#eee; padding:48px 64px; }
  h1 { color:#ff9a2a; }
  code, pre { color:#7ce0ff; background:#000; padding:2px 6px; border-radius:4px; }
  .muted { color:#888; }
</style>
</head>
<body>
  <h1>mxd-bar 占位页</h1>
  <p class="muted">远程接口尚未接入 —— 此页仅供确认「打开浏览器」链路可用。</p>
  <p>Query：<code id="q">…</code></p>
  <p class="muted">（uid 等参数会显示在上方；接入真实后端后这里会是真实网页。）</p>
  <script>document.getElementById('q').textContent = location.search || '(空)'</script>
</body>
</html>
"#;

/// 占位网页路径：`%APPDATA%\mxd-bar\view.html`。
fn view_page_path() -> PathBuf {
    crate::config::data_dir().join("view.html")
}

/// 确保占位网页存在（不存在才写一次；已存在就复用，不改动用户内容）。
fn ensure_view_page() -> PathBuf {
    let p = view_page_path();
    if !p.is_file() {
        if let Some(dir) = p.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if std::fs::write(&p, VIEW_HTML).is_ok() {
            #[cfg(debug_assertions)]
            eprintln!("[mxd-bar] 已生成占位网页: {}", p.display());
        }
    }
    p
}

/// 把本地路径转成 `file:///C:/...` URL。路径段做 percent-encode（非 ASCII、#、% 等）。
fn file_url(path: &Path) -> String {
    let s = path.to_string_lossy().replace('\\', "/");
    let mut out = String::from("file:///");
    for ch in s.chars() {
        if ch.is_ascii_alphanumeric() || matches!(ch, '/' | '.' | '-' | '_' | '~') {
            out.push(ch);
        } else {
            let mut buf = [0u8; 4];
            for b in ch.encode_utf8(&mut buf).as_bytes() {
                let _ = write!(out, "%{b:02X}");
            }
        }
    }
    out
}

/// 上报成功页展示/跳转的网址（占位：本地网页 + view=report）。
pub fn report_view_url() -> String {
    format!("{}?view=report", file_url(&ensure_view_page()))
}

/// 管理数据网址（占位：本地网页 + uid=<32hex>）。
pub fn manage_url(uid: &str) -> String {
    format!("{}?view=manage&uid={uid}", file_url(&ensure_view_page()))
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
