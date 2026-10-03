//! 约定检查：**不要在 UI 线程的回调里直接做窗口操作**。
//!
//! 这个项目已经踩过三次同一类坑，每一次现象都不一样、但根因完全相同
//! （在 Windows 的 UI 线程回调栈里再去做窗口操作 → 重入死锁）：
//!
//! 1. 同步 `#[tauri::command]` 里调 `WebviewWindowBuilder::build()`
//!    → 登录窗口白屏，而且点 X / 按 Esc 都关不掉（整个 UI 线程卡住）；
//! 2. 注册窗口事件 / 全局热键的回调里直接 `hud.show() / hide()`；
//! 3. 单实例插件的回调里 `show() / unminimize() / set_focus()`
//!    → 第一个实例卡死，之后每次双击都只是多一个卡在 `SendMessage` 里的
//!    僵尸进程，用户看到的就是「双击没反应」。
//!
//! 规矩很简单：**这些回调里只能“投递”，不能“操作”** ——
//! 窗口操作要么放进 `std::thread::spawn`，要么交给 `async` 命令在 tokio 上跑。
//!
//! 这个测试用最朴素的方式守住它：把回调闭包找出来，扫它的函数体里有没有
//! 直接调用窗口操作。它是文本级的启发式检查（不理解语义），但足以拦住上面
//! 三种写法——`scanner_catches_the_single_instance_deadlock` 就是这个自证。

use std::path::{Path, PathBuf};

/// 回调入口：这些闭包都在 UI 线程里被同步调用。
///
/// 注意这里**只匹配到左括号**，不写 `(|`：闭包常写成 `move |app, event| { ... }`
/// （尤其是全局快捷键的 `with_handler`），只要多一个 `move` 就匹配不上 ——
/// 那个监听热键的回调曾经就这样滑过了这道检查。
/// 定位到入口后第一个 `{` 就是闭包体，中间不会出现别的大括号。
const CALLBACK_ENTRIES: [&str; 6] = [
    "init(",
    "with_handler(",
    "on_window_event(",
    "on_menu_event(",
    "on_tray_icon_event(",
    "on_webview_event(",
];

/// 这些调用会往事件循环投递消息；在 UI 线程的回调栈里直接调就会卡住。
const DANGEROUS_OPS: [&str; 8] = [
    ".show()",
    ".hide()",
    ".set_focus()",
    ".unminimize()",
    ".set_position(",
    ".set_size(",
    ".maximize()",
    ".close()",
];

/// 「把窗口操作丢出去」的写法；出现它就认为该回调是安全的。
const DELEGATIONS: [&str; 3] = ["thread::spawn", "tauri::async_runtime", "spawn_blocking"];

/// 找出所有回调闭包的**函数体**（花括号配对）。
fn callback_bodies(source: &str) -> Vec<String> {
    let mut bodies = Vec::new();

    for entry in CALLBACK_ENTRIES {
        let mut from = 0;
        while let Some(offset) = source[from..].find(entry) {
            let at = from + offset;
            from = at + entry.len();

            let Some(open_offset) = source[from..].find('{') else {
                continue;
            };
            let open = from + open_offset;

            let mut depth = 0usize;
            let mut end = None;
            for (index, ch) in source[open..].char_indices() {
                match ch {
                    '{' => depth += 1,
                    '}' => {
                        depth -= 1;
                        if depth == 0 {
                            end = Some(open + index);
                            break;
                        }
                    }
                    _ => {}
                }
            }

            if let Some(end) = end {
                bodies.push(source[open..=end].to_string());
                from = end;
            }
        }
    }

    bodies
}

/// 扫一个函数体：里面直接做了窗口操作、又没有投递出去，就算违规。
fn scan_body(body: &str) -> Vec<String> {
    if DELEGATIONS.iter().any(|marker| body.contains(marker)) {
        return Vec::new();
    }

    let mut found = Vec::new();
    for op in DANGEROUS_OPS {
        if let Some(at) = body.find(op) {
            let line = body[..at].matches('\n').count() + 1;
            let snippet: String = body[at.saturating_sub(60)..]
                .chars()
                .take_while(|c| *c != '\n')
                .collect();
            found.push(format!("第 {line} 行附近直接调用了 {op} —— {snippet}"));
        }
    }
    found
}

/// 找出所有**同步** `#[tauri::command]` 的函数体（花括号配对）。
///
/// 为什么同步命令也在扫描范围内：在 Windows 上同步命令就运行在 UI 线程、
/// WebView2 的回调栈里，在那里 `show() / hide() / close()` 一样会重入事件循环。
/// 以前这道检查只扫回调闭包，于是 `toggle_hud` 这类命令体里的直接窗口操作
/// 一直是漏网的（`async` 命令跑在 tokio 上，不受这条约束）。
fn sync_command_bodies(source: &str) -> Vec<String> {
    let mut bodies = Vec::new();
    let mut from = 0;
    let marker = "#[tauri::command]";

    while let Some(offset) = source[from..].find(marker) {
        let at = from + offset;
        from = at + marker.len();

        // 函数签名：从 `fn ` 到函数体的第一个 `{`
        let Some(fn_offset) = source[from..].find("fn ") else {
            continue;
        };
        let fn_at = from + fn_offset;
        let Some(open_offset) = source[fn_at..].find('{') else {
            continue;
        };
        let open = fn_at + open_offset;
        // 签名要从属性后面开始截，而不是从 `fn ` 开始：
        // `async` 写在 `fn` **之前**（`pub async fn`），从 `fn` 截会把它漏掉，
        // 于是所有 async 命令都会被误报成同步命令。
        let signature = &source[at..open];

        // 签名里写了 `async` 才算 async 命令（它们跑在 tokio 上，不受这条约束）
        if signature.contains("async") {
            continue;
        }

        let mut depth = 0usize;
        for (index, ch) in source[open..].char_indices() {
            match ch {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        bodies.push(source[open..=open + index].to_string());
                        from = open + index;
                        break;
                    }
                }
                _ => {}
            }
        }
    }

    bodies
}

/// 返回源码里所有「在回调里直接操作窗口」的可疑片段。
fn violations_in(source: &str) -> Vec<String> {
    let mut found = Vec::new();

    for body in callback_bodies(source) {
        found.extend(scan_body(&body));
    }

    found
}

/// 返回源码里所有「在同步命令里直接操作窗口」的可疑片段。
fn command_violations_in(source: &str) -> Vec<String> {
    let mut found = Vec::new();
    for body in sync_command_bodies(source) {
        found.extend(scan_body(&body));
    }
    found
}

fn rust_sources() -> Vec<PathBuf> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
        .unwrap_or_else(|err| panic!("读不到 {}: {err}", dir.display()))
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| path.extension().is_some_and(|ext| ext == "rs"))
        .collect();
    files.sort();
    files
}

#[test]
fn window_ops_are_not_called_directly_inside_ui_callbacks() {
    let mut problems = Vec::new();

    for path in rust_sources() {
        let source = std::fs::read_to_string(&path)
            .unwrap_or_else(|err| panic!("读不到 {}: {err}", path.display()));
        for violation in violations_in(&source) {
            problems.push(format!("{}：{violation}", path.display()));
        }
    }

    assert!(
        problems.is_empty(),
        "发现「在 UI 线程回调里直接操作窗口」的写法，这会重入死锁：\n{}\n\
         请把窗口操作放进 std::thread::spawn（或在 async 命令里做），\
         详见 src-tauri/tests/ui_thread_callbacks.rs 的文件说明。",
        problems.join("\n")
    );
}

/// 同一道纪律也适用于**同步命令**。
///
/// 它们以前是漏网的：`toggle_hud` / `hide_hud` / `show_hud` 三个命令都直接在
/// 函数体里调 `show() / hide() / set_focus()`，而侧栏那个「呼出 / 收起查价窗」
/// 按钮走的正是这条路径。同样的重入风险，只是恰好还没演成大故障。
#[test]
fn window_ops_are_not_called_directly_inside_sync_commands() {
    let mut problems = Vec::new();

    for path in rust_sources() {
        let source = std::fs::read_to_string(&path)
            .unwrap_or_else(|err| panic!("读不到 {}: {err}", path.display()));
        for violation in command_violations_in(&source) {
            problems.push(format!("{}：{violation}", path.display()));
        }
    }

    assert!(
        problems.is_empty(),
        "发现「在同步命令里直接操作窗口」的写法（同步命令跑在 Windows UI 线程上）：\n{}\n\
         请改调 lib.rs 里自带投递的 toggle_hud_window / hide_window / close_window。",
        problems.join("\n")
    );
}

/// 自证：同步命令里的裸窗口操作会被抓出来。
#[test]
fn scanner_catches_window_ops_in_sync_commands() {
    let broken = r#"
        #[tauri::command]
        pub fn toggle_hud(app: AppHandle) -> Result<bool, String> {
            if let Some(hud) = app.get_webview_window("hud") {
                hud.show().map_err(|e| e.to_string())?;
                hud.set_focus().map_err(|e| e.to_string())?;
            }
            Ok(true)
        }
    "#;
    assert_eq!(
        command_violations_in(broken).len(),
        2,
        "同步命令里的 show / set_focus 应当被报出来"
    );

    // 投递出去就安全
    let fixed = r#"
        #[tauri::command]
        pub fn toggle_hud(app: AppHandle) {
            let app = app.clone();
            std::thread::spawn(move || crate::toggle_hud_window(&app));
        }
    "#;
    assert!(
        command_violations_in(fixed).is_empty(),
        "投递写法不该被报出来"
    );

    // async 命令跑在 tokio 上，不受这条约束
    let async_ok = r#"
        #[tauri::command]
        pub async fn open_login_window(app: AppHandle) -> Result<(), String> {
            let window = builder.build().map_err(|e| e.to_string())?;
            window.set_focus().map_err(|e| e.to_string())?;
            Ok(())
        }
    "#;
    assert!(
        command_violations_in(async_ok).is_empty(),
        "async 命令不该被报出来"
    );
}

/// 自证：`move` 也不能让它失效。
///
/// 全局快捷键那个回调写成 `.with_handler(move |app, _, event| { ... })`，
/// 老版本的检查写死了 `with_handler(|`，于是整个回调体都没被扫到，
/// 里面直接调 `hud.show() / hide()` 也没人拦。
#[test]
fn scanner_sees_through_move_closures() {
    let broken = r#"
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(move |app, _shortcut, event| {
                    if let Some(hud) = app.get_webview_window("hud") {
                        let _ = hud.show();
                        let _ = hud.set_focus();
                    }
                })
                .build(),
        );
    "#;
    assert_eq!(
        violations_in(broken).len(),
        2,
        "move 闭包里的 show / set_focus 也应当被报出来"
    );

    // 正确的写法：回调里只投递
    let fixed = r#"
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(move |app, _shortcut, event| {
                    let app = app.clone();
                    std::thread::spawn(move || {
                        if let Some(hud) = app.get_webview_window("hud") {
                            let _ = hud.show();
                        }
                    });
                })
                .build(),
        );
    "#;
    assert!(violations_in(fixed).is_empty(), "投递写法不该被报出来");
}

/// 自证：这个检查确实能抓住单实例那次死锁的写法。
#[test]
fn scanner_catches_the_single_instance_deadlock() {
    // 出事前的写法：回调里直接操作窗口。
    let broken = r#"
        .plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.show();
                let _ = window.set_focus();
            }
        }))
    "#;
    assert_eq!(
        violations_in(broken).len(),
        2,
        "应报出 show / set_focus 两处"
    );

    // 修好之后的写法：只投递，不在回调栈里操作。
    let fixed = r#"
        .plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            let app = app.clone();
            std::thread::spawn(move || {
                if let Some(window) = app.get_webview_window("main") {
                    let _ = window.show();
                    let _ = window.set_focus();
                }
            });
        }))
    "#;
    assert!(violations_in(fixed).is_empty(), "不该报出任何问题");
}
