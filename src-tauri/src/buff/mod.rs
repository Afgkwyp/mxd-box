//! BUFF 到期提醒 —— 盯着游戏画面顶部的 BUFF 图标，快结束时念一声「XX 即将结束」。
//!
//! 照网页版「枫记」（fj.need.run）的 BUFF 提醒移植：图标匹配、读倒计时数字、
//! 用多次读数钉住终点、一轮只提醒一次，这几步的做法和门槛都沿用它（各文件开头有出处）。
//!
//! # 和网页版不一样的地方
//!
//! * 网页版从它自己的技能 / 道具图库里挑图标（私有接口）；我们的图标有两个来源：
//!   - **内置技能图标**（`catalog`，来自小册子技能图鉴，打包在程序里）—— 直接挑；
//!   - 图鉴里没有的（药水、道具 BUFF），在游戏截图上**框一下**。
//! * 游戏界面可能被整体拉伸（全屏时按内部分辨率放大，比如 1366×768 拉到 2560×1440 是 1.872 倍）。
//!   所以画面先按**界面倍数**缩回原生像素（见 `scale.rs`），图标模板都是原生大小，
//!   框下来的图标也存原生像素。倍数优先用经验模块 HUD 锚点量出来的，没有就轮流试几个常见分辨率。
//! * 画面直接从游戏窗口抓（`exp::capture`，和经验统计同一套），不用浏览器共享屏幕。
//! * 只看客户区**上面 30%**（BUFF 栏所在的位置，网页版默认的识别区也是这块）。
//!
//! # 循环
//!
//! 一条线程，每 0.5 秒：抓顶部那条 → 按界面倍数缩回原生 → 每个 BUFF 先在上次的位置附近找（很快），
//! 没有上次位置的才整条搜（慢一些，每帧最多搜 3 个）→ 读左下角的秒数 → 喂给
//! `timer::BuffTimer` → 该提醒就念。游戏没开 / 最小化时暂停，不抓画面。
//! 没有启用的 BUFF 时什么都不抓。
//!
//! 设置整份存成 JSON（`buff_config`），图标是 PNG 的 base64，跟着设置一起存。

mod catalog;
mod digits;
mod image;
mod matcher;
#[cfg(test)]
mod probe;
mod scale;
mod speech;
mod timer;

use crate::commands::AppState;
use crate::exp::capture::{self, Frame};
use base64::Engine;
use image::Gray;
use matcher::{Hit, IconTemplate};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager, State};
use timer::{BuffTimer, Presence, Reminder};

pub const CONFIG_KEY: &str = "buff_config";
pub const STATUS_EVENT: &str = "buff-status";
/// 抓画面的间隔。
const TICK: Duration = Duration::from_millis(500);
/// 识别区：客户区上面这么高的比例。
const BAND: f64 = 0.3;
/// 识别区亮度标准差到这个值以上才算「画面有内容」（黑屏、纯色加载画面达不到）。
const BAND_CONTENT_STDDEV: f64 = 12.0;
/// 每帧最多整条搜几个 BUFF（整条搜比较慢）。
const DISCOVERIES_PER_TICK: usize = 3;
/// 框图标的尺寸范围（像素）。
const MIN_ICON: u32 = 12;
const MAX_ICON: u32 = 128;
/// 最多几个 BUFF。
const MAX_ITEMS: usize = 12;

fn default_true() -> bool {
    true
}
fn default_threshold() -> u32 {
    10
}
fn default_volume() -> u8 {
    100
}
fn default_rate() -> i8 {
    2
}

/// 一个要盯的 BUFF。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BuffItem {
    pub id: String,
    pub name: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// 剩几秒时提醒（1–59）
    #[serde(default = "default_threshold")]
    pub threshold: u32,
    /// 念出来（关掉就只响提示音）
    #[serde(default = "default_true")]
    pub speech: bool,
    /// 图标消失时再提醒一次「已结束」
    #[serde(default)]
    pub expire_alert: bool,
    /// 图标（PNG，base64）：框下来的，或内置技能图标
    pub icon_png: String,
    pub width: u32,
    pub height: u32,
    /// 框图标时游戏客户区的高度（老版本框的图标靠它猜放大倍数）。
    /// 内置技能图标是 0。
    pub client_h: u32,
    /// 图标已经是原生（1 倍）像素：内置图标、以及知道倍数时框下来的
    #[serde(default)]
    pub native: bool,
    /// 来自内置技能图标时的技能 ID
    #[serde(default)]
    pub skill_id: Option<String>,
}

impl BuffItem {
    /// 老版本框的图标没记倍数，按当时的客户区高度猜；认不出就重新框一次。
    fn legacy_scale(&self) -> f32 {
        if self.client_h <= 1080 {
            1.0
        } else {
            self.client_h as f32 / 768.0
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BuffConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_volume")]
    pub volume: u8,
    #[serde(default = "default_rate")]
    pub rate: i8,
    #[serde(default)]
    pub items: Vec<BuffItem>,
}

impl Default for BuffConfig {
    fn default() -> Self {
        Self { enabled: false, volume: default_volume(), rate: default_rate(), items: Vec::new() }
    }
}

impl BuffConfig {
    fn validate(&mut self) -> Result<(), String> {
        self.volume = self.volume.min(100);
        self.rate = self.rate.clamp(-10, 10);
        if self.items.len() > MAX_ITEMS {
            return Err(format!("最多盯 {MAX_ITEMS} 个 BUFF"));
        }
        for item in &mut self.items {
            item.name = item.name.trim().chars().take(20).collect();
            if item.name.is_empty() {
                return Err("BUFF 名字不能空着（提醒要念出来）".to_string());
            }
            if !(1..=59).contains(&item.threshold) {
                return Err(format!("「{}」的提醒秒数要在 1–59 之间", item.name));
            }
        }
        Ok(())
    }
}

pub fn load_config(db: &crate::db::Database) -> BuffConfig {
    db.get_setting(CONFIG_KEY)
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default()
}

fn store_config(db: &crate::db::Database, config: &BuffConfig) -> Result<(), String> {
    let raw = serde_json::to_string(config).map_err(|err| format!("BUFF 设置序列化失败：{err}"))?;
    db.set_setting(CONFIG_KEY, &raw).map_err(|err| format!("保存 BUFF 设置失败：{err}"))
}

/// 推给界面的状态。
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct BuffItemStatus {
    pub id: String,
    pub name: String,
    pub presence: Presence,
    /// 剩余秒数（显示用；`None` = 不知道）
    pub seconds: Option<u32>,
    /// 还在白字分钟阶段时的分钟数（「剩 N 分多」）
    pub minutes: Option<u32>,
    /// 终点钉住了（提醒靠的就是它）
    pub established: bool,
    /// 已经进入提醒阈值
    pub nearing: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Default)]
pub struct BuffStatus {
    /// 识别循环在干活
    pub running: bool,
    /// 一句话说明现在的情况（没开 / 没找到游戏 / 在盯几个）
    pub note: String,
    pub items: Vec<BuffItemStatus>,
}

/// 循环和命令之间共享的东西。
struct Shared {
    config: Mutex<BuffConfig>,
    /// 设置每改一次 +1，循环据此重建模板
    version: AtomicU64,
    status: Mutex<BuffStatus>,
    /// 最近一次给「框图标」抓的画面（原生分辨率的顶部那条）和当时的客户区高度
    snapshot: Mutex<Option<(Frame, u32)>>,
    /// 经验模块 HUD 锚点量出的界面倍数：(客户区宽, 高, 倍数)
    hud_scale: Mutex<Option<(i32, i32, f32)>>,
    /// 识别循环认定的界面倍数：(客户区宽, 高, 倍数)；还在搜的时候是 None
    band_scale: Mutex<Option<(i32, i32, (f32, f32))>>,
}

fn shared() -> &'static Shared {
    static SHARED: OnceLock<Shared> = OnceLock::new();
    SHARED.get_or_init(|| Shared {
        config: Mutex::new(BuffConfig::default()),
        version: AtomicU64::new(0),
        status: Mutex::new(BuffStatus::default()),
        snapshot: Mutex::new(None),
        hud_scale: Mutex::new(None),
        band_scale: Mutex::new(None),
    })
}

fn set_config(config: BuffConfig) {
    let shared = shared();
    *shared.config.lock().unwrap_or_else(|e| e.into_inner()) = config;
    shared.version.fetch_add(1, Ordering::SeqCst);
}

/// 经验模块 HUD 锚点量出的界面倍数，给 BUFF 用。
pub fn note_hud_scale(client_w: i32, client_h: i32, scale: f64) {
    *shared().hud_scale.lock().unwrap_or_else(|e| e.into_inner()) =
        Some((client_w, client_h, scale as f32));
}

/// 经验模块 HUD 锚点量出的倍数（客户区尺寸对得上才算）。
fn hud_scale_at(client_w: i32, client_h: i32) -> Option<f32> {
    shared()
        .hud_scale
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .filter(|&(w, h, _)| w == client_w && h == client_h)
        .map(|(_, _, s)| s)
}

/// 框图标时用的倍数：识别循环认定的优先，其次是 HUD 锚点的。
fn ui_scale_at(client_w: i32, client_h: i32) -> Option<(f32, f32)> {
    shared()
        .band_scale
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .filter(|&(w, h, _)| w == client_w && h == client_h)
        .map(|(_, _, s)| s)
        .or_else(|| hud_scale_at(client_w, client_h).map(|m| scale::snap(client_w as u32, client_h as u32, m)))
}

// ---------------------------------------------------------------------------
// PNG 进出
// ---------------------------------------------------------------------------

fn encode_png(rgba: &[u8], width: u32, height: u32) -> Result<String, String> {
    let mut bytes = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut bytes, width, height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().map_err(|err| format!("编码图标失败：{err}"))?;
        writer.write_image_data(rgba).map_err(|err| format!("编码图标失败：{err}"))?;
    }
    Ok(base64::engine::general_purpose::STANDARD.encode(bytes))
}

/// base64 PNG → (RGBA, 宽, 高)。
fn decode_png(b64: &str) -> Option<(Vec<u8>, usize, usize)> {
    let bytes = base64::engine::general_purpose::STANDARD.decode(b64).ok()?;
    decode_png_bytes(bytes)
}

/// PNG 文件的字节 → (RGBA, 宽, 高)。
fn decode_png_bytes(bytes: Vec<u8>) -> Option<(Vec<u8>, usize, usize)> {
    let mut decoder = png::Decoder::new(std::io::Cursor::new(bytes));
    decoder.set_transformations(png::Transformations::normalize_to_color8());
    let mut reader = decoder.read_info().ok()?;
    let mut buf = vec![0; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buf).ok()?;
    let (w, h) = (info.width as usize, info.height as usize);
    let rgba = match info.color_type {
        png::ColorType::Rgba => buf[..w * h * 4].to_vec(),
        png::ColorType::Rgb => buf[..w * h * 3]
            .chunks_exact(3)
            .flat_map(|px| [px[0], px[1], px[2], 255])
            .collect(),
        _ => return None,
    };
    Some((rgba, w, h))
}

/// BGRA 画面的一块 → RGBA。
fn crop_rgba(frame: &Frame, x: usize, y: usize, w: usize, h: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(w * h * 4);
    for yy in y..y + h {
        for xx in x..x + w {
            let i = (yy * frame.width + xx) * 4;
            out.extend_from_slice(&[frame.bgra[i + 2], frame.bgra[i + 1], frame.bgra[i], 255]);
        }
    }
    out
}

// ---------------------------------------------------------------------------
// 识别循环
// ---------------------------------------------------------------------------

/// 一个 BUFF 在循环里的状态。
struct Tracked {
    item: BuffItem,
    /// 原生大小的模板（建一次）
    template: IconTemplate,
    last_hit: Option<Hit>,
    timer: BuffTimer,
}

impl Tracked {
    fn new(item: BuffItem) -> Option<Self> {
        let (rgba, w, h) = decode_png(&item.icon_png)?;
        let (gray, opaque) = Gray::from_rgba(&rgba, w, h);
        // 内置图标和存了原生像素的框图标直接用；老版本框的按猜的倍数缩回原生
        let template = (if item.client_h == 0 || item.native {
            IconTemplate::new(&gray, &opaque)
        } else {
            IconTemplate::scaled(&gray, &opaque, 1.0 / item.legacy_scale())
        })?;
        Some(Self { item, template, last_hit: None, timer: BuffTimer::default() })
    }
}

fn status_of(tracked: &[Tracked], now: f64, running: bool, note: String) -> BuffStatus {
    BuffStatus {
        running,
        note,
        items: tracked
            .iter()
            .map(|t| BuffItemStatus {
                id: t.item.id.clone(),
                name: t.item.name.clone(),
                presence: t.timer.presence,
                seconds: t.timer.display_seconds(now),
                minutes: t.timer.display_minutes(now),
                established: t.timer.established(),
                nearing: t
                    .timer
                    .remaining_upper(now)
                    .is_some_and(|upper| upper <= t.item.threshold as f64),
            })
            .collect(),
    }
}

fn publish(app: &AppHandle, status: BuffStatus) {
    let mut current = shared().status.lock().unwrap_or_else(|e| e.into_inner());
    if *current == status {
        return;
    }
    *current = status.clone();
    drop(current);
    let _ = app.emit(STATUS_EVENT, &status);
}

fn announce(item: &BuffItem, reminder: Reminder, config: &BuffConfig) {
    let text = match reminder {
        Reminder::Nearing(_) => format!("{}即将结束", item.name),
        Reminder::Expired => format!("{}已结束", item.name),
    };
    log::info!("BUFF 提醒：{text}");
    if item.speech {
        speech::speak(&text, config.volume, config.rate);
    } else {
        crate::alert::play_alert_sound();
    }
}

/// 客户区尺寸或界面倍数变了：按旧画面记的位置和读数全部作废（提醒记录保留，不重复念）。
fn forget(tracked: &mut [Tracked]) {
    for t in tracked {
        t.last_hit = None;
        t.timer.forget_readings();
    }
}

/// 识别区里有没有内容：亮度的标准差够大才算（纯色的黑屏、加载画面都算空）。隔几个像素取一个样。
fn band_has_content(frame: &Frame) -> bool {
    let greens: Vec<f64> = frame.bgra.chunks_exact(4).step_by(7).map(|p| p[1] as f64).collect();
    if greens.is_empty() {
        return false;
    }
    let mean = greens.iter().sum::<f64>() / greens.len() as f64;
    let var = greens.iter().map(|g| (g - mean) * (g - mean)).sum::<f64>() / greens.len() as f64;
    var.sqrt() >= BAND_CONTENT_STDDEV
}

/// 这一帧里有没有哪个 BUFF 认得出来（最多看 `DISCOVERIES_PER_TICK` 个，从 `from` 轮着来）。返回认到的那个的名字。
fn spot(pic: &scale::Picture, tracked: &[Tracked], from: usize) -> Option<String> {
    let count = tracked.len();
    (0..count.min(DISCOVERIES_PER_TICK))
        .map(|k| &tracked[(from + k) % count])
        .find(|t| t.template.discover(&pic.pyramid).is_some())
        .map(|t| t.item.name.clone())
}

fn run(app: AppHandle) {
    let started = Instant::now();
    let mut version = u64::MAX;
    let mut config = BuffConfig::default();
    let mut tracked: Vec<Tracked> = Vec::new();
    let mut hwnd: Option<isize> = None;
    let mut discover_from = 0usize;
    let mut tracker = scale::ScaleTracker::default();
    // 上一帧的客户区尺寸和界面倍数：变了就作废
    let mut last_client: Option<(i32, i32)> = None;
    let mut last_scale: Option<(f32, f32)> = None;

    loop {
        std::thread::sleep(TICK);
        let now = started.elapsed().as_secs_f64();

        let latest = shared().version.load(Ordering::SeqCst);
        if latest != version {
            version = latest;
            config = shared().config.lock().unwrap_or_else(|e| e.into_inner()).clone();
            // 留住还在的 BUFF 的计时状态；改了图标的重建
            let mut old: HashMap<String, Tracked> =
                tracked.drain(..).map(|t| (t.item.id.clone(), t)).collect();
            for item in config.items.iter().filter(|i| i.enabled) {
                match old.remove(&item.id) {
                    Some(mut t) if t.item.icon_png == item.icon_png => {
                        t.item = item.clone();
                        tracked.push(t);
                    }
                    _ => match Tracked::new(item.clone()) {
                        Some(t) => tracked.push(t),
                        None => log::warn!("BUFF「{}」的图标解不开，跳过", item.name),
                    },
                }
            }
        }

        if !config.enabled || tracked.is_empty() {
            let note = if config.enabled { "还没有启用的 BUFF" } else { "没开" };
            publish(&app, status_of(&tracked, now, false, note.to_string()));
            continue;
        }

        if !hwnd.is_some_and(capture::is_window) {
            hwnd = capture::find_game_window().map(|g| g.hwnd);
        }
        let Some(window) = hwnd else {
            publish(&app, status_of(&tracked, now, false, "没找到游戏窗口".to_string()));
            continue;
        };
        if capture::is_minimized(window) {
            publish(&app, status_of(&tracked, now, false, "游戏最小化了，暂停".to_string()));
            continue;
        }
        let Some((client_w, client_h)) = capture::client_size(window) else {
            continue;
        };
        let band_h = ((client_h as f64 * BAND).ceil() as i32).max(1);
        let Some((frame, _)) = capture::grab_client(window, 0, 0, client_w, band_h) else {
            publish(&app, status_of(&tracked, now, false, "抓不到游戏画面".to_string()));
            continue;
        };

        if last_client != Some((client_w, client_h)) {
            last_client = Some((client_w, client_h));
            forget(&mut tracked);
        }

        // 这一帧的界面倍数：HUD 锚点量的 → 之前认定的 → 还没有就每帧试几个候选，认到一个就锁定
        let plan = tracker.plan((client_w, client_h), hud_scale_at(client_w, client_h));
        let locked_plan = matches!(plan, scale::Plan::Locked(_));
        let found = match plan {
            scale::Plan::Locked(s) => Some((s, scale::Picture::new(&frame.bgra, frame.width, frame.height, s))),
            scale::Plan::Search(batch) => batch.into_iter().find_map(|s| {
                let pic = scale::Picture::new(&frame.bgra, frame.width, frame.height, s);
                spot(&pic, &tracked, discover_from).map(|name| {
                    tracker.found(s);
                    log::info!("BUFF：游戏界面是 {:.3}×{:.3} 倍（「{name}」认出来的）", s.0, s.1);
                    (s, pic)
                })
            }),
        };
        let Some((mult, pic)) = found else {
            *shared().band_scale.lock().unwrap_or_else(|e| e.into_inner()) = None;
            let note = if tracker.searched_in_vain() {
                // 只有画面上确实有东西、找了很久还认不出，才让用户重新框；画面空着（BUFF 没出现、
                // 加载画面之类）时说不出是不是该重框，就说中性的话
                if band_has_content(&frame) {
                    "找了一阵没认出已启用的 BUFF 图标，游戏里能看到它的话，请重新框一下图标".to_string()
                } else {
                    "没在画面上找到已启用的 BUFF".to_string()
                }
            } else {
                "正在找游戏界面的倍数".to_string()
            };
            publish(&app, status_of(&tracked, now, true, note));
            continue;
        };

        if last_scale.is_some_and(|p| !scale::same_scale(p, mult)) {
            forget(&mut tracked);
        }
        last_scale = Some(mult);
        // 倍数认定了才告诉「框图标」那边
        *shared().band_scale.lock().unwrap_or_else(|e| e.into_inner()) = Some((client_w, client_h, mult));

        let count = tracked.len();
        let mut discoveries = 0;
        let mut seen = false;
        for k in 0..count {
            let index = (discover_from + k) % count;
            let t = &mut tracked[index];
            let previous = t.last_hit;
            let mut hit = previous.and_then(|p| t.template.track(&pic.pyramid, &p));
            if hit.is_none() && discoveries < DISCOVERIES_PER_TICK {
                discoveries += 1;
                hit = t.template.discover(&pic.pyramid);
            } else if hit.is_none() {
                // 这帧轮不到整条搜：沿用上一帧的判断，不当成「没看见」
                continue;
            }
            seen |= hit.is_some();
            let reading = hit.and_then(|h| scale::read_at(&pic, &h));
            t.last_hit = hit;
            t.timer.observe(now, hit.is_some(), reading.map(|r| (r.value, r.unit)));
            if let Some(reminder) = t.timer.reminder(now, t.item.threshold, t.item.expire_alert) {
                announce(&t.item, reminder, &config);
            }
        }
        discover_from = discover_from.wrapping_add(1);
        if locked_plan {
            tracker.seen(seen);
        }

        let note = format!("在盯 {} 个 BUFF", tracked.len());
        publish(&app, status_of(&tracked, now, true, note));
    }
}

/// 启动识别线程（没开 / 没有 BUFF 时它只是空转，不抓画面）。
pub fn spawn(app: AppHandle) {
    if let Some(state) = app.try_state::<AppState>() {
        set_config(load_config(&state.db));
    }
    std::thread::Builder::new()
        .name("buff-monitor".into())
        .spawn(move || run(app))
        .expect("起 BUFF 识别线程失败");
}

// ---------------------------------------------------------------------------
// 命令
// ---------------------------------------------------------------------------

#[tauri::command]
pub fn get_buff_config(state: State<'_, AppState>) -> BuffConfig {
    load_config(&state.db)
}

/// 存整份设置（界面改了开关 / 名字 / 秒数）。
#[tauri::command]
pub fn save_buff_config(state: State<'_, AppState>, mut config: BuffConfig) -> Result<BuffConfig, String> {
    config.validate()?;
    store_config(&state.db, &config)?;
    set_config(config.clone());
    Ok(config)
}

#[tauri::command]
pub fn get_buff_status() -> BuffStatus {
    shared().status.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// 给「框图标」用的截图：游戏客户区上面那条，原生分辨率。
#[derive(Debug, Clone, Serialize)]
pub struct BuffSnapshot {
    pub png_base64: String,
    pub width: usize,
    pub height: usize,
}

#[tauri::command]
pub async fn buff_capture_frame() -> Result<BuffSnapshot, String> {
    tauri::async_runtime::spawn_blocking(|| {
        let game = capture::find_game_window().ok_or_else(|| "没找到游戏窗口".to_string())?;
        if capture::is_minimized(game.hwnd) {
            return Err("游戏最小化了，先把它切出来".to_string());
        }
        let (client_w, client_h) =
            capture::client_size(game.hwnd).ok_or_else(|| "拿不到游戏客户区尺寸".to_string())?;
        let band_h = ((client_h as f64 * BAND).ceil() as i32).max(1);
        let (frame, _) = capture::grab_client(game.hwnd, 0, 0, client_w, band_h)
            .ok_or_else(|| "抓不到游戏画面".to_string())?;
        let rgba = crop_rgba(&frame, 0, 0, frame.width, frame.height);
        let png_base64 = encode_png(&rgba, frame.width as u32, frame.height as u32)?;
        let snapshot = BuffSnapshot { png_base64, width: frame.width, height: frame.height };
        *shared().snapshot.lock().unwrap_or_else(|e| e.into_inner()) = Some((frame, client_h as u32));
        Ok(snapshot)
    })
    .await
    .map_err(|err| format!("截图任务失败：{err}"))?
}

/// 从刚才那张截图里框一块当 BUFF 图标，加进设置。
#[tauri::command]
pub fn add_buff_from_frame(
    state: State<'_, AppState>,
    name: String,
    x: u32,
    y: u32,
    width: u32,
    height: u32,
) -> Result<BuffConfig, String> {
    if !(MIN_ICON..=MAX_ICON).contains(&width) || !(MIN_ICON..=MAX_ICON).contains(&height) {
        return Err(format!("框的大小要在 {MIN_ICON}–{MAX_ICON} 像素之间（现在 {width}×{height}）"));
    }
    let guard = shared().snapshot.lock().unwrap_or_else(|e| e.into_inner());
    let (frame, client_h) = guard.as_ref().ok_or_else(|| "先截一张图".to_string())?;
    let (x, y, w, h) = (x as usize, y as usize, width as usize, height as usize);
    if x + w > frame.width || y + h > frame.height {
        return Err("框超出了截图范围".to_string());
    }
    let rgba = crop_rgba(frame, x, y, w, h);
    // 知道界面倍数就缩回原生像素再存；不知道就照原样存，识别时按客户区高度猜
    let ui_scale = ui_scale_at(frame.width as i32, *client_h as i32);
    let (rgba, width, height, native) = match ui_scale {
        Some(s) => {
            let (native_rgba, nw, nh) = scale::normalize(&rgba, w, h, s.0, s.1);
            (native_rgba, nw as u32, nh as u32, true)
        }
        None => (rgba, width, height, false),
    };
    // 太平的一块（框到了空白处）做不了模板
    let (gray, opaque) = Gray::from_rgba(&rgba, width as usize, height as usize);
    if IconTemplate::new(&gray, &opaque).is_none() {
        return Err("框住的地方几乎没有花纹，认不出来 —— 框住 BUFF 图标本身".to_string());
    }
    let icon_png = encode_png(&rgba, width, height)?;
    let client_h = *client_h;
    drop(guard);

    add_item(&state, name, icon_png, width, height, client_h, None, native)
}

#[allow(clippy::too_many_arguments)]
fn add_item(
    state: &AppState,
    name: String,
    icon_png: String,
    width: u32,
    height: u32,
    client_h: u32,
    skill_id: Option<String>,
    native: bool,
) -> Result<BuffConfig, String> {
    let mut config = load_config(&state.db);
    let id = format!(
        "{:x}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or_default()
    );
    config.items.push(BuffItem {
        id,
        name: name.trim().to_string(),
        enabled: true,
        threshold: default_threshold(),
        speech: true,
        expire_alert: false,
        icon_png,
        width,
        height,
        client_h,
        native,
        skill_id,
    });
    config.enabled = true;
    config.validate()?;
    store_config(&state.db, &config)?;
    set_config(config.clone());
    Ok(config)
}

/// 内置的技能图标（小册子技能图鉴里的 BUFF 类技能），给「添加 BUFF」挑。
#[tauri::command]
pub fn list_buff_skills() -> Vec<catalog::SkillIcon> {
    catalog::skills().to_vec()
}

/// 从内置技能图标里挑一个加进设置。
#[tauri::command]
pub fn add_buff_from_skill(state: State<'_, AppState>, skill_id: String) -> Result<BuffConfig, String> {
    let skill = catalog::find(&skill_id).ok_or_else(|| format!("没有这个技能（{skill_id}）"))?;
    let config = load_config(&state.db);
    if config.items.iter().any(|i| i.skill_id.as_deref() == Some(skill_id.as_str())) {
        return Err(format!("「{}」已经在列表里了", skill.name));
    }
    add_item(&state, skill.name.clone(), skill.png.clone(), 32, 32, 0, Some(skill_id), true)
}

#[tauri::command]
pub fn delete_buff(state: State<'_, AppState>, id: String) -> Result<BuffConfig, String> {
    let mut config = load_config(&state.db);
    config.items.retain(|item| item.id != id);
    store_config(&state.db, &config)?;
    set_config(config.clone());
    Ok(config)
}

/// 试听：用当前的音量 / 语速念一句。
#[tauri::command]
pub fn test_buff_speech(text: String, volume: u8, rate: i8) {
    let text = text.trim();
    speech::speak(if text.is_empty() { "BUFF即将结束" } else { text }, volume, rate);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn png_round_trip_keeps_pixels() {
        let rgba: Vec<u8> = (0..16 * 12).flat_map(|i| [i as u8, 3, 200, 255]).collect();
        let b64 = encode_png(&rgba, 16, 12).unwrap();
        let (back, w, h) = decode_png(&b64).unwrap();
        assert_eq!((w, h), (16, 12));
        assert_eq!(back, rgba);
    }

    #[test]
    fn config_defaults_and_validation() {
        let config: BuffConfig = serde_json::from_str("{}").unwrap();
        assert_eq!(config, BuffConfig::default());
        let item: BuffItem = serde_json::from_str(
            r#"{"id":"a","name":" 圣盾 ","icon_png":"","width":32,"height":32,"client_h":768}"#,
        )
        .unwrap();
        assert!(item.enabled && item.speech && !item.expire_alert);
        assert_eq!(item.threshold, 10);
        let mut config = BuffConfig { items: vec![item.clone()], ..Default::default() };
        config.validate().unwrap();
        assert_eq!(config.items[0].name, "圣盾");
        config.items[0].threshold = 60;
        assert!(config.validate().is_err());
        config.items[0].threshold = 5;
        config.items[0].name = "  ".into();
        assert!(config.validate().is_err());
    }

    #[test]
    fn legacy_items_guess_their_crop_scale() {
        let mut item: BuffItem = serde_json::from_str(
            r#"{"id":"a","name":"圣盾","icon_png":"","width":32,"height":32,"client_h":1080}"#,
        )
        .unwrap();
        assert_eq!(item.legacy_scale(), 1.0);
        item.client_h = 1440;
        assert!((item.legacy_scale() - 1.875).abs() < 1e-4);
    }

    /// 老设置里没有 `native` 字段：按「不是原生」处理（走猜倍数的老路）。
    #[test]
    fn item_without_native_flag_is_legacy() {
        let item: BuffItem = serde_json::from_str(
            r#"{"id":"a","name":"圣盾","icon_png":"","width":32,"height":32,"client_h":768}"#,
        )
        .unwrap();
        assert!(!item.native);
    }
}
