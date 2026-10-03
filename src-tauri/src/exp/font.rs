//! 内置字形表 + 切字 + 定位。
//!
//! ## 为什么全部内置，不做运行时学习
//!
//! 经验那一行是**点阵字体**（字高 7 像素），字符集是**封闭**的：
//!
//! ```text
//! EXP 12145(60.08%)
//! ```
//!
//! 只可能出现 `0-9` 和 `(` `)` `.` `%` 这 14 个符号。既然只有 14 个，
//! 就没有任何理由让程序在运行时去「学」——每个字形的点阵都是量出来的常量，
//! 直接写在这里，开箱即用、每次启动都一样。
//!
//! 早先的版本只内置了其中一部分，缺的靠自动学习补。那条路被证明是错的：
//! 学习需要「同一个字形出现在能定死的位子上」才有唯一解，而升级那一瞬的
//! `0.00%` 又是多解的 —— 结果就是**时好时坏**，用户看到的是「经常识别不到」。
//! 内置全表之后这一类问题从根上不存在。
//!
//! ## 这些点阵是哪里来的
//!
//! * `0 2 4 6 7 9` —— 从真机抓屏里抠出来的，并被经验表校验过
//!   （`427096 / 46.09%` 在 120 级表里唯一成立的等级是 55）。
//! * `3 5 8` —— 真机上按经验表恒等式核实过的（三帧一致、位置各不相同）。
//! * `1` —— 从真机帧里直接取出来的：那一帧的经验字段是 `12145(60.08%)`，
//!   而 `12145 ÷ 20216 = 60.08%` 精确成立（20216 是 20 级的升级所需），
//!   于是「认不出的那个 2 像素宽字形必然是 1」这件事是被算术锁死的。
//! * `(` `)` `.` `%` —— 同样来自真机帧。
//!
//! `builtin_font_is_complete` 这条单测钉住「14 个符号一个不少」。
//!
//! ## 为什么字段定位**不能**看字形之间的空隙
//!
//! 这是踩过的坑里最隐蔽的一个。数字 `1` 的字形只有 **2 像素宽**，而它占的
//! 字格是 5 像素 —— 于是它右边到下一个字形之间会空出 **3 像素**。
//! 旧实现用「空隙 ≤ 2 像素算贴在一起」来把经验字段从整条状态栏里切出来，
//! 这个 3 像素的空隙就把字段从中间切断了：定位失败，整帧作废。
//!
//! 真机实测的后果：20 级（升级所需 20216）的经验值里含 `1` 的比例很高，
//! 于是 **50 帧里只有 8 帧能读出来**，而且读出来的那几帧还把开头的 `1`
//! 悄悄丢了（`12302` 读成 `2302` —— 一个看起来挺像真的的错数字）。
//!
//! 现在改成**纯结构解析**：从最右边的 `)` 往左，严格按
//! `数字+(整数.两位小数%)` 的格式吃字形，空隙多大都无所谓。
//! 任何一个位置对不上就整帧作废 —— 宁可读不到，绝不给出半个数字。

/// 字形单元格高度。游戏这一行字固定 7 像素（真机 1920×1080 实测）。
pub const CELL_HEIGHT: usize = 7;

/// 亮度阈值：这一行是亮字深底（白字 235+、底色 40~120），取中间偏上。
pub const INK_THRESHOLD: u8 = 170;

/// 单个字形最多几个像素宽。再宽的一定不是字：状态栏里还挤着 HP/MP 的数值、
/// 右侧按钮上的文字，实测有 18~29 像素宽的段。那些当分隔符丢掉。
const MAX_GLYPH_WIDTH: usize = 12;

/// 匹配容差（汉明距离）。抓屏是逐像素搬运，正常情况距离就是 0；
/// 留 1 个像素给「窗口被系统缩放」这种边缘情况 —— 但一旦有两个模板同样接近，
/// 就宁可判「认不出」（见 [`Font::match_glyph`]）。
pub const MATCH_TOLERANCE: usize = 1;

/// 经验数字最多几位（120 级需要 2971 万，8 位；留到 9 位）。
const MAX_NUMBER_DIGITS: usize = 9;

/// 经验数字和它左边那个字形之间，至少要空几列才算「两回事」。
///
/// 这个数不是拍的，是真机量出来的：
///
/// * 数字**内部**字形之间的空白最多 **3** 列 —— 数字 `1` 的字形只有 2 像素宽，
///   而它占的字格是 5 像素，于是右边会多空出两列；
/// * `EXP` 标签的 `P` 到数字之间空 **9** 列。
///
/// 取 5：比数字内部的 3 宽、比标签的 9 窄。它只在「左边那个字形认不出」时用来
/// 判断「它到底是不是这个数字的一部分」——认得出的时候直接看字符，不用看空白。
const MIN_NUMBER_GAP: usize = 5;

// ---------------------------------------------------------------------------
// 点阵
// ---------------------------------------------------------------------------

/// 一个字形位图：等宽的行，`true` = 有墨。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Bitmap {
    rows: Vec<Vec<bool>>,
}

impl Bitmap {
    pub fn from_ascii(rows: &[&str]) -> Self {
        Self {
            rows: rows
                .iter()
                .map(|row| row.chars().map(|c| c == '#').collect())
                .collect(),
        }
    }

    pub fn from_lines(rows: Vec<String>) -> Self {
        Self {
            rows: rows
                .iter()
                .map(|row| row.chars().map(|c| c == '#').collect())
                .collect(),
        }
    }

    /// 序列化成一行可读文本（换行用 `|`），日志和诊断里用。
    pub fn to_ascii(&self) -> String {
        self.rows
            .iter()
            .map(|row| {
                row.iter()
                    .map(|ink| if *ink { '#' } else { '.' })
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("|")
    }
    pub fn width(&self) -> usize {
        self.rows.first().map(|row| row.len()).unwrap_or(0)
    }

    pub fn height(&self) -> usize {
        self.rows.len()
    }

    /// 两个**同尺寸**位图之间不同的像素个数。尺寸不同返回 `None`（不可比）。
    pub fn distance(&self, other: &Bitmap) -> Option<usize> {
        if self.width() != other.width() || self.height() != other.height() {
            return None;
        }
        Some(
            self.rows
                .iter()
                .zip(other.rows.iter())
                .map(|(a, b)| a.iter().zip(b.iter()).filter(|(x, y)| x != y).count())
                .sum(),
        )
    }

    /// 重采样到 `target_w × target_h`（面积平均 + 多数表决），返回二值位图。
    ///
    /// 逐输出格收集源像素（放大时散着放会留下空格，见 `normalize_band` 的同款说明）。
    pub fn resized(&self, target_w: usize, target_h: usize) -> Option<Bitmap> {
        if self.width() == 0 || self.height() == 0 || target_w == 0 || target_h == 0 {
            return None;
        }
        let (w, h) = (self.width(), self.height());
        let mut rows = Vec::with_capacity(target_h);
        for oy in 0..target_h {
            let sy0 = oy * h / target_h;
            let sy1 = ((oy + 1) * h / target_h).max(sy0 + 1).min(h);
            let mut row = Vec::with_capacity(target_w);
            for ox in 0..target_w {
                let sx0 = ox * w / target_w;
                let sx1 = ((ox + 1) * w / target_w).max(sx0 + 1).min(w);
                let mut ink = 0usize;
                let mut count = 0usize;
                for source_row in &self.rows[sy0..sy1] {
                    for value in &source_row[sx0..sx1] {
                        if *value {
                            ink += 1;
                        }
                        count += 1;
                    }
                }
                row.push(count > 0 && ink * 2 >= count);
            }
            rows.push(row);
        }
        Some(Bitmap { rows })
    }
}

// ---------------------------------------------------------------------------
// 内置字形表
// ---------------------------------------------------------------------------

/// 经验那一行会用到的**全部** 14 个符号。
///
/// 顺序按字符排，方便人对着看。每一行的来历见模块文档。
pub const BUILTIN: &[(char, &[&str])] = &[
    (
        '0',
        &[".###.", "#...#", "#...#", "#...#", "#...#", "#...#", ".###."],
    ),
    // 数字 1 只有 2 像素宽：这是真机的样子，不是画错了。
    // 也正因为窄，它和邻居之间的空隙会变成 3 像素 —— 见模块文档里那个坑。
    ('1', &[".#", "##", ".#", ".#", ".#", ".#", ".#"]),
    (
        '2',
        &[".###.", "#...#", "....#", "...#.", "..#..", ".#...", "#####"],
    ),
    (
        '3',
        &[".###.", "#...#", "....#", "..##.", "....#", "#...#", ".###."],
    ),
    (
        '4',
        &["...#.", "..##.", ".#.#.", "#..#.", "#####", "...#.", "...#."],
    ),
    (
        '5',
        &["#####", "#....", "#....", ".###.", "....#", "#...#", ".###."],
    ),
    (
        '6',
        &[".###.", "#...#", "#....", "####.", "#...#", "#...#", ".###."],
    ),
    (
        '7',
        &["#####", "....#", "....#", "....#", "...#.", "..#..", "..#.."],
    ),
    (
        '8',
        &[".###.", "#...#", "#...#", ".###.", "#...#", "#...#", ".###."],
    ),
    (
        '9',
        &[".###.", "#...#", "#...#", ".####", "....#", "#...#", ".###."],
    ),
    ('(', &["##", "#.", "#.", "#.", "#.", "#.", "#."]),
    (')', &["##", ".#", ".#", ".#", ".#", ".#", ".#"]),
    ('.', &["..", "..", "..", "..", "#.", "##", ".."]),
    (
        '%',
        &[
            ".#....#", "#.#..#.", ".#..#..", "...#...", "..#..#.", ".#..#.#", "#....#.",
        ],
    ),
    // `EXP` 这三个标签字**必须**在表里，不是为了显示，而是为了给数字定左边界：
    // 数字左边紧挨着的那个字形如果**认得出**（就是 `P`），数字就到此为止；
    // 如果**认不出**，就得靠空白宽度判断它是不是数字的一部分 —— 判断不了就整帧作废。
    // 少了它们，一个认不出的字形会把数字悄悄截断（`?2145` 读成 `2145`）。
    ('E', &["######", "#####.", "##....", "##....", "#####.", "#####.", "##...."]),
    ('X', &["#....#", "##..##", ".####.", ".####.", "..##..", ".####.", ".####."]),
    ('P', &["#####.", "######", "##..##", "##..##", "######", "#####.", "##...."]),
];

/// 字形表。
#[derive(Clone)]
pub struct Font {
    entries: Vec<(char, Bitmap)>,
}

impl Font {
    /// 内置全表 —— 这是唯一的来源，没有「学到一半」的状态。
    pub fn builtin() -> Self {
        Self {
            entries: BUILTIN
                .iter()
                .map(|(ch, rows)| (*ch, Bitmap::from_ascii(rows)))
                .collect(),
        }
    }

    pub fn bitmap_of(&self, ch: char) -> Option<&Bitmap> {
        self.entries
            .iter()
            .find(|(known, _)| *known == ch)
            .map(|(_, bitmap)| bitmap)
    }

    /// 认识的符号（按字符排序），诊断页显示用。
    pub fn known(&self) -> String {
        let mut chars: Vec<char> = self.entries.iter().map(|(ch, _)| *ch).collect();
        chars.sort_unstable();
        chars.into_iter().collect()
    }

    /// 认一个字形。
    ///
    /// 规则：尺寸必须一致；距离最小的那个要被**唯一**选中（距离并列就算认不出），
    /// 且距离不超过 [`MATCH_TOLERANCE`]。字是点阵、抓屏是逐像素搬运，正常情况距离是 0。
    ///
    /// 「并列就认不出」这条很重要：`1` 和 `(` 都是 2 像素宽，如果哪天字体改成了
    /// 两者同形，这里会如实报「认不出」，而不是猜一个 —— 猜错一位数字比读不到严重得多。
    pub fn match_glyph(&self, bitmap: &Bitmap) -> Option<char> {
        self.match_glyph_within(bitmap, MATCH_TOLERANCE)
    }

    /// 同 [`match_glyph`]，但容差可调。
    ///
    /// 给按比例缩放过（非 1080p）的字形用：缩放会带一两个像素的边缘模糊，
    /// 逐像素的 0 容差会全军覆没。容差本身仍然很小，且「并列即认不出」照旧。
    pub fn match_glyph_within(&self, bitmap: &Bitmap, tolerance: usize) -> Option<char> {
        let mut best: Option<(char, usize)> = None;
        let mut tie = false;
        for (ch, template) in &self.entries {
            let Some(distance) = bitmap.distance(template) else {
                continue;
            };
            match best {
                None => best = Some((*ch, distance)),
                Some((_, current)) if distance < current => {
                    best = Some((*ch, distance));
                    tie = false;
                }
                Some((_, current)) if distance == current => tie = true,
                _ => {}
            }
        }
        match best {
            Some((ch, distance)) if distance <= tolerance && !tie => Some(ch),
            _ => None,
        }
    }

    /// 认一个**尺寸对不上**的字形：先按模板尺寸重采样再比。
    ///
    /// 为什么需要它：界面按 1.33 倍这类非整数比例缩放时，同一个字形切出来的段
    /// 可能比模板宽 1~2 像素（`Bitmap::distance` 对尺寸不同直接返回 `None`，
    /// 于是整行读不出来）。把切出来的段缩到模板尺寸（多数表决）再比，
    /// 形状仍然是原来那一个，只是多了一道归一化。
    pub fn match_glyph_scaled(&self, bitmap: &Bitmap, tolerance: usize) -> Option<char> {
        let mut best: Option<(char, usize)> = None;
        let mut tie = false;
        for (ch, template) in &self.entries {
            let resized = if bitmap.width() == template.width() && bitmap.height() == template.height()
            {
                bitmap.clone()
            } else {
                match bitmap.resized(template.width(), template.height()) {
                    Some(resized) => resized,
                    None => continue,
                }
            };
            let Some(distance) = resized.distance(template) else {
                continue;
            };
            match best {
                None => best = Some((*ch, distance)),
                Some((_, current)) if distance < current => {
                    best = Some((*ch, distance));
                    tie = false;
                }
                Some((_, current)) if distance == current => tie = true,
                _ => {}
            }
        }
        match best {
            Some((ch, distance)) if distance <= tolerance && !tie => Some(ch),
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------
// 像素
// ---------------------------------------------------------------------------

/// 一块 BGRA 像素（抓屏模块的原样输出）。
pub struct Pixels<'a> {
    pub width: usize,
    pub height: usize,
    pub bgra: &'a [u8],
}

impl Pixels<'_> {
    /// 亮度（人眼权重）。抓屏给的是 BGRA，所以通道顺序是 B、G、R、A。
    pub fn luma(&self, x: usize, y: usize) -> u8 {
        let offset = (y * self.width + x) * 4;
        let (b, g, r) = (
            self.bgra[offset] as u32,
            self.bgra[offset + 1] as u32,
            self.bgra[offset + 2] as u32,
        );
        ((r * 299 + g * 587 + b * 114) / 1000) as u8
    }

    /// 这一块是不是「什么都没画」（全黑或全白）。
    ///
    /// 用来把「PrintWindow 悄悄返回了一张空图」和「游戏画面上真的没有那一行」
    /// 分开 —— 这两种情况的处置完全不同，报同一句话会把人引向错的方向。
    pub fn looks_blank(&self) -> bool {
        if self.width == 0 || self.height == 0 {
            return true;
        }
        let mut min = 255u8;
        let mut max = 0u8;
        // 抽样就够：一帧 1920×44 全扫一遍没必要
        for y in (0..self.height).step_by(3) {
            for x in (0..self.width).step_by(7) {
                let luma = self.luma(x, y);
                min = min.min(luma);
                max = max.max(luma);
            }
        }
        max.saturating_sub(min) < 8
    }

    /// 截取子区域并返回独立连续的 BGRA 内存块 `(actual_w, actual_h, buffer)`。
    pub fn crop(&self, x: usize, y: usize, w: usize, h: usize) -> (usize, usize, Vec<u8>) {
        if self.width == 0 || self.height == 0 || w == 0 || h == 0 || x >= self.width || y >= self.height {
            return (0, 0, Vec::new());
        }
        let x1 = (x + w).min(self.width);
        let y1 = (y + h).min(self.height);
        let actual_w = x1 - x;
        let actual_h = y1 - y;
        let mut buffer = Vec::with_capacity(actual_w * actual_h * 4);
        for row in y..y1 {
            let start = (row * self.width + x) * 4;
            let end = (row * self.width + x1) * 4;
            buffer.extend_from_slice(&self.bgra[start..end]);
        }
        (actual_w, actual_h, buffer)
    }
}

/// 检测到的疑似文字横带区域。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TextLineCandidate {
    /// 裁剪矩形在底栏中的像素坐标 (x, y, width, height)
    pub rect: (usize, usize, usize, usize),
    /// 检测到的实际文字笔画高度（像素），直接反映游戏界面的文字字号与缩放比例
    pub text_height: usize,
}

/// 一次最多交出几条候选横带。识别层会逐条验证（经验表自洽才采纳），
/// 所以宁可多给几条，也不要在「第一条猜错了」时整帧作废。
/// 16 条是给「整幅画面里全是地形/聊天纹理」留的余量：多给候选只多几次
/// 廉价的点阵缩放，OCR 那边另有条数上限（见 `reader::recognize`）。
pub const MAX_CANDIDATES: usize = 16;

/// 在画面里粗定位所有「像一行密集文字」的横带（按分数从高到低）。
///
/// # 为什么粗定位行得通
///
/// 游戏界面随分辨率缩放时，字高会变（1440p 约 9~10px、4K 约 14px、768p 约 5px），
/// 内置的 7px 点阵字形无法直接比对。但经验数字行在画面里依然呈现独特的像素特征：
/// 一行十几个密集字符，水平方向有频繁的笔画黑白跃迁（0->1 翻转）；
/// 而背景、装饰线要么没有跃迁，要么只有一两处转折。
///
/// 本函数只负责框出候选（矩形 + 实测笔画高度），认字交给
/// [`read_candidate`]（按字高缩回 7px 点阵）或 OCR，**最终一律由
/// `table::validate` 自洽算式把关**，绝不凭启发式猜数字。
pub fn find_candidate_boxes(pixels: &Pixels<'_>, limit: usize) -> Vec<TextLineCandidate> {
    if pixels.width < 40 || pixels.height < 4 {
        return Vec::new();
    }
    let limit = limit.max(1);

    // 1) 统计每一行的水平笔画跃迁次数与墨点数
    let mut row_transitions = vec![0usize; pixels.height];
    let mut row_inks = vec![0usize; pixels.height];
    for y in 0..pixels.height {
        let mut transitions = 0;
        let mut inks = 0;
        let mut prev_ink = false;
        for x in 0..pixels.width {
            let ink = pixels.luma(x, y) > INK_THRESHOLD;
            if ink {
                inks += 1;
                if !prev_ink {
                    transitions += 1;
                }
            }
            prev_ink = ink;
        }
        row_transitions[y] = transitions;
        row_inks[y] = inks;
    }

    // 2) 把「合格行」分组成连续区间。合格条件放得比最终判据宽：
    //    这里只负责别漏掉，误检由识别层挡。
    let is_text_row = |y: usize| row_transitions[y] >= 2 && row_inks[y] >= 4;

    let mut runs: Vec<(usize, usize)> = Vec::new();
    let mut current_start: Option<usize> = None;
    let mut gap_count = 0;

    for y in 0..pixels.height {
        if is_text_row(y) {
            gap_count = 0;
            if current_start.is_none() {
                current_start = Some(y);
            }
        } else if let Some(start) = current_start {
            gap_count += 1;
            // 允许行内最多 2 个像素的垂直微弱间隙（抗锯齿行常常弱一两行）
            if gap_count > 2 {
                runs.push((start, y - gap_count));
                current_start = None;
                gap_count = 0;
            }
        }
    }
    if let Some(start) = current_start {
        runs.push((start, pixels.height.saturating_sub(1)));
    }

    if runs.is_empty() {
        return Vec::new();
    }

    // 3) 区间太长时切成 ≤40 行的**滑窗**。
    //
    //    这一步不是可选的：真实画面里游戏世界的地形、亮底 UI 会让几十上百行
    //    连续「合格」，整段丢掉就等于「自动定位永远找不到那一行」（真机现场）。
    //    窗口步长 8：16 像素以内的文字至少完整落在某一个窗口里。
    const WINDOW_ROWS: usize = 40;
    const WINDOW_STEP: usize = 8;
    let mut vertical_bands: Vec<(usize, usize)> = Vec::new();
    for (start, end) in runs {
        if end - start + 1 <= WINDOW_ROWS {
            vertical_bands.push((start, end));
            continue;
        }
        let mut y = start;
        loop {
            let window_end = (y + WINDOW_ROWS - 1).min(end);
            vertical_bands.push((y, window_end));
            if window_end >= end {
                break;
            }
            y += WINDOW_STEP;
        }
    }

    // 4) 每条垂直带里按列投影切水平簇；一个簇 = 一行连续的文字。
    //    簇的**实际笔画高度**（tight_h）在这里量出来：窗口带可能有 40 行高，
    //    但文字只有 7~16 行，缩放和评分都要用真实值。
    let mut candidates: Vec<(usize, TextLineCandidate)> = Vec::new();

    for (y0, y1) in vertical_bands {
        let band_h = y1 - y0 + 1;
        // 允许的字符间空白列上限：一个字宽的量级。窗口带用 16 行封顶来估，
        // 否则 40 行的窗口会把行间空白也当成「字符间隙」。
        let gap_h = band_h.min(16);
        let max_char_gap = (gap_h * 3 / 2).clamp(6, 24);

        // (x0, x1, ink_cols, tight_h)
        let mut clusters: Vec<(usize, usize, usize, usize)> = Vec::new();
        let mut cur_cluster: Option<(usize, usize, usize)> = None;
        let mut gap = 0;

        let flush = |c: (usize, usize, usize), clusters: &mut Vec<(usize, usize, usize, usize)>| {
            let tight_h = (y0..=y1)
                .filter(|&y| (c.0..=c.1).any(|x| pixels.luma(x, y) > INK_THRESHOLD))
                .count();
            if is_text_cluster(c.1 - c.0 + 1, tight_h) {
                clusters.push((c.0, c.1, c.2, tight_h));
            }
        };

        for x in 0..pixels.width {
            let col_has_ink = (y0..=y1).any(|y| pixels.luma(x, y) > INK_THRESHOLD);
            if col_has_ink {
                gap = 0;
                match &mut cur_cluster {
                    None => cur_cluster = Some((x, x, 1)),
                    Some(c) => {
                        c.1 = x;
                        c.2 += 1;
                    }
                }
            } else if let Some(c) = cur_cluster {
                gap += 1;
                if gap > max_char_gap {
                    flush(c, &mut clusters);
                    cur_cluster = None;
                    gap = 0;
                }
            }
        }
        if let Some(c) = cur_cluster {
            flush(c, &mut clusters);
        }

        for (x0, x1, ink_cols, tight_h) in clusters {
            // 评分依据：簇内笔画跃迁总和与墨点分布
            let mut trans_sum: usize = 0;
            let mut ink_pixels: usize = 0;
            for y in y0..=y1 {
                let mut t = 0;
                let mut prev = false;
                for x in x0..=x1 {
                    let ink = pixels.luma(x, y) > INK_THRESHOLD;
                    if ink {
                        ink_pixels += 1;
                        if !prev {
                            t += 1;
                        }
                    }
                    prev = ink;
                }
                trans_sum += t;
            }

            // 墨点占比离「一行字」太远（>50% 是亮底 UI/地形噪声，<2% 是装饰线）
            // 直接不要：真机经验行的墨点占比在 20% 上下（README 的实测表）。
            // 这同时省掉了在亮底画面（登录 / 选人）上白跑 OCR 的开销。
            let area = ((x1 - x0 + 1) * tight_h.max(1)) as f64;
            let ink_ratio = ink_pixels as f64 / area.max(1.0);
            if !(0.02..=0.5).contains(&ink_ratio) {
                continue;
            }
            // 位置先验：经验那一行在**底部 HUD** 上（网页版枫记的搜索范围也锚在底部）。
            // 这只影响候选排序，不改变任何判据 —— 真伪仍由经验表决定。
            let center_y = (y0 + y1) as f64 / 2.0;
            let height = pixels.height as f64;
            let position_bonus = if center_y > height * 0.85 {
                2.0
            } else if center_y > height * 0.67 {
                1.3
            } else {
                1.0
            };
            let score = ((trans_sum * 10 + ink_cols) as f64 * position_bonus) as usize;

            // 给缩放/OCR 留一点四周边缘余量，防止双线性缩放时笔画被硬切断
            let pad_x = (tight_h / 3).clamp(2, 8);
            let pad_y = (tight_h / 4).clamp(1, 4);
            let crop_x0 = x0.saturating_sub(pad_x);
            let crop_y0 = y0.saturating_sub(pad_y);
            let crop_x1 = (x1 + pad_x + 1).min(pixels.width);
            let crop_y1 = (y1 + pad_y + 1).min(pixels.height);

            let candidate = TextLineCandidate {
                rect: (crop_x0, crop_y0, crop_x1 - crop_x0, crop_y1 - crop_y0),
                text_height: tight_h.max(1),
            };
            if let Some(existing) = candidates
                .iter_mut()
                .find(|(_, kept)| candidate_overlaps(kept, &candidate))
            {
                if score > existing.0 {
                    *existing = (score, candidate);
                }
            } else {
                candidates.push((score, candidate));
            }
        }
    }

    candidates.sort_by(|a, b| b.0.cmp(&a.0));
    candidates
        .into_iter()
        .take(limit)
        .map(|(_, candidate)| candidate)
        .collect()
}

/// 横带「像一行字」的最低门槛：宽度至少是字高的 4 倍（经验行有十几个字符），
/// 高度不超过 32（真实文字在 40 行窗口里必然上下留白；整窗都是墨的那是噪声/地形）。
fn is_text_cluster(width: usize, height: usize) -> bool {
    (4..=32).contains(&height) && width >= height * 4
}

/// 两个候选带是不是指向同一行（中心点和宽高都接近）。
fn candidate_overlaps(a: &TextLineCandidate, b: &TextLineCandidate) -> bool {
    let (ax, ay, aw, ah) = a.rect;
    let (bx, by, bw, bh) = b.rect;
    let (acx, acy) = (ax as f64 + aw as f64 / 2.0, ay as f64 + ah as f64 / 2.0);
    let (bcx, bcy) = (bx as f64 + bw as f64 / 2.0, by as f64 + bh as f64 / 2.0);
    (acx - bcx).abs() <= (aw.max(bw) as f64) * 0.25
        && (acy - bcy).abs() <= (ah.max(bh) as f64) * 0.5
}

/// 分数最高的那一条（诊断页 / 单测用）。
#[cfg(test)]
pub fn find_candidate_box(pixels: &Pixels<'_>) -> Option<TextLineCandidate> {
    find_candidate_boxes(pixels, 1).into_iter().next()
}

// ---------------------------------------------------------------------------
// 切字
// ---------------------------------------------------------------------------

/// 切出来的一个字形：位图 + 它在帧里的横坐标范围。
#[derive(Debug, Clone)]
pub struct Glyph {
    pub x0: usize,
    pub x1: usize,
    pub bitmap: Bitmap,
}

/// 在一个 7 像素高的横带里按「有墨 / 没墨」切成字形。
///
/// 只做切分，不判断「这行是不是经验那一行」。过宽的段（HP/MP 数值块、按钮文字）
/// 当分隔符丢掉，而不是否掉整行 —— 旧版因为「这条带里出现了宽段」就把整行否掉，
/// 结果在真机上一直读不到。
fn segment(pixels: &Pixels<'_>, band_y: usize) -> Vec<Glyph> {
    let mut runs: Vec<(usize, usize)> = Vec::new();
    let mut current: Option<(usize, usize)> = None;

    for x in 0..pixels.width {
        let inked = (band_y..band_y + CELL_HEIGHT).any(|y| pixels.luma(x, y) > INK_THRESHOLD);
        match (&mut current, inked) {
            (None, true) => current = Some((x, x)),
            (Some(run), true) => run.1 = x,
            (Some(run), false) => {
                runs.push(*run);
                current = None;
            }
            (None, false) => {}
        }
    }
    if let Some(run) = current {
        runs.push(run);
    }

    runs.into_iter()
        .filter(|(a, b)| b - a + 1 <= MAX_GLYPH_WIDTH)
        .map(|(a, b)| Glyph {
            x0: a,
            x1: b,
            bitmap: Bitmap::from_lines(
                (band_y..band_y + CELL_HEIGHT)
                    .map(|y| {
                        (a..=b)
                            .map(|x| if pixels.luma(x, y) > INK_THRESHOLD { '#' } else { '.' })
                            .collect::<String>()
                    })
                    .collect(),
            ),
        })
        .collect()
}

// ---------------------------------------------------------------------------
// 字段定位（纯结构）
// ---------------------------------------------------------------------------

/// 一次成功的定位：经验、百分比，以及字段在字形序列里的下标范围。
#[derive(Debug, Clone, PartialEq)]
pub struct Field {
    pub exp: u64,
    pub percent: f64,
    /// 参与这个字段的字形下标（闭区间）—— 用来算字段的像素范围
    pub start: usize,
    pub end: usize,
}

fn is_digit(slot: &Option<char>) -> bool {
    matches!(slot, Some(ch) if ch.is_ascii_digit())
}

/// 从 `close`（`)` 所在下标）往左，严格按 `数字+(整数.两位小数%)` 取一段字段。
///
/// **不看空隙**：只要每一个位子上的字形都对得上格式，中间空多远都行。
/// 任何一位对不上（认不出、或者根本不是那个符号）就返回 `None`。
///
/// 唯一必须看空隙的地方是**数字的左边界**：左边那个字形认不出时，它可能是
/// 一个我们没认出来的数字 —— 那就会把数字悄悄截断。这时只有它离得足够远
/// （空白 ≥ [`MIN_NUMBER_GAP`] 列）才能断定它不属于这个数字，否则整帧作废。
fn parse_at(glyphs: &[Glyph], slots: &[Option<char>], close: usize) -> Option<Field> {
    if slots.get(close) != Some(&Some(')')) {
        return None;
    }
    if slots.get(close.checked_sub(1)?) != Some(&Some('%')) {
        return None;
    }
    let frac_hi = close.checked_sub(2)?;
    let frac_lo = close.checked_sub(3)?;
    if !is_digit(slots.get(frac_lo)?) || !is_digit(slots.get(frac_hi)?) {
        return None;
    }
    let dot = close.checked_sub(4)?;
    if slots.get(dot) != Some(&Some('.')) {
        return None;
    }

    // 整数部分 1~3 位（`100.00%` 是升级那一瞬会出现的）
    let mut cursor = dot;
    let mut int_text = String::new();
    while int_text.len() < 3 {
        match cursor.checked_sub(1) {
            Some(index) if is_digit(&slots[index]) => {
                int_text.insert(0, slots[index].unwrap_or('0'));
                cursor = index;
            }
            _ => break,
        }
    }
    if int_text.is_empty() {
        return None;
    }

    let open = cursor.checked_sub(1)?;
    if slots.get(open) != Some(&Some('(')) {
        return None;
    }

    // 经验数字：往左一位一位吃，直到碰到**认得出的**非数字（正常是 `P`）为止。
    let mut start = open;
    let mut digits: Vec<char> = Vec::new();
    while let Some(index) = start.checked_sub(1) {
        match slots.get(index) {
            Some(Some(ch)) if ch.is_ascii_digit() => {
                digits.insert(0, *ch);
                start = index;
            }
            // 认得出的非数字 = 数字到此为止，可以放行
            Some(Some(_)) => break,
            // 认不出的字形：它**可能**是被漏掉的数字。只有离得够远才敢放行。
            _ => {
                let gap = glyphs[start]
                    .x0
                    .saturating_sub(glyphs.get(index).map(|glyph| glyph.x1 + 1).unwrap_or(0));
                if gap < MIN_NUMBER_GAP {
                    return None;
                }
                break;
            }
        }
    }
    if digits.is_empty() || digits.len() > MAX_NUMBER_DIGITS {
        return None;
    }

    let exp: u64 = digits.iter().collect::<String>().parse().ok()?;
    let percent: f64 = format!(
        "{}.{}{}",
        int_text,
        slots[frac_lo].unwrap_or('0'),
        slots[frac_hi].unwrap_or('0')
    )
    .parse()
    .ok()?;
    if !(0.0..=100.0).contains(&percent) {
        return None;
    }

    Some(Field {
        exp,
        percent,
        start,
        end: close,
    })
}

/// 在一整行字形里找经验字段。
///
/// 锚点是右边那一对 `%` `)` —— 在整条状态栏里它们是独一无二的结构。
/// 从右往左找第一个 `)`，再从它往左按格式解析。
pub(crate) fn locate(glyphs: &[Glyph], slots: &[Option<char>]) -> Option<Field> {
    for close in (0..slots.len()).rev() {
        if let Some(field) = parse_at(glyphs, slots, close) {
            return Some(field);
        }
    }
    None
}

// ---------------------------------------------------------------------------
// 读一整行
// ---------------------------------------------------------------------------

/// 一次完整的读数。
#[derive(Debug, Clone, PartialEq)]
pub struct Reading {
    pub exp: u64,
    pub percent: f64,
    /// 原始字形串（诊断用），认不出的位子写 `?`
    pub raw: String,
    /// 字段在帧里的像素范围（诊断/校准页用）
    pub rect: (usize, usize, usize, usize),
    /// 有几个字形是逐像素命中（选哪条横带的依据之一）
    pub exact: usize,
    pub glyphs: usize,
    /// 这一行实际测到的墨迹高度（像素）。1080p 下的点阵字是 7；
    /// 其他分辨率 / 界面缩放下它是 5~36 —— 识别时就按它把候选带缩回 7 像素。
    pub text_height: usize,
}

/// 这一帧里能读到的最好的那一行。
///
/// 扫法：自上而下逐个尝试 7 像素高的横带，每带切字、定位、解析，
/// 最后挑**最可信**的一条，而不是撞上第一条就返回。
///
/// 排序依据：
/// 1. 经验表能唯一确定等级（说明三个数互相自洽）最好；
/// 2. 相同档位时，逐像素命中的字形越多越好 —— 抓屏是逐像素搬运，
///    切对了的那条带上距离都是 0，切歪一条就会多出好几个像素的差。
pub fn read_line(pixels: &Pixels<'_>, font: &Font) -> Option<Reading> {
    read_line_with(pixels, font, MATCH_TOLERANCE)
}

/// 同 [`read_line`]，但可以指定匹配容差。
///
/// 按比例缩放过（非 1080p）的字形边缘会带一两个像素的模糊，
/// 需要比「逐像素搬运」宽一点的容差；容差仍然很小，且「并列即认不出」。
pub fn read_line_with(pixels: &Pixels<'_>, font: &Font, tolerance: usize) -> Option<Reading> {
    read_line_impl(pixels, font, tolerance, false)
}

/// 给**按比例缩放过**的画面用：字形段尺寸可能和模板差 1~2 像素，
/// 先按模板尺寸重采样再比（见 [`Font::match_glyph_scaled`]）。
fn read_line_scaled(pixels: &Pixels<'_>, font: &Font, tolerance: usize) -> Option<Reading> {
    read_line_impl(pixels, font, tolerance, true)
}

fn read_line_impl(
    pixels: &Pixels<'_>,
    font: &Font,
    tolerance: usize,
    size_tolerant: bool,
) -> Option<Reading> {
    if pixels.height < CELL_HEIGHT || pixels.width < 40 {
        return None;
    }
    let mut best: Option<((u8, std::cmp::Reverse<usize>), Reading)> = None;

    for band_y in 0..=(pixels.height - CELL_HEIGHT) {
        let glyphs = segment(pixels, band_y);
        if glyphs.len() < 7 {
            continue;
        }
        let slots: Vec<Option<char>> = glyphs
            .iter()
            .map(|glyph| {
                if size_tolerant {
                    font.match_glyph_scaled(&glyph.bitmap, tolerance)
                } else {
                    font.match_glyph_within(&glyph.bitmap, tolerance)
                }
            })
            .collect();
        let Some(field) = locate(&glyphs, &slots) else {
            continue;
        };
        let exact = glyphs
            .iter()
            .filter(|glyph| {
                font.bitmap_of(font.match_glyph(&glyph.bitmap).unwrap_or('\0'))
                    .and_then(|template| glyph.bitmap.distance(template))
                    == Some(0)
            })
            .count();
        // 评分分级：
        // 0: Known —— 经验表能唯一确定等级（三个数完全自洽，最可信）；
        // 1: Ambiguous —— 多个等级成立（例如刚好升级归零时的 0(0.00%)，虽无法直接定级但数值完全合法）；
        // 2: Contradiction —— 没有任何等级成立（纯错帧或杂乱噪点切出的数字）。
        //
        // 旧实现将 Ambiguous 与 Contradiction 同打为 rank 1，导致合法归零帧（字数少、exact 仅约 5）
        // 容易被上方噪点误切出的较长错串（exact 达到 6 但经验表矛盾）逆袭抢占，整帧被误判为坏帧。
        let rank = match crate::exp::table::validate(field.exp, field.percent) {
            crate::exp::table::Verdict::Known(_) => 0u8,
            crate::exp::table::Verdict::Ambiguous => 1u8,
            crate::exp::table::Verdict::Contradiction => 2u8,
        };
        let reading = Reading {
            exp: field.exp,
            percent: field.percent,
            raw: slots
                .iter()
                .map(|slot| slot.unwrap_or('?'))
                .collect::<String>(),
            rect: (
                glyphs[field.start].x0,
                band_y,
                glyphs[field.end].x1 + 1,
                band_y + CELL_HEIGHT,
            ),
            exact,
            glyphs: glyphs.len(),
            text_height: CELL_HEIGHT,
        };
        let score = (rank, std::cmp::Reverse(exact));
        if best.as_ref().map(|(current, _)| score < *current).unwrap_or(true) {
            best = Some((score, reading));
        }
        // 为什么不在此处 if rank == 0 { break; }：
        // 旧实现一碰到 rank == 0 就立即 break，导致垂直偏上 1 像素的横带只要勉强
        // 靠容差凑出合法数字就提前退出，正中那条 exact 满分（距离为 0）的横带连比对的机会都没有。
        // 底部条总高仅 44 像素，候选带极少，完整扫完所有候选带仅需几十微秒，能确保挑出垂直切得最准的那一行。
    }

    best.map(|(_, reading)| reading)
}

/// 画面不超过这个高度时，先跑一遍原生 7px 快路（便宜、1080p 下一步到位）。
///
/// 更高的画面（整幅客户区）不跑：`read_line` 在每个像素高度上都要切一遍字，
/// 1440p 全幅是两千万次亮度读取 —— 交给候选带管线，只扫有文字的带。
const NATIVE_SCAN_MAX_HEIGHT: usize = 300;

/// 缩放回 7px 之后的匹配容差：比逐像素搬运宽 1 个像素，容忍缩放边缘的模糊。
const SCALED_MATCH_TOLERANCE: usize = 2;

/// 在**任意缩放**的画面里读经验行（字形路径，不含 OCR）。
///
/// 流程：
/// 1. 矮画面（校准过的小裁剪 / 诊断条）先走原生 7px 快路；
/// 2. 用 [`find_candidate_boxes`] 框出所有像文字的横带；
/// 3. 每条候选在[`read_candidate`]里按实测高度缩回 7px 再走同一套点阵字形；
/// 4. 用经验表自洽程度 + 逐像素命中数挑最好的一条。
///
/// 认错的风险由经验表拦住（`table::validate`），所以候选多给几条没有坏处。
pub fn read_frame(pixels: &Pixels<'_>, font: &Font) -> Option<Reading> {
    let mut best: Option<Reading> = None;
    if pixels.height <= NATIVE_SCAN_MAX_HEIGHT && pixels.height >= CELL_HEIGHT {
        if let Some(reading) = read_line(pixels, font) {
            if reading_rank(&reading) == 0 {
                return Some(reading);
            }
            best = Some(reading);
        }
    }
    if pixels.height < 4 || pixels.width < 8 {
        return best;
    }
    for candidate in find_candidate_boxes(pixels, MAX_CANDIDATES) {
        if let Some(reading) = read_candidate(pixels, &candidate, font) {
            if better_reading(&reading, best.as_ref()) {
                best = Some(reading);
            }
        }
    }
    best
}

/// 把一条候选横带缩回 7px 点阵后读一次（这就是「任意分辨率」的关键一步）。
///
/// 候选带是粗定位的结果，上下可能并进相邻的另一行 / 装饰线 / 抗锯齿行。
/// 所以这里：
/// 1. 先按「连续墨行」把候选切成**子带**（两个子带之间允许 1 行空隙），
///    每段单独识别 —— 直接取「所有墨的包围盒」会把两行并成一行，
///    等比缩放后谁都认不出；
/// 2. 每段再试若干裁剪窗口（上下各多切掉 0~3 行），让缩放正好压在真实字形上；
/// 3. 最后挑自洽程度最高的读数。
pub fn read_candidate(
    pixels: &Pixels<'_>,
    candidate: &TextLineCandidate,
    font: &Font,
) -> Option<Reading> {
    let (cx, cy, cw, ch) = candidate.rect;
    let x1 = (cx + cw).min(pixels.width);
    let y1 = (cy + ch).min(pixels.height);
    if cw == 0 || ch == 0 || x1 <= cx || y1 <= cy {
        return None;
    }

    // 1) 连续墨行 → 子带
    let mut bands: Vec<(usize, usize)> = Vec::new();
    let mut start: Option<usize> = None;
    let mut gap = 0;
    for y in cy..y1 {
        let has_ink = (cx..x1).any(|x| pixels.luma(x, y) > INK_THRESHOLD);
        if has_ink {
            gap = 0;
            start.get_or_insert(y);
        } else if let Some(band_start) = start {
            gap += 1;
            if gap > 1 {
                bands.push((band_start, y - gap));
                start = None;
                gap = 0;
            }
        }
    }
    if let Some(band_start) = start {
        bands.push((band_start, y1 - 1));
    }

    let mut best: Option<Reading> = None;
    for (band_top, band_bottom) in bands {
        let band_h = band_bottom - band_top + 1;
        if !(4..=40).contains(&band_h) {
            continue;
        }
        // 2) 这一段的水平紧边界
        let mut left = None;
        let mut right = None;
        for x in cx..x1 {
            if (band_top..=band_bottom).any(|y| pixels.luma(x, y) > INK_THRESHOLD) {
                left.get_or_insert(x);
                right = Some(x);
            }
        }
        let (Some(left), Some(right)) = (left, right) else {
            continue;
        };
        if right - left + 1 < band_h * 3 {
            continue;
        }
        // 3) 裁剪窗口
        for (skip_top, skip_bottom) in crop_windows(band_h) {
            let top = band_top + skip_top;
            let bottom = band_bottom.saturating_sub(skip_bottom);
            if bottom <= top + 2 {
                continue;
            }
            if let Some(reading) = read_normalized(pixels, left, top, right, bottom, font) {
                if better_reading(&reading, best.as_ref()) {
                    best = Some(reading);
                }
            }
        }
    }
    best
}

/// 尝试的裁剪窗口：上下各多切掉 0~3 行（保证至少剩 4 行）。
///
/// 为什么需要这么多组合：候选带的边界是粗定位，可能多包进 1~3 行抗锯齿 /
/// 相邻元素；只有把那些行切掉，等比缩放的采样相位才正好压在真实字形上。
fn crop_windows(height: usize) -> Vec<(usize, usize)> {
    let mut windows = Vec::new();
    for skip_top in 0..=3usize {
        for skip_bottom in 0..=3usize {
            if height < 4 + skip_top + skip_bottom {
                continue;
            }
            windows.push((skip_top, skip_bottom));
        }
    }
    windows
}

/// 把墨迹矩形等比缩到 7 像素高，二值化后走点阵字形；读数矩形映射回原画面坐标。
fn read_normalized(
    pixels: &Pixels<'_>,
    x0: usize,
    y0: usize,
    x1: usize,
    y1: usize,
    font: &Font,
) -> Option<Reading> {
    let source_w = x1 - x0 + 1;
    let source_h = y1 - y0 + 1;
    if source_w < 8 || source_h < 4 {
        return None;
    }
    let out_w = ((source_w as f64) * (CELL_HEIGHT as f64) / (source_h as f64)).round() as usize;
    let out_w = out_w.clamp(8, 4096);
    let bgra = normalize_band(pixels, x0, y0, x1, y1, out_w, CELL_HEIGHT)?;
    let normalized = Pixels {
        width: out_w,
        height: CELL_HEIGHT,
        bgra: &bgra,
    };
    let reading = read_line_scaled(&normalized, font, SCALED_MATCH_TOLERANCE)?;
    let sx = source_w as f64 / out_w as f64;
    let sy = source_h as f64 / CELL_HEIGHT as f64;
    let map = |value: usize| (x0 as f64 + value as f64 * sx).round() as usize;
    let map_y = |value: usize| (y0 as f64 + value as f64 * sy).round() as usize;
    Some(Reading {
        rect: (
            map(reading.rect.0),
            map_y(reading.rect.1),
            map(reading.rect.2),
            map_y(reading.rect.3),
        ),
        text_height: source_h,
        ..reading
    })
}

/// 面积平均缩放到 `out_w × out_h`，再按自适应阈值二值化。
///
/// 阈值不写死 170：缩放会整体压暗笔画，而底色也可能不是纯黑（经验条是彩色的）。
/// 按分位数取「暗部 / 亮部」的中点，对比度太低（不是文字）直接判失败。
///
/// 逐**输出**像素收集源像素（而不是把源像素散到输出格子里）：放大时输出格
/// 比源多，散着放会留下没拿到像素的空格（5→7 时整行变成背景），
/// 收集式则每个输出格至少取一个源像素，放大缩小都对。
fn normalize_band(
    pixels: &Pixels<'_>,
    x0: usize,
    y0: usize,
    x1: usize,
    y1: usize,
    out_w: usize,
    out_h: usize,
) -> Option<Vec<u8>> {
    let w = x1 - x0 + 1;
    let h = y1 - y0 + 1;
    let mut average = vec![0u8; out_w * out_h];
    for oy in 0..out_h {
        let sy0 = y0 + oy * h / out_h;
        let sy1 = (y0 + (oy + 1) * h / out_h).max(sy0 + 1).min(y1 + 1);
        for ox in 0..out_w {
            let sx0 = x0 + ox * w / out_w;
            let sx1 = (x0 + (ox + 1) * w / out_w).max(sx0 + 1).min(x1 + 1);
            let mut sum = 0u32;
            let mut count = 0u32;
            for sy in sy0..sy1 {
                for sx in sx0..sx1 {
                    sum += pixels.luma(sx, sy) as u32;
                    count += 1;
                }
            }
            average[oy * out_w + ox] = if count == 0 { 0 } else { (sum / count) as u8 };
        }
    }
    let mut sorted = average.clone();
    sorted.sort_unstable();
    let lo = sorted[sorted.len() * 20 / 100] as u16;
    let hi = sorted[sorted.len() * 85 / 100] as u16;
    if hi.saturating_sub(lo) < 24 {
        return None;
    }
    let threshold = ((lo + hi) / 2) as u8;
    let mut bgra = vec![0u8; out_w * out_h * 4];
    for (index, value) in average.iter_mut().enumerate() {
        let ink = if *value >= threshold { 255u8 } else { 20u8 };
        let offset = index * 4;
        bgra[offset] = ink;
        bgra[offset + 1] = ink;
        bgra[offset + 2] = ink;
        bgra[offset + 3] = 255;
    }
    Some(bgra)
}

/// 读数的可信度分级（与 `read_line` 内部的评分同一套口径）。
pub(crate) fn reading_rank(reading: &Reading) -> u8 {
    match crate::exp::table::validate(reading.exp, reading.percent) {
        crate::exp::table::Verdict::Known(_) => 0,
        crate::exp::table::Verdict::Ambiguous => 1,
        crate::exp::table::Verdict::Contradiction => 2,
    }
}

/// 两个读数哪个更可信：先比自洽等级，再比逐像素命中的字形数。
pub(crate) fn better_reading(candidate: &Reading, current: Option<&Reading>) -> bool {
    match current {
        None => true,
        Some(current) => {
            let (new_rank, old_rank) = (reading_rank(candidate), reading_rank(current));
            new_rank < old_rank || (new_rank == old_rank && candidate.exact > current.exact)
        }
    }
}


/// 画面里那些**认不出**的字形（诊断用）。
///
/// 正常情况下永远是空的 —— 14 个符号全都内置了。它一旦非空，含义只有一个：
/// **游戏换了字体或改了缩放**。把点阵写进日志，就能一眼分清
/// 「画面里根本没有那一行」和「有字但对不上」，这两种情况要做的事完全不同。
pub fn unmatched_glyphs(pixels: &Pixels<'_>, font: &Font) -> Vec<(usize, String)> {
    if pixels.height < CELL_HEIGHT {
        return Vec::new();
    }
    for band_y in 0..=(pixels.height - CELL_HEIGHT) {
        let mut found = Vec::new();
        for glyph in segment(pixels, band_y) {
            if font.match_glyph(&glyph.bitmap).is_none() {
                found.push((glyph.x0, glyph.bitmap.to_ascii()));
            }
        }
        if !found.is_empty() {
            return found;
        }
    }
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 真机素材：从真实抓屏里裁下来的底部一条（240×41），含 `EXP 427096(46.09%)`。
    fn real_frame() -> Vec<u8> {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/exp_field_real_frame.bgra");
        std::fs::read(&path).expect("读不到真机画面素材")
    }

    /// 造一段「已经切好的字形」，用来单独测定位逻辑。
    ///
    /// `gap` 是字形之间空几列 —— 真机上它是变的（数字 `1` 只有 2 像素宽，
    /// 旁边就会空 3 列），所以测试里要能指定。`?` 表示认不出的字形。
    fn line(text: &str, gap: usize) -> (Vec<Glyph>, Vec<Option<char>>) {
        let font = Font::builtin();
        let (mut glyphs, mut slots) = (Vec::new(), Vec::new());
        let mut x = 0usize;
        for ch in text.chars() {
            // 空格不是字形，只是一段空白（状态栏里 `EXP` 和数字之间就是它）
            if ch == ' ' {
                x += 5;
                continue;
            }
            let bitmap = if ch == '?' {
                Bitmap::from_ascii(&["#", "#", "#", "#", "#", "#", "#"])
            } else {
                font.bitmap_of(ch)
                    .unwrap_or_else(|| panic!("字形表里应该有 {ch}"))
                    .clone()
            };
            let width = bitmap.width();
            glyphs.push(Glyph {
                x0: x,
                x1: x + width - 1,
                bitmap,
            });
            slots.push(if ch == '?' { None } else { Some(ch) });
            x += width + gap;
        }
        (glyphs, slots)
    }

    /// 内置字形表必须是**完整的**。
    ///
    /// 这一条是「不做运行时学习」这个决定的地基：只要有一个符号缺席，
    /// 含它的那些经验值就整帧读不到（真机上缺一个 `1`，成功率掉到 16%）。
    /// `EXP` 三个标签字也在里面 —— 它们的作用是给数字定左边界。
    #[test]
    fn builtin_font_is_complete() {
        let font = Font::builtin();
        assert_eq!(font.known(), "%().0123456789EPX", "认识的符号不对");
        for digit in '0'..='9' {
            assert!(font.bitmap_of(digit).is_some(), "缺数字 {digit}");
        }
        for symbol in ['(', ')', '.', '%', 'E', 'X', 'P'] {
            assert!(font.bitmap_of(symbol).is_some(), "缺符号 {symbol}");
        }
        assert_eq!(BUILTIN.len(), 17);
    }

    /// 每个字形的高度都必须是 7 —— 切成别的行数就不可能匹配上。
    #[test]
    fn every_glyph_is_seven_rows_tall() {
        for (ch, rows) in BUILTIN {
            assert_eq!(rows.len(), CELL_HEIGHT, "{ch} 的行数不对");
            let width = rows[0].len();
            assert!(rows.iter().all(|row| row.len() == width), "{ch} 各行宽度不一致");
        }
    }

    /// 内置字形两两不能同形 —— 同形会让匹配永远并列，那个字符就永远认不出。
    #[test]
    fn no_two_glyphs_are_identical() {
        let font = Font::builtin();
        for (index, (a, bitmap_a)) in BUILTIN.iter().enumerate() {
            for (b, bitmap_b) in BUILTIN.iter().skip(index + 1) {
                let distance = Bitmap::from_ascii(bitmap_a).distance(&Bitmap::from_ascii(bitmap_b));
                if let Some(distance) = distance {
                    assert!(
                        distance > MATCH_TOLERANCE,
                        "{a} 和 {b} 只差 {distance} 个像素 —— 会永远认不出"
                    );
                }
            }
        }
        // 顺带确认 2 像素宽的那两个（`1` `(`）确实是可区分的
        let one = font.bitmap_of('1').unwrap();
        let open = font.bitmap_of('(').unwrap();
        assert_eq!(one.width(), open.width());
        assert!(one.distance(open).unwrap() > MATCH_TOLERANCE);
    }

    /// 真机帧必须能读出来（`EXP 427096(46.09%)`）。
    #[test]
    fn reads_the_real_captured_frame() {
        let raw = real_frame();
        let pixels = Pixels { width: 240, height: 41, bgra: &raw };
        let reading = read_line(&pixels, &Font::builtin()).expect("真机画面应该能读出来");
        println!("真机帧读数：{} → {} / {}", reading.raw, reading.exp, reading.percent);
        assert_eq!(reading.exp, 427_096);
        assert_eq!(reading.percent, 46.09);
        assert_eq!(
            crate::exp::table::validate(reading.exp, reading.percent),
            crate::exp::table::Verdict::Known(55),
            "三个数必须和经验表自洽"
        );
    }

    /// 真机 1080p 帧走**完整的新管线**（`read_frame`：候选 + 缩放）也要一次读通，
    /// 而且读出来的是点阵字形、不是 OCR —— 快路不能因为加了多尺度就退化。
    #[test]
    fn the_real_frame_reads_through_the_full_pipeline_without_ocr() {
        let raw = real_frame();
        let pixels = Pixels { width: 240, height: 41, bgra: &raw };
        let reading = read_frame(&pixels, &Font::builtin()).expect("真机帧应能读出");
        assert_eq!((reading.exp, reading.percent), (427_096, 46.09));
        assert!(!reading.raw.contains("[OCR"), "1080p 快路不得走 OCR");
        assert_eq!(reading.text_height, 7, "1080p 的字高实测应为 7");
        // 同一帧也要能被候选检测框出来（自动定位走的就是这条路）
        let candidates = find_candidate_boxes(&pixels, 5);
        println!("真机帧候选：{candidates:?}");
        assert!(!candidates.is_empty(), "真机帧应有候选横带");
        assert!(
            candidates.iter().any(|candidate| (5..=9).contains(&candidate.text_height)),
            "真机帧里应有一条字高在 5~9 的候选：{candidates:?}"
        );
    }

    /// **整幅 1080p 画面里也要找得到那一行**（自动定位的真实场景）。
    ///
    /// 上半部分是「游戏世界」噪声（每行都有大量亮暗跃迁 —— 真机上是地形/粒子），
    /// 下半部分是深色 HUD；把真机底条贴在底部。候选检测必须：
    /// ① 不被几百行的连续噪声整段吞掉（滑窗拆分）；
    /// ② 在 HUD 里把那一行单独切出来；
    /// ③ 最终读数逐字正确。
    #[test]
    fn the_real_strip_reads_inside_a_full_hd_frame_with_a_noisy_world() {
        let raw = real_frame();
        let (strip_w, strip_h) = (240usize, 41usize);
        let (width, height) = (1920usize, 1080usize);
        let mut bgra = vec![20u8; width * height * 4];
        // 「游戏世界」：确定性噪声，保证每一行都合格（形成超长连续区间）
        for y in 0..height * 7 / 10 {
            for x in 0..width {
                let value = if (x / 3 + y / 2) % 2 == 0 { 200u8 } else { 30u8 };
                let offset = (y * width + x) * 4;
                bgra[offset] = value;
                bgra[offset + 1] = value;
                bgra[offset + 2] = value;
                bgra[offset + 3] = 255;
            }
        }
        // 真机底条贴进 HUD（HUD 区保持深色底）
        let origin = (100usize, height - strip_h - 20);
        for row in 0..strip_h {
            let source = row * strip_w * 4;
            let target = ((origin.1 + row) * width + origin.0) * 4;
            bgra[target..target + strip_w * 4]
                .copy_from_slice(&raw[source..source + strip_w * 4]);
        }
        let pixels = Pixels {
            width,
            height,
            bgra: &bgra,
        };

        let candidates = find_candidate_boxes(&pixels, MAX_CANDIDATES);
        let expected = {
            let strip_pixels = Pixels {
                width: strip_w,
                height: strip_h,
                bgra: &raw,
            };
            let line = read_line(&strip_pixels, &Font::builtin()).expect("真机底条应能读出");
            (
                origin.0 + line.rect.0,
                origin.1 + line.rect.1,
                origin.0 + line.rect.2,
                origin.1 + line.rect.3,
            )
        };
        let hud = candidates
            .iter()
            .find(|candidate| {
                let (x, y, w, h) = candidate.rect;
                x <= expected.0 && y <= expected.1 && x + w >= expected.2 && y + h >= expected.3
            })
            .unwrap_or_else(|| panic!("应切出覆盖真机那一行的候选：{candidates:?}"));
        assert!(
            (5..=9).contains(&hud.text_height),
            "HUD 候选的字高应接近真机的 7：{hud:?}"
        );

        let reading = read_frame(&pixels, &Font::builtin()).expect("整幅画面里应能读出真机那一行");
        assert_eq!((reading.exp, reading.percent), (427_096, 46.09));
        assert!(!reading.raw.contains("[OCR"), "1080p 真机帧不该走 OCR");
    }

    /// **字段定位不能看空隙**：把数字 `1`（2 像素宽）夹在中间也要能定位。
    ///
    /// 这条钉的就是真机上那个 bug：`1` 左边空 3 像素，旧实现「空隙 > 2 就断开」
    /// 于是把字段切成两半，整帧作废。
    #[test]
    fn a_narrow_digit_does_not_split_the_field() {
        // 逐个字形之间空 3 列 —— 真机上数字 1 造成的空隙就是这个宽度
        let (glyphs, slots) = line("EXP12145(60.08%)", 3);
        let field = locate(&glyphs, &slots).expect("应该定位到字段");
        assert_eq!(field.exp, 12_145);
        assert_eq!(field.percent, 60.08);
        // 而且它必须是**完整的**数字，不能把开头的 1 丢掉
        assert_eq!(slots[field.start], Some('1'), "开头那一位不能被丢掉");
    }

    /// 认不出的字形**紧挨**着数字时，整帧作废 —— 绝不能悄悄缩短数字。
    ///
    /// `?2145(60.08%)` 里第一位认不出：如果按「能读多少读多少」，会得到 `2145`，
    /// 而 `2145 ÷ 20216 = 10.61%` 对不上 60.08% —— 经验表**这一次**能拦住；
    /// 但万一缩出来的数字恰好自洽，就会被当成真数据。所以这里直接拒读。
    #[test]
    fn an_unmatched_glyph_next_to_the_number_blocks_the_whole_field() {
        let (glyphs, slots) = line("EXP?2145(60.08%)", 1);
        assert!(locate(&glyphs, &slots).is_none(), "认不出的字形挨着数字时不能给数字");
    }

    /// 反过来：认不出的字形离得**够远**（空白 ≥ MIN_NUMBER_GAP）时，它显然不是
    /// 数字的一部分，不该因为它整帧作废。
    #[test]
    fn an_unmatched_glyph_far_from_the_number_is_tolerated() {
        let (glyphs, slots) = line("EXP?12145(60.08%)", MIN_NUMBER_GAP + 2);
        let field = locate(&glyphs, &slots).expect("离得够远就该照读");
        assert_eq!(field.exp, 12_145);
    }

    /// 百分比必须是两位小数、括号成对；格式不对就整帧作废。
    #[test]
    fn the_format_is_enforced_strictly() {
        for (text, ok) in [
            ("12145(60.08%)", true),
            ("0(0.00%)", true),
            ("999999999(100.00%)", true),
            ("12145(60.8%)", false),  // 只有一位小数
            ("12145(60.08%", false),  // 少了右括号
            ("12145 60.08%)", false), // 少了左括号
            ("(60.08%)", false),      // 没有经验数字
            ("12145(160.08%)", false),// 百分比超过 100
            ("1234567890(0.00%)", false), // 数字位数超上限
        ] {
            let (glyphs, slots) = line(text, 1);
            assert_eq!(
                locate(&glyphs, &slots).is_some(),
                ok,
                "{text} 的判定不对（期望 {ok}）"
            );
        }
    }

    /// 末尾多一个 `)` 时取**右边**那个（真实状态栏里 `%)` 后面还挤着别的东西）。
    #[test]
    fn the_rightmost_anchor_wins() {
        let (glyphs, slots) = line("12145(60.08%)))", 1);
        let field = locate(&glyphs, &slots).expect("应该定位到字段");
        assert_eq!(field.exp, 12_145);
        assert_eq!(slots[field.end], Some(')'));
    }

    /// 「同形并列」必须判认不出：两个模板一模一样时，任何匹配都是并列。
    ///
    /// 这一条是**故意**的：宁可认不出、整帧作废，也不能在两个同样接近的候选里
    /// 挑一个 —— 挑错一位数字比读不到严重得多。
    /// （内置表本身保证两两不同形，见 `no_two_glyphs_are_identical`。）
    #[test]
    fn two_identical_templates_are_ambiguous() {
        let mut font = Font::builtin();
        let zero = font.bitmap_of('0').unwrap().clone();
        font.entries.push(('Z', zero.clone()));
        assert_eq!(font.match_glyph(&zero), None, "并列时必须判认不出");
    }

    /// 空白的行不该被当成经验字段。
    #[test]
    fn a_blank_line_is_not_a_reading() {
        let bgra = vec![20u8; 200 * 20 * 4];
        let pixels = Pixels { width: 200, height: 20, bgra: &bgra };
        assert!(read_line(&pixels, &Font::builtin()).is_none());
        assert!(pixels.looks_blank());
    }

    /// 渲染文本到测试画面内存的辅助函数。
    fn render_test_frame(lines: &[(usize, usize, &str)], width: usize, height: usize) -> Vec<u8> {
        let font = Font::builtin();
        let mut bgra = vec![20u8; width * height * 4];
        for &(start_x, start_y, text) in lines {
            let mut cur_x = start_x;
            for ch in text.chars() {
                if ch == ' ' {
                    cur_x += 4;
                    continue;
                }
                if let Some(bitmap) = font.bitmap_of(ch) {
                    for (row_idx, row) in bitmap.rows.iter().enumerate() {
                        let y = start_y + row_idx;
                        if y >= height {
                            continue;
                        }
                        for (col_idx, &ink) in row.iter().enumerate() {
                            let x = cur_x + col_idx;
                            if x >= width {
                                continue;
                            }
                            let offset = (y * width + x) * 4;
                            let val = if ink { 255u8 } else { 20u8 };
                            bgra[offset] = val;
                            bgra[offset + 1] = val;
                            bgra[offset + 2] = val;
                            bgra[offset + 3] = 255;
                        }
                    }
                    cur_x += bitmap.width() + 1;
                }
            }
        }
        bgra
    }

    /// 合法多解的 0 经验帧（Ambiguous）必须胜过更长的经验表矛盾错帧（Contradiction）（P1-4 回归测试）。
    #[test]
    fn ambiguous_zero_exp_beats_longer_contradiction_noise() {
        let font = Font::builtin();
        // 构造包含两行的画面：
        // 第 1 行 (y=0): "EXP 0(0.00%)" —— 合法升级归零帧，Ambiguous，字符少
        // 第 2 行 (y=12): "EXP 12345(67.89%)" —— 错位噪点凑成的行，Contradiction，字符多
        let bgra = render_test_frame(
            &[
                (10, 0, "EXP 0(0.00%)"),
                (10, 12, "EXP 12345(67.89%)"),
            ],
            180,
            24,
        );
        let pixels = Pixels {
            width: 180,
            height: 24,
            bgra: &bgra,
        };
        let reading = read_line(&pixels, &font).expect("应该成功读出有效行");
        assert_eq!(reading.exp, 0, "应优先选择 Ambiguous 的 0 经验行，而非 Contradiction 错行");
        assert_eq!(reading.percent, 0.0);
    }

    /// 删掉 early break 后，exact 择优能正确选出垂直居中最佳的横带（P1-3 回归测试）。
    #[test]
    fn exact_tie_breaker_picks_cleanest_vertical_band() {
        let font = Font::builtin();
        // 构造两行都是合法经验值（Known 55 级）的带：
        // 第 1 行 (y=0): "EXP 427096(46.09%)"，但有 1 个像素噪点（使其 exact 不是满分，但距离 <= MATCH_TOLERANCE 仍能匹配）
        // 第 2 行 (y=10): "EXP 427096(46.09%)"，完美无噪点（exact 满分）
        let mut bgra = render_test_frame(
            &[
                (10, 0, "EXP 427096(46.09%)"),
                (10, 10, "EXP 427096(46.09%)"),
            ],
            200,
            22,
        );

        // 在第 1 行故意翻转 1 个墨点像素，制造微小容差匹配 (y=0 处 'E' 的第一笔墨点)
        bgra[(0 * 200 + 10) * 4] = 20;
        bgra[(0 * 200 + 10) * 4 + 1] = 20;
        bgra[(0 * 200 + 10) * 4 + 2] = 20;

        let pixels = Pixels {
            width: 200,
            height: 22,
            bgra: &bgra,
        };
        let reading = read_line(&pixels, &font).expect("应该成功读出");
        assert_eq!(reading.exp, 427_096);
        assert_eq!(reading.percent, 46.09);
        // 旧实现因为碰到 y=0 的 rank==0 就 break，选出的 rect 顶端是 0
        // 修复后不提前退出，会挑出 y=10 处 exact 最高的完美横带
        assert_eq!(reading.rect.1, 10, "应该择优选中 exact 更高的 y=10 横带，而不是在 y=0 提前退出");
    }

    /// 按倍率缩放渲染文本（测试非 1080p 分辨率下的候选框提取）。
    fn render_scaled_text(
        text: &str,
        start_x: usize,
        start_y: usize,
        scale: f64,
        width: usize,
        height: usize,
    ) -> (Vec<u8>, usize, usize) {
        let font = Font::builtin();
        let mut bgra = vec![20u8; width * height * 4];
        let mut cur_x = start_x;
        let scaled_h = (CELL_HEIGHT as f64 * scale).round() as usize;

        for ch in text.chars() {
            if ch == ' ' {
                cur_x += (4.0 * scale).round() as usize;
                continue;
            }
            if let Some(bitmap) = font.bitmap_of(ch) {
                let scaled_w = (bitmap.width() as f64 * scale).round() as usize;
                for target_y in 0..scaled_h {
                    let src_y = (target_y as f64 / scale).floor() as usize;
                    if src_y >= bitmap.height() {
                        continue;
                    }
                    let y = start_y + target_y;
                    if y >= height {
                        continue;
                    }
                    for target_x in 0..scaled_w {
                        let src_x = (target_x as f64 / scale).floor() as usize;
                        if src_x >= bitmap.width() {
                            continue;
                        }
                        let x = cur_x + target_x;
                        if x >= width {
                            continue;
                        }
                        if bitmap.rows[src_y][src_x] {
                            let offset = (y * width + x) * 4;
                            bgra[offset] = 255;
                            bgra[offset + 1] = 255;
                            bgra[offset + 2] = 255;
                            bgra[offset + 3] = 255;
                        }
                    }
                }
                cur_x += scaled_w + (1.0 * scale).round() as usize;
            }
        }

        let drawn_w = cur_x.saturating_sub(start_x);
        (bgra, drawn_w, scaled_h)
    }

    /// 按倍率**双线性**渲染文本 —— 更接近游戏把整个界面按比例缩放时的画面
    /// （放大后有灰度抗锯齿、没有整行重复/丢失）。
    fn render_bilinear_text(
        text: &str,
        start_x: usize,
        start_y: usize,
        scale: f64,
        width: usize,
        height: usize,
    ) -> (Vec<u8>, usize, usize) {
        let font = Font::builtin();
        let mut bgra = vec![20u8; width * height * 4];
        let scaled_h = (CELL_HEIGHT as f64 * scale).round() as usize;
        let mut cur_x = start_x as f64;
        for ch in text.chars() {
            if ch == ' ' {
                cur_x += 4.0 * scale;
                continue;
            }
            let Some(bitmap) = font.bitmap_of(ch) else {
                continue;
            };
            let scaled_w = (bitmap.width() as f64 * scale).round() as usize;
            if scaled_w == 0 || scaled_h == 0 {
                continue;
            }
            for oy in 0..scaled_h {
                let sy = ((oy as f64 + 0.5) / scale - 0.5).clamp(0.0, (CELL_HEIGHT - 1) as f64);
                let sy0 = sy.floor() as usize;
                let sy1 = (sy0 + 1).min(CELL_HEIGHT - 1);
                let fy = sy - sy0 as f64;
                for ox in 0..scaled_w {
                    let sx = ((ox as f64 + 0.5) / scale - 0.5)
                        .clamp(0.0, (bitmap.width() - 1) as f64);
                    let sx0 = sx.floor() as usize;
                    let sx1 = (sx0 + 1).min(bitmap.width() - 1);
                    let fx = sx - sx0 as f64;
                    let at = |x: usize, y: usize| if bitmap.rows[y][x] { 1.0 } else { 0.0 };
                    let value = at(sx0, sy0) * (1.0 - fx) * (1.0 - fy)
                        + at(sx1, sy0) * fx * (1.0 - fy)
                        + at(sx0, sy1) * (1.0 - fx) * fy
                        + at(sx1, sy1) * fx * fy;
                    let luma = (20.0 + value * 235.0).round() as u8;
                    let x = cur_x as usize + ox;
                    let y = start_y + oy;
                    if x < width && y < height {
                        let offset = (y * width + x) * 4;
                        bgra[offset] = luma;
                        bgra[offset + 1] = luma;
                        bgra[offset + 2] = luma;
                        bgra[offset + 3] = 255;
                    }
                }
            }
            cur_x += scaled_w as f64 + scale;
        }
        (bgra, cur_x.round() as usize - start_x, scaled_h)
    }

    /// 验证非 1080p 缩放字形下粗定位能够正确框出文字区域并测准字高。
    #[test]
    fn candidate_box_finds_scaled_text_and_detects_height() {
        let text = "EXP 427096(46.09%)";
        let (start_x, start_y) = (40, 15);
        let scale = 1.33; // 1440p 下字高缩放比 (7 -> ~9)
        let (bgra, drawn_w, drawn_h) = render_scaled_text(text, start_x, start_y, scale, 300, 50);
        let pixels = Pixels {
            width: 300,
            height: 50,
            bgra: &bgra,
        };

        // 1. 验证既有的 7px 严格点阵比对确实无法读出（这正是用户在 1440p 遭遇的 no_field）
        let font = Font::builtin();
        assert!(
            read_line(&pixels, &font).is_none(),
            "1.33 倍缩放下的点阵字形必须被既有字形表判定为无法匹配（证明问题现场存在）"
        );

        // 2. 粗定位必须成功交出候选矩形与实际字高
        let cand = find_candidate_box(&pixels).expect("必须成功检测出候选文字横带");

        // 验证候选框完全包含实际绘制的文字范围
        assert!(cand.rect.0 <= start_x, "裁剪框左边界必须包含文字起点");
        assert!(cand.rect.0 + cand.rect.2 >= start_x + drawn_w, "裁剪框右边界必须包含文字终点");
        assert!(cand.rect.1 <= start_y, "裁剪框上边界必须包含文字顶端");
        assert!(cand.rect.1 + cand.rect.3 >= start_y + drawn_h, "裁剪框下边界必须包含文字底端");

        // 验证检测到的文字笔画高度准确对应 1.33 倍后的字高 (约 9 像素)
        assert_eq!(cand.text_height, drawn_h, "检测出的文字笔画高度必须准确对应 9 像素");
    }

    /// **这一条就是「换分辨率也能读」的回归测试（字形路径）**。
    ///
    /// 点阵字形在整数倍缩放（2×，4K 下游戏界面常见的一档）下必须照样读出。
    /// 非整数倍（1.33 等）由 OCR 接力，见 `ocr_reads_the_scaled_exp_line`。
    #[test]
    fn scaled_font_reads_the_exp_line_at_integer_ui_scales() {
        let font = Font::builtin();
        type Render = fn(&str, usize, usize, f64, usize, usize) -> (Vec<u8>, usize, usize);
        for (label, render) in [("最近邻", render_scaled_text), ("双线性", render_bilinear_text)] as
            [(&str, Render); 2]
        {
            for scale in [1.0, 2.0] {
                let (bgra, drawn_w, drawn_h) =
                    render("EXP 427096(46.09%)", 20, 10, scale, 500, 80);
                let pixels = Pixels {
                    width: 500,
                    height: 80,
                    bgra: &bgra,
                };
                let reading = read_frame(&pixels, &font).unwrap_or_else(|| {
                    panic!("{label} {scale} 倍缩放下必须能读出经验行（字高 {drawn_h}，宽 {drawn_w}）")
                });
                assert_eq!(reading.exp, 427_096, "{label} {scale} 倍下经验数字不对：{}", reading.raw);
                assert_eq!(reading.percent, 46.09, "{label} {scale} 倍下百分比不对：{}", reading.raw);
                assert_eq!(
                    crate::exp::table::validate(reading.exp, reading.percent),
                    crate::exp::table::Verdict::Known(55),
                    "{label} {scale} 倍下必须与经验表自洽"
                );
                assert_eq!(reading.text_height, drawn_h, "{label} {scale} 倍下应如实报出实测字高");
            }
        }
    }

    /// **非整数倍缩放（1440p 这类）由 PP-OCR 接力。**
    ///
    /// 这条测试同时钉住两件事：候选横带能把那一行框出来，且 PP-OCR 的识别结果
    /// 能过 `parse_exp_ocr` 的经验表校验（不是「看着差不多」）。
    ///
    /// 最近邻一档只测放大：最近邻**下采样**会整行丢像素（7 行只剩 5 行），
    /// 那不是游戏的渲染方式（游戏是双线性缩放），没有必要为它设计算法。
    #[test]
    fn ocr_reads_the_scaled_exp_line() {
        type Render = fn(&str, usize, usize, f64, usize, usize) -> (Vec<u8>, usize, usize);
        let cases: [(&str, Render, &[f64]); 2] = [
            ("最近邻", render_scaled_text, &[1.15, 1.33, 1.5]),
            ("双线性", render_bilinear_text, &[0.7, 0.85, 1.15, 1.33, 1.5]),
        ];
        for (label, render, scales) in cases {
            for scale in scales {
                let scale = *scale;
                let (bgra, _, _) = render("EXP 427096(46.09%)", 20, 10, scale, 500, 80);
                let pixels = Pixels {
                    width: 500,
                    height: 80,
                    bgra: &bgra,
                };
                let candidate = find_candidate_boxes(&pixels, 4)
                    .into_iter()
                    .next()
                    .unwrap_or_else(|| panic!("{label} {scale} 倍下应检出候选横带"));
                let (cw, ch, buffer) = pixels.crop(
                    candidate.rect.0,
                    candidate.rect.1,
                    candidate.rect.2,
                    candidate.rect.3,
                );
                let crop = Pixels {
                    width: cw,
                    height: ch,
                    bgra: &buffer,
                };
                let reading = crate::exp::ocr::recognize_line(&crop)
                    .unwrap_or_else(|| panic!("{label} {scale} 倍下 OCR 应认出文字"));
                assert_eq!(
                    crate::exp::ocr::parse_exp_ocr(&reading.text),
                    Some((427_096, 46.09)),
                    "{label} {scale} 倍下 OCR 文本「{}」应解析并过经验表",
                    reading.text
                );
            }
        }
    }

    /// 画面里有多行文字时，要挑出**过了经验表**的那一行，而不是笔画最多的聊天行。
    #[test]
    fn read_frame_prefers_the_line_that_passes_the_exp_table() {
        let font = Font::builtin();
        // 上面一条聊天行（笔画密度更高），下面才是经验行。
        let bgra = render_test_frame(
            &[
                (8, 2, "GZG: 出售 1000000000 mesos 12345678901234567890"),
                (8, 22, "EXP 12145(60.08%)"),
            ],
            420,
            40,
        );
        let pixels = Pixels {
            width: 420,
            height: 40,
            bgra: &bgra,
        };
        let reading = read_frame(&pixels, &font).expect("必须读出经验行");
        assert_eq!(reading.exp, 12_145);
        assert_eq!(reading.percent, 60.08);
        assert_eq!(
            crate::exp::table::validate(reading.exp, reading.percent),
            crate::exp::table::Verdict::Known(20)
        );
    }

    /// 缩放读数交出的矩形必须能映射回原画面（校准页/自动定位都靠它）。
    #[test]
    fn scaled_reading_maps_its_rect_back_to_the_source_pixels() {
        let font = Font::builtin();
        let (bgra, drawn_w, drawn_h) = render_scaled_text("EXP 427096(46.09%)", 30, 12, 2.0, 500, 80);
        let pixels = Pixels {
            width: 500,
            height: 80,
            bgra: &bgra,
        };
        let reading = read_frame(&pixels, &font).expect("2 倍下必须读出");
        let (x0, y0, x1, y1) = reading.rect;
        // 矩形要落在绘制区域内（只要求不越界，不要求逐像素贴边）
        assert!(x0 >= 20 && y0 >= 5, "rect 左/上越界：{:?}", reading.rect);
        assert!(x1 <= 40 + drawn_w && y1 <= 18 + drawn_h, "rect 右/下越界：{:?}", reading.rect);
        assert!(x1 > x0 && y1 > y0);
    }

}
