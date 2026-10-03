//! 小册子「开服监控」数据源。
//!
//! 站点自己就在做这件事：https://mxdc.dvg.cn/tools/server-monitor/
//! 它每分钟去探一次游戏登录入口，把「维护中 / 正在确认 / 服务器正常」连同
//! 状态变化历史都记在一个只回 JSON 的接口里（页面上的「状态变化」就是它）。
//!
//! 我们**接它而不是自己另起一套判断**，理由是：
//!
//! * 用户要的语义是「官方开服了没有」，而这个站点已经在为整个社区回答它，
//!   比我们自己盯着一个可能轮换掉的登录网关端口更接近标准答案；
//! * 用户自填端口的 TCP 握手只能证明「这个端口通了」，地址一换就默默失效 ——
//!   那种「静静地变错」恰恰是告警工具最糟的失败模式；
//! * 两条来源一起用正好互补：小册子说恢复 → 我们提醒；自己那个地址连不上 →
//!   提示重新检测。共用 `alert::fire_open_alert` 里的冷却，不会各响一遍。
//!
//! ## 接口形状（2026-09 实测）
//!
//! ```json
//! { "ok": true, "now": 1790046981, "interval": 20, "stale": false,
//!   "snapshot": {
//!     "checked_at": 1790046961,
//!     "status": "login_recovered",
//!     "login":    { "name": "登录入口", "status": "greeting", "detail": "握手正常" },
//!     "official": { "status": "unknown", "detail": "缓存标记维护（时效待确认）" },
//!     "event_id": 4, "notification_rule": 4, "changed_at": 1789984081,
//!     "history": [ { "at": 1789984081, "status": "login_recovered",
//!                    "login": "greeting", "official": "unknown" } ] } }
//! ```
//!
//! 注意 `history` 里 `login` / `official` 是**字符串**，而 `snapshot` 里是
//! **对象** —— 同一份数据两种形状，所以分成两个结构体，并且都宽容缺字段。

use crate::alert::{fire_open_alert, AlertSource};
use crate::mxdc_client::MxdcClient;
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tauri::{AppHandle, Emitter};

/// 小册子开服监控接口（只回 JSON，不需要登录）。
pub const MONITOR_API: &str = "https://mxdc.dvg.cn/tools/server-monitor/api.php";

/// 首次抓取就报警的「新鲜期」。
///
/// 正常情况下第一次采样只用来建立基线 —— 打开程序时服务器本来就开着，
/// 不该报「刚刚开服了」。但如果这次「正常」是刚刚才发生的（例如维护结束你马上
/// 打开工具箱），那就是用户真正在等的那个时刻，应该照报。
const FRESH_TRANSITION_SEC: i64 = 600;

/// 站点给的 `checked_at` 超过这个岁数就认为数据不可信，不据此报警。
const STALE_SAMPLE_SEC: i64 = 300;

/// **我们自己**多久没问到新数据，这份结论就不算新鲜（界面必须如实显示年龄）。
///
/// 和上面那个 `STALE_SAMPLE_SEC` 不是一回事：那个是站点说“我的采样太旧了”，
/// 这个是我们这边（断网 / 站点 5xx / 监控被停用）—— 以前年龄只在采样成功那一刻
/// 算一次、之后原地冻结，于是断网一小时，界面还写着「检测于 11:29:01（20 秒前）」，
/// 把没数据显示成数据很新。默认 30 秒一轮，5 分钟等于至少十轮都没成功。
const LOCAL_STALE_SEC: i64 = 300;

// ---------------------------------------------------------------------------
// 接口模型
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize)]
pub struct LoginProbe {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub detail: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct OfficialProbe {
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub detail: Option<String>,
}

/// 历史里的一个点。`login` / `official` 在这里是**字符串**，别和 `snapshot` 混。
#[derive(Debug, Clone, Deserialize)]
pub struct HistoryEntry {
    #[serde(default)]
    pub at: i64,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub login: String,
    #[serde(default)]
    pub official: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Snapshot {
    #[serde(default)]
    pub checked_at: i64,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub login: Option<LoginProbe>,
    #[serde(default)]
    pub official: Option<OfficialProbe>,
    #[serde(default)]
    pub changed_at: Option<i64>,
    #[serde(default)]
    pub history: Vec<HistoryEntry>,
}

/// 接口外层信封。
#[derive(Debug, Clone, Deserialize)]
struct Envelope {
    #[serde(default)]
    ok: bool,
    #[serde(default)]
    snapshot: Option<Snapshot>,
    #[serde(default)]
    interval: Option<u64>,
    #[serde(default)]
    stale: bool,
    #[serde(default)]
    error: Option<String>,
    #[serde(default)]
    message: Option<String>,
}

/// 一次采样的完整结果：快照 + 站点自己标的两项可信度信息。
#[derive(Debug, Clone)]
pub struct MonitorFeed {
    pub snapshot: Snapshot,
    /// 站点自己说「这份快照可能已经不新了」（它的采集器卡住 / 刚恢复）
    pub stale: bool,
    /// 站点自己的采集间隔，用来解释「数据最老能有多旧」
    pub interval_sec: Option<u64>,
}

// ---------------------------------------------------------------------------
// 状态说法 / 报警规则（纯函数，全部有单测）
// ---------------------------------------------------------------------------

/// 站点状态码 → 中文说法。
///
/// 站点页面上原样的措辞（「服务器维护中」这类），不自己另造一套，用户对照
/// 小册子页面时不会觉得两个工具在说两件事。
pub fn status_label(status: &str) -> String {
    match status {
        "login_recovered" => "服务器正常".to_string(),
        "maintenance" => "服务器维护中".to_string(),
        "confirming" => "正在确认状态".to_string(),
        "unavailable" => "状态暂不可用".to_string(),
        "unknown" | "" => "状态未知".to_string(),
        // 站点以后加了新状态码：原样带出来，别装作是「未知」
        other => format!("未知状态（{}）", other),
    }
}

/// 这个状态是不是「已经确认可以登录」。
///
/// 只看 `login_recovered`：`confirming` 是站点还在确认，那时候喊「开服了」
/// 会在维护延长时误报，而误报一次就会让人开始不信这个提醒。
pub fn is_open(status: &str) -> bool {
    status == "login_recovered"
}

/// 官方公告的中文说法。
pub fn official_label(status: &str) -> String {
    match status {
        "maintenance" => "官方公告：维护中".to_string(),
        "open" | "opened" => "官方公告：已开服".to_string(),
        "unknown" | "" => "暂无官方消息".to_string(),
        other => format!("官方状态（{}）", other),
    }
}

/// 到底要不要为这次采样报警。
///
/// 规则：**只有真的「从非正常变成正常」才报**。
/// * `previous` 是本进程记着的上一轮状态（抓取失败时保留旧值）；
/// * 首次采样（`previous = None`）只建立基线，除非这次「正常」是新鲜发生的
///   （见 `FRESH_TRANSITION_SEC`）—— 维护刚结束时打开工具箱，正是最该被提醒的场景；
/// * `checked_at` 太旧（站点采集器卡住）时不报，宁可不报也别报错的。
pub fn should_alert(
    previous: Option<&str>,
    current: &str,
    changed_at: Option<i64>,
    checked_at: i64,
    now: i64,
) -> bool {
    if !is_open(current) {
        return false;
    }

    if checked_at > 0 && now - checked_at > STALE_SAMPLE_SEC {
        return false;
    }

    match previous {
        // 已经报过的状态别重复报
        Some(prev) if is_open(prev) => false,
        Some(_) => true,
        None => match changed_at {
            Some(at) if at > 0 => now - at <= FRESH_TRANSITION_SEC,
            // 站点没给变更时刻（老接口 / 字段缺失）：保守起见不报
            _ => false,
        },
    }
}

/// 登录入口自己的说法（`snapshot.login.status`）。
///
/// 和总状态分开写：总状态是站点对「能不能登录」的综合判断，
/// 而这里是它对登录入口那一次握手的原始观察，排查时两个都要看。
pub fn login_label(status: &str) -> String {
    match status {
        "greeting" => "登录入口握手正常".to_string(),
        "unavailable" => "登录入口连不上".to_string(),
        "unknown" | "" => "登录入口状态未知".to_string(),
        other => format!("登录入口（{}）", other),
    }
}

/// 解析接口响应。
///
/// 「站点自报 stale」直接带出来，不在这里丢掉：它正是「不要据此报警」的依据。
pub fn parse_snapshot(body: &str) -> Result<MonitorFeed, String> {
    let envelope: Envelope =
        serde_json::from_str(body.trim()).map_err(|e| format!("开服监控接口不是预期的 JSON: {}", e))?;

    if !envelope.ok {
        let reason = envelope
            .error
            .or(envelope.message)
            .unwrap_or_else(|| "站点返回 ok=false".to_string());
        return Err(format!("开服监控接口报错: {}", reason));
    }

    let mut snapshot = envelope
        .snapshot
        .ok_or_else(|| "开服监控接口没有 snapshot 字段".to_string())?;

    // 历史太长，界面只画最近这些；顺手排序成「新 → 旧」
    snapshot.history.sort_by(|a, b| b.at.cmp(&a.at));
    snapshot.history.truncate(40);

    Ok(MonitorFeed {
        snapshot,
        stale: envelope.stale,
        interval_sec: envelope.interval,
    })
}

/// 这一次采样该不该报警。
///
/// 比 `should_alert` 多一层：站点自己标了 `stale`（它的采集器没跟上）时不报 ——
/// 「不一定还成立」的结论不该把人从床上叫起来。
pub fn feed_should_alert(feed: &MonitorFeed, previous: Option<&str>, now: i64) -> bool {
    if feed.stale {
        return false;
    }
    should_alert(
        previous,
        &feed.snapshot.status,
        feed.snapshot.changed_at,
        feed.snapshot.checked_at,
        now,
    )
}

// ---------------------------------------------------------------------------
// 给界面的状态
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
pub struct HistoryPoint {
    /// 本地时间，例如 "2026-09-21 17:48:01"
    pub at: String,
    pub status: String,
    pub label: String,
    pub open: bool,
    /// 那一刻登录入口自己的说法（历史里是状态码，这里翻成中文）
    pub login_label: String,
    /// 那一刻官方公告的状态
    pub official_label: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct MonitorStatus {
    /// 用户有没有开这个监控（关了就不轮询）
    pub enabled: bool,
    pub interval_sec: u64,
    /// 站点原始状态码
    pub status: String,
    /// 中文说法
    pub status_label: String,
    /// 是否已确认可以登录
    pub open: bool,
    /// 状态维持至今的起点（本地时间字符串），取不到就是空
    pub changed_at: String,
    /// 站点这次采样的时刻（本地时间字符串，给人看的）
    pub checked_at: String,
    /// 站点这次采样的**原始时间戳**（epoch 秒，用来做算术的）。
    ///
    /// 年龄一律拿这个算，不拿 `checked_at` 那个字符串：字符串没法做减法，
    /// 也就没法在每次读的时候把年龄刷新成真实值。
    pub checked_at_epoch: i64,
    /// 距今多少秒前采到；从未成功过 = None。**每次读都按当前时间重算**，
    /// 不是采样时算好存下来的定值（那样失败 / 停用之后它就不动了）。
    pub checked_ago_sec: Option<i64>,
    /// 这份结论**已经不新鲜**：监控已停用、从没拿到过数据，或距上次成功采样
    /// 超过 `LOCAL_STALE_SEC`。`false` 才代表“监控开着，且这是 5 分钟内的活数据”。
    ///
    /// 它不改变结论本身 —— `status` / `open` 照旧保留（站点抖一下不该让用户紧张），
    /// 只是告诉界面：这句话已经旧了，别再当实时状态渲染。
    pub snapshot_stale: bool,
    pub official_label: String,
    pub official_detail: String,
    /// 登录入口的原始观察（"登录入口握手正常"）
    pub login_label: String,
    pub login_detail: String,
    /// 最近的状态变化（新 → 旧）
    pub history: Vec<HistoryPoint>,
    /// 抓取失败时的一句话（网络不通 / 站点报错），成功时为 None
    pub error: Option<String>,
    /// 数据来源说明，界面上照实写出来
    pub source: String,
}

impl MonitorStatus {
    fn initial(enabled: bool, interval_sec: u64) -> Self {
        Self {
            enabled,
            interval_sec,
            status: String::new(),
            status_label: "还没拿到数据".to_string(),
            open: false,
            changed_at: String::new(),
            checked_at: String::new(),
            checked_at_epoch: 0,
            checked_ago_sec: None,
            // 还没有任何采样：按“不新鲜”算，直到第一次成功采到东西。
            snapshot_stale: true,
            official_label: official_label("unknown"),
            official_detail: String::new(),
            login_label: login_label("unknown"),
            login_detail: String::new(),
            history: Vec::new(),
            error: None,
            source: "小册子 · 开服监控（tools/server-monitor/api.php）".to_string(),
        }
    }
}

/// 把「这份结论有多旧」如实写进状态。
///
/// 为什么不干脆在采样成功时算好存起来（以前就是这样）：那样得到的是一个**定值**，
/// 失败、停用之后它原地冻结，而恰恰是没数据的时候最需要它动 —— 界面拿它显示
///「检测于 xx（多久之前）」，冻结的年龄就是一句谎话。所以每次读都重算，
/// `status()`（命令与广播走的都是它）与采样写入都调这一个函数，别再各算各的。
fn recompute_freshness(status: &mut MonitorStatus, now: i64) {
    status.checked_ago_sec =
        (status.checked_at_epoch > 0).then(|| (now - status.checked_at_epoch).max(0));
    status.snapshot_stale = !status.enabled
        || status.checked_at_epoch <= 0
        || now - status.checked_at_epoch > LOCAL_STALE_SEC;
}

fn local_time(epoch: i64) -> String {
    if epoch <= 0 {
        return String::new();
    }
    chrono::DateTime::from_timestamp(epoch, 0)
        .map(|utc| {
            let local: chrono::DateTime<chrono::Local> = utc.into();
            local.format("%Y-%m-%d %H:%M:%S").to_string()
        })
        .unwrap_or_default()
}

// ---------------------------------------------------------------------------
// 轮询管理
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct MonitorConfig {
    pub enabled: bool,
    pub interval_sec: u64,
}

pub struct MonitorManager {
    /// **加锁顺序：先 `config`，后 `status`。** 同一个地方要碰这两把锁时，
    /// 必须按这个顺序拿；反过来（拿着 `status` 再去拿 `config`）会死锁。
    ///
    /// 为什么要有这条规矩：`update_config` 跑在 Tauri 的命令线程上、`refresh_once`
    /// 跑在 tokio 的轮询任务上，是两个不同的 OS 线程。它俩曾经一个走
    /// `config.write → status.write`、另一个走 `status.write → config.read`，方向相反 ——
    /// parking_lot 的写者排队机制会让 A 等 B 的写锁、B 等 A 的写锁，**双向永久阻塞、不会自解**：
    /// 监控从此不再采样，而且一句日志、一条界面提示都没有（正是本项目最忌讳的「静静地变错」），
    /// 设置页那边的「保存拉取间隔」也一起转圈卡死，只能重启。
    ///
    /// 只碰**一把**锁的调用（`config()` / `status()` / `broadcast` / `record_error` /
    /// 轮询循环里两处分开的 `manager.config()`）不受这条约束 —— 约束针对的是**嵌套**拿锁。
    /// 这种事没法写成可移植的并发测试（要复现得靠两个线程恰好交错在几微秒的窗口里），
    /// 所以由 `tests/monitor_lock_order.rs` 扫源码守住这条顺序。
    config: RwLock<MonitorConfig>,
    status: RwLock<MonitorStatus>,
    /// 上一轮的站点状态码 —— 报警只看它有没有从「非正常」翻到「正常」。
    last_status: RwLock<Option<String>>,
    /// 抓取失败时不重复刷屏（日志与事件都只在结论变化时推）。
    last_error: RwLock<Option<String>>,
    /// 是否已经成功抓过一次（决定首次采样只建基线）。
    started: AtomicBool,
    client: Arc<MxdcClient>,
}

impl MonitorManager {
    pub fn new(client: Arc<MxdcClient>, config: MonitorConfig) -> Self {
        Self {
            status: RwLock::new(MonitorStatus::initial(config.enabled, config.interval_sec)),
            config: RwLock::new(config),
            last_status: RwLock::new(None),
            last_error: RwLock::new(None),
            started: AtomicBool::new(false),
            client,
        }
    }

    pub fn config(&self) -> MonitorConfig {
        self.config.read().clone()
    }

    pub fn status(&self) -> MonitorStatus {
        let mut status = self.status.read().clone();
        // 每次读都把年龄刷新成真实值：状态本身（`status` / `open` / `history`）
        // 是上一次成功采样的结论，保持不动；**只有“它有多旧”必须是活的**。
        recompute_freshness(&mut status, chrono::Utc::now().timestamp());
        status
    }

    pub fn update_config(&self, config: MonitorConfig) {
        let mut current = self.config.write();
        let interval_changed = current.interval_sec != config.interval_sec;
        let enabled_changed = current.enabled != config.enabled;
        *current = config.clone();

        {
            let mut status = self.status.write();
            status.enabled = config.enabled;
            status.interval_sec = config.interval_sec;
        }

        if enabled_changed {
            log::info!(
                "小册子开服监控{}",
                if config.enabled {
                    format!("已启用（每 {} 秒轮询）", config.interval_sec)
                } else {
                    "已关闭".to_string()
                }
            );
        } else if interval_changed {
            log::info!("小册子开服监控间隔改为 {} 秒", config.interval_sec);
        }
    }

    /// 启动轮询循环。
    ///
    /// 和探测循环一样，第一次采样**立刻**做（不等一个间隔），否则用户重新打开
    /// 工具箱后要盯着「还没拿到数据」看一整轮。
    pub fn start_loop(self: &Arc<Self>, app: AppHandle) {
        let manager = Arc::clone(self);
        tauri::async_runtime::spawn(async move {
            loop {
                let config = manager.config();
                if config.enabled {
                    let outcome = manager.refresh_once(&app).await;
                    if let Err(err) = outcome {
                        log::warn!("小册子开服监控抓取失败: {}", err);
                    }
                } else {
                    // 停用后不再打站点，但“这份结论有多旧”仍在变 —— 照旧推给界面。
                    // 少了这一下，顶栏会永远挂着停用前那句「服务器正常（20 秒前）」，
                    // 把“监控已经关了”藏起来：那正是把没数据显示成数据很新的另一种写法。
                    manager.broadcast(&app);
                }

                let wait = manager.config().interval_sec.max(10);
                tokio::time::sleep(Duration::from_secs(wait)).await;
            }
        });
    }

    /// 采样一次，并把结论广播出去 / 必要时报警。
    ///
    /// 抓取失败**不清空**上一份结论：站点偶尔抖一下不代表服务器挂了，
    /// 把状态擦成「未知」反而会让用户以为出了事。失败只记在 `error` 里。
    pub async fn refresh_once(&self, app: &AppHandle) -> Result<(), String> {
        let body = match self.client.fetch_server_monitor().await {
            Ok(body) => body,
            Err(err) => {
                self.record_error(&err);
                self.broadcast(app);
                return Err(err);
            }
        };

        let feed = match parse_snapshot(&body) {
            Ok(feed) => feed,
            Err(err) => {
                self.record_error(&err);
                self.broadcast(app);
                return Err(err);
            }
        };
        let snapshot = &feed.snapshot;

        let now = chrono::Utc::now().timestamp();
        let previous = self.last_status.read().clone();

        let should_fire = feed_should_alert(&feed, previous.as_deref(), now);

        {
            // 锁序：**先 config、后 status**（见 struct 上那段说明）。
            //
            // 而且 config 的读锁要**一直持到 status 写完**，不是先 clone 再拿锁：
            // 先 clone 会出现「我们读到旧 config → update_config 把两把锁都改成新值 →
            // 我们再拿 status 写锁，用旧值把人家刚写好的 enabled/interval 盖回去」，
            // 而监控一旦被关掉就不再采样，那个错值再也没人纠正。持着读锁，
            // update_config 就只能排在本次状态写完之后，顺序天然对得上。
            // （读锁是共享的，两次采样撞车也互不阻塞；这里没有 await，不会跨着锁睡。）
            let config = self.config.read();
            let mut status = self.status.write();
            *status = MonitorStatus {
                enabled: config.enabled,
                interval_sec: config.interval_sec,
                status: snapshot.status.clone(),
                status_label: status_label(&snapshot.status),
                open: is_open(&snapshot.status),
                changed_at: local_time(snapshot.changed_at.unwrap_or(0)),
                checked_at: local_time(snapshot.checked_at),
                // 原始采样时刻：年龄的算术全靠它（`checked_at` 那个字符串是给人看的）
                checked_at_epoch: snapshot.checked_at,
                checked_ago_sec: None,
                snapshot_stale: false,
                official_label: official_label(
                    snapshot
                        .official
                        .as_ref()
                        .map(|official| official.status.as_str())
                        .unwrap_or("unknown"),
                ),
                official_detail: snapshot
                    .official
                    .as_ref()
                    .and_then(|official| official.detail.clone())
                    .unwrap_or_default(),
                login_label: login_label(
                    snapshot
                        .login
                        .as_ref()
                        .map(|login| login.status.as_str())
                        .unwrap_or("unknown"),
                ),
                login_detail: snapshot
                    .login
                    .as_ref()
                    .map(|login| {
                        // 站点有时只给名字不给 detail，那时候名字本身就是信息
                        if login.detail.trim().is_empty() {
                            login.name.clone()
                        } else {
                            login.detail.clone()
                        }
                    })
                    .unwrap_or_default(),
                history: snapshot
                    .history
                    .iter()
                    .map(|entry| HistoryPoint {
                        at: local_time(entry.at),
                        status: entry.status.clone(),
                        label: status_label(&entry.status),
                        open: is_open(&entry.status),
                        login_label: login_label(&entry.login),
                        official_label: official_label(&entry.official),
                    })
                    .collect(),
                error: None,
                source: "小册子 · 开服监控（tools/server-monitor/api.php）".to_string(),
            };
            // 年龄与「陈旧」不写死在字面量里：写死的值从采样那一刻起就不动了，
            // 而“这结论有多旧”必须每次读都变（`status()` 里也会再算一遍）。
            recompute_freshness(&mut status, now);
        }

        if self.last_error.read().is_some() {
            log::info!("小册子开服监控恢复正常");
        }
        *self.last_error.write() = None;
        *self.last_status.write() = Some(snapshot.status.clone());

        if feed.stale {
            log::warn!("小册子开服监控：站点标了 stale（采集器没跟上），本轮不报警");
        }

        if snapshot.status != previous.clone().unwrap_or_default() {
            log::info!(
                "小册子开服监控：{} → {}（检测于 {}，站点采集间隔 {} 秒）",
                previous.as_deref().map(status_label).unwrap_or_else(|| "（首次采样）".to_string()),
                status_label(&snapshot.status),
                local_time(snapshot.checked_at),
                feed.interval_sec.unwrap_or(0)
            );
        }

        self.broadcast(app);
        self.started.store(true, Ordering::Relaxed);

        if should_fire {
            fire_open_alert(
                app,
                AlertSource::Monitor,
                format!(
                    "小册子「开服监控」检测到{}（检测于 {}）。维护结束后第一件事就是登录抢位置。",
                    status_label(&snapshot.status),
                    local_time(snapshot.checked_at)
                ),
            );
        }

        Ok(())
    }

    fn record_error(&self, err: &str) {
        let first_time = {
            let mut slot = self.last_error.write();
            let changed = slot.as_deref() != Some(err);
            *slot = Some(err.to_string());
            changed
        };

        self.status.write().error = Some(err.to_string());

        if first_time {
            log::warn!("小册子开服监控暂时取不到数据: {}", err);
        }
    }

    fn broadcast(&self, app: &AppHandle) {
        let status = self.status();
        if let Err(err) = app.emit("server-monitor-update", &status) {
            log::debug!("推送开服监控状态失败: {}", err);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 2026-09-22 实测的真实响应（缩进过，字段一个没改）。
    const LIVE_SAMPLE: &str = r#"{
        "ok": true,
        "now": 1790046981,
        "snapshot": {
            "checked_at": 1790046961,
            "status": "login_recovered",
            "login": { "name": "登录入口", "status": "greeting", "detail": "握手正常" },
            "official": { "status": "unknown", "detail": "缓存标记维护（时效待确认）" },
            "event_id": 4,
            "notification_rule": 4,
            "opening_basis": null,
            "history": [
                { "at": 1789984081, "status": "login_recovered", "login": "greeting", "official": "unknown" },
                { "at": 1789984026, "status": "confirming", "login": "greeting", "official": "unknown" },
                { "at": 1789983965, "status": "unknown", "login": "unknown", "official": "unknown" }
            ],
            "collector": "scheduled-cli",
            "changed_at": 1789984081
        },
        "interval": 20,
        "stale": false
    }"#;

    #[test]
    fn parses_live_response() {
        let feed = parse_snapshot(LIVE_SAMPLE).expect("真实响应必须能解析");
        assert_eq!(feed.interval_sec, Some(20));
        assert!(!feed.stale);

        let snapshot = &feed.snapshot;
        assert_eq!(snapshot.status, "login_recovered");
        assert_eq!(snapshot.checked_at, 1790046961);
        assert_eq!(snapshot.changed_at, Some(1789984081));
        assert_eq!(snapshot.login.as_ref().unwrap().detail, "握手正常");
        assert_eq!(snapshot.history.len(), 3);
        // 历史的 login/official 是字符串，别按对象去解
        assert_eq!(snapshot.history[0].status, "login_recovered");
    }

    #[test]
    fn history_is_sorted_newest_first() {
        let body = r#"{"ok":true,"snapshot":{"status":"maintenance","history":[
            {"at":100,"status":"unavailable"},
            {"at":300,"status":"maintenance"},
            {"at":200,"status":"confirming"}
        ]}}"#;
        let feed = parse_snapshot(body).unwrap();
        let times: Vec<i64> = feed.snapshot.history.iter().map(|entry| entry.at).collect();
        assert_eq!(times, vec![300, 200, 100]);
    }

    #[test]
    fn tolerates_missing_optional_fields() {
        // 站点以后精简字段（或本次只是部分字段缺失）不该让整个功能炸掉
        let body = r#"{"ok":true,"snapshot":{"status":"maintenance"}}"#;
        let feed = parse_snapshot(body).unwrap();
        assert_eq!(feed.snapshot.status, "maintenance");
        assert!(feed.snapshot.login.is_none());
        assert!(feed.snapshot.history.is_empty());
        assert_eq!(feed.interval_sec, None);
    }

    #[test]
    fn reports_site_side_errors() {
        let err = parse_snapshot(r#"{"ok":false,"error":"collector offline"}"#).unwrap_err();
        assert!(err.contains("collector offline"), "实际错误：{}", err);

        // 压根不是 JSON（被拦截页 / 网关错误页）
        assert!(parse_snapshot("<html>502 Bad Gateway</html>").is_err());
        // 有 OK 但没有 snapshot
        assert!(parse_snapshot(r#"{"ok":true}"#).is_err());
    }

    #[test]
    fn status_labels_match_the_site_wording() {
        assert_eq!(status_label("login_recovered"), "服务器正常");
        assert_eq!(status_label("maintenance"), "服务器维护中");
        assert_eq!(status_label("confirming"), "正在确认状态");
        assert_eq!(status_label("unavailable"), "状态暂不可用");
        assert_eq!(status_label("unknown"), "状态未知");
        // 站点将来加的新状态码原样带出来，不要混进「状态未知」
        assert_eq!(status_label("teapot"), "未知状态（teapot）");
    }

    #[test]
    fn only_login_recovered_counts_as_open() {
        assert!(is_open("login_recovered"));
        // 「正在确认状态」不是开服：这时候喊开服，维护一延长就误报，
        // 而误报一次就会让人开始不信这个提醒
        assert!(!is_open("confirming"));
        assert!(!is_open("maintenance"));
        assert!(!is_open("unavailable"));
    }

    #[test]
    fn alerts_only_on_transition_into_open() {
        let now = 1_790_046_981;

        // 维护中 → 正常：正是要报的那一次
        assert!(should_alert(
            Some("maintenance"),
            "login_recovered",
            None,
            now - 20,
            now
        ));
        // 不可用 → 正在确认 → 正常，两步都会报吗？确认阶段不报，只报最后那步
        assert!(!should_alert(
            Some("unavailable"),
            "confirming",
            None,
            now - 20,
            now
        ));
        // 一直正常 → 一直正常：不要反复响
        assert!(!should_alert(
            Some("login_recovered"),
            "login_recovered",
            None,
            now - 20,
            now
        ));
        // 正常 → 维护中：不报（开服提醒只管开服）
        assert!(!should_alert(
            Some("login_recovered"),
            "maintenance",
            None,
            now - 20,
            now
        ));
    }

    #[test]
    fn first_sample_only_alerts_when_the_recovery_is_fresh() {
        let now = 1_790_046_981;

        // 打开工具箱时服务器本来就开着（几小时前就正常了）：不报，避免「一开就喊开服」
        assert!(!should_alert(
            None,
            "login_recovered",
            Some(now - 6 * 3600),
            now - 20,
            now
        ));

        // 维护刚结束，用户马上打开工具箱：这就是他在等的那一刻，报
        assert!(should_alert(
            None,
            "login_recovered",
            Some(now - 90),
            now - 20,
            now
        ));

        // 站点没给变更时刻：宁可少报也不要误报
        assert!(!should_alert(None, "login_recovered", None, now - 20, now));
    }

    #[test]
    fn stale_feeds_never_alert() {
        let now = 1_790_046_981;
        // 站点自己标了 stale：它的采集器没跟上，结论不一定还成立
        let feed = MonitorFeed {
            snapshot: Snapshot {
                checked_at: now - 20,
                status: "login_recovered".to_string(),
                login: None,
                official: None,
                changed_at: Some(now - 30),
                history: Vec::new(),
            },
            stale: true,
            interval_sec: Some(20),
        };
        assert!(!feed_should_alert(&feed, Some("maintenance"), now));

        // 同一份快照只要不带 stale，就该照报
        let fresh = MonitorFeed {
            stale: false,
            ..feed
        };
        assert!(feed_should_alert(&fresh, Some("maintenance"), now));
    }

    #[test]
    fn stale_samples_never_alert() {
        let now = 1_790_046_981;
        // 站点采集器卡住了（采样时刻是 20 分钟前），即使状态是「正常」也不报
        assert!(!should_alert(
            Some("maintenance"),
            "login_recovered",
            None,
            now - 1200,
            now
        ));
    }

    /// 「这结论有多旧」必须跟着时钟走，而不是采样那一刻算好就冻结。
    ///
    /// 冻结的后果就是评审里那条 P1：断网一小时，界面还写着「检测于 11:29:01
    /// （20 秒前）」—— 把没数据显示成数据很新。
    #[test]
    fn freshness_follows_the_clock_instead_of_freezing_at_sample_time() {
        let mut status = MonitorStatus::initial(true, 30);
        let sampled_at = 1_790_046_961i64;
        status.status = "login_recovered".to_string();
        status.status_label = "服务器正常".to_string();
        status.open = true;
        status.checked_at_epoch = sampled_at;

        // 采样刚完成：10 秒前、新鲜
        recompute_freshness(&mut status, sampled_at + 10);
        assert_eq!(status.checked_ago_sec, Some(10));
        assert!(!status.snapshot_stale);

        // 一小时后（期间一次都没采到）：年龄如实变大，而且早就该算陈旧
        recompute_freshness(&mut status, sampled_at + 3600);
        assert_eq!(status.checked_ago_sec, Some(3600));
        assert!(status.snapshot_stale);

        // 但**结论一个字都没动** —— README：抓取失败不清空上一份结论
        assert_eq!(status.status, "login_recovered");
        assert_eq!(status.status_label, "服务器正常");
        assert!(status.open);
    }

    /// 一次都没采到：没有年龄可报，也绝不能被当成“有一份新鲜结论”。
    #[test]
    fn never_sampled_has_no_age_and_counts_as_stale() {
        let mut status = MonitorStatus::initial(true, 30);
        recompute_freshness(&mut status, 1_000_000);
        assert_eq!(status.checked_ago_sec, None);
        assert!(status.snapshot_stale);
        assert_eq!(status.status_label, "还没拿到数据");
    }

    /// 停用监控：旧结论照旧保留（站点怎么说还怎么说），但必须立刻标成
    ///「别当实时」—— 否则停用之后顶栏永远绿着、写「20 秒前」。
    #[test]
    fn disabling_the_monitor_marks_the_frozen_snapshot_stale() {
        let mut status = MonitorStatus::initial(false, 30);
        status.status = "login_recovered".to_string();
        status.open = true;
        status.checked_at_epoch = 1_000_000;

        recompute_freshness(&mut status, 1_000_000 + 5);
        // 年龄照实说：刚停用时那份数据确实才 5 秒前采的
        assert_eq!(status.checked_ago_sec, Some(5));
        assert!(
            status.snapshot_stale,
            "监控已经不采样了，这份结论就是停在那儿的"
        );
        assert_eq!(status.status, "login_recovered");
    }

    /// 走 `status()`（命令与广播都走它）也要拿到活的年龄 —— 只在采样时算的话，
    /// 广播出去的永远是采样那一刻的数字，界面再怎么渲染也救不回来。
    #[test]
    fn status_getter_reports_the_real_age_of_the_last_sample() {
        let manager = MonitorManager::new(
            Arc::new(MxdcClient::new()),
            MonitorConfig {
                enabled: true,
                interval_sec: 30,
            },
        );
        {
            let mut status = manager.status.write();
            status.status = "maintenance".to_string();
            status.status_label = status_label("maintenance").to_string();
            status.checked_at_epoch = chrono::Utc::now().timestamp() - 7200;
        }

        let shown = manager.status();
        assert!(
            matches!(shown.checked_ago_sec, Some(7200) | Some(7201)),
            "年龄应当是两小时前，实际 {:?}",
            shown.checked_ago_sec
        );
        assert!(shown.snapshot_stale);
        // 旧结论原样保留：老化只改「有多旧」，不改「站点怎么说」
        assert_eq!(shown.status_label, "服务器维护中");
    }
}
