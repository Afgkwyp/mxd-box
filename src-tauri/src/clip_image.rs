//! 把一张 PNG 放进系统剪贴板 —— 小结卡片的「复制图片」。
//!
//! 为什么要自己写剪贴板：桌面版微信 / QQ 没有给别的程序用的「分享」接口，
//! 发图最短的路就是「复制 → 到聊天框里 Ctrl+V」。它们粘贴时认的是 `CF_DIB`
//! （一张未压缩的位图），不认 PNG 文件的字节，所以这里把 PNG 解开、转成 DIB 再放进去。
//!
//! 不引剪贴板的 crate：要的只是「写一张图」这一个动作，几十行 FFI 就够，
//! 和 `blacklist.rs` 里读剪贴板文字的做法一致。

/// 透明像素垫的底色（和卡片底板同一个颜色）。
///
/// 24 位 DIB 没有透明通道；卡片本来就是不透明的，这只是给万一出现的半透明边缘兜底，
/// 免得它们在聊天框里变成一圈黑边。
const BACKDROP: [u8; 3] = [0x0e, 0x0a, 0x08];

/// 单边像素上限：卡片是一千多像素见方，超过这个数的一定不是我们画的那张。
const MAX_SIDE: u32 = 8192;

/// PNG → `CF_DIB` 要的那一段内存：`BITMAPINFOHEADER`（40 字节）+ 自下而上的 BGR 像素行。
pub fn png_to_dib(png_bytes: &[u8]) -> Result<Vec<u8>, String> {
    let mut decoder = png::Decoder::new(std::io::Cursor::new(png_bytes));
    // 调色板 / 灰度 / 16 位统统展开成 8 位的 RGB(A)，下面只用处理两种排列
    decoder.set_transformations(png::Transformations::normalize_to_color8());
    let mut reader = decoder
        .read_info()
        .map_err(|err| format!("图片解不开：{}", err))?;
    let (width, height) = {
        let info = reader.info();
        (info.width, info.height)
    };
    if width == 0 || height == 0 || width > MAX_SIDE || height > MAX_SIDE {
        return Err(format!("图片尺寸不对（{}×{}）", width, height));
    }

    let mut pixels = vec![0u8; reader.output_buffer_size()];
    let frame = reader
        .next_frame(&mut pixels)
        .map_err(|err| format!("图片解不开：{}", err))?;
    let channels = frame.color_type.samples();
    if channels == 0 || frame.line_size < width as usize * channels {
        return Err("图片像素格式不认识".to_string());
    }

    let (width, height) = (width as usize, height as usize);
    // 每行按 4 字节对齐（DIB 的规矩）
    let stride = (width * 3 + 3) & !3;
    let mut dib = vec![0u8; 40 + stride * height];
    dib[0..4].copy_from_slice(&40u32.to_le_bytes());
    dib[4..8].copy_from_slice(&(width as i32).to_le_bytes());
    // 高度为正 = 自下而上存放
    dib[8..12].copy_from_slice(&(height as i32).to_le_bytes());
    dib[12..14].copy_from_slice(&1u16.to_le_bytes());
    dib[14..16].copy_from_slice(&24u16.to_le_bytes());
    dib[20..24].copy_from_slice(&((stride * height) as u32).to_le_bytes());

    for y in 0..height {
        let source = &pixels[y * frame.line_size..];
        let target = 40 + (height - 1 - y) * stride;
        for x in 0..width {
            let pixel = &source[x * channels..x * channels + channels];
            // 灰度图只有 1~2 个通道：三个颜色都取第一个
            let (rgb, alpha) = match channels {
                1 => ([pixel[0]; 3], 255u16),
                2 => ([pixel[0]; 3], pixel[1] as u16),
                3 => ([pixel[0], pixel[1], pixel[2]], 255u16),
                _ => ([pixel[0], pixel[1], pixel[2]], pixel[3] as u16),
            };
            for channel in 0..3 {
                let blended =
                    (rgb[channel] as u16 * alpha + BACKDROP[channel] as u16 * (255 - alpha)) / 255;
                // DIB 里是 B、G、R 的顺序
                dib[target + x * 3 + (2 - channel)] = blended as u8;
            }
        }
    }
    Ok(dib)
}

#[cfg(windows)]
mod native {
    use std::ffi::c_void;

    #[link(name = "user32")]
    extern "system" {
        fn OpenClipboard(hwnd: *mut c_void) -> i32;
        fn CloseClipboard() -> i32;
        fn EmptyClipboard() -> i32;
        fn SetClipboardData(format: u32, mem: *mut c_void) -> *mut c_void;
    }

    #[link(name = "kernel32")]
    extern "system" {
        fn GlobalAlloc(flags: u32, bytes: usize) -> *mut c_void;
        fn GlobalLock(mem: *mut c_void) -> *mut c_void;
        fn GlobalUnlock(mem: *mut c_void) -> i32;
        fn GlobalFree(mem: *mut c_void) -> *mut c_void;
    }

    const CF_DIB: u32 = 8;
    const GMEM_MOVEABLE: u32 = 0x0002;

    /// 剪贴板是全局独占的：别的程序（输入法、剪贴板管理器）正占着时会打不开，
    /// 隔一小会儿再试几次，基本都能等到。
    fn open() -> bool {
        for _ in 0..10 {
            if unsafe { OpenClipboard(std::ptr::null_mut()) } != 0 {
                return true;
            }
            std::thread::sleep(std::time::Duration::from_millis(30));
        }
        false
    }

    pub fn set_dib(dib: &[u8]) -> Result<(), String> {
        unsafe {
            let mem = GlobalAlloc(GMEM_MOVEABLE, dib.len());
            if mem.is_null() {
                return Err("内存不够，图片没复制上".to_string());
            }
            let target = GlobalLock(mem) as *mut u8;
            if target.is_null() {
                GlobalFree(mem);
                return Err("内存不够，图片没复制上".to_string());
            }
            std::ptr::copy_nonoverlapping(dib.as_ptr(), target, dib.len());
            GlobalUnlock(mem);

            if !open() {
                GlobalFree(mem);
                return Err("剪贴板正被别的程序占着，稍等一下再点一次".to_string());
            }
            EmptyClipboard();
            // 放成功以后这块内存归系统管，不能再释放；失败了才是我们的
            let placed = !SetClipboardData(CF_DIB, mem).is_null();
            CloseClipboard();
            if !placed {
                GlobalFree(mem);
                return Err("图片没放进剪贴板，再点一次试试".to_string());
            }
            Ok(())
        }
    }
}

/// 把 PNG 放进剪贴板（作为一张位图）。
pub fn copy_png(png_bytes: &[u8]) -> Result<(), String> {
    let dib = png_to_dib(png_bytes)?;
    #[cfg(windows)]
    {
        native::set_dib(&dib)
    }
    #[cfg(not(windows))]
    {
        let _ = dib;
        Err("这个系统上不支持复制图片".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn encode(width: u32, height: u32, color: png::ColorType, data: &[u8]) -> Vec<u8> {
        let mut bytes = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut bytes, width, height);
            encoder.set_color(color);
            encoder.set_depth(png::BitDepth::Eight);
            let mut writer = encoder.write_header().unwrap();
            writer.write_image_data(data).unwrap();
        }
        bytes
    }

    #[test]
    fn dib_is_bottom_up_bgr_with_padded_rows() {
        // 3×2：上一行 红 绿 蓝，下一行 白 黑 白
        let rgb = [
            255, 0, 0, 0, 255, 0, 0, 0, 255, //
            255, 255, 255, 0, 0, 0, 255, 255, 255,
        ];
        let dib = png_to_dib(&encode(3, 2, png::ColorType::Rgb, &rgb)).unwrap();

        // 3 像素 × 3 字节 = 9，对齐到 12
        assert_eq!(dib.len(), 40 + 12 * 2);
        assert_eq!(u32::from_le_bytes(dib[0..4].try_into().unwrap()), 40);
        assert_eq!(i32::from_le_bytes(dib[4..8].try_into().unwrap()), 3);
        assert_eq!(i32::from_le_bytes(dib[8..12].try_into().unwrap()), 2);
        assert_eq!(u16::from_le_bytes(dib[14..16].try_into().unwrap()), 24);

        // 第一行存的是图片的**最下面**一行：白 黑 白
        assert_eq!(&dib[40..49], &[255, 255, 255, 0, 0, 0, 255, 255, 255]);
        // 第二行是图片最上面一行，顺序 B G R：红 → (0,0,255)
        assert_eq!(&dib[52..61], &[0, 0, 255, 0, 255, 0, 255, 0, 0]);
    }

    #[test]
    fn transparent_pixels_fall_back_to_the_backdrop() {
        let rgba = [10, 20, 30, 0, 10, 20, 30, 255];
        let dib = png_to_dib(&encode(2, 1, png::ColorType::Rgba, &rgba)).unwrap();
        assert_eq!(&dib[40..43], &[BACKDROP[2], BACKDROP[1], BACKDROP[0]]);
        assert_eq!(&dib[43..46], &[30, 20, 10]);
    }

    #[test]
    fn garbage_is_rejected() {
        assert!(png_to_dib(b"not a png").is_err());
    }
}
