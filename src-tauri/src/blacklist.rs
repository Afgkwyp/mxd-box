//! 避坑黑名单：**比对**与**自动嗅探**。
//!
//! ## 为什么要有这个模块
//!
//! 原来的黑名单是一个「能录、能查表」的本地表格，配一个前端定时读剪贴板的嗅探。
//! 三个真问题：
//!
//! 1. **嗅探在真正需要的时候不工作**。前端那个 `setInterval` 只挂在黑名单页上，
//!    切到别的页签就没了；主窗口关到托盘之后更是整条链路都不存在。而「要查一个
//!    名字」的时刻恰恰是**正在游戏里交易**的时候 —— 那时候你既不在黑名单页，
//!    主窗口多半也是收着的。
//! 2. **比对太粗**。原来按 `player_name = ?` 精确匹配，也就是
//!    「张三 」和「张三」被当成两个人（从聊天框或网页复制来的名字常常带空格，
//!    输入法还会给出全角字符）；而且不看区服 —— 别的服有个同名的人，
//!    也会被判成「命中」，反过来就是误伤。
//! 3. **录重了没人管**。同一个骗子记两次就是两行，导出给别人也是一串重复。
//!
//! 所以这里做三件事：
//!
//! * `normalize`：把名字归一化（去空格、全角转半角、英文小写）之后再比；
//! * `judge`：给出**四档**结论 —— 确定命中 / 只在别的服记过 / 名字很像 / 本地无记录，
//!   并把「同名但不能确认是同一人」这类实情说清楚，而不是二值地喊「安全 / 危险」；
//! * `spawn_clipboard_watcher`：把嗅探搬到 Rust 侧，**与界面是否打开无关**，
//!   命中就响铃 + 右下角弹提醒窗（复用语开服提醒那个通道）。
//!
//! 嗅探只用 `GetClipboardSequenceNumber` 判断「剪贴板有没有换过内容」，
//! 变了才真去读文本 —— 不轮询读取内容本身，也就不会在别的程序持有剪贴板时
//! 频繁撞 `OpenClipboard` 失败。

use crate::db::{BlacklistEntry, Database};
use serde::Serialize;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager};

/// 剪贴板轮询间隔。`GetClipboardSequenceNumber` 只是个计数器读取，很便宜。
const POLL_INTERVAL: Duration = Duration::from_millis(700);
/// 和 alert.rs 的剪贴板冷却一致：同名命中到期后仍应允许再次提醒。
const CLIPBOARD_ALERT_DEDUP: Duration = Duration::from_secs(90);

/// 读失败时不消费序号：剪贴板可能暂时被别的程序占用，同一代要留给下一轮重试。
fn read_new_clipboard_text(
    sequence: u32,
    last_sequence: &mut u32,
    read_text: impl FnOnce() -> Option<String>,
) -> Option<String> {
    if sequence == 0 || sequence == *last_sequence {
        return None;
    }
    let text = read_text()?;
    *last_sequence = sequence;
    Some(text)
}

fn was_recently_alerted(last_alerted: Option<&(String, Instant)>, key: &str, now: Instant) -> bool {
    matches!(
        last_alerted,
        Some((last, at))
            if last.as_str() == key && now.saturating_duration_since(*at) < CLIPBOARD_ALERT_DEDUP
    )
}

/// 玩家名字的长度范围（按字符数）。低于 2 个字符的东西（「1」「。」）几乎一定
/// 是误复制，拿去比对只会制造噪音。
const MIN_NAME_CHARS: usize = 2;
const MAX_NAME_CHARS: usize = 16;

/// 一次比对的结论。
#[derive(Debug, Clone, Serialize)]
pub struct BlacklistVerdict {
    /// `hit`（确定命中）/ `other_server`（只在别的服记过）/ `similar`（名字很像）
    /// / `clear`（本地无记录）
    pub status: String,
    /// 真正拿去比对的那个名字（去掉空格、全角转半角之后）
    pub checked_name: String,
    /// 比对时用的区服；没取到时是空串
    pub checked_server: String,
    /// 本地库里一共有多少条记录（结论的「样本量」）
    pub total_local: usize,
    /// 确定命中 / 别的服命中时那条记录
    pub entry: Option<BlacklistEntry>,
    /// 相关记录（同名多服、或是「名字很像」的那些）
    pub related: Vec<BlacklistEntry>,
    /// 一句话解释这个结论是怎么来的
    pub note: String,
}

/// 名字归一化：去空格、全角转半角、英文小写。
///
/// 这三件事都是「肉眼看上去一样、字符串却不同」的经典来源，而它们恰好又最常
/// 出现在复制粘贴的路径上（网页表格里的全角括号、名字后面跟的空格）。
pub fn normalize(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    for ch in name.chars() {
        // 各类空白（含全角空格 U+3000、不间断空格）一律丢掉：
        // 游戏角色名里不会有空格。
        if ch.is_whitespace() || ch == '\u{3000}' || ch == '\u{00a0}' {
            continue;
        }
        // 全角 ASCII（！～～）折回半角，全角字母数字与全角标点都能对上
        let code = ch as u32;
        let folded = if (0xFF01..=0xFF5E).contains(&code) {
            char::from_u32(code - 0xFEE0).unwrap_or(ch)
        } else {
            ch
        };
        for lower in folded.to_lowercase() {
            out.push(lower);
        }
    }
    out
}

/// 判断一段剪贴板文本「像不像一个角色名」。
///
/// 目的不是识别得有多准，而是**别乱报**：从网页复制的正文、导出的 JSON、
/// 一整段聊天记录，都不该被拿去比对（否则「命中」会变成廉价噪音，
/// 用户很快就学会无视这个提醒）。
/// 这段文本看起来是不是一个网址。
///
/// 大小写不敏感是**必须的**：以前只挡小写的 "http"，于是复制一句
/// 「HTTPS://MXDC.DVG.CN/...」就会被当成角色名拿去比对，还可能因为
/// 命中某条记录而响铃 —— 剪贴板里出现网址是极常见的事。
fn looks_like_url(text: &str) -> bool {
    let lowered = text.to_ascii_lowercase();
    lowered.contains("http")
        || lowered.contains("www.")
        || lowered.contains("://")
        || lowered.contains(".com")
        || lowered.contains(".cn/")
}

pub fn plausibly_player_name(text: &str) -> Option<String> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return None;
    }
    // 换行 = 多行内容（一段文字、一份 JSON），不是名字
    if trimmed.contains('\n') || trimmed.contains('\r') {
        return None;
    }
    // 内部还有空白，或者本来就长得像数据，直接放弃
    if trimmed.chars().any(|c| c.is_whitespace()) {
        return None;
    }
    if trimmed.starts_with('{')
        || trimmed.starts_with('[')
        || trimmed.starts_with('<')
        || looks_like_url(trimmed)
    {
        return None;
    }

    let count = trimmed.chars().count();
    if !(MIN_NAME_CHARS..=MAX_NAME_CHARS).contains(&count) {
        return None;
    }
    // 纯数字 / 纯标点也不算名字（金币数、价格、QQ 号都会被复制）
    if !trimmed.chars().any(|c| c.is_alphanumeric()) {
        return None;
    }
    if trimmed.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }

    Some(trimmed.to_string())
}

/// 一次比对的完整判断。**纯函数**：不碰数据库，所以能用单测把规则钉住。
///
/// 四档结论的动机：
///
/// * `hit` —— 同名 + 同服（或记录是「全服通用」）。可以直接提醒。
/// * `other_server` —— 名字一模一样，但记录挂在别的服上。游戏里同名很常见，
///   直接说「命中黑名单」就是误伤好人，所以只提示「这个名字在 X 服被记过，
///   自己判断」。
/// * `similar` —— 名字互相包含（≥3 个字）。用来抓「张三」vs「张三丰」这种
///   小号式改名，属于提醒，不是判定。
/// * `clear` —— 本地库里没有。要说清「这只代表本机没有记录」，不能反过来当
///   免死金牌。
pub fn judge(entries: &[BlacklistEntry], name: &str, server: Option<&str>) -> BlacklistVerdict {
    let key = normalize(name);
    let checked_server = server.unwrap_or("").to_string();
    let mut verdict = BlacklistVerdict {
        status: "clear".to_string(),
        checked_name: key.clone(),
        checked_server: checked_server.clone(),
        total_local: entries.len(),
        entry: None,
        related: Vec::new(),
        note: String::new(),
    };

    if key.is_empty() {
        verdict.note = "名字是空的，没法比对。".to_string();
        return verdict;
    }

    let same_name: Vec<&BlacklistEntry> = entries
        .iter()
        .filter(|entry| normalize(&entry.player_name) == key)
        .collect();

    if !same_name.is_empty() {
        // 记录本身就标了「全服通用」时，任何区服都算命中
        let on_this_server: Vec<&BlacklistEntry> = same_name
            .iter()
            .copied()
            .filter(|entry| {
                entry.server == "全服"
                    || (!checked_server.is_empty() && entry.server == checked_server)
            })
            .collect();

        if !on_this_server.is_empty() {
            verdict.status = "hit".to_string();
            verdict.entry = Some(on_this_server[0].clone());
            verdict.related = on_this_server
                .iter()
                .skip(1)
                .map(|entry| (*entry).clone())
                .collect();
            verdict.note = format!(
                "同名 + 同服，本地库 {} 条记录{}。",
                on_this_server.len(),
                if checked_server.is_empty() {
                    "".to_string()
                } else {
                    format!("（区服：{}）", checked_server)
                }
            );
            return verdict;
        }

        // 只有别的服的记录：提醒但不判死 —— 同名很常见
        verdict.status = "other_server".to_string();
        verdict.entry = Some(same_name[0].clone());
        verdict.related = same_name.iter().map(|entry| (*entry).clone()).collect();
        verdict.note = match &checked_server {
            server if !server.is_empty() => format!(
                "「{}」在别的区服被记过，本服（{}）没有记录。同名不代表同一人，请自行判断。",
                same_name[0].server,
                server
            ),
            _ => "只在别的区服被记过，本服没有记录。同名不代表同一人，请自行判断。".to_string(),
        };
        return verdict;
    }

    // 没有完全同名的：找「名字互相包含」的可疑项。
    //
    // 两边的门槛不一样，故意如此：
    //   * **已记录的名字至少 3 个字** —— 「张三」被记过，不代表「张三丰」有问题，
    //     两个字的名字太泛了，拿它去包含匹配会满屏噪音；
    //   * **查的名字至少 2 个字** —— 「张三」这三个字里其实只输错了一个字，
    //     这种小号式改名正是想抓的目标（单字查询则一律不比）。
    let key_chars = key.chars().count();
    if key_chars >= 2 {
        for entry in entries {
            let other = normalize(&entry.player_name);
            let other_chars = other.chars().count();
            if other_chars >= 3 && (other.contains(&key) || key.contains(&other)) {
                verdict.related.push(entry.clone());
            }
        }
        if !verdict.related.is_empty() {
            verdict.status = "similar".to_string();
            verdict.note = format!(
                "没有完全同名，但有 {} 个名字很接近，可能是小号式改名。",
                verdict.related.len()
            );
            return verdict;
        }
    }

    // 注意：这里是纯文本（界面直接渲染，不解析 markdown），别在这里写 `**加粗**`，
    // 它会原样显示成星号。
    verdict.note = format!(
        "本地库 {} 条里没有「{}」。不代表他干净，大额交易请走担保。",
        entries.len(),
        name.trim()
    );
    verdict
}

/// 预设区服的**名字**（黑名单按名字分区服，而设置里存的是 id）。
fn preset_server_name(db: &Database) -> Option<String> {
    let id = db.get_all_settings().default_server_id;
    crate::mxdc_client::server_list()
        .into_iter()
        .find(|server| server.id == id)
        .map(|server| server.name)
}

/// 比对一次并给出结论（命令与嗅探共用）。
pub fn check(db: &Database, name: &str, server: Option<String>) -> Result<BlacklistVerdict, String> {
    let entries = db
        .get_blacklist()
        .map_err(|e| format!("读取黑名单失败: {}", e))?;
    let server = server.or_else(|| preset_server_name(db));
    Ok(judge(&entries, name, server.as_deref()))
}

// ---------------------------------------------------------------------------
// 剪贴板嗅探
// ---------------------------------------------------------------------------

#[cfg(windows)]
mod clipboard {
    use std::ffi::c_void;

    #[link(name = "user32")]
    extern "system" {
        fn GetClipboardSequenceNumber() -> u32;
        fn OpenClipboard(hwnd: *mut c_void) -> i32;
        fn CloseClipboard() -> i32;
        fn IsClipboardFormatAvailable(format: u32) -> i32;
        fn GetClipboardData(format: u32) -> *mut c_void;
    }

    #[link(name = "kernel32")]
    extern "system" {
        fn GlobalLock(mem: *mut c_void) -> *mut c_void;
        fn GlobalUnlock(mem: *mut c_void) -> i32;
        fn GlobalSize(mem: *mut c_void) -> usize;
    }

    /// `CF_UNICODETEXT`
    const CF_UNICODETEXT: u32 = 13;

    /// 剪贴板的「第几代」计数。每次有程序写入剪贴板它都会变 ——
    /// 我们只读这个计数，变了才真的去读内容。
    pub fn sequence() -> u32 {
        unsafe { GetClipboardSequenceNumber() }
    }

    /// 读出剪贴板里的文本。拿不到（别的程序正占着剪贴板、或里面不是文本）就返回 None。
    ///
    /// 加锁失败是正常现象（剪贴板是全局独占资源），所以调用方不该把它当错误，
    /// 下一次轮询再来就是了。
    pub fn read_text() -> Option<String> {
        unsafe {
            if OpenClipboard(std::ptr::null_mut()) == 0 {
                return None;
            }

            let result = (|| {
                if IsClipboardFormatAvailable(CF_UNICODETEXT) == 0 {
                    return None;
                }
                let handle = GetClipboardData(CF_UNICODETEXT);
                if handle.is_null() {
                    return None;
                }

                // 先问大小再读，不依赖字符串自己带终止符 —— 越界读是这个 API
                // 最容易写错的地方。
                let bytes = GlobalSize(handle);
                if bytes < 2 {
                    return None;
                }
                let max_units = (bytes / 2).min(4096);

                let ptr = GlobalLock(handle) as *const u16;
                if ptr.is_null() {
                    return None;
                }

                let mut len = 0usize;
                while len < max_units && *ptr.add(len) != 0 {
                    len += 1;
                }
                let slice = std::slice::from_raw_parts(ptr, len);
                let text = String::from_utf16_lossy(slice);

                GlobalUnlock(handle);
                Some(text)
            })();

            CloseClipboard();
            result
        }
    }
}

#[cfg(not(windows))]
mod clipboard {
    pub fn sequence() -> u32 {
        0
    }
    pub fn read_text() -> Option<String> {
        None
    }
}

/// 起一个常驻线程盯着剪贴板：复制到角色名且命中黑名单时报警。
///
/// **和界面无关**：主窗口收进托盘、切到别的页签都不影响它 ——
/// 「正在游戏里交易」正是最需要它的时候。
///
/// 只在**确定命中**（`hit`）时报警。「别的服有同名」和「名字很像」都只写日志，
/// 因为那两档是「请你自己看一眼」，不是「这个人有问题」——拿它们弹窗响铃，
/// 用户很快就会把这个提醒当噪音。
pub fn spawn_clipboard_watcher(app: AppHandle) {
    std::thread::spawn(move || {
        let mut last_sequence = clipboard::sequence();
        let mut last_alerted: Option<(String, Instant)> = None;

        loop {
            std::thread::sleep(POLL_INTERVAL);

            let sequence = clipboard::sequence();
            let Some(text) =
                read_new_clipboard_text(sequence, &mut last_sequence, clipboard::read_text)
            else {
                continue;
            };
            let Some(name) = plausibly_player_name(&text) else {
                continue;
            };

            let Some(state) = app.try_state::<crate::commands::AppState>() else {
                continue;
            };
            // 开关在设置里，读一次很便宜（一行 SQLite 查询，只在剪贴板变化时才读）
            if !state.db.get_all_settings().blacklist_watch {
                continue;
            }

            let server = preset_server_name(&state.db);
            let verdict = match check(&state.db, &name, server) {
                Ok(verdict) => verdict,
                Err(err) => {
                    log::warn!("剪贴板比对失败：{}", err);
                    continue;
                }
            };

            if verdict.status != "hit" {
                log::debug!(
                    "剪贴板里的「{}」比对结果：{}（不提醒）",
                    name,
                    verdict.status
                );
                continue;
            }

            let key = verdict.checked_name.clone();
            let now = Instant::now();
            if was_recently_alerted(last_alerted.as_ref(), &key, now) {
                continue;
            }

            let Some(entry) = verdict.entry.clone() else {
                continue;
            };
            if !crate::alert::fire_blacklist_hit(&app, &entry) {
                continue;
            }
            last_alerted = Some((key, Instant::now()));
            log::info!(
                "剪贴板命中黑名单：「{}」（{} · {}）",
                entry.player_name,
                entry.server,
                entry.category
            );

            // 主窗口那边（如果开着）挂一条横幅，方便立刻看细节
            if let Err(err) = app.emit_to("main", "blacklist-hit", &verdict) {
                log::debug!("投递黑名单命中事件失败（提醒窗仍然会弹）：{}", err);
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(server: &str, name: &str) -> BlacklistEntry {
        BlacklistEntry {
            id: None,
            server: server.to_string(),
            player_name: name.to_string(),
            category: "骗子".to_string(),
            reason: "跑单".to_string(),
            created_at: "2026/09/20".to_string(),
        }
    }

    #[test]
    fn failed_clipboard_read_retries_the_same_sequence() {
        let mut last_sequence = 10;
        assert_eq!(
            read_new_clipboard_text(11, &mut last_sequence, || None),
            None
        );
        assert_eq!(last_sequence, 10, "读失败不能消费序号");
        assert_eq!(
            read_new_clipboard_text(11, &mut last_sequence, || Some("张三".to_string())),
            Some("张三".to_string())
        );
        assert_eq!(last_sequence, 11);
        assert_eq!(
            read_new_clipboard_text(11, &mut last_sequence, || Some("不会读取".to_string())),
            None
        );
    }

    #[test]
    fn same_name_clipboard_dedup_expires_with_the_90_second_cooldown() {
        let now = Instant::now();
        let last = ("张三".to_string(), now);
        assert!(was_recently_alerted(
            Some(&last),
            "张三",
            now + Duration::from_secs(89)
        ));
        assert!(!was_recently_alerted(
            Some(&last),
            "张三",
            now + Duration::from_secs(90)
        ));
        assert!(!was_recently_alerted(
            Some(&last),
            "李四",
            now + Duration::from_secs(1)
        ));
    }

    #[test]
    fn normalize_strips_spaces_and_folds_width_and_case() {
        assert_eq!(normalize("张 三"), "张三");
        assert_eq!(normalize(" 张三 "), "张三");
        // 全角字母数字 + 大写字 → 半角小写
        assert_eq!(normalize("ＡＢＣ１２３"), "abc123");
        // 全角空格
        assert_eq!(normalize("李\u{3000}四"), "李四");
    }

    #[test]
    fn judge_hits_on_same_server() {
        let entries = vec![entry("绿水灵", "盗号小贼")];
        let verdict = judge(&entries, "盗号小贼", Some("绿水灵"));
        assert_eq!(verdict.status, "hit");
        assert!(verdict.entry.is_some());
    }

    #[test]
    fn judge_hit_survives_whitespace_in_the_copied_name() {
        let entries = vec![entry("绿水灵", "盗号小贼")];
        let verdict = judge(&entries, " 盗号 小贼 ", Some("绿水灵"));
        assert_eq!(verdict.status, "hit");
        assert_eq!(verdict.checked_name, "盗号小贼");
    }

    #[test]
    fn judge_flags_other_server_without_convicting() {
        let entries = vec![entry("蘑菇仔", "路人甲")];
        let verdict = judge(&entries, "路人甲", Some("绿水灵"));
        assert_eq!(verdict.status, "other_server");
        assert!(verdict.note.contains("自行判断"));
    }

    #[test]
    fn judge_treats_global_records_as_hits_anywhere() {
        let entries = vec![entry("全服", "职业骗子")];
        let verdict = judge(&entries, "职业骗子", Some("小白兔"));
        assert_eq!(verdict.status, "hit");
    }

    #[test]
    fn judge_warns_about_similar_names() {
        let entries = vec![entry("绿水灵", "张三丰")];
        let verdict = judge(&entries, "张三", Some("绿水灵"));
        assert_eq!(verdict.status, "similar");
        assert_eq!(verdict.related.len(), 1);
    }

    #[test]
    fn judge_does_not_guess_from_two_character_names() {
        let entries = vec![entry("绿水灵", "小霸")];
        // 两个字的名字不做「互相包含」的猜测，避免噪音
        let verdict = judge(&entries, "小", Some("绿水灵"));
        assert_eq!(verdict.status, "clear");
    }

    #[test]
    fn judge_says_clear_without_pretending_it_is_a_clean_record() {
        let entries = vec![entry("绿水灵", "张三")];
        let verdict = judge(&entries, "王五", Some("绿水灵"));
        assert_eq!(verdict.status, "clear");
        assert!(verdict.note.contains("不代表他干净"));
    }

    #[test]
    fn plausibly_player_name_accepts_real_names_only() {
        assert_eq!(
            plausibly_player_name(" 盗号小贼 "),
            Some("盗号小贼".to_string())
        );
        assert!(plausibly_player_name("").is_none());
        assert!(plausibly_player_name("1").is_none());
        assert!(plausibly_player_name("1234567890").is_none());
        assert!(plausibly_player_name("张三\n李四").is_none());
        assert!(plausibly_player_name("张三 李四").is_none());
        assert!(plausibly_player_name("[{\"server\":\"蘑菇仔\"}]").is_none());
        assert!(plausibly_player_name("https://mxdc.dvg.cn/").is_none());
        // 大小写同样要挡住：复制出来的网址经常是大写开头的
        assert!(plausibly_player_name("HTTPS://MXDC.DVG.CN/tools").is_none());
        assert!(plausibly_player_name("WWW.MXDC.DVG.CN").is_none());
        assert!(plausibly_player_name("mxdc.dvg.cn/a").is_none());
        assert!(plausibly_player_name("一个名字长到超过十六个字的玩家昵称").is_none());
        assert!(plausibly_player_name("。。。").is_none());
    }
}
