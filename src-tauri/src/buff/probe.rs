//! BUFF 秒数的现场采样探针（只在开发时手工跑，不进产品）。
//!
//! 用途：冒险岛怀旧服的 BUFF 倒计时数字和网页版字形库的字不是同一套字体，
//! 要从真机游戏里收集字形。这个探针每 0.5 秒抓一次客户区**右上角那一块**
//! （BUFF 栏所在的位置：右半边、顶部 12% 高），按时间存成 PNG，之后挑数字来用。
//!
//! 跑法（先把游戏开好、BUFF 在跑，窗口别让别的窗口压在右上角上面）：
//!
//! ```text
//! cd src-tauri
//! $env:DUMP_DIR = "C:\某个目录"
//! $env:DUMP_SECONDS = "240"        # 可不设，默认 240 秒
//! cargo test --release --lib -- --ignored dump_buff_bar --nocapture
//! ```
//!
//! 文件名是 `{毫秒:07}_{客户区宽}x{客户区高}.png`。
//! 注意：游戏被别的窗口挡住右上角，抓到的就是那个窗口，不是 BUFF 栏。
//!
//! 为什么放在 `#[cfg(test)]` + `#[ignore]`：它依赖真实游戏窗口，不能进正常测试；
//! 又要访问 crate 内部的 `exp::capture`，所以不放在 `tests/` 里。

use crate::exp::capture;
use std::time::{Duration, Instant};

/// 抓的间隔。
const TICK: Duration = Duration::from_millis(500);
/// 抓的高度占客户区高度的比例（BUFF 栏在顶部）。
const BAND: f64 = 0.12;

/// BGRA 一帧 → RGB PNG。
fn save_png(path: &std::path::Path, frame: &capture::Frame) {
    let rgb: Vec<u8> = frame.bgra.chunks_exact(4).flat_map(|p| [p[2], p[1], p[0]]).collect();
    let file = std::fs::File::create(path).expect("写文件失败");
    let mut encoder = png::Encoder::new(
        std::io::BufWriter::new(file),
        frame.width as u32,
        frame.height as u32,
    );
    encoder.set_color(png::ColorType::Rgb);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().expect("PNG 头写入失败");
    writer.write_image_data(&rgb).expect("PNG 数据写入失败");
}

#[test]
#[ignore]
fn dump_buff_bar() {
    capture::ensure_dpi_aware();
    let dir = std::path::PathBuf::from(std::env::var("DUMP_DIR").expect("要设 DUMP_DIR"));
    std::fs::create_dir_all(&dir).expect("建目录失败");
    let seconds: u64 = std::env::var("DUMP_SECONDS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(240);
    println!("存到 {} · 抓 {seconds} 秒", dir.display());

    let started = Instant::now();
    let mut hwnd: Option<isize> = None;
    let mut method: Option<capture::Method> = None;
    let mut saved = 0usize;
    while started.elapsed() < Duration::from_secs(seconds) {
        std::thread::sleep(TICK);
        if !hwnd.is_some_and(capture::is_window) {
            hwnd = capture::find_game_window().map(|g| g.hwnd);
        }
        let Some(window) = hwnd else {
            continue;
        };
        if capture::is_minimized(window) {
            continue;
        }
        let Some((w, h)) = capture::client_size(window) else {
            continue;
        };
        let x = w / 2;
        let band_h = ((h as f64 * BAND).round() as i32).max(1);
        let Some((frame, how)) = capture::grab_client(window, x, 0, w - x, band_h) else {
            continue;
        };
        method.get_or_insert(how);
        let elapsed_ms = started.elapsed().as_millis();
        save_png(&dir.join(format!("{elapsed_ms:07}_{w}x{h}.png")), &frame);
        saved += 1;
        if saved % 20 == 0 {
            println!("第 {saved} 张 · {elapsed_ms} ms · 客户区 {w}×{h} · 抓法 {how:?}");
        }
    }
    println!("完成：一共存了 {saved} 张 · 抓法 {method:?}");
}
