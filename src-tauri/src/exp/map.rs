//! 认「现在在哪个地图」，用来给会话打上「在哪练的」。
//!
//! 它会出现在历史列表和小结卡片上，所以以前要用户自己填 —— 填的人不多、填错的不少。
//!
//! # 为什么**不**去读小地图上那几个字
//!
//! 先说清楚这条路为什么走不通，免得以后有人再试一遍。
//! 小地图里的地图名是 **11 像素高的抗锯齿中文**（白填充 + 黑描边 + 渐变，
//! 底色浅蓝）。用 Windows 自带的 OCR（`Windows.Media.Ocr`，本机
//! `Language.OCR~~~zh-CN` 是 `Installed`）实测过一轮：
//!
//! | 喂进去的东西 | OCR 读出来 |
//! | --- | --- |
//! | 64 像素清晰黑字（对照） | `坠 落 主 义 金 银 岛` ✅ |
//! | 24 像素清晰黑字 | `坠 落 主 义 金 银 岛` ✅ |
//! | 16 像素清晰黑字 | `坐 落 主 义 金 岛` ⚠️ 错两个字 |
//! | 12 像素清晰黑字 | `金 裉 島` ❌ |
//! | **游戏里的 11 像素地图名** | 空（二值化 / 填洞 / 灰度 / 原图，六种喂法全空） |
//!
//! 关键在最后两行：**连干净的黑字白底，12 像素都已经读不准了**。
//! 放大不会凭空补出细节 —— 它只是把同样贫瘠的信息摊开（所以 ×4 和 ×8 一样读不出）。
//! 而认错一个地名（「坐落主义」而不是「坠落主义」）比不显示更糟：
//! 一张写着别的地图的卡片发出去，比没写地图更没有用。
//!
//! # 所以走哪条路：**记指纹，认一次就永远认得出**
//!
//! 同一张地图，小地图那一块像素是**逐点一样**的（文字是固定的，不随角色移动变化）。
//! 于是：
//!
//! 1. 每次采样算一个像素指纹（FNV-1a，几十微秒）；
//! 2. 指纹在本地库里查得到名字 → 直接用，**用户什么都不用做**；
//! 3. 查不到（第一次来这张图）→ 界面请他填一次，填完记下来；
//! 4. 以后每次来这张图都是自动的。
//!
//! 指纹是精确匹配（同一张图必然逐点相同），所以**永远不会认错地名** ——
//! 这一点比 OCR 强，也是选它的主要理由。用户只需要每张新地图填一次，
//! 而不是每段会话填一次。
//!
//! 指纹用**像素内容**而不是「第几张小地图」这种序号，所以换分辨率、
//! 换客户端版本之后指纹自然会变（重新填一次），不会张冠李戴。

use crate::exp::capture;
use crate::exp::font::Pixels;
use crate::exp::ocr;
use std::collections::HashMap;
use std::time::{Duration, Instant};

/// 基准分辨率（真机测量时的客户区尺寸）。
const BASE_WIDTH: f64 = 1920.0;
const BASE_HEIGHT: f64 = 1080.0;

/// 小地图里地图名那一块（1920×1080 基准下的客户区坐标）。
///
/// 小地图固定在游戏窗口左上角，框内是「地名（上一行）+ 地图名（下一行）」两行字。
/// 宽度取到小地图内沿**之前**：再往右一点就是小地图自己的边框，指纹会跟着
/// 角色在地图上的位置之外的无关像素一起抖。
pub const BASE_REGION: (i32, i32, i32, i32) = (48, 26, 68, 34);

/// 单独给识别用的两行（比整块更紧一点：别把框线带进去）。
///
/// 尺寸**固定**是有意的：识别模型的输入宽度是按长宽比算的，裁剪尺寸固定，
/// 模型宽度就固定（只有两个），于是「按宽度缓存已优化的模型」不会反复重建。
pub const BASE_LINE_REGION: (i32, i32, i32, i32) = (50, 27, 68, 15);
pub const BASE_MAP_LINE_REGION: (i32, i32, i32, i32) = (50, 43, 68, 16);

/// 隔多久重新看一眼小地图（换地图不用等太久，但也别每帧都抓）。
const LOOK_INTERVAL: Duration = Duration::from_secs(2);

/// 未识别地图的补偿重试冷却间隔（切图首帧认不出时，等画面稳定后重新识别的间隔）。
const RETRY_INTERVAL: Duration = Duration::from_secs(8);

/// 连续失败达到此阈值后，推翻当前分辨率下记住的模式，允许重新探测另一种模式。
pub const CONSECUTIVE_FAILURES_THRESHOLD: u32 = 3;

/// 小地图区域在当前窗口客户区下的计算模式。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RegionMode {
    /// 等比缩放模式：假设游戏 UI 随窗口分辨率等比拉伸。
    Scaled,
    /// 绝对像素模式：假设游戏 UI 为固定点阵尺寸（无论分辨率多大，小地图都保持 1080p 基准大小）。
    Absolute,
}

impl RegionMode {
    pub fn label(self) -> &'static str {
        match self {
            RegionMode::Scaled => "等比缩放(Scaled)",
            RegionMode::Absolute => "绝对像素(Absolute)",
        }
    }

    pub fn other(self) -> Self {
        match self {
            RegionMode::Scaled => RegionMode::Absolute,
            RegionMode::Absolute => RegionMode::Scaled,
        }
    }
}

/// 分辨率模式记忆条目：记住的模式 + 连续失败次数。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResolutionPreference {
    pub mode: RegionMode,
    pub consecutive_failures: u32,
}

/// 将 1080p 基准下的区域按当前窗口客户区尺寸等比缩放并夹紧在客户区内。
///
/// # 为什么做等比缩放与已知局限
///
/// 1. 为什么做等比缩放：
///    旧实现直接写死 1080p 客户区绝对坐标，一旦玩家在 800×600、1280×720、1366×768 或 2K/4K 窗口下运行，
///    原本位于小地图顶部的地名框就会抓空（抓到边框外、甚至越界导致 grab_client 返回 None），
///    地图识别模型和像素指纹整体瘫痪。按当前宽高相对 1920×1080 的比例缩放，
///    至少能保证采样框在不同分辨率下粗略对准小地图头部，不发生整体错位。
///
/// 2. 已知局限（让作者真机量出真实区域后有地方改）：
///    - 部分怀旧服客户端 UI 元素是固定像素大小（固定点阵，不随分辨率缩放），仅游戏视口变大，
///      此时小地图依然是固定 800×600 时代的固定像素，等比放大到 1080p/2K 会导致框比小地图本身还大；
///    - 部分基于 Unity 的怀旧服 UI 采用 CanvasScaler，但可能设置了不同的 Match 模式（如固定宽或固定高），
///      非 16:9 分辨率（如 4:3 的 800×600、1024×768，或 16:10 的 1680×1050）时，
///      长宽比不一致会导致等比拉伸产生微量偏移；
///    - 若未来作者在更多分辨率下实测获得精确坐标表，可在此处优先查表（如 800×600、1280×720 固定预设），
///      无预设时再退回此比例缩放。
pub fn scale_region(
    base: (i32, i32, i32, i32),
    client_w: i32,
    client_h: i32,
) -> (i32, i32, i32, i32) {
    if client_w <= 0 || client_h <= 0 {
        return (0, 0, 0, 0);
    }
    let scale_x = client_w as f64 / BASE_WIDTH;
    let scale_y = client_h as f64 / BASE_HEIGHT;

    let x = (base.0 as f64 * scale_x).round() as i32;
    let y = (base.1 as f64 * scale_y).round() as i32;
    // 宽和高至少保留 1 像素，避免退化为 0
    let w = ((base.2 as f64 * scale_x).round() as i32).max(1);
    let h = ((base.3 as f64 * scale_y).round() as i32).max(1);

    // 边界夹紧在客户区内部
    let clamped_x = x.clamp(0, (client_w - 1).max(0));
    let clamped_y = y.clamp(0, (client_h - 1).max(0));
    let clamped_w = w.min(client_w - clamped_x).max(1);
    let clamped_h = h.min(client_h - clamped_y).max(1);

    (clamped_x, clamped_y, clamped_w, clamped_h)
}

/// 将 1080p 基准下的区域按原始绝对像素坐标在当前客户区内夹紧（不缩放）。
///
/// 针对怀旧服客户端 UI 为固定点阵尺寸的假设：无论窗口拉大到 2K 还是 4K，
/// 或者是 800×600 老客户端，界面元素均按 1:1 绝对像素绘制在左上角。
pub fn absolute_region(
    base: (i32, i32, i32, i32),
    client_w: i32,
    client_h: i32,
) -> (i32, i32, i32, i32) {
    if client_w <= 0 || client_h <= 0 {
        return (0, 0, 0, 0);
    }
    let clamped_x = base.0.clamp(0, (client_w - 1).max(0));
    let clamped_y = base.1.clamp(0, (client_h - 1).max(0));
    let clamped_w = base.2.min(client_w - clamped_x).max(1);
    let clamped_h = base.3.min(client_h - clamped_y).max(1);
    (clamped_x, clamped_y, clamped_w, clamped_h)
}

/// 根据给定的区域模式（Scaled 或 Absolute）计算目标区域并夹紧在客户区内。
pub fn region_for(
    mode: RegionMode,
    base: (i32, i32, i32, i32),
    client_w: i32,
    client_h: i32,
) -> (i32, i32, i32, i32) {
    match mode {
        RegionMode::Scaled => scale_region(base, client_w, client_h),
        RegionMode::Absolute => absolute_region(base, client_w, client_h),
    }
}

/// 现在在哪个地图。
#[derive(Debug, Clone, PartialEq, Default)]
pub struct MapSnapshot {
    /// 认出来的地图名（库里查到的）
    pub name: Option<String>,
    /// 这一块的像素指纹 —— 界面填完名字要拿它去存
    pub fingerprint: Option<String>,
    /// 指纹有了但库里没有名字：界面需要问一次
    pub unknown: bool,
}

/// 认地图的东西。
pub struct MapReader {
    snapshot: MapSnapshot,
    candidate: Option<String>,
    last_look: Option<Instant>,
    last_retry: Option<Instant>,
    /// 分辨率模式记忆：按客户区尺寸 (client_w, client_h) 记录哪种模式走通了。
    /// 伴随连续失败计数：若记住的模式连续失败多次，推翻记忆，换另一种重新尝试。
    pub resolution_modes: HashMap<(i32, i32), ResolutionPreference>,
}

impl MapReader {
    pub fn new() -> Self {
        Self {
            snapshot: MapSnapshot::default(),
            candidate: None,
            last_look: None,
            last_retry: None,
            resolution_modes: HashMap::new(),
        }
    }

    pub fn snapshot(&self) -> MapSnapshot {
        self.snapshot.clone()
    }

    /// 把用户填的名字记到当前指纹上。
    ///
    /// 返回要落库的（指纹, 名字）；指纹还没算出来时返回 `None`。
    pub fn learn(&mut self, name: &str) -> Option<(String, String)> {
        let name = name.trim();
        let fingerprint = self.snapshot.fingerprint.clone()?;
        if name.is_empty() {
            return None;
        }
        self.snapshot.name = Some(name.to_string());
        self.snapshot.unknown = false;
        Some((fingerprint, name.to_string()))
    }

    /// 获取当前客户区尺寸下的候选模式列表 (primary, optional secondary)。
    ///
    /// 1. 若当前尺寸为 1920×1080（基准），则 Scaled 与 Absolute 计算结果完全一致，
    ///    此时只返回 `(RegionMode::Scaled, None)`，坚决避免无意义的二次抓屏与 OCR 推理。
    /// 2. 若不一致，优先使用该分辨率下此前记住的有效模式；若尚未记忆，默认优先尝试 Scaled。
    ///    次选为相反模式，供首选失败时备选回退。
    pub fn candidate_modes(&self, client_w: i32, client_h: i32) -> (RegionMode, Option<RegionMode>) {
        let scaled = scale_region(BASE_REGION, client_w, client_h);
        let absolute = absolute_region(BASE_REGION, client_w, client_h);
        if scaled == absolute {
            return (RegionMode::Scaled, None);
        }

        let primary = self
            .resolution_modes
            .get(&(client_w, client_h))
            .map(|pref| pref.mode)
            .unwrap_or(RegionMode::Scaled);
        (primary, Some(primary.other()))
    }

    /// 记录某分辨率下模式执行成功：重置连续失败计数并记住该有效模式。
    pub fn record_success(&mut self, client_w: i32, client_h: i32, mode: RegionMode) {
        let scaled = scale_region(BASE_REGION, client_w, client_h);
        let absolute = absolute_region(BASE_REGION, client_w, client_h);
        if scaled == absolute {
            return;
        }
        self.resolution_modes.insert(
            (client_w, client_h),
            ResolutionPreference {
                mode,
                consecutive_failures: 0,
            },
        );
    }

    /// 记录某分辨率下识别失败：累加连续失败计数。
    ///
    /// # 为什么记忆必须能被推翻
    ///
    /// 怀旧服环境极其复杂，一次偶然的噪点匹配或临时窗口变动可能把错误模式存入记忆。
    /// 若不设退出机制，错误模式将永久霸占首选，重试机制也将沦为空转（再次陷入类似
    /// "首帧失败就永久 unknown" 的死胡同）。连续失败达到阈值（3次）后，推翻当前记忆，
    /// 切换为另一模式作为首选重新探测。
    pub fn record_failure(&mut self, client_w: i32, client_h: i32) {
        let scaled = scale_region(BASE_REGION, client_w, client_h);
        let absolute = absolute_region(BASE_REGION, client_w, client_h);
        if scaled == absolute {
            return;
        }
        if let Some(pref) = self.resolution_modes.get_mut(&(client_w, client_h)) {
            pref.consecutive_failures += 1;
            if pref.consecutive_failures >= CONSECUTIVE_FAILURES_THRESHOLD {
                log::warn!(
                    "地图识别：客户区 {}×{} 下模式 {:?} 连续 {} 次失败，推翻记忆并换用 {:?}",
                    client_w,
                    client_h,
                    pref.mode,
                    pref.consecutive_failures,
                    pref.mode.other()
                );
                pref.mode = pref.mode.other();
                pref.consecutive_failures = 0;
            }
        }
    }

    /// 纯函数级状态机决策：给定分辨率与当前模式偏好，按「先试后改」策略探测有效模式。
    ///
    /// `recognize_fn` 接收 `RegionMode`，返回 `Option<String>`（识别出的地图名）。
    /// 返回 `(生效的模式, 识别出的地图名)`。
    pub fn try_recognize_with<F>(
        &mut self,
        client_w: i32,
        client_h: i32,
        mut recognize_fn: F,
    ) -> (RegionMode, Option<String>)
    where
        F: FnMut(RegionMode) -> Option<String>,
    {
        let (primary, secondary) = self.candidate_modes(client_w, client_h);

        // 1. 先试当前偏好模式
        if let Some(name) = recognize_fn(primary) {
            self.record_success(client_w, client_h, primary);
            return (primary, Some(name));
        }

        // 2. 首选失败：若存在不同坐标的备选模式（非 1080p），尝试备选模式
        if let Some(alt) = secondary {
            if let Some(name) = recognize_fn(alt) {
                // 备选模式成功：更新偏好记忆，后续该分辨率直接以它为首选
                self.record_success(client_w, client_h, alt);
                return (alt, Some(name));
            }
        }

        // 3. 所有可用模式均未认出：累积首选模式的失败计数（达到阈值推翻记忆）
        self.record_failure(client_w, client_h);
        (primary, None)
    }

    /// 推进指纹状态机（方便单元测试去抖与单模式下的补偿重试逻辑）。
    #[cfg(test)]
    fn update_fingerprint(
        &mut self,
        fingerprint: String,
        lookup: impl Fn(&str) -> Option<String>,
        mut recognize: impl FnMut() -> Option<String>,
    ) -> MapSnapshot {
        if Some(&fingerprint) == self.snapshot.fingerprint.as_ref() {
            self.candidate = None;

            let should_retry = self.snapshot.unknown
                && self.snapshot.name.is_none()
                && self.last_retry.map(|t| t.elapsed() >= RETRY_INTERVAL).unwrap_or(true);

            if !should_retry {
                return self.snapshot.clone();
            }

            self.last_retry = Some(Instant::now());
            let name = match lookup(&fingerprint) {
                Some(name) => Some(name),
                None => recognize(),
            };
            if let Some(name) = &name {
                self.snapshot = MapSnapshot {
                    unknown: false,
                    name: Some(name.clone()),
                    fingerprint: Some(fingerprint),
                };
            }
            return self.snapshot.clone();
        }

        if self.candidate.as_deref() != Some(fingerprint.as_str()) {
            self.candidate = Some(fingerprint);
            return self.snapshot.clone();
        }
        self.candidate = None;
        self.last_retry = Some(Instant::now());

        let name = match lookup(&fingerprint) {
            Some(name) => Some(name),
            None => recognize(),
        };
        self.snapshot = MapSnapshot {
            unknown: name.is_none(),
            name,
            fingerprint: Some(fingerprint),
        };
        self.snapshot.clone()
    }

    /// 对当前窗口尝试辨认地图：优先尝试首选模式，失败则回退至备选模式，并返回最终生效的指纹。
    fn resolve_map_name(
        &mut self,
        hwnd: isize,
        client_w: i32,
        client_h: i32,
        primary_mode: RegionMode,
        primary_fp: String,
        lookup: &impl Fn(&str) -> Option<String>,
    ) -> (RegionMode, Option<String>, String) {
        let mut final_fp = primary_fp.clone();

        let (mode, name) = self.try_recognize_with(client_w, client_h, |probe_mode| {
            if probe_mode == primary_mode {
                // 首选模式：已算好主指纹，先查库，查不到再跑 OCR
                let line = region_for(probe_mode, BASE_MAP_LINE_REGION, client_w, client_h);
                settle_name(lookup(&primary_fp), || {
                    read_map_name_at(hwnd, line.0, line.1, line.2, line.3)
                })
            } else {
                // 备选模式：必须按备选模式重新截取完整区域算指纹！
                // 确保指纹真正落在有效文字上，而非背景噪点。
                let full = region_for(probe_mode, BASE_REGION, client_w, client_h);
                let (alt_frame, _) = capture::grab_client(hwnd, full.0, full.1, full.2, full.3)?;
                let alt_fp = format!("{:016x}", fingerprint(&alt_frame.pixels()));

                let line = region_for(probe_mode, BASE_MAP_LINE_REGION, client_w, client_h);
                let ocr_res = settle_name(lookup(&alt_fp), || {
                    read_map_name_at(hwnd, line.0, line.1, line.2, line.3)
                });
                if ocr_res.is_some() {
                    final_fp = alt_fp;
                }
                ocr_res
            }
        });

        (mode, name, final_fp)
    }

    /// 隔一会儿看一眼小地图，指纹变了就查一次库。
    ///
    /// `override_rect` 是**手动校准过的地图名区域**（客户区物理像素）。有它时
    /// 指纹和 OCR 都用这一块，不再猜 Scaled / Absolute 模式 —— 这就是
    /// 「地图名也能手动校准」的落点。
    ///
    /// `lookup` 是「按指纹查名字」，由调用方提供（读库 / 读缓存都行）。
    pub fn refresh(
        &mut self,
        hwnd: isize,
        override_rect: Option<(i32, i32, i32, i32)>,
        lookup: impl Fn(&str) -> Option<String>,
    ) -> MapSnapshot {
        if let Some(last) = self.last_look {
            if last.elapsed() < LOOK_INTERVAL {
                return self.snapshot.clone();
            }
        }
        self.last_look = Some(Instant::now());

        if let Some((x, y, width, height)) = override_rect {
            return self.refresh_override(hwnd, x, y, width, height, &lookup);
        }

        let (client_w, client_h) = capture::client_size(hwnd).unwrap_or((1920, 1080));
        let (primary_mode, _) = self.candidate_modes(client_w, client_h);

        // 1. 先用当前首选模式抓取底图与指纹
        let (x, y, width, height) = region_for(primary_mode, BASE_REGION, client_w, client_h);
        let Some((frame, _)) = capture::grab_client(hwnd, x, y, width, height) else {
            return self.snapshot.clone();
        };
        let pixels = frame.pixels();
        if pixels.looks_blank() {
            // 不在角色里 / 小地图收起来了：留着上一次的结果，别一闪一闪的
            return self.snapshot.clone();
        }
        let fingerprint = format!("{:016x}", fingerprint(&pixels));

        // 2. 检查是否仍在老地图
        if Some(&fingerprint) == self.snapshot.fingerprint.as_ref() {
            self.candidate = None;

            // 若当前地图已有名，直接快速返回
            if self.snapshot.name.is_some() {
                return self.snapshot.clone();
            }

            // 若处于未识别状态（unknown 且 name 为 None），受冷却控制触发多模式重试
            let should_retry = self.last_retry.map(|t| t.elapsed() >= RETRY_INTERVAL).unwrap_or(true);
            if !should_retry {
                return self.snapshot.clone();
            }

            self.last_retry = Some(Instant::now());
            let (effective_mode, recognized_name, effective_fp) = self.resolve_map_name(
                hwnd,
                client_w,
                client_h,
                primary_mode,
                fingerprint.clone(),
                &lookup,
            );

            if let Some(name) = recognized_name {
                log::info!(
                    "地图补偿识别成功：客户区 {}×{} 采用 [{}] 区域识别为「{}」（指纹 {}）",
                    client_w,
                    client_h,
                    effective_mode.label(),
                    name,
                    effective_fp
                );
                self.snapshot = MapSnapshot {
                    unknown: false,
                    name: Some(name),
                    fingerprint: Some(effective_fp),
                };
            }
            return self.snapshot.clone();
        }

        // 3. 新指纹去抖（连续两次一致才采纳，避免切场景瞬间噪点导致地图名闪烁）
        if self.candidate.as_deref() != Some(fingerprint.as_str()) {
            self.candidate = Some(fingerprint);
            return self.snapshot.clone();
        }
        self.candidate = None;
        self.last_retry = Some(Instant::now());

        // 4. 连续两次一致：正式进入新地图识别阶段，跑双候选探测与回退
        //
        // # 为什么指纹必须绑定到最终生效的模式上
        //
        // 若在 4K 固定点阵客户端下误用 Scaled 区域抓取，采样框会大幅偏出小地图，
        // 落在空白游戏背景上。二值化后整块退化为全黑，导致不同地图全部算出同一个假指纹。
        // `exp_maps` 一旦把第一张图的名字绑定上去，后续所有地图都会被错误套用成同一张图！
        // 宁可报未知地图让用户手动填，也绝不能让背景假指纹污染全局地图账本。
        // 因此，必须在备选模式识别成功时，将有效指纹更新为备选区域算出的真指纹。
        let (effective_mode, recognized_name, effective_fp) = self.resolve_map_name(
            hwnd,
            client_w,
            client_h,
            primary_mode,
            fingerprint,
            &lookup,
        );

        if let Some(name) = &recognized_name {
            log::info!(
                "地图识别生效：客户区 {}×{} 采用 [{}] 区域识别为「{}」（指纹 {}）",
                client_w,
                client_h,
                effective_mode.label(),
                name,
                effective_fp
            );
        }

        self.snapshot = MapSnapshot {
            unknown: recognized_name.is_none(),
            name: recognized_name,
            fingerprint: Some(effective_fp),
        };
        self.snapshot.clone()
    }

    /// 手动校准过地图名区域时的识别：指纹和 OCR 都用这一块。
    ///
    /// 去抖规则与自动路径一致（新指纹连续两次一致才采纳），但不需要模式回退：
    /// 框是用户按当前分辨率拖的，比例换算已经跟着客户区走了。
    fn refresh_override(
        &mut self,
        hwnd: isize,
        x: i32,
        y: i32,
        width: i32,
        height: i32,
        lookup: &impl Fn(&str) -> Option<String>,
    ) -> MapSnapshot {
        let Some((frame, _)) = capture::grab_client(hwnd, x, y, width, height) else {
            return self.snapshot.clone();
        };
        let pixels = frame.pixels();
        if pixels.looks_blank() {
            return self.snapshot.clone();
        }
        let fingerprint = format!("{:016x}", fingerprint(&pixels));

        // 还在同一张图：没名字就按冷却重试一次 OCR
        if Some(&fingerprint) == self.snapshot.fingerprint.as_ref() {
            self.candidate = None;
            if self.snapshot.name.is_some() {
                return self.snapshot.clone();
            }
            let should_retry = self
                .last_retry
                .map(|t| t.elapsed() >= RETRY_INTERVAL)
                .unwrap_or(true);
            if !should_retry {
                return self.snapshot.clone();
            }
            self.last_retry = Some(Instant::now());
            if let Some(name) = settle_name(lookup(&fingerprint), || {
                read_map_name_at(hwnd, x, y, width, height)
            }) {
                self.snapshot = MapSnapshot {
                    unknown: false,
                    name: Some(name),
                    fingerprint: Some(fingerprint),
                };
            }
            return self.snapshot.clone();
        }

        // 新指纹：连续两次一致才采纳（和自动路径同一条去抖规则）
        if self.candidate.as_deref() != Some(fingerprint.as_str()) {
            self.candidate = Some(fingerprint);
            return self.snapshot.clone();
        }
        self.candidate = None;
        self.last_retry = Some(Instant::now());
        let name = settle_name(lookup(&fingerprint), || {
            read_map_name_at(hwnd, x, y, width, height)
        });
        self.snapshot = MapSnapshot {
            unknown: name.is_none(),
            name,
            fingerprint: Some(fingerprint),
        };
        self.snapshot.clone()
    }
}

impl Default for MapReader {
    fn default() -> Self {
        Self::new()
    }
}

/// 地图像素指纹的高阈值二值化界限。
///
/// 为什么选 200：
/// 小地图的地图名字样是「纯白填充 + 黑描边 + 渐变」（文字中心白字亮度在 220~255）。
/// 浅蓝/半透明底色通常在 40~130；而冰峰雪岭的落雪、勇士部落的沙尘、神社落樱等全屏天气粒子
/// 在该区域的半透明合成灰度大多在 100~175。
/// 取 200 作为硬门槛：将文字中心最稳定的纯白前景二值化为 1，其余背景及半透明天气粒子全部压成 0。
/// 这样既保留了不同地名独一无二的字形指纹，又彻底免疫了动态粒子每秒造成的像素微动。
const FINGERPRINT_INK_THRESHOLD: u8 = 200;

/// 像素指纹：先经高阈值二值化剔除天气粒子与半透明背景，再用 FNV-1a 计算前景字形哈希。
fn fingerprint(pixels: &Pixels<'_>) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for y in 0..pixels.height {
        for x in 0..pixels.width {
            let ink = if pixels.luma(x, y) >= FINGERPRINT_INK_THRESHOLD {
                1u64
            } else {
                0u64
            };
            hash ^= ink;
            hash = hash.wrapping_mul(0x1000_0000_01b3);
        }
    }
    hash
}

/// 现场认一次地图名：截小地图里的**下一行**，跑 PP-OCR 识别模型。
///
/// 为什么不认上一行：上一行是**地名**（「金银岛」这种大区名），
/// 玩家关心、历史里要记的是**地图名**（「坠落主义」「废弃都市」）。
///
/// 认不出来就老实返回 `None`：宁可没有地图名，也不要一个认错的 ——
/// 一张写着别的地图的卡片发出去，比没写地图更没有用。
#[allow(dead_code)]
fn read_map_name(hwnd: isize) -> Option<String> {
    let (client_w, client_h) = capture::client_size(hwnd).unwrap_or((1920, 1080));
    let (x, y, width, height) = scale_region(BASE_MAP_LINE_REGION, client_w, client_h);
    read_map_name_at(hwnd, x, y, width, height)
}

fn read_map_name_at(hwnd: isize, x: i32, y: i32, width: i32, height: i32) -> Option<String> {
    let (frame, _) = capture::grab_client(hwnd, x, y, width, height)?;
    let pixels = frame.pixels();
    if pixels.looks_blank() {
        return None;
    }
    let reading = read_map_name_pixels(&pixels)?;
    if !ocr::looks_like_map_name(&reading) {
        log::info!(
            "地图名识别结果不可信，丢掉：{:?}（置信度 {:.2}）",
            reading.text,
            reading.confidence
        );
        return None;
    }
    Some(reading.text)
}

/// 库里记的名字和现场读出来的名字，用哪一个。
///
/// 平时库里有就用库里的（那是用户确认过的）。唯一的例外是**修补旧版本读丢的数字**：
/// 以前长地名会被压扁，数字读丢（见 [`read_map_name_pixels`]），而用户点「计入历史」时
/// 那个缺了数字的名字就跟着指纹存进了库 —— 不处理的话，识别修好了，这张图却永远显示旧名字。
///
/// 所以：库里的名字**不带数字**时再读一遍，现场读出来的只是「多了数字」才换成新的。
/// 别的差别一律以库里为准（用户自己起的叫法、OCR 认错的字，都不该被现场结果盖掉）。
fn settle_name(cached: Option<String>, read: impl FnOnce() -> Option<String>) -> Option<String> {
    let Some(cached) = cached else {
        return read();
    };
    if cached.chars().any(|ch| ch.is_ascii_digit()) {
        return Some(cached);
    }
    match read() {
        Some(fresh) if restores_digits(&cached, &fresh) => {
            log::info!("地图名补回数字：库里记的「{cached}」→ 现场读到「{fresh}」");
            Some(fresh)
        }
        _ => Some(cached),
    }
}

/// `fresh` 是不是「`cached` 加上几个数字」：只比字和数字，标点不算（旧版本还会把 `<` 读成 `-`）。
fn restores_digits(cached: &str, fresh: &str) -> bool {
    let letters = |text: &str| -> String { text.chars().filter(|ch| ch.is_alphanumeric()).collect() };
    let (old, new) = (letters(cached), letters(fresh));
    let without_digits: String = new.chars().filter(|ch| !ch.is_ascii_digit()).collect();
    old != new && !old.is_empty() && old == without_digits
}

/// 字和面板底色至少差这么多亮度才算「有字」（面板底是一片纯色，字是深色带浅色描边）。
const TEXT_CONTRAST: i32 = 40;
/// 裁到有字的那一段时，左右各留几列底色（贴着字裁，模型会把头尾的笔画吃掉）。
const TEXT_MARGIN: usize = 4;

/// 从已经抓好的「地图名那一块」里读名字（还没做「像不像地图名」的判断）。
///
/// # 为什么要先裁、而且不能压扁
///
/// 自动定位出来的框是**整条面板**那么宽（真机 1080p：208×18），字只占左边一截，
/// 右边是一大片底色。模型的输入是等比放大到高 48，再限宽 320 —— 208×18 放大后是 555 宽，
/// 压进 320 等于把字横向挤到 58%。汉字挤扁了还认得出，半个字宽的数字和尖括号就没了：
/// 真机上 `地铁二号线<第3地区>` 读成 `地铁二号线-的地区`，`猴子沼泽地3` 读成 `猴子沼泽地`，
/// 而字少、面板窄的 `第3军营` 是对的 —— 「有时候丢数字」就是这么来的。
///
/// 所以这里先把框裁到有字的那一段，再走**不压扁**的那条识别路径。
pub(crate) fn read_map_name_pixels(pixels: &Pixels<'_>) -> Option<ocr::Reading> {
    let cropped = crop_to_text(pixels);
    let mut reading = match &cropped {
        Some((bgra, width)) => ocr::recognize_wide_line(&Pixels {
            width: *width,
            height: pixels.height,
            bgra,
        })?,
        // 没找到字的边界（底色不纯 / 整块都是字）：原样喂，但同样不压扁
        None => ocr::recognize_wide_line(pixels)?,
    };
    reading.text = tidy_map_name(&reading.text);
    Some(reading)
}

/// 裁到有字的那一段：返回裁好的 BGRA 和它的宽度；找不到字就返回 `None`。
fn crop_to_text(pixels: &Pixels<'_>) -> Option<(Vec<u8>, usize)> {
    if pixels.width == 0 || pixels.height == 0 {
        return None;
    }
    // 底色 = 出现最多的亮度（面板底是纯色，占了这一块的大半）
    let mut histogram = [0u32; 256];
    for y in 0..pixels.height {
        for x in 0..pixels.width {
            histogram[pixels.luma(x, y) as usize] += 1;
        }
    }
    let background = histogram
        .iter()
        .enumerate()
        .max_by_key(|(_, count)| **count)
        .map(|(luma, _)| luma as i32)?;
    let has_text = |x: usize| {
        (0..pixels.height).any(|y| (pixels.luma(x, y) as i32 - background).abs() > TEXT_CONTRAST)
    };
    let first = (0..pixels.width).find(|x| has_text(*x))?;
    let last = (0..pixels.width).rev().find(|x| has_text(*x))?;
    let left = first.saturating_sub(TEXT_MARGIN);
    let right = (last + TEXT_MARGIN + 1).min(pixels.width);
    let width = right - left;
    // 太窄的不是一行字（一个噪点）：让调用方原样喂
    if width < 8 {
        return None;
    }
    let mut bgra = Vec::with_capacity(width * pixels.height * 4);
    for y in 0..pixels.height {
        let row = (y * pixels.width + left) * 4;
        bgra.extend_from_slice(&pixels.bgra[row..row + width * 4]);
    }
    Some((bgra, width))
}

/// 收拾识别出来的地图名：去空白；尖括号要么成对、要么不要。
///
/// 游戏里有 `地铁二号线<第3地区>` 这种带尖括号的名字，而 11 像素高的 `<` 只有三四个像素宽，
/// 模型常常只认出一边（真机：`地铁二号线第3地区>`）。只剩半边的括号比没有括号更像认错了，
/// 所以不成对就整个拿掉 —— 名字里的字和数字一个不少。
fn tidy_map_name(text: &str) -> String {
    let unified: String = text
        .chars()
        .filter(|ch| !ch.is_whitespace())
        .map(|ch| match ch {
            '＜' | '〈' | '《' => '<',
            '＞' | '〉' | '》' => '>',
            other => other,
        })
        .collect();
    let opens = unified.matches('<').count();
    let closes = unified.matches('>').count();
    if opens == closes {
        unified
    } else {
        unified.chars().filter(|ch| !matches!(ch, '<' | '>')).collect()
    }
}

/// 识别上一行（地名）。当前不用它，留着给诊断和将来的筛选。
#[allow(dead_code)]
fn read_area_name(hwnd: isize) -> Option<String> {
    let (client_w, client_h) = capture::client_size(hwnd).unwrap_or((1920, 1080));
    let (x, y, width, height) = scale_region(BASE_LINE_REGION, client_w, client_h);
    let (frame, _) = capture::grab_client(hwnd, x, y, width, height)?;
    ocr::recognize_line(&frame.pixels()).map(|reading| reading.text)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame_of(text: &[u8]) -> Vec<u8> {
        // 造一块 w×h 的 BGRA：给定灰度图
        let mut bgra = Vec::with_capacity(text.len() * 4);
        for value in text {
            bgra.extend_from_slice(&[*value, *value, *value, 255]);
        }
        bgra
    }

    /// 真机素材：1080p 下自动定位出来的地图名那一块（208×18），上面写着 `地铁二号线<第3地区>`。
    fn subway_strip() -> Vec<u8> {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/map_name_subway_208x18.bgra");
        std::fs::read(&path).expect("读不到地图名素材")
    }

    /// 裁到有字的那一段：真机这一块字只占左边 120 来列，右边 80 多列都是底色。
    #[test]
    fn the_strip_is_cropped_to_its_text() {
        let bgra = subway_strip();
        let pixels = Pixels { width: 208, height: 18, bgra: &bgra };
        let (cropped, width) = crop_to_text(&pixels).expect("应该找得到字");
        assert!((118..=132).contains(&width), "裁出来 {width} 列");
        assert_eq!(cropped.len(), width * 18 * 4);

        // 一片纯色里没有字
        let flat = vec![178u8; 40 * 18 * 4];
        assert!(crop_to_text(&Pixels { width: 40, height: 18, bgra: &flat }).is_none());
    }

    /// **用户报的问题**：`地铁二号线<第3地区>` 以前读成 `地铁二号线-第地区`，数字丢了。
    ///
    /// 这一条会真的跑一次识别模型（几秒）。旧读法（整条面板压进 320 宽）在同一份素材上
    /// 读不出 `3`，所以它同时钉住了「为什么要先裁、不压扁」。
    #[test]
    fn digits_in_a_long_map_name_survive() {
        let bgra = subway_strip();
        let pixels = Pixels { width: 208, height: 18, bgra: &bgra };
        let reading = read_map_name_pixels(&pixels).expect("应该读得出来");
        assert_eq!(reading.text, "地铁二号线第3地区");
        assert!(ocr::looks_like_map_name(&reading), "{reading:?}");

        let squeezed = ocr::recognize_line(&pixels).expect("旧读法也有结果");
        assert!(!squeezed.text.contains('3'), "旧读法居然读出了数字：{squeezed:?}");
    }

    /// 尖括号要么成对，要么不要。
    #[test]
    fn angle_brackets_are_kept_only_in_pairs() {
        assert_eq!(tidy_map_name("地铁二号线<第3地区>"), "地铁二号线<第3地区>");
        assert_eq!(tidy_map_name("地铁二号线第3地区>"), "地铁二号线第3地区");
        assert_eq!(tidy_map_name("地铁二号线＜第3地区＞"), "地铁二号线<第3地区>");
        assert_eq!(tidy_map_name(" 猴子沼泽地 3 "), "猴子沼泽地3");
    }

    /// 库里记着旧版本读丢数字的名字时，用现场读到的补回来；别的情况以库里为准。
    #[test]
    fn a_cached_name_only_yields_to_restored_digits() {
        let fresh = |name: &str| {
            let name = name.to_string();
            move || Some(name)
        };
        // 库里没有：用现场读的
        assert_eq!(settle_name(None, fresh("蚂蚁洞")).as_deref(), Some("蚂蚁洞"));
        // 旧版本读丢了数字（连带把 `<` 读成了 `-`）
        assert_eq!(
            settle_name(Some("猴子沼泽地".into()), fresh("猴子沼泽地3")).as_deref(),
            Some("猴子沼泽地3")
        );
        assert_eq!(
            settle_name(Some("地铁二号线-第地区".into()), fresh("地铁二号线第3地区")).as_deref(),
            Some("地铁二号线第3地区")
        );
        // 用户自己起的叫法、现场认错一个字：都不换
        assert_eq!(
            settle_name(Some("沼泽".into()), fresh("猴子沼泽地3")).as_deref(),
            Some("沼泽")
        );
        assert_eq!(
            settle_name(Some("魔法密林".into()), fresh("随法密林")).as_deref(),
            Some("魔法密林")
        );
        // 现场没读出来：留着库里的
        assert_eq!(settle_name(Some("金银岛".into()), || None).as_deref(), Some("金银岛"));
        // 库里的名字已经带数字：不再读第二遍
        let mut asked = false;
        let kept = settle_name(Some("第3军营".into()), || {
            asked = true;
            Some("第8军营".to_string())
        });
        assert_eq!(kept.as_deref(), Some("第3军营"));
        assert!(!asked, "带数字的名字不该再跑一次识别");
    }

    /// 同一块像素的指纹要稳定，不同的要不一样。
    #[test]
    fn the_fingerprint_is_stable_and_discriminating() {
        let bright = vec![200u8; 8 * 8 * 4];
        let dark = vec![100u8; 8 * 8 * 4];
        let a = Pixels { width: 8, height: 8, bgra: &bright };
        let b = Pixels { width: 8, height: 8, bgra: &bright };
        let c = Pixels { width: 8, height: 8, bgra: &dark };
        assert_eq!(fingerprint(&a), fingerprint(&b));
        assert_ne!(fingerprint(&a), fingerprint(&c));
    }

    /// 用户填过一次之后，同一张地图就自动认得出（不用再填）。
    #[test]
    fn a_named_map_is_recognised_without_asking_again() {
        let mut reader = MapReader::new();
        // 直接手工摆一个指纹，跳过抓屏
        reader.snapshot = MapSnapshot {
            name: Some("坠落主义".to_string()),
            fingerprint: Some("abc".to_string()),
            unknown: false,
        };
        // 抓不到窗口（hwnd=0）时保持原样，关键是**不能变成「要问用户」**
        let snapshot = reader.refresh(0, None, |_| Some("坠落主义".to_string()));
        assert!(!snapshot.unknown);
        assert_eq!(snapshot.name.as_deref(), Some("坠落主义"));
    }

    /// 新指纹要**连续两次一致**才采纳 —— 一帧渲染抖动不该把地图名抖没。
    #[test]
    fn a_new_fingerprint_must_be_seen_twice() {
        let mut reader = MapReader::new();
        reader.snapshot = MapSnapshot {
            name: Some("蚂蚁洞".to_string()),
            fingerprint: Some("old".to_string()),
            unknown: false,
        };
        // 模拟 refresh 里那段判定（抓屏部分跳过，逻辑单独验）
        let decide = |reader: &mut MapReader, fingerprint: &str| {
            if Some(fingerprint) == reader.snapshot.fingerprint.as_deref() {
                reader.candidate = None;
                return false;
            }
            if reader.candidate.as_deref() != Some(fingerprint) {
                reader.candidate = Some(fingerprint.to_string());
                return false;
            }
            reader.candidate = None;
            reader.snapshot = MapSnapshot {
                name: None,
                fingerprint: Some(fingerprint.to_string()),
                unknown: true,
            };
            true
        };
        assert!(!decide(&mut reader, "new"), "第一次见到只记下候选");
        assert_eq!(reader.snapshot.name.as_deref(), Some("蚂蚁洞"), "还不能换掉");
        assert!(decide(&mut reader, "new"), "第二次一致才采纳");
        assert!(reader.snapshot.unknown);

        // 抖动：新指纹出现一次又回到老的 → 候选被清掉，老地图留着
        reader.snapshot = MapSnapshot {
            name: Some("坠落主义".to_string()),
            fingerprint: Some("stable".to_string()),
            unknown: false,
        };
        assert!(!decide(&mut reader, "jitter"));
        assert!(!decide(&mut reader, "stable"), "回到老指纹不换地图");
        assert!(reader.candidate.is_none(), "候选要被清掉");
        assert_eq!(reader.snapshot.name.as_deref(), Some("坠落主义"));
    }

    /// `learn` 要把名字挂到当前指纹上，并立刻生效。
    #[test]
    fn learn_attaches_the_name_to_the_current_fingerprint() {
        let mut reader = MapReader::new();
        assert_eq!(reader.learn("坠落主义"), None, "还没有指纹时学不了");

        reader.snapshot = MapSnapshot {
            name: None,
            fingerprint: Some("f00d".to_string()),
            unknown: true,
        };
        assert_eq!(
            reader.learn(" 坠落主义 "),
            Some(("f00d".to_string(), "坠落主义".to_string()))
        );
        let snapshot = reader.snapshot();
        assert_eq!(snapshot.name.as_deref(), Some("坠落主义"));
        assert!(!snapshot.unknown, "填过之后就不该再问");
        assert_eq!(reader.learn("   "), None, "空白名字不算");
    }

    /// 空白画面 / 抓不到窗口时保持上一次的结果，不能把地图名抖没。
    #[test]
    fn a_blank_look_keeps_the_previous_map() {
        let mut reader = MapReader::new();
        reader.snapshot = MapSnapshot {
            name: Some("蚂蚁洞".to_string()),
            fingerprint: Some("beef".to_string()),
            unknown: false,
        };
        let snapshot = reader.refresh(0, None, |_| None);
        assert_eq!(snapshot.name.as_deref(), Some("蚂蚁洞"));
        assert_eq!(snapshot.fingerprint.as_deref(), Some("beef"));
        // 指纹函数本身对一块普通像素也要给出确定的答案
        let bgra = frame_of(&[180u8; 64]);
        let pixels = Pixels { width: 8, height: 8, bgra: &bgra };
        assert_eq!(fingerprint(&pixels), fingerprint(&pixels));
    }

    /// 验证小地图区域随客户区尺寸等比缩放且边界夹紧（P1-1 回归测试）。
    #[test]
    fn scale_region_adapts_to_resolutions() {
        // 1. 1080p 基准下应保持原样
        assert_eq!(
            scale_region(BASE_REGION, 1920, 1080),
            (48, 26, 68, 34)
        );
        assert_eq!(
            scale_region(BASE_MAP_LINE_REGION, 1920, 1080),
            (50, 43, 68, 16)
        );

        // 2. 720p（1280×720，2/3 比例）
        let scaled_720 = scale_region(BASE_REGION, 1280, 720);
        assert_eq!(scaled_720, (32, 17, 45, 23));

        // 3. 800×600（4:3 窗口）
        let scaled_800 = scale_region(BASE_REGION, 800, 600);
        assert!(scaled_800.0 >= 0 && scaled_800.0 + scaled_800.2 <= 800);
        assert!(scaled_800.1 >= 0 && scaled_800.1 + scaled_800.3 <= 600);

        // 4. 边界夹紧（极小尺寸不越界且宽高不为 0）
        let tiny = scale_region(BASE_REGION, 10, 10);
        assert!(tiny.0 + tiny.2 <= 10);
        assert!(tiny.1 + tiny.3 <= 10);
        assert!(tiny.2 >= 1 && tiny.3 >= 1);
    }

    /// 未识别地图在冷却后允许重试，而不是永久锁定在 unknown（P1-2 回归测试）。
    #[test]
    fn unrecognized_map_retries_after_cooldown() {
        let mut reader = MapReader::new();
        let fp = "ant_cave".to_string();

        // 第一次见到：仅记入候选
        let s1 = reader.update_fingerprint(fp.clone(), |_| None, || None);
        assert!(s1.fingerprint.is_none());

        // 第二次见到（抖动通过）：OCR 失败（返回 None），标记为 unknown
        let s2 = reader.update_fingerprint(fp.clone(), |_| None, || None);
        assert_eq!(s2.fingerprint.as_deref(), Some("ant_cave"));
        assert!(s2.unknown);
        assert_eq!(s2.name, None);

        // 立即再次轮询（冷却未到）：OCR 不应被调用，保持 unknown
        let mut ocr_called = false;
        let s3 = reader.update_fingerprint(fp.clone(), |_| None, || {
            ocr_called = true;
            None
        });
        assert!(!ocr_called, "冷却期内不应重复调用 OCR 推理");
        assert!(s3.unknown);

        // 模拟时间流逝（冷却已过）：再次轮询应触发重试，识别成功后解除 unknown
        reader.last_retry = Some(Instant::now() - Duration::from_secs(10));
        let s4 = reader.update_fingerprint(fp.clone(), |_| None, || {
            ocr_called = true;
            Some("蚂蚁洞".to_string())
        });
        assert!(ocr_called, "冷却过后必须允许补偿重试");
        assert!(!s4.unknown, "重试成功后应解除 unknown");
        assert_eq!(s4.name.as_deref(), Some("蚂蚁洞"));

        // 成功识别后再次轮询：已有名，不再重复 OCR
        ocr_called = false;
        let s5 = reader.update_fingerprint(fp, |_| None, || {
            ocr_called = true;
            Some("其它".to_string())
        });
        assert!(!ocr_called, "已识别地图不应再调用 OCR");
        assert_eq!(s5.name.as_deref(), Some("蚂蚁洞"));
    }

    /// 验证二值化指纹对动态天气粒子与半透明底色噪声的免疫力（P2 回归测试）。
    #[test]
    fn binarized_fingerprint_is_immune_to_weather_noise() {
        // 构造基础地图名画面（白字前景 240，背景 50）
        let mut base_bgra = vec![50u8; 16 * 16 * 4];
        for i in 4..12 {
            let offset_h = (8 * 16 + i) * 4;
            let offset_v = (i * 16 + 8) * 4;
            for c in 0..3 {
                base_bgra[offset_h + c] = 240;
                base_bgra[offset_v + c] = 240;
            }
        }
        let base_pixels = Pixels { width: 16, height: 16, bgra: &base_bgra };
        let base_fp = fingerprint(&base_pixels);

        // 场景 A：加入落雪粒子（背景像素微提亮至 130~160，低于 200 门槛）
        let mut noisy_a_bgra = base_bgra.clone();
        for offset in [2 * 16 + 3, 14 * 16 + 13] {
            noisy_a_bgra[offset * 4] = 140;
            noisy_a_bgra[offset * 4 + 1] = 140;
            noisy_a_bgra[offset * 4 + 2] = 140;
        }
        let pixels_a = Pixels { width: 16, height: 16, bgra: &noisy_a_bgra };

        // 场景 B：加入不同位置的沙尘/落樱粒子（背景像素提亮至 110~175）
        let mut noisy_b_bgra = base_bgra.clone();
        for offset in [1 * 16 + 14, 13 * 16 + 2] {
            noisy_b_bgra[offset * 4] = 175;
            noisy_b_bgra[offset * 4 + 1] = 175;
            noisy_b_bgra[offset * 4 + 2] = 175;
        }
        let pixels_b = Pixels { width: 16, height: 16, bgra: &noisy_b_bgra };

        // 断言：由于背景粒子低于 200 阈值，二值化后指纹完全相同
        assert_eq!(
            fingerprint(&pixels_a),
            base_fp,
            "带落雪粒子的指纹必须与无噪声底图指纹一致"
        );
        assert_eq!(
            fingerprint(&pixels_b),
            base_fp,
            "不同天气粒子的指纹必须一致"
        );

        // 对照：如果按旧的 raw luma 哈希，两者必然产生不同指纹
        let raw_hash = |p: &Pixels<'_>| {
            let mut h: u64 = 0xcbf2_9ce4_8422_2325;
            for y in 0..p.height {
                for x in 0..p.width {
                    h ^= p.luma(x, y) as u64;
                    h = h.wrapping_mul(0x1000_0000_01b3);
                }
            }
            h
        };
        assert_ne!(
            raw_hash(&pixels_a),
            raw_hash(&pixels_b),
            "旧的全像素哈希在此场景下必定抖动不一致"
        );
    }

    /// 验证客户区为 1920×1080 时，Scaled 与 Absolute 相同且只跑一次 probe。
    #[test]
    fn candidates_are_identical_at_1080p() {
        assert_eq!(
            scale_region(BASE_REGION, 1920, 1080),
            absolute_region(BASE_REGION, 1920, 1080)
        );
        assert_eq!(
            scale_region(BASE_MAP_LINE_REGION, 1920, 1080),
            absolute_region(BASE_MAP_LINE_REGION, 1920, 1080)
        );

        let mut reader = MapReader::new();
        let (primary, secondary) = reader.candidate_modes(1920, 1080);
        assert_eq!(primary, RegionMode::Scaled);
        assert_eq!(secondary, None, "1080p 下两种模式相同，备选模式必须为 None，坚决不跑第二次");

        let mut call_count = 0;
        let (mode, name) = reader.try_recognize_with(1920, 1080, |_mode| {
            call_count += 1;
            None // 模拟失败
        });
        assert_eq!(call_count, 1, "1080p 下即使识别失败，也只尝试 1 次，不进行无意义重试");
        assert_eq!(mode, RegionMode::Scaled);
        assert_eq!(name, None);
    }

    /// 验证非 1080p 分辨率下，Scaled 与 Absolute 产生不同区域（如 4K 下 Absolute 仍保持原样）。
    #[test]
    fn candidates_differ_at_non_1080p() {
        // 4K (3840×2160)
        let scaled_4k = scale_region(BASE_REGION, 3840, 2160);
        let absolute_4k = absolute_region(BASE_REGION, 3840, 2160);
        assert_ne!(scaled_4k, absolute_4k);
        assert_eq!(scaled_4k, (96, 52, 136, 68));
        assert_eq!(absolute_4k, (48, 26, 68, 34), "4K 下 Absolute 模式必须保持基准原始像素坐标");

        // 1366×768 (常见笔记本全屏/半屏)
        let scaled_768 = scale_region(BASE_REGION, 1366, 768);
        let absolute_768 = absolute_region(BASE_REGION, 1366, 768);
        assert_ne!(scaled_768, absolute_768);
        assert_eq!(absolute_768, (48, 26, 68, 34));

        let reader = MapReader::new();
        let (p4k, s4k) = reader.candidate_modes(3840, 2160);
        assert_eq!(p4k, RegionMode::Scaled);
        assert_eq!(s4k, Some(RegionMode::Absolute));
    }

    /// 验证状态机「首选失败 → 换另一种成功 → 记忆更新为新模式」的先试后改流程。
    #[test]
    fn state_machine_switches_and_remembers_working_mode() {
        let mut reader = MapReader::new();
        let (w, h) = (3840, 2160);

        // 初始未记忆时，首选是 Scaled，备选是 Absolute
        let (p_init, s_init) = reader.candidate_modes(w, h);
        assert_eq!(p_init, RegionMode::Scaled);
        assert_eq!(s_init, Some(RegionMode::Absolute));

        // 模拟：在 4K 下 Scaled 抓空（返回 None），但 Absolute 成功读出 "勇士部落"
        let (mode, name) = reader.try_recognize_with(w, h, |m| match m {
            RegionMode::Scaled => None,
            RegionMode::Absolute => Some("勇士部落".to_string()),
        });

        assert_eq!(mode, RegionMode::Absolute, "应回退并采纳成功的 Absolute 模式");
        assert_eq!(name.as_deref(), Some("勇士部落"));

        // 验证记忆已更新：后续在 4K 下，首选直接变为 Absolute，不再以 Scaled 开局
        let (p_next, s_next) = reader.candidate_modes(w, h);
        assert_eq!(p_next, RegionMode::Absolute, "成功后必须记住该分辨率下的有效模式");
        assert_eq!(s_next, Some(RegionMode::Scaled));

        // 后续直接命中首选 Absolute，只跑 1 次即成功
        let mut count = 0;
        let (mode2, name2) = reader.try_recognize_with(w, h, |m| {
            count += 1;
            if m == RegionMode::Absolute {
                Some("勇士部落".to_string())
            } else {
                None
            }
        });
        assert_eq!(count, 1, "命中记住的模式时，直接成功，不跑备选");
        assert_eq!(mode2, RegionMode::Absolute);
        assert_eq!(name2.as_deref(), Some("勇士部落"));
    }

    /// 验证连续失败达到阈值（3次）后，推翻当前记忆，换回另一模式重新探测。
    #[test]
    fn consecutive_failures_overturn_memory() {
        let mut reader = MapReader::new();
        let (w, h) = (3840, 2160);

        // 初始记住 Absolute 模式
        reader.record_success(w, h, RegionMode::Absolute);
        assert_eq!(reader.candidate_modes(w, h).0, RegionMode::Absolute);

        // 失败第 1 次：未达阈值，仍保持 Absolute
        reader.try_recognize_with(w, h, |_| None);
        assert_eq!(reader.candidate_modes(w, h).0, RegionMode::Absolute);

        // 失败第 2 次：仍未达阈值
        reader.try_recognize_with(w, h, |_| None);
        assert_eq!(reader.candidate_modes(w, h).0, RegionMode::Absolute);

        // 失败第 3 次：达到 CONSECUTIVE_FAILURES_THRESHOLD (3)，推翻记忆！
        reader.try_recognize_with(w, h, |_| None);
        assert_eq!(
            reader.candidate_modes(w, h).0,
            RegionMode::Scaled,
            "连续 3 次失败后必须推翻记忆，切换为 Scaled 重新探测"
        );
    }
}
