//! 经验统计：**采样循环 + 会话状态机 + 每分钟/每小时速率**。
//!
//! ## 会话的口径
//!
//! * **开始**：不点就不统计（程序启动**不**自动开始），对着登录界面发呆不该产生数据。
//! * **暂停**：停止统计，而且这段墙钟时间**不计入任何窗口** —— 否则去吃个饭回来，
//!   「一小时经验」会被白拉低。窗口用的是**会话内的有效时间**，不是墙钟时间。
//! * **结束**：把这段交出去，由界面问一句「计入历史吗」。
//!
//! ## 三条「绝不含糊」的规矩
//!
//! 1. **读不到就不计**：抓不到画面、画面里没有那一行、或者和经验表对不上，
//!    一律当作「这一刻没有数据」，绝不拿上一次的值凑合，也绝不记 0。
//! 2. **停手要看得出来**：连续 90 秒没有新收益就显示「已停止」，
//!    而不是让数字慢慢衰减到 0 —— 那看起来像程序坏了。
//! 3. **跨级的账用累计经验算**：升级瞬间经验归零，直接相减是巨大的负数。
//!    见 [`crate::exp::table::advance`]。
//!
//! ## 时间口径（和同类工具刻意不同的一点）
//!
//! 可用读数的每一帧都会推进会话时钟，**哪怕这一帧的增量不可用** ——
//! 时钟不推进的话，被拒的那段时间会被从分母里悄悄抹掉，报出来的速率就偏高。
//! 暂停则是「精确扣除」：恢复时把下一帧标成**只立基准**，暂停期间你一直在打也无所谓。

use crate::db::{Database, ExpSampleRow, ExpSessionRow};
use crate::exp::map::{MapReader, MapSnapshot};
use crate::exp::reader::{CalibrationTest, ExpReader, ReadFailure, Sample};
use crate::exp::region::{NormRect, RegionProfile};
use crate::exp::table;
use parking_lot::Mutex;
use serde::Serialize;
use std::collections::VecDeque;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager};

/// 统计进行中的采样间隔。读的是底部那一条（一次后台截屏 + 切字，十几毫秒），
/// 1 秒一个点对「每分钟经验」这个口径刚好够密，也不会让游戏掉帧。
const ACTIVE_INTERVAL: Duration = Duration::from_secs(1);
/// 没在统计时的采样间隔：只为把「当前等级 · 百分比」显示出来。
const IDLE_INTERVAL: Duration = Duration::from_secs(3);
/// 两次成功读数之间超过这么久，就把这段增量**排除在速率之外**
/// （画面被挡了一段时间时，那段经验是什么时候涨的已经不知道了，但总数还是要算）。
const GAP_SECS: f64 = 6.0;
/// 连续多久没有新收益就显示「已停止」。
const IDLE_DISPLAY_SECS: f64 = 90.0;
/// 每分钟在历史里留一个点（不是每个采样点都留 —— 1 秒一个点一天就是八万条）。
const SAMPLE_BUCKET_SECS: f64 = 60.0;
/// 采样点保留天数。
const SAMPLE_RETENTION_DAYS: i64 = 30;
/// 两次采样之间超过这么久，说明采样循环被挂起过（系统休眠、进程被冻），
/// 这段时间既不该记成「看不到画面」也不该记成「读到了」—— 直接不算。
const MAX_TICK_WALL_SECS: f64 = 10.0;

/// 覆盖率到这个数以上才算「数据完整」/「可参考」。
const COVERAGE_GOOD: f64 = 0.95;
const COVERAGE_FAIR: f64 = 0.80;
/// 「数学拒绝帧」占采样帧数的比例上限（“可参考”那一档）。
const REJECT_RATIO_FAIR: f64 = 0.05;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    Idle,
    Running,
    Paused,
}

impl Phase {
    fn label(self) -> &'static str {
        match self {
            Phase::Idle => "未开始",
            Phase::Running => "统计中",
            Phase::Paused => "已暂停",
        }
    }
}

/// 会话期间攒下来的「数据可不可信」的原料，单位都是**墙钟秒**，
/// 而且只在「统计中」累加：暂停、未开始的时间不进来。
#[derive(Debug, Clone, Copy, Default)]
struct SessionStats {
    /// 能读到画面并认出数字的时间
    readable_secs: f64,
    /// 读不到的时间（最小化 / 不在角色里 / 认不出）
    unreadable_secs: f64,
    frames: u32,
    /// 读到了但没过校验的帧数（「数学拒绝帧」）
    rejected_frames: u32,
    /// 真正在涨经验的时间（挂机比例的反面）
    gaining_secs: f64,
}

/// 一段「阶段效率」：一次升级 → 下一次升级之间的速率。
#[derive(Debug, Clone, Serialize)]
pub struct StagePoint {
    /// 阶段开始时的等级（升级后的新等级）
    pub level: u32,
    pub started_at_secs: f64,
    pub ended_at_secs: f64,
    pub start_exp: u64,
    pub gained: u64,
    pub secs: f64,
    pub per_hour: Option<f64>,
    pub ongoing: bool,
}

/// 一段会话的数据可信度结论。
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SessionQuality {
    /// `2` = 数据完整 / `1` = 可参考 / `0` = 不建议用于比较
    pub grade: u8,
    pub label: String,
    pub reason: String,
    pub coverage: f64,
    /// 真正在涨经验的时间占比（挂机多不多 —— 和水不水是两回事）
    pub active_ratio: f64,
    pub rejected_frames: u32,
}

impl SessionStats {
    fn coverage(&self) -> f64 {
        let total = self.readable_secs + self.unreadable_secs;
        if total <= 0.0 {
            1.0
        } else {
            self.readable_secs / total
        }
    }

    fn active_ratio(&self, active_secs: f64) -> f64 {
        if active_secs <= 1.0 {
            0.0
        } else {
            (self.gaining_secs / active_secs).clamp(0.0, 1.0)
        }
    }

    fn grade(&self, active_secs: f64) -> SessionQuality {
        let coverage = self.coverage();
        let active_ratio = self.active_ratio(active_secs);

        if self.frames == 0 && self.readable_secs + self.unreadable_secs < 1.0 {
            return SessionQuality {
                grade: 1,
                label: "可参考".to_string(),
                reason: "这一段太短，还没采到几个点".to_string(),
                coverage,
                active_ratio,
                rejected_frames: 0,
            };
        }

        let mut reasons: Vec<String> = Vec::new();
        if coverage < COVERAGE_GOOD {
            reasons.push(format!(
                "有 {:.0}% 的时间读不到游戏画面，这一段的每小时收益会偏低",
                (1.0 - coverage) * 100.0
            ));
        }
        if self.rejected_frames > 0 {
            reasons.push(format!(
                "有 {} 帧的读数和经验表对不上，没有计入统计",
                self.rejected_frames
            ));
        }
        let reject_ratio = if self.frames == 0 {
            0.0
        } else {
            self.rejected_frames as f64 / self.frames as f64
        };

        let grade = if coverage >= COVERAGE_GOOD && self.rejected_frames == 0 {
            2
        } else if coverage >= COVERAGE_FAIR && reject_ratio <= REJECT_RATIO_FAIR {
            1
        } else {
            0
        };
        let (label, reason) = match grade {
            2 => (
                "数据完整",
                "整段都读到了游戏画面，每一帧都通过了经验表校验".to_string(),
            ),
            1 => ("可参考", reasons.join("；")),
            _ => ("不建议用于比较", reasons.join("；")),
        };
        SessionQuality {
            grade,
            label: label.to_string(),
            reason,
            coverage,
            active_ratio,
            rejected_frames: self.rejected_frames,
        }
    }
}

/// 窗口速率至少要有这么长的实测时间才给数 —— 太短的话是纯噪声。
const MIN_RATE_SPAN: f64 = 10.0;

/// 一个增量点：`active_secs` 是「会话内有效时间」（暂停期间不推进）。
///
/// 只存「到这一刻为止一共涨了多少」这一个数，速率的分子直接由两个点相减得到。
/// 不存单帧增量：单帧的和与管线相减本来等价，但管线能在窗口边界上**插值**，
/// 于是「最近 60 秒」是真正的 60 秒滑窗，不会因为采样点的位置抖动。
#[derive(Debug, Clone, Copy)]
struct GainPoint {
    active_secs: f64,
    /// 会话累计收益（单调不减）
    total: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct SessionStatus {
    pub started_at: String,
    pub started_unix: i64,
    pub ended_unix: i64,
    /// 会话内有效时间（秒）—— 暂停那段时间不算在里面
    pub active_secs: f64,
    pub gained_exp: u64,
    pub start_level: Option<u32>,
    pub current_level: Option<u32>,
    pub start_percent: Option<f64>,
    pub current_percent: Option<f64>,
    pub level_ups: u32,
    /// 这一段自己的平均速率（按有效时间算）
    pub per_hour: Option<f64>,
    pub quality: SessionQuality,
    /// 当前累计经验（从 1 级 0 经验算起）
    pub cumulative: Option<u64>,
    /// 按升级分段的效率（含进行中的一段）
    pub stages: Vec<StagePoint>,
    /// 预计还要多久升级（秒）
    pub eta_secs: Option<f64>,
    /// 距升级还差多少经验
    pub remaining_exp: Option<u64>,
}

/// 结束之后等用户确认「计入历史吗」的那一段。
#[derive(Debug, Clone, Serialize)]
pub struct ExpPending {
    pub active_secs: f64,
    pub gained_exp: u64,
    pub quality: SessionQuality,
    /// 结束瞬间冻结的整段结论（会话一结束 `session` 就空了，
    /// 没有这份快照，确认框和小结卡片就都画不出来）
    pub summary: SessionStatus,
}

#[derive(Debug, Clone, Serialize)]
pub struct ExpStatus {
    pub phase: String,
    pub phase_label: String,
    /// ok / no_window / minimized / capture_failed / blank_frame / no_field / contradiction
    pub read_state: String,
    /// 给界面的一句话（出错时是原因 + 怎么办）
    pub read_message: String,
    pub raw: Option<String>,
    pub exp: Option<u64>,
    pub percent: Option<f64>,
    pub level: Option<u32>,
    /// 当前累计经验（从 1 级 0 经验算起）
    pub cumulative: Option<u64>,
    pub last_read_at: Option<String>,
    /// 最近一分钟 / 最近一小时的经验（按实际测量区间折算）
    pub per_minute: Option<f64>,
    pub per_hour: Option<f64>,
    /// 上面两个数各自实际测了多久（秒）—— 不满一分钟/一小时时要显示出来
    pub minute_span_secs: f64,
    pub hour_span_secs: f64,
    /// 连续多久没有新收益（秒）
    pub idle_secs: f64,
    /// 停手了：界面要把数字置灰写着「已停止」，而不是看着它慢慢衰减到 0
    pub stopped: bool,
    /// 恢复后效率：一段「读不到画面」之后，从恢复的第一个可信帧起算的每小时
    pub recovery_per_hour: Option<f64>,
    pub recovery_span_secs: f64,
    pub session: Option<SessionStatus>,
    pub quality: Option<SessionQuality>,
    pub pending: Option<ExpPending>,
    pub window_title: Option<String>,
    /// 现在在哪个地图（认小地图的像素指纹，第一次遇到要用户填一次）
    pub map_name: Option<String>,
    /// 遇到一张没认过的地图：界面请他填一次名字，之后就一直自动
    pub map_unknown: bool,
    /// 这一次抓屏走的是哪条路：`window` = 后台截屏（不怕被挡住）／`screen` = 兜底
    pub capture_method: Option<String>,
    /// 认识的字符（诊断页显示）
    pub font_symbols: String,
    /// 当前用的校准框（比例坐标 + 来源），界面显示「已校准（自动/手动）」
    pub region: Option<RegionProfile>,
    /// 手动框连续读不到：提示「校准可能失效，重新框一次」
    pub region_lost: bool,
    /// 一次性提示（升级 / 换角色 / 掉经验）
    pub notice: Option<String>,
    /// 练级目标的进度；没设目标就是 `None`
    pub goal: Option<ExpGoal>,
}

/// 练级目标：到目标等级还差多少、按本段时速还要多久。
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ExpGoal {
    pub target_level: u32,
    /// 到「目标等级 0%」还差的经验；现在的等级还没读到时 `None`，已经到了是 0
    pub remaining_exp: Option<u64>,
    /// 按本段平均时速还要多久（秒）；没在统计 / 没涨经验 / 已经到了都是 `None`
    pub eta_secs: Option<f64>,
    pub reached: bool,
}

/// 目标等级的合法范围：至少 2 级，最高到经验表的最后一级。
pub const MIN_GOAL_LEVEL: u32 = 2;
pub const MAX_GOAL_LEVEL: u32 = table::EXP_TABLE.len() as u32;

/// 算练级目标的进度。全靠累计经验做减法 —— 中间要升几级都不用管。
fn goal_progress(target: u32, current_cum: Option<u64>, per_hour: Option<f64>) -> ExpGoal {
    let remaining = table::cumulative(target, 0)
        .zip(current_cum)
        .map(|(goal, now)| goal.saturating_sub(now));
    let reached = remaining == Some(0);
    let eta_secs = remaining
        .filter(|left| *left > 0)
        .zip(per_hour.filter(|rate| *rate > 0.0))
        .map(|(left, rate)| left as f64 * 3600.0 / rate);
    ExpGoal {
        target_level: target,
        remaining_exp: remaining,
        eta_secs,
        reached,
    }
}

struct Session {
    started_at: chrono::DateTime<chrono::Local>,
    started_unix: i64,
    /// 结束时刻（墙钟）。进行中是 `None`；`end()` 那一刻冻结下来。
    ended_unix: Option<i64>,
    active_secs: f64,
    gained_exp: u64,
    start_level: Option<u32>,
    start_percent: Option<f64>,
    current_level: Option<u32>,
    current_exp: Option<u64>,
    current_percent: Option<f64>,
    level_ups: u32,
    start_cum: Option<u64>,
    end_cum: Option<u64>,
    stats: SessionStats,
    points: Vec<ExpSampleRow>,
    last_bucket: i64,
}

impl Session {
    fn quality(&self) -> SessionQuality {
        self.stats.grade(self.active_secs)
    }

    fn status(&self) -> SessionStatus {
        let per_hour = if self.active_secs > 1.0 {
            Some(self.gained_exp as f64 * 3600.0 / self.active_secs)
        } else {
            None
        };
        // 阶段效率：把每分钟的采样点按「等级」切开（同一级里经验是单调涨的，
        // 升级那一次归零会形成新的分组），每段净经验 ÷ 净时长就是这一级的真实时速。
        let mut stages: Vec<StagePoint> = Vec::new();
        for point in &self.points {
            let level = point
                .level
                .unwrap_or_else(|| stages.last().map(|stage: &StagePoint| stage.level).unwrap_or(0));
            match stages.last_mut() {
                Some(stage) if stage.level == level => {
                    stage.ended_at_secs = point.offset_secs as f64;
                    stage.gained = point.exp.saturating_sub(stage.start_exp);
                }
                _ => stages.push(StagePoint {
                    level,
                    started_at_secs: point.offset_secs as f64,
                    ended_at_secs: point.offset_secs as f64,
                    start_exp: point.exp,
                    gained: 0,
                    secs: 0.0,
                    per_hour: None,
                    ongoing: false,
                }),
            }
        }
        let total = stages.len();
        for (index, stage) in stages.iter_mut().enumerate() {
            if index + 1 == total {
                stage.ended_at_secs = self.active_secs;
                stage.ongoing = true;
                // 进行中的分段不能停在「上一个采样点的差值」上：采样每 60 秒才
                // 进一位，这一段其实已经又走了最多 59 秒 —— gained 用**当前**的
                // （等级，本级经验）重算，不然正在练的这一段时速常年显示 0 或旧值。
                // 等级对不上（恢复基准的那几帧、等级还没被采样点跟上）就不动它：
                // 宁可少算，也不能把另一级的经验算进这一级。
                if self.current_level == Some(stage.level) {
                    if let Some(exp) = self.current_exp {
                        if let (Some(now), Some(start)) = (
                            table::cumulative(stage.level, exp),
                            table::cumulative(stage.level, stage.start_exp),
                        ) {
                            stage.gained = now.saturating_sub(start);
                        }
                    }
                }
            }
            stage.secs = (stage.ended_at_secs - stage.started_at_secs).max(0.0);
            stage.per_hour = if stage.secs >= 30.0 && stage.gained > 0 {
                Some(stage.gained as f64 * 3600.0 / stage.secs)
            } else {
                None
            };
        }

        let last_exp = self.current_exp.unwrap_or(0);
        // ETA：距升级的经验 ÷ 当前速率（停手时速率消失，ETA 也消失）
        let eta_secs = self
            .current_level
            .and_then(|level| table::remaining(level, last_exp))
            .zip(per_hour)
            .filter(|(_, rate)| *rate > 0.0)
            .map(|(left, rate)| left as f64 * 3600.0 / rate);

        SessionStatus {
            started_at: self.started_at.format("%m-%d %H:%M").to_string(),
            started_unix: self.started_unix,
            ended_unix: self
                .ended_unix
                .unwrap_or_else(|| chrono::Local::now().timestamp()),
            active_secs: self.active_secs,
            gained_exp: self.gained_exp,
            start_level: self.start_level,
            current_level: self.current_level,
            start_percent: self.start_percent,
            current_percent: self.current_percent,
            level_ups: self.level_ups,
            per_hour,
            quality: self.quality(),
            cumulative: self.end_cum,
            stages,
            eta_secs,
            remaining_exp: self
                .current_level
                .and_then(|level| table::remaining(level, last_exp)),
        }
    }
}

/// 恢复后效率的起点：一段「读不到画面」之后，从第一个可信帧开始重新计时的锚。
#[derive(Debug, Clone, Copy)]
struct RecoveryAnchor {
    active_secs: f64,
    cum: Option<u64>,
}

struct Inner {
    /// 最近一次认到的地图（界面显示用）。认地图那件事本身在 `ExpTracker::io` 里干
    place_snapshot: MapSnapshot,
    /// 游戏窗口标题（界面显示用）。
    ///
    /// 也在 tick 的第 3 段随状态机一起存进来：`status()` 每 2 秒被同步命令调一次，
    /// 为了一个几乎不变的标题去拿 `io` 锁，就可能排在几百毫秒的 ONNX 推理后面。
    window_title: Option<String>,
    phase: Phase,
    session: Option<Session>,
    pending: Option<Session>,
    points: VecDeque<GainPoint>,
    /// 一小时速率窗口的**左基准**：修剪丢掉的最后那个增量点。
    ///
    /// 采样队列只保留最近 3700 秒，挂机间隙一长，窗口边界（active-3600）会
    /// 落在队首**之前** —— 那时若仍从会话起点 (0, 0) 插值，等于把窗口的左端
    /// 拉回全场开头，一小时的窗口借到全场的总量，时速凭空暴涨（评审 P1#1）。
    /// 两种修法的取舍：在修剪处保留「被丢的最后一点」当左基准，插值永远
    /// 有一个不晚于边界的真锚点，长间隙后刚打出的第一笔大经验也不会被
    /// 抹成 0；另一条路（`at` 早于队首就返回队首的 total）只需一行补位，
    /// 但误差方向是「把窗口内已知增量也归零」，刚打出大经验时时速直接归零，
    /// 界面上看就是「明明在涨、速率却是 0」。取前者：多存一个点，
    /// 换两个方向都不说假话。会话开始 / 结束清空队列时一并清掉。
    window_origin: Option<GainPoint>,
    /// 上一次被接受的读数：那一刻的（时刻, 本级经验, 等级）。
    /// 等级是**账本自己推出来的**，不再指望每帧都从经验表重新定一次。
    baseline: Option<(Instant, u64, u32)>,
    /// 上一次被接受的读数（不受暂停影响），界面显示的也是它
    last_effective: Option<(u64, u32)>,
    /// 下一个可用读数**只用来立基准**，不计增量。
    /// 「开始统计」和「从暂停恢复」都置上：那两处的经验变化发生在计时之外
    /// （暂停期间你可能一直在打），记进这一段就是凭空多算。
    rebasing: bool,
    last_sample: Option<Sample>,
    failure: Option<ReadFailure>,
    last_read_at: Option<Instant>,
    last_gain_at: Option<Instant>,
    /// 上一轮采样的时刻，用来把时间分成「读得到」和「读不到」两堆
    last_tick: Option<Instant>,
    recovery_anchor: Option<RecoveryAnchor>,
    confirmed_streak: u32,
    notice: Option<String>,
    /// 练级目标等级（存在设置 `exp_goal_level` 里，启动时读进来）
    goal_level: Option<u32>,
    /// 抓屏方式变了就写一次日志（兜底路径值得知道）
    last_method: Option<&'static str>,
    /// 「第一次读到」「第一次读不到」各写一次日志。
    ///
    /// 没有这两条，出问题时日志里什么都没有（正常读数不打日志），
    /// 只能靠猜是抓不到、还是认不出、还是经验表对不上。
    logged_first_read: bool,
    logged_first_failure: bool,
    /// 当前用的校准框（从 `io.reader` 拷过来；`status()` 不许碰 io 锁）。
    region: Option<RegionProfile>,
    /// 手动框连续读不到（从 reader 拷过来）。
    region_lost: bool,
}

/// 抓屏与认字的家当：游戏窗口句柄、字形表、地图指纹缓存。
///
/// **为什么单开一把锁、不跟状态机共用 `inner`**：这里面有慢活 —— 地图名要过
/// ONNX 模型，第一次按宽度编译优化要几百毫秒到几秒、推理还要几十毫秒，抓屏
/// 本身也要十几毫秒。它们以前跑在 `inner` 大锁里面，于是每个采样周期都有几百
/// 毫秒里前端任何一条要拿 `inner` 的命令（内存、经验状态、开始/暂停）都得排队，
/// 界面表现就是「卡了一下」。
///
/// **锁序：`inner` 与 `io` 从不同时持有。** 谁先拿 io 谁就先把锁放干净
/// （`tick` 就是：io 干活 → 放掉 → inner 合并）。只要有一处拿着 `inner` 去等 `io`，
/// tick 每一轮都会把状态机锁堵住，这次要修的卡顿就又回来了；反过来同理。
struct Io {
    reader: ExpReader,
    /// 认「现在在哪个地图」（记小地图的像素指纹，见 `exp::map`）
    place: MapReader,
}

pub struct ExpTracker {
    inner: Mutex<Inner>,
    /// 抓屏 / 模型加载 / 推理都在这把锁里（见 `Io` 上的说明与锁序约定）
    io: Mutex<Io>,
    /// 内置字形表那串字符，**构造时算一次**（`Font::known()` 不是编译期常量，
    /// 它要把字表排序后拼起来，但内容整个进程里不变）。
    ///
    /// 存在这里而不是留在 `io` 里，是为了让 `status()` 少碰一把锁：
    /// 那个函数在 UI 线程上每 2 秒被调一次，碰 `io` 就可能撞上正在做
    /// ONNX 推理的那几百毫秒，界面跟着卡一下。
    font_symbols: String,
}

impl ExpTracker {
    pub fn new(reader: ExpReader) -> Self {
        // 先把字形表那串字符取出来再把 reader 搬进 io：它以后不跟着锁走
        let font_symbols = reader.font().known();
        Self {
            io: Mutex::new(Io {
                reader,
                place: MapReader::new(),
            }),
            inner: Mutex::new(Inner {
                place_snapshot: MapSnapshot::default(),
                window_title: None,
                phase: Phase::Idle,
                session: None,
                pending: None,
                points: VecDeque::new(),
                window_origin: None,
                baseline: None,
                last_effective: None,
                rebasing: false,
                last_sample: None,
                failure: None,
                last_read_at: None,
                last_gain_at: None,
                last_tick: None,
                recovery_anchor: None,
                confirmed_streak: 0,
                notice: None,
                goal_level: None,
                last_method: None,
                logged_first_read: false,
                logged_first_failure: false,
                region: None,
                region_lost: false,
            }),
            font_symbols,
        }
    }

    /// 采样循环。
    pub fn start_loop(self: &Arc<Self>, app: AppHandle, db: Arc<Database>) {
        let tracker = Arc::clone(self);
        tauri::async_runtime::spawn(async move {
            if let Err(err) = db.purge_exp_samples(SAMPLE_RETENTION_DAYS) {
                log::warn!("清理过期经验采样失败：{err}");
            }
            loop {
                let interval = {
                    let running = tracker.inner.lock().phase == Phase::Running;
                    if running {
                        ACTIVE_INTERVAL
                    } else {
                        IDLE_INTERVAL
                    }
                };
                tracker.tick(&app, &db);
                tokio::time::sleep(interval).await;
            }
        });
    }

    fn tick(&self, app: &AppHandle, db: &Database) {
        // 1) 短暂拿 inner：只算这一轮的时间片（wall 拿它分「读得到 / 读不到」）。
        //    打基准时这件事不能等 io —— 那边可能正在编译模型，一等就是几百毫秒，
        //    下一轮的 wall 会被这个延迟撑大。
        let now = Instant::now();
        let wall = {
            let mut guard = self.inner.lock();
            let wall = guard
                .last_tick
                .map(|stamp| (now - stamp).as_secs_f64().min(MAX_TICK_WALL_SECS))
                .unwrap_or(0.0);
            guard.last_tick = Some(now);
            wall
        };

        // 1.5) 人在不在游戏里。在登录 / 选角色界面，经验行和小地图都不存在，每 3 秒
        //      把整幅画面搜一遍、再对着登录画面跑一次地图 OCR 纯属空转（真机日志里
        //      停在登录界面时模型都被拉起来了）。满足任意一条就算「在」：
        //      * 正在统计（用户明确要读，全力读）；
        //      * 上一轮读成了（HUD 锚点那条主路径每轮都试，不受这个开关影响）；
        //      * 游戏连着一条认得出的线（`channel` 模块查 TCP 表，进角色几秒内就知道）。
        //      都不满足时并不是「不读」：只是整幅搜索降到十几秒一次、地图先不认。
        let in_game = {
            let guard = self.inner.lock();
            guard.phase == Phase::Running
                || (guard.failure.is_none() && guard.last_sample.is_some())
        } || on_a_channel(app);

        // 2) io 锁里干慢活：认地图（可能触发 ONNX 编译与推理）+ 抓屏 + 认字形。
        //    **这一段绝不拿着 inner** —— 以前它在大锁里面，每个采样周期都有几百
        //    毫秒任何要拿 inner 的命令（内存 / 经验状态 / 开始暂停）都得排队。
        let (snapshot, result, window_title, region, region_lost) = {
            let mut io = self.io.lock();
            let window = io.reader.window();
            // 先看看在哪个地图。优先用手动校准的地图名区域；没有就用 HUD 锚点
            // 推出的小地图区域（网页版的做法：任何分辨率都能自动定位）。
            // 指纹和 OCR 都用这个框（见 `exp::map::refresh` 的 override 参数）。
            let map_override = window
                .and_then(|hwnd| {
                    let (client_w, client_h) = crate::exp::capture::client_size(hwnd)?;
                    if let Some(profile) = io
                        .reader
                        .calibration_for(crate::exp::region::KIND_MAP_NAME)
                    {
                        return profile.rect.to_pixel(client_w, client_h);
                    }
                    io.reader.ensure_auto_map_rect(client_w, client_h)
                })
                .map(|rect| (rect.x, rect.y, rect.w, rect.h));
            let snapshot = match window {
                Some(hwnd) if in_game => {
                    io.place.refresh(hwnd, map_override, |key| db.get_exp_map(key))
                }
                _ => io.place.snapshot(),
            };
            io.reader.set_in_game(in_game);
            let result = io.reader.read();
            // 标题 / 校准框顺手在这里取走（它们只在 `io` 里），下一段随状态机存进 inner —
            // `status()` 就再也不用为这些几乎不变的东西去碰 `io` 锁
            (
                snapshot,
                result,
                io.reader.window_title(),
                io.reader.calibration(),
                io.reader.manual_lost(),
            )
        };

        // 2.5) OCR 的名字只差一个字时，拉回本地记过的名字（「随法密林」→「魔法密林」）。
        //      放在拿 inner 之前：查库是数据库的活，不该在状态机锁里做。
        let snapshot = match snapshot.name.clone() {
            Some(name) => match snap_map_name(db, &name) {
                Some(fixed) => {
                    log::info!("地图名近似修正：{name} → {fixed}");
                    MapSnapshot {
                        name: Some(fixed),
                        ..snapshot
                    }
                }
                None => snapshot,
            },
            None => snapshot,
        };

        // 3) 只在把这一轮的读数合并进状态机时持 inner（快照 + 窗口标题 + apply_read）
        {
            let mut guard = self.inner.lock();
            guard.window_title = window_title;
            guard.region = region;
            guard.region_lost = region_lost;
            if snapshot != guard.place_snapshot {
                if snapshot.unknown {
                    log::info!("遇到一张没认过的地图，等用户给它起个名字");
                }
                guard.place_snapshot = snapshot;
            }
            apply_read(&mut guard, result, now, wall);
        }

        let status = self.status();
        if let Err(err) = app.emit("exp-update", &status) {
            log::debug!("广播经验状态失败：{err}");
        }
    }

    pub fn status(&self) -> ExpStatus {
        // **这个函数体里不许出现 `self.io`。**
        // 它是同步命令 `get_exp_status` 的必经之路，前端每 2 秒在 UI 线程上调一次；
        // 而 `io` 那把锁里可能是几百毫秒的 ONNX 编译与推理 —— 碰一下，界面就卡一下。
        // 窗口标题在 tick 的第 3 段已经随状态机存进 inner，字形表是构造时算好的常量串，
        // 两样都不必再去要锁（`status_never_touches_the_io_lock` 扫源码钉住这条）。
        let guard = self.inner.lock();
        let session = guard.session.as_ref().map(|session| session.status());
        let (per_minute, minute_span) = guard
            .session
            .as_ref()
            .map(|session| {
                window_rate(
                    &guard.points,
                    guard.window_origin,
                    session.gained_exp,
                    session.active_secs,
                    60.0,
                )
            })
            .unwrap_or((None, 0.0));
        let (per_hour, hour_span) = guard
            .session
            .as_ref()
            .map(|session| {
                window_rate(
                    &guard.points,
                    guard.window_origin,
                    session.gained_exp,
                    session.active_secs,
                    3600.0,
                )
            })
            .unwrap_or((None, 0.0));

        let idle_secs = guard
            .last_gain_at
            .map(|stamp| stamp.elapsed().as_secs_f64())
            .unwrap_or(0.0);

        let (recovery_per_hour, recovery_span_secs) = match &guard.recovery_anchor {
            Some(anchor) => {
                let cum = guard
                    .last_effective
                    .and_then(|(exp, level)| table::cumulative(level, exp));
                recovery_rate(anchor, cum, current_active(&guard))
            }
            None => (None, 0.0),
        };

        let quality = guard.session.as_ref().map(|session| session.quality());
        let pending = guard.pending.as_ref().map(|session| ExpPending {
            active_secs: session.active_secs,
            gained_exp: session.gained_exp,
            quality: session.quality(),
            summary: session.status(),
        });
        let cumulative = guard
            .last_effective
            .and_then(|(exp, level)| table::cumulative(level, exp));

        let (read_state, read_message, raw) = match &guard.failure {
            Some(failure) => (
                failure.code().to_string(),
                failure.message(),
                failure.raw().map(|value| value.to_string()),
            ),
            None => (
                "ok".to_string(),
                match &guard.last_sample {
                    Some(sample) => format!("读到 {}", sample.raw),
                    None => "还没读到".to_string(),
                },
                None,
            ),
        };

        // 练级目标：现在的累计经验优先用账本的；还没开始过统计就用最近一次读数
        // （经验表定不出等级时拿等级框读到的顶上，和上面的 `level` 同一个口径）
        let goal = guard.goal_level.map(|target| {
            let current = cumulative.or_else(|| {
                guard.last_sample.as_ref().and_then(|sample| {
                    sample
                        .level
                        .or(sample.screen_level)
                        .and_then(|level| table::cumulative(level, sample.exp))
                })
            });
            goal_progress(target, current, session.as_ref().and_then(|s| s.per_hour))
        });

        ExpStatus {
            phase: match guard.phase {
                Phase::Idle => "idle".to_string(),
                Phase::Running => "running".to_string(),
                Phase::Paused => "paused".to_string(),
            },
            phase_label: guard.phase.label().to_string(),
            read_state,
            read_message,
            raw,
            exp: guard.last_sample.as_ref().map(|sample| sample.exp),
            percent: guard.last_sample.as_ref().map(|sample| sample.percent),
            level: guard
                .last_effective
                .map(|(_, level)| level)
                // 经验表定不出等级（刚升级、经验只有几百）时用等级框读到的顶上 ——
                // 没开始统计也要能显示「当前多少级」，和 `start_level` 同一个口径
                .or_else(|| {
                    guard
                        .last_sample
                        .as_ref()
                        .and_then(|sample| sample.level.or(sample.screen_level))
                }),
            cumulative,
            last_read_at: guard
                .last_read_at
                .map(|stamp| stamp.elapsed().as_secs().to_string()),
            per_minute,
            per_hour,
            minute_span_secs: minute_span,
            hour_span_secs: hour_span,
            idle_secs,
            stopped: guard.session.is_some() && idle_secs > IDLE_DISPLAY_SECS,
            recovery_per_hour,
            recovery_span_secs,
            session,
            quality,
            pending,
            window_title: guard.window_title.clone(),
            map_name: guard.place_snapshot.name.clone(),
            map_unknown: guard.place_snapshot.unknown,
            capture_method: guard
                .last_sample
                .as_ref()
                .map(|sample| match sample.method {
                    crate::exp::capture::Method::Window => "window".to_string(),
                    crate::exp::capture::Method::Screen => "screen".to_string(),
                }),
            font_symbols: self.font_symbols.clone(),
            region: guard.region.clone(),
            region_lost: guard.region_lost,
            notice: guard.notice.clone(),
            goal,
        }
    }

    /// 「测试读数」：拿一个还没保存的框立刻读一次（地图 / 等级 / 经验行都走它）。
    pub fn test_region(&self, kind: &str, rect: NormRect) -> CalibrationTest {
        self.io.lock().reader.test_region(kind, rect)
    }

    /// 「自动识别」：抓一整帧自动定位并学成 auto 档。
    pub fn auto_calibrate(&self) -> CalibrationTest {
        self.io.lock().reader.auto_calibrate()
    }

    /// 命令层保存了新的校准档后刷新 reader 的内存缓存（按来源区分覆盖规则）。
    pub fn set_region(&self, profile: RegionProfile) {
        let manual = profile.source == crate::exp::region::RegionSource::Manual;
        self.io.lock().reader.set_region(profile, manual);
    }

    /// 命令层清掉了某个 kind 的校准：回到整幅画面自动定位。
    pub fn clear_region(&self, kind: &str) {
        self.io.lock().reader.clear_region(kind);
    }

    /// 开始统计。
    ///
    /// **不点就不开始**（对着登录界面发呆不该产生数据）。门槛只有一条：
    /// 经验表能定出等级 —— 也就是「经验数字和百分比互相自洽」。
    pub fn start(&self) -> Result<(), String> {
        // 已经在「已暂停」时，「开始」等价于「继续」：命令名字叫错（前端发成
        // start_exp_session 而不是 resume_exp_session）不该让用户看到「点了没反应」。
        // 真机上就是这么翻车的：暂停之后点开始，后端回「已经在统计里了」，
        // 而界面把错误吞了 —— 现象和「按钮坏了」一模一样。
        // 锁必须先放掉再往下走（parking_lot 不可重入，见 toggle）。
        let phase = self.inner.lock().phase;
        if phase == Phase::Paused {
            return self.resume();
        }

        let mut guard = self.inner.lock();
        if guard.phase != Phase::Idle {
            return Err("已经在统计里了".to_string());
        }
        let Some(sample) = guard.last_sample.clone() else {
            return Err("还没读到游戏里的经验值，先确认游戏画面能读到再做开始".to_string());
        };
        // 等级优先用经验表定的；表定不出（刚升级、经验只有几百，多解）就用
        // **等级框**读出来的等级 —— HUD 锚点/手动校准都能给到它。
        // 以前这里只看经验表，于是「框选成功了还说定不出等级」，只能重启软件。
        let last_effective = guard.last_effective.map(|(_, level)| level);
        let Some(level) = start_level(&sample, last_effective) else {
            return Err(
                "还定不出等级（经验数字太小、经验表说不准，等级框也没读到），再等一两秒".to_string(),
            );
        };

        let now = Instant::now();
        let cum = table::cumulative(level, sample.exp);
        guard.session = Some(Session {
            started_at: chrono::Local::now(),
            started_unix: chrono::Local::now().timestamp(),
            ended_unix: None,
            active_secs: 0.0,
            gained_exp: 0,
            start_level: Some(level),
            start_percent: Some(sample.percent),
            current_level: Some(level),
            current_exp: Some(sample.exp),
            current_percent: Some(sample.percent),
            level_ups: 0,
            start_cum: cum,
            end_cum: cum,
            stats: SessionStats::default(),
            points: vec![ExpSampleRow {
                captured_unix: chrono::Local::now().timestamp(),
                offset_secs: 0,
                level: Some(level),
                exp: sample.exp,
                percent: sample.percent,
            }],
            last_bucket: 0,
        });
        guard.points.clear();
        guard.window_origin = None;
        guard.baseline = Some((now, sample.exp, level));
        guard.last_effective = Some((sample.exp, level));
        guard.rebasing = false;
        guard.last_gain_at = None;
        guard.recovery_anchor = None;
        guard.confirmed_streak = 0;
        guard.phase = Phase::Running;
        guard.notice = Some(format!(
            "开始统计：{} 级 {}({}%)。数据只在点「结束」并选择计入历史后才会落库。",
            level, sample.exp, sample.percent
        ));
        log::info!("经验统计开始：{} 级 {}({}%)", level, sample.exp, sample.percent);
        Ok(())
    }

    pub fn pause(&self) -> Result<(), String> {
        let mut guard = self.inner.lock();
        if guard.phase != Phase::Running {
            return Err("现在没在统计".to_string());
        }
        guard.phase = Phase::Paused;
        guard.notice = Some("已暂停：这段墙钟时间不计入任何窗口（一小时数不会被挂机拉低）".to_string());
        log::info!("经验统计已暂停");
        Ok(())
    }

    pub fn resume(&self) -> Result<(), String> {
        let mut guard = self.inner.lock();
        if guard.phase != Phase::Paused {
            return Err("现在没有暂停中的统计".to_string());
        }
        guard.phase = Phase::Running;
        // 暂停期间经验也会涨（用户可能一边挂着一边打），所以**换一个基准**：
        // 把暂停前到现在的这段增量整段丢掉，而不是算进恢复后的第一秒。
        // 等级接着用（不是清空重来）—— 清空之后要重新靠经验表定级，
        // 而升级那一瞬的 `0.00%` 是多解的，恢复之后可能空转好几帧。
        guard.rebasing = true;
        guard.notice = Some("已恢复：暂停那一段时间没有计入".to_string());
        log::info!("经验统计已恢复");
        Ok(())
    }

    /// 结束一段：交出去给界面确认，**先不落库**。
    pub fn end(&self) -> Result<SessionStatus, String> {
        let mut guard = self.inner.lock();
        let Some(mut session) = guard.session.take() else {
            return Err("现在没有正在统计的一段".to_string());
        };
        session.ended_unix = Some(chrono::Local::now().timestamp());
        let status = session.status();
        guard.phase = Phase::Idle;
        guard.points.clear();
        guard.window_origin = None;
        guard.baseline = None;
        guard.last_gain_at = None;
        guard.recovery_anchor = None;
        guard.confirmed_streak = 0;
        let quality = status.quality.clone();
        guard.pending = Some(session);
        guard.notice = Some(format!(
            "这一段已结束（{}）：等你在上面确认是否计入历史",
            quality.label
        ));
        log::info!(
            "经验统计结束：{} 秒有效时间，获得 {} 经验，质量 {}（覆盖 {:.0}%）",
            status.active_secs.round(),
            status.gained_exp,
            quality.label,
            quality.coverage * 100.0
        );
        Ok(status)
    }

    /// 前一个问题的答案：计不计入历史。
    pub fn resolve_pending(
        &self,
        save: bool,
        map_name: &str,
        db: &Database,
    ) -> Result<String, String> {
        // 第一段：把待确认的那一段取走（**短暂持锁**，其余工作都放到锁外）。
        let session = {
            let mut guard = self.inner.lock();
            let Some(session) = guard.pending.take() else {
                return Err("没有等待确认的一段".to_string());
            };
            if !save {
                guard.notice = Some("这一段没有计入历史，已经丢掉".to_string());
                log::info!("经验会话被用户丢弃（{} 秒）", session.active_secs.round());
                return Ok("这一段没有计入历史".to_string());
            }
            session
        };
        // 地图名是用户手填的：去掉首尾空白、截到 24 个字
        let map_name: String = map_name.trim().chars().take(24).collect();

        // 第二段：**顺手把地图名和当前这张图的指纹记在一起。**
        // 用户填过一次之后，以后每次来这张图都会自动填好 —— 不需要他再做任何事，
        // 也不需要界面上多一个「记住这张图」的按钮。
        //
        // 这一段必须在 inner 锁**外面**做：指纹缓存和截图都在 io 那把锁里，
        // 而 io 可能正在做 ONNX 编译（几百毫秒）—— 拿着 inner 等它，
        // 界面就又卡住了（见 `Io` 上的锁序约定：两把锁从不同时持有）。
        if !map_name.is_empty() {
            let learned = self.io.lock().place.learn(&map_name);
            if let Some((fingerprint, name)) = learned {
                if let Err(err) = db.set_exp_map(&fingerprint, &name) {
                    log::warn!("记住地图名失败：{err}");
                } else {
                    let snapshot = self.io.lock().place.snapshot();
                    self.inner.lock().place_snapshot = snapshot;
                }
            }
        }

        // 结束时刻用 `end()` 冻结的墙钟，**不能**用 `started_unix + active_secs` 反算：
        // 那个式子把「暂停 2 小时」从历史里抹掉 —— 11:00 开始、中途暂停吃饭、
        // 14:30 才点结束的一段，落库成「11:30 就结束了」，历史时间轴就交叠错乱。
        // 正常路径 `end()` 一定已经写上了；`unwrap_or_else` 兜的是直接构造
        // `pending` 的非常规路径（老数据、测试），那时拿当前时间是最不坏的猜。
        let ended = session
            .ended_unix
            .unwrap_or_else(|| chrono::Local::now().timestamp());
        let quality = session.quality();
        let row = ExpSessionRow {
            started_unix: session.started_unix,
            ended_unix: ended,
            active_secs: session.active_secs,
            start_level: session.start_level,
            end_level: session.current_level,
            start_exp: session.points.first().map(|point| point.exp).unwrap_or(0),
            // 终点经验用**实时值**而不是采样队列的最后一个点：队列每 60 秒才进一位，
            // 5 分 50 秒结束的那一段，旧写法落库的终点是 5 分 00 秒的读数，
            // 和同一行里实时的 gained_exp 对不上账。起点没有对应的八字段实时值，
            // 仍从第一个采样点取（统计开始时本来就有个起点点，误差为零）。
            end_exp: session
                .current_exp
                .or_else(|| session.points.last().map(|point| point.exp))
                .unwrap_or(0),
            start_percent: session.start_percent,
            end_percent: session.current_percent,
            gained_exp: session.gained_exp,
            start_cum: session.start_cum,
            end_cum: session.end_cum,
            quality: quality.grade,
            quality_reason: quality.reason.clone(),
            coverage: quality.coverage,
            idle_ratio: 1.0 - quality.active_ratio,
            rejected_frames: quality.rejected_frames,
            map_name,
        };
        let id = db
            .insert_exp_session(&row, &session.points)
            .map_err(|err| format!("写入历史失败：{err}"))?;
        let _ = db.purge_exp_samples(SAMPLE_RETENTION_DAYS);
        let notice = format!(
            "已计入历史（第 {} 段）：{} 秒有效时间，获得 {} 经验 · {}",
            id,
            session.active_secs.round(),
            session.gained_exp,
            quality.label
        );
        self.inner.lock().notice = Some(notice.clone());
        Ok(notice)
    }

    /// 快捷键 / 按钮统一走它：未开始→开始，统计中→暂停，暂停中→恢复。
    ///
    /// **先把锁放掉再往下走。** 写成 `match self.inner.lock().phase { Phase::Idle => self.start(), .. }`
    /// 是一个会**把整个程序挂死**的写法：那个临时 guard 活到整个 `match` 语句结束，
    /// 而 `start()` / `pause()` / `resume()` 里都要再 `lock()` 一次 ——
    /// `parking_lot::Mutex` 不可重入，于是自己等自己，界面永久转圈。
    /// （真机上就是这么挂的：`Ctrl+Alt+P` 一按，进程就再也不响应了。
    /// `toggling_never_deadlocks` 钉住这一条。）
    pub fn toggle(&self) -> Result<(), String> {
        let phase = self.inner.lock().phase;
        match phase {
            Phase::Idle => self.start(),
            Phase::Running => self.pause(),
            Phase::Paused => self.resume(),
        }
    }

    /// 诊断页：我们眼里看到的底部条。
    ///
    /// **这里拿 `io` 锁是故意的，不改。** `status()` 不能碰 `io`（它每 2 秒被
    /// 同步命令调一次，见那边的说明），但 `preview()` 不同：它是**用户主动点
    /// 诊断按钮才跑一次**的，而且它要的就是「刚才那一帧」—— 抓屏本来就是被诊断
    /// 的对象，为了少拿一次锁而把抓屏拆出去，诊断页就不再是同一次抓取的结果了。
    /// 调用方 `get_exp_preview` 是 `async` 命令，跑在 tokio 工作线程上（不在 UI
    /// 线程），几百毫秒的 ONNX 编译最多占住一个工作线程，界面照常响应。
    pub fn preview(&self) -> Result<Vec<String>, String> {
        // 只碰 io 一把锁（抓屏也在这里面），不碰 inner
        self.io.lock().reader.preview()
    }


    /// 设 / 清练级目标（只改内存里的；落库在命令层）。
    pub fn set_goal(&self, level: Option<u32>) {
        self.inner.lock().goal_level = level;
    }

    /// 已经认过的地图名（界面上的快捷按钮）。
    pub fn known_places(&self, db: &Database) -> Vec<String> {
        db.list_exp_maps().unwrap_or_default()
    }

    pub fn clear_notice(&self) {
        self.inner.lock().notice = None;
    }
}

/// 游戏是不是连着一条认得出的线（= 人在角色里）。
///
/// `AppState` 还没注册好（启动头几秒）或者拿不到时按「在」处理：宁可多干活，
/// 也不能因为判断不了就把读数降级。
fn on_a_channel(app: &AppHandle) -> bool {
    app.try_state::<crate::commands::AppState>()
        .map(|state| {
            let channel = state.channel_watcher.state();
            channel.channel.is_some() && channel.source != "stale"
        })
        .unwrap_or(true)
}

/// 把一次读到的结果并进状态机。抽出来是为了能单独写测试。
fn apply_read(guard: &mut Inner, result: Result<Sample, ReadFailure>, now: Instant, wall: f64) {
    let running = guard.phase == Phase::Running;

    let sample = match result {
        Ok(sample) => sample,
        Err(failure) => {
            if !guard.logged_first_failure {
                guard.logged_first_failure = true;
                log::info!(
                    "经验统计：第一次读数失败 —— {}（{}）",
                    failure.message(),
                    failure.code()
                );
            }
            if running {
                if let Some(session) = guard.session.as_mut() {
                    session.stats.unreadable_secs += wall;
                    session.stats.frames += 1;
                    // 「读到了但没过校验」才是真正值得记一笔的（“数学拒绝帧”）。
                    // 最小化、不在角色里那些是环境问题，覆盖率会如实掉下来，不单独计数。
                    if matches!(
                        failure,
                        ReadFailure::Contradiction { .. }
                    ) {
                        session.stats.rejected_frames += 1;
                    }
                }
            }
            log::debug!("经验读数没有拿到：{}", failure.message());
            guard.failure = Some(failure);
            return;
        }
    };

    // 「上一帧是不是失败的」要在清空 failure 之前判断（恢复后效率的锚点靠它）
    let was_failing = guard.failure.is_some();
    guard.failure = None;
    guard.last_read_at = Some(now);
    if let Some(previous) = guard.last_method {
        let current = match sample.method {
            crate::exp::capture::Method::Window => "window",
            crate::exp::capture::Method::Screen => "screen",
        };
        if previous != current {
            log::info!("经验抓屏方式变为 {current}");
        }
    }
    guard.last_method = Some(match sample.method {
        crate::exp::capture::Method::Window => "window",
        crate::exp::capture::Method::Screen => "screen",
    });
    if !guard.logged_first_read {
        guard.logged_first_read = true;
        log::info!(
            "经验统计：第一次读到 {} → 经验 {} / {}%（表定 {:?} · 等级框 {:?} · 抓法 {:?}）",
            sample.raw,
            sample.exp,
            sample.percent,
            sample.level,
            sample.screen_level,
            sample.method
        );
    }
    guard.last_sample = Some(sample.clone());

    if !running {
        // 没在统计：只更新「当前值」显示，不推进任何计时。
        // 「未开始」时等级跟着读数走 —— 不然换了角色，界面上还挂着上一段统计
        // 留下的那个角色的等级（暂停中不动：那是账本的基准）。
        if guard.phase == Phase::Idle {
            if let Some(level) = sample.level.or(sample.screen_level) {
                guard.last_effective = Some((sample.exp, level));
            }
        }
        return;
    }

    let previous = guard.baseline;
    let Some((prev_exp, prev_level)) = previous.map(|(_, exp, level)| (exp, level)) else {
        // 还没有基准：经验表能定出等级就能立起来（定不出就先等一帧好的）；
        // 校准过等级区域的话，屏幕上的等级可以顶上（升级瞬间的 `0(0.00%)` 就是这种帧）。
        let Some(level) = sample.level.or(sample.screen_level) else {
            if let Some(session) = guard.session.as_mut() {
                session.stats.frames += 1;
                session.stats.unreadable_secs += wall;
            }
            return;
        };
        guard.baseline = Some((now, sample.exp, level));
        guard.last_effective = Some((sample.exp, level));
        if let Some(session) = guard.session.as_mut() {
            session.stats.readable_secs += wall;
            session.stats.frames += 1;
            session.current_level = Some(level);
            session.current_exp = Some(sample.exp);
            session.current_percent = Some(sample.percent);
            session.end_cum = table::cumulative(level, sample.exp);
        }
        guard.notice = Some(format!(
            "已认到 {} 级 {}({}%)，从这里开始记",
            level, sample.exp, sample.percent
        ));
        return;
    };

    // ── 重立基准的帧 ────────────────────────────────────────────────
    // 「开始统计」和「从暂停恢复」都会置上 `rebasing`：那两处的经验变化发生在
    // 我们的计时之外，记进这一段就是凭空多算。所以这一帧只把基准挪到当下。
    if guard.rebasing {
        guard.rebasing = false;
        // 等级优先用读数自己定的那个（经验表定不出时用校准过的等级区域），
        // 都定不出来就顺着上一帧往上找一个**装得下**的
        // （否则会留下「本级经验已经超过本级所需」的假基准，下一帧凭空多算一整级）
        let level = sample
            .level
            .or(sample.screen_level)
            .unwrap_or_else(|| table::level_holding(prev_level, sample.exp).unwrap_or(prev_level));
        guard.baseline = Some((now, sample.exp, level));
        guard.last_effective = Some((sample.exp, level));
        if let Some(session) = guard.session.as_mut() {
            session.stats.readable_secs += wall;
            session.stats.frames += 1;
            session.current_level = Some(level);
            session.current_exp = Some(sample.exp);
            session.end_cum = table::cumulative(level, sample.exp);
        }
        log::debug!("经验统计：重立基准（{} 级 {}），这一帧不计增量", level, sample.exp);
        return;
    }

    let elapsed = previous
        .map(|(stamp, _, _)| (now - stamp).as_secs_f64())
        .unwrap_or(0.0);
    let expected = expected_gain(&guard, elapsed);
    let verdict = table::advance(
        prev_level,
        prev_exp,
        sample.exp,
        sample.percent,
        expected,
    );

    // 公共部分：**每一帧可用读数都推进时钟**（哪怕这一帧的增量不可用）。
    // 时钟不推进的话，被拒的那段时间会被从分母里悄悄抹掉，报出来的速率就偏高。
    //
    // 注意：这里推进会话时钟必须累加「本次 tick 的真实增量 wall」，绝不能用「距基准的跨度 elapsed」。
    // 遇到 `Advance::Unclear` 坏帧时，状态机有意不更新基准点（留给下一帧好值继续对账）；
    // 若用 elapsed 累加，连续 N 帧坏帧会导致累加量呈等差数列二次方膨胀（N*(N+1)/2 · Δt），
    // 真实过去 10 秒会被算成 55 秒，分母暴增导致时速断崖式跌零。
    if let Some(session) = guard.session.as_mut() {
        session.stats.readable_secs += wall;
        session.stats.frames += 1;
        session.active_secs += wall;
        session.current_percent = Some(sample.percent);
    }

    match verdict {
        table::Advance::Gained {
            level,
            exp,
            gained,
            level_ups,
        } => {
            if let Some(session) = guard.session.as_mut() {
                session.current_level = Some(level);
                session.current_exp = Some(exp);
                session.end_cum = table::cumulative(level, exp);
                session.gained_exp += gained;
                session.level_ups += level_ups;
                // 升级帧必须往采样序列里**补交界点**（每跨过一级补一对）。
                // stages 按 60 秒一个的采样点切段，升级发生在分钟中间时，
                // 不补点的话两头的经验都找不到归属：上一级的尾部
                // （上个整分钟点 → 升级那一刻）记不进旧分段，新级的头部
                // （升级那刻 → 下个整分钟点）又从零起步。
                // 被跨过的那级记到**满值**（那一刻它确实是被填满的，
                // 百分比 100% 是当时的真实显示），新级从实际读数起步 ——
                // 两段加起来正好拼出这一帧的总增量，不多不少。
                // 挂在 level_ups 上而不是每帧都插：普通帧一个采样点就够。
                if level_ups > 0 {
                    let boundary_secs = session.active_secs.round() as i64;
                    let boundary_unix = chrono::Local::now().timestamp();
                    for crossed in prev_level..level {
                        if let Some(full) = table::requirement(crossed) {
                            session.points.push(ExpSampleRow {
                                captured_unix: boundary_unix,
                                offset_secs: boundary_secs,
                                level: Some(crossed),
                                exp: full,
                                percent: 100.0,
                            });
                        }
                    }
                    session.points.push(ExpSampleRow {
                        captured_unix: boundary_unix,
                        offset_secs: boundary_secs,
                        level: Some(level),
                        exp,
                        percent: sample.percent,
                    });
                }
                if gained > 0 {
                    // 有效打怪时间统计：这里使用 elapsed（距上一基准的跨度）是有意的。
                    // 若此前经历过几帧坏帧，在终于涨经验的这一刻，这笔收益是在整个 elapsed 期间打出来的，
                    // 应当把这段时间计入活跃打怪时长（受 GAP_SECS 上限 6 秒钳制，防大断档）；
                    // 这样派生出的打怪活跃度 `active_ratio = gaining_secs / active_secs` 在遭遇偶发坏帧后
                    // 仍能维持合理比率（不会被误降为"挂机"），且自身带有 clamp(0.0, 1.0) 兜底。
                    session.stats.gaining_secs += gained_cover(elapsed);
                    // 存的是「到这一刻为止一共涨了多少」——速率的分子由管线相减得到
                    guard.points.push_back(GainPoint {
                        active_secs: session.active_secs,
                        total: session.gained_exp,
                    });
                    guard.last_gain_at = Some(now);
                }
            }
            guard.baseline = Some((now, exp, level));
            guard.last_effective = Some((exp, level));
            if level_ups > 0 {
                guard.notice = Some(format!(
                    "升级了：{} 级 → {} 级（这一帧 +{}）",
                    level - level_ups,
                    level,
                    gained
                ));
                log::info!("经验统计：升级到 {level} 级（一帧跨 {level_ups} 级，+{gained}）");
            }
        }
        table::Advance::Loss { level, exp, lost } => {
            // 死亡掉经验：如实报出来，**不算成负收益**（那会把这一段拖下水），
            // 也不算成换角色（那会白丢一段）。基准挪到新值上继续统计。
            if let Some(session) = guard.session.as_mut() {
                session.current_level = Some(level);
                session.current_exp = Some(exp);
                session.end_cum = table::cumulative(level, exp);
            }
            guard.baseline = Some((now, exp, level));
            guard.last_effective = Some((exp, level));
            guard.notice = Some(format!("这一级里经验掉了 {lost}（死亡？），没有记成负收益"));
            log::info!("经验统计：本级经验下降 {lost}（按掉落处理，不计负收益）");
        }
        table::Advance::Shifted { too_big } => {
            // 这一帧自己自洽，只是和上一帧接不上。两种情况都**不计增量、
            // 把基准挪到当下** —— 不挪的话下一帧还是接不上，统计就永久停摆了。
            // 等级和重立基准同一个口径（经验表 → 等级框 → 顺着上一帧往上找）：
            // 换到一个经验很少的小号时经验表定不出等级，只往上找会把旧角色的
            // 等级留在基准里，之后每一帧都接不上、每一帧都判「换角色」。
            let level = sample
                .level
                .or(sample.screen_level)
                .unwrap_or_else(|| table::level_holding(prev_level, sample.exp).unwrap_or(prev_level));
            guard.baseline = Some((now, sample.exp, level));
            guard.last_effective = Some((sample.exp, level));
            if let Some(session) = guard.session.as_mut() {
                session.current_level = Some(level);
                session.current_exp = Some(sample.exp);
                session.end_cum = table::cumulative(level, sample.exp);
                if too_big {
                    session.stats.rejected_frames += 1;
                }
            }
            guard.notice = Some(if too_big {
                "这一帧的增量大得不像真的，没有计入（认错了一位数字？）".to_string()
            } else {
                "看起来换角色了：跨过的那一瞬没有计入统计".to_string()
            });
        }
        table::Advance::Unclear => {
            // **不动基准** —— 下一帧接着按上一帧的好值算，不会因为一帧坏数据丢掉一段。
            if let Some(session) = guard.session.as_mut() {
                session.stats.rejected_frames += 1;
            }
            log::debug!(
                "经验统计：这一帧和上一帧接不上（{prev_level} 级 {prev_exp} → {} / {}），不记数",
                sample.exp,
                sample.percent
            );
        }
    }

    // 每分钟留一个历史点
    if let Some(session) = guard.session.as_mut() {
        let bucket = (session.active_secs / SAMPLE_BUCKET_SECS).floor() as i64;
        if bucket > session.last_bucket {
            session.last_bucket = bucket;
            session.points.push(ExpSampleRow {
                captured_unix: chrono::Local::now().timestamp(),
                offset_secs: session.active_secs.round() as i64,
                level: session.current_level,
                exp: session.current_exp.unwrap_or(0),
                percent: session.current_percent.unwrap_or(0.0),
            });
        }
    }

    // 恢复后效率：失败段之后的第一个可信帧立一个锚，之后每个可信帧把锚的累计
    // 推进到当前 —— 算出来的速率只覆盖「恢复后的连续可信区间」。
    if was_failing {
        guard.confirmed_streak = 0;
        guard.recovery_anchor = Some(RecoveryAnchor {
            active_secs: current_active(guard),
            cum: guard
                .last_effective
                .and_then(|(exp, level)| table::cumulative(level, exp)),
        });
    } else {
        guard.confirmed_streak += 1;
        if guard.confirmed_streak >= 30 && guard.recovery_anchor.is_some() {
            guard.recovery_anchor = None;
        }
    }

    // 一小时窗口用不上那么多点。
    //
    // 被丢的点不能直接消失：速率窗口的边界比修剪线（3700）晚 100 秒，
    // 挂机间隙一长，边界会落在队首之前 —— 此时插值若退回会话起点 (0,0)，
    // 一小时的窗口就借到全场的总量（评审 P1#1）。把被丢的最后一点留在
    // `window_origin` 里当左基准，窗口边界永远插得出来。
    while let Some(front) = guard.points.front().copied() {
        if front.active_secs >= current_active(&guard) - 3700.0 {
            break;
        }
        guard.window_origin = Some(front);
        guard.points.pop_front();
    }
}

/// 「开始统计」时怎么定等级：经验表 → **等级框**（HUD 锚点/手动校准都能给到）→
/// 上一次的有效等级。
///
/// 经验表在**经验值很小**时会多解：刚升级那几秒（读出 `700(0.07%)`），
/// 几十个等级的累计经验都装得下这 700 —— 这不是读数错误，但表定不出等级。
/// 以前这里只看经验表，于是「等级框都框选成功了还说定不出等级」，只能重启软件。
fn start_level(sample: &Sample, last_effective: Option<u32>) -> Option<u32> {
    sample.level.or(sample.screen_level).or(last_effective)
}

/// 地图名只差一个字时，拉回本地记过的名字：PP-OCR 偶尔把「魔法密林」读成
/// 「随法密林」，而这张图我们很可能已经记过正确的名字（历史会话/手填）。
///
/// 只接受**等长、差一个字、且唯一候选**的近似 —— 宁可留着近似的 OCR 结果，
/// 也不要把它拉到另一张地图的名字上。返回 `None` 表示「不用改」。
fn snap_map_name(db: &Database, name: &str) -> Option<String> {
    let known = db.list_exp_maps().ok()?;
    if known.iter().any(|candidate| candidate == name) {
        return None;
    }
    let mut hit: Option<&String> = None;
    for candidate in &known {
        if candidate.chars().count() != name.chars().count() {
            continue;
        }
        if !is_one_edit_apart(candidate, name) {
            continue;
        }
        if hit.is_some() {
            return None; // 多个候选，不猜
        }
        hit = Some(candidate);
    }
    hit.cloned()
}

/// 等长的两个串是不是只差一个字。
fn is_one_edit_apart(a: &str, b: &str) -> bool {
    let mut diff = 0;
    for (x, y) in a.chars().zip(b.chars()) {
        if x != y {
            diff += 1;
            if diff > 1 {
                return false;
            }
        }
    }
    diff == 1
}

/// 按最近的速率估一个「这一帧大概该涨多少」。
///
/// **只用来在多个合法解释里挑一个**（见 [`table::advance`]），从不用来否掉一帧数据。
/// 速率还没站稳（一分钟窗口里不足 20 秒）时给 `None`，让账本走保守分支。
fn expected_gain(guard: &Inner, elapsed: f64) -> Option<f64> {
    if elapsed <= 0.0 {
        return None;
    }
    let session = guard.session.as_ref()?;
    let (rate, span) = window_rate(
        &guard.points,
        guard.window_origin,
        session.gained_exp,
        session.active_secs,
        60.0,
    );
    let rate = rate.filter(|_| span >= 20.0)?;
    Some(rate / 60.0 * elapsed)
}

/// 恢复后效率：从锚点（恢复后的第一个可信帧）到现在的净经验 ÷ 净时长。
fn recovery_rate(anchor: &RecoveryAnchor, cum: Option<u64>, active_now: f64) -> (Option<f64>, f64) {
    let span = (active_now - anchor.active_secs).max(0.0);
    match (anchor.cum, cum) {
        (Some(before), Some(after)) if after > before && span >= 10.0 => {
            (Some((after - before) as f64 * 3600.0 / span), span)
        }
        _ => (None, span),
    }
}

/// 这一帧增量**实际覆盖**了多久（换算速率和「挂了多久机」都按它算）。
fn gained_cover(elapsed: f64) -> f64 {
    elapsed.clamp(0.0, GAP_SECS)
}

fn current_active(guard: &Inner) -> f64 {
    guard
        .session
        .as_ref()
        .map(|session| session.active_secs)
        .unwrap_or(0.0)
}

/// 最近 `window_secs` 有效时间里的经验，折算成「每 `window_secs`」的量。
///
/// 返回（折算后的数量, 实际测了多久）。实测量不满一个窗口时如实返回，
/// 由界面标注「实测 X」—— 不能把 12 分钟的数当一小时的平均报出去。
///
/// # 分母必须是「观察了多久」，不是「涨经验的那几帧加起来多久」
///
/// 这里踩过一个坑，而且是用户先发现的：早先的实现把分母写成「有增量的那些帧的
/// 时长之和」。打怪是**一阵一阵**的 —— 一分钟里可能只有 26 秒在涨经验，
/// 另外 34 秒在跑图、捡东西、站着不动。于是那 26 秒的收益被当成一整分钟的量报出去。
/// 真机上的表现：悬浮窗显示「每小时 103,298」，而同一时刻的小结卡片显示
/// 「4.2 万 / 时」—— 差了 2.5 倍。卡片用的是「净经验 ÷ 会话有效时间」，那个是对的。
///
/// 现在的口径和卡片完全一致：**窗口内净涨了多少 ÷ 窗口本身有多长**。
/// 窗口长度取「会话有效时间」与 `window_secs` 的较小值：刚开统计时窗口还没长满，
/// 就按实测的那一段折算。
fn window_rate(
    points: &VecDeque<GainPoint>,
    // `origin`：队列左侧被修剪掉的最后一个点（修剪线之外的最后已知总量）。
    // 没修剪过就是 `None`，插值照旧从会话起点 (0, 0) 起算。
    origin: Option<GainPoint>,
    now_total: u64,
    active_now: f64,
    window_secs: f64,
) -> (Option<f64>, f64) {
    let span = window_secs.min(active_now);
    if span < MIN_RATE_SPAN {
        return (None, span.max(0.0));
    }
    let cutoff = active_now - window_secs;
    let base = if cutoff <= 0.0 {
        0
    } else {
        total_at(origin, points, cutoff)
    };
    let gained = now_total.saturating_sub(base);
    (Some(gained as f64 * window_secs / span), span)
}

/// 会话进行到 `at` 秒时一共涨了多少（采样点之间线性插值）。
///
/// 插值基准由 `origin` 提供：会话没被修剪过时传 `None`，从 `(0 秒, 0 点经验)`
/// 起算（第一个采样点之前那一段确实一格都没涨）；修剪过之后传被丢的最后
/// 一个点 —— 队首已经晚于边界时，只有它能把窗口的左端钉在真实历史上。
fn total_at(origin: Option<GainPoint>, points: &VecDeque<GainPoint>, at: f64) -> u64 {
    let mut before = origin
        .map(|point| (point.active_secs, point.total))
        .unwrap_or((0.0f64, 0u64));
    for point in points {
        if point.active_secs <= at {
            before = (point.active_secs, point.total);
            continue;
        }
        let width = point.active_secs - before.0;
        if width <= 0.0 {
            return point.total;
        }
        let ratio = ((at - before.0) / width).clamp(0.0, 1.0);
        let value = before.1 as f64 + (point.total - before.1) as f64 * ratio;
        return (value.round() as u64).clamp(before.1, point.total);
    }
    before.1
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::exp::capture::Method;
    use crate::exp::font::Font;

    /// 一个增量点：会话进行到 `active_secs` 秒时，一共涨到 `total`。
    fn point(active_secs: f64, total: u64) -> GainPoint {
        GainPoint { active_secs, total }
    }

    /// **用户报的那条 bug 的回归测试**：打怪是一阵一阵的，速率不能只拿
    /// 「正在涨经验的那几秒」当分母。
    ///
    /// 真机上的原始数据：那一分钟里只有 26 秒在涨经验（共 706 点），另外 34 秒
    /// 在跑图捡东西。旧实现报「每分钟 1,630」（= 706 × 60 ÷ 26），
    /// 而同一条会话的平均其实是 706 —— 虚高 2.3 倍，用户看到悬浮窗写「每小时 103,298」
    /// 而小结卡片写「4.2 万」，差了 2.5 倍。
    #[test]
    fn a_minute_with_gaps_is_not_extrapolated_from_the_gaps() {
        // 706 点全部在前 26 秒里涨完，之后 34 秒一动不动
        let points: VecDeque<GainPoint> = vec![point(26.0, 706)].into();
        let (rate, span) = window_rate(&points, None, 706, 60.0, 60.0);
        assert_eq!(span, 60.0, "分母必须是观察到的 60 秒，不是涨经验的那 26 秒");
        let rate = rate.unwrap();
        assert!((rate - 706.0).abs() < 1.0, "每分钟应该是 706，不是 1630：{rate}");
    }

    /// 窗口比会话还长时，按**实际测的那一段**折算，并把实测时长报出去。
    #[test]
    fn a_young_session_is_scaled_from_what_was_actually_measured() {
        // 6 分钟有效时间、净涨 4,264（就是小结卡片上那一组数）
        let points: VecDeque<GainPoint> = vec![point(120.0, 2_000), point(360.0, 4_264)].into();
        let (per_hour, hour_span) = window_rate(&points, None, 4_264, 360.0, 3600.0);
        assert_eq!(hour_span, 360.0);
        assert!(
            (per_hour.unwrap() - 42_640.0).abs() < 1.0,
            "每小时应该和卡片一致（4.2 万）：{per_hour:?}"
        );
        // 同一份数据，每分钟窗口是**滑窗**而不是整段平均：采样点之间按线性插值，
        // 最近一分钟（300~360 秒）落在 120→360 那一段的后四分之一上
        // （(4264-2000) × 60/240 = 566）。它和整段平均不一样是应该的。
        let (per_minute, minute_span) = window_rate(&points, None, 4_264, 360.0, 60.0);
        assert_eq!(minute_span, 60.0);
        assert!(
            (per_minute.unwrap() - 566.0).abs() < 1.0,
            "最近一分钟的经验：{per_minute:?}"
        );
    }

    /// 滑窗只看窗口内的那一段，窗口之外的老数据不算进来（靠管线相减 + 插值）。
    #[test]
    fn only_the_tail_of_the_curve_counts() {
        let points: VecDeque<GainPoint> =
            vec![point(10.0, 999_999), point(3_580.0, 1_000_000)].into();
        let (rate, span) = window_rate(&points, None, 1_000_600, 3_600.0, 60.0);
        assert_eq!(span, 60.0);
        // 窗口是 [3540, 3600]：管线在 3540 处插值出 1_000_000（最后一个点之前是平的），
        // 到 3600 是 1_000_600 → 600 点 / 60 秒
        assert!((rate.unwrap() - 600.0).abs() < 1.0, "只该算窗口内的：{rate:?}");
    }

    /// 太短的窗口不给数（那是噪声，不是速率）。
    #[test]
    fn too_short_a_window_has_no_rate() {
        let points: VecDeque<GainPoint> = vec![point(3.0, 300)].into();
        let (rate, span) = window_rate(&points, None, 300, 3.0, 60.0);
        assert!(rate.is_none());
        assert_eq!(span, 3.0);
    }

    /// 管线插值：窗口边界落在两个采样点中间时要按比例取，不能取整点。
    #[test]
    fn the_curve_interpolates_between_samples() {
        let points: VecDeque<GainPoint> = vec![point(10.0, 1_000), point(20.0, 3_000)].into();
        assert_eq!(total_at(None, &points, 0.0), 0, "没修剪过：会话开始时是 0");
        assert_eq!(total_at(None, &points, 10.0), 1_000);
        assert_eq!(total_at(None, &points, 15.0), 2_000, "正中间就是一半");
        assert_eq!(total_at(None, &points, 99.0), 3_000, "超出最后一个点就取最后的值");
    }

    /// **修剪后的窗口绝不能借会话起点当基准**（P1#1 回归）。
    ///
    /// 场景：开场领了 100 万任务经验，之后挂机近一小时没动。3700 秒的修剪线
    /// 把 (10 秒, 100 万) 那个点丢掉之后，队列只剩 (3700, 200 万)，而窗口边界
    /// （311 秒）落在队首之前 —— 旧实现从 (0, 0) 起插值，把窗口外的那 100 万
    /// 也算进这一小时。留着锚点插值：起点真值是 100 万，窗口内涨的是约 92 万，
    /// 两个方向都不说假话。
    #[test]
    fn a_pruned_window_never_borrows_the_session_start() {
        let origin = Some(point(10.0, 1_000_000));
        let points: VecDeque<GainPoint> = vec![point(3_700.0, 2_000_000)].into();
        let (rate, span) = window_rate(&points, origin, 2_000_000, 3_711.0, 3600.0);
        assert_eq!(span, 3600.0);
        let rate = rate.unwrap();
        // 真值：311 秒时总量已是 100 万（起点后没涨过），窗口内净涨约 92 万。
        // 旧实现（从 (0,0) 插）会报 194 万（凭空多出窗口外的 100 万）。
        assert!(
            (rate - 1_000_000.0).abs() < 100_000.0,
            "锚点插值应报出约 92 万，而不是 194 万或 0：{rate}"
        );
    }

    /// 队列被剪空（最近一小时一帧经验都没涨）时，锚点就是最后的已知总量，
    /// 算出来的窗口增量是 0 —— 不能退回 (0, 0) 把全场的总量报成「这一小时」。
    #[test]
    fn a_pruned_empty_queue_reports_no_new_gain() {
        let origin = Some(point(10.0, 2_000_000));
        let points: VecDeque<GainPoint> = VecDeque::new();
        let (rate, _) = window_rate(&points, origin, 2_000_000, 3_711.0, 3600.0);
        assert!(
            rate.unwrap().abs() < 1e-9,
            "这一小时确实一帧没涨，速率应该是 0"
        );
    }

    /// **分钟中途升级，分段经验不能整段丢失**（P1#4 回归）。
    ///
    /// 采样每 60 秒一个点，升级发生在第 85 秒时：旧实现里上一级的尾部
    /// （第 60 秒采样 → 升级那刻）和新级的头部都找不到采样点归属，
    /// 分段时速严重失真。升级帧现在会补交界点：被跨过的级记到满值、
    /// 新级从实际读数起步，两段拼起来正好是这一帧的总增量。
    #[test]
    fn a_mid_minute_level_up_keeps_both_sides_of_the_boundary() {
        let mut state = inner(Phase::Running);
        let need55 = table::requirement(55).unwrap();
        // 起点就选在 90% 分界之上（840000/926689 ≈ 90.6%）：升级前的最后一段
        // 把本级填满，才符合「快满时归零才是升级」的判定，升级帧才走得通。
        let start_exp = 840_000;
        let t0 = Instant::now();
        // 会话起点（真实 start() 会推一个 0 秒的起始点）；基准和当前值也对齐到它，
        // 不然第一帧会把「427096 → 840000」的差额也算进这一段。
        {
            let session = state.session.as_mut().unwrap();
            session.current_exp = Some(start_exp);
            session.points = vec![ExpSampleRow {
                captured_unix: 0,
                offset_secs: 0,
                level: Some(55),
                exp: start_exp,
                percent: 46.09,
            }];
        }
        state.baseline = Some((t0, start_exp, 55));
        state.last_effective = Some((start_exp, 55));

        // 第 60 秒：普通采样点（55 级，本级 860000）
        apply_read(&mut state, Ok(sample(860_000, 55)), t0, 60.0);
        assert_eq!(state.session.as_ref().unwrap().points.len(), 2, "整分钟点要落盘");

        // 第 85 秒升级（55 → 56，本级归零）：升级帧必须补交界点
        apply_read(&mut state, Ok(sample(0, 56)), t0, 25.0);
        let session = state.session.as_ref().unwrap();
        assert_eq!(session.current_level, Some(56));
        assert_eq!(session.gained_exp, 20_000 + (need55 - 860_000));
        // 四个点：起点 + 整分钟点 + 升级帧补的两个交界点（旧级满值、新级起点）
        assert_eq!(session.points.len(), 4, "升级帧要补两个交界点：{:#?}", session.points);
        assert_eq!(session.points[2].level, Some(55));
        assert_eq!(session.points[2].exp, need55, "被跨过的级记到满值");
        assert_eq!(session.points[3].level, Some(56));
        assert_eq!(session.points[3].exp, 0);

        // 分段不丢：55 级那段的净经验应该是「从 840000 一直填满到本级所需」
        let status = session.status();
        assert_eq!(status.stages.len(), 2, "按级切成两段");
        assert_eq!(status.stages[0].level, 55);
        assert_eq!(
            status.stages[0].gained,
            need55 - start_exp,
            "旧分段要包含尾部（上个采样点到升级那刻的差额）"
        );
        assert_eq!(status.stages[0].ended_at_secs, 85.0, "旧分段结束在升级那刻");
        assert!(!status.stages[0].ongoing);
        assert_eq!(status.stages[1].level, 56);
        assert!(status.stages[1].ongoing);
        assert_eq!(status.stages[1].gained, 0, "刚升级，新级还没涨");

        // 继续练：新级的分段 gained 要跟着**当前**读数走，不能停在旧采样点
        apply_read(&mut state, Ok(sample(1_300, 56)), t0, 65.0);
        let status = state.session.as_ref().unwrap().status();
        let ongoing = status.stages.last().unwrap();
        assert!(ongoing.ongoing);
        assert_eq!(
            ongoing.gained, 1_300,
            "进行中分段的 gained 用当前值，不能停在采样点的差值上"
        );
        assert_eq!(ongoing.ended_at_secs, 150.0);
    }

    /// 真机上这一帧**会显示**的百分比：游戏算的就是 `round(经验 ÷ 本级所需 × 100, 2)`。
    /// 测试里一律用它，不手写一个「差不多的」数 —— 手写过一次错的，
    /// 结果把一个本来唯一可解的帧变成了多解，测试反而在证明错的东西。
    fn shown_percent(exp: u64, level: u32) -> f64 {
        let need = table::requirement(level).expect("等级要在表里");
        (exp as f64 / need as f64 * 10000.0).round() / 100.0
    }

    fn sample(exp: u64, level: u32) -> Sample {
        Sample {
            exp,
            percent: shown_percent(exp, level),
            level: Some(level),
            raw: format!("{exp}({:.2}%)", shown_percent(exp, level)),
            method: Method::Window,
            region: None,
            screen_level: None,
        }
    }

    fn inner(phase: Phase) -> Inner {
        let start = table::cumulative(55, 427_096).unwrap();
        Inner {
            place_snapshot: MapSnapshot::default(),
            window_title: None,
            phase,
            session: Some(Session {
                started_at: chrono::Local::now(),
                started_unix: 0,
                ended_unix: None,
                active_secs: 0.0,
                gained_exp: 0,
                start_level: Some(55),
                start_percent: Some(46.09),
                current_level: Some(55),
                current_exp: Some(427_096),
                current_percent: Some(46.09),
                level_ups: 0,
                start_cum: Some(start),
                end_cum: Some(start),
                stats: SessionStats::default(),
                points: Vec::new(),
                last_bucket: 0,
            }),
            pending: None,
            points: VecDeque::new(),
            window_origin: None,
            baseline: Some((Instant::now(), 427_096, 55)),
            last_effective: Some((427_096, 55)),
            rebasing: false,
            last_sample: None,
            failure: None,
            last_read_at: None,
            last_gain_at: None,
            last_tick: None,
            recovery_anchor: None,
            confirmed_streak: 0,
            notice: None,
            goal_level: None,
            last_method: None,
            logged_first_read: false,
            logged_first_failure: false,
            region: None,
            region_lost: false,
        }
    }

    fn gained(inner: &Inner) -> u64 {
        inner.session.as_ref().map(|s| s.gained_exp).unwrap_or(0)
    }

    #[test]
    fn gains_accumulate_into_the_session() {
        let mut state = inner(Phase::Running);
        apply_read(&mut state, Ok(sample(427_596, 55)), Instant::now(), 1.0);
        assert_eq!(gained(&state), 500);
        assert_eq!(state.points.len(), 1);
    }

    /// 升级那一帧：本级剩余 + 新一级已有的，**绝不是负数**。
    #[test]
    fn level_up_counts_both_sides() {
        let mut state = inner(Phase::Running);
        let need = table::requirement(55).unwrap();
        state.baseline = Some((Instant::now(), need - 100, 55));
        apply_read(&mut state, Ok(sample(50, 56)), Instant::now(), 1.0);
        assert_eq!(gained(&state), 150);
        assert_eq!(state.session.as_ref().unwrap().level_ups, 1);
    }

    /// **坏帧不能污染下一帧**：认错的帧不动基准，下一帧照样按上一帧的好值算。
    #[test]
    fn a_rejected_frame_does_not_poison_the_next_one() {
        let mut state = inner(Phase::Running);
        let good = 427_596;
        apply_read(&mut state, Ok(sample(good, 55)), Instant::now(), 1.0);
        assert_eq!(state.baseline.map(|(_, exp, _)| exp), Some(good));

        let mut bad = sample(427_598, 55);
        bad.percent = 48.09; // 百分比对不上
        apply_read(&mut state, Ok(bad), Instant::now(), 1.0);
        assert_eq!(state.baseline.map(|(_, exp, _)| exp), Some(good), "坏帧不能动基准");
        assert_eq!(gained(&state), 500, "坏帧一点都不能进账");

        apply_read(&mut state, Ok(sample(good + 900, 55)), Instant::now(), 1.0);
        assert_eq!(gained(&state), 500 + 900, "坏帧不该吃掉后面这一帧");
    }

    /// **连续坏帧绝不能让 active_secs 二次方膨胀**（P0-2 回归测试）。
    ///
    /// 触发根因：
    /// `Advance::Unclear` 坏帧时状态机有意不动基准（下一帧接着按上一帧的好值算），
    /// 但旧实现推进会话时钟时用了「距基准的跨度 elapsed」（T_n - T_0），导致
    /// 连续 N 个坏帧累加 N(N+1)/2·Δt。真实过去 5 秒，分母被累加成 15 秒，时速暴跌至 1/3。
    ///
    /// 修复后时钟推进采用本次 tick 的真实增量 `wall`，无论连续多少个坏帧，
    /// `active_secs` 都严格按真实经过的时间线性增长。
    #[test]
    fn consecutive_unclear_frames_do_not_inflate_active_secs() {
        let mut state = inner(Phase::Running);
        let t0 = Instant::now();
        // 初始好帧：立稳基准
        apply_read(&mut state, Ok(sample(427_096, 55)), t0, 1.0);
        assert_eq!(state.session.as_ref().unwrap().active_secs, 1.0);

        // 连续喂 5 帧经验表对不上的坏帧（Advance::Unclear）
        // 模拟真实每秒一次采样：每个 tick 真实经过 1 秒（wall = 1.0）
        let mut bad = sample(427_598, 55);
        bad.percent = 48.09; // 百分比与经验表矛盾

        for i in 1..=5 {
            let tick_now = t0 + Duration::from_secs(i + 1);
            apply_read(&mut state, Ok(bad.clone()), tick_now, 1.0);
        }

        let session = state.session.as_ref().unwrap();
        // 加上最初那 1 帧，总共 6 轮每轮 1.0 秒，真实过去 6.0 秒
        // 旧实现里这里会累加 1 + (2 + 3 + 4 + 5 + 6) = 21.0 秒！
        assert!(
            (session.active_secs - 6.0).abs() < 1e-6,
            "active_secs 应该是 6.0 秒，而不是膨胀后的 {} 秒",
            session.active_secs
        );
        assert_eq!(session.stats.rejected_frames, 5, "5 帧坏帧都应记入拒绝统计");

        // 坏帧结束后恢复一个正常读数（+500 经验）
        let t_recovery = t0 + Duration::from_secs(7);
        apply_read(&mut state, Ok(sample(427_596, 55)), t_recovery, 1.0);

        let session = state.session.as_ref().unwrap();
        assert!((session.active_secs - 7.0).abs() < 1e-6);
        assert_eq!(session.gained_exp, 500);

        // 派生比值验证：经过 5 帧坏帧后涨经验，gained_cover(elapsed) 覆盖了
        // 自上一基准以来的有效打怪时间，active_ratio 不会被坏帧误降为挂机
        assert!(session.stats.active_ratio(session.active_secs) > 0.8);
    }

    /// 暂停期间读数只更新显示，不推进计时。
    #[test]
    fn paused_reads_do_not_advance_the_clock() {
        let mut state = inner(Phase::Paused);
        apply_read(&mut state, Ok(sample(500_000, 55)), Instant::now(), 1.0);
        assert_eq!(state.session.as_ref().unwrap().active_secs, 0.0);
        assert_eq!(gained(&state), 0);
    }

    /// **暂停那一段的增量不能记进来**：恢复时把下一帧标成「只立基准」。
    #[test]
    fn resuming_drops_whatever_happened_during_the_pause() {
        let mut state = inner(Phase::Running);
        apply_read(&mut state, Ok(sample(427_596, 55)), Instant::now(), 1.0);
        assert_eq!(gained(&state), 500);

        // 暂停 10 分钟，期间打了 30 万经验
        state.rebasing = true;
        apply_read(&mut state, Ok(sample(727_596, 55)), Instant::now(), 1.0);
        assert_eq!(gained(&state), 500, "暂停期间的 30 万不能记进来");
        assert!(!state.rebasing, "重立基准只用一帧");

        apply_read(&mut state, Ok(sample(728_000, 55)), Instant::now(), 1.0);
        assert_eq!(gained(&state), 500 + 404);
    }

    /// **用户报的那条 bug**：升级后经验值很小（`700(0.07%)`）时经验表多解，
    /// 但等级框读得到 —— 「开始统计」必须用它顶上，不能把用户拦在门外。
    #[test]
    fn starting_uses_the_level_box_when_the_exp_table_is_ambiguous() {
        let mut upgraded = sample(700, 56);
        upgraded.level = None; // 经验表：多解
        upgraded.screen_level = Some(56); // 等级框：56
        assert_eq!(start_level(&upgraded, None), Some(56));

        // 表定得出就用表；两个都没有才退回上一次的有效等级
        assert_eq!(start_level(&sample(427_096, 55), None), Some(55));
        let mut blank = sample(700, 56);
        blank.level = None;
        blank.screen_level = None;
        assert_eq!(start_level(&blank, Some(48)), Some(48));
        assert_eq!(start_level(&blank, None), None);
    }

    /// 统计中换到小号、经验表定不出等级：基准的等级要用等级框的，
    /// 不能把旧角色的等级留在账本里（那样之后每一帧都接不上）。
    #[test]
    fn a_character_switch_takes_the_level_from_the_level_box() {
        let mut state = inner(Phase::Running);
        let mut alt = sample(700, 12);
        alt.level = None;
        alt.screen_level = Some(12);
        apply_read(&mut state, Ok(alt), Instant::now(), 1.0);
        assert_eq!(state.baseline.map(|(_, exp, level)| (exp, level)), Some((700, 12)));
        assert_eq!(gained(&state), 0);

        // 下一帧就能正常接上
        apply_read(&mut state, Ok(sample(760, 12)), Instant::now(), 1.0);
        assert_eq!(gained(&state), 60);
    }

    /// 没在统计时换角色：显示的等级跟着读数走，不挂着上一段留下的那个。
    #[test]
    fn an_idle_read_refreshes_the_shown_level() {
        let mut state = inner(Phase::Idle);
        state.session = None;
        state.baseline = None;
        apply_read(&mut state, Ok(sample(9_000, 20)), Instant::now(), 3.0);
        assert_eq!(state.last_effective, Some((9_000, 20)));

        // 暂停中不动：那是账本的基准
        let mut paused = inner(Phase::Paused);
        apply_read(&mut paused, Ok(sample(9_000, 20)), Instant::now(), 3.0);
        assert_eq!(paused.last_effective, Some((427_096, 55)));
    }

    /// 地图名近似只在**等长、差一个字、唯一候选**时采纳。
    #[test]
    fn map_name_snaps_to_a_single_one_character_neighbour() {
        assert!(is_one_edit_apart("魔法密林", "随法密林"));
        assert!(!is_one_edit_apart("魔法密林", "魔法密林"));
        // 「魔幻秘林」差两个字 —— 不认
        assert!(!is_one_edit_apart("魔法密林", "魔幻秘林"));
        assert!(!is_one_edit_apart("魔法密林", "魔法密林镇"));

        let path = std::env::temp_dir().join(format!(
            "mxdbox-test-map-snap-{}.db",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);
        let db = Database::new(path.clone()).expect("建库失败");
        db.set_exp_map("fp1", "魔法密林").expect("写地图名失败");
        db.set_exp_map("fp2", "废弃都市").expect("写地图名失败");
        assert_eq!(snap_map_name(&db, "随法密林").as_deref(), Some("魔法密林"));
        assert_eq!(snap_map_name(&db, "魔法密林"), None);
        assert_eq!(snap_map_name(&db, "天空之城"), None);
        // 两个等长的近似候选 → 不猜
        db.set_exp_map("fp3", "魔法密休").expect("写地图名失败");
        assert_eq!(snap_map_name(&db, "魔法密林"), None);
        let _ = std::fs::remove_file(path);
    }

    /// 读失败不推进任何东西（尤其不能记 0）。
    #[test]
    fn failed_reads_change_nothing_but_the_status() {
        let mut state = inner(Phase::Running);
        apply_read(
            &mut state,
            Err(ReadFailure::NoFieldAt {
                client: "1920×1080".to_string(),
                detail: String::new(),
            }),
            Instant::now(),
            1.0,
        );
        assert_eq!(gained(&state), 0);
        assert_eq!(state.session.as_ref().unwrap().active_secs, 0.0);
        assert_eq!(state.failure.map(|f| f.code()), Some("no_field"));
    }

    /// **用户报的那条 bug 的机器证明**：从 1 级一路打上去，
    /// 每一个增量都非负，累计经验只增不减。
    #[test]
    fn a_long_random_walk_never_produces_a_negative_delta() {
        let mut state = inner(Phase::Running);
        state.session = Some(Session {
            started_at: chrono::Local::now(),
            started_unix: 0,
            ended_unix: None,
            active_secs: 0.0,
            gained_exp: 0,
            start_level: Some(1),
            start_percent: Some(0.0),
            current_level: Some(1),
            current_exp: Some(0),
            current_percent: Some(0.0),
            level_ups: 0,
            start_cum: Some(0),
            end_cum: Some(0),
            stats: SessionStats::default(),
            points: Vec::new(),
            last_bucket: 0,
        });
        state.baseline = Some((Instant::now(), 0, 1));
        state.last_effective = Some((0, 1));

        let mut seed: u64 = 0x2545_F491_4F6C_DD1D;
        let (mut exp, mut level, mut total) = (0u64, 1u32, 0u64);
        let mut level_ups_seen = 0u32;
        for step in 0..4_000 {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            exp += seed % 30_000;
            while level < 120 {
                let Some(need) = table::requirement(level) else {
                    break;
                };
                if exp < need {
                    break;
                }
                exp -= need;
                level += 1;
                level_ups_seen += 1;
            }
            if level >= 120 {
                break;
            }
            let mut frame = sample(exp, level);
            // 偶尔掺一帧认错的
            if step % 37 == 0 {
                frame.percent = (frame.percent + 7.0).min(100.0);
            }
            apply_read(&mut state, Ok(frame), Instant::now(), 1.0);

            let now_total = gained(&state);
            assert!(
                now_total >= total,
                "第 {step} 步累计经验倒退了：{total} → {now_total}（{level} 级 {exp} 点）"
            );
            total = now_total;
        }
        assert!(level_ups_seen > 50, "这一趟应该升了不少级：{level_ups_seen}");
        assert!(total > 0, "应该记到经验");
    }

    // -----------------------------------------------------------------------
    // 质量结论
    // -----------------------------------------------------------------------

    fn stats(readable: f64, unreadable: f64, frames: u32, rejected: u32, gaining: f64) -> SessionStats {
        SessionStats {
            readable_secs: readable,
            unreadable_secs: unreadable,
            frames,
            rejected_frames: rejected,
            gaining_secs: gaining,
        }
    }

    #[test]
    fn a_clean_session_is_graded_complete() {
        let quality = stats(600.0, 0.0, 300, 0, 500.0).grade(600.0);
        assert_eq!(quality.grade, 2);
        assert_eq!(quality.label, "数据完整");
        assert!((quality.coverage - 1.0).abs() < 1e-9);
    }

    #[test]
    fn a_hidden_screen_downgrades_the_session() {
        let quality = stats(300.0, 300.0, 300, 0, 250.0).grade(600.0);
        assert_eq!(quality.grade, 0);
        assert!(quality.reason.contains("读不到游戏画面"), "{}", quality.reason);
        assert_eq!(stats(540.0, 60.0, 300, 0, 500.0).grade(600.0).grade, 1);
        assert_eq!(stats(570.0, 30.0, 300, 0, 500.0).grade(600.0).grade, 2);
    }

    #[test]
    fn any_rejected_frame_stops_it_being_called_complete() {
        let quality = stats(600.0, 0.0, 300, 1, 500.0).grade(600.0);
        assert_eq!(quality.grade, 1);
        assert!(quality.reason.contains("对不上"), "{}", quality.reason);
    }

    /// 挂机多不代表数据不可信 —— 两件事分开说。
    #[test]
    fn idle_time_is_reported_separately_from_trust() {
        let quality = stats(600.0, 0.0, 300, 0, 120.0).grade(600.0);
        assert_eq!(quality.grade, 2, "挂机不影响「读得到」这件事");
        assert!((quality.active_ratio - 0.2).abs() < 1e-9);
    }

    #[test]
    fn a_too_short_session_is_not_called_complete() {
        let quality = SessionStats::default().grade(0.0);
        assert_eq!(quality.grade, 1);
        assert!(quality.reason.contains("太短"), "{}", quality.reason);
    }

    /// **`toggle` 绝不能死锁。**
    ///
    /// 真机上挂过一次：`match self.inner.lock().phase { .. => self.start(), .. }` 里
    /// 那个临时 guard 活到整个 `match` 结束，而 `start()` 又要 `lock()` 一次 ——
    /// 不可重入的锁自己等自己，按一下 `Ctrl+Alt+P` 进程就永久不响应了。
    ///
    /// 用另一个线程 + 超时来验：死锁时这个测试会**失败**而不是把测试套件挂住。
    #[test]
    fn toggling_never_deadlocks() {
        for _ in 0..3 {
            let tracker = std::sync::Arc::new(ExpTracker::new(ExpReader::new(Font::builtin())));
            let (sender, receiver) = std::sync::mpsc::channel();
            let worker = {
                let tracker = std::sync::Arc::clone(&tracker);
                std::thread::spawn(move || {
                    // 三次切换走完一个完整循环（开始可能因为读不到游戏而返回 Err，
                    // 那也是**正常返回**，只要不是卡住）
                    let _ = tracker.toggle();
                    let _ = tracker.toggle();
                    let _ = tracker.toggle();
                    let _ = sender.send(());
                })
            };
            receiver
                .recv_timeout(Duration::from_secs(10))
                .expect("toggle() 死锁了 —— 检查是不是在持锁的情况下又调用了 start/pause/resume");
            worker.join().expect("线程崩了");
        }
    }

    /// **暂停状态下调 start 必须是「继续」**，不能报错。
    ///
    /// 真机事故：悬浮窗在暂停后点开始，前端发的是 start_exp_session，
    /// 后端回「已经在统计里了」，而界面把错误吞掉 —— 表现就是「点了没反应」。
    /// 现在两条路都通：前端按状态选命令，后端也把 start 当成 resume。
    #[test]
    fn starting_while_paused_resumes_instead_of_failing() {
        let tracker = ExpTracker::new(ExpReader::new(Font::builtin()));
        {
            let mut guard = tracker.inner.lock();
            guard.phase = Phase::Paused;
            guard.session = Some(Session {
                started_at: chrono::Local::now(),
                started_unix: 0,
                ended_unix: None,
                active_secs: 12.0,
                gained_exp: 0,
                start_level: Some(21),
                start_percent: Some(0.0),
                current_level: Some(21),
                current_exp: Some(0),
                current_percent: Some(0.0),
                level_ups: 0,
                start_cum: None,
                end_cum: None,
                stats: SessionStats::default(),
                points: Vec::new(),
                last_bucket: 0,
            });
        }
        tracker.start().expect("暂停状态下 start 不该报错");
        assert_eq!(tracker.inner.lock().phase, Phase::Running, "应该变成统计中");
    }

    /// `status()` 是同步命令 `get_exp_status` 的必经之路（前端每 2 秒在 UI 线程上
    /// 调一次），**它绝不能碰 `io` 锁** —— 那把锁里可能是几百毫秒的 ONNX 编译与
    /// 推理，碰一下界面就卡一下。这条以前被破坏过一次（把慢活移出 inner 时，
    /// 顺手把两样几乎静态的东西放进了 io），所以扫源码钉住，不靠自觉。
    #[test]
    fn goal_is_plain_subtraction_on_cumulative_exp() {
        // 55 级 427,096 → 目标 57 级：本级剩下的 + 56 级整级
        let now = table::cumulative(55, 427_096);
        let left = table::remaining(55, 427_096).unwrap() + table::requirement(56).unwrap();
        let goal = goal_progress(57, now, Some(left as f64));
        assert_eq!(goal.remaining_exp, Some(left));
        assert!(!goal.reached);
        // 时速刚好等于剩余经验 → 正好一小时
        assert!((goal.eta_secs.unwrap() - 3600.0).abs() < 1e-6);

        // 没在统计（没有时速）/ 时速是 0：还差多少照样给，只是没有时间
        assert_eq!(goal_progress(57, now, None).eta_secs, None);
        assert_eq!(goal_progress(57, now, Some(0.0)).eta_secs, None);
        assert_eq!(goal_progress(57, now, None).remaining_exp, Some(left));

        // 已经到了 / 超过了：差 0，标记到达，不给时间
        let reached = goal_progress(55, now, Some(1_000_000.0));
        assert_eq!((reached.remaining_exp, reached.reached, reached.eta_secs), (Some(0), true, None));

        // 还没读到现在的等级：目标还在，只是算不出差多少
        let unknown = goal_progress(57, None, Some(1_000_000.0));
        assert_eq!((unknown.remaining_exp, unknown.reached, unknown.eta_secs), (None, false, None));

        // 目标可以设到经验表最后一级
        assert!(goal_progress(MAX_GOAL_LEVEL, now, None).remaining_exp.is_some());
    }

    #[test]
    fn status_never_touches_the_io_lock() {
        let source = include_str!("tracker.rs");
        let start = source
            .find("pub fn status(&self)")
            .expect("找不到 status()，改名了就同步改这个测试");
        let open = source[start..]
            .find('{')
            .map(|offset| start + offset)
            .expect("status() 没有函数体");
        let mut depth = 0usize;
        let mut end = None;
        for (offset, ch) in source[open..].char_indices() {
            match ch {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        end = Some(open + offset);
                        break;
                    }
                }
                _ => {}
            }
        }
        let body = &source[open..=end.expect("status() 的函数体没闭合")];
        // 只看代码行：注释里提一句 `self.io` 是正常的（比如就在解释这条规矩），
        // 真正要拦的是**代码**里再去拿那把锁
        let code: String = body
            .lines()
            .filter(|line| !line.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            !code.contains("self.io"),
            "status() 又去碰 io 锁了：它每 2 秒在 UI 线程上被调一次，会把界面卡住"
        );
        // 光“没碰 io”不够，还得确认那两样是从该来的地方来的
        assert!(
            body.contains("guard.window_title"),
            "窗口标题应当从 inner 读（防止有人改回 io）"
        );
        assert!(
            body.contains("self.font_symbols"),
            "字形表应当读构造时算好的常量串（防止有人改回 io）"
        );
    }

    /// 采样期间「读得到 / 读不到」要真的累起来，而且**最小化不算拒绝帧**。
    #[test]
    fn coverage_counts_readable_and_unreadable_time() {
        let mut state = inner(Phase::Running);
        state.baseline = Some((Instant::now() - Duration::from_secs(2), 427_096, 55));
        apply_read(&mut state, Ok(sample(427_596, 55)), Instant::now(), 2.0);
        apply_read(&mut state, Err(ReadFailure::Minimized), Instant::now(), 2.0);
        apply_read(
            &mut state,
            Err(ReadFailure::Contradiction {
                raw: "427098(48.09%)".to_string(),
                exp: 427_098,
                percent: 48.09,
            }),
            Instant::now(),
            2.0,
        );
        let session = state.session.as_ref().unwrap();
        assert_eq!(session.stats.frames, 3);
        assert!((session.stats.readable_secs - 2.0).abs() < 1e-9);
        assert!((session.stats.unreadable_secs - 4.0).abs() < 1e-9);
        assert_eq!(session.stats.rejected_frames, 1, "最小化不算拒绝帧");
        assert!((session.quality().coverage - 1.0 / 3.0).abs() < 1e-9);
    }

    /// **落库的结束时间必须是真实墙钟，不能用 started+active 反算**（P1#5 回归）。
    ///
    /// 场景：11:00 开始统计，中途暂停去吃了两小时饭，14:30 点「结束」。
    /// 有效时间只有 1 小时，旧实现拿 `started + active_secs` 反算，落库成
    /// 「12:00 结束」 —— 历史时间轴交叠错乱，和真实世界完全脱节。
    /// 顺带钉住 end_exp：它必须取实时值，不能取采样队列里最后一个点
    /// （最多滞后 59 秒，和同一行的 gained_exp 对不上账）。
    #[test]
    fn resolve_pending_keeps_the_real_wall_clock_end() {
        use crate::db::Database;
        let tracker = ExpTracker::new(ExpReader::new(Font::builtin()));
        let started = chrono::Local::now().timestamp() - 12_600; // 3.5 小时前
        {
            let mut guard = tracker.inner.lock();
            let cum = table::cumulative(55, 427_999);
            guard.pending = Some(Session {
                started_at: chrono::Local::now(),
                started_unix: started,
                // end() 在结束时冻结的墙钟：比开始晚了 3.5 小时（含暂停 2 小时）
                ended_unix: Some(started + 12_600),
                active_secs: 3_600.0, // 有效时间只有 1 小时
                gained_exp: 903,
                start_level: Some(55),
                start_percent: Some(46.09),
                current_level: Some(55),
                current_exp: Some(427_999),
                current_percent: Some(46.10),
                level_ups: 0,
                start_cum: Some(cum.unwrap() - 903),
                end_cum: cum,
                stats: SessionStats::default(),
                points: vec![
                    ExpSampleRow {
                        captured_unix: started,
                        offset_secs: 0,
                        level: Some(55),
                        exp: 427_096,
                        percent: 46.09,
                    },
                    // 队列最后一个点停在 3600 秒处（最多滞后 59 秒），
                    // 不是结束那一刻的读数 —— end_exp 不能取它
                    ExpSampleRow {
                        captured_unix: started + 12_600,
                        offset_secs: 3_600,
                        level: Some(55),
                        exp: 427_596,
                        percent: 46.15,
                    },
                ],
                last_bucket: 60,
            });
        }

        let db_path = std::env::temp_dir().join(format!(
            "mxdbox-test-resolve-{}.db",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&db_path);
        let db = Database::new(db_path.clone()).expect("建库失败");
        tracker
            .resolve_pending(true, "", &db)
            .expect("计入历史不该失败");

        let rows = db.exp_history(10).expect("读历史失败");
        assert_eq!(rows.len(), 1);
        let row = &rows[0];
        assert_eq!(
            row.ended_unix, started + 12_600,
            "结束时间必须是 end() 冻结的墙钟，不是 started+active 反算的「开始后 1 小时」"
        );
        assert_eq!(row.active_secs, 3_600.0);

        // end_exp 不在历史列表的返回形状里，直接查库对账
        {
            let conn = rusqlite::Connection::open(&db_path).expect("打开库失败");
            let (end_exp, gained): (u64, u64) = conn
                .query_row(
                    "SELECT end_exp, gained_exp FROM exp_sessions LIMIT 1",
                    [],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .expect("读会话行失败");
            assert_eq!(end_exp, 427_999, "终点经验用实时值，不是滞后 59 秒的采样点");
            assert_eq!(gained, 903, "总增量不该被这条修复影响");
        }

        let _ = std::fs::remove_file(&db_path);
    }
}
