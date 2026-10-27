//! BUFF 识别用到的几样灰度图运算：取亮度、横向 Sobel、缩放、形态学顶帽、连通域。
//!
//! 网页版是靠 OpenCV（wasm）做这些；我们只用到其中很小一块，自己写几十行比
//! 带一个 OpenCV 进单文件 exe 划算得多（发布物一直守着「拷过去就能用」）。

/// 单通道浮点图（行优先）。值域随用途而定：亮度是 0–255，Sobel 可正可负。
#[derive(Debug, Clone, PartialEq)]
pub struct Gray {
    pub width: usize,
    pub height: usize,
    pub data: Vec<f32>,
}

impl Gray {
    pub fn new(width: usize, height: usize) -> Self {
        Self { width, height, data: vec![0.0; width * height] }
    }

    #[inline]
    pub fn at(&self, x: usize, y: usize) -> f32 {
        self.data[y * self.width + x]
    }

    /// BGRA 像素 → 亮度（BT.601，和 OpenCV 的 `COLOR_RGB2GRAY` 一样的权重）。
    pub fn from_bgra(bgra: &[u8], width: usize, height: usize) -> Self {
        let mut out = Self::new(width, height);
        for (i, px) in bgra.chunks_exact(4).take(width * height).enumerate() {
            out.data[i] = 0.114 * px[0] as f32 + 0.587 * px[1] as f32 + 0.299 * px[2] as f32;
        }
        out
    }

    /// 从 RGBA（PNG 解出来的样子）取亮度，同时给出不透明度掩码。
    pub fn from_rgba(rgba: &[u8], width: usize, height: usize) -> (Self, Vec<bool>) {
        let mut out = Self::new(width, height);
        let mut opaque = vec![true; width * height];
        for (i, px) in rgba.chunks_exact(4).take(width * height).enumerate() {
            out.data[i] = 0.299 * px[0] as f32 + 0.587 * px[1] as f32 + 0.114 * px[2] as f32;
            opaque[i] = px[3] >= 128;
        }
        (out, opaque)
    }

    /// 截一块（越界部分按边缘复制补齐，和 OpenCV 的 `BORDER_REPLICATE` 一样）。
    pub fn crop(&self, x: i64, y: i64, width: usize, height: usize) -> Self {
        let mut out = Self::new(width, height);
        for yy in 0..height {
            let sy = (y + yy as i64).clamp(0, self.height as i64 - 1) as usize;
            for xx in 0..width {
                let sx = (x + xx as i64).clamp(0, self.width as i64 - 1) as usize;
                out.data[yy * width + xx] = self.at(sx, sy);
            }
        }
        out
    }

    pub fn max(&self) -> f32 {
        self.data.iter().copied().fold(f32::MIN, f32::max)
    }
}

/// 横向 Sobel（3×3，边缘复制）。只取 x 方向：BUFF 图标的边多是竖笔画和图标框，
/// 网页版也是灰度 + 横向梯度两路一起比，单看灰度容易被底色深浅带偏。
pub fn sobel_x(src: &Gray) -> Gray {
    let (w, h) = (src.width, src.height);
    let mut out = Gray::new(w, h);
    if w == 0 || h == 0 {
        return out;
    }
    let px = |x: isize, y: isize| {
        let x = x.clamp(0, w as isize - 1) as usize;
        let y = y.clamp(0, h as isize - 1) as usize;
        src.data[y * w + x]
    };
    for y in 0..h as isize {
        for x in 0..w as isize {
            let right = px(x + 1, y - 1) + 2.0 * px(x + 1, y) + px(x + 1, y + 1);
            let left = px(x - 1, y - 1) + 2.0 * px(x - 1, y) + px(x - 1, y + 1);
            out.data[y as usize * w + x as usize] = right - left;
        }
    }
    out
}

/// 2×2 平均缩小一半（粗搜用）。奇数边多出的那一行 / 列直接丢掉。
pub fn half(src: &Gray) -> Gray {
    let (w, h) = (src.width / 2, src.height / 2);
    let mut out = Gray::new(w, h);
    for y in 0..h {
        for x in 0..w {
            let (sx, sy) = (x * 2, y * 2);
            out.data[y * w + x] = 0.25
                * (src.at(sx, sy) + src.at(sx + 1, sy) + src.at(sx, sy + 1) + src.at(sx + 1, sy + 1));
        }
    }
    out
}

/// 双线性缩放（像素中心对齐，和 OpenCV 的 `INTER_LINEAR` 一致）。
pub fn resize(src: &Gray, width: usize, height: usize) -> Gray {
    let mut out = Gray::new(width, height);
    if src.width == 0 || src.height == 0 || width == 0 || height == 0 {
        return out;
    }
    let (sx, sy) = (src.width as f32 / width as f32, src.height as f32 / height as f32);
    for y in 0..height {
        let fy = ((y as f32 + 0.5) * sy - 0.5).clamp(0.0, (src.height - 1) as f32);
        let (y0, ty) = (fy.floor() as usize, fy - fy.floor());
        let y1 = (y0 + 1).min(src.height - 1);
        for x in 0..width {
            let fx = ((x as f32 + 0.5) * sx - 0.5).clamp(0.0, (src.width - 1) as f32);
            let (x0, tx) = (fx.floor() as usize, fx - fx.floor());
            let x1 = (x0 + 1).min(src.width - 1);
            let top = src.at(x0, y0) * (1.0 - tx) + src.at(x1, y0) * tx;
            let bottom = src.at(x0, y1) * (1.0 - tx) + src.at(x1, y1) * tx;
            out.data[y * width + x] = top * (1.0 - ty) + bottom * ty;
        }
    }
    out
}

/// 方形结构元的腐蚀 / 膨胀（`size` 为奇数边长，边缘复制）。
fn morph(src: &Gray, size: usize, dilate: bool) -> Gray {
    let r = (size / 2) as isize;
    let (w, h) = (src.width as isize, src.height as isize);
    let pick = |a: f32, b: f32| if dilate { a.max(b) } else { a.min(b) };
    // 先横后竖，两遍一维代替一遍二维。
    let mut rows = Gray::new(src.width, src.height);
    for y in 0..h {
        for x in 0..w {
            let mut v = src.data[(y * w + x) as usize];
            for dx in -r..=r {
                let xx = (x + dx).clamp(0, w - 1);
                v = pick(v, src.data[(y * w + xx) as usize]);
            }
            rows.data[(y * w + x) as usize] = v;
        }
    }
    let mut out = Gray::new(src.width, src.height);
    for y in 0..h {
        for x in 0..w {
            let mut v = rows.data[(y * w + x) as usize];
            for dy in -r..=r {
                let yy = (y + dy).clamp(0, h - 1);
                v = pick(v, rows.data[(yy * w + x) as usize]);
            }
            out.data[(y * w + x) as usize] = v;
        }
    }
    out
}

/// 白顶帽：原图减去开运算。留下「比周围亮、又比结构元细」的笔画 ——
/// 正好是倒计时那种白字黑边的小数字，图标本身的大块亮面会被减掉。
pub fn white_top_hat(src: &Gray, size: usize) -> Gray {
    let opened = morph(&morph(src, size, false), size, true);
    let mut out = Gray::new(src.width, src.height);
    for (i, v) in out.data.iter_mut().enumerate() {
        *v = (src.data[i] - opened.data[i]).max(0.0);
    }
    out
}

/// 二值图（`true` = 墨）。
#[derive(Debug, Clone, PartialEq)]
pub struct Mask {
    pub width: usize,
    pub height: usize,
    pub data: Vec<bool>,
}

impl Mask {
    #[inline]
    pub fn on(&self, x: usize, y: usize) -> bool {
        self.data[y * self.width + x]
    }

    /// 网页版的「前景」：顶帽之后取 max(下限, 最大值 × 比例) 为阈值。
    pub fn foreground(gray: &Gray, size: usize, ratio: f32, minimum: f32) -> Self {
        let top = white_top_hat(gray, size);
        let threshold = minimum.max(top.max() * ratio);
        Self {
            width: gray.width,
            height: gray.height,
            data: top.data.iter().map(|&v| v >= threshold).collect(),
        }
    }
}

/// 一个连通域的外接框与面积。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Blob {
    pub x: usize,
    pub y: usize,
    pub width: usize,
    pub height: usize,
    pub area: usize,
}

/// 8 连通的连通域。
pub fn components(mask: &Mask) -> Vec<Blob> {
    let (w, h) = (mask.width, mask.height);
    let mut seen = vec![false; w * h];
    let mut out = Vec::new();
    let mut stack = Vec::new();
    for start in 0..w * h {
        if seen[start] || !mask.data[start] {
            continue;
        }
        seen[start] = true;
        stack.push(start);
        let (mut x0, mut y0, mut x1, mut y1, mut area) = (w, h, 0, 0, 0);
        while let Some(i) = stack.pop() {
            let (x, y) = (i % w, i / w);
            area += 1;
            x0 = x0.min(x);
            y0 = y0.min(y);
            x1 = x1.max(x);
            y1 = y1.max(y);
            for dy in -1isize..=1 {
                for dx in -1isize..=1 {
                    let (nx, ny) = (x as isize + dx, y as isize + dy);
                    if nx < 0 || ny < 0 || nx >= w as isize || ny >= h as isize {
                        continue;
                    }
                    let j = ny as usize * w + nx as usize;
                    if !seen[j] && mask.data[j] {
                        seen[j] = true;
                        stack.push(j);
                    }
                }
            }
        }
        out.push(Blob { x: x0, y: y0, width: x1 - x0 + 1, height: y1 - y0 + 1, area });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn top_hat_keeps_thin_bright_strokes_and_drops_flat_areas() {
        // 左半边是一大块亮面，右边一根 1 像素宽的亮线
        let mut g = Gray::new(12, 8);
        for y in 0..8 {
            for x in 0..5 {
                g.data[y * 12 + x] = 200.0;
            }
            g.data[y * 12 + 9] = 220.0;
        }
        let top = white_top_hat(&g, 3);
        assert!(top.at(9, 4) > 200.0, "细线应该留下");
        assert!(top.at(2, 4) < 1.0, "大块亮面应该被减掉");
    }

    #[test]
    fn components_are_eight_connected() {
        let mask = Mask {
            width: 5,
            height: 3,
            data: vec![
                true, false, false, false, true, //
                false, true, false, false, true, //
                false, false, false, false, false,
            ],
        };
        let mut blobs = components(&mask);
        blobs.sort_by_key(|b| b.x);
        assert_eq!(blobs.len(), 2);
        assert_eq!((blobs[0].width, blobs[0].height, blobs[0].area), (2, 2, 2));
        assert_eq!((blobs[1].x, blobs[1].height), (4, 2));
    }

    #[test]
    fn resize_and_half_preserve_constant_images() {
        let g = Gray { width: 4, height: 4, data: vec![7.0; 16] };
        assert!(resize(&g, 9, 5).data.iter().all(|&v| (v - 7.0).abs() < 1e-4));
        assert!(half(&g).data.iter().all(|&v| (v - 7.0).abs() < 1e-4));
    }
}
