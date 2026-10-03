//! 托盘「退出程序」的防误触。
//!
//! ## 踩到的是什么
//!
//! 清图标缓存时手动重启了一次资源管理器，日志里当场出现：
//!
//! ```text
//! [10:03:00][mxd_box_lib][INFO] 托盘菜单 → 退出程序
//! ```
//!
//! 没人点它 —— 是 Windows 在 shell 被强杀时**补发了一个陈旧的托盘菜单事件**，
//! 而我们的处理是「听到就退出、没有任何防护」。对用户来说这是最难查的一类怪事：
//! 「重启一下资源管理器，枫之助就没了」。
//!
//! ## 判据
//!
//! 两个事实里任意一个成立，才算「这次退出是真的」：
//!
//! 1. **任务栏窗口还在**（`Shell_TrayWnd`）。shell 被拆掉的那一瞬间它是不存在的 ——
//!    菜单都跟着一起没了，自然不可能有人在点它；
//! 2. 或者，**这一段时间里确实右键点过托盘图标**（正常的退出一定紧跟在那之后）。
//!
//! 两条取「或」是故意的：这条判据的**失败方向必须是「还能退出」**。
//! 如果某个系统版本上类名不一样（第 1 条永远为假）、或者右键事件没上报
//! （第 2 条永远为假），只要另一条成立就不影响正常使用；
//! 只有两条都不成立时才忽略事件，而那正好就是 shell 已经不在的时刻。

use std::time::{Duration, Instant};

/// 右键托盘图标之后多久内的「退出程序」才算数。
/// 给得宽松一些：菜单开着不动、想清楚再点，是很正常的行为。
const CONTEXT_CLICK_WINDOW: Duration = Duration::from_secs(120);

/// 这个「退出程序」菜单事件值不值得当真。
pub fn should_honor_quit(
    tray_window_exists: bool,
    last_context_click: Option<Instant>,
    now: Instant,
) -> bool {
    if tray_window_exists {
        return true;
    }
    matches!(
        last_context_click,
        Some(stamp) if now.saturating_duration_since(stamp) <= CONTEXT_CLICK_WINDOW
    )
}

/// 任务栏（shell 的托盘宿主窗口）现在还在不在。
pub fn tray_window_exists() -> bool {
    #[cfg(windows)]
    {
        win::find_class("Shell_TrayWnd") || win::find_class("Shell_SecondaryTrayWnd")
    }
    #[cfg(not(windows))]
    {
        true
    }
}

#[cfg(windows)]
mod win {
    type Hwnd = isize;

    #[link(name = "user32")]
    extern "system" {
        fn FindWindowW(class: *const u16, window: *const u16) -> Hwnd;
    }

    pub fn find_class(class: &str) -> bool {
        let mut wide: Vec<u16> = class.encode_utf16().collect();
        wide.push(0);
        unsafe { FindWindowW(wide.as_ptr(), std::ptr::null()) != 0 }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normal_quit_is_honoured() {
        let now = Instant::now();
        // 任务栏在，且刚才右键过 —— 正常退出
        assert!(should_honor_quit(true, Some(now), now));
        // 任务栏在，但右键事件没上报（某些系统/版本）—— 仍然要能退出
        assert!(should_honor_quit(true, None, now));
    }

    #[test]
    fn explorer_restart_event_is_ignored() {
        let now = Instant::now();
        // shell 不在了、也没有任何右键 —— 正是那次误退出的场景
        assert!(!should_honor_quit(false, None, now));
        // 很久以前右键过，但 shell 已经没了 —— 同样不算数
        let long_ago = now - Duration::from_secs(600);
        assert!(!should_honor_quit(false, Some(long_ago), now));
    }

    #[test]
    fn recent_context_click_still_saves_the_day() {
        let now = Instant::now();
        // 类名判断在这台机器上失灵，但用户刚右键过 —— 别把正常退出拦掉
        assert!(should_honor_quit(false, Some(now), now));
        assert!(should_honor_quit(
            false,
            Some(now - Duration::from_secs(60)),
            now
        ));
    }
}
