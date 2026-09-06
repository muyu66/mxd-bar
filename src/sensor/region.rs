//! 颜色定位区：把「红商城锚 + 淡绿括号」的多分辨率经验区从整窗截图中找出来。
//!
//! 移植自离线原型（已 28/28 经用户逐张核验；原型 Python 脚本已删，参数照抄自本文件/digits.rs
//! 内联注释与 `digitcheck` 离线回归）。**反外挂边界**：这里只产出几何（值条矩形 + 左括号列），
//! **不含任何数值判定**——数值一律由 `digits.rs` 的字形 OCR 去读。颜色仅用于定位。
//!
//! 几何流程（全部照抄 Python 已验参数）：
//!   1. 底部条带 y ≥ 0.80·H 内找红色块（H≤9 或 H≥170），右侧同排近邻绿块（H 30..100）= 红商城；
//!   2. 商城左缘以左、竖带内最近的暗竖直分隔线 = 经验数值条左缘（没有就退回 3×sh 宽）；
//!   3. 条内文本行找「够高的竖淡绿段」最左者 = 左括号 `[` 的左缘列 bl。
//!
//! 条带灰度（Rec601，与 `digits.rs` 一致）一次算出随结果返回，供读数共用，避免二次换算。

use super::capture::BgraFrame;

/// 底部条带起点分数（找锚只在下 20%）。
const B0FRAC: f64 = 0.80;
/// 连通域最小面积。
const MIN_AREA: i32 = 80;
/// 商城/拍卖按钮最小尺寸、宽高比约束。
const BTN_MIN: i32 = 12;
const GREEN_ASPECT_MAX: f64 = 4.0;

/// 定位结果：经验数值条矩形（整窗坐标，右缘=商城左缘，不含商城）+ 条带灰度 + 括号列。
pub struct ExpRegion {
    pub x0: i32,
    pub y0: i32,
    pub x1: i32,
    pub y1: i32,
    /// 条带内（相对 x0）左括号 `[` 的左缘列。
    pub bracket_col: i32,
    /// 条带 Rec601 灰度，行主序，长度 (x1-x0)×(y1-y0)。
    pub g: Vec<u8>,
}

/// 像素 Rec601 亮度（与 digits.rs 同一公式）。
fn lum(b: u8, g: u8, r: u8) -> u8 {
    ((299 * r as i32 + 587 * g as i32 + 114 * b as i32) / 1000) as u8
}

/// BGR → HSV（0..179 / 0..255 / 0..255，照 cv2.COLOR_BGR2HSV 语义）。
fn hsv(b: u8, g: u8, r: u8) -> (i32, i32, i32) {
    let (r, g, b) = (r as f64, g as f64, b as f64);
    let mx = r.max(g).max(b);
    let mn = r.min(g).min(b);
    let v = mx;
    let mut s = 0f64;
    if mx > 0.0 {
        s = 255.0 * (mx - mn) / mx;
    }
    let mut hdeg = 0f64;
    let d = mx - mn;
    if d > 0.0 {
        if mx == r {
            hdeg = 60.0 * (g - b) / d;
        } else if mx == g {
            hdeg = 120.0 + 60.0 * (b - r) / d;
        } else {
            hdeg = 240.0 + 60.0 * (r - g) / d;
        }
        if hdeg < 0.0 {
            hdeg += 360.0;
        }
    }
    let h = ((hdeg * 0.5).round() as i32) % 180;
    (h, s.round() as i32, v as i32)
}

/// 8-连通域（只在 y≥b0 的底部条带内，坐标回原帧）。
struct Comp {
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    area: i32,
}

/// 底部条带内、满足谓词的像素做 8-连通域。pred 收到原帧 BGRA。
fn band_comps(frame: &BgraFrame, b0: i32, pred: impl Fn(u8, u8, u8) -> bool) -> Vec<Comp> {
    let w = frame.w;
    let rows = frame.h - b0;
    let mut out = Vec::new();
    if rows <= 0 || w <= 0 {
        return out;
    }
    let wu = w as usize;
    // 逐像素判定 → 条带掩码（破坏性标记 visited=0）。
    let mut mask = vec![0u8; wu * rows as usize];
    for (i, chunk) in frame.pixels.chunks_exact(4).enumerate() {
        let x = (i % wu) as i32;
        let y = (i / wu) as i32;
        if y < b0 {
            continue;
        }
        let rel = (y - b0) as usize;
        if pred(chunk[0], chunk[1], chunk[2]) {
            mask[rel * wu + x as usize] = 1;
        }
    }
    // 线性 BFS 标记。
    let mut stack: Vec<(i32, i32)> = Vec::with_capacity(4096);
    for yy in 0..rows {
        for xx in 0..w {
            let idx = (yy as usize) * wu + xx as usize;
            if mask[idx] == 0 {
                continue;
            }
            let mut area = 0i32;
            let (mut minx, mut maxx) = (xx, xx);
            let (mut miny, mut maxy) = (yy, yy);
            stack.clear();
            stack.push((xx, yy));
            mask[idx] = 0;
            while let Some((x, y)) = stack.pop() {
                area += 1;
                minx = minx.min(x);
                maxx = maxx.max(x);
                miny = miny.min(y);
                maxy = maxy.max(y);
                for dy in -1..=1 {
                    for dx in -1..=1 {
                        if dx == 0 && dy == 0 {
                            continue;
                        }
                        let (nx, ny) = (x + dx, y + dy);
                        if nx >= 0 && nx < w && ny >= 0 && ny < rows {
                            let ni = (ny as usize) * wu + nx as usize;
                            if mask[ni] == 1 {
                                mask[ni] = 0;
                                stack.push((nx, ny));
                            }
                        }
                    }
                }
            }
            if area >= MIN_AREA {
                out.push(Comp {
                    x: minx,
                    y: b0 + miny,
                    w: maxx - minx + 1,
                    h: maxy - miny + 1,
                    area,
                });
            }
        }
    }
    out
}

/// 底部条带内找红商城：红块 + 右侧同排绿块（拍卖按钮佐证）。返回 (x,y,w,h) 或 None。
fn find_shop(frame: &BgraFrame) -> Option<(i32, i32, i32, i32)> {
    let h = frame.h;
    let b0 = (B0FRAC * h as f64) as i32;
    // 先做一次整带 HSV 判定，同时收红/绿两个掩码（避免每像素算两遍）。
    let reds = band_comps(frame, b0, |b, g, r| {
        let (hx, s, v) = hsv(b, g, r);
        (hx <= 9 || hx >= 170) && s > 110 && v > 70
    });
    let greens = band_comps(frame, b0, |b, g, r| {
        let (hx, s, v) = hsv(b, g, r);
        (30..=100).contains(&hx) && s > 90 && v > 70
    });
    let rc: Vec<&Comp> = reds
        .iter()
        .filter(|c| {
            c.w >= BTN_MIN
                && c.h >= BTN_MIN
                && (1.0..=3.5).contains(&(c.w as f64 / c.h as f64))
        })
        .collect();
    let gc: Vec<&Comp> = greens
        .iter()
        .filter(|c| c.w >= BTN_MIN && c.h >= BTN_MIN && c.w as f64 / c.h as f64 <= GREEN_ASPECT_MAX)
        .collect();
    let mut best: Option<&Comp> = None;
    for red in &rc {
        for green in &gc {
            let same_row = (green.y - red.y).abs() <= 10
                && ((green.y + green.h) - (red.y + red.h)).abs() <= 12;
            let in_x = green.x >= red.x + red.w - 6 && green.x <= red.x + red.w + 26;
            if same_row && in_x && (best.is_none() || red.area > best.unwrap().area) {
                best = Some(red);
            }
        }
    }
    best.map(|c| (c.x, c.y, c.w, c.h))
}

/// shop 左侧第一个数值条 → (x0, y0, x1, y1)。
fn value_bar(frame: &BgraFrame, shop: (i32, i32, i32, i32)) -> Option<(i32, i32, i32, i32)> {
    let (sx, sy, sw, sh, _a) = (shop.0, shop.1, shop.2, shop.3, 0);
    let w = frame.w;
    let h = frame.h;
    let x1 = w.min(sx);
    let y0 = 0.max(sy);
    let y1 = h.min(sy + sh);
    if y1 <= y0 {
        return None;
    }
    // 竖带内 luma<90 覆盖率 ≥0.7 的暗竖列 = 分隔线候选。
    let mut cov = vec![0i32; w as usize];
    for y in y0..y1 {
        let row_base = (y as usize) * (w as usize) * 4;
        for x in 0..w {
            let o = row_base + x as usize * 4;
            let (b, g, r) = (frame.pixels[o], frame.pixels[o + 1], frame.pixels[o + 2]);
            if lum(b, g, r) < 90 {
                cov[x as usize] += 1;
            }
        }
    }
    let thr = (7 * (y1 - y0)) / 10; // int(0.7×(y1-y0))
    let cutoff = x1 - ((6 * sw) / 10); // x1 - ⌊0.6·sw⌋（排除商城自身描边）
    let mut seps: Vec<i32> = Vec::new();
    for (x, &c) in cov.iter().enumerate() {
        let x = x as i32;
        if c >= thr && x < cutoff {
            seps.push(x);
        }
    }
    let x0 = if let Some(&m) = seps.iter().max() {
        m + 1
    } else {
        0.max(x1 - 3 * sw)
    };
    if x1 - x0 < 8 || y1 <= y0 {
        return None;
    }
    Some((x0, y0, x1, y1))
}

/// 条内找左括号 `[` 的左缘列（条带相对）。找不到 → None。
fn bracket_left(frame: &BgraFrame, g: &[u8], x0: i32, y0: i32, bw: i32, bh: i32) -> Option<i32> {
    if bw <= 0 || bh <= 0 {
        return None;
    }
    let w = frame.w;
    // 文本行：行内 luma≥150 计数 ∈ [3, 0.5·条宽]。
    let mut text = vec![false; bh as usize];
    let mut nt = 0i32;
    for y in 0..bh {
        let row_off = (y as usize) * (bw as usize);
        let mut cnt = 0i32;
        for x in 0..bw {
            if g[row_off + x as usize] >= 150 {
                cnt += 1;
            }
        }
        if (3..=(bw / 2)).contains(&cnt) {
            text[y as usize] = true;
            nt += 1;
        }
    }
    let tall = if nt > 0 { 4.max((35 * nt) / 100) } else { 4 };
    // 文本行内淡绿像素按列计数。
    let mut coln = vec![0i32; bw as usize];
    for y in 0..bh {
        if !text[y as usize] {
            continue;
        }
        let yy = y0 + y;
        let row_base = (yy as usize) * (w as usize) * 4;
        for x in 0..bw {
            let o = row_base + (x0 + x) as usize * 4;
            let (b, gx, r) = (frame.pixels[o], frame.pixels[o + 1], frame.pixels[o + 2]);
            let (b, gx, r) = (b as i32, gx as i32, r as i32);
            if gx >= 140 && gx > r + 12 && gx > b + 22 && b >= 45 {
                coln[x as usize] += 1;
            }
        }
    }
    // 绿列连续段（段宽 ≤14 且最高列 ≥ tall）→ 最左段起点。
    let mut on = false;
    let mut st = 0;
    for cx in 0..bw {
        if coln[cx as usize] >= 1 && !on {
            st = cx;
            on = true;
        } else if coln[cx as usize] < 1 && on {
            if coln[st as usize..cx as usize].iter().max().copied().unwrap_or(0) >= tall
                && cx - st <= 14
            {
                return Some(st);
            }
            on = false;
        }
    }
    if on {
        if coln[st as usize..bw as usize].iter().max().copied().unwrap_or(0) >= tall
            && bw - st <= 14
        {
            return Some(st);
        }
    }
    None
}

/// 整窗截图 → 经验数值条区域 + 条带灰度 + 左括号列。找不到（无锚/退化）→ None。
pub fn locate(frame: &BgraFrame) -> Option<ExpRegion> {
    // 极小帧（非游戏整窗）直接放弃，避免误读/空转。
    if frame.w < 400 || frame.h < 300 {
        return None;
    }
    let shop = find_shop(frame)?;
    let (x0, y0, x1, y1) = value_bar(frame, shop)?;
    let bw = x1 - x0;
    let bh = y1 - y0;
    // 条带 Rec601 灰度（读数共用）。
    let mut g = vec![0u8; (bw * bh) as usize];
    let w = frame.w;
    for y in 0..bh {
        let yy = y0 + y;
        let row_base = (yy as usize) * (w as usize) * 4;
        let out_off = (y as usize) * (bw as usize);
        for x in 0..bw {
            let o = row_base + (x0 + x) as usize * 4;
            let (b, gx, r) = (frame.pixels[o], frame.pixels[o + 1], frame.pixels[o + 2]);
            g[out_off + x as usize] = lum(b, gx, r);
        }
    }
    let bracket_col = bracket_left(frame, &g, x0, y0, bw, bh)?;
    Some(ExpRegion {
        x0,
        y0,
        x1,
        y1,
        bracket_col,
        g,
    })
}
