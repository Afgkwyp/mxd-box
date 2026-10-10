use parking_lot::{Mutex, RwLock};
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Instant;
use sysinfo::System;
use tauri::{AppHandle, Emitter};
use tokio::time::{sleep, Duration};

/// 连续在线上多久才提醒（挡住打开程序、切场景时的瞬时尖峰）。
///
/// 采样循环每 2 秒一次，所以 30 秒约等于连续 15 次采样在线上。下面这三个数
/// （30 秒 / 回落 5 个百分点 / 最短 10 分钟）都是参照金价提醒定的起点，
/// **没有实测依据**，发出去之后按群里的反馈调。
const ALERT_SUSTAIN: Duration = Duration::from_secs(30);

/// 响过之后，占用要回落到「警戒线 − 这个百分点」以下才重新上膛
/// （例如线是 85%，要回到 80% 以下）。
const ALERT_REARM_DROP: f32 = 5.0;

/// 两次提醒至少隔这么久。兜底：即使规则上已经重新上膛，也不连响。
const ALERT_MIN_INTERVAL: Duration = Duration::from_secs(600);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryPayload {
    pub total_mb: u64,
    pub used_mb: u64,
    pub percent: f32,
    pub threshold: u32,
    pub is_warning: bool,
}

/// 内存到线提醒的判定状态。只存在内存里（不落库），程序重启即清。
///
/// 时间用单调时钟的 `Instant`：睡眠期间的时长算不算进去取决于系统，最坏情况
/// 是唤醒后多等一会儿才响，不会因为墙钟跳变而误响。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct MemoryAlertState {
    /// 连续在线的起点；`None` = 现在不在线上（或还没开始累计）
    over_since: Option<Instant>,
    /// 已经响过、还没退回到「警戒线 − 5 个百分点」以下
    fired: bool,
    /// 上一次响的时刻（最短间隔用它算）
    last_fired: Option<Instant>,
}

/// 这一次该不该响，以及响完 / 没响之后状态变成什么。
///
/// 照 `meso_alert::decide` 的形态写：纯函数，不碰 `AppHandle`、不读机器内存，
/// `now` 由调用方传进来 —— 单测因此可以随意拨表，不真的等 30 秒、10 分钟。
///
/// 规则（见 `docs/specs/memory-alert.md`）：
///
/// * 开关关着、游戏没在跑、占用没到线 → 不响，持续计数清零；
/// * 连续 30 秒在线上才响（中间一掉回线下就重新计数）；
/// * 响过之后不重复响，直到占用退回到「警戒线 − 5 个百分点」以下重新上膛；
/// * 即使重新上膛了，距上一次提醒不满 10 分钟也不响
///   （满了、且「连续 30 秒在线上」还成立时才响）。
pub fn decide(
    percent: f32,
    threshold: u32,
    enabled: bool,
    game_running: bool,
    state: &MemoryAlertState,
    now: Instant,
) -> (bool, MemoryAlertState) {
    let mut next = *state;
    let line = threshold as f32;

    // 「已经响过」的解除条件只看占用有没有退够 —— 退不够就在阈值附近来回抖，
    // 不会响第二次。
    if next.fired && percent < line - ALERT_REARM_DROP {
        next.fired = false;
    }

    // 不满足「可以响」的前置条件时不累计持续时间。`fired` 保留：游戏关掉再开、
    // 中途离开线上一下，都不该让「已经响过」这个结论失效。
    if !enabled || !game_running || percent < line {
        next.over_since = None;
        return (false, next);
    }

    let since = *next.over_since.get_or_insert(now);
    let sustained = now.duration_since(since) >= ALERT_SUSTAIN;
    let cooled = next
        .last_fired
        .map(|last| now.duration_since(last) >= ALERT_MIN_INTERVAL)
        .unwrap_or(true);

    if !next.fired && sustained && cooled {
        next.fired = true;
        next.last_fired = Some(now);
        return (true, next);
    }

    (false, next)
}

/// 提醒窗里的正文。数字口径和侧栏、两个悬浮窗上显示的一致。
fn alert_body(payload: &MemoryPayload) -> String {
    format!(
        "内存占用 {:.0}%（已用 {:.1} GB / 共 {:.0} GB），到了你设的 {}% 警戒线。建议进商城刷新。",
        payload.percent,
        payload.used_mb as f64 / 1024.0,
        payload.total_mb as f64 / 1024.0,
        payload.threshold
    )
}

pub struct MemoryMonitor {
    pub threshold: Arc<AtomicU32>,
    /// 到线时提醒开关（设置页和总览页的两个开关写的是同一个值）。
    pub alert_enabled: Arc<AtomicBool>,
    /// 到线提醒的判定状态；改警戒线 / 拨开关时清零（见 `set_threshold`）。
    alert_state: Arc<Mutex<MemoryAlertState>>,
    /// 最近一次采样的结果。
    ///
    /// 界面刚打开时先来问一次 —— 否则第一帧只能显示一个编造的占用率
    /// （曾经写死成 16384/7200/44%），在一段时间里看着像真的。
    latest: Arc<RwLock<Option<MemoryPayload>>>,
}

impl MemoryMonitor {
    pub fn new(initial_threshold: u32, alert_enabled: bool) -> Self {
        Self {
            threshold: Arc::new(AtomicU32::new(initial_threshold)),
            alert_enabled: Arc::new(AtomicBool::new(alert_enabled)),
            alert_state: Arc::new(Mutex::new(MemoryAlertState::default())),
            latest: Arc::new(RwLock::new(None)),
        }
    }

    pub fn set_threshold(&self, val: u32) {
        self.threshold.store(val, Ordering::Relaxed);
        // 警戒线变了：旧线下的「已经响过 / 连续了多久」都不算数，按新线重新判断
        // （和金价提醒改阈值的处理一致）。
        self.reset_alert_state();
    }

    /// 开 / 关到线提醒。开关变化同样重新判断 —— 如果此刻已经在线上，
    /// 连续 30 秒后会响一次（用户刚打开就该知道现在是超的）。
    pub fn set_alert_enabled(&self, enabled: bool) {
        self.alert_enabled.store(enabled, Ordering::Relaxed);
        self.reset_alert_state();
    }

    fn reset_alert_state(&self) {
        *self.alert_state.lock() = MemoryAlertState::default();
    }

    #[allow(dead_code)]
    pub fn get_threshold(&self) -> u32 {
        self.threshold.load(Ordering::Relaxed)
    }

    /// 最近一次采样（主循环每 2 秒刷一次，所以基本总是有值）。
    pub fn latest(&self) -> Option<MemoryPayload> {
        self.latest.read().clone()
    }

    pub fn start_loop(&self, app_handle: AppHandle) {
        let threshold_arc = Arc::clone(&self.threshold);
        let alert_enabled = Arc::clone(&self.alert_enabled);
        let alert_state = Arc::clone(&self.alert_state);
        let latest = Arc::clone(&self.latest);

        tauri::async_runtime::spawn(async move {
            let mut sys = System::new_all();

            loop {
                sys.refresh_memory();

                let total = sys.total_memory();
                let used = sys.used_memory();

                let percent = if total > 0 {
                    ((used as f64 / total as f64) * 100.0) as f32
                } else {
                    0.0
                };

                let current_thresh = threshold_arc.load(Ordering::Relaxed);
                let is_warning = percent >= current_thresh as f32;

                let payload = MemoryPayload {
                    total_mb: total / 1024 / 1024,
                    used_mb: used / 1024 / 1024,
                    percent,
                    threshold: current_thresh,
                    is_warning,
                };

                *latest.write() = Some(payload.clone());
                let _ = app_handle.emit("memory-update", &payload);

                // 到线提醒：判定交给纯函数（`decide`），这里只喂数据、接出口。
                //
                // 「游戏是否在跑」只在真有可能响（开关开着且已经过线）时才去查 ——
                // `find_game_window` 要遍历一遍窗口，没必要每 2 秒都做。
                let enabled = alert_enabled.load(Ordering::Relaxed);
                let game_running = enabled
                    && is_warning
                    && crate::exp::capture::find_game_window().is_some();
                let fire = {
                    let mut state = alert_state.lock();
                    let (fire, next) = decide(
                        percent,
                        current_thresh,
                        enabled,
                        game_running,
                        &state,
                        Instant::now(),
                    );
                    *state = next;
                    fire
                };
                if fire {
                    crate::alert::fire_memory_alert(&app_handle, alert_body(&payload));
                }

                sleep(Duration::from_secs(2)).await;
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 判定是纯函数：测试里直接拨表，不真的等 30 秒 / 10 分钟，也不碰机器内存。
    fn at(base: Instant, secs: u64) -> Instant {
        base + Duration::from_secs(secs)
    }

    /// 1. 占用一直在线下 → 不响。
    #[test]
    fn staying_below_the_line_never_fires() {
        let base = Instant::now();
        let mut state = MemoryAlertState::default();
        for i in 0..600 {
            let (fire, next) = decide(60.0, 85, true, true, &state, at(base, i * 2));
            assert!(!fire);
            state = next;
        }
    }

    /// 2. 过线但不满 30 秒就回落 → 不响，计数清零（再次过线要重新攒够 30 秒）。
    #[test]
    fn a_short_spike_does_not_fire_and_resets_the_count() {
        let base = Instant::now();
        let (fire, state) = decide(90.0, 85, true, true, &MemoryAlertState::default(), at(base, 0));
        assert!(!fire);
        let (fire, state) = decide(90.0, 85, true, true, &state, at(base, 28));
        assert!(!fire, "28 秒还不到 30 秒");
        let (fire, state) = decide(70.0, 85, true, true, &state, at(base, 30));
        assert!(!fire, "掉到线下只是清零，不响");
        // 再次过线：必须从头连续 30 秒
        let (fire, state) = decide(90.0, 85, true, true, &state, at(base, 32));
        assert!(!fire);
        let (fire, _) = decide(90.0, 85, true, true, &state, at(base, 60));
        assert!(!fire, "从 32 秒起才 28 秒");
        let (fire, _) = decide(90.0, 85, true, true, &state, at(base, 62));
        assert!(fire, "重新连续 30 秒了，该响");
    }

    /// 3 + 4. 连续 30 秒在线上 → 响一次；之后一直在线 / 只小幅回落都不再响。
    #[test]
    fn sustained_alert_fires_once_and_not_again_while_it_stays_high() {
        let base = Instant::now();
        let mut state = MemoryAlertState::default();
        let mut fired_at = Vec::new();
        for i in 0..=16 {
            let (fire, next) = decide(90.0, 85, true, true, &state, at(base, i * 2));
            state = next;
            if fire {
                fired_at.push(i * 2);
            }
        }
        assert_eq!(fired_at, vec![30], "从第一次过线起第 30 秒响，且只响一次");
        // 一直在线：再过 20 分钟也不响
        let (fire, state) = decide(90.0, 85, true, true, &state, at(base, 1_200));
        assert!(!fire);
        // 只回落一点点（没到线下 5 个百分点）：也不响
        let (fire, _) = decide(88.0, 85, true, true, &state, at(base, 1_800));
        assert!(!fire);
    }

    /// 5. 响过之后回落到线下 3 个百分点再回升 → 不响（没退够，没重新上膛）。
    #[test]
    fn hovering_within_five_points_does_not_rearm() {
        let base = Instant::now();
        let (_, state) = decide(90.0, 85, true, true, &MemoryAlertState::default(), at(base, 0));
        let (fire, state) = decide(90.0, 85, true, true, &state, at(base, 30));
        assert!(fire);
        // 82%：比线下低 3 个点，没退够 5 个点
        let (fire, state) = decide(82.0, 85, true, true, &state, at(base, 120));
        assert!(!fire);
        // 回升并持续 30 秒：仍然不响
        let (fire, state) = decide(88.0, 85, true, true, &state, at(base, 180));
        assert!(!fire);
        let (fire, _) = decide(88.0, 85, true, true, &state, at(base, 210));
        assert!(!fire, "没重新上膛，连续再久也不响");
    }

    /// 6. 退够 5 个点重新上膛，但距上次不满 10 分钟仍不响；满了且条件还在就响。
    #[test]
    fn the_minimum_interval_holds_a_second_alert_until_ten_minutes() {
        let base = Instant::now();
        let (_, state) = decide(90.0, 85, true, true, &MemoryAlertState::default(), at(base, 0));
        let (fire, state) = decide(90.0, 85, true, true, &state, at(base, 30));
        assert!(fire);
        // 退够 5 个点（80% 以下）：重新上膛，但这次不算响
        let (fire, state) = decide(79.0, 85, true, true, &state, at(base, 60));
        assert!(!fire);
        // 回升并持续 30 秒：距上次才 2 分钟，不响
        let (fire, state) = decide(90.0, 85, true, true, &state, at(base, 120));
        assert!(!fire);
        let (fire, state) = decide(90.0, 85, true, true, &state, at(base, 150));
        assert!(!fire, "距上次不满 10 分钟");
        // 条件一直没变、等到满 10 分钟（上次响在 30 秒，所以是 630 秒）：响
        let (fire, state) = decide(90.0, 85, true, true, &state, at(base, 629));
        assert!(!fire);
        let (fire, _) = decide(90.0, 85, true, true, &state, at(base, 630));
        assert!(fire, "满 10 分钟且仍满足条件，响一次");
    }

    /// 7. 开关关着 → 任何情况都不响，也不累计持续时间。
    #[test]
    fn the_switch_off_never_fires_and_does_not_count() {
        let base = Instant::now();
        let mut state = MemoryAlertState::default();
        for i in 0..600 {
            let (fire, next) = decide(99.0, 85, false, true, &state, at(base, i));
            assert!(!fire);
            state = next;
        }
        // 打开开关后，仍要重新连续 30 秒才响（命令层会顺带把状态清零）
        let (_, state) = decide(99.0, 85, true, true, &state, at(base, 601));
        let (fire, _) = decide(99.0, 85, true, true, &state, at(base, 630));
        assert!(!fire);
        let (fire, _) = decide(99.0, 85, true, true, &state, at(base, 631));
        assert!(fire);
    }

    /// 8. 游戏没在跑 → 不响；游戏开起来后要重新连续 30 秒才响。
    #[test]
    fn the_game_must_be_running() {
        let base = Instant::now();
        let mut state = MemoryAlertState::default();
        for i in 0..600 {
            let (fire, next) = decide(99.0, 85, true, false, &state, at(base, i));
            assert!(!fire);
            state = next;
        }
        let (_, state) = decide(99.0, 85, true, true, &state, at(base, 700));
        let (fire, _) = decide(99.0, 85, true, true, &state, at(base, 729));
        assert!(!fire);
        let (fire, _) = decide(99.0, 85, true, true, &state, at(base, 730));
        assert!(fire);
    }

    /// 9. 改警戒线 / 拨开关会把判定状态清零：原来「已响过」，改完如果仍在新线上，
    ///    连续 30 秒后可以再响一次。
    #[test]
    fn changing_the_line_or_the_switch_resets_the_state() {
        let monitor = MemoryMonitor::new(85, true);
        let base = Instant::now();

        // 先让它响过一次，并处于「已响过」状态
        let (_, state) = decide(90.0, 85, true, true, &MemoryAlertState::default(), at(base, 0));
        let (fire, state) = decide(90.0, 85, true, true, &state, at(base, 30));
        assert!(fire);
        *monitor.alert_state.lock() = state;

        // 改警戒线：状态清零，按新线重新判断
        monitor.set_threshold(80);
        assert_eq!(*monitor.alert_state.lock(), MemoryAlertState::default());
        let (_, state) = decide(85.0, 80, true, true, &MemoryAlertState::default(), at(base, 60));
        let (fire, _) = decide(85.0, 80, true, true, &state, at(base, 90));
        assert!(fire, "85% 仍在新线 80% 上，连续 30 秒后要能再响");

        // 拨开关同样清零
        *monitor.alert_state.lock() = state;
        monitor.set_alert_enabled(false);
        assert_eq!(*monitor.alert_state.lock(), MemoryAlertState::default());
    }
}
