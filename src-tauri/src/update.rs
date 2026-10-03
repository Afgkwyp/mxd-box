//! 版本自检：去一个用户自己填的「更新页」看一眼有没有新版本。
//!
//! ## 为什么只做「提示」，不做自动安装
//!
//! 这个程序是**解压即用的便携版**，没有安装器、没有数字签名。自动更新在 Windows 上
//! 要么得改成 NSIS/MSI 安装包（要保管一对签名密钥，私钥丢了老用户就再也升不了级），
//! 要么得自己写「下载 → 退出时把旧 exe 改名 → 写入新的」这种替换流程 —— 后者的失败
//! 模式是「朋友的程序坏了」，对一个小工具来说不值。所以这里只回答一个问题：
//! **有没有新版本**？有的话给一个按钮，用系统浏览器打开下载页，剩下的交给用户。
//!
//! ## 三种来源，按可靠程度排
//!
//! 更新页是**网盘分享页**（夸克 / 蓝奏云 / 微云这类）。那种页面不是给程序读的，
//! 而且各家格式不一样，所以这里试三种读法：
//!
//! 1. **JSON 清单**（最可靠）：`{"version":"0.1.1","url":"...","notes":"..."}`，
//!    放在任何能直接 GET 到文本的地方；
//! 2. **夸克网盘分享**：它的分享页是个前端渲染的空壳（HTML 里**一个文件名都没有**，
//!    只有 11 KB 的 SPA 骨架），所以得走它自己的分享接口：先 `sharepage/token`
//!    换一个短期 `stoken`，再用 `sharepage/detail` 拿文件名列表。这两个接口不需要登录、
//!    不要提取码（分享设了提取码时会明确报出来）。分享既可以是一个 zip 直接放在
//!    根目录，也可以是「一个文件夹，zip 在里面」，所以列表要**逐层看**
//!    （只读第一层时，第二种会读到文件夹名，永远算不出新版本）；
//! 3. **页面里的文件名**：静态页面 / 目录列表上直接能看到文件名，于是约定
//!    「新版的压缩包命名成 `枫之助-0.1.1.zip`」，程序就从页面文本里找
//!    「枫之助」附近的三段式版本号，取最大的那个。
//!
//! 三种都失败时**明确说失败**（并把原因写出来），绝不猜一个版本号出来 ——
//! 一个假的「已是最新」比「检测失败」更糟。
//!
//! ## 夸克那条通道的诚实说明
//!
//! 这是**未公开的接口**，夸克随时可能改。这也是为什么它排在最后、而且失败时把
//! 原话说出来（「夸克网盘拒绝了这次查询：分享不存在」）而不是笼统地说「检查失败」：
//! 真失效了，用户至少能看出是分享链接的问题还是程序的问题。最坏的结果也只是
//! 「没能确认有没有新版本」，**不会**变成「已是最新」那种骗人的结论。

use serde::Serialize;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// 编译期内置的更新页地址（夸克网盘分享页，永久有效）。
///
/// **只写在这里，界面上没有地方能改**：朋友的机器上就靠这个值自动检查，
/// 让用户自己填一个网盘地址，等于把「有没有新版本」这件事交给他们去配置。
/// 所以发新版时约定：**把新的 `枫之助-x.y.z.zip` 传到同一个分享页**（替换旧文件、
/// 链接不变），这里就永远不用改。
///
/// 留空 = 不检查。
pub const DEFAULT_UPDATE_URL: &str = "https://pan.quark.cn/s/a9632b3efa8e";

/// 内置更新页地址（给界面用：基础配置页里那个「更新页」链接）。
///
/// 界面**不自己写死**这个地址，一律问这里 —— 换网盘只改这一行，
/// 不会出现「程序去 A 页检查、界面把用户引到 B 页」这种不一致。
pub fn page_url() -> String {
    DEFAULT_UPDATE_URL.to_string()
}

/// 夸克的两个分享接口（都不需要登录）。
const QUARK_TOKEN_URL: &str = "https://drive-pc.quark.cn/1/clouddrive/share/sharepage/token";
const QUARK_DETAIL_URL: &str = "https://drive-pc.quark.cn/1/clouddrive/share/sharepage/detail";
// 分享页本身是 SPA；必须用文件下载接口换临时直链，不能把 HTML 页面存成 .zip。
const QUARK_DOWNLOAD_URL: &str = "https://drive-pc.quark.cn/1/clouddrive/file/download";

/// 一次版本自检的结果。字段名就是界面直接读的那份契约。
#[derive(Debug, Clone, Serialize)]
pub struct UpdateInfo {
    /// 当前在跑的版本（来自 Cargo.toml）
    pub current: String,
    /// 更新页上读到的最新版本（读不到就是 None）
    pub latest: Option<String>,
    /// 有没有新版本（两者都读到时才有意义）
    pub has_update: bool,
    /// 点「去下载」要打开的地址
    pub download_url: Option<String>,
    /// 更新说明（JSON 清单里带了才显示）
    pub notes: Option<String>,
    /// 这次是怎么读出来的：`"json"` / `"page"` / `"none"`
    pub source: String,
    /// 失败原因（网络不通 / 页面上找不到版本号）
    pub error: Option<String>,
}

impl UpdateInfo {
    fn failed(current: &str, error: impl Into<String>) -> Self {
        Self {
            current: current.to_string(),
            latest: None,
            has_update: false,
            download_url: None,
            notes: None,
            source: "none".to_string(),
            error: Some(error.into()),
        }
    }
}

/// 三段式版本号。`0.1.1` / `0.1.1-beta` 都按 `(0,1,1)` 处理 ——
/// 这里只回答「网络上的那个比在跑的这个新吗」，不需要完整的 semver 语义。
pub type Ver = (u32, u32, u32);

/// 从一段文本里抠出版本号（`v0.1.1` / `0.1.1` / `0.1.1-beta`）。
///
/// 宽容：两段（`0.1`）也算，缺的位补 0 —— JSON 清单里的版本号是人手写的。
pub fn parse_version(text: &str) -> Option<Ver> {
    let core = version_core(text)?;
    let mut parts = core.split('.');
    let mut numbers = [0u32; 3];
    let mut seen = 0usize;
    for (index, part) in parts.by_ref().take(3).enumerate() {
        if part.is_empty() || !part.chars().all(|c| c.is_ascii_digit()) {
            return None;
        }
        numbers[index] = part.parse().ok()?;
        seen += 1;
    }
    if seen < 2 {
        return None;
    }
    Some((numbers[0], numbers[1], numbers[2]))
}

/// 页面模式专用：**必须正好三段**（`x.y.z`）。
///
/// 为什么这里严格：分享页上到处是别的数字 —— 文件大小（`5.7 MB`）、下载次数、
/// 上传日期（`2026-09-22`）。允许两段的话，`5.7` 会被读成版本 `5.7.0`，
/// 然后就永远提示「有新版本」。
fn parse_three_part(token: &str) -> Option<Ver> {
    // 文件名里版本号后面跟的是 `.zip`，所以切出来常常带一个尾点：`0.1.1.`
    let core = version_core(token)?;
    let parts: Vec<&str> = core.split('.').collect();
    if parts.len() != 3 {
        return None;
    }
    let mut numbers = [0u32; 3];
    for (index, part) in parts.iter().enumerate() {
        if part.is_empty() || !part.chars().all(|c| c.is_ascii_digit()) {
            return None;
        }
        numbers[index] = part.parse().ok()?;
    }
    Some((numbers[0], numbers[1], numbers[2]))
}

/// 从 token 里取出「版本号本体」：去掉 `v` 前缀、尾点，以及后面的非数字内容。
fn version_core(text: &str) -> Option<String> {
    let trimmed = text.trim().trim_start_matches(['v', 'V']);
    let core: String = trimmed
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.')
        .collect();
    let core = core.trim_end_matches('.').to_string();
    if core.is_empty() {
        None
    } else {
        Some(core)
    }
}

/// 后者比前者新吗？相等或更旧都是 false。
pub fn is_newer(latest: Ver, current: Ver) -> bool {
    latest > current
}

/// 从「像 `枫之助-0.1.1.zip` 的文本」里找出所有跟产品名挨着的版本号，取最大的。
///
/// 关键在于**必须挨着产品名**：分享页上到处是日期（`2026-09-22`）、大小、下载次数，
/// 随便抓一个三段式数字就会读出一个假版本（比如把日期当版本，然后永远提示「有新版本」）。
/// 所以只在「枫之助」后面 60 个字符以内找版本号。
pub fn version_from_page(html: &str) -> Option<Ver> {
    const ANCHOR: &str = "枫之助";
    const WINDOW: usize = 60;

    let mut best: Option<Ver> = None;
    for (at, _) in html.match_indices(ANCHOR) {
        let rest: String = html[at..].chars().take(WINDOW).collect();
        // 逐段试：`枫之助-0.1.1.zip` 里的 `0.1.1`、`枫之助 v0.1.1` 里的 `0.1.1`
        for token in rest.split(|c: char| {
            !(c.is_ascii_digit() || c == '.' || c == 'v' || c == 'V')
        }) {
            if let Some(ver) = parse_three_part(token) {
                best = Some(match best {
                    Some(prev) if prev >= ver => prev,
                    _ => ver,
                });
            }
        }
    }
    best
}

/// 从夸克分享链接里取出它的分享 ID（`https://pan.quark.cn/s/<id>` 里的 `<id>`）。
///
/// 只按形状认（夸克域名 + `/s/<ID>` 路径），不猜内容：认不出来就返回 `None`，
/// 走通用的「页面里找文件名」那条路。
///
/// 兼容这几种实际会出现的写法：带 `?pwd=xxxx`、带尾斜杠、`#/list/share` 这种
/// 前端路由。
pub fn quark_share_id(url: &str) -> Option<String> {
    let parsed = reqwest::Url::parse(url.trim()).ok()?;
    if !parsed.host_str()?.ends_with("quark.cn") {
        return None;
    }
    let mut segments = parsed.path_segments()?;
    if segments.next()? != "s" {
        return None;
    }
    let id = segments.next()?.trim().to_string();
    if id.is_empty() || !id.chars().all(|c| c.is_ascii_alphanumeric()) {
        return None;
    }
    Some(id)
}

/// 从夸克的响应里把所有文件名抠出来（递归 —— 文件列表可能嵌在 `data.list` 里）。
///
/// 为什么连 `title` 一起收：**单文件分享**时令牌接口返回的 `title` 就是文件名，
/// 没有列表；文件夹分享时 `title` 是目录名、真正的文件名在 `list` 里。两种都收，
/// 上层反正只会从里面挑「挨着产品名的三段式版本号」。
fn collect_file_names(value: &serde_json::Value, out: &mut Vec<String>) {
    match value {
        serde_json::Value::Object(map) => {
            for (key, item) in map {
                if key == "file_name" || key == "title" {
                    if let Some(name) = item.as_str() {
                        out.push(name.to_string());
                    }
                } else {
                    collect_file_names(item, out);
                }
            }
        }
        serde_json::Value::Array(items) => {
            for item in items {
                collect_file_names(item, out);
            }
        }
        _ => {}
    }
}

/// 最多往下看几层目录。
///
/// 分享的两种常见形态都要能读：直接丢一个 zip 在分享根目录，或者建一个文件夹
/// 名叫「枫之助」把 zip 放里面（**第二种只读第一层会读到文件夹名，永远读不出
/// 版本号** —— 这个错误真实发生过一次）。两层足够覆盖，不跟着别人的目录结构
/// 无限递归。
const QUARK_MAX_DEPTH: usize = 3;

/// 一次查询最多发几个列表请求（防止有人分享一棵很大的目录树）。
const QUARK_MAX_REQUESTS: usize = 8;

/// 一次分享扫描的结果。
#[derive(Clone)]
struct QuarkDownloadFile {
    name: String,
    fid: String,
    share_fid_token: String,
}

struct QuarkScan {
    /// 读到的所有名字（文件和文件夹都在内，上层只关心能抠出版本号的那些）
    names: Vec<String>,
    /// 下载接口需要这两个分享临时值；不持久化，避免 stoken 过期后留下坏状态。
    stoken: String,
    files: Vec<QuarkDownloadFile>,
    /// 因为层数 / 请求数上限**没进去看**的文件夹。
    ///
    /// 单独留着是为了报错语如实：这种情况要说「没往里看」，不能说成「里面没有」。
    skipped_folders: Vec<String>,
}

/// 问夸克要一次分享里的文件名列表。
///
/// 两步：`sharepage/token` 换 `stoken`（公开分享不需要登录，带没带提取码都在
/// 这里体现），再从分享根目录逐层 `sharepage/detail` 取列表。
///
/// 逐层是必需的（见 `QUARK_MAX_DEPTH` 的说明）：只看第一层时，
/// 「文件夹里装着 zip」这种分享会读出文件夹名，于是永远显示「没能确认」。
/// 真读不到时错误信息会把读到的名字报出来，以便判断是「文件没传」还是
/// 「程序没读对」。
async fn quark_file_names(
    client: &reqwest::Client,
    pwd_id: &str,
) -> Result<QuarkScan, String> {
    let token_url = reqwest::Url::parse_with_params(
        QUARK_TOKEN_URL,
        [("pr", "ucpro"), ("fr", "pc"), ("uc_param_str", "")],
    )
    .map_err(|err| format!("构造请求失败：{}", err))?;

    let body: serde_json::Value = client
        .post(token_url)
        .header("Referer", "https://pan.quark.cn/")
        .json(&serde_json::json!({ "passcode": "", "pwd_id": pwd_id }))
        .send()
        .await
        .map_err(|err| format!("连不上夸克网盘：{}", short_error(&err.to_string())))?
        .json()
        .await
        .map_err(|err| format!("夸克返回的不是 JSON：{}", short_error(&err.to_string())))?;

    if body.get("status").and_then(|v| v.as_i64()) != Some(200) {
        let message = body
            .get("message")
            .and_then(|v| v.as_str())
            .unwrap_or("未知原因");
        return Err(format!(
            "夸克网盘拒绝了这次查询：{}（分享可能已被取消，或者设了提取码）",
            message
        ));
    }

    let data = body
        .get("data")
        .ok_or_else(|| "夸克没有返回分享内容（链接可能已经失效）".to_string())?;
    let stoken = data
        .get("stoken")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "夸克没有给出会话凭证 —— 这个分享大概设了提取码".to_string())?
        .to_string();

    // 单文件分享：文件名就在 token 那一步的 `title` 里
    let mut names = Vec::new();
    collect_file_names(data, &mut names);
    let mut files = Vec::new();
    let mut skipped_folders = Vec::new();

    // 从分享根目录（`pdir_fid = 0`）开始逐层列，遇到文件夹就进去看。
    // 队列里存上目录名，是为了「因为上限没进去」时能报出人看的懂的名字。
    let mut queue: std::collections::VecDeque<(String, String, usize)> =
        std::collections::VecDeque::new();
    queue.push_back(("0".to_string(), "分享根目录".to_string(), 0));
    let mut requests = 0usize;

    while let Some((fid, folder, depth)) = queue.pop_front() {
        if requests >= QUARK_MAX_REQUESTS {
            skipped_folders.push(folder);
            continue;
        }
        requests += 1;

        let items = match quark_list(client, pwd_id, &stoken, &fid).await {
            Ok(items) => items,
            // 这一步失败不算致命：单文件分享本来就没有列表
            Err(_) => continue,
        };

        for item in &items {
            let Some(name) = item.get("file_name").and_then(|v| v.as_str()) else {
                continue;
            };
            names.push(name.to_string());

            let is_dir = item.get("dir").and_then(|v| v.as_bool()).unwrap_or(false);
            if !is_dir {
                if let (Some(fid), Some(share_fid_token)) = (
                    item.get("fid").and_then(|v| v.as_str()),
                    item.get("share_fid_token").and_then(|v| v.as_str()),
                ) {
                    files.push(QuarkDownloadFile {
                        name: name.to_string(),
                        fid: fid.to_string(),
                        share_fid_token: share_fid_token.to_string(),
                    });
                }
                continue;
            }
            match item.get("fid").and_then(|v| v.as_str()) {
                Some(child) if depth + 1 < QUARK_MAX_DEPTH => {
                    queue.push_back((child.to_string(), name.to_string(), depth + 1))
                }
                // 到了层数上限就如实记下来：后面报错时要说「没往里看」，
                // 而不是让用户以为「里面没有」
                _ => skipped_folders.push(name.to_string()),
            }
        }
    }

    names.sort();
    names.dedup();
    names.retain(|name| !name.trim().is_empty());
    if names.is_empty() {
        return Err("夸克分享里一个文件都没读到（可能是空分享，或者接口改了）".to_string());
    }
    files.sort_by(|left, right| left.fid.cmp(&right.fid));
    files.dedup_by(|left, right| left.fid == right.fid);
    Ok(QuarkScan {
        names,
        stoken,
        files,
        skipped_folders,
    })
}

/// 一层目录列表：`sharepage/detail` 带上要看的目录（`pdir_fid`）。
///
/// 只取 `data.list`，失败就把原因返回给调用方 —— 上层对「根目录没列表」和
/// 「子目录没读到」的处理不一样（前者是正常的单文件分享，后者要如实说出来）。
async fn quark_list(
    client: &reqwest::Client,
    pwd_id: &str,
    stoken: &str,
    pdir_fid: &str,
) -> Result<Vec<serde_json::Value>, String> {
    let url = reqwest::Url::parse_with_params(
        QUARK_DETAIL_URL,
        [
            ("pr", "ucpro"),
            ("fr", "pc"),
            ("uc_param_str", ""),
            ("pwd_id", pwd_id),
            ("stoken", stoken),
            ("pdir_fid", pdir_fid),
            ("force", "0"),
            ("_page", "1"),
            ("_size", "200"),
            ("_fetch_banner", "0"),
            ("_fetch_share", "1"),
            ("_fetch_total", "0"),
            ("_sort", "file_type:asc,updated_at:desc"),
        ],
    )
    .map_err(|err| format!("构造请求失败：{}", err))?;

    let body: serde_json::Value = client
        .get(url)
        .header("Referer", "https://pan.quark.cn/")
        .send()
        .await
        .map_err(|err| format!("连不上夸克网盘：{}", short_error(&err.to_string())))?
        .json()
        .await
        .map_err(|err| format!("夸克返回的不是 JSON：{}", short_error(&err.to_string())))?;

    Ok(body
        .get("data")
        .and_then(|data| data.get("list"))
        .and_then(|list| list.as_array())
        .cloned()
        .unwrap_or_default())
}

fn latest_quark_download_file(scan: &QuarkScan) -> Option<(&QuarkDownloadFile, Ver)> {
    scan.files
        .iter()
        .filter(|file| file.name.to_ascii_lowercase().ends_with(".zip"))
        .filter_map(|file| version_from_page(&file.name).map(|version| (file, version)))
        .max_by_key(|(_, version)| *version)
}

fn quark_download_url_from_response(body: &serde_json::Value) -> Option<&str> {
    let data = body.get("data")?;
    data.get("download_url")
        .and_then(serde_json::Value::as_str)
        .or_else(|| {
            data.as_array()?.iter().find_map(|entry| {
                entry.get("download_url").and_then(serde_json::Value::as_str)
            })
        })
}

async fn quark_download_link(
    client: &reqwest::Client,
    pwd_id: &str,
    scan: &QuarkScan,
) -> Result<(String, String), String> {
    let Some((file, version)) = latest_quark_download_file(scan) else {
        return Err(format!(
            "夸克分享里没有可下载的枫之助 ZIP：{}",
            preview_names(&scan.names)
        ));
    };
    let Some(endpoint) = reqwest::Url::parse_with_params(
        QUARK_DOWNLOAD_URL,
        [("pr", "ucpro"), ("fr", "pc"), ("uc_param_str", "")],
    )
    .ok() else {
        return Err("构造夸克下载请求失败".to_string());
    };

    let body: serde_json::Value = client
        .post(endpoint)
        .header("Referer", "https://pan.quark.cn/")
        .json(&serde_json::json!({
            "fids": [file.fid.clone()],
            "pwd_id": pwd_id,
            "stoken": scan.stoken.clone(),
            "share_fid_tokens": [file.share_fid_token.clone()],
        }))
        .send()
        .await
        .map_err(|err| format!("获取夸克下载地址失败：{}", short_error(&err.to_string())))?
        .json()
        .await
        .map_err(|err| format!("夸克下载接口没有返回 JSON：{}", short_error(&err.to_string())))?;

    if body.get("status").and_then(|value| value.as_i64()) != Some(200) {
        let message = body
            .get("message")
            .and_then(|value| value.as_str())
            .unwrap_or("未知原因");
        return Err(format!("夸克没有给出下载地址：{message}"));
    }
    let direct = quark_download_url_from_response(&body)
        .ok_or_else(|| "夸克下载响应里缺少 download_url".to_string())?;
    let direct = crate::commands::validate_external_url(direct)?;
    let fallback = format!("枫之助-{}.{}.{}.zip", version.0, version.1, version.2);
    Ok((direct.to_string(), fallback))
}

/// 把读到的文件名拼成一行给人看的预览（错误信息里附带，方便判断是没传还是没读对）。
fn preview_names(names: &[String]) -> String {
    let joined = names.join(" / ");
    let cut: String = joined.chars().take(120).collect();
    if joined.chars().count() > 120 {
        format!("{}…", cut)
    } else {
        cut
    }
}

/// 从 JSON 清单里读版本 / 下载地址 / 更新说明。
///
/// 键名做几种常见写法的兼容：写 `version`、`latest` 还是 `ver` 都认
/// （发布的人不一定会去看文档，宽容一点比报错强）。
pub fn parse_manifest(json: &str) -> Option<(Ver, Option<String>, Option<String>)> {
    let value: serde_json::Value = serde_json::from_str(json).ok()?;

    let pick = |keys: &[&str]| -> Option<String> {
        keys.iter()
            .find_map(|key| value.get(key).and_then(|v| v.as_str()))
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
    };

    let version = pick(&["version", "latest", "ver", "tag", "tag_name"])?;
    let ver = parse_version(&version)?;
    // 清单来自远端，下载地址也必须通过外链命令的同一条 HTTPS 白名单。
    // 不安全的地址让整份清单解析失败，不能把它送进系统协议处理器。
    let url = match pick(&["url", "download", "link", "download_url"]) {
        Some(url) => Some(crate::commands::validate_external_url(&url).ok()?.to_string()),
        None => None,
    };
    let notes = pick(&["notes", "changelog", "description", "body"]);
    Some((ver, url, notes))
}

/// 一次自检：拉更新页，先按 JSON 清单解析，不行再按「页面里的文件名」解析。
///
/// 超时 10 秒：这只是个顺手的检查，不该让界面转圈等太久。
pub async fn check(url: &str) -> UpdateInfo {
    let current = env!("CARGO_PKG_VERSION").to_string();
    let target = url.trim();
    if target.is_empty() {
        return UpdateInfo::failed(&current, "这个构建里没有内置更新页地址");
    }

    let client = match reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        // 网盘页面会挡掉没有 UA 的请求
        .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/133.0.0.0 Safari/537.36")
        .build()
    {
        Ok(client) => client,
        Err(err) => return UpdateInfo::failed(&current, format!("网络组件初始化失败：{}", err)),
    };

    let response = match client.get(target).send().await {
        Ok(response) => response,
        Err(err) => {
            return UpdateInfo::failed(
                &current,
                format!("连不上更新页（{}）：{}", target, short_error(&err.to_string())),
            )
        }
    };

    let status = response.status();
    if !status.is_success() {
        return UpdateInfo::failed(
            &current,
            format!("更新页返回 HTTP {}（{}）", status.as_u16(), target),
        );
    }

    let body = response.text().await.unwrap_or_default();
    if body.trim().is_empty() {
        return UpdateInfo::failed(&current, "更新页是空的（网盘直链经常这样，建议换成能直接读到的地址）");
    }

    let current_ver = parse_version(&current).unwrap_or((0, 0, 0));

    // ① JSON 清单
    if let Some((latest, url, notes)) = parse_manifest(&body) {
        log::info!("版本自检（JSON 清单）：本地 {} / 最新 {:?}", current, latest);
        return UpdateInfo {
            current,
            latest: Some(format!("{}.{}.{}", latest.0, latest.1, latest.2)),
            has_update: is_newer(latest, current_ver),
            // 清单没给下载地址就退回更新页本身
            download_url: Some(url.unwrap_or_else(|| target.to_string())),
            notes,
            source: "json".to_string(),
            error: None,
        };
    }

    // ② 夸克网盘分享：页面是前端渲染的空壳，HTML 里没有文件名，得走它的分享接口
    if let Some(pwd_id) = quark_share_id(target) {
        let scan = match quark_file_names(&client, &pwd_id).await {
            Ok(scan) => scan,
            Err(reason) => return UpdateInfo::failed(&current, reason),
        };

        if let Some(latest) = version_from_page(&scan.names.join("\n")) {
            log::info!("版本自检（夸克分享）：本地 {} / 最新 {:?}", current, latest);
            return UpdateInfo {
                current,
                latest: Some(format!("{}.{}.{}", latest.0, latest.1, latest.2)),
                has_update: is_newer(latest, current_ver),
                download_url: Some(target.to_string()),
                notes: None,
                source: "quark".to_string(),
                error: None,
            };
        }

        let mut reason = format!(
            "夸克分享里没有形如「枫之助-x.y.z.zip」的文件名，读到的是：{}",
            preview_names(&scan.names)
        );
        if !scan.skipped_folders.is_empty() {
            reason.push_str(&format!(
                "（还有文件夹没往里看：{}）",
                preview_names(&scan.skipped_folders)
            ));
        }
        return UpdateInfo::failed(&current, reason);
    }

    // ③ 页面里直接能看到文件名（静态页 / 目录列表）
    if let Some(latest) = version_from_page(&body) {
        log::info!("版本自检（页面文件名）：本地 {} / 最新 {:?}", current, latest);
        return UpdateInfo {
            current,
            latest: Some(format!("{}.{}.{}", latest.0, latest.1, latest.2)),
            has_update: is_newer(latest, current_ver),
            download_url: Some(target.to_string()),
            notes: None,
            source: "page".to_string(),
            error: None,
        };
    }

    UpdateInfo::failed(
        &current,
        "在更新页里没找到版本号（页面里应当能看到类似「枫之助-0.1.1.zip」的文件名）",
    )
}

/// 错误信息里往往带着很长的 URL，界面上只看前面一段就够。
fn short_error(raw: &str) -> String {
    let cut: String = raw.chars().take(80).collect();
    if raw.chars().count() > 80 {
        format!("{}…", cut)
    } else {
        cut
    }
}

fn disposition_filename(header: &str) -> Option<String> {
    let mut plain = None;
    for parameter in header.split(';').skip(1) {
        let Some((name, value)) = parameter.trim().split_once('=') else {
            continue;
        };
        let value = value.trim().trim_matches('"');
        if name.trim().eq_ignore_ascii_case("filename*") {
            let encoded = value.split_once("''").map(|(_, name)| name).unwrap_or(value);
            if let Ok(decoded) = urlencoding::decode(encoded) {
                if !decoded.is_empty() {
                    return Some(decoded.into_owned());
                }
            }
        } else if name.trim().eq_ignore_ascii_case("filename") {
            plain = Some(value.to_string());
        }
    }
    plain
}

fn safe_zip_filename(response_name: Option<&str>, fallback: &str) -> String {
    let Some(raw) = response_name else {
        return fallback.to_string();
    };
    let leaf = raw.rsplit(['/', '\\']).next().unwrap_or("").trim();
    let clean: String = leaf
        .chars()
        .map(|ch| {
            if ch.is_control() || matches!(ch, '<' | '>' | ':' | '"' | '|' | '?' | '*') {
                '_'
            } else {
                ch
            }
        })
        .collect();
    let clean = clean.trim_end_matches([' ', '.']);
    if clean.to_ascii_lowercase().ends_with(".zip") && !clean.is_empty() {
        clean.to_string()
    } else {
        fallback.to_string()
    }
}

fn fallback_update_filename(url: &reqwest::Url) -> String {
    let version = url
        .path_segments()
        .and_then(|mut segments| segments.next_back())
        .and_then(|name| urlencoding::decode(name).ok())
        .and_then(|name| version_from_page(&name).or_else(|| parse_three_part(&name)))
        .or_else(|| parse_version(env!("CARGO_PKG_VERSION")))
        .unwrap_or((0, 0, 0));
    format!("枫之助-{}.{}.{}.zip", version.0, version.1, version.2)
}

/// ponytail: 单次缓冲封顶 512 MiB；若真实发布包超过此值，再改成流式写盘并提高上限。
const MAX_UPDATE_DOWNLOAD_BYTES: u64 = 512 * 1024 * 1024;

fn has_zip_signature(bytes: &[u8]) -> bool {
    bytes.starts_with(b"PK\x03\x04") || bytes.starts_with(b"PK\x05\x06")
}

/// 下载到用户桌面但不替换运行中的 exe：下载失败不能把便携版变成打不开的半成品。
pub async fn download_to_desktop(url: &str, desktop: &Path) -> Result<PathBuf, String> {
    let source = crate::commands::validate_external_url(url)?;
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(180))
        .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/133.0.0.0 Safari/537.36")
        .build()
        .map_err(|err| format!("初始化下载连接失败：{err}"))?;

    let (download_url, fallback, is_quark_share) =
        if let Some(pwd_id) = quark_share_id(source.as_str()) {
            let scan = quark_file_names(&client, &pwd_id).await?;
            let (download_url, fallback) = quark_download_link(&client, &pwd_id, &scan).await?;
            (download_url, fallback, true)
        } else {
            (source.to_string(), fallback_update_filename(&source), false)
        };

    let request = client.get(download_url.as_str());
    let request = if is_quark_share {
        request.header("Referer", "https://pan.quark.cn/")
    } else {
        request
    };
    let response = request
        .send()
        .await
        .map_err(|err| format!("下载更新包失败：{}", short_error(&err.to_string())))?;
    // 不接受 HTTPS 直链重定向降级到 HTTP，否则下载内容可在传输中被替换。
    crate::commands::validate_external_url(response.url().as_str())?;
    if !response.status().is_success() {
        return Err(format!("下载更新包返回 HTTP {}", response.status().as_u16()));
    }
    if response
        .content_length()
        .is_some_and(|length| length > MAX_UPDATE_DOWNLOAD_BYTES)
    {
        return Err("更新包超过 512 MB，已停止下载".to_string());
    }
    let response_name = response
        .headers()
        .get(reqwest::header::CONTENT_DISPOSITION)
        .and_then(|header| header.to_str().ok())
        .and_then(disposition_filename);
    let bytes = response
        .bytes()
        .await
        .map_err(|err| format!("读取更新包失败：{}", short_error(&err.to_string())))?;
    if bytes.len() as u64 > MAX_UPDATE_DOWNLOAD_BYTES {
        return Err("更新包超过 512 MB，已停止下载".to_string());
    }
    if !has_zip_signature(&bytes) {
        return Err("下载地址返回的内容不是 ZIP；请打开更新页手动下载".to_string());
    }

    std::fs::create_dir_all(desktop).map_err(|err| format!("无法访问桌面目录：{err}"))?;
    let filename = safe_zip_filename(response_name.as_deref(), &fallback);
    let path = desktop.join(filename);
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .map_err(|err| {
            if err.kind() == std::io::ErrorKind::AlreadyExists {
                format!("桌面已存在同名更新包：{}", path.display())
            } else {
                format!("创建更新包失败：{err}")
            }
        })?;
    if let Err(err) = file.write_all(&bytes) {
        drop(file);
        let _ = std::fs::remove_file(&path);
        return Err(format!("保存更新包失败：{err}"));
    }
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zip_signature_rejects_a_share_page_instead_of_saving_html_as_a_zip() {
        assert!(has_zip_signature(b"PK\x03\x04zip-data"));
        assert!(has_zip_signature(b"PK\x05\x06empty-zip"));
        assert!(!has_zip_signature(b"<!doctype html>"));
    }

    #[test]
    fn download_names_are_decoded_sanitized_and_fall_back_to_versioned_zip() {
        assert_eq!(
            disposition_filename(
                "attachment; filename*=UTF-8''%E6%9E%AB%E4%B9%8B%E5%8A%A9-0.1.4.zip"
            ),
            Some("枫之助-0.1.4.zip".to_string())
        );
        assert_eq!(
            safe_zip_filename(Some(r"..\\outside\\枫之助-0.1.4.zip"), "枫之助-0.1.3.zip"),
            "枫之助-0.1.4.zip"
        );
        assert_eq!(
            safe_zip_filename(Some("not-a-zip.html"), "枫之助-0.1.3.zip"),
            "枫之助-0.1.3.zip"
        );
    }

    #[test]
    fn quark_download_response_accepts_object_and_list_shapes() {
        let object = serde_json::json!({ "data": { "download_url": "https://cdn.example/a.zip" } });
        let list = serde_json::json!({ "data": [{ "download_url": "https://cdn.example/b.zip" }] });
        assert_eq!(
            quark_download_url_from_response(&object),
            Some("https://cdn.example/a.zip")
        );
        assert_eq!(
            quark_download_url_from_response(&list),
            Some("https://cdn.example/b.zip")
        );
    }

    #[test]
    fn chooses_the_newest_named_quark_zip() {
        let scan = QuarkScan {
            names: Vec::new(),
            stoken: String::new(),
            files: vec![
                QuarkDownloadFile {
                    name: "枫之助-0.1.2.zip".to_string(),
                    fid: "old".to_string(),
                    share_fid_token: "t1".to_string(),
                },
                QuarkDownloadFile {
                    name: "枫之助-0.1.4.zip".to_string(),
                    fid: "new".to_string(),
                    share_fid_token: "t2".to_string(),
                },
            ],
            skipped_folders: Vec::new(),
        };
        let (file, version) = latest_quark_download_file(&scan).expect("应选出版本最高的 ZIP");
        assert_eq!(file.fid, "new");
        assert_eq!(version, (0, 1, 4));
    }

    #[test]
    fn parses_common_version_writings() {
        assert_eq!(parse_version("0.1.1"), Some((0, 1, 1)));
        assert_eq!(parse_version("v0.2.0"), Some((0, 2, 0)));
        assert_eq!(parse_version(" 1.2 "), Some((1, 2, 0)));
        assert_eq!(parse_version("0.1.1-beta"), Some((0, 1, 1)));
        assert_eq!(parse_version("v1"), None);
        assert_eq!(parse_version("随便一句话"), None);
        assert_eq!(parse_version(""), None);
    }

    /// 页面模式只认三段式：文件大小、下载次数、日期都不该被读成版本。
    #[test]
    fn page_scanner_requires_three_segments() {
        assert_eq!(parse_three_part("0.1.1."), Some((0, 1, 1)));
        assert_eq!(parse_three_part("v0.2.0"), Some((0, 2, 0)));
        assert_eq!(parse_three_part("5.7"), None, "文件大小不能被当成版本");
        assert_eq!(parse_three_part("2026"), None);
        assert_eq!(parse_three_part("12"), None);
        // 版本号挨着产品名，但旁边只有大小 → 读不出来（不猜）
        assert_eq!(version_from_page("<div>枫之助 大小 5.7 MB</div>"), None);
    }

    #[test]
    fn newer_means_strictly_newer() {
        assert!(is_newer((0, 1, 1), (0, 1, 0)));
        assert!(is_newer((0, 2, 0), (0, 1, 9)));
        assert!(is_newer((1, 0, 0), (0, 9, 9)));
        assert!(!is_newer((0, 1, 0), (0, 1, 0)));
        assert!(!is_newer((0, 0, 9), (0, 1, 0)));
    }

    /// 页面解析最容易出的错是「把日期当版本」，这里钉住它。
    #[test]
    fn page_version_must_sit_next_to_the_product_name() {
        let page = r#"
            <html><head><title>蓝奏云 文件分享</title></head>
            <body>
              <div class="file-name">枫之助-怀旧服小助手-0.1.1.zip</div>
              <span>上传时间 2026-09-22</span>
              <span>下载次数 12</span>
              <span>大小 5.7 MB</span>
            </body></html>
        "#;
        assert_eq!(version_from_page(page), Some((0, 1, 1)));

        // 只有日期、没有产品名挨着的版本号 → 读不出来（绝不许把 2026-09-22 当成版本）
        let date_only = r#"<div>上传时间 2026-09-22 版本说明见群里</div>"#;
        assert_eq!(version_from_page(date_only), None);

        // 一个页面上有多个版本（历史列表）→ 取最大的那个
        let multiple = r#"
            <a>枫之助-0.1.0.zip</a>
            <a>枫之助-0.2.0.zip</a>
            <a>枫之助-0.1.9.zip</a>
        "#;
        assert_eq!(version_from_page(multiple), Some((0, 2, 0)));
    }

    #[test]
    fn manifest_accepts_a_few_key_spellings() {
        let full = r#"{"version":"0.2.0","url":"https://example.com/dl","notes":"修了快捷键"}"#;
        let (ver, url, notes) = parse_manifest(full).expect("应当能解析");
        assert_eq!(ver, (0, 2, 0));
        assert_eq!(url.as_deref(), Some("https://example.com/dl"));
        assert_eq!(notes.as_deref(), Some("修了快捷键"));

        // 只写版本号也认（下载地址退回更新页本身）
        let minimal = r#"{"latest":"v0.3.0"}"#;
        let (ver, url, _) = parse_manifest(minimal).expect("应当能解析");
        assert_eq!(ver, (0, 3, 0));
        assert!(url.is_none());

        // 清单的下载地址与 open_external_url 共用 HTTPS 白名单；不能远程指定文件或 SMB。
        for unsafe_url in [
            "http://example.com/download.zip",
            "file:///C:/Windows/System32/calc.exe",
            r"\\server\share\payload.exe",
        ] {
            let manifest = serde_json::json!({ "version": "0.4.0", "url": unsafe_url }).to_string();
            assert!(parse_manifest(&manifest).is_none(), "应拒绝：{unsafe_url:?}");
        }

        // 不是 JSON / 没有版本号 → None，别硬猜
        assert!(parse_manifest("<html>不是 JSON</html>").is_none());
        assert!(parse_manifest(r#"{"foo":"bar"}"#).is_none());
    }

    /// 「连不上」必须说成「连不上」，不能报成「已是最新」。
    #[tokio::test]
    async fn network_failure_is_reported_not_swallowed() {
        let info = check("http://127.0.0.1:1/version.json").await;
        assert!(!info.has_update);
        assert!(info.latest.is_none());
        assert!(info.error.is_some(), "失败必须带原因");
        assert_eq!(info.source, "none");
    }

    #[tokio::test]
    async fn empty_url_says_so() {
        let info = check("   ").await;
        assert_eq!(info.source, "none");
        assert!(info.error.unwrap().contains("没有内置"));
    }

    /// 夸克分享链接的识别：只要形状对，带提取码 / 尾斜杠 / 前端路由都能认。
    #[test]
    fn recognizes_quark_share_links() {
        assert_eq!(
            quark_share_id("https://pan.quark.cn/s/decbbd691935"),
            Some("decbbd691935".to_string())
        );
        assert_eq!(
            quark_share_id("https://pan.quark.cn/s/decbbd691935?pwd=2333#/list/share"),
            Some("decbbd691935".to_string())
        );
        assert_eq!(
            quark_share_id("  https://pan.quark.cn/s/abc123/  "),
            Some("abc123".to_string())
        );

        // 长得不像夸克分享的一律不认（宁可不试那条通道，也不要瞎请求）
        assert_eq!(quark_share_id("https://example.com/s/abc123"), None);
        assert_eq!(quark_share_id("https://pan.quark.cn/list#all"), None);
        assert_eq!(quark_share_id("https://pan.quark.cn/s/"), None);
        assert_eq!(quark_share_id("https://pan.quark.cn/s/有中文"), None);
        assert_eq!(quark_share_id("随便一句话"), None);
    }

    /// 夸克响应里文件名藏在好几个地方（单文件在 `title`，文件夹在 `list`），都要收全。
    #[test]
    fn collects_quark_file_names_from_a_real_shaped_response() {
        let body: serde_json::Value = serde_json::from_str(
            r#"{
                "status": 200,
                "code": 0,
                "data": {
                    "stoken": "dummy-stoken-for-test",
                    "title": "枫之助-0.1.0.zip",
                    "share": { "title": "枫之助-0.1.0.zip", "file_num": 1 },
                    "list": [
                        { "fid": "a1", "file_name": "枫之助-0.1.1.zip", "dir": false },
                        { "fid": "a2", "file_name": "使用说明.txt", "dir": false }
                    ]
                }
            }"#,
        )
        .expect("这段 JSON 就照真响应的形状写的");

        let mut names = Vec::new();
        collect_file_names(&body, &mut names);
        assert!(names.iter().any(|n| n == "枫之助-0.1.1.zip"));
        assert!(names.iter().any(|n| n == "使用说明.txt"));

        // 列表里的版本比标题里的新 —— 取最大的那个（新版传上去、旧文件还在的时候）
        assert_eq!(version_from_page(&names.join("\n")), Some((0, 1, 1)));
    }

    /// 「文件夹里装 zip」这种分享的坑：只看到文件夹名时既不许报出一个假版本，
    /// 也不许说成「已是最新」—— 事实上这个时候我们什么都不知道。
    #[test]
    fn folder_name_is_not_a_version() {
        assert_eq!(version_from_page("枫之助"), None);
        assert_eq!(version_from_page("枫之助\n使用说明.txt"), None);
        // 进到文件夹里之后读到的才是真文件名
        assert_eq!(version_from_page("枫之助-0.1.1.zip"), Some((0, 1, 1)));
    }

    /// 内置地址必须是「能用」的形状。
    ///
    /// 防的是手抄链接时少一个字符：那种错在自己机器上不一定看得出来（顶多一句
    /// 「没能确认」），而在朋友的机器上就是**永远不提示更新**。夸克链接尤其明显 ——
    /// 抄错的夸克链接会从「夸克通道」掉到「页面里找文件名」那条路，而夸克分享页里
    /// 根本没有文件名，于是永远读不出结果。
    #[test]
    fn builtin_update_url_is_usable() {
        if DEFAULT_UPDATE_URL.is_empty() {
            return; // 留空 = 故意不检查
        }
        assert!(
            DEFAULT_UPDATE_URL.starts_with("https://"),
            "更新页地址必须是 https：{}",
            DEFAULT_UPDATE_URL
        );
        if DEFAULT_UPDATE_URL.contains("quark.cn") {
            assert!(
                quark_share_id(DEFAULT_UPDATE_URL).is_some(),
                "夸克分享链接的形状不对（应当是 https://pan.quark.cn/s/<ID>）：{}",
                DEFAULT_UPDATE_URL
            );
        }
        // 界面上的「更新页」链接必须和程序自己去检查的是同一个地址
        assert_eq!(page_url(), DEFAULT_UPDATE_URL);
    }

    /// 真网络：拿真的夸克分享页跑一次。
    ///
    /// 默认跳过（会是网络请求），需要时手动跑：
    /// `cargo test -- --ignored`。发布前跑一次就能确认「朋友那边点得开更新提示」。
    #[tokio::test]
    #[ignore = "需要能访问夸克网盘（cargo test -- --ignored）"]
    async fn real_quark_share_is_readable() {
        assert!(
            !DEFAULT_UPDATE_URL.is_empty(),
            "内置更新页地址是空的，检查永远不会跑"
        );
        let info = check(DEFAULT_UPDATE_URL).await;
        assert!(info.error.is_none(), "读失败：{:?}", info.error);
        assert_eq!(info.source, "quark");
        assert!(info.latest.is_some(), "没读出最新版本号");
        log::info!("夸克分享上的版本：{:?}", info.latest);
    }
}
