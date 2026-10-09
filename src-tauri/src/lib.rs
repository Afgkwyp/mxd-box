mod alert;
mod blacklist;
mod channel;
mod clip_image;
mod commands;
mod db;
mod exp;
mod hotkeys;
mod memory;
mod meso_alert;
mod mxdc_client;
mod mxdc_monitor;
mod shell;
mod update;

use commands::*;
use db::Database;
use exp::capture;
use exp::font::Font;
use exp::tracker::ExpTracker;
use exp::ExpReader;
use memory::MemoryMonitor;
use mxdc_client::MxdcClient;
use mxdc_monitor::{MonitorConfig, MonitorManager};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Instant;
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_global_shortcut::{Shortcut, ShortcutState};

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        // 单实例保护，必须注册在最前面。
        //
        // 之前踩过的坑：磁盘上同时存在两个构建（旧的 debug 版 + 新的 release 版），
        // 两个进程同时跑会共用同一个 SQLite 缓存、抢同一个 Alt+F 全局热键。
        // 于是 Alt+F 呼出的是**先启动那个进程**的 HUD，而旧构建的拍卖查询是坏的，
        // 它还会把「0 条结果」写进共享缓存，反过来让另一个进程的窗口也查不到东西。
        // 现在再启动一个实例时，它会请求重启并退出，只会把已有窗口抬到前面。
        //
        // ⚠️ 回调里千万不能直接调窗口操作。这个回调是在**主线程的窗口过程里**
        // 被同步调用的（插件在 Windows 上用 WM_COPYDATA 投递），直接 show()/
        // set_focus() 就等于在主线程的消息处理栈里再做窗口操作 —— 和本项目早先
        // 那个「登录窗口白屏且关不掉」是同一类重入死锁：一旦卡住，第一个实例就不
        // 再泵消息，之后每次双击都只是多一个卡在 SendMessage 里的僵尸进程，
        // 用户看到的就是「双击后没反应」。
        //
        // 所以这里只克隆句柄，把窗口操作丢到其它线程：那些调用会变成往事件循环
        // 投递消息，等本次窗口过程返回后才执行。
        .plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            let app = app.clone();
            std::thread::spawn(move || {
                if let Some(window) = app.get_webview_window("main") {
                    let _ = window.show();
                    let _ = window.unminimize();
                    let _ = window.set_focus();
                }
            });
        }))
        // 日志级别压到 Info：scraper/selectors 在 DEBUG 下会把每个元素的
        // 选择器匹配过程都写进日志（一次页面解析就是十几 KB），结果真正
        // 有价值的查询日志会被冲掉，出问题后没法回溯。
        .plugin(
            tauri_plugin_log::Builder::default()
                .level(log::LevelFilter::Info)
                .build(),
        )
        .plugin(tauri_plugin_notification::init())
        // 主窗口「关闭」的语义。
        //
        // 以前点右上角的 X 之后进程并不会退出：HUD 是一个**常驻的隐藏窗口**，
        // 只要还有窗口活着，Tauri 就不会因为「主窗口关了」而结束进程，
        // 于是任务管理器里一直挂着一个没有界面的进程 —— 用户看到的就是
        // 「点了退出，进程还在」。现在默认真正退出；只有设置里勾了
        // 「关闭主窗口时退到托盘常驻」才改成隐藏到托盘（需要后台跑开服提醒
        // 的用户才开它），托盘菜单里也有明确的「退出程序」。
        .on_window_event(|window, event| {
            if window.label() != "main" {
                return;
            }
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                let close_to_tray = window
                    .app_handle()
                    .try_state::<AppState>()
                    .map(|state| state.close_to_tray.load(Ordering::Relaxed))
                    .unwrap_or(false);

                if close_to_tray {
                    // prevent_close 必须同步调用；窗口操作则丢给别的线程。
                    api.prevent_close();
                    hide_window(window.app_handle(), "main");
                    log::info!("主窗口关闭 → 退到托盘常驻（可在设置里改为直接退出）");
                } else {
                    log::info!("主窗口关闭 → 退出进程");
                    exit_app(window.app_handle());
                }
            }
        })
        .setup(|app| {
            // 0. 先把“我在跑哪个二进制”写进日志。
            //    之前就被两个不同构建的实例绕了很久：Alt+F 弹出的是旧进程的 HUD，
            //    而旧进程的服务器清单和查询参数都是旧的，看起来像新代码没生效。
            let build = build_info();
            log::info!(
                "启动 v{}，二进制：{}（文件时间 {}）",
                build.version,
                build.exe_path,
                build.built_at
            );

            // 1. App Data Dir & SQLite init
            let app_data_dir = app.path().app_data_dir().map_err(|err| {
                let message = format!(
                    "无法确定应用数据目录，已停止启动以避免把个人数据写入当前目录：{err}"
                );
                log::error!("{message}");
                std::io::Error::other(message)
            })?;
            std::fs::create_dir_all(&app_data_dir).map_err(|err| {
                let message = format!(
                    "无法创建应用数据目录 {}，已停止启动：{err}",
                    app_data_dir.display()
                );
                log::error!("{message}");
                std::io::Error::other(message)
            })?;
            let db_path = app_data_dir.join("mxd_box.db");
            let database = Arc::new(Database::new(db_path.clone()).map_err(|err| {
                let message = format!("无法打开应用数据库 {}，已停止启动：{err}", db_path.display());
                log::error!("{message}");
                std::io::Error::other(message)
            })?);

            let settings = database.get_all_settings();

            // 1.5 屏幕抓取要看的是**物理像素**。
            //     缩放不是 100% 的显示器上，少了这一次声明，算出来的坐标会被系统
            //     虚拟化，抓到的画面整体偏移 —— 而且照样「成功」，只是内容对不上。
            capture::ensure_dpi_aware();

            // 2. Memory Monitor（悬浮窗与顶栏的内存监控都来自这个循环）
            let memory_monitor = Arc::new(MemoryMonitor::new(
                settings.memory_threshold,
                settings.memory_alert_enabled,
            ));
            memory_monitor.start_loop(app.handle().clone());

            // 2.5 经验统计。
            //
            //     字形表是**内置的完整 14 个符号**（`0-9 ( ) . %`），不做任何运行时学习 ——
            //     经验那一行的字符集本来就是封闭的，学它只会带来「时好时坏」。
            //     抓屏用**后台截屏**（PrintWindow），游戏被别的窗口压住也照读不误；
            //     旧实现从屏幕 DC 抓，被压住时抓到的是别人的像素，这就是
            //     「经常识别不到」的头号原因。整个链路只截屏读像素，不碰游戏进程。
            //
            //     找位置分三层（见 `exp::reader` / `exp::region`）：校准框（比例坐标，
            //     换分辨率跟着走）→ 整幅画面多尺度自动定位 → PP-OCR 兜底。
            //     学到的框要连续两帧稳定才落库；手动框永不被自动覆盖。
            let store: Arc<dyn exp::region::RegionStore> =
                Arc::clone(&database) as Arc<dyn exp::region::RegionStore>;
            let reader = ExpReader::with_store(Font::builtin(), Some(store));
            let exp_tracker = Arc::new(ExpTracker::new(reader));
            // 练级目标（设过的话）：重启还在
            exp_tracker.set_goal(
                database
                    .get_setting(EXP_GOAL_KEY)
                    .and_then(|raw| raw.trim().parse::<u32>().ok()),
            );
            exp_tracker.start_loop(app.handle().clone(), Arc::clone(&database));

            // 2.6 当前频道（几线）。
            //
            //     游戏只在「切换频道」窗口里显示当前线，磁盘上不落任何记录；
            //     但它是**一条线一个服务端端口**，查系统 TCP 表就能反推
            //     （实测：47 线 = 8586，见 `channel.rs` 模块头）。
            //     同样不碰游戏进程 —— 只是读连接表上的端口号。
            let channel_watcher = Arc::new(channel::ChannelWatcher::new(&database));
            channel_watcher.start_loop(app.handle().clone(), Arc::clone(&database));

            // 3. MXDC Client（拍卖 / 金价 / 图鉴 / 开服监控共用同一个连接池）
            let mxdc_client = Arc::new(MxdcClient::new());

            // 4.5 小册子「开服监控」轮询
            //
            // 站点自己就在每分钟探一次登录入口，我们接它的结论来做开服提醒 ——
            // 比自己盯一个可能轮换掉的网关端口更接近「官方开服了没有」。
            // 首次采样立刻做，别让用户对着「还没拿到数据」等一整轮。
            let monitor_manager = Arc::new(MonitorManager::new(
                Arc::clone(&mxdc_client),
                MonitorConfig {
                    enabled: settings.monitor_enabled,
                    interval_sec: settings.monitor_interval_sec,
                },
            ));

            // 5. 悬浮窗快捷键（默认 F10，切换显示 / 隐藏）
            //
            // 回调里**只投递、不操作**：热键回调是在主线程的事件循环里同步调用的，
            // 在那里直接 show()/hide() 就是往自己的消息栈里再做窗口操作 ——
            // 本项目已经在同一个坑里摔过三次（见 tests/ui_thread_callbacks.rs）。
            let _ = app.handle().plugin(
                tauri_plugin_global_shortcut::Builder::new()
                    .with_handler(move |app, shortcut, event| {
                        // 只响应「按下」，否则按住不放会连续切换、窗口开始闪。
                        if event.state() != ShortcutState::Pressed {
                            return;
                        }
                        // 回调里**只投递**：下面这个判断要读数据库（会拿锁），
                        // 而回调是在主线程的事件循环里跑的。
                        let app = app.clone();
                        let pressed: Shortcut = shortcut.clone();
                        std::thread::spawn(move || dispatch_hotkey(&app, &pressed));
                    })
                    .build(),
            );

            hotkeys::apply_all(
                app.handle(),
                &settings.hotkey,
                &settings.lite_float_hotkey,
                &settings.exp_hotkey,
            );

            // 6. Manage AppState
            app.manage(AppState {
                db: database,
                memory_monitor,
                monitor_manager,
                mxdc_client,
                exp_tracker,
                channel_watcher,
                close_to_tray: Arc::new(AtomicBool::new(settings.close_to_tray)),
            });

            // 监控首轮可能马上触发提醒，先注册 AppState 才能同时写入提醒历史。
            app.state::<AppState>()
                .monitor_manager
                .start_loop(app.handle().clone());

            // 7. 系统托盘
            setup_tray(app.handle())?;

            // 8. 剪贴板黑名单嗅探。
            //
            // 放在 Rust 侧、和界面是否打开无关：要查一个人的时刻恰好是
            // **正在游戏里交易**的时候 —— 那时你不在黑名单页，主窗口多半
            // 也收在托盘里（原来那个前端定时器在这种情况下根本不运行）。
            blacklist::spawn_clipboard_watcher(app.handle().clone());

            // 9. 金价到价提醒：开着才每 10 分钟看一眼报价，没开什么请求都不发。
            meso_alert::spawn(app.handle().clone());

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_settings,
            get_server_list,
            get_build_info,
            take_version_greeting,
            get_update_page_url,
            patch_settings,
            set_memory_threshold,
            set_memory_alert,
            toggle_hud,
            hide_hud,
            show_hud,
            query_market,
            get_item_detail,
            search_drops,
            get_meso_report,
            list_alert_history,
            clear_alert_history,
            get_blacklist,
            add_blacklist,
            remove_blacklist,
            check_blacklist,
            set_blacklist_watch,
            import_blacklist,
            export_blacklist,
            test_open_alert,
            get_server_monitor_status,
            refresh_server_monitor,
            get_latest_open_alert,
            dismiss_open_alert,
            focus_main_window,
            get_hotkey_status,
            apply_hotkey,
            get_float_window_visible,
            toggle_lite_float,
            hide_lite_float,
            set_default_server,
            quit_app,
            save_auth_cookie,
            open_login_window,
            open_qr_login_window,
            open_external_url,
            test_cookie_validity,
            close_login_window,
            begin_hotkey_capture,
            end_hotkey_capture,
            get_memory_status,
            check_for_update,
            download_update,
            get_exp_status,
            set_exp_goal,
            get_meso_alert,
            set_meso_alert,
            get_channel_state,
            set_channel_manual,
            start_exp_session,
            pause_exp_session,
            resume_exp_session,
            end_exp_session,
            resolve_exp_session,
            get_exp_preview,
            get_region_profile,
            save_region_profile,
            clear_region_profiles,
            test_exp_region,
            auto_calibrate_exp,
            calibration_overlay_geometry,
            open_calibration_overlay,
            close_calibration_overlay,
            calibration_frame,
            known_exp_maps,
            get_exp_history,
            exp_report,
            get_exp_curve,
            clear_exp_history,
            delete_exp_session,
            clear_exp_notice,
            list_exp_characters,
            rename_exp_character,
            delete_exp_character,
            save_share_card,
            copy_share_card,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

// ---------------------------------------------------------------------------
// 托盘 / 窗口操作
//
// 这里的每个函数都**把窗口操作丢给另一个线程**。
// 托盘事件、菜单事件、窗口事件都是在主线程的事件循环里同步回调的，
// 在那里直接 show()/hide()/set_focus() 就是在自己的消息处理栈里再做窗口
// 操作 —— 本项目的登录窗白屏、单实例卡死都是这么来的。
// 规矩：**回调里只能“投递”，不能“操作”**（见
// src-tauri/tests/ui_thread_callbacks.rs，有一道自动检查守着它）。
// ---------------------------------------------------------------------------

/// 把指定窗口显示出来、取消最小化、并置顶聚焦。
pub fn reveal_window(app: &AppHandle, label: &str) {
    let app = app.clone();
    let label = label.to_string();
    std::thread::spawn(move || {
        if let Some(window) = app.get_webview_window(&label) {
            let _ = window.show();
            let _ = window.unminimize();
            let _ = window.set_focus();
        }
    });
}

pub fn hide_window(app: &AppHandle, label: &str) {
    let app = app.clone();
    let label = label.to_string();
    std::thread::spawn(move || {
        // 两个悬浮窗收起前要先把画面清空（见 `hide_overlay`）
        if overlay_state(&label).is_some() {
            hide_overlay(&app, &label);
        } else if let Some(window) = app.get_webview_window(&label) {
            let _ = window.hide();
        }
    });
}

// ---------------------------------------------------------------------------
// 悬浮窗的收起 / 呼出
//
// 悬浮窗是 hide / show 出来的，页面一直活着。直接 `hide()` 的话，窗口里**最后画出来的
// 那一帧**（完整的面板）会留在系统那边；下次 `show()` 的一瞬间先贴出来的就是它，
// 随后页面才收到「我可见了」、从透明开始播入场动画 —— 用户看到的是
// 「面板闪一下 → 消失 → 再淡入」（用户报的「按下快捷键后有明显的闪烁，闪烁后才淡入」）。
//
// 所以收起分两步：先告诉页面「要收了」，它把面板清成透明并画出这一帧；
// 等一小会儿再真的 `hide()`。这样下次呼出时先贴出来的是一张空画面，动画从空开始，不闪。
// ---------------------------------------------------------------------------

/// 一个悬浮窗的收起状态。
struct OverlayState {
    /// 每次呼出 / 收起都加一。收起要等一小会儿才真的 `hide()`，这期间用户又按了一次
    /// 快捷键（要呼出），代次就变了，那次收起作废。
    epoch: std::sync::atomic::AtomicU64,
    /// 正在收起（已经通知页面清空，还没 `hide()`）。这时窗口在系统那边还算「可见」，
    /// 但对用户来说它已经收起了 —— 切换时要当成「没显示」。
    hiding: AtomicBool,
}

fn overlay_state(label: &str) -> Option<&'static OverlayState> {
    static HUD: OverlayState = OverlayState {
        epoch: std::sync::atomic::AtomicU64::new(0),
        hiding: AtomicBool::new(false),
    };
    static FLOAT: OverlayState = OverlayState {
        epoch: std::sync::atomic::AtomicU64::new(0),
        hiding: AtomicBool::new(false),
    };
    match label {
        "hud" => Some(&HUD),
        "float" => Some(&FLOAT),
        _ => None,
    }
}

/// 页面收到「要收了」到它把空画面画出来，留这么久（几帧的时间）。
const OVERLAY_BLANK_DELAY: std::time::Duration = std::time::Duration::from_millis(70);

/// 收起一个悬浮窗。**只能在已经丢出去的线程里调**（里面会睡一小会儿，还有窗口操作）。
fn hide_overlay(app: &AppHandle, label: &str) {
    let Some(state) = overlay_state(label) else {
        return;
    };
    let ticket = state.epoch.fetch_add(1, Ordering::SeqCst) + 1;
    state.hiding.store(true, Ordering::SeqCst);
    let _ = app.emit_to(label, "overlay-hiding", ());
    std::thread::sleep(OVERLAY_BLANK_DELAY);
    // 这期间又被呼出了：别把刚出来的窗口收掉
    if state.epoch.load(Ordering::SeqCst) != ticket {
        return;
    }
    if let Some(window) = app.get_webview_window(label) {
        let _ = window.hide();
    }
    state.hiding.store(false, Ordering::SeqCst);
}

/// 呼出一个悬浮窗。**只能在已经丢出去的线程里调**。
fn show_overlay(app: &AppHandle, label: &str) {
    if let Some(state) = overlay_state(label) {
        state.epoch.fetch_add(1, Ordering::SeqCst);
        state.hiding.store(false, Ordering::SeqCst);
    }
    if let Some(window) = app.get_webview_window(label) {
        let _ = window.show();
        let _ = window.set_focus();
    }
    // 页面靠「可见性变化 / 获得焦点」知道自己被呼出了，但这两个信号都有不来的时候；
    // 而它收起前把自己清成了透明 —— 再补一个明确的通知，免得呼出来是一个空窗口。
    let _ = app.emit_to(label, "overlay-shown", ());
}

/// 切换一个悬浮窗：显示着就收起，否则呼出。**只能在已经丢出去的线程里调**。
fn toggle_overlay(app: &AppHandle, label: &str) {
    let Some(window) = app.get_webview_window(label) else {
        return;
    };
    let hiding = overlay_state(label)
        .map(|state| state.hiding.load(Ordering::SeqCst))
        .unwrap_or(false);
    if window.is_visible().unwrap_or(false) && !hiding {
        hide_overlay(app, label);
    } else {
        show_overlay(app, label);
    }
}

/// 关掉一个窗口（登录窗用完就收）。
///
/// 和 `hide_window` 一样必须投递：`close()` 会往事件循环发消息，在同步命令里
/// 直接调就是那道熟悉的「在 UI 线程回调里操作窗口」死锁。
pub fn close_window(app: &AppHandle, label: &str) {
    let app = app.clone();
    let label = label.to_string();
    std::thread::spawn(move || {
        if let Some(window) = app.get_webview_window(&label) {
            if let Err(err) = window.close() {
                log::warn!("关闭窗口 {} 失败：{}", label, err);
            }
        }
    });
}

/// 按下的全局快捷键对应哪个悬浮窗，然后切换它。
///
/// 两个悬浮窗各注册一个键，回调里拿到的是「哪一个键」。靠按键 → 窗口的映射分派，
/// 而不是「按一下就两个都切」：后者在用户只看金价时会把查价窗也顶出来。
///
/// ⚠️ 这里**必须比较解析后的 `Shortcut`，不能比较字符串**。
/// `Shortcut` 的 `Display` 用的是键码名（`Code::KeyF` → `KeyF`），而设置里存的是
/// 用户看见的写法（`Alt+F`），两者永远不相等 —— 曾经就是这样：`F10` 因为名字恰好
/// 一样而能用，`Alt+F` 却怎么按都没反应（这恰好也是用户报的「没有悬浮窗」）。
fn dispatch_hotkey(app: &AppHandle, pressed: &Shortcut) {
    let Some(settings) = app
        .try_state::<AppState>()
        .map(|state| state.db.get_all_settings())
    else {
        log::warn!("快捷键按下但读不到设置，默认切换精简悬浮窗");
        toggle_float_window(app);
        return;
    };

    // 用和注册时同一个解析器，`Alt+F` / `Ctrl+Shift+K` 这些写法就能对上了。
    let is = |spec: &str| spec.parse::<Shortcut>().map(|it| it == *pressed).unwrap_or(false);

    if is(&settings.hotkey) {
        toggle_hud_window(app);
    } else if is(&settings.lite_float_hotkey) {
        toggle_float_window(app);
    } else if is(&settings.exp_hotkey) {
        // 经验统计的开始 / 暂停 / 恢复。这里已经在一个 `std::thread::spawn` 出来的
        // 线程里（见上面的热键回调），而 toggle 里会去截屏读像素 —— 绝不能放回
        // 主线程的事件循环里做。
        if let Some(state) = app.try_state::<AppState>() {
            match state.exp_tracker.toggle() {
                Ok(()) => log::info!("热键 → 经验统计已切换"),
                Err(err) => log::warn!("热键切换经验统计失败：{}", err),
            }
        }
    } else {
        // 两个都不匹配：多半是刚改完键、回调里的旧对象还在。两个都别动更安全，
        // 但把实情写进日志 —— 「设了快捷键没反应」这类问题只能靠它定位。
        log::warn!(
            "按下的快捷键 {}（键码写法 {}）与配置不符，已忽略；当前配置：查价 {} / 精简 {}",
            pressed,
            format!("{:?}", pressed),
            settings.hotkey,
            settings.lite_float_hotkey
        );
    }
}

/// 呼出 / 收起**经验窗**（原「精简悬浮窗」，现在只放经验 + 内存）。
///
/// 公开给 `commands.rs` 与托盘菜单共用：**切换的方式** —— 按一次出来、再按一次收起。
pub fn toggle_float_window(app: &AppHandle) {
    let app = app.clone();
    std::thread::spawn(move || toggle_overlay(&app, "float"));
}

/// 呼出 / 收起**查价悬浮窗**（和它的全局快捷键一个效果）。
pub fn toggle_hud_window(app: &AppHandle) {
    let app = app.clone();
    std::thread::spawn(move || toggle_overlay(&app, "hud"));
}

/// 上一次「右键托盘图标」的时刻（打开托盘菜单的那一下）。
///
/// 存在这里而不是挂在托盘对象上，是因为菜单回调拿不到托盘对象；
/// 它只有一处读者（`tray-quit`），而且只关心「刚刚有没有过」。
fn tray_context_click_slot() -> &'static parking_lot::Mutex<Option<Instant>> {
    static SLOT: std::sync::OnceLock<parking_lot::Mutex<Option<Instant>>> = std::sync::OnceLock::new();
    SLOT.get_or_init(|| parking_lot::Mutex::new(None))
}

/// 真正结束整个进程（托盘菜单 / 界面里的「退出程序」都走它）。
fn exit_app(app: &AppHandle) {
    let app = app.clone();
    std::thread::spawn(move || app.exit(0));
}

/// 系统托盘：关掉主窗口之后还有地方能把窗口找回来、呼出 HUD、真正退出。
fn setup_tray(app: &AppHandle) -> tauri::Result<()> {
    let show_main = MenuItem::with_id(app, "tray-show-main", "显示主窗口", true, None::<&str>)?;
    let toggle_hud = MenuItem::with_id(
        app,
        "tray-toggle-hud",
        "呼出 / 收起查价悬浮窗",
        true,
        None::<&str>,
    )?;
    let toggle_float = MenuItem::with_id(
        app,
        "tray-toggle-float",
        "呼出 / 收起经验窗（经验统计）",
        true,
        None::<&str>,
    )?;
    let hide_main = MenuItem::with_id(
        app,
        "tray-hide-main",
        "隐藏主窗口（后台常驻）",
        true,
        None::<&str>,
    )?;
    let quit = MenuItem::with_id(app, "tray-quit", "退出程序", true, None::<&str>)?;
    let separator = PredefinedMenuItem::separator(app)?;

    let menu = Menu::with_items(
        app,
        &[
            &show_main,
            &toggle_hud,
            &toggle_float,
            &hide_main,
            &separator,
            &quit,
        ],
    )?;

    let mut builder = TrayIconBuilder::with_id("mxd-tray")
        // 文案要和行为一致：下面单击和双击都会把主窗口叫回来
        // （单击就响应比要求双击更顺手，所以改文案而不是改行为）
        .tooltip("枫之助（单击显示主窗口 · 右键菜单）")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id().as_ref() {
            "tray-show-main" => reveal_window(app, "main"),
            "tray-toggle-hud" => toggle_hud_window(app),
            "tray-toggle-float" => toggle_float_window(app),
            "tray-hide-main" => hide_window(app, "main"),
            "tray-quit" => {
                // ⚠️ 不能「听到就退」：Windows 在 shell 被强杀时会补发一个陈旧的
                // 菜单事件，实测重启资源管理器就会让程序自己退出（见 shell.rs）。
                let last_click = *tray_context_click_slot().lock();
                if shell::should_honor_quit(shell::tray_window_exists(), last_click, Instant::now()) {
                    log::info!("托盘菜单 → 退出程序");
                    exit_app(app);
                } else {
                    log::warn!(
                        "忽略了一次来源不明的「退出程序」事件（任务栏不在、也没右键过托盘）—— 这是 shell 重建时补发的陈旧事件"
                    );
                }
            }
            _ => {}
        })
        .on_tray_icon_event(|tray, event| match event {
            // 左键单击 / 双击都用来把主窗口找回来（Windows 上双击有独立事件）
            TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            }
            | TrayIconEvent::DoubleClick { .. } => reveal_window(tray.app_handle(), "main"),
            // 右键（就是打开托盘菜单那一下）记一笔时间，给上面那道防误触当依据
            TrayIconEvent::Click {
                button: MouseButton::Right,
                ..
            } => {
                *tray_context_click_slot().lock() = Some(Instant::now());
            }
            _ => {}
        });

    // 用打包进二进制的应用图标，避免再引入图片解码依赖
    if let Some(icon) = app.default_window_icon().cloned() {
        builder = builder.icon(icon);
    } else {
        log::warn!("没有默认窗口图标，托盘图标可能不可见");
    }

    builder.build(app)?;
    log::info!("系统托盘已就绪");
    Ok(())
}
