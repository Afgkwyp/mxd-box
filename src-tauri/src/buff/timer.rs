//! 一个 BUFF 的状态：在不在、还剩几秒、该不该提醒。纯逻辑，不碰画面，方便单测。
//!
//! 思路照网页版（枫记）的 `tracker.ts` / `timer.ts` / `reminders.ts`，做了简化：
//!
//! # 在不在（防抖）
//!
//! 连着 2 帧认到才算「出现」；连着 3 帧没认到、并且至少过了 1 秒才算「消失」。
//! 一帧没认到多半是技能特效或者鼠标挡了一下，不能当成 BUFF 没了。
//! 每次「消失 → 出现」算一轮新的（`cycle` + 1），提醒按轮记，一轮只响一次。
//!
//! # 还剩几秒（终点区间）
//!
//! 图标上显示 `n` 说明剩余时间在 `[n, n+1)` 秒，也就是终点落在 `[t+n, t+n+1)`。
//! 多读几次，把这些区间**求交集**，就能把终点钉得比一秒还准，而且一两次读错会因为
//! 交集变空而被发现，不会直接报出来：
//!
//! * 至少 3 次读数、3 个不同的数、跨度 ≥ 2 秒、交集不空，才算「定住了」；
//! * 定住之后，读数和终点对不上：连续 2 次才认（一次多半是读错），
//!   终点变晚 = 重新放了技能（续上了，新的一轮），变早 = 之前定错了，都从头来；
//! * 终点已经过了图标还在：也是续上了（倒计时数字常常只在最后一分钟才显示）。
//! * 还没定住时数字**往上跳**了（比上一次大 2 以上）：前面的读数不属于这一段倒计时
//!   （读错 / 续上了），清掉重新攒 —— 网页版 `timer.ts` 同样的处理，不然旧读数要一条条
//!   被交集挤掉，核对会拖很久。
//!
//! # 分钟（白字）
//!
//! 剩一分钟以上时游戏显示白字的分钟数（`Unit::Minutes`）。它**不进**秒数的交集（不然
//! 白字「1」会被当成 1 秒），只用来：界面显示「剩 N 分多」；以及判断续上了 —— 秒数阶段
//! 之后又连着 2 次看到分钟，说明技能重新放过，换新的一轮。
//!
//! # 提醒
//!
//! 剩余时间的**上限**降到阈值以下时提醒「即将结束」，一轮一次；
//! 可选在图标消失时提醒「已结束」（默认关）。

use super::digits::Unit;

/// 读数允许的误差（秒）：抓屏时刻和画面实际时刻差一点点。
const PAD: f64 = 0.25;
/// 定住终点需要的读数条件。
const MIN_READINGS: usize = 3;
const MIN_DISTINCT: usize = 3;
const MIN_SPAN: f64 = 2.0;
/// 读数最多留这么多条（只用来求交集，旧的意义不大）。
const MAX_READINGS: usize = 24;
/// 连续几次对不上才推翻已经定住的终点。
const CONFLICTS_TO_RESET: u32 = 2;
/// 终点要晚这么多秒以上，才算续上（比读错的误差大）。
const RENEW_GAP: f64 = 2.0;
/// 续上要连着这么多次读数彼此对得上（约 1.5 秒，抗得住单次或两次大误读）。
const RENEW_READINGS: usize = 3;
/// 终点过去这么久图标还在：认为续上了。
const OVERDUE_GRACE: f64 = 1.5;
/// 秒数阶段之后连着几次看到分钟才算续上。
const MINUTES_TO_RENEW: u32 = 2;
/// 读数多久以内还拿来给界面显示。
const DISPLAY_FRESH: f64 = 2.0;
/// 出现 / 消失的防抖。
const HITS_TO_PRESENT: u32 = 2;
const MISSES_TO_MISSING: u32 = 3;
const MISSING_AFTER: f64 = 1.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Presence {
    Unknown,
    Present,
    Missing,
}

/// 这一帧要发出的提醒。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reminder {
    /// 即将结束（带提醒那一刻剩余秒数的上限，取整）
    Nearing(u32),
    /// 已结束
    Expired,
}

#[derive(Debug, Clone)]
pub struct BuffTimer {
    pub presence: Presence,
    pub cycle: u32,
    hits: u32,
    misses: u32,
    miss_since: Option<f64>,
    readings: Vec<(f64, u32)>,
    /// 定住的终点区间（绝对时间，秒）
    endpoint: Option<(f64, f64)>,
    conflicts: u32,
    /// 终点之后明显更晚的读数，还没凑够 `RENEW_READINGS` 次一致的（凑够了才算续上）
    pending: Vec<(f64, u32)>,
    /// 最近一次读到的数和时刻（界面上「还没定住」时也想显示个大概）
    last_reading: Option<(f64, u32)>,
    /// 最近一次读到的分钟数和时刻
    last_minutes: Option<(f64, u32)>,
    /// 连着读到分钟的次数（读到秒数清零）
    minute_streak: u32,
    nearing_cycle: Option<u32>,
    expired_cycle: Option<u32>,
}

impl Default for BuffTimer {
    fn default() -> Self {
        Self {
            presence: Presence::Unknown,
            cycle: 0,
            hits: 0,
            misses: 0,
            miss_since: None,
            readings: Vec::new(),
            endpoint: None,
            conflicts: 0,
            pending: Vec::new(),
            last_reading: None,
            last_minutes: None,
            minute_streak: 0,
            nearing_cycle: None,
            expired_cycle: None,
        }
    }
}

fn interval(t: f64, n: u32) -> (f64, f64) {
    (t + n as f64 - PAD, t + n as f64 + 1.0 + PAD)
}

impl BuffTimer {
    /// 换一轮：清掉这一轮的读数。
    fn new_cycle(&mut self) {
        self.cycle += 1;
        self.readings.clear();
        self.endpoint = None;
        self.conflicts = 0;
        self.pending.clear();
        self.last_reading = None;
        self.last_minutes = None;
        self.minute_streak = 0;
    }

    /// 画面的尺寸或界面倍数变了：之前的读数都是按旧画面读的，清掉重来。在不在、这一轮的提醒记录保留，不重复念。
    pub fn forget_readings(&mut self) {
        self.readings.clear();
        self.endpoint = None;
        self.conflicts = 0;
        self.pending.clear();
        self.last_reading = None;
        self.last_minutes = None;
        self.minute_streak = 0;
    }

    /// 喂一帧：`seen` = 这帧认到了图标，`reading` = 读到的数和单位。
    pub fn observe(&mut self, now: f64, seen: bool, reading: Option<(u32, Unit)>) {
        if seen {
            self.hits += 1;
            self.misses = 0;
            self.miss_since = None;
            if self.presence != Presence::Present && self.hits >= HITS_TO_PRESENT {
                self.presence = Presence::Present;
                self.new_cycle();
            }
        } else {
            self.hits = 0;
            self.misses += 1;
            let since = *self.miss_since.get_or_insert(now);
            if self.presence != Presence::Missing
                && self.misses >= MISSES_TO_MISSING
                && now - since >= MISSING_AFTER
            {
                self.presence = Presence::Missing;
            }
        }
        if self.presence != Presence::Present {
            return;
        }
        if let Some((_, hi)) = self.endpoint {
            if now > hi + OVERDUE_GRACE {
                // 终点过了图标还在：续上了
                self.new_cycle();
            }
        }
        match reading.filter(|_| seen) {
            Some((m, Unit::Minutes)) if m > 0 => self.add_minutes(now, m),
            Some((n, Unit::Seconds)) => {
                self.minute_streak = 0;
                self.last_minutes = None;
                self.add_reading(now, n);
            }
            _ => {}
        }
    }

    fn add_minutes(&mut self, now: f64, m: u32) {
        self.last_minutes = Some((now, m));
        self.minute_streak += 1;
        let had_seconds = self.endpoint.is_some() || !self.readings.is_empty();
        if had_seconds && self.minute_streak >= MINUTES_TO_RENEW {
            // 秒数阶段之后又是分钟：续上了
            if self.endpoint.is_some() {
                self.new_cycle();
                self.last_minutes = Some((now, m));
            } else {
                self.readings.clear();
                self.last_reading = None;
            }
        }
    }

    fn add_reading(&mut self, now: f64, n: u32) {
        if self.endpoint.is_none() && self.readings.last().is_some_and(|&(_, last)| n > last + 2) {
            // 还没定住时数往上跳：之前的读数不是这一段倒计时的
            self.readings.clear();
        }
        self.last_reading = Some((now, n));
        let (lo, hi) = interval(now, n);
        if let Some((end_lo, end_hi)) = self.endpoint {
            if lo < end_hi && hi > end_lo {
                // 对得上：收窄
                self.conflicts = 0;
                self.pending.clear();
                self.endpoint = Some((end_lo.max(lo), end_hi.min(hi)));
                return;
            }
            if lo >= end_hi + RENEW_GAP {
                // 终点明显变晚：可能是续上了，也可能是一次大的误读（比如 9 读成 18）。
                // 要连着 RENEW_READINGS 次读数彼此对得上（终点相差在 1 秒内）才算续上，
                // 单次或两次大误读攒不够，不会重新念一遍同一段倒计时。
                self.conflicts = 0;
                if !self.pending_agrees(now, n, lo, hi) {
                    self.pending.clear();
                }
                self.pending.push((now, n));
                if self.pending.len() < RENEW_READINGS {
                    self.last_reading = Some((now, n));
                    return;
                }
                let moved = std::mem::take(&mut self.pending);
                self.new_cycle();
                self.readings = moved;
                self.last_reading = Some((now, n));
                self.try_establish();
                return;
            }
            // 变早或只差一点：读错了，从头读。只差一点不能当续上（否则连着读错会让同一段倒计时再念一遍）。
            self.pending.clear();
            self.conflicts += 1;
            if self.conflicts < CONFLICTS_TO_RESET {
                return;
            }
            self.readings.clear();
            self.endpoint = None;
            self.conflicts = 0;
            self.last_reading = Some((now, n));
        }
        self.readings.push((now, n));
        if self.readings.len() > MAX_READINGS {
            self.readings.remove(0);
        }
        self.try_establish();
    }

    /// 这次读数能不能和待定的续上读数互相印证：终点估计差在 1 秒内，而且区间有交集。
    fn pending_agrees(&self, now: f64, n: u32, lo: f64, hi: f64) -> bool {
        let Some(&(t0, n0)) = self.pending.first() else {
            return true;
        };
        if (now + n as f64 - (t0 + n0 as f64)).abs() > 1.0 {
            return false;
        }
        let (cl, ch) = self.pending.iter().fold((f64::MIN, f64::MAX), |(a, b), &(t, m)| {
            let (x, y) = interval(t, m);
            (a.max(x), b.min(y))
        });
        lo < ch && hi > cl
    }

    fn try_establish(&mut self) {
        let r = &self.readings;
        if r.len() < MIN_READINGS {
            return;
        }
        let mut values: Vec<u32> = r.iter().map(|&(_, n)| n).collect();
        values.sort_unstable();
        values.dedup();
        let span = r[r.len() - 1].0 - r[0].0;
        if values.len() < MIN_DISTINCT || span < MIN_SPAN {
            return;
        }
        let (lo, hi) = r.iter().fold((f64::MIN, f64::MAX), |(lo, hi), &(t, n)| {
            let (a, b) = interval(t, n);
            (lo.max(a), hi.min(b))
        });
        if lo < hi {
            self.endpoint = Some((lo, hi));
            self.conflicts = 0;
        } else {
            // 交集是空的：里面有读错的。丢掉最老的一条，等新读数
            self.readings.remove(0);
        }
    }

    /// 终点定住了吗。
    pub fn established(&self) -> bool {
        self.presence == Presence::Present && self.endpoint.is_some()
    }

    /// 剩余秒数的上限（定住了才有）。
    pub fn remaining_upper(&self, now: f64) -> Option<f64> {
        if self.presence != Presence::Present {
            return None;
        }
        self.endpoint.map(|(_, hi)| (hi - PAD - now).max(0.0))
    }

    /// 给界面显示的剩余秒数：定住了用终点；还没定住、刚读到过就用那次的读数往下推。
    pub fn display_seconds(&self, now: f64) -> Option<u32> {
        if self.presence != Presence::Present {
            return None;
        }
        if let Some((lo, _)) = self.endpoint {
            return Some((lo + PAD - now).max(0.0).floor() as u32);
        }
        let (t, n) = self.last_reading?;
        let age = now - t;
        (age <= DISPLAY_FRESH).then(|| (n as f64 - age).max(0.0).floor() as u32)
    }

    /// 给界面显示的「剩 N 分多」：秒数还没定住、最近读到的是白字分钟时才有。
    pub fn display_minutes(&self, now: f64) -> Option<u32> {
        if self.presence != Presence::Present || self.endpoint.is_some() {
            return None;
        }
        let (t, m) = self.last_minutes?;
        (now - t <= DISPLAY_FRESH).then_some(m)
    }

    /// 这一刻要不要提醒。`threshold` 是「剩几秒时提醒」。
    pub fn reminder(&mut self, now: f64, threshold: u32, expire_alert: bool) -> Option<Reminder> {
        if self.presence == Presence::Missing {
            // 这一轮出现过、还没报过结束
            if expire_alert && self.cycle > 0 && self.expired_cycle != Some(self.cycle) {
                self.expired_cycle = Some(self.cycle);
                return Some(Reminder::Expired);
            }
            return None;
        }
        let upper = self.remaining_upper(now)?;
        if upper <= threshold as f64 && self.nearing_cycle != Some(self.cycle) {
            self.nearing_cycle = Some(self.cycle);
            return Some(Reminder::Nearing(upper.ceil() as u32));
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 模拟一个从 `start` 秒开始、还剩 `remaining` 秒的 BUFF，每 0.5 秒看一眼。
    fn countdown(timer: &mut BuffTimer, from: f64, to: f64, end: f64) -> Vec<(f64, Reminder)> {
        let mut out = Vec::new();
        let mut t = from;
        while t < to {
            let left = end - t;
            // 剩一分钟以上是白字分钟数，最后一分钟是彩色秒数
            let reading = if left >= 60.0 {
                Some(((left / 60.0).floor() as u32, Unit::Minutes))
            } else {
                (left >= 0.0).then(|| (left.floor() as u32, Unit::Seconds))
            };
            timer.observe(t, true, reading);
            if let Some(r) = timer.reminder(t, 10, true) {
                out.push((t, r));
            }
            t += 0.5;
        }
        out
    }

    #[test]
    fn establishes_the_endpoint_and_reminds_once_near_the_end() {
        let mut timer = BuffTimer::default();
        let fired = countdown(&mut timer, 0.0, 29.0, 30.3);
        assert!(timer.established());
        // 终点钉得比一秒还准
        let upper = timer.remaining_upper(20.0).unwrap();
        assert!((upper - 10.3).abs() < 0.5, "{upper}");
        // 只在剩 10 秒左右时响一次
        assert_eq!(fired.len(), 1, "{fired:?}");
        let (t, r) = fired[0];
        assert!(matches!(r, Reminder::Nearing(9..=11)), "{r:?}");
        assert!((19.0..=21.0).contains(&t), "{t}");
    }

    #[test]
    fn one_misread_does_not_move_the_endpoint() {
        let mut timer = BuffTimer::default();
        countdown(&mut timer, 0.0, 5.0, 50.0);
        assert!(timer.established());
        let before = timer.remaining_upper(5.0).unwrap();
        // 一帧把 44 读成了 14
        timer.observe(5.5, true, Some((14, Unit::Seconds)));
        assert_eq!(timer.remaining_upper(5.5).map(|u| (u - (before - 0.5)).abs() < 0.01), Some(true));
        // 下一帧正常：还是原来的终点，提醒也不会提前
        timer.observe(6.0, true, Some((43, Unit::Seconds)));
        assert!(timer.reminder(6.0, 10, false).is_none());
    }

    #[test]
    fn a_later_endpoint_means_the_buff_was_renewed() {
        let mut timer = BuffTimer::default();
        let fired = countdown(&mut timer, 0.0, 22.0, 30.0);
        assert_eq!(fired.len(), 1);
        let cycle = timer.cycle;
        // 第 22 秒重新放了技能：又是 59、58……
        let fired = countdown(&mut timer, 22.0, 82.0, 22.0 + 59.9);
        assert!(timer.cycle > cycle, "续上算新的一轮");
        assert_eq!(fired.iter().filter(|(_, r)| matches!(r, Reminder::Nearing(_))).count(), 1);
    }

    #[test]
    fn brief_occlusion_is_not_an_expiry_but_a_real_disappearance_is() {
        let mut timer = BuffTimer::default();
        timer.observe(0.0, true, None);
        timer.observe(0.5, true, None);
        assert_eq!(timer.presence, Presence::Present);
        // 被挡了两帧
        timer.observe(1.0, false, None);
        timer.observe(1.5, false, None);
        timer.observe(2.0, true, None);
        assert_eq!(timer.presence, Presence::Present);
        assert_eq!(timer.cycle, 1);
        // 真的没了
        for i in 0..4 {
            timer.observe(3.0 + i as f64 * 0.5, false, None);
        }
        assert_eq!(timer.presence, Presence::Missing);
        assert_eq!(timer.reminder(5.0, 10, true), Some(Reminder::Expired));
        assert_eq!(timer.reminder(5.5, 10, true), None, "只报一次");
        // 再出现：新的一轮
        timer.observe(8.0, true, None);
        timer.observe(8.5, true, None);
        assert_eq!(timer.cycle, 2);
    }

    #[test]
    fn overdue_icon_starts_a_new_cycle() {
        let mut timer = BuffTimer::default();
        countdown(&mut timer, 0.0, 10.0, 12.0);
        assert!(timer.established());
        let cycle = timer.cycle;
        // 终点过了，图标还在、但读不到数（续上了，时间又长了，数字不显示）
        for i in 0..8 {
            timer.observe(12.0 + i as f64 * 0.5, true, None);
        }
        assert!(timer.cycle > cycle);
        assert!(!timer.established());
    }

    /// 用户真机上的情况：2 分多钟的 BUFF，前面一直是白字分钟。
    #[test]
    fn minutes_show_as_minutes_and_seconds_lock_on_quickly_after() {
        let mut timer = BuffTimer::default();
        let fired = countdown(&mut timer, 0.0, 30.0, 150.3);
        assert!(fired.is_empty() && !timer.established());
        assert_eq!(timer.display_minutes(29.5), Some(2));
        assert_eq!(timer.display_seconds(29.5), None, "白字「2」不能当成 2 秒");
        // 进最后一分钟（第 90.3 秒起是 59、58……）：几秒内就要定住
        countdown(&mut timer, 30.0, 94.0, 150.3);
        assert!(timer.established(), "分钟读数不能拖住秒数的核对");
        assert_eq!(timer.display_minutes(93.5), None);
        let fired = countdown(&mut timer, 94.0, 150.0, 150.3);
        assert_eq!(fired.len(), 1, "{fired:?}");
        assert!((139.0..=141.5).contains(&fired[0].0), "{fired:?}");
    }

    #[test]
    fn minutes_after_the_seconds_phase_mean_the_buff_was_renewed() {
        let mut timer = BuffTimer::default();
        countdown(&mut timer, 0.0, 20.0, 40.0);
        assert!(timer.established());
        let cycle = timer.cycle;
        // 剩 20 秒时重新放了技能：又变回白字分钟
        let fired = countdown(&mut timer, 20.0, 200.0, 20.0 + 150.0);
        assert!(timer.cycle > cycle, "续上算新的一轮");
        assert_eq!(fired.len(), 1, "新一轮也要提醒一次：{fired:?}");
        assert!(fired[0].0 > 150.0, "不能按旧终点提醒：{fired:?}");
    }

    #[test]
    fn a_jump_up_before_locking_on_starts_fresh_evidence() {
        let mut timer = BuffTimer::default();
        timer.observe(0.0, true, Some((5, Unit::Seconds)));
        timer.observe(0.5, true, Some((5, Unit::Seconds)));
        timer.observe(1.0, true, Some((4, Unit::Seconds)));
        // 续上了：59 秒的新倒计时，3 个不同的数、跨 2 秒就该定住
        let fired = countdown(&mut timer, 1.5, 4.0, 61.2);
        assert!(fired.is_empty());
        assert!(timer.established());
        let upper = timer.remaining_upper(4.0).unwrap();
        assert!((upper - 57.2).abs() < 0.6, "{upper}");
    }

    #[test]
    fn no_reminder_without_enough_evidence() {
        let mut timer = BuffTimer::default();
        timer.observe(0.0, true, Some((8, Unit::Seconds)));
        timer.observe(0.5, true, Some((7, Unit::Seconds)));
        // 才两次读数：不报（宁可晚，不可错）
        assert!(timer.reminder(0.5, 10, false).is_none());
        assert_eq!(timer.display_seconds(0.5), Some(7));
    }

    /// 探针：只在秒数阶段（最后 30 秒）喂数，指定的几帧读错。返回所有提醒（时刻，剩余真实秒数）。
    fn misread_run(end: f64, bad_at: &[f64], bad: impl Fn(u32) -> u32) -> Vec<(f64, f64)> {
        let mut timer = BuffTimer::default();
        let mut out = Vec::new();
        let mut t = (end - 30.0).max(0.0);
        while t < end + 0.5 {
            let left = end - t;
            let reading = (left >= 0.0).then(|| {
                let n = left.floor() as u32;
                let v = if bad_at.iter().any(|&b| (b - t).abs() < 0.01) { bad(n) } else { n };
                (v, Unit::Seconds)
            });
            timer.observe(t, true, reading);
            if let Some(Reminder::Nearing(_)) = timer.reminder(t, 10, false) {
                out.push((t, end - t));
            }
            t += 0.5;
        }
        out
    }

    /// 真的续上（重新放了 BUFF，倒计时回到满）：3 帧内要看出来，之后新的一轮正常提醒一次。
    #[test]
    fn a_real_renewal_is_seen_within_three_frames_and_reminds_again() {
        let reading_at = |t: f64, end: f64| {
            let left = end - t;
            if left >= 60.0 {
                Some(((left / 60.0).floor() as u32, Unit::Minutes))
            } else {
                (left >= 0.0).then(|| (left.floor() as u32, Unit::Seconds))
            }
        };
        let mut timer = BuffTimer::default();
        let mut nearing: Vec<(u32, f64)> = Vec::new();
        // 第一轮：终点在 30 秒，剩 10 秒左右提醒一次
        let mut t = 0.0;
        while t < 25.0 {
            timer.observe(t, true, reading_at(t, 30.0));
            if let Some(Reminder::Nearing(_)) = timer.reminder(t, 10, false) {
                nearing.push((1, t));
            }
            t += 0.5;
        }
        let old_cycle = timer.cycle;
        // 续上：t = 25 时重新放，终点到 55。要在 3 帧内看出来
        let mut frames = 0;
        while timer.cycle == old_cycle && frames < 10 {
            timer.observe(t, true, reading_at(t, 55.0));
            frames += 1;
            t += 0.5;
        }
        assert!(timer.cycle > old_cycle, "续上没识别出来");
        assert!(frames <= 3, "续上要在 3 帧内看出来，实际 {frames} 帧");
        // 新一轮：正常提醒一次
        while t < 56.0 {
            timer.observe(t, true, reading_at(t, 55.0));
            if let Some(Reminder::Nearing(_)) = timer.reminder(t, 10, false) {
                nearing.push((2, t));
            }
            t += 0.5;
        }
        let round1 = nearing.iter().filter(|&&(c, _)| c == 1).count();
        let round2: Vec<f64> = nearing.iter().filter(|&&(c, _)| c == 2).map(|&(_, t)| t).collect();
        assert_eq!(round1, 1, "第一轮要提醒一次：{nearing:?}");
        assert_eq!(round2.len(), 1, "续上后的新一轮要提醒且只一次：{nearing:?}");
        let left = 55.0 - round2[0];
        assert!((6.0..=11.5).contains(&left), "新一轮提醒时剩 {left} 秒");
    }

    /// 单次或连续两次读错（±1、±2、±5、±9、+10，和「9 读成 18」）：提醒只响一次，而且不会早于真实的 10 秒。
    /// 读错允许晚一点（实测最多晚约 4 秒，即剩 6 秒才响），但不能早、不能重复。
    #[test]
    fn one_or_two_misreads_never_duplicate_or_fire_early() {
        type Misread = (&'static str, fn(u32) -> u32);
        let misreads: [Misread; 8] = [
            ("-2", |n| n.saturating_sub(2)),
            ("-1", |n| n.saturating_sub(1)),
            ("+1", |n| n + 1),
            ("+2", |n| n + 2),
            ("-5", |n| n.saturating_sub(5)),
            ("+9", |n| n + 9),
            ("+10", |n| n + 10),
            ("9读成18", |n| if n >= 10 { n - 9 } else { n + 9 }),
        ];
        for &end in &[30.0f64, 30.3, 30.7, 30.9] {
            let start = end - 30.0;
            let mut frames = Vec::new();
            let mut t = start;
            while t < end {
                frames.push(t);
                t += 0.5;
            }
            for (label, bad) in misreads {
                for &f in &frames {
                    // 单帧、连续两帧读错：都不能重复念、不能早念（续上要连着 3 次读数印证，两次大误读攒不够）
                    let pairs: Vec<Vec<f64>> = vec![vec![f], vec![f, f + 0.5]];
                    for pair in pairs {
                        let fired = misread_run(end, &pair, bad);
                        assert_eq!(fired.len(), 1, "{label} 读错在 {pair:?}（终点 {end}）：{fired:?}");
                        let left = fired[0].1;
                        assert!(
                            (6.0..=11.5).contains(&left),
                            "{label} 读错在 {pair:?}（终点 {end}）：提醒时剩 {left} 秒，早了或晚太多：{fired:?}"
                        );
                    }
                }
            }
        }
    }
}
