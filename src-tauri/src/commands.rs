use crate::alert::{self, AlertPayload, AlertSource};
use crate::blacklist;
use crate::channel::{self, ChannelState};
use crate::db::{
    AlertHistoryRow, AppSettings, BlacklistEntry, Database, ExpCurvePoint, ExpHistoryRow,
};
use crate::db::ExpTotals;
use crate::exp::capture::{self, Frame, Method, ScreenMode};
use crate::exp::reader::CalibrationTest;
use crate::exp::region::{self, NormRect, RegionProfile};
use crate::exp::{ExpStatus, ExpTracker, SessionStatus};
use crate::exp::tracker::{MAX_GOAL_LEVEL, MIN_GOAL_LEVEL};
use crate::hotkeys::{self, HotkeyAction, HotkeyStatus};
use crate::meso_alert::{self, MesoAlertConfig};
use crate::memory::{MemoryMonitor, MemoryPayload};
use crate::mxdc_client::{server_list, ItemDetail, MarketItem, MesoReport, MxdcClient, ServerInfo};
use crate::mxdc_monitor::{MonitorConfig, MonitorManager, MonitorStatus};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tauri::{AppHandle, Emitter, Manager, State};

/// 练级目标等级存在设置表里的键。
pub const EXP_GOAL_KEY: &str = "exp_goal_level";

pub struct AppState {
    pub db: Arc<Database>,
    pub memory_monitor: Arc<MemoryMonitor>,
    /// 小册子「开服监控」轮询（站点每分钟探一次登录入口，我们接它的结论报警）
    pub monitor_manager: Arc<MonitorManager>,
    pub mxdc_client: Arc<MxdcClient>,
    /// 经验统计（截屏读像素，不碰游戏进程）
    pub exp_tracker: Arc<ExpTracker>,
    /// 当前频道（几线）：查游戏进程的 TCP 连接端口反推，不碰游戏进程
    pub channel_watcher: Arc<channel::ChannelWatcher>,
    /// 关闭主窗口时是退到托盘常驻、还是真正退出进程。
    ///
    /// 放在内存里（而不是关闭那一刻再查数据库）是故意的：窗口关闭事件是在
    /// 主线程的窗口过程里同步回调的，那里绝不能去抢 SQLite 的锁 ——
    /// 这个项目已经在「在 UI 线程回调里做重活」上栽过三次了。
    pub close_to_tray: Arc<AtomicBool>,
}

#[tauri::command]
pub fn get_settings(state: State<'_, AppState>) -> AppSettings {
    state.db.get_all_settings()
}

/// 小册子真实的区服一览（拍卖工具的 `server` 参数）。
/// 前端下拉框统一从这里取，避免再出现「各处各写一份、漏掉蓝蜗牛」的情况。
#[tauri::command]
pub fn get_server_list() -> Vec<ServerInfo> {
    server_list()
}

/// 当前正在运行的二进制身份。
///
/// 排查时吃过亏：磁盘上同时存在新、旧两份构建，两个进程一起跑，
/// Alt+F 呼出的是旧进程的 HUD（服务器清单、查询参数都是旧的），
/// 却看起来像“新代码没改到”。把版本和二进制时间暴露出来，
/// 日志和界面上一眼就能确认在跑哪个构建。
#[derive(Debug, Clone, serde::Serialize)]
pub struct BuildInfo {
    pub version: String,
    /// 二进制文件时间（比“编译时刻”更可信，它就是你双击的那个文件）
    pub built_at: String,
    pub exe_path: String,
}

pub fn build_info() -> BuildInfo {
    let path = std::env::current_exe().unwrap_or_default();
    let built_at = std::fs::metadata(&path)
        .and_then(|meta| meta.modified())
        .ok()
        .map(|stamp| {
            let local: chrono::DateTime<chrono::Local> = stamp.into();
            local.format("%Y-%m-%d %H:%M:%S").to_string()
        })
        .unwrap_or_else(|| "未知".to_string());

    BuildInfo {
        version: env!("CARGO_PKG_VERSION").to_string(),
        built_at,
        exe_path: path.display().to_string(),
    }
}

#[tauri::command]
pub fn get_build_info() -> BuildInfo {
    build_info()
}

/// 「这一版更新了什么」该不该弹 —— 该弹就把版本号还回来，并记下已经见过。
///
/// 判断放后端有两个理由：
///
/// 1. **「只弹一次」不能依赖前端的执行次数**。每次开窗、WebView 重载都会跑一遍
///    前端的挂载逻辑；把「见没见过」记在前端，弹窗就会重复弹。
/// 2. 版本号只有一个权威来源（编译时定死的 `CARGO_PKG_VERSION`），前端不必自己维护。
///
/// 第一次安装也会弹（没有记录 = 没见过），这正好 —— 新用户第一次打开就该知道
/// 这东西能干什么。
#[tauri::command]
pub fn take_version_greeting(state: State<'_, AppState>) -> Option<String> {
    let current = env!("CARGO_PKG_VERSION").to_string();
    let seen = state.db.get_setting("last_seen_version");
    if seen.as_deref() == Some(current.as_str()) {
        return None;
    }
    if let Err(err) = state.db.set_setting("last_seen_version", &current) {
        log::warn!("记下「已见过的版本」失败：{err}");
    }
    log::info!("第一次见到 v{current}（上次是 {seen:?}），会弹一次更新说明");
    Some(current)
}

/// 设置里**只改传进来的那几个字段**，其余保持库里现值，并当场生效。
///
/// 为什么不是「提交整份设置」（原来那个 `save_settings`）：设置页已经拆成三个
/// 导航页（悬浮窗与内存 / 开服提醒 / 基础配置），各页只管自己那几项，而且都是
/// **改完立刻生效**。整份提交留着一个很隐蔽的坑：A 页加载了一份设置，用户又去
/// B 页改了东西，再回 A 页点保存，就把 B 页的改动盖回旧值了。按字段改没有这种可能。
#[derive(Debug, Default, serde::Deserialize)]
pub struct SettingsPatch {
    /// 订阅小册子开服监控（站点每分钟探一次登录入口）
    pub monitor_enabled: Option<bool>,
    /// 拉取开服监控的间隔（秒）
    pub monitor_interval_sec: Option<u64>,
    /// 关闭主窗口时：退到托盘常驻 / 真正退出进程
    pub close_to_tray: Option<bool>,
}

#[tauri::command]
pub fn patch_settings(
    patch: SettingsPatch,
    state: State<'_, AppState>,
) -> Result<AppSettings, String> {
    state
        .db
        .patch_settings(
            patch.monitor_enabled,
            patch.monitor_interval_sec,
            patch.close_to_tray,
            None,
        )
        .map_err(|e| format!("保存配置失败: {}", e))?;

    // 返回形状仍是完整 AppSettings，避免前端协议变化；数据库只写入 patch 给出的键。
    let settings = state.db.get_all_settings();
    if let Some(value) = patch.close_to_tray {
        state.close_to_tray.store(value, Ordering::Relaxed);
    }

    // 只对**这次真的改了的那几项**做运行时副作用。
    //
    // 尤其是快捷键：每次 patch 都顺手 `apply_all` 一次是没有必要的（那会让键
    // 有一瞬间是注销状态），而改键捕获过程中重注册更是会把这次修改盖回去。
    // 快捷键自己有 `apply_hotkey` 这条路径，滑杆/区服同理。
    if patch.monitor_enabled.is_some() || patch.monitor_interval_sec.is_some() {
        state.monitor_manager.update_config(MonitorConfig {
            enabled: settings.monitor_enabled,
            interval_sec: settings.monitor_interval_sec,
        });
    }
    if let Some(value) = patch.close_to_tray {
        state.close_to_tray.store(value, Ordering::Relaxed);
    }

    Ok(settings)
}

/// 两个悬浮窗快捷键的当前状态（界面用来显示「这个键到底注册上了没有」）。
#[tauri::command]
pub fn get_hotkey_status() -> Vec<HotkeyStatus> {
    hotkeys::statuses()
}

/// 单独注册一个快捷键（设置页里按完键就能立刻重注册并看到结果）。
///
/// `action` 取 `"price_hud"` / `"lite_float"` / `"exp_toggle"`。
#[tauri::command]
pub fn apply_hotkey(
    app: AppHandle,
    action: String,
    hotkey: String,
    state: State<'_, AppState>,
) -> Result<Vec<HotkeyStatus>, String> {
    let mut settings = state.db.get_all_settings();
    match action.as_str() {
        "price_hud" => settings.hotkey = hotkey,
        "lite_float" => settings.lite_float_hotkey = hotkey,
        "exp_toggle" => settings.exp_hotkey = hotkey,
        other => return Err(format!("未知的快捷键：{}", other)),
    }

    // 一次性重注册三个：撞键（两个动作填成同一个键）要能被发现 ——
    // 交给系统去撞的结果是「谁先注册谁生效」，而注册顺序是实现细节。
    let statuses = hotkeys::apply_all(
        &app,
        &settings.hotkey,
        &settings.lite_float_hotkey,
        &settings.exp_hotkey,
    );

    // 顺手落库：这样即使界面没点「保存全部配置」，改键也已经生效且记得住。
    if let Err(err) = state.db.set_setting("hotkey", &settings.hotkey) {
        log::warn!("写入查价悬浮窗快捷键失败: {}", err);
    }
    if let Err(err) = state
        .db
        .set_setting("lite_float_hotkey", &settings.lite_float_hotkey)
    {
        log::warn!("写入精简悬浮窗快捷键失败: {}", err);
    }
    if let Err(err) = state.db.set_setting("exp_hotkey", &settings.exp_hotkey) {
        log::warn!("写入经验统计快捷键失败: {}", err);
    }

    Ok(statuses)
}

// ---------------------------------------------------------------------------
// 经验统计
//
// 只有 `get_exp_preview` 是 async 的：它会在里面截屏（十几毫秒的阻塞调用），
// 放 tokio 的 `async` 命令里，别占着 WebView2 的回调栈 —— 这个项目在这上面栽过三次。
// 其它命令读的都是内存里的状态，同步就够。
// ---------------------------------------------------------------------------

#[tauri::command]
pub fn get_exp_status(state: State<'_, AppState>) -> ExpStatus {
    state.exp_tracker.status()
}

/// 当前频道（几线）：`auto` = 从游戏连接读到的；`manual` = 用户手动定的；
/// `stale` = 上次记录（游戏没开或还没连上）。
#[tauri::command]
pub fn get_channel_state(state: State<'_, AppState>) -> ChannelState {
    state.channel_watcher.state()
}

/// 手动定一次线号：把这台服务器节点的端口基准校准回来（一台只需定一次）。
#[tauri::command]
pub fn set_channel_manual(
    channel: u32,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<ChannelState, String> {
    let next = state.channel_watcher.set_manual(&state.db, channel)?;
    if let Err(err) = app.emit(channel::EVENT_NAME, &next) {
        log::warn!("广播频道变化失败：{err}");
    }
    Ok(next)
}

/// 设 / 清练级目标等级（`None` = 不设目标）。存在设置里，重启还在。
#[tauri::command]
pub fn set_exp_goal(level: Option<u32>, state: State<'_, AppState>) -> Result<ExpStatus, String> {
    if let Some(level) = level {
        if !(MIN_GOAL_LEVEL..=MAX_GOAL_LEVEL).contains(&level) {
            return Err(format!("目标等级要在 {MIN_GOAL_LEVEL}–{MAX_GOAL_LEVEL} 之间"));
        }
    }
    state
        .db
        .set_setting(
            EXP_GOAL_KEY,
            &level.map(|value| value.to_string()).unwrap_or_default(),
        )
        .map_err(|err| format!("保存练级目标失败：{err}"))?;
    state.exp_tracker.set_goal(level);
    Ok(state.exp_tracker.status())
}

/// 开始统计。**不点就不开始**（对着登录界面发呆不该产生数据）。
#[tauri::command]
pub fn start_exp_session(state: State<'_, AppState>) -> Result<ExpStatus, String> {
    state.exp_tracker.start()?;
    let status = state.exp_tracker.status();
    log::info!("经验统计：开始");
    Ok(status)
}

#[tauri::command]
pub fn pause_exp_session(state: State<'_, AppState>) -> Result<ExpStatus, String> {
    state.exp_tracker.pause()?;
    Ok(state.exp_tracker.status())
}

#[tauri::command]
pub fn resume_exp_session(state: State<'_, AppState>) -> Result<ExpStatus, String> {
    state.exp_tracker.resume()?;
    Ok(state.exp_tracker.status())
}

/// 结束一段。**不落库** —— 界面随后要问一句「计入历史吗」。
#[tauri::command]
pub fn end_exp_session(state: State<'_, AppState>) -> Result<SessionStatus, String> {
    state.exp_tracker.end()
}

/// 「计不计入历史」的回答。不计入就连每分钟的采样点一起丢掉。
/// `map_name` 是用户给这一段填的地图（可空）—— 存进去，历史和卡片上都能看见「在哪练的」。
#[tauri::command]
pub fn resolve_exp_session(
    save: bool,
    map_name: Option<String>,
    state: State<'_, AppState>,
) -> Result<String, String> {
    let message = state
        .exp_tracker
        .resolve_pending(save, map_name.as_deref().unwrap_or(""), &state.db)?;
    Ok(message)
}

/// 诊断页要看「我们眼里看到了什么」：把底部那一条画成点阵文本。
///
/// 字形表是内置的，正常永远不需要它；它的用处是万一哪天游戏改了字体，
/// 能让用户和我们一眼看出「画面里确实有字，只是对不上」。
#[tauri::command]
pub async fn get_exp_preview(state: State<'_, AppState>) -> Result<Vec<String>, String> {
    state.exp_tracker.preview()
}

fn valid_region_kind(kind: &str) -> bool {
    region::is_valid_kind(kind)
}

/// 当前校准档（比例坐标，一行一个 kind）。
///
/// `None` = 还没有校准过（读的时候整幅画面自动定位）。与分辨率无关的口径
/// 见 `exp::region` 的模块文档：**不按客户区尺寸分开存**。
#[tauri::command]
pub fn get_region_profile(kind: String, state: State<'_, AppState>) -> Option<RegionProfile> {
    if !valid_region_kind(&kind) {
        return None;
    }
    state.db.get_region_profile(&kind)
}

/// 保存一条**手动**校准（前端拖框交上来的比例矩形）。
///
/// 比例只保证「在客户区里」，物理尺寸还要用当前客户区校一次：
/// 一个 0.1% 宽的框合法但没有意义，交给抓屏只会出乱子。
#[tauri::command]
pub async fn save_region_profile(
    kind: String,
    rect: NormRect,
    text_height: Option<u32>,
    state: State<'_, AppState>,
) -> Result<RegionProfile, String> {
    if !valid_region_kind(&kind) {
        return Err("不支持的校准区域类型".to_string());
    }
    if !rect.is_valid() {
        return Err("校准矩形不在客户区范围内，请重新框选".to_string());
    }
    let db = Arc::clone(&state.db);
    let tracker = Arc::clone(&state.exp_tracker);
    // 抓窗口 / 查尺寸是阻塞活；同步命令会把 UI 线程卡住，走 async + 阻塞线程。
    tauri::async_runtime::spawn_blocking(move || {
        let game =
            capture::find_game_window().ok_or_else(|| "没找到游戏窗口（游戏没开？）".to_string())?;
        let (client_w, client_h) = capture::client_size(game.hwnd)
            .ok_or_else(|| "拿不到游戏客户区尺寸".to_string())?;
        let pixel = rect
            .to_pixel(client_w, client_h)
            .ok_or_else(|| "校准矩形无法映射到客户区".to_string())?;
        if !pixel.is_usable() {
            return Err("框太小了（至少 8×8 像素），拖动四边把它调大一点".to_string());
        }
        let profile = RegionProfile {
            kind,
            rect,
            source: region::RegionSource::Manual,
            text_height,
            learned_unix: 0,
        };
        let saved = db
            .save_region_profile(&profile, Some((client_w, client_h)))
            .map_err(|err| format!("保存校准失败：{err}"))?;
        tracker.set_region(saved.clone());
        Ok(saved)
    })
    .await
    .map_err(|err| format!("保存校准任务失败：{err}"))?
}

/// 清掉某个 kind 的校准（回自动识别）。
#[tauri::command]
pub async fn clear_region_profiles(kind: String, state: State<'_, AppState>) -> Result<(), String> {
    if !valid_region_kind(&kind) {
        return Err("不支持的校准区域类型".to_string());
    }
    let db = Arc::clone(&state.db);
    let tracker = Arc::clone(&state.exp_tracker);
    tauri::async_runtime::spawn_blocking(move || {
        db.clear_region_profiles(&kind)
            .map_err(|err| format!("清除校准失败：{err}"))?;
        tracker.clear_region(&kind);
        Ok(())
    })
    .await
    .map_err(|err| format!("清除校准任务失败：{err}"))?
}

/// 「测试读数」：拿一个**还没保存**的框立刻读一次，不落库、也不动当前校准。
///
/// 三种 kind 各有各的验收口径（见 `ExpReader::test_region`）：
/// 经验行要过经验表自洽，地图名要过 `looks_like_map_name`，等级要抠得出 1~120。
#[tauri::command]
pub async fn test_exp_region(
    kind: String,
    rect: NormRect,
    state: State<'_, AppState>,
) -> Result<CalibrationTest, String> {
    if !region::is_valid_kind(&kind) {
        return Err("不支持的校准类型".to_string());
    }
    let tracker = Arc::clone(&state.exp_tracker);
    tauri::async_runtime::spawn_blocking(move || Ok(tracker.test_region(&kind, rect)))
        .await
        .map_err(|err| format!("测试读数任务失败：{err}"))?
}

/// 「自动识别」：抓一整帧自动定位经验行，成功即学成 auto 档并落库。
///
/// 这是用户**显式**点的动作，所以以它为准：手动档就此让位（与网页版枫记的
/// 「重新自动识别」一致）。后台的自动学习不会覆盖手动档（见 `exp::reader`）。
#[tauri::command]
pub async fn auto_calibrate_exp(state: State<'_, AppState>) -> Result<CalibrationTest, String> {
    let tracker = Arc::clone(&state.exp_tracker);
    tauri::async_runtime::spawn_blocking(move || Ok(tracker.auto_calibrate()))
        .await
        .map_err(|err| format!("自动识别任务失败：{err}"))?
}

/// 校准蒙版窗需要的几何：游戏客户区尺寸（物理像素）+ 已有的校准档。
#[derive(Debug, Clone, serde::Serialize)]
pub struct CalibrationGeometry {
    pub client_w: i32,
    pub client_h: i32,
    pub profile: Option<RegionProfile>,
}

/// 蒙版窗自己来问「我该盖在多大一块上、有没有已存的框」。
#[tauri::command]
pub fn calibration_overlay_geometry(state: State<'_, AppState>) -> Result<CalibrationGeometry, String> {
    let game = capture::find_game_window().ok_or_else(|| "没找到游戏窗口（游戏没开？）".to_string())?;
    let (_, _, client_w, client_h) = capture::client_screen_rect(game.hwnd)
        .ok_or_else(|| "拿不到游戏客户区位置".to_string())?;
    if client_w <= 0 || client_h <= 0 {
        return Err("游戏客户区尺寸异常".to_string());
    }
    Ok(CalibrationGeometry {
        client_w,
        client_h,
        profile: state.db.get_region_profile(region::KIND_EXP_LINE),
    })
}

/// 打开「游戏画面蒙版」校准窗：正好盖在游戏客户区上，直接在真实画面上拖框。
///
/// # 为什么把游戏抬到前台、把主窗口藏起来
///
/// 蒙版是**透明的**：它只负责压暗和画框，游戏画面必须能透过它看见。
/// 用户主动点了「手动校准」，所以把游戏抬到前面、让主窗口先让开是预期行为
/// （平时我们绝不抢焦点）。关掉蒙版时再把主窗口抬回来。
///
/// # 为什么必须 `async`
///
/// Windows 上在同步命令里 `WebviewWindowBuilder::build()` 会死锁（项目里
/// 登录窗白屏就是这么来的，见 `open_login_window` 上面那段注释）。
#[tauri::command]
pub async fn open_calibration_overlay(app: AppHandle) -> Result<(), String> {
    const LABEL: &str = "calibration-overlay";

    let game = capture::find_game_window().ok_or_else(|| "没找到游戏窗口（游戏没开？）".to_string())?;
    let (x, y, width, height) = capture::client_screen_rect(game.hwnd)
        .ok_or_else(|| "拿不到游戏客户区位置".to_string())?;
    if width <= 0 || height <= 0 {
        return Err("游戏客户区尺寸异常".to_string());
    }

    // 用户显式发起的动作：把游戏抬到前台，否则他看到的会是别的窗口 + 一层蒙版。
    capture::bring_to_foreground(game.hwnd);
    if let Some(main) = app.get_webview_window("main") {
        let _ = main.hide();
    }

    if let Some(existing) = app.get_webview_window(LABEL) {
        // 已经开着：跟上游戏窗口当前的位置和大小（游戏可能刚挪过 / 改过分辨率）。
        let _ = existing.set_position(tauri::PhysicalPosition::new(x, y));
        let _ = existing.set_size(tauri::PhysicalSize::new(width as u32, height as u32));
        capture::bring_to_foreground(game.hwnd);
        let _ = existing.show();
        let _ = existing.set_focus();
        if let Some(main) = app.get_webview_window("main") {
            let _ = main.hide();
        }
        return Ok(());
    }

    let overlay = tauri::WebviewWindowBuilder::new(
        &app,
        LABEL,
        tauri::WebviewUrl::App("index.html?window=calibration-overlay".into()),
    )
    .title("校准 · 经验行位置")
    .decorations(false)
    .transparent(true)
    .always_on_top(true)
    .skip_taskbar(true)
    .resizable(false)
    .shadow(false)
    .visible(false)
    .build()
    .map_err(|err| format!("打开校准蒙版失败：{err}"))?;

    // 位置与大小都用**物理像素**：客户区是物理像素，中间不能让 DPI 缩放再插一手。
    overlay
        .set_position(tauri::PhysicalPosition::new(x, y))
        .map_err(|err| format!("摆放校准蒙版失败：{err}"))?;
    overlay
        .set_size(tauri::PhysicalSize::new(width as u32, height as u32))
        .map_err(|err| format!("调整校准蒙版大小失败：{err}"))?;
    // 顺序：先把蒙版亮出来并聚焦，再藏主窗口 —— 万一 show 失败，主窗口还开着，
    // 用户不会对着一个「什么都没出现、主窗口也没了」的局面。
    overlay
        .show()
        .map_err(|err| format!("显示校准蒙版失败：{err}"))?;
    overlay
        .set_focus()
        .map_err(|err| format!("聚焦校准蒙版失败：{err}"))?;
    capture::bring_to_foreground(game.hwnd);
    if let Some(main) = app.get_webview_window("main") {
        let _ = main.hide();
    }
    log::info!(
        "校准蒙版已打开：客户区 {width}×{height} @({x},{y})（游戏窗口 {:#x}）",
        game.hwnd
    );
    Ok(())
}

/// 关掉校准蒙版窗；`restore_main` = 把主窗口抬回来（保存 / 取消都走它）。
#[tauri::command]
pub fn close_calibration_overlay(app: AppHandle, restore_main: bool) {
    std::thread::spawn(move || {
        if let Some(overlay) = app.get_webview_window("calibration-overlay") {
            let _ = overlay.close();
        }
        if restore_main {
            if let Some(main) = app.get_webview_window("main") {
                let _ = main.show();
                let _ = main.unminimize();
                let _ = main.set_focus();
            }
        }
    });
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct CalibrationFrame {
    pub png_base64: String,
    pub image_w: u32,
    pub image_h: u32,
    pub client_w: i32,
    pub client_h: i32,
    /// "Windowed" | "Framed" | "Borderless" —— 顺手给用户说清我们抓的是哪种窗口
    pub screen_mode: String,
}

/// 限住前端误传的巨宽；2048×1152 的 16:9 帧原始约 9 MiB，base64 约 12 MiB，PNG 通常更小。
const MAX_CALIBRATION_WIDTH: u32 = 2048;

fn encode_calibration_png(frame: &Frame, max_width: u32) -> Result<(String, u32, u32), String> {
    if frame.width == 0 || frame.height == 0 {
        return Err("游戏客户区截图为空".to_string());
    }
    let expected_len = frame
        .width
        .checked_mul(frame.height)
        .and_then(|pixels| pixels.checked_mul(4))
        .ok_or_else(|| "游戏客户区截图尺寸无效".to_string())?;
    if frame.bgra.len() != expected_len {
        return Err("游戏客户区截图数据不完整".to_string());
    }

    let max_width = max_width.clamp(1, MAX_CALIBRATION_WIDTH) as usize;
    let image_w = frame.width.min(max_width);
    let image_h = ((frame.height as f64 * image_w as f64 / frame.width as f64).round() as usize)
        .max(1);
    let output_len = image_w
        .checked_mul(image_h)
        .and_then(|pixels| pixels.checked_mul(4))
        .ok_or_else(|| "校准预览尺寸无效".to_string())?;
    let mut rgba = vec![0; output_len];
    for y in 0..image_h {
        let source_y = y * frame.height / image_h;
        for x in 0..image_w {
            let source_x = x * frame.width / image_w;
            let source = (source_y * frame.width + source_x) * 4;
            let target = (y * image_w + x) * 4;
            rgba[target..target + 4].copy_from_slice(&[
                frame.bgra[source + 2],
                frame.bgra[source + 1],
                frame.bgra[source],
                frame.bgra[source + 3],
            ]);
        }
    }

    let mut png_bytes = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut png_bytes, image_w as u32, image_h as u32);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder
            .write_header()
            .map_err(|err| format!("编码校准截图失败：{err}"))?;
        writer
            .write_image_data(&rgba)
            .map_err(|err| format!("编码校准截图失败：{err}"))?;
    }

    use base64::Engine as _;
    Ok((
        base64::engine::general_purpose::STANDARD.encode(png_bytes),
        image_w as u32,
        image_h as u32,
    ))
}

/// 校准窗要一张**完整客户区**截图（用户在上面拖框）。
///
/// 后台截屏（PrintWindow）优先 —— 它抓的是窗口自己，游戏被挡住也拿得到；
/// 它空白且游戏在前台时，再用屏幕 DC 兜一次。点一次才抓一次，不放轮询。
#[tauri::command]
pub async fn calibration_frame(max_width: u32) -> Result<CalibrationFrame, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let game = capture::find_game_window().ok_or_else(|| "没找到游戏窗口".to_string())?;
        let (client_w, client_h) = capture::client_size(game.hwnd)
            .ok_or_else(|| "拿不到游戏客户区尺寸".to_string())?;
        let mut frame = capture::grab_client_by_method(
            game.hwnd,
            0,
            0,
            client_w,
            client_h,
            Method::Window,
        )
        .filter(|frame| !frame.pixels().looks_blank());
        if frame.is_none() && capture::is_foreground(game.hwnd) {
            frame = capture::grab_client_by_method(
                game.hwnd,
                0,
                0,
                client_w,
                client_h,
                Method::Screen,
            )
            .filter(|frame| !frame.pixels().looks_blank());
        }
        let frame = frame.ok_or_else(|| {
            format!(
                "游戏客户区抓取失败或画面为空白（{}）",
                capture::window_class_name(game.hwnd)
            )
        })?;
        let (png_base64, image_w, image_h) = encode_calibration_png(&frame, max_width)?;
        let screen_mode = match capture::screen_mode(game.hwnd) {
            ScreenMode::Windowed => "Windowed",
            ScreenMode::Framed => "Framed",
            ScreenMode::Borderless => "Borderless",
        }
        .to_string();
        Ok(CalibrationFrame {
            png_base64,
            image_w,
            image_h,
            client_w,
            client_h,
            screen_mode,
        })
    })
    .await
    .map_err(|err| format!("校准截图任务失败：{err}"))?
}

/// 已经认过的地图名（界面上的快捷按钮，省得每次手打）。
#[tauri::command]
pub fn known_exp_maps(state: State<'_, AppState>) -> Vec<String> {
    state.exp_tracker.known_places(&state.db)
}

/// 历史列表 + 汇总。
///
/// 汇总（共几段 / 累计有效时间 / 累计经验）单独从库里查，**不是把返回的列表加起来**：
/// 列表只取最近几十条，拿它当总数会把「一共练了多少」报少。
#[derive(serde::Serialize)]
pub struct ExpHistoryPayload {
    pub rows: Vec<ExpHistoryRow>,
    pub totals: ExpTotals,
}

/// `character` 给了就只看这个角色的（列表和汇总都是）。
#[tauri::command]
pub fn get_exp_history(
    limit: Option<u32>,
    character: Option<i64>,
    state: State<'_, AppState>,
) -> Result<ExpHistoryPayload, String> {
    let rows = state
        .db
        .exp_history(limit.unwrap_or(20).clamp(1, 200), character)
        .map_err(|err| format!("读经验历史失败：{}", err))?;
    let totals = state
        .db
        .exp_totals(character)
        .map_err(|err| format!("读经验汇总失败：{}", err))?;
    Ok(ExpHistoryPayload { rows, totals })
}

/// 角色列表（最近玩的在前）：名字、职业、等级、经验进度，和各自名下历史的汇总。
#[tauri::command]
pub fn list_exp_characters(
    state: State<'_, AppState>,
) -> Result<Vec<crate::db::CharacterRow>, String> {
    state
        .db
        .list_characters()
        .map_err(|err| format!("读角色列表失败：{}", err))
}

/// 给角色改名（名字是 OCR 读的，认错了字时自己改）。
#[tauri::command]
pub fn rename_exp_character(
    id: i64,
    name: String,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let name: String = name.trim().chars().take(24).collect();
    if name.is_empty() {
        return Err("名字不能是空的".to_string());
    }
    state
        .db
        .rename_character(id, &name)
        .map_err(|err| format!("改名失败：{}", err))?;
    state.exp_tracker.forget_character();
    Ok(())
}

/// 从列表里拿掉一个角色。它名下的历史留着（变成没有归属的记录）。
#[tauri::command]
pub fn delete_exp_character(id: i64, state: State<'_, AppState>) -> Result<(), String> {
    state
        .db
        .delete_character(id)
        .map_err(|err| format!("删除角色失败：{}", err))?;
    state.exp_tracker.forget_character();
    Ok(())
}

#[tauri::command]
pub fn exp_report(
    limit: u32,
    state: State<'_, AppState>,
) -> Result<crate::exp::report::ExpReport, String> {
    crate::exp::report::build(&state.db, limit)
}

#[tauri::command]
pub fn get_exp_curve(
    character: Option<i64>,
    state: State<'_, AppState>,
) -> Result<Vec<ExpCurvePoint>, String> {
    state
        .db
        .exp_curve(2_000, character)
        .map_err(|err| format!("读经验曲线失败：{}", err))
}

/// 删掉历史里的一段（练级页每一行的删除按钮）。
///
/// 删的是已经存进历史的记录，和正在统计的这一段无关；那段不存在（已经被删过）也算成功，
/// 界面刷新一下列表就对上了。
#[tauri::command]
pub fn delete_exp_session(id: i64, state: State<'_, AppState>) -> Result<(), String> {
    state
        .db
        .delete_exp_session(id)
        .map(|_| ())
        .map_err(|err| format!("删除这一段失败：{}", err))
}

#[tauri::command]
pub fn clear_exp_history(state: State<'_, AppState>) -> Result<usize, String> {
    state
        .db
        .clear_exp_history()
        .map_err(|err| format!("清空经验历史失败：{}", err))
}

#[tauri::command]
pub fn clear_exp_notice(state: State<'_, AppState>) {
    state.exp_tracker.clear_notice();
}

/// 小结卡片的存放目录：`图片\枫之助`。
///
/// 不放到程序自己的数据目录：那是给程序读写用的，用户不该去那里翻文件；
/// 而「刚打完一段，要把成绩发给群里」是个社交动作，图片就该落在用户熟悉的图片文件夹。
fn share_card_dir() -> Result<std::path::PathBuf, String> {
    #[cfg(windows)]
    {
        let home = std::env::var("USERPROFILE")
            .map_err(|_| "找不到用户目录，无法保存小结卡片".to_string())?;
        Ok(std::path::Path::new(&home).join("Pictures").join("枫之助"))
    }
    #[cfg(not(windows))]
    {
        Ok(std::env::temp_dir().join("枫之助"))
    }
}

/// 文件名只留安全的字：地图名是用户自己填的，里面什么字符都可能有。
/// 截到 20 个字，再去掉路径分隔符与 Windows 禁止的那几个字符。
fn sanitize_file_name(raw: &str) -> String {
    let cleaned: String = raw
        .trim()
        .chars()
        .filter(|ch| !matches!(ch, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|'))
        .filter(|ch| !ch.is_control())
        .take(20)
        .collect();
    let cleaned = cleaned.trim().to_string();
    if cleaned.is_empty() {
        "经验小结".to_string()
    } else {
        cleaned
    }
}

/// 生成完给用户**摆到面前**：用资源管理器选中刚存的那张图。
/// 失败就算了（无头环境 / 被安全软件拦）—— 图片已经存好，路径也已经返回给界面了。
fn reveal_in_explorer(path: &std::path::Path) {
    #[cfg(windows)]
    {
        let _ = std::process::Command::new("explorer")
            .arg(format!("/select,{}", path.display()))
            .spawn();
    }
    #[cfg(not(windows))]
    {
        let _ = path;
    }
}

/// 把前端传来的 base64 解回 PNG 字节，并且先验一下 PNG 头。
///
/// 不验的话，一段截断的 base64（IPC 传大字符串时真的会截断）会变成一张
/// 打不开的 `.png` 落在用户图片文件夹里 —— 那比直接报错难排查得多。
fn decode_png(png_base64: &str) -> Result<Vec<u8>, String> {
    use base64::Engine as _;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(png_base64.trim())
        .map_err(|err| format!("图片数据不完整（{}），请重新生成一次", err))?;
    if bytes.len() < 8 || &bytes[..4] != b"\x89PNG" {
        return Err("生成的不是一张有效的 PNG 图片".to_string());
    }
    Ok(bytes)
}

/// 保存小结卡片，返回图片的完整路径。
///
/// 卡片是**前端用 canvas 画的**（字体、配色跟界面一模一样），这里只负责：
/// 1. 把 base64 解回 PNG —— 先验一下 PNG 头，别把一段乱码写成 `.png`；
/// 2. 落盘到 `图片\枫之助\<地图或默认>-<时间戳>.png`，同名不会互相覆盖；
/// 3. 在资源管理器里选中它，用户直接拖进 QQ / 微信就能发。
#[tauri::command]
pub fn save_share_card(png_base64: String, file_name: Option<String>) -> Result<String, String> {
    let bytes = decode_png(&png_base64)?;

    let dir = share_card_dir()?;
    std::fs::create_dir_all(&dir).map_err(|err| format!("建图片目录失败：{}", err))?;
    let stem = sanitize_file_name(file_name.as_deref().unwrap_or(""));
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
    let path = dir.join(format!("{}-{}.png", stem, stamp));
    std::fs::write(&path, &bytes).map_err(|err| format!("写图片失败：{}", err))?;
    reveal_in_explorer(&path);
    log::info!("小结卡片已保存：{}", path.display());
    Ok(path.display().to_string())
}

/// 把小结卡片放进剪贴板：到微信 / QQ 的聊天框里 Ctrl+V 就是一张图。
///
/// 和存图走同一份 base64、同一道 PNG 头校验；解图 + 等剪贴板可能要几百毫秒，
/// 放到阻塞线程上做，不卡界面。
#[tauri::command]
pub async fn copy_share_card(png_base64: String) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        let bytes = decode_png(&png_base64)?;
        crate::clip_image::copy_png(&bytes)
    })
    .await
    .map_err(|err| format!("复制图片的任务失败：{err}"))?
}

/// 设定全局预设区服（拍卖查询 / 悬浮窗的默认服）。
///
/// 区服 id 必须真的存在：不校验的话，界面上保存一个不存在的 id，之后每次查询
/// 都会静默返回空结果，看起来像「站上没有这个道具」。
#[tauri::command]
pub fn set_default_server(
    app: AppHandle,
    server_id: u32,
    state: State<'_, AppState>,
) -> Result<u32, String> {
    if !crate::mxdc_client::SERVER_IDS.contains(&server_id) {
        return Err(format!("区服 id {} 不存在", server_id));
    }

    state
        .db
        .set_setting("default_server_id", &server_id.to_string())
        .map_err(|e| format!("保存预设区服失败: {}", e))?;

    log::info!("全局预设区服 → {}", server_id);

    // 主窗口和悬浮窗是两个 WebView：一个改了默认服，另一个要跟着切，
    // 否则用户会看到两边显示不同区服的物价。
    if let Err(err) = app.emit("default-server-changed", server_id) {
        log::debug!("广播预设区服失败: {}", err);
    }

    Ok(server_id)
}

/// 真正结束进程。
///
/// 之前只有右上角的 X 一条路，而 HUD 是常驻的隐藏窗口，所以关掉主窗口
/// 并不会让进程退出 —— 用户看到的就是「点了退出，进程还在」。
#[tauri::command]
pub fn quit_app(app: AppHandle) {
    log::info!("用户要求退出程序");
    app.exit(0);
}

#[tauri::command]
pub fn set_memory_threshold(threshold: u32, state: State<'_, AppState>) -> Result<(), String> {
    state.memory_monitor.set_threshold(threshold);
    state
        .db
        .set_setting("memory_threshold", &threshold.to_string())
        .map_err(|e| format!("数据库更新失败: {}", e))?;
    Ok(())
}

/// 开 / 关内存到线提醒（立刻生效；设置页和总览页的两个开关写的是同一个值）。
///
/// 独立命令、不塞进 `patch_settings`：它除了写库还要更新 `MemoryMonitor` 里的
/// 开关和判定状态，和 `set_blacklist_watch` 同类。
#[tauri::command]
pub fn set_memory_alert(enabled: bool, state: State<'_, AppState>) -> Result<(), String> {
    state.memory_monitor.set_alert_enabled(enabled);
    state
        .db
        .set_setting("memory_alert_enabled", if enabled { "1" } else { "0" })
        .map_err(|e| format!("数据库更新失败: {}", e))?;
    log::info!(
        "内存到线提醒：{}",
        if enabled { "已开启" } else { "已关闭" }
    );
    Ok(())
}

/// 呼出 / 收起**查价悬浮窗**（侧栏按钮和 Alt+F 走的是同一个效果）。
///
/// 窗口操作一律**投递出去**（`thread::spawn`），不在命令函数体里同步做：
/// 同步命令在 Windows 上跑在 UI 线程、WebView2 的回调栈里，在那里调
/// `show() / hide() / set_focus()` 会重入事件循环 —— 这个项目的登录窗白屏、
/// 单实例卡死都是同一类根因（见 `tests/ui_thread_callbacks.rs`）。
#[tauri::command]
pub fn toggle_hud(app: AppHandle) {
    let app = app.clone();
    std::thread::spawn(move || crate::toggle_hud_window(&app));
}

#[tauri::command]
pub fn hide_hud(app: AppHandle) {
    crate::hide_window(&app, "hud");
}

#[tauri::command]
pub fn show_hud(app: AppHandle) {
    crate::reveal_window(&app, "hud");
}

/// 道具图鉴详情（官方基准属性 + NPC 出售价）。
///
/// 图鉴资料几乎不变，缓存一整天，反复点「详情」不会每次都打站点。
#[tauri::command]
pub async fn get_item_detail(
    item_id: String,
    state: State<'_, AppState>,
) -> Result<ItemDetail, String> {
    let id = item_id.trim();
    if id.is_empty() {
        return Err("缺少道具 ID".to_string());
    }

    let cache_key = format!("item_detail_{}", id);
    if let Some((cached_json, _)) = state.db.get_cached_price(&cache_key, 24 * 3600) {
        if let Ok(detail) = serde_json::from_str::<ItemDetail>(&cached_json) {
            log::info!("道具图鉴 {} 命中缓存：{}", id, detail.name);
            return Ok(detail);
        }
    }

    let detail = state.mxdc_client.get_item_detail(id).await?;

    match serde_json::to_string(&detail) {
        Ok(json) => {
            let _ = state.db.set_cached_price(&cache_key, &json);
        }
        Err(err) => log::warn!("序列化道具图鉴失败: {}", err),
    }

    log::info!(
        "道具图鉴 {}：{}（装备={}，属性 {} 条，售价「{}」）",
        id,
        detail.name,
        detail.equipment,
        detail.props.len(),
        detail.sell_price
    );

    Ok(detail)
}

/// 掉落速查：怪物名 / 道具名 / 怪物ID / 道具ID 都走这一个命令。
///
/// 三个刻意的做法：
///
/// * **公开接口不需要登录** —— 朋友拿到工具、没登录小册子也能用（这是这个功能
///   最“能分享”的一点）；
/// * 掉落资料是**静态数据**（不像金价会变），所以缓存 1 小时；
/// * **空结果不进缓存**：站点偶发地会回一个“没结果”的响应（数据没准备好 / 抖动），
///   一旦写进缓存，用户接下来一小时都会查到空 —— 和拍卖查询踩的是同一个坑。
#[tauri::command]
pub async fn search_drops(
    keyword: String,
    page: Option<u32>,
    state: State<'_, AppState>,
) -> Result<crate::mxdc_client::DropSearch, String> {
    let trimmed = keyword.trim();
    if trimmed.is_empty() {
        return Err("请输入怪物名、道具名或 ID".to_string());
    }
    if trimmed.chars().count() > 32 {
        return Err("关键词太长了".to_string());
    }
    let page = page.unwrap_or(1).max(1);

    let cache_key = format!("drops:{}:{}", trimmed, page);
    if let Some((cached_json, _)) =
        state
            .db
            .get_cached_price(&cache_key, crate::mxdc_client::DROP_CACHE_TTL_SEC)
    {
        if let Ok(hit) = serde_json::from_str::<crate::mxdc_client::DropSearch>(&cached_json) {
            log::info!("掉落查询「{}」第 {} 页命中缓存", trimmed, page);
            return Ok(hit);
        }
    }

    let result = state.mxdc_client.search_drops(trimmed, page).await?;

    let has_content =
        !result.results.is_empty() || !result.matches.items.is_empty() || !result.matches.mobs.is_empty();
    if has_content {
        match serde_json::to_string(&result) {
            Ok(json) => {
                let _ = state.db.set_cached_price(&cache_key, &json);
            }
            Err(err) => log::warn!("序列化掉落结果失败: {}", err),
        }
    }

    log::info!(
        "掉落查询「{}」第 {} 页：命中怪物 {} / 道具 {}，返回 {} 只怪物的掉落（共 {} 条）",
        trimmed,
        page,
        result.matches.mobs.len(),
        result.matches.items.len(),
        result.results.len(),
        result.meta.drop_total
    );

    Ok(result)
}

/// 一次物价查询的结果，除了条目本身还把“数据从哪来”一并告诉界面。
#[derive(Debug, Clone, serde::Serialize)]
pub struct MarketQueryResult {
    pub items: Vec<MarketItem>,
    /// "live" = 本次真的去小册子抓的；"cache" = 命中的本地缓存
    pub source: String,
    /// source = "cache" 时，这份缓存是几秒前写进去的
    pub cached_seconds_ago: Option<i64>,
}

fn cache_market_items(db: &Database, cache_key: &str, items: &[MarketItem]) -> Result<bool, String> {
    // 空结果可能是站点抖动造成的假阴性，缓存后会把两个窗口都锁进 5 分钟的空白。
    if items.is_empty() {
        return Ok(false);
    }
    let json = serde_json::to_string(items).map_err(|err| format!("序列化物价缓存失败：{err}"))?;
    db.set_cached_price(cache_key, &json)
        .map_err(|err| format!("写入物价缓存失败：{err}"))?;
    Ok(true)
}

async fn query_market_items(
    state: &AppState,
    keyword: &str,
    server_id: u32,
    force_refresh: bool,
    caller: &str,
) -> Result<MarketQueryResult, String> {
    let keyword = keyword.trim();
    if keyword.is_empty() {
        return Ok(MarketQueryResult {
            items: Vec::new(),
            source: "live".to_string(),
            cached_seconds_ago: None,
        });
    }

    let cache_key = format!("market_{}_{}", server_id, keyword);
    if !force_refresh {
        if let Some((cached_json, updated_at)) = state.db.get_cached_price(&cache_key, 300) {
            if let Ok(items) = serde_json::from_str::<Vec<MarketItem>>(&cached_json) {
                let age = chrono::Utc::now().timestamp() - updated_at;
                log::info!(
                    "[{}] 拍卖查询 {} 命中缓存（{} 秒前写入）：{} 条",
                    caller,
                    cache_key,
                    age,
                    items.len()
                );
                return Ok(MarketQueryResult {
                    items,
                    source: "cache".to_string(),
                    cached_seconds_ago: Some(age),
                });
            }
        }
    }

    let cookie = state.db.get_setting("auth_cookie").unwrap_or_default();
    log::info!(
        "[{}] 拍卖查询 {} {}，开始抓取（cookie {} 字符）",
        caller,
        cache_key,
        if force_refresh { "强制刷新" } else { "未命中缓存" },
        cookie.len()
    );
    let items = state
        .mxdc_client
        .search_market(keyword, server_id, &cookie)
        .await?;

    match cache_market_items(&state.db, &cache_key, &items) {
        Ok(true) => log::info!(
            "[{}] 拍卖查询 {} 抓到 {} 条并写入缓存",
            caller,
            cache_key,
            items.len()
        ),
        Ok(false) => log::info!("[{}] 拍卖查询 {} 没有结果，不写入缓存", caller, cache_key),
        Err(err) => log::warn!("[{}] {}", caller, err),
    }

    Ok(MarketQueryResult {
        items,
        source: "live".to_string(),
        cached_seconds_ago: None,
    })
}

/// 拍卖查询：先看缓存，缓存过期（或强制刷新）就现抓一次。
#[tauri::command]
pub async fn query_market(
    keyword: String,
    server_id: u32,
    // 忽略缓存、强制重新抓一次（界面上的「强制刷新」按钮）
    force_refresh: bool,
    webview: tauri::Webview,
    state: State<'_, AppState>,
) -> Result<MarketQueryResult, String> {
    let caller = webview.label().to_string();
    query_market_items(&state, &keyword, server_id, force_refresh, &caller).await
}

#[tauri::command]
pub async fn get_meso_report(state: State<'_, AppState>) -> Result<MesoReport, String> {
    load_meso_report(&state.db, &state.mxdc_client).await
}

/// 取金价报表（带 10 分钟缓存）。界面的「行情」和后台的金价到价提醒共用这一个入口，
/// 所以后台多看一眼不会多打站点。
pub async fn load_meso_report(db: &Database, client: &MxdcClient) -> Result<MesoReport, String> {
    let cache_key = "meso_report_all";

    // 当前报价与历史走势来自同一个页面，一次取回、一次缓存。
    // TTL 对齐站点自己的采集/刷新间隔（10 分钟）。
    if let Some((cached_json, _)) = db.get_cached_price(cache_key, 600) {
        if let Ok(report) = serde_json::from_str::<MesoReport>(&cached_json) {
            return Ok(report);
        }
    }

    let cookie = db.get_setting("auth_cookie").unwrap_or_default();
    let report = client.get_meso_report(&cookie).await?;

    if let Ok(json_str) = serde_json::to_string(&report) {
        let _ = db.set_cached_price(cache_key, &json_str);
    }

    Ok(report)
}

/// 金价到价提醒的当前设置。
#[tauri::command]
pub fn get_meso_alert(state: State<'_, AppState>) -> MesoAlertConfig {
    meso_alert::load_config(&state.db)
}

/// 改金价到价提醒（改完立刻生效，并马上查一次 —— 已经满足的条件不用等十分钟才响）。
#[tauri::command]
pub fn set_meso_alert(
    config: MesoAlertConfig,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<MesoAlertConfig, String> {
    meso_alert::save_config(&state.db, &config)?;
    log::info!(
        "金价到价提醒 → {}（涨到 {:?} / 跌到 {:?}）",
        if config.enabled { "开" } else { "关" },
        config.above,
        config.below
    );
    tauri::async_runtime::spawn(async move { meso_alert::check_once(&app).await });
    Ok(config)
}

#[tauri::command]
pub fn get_blacklist(state: State<'_, AppState>) -> Result<Vec<BlacklistEntry>, String> {
    state
        .db
        .get_blacklist()
        .map_err(|e| format!("查询黑名单失败: {}", e))
}

#[tauri::command]
pub fn add_blacklist(
    entry: BlacklistEntry,
    state: State<'_, AppState>,
) -> Result<i64, String> {
    if entry.player_name.trim().is_empty() {
        return Err("角色名不能为空".to_string());
    }
    state
        .db
        .add_blacklist(&entry)
        .map_err(|e| format!("添加黑名单失败: {}", e))
}

#[tauri::command]
pub fn remove_blacklist(id: i64, state: State<'_, AppState>) -> Result<(), String> {
    state
        .db
        .remove_blacklist(id)
        .map_err(|e| format!("删除黑名单失败: {}", e))
}

/// 查一个人：返回**四档**结论（确定命中 / 只在别的服记过 / 名字很像 / 本地无记录）。
///
/// 为什么不只返回「有 / 没有」：游戏里同名很常见，一个二值的红绿灯会把好人判死
/// （反面也一样：说「安全」等于给骗子担保）。所以这里把「能确认的」和「只是巧合的」
/// 分开，并在 `note` 里写清楚结论是怎么来的。
///
/// `server` 不传时用全局预设区服 —— 界面就不用自己传了。
#[tauri::command]
pub fn check_blacklist(
    name: String,
    server: Option<String>,
    state: State<'_, AppState>,
) -> Result<blacklist::BlacklistVerdict, String> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err("请先输入要查的角色名".to_string());
    }
    if trimmed.chars().count() > 24 {
        return Err("这看起来不是角色名（太长了）".to_string());
    }
    blacklist::check(&state.db, trimmed, server)
}

/// 开 / 关剪贴板嗅探（立刻生效，不用等「保存全部配置」）。
#[tauri::command]
pub fn set_blacklist_watch(enabled: bool, state: State<'_, AppState>) -> Result<(), String> {
    state
        .db
        .patch_settings(None, None, None, Some(enabled))
        .map_err(|e| format!("保存设置失败: {}", e))?;
    log::info!(
        "剪贴板黑名单嗅探：{}",
        if enabled { "已开启" } else { "已关闭" }
    );
    Ok(())
}

#[tauri::command]
pub fn list_alert_history(limit: u32, state: State<'_, AppState>) -> Vec<AlertHistoryRow> {
    state.db.list_alert_history(limit.min(500))
}

#[tauri::command]
pub fn clear_alert_history(state: State<'_, AppState>) -> Result<(), String> {
    state
        .db
        .clear_alert_history()
        .map_err(|err| format!("清空提醒历史失败：{err}"))
}

#[tauri::command]
pub fn import_blacklist(
    json_data: String,
    state: State<'_, AppState>,
) -> Result<usize, String> {
    let entries: Vec<BlacklistEntry> =
        serde_json::from_str(&json_data).map_err(|e| format!("JSON 解析失败: {}", e))?;

    let mut count = 0;
    for entry in entries {
        if !entry.player_name.trim().is_empty() {
            if state.db.add_blacklist(&entry).is_ok() {
                count += 1;
            }
        }
    }
    Ok(count)
}

#[tauri::command]
pub fn export_blacklist(state: State<'_, AppState>) -> Result<String, String> {
    let list = state
        .db
        .get_blacklist()
        .map_err(|e| format!("导出黑名单失败: {}", e))?;
    serde_json::to_string_pretty(&list).map_err(|e| format!("序列化失败: {}", e))
}

/// 某个悬浮窗当前是不是开着（设置页的「测试呼出」按钮用它回显结果）。
#[tauri::command]
pub fn get_float_window_visible(app: AppHandle, action: String) -> Result<bool, String> {
    let target = match action.as_str() {
        "price_hud" => HotkeyAction::PriceHud,
        "lite_float" => HotkeyAction::LiteFloat,
        other => return Err(format!("未知的悬浮窗：{}", other)),
    };
    Ok(hotkeys::window_visible(&app, target))
}

/// 呼出 / 收起**精简悬浮窗**（当前金价 + 内存）。
///
/// 和查价悬浮窗是两个独立窗口：互不遮挡、各自记忆位置，快捷键也各一个。
#[tauri::command]
pub fn toggle_lite_float(app: AppHandle) {
    let app = app.clone();
    std::thread::spawn(move || crate::toggle_float_window(&app));
}

#[tauri::command]
pub fn hide_lite_float(app: AppHandle) {
    let app = app.clone();
    std::thread::spawn(move || crate::hide_window(&app, "float"));
}

/// 手动测一遍开服提醒（响铃 + 任务栏闪烁 + 右下角提醒窗）。
///
/// 为什么要有这个按钮：「开服了到底会不会响」只能等真的开服才知道 —— 用户没法验，
/// 我们也没法验。所以把提醒抽成 `alert::fire_open_alert`，让这里和真正的开服
/// **走完全同一条路径**：点一次能听到、能看到东西，那真开服时就一定会提醒你。
///
/// 提醒内容是**上一次真实报警的原文**（如果这轮运行里报过警），这样测试不只是
/// 「能不能弹窗」，还能顺手回看「刚才那条提醒到底说了什么」。
///
/// `async`：窗口创建与通知插件在 Windows 上都要碰 COM / WinRT，不该占着 UI 线程跑。
#[tauri::command]
pub async fn test_open_alert(app: AppHandle) -> Result<(), String> {
    // 冷却期内直接返回 false，但界面上的按钮不该“点了没反应”，
    // 所以这里主动把冷却说明写进提醒正文。
    let cooldown_hint = alert::cooldown_remaining_sec();

    let body = match cooldown_hint {
        Some(seconds) => format!(
            "这一条是测试提醒（距上次真实提醒 {} 秒，同一次开服只提醒一次是故意的）。\n响铃、任务栏闪烁、右下角弹窗与系统通知，真开服时走的都是这条路径。",
            seconds
        ),
        None => "这一条是测试提醒：响铃、任务栏闪烁、右下角弹窗与系统通知，\n真开服时走的都是这条路径。\n如果这时能看到右下角的提醒窗，说明真开服时一定会提醒你。"
            .to_string(),
    };

    fire_test_alert(&app, body);
    Ok(())
}

/// 测试提醒绕过冷却：否则刚报过一次警的用户点「测试」会什么都不发生，
/// 而那个按钮的全部意义就是「让我确认它真的会响」。
fn fire_test_alert(app: &AppHandle, body: String) {
    alert::fire_open_alert_force(app, AlertSource::Test, body);
}

/// 提醒窗的内容（它挂载后会主动来取一次，因为「先建窗、后加载页面」期间
/// 发出去的事件一定会丢）。
#[tauri::command]
pub fn get_latest_open_alert() -> Option<AlertPayload> {
    alert::latest_alert()
}

/// 用户点「知道了」：关掉提醒窗（与其说关窗，不如说是把那个 WebView2 进程
/// 收回去 —— 这是个内存监控工具，不该自己常驻几十 MB）。
#[tauri::command]
pub fn dismiss_open_alert(app: AppHandle) {
    alert::close_alert_window(&app);
}

/// 提醒窗上的「打开工具箱」：把主窗口抬到最前。
///
/// 这里可以抢焦点：用户是主动点的，和「开服时自动把游戏踢到后台」完全不同。
#[tauri::command]
pub fn focus_main_window(app: AppHandle) {
    let app = app.clone();
    std::thread::spawn(move || {
        if let Some(window) = app.get_webview_window("main") {
            let _ = window.show();
            let _ = window.unminimize();
            let _ = window.set_focus();
        }
    });
}

/// 小册子「开服监控」当前状态（站点每分钟探一次登录入口）。
#[tauri::command]
pub fn get_server_monitor_status(state: State<'_, AppState>) -> MonitorStatus {
    state.monitor_manager.status()
}

/// 手动立刻重拉一次开服监控（设置页的「立即刷新」按钮）。
#[tauri::command]
pub async fn refresh_server_monitor(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<MonitorStatus, String> {
    if let Err(err) = state.monitor_manager.refresh_once(&app).await {
        // 失败也把状态返回去：界面要显示「取不到数据」的原因，
        // 而不是弹一个错误的红字。
        log::warn!("手动刷新开服监控失败: {}", err);
    }
    Ok(state.monitor_manager.status())
}

#[tauri::command]
pub fn save_auth_cookie(cookie: String, state: State<'_, AppState>, app: AppHandle) -> Result<(), String> {
    state
        .db
        .set_setting("auth_cookie", &cookie)
        .map_err(|e| format!("保存 Cookie 失败: {}", e))?;

    // Close login window if open（同样投递出去，别在命令体里同步关窗）
    crate::close_window(&app, "login");

    Ok(())
}

/// 外链只允许 HTTPS：本项目的外站和更新页都是 HTTPS；HTTP 会降级传输，
/// 其他 scheme 可能交给 Windows 打开本地文件、SMB 共享或任意协议处理器。
pub(crate) fn validate_external_url(raw: &str) -> Result<reqwest::Url, String> {
    let url = reqwest::Url::parse(raw.trim())
        .map_err(|_| "只允许打开 HTTPS 网页地址".to_string())?;
    if url.scheme() != "https" || url.host_str().is_none() {
        return Err("只允许打开 HTTPS 网页地址".to_string());
    }
    Ok(url)
}

#[tauri::command]
pub fn open_external_url(url: String) -> Result<(), String> {
    let url = validate_external_url(&url)?;
    #[cfg(windows)]
    {
        std::process::Command::new("rundll32")
            .args(["url.dll,FileProtocolHandler", url.as_str()])
            .spawn()
            .map_err(|e| format!("无法打开浏览器: {}", e))?;
    }
    #[cfg(not(windows))]
    {
        let _ = open::that(url.as_str());
    }
    Ok(())
}

/// 一次 Cookie 体检的结论。**必须是三态**，不能是「有效 / 无效」二选一。
///
/// 以前只有真假两种返回值，而只有收到 401 才算失败：网络不通、站点 502、
/// 被 CDN 拦下……统统落进「其它错误」，然后被当成**验证成功**写库，界面上还显示
/// 「验证成功！Cookie 已注入并生效」。用户看到的是「验证通过了，但查价还是提示
/// 未登录」——把「我们没验成」说成「验过了且有效」，比直接说失败更糟。
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CookieProbe {
    /// 真的通了：请求带着这份 Cookie 查到了行情
    Valid,
    /// 站点明确说没登录 / 没权限
    Invalid,
    /// 没能得出结论（网络问题、站点故障）—— 两种都不能说成「有效」
    Unverified,
}

impl CookieProbe {
    /// 这份 Cookie 值不值得存下来。
    ///
    /// 连不上的时候也存：用户就是手动粘进来的，拒绝保存只会让他重来一遍；
    /// 但界面上必须说清「存了，但没验成」。
    pub fn should_save(self) -> bool {
        !matches!(self, CookieProbe::Invalid)
    }
}

/// 把「探测请求的结果」翻译成三态结论。抽成纯函数是为了能测：
/// 这里错一次，用户就会在「验证成功」的假象里继续查不到东西。
fn classify_cookie_probe(result: Result<(), String>) -> CookieProbe {
    match result {
        Ok(()) => CookieProbe::Valid,
        Err(err) => {
            let lower = err.to_ascii_lowercase();
            // 站点自己说「未登录 / 无权限」：401、403 都算
            if lower.contains("401") || lower.contains("403") {
                CookieProbe::Invalid
            } else {
                CookieProbe::Unverified
            }
        }
    }
}

#[tauri::command]
pub async fn test_cookie_validity(
    cookie: String,
    state: State<'_, AppState>,
) -> Result<CookieProbe, String> {
    let c = cookie.trim();
    if c.is_empty() {
        return Ok(CookieProbe::Invalid);
    }

    // 用真的需要登录的接口去验，而不是看「本地有没有这个字段」
    let probe = state
        .mxdc_client
        .search_market("锅盖", 1, c)
        .await
        .map(|_| ());
    let verdict = classify_cookie_probe(probe);

    if verdict.should_save() && state.db.set_setting("auth_cookie", c).is_err() {
        return Err("验证通过但保存 Cookie 失败，请重试".to_string());
    }

    log::info!("Cookie 体检结论：{:?}", verdict);
    Ok(verdict)
}

#[tauri::command]
pub fn close_login_window(app: AppHandle) {
    crate::close_window(&app, "login");
}

/// 进入改键捕获模式：临时注销全局快捷键。
///
/// 必须由界面在「开始捕获」时调用 —— 全局快捷键是 OS 级的，键被注册时
/// 系统会把按键直接吞掉，webview 收不到 keydown，于是「想把 F10 改成别的键」
/// 时按 F10 毫无反应、还顺手把悬浮窗呼了出来（详见 `hotkeys::pause` 的说明）。
#[tauri::command]
pub fn begin_hotkey_capture(app: AppHandle) {
    hotkeys::pause(&app);
}

/// 退出改键捕获模式：把快捷键挂回去（照最近一次收到的配置），并返回真实结果。
///
/// 返回值就是界面要显示的那份状态 —— 捕获成功、Esc 取消、点到别处、离开设置页
/// 都会走到这里，所以不会出现「改键过程中快捷键被永久注销」的状态。
#[tauri::command]
pub fn end_hotkey_capture(app: AppHandle) -> Vec<HotkeyStatus> {
    hotkeys::resume(&app)
}

/// 当前内存占用（界面刚打开时先来问一次，别用编造的初始值占着屏幕）。
#[tauri::command]
pub fn get_memory_status(state: State<'_, AppState>) -> Option<MemoryPayload> {
    state.memory_monitor.latest()
}

/// 版本自检：去内置的「更新页」看一眼有没有新版本。
///
/// **不带参数**：地址是编译进二进制的常量（`update::DEFAULT_UPDATE_URL`，一个夸克
/// 分享页），界面上没有任何地方能改 —— 让用户自己去填一个网盘地址，等于把
/// 「有没有新版本」这件事交给他配置，而他要的明明是「它自己看一眼」。
/// **只回答「有没有新版本」**，不下载、不替换、不安装 —— 便携版自己替换 exe 的失败
/// 模式是「朋友的程序坏了」，不值当（详见 `update.rs` 的开头说明）。
#[tauri::command]
pub async fn check_for_update() -> Result<crate::update::UpdateInfo, String> {
    Ok(crate::update::check(crate::update::DEFAULT_UPDATE_URL).await)
}

/// 内置更新页地址（基础配置页里那个「更新页」链接）。
///
/// 界面上**没有输入框**，只是把编译进程序里的那个地址显示/打开出来：用户想自己
/// 去看一眼有没有新版本时有个去处，而不用去问谁要链接。地址只有一处定义
/// （`update::DEFAULT_UPDATE_URL`），界面不自己写死，免得两边不一致。
#[tauri::command]
pub fn get_update_page_url() -> String {
    crate::update::page_url()
}

#[tauri::command]
pub async fn download_update(app: AppHandle, url: String) -> Result<String, String> {
    let desktop = app
        .path()
        .desktop_dir()
        .map_err(|err| format!("无法找到桌面目录：{err}"))?;
    let path = crate::update::download_to_desktop(&url, &desktop).await?;
    reveal_in_explorer(&path);
    log::info!("更新包已保存到桌面：{}", path.display());
    Ok(path.display().to_string())
}

// NOTE: this command MUST stay `async`.
// On Windows a synchronous command runs on the UI thread, inside WebView2's
// callback. Building a new WebView2 controller there re-enters WebView2 and
// deadlocks: the OS window appears but stays blank/white and never pumps
// messages, so it cannot be closed either.
// Tauri documents this on `WebviewWindowBuilder::new`: "On Windows, this
// function deadlocks when used in a synchronous command and event handlers.
// You should use `async` commands and separate threads when creating windows."
// See https://github.com/tauri-apps/tauri/issues/13963
#[tauri::command]
pub async fn open_login_window(app: AppHandle) -> Result<(), String> {
    if let Some(existing) = app.get_webview_window("login") {
        let _ = existing.show();
        let _ = existing.set_focus();
        return Ok(());
    }

    // Open internal view: index.html?window=login
    let webview_builder = tauri::WebviewUrl::App("index.html?window=login".into());

    let _win = tauri::WebviewWindowBuilder::new(&app, "login", webview_builder)
        .title("小册子账号授权绑定")
        .inner_size(520.0, 640.0)
        .resizable(false)
        .decorations(true)
        .always_on_top(true)
        .center()
        .build()
        .map_err(|e| format!("打开登录窗口失败: {}", e))?;

    Ok(())
}

// ---------------------------------------------------------------------------
// 微信扫码登录（内嵌 WebView2 打开小册子，登录后自动提取会话 Cookie）
// ---------------------------------------------------------------------------

/// 小册子站点首页：扫码窗口的入口，也是要提取 Cookie 的归属域。
const MXDC_HOME: &str = "https://mxdc.dvg.cn/";

/// 与 `MxdcClient` 用同一套 UA —— 站点已经接受过这个 UA 的请求。
const BROWSER_UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/133.0.0.0 Safari/537.36";

/// 注入到站点页面顶部的提示条（扫码窗口加载的是站点本身，没有我们自己的 UI）。
const QR_HINT_SCRIPT: &str = r#"
(function () {
  var ID = '__mxd_box_qr_hint__';
  function mount() {
    if (document.getElementById(ID) || !document.body) return;
    var bar = document.createElement('div');
    bar.id = ID;
    bar.style.cssText = 'position:fixed;top:0;left:0;right:0;z-index:2147483647;' +
      'background:#0d1117;color:#fbbf24;padding:6px 12px;text-align:center;' +
      'font:600 12px/1.6 "Microsoft YaHei",sans-serif;' +
      'border-bottom:1px solid rgba(255,255,255,.15)';
    bar.textContent = '扫码登录后无需任何操作 · 工具箱会自动保存凭证并关闭本窗口';
    document.body.appendChild(bar);
  }
  if (document.readyState === 'loading') {
    document.addEventListener('DOMContentLoaded', mount);
  } else {
    mount();
  }
})();
"#;

/// 打开内嵌的「微信扫码登录」窗口。
///
/// 同样是 `async`：Windows 上在同步命令里 `build()` 会死锁（见 `open_login_window`）。
#[tauri::command]
pub async fn open_qr_login_window(app: AppHandle) -> Result<(), String> {
    if let Some(existing) = app.get_webview_window("login-qr") {
        let _ = existing.show();
        let _ = existing.set_focus();
        return Ok(());
    }

    let url = MXDC_HOME
        .parse::<tauri::Url>()
        .map_err(|e| format!("无效的扫码地址: {}", e))?;

    tauri::WebviewWindowBuilder::new(&app, "login-qr", tauri::WebviewUrl::External(url))
        .title("微信扫码登录小册子")
        .inner_size(460.0, 820.0)
        .resizable(true)
        .decorations(true)
        .always_on_top(true)
        .center()
        .user_agent(BROWSER_UA)
        .initialization_script(QR_HINT_SCRIPT)
        .build()
        .map_err(|e| format!("打开扫码窗口失败: {}", e))?;

    spawn_login_cookie_watcher(app);

    Ok(())
}

/// 后台盯着扫码窗口的 Cookie：一旦 Cookie 真的能通过行情接口，就写库、
/// 通知界面、关掉扫码窗口。
///
/// 扫描窗口加载的是站点页面，我们自己的界面不在里面，所以这个轮询只能放在 Rust 侧。
/// 注意 `cookies_for_url` 内部是「投递消息 + 阻塞等待」：从主线程调用会死锁
/// （Tauri 官方文档在 `WebviewWindow::cookies_for_url` 上也挂了同样的警告），
/// 因此这里必须跑在 async runtime 的工作线程上。
fn spawn_login_cookie_watcher(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        let poll = std::time::Duration::from_millis(1200);
        // 未登录时站点也会下发 PHPSESSID，所以「Cookie 有值」不等于「已登录」，
        // 必须靠真实请求去验，并且对同一份 Cookie 做节流。
        let retry_gap = std::time::Duration::from_secs(3);
        let grace = std::time::Duration::from_secs(45);

        let started = std::time::Instant::now();
        let mut last_tried = String::new();
        let mut last_attempt = started - retry_gap;
        let mut hinted = false;

        loop {
            tokio::time::sleep(poll).await;

            // 用户手动关掉窗口就结束轮询
            let Some(win) = app.get_webview_window("login-qr") else {
                break;
            };

            let Ok(url) = MXDC_HOME.parse::<tauri::Url>() else {
                break;
            };

            // 这一步只读本地 Cookie jar，不走网络，所以可以高频轮询
            let jar = match win.cookies_for_url(url) {
                Ok(cookies) => cookies
                    .iter()
                    .map(|c| format!("{}={}", c.name(), c.value()))
                    .collect::<Vec<_>>()
                    .join("; "),
                Err(_) => continue,
            };

            if jar.is_empty() {
                continue;
            }

            if !hinted && started.elapsed() > grace {
                hinted = true;
                let _ = app.emit("login-cookie-timeout", ());
            }

            if jar == last_tried || last_attempt.elapsed() < retry_gap {
                continue;
            }
            last_tried = jar.clone();
            last_attempt = std::time::Instant::now();

            let (db, client) = {
                let state = app.state::<AppState>();
                (state.db.clone(), state.mxdc_client.clone())
            };

            // 只有真正查得动行情才算登录成功；未登录 / 网络抖动都继续等下一份 Cookie
            if client.search_market("锅盖", 1, &jar).await.is_err() {
                continue;
            }

            if db.set_setting("auth_cookie", &jar).is_err() {
                continue;
            }

            let _ = app.emit("login-cookie-updated", &jar);
            if let Some(win) = app.get_webview_window("login-qr") {
                let _ = win.close();
            }
            break;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 「验证成功」这四个字是用户判断「Cookie 到底有没有用」的唯一依据，
    /// 所以三种结论必须严格区分：站点说没登录 = 无效；我们压根没连上 = 无法验证。
    /// 以前这两种都返回「成功」，用户会一直在「验证通过但查不到东西」里绕。
    #[test]
    fn cookie_probe_never_calls_a_network_failure_a_success() {
        assert_eq!(classify_cookie_probe(Ok(())), CookieProbe::Valid);

        // 站点明确拒绝：401（未登录）/ 403（无权限）
        assert_eq!(
            classify_cookie_probe(Err(
                "401 Unauthorized: 查询行情需要先登录小册子账号".to_string()
            )),
            CookieProbe::Invalid
        );
        assert_eq!(
            classify_cookie_probe(Err("小册子返回 HTTP 403".to_string())),
            CookieProbe::Invalid
        );

        // 其余一切都只能算「没验成」：网络不通、站点 5xx、被 CDN 拦下……
        assert_eq!(
            classify_cookie_probe(Err("error sending request for url (https://mxdc.dvg.cn/)".into())),
            CookieProbe::Unverified
        );
        assert_eq!(
            classify_cookie_probe(Err("小册子返回 HTTP 502（https://mxdc.dvg.cn/api）".into())),
            CookieProbe::Unverified
        );
        assert_eq!(
            classify_cookie_probe(Err("未能从小册子拍卖页取得查询凭证（未登录或页面结构已变化）。".into())),
            CookieProbe::Unverified
        );
    }

    /// 无效的 Cookie 不落库：存下去只会让之后每次查询都带着一份过期的凭证，
    /// 而界面上还写着「已注入并生效」。其余两种都存（是用户手动粘进来的）。
    #[test]
    fn only_invalid_cookies_are_refused() {
        assert!(CookieProbe::Valid.should_save());
        assert!(CookieProbe::Unverified.should_save());
        assert!(!CookieProbe::Invalid.should_save());
    }

    #[test]
    fn empty_market_results_are_never_written_to_cache() {
        let path = std::env::temp_dir().join(format!(
            "mxdbox-test-market-cache-{}.db",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);
        let db = Database::new(path.clone()).expect("建库失败");
        assert!(!cache_market_items(&db, "market_1_test", &[]).expect("缓存写入失败"));
        assert!(db.get_cached_price("market_1_test", 300).is_none());
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn external_urls_only_allow_https_web_addresses() {
        let safe = validate_external_url(" https://mxdc.dvg.cn/tools/ ").expect("HTTPS 网页应放行");
        assert_eq!(safe.scheme(), "https");
        assert_eq!(safe.host_str(), Some("mxdc.dvg.cn"));

        for url in [
            "http://example.com/",
            "file:///C:/Windows/System32/calc.exe",
            r"\\server\share\payload.exe",
            "javascript:alert(1)",
            "//example.com/path",
            "",
        ] {
            assert!(validate_external_url(url).is_err(), "应拒绝：{url:?}");
        }
    }

    /// 地图名是用户手填的，会直接进文件名：路径分隔符和 Windows 禁用字符必须滤掉、
    /// 超长的要截断、空名字要有兜底 —— 否则「生成卡片」会失败在一个文件名不合法上，
    /// 而用户完全看不出自己哪里填错了。
    #[test]
    fn share_card_file_names_are_sanitized() {
        assert_eq!(sanitize_file_name(""), "经验小结");
        assert_eq!(sanitize_file_name("   "), "经验小结");
        assert_eq!(sanitize_file_name("蚂蚁洞"), "蚂蚁洞");
        // 路径分隔符不能留下来：否则卡片会被写到别的目录去
        assert!(!sanitize_file_name("../..\\windows\\system32").contains('/'));
        assert!(!sanitize_file_name("../..\\windows\\system32").contains('\\'));
        assert_eq!(sanitize_file_name("a:b*c?d"), "abcd");
        assert_eq!(sanitize_file_name(" 蘑菇神社 "), "蘑菇神社");
        // 25 个字的地图名截到 20 个
        let long = "一二三四五六七八九十一二三四五六七八九十一二三四五";
        assert_eq!(sanitize_file_name(long), "一二三四五六七八九十一二三四五六七八九十");
    }

    /// 卡片数据必须是真 PNG。截断的 base64 不能变成一张打不开的 `.png`。
    #[test]
    fn share_card_payload_must_be_a_png() {
        // 1×1 透明 PNG 的标准 base64
        let real = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNkYPhfDwAChwGA60e6kgAAAABJRU5ErkJggg==";
        let bytes = decode_png(real).expect("真 PNG 应该放行");
        assert_eq!(&bytes[1..4], b"PNG");
        // 截断 / 乱码 / 空：一律报错，而不是写出一张坏图
        assert!(decode_png("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfF").is_err());
        assert!(decode_png("%%%not-base64%%%").is_err());
        assert!(decode_png("").is_err());
    }

    #[test]
    fn calibration_png_is_valid_and_respects_the_width_limit() {
        use base64::Engine as _;

        let frame = Frame {
            width: 2,
            height: 2,
            bgra: vec![
                0, 0, 255, 255, 0, 255, 0, 255,
                255, 0, 0, 255, 255, 255, 255, 255,
            ],
        };
        let (encoded, image_w, image_h) =
            encode_calibration_png(&frame, u32::MAX).expect("小图应可编码");
        assert_eq!((image_w, image_h), (2, 2));

        let bytes = base64::engine::general_purpose::STANDARD
            .decode(encoded)
            .expect("PNG base64 应有效");
        assert_eq!(&bytes[..8], b"\x89PNG\r\n\x1a\n");
        let mut reader = png::Decoder::new(bytes.as_slice())
            .read_info()
            .expect("PNG 头应可解析");
        let mut decoded = vec![0; reader.output_buffer_size()];
        let info = reader.next_frame(&mut decoded).expect("PNG 数据应完整");
        assert_eq!((info.width, info.height), (2, 2));

        let (small, image_w, image_h) =
            encode_calibration_png(&frame, 1).expect("缩小后仍应可编码");
        assert_eq!((image_w, image_h), (1, 1));
        let small = base64::engine::general_purpose::STANDARD
            .decode(small)
            .expect("缩小图 base64 应有效");
        let mut reader = png::Decoder::new(small.as_slice())
            .read_info()
            .expect("缩小 PNG 应可解析");
        let mut decoded = vec![0; reader.output_buffer_size()];
        reader.next_frame(&mut decoded).expect("缩小 PNG 数据应完整");
        assert_eq!(&decoded[..4], &[255, 0, 0, 255]);
    }

    /// 校准档的坐标是**比例**：同一份框换算到不同客户区都能用；
    /// 越界 / 过小的框要被命令层挡住（这里验证底层判据）。
    #[test]
    fn calibration_rects_are_ratio_based_and_guarded() {
        use crate::exp::region::{NormRect, MIN_PIXEL_SIDE};

        let rect = NormRect::new(0.1, 0.9, 0.2, 0.02).expect("合法比例");
        let at_1080 = rect.to_pixel(1920, 1080).expect("1080p 应能映射");
        let at_1440 = rect.to_pixel(2560, 1440).expect("1440p 应能映射");
        assert_eq!((at_1080.x, at_1080.y), (192, 972));
        assert_eq!((at_1440.x, at_1440.y), (256, 1296));
        assert!(at_1440.w > at_1080.w, "同一比例在更大客户区应映射成更大的框");

        assert!(NormRect::new(0.0, 0.0, 0.0, 0.5).is_none(), "零宽框不合法");
        assert!(NormRect::new(0.95, 0.0, 0.2, 0.5).is_none(), "出界框不合法");
        let tiny = NormRect::new(0.0, 0.0, 0.001, 0.001)
            .expect("比例上合法")
            .to_pixel(1920, 1080)
            .expect("应能映射");
        assert!(!tiny.is_usable(), "物理上小于 {MIN_PIXEL_SIDE}px 的框应被拒绝");
    }

    /// 卡片要落在用户找得到的地方（图片文件夹），不是程序的数据目录
    #[cfg(windows)]
    #[test]
    fn share_card_dir_is_under_pictures() {
        let dir = share_card_dir().expect("用户目录应该存在");
        assert!(dir.ends_with(std::path::Path::new("Pictures").join("枫之助")));
    }
}
