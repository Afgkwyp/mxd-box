//! 当前频道（几线）—— 从游戏进程的 TCP 连接反推。
//!
//! # 为什么是网络而不是画面
//!
//! 怀旧服的当前频道只在「切换频道」窗口里显示，磁盘、注册表和日志里都不写
//! （2026-09-28 把安装目录、LocalLow、注册表、启动器日志、SDK 日志和几个
//! 第三方工具都翻过一遍，零命中）。但它是**一条线一个服务端端口**：
//! 换线会断掉旧连接、连到新端口，端口跟着线号 +1。
//!
//! # 服务器农场：实测录入的对照表
//!
//! **每台机器固定负责 3 条线**：端口 8585/8586/8587 就是组内第 1/2/3 条。
//! 每个区服用哪一段机器、以及段内怎么排，是部署出来的，只能实测录入：
//!
//! * **绿水灵**：`.76`–`.95`，1–3 线在 `.76`、58–60 线在 `.95`
//!   （2026-09-28 现场扫描全部 60 条线，跨天复核）
//! * **蓝蜗牛**：`.27`–`.48` 里去掉 `.31`、`.39` 两台（这两台不在这个服的
//!   列表里），1–3 线在 `.27`、58–60 线在 `.48`
//! * **蘑菇仔**：`.49`–`.75` 里去掉 `.51`–`.54`、`.58`、`.60`、`.61` 七台，
//!   1–3 线在 `.49`、58–60 线在 `.75`
//! * **漂漂猪**：`.140`–`.165` 里去掉 `.143`、`.146`、`.148`、`.154`、`.155`、`.159`
//!   六台，1–3 线在 `.140`、58–60 线在 `.165`
//! * **小白兔**：`.166`–`.187` 里去掉 `.171`、`.185` 两台，1–3 线在 `.166`、
//!   58–60 线在 `.187`
//!
//! 五个区服的机器号互不重叠（整片机器按区服切开分，段内的洞是别的东西在用 ——
//! 我们不需要知道是什么，实测录入即可）。**每张表都是现场扫点核对过的**：
//! 扫点时要一条一条按顺序切，漏切会让对号整体错位（小白兔就踩过一次，
//! 靠穷举反解才发现）。
//!
//! 具体表见 [`FARMS`] / [`channel_from_farm`]。形状对不上的区服一律返回
//! 「未知」，绝不硬猜（宁可让用户点一下，也不给一个错数字）。
//!
//! # 兜底：按节点学基准
//!
//! 没录入的区服/以后换了部署时，退回「按节点学一次」：第一次连到某个节点让
//! 用户点一下真实线号（`set_manual`），基准记成 `端口 − 线号` 存进设置
//! （`channel_host_bases`，JSON：地址 → 基准），之后那个节点自动。
//! 手动校准过的基准优先于农场表（手动永远最大）。
//!
//! # 上次在哪条线
//!
//! 掉线重登以后经常想不起来掉线前在哪条线。所以每次「离开一条线」都记一笔
//! （[`ChannelVisit`]，最多 [`RECENT_LIMIT`] 条，存在设置 `channel_recent` 里）：
//! 直接换线走的记 `dropped = false`，连接断了（掉线 / 下线 / 关游戏）之后才到
//! 别的线的记 `dropped = true`。回到列表里的某条线时把它从列表里拿掉 ——
//! 列表永远是「除了现在这条以外，最近待过的」。
//!
//! 整条链路不读游戏内存、不截屏、不注入：只是查系统 TCP 表里的连接。

use crate::db::Database;
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tauri::{AppHandle, Emitter};
use tokio::time::{sleep, Duration};

pub const MIN_CHANNEL: u32 = 1;
pub const MAX_CHANNEL: u32 = 60;

/// 「当前几线」推给界面的载荷。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChannelState {
    /// `None` = 现在不知道（游戏没开 / 这条节点还没校准）
    pub channel: Option<u32>,
    /// 区服名（绿水灵 / 蓝蜗牛 …）：节点在已录入的农场表里才有
    pub server: Option<String>,
    /// `auto` 从连接读到的 / `manual` 手动定的 / `uncalibrated` 节点还没校准 /
    /// `stale` 上次记录（游戏没开）
    pub source: String,
    /// 远端服务器地址（校准就是按它记的）
    pub host: Option<String>,
    /// 远端端口；`线号 = 端口 − base`
    pub port: Option<u16>,
    /// 这个节点学到的基准；`None` = 还没校准
    pub base: Option<i32>,
    /// 这条线最后一次确认还连着的时间（`stale` 时就是发现连接没了的时刻）
    pub updated_unix: i64,
    /// 最近待过的线（新的在前，不含现在这条）—— 掉线后回想「上次在哪条线」用
    pub recent: Vec<ChannelVisit>,
}

/// 待过、已经离开的一条线。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChannelVisit {
    pub server: Option<String>,
    pub channel: u32,
    /// 最后一次看到还在这条线的时间
    pub last_seen_unix: i64,
    /// `true` = 连接断了以后才离开的（掉线 / 下线 / 关游戏）；`false` = 直接换线走的
    pub dropped: bool,
}

impl Default for ChannelState {
    fn default() -> Self {
        Self {
            channel: None,
            server: None,
            source: "stale".to_string(),
            host: None,
            port: None,
            base: None,
            updated_unix: 0,
            recent: Vec::new(),
        }
    }
}

impl ChannelState {
    fn new(
        channel: Option<u32>,
        source: &str,
        host: Option<String>,
        port: Option<u16>,
        base: Option<i32>,
        updated_unix: i64,
    ) -> Self {
        // 区服名跟着节点走：认得出的节点一律带上（哪怕线号是手动校准的）
        let server = host.as_deref().and_then(farm_name).map(str::to_string);
        Self {
            channel,
            server,
            source: source.to_string(),
            host,
            port,
            base,
            updated_unix,
            recent: Vec::new(),
        }
    }
}

pub const HOST_BASES_KEY: &str = "channel_host_bases";
pub const LAST_KEY: &str = "channel_last";
pub const LAST_UNIX_KEY: &str = "channel_last_unix";
pub const LAST_SERVER_KEY: &str = "channel_last_server";
pub const RECENT_KEY: &str = "channel_recent";
/// 「最近待过的线」留几条。掉线重登后手滑多换了一两次线也还找得回来。
pub const RECENT_LIMIT: usize = 5;
pub const EVENT_NAME: &str = "channel-update";

/// 探测间隔。换线要断连重连，3 秒一轮足够快，也几乎不花什么。
const POLL_INTERVAL: Duration = Duration::from_secs(3);
/// 连续几轮找不到连接才把来源标成「旧值」（换线中间会短暂断连，别闪）。
const MISSES_BEFORE_STALE: u32 = 2;

// ---------------------------------------------------------------------------
// 纯逻辑：端口 ⇄ 线号、按节点记的基准
// ---------------------------------------------------------------------------

/// 端口换算成线号；落在 1–60 之外就认为它不是频道连接。
pub fn channel_from_port(port: u16, base: i32) -> Option<u32> {
    let channel = port as i32 - base;
    if (MIN_CHANNEL as i32..=MAX_CHANNEL as i32).contains(&channel) {
        Some(channel as u32)
    } else {
        None
    }
}

/// 实测录入的服务器农场。
///
/// **每组 = 3 条线**，端口 8585/8586/8587 对应组内第 1/2/3 条；`octets` 是
/// 每一组的机器末段，**按组序排列**（机器号不一定连续 —— 蓝蜗牛的列表里
/// `.31`、`.39` 两台就不属于这个服）。
///
/// 录入方法：换线扫一遍（每个组取一条线），人工核对后加到这里；
/// 形状对不上的区服会走「线号未知 → 点一下校准」的兜底，绝不硬猜。
struct Farm {
    name: &'static str,
    octets: &'static [u8],
}

const FARMS: &[Farm] = &[
    // 绿水灵：1–3 线在 .76、58–60 线在 .95（2026-09-28 全量扫描，跨天复核）
    Farm {
        name: "绿水灵",
        octets: &[
            76, 77, 78, 79, 80, 81, 82, 83, 84, 85, 86, 87, 88, 89, 90, 91, 92, 93, 94, 95,
        ],
    },
    // 蓝蜗牛：1–3 线在 .27、58–60 线在 .48；中间 .31/.39 两台不在这个服的列表里
    Farm {
        name: "蓝蜗牛",
        octets: &[
            27, 28, 29, 30, 32, 33, 34, 35, 36, 37, 38, 40, 41, 42, 43, 44, 45, 46, 47, 48,
        ],
    },
    // 蘑菇仔：1–3 线在 .49、58–60 线在 .75；.51–.54/.58/.60/.61 七台不在列表里
    Farm {
        name: "蘑菇仔",
        octets: &[
            49, 50, 55, 56, 57, 59, 62, 63, 64, 65, 66, 67, 68, 69, 70, 71, 72, 73, 74, 75,
        ],
    },
    // 漂漂猪：1–3 线在 .140、58–60 线在 .165；.143/.146/.148/.154/.155/.159 六台不在列表里
    Farm {
        name: "漂漂猪",
        octets: &[
            140, 141, 142, 144, 145, 147, 149, 150, 151, 152, 153, 156, 157, 158, 160, 161, 162,
            163, 164, 165,
        ],
    },
    // 小白兔：1–3 线在 .166、58–60 线在 .187；.171/.185 两台不在列表里
    Farm {
        name: "小白兔",
        octets: &[
            166, 167, 168, 169, 170, 172, 173, 174, 175, 176, 177, 178, 179, 180, 181, 182, 183,
            184, 186, 187,
        ],
    },
];

/// 这台机器属于哪个农场、是第几组（从 0 数）。
fn farm_slot(host: &str) -> Option<(&'static Farm, usize)> {
    let last: u8 = host.rsplit('.').next()?.trim().parse().ok()?;
    FARMS.iter().find_map(|farm| {
        farm.octets
            .iter()
            .position(|octet| *octet == last)
            .map(|index| (farm, index))
    })
}

/// 这台机器是哪个区服的（只看机器，不看端口）—— 界面上「绿水灵 · 47线」的前半截。
pub fn farm_name(host: &str) -> Option<&'static str> {
    farm_slot(host).map(|(farm, _)| farm.name)
}

/// 命中的农场（名字 + 线号）—— `auto` 状态就是靠它。
pub fn farm_hit(host: &str, port: u16) -> Option<(&'static str, u32)> {
    if !(8585..=8587).contains(&port) {
        return None;
    }
    let (farm, index) = farm_slot(host)?;
    let channel = index as u32 * 3 + (port - 8584) as u32;
    (MIN_CHANNEL..=MAX_CHANNEL)
        .contains(&channel)
        .then_some((farm.name, channel))
}

/// 主机末段 + 端口 → 线号：在已知的服务器农场里查。
///
/// 认不出形状（端口不是 8585–8587、或末段不在任何已录入的列表里）就返回 `None`，
/// 交给按节点学习的基准兜底。
pub fn channel_from_farm(host: &str, port: u16) -> Option<u32> {
    farm_hit(host, port).map(|(_, channel)| channel)
}

/// 这台连接是几线：先看这台节点有没有手动校准过的基准（手动优先），
/// 没有就用服务器农场的固定规律算。
pub fn channel_for(host: &str, port: u16, bases: &HashMap<String, i32>) -> Option<u32> {
    if let Some(base) = bases.get(host) {
        if let Some(channel) = channel_from_port(port, *base) {
            return Some(channel);
        }
    }
    channel_from_farm(host, port)
}

/// 每个服务器节点学到的基准（地址 → 基准）。
pub fn host_bases(db: &Database) -> HashMap<String, i32> {
    db.get_setting(HOST_BASES_KEY)
        .and_then(|raw| serde_json::from_str::<HashMap<String, i32>>(&raw).ok())
        .unwrap_or_default()
}

fn save_host_base(db: &Database, host: &str, base: i32) {
    let mut bases = host_bases(db);
    bases.insert(host.to_string(), base);
    match serde_json::to_string(&bases) {
        Ok(json) => {
            if let Err(err) = db.set_setting(HOST_BASES_KEY, &json) {
                log::warn!("保存频道基准失败：{err}");
            }
        }
        Err(err) => log::warn!("序列化频道基准失败：{err}"),
    }
}

/// 是不是同一条线。有一边不知道区服（旧版本存下来的记录没有区服名）就只比线号。
fn same_line(
    a_server: Option<&str>,
    a_channel: u32,
    b_server: Option<&str>,
    b_channel: u32,
) -> bool {
    a_channel == b_channel
        && match (a_server, b_server) {
            (Some(a), Some(b)) => a == b,
            _ => true,
        }
}

/// 把「现在这条线」从列表里拿掉（回到待过的线时用）。
fn forget_line(recent: &mut Vec<ChannelVisit>, server: Option<&str>, channel: u32) {
    recent.retain(|visit| !same_line(visit.server.as_deref(), visit.channel, server, channel));
}

/// 记一笔「离开了这条线」：同一条线只留最新的一笔，超出 [`RECENT_LIMIT`] 丢最旧的。
fn remember_visit(recent: &mut Vec<ChannelVisit>, visit: ChannelVisit) {
    forget_line(recent, visit.server.as_deref(), visit.channel);
    recent.insert(0, visit);
    recent.truncate(RECENT_LIMIT);
}

/// 离开 `previous` 那条线时该记的一笔；之前本来就不知道在哪条线就没有可记的。
///
/// `dropped`：调用方明确知道连接是断了；之前已经标成 `stale` 的也算断了。
fn visit_on_leave(previous: &ChannelState, dropped: bool, now: i64) -> Option<ChannelVisit> {
    let stale = previous.source == "stale";
    Some(ChannelVisit {
        server: previous.server.clone(),
        channel: previous.channel?,
        // 断线的时间在标 `stale` 那一刻已经记下了；直接换线的就是现在
        last_seen_unix: if stale { previous.updated_unix } else { now },
        dropped: dropped || stale,
    })
}

/// 到了一条认得出的线之后的「最近待过」列表。
pub fn recent_after_arrival(
    previous: &ChannelState,
    server: Option<&str>,
    channel: u32,
    now: i64,
) -> Vec<ChannelVisit> {
    let mut recent = previous.recent.clone();
    if let Some(visit) = visit_on_leave(previous, false, now) {
        if !same_line(visit.server.as_deref(), visit.channel, server, channel) {
            remember_visit(&mut recent, visit);
        }
    }
    forget_line(&mut recent, server, channel);
    recent
}

/// 连接换成了认不出线号的节点（登录服 / 没录入的区服）之后的列表：
/// 原来那条线算「断开后离开」。
pub fn recent_after_loss(previous: &ChannelState, now: i64) -> Vec<ChannelVisit> {
    let mut recent = previous.recent.clone();
    if let Some(visit) = visit_on_leave(previous, true, now) {
        remember_visit(&mut recent, visit);
    }
    recent
}

fn load_recent(db: &Database) -> Vec<ChannelVisit> {
    let mut recent = db
        .get_setting(RECENT_KEY)
        .and_then(|raw| serde_json::from_str::<Vec<ChannelVisit>>(&raw).ok())
        .unwrap_or_default();
    recent.retain(|visit| (MIN_CHANNEL..=MAX_CHANNEL).contains(&visit.channel));
    recent.truncate(RECENT_LIMIT);
    recent
}

fn save_recent(db: &Database, recent: &[ChannelVisit]) {
    match serde_json::to_string(recent) {
        Ok(json) => {
            if let Err(err) = db.set_setting(RECENT_KEY, &json) {
                log::warn!("保存最近待过的线失败：{err}");
            }
        }
        Err(err) => log::warn!("序列化最近待过的线失败：{err}"),
    }
}

/// 网络序存在 u32 里的 IPv4 → "a.b.c.d"。
///
/// `MIB_TCPROW_OWNER_PID.dwRemoteAddr` 是网络序的 4 个字节，小端机上读进 u32 后
/// **低 8 位就是第一段**。
pub fn ipv4_string(raw: u32) -> String {
    format!(
        "{}.{}.{}.{}",
        raw & 0xFF,
        (raw >> 8) & 0xFF,
        (raw >> 16) & 0xFF,
        (raw >> 24) & 0xFF
    )
}

// ---------------------------------------------------------------------------
// Windows：查游戏进程的 ESTABLISHED TCP 连接
// ---------------------------------------------------------------------------

#[cfg(windows)]
mod win {
    use std::ffi::c_void;

    const AF_INET: u32 = 2;
    /// TCP_TABLE_OWNER_PID_ALL：带进程号的完整 IPv4 TCP 表。
    const TCP_TABLE_OWNER_PID_ALL: u32 = 5;
    /// MIB_TCP_STATE_ESTAB
    const TCP_STATE_ESTABLISHED: u32 = 5;
    const ERROR_INSUFFICIENT_BUFFER: u32 = 122;

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct TcpRowOwnerPid {
        state: u32,
        local_addr: u32,
        local_port: u32,
        remote_addr: u32,
        remote_port: u32,
        owning_pid: u32,
    }

    #[link(name = "iphlpapi")]
    extern "system" {
        fn GetExtendedTcpTable(
            table: *mut c_void,
            size: *mut u32,
            order: i32,
            af: u32,
            table_class: u32,
            reserved: u32,
        ) -> u32;
    }

    #[link(name = "user32")]
    extern "system" {
        fn GetWindowThreadProcessId(hwnd: isize, pid: *mut u32) -> u32;
    }

    /// 窗口属于哪个进程。
    pub fn window_process_id(hwnd: isize) -> Option<u32> {
        let mut pid = 0u32;
        let thread = unsafe { GetWindowThreadProcessId(hwnd, &mut pid) };
        if thread == 0 || pid == 0 {
            None
        } else {
            Some(pid)
        }
    }

    /// 某个进程所有「已连接」的 IPv4 远端地址与端口（回环不算）。
    pub fn established_remotes(pid: u32) -> Vec<(u32, u16)> {
        // 第一次问长度：表会被并发改，所以第二次可能还要再要一次长度。
        let mut size = 0u32;
        let first = unsafe {
            GetExtendedTcpTable(
                std::ptr::null_mut(),
                &mut size,
                0,
                AF_INET,
                TCP_TABLE_OWNER_PID_ALL,
                0,
            )
        };
        if first != ERROR_INSUFFICIENT_BUFFER || size == 0 {
            return Vec::new();
        }

        // 用 u32 缓冲保证 4 字节对齐：表头是一个 u32 条数，后面每行 6 个 u32。
        let mut buffer = vec![0u32; (size as usize).div_ceil(4)];
        let result = unsafe {
            GetExtendedTcpTable(
                buffer.as_mut_ptr() as *mut c_void,
                &mut size,
                0,
                AF_INET,
                TCP_TABLE_OWNER_PID_ALL,
                0,
            )
        };
        if result != 0 {
            return Vec::new();
        }

        let rows = buffer[0] as usize;
        let table = unsafe {
            std::slice::from_raw_parts(buffer.as_ptr().add(1) as *const TcpRowOwnerPid, rows)
        };
        let mut remotes = Vec::new();
        for row in table {
            if row.owning_pid != pid || row.state != TCP_STATE_ESTABLISHED {
                continue;
            }
            // 回环（127.x）不是游戏服务器；地址按网络序存在 u32 里，低 8 位就是首段。
            if row.remote_addr & 0xFF == 127 {
                continue;
            }
            let port = u16::from_be((row.remote_port & 0xFFFF) as u16);
            if port != 0 {
                remotes.push((row.remote_addr, port));
            }
        }
        remotes
    }
}

#[cfg(not(windows))]
mod win {
    pub fn window_process_id(_hwnd: isize) -> Option<u32> {
        None
    }

    pub fn established_remotes(_pid: u32) -> Vec<(u32, u16)> {
        Vec::new()
    }
}

/// 游戏进程当前所有 ESTABLISHED 远端（地址字符串, 端口）；游戏没开就是空。
pub fn current_game_remotes() -> Vec<(String, u16)> {
    let Some(window) = crate::exp::capture::find_game_window() else {
        return Vec::new();
    };
    let Some(pid) = win::window_process_id(window.hwnd) else {
        return Vec::new();
    };
    win::established_remotes(pid)
        .into_iter()
        .map(|(addr, port)| (ipv4_string(addr), port))
        .collect()
}

// ---------------------------------------------------------------------------
// 探测循环
// ---------------------------------------------------------------------------

enum Detection {
    /// 换算出线号（基准来自手动校准，或服务器农场的固定规律）
    Known {
        host: String,
        port: u16,
        channel: u32,
        base: Option<i32>,
    },
    /// 只看到一条连接，但这个节点的形状认不出来、也没校准过
    Uncalibrated { host: String, port: u16 },
    /// 没看到游戏连接
    Missing,
}

/// 「当前几线」的状态机：每 3 秒查一次连接，变了就落库 + 广播。
pub struct ChannelWatcher {
    state: RwLock<ChannelState>,
    misses: RwLock<u32>,
}

impl ChannelWatcher {
    /// 从数据库读上次的记录：程序刚启动、游戏还没开时先显示旧值（标 `stale`）。
    pub fn new(db: &Database) -> Self {
        let channel = db
            .get_setting(LAST_KEY)
            .and_then(|value| value.trim().parse::<u32>().ok())
            .filter(|channel| (MIN_CHANNEL..=MAX_CHANNEL).contains(channel));
        let updated_unix = db
            .get_setting(LAST_UNIX_KEY)
            .and_then(|value| value.trim().parse::<i64>().ok())
            .unwrap_or(0);
        let mut state = ChannelState::new(channel, "stale", None, None, None, updated_unix);
        // 没有节点地址可查，区服名用上次存下来的
        state.server = db
            .get_setting(LAST_SERVER_KEY)
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty());
        state.recent = load_recent(db);
        if let Some(channel) = state.channel {
            forget_line(&mut state.recent, state.server.as_deref(), channel);
        }
        Self {
            state: RwLock::new(state),
            misses: RwLock::new(0),
        }
    }

    pub fn state(&self) -> ChannelState {
        self.state.read().clone()
    }

    /// 手动定一次线号：把这台节点的基准记成 `端口 − 线号`，之后这台永久自动。
    ///
    /// 看不到游戏连接就不动（没有端口可校准，硬记一个值只会误导）。
    pub fn set_manual(&self, db: &Database, channel: u32) -> Result<ChannelState, String> {
        if !(MIN_CHANNEL..=MAX_CHANNEL).contains(&channel) {
            return Err(format!("线号要在 {MIN_CHANNEL}–{MAX_CHANNEL} 之间"));
        }
        let (host, port) = self.anchor(db).ok_or_else(|| {
            "现在看不到游戏连接：先进游戏、进角色，再点一次".to_string()
        })?;
        let base = port as i32 - channel as i32;
        if !(1_000..=65_000).contains(&base) {
            return Err(format!("端口 {port} 和 {channel} 线对不上，先确认游戏里显示的线号"));
        }
        save_host_base(db, &host, base);
        log::info!("校准频道基准：{host} → {base}（{channel} 线 = 端口 {port}）");
        let mut state = ChannelState::new(
            Some(channel),
            "manual",
            Some(host),
            Some(port),
            Some(base),
            chrono::Utc::now().timestamp(),
        );
        // 纠正线号不算换线：原来显示的那个数是认错的，不记进「待过的线」
        let previous = self.state.read().clone();
        state.recent = previous.recent.clone();
        forget_line(&mut state.recent, state.server.as_deref(), channel);
        if state.recent != previous.recent {
            save_recent(db, &state.recent);
        }
        *self.state.write() = state.clone();
        *self.misses.write() = 0;
        save(db, &state);
        Ok(state)
    }

    /// 现在拿哪条连接来校准：
    /// 唯一一条就直接用；多条时挑「能算出合法线号」的那条（校准过的基准或农场规律），
    /// 否则认输。
    fn anchor(&self, db: &Database) -> Option<(String, u16)> {
        let remotes = current_game_remotes();
        if remotes.len() == 1 {
            return remotes.into_iter().next();
        }
        let bases = host_bases(db);
        remotes
            .into_iter()
            .find(|(host, port)| channel_for(host, *port, &bases).is_some())
    }

    /// 采样循环：读连接 → 换算 → 变了就广播。
    pub fn start_loop(self: &Arc<Self>, app: AppHandle, db: Arc<Database>) {
        let watcher = Arc::clone(self);
        tauri::async_runtime::spawn(async move {
            loop {
                watcher.tick(&app, &db);
                sleep(POLL_INTERVAL).await;
            }
        });
    }

    fn detect(&self, db: &Database) -> Detection {
        let bases = host_bases(db);
        let remotes = current_game_remotes();
        for (host, port) in &remotes {
            if let Some(channel) = channel_for(host, *port, &bases) {
                return Detection::Known {
                    host: host.clone(),
                    port: *port,
                    channel,
                    base: bases.get(host).copied(),
                };
            }
        }
        if remotes.len() == 1 {
            let (host, port) = &remotes[0];
            return Detection::Uncalibrated {
                host: host.clone(),
                port: *port,
            };
        }
        Detection::Missing
    }

    fn tick(&self, app: &AppHandle, db: &Database) {
        let now = chrono::Utc::now().timestamp();
        match self.detect(db) {
            Detection::Known {
                host,
                port,
                channel,
                base,
            } => {
                *self.misses.write() = 0;
                let previous = self.state.read().clone();
                if previous.channel == Some(channel)
                    && previous.source == "auto"
                    && previous.host.as_deref() == Some(host.as_str())
                    && previous.port == Some(port)
                {
                    return;
                }
                let mut state = ChannelState::new(
                    Some(channel),
                    "auto",
                    Some(host.clone()),
                    Some(port),
                    base,
                    now,
                );
                state.recent =
                    recent_after_arrival(&previous, state.server.as_deref(), channel, now);
                if state.recent != previous.recent {
                    save_recent(db, &state.recent);
                }
                *self.state.write() = state.clone();
                save(db, &state);
                let via = state
                    .server
                    .as_deref()
                    .map(|name| format!(" · {name}"))
                    .unwrap_or_default();
                log::info!("当前频道：{channel} 线（{host}:{port}{via}）");
                if let Err(err) = app.emit(EVENT_NAME, &state) {
                    log::warn!("广播频道变化失败：{err}");
                }
            }
            Detection::Uncalibrated { host, port } => {
                *self.misses.write() = 0;
                let previous = self.state.read().clone();
                if previous.source == "uncalibrated"
                    && previous.host.as_deref() == Some(host.as_str())
                    && previous.port == Some(port)
                {
                    return;
                }
                let mut state =
                    ChannelState::new(None, "uncalibrated", Some(host.clone()), Some(port), None, now);
                // 原来那条线到此为止：记进「最近待过」，不然重登后就找不回来了
                state.recent = recent_after_loss(&previous, now);
                if state.recent != previous.recent {
                    save_recent(db, &state.recent);
                }
                *self.state.write() = state.clone();
                log::info!("看到服务器 {host}:{port} —— 这个节点还没校准过");
                if let Err(err) = app.emit(EVENT_NAME, &state) {
                    log::warn!("广播频道变化失败：{err}");
                }
            }
            Detection::Missing => {
                // 游戏没开 / 连接还没建立：短暂断连别闪，连续错过几轮才标旧值。
                let mut misses = self.misses.write();
                *misses = misses.saturating_add(1);
                let should_mark =
                    *misses == MISSES_BEFORE_STALE && self.state.read().source != "stale";
                drop(misses);
                if should_mark {
                    let mut state = self.state.read().clone();
                    state.source = "stale".to_string();
                    // 连接没了：节点信息作废；线号和区服留着当「上次在哪条线」
                    state.host = None;
                    state.port = None;
                    state.base = None;
                    state.updated_unix = now;
                    *self.state.write() = state.clone();
                    save(db, &state);
                    if let Err(err) = app.emit(EVENT_NAME, &state) {
                        log::warn!("广播频道变化失败：{err}");
                    }
                }
            }
        }
    }
}

fn save(db: &Database, state: &ChannelState) {
    if let Some(channel) = state.channel {
        if let Err(err) = db.set_setting(LAST_KEY, &channel.to_string()) {
            log::warn!("保存当前线号失败：{err}");
        }
        if let Err(err) = db.set_setting(LAST_UNIX_KEY, &state.updated_unix.to_string()) {
            log::warn!("保存当前线号时间失败：{err}");
        }
        let server = state.server.as_deref().unwrap_or("");
        if let Err(err) = db.set_setting(LAST_SERVER_KEY, server) {
            log::warn!("保存当前区服失败：{err}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Database;

    fn temp_db(tag: &str) -> (Database, std::path::PathBuf) {
        let path = std::env::temp_dir().join(format!(
            "mxdbox-test-channel-{tag}-{}.db",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);
        let db = Database::new(path.clone()).expect("建库失败");
        (db, path)
    }

    #[test]
    fn farm_table_maps_all_recorded_servers() {
        // 绿水灵（2026-09-28 全量扫描 + 跨天复核）
        assert_eq!(channel_from_farm("43.142.194.77", 8585), Some(4));
        assert_eq!(channel_from_farm("43.142.194.77", 8586), Some(5));
        assert_eq!(channel_from_farm("43.142.194.77", 8587), Some(6));
        assert_eq!(channel_from_farm("43.142.194.78", 8585), Some(7));
        assert_eq!(channel_from_farm("43.142.194.79", 8585), Some(10));
        assert_eq!(channel_from_farm("43.142.194.82", 8586), Some(20));
        assert_eq!(channel_from_farm("43.142.194.85", 8587), Some(30));
        assert_eq!(channel_from_farm("43.142.194.89", 8585), Some(40));
        assert_eq!(channel_from_farm("43.142.194.92", 8586), Some(50));
        assert_eq!(channel_from_farm("43.142.194.95", 8587), Some(60));
        assert_eq!(channel_from_farm("43.142.194.91", 8586), Some(47));
        assert_eq!(channel_from_farm("43.142.194.91", 8587), Some(48));

        // 蓝蜗牛（现场扫了 11 个点，含 .31/.39 两处跳号）
        assert_eq!(channel_from_farm("43.142.194.27", 8585), Some(1));
        assert_eq!(channel_from_farm("43.142.194.27", 8587), Some(3));
        assert_eq!(channel_from_farm("43.142.194.28", 8585), Some(4));
        assert_eq!(channel_from_farm("43.142.194.29", 8587), Some(9));
        assert_eq!(channel_from_farm("43.142.194.30", 8585), Some(10));
        assert_eq!(channel_from_farm("43.142.194.32", 8585), Some(13));
        assert_eq!(channel_from_farm("43.142.194.34", 8585), Some(19));
        assert_eq!(channel_from_farm("43.142.194.37", 8585), Some(28));
        assert_eq!(channel_from_farm("43.142.194.40", 8585), Some(34));
        assert_eq!(channel_from_farm("43.142.194.41", 8585), Some(37));
        assert_eq!(channel_from_farm("43.142.194.44", 8587), Some(48));
        assert_eq!(channel_from_farm("43.142.194.48", 8585), Some(58));

        // 跳号的两台不在这个服的列表里 → 未知（不硬猜）
        assert_eq!(channel_from_farm("43.142.194.31", 8585), None);
        assert_eq!(channel_from_farm("43.142.194.39", 8585), None);

        // 蘑菇仔（现场扫了 13 个点，含 .51–.54/.58/.60/.61 七处跳号）
        assert_eq!(channel_from_farm("43.142.194.49", 8585), Some(1));
        assert_eq!(channel_from_farm("43.142.194.49", 8586), Some(2));
        assert_eq!(channel_from_farm("43.142.194.49", 8587), Some(3));
        assert_eq!(channel_from_farm("43.142.194.50", 8585), Some(4));
        assert_eq!(channel_from_farm("43.142.194.55", 8587), Some(9));
        assert_eq!(channel_from_farm("43.142.194.56", 8585), Some(10));
        assert_eq!(channel_from_farm("43.142.194.57", 8585), Some(13));
        assert_eq!(channel_from_farm("43.142.194.59", 8585), Some(16));
        assert_eq!(channel_from_farm("43.142.194.62", 8585), Some(19));
        assert_eq!(channel_from_farm("43.142.194.63", 8585), Some(22));
        assert_eq!(channel_from_farm("43.142.194.64", 8585), Some(25));
        assert_eq!(channel_from_farm("43.142.194.65", 8585), Some(28));
        assert_eq!(channel_from_farm("43.142.194.75", 8585), Some(58));
        // 跳号的那些不在这个服的列表里 → 未知
        assert_eq!(channel_from_farm("43.142.194.51", 8585), None);
        assert_eq!(channel_from_farm("43.142.194.54", 8585), None);
        assert_eq!(channel_from_farm("43.142.194.58", 8585), None);
        assert_eq!(channel_from_farm("43.142.194.60", 8585), None);
        assert_eq!(channel_from_farm("43.142.194.61", 8585), None);

        // 漂漂猪（现场扫了 21 个点，含 .143/.146/.148/.154/.155/.159 六处跳号）
        assert_eq!(channel_from_farm("43.142.194.140", 8585), Some(1));
        assert_eq!(channel_from_farm("43.142.194.140", 8587), Some(3));
        assert_eq!(channel_from_farm("43.142.194.141", 8586), Some(5));
        assert_eq!(channel_from_farm("43.142.194.142", 8587), Some(9));
        assert_eq!(channel_from_farm("43.142.194.144", 8585), Some(10));
        assert_eq!(channel_from_farm("43.142.194.145", 8585), Some(13));
        assert_eq!(channel_from_farm("43.142.194.147", 8585), Some(16));
        assert_eq!(channel_from_farm("43.142.194.149", 8585), Some(19));
        assert_eq!(channel_from_farm("43.142.194.150", 8585), Some(22));
        assert_eq!(channel_from_farm("43.142.194.151", 8585), Some(25));
        assert_eq!(channel_from_farm("43.142.194.152", 8585), Some(28));
        assert_eq!(channel_from_farm("43.142.194.153", 8585), Some(31));
        assert_eq!(channel_from_farm("43.142.194.156", 8585), Some(34));
        assert_eq!(channel_from_farm("43.142.194.157", 8585), Some(37));
        assert_eq!(channel_from_farm("43.142.194.158", 8585), Some(40));
        assert_eq!(channel_from_farm("43.142.194.160", 8585), Some(43));
        assert_eq!(channel_from_farm("43.142.194.161", 8585), Some(46));
        assert_eq!(channel_from_farm("43.142.194.162", 8585), Some(49));
        assert_eq!(channel_from_farm("43.142.194.163", 8585), Some(52));
        assert_eq!(channel_from_farm("43.142.194.164", 8585), Some(55));
        assert_eq!(channel_from_farm("43.142.194.165", 8585), Some(58));
        // 跳号的那些不在这个服的列表里 → 未知
        assert_eq!(channel_from_farm("43.142.194.143", 8585), None);
        assert_eq!(channel_from_farm("43.142.194.146", 8585), None);
        assert_eq!(channel_from_farm("43.142.194.148", 8585), None);
        assert_eq!(channel_from_farm("43.142.194.154", 8585), None);
        assert_eq!(channel_from_farm("43.142.194.155", 8585), None);
        assert_eq!(channel_from_farm("43.142.194.159", 8585), None);

        // 小白兔（现场扫了 20 多个点，含 .171/.185 两处跳号）
        assert_eq!(channel_from_farm("43.142.194.166", 8585), Some(1));
        assert_eq!(channel_from_farm("43.142.194.166", 8586), Some(2));
        assert_eq!(channel_from_farm("43.142.194.166", 8587), Some(3));
        assert_eq!(channel_from_farm("43.142.194.167", 8585), Some(4));
        assert_eq!(channel_from_farm("43.142.194.168", 8587), Some(9));
        assert_eq!(channel_from_farm("43.142.194.170", 8585), Some(13));
        assert_eq!(channel_from_farm("43.142.194.172", 8585), Some(16));
        assert_eq!(channel_from_farm("43.142.194.175", 8585), Some(25));
        assert_eq!(channel_from_farm("43.142.194.176", 8585), Some(28));
        assert_eq!(channel_from_farm("43.142.194.177", 8585), Some(31));
        assert_eq!(channel_from_farm("43.142.194.178", 8585), Some(34));
        assert_eq!(channel_from_farm("43.142.194.180", 8585), Some(40));
        assert_eq!(channel_from_farm("43.142.194.183", 8585), Some(49));
        assert_eq!(channel_from_farm("43.142.194.184", 8585), Some(52));
        assert_eq!(channel_from_farm("43.142.194.186", 8585), Some(55));
        assert_eq!(channel_from_farm("43.142.194.187", 8585), Some(58));
        // 跳号的两台不在这个服的列表里 → 未知
        assert_eq!(channel_from_farm("43.142.194.171", 8585), None);
        assert_eq!(channel_from_farm("43.142.194.185", 8585), None);

        // 形状不认识的（端口不对 / 末段没录入）一律不认
        assert_eq!(channel_from_farm("43.142.194.91", 8484), None);
        assert_eq!(channel_from_farm("43.142.194.26", 8585), None);
        assert_eq!(channel_from_farm("10.0.0.80", 9999), None);
    }

    #[test]
    fn learned_base_wins_over_the_farm_pattern() {
        let mut bases = HashMap::new();
        bases.insert("43.142.194.77".to_string(), 8585 - 30); // 手动校准成 30 线
        assert_eq!(channel_for("43.142.194.77", 8585, &bases), Some(30));
        // 没校准过的节点走农场规律
        assert_eq!(channel_for("43.142.194.78", 8586, &bases), Some(8));
        // 校准过但算不出合法线号的，退回农场规律
        bases.insert("43.142.194.78".to_string(), 1);
        assert_eq!(channel_for("43.142.194.78", 8586, &bases), Some(8));
    }

    #[test]
    fn port_maps_to_channel_with_the_nodes_base() {
        // 实测：43.142.194.78 节点 7 线 = 8585、8 线 = 8586 → 基准 8578
        assert_eq!(channel_from_port(8585, 8578), Some(7));
        assert_eq!(channel_from_port(8586, 8578), Some(8));
        // 实测：43.142.194.91 节点 47 线 = 8586、48 线 = 8587 → 基准 8539
        assert_eq!(channel_from_port(8586, 8539), Some(47));
        assert_eq!(channel_from_port(8587, 8539), Some(48));
        // 区间外的一律不认
        assert_eq!(channel_from_port(8578, 8578), None);
        assert_eq!(channel_from_port(8639, 8578), None);
        assert_eq!(channel_from_port(443, 8578), None);
    }

    #[test]
    fn learns_a_base_per_host() {
        let (db, path) = temp_db("host-base");
        save_host_base(&db, "43.142.194.78", 8578);
        save_host_base(&db, "43.142.194.91", 8539);
        let bases = host_bases(&db);
        assert_eq!(bases.get("43.142.194.78"), Some(&8578));
        assert_eq!(bases.get("43.142.194.91"), Some(&8539));
        // 同一个端口在不同节点上是不同的线
        assert_eq!(channel_from_port(8586, bases["43.142.194.78"]), Some(8));
        assert_eq!(channel_from_port(8586, bases["43.142.194.91"]), Some(47));
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn corrupt_host_bases_json_falls_back_to_empty() {
        let (db, path) = temp_db("bad-json");
        db.set_setting(HOST_BASES_KEY, "not json").expect("写坏 JSON 失败");
        assert!(host_bases(&db).is_empty());
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn rejects_channels_outside_one_to_sixty() {
        let (db, path) = temp_db("range");
        let watcher = ChannelWatcher::new(&db);
        assert!(watcher.set_manual(&db, 0).is_err());
        assert!(watcher.set_manual(&db, 61).is_err());
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn formats_ipv4_from_network_order() {
        // 43.142.194.91 的网络序字节 [0x2B, 0x8E, 0xC2, 0x5B]，小端读进 u32
        let raw = u32::from_le_bytes([0x2B, 0x8E, 0xC2, 0x5B]);
        assert_eq!(ipv4_string(raw), "43.142.194.91");
        let raw = u32::from_le_bytes([127, 0, 0, 1]);
        assert_eq!(ipv4_string(raw), "127.0.0.1");
    }

    fn on_line(server: Option<&str>, channel: u32, source: &str, updated_unix: i64) -> ChannelState {
        ChannelState {
            channel: Some(channel),
            server: server.map(str::to_string),
            source: source.to_string(),
            updated_unix,
            ..ChannelState::default()
        }
    }

    #[test]
    fn server_name_comes_from_the_node_regardless_of_port() {
        assert_eq!(farm_name("43.142.194.91"), Some("绿水灵"));
        assert_eq!(farm_name("43.142.194.27"), Some("蓝蜗牛"));
        assert_eq!(farm_name("43.142.194.49"), Some("蘑菇仔"));
        assert_eq!(farm_name("43.142.194.140"), Some("漂漂猪"));
        assert_eq!(farm_name("43.142.194.187"), Some("小白兔"));
        // 跳号的机器、没录入的机器都不认
        assert_eq!(farm_name("43.142.194.31"), None);
        assert_eq!(farm_name("10.0.0.1"), None);
        // 状态里的区服名跟着节点走，手动校准的也带
        let state = ChannelState::new(Some(30), "manual", Some("43.142.194.77".into()), Some(8585), Some(8555), 0);
        assert_eq!(state.server.as_deref(), Some("绿水灵"));
    }

    #[test]
    fn switching_lines_remembers_the_one_just_left() {
        let previous = on_line(Some("绿水灵"), 47, "auto", 100);
        let recent = recent_after_arrival(&previous, Some("绿水灵"), 48, 200);
        assert_eq!(
            recent,
            vec![ChannelVisit {
                server: Some("绿水灵".to_string()),
                channel: 47,
                last_seen_unix: 200,
                dropped: false,
            }]
        );
        // 同一条线上重复确认不产生记录
        assert!(recent_after_arrival(&previous, Some("绿水灵"), 47, 200).is_empty());
    }

    #[test]
    fn a_drop_is_remembered_with_the_time_the_connection_went_away() {
        // 47 线掉线（300 时标成 stale），重登进了 1 线
        let dropped = on_line(Some("绿水灵"), 47, "stale", 300);
        let recent = recent_after_arrival(&dropped, Some("绿水灵"), 1, 900);
        assert_eq!(recent.len(), 1);
        assert_eq!(recent[0].channel, 47);
        assert_eq!(recent[0].last_seen_unix, 300);
        assert!(recent[0].dropped);

        // 掉线后重登回同一条线：没有「上次」可言
        assert!(recent_after_arrival(&dropped, Some("绿水灵"), 47, 900).is_empty());

        // 掉线后先看到登录服（认不出线号）也要记下来
        let live = on_line(Some("绿水灵"), 47, "auto", 100);
        let recent = recent_after_loss(&live, 400);
        assert_eq!(recent[0].channel, 47);
        assert_eq!(recent[0].last_seen_unix, 400);
        assert!(recent[0].dropped);
    }

    #[test]
    fn returning_to_a_line_takes_it_off_the_list_and_the_list_is_capped() {
        // 47 → 1 → 47：回到 47 以后列表里只剩 1
        let mut state = on_line(Some("绿水灵"), 1, "auto", 0);
        state.recent = recent_after_arrival(&on_line(Some("绿水灵"), 47, "stale", 0), Some("绿水灵"), 1, 10);
        let recent = recent_after_arrival(&state, Some("绿水灵"), 47, 20);
        assert_eq!(recent.iter().map(|visit| visit.channel).collect::<Vec<_>>(), vec![1]);

        // 一路换下去只留最近 RECENT_LIMIT 条，新的在前
        let mut state = on_line(Some("绿水灵"), 1, "auto", 0);
        for channel in 2..=10 {
            let recent = recent_after_arrival(&state, Some("绿水灵"), channel, channel as i64);
            state = on_line(Some("绿水灵"), channel, "auto", channel as i64);
            state.recent = recent;
        }
        assert_eq!(
            state.recent.iter().map(|visit| visit.channel).collect::<Vec<_>>(),
            vec![9, 8, 7, 6, 5]
        );

        // 同线号不同区服是两条线
        let recent = recent_after_arrival(&on_line(Some("绿水灵"), 5, "auto", 0), Some("蓝蜗牛"), 5, 1);
        assert_eq!(recent.len(), 1);
        // 旧版本存下来的记录没有区服名：只比线号，不把自己记成「上次」
        assert!(recent_after_arrival(&on_line(None, 5, "stale", 0), Some("绿水灵"), 5, 1).is_empty());
    }

    #[test]
    fn restart_restores_server_and_recent_lines() {
        let (db, path) = temp_db("recent");
        let mut state = on_line(Some("绿水灵"), 3, "auto", 1790572000);
        state.recent = vec![ChannelVisit {
            server: Some("绿水灵".to_string()),
            channel: 47,
            last_seen_unix: 1790571000,
            dropped: true,
        }];
        save(&db, &state);
        save_recent(&db, &state.recent);

        let restored = ChannelWatcher::new(&db).state();
        assert_eq!(restored.channel, Some(3));
        assert_eq!(restored.server.as_deref(), Some("绿水灵"));
        assert_eq!(restored.source, "stale");
        assert_eq!(restored.recent, state.recent);

        // 存坏了就当没有
        db.set_setting(RECENT_KEY, "not json").expect("写坏 JSON 失败");
        assert!(ChannelWatcher::new(&db).state().recent.is_empty());
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn restart_shows_the_last_channel_as_stale() {
        let (db, path) = temp_db("restart");
        db.set_setting(LAST_KEY, "8").expect("写线号失败");
        db.set_setting(LAST_UNIX_KEY, "1790572000").expect("写时间失败");
        let watcher = ChannelWatcher::new(&db);
        let state = watcher.state();
        assert_eq!(state.channel, Some(8));
        assert_eq!(state.source, "stale");
        assert_eq!(state.base, None);
        let _ = std::fs::remove_file(path);
    }

    /// 真机探针：游戏开着时打印连接、各节点已知基准和换算出的线号。
    ///
    /// ```text
    /// cd src-tauri
    /// cargo test --lib -- --ignored probe_channel_port --nocapture
    /// ```
    #[test]
    #[ignore]
    fn probe_channel_port() {
        crate::exp::capture::ensure_dpi_aware();
        let (db, path) = temp_db("probe");
        println!("已校准的节点基准：{:?}", host_bases(&db));
        let Some(window) = crate::exp::capture::find_game_window() else {
            println!("没找到游戏窗口（游戏没开？）");
            let _ = std::fs::remove_file(path);
            return;
        };
        println!("窗口：{}（hwnd {}）", window.title, window.hwnd);
        let Some(pid) = win::window_process_id(window.hwnd) else {
            println!("拿不到进程号");
            let _ = std::fs::remove_file(path);
            return;
        };
        println!("进程号：{pid}");
        let remotes = current_game_remotes();
        println!("ESTABLISHED 远端：{remotes:?}");
        let bases = host_bases(&db);
        for (host, port) in &remotes {
            let base = bases.get(host).copied();
            println!(
                "  {host}:{port} 基准 {base:?} → {:?} 线（农场规律 {:?}）",
                channel_for(host, *port, &bases),
                channel_from_farm(host, *port)
            );
        }
        let _ = std::fs::remove_file(path);
    }
}
