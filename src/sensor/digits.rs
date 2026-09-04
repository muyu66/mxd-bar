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

/// 读当前级累积 EXP 的全量整数。读不到（值区被盖/分辨率不符/几何漂出窗口）返回 None。
pub fn read_exp_value(frame: &BgraFrame) -> Option<i64> {
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
