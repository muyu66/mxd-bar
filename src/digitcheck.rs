//! `--digit [img...]`：内嵌 0-9 模板分类器的离线回归（仅 debug 构建编入）。
//!
//! 与实机同一代码路径（`digits::read_exp_value`），对离线帧跑分类器并对照真值。
//! 真值来源：文件名本身就是数字 → 用之；`here_full.bmp`=527819、`mxdbar_tesslive_cap.bmp`=534852、
//! `未知_1920x1080.png`=1212644 是已知抓帧 → 手写映射；`test-resolution/2k_3840x2160_7532.png` 这类 → 末段 `_7532`。
//! 默认跑 test-img/*.png（含 88/0 等边界）+ 两张根目录 BMP + test-resolution/*.png（8 张，排除 *_exp 裁图），
//! 合计 28 张，全 ✓ 才算出厂回归门。用于上线实机前先验证分类器读数。
//!
//! 读图不走任何 OCR：这里只保留 WinRT `StorageFile→BitmapDecoder→SoftwareBitmap`
//! 解码成 Bgra8 裸像素的路径（Windows 自带解码 PNG/BMP，无需额外图片库）。
//! 此文件是整窗截图 + 像素模板分类读数的产物，不含任何读内存/读色块代码。

use std::path::{Path, PathBuf};

use windows::core::{Interface, Result as WinResult};
use windows::Graphics::Imaging::{
    BitmapAlphaMode, BitmapBufferAccessMode, BitmapPixelFormat, SoftwareBitmap,
};
use windows::Storage::{FileAccessMode, StorageFile};
use windows_future::AsyncStatus;

use crate::sensor::capture::BgraFrame;
use crate::sensor::digits::read_exp_value;

// ---------------------------------------------------------------------------
// WinRT 公寓初始化（每线程一次；进入 WinRT 前必须先 init）
// ---------------------------------------------------------------------------

/// 在进入 WinRT 的线程上调用一次（进入即初始化，drop 即反初始化）。
struct Rt;

impl Rt {
    fn init() -> WinResult<Self> {
        unsafe {
            windows::Win32::System::WinRT::RoInitialize(
                windows::Win32::System::WinRT::RO_INIT_MULTITHREADED,
            )?;
        }
        Ok(Rt)
    }
}

impl Drop for Rt {
    fn drop(&mut self) {
        unsafe {
            windows::Win32::System::WinRT::RoUninitialize();
        }
    }
}

// ---------------------------------------------------------------------------
// 异步阻塞小工具（windows_future 的 IAsyncOperation 无 .get()）
// ---------------------------------------------------------------------------

fn wait<T>(op: windows_future::IAsyncOperation<T>) -> WinResult<T>
where
    T: windows::core::RuntimeType + 'static,
{
    let op = &op;
    loop {
        match op.Status()? {
            AsyncStatus::Completed => return op.GetResults(),
            // Error / Canceled：GetResults 会返回对应 HRESULT 错误
            AsyncStatus::Started => std::thread::sleep(std::time::Duration::from_millis(2)),
            _ => return op.GetResults(),
        }
    }
}

// ---------------------------------------------------------------------------
// 离线读图：StorageFile → BitmapDecoder → SoftwareBitmap(Bgra8+Premultiplied)
// ---------------------------------------------------------------------------

/// 解码图片为 Bgra8 的裸像素（宽×高×4，无行填充），供离线分类器回归读帧用。
fn decode_file(path: &Path) -> WinResult<(i32, i32, Vec<u8>)> {
    let hpath = windows::core::HSTRING::from(path.to_string_lossy().as_ref());
    let file = wait(StorageFile::GetFileFromPathAsync(&hpath)?)?;
    let stream = wait(file.OpenAsync(FileAccessMode::Read)?)?;
    let decoder = wait(windows::Graphics::Imaging::BitmapDecoder::CreateAsync(&stream)?)?;
    let sb0 = wait(decoder.GetSoftwareBitmapAsync()?)?;
    // digits 分类器按亮度阈值判"亮字形"，alpha 无影响；统一转 Bgra8+Premultiplied
    let sb = SoftwareBitmap::ConvertWithAlpha(
        &sb0,
        BitmapPixelFormat::Bgra8,
        BitmapAlphaMode::Premultiplied,
    )?;
    raw_from_sb(&sb)
}

/// 把 SoftwareBitmap 的像素按行拷成 Vec<u8>（尊重 plane stride）。
fn raw_from_sb(sb: &SoftwareBitmap) -> WinResult<(i32, i32, Vec<u8>)> {
    let w = sb.PixelWidth()? as i32;
    let h = sb.PixelHeight()? as i32;
    let buf = sb.LockBuffer(BitmapBufferAccessMode::Read)?;
    let plane = buf.GetPlaneDescription(0)?;
    let reference = buf.CreateReference()?;
    let access = reference.cast::<windows::Win32::System::WinRT::IMemoryBufferByteAccess>()?;
    let mut ptr: *mut u8 = std::ptr::null_mut();
    let mut cap: u32 = 0;
    unsafe { access.GetBuffer(&mut ptr, &mut cap)?; }

    let stride = plane.Stride as usize;
    let row = (w as usize) * 4;
    let mut out = Vec::with_capacity((h as usize) * row);
    unsafe {
        let src = std::slice::from_raw_parts(ptr, cap as usize);
        for y in 0..h as usize {
            let off = plane.StartIndex as usize + y * stride;
            out.extend_from_slice(&src[off..off + row]);
        }
    }
    drop(access);
    drop(reference);
    drop(buf);
    Ok((w, h, out))
}

// ---------------------------------------------------------------------------
// 回归主入口
// ---------------------------------------------------------------------------

/// `--digit [img...]`：对离线帧跑内嵌模板分类器，与文件名真值对照。
/// 默认遍历 test-img/*.png（文件名=EXP）+ 根目录 here_full/cap 两张已知帧。
pub fn run() -> eframe::Result {
    let _rt = match Rt::init() {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("WinRT 初始化失败: {e}");
            return Ok(());
        }
    };
    let args: Vec<String> = std::env::args().collect();
    let imgs: Vec<String> = if args.len() > 2 {
        args.iter().skip(2).cloned().collect()
    } else {
        let mut v = vec![];
        // test-img/*.png：文件名本身就是真值（18 张）。
        if let Ok(rd) = std::fs::read_dir("test-img") {
            for e in rd.flatten() {
                let p = e.path();
                let is_digit_png = p.extension().and_then(|s| s.to_str()) == Some("png")
                    && p
                        .file_stem()
                        .and_then(|s| s.to_str())
                        .is_some_and(|s| s.parse::<i64>().is_ok());
                if is_digit_png {
                    v.push(p.to_string_lossy().into_owned());
                }
            }
        }
        v.sort();
        // 根目录两张已知抓帧。
        for extra in ["here_full.bmp", "mxdbar_tesslive_cap.bmp"] {
            if Path::new(extra).exists() {
                v.push(extra.to_string());
            }
        }
        // test-resolution/*.png：排除 `_exp` 裁图（8 张），真值见 truth_of（末段 `_<整数>` 或手写映射）。
        if let Ok(rd) = std::fs::read_dir("test-resolution") {
            let mut rr: Vec<String> = rd
                .flatten()
                .filter(|e| e.path().is_file())
                .map(|e| e.path())
                .filter(|p| {
                    p.extension().and_then(|s| s.to_str()) == Some("png")
                        && p
                            .file_stem()
                            .and_then(|s| s.to_str())
                            .is_some_and(|s| !s.contains("_exp"))
                })
                .map(|p| p.to_string_lossy().into_owned())
                .collect();
            rr.sort();
            v.extend(rr);
        }
        v
    };
    if imgs.is_empty() {
        eprintln!("没有找到测试图（默认 test-img/*.png + here_full/cap）");
        return Ok(());
    }
    let mut ok = 0u32;
    let mut total = 0u32;
    for ip in &imgs {
        let abs = std::path::absolute(Path::new(ip)).unwrap_or_else(|_| PathBuf::from(ip));
        let stem = Path::new(ip)
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| ip.clone());
        // 真值：文件名本身是数字 → 用之；here_full / cap / 未知_1920x1080 是已知抓帧 → 手写映射；
        // test-resolution 这类 `2k_3840x2160_7532` → 末段 `_7532`。
        let truth: Option<i64> = stem
            .parse::<i64>()
            .ok()
            .or_else(|| match stem.as_str() {
                "here_full" => Some(527_819),
                "mxdbar_tesslive_cap" => Some(534_852),
                "未知_1920x1080" => Some(1_212_644),
                _ => stem.rsplit('_').next().and_then(|t| t.parse::<i64>().ok()),
            });
        match decode_file(&abs) {
            Ok((w, h, px)) => {
                let fr = BgraFrame { w, h, pixels: px };
                let exp = read_exp_value(&fr);
                match (exp, truth) {
                    (Some(e), Some(t)) => {
                        total += 1;
                        if e == t {
                            ok += 1;
                        }
                        println!("{stem}: 分类器={e} 真值={t}  {}", if e == t { "✓" } else { "✗" });
                    }
                    (Some(e), None) => println!("{stem}: 分类器={e}（文件名无真值）"),
                    (None, _) => println!("{stem}: (未读到)"),
                }
            }
            Err(e) => eprintln!("读图失败 {ip}: {e}"),
        }
    }
    println!("—— {ok}/{total} 正确 ——");
    Ok(())
}
