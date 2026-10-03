use parking_lot::Mutex;
use reqwest::header::{HeaderMap, HeaderValue, ACCEPT, REFERER, RETRY_AFTER, USER_AGENT};
use reqwest::Client;
use scraper::{ElementRef, Html, Selector};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::time::Duration;

/// 小册子真实的区服表。
///
/// 站点上只有这 5 个服，拍卖工具的 `server` 参数就是这里的 `id`（1..5）。
/// 注意：**没有「全服」这个概念** —— 不传 `server` 时服务端只会回蓝蜗牛。
/// 之前 UI 里虚构了一个 `1 => 全服`，既漏掉了蓝蜗牛，又让「全服综合比价」
/// 实际上一直在查蓝蜗牛。
///
/// 这份清单是唯一数据源：前端通过 `get_server_list` 命令拿，Rust 侧直接用。
pub const SERVERS: [(u32, &str); 5] = [
    (1, "蓝蜗牛"),
    (2, "蘑菇仔"),
    (3, "绿水灵"),
    (4, "漂漂猪"),
    (5, "小白兔"),
];

/// 合法区服 id。保存「全局预设区服」时拿它校验：
/// 不校验的话，界面上存进一个不存在的 id，之后每次查询都只会静默返回空结果，
/// 看起来却像「站上没有这个道具」。
pub const SERVER_IDS: [u32; 5] = [1, 2, 3, 4, 5];

const MARKET_URL: &str = "https://mxdc.dvg.cn/tools/market/";
const PRICE_TREND_URL: &str = "https://mxdc.dvg.cn/tools/price-trend/";
const SITE_ORIGIN: &str = "https://mxdc.dvg.cn";

/// 掉落速查接口（公开、不需要登录）：怪物名 / 道具名 / 怪物ID / 道具ID 都走它。
///
/// 页面 `/tools/drops/` 是 Vue 前端渲染的，真正的数据在这里 —— 页面上的
/// `apiUrl` 就指向它（对着真实页面核对过）。
pub const DROP_SEARCH_API: &str = "https://mxdc.dvg.cn/api/drop-search.php";

/// 429 / 5xx 最多再补几次刀（**不含**第一次）：1s → 2s → 4s，退避总共 7 秒封顶。
///
/// 为什么必须封顶：`fetch` 的调用方自己还有重试逻辑（拍卖查询会换凭证、会原样重查），
/// 两边都“多试几次”叠起来就是对着一个不是我们自己的站点连发。
const MAX_HTTP_RETRY: u32 = 3;

/// 站点让我们等多久就等多久的**上限**。`Retry-After` 写 60 秒也只等 10 秒 ——
/// 这是个交互式查询，用户对着转圈等到天荒地老，还不如拿一句「稍后再试」。
const MAX_RETRY_AFTER_SEC: u64 = 10;

/// 「找到 0 个」之后隔多久再问一次。
///
/// 以前是几毫秒内立刻重发：站点“数据还没就绪”这个状态根本来不及变，两次拿到的
/// 多半还是同一句「找到 0 个」，重试等于白发一个请求（对站点是纯浪费）。
/// 隔小半秒再问，才有机会真的问到东西；超过一秒又会让用户觉得卡住了。
const EMPTY_RETRY_DELAY_MS: u64 = 400;

/// 掉落数据是**静态资料**（不像金价会变），所以可以放心缓存久一点。
/// 缓存由 `commands::search_drops` 负责，这里只定 TTL。
pub const DROP_CACHE_TTL_SEC: i64 = 3600;

/// 道具图鉴页（掉落结果里的道具、怪物都能点回站点看详情）。
pub fn item_page_url(item_id: i64) -> String {
    format!("{}/item_info.php?id={}", SITE_ORIGIN, item_id)
}

/// 怪物图鉴页。
pub fn mob_page_url(mob_id: i64) -> String {
    format!("{}/mob_info.php?id={}", SITE_ORIGIN, mob_id)
}

/// 地图页面。
pub fn map_page_url(map_id: i64) -> String {
    format!("{}/map_info.php?id={}", SITE_ORIGIN, map_id)
}

/// 掉落查询里的一件道具。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DropItemRef {
    // 站点给的键是 `itemid`，给前端的是 `itemId` —— 读写分开指定，两边都干净。
    #[serde(rename(deserialize = "itemid", serialize = "itemId"), default)]
    pub item_id: i64,
    #[serde(default)]
    pub name: String,
    /// 图标地址（取回来时是 `/dbsource/...`，解析后统一补成绝对地址）
    #[serde(default)]
    pub icon: String,
    #[serde(default)]
    pub req_level: u32,
    #[serde(default)]
    pub main_category: String,
    #[serde(default)]
    pub sub_category: String,
    /// 小册子上的道具图鉴页（取回来的响应里没有，由 `finalize_drop_result` 补上）。
    ///
    /// 放在后端拼是故意的：站点的页面路径只有一处需要维护，前端不用知道
    /// `/item_info.php?id=` 这种约定。
    #[serde(default)]
    pub page_url: String,
}

/// 掉落查询里的一只怪物。
///
/// ⚠️ 字段类型是**跟着站点实测来的**，别想当然：
/// * `hp` / `exp` / `pad` / `mad` 是**字符串**，而且不少老怪物整块数据缺失；
/// * `boss` 是数字 `0/1`，不是布尔（所以用 `flexible_flag` 解）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DropMob {
    #[serde(rename(deserialize = "mobid", serialize = "mobId"), default)]
    pub mob_id: i64,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub level: u32,
    #[serde(default, deserialize_with = "flexible_flag")]
    pub boss: bool,
    #[serde(default)]
    pub category_label: String,
    #[serde(default)]
    pub icon: String,
    #[serde(default)]
    pub hp: String,
    #[serde(default)]
    pub exp: String,
    #[serde(default)]
    pub pad: String,
    #[serde(default)]
    pub mad: String,
    /// 小册子上的怪物图鉴页（由 `finalize_drop_result` 补上）
    #[serde(default)]
    pub page_url: String,
}

/// 一只怪物的一条掉落。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DropEntry {
    pub item: DropItemRef,
    /// 站点内部的概率值（万分比之类，不要自己换算成百分比展示）
    #[serde(default)]
    pub chance: u32,
    /// 站点已经算好的可读概率，例如 `"0.007%"` —— 界面**直接用这个**，
    /// 免得我们自己换算出和站点不一致的数字。
    #[serde(default)]
    pub chance_text: String,
    #[serde(default)]
    pub min: u32,
    #[serde(default)]
    pub max: u32,
    /// 这条掉落是否命中了你查的那个关键词（站点给的，用来高亮）
    #[serde(default, deserialize_with = "flexible_flag")]
    pub matched: bool,
    #[serde(default)]
    pub questid: u32,
}

/// 怪物会出没的一张地图。名字里自带怪物数量，例如「智慧森林（30只）」。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DropMap {
    #[serde(rename(deserialize = "mapid", serialize = "mapId"), default)]
    pub map_id: i64,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub street: String,
    #[serde(default)]
    pub icon: String,
    /// 小册子上的地图页（由 `finalize_drop_result` 补上）
    #[serde(default)]
    pub page_url: String,
}

/// 一只怪物的完整结果：它掉什么、在哪刷。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DropMobResult {
    pub mob: DropMob,
    #[serde(default)]
    pub drops: Vec<DropEntry>,
    #[serde(default)]
    pub maps: Vec<DropMap>,
    #[serde(default)]
    pub matched_drop_count: u32,
    /// 站点给的理由，例如 `["道具掉落"]`
    #[serde(default)]
    pub reasons: Vec<String>,
}

/// 关键词命中的道具 / 怪物（让用户能直接点进正确的那个，而不是靠猜）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DropMatches {
    #[serde(default)]
    pub items: Vec<DropItemRef>,
    #[serde(default)]
    pub mobs: Vec<DropMob>,
    #[serde(default)]
    pub item_total: u32,
    #[serde(default)]
    pub mob_total: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DropMeta {
    #[serde(default)]
    pub keyword: String,
    #[serde(default)]
    pub page: u32,
    #[serde(default)]
    pub page_size: u32,
    #[serde(default)]
    pub total: u32,
    #[serde(default)]
    pub total_pages: u32,
    #[serde(default)]
    pub drop_total: u32,
    #[serde(default, deserialize_with = "flexible_flag")]
    pub boss: bool,
    #[serde(default)]
    pub source: String,
    #[serde(default)]
    pub source_label: String,
}

/// 一次掉落查询的完整响应。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DropSearch {
    #[serde(default)]
    pub ok: bool,
    pub meta: DropMeta,
    pub matches: DropMatches,
    #[serde(default)]
    pub results: Vec<DropMobResult>,
}

/// 宽容地把 `0/1`、`true/false`、`"1"/""` 都读成布尔。
///
/// 站点的同一个语义在不同字段里是不同写的（详情页 `boss: 1`、另一处 `boss: true`），
/// 一旦按死一种类型，站点哪天换个写法就会**整个响应解析失败** —— 对查询类功能来说
/// 这比少一个字段糟得多。
fn flexible_flag<'de, D>(deserializer: D) -> Result<bool, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = Option::<serde_json::Value>::deserialize(deserializer)?;
    Ok(match value {
        Some(serde_json::Value::Bool(flag)) => flag,
        Some(serde_json::Value::Number(number)) => number.as_f64().unwrap_or(0.0) != 0.0,
        Some(serde_json::Value::String(text)) => {
            let trimmed = text.trim();
            !trimmed.is_empty() && trimmed != "0" && trimmed != "false"
        }
        _ => false,
    })
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MarketItem {
    pub id: String,
    pub name: String,
    pub icon_url: String,
    pub server_name: String,
    pub lowest_price: String,
    pub detail_link: String,
    pub category: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MesoRate {
    pub server_name: String,
    pub wan_rate: String,      // 1 元 = X 万金
    pub yuan_rate: String,     // 1 万金 = Y 元
    pub package_price: String, // 商品价格（元）
    pub stock: String,         // 库存
}

/// 历史快照里的一个时间点：把该时刻所有区服的“1 元可兑换万金”摊平成一个 map，
/// 这样前端直接喂给折线图就是「一条线一个区服」。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MesoHistoryPoint {
    /// 展示用时间标签，例如 "09-22 01:00"
    pub at: String,
    /// 原始 RFC3339 时间戳
    pub raw: String,
    /// 区服名 -> 1 元可兑换万金
    pub values: HashMap<String, f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MesoHistory {
    /// 折线顺序（固定按 SERVERS 排列，保证颜色稳定）
    pub series: Vec<String>,
    pub hour: Vec<MesoHistoryPoint>,
    pub day: Vec<MesoHistoryPoint>,
}

/// 金价页一次要的全部东西：当前报价 + 历史走势。
/// 两者本来就来自同一个页面的同一段内联 JSON，所以一次取回、一次缓存。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MesoReport {
    pub latest: Vec<MesoRate>,
    pub history: MesoHistory,
}

/// 图鉴页上的一条「标签 / 数值」，例如 装备分类 = 盾牌、需求等级 = 10。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ItemFact {
    pub label: String,
    pub value: String,
}

/// 装备的基准属性，例如 物理防御力 +10（9-12）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ItemProperty {
    pub label: String,
    pub value: String,
    /// 游戏内的随机浮动范围，例如 "(9-12)"
    #[serde(default)]
    pub range: Option<String>,
}

/// 小册子道具图鉴上的官方资料。装备就是它的基准属性，另外都带 NPC 出售价格。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ItemDetail {
    pub id: String,
    pub name: String,
    pub icon_url: String,
    pub equipment: bool,
    /// 需求等级 / 力量 / 敏捷 / 智力 / 运气 / 人气（数值为 0 的不返回，少一堆噪音）
    pub reqs: Vec<ItemFact>,
    /// 可装备的职业
    pub jobs: Vec<String>,
    /// 其它固定字段，例如 装备分类 = 盾牌
    pub facts: Vec<ItemFact>,
    /// 基准属性（物理/魔法防御力、攻击力、……）
    pub props: Vec<ItemProperty>,
    /// 例如「可使用卷轴次数：7 次」
    pub upgrade: String,
    /// 例如「出售价格：2,000 金币」，或「无法出售或价格未知」
    pub sell_price: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ServerInfo {
    pub id: u32,
    pub name: String,
}

pub fn server_list() -> Vec<ServerInfo> {
    SERVERS
        .iter()
        .map(|(id, name)| ServerInfo {
            id: *id,
            name: (*name).to_string(),
        })
        .collect()
}

pub struct MxdcClient {
    client: Client,
    /// 拍卖工具页里隐藏的查询凭证。服务端要求带上它，否则只返回
    /// 「查询凭证已过期，请重新提交一次。」并且一条结果都没有。
    market_token: Mutex<Option<String>>,
}

/// 每个请求都要带的默认头（UA / Referer / Accept）。
///
/// 抽成一个函数是给单测用的：`new()` 里那句 `.expect()` 得有人验证 —— 单测拿
/// 同一份头去 `build()`，证明这份配置建得起来，它才不是悬在启动路径上的一颗雷。
fn site_headers() -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(
        USER_AGENT,
        HeaderValue::from_static("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/133.0.0.0 Safari/537.36"),
    );
    headers.insert(
        REFERER,
        HeaderValue::from_static("https://mxdc.dvg.cn/"),
    );
    headers.insert(
        ACCEPT,
        HeaderValue::from_static("text/html,application/xhtml+xml,application/xml;q=0.9,image/avif,image/webp,*/*;q=0.8"),
    );
    headers
}

impl MxdcClient {
    pub fn new() -> Self {
        let client = Client::builder()
            .default_headers(site_headers())
            .timeout(Duration::from_secs(15))
            .build()
            // 为什么是 expect，而不是原来那个 `unwrap_or_default()` 兜底：
            // 默认出来的 Client **没有**上面那组 UA/Referer，也**没有** 15 秒超时 ——
            // 静默降级的后果是请求带着 reqwest 的默认 UA 出去（更容易被风控）
            // 而且命令可能挂很久才回错，排查时还看不出客户端已经不是原来那个。
            // 建不成只可能是请求头写错了，那是启动就该看见的错误，不该拖到运行期变成谜。
            .expect("构建小册子 HTTP 客户端失败：默认请求头配置有问题");

        Self {
            client,
            market_token: Mutex::new(None),
        }
    }

    async fn fetch(&self, url: &str, cookie: &str) -> Result<String, String> {
        // 429 / 5xx 在这里**就地退避重试**，调用方（拍卖 / 图鉴 / 金价）不用各写一套：
        // 它们本来就有自己的重试逻辑（换凭证、原样重查），两边叠起来才会失控。
        // 封顶次数写死在 MAX_HTTP_RETRY，而不是“直到成功为止”。
        let mut failures = 0u32;

        loop {
            let mut req = self.client.get(url);
            if !cookie.is_empty() {
                req = req.header("Cookie", cookie);
            }

            let resp = req.send().await.map_err(|e| format!("网络请求失败: {}", e))?;
            let status = resp.status();
            let code = status.as_u16();

            // 401 不重试：重发一万次也不会变，直接把“去扫码”这句话给用户。
            if code == 401 {
                return Err(http_status_error(code, url));
            }

            if status.is_server_error() || code == 429 {
                failures += 1;
                let retry_after = resp
                    .headers()
                    .get(RETRY_AFTER)
                    .and_then(|value| value.to_str().ok());
                match retry_delay(failures, retry_after) {
                    Some(wait) => {
                        log::warn!(
                            "小册子返回 HTTP {}（{}），等 {:.0} 秒后重试（第 {} 次）",
                            code,
                            url,
                            wait.as_secs_f64(),
                            failures
                        );
                        tokio::time::sleep(wait).await;
                        continue;
                    }
                    // 站点说等就等、次数到顶就认输：这时候的错误信息要把状态码和
                    // “到底该怎么办”一起给出来，别只留一个 HTTP 数字。
                    None => return Err(http_status_error(code, url)),
                }
            }

            if !status.is_success() {
                return Err(http_status_error(code, url));
            }

            let header = |name: &str| {
                resp.headers()
                    .get(name)
                    .and_then(|value| value.to_str().ok())
                    .unwrap_or("")
                    .to_string()
            };
            let content_type = header("content-type");
            let content_encoding = header("content-encoding");
            let content_length = header("content-length");

            let body = resp
                .text()
                .await
                .map_err(|e| format!("解析响应失败: {}", e))?;

            // 这几项是排查「页面里什么都解析不到」的关键证据：
            // 小册子的结果区在 180KB 页面的最末 12KB，只要拿到的不是正常结果页
            // （被压缩成一堆乱码、被风控页替换、连接被截断），就会「没卡片也没报错」，
            // 看起来和「真的没匹配到」一模一样。
            log::info!(
                "响应 HTTP {} 解出 {} 字节 content-type={:?} content-encoding={:?} content-length={:?} 像HTML={}",
                code,
                body.len(),
                content_type,
                content_encoding,
                content_length,
                looks_like_html(&body)
            );

            return Ok(body);
        }
    }

    /// 小册子「开服监控」接口：只回一小段 JSON，不需要登录。
    ///
    /// 单独一个方法、不复用 `fetch`：那个方法是为 180KB 的 HTML 结果页写的
    /// （会打印 content-type / 编码 / 像不像 HTML 一整套诊断日志），而这里是
    /// 每分钟轮询一次的监控接口，没必要跟着刷日志。
    ///
    /// 解析交给 `mxdc_monitor::parse_snapshot`，这里只负责把原始 JSON 取回来。
    pub async fn fetch_server_monitor(&self) -> Result<String, String> {
        let resp = self
            .client
            .get(crate::mxdc_monitor::MONITOR_API)
            .header(ACCEPT, "application/json, text/plain, */*")
            .timeout(Duration::from_secs(10))
            .send()
            .await
            .map_err(|e| format!("网络请求失败: {}", e))?;

        let status = resp.status();
        if !status.is_success() {
            // 监控接口不需要登录，但状态码的“人话”是共用的（403 风控 / 5xx 站点故障），
            // 三处各写一句迟早会漏掉同一批状态码。
            return Err(http_status_error(
                status.as_u16(),
                crate::mxdc_monitor::MONITOR_API,
            ));
        }

        let body = resp
            .text()
            .await
            .map_err(|e| format!("读取开服监控响应失败: {}", e))?;

        // 站点偶尔会用一整页 HTML 错误页顶掉 JSON（风控 / 网关错误），
        // 那种情况直接说清楚，别让解析器去猜“是不是 JSON”。
        if looks_like_html(&body) {
            return Err(format!(
                "开服监控接口回了一段 HTML（{} 字节），不是预期的 JSON",
                body.len()
            ));
        }

        Ok(body)
    }

    /// 取一个**只回 JSON** 的公开接口（掉落速查这类）。
    ///
    /// 不直接复用 `fetch`：那是为 180KB 的 HTML 结果页写的，每次都会打一整套
    /// content-type / 编码 / 像不像 HTML 的诊断日志，而这类小接口查一次只回几 KB，
    /// 那行日志只会把真正有用的记录冲掉。
    async fn fetch_json(&self, url: &str) -> Result<String, String> {
        let resp = self
            .client
            .get(url)
            .header(ACCEPT, "application/json, text/plain, */*")
            .header("X-Requested-With", "XMLHttpRequest")
            .timeout(Duration::from_secs(15))
            .send()
            .await
            .map_err(|e| format!("网络请求失败: {}", e))?;

        let status = resp.status();
        if !status.is_success() {
            return Err(http_status_error(status.as_u16(), url));
        }

        let body = resp
            .text()
            .await
            .map_err(|e| format!("读取响应失败: {}", e))?;

        // 站点偶尔会用一整页 HTML 错误页顶掉 JSON（风控 / 网关错误），
        // 先说清楚，别让解析器去猜「是不是 JSON」。
        if looks_like_html(&body) {
            return Err(format!(
                "接口回了一段 HTML（{} 字节），不是预期的 JSON",
                body.len()
            ));
        }

        Ok(body)
    }

    /// 掉落速查：怪物名 / 道具名 / 怪物ID / 道具ID 都走这一个接口。
    ///
    /// 公开接口、不需要登录 —— 朋友拿到工具没登录也能用。
    pub async fn search_drops(&self, keyword: &str, page: u32) -> Result<DropSearch, String> {
        let url = format!(
            "{}?q={}&page={}",
            DROP_SEARCH_API,
            urlencoding::encode(keyword),
            page.max(1)
        );
        let body = self.fetch_json(&url).await?;

        let mut parsed: DropSearch = serde_json::from_str(&body).map_err(|e| {
            format!(
                "解析掉落数据失败: {}（响应 {} 字节，页面结构可能已变化）",
                e,
                body.len()
            )
        })?;

        if !parsed.ok {
            return Err("小册子掉落接口回了 ok=false，请稍后再试。".to_string());
        }

        finalize_drop_result(&mut parsed);
        Ok(parsed)
    }

    /// 取拍卖查询凭证 `st`（服务端渲染在工具页的 `input[name=st]` 里）。
    /// 取到后会缓存复用；`force_refresh` 用于服务端判定凭证过期时重取。
    async fn market_token(&self, cookie: &str, force_refresh: bool) -> Result<String, String> {
        if !force_refresh {
            if let Some(cached) = self.market_token.lock().clone() {
                return Ok(cached);
            }
        }

        let html = self.fetch(MARKET_URL, cookie).await?;
        let doc = Html::parse_document(&html);
        let sel = Selector::parse("input[name=st]").unwrap();

        let token = doc
            .select(&sel)
            .next()
            .and_then(|e| e.value().attr("value"))
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty())
            .ok_or_else(|| {
                // 取不到凭证通常不是“改版”：站点 302 到登录页时最终是 200，
                // 页面上自然没有那个 input ——先问是不是登录页，再退回去猜结构。
                login_page_reason(&html).unwrap_or_else(|| {
                    "未能从小册子拍卖页取得查询凭证（未登录或页面结构已变化）。".to_string()
                })
            })?;

        *self.market_token.lock() = Some(token.clone());
        Ok(token)
    }

    pub async fn search_market(
        &self,
        keyword: &str,
        server_id: u32,
        cookie: &str,
    ) -> Result<Vec<MarketItem>, String> {
        // 一次查询的节奏（每一步都有理由，也都有上限）：
        //  1. 正常那次；
        //  2. 服务端说「找到 0 个匹配道具」时**隔小半秒**再原样查一次 —— 小册子偶发
        //     假阴性（自身数据没准备好时也回「找到 0 个」），不等一下就重发，
        //     拿到的还是同一句；
        //  3. 页面不对（凭证过期 / 登录页 / 乱码）时重取凭证再查一次。
        //
        // MAX_MARKET_ATTEMPTS 是硬顶：逻辑上三次到头，但**写死一个上限**才叫封顶 ——
        // 哪天有人再往里加一个 continue，也不会变成对着站点无限连发。
        const MAX_MARKET_ATTEMPTS: u32 = 3;
        let mut refreshed_token = false;
        let mut retried_empty = false;
        let mut attempt = 0;

        loop {
            attempt += 1;
            if attempt > MAX_MARKET_ATTEMPTS {
                return Err(format!(
                    "拍卖查询连试 {} 次都没拿到结果（q={:?} server={}），稍后再试",
                    MAX_MARKET_ATTEMPTS, keyword, server_id
                ));
            }

            let token = self.market_token(cookie, refreshed_token).await?;

            // 注意 URL 形状：参数名是 `st` / `server` / `q`。
            // 以前的 `server_id=` 是不存在的参数名，再叠上缺 `st`，
            // 服务端永远只回一句「查询凭证已过期」，于是页面里一条
            // `.market-result-card` 都没有 —— 前端就显示「暂无查询数据」。
            let url = format!(
                "{}?st={}&server={}&q={}",
                MARKET_URL,
                urlencoding::encode(&token),
                server_id,
                urlencoding::encode(keyword)
            );

            let html = self.fetch(&url, cookie).await?;
            let items = parse_market_cards(&html);

            if !items.is_empty() {
                log::info!(
                    "拍卖查询 server={} q={:?} 第{}次：{} 条结果",
                    server_id,
                    keyword,
                    attempt,
                    items.len()
                );
                return Ok(items);
            }

            // 一条结果卡都没有，要分清三种情况：
            //  · 服务端明确说「找到 0 个匹配道具」—— 真的没匹配到，正常返回空；
            //  · 服务端给了错误提示（凭证过期 / 风控）—— 重取凭证再试一次；
            //  · 服务端给的压根不是结果页（连「找到 N 个匹配道具」都没有，
            //    常见于乱码页、被拦截页）—— 同样重取一次。
            //
            // 后两种以前会被静默当成「没查到」返回空数组，于是被上层当结果缓存 5 分钟，
            // 界面就一直显示「暂无查询数据」。宁可报错也不能静默空洞。
            let summary = market_summary(&html);
            let notice = extract_market_error(&html);

            log::info!(
                "拍卖查询 server={} q={:?} 第{}次无卡片：{} 字节，summary={:?}，notice={:?}",
                server_id,
                keyword,
                attempt,
                html.len(),
                summary.as_deref().unwrap_or("(无)"),
                notice.as_deref().unwrap_or("(无)")
            );

            match classify_empty_page(
                &html,
                notice.as_deref(),
                summary.as_deref(),
                retried_empty,
                refreshed_token,
            ) {
                EmptyPage::RetryEmpty => {
                    retried_empty = true;
                    log::info!(
                        "拍卖查询 server={} q={:?} 第{}次服务端说「{}」，等 {}ms 原样重试一次",
                        server_id,
                        keyword,
                        attempt,
                        summary.as_deref().unwrap_or_default(),
                        EMPTY_RETRY_DELAY_MS
                    );
                    // 间隔不是为了礼貌，是为了有用：站点“数据没就绪”这个状态
                    // 几毫秒内不会变，立刻重发拿到的还是那句「找到 0 个」。
                    tokio::time::sleep(Duration::from_millis(EMPTY_RETRY_DELAY_MS)).await;
                    continue;
                }
                EmptyPage::ReturnEmpty => {
                    log::info!(
                        "拍卖查询 server={} q={:?} 两次都是「{}」，按没有匹配道具处理",
                        server_id,
                        keyword,
                        summary.as_deref().unwrap_or_default()
                    );
                    return Ok(Vec::new());
                }
                EmptyPage::RefreshToken(reason) => {
                    log::warn!(
                        "拍卖查询 server={} q={:?} 第{}次异常（{}），重取凭证再试一次",
                        server_id,
                        keyword,
                        attempt,
                        reason
                    );
                    refreshed_token = true;
                }
                // 换了凭证还是不对：报错。**不能**静默返回空 —— 上层会把空结果
                // 当“查不到”缓存 5 分钟，两个窗口就都查不到东西了。
                EmptyPage::Fail(reason) => {
                    log::warn!(
                        "拍卖查询 server={} q={:?} 重取凭证后仍然失败：{}",
                        server_id,
                        keyword,
                        reason
                    );
                    return Err(reason);
                }
            }
        }
    }

    /// 取道具图鉴里的官方资料（装备即基准属性 + NPC 出售价）。
    ///
    /// 图鉴页 `item_info.php` 是公开的，不需要登录凭证。
    pub async fn get_item_detail(&self, item_id: &str) -> Result<ItemDetail, String> {
        let url = format!(
            "{}/item_info.php?id={}",
            SITE_ORIGIN,
            urlencoding::encode(item_id)
        );
        let html = self.fetch(&url, "").await?;

        parse_item_detail(&html, item_id).ok_or_else(|| {
            // 登录页检测要排在“页面结构可能已变化”**之前**：cookie 失效时站点
            // 302 到登录页、最终状态码是 200，照旧报“改版了”会让人去找一场
            // 根本没发生的改版。
            login_page_reason(&html).unwrap_or_else(|| {
                format!(
                    "没能从道具图鉴页解析出 {} 的资料（页面结构可能已变化，像HTML={}）",
                    item_id,
                    looks_like_html(&html)
                )
            })
        })
    }

    pub async fn get_meso_report(&self, cookie: &str) -> Result<MesoReport, String> {
        let html = self.fetch(PRICE_TREND_URL, cookie).await?;

        // 这个页面的报价表是前端渲染的（<tbody id="pt-table"> 在 HTML 里是空的），
        // 真正的数据（当前报价 + 历史快照）都以内联 JSON 的形式挂在
        // window.PRICE_TREND_DATA 上。
        let json = extract_price_trend_json(&html).ok_or_else(|| {
            login_page_reason(&html).unwrap_or_else(|| {
                "未能在金价页面找到内联数据 window.PRICE_TREND_DATA（页面结构可能已变化）。"
                    .to_string()
            })
        })?;

        let root: PriceTrendRoot = serde_json::from_str(&json)
            .map_err(|e| format!("解析金价数据失败: {}", e))?;

        let latest = rates_from_items(&root.latest.items);
        if latest.is_empty() {
            return Err("小册子金价接口返回了空数据，请稍后再试。".to_string());
        }

        let history = MesoHistory {
            series: SERVERS.iter().map(|(_, name)| (*name).to_string()).collect(),
            hour: history_points(&root.history_hour, "%m-%d %H:%M"),
            day: history_points(&root.history_day, "%m-%d"),
        };

        Ok(MesoReport { latest, history })
    }
}

/// 收尾：把相对图标补成绝对地址 + 拼好各条记录在小册子上的页面地址。
///
/// 站点给的是 `/dbsource/icon/item/1092008.png`，而界面上的 `<img src>` 需要一个
/// 能直接请求的地址 —— 前端不该知道站点域名怎么拼。
fn finalize_drop_result(result: &mut DropSearch) {
    for item in result.matches.items.iter_mut() {
        item.icon = absolute_url(&item.icon);
        if item.item_id > 0 {
            item.page_url = item_page_url(item.item_id);
        }
    }
    for mob in result.matches.mobs.iter_mut() {
        mob.icon = absolute_url(&mob.icon);
        if mob.mob_id > 0 {
            mob.page_url = mob_page_url(mob.mob_id);
        }
    }
    for hit in result.results.iter_mut() {
        hit.mob.icon = absolute_url(&hit.mob.icon);
        if hit.mob.mob_id > 0 {
            hit.mob.page_url = mob_page_url(hit.mob.mob_id);
        }
        for drop in hit.drops.iter_mut() {
            drop.item.icon = absolute_url(&drop.item.icon);
            if drop.item.item_id > 0 {
                drop.item.page_url = item_page_url(drop.item.item_id);
            }
        }
        for map in hit.maps.iter_mut() {
            map.icon = absolute_url(&map.icon);
            if map.map_id > 0 {
                map.page_url = map_page_url(map.map_id);
            }
        }
    }
}

fn rates_from_items(items: &[PriceTrendItem]) -> Vec<MesoRate> {
    items
        .iter()
        .filter_map(|item| {
            let quote = item.quote.as_ref()?;
            Some(MesoRate {
                server_name: item.area_name.clone(),
                wan_rate: fmt_num(quote.yuan_to_wan_gold),
                yuan_rate: fmt_num(quote.wan_gold_to_yuan),
                package_price: fmt_num(quote.price_yuan),
                stock: quote.stock.map(|s| s.to_string()).unwrap_or_default(),
            })
        })
        .collect()
}

/// 把历史快照转成图上的点。站点给的数据未必按时间排序，这里统一排好。
fn history_points(snapshots: &[PriceTrendSnapshot], label_format: &str) -> Vec<MesoHistoryPoint> {
    let mut parsed: Vec<(chrono::DateTime<chrono::FixedOffset>, &PriceTrendSnapshot)> = snapshots
        .iter()
        .filter_map(|snapshot| {
            chrono::DateTime::parse_from_rfc3339(&snapshot.collected_at)
                .ok()
                .map(|at| (at, snapshot))
        })
        .collect();

    parsed.sort_by(|a, b| a.0.cmp(&b.0));

    parsed
        .into_iter()
        .map(|(at, snapshot)| {
            let values = snapshot
                .items
                .iter()
                .filter_map(|item| {
                    let quote = item.quote.as_ref()?;
                    let value = quote.yuan_to_wan_gold?;
                    Some((item.area_name.clone(), value))
                })
                .collect();

            MesoHistoryPoint {
                at: at.format(label_format).to_string(),
                raw: at.to_rfc3339(),
                values,
            }
        })
        .collect()
}

// ---------------------------------------------------------------- 解析辅助

fn parse_market_cards(html: &str) -> Vec<MarketItem> {
    let doc = Html::parse_document(html);

    let card_sel = Selector::parse("a.market-result-card").unwrap();
    let title_sel = Selector::parse(".market-item-title strong").unwrap();
    let category_sel = Selector::parse(".market-item-title em").unwrap();
    let price_sel = Selector::parse(".market-price-block strong").unwrap();
    let server_sel = Selector::parse(".market-price-block small").unwrap();
    let img_sel = Selector::parse(".market-item-icon img").unwrap();

    doc.select(&card_sel)
        .map(|card| {
            let detail_link = card
                .value()
                .attr("href")
                .map(absolute_url)
                .unwrap_or_default();

            // `.market-price-block small` 形如「蓝蜗牛 最低价」，取第一个词就是区服名。
            let server_name = text_of(&card, &server_sel)
                .split_whitespace()
                .next()
                .unwrap_or_default()
                .to_string();

            let category = text_of(&card, &category_sel);

            MarketItem {
                id: query_param(&detail_link, "id").unwrap_or_default(),
                name: text_of(&card, &title_sel),
                icon_url: card
                    .select(&img_sel)
                    .next()
                    .and_then(|e| e.value().attr("src"))
                    .map(absolute_url)
                    .unwrap_or_default(),
                server_name,
                // 可能是「598,888 金币」，也可能是「暂无在售」
                lowest_price: text_of(&card, &price_sel),
                detail_link,
                category: if category.is_empty() {
                    "道具".to_string()
                } else {
                    category
                },
            }
        })
        .collect()
}

/// 结果页头部那句「绿水灵 · 找到 2 个匹配道具」。
///
/// 用它区分「真的没有匹配道具」和「服务端返回的压根不是结果页」——
/// 后者以前会被静默当成「没查到」，还会被缓存 5 分钟。
fn market_summary(html: &str) -> Option<String> {
    let doc = Html::parse_document(html);
    let sel = Selector::parse("p").ok()?;

    doc.select(&sel).find_map(|element| {
        let text = element
            .text()
            .collect::<Vec<_>>()
            .join("")
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");

        (text.contains('·') && text.contains("找到")).then_some(text)
    })
}

/// 粗略判断一段响应像不像 HTML。
///
/// 被 reqwest 当文本错误解码的压缩字节会变成乱码字符串（不会报错），
/// 这里用来把这种情况识别出来，而不是当成「页面里没有结果」。
fn looks_like_html(body: &str) -> bool {
    let head: String = body.chars().take(200).collect::<String>().to_lowercase();
    head.contains("<!doctype html") || head.contains("<html")
}

/// 把一段 HTML 压成单行短文本，只在日志/报错里留一点现场。
fn snippet(html: &str, max_chars: usize) -> String {
    html.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(max_chars)
        .collect()
}

// ------------------------------------------------- 网络失败的“人话”与重试

/// 状态码 → 一句用户能照着做的话；认不出来的状态码返回 None（调用方退回纯状态码）。
///
/// 为什么抽出来：以前 401 只有 `fetch` 认识，另外两个 JSON 接口拿到 401 只会说
///「小册子接口返回 HTTP 401」—— 同一个站点、同一件事，三处说法不一样，
/// 用户不知道该干什么。`fetch` / `fetch_json` / 开服监控接口现在共用这一句。
fn status_hint(code: u16) -> Option<&'static str> {
    match code {
        401 => Some("需要先登录小册子账号（登录态已失效），请在设置里重新扫码"),
        403 => Some("疑似被站点风控拦下，稍后再试"),
        429 => Some("查询太频繁被限流了，稍后再试"),
        500..=599 => Some("站点服务端出错（过载或维护中），稍后再试"),
        _ => None,
    }
}

/// 一个非成功状态码的标准错误文案（三个 fetch 共用一处，免得各写各的）。
///
/// 注意文案里必须留着状态码数字：`commands.rs::classify_cookie_probe` 靠错误信息
/// 里的「401」/「403」认“站点明确说没登录”，把它弄丢了，过期 Cookie 会被判成
///「没验成」然后照旧存下来 —— 界面显示已保存，查价却永远提示未登录。
fn http_status_error(code: u16, url: &str) -> String {
    match status_hint(code) {
        Some(hint) => format!("小册子返回 HTTP {}：{}（{}）", code, hint, url),
        None => format!("小册子返回 HTTP {}（{}）", code, url),
    }
}

/// 遇到 429 / 5xx，第 `failures` 次失败后还要不要重试、等多久。
///
/// * 站点给了 `Retry-After` 就听它的 —— 只认秒数；HTTP-date 那种写法退回自己的
///   退避（宁可早一点点再试，也不为了解析日期去引一个日期库）；
/// * 没给就 1s → 2s → 4s 指数退避；
/// * 超过 `MAX_HTTP_RETRY` 次返回 None，调用方报错收工。
///
/// 为什么不能像以前那样立刻重发：限流之后马上再打一次等于往枪口上撞，
/// 站点只会把我们攫得更死 —— 而这个工具的所有功能共用同一个出口。
fn retry_delay(failures: u32, retry_after: Option<&str>) -> Option<Duration> {
    if failures > MAX_HTTP_RETRY {
        return None;
    }
    let fallback = Duration::from_secs(1u64 << failures.saturating_sub(1).min(3));

    let Some(text) = retry_after else {
        return Some(fallback);
    };
    match text.trim().parse::<u64>() {
        // 0 秒等于“立刻再来一次”，那正是这里要避免的；当成没给处理。
        Ok(seconds) if seconds > 0 => Some(Duration::from_secs(seconds.min(MAX_RETRY_AFTER_SEC))),
        _ => Some(fallback),
    }
}

/// 这段响应是不是一张登录页。
///
/// **只在本来要的内容已经没拿到时才调用**（没卡片 / 没解析出资料 / 没找到内联数据）
/// —— 这是刻意的调用约定：正常结果页里就算藏着个登录弹窗，也不会走到这些分支，
/// 所以检测可以只挑登录页独有的东西，不用担心误伤查得好好的查询。
///
/// 为什么需要它：站点 302 到登录页时最终状态码是 200，光看状态码完全看不出来，
/// 以前这种情况会一路走到「页面结构可能已变化」，把用户引去找一场
/// 根本没发生的改版。
fn looks_like_login_page(body: &str) -> bool {
    const SIGNALS: [&str; 5] = [
        "type=\"password\"", // 密码框：小册子的结果页里不会有
        "扫码登录",
        "请先登录",
        "请登录后",
        "登录已过期",
    ];
    SIGNALS.iter().any(|signal| body.contains(signal))
}

/// 登录页 → 一句用户能立刻照着做的话（不是登录页则为 None）。
fn login_page_reason(body: &str) -> Option<String> {
    looks_like_login_page(body).then(|| {
        "小册子把你带到了登录页（登录态已失效：401）——请在设置里重新扫码登录。".to_string()
    })
}

/// 「一张卡片、一句提示、连 summary 都没有」时给用户的一句话。
fn not_result_page_reason(html: &str) -> String {
    if let Some(reason) = login_page_reason(html) {
        return reason;
    }
    format!(
        "小册子返回的不是拍卖结果页（没有「找到 N 个匹配道具」字样，像HTML={}）：{}",
        looks_like_html(html),
        snippet(html, 200)
    )
}

/// 一次「一张结果卡都没有」的查询该怎么处置。
///
/// 抽成纯函数是因为这段分支是整条拍卖查询的命门：它决定要不要**再打一次站点**
/// （原样重试 / 换凭证重试）、什么时候可以放心返回空、什么时候必须报错。
/// 以前它内联在 `search_market` 的 `match` 里，没法测 —— 想往里加“隔一下再重试”
/// 或“登录页提示”就只能靠人眼盯着改。
#[derive(Debug, PartialEq, Eq)]
enum EmptyPage {
    /// 服务端说「找到 0 个」，但还没原样重试过：等一下再问一次
    RetryEmpty,
    /// 两次都是「找到 0 个」：按“真没有这个道具”处理
    ReturnEmpty,
    /// 页面不对（凭证过期 / 登录页 / 乱码）：换凭证再问一次，附上原因
    RefreshToken(String),
    /// 换了凭证还是不对：把原因报出去（不能静默返回空）
    Fail(String),
}

fn classify_empty_page(
    html: &str,
    notice: Option<&str>,
    summary: Option<&str>,
    retried_empty: bool,
    refreshed_token: bool,
) -> EmptyPage {
    // 页面本身不对时，是「换个凭证再试」还是“已经换过了、认输”，
    // 取决于 refreshed_token —— 两种情况的原因都要原样交出去。
    let retry_or_fail = |reason: String| {
        if refreshed_token {
            EmptyPage::Fail(reason)
        } else {
            EmptyPage::RefreshToken(reason)
        }
    };

    match (notice, summary) {
        // 服务端明确说「找到 0 个匹配道具」：多数是真的没有这个道具，
        // 但小册子在自身数据没准备好时会同样回这一句 —— 那是假阴性，
        // 原样再查一次（中间隔 EMPTY_RETRY_DELAY_MS）才分得开。
        (None, Some(_)) if !retried_empty => EmptyPage::RetryEmpty,
        (None, Some(_)) => EmptyPage::ReturnEmpty,
        (Some(message), _) => retry_or_fail(message.to_string()),
        (None, None) => retry_or_fail(not_result_page_reason(html)),
    }
}

/// 解析道具图鉴页里的游戏内样式提示框。
///
/// 结构（实测「锅盖」的页面）：
/// ```html
/// <article class="gcw-client-item-tip is-equip">
///   <span class="gcw-client-item-id">01092008</span>
///   <header class="gcw-client-item-title"><h1>锅盖</h1></header>
///   <div class="gcw-client-item-reqs"><span><small>需求等级：</small><b>10</b></span>…</div>
///   <div class="gcw-client-item-jobs"><b>战士</b>…</div>
///   <div class="gcw-client-item-facts"><div><small>装备分类</small><b>盾牌</b></div></div>
///   <div class="gcw-client-item-props"><div><span>物理防御力</span>
///     <b class="gcw-client-item-prop-value">+10<small class="gcw-client-item-prop-range">(9-12)</small></b>
///   </div></div>
///   <div class="gcw-client-item-upgrade">可使用卷轴次数：7 次</div>
///   <footer class="gcw-client-item-footer"><span>出售价格：<b>2,000</b> 金币</span></footer>
/// </article>
/// ```
fn parse_item_detail(html: &str, fallback_id: &str) -> Option<ItemDetail> {
    let doc = Html::parse_document(html);
    let article = doc.select(&Selector::parse("article.gcw-client-item-tip").ok()?).next()?;

    let name = text_of(&article, &Selector::parse(".gcw-client-item-title h1").ok()?);
    if name.is_empty() {
        return None;
    }

    // 页面上写的是补零后的展示 ID（01092008），还原成真实 ID。
    let listed_id = text_of(&article, &Selector::parse(".gcw-client-item-id").ok()?);
    let id = listed_id.trim_start_matches('0').to_string();
    let id = if id.is_empty() {
        fallback_id.to_string()
    } else {
        id
    };

    let equipment = article
        .value()
        .attr("class")
        .map(|class| class.contains("is-equip"))
        .unwrap_or(false);

    let icon_url = article
        .select(&Selector::parse(".gcw-client-item-icon img").ok()?)
        .next()
        .and_then(|img| img.value().attr("src"))
        .map(absolute_url)
        .unwrap_or_default();

    // 需求等级：10 / 需求力量：0 …… 零值一律不返回（原页面会给它们加 is-zero）。
    let reqs = label_value_pairs(&article, ".gcw-client-item-reqs > span")
        .into_iter()
        .filter(|fact| fact.value != "0")
        .collect();

    let jobs = article
        .select(&Selector::parse(".gcw-client-item-jobs b").ok()?)
        .map(|element| element_text(&element))
        .filter(|job| !job.is_empty())
        .collect();

    let facts = label_value_pairs(&article, ".gcw-client-item-facts > div");

    let small_sel = Selector::parse("small").unwrap();
    let bold_sel = Selector::parse("b").unwrap();
    let props = article
        .select(&Selector::parse(".gcw-client-item-props > div").ok()?)
        .filter_map(|row| {
            let label = text_of(&row, &Selector::parse("span").ok()?);
            let value_element = row.select(&bold_sel).next()?;
            let range = text_of(&value_element, &small_sel);
            let full = element_text(&value_element);
            // <b> 的文字里嵌着范围，先去重再保留数值本身。
            let value = if range.is_empty() {
                full
            } else {
                full.replace(&range, "").trim().to_string()
            };

            (!label.is_empty()).then_some(ItemProperty {
                label,
                value,
                range: (!range.is_empty()).then_some(range),
            })
        })
        .collect();

    let upgrade = text_of(&article, &Selector::parse(".gcw-client-item-upgrade").ok()?);
    let sell_price = text_of(&article, &Selector::parse(".gcw-client-item-footer").ok()?);

    Some(ItemDetail {
        id,
        name,
        icon_url,
        equipment,
        reqs,
        jobs,
        facts,
        props,
        upgrade,
        sell_price,
    })
}

/// 把「<div><small>标签</small><b>数值</b></div>」这类成对结构抽出来。
fn label_value_pairs(scope: &ElementRef, selector: &str) -> Vec<ItemFact> {
    let Ok(sel) = Selector::parse(selector) else {
        return Vec::new();
    };
    let small_sel = Selector::parse("small").unwrap();
    let bold_sel = Selector::parse("b").unwrap();

    scope
        .select(&sel)
        .filter_map(|row| {
            let label = text_of(&row, &small_sel);
            let value = text_of(&row, &bold_sel);
            (!label.is_empty() && !value.is_empty()).then_some(ItemFact { label, value })
        })
        .collect()
}

fn element_text(element: &ElementRef) -> String {
    element
        .text()
        .collect::<Vec<_>>()
        .join("")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// 把拍卖页上的错误提示（例如「查询凭证已过期，请重新提交一次。」）读出来。
fn extract_market_error(html: &str) -> Option<String> {
    let doc = Html::parse_document(html);
    let sel = Selector::parse(".market-notice.is-error").ok()?;
    let text = doc
        .select(&sel)
        .next()
        .map(|e| e.text().collect::<Vec<_>>().join("").trim().to_string())
        .unwrap_or_default();

    if text.is_empty() {
        None
    } else {
        Some(format!("小册子返回：{}", text))
    }
}

fn text_of(element: &ElementRef, sel: &Selector) -> String {
    element
        .select(sel)
        .next()
        .map(|e| e.text().collect::<Vec<_>>().join("").trim().to_string())
        .unwrap_or_default()
}

fn absolute_url(src: &str) -> String {
    if src.starts_with("http") || src.is_empty() {
        src.to_string()
    } else {
        format!("{}{}", SITE_ORIGIN, src)
    }
}

fn query_param(url: &str, key: &str) -> Option<String> {
    let needle = format!("{}=", key);
    let start = url.find(&needle)? + needle.len();
    let rest = &url[start..];
    let end = rest.find('&').unwrap_or(rest.len());
    Some(rest[..end].to_string())
}

fn fmt_num(value: Option<f64>) -> String {
    match value {
        Some(n) if !n.is_finite() => String::new(),
        Some(n) if n.fract() == 0.0 => format!("{:.0}", n),
        Some(n) => format!("{}", n),
        None => String::new(),
    }
}

/// 从 HTML 里把 `window.PRICE_TREND_DATA = {...};` 的对象抠出来
/// （按花括号配对扫描，跳过字符串字面量，避免被内容里的 `}` 骗到）。
fn extract_price_trend_json(html: &str) -> Option<String> {
    let marker = "window.PRICE_TREND_DATA";
    let after_marker = html.find(marker)? + marker.len();
    let open = after_marker + html[after_marker..].find('{')?;

    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;

    for (offset, ch) in html[open..].char_indices() {
        if in_string {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_string = false;
            }
            continue;
        }

        match ch {
            '"' => in_string = true,
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(html[open..=open + offset].to_string());
                }
            }
            _ => {}
        }
    }

    None
}

// ------------------------------------------------- PRICE_TREND_DATA 结构

#[derive(Deserialize)]
struct PriceTrendRoot {
    latest: PriceTrendLatest,
    #[serde(default)]
    history_hour: Vec<PriceTrendSnapshot>,
    #[serde(default)]
    history_day: Vec<PriceTrendSnapshot>,
}

#[derive(Deserialize)]
struct PriceTrendLatest {
    #[allow(dead_code)]
    collected_at: Option<String>,
    items: Vec<PriceTrendItem>,
}

#[derive(Deserialize)]
struct PriceTrendSnapshot {
    collected_at: String,
    items: Vec<PriceTrendItem>,
}

#[derive(Deserialize)]
struct PriceTrendItem {
    area_name: String,
    quote: Option<PriceTrendQuote>,
}

#[derive(Deserialize)]
struct PriceTrendQuote {
    yuan_to_wan_gold: Option<f64>,
    wan_gold_to_yuan: Option<f64>,
    price_yuan: Option<f64>,
    stock: Option<i64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn server_table_matches_site() {
        assert_eq!(
            SERVERS,
            [
                (1, "蓝蜗牛"),
                (2, "蘑菇仔"),
                (3, "绿水灵"),
                (4, "漂漂猪"),
                (5, "小白兔"),
            ]
        );
        assert_eq!(server_list().len(), 5);
    }

    #[test]
    fn extracts_price_trend_json_despite_braces_in_strings() {
        let html = r#"<script>
        window.PRICE_TREND_TOKEN = "abc";
        window.PRICE_TREND_DATA = {"latest":{"items":[{"area_name":"蓝蜗牛 } {\"","quote":{"stock":1}}]}};
        </script>"#;
        let json = extract_price_trend_json(html).expect("json");
        let root: PriceTrendRoot = serde_json::from_str(&json).expect("parse");
        assert_eq!(root.latest.items.len(), 1);
        assert_eq!(root.latest.items[0].area_name, "蓝蜗牛 } {\"");
    }

    #[test]
    fn parses_a_market_result_card() {
        let html = r#"
        <div class="market-result-list">
          <a class="market-result-card" href="/tools/market/item.php?id=1092008&amp;server=1&amp;ticket=abc">
            <span class="market-item-icon"><img src="/dbsource/icon/item/1092008.png" alt=""></span>
            <span class="market-item-main">
              <span class="market-item-title"><strong>锅盖</strong><em>Shield</em></span>
              <small>ID 1092008 · 暂无道具描述</small>
            </span>
            <span class="market-price-block">
              <small>蓝蜗牛 最低价</small>
              <strong>598,888 <em>金币</em></strong>
              <span class="is-down">-3.8% 较上次</span>
            </span>
          </a>
        </div>"#;

        let items = parse_market_cards(html);
        assert_eq!(items.len(), 1);
        let item = &items[0];
        assert_eq!(item.id, "1092008");
        assert_eq!(item.name, "锅盖");
        assert_eq!(item.server_name, "蓝蜗牛");
        assert_eq!(item.category, "Shield");
        assert_eq!(item.lowest_price, "598,888 金币");
        assert_eq!(item.icon_url, "https://mxdc.dvg.cn/dbsource/icon/item/1092008.png");
        assert!(item.detail_link.contains("id=1092008&server=1"));
    }

    #[test]
    fn reads_the_market_error_notice() {
        let html = r#"
        <div class="market-notice is-error">
          <span class="tw-icon-[mdi--alert-circle-outline]"></span>
          <span>查询凭证已过期，请重新提交一次。</span>
        </div>"#;
        assert_eq!(
            extract_market_error(html).as_deref(),
            Some("小册子返回：查询凭证已过期，请重新提交一次。")
        );
        assert!(extract_market_error("<div class=\"market-results\"></div>").is_none());
    }

    #[test]
    fn builds_history_points_sorted_and_labelled() {
        // 故意把时间倒着放，验证会自行排序；历史快照的 quote 里没有 price/stock。
        let json = r#"{
          "latest": {"items": []},
          "history_hour": [
            {"collected_at":"2026-09-21T17:00:00+08:00","items":[
              {"area_name":"蓝蜗牛","quote":{"yuan_to_wan_gold":7.97,"wan_gold_to_yuan":0.1255}},
              {"area_name":"蘑菇仔","quote":{"yuan_to_wan_gold":8.85}}]},
            {"collected_at":"2026-09-21T16:00:00+08:00","items":[
              {"area_name":"蓝蜗牛","quote":{"yuan_to_wan_gold":8.02}}]}
          ],
          "history_day": [
            {"collected_at":"2026-09-21T00:00:00+08:00","items":[
              {"area_name":"蓝蜗牛","quote":{"yuan_to_wan_gold":7.9}}]}
          ]
        }"#;

        let root: PriceTrendRoot = serde_json::from_str(json).unwrap();
        let hour = history_points(&root.history_hour, "%m-%d %H:%M");

        assert_eq!(hour.len(), 2);
        assert_eq!(hour[0].at, "09-21 16:00");
        assert_eq!(hour[1].at, "09-21 17:00");
        assert_eq!(hour[0].values.get("蘑菇仔"), None);
        assert_eq!(hour[1].values.get("蓝蜗牛"), Some(&7.97));

        let day = history_points(&root.history_day, "%m-%d");
        assert_eq!(day.len(), 1);
        assert_eq!(day[0].at, "09-21");
    }

    #[test]
    fn formats_rates_readably() {
        assert_eq!(fmt_num(Some(8.1633)), "8.1633");
        assert_eq!(fmt_num(Some(69.0)), "69");
        assert_eq!(fmt_num(None), "");
    }

    #[test]
    fn parses_item_detail_tooltip() {
        // 结构按实测的小册子图鉴页写的（锅盖 = 盾牌）。
        let html = r#"
        <main>
        <article class="gcw-client-item-tip gcw-client-item-tip--page is-equip">
          <span class="gcw-client-item-id">01092008</span>
          <header class="gcw-client-item-title"><h1>锅盖</h1></header>
          <div class="gcw-client-item-divider"></div>
          <div class="gcw-client-item-core">
            <div class="gcw-client-item-icon"><img src="/dbsource/icon/item/1092008.png" alt=""></div>
            <div class="gcw-client-item-summary">
              <div class="gcw-client-item-reqs">
                <span><small>需求等级：</small><b>10</b></span>
                <span class="is-zero"><small>需求力量：</small><b>0</b></span>
                <span class="is-zero"><small>需求人气：</small><b>0</b></span>
              </div>
            </div>
          </div>
          <div class="gcw-client-item-jobs"><b class="is-available">新手</b><b class="is-available">战士</b></div>
          <div class="gcw-client-item-details">
            <div class="gcw-client-item-facts">
              <div><small>装备分类</small><b>盾牌</b></div>
            </div>
            <div class="gcw-client-item-props">
              <div><span>物理防御力</span><b class="gcw-client-item-prop-value">+10<small class="gcw-client-item-prop-range">(9-12)</small></b></div>
            </div>
            <div class="gcw-client-item-upgrade">可使用卷轴次数：7 次</div>
          </div>
          <footer class="gcw-client-item-footer"><span>出售价格：<b>2,000</b> 金币</span></footer>
        </article>
        </main>"#;

        let detail = parse_item_detail(html, "1092008").expect("应能解析出图鉴资料");
        assert_eq!(detail.id, "1092008");
        assert_eq!(detail.name, "锅盖");
        assert!(detail.equipment);
        assert_eq!(
            detail.icon_url,
            "https://mxdc.dvg.cn/dbsource/icon/item/1092008.png"
        );
        // 零值需求不进结果
        assert_eq!(detail.reqs.len(), 1);
        assert_eq!(detail.reqs[0].label, "需求等级：");
        assert_eq!(detail.reqs[0].value, "10");
        assert_eq!(detail.jobs, vec!["新手", "战士"]);
        assert_eq!(detail.facts.len(), 1);
        assert_eq!(detail.facts[0].label, "装备分类");
        assert_eq!(detail.facts[0].value, "盾牌");
        assert_eq!(detail.props.len(), 1);
        assert_eq!(detail.props[0].label, "物理防御力");
        assert_eq!(detail.props[0].value, "+10");
        assert_eq!(detail.props[0].range.as_deref(), Some("(9-12)"));
        assert_eq!(detail.upgrade, "可使用卷轴次数：7 次");
        assert_eq!(detail.sell_price, "出售价格：2,000 金币");
    }

    #[test]
    fn parses_item_detail_that_cannot_be_sold() {
        let html = r#"<article class="gcw-client-item-tip">
        <span class="gcw-client-item-id">01002000</span>
        <header class="gcw-client-item-title"><h1>红色药水</h1></header>
        <footer class="gcw-client-item-footer"><span>无法出售或价格未知</span></footer>
        </article>"#;

        let detail = parse_item_detail(html, "1002000").expect("应能解析");
        assert_eq!(detail.name, "红色药水");
        assert!(!detail.equipment);
        assert_eq!(detail.sell_price, "无法出售或价格未知");
        assert!(detail.props.is_empty());
        assert!(detail.reqs.is_empty());
    }

    /// 实机探针（跑法：`cargo test --lib probe_item_detail -- --ignored --nocapture`）。
    /// 对着真实图鉴页验证解析：装备、饰品、不可出售的道具各来一个。
    #[test]
    #[ignore]
    fn probe_item_detail() {
        let client = MxdcClient::new();

        tauri::async_runtime::block_on(async {
            for id in ["1092008", "1103451", "1002000"] {
                match client.get_item_detail(id).await {
                    Ok(detail) => println!(
                        "{} -> {}｜装备={}｜需求={:?}｜分类={:?}｜属性={:?}｜卷轴={:?}｜售价={:?}",
                        id,
                        detail.name,
                        detail.equipment,
                        detail.reqs,
                        detail.facts,
                        detail.props,
                        detail.upgrade,
                        detail.sell_price
                    ),
                    Err(err) => println!("{} -> Err({})", id, err),
                }
            }
        });
    }

    #[test]
    fn parses_drop_search_response() {
        // 真实响应（2026-09-22 实测，截取两条结果 + 命中列表）
        let body = r#"{
          "ok": true,
          "meta": {"source":"mxdc","sourceLabel":"怀旧服","keyword":"锅盖","page":1,"pageSize":6,
                   "total":4,"totalPages":1,"dropTotal":4,"detail":"smart","order":"","boss":0},
          "matches": {
            "items": [{"itemid":1092008,"name":"锅盖","icon":"/dbsource/icon/item/1092008.png",
                       "reqLevel":10,"mainCategory":"Armor","subCategory":"Armor"}],
            "mobs": [],
            "itemTotal": 2,
            "mobTotal": 0
          },
          "results": [
            {"mob": {"mobid":1110100,"name":"绿蘑菇","level":15,"boss":0,"categoryLabel":"其他",
                      "icon":"/dbsource/mobsource/Mob._Canvas.1110100.img.stand.0.png",
                      "hp":"250","exp":"26","pad":"82","mad":"0"},
             "drops": [{"item":{"itemid":1092008,"name":"锅盖","icon":"/dbsource/icon/item/1092008.png",
                               "reqLevel":10,"mainCategory":"Armor","subCategory":"Armor"},
                        "chance":70,"chanceText":"0.007%","min":1,"max":1,"matched":1,"questid":0}],
             "maps": [{"mapid":100040000,"name":"魔法森林南部（3只）","street":"金银岛",
                       "icon":"/dbsource/mapsource/Map.Map.Map1.100040000.img.miniMap.canvas.png"}],
             "matchedDropCount": 1,
             "reasons": ["道具掉落"]}
          ]
        }"#;

        let mut parsed: DropSearch = serde_json::from_str(body).expect("parse");
        finalize_drop_result(&mut parsed);

        assert!(parsed.ok);
        assert_eq!(parsed.meta.keyword, "锅盖");
        assert_eq!(parsed.meta.total, 4);
        assert!(!parsed.meta.boss); // 站点给的是数字 0，不是布尔
        assert_eq!(parsed.matches.items[0].item_id, 1092008);

        let hit = &parsed.results[0];
        assert_eq!(hit.mob.mob_id, 1110100);
        assert_eq!(hit.mob.hp, "250"); // HP 是字符串
        assert_eq!(hit.reasons, vec!["道具掉落"]);
        assert!(hit.drops[0].matched);
        // 概率直接用站点算好的那个串，不自己换算
        assert_eq!(hit.drops[0].chance_text, "0.007%");
        assert_eq!(hit.maps[0].street, "金银岛");
        // 图标都补成绝对地址
        assert_eq!(
            hit.drops[0].item.icon,
            "https://mxdc.dvg.cn/dbsource/icon/item/1092008.png"
        );
        assert_eq!(
            hit.mob.icon,
            "https://mxdc.dvg.cn/dbsource/mobsource/Mob._Canvas.1110100.img.stand.0.png"
        );
        // 页面地址也在后端拼好
        assert_eq!(hit.drops[0].item.page_url, item_page_url(1092008));
        assert_eq!(hit.mob.page_url, mob_page_url(1110100));
        assert_eq!(hit.maps[0].page_url, map_page_url(100040000));
    }

    /// 前端拿到的**键名**是另一份契约（后端会把站点的 `itemid` 归一成 `itemId`），
    /// 所以这个测试管的是「序列化出去长什么样」，而不是「站点回什么」。
    /// 改字段名 / 中间一个 rename 写错，这里会立刻红 —— 否则只能是「界面上某块空白」
    /// 这种最难查的症状。
    #[test]
    fn drop_search_serializes_with_the_keys_the_ui_reads() {
        let body = r#"{"ok":true,
          "meta":{"keyword":"锅盖","page":1,"pageSize":6,"total":4,"totalPages":2,"dropTotal":4,"boss":0,
                  "source":"mxdc","sourceLabel":"怀旧服"},
          "matches":{"items":[{"itemid":1092008,"name":"锅盖","icon":"/i.png","reqLevel":10,
                     "mainCategory":"Armor","subCategory":"Armor"}],"mobs":[],"itemTotal":1,"mobTotal":0},
          "results":[{"mob":{"mobid":1110100,"name":"绿蘑菇","level":15,"boss":0,
                    "categoryLabel":"其他","icon":"/m.png","hp":"250","exp":"26","pad":"82","mad":"0"},
            "drops":[{"item":{"itemid":1,"name":"绿蘑菇盖","icon":"/i2.png","reqLevel":0,
                       "mainCategory":"","subCategory":""},
                     "chance":3600,"chanceText":"36%","min":1,"max":2,"matched":1,"questid":7}],
            "maps":[{"mapid":100040000,"name":"魔法森林南部（3只）","street":"金银岛","icon":"/mp.png"}],
            "matchedDropCount":1,"reasons":["道具掉落"]}]}"#;

        let mut parsed: DropSearch = serde_json::from_str(body).expect("parse");
        finalize_drop_result(&mut parsed);
        let json: serde_json::Value =
            serde_json::from_str(&serde_json::to_string(&parsed).unwrap()).unwrap();

        // 前端读的每一个键都在这——改一个就红。
        assert_eq!(json["meta"]["totalPages"], 2);
        assert_eq!(json["meta"]["dropTotal"], 4);
        assert_eq!(json["meta"]["sourceLabel"], "怀旧服");
        assert_eq!(json["matches"]["items"][0]["itemId"], 1092008);
        assert_eq!(json["matches"]["items"][0]["reqLevel"], 10);
        assert_eq!(json["matches"]["items"][0]["pageUrl"], item_page_url(1092008));
        assert_eq!(json["results"][0]["mob"]["mobId"], 1110100);
        assert_eq!(json["results"][0]["mob"]["categoryLabel"], "其他");
        assert_eq!(json["results"][0]["mob"]["pageUrl"], mob_page_url(1110100));
        assert_eq!(json["results"][0]["drops"][0]["chanceText"], "36%");
        assert_eq!(json["results"][0]["drops"][0]["matched"], true);
        assert_eq!(json["results"][0]["drops"][0]["questid"], 7);
        assert_eq!(json["results"][0]["matchedDropCount"], 1);
        assert_eq!(json["results"][0]["maps"][0]["mapId"], 100040000);
        assert_eq!(json["results"][0]["maps"][0]["pageUrl"], map_page_url(100040000));
    }

    #[test]
    fn drop_search_tolerates_missing_mob_stats() {
        // 实测：老怪物在「命中列表」里没有 hp/exp/icon，level 也可能是 0 ——
        // 缺字段不能把整次查询搞挂。
        let body = r#"{"ok":true,"meta":{"keyword":"绿蘑菇","total":5,"totalPages":1},
          "matches":{"items":[],"mobs":[{"mobid":9101000,"name":"绿蘑菇","level":0,"boss":0,
            "categoryLabel":"其他","icon":"","hp":"","exp":"","pad":"","mad":""}]},
          "results":[]}"#;
        let parsed: DropSearch = serde_json::from_str(body).expect("parse");
        assert_eq!(parsed.matches.mobs.len(), 1);
        assert_eq!(parsed.matches.mobs[0].name, "绿蘑菇");
        assert_eq!(parsed.matches.mobs[0].level, 0);
        assert!(parsed.results.is_empty());
    }

    /// 手工用例（`cargo test --lib probe_live_drops -- --ignored --nocapture`）：
    /// 对着真实接口跑三种关键词（道具名 / 怪物名 / ID），确认字段没变。只打摘要日志。
    #[test]
    #[ignore]
    fn probe_live_drops() {
        let client = MxdcClient::new();
        tauri::async_runtime::block_on(async {
            for keyword in ["锅盖", "绿蘑菇", "无魂猴", "1110100"] {
                match client.search_drops(keyword, 1).await {
                    Ok(result) => {
                        println!(
                            "[{}] total={} pages={} 命中怪物={} 命中道具={}",
                            keyword,
                            result.meta.total,
                            result.meta.total_pages,
                            result.matches.mobs.len(),
                            result.matches.items.len()
                        );
                        for hit in result.results.iter().take(3) {
                            println!(
                                "  {} Lv{} HP{} EXP{} · 掉落 {} 条 · 地图 {} 个",
                                hit.mob.name,
                                hit.mob.level,
                                hit.mob.hp,
                                hit.mob.exp,
                                hit.drops.len(),
                                hit.maps.len()
                            );
                            for drop in hit.drops.iter().take(4) {
                                println!(
                                    "    - {} {} ×{}-{} matched={}",
                                    drop.item.name,
                                    drop.chance_text,
                                    drop.min,
                                    drop.max,
                                    drop.matched
                                );
                            }
                        }
                    }
                    Err(err) => println!("[{}] ERR {}", keyword, err),
                }
            }
        });
    }

    /// 临时诊断（跑法：`cargo test --lib probe_live_market -- --ignored --nocapture`）。
    ///
    /// 用**应用自己的 HTTP 栈**（reqwest + 同一套请求头）+ 数据库里的真实 Cookie，
    /// 重放那几次「查不到」的查询，把它实际收到的页面特征打出来。
    /// 只打印长度和片段，不打印 Cookie / 令牌本身。
    #[test]
    #[ignore]
    fn probe_live_market() {
        let appdata = std::env::var("APPDATA").expect("APPDATA");
        let db_path = format!("{}\\com.mxdbox.tool\\mxd_box.db", appdata);
        let conn = rusqlite::Connection::open(&db_path).expect("打开数据库失败");
        let cookie: String = conn
            .query_row(
                "select value from settings where key = 'auth_cookie'",
                [],
                |row| row.get(0),
            )
            .expect("读取 Cookie 失败");
        println!("cookie len = {}", cookie.len());

        let client = MxdcClient::new();

        tauri::async_runtime::block_on(async {
            let cases: [(u32, &str); 6] = [
                (1, "锅盖"),
                (3, "锅盖"),
                (3, "炎弩"),
                (2, "锅盖"),
                (4, "锅盖"),
                (3, "战斗"),
            ];

            for (server, keyword) in cases {
                let token = match client.market_token(&cookie, false).await {
                    Ok(token) => token,
                    Err(err) => {
                        println!("server={} q={:?} 取令牌失败: {}", server, keyword, err);
                        continue;
                    }
                };

                let url = format!(
                    "{}?st={}&server={}&q={}",
                    MARKET_URL,
                    urlencoding::encode(&token),
                    server,
                    urlencoding::encode(keyword)
                );

                match client.fetch(&url, &cookie).await {
                    Ok(html) => {
                        let items = parse_market_cards(&html);
                        let notice = extract_market_error(&html);
                        let head = {
                            // 按字符取窗口，避免在多字节边界上切开导致 panic。
                            let flat = html.split_whitespace().collect::<Vec<_>>().join(" ");
                            let chars: Vec<char> = flat.chars().collect();
                            let at = chars
                                .windows(2)
                                .position(|w| w == ['找', '到'])
                                .map(|i| i.saturating_sub(30))
                                .unwrap_or(0);
                            let window: String = chars
                                .iter()
                                .skip(at)
                                .take(90)
                                .collect();
                            if flat.contains("找到") {
                                window
                            } else {
                                format!("(没有『找到 N 个匹配道具』字样) 开头: {}", window)
                            }
                        };
                        println!(
                            "server={} q={:?} -> http_len={} cards={} notice={:?}\n    around: {}",
                            server,
                            keyword,
                            html.len(),
                            items.len(),
                            notice,
                            head
                        );
                        if items.is_empty() {
                            let flat = html.split_whitespace().collect::<Vec<_>>().join(" ");
                            let head600: String = flat.chars().take(600).collect();
                            println!("    body head: {}", head600);
                        }
                    }
                    Err(err) => println!("server={} q={:?} 请求失败: {}", server, keyword, err),
                }
            }

            // 再直查一次原始响应：响应头 + 原始字节，确认站点到底发不发压缩内容、
            // 以及解出来的东西是不是 HTML。（reqwest 不再报错时文本会变成乱码，
            // 那条路会让页面“没卡片也没报错”。）
            let token = client.market_token(&cookie, false).await.unwrap_or_default();
            let url = format!(
                "{}?st={}&server={}&q={}",
                MARKET_URL,
                urlencoding::encode(&token),
                1,
                urlencoding::encode("锅盖")
            );
            match client.client.get(&url).header("Cookie", &cookie).send().await {
                Ok(resp) => {
                    println!("--- 原始响应 ---");
                    for (name, value) in resp.headers() {
                        println!("    {}: {}", name, value.to_str().unwrap_or("(非文本)"));
                    }
                    match resp.bytes().await {
                        Ok(bytes) => {
                            let head: String = String::from_utf8_lossy(&bytes)
                                .chars()
                                .take(60)
                                .collect();
                            println!(
                                "    原始字节={} 开头={:?} 像HTML={}",
                                bytes.len(),
                                head.replace(['\n', '\r'], " "),
                                looks_like_html(&String::from_utf8_lossy(&bytes))
                            );
                        }
                        Err(err) => println!("    读字节失败: {}", err),
                    }
                }
                Err(err) => println!("--- 原始请求失败: {}", err),
            }

            // 走一遍完整的 search_market：一个有结果的、一个真的没匹配的，
            // 顺便确认「找到 0 个」的重试分支不会转不停。
            for (server, keyword) in [(2u32, "锅盖"), (1, "zzz不存在的道具zzz")] {
                match client.search_market(keyword, server, &cookie).await {
                    Ok(items) => println!(
                        "search_market server={} q={:?} -> Ok({} 条) {:?}",
                        server,
                        keyword,
                        items.len(),
                        items.iter().map(|item| item.name.clone()).collect::<Vec<_>>()
                    ),
                    Err(err) => println!(
                        "search_market server={} q={:?} -> Err({})",
                        server, keyword, err
                    ),
                }
            }
        });
    }

    /// `new()` 里那句 `.expect()` 不是句空话：这份默认请求头真的建得起来。
    ///
    /// 建不成的话程序启动就该炸（那是写错了），而不是像以前那样静默换成一个
    /// **没 UA、没超时**的客户端 —— 那种降级运行期才暴露，而且长得跟正常一样。
    #[test]
    fn default_request_headers_build_into_a_client() {
        let built = Client::builder()
            .default_headers(site_headers())
            .timeout(Duration::from_secs(15))
            .build();
        assert!(built.is_ok(), "默认请求头写错了：{:?}", built.err());
    }

    /// 429 / 5xx 的退避：站点给了 `Retry-After` 就听它的，没给就 1s → 2s → 4s，
    /// 超过次数一律返回 None（调用方报错收工，不能无限重发）。
    #[test]
    fn backoff_honours_retry_after_then_falls_back_and_stops() {
        // 站点说了算（只认秒数）
        assert_eq!(retry_delay(1, Some("7")), Some(Duration::from_secs(7)));
        // 没给就 1 → 2 → 4
        assert_eq!(retry_delay(1, None), Some(Duration::from_secs(1)));
        assert_eq!(retry_delay(2, None), Some(Duration::from_secs(2)));
        assert_eq!(retry_delay(3, None), Some(Duration::from_secs(4)));
        // 封顶：第 4 次不再重试
        assert_eq!(retry_delay(4, None), None);

        // 站点让我们等太久 / `Retry-After` 写的是日期 / 写的是 0 —— 一律退回
        // 自己的退避：用户在等一个交互式查询，不能被一个 60 秒的数字挂住
        assert_eq!(
            retry_delay(1, Some("120")),
            Some(Duration::from_secs(MAX_RETRY_AFTER_SEC))
        );
        assert_eq!(
            retry_delay(1, Some("Wed, 21 Oct 2026 07:28:00 GMT")),
            Some(Duration::from_secs(1))
        );
        assert_eq!(retry_delay(2, Some("0")), Some(Duration::from_secs(2)));
    }

    /// 「找到 0 个」的重试必须隔开（不是几毫秒内原样再发一次）。
    /// 常量改成 0 就等于把那次重试变回纯浪费 —— 这个测试就是拦那个。
    #[test]
    fn empty_result_retry_is_spaced_out_not_instant() {
        assert!(
            (300..=1000).contains(&EMPTY_RETRY_DELAY_MS),
            "重试间隔应当在 300~1000ms 之间，实际 {}ms",
            EMPTY_RETRY_DELAY_MS
        );
    }

    /// 「一张结果卡都没有」的四种处置，每一种都对着一段真实形状的页面。
    #[test]
    fn empty_result_page_decisions_cover_every_case() {
        let zero = r#"<html><body><div class="market-result-list">
            <p>绿水灵 · 找到 0 个匹配道具</p></div></body></html>"#;
        let notice = r#"<div class="market-notice is-error">
            <span>查询凭证已过期，请重新提交一次。</span></div>"#;
        let login = r#"<!doctype html><html><body><form action="/login.php">
            <input type="password" name="pwd"></form></body></html>"#;
        let garbage = "无法解压的乱码页面，既没有卡片也没有 summary";

        // 1) 服务端说「找到 0 个」但还没重试过 → 等一下原样再查
        assert_eq!(
            classify_empty_page(zero, None, market_summary(zero).as_deref(), false, false),
            EmptyPage::RetryEmpty
        );
        // 2) 第二次还是「找到 0 个」→ 按真没有这个道具处理
        assert_eq!(
            classify_empty_page(zero, None, market_summary(zero).as_deref(), true, false),
            EmptyPage::ReturnEmpty
        );

        // 3) 有错误提示（凭证过期）→ 换凭证再试；已经换过一次就报错
        let notice_text = extract_market_error(notice);
        assert!(matches!(
            classify_empty_page(notice, notice_text.as_deref(), None, false, false),
            EmptyPage::RefreshToken(_)
        ));
        assert!(matches!(
            classify_empty_page(notice, notice_text.as_deref(), None, false, true),
            EmptyPage::Fail(_)
        ));

        // 4) 站点 302 到登录页（最终 200，什么都没解析出来）：先说人话，
        //    而且文案里得留着 401 —— Cookie 体检靠它认「站点明确说没登录」
        let reason = match classify_empty_page(login, None, None, false, false) {
            EmptyPage::RefreshToken(reason) => reason,
            other => panic!("登录页应当先换凭证再试，实际 {:?}", other),
        };
        assert!(reason.contains("登录页") && reason.contains("401"), "{}", reason);

        let reason = match classify_empty_page(login, None, None, false, true) {
            EmptyPage::Fail(reason) => reason,
            other => panic!("换了凭证还是登录页就该报错，实际 {:?}", other),
        };
        assert!(reason.contains("重新扫码"), "{}", reason);

        // 5) 既不是登录页、又连 summary 都没有的乱码页 → 退回「不是结果页」并留现场
        let reason = match classify_empty_page(garbage, None, None, false, true) {
            EmptyPage::Fail(reason) => reason,
            other => panic!("实际 {:?}", other),
        };
        assert!(reason.contains("不是拍卖结果页"), "{}", reason);
    }

    /// 登录页检测 + 「去扫码」这句话。以前 cookie 失效会一路走到
    /// 「页面结构可能已变化」，把用户引去找一场根本没发生的改版。
    #[test]
    fn login_pages_are_recognised_and_tell_the_user_to_rescan() {
        assert!(looks_like_login_page(r#"<form><input type="password"></form>"#));
        assert!(looks_like_login_page("<p>扫码登录后无需任何操作</p>"));
        assert!(looks_like_login_page("<p>请先登录后再查询</p>"));
        // 正常页面不误伤（调用约定：只在内容已经没拿到时才问这一句）
        assert!(!looks_like_login_page(r#"<a class="market-result-card">锅盖</a>"#));
        assert!(!looks_like_login_page(r#"<article class="gcw-client-item-tip">"#));
        assert!(!looks_like_login_page("一堆压缩失败的乱码"));

        let reason = login_page_reason(r#"<input type="password">"#).expect("登录页要有理由");
        assert!(
            reason.contains("401") && reason.contains("扫码"),
            "{}",
            reason
        );
        assert_eq!(login_page_reason("<div>正常结果页</div>"), None);
    }

    /// 状态码 → 人话：401 以前只有 `fetch` 认识，另外两个 JSON 接口拿到就只报
    ///「HTTP 401」；现在三处共用一句，而且文案里必须留着状态码数字。
    #[test]
    fn status_hints_say_what_to_do_and_keep_the_code() {
        assert!(status_hint(401).unwrap().contains("登录"));
        assert!(status_hint(403).unwrap().contains("风控"));
        assert!(status_hint(429).unwrap().contains("限流"));
        assert!(status_hint(502).unwrap().contains("站点"));
        assert_eq!(status_hint(404), None);

        let url = "https://mxdc.dvg.cn/tools/market/";
        assert!(
            http_status_error(401, url).contains("401")
                && http_status_error(401, url).contains("扫码"),
            "401 的文案要能被 commands.rs 的 Cookie 体检认出来"
        );
        assert!(http_status_error(404, url).contains("HTTP 404"));
    }
}
