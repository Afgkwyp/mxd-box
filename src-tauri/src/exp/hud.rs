//! HUD 锚点定位 —— **移植自网页版枫记（fj.need.run）的 mxdc 预设**。
//!
//! # 为什么不能用「按分辨率比例猜」
//!
//! 校准存成比例坐标看起来很美，但比例只在「游戏界面严格等比缩放」时成立。
//! 真机上至少三种情况会让它失效：
//!
//! * UI 缩放有档位（1.25×/1.5× 这类），与分辨率之比不是同一个数；
//! * 窗口化时客户区是任意尺寸（不一定 16:9）；
//! * Windows 显示缩放（150% 这类）会让画面整体被系统拉伸。
//!
//! 网页版枫记的做法是**确定性**的，跟缩放无关：
//!
//! 1. 找血条的**亮色边框**——每一行扫出亮的横向长条（长度 70~350px）；
//! 2. 三个条（HP/MP/EXP）之间的水平距离是固定的 `[0,168,344]`（参考坐标系），
//!    从任意两个条的距离反推出**缩放** `sx`；
//! 3. 按 `sx` 把 **HP/MP/EXP 标签的像素模板**缩放后逐点比对（带相关度与容差），
//!    对得上才算锚点；
//! 4. 找不到就在 1/2、1/4 的降采样图上再找一遍（多尺度金字塔）。
//!
//! 锚点一定住，经验行/等级框就是「锚点 + 固定偏移 × 缩放」，**任何分辨率、
//! 任何显示缩放都不需要用户校准**。手动校准只作为兜底。
//!
//! 模板（`assets/mxdc-hud-templates.json`）直接从网页版的
//! `layout-templates.json` 抠出来（HP/MP/EXP 标签的像素值），保证两边看到的是
//! 同一套东西。

use crate::exp::font::Pixels;
use serde::Deserialize;
use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::OnceLock;

const TEMPLATES_JSON: &str = include_str!("../../assets/mxdc-hud-templates.json");

// ---------------------------------------------------------------------------
// 参考坐标系（网页版 `layout-regions.ts`）
// ---------------------------------------------------------------------------

/// HUD 锚点 = HP 条**顶边框左端**在参考坐标系里的位置
/// （参考坐标系是 1364×39 的 HUD 精灵，见网页版的 `layout-regions.ts`）。
const ANCHOR_X: f64 = 465.0;
const ANCHOR_Y: f64 = 17.0;
/// 三个条左端相对 HP 条的参考偏移。
const BAR_OFFSETS: [f64; 3] = [0.0, 168.0, 344.0];
/// 条边框的参考宽度。
const BAR_WIDTH: f64 = 165.0;
/// 上/下边框之间的参考距离。
const BORDER_GAP: f64 = 17.0;
/// 标签相对锚点的 y 偏移（标签画在边框上方）。
const LABEL_DY: f64 = 13.0;
/// 经验文字（`EXP xx(xx.xx%)` 那一行）在参考坐标系里的矩形。
const EXP_TEXT: (f64, f64, f64, f64) = (808.0, 3.0, 167.0, 16.0);
/// 等级区（`Lv.xx`）在参考坐标系里的矩形。
const LEVEL_BOX: (f64, f64, f64, f64) = (8.0, 7.0, 86.0, 28.0);
/// 职业 / 角色名那两行字（等级右边：上一行职业、下一行角色名）。
///
/// 1080p 宽版真机量的：字从 x=110 起，职业占 y 7..17、角色名占 y 20..31；
/// 右边一直留到 HP 标签前的分隔线（x≈451），名字再长也到不了那儿。
const JOB_TEXT: (f64, f64, f64, f64) = (104.0, 4.0, 340.0, 15.0);
const NAME_TEXT: (f64, f64, f64, f64) = (104.0, 18.0, 340.0, 15.0);

/// 紧凑皮肤（短血条，网页版 `D`）：1280×37 的 HUD 精灵，三个条宽度不一。
///
/// 宽版一个都找不到时才找它（和网页版的顺序一样）。
const COMPACT_OFFSETS: [f64; 3] = [0.0, 108.0, 221.0];
const COMPACT_WIDTHS: [f64; 3] = [105.0, 105.0, 115.0];
const COMPACT_BORDER_GAP: f64 = 15.0;
const COMPACT_BAR_HEIGHT: f64 = 12.0;
const COMPACT_ANCHOR_X: f64 = 220.0;
const COMPACT_ANCHOR_Y: f64 = 18.0;
const COMPACT_LEVEL_BOX: (f64, f64, f64, f64) = (7.0, 7.0, 72.0, 28.0);

/// 经验条填充色（网页版 mxdc 预设 `bars.exp.colors`）与 RGB 欧氏距离容差。
///
/// 真机素材上满格那一段是 `(228,254,2)` → 底部渐暗到 `(125,139,1)`，
/// 空的那一段是灰白 `(204,204,204)`（距离 190+，不会误认）。
const EXP_BAR_COLORS: [[f64; 3]; 2] = [[205.0, 225.0, 12.0], [143.0, 199.0, 15.0]];
const EXP_BAR_TOLERANCE: f64 = 82.0;

/// 每帧确认「HUD 还在原地」时，经验标签模板误差的上限（网页版 `validBar` 同量级）。
const LABEL_VISIBLE_ERROR: f64 = 50.0;

/// HUD 皮肤（网页版的 `hudVariant`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HudSkin {
    /// 1364 宽的 HUD：三个条等宽 165，间距 `[0,168,344]`。
    Wide,
    /// 1280 宽的 HUD：条宽 `[105,105,115]`，间距 `[0,108,221]`。
    Compact,
}

impl HudSkin {
    pub fn label(self) -> &'static str {
        match self {
            HudSkin::Wide => "宽版",
            HudSkin::Compact => "紧凑版",
        }
    }

    fn offsets(self) -> [f64; 3] {
        match self {
            HudSkin::Wide => BAR_OFFSETS,
            HudSkin::Compact => COMPACT_OFFSETS,
        }
    }

    fn widths(self) -> [f64; 3] {
        match self {
            HudSkin::Wide => [BAR_WIDTH; 3],
            HudSkin::Compact => COMPACT_WIDTHS,
        }
    }

    fn border_gap(self) -> f64 {
        match self {
            HudSkin::Wide => BORDER_GAP,
            HudSkin::Compact => COMPACT_BORDER_GAP,
        }
    }

    /// 标签左端在条左端左边多少（参考像素）。
    fn label_dx(self) -> f64 {
        match self {
            HudSkin::Wide => 1.0,
            HudSkin::Compact => 2.0,
        }
    }
}

/// 亮条扫描用的阈值组（网页版实测：不同底色/亮度都要照顾到）。
const LIGHT_THRESHOLDS: [u8; 4] = [175, 140, 110, 205];
/// 条宽与 `165 × sx` 的最大误差。
const WIDTH_TOLERANCE: f64 = 4.0;
/// 缩放上限（网页版同款）。
const MAX_SCALE: f64 = 4.06;
const DETECT_MAX_SCALE: f64 = 2.06;
/// 低于这个缩放走「undersized」分支（4K 之上才可能用到）。
const MIN_SCALE: f64 = 0.46;

// ---------------------------------------------------------------------------
// 模板
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct RawTemplate {
    width: usize,
    height: usize,
    /// 每项是 `[x, y, r, g, b]`。
    pixels: Vec<[i64; 5]>,
}

#[derive(Deserialize)]
struct RawPack {
    wide: HashMap<String, RawTemplate>,
    compact: HashMap<String, RawTemplate>,
}

#[derive(Clone)]
struct Template {
    width: usize,
    height: usize,
    /// 行优先的 RGB。
    rgb: Vec<[f64; 3]>,
}

fn to_template(raw: &RawTemplate) -> Template {
    let mut rgb = vec![[0.0f64; 3]; raw.width * raw.height];
    for entry in &raw.pixels {
        let (x, y) = (entry[0] as usize, entry[1] as usize);
        if x < raw.width && y < raw.height {
            rgb[y * raw.width + x] = [entry[2] as f64, entry[3] as f64, entry[4] as f64];
        }
    }
    Template {
        width: raw.width,
        height: raw.height,
        rgb,
    }
}

/// 三个标签模板 + 稳定 id（缩放缓存按 id 索引）。
#[derive(Clone)]
struct Labels {
    templates: [Template; 3],
    ids: [usize; 3],
}

impl Labels {
    fn from_raw(map: &HashMap<String, RawTemplate>, base_id: usize) -> Option<Self> {
        let hp = to_template(map.get("hp")?);
        let mp = to_template(map.get("mp")?);
        let exp = to_template(map.get("exp")?);
        Some(Self {
            templates: [hp, mp, exp],
            ids: [base_id, base_id + 1, base_id + 2],
        })
    }
}

struct Pack {
    wide: Labels,
    compact: Labels,
    /// 小地图图标（39×14）—— 地图名区域的锚点。
    minimap: Option<(usize, Template)>,
}

fn pack() -> Option<&'static Pack> {
    static PACK: OnceLock<Option<Pack>> = OnceLock::new();
    PACK.get_or_init(|| {
        let raw: RawPack = serde_json::from_str(TEMPLATES_JSON).ok()?;
        let wide = Labels::from_raw(&raw.wide, 0)?;
        // 紧凑皮肤（短血条）的模板：没有就用宽版顶替（只是兜底路径）。
        let compact = Labels::from_raw(&raw.compact, 3).unwrap_or_else(|| wide.clone());
        let minimap = raw
            .wide
            .get("minimap")
            .map(|t| (6usize, to_template(t)));
        Some(Pack {
            wide,
            compact,
            minimap,
        })
    })
    .as_ref()
}

impl Pack {
    fn labels(&self, skin: HudSkin) -> &Labels {
        match skin {
            HudSkin::Wide => &self.wide,
            HudSkin::Compact => &self.compact,
        }
    }
}

// ---------------------------------------------------------------------------
// 一帧图像（BGRA）
// ---------------------------------------------------------------------------

struct Img<'a> {
    width: usize,
    height: usize,
    data: Cow<'a, [u8]>,
}

impl<'a> Img<'a> {
    fn wrap(pixels: &'a Pixels<'_>) -> Self {
        Self {
            width: pixels.width,
            height: pixels.height,
            data: Cow::Borrowed(pixels.bgra),
        }
    }

    /// 2×2 平均降采样（网页版的 `halfImage`）。
    fn half(&self) -> Img<'static> {
        let width = self.width.div_ceil(2);
        let height = self.height.div_ceil(2);
        let mut data = vec![0u8; width * height * 4];
        for y in 0..height {
            for x in 0..width {
                for channel in 0..4 {
                    let mut sum = 0u32;
                    for dy in 0..2 {
                        for dx in 0..2 {
                            let sy = (y * 2 + dy).min(self.height - 1);
                            let sx = (x * 2 + dx).min(self.width - 1);
                            sum += self.data[(sy * self.width + sx) * 4 + channel] as u32;
                        }
                    }
                    data[(y * width + x) * 4 + channel] = (sum / 4) as u8;
                }
            }
        }
        Img {
            width,
            height,
            data: Cow::Owned(data),
        }
    }

    fn r(&self, x: usize, y: usize) -> f64 {
        self.data[(y * self.width + x) * 4 + 2] as f64
    }
    fn g(&self, x: usize, y: usize) -> f64 {
        self.data[(y * self.width + x) * 4 + 1] as f64
    }
    fn b(&self, x: usize, y: usize) -> f64 {
        self.data[(y * self.width + x) * 4] as f64
    }
    /// 网页版同款灰度：`(r + 2g + b) / 4`。
    fn gray(&self, x: usize, y: usize) -> f64 {
        (self.r(x, y) + self.g(x, y) * 2.0 + self.b(x, y)) / 4.0
    }
    fn rgb(&self, x: usize, y: usize) -> [f64; 3] {
        [self.r(x, y), self.g(x, y), self.b(x, y)]
    }
}

// ---------------------------------------------------------------------------
// 模板缩放 + 误差
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct Scaled {
    w: usize,
    h: usize,
    rgb: Vec<[f64; 3]>,
}

struct Matcher {
    cache: HashMap<(usize, usize, usize, u8), Scaled>,
}

impl Matcher {
    fn new() -> Self {
        Self {
            cache: HashMap::new(),
        }
    }

    /// 把模板缩放到 `sx × sy`（网页版 `scaled`：bilinear；`nearest` 是亚像素相位变体）。
    fn scaled(&mut self, id: usize, t: &Template, sx: f64, sy: f64, nearest: u8) -> &Scaled {
        let w = ((t.width as f64 * sx).round() as usize).max(3);
        let h = ((t.height as f64 * sy).round() as usize).max(3);
        let key = (id, w, h, nearest);
        if !self.cache.contains_key(&key) {
            let mut rgb = vec![[0.0f64; 3]; w * h];
            for oy in 0..h {
                for ox in 0..w {
                    if nearest > 0 {
                        let nx = (((ox as f64 + 0.5) / sx - 0.5)
                            + ((nearest as i64 - 1) % 3) as f64 * 0.5)
                            .floor()
                            .clamp(0.0, (t.width - 1) as f64) as usize;
                        let ny = (((oy as f64 + 0.5) / sy - 0.5)
                            + ((nearest as i64 - 1) / 3) as f64 * 0.5)
                            .floor()
                            .clamp(0.0, (t.height - 1) as f64) as usize;
                        rgb[oy * w + ox] = t.rgb[ny * t.width + nx];
                        continue;
                    }
                    let tx = ((ox as f64 + 0.5) * t.width as f64 / w as f64 - 0.5).max(0.0);
                    let ty = ((oy as f64 + 0.5) * t.height as f64 / h as f64 - 0.5).max(0.0);
                    let x0 = tx.floor() as usize;
                    let y0 = ty.floor() as usize;
                    let x1 = (x0 + 1).min(t.width - 1);
                    let y1 = (y0 + 1).min(t.height - 1);
                    let fx = tx - x0 as f64;
                    let fy = ty - y0 as f64;
                    for c in 0..3 {
                        let top = t.rgb[y0 * t.width + x0][c] * (1.0 - fx)
                            + t.rgb[y0 * t.width + x1][c] * fx;
                        let bottom = t.rgb[y1 * t.width + x0][c] * (1.0 - fx)
                            + t.rgb[y1 * t.width + x1][c] * fx;
                        rgb[oy * w + ox][c] = top * (1.0 - fy) + bottom * fy;
                    }
                }
            }
            // 缓存别无限涨（网页版也是 512 条上限）
            if self.cache.len() > 512 {
                self.cache.clear();
            }
            self.cache.insert(key, Scaled { w, h, rgb });
        }
        &self.cache[&key]
    }

    /// 一次模板比对（网页版 `patchVariantError`）：平均通道差 + 相关度惩罚。
    fn patch_variant_error(
        &mut self,
        frame: &Img<'_>,
        id: usize,
        t: &Template,
        x: f64,
        y: f64,
        sx: f64,
        sy: f64,
        nearest: u8,
    ) -> f64 {
        let a = self.scaled(id, t, sx, sy, nearest);
        let left = x.round();
        let top = y.round();
        if left < 0.0
            || top < 0.0
            || left + a.w as f64 > frame.width as f64
            || top + a.h as f64 > frame.height as f64
        {
            return 255.0;
        }
        let (lx, ty) = (left as usize, top as usize);
        let mut error = 0.0f64;
        let (mut sum_a, mut sum_b) = (0.0f64, 0.0f64);
        let (mut aa, mut bb, mut ab) = (0.0f64, 0.0f64, 0.0f64);
        let mut n = 0usize;
        for dy in 0..a.h {
            for dx in 0..a.w {
                let frame_rgb = frame.rgb(lx + dx, ty + dy);
                let tpl = a.rgb[dy * a.w + dx];
                for c in 0..3 {
                    error += (tpl[c] - frame_rgb[c]).abs() / 3.0;
                }
                let av = (tpl[0] + tpl[1] * 2.0 + tpl[2]) / 4.0;
                let bv = (frame_rgb[0] + frame_rgb[1] * 2.0 + frame_rgb[2]) / 4.0;
                sum_a += av;
                sum_b += bv;
                aa += av * av;
                bb += bv * bv;
                ab += av * bv;
                n += 1;
                if n >= 12 && error / n as f64 > 85.0 {
                    return 255.0;
                }
            }
        }
        if n == 0 {
            return 255.0;
        }
        let nf = n as f64;
        let variance =
            ((aa - sum_a * sum_a / nf).max(0.0) * (bb - sum_b * sum_b / nf).max(0.0)).sqrt();
        let correlation = if variance > 1.0 {
            (ab - sum_a * sum_b / nf) / variance
        } else {
            0.0
        };
        if correlation < 0.15 {
            return 255.0;
        }
        error / nf * 0.6 + (1.0 - correlation) * 35.0
    }

    /// 网页版 `patchError`：小尺度时多试几个亚像素相位变体。
    #[allow(clippy::too_many_arguments)]
    fn patch_error(
        &mut self,
        frame: &Img<'_>,
        id: usize,
        t: &Template,
        x: f64,
        y: f64,
        sx: f64,
        sy: f64,
    ) -> f64 {
        let smooth = self.patch_variant_error(frame, id, t, x, y, sx, sy, 0);
        if smooth < 35.0 || (sx >= 1.0 && sy >= 1.0) {
            return smooth;
        }
        let mut best = smooth;
        for variant in 1..=9u8 {
            best = best.min(self.patch_variant_error(frame, id, t, x, y, sx, sy, variant));
            if best < 25.0 {
                break;
            }
        }
        best
    }

    /// 网页版 `refined`：在 ±radius 范围内取最小误差。
    #[allow(clippy::too_many_arguments)]
    fn refined(
        &mut self,
        frame: &Img<'_>,
        id: usize,
        t: &Template,
        x: f64,
        y: f64,
        sx: f64,
        sy: f64,
        radius: i32,
    ) -> f64 {
        let mut best = 255.0f64;
        for dy in -radius..=radius {
            for dx in -radius..=radius {
                best = best.min(self.patch_error(
                    frame,
                    id,
                    t,
                    x + dx as f64,
                    y + dy as f64,
                    sx,
                    sy,
                ));
            }
        }
        best
    }
}

// ---------------------------------------------------------------------------
// 亮条 / 边框
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy)]
struct Run {
    x: i32,
    width: i32,
}

/// 一行里所有「亮且够长」的横向条（网页版 `lightRuns`）。
fn light_runs(f: &Img<'_>, y: usize, threshold: u8, min: i32, max: i32) -> Vec<Run> {
    let mut runs = Vec::new();
    let mut start: Option<i32> = None;
    for x in 0..f.width {
        let light = (f.r(x, y) + f.g(x, y) * 2.0 + f.b(x, y)) / 4.0 > threshold as f64;
        if light {
            if start.is_none() {
                start = Some(x as i32);
            }
        } else if let Some(s) = start {
            let width = x as i32 - s;
            if width >= min && width <= max {
                runs.push(Run { x: s, width });
            }
            start = None;
        }
    }
    if let Some(s) = start {
        let width = f.width as i32 - s;
        if width >= min && width <= max {
            runs.push(Run { x: s, width });
        }
    }
    runs
}

fn border_row(f: &Img<'_>, x: i32, y: i32, width: i32) -> bool {
    if width <= 4 || y < 0 || y >= f.height as i32 {
        return false;
    }
    let mut count = 0;
    let mut total = 0;
    for dx in 2..width - 2 {
        let px = x + dx;
        if px < 0 || px >= f.width as i32 {
            continue;
        }
        if f.gray(px as usize, y as usize) > 115.0 {
            count += 1;
        }
        total += 1;
    }
    total > 0 && count as f64 / total as f64 > 0.78
}

/// 上下各让一行（边框常跨 1~2 行）。
fn border(f: &Img<'_>, x: i32, y: i32, width: i32) -> bool {
    [-1, 0, 1]
        .iter()
        .any(|dy| border_row(f, x, y + dy, width))
}

// ---------------------------------------------------------------------------
// 锚点搜索
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy)]
pub struct HudAnchor {
    pub x: f64,
    pub y: f64,
    pub sx: f64,
    pub sy: f64,
    pub error: f64,
    pub skin: HudSkin,
}

/// 定位结果：锚点 + 由它推出的区域（都是本帧的像素坐标）。
#[derive(Debug, Clone, Copy)]
pub struct HudLayout {
    pub anchor: HudAnchor,
    /// 经验文字区（`EXP xx(xx.xx%)`）。
    pub exp_text: (i32, i32, i32, i32),
    /// 经验条内部（填充色所在的那一块，网页版 `barRect`）。
    pub exp_bar: (i32, i32, i32, i32),
    /// `EXP` 标签（每帧用它确认 HUD 还在原地）。
    pub exp_label: (i32, i32, i32, i32),
    /// 等级区（`Lv.xx`）。
    pub level_box: (i32, i32, i32, i32),
    /// 小地图锚点推出的地图名区域（找不到小地图时为 `None`）。
    pub map_name: Option<(i32, i32, i32, i32)>,
    /// 职业 / 角色名那两行字。紧凑版 HUD 还没有真机画面可量，先是 `None`。
    pub job_text: Option<(i32, i32, i32, i32)>,
    pub name_text: Option<(i32, i32, i32, i32)>,
}

impl HudLayout {
    /// 读经验要抓的那一小块（标签 + 文字 + 条，四周留几像素）。
    ///
    /// 定位只在尺寸变化时做一次；之后每个 tick 只抓这一块，而不是整幅客户区
    /// （4K 全幅一帧 33MB，这一块不到 100KB）。
    pub fn read_strip(&self, client_w: i32, client_h: i32) -> (i32, i32, i32, i32) {
        let margin = (4.0 * self.anchor.sx.max(self.anchor.sy)).ceil() as i32 + 2;
        let rects = [self.exp_text, self.exp_bar, self.exp_label];
        let x0 = rects.iter().map(|r| r.0).min().unwrap_or(0) - margin;
        let y0 = rects.iter().map(|r| r.1).min().unwrap_or(0) - margin;
        let x1 = rects.iter().map(|r| r.0 + r.2).max().unwrap_or(0) + margin;
        let y1 = rects.iter().map(|r| r.1 + r.3).max().unwrap_or(0) + margin;
        let (x0, y0) = (x0.max(0), y0.max(0));
        let (x1, y1) = (x1.min(client_w), y1.min(client_h));
        (x0, y0, (x1 - x0).max(1), (y1 - y0).max(1))
    }

    /// 经验文字区在参考坐标系里的精确（浮点）矩形 —— 还原成原生像素要用它，
    /// `exp_text` 已经取整过，拿它反推会带进半个像素的相位误差。
    pub fn exp_text_exact(&self) -> (f64, f64, f64, f64) {
        let a = &self.anchor;
        match a.skin {
            HudSkin::Wide => {
                let left = a.x - ANCHOR_X * a.sx;
                let top = a.y - ANCHOR_Y * a.sy;
                (
                    left + EXP_TEXT.0 * a.sx,
                    top + EXP_TEXT.1 * a.sy,
                    EXP_TEXT.2 * a.sx,
                    EXP_TEXT.3 * a.sy,
                )
            }
            HudSkin::Compact => {
                let bar = a.x + COMPACT_OFFSETS[2] * a.sx;
                (
                    bar - 2.0 * a.sx,
                    a.y - 15.0 * a.sy,
                    (COMPACT_WIDTHS[2] + 3.0) * a.sx,
                    15.0 * a.sy,
                )
            }
        }
    }

    /// `EXP` 标签右端（参考坐标换算到本帧像素）：数字从这里往右开始。
    pub fn exp_label_right(&self) -> f64 {
        let a = &self.anchor;
        let bar = a.x + a.skin.offsets()[2] * a.sx;
        bar + (25.0 - a.skin.label_dx()) * a.sx
    }
}

fn hud_candidates(f: &Img<'_>, labels: &Labels, matcher: &mut Matcher) -> (Vec<HudAnchor>, bool) {
    let mut hud: Vec<HudAnchor> = Vec::new();
    let mut seen: HashMap<(i32, i32, i32), ()> = HashMap::new();
    let mut undersized = false;

    for threshold in LIGHT_THRESHOLDS {
        for y in 0..f.height.saturating_sub(7) {
            let runs = light_runs(f, y, threshold, 70, 350);
            for i in 0..runs.len() {
                for j in (i + 1)..runs.len() {
                    let a = runs[i];
                    let b = runs[j];
                    if b.x - a.x > 710 {
                        break;
                    }
                    if (a.width - b.width).abs() > 5 {
                        continue;
                    }
                    for (ia, ib) in [(0usize, 1usize), (1, 2), (0, 2)] {
                        let sx = (b.x - a.x) as f64 / (BAR_OFFSETS[ib] - BAR_OFFSETS[ia]);
                        if (a.width as f64 - BAR_WIDTH * sx).abs() > WIDTH_TOLERANCE
                            || sx > DETECT_MAX_SCALE
                        {
                            continue;
                        }
                        if sx < MIN_SCALE {
                            if sx >= 0.2
                                && matcher.refined(
                                    f,
                                    labels.ids[ia],
                                    &labels.templates[ia],
                                    a.x as f64 - sx,
                                    y as f64 - LABEL_DY * sx,
                                    sx,
                                    sx,
                                    1,
                                ) < 40.0
                                && matcher.refined(
                                    f,
                                    labels.ids[ib],
                                    &labels.templates[ib],
                                    b.x as f64 - sx,
                                    y as f64 - LABEL_DY * sx,
                                    sx,
                                    sx,
                                    1,
                                ) < 40.0
                            {
                                undersized = true;
                            }
                            continue;
                        }
                        let x = a.x as f64 - BAR_OFFSETS[ia] * sx;
                        let key = (
                            x.round() as i32,
                            y as i32,
                            (sx * 100.0).round() as i32,
                        );
                        if seen.contains_key(&key) {
                            continue;
                        }
                        seen.insert(key, ());
                        for height in 8..=35i32 {
                            let measured = height as f64 / BORDER_GAP;
                            let sy = if (measured - sx).abs() < 0.045 {
                                sx
                            } else {
                                measured
                            };
                            if y + height as usize >= f.height
                                || !border(f, a.x, y as i32 + height, a.width)
                                || !border(f, b.x, y as i32 + height, b.width)
                            {
                                continue;
                            }
                            let e1 = matcher.refined(
                                f,
                                labels.ids[ia],
                                &labels.templates[ia],
                                a.x as f64 - sx,
                                y as f64 - LABEL_DY * sy,
                                sx,
                                sy,
                                1,
                            );
                            if e1 > 46.0 {
                                continue;
                            }
                            let e2 = matcher.refined(
                                f,
                                labels.ids[ib],
                                &labels.templates[ib],
                                b.x as f64 - sx,
                                y as f64 - LABEL_DY * sy,
                                sx,
                                sy,
                                1,
                            );
                            if e2 > 46.0 {
                                continue;
                            }
                            let width_error = (a.width as f64 - BAR_WIDTH * sx).abs()
                                + (b.width as f64 - BAR_WIDTH * sx).abs();
                            let error = (e1 + e2) / 2.0
                                + width_error * 10.0
                                + (3.0f64).min((sy - sx).abs() * 25.0);
                            let found = HudAnchor {
                                x,
                                y: y as f64,
                                sx,
                                sy,
                                error,
                                skin: HudSkin::Wide,
                            };
                            if let Some(existing) = hud.iter_mut().find(|v| {
                                (v.x - x).abs() < 8.0 && (v.y - y as f64).abs() < 8.0
                            }) {
                                if error < existing.error {
                                    *existing = found;
                                }
                            } else {
                                hud.push(found);
                            }
                        }
                    }
                }
            }
        }
    }
    (hud, undersized)
}

/// 紧凑皮肤的第 `n` 个条是否成立：上下边框都在 + 标签模板对得上（网页版 `Y`）。
fn compact_bar_ok(
    f: &Img<'_>,
    labels: &Labels,
    matcher: &mut Matcher,
    a: &HudAnchor,
    n: usize,
    limit: f64,
) -> bool {
    let x = a.x + COMPACT_OFFSETS[n] * a.sx;
    let width = (COMPACT_WIDTHS[n] * a.sx).round() as i32;
    border(f, x.round() as i32, a.y.round() as i32, width)
        && border(
            f,
            x.round() as i32,
            (a.y + a.skin.border_gap() * a.sy).round() as i32,
            width,
        )
        && matcher.refined(
            f,
            labels.ids[n],
            &labels.templates[n],
            x - 2.0 * a.sx,
            a.y - LABEL_DY * a.sy,
            a.sx,
            a.sy,
            1,
        ) <= limit
}

/// 紧凑皮肤的候选锚点（网页版 `le`）：宽版一个都没找到时才跑。
fn compact_candidates(f: &Img<'_>, labels: &Labels, matcher: &mut Matcher) -> Vec<HudAnchor> {
    let mut hud: Vec<HudAnchor> = Vec::new();
    let mut seen: HashMap<(i32, i32, i32), ()> = HashMap::new();
    for threshold in LIGHT_THRESHOLDS {
        for y in 0..f.height.saturating_sub(7) {
            let runs = light_runs(f, y, threshold, 45, 350);
            for i in 0..runs.len() {
                for j in (i + 1)..runs.len() {
                    let a = runs[i];
                    let b = runs[j];
                    if b.x - a.x > 710 {
                        break;
                    }
                    for (ia, ib) in [(0usize, 1usize), (1, 2), (0, 2)] {
                        let sx = (b.x - a.x) as f64 / (COMPACT_OFFSETS[ib] - COMPACT_OFFSETS[ia]);
                        let width_error = (a.width as f64 - COMPACT_WIDTHS[ia] * sx).abs()
                            + (b.width as f64 - COMPACT_WIDTHS[ib] * sx).abs();
                        if !(MIN_SCALE..=DETECT_MAX_SCALE).contains(&sx)
                            || (a.width as f64 - COMPACT_WIDTHS[ia] * sx).abs() > WIDTH_TOLERANCE
                            || (b.width as f64 - COMPACT_WIDTHS[ib] * sx).abs() > WIDTH_TOLERANCE
                        {
                            continue;
                        }
                        let x = a.x as f64 - COMPACT_OFFSETS[ia] * sx;
                        let key = (x.round() as i32, y as i32, (sx * 100.0).round() as i32);
                        if seen.insert(key, ()).is_some() {
                            continue;
                        }
                        for height in 7..=32i32 {
                            let measured = height as f64 / COMPACT_BORDER_GAP;
                            let sy = if (measured - sx).abs() < 0.045 {
                                sx
                            } else {
                                measured
                            };
                            if y + height as usize >= f.height {
                                continue;
                            }
                            let candidate = HudAnchor {
                                x,
                                y: y as f64,
                                sx,
                                sy,
                                error: 0.0,
                                skin: HudSkin::Compact,
                            };
                            if !compact_bar_ok(f, labels, matcher, &candidate, ia, 46.0)
                                || !compact_bar_ok(f, labels, matcher, &candidate, ib, 46.0)
                            {
                                continue;
                            }
                            let e1 = matcher.refined(
                                f,
                                labels.ids[ia],
                                &labels.templates[ia],
                                a.x as f64 - 2.0 * sx,
                                y as f64 - LABEL_DY * sy,
                                sx,
                                sy,
                                1,
                            );
                            let e2 = matcher.refined(
                                f,
                                labels.ids[ib],
                                &labels.templates[ib],
                                b.x as f64 - 2.0 * sx,
                                y as f64 - LABEL_DY * sy,
                                sx,
                                sy,
                                1,
                            );
                            let error = (e1 + e2) / 2.0
                                + width_error * 10.0
                                + (3.0f64).min((sy - sx).abs() * 25.0);
                            let found = HudAnchor { error, ..candidate };
                            if let Some(existing) = hud.iter_mut().find(|v| {
                                (v.x - x).abs() < 8.0 && (v.y - y as f64).abs() < 8.0
                            }) {
                                if error < existing.error {
                                    *existing = found;
                                }
                            } else {
                                hud.push(found);
                            }
                        }
                    }
                }
            }
        }
    }
    hud
}

/// 在源分辨率上把候选锚点附近再修一遍（网页版 `refineHud`）。
fn refine_hud(
    f: &Img<'_>,
    labels: &Labels,
    matcher: &mut Matcher,
    candidate: &HudAnchor,
    radius: i32,
) -> Option<HudAnchor> {
    let mut best: Option<HudAnchor> = None;
    for dy in -radius..=radius {
        for dx in -radius..=radius {
            let a = HudAnchor {
                x: candidate.x + dx as f64,
                y: candidate.y + dy as f64,
                ..*candidate
            };
            if a.skin == HudSkin::Compact {
                let ok: Vec<usize> = (0..3)
                    .filter(|n| compact_bar_ok(f, labels, matcher, &a, *n, 46.0))
                    .collect();
                if ok.len() < 2 {
                    continue;
                }
                let error = ok
                    .iter()
                    .map(|n| {
                        matcher.refined(
                            f,
                            labels.ids[*n],
                            &labels.templates[*n],
                            a.x + COMPACT_OFFSETS[*n] * a.sx - 2.0 * a.sx,
                            a.y - LABEL_DY * a.sy,
                            a.sx,
                            a.sy,
                            1,
                        )
                    })
                    .sum::<f64>()
                    / ok.len() as f64
                    + (dx.abs() + dy.abs()) as f64 * 0.05;
                if best.as_ref().map(|b| error < b.error).unwrap_or(true) {
                    best = Some(HudAnchor { error, ..a });
                }
                continue;
            }
            let mut errors: Vec<f64> = (0..3)
                .map(|i| {
                    matcher.refined(
                        f,
                        labels.ids[i],
                        &labels.templates[i],
                        a.x + BAR_OFFSETS[i] * a.sx - a.sx,
                        a.y - LABEL_DY * a.sy,
                        a.sx,
                        a.sy,
                        1,
                    )
                })
                .collect();
            errors.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
            if errors[1] > 46.0 {
                continue;
            }
            let borders = (0..3)
                .filter(|i| {
                    border(
                        f,
                        (a.x + BAR_OFFSETS[*i] * a.sx).round() as i32,
                        a.y.round() as i32,
                        (BAR_WIDTH * a.sx).round() as i32,
                    ) && border(
                        f,
                        (a.x + BAR_OFFSETS[*i] * a.sx).round() as i32,
                        (a.y + BORDER_GAP * a.sy).round() as i32,
                        (BAR_WIDTH * a.sx).round() as i32,
                    )
                })
                .count();
            if borders < 2 {
                continue;
            }
            let error = (errors[0] + errors[1]) / 2.0 + (dx.abs() + dy.abs()) as f64 * 0.05;
            if best.as_ref().map(|b| error < b.error).unwrap_or(true) {
                best = Some(HudAnchor { error, ..a });
            }
        }
    }
    best
}

/// 浮点矩形取整并钳进画面：裁到画面外会让 OCR/字形都读不出来。
fn clamp_rect(rect: (f64, f64, f64, f64), frame: (usize, usize)) -> (i32, i32, i32, i32) {
    let x = rect.0.round() as i32;
    let y = rect.1.round() as i32;
    let w = rect.2.round().max(1.0) as i32;
    let h = rect.3.round().max(1.0) as i32;
    let x1 = (x + w).min(frame.0 as i32);
    let y1 = (y + h).min(frame.1 as i32);
    (x.max(0), y.max(0), (x1 - x.max(0)).max(1), (y1 - y.max(0)).max(1))
}

/// HUD 精灵参考坐标 → 本帧像素（`origin` 是锚点在精灵里的参考坐标）。
fn sprite_rect(
    anchor: &HudAnchor,
    origin: (f64, f64),
    reference: (f64, f64, f64, f64),
) -> (f64, f64, f64, f64) {
    let left = anchor.x - origin.0 * anchor.sx;
    let top = anchor.y - origin.1 * anchor.sy;
    (
        left + reference.0 * anchor.sx,
        top + reference.1 * anchor.sy,
        reference.2 * anchor.sx,
        reference.3 * anchor.sy,
    )
}

/// 由锚点推出本帧的各个区域（网页版 `J` / `ce`）。
fn layout_for(anchor: HudAnchor, pixels: &Pixels<'_>) -> HudLayout {
    let frame = (pixels.width, pixels.height);
    let a = &anchor;
    let exp_bar_x = a.x + a.skin.offsets()[2] * a.sx;
    let (exp_bar, level_box) = match a.skin {
        HudSkin::Wide => (
            (exp_bar_x, a.y + 2.0 * a.sy, BAR_WIDTH * a.sx, 14.0 * a.sy),
            sprite_rect(a, (ANCHOR_X, ANCHOR_Y), LEVEL_BOX),
        ),
        HudSkin::Compact => (
            (
                exp_bar_x,
                a.y + 2.0 * a.sy,
                COMPACT_WIDTHS[2] * a.sx,
                COMPACT_BAR_HEIGHT * a.sy,
            ),
            sprite_rect(a, (COMPACT_ANCHOR_X, COMPACT_ANCHOR_Y), COMPACT_LEVEL_BOX),
        ),
    };
    let exp_label = (
        exp_bar_x - a.skin.label_dx() * a.sx,
        a.y - LABEL_DY * a.sy,
        25.0 * a.sx,
        11.0 * a.sy,
    );
    let identity = |reference| match a.skin {
        HudSkin::Wide => Some(clamp_rect(
            sprite_rect(a, (ANCHOR_X, ANCHOR_Y), reference),
            frame,
        )),
        HudSkin::Compact => None,
    };
    let mut layout = HudLayout {
        anchor,
        exp_text: (0, 0, 1, 1),
        exp_bar: clamp_rect(exp_bar, frame),
        exp_label: clamp_rect(exp_label, frame),
        level_box: clamp_rect(level_box, frame),
        map_name: None,
        job_text: identity(JOB_TEXT),
        name_text: identity(NAME_TEXT),
    };
    layout.exp_text = clamp_rect(layout.exp_text_exact(), frame);
    layout
}

/// 测试用：已知锚点（真机画面上量出来的）直接推出布局，不用带着整幅画面。
#[cfg(test)]
pub(crate) fn layout_from_anchor(anchor: HudAnchor, width: usize, height: usize) -> HudLayout {
    let empty = Pixels {
        width,
        height,
        bgra: &[],
    };
    layout_for(anchor, &empty)
}

/// 把缩放量**准**：用 HP 条与 EXP 条两条上边框的左端距离重新算一次 `sx`。
///
/// 为什么需要：候选阶段的 `sx` 可能来自相邻两个条（跨度只有 168 参考像素），
/// 边框起点各有 ±1 像素的误差 —— 1.875 倍拉伸时那是 0.3% 的缩放误差。
/// 经验文字在锚点右边 343 参考像素处，把它**还原回原生像素**逐点认字时，
/// 0.3% 会累积成一个多像素的错位，字形就对不上了。
/// 用 HP↔EXP 这一对（跨度 344）重量，误差减半；两个条用同一个阈值量，
/// 模糊造成的边缘偏移还会相互抵消。
fn refine_scale(pixels: &Pixels<'_>, anchor: HudAnchor) -> HudAnchor {
    let f = Img::wrap(pixels);
    let offsets = anchor.skin.offsets();
    let widths = anchor.skin.widths();
    let span = offsets[2] - offsets[0];
    let near = |runs: &[Run], x: f64, width: f64| {
        runs.iter()
            .filter(|r| (r.x as f64 - x).abs() <= 2.0 + anchor.sx)
            .filter(|r| (r.width as f64 - width).abs() <= WIDTH_TOLERANCE + 2.0)
            .min_by(|a, b| {
                (a.x as f64 - x)
                    .abs()
                    .partial_cmp(&(b.x as f64 - x).abs())
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .copied()
    };
    let y0 = anchor.y.round() as i64;
    let mut measured: Vec<f64> = Vec::new();
    for dy in -1..=1i64 {
        let y = y0 + dy;
        if y < 0 || y as usize >= f.height {
            continue;
        }
        for threshold in LIGHT_THRESHOLDS {
            let runs = light_runs(&f, y as usize, threshold, 20, 1400);
            let first = near(&runs, anchor.x, widths[0] * anchor.sx);
            let last = near(
                &runs,
                anchor.x + offsets[2] * anchor.sx,
                widths[2] * anchor.sx,
            );
            if let (Some(first), Some(last)) = (first, last) {
                measured.push((last.x - first.x) as f64 / span);
            }
        }
    }
    if measured.is_empty() {
        return anchor;
    }
    measured.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let sx = measured[measured.len() / 2];
    // 只接受「修正」，不接受「推翻」：差得太多说明量到的是别的东西
    if (sx - anchor.sx).abs() > anchor.sx * 0.03 {
        return anchor;
    }
    let sy = if (anchor.sy - anchor.sx).abs() < 1e-9 {
        sx
    } else {
        anchor.sy
    };
    HudAnchor { sx, sy, ..anchor }
}

/// 网页版 `layoutSearch` 的 HUD 部分：1× / 1/2 / 1/4 三级金字塔 + 去重 + 精修；
/// 宽版一个都找不到时再找紧凑版。
pub fn locate(pixels: &Pixels<'_>) -> Option<HudLayout> {
    let pack = pack()?;
    // 只有 1080p 基准 HUD 的 39 像素高，画面比它矮就没什么可找的
    if pixels.width < 200 || pixels.height < 60 {
        return None;
    }
    let mut matcher = Matcher::new();
    let mut hud: Vec<HudAnchor> = Vec::new();

    // **先粗后细**：4K 下 factor=1 是 800 万像素 × 4 组阈值的全扫，先扫 1/4 与 1/2
    // 层，够自信（误差 ≤ 10）就不扫最细那层了 —— 锚点只要「够对准经验行」，
    // 后面还有经验表自检兜底。
    let level1 = Img::wrap(pixels);
    let level2 = level1.half();
    let level4 = level2.half();
    let levels: [(&Img<'_>, f64); 3] = [(&level4, 4.0), (&level2, 2.0), (&level1, 1.0)];
    for skin in [HudSkin::Wide, HudSkin::Compact] {
        let labels = pack.labels(skin);
        for (level, factor) in levels {
            if !hud.is_empty() && hud.iter().any(|a| a.error <= 10.0) {
                break;
            }
            collect_candidates(level, factor, skin, labels, &mut matcher, pixels, &mut hud);
        }
        if !hud.is_empty() {
            break;
        }
    }

    if hud.is_empty() {
        return None;
    }
    hud.sort_by(|a, b| a.error.partial_cmp(&b.error).unwrap_or(std::cmp::Ordering::Equal));
    // 多个候选（网页版直接判「多场景」放弃）：只有第一名**明显**更好才采纳 ——
    // 要么它本身很自信，要么和第二名拉开了差距。模糊的拉伸画面（全屏把 768p 拉到
    // 1440p）误差本来就在 20 上下，只看绝对值会把唯一正确的那个也拒掉。
    if hud.len() > 1 && hud[0].error > 18.0 && hud[1].error - hud[0].error < 8.0 {
        return None;
    }
    let anchor = hud[0];
    if anchor.sx <= 0.0 || anchor.sy <= 0.0 {
        return None;
    }
    let anchor = refine_scale(pixels, anchor);
    let mut layout = layout_for(anchor, pixels);
    layout.map_name = locate_map(pixels, anchor.sx, anchor.sy);
    Some(layout)
}

/// 每帧确认 HUD 还在原地：`EXP` 标签模板在预期位置附近对得上。
///
/// `pixels` 可以只是一小块（左上角在客户区的 `origin`）—— 每个 tick 只抓经验
/// 那一小块，不重扫整幅画面。对不上（被面板盖住 / 过场 / 窗口改了 UI 缩放）
/// 就别在旧位置上硬读了。
pub fn exp_label_visible(pixels: &Pixels<'_>, origin: (i32, i32), layout: &HudLayout) -> bool {
    let Some(pack) = pack() else {
        return false;
    };
    let a = &layout.anchor;
    let labels = pack.labels(a.skin);
    let f = Img::wrap(pixels);
    let mut matcher = Matcher::new();
    let x = a.x + a.skin.offsets()[2] * a.sx - a.skin.label_dx() * a.sx - origin.0 as f64;
    let y = a.y - LABEL_DY * a.sy - origin.1 as f64;
    // 半径 2（网页版 `he` 同款）：锚点在拉伸画面上本来就有 ±1 像素的误差
    matcher.refined(&f, labels.ids[2], &labels.templates[2], x, y, a.sx, a.sy, 2)
        <= LABEL_VISIBLE_ERROR
}

/// 经验条的填充比例（网页版 `me`）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BarReading {
    /// 0~1
    pub ratio: f64,
    /// 0~1：三条扫描线一致、且都量到了才高
    pub confidence: f64,
}

/// 量经验条填到了哪儿：在条高 28% / 50% / 72% 三条扫描线上找**最右边**的
/// 填充色像素。和字无关，模糊、拉伸都不怕 —— 网页版拿它和 OCR 读数互相印证。
///
/// 0% 的时候条是空的，量不出来（返回 `None`），这不是错误。
pub fn exp_bar_ratio(
    pixels: &Pixels<'_>,
    origin: (i32, i32),
    rect: (i32, i32, i32, i32),
) -> Option<BarReading> {
    let f = Img::wrap(pixels);
    let (x0, y0, w, h) = (rect.0 - origin.0, rect.1 - origin.1, rect.2, rect.3);
    if w < 8 || h < 2 || x0 < 0 || y0 < 0 {
        return None;
    }
    if (x0 + w) as usize > f.width || (y0 + h) as usize > f.height {
        return None;
    }
    let matches = |x: usize, y: usize| {
        let rgb = f.rgb(x, y);
        EXP_BAR_COLORS.iter().any(|color| {
            let d = (rgb[0] - color[0]).powi(2)
                + (rgb[1] - color[1]).powi(2)
                + (rgb[2] - color[2]).powi(2);
            d.sqrt() <= EXP_BAR_TOLERANCE
        })
    };
    let lines = [0.28f64, 0.5, 0.72];
    let mut ratios: Vec<f64> = Vec::new();
    for line in lines {
        let y = (y0 + (h as f64 * line).floor() as i32) as usize;
        let mut last: Option<usize> = None;
        let mut count = 0usize;
        for dx in 0..w as usize {
            if matches(x0 as usize + dx, y) {
                last = Some(dx);
                count += 1;
            }
        }
        if let Some(last) = last {
            if count as f64 >= 2.0f64.max(w as f64 * 0.02) {
                ratios.push(((last + 1) as f64 / w as f64).clamp(0.0, 1.0));
            }
        }
    }
    if ratios.is_empty() {
        return None;
    }
    ratios.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let ratio = ratios[ratios.len() / 2];
    let spread = ratios
        .iter()
        .map(|r| (r - ratio).abs())
        .fold(0.0f64, f64::max);
    let confidence =
        (ratios.len() as f64 / lines.len() as f64 * (1.0 - spread * 5.0)).max(0.0);
    Some(BarReading { ratio, confidence })
}

// ---------------------------------------------------------------------------
// 小地图 → 地图名区域（网页版 `mapField` + `mapPanelRight` 的移植）
// ---------------------------------------------------------------------------

/// 小地图锚点（39×14 的图标模板）→ 地图名那一行。
///
/// 小地图在左上角，图标模板扫到之后，地图名就在它右边固定偏移处
/// （`+48×sx, +43×sy`），宽度再用面板背景色裁到真实边界。
pub fn locate_map(pixels: &Pixels<'_>, sx: f64, sy: f64) -> Option<(i32, i32, i32, i32)> {
    let pack = pack()?;
    let (minimap_id, minimap) = pack.minimap.as_ref()?;
    if sx <= 0.0 || sy <= 0.0 {
        return None;
    }
    let frame = Img::wrap(pixels);
    let mut matcher = Matcher::new();
    // 大尺寸时先在对等的金字塔层上粗筛（和网页版一样）
    let factor: f64 = if sx >= 3.0 && sy >= 3.0 {
        4.0
    } else if sx >= 1.5 && sy >= 1.5 {
        2.0
    } else {
        1.0
    };
    let level2;
    let level4;
    let coarse: &Img<'_> = match factor as i32 {
        4 => {
            level2 = frame.half();
            level4 = level2.half();
            &level4
        }
        2 => {
            level2 = frame.half();
            &level2
        }
        _ => &frame,
    };
    let mx = sx / factor;
    let my = sy / factor;
    let max_y = coarse.height as f64 - 20.0 * my;
    let max_x = coarse.width as f64 - 45.0 * mx;
    if max_y <= 0.0 || max_x <= 0.0 {
        return None;
    }

    let mut maps: Vec<(f64, i32, i32)> = Vec::new(); // (error, x, y)
    for y in 0..max_y as usize {
        for x in 0..max_x as usize {
            let (r, g, b) = (coarse.r(x, y), coarse.g(x, y), coarse.b(x, y));
            // 小地图面板是偏蓝绿的：先按颜色挡掉绝大多数位置
            if r < 80.0 || b < r + 6.0 || g < r + 2.0 {
                continue;
            }
            if factor >= 2.0
                && matcher.patch_variant_error(
                    coarse,
                    *minimap_id,
                    minimap,
                    x as f64,
                    y as f64,
                    mx,
                    my,
                    0,
                ) > 55.0
            {
                continue;
            }
            let radius = if factor > 1.0 { factor as i32 } else { 0 };
            for dy in -radius..=radius {
                for dx in -radius..=radius {
                    let px = (x as f64 * factor) as i32 + dx;
                    let py = (y as f64 * factor) as i32 + dy;
                    let error = matcher.patch_error(
                        &frame,
                        *minimap_id,
                        minimap,
                        px as f64,
                        py as f64,
                        sx,
                        sy,
                    );
                    if error > 43.0 {
                        continue;
                    }
                    if let Some(existing) = maps.iter_mut().find(|(_, ex, ey)| {
                        (px - *ex).abs() < (20.0 * sx) as i32
                            && (py - *ey).abs() < (15.0 * sy) as i32
                    }) {
                        if error < existing.0 {
                            *existing = (error, px, py);
                        }
                    } else {
                        maps.push((error, px, py));
                    }
                }
            }
        }
    }
    // 多于一处小地图：宁可不要（网页版判「多场景」放弃）
    if maps.len() != 1 {
        return None;
    }
    let (_, x, y) = maps[0];
    map_field(&frame, x as f64, y as f64, sx, sy)
}

/// 由小地图锚点算出地图名区域（网页版 `mapField`）。
fn map_field(f: &Img<'_>, x: f64, y: f64, sx: f64, sy: f64) -> Option<(i32, i32, i32, i32)> {
    let left = x - 5.0 * sx;
    let top = y - 4.0 * sy;
    let start = (left + 48.0 * sx).round() as i32;
    let area_y = (top + 43.0 * sy).round() as i32;
    let area_w = (402.0 * sx).round().max(1.0) as i32;
    let area_h = (18.0 * sy).round().max(1.0) as i32;
    let right = map_panel_right(f, start, area_y, area_w, area_h)?;
    if right - start > (20.0 * sx) as i32 {
        Some((start, area_y, right - start, area_h))
    } else {
        None
    }
}

/// 面板右边界：从左边往右扫，遇到背景色断档（连续 gap 列）就停。
fn map_panel_right(f: &Img<'_>, x: i32, y: i32, width: i32, height: i32) -> Option<i32> {
    let background = background_color(f, x, y, width, height)?;
    let end = (x + width).min(f.width as i32);
    if end <= x {
        return None;
    }
    let mut last = x;
    let mut misses = 0;
    let gap = 3.max((height as f64 / 4.0).round() as i32);
    let end_y = (y + height).min(f.height as i32);
    let required = (height as f64 * 0.28).ceil() as i32;
    for px in x..end {
        let mut matching = 0;
        for py in y.max(0)..end_y {
            let rgb = f.rgb(px.max(0) as usize, py as usize);
            if (rgb[0] - background[0]).abs() < 24.0
                && (rgb[1] - background[1]).abs() < 24.0
                && (rgb[2] - background[2]).abs() < 24.0
            {
                matching += 1;
            }
            if matching >= required {
                break;
            }
        }
        if matching >= required {
            last = px;
            misses = 0;
        } else {
            misses += 1;
            if misses >= gap && px - x > height {
                return Some(last + 1);
            }
        }
    }
    Some(end)
}

/// 区域里最主要的背景色（网页版 `backgroundColor`：直方图 + 平均数）。
fn background_color(f: &Img<'_>, x: i32, y: i32, width: i32, height: i32) -> Option<[f64; 3]> {
    let mut bins: HashMap<i32, (u32, [f64; 3])> = HashMap::new();
    let x_end = (x + width).min(f.width as i32).min(x + height * 2);
    let y_end = (y + height).min(f.height as i32);
    for py in y.max(0)..y_end {
        for px in x.max(0)..x_end {
            let rgb = f.rgb(px as usize, py as usize);
            if rgb[0] < 65.0 || rgb[2] < rgb[0] + 8.0 || rgb[1] < rgb[0] + 6.0 || rgb[2] >= 245.0 {
                continue;
            }
            let key = ((rgb[0] as i32) >> 3) * 1024
                + ((rgb[1] as i32) >> 3) * 32
                + ((rgb[2] as i32) >> 3);
            let entry = bins.entry(key).or_insert((0, [0.0; 3]));
            entry.0 += 1;
            entry.1[0] += rgb[0];
            entry.1[1] += rgb[1];
            entry.1[2] += rgb[2];
        }
    }
    let best = bins
        .values()
        .max_by_key(|(count, _)| *count)
        .filter(|(count, _)| *count >= 4.max(height.max(0) as u32))?;
    Some([
        best.1[0] / best.0 as f64,
        best.1[1] / best.0 as f64,
        best.1[2] / best.0 as f64,
    ])
}

/// 在一个金字塔层上找候选，映射回源分辨率、去重、精修后并入 `hud`。
fn collect_candidates(
    level: &Img<'_>,
    factor: f64,
    skin: HudSkin,
    labels: &Labels,
    matcher: &mut Matcher,
    source: &Pixels<'_>,
    hud: &mut Vec<HudAnchor>,
) {
    let anchors = match skin {
        HudSkin::Wide => hud_candidates(level, labels, matcher).0,
        HudSkin::Compact => compact_candidates(level, labels, matcher),
    };
    let source_img = Img::wrap(source);
    for candidate in anchors {
        let mapped = HudAnchor {
            x: candidate.x * factor,
            y: candidate.y * factor,
            sx: candidate.sx * factor,
            sy: candidate.sy * factor,
            ..candidate
        };
        if mapped.sx > MAX_SCALE || mapped.sy > MAX_SCALE {
            continue;
        }
        // 源分辨率上已经有近邻就别重复（降采样出来的会重新精修）
        if hud.iter().any(|a| {
            (a.x - mapped.x).abs() < 8.0 * a.sx.max(mapped.sx)
                && (a.y - mapped.y).abs() < 8.0 * a.sx.max(mapped.sx)
        }) {
            continue;
        }
        let refined = if factor == 1.0 {
            Some(mapped)
        } else {
            refine_hud(&source_img, labels, matcher, &mapped, factor as i32)
        };
        let Some(refined) = refined else {
            continue;
        };
        if let Some(existing) = hud.iter_mut().find(|a| {
            (a.x - refined.x).abs() < 8.0 * a.sx.max(refined.sx)
                && (a.y - refined.y).abs() < 8.0 * a.sx.max(refined.sx)
        }) {
            if refined.error < existing.error {
                *existing = refined;
            }
        } else {
            hud.push(refined);
        }
    }
}

/// 测试用：在 `bgra`（`width × height`）上按 `skin` 画一套 HUD 骨架 ——
/// 三条亮边框 + 三个标签（模板本身按 `scale` 缩放）。锚点是 HP 条上边框左端。
///
/// 这是端到端验证移植正确性的手段（不需要真机）；`hudread` 的测试也用它
/// 搭一张完整的原生画面，再把真机那条经验行贴上去。
#[cfg(test)]
pub(crate) fn paint_hud(
    bgra: &mut [u8],
    width: usize,
    height: usize,
    anchor: (f64, f64),
    scale: f64,
    skin: HudSkin,
) {
    let pack = pack().expect("模板应能加载");
    let (anchor_x, anchor_y) = anchor;
    let put = |bgra: &mut [u8], x: i32, y: i32, rgb: [f64; 3]| {
        if x < 0 || y < 0 || x >= width as i32 || y >= height as i32 {
            return;
        }
        let i = (y as usize * width + x as usize) * 4;
        bgra[i] = rgb[2] as u8;
        bgra[i + 1] = rgb[1] as u8;
        bgra[i + 2] = rgb[0] as u8;
        bgra[i + 3] = 255;
    };
    // 三条条的上下边框（亮线）
    for i in 0..3 {
        let bx = anchor_x + skin.offsets()[i] * scale;
        let bar_w = (skin.widths()[i] * scale).round() as i32;
        for dy in [0.0, skin.border_gap() * scale] {
            for dx in 0..bar_w {
                let y = (anchor_y + dy).round() as i32;
                put(bgra, bx.round() as i32 + dx, y, [235.0, 235.0, 235.0]);
                put(bgra, bx.round() as i32 + dx, y + 1, [180.0, 180.0, 180.0]);
            }
        }
    }
    // 三个标签：按 scale 缩放模板画上去
    let labels = pack.labels(skin);
    for i in 0..3 {
        let t = &labels.templates[i];
        let bx = anchor_x + skin.offsets()[i] * scale;
        let left = bx - skin.label_dx() * scale;
        let top = anchor_y - LABEL_DY * scale;
        let w = ((t.width as f64 * scale).round() as usize).max(1);
        let h = ((t.height as f64 * scale).round() as usize).max(1);
        for oy in 0..h {
            for ox in 0..w {
                let tx = ((ox as f64 + 0.5) * t.width as f64 / w as f64 - 0.5)
                    .max(0.0)
                    .round() as usize;
                let ty = ((oy as f64 + 0.5) * t.height as f64 / h as f64 - 0.5)
                    .max(0.0)
                    .round() as usize;
                let rgb = t.rgb[ty.min(t.height - 1) * t.width + tx.min(t.width - 1)];
                put(
                    bgra,
                    left.round() as i32 + ox as i32,
                    top.round() as i32 + oy as i32,
                    rgb,
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 用模板本身合成一张「游戏画面」：暗底 + 三条亮边框 + 三个标签。
    fn synth_hud(scale: f64, width: usize, height: usize) -> (Vec<u8>, (i32, i32)) {
        synth_skin(scale, width, height, HudSkin::Wide)
    }

    fn synth_skin(scale: f64, width: usize, height: usize, skin: HudSkin) -> (Vec<u8>, (i32, i32)) {
        let mut bgra = vec![20u8; width * height * 4];
        let anchor_x = 120.0;
        let anchor_y = height as f64 - 120.0;
        paint_hud(&mut bgra, width, height, (anchor_x, anchor_y), scale, skin);
        (bgra, (anchor_x.round() as i32, anchor_y.round() as i32))
    }

    /// 紧凑皮肤（短血条）：宽版找不到时要能退到它（以前模板载入了却从来没用过）。
    #[test]
    fn the_compact_skin_is_found_when_the_wide_one_is_absent() {
        for scale in [1.0f64, 1.5] {
            let (bgra, (ax, ay)) = synth_skin(scale, 1280, 720, HudSkin::Compact);
            let pixels = Pixels {
                width: 1280,
                height: 720,
                bgra: &bgra,
            };
            let layout =
                locate(&pixels).unwrap_or_else(|| panic!("{scale}× 下必须定位到紧凑版 HUD"));
            assert_eq!(layout.anchor.skin, HudSkin::Compact, "{scale}× 应认成紧凑版");
            assert!((layout.anchor.x - ax as f64).abs() < 4.0, "{scale}× 锚点 x 偏了");
            assert!((layout.anchor.y - ay as f64).abs() < 4.0, "{scale}× 锚点 y 偏了");
            assert!((layout.anchor.sx - scale).abs() < 0.05, "{scale}× 缩放估计偏差过大");
            // 经验条就是第三个条（参考偏移 221、宽 115）
            let (bx, _, bw, _) = layout.exp_bar;
            assert!(
                (bx as f64 - (ax as f64 + 221.0 * scale)).abs() <= 2.0,
                "{scale}× 经验条左端不对：{bx}"
            );
            assert!((bw as f64 - 115.0 * scale).abs() <= 2.0, "{scale}× 经验条宽度不对：{bw}");
        }
    }

    #[test]
    fn templates_load_and_are_not_empty() {
        let pack = pack().expect("内置模板应能解析");
        for i in 0..3 {
            assert!(pack.wide.templates[i].width >= 10);
            assert!(pack.wide.templates[i].height >= 8);
        }
        assert_eq!(pack.wide.templates[0].width, 17);
        assert_eq!(pack.wide.templates[2].width, 25);
    }

    /// 网页版这套锚点定位的**核心承诺**：1× 与 1.5×（模拟 150% 显示缩放 /
    /// 1440p）都能定位，而且推出的经验行区域就在画的那一行上。
    #[test]
    fn hud_anchor_is_found_at_100_and_150_percent_scale() {
        for scale in [1.0f64, 1.5] {
            let (bgra, (ax, ay)) = synth_hud(scale, 1280, 720);
            let pixels = Pixels {
                width: 1280,
                height: 720,
                bgra: &bgra,
            };
            let layout = locate(&pixels)
                .unwrap_or_else(|| panic!("{scale}× 下必须能定位到 HUD 锚点"));
            let error_x = (layout.anchor.x - ax as f64).abs();
            let error_y = (layout.anchor.y - ay as f64).abs();
            assert!(error_x < 4.0, "{scale}× 锚点 x 偏了 {error_x}");
            assert!(error_y < 4.0, "{scale}× 锚点 y 偏了 {error_y}");
            assert!(
                (layout.anchor.sx - scale).abs() < 0.12,
                "{scale}× 缩放估计 {:.3} 偏差过大",
                layout.anchor.sx
            );
            // 经验行区域应落在锚点右下方（参考偏移 343px × 缩放）
            let (ex, _ey, ew, eh) = layout.exp_text;
            assert!(ex > ax, "{scale}× 经验行应在锚点右侧");
            assert!(ew > 40 && eh > 6, "{scale}× 经验行区域太小：{ew}×{eh}");
        }
    }
}
