//! 全局快捷键：两个悬浮窗各一个（都是「按一次出来、再按一次收起」）。
//!
//! * `PriceHud` → **查价悬浮窗**（`hud` 窗口，拍卖行查价，默认 `Alt+F`）
//! * `LiteFloat` → **经验窗**（`float` 窗口，经验 / 小时 + 内存，默认 `F10`；旧称「精简悬浮窗」）
//!
//! 两者是**不同的窗口**，所以各配一个快捷键，互不影响。
//!
//! ## 为什么单独一个模块
//!
//! 以前这段逻辑散在 `setup()` 里，而且只做了一件事：
//!
//! ```ignore
//! if let Ok(shortcut) = hotkey_str.parse::<Shortcut>() {
//!     let _ = app.global_shortcut().register(shortcut);   // ⚠️ 结果被丢掉
//! }
//! ```
//!
//! 两个真问题：
//!
//! 1. **注册失败是静默的**。这个键被别的程序占着、或者用户填了一个平台不认识的
//!    组合，`register` 会返回 `Err`，而界面什么都不说 —— 用户看到的就是
//!    「设了快捷键，按下去毫无反应」，非常像「这个功能没做」或者「程序坏了」。
//!    现在每个键的注册结果都存在这里，界面直接读出来显示。
//! 2. **改完要重启才生效**。`save_settings` 从来没重新注册过，所以用户在设置里
//!    改成别的键、当场试一下没反应，只能重启程序才生效 —— 这几乎必然会让人以为
//!    设置没保存成功。现在保存即重注册。

use parking_lot::RwLock;
use std::sync::OnceLock;
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut};

/// 查价悬浮窗的默认键。历史默认值，保持不变（老用户已习惯）。
pub const DEFAULT_PRICE_HUD_HOTKEY: &str = "Alt+F";
/// 精简悬浮窗（金价 + 内存 + 经验）的默认键：单键、不需要组合、游戏里也好按。
pub const DEFAULT_LITE_FLOAT_HOTKEY: &str = "F10";

/// 开始 / 暂停 / 恢复**经验统计**的默认键。
///
/// 特意用组合键而**不是**像 F10 那样的单键：全局快捷键是 OS 级的 `RegisterHotKey`，
/// 一旦注册就从游戏手里把这个键抢走了，游戏那边没有任何提示。
/// 用组合键不容易碰上游戏自己的技能键。
pub const DEFAULT_EXP_TOGGLE_HOTKEY: &str = "Ctrl+Alt+P";

/// 三个全局快捷键各对应一个动作。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HotkeyAction {
    PriceHud,
    LiteFloat,
    /// 开始 / 暂停 / 恢复经验统计（不涉及窗口，只是一个开关）
    ExpToggle,
}

impl HotkeyAction {
    pub fn code(self) -> &'static str {
        match self {
            HotkeyAction::PriceHud => "price_hud",
            HotkeyAction::LiteFloat => "lite_float",
            HotkeyAction::ExpToggle => "exp_toggle",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            HotkeyAction::PriceHud => "查价悬浮窗",
            HotkeyAction::LiteFloat => "经验窗",
            HotkeyAction::ExpToggle => "经验统计",
        }
    }

    pub fn default_hotkey(self) -> &'static str {
        match self {
            HotkeyAction::PriceHud => DEFAULT_PRICE_HUD_HOTKEY,
            HotkeyAction::LiteFloat => DEFAULT_LITE_FLOAT_HOTKEY,
            HotkeyAction::ExpToggle => DEFAULT_EXP_TOGGLE_HOTKEY,
        }
    }

    /// 窗口标识（和 `tauri.conf.json` / `?window=` 里的名字一致）。
    /// 经验统计不控制窗口，所以是空的。
    pub fn window_label(self) -> &'static str {
        match self {
            HotkeyAction::PriceHud => "hud",
            HotkeyAction::LiteFloat => "float",
            HotkeyAction::ExpToggle => "",
        }
    }

    pub fn all() -> [HotkeyAction; 3] {
        [
            HotkeyAction::PriceHud,
            HotkeyAction::LiteFloat,
            HotkeyAction::ExpToggle,
        ]
    }
}

/// 一个动作的注册状态。
#[derive(Debug, Clone, serde::Serialize)]
pub struct HotkeyStatus {
    /// "price_hud" / "lite_float"
    pub action: String,
    /// 中文名，界面直接用
    pub label: String,
    /// 实际在用的写法，例如 `F10` / `Ctrl+Shift+K`
    pub hotkey: String,
    /// 这个键是否真的被系统接受了
    pub registered: bool,
    /// 注册失败时的一句话（格式不认识 / 被别的程序占用 / 和另一个快捷键撞了）
    pub error: Option<String>,
    /// 默认值，界面上的「恢复默认」按钮用
    pub default_hotkey: String,
}

fn status_slot() -> &'static RwLock<Vec<HotkeyStatus>> {
    static STATUS: OnceLock<RwLock<Vec<HotkeyStatus>>> = OnceLock::new();
    STATUS.get_or_init(|| RwLock::new(Vec::new()))
}

pub fn statuses() -> Vec<HotkeyStatus> {
    status_slot().read().clone()
}

/// 「配置里想要的」三个快捷键，即最近一次 `apply_all` 收到的值。
///
/// 改键模式期间注册被推迟，这就是「等会儿要挂回去的那份配置」。
fn desired_slot() -> &'static RwLock<(String, String, String)> {
    static DESIRED: OnceLock<RwLock<(String, String, String)>> = OnceLock::new();
    DESIRED.get_or_init(|| {
        RwLock::new((
            DEFAULT_PRICE_HUD_HOTKEY.to_string(),
            DEFAULT_LITE_FLOAT_HOTKEY.to_string(),
            DEFAULT_EXP_TOGGLE_HOTKEY.to_string(),
        ))
    })
}

/// 是否正处在「改键捕获」模式（全局快捷键被临时注销）。
fn paused_slot() -> &'static RwLock<bool> {
    static PAUSED: OnceLock<RwLock<bool>> = OnceLock::new();
    PAUSED.get_or_init(|| RwLock::new(false))
}

pub fn is_paused() -> bool {
    *paused_slot().read()
}

/// 进入改键捕获模式：**把全局快捷键全部注销**。
///
/// 这一条不是锦上添花，是必须的：全局快捷键是 OS 级的 `RegisterHotKey`，
/// 键一旦注册，Windows 会把那次按键直接投递给我们的消息队列，**根本不送给
/// webview** —— 于是在设置页里点「按下快捷键」后按当前的 `F10`，前端收不到
/// keydown（`preventDefault` 拦不住 OS 级分发），而悬浮窗反被呼了出来。
/// 用户看到的现象是「我想把 F10 改成别的键，按 F10 没反应 / 窗口乱跳」。
///
/// 所以：捕获开始时全部注销，捕获结束时（捕获成功 / Esc / 点到别处 / 离开设置页）
/// 再照最新配置挂回去。
pub fn pause(app: &tauri::AppHandle) {
    let mut paused = paused_slot().write();
    if *paused {
        return;
    }
    *paused = true;
    drop(paused);

    if let Err(err) = app.global_shortcut().unregister_all() {
        log::warn!("改键模式下注销全局快捷键失败：{}", err);
    }
    log::info!("进入改键模式：全局快捷键已临时注销（捕获结束会自动挂回）");
}

/// 退出改键捕获模式：照「最近一次收到的配置」把快捷键挂回去。
///
/// 为什么用记下来的期望值、而不是让界面把值带过来或者回数据库读：
/// 「捕获结束」和「apply_hotkey」是两个并行的 invoke，到达顺序不确定。
/// 期望值由 `apply_all` 写入、只增不减，所以两种交错顺序都会收敛到
/// **最新那份配置**；反过来回库读有可能把改键前的旧值当成新值注册上去，
/// 用户看到的就是「改完键又变回原来那个」。
pub fn resume(app: &tauri::AppHandle) -> Vec<HotkeyStatus> {
    *paused_slot().write() = false;
    let (price_hud, lite_float, exp_toggle) = desired_slot().read().clone();
    let statuses = apply_all(app, &price_hud, &lite_float, &exp_toggle);
    let failed = statuses.iter().filter(|s| !s.registered).count();
    if failed == 0 {
        log::info!("退出改键模式：三个快捷键都已挂回");
    } else {
        log::warn!("退出改键模式：{} 个快捷键没能挂回（详见各条状态）", failed);
    }
    statuses
}

/// 把一个界面上的写法规范成 `Shortcut` 能解析的形式。
///
/// 界面上的捕获结果是「按了哪些修饰键 + 主键」，例如 `Ctrl+Shift+K`、`F10`。
/// 这里只做三件小事：去掉空格、把别名统一（`Control`→`Ctrl`、`Cmd`/`Meta`→`Super`）、
/// 空串回落到该动作的默认值。
pub fn normalize(action: HotkeyAction, spec: &str) -> String {
    let trimmed = spec.trim();
    if trimmed.is_empty() {
        return action.default_hotkey().to_string();
    }

    let mut parts: Vec<String> = Vec::new();
    for raw in trimmed.split('+') {
        let token = raw.trim();
        if token.is_empty() {
            continue;
        }
        let canonical = match token.to_ascii_lowercase().as_str() {
            "control" | "ctrl" => "Ctrl".to_string(),
            "alt" | "option" => "Alt".to_string(),
            "shift" => "Shift".to_string(),
            "cmd" | "command" | "meta" | "super" | "win" => "Super".to_string(),
            // 主键保持捕获给的大小写（F10 / K / 1）
            _ => token.to_string(),
        };
        parts.push(canonical);
    }

    if parts.is_empty() {
        return action.default_hotkey().to_string();
    }
    parts.join("+")
}

/// 重新注册**三个**快捷键（先全部注销，再逐个注册），并把各自结果记下来。
///
/// 一次性全量重注册而不是增量改：两个键有可能撞在一起（用户把两边都设成 F10），
/// 增量改很容易留下「新的没注册上、旧的已经注销掉」的空档，全量重注册则每轮都从
/// 干净状态开始，撞键也能当场报出来。
pub fn apply_all(
    app: &tauri::AppHandle,
    price_hud: &str,
    lite_float: &str,
    exp_toggle: &str,
) -> Vec<HotkeyStatus> {
    // 先记下「想要的配置」：改键模式期间注册会被系统直接吞掉，
    // 这次不注册，但 `resume` 要照最新这份挂回去。
    *desired_slot().write() = (
        price_hud.to_string(),
        lite_float.to_string(),
        exp_toggle.to_string(),
    );

    if is_paused() {
        log::info!("改键模式中：暂不注册（等捕获结束后自动挂回）");
        // 返回**空数组** = 「这次没有新的注册结论」，界面收到空数组保持原样显示；
        // 返回旧状态会让界面分不清这是新结果还是老结果。
        return Vec::new();
    }

    let manager = app.global_shortcut();

    if let Err(err) = manager.unregister_all() {
        log::warn!("注销旧快捷键失败（继续尝试注册新的）：{}", err);
    }

    let actions = HotkeyAction::all();
    let specs: Vec<String> = actions
        .iter()
        .map(|action| {
            let raw = match action {
                HotkeyAction::PriceHud => price_hud,
                HotkeyAction::LiteFloat => lite_float,
                HotkeyAction::ExpToggle => exp_toggle,
            };
            normalize(*action, raw)
        })
        .collect();

    // 先查重：三个键里有撞在一起的，**一个都不注册**。
    // 交给系统去撞的结果是「谁先注册谁生效」，而注册顺序是实现细节 ——
    // 用户看到的是「按了这个键弹出的是另一个窗口」，最难查的那类问题。
    let mut duplicates: Vec<Option<&'static str>> = vec![None; actions.len()];
    for i in 0..specs.len() {
        for j in (i + 1)..specs.len() {
            if specs[i].eq_ignore_ascii_case(&specs[j]) {
                duplicates[i] = Some(actions[j].label());
                duplicates[j] = Some(actions[i].label());
            }
        }
    }

    let mut results = Vec::new();
    for (index, action) in actions.iter().enumerate() {
        if let Some(other) = duplicates[index] {
            let message = format!(
                "「{}」和「{}」的快捷键重复了，两边都暂时没用 —— 请改掉其中一个",
                specs[index], other
            );
            log::warn!("{}（{}）", message, action.label());
            results.push(HotkeyStatus {
                action: action.code().to_string(),
                label: action.label().to_string(),
                hotkey: specs[index].clone(),
                registered: false,
                error: Some(message),
                default_hotkey: action.default_hotkey().to_string(),
            });
            continue;
        }
        results.push(register_one(&manager, *action, &specs[index]));
    }

    *status_slot().write() = results.clone();

    // 把结果广播出去：界面上的「按 X 切换显示」这类提示要跟着改。
    // 之前是只在窗口挂载时读一次 —— 主窗口一直挂着，所以在设置页改完键，
    // 旧提示会一直停在原来的键上（用户报的就是「改了键，别处还写着 Alt+F」）。
    use tauri::Emitter;
    if let Err(err) = app.emit("hotkeys-changed", &results) {
        log::debug!("广播快捷键状态失败（一般是因为还在启动阶段）：{}", err);
    }

    results
}

fn register_one(
    manager: &tauri_plugin_global_shortcut::GlobalShortcut<tauri::Wry>,
    action: HotkeyAction,
    spec: &str,
) -> HotkeyStatus {
    let normalized = normalize(action, spec);
    let mut status = HotkeyStatus {
        action: action.code().to_string(),
        label: action.label().to_string(),
        hotkey: normalized.clone(),
        registered: false,
        error: None,
        default_hotkey: action.default_hotkey().to_string(),
    };

    let parsed = match normalized.parse::<Shortcut>() {
        Ok(shortcut) => shortcut,
        Err(err) => {
            let message = format!("不认识的快捷键「{}」：{}", normalized, err);
            log::warn!("{}（{}）", message, action.label());
            status.error = Some(message);
            return status;
        }
    };

    match manager.register(parsed) {
        Ok(()) => {
            log::info!("{}快捷键已注册：{}", action.label(), status.hotkey);
            status.registered = true;
        }
        Err(err) => {
            let message = format!(
                "「{}」没有被系统接受（多半被别的程序占用，或者和另一个悬浮窗的键撞了）：{}",
                status.hotkey, err
            );
            log::warn!("{}（{}）", message, action.label());
            status.error = Some(message);
        }
    }

    status
}

/// 某个悬浮窗是否正开着（设置页的「测试呼出」按钮用它回显结果：窗口显示是异步
/// 投递的，点完立刻读会读到切换前的状态）。
pub fn window_visible(app: &tauri::AppHandle, action: HotkeyAction) -> bool {
    use tauri::Manager;
    app.get_webview_window(action.window_label())
        .and_then(|window| window.is_visible().ok())
        .unwrap_or(false)
}
