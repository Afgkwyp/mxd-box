//! HUD 定位之后的读数：**用锚点量出来的缩放，把经验那一行还原回游戏原生像素再认字**。
//!
//! # 为什么全屏之后读不到（真机日志里的现场）
//!
//! 游戏选 1366×768、显示器是 2560×1440 时，「全屏」是把 768p 的画面**整幅拉伸**
//! 1.875 倍铺满屏幕（日志：`HUD 锚点定位：客户区 2560×1440 · 缩放 1.88`）。
//! 非整数倍的双线性拉伸会把 7 像素高的点阵字抹成一片灰边，内置字形对不上；
//! 旧的通用路径只能从模糊的墨迹里**重新猜**字高，而绿色括号 `[` 的亮度 171
//! 只比墨迹阈值 170 高 1，糊一下就没了 —— 于是「锚点找到了，就是读不出」。
//!
//! 可是锚点已经把缩放量准了（HP↔EXP 两个条的间距），没理由再猜：
//!
//! 1. 按 `sx/sy` 把文字区**逆向采样**回原生像素（原生像素中心 ↔ 客户区坐标），
//!    双线性拉伸在原像素中心处的取值就是原值，字形基本复原；
//! 2. 按「最大通道 + 逐行底色」二值化（括号是绿色，亮度低但最大通道 204）；
//! 3. 锚点有 ±1 像素的误差，所以试几组亚像素相位，取经验表最自洽的那条；
//! 4. 字形路径都不行时，用 PP-OCR 读**原分辨率**的数字区（跳过 `EXP` 标签），
//!    和网页版一样只把字那一块喂给模型。
//!
//! 所有读数最后仍然要过 `table::validate`，这里不放松任何一条校验。

use crate::exp::font::{self, Font, Pixels};
use crate::exp::hud::HudLayout;

/// 读出来的经验行 + 走的是哪条路（写日志用）。
#[derive(Debug, Clone)]
pub struct HudReading {
    /// `rect` 已换算成客户区像素坐标。
    pub reading: font::Reading,
    pub path: ReadPath,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadPath {
    /// 1× 原样读（窗口化 1080p / 768p）
    Native,
    /// 按锚点量出的缩放，把点阵逐字放大比对（无边框全屏的最近邻拉伸）
    Scaled,
    /// 按缩放灰度逐字滑动比对（系统 DPI 缩放又糊了一遍：4K + 150%）
    Soft,
    /// 逆向采样回原生像素后读字形（平滑拉伸 / 任意缩放）
    Resampled,
    /// PP-OCR
    Ocr,
}

impl ReadPath {
    pub fn label(self) -> &'static str {
        match self {
            ReadPath::Native => "原生字形",
            ReadPath::Scaled => "按缩放逐字比对点阵",
            ReadPath::Soft => "按缩放灰度滑动比对",
            ReadPath::Resampled => "还原像素后认字形",
            ReadPath::Ocr => "PP-OCR",
        }
    }
}

// ---------------------------------------------------------------------------
// 按缩放逐字比对点阵（无边框全屏的真机情况）
// ---------------------------------------------------------------------------
//
// 真机抓图（2560×1440 无边框全屏、游戏 1366×768，缩放 1.874）量出来的事实：
//
// * 数字是**最近邻**放大的：每个原生像素被整块复制成 1 或 2 个像素，不混色；
// * 但**每个字符是各自贴上去的**，各有各的像素相位 —— 同一行里有的字「第 4 行
//   只占 1 个像素」，隔壁的字是「第 3 行只占 1 个像素」。于是没有一个网格能把整行
//   还原回 1×（试过：数字之间的空隙会被吃掉、字形粘连）；
// * 背景渐变、经验条又是平滑插值的，和字的放大方式都不一样。
//
// 所以逐个字来：按空列切出每个字，拿内置点阵按**锚点量出来的缩放**、在几种相位下
// 放大，和这个字逐像素比，选最吻合的。缩放是已知的，要猜的只有相位 —— 这比 OCR
// 猜整串稳得多；结果仍交给同一套结构解析 + 经验表自检。

/// 相位的搜索步数（每个方向在 `[0, 1)` 个原生像素里均匀取）。
///
/// 要细：相位决定每个原生像素被复制成 1 行还是 2 行，步长 0.2 时有的字总差一两行对不上。
const PHASE_STEPS: usize = 12;
/// 一个字「认得出」：失配像素占（模板墨 ∪ 实际墨）的比例上限。
const GLYPH_MAX_MISMATCH: f64 = 0.22;
/// 第一名要比第二名（换一个字）好这么多，才不算「两个字都像」。
const GLYPH_MIN_MARGIN: f64 = 0.05;

/// 经验字段会用到的字符（`EXP` 标签在裁剪区域外）。
const FIELD_CHARS: [char; 14] = [
    '0', '1', '2', '3', '4', '5', '6', '7', '8', '9', '(', ')', '.', '%',
];

/// 二值化后的一块：`true` = 墨。
struct InkMask {
    width: usize,
    height: usize,
    ink: Vec<bool>,
}

impl InkMask {
    fn from(pixels: &Pixels<'_>) -> Option<Self> {
        let binary = binarize(pixels)?;
        let b = binary.pixels();
        Some(Self {
            width: b.width,
            height: b.height,
            ink: (0..b.width * b.height).map(|i| b.bgra[i * 4] > 128).collect(),
        })
    }

    fn at(&self, x: i64, y: i64) -> bool {
        x >= 0
            && y >= 0
            && (x as usize) < self.width
            && (y as usize) < self.height
            && self.ink[y as usize * self.width + x as usize]
    }
}

/// 逐字比对用的模板：内置点阵，括号换成真机上的完整形状。
///
/// 内置的 `(` `)` 只有 7 行（点阵字格就 7 行高，够结构解析认），可真机上的括号是
/// 9 行高的 `[` `]`，下面还有一道横。逐字比对会看字的全高，用完整形状才对得上。
fn scaled_templates(font: &Font) -> Vec<(char, Vec<Vec<bool>>)> {
    let bracket = |stem: usize| -> Vec<Vec<bool>> {
        (0..9)
            .map(|row| {
                let mut line = vec![false; 2];
                line[stem] = true;
                if row == 0 || row == 8 {
                    line[1 - stem] = true;
                }
                line
            })
            .collect()
    };
    FIELD_CHARS
        .iter()
        .filter_map(|ch| match ch {
            '(' => Some((*ch, bracket(0))),
            ')' => Some((*ch, bracket(1))),
            _ => font.bitmap_of(*ch).map(|b| (*ch, bitmap_rows(b))),
        })
        .collect()
}

/// 一个字的点阵（按行）。
fn bitmap_rows(bitmap: &font::Bitmap) -> Vec<Vec<bool>> {
    bitmap
        .to_ascii()
        .split('|')
        .map(|row| row.chars().map(|c| c == '#').collect())
        .collect()
}

/// 一个方向上所有「对位 × 相位」摆法各自把窗口里的每个像素映射到第几个原生像素
/// （`-1` = 落在字外），**去重之后**交出来。
///
/// 1 像素的位移 × 12 档相位有 36 种摆法，可它们落到整数像素上的分配方式只有十来种 ——
/// 先去重，逐字比对的工作量少一个数量级。
fn axis_maps(window: (i64, i64), start: i64, len: usize, scale: f64) -> Vec<Vec<i32>> {
    let mut maps: Vec<Vec<i32>> = Vec::new();
    for shift in -1i64..=1 {
        for step in 0..PHASE_STEPS {
            let phase = step as f64 / PHASE_STEPS as f64 * scale;
            let map: Vec<i32> = (window.0..window.1)
                .map(|p| {
                    let index = (((p - start - shift) as f64 + phase) / scale).floor();
                    if index >= 0.0 && (index as usize) < len {
                        index as i32
                    } else {
                        -1
                    }
                })
                .collect();
            if !maps.contains(&map) {
                maps.push(map);
            }
        }
    }
    maps
}

/// 一个模板在这一块上最好的失配率（各种相位、±1 像素的对位里取最小）。
fn best_mismatch(
    mask: &InkMask,
    (x0, x1): (usize, usize),
    (top, band): (usize, usize),
    rows: &[Vec<bool>],
    scale: (f64, f64),
) -> Option<f64> {
    let width = rows.first().map(|r| r.len()).unwrap_or(0);
    let has_ink = |i: &usize| rows.iter().any(|row| row[*i]);
    let first_ink = (0..width).find(has_ink)?;
    let last_ink = (0..width).rev().find(has_ink)?;
    // 最近邻放大下，n 个原生像素只可能占 ⌊n·S⌋ 或 ⌈n·S⌉ 列 —— 宽度差出 1.5 列
    // 就不是这个字（也挡住「`8` 把粘在后面的小数点一起吞掉」这种误认）
    let ink_w = (last_ink - first_ink + 1) as f64 * scale.0;
    if (ink_w - (x1 - x0 + 1) as f64).abs() > 1.5 {
        return None;
    }
    // 比对窗口：左右各多 2 列；上下各多看 2 行 —— 每个字各自取整贴图，
    // 同一行里的字可能错开一行；括号的模板是真机那样 9 行高的，窗口要装得下它
    let columns = (x0 as i64 - 2, x1 as i64 + 3);
    let lines = (top as i64 - 2, (top + band) as i64 + (2.0 * scale.1).ceil() as i64 + 2);
    // 让模板最左的墨对齐这一块的左缘（`1` 的点阵左边空一列）
    let left = x0 as i64 - (first_ink as f64 * scale.0).round() as i64;
    let column_maps = axis_maps(columns, left, width, scale.0);
    let row_maps = axis_maps(lines, top as i64, rows.len(), scale.1);
    // 这一块自己的墨（只算它自己的列，旁边的字不算）
    let observed: Vec<Vec<bool>> = (lines.0..lines.1)
        .map(|v| {
            (columns.0..columns.1)
                .map(|u| u >= x0 as i64 && u <= x1 as i64 && mask.at(u, v))
                .collect()
        })
        .collect();
    let mut best = f64::MAX;
    for row_map in &row_maps {
        for column_map in &column_maps {
            let (mut miss, mut total) = (0usize, 0usize);
            for (r, &j) in row_map.iter().enumerate() {
                for (c, &i) in column_map.iter().enumerate() {
                    let t = j >= 0 && i >= 0 && rows[j as usize][i as usize];
                    let m = observed[r][c];
                    if t || m {
                        total += 1;
                        if t != m {
                            miss += 1;
                        }
                    }
                }
            }
            if total > 0 {
                best = best.min(miss as f64 / total as f64);
            }
            if best == 0.0 {
                return Some(0.0);
            }
        }
    }
    Some(best)
}

/// 这一块最像哪个字；两个字都像、或者都不像，就是认不出（`None`）。
fn match_scaled_glyph(
    mask: &InkMask,
    columns: (usize, usize),
    line: (usize, usize),
    scale: (f64, f64),
    templates: &[(char, Vec<Vec<bool>>)],
) -> Option<(char, f64)> {
    if looks_like_dot(mask, columns, line, scale) {
        return Some(('.', 0.0));
    }
    let mut scores: Vec<(f64, char)> = templates
        .iter()
        .filter_map(|(ch, rows)| {
            best_mismatch(mask, columns, line, rows, scale).map(|score| (score, *ch))
        })
        .collect();
    scores.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    let (score, ch) = *scores.first()?;
    let runner_up = scores.get(1).map(|s| s.0).unwrap_or(f64::MAX);
    (score <= GLYPH_MAX_MISMATCH && runner_up - score >= GLYPH_MIN_MARGIN).then_some((ch, score))
}

/// 小数点：又小（不超过 2 个原生像素见方）、又只在字行的下半截。
///
/// 真机上小数点带一圈半透明的抗锯齿（1× 素材里是 `(201,250,191)` 这种色），
/// 二值化之后形状和点阵模板对不准；可它在这一行里的特征是独一份的 ——
/// 其余字符都顶着字行上沿。
fn looks_like_dot(
    mask: &InkMask,
    (x0, x1): (usize, usize),
    (top, band): (usize, usize),
    scale: (f64, f64),
) -> bool {
    let rows: Vec<usize> = (top..top + band)
        .filter(|y| (x0..=x1).any(|x| mask.at(x as i64, *y as i64)))
        .collect();
    let (Some(&first), Some(&last)) = (rows.first(), rows.last()) else {
        return false;
    };
    let small = (x1 - x0 + 1) as f64 <= 2.0 * scale.0 + 1.0
        && (last - first + 1) as f64 <= 2.0 * scale.1 + 1.0;
    small && (first - top) as f64 >= band as f64 * 0.45
}

/// 认一块；认不出时试着把它切成**紧挨着的两个字**。
///
/// 每个字各自取整贴图，原生 1 像素的字距有时会被挤没（真机上 `8` 和后面的
/// 小数点就粘成了一块）。两半都认得出才采纳，取失配最小的切法。
fn match_or_split(
    mask: &InkMask,
    (x0, x1): (usize, usize),
    line: (usize, usize),
    scale: (f64, f64),
    templates: &[(char, Vec<Vec<bool>>)],
) -> Vec<((usize, usize), Option<char>)> {
    if let Some((ch, _)) = match_scaled_glyph(mask, (x0, x1), line, scale, templates) {
        return vec![((x0, x1), Some(ch))];
    }
    let mut best: Option<(f64, usize, char, char)> = None;
    for cut in x0 + 1..=x1 {
        let left = match_scaled_glyph(mask, (x0, cut - 1), line, scale, templates);
        let right = match_scaled_glyph(mask, (cut, x1), line, scale, templates);
        if let (Some((a, sa)), Some((b, sb))) = (left, right) {
            if best.map(|best| sa + sb < best.0).unwrap_or(true) {
                best = Some((sa + sb, cut, a, b));
            }
        }
    }
    match best {
        Some((_, cut, a, b)) => vec![((x0, cut - 1), Some(a)), ((cut, x1), Some(b))],
        None => vec![((x0, x1), None)],
    }
}

/// 在数字那一段（客户区分辨率）逐字比对，交出读数（矩形是这一块里的坐标）。
fn read_scaled(crop: &Pixels<'_>, scale: (f64, f64), font: &Font) -> Option<font::Reading> {
    let mask = InkMask::from(crop)?;
    let band = (font::CELL_HEIGHT as f64 * scale.1).round() as usize;
    // 字行顶：第一行有墨的（数字和括号是顶端对齐的）
    let top = (0..mask.height)
        .find(|y| (0..mask.width).any(|x| mask.at(x as i64, *y as i64)))?;
    if top + band > mask.height {
        return None;
    }
    let column_has_ink =
        |x: usize| (top..top + band).any(|y| mask.at(x as i64, y as i64));
    // 按空列切字
    let mut blobs: Vec<(usize, usize)> = Vec::new();
    let mut start: Option<usize> = None;
    for x in 0..=mask.width {
        let ink = x < mask.width && column_has_ink(x);
        match (ink, start) {
            (true, None) => start = Some(x),
            (false, Some(s)) => {
                blobs.push((s, x - 1));
                start = None;
            }
            _ => {}
        }
    }
    if blobs.len() < 7 {
        return None;
    }
    let templates = scaled_templates(font);
    // 从左往右认；认出右括号就停 —— 字段到它为止，右边的东西和读数无关，
    // 还省下对它们逐字比对 / 试切分的功夫
    let mut pieces: Vec<((usize, usize), Option<char>)> = Vec::new();
    for blob in &blobs {
        let matched = match_or_split(&mask, *blob, (top, band), scale, &templates);
        // 数字开始之后还有切开也认不出的字：这一帧不是这种渲染（比如又被系统缩放
        // 糊了一遍），带着它也读不出自洽的一行，别在后面的字上白花功夫。
        // 数字之前的认不出没关系 —— 手动框常把 `EXP` 标签一起框进来
        let started = pieces.iter().any(|(_, slot)| slot.is_some());
        if started && matched.iter().any(|(_, slot)| slot.is_none()) {
            return None;
        }
        pieces.extend(matched);
        if pieces.last().map(|(_, slot)| *slot == Some(')')).unwrap_or(false) {
            break;
        }
    }
    let slots: Vec<Option<char>> = pieces.iter().map(|(_, slot)| *slot).collect();
    let glyphs: Vec<font::Glyph> = pieces
        .iter()
        .map(|((x0, x1), slot)| font::Glyph {
            x0: *x0,
            x1: *x1,
            bitmap: slot
                .and_then(|ch| font.bitmap_of(ch).cloned())
                .unwrap_or_else(|| font::Bitmap::from_ascii(&["#"])),
        })
        .collect();
    let field = font::locate(&glyphs, &slots)?;
    // 经验数字的第一位贴着裁剪左缘：它可能是被裁掉一半的数字（或者左边还有一位
    // 没裁进来）。宁可不认，也不给半个数字。
    if glyphs[field.start].x0 < 2 {
        return None;
    }
    Some(font::Reading {
        exp: field.exp,
        percent: field.percent,
        raw: slots.iter().map(|s| s.unwrap_or('?')).collect(),
        rect: (glyphs[field.start].x0, top, glyphs[field.end].x1 + 1, top + band),
        exact: 0,
        glyphs: glyphs.len(),
        text_height: band,
    })
}

// ---------------------------------------------------------------------------
// 灰度逐字滑动比对（两次拉伸糊在一起的情况）
// ---------------------------------------------------------------------------
//
// 4K 显示器常开 150% 缩放。游戏不认高 DPI，Windows 就把它的画面**再平滑拉伸
// 1.5 倍**：游戏先按 1.25 倍逐字最近邻贴字，系统再整幅双线性一糊（日志里窗口化
// 客户区 2880×1620 · 缩放 1.500、全屏 3840×2160 · 缩放 1.884 就是这种）。
// 二值化之后字的笔画一粗一细、相邻的字粘成一片 —— 按空列切字、逐像素比对都失效，
// 逆向采样也还原不回来（两次拉伸的网格对不上），只剩 OCR 时好时坏。
//
// 这里不二值化、也不切字：把每个点阵按缩放**画成灰度**（每个客户区像素被字覆盖
// 的面积，再做一次和拉伸同量级的模糊），从左到右一个字一个字地**滑过去**比 ——
// 上一个字认出来，下一个字的位置就大致知道了（字宽 + 1 个原生像素的字距），
// 只要在附近小范围里找。缩放已知，糊了也好，粘了也好，都只是灰度上差一点。

/// 灰度比对里一个字「认得出」：失配（Σ|实际−模板| / Σmax）的上限。
const SOFT_MAX_MISMATCH: f64 = 0.55;
/// 模板轻糊的中心权重（`[1, c, 1]`，见 [`render_soft`]）。
const SOFT_BLUR_CENTER: f32 = 6.0;
/// 小数点的失配上限（见 [`soft_limit`]）。
const SOFT_DOT_MAX_MISMATCH: f64 = 0.75;
/// 位置搜索的步长（客户区像素）。
const SOFT_STEP: f64 = 0.25;
/// 下一个字离预计位置最多偏多少（原生像素；至少 2 个客户区像素）：每个字各自
/// 取整贴图，两次拉伸下来能差出一个半到两个客户区像素，缩放越大差得越多。
const SOFT_ADVANCE_REACH: f64 = 0.9;
/// 束搜索每一步留几条假设。
const SOFT_BEAM: usize = 3;
/// 两条都和经验表自洽、数不一样的假设，总失配差不到这么多就算分不清
/// （大约是「分歧的那个字上，失配只差这么多」）。
const SOFT_AMBIGUITY: f64 = 0.03;

/// 灰度比对的模板：点阵换成墨量（0~1）。小数点按真机的样子带一圈半透明边
/// （1× 素材里量到 `(201,…)` `(150,…)` `(250,…)` `(191,…)`，折成墨量大约如下）——
/// 它只有三四个像素，边上差一点失配率就上去了。
fn soft_templates(font: &Font) -> Vec<(char, Vec<Vec<f32>>)> {
    let label = LABEL_CHARS
        .iter()
        .filter_map(|ch| font.bitmap_of(*ch).map(|b| (*ch, bitmap_rows(b))));
    scaled_templates(font)
        .into_iter()
        .chain(label)
        .map(|(ch, rows)| {
            let mut soft: Vec<Vec<f32>> = rows
                .iter()
                .map(|row| row.iter().map(|&ink| if ink { 1.0 } else { 0.0 }).collect())
                .collect();
            if ch == '.' && soft.len() == 7 && soft[4].len() == 2 {
                soft[4] = vec![0.7, 0.35];
                soft[5] = vec![1.0, 0.65];
            }
            (ch, soft)
        })
        .collect()
}

/// `EXP` 标签的三个字：只用来认出「这一段是标签，不是数字的开头」。
const LABEL_CHARS: [char; 3] = ['E', 'X', 'P'];

/// 数字区的灰度墨图：底色 → 0，白字 → 1（逐行扣底色，和 [`binarize`] 同一套）。
struct SoftInk {
    width: usize,
    height: usize,
    value: Vec<f32>,
}

impl SoftInk {
    fn from(pixels: &Pixels<'_>) -> Option<Self> {
        let (w, h) = (pixels.width, pixels.height);
        if w == 0 || h == 0 {
            return None;
        }
        let all: Vec<u8> = (0..h)
            .flat_map(|y| (0..w).map(move |x| (x, y)))
            .map(|(x, y)| ink(pixels, x, y))
            .collect();
        let mut sorted = all.clone();
        sorted.sort_unstable();
        let high = sorted[(sorted.len() * 97 / 100).min(sorted.len() - 1)] as f32;
        let mut value = vec![0f32; w * h];
        for y in 0..h {
            let mut row: Vec<u8> = all[y * w..(y + 1) * w].to_vec();
            row.sort_unstable();
            let background = row[w * 30 / 100] as f32;
            if high - background < 40.0 {
                continue;
            }
            for x in 0..w {
                let v = (all[y * w + x] as f32 - background) / (high - background);
                value[y * w + x] = v.clamp(0.0, 1.0);
            }
        }
        Some(Self {
            width: w,
            height: h,
            value,
        })
    }

    fn at(&self, x: i64, y: i64) -> f32 {
        if x < 0 || y < 0 || x as usize >= self.width || y as usize >= self.height {
            0.0
        } else {
            self.value[y as usize * self.width + x as usize]
        }
    }
}

/// 点阵在客户区里的灰度样子：原点 `(ox, oy)`（字格左上角，可以是小数），
/// 每个客户区像素 = 它被墨覆盖的面积，再轻糊一下（拉伸糊出来的半格边）。
/// 交出 `(左, 上, 宽, 高, 值)`，窗口比字格四周各多 1 个原生像素。
fn render_soft(
    rows: &[Vec<f32>],
    (ox, oy): (f64, f64),
    scale: (f64, f64),
) -> (i64, i64, usize, usize, Vec<f32>) {
    let cols = rows.first().map(|r| r.len()).unwrap_or(0);
    // 字格右边再带上 1 列字距（那里必须是空的）
    let x0 = (ox - scale.0).floor() as i64;
    let y0 = (oy - scale.1).floor() as i64;
    let x1 = (ox + (cols + 1) as f64 * scale.0).floor() as i64;
    let y1 = (oy + (rows.len() + 1) as f64 * scale.1).ceil() as i64;
    let (w, h) = ((x1 - x0) as usize, (y1 - y0) as usize);
    // 每一列 / 每一行和各个原生格子的重叠长度，面积 = 两者之积
    let overlap = |p: i64, origin: f64, s: f64, n: usize| -> Vec<(usize, f64)> {
        let (a, b) = (p as f64 - origin, p as f64 + 1.0 - origin);
        let first = (a / s).floor().max(0.0) as usize;
        let last = ((b / s).ceil().max(0.0) as usize).min(n);
        (first..last)
            .filter_map(|i| {
                let lo = a.max(i as f64 * s);
                let hi = b.min((i + 1) as f64 * s);
                (hi > lo).then_some((i, hi - lo))
            })
            .collect()
    };
    let col_overlap: Vec<Vec<(usize, f64)>> =
        (x0..x1).map(|x| overlap(x, ox, scale.0, cols)).collect();
    let row_overlap: Vec<Vec<(usize, f64)>> =
        (y0..y1).map(|y| overlap(y, oy, scale.1, rows.len())).collect();
    let mut sharp = vec![0f32; w * h];
    for (r, row_parts) in row_overlap.iter().enumerate() {
        for (c, col_parts) in col_overlap.iter().enumerate() {
            let mut cover = 0.0;
            for &(j, dy) in row_parts {
                for &(i, dx) in col_parts {
                    cover += rows[j][i] as f64 * dx * dy;
                }
            }
            sharp[r * w + c] = cover as f32;
        }
    }
    // 再轻轻糊一下（`[1,6,1]`，横竖各一次）：两次拉伸下来细笔画会摊成两个半亮的像素，
    // 纯覆盖面积比它锐利；糊得太重（`[1,2,1]`）又会把竖笔画的峰值压到 0.6 ——
    // 这一档是在几种 DPI 组合上试出来的，`3`/`8` 分得最开
    let center = SOFT_BLUR_CENTER;
    let get = |v: &[f32], x: i64, y: i64| {
        if x < 0 || y < 0 || x as usize >= w || y as usize >= h {
            0.0
        } else {
            v[y as usize * w + x as usize]
        }
    };
    let mut tmp = vec![0f32; w * h];
    for y in 0..h as i64 {
        for x in 0..w as i64 {
            let sum = get(&sharp, x - 1, y) + center * get(&sharp, x, y) + get(&sharp, x + 1, y);
            tmp[y as usize * w + x as usize] = sum / (center + 2.0);
        }
    }
    let mut out = vec![0f32; w * h];
    for y in 0..h as i64 {
        for x in 0..w as i64 {
            let sum = get(&tmp, x, y - 1) + center * get(&tmp, x, y) + get(&tmp, x, y + 1);
            out[y as usize * w + x as usize] = sum / (center + 2.0);
        }
    }
    (x0, y0, w, h, out)
}

/// 模板画在客户区里的样子（相对于字格原点的整数部分）。
struct SoftRender {
    x0: i64,
    y0: i64,
    width: usize,
    height: usize,
    value: Vec<f32>,
}

/// 同一个模板只有 4×4 种亚像素相位（位置按 [`SOFT_STEP`] 对齐），各画一次就够了。
struct SoftRenders<'a> {
    templates: &'a [(char, Vec<Vec<f32>>)],
    scale: (f64, f64),
    cache: Vec<Option<SoftRender>>,
}

impl<'a> SoftRenders<'a> {
    const PHASES: i64 = 4;

    fn new(templates: &'a [(char, Vec<Vec<f32>>)], scale: (f64, f64)) -> Self {
        let slots = templates.len() * (Self::PHASES * Self::PHASES) as usize;
        Self {
            templates,
            scale,
            cache: (0..slots).map(|_| None).collect(),
        }
    }

    /// 第 `t` 个模板画在 `(ox, oy)`（先对齐到 1/4 像素）：交出缓存槽位和整数位移。
    fn place(&mut self, t: usize, (ox, oy): (f64, f64)) -> (usize, i64, i64) {
        let quarter = |v: f64| {
            let q = (v * Self::PHASES as f64).round() as i64;
            (q.div_euclid(Self::PHASES), q.rem_euclid(Self::PHASES))
        };
        let ((ix, fx), (iy, fy)) = (quarter(ox), quarter(oy));
        let slot = (t as i64 * Self::PHASES * Self::PHASES + fx * Self::PHASES + fy) as usize;
        if self.cache[slot].is_none() {
            let origin = (fx as f64 / Self::PHASES as f64, fy as f64 / Self::PHASES as f64);
            let (x0, y0, width, height, value) =
                render_soft(&self.templates[t].1, origin, self.scale);
            self.cache[slot] = Some(SoftRender {
                x0,
                y0,
                width,
                height,
                value,
            });
        }
        (slot, ix, iy)
    }

    /// 放好的模板在客户区 `(x, y)` 处的墨量。
    fn value_at(&self, (slot, ix, iy): (usize, i64, i64), x: i64, y: i64) -> f32 {
        let Some(render) = &self.cache[slot] else {
            return 0.0;
        };
        let (c, r) = (x - ix - render.x0, y - iy - render.y0);
        if c < 0 || r < 0 || c as usize >= render.width || r as usize >= render.height {
            0.0
        } else {
            render.value[r as usize * render.width + c as usize]
        }
    }

    /// 第 `t` 个模板放在 `(ox, oy)` 时和实际的失配：Σ|实际−预期| / Σmax(实际, 预期)。
    ///
    /// `previous`：前一个字（模板, 位置）。它糊出来的边会渗进这个字的窗口 ——
    /// 预期里把它也画上（取两者较大），而不是当成这个字的墨去比：
    /// 否则 `4` 右边那一竖糊过来，紧挨着的 `3` 左边就「多了墨」，看着像 `8`。
    fn mismatch(
        &mut self,
        ink: &SoftInk,
        t: usize,
        origin: (f64, f64),
        previous: Option<(usize, (f64, f64))>,
    ) -> f64 {
        let here = self.place(t, origin);
        let before = previous.map(|(pt, po)| self.place(pt, po));
        let Some(render) = &self.cache[here.0] else {
            return f64::MAX;
        };
        let (mut diff, mut total) = (0f64, 0f64);
        for r in 0..render.height {
            let y = here.2 + render.y0 + r as i64;
            for c in 0..render.width {
                let x = here.1 + render.x0 + c as i64;
                let mut e = render.value[r * render.width + c];
                if let Some(before) = before {
                    e = e.max(self.value_at(before, x, y));
                }
                let m = ink.at(x, y);
                diff += (m - e).abs() as f64;
                total += m.max(e) as f64;
            }
        }
        if total <= 0.0 {
            f64::MAX
        } else {
            diff / total
        }
    }
}

/// 灰度滑动比对的一条假设：到目前为止认出的字，和下一个字该从哪儿找。
#[derive(Clone)]
struct SoftPath {
    /// 各字失配之和（排序用平均值）
    total: f64,
    /// (字格左, 字格右, 字)
    pieces: Vec<(f64, f64, char)>,
    /// 下一个字格左上角的预计 x
    next_x: f64,
    /// 字行顶（第一个字定下来之后就跟着它）
    top: f64,
    /// 前一个字（模板序号, 位置）
    previous: Option<(usize, (f64, f64))>,
}

impl SoftPath {
    fn mean(&self) -> f64 {
        self.total / self.pieces.len().max(1) as f64
    }
}

/// 经验那一行的格式 `数字…[数字….两位数字%]`：已经认出 `so_far` 之后，下一个字能不能是 `ch`。
///
/// 束搜索里的假设常常只差在前面某个字（`1` 还是 `[`），后面一模一样 ——
/// 不合格式的当场就丢，别让它们占着名额把对的那条挤出去。
fn soft_grammar_allows(so_far: &[(f64, f64, char)], ch: char) -> bool {
    let chars: Vec<char> = so_far.iter().map(|p| p.2).collect();
    let Some(open) = chars.iter().position(|c| *c == '(') else {
        // 还在经验数字里：数字，或者（至少一位之后）左括号
        return ch.is_ascii_digit() && chars.len() < 12 || ch == '(' && !chars.is_empty();
    };
    let percent = &chars[open + 1..];
    match percent.iter().position(|c| *c == '.') {
        // 百分比的整数部分：1~3 位，然后小数点
        None => ch.is_ascii_digit() && percent.len() < 3 || ch == '.' && !percent.is_empty(),
        Some(dot) => {
            let decimals = percent.len() - dot - 1;
            match (decimals, percent.last()) {
                (0 | 1, _) => ch.is_ascii_digit(),
                (2, _) => ch == '%',
                (_, Some('%')) => ch == ')',
                _ => false,
            }
        }
    }
}

/// 一个字「认得出」的失配上限。小数点只有三四个像素，两次拉伸之后形状最不可靠，
/// 放宽一些 —— 它仍然要在所有模板里最像，而且整行最后还要过经验表。
fn soft_limit(ch: char) -> f64 {
    if ch == '.' {
        SOFT_DOT_MAX_MISMATCH
    } else {
        SOFT_MAX_MISMATCH
    }
}

/// 在数字那一段按灰度逐字滑动比对，交出读数（矩形是这一块里的坐标）。
///
/// 每个字都只在上一个字「后面一格」附近找，所以一个字放偏了会带偏后面的；
/// 不只押最像的那一个，每一步留几条最好的假设（束搜索），最后按经验表挑。
fn read_soft(crop: &Pixels<'_>, scale: (f64, f64), font: &Font) -> Option<font::Reading> {
    let ink = SoftInk::from(crop)?;
    let templates = soft_templates(font);
    let mut renders = SoftRenders::new(&templates, scale);
    let column_ink: Vec<f32> = (0..ink.width)
        .map(|x| (0..ink.height).map(|y| ink.at(x as i64, y as i64)).fold(0f32, f32::max))
        .collect();
    let row_ink =
        |y: usize| (0..ink.width).map(|x| ink.at(x as i64, y as i64)).fold(0f32, f32::max);
    // 字行顶是第一行像样的墨（半糊的边算半行）
    let first_y = (0..ink.height).find(|y| row_ink(*y) > 0.5)? as f64;
    // 第一个字从某一段墨的左端开始。HUD 路径裁的就是数字那一段，第一段就是；
    // 手动框里常带着 `EXP` 标签，那几段对不上任何模板，换下一段再起头
    let starts: Vec<usize> = (0..ink.width)
        .filter(|&x| column_ink[x] > 0.5 && (x == 0 || column_ink[x - 1] <= 0.5))
        .take(8)
        .collect();
    // 只有「这一段连第一个字都认不出」才换下一段；认出了却没读成（或者分不清），
    // 就到此为止 —— 从数字中间起头只会少读前几位
    for first_x in starts {
        if let Ok(reading) =
            read_soft_from(&ink, &mut renders, &column_ink, first_x, first_y, scale, font)
        {
            return reading;
        }
    }
    None
}

/// [`read_soft`] 的一次尝试：第一个字从 `first_x` 那一列起头。
/// `Err(())`：这一段连第一个字都认不出（多半是 `EXP` 标签）。
fn read_soft_from(
    ink: &SoftInk,
    renders: &mut SoftRenders<'_>,
    column_ink: &[f32],
    first_x: usize,
    first_y: f64,
    scale: (f64, f64),
    font: &Font,
) -> Result<Option<font::Reading>, ()> {
    let templates = renders.templates;
    let steps = |center: f64, radius: f64| -> Vec<f64> {
        let n = (radius / SOFT_STEP).round() as i64;
        (-n..=n).map(|k| center + k as f64 * SOFT_STEP).collect()
    };
    let ink_left = |rows: &[Vec<f32>]| {
        (0..rows[0].len()).find(|i| rows.iter().any(|r| r[*i] > 0.0)).unwrap_or(0)
    };
    let ink_after = |x: f64| {
        let from = x.ceil().max(0.0) as usize;
        column_ink.iter().skip(from).any(|v| *v > 0.5)
    };

    // 搜索半径跟着缩放走：每个字各自取整贴图的误差、再被系统拉伸放大，
    // 都是「原生像素的几分之一 × 缩放」这个量级
    let reach_x = (SOFT_ADVANCE_REACH * scale.0).max(2.0);
    let reach_y = (SOFT_ADVANCE_REACH * 0.5 * scale.1).max(0.75);
    let mut beam: Vec<SoftPath> = vec![SoftPath {
        total: 0.0,
        pieces: Vec::new(),
        next_x: f64::NAN,
        top: first_y - 0.5,
        previous: None,
    }];
    let mut finished: Vec<SoftPath> = Vec::new();
    for _ in 0..24 {
        let mut grown: Vec<SoftPath> = Vec::new();
        for path in &beam {
            // 这条假设下，每个模板各自的最佳摆位
            let first = path.pieces.is_empty();
            let mut options: Vec<(f64, char, f64, f64, usize, usize)> = templates
                .iter()
                .enumerate()
                // 经验数字总是以数字开头：第一个字只在 0~9 和标签字母里挑
                // （字母赢了就说明这一段是标签）；之后就不再比标签字母
                .filter(|(_, (ch, _))| {
                    if first {
                        ch.is_ascii_digit() || LABEL_CHARS.contains(ch)
                    } else {
                        soft_grammar_allows(&path.pieces, *ch)
                    }
                })
                .filter_map(|(t, (ch, rows))| {
                    let xs = if first {
                        steps(first_x as f64 - ink_left(rows) as f64 * scale.0, reach_x)
                    } else {
                        steps(path.next_x, reach_x)
                    };
                    let ys = steps(path.top, if first { reach_x } else { reach_y });
                    let mut best: Option<(f64, f64, f64)> = None;
                    for &oy in &ys {
                        for &ox in &xs {
                            let score = renders.mismatch(ink, t, (ox, oy), path.previous);
                            if best.map(|b| score < b.0).unwrap_or(true) {
                                best = Some((score, ox, oy));
                            }
                        }
                    }
                    let (score, ox, oy) = best?;
                    (score <= soft_limit(*ch)).then_some((score, *ch, ox, oy, rows[0].len(), t))
                })
                .collect();
            options.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
            if first {
                // 最像的是标签字母：这一段是 `EXP`，换下一段起头
                if options.first().map(|o| LABEL_CHARS.contains(&o.1)).unwrap_or(false) {
                    return Err(());
                }
                options.retain(|o| o.1.is_ascii_digit());
            }
            if options.is_empty() && !first {
                finished.push(path.clone());
            }
            for &(score, ch, ox, oy, cols, t) in options.iter().take(SOFT_BEAM) {
                let right = ox + cols as f64 * scale.0;
                let mut next = path.clone();
                next.total += score;
                next.pieces.push((ox, right, ch));
                next.next_x = ox + (cols + 1) as f64 * scale.0;
                next.previous = Some((t, (ox, oy)));
                if first {
                    next.top = oy;
                }
                if ch == ')' || !ink_after(right) {
                    finished.push(next);
                } else {
                    grown.push(next);
                }
            }
        }
        grown.sort_by(|a, b| a.mean().partial_cmp(&b.mean()).unwrap_or(std::cmp::Ordering::Equal));
        // 名额按「读到格式的哪一段」分开算：还在经验数字里的、已经过了 `[` 的……
        // 否则「把 `[` 认成 `1`」的几条（后面几个字一样）会占满名额，
        // 把真过了括号的那条挤掉，而它们到小数点那里才会全军覆没
        let stage = |path: &SoftPath| {
            let has = |c: char| path.pieces.iter().any(|p| p.2 == c);
            (has('('), has('.'), has('%'))
        };
        let mut kept: Vec<SoftPath> = Vec::new();
        for path in grown {
            if kept.iter().filter(|k| stage(k) == stage(&path)).count() < SOFT_BEAM {
                kept.push(path);
            }
        }
        let grown = kept;
        if grown.is_empty() {
            break;
        }
        beam = grown;
    }
    finished.extend(beam);
    if finished.iter().all(|path| path.pieces.is_empty()) {
        return Err(());
    }
    finished.sort_by(|a, b| a.mean().partial_cmp(&b.mean()).unwrap_or(std::cmp::Ordering::Equal));

    let band = (font::CELL_HEIGHT as f64 * scale.1).round() as usize;
    let mut best: Option<font::Reading> = None;
    // 第一条和经验表自洽的假设（和它的平均失配）
    let mut accepted: Option<(font::Reading, f64)> = None;
    for path in &finished {
        if path.pieces.len() < 7 {
            continue;
        }
        let slots: Vec<Option<char>> = path.pieces.iter().map(|p| Some(p.2)).collect();
        let glyphs: Vec<font::Glyph> = path
            .pieces
            .iter()
            .map(|&(left, right, ch)| {
                let x0 = left.round().max(0.0) as usize;
                font::Glyph {
                    x0,
                    x1: (right.round().max(1.0) as usize - 1).max(x0),
                    bitmap: font
                        .bitmap_of(ch)
                        .cloned()
                        .unwrap_or_else(|| font::Bitmap::from_ascii(&["#"])),
                }
            })
            .collect();
        let Some(field) = font::locate(&glyphs, &slots) else {
            continue;
        };
        // 第一位贴着裁剪左缘：可能是被裁掉一半的数字，宁可不认
        if glyphs[field.start].x0 < 2 {
            continue;
        }
        let top = path.top.round().max(0.0) as usize;
        let reading = font::Reading {
            exp: field.exp,
            percent: field.percent,
            raw: slots.iter().map(|s| s.unwrap_or('?')).collect(),
            rect: (glyphs[field.start].x0, top, glyphs[field.end].x1 + 1, top + band),
            exact: 0,
            glyphs: glyphs.len(),
            text_height: band,
        };
        // 假设按失配从好到坏排好了：第一条和经验表自洽的就是答案 ——
        // 除非紧跟着还有一条**差不多一样像**、也自洽、数却不一样的
        // （`3`/`8` 糊得分不清时，两个数都能和百分比对上）。那就宁可这一帧不认。
        if font::reading_rank(&reading) == 0 {
            match &accepted {
                None => accepted = Some((reading, path.mean())),
                Some((first, mean)) => {
                    // 比总失配（≈ 分歧的那一个字上的差），不比平均 —— 一行十几个字，
                    // 一个字上的差距摊到平均里就看不出来了
                    if (path.mean() - mean) * path.pieces.len() as f64 > SOFT_AMBIGUITY {
                        break;
                    }
                    if (first.exp, first.percent) != (reading.exp, reading.percent) {
                        return Ok(None);
                    }
                }
            }
            continue;
        }
        if font::better_reading(&reading, best.as_ref()) {
            best = Some(reading);
        }
    }
    Ok(accepted.map(|(reading, _)| reading).or(best))
}

/// 一片连续有墨的行里，经验那一行可能的（字行顶, 缩放）。
///
/// 不能拿整片的高度去除：框得松时，右边的别的东西会和字行在竖直方向连成一片。
/// 按列切成一个个字、量每个字的高度 —— 数字是 7 个原生像素高，而一行里数字最多，
/// 所以**出现最多的字高 ÷ 7** 就是缩放（再备一个 ÷ 9：万一最多的是括号那种高度）。
fn line_candidates(mask: &InkMask, run_top: usize, run_bottom: usize) -> Vec<(usize, f64)> {
    // 按空列切块，每块量它的墨从哪行到哪行
    let mut blobs: Vec<(usize, usize)> = Vec::new(); // (顶, 高)
    let mut current: Option<(usize, usize)> = None; // (顶, 底)
    for x in 0..=mask.width {
        let extent = (x < mask.width)
            .then(|| {
                let rows: Vec<usize> = (run_top..=run_bottom)
                    .filter(|y| mask.at(x as i64, *y as i64))
                    .collect();
                Some((*rows.first()?, *rows.last()?))
            })
            .flatten();
        match (extent, current) {
            (Some((top, bottom)), None) => current = Some((top, bottom)),
            (Some((top, bottom)), Some((ct, cb))) => current = Some((ct.min(top), cb.max(bottom))),
            (None, Some((ct, cb))) => {
                blobs.push((ct, cb - ct + 1));
                current = None;
            }
            (None, None) => {}
        }
    }
    // 字高的众数（±1 行算同一个）
    let mut counts: Vec<(usize, usize)> = Vec::new(); // (高, 次数)
    for &(_, height) in &blobs {
        if height < 5 {
            continue;
        }
        let near = blobs
            .iter()
            .filter(|(_, other)| other.abs_diff(height) <= 1)
            .count();
        if !counts.iter().any(|(h, _)| *h == height) {
            counts.push((height, near));
        }
    }
    counts.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    let mut candidates = Vec::new();
    for &(height, _) in counts.iter().take(2) {
        // 这个高度的字，它们的顶（众数）就是字行顶
        let mut tops: Vec<usize> = blobs
            .iter()
            .filter(|(_, h)| h.abs_diff(height) <= 1)
            .map(|(top, _)| *top)
            .collect();
        tops.sort_unstable();
        let line_top = tops[tops.len() / 2];
        for divisor in [7.0, 9.0] {
            candidates.push((line_top, height as f64 / divisor));
        }
    }
    candidates
}

/// **没有 HUD 锚点时**的逐字比对：手动框、旧的自动框、整幅搜索的候选带。
///
/// 锚点给不出缩放，就从文字行本身量：括号是 9 个原生像素高、数字是 7 个，
/// 文字行（有墨、又不是整行亮线的连续几行）的高度 ÷ 9 或 ÷ 7 就是缩放的候选。
/// 逐字比对对缩放的容差有 10% 上下，这个估计足够准。
///
/// 这一步让「手动框选」在全屏拉伸下也和 HUD 路径一样稳 —— 以前手动框只能走
/// 按墨迹高度猜的点阵 + OCR，全屏下照样时好时坏。
pub fn read_unanchored(pixels: &Pixels<'_>, font: &Font) -> Option<font::Reading> {
    if pixels.width < 16 || pixels.height < 5 {
        return None;
    }
    let mask = InkMask::from(pixels)?;
    let row_ink = |y: usize| (0..mask.width).filter(|x| mask.at(*x as i64, y as i64)).count();
    // 文字行：有墨，但不是边框 / 经验条那种横贯大半个框的亮带
    let text_row = |y: usize| {
        let ink = row_ink(y);
        ink > 0 && (ink as f64) < mask.width as f64 * 0.6
    };
    let mut runs: Vec<(usize, usize)> = Vec::new();
    let mut start: Option<usize> = None;
    for y in 0..=mask.height {
        let text = y < mask.height && text_row(y);
        match (text, start) {
            (true, None) => start = Some(y),
            (false, Some(s)) => {
                runs.push((s, y - 1));
                start = None;
            }
            _ => {}
        }
    }
    let mut best: Option<font::Reading> = None;
    for (run_top, run_bottom) in runs {
        if !(5..=160).contains(&(run_bottom - run_top + 1)) {
            continue;
        }
        for (line_top, scale) in line_candidates(&mask, run_top, run_bottom) {
            if !(0.95..=4.2).contains(&scale) {
                continue;
            }
            // 只拿这一行（上下留两行）：让「第一行有墨的就是字行顶」成立
            let y0 = line_top.saturating_sub(2);
            let y1 = (line_top + (9.0 * scale).ceil() as usize + 3).min(pixels.height);
            if y1 <= y0 {
                continue;
            }
            let (cw, ch, buffer) = pixels.crop(0, y0, pixels.width, y1 - y0);
            let line = Pixels {
                width: cw,
                height: ch,
                bgra: &buffer,
            };
            // 逐像素比对读不成自洽的一行，就灰度滑动比对（系统 DPI 缩放又糊了一遍时）
            let scaled = read_scaled(&line, (scale, scale), font);
            let reading = match scaled {
                Some(reading) if font::reading_rank(&reading) == 0 => Some(reading),
                scaled => read_soft(&line, (scale, scale), font).or(scaled),
            };
            let Some(reading) = reading else {
                continue;
            };
            let (rx0, ry0, rx1, ry1) = reading.rect;
            let reading = font::Reading {
                rect: (rx0, ry0 + y0, rx1, ry1 + y0),
                ..reading
            };
            if font::reading_rank(&reading) == 0 {
                return Some(reading);
            }
            if font::better_reading(&reading, best.as_ref()) {
                best = Some(reading);
            }
        }
    }
    best.filter(|reading| font::reading_rank(reading) <= 1)
}

/// 逆向采样的亚像素相位（原生像素为单位）。锚点定位误差在 ±1 个客户区像素以内，
/// 缩放 ≥1 时就是 ±0.5 个原生像素以内。先试 0，由近及远。
const PHASES: [f64; 5] = [0.0, 0.25, -0.25, 0.5, -0.5];

/// 一块自己持有内存的 BGRA 图。
struct Owned {
    width: usize,
    height: usize,
    bgra: Vec<u8>,
}

impl Owned {
    fn pixels(&self) -> Pixels<'_> {
        Pixels {
            width: self.width,
            height: self.height,
            bgra: &self.bgra,
        }
    }
}

/// 逆向采样：输出的原生像素 `(i, j)` 的中心，对应客户区里的
/// `(left + (i + 0.5)·sx − 0.5, top + (j + 0.5)·sy − 0.5)`，双线性取值。
///
/// 双线性拉伸在原像素中心处的插值结果就是原像素本身；最近邻拉伸（整数倍 2×）
/// 在那里也正好落在同一个原像素的块里 —— 两种渲染方式都能还原。
fn resample(
    src: &Pixels<'_>,
    left: f64,
    top: f64,
    sx: f64,
    sy: f64,
    out_w: usize,
    out_h: usize,
) -> Option<Owned> {
    if src.width == 0 || src.height == 0 || out_w == 0 || out_h == 0 {
        return None;
    }
    let max_x = (src.width - 1) as f64;
    let max_y = (src.height - 1) as f64;
    let mut bgra = vec![0u8; out_w * out_h * 4];
    for j in 0..out_h {
        let cy = (top + (j as f64 + 0.5) * sy - 0.5).clamp(0.0, max_y);
        let y0 = cy.floor() as usize;
        let y1 = (y0 + 1).min(src.height - 1);
        let fy = cy - y0 as f64;
        for i in 0..out_w {
            let cx = (left + (i as f64 + 0.5) * sx - 0.5).clamp(0.0, max_x);
            let x0 = cx.floor() as usize;
            let x1 = (x0 + 1).min(src.width - 1);
            let fx = cx - x0 as f64;
            let at =
                |x: usize, y: usize, c: usize| src.bgra[(y * src.width + x) * 4 + c] as f64;
            let out = (j * out_w + i) * 4;
            for c in 0..3 {
                let top_row = at(x0, y0, c) * (1.0 - fx) + at(x1, y0, c) * fx;
                let bottom_row = at(x0, y1, c) * (1.0 - fx) + at(x1, y1, c) * fx;
                bgra[out + c] = (top_row * (1.0 - fy) + bottom_row * fy).round() as u8;
            }
            bgra[out + 3] = 255;
        }
    }
    Some(Owned {
        width: out_w,
        height: out_h,
        bgra,
    })
}

/// 每个像素的「墨」值：三个通道里最大的那个，**绿色再提一档**。
///
/// 数字是白的（255），括号是绿的 `(153,204,51)`：亮度只有 171，最大通道 204；
/// 底色是灰蓝的暗色（最大通道 70~120）。
///
/// 为什么绿色要单独提：括号 `[` 顶上那一个像素的小钩是整行最暗的墨，拉伸一糊
/// （和底色对半混）最大通道只剩 150 出头，掉到阈值下面 —— `[` 就退化成一根竖线，
/// 哪个字形都对不上，而结构解析**必须**认出这个括号（真机日志里「锚点找到了、
/// 数字也清楚，就是整帧作废」正是它）。绿得明显（G 比 R、B 都高出一截）的像素
/// 按括号的颜色归一到白字的量级，糊了一半也还在阈值之上。
fn ink(pixels: &Pixels<'_>, x: usize, y: usize) -> u8 {
    let offset = (y * pixels.width + x) * 4;
    let (b, g, r) = (
        pixels.bgra[offset] as u32,
        pixels.bgra[offset + 1] as u32,
        pixels.bgra[offset + 2] as u32,
    );
    let max = r.max(g).max(b);
    if g > r + 20 && g > b + 40 {
        max.max((g * 5 / 4).min(255)) as u8
    } else {
        max as u8
    }
}

/// 按「最大通道 + 逐行底色」二值化成白字黑底（给点阵字形用）。
///
/// 底色是**竖向渐变**的（从上到下 125 → 60），一个全局阈值要么切掉上面的字、
/// 要么把下面的底吃进来；每行各取底色（30 分位，字在一行里占不到一半）再定阈值。
fn binarize(strip: &Pixels<'_>) -> Option<Owned> {
    let (w, h) = (strip.width, strip.height);
    if w == 0 || h == 0 {
        return None;
    }
    let mut all: Vec<u8> = Vec::with_capacity(w * h);
    for y in 0..h {
        for x in 0..w {
            all.push(ink(strip, x, y));
        }
    }
    let mut sorted = all.clone();
    sorted.sort_unstable();
    let high = sorted[(sorted.len() * 97 / 100).min(sorted.len() - 1)] as f64;
    let mut bgra = vec![0u8; w * h * 4];
    for y in 0..h {
        let mut row: Vec<u8> = all[y * w..(y + 1) * w].to_vec();
        row.sort_unstable();
        let background = row[w * 30 / 100] as f64;
        // 这一行没有像样的对比度（整行都是底，或整行都是亮边框）：全当底
        let threshold = if high - background < 40.0 {
            f64::MAX
        } else {
            background + (high - background) * 0.45
        };
        for x in 0..w {
            let value = if all[y * w + x] as f64 > threshold {
                255u8
            } else {
                20u8
            };
            let offset = (y * w + x) * 4;
            bgra[offset] = value;
            bgra[offset + 1] = value;
            bgra[offset + 2] = value;
            bgra[offset + 3] = 255;
        }
    }
    Some(Owned {
        width: w,
        height: h,
        bgra,
    })
}

/// 在一块像素里读经验行（字形路径），把读数矩形映射回客户区。
fn read_glyphs(
    strip: &Pixels<'_>,
    font: &Font,
    tolerance: usize,
    map: impl Fn(f64, f64) -> (f64, f64),
) -> Option<font::Reading> {
    let reading = font::read_line_with(strip, font, tolerance)?;
    let (x0, y0, x1, y1) = reading.rect;
    let (cx0, cy0) = map(x0 as f64, y0 as f64);
    let (cx1, cy1) = map(x1 as f64, y1 as f64);
    Some(font::Reading {
        rect: (
            cx0.max(0.0).round() as usize,
            cy0.max(0.0).round() as usize,
            cx1.max(0.0).round() as usize,
            cy1.max(0.0).round() as usize,
        ),
        // 字高按**客户区像素**报（校准档的外扩、诊断都按它算）：
        // 读数矩形正好是一个 7 行的字格，映射回去的高度就是 7 × 缩放
        text_height: ((cy1 - cy0).round() as usize).max(1),
        ..reading
    })
}

/// 在 HUD 定位好的经验文字区读一次。
///
/// `pixels` 是抓回来的那一小块（左上角在客户区的 `origin`），可以是整幅客户区
/// （`origin = (0, 0)`），也可以是 [`HudLayout::read_strip`] 那一小条。
pub fn read_exp(
    pixels: &Pixels<'_>,
    origin: (i32, i32),
    layout: &HudLayout,
    font: &Font,
) -> Option<HudReading> {
    let a = layout.anchor;
    if a.sx <= 0.0 || a.sy <= 0.0 {
        return None;
    }
    let (tx, ty, tw, th) = layout.exp_text_exact();
    // 四周留一点（网页版给了 8%）：锚点有 ±1 像素误差，字形定左边界也要 `EXP` 标签
    let pad_x = 3.0 * a.sx;
    let pad_y = 2.0 * a.sy;
    let (ox, oy) = (origin.0 as f64, origin.1 as f64);
    let left = tx - pad_x - ox;
    let top = ty - pad_y - oy;
    let width = tw + pad_x * 2.0;
    let height = th + pad_y * 2.0;
    let mut best: Option<HudReading> = None;
    let consider = |candidate: Option<font::Reading>,
                    path: ReadPath,
                    best: &mut Option<HudReading>| {
        if let Some(reading) = candidate {
            let better = match best {
                None => true,
                Some(current) => font::better_reading(&reading, Some(&current.reading)),
            };
            if better {
                *best = Some(HudReading { reading, path });
            }
        }
    };
    let good = |best: &Option<HudReading>| {
        best.as_ref()
            .map(|b| font::reading_rank(&b.reading) == 0)
            .unwrap_or(false)
    };

    // ── 1. 1×：原样读（窗口化最常见，逐像素搬运，最快也最准）──────────
    if (a.sx - 1.0).abs() < 0.02 && (a.sy - 1.0).abs() < 0.02 {
        let x = left.round().max(0.0) as usize;
        let y = top.round().max(0.0) as usize;
        let (cw, ch, buffer) =
            pixels.crop(x, y, width.round() as usize, height.round() as usize);
        if cw >= 40 && ch >= font::CELL_HEIGHT {
            let crop = Pixels {
                width: cw,
                height: ch,
                bgra: &buffer,
            };
            let map = |px: f64, py: f64| (px + x as f64 + ox, py + y as f64 + oy);
            consider(
                read_glyphs(&crop, font, font::MATCH_TOLERANCE, map),
                ReadPath::Native,
                &mut best,
            );
            if !good(&best) {
                if let Some(binary) = binarize(&crop) {
                    consider(
                        read_glyphs(&binary.pixels(), font, 1, map),
                        ReadPath::Native,
                        &mut best,
                    );
                }
            }
        }
        if good(&best) {
            return best;
        }
    }

    // ── 2. 按缩放逐字比对点阵（无边框全屏：每个字各自最近邻放大）──────
    //
    // 只拿**数字那一段**：`EXP` 标签、底板渐变、经验条各画各的，和数字无关。
    if (a.sx - 1.0).abs() >= 0.02 || (a.sy - 1.0).abs() >= 0.02 {
        let (x0, y0, x1, y1) = digits_region(pixels, origin, layout);
        let (cw, ch, buffer) = pixels.crop(x0, y0, x1 - x0, y1 - y0);
        let crop = Pixels {
            width: cw,
            height: ch,
            bgra: &buffer,
        };
        let shift = |v: usize, base: usize, o: i32| v + base + o.max(0) as usize;
        let to_client = |r: font::Reading| {
            let (rx0, ry0, rx1, ry1) = r.rect;
            font::Reading {
                rect: (
                    shift(rx0, x0, origin.0),
                    shift(ry0, y0, origin.1),
                    shift(rx1, x0, origin.0),
                    shift(ry1, y0, origin.1),
                ),
                ..r
            }
        };
        let reading = read_scaled(&crop, (a.sx, a.sy), font).map(to_client);
        consider(reading, ReadPath::Scaled, &mut best);
        if good(&best) {
            return best;
        }
        // ── 2b. 灰度滑动比对（又被系统 DPI 缩放糊了一遍：字粘连、笔画粗细不一）──
        let reading = read_soft(&crop, (a.sx, a.sy), font).map(to_client);
        consider(reading, ReadPath::Soft, &mut best);
        if good(&best) {
            return best;
        }
    }

    // ── 3. 还原回原生像素再认字形（平滑缩放）──────────────────────────
    let native_w = (width / a.sx).round() as usize;
    let native_h = (height / a.sy).round() as usize;
    if native_w >= 40 && native_h >= font::CELL_HEIGHT {
        'phases: for phase_y in PHASES {
            for phase_x in PHASES {
                let strip_left = left + phase_x * a.sx;
                let strip_top = top + phase_y * a.sy;
                let Some(strip) =
                    resample(pixels, strip_left, strip_top, a.sx, a.sy, native_w, native_h)
                else {
                    continue;
                };
                let map = |px: f64, py: f64| {
                    (strip_left + px * a.sx + ox, strip_top + py * a.sy + oy)
                };
                consider(
                    read_glyphs(&strip.pixels(), font, font::MATCH_TOLERANCE, map),
                    ReadPath::Resampled,
                    &mut best,
                );
                if !good(&best) {
                    if let Some(binary) = binarize(&strip.pixels()) {
                        consider(
                            read_glyphs(&binary.pixels(), font, 2, map),
                            ReadPath::Resampled,
                            &mut best,
                        );
                    }
                }
                if good(&best) {
                    break 'phases;
                }
            }
        }
    }
    if good(&best) {
        return best;
    }

    // ── 4. PP-OCR 读原分辨率的数字区 ─────────────────────────────────
    //
    // 只喂**数字那一块**：从 `EXP` 标签右边到文字区右端、边框上方。标签和条的
    // 亮边框会让单行识别模型把两行并成一行（宽度也会超过模型的 320 上限被压扁）。
    if let Some(ocr) = read_ocr(pixels, origin, layout) {
        consider(Some(ocr), ReadPath::Ocr, &mut best);
    }

    // 经验表判「矛盾」的读数不交出去：交给上层的只能是自洽 / 多解的
    best.filter(|b| font::reading_rank(&b.reading) <= 1)
}

/// 数字那一段（`pixels` 里的坐标，半开区间 `x0..x1, y0..y1`）：
/// 从 `EXP` 标签右边到文字区右端、经验条上边框之上。
fn digits_region(
    pixels: &Pixels<'_>,
    origin: (i32, i32),
    layout: &HudLayout,
) -> (usize, usize, usize, usize) {
    let a = layout.anchor;
    let (tx, ty, tw, _) = layout.exp_text_exact();
    let (ox, oy) = (origin.0 as f64, origin.1 as f64);
    let x0 = (layout.exp_label_right() + 1.0 * a.sx - ox).max(0.0) as usize;
    let x1 = (tx + tw + 3.0 * a.sx - ox).clamp(0.0, pixels.width as f64) as usize;
    let y0 = (ty - 2.0 * a.sy - oy).max(0.0) as usize;
    // 条的上边框就在锚点那一行：停在它上面半个原生像素
    let y1 = (a.y - 0.5 * a.sy - oy).clamp(0.0, pixels.height as f64) as usize;
    (x0.min(x1), y0.min(y1), x1, y1)
}

/// 数字区 PP-OCR：紧贴墨迹裁切后交给单行识别模型。
fn read_ocr(
    pixels: &Pixels<'_>,
    origin: (i32, i32),
    layout: &HudLayout,
) -> Option<font::Reading> {
    let a = layout.anchor;
    let (x0, y0, x1, y1) = digits_region(pixels, origin, layout);
    if x1 < x0 + 8 || y1 < y0 + 4 {
        return None;
    }
    let (cw, ch, buffer) = pixels.crop(x0, y0, x1 - x0, y1 - y0);
    let crop = Pixels {
        width: cw,
        height: ch,
        bgra: &buffer,
    };
    let (bx0, by0, bx1, by1) = ink_bounds(&crop)?;
    // 紧贴墨迹，四周留字高的三成（网页版 `He` 同款）
    let text_h = by1 - by0 + 1;
    let margin = ((text_h as f64 * 0.3).ceil() as usize).max(2);
    let (cx0, cy0) = (bx0.saturating_sub(margin), by0.saturating_sub(margin));
    let (cx1, cy1) = ((bx1 + margin + 1).min(cw), (by1 + margin + 1).min(ch));
    let (tw2, th2, tight) = crop.crop(cx0, cy0, cx1 - cx0, cy1 - cy0);
    let tight = Pixels {
        width: tw2,
        height: th2,
        bgra: &tight,
    };
    let recognized = crate::exp::ocr::recognize_wide_line(&tight)?;
    let (exp, percent) = crate::exp::ocr::parse_exp_ocr(&recognized.text)?;
    Some(font::Reading {
        exp,
        percent,
        raw: format!("{} [OCR·缩放{:.2}]", recognized.text, a.sx),
        rect: (
            x0 + cx0 + origin.0.max(0) as usize,
            y0 + cy0 + origin.1.max(0) as usize,
            x0 + cx1 + origin.0.max(0) as usize,
            y0 + cy1 + origin.1.max(0) as usize,
        ),
        exact: 0,
        glyphs: 0,
        text_height: text_h,
    })
}

/// 墨迹的包围盒（最大通道高于逐行底色一截的像素）。
fn ink_bounds(crop: &Pixels<'_>) -> Option<(usize, usize, usize, usize)> {
    let binary = binarize(crop)?;
    let b = binary.pixels();
    let (mut x0, mut y0, mut x1, mut y1) = (usize::MAX, usize::MAX, 0usize, 0usize);
    for y in 0..b.height {
        for x in 0..b.width {
            if b.bgra[(y * b.width + x) * 4] > 128 {
                x0 = x0.min(x);
                y0 = y0.min(y);
                x1 = x1.max(x);
                y1 = y1.max(y);
            }
        }
    }
    (x0 != usize::MAX && x1 > x0 + 4 && y1 >= y0 + 2).then_some((x0, y0, x1, y1))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::exp::hud::{self, HudSkin};
    use crate::exp::table::{self, Verdict};

    /// 真机素材（240×41，1×）：`EXP 427096[46.09%]` + 经验条（46.09%）。
    /// EXP 条上边框左端在素材的 (87, 22)。
    const FIXTURE_W: usize = 240;
    const FIXTURE_H: usize = 41;
    const FIXTURE_BAR: (usize, usize) = (87, 22);

    fn fixture() -> Vec<u8> {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/exp_field_real_frame.bgra");
        std::fs::read(&path).expect("读不到真机画面素材")
    }

    /// 一张原生分辨率的游戏画面：有点纹理的世界 + HUD 骨架 + 真机那条经验行，
    /// 按网页版的几何关系贴在 EXP 条的位置上。返回 (BGRA, 锚点)。
    fn native_frame(width: usize, height: usize) -> (Vec<u8>, (usize, usize)) {
        let mut bgra = vec![0u8; width * height * 4];
        for y in 0..height {
            for x in 0..width {
                // 世界画面：斜条纹 + 渐变，别让它是一片纯色（纯色会让定位白捡便宜）
                let v = (40 + (x * 7 + y * 3) % 60 + y * 40 / height) as u8;
                let i = (y * width + x) * 4;
                bgra[i] = v / 2;
                bgra[i + 1] = v;
                bgra[i + 2] = v / 3;
                bgra[i + 3] = 255;
            }
        }
        // HUD 精灵贴底（39 高），锚点 = HP 条上边框左端 = 精灵里的 (465, 17)
        let anchor = (width / 2 - 682 + 465, height - 39 + 17);
        hud::paint_hud(
            &mut bgra,
            width,
            height,
            (anchor.0 as f64, anchor.1 as f64),
            1.0,
            HudSkin::Wide,
        );
        let strip = fixture();
        let ox = anchor.0 + 344 - FIXTURE_BAR.0;
        let oy = anchor.1 - FIXTURE_BAR.1;
        for y in 0..FIXTURE_H {
            for x in 0..FIXTURE_W {
                let (dx, dy) = (ox + x, oy + y);
                if dx < width && dy < height {
                    let src = (y * FIXTURE_W + x) * 4;
                    let dst = (dy * width + dx) * 4;
                    bgra[dst..dst + 4].copy_from_slice(&strip[src..src + 4]);
                }
            }
        }
        (bgra, anchor)
    }

    /// 把整幅画面拉伸到 `dw × dh` —— 模拟「游戏分辨率 < 显示器、全屏铺满」。
    /// `bilinear = false` 是最近邻（整数倍放大时有的渲染路径就是这样）。
    fn stretch(src: &[u8], sw: usize, sh: usize, dw: usize, dh: usize, bilinear: bool) -> Vec<u8> {
        let mut out = vec![0u8; dw * dh * 4];
        let fx = sw as f64 / dw as f64;
        let fy = sh as f64 / dh as f64;
        for v in 0..dh {
            for u in 0..dw {
                let dst = (v * dw + u) * 4;
                if !bilinear {
                    let x = (((u as f64 + 0.5) * fx) as usize).min(sw - 1);
                    let y = (((v as f64 + 0.5) * fy) as usize).min(sh - 1);
                    let src_i = (y * sw + x) * 4;
                    out[dst..dst + 4].copy_from_slice(&src[src_i..src_i + 4]);
                    continue;
                }
                let cx = ((u as f64 + 0.5) * fx - 0.5).clamp(0.0, (sw - 1) as f64);
                let cy = ((v as f64 + 0.5) * fy - 0.5).clamp(0.0, (sh - 1) as f64);
                let (x0, y0) = (cx.floor() as usize, cy.floor() as usize);
                let (x1, y1) = ((x0 + 1).min(sw - 1), (y0 + 1).min(sh - 1));
                let (ax, ay) = (cx - x0 as f64, cy - y0 as f64);
                for c in 0..4 {
                    let p = |x: usize, y: usize| src[(y * sw + x) * 4 + c] as f64;
                    let top = p(x0, y0) * (1.0 - ax) + p(x1, y0) * ax;
                    let bottom = p(x0, y1) * (1.0 - ax) + p(x1, y1) * ax;
                    out[dst + c] = (top * (1.0 - ay) + bottom * ay).round() as u8;
                }
            }
        }
        out
    }

    /// 定位 + 读数 + 经验条，一次走完（和 `ExpReader` 每个 tick 做的事一样）。
    fn locate_and_read(bgra: &[u8], width: usize, height: usize) -> (HudLayout, HudReading) {
        let pixels = Pixels {
            width,
            height,
            bgra,
        };
        let layout = hud::locate(&pixels)
            .unwrap_or_else(|| panic!("{width}×{height} 下必须定位到 HUD"));
        let font = Font::builtin();
        // 和产品路径一样：只抓那一小条来读
        let (sx, sy, sw, sh) = layout.read_strip(width as i32, height as i32);
        let (cw, ch, buffer) = pixels.crop(sx as usize, sy as usize, sw as usize, sh as usize);
        let strip = Pixels {
            width: cw,
            height: ch,
            bgra: &buffer,
        };
        assert!(
            hud::exp_label_visible(&strip, (sx, sy), &layout),
            "{width}×{height} 下每帧的 EXP 标签确认必须通过（缩放 {:.3}）",
            layout.anchor.sx
        );
        let reading = read_exp(&strip, (sx, sy), &layout, &font).unwrap_or_else(|| {
            panic!(
                "{width}×{height}（缩放 {:.3}×{:.3}）下必须读出经验行",
                layout.anchor.sx, layout.anchor.sy
            )
        });
        (layout, reading)
    }

    fn assert_reads_the_real_line(label: &str, reading: &HudReading) {
        assert_eq!(
            (reading.reading.exp, reading.reading.percent),
            (427_096, 46.09),
            "{label}：读成了「{}」（{}）",
            reading.reading.raw,
            reading.path.label()
        );
        assert_eq!(
            table::validate(reading.reading.exp, reading.reading.percent),
            Verdict::Known(55),
            "{label}：必须与经验表自洽"
        );
    }

    /// 1×（窗口化 768p / 1080p）：原样读，走最快的那条路。
    #[test]
    fn reads_the_real_line_at_native_scale() {
        for (w, h) in [(1366usize, 768usize), (1920, 1080)] {
            let (bgra, _) = native_frame(w, h);
            let (layout, reading) = locate_and_read(&bgra, w, h);
            assert!((layout.anchor.sx - 1.0).abs() < 0.01);
            assert_eq!(reading.path, ReadPath::Native, "{w}×{h} 应走原生字形");
            assert_reads_the_real_line(&format!("{w}×{h} 原生"), &reading);
        }
    }

    /// **这就是用户报的那个场景**：游戏 1366×768，全屏铺满 2560×1440（1.875 倍拉伸）。
    /// 以及 1920 宽 / 4K 显示器上的同类拉伸，双线性和最近邻两种渲染都要读得出。
    #[test]
    fn reads_the_real_line_when_fullscreen_stretches_the_game() {
        let cases: [((usize, usize), (usize, usize), bool); 6] = [
            ((1366, 768), (2560, 1440), true),  // 2K 显示器 · 游戏 768p · 全屏（用户现场）
            ((1366, 768), (1920, 1080), true),  // 1080p 显示器 · 游戏 768p · 全屏
            ((1366, 768), (2732, 1536), false), // 整数 2 倍（最近邻）
            ((1366, 768), (3840, 2160), true),  // 4K 显示器 · 游戏 768p · 全屏
            ((1920, 1080), (2560, 1440), true), // 2K 显示器 · 游戏 1080p · 全屏
            ((1920, 1080), (3840, 2160), true), // 4K 显示器 · 游戏 1080p · 全屏
        ];
        for ((nw, nh), (dw, dh), bilinear) in cases {
            let (native, _) = native_frame(nw, nh);
            let client = stretch(&native, nw, nh, dw, dh, bilinear);
            let label = format!(
                "{nw}×{nh} → {dw}×{dh}（{}）",
                if bilinear { "双线性" } else { "最近邻" }
            );
            let (layout, reading) = locate_and_read(&client, dw, dh);
            let expected = dw as f64 / nw as f64;
            assert!(
                (layout.anchor.sx - expected).abs() < expected * 0.01,
                "{label}：缩放量成 {:.4}，应为 {expected:.4}",
                layout.anchor.sx
            );
            assert_reads_the_real_line(&label, &reading);
        }
    }

    /// 经验条比例：和读数互相印证用，拉伸之后也要量得准。
    #[test]
    fn the_exp_bar_ratio_matches_the_percentage() {
        for (dw, dh) in [(1366usize, 768usize), (2560, 1440)] {
            let (native, _) = native_frame(1366, 768);
            let client = stretch(&native, 1366, 768, dw, dh, true);
            let pixels = Pixels {
                width: dw,
                height: dh,
                bgra: &client,
            };
            let layout = hud::locate(&pixels).expect("必须定位到 HUD");
            let bar = hud::exp_bar_ratio(&pixels, (0, 0), layout.exp_bar)
                .unwrap_or_else(|| panic!("{dw}×{dh} 下必须量出经验条"));
            assert!(
                (bar.ratio - 0.4609).abs() < 0.015,
                "{dw}×{dh} 下经验条比例 {:.4} 与 46.09% 差太多",
                bar.ratio
            );
            assert!(bar.confidence > 0.8, "三条扫描线应一致：{:?}", bar);
        }
    }

    /// 把素材里的数字换成任意一串（保留真机的 `EXP` 标签、渐变底色和经验条）：
    /// 白色数字用内置点阵，括号按真机的样子画成 9 行高的绿色 `[` `]`。
    fn repaint_digits(native: &mut [u8], width: usize, anchor: (usize, usize), text: &str) {
        let font = Font::builtin();
        // 素材里数字从 x=116 开始、顶在 y=10（EXP 条左端在 87、上边框在 22）
        let left = anchor.0 + 344 - FIXTURE_BAR.0;
        let top = anchor.1 - FIXTURE_BAR.1;
        let (x_start, y_text) = (left + 116, top + 10);
        // 先用每一行自己的底色抹掉原来的数字（底色横向是均匀的）
        for y in top + 9..top + 20 {
            let bg: [u8; 4] = native[(y * width + left + 5) * 4..(y * width + left + 5) * 4 + 4]
                .try_into()
                .unwrap();
            for x in x_start - 2..left + 238 {
                native[(y * width + x) * 4..(y * width + x) * 4 + 4].copy_from_slice(&bg);
            }
        }
        let put = |native: &mut [u8], x: usize, y: usize, rgb: [u8; 3]| {
            let i = (y * width + x) * 4;
            native[i..i + 4].copy_from_slice(&[rgb[2], rgb[1], rgb[0], 255]);
        };
        const GREEN: [u8; 3] = [153, 204, 51];
        let mut x = x_start;
        for ch in text.chars() {
            match ch {
                '[' | ']' => {
                    // 9 行：上下两行是两像素的横，中间一根竖线
                    let (stem, hook) = if ch == '[' { (x, x + 1) } else { (x + 1, x) };
                    for dy in 0..9 {
                        put(native, stem, y_text + dy, GREEN);
                    }
                    put(native, hook, y_text, GREEN);
                    put(native, hook, y_text + 8, GREEN);
                    x += 3;
                }
                _ => {
                    let bitmap = font.bitmap_of(ch).expect("字形表里应该有");
                    for (dy, row) in bitmap.to_ascii().split('|').enumerate() {
                        for (dx, cell) in row.chars().enumerate() {
                            if cell == '#' {
                                put(native, x + dx, y_text + dy, [255, 255, 255]);
                            }
                        }
                    }
                    x += bitmap.width() + 1;
                }
            }
        }
    }

    /// 不只认得素材那一串：含 `1`（2 像素宽、最容易糊掉）的、8 位数的高等级经验，
    /// 在原生 / 全屏拉伸下都要读对。
    #[test]
    fn reads_other_numbers_including_the_narrow_one() {
        // 等级 → 本级经验：百分比按游戏的显示（四舍五入到两位）
        let line = |level: u32, exp: u64| {
            let need = table::requirement(level).expect("等级要在表里");
            let percent = (exp as f64 / need as f64 * 10000.0).round() / 100.0;
            (format!("{exp}[{percent:.2}%]"), exp, percent)
        };
        let need_110 = table::requirement(110).expect("110 级要在表里");
        let cases = [
            line(20, 12_145),
            line(35, 111_111 % table::requirement(35).unwrap()),
            line(110, need_110 * 3 / 7),
        ];
        for (text, exp, percent) in cases {
            let (mut native, anchor) = native_frame(1366, 768);
            repaint_digits(&mut native, 1366, anchor, &text);
            for (dw, dh) in [(1366usize, 768usize), (2560, 1440), (1920, 1080), (3840, 2160)] {
                let client = if dw == 1366 {
                    native.clone()
                } else {
                    stretch(&native, 1366, 768, dw, dh, true)
                };
                let (_, reading) = locate_and_read(&client, dw, dh);
                assert_eq!(
                    (reading.reading.exp, reading.reading.percent),
                    (exp, percent),
                    "{text} @ {dw}×{dh}：读成了「{}」（{}）",
                    reading.reading.raw,
                    reading.path.label()
                );
            }
        }
    }

    /// 一个字的原生点阵（`true` = 墨）+ 颜色。括号是 9 行高的绿色 `[` `]`，
    /// 小数点带真机那样的半透明边（1× 素材里量到的颜色）。
    fn native_glyph(ch: char) -> (Vec<Vec<Option<[u8; 3]>>>, usize) {
        const WHITE: [u8; 3] = [255, 255, 255];
        const GREEN: [u8; 3] = [153, 204, 51];
        match ch {
            '[' | ']' => {
                let mut rows = vec![vec![None; 2]; 9];
                let (stem, hook) = if ch == '[' { (0, 1) } else { (1, 0) };
                for row in rows.iter_mut() {
                    row[stem] = Some(GREEN);
                }
                rows[0][hook] = Some(GREEN);
                rows[8][hook] = Some(GREEN);
                (rows, 2)
            }
            '.' => {
                let mut rows = vec![vec![None; 2]; 7];
                rows[4] = vec![Some([201, 201, 201]), Some([150, 150, 150])];
                rows[5] = vec![Some([250, 250, 250]), Some([191, 191, 191])];
                (rows, 2)
            }
            _ => {
                let bitmap = Font::builtin().bitmap_of(ch).expect("字形表里应该有").clone();
                let rows: Vec<Vec<Option<[u8; 3]>>> = bitmap
                    .to_ascii()
                    .split('|')
                    .map(|row| row.chars().map(|c| (c == '#').then_some(WHITE)).collect())
                    .collect();
                (rows, bitmap.width())
            }
        }
    }

    /// 按真机量到的方式画「全屏」：底图（标签、渐变、经验条）平滑拉伸，
    /// 数字那一串**每个字各自**按最近邻贴上去、各有各的随机相位。
    ///
    /// `stretched` 是已经平滑拉伸好的底图（每种分辨率只拉一次）。
    fn fullscreen_frame(
        stretched: &[u8],
        (nw, nh): (usize, usize),
        (dw, dh): (usize, usize),
        anchor: (usize, usize),
        text: &str,
        seed: &mut u64,
    ) -> Vec<u8> {
        let mut client = stretched.to_vec();
        let (sx, sy) = (dw as f64 / nw as f64, dh as f64 / nh as f64);
        let left = anchor.0 + 344 - FIXTURE_BAR.0;
        let top = anchor.1 - FIXTURE_BAR.1;
        let mut native_x = (left + 116) as f64;
        let native_y = (top + 10) as f64;
        let mut random = || {
            *seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            (*seed >> 33) as f64 / (1u64 << 31) as f64
        };
        for ch in text.chars() {
            let (rows, width) = native_glyph(ch);
            // 显卡贴图的光栅化：每个字是一个四边形，左上角落在**非整数**位置
            // （各自取整的误差不同）；像素中心落在四边形里才画，
            // 取的是 ⌊(像素中心 − 原点) / 缩放⌋ 那个原生像素（最近邻）
            let origin_x = native_x * sx + random() - 0.5;
            let origin_y = native_y * sy + random() - 0.5;
            let (quad_w, quad_h) = (width as f64 * sx, rows.len() as f64 * sy);
            for y in origin_y.floor() as i64..(origin_y + quad_h).ceil() as i64 + 1 {
                for x in origin_x.floor() as i64..(origin_x + quad_w).ceil() as i64 + 1 {
                    let (cx, cy) = (x as f64 + 0.5 - origin_x, y as f64 + 0.5 - origin_y);
                    if cx < 0.0 || cy < 0.0 || cx >= quad_w || cy >= quad_h {
                        continue;
                    }
                    let (i, j) = ((cx / sx).floor() as usize, (cy / sy).floor() as usize);
                    let Some(Some(rgb)) = rows.get(j).and_then(|row| row.get(i)) else {
                        continue;
                    };
                    let at = (y as usize * dw + x as usize) * 4;
                    client[at..at + 4].copy_from_slice(&[rgb[2], rgb[1], rgb[0], 255]);
                }
            }
            native_x += (width + 1) as f64;
        }
        client
    }

    fn debug_scaled(pixels: &Pixels<'_>, layout: &HudLayout) {
        let font = Font::builtin();
        let (x0, y0, x1, y1) = digits_region(pixels, (0, 0), layout);
        let (cw, ch, b2) = pixels.crop(x0, y0, x1 - x0, y1 - y0);
        let t = Pixels { width: cw, height: ch, bgra: &b2 };
        let mask = InkMask::from(&t).unwrap();
        let scale = (layout.anchor.sx, layout.anchor.sy);
        let band = (7.0 * scale.1).round() as usize;
        let top = (0..mask.height).find(|y| (0..mask.width).any(|x| mask.at(x as i64, *y as i64))).unwrap();
        for y in top.saturating_sub(1)..(top + band + 4).min(mask.height) {
            eprintln!("DBG {y:2} {}", (0..mask.width.min(200)).map(|x| if mask.at(x as i64, y as i64) {'#'} else {'.'}).collect::<String>());
        }
        let templates = scaled_templates(&font);
        let mut start = None;
        let mut blobs = vec![];
        for x in 0..=mask.width {
            let ink = x < mask.width && (top..top + band).any(|y| mask.at(x as i64, y as i64));
            match (ink, start) {
                (true, None) => start = Some(x),
                (false, Some(s)) => { blobs.push((s, x - 1)); start = None; }
                _ => {}
            }
        }
        for b in &blobs {
            let mut sc: Vec<(f64, char)> = templates.iter().filter_map(|(c, r)| best_mismatch(&mask, *b, (top, band), r, scale).map(|s| (s, *c))).collect();
            sc.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
            eprintln!("DBG blob {:?} w={} → {:?} => {:?}", b, b.1 - b.0 + 1, &sc[..sc.len().min(3)], match_or_split(&mask, *b, (top, band), scale, &templates));
        }
        eprintln!("DBG top {top} band {band} scale {scale:?}");
    }

    /// **全屏时好时坏的回归测试**：按真机的渲染方式，几十组随机经验值在
    /// 各档全屏拉伸下都必须逐字读对，而且走的是逐字比对（不是碰运气的 OCR）。
    #[test]
    fn fullscreen_per_glyph_rendering_reads_every_value() {
        let font = Font::builtin();
        let mut seed = 0x5eed_u64;
        let cases: [((usize, usize), (usize, usize)); 4] = [
            ((1366, 768), (2560, 1440)), // 用户现场
            ((1366, 768), (1920, 1080)),
            ((1366, 768), (3840, 2160)),
            ((1920, 1080), (2560, 1440)),
        ];
        for ((nw, nh), (dw, dh)) in cases {
            let (mut base, anchor) = native_frame(nw, nh);
            repaint_digits(&mut base, nw, anchor, "");
            let base = stretch(&base, nw, nh, dw, dh, true);
            // 定位只做一次（和产品一样按客户区尺寸缓存）
            let first = fullscreen_frame(&base, (nw, nh), (dw, dh), anchor, "1[0.00%]", &mut seed);
            let layout = hud::locate(&Pixels {
                width: dw,
                height: dh,
                bgra: &first,
            })
            .unwrap_or_else(|| panic!("{dw}×{dh} 下必须定位到 HUD"));
            let (mut scaled, mut total) = (0, 0);
            let mut slowest = std::time::Duration::ZERO;
            for _ in 0..25 {
                seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
                let level = 10 + (seed >> 40) as u32 % 100;
                let need = table::requirement(level).expect("等级要在表里");
                let exp = (seed >> 7) % need;
                let percent = (exp as f64 / need as f64 * 10000.0).round() / 100.0;
                let text = format!("{exp}[{percent:.2}%]");
                let client = fullscreen_frame(&base, (nw, nh), (dw, dh), anchor, &text, &mut seed);
                let pixels = Pixels {
                    width: dw,
                    height: dh,
                    bgra: &client,
                };
                let started = std::time::Instant::now();
                let read = read_exp(&pixels, (0, 0), &layout, &font)
                    .unwrap_or_else(|| panic!("{text} @ {dw}×{dh}：读不出"));
                slowest = slowest.max(started.elapsed());
                if read.path != ReadPath::Scaled {
                    debug_scaled(&pixels, &layout);
                }
                assert_eq!(
                    (read.reading.exp, read.reading.percent),
                    (exp, percent),
                    "{text} @ {dw}×{dh}：读成了「{}」（{}）",
                    read.reading.raw,
                    read.path.label()
                );
                total += 1;
                if read.path == ReadPath::Scaled {
                    scaled += 1;
                }
            }
            assert_eq!(scaled, total, "{dw}×{dh}：应全部由逐字比对读出（{scaled}/{total}）");
            println!("{dw}×{dh}：{total} 组全部读对，单次读数最慢 {slowest:?}");
        }
    }

    /// **真机素材**：2560×1440 无边框全屏（游戏 1366×768，缩放 1.872）抓下来的
    /// 经验那一条（333×76，左上角在客户区 (1504,1363)），画面上是 `16677[68.34%]`。
    ///
    /// 这张图上旧的字形路径和逆向采样都读不出（只有 OCR 偶尔读对），
    /// 逐字比对必须稳稳读对，而且每帧确认 HUD 的标签检查也得过。
    #[test]
    fn reads_the_real_fullscreen_capture() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/exp_strip_fullscreen_2560x1440.bgra");
        let strip = std::fs::read(&path).expect("读不到真机全屏素材");
        let pixels = Pixels {
            width: 333,
            height: 76,
            bgra: &strip,
        };
        // 真机那一帧整幅画面上定位出来的锚点（见 `HUD 锚点定位` 那行日志）
        let anchor = hud::HudAnchor {
            x: 872.0,
            y: 1399.0,
            sx: 1.872093023255814,
            sy: 1.872093023255814,
            error: 20.7,
            skin: HudSkin::Wide,
        };
        let layout = hud::layout_from_anchor(anchor, 2560, 1440);
        let origin = (1504, 1363);
        assert_eq!(layout.read_strip(2560, 1440), (1504, 1363, 333, 76));
        assert!(hud::exp_label_visible(&pixels, origin, &layout), "EXP 标签确认必须通过");
        let read = read_exp(&pixels, origin, &layout, &Font::builtin()).expect("必须读出");
        assert_eq!((read.reading.exp, read.reading.percent), (16_677, 68.34));
        assert_eq!(read.path, ReadPath::Scaled, "应由逐字比对读出，而不是靠 OCR");
        let bar = hud::exp_bar_ratio(&pixels, origin, layout.exp_bar).expect("经验条要量得出");
        assert!((bar.ratio - 0.6834).abs() < 0.03, "经验条 {:.4} 应与 68.34% 一致", bar.ratio);
    }

    /// **手动框选兜底**：没有 HUD 锚点（自动定位失败的机器上，用户自己框），
    /// 真机全屏素材整条当成一个手动框 —— 里面有标签、边框、经验条 —— 也要读对。
    #[test]
    fn a_manual_box_on_the_real_fullscreen_capture_reads() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/exp_strip_fullscreen_2560x1440.bgra");
        let strip = std::fs::read(&path).expect("读不到真机全屏素材");
        let pixels = Pixels {
            width: 333,
            height: 76,
            bgra: &strip,
        };
        let reading = read_unanchored(&pixels, &Font::builtin()).expect("手动框里必须读出");
        assert_eq!((reading.exp, reading.percent), (16_677, 68.34), "读成了 {}", reading.raw);
    }

    /// 手动框在各档全屏（含 4K）下：框得松（连经验条一起框进来）、框得紧（只框字）
    /// 都要读对。缩放是从框里的字行高度推出来的，不靠 HUD 锚点。
    #[test]
    fn manual_boxes_read_at_every_fullscreen_scale() {
        let font = Font::builtin();
        let mut seed = 0xb0c5_u64;
        let cases: [((usize, usize), (usize, usize)); 3] = [
            ((1366, 768), (3840, 2160)),
            ((1920, 1080), (3840, 2160)),
            ((1366, 768), (2560, 1440)),
        ];
        for ((nw, nh), (dw, dh)) in cases {
            let (mut base, anchor) = native_frame(nw, nh);
            repaint_digits(&mut base, nw, anchor, "");
            let base = stretch(&base, nw, nh, dw, dh, true);
            let (sx, sy) = (dw as f64 / nw as f64, dh as f64 / nh as f64);
            // 经验文字区（原生坐标）→ 客户区
            let text_x = (anchor.0 + 343) as f64 * sx;
            let text_y = (anchor.1 - 14) as f64 * sy;
            let boxes = [
                // 松：标签 + 文字 + 经验条
                (text_x - 12.0 * sx, text_y - 6.0 * sy, 190.0 * sx, 40.0 * sy),
                // 紧：只框数字那一行（左边离第一位数字留 3 个原生像素 ——
                // 读框时本来还会再外扩一圈；框正好切在数字上会被故意拒读，免得少读一位）
                (text_x + 27.0 * sx, text_y + 1.0 * sy, 130.0 * sx, 14.0 * sy),
            ];
            for _ in 0..6 {
                seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
                let level = 10 + (seed >> 40) as u32 % 100;
                let need = table::requirement(level).expect("等级要在表里");
                let exp = (seed >> 7) % need;
                let percent = (exp as f64 / need as f64 * 10000.0).round() / 100.0;
                let text = format!("{exp}[{percent:.2}%]");
                let client = fullscreen_frame(&base, (nw, nh), (dw, dh), anchor, &text, &mut seed);
                let pixels = Pixels {
                    width: dw,
                    height: dh,
                    bgra: &client,
                };
                for (bx, by, bw, bh) in boxes {
                    let (cw, ch, buffer) = pixels.crop(
                        bx as usize,
                        by as usize,
                        bw as usize,
                        bh as usize,
                    );
                    let crop = Pixels {
                        width: cw,
                        height: ch,
                        bgra: &buffer,
                    };
                    let reading = read_unanchored(&crop, &font).unwrap_or_else(|| {
                        panic!("{text} @ {dw}×{dh} 手动框 {bw:.0}×{bh:.0}：读不出")
                    });
                    assert_eq!(
                        (reading.exp, reading.percent),
                        (exp, percent),
                        "{text} @ {dw}×{dh} 手动框 {bw:.0}×{bh:.0}：读成了 {}",
                        reading.raw
                    );
                }
            }
        }
    }

    /// **4K + 系统 DPI 缩放**（朋友现场）：游戏不认高 DPI，全屏时它先按自己的分辨率
    /// 逐字最近邻放大（`native → mid`），Windows 再把整幅画面双线性拉伸到显示器
    /// （`mid → client`）。两次拉伸糊在一起，逐像素比对和逆向采样都不灵了。
    ///
    /// 要求：**一个都不能读错**（读错比读不出糟得多）；绝大多数帧要读出来，
    /// 而且主要靠灰度滑动比对，不是碰运气的 OCR。
    #[test]
    fn dpi_scaled_fullscreen_reads_without_misreads() {
        let font = Font::builtin();
        let cases: [((usize, usize), (usize, usize), (usize, usize)); 3] = [
            // 4K @150%：朋友日志里的「3840×2160 · 缩放 1.884」
            ((2048, 1152), (2560, 1440), (3840, 2160)),
            // 4K @150%，游戏 1080p
            ((1920, 1080), (2560, 1440), (3840, 2160)),
            // 2K @125%，游戏 768p
            ((1366, 768), (2048, 1152), (2560, 1440)),
        ];
        let mut seed = 0xd91_u64;
        for (native, mid, client) in cases {
            let (mut base, anchor) = native_frame(native.0, native.1);
            repaint_digits(&mut base, native.0, anchor, "");
            let mid_base = stretch(&base, native.0, native.1, mid.0, mid.1, true);
            let mut frame = stretch(&mid_base, mid.0, mid.1, client.0, client.1, true);
            // 只有 HUD 附近那几行会变：每一帧只重拉那一段（整幅 4K 在调试构建里太慢）
            let rows = client.1 * 88 / 100..client.1;
            let label = format!("{native:?}→{mid:?}→{client:?}");
            let mut layout: Option<HudLayout> = None;
            let (mut total, mut read, mut soft, mut manual) = (0, 0, 0, 0);
            // 手动框（松：标签 + 文字 + 经验条），和 `manual_boxes_read_at_every_fullscreen_scale` 一样
            let s = client.0 as f64 / native.0 as f64;
            let (text_x, text_y) = ((anchor.0 + 343) as f64 * s, (anchor.1 - 14) as f64 * s);
            let manual_box = (text_x - 12.0 * s, text_y - 6.0 * s, 190.0 * s, 40.0 * s);
            for _ in 0..8 {
                seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
                let level = 10 + (seed >> 40) as u32 % 100;
                let need = table::requirement(level).expect("等级要在表里");
                let exp = (seed >> 7) % need;
                let percent = (exp as f64 / need as f64 * 10000.0).round() / 100.0;
                let text = format!("{exp}[{percent:.2}%]");
                let game = fullscreen_frame(&mid_base, native, mid, anchor, &text, &mut seed);
                stretch_rows(&game, mid, &mut frame, client, rows.clone());
                let pixels = Pixels {
                    width: client.0,
                    height: client.1,
                    bgra: &frame,
                };
                let layout = layout.get_or_insert_with(|| {
                    hud::locate(&pixels).unwrap_or_else(|| panic!("{label}：必须定位到 HUD"))
                });
                total += 1;
                // 没有锚点的手动框：同样不许读错
                let (bx, by, bw, bh) = manual_box;
                let (cw, ch, buffer) =
                    pixels.crop(bx as usize, by as usize, bw as usize, bh as usize);
                let boxed = Pixels {
                    width: cw,
                    height: ch,
                    bgra: &buffer,
                };
                if let Some(reading) = read_unanchored(&boxed, &font) {
                    assert_eq!(
                        (reading.exp, reading.percent),
                        (exp, percent),
                        "{text} @ {label} 手动框：读成了「{}」",
                        reading.raw
                    );
                    manual += 1;
                }
                let Some(reading) = read_exp(&pixels, (0, 0), layout, &font) else {
                    continue;
                };
                assert_eq!(
                    (reading.reading.exp, reading.reading.percent),
                    (exp, percent),
                    "{text} @ {label}：读成了「{}」（{}）",
                    reading.reading.raw,
                    reading.path.label()
                );
                read += 1;
                if reading.path == ReadPath::Soft {
                    soft += 1;
                }
            }
            println!("{label}：{read}/{total} 读出，其中灰度滑动比对 {soft}；手动框 {manual}/{total}");
            assert!(read * 4 >= total * 3, "{label}：读出的太少（{read}/{total}）");
            assert!(soft * 2 >= total, "{label}：应主要靠灰度滑动比对（{soft}/{total}）");
        }
    }

    /// 只重拉目标画面的某几行（双线性，和 [`stretch`] 一样的采样）。
    fn stretch_rows(
        src: &[u8],
        (sw, sh): (usize, usize),
        dst: &mut [u8],
        (dw, dh): (usize, usize),
        rows: std::ops::Range<usize>,
    ) {
        let fx = sw as f64 / dw as f64;
        let fy = sh as f64 / dh as f64;
        for v in rows {
            let cy = ((v as f64 + 0.5) * fy - 0.5).clamp(0.0, (sh - 1) as f64);
            let y0 = cy.floor() as usize;
            let y1 = (y0 + 1).min(sh - 1);
            let ay = cy - y0 as f64;
            for u in 0..dw {
                let cx = ((u as f64 + 0.5) * fx - 0.5).clamp(0.0, (sw - 1) as f64);
                let x0 = cx.floor() as usize;
                let x1 = (x0 + 1).min(sw - 1);
                let ax = cx - x0 as f64;
                let dst_i = (v * dw + u) * 4;
                for c in 0..4 {
                    let p = |x: usize, y: usize| src[(y * sw + x) * 4 + c] as f64;
                    let top = p(x0, y0) * (1.0 - ax) + p(x1, y0) * ax;
                    let bottom = p(x0, y1) * (1.0 - ax) + p(x1, y1) * ax;
                    dst[dst_i + c] = (top * (1.0 - ay) + bottom * ay).round() as u8;
                }
            }
        }
    }

    /// 被别的东西盖住（面板 / 过场）：标签确认不过，不在旧位置上硬读。
    #[test]
    fn a_covered_hud_is_not_read_blindly() {
        let (mut bgra, _) = native_frame(1366, 768);
        let pixels = Pixels {
            width: 1366,
            height: 768,
            bgra: &bgra,
        };
        let layout = hud::locate(&pixels).expect("必须定位到 HUD");
        // 一块面板盖住经验那一段
        let (lx, ly, lw, lh) = layout.read_strip(1366, 768);
        for y in ly..ly + lh {
            for x in lx..lx + lw {
                let i = (y as usize * 1366 + x as usize) * 4;
                bgra[i..i + 4].copy_from_slice(&[230, 225, 220, 255]);
            }
        }
        let pixels = Pixels {
            width: 1366,
            height: 768,
            bgra: &bgra,
        };
        assert!(!hud::exp_label_visible(&pixels, (0, 0), &layout));
        assert!(read_exp(&pixels, (0, 0), &layout, &Font::builtin()).is_none());
    }
}
