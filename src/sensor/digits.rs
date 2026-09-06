//! 生产 EXP 读取：内嵌 0-9 字形模板分类器。**不依赖 tesseract / 外部引擎 / WinRT。**
//!
//! 为什么换掉 tesseract：游戏 EXP 数字是「亮环字形」，每个 0-9 在该渲染下是**单一固定位图**
//! （每个数字在 18 张打怪真值帧里都只有 1 种字形，见下方模板）。tesseract 内部自适应二值化
//! 会随整行上下文漂移 —— 同一个像素级完美的「8」，长行读成 8、短行读成 3（88.png 被读成 33、
//! here_full.bmp 527819 被读成 527315）。数字一共只有 10 种字形，不如直接逐格取固定 6px 宽的
//! 84-bit 二值 mask，和这 10 个模板按 Hamming 距离硬匹配。离线 20/20 全对。
//!
//! 几何（按 1382×807 整窗标定，仅整窗截图 + 像素分析，反外挂暴露面不变）：
//! - 值区数字每个占 6px 宽、pitch=6，字形高 ~7px（y≈769..776）；
//! - 模板行序对应 y=765..779（14 行），行索引 4..10 才是字形主体，其余是空白边距；
//! - 首格 x0 实机有 ±1 漂移（here_full/cap 是 756，测试 PNG 是 757），故运行时在 751..=762
//!   搜索取「实心格最多、总失配最小」的对齐；
//! - 窗口分辨率/布局变了要整段重新标定（同 tess.rs 的 BAND）。

use super::capture::BgraFrame;

/// 值区顶端 y：模板 14 行覆盖 y=765..779。
const Y_TOP: i32 = 765;
const ROWS: i32 = 14;
const CELL_W: usize = 6;
/// 逐像素亮度阈值：lum>=150 记 1(ink)。字形亮环 lum≈250、背景≈50..80，对比极高。
const LUM_THR: i32 = 150;
/// x0 搜索范围（吸收首列 ±1px 漂移）。
const X0_LO: i32 = 751;
const X0_HI: i32 = 762;
/// 单个数字格位数 = 14×6 = 84。
const BITS: usize = (ROWS as usize) * CELL_W;
/// 空格（整格无 ink）→ 数字串到此结束。
const BLANK_INK: u32 = 0;
/// 「实心格」容忍：与某模板失配 ≤10%（84 的 10%≈8 bit）才计为该数字稳定匹配。
const SOLID_MAX_MIS: u32 = 8;
/// 单格与最佳模板失配 >18%（≈15 bit）→ 该格不是 0-9（括号/百分/小数点等）→ 停止读数。
const STOP_MIS: u32 = 15;

// —— 0-9 字形模板：[[行 0..13], [列 0..5]]，1=ink ——
// 从 test-img/*.png（文件名=EXP 真值）逐位众数重建，0-9 每字唯一（18 帧无歧义）。

/// 0
const TPL_0: [[u8; 6]; 14] = [
    [0, 0, 0, 0, 0, 0], [0, 0, 0, 0, 0, 0], [0, 0, 0, 0, 0, 0], [0, 0, 0, 0, 0, 0],
    [0, 1, 1, 1, 0, 0], [1, 0, 0, 0, 1, 0], [1, 0, 0, 0, 1, 0], [1, 0, 0, 0, 1, 0],
    [1, 0, 0, 0, 1, 0], [1, 0, 0, 0, 1, 0], [0, 1, 1, 1, 0, 0], [0, 0, 0, 0, 0, 0],
    [0, 0, 0, 0, 0, 0], [0, 0, 0, 0, 0, 0],
];
/// 1
const TPL_1: [[u8; 6]; 14] = [
    [0, 0, 0, 0, 0, 0], [0, 0, 0, 0, 0, 0], [0, 0, 0, 0, 0, 0], [0, 0, 0, 0, 0, 0],
    [0, 0, 1, 0, 0, 0], [0, 1, 1, 0, 0, 0], [0, 0, 1, 0, 0, 0], [0, 0, 1, 0, 0, 0],
    [0, 0, 1, 0, 0, 0], [0, 0, 1, 0, 0, 0], [0, 0, 1, 0, 0, 0], [0, 0, 0, 0, 0, 0],
    [0, 0, 0, 0, 0, 0], [0, 0, 0, 0, 0, 0],
];
/// 2
const TPL_2: [[u8; 6]; 14] = [
    [0, 0, 0, 0, 0, 0], [0, 0, 0, 0, 0, 0], [0, 0, 0, 0, 0, 0], [0, 0, 0, 0, 0, 0],
    [0, 1, 1, 1, 0, 0], [1, 0, 0, 0, 1, 0], [0, 0, 0, 0, 1, 0], [0, 0, 0, 1, 0, 0],
    [0, 0, 1, 0, 0, 0], [0, 1, 0, 0, 0, 0], [1, 1, 1, 1, 1, 0], [0, 0, 0, 0, 0, 0],
    [0, 0, 0, 0, 0, 0], [0, 0, 0, 0, 0, 0],
];
/// 3
const TPL_3: [[u8; 6]; 14] = [
    [0, 0, 0, 0, 0, 0], [0, 0, 0, 0, 0, 0], [0, 0, 0, 0, 0, 0], [0, 0, 0, 0, 0, 0],
    [0, 1, 1, 1, 0, 0], [1, 0, 0, 0, 1, 0], [0, 0, 0, 0, 1, 0], [0, 0, 1, 1, 0, 0],
    [0, 0, 0, 0, 1, 0], [1, 0, 0, 0, 1, 0], [0, 1, 1, 1, 0, 0], [0, 0, 0, 0, 0, 0],
    [0, 0, 0, 0, 0, 0], [0, 0, 0, 0, 0, 0],
];
/// 4
const TPL_4: [[u8; 6]; 14] = [
    [0, 0, 0, 0, 0, 0], [0, 0, 0, 0, 0, 0], [0, 0, 0, 0, 0, 0], [0, 0, 0, 0, 0, 0],
    [0, 0, 0, 1, 0, 0], [0, 0, 1, 1, 0, 0], [0, 1, 0, 1, 0, 0], [1, 0, 0, 1, 0, 0],
    [1, 1, 1, 1, 1, 0], [0, 0, 0, 1, 0, 0], [0, 0, 0, 1, 0, 0], [0, 0, 0, 0, 0, 0],
    [0, 0, 0, 0, 0, 0], [0, 0, 0, 0, 0, 0],
];
/// 5
const TPL_5: [[u8; 6]; 14] = [
    [0, 0, 0, 0, 0, 0], [0, 0, 0, 0, 0, 0], [0, 0, 0, 0, 0, 0], [0, 0, 0, 0, 0, 0],
    [1, 1, 1, 1, 1, 0], [1, 0, 0, 0, 0, 0], [1, 0, 0, 0, 0, 0], [0, 1, 1, 1, 0, 0],
    [0, 0, 0, 0, 1, 0], [1, 0, 0, 0, 1, 0], [0, 1, 1, 1, 0, 0], [0, 0, 0, 0, 0, 0],
    [0, 0, 0, 0, 0, 0], [0, 0, 0, 0, 0, 0],
];
/// 6
const TPL_6: [[u8; 6]; 14] = [
    [0, 0, 0, 0, 0, 0], [0, 0, 0, 0, 0, 0], [0, 0, 0, 0, 0, 0], [0, 0, 0, 0, 0, 0],
    [0, 1, 1, 1, 0, 0], [1, 0, 0, 0, 1, 0], [1, 0, 0, 0, 0, 0], [1, 1, 1, 1, 0, 0],
    [1, 0, 0, 0, 1, 0], [1, 0, 0, 0, 1, 0], [0, 1, 1, 1, 0, 0], [0, 0, 0, 0, 0, 0],
    [0, 0, 0, 0, 0, 0], [0, 0, 0, 0, 0, 0],
];
/// 7
const TPL_7: [[u8; 6]; 14] = [
    [0, 0, 0, 0, 0, 0], [0, 0, 0, 0, 0, 0], [0, 0, 0, 0, 0, 0], [0, 0, 0, 0, 0, 0],
    [1, 1, 1, 1, 1, 0], [0, 0, 0, 0, 1, 0], [0, 0, 0, 0, 1, 0], [0, 0, 0, 0, 1, 0],
    [0, 0, 0, 1, 0, 0], [0, 0, 1, 0, 0, 0], [0, 0, 1, 0, 0, 0], [0, 0, 0, 0, 0, 0],
    [0, 0, 0, 0, 0, 0], [0, 0, 0, 0, 0, 0],
];
/// 8
const TPL_8: [[u8; 6]; 14] = [
    [0, 0, 0, 0, 0, 0], [0, 0, 0, 0, 0, 0], [0, 0, 0, 0, 0, 0], [0, 0, 0, 0, 0, 0],
    [0, 1, 1, 1, 0, 0], [1, 0, 0, 0, 1, 0], [1, 0, 0, 0, 1, 0], [0, 1, 1, 1, 0, 0],
    [1, 0, 0, 0, 1, 0], [1, 0, 0, 0, 1, 0], [0, 1, 1, 1, 0, 0], [0, 0, 0, 0, 0, 0],
    [0, 0, 0, 0, 0, 0], [0, 0, 0, 0, 0, 0],
];
/// 9
const TPL_9: [[u8; 6]; 14] = [
    [0, 0, 0, 0, 0, 0], [0, 0, 0, 0, 0, 0], [0, 0, 0, 0, 0, 0], [0, 0, 0, 0, 0, 0],
    [0, 1, 1, 1, 0, 0], [1, 0, 0, 0, 1, 0], [1, 0, 0, 0, 1, 0], [0, 1, 1, 1, 1, 0],
    [0, 0, 0, 0, 1, 0], [1, 0, 0, 0, 1, 0], [0, 1, 1, 1, 0, 0], [0, 0, 0, 0, 0, 0],
    [0, 0, 0, 0, 0, 0], [0, 0, 0, 0, 0, 0],
];

/// 10 个模板按数字下标排列，TPL[d] 即数字 d 的字形。
const TPL: [[[u8; 6]; 14]; 10] = [
    TPL_0, TPL_1, TPL_2, TPL_3, TPL_4, TPL_5, TPL_6, TPL_7, TPL_8, TPL_9,
];

/// 取帧内 (x0, Y_TOP) 起、14 行 × 6 列的 ink mask（行序 y 增、列序 x 增）。
fn sample_mask(frame: &BgraFrame, x0: i32) -> [u8; BITS] {
    let mut m = [0u8; BITS];
    let w = frame.w as usize;
    let px = &frame.pixels;
    for r in 0..ROWS {
        let y = (Y_TOP + r) as usize;
        let row_base = (y * w + x0 as usize) * 4;
        for c in 0..CELL_W {
            let o = row_base + c * 4;
            // top-down BGRA：像素字节序 B,G,R,A。
            let (b, g, rr) = (px[o], px[o + 1], px[o + 2]);
            let lum = (299 * rr as i32 + 587 * g as i32 + 114 * b as i32) / 1000;
            m[(r as usize) * CELL_W + c] = (lum >= LUM_THR) as u8;
        }
    }
    m
}

/// mask 与某模板的 Hamming 距离（逐位异或求和）。
fn dist(m: &[u8; BITS], tpl: &[[u8; 6]; 14]) -> u32 {
    let mut d = 0u32;
    for (i, b) in m.iter().enumerate() {
        d += (b ^ tpl[i / CELL_W][i % CELL_W]) as u32;
    }
    d
}

/// 从 x0 起从左往右按 6px 格解码，返回 (各格数字, 总失配, 实心格数)。
/// 遇空格（整格无 ink）或非数字格（与最佳模板失配 >18%）即停。
fn decode_at(frame: &BgraFrame, x0: i32) -> (Vec<u8>, u32, u32) {
    let mut digits: Vec<u8> = Vec::with_capacity(8);
    let mut cost = 0u32;
    let mut solids = 0u32;
    // 值区最多 12 位，够 9,999,999 级以内的 EXP；帧宽不够就自然截断。
    for i in 0..12 {
        let x = x0 + (i as i32) * CELL_W as i32;
        let m = sample_mask(frame, x);
        let ink: u32 = m.iter().map(|&b| b as u32).sum();
        if ink == BLANK_INK {
            break;
        }
        let (mis, digit) = (0..10).map(|d| (dist(&m, &TPL[d]), d as u8)).min().unwrap();
        if mis > STOP_MIS {
            break; // 不是 0-9（括号/百分/点等）
        }
        if mis <= SOLID_MAX_MIS {
            solids += 1;
        }
        digits.push(digit);
        cost += mis;
    }
    (digits, cost, solids)
}

/// 读当前级累积 EXP 的全量整数（只对 1382×807 原生整窗几何生效）。读不到返回 None。
/// 多分辨率改由 `read_exp_value` 先走 `region::locate` 锚定位，此函数作回退。
fn read_exp_value_native(frame: &BgraFrame) -> Option<i64> {
    // 值区按 1382×807 标定；至少留到 X0_HI+12 格、Y_TOP+14 行，否则几何不适用。
    if frame.w < X0_HI + 6 * 12 || frame.h < Y_TOP + ROWS {
        return None;
    }
    let mut best: Option<(u32, u32, i64)> = None; // (实心格数, 总失配, 数值)
    for x0 in X0_LO..=X0_HI {
        let (digits, cost, solids) = decode_at(frame, x0);
        if digits.is_empty() {
            continue;
        }
        let mut val: i64 = 0;
        for d in &digits {
            val = val * 10 + *d as i64;
        }
        let better = match best {
            None => true,
            Some((bs, bc, _)) => solids > bs || (solids == bs && cost < bc),
        };
        if better {
            best = Some((solids, cost, val));
        }
    }
    best.map(|(_, _, v)| v)
}

// ---------------------------------------------------------------------------
// 多分辨率双路读 EXP：region 颜色定位 → 条内淡绿 `[` 左侧比例字 OCR。
// ---------------------------------------------------------------------------

/// 读经验值的统一入口：
/// 1. `region::locate` 颜色定位出经验数值条 + 左括号列 → 条内读 bl 左边数字（多分辨率，含原生帧）；
/// 2. 锚定位读不出（找不到红商城/无括号）→ 回退原生 1382×807 几何读数。
/// 颜色只用来定位；数值一律走下方字形 OCR。
pub fn read_exp_value(frame: &BgraFrame) -> Option<i64> {
    if let Some(reg) = super::region::locate(frame) {
        let bw = reg.x1 - reg.x0;
        let bh = reg.y1 - reg.y0;
        if let Some(v) = read_band_left_of_bracket(&reg.g, bw, bh, reg.bracket_col) {
            return Some(v);
        }
    }
    read_exp_value_native(frame)
}

/// 单格 OCR 最低失配上限（fraction，同 Python MIS_MAX=0.20）。
const MIS_MAX: f32 = 0.20;

/// 单个候选字：低失配数字 + 失配率 + 右缘列 + 字高（分组行内 ink 行数）。
struct Cell {
    d: Option<u8>,
    mis: f32,
    xb: i32,
    hgt: i32,
}

/// 模板数字 d 的 ink 外接框：(顶行, 左列, 行数, 列数)。
fn tpl_ink(d: usize) -> (i32, i32, i32, i32) {
    let mut top = ROWS as i32;
    let mut bot = -1;
    let mut left = CELL_W as i32;
    let mut right = -1;
    for r in 0..ROWS as usize {
        for c in 0..CELL_W {
            if TPL[d][r][c] == 1 {
                top = top.min(r as i32);
                bot = bot.max(r as i32);
                left = left.min(c as i32);
                right = right.max(c as i32);
            }
        }
    }
    (top, left, bot - top + 1, right - left + 1)
}

/// 闭区间重叠长度（a≤b、c≤d），负则取 0。
fn overlap(a: f64, b: f64, c: f64, d: f64) -> f64 {
    (b.min(d) - a.max(c)).max(0.0)
}

/// 把 sh×sw 的 0/1 字形按面积平均重采样到 th×tw（逐像素精确覆盖，不加图片库）。
/// 返回 th×tw 的 0..1 覆盖率。
fn resample_area(src: &[u8], sh: i32, sw: i32, th: i32, tw: i32) -> Vec<f64> {
    let mut out = vec![0f64; (th * tw) as usize];
    if sh <= 0 || sw <= 0 || th <= 0 || tw <= 0 {
        return out;
    }
    for tr in 0..th {
        let ya = tr as f64 * sh as f64 / th as f64;
        let yb = (tr + 1) as f64 * sh as f64 / th as f64;
        for tc in 0..tw {
            let xa = tc as f64 * sw as f64 / tw as f64;
            let xb = (tc + 1) as f64 * sw as f64 / tw as f64;
            let mut sum = 0f64;
            for i in (ya.floor() as i32)..(yb.ceil() as i32) {
                if i < 0 || i >= sh {
                    continue;
                }
                let wy = overlap(ya, yb, i as f64, (i + 1) as f64);
                if wy <= 0.0 {
                    continue;
                }
                let base = (i * sw) as usize;
                for j in (xa.floor() as i32)..(xb.ceil() as i32) {
                    if j < 0 || j >= sw {
                        continue;
                    }
                    let wx = overlap(xa, xb, j as f64, (j + 1) as f64);
                    if wx <= 0.0 {
                        continue;
                    }
                    sum += src[base + j as usize] as f64 * wy * wx;
                }
            }
            let area = (yb - ya) * (xb - xa);
            out[(tr * tw + tc) as usize] = if area > 0.0 { sum / area } else { 0.0 };
        }
    }
    out
}

/// 对已裁 ink 外接框的 0/1 字形做比例 OCR：与 10 个模板各自动框分别面积重采样，
/// 二值化(>0.5)后算失配率，取最小 → (digit, 失配率)。
fn ocr_cell(src: &[u8], sh: i32, sw: i32) -> Option<(u8, f32)> {
    let mut best: Option<(u8, f32)> = None;
    for d in 0..10 {
        let (trow, tcol, th, tw) = tpl_ink(d);
        let mut tplv = vec![0u8; (th * tw) as usize];
        for r in 0..th as usize {
            for c in 0..tw as usize {
                tplv[r * tw as usize + c] = TPL[d][trow as usize + r][tcol as usize + c];
            }
        }
        let avg = resample_area(src, sh, sw, th, tw);
        let mut mis = 0f64;
        let total = (th * tw) as f64;
        for k in 0..(th * tw) as usize {
            let bin = if avg[k] > 0.5 { 1.0 } else { 0.0 };
            mis += (bin - tplv[k] as f64).abs();
        }
        let mf = mis / total;
        if best.is_none() || mf < best.unwrap().1 as f64 {
            best = Some((d as u8, mf as f32));
        }
    }
    best
}

/// 众数（平局取先出现者，同 Python Counter.most_common(1) 的稳定序）。
fn mode(xs: &[i32]) -> i32 {
    let mut counts: Vec<(i32, i32)> = Vec::new();
    for &v in xs {
        if let Some(e) = counts.iter_mut().find(|(k, _)| *k == v) {
            e.1 += 1;
        } else {
            counts.push((v, 1));
        }
    }
    let mut best = counts[0];
    for c in &counts {
        if c.1 > best.1 {
            best = *c;
        }
    }
    best.0
}

/// 字典序比较候选键 (thr, -len, -右缘, avg)：更小者胜。
fn key_better(a: &(i32, i32, i32, f32), b: &(i32, i32, i32, f32)) -> bool {
    if a.0 != b.0 {
        return a.0 < b.0;
    }
    if a.1 != b.1 {
        return a.1 < b.1;
    }
    if a.2 != b.2 {
        return a.2 < b.2;
    }
    a.3 < b.3
}

/// 在条带灰度 `g`（宽 bw×高 bh，行主序）里，取左括号 `[`（列 bl）左侧的 EXP 值数字串。
/// 亮/暗 × 多阈值，逐字 ink-bbox 归一 OCR，取「低失配、同高、尽量长、尽量贴括号」的字段。
fn read_band_left_of_bracket(g: &[u8], bw: i32, bh: i32, bl: i32) -> Option<i64> {
    if bl < 3 {
        return None;
    }
    let bwu = bw as usize;
    let bhu = bh as usize;
    let blu = bl as usize;
    let mut best: Option<((i32, i32, i32, f32), i64)> = None;
    for pol in 0..2 {
        for &thr in &[120, 150, 190] {
            // 左条带 white 掩码：亮极性 = luma≥thr，暗极性 = luma≤255-thr。
            let mut white = vec![false; bhu * blu];
            for y in 0..bh {
                let base = (y as usize) * bwu;
                let yo = (y as usize) * blu;
                for x in 0..bl {
                    let v = g[base + x as usize] as i32;
                    white[yo + x as usize] = if pol == 0 { v >= thr } else { v <= 255 - thr };
                }
            }
            // 文本行：行 ink 数 ∈ [3, 0.6·bl]。
            let hi = (6 * blu) / 10;
            let mut rows: Vec<i32> = Vec::new();
            for y in 0..bh {
                let mut c = 0i32;
                let yo = (y as usize) * blu;
                for &w in &white[yo..yo + blu] {
                    if w {
                        c += 1;
                    }
                }
                if c >= 3 && (c as usize) <= hi {
                    rows.push(y);
                }
            }
            if rows.is_empty() {
                continue;
            }
            // 相邻文本行 gap>2 分段。
            let mut gs = 0usize;
            for gi in 0..rows.len() {
                let is_end = gi + 1 == rows.len() || rows[gi + 1] - rows[gi] > 2;
                if !is_end {
                    continue;
                }
                let r0 = rows[gs];
                let r1 = rows[gi];
                // 列向空白裂字（跨整组行的全空列 = 字符边界）。
                let mut spans: Vec<(i32, i32)> = Vec::new();
                let mut cur: Option<i32> = None;
                for x in 0..bl {
                    let mut any = false;
                    for y in r0..=r1 {
                        if white[(y as usize) * blu + x as usize] {
                            any = true;
                            break;
                        }
                    }
                    if any {
                        if cur.is_none() {
                            cur = Some(x);
                        }
                    } else if let Some(s) = cur.take() {
                        spans.push((s, x - 1));
                    }
                }
                if let Some(s) = cur.take() {
                    spans.push((s, bl - 1));
                }
                let mut cells: Vec<Cell> = Vec::new();
                for (xa, xb) in spans {
                    if xb - xa < 1 {
                        continue; // 字宽需 ≥2（同 Python）
                    }
                    let gh = (r1 - r0 + 1) as usize;
                    let wc = (xb - xa + 1) as usize;
                    let mut anyrow = vec![false; gh];
                    let mut anycol = vec![false; wc];
                    for (ii, y) in (r0..=r1).enumerate() {
                        let yo = (y as usize) * blu + xa as usize;
                        for jj in 0..wc {
                            if white[yo + jj] {
                                anyrow[ii] = true;
                                anycol[jj] = true;
                            }
                        }
                    }
                    if !anyrow.iter().any(|&b| b) {
                        continue;
                    }
                    let top = anyrow.iter().position(|&b| b).unwrap();
                    let bot = anyrow.iter().rposition(|&b| b).unwrap();
                    let hgt = (bot - top + 1) as i32;
                    if hgt < 3 {
                        continue;
                    }
                    let left = anycol.iter().position(|&b| b).unwrap();
                    let right = anycol.iter().rposition(|&b| b).unwrap();
                    let sw = (right - left + 1) as i32;
                    let sh = (bot - top + 1) as i32;
                    // 裁 ink 外接框的 0/1 字形 → OCR。
                    let mut src = vec![0u8; (sh as usize) * (sw as usize)];
                    for ii in top..=bot {
                        let y = r0 + ii as i32;
                        let yo = (y as usize) * blu + (xa as usize + left);
                        for jj in left..=right {
                            if white[yo + (jj - left)] {
                                src[(ii - top) * (sw as usize) + (jj - left)] = 1;
                            }
                        }
                    }
                    let rr = ocr_cell(&src, sh, sw);
                    cells.push(Cell {
                        d: rr.map(|t| t.0),
                        mis: rr.map_or(1.0, |t| t.1),
                        xb,
                        hgt,
                    });
                }
                // 低失配字的众数字高 → 同高过滤（排除 EXP 标签/实心底条）。
                let low_h: Vec<i32> = cells
                    .iter()
                    .filter(|c| c.d.is_some() && c.mis <= MIS_MAX)
                    .map(|c| c.hgt)
                    .collect();
                if low_h.is_empty() {
                    gs = gi + 1;
                    continue;
                }
                let hmed = mode(&low_h);
                let tol = 0.2 * hmed as f32;
                // 连续满足 ok 的字 = 值字段（允许单字，如 0）。
                let mut segs: Vec<Vec<&Cell>> = Vec::new();
                let mut seg: Vec<&Cell> = Vec::new();
                for c in cells.iter() {
                    let ok = c.d.is_some()
                        && c.mis <= MIS_MAX
                        && ((c.hgt - hmed) as f32).abs() <= tol;
                    if ok {
                        seg.push(c);
                    } else if !seg.is_empty() {
                        segs.push(std::mem::replace(&mut seg, Vec::new()));
                    }
                }
                if !seg.is_empty() {
                    segs.push(seg);
                }
                for s in segs {
                    if s.is_empty() {
                        continue;
                    }
                    let ds: String = s.iter().map(|c| (b'0' + c.d.unwrap()) as char).collect();
                    let len = ds.len() as i32;
                    let right = s.last().unwrap().xb;
                    let avg: f32 = s.iter().map(|c| c.mis).sum::<f32>() / s.len() as f32;
                    if let Some(val) = ds.parse::<i64>().ok() {
                        let key = (thr, -len, -right, avg);
                        let better = match &best {
                            None => true,
                            Some((bk, _)) => key_better(&key, bk),
                        };
                        if better {
                            best = Some((key, val));
                        }
                    }
                }
                gs = gi + 1;
            }
        }
    }
    best.map(|(_, v)| v)
}

// ---------------------------------------------------------------------------
// 自测：合成"截图"帧验证读 EXP
// ---------------------------------------------------------------------------

/// 测试专用：把一段十进制数字串按本模块标定的值区几何画成一帧 BGRA"截图"
/// （数字从 `gx0` 起、每个 6px 一格，字形 = 内嵌模板 TPL，ink 亮度≈240、背景≈40）。
/// 等价于把真实屏幕上这段 EXP 数字原样画进像素缓冲，供 `read_exp_value` 离线回归。
/// 只在 test 构建编入（sampler 的端到端测试也复用它）。
#[cfg(test)]
pub(crate) fn synth_value_frame(value: &str, gx0: i32, w: i32, h: i32) -> BgraFrame {
    debug_assert!(value.bytes().all(|b| b.is_ascii_digit()), "只支持 0-9");
    let wu = w as usize;
    let mut px = vec![40u8; wu * h as usize * 4]; // BGRA，背景 lum≈40（<LUM_THR）
    for p in px.chunks_exact_mut(4) {
        p[3] = 255;
    }
    for (i, ch) in value.bytes().enumerate() {
        let d = (ch - b'0') as usize;
        let gx = gx0 + (i as i32) * CELL_W as i32;
        for (r, row) in TPL[d].iter().enumerate() {
            let y = Y_TOP + r as i32;
            for (c, &ink) in row.iter().enumerate() {
                if ink == 1 {
                    let x = gx + c as i32;
                    let o = (y as usize * wu + x as usize) * 4;
                    px[o] = 240;
                    px[o + 1] = 240;
                    px[o + 2] = 240; // lum≈240（≥LUM_THR）
                    px[o + 3] = 255;
                }
            }
        }
    }
    BgraFrame { w, h, pixels: px }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 画一整帧纯背景（无任何值区 ink），用于几何守卫测试。
    fn blank(w: i32, h: i32) -> BgraFrame {
        let mut px = vec![40u8; w as usize * h as usize * 4];
        for p in px.chunks_exact_mut(4) {
            p[3] = 255;
        }
        BgraFrame { w, h, pixels: px }
    }

    #[test]
    fn reads_synthesized_value_region() {
        // 各种位长/含 0 的实际 EXP 值；数字落在 x0 搜索窗内的不同位置。
        let cases = [
            ("527819", 756),
            ("1200", 758),
            ("38", 752),
            ("7", 761),
            ("700000", 757),
            ("9999999", 758),
            ("0", 758),
            ("1", 751),
        ];
        for (s, gx) in cases {
            let want = s.parse::<i64>().unwrap();
            let got = read_exp_value(&synth_value_frame(s, gx, 900, 800));
            assert_eq!(got, Some(want), "把 {s} 画在 gx0={gx} 应读回 {want}");
        }
    }

    #[test]
    fn absorbs_anchor_drift_within_search_window() {
        // 值区首格 ±1px 漂移（实机 756/757 都见过）：只要锚点在 751..=762 内都应读对。
        for gx in 751..=762 {
            let got = read_exp_value(&synth_value_frame("527819", gx, 900, 800));
            assert_eq!(got, Some(527_819), "数字首格漂移到 gx0={gx} 仍应读回 527819");
        }
    }

    #[test]
    fn geometry_guard_returns_none() {
        // 帧太小（几何不符）→ None：分别卡在宽 / 高守卫上。
        assert_eq!(read_exp_value(&blank(830, 800)), None, "宽不足 834 → None");
        assert_eq!(read_exp_value(&blank(900, 770)), None, "高不足 779 → None");
        // 大小够但没有数字 → None。
        assert_eq!(read_exp_value(&blank(900, 800)), None, "值区无 ink → None");
    }
}
