//! 抓游戏窗口的画面（只抓，不识别）。
//!
//! ## 为什么必须抓**窗口**，而不是「屏幕上那一块」
//!
//! 这是「经常识别不到」的头号原因，而且是实测出来的：
//! 同一个探针，在「游戏在前台」和「游戏被浏览器挡住」两种情况下各跑一遍，
//! 比较三种抓法的墨点占比（经验条那一行的合理值在 20% 上下）：
//!
//! | 场景 | 屏幕 DC（旧实现） | PrintWindow(0) | PrintWindow(2) |
//! | --- | --- | --- | --- |
//! | 游戏在前台 | 20.2% ✅ | 20.2% ✅ | 20.2% ✅ |
//! | **游戏被浏览器挡住** | **100.0% ❌** | **20.2% ✅** | **20.2% ✅** |
//!
//! 屏幕 DC 抓的是「屏幕上那个矩形现在显示的像素」—— 游戏上面压着任何窗口
//! （浏览器、聊天窗、别的工具），抓回来的就是**那个窗口**的像素，于是识别层
//! 报「画面上没有经验那一行」，把那句话读成「不在角色里 / 去校准一次」，
//! 把人引向完全错误的方向。
//!
//! `PrintWindow` 抓的是**窗口自己**，合成在哪一层、上面压着谁都不影响。
//! 这就是参考工具说明里那句「后台截屏」的意思，也是它「不会说找不到画面」的原因。
//!
//! ## 旧注释说「PrintWindow 会把调用线程挂死」，那是误判
//!
//! 本机实测（同一个 Unity/D3D11 窗口）：`PrintWindow(0)` 13.6 ms、
//! `PrintWindow(2)` 24.2 ms、屏幕 DC 3.5 ms，三者**像素逐点相同**，都不挂。
//! 当年那次挂死多半是对**最小化**窗口调用，或者申请了尺寸不对的位图 ——
//! `PrintWindow` 永远从**窗口**左上角开始渲染整个窗口（含标题栏），
//! 按客户区坐标申请小位图就会拿到标题栏的内容（这个错误我这次也犯了一遍，
//! 所以在这里写下来）。
//!
//! ## 代价
//!
//! 每帧要多花十几毫秒（渲染整个窗口 1936×1119 再裁出底部那一条，
//! 高度见 `strip_height`）。采样间隔是 1~2 秒，这点开销可以忽略。
//! 抓回来的只有底部那一条（44 行时约 338 KB）。
//!
//! ## 独占全屏：`PrintWindow` 会「成功返回，画面全黑」
//!
//! 独占全屏的 D3D 窗口把画面直接送进扫描输出，`WM_PRINT` 问得到的表面里
//! 什么都没有 —— 函数只是「画完了」，返回 TRUE，位图全黑。以前这种帧会
//! 直接交给上层，报成「游戏窗口还没画出来？」，把一个必须**改窗口模式**
//! 才能好的问题说成「等一下就好」。现在：黑了就再想一次屏幕 DC（只在游戏
//! 确实在前台时采信），两条路都黑就把「是不是全屏」这个现场一并交上去，
//! 由 `reader` 分开说（详见 `grab_client_fallback` 与 `ReadFailure::BlankFrame`）。

use crate::exp::font::{self, Pixels};

/// 底部条抓多少行 —— **跟着客户区高度走**，不再是一个写死的数字。
///
/// ## 为什么不能写死 44
///
/// 44 是在 1920×1080 真机上量的（经验那一行在最底下 30 行内，留到 44 容忍排布
/// 差异）。可换分辨率、尤其是游戏自己做了 UI 缩放之后，那一行会**离开**这 44 行
/// —— 于是每帧都报 NoField，文案还说「不在角色里」，而用户看着角色明明站着。
///
/// ## 条变高的代价（读完 `font::read_line` 之后的结论，下一个人别再重算一遍）
///
/// * **成本**：`read_line` 逐行扫 `0..=高度-7`，每带 7 像素高，成本是
///   O(条高 × 宽)。44 行 → 约 37 带、几十微秒；144 行 → 约 137 带、**1~3 毫秒**。
///   对比 PrintWindow 本身的 13 毫秒、采样 1~2 秒一次，完全可忽略。
/// * **误认风险**：条高了，候选带多了，确实更可能把别的文字切出一串数字 ——
///   但后面还挂着三道闸：① `segment` 要求 ≥7 个相邻字形；② `parse_at` 要求
///   形状就是 `12.34%)` 那一截（小数点、百分号、右括号一个都不能少）；③
///   `table::validate` 要求「经验 ÷ 本级所需 ≈ 百分比」自洽，评分先比这一项
///   再比 exact（完全匹配字形数），真经验行的 exact 永远压得住 HP/MP 那串数字。
///   所以放宽条高买到的是「低分辨率也能扫到那一行」，付出的是几毫秒 —— 值。
///
/// ## 数字怎么定的
///
/// * 下限 **44**：1080p 实测值，**绝不比改之前更差**；
/// * 比例 **10%**：1080p → 108 行、768 → 77 行、1440 → 144 行；
/// * 上限 **144**：再高只会把聊天框/按钮扫进来，而 144 已经覆盖 4K 按比例该有的
///   88 行（44 × 2160/1080），留了六成富余。
const STRIP_RATIO: f64 = 0.10;
const STRIP_HEIGHT_MIN: i32 = 44;
const STRIP_HEIGHT_MAX: i32 = 144;

/// 这一次该抓客户区最底下多少行（纯函数，单测拿不同分辨率钉住）。
///
/// 宁可多扫也不漏扫：条是**从下往上长**的，它永远包含旧的那 44 行，
/// 所以改之前能读到的画面，改之后只会看得更全。
pub fn strip_height(client_height: i32) -> i32 {
    if client_height <= 0 {
        return 0;
    }
    let scaled = (client_height as f64 * STRIP_RATIO).round() as i32;
    // .min(client_height) 兵底：窗口被拖成一条时，别去抓不存在的行
    scaled
        .clamp(STRIP_HEIGHT_MIN, STRIP_HEIGHT_MAX)
        .min(client_height)
}

/// 一帧的亮度粗统计 —— 诊断用：一眼分「亮底暗字 / 暗底亮字 / 一片纯色」。
///
/// 为什么用直方图的中位数而不是平均：一张**大面积纯色**的错图（拍到了别的窗口、
/// 或整片全黑）平均值也会很好看，中位数 + 亮像素占比一对比就露馅了 ——
/// 「中位 12 亮 8%」是暗底亮字，「中位 203 亮 96%」是亮底，「min=max」是纯色。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LumaStats {
    pub min: u8,
    pub median: u8,
    pub max: u8,
    /// 亮于 [`crate::exp::font::INK_THRESHOLD`] 的像素占比（0~1）
    pub bright_ratio: f64,
}

/// 全图扫一遍（不抽样）：诊断是**用户点一下才跑一次**的动作，
/// 2560×1440 ≈ 370 万次查表也就十几毫秒，而抽样会把「有没有一小片亮字」抽漏。
pub fn luma_stats(pixels: &Pixels<'_>) -> LumaStats {
    if pixels.width == 0 || pixels.height == 0 {
        return LumaStats {
            min: 0,
            median: 0,
            max: 0,
            bright_ratio: 0.0,
        };
    }
    let mut buckets = [0u64; 256];
    let mut total = 0u64;
    let mut bright = 0u64;
    for y in 0..pixels.height {
        for x in 0..pixels.width {
            let luma = pixels.luma(x, y);
            buckets[luma as usize] += 1;
            total += 1;
            if luma > font::INK_THRESHOLD {
                bright += 1;
            }
        }
    }
    let mut min = 0u8;
    while min < 255 && buckets[min as usize] == 0 {
        min += 1;
    }
    let mut max = 255u8;
    while max > 0 && buckets[max as usize] == 0 {
        max -= 1;
    }
    // 中位数：累计到一半的那个桶（小值优先，与直方图读法一致）
    let half = total.div_ceil(2);
    let mut seen = 0u64;
    let mut median = min;
    for value in min..=max {
        seen += buckets[value as usize];
        if seen >= half {
            median = value;
            break;
        }
    }
    LumaStats {
        min,
        median,
        max,
        bright_ratio: bright as f64 / total as f64,
    }
}

/// 降采样成 `cols × rows` 的粗图，**每格取块内最大亮度**。
///
/// 为什么取 max 不取平均：经验那一行只有 ~11 像素高，而 1440 行降到 45 行后
/// 每格高 32 像素 —— 平均会把「白字黑底」抹成一片深灰、直接掉到阈值以下，
/// 图上就成了「看起来什么都没有」，而那恰恰是我们最想看见的东西。
/// 取 max 保住「这一块里出现过亮像素」这个事实，小字号也留得住。
/// 代价是暗噪声同样会被放大，所以这张图**永远配着直方图一起看**
/// （中位数 + 亮像素占比），两者对不上就知道那是噪声不是字。
pub fn coarse_grid(pixels: &Pixels<'_>, cols: usize, rows: usize) -> Vec<Vec<u8>> {
    if cols == 0 || rows == 0 || pixels.width == 0 || pixels.height == 0 {
        return Vec::new();
    }
    let mut grid = vec![vec![0u8; cols]; rows];
    for row in 0..rows {
        let y0 = row * pixels.height / rows;
        let y1 = ((row + 1) * pixels.height / rows).max(y0 + 1).min(pixels.height);
        for col in 0..cols {
            let x0 = col * pixels.width / cols;
            let x1 = ((col + 1) * pixels.width / cols).max(x0 + 1).min(pixels.width);
            let mut peak = 0u8;
            for y in y0..y1 {
                for x in x0..x1 {
                    peak = peak.max(pixels.luma(x, y));
                }
            }
            grid[row][col] = peak;
        }
    }
    grid
}

/// 两张粗图有多像：逐格比较，返回亮度差在 `tolerance` 内的格子占比。
///
/// 用它回答「PrintWindow 和屏幕 DC 抓到的是不是同一份画面」。**不逐像素比**：
/// 两次抓取之间游戏自己也在动（动画、粒子、聊天滚屏），逐像素比永远“不同”，
/// 结论就废了；降到粗图再给容差，比的是**画面结构**而不是像素。
pub fn grid_similarity(a: &[Vec<u8>], b: &[Vec<u8>], tolerance: u8) -> f64 {
    if a.is_empty() || a.len() != b.len() {
        return 0.0;
    }
    let mut same = 0usize;
    let mut total = 0usize;
    for (row_a, row_b) in a.iter().zip(b.iter()) {
        if row_a.len() != row_b.len() {
            return 0.0;
        }
        for (value_a, value_b) in row_a.iter().zip(row_b.iter()) {
            total += 1;
            if value_a.abs_diff(*value_b) <= tolerance {
                same += 1;
            }
        }
    }
    if total == 0 {
        0.0
    } else {
        same as f64 / total as f64
    }
}

/// 窗口矩形是不是和显示器矩形重合到「可以当成全屏」的程度（纯函数）。
///
/// 判据要能拿任意分辨率在单测里验，而不是只能在真机上看 —— 这个数字直接
///决定 BlankFrame 给用户哪句话（全屏 vs 窗口没画出来），说错就把人引错方向。
///
/// `tolerance = 2`：独占全屏与无边框窗口通常逐像素等于屏幕，留 2 像素给
/// DPI 取整；**「最大化窗口」四条边都往外溢几像素，不算全屏** —— 它不是全屏，
/// 把它报成「全屏抓不到」只会误导。
fn rect_is_fullscreen(
    window: (i32, i32, i32, i32),
    monitor: (i32, i32, i32, i32),
    tolerance: i32,
) -> bool {
    let near = |a: i32, b: i32| (a - b).abs() <= tolerance;
    near(window.0, monitor.0)
        && near(window.1, monitor.1)
        && near(window.2, monitor.2)
        && near(window.3, monitor.3)
}

/// 窗口在屏幕上是什么**形态**。
///
/// **只有形态，不猜意图。** Win32 的样式位**区分不了「独占全屏」和「无边框全屏」**
/// —— 两者都可能是 `WS_POPUP`、都恰好铺满屏幕，从别的进程里也没法问 D3D 要答案。
/// 所以这里只分三档，形态决定**第一句话**；“到底是独占还是无边框”由实证给出：
/// 对比 PrintWindow 与屏幕 DC 抓到的两份画面（见 `preview()` 的两路对比行）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScreenMode {
    /// 没占满屏幕：普通窗口
    Windowed,
    /// 占满屏幕但**带**可调边框（WS_CAPTION / WS_THICKFRAME）—— 更像「最大化」，
    /// 不是游戏里那个全屏开关，不该建议用户“去改全屏模式”
    Framed,
    /// 占满屏幕且**无**边框 —— 游戏里叫「全屏」的东西多半是这一档
    /// （用户那句「更像无边框窗口」说的就是它）
    Borderless,
}

impl ScreenMode {
    /// 判据行与文案里用的短名
    pub fn label(self) -> &'static str {
        match self {
            ScreenMode::Windowed => "窗口",
            ScreenMode::Framed => "占满屏幕但带边框",
            ScreenMode::Borderless => "无边框全屏",
        }
    }
}

/// 形态判据（纯函数）：先看**占没占满**，再看**有没有边框**。
///
/// 抽出来是为了能在单测里拿任意分辨率 / 任意样式验，而不是只能在真机上看 —
/// 这个分类直接决定报错时给用户哪句话，分错了就是把人引向错的方向。
fn classify_screen_mode(
    window: (i32, i32, i32, i32),
    monitor: (i32, i32, i32, i32),
    has_frame: bool,
    tolerance: i32,
) -> ScreenMode {
    if !rect_is_fullscreen(window, monitor, tolerance) {
        return ScreenMode::Windowed;
    }
    if has_frame {
        ScreenMode::Framed
    } else {
        ScreenMode::Borderless
    }
}

/// 找到的游戏窗口。
#[derive(Debug, Clone)]
pub struct GameWindow {
    pub hwnd: isize,
    pub title: String,
}

/// 抓到的一帧画面（BGRA，行优先、自上而下）。
pub struct Frame {
    pub width: usize,
    pub height: usize,
    pub bgra: Vec<u8>,
}

impl Frame {
    pub fn pixels(&self) -> Pixels<'_> {
        Pixels {
            width: self.width,
            height: self.height,
            bgra: &self.bgra,
        }
    }
}

/// 抓屏是怎么完成的 —— 出错时给用户的话不一样，也方便排查。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    /// 后台截屏（`PrintWindow`）：抓的是窗口自己，不怕被挡住
    Window,
    /// 退回屏幕 DC：`PrintWindow` 失败**或拿到整片全黑**时用，且要求游戏在前台
    Screen,
}

/// 交给系统「这个进程按物理像素跟我说话」。
///
/// 少了这一步，缩放不是 100% 的显示器上算出来的坐标会被系统虚拟化：
/// 客户区尺寸和窗口尺寸会一起缩小，`PrintWindow` 渲染出来的却是物理像素，
/// 两者对不上就会裁错位置（而且照样「成功」，只是内容是错的）。
pub fn ensure_dpi_aware() {
    #[cfg(windows)]
    win::ensure_dpi_aware();
}

/// 找游戏窗口。返回 `None` = 游戏没开。
pub fn find_game_window() -> Option<GameWindow> {
    #[cfg(windows)]
    {
        win::find_game_window()
    }
    #[cfg(not(windows))]
    {
        None
    }
}

/// 这个句柄现在还是不是一个窗口（游戏重启后句柄就废了）。
pub fn is_window(hwnd: isize) -> bool {
    #[cfg(windows)]
    {
        win::is_window(hwnd)
    }
    #[cfg(not(windows))]
    {
        let _ = hwnd;
        false
    }
}

/// 窗口最小化了吗。
///
/// 最小化之后窗口不再渲染（DWM 也没有可合成的表面），`PrintWindow` 拿不到新画面。
/// 所以这种情况要单独说清楚，而不是报「画面里没有经验那一行」。
pub fn is_minimized(hwnd: isize) -> bool {
    #[cfg(windows)]
    {
        win::is_minimized(hwnd)
    }
    #[cfg(not(windows))]
    {
        let _ = hwnd;
        false
    }
}

/// 游戏窗口现在是不是前台窗口。
///
/// 屏幕 DC 的兜底只在这种情况下才可信：全屏时游戏必然在前台，屏幕上就是它自己；
/// 窗口模式被别的窗口盖住时，屏幕那块是**别人**的像素。
pub fn is_foreground(hwnd: isize) -> bool {
    #[cfg(windows)]
    {
        win::is_foreground(hwnd)
    }
    #[cfg(not(windows))]
    {
        let _ = hwnd;
        false
    }
}

/// 游戏窗口在屏幕上的形态（占没占满 / 带不带边框）。
///
/// 拿它把「全屏形态下抓到全黑」和「窗口还没画出来」分开说 —— 两者的处置完全不同：
/// 后者等一两帧就好，前者才需要考虑“改窗口模式”。
pub fn screen_mode(hwnd: isize) -> ScreenMode {
    #[cfg(windows)]
    {
        win::screen_mode(hwnd)
    }
    #[cfg(not(windows))]
    {
        let _ = hwnd;
        ScreenMode::Windowed
    }
}

/// 窗口类名（诊断用：万一抓错了窗口，这里是证据）。
pub fn window_class_name(hwnd: isize) -> String {
    #[cfg(windows)]
    {
        win::window_class(hwnd)
    }
    #[cfg(not(windows))]
    {
        let _ = hwnd;
        String::new()
    }
}

/// 客户区尺寸（宽, 高）。
pub fn client_size(hwnd: isize) -> Option<(i32, i32)> {
    #[cfg(windows)]
    {
        win::client_size(hwnd)
    }
    #[cfg(not(windows))]
    {
        let _ = hwnd;
        None
    }
}

/// 客户区在屏幕物理坐标里的位置和尺寸 `(x, y, w, h)` —— 校准蒙版定位用。
pub fn client_screen_rect(hwnd: isize) -> Option<(i32, i32, i32, i32)> {
    #[cfg(windows)]
    {
        win::client_screen_rect(hwnd)
    }
    #[cfg(not(windows))]
    {
        let _ = hwnd;
        None
    }
}

/// 把游戏窗口抬到最前面。只在用户主动点「手动校准」时调用（平时不抢焦点）。
pub fn bring_to_foreground(hwnd: isize) {
    #[cfg(windows)]
    {
        win::bring_to_foreground(hwnd);
    }
    #[cfg(not(windows))]
    {
        let _ = hwnd;
    }
}

/// 只给开发探针用：窗口被最小化时**不抢焦点**地把它恢复出来（`SW_SHOWNOACTIVATE`），
/// 好让 `PrintWindow` 能拿到画面做现场诊断。产品路径不调用它。
#[cfg(test)]
pub fn restore_without_activating(hwnd: isize) {
    #[cfg(windows)]
    {
        win::restore_without_activating(hwnd);
    }
    #[cfg(not(windows))]
    {
        let _ = hwnd;
    }
}

/// 抓游戏**客户区最底下那一条**（经验所在的地方）。
pub fn grab_strip(hwnd: isize) -> Option<(Frame, Method)> {
    let (width, height) = client_size(hwnd)?;
    let strip = strip_height(height);
    if strip <= 0 {
        return None;
    }
    grab_client(hwnd, 0, height - strip, width, strip)
}

/// 用**指定方式**抓一块（诊断与「认不出时再试一条」用）。
///
/// 平时走 `grab_client`：它会自己兜底、自己挑一条能用的路。诊断要的恰恰是
///「两条路**各自**抓到了什么」—— 这里不做任何兜底与挑选，各抓各的才比得出结论；
/// 读不出经验行时也用它换一条路再试一次（`read()` 那边拿「读不读得出来」验证）。
pub fn grab_client_by_method(
    hwnd: isize,
    x: i32,
    y: i32,
    width: i32,
    height: i32,
    method: Method,
) -> Option<Frame> {
    if width <= 0 || height <= 0 {
        return None;
    }
    #[cfg(windows)]
    {
        match method {
            Method::Window => win::grab_window(hwnd, x, y, width, height),
            Method::Screen => win::grab_screen(hwnd, x, y, width, height),
        }
    }
    #[cfg(not(windows))]
    {
        let _ = (hwnd, x, y, width, height, method);
        None
    }
}

/// 后台截屏拿到**整片全黑**时的处置：先想一次屏幕 DC，不行就认下这个黑帧。
///
/// 独占全屏的 D3D 窗口就是会让 `PrintWindow`「成功返回、画面却全黑」：它的背面
/// 缓冲不在 `WM_PRINT` 问得到的地方，函数只是「画完了」。以前这里直接把黑帧交出去，
/// 上层就报「游戏窗口还没画出来？」—— 把一个「等一下就好」的解释，送给了一个
/// 必须改窗口模式才好的问题。
///
/// 屏幕 DC 只在**游戏确实在前台**时才采信（全屏时它必然在前台）：窗口模式被别的
/// 窗口盖住时，屏幕上那块是别人的像素，拿回来会变成「读不到经验那一行」，
/// 比黑帧更误导。
///
/// 代价：加载 / 过场本来就是黑的，这时会**多抓一次屏**（约 3.5 毫秒），然后两边
/// 都黑、照旧报空白。一次多余的抓屏，换来「全屏抓不到」和「还没画出来」能被分清
/// —— 采样 1~2 秒一次，3.5 毫秒不在预算的敏感区里，这个交换划算。
#[cfg(windows)]
fn grab_client_fallback(
    hwnd: isize,
    x: i32,
    y: i32,
    width: i32,
    height: i32,
    black: Frame,
) -> Option<(Frame, Method)> {
    if win::is_foreground(hwnd) {
        if let Some(screen_frame) = win::grab_screen(hwnd, x, y, width, height) {
            if !screen_frame.pixels().looks_blank() {
                log::info!(
                    "后台截屏拿到整片全黑，改用屏幕 DC 兜底成功（{}×{}，游戏在前台）",
                    width,
                    height
                );
                return Some((screen_frame, Method::Screen));
            }
        }
    }
    // 两条路都黑（或游戏不在前台）—— 那就真是黑的，原样交出去，
    // 由上层按「全屏 / 窗口」两种现场给出不同的话。
    Some((black, Method::Window))
}

/// 抓客户区里的某一块（坐标相对客户区左上角）。
///
/// 先试后台截屏；**拿到的帧是整片全黑时也再想一次屏幕 DC**（独占全屏的
/// D3D 窗口就是「成功返回但全黑」，详见 `grab_client_fallback`），
/// `PrintWindow` 彻底失败时同样退回屏幕 DC（那时要求游戏在前台）。
/// 两条路共同的目标是：别把「抓到的是别的窗口 / 一片黑」冒充成能读的画面。
pub fn grab_client(hwnd: isize, x: i32, y: i32, width: i32, height: i32) -> Option<(Frame, Method)> {
    if width <= 0 || height <= 0 {
        return None;
    }
    #[cfg(windows)]
    {
        if let Some(frame) = win::grab_window(hwnd, x, y, width, height) {
            if !frame.pixels().looks_blank() {
                return Some((frame, Method::Window));
            }
            grab_client_fallback(hwnd, x, y, width, height, frame)
        } else {
            win::grab_screen(hwnd, x, y, width, height).map(|frame| (frame, Method::Screen))
        }
    }
    #[cfg(not(windows))]
    {
        let _ = (hwnd, x, y, width, height);
        None
    }
}

/// 抖一抖窗口再抓 —— 真机自检用（最小化之后恢复要等一两帧才画得出来）。
#[cfg(test)]
pub fn settle() {
    std::thread::sleep(std::time::Duration::from_millis(250));
}

#[cfg(windows)]
mod win {
    use super::{classify_screen_mode, Frame, GameWindow, ScreenMode};
    use std::sync::Once;

    type Hwnd = isize;
    type Hdc = isize;
    type Hbitmap = isize;
    type Hgdiobj = isize;

    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    struct Rect {
        left: i32,
        top: i32,
        right: i32,
        bottom: i32,
    }

    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    struct Point {
        x: i32,
        y: i32,
    }

    /// 必须和 Win32 的 `BITMAPINFOHEADER` 逐字段一致：字段顺序或宽度错一个，
    /// `GetDIBits` 就会按错误的步长解释内存，表现是画面斜着错位（比崩溃更难查）。
    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    struct BitmapInfoHeader {
        size: u32,
        width: i32,
        height: i32,
        planes: u16,
        bit_count: u16,
        compression: u32,
        size_image: u32,
        x_pels_per_meter: i32,
        y_pels_per_meter: i32,
        clr_used: u32,
        clr_important: u32,
    }

    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    struct BitmapInfo {
        header: BitmapInfoHeader,
        colors: [u32; 3],
    }

    const SRCCOPY: u32 = 0x00CC_0020;
    const DIB_RGB_COLORS: u32 = 0;
    /// `PW_RENDERFULLCONTENT`：走 DWM 的合成结果，D3D/Unity 窗口要它才画得出内容
    const PW_RENDERFULLCONTENT: u32 = 2;
    const PER_MONITOR_AWARE_V2: isize = -4;
    const UNITY_CLASS: &str = "UnityWndClass";
    const GAME_TITLE_HINT: &str = "冒险岛";

    #[link(name = "user32")]
    extern "system" {
        fn EnumWindows(callback: unsafe extern "system" fn(Hwnd, isize) -> i32, param: isize) -> i32;
        fn IsWindow(hwnd: Hwnd) -> i32;
        fn IsWindowVisible(hwnd: Hwnd) -> i32;
        fn IsIconic(hwnd: Hwnd) -> i32;
        fn GetWindowTextLengthW(hwnd: Hwnd) -> i32;
        fn GetWindowTextW(hwnd: Hwnd, buffer: *mut u16, max: i32) -> i32;
        fn GetClassNameW(hwnd: Hwnd, buffer: *mut u16, max: i32) -> i32;
        fn GetForegroundWindow() -> Hwnd;
        fn GetWindowLongW(hwnd: Hwnd, index: i32) -> i32;
        fn MonitorFromWindow(hwnd: Hwnd, flags: u32) -> isize;
        fn GetMonitorInfoW(monitor: isize, info: *mut MonitorInfo) -> i32;
        fn GetClientRect(hwnd: Hwnd, rect: *mut Rect) -> i32;
        fn GetWindowRect(hwnd: Hwnd, rect: *mut Rect) -> i32;
        fn ClientToScreen(hwnd: Hwnd, point: *mut Point) -> i32;
        fn SetForegroundWindow(hwnd: Hwnd) -> i32;
        fn ShowWindow(hwnd: Hwnd, command: i32) -> i32;
        pub(super) fn GetDC(hwnd: Hwnd) -> Hdc;
        fn ReleaseDC(hwnd: Hwnd, hdc: Hdc) -> i32;
        fn PrintWindow(hwnd: Hwnd, hdc: Hdc, flags: u32) -> i32;
        fn SetProcessDPIAware() -> i32;
        fn SetProcessDpiAwarenessContext(value: isize) -> i32;
    }

    #[link(name = "gdi32")]
    extern "system" {
        pub(super) fn CreateCompatibleDC(hdc: Hdc) -> Hdc;
        pub(super) fn CreateCompatibleBitmap(hdc: Hdc, width: i32, height: i32) -> Hbitmap;
        fn SelectObject(hdc: Hdc, object: Hgdiobj) -> Hgdiobj;
        fn BitBlt(
            dst: Hdc,
            x: i32,
            y: i32,
            width: i32,
            height: i32,
            src: Hdc,
            src_x: i32,
            src_y: i32,
            rop: u32,
        ) -> i32;
        fn GetDIBits(
            hdc: Hdc,
            bitmap: Hbitmap,
            start: u32,
            lines: u32,
            bits: *mut u8,
            info: *mut BitmapInfo,
            usage: u32,
        ) -> i32;
        pub(super) fn DeleteObject(object: Hgdiobj) -> i32;
        fn DeleteDC(hdc: Hdc) -> i32;
    }

    /// GDI 资源的 RAII 包装：早退时也要还回去，泄漏会拖垮整个桌面。
    pub(super) struct MemDc(pub(super) Hdc);
    impl Drop for MemDc {
        fn drop(&mut self) {
            if self.0 != 0 {
                unsafe {
                    DeleteDC(self.0);
                }
            }
        }
    }
    pub(super) struct Bitmap(pub(super) Hbitmap);
    impl Drop for Bitmap {
        fn drop(&mut self) {
            if self.0 != 0 {
                unsafe {
                    DeleteObject(self.0);
                }
            }
        }
    }
    pub(super) struct ScreenDc(pub(super) Hdc);
    impl Drop for ScreenDc {
        fn drop(&mut self) {
            if self.0 != 0 {
                unsafe {
                    ReleaseDC(0, self.0);
                }
            }
        }
    }

    /// 位图选入 DC 的 RAII 守卫：维持选入生命周期，并在析构时反选（Deselect）。
    ///
    /// # 为什么必须先反选
    ///
    /// 这是 Windows GDI 极为隐蔽的一个底层约束：处于「被选入 DC」状态的位图，
    /// 调用 `DeleteObject` 会直接失败并返回 0（FALSE），位图占用的内存与句柄完全无法释放。
    /// 旧实现丢弃了 `SelectObject` 返回的旧位图句柄，Bitmap 析构时位图仍被 DC 引用着；
    /// 而 `MemDc` 的 `DeleteDC` 只释放 DC 自身结构，绝不级联释放被选入的外部位图。
    ///
    /// 结果就是每次抓屏泄漏整窗位图（1080p 下约 8.3MB）与切片位图，挂机数小时
    /// 就会吃满单进程 10,000 个 GDI 句柄上限，导致抓屏甚至系统界面绘制彻底崩溃。
    ///
    /// 该守卫保存 `SelectObject` 换出的原默认对象；析构时**先反选**（把原对象换回去），
    /// 使位图脱离 DC，随后 `Bitmap::drop` 才能真正删除位图，最后 `MemDc::drop` 删除 DC。
    pub(super) struct SelectedBitmap<'a> {
        dc: &'a MemDc,
        _bmp: &'a Bitmap,
        old: Hgdiobj,
    }

    impl Drop for SelectedBitmap<'_> {
        fn drop(&mut self) {
            unsafe {
                SelectObject(self.dc.0, self.old);
            }
        }
    }

    impl MemDc {
        pub(super) fn select<'a>(&'a self, bmp: &'a Bitmap) -> Option<SelectedBitmap<'a>> {
            let old = unsafe { SelectObject(self.0, bmp.0) };
            if old == 0 || old == -1 {
                return None;
            }
            Some(SelectedBitmap {
                dc: self,
                _bmp: bmp,
                old,
            })
        }
    }

    pub fn ensure_dpi_aware() {
        static ONCE: Once = Once::new();
        ONCE.call_once(|| unsafe {
            // 先试 per-monitor v2（更准），失败再退回老的 SetProcessDPIAware。
            // 两个都失败通常意味着进程已经声明过 —— 那正是我们想要的状态。
            if SetProcessDpiAwarenessContext(PER_MONITOR_AWARE_V2) == 0 {
                let _ = SetProcessDPIAware();
            }
            log::info!("屏幕抓取：已请求按物理像素工作（DPI 感知）");
        });
    }

    fn window_title(hwnd: Hwnd) -> String {
        let len = unsafe { GetWindowTextLengthW(hwnd) };
        if len <= 0 {
            return String::new();
        }
        let mut buffer = vec![0u16; len as usize + 1];
        let written = unsafe { GetWindowTextW(hwnd, buffer.as_mut_ptr(), len + 1) };
        String::from_utf16_lossy(&buffer[..written.max(0) as usize])
    }

    pub(super) fn window_class(hwnd: Hwnd) -> String {
        let mut buffer = vec![0u16; 256];
        let written = unsafe { GetClassNameW(hwnd, buffer.as_mut_ptr(), 256) };
        String::from_utf16_lossy(&buffer[..written.max(0) as usize])
    }

    extern "system" fn collect(hwnd: Hwnd, param: isize) -> i32 {
        // SAFETY: 调用方保证 param 指向一个在本函数存活期内有效的 Vec
        let out = unsafe { &mut *(param as *mut Vec<(Hwnd, String)>) };
        if unsafe { IsWindowVisible(hwnd) } != 0 && window_class(hwnd) == UNITY_CLASS {
            out.push((hwnd, window_title(hwnd)));
        }
        1
    }

    pub fn find_game_window() -> Option<GameWindow> {
        let mut candidates: Vec<(Hwnd, String)> = Vec::new();
        unsafe {
            EnumWindows(collect, &mut candidates as *mut _ as isize);
        }
        // 最小化的窗口**也要留着**：留着才能说清「游戏最小化了」，
        // 而不是报成「没找到游戏窗口」——用户会以为是游戏没开。
        candidates
            .into_iter()
            .max_by_key(|(hwnd, title)| {
                let hinted = if title.contains(GAME_TITLE_HINT) { 1 } else { 0 };
                let area = client_size(*hwnd)
                    .map(|(w, h)| w as i64 * h as i64)
                    .unwrap_or(0);
                (hinted, area)
            })
            .map(|(hwnd, title)| GameWindow { hwnd, title })
    }

    pub fn is_window(hwnd: Hwnd) -> bool {
        unsafe { IsWindow(hwnd) != 0 }
    }

    pub fn is_minimized(hwnd: Hwnd) -> bool {
        unsafe { IsIconic(hwnd) != 0 }
    }

    pub fn is_foreground(hwnd: Hwnd) -> bool {
        unsafe { GetForegroundWindow() == hwnd }
    }

    /// `MONITOR_DEFAULTTONEAREST`：拿不到精确的显示器时就近取一个，
    /// 总比把全屏判成「不是全屏」好（这个判断只用来选文案，取错了也只影响一句话）。
    const MONITOR_DEFAULTTONEAREST: u32 = 2;

    /// `MONITORINFO`：只取到 `rc_monitor` 就够（我们要的是屏幕矩形）。
    /// 字段顺序与大小必须和 Win32 一致，否则 `GetMonitorInfoW` 会直接写失败。
    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    struct MonitorInfo {
        size: u32,
        rc_monitor: Rect,
        rc_work: Rect,
        flags: u32,
    }

    /// 游戏窗口在屏幕上的形态。
    ///
    /// 两段判据：窗口矩形 vs 显示器矩形（容差 2 像素，抽在 `classify_screen_mode`
    /// 里，为的是能在单测里拿任意分辨率验），再看样式里有没有可调边框。
    /// 注意这里**问不出「独占全屏」**：它和无边框窗口在 Win32 眼里长得一模一样，
    /// 要分开只能看两路抓图对不对得上（见 `preview()`）。
    pub fn screen_mode(hwnd: Hwnd) -> ScreenMode {
        let mut window = Rect::default();
        if unsafe { GetWindowRect(hwnd, &mut window) } == 0 {
            return ScreenMode::Windowed;
        }
        let monitor = unsafe { MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST) };
        if monitor == 0 {
            return ScreenMode::Windowed;
        }
        let mut info = MonitorInfo {
            size: std::mem::size_of::<MonitorInfo>() as u32,
            ..Default::default()
        };
        if unsafe { GetMonitorInfoW(monitor, &mut info) } == 0 {
            return ScreenMode::Windowed;
        }
        // `WS_CAPTION`（标题栏+边框）与 `WS_THICKFRAME`（可拖拽调大小的框）——
        // 无边框窗口两者都没有，最大化窗口两者都有
        let style = unsafe { GetWindowLongW(hwnd, GWL_STYLE) } as u32;
        let has_frame = style & (WS_CAPTION | WS_THICKFRAME) != 0;
        classify_screen_mode(
            (window.left, window.top, window.right, window.bottom),
            (
                info.rc_monitor.left,
                info.rc_monitor.top,
                info.rc_monitor.right,
                info.rc_monitor.bottom,
            ),
            has_frame,
            2,
        )
    }

    const GWL_STYLE: i32 = -16;
    const WS_CAPTION: u32 = 0x00C0_0000;
    const WS_THICKFRAME: u32 = 0x0004_0000;

    /// 客户区尺寸与它在窗口坐标里的偏移。
    fn geometry(hwnd: Hwnd) -> Option<(i32, i32, i32, i32)> {
        let mut client = Rect::default();
        if unsafe { GetClientRect(hwnd, &mut client) } == 0 {
            return None;
        }
        let mut window = Rect::default();
        if unsafe { GetWindowRect(hwnd, &mut window) } == 0 {
            return None;
        }
        let mut origin = Point { x: 0, y: 0 };
        if unsafe { ClientToScreen(hwnd, &mut origin) } == 0 {
            return None;
        }
        Some((
            client.right - client.left,
            client.bottom - client.top,
            origin.x - window.left,
            origin.y - window.top,
        ))
    }

    pub fn client_size(hwnd: Hwnd) -> Option<(i32, i32)> {
        let mut client = Rect::default();
        if unsafe { GetClientRect(hwnd, &mut client) } == 0 {
            return None;
        }
        Some((client.right - client.left, client.bottom - client.top))
    }

    /// 客户区在**屏幕物理坐标**里的位置和尺寸 `(x, y, w, h)`。
    ///
    /// 校准蒙版要正好盖在游戏客户区上，靠的就是这四个数。
    /// `ClientToScreen(0,0)` 给的是客户区左上角在屏幕上的位置（进程已声明 DPI aware，
    /// 所以拿到的就是物理像素，和 PrintWindow 的坐标同一套）。
    pub fn client_screen_rect(hwnd: Hwnd) -> Option<(i32, i32, i32, i32)> {
        let (width, height) = client_size(hwnd)?;
        let mut origin = Point { x: 0, y: 0 };
        if unsafe { ClientToScreen(hwnd, &mut origin) } == 0 {
            return None;
        }
        Some((origin.x, origin.y, width, height))
    }

    /// 把游戏窗口抬到最前面（用户主动点「手动校准」时的预期行为）。
    ///
    /// 只在校准这一条用户显式发起的路径上调用 —— 平时绝不抢焦点。
    pub fn bring_to_foreground(hwnd: Hwnd) {
        unsafe {
            if IsIconic(hwnd) != 0 {
                ShowWindow(hwnd, 9); // SW_RESTORE
            }
            SetForegroundWindow(hwnd);
        }
    }

    /// 不抢焦点地恢复（开发探针用）：`SW_SHOWNOACTIVATE` 只把窗口显示出来，
    /// 不激活、不抢焦点，`PrintWindow` 就能拿到画面了。
    #[cfg(test)]
    pub fn restore_without_activating(hwnd: Hwnd) {
        unsafe {
            if IsIconic(hwnd) != 0 {
                ShowWindow(hwnd, 4); // SW_SHOWNOACTIVATE
            }
        }
    }

    fn read_bits(dc: Hdc, bitmap: Hbitmap, width: i32, height: i32) -> Option<Vec<u8>> {
        let mut info = BitmapInfo {
            header: BitmapInfoHeader {
                size: std::mem::size_of::<BitmapInfoHeader>() as u32,
                width,
                // 负高度 = 自上而下的行序，省得自己翻转
                height: -height,
                planes: 1,
                bit_count: 32,
                compression: 0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut buffer = vec![0u8; (width as usize) * (height as usize) * 4];
        let lines = unsafe {
            GetDIBits(
                dc,
                bitmap,
                0,
                height as u32,
                buffer.as_mut_ptr(),
                &mut info,
                DIB_RGB_COLORS,
            )
        };
        if lines == 0 {
            return None;
        }
        Some(buffer)
    }

    /// 后台截屏：`PrintWindow` 渲染**整个窗口**，再从中裁出客户区里的一块。
    ///
    /// 注意必须先申请「整个窗口大小」的位图：`PrintWindow` 永远从窗口左上角
    /// 开始画（含标题栏），按客户区尺寸申请就会拿到标题栏的内容。
    pub fn grab_window(hwnd: Hwnd, x: i32, y: i32, width: i32, height: i32) -> Option<Frame> {
        let (client_w, client_h, offset_x, offset_y) = geometry(hwnd)?;
        if x < 0 || y < 0 || x + width > client_w || y + height > client_h {
            return None;
        }
        let mut window = Rect::default();
        if unsafe { GetWindowRect(hwnd, &mut window) } == 0 {
            return None;
        }
        let (window_w, window_h) = (window.right - window.left, window.bottom - window.top);
        if window_w <= 0 || window_h <= 0 {
            return None;
        }

        let screen = ScreenDc(unsafe { GetDC(0) });
        if screen.0 == 0 {
            return None;
        }
        // 整窗口那一层
        let full_dc = MemDc(unsafe { CreateCompatibleDC(screen.0) });
        let full_bmp = Bitmap(unsafe { CreateCompatibleBitmap(screen.0, window_w, window_h) });
        if full_dc.0 == 0 || full_bmp.0 == 0 {
            return None;
        }
        let _full_sel = full_dc.select(&full_bmp)?;

        // 先试 PW_RENDERFULLCONTENT（D3D 窗口靠它才有内容），失败再试老旗标
        let rendered = unsafe { PrintWindow(hwnd, full_dc.0, PW_RENDERFULLCONTENT) } != 0
            || unsafe { PrintWindow(hwnd, full_dc.0, 0) } != 0;
        if !rendered {
            return None;
        }

        // 目标那一小块
        let strip_dc = MemDc(unsafe { CreateCompatibleDC(screen.0) });
        let strip_bmp = Bitmap(unsafe { CreateCompatibleBitmap(screen.0, width, height) });
        if strip_dc.0 == 0 || strip_bmp.0 == 0 {
            return None;
        }
        let _strip_sel = strip_dc.select(&strip_bmp)?;
        let ok = unsafe {
            BitBlt(
                strip_dc.0,
                0,
                0,
                width,
                height,
                full_dc.0,
                offset_x + x,
                offset_y + y,
                SRCCOPY,
            )
        };
        if ok == 0 {
            return None;
        }
        let bgra = read_bits(strip_dc.0, strip_bmp.0, width, height)?;
        Some(Frame {
            width: width as usize,
            height: height as usize,
            bgra,
        })
    }

    /// 兜底：从屏幕 DC 抓。**要求游戏就在前台**，被挡住会抓到别人的像素。
    pub fn grab_screen(hwnd: Hwnd, x: i32, y: i32, width: i32, height: i32) -> Option<Frame> {
        let (client_w, client_h, _, _) = geometry(hwnd)?;
        if x < 0 || y < 0 || x + width > client_w || y + height > client_h {
            return None;
        }
        let mut origin = Point { x: 0, y: 0 };
        if unsafe { ClientToScreen(hwnd, &mut origin) } == 0 {
            return None;
        }
        let screen = ScreenDc(unsafe { GetDC(0) });
        if screen.0 == 0 {
            return None;
        }
        let dc = MemDc(unsafe { CreateCompatibleDC(screen.0) });
        let bmp = Bitmap(unsafe { CreateCompatibleBitmap(screen.0, width, height) });
        if dc.0 == 0 || bmp.0 == 0 {
            return None;
        }
        let _sel = dc.select(&bmp)?;
        let ok = unsafe {
            BitBlt(
                dc.0,
                0,
                0,
                width,
                height,
                screen.0,
                origin.x + x,
                origin.y + y,
                SRCCOPY,
            )
        };
        if ok == 0 {
            return None;
        }
        let bgra = read_bits(dc.0, bmp.0, width, height)?;
        Some(Frame {
            width: width as usize,
            height: height as usize,
            bgra,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 真机自检：抓游戏窗口最底下那一条，并且**必须能读出经验**。
    ///
    /// 默认跳过（需要游戏开着）。它证明的是这个模块唯一重要的一件事：
    /// 在**不把游戏切到前台**的情况下也能抓到可读的画面 ——
    /// 这正是旧实现（屏幕 DC）做不到、于是「经常识别不到」的地方。
    ///
    /// ```text
    /// cd src-tauri && cargo test --lib -- --ignored capture_reads_without_foreground --nocapture
    /// ```
    #[test]
    #[ignore = "需要游戏正在运行"]
    fn capture_reads_without_foreground() {
        ensure_dpi_aware();
        let Some(window) = find_game_window() else {
            panic!("没找到游戏窗口（游戏没开？）");
        };
        println!("游戏窗口：{}（句柄 {}）", window.title, window.hwnd);
        let (w, h) = client_size(window.hwnd).expect("拿不到客户区尺寸");
        println!("客户区 {w}×{h}   最小化={}", is_minimized(window.hwnd));

        let font = crate::exp::font::Font::builtin();
        for round in 0..5 {
            let Some((frame, method)) = grab_strip(window.hwnd) else {
                panic!("第 {round} 次抓取失败");
            };
            let pixels = frame.pixels();
            print!(
                "第 {round} 次：{method:?}  {}×{}  空白={}  墨点=",
                frame.width,
                frame.height,
                pixels.looks_blank()
            );
            let ink = (0..frame.height)
                .flat_map(|y| (0..frame.width).map(move |x| (x, y)))
                .filter(|(x, y)| pixels.luma(*x, *y) > crate::exp::font::INK_THRESHOLD)
                .count();
            println!("{:.1}%", ink as f64 / (frame.width * frame.height) as f64 * 100.0);
            assert!(!pixels.looks_blank(), "抓到的是空白画面");
            match crate::exp::font::read_line(&pixels, &font) {
                Some(reading) => println!(
                    "           读出：{} → {} / {}%（原始串 {}）",
                    reading.raw, reading.exp, reading.percent, reading.raw
                ),
                None => println!("           这一帧读不出经验那一行"),
            }
            settle();
        }
    }

    /// 客户区尺寸是要来算裁剪偏移的，拿不到就该老实返回 `None`。
    #[test]
    fn a_dead_handle_yields_no_geometry() {
        assert_eq!(client_size(0), None);
        assert!(!is_window(0));
        assert!(grab_client(0, 0, 0, 10, 10).is_none());
        assert!(grab_strip(0).is_none());
    }

    /// 底部条跟着客户区高度走（假设 B）：**下限 44、比例 10%、上限 144**。
    ///
    /// 这三个数就是「768 还能不能读到经验」的答案：
    /// 768 → 77 行（以前写死 44，UI 一缩放那一行就跑出去了）；
    /// 1080 → 108 行（实测能读到的 44 行被完整包含在内）；
    /// 4K → 144 行封顶（按比例本该 216，但按 1080 基准只需 88 行）。
    #[test]
    fn strip_height_follows_the_client_height_with_bounds() {
        // 768p：比例生效，且必然包含旧的 44 行
        assert_eq!(strip_height(768), 77);
        assert!(strip_height(768) >= 44);
        // 1080p：比改之前更宽裕，但不多花冤枉钱
        assert_eq!(strip_height(1080), 108);
        // 1440p：正好顶到上限
        assert_eq!(strip_height(1440), 144);
        // 4K：封顶在 144（再高只是把聊天框扫进来）
        assert_eq!(strip_height(2160), 144);
        // 小分辨率：比例算出来比下限还小，下限托底 —— 绝不比写死 44 时更差
        assert_eq!(strip_height(600), 60);
        assert_eq!(strip_height(400), 44);
        // 窗口被拖成一条：不能去抓不存在的行
        assert_eq!(strip_height(30), 30);
        // 拿不到高度 / 高度为 0：给 0，让上层自己返回 None
        assert_eq!(strip_height(0), 0);
        assert_eq!(strip_height(-1), 0);
    }

    /// 全屏判据（假设 A 的文案分流靠它）：拿窗口矩形比显示器矩形，
    /// 容差 2 像素。判据本身必须能在单测里拿任意分辨率验，
    /// 而不是只能在真机上看 —— 说错一次就把用户引向错的方向。
    #[test]
    fn fullscreen_is_decided_by_the_monitor_rect() {
        // 1366×768 的无边框 / 独占全屏：逐像素相等
        assert!(rect_is_fullscreen(
            (0, 0, 1366, 768),
            (0, 0, 1366, 768),
            2
        ));
        // DPI 取整差 1~2 像素：仍算全屏
        assert!(rect_is_fullscreen((1, 2, 1364, 766), (0, 0, 1366, 768), 2));
        // 副显示器上的全屏（坐标从 1366 开始）：原点也参与比较
        assert!(rect_is_fullscreen(
            (1366, 0, 2732, 768),
            (1366, 0, 2732, 768),
            2
        ));
        // 普通窗口：不是全屏（文案不能把「窗口还没画出来」说成「全屏抓不到」）
        assert!(!rect_is_fullscreen(
            (100, 100, 1100, 700),
            (0, 0, 1366, 768),
            2
        ));
        // 最大化窗口：四条边往外溢几像素，**不算全屏** —— 它不是全屏
        assert!(!rect_is_fullscreen(
            (-4, -4, 1370, 772),
            (0, 0, 1366, 768),
            2
        ));
        // 比屏幕还大的窗口（超出显示器范围）：也不算
        assert!(!rect_is_fullscreen(
            (0, 0, 1920, 1080),
            (0, 0, 1366, 768),
            2
        ));
    }

    /// 造一张**亮度就是给定值**的图（r=g=b=v 时 `luma` 恰好等于 v）。
    fn synthetic(width: usize, height: usize, value: impl Fn(usize, usize) -> u8) -> Vec<u8> {
        let mut bgra = vec![0u8; width * height * 4];
        for y in 0..height {
            for x in 0..width {
                let offset = (y * width + x) * 4;
                let v = value(x, y);
                bgra[offset] = v;
                bgra[offset + 1] = v;
                bgra[offset + 2] = v;
                bgra[offset + 3] = 255;
            }
        }
        bgra
    }

    /// 直方图三个数 + 亮像素占比：中位数用来看「这片图到底是什么底色」，
    /// 平均值在大片纯色面前会骗人（错拍一张纯黑 / 纯白，平均值也理直气壮）。
    #[test]
    fn luma_stats_summarizes_the_frame() {
        // 100 个像素：前 60 个是 10（暗底），后 40 个是 200（亮字）
        let width = 10usize;
        let height = 10usize;
        let bgra = synthetic(width, height, |x, y| {
            if y * width + x < 60 {
                10
            } else {
                200
            }
        });
        let pixels = crate::exp::font::Pixels {
            width,
            height,
            bgra: &bgra,
        };
        let stats = luma_stats(&pixels);
        assert_eq!(stats.min, 10);
        assert_eq!(stats.max, 200);
        assert_eq!(stats.median, 10, "累计过半的那个桶是 10");
        assert!(
            (stats.bright_ratio - 0.4).abs() < 1e-9,
            "{}",
            stats.bright_ratio
        );

        // 空帧不越界（拿不到尺寸时会传进来）
        let empty = crate::exp::font::Pixels {
            width: 0,
            height: 0,
            bgra: &[],
        };
        assert_eq!(luma_stats(&empty).max, 0);
    }

    /// 降采样取**块内最大亮度** —— 这是整张粗图的立身之本：
    /// 经验那一行只有十几像素高，取平均会被抹平到阈值以下，图上就“什么都没有”。
    #[test]
    fn coarse_grid_keeps_thin_bright_lines_that_averaging_would_erase() {
        let width = 16usize;
        let height = 32usize;
        // y=8 那一整行是亮的（比 11 像素高的经验行还苛刻），其余全黑
        let bgra = synthetic(width, height, |_, y| if y == 8 { 255 } else { 0 });
        let pixels = crate::exp::font::Pixels {
            width,
            height,
            bgra: &bgra,
        };
        // 4 列 × 2 行 → 每格 8×16 像素
        let grid = coarse_grid(&pixels, 4, 2);
        assert_eq!(grid.len(), 2);
        assert!(grid.iter().all(|row| row.len() == 4));
        // 亮线所在的那一行：每一格都该是 255
        assert!(grid[0].iter().all(|&value| value == 255), "{grid:?}");
        // 另一行全黑
        assert!(grid[1].iter().all(|&value| value == 0), "{grid:?}");

        // 同一块取平均会掉到阈值以下 —— 这就是不选平均的理由（写成断言，
        // 免得下一个人“顺手改成平均”还以为只是换了个口味）
        let block: u32 = (0..16).map(|y| if y == 8 { 255u32 } else { 0 }).sum();
        let mean = (block / 16) as u8;
        assert!(
            mean < crate::exp::font::INK_THRESHOLD,
            "平均只有 {mean}，会被当成黑底丢掉"
        );
    }

    /// 尺寸不合也不许越界 / 溢出：诊断页会拿任意分辨率的图来降采样。
    #[test]
    fn coarse_grid_always_fits_the_requested_shape() {
        let bgra = synthetic(3, 3, |_, _| 42);
        let pixels = crate::exp::font::Pixels {
            width: 3,
            height: 3,
            bgra: &bgra,
        };
        let grid = coarse_grid(&pixels, 160, 45);
        assert_eq!(grid.len(), 45);
        assert!(grid.iter().all(|row| row.len() == 160));
        assert!(grid.iter().flatten().all(|&value| value == 42));

        // 0 列 / 0 行 / 空帧：返回空图而不是 panic
        assert!(coarse_grid(&pixels, 0, 45).is_empty());
        assert!(coarse_grid(&pixels, 160, 0).is_empty());
        let empty = crate::exp::font::Pixels {
            width: 0,
            height: 0,
            bgra: &[],
        };
        assert!(coarse_grid(&empty, 160, 45).is_empty());
    }

    /// 两路抓图的对比：结构相同就该判「同一份」，真不同就该判「对不上」。
    #[test]
    fn grid_similarity_tells_the_same_picture_from_a_different_one() {
        let a = vec![vec![10u8, 20, 30, 40], vec![50, 60, 70, 80]];
        // 逐格一样
        assert_eq!(grid_similarity(&a, &a, 32), 1.0);
        // 游戏自己在动：差一点仍在容差内
        let mut animated = a.clone();
        animated[0][0] = 10 + 32;
        assert_eq!(grid_similarity(&a, &animated, 32), 1.0);
        // 8 格里有 1 格差很多（另一张图）：7/8 = 0.875
        let mut other = a.clone();
        other[1][3] = 240;
        assert!((grid_similarity(&a, &other, 32) - 0.875).abs() < 1e-9);
        // 尺寸对不上 / 空图：直接 0.0，不给“碰巧很像”的假结论
        assert_eq!(grid_similarity(&a, &vec![vec![10u8]], 32), 0.0);
        assert_eq!(grid_similarity(&[], &[], 32), 0.0);
    }

    /// 形态分类（文案分流靠它）：**先看占没占满，再看有没有边框**。
    /// 用户报的 2560×1440「无边框全屏」就是第一档 —— 占满 + 无框。
    #[test]
    fn screen_mode_is_decided_by_size_then_frame() {
        // 2560×1440 无边框全屏：占满、样式里没有可调边框
        assert_eq!(
            classify_screen_mode((0, 0, 2560, 1440), (0, 0, 2560, 1440), false, 2),
            ScreenMode::Borderless
        );
        // 占满但带边框（更像最大化窗口）——不是游戏里那个全屏开关
        assert_eq!(
            classify_screen_mode((0, 0, 1366, 768), (0, 0, 1366, 768), true, 2),
            ScreenMode::Framed
        );
        // 没占满：不管带不带框都是普通窗口
        assert_eq!(
            classify_screen_mode((100, 100, 1100, 700), (0, 0, 1366, 768), true, 2),
            ScreenMode::Windowed
        );
        assert_eq!(
            classify_screen_mode((100, 100, 1100, 700), (0, 0, 1366, 768), false, 2),
            ScreenMode::Windowed
        );
        // 最大化窗口四条边往外溢几像素：尺寸先判，直接归窗口
        assert_eq!(
            classify_screen_mode((-4, -4, 1370, 772), (0, 0, 1366, 768), true, 2),
            ScreenMode::Windowed
        );
        // DPI 取整差 1 像素：仍在容差内，算占满
        assert_eq!(
            classify_screen_mode((1, 2, 1364, 766), (0, 0, 1366, 768), false, 2),
            ScreenMode::Borderless
        );

        // 短名是给判据行和文案用的，三档必须互不相同
        assert_ne!(ScreenMode::Windowed.label(), ScreenMode::Borderless.label());
        assert_ne!(ScreenMode::Framed.label(), ScreenMode::Borderless.label());
    }
}
