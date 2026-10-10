//! 提醒的「出口」：响铃 + 任务栏闪烁 + 我们自己的置顶弹窗。
//!
//! 四类内容共用响铃与提醒窗：开服恢复、剪贴板命中黑名单、金价到价、内存到线
//! （设置页的测试按钮也走同一条出口）。开服与剪贴板各有一个冷却，金价与内存的
//! 节流放在各自的判定函数里（见 `meso_alert.rs` / `memory.rs`）—— 避免其中一种
//! 事件吞掉另一种真正需要用户处理的提醒。
//!
//! ## 为什么不靠 Windows 原生通知
//!
//! 这是**未安装版**（`bundle.active = false`，没有开始菜单快捷方式、没有注册
//! AppUserModelID）。Windows 上这种进程调 WinRT 的 Toast 会被**静默丢弃**：
//! `show()` 返回成功、日志里一个错都没有、屏幕上一片安静。实测就是这样 ——
//! 用户点「测试开服提醒」能听到蜂鸣，但通知压根没出现。
//!
//! 打开程序第一晚就撞上「该响的时候没响」，是整个工具最不能接受的失败模式，
//! 所以提醒**不能依赖 Toast**：
//!
//!   1. 响铃：`user32!MessageBeep` + `winmm!PlaySoundW`，都放不出来就退回
//!      `kernel32!Beep`。三个都是直接 FFI，不起子进程（早期那版用
//!      `powershell [console]::beep`，会先闪一个黑色控制台窗口）。
//!   2. 任务栏图标闪烁：`request_user_attention`。
//!   3. **自己的置顶提醒窗**（`alert-toast`）：一个无边框、始终置顶的小窗，
//!      画在屏幕右下角。它是唯一 100% 由我们控制的可见通道，也是这次真正
//!      把「点测试只听到声音、看不到东西」修掉的东西。
//!   4. 仍然顺手试一次原生 Toast —— 以后要是出了正式安装版，它会自动开始生效。
//!
//! ## 两个刻意的取舍
//!
//! * **不抢主窗口焦点**。开服那一刻用户多半正在游戏里（或者在等维护结束），
//!   把工具箱顶到前台等于把游戏踢到后台 —— 打怪途中被弹出去，比少一次提醒更糟。
//!   所以只闪任务栏 + 弹自己的小窗。
//! * **提醒窗用完即销毁**。这是个内存监控工具，多常驻一个 WebView2 进程要
//!   多占几十 MB，而这些内存正是它要帮游戏省下来的。开服提醒一辈子响不了几次，
//!   每次现开（约 0.3 秒）完全够用。

use parking_lot::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager, UserAttentionType};
use tauri_plugin_notification::NotificationExt;

/// 提醒窗的窗口标识（前端按 `?window=alert` 渲染）。
pub const ALERT_WINDOW_LABEL: &str = "alert-toast";

/// 提醒窗的自动消失时间。
const TOAST_LIFETIME: Duration = Duration::from_secs(20);

/// 两条来源共用这个冷却，免得同一场开服提醒两遍。
/// 十分钟足够长：真的「维护-开服-再维护-再开服」不会在十分钟内来两轮。
const ALERT_COOLDOWN: Duration = Duration::from_secs(600);

/// 提醒窗距离屏幕右下角的留白（逻辑像素）。
const TOAST_MARGIN: f64 = 24.0;

/// 谁报的警。来源写清楚，用户才能一眼分辨是开服还是交易对象。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AlertSource {
    /// 小册子「开服监控」接口（唯一的数据源）
    Monitor,
    /// 设置里的测试按钮
    Test,
    /// 剪贴板里的角色名命中了本地黑名单库
    Clipboard,
    /// 预设区服的金价到了设定的价位（见 `meso_alert.rs`）
    MesoPrice,
    /// 物理内存占用到了警戒线（见 `memory.rs`）
    Memory,
}

impl AlertSource {
    fn code(self) -> &'static str {
        match self {
            AlertSource::Monitor => "monitor",
            AlertSource::Test => "test",
            AlertSource::Clipboard => "clipboard",
            AlertSource::MesoPrice => "meso",
            AlertSource::Memory => "memory",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            AlertSource::Monitor => "小册子开服监控",
            AlertSource::Test => "测试提醒",
            AlertSource::Clipboard => "剪贴板比对黑名单",
            AlertSource::MesoPrice => "金价到价提醒",
            AlertSource::Memory => "内存到线提醒",
        }
    }

    fn title(self) -> &'static str {
        match self {
            AlertSource::Test => "🔔 开服提醒测试",
            AlertSource::Clipboard => "⚠️ 这个角色在黑名单里",
            AlertSource::Monitor => "🎉 冒险岛怀旧服已开服！",
            AlertSource::MesoPrice => "💰 金价到了你设的价位",
            AlertSource::Memory => "⚠️ 内存占用到线了",
        }
    }
}

/// 一次提醒的完整内容，同时也是发给提醒窗的事件负载。
#[derive(Debug, Clone, serde::Serialize)]
pub struct AlertPayload {
    pub title: String,
    /// 正文（「谁说的、什么时候说的」）
    pub body: String,
    /// 触发时刻，本地 `HH:MM:SS`
    pub at: String,
    /// "monitor" / "test" / "clipboard" / "meso" / "memory"
    pub source: String,
    /// 来源的中文说法
    pub source_label: String,
}

fn last_alert_slot() -> &'static Mutex<Option<Instant>> {
    static LAST_ALERT: OnceLock<Mutex<Option<Instant>>> = OnceLock::new();
    LAST_ALERT.get_or_init(|| Mutex::new(None))
}

/// 最近一次提醒的内容。提醒窗是「先建窗、后加载页面」，页面挂载时事件可能还
/// 没开始监听，所以它挂载后会主动来取一次；事件只负责「窗已经开着时再响一次」。
fn latest_alert_slot() -> &'static Mutex<Option<AlertPayload>> {
    static LATEST_ALERT: OnceLock<Mutex<Option<AlertPayload>>> = OnceLock::new();
    LATEST_ALERT.get_or_init(|| Mutex::new(None))
}

/// 第几次提醒。自动关闭的计时线程靠它判断「我该关的那个窗是不是已经被新提醒
/// 顶掉了」——否则连着响两次时，第一个计时器会把第二个提醒一起关掉。
fn alert_generation() -> &'static AtomicU64 {
    static GENERATION: AtomicU64 = AtomicU64::new(0);
    &GENERATION
}

pub fn latest_alert() -> Option<AlertPayload> {
    latest_alert_slot().lock().clone()
}

/// 距上一次真实提醒过了多少秒；不在冷却期内返回 None。
///
/// 只给设置页的测试按钮用：它需要把「现在点测试为什么和真开服不是一回事」
/// 说清楚，而不是让用户怀疑提醒坏了。
pub fn cooldown_remaining_sec() -> Option<u64> {
    let slot = last_alert_slot().lock();
    let previous = (*slot)?;
    let elapsed = previous.elapsed();
    (elapsed < ALERT_COOLDOWN).then(|| elapsed.as_secs())
}

/// 触发一次开服提醒。`body` 是给用户看的「谁说的服务器开了」。
///
/// 冷却期内直接返回，两条来源不会各响一遍。
pub fn fire_open_alert(app: &AppHandle, source: AlertSource, body: String) {
    if !take_cooldown_slot() {
        return;
    }
    if source == AlertSource::Monitor {
        record_alert_history(app, "server_open", source.title(), &body);
    }
    dispatch(app, source, body);
}

/// 绕过冷却的提醒：**只给设置页的测试按钮**。
///
/// 它刻意**不动冷却时间戳**：否则「点一次测试 → 五分钟真开服了」会被那次测试
/// 按下去，用户就会得出「测试能响、真开服不响」这个最糟的结论。
pub fn fire_open_alert_force(app: &AppHandle, source: AlertSource, body: String) {
    dispatch(app, source, body);
}

/// 金价到价提醒。
///
/// 不走开服提醒那个十分钟冷却：要不要响由 `meso_alert::decide` 决定（只在价格
/// 跨过阈值的那一下响，退回去够远才重新上膛），两件事共用一个冷却只会互相吃掉。
pub fn fire_meso_alert(app: &AppHandle, body: String) {
    record_alert_history(app, "meso_price", AlertSource::MesoPrice.title(), &body);
    dispatch(app, AlertSource::MesoPrice, body);
}

/// 内存到线提醒。
///
/// 同样不走开服提醒那个十分钟冷却：要不要响由 `memory::decide` 决定（连续 30 秒
/// 在线上才响、回落 5 个百分点才重新上膛、两次至少隔 10 分钟），两件事共用一个
/// 冷却只会互相吃掉。
pub fn fire_memory_alert(app: &AppHandle, body: String) {
    record_alert_history(app, "memory_high", AlertSource::Memory.title(), &body);
    dispatch(app, AlertSource::Memory, body);
}

/// 冷却闸门：不在冷却期内就占下这个位置。
fn take_cooldown_slot() -> bool {
    let mut slot = last_alert_slot().lock();
    let now = Instant::now();
    if let Some(previous) = *slot {
        let elapsed = now.duration_since(previous);
        if elapsed < ALERT_COOLDOWN {
            log::info!("开服提醒在冷却期内（{} 秒前刚提醒过），跳过", elapsed.as_secs());
            return false;
        }
    }
    *slot = Some(now);
    true
}

/// 剪贴板命中的冷却。它和开服提醒**各用各的**：查黑名单是随时的（可能一晚上
/// 复制到好几个名字），而开服提醒一辈子响不了几次 —— 共用一个冷却只会互相吃掉。
const CLIPBOARD_COOLDOWN: Duration = Duration::from_secs(90);

fn clipboard_slot() -> &'static Mutex<Option<Instant>> {
    static LAST_CLIPBOARD_ALERT: OnceLock<Mutex<Option<Instant>>> = OnceLock::new();
    LAST_CLIPBOARD_ALERT.get_or_init(|| Mutex::new(None))
}

/// 剪贴板命中黑名单时提醒一次。返回是否真的报了（冷却期内返回 false）。
///
/// 只在**确定命中**（同名 + 同服）时调用 —— 由 `blacklist` 模块判定。
pub fn fire_blacklist_hit(app: &AppHandle, entry: &crate::db::BlacklistEntry) -> bool {
    {
        let mut slot = clipboard_slot().lock();
        let now = Instant::now();
        if let Some(previous) = *slot {
            let elapsed = now.duration_since(previous);
            if elapsed < CLIPBOARD_COOLDOWN {
                log::info!(
                    "黑名单提醒在冷却期内（{} 秒前刚提醒过），跳过",
                    elapsed.as_secs()
                );
                return false;
            }
        }
        *slot = Some(now);
    }

    let payload = AlertPayload {
        title: AlertSource::Clipboard.title().to_string(),
        body: format!(
            "角色「{}」（{}）· {}\n记录原因：{}\n记录于 {}\n\n—— 来自本机黑名单库的剪贴板自动比对。大额交易请走担保。",
            entry.player_name, entry.server, entry.category, entry.reason, entry.created_at
        ),
        at: chrono::Local::now().format("%H:%M:%S").to_string(),
        source: AlertSource::Clipboard.code().to_string(),
        source_label: AlertSource::Clipboard.label().to_string(),
    };

    record_alert_history(app, "blacklist_hit", &payload.title, &payload.body);
    dispatch_payload(app, payload);
    true
}

fn record_alert_history(app: &AppHandle, kind: &str, title: &str, detail: &str) {
    let Some(state) = app.try_state::<crate::commands::AppState>() else {
        log::warn!("无法记录提醒历史（AppState 尚未就绪）：{}", kind);
        return;
    };
    if let Err(err) = state.db.record_alert(kind, title, detail) {
        log::warn!("记录提醒历史失败（{}）：{}", kind, err);
    }
}

fn dispatch(app: &AppHandle, source: AlertSource, body: String) {
    let payload = AlertPayload {
        title: source.title().to_string(),
        body,
        at: chrono::Local::now().format("%H:%M:%S").to_string(),
        source: source.code().to_string(),
        source_label: source.label().to_string(),
    };
    dispatch_payload(app, payload);
}

/// 提醒的「出口」：不管内容是谁给的，响铃 / 闪任务栏 / 弹窗这三件事都一样。
fn dispatch_payload(app: &AppHandle, payload: AlertPayload) {
    log::info!(
        "触发提醒（来源：{}）：{}",
        payload.source_label,
        payload.body
    );

    *latest_alert_slot().lock() = Some(payload.clone());
    let generation = alert_generation().fetch_add(1, Ordering::SeqCst) + 1;

    play_alert_sound();
    flash_taskbar(app);

    // 建窗 / 关窗都在别的线程上做：窗口操作只能投递到事件循环，不能在
    // 调用方的栈上同步做（GUI 线程会重入，见 tests/ui_thread_callbacks.rs）。
    let window_app = app.clone();
    let window_payload = payload.clone();
    std::thread::spawn(move || show_alert_window(&window_app, window_payload));

    let close_app = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(TOAST_LIFETIME);
        // 期间又响过一次，就让新的计时器管那个窗
        if alert_generation().load(Ordering::SeqCst) != generation {
            return;
        }
        close_alert_window(&close_app);
    });

    show_os_toast(app, &payload);
}

/// 任务栏图标闪烁。
///
/// **故意不把主窗口抬到最前**：开服那一刻用户很可能正在游戏里，
/// `set_focus` 会把游戏踢到后台。提醒要显眼，但不能打断游戏。
fn flash_taskbar(app: &AppHandle) {
    let Some(window) = app.get_webview_window("main") else {
        return;
    };
    // request_user_attention 本身是「投递」语义（走事件循环），
    // 不会像 set_focus 那样在调用栈里做窗口激活。
    if window
        .request_user_attention(Some(UserAttentionType::Critical))
        .is_err()
    {
        log::debug!("任务栏闪烁请求失败（不影响响铃与弹窗提醒）");
    }
}

/// 关掉提醒窗（用户点「知道了」、或自动超时）。
///
/// 是 `close` 不是 `hide`：多留一个 WebView2 进程要多占几十 MB，
/// 而这正是这个工具要帮游戏省下来的内存。下次提醒现开就行。
pub fn close_alert_window(app: &AppHandle) {
    // 投递出去关：这个函数的调用方之一是同步命令 `dismiss_open_alert`，
    // 而同步命令在 Windows 上跑在 UI 线程、WebView2 的回调栈里，
    // 在那里直接 `close()` 会重入事件循环（同类死锁已经踩过三次）。
    let app = app.clone();
    std::thread::spawn(move || {
        let Some(window) = app.get_webview_window(ALERT_WINDOW_LABEL) else {
            return;
        };
        if let Err(err) = window.close() {
            log::warn!("关闭开服提醒窗失败：{}", err);
        }
    });
}

/// 确保提醒窗存在、显示在右下角，并把这次的内容塞进去。
fn show_alert_window(app: &AppHandle, payload: AlertPayload) {
    let window = match app.get_webview_window(ALERT_WINDOW_LABEL) {
        Some(window) => window,
        None => match build_alert_window(app) {
            Ok(window) => window,
            Err(err) => {
                log::error!("创建开服提醒窗失败（响铃与任务栏闪烁仍然有效）：{}", err);
                return;
            }
        },
    };

    position_bottom_right(&window);

    if let Err(err) = window.show() {
        log::warn!("显示开服提醒窗失败：{}", err);
        return;
    }

    // 「到底弹出来了没有」是这条链路唯一没法靠单测回答的问题，所以写进日志：
    // 出问题时用户手上就有一行可对照的证据。
    log::info!(
        "开服提醒窗已显示（右下角置顶，{} 秒后自动关闭，visible={}）",
        TOAST_LIFETIME.as_secs(),
        window.is_visible().unwrap_or(false)
    );

    // 页面还没挂载完时这条事件会丢，所以提醒窗挂载后还会主动取一次
    // `get_latest_open_alert`，两条路兜底。
    if let Err(err) = app.emit_to(ALERT_WINDOW_LABEL, "open-alert", payload) {
        log::debug!("投递提醒事件失败（提醒窗会自行拉取内容）：{}", err);
    }
}

fn build_alert_window(app: &AppHandle) -> Result<tauri::WebviewWindow, String> {
    tauri::WebviewWindowBuilder::new(
        app,
        ALERT_WINDOW_LABEL,
        tauri::WebviewUrl::App("index.html?window=alert".into()),
    )
    .title("开服提醒")
    // 高度按「最长的提醒文本也不出现滚动条」定：测试提醒的正文有一百多字，
    // 在 392 宽下会折成四行。宁可给足留白，也别让用户在提醒窗里翻页。
    .inner_size(392.0, 224.0)
    .resizable(false)
    .decorations(false)
    .transparent(true)
    .shadow(true)
    .always_on_top(true)
    .skip_taskbar(true)
    // 不抢焦点：正打着游戏时被弹出去比少一次提醒更糟
    .focused(false)
    .visible(false)
    .build()
    .map_err(|e| e.to_string())
}

/// 把提醒窗摆到主屏右下角。
///
/// 用物理像素算：多显示器下主屏的原点不一定是 (0,0)，`size()` 也不含任务栏，
/// 所以先取显示器位置再往里缩。
fn position_bottom_right(window: &tauri::WebviewWindow) {
    let Ok(Some(monitor)) = window.primary_monitor() else {
        return;
    };
    let Ok(size) = window.outer_size() else {
        return;
    };

    let scale = monitor.scale_factor();
    let margin = (TOAST_MARGIN * scale) as i32;
    let monitor_position = monitor.position();
    let monitor_size = monitor.size();

    let x = monitor_position.x + monitor_size.width as i32 - size.width as i32 - margin;
    let y = monitor_position.y + monitor_size.height as i32 - size.height as i32 - margin;

    if let Err(err) = window.set_position(tauri::PhysicalPosition::new(x, y)) {
        log::debug!("提醒窗定位失败（会停在系统默认位置）：{}", err);
    }
}

fn show_os_toast(app: &AppHandle, payload: &AlertPayload) {
    // 这一步是「环境支持就顺便弹一个」。
    //
    // 未安装版（没有注册 AppUserModelID）时 Windows 会把它静默丢掉，`show()`
    // 照样返回 Ok —— 所以它**只是加分项**，真正的可见通道是上面那个置顶小窗。
    if let Err(err) = app
        .notification()
        .builder()
        .title(payload.title.clone())
        .body(payload.body.clone())
        .show()
    {
        log::warn!("系统通知发送失败（不影响响铃与弹窗提醒）：{}", err);
    }
}

// ---------------------------------------------------------------------------
// 声音
// ---------------------------------------------------------------------------

#[cfg(windows)]
const MB_ICONEXCLAMATION: u32 = 0x0000_0030;
#[cfg(windows)]
const SND_ASYNC: u32 = 0x0000_0001;
#[cfg(windows)]
const SND_NODEFAULT: u32 = 0x0000_0002;
#[cfg(windows)]
const SND_FILENAME: u32 = 0x0002_0000;

#[cfg(windows)]
#[link(name = "user32")]
extern "system" {
    fn MessageBeep(u_type: u32) -> i32;
}

#[cfg(windows)]
#[link(name = "kernel32")]
extern "system" {
    fn Beep(dw_freq: u32, dw_duration: u32) -> i32;
}

#[cfg(windows)]
#[link(name = "winmm")]
extern "system" {
    fn PlaySoundW(psz_sound: *const u16, hmod: *mut std::ffi::c_void, fdw_sound: u32) -> i32;
}

/// 放一段明确的系统通知音（比 MessageBeep 能触发的提示音明显得多）。
///
/// 这些 wav 的文件名在各语言版本的 Windows 上都是一样的，所以先拼「Windows
/// 目录 + 文件名」，存在才放；文件名换了（或用户删了）就返回 false 让调用方兜底。
#[cfg(windows)]
fn play_notification_wav() -> bool {
    let root = std::env::var("SystemRoot").unwrap_or_else(|_| "C:".to_string());

    for name in [
        "Windows Notify System Generic.wav",
        "Windows Notify.wav",
        "Windows Ding.wav",
        "Alarm01.wav",
    ] {
        let path = std::path::Path::new(&root).join("Media").join(name);
        if !path.exists() {
            continue;
        }
        let text = path.to_string_lossy().to_string();
        let wide: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
        let ok = unsafe {
            PlaySoundW(
                wide.as_ptr(),
                std::ptr::null_mut(),
                SND_FILENAME | SND_ASYNC | SND_NODEFAULT,
            )
        };
        if ok != 0 {
            return true;
        }
    }
    false
}

/// 两层声音，从“克制”到“你一定听得见”。
///
/// 只调 MessageBeep 是不够的：Windows 默认声音方案里，它能触发的
/// SystemAsterisk / SystemExclamation 都被映射成很轻的 `Windows Background.wav`
/// （本机注册表实测），戴着耳机打游戏基本听不见。
///
/// 放在单独的线程里：这些 API 名字上是异步的，但两次之间要 sleep，
/// 不能把轮询循环卡住。
#[cfg(windows)]
pub fn play_alert_sound() {
    std::thread::spawn(|| {
        unsafe {
            MessageBeep(MB_ICONEXCLAMATION);
        }
        std::thread::sleep(Duration::from_millis(280));

        // 注意顺序：后放的会打断前一个。通知音放最后，别让 MessageBeep 截断它。
        if !play_notification_wav() {
            unsafe {
                Beep(1200, 350);
            }
        }
    });
}

#[cfg(not(windows))]
pub fn play_alert_sound() {}
