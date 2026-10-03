//! 金价到价提醒 —— 预设区服的金价涨到 / 跌到你设的数时提醒一次。
//!
//! # 盯的是哪个数
//!
//! 站点报价里的「1 元能换多少万金」（`MesoRate::wan_rate`），也就是主窗口总览页
//! 最大的那个数。它**越大 = 金币越便宜**：
//!
//! * `above`：涨到这个数或以上时提醒（金币便宜了，想买金的人盯这个）；
//! * `below`：跌到这个数或以下时提醒（金币贵了，想卖金的人盯这个）。
//!
//! 两个都可以只填一个。区服跟着全局预设区服走，不另设一份。
//!
//! # 什么时候查
//!
//! 开着的时候每 10 分钟查一次（站点自己就是 10 分钟刷新一次报价，问得更勤没意义），
//! 走的是和「行情」页同一个带缓存的取数函数 —— 主窗口刚取过就直接用缓存，不多打站点。
//! 没开、或者两个数都没填时，这个循环什么请求都不发。
//! 改完设置会立刻查一次：已经满足的条件不用等十分钟才响。
//!
//! # 只在「跨过去」的那一下提醒
//!
//! 金价会在一个数附近停很久，每次查到「仍然满足」都响就成了骚扰。所以记一个状态
//! （`meso_alert_state`，形如 `3:above`）：同一个区服、同一个方向响过之后不再响，
//! 直到价格**退回去超过 1%** 才重新上膛（`REARM_MARGIN`）—— 不然价格在阈值上下
//! 一两分钱地晃，每晃一次就响一次。换区服、改阈值、关了再开，都会清掉这个状态。

use crate::commands::AppState;
use crate::db::Database;
use crate::mxdc_client::{MesoReport, SERVERS};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tauri::{AppHandle, Manager};
use tokio::time::{sleep, Duration};

pub const ENABLED_KEY: &str = "meso_alert_enabled";
pub const ABOVE_KEY: &str = "meso_alert_above";
pub const BELOW_KEY: &str = "meso_alert_below";
pub const STATE_KEY: &str = "meso_alert_state";

/// 查一次的间隔：和站点报价的刷新间隔、取数缓存的有效期一致。
const CHECK_INTERVAL: Duration = Duration::from_secs(600);
/// 启动后等一会儿再查第一次：别和启动时那一堆请求挤在一起。
const FIRST_DELAY: Duration = Duration::from_secs(20);
/// 响过之后，价格要退回阈值这么多比例才重新上膛。
const REARM_MARGIN: f64 = 0.01;
/// 阈值的合理范围（1 元换的万金）。只是挡住手滑填错的数。
const MIN_RATE: f64 = 0.01;
const MAX_RATE: f64 = 100_000.0;

/// 界面上那张「金价到价提醒」卡片的全部设置。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MesoAlertConfig {
    pub enabled: bool,
    /// 1 元换的万金 ≥ 这个数时提醒；`None` = 不盯这个方向
    pub above: Option<f64>,
    /// 1 元换的万金 ≤ 这个数时提醒；`None` = 不盯这个方向
    pub below: Option<f64>,
}

impl MesoAlertConfig {
    fn watching(&self) -> bool {
        self.enabled && (self.above.is_some() || self.below.is_some())
    }

    /// 挡住没有意义的设置（负数、两个阈值互相包住对方）。
    pub fn validate(&self) -> Result<(), String> {
        for (label, value) in [("涨到", self.above), ("跌到", self.below)] {
            if let Some(value) = value {
                if !value.is_finite() || !(MIN_RATE..=MAX_RATE).contains(&value) {
                    return Err(format!("「{label}」那个数要在 {MIN_RATE}–{MAX_RATE} 之间"));
                }
            }
        }
        if let (Some(above), Some(below)) = (self.above, self.below) {
            if below >= above {
                return Err("「跌到」的数要比「涨到」的数小，不然任何价格都会触发".to_string());
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Above,
    Below,
}

impl Side {
    fn code(self) -> &'static str {
        match self {
            Side::Above => "above",
            Side::Below => "below",
        }
    }
}

pub fn load_config(db: &Database) -> MesoAlertConfig {
    let number = |key: &str| {
        db.get_setting(key)
            .and_then(|raw| raw.trim().parse::<f64>().ok())
            .filter(|value| value.is_finite() && *value > 0.0)
    };
    MesoAlertConfig {
        enabled: db.get_setting(ENABLED_KEY).as_deref() == Some("1"),
        above: number(ABOVE_KEY),
        below: number(BELOW_KEY),
    }
}

/// 存设置，并清掉「已经响过」的状态：阈值变了，旧的结论就不算数了。
pub fn save_config(db: &Database, config: &MesoAlertConfig) -> Result<(), String> {
    config.validate()?;
    let text = |value: Option<f64>| value.map(|v| v.to_string()).unwrap_or_default();
    let pairs = [
        (ENABLED_KEY, if config.enabled { "1" } else { "0" }.to_string()),
        (ABOVE_KEY, text(config.above)),
        (BELOW_KEY, text(config.below)),
        (STATE_KEY, String::new()),
    ];
    for (key, value) in pairs {
        db.set_setting(key, &value)
            .map_err(|err| format!("保存金价提醒设置失败：{err}"))?;
    }
    Ok(())
}

/// 这一次该不该响，以及响完 / 没响之后状态变成什么。
///
/// `last_state` 是上次留下的状态（`""` = 没响过，`"3:above"` = 3 区「涨到」已经响过）。
/// 返回 `(这次要响的方向, 新状态)`。
pub fn decide(
    rate: f64,
    config: &MesoAlertConfig,
    server_id: u32,
    last_state: &str,
) -> (Option<Side>, String) {
    let hit = match (config.above, config.below) {
        (Some(above), _) if rate >= above => Some(Side::Above),
        (_, Some(below)) if rate <= below => Some(Side::Below),
        _ => None,
    };
    let key = |side: Side| format!("{server_id}:{}", side.code());

    if let Some(side) = hit {
        let state = key(side);
        // 同一个区服、同一个方向已经响过：不重复响
        let fire = (state != last_state).then_some(side);
        return (fire, state);
    }

    // 没满足：只有退回去够远才重新上膛，免得在阈值上下晃一次响一次
    let still_near = |side: Side, threshold: Option<f64>| {
        last_state == key(side)
            && threshold.is_some_and(|threshold| match side {
                Side::Above => rate >= threshold * (1.0 - REARM_MARGIN),
                Side::Below => rate <= threshold * (1.0 + REARM_MARGIN),
            })
    };
    if still_near(Side::Above, config.above) || still_near(Side::Below, config.below) {
        return (None, last_state.to_string());
    }
    (None, String::new())
}

/// 报价表里某个区服的「1 元换多少万金」。
fn rate_of(report: &MesoReport, server_name: &str) -> Option<f64> {
    report
        .latest
        .iter()
        .find(|rate| rate.server_name == server_name)
        .and_then(|rate| rate.wan_rate.trim().parse::<f64>().ok())
        .filter(|value| value.is_finite() && *value > 0.0)
}

fn alert_body(server_name: &str, rate: f64, side: Side, config: &MesoAlertConfig) -> String {
    let (verb, threshold, meaning) = match side {
        Side::Above => ("涨到", config.above, "金币变便宜了"),
        Side::Below => ("跌到", config.below, "金币变贵了"),
    };
    format!(
        "{server_name}：1 元现在能换 {rate:.2} 万金，已经{verb}你设的 {} 万金（{meaning}）。\n\n\
         —— 数据来自小册子的金价报价，10 分钟更新一次；价格退回去之后再次到价才会再提醒。",
        threshold.map(|value| format!("{value:.2}")).unwrap_or_default()
    )
}

/// 查一次。没开 / 没填阈值时什么请求都不发。
pub async fn check_once(app: &AppHandle) {
    let Some(state) = app.try_state::<AppState>() else {
        return;
    };
    let (db, client) = (Arc::clone(&state.db), Arc::clone(&state.mxdc_client));
    drop(state);

    let config = load_config(&db);
    if !config.watching() {
        return;
    }
    let server_id = db.get_all_settings().default_server_id;
    let Some(server_name) = SERVERS
        .iter()
        .find(|(id, _)| *id == server_id)
        .map(|(_, name)| *name)
    else {
        return;
    };

    let report = match crate::commands::load_meso_report(&db, &client).await {
        Ok(report) => report,
        Err(err) => {
            // 取不到就等下一轮：没有数据不等于「没到价」，更不能拿旧数据凑
            log::info!("金价提醒：这一轮没取到报价（{err}），下一轮再看");
            return;
        }
    };
    let Some(rate) = rate_of(&report, server_name) else {
        log::info!("金价提醒：报价表里没有 {server_name} 的数，下一轮再看");
        return;
    };

    let last_state = db.get_setting(STATE_KEY).unwrap_or_default();
    let (fire, next_state) = decide(rate, &config, server_id, &last_state);
    if next_state != last_state {
        if let Err(err) = db.set_setting(STATE_KEY, &next_state) {
            log::warn!("保存金价提醒状态失败：{err}");
        }
    }
    if let Some(side) = fire {
        crate::alert::fire_meso_alert(app, alert_body(server_name, rate, side, &config));
    }
}

/// 后台循环：每 10 分钟看一眼。
pub fn spawn(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        sleep(FIRST_DELAY).await;
        loop {
            check_once(&app).await;
            sleep(CHECK_INTERVAL).await;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(above: Option<f64>, below: Option<f64>) -> MesoAlertConfig {
        MesoAlertConfig {
            enabled: true,
            above,
            below,
        }
    }

    #[test]
    fn fires_once_when_the_price_crosses_and_not_again_while_it_stays() {
        let cfg = config(Some(9.0), None);
        // 还没到
        assert_eq!(decide(8.8, &cfg, 3, ""), (None, String::new()));
        // 到了：响一次
        let (fire, state) = decide(9.1, &cfg, 3, "");
        assert_eq!(fire, Some(Side::Above));
        assert_eq!(state, "3:above");
        // 还在上面：不重复响
        assert_eq!(decide(9.3, &cfg, 3, &state), (None, state.clone()));
    }

    #[test]
    fn hovering_around_the_threshold_does_not_ring_every_time() {
        let cfg = config(Some(9.0), None);
        // 响过之后只退了一点点（不到 1%）：状态留着，回来也不再响
        let (fire, state) = decide(8.95, &cfg, 3, "3:above");
        assert_eq!((fire, state.as_str()), (None, "3:above"));
        assert_eq!(decide(9.02, &cfg, 3, &state).0, None);
        // 退回去超过 1%：重新上膛，下次到价会再响
        let (fire, state) = decide(8.8, &cfg, 3, "3:above");
        assert_eq!((fire, state.as_str()), (None, ""));
        assert_eq!(decide(9.0, &cfg, 3, &state).0, Some(Side::Above));
    }

    #[test]
    fn below_threshold_works_the_same_way_and_servers_do_not_share_state() {
        let cfg = config(Some(9.5), Some(8.0));
        let (fire, state) = decide(7.9, &cfg, 2, "");
        assert_eq!((fire, state.as_str()), (Some(Side::Below), "2:below"));
        assert_eq!(decide(7.5, &cfg, 2, &state).0, None);
        // 换了区服：那个服还没响过
        assert_eq!(decide(7.9, &cfg, 5, &state).0, Some(Side::Below));
        // 从「跌到」直接翻到「涨到」：是另一件事，要响
        assert_eq!(decide(9.6, &cfg, 2, &state).0, Some(Side::Above));
        // 回到两个阈值中间、离两边都够远：清空
        assert_eq!(decide(8.8, &cfg, 2, &state), (None, String::new()));
    }

    #[test]
    fn rejects_settings_that_would_always_or_never_make_sense() {
        assert!(config(Some(9.0), Some(8.0)).validate().is_ok());
        assert!(config(None, None).validate().is_ok());
        assert!(config(Some(-1.0), None).validate().is_err());
        assert!(config(Some(f64::NAN), None).validate().is_err());
        // 「跌到」≥「涨到」：任何价格都满足其中一个
        assert!(config(Some(8.0), Some(8.0)).validate().is_err());
        assert!(config(Some(8.0), Some(9.0)).validate().is_err());
    }

    #[test]
    fn settings_round_trip_and_saving_rearms() {
        let path = std::env::temp_dir().join(format!(
            "mxdbox-test-meso-alert-{}.db",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);
        let db = Database::new(path.clone()).expect("建库失败");

        // 没设过：关着，两个方向都不盯
        assert_eq!(load_config(&db), MesoAlertConfig { enabled: false, above: None, below: None });

        db.set_setting(STATE_KEY, "3:above").expect("写状态失败");
        let cfg = config(Some(9.25), None);
        save_config(&db, &cfg).expect("保存失败");
        assert_eq!(load_config(&db), cfg);
        // 改了设置：之前「已经响过」不算数了
        assert_eq!(db.get_setting(STATE_KEY).unwrap_or_default(), "");

        // 不合理的设置存不进去，原来的还在
        assert!(save_config(&db, &config(Some(8.0), Some(9.0))).is_err());
        assert_eq!(load_config(&db), cfg);
        let _ = std::fs::remove_file(path);
    }
}
