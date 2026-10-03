use crate::exp::region::{RegionProfile, RegionSource, RegionStore};
use parking_lot::Mutex;
use rusqlite::{params, Connection, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlacklistEntry {
    pub id: Option<i64>,
    pub server: String,
    pub player_name: String,
    pub category: String, // 骗子/抢怪/黑金/跑单/其他
    pub reason: String,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AlertHistoryRow {
    pub id: i64,
    pub kind: String,
    pub title: String,
    pub detail: String,
    pub fired_unix: i64,
}

/// 一段经验会话（`exp_sessions` 一行）。
///
/// 除了「得到多少经验」之外，这一行还要回答**这段数据能不能信**：
/// `quality` 是结论，`quality_reason` 是理由（原话存下来，过段时间也能看懂）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExpSessionRow {
    pub started_unix: i64,
    pub ended_unix: i64,
    /// 会话内**有效时间**（暂停那段不算）
    pub active_secs: f64,
    pub start_level: Option<u32>,
    pub end_level: Option<u32>,
    pub start_exp: u64,
    pub end_exp: u64,
    pub start_percent: Option<f64>,
    pub end_percent: Option<f64>,
    pub gained_exp: u64,
    /// 起止两端的**累计经验**（从 1 级 0 经验算起）。
    /// 有它就能跨级、跨段求和，也是「这一段到底涨了多少」的第二种算法。
    pub start_cum: Option<u64>,
    pub end_cum: Option<u64>,
    /// 2 = 数据完整 / 1 = 可参考 / 0 = 不建议用于比较
    pub quality: u8,
    pub quality_reason: String,
    /// 能读到画面并成功识别的时间占比（0~1）
    pub coverage: f64,
    /// 真正在涨经验的时间占比（挂机比例的反面）
    pub idle_ratio: f64,
    /// 读到了结构但没过校验的帧数（「数学拒绝帧」）
    pub rejected_frames: u32,
    /// 这一段的练级地图（**用户自己填的**，空串 = 没填）。
    ///
    /// 程序不去认地图：地图名在小地图那一角，认错了比不认更糟 —— 一张写着
    /// 「蘑菇神社」实际在蚂蚁洞的小结卡片，比没写地图更没用。填一次就能进历史
    /// 和小结卡片。
    pub map_name: String,
}

/// 会话里的一个采样点（每分钟一个）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExpSampleRow {
    pub captured_unix: i64,
    /// 会话开始后多少秒（有效时间）
    pub offset_secs: i64,
    pub level: Option<u32>,
    pub exp: u64,
    pub percent: f64,
}

/// 历史列表用的一行（含界面要显示的几个派生值）。
#[derive(Debug, Clone, Serialize)]
pub struct ExpHistoryRow {
    pub id: i64,
    pub started_unix: i64,
    pub ended_unix: i64,
    pub active_secs: f64,
    pub start_level: Option<u32>,
    pub end_level: Option<u32>,
    pub start_percent: Option<f64>,
    pub end_percent: Option<f64>,
    pub gained_exp: u64,
    pub quality: u8,
    pub quality_reason: String,
    pub coverage: f64,
    pub idle_ratio: f64,
    /// 这一段的练级地图（用户填的；没填是空串）
    pub map_name: String,
}

/// 历史汇总（页面上那句「共 N 段 · 累计练了多久 · 一共多少经验」）。
#[derive(Debug, Clone, Serialize, Default)]
pub struct ExpTotals {
    pub sessions: u32,
    pub active_secs: f64,
    pub gained_exp: u64,
}

/// 曲线用的一个点。
#[derive(Debug, Clone, Serialize)]
pub struct ExpCurvePoint {
    pub captured_unix: i64,
    pub exp: u64,
    pub percent: f64,
    pub level: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppSettings {
    pub memory_threshold: u32,
    /// 呼出 / 收起**查价悬浮窗**的全局快捷键（按压捕获，默认 `Alt+F`）
    #[serde(default = "default_price_hud_hotkey")]
    pub hotkey: String,
    /// 呼出 / 收起**精简悬浮窗**（当前金价 + 内存）的全局快捷键，默认 `F10`
    #[serde(default = "default_lite_float_hotkey")]
    pub lite_float_hotkey: String,
    /// 全局预设区服（小册子区服表里的 `id`）。
    ///
    /// 以前拍卖查询 / 悬浮窗各自 `useState(1)` 写死蓝蜗牛，谁都没记住用户在哪个服 ——
    /// 于是每开一个窗口都要重新选一次。现在这里存一份全局的，谁改了都写回来。
    #[serde(default = "default_server_id")]
    pub default_server_id: u32,
    /// 订阅小册子「开服监控」：站点每分钟探一次登录入口，我们接它的结论做开服提醒。
    /// 默认开启 —— 它是**唯一**不需要用户自己填任何地址就能用的开服提醒（自填
    /// 网关端口那套会随着机房轮换默默失效）。轮询只是一个小 JSON 请求。
    #[serde(default = "default_true")]
    pub monitor_enabled: bool,
    /// 拉取开服监控的间隔。站点自己每 20 秒采集一次，我们问得太勤没意义，
    /// 但开服是「差一分钟就抢不到位置」的事，所以默认 30 秒。
    #[serde(default = "default_monitor_interval")]
    pub monitor_interval_sec: u64,
    /// 开始 / 暂停 / 恢复**经验统计**的全局快捷键。
    ///
    /// 默认给组合键而不是像 F10 那样的单键：全局快捷键会**从游戏手里把按键抢走**
    /// （游戏那边没有任何提示），用组合键不容易撞上游戏的技能键。
    #[serde(default = "default_exp_hotkey")]
    pub exp_hotkey: String,
    pub auth_cookie: String,
    /// 关闭主窗口时：true = 退到托盘常驻（开服提醒、剪贴板嗅探、全局快捷键都还在），
    /// false = 真正退出进程。
    ///
    /// **默认开启**：这个工具的价值有大半在「主窗口关掉之后」—— 等开服、
    /// 盯内存、游戏里按快捷键，都发生在主窗口不在前台的时候。默认关掉的话，
    /// 用户点一次 X 就把这些全停了，而他并不知道自己关掉了什么。
    #[serde(default = "default_true")]
    pub close_to_tray: bool,
    /// 剪贴板嗅探：复制到的文本看起来像角色名时，自动比对本地黑名单库，
    /// **确定命中**（同名 + 同服）就响铃 + 弹提醒窗。
    ///
    /// 默认开 —— 这是黑名单从「录了也不一定记得查」变成真正有用的关键：
    /// 要查名字的时刻恰好是在游戏里交易时，那时你不会去点开黑名单页。
    /// 嗅探只在本地比对，内容不出本机、不写日志。
    #[serde(default = "default_true")]
    pub blacklist_watch: bool,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            memory_threshold: 85,
            hotkey: default_price_hud_hotkey(),
            lite_float_hotkey: default_lite_float_hotkey(),
            default_server_id: default_server_id(),
            monitor_enabled: true,
            monitor_interval_sec: default_monitor_interval(),
            auth_cookie: String::new(),
            close_to_tray: true,
            blacklist_watch: true,
            exp_hotkey: default_exp_hotkey(),
        }
    }
}

/// `serde(default = ...)` 要的几个常量默认值。
/// 单独写出来是因为 `#[serde(default)]` 只支持零参函数。
fn default_true() -> bool {
    true
}

/// 小册子区服表里的第一个服（蓝蜗牛）。id 的合法值由 `mxdc_client::SERVERS` 决定。
fn default_server_id() -> u32 {
    1
}

fn default_price_hud_hotkey() -> String {
    crate::hotkeys::DEFAULT_PRICE_HUD_HOTKEY.to_string()
}

fn default_lite_float_hotkey() -> String {
    crate::hotkeys::DEFAULT_LITE_FLOAT_HOTKEY.to_string()
}

fn default_monitor_interval() -> u64 {
    30
}

fn default_exp_hotkey() -> String {
    crate::hotkeys::DEFAULT_EXP_TOGGLE_HOTKEY.to_string()
}


/// 表里没有这一列就加上（SQLite 的 `ADD COLUMN` 一次只能加一列，所以要逐列调用）。
///
/// 只按列名判断存在与否：类型写错是我们自己代码的问题，而单测会建一个真实的老库验一遍。
fn add_column_if_missing(conn: &Connection, table: &str, column: &str, decl: &str) -> Result<()> {
    let mut stmt = conn.prepare(&format!("PRAGMA table_info({})", table))?;
    let mut rows = stmt.query([])?;
    while let Some(row) = rows.next()? {
        let name: String = row.get(1)?;
        if name == column {
            return Ok(());
        }
    }
    // 列名是我们自己写死的字面量，不是用户输入，所以这里没有注入面
    conn.execute(
        &format!("ALTER TABLE {} ADD COLUMN {} {}", table, column, decl),
        [],
    )?;
    log::info!("数据库补列：{}.{}", table, column);
    Ok(())
}

// 图鉴缓存最长 24 小时；没有过期时间列时按最长 TTL 清理，避免删掉仍有效的条目。
const PRICE_CACHE_MAX_AGE_SEC: i64 = 24 * 60 * 60;

/// 把开发版那张「按尺寸 + DPI 分像素矩形」的校准表清掉（只清形状不对的表）。
///
/// 判据是 `text_height` 列：新表一定有它，旧表一定没有。表不存在时什么都不做。
fn drop_legacy_region_profiles(conn: &Connection) -> Result<()> {
    let mut stmt = conn.prepare("PRAGMA table_info(region_profiles)")?;
    let columns: Vec<String> = stmt
        .query_map([], |row| row.get::<_, String>(1))?
        .filter_map(Result::ok)
        .collect();
    if !columns.is_empty() && !columns.iter().any(|column| column == "text_height") {
        conn.execute("DROP TABLE region_profiles", [])?;
        log::info!("检测到旧版按分辨率分区的校准表，已重建为比例表");
    }
    Ok(())
}

fn upsert_setting(conn: &Connection, key: &str, value: &str) -> Result<()> {
    conn.execute(
        "INSERT INTO settings (key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = ?2",
        params![key, value],
    )?;
    Ok(())
}

pub struct Database {
    pub conn: Mutex<Connection>,
}

impl Database {
    pub fn new(path: PathBuf) -> Result<Self> {
        let conn = Connection::open(path)?;
        let db = Self {
            conn: Mutex::new(conn),
        };
        db.init_tables()?;
        Ok(db)
    }

    fn init_tables(&self) -> Result<()> {
        let conn = self.conn.lock();

        // 早期开发版建过一张按（客户区尺寸 + DPI）分区的 region_profiles。
        // 那个模型整个被换掉了（比例坐标，见 `exp::region`），而它从未随版本发布，
        // 所以这里不做逐行搬运、直接重建 —— 校准本来就可以随时重新学一次。
        drop_legacy_region_profiles(&conn)?;

        conn.execute_batch(
            "
            CREATE TABLE IF NOT EXISTS settings (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS blacklist (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                server TEXT NOT NULL,
                player_name TEXT NOT NULL,
                category TEXT NOT NULL,
                reason TEXT NOT NULL,
                created_at TEXT NOT NULL
            );

            CREATE INDEX IF NOT EXISTS idx_blacklist_name ON blacklist(player_name);

            CREATE TABLE IF NOT EXISTS alert_history (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                kind TEXT NOT NULL,
                title TEXT NOT NULL,
                detail TEXT NOT NULL,
                fired_unix INTEGER NOT NULL
            );

            CREATE TABLE IF NOT EXISTS price_cache (
                cache_key TEXT PRIMARY KEY,
                response_json TEXT NOT NULL,
                updated_at INTEGER NOT NULL
            );

            CREATE TABLE IF NOT EXISTS meso_cache (
                server_id INTEGER PRIMARY KEY,
                server_name TEXT NOT NULL,
                wan_rate REAL NOT NULL,
                yuan_rate REAL NOT NULL,
                updated_at INTEGER NOT NULL
            );

            -- 经验统计：一段会话 + 它每分钟的一个采样点。
            -- 采样点只留每分钟一个（2 秒一个点一天就是四万条，没有意义）。
            -- 只有用户点「结束」并选择计入历史时才会写进来 —— 没算数的那一段
            -- 连采样点一起丢掉，历史里不会出现没有归属的数据。
            CREATE TABLE IF NOT EXISTS exp_sessions (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                started_at INTEGER NOT NULL,
                ended_at INTEGER NOT NULL,
                active_secs REAL NOT NULL,
                start_level INTEGER,
                end_level INTEGER,
                start_exp INTEGER NOT NULL,
                end_exp INTEGER NOT NULL,
                start_percent REAL,
                end_percent REAL,
                gained_exp INTEGER NOT NULL,
                -- 累计经验账本（从 1 级 0 经验算起）：跨级、跨段求和都靠它
                start_cum INTEGER,
                end_cum INTEGER,
                -- 这段数据的可信度结论与理由（界面上一眼要能看出哪几段别拿去比）
                quality INTEGER NOT NULL DEFAULT 2,
                quality_reason TEXT NOT NULL DEFAULT '',
                coverage REAL NOT NULL DEFAULT 1.0,
                idle_ratio REAL NOT NULL DEFAULT 0.0,
                rejected_frames INTEGER NOT NULL DEFAULT 0
            );

            CREATE TABLE IF NOT EXISTS exp_samples (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                session_id INTEGER NOT NULL,
                captured_at INTEGER NOT NULL,
                offset_secs INTEGER NOT NULL,
                level INTEGER,
                exp INTEGER NOT NULL,
                percent REAL NOT NULL
            );

            CREATE INDEX IF NOT EXISTS idx_exp_samples_session ON exp_samples(session_id);
            CREATE INDEX IF NOT EXISTS idx_exp_samples_time ON exp_samples(captured_at);

            -- 地图名：键是小地图那一块的**像素指纹**。
            --
            -- 为什么不存「地图 id」或者「按顺序编号」：我们没有任何办法知道游戏的地图 id，
            -- 而小地图上那几个字是 11 像素高的中文，OCR 读不准（见 `exp::map` 里的实测表）。
            -- 像素指纹是精确的（同一张地图必然逐点相同），所以这样存的名字**永远不会串**。
            -- 代价只是每张新地图要用户填一次，之后每次来都自动认得出。
            CREATE TABLE IF NOT EXISTS exp_maps (
                fingerprint TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                updated_at INTEGER NOT NULL
            );

            -- 校准区域：**比例坐标**，一行一个 kind（见 `exp::region` 的模块文档）。
            --
            -- 为什么不是「按客户区尺寸 + DPI 分开存像素矩形」：那条路要求用户在每个
            -- 分辨率上各框一次，分辨率无穷多；而且游戏界面本身是随分辨率缩放的，
            -- 比例才是那个不变量。读不出来时自动退回整幅画面重定位，不需要第二套坐标。
            --
            -- x / y / w / h 都是相对客户区的比例（0..1）；client_w / client_h 只记录
            -- 最近学到时的尺寸（诊断用），不参与命中判断。
            CREATE TABLE IF NOT EXISTS region_profiles (
                kind TEXT PRIMARY KEY,
                x REAL NOT NULL,
                y REAL NOT NULL,
                w REAL NOT NULL,
                h REAL NOT NULL,
                source TEXT NOT NULL,
                text_height INTEGER,
                client_w INTEGER,
                client_h INTEGER,
                learned_unix INTEGER NOT NULL
            );
            ",
        )?;

        // 说明：早期版本有一张 `exp_glyphs`，存的是「运行时自动学到的字形」。
        // 那条路已经整个删掉了 —— 经验那一行的字符集是封闭的 14 个符号，
        // 现在全部内置在 `exp::font` 里，开箱即用，不需要学，也就不需要存。
        // 老库里那张表**故意留着不动**（没人读它），免得动用户的数据。

        // `CREATE TABLE IF NOT EXISTS` 不会给老表补列；缺少这些列会让升级后的写入失败。
        for (table, column, decl) in [
            ("exp_sessions", "start_cum", "INTEGER"),
            ("exp_sessions", "end_cum", "INTEGER"),
            ("exp_sessions", "quality", "INTEGER NOT NULL DEFAULT 2"),
            ("exp_sessions", "quality_reason", "TEXT NOT NULL DEFAULT ''"),
            ("exp_sessions", "coverage", "REAL NOT NULL DEFAULT 1.0"),
            ("exp_sessions", "idle_ratio", "REAL NOT NULL DEFAULT 0.0"),
            ("exp_sessions", "rejected_frames", "INTEGER NOT NULL DEFAULT 0"),
            ("exp_sessions", "map_name", "TEXT NOT NULL DEFAULT ''"),
        ] {
            add_column_if_missing(&conn, table, column, decl)?;
        }

        Ok(())
    }

    pub fn get_setting(&self, key: &str) -> Option<String> {
        let conn = self.conn.lock();
        let mut stmt = conn
            .prepare("SELECT value FROM settings WHERE key = ?1")
            .ok()?;
        let mut rows = stmt.query(params![key]).ok()?;
        if let Some(row) = rows.next().ok()? {
            row.get(0).ok()
        } else {
            None
        }
    }

    pub fn set_setting(&self, key: &str, value: &str) -> Result<()> {
        let conn = self.conn.lock();
        upsert_setting(&conn, key, value)
    }

    pub fn get_all_settings(&self) -> AppSettings {
        let mut settings = AppSettings::default();
        if let Some(v) = self.get_setting("memory_threshold") {
            if let Ok(n) = v.parse() {
                settings.memory_threshold = n;
            }
        }
        if let Some(v) = self.get_setting("hotkey") {
            let trimmed = v.trim();
            if !trimmed.is_empty() {
                settings.hotkey = trimmed.to_string();
            }
        }
        if let Some(v) = self.get_setting("lite_float_hotkey") {
            let trimmed = v.trim();
            if !trimmed.is_empty() {
                settings.lite_float_hotkey = trimmed.to_string();
            }
        }
        if let Some(v) = self.get_setting("default_server_id") {
            if let Ok(n) = v.parse() {
                settings.default_server_id = n;
            }
        }
        if let Some(v) = self.get_setting("monitor_enabled") {
            settings.monitor_enabled = v == "true" || v == "1";
        }
        if let Some(v) = self.get_setting("monitor_interval_sec") {
            if let Ok(n) = v.parse() {
                settings.monitor_interval_sec = n;
            }
        }
        if let Some(v) = self.get_setting("auth_cookie") {
            settings.auth_cookie = v;
        }
        if let Some(v) = self.get_setting("close_to_tray") {
            settings.close_to_tray = v == "true" || v == "1";
        }
        if let Some(v) = self.get_setting("blacklist_watch") {
            settings.blacklist_watch = v == "true" || v == "1";
        }
        if let Some(v) = self.get_setting("exp_hotkey") {
            let trimmed = v.trim();
            if !trimmed.is_empty() {
                settings.exp_hotkey = trimmed.to_string();
            }
        }
        settings
    }

    /// 原子地只更新传入的设置键：旧的完整快照不能覆盖并发写入的 Cookie 等字段。
    pub fn patch_settings(
        &self,
        monitor_enabled: Option<bool>,
        monitor_interval_sec: Option<u64>,
        close_to_tray: Option<bool>,
        blacklist_watch: Option<bool>,
    ) -> Result<()> {
        let mut conn = self.conn.lock();
        let tx = conn.transaction()?;

        if let Some(value) = monitor_enabled {
            upsert_setting(&tx, "monitor_enabled", if value { "1" } else { "0" })?;
        }
        if let Some(value) = monitor_interval_sec {
            upsert_setting(&tx, "monitor_interval_sec", &value.to_string())?;
        }
        if let Some(value) = close_to_tray {
            upsert_setting(&tx, "close_to_tray", if value { "1" } else { "0" })?;
        }
        if let Some(value) = blacklist_watch {
            upsert_setting(&tx, "blacklist_watch", if value { "1" } else { "0" })?;
        }

        tx.commit()
    }

    // -----------------------------------------------------------------------
    // 区域校准（比例坐标，一行一个 kind）
    // -----------------------------------------------------------------------

    /// 读一条校准记录（没有就 `None`）。
    pub fn get_region_profile(&self, kind: &str) -> Option<RegionProfile> {
        let conn = self.conn.lock();
        conn.query_row(
            "SELECT kind, x, y, w, h, source, text_height, learned_unix
             FROM region_profiles WHERE kind = ?1",
            params![kind],
            |row| {
                let source: String = row.get(5)?;
                Ok(RegionProfile {
                    kind: row.get(0)?,
                    rect: crate::exp::region::NormRect {
                        x: row.get(1)?,
                        y: row.get(2)?,
                        w: row.get(3)?,
                        h: row.get(4)?,
                    },
                    source: RegionSource::parse(&source).unwrap_or(RegionSource::Auto),
                    text_height: row.get(6)?,
                    learned_unix: row.get(7)?,
                })
            },
        )
        .ok()
        .filter(|profile| profile.rect.is_valid())
    }

    /// 保存（或覆盖）一条校准记录。
    pub fn save_region_profile(
        &self,
        profile: &RegionProfile,
        client: Option<(i32, i32)>,
    ) -> Result<RegionProfile> {
        let conn = self.conn.lock();
        let saved = RegionProfile {
            learned_unix: chrono::Utc::now().timestamp(),
            ..profile.clone()
        };
        let (client_w, client_h) = match client {
            Some((w, h)) => (Some(w), Some(h)),
            None => (None, None),
        };
        conn.execute(
            "INSERT INTO region_profiles
             (kind, x, y, w, h, source, text_height, client_w, client_h, learned_unix)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
             ON CONFLICT(kind) DO UPDATE SET
                x = excluded.x, y = excluded.y, w = excluded.w, h = excluded.h,
                source = excluded.source, text_height = excluded.text_height,
                client_w = excluded.client_w, client_h = excluded.client_h,
                learned_unix = excluded.learned_unix",
            params![
                saved.kind,
                saved.rect.x,
                saved.rect.y,
                saved.rect.w,
                saved.rect.h,
                saved.source.as_str(),
                saved.text_height,
                client_w,
                client_h,
                saved.learned_unix,
            ],
        )?;
        Ok(saved)
    }

    /// 清掉某个 kind 的校准记录（回自动识别）。
    pub fn clear_region_profiles(&self, kind: &str) -> Result<()> {
        let conn = self.conn.lock();
        conn.execute("DELETE FROM region_profiles WHERE kind = ?1", params![kind])?;
        Ok(())
    }

    // -----------------------------------------------------------------------
    // 经验统计
    // -----------------------------------------------------------------------

    /// 写入一段会话和它每分钟的采样点（同一个事务）。
    pub fn insert_exp_session(
        &self,
        session: &ExpSessionRow,
        points: &[ExpSampleRow],
    ) -> Result<i64> {
        let mut conn = self.conn.lock();
        let tx = conn.transaction()?;
        tx.execute(
            "INSERT INTO exp_sessions
             (started_at, ended_at, active_secs, start_level, end_level,
              start_exp, end_exp, start_percent, end_percent, gained_exp,
              start_cum, end_cum, quality, quality_reason, coverage, idle_ratio,
              rejected_frames, map_name)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14,
                     ?15, ?16, ?17, ?18)",
            params![
                session.started_unix,
                session.ended_unix,
                session.active_secs,
                session.start_level,
                session.end_level,
                session.start_exp,
                session.end_exp,
                session.start_percent,
                session.end_percent,
                session.gained_exp,
                session.start_cum,
                session.end_cum,
                session.quality,
                session.quality_reason,
                session.coverage,
                session.idle_ratio,
                session.rejected_frames,
                session.map_name,
            ],
        )?;
        let id = tx.last_insert_rowid();
        for point in points {
            tx.execute(
                "INSERT INTO exp_samples
                 (session_id, captured_at, offset_secs, level, exp, percent)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    id,
                    point.captured_unix,
                    point.offset_secs,
                    point.level,
                    point.exp,
                    point.percent,
                ],
            )?;
        }
        tx.commit()?;
        log::info!(
            "经验会话已入库：id={} 有效 {} 秒，{} 个采样点",
            id,
            session.active_secs.round(),
            points.len()
        );
        Ok(id)
    }

    /// 历史列表（最近在前）。
    pub fn exp_history(&self, limit: u32) -> Result<Vec<ExpHistoryRow>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
            "SELECT id, started_at, ended_at, active_secs, start_level, end_level,
                    start_percent, end_percent, gained_exp,
                    quality, quality_reason, coverage, idle_ratio, map_name
             FROM exp_sessions ORDER BY id DESC LIMIT ?1",
        )?;
        let rows = stmt.query_map(params![limit], |row| {
            Ok(ExpHistoryRow {
                id: row.get(0)?,
                started_unix: row.get(1)?,
                ended_unix: row.get(2)?,
                active_secs: row.get(3)?,
                start_level: row.get(4)?,
                end_level: row.get(5)?,
                start_percent: row.get(6)?,
                end_percent: row.get(7)?,
                gained_exp: row.get(8)?,
                quality: row.get(9)?,
                quality_reason: row.get(10)?,
                coverage: row.get(11)?,
                idle_ratio: row.get(12)?,
                map_name: row.get(13)?,
            })
        })?;
        let mut list = Vec::new();
        for item in rows {
            list.push(item?);
        }
        Ok(list)
    }

    /// 历史汇总：共多少段、累计有效时间、累计经验。
    ///
    /// 汇总用 SQL 现算而不是把列表加起来：列表只取最近 30 条，
    /// 拿它当总数会把「一共练了多少」报少。
    pub fn exp_totals(&self) -> Result<ExpTotals> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
            "SELECT COUNT(*), COALESCE(SUM(active_secs), 0), COALESCE(SUM(gained_exp), 0)
             FROM exp_sessions",
        )?;
        let totals = stmt.query_row([], |row| {
            Ok(ExpTotals {
                sessions: row.get(0)?,
                active_secs: row.get(1)?,
                gained_exp: row.get(2)?,
            })
        })?;
        Ok(totals)
    }

    /// 曲线用的采样点（每段会话的每分钟一个点）。
    pub fn exp_curve(&self, limit: u32) -> Result<Vec<ExpCurvePoint>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
            "SELECT captured_at, exp, percent, level FROM exp_samples
             ORDER BY captured_at DESC LIMIT ?1",
        )?;
        let rows = stmt.query_map(params![limit], |row| {
            Ok(ExpCurvePoint {
                captured_unix: row.get(0)?,
                exp: row.get(1)?,
                percent: row.get(2)?,
                level: row.get(3)?,
            })
        })?;
        let mut list = Vec::new();
        for item in rows {
            list.push(item?);
        }
        list.reverse();
        Ok(list)
    }

    /// 采样点保留上限：老的删掉，库不会一直长。
    pub fn purge_exp_samples(&self, retention_days: i64) -> Result<usize> {
        let conn = self.conn.lock();
        let cutoff = chrono::Utc::now().timestamp() - retention_days * 86_400;
        let removed = conn.execute(
            "DELETE FROM exp_samples WHERE captured_at < ?1",
            params![cutoff],
        )?;
        if removed > 0 {
            log::info!("清理了 {} 条过期经验采样点", removed);
        }
        Ok(removed)
    }

    /// 删掉历史里的一段（连同它的采样点，同一个事务）。返回这一段原来在不在。
    ///
    /// 地图名的记忆（`exp_maps`）不动：那是「这张图叫什么」，不属于某一段会话。
    pub fn delete_exp_session(&self, id: i64) -> Result<bool> {
        let mut conn = self.conn.lock();
        let tx = conn.transaction()?;
        let removed = tx.execute("DELETE FROM exp_sessions WHERE id = ?1", params![id])?;
        tx.execute("DELETE FROM exp_samples WHERE session_id = ?1", params![id])?;
        tx.commit()?;
        if removed > 0 {
            log::info!("经验历史删掉一段：id={}", id);
        }
        Ok(removed > 0)
    }

    /// 清空经验历史（会话 + 采样点）。
    pub fn clear_exp_history(&self) -> Result<usize> {
        let conn = self.conn.lock();
        let sessions = conn.execute("DELETE FROM exp_sessions", [])?;
        conn.execute("DELETE FROM exp_samples", [])?;
        log::info!("经验历史已清空（{} 段）", sessions);
        Ok(sessions)
    }

    /// 按小地图的像素指纹查地图名（查不到就是第一次来这张图）。
    pub fn get_exp_map(&self, fingerprint: &str) -> Option<String> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare("SELECT name FROM exp_maps WHERE fingerprint = ?1").ok()?;
        stmt.query_row(params![fingerprint], |row| row.get::<_, String>(0))
            .ok()
    }

    /// 记住「这个指纹是哪个地图」。
    pub fn set_exp_map(&self, fingerprint: &str, name: &str) -> Result<()> {
        let conn = self.conn.lock();
        conn.execute(
            "INSERT INTO exp_maps (fingerprint, name, updated_at) VALUES (?1, ?2, ?3)
             ON CONFLICT(fingerprint) DO UPDATE SET name = ?2, updated_at = ?3",
            params![fingerprint, name, chrono::Utc::now().timestamp()],
        )?;
        Ok(())
    }

    /// 已经认过的地图名（去重、按最近用的排在前）—— 输入框旁边的快捷按钮用。
    pub fn list_exp_maps(&self) -> Result<Vec<String>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
            "SELECT name FROM exp_maps GROUP BY name ORDER BY MAX(updated_at) DESC LIMIT 12",
        )?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
        Ok(rows.filter_map(|item| item.ok()).collect())
    }

    pub fn get_blacklist(&self) -> Result<Vec<BlacklistEntry>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
            "SELECT id, server, player_name, category, reason, created_at
             FROM blacklist ORDER BY id DESC",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(BlacklistEntry {
                id: Some(row.get(0)?),
                server: row.get(1)?,
                player_name: row.get(2)?,
                category: row.get(3)?,
                reason: row.get(4)?,
                created_at: row.get(5)?,
            })
        })?;

        let mut list = Vec::new();
        for item in rows {
            list.push(item?);
        }
        Ok(list)
    }

    /// 录入一条黑名单。
    ///
    /// **同名同服只留一条**（按归一化后的名字比，所以「张三 」和「张三」算同一个）：
    /// 反复录入同一个人时更新他的类别与原因，而不是堆出一串重复行 ——
    /// 那些重复行在导出给别人时也一样难看，而且容易让「共 N 条记录」这种
    /// 数字看起来比实际更权威。
    pub fn add_blacklist(&self, entry: &BlacklistEntry) -> Result<i64> {
        let conn = self.conn.lock();
        let key = crate::blacklist::normalize(&entry.player_name);

        let existing: Option<i64> = {
            let mut stmt = conn.prepare("SELECT id, player_name FROM blacklist WHERE server = ?1")?;
            let rows = stmt.query_map(params![entry.server], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
            })?;
            let mut found = None;
            for row in rows {
                let (id, name) = row?;
                if !key.is_empty() && crate::blacklist::normalize(&name) == key {
                    found = Some(id);
                    break;
                }
            }
            found
        };

        if let Some(id) = existing {
            conn.execute(
                "UPDATE blacklist SET category = ?1, reason = ?2, created_at = ?3 WHERE id = ?4",
                params![
                    entry.category,
                    entry.reason,
                    entry.created_at,
                    id
                ],
            )?;
            return Ok(id);
        }

        conn.execute(
            "INSERT INTO blacklist (server, player_name, category, reason, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                entry.server,
                entry.player_name,
                entry.category,
                entry.reason,
                entry.created_at
            ],
        )?;
        Ok(conn.last_insert_rowid())
    }

    pub fn remove_blacklist(&self, id: i64) -> Result<()> {
        let conn = self.conn.lock();
        conn.execute("DELETE FROM blacklist WHERE id = ?1", params![id])?;
        Ok(())
    }

    pub fn list_alert_history(&self, limit: u32) -> Vec<AlertHistoryRow> {
        let conn = self.conn.lock();
        let result = (|| {
            let mut stmt = conn.prepare(
                "SELECT id, kind, title, detail, fired_unix
                 FROM alert_history ORDER BY fired_unix DESC, id DESC LIMIT ?1",
            )?;
            let rows = stmt.query_map(params![limit], |row| {
                Ok(AlertHistoryRow {
                    id: row.get(0)?,
                    kind: row.get(1)?,
                    title: row.get(2)?,
                    detail: row.get(3)?,
                    fired_unix: row.get(4)?,
                })
            })?;
            rows.collect::<Result<Vec<_>>>()
        })();
        result.unwrap_or_else(|err| {
            log::error!("读取提醒历史失败：{err}");
            Vec::new()
        })
    }

    pub fn record_alert(&self, kind: &str, title: &str, detail: &str) -> Result<()> {
        let conn = self.conn.lock();
        conn.execute(
            "INSERT INTO alert_history (kind, title, detail, fired_unix) VALUES (?1, ?2, ?3, ?4)",
            params![kind, title, detail, chrono::Utc::now().timestamp()],
        )?;
        Ok(())
    }

    pub fn clear_alert_history(&self) -> Result<()> {
        let conn = self.conn.lock();
        conn.execute("DELETE FROM alert_history", [])?;
        Ok(())
    }

    /// 取缓存条目，连**写入时间**一起给出去。
    ///
    /// 界面要用它显示「这份数据是几秒/几分钟前的」，让用户自己判断
    /// 要不要点强制刷新——共享缓存曾经被别的实例写脏过，这种透明度很必要。
    pub fn get_cached_price(&self, key: &str, ttl_seconds: i64) -> Option<(String, i64)> {
        let conn = self.conn.lock();
        let now = chrono::Utc::now().timestamp();
        let mut stmt = conn
            .prepare("SELECT response_json, updated_at FROM price_cache WHERE cache_key = ?1")
            .ok()?;
        let mut rows = stmt.query(params![key]).ok()?;
        if let Some(row) = rows.next().ok()? {
            let json: String = row.get(0).ok()?;
            let updated: i64 = row.get(1).ok()?;
            if now - updated < ttl_seconds {
                return Some((json, updated));
            }
        }
        None
    }

    pub fn set_cached_price(&self, key: &str, json: &str) -> Result<()> {
        let conn = self.conn.lock();
        let now = chrono::Utc::now().timestamp();
        conn.execute(
            "DELETE FROM price_cache WHERE updated_at <= ?1",
            params![now - PRICE_CACHE_MAX_AGE_SEC],
        )?;
        conn.execute(
            "INSERT INTO price_cache (cache_key, response_json, updated_at)
             VALUES (?1, ?2, ?3)
             ON CONFLICT(cache_key) DO UPDATE SET response_json = ?2, updated_at = ?3",
            params![key, json, now],
        )?;
        Ok(())
    }
}

impl RegionStore for Database {
    fn load_region(&self, kind: &str) -> Option<RegionProfile> {
        self.get_region_profile(kind)
    }

    fn save_region(&self, profile: &RegionProfile) -> std::result::Result<(), String> {
        self.save_region_profile(profile, None)
            .map(|_| ())
            .map_err(|err| err.to_string())
    }

    fn clear_region(&self, kind: &str) -> std::result::Result<(), String> {
        self.clear_region_profiles(kind)
            .map_err(|err| err.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 两个测试各用各的库文件，别互相踩。
    fn temp_db(tag: &str) -> (Database, PathBuf) {
        let path = std::env::temp_dir().join(format!(
            "mxdbox-test-{}-{}.db",
            tag,
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);
        (Database::new(path.clone()).expect("建库失败"), path)
    }

    fn sample_row(quality: u8) -> ExpSessionRow {
        ExpSessionRow {
            started_unix: 1_700_000_000,
            ended_unix: 1_700_003_600,
            active_secs: 3600.0,
            start_level: Some(55),
            end_level: Some(56),
            start_exp: 427_096,
            end_exp: 30,
            start_percent: Some(46.09),
            end_percent: Some(0.0),
            gained_exp: 926_689 - 427_096 + 30,
            start_cum: exp_table_cum(55, 427_096),
            end_cum: exp_table_cum(56, 30),
            quality,
            quality_reason: "整段都能读到画面".to_string(),
            coverage: 0.98,
            idle_ratio: 0.2,
            rejected_frames: 0,
            map_name: "蚂蚁洞".to_string(),
        }
    }

    fn exp_table_cum(level: u32, exp: u64) -> Option<u64> {
        crate::exp::table::cumulative(level, exp)
    }

    fn profile(kind: &str, x: f64, y: f64, w: f64, h: f64, source: RegionSource) -> RegionProfile {
        RegionProfile {
            kind: kind.to_string(),
            rect: crate::exp::region::NormRect { x, y, w, h },
            source,
            text_height: Some(7),
            learned_unix: 0,
        }
    }

    /// 删一段只删这一段：别的会话、别的采样点、汇总数字都要跟着对得上。
    #[test]
    fn deleting_one_exp_session_leaves_the_others() {
        let (db, path) = temp_db("exp-delete-one");
        let point = |at: i64| ExpSampleRow {
            captured_unix: at,
            offset_secs: 60,
            level: Some(55),
            exp: 427_096,
            percent: 46.09,
        };
        let first = db
            .insert_exp_session(&sample_row(2), &[point(1), point(2)])
            .expect("写第一段失败");
        let second = db
            .insert_exp_session(&sample_row(1), &[point(3)])
            .expect("写第二段失败");

        assert!(db.delete_exp_session(first).expect("删除失败"));
        let rows = db.exp_history(10).expect("读历史失败");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, second);
        assert_eq!(db.exp_totals().expect("读汇总失败").sessions, 1);
        // 第一段的两个采样点跟着没了，第二段的还在
        let curve = db.exp_curve(100).expect("读曲线失败");
        assert_eq!(curve.len(), 1);
        assert_eq!(curve[0].captured_unix, 3);

        // 删一个不存在的：不报错，如实说没删到
        assert!(!db.delete_exp_session(first).expect("重复删除不该报错"));

        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn region_profiles_upsert_by_kind() {
        let (db, path) = temp_db("region-upsert");

        let auto = db
            .save_region_profile(&profile("exp_line", 0.01, 0.02, 0.3, 0.04, RegionSource::Auto), None)
            .expect("保存自动区域失败");
        assert_eq!(auto.source, RegionSource::Auto);

        let manual = db
            .save_region_profile(
                &profile("exp_line", 0.012, 0.022, 0.32, 0.044, RegionSource::Manual),
                Some((2560, 1440)),
            )
            .expect("覆盖成手动区域失败");
        assert_eq!(manual.source, RegionSource::Manual);
        assert!(manual.learned_unix > 0, "落库时要盖上时间戳");
        let loaded = db.get_region_profile("exp_line").expect("同 kind 应命中");
        assert_eq!(loaded.source, RegionSource::Manual);
        assert!((loaded.rect.w - 0.32).abs() < 1e-9);

        // 比例坐标与分辨率无关：另一个尺寸读到的还是这同一条记录
        assert!(db.get_region_profile("map_name").is_none());
        assert!(db.get_region_profile("exp_line").is_some());

        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn clearing_region_profiles_only_removes_the_requested_kind() {
        let (db, path) = temp_db("region-clear");
        db.save_region_profile(&profile("exp_line", 0.01, 0.02, 0.3, 0.04, RegionSource::Manual), None)
            .expect("保存经验区域失败");
        db.save_region_profile(&profile("map_name", 0.05, 0.05, 0.1, 0.02, RegionSource::Manual), None)
            .expect("保存地图区域失败");

        db.clear_region_profiles("exp_line")
            .expect("清除经验区域失败");

        assert!(db.get_region_profile("exp_line").is_none());
        assert!(db.get_region_profile("map_name").is_some());

        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn old_database_gets_the_region_profiles_table() {
        let path = std::env::temp_dir().join(format!(
            "mxdbox-test-region-migrate-{}.db",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);
        {
            let conn = Connection::open(&path).expect("建旧库失败");
            conn.execute_batch(
                "CREATE TABLE exp_maps (
                    fingerprint TEXT PRIMARY KEY,
                    name TEXT NOT NULL,
                    updated_at INTEGER NOT NULL
                );",
            )
            .expect("建旧地图表失败");
        }

        let db = Database::new(path.clone()).expect("旧库创建区域表失败");
        let saved = db
            .save_region_profile(&profile("exp_line", 0.01, 0.02, 0.3, 0.04, RegionSource::Manual), None)
            .expect("旧库写入区域失败");
        let loaded = db.get_region_profile("exp_line").expect("旧库中的区域应可读");
        assert_eq!(
            (loaded.rect, loaded.source, loaded.text_height),
            (saved.rect, saved.source, saved.text_height)
        );

        let _ = std::fs::remove_file(path);
    }

    /// 开发版那张「按尺寸 + DPI 分像素矩形」的表要能被整表换成比例表 ——
    /// 否则 `CREATE TABLE IF NOT EXISTS` 什么都不做，之后每次查询都失败，
    /// 用户看到的是「校准保存不了」。这里手工建出旧形状，再让 `Database::new` 修它。
    #[test]
    fn a_legacy_pixel_region_table_is_rebuilt_instead_of_breaking_writes() {
        let path = std::env::temp_dir().join(format!(
            "mxdbox-test-region-legacy-shape-{}.db",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);
        {
            let conn = Connection::open(&path).expect("建旧库失败");
            conn.execute_batch(
                "CREATE TABLE region_profiles (
                    kind TEXT NOT NULL,
                    client_w INTEGER NOT NULL,
                    client_h INTEGER NOT NULL,
                    window_dpi INTEGER NOT NULL,
                    x INTEGER NOT NULL,
                    y INTEGER NOT NULL,
                    w INTEGER NOT NULL,
                    h INTEGER NOT NULL,
                    source TEXT NOT NULL,
                    learned_unix INTEGER NOT NULL,
                    PRIMARY KEY(kind, client_w, client_h, window_dpi)
                );",
            )
            .expect("建旧校准表失败");
        }

        let db = Database::new(path.clone()).expect("旧表应被重建而不是让初始化失败");
        db.save_region_profile(&profile("exp_line", 0.01, 0.02, 0.3, 0.04, RegionSource::Auto), None)
            .expect("重建后的表应能写入");
        assert!(db.get_region_profile("exp_line").is_some());

        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn alert_history_is_newest_first_limited_and_clearable() {
        let (db, path) = temp_db("alert-history");
        db.record_alert("server_open", "开服提醒", "服务器正常")
            .expect("记录提醒失败");
        db.record_alert("clipboard", "黑名单命中", "这个名字在黑名单里")
            .expect("记录提醒失败");

        let recent = db.list_alert_history(1);
        assert_eq!(recent.len(), 1);
        assert_eq!(recent[0].kind, "clipboard");
        assert_eq!(recent[0].title, "黑名单命中");
        assert_eq!(recent[0].detail, "这个名字在黑名单里");
        assert!(recent[0].fired_unix > 0);

        db.clear_alert_history().expect("清理提醒历史失败");
        assert!(db.list_alert_history(10).is_empty());
        let _ = std::fs::remove_file(path);
    }

    /// 会话写进去再读出来，质量那几列不能丢（否则界面上的「这一段可不可信」就没了）。
    #[test]
    fn session_round_trips_with_its_quality() {
        let (db, path) = temp_db("round-trip");
        let row = sample_row(1);
        let id = db.insert_exp_session(&row, &[]).expect("写会话失败");
        assert_eq!(id, 1);

        let history = db.exp_history(10).expect("读历史失败");
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].quality, 1);
        assert_eq!(history[0].quality_reason, "整段都能读到画面");
        assert!((history[0].coverage - 0.98).abs() < 1e-9);
        assert!((history[0].idle_ratio - 0.2).abs() < 1e-9);
        assert_eq!(history[0].gained_exp, row.gained_exp);
        // 地图名要跟着这一行回来（小结卡片和历史列表都靠它）
        assert_eq!(history[0].map_name, "蚂蚁洞");

        let totals = db.exp_totals().expect("读汇总失败");
        assert_eq!(totals.sessions, 1);
        assert!((totals.active_secs - 3600.0).abs() < 1e-9);
        assert_eq!(totals.gained_exp, row.gained_exp);

        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn settings_patch_preserves_unpatched_values() {
        let (db, path) = temp_db("settings-patch");
        db.set_setting("auth_cookie", "fresh-cookie").expect("写 Cookie 失败");
        db.set_setting("hotkey", "Ctrl+Shift+K").expect("写快捷键失败");

        db.patch_settings(Some(false), Some(45), None, Some(false))
            .expect("局部设置更新失败");

        let settings = db.get_all_settings();
        assert_eq!(settings.auth_cookie, "fresh-cookie");
        assert_eq!(settings.hotkey, "Ctrl+Shift+K");
        assert!(!settings.monitor_enabled);
        assert_eq!(settings.monitor_interval_sec, 45);
        assert!(settings.close_to_tray);
        assert!(!settings.blacklist_watch);

        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn cache_write_prunes_rows_older_than_the_longest_ttl() {
        let (db, path) = temp_db("cache-retention");
        let now = chrono::Utc::now().timestamp();
        {
            let conn = db.conn.lock();
            conn.execute(
                "INSERT INTO price_cache (cache_key, response_json, updated_at) VALUES (?1, ?2, ?3)",
                params!["expired", "{}", now - PRICE_CACHE_MAX_AGE_SEC - 3600],
            )
            .expect("插入过期缓存失败");
            conn.execute(
                "INSERT INTO price_cache (cache_key, response_json, updated_at) VALUES (?1, ?2, ?3)",
                params!["recent", "{}", now - 23 * 3600],
            )
            .expect("插入近期缓存失败");
        }

        db.set_cached_price("fresh", "{}")
            .expect("写入缓存失败");
        let conn = db.conn.lock();
        let expired: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM price_cache WHERE cache_key = 'expired'",
                [],
                |row| row.get(0),
            )
            .expect("查询过期缓存失败");
        let recent: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM price_cache WHERE cache_key = 'recent'",
                [],
                |row| row.get(0),
            )
            .expect("查询近期缓存失败");
        let fresh: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM price_cache WHERE cache_key = 'fresh'",
                [],
                |row| row.get(0),
            )
            .expect("查询新缓存失败");
        assert_eq!(expired, 0);
        assert_eq!(recent, 1);
        assert_eq!(fresh, 1);
        drop(conn);

        let _ = std::fs::remove_file(path);
    }

    /// **老库升级**：0.1.3 建的 exp_sessions 没有质量/累计那几列，程序必须自己补上。
    /// 不补的话用户升完程序、打完一段、点「计入历史」时写库会失败。
    #[test]
    fn old_database_gets_the_new_columns() {
        let path = std::env::temp_dir().join(format!(
            "mxdbox-test-migrate-{}.db",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);

        // 先手工造一个「老版本」的库：只有旧列，并且已经有一段老数据
        {
            let conn = Connection::open(&path).expect("建老库失败");
            conn.execute_batch(
                "CREATE TABLE exp_sessions (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    started_at INTEGER NOT NULL,
                    ended_at INTEGER NOT NULL,
                    active_secs REAL NOT NULL,
                    start_level INTEGER,
                    end_level INTEGER,
                    start_exp INTEGER NOT NULL,
                    end_exp INTEGER NOT NULL,
                    start_percent REAL,
                    end_percent REAL,
                    gained_exp INTEGER NOT NULL
                );
                INSERT INTO exp_sessions
                    (started_at, ended_at, active_secs, start_exp, end_exp, gained_exp)
                VALUES (1, 2, 60.0, 100, 200, 100);",
            )
            .expect("造老库失败");
        }

        // 新版本打开它 → 自动补列
        let db = Database::new(path.clone()).expect("升级老库失败");
        let history = db.exp_history(10).expect("读老数据失败");
        assert_eq!(history.len(), 1, "老数据必须还在");
        assert_eq!(history[0].gained_exp, 100);
        // 老数据没有质量结论，按「数据完整」的默认值读出来（不做假结论：旧行本来就是
        // 在有自检的前提下记的，只是那时还没分级）
        assert_eq!(history[0].quality, 2);
        // 老行没有地图名：补列后读出来是空串，不能变成垃圾值
        assert_eq!(history[0].map_name, "");

        // 补完列之后要能正常写入新数据
        let mut row = sample_row(2);
        row.start_cum = None;
        row.end_cum = None;
        db.insert_exp_session(&row, &[]).expect("补列后写入失败");

        let _ = std::fs::remove_file(path);
    }
}
