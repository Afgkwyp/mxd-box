//! 在画面里找 BUFF 图标：灰度 + 横向梯度两路**归一化相关**（NCC），粗搜 + 精修。
//!
//! # 和网页版（枫记）的关系
//!
//! 打分方式照搬网页版 `vision-core.js` 的 `exact()`：同一个位置上，灰度相关占 0.45、
//! 横向 Sobel 相关占 0.55。它们的门槛 0.73 也沿用（`DISCOVER_SCORE`）。
//!
//! 模板有两种来源：内置技能图标（32×32 原图）；或用户**直接在游戏截图上框的图标**。
//! 两种都是原生大小：画面先按界面倍数缩回原生像素（见 `scale.rs`），模板不跟着分辨率变。
//! 老版本框的图标没存原生像素，才用 `IconTemplate::scaled` 按猜的倍数缩一下。
//!
//! # 掩码：左下角不参与比较
//!
//! 剩余时间的数字画在图标**左下角**（网页版的数字区就是左下 ~60% 宽、下 ~40% 高），
//! 数字每秒都在变，参与比较只会拉低分数。和网页版 `identity()` 用的区域一样，
//! 只比上半截和右侧一条。框选时多框进来的一圈背景也剔掉（最外 1 像素）。
//!
//! # 已出现的图标降低门槛
//!
//! BUFF 快到期时图标会变暗 / 闪烁，分数会掉。所以「第一次找到」要 0.73，
//! 「在上一帧那个位置附近接着找」只要 0.55（`TRACK_SCORE`）—— 位置本身就是证据。

use super::image::{half, resize, sobel_x, Gray};

/// 第一次认定「这里是这个图标」的分数（网页版的 `threshold = .7287…`）。
pub const DISCOVER_SCORE: f32 = 0.73;
/// 在上一帧位置附近接着认的分数。
pub const TRACK_SCORE: f32 = 0.55;
/// 接着找时，在上一帧位置周围多大范围内找（像素，原生分辨率）。
const TRACK_RADIUS: i64 = 6;
/// 粗搜保留几个候选去精修。
const COARSE_CANDIDATES: usize = 4;
/// 灰度 / 梯度两路的权重（网页版同款）。
const GRAY_WEIGHT: f32 = 0.45;
const EDGE_WEIGHT: f32 = 0.55;

/// 某一个尺度下准备好的模板：掩码内像素已经减均值、除以范数。
#[derive(Debug, Clone)]
struct Plane {
    width: usize,
    height: usize,
    /// 参与比较的像素在模板里的偏移
    offsets: Vec<(usize, usize)>,
    gray: Vec<f32>,
    edge: Vec<f32>,
}

impl Plane {
    fn new(gray: &Gray, keep: &[bool]) -> Option<Self> {
        let edge = sobel_x(gray);
        let mut offsets = Vec::new();
        for y in 0..gray.height {
            for x in 0..gray.width {
                if keep[y * gray.width + x] {
                    offsets.push((x, y));
                }
            }
        }
        let normalized = |src: &Gray| -> Option<Vec<f32>> {
            let values: Vec<f32> = offsets.iter().map(|&(x, y)| src.at(x, y)).collect();
            let mean = values.iter().sum::<f32>() / values.len().max(1) as f32;
            let centered: Vec<f32> = values.iter().map(|v| v - mean).collect();
            let norm = centered.iter().map(|v| v * v).sum::<f32>().sqrt();
            (norm > 1e-3).then(|| centered.iter().map(|v| v / norm).collect())
        };
        // 纯色块没有可比的东西：宁可不认，也别处处都「像」。
        let gray_vec = normalized(gray)?;
        let edge_vec = normalized(&edge).unwrap_or_else(|| vec![0.0; offsets.len()]);
        (offsets.len() >= 16).then_some(Self {
            width: gray.width,
            height: gray.height,
            offsets,
            gray: gray_vec,
            edge: edge_vec,
        })
    }

    /// 模板左上角放在 `(x, y)` 时的分数。
    fn score_at(&self, frame: &Features, x: usize, y: usize) -> f32 {
        let ncc = |template: &[f32], plane: &Gray| {
            let (mut sum, mut sq, mut dot) = (0.0f32, 0.0f32, 0.0f32);
            for (i, &(dx, dy)) in self.offsets.iter().enumerate() {
                let v = plane.data[(y + dy) * plane.width + x + dx];
                sum += v;
                sq += v * v;
                dot += v * template[i];
            }
            let n = self.offsets.len() as f32;
            let var = sq - sum * sum / n;
            if var <= 1e-3 {
                0.0
            } else {
                // 模板已经零均值，`dot` 里窗口均值那一项自然抵消
                dot / var.sqrt()
            }
        };
        GRAY_WEIGHT * ncc(&self.gray, &frame.gray) + EDGE_WEIGHT * ncc(&self.edge, &frame.edge)
    }
}

/// 一帧画面准备好的特征（原生 + 半分辨率）。
pub struct Features {
    pub gray: Gray,
    pub edge: Gray,
}

pub struct FramePyramid {
    pub full: Features,
    half: Features,
}

impl FramePyramid {
    pub fn new(gray: Gray) -> Self {
        let small = half(&gray);
        Self {
            full: Features { edge: sobel_x(&gray), gray },
            half: Features { edge: sobel_x(&small), gray: small },
        }
    }

    pub fn width(&self) -> usize {
        self.full.gray.width
    }

    pub fn height(&self) -> usize {
        self.full.gray.height
    }
}

/// 一个 BUFF 的模板（某个客户区高度下）。
#[derive(Debug, Clone)]
pub struct IconTemplate {
    full: Plane,
    half: Option<Plane>,
}

/// 找到的位置：左上角 + 边长（像素，相对传进来的画面）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Hit {
    pub x: usize,
    pub y: usize,
    pub width: usize,
    pub height: usize,
    pub score: f32,
}

/// 这个像素参与比较吗：剔掉最外一圈、和左下角的数字区。
fn keep_pixel(x: usize, y: usize, width: usize, height: usize) -> bool {
    let border = x == 0 || y == 0 || x + 1 == width || y + 1 == height;
    let digits = (x as f32) < width as f32 * 0.62 && (y as f32) >= height as f32 * 0.55;
    !border && !digits
}

impl IconTemplate {
    /// `gray` 是框下来的图标亮度，`opaque` 是 PNG 的不透明像素。
    pub fn new(gray: &Gray, opaque: &[bool]) -> Option<Self> {
        if gray.width < 8 || gray.height < 8 {
            return None;
        }
        let keep: Vec<bool> = (0..gray.width * gray.height)
            .map(|i| opaque[i] && keep_pixel(i % gray.width, i / gray.width, gray.width, gray.height))
            .collect();
        let full = Plane::new(gray, &keep)?;
        let small = half(gray);
        let small_keep: Vec<bool> = (0..small.width * small.height)
            .map(|i| {
                let (x, y) = (i % small.width, i / small.width);
                keep_pixel(x, y, small.width, small.height)
                    && opaque.get(y * 2 * gray.width + x * 2).copied().unwrap_or(false)
            })
            .collect();
        let half = (small.width >= 6 && small.height >= 6)
            .then(|| Plane::new(&small, &small_keep))
            .flatten();
        Some(Self { full, half })
    }

    /// 游戏分辨率变了：按比例缩放模板（比例接近 1 时原样返回）。
    pub fn scaled(gray: &Gray, opaque: &[bool], ratio: f32) -> Option<Self> {
        if (ratio - 1.0).abs() < 0.03 {
            return Self::new(gray, opaque);
        }
        let w = ((gray.width as f32 * ratio).round() as usize).max(8);
        let h = ((gray.height as f32 * ratio).round() as usize).max(8);
        let resized = resize(gray, w, h);
        let alpha = Gray {
            width: gray.width,
            height: gray.height,
            data: opaque.iter().map(|&o| if o { 1.0 } else { 0.0 }).collect(),
        };
        let alpha = resize(&alpha, w, h);
        let opaque: Vec<bool> = alpha.data.iter().map(|&v| v >= 0.5).collect();
        Self::new(&resized, &opaque)
    }

    fn hit(&self, x: usize, y: usize, score: f32) -> Hit {
        Hit { x, y, width: self.full.width, height: self.full.height, score }
    }

    /// 在 `(cx, cy)` 周围 `radius` 内找最高分（原生分辨率）。
    fn refine(&self, frame: &Features, cx: i64, cy: i64, radius: i64) -> Option<Hit> {
        let max_x = frame.gray.width as i64 - self.full.width as i64;
        let max_y = frame.gray.height as i64 - self.full.height as i64;
        if max_x < 0 || max_y < 0 {
            return None;
        }
        let mut best: Option<Hit> = None;
        for y in (cy - radius).max(0)..=(cy + radius).min(max_y) {
            for x in (cx - radius).max(0)..=(cx + radius).min(max_x) {
                let score = self.full.score_at(frame, x as usize, y as usize);
                if best.map_or(true, |b| score > b.score) {
                    best = Some(self.hit(x as usize, y as usize, score));
                }
            }
        }
        best
    }

    /// 接着上一帧的位置找（只看附近）。分数够 `TRACK_SCORE` 才算。
    pub fn track(&self, frame: &FramePyramid, previous: &Hit) -> Option<Hit> {
        self.refine(&frame.full, previous.x as i64, previous.y as i64, TRACK_RADIUS)
            .filter(|hit| hit.score >= TRACK_SCORE)
    }

    /// 整幅画面里找：半分辨率粗搜出几个候选，再回原生分辨率精修。
    /// 分数够 `DISCOVER_SCORE` 才算。
    pub fn discover(&self, frame: &FramePyramid) -> Option<Hit> {
        let best = match &self.half {
            Some(small) => {
                let (fw, fh) = (frame.half.gray.width, frame.half.gray.height);
                if fw < small.width || fh < small.height {
                    return None;
                }
                let mut candidates: Vec<(f32, usize, usize)> = Vec::new();
                for y in 0..=fh - small.height {
                    for x in 0..=fw - small.width {
                        let score = small.score_at(&frame.half, x, y);
                        // 粗搜的分数天然偏低（细节被平均掉了），门槛放宽一截
                        if score < DISCOVER_SCORE - 0.2 {
                            continue;
                        }
                        insert_candidate(&mut candidates, (score, x, y), small.width / 2);
                    }
                }
                candidates
                    .iter()
                    .filter_map(|&(_, x, y)| {
                        self.refine(&frame.full, x as i64 * 2, y as i64 * 2, 2)
                    })
                    .max_by(|a, b| a.score.total_cmp(&b.score))
            }
            // 太小的模板没有半分辨率版：直接原生整幅搜
            None => {
                let (fw, fh) = (frame.width(), frame.height());
                if fw < self.full.width || fh < self.full.height {
                    return None;
                }
                let mut best: Option<Hit> = None;
                for y in 0..=fh - self.full.height {
                    for x in 0..=fw - self.full.width {
                        let score = self.full.score_at(&frame.full, x, y);
                        if best.map_or(true, |b| score > b.score) {
                            best = Some(self.hit(x, y, score));
                        }
                    }
                }
                best
            }
        };
        best.filter(|hit| hit.score >= DISCOVER_SCORE)
    }
}

/// 维护「分数最高的几个、彼此不挨着」的候选表。
fn insert_candidate(list: &mut Vec<(f32, usize, usize)>, item: (f32, usize, usize), spacing: usize) {
    let near = |a: &(f32, usize, usize)| a.1.abs_diff(item.1) <= spacing && a.2.abs_diff(item.2) <= spacing;
    if let Some(pos) = list.iter().position(near) {
        if list[pos].0 >= item.0 {
            return;
        }
        list.remove(pos);
    }
    list.push(item);
    list.sort_by(|a, b| b.0.total_cmp(&a.0));
    list.truncate(COARSE_CANDIDATES);
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// 一个有棱有角的假图标（几块不同亮度 + 斜线），尺寸 `size`。
    pub(crate) fn fake_icon(size: usize, seed: u32) -> Gray {
        let mut g = Gray::new(size, size);
        // 系数随 seed 变：不同 seed 是不同的花纹，不只是同一张图平移
        let s = seed as usize;
        let (a, b, c) = (3 + 2 * s, 5 + s, 1 + s % 3);
        for y in 0..size {
            for x in 0..size {
                let mut v = 40.0 + ((x * a + y * b) % 23) as f32 * 4.0;
                if (x * c + 2 * y + s) % 9 < 3 {
                    v += 120.0;
                }
                // 亮块的位置也随 seed 变（奇数在右上，偶数在右侧中段）
                let band = if s % 2 == 1 { 0..size / 3 } else { size / 3..size * 2 / 3 };
                if x > size / 2 && band.contains(&y) {
                    v = 230.0 - (seed as f32) * 20.0;
                }
                g.data[y * size + x] = v.min(255.0);
            }
        }
        g
    }

    /// 平滑但有起伏的背景（像游戏场景），把图标贴在 `(ox, oy)`。
    pub(crate) fn scene(width: usize, height: usize, icon: &Gray, ox: usize, oy: usize) -> Gray {
        let mut g = Gray::new(width, height);
        for y in 0..height {
            for x in 0..width {
                g.data[y * width + x] = 90.0 + 30.0 * ((x as f32) * 0.05).sin() + (y % 17) as f32;
            }
        }
        for y in 0..icon.height {
            for x in 0..icon.width {
                g.data[(oy + y) * width + ox + x] = icon.at(x, y);
            }
        }
        g
    }

    #[test]
    fn finds_the_icon_and_not_a_different_one() {
        let icon = fake_icon(32, 1);
        let other = fake_icon(32, 4);
        let frame = FramePyramid::new(scene(400, 120, &icon, 263, 41));
        let template = IconTemplate::new(&icon, &vec![true; 32 * 32]).unwrap();
        let hit = template.discover(&frame).expect("应该找到");
        assert_eq!((hit.x, hit.y), (263, 41));
        assert!(hit.score > 0.95, "原样贴上去应该接近满分：{}", hit.score);

        let wrong = IconTemplate::new(&other, &vec![true; 32 * 32]).unwrap();
        let found = wrong.discover(&frame);
        assert!(found.is_none(), "别的图标不能认成它：{found:?}");
    }

    #[test]
    fn digits_in_the_lower_left_do_not_break_matching() {
        let icon = fake_icon(32, 2);
        let mut shown = icon.clone();
        // 左下角画上白色数字块（会变的倒计时）
        for y in 20..30 {
            for x in 2..16 {
                shown.data[y * 32 + x] = if (x + y) % 3 == 0 { 255.0 } else { 10.0 };
            }
        }
        let frame = FramePyramid::new(scene(300, 100, &shown, 120, 30));
        let template = IconTemplate::new(&icon, &vec![true; 32 * 32]).unwrap();
        let hit = template.discover(&frame).expect("数字区被剔掉，照样要找到");
        assert_eq!((hit.x, hit.y), (120, 30));
    }

    #[test]
    fn tracking_tolerates_a_darkened_icon_near_the_old_spot() {
        let icon = fake_icon(32, 3);
        let mut dim = icon.clone();
        // 快到期：上半截被压暗、整体变灰
        for y in 0..32 {
            for x in 0..32 {
                let v = &mut dim.data[y * 32 + x];
                *v = if y < 14 { *v * 0.45 } else { *v * 0.8 + 20.0 };
            }
        }
        let template = IconTemplate::new(&icon, &vec![true; 32 * 32]).unwrap();
        let frame = FramePyramid::new(scene(300, 100, &dim, 150, 40));
        let previous = Hit { x: 148, y: 41, width: 32, height: 32, score: 0.9 };
        let hit = template.track(&frame, &previous).expect("附近接着认");
        assert_eq!((hit.x, hit.y), (150, 40));
    }

    #[test]
    fn scaled_template_matches_a_larger_render() {
        let icon = fake_icon(32, 5);
        let big = resize(&icon, 48, 48);
        let frame = FramePyramid::new(scene(400, 140, &big, 200, 50));
        let template = IconTemplate::scaled(&icon, &vec![true; 32 * 32], 1.5).unwrap();
        let hit = template.discover(&frame).expect("按比例放大后应该找到");
        assert!(hit.x.abs_diff(200) <= 1 && hit.y.abs_diff(50) <= 1, "{hit:?}");
    }
}
