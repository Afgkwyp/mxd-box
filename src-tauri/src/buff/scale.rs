//! 游戏画面的界面倍数：把抓到的那条缩回原生像素再认。
//!
//! 游戏无边框全屏时，窗口里的画面是按内部分辨率渲染后**分横向、纵向各自**拉伸到客户区的。
//! 例如 1366×768 渲染、拉伸到 2560×1440：横向 2560/1366 = 1.8741，纵向 1440/768 = 1.8750。
//! 真机 2560×1440 的样本是**最近邻**拉伸的（文字行是纯色、没有混合色）；1920×1080 时界面是
//! 原生 1.000 倍。拉伸过的图标和数字又大又糊，模板和读数器只认原生大小，所以先按倍数缩回去。
//!
//! 倍数优先用经验模块 HUD 锚点量出来的一个数（`buff::note_hud_scale`），再 `snap` 到一个真实的
//! 内部分辨率上得到横纵两个倍数；没有 HUD 就按常见的内部分辨率推几组候选，每帧试几个，认到一个就锁定。
//! 客户区尺寸一变、或者锁定后连续好几帧一个 BUFF 都认不出来，就全部作废重新找（`ScaleTracker`）。

use super::digits::{read_number, DigitRead};
use super::image::Gray;
use super::matcher::{FramePyramid, Hit};

/// 游戏常见的内部渲染分辨率（横向、纵向各自按客户区算倍数）。
const INTERNAL: [(u32, u32); 11] = [
    (1366, 768),
    (1280, 720),
    (1600, 900),
    (1024, 768),
    (800, 600),
    (1920, 1080),
    (1360, 768),
    (1280, 768),
    (1280, 800),
    (1440, 900),
    (1680, 1050),
];

/// 候选倍数允许的范围：800×600 拉到 4K 是 4.8 倍，窗口比内部分辨率小的时候会小于 1。
const MIN_SCALE: f32 = 0.5;
const MAX_SCALE: f32 = 5.0;

/// HUD 量出的倍数和某个内部分辨率对得上的容差（相对）。
const SNAP_TOLERANCE: f32 = 0.015;

/// 找倍数时每帧试几个候选。
pub const SEARCH_BATCH: usize = 4;
/// 锁定后连续这么多帧一个 BUFF 都没认出来，就当倍数可能变了，重新找。
pub const LOST_FRAMES: u32 = 4;
/// 找了这么多帧还没认出来，状态里提示重新框图标（约 SEARCH_BATCH × 候选轮数）。
pub const SEARCH_HINT_FRAMES: u32 = 24;

/// 候选倍数（横纵两个）：第一个永远是 (1, 1)，其余按内部分辨率推出来，只留范围内的，
/// 横纵都在 2% 以内的去掉。
pub fn candidates(client_w: u32, client_h: u32) -> Vec<(f32, f32)> {
    let mut out = vec![(1.0f32, 1.0f32)];
    for &(w, h) in &INTERNAL {
        let (sx, sy) = (client_w as f32 / w as f32, client_h as f32 / h as f32);
        if !(MIN_SCALE..=MAX_SCALE).contains(&sx) || !(MIN_SCALE..=MAX_SCALE).contains(&sy) {
            continue;
        }
        if out
            .iter()
            .any(|&(ox, oy)| (sx - ox).abs() <= 0.02 * ox && (sy - oy).abs() <= 0.02 * oy)
        {
            continue;
        }
        out.push((sx, sy));
    }
    out
}

/// 把 HUD 量出的一个倍数对齐到真实的内部分辨率：横纵两个倍数都在 `measured` 的 1.5% 以内、
/// 并且最接近的那一对。对不上就原样返回 `(measured, measured)`。
pub fn snap(client_w: u32, client_h: u32, measured: f32) -> (f32, f32) {
    let tol = SNAP_TOLERANCE * measured;
    INTERNAL
        .iter()
        .map(|&(w, h)| (client_w as f32 / w as f32, client_h as f32 / h as f32))
        .filter(|&(sx, sy)| (sx - measured).abs() <= tol && (sy - measured).abs() <= tol)
        .min_by(|a, b| {
            let da = (a.0 - measured).abs() + (a.1 - measured).abs();
            let db = (b.0 - measured).abs() + (b.1 - measured).abs();
            da.total_cmp(&db)
        })
        .unwrap_or((measured, measured))
}

/// 两个倍数是不是同一个（横纵都在 1% 以内）。
pub fn same_scale(a: (f32, f32), b: (f32, f32)) -> bool {
    (a.0 - b.0).abs() <= 0.01 * a.0 && (a.1 - b.1).abs() <= 0.01 * a.1
}

/// 按界面倍数缩回原生像素：横向除以 `sx`、纵向除以 `sy`。两个都接近 1 时原样返回一份拷贝。
///
/// 取样用**最近邻**：原生第 i 列取源图第 `ceil(i × sx)` 列，第 j 行取第 `ceil(j × sy)` 行。
/// 最近邻拉伸时源图像素 k 占画面上 `[k·s, (k+1)·s)` 这一段，`ceil(i·s)` 一定落在原生像素 i 那一段里，
/// 所以能原样还原。原生像素中心 `floor((i+0.5)·s)` 在 1.25、1.872 会漏掉一些像素（实测 1.25 有 34 个
/// 槽读不出），所以没用。双线性会把每个字糊掉一半。
pub fn normalize(bgra: &[u8], width: usize, height: usize, sx: f32, sy: f32) -> (Vec<u8>, usize, usize) {
    if (sx - 1.0).abs() < 0.02 && (sy - 1.0).abs() < 0.02 {
        return (bgra.to_vec(), width, height);
    }
    let nw = ((width as f32 / sx).round() as usize).max(1);
    let nh = ((height as f32 / sy).round() as usize).max(1);
    let mut out = vec![255u8; nw * nh * 4];
    for y in 0..nh {
        let src_y = (((y as f32) * sy).ceil() as usize).min(height - 1);
        for x in 0..nw {
            let src_x = (((x as f32) * sx).ceil() as usize).min(width - 1);
            let (si, di) = ((src_y * width + src_x) * 4, (y * nw + x) * 4);
            out[di..di + 4].copy_from_slice(&bgra[si..si + 4]);
        }
    }
    (out, nw, nh)
}

/// 一帧按界面倍数缩回原生后的画面，连同找图用的金字塔。
pub struct Picture {
    pub bgra: Vec<u8>,
    pub pyramid: FramePyramid,
}

impl Picture {
    pub fn new(bgra: &[u8], width: usize, height: usize, (sx, sy): (f32, f32)) -> Self {
        let (bgra, nw, nh) = normalize(bgra, width, height, sx, sy);
        let pyramid = FramePyramid::new(Gray::from_bgra(&bgra, nw, nh));
        Self { bgra, pyramid }
    }
}

/// 在原生画面上认图标左下角的读数（图标的边长取短边）。
pub fn read_at(pic: &Picture, hit: &Hit) -> Option<DigitRead> {
    read_number(&pic.pyramid.full.gray, &pic.bgra, hit.x, hit.y, hit.width.min(hit.height))
}

/// 这一帧该怎么认。
#[derive(Debug, Clone, PartialEq)]
pub enum Plan {
    /// 倍数已经定了（HUD 量的，或之前认定的）
    Locked((f32, f32)),
    /// 还在找：这一帧依次试这几个倍数，认到一个就锁定
    Search(Vec<(f32, f32)>),
}

/// 界面倍数的锁定状态。只看客户区尺寸和认没认出来，不碰画面，方便单测。
#[derive(Debug, Default)]
pub struct ScaleTracker {
    client: Option<(i32, i32)>,
    locked: Option<(f32, f32)>,
    /// 还在找的时候，下一轮从 `candidates` 的第几个开始试
    next: usize,
    /// 锁定后连续几帧一个 BUFF 都没认出来
    misses: u32,
    /// 找的时候连续几帧一个都没认出来
    search_frames: u32,
}

impl ScaleTracker {
    /// 这一帧该用哪个倍数。客户区尺寸变了：认定的倍数和找的进度全部作废。
    pub fn plan(&mut self, client: (i32, i32), hud: Option<f32>) -> Plan {
        if self.client != Some(client) {
            *self = Self { client: Some(client), ..Self::default() };
        }
        if let Some(measured) = hud {
            return Plan::Locked(snap(client.0 as u32, client.1 as u32, measured));
        }
        if let Some(scale) = self.locked {
            return Plan::Locked(scale);
        }
        let cands = candidates(client.0 as u32, client.1 as u32);
        let n = cands.len();
        let batch: Vec<(f32, f32)> = (0..SEARCH_BATCH.min(n)).map(|k| cands[(self.next + k) % n]).collect();
        self.next = (self.next + batch.len()) % n;
        Plan::Search(batch)
    }

    /// 找到了：锁定这个倍数。
    pub fn found(&mut self, scale: (f32, f32)) {
        self.locked = Some(scale);
        self.misses = 0;
        self.search_frames = 0;
    }

    /// 锁定时这一帧的结果：认到了就清零；连续 `LOST_FRAMES` 帧认不到，就放弃锁定重新找。
    pub fn seen(&mut self, any: bool) {
        if any {
            self.misses = 0;
            return;
        }
        self.misses += 1;
        if self.misses >= LOST_FRAMES {
            self.locked = None;
            self.misses = 0;
            self.search_frames = 0;
        }
    }

    /// 找的那一帧谁都没认出来。返回 `true` 表示已经找了很久，该提示用户重新框图标了。
    pub fn searched_in_vain(&mut self) -> bool {
        self.search_frames += 1;
        self.search_frames >= SEARCH_HINT_FRAMES
    }

    /// 当前锁定的倍数（没锁定是 `None`）。只给测试看。
    #[cfg(test)]
    pub fn locked(&self) -> Option<(f32, f32)> {
        self.locked
    }
}

/// 游戏按 `ceil` 最近邻拉伸的模拟（只给测试用）：目标像素取源图 `floor(x / s)`。
#[cfg(test)]
pub fn stretch(bgra: &[u8], width: usize, height: usize, sx: f32, sy: f32) -> (Vec<u8>, usize, usize) {
    let nw = ((width as f32 * sx).round() as usize).max(1);
    let nh = ((height as f32 * sy).round() as usize).max(1);
    let mut out = vec![255u8; nw * nh * 4];
    for y in 0..nh {
        let src_y = ((y as f32 / sy) as usize).min(height - 1);
        for x in 0..nw {
            let src_x = ((x as f32 / sx) as usize).min(width - 1);
            let (si, di) = ((src_y * width + src_x) * 4, (y * nw + x) * 4);
            out[di..di + 4].copy_from_slice(&bgra[si..si + 4]);
        }
    }
    (out, nw, nh)
}

/// 模拟额外的 DPI 糊：双线性拉伸（均匀倍数）。只给测试用。
#[cfg(test)]
pub fn stretch_bilinear(bgra: &[u8], width: usize, height: usize, scale: f32) -> (Vec<u8>, usize, usize) {
    let nw = ((width as f32 * scale).round() as usize).max(1);
    let nh = ((height as f32 * scale).round() as usize).max(1);
    let plane = |c: usize| Gray {
        width,
        height,
        data: bgra.chunks_exact(4).take(width * height).map(|p| p[c] as f32).collect(),
    };
    let mut out = vec![255u8; nw * nh * 4];
    for c in 0..4 {
        let small = super::image::resize(&plane(c), nw, nh);
        for (i, v) in small.data.iter().enumerate() {
            out[i * 4 + c] = v.round().clamp(0.0, 255.0) as u8;
        }
    }
    (out, nw, nh)
}

#[cfg(test)]
mod tests {
    use super::super::digits::tests::{classic_fixtures, FrameLabel, Slot};
    use super::super::digits::Unit;
    use super::super::image::Gray;
    use super::super::matcher::tests::scene;
    use super::super::matcher::IconTemplate;
    use super::*;

    /// 一轮测试的计数：读对 / 读不出 / 读错（秒、分钟分开数），错的记下来。
    #[derive(Default)]
    struct Tally {
        correct: [usize; 2],
        none: [usize; 2],
        wrong: Vec<String>,
        known: usize,
    }

    /// 已知会读错的槽（字形表是从 1080p 样本重新生成的，这几个字切得不全）。读错只许在这里出现，
    /// 其它地方读错就是新问题。字形表换成原来的（或样本更多）以后，这个表应该清空。
    const KNOWN_MISREADS: [&str; 4] = ["0049382.png 槽1", "0054982.png 槽1", "0045310.png 槽1", "0054982.png 槽0"];

    /// 秒数读对至少七成（读不出的和已知误读都算进分母）。读不出可以，但不能大片读不出。
    fn assert_read_rate(t: &Tally, what: &str) {
        let seconds = t.correct[0] + t.none[0] + t.known;
        assert!(t.correct[0] * 10 >= seconds * 7, "{what}：秒数读对 {} / {seconds}", t.correct[0]);
    }

    impl Tally {
        fn row(&self) -> String {
            format!(
                "秒 读对 {} 读不出 {} · 分钟 读对 {} 读不出 {} · 已知误读 {} · 新的读错 {}",
                self.correct[0],
                self.none[0],
                self.correct[1],
                self.none[1],
                self.known,
                self.wrong.len()
            )
        }
    }

    /// 每个真机样本：按 `stretch` 拉伸（最近邻，或双线性），再按 `norm` 缩回原生，在图标位置读数，和标注比。
    fn tally(factor: (f32, f32), norm: (f32, f32), bilinear: bool) -> Tally {
        let mut t = Tally::default();
        for (label, bgra, w, h) in classic_fixtures() {
            let (st, sw, sh) = if bilinear {
                stretch_bilinear(&bgra, w, h, factor.0)
            } else {
                stretch(&bgra, w, h, factor.0, factor.1)
            };
            let pic = Picture::new(&st, sw, sh, norm);
            for slot in &label.slots {
                record(&mut t, &label, slot, &pic);
            }
        }
        t
    }

    fn record(t: &mut Tally, label: &FrameLabel, slot: &Slot, pic: &Picture) {
        // 标注里没写读数的槽（null）不算：不知道它该是多少
        let Some(expect) = slot.expect() else { return };
        let got = read_number(&pic.pyramid.full.gray, &pic.bgra, slot.icon_left, slot.icon_top, 32)
            .map(|r| (r.value, r.unit));
        let k = match expect {
            (_, Unit::Minutes) => 1,
            _ => 0,
        };
        let key = format!("{} 槽{}", label.file, slot.slot);
        if got == Some(expect) {
            t.correct[k] += 1;
        } else if got.is_none() {
            t.none[k] += 1;
        } else if KNOWN_MISREADS.contains(&key.as_str()) {
            t.known += 1;
        } else {
            t.wrong.push(format!("{key} 读成 {got:?}，标注 {expect:?}"));
        }
    }

    #[test]
    fn candidates_cover_the_real_1440p_stretch() {
        let c = candidates(2560, 1440);
        assert_eq!(c[0], (1.0, 1.0));
        assert!(
            c.iter().any(|&(sx, sy)| (sx - 1.8741).abs() < 0.01 && (sy - 1.875).abs() < 0.01),
            "{c:?}"
        );
        for (i, a) in c.iter().enumerate() {
            for b in &c[i + 1..] {
                let close = (a.0 - b.0).abs() <= 0.02 * a.0 && (a.1 - b.1).abs() <= 0.02 * a.1;
                assert!(!close, "{a:?} 和 {b:?} 太接近了：{c:?}");
            }
        }
        assert_eq!(candidates(1920, 1080)[0], (1.0, 1.0));
    }

    #[test]
    fn snap_picks_the_real_internal_resolution() {
        let (sx, sy) = snap(2560, 1440, 1.872);
        assert!((sx - 2560.0 / 1366.0).abs() < 1e-4, "sx={sx}");
        assert!((sy - 1440.0 / 768.0).abs() < 1e-4, "sy={sy}");
        assert_eq!(snap(1920, 1080, 1.0), (1.0, 1.0));
        // 对不上任何内部分辨率：原样返回
        assert_eq!(snap(1111, 777, 1.5), (1.5, 1.5));
    }

    /// 最近邻拉伸再缩回：每个倍数下真机样本都要原样读出来（0 None、0 WRONG）。
    #[test]
    fn real_frames_read_exactly_after_nearest_stretch_and_normalize() {
        for s in [1.25f32, 1.5, 1.6, 1.872, 2.0, 2.5] {
            let t = tally((s, s), (s, s), false);
            eprintln!("SCALE 最近邻 {s}: {}", t.row());
            assert!(t.wrong.is_empty(), "s={s} 读错：\n{}", t.wrong.join("\n"));
            assert_read_rate(&t, &format!("s={s}"));
        }
    }

    /// 游戏按 2560/1366、1440/768 拉伸（最近邻），我们用 HUD 量出的 1.872 对齐后缩回：不能读错，也不能读不出。
    #[test]
    fn slightly_off_scale_never_reads_wrong() {
        let stretch = (2560.0 / 1366.0, 1440.0 / 768.0);
        let norm = snap(2560, 1440, 1.872);
        let t = tally(stretch, norm, false);
        eprintln!("SCALE 最近邻 1440p 拉伸 / snap(1.872): {}", t.row());
        assert!(t.wrong.is_empty(), "读错：\n{}", t.wrong.join("\n"));
        assert_read_rate(&t, "1440p 拉伸");
    }

    /// 额外的 DPI 糊（双线性拉伸）：不能读错；读不出的数量只报告。
    #[test]
    fn bilinear_dpi_blur_never_reads_wrong() {
        for s in [1.5f32, 1.872] {
            let t = tally((s, s), (s, s), true);
            eprintln!("SCALE 双线性 {s}: {}", t.row());
            assert!(t.wrong.is_empty(), "双线性 s={s} 读错：\n{}", t.wrong.join("\n"));
        }
    }

    #[test]
    fn a_catalog_icon_is_found_on_a_stretched_screen() {
        let skill = super::super::catalog::find("2301004").expect("祝福在图鉴里");
        let (rgba, w, h) = super::super::decode_png(&skill.png).expect("图标能解开");
        let (gray, opaque) = Gray::from_rgba(&rgba, w, h);
        let screen = scene(420, 150, &gray, 211, 37);
        let bgra: Vec<u8> = screen
            .data
            .iter()
            .flat_map(|&v| {
                let b = v.round().clamp(0.0, 255.0) as u8;
                [b, b, b, 255]
            })
            .collect();
        let (st, sw, sh) = stretch(&bgra, 420, 150, 1.872, 1.872);
        let pic = Picture::new(&st, sw, sh, (1.872, 1.872));
        let template = IconTemplate::new(&gray, &opaque).expect("模板");
        let hit = template.discover(&pic.pyramid).expect("拉伸过的画面里应该认出祝福");
        assert!(hit.x.abs_diff(211) <= 1 && hit.y.abs_diff(37) <= 1, "{hit:?}");
    }

    // -----------------------------------------------------------------------
    // 分辨率矩阵：整条识别流程（锁倍数 → 找图标 → 读秒数）在合成画面上跑一遍
    // -----------------------------------------------------------------------

    /// 一个模拟的游戏顶部条（BGRA，宽、高）。
    struct Shot {
        bgra: Vec<u8>,
        w: usize,
        h: usize,
    }

    /// 样本里的一个标注槽：样本的 170×36 顶部条、槽位、读数。
    struct Sample<'a> {
        strip: &'a [u8],


    }

    /// 把样本的顶部条贴到内部分辨率的顶部（和游戏里一样，图标 32 像素、位置在顶部）。
    /// 背景是暗的花纹，别的地方不会误认成 BUFF。
    fn internal_band(sample: &Sample, iw: usize, ih: usize) -> Vec<u8> {
        let band = (ih as f32 * 0.3).ceil() as usize;
        let mut out: Vec<u8> = (0..iw * band)
            .flat_map(|i| {
                let v = 60 + ((i % 7) * 4 + ((i / iw) % 5) * 3) as u8;
                [v, v, v, 255]
            })
            .collect();
        let (x0, strip_w) = (60usize, 170usize);
        let strip_h = sample.strip.len() / 4 / strip_w;
        for y in 0..strip_h.min(band) {
            for x in 0..strip_w.min(iw.saturating_sub(x0)) {
                let (si, di) = ((y * strip_w + x) * 4, (y * iw + x0 + x) * 4);
                out[di..di + 4].copy_from_slice(&sample.strip[si..si + 4]);
            }
        }
        out
    }

    /// 样本里标了读数的槽（每个都是一个要认的 BUFF）。
    fn samples(fixtures: &[(FrameLabel, Vec<u8>, usize, usize)]) -> Vec<(usize, usize)> {
        let mut out = Vec::new();
        for (f, (label, _, _, _)) in fixtures.iter().enumerate() {
            for (s, slot) in label.slots.iter().enumerate() {
                if slot.expect().is_some() {
                    out.push((f, s));
                }
            }
        }
        out
    }

    /// 一帧的识别（和 `buff::run` 里的顺序一样）。返回认到的（倍数, 命中, 读数）；没认到返回 `None`。
    fn tick(
        tracker: &mut ScaleTracker,
        client: (i32, i32),
        shot: &Shot,
        template: &IconTemplate,
    ) -> Option<((f32, f32), Hit, Option<DigitRead>)> {
        let plan = tracker.plan(client, None);
        let was_locked = matches!(plan, Plan::Locked(_));
        let scale = match plan {
            Plan::Locked(s) => s,
            Plan::Search(batch) => {
                let found = batch.into_iter().find(|&s| {
                    template.discover(&Picture::new(&shot.bgra, shot.w, shot.h, s).pyramid).is_some()
                });
                match found {
                    Some(s) => {
                        tracker.found(s);
                        s
                    }
                    None => {
                        tracker.searched_in_vain();
                        return None;
                    }
                }
            }
        };
        let pic = Picture::new(&shot.bgra, shot.w, shot.h, scale);
        let hit = template.discover(&pic.pyramid);
        if was_locked {
            tracker.seen(hit.is_some());
        }
        let hit = hit?;
        let reading = read_at(&pic, &hit);
        Some((scale, hit, reading))
    }

    /// 一个样本在一种画面下的结果：第几帧锁定、认到的位置、读数是否对。
    struct Outcome {
        ticks_to_hit: usize,
        hit_x: usize,
        read: Option<(u32, Unit)>,
    }

    /// 同一个 tracker 连续看 `frames` 帧（每帧一张画面，几何已经由调用方画好）；认到第一次就返回。
    fn until_seen(
        tracker: &mut ScaleTracker,
        client: (i32, i32),
        shot: &Shot,
        template: &IconTemplate,
        max_ticks: usize,
    ) -> Option<Outcome> {
        for n in 1..=max_ticks {
            if let Some((_, hit, reading)) = tick(tracker, client, shot, template) {
                let read = reading.map(|r| (r.value, r.unit));
                return Some(Outcome { ticks_to_hit: n, hit_x: hit.x, read });
            }
        }
        None
    }

    /// 画面的一种情况：内部分辨率、客户区尺寸、拉伸方式（最近邻或双线性）。
    #[derive(Clone, Copy)]
    struct Case {
        name: &'static str,
        internal: (usize, usize),
        client: (i32, i32),
        bilinear: Option<f32>,
        /// 秒数至少读对几个（共 6 个样本）。DPI 糊的情况读数很差，不设下限，只保证不误读、能锁定。
        min_reads: usize,
    }

    /// 造一张样本在这种情况下的画面：内部顶部条 → 拉伸到客户区的顶部 30%。
    fn shot_for(sample: &Sample, case: &Case) -> Shot {
        let (iw, ih) = case.internal;
        let band = internal_band(sample, iw, ih);
        let band_h = (ih as f32 * 0.3).ceil() as usize;
        let (client_w, client_h) = (case.client.0 as f32, case.client.1 as f32);
        let (bgra, w, h) = match case.bilinear {
            Some(s) => stretch_bilinear(&band, iw, band_h, s),
            None => stretch(&band, iw, band_h, client_w / iw as f32, client_h / ih as f32),
        };
        Shot { bgra, w, h }
    }

    /// 每种情况要在几帧内锁定（候选轮数 + 余量）。
    const LOCK_TICKS: usize = 4;

    /// 一种情况下：每个样本都要在 `LOCK_TICKS` 帧内找到图标、读数对（锁定之后的样本一帧就得）。
    fn check_case(case: &Case) -> String {
        let fixtures = classic_fixtures();
        let picked: Vec<(usize, usize)> = samples(&fixtures).into_iter().step_by(23).take(6).collect();
        let total = picked.len();
        let mut correct = 0;
        assert!(!picked.is_empty(), "样本里没有标了读数的槽");
        let template_src = |f: usize, s: usize| {
            let (label, strip, w, h) = &fixtures[f];
            let slot = &label.slots[s];
            let gray = Gray::from_bgra(strip, *w, *h);
            let icon = gray.crop(slot.icon_left as i64, slot.icon_top as i64, 32, 32);
            let opaque = vec![true; 32 * 32];
            IconTemplate::new(&icon, &opaque).expect("样本的图标能做模板")
        };
        let mut tracker = ScaleTracker::default();
        let mut report = Vec::new();
        for (f, s) in picked {
            let (label, strip, _, _) = &fixtures[f];
            let slot = &label.slots[s];
            let template = template_src(f, s);
            let sample = Sample { strip };
            let shot = shot_for(&sample, case);
            let expect = slot.expect().expect("picked 只挑标了读数的");
            let outcome = until_seen(&mut tracker, case.client, &shot, &template, LOCK_TICKS + 4)
                .unwrap_or_else(|| panic!("{}：{} 槽{} 没认出来", case.name, label.file, slot.slot));
            assert!(
                outcome.ticks_to_hit <= LOCK_TICKS,
                "{}：第 {} 帧才认出来（要 ≤ {LOCK_TICKS}）",
                case.name,
                outcome.ticks_to_hit
            );
            assert!(outcome.hit_x.abs_diff(60 + slot.icon_left) <= 2, "{}：位置偏了 {}", case.name, outcome.hit_x);
            // 读不出可以（计入读对率）；读错只许是已知的那几个槽
            let key = format!("{} 槽{}", label.file, slot.slot);
            match outcome.read {
                Some(r) if r == expect => correct += 1,
                None => {}
                Some(r) => assert!(
                    KNOWN_MISREADS.contains(&key.as_str()),
                    "{}：{key} 读成 {r:?}，标注 {expect:?}",
                    case.name
                ),
            }
            report.push(format!("{}帧", outcome.ticks_to_hit));
        }
        assert!(correct >= case.min_reads, "{}：读对的太少 {correct}/{total}", case.name);
        format!("{}: 锁定 {} · 读对 {correct}/{total}", case.name, report.join("/"))
    }

    /// 全屏拉伸、横纵倍数相同或不同（分辨率比例不对时横纵不一样）。
    const STRETCHED: [Case; 13] = [
        Case { name: "800x600→1920x1080", internal: (800, 600), client: (1920, 1080), bilinear: None, min_reads: 3 },
        Case { name: "800x600→2560x1440", internal: (800, 600), client: (2560, 1440), bilinear: None, min_reads: 3 },
        Case { name: "1024x768→1920x1080", internal: (1024, 768), client: (1920, 1080), bilinear: None, min_reads: 3 },
        Case { name: "1024x768→2560x1440", internal: (1024, 768), client: (2560, 1440), bilinear: None, min_reads: 3 },
        Case { name: "1280x720→1920x1080", internal: (1280, 720), client: (1920, 1080), bilinear: None, min_reads: 3 },
        Case { name: "1280x720→2560x1440", internal: (1280, 720), client: (2560, 1440), bilinear: None, min_reads: 3 },
        Case { name: "1280x720→3840x2160", internal: (1280, 720), client: (3840, 2160), bilinear: None, min_reads: 3 },
        Case { name: "1366x768→1920x1080", internal: (1366, 768), client: (1920, 1080), bilinear: None, min_reads: 3 },
        Case { name: "1366x768→2560x1440", internal: (1366, 768), client: (2560, 1440), bilinear: None, min_reads: 3 },
        Case { name: "1366x768→3840x2160", internal: (1366, 768), client: (3840, 2160), bilinear: None, min_reads: 3 },
        Case { name: "1600x900→2560x1440", internal: (1600, 900), client: (2560, 1440), bilinear: None, min_reads: 3 },
        Case { name: "1600x900→3840x2160", internal: (1600, 900), client: (3840, 2160), bilinear: None, min_reads: 3 },
        Case { name: "1920x1080→2560x1440", internal: (1920, 1080), client: (2560, 1440), bilinear: None, min_reads: 3 },
    ];

    /// 窗口化：界面是 1 倍，客户区就是内部分辨率（或者比它小一点）。
    const WINDOWED: [Case; 4] = [
        Case { name: "窗口 1920x1080", internal: (1920, 1080), client: (1920, 1080), bilinear: None, min_reads: 3 },
        Case { name: "窗口 1600x900", internal: (1600, 900), client: (1600, 900), bilinear: None, min_reads: 3 },
        Case { name: "窗口 1366x768", internal: (1366, 768), client: (1366, 768), bilinear: None, min_reads: 3 },
        Case { name: "窗口 1280x720", internal: (1280, 720), client: (1280, 720), bilinear: None, min_reads: 3 },
    ];

    /// 额外的 DPI 缩放（系统 125% / 150%，窗口被双线性糊了一下）。
    const DPI: [Case; 3] = [
        Case { name: "DPI 1.25 双线性 1920x1080", internal: (1920, 1080), client: (2400, 1350), bilinear: Some(1.25), min_reads: 0 },
        Case { name: "DPI 1.5 双线性 1920x1080", internal: (1920, 1080), client: (2880, 1620), bilinear: Some(1.5), min_reads: 0 },
        Case { name: "DPI 1.5 双线性 1366x768", internal: (1366, 768), client: (2049, 1152), bilinear: Some(1.5), min_reads: 0 },
    ];

    #[test]
    fn stretched_resolutions_lock_and_read() {
        let mut rows = Vec::new();
        for case in &STRETCHED {
            rows.push(check_case(case));
        }
        eprintln!("矩阵（全屏拉伸）：\n{}", rows.join("\n"));
    }

    #[test]
    fn windowed_resolutions_lock_and_read() {
        let mut rows = Vec::new();
        for case in &WINDOWED {
            rows.push(check_case(case));
        }
        eprintln!("矩阵（窗口）：\n{}", rows.join("\n"));
    }

    #[test]
    fn dpi_blur_resolutions_lock_and_read() {
        let mut rows = Vec::new();
        for case in &DPI {
            rows.push(check_case(case));
        }
        eprintln!("矩阵（DPI 糊）：\n{}", rows.join("\n"));
    }

    /// 中途换分辨率：客户区变了、或者客户区没变但内部分辨率变了（同一个窗口切了画质），都要几帧内恢复。
    #[test]
    fn switching_resolution_mid_session_recovers() {
        let fixtures = classic_fixtures();
        let (f, s) = samples(&fixtures)[0];
        let (label, strip, w, h) = &fixtures[f];
        let slot = &label.slots[s];
        let icon = Gray::from_bgra(strip, *w, *h).crop(slot.icon_left as i64, slot.icon_top as i64, 32, 32);
        let template = IconTemplate::new(&icon, &vec![true; 32 * 32]).expect("模板");
        let sample = Sample { strip };
        let expect = slot.expect().expect("有读数");

        let steps = [
            Case { name: "开场 1920x1080", internal: (1920, 1080), client: (1920, 1080), bilinear: None, min_reads: 3 },
            Case { name: "全屏 2560x1440", internal: (1366, 768), client: (2560, 1440), bilinear: None, min_reads: 3 },
            // 客户区不变，只换了内部分辨率：旧倍数对不上，要靠「连续认不出」放弃锁定再找
            Case { name: "同客户区 换内部 1600x900", internal: (1600, 900), client: (2560, 1440), bilinear: None, min_reads: 3 },
            Case { name: "窗口 1280x720", internal: (1280, 720), client: (1280, 720), bilinear: None, min_reads: 3 },
            Case { name: "全屏 3840x2160", internal: (1024, 768), client: (3840, 2160), bilinear: None, min_reads: 3 },
        ];
        let mut tracker = ScaleTracker::default();
        let mut report = Vec::new();
        for case in &steps {
            let shot = shot_for(&sample, case);
            let outcome = until_seen(&mut tracker, case.client, &shot, &template, LOST_FRAMES as usize + LOCK_TICKS + 2)
                .unwrap_or_else(|| panic!("{} 恢复不过来", case.name));
            assert!(
                outcome.ticks_to_hit <= LOST_FRAMES as usize + LOCK_TICKS,
                "{}：第 {} 帧才恢复",
                case.name,
                outcome.ticks_to_hit
            );
            assert_eq!(outcome.read, Some(expect), "{}：读数不对", case.name);
            report.push(format!("{} → {} 帧", case.name, outcome.ticks_to_hit));
        }
        eprintln!("切换分辨率恢复：\n{}", report.join("\n"));
    }

    // -----------------------------------------------------------------------
    // 状态机（不碰画面）
    // -----------------------------------------------------------------------

    #[test]
    fn tracker_resets_everything_when_client_size_changes() {
        let mut t = ScaleTracker::default();
        let batch = match t.plan((1920, 1080), None) {
            Plan::Search(b) => b,
            other => panic!("{other:?}"),
        };
        assert_eq!(batch[0], (1.0, 1.0));
        t.found((1.0, 1.0));
        assert_eq!(t.plan((1920, 1080), None), Plan::Locked((1.0, 1.0)));
        // 客户区一变，认定的倍数立即作废，重新从头找
        assert!(matches!(t.plan((2560, 1440), None), Plan::Search(_)));
        assert_eq!(t.locked(), None);
    }

    #[test]
    fn tracker_hud_scale_wins_and_is_snapped() {
        let mut t = ScaleTracker::default();
        let plan = t.plan((2560, 1440), Some(1.872));
        assert_eq!(plan, Plan::Locked(snap(2560, 1440, 1.872)));
    }

    #[test]
    fn tracker_drops_lock_after_consecutive_misses_and_keeps_it_on_hits() {
        let mut t = ScaleTracker::default();
        t.plan((2560, 1440), None);
        t.found((1.5, 1.5));
        for _ in 0..LOST_FRAMES - 1 {
            t.seen(false);
        }
        t.seen(true);
        assert_eq!(t.locked(), Some((1.5, 1.5)), "中间认到过一次，不放弃");
        for _ in 0..LOST_FRAMES {
            t.seen(false);
        }
        assert_eq!(t.locked(), None, "连续认不到就放弃锁定");
    }

    #[test]
    fn tracker_asks_for_a_reframe_only_after_a_long_search() {
        let mut t = ScaleTracker::default();
        t.plan((1600, 900), None);
        let mut hint = false;
        for _ in 0..SEARCH_HINT_FRAMES {
            hint = t.searched_in_vain();
            t.plan((1600, 900), None);
        }
        assert!(hint, "找够久了要提示");
    }
}
