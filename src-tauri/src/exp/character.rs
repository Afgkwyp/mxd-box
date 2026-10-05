//! 认「现在玩的是哪个角色」：状态栏 `LV.` 右边那两行字（上一行职业、下一行角色名）。
//!
//! 很多人至少练两个号。不分角色的话，角色列表做不出来，历史里两个号的记录也混在一起。
//!
//! # 角色靠什么认：名字那一块的像素指纹，不是认出来的字
//!
//! 和地图同一个思路（见 [`crate::exp::map`]）：同一个角色名，那一块像素逐点相同，
//! 指纹是精确的。OCR 只负责把名字和职业**读出来给人看** —— 角色名是玩家自己起的，
//! 中文、字母、数字什么都有，模型偶尔认错一个字很正常；要是拿认出来的字当身份，
//! 认错一次就多出一个「新角色」，历史也跟着劈成两半。
//!
//! 指纹换了分辨率会变，所以库里一个角色可以挂多个指纹；新指纹读出来的名字和
//! 已有角色一样时并到那个角色上（见 `Database::resolve_character`）。
//!
//! # 职业会变
//!
//! 转职之后名字不变、职业那一行变了，所以职业那一行单独记一个指纹，变了就重读。

use crate::exp::capture;
use crate::exp::font::Pixels;
use crate::exp::map;
use crate::exp::ocr;
use std::collections::HashMap;
use std::time::{Duration, Instant};

/// 隔多久看一眼（换角色要过一遍选人界面，用不着每帧都抓）。
const LOOK_INTERVAL: Duration = Duration::from_secs(2);
/// 名字没读出来时隔多久再试。
const RETRY_INTERVAL: Duration = Duration::from_secs(8);
/// 亲眼确认过「还是这个角色」之后，这个结论管多久。
const VERIFIED_FOR: Duration = Duration::from_secs(6);
/// 经验读不到过一阵之后，多久之内还没重新确认角色就先别信读数是原来那个角色的。
/// 比状态栏重新定位的冷却（15 秒）加上两次确认的时间长一些。
const SUSPECT_FOR: Duration = Duration::from_secs(30);
/// 那一块里至少要有这么多个亮点才算「有字」（一两个亮点是噪声，不是名字）。
const MIN_INK: usize = 12;
/// 最亮的地方至少要比底亮这么多才算有字。
const MIN_TEXT_CONTRAST: i32 = 60;
/// 角色名最多 12 个字符；读出更长的就是把别的东西读进来了。
const MAX_NAME_CHARS: usize = 12;

/// 游戏里的职业名。读出来只差一个字的拉回到这张表上（`牧帅` → `牧师`）；
/// 表里没有的原样留着 —— 这张表不全也不要紧，它只管纠错，不管放行。
const JOBS: &[&str] = &[
    "新手", "战士", "剑客", "准骑士", "枪战士", "勇士", "骑士", "龙骑士", "英雄", "圣骑士",
    "黑骑士", "魔法师", "牧师", "祭司", "主教", "弓箭手", "猎人", "弩弓手", "射手", "游侠",
    "神射手", "箭神", "飞侠", "刺客", "侠客", "无影人", "独行客", "隐士", "侠盗", "海盗",
    "拳手", "火枪手", "斗士", "大副", "冲锋队长", "船长",
];

/// 这一刻看到的角色。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sighting {
    /// 名字那一块的像素指纹（角色的身份）
    pub mark: String,
    /// 读出来的名字；没读出来是空串（身份仍然由指纹定）
    pub name: String,
    /// 读出来的职业；没读出来是空串
    pub job: String,
}

pub struct CharacterReader {
    current: Option<Sighting>,
    /// 当前职业那一行的指纹（变了就重读职业）
    job_mark: Option<String>,
    /// 新指纹要连续两次一致才采纳（和地图同一条去抖规则）
    candidate: Option<String>,
    last_look: Option<Instant>,
    last_retry: Option<Instant>,
    /// 读过的名字按指纹记着：两个号来回切时不用每次都跑一遍模型
    names: HashMap<String, String>,
    /// 最近一次**亲眼看到**名字那一块就是当前角色的时刻。
    /// 经验读不到（可能正在换角色）时清空，见 [`CharacterReader::doubt`]。
    confirmed_at: Option<Instant>,
    doubted_at: Option<Instant>,
}

impl CharacterReader {
    pub fn new() -> Self {
        Self {
            current: None,
            job_mark: None,
            candidate: None,
            last_look: None,
            last_retry: None,
            names: HashMap::new(),
            confirmed_at: None,
            doubted_at: None,
        }
    }

    /// 经验这一轮没读到：人可能正在换角色，之前「还是这个角色」的结论作废，
    /// 要重新亲眼看到名字才算数。
    ///
    /// 为什么需要它：[`CharacterReader::look`] 抓不到 / 没轮到看的时候返回的是**上一次**
    /// 的角色。换角色回来的头十几秒状态栏还在重新定位，这期间经验已经读得出来、
    /// 角色却还是旧的 —— 真机上新角色的等级和经验就这样写到了旧角色头上。
    pub fn doubt(&mut self) {
        self.confirmed_at = None;
        // 每次读不到都往后推：选人界面停多久都行，窗口从**最后一次**读不到算起
        self.doubted_at = Some(Instant::now());
    }

    /// 刚刚亲眼确认过现在就是 [`CharacterReader::look`] 返回的那个角色。
    pub fn verified(&self) -> bool {
        self.candidate.is_none()
            && self.confirmed_at.is_some_and(|at| at.elapsed() <= VERIFIED_FOR)
    }

    /// 经验刚恢复、角色还没重新确认：这几帧的读数先别算在原来那个角色头上。
    /// 只管一小段时间（[`SUSPECT_FOR`]）—— 名字那一块一直看不到（被面板盖住）时
    /// 不能因此让统计永远停着。
    pub fn suspect(&self) -> bool {
        self.confirmed_at.is_none()
            && self.doubted_at.is_some_and(|at| at.elapsed() <= SUSPECT_FOR)
    }

    /// 名字那一块正在变（看到了新指纹、还没连续两次确认）。
    ///
    /// 这几秒里读到的经验是谁的还说不准，调用方不该把它记进正在统计的那一段。
    pub fn unsettled(&self) -> bool {
        self.candidate.is_some()
    }

    /// 看一眼状态栏，返回当前确认的角色（还没见过任何角色时是 `None`）。
    ///
    /// 抓不到、那一块没有字（被面板盖住 / 过场）时留着上一次的结果。
    pub fn look(
        &mut self,
        hwnd: isize,
        name_rect: (i32, i32, i32, i32),
        job_rect: (i32, i32, i32, i32),
    ) -> Option<Sighting> {
        // 有候选在等确认时不限流：越早确认，「说不准是谁」的那几秒越短
        if self.candidate.is_none() {
            if let Some(last) = self.last_look {
                if last.elapsed() < LOOK_INTERVAL {
                    return self.current.clone();
                }
            }
        }
        self.last_look = Some(Instant::now());

        let grab = |rect: (i32, i32, i32, i32)| {
            capture::grab_client(hwnd, rect.0, rect.1, rect.2, rect.3).map(|(frame, _)| frame)
        };
        let Some(name_frame) = grab(name_rect) else {
            return self.current.clone();
        };
        let name_pixels = name_frame.pixels();
        let Some(mark) = text_mark(&name_pixels) else {
            return self.current.clone();
        };

        let same = self.current.as_ref().is_some_and(|seen| seen.mark == mark);
        if !same {
            // 新指纹：连续两次一致才采纳
            if self.candidate.as_deref() != Some(mark.as_str()) {
                self.candidate = Some(mark);
                return self.current.clone();
            }
            self.candidate = None;
            self.last_retry = Some(Instant::now());
            let name = match self.names.get(&mark) {
                Some(name) => name.clone(),
                None => self.read_name(&mark, &name_pixels),
            };
            self.current = Some(Sighting {
                mark,
                name,
                job: String::new(),
            });
            self.job_mark = None;
            self.confirmed_at = Some(Instant::now());
        } else {
            self.candidate = None;
            self.confirmed_at = Some(Instant::now());
            let retry_due = self
                .last_retry
                .map_or(true, |at| at.elapsed() >= RETRY_INTERVAL);
            if self.current.as_ref().is_some_and(|seen| seen.name.is_empty()) && retry_due {
                self.last_retry = Some(Instant::now());
                let name = self.read_name(&mark, &name_pixels);
                if let Some(seen) = self.current.as_mut() {
                    seen.name = name;
                }
            }
        }

        // 职业那一行：指纹变了（刚换角色 / 转职了）才重读
        if let Some(job_frame) = grab(job_rect) {
            let job_pixels = job_frame.pixels();
            if let Some(job_mark) = text_mark(&job_pixels) {
                if self.job_mark.as_deref() != Some(job_mark.as_str()) {
                    if let Some(job) = read_job(&job_pixels) {
                        if let Some(seen) = self.current.as_mut() {
                            seen.job = job;
                        }
                        self.job_mark = Some(job_mark);
                    }
                }
            }
        }
        self.current.clone()
    }

    fn read_name(&mut self, mark: &str, pixels: &Pixels<'_>) -> String {
        match read_name(pixels) {
            Some(name) => {
                self.names.insert(mark.to_string(), name.clone());
                name
            }
            None => String::new(),
        }
    }
}

impl Default for CharacterReader {
    fn default() -> Self {
        Self::new()
    }
}

/// 那一块的指纹；没有字返回 `None`。
///
/// 「多亮算字」按这一块自己定：取底色和最亮处的中间偏上。原生大小时白字是 255，
/// 固定卡 200 没问题；但窗口比 1080p 小、整个界面被缩小之后，11 像素的字糊成七八像素，
/// 最亮处常常到不了 200 —— 固定门槛下那一行会被当成「没有字」，职业就一直读不出来。
fn text_mark(pixels: &Pixels<'_>) -> Option<String> {
    if pixels.width == 0 || pixels.height == 0 {
        return None;
    }
    let mut histogram = [0u32; 256];
    let mut peak = 0i32;
    for y in 0..pixels.height {
        for x in 0..pixels.width {
            let luma = pixels.luma(x, y);
            histogram[luma as usize] += 1;
            peak = peak.max(luma as i32);
        }
    }
    let background = histogram
        .iter()
        .enumerate()
        .max_by_key(|(_, count)| **count)
        .map(|(luma, _)| luma as i32)?;
    if peak - background < MIN_TEXT_CONTRAST {
        return None;
    }
    let threshold = (background + (peak - background) * 3 / 5) as u8;
    let ink: u32 = histogram[threshold as usize..].iter().sum();
    (ink as usize >= MIN_INK)
        .then(|| format!("{:016x}", map::fingerprint_above(pixels, threshold)))
}

/// 读角色名。名字什么字符都可能有，所以只卡长度和置信度，不卡「得是汉字」。
pub(crate) fn read_name(pixels: &Pixels<'_>) -> Option<String> {
    let plausible = |reading: &ocr::Reading| {
        let count = reading.text.chars().count();
        (1..=MAX_NAME_CHARS).contains(&count) && reading.confidence >= 0.6
    };
    let reading = map::read_white_text(pixels, plausible)?;
    plausible(&reading).then_some(reading.text)
}

/// 读职业，并拉回到职业表上。
pub(crate) fn read_job(pixels: &Pixels<'_>) -> Option<String> {
    let reading = map::read_white_text(pixels, ocr::looks_like_map_name)?;
    ocr::looks_like_map_name(&reading).then(|| settle_job(&reading.text))
}

/// 读出来的职业只差一个字、而且表里只有一个这样的，就用表里的。
fn settle_job(text: &str) -> String {
    if JOBS.contains(&text) {
        return text.to_string();
    }
    let near: Vec<&&str> = JOBS
        .iter()
        .filter(|job| {
            job.chars().count() == text.chars().count()
                && job.chars().zip(text.chars()).filter(|(a, b)| a != b).count() == 1
        })
        .collect();
    match near.as_slice() {
        [only] => only.to_string(),
        _ => text.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 真机素材：1080p 宽版 HUD 上职业那一行（340×15），写着 `牧师`。
    fn job_strip() -> Vec<u8> {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/hud_job_priest_340x15.bgra");
        std::fs::read(&path).expect("读不到职业素材")
    }

    /// 会真的跑一次识别模型。
    #[test]
    fn the_job_line_reads_from_a_real_hud() {
        let bgra = job_strip();
        let pixels = Pixels { width: 340, height: 15, bgra: &bgra };
        assert_eq!(read_job(&pixels).as_deref(), Some("牧师"));
        assert!(text_mark(&pixels).is_some());
    }

    /// 开发用：对着一块存下来的名字条（340×15 裸 BGRA）看读出来什么。
    /// 角色名是玩家自己的，不放进仓库当素材，所以这条不进常规测试。
    ///
    /// ```text
    /// NAME_STRIP=某个文件 cargo test --lib -- --ignored probe_name_strip --nocapture
    /// ```
    #[test]
    #[ignore]
    fn probe_name_strip() {
        let bgra = std::fs::read(std::env::var("NAME_STRIP").expect("要设 NAME_STRIP")).unwrap();
        let pixels = Pixels { width: 340, height: 15, bgra: &bgra };
        println!("指纹 {:?} · 名字 {:?}", text_mark(&pixels), read_name(&pixels));
    }

    /// 开发用：对着一整幅存下来的画面（裸 BGRA）走一遍「定位 HUD → 读名字和职业」。
    /// 换分辨率 / 全屏拉伸后读不读得出来，拿它验。
    ///
    /// ```text
    /// FRAME=某个文件 FRAME_W=2560 FRAME_H=1440 cargo test --lib -- --ignored probe_identity_frame --nocapture
    /// ```
    #[test]
    #[ignore]
    fn probe_identity_frame() {
        let number = |key: &str| -> usize { std::env::var(key).unwrap().parse().unwrap() };
        let (width, height) = (number("FRAME_W"), number("FRAME_H"));
        let bgra = std::fs::read(std::env::var("FRAME").expect("要设 FRAME")).unwrap();
        let pixels = Pixels { width, height, bgra: &bgra };
        let Some(layout) = crate::exp::hud::locate(&pixels) else {
            println!("{width}×{height}: HUD 没定位到");
            return;
        };
        let cut = |rect: (i32, i32, i32, i32)| {
            let (w, h, buffer) =
                pixels.crop(rect.0 as usize, rect.1 as usize, rect.2 as usize, rect.3 as usize);
            (w, h, buffer)
        };
        let read = |rect: Option<(i32, i32, i32, i32)>, name: bool| {
            let (w, h, buffer) = cut(rect?);
            let strip = Pixels { width: w, height: h, bgra: &buffer };
            let text = if name { read_name(&strip) } else { read_job(&strip) };
            Some((text, text_mark(&strip).is_some()))
        };
        println!(
            "{width}×{height}: {} 缩放 {:.3} · 名字 {:?} · 职业 {:?}",
            layout.anchor.skin.label(),
            layout.anchor.sx,
            read(layout.name_text, true),
            read(layout.job_text, false),
        );
    }

    /// 经验读不到过之后，要重新看到名字才算「确认是这个角色」。
    #[test]
    fn a_failed_read_voids_the_confirmation() {
        let mut reader = CharacterReader::new();
        assert!(!reader.verified());
        assert!(!reader.suspect(), "从没出过问题时不算可疑");

        reader.confirmed_at = Some(Instant::now());
        assert!(reader.verified());
        reader.doubt();
        assert!(!reader.verified());
        assert!(reader.suspect());

        // 可疑只管一小段时间：名字一直看不到时不能让统计永远停着
        reader.doubted_at = Some(Instant::now() - SUSPECT_FOR - Duration::from_secs(1));
        assert!(!reader.suspect());

        // 确认过期也不算数
        reader.confirmed_at = Some(Instant::now() - VERIFIED_FOR - Duration::from_secs(1));
        assert!(!reader.verified());
    }

    #[test]
    fn an_empty_strip_has_no_mark() {
        let flat = vec![60u8; 40 * 15 * 4];
        assert!(text_mark(&Pixels { width: 40, height: 15, bgra: &flat }).is_none());
    }

    #[test]
    fn a_misread_job_snaps_to_the_table() {
        assert_eq!(settle_job("牧师"), "牧师");
        assert_eq!(settle_job("牧帅"), "牧师");
        // 表里没有的原样留着
        assert_eq!(settle_job("魔导师(冰,雷)"), "魔导师(冰,雷)");
        // 差一个字的有两个（骑士 / 勇士）：不猜
        assert_eq!(settle_job("某士"), "某士");
    }
}
