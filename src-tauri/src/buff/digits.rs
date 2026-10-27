//! 读 BUFF 图标左下角的剩余时间（带黑边的小数字：白字是分钟，彩色字是秒，0–59）。
//!
//! 做法移植自网页版（枫记）的 `vision-glyphs.js` / `vision-glyph-resources.js` /
//! `vision-glyph-shape.js`：
//!
//! 1. 数字区：图标左边往外 0.25 个图标，到图标下沿再往下 0.2 个（数字会压在边框上）；
//! 2. **白顶帽**取前景（比周围亮、又比结构元细的笔画 —— 正是描边数字），
//!    阈值取最大值的 0.5 和 0.3 各试一遍（抗锯齿让不同笔画的亮度不一样）；
//! 3. 连通域：高度在图标的 0.22–0.48 之间、不比自己高还宽的才像一个字；
//!    第一个字要贴着图标左边，右边紧挨着、底边对齐、高度相近的算第二位；
//!    第二位右边如果还有像笔画的碎块，说明这次掩码把某个字切坏了 —— 整次作废，
//!    绝不把「15」只读成「1」；
//! 4. 每个字修掉零星毛刺（`glyph_core`），缩成 12×18，和参考字形比
//!    `2·点积 / 能量和`，再按宽高比、封闭孔数和孔的位置扣分；
//! 5. 两种阈值的结果取共识：字数多的优先，两个都「可用」却读成不同的数 → 不报。
//!
//! 参考字形是 `assets/buff-digit-glyphs.json`，由网页版的四份字形表合成：
//! 36 张灰度小图（启动时按 1 / 1.5 / 2 倍各取一次前景）+ 两百多个二值掩码。
//!
//! 门槛和网页版一样：最弱的那个字分数 ≥ 0.82、领先第二名（另一个数字）≥ 0.04。
//! 读不准就返回 `None` —— 宁可不报秒数，也不能报错的秒数（报错了提醒会早响 / 晚响）。
//!
//! 没搬的：网页版另有「只留彩色像素」的掩码（给彩色数字的 HUD 用）、放大重读、
//! 用模板背景证明碎块是图标花纹等补救路径。
//!
//! # 分钟还是秒：看字的颜色
//!
//! 游戏里剩一分钟以上时显示**白字的分钟数**，最后一分钟才是彩色的秒数（用户在真机上
//! 确认的）。网页版不看颜色、靠数字变化的快慢猜单位，结果整段分钟期都卡在「核对中」，
//! 还会把白字「1」当成 1 秒。这里取每个字最亮的那些笔画像素，看它们有多「彩」
//! （`(最大 - 最小) / 最大` 通道）：接近灰白就是分钟（`unit_of`）。

use super::image::{components, resize, Blob, Gray, Mask};
use serde::Deserialize;
use std::sync::OnceLock;

const GLYPH_BANK: &str = include_str!("../../assets/buff-digit-glyphs.json");
/// 比较时统一缩成的尺寸（网页版同款）。
const VEC_W: usize = 12;
const VEC_H: usize = 18;
/// 单字的分数 / 领先门槛。
pub const GLYPH_SCORE: f32 = 0.82;
pub const GLYPH_MARGIN: f32 = 0.04;
/// 顶帽之后的两档阈值（占最大值的比例）。
const THRESHOLDS: [f32; 2] = [0.5, 0.3];

#[derive(Deserialize)]
struct BankFile {
    gray: Vec<GrayGlyph>,
    masks: Vec<MaskGlyph>,
}

#[derive(Deserialize)]
struct GrayGlyph {
    digit: u8,
    width: usize,
    height: usize,
    pixels: Vec<u8>,
}

#[derive(Deserialize)]
struct MaskGlyph {
    digit: u8,
    width: usize,
    height: usize,
    /// 行优先的 `0` / `1`
    bits: String,
}

impl MaskGlyph {
    fn mask(&self) -> Mask {
        Mask {
            width: self.width,
            height: self.height,
            data: self.bits.bytes().map(|b| b == b'1').collect(),
        }
    }
}

struct Reference {
    digit: u8,
    ratio: f32,
    vector: Vec<f32>,
    energy: f32,
    holes: usize,
    /// 孔中心的平均高度；灰度参考在网页版里没记，不参与扣分
    void_y: Option<f32>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct Rect {
    x: usize,
    y: usize,
    width: usize,
    height: usize,
}

impl From<Blob> for Rect {
    fn from(b: Blob) -> Self {
        Self { x: b.x, y: b.y, width: b.width, height: b.height }
    }
}

fn reference(mask: &Mask, bounds: Rect, digit: u8, with_void: bool) -> Reference {
    let core = glyph_core(mask, bounds);
    let vector = vectorize(mask, core);
    let (holes, void_y) = glyph_holes(mask, core);
    Reference {
        digit,
        ratio: core.width as f32 / core.height as f32,
        energy: vector.iter().map(|v| v * v).sum(),
        vector,
        holes,
        void_y: with_void.then_some(void_y),
    }
}

fn references() -> &'static [Reference] {
    static REFS: OnceLock<Vec<Reference>> = OnceLock::new();
    REFS.get_or_init(|| {
        let bank: BankFile = match serde_json::from_str(GLYPH_BANK) {
            Ok(bank) => bank,
            Err(err) => {
                log::error!("BUFF 数字字形表解析失败：{err}");
                return Vec::new();
            }
        };
        let mut out = Vec::new();
        // 灰度参考：分数缩放会改变笔画在二值化之前的样子，所以先缩放再取前景
        for glyph in &bank.gray {
            let gray = Gray {
                width: glyph.width,
                height: glyph.height,
                data: glyph.pixels.iter().map(|&v| v as f32).collect(),
            };
            for s in [1.0f32, 1.5, 2.0] {
                let w = (glyph.width as f32 * s).round() as usize;
                let h = (glyph.height as f32 * s).round() as usize;
                let resized = resize(&gray, w, h);
                let size = ((3.0 * s).round() as usize | 1).max(3);
                let mask = Mask::foreground(&resized, size, 0.5, 24.0);
                for blob in components(&mask) {
                    if (blob.x as f32) < 6.0 * s
                        && (blob.height as f32) >= 5.0 * s
                        && (blob.width as f32) <= 8.0 * s
                    {
                        out.push(reference(&mask, blob.into(), glyph.digit, false));
                    }
                }
            }
        }
        for glyph in &bank.masks {
            let mask = glyph.mask();
            if mask.data.len() != glyph.width * glyph.height {
                continue;
            }
            let full = Rect { x: 0, y: 0, width: glyph.width, height: glyph.height };
            out.push(reference(&mask, full, glyph.digit, true));
        }
        out
    })
}

/// 去掉外沿上零星的毛刺（最多修两轮）。完整性判断仍然看原框。
fn glyph_core(mask: &Mask, mut r: Rect) -> Rect {
    for _ in 0..2 {
        if r.width < 3 || r.height < 5 {
            break;
        }
        let row = |y: usize| (r.x..r.x + r.width).filter(|&x| mask.on(x, y)).count() as f32;
        let col = |x: usize| (r.y..r.y + r.height).filter(|&y| mask.on(x, y)).count() as f32;
        let (w, h) = (r.width as f32, r.height as f32);
        let (top, bottom) = (r.y, r.y + r.height - 1);
        let (left, right) = (r.x, r.x + r.width - 1);
        if row(top) <= (w * 0.2).max(1.0) && row(top + 1) >= (row(top) * 2.5).max(3.0) {
            r.y += 1;
            r.height -= 1;
        } else if row(bottom) <= (w * 0.2).max(1.0)
            && row(bottom - 1) >= (row(bottom) * 2.5).max(3.0)
        {
            r.height -= 1;
        } else if col(left) <= (h * 0.12).max(1.0) && col(left + 1) >= (col(left) * 2.5).max(3.0) {
            r.x += 1;
            r.width -= 1;
        } else if col(right) <= (h * 0.12).max(1.0)
            && col(right - 1) >= (col(right) * 2.5).max(3.0)
        {
            r.width -= 1;
        } else {
            break;
        }
    }
    r
}

/// 框里封闭的空洞数（0、6、8、9 的圈）和它们中心的平均高度（0–1）。
fn glyph_holes(mask: &Mask, r: Rect) -> (usize, f32) {
    let (w, h) = (r.width, r.height);
    let on = |x: usize, y: usize| mask.on(r.x + x, r.y + y);
    let mut seen = vec![false; w * h];
    let (mut holes, mut center) = (0usize, 0.0f32);
    for start in 0..w * h {
        if seen[start] || on(start % w, start / w) {
            continue;
        }
        seen[start] = true;
        let mut todo = vec![start];
        let (mut edge, mut area, mut total_y) = (false, 0usize, 0.0f32);
        while let Some(i) = todo.pop() {
            let (cx, cy) = (i % w, i / w);
            area += 1;
            total_y += cy as f32 + 0.5;
            if cx == 0 || cy == 0 || cx == w - 1 || cy == h - 1 {
                edge = true;
            }
            let neighbors = [
                (cx.wrapping_sub(1), cy),
                (cx + 1, cy),
                (cx, cy.wrapping_sub(1)),
                (cx, cy + 1),
            ];
            for (nx, ny) in neighbors {
                if nx < w && ny < h && !seen[ny * w + nx] && !on(nx, ny) {
                    seen[ny * w + nx] = true;
                    todo.push(ny * w + nx);
                }
            }
        }
        if !edge && area as f32 >= (w as f32 * h as f32 * 0.01).max(2.0) {
            holes += 1;
            center += total_y / area as f32 / h as f32;
        }
    }
    (holes, if holes > 0 { center / holes as f32 } else { 0.0 })
}

/// 掩码里的一个框缩成 12×18 的 0–1 向量。
fn vectorize(mask: &Mask, r: Rect) -> Vec<f32> {
    let mut crop = Gray::new(r.width, r.height);
    for y in 0..r.height {
        for x in 0..r.width {
            crop.data[y * r.width + x] = if mask.on(r.x + x, r.y + y) { 1.0 } else { 0.0 };
        }
    }
    resize(&crop, VEC_W, VEC_H).data
}

/// 一个字的认读结果。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Glyph {
    pub digit: u8,
    pub score: f32,
    /// 比第二名（另一个数字）高出多少
    pub margin: f32,
}

/// 认一个字。宽高比和所有参考都对不上时返回 `None`。
fn classify(mask: &Mask, blob: Rect) -> Option<Glyph> {
    let core = glyph_core(mask, blob);
    let vector = vectorize(mask, core);
    let energy: f32 = vector.iter().map(|v| v * v).sum();
    let (holes, void_y) = glyph_holes(mask, core);
    let ratio = core.width as f32 / core.height as f32;
    let mut best = [None::<f32>; 10];
    for reference in references() {
        let aspect = (ratio / reference.ratio).ln().abs();
        if aspect > 0.45 {
            continue;
        }
        let dot: f32 = vector.iter().zip(&reference.vector).map(|(a, b)| a * b).sum();
        let void_penalty = match reference.void_y {
            Some(ref_y) if holes > 0 && reference.holes > 0 => (void_y - ref_y).abs() * 0.15,
            _ => 0.0,
        };
        let penalty = holes.abs_diff(reference.holes) as f32 * 0.06 + aspect * 0.08 + void_penalty;
        let score = (2.0 * dot / (energy + reference.energy).max(1e-6) - penalty).max(0.0);
        let slot = &mut best[reference.digit as usize];
        *slot = Some(slot.map_or(score, |s| s.max(score)));
    }
    let mut ranked: Vec<(u8, f32)> =
        (0..10u8).filter_map(|d| best[d as usize].map(|s| (d, s))).collect();
    ranked.sort_by(|a, b| b.1.total_cmp(&a.1));
    let (digit, score) = *ranked.first()?;
    let second = ranked.get(1).map_or(0.0, |r| r.1);
    Some(Glyph { digit, score, margin: score - second })
}

/// 一种阈值下读出的一个候选。
#[derive(Debug, Clone, PartialEq)]
struct Candidate {
    value: u32,
    glyphs: Vec<Glyph>,
    /// 每个字在画面里的框，用来比较两种阈值读到的是不是同一处
    boxes: Vec<Rect>,
    score: f32,
    usable: bool,
}

/// 数字的单位（看颜色定的）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unit {
    /// 彩色字：最后一分钟的秒数
    Seconds,
    /// 白字：剩一分钟以上时的分钟数
    Minutes,
}

/// 读出来的数。
#[derive(Debug, Clone, PartialEq)]
pub struct DigitRead {
    pub value: u32,
    pub unit: Unit,
    pub glyphs: Vec<Glyph>,
    /// 最弱那个字的分数
    pub score: f32,
}

/// 笔画的「彩度」低于这个算白字（分钟）。实测游戏截图：白字笔画是 `#ffffff`、边上带点
/// 蓝灰（彩度 0 ~ 0.13）；秒数是金黄 `#fcca02` 一类（彩度约 0.99）。取中间偏白的一侧，
/// 两边都留足余量。
const WHITE_CHROMA: f32 = 0.4;

/// 看字的颜色定单位：每个字框里取亮度在最亮那个的 75% 以上的像素（笔画芯，避开黑边
/// 和背景），算它们平均的 `(最大通道 - 最小通道) / 最大通道`。
fn unit_of(gray: &Gray, bgra: &[u8], boxes: &[Rect]) -> Option<Unit> {
    let (mut chroma, mut count) = (0.0f32, 0usize);
    for r in boxes {
        let pixels: Vec<(f32, usize)> = (r.y..r.y + r.height)
            .flat_map(|y| (r.x..r.x + r.width).map(move |x| (x, y)))
            .filter(|&(x, y)| x < gray.width && y < gray.height)
            .map(|(x, y)| (gray.at(x, y), y * gray.width + x))
            .collect();
        let brightest = pixels.iter().map(|p| p.0).fold(0.0f32, f32::max);
        for &(_, i) in pixels.iter().filter(|p| p.0 >= brightest * 0.75) {
            let px = bgra.get(i * 4..i * 4 + 3)?;
            let max = px.iter().copied().max().unwrap_or(0) as f32;
            let min = px.iter().copied().min().unwrap_or(0) as f32;
            if max > 0.0 {
                chroma += (max - min) / max;
                count += 1;
            }
        }
    }
    (count > 0).then(|| if chroma / (count as f32) < WHITE_CHROMA { Unit::Minutes } else { Unit::Seconds })
}

/// 一种阈值下读一次（网页版的 `readMask`）。
fn read_mask(frame: &Gray, x: usize, y: usize, size: usize, threshold: f32) -> Option<Candidate> {
    let s = size as f32;
    let (fx, fy) = (x as f32, y as f32);
    let left = (fx - s * 0.25).floor().max(0.0) as usize;
    let top = (fy + s * 0.45).floor().max(0.0) as usize;
    let right = ((fx + s * 0.8).ceil() as usize).min(frame.width);
    let bottom = ((fy + s * 1.2).ceil() as usize).min(frame.height);
    if right <= left || bottom <= top {
        return None;
    }
    let region = frame.crop(left as i64, top as i64, right - left, bottom - top);
    let kernel = ((s * 0.08).round() as usize | 1).max(3);
    let mask = Mask::foreground(&region, kernel, threshold, 24.0);
    let pieces = components(&mask);

    let parts: Vec<Blob> = pieces
        .iter()
        .copied()
        .filter(|b| {
            let h = b.height as f32;
            h >= s * 0.22 && h <= s * 0.48 && b.width <= b.height && b.y + b.height < mask.height
        })
        .collect();
    let mut candidates = Vec::new();
    for first in parts.iter().filter(|b| {
        let gx = (b.x + left) as f32;
        gx >= fx - s * 0.2 && gx <= fx + s * 0.1
    }) {
        let fh = first.height as f32;
        let first_end = first.x + first.width;
        let first_bottom = first.y + first.height;
        let second = parts
            .iter()
            .filter(|b| {
                b.x >= first_end
                    && (b.x - first_end) as f32 <= fh * 0.5
                    && ((b.y + b.height).abs_diff(first_bottom) as f32) <= (fh * 0.15).max(2.0)
                    && (b.height.abs_diff(first.height) as f32) <= (fh * 0.2).max(2.0)
            })
            .min_by_key(|b| b.x);
        let mut boxes = vec![*first];
        boxes.extend(second.copied());
        let Some(glyphs) =
            boxes.iter().map(|&b| classify(&mask, b.into())).collect::<Option<Vec<Glyph>>>()
        else {
            continue;
        };

        // 最后一个字右边还有像笔画的碎块：某个字被这次的掩码切坏了，这次不算完整
        let last = boxes[boxes.len() - 1];
        let last_end = last.x + last.width;
        let trailing = pieces.iter().any(|b| {
            let overlap = (b.y + b.height).min(first_bottom) as f32 - b.y.max(first.y) as f32;
            !boxes.contains(b)
                && b.x >= last_end
                && (b.x - last_end) as f32 <= fh * 0.5
                && b.height as f32 >= fh * 0.35
                && b.area as f32 >= (first.area as f32 * 0.15).max(3.0)
                && overlap >= fh * 0.4
        });
        // 只认出一个字，但第二位的位置上还有墨（第二个字被切碎或没切出来）：读成一位数是错的，这次不算
        let missing_second = boxes.len() == 1
            && pieces.iter().any(|b| {
                let overlap = (b.y + b.height).min(first_bottom) as f32 - b.y.max(first.y) as f32;
                !boxes.contains(b)
                    && b.x >= first_end
                    && (b.x - first_end) as f32 <= fh * 1.0
                    && b.area >= 3
                    && overlap >= fh * 0.3
            });
        let score = glyphs.iter().map(|g| g.score).fold(f32::MAX, f32::min);
        let margin = glyphs.iter().map(|g| g.margin).fold(f32::MAX, f32::min);
        candidates.push(Candidate {
            value: glyphs.iter().fold(0u32, |n, g| n * 10 + g.digit as u32),
            boxes: boxes
                .iter()
                .map(|b| Rect { x: b.x + left, y: b.y + top, width: b.width, height: b.height })
                .collect(),
            usable: !trailing && !missing_second && score >= GLYPH_SCORE && margin >= GLYPH_MARGIN,
            glyphs,
            score,
        });
    }
    candidates
        .into_iter()
        .max_by(|a, b| a.usable.cmp(&b.usable).then(a.score.total_cmp(&b.score)))
}

/// 两个框重叠得够多（IoU ≥ 0.65）就算同一个字。
fn same_place(a: Rect, b: Rect) -> bool {
    let w = (a.x + a.width).min(b.x + b.width) as f32 - a.x.max(b.x) as f32;
    let h = (a.y + a.height).min(b.y + b.height) as f32 - a.y.max(b.y) as f32;
    let inter = w.max(0.0) * h.max(0.0);
    let union = (a.width * a.height + b.width * b.height) as f32 - inter;
    union > 0.0 && inter / union >= 0.65
}

/// 读图标 `(x, y, size)` 左下角的数和它的单位。`frame` 是灰度画面，`bgra` 是同一帧的
/// 原始像素（定单位用）。读不准返回 `None`。
pub fn read_number(frame: &Gray, bgra: &[u8], x: usize, y: usize, size: usize) -> Option<DigitRead> {
    if size < 12 {
        return None;
    }
    let reads: Vec<Candidate> =
        THRESHOLDS.iter().filter_map(|&t| read_mask(frame, x, y, size, t)).collect();
    // 抗锯齿可能把一个字拆碎：任一阈值看到了完整的第二位，就不接受更短的读法
    let digits = reads.iter().map(|r| r.glyphs.len()).max()?;
    let mut variants: Vec<&Candidate> = reads.iter().filter(|r| r.glyphs.len() == digits).collect();
    variants.sort_by(|a, b| b.usable.cmp(&a.usable).then(b.score.total_cmp(&a.score)));
    let best = variants[0];
    if !best.usable {
        return None;
    }
    // 两种阈值都「可用」却读成不同的数（或读的不是同一处）：不报
    let conflict = variants.iter().any(|r| {
        r.usable
            && (r.value != best.value
                || r.boxes.iter().zip(&best.boxes).any(|(a, b)| !same_place(*a, *b)))
    });
    if conflict || best.value > 59 {
        return None;
    }
    let unit = unit_of(frame, bgra, &best.boxes)?;
    Some(DigitRead { value: best.value, unit, glyphs: best.glyphs.clone(), score: best.score })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// 真机样本的标注（`testdata/classic/labels.json`）：一张 170×36 的顶部条里每个 BUFF 的位置和读数。
    #[derive(Deserialize)]
    pub(crate) struct FrameLabel {
        pub file: String,
        pub slots: Vec<Slot>,
    }

    #[derive(Deserialize)]
    pub(crate) struct Slot {
        pub slot: usize,
        pub icon_left: usize,
        pub icon_top: usize,
        pub value: Option<u32>,
        pub unit: Option<String>,
    }

    impl Slot {
        /// 标注的读数和单位；没标（或单位不认识）返回 `None`。
        pub(crate) fn expect(&self) -> Option<(u32, Unit)> {
            let unit = match self.unit.as_deref()? {
                "Seconds" => Unit::Seconds,
                "Minutes" => Unit::Minutes,
                _ => return None,
            };
            Some((self.value?, unit))
        }
    }

    /// 1080p 真机样本：每张是图标顶部条（BGRA，宽、高）+ 标注。顺序就是 labels.json 的顺序。
    pub(crate) fn classic_fixtures() -> Vec<(FrameLabel, Vec<u8>, usize, usize)> {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/src/buff/testdata/classic");
        let labels: Vec<FrameLabel> =
            serde_json::from_str(&std::fs::read_to_string(format!("{dir}/labels.json")).unwrap()).unwrap();
        labels
            .into_iter()
            .map(|label| {
                let bytes = std::fs::read(format!("{dir}/{}", label.file)).unwrap();
                let (rgba, w, h) = super::super::decode_png_bytes(bytes).expect("样本 PNG 能解开");
                let bgra = rgba.chunks_exact(4).flat_map(|p| [p[2], p[1], p[0], 255]).collect();
                (label, bgra, w, h)
            })
            .collect()
    }

    /// 字形表里某个数字的二值掩码：取高度最接近 13 像素的（游戏原生大小的截图字）。
    fn glyph_mask(digit: u8) -> Mask {
        let bank: BankFile = serde_json::from_str(GLYPH_BANK).unwrap();
        bank.masks.iter().filter(|g| g.digit == digit).min_by_key(|g| g.height.abs_diff(13)).unwrap().mask()
    }

    /// 在 `(ox, oy)` 画一个游戏里那样的白字黑边数字（最近邻放大 `scale` 倍），返回字宽。
    ///
    /// 表里的掩码是网页版顶帽**之后**的样子；原样涂白的话，粗的拐角会凑出整块
    /// 比结构元还大的亮面、被顶帽减掉。所以涂之前把「周围全是墨」的内部像素挖空，
    /// 让笔画像游戏里那样细。
    pub(crate) fn paint_digit(frame: &mut Gray, digit: u8, ox: usize, oy: usize, scale: f32) -> usize {
        let glyph = glyph_mask(digit);
        let w = (glyph.width as f32 * scale).round() as usize;
        let h = (glyph.height as f32 * scale).round() as usize;
        let inked = |x: i64, y: i64| {
            x >= 0
                && y >= 0
                && (x as usize) < w
                && (y as usize) < h
                && glyph.on(
                    ((x as f32 / scale) as usize).min(glyph.width - 1),
                    ((y as f32 / scale) as usize).min(glyph.height - 1),
                )
        };
        // 和读数时的结构元一样大：图标 32 → 3，48 → 5
        let r = (((32.0 * scale * 0.08).round() as i64) | 1).max(3) / 2;
        let interior = |x: i64, y: i64| (-r..=r).all(|dy| (-r..=r).all(|dx| inked(x + dx, y + dy)));
        let on = |x: i64, y: i64| inked(x, y) && !interior(x, y);
        for y in -1..=h as i64 {
            for x in -1..=w as i64 {
                let (px, py) = ((ox as i64 + x) as usize, (oy as i64 + y) as usize);
                let i = py * frame.width + px;
                if on(x, y) {
                    frame.data[i] = 245.0;
                } else if (-1..=1).any(|dy| (-1..=1).any(|dx| on(x + dx, y + dy))) {
                    frame.data[i] = 15.0; // 黑边
                }
            }
        }
        w
    }

    /// 一张带花纹的暗图（假装是图标和它周围）。
    pub(crate) fn icon_frame() -> Gray {
        let mut g = Gray::new(160, 110);
        for (i, v) in g.data.iter_mut().enumerate() {
            *v = 60.0 + (i % 7) as f32 * 4.0 + ((i / 160) % 5) as f32 * 3.0;
        }
        g
    }

    /// 灰度画面上色成 BGRA：`bgr` 是每个通道占亮度的比例（白 = `[1, 1, 1]`）。
    /// 只给亮的笔画上色，暗的背景和黑边保持灰色，像游戏里那样。
    pub(crate) fn tint(frame: &Gray, bgr: [f32; 3]) -> Vec<u8> {
        frame
            .data
            .iter()
            .flat_map(|&v| {
                let c = |k: f32| if v > 150.0 { (v * k) as u8 } else { v as u8 };
                [c(bgr[0]), c(bgr[1]), c(bgr[2]), 255]
            })
            .collect()
    }

    /// 秒数的颜色（金黄字，游戏截图里量的 `#fcca02`，BGR 顺序）。
    pub(crate) const YELLOW: [f32; 3] = [0.01, 0.8, 1.0];
    const WHITE: [f32; 3] = [1.0, 1.0, 1.0];

    pub(crate) fn paint_number(frame: &mut Gray, value: u32, x: usize, y: usize, scale: f32) {
        let mut cx = x;
        for b in value.to_string().bytes() {
            cx += paint_digit(frame, b - b'0', cx, y, scale) + (2.0 * scale).round() as usize;
        }
    }

    /// 从真机样本重新生成字形表：`BUFF_WRITE_GLYPH_BANK=1 cargo test --lib regenerate_glyph_bank`。
    /// 平时什么都不做。每个标了读数的槽，按和 `read_mask` 一样的区域、阈值切字；切出的字数
    /// 和读数位数对不上的槽跳过（不猜）。
    #[test]
    fn regenerate_glyph_bank() {
        if std::env::var_os("BUFF_WRITE_GLYPH_BANK").is_none() {
            return;
        }
        const S: f32 = 32.0;
        let (mut gray, mut masks) = (Vec::new(), Vec::new());
        let (mut used, mut skipped) = (0usize, 0usize);
        for (label, bgra, w, h) in classic_fixtures() {
            let frame = Gray::from_bgra(&bgra, w, h);
            for slot in &label.slots {
                let Some((value, _)) = slot.expect() else { continue };
                let digits: Vec<u8> = value.to_string().bytes().map(|b| b - b'0').collect();
                let (fx, fy) = (slot.icon_left as f32, slot.icon_top as f32);
                let left = (fx - S * 0.25).floor().max(0.0) as usize;
                let top = (fy + S * 0.45).floor().max(0.0) as usize;
                let right = ((fx + S * 0.8).ceil() as usize).min(frame.width);
                let bottom = ((fy + S * 1.2).ceil() as usize).min(frame.height);
                let region = frame.crop(left as i64, top as i64, right - left, bottom - top);
                let mut taken = false;
                for (k, threshold) in THRESHOLDS.iter().enumerate() {
                    let mask = Mask::foreground(&region, 3, *threshold, 24.0);
                    let mut parts: Vec<Blob> = components(&mask)
                        .into_iter()
                        .filter(|b| {
                            let bh = b.height as f32;
                            bh >= S * 0.22 && bh <= S * 0.48 && b.width <= b.height && b.y + b.height < mask.height
                        })
                        .collect();
                    parts.sort_by_key(|b| b.x);
                    if parts.len() != digits.len() {
                        continue;
                    }
                    taken = true;
                    for (part, &digit) in parts.iter().zip(&digits) {
                        let bits: String = (0..part.height)
                            .flat_map(|yy| (0..part.width).map(move |xx| (part.x + xx, part.y + yy)))
                            .map(|(xx, yy)| if mask.on(xx, yy) { '1' } else { '0' })
                            .collect();
                        masks.push(serde_json::json!({
                            "digit": digit, "width": part.width, "height": part.height, "bits": bits,
                        }));
                        // 灰度参考只取一次（第一个阈值），四周留 2 像素
                        if k == 0 {
                            let (ax, ay) = (part.x + left, part.y + top);
                            let crop = frame.crop(ax as i64 - 2, ay as i64 - 2, part.width + 4, part.height + 4);
                            let pixels: Vec<u8> = crop.data.iter().map(|v| v.round().clamp(0.0, 255.0) as u8).collect();
                            gray.push(serde_json::json!({
                                "digit": digit, "width": crop.width, "height": crop.height, "pixels": pixels,
                            }));
                        }
                    }
                }
                if taken {
                    used += 1;
                } else {
                    skipped += 1;
                }
            }
        }
        let counts = (gray.len(), masks.len());
        let bank = serde_json::json!({ "gray": gray, "masks": masks });
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/assets/buff-digit-glyphs.json");
        std::fs::write(path, bank.to_string()).unwrap();
        eprintln!("字形表：用了 {used} 个槽，跳过 {skipped} 个；灰度字 {}，掩码 {}", counts.0, counts.1);
    }

    #[test]
    fn glyph_bank_loads_every_digit() {
        let refs = references();
        for d in 0..10u8 {
            assert!(refs.iter().any(|r| r.digit == d), "缺数字 {d}");
        }
    }

    /// 0–59 全扫一遍：**任何时候都不能读成别的数**（读不出可以，宁可不报）。返回读对的个数。
    ///
    /// 这里的字是拿字形表里的掩码画出来的假字，所以读对的比例只当参考：字形表是 1080p 真机样本
    /// 重新生成的（见 `regenerate_glyph_bank`），这里画的字和样本不完全一样，读不出的会多一些。
    fn sweep(scale: f32, size: usize, y: usize) -> usize {
        let mut correct = 0;
        for value in 0..60u32 {
            let mut frame = icon_frame();
            paint_number(&mut frame, value, 61, y + size * 9 / 16, scale);
            let read = read_number(&frame, &tint(&frame, YELLOW), 60, y, size).map(|r| {
                assert_eq!(r.unit, Unit::Seconds, "{value} 是黄字，应该是秒");
                r.value
            });
            assert!(read.is_none() || read == Some(value), "{value} 读成了 {read:?}");
            correct += usize::from(read == Some(value));
        }
        eprintln!("合成数字 scale {scale}：读对 {correct}/60");
        correct
    }

    #[test]
    fn reads_every_value_at_native_scale() {
        assert!(sweep(1.0, 32, 30) >= 30, "原生大小读对的太少");
    }

    #[test]
    fn reads_every_value_on_a_larger_display_scale() {
        assert!(sweep(1.5, 48, 20) >= 25, "1.5 倍读对的太少");
    }

    #[test]
    fn blank_corner_reads_nothing() {
        let frame = icon_frame();
        assert_eq!(read_number(&frame, &tint(&frame, WHITE), 60, 30, 32), None);
    }

    /// 白字是分钟、彩色字是秒（同一个数字）。找一个两种颜色都读得出的数来比。
    #[test]
    fn white_digits_are_minutes_and_coloured_ones_are_seconds() {
        let mut both = 0;
        for value in 0..60u32 {
            let mut frame = icon_frame();
            paint_number(&mut frame, value, 61, 30 + 18, 1.0);
            let white = read_number(&frame, &tint(&frame, WHITE), 60, 30, 32);
            let yellow = read_number(&frame, &tint(&frame, YELLOW), 60, 30, 32);
            if let (Some(w), Some(y)) = (white, yellow) {
                assert_eq!((w.value, w.unit), (value, Unit::Minutes), "{value} 白字");
                assert_eq!((y.value, y.unit), (value, Unit::Seconds), "{value} 黄字");
                both += 1;
            }
        }
        assert!(both >= 10, "两种颜色都读得出的太少：{both}");
    }
}
