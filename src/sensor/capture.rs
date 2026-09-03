//! 找到 `Maplestory_Classic.exe` 的主窗口并抓取整窗像素（top-down BGRA）。
//!
//! 反外挂约束（见 开发.md）：只允许**整窗截图再分析**，绝不读游戏内存/色块。
//! 这里用 PrintWindow(PW_RENDERFULLCONTENT) 把窗口画进一块 DIB section，读回它的位；
//! 若 PrintWindow 返回失败（D3D 独占/被保护）再回退到屏幕 DC 的 BitBlt。

use windows::core::BOOL;
use windows::Win32::Foundation::{CloseHandle, HWND, LPARAM, RECT};
use windows::Win32::Graphics::Gdi::{
    BitBlt, CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, GetDC, ReleaseDC,
    SelectObject, BITMAPINFO, BI_RGB, DIB_RGB_COLORS, SRCCOPY,
};
use windows::Win32::Storage::Xps::{PrintWindow, PRINT_WINDOW_FLAGS};
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetWindowRect, GetWindowThreadProcessId, IsWindowVisible, IsIconic,
    PW_RENDERFULLCONTENT,
};

/// 一帧 top-down BGRA 图（每像素 4 字节，行与行间无填充）。
pub struct BgraFrame {
    pub w: i32,
    pub h: i32,
    pub pixels: Vec<u8>,
}

/// 找指定 exe 的第一个进程 PID。
fn find_pid(exe: &str) -> Option<u32> {
    unsafe {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0).ok()?;
        let mut entry: PROCESSENTRY32W = std::mem::zeroed();
        entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
        if Process32FirstW(snap, &mut entry).is_err() {
            let _ = CloseHandle(snap);
            return None;
        }
        loop {
            let name = String::from_utf16_lossy(&entry.szExeFile);
            let name = name.trim_end_matches('\0');
            if name.eq_ignore_ascii_case(exe) {
                let pid = entry.th32ProcessID;
                let _ = CloseHandle(snap);
                return Some(pid);
            }
            if Process32NextW(snap, &mut entry).is_err() {
                break;
            }
        }
        let _ = CloseHandle(snap);
        None
    }
}

/// EnumWindows 回调上下文：记录匹配 pid 的第一个可见 hwnd。
struct FindCtx {
    pid: u32,
    hwnd: Option<HWND>,
}

unsafe extern "system" fn enum_proc(h: HWND, lparam: LPARAM) -> BOOL {
    unsafe {
        let ctx = &mut *(lparam.0 as *mut FindCtx);
        if IsWindowVisible(h).as_bool() {
            let mut pid: u32 = 0;
            GetWindowThreadProcessId(h, Some(&mut pid));
            if pid == ctx.pid {
                ctx.hwnd = Some(h);
                return BOOL(0); // 停止枚举
            }
        }
        BOOL(1)
    }
}

/// 找游戏主窗口 hwnd（第一个可见的顶层窗口）。
pub fn find_game_hwnd() -> Option<HWND> {
    let pid = find_pid("Maplestory_Classic.exe")?;
    let mut ctx = FindCtx { pid, hwnd: None };
    unsafe {
        let _ = EnumWindows(Some(enum_proc), LPARAM(&mut ctx as *mut FindCtx as isize));
    }
    ctx.hwnd
}

/// 整窗截图。返回 None = 窗口无效/太小/无法分配 DIB。
pub fn capture_window(hwnd: HWND) -> Option<BgraFrame> {
    unsafe {
        let mut rc: RECT = std::mem::zeroed();
        GetWindowRect(hwnd, &mut rc).ok()?;
        let w = rc.right - rc.left;
        let h = rc.bottom - rc.top;
        if w <= 0 || h <= 0 || w > 4096 || h > 4096 {
            return None;
        }

        let screen = GetDC(None); // 可能为空；用于 BitBlt 兜底 + 创建兼容位图
        let mem = CreateCompatibleDC(if screen.0.is_null() { None } else { Some(screen) });
        if mem.0.is_null() {
            let _ = ReleaseDC(None, screen);
            return None;
        }

        // 32bpp top-down DIB：拿到可直读的像素指针，省掉 GetDIBits。
        let mut bi: BITMAPINFO = std::mem::zeroed();
        bi.bmiHeader.biSize = std::mem::size_of::<windows::Win32::Graphics::Gdi::BITMAPINFOHEADER>() as u32;
        bi.bmiHeader.biWidth = w;
        bi.bmiHeader.biHeight = -h; // 负高 = top-down（内存首行即画面顶部）
        bi.bmiHeader.biPlanes = 1;
        bi.bmiHeader.biBitCount = 32;
        bi.bmiHeader.biCompression = BI_RGB.0;
        let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
        let dib = match CreateDIBSection(
            if screen.0.is_null() { None } else { Some(screen) },
            &bi,
            DIB_RGB_COLORS,
            &mut bits,
            None,
            0,
        ) {
            Ok(d) => d,
            Err(_) => {
                let _ = DeleteDC(mem);
                let _ = ReleaseDC(None, screen);
                return None;
            }
        };

        // 把 DIB 选进内存 DC，PrintWindow 才有地方画。
        let old = SelectObject(mem, dib.into());

        let printed = PrintWindow(hwnd, mem, PRINT_WINDOW_FLAGS(PW_RENDERFULLCONTENT)).as_bool();
        let mut frame = read_dib(bits, w, h);

        // 兜底的关键：PrintWindow 可能返回“成功”但内容是黑的（D3D/反外挂拦、最小化时），
        // 不能只看返回值——内容空白同样要退回屏幕 DC 的 BitBlt（仅限普通窗口模式）。
        let blank = frame_is_blank(&frame);
        if (!printed || blank) && !screen.0.is_null() {
            let _ = BitBlt(mem, 0, 0, w, h, Some(screen), rc.left, rc.top, SRCCOPY);
            frame = read_dib(bits, w, h);
        }

        // 清理：还原旧对象 → 释放位图 → 删 DC → 还屏幕 DC。
        if !old.0.is_null() {
            let _ = SelectObject(mem, old);
        }
        let _ = DeleteObject(dib.into());
        let _ = DeleteDC(mem);
        let _ = ReleaseDC(None, screen);
        Some(frame)
    }
}

/// 把 DIB section 的像素按行读成 `BgraFrame`（逐行紧密排列，无行填充）。
fn read_dib(bits: *mut core::ffi::c_void, w: i32, h: i32) -> BgraFrame {
    let total = (w as usize) * (h as usize) * 4;
    let mut pixels = Vec::with_capacity(total);
    let src = unsafe { std::slice::from_raw_parts(bits as *const u8, total) };
    pixels.extend_from_slice(src);
    BgraFrame { w, h, pixels }
}

/// 诊断用：窗口几何与最小化状态（`--live` 打空白时用来解释原因）。
pub struct WinGeo {
    /// 几何(左上角+宽高)：release 只在 debug `--live` 的诊断打印里被读，故允许未读。
    #[allow(dead_code)]
    pub x: i32,
    #[allow(dead_code)]
    pub y: i32,
    #[allow(dead_code)]
    pub w: i32,
    #[allow(dead_code)]
    pub h: i32,
    /// true = 最小化（PrintWindow / BitBlt 都拿不到画面）。
    pub minimized: bool,
}

/// 查询窗口几何与最小化状态；取不到返回 None。
pub fn window_geo(hwnd: HWND) -> Option<WinGeo> {
    unsafe {
        let mut rc: RECT = std::mem::zeroed();
        GetWindowRect(hwnd, &mut rc).ok()?;
        Some(WinGeo {
            x: rc.left,
            y: rc.top,
            w: rc.right - rc.left,
            h: rc.bottom - rc.top,
            minimized: IsIconic(hwnd).as_bool(),
        })
    }
}

/// 粗略“内容方差”：整帧近乎单色 → 判定抓黑/空白（PrintWindow 被拦）。
pub fn frame_is_blank(f: &BgraFrame) -> bool {
    let n = f.pixels.len();
    if n == 0 {
        return true;
    }
    // 等间隔采样若干点算均值和平均偏差。
    let step = (n / 4096).max(1);
    let mut sum: u64 = 0;
    let mut count = 0u32;
    let mut i = 0usize;
    while i < n {
        sum += f.pixels[i] as u64;
        count += 1;
        i += step;
    }
    if count == 0 {
        return true;
    }
    let mean = (sum / count as u64) as i64;
    let mut dev: u64 = 0;
    let mut i = 0usize;
    while i < n {
        let d = f.pixels[i] as i64 - mean;
        dev += (d * d) as u64;
        i += step;
    }
    let avg_dev = dev / count as u64;
    avg_dev < 25 // 全黑/纯色窗口 ≈0
}
