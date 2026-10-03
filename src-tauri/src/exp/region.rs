//! 校准区域的**归一化表示**与存取。
//!
//! # 为什么坐标必须是比例，不是像素
//!
//! 经验那一行会跟着游戏分辨率 / 界面缩放走：1080p 下字高 7 像素、1440p 约 9~10、
//! 768p 约 5、4K 约 14（真机实测，见 docs/DEVELOPMENT.md 的「经验统计」一节）。
//! 如果像早期实现那样把「客户区 2560×1440 上的 (10, 20, 300, 40)」存进库，
//! 那么换一个分辨率就必然对不上 —— 要么再框一次（用户骂），要么按比例猜
//! （两套坐标模型迟早漂移）。
//!
//! 网页版枫记（`fj.need.run`）的做法是：**所有 ROI 都存成帧尺寸的比例**
//! （`x/width, y/height`），再在运行时把比例乘回当前帧尺寸。我们照做，
//! 但比它多一层保险：读的时候如果框内读不到，自动退回整幅画面重新定位，
//! 定位结果连续两次一致才覆盖自动档；手动档永不被自动覆盖。
//!
//! # 单位约定（整个模块只有这一套）
//!
//! * [`NormRect`]：x / y / w / h 都是**客户区比例**，范围 0..=1；
//! * [`PixelRect`]：客户区**物理像素**（PrintWindow 抓到的就是物理像素）；
//! * 落库 / 过 IPC 的一律是 `NormRect`，像素矩形只活在内存里。

use serde::{Deserialize, Serialize};

/// 经验行（`EXP 427096(46.09%)` 那一行）。
pub const KIND_EXP_LINE: &str = "exp_line";
/// 小地图上的地图名（预留给同一条「比例框」管线）。
pub const KIND_MAP_NAME: &str = "map_name";
/// 等级数字（HUD 上的 `Lv.55`；经验表定不出等级时的兜底）。
pub const KIND_LEVEL: &str = "level";

/// 支持的校准类型（命令层用同一条判据，避免各处各写一份）。
pub fn is_valid_kind(kind: &str) -> bool {
    matches!(kind, KIND_EXP_LINE | KIND_MAP_NAME | KIND_LEVEL)
}

/// 归一化矩形：相对客户区左上角的比例。
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct NormRect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

/// 客户区物理像素矩形。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PixelRect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

/// 一个归一化矩形要能映射成像素矩形，至少得有这么大（防止 0 宽 / 反框）。
pub const MIN_PIXEL_SIDE: i32 = 8;

impl NormRect {
    /// 造一个矩形；非有限值 / 越界 / 空矩形一律返回 `None`。
    pub fn new(x: f64, y: f64, w: f64, h: f64) -> Option<Self> {
        let rect = Self { x, y, w, h };
        rect.is_valid().then_some(rect)
    }

    pub fn is_valid(&self) -> bool {
        [self.x, self.y, self.w, self.h].iter().all(|v| v.is_finite())
            && self.x >= 0.0
            && self.y >= 0.0
            && self.w > 0.0
            && self.h > 0.0
            && self.x + self.w <= 1.0 + 1e-9
            && self.y + self.h <= 1.0 + 1e-9
    }

    /// 从像素矩形换算（`client_w` / `client_h` 必须为正）。
    pub fn from_pixel(rect: PixelRect, client_w: i32, client_h: i32) -> Option<Self> {
        if client_w <= 0 || client_h <= 0 || rect.w <= 0 || rect.h <= 0 {
            return None;
        }
        Self::new(
            rect.x as f64 / client_w as f64,
            rect.y as f64 / client_h as f64,
            rect.w as f64 / client_w as f64,
            rect.h as f64 / client_h as f64,
        )
    }

    /// 映射回像素矩形（round 到整数像素，先向外取整保证不丢边）。
    ///
    /// 先向外取整再钳客户区、最后保证最小边 —— 与 `PixelRect::clamp_to` 一致。
    /// 远端减一个极小量：`0.6 * 500` 在浮点里是 `300.00000000000006`，
    /// 不减这一下 `ceil` 会凭空多出一行像素。
    pub fn to_pixel(self, client_w: i32, client_h: i32) -> Option<PixelRect> {
        if client_w <= 0 || client_h <= 0 || !self.is_valid() {
            return None;
        }
        let x0 = (self.x * client_w as f64).floor() as i32;
        let y0 = (self.y * client_h as f64).floor() as i32;
        let x1 = ((self.x + self.w) * client_w as f64 - 1e-6).ceil() as i32;
        let y1 = ((self.y + self.h) * client_h as f64 - 1e-6).ceil() as i32;
        Some(
            PixelRect {
                x: x0,
                y: y0,
                w: (x1 - x0).max(1),
                h: (y1 - y0).max(1),
            }
            .clamp_to(client_w, client_h),
        )
    }

    /// 交并比 —— 判断「两次定位是不是同一行」的统一尺子。
    pub fn iou(self, other: Self) -> f64 {
        let x1 = self.x.max(other.x);
        let y1 = self.y.max(other.y);
        let x2 = (self.x + self.w).min(other.x + other.w);
        let y2 = (self.y + self.h).min(other.y + other.h);
        let intersection = (x2 - x1).max(0.0) * (y2 - y1).max(0.0);
        let union = self.w * self.h + other.w * other.h - intersection;
        if union <= 0.0 {
            0.0
        } else {
            intersection / union
        }
    }
}

impl PixelRect {
    /// 钳进客户区；宽高至少 1。
    pub fn clamp_to(self, client_w: i32, client_h: i32) -> Self {
        let x = self.x.clamp(0, client_w.max(1) - 1);
        let y = self.y.clamp(0, client_h.max(1) - 1);
        let w = self.w.clamp(1, client_w - x);
        let h = self.h.clamp(1, client_h - y);
        Self { x, y, w, h }
    }

    /// 四周各向外扩若干像素（钳客户区）。
    pub fn expanded(self, pad: i32, client_w: i32, client_h: i32) -> Self {
        Self {
            x: self.x - pad,
            y: self.y - pad,
            w: self.w + pad * 2,
            h: self.h + pad * 2,
        }
        .clamp_to(client_w, client_h)
    }

    /// 够不够大（太小的框交给裁剪只会出乱子）。
    pub fn is_usable(self) -> bool {
        self.w >= MIN_PIXEL_SIDE && self.h >= MIN_PIXEL_SIDE
    }
}

/// 校准档从哪来。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RegionSource {
    /// 自动定位学到的；读写失败会被重新定位覆盖。
    Auto,
    /// 用户手动框的；**永不被自动覆盖**，只能由用户显式重置。
    Manual,
}

impl RegionSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Manual => "manual",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "auto" => Some(Self::Auto),
            "manual" => Some(Self::Manual),
            _ => None,
        }
    }
}

/// 一条校准记录。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RegionProfile {
    pub kind: String,
    pub rect: NormRect,
    pub source: RegionSource,
    /// 学到的字形高度（像素，客户区物理像素）。只是提示 / 诊断用，
    /// 读数失败重定位时不依赖它。
    pub text_height: Option<u32>,
    /// 最近一次落库时刻（unix 秒）。
    pub learned_unix: i64,
}

/// 校准档的持久化接口。
///
/// 单独抽一个 trait 而不是让 `ExpReader` 直接拿 `Database`：
/// 单测里喂一个内存实现就能验证「自动定位 → 连续两次一致 → 落库 / 不覆盖手动档」
/// 这条状态机，不需要真的 SQLite，也不需要真的游戏窗口。
pub trait RegionStore: Send + Sync {
    fn load_region(&self, kind: &str) -> Option<RegionProfile>;
    fn save_region(&self, profile: &RegionProfile) -> Result<(), String>;
    fn clear_region(&self, kind: &str) -> Result<(), String>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalized_rects_must_be_finite_and_inside_the_frame() {
        assert!(NormRect::new(0.0, 0.0, 1.0, 1.0).is_some());
        assert!(NormRect::new(0.1, 0.2, 0.3, 0.4).is_some());
        assert!(NormRect::new(-0.1, 0.0, 0.5, 0.5).is_none());
        assert!(NormRect::new(0.0, 0.0, 1.1, 0.5).is_none());
        assert!(NormRect::new(0.0, 0.0, 0.0, 0.5).is_none());
        assert!(NormRect::new(f64::NAN, 0.0, 0.5, 0.5).is_none());
        assert!(NormRect::new(0.9, 0.9, 0.2, 0.2).is_none());
    }

    #[test]
    fn pixel_mapping_round_trips_and_always_contains_the_original() {
        let rect = NormRect::new(0.1, 0.2, 0.3, 0.4).unwrap();
        let pixel = rect.to_pixel(1000, 500).unwrap();
        assert_eq!((pixel.x, pixel.y), (100, 100));
        assert_eq!((pixel.w, pixel.h), (300, 200));

        // 向外取整：源矩形里的每个像素都必须落在像素矩形里
        let odd = NormRect::new(0.101, 0.201, 0.099, 0.099).unwrap();
        let pixel = odd.to_pixel(1920, 1080).unwrap();
        assert!(pixel.x as f64 <= 0.101 * 1920.0);
        assert!(pixel.y as f64 <= 0.201 * 1080.0);
        assert!((pixel.x + pixel.w) as f64 >= (0.101 + 0.099) * 1920.0 - 1e-6);
        assert!((pixel.y + pixel.h) as f64 >= (0.201 + 0.099) * 1080.0 - 1e-6);

        // 反过来：像素矩形换算成比例，再映射回来，尺寸不缩水
        let back = NormRect::from_pixel(pixel, 1920, 1080).unwrap();
        let again = back.to_pixel(1920, 1080).unwrap();
        assert!(again.x <= pixel.x && again.y <= pixel.y);
        assert!(again.x + again.w >= pixel.x + pixel.w);
        assert!(again.y + again.h >= pixel.y + pixel.h);
    }

    #[test]
    fn tiny_frames_degenerate_but_never_panic() {
        assert!(NormRect::new(0.5, 0.5, 1.0, 1.0).is_none());
        let rect = NormRect::new(0.0, 0.0, 1.0, 1.0).unwrap();
        assert!(rect.to_pixel(0, 0).is_none());
        assert!(rect.to_pixel(4, 4).unwrap().is_usable() == false);
        assert!(rect.to_pixel(20, 20).unwrap().is_usable());
    }

    #[test]
    fn iou_says_when_two_rects_are_the_same_line() {
        let a = NormRect::new(0.1, 0.1, 0.2, 0.1).unwrap();
        let same = NormRect::new(0.102, 0.101, 0.2, 0.1).unwrap();
        let other = NormRect::new(0.5, 0.5, 0.2, 0.1).unwrap();
        assert!(a.iou(same) > 0.8);
        assert_eq!(a.iou(other), 0.0);
    }

    #[test]
    fn pixel_rect_clamp_never_escapes_the_client() {
        let rect = PixelRect {
            x: -5,
            y: -5,
            w: 5000,
            h: 5000,
        }
        .clamp_to(1920, 1080);
        assert_eq!(rect.x, 0);
        assert_eq!(rect.y, 0);
        assert_eq!(rect.w, 1920);
        assert_eq!(rect.h, 1080);
        let grown = PixelRect {
            x: 100,
            y: 100,
            w: 50,
            h: 50,
        }
        .expanded(20, 1920, 1080);
        assert_eq!((grown.x, grown.y, grown.w, grown.h), (80, 80, 90, 90));
    }

    #[test]
    fn source_strings_round_trip() {
        for source in [RegionSource::Auto, RegionSource::Manual] {
            assert_eq!(RegionSource::parse(source.as_str()), Some(source));
        }
        assert_eq!(RegionSource::parse("learned"), None);
    }
}
