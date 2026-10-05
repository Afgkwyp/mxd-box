//! 一次完整的读数：**找对地方 → 切字比对 → 经验表自检**。
//!
//! # 「在哪儿读」这件事由三段组成
//!
//! 1. **手动框先读**（用户明确指定的，比例坐标换算成像素，见 `exp::region`）；
//! 2. **HUD 锚点（主路径）**：血条边框 + 标签模板定位 HUD、量出缩放（见 `exp::hud`），
//!    每帧只抓经验那一小条，按缩放还原回原生像素认字（见 `exp::hudread`），
//!    再和经验条的填充比例互相印证。全屏拉伸、1440p/4K、显示缩放都不用校准；
//! 3. 旧版本学下来的**自动框**；
//! 4. 都不行：抓**整幅客户区**，用 [`font::read_frame`] 的多尺度候选管线 +
//!    PP-OCR 兜底找经验行，解析结果同样必须过 [`table::validate`]。
//!
//! 整幅搜索的成功读数会自动学成一个 `auto` 校准档；连续两帧位置一致才落库
//! （HUD 路径不学：锚点每次启动都会重新定位）。
//! **手动档永不被自动覆盖**（见 [`ExpReader::manual_lost`]）。
//!
//! # 两条硬的规矩（从上一版继承，未变）
//!
//! 1. **读不到就明说读不到。** 抓屏成功不等于读到了经验 ——
//!    画面里没有那一行（不在角色里、加载中）时要说清楚，绝不拿上一次的值凑合。
//! 2. **绝不给出半个数字。** 经验那一行里每一位都必须是认识的字形；
//!    有一位对不上，整帧作废。少读一位的 `2302` 看起来像个真数
//!    （它甚至能通过一部分校验），比明说「不知道」危险得多。

use crate::exp::capture::{self, GameWindow, Method, ScreenMode};
use crate::exp::font::{self, Font, Pixels};
use crate::exp::hud::{self, HudLayout};
use crate::exp::hudread;
use crate::exp::region::{self, NormRect, PixelRect, RegionProfile, RegionSource, RegionStore};
use crate::exp::table::{self, Verdict};
use serde::Serialize;
use std::sync::Arc;

/// 一次成功的读数。
#[derive(Debug, Clone, PartialEq)]
pub struct Sample {
    pub exp: u64,
    pub percent: f64,
    /// 经验表算出来的等级。表定不出来时为 `None`（照常统计，只是不知道等级）。
    pub level: Option<u32>,
    /// 原始字形串（诊断用）
    pub raw: String,
    /// 这次抓屏走的是哪条路（后台截屏 / 兜底屏幕 DC）
    pub method: Method,
    /// 这一帧读的是哪个框（校准/自动定位），以及框从哪来。
    pub region: Option<RegionHit>,
    /// 从**校准过的等级区域**直接读到的等级（经验表定不出等级时的兜底）。
    pub screen_level: Option<u32>,
}

/// 本帧实际命中的框（给状态栏 / 日志用）。
#[derive(Debug, Clone, PartialEq)]
pub struct RegionHit {
    pub rect: NormRect,
    pub source: RegionSource,
    /// 实测字形高度（客户区物理像素）
    pub text_height: usize,
}

/// 校准测试 / 自动识别的结果（手动框选的「测试读数」与「自动识别」共用）。
#[derive(Debug, Clone, Serialize)]
pub struct CalibrationTest {
    pub ok: bool,
    /// 给界面的一句话（成功是读数，失败是原因与怎么办）
    pub message: String,
    pub raw: Option<String>,
    pub exp: Option<u64>,
    pub percent: Option<f64>,
    pub level: Option<u32>,
    pub text_height: Option<u32>,
    /// 成功保存 / 学到的框
    pub profile: Option<RegionProfile>,
}

impl CalibrationTest {
    fn failed(message: impl Into<String>) -> Self {
        Self {
            ok: false,
            message: message.into(),
            raw: None,
            exp: None,
            percent: None,
            level: None,
            text_height: None,
            profile: None,
        }
    }

    fn from_reading(reading: &font::Reading, profile: Option<RegionProfile>) -> Self {
        let level = match table::validate(reading.exp, reading.percent) {
            Verdict::Known(level) => Some(level),
            _ => None,
        };
        Self {
            ok: true,
            message: format!(
                "认出来了：Lv.{} · {}({:.2}%)",
                level
                    .map(|level| level.to_string())
                    .unwrap_or_else(|| "—".to_string()),
                reading.exp,
                reading.percent
            ),
            raw: Some(reading.raw.clone()),
            exp: Some(reading.exp),
            percent: Some(reading.percent),
            level,
            text_height: Some(reading.text_height as u32),
            profile,
        }
    }

    /// 地图名 / 等级这类「只认一段文字」的测试结果。
    fn from_region_text(
        raw: Option<String>,
        message: String,
        level: Option<u32>,
        profile: RegionProfile,
    ) -> Self {
        Self {
            ok: true,
            message,
            raw,
            exp: None,
            percent: None,
            level,
            text_height: profile.text_height,
            profile: Some(profile),
        }
    }
}

/// 读不到的原因 —— 每一条都要能原话显示给用户。
#[derive(Debug, Clone, PartialEq)]
pub enum ReadFailure {
    /// 没找到游戏窗口（游戏没开）
    NoWindow,
    /// 窗口最小化了：最小化之后游戏不再渲染，抓不到新画面
    Minimized,
    /// 抓屏本身失败
    CaptureFailed(String),
    /// 抓到了，但是一张空白图（全黑或全白）。
    ///
    /// `mode` 是**现场**：窗口形态不同，该做的事完全不同 —— 普通窗口黑是
    ///「还没画出来」，等一两帧就好；只有占满屏幕且无边框那一档，才需要考虑
    ///「改成窗口模式」。说成一句话就会把人引错方向。
    BlankFrame { mode: ScreenMode },
    /// 抓到了画面，但里面没有经验那一行 —— 带着「我们到底看了哪儿」的现场。
    ///
    /// 客户区尺寸 + 试过哪些路（校准框 / 整幅搜索 / OCR）：
    /// 少了这些，用户报「明明站在角色里却说读不到」时我们只能猜。
    NoFieldAt { client: String, detail: String },
    /// 字都认出来了，可是和经验表对不上 —— 这一帧读错了
    Contradiction { raw: String, exp: u64, percent: f64 },
}

impl ReadFailure {
    /// 给界面用的一句话（只留「发生了什么 + 怎么办」）。
    pub fn message(&self) -> String {
        match self {
            ReadFailure::NoWindow => "没找到游戏窗口（游戏没开？）".to_string(),
            ReadFailure::Minimized => "游戏窗口最小化了，看不到经验".to_string(),
            ReadFailure::CaptureFailed(reason) => format!("抓不到游戏窗口的画面：{reason}"),
            ReadFailure::BlankFrame { mode } => match mode {
                // 占满屏幕且无边框：只有这一档才给「改窗口模式」这个动作 ——
                // 普通窗口黑是「还没画出来」，叫人去改模式只会白折腾
                ScreenMode::Borderless => {
                    "窗口铺满屏幕、没有边框（游戏里的「全屏」多半就是这一档），这一帧却是全黑：\
                     如果它是**独占全屏**，画面不经过桌面合成、两条截屏路都可能拿不到，\
                     先切成「窗口」模式试试；如果它本来就只是无边框窗口，那就是游戏自己\
                     在黑屏（加载 / 过场），等一两帧再看"
                        .to_string()
                }
                ScreenMode::Framed => {
                    "抓到的画面是整片空白的（这扇窗口带边框、只是占满了屏幕，更像最大化窗口；\
                     游戏窗口还没画出来？刚启动 / 最小化恢复后要等一两帧）"
                        .to_string()
                }
                ScreenMode::Windowed => {
                    "抓到的画面是整片空白的（游戏窗口还没画出来？刚启动 / 最小化恢复后\
                     要等一两帧）"
                        .to_string()
                }
            },
            ReadFailure::NoFieldAt { client, detail } => format!(
                "画面上没有经验那一行（搜过整幅客户区 {client}）{detail}：\
                 常见是不在角色里（登录 / 选人 / 加载中）或被游戏自己的面板盖住了那一行；\
                 一直这样可以在主窗口点「校准位置」自动识别或手动框选"
            ),
            ReadFailure::Contradiction { .. } => {
                "读数和经验表对不上，这一帧不算数".to_string()
            }
        }
    }

    pub fn code(&self) -> &'static str {
        match self {
            ReadFailure::NoWindow => "no_window",
            ReadFailure::Minimized => "minimized",
            ReadFailure::CaptureFailed(_) => "capture_failed",
            ReadFailure::BlankFrame { .. } => "blank_frame",
            ReadFailure::NoFieldAt { .. } => "no_field",
            ReadFailure::Contradiction { .. } => "contradiction",
        }
    }

    /// 这一帧是不是「读到了东西但不可信」（要显示原始串给用户判断）。
    pub fn raw(&self) -> Option<&str> {
        match self {
            ReadFailure::Contradiction { raw, .. } => Some(raw.as_str()),
            _ => None,
        }
    }
}

/// 整窗粗图的列数 / 行数。
///
/// 160 列是「粘到聊天框里也还能看」的上限；45 行让每格高约 32 像素（1440p），
/// 纵横向比例大致保住，图不会被压成一条。
const COARSE_COLS: usize = 160;
const COARSE_ROWS: usize = 45;

/// 底部条落在粗图的哪几行（半开区间）——给行首打标记用，
/// 这样一眼能看出「经验那一行**该**在图上的哪里」，它要是出现在别处就是问题。
fn coarse_rows_in_strip(client_height: i32, strip: i32, rows: usize) -> (usize, usize) {
    if client_height <= 0 || strip <= 0 || rows == 0 {
        return (0, 0);
    }
    let top = (client_height - strip).max(0) as usize;
    let height = client_height as usize;
    let start = (top * rows / height).min(rows - 1);
    let end = ((top + strip as usize) * rows / height).min(rows).max(start + 1);
    (start, end)
}

/// 两张粗图逐格差多少以内算「同一格」。
///
/// 给得比较宽（32/255）：两次抓取之间游戏自己在动（动画、粒子、聊天滚屏），
/// 卡太紧会把“同一份画面”误判成“不一样”，那结论就白下了。
const GRID_TOLERANCE: u8 = 32;

/// 相同率达到多少就认为「两条路抓到的是同一份画面」。
const AGREE_RATIO: f64 = 0.85;

/// 自动定位的框连续两帧交并比到这个数，才认为「稳定了」并落库。
const CONFIRM_IOU: f64 = 0.70;

/// 自动框连续这么多帧读不到就丢掉重找（手动框只标记「可能失效」，不丢）。
const AUTO_MISS_LIMIT: u32 = 2;
/// 手动框连续这么多帧读不到，就把「校准可能失效」告诉界面。
const MANUAL_MISS_LIMIT: u32 = 3;

/// HUD 锚点连续这么多帧「标签对不上」就丢掉重找（窗口改了 UI 缩放 / 挪了位置）。
const HUD_MISS_LIMIT: u32 = 3;
/// HUD 锚点处「标签看得到、数字却读不成」连续这么多帧，也丢掉重找。
///
/// 重新定位之后还是读不成（真读不出来的画面）就把这个数翻倍、最多到
/// [`HUD_UNREAD_LIMIT_MAX`]：越隔越久再试，不把整幅扫描和日志刷成心跳。
const HUD_UNREAD_LIMIT: u32 = 5;
const HUD_UNREAD_LIMIT_MAX: u32 = 80;

/// 读数和经验条填充比例最多差多少（0~1）还算一致。
///
/// 网页版取 1.8%；条是 165 个原生像素宽，一像素就是 0.6%，拉伸后边缘再糊一像素，
/// 这里放到 3%：它只用来拦「认错了一位但恰好过了经验表」的帧，不是精确比对。
const BAR_AGREEMENT: f64 = 0.03;
/// 经验条量得够可信（三条扫描线基本一致）才拿它去否决读数。
const BAR_MIN_CONFIDENCE: f64 = 0.6;

/// 一条读数是从哪条路来的（决定要不要把位置学成「自动框」）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HitSource {
    /// 读的是已有的校准框（手动 / 自动）
    Calibration,
    /// 整幅搜索找到的：连续两帧一致就学成自动框
    Search,
    /// HUD 锚点推出来的：每次启动都会重新定位，不学成框
    Hud,
}

/// HUD 路径这一帧的结果。
enum HudAttempt {
    Read(Sample),
    /// 没定位到 HUD（登录 / 加载 / 被整片盖住）：交给后面的通用路径
    NotLocated,
    /// 定位到了但这一帧读不出来 —— 带着现场（缩放、走过哪些路）给失败提示用
    Unreadable(String),
    /// 读出来了、却和经验表 / 经验条对不上
    Rejected(ReadFailure),
}

/// 抓框时四周外扩：读数解析需要数字左边的 `EXP` 标签或一段空白来定左边界。
pub(crate) fn region_padding(text_height: usize, rect: PixelRect) -> i32 {
    let by_height = (text_height as i32).max(6);
    by_height.min(rect.w.max(rect.h)).clamp(6, 64)
}

/// 从一条读数算出「该记成多大的框」：数字四周留一个字的余量。
fn rect_from_reading(reading: &font::Reading, origin_x: i32, origin_y: i32) -> PixelRect {
    let (x0, y0, x1, y1) = reading.rect;
    let text_height = reading.text_height.max(1) as i32;
    let pad_x = text_height;
    let pad_y = (text_height / 2).max(2);
    PixelRect {
        x: origin_x + x0 as i32 - pad_x,
        y: origin_y + y0 as i32 - pad_y,
        w: (x1 - x0) as i32 + pad_x * 2,
        h: (y1 - y0) as i32 + pad_y * 2,
    }
}

/// 从 OCR 文本里抠出等级数字（`Lv.55` / `55` / `LV 55`；带常见形近字纠正）。
///
/// 只认 1~120：再多就是噪声凑出来的，宁可当没读到。
pub(crate) fn parse_level_ocr(text: &str) -> Option<u32> {
    let mut digits = String::new();
    for ch in text.chars() {
        match ch {
            'O' | 'o' => digits.push('0'),
            'l' | 'I' | '|' => digits.push('1'),
            'S' | 's' => digits.push('5'),
            'Z' | 'z' => digits.push('2'),
            c if c.is_ascii_digit() => digits.push(c),
            _ => {}
        }
    }
    let level: u32 = digits.parse().ok()?;
    (1..=120).contains(&level).then_some(level)
}

/// 读取器：记着找到的窗口、字形表和校准框。
pub struct ExpReader {
    font: Font,
    /// 校准档的持久化出口；单测里是 `None`（只学在内存里）。
    store: Option<Arc<dyn RegionStore>>,
    window: Option<GameWindow>,
    /// 当前使用的框（校准档或自动定位）。
    region: Option<RegionProfile>,
    /// 地图名 / 等级的手动校准框（和经验的框同一套比例模型，只是各自一个槽位）。
    map_region: Option<RegionProfile>,
    level_region: Option<RegionProfile>,
    /// 库里已有的**手动**档：它永远不被自动定位覆盖。
    manual: Option<RegionProfile>,
    /// 自动定位的候选（连续两帧稳定才落库）。
    pending: Option<(NormRect, u32)>,
    /// 当前框（自动档）连续读不到的帧数。
    misses: u32,
    /// 手动档连续读不到的帧数（与自动档分开计：手动框失败后会让位给自动定位，
    /// 但「校准可能失效」的提示要按手动框自己失败了几次来给）。
    manual_misses: u32,
    /// 手动框连续读不到：界面要能提示「校准可能失效」。
    manual_lost: bool,
    /// 首次读取时从 store 载入一次（之后命令层改档会直接刷新这里的缓存）。
    region_loaded: bool,
    /// HUD 锚点定位结果（按客户区尺寸缓存）—— 网页版那套「血条边框 + 标签模板」
    /// 定位，任何分辨率/显示缩放都不用校准。读不到时重找。
    hud_layout: Option<(i32, i32, HudLayout)>,
    /// 上次定位失败的时刻：失败也有冷却，免得每个 tick 都拿全图扫一遍。
    hud_layout_miss: Option<std::time::Instant>,
    /// 用缓存锚点连续失败的次数：够多就丢掉重找。
    hud_misses: u32,
    /// 用缓存锚点「标签看得到、数字却读不成」的连续帧数，以及这一轮的上限
    /// （见 [`ExpReader::note_hud_unread`]）。
    hud_unread: u32,
    hud_unread_limit: u32,
    /// 「读不到」的诊断只写一次日志 —— 每帧都写会把日志冲成一片
    logged_blind: bool,
    /// 上一次失败的 code。**只在 code 变了时才写一行日志**：采样 1~2 秒一次，
    /// 卡在同一种失败上时一天能写几万行，而真正要看的判据（尺寸 / 条高 /
    /// 抓法 / 黑不黑 / 全屏）会被冲得看不见。日志要的是判据，不是心跳。
    last_failure: Option<String>,
    /// 「屏幕 DC 救回来过」这件事**只记一次**：真出问题时（PrintWindow 一直给错
    /// 画面）它会每 1~2 秒重演一次，每次都写就把日志冲了。
    logged_screen_rescue: bool,
    /// 「PP-OCR 兜底救回来过」也只记一次日志，避免每秒刷屏
    logged_ocr_rescue: bool,
    /// HUD 路径上一次成功走的是哪条路（换路时写一行日志）
    logged_hud_path: Option<hudread::ReadPath>,
    /// 上一次存经验条样本图的时间（限流，见 [`ExpReader::save_hud_sample`]）
    last_sample: Option<std::time::Instant>,
    /// 人在不在游戏里（调用方每轮告诉我们，见 `tracker.rs` 的 tick）。
    /// 不在时整幅搜索降频：登录 / 选角色界面上没有经验行，搜也是白搜。
    in_game: bool,
    /// 上一次真的跑整幅搜索的时刻，以及它失败的原因（降频期间沿用这个结论）
    last_wide_search: Option<std::time::Instant>,
    last_wide_failure: Option<ReadFailure>,
}

/// 人不在游戏里时，整幅搜索最多隔这么久跑一次。
///
/// 不是「不跑」：没录入的区服、TCP 表查不到的环境下「在不在游戏里」会判成不在，
/// 那时这条兜底路径仍然要有，只是从每 3 秒一次降到十几秒一次。
const OFFLINE_SEARCH_INTERVAL: std::time::Duration = std::time::Duration::from_secs(15);

/// 这一轮要不要真的跑整幅搜索。
fn wide_search_due(in_game: bool, last: Option<std::time::Instant>) -> bool {
    in_game || last.map_or(true, |at| at.elapsed() >= OFFLINE_SEARCH_INTERVAL)
}

impl ExpReader {
    /// 只学在内存里的读取器（单测用；产品路径一律带 store）。
    #[cfg(test)]
    pub fn new(font: Font) -> Self {
        Self::with_store(font, None)
    }

    pub fn with_store(font: Font, store: Option<Arc<dyn RegionStore>>) -> Self {
        Self {
            font,
            store,
            window: None,
            region: None,
            map_region: None,
            level_region: None,
            manual: None,
            pending: None,
            misses: 0,
            manual_misses: 0,
            manual_lost: false,
            region_loaded: false,
            hud_layout: None,
            hud_layout_miss: None,
            hud_misses: 0,
            hud_unread: 0,
            hud_unread_limit: HUD_UNREAD_LIMIT,
            logged_blind: false,
            last_failure: None,
            logged_screen_rescue: false,
            logged_ocr_rescue: false,
            logged_hud_path: None,
            last_sample: None,
            in_game: true,
            last_wide_search: None,
            last_wide_failure: None,
        }
    }

    /// 告诉读取器人在不在游戏里（不在时整幅搜索降频，见 [`OFFLINE_SEARCH_INTERVAL`]）。
    pub fn set_in_game(&mut self, in_game: bool) {
        self.in_game = in_game;
    }

    pub fn font(&self) -> &Font {
        &self.font
    }

    pub fn window_title(&self) -> Option<String> {
        self.window.as_ref().map(|window| window.title.clone())
    }

    /// 游戏窗口的句柄（认地图要抓小地图那一块，需要它）。
    ///
    /// 顺手把校准档读进来（只做一次）：tracker 的每一轮是「先认地图、再读经验」，
    /// 地图那边的 override 要在这一轮就生效，不能等 `read()` 里才加载。
    pub fn window(&mut self) -> Option<isize> {
        self.ensure_region_loaded();
        self.ensure_window().map(|window| window.hwnd)
    }

    /// 当前用的框（状态栏显示用）。
    pub fn calibration(&self) -> Option<RegionProfile> {
        self.region.clone()
    }

    /// 按 kind 取校准框（地图覆盖 / 等级兜底 / 状态显示都用它）。
    pub fn calibration_for(&self, kind: &str) -> Option<RegionProfile> {
        match kind {
            region::KIND_MAP_NAME => self.map_region.clone(),
            region::KIND_LEVEL => self.level_region.clone(),
            _ => self.region.clone(),
        }
    }

    /// 手动档是不是连续读不到了（界面提示「重新框一次」）。
    pub fn manual_lost(&self) -> bool {
        self.manual_lost
    }

    /// 命令层保存了新的校准档后，把缓存换成它（按 kind 落到各自的槽位）。
    pub fn set_region(&mut self, profile: RegionProfile, manual: bool) {
        match profile.kind.as_str() {
            region::KIND_MAP_NAME => self.map_region = Some(profile),
            region::KIND_LEVEL => self.level_region = Some(profile),
            _ => {
                if manual {
                    self.manual = Some(profile.clone());
                }
                self.region = Some(profile);
                self.pending = None;
                self.misses = 0;
                self.manual_misses = 0;
                if manual {
                    self.manual_lost = false;
                }
            }
        }
        self.region_loaded = true;
    }

    /// 命令层清掉某个 kind 的校准后调用。
    pub fn clear_region(&mut self, kind: &str) {
        match kind {
            region::KIND_MAP_NAME => self.map_region = None,
            region::KIND_LEVEL => self.level_region = None,
            region::KIND_EXP_LINE => {
                self.region = None;
                self.manual = None;
                self.pending = None;
                self.misses = 0;
                self.manual_misses = 0;
                self.manual_lost = false;
            }
            _ => {}
        }
        self.region_loaded = true;
    }

    /// 手上的句柄还有效吗。
    ///
    /// **不要把「不能抓」当成「句柄废了」**：最小化只是暂时抓不到，句柄还是好的 ——
    /// 否则每一轮采样都要重新枚举一遍窗口，报出来的原因也会从「游戏最小化了」
    /// 退化成「没找到游戏窗口」。
    fn ensure_window(&mut self) -> Option<&GameWindow> {
        let valid = self
            .window
            .as_ref()
            .map(|window| capture::is_window(window.hwnd))
            .unwrap_or(false);
        if !valid {
            self.window = capture::find_game_window();
        }
        self.window.as_ref()
    }

    /// 把校准档读进内存（只做一次；命令层改档会直接刷新缓存）。
    fn ensure_region_loaded(&mut self) {
        if self.region_loaded {
            return;
        }
        self.region_loaded = true;
        let Some(store) = self.store.clone() else {
            return;
        };
        if let Some(profile) = store.load_region(region::KIND_EXP_LINE) {
            log::info!(
                "已载入经验行校准：{} 档 · 比例 ({:.3},{:.3}) {:.3}×{:.3}",
                profile.source.as_str(),
                profile.rect.x,
                profile.rect.y,
                profile.rect.w,
                profile.rect.h
            );
            if profile.source == RegionSource::Manual {
                self.manual = Some(profile.clone());
            }
            self.region = Some(profile);
        }
        self.map_region = store.load_region(region::KIND_MAP_NAME);
        self.level_region = store.load_region(region::KIND_LEVEL);
    }

    /// 记一次失败：**只在失败原因换了的时候**写一行日志，带上现场证据。
    ///
    /// 用户报的是「不能用了」，能定位的只有判据：客户区多大、这次搜了哪儿、
    /// 黑不黑、游戏在不在前台、有没有校准框。没有这一行，几种失败在日志里
    /// 只剩一句「读不到」，下次报障还是只能猜。
    fn note_failure(&mut self, failure: ReadFailure, evidence: &str) -> ReadFailure {
        let code = failure.code().to_string();
        if self.last_failure.as_deref() != Some(code.as_str()) {
            self.last_failure = Some(code.clone());
            log::info!(
                "经验读取失败（{}）：{}｜ {}",
                code,
                failure.message(),
                evidence
            );
        }
        failure
    }

    /// 抓一个矩形：先后台截屏；读不出来（或整片空白）且游戏在前台时用屏幕 DC 兜底。
    ///
    /// 屏幕 DC 只有「游戏在前台、而且那块没被别的窗口盖住」才可信；
    /// 我们不猜遮挡，用「这一份里读不读得出经验行」验证 —— 这也是整个项目
    /// 「试一条、验证、记住」的做法。
    fn grab_and_read(&mut self, hwnd: isize, rect: PixelRect) -> Option<(font::Reading, Method)> {
        if let Some((frame, method)) = capture::grab_client(hwnd, rect.x, rect.y, rect.w, rect.h) {
            let pixels = frame.pixels();
            if !pixels.looks_blank() {
                if let Some(reading) = self.recognize(&pixels) {
                    return Some((reading, method));
                }
            }
        }
        if capture::is_foreground(hwnd) {
            if let Some(frame) =
                capture::grab_client_by_method(hwnd, rect.x, rect.y, rect.w, rect.h, Method::Screen)
            {
                let pixels = frame.pixels();
                if !pixels.looks_blank() {
                    if let Some(reading) = self.recognize(&pixels) {
                        if !self.logged_screen_rescue {
                            self.logged_screen_rescue = true;
                            log::info!(
                                "后台截屏读不出经验那一行，改用屏幕 DC 读出来了（{}×{} @{},{})",
                                rect.w,
                                rect.h,
                                rect.x,
                                rect.y
                            );
                        }
                        return Some((reading, Method::Screen));
                    }
                }
            }
        }
        None
    }

    /// 认一帧（手动框 / 自动框 / 整幅搜索共用）：点阵字形 → 按行高推算缩放逐字比对
    /// → PP-OCR 兜底。
    ///
    /// 返回的是**字形路径能给出的最好读数**（可能是不自洽的那条，交给上层如实报）；
    /// 逐字比对 / OCR 只有比它更好的时候才顶替。
    fn recognize(&mut self, pixels: &Pixels<'_>) -> Option<font::Reading> {
        let glyph = font::read_frame(pixels, &self.font);
        if let Some(reading) = &glyph {
            if table::validate(reading.exp, reading.percent) != Verdict::Contradiction {
                return glyph;
            }
        }

        // 手动框 / 自动框这种小块：整块直接逐字比对（全屏拉伸下点阵对不上，靠它）
        if pixels.height <= 300 {
            if let Some(reading) = hudread::read_unanchored(pixels, &self.font) {
                return Some(reading);
            }
        }

        // 候选带最多喂 6 条给 OCR：单条推理几十毫秒，多了这一帧就超过采样间隔了。
        // 排序带底部先验（经验行在 HUD 上），所以这几条里通常就有它。
        for candidate in font::find_candidate_boxes(pixels, 6) {
            let (x, y, w, h) = candidate.rect;
            let (crop_w, crop_h, buffer) = pixels.crop(x, y, w, h);
            if crop_w < 8 || crop_h < 8 {
                continue;
            }
            let crop = Pixels {
                width: crop_w,
                height: crop_h,
                bgra: &buffer,
            };
            // 大画面（整幅搜索）的候选带：先逐字比对，比 OCR 稳也比 OCR 快
            if pixels.height > 300 {
                if let Some(reading) = hudread::read_unanchored(&crop, &self.font) {
                    let (rx0, ry0, rx1, ry1) = reading.rect;
                    return Some(font::Reading {
                        rect: (rx0 + x, ry0 + y, rx1 + x, ry1 + y),
                        ..reading
                    });
                }
            }
            let Some(ocr) = crate::exp::ocr::recognize_line(&crop) else {
                continue;
            };
            let Some((exp, percent)) = crate::exp::ocr::parse_exp_ocr(&ocr.text) else {
                continue;
            };
            if !self.logged_ocr_rescue {
                self.logged_ocr_rescue = true;
                log::info!(
                    "点阵字形未命中，改用 PP-OCR 识别成功（实测字高 {} 像素，读出 {}）",
                    candidate.text_height,
                    ocr.text
                );
            }
            return Some(font::Reading {
                exp,
                percent,
                raw: format!("{} [OCR字高{}px]", ocr.text, candidate.text_height),
                rect: (x, y, x + w, y + h),
                exact: 0,
                glyphs: 0,
                text_height: candidate.text_height,
            });
        }

        glyph
    }

    /// 抓一个框、跑 PP-OCR。地图名 / 等级的校准与运行期读取共用。
    fn ocr_exact(&mut self, hwnd: isize, rect: PixelRect) -> Option<crate::exp::ocr::Reading> {
        let (frame, _) = capture::grab_client(hwnd, rect.x, rect.y, rect.w, rect.h)?;
        let pixels = frame.pixels();
        if pixels.looks_blank() {
            return None;
        }
        crate::exp::ocr::recognize_line(&pixels)
    }

    /// 同一块地方给两个裁剪版本让调用方挑：**先原样**，再带少量上下文。
    ///
    /// 为什么要「先原样」：PP-OCR 是**单行**模型。经验条那套「四周留白」的规则
    /// 套到地图名 / 等级上，会把上下相邻的行一起裁进来，模型读出来是一串混字
    /// （真机现场：框得越准反而认不出，框得偏一点反而过）。原样读不出来时，
    /// 再试带一点上下文的版本（框太紧、笔画被切掉时用得上）。
    fn ocr_region_variants(
        &mut self,
        hwnd: isize,
        area: PixelRect,
        client_w: i32,
        client_h: i32,
    ) -> Vec<crate::exp::ocr::Reading> {
        let mut variants = vec![area];
        // 上下文最多给到框高的一半：再多就又要并行了
        let pad = region_padding(area.h.max(8) as usize, area).min((area.h / 2).max(2));
        let padded = area.expanded(pad, client_w, client_h);
        if padded != area {
            variants.push(padded);
        }
        variants
            .into_iter()
            .filter_map(|rect| self.ocr_exact(hwnd, rect))
            .collect()
    }

    /// 从**校准过的等级区域**读等级（经验表定不出等级时才用，见 `read`）。
    ///
    /// 没校准过就走 HUD 锚点推出的等级框 —— 网页版就是这么自动定位的。
    fn read_level_region(&mut self, hwnd: isize, client_w: i32, client_h: i32) -> Option<u32> {
        let mut areas: Vec<PixelRect> = Vec::new();
        if let Some(profile) = self.level_region.clone() {
            if let Some(area) = profile.rect.to_pixel(client_w, client_h) {
                areas.push(area);
            }
        }
        if let Some(area) = self.hud_level_box(hwnd, client_w, client_h) {
            areas.push(area);
        }
        for area in areas {
            for reading in self.ocr_region_variants(hwnd, area, client_w, client_h) {
                if let Some(level) = parse_level_ocr(&reading.text) {
                    return Some(level);
                }
            }
        }
        None
    }

    /// HUD 锚点推出的等级框（没有缓存就抓一帧定位一次）。
    fn hud_level_box(&mut self, hwnd: isize, client_w: i32, client_h: i32) -> Option<PixelRect> {
        let layout = self.ensure_hud_layout(hwnd, client_w, client_h)?;
        let (x, y, w, h) = layout.level_box;
        Some(PixelRect { x, y, w, h })
    }

    /// HUD 锚点（网页版那套确定性的多尺度定位）。
    ///
    /// 结果按客户区尺寸缓存：同一尺寸下锚点不会变，没必要每个 tick 都扫全图；
    /// 找不到时也有 15 秒冷却（登录界面 / HUD 被面板挡住时不至于每个 tick 都扫）。
    /// 连续读不到 3 次会把缓存丢掉重找（游戏窗口挪动 / 改 UI 缩放都能自愈）。
    fn ensure_hud_layout(
        &mut self,
        hwnd: isize,
        client_w: i32,
        client_h: i32,
    ) -> Option<HudLayout> {
        if let Some((cached_w, cached_h, layout)) = self.hud_layout {
            if cached_w == client_w && cached_h == client_h {
                return Some(layout);
            }
        }
        if let Some(at) = self.hud_layout_miss {
            if at.elapsed() < std::time::Duration::from_secs(15) {
                return None;
            }
        }
        let (frame, _) = capture::grab_client(hwnd, 0, 0, client_w, client_h)?;
        let pixels = frame.pixels();
        if pixels.looks_blank() {
            self.hud_layout_miss = Some(std::time::Instant::now());
            return None;
        }
        let layout = match hud::locate(&pixels) {
            Some(layout) => layout,
            None => {
                self.hud_layout_miss = Some(std::time::Instant::now());
                return None;
            }
        };
        log::info!(
            "HUD 锚点定位：客户区 {}×{} · {} · 缩放 {:.3}×{:.3} · 误差 {:.1} · 经验行 ({}, {}, {}×{}) · 经验条 ({}, {}, {}×{})",
            client_w,
            client_h,
            layout.anchor.skin.label(),
            layout.anchor.sx,
            layout.anchor.sy,
            layout.anchor.error,
            layout.exp_text.0,
            layout.exp_text.1,
            layout.exp_text.2,
            layout.exp_text.3,
            layout.exp_bar.0,
            layout.exp_bar.1,
            layout.exp_bar.2,
            layout.exp_bar.3
        );
        self.hud_layout = Some((client_w, client_h, layout));
        self.hud_layout_miss = None;
        self.hud_misses = 0;
        Some(layout)
    }

    /// 状态栏上（角色名, 职业）那两行字的位置。
    ///
    /// 锚点没有就现找一次（找不到有冷却，见 [`Self::ensure_hud_layout`]）。**不能只用
    /// 已经缓存的**：经验行、地图名都手动校准过的人，读数走的是手动框，HUD 那条路
    /// 一次都不会跑，锚点永远是空的 —— 角色列表就一直是空的（真机上就是这么漏的）。
    pub fn identity_rects(
        &mut self,
        client_w: i32,
        client_h: i32,
    ) -> Option<((i32, i32, i32, i32), (i32, i32, i32, i32))> {
        let hwnd = self.ensure_window()?.hwnd;
        let layout = self.ensure_hud_layout(hwnd, client_w, client_h)?;
        Some((layout.name_text?, layout.job_text?))
    }

    /// 自动的地图名区域（HUD 锚点 → 小地图）。给 tracker 的识图流程用：
    /// 手动校准过就优先用那个（调用方负责），这里只是自动兜底。
    pub fn ensure_auto_map_rect(&mut self, client_w: i32, client_h: i32) -> Option<PixelRect> {
        let hwnd = self.ensure_window()?.hwnd;
        let layout = self.ensure_hud_layout(hwnd, client_w, client_h)?;
        layout.map_name.map(|(x, y, w, h)| PixelRect { x, y, w, h })
    }

    /// 锚点处标签看得到、这一帧却没读成（数字认不出 / 和经验条、经验表对不上）。
    ///
    /// 连续够多帧就丢掉锚点重新定位。换角色之后「只有重开软件才好」就是卡在这儿：
    /// 进角色那一下 HUD 还在过场里，那时定下的锚点会差一两个像素 / 一点缩放；
    /// 标签那道检查放得宽（半径 2、误差 50）照样过，数字却永远对不上 ——
    /// 而以前只有「标签看不到」才会丢锚点，这种锚点就一直用到退出。
    fn note_hud_unread(&mut self) {
        self.hud_unread += 1;
        if self.hud_unread < self.hud_unread_limit {
            return;
        }
        log::info!(
            "HUD 锚点处连续 {} 帧看得到 EXP 标签却读不成，丢掉重新定位",
            self.hud_unread
        );
        self.hud_layout = None;
        self.hud_unread = 0;
        self.hud_unread_limit = (self.hud_unread_limit * 2).min(HUD_UNREAD_LIMIT_MAX);
    }

    /// HUD 路径（主路径）：锚点定位（按客户区尺寸缓存）→ 只抓经验那一小条 →
    /// 确认标签还在 → 按锚点量出的缩放还原像素认字 → 和经验条比例互相印证。
    ///
    /// 这条路不看分辨率、不看显示缩放、不看是不是全屏拉伸 —— 缩放是从画面里
    /// 量出来的（网页版枫记的做法）。
    fn read_via_hud(
        &mut self,
        hwnd: isize,
        client_w: i32,
        client_h: i32,
        scene: &str,
    ) -> HudAttempt {
        let Some(layout) = self.ensure_hud_layout(hwnd, client_w, client_h) else {
            return HudAttempt::NotLocated;
        };
        let (sx, sy, sw, sh) = layout.read_strip(client_w, client_h);
        let Some((frame, mut method)) = capture::grab_client(hwnd, sx, sy, sw, sh) else {
            return HudAttempt::NotLocated;
        };
        let mut frame = frame;
        let visible = |frame: &capture::Frame| {
            let pixels = frame.pixels();
            !pixels.looks_blank() && hud::exp_label_visible(&pixels, (sx, sy), &layout)
        };
        if !visible(&frame) && capture::is_foreground(hwnd) {
            // 后台截屏在无边框 / 独占全屏下偶尔给的是错的画面，屏幕 DC 反而拍得到
            if let Some(screen) =
                capture::grab_client_by_method(hwnd, sx, sy, sw, sh, Method::Screen)
            {
                if visible(&screen) {
                    frame = screen;
                    method = Method::Screen;
                }
            }
        }
        if !visible(&frame) {
            // 标签对不上：被面板盖住 / 加载中 / 窗口改了 UI 缩放。连续几帧就丢掉锚点重找
            self.hud_misses += 1;
            if self.hud_misses >= HUD_MISS_LIMIT {
                log::info!(
                    "HUD 锚点处连续 {} 帧看不到 EXP 标签，丢掉重新定位",
                    self.hud_misses
                );
                self.hud_layout = None;
                self.hud_misses = 0;
            }
            return HudAttempt::Unreadable(format!(
                "，HUD 在 ({:.0},{:.0}) 缩放 {:.2}，但这一帧那里看不到 EXP 标签（被面板盖住 / 加载中？）",
                layout.anchor.x, layout.anchor.y, layout.anchor.sx
            ));
        }
        self.hud_misses = 0;

        let pixels = frame.pixels();
        let bar = hud::exp_bar_ratio(&pixels, (sx, sy), layout.exp_bar);
        let Some(read) = hudread::read_exp(&pixels, (sx, sy), &layout, &self.font) else {
            self.save_hud_sample(&pixels, (sx, sy), (client_w, client_h), &layout, "读不出");
            self.note_hud_unread();
            return HudAttempt::Unreadable(format!(
                "，HUD 已定位（{} · 缩放 {:.3}×{:.3}），经验条 {}，但数字认不出（原生字形 / 还原像素 / OCR 都试过）",
                layout.anchor.skin.label(),
                layout.anchor.sx,
                layout.anchor.sy,
                bar.map(|b| format!("{:.1}%", b.ratio * 100.0))
                    .unwrap_or_else(|| "量不出".to_string())
            ));
        };

        // 和经验条互相印证：认错一位却恰好过了经验表的帧，在这里被拦下
        if let Some(bar) = bar.filter(|b| b.confidence >= BAR_MIN_CONFIDENCE) {
            let gap = (read.reading.percent / 100.0 - bar.ratio).abs();
            if gap > BAR_AGREEMENT {
                let evidence = format!(
                    "{scene} · HUD 读出 {}，经验条却是 {:.2}%（差 {:.2}%）",
                    read.reading.raw,
                    bar.ratio * 100.0,
                    gap * 100.0
                );
                self.note_hud_unread();
                return HudAttempt::Rejected(self.note_failure(
                    ReadFailure::Contradiction {
                        raw: read.reading.raw.clone(),
                        exp: read.reading.exp,
                        percent: read.reading.percent,
                    },
                    &evidence,
                ));
            }
        }

        if self.logged_hud_path != Some(read.path) {
            self.logged_hud_path = Some(read.path);
            // 每条路第一次读成也存一张：真机上各种渲染长什么样，下次对着改
            let tag = format!("读成·{}", read.path.label());
            self.save_hud_sample(&pixels, (sx, sy), (client_w, client_h), &layout, &tag);
            log::info!(
                "经验读数走 HUD 路径：{}（{} · 缩放 {:.3}×{:.3} · 读出 {} · 经验条 {}）",
                read.path.label(),
                layout.anchor.skin.label(),
                layout.anchor.sx,
                layout.anchor.sy,
                read.reading.raw,
                bar.map(|b| format!("{:.2}%", b.ratio * 100.0))
                    .unwrap_or_else(|| "量不出".to_string())
            );
        }
        let origin = PixelRect {
            x: 0,
            y: 0,
            w: client_w,
            h: client_h,
        };
        match self.sample_from_reading(
            hwnd,
            client_w,
            client_h,
            scene,
            read.reading,
            method,
            origin,
            HitSource::Hud,
        ) {
            Ok(sample) => {
                self.hud_unread = 0;
                self.hud_unread_limit = HUD_UNREAD_LIMIT;
                HudAttempt::Read(sample)
            }
            Err(failure) => {
                self.note_hud_unread();
                HudAttempt::Rejected(failure)
            }
        }
    }

    /// 把 HUD 那一条存成 PNG 放进日志目录的 `经验样本` 里 —— 用户把日志文件夹发回来，
    /// 就有真机画面可以对着改（单靠日志文字，没见过的渲染方式只能猜）。
    ///
    /// 文件名带上复原现场要的全部数：客户区、锚点、缩放、这一条在客户区里的位置。
    /// 限流：两张之间至少隔 10 秒，目录里最多留 40 张，不会越攒越多。
    fn save_hud_sample(
        &mut self,
        pixels: &Pixels<'_>,
        origin: (i32, i32),
        client: (i32, i32),
        layout: &HudLayout,
        tag: &str,
    ) {
        const MAX_SAMPLES: usize = 40;
        const MIN_GAP: std::time::Duration = std::time::Duration::from_secs(10);
        if cfg!(test) || self.last_sample.is_some_and(|at| at.elapsed() < MIN_GAP) {
            return;
        }
        self.last_sample = Some(std::time::Instant::now());
        let Some(dir) = std::env::var_os("LOCALAPPDATA").map(|base| {
            std::path::PathBuf::from(base)
                .join("com.mxdbox.tool")
                .join("logs")
                .join("经验样本")
        }) else {
            return;
        };
        if std::fs::create_dir_all(&dir).is_err() {
            return;
        }
        let existing = std::fs::read_dir(&dir).map(|d| d.count()).unwrap_or(0);
        if existing >= MAX_SAMPLES {
            return;
        }
        let a = layout.anchor;
        let name = format!(
            "{}-{}-客户区{}x{}-锚点{:.1}_{:.1}-缩放{:.4}x{:.4}-{}-原点{}_{}.png",
            chrono::Local::now().format("%m%d-%H%M%S"),
            tag,
            client.0,
            client.1,
            a.x,
            a.y,
            a.sx,
            a.sy,
            a.skin.label(),
            origin.0,
            origin.1
        );
        let rgba: Vec<u8> = pixels
            .bgra
            .chunks_exact(4)
            .flat_map(|p| [p[2], p[1], p[0], 255])
            .collect();
        let path = dir.join(&name);
        let written = std::fs::File::create(&path).ok().and_then(|file| {
            let mut encoder = png::Encoder::new(
                std::io::BufWriter::new(file),
                pixels.width as u32,
                pixels.height as u32,
            );
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            let mut writer = encoder.write_header().ok()?;
            writer.write_image_data(&rgba).ok()
        });
        if written.is_some() {
            log::info!("存了一张经验条样本：{}", path.display());
        }
    }

    /// 自动定位学到的框：连续两帧稳定才落库；手动档在场时只暂用、不覆盖。
    fn note_auto_found(&mut self, rect: NormRect, text_height: usize) -> RegionProfile {
        let profile = RegionProfile {
            kind: region::KIND_EXP_LINE.to_string(),
            rect,
            source: RegionSource::Auto,
            text_height: Some(text_height as u32),
            learned_unix: chrono::Utc::now().timestamp(),
        };
        let hits = match &mut self.pending {
            Some((previous, hits)) if previous.iou(rect) >= CONFIRM_IOU => {
                *hits += 1;
                *hits
            }
            _ => {
                self.pending = Some((rect, 1));
                1
            }
        };
        self.misses = 0;
        // 手动框读不到、而自动定位在别处读出来了：如实标记「手动校准可能失效」，
        // 让界面提醒用户（不静默换位置，也不覆盖手动档）。
        if let Some(manual) = &self.manual {
            if manual.rect.iou(rect) < 0.5 {
                self.manual_lost = true;
            }
        }
        // 还没稳定：先暂用（下一帧在框内读，读出来且位置一致就算第二次确认）。
        self.region = Some(profile.clone());
        if hits >= 2 && self.manual.is_none() {
            if let Some(store) = &self.store {
                if let Err(err) = store.save_region(&profile) {
                    log::warn!("保存自动校准失败：{err}");
                } else {
                    log::info!(
                        "已自动学到经验行位置：比例 ({:.3},{:.3}) {:.3}×{:.3}",
                        rect.x,
                        rect.y,
                        rect.w,
                        rect.h
                    );
                }
            }
            self.pending = None;
        }
        profile
    }

    /// 在框内读成功之后，把这条读数也算作「自动候选的一次确认」（避免反复全帧搜索）。
    fn confirm_pending(&mut self, reading_rect: NormRect) {
        let Some((previous, hits)) = self.pending.clone() else {
            return;
        };
        if previous.iou(reading_rect) < CONFIRM_IOU {
            // 框内的读数和暂用框差太多：重新从当前读数起算
            self.pending = Some((reading_rect, 1));
            return;
        }
        let hits = hits + 1;
        self.pending = Some((reading_rect, hits));
        if hits >= 2 && self.manual.is_none() {
            if let Some(profile) = self.region.clone() {
                let profile = RegionProfile {
                    rect: reading_rect,
                    learned_unix: chrono::Utc::now().timestamp(),
                    ..profile
                };
                if let Some(store) = &self.store {
                    if let Err(err) = store.save_region(&profile) {
                        log::warn!("保存自动校准失败：{err}");
                    } else {
                        log::info!("已自动学到经验行位置（框内二次确认）");
                    }
                }
                self.region = Some(profile);
                self.pending = None;
            }
        }
    }

    /// 框连续读不到时的处置。
    fn note_region_miss(&mut self) {
        self.misses += 1;
        let source = self.region.as_ref().map(|profile| profile.source);
        match source {
            Some(RegionSource::Auto) if self.misses >= AUTO_MISS_LIMIT => {
                log::info!("自动学到的经验行位置连续读不到，丢掉重新找");
                if let Some(store) = &self.store {
                    if let Err(err) = store.clear_region(region::KIND_EXP_LINE) {
                        log::warn!("清掉失效的自动校准失败：{err}");
                    }
                }
                self.region = None;
                self.pending = None;
                self.misses = 0;
            }
            Some(RegionSource::Manual) => {
                self.manual_misses += 1;
                if self.manual_misses >= MANUAL_MISS_LIMIT && !self.manual_lost {
                    self.manual_lost = true;
                    log::info!("手动校准的经验行位置连续读不到，已标记为可能失效");
                }
            }
            _ => {}
        }
    }

    /// 把一条读数收尾成 [`Sample`]：过经验表自检 → 记录框 → （定不出等级时）
    /// 读一次校准过 / HUD 锚点推出的等级框。
    ///
    /// `origin` 是「这条读数的坐标相对哪一块」：校准框读的是框的像素矩形，
    /// 整幅搜索是 (0,0)；HUD 路径的读数本来就是客户区坐标，也传 (0,0)。
    #[allow(clippy::too_many_arguments)]
    fn sample_from_reading(
        &mut self,
        hwnd: isize,
        client_w: i32,
        client_h: i32,
        scene: &str,
        reading: font::Reading,
        method: Method,
        origin: PixelRect,
        hit_source: HitSource,
    ) -> Result<Sample, ReadFailure> {
        let level = match table::validate(reading.exp, reading.percent) {
            Verdict::Known(level) => Some(level),
            Verdict::Ambiguous => None,
            Verdict::Contradiction => {
                let evidence = format!("{scene} · 抓法 {method:?} · 读出 {}", reading.raw);
                return Err(self.note_failure(
                    ReadFailure::Contradiction {
                        raw: reading.raw,
                        exp: reading.exp,
                        percent: reading.percent,
                    },
                    &evidence,
                ));
            }
        };
        let hit = self.hit_from_region(&reading, origin, (client_w, client_h), hit_source);
        // 经验表定不出等级（`0(0.00%)` 这类多解帧）时，才去读校准过的 /
        // HUD 锚点推出的等级区域：正常帧不走这一次 OCR。
        let screen_level = if level.is_none() {
            self.read_level_region(hwnd, client_w, client_h)
        } else {
            None
        };
        self.logged_screen_rescue = method == Method::Screen;
        self.logged_ocr_rescue = reading.raw.contains("[OCR");
        self.last_failure = None;
        Ok(Sample {
            exp: reading.exp,
            percent: reading.percent,
            level,
            raw: reading.raw,
            method,
            region: Some(hit),
            screen_level,
        })
    }

    /// 这次读数该交出怎样的框信息（并把自动候选推进一帧）。
    fn hit_from_region(
        &mut self,
        reading: &font::Reading,
        origin: PixelRect,
        client: (i32, i32),
        hit_source: HitSource,
    ) -> RegionHit {
        if hit_source == HitSource::Hud {
            // HUD 锚点每次启动都会重新定位，不需要学成「自动框」落库 ——
            // 落了反而有害：下一帧会先拿那个框走通用路径（非整数缩放下必然读不出），
            // 白白多抓一次屏、多记一次失败。
            let found = rect_from_reading(reading, origin.x, origin.y).clamp_to(client.0, client.1);
            let rect = NormRect::from_pixel(found, client.0, client.1).unwrap_or(NormRect {
                x: 0.0,
                y: 0.0,
                w: 1.0,
                h: 1.0,
            });
            return RegionHit {
                rect,
                source: RegionSource::Auto,
                text_height: reading.text_height,
            };
        }
        if hit_source == HitSource::Calibration {
            if let Some(profile) = self.region.clone() {
                if profile.source == RegionSource::Manual {
                    self.manual_lost = false;
                    self.manual_misses = 0;
                }
                self.misses = 0;
                if profile.source == RegionSource::Auto && self.pending.is_some() {
                    let found = rect_from_reading(reading, origin.x, origin.y)
                        .clamp_to(client.0, client.1);
                    if let Some(rect) = NormRect::from_pixel(found, client.0, client.1) {
                        self.confirm_pending(rect);
                    }
                }
                return RegionHit {
                    rect: profile.rect,
                    source: profile.source,
                    text_height: reading.text_height,
                };
            }
        }
        let found = rect_from_reading(reading, origin.x, origin.y).clamp_to(client.0, client.1);
        let rect = NormRect::from_pixel(found, client.0, client.1)
            .unwrap_or(NormRect {
                x: 0.0,
                y: 0.0,
                w: 1.0,
                h: 1.0,
            });
        let profile = self.note_auto_found(rect, reading.text_height);
        RegionHit {
            rect: profile.rect,
            source: profile.source,
            text_height: reading.text_height,
        }
    }

    /// 读一次。返回 `Err` 时**绝不能用上一次的值凑合**（那是假数据）。
    pub fn read(&mut self) -> Result<Sample, ReadFailure> {
        // 先把句柄拿成一个 `isize`：别让 `&self.window` 的借用跨到下面的
        // `note_failure(&mut self)` —— 那里要写 `last_failure`。
        let hwnd = self.ensure_window().map(|window| window.hwnd);
        let Some(hwnd) = hwnd else {
            return Err(
                self.note_failure(ReadFailure::NoWindow, "没有可抓的游戏窗口（游戏没开？）")
            );
        };

        let (client_w, client_h) = capture::client_size(hwnd).unwrap_or((0, 0));
        let mode = capture::screen_mode(hwnd);
        let scene = format!(
            "客户区 {client_w}×{client_h} · 形态={} · 前台={}",
            mode.label(),
            capture::is_foreground(hwnd)
        );
        // 最小化要先判：最小化的窗口客户区就是 0×0，后判会被报成「抓不到画面」
        if capture::is_minimized(hwnd) {
            return Err(self.note_failure(ReadFailure::Minimized, &scene));
        }

        if client_w <= 0 || client_h <= 0 {
            return Err(self.note_failure(
                ReadFailure::CaptureFailed("拿不到客户区尺寸".to_string()),
                &scene,
            ));
        }

        self.ensure_region_loaded();

        // 顺序：**手动框 → HUD 锚点 → 自动框 → 整幅搜索**。
        //
        // 手动框是用户明确指定的，先读它；HUD 锚点是主路径（任何缩放都读得出）；
        // 旧版本学下来的自动框排在 HUD 后面 —— 它走的是通用路径，全屏拉伸下
        // 必然读不出，排在前面只会让每一帧先白失败一次。
        //
        // 手动框已经被判「可能失效」（连续读不到）时让 HUD 先上：真机日志里
        // 一个失效的手动框每帧先失败一次、再把通用路径的 OCR 跑一遍，一帧要 20 秒。
        let manual_first = !self.manual_lost
            && self
                .region
                .as_ref()
                .map(|profile| profile.source == RegionSource::Manual)
                .unwrap_or(false);

        // ── 1. 手动框 ───────────────────────────────────────────────────────
        if manual_first {
            if let Some(result) = self.read_region(hwnd, client_w, client_h, &scene) {
                return result;
            }
        }

        // ── 2. HUD 锚点（主路径）────────────────────────────────────────────
        let mut hud_detail: Option<String> = None;
        let mut hud_rejection: Option<ReadFailure> = None;
        match self.read_via_hud(hwnd, client_w, client_h, &scene) {
            HudAttempt::Read(sample) => return Ok(sample),
            HudAttempt::NotLocated => {}
            HudAttempt::Unreadable(detail) => hud_detail = Some(detail),
            HudAttempt::Rejected(failure) => hud_rejection = Some(failure),
        }

        // ── 3. 自动框（旧版本学下来的）/ 已判失效的手动框 ────────────────────
        if !manual_first {
            if let Some(result) = self.read_region(hwnd, client_w, client_h, &scene) {
                return result;
            }
        }

        // ── 4. 整幅客户区搜索（通用路径）───────────────────────────────────
        //
        // 这一步最贵：整幅抓屏 + 候选横带 + OCR。人不在游戏里、HUD 锚点也没定位到时
        // （登录 / 选角色界面）它注定找不到，没必要每轮都跑 —— 隔一阵才真跑一次，
        // 其余时候沿用上一次的结论。HUD 那条主路径上面已经试过了，不受影响。
        let hud_absent = hud_detail.is_none() && hud_rejection.is_none();
        if hud_absent && !wide_search_due(self.in_game, self.last_wide_search) {
            if let Some(failure) = self.last_wide_failure.clone() {
                return Err(failure);
            }
        }
        let outcome = match self.search_whole_client(hwnd, client_w, client_h, &scene, hud_detail)
        {
            // HUD 那边读出来了只是没过校验：如实报「对不上」，别报成「没有那一行」
            Err(ReadFailure::NoFieldAt { .. }) if hud_rejection.is_some() => {
                Err(hud_rejection.expect("刚判过 is_some"))
            }
            other => other,
        };
        self.last_wide_search = Some(std::time::Instant::now());
        self.last_wide_failure = outcome.as_ref().err().cloned();
        outcome
    }

    /// 读当前的校准框（手动 / 自动）。`None` = 框读不到，继续走下一条路。
    fn read_region(
        &mut self,
        hwnd: isize,
        client_w: i32,
        client_h: i32,
        scene: &str,
    ) -> Option<Result<Sample, ReadFailure>> {
        let profile = self.region.clone()?;
        if let Some(area) = profile.rect.to_pixel(client_w, client_h) {
            let pad = region_padding(profile.text_height.unwrap_or(8) as usize, area);
            let padded = area.expanded(pad, client_w, client_h);
            if let Some((reading, method)) = self.grab_and_read(hwnd, padded) {
                match self.sample_from_reading(
                    hwnd,
                    client_w,
                    client_h,
                    scene,
                    reading,
                    method,
                    padded,
                    HitSource::Calibration,
                ) {
                    Ok(sample) => return Some(Ok(sample)),
                    // 框内读到但不自洽：不采纳，继续走下一条路
                    Err(ReadFailure::Contradiction { .. }) => {}
                    Err(other) => return Some(Err(other)),
                }
            }
        }
        self.note_region_miss();
        None
    }

    /// 抓整幅客户区，多尺度找经验行；找到就成了自动校准（连续两帧稳定后落库）。
    ///
    /// `hud_detail`：HUD 路径定位到了却没读出来时的现场，读不到时拼进失败提示。
    fn search_whole_client(
        &mut self,
        hwnd: isize,
        client_w: i32,
        client_h: i32,
        scene: &str,
        hud_detail: Option<String>,
    ) -> Result<Sample, ReadFailure> {
        let whole = PixelRect {
            x: 0,
            y: 0,
            w: client_w,
            h: client_h,
        };
        let Some((frame, method)) = capture::grab_client(hwnd, 0, 0, client_w, client_h) else {
            let evidence = format!("{scene} · 整幅抓屏返回 None（坐标取不到或渲染失败）");
            return Err(
                self.note_failure(ReadFailure::CaptureFailed("窗口坐标取不到或渲染失败".to_string()), &evidence)
            );
        };
        let pixels = frame.pixels();
        if pixels.looks_blank() {
            // 形态不同，该做的事不同：只有占满屏幕且无边框那档才谈得上“改窗口模式”
            let evidence = format!("{scene} · 抓法 {method:?} · 整片空白=true");
            return Err(self.note_failure(ReadFailure::BlankFrame { mode: capture::screen_mode(hwnd) }, &evidence));
        }

        let mut method = method;

        // HUD 锚点路径已经在 `read()` 里先走过了；到这里是它没定位到 / 没读出来，
        // 用通用的候选横带 + OCR 再找一遍。
        let mut reading = self.recognize(&pixels);
        let mut both_tried: Option<(capture::LumaStats, capture::LumaStats)> = None;
        if reading.is_none() && capture::is_foreground(hwnd) {
            // PrintWindow 在无边框 / 独占全屏下可能给的是**错的画面**（不是游戏），
            // 屏幕 DC 有时反而拍得到 —— 这一条能把「拍错了」救回来。
            if let Some(screen_frame) =
                capture::grab_client_by_method(hwnd, 0, 0, client_w, client_h, Method::Screen)
            {
                let screen_pixels = screen_frame.pixels();
                match self.recognize(&screen_pixels) {
                    Some(found) => {
                        if !self.logged_screen_rescue {
                            self.logged_screen_rescue = true;
                            log::info!(
                                "整幅后台截屏读不出经验那一行，改用屏幕 DC 读出来了（{}）",
                                scene
                            );
                        }
                        method = Method::Screen;
                        reading = Some(found);
                    }
                    None => {
                        both_tried = Some((
                            capture::luma_stats(&pixels),
                            capture::luma_stats(&screen_pixels),
                        ));
                    }
                }
            }
        }

        let Some(reading) = reading else {
            if !self.logged_blind {
                self.logged_blind = true;
                log_frame(&pixels);
                let candidates = font::find_candidate_boxes(&pixels, 4);
                if candidates.is_empty() {
                    log::warn!("整幅画面里没有检出任何文字横带（不在角色里？画面是别的窗口？）");
                } else {
                    let list: Vec<String> = candidates
                        .iter()
                        .map(|candidate| {
                            format!(
                                "({}, {}, {}×{}) 字高 {}",
                                candidate.rect.0,
                                candidate.rect.1,
                                candidate.rect.2,
                                candidate.rect.3,
                                candidate.text_height
                            )
                        })
                        .collect();
                    log::warn!("整幅画面检出候选横带但经验表都没自洽：{}", list.join("；"));
                }
                // 「画面里确实有字、只是奇形怪状」和「根本没有那一行」要做的事不同：
                // 前者是游戏改了字体 / 缩放，后者是人不角色。把认不出的字形点阵写下来。
                let unmatched = font::unmatched_glyphs(&pixels, &self.font);
                if !unmatched.is_empty() {
                    let list: Vec<String> = unmatched
                        .iter()
                        .take(4)
                        .map(|(x, bitmap)| format!("x={x} {bitmap}"))
                        .collect();
                    log::warn!(
                        "画面里有 {} 个字形和内置字形表对不上（游戏换了字体或缩放？）：\n{}",
                        unmatched.len(),
                        list.join("\n")
                    );
                }
            }
            let detail = match both_tried {
                Some((window_stats, screen_stats)) => format!(
                    "，而且**两条抓屏路都试过了**（后台截屏：中位 {} 亮 {:.1}%；屏幕截屏：中位 {} 亮 {:.1}%）",
                    window_stats.median,
                    window_stats.bright_ratio * 100.0,
                    screen_stats.median,
                    screen_stats.bright_ratio * 100.0
                ),
                None => "，自动定位和 OCR 都试过了".to_string(),
            };
            let detail = match hud_detail {
                Some(hud) => format!("{detail}{hud}"),
                None => detail,
            };
            let evidence = format!("{scene} · 抓法 {method:?} · 整片空白=false{detail}");
            return Err(self.note_failure(
                ReadFailure::NoFieldAt {
                    client: format!("{client_w}×{client_h}"),
                    detail,
                },
                &evidence,
            ));
        };

        self.sample_from_reading(
            hwnd,
            client_w,
            client_h,
            scene,
            reading,
            method,
            whole,
            HitSource::Search,
        )
    }

    /// 「测试读数」：拿一个还没保存的框立刻读一次，不落库、不改当前校准。
    ///
    /// 三种 kind 各有各的验收口径：
    /// * `exp_line`：经验表双数自洽（读对了必然自洽）；
    /// * `map_name`：OCR 文本要过 `looks_like_map_name`（汉字为主、字数 1~12）；
    /// * `level`：OCR 文本里能抠出 1~120 的等级数字。
    pub fn test_region(&mut self, kind: &str, rect: NormRect) -> CalibrationTest {
        let Some(hwnd) = self.ensure_window().map(|window| window.hwnd) else {
            return CalibrationTest::failed("没找到游戏窗口（游戏没开？）");
        };
        let (client_w, client_h) = capture::client_size(hwnd).unwrap_or((0, 0));
        let Some(area) = rect.to_pixel(client_w, client_h) else {
            return CalibrationTest::failed("这个框在当前客户区里放不下（改一下大小或位置）");
        };
        if !area.is_usable() {
            return CalibrationTest::failed("框太小了（至少 8×8 像素），拖动四边把它调大一点");
        }
        if capture::is_minimized(hwnd) {
            return CalibrationTest::failed("游戏窗口最小化了，看不到画面");
        }

        match kind {
            region::KIND_MAP_NAME => {
                let readings = self.ocr_region_variants(hwnd, area, client_w, client_h);
                if readings.is_empty() {
                    return CalibrationTest::failed(
                        "框内没认出文字：对准小地图上的地图名那一行，而且人站在角色里",
                    );
                }
                let Some(reading) = readings
                    .iter()
                    .find(|reading| crate::exp::ocr::looks_like_map_name(reading))
                else {
                    let best = readings
                        .iter()
                        .max_by(|a, b| a.confidence.total_cmp(&b.confidence))
                        .expect("readings 非空");
                    return CalibrationTest::failed(format!(
                        "框里读到的是「{}」（置信度 {:.2}），不像地图名 —— 重新对准那一行",
                        best.text, best.confidence
                    ));
                };
                let profile = self.manual_profile(kind, rect, area.h as u32);
                CalibrationTest::from_region_text(
                    Some(reading.text.trim().to_string()),
                    format!("认出来了：地图名「{}」", reading.text.trim()),
                    None,
                    profile,
                )
            }
            region::KIND_LEVEL => {
                let readings = self.ocr_region_variants(hwnd, area, client_w, client_h);
                if readings.is_empty() {
                    return CalibrationTest::failed(
                        "框内没认出文字：对准 HUD 上的等级数字（Lv.55 那一小段）",
                    );
                }
                let parsed = readings
                    .iter()
                    .find_map(|reading| parse_level_ocr(&reading.text).map(|level| (level, reading)));
                let Some((level, reading)) = parsed else {
                    let best = readings
                        .iter()
                        .max_by(|a, b| a.confidence.total_cmp(&b.confidence))
                        .expect("readings 非空");
                    return CalibrationTest::failed(format!(
                        "框里读到的是「{}」，抠不出 1~120 的等级数字 —— 重新对准它",
                        best.text
                    ));
                };
                let profile = self.manual_profile(kind, rect, area.h as u32);
                CalibrationTest::from_region_text(
                    Some(reading.text.trim().to_string()),
                    format!("认出来了：等级 Lv.{level}"),
                    Some(level),
                    profile,
                )
            }
            _ => {
                let text_height = area.h.max(8) as usize;
                let padded = area.expanded(region_padding(text_height, area), client_w, client_h);
                let Some((reading, _)) = self.grab_and_read(hwnd, padded) else {
                    return CalibrationTest::failed(
                        "框内没认出经验那一行：确认框套住了「Lv. 经验 (xx.xx%)」那一行，\
                         而且人站在角色里（不是登录 / 加载画面）",
                    );
                };
                let profile = NormRect::from_pixel(
                    rect_from_reading(&reading, padded.x, padded.y).clamp_to(client_w, client_h),
                    client_w,
                    client_h,
                )
                .map(|fitted| RegionProfile {
                    kind: region::KIND_EXP_LINE.to_string(),
                    rect: fitted,
                    source: RegionSource::Manual,
                    text_height: Some(reading.text_height as u32),
                    learned_unix: 0,
                });
                if table::validate(reading.exp, reading.percent) == Verdict::Contradiction {
                    return CalibrationTest::failed(format!(
                        "框里读出了「{}」，但和经验表对不上 —— 再微调一下框的位置",
                        reading.raw
                    ));
                }
                CalibrationTest::from_reading(&reading, profile)
            }
        }
    }

    /// 地图名 / 等级的「手动档」记录（框就是用户自己拖的那个，不缩不放）。
    fn manual_profile(&self, kind: &str, rect: NormRect, text_height: u32) -> RegionProfile {
        RegionProfile {
            kind: kind.to_string(),
            rect,
            source: RegionSource::Manual,
            text_height: Some(text_height),
            learned_unix: chrono::Utc::now().timestamp(),
        }
    }

    /// 「自动识别」：抓一整帧自动定位，找到就学成 auto 档（显式操作，覆盖手动档）。
    pub fn auto_calibrate(&mut self) -> CalibrationTest {
        let Some(hwnd) = self.ensure_window().map(|window| window.hwnd) else {
            return CalibrationTest::failed("没找到游戏窗口（游戏没开？）");
        };
        let (client_w, client_h) = capture::client_size(hwnd).unwrap_or((0, 0));
        if client_w <= 0 || client_h <= 0 {
            return CalibrationTest::failed("拿不到游戏客户区尺寸（窗口刚创建？）");
        }
        if capture::is_minimized(hwnd) {
            return CalibrationTest::failed("游戏窗口最小化了，看不到经验");
        }
        let Some((frame, _)) = capture::grab_client(hwnd, 0, 0, client_w, client_h) else {
            return CalibrationTest::failed("抓不到游戏窗口的画面");
        };
        let pixels = frame.pixels();
        if pixels.looks_blank() {
            return CalibrationTest::failed("抓到的画面是空白的（游戏还没画出来？再点一次）");
        }
        // 先走 HUD 锚点（任何缩放都读得出），显式点「自动识别」时不用冷却、重新定位
        let hud_read = hud::locate(&pixels).and_then(|layout| {
            self.hud_layout = Some((client_w, client_h, layout));
            self.hud_layout_miss = None;
            self.hud_misses = 0;
            hudread::read_exp(&pixels, (0, 0), &layout, &self.font).map(|read| read.reading)
        });
        let Some(reading) = hud_read.or_else(|| self.recognize(&pixels)) else {
            return CalibrationTest::failed(
                "画面里没找到经验那一行：确认在角色里（不是登录 / 加载画面），\
                 或被游戏自己的面板挡住了",
            );
        };
        if table::validate(reading.exp, reading.percent) == Verdict::Contradiction {
            return CalibrationTest::failed(format!(
                "找到了疑似经验行「{}」，但和经验表对不上，不采用",
                reading.raw
            ));
        }
        let Some(rect) = NormRect::from_pixel(
            rect_from_reading(&reading, 0, 0).clamp_to(client_w, client_h),
            client_w,
            client_h,
        ) else {
            return CalibrationTest::failed("读到的位置换算失败（客户区尺寸异常）");
        };
        let profile = RegionProfile {
            kind: region::KIND_EXP_LINE.to_string(),
            rect,
            source: RegionSource::Auto,
            text_height: Some(reading.text_height as u32),
            learned_unix: chrono::Utc::now().timestamp(),
        };
        let saved = match &self.store {
            Some(store) => match store.save_region(&profile) {
                Ok(()) => profile.clone(),
                Err(err) => return CalibrationTest::failed(format!("保存自动校准失败：{err}")),
            },
            None => profile.clone(),
        };
        // 显式「自动识别」是用户的动作：以它为准，手动档就此让位（与网页版一致）。
        self.region = Some(saved.clone());
        self.manual = None;
        self.pending = None;
        self.misses = 0;
        self.manual_misses = 0;
        self.manual_lost = false;
        self.last_failure = None;
        CalibrationTest::from_reading(&reading, Some(saved))
    }

    /// 诊断页：一次 dump 就该能分清「拍错了画面」和「那一行不在画面里」。
    ///
    /// 返回类型仍是 `Vec<String>`（`get_exp_preview` 的契约，前端按钮不用改）：
    /// **前面是判据，后面是图** —— 判据先出去，被截断也不影响定位。
    ///
    /// 结构：判据行 → 候选横带 → 两路抓图对比 → 整窗粗图 → 原图。
    /// **不依赖经验行能被认出来** —— 只要抓得到就画得出来，所以全屏、非 1080p
    /// 这些「读不出数」的现场照样能用。
    pub fn preview(&mut self) -> Result<Vec<String>, String> {
        let Some(window) = self.ensure_window() else {
            return Err("没找到游戏窗口".to_string());
        };
        let hwnd = window.hwnd;
        let title = window.title.clone();
        let (client_w, client_h) = capture::client_size(hwnd).unwrap_or((0, 0));
        let strip = capture::strip_height(client_h);
        if capture::is_minimized(hwnd) {
            return Err(format!("游戏窗口最小化了（客户区 {client_w}×{client_h}）"));
        }

        // 0) 底部条先抓一次：判据里的「空白」「抓法」和末尾那张原图用的就是它
        let Some((frame, method)) = capture::grab_strip(hwnd) else {
            return Err(format!("抓不到游戏窗口的画面（客户区 {client_w}×{client_h}）"));
        };
        let pixels = frame.pixels();
        let strip_blank = pixels.looks_blank();

        // 1) 后台截屏抓**整个客户区**：只画底部条看不出全局 ——
        //    「那一行到底在不在这张图上」「这张图到底是不是游戏」都得看整窗。
        let Some(full) = capture::grab_client_by_method(
            hwnd,
            0,
            0,
            client_w,
            client_h,
            Method::Window,
        ) else {
            return Err(format!("后台截屏抓不到整个客户区（{client_w}×{client_h}）"));
        };
        let full_pixels = full.pixels();
        let stats_window = capture::luma_stats(&full_pixels);
        let grid_window = capture::coarse_grid(&full_pixels, COARSE_COLS, COARSE_ROWS);

        // 2) 同一块区域再用屏幕 DC 抓一次，**只在游戏确实在前台时才做**：
        //    屏幕上那块此刻显示的是谁我们就抓到谁，被自己的窗口盖住时，
        //    对比出来的“不同”是假的 —— 宁可跳过，也不把结论带偏。
        let comparison = if !capture::is_foreground(hwnd) {
            "两路对比：跳过（游戏不在前台，屏幕那块是别的窗口）—— 把游戏切到最前面再点一次才有结论"
                .to_string()
        } else {
            match capture::grab_client_by_method(hwnd, 0, 0, client_w, client_h, Method::Screen) {
                Some(screen_frame) => {
                    let screen_pixels = screen_frame.pixels();
                    let stats_screen = capture::luma_stats(&screen_pixels);
                    let grid_screen =
                        capture::coarse_grid(&screen_pixels, COARSE_COLS, COARSE_ROWS);
                    let same = capture::grid_similarity(
                        &grid_window,
                        &grid_screen,
                        GRID_TOLERANCE,
                    );
                    let verdict = if same >= AGREE_RATIO {
                        "两份一致 → 拍到的确实是屏幕上的画面，问题不在 PrintWindow（看候选横带与行标记）"
                    } else {
                        "两份对不上 → PrintWindow 给的很可能不是这个游戏画面（无边框 / 独占全屏的典型症状）"
                    };
                    format!(
                        "两路对比：相同率 {:.0}%（后台截屏 min/中位/max={}/{}/{} 亮 {:.1}% · 屏幕截屏 min/中位/max={}/{}/{} 亮 {:.1}%）｜ {}",
                        same * 100.0,
                        stats_window.min,
                        stats_window.median,
                        stats_window.max,
                        stats_window.bright_ratio * 100.0,
                        stats_screen.min,
                        stats_screen.median,
                        stats_screen.max,
                        stats_screen.bright_ratio * 100.0,
                        verdict
                    )
                }
                None => "两路对比：屏幕 DC 这次没抓到（坐标 / DC 失败）".to_string(),
            }
        };

        // 3) 判据行（先出去，被截断也带得走）
        let shape = if stats_window.min == stats_window.max {
            "一片纯色（拍错了东西）"
        } else if stats_window.bright_ratio > 0.5 {
            "亮底暗字"
        } else {
            "暗底亮字"
        };
        let (strip_from, strip_to) = coarse_rows_in_strip(client_h, strip, COARSE_ROWS);
        let candidates = font::find_candidate_boxes(&full_pixels, 5);
        let candidate_line = if candidates.is_empty() {
            "候选横带：整幅画面未检出连续文字横带".to_string()
        } else {
            let list: Vec<String> = candidates
                .iter()
                .map(|candidate| {
                    format!(
                        "(x={}, y={}, {}×{}, 字高 {})",
                        candidate.rect.0,
                        candidate.rect.1,
                        candidate.rect.2,
                        candidate.rect.3,
                        candidate.text_height
                    )
                })
                .collect();
            format!("候选横带（从高到低）：{}", list.join(" · "))
        };
        let region_line = match self.calibration() {
            Some(profile) => {
                let pixel = profile
                    .rect
                    .to_pixel(client_w, client_h)
                    .map(|rect| format!("(x={}, y={}, {}×{})", rect.x, rect.y, rect.w, rect.h))
                    .unwrap_or_else(|| "（换算失败）".to_string());
                format!(
                    "校准框：{} 档 · 客户区 {} · 字高 {}",
                    profile.source.as_str(),
                    pixel,
                    profile
                        .text_height
                        .map(|height| format!("{height}px"))
                        .unwrap_or_else(|| "?".to_string())
                )
            }
            None => "校准框：无（整幅画面自动定位）".to_string(),
        };
        let mut lines = vec![
            format!(
                "判据：客户区 {client_w}×{client_h} · 条高 {strip} 行 · 形态={} · 底部条抓法 {method:?} · 条空白={} · 前台={}",
                capture::screen_mode(hwnd).label(),
                strip_blank,
                capture::is_foreground(hwnd)
            ),
            region_line,
            candidate_line,
            format!(
                "窗口：标题={:?} · 类名={}",
                title,
                capture::window_class_name(hwnd)
            ),
            format!(
                "亮度：min {} · 中位 {} · max {} · >{} 占 {:.1}%（{}）",
                stats_window.min,
                stats_window.median,
                stats_window.max,
                font::INK_THRESHOLD,
                stats_window.bright_ratio * 100.0,
                shape
            ),
            comparison,
            "怎么读：Win32 分不出「独占全屏」和「无边框全屏」（两者样式与尺寸一模一样），\
             所以这一档由上面那行的对比结果实证 —— 两份对不上就是 PrintWindow 没拿到游戏\
             画面（独占全屏的典型症状，该改窗口模式）；两份一致就去看候选横带"
                .to_string(),
            format!(
                "行标记：底部条 = 原始第 {}~{} 行 → 粗图第 {}~{} 行（行首 *）",
                (client_h - strip).max(0),
                client_h,
                strip_from + 1,
                strip_to
            ),
            format!("整窗粗图（{COARSE_COLS}×{COARSE_ROWS}，每格取块内最大亮度，防抹平小字号）："),
        ];

        // 4) 粗图本体
        for (index, row) in grid_window.iter().enumerate() {
            let marker = if index >= strip_from && index < strip_to {
                '*'
            } else {
                ' '
            };
            lines.push(format!(
                "{marker}{}",
                row.iter()
                    .map(|&value| match value {
                        _ if value > font::INK_THRESHOLD => '#',
                        150..=255 => '+',
                        80..=149 => '.',
                        _ => ' ',
                    })
                    .collect::<String>()
            ));
        }

        // 5) 底部条原图（原分辨率，用来认字形）
        lines.push(format!("底部条原图（每 2 列取一列，共 {} 行）：", pixels.height));
        lines.extend((0..pixels.height).map(|y| {
            (0..pixels.width)
                .step_by(2)
                .map(|x| {
                    if pixels.luma(x, y) > font::INK_THRESHOLD {
                        '#'
                    } else {
                        '.'
                    }
                })
                .collect::<String>()
        }));
        Ok(lines)
    }
}

/// 把一帧画面画成 ASCII（日志用，每次运行最多写一次）。
///
/// 没有这一条，「读不到」这类问题只能靠猜：是没找到窗口、是空白图、
/// 还是画面里真的没有那一行 —— 三种情况的修法完全不同。
pub fn log_frame(pixels: &Pixels<'_>) {
    let step = (pixels.width / 160).max(1);
    let mut lines = Vec::with_capacity(pixels.height);
    for y in 0..pixels.height {
        let line: String = (0..pixels.width)
            .step_by(step)
            .map(|x| match pixels.luma(x, y) {
                200..=255 => '#',
                150..=199 => '+',
                80..=149 => '.',
                _ => ' ',
            })
            .collect();
        lines.push(line);
    }
    log::info!(
        "读不到经验那一行。这一帧的画面（每 {} 列取一列）：\n{}",
        step,
        lines.join("\n")
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 真机全屏素材（2560×1440，缩放 1.872）那一条。
    fn fullscreen_strip() -> Vec<u8> {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/exp_strip_fullscreen_2560x1440.bgra");
        std::fs::read(&path).expect("读不到真机全屏素材")
    }

    /// **手动框兜底**：手动框 / 自动框走的是通用识别，全屏拉伸下它也必须读对
    /// （以前只有点阵 + OCR，全屏下照样时好时坏）。
    #[test]
    fn a_calibrated_box_reads_the_real_fullscreen_capture() {
        let strip = fullscreen_strip();
        let pixels = Pixels {
            width: 333,
            height: 76,
            bgra: &strip,
        };
        let mut reader = ExpReader::new(Font::builtin());
        let reading = reader.recognize(&pixels).expect("框里必须读出");
        assert_eq!((reading.exp, reading.percent), (16_677, 68.34), "读成了 {}", reading.raw);
    }

    /// **整幅搜索兜底**：HUD 锚点没定位到时，在一大幅画面里找到那一行并读对。
    #[test]
    fn the_whole_frame_search_reads_the_real_fullscreen_capture() {
        let strip = fullscreen_strip();
        let (width, height) = (1200usize, 500usize);
        let mut frame = vec![0u8; width * height * 4];
        for (i, px) in frame.chunks_mut(4).enumerate() {
            let v = (30 + (i % width) * 40 / width) as u8;
            px.copy_from_slice(&[v, v + 5, v, 255]);
        }
        let (ox, oy) = (700usize, 380usize);
        for y in 0..76 {
            let src = y * 333 * 4;
            let dst = ((oy + y) * width + ox) * 4;
            frame[dst..dst + 333 * 4].copy_from_slice(&strip[src..src + 333 * 4]);
        }
        let pixels = Pixels {
            width,
            height,
            bgra: &frame,
        };
        let mut reader = ExpReader::new(Font::builtin());
        let reading = reader.recognize(&pixels).expect("整幅里必须找到并读出");
        assert_eq!((reading.exp, reading.percent), (16_677, 68.34), "读成了 {}", reading.raw);
        let (x0, y0, _, _) = reading.rect;
        assert!(x0 >= ox && y0 >= oy, "读数位置应在贴图处：{:?}", reading.rect);
    }

    /// 真机自检：**不把游戏切到前台**，直接读 —— 这正是旧实现做不到的事。
    ///
    /// ```text
    /// cd src-tauri && cargo test --lib -- --ignored reader_reads_without_foreground --nocapture
    /// ```
    #[test]
    #[ignore = "需要游戏正在运行"]
    fn reader_reads_without_foreground() {
        capture::ensure_dpi_aware();
        let mut reader = ExpReader::new(Font::builtin());
        let mut ok = 0;
        let mut total = 0;
        for _ in 0..10 {
            total += 1;
            match reader.read() {
                Ok(sample) => {
                    ok += 1;
                    println!(
                        "读到 {} → {} / {}%（等级 {:?}，抓法 {:?}）",
                        sample.raw, sample.exp, sample.percent, sample.level, sample.method
                    );
                    assert!(sample.exp > 0);
                    assert_ne!(
                        table::validate(sample.exp, sample.percent),
                        Verdict::Contradiction
                    );
                }
                Err(failure) => println!("没读到（{}）：{}", failure.code(), failure.message()),
            }
            capture::settle();
        }
        println!("成功率 {ok}/{total}");
        assert!(ok * 2 >= total, "成功率不到一半 —— 抓屏或字形表有问题");
    }

    /// 没找到窗口时要老实报 `no_window`，而不是别的。
    #[test]
    fn a_missing_window_says_so() {
        let failure = ReadFailure::NoWindow;
        assert_eq!(failure.code(), "no_window");
        assert!(failure.message().contains("没找到游戏窗口"));
    }

    /// 失败信息里**不许**再出现「被别的窗口挡住 / 去校准」这类话 ——
    /// 后台截屏根本不怕遮挡，看到那句会把人引向错的方向。
    #[test]
    fn failure_messages_never_blame_occlusion() {
        for failure in [
            ReadFailure::NoWindow,
            ReadFailure::Minimized,
            ReadFailure::BlankFrame {
                mode: capture::ScreenMode::Windowed,
            },
            ReadFailure::BlankFrame {
                mode: capture::ScreenMode::Framed,
            },
            ReadFailure::BlankFrame {
                mode: capture::ScreenMode::Borderless,
            },
            ReadFailure::NoFieldAt {
                client: "1366×768".to_string(),
                detail: String::new(),
            },
            ReadFailure::CaptureFailed("x".to_string()),
            ReadFailure::Contradiction {
                raw: "1(0.01%)".to_string(),
                exp: 1,
                percent: 0.01,
            },
        ] {
            let message = failure.message();
            assert!(!message.contains("挡住"), "{message}");
            assert!(!message.contains("屏幕外"), "{message}");
        }
    }

    /// 同一个 `blank_frame`，**窗口形态不同 → 该做的事不同**：
    /// 只有「占满且无边框」那档才谈得上“改成窗口模式”（那里面可能藏着独占全屏），
    /// 普通窗口 / 最大化窗口黑了就是「还没画出来」，叫人去改模式只会白折腾。
    #[test]
    fn blank_frame_gives_advice_matching_the_window_shape() {
        let borderless = ReadFailure::BlankFrame {
            mode: capture::ScreenMode::Borderless,
        }
        .message();
        let windowed = ReadFailure::BlankFrame {
            mode: capture::ScreenMode::Windowed,
        }
        .message();
        let framed = ReadFailure::BlankFrame {
            mode: capture::ScreenMode::Framed,
        }
        .message();

        assert_ne!(borderless, windowed, "两种现场必须说不同的话");
        assert_ne!(windowed, framed, "带框占满和普通窗口也得分开说");

        // 无边框全屏：把独占这个可能性和「改成窗口」这个动作都说清楚
        assert!(borderless.contains("无边框"), "{borderless}");
        assert!(borderless.contains("独占"), "{borderless}");
        assert!(borderless.contains("窗口」模式"), "{borderless}");

        // 普通窗口：只说等一两帧，**不许**叫用户去改全屏模式
        assert!(windowed.contains("还没画出来"), "{windowed}");
        assert!(!windowed.contains("独占"), "{windowed}");
        assert!(!windowed.contains("切成"), "{windowed}");
        assert!(!framed.contains("独占"), "{framed}");

        // 都是 blank_frame，前端靠 code 分类不受影响
        assert_eq!(
            ReadFailure::BlankFrame {
                mode: capture::ScreenMode::Borderless
            }
            .code(),
            "blank_frame"
        );
    }

    /// NoField 的文案要**带上我们实际看到的东西**：客户区尺寸，以及试过哪些路。
    #[test]
    fn no_field_message_carries_the_client_size_and_detail() {
        let message = ReadFailure::NoFieldAt {
            client: "1366×768".to_string(),
            detail: "，自动定位和 OCR 都试过了".to_string(),
        }
        .message();
        assert!(message.contains("1366×768"), "{message}");
        assert!(message.contains("没有经验那一行"), "{message}");
        assert!(message.contains("自动定位"), "{message}");
        assert!(message.contains("校准位置"), "{message}");
    }

    /// 行标记：底部条必须落在粗图**最底下那几行**，且覆盖到最后一行 ——
    /// 这行标记就是回答「经验那一行该在图上哪儿」的那把尺子。
    #[test]
    fn coarse_rows_in_strip_marks_the_bottom_band() {
        // 1440p、条高 144（10%）：条从原始 1296 行开始 → 粗图第 41~45 行（半开 40..45）
        assert_eq!(coarse_rows_in_strip(1440, 144, 45), (40, 45));
        // 1080p 同理
        assert_eq!(coarse_rows_in_strip(1080, 108, 45), (40, 45));
        // 条占满整屏（窗口比条还矮）：整张图都该标
        assert_eq!(coarse_rows_in_strip(100, 100, 45), (0, 45));
        // 拿不到尺寸 / 条高：不标，也不越界
        assert_eq!(coarse_rows_in_strip(0, 0, 45), (0, 0));
        assert_eq!(coarse_rows_in_strip(1440, 0, 45), (0, 0));
    }

    /// 读到的框要按**比例**存：同一个读数在 1080p / 1440p / 4K 下归一化后一致。
    #[test]
    fn a_found_reading_becomes_a_resolution_independent_rect() {
        let reading = font::Reading {
            exp: 427_096,
            percent: 46.09,
            raw: "427096(46.09%)".to_string(),
            rect: (100, 30, 200, 40),
            exact: 10,
            glyphs: 12,
            text_height: 10,
        };
        let pixels = rect_from_reading(&reading, 0, 0);
        let norm = NormRect::from_pixel(pixels, 1920, 1080).expect("应能换算");
        assert!(norm.is_valid());
        // 同一个读数在另一分辨率上：按比例映射回去，语义上仍是右下角那一行
        let at_1440 = norm.to_pixel(2560, 1440).expect("应能映射");
        assert!(at_1440.x > 0 && at_1440.y > 0);
        assert!(at_1440.x + at_1440.w <= 2560);
        assert!(at_1440.y + at_1440.h <= 1440);
    }

    /// 「框读不到」这件事要分档：自动档连续两帧就重找；手动档只标记失效，不丢。
    #[test]
    fn a_broken_automatic_region_is_dropped_but_a_manual_one_is_only_flagged() {        let mut reader = ExpReader::new(Font::builtin());
        reader.region = Some(RegionProfile {
            kind: region::KIND_EXP_LINE.to_string(),
            rect: NormRect::new(0.1, 0.1, 0.2, 0.05).unwrap(),
            source: RegionSource::Auto,
            text_height: Some(7),
            learned_unix: 0,
        });
        reader.note_region_miss();
        assert!(reader.region.is_some(), "一帧失败先不丢");
        reader.note_region_miss();
        assert!(reader.region.is_none(), "自动档连续两帧失败要重找");

        reader.region = Some(RegionProfile {
            kind: region::KIND_EXP_LINE.to_string(),
            rect: NormRect::new(0.1, 0.1, 0.2, 0.05).unwrap(),
            source: RegionSource::Manual,
            text_height: Some(7),
            learned_unix: 0,
        });
        for _ in 0..MANUAL_MISS_LIMIT {
            reader.note_region_miss();
        }
        assert!(reader.region.is_some(), "手动档不许被自动丢掉");
        assert!(reader.manual_lost(), "但要标记「可能失效」给界面");
    }

    /// 自动定位连续两帧位置一致才落库；只出现一次不写库。
    #[test]
    fn an_automatic_region_needs_two_stable_frames_before_it_is_saved() {
        use parking_lot::Mutex;
        use std::sync::Arc;

        struct MemStore(Mutex<Option<RegionProfile>>);
        impl RegionStore for MemStore {
            fn load_region(&self, _kind: &str) -> Option<RegionProfile> {
                self.0.lock().clone()
            }
            fn save_region(&self, profile: &RegionProfile) -> Result<(), String> {
                *self.0.lock() = Some(profile.clone());
                Ok(())
            }
            fn clear_region(&self, _kind: &str) -> Result<(), String> {
                *self.0.lock() = None;
                Ok(())
            }
        }

        let store = Arc::new(MemStore(Mutex::new(None)));
        let mut reader = ExpReader::with_store(Font::builtin(), Some(store.clone()));
        let rect = NormRect::new(0.1, 0.9, 0.2, 0.02).unwrap();
        reader.note_auto_found(rect, 7);
        assert!(store.load_region(region::KIND_EXP_LINE).is_none(), "第一帧不落库");
        // 第二帧位置一致 → 落库
        reader.confirm_pending(rect);
        assert!(store.load_region(region::KIND_EXP_LINE).is_some(), "第二帧应落库");
    }

    /// 手动档在场时，自动定位**不许**覆盖它。
    #[test]
    fn an_automatic_region_never_overwrites_a_manual_one() {
        use parking_lot::Mutex;
        use std::sync::Arc;

        struct MemStore(Mutex<Option<RegionProfile>>);
        impl RegionStore for MemStore {
            fn load_region(&self, _kind: &str) -> Option<RegionProfile> {
                self.0.lock().clone()
            }
            fn save_region(&self, profile: &RegionProfile) -> Result<(), String> {
                *self.0.lock() = Some(profile.clone());
                Ok(())
            }
            fn clear_region(&self, _kind: &str) -> Result<(), String> {
                *self.0.lock() = None;
                Ok(())
            }
        }

        let manual = RegionProfile {
            kind: region::KIND_EXP_LINE.to_string(),
            rect: NormRect::new(0.4, 0.9, 0.2, 0.02).unwrap(),
            source: RegionSource::Manual,
            text_height: Some(7),
            learned_unix: 123,
        };
        let store = Arc::new(MemStore(Mutex::new(Some(manual.clone()))));
        let mut reader = ExpReader::with_store(Font::builtin(), Some(store.clone()));
        reader.manual = Some(manual.clone());
        reader.region = Some(manual.clone());
        let rect = NormRect::new(0.1, 0.9, 0.2, 0.02).unwrap();
        reader.note_auto_found(rect, 7);
        reader.confirm_pending(rect);
        assert_eq!(
            store.load_region(region::KIND_EXP_LINE).unwrap().source,
            RegionSource::Manual,
            "自动定位不许覆盖手动档"
        );
        assert!(
            reader.manual_lost(),
            "手动框读不到、自动定位在别处读到了，要如实标记「校准可能失效」"
        );
    }

    /// 手动框没读到时，自动定位在同一位置读出来（交并比高）→ 不算失效。
    #[test]
    fn an_automatic_region_at_the_manual_position_does_not_flag_it() {
        let mut reader = ExpReader::new(Font::builtin());
        let rect = NormRect::new(0.4, 0.9, 0.2, 0.02).unwrap();
        reader.manual = Some(RegionProfile {
            kind: region::KIND_EXP_LINE.to_string(),
            rect,
            source: RegionSource::Manual,
            text_height: Some(7),
            learned_unix: 123,
        });
        reader.note_auto_found(rect, 7);
        assert!(!reader.manual_lost(), "同一位置不该被标记失效");
    }

    /// 等级 OCR 解析：`Lv.55` / `LV 55` / 形近字纠正；越界的一律当没读到。
    #[test]
    fn level_ocr_parses_the_hud_number_and_rejects_noise() {
        assert_eq!(parse_level_ocr("Lv.55"), Some(55));
        assert_eq!(parse_level_ocr("LV 55"), Some(55));
        assert_eq!(parse_level_ocr("55"), Some(55));
        assert_eq!(parse_level_ocr("Lv.l20"), Some(120));
        assert_eq!(parse_level_ocr("Lv.O9"), Some(9));
        assert_eq!(parse_level_ocr("Lv.1Z0"), Some(120));
        // 把上下相邻的行（经验数字）一起裁进来时，数字串太长 → 如实判没读到。
        // 这正是「框得越准反而认不出」的现场：给 OCR 的裁剪加了多余留白。
        assert_eq!(parse_level_ocr("Lv.55 EXP 333753(75.19%)"), None);
        // 越界 / 没有数字：宁可当没读到
        assert_eq!(parse_level_ocr("Lv.121"), None);
        assert_eq!(parse_level_ocr("Lv.0"), None);
        assert_eq!(parse_level_ocr("Lv."), None);
        assert_eq!(parse_level_ocr("经验"), None);
    }

    /// 标签看得到却一直读不成：到数就要求重新定位，而且越试越稀。
    #[test]
    fn an_anchor_that_never_reads_is_given_up_with_backoff() {
        let mut reader = ExpReader::new(Font::builtin());
        for _ in 0..HUD_UNREAD_LIMIT - 1 {
            reader.note_hud_unread();
        }
        assert_eq!(reader.hud_unread, HUD_UNREAD_LIMIT - 1);
        reader.note_hud_unread();
        assert_eq!(reader.hud_unread, 0);
        assert_eq!(reader.hud_unread_limit, HUD_UNREAD_LIMIT * 2);
        for _ in 0..1000 {
            reader.note_hud_unread();
        }
        assert_eq!(reader.hud_unread_limit, HUD_UNREAD_LIMIT_MAX);
    }

    /// 三种 kind 都能被校准表接受（命令层用同一份判据）。
    #[test]
    fn all_three_calibration_kinds_are_valid() {
        for kind in [
            region::KIND_EXP_LINE,
            region::KIND_MAP_NAME,
            region::KIND_LEVEL,
        ] {
            assert!(region::is_valid_kind(kind), "{kind} 应该是合法类型");
        }
        assert!(!region::is_valid_kind("gold"));
    }
}
