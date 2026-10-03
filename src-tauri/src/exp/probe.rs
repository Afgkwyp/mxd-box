//! 只在开发时手工跑的经验统计「现场探针」。
//!
//! 用途：在**不改产品代码**的前提下，把游戏窗口当前拍到的画面摊开来看 ——
//! 用户报告「某个分辨率下读不到经验」而我们手里没有他那台机器的画面时，
//! 这个探针是本机复现与定位的唯一手段（比让用户反复点按钮快得多）。
//!
//! 跑法（游戏先开好、切到那个分辨率）：
//!
//! ```text
//! cd src-tauri
//! cargo test --lib -- --ignored probe_exp_layout --nocapture
//! ```
//!
//! 它会打印：窗口几何与形态、两种抓法的亮度统计、整窗粗图、底栏 1:1 字符图、
//! 现有候选横带的结论，以及**从底部往上逐条 OCR 的扫描结果**（后者用来回答
//! 「经验那一行到底在哪个高度」这个问题）。
//!
//! 为什么放在 `#[cfg(test)]` + `#[ignore]`：它是诊断工具不是产品功能，不能进
//! 正常测试（会依赖真实游戏窗口），但需要访问 crate 内部的 `exp::*`，所以不能
//! 放在 `tests/` 里当集成测试。

use crate::exp::{capture, font, ocr, table};

/// 每块取**最大** luma：经验那一行只有 9~14 像素高，块平均会把它抹平。
fn coarse_map(pixels: &font::Pixels<'_>, cols: usize, rows: usize) -> Vec<String> {
    let mut out = Vec::with_capacity(rows);
    for row in 0..rows {
        let y0 = row * pixels.height / rows;
        let y1 = ((row + 1) * pixels.height / rows).max(y0 + 1);
        let mut line = String::with_capacity(cols);
        for col in 0..cols {
            let x0 = col * pixels.width / cols;
            let x1 = ((col + 1) * pixels.width / cols).max(x0 + 1);
            let mut peak = 0u8;
            for y in y0..y1.min(pixels.height) {
                for x in x0..x1.min(pixels.width) {
                    peak = peak.max(pixels.luma(x, y));
                }
            }
            line.push(if peak > font::INK_THRESHOLD { '#' } else { '.' });
        }
        out.push(line);
    }
    out
}

/// 逐行原样（横向抽样）打印，行首带**该行在客户区里的 y 坐标**，便于直接读位置。
fn band_map(pixels: &font::Pixels<'_>, top: usize, step_x: usize) -> Vec<String> {
    let mut out = Vec::with_capacity(pixels.height - top);
    for y in top..pixels.height {
        let mut line = format!("y={y:>4} ");
        let mut x = 0;
        while x < pixels.width {
            line.push(if pixels.luma(x, y) > font::INK_THRESHOLD { '#' } else { '.' });
            x += step_x;
        }
        out.push(line);
    }
    out
}

fn stats_line(label: &str, pixels: &font::Pixels<'_>) -> String {
    let s = capture::luma_stats(pixels);
    format!(
        "{label}: {}×{} 中位 {} min {} max {} 亮 {:.1}%",
        pixels.width,
        pixels.height,
        s.median,
        s.min,
        s.max,
        s.bright_ratio * 100.0
    )
}

#[test]
#[ignore]
fn probe_exp_layout() {
    capture::ensure_dpi_aware();

    // 让调用方有时间把游戏切到最前面（跑在后台时终端会抢焦点）：
    //   PROBE_WAIT_MS=8000 cargo test ... probe_exp_layout
    if let Ok(ms) = std::env::var("PROBE_WAIT_MS") {
        if let Ok(ms) = ms.parse::<u64>() {
            println!("等 {ms} 毫秒后开始抓屏（现在去把游戏切到最前面）…");
            std::thread::sleep(std::time::Duration::from_millis(ms));
        }
    }

    let Some(window) = capture::find_game_window() else {
        println!("没找到游戏窗口（游戏没开？）");
        return;
    };
    let hwnd = window.hwnd;
    // 最小化时 PrintWindow 拿不到画面：用「不抢焦点」的方式恢复一下（只影响这个探针）
    if capture::is_minimized(hwnd) {
        println!("窗口是最小化的 —— 用 SW_SHOWNOACTIVATE 恢复（不会抢你当前窗口的焦点）…");
        capture::restore_without_activating(hwnd);
        std::thread::sleep(std::time::Duration::from_millis(1500));
    }
    let (client_w, client_h) = capture::client_size(hwnd).unwrap_or((0, 0));
    let strip = capture::strip_height(client_h);

    println!("==================== 窗口 ====================");
    println!("标题      : {}", window.title);
    println!("hwnd      : {hwnd}");
    println!("客户区    : {client_w}×{client_h}");
    println!("条高      : {strip} 行（客户区高的 10%，44 下限 / 144 上限）");
    println!("前台      : {}", capture::is_foreground(hwnd));
    println!("形态      : {:?}", capture::screen_mode(hwnd));

    // ---------- 整窗 ----------
    println!("\n==================== 整窗（PrintWindow）====================");
    let Some((full, method)) = capture::grab_client(hwnd, 0, 0, client_w, client_h) else {
        println!("整窗抓不到");
        return;
    };
    let full_pixels = full.pixels();
    println!("抓法      : {method:?}");
    println!("{}", stats_line("整窗", &full_pixels));
    println!("\n--- 整窗粗图（每格取最大亮度，# = 亮于 {}）---", font::INK_THRESHOLD);
    for line in coarse_map(&full_pixels, 160, 45) {
        println!("{line}");
    }

    // ---------- 底栏 ----------
    println!("\n==================== 底栏（最底下 {strip} 行）====================");
    let Some((strip_frame, strip_method)) =
        capture::grab_client(hwnd, 0, client_h - strip, client_w, strip)
    else {
        println!("底栏抓不到");
        return;
    };
    let strip_pixels = strip_frame.pixels();
    println!("抓法      : {strip_method:?}");
    println!("{}", stats_line("底栏", &strip_pixels));

    // ---------- HUD 锚点（网页版枫记的做法） ----------
    println!("\n==================== HUD 锚点定位（血条边框 + 标签模板，多尺度）====================");
    let probe_font = font::Font::builtin();
    match crate::exp::hud::locate(&full_pixels) {
        Some(layout) => {
            let (ex, ey, ew, eh) = layout.exp_text;
            let (lx, ly, lw, lh) = layout.level_box;
            println!(
                "锚点: ({:.1}, {:.1}) · 缩放 {:.3}×{:.3} · 误差 {:.1}",
                layout.anchor.x, layout.anchor.y, layout.anchor.sx, layout.anchor.sy, layout.anchor.error
            );
            println!("经验行: ({ex}, {ey}, {ew}×{eh}) · 等级框: ({lx}, {ly}, {lw}×{lh})");
            match layout.map_name {
                Some((mx, my, mw, mh)) => {
                    println!("地图名: ({mx}, {my}, {mw}×{mh})");
                    if mw >= 8 && mh >= 4 {
                        let (cw, ch, buf) = full_pixels.crop(
                            mx.max(0) as usize,
                            my.max(0) as usize,
                            mw as usize,
                            mh as usize,
                        );
                        if cw >= 8 && ch >= 4 {
                            let cropped = font::Pixels { width: cw, height: ch, bgra: &buf };
                            match ocr::recognize_line(&cropped) {
                                Some(reading) => println!(
                                    "地图名 OCR: 「{}」(conf {:.2}) · 像地图名={}",
                                    reading.text,
                                    reading.confidence,
                                    ocr::looks_like_map_name(&reading)
                                ),
                                None => println!("地图名 OCR: 认不出"),
                            }
                        }
                    }
                }
                None => println!("地图名: 小地图锚点没找到"),
            }
            if ew >= 8 && eh >= 8 {
                let (cw, ch, buf) = full_pixels.crop(
                    ex.max(0) as usize,
                    ey.max(0) as usize,
                    ew as usize,
                    eh as usize,
                );
                if cw >= 8 && ch >= 8 {
                    let cropped = font::Pixels { width: cw, height: ch, bgra: &buf };
                    if let Some(reading) = ocr::recognize_line(&cropped) {
                        println!(
                            "经验行 OCR: 「{}」(conf {:.2}) → {:?}",
                            reading.text,
                            reading.confidence,
                            ocr::parse_exp_ocr(&reading.text)
                        );
                    }
                    if let Some(reading) = font::read_frame(&cropped, &probe_font) {
                        println!("经验行点阵: 「{}」→ {:?}", reading.raw, table::validate(reading.exp, reading.percent));
                    } else {
                        let unmatched = font::unmatched_glyphs(&cropped, &probe_font);
                        println!("经验行点阵: 认不出（对不上的字形 {} 个）", unmatched.len());
                        for (x, bitmap) in unmatched.iter().take(4) {
                            println!("   x={x} {bitmap}");
                        }
                        // 逐行画出这一小条（1:1），字形差异一眼可见
                        for y in 0..ch {
                            let line: String = (0..cw)
                                .map(|x| if cropped.luma(x, y) > font::INK_THRESHOLD { '#' } else { '.' })
                                .collect();
                            println!("   {line}");
                        }
                    }
                }
            }
        }
        None => println!("HUD 锚点: 没找到（不在角色里？血条被面板挡住？）"),
    }

    // ---------- 多尺度候选与读数（和产品读取链同一套） ----------
    println!("\n==================== 整幅自动定位（多尺度点阵 + OCR）====================");
    let probe_font = font::Font::builtin();
    match font::read_frame(&full_pixels, &probe_font) {
        Some(reading) => println!(
            "read_frame: ★ {} → exp {} / {}% · 字高 {} · 校验 {:?}",
            reading.raw,
            reading.exp,
            reading.percent,
            reading.text_height,
            table::validate(reading.exp, reading.percent)
        ),
        None => println!("read_frame: 点阵多尺度路径没有读出（下面逐条候选看原因）"),
    }

    let candidates = font::find_candidate_boxes(&full_pixels, 8);
    println!("候选横带共 {} 条（按分数从高到低）：", candidates.len());
    for (index, candidate) in candidates.iter().enumerate() {
        let reading = font::read_candidate(&full_pixels, candidate, &probe_font);
        let reading_line = match &reading {
            Some(reading) => format!(
                "点阵「{}」→ {:?}",
                reading.raw,
                table::validate(reading.exp, reading.percent)
            ),
            None => "点阵认不出".to_string(),
        };
        let (cw, ch, buf) = full_pixels.crop(
            candidate.rect.0,
            candidate.rect.1,
            candidate.rect.2,
            candidate.rect.3,
        );
        let ocr_line = if cw >= 8 && ch >= 8 {
            let cropped = font::Pixels {
                width: cw,
                height: ch,
                bgra: &buf,
            };
            match ocr::recognize_line(&cropped) {
                Some(reading) => format!(
                    "OCR「{}」(conf {:.2}) → {:?}",
                    reading.text,
                    reading.confidence,
                    ocr::parse_exp_ocr(&reading.text)
                ),
                None => "OCR 认不出文字".to_string(),
            }
        } else {
            "裁剪太小，跳过 OCR".to_string()
        };
        println!(
            "  #{index} (x={}, y={}, {}×{}, 字高 {})：{reading_line} · {ocr_line}",
            candidate.rect.0,
            candidate.rect.1,
            candidate.rect.2,
            candidate.rect.3,
            candidate.text_height
        );
    }

    // ---------- 认不出的字形（诊断「有字但对不上」） ----------
    if font::read_frame(&full_pixels, &probe_font).is_none() {
        let unmatched = font::unmatched_glyphs(&full_pixels, &probe_font);
        if unmatched.is_empty() {
            println!("对不上的字形：无（画面里可能根本没有那一行文字）");
        } else {
            println!("对不上的字形 {} 个（前 4 个）：", unmatched.len());
            for (x, bitmap) in unmatched.iter().take(4) {
                println!("  x={x} {bitmap}");
            }
        }
    }

    // ---------- 两种抓法对比：后台截屏到底是不是这个游戏 ----------
    println!("\n==================== 两种抓法对比 ====================");
    let screen_frame = capture::grab_client_by_method(
        hwnd,
        0,
        0,
        client_w,
        client_h,
        capture::Method::Screen,
    );
    match &screen_frame {
        Some(frame) => {
            let sp = frame.pixels();
            println!("{}", stats_line("屏幕 DC ", &sp));
            // 逐块比较（容差 32，块 16×16）：动画会让逐像素比较永远“不同”
            let step = 16usize;
            let mut same = 0usize;
            let mut total = 0usize;
            let mut y = 0usize;
            while y + step <= full_pixels.height.min(sp.height) {
                let mut x = 0usize;
                while x + step <= full_pixels.width.min(sp.width) {
                    let mut far = 0usize;
                    for dy in 0..step {
                        for dx in 0..step {
                            let a = full_pixels.luma(x + dx, y + dy) as i32;
                            let b = sp.luma(x + dx, y + dy) as i32;
                            if (a - b).abs() > 32 {
                                far += 1;
                            }
                        }
                    }
                    if (far as f64) / ((step * step) as f64) < 0.15 {
                        same += 1;
                    }
                    total += 1;
                    x += step;
                }
                y += step;
            }
            let ratio = if total == 0 { 0.0 } else { same as f64 / total as f64 };
            println!("两图一致块占比: {:.1}% （低于 85% 说明后台截屏拿到的不是屏幕上的那个画面）", ratio * 100.0);
            println!("前台      : {}", capture::is_foreground(hwnd));
        }
        None => println!("屏幕 DC 抓不到"),
    }

    // ---------- 逐条带扫描：经验那一行到底在多高 ----------
    println!("\n==================== 逐条带 OCR 扫描（从底部往上）====================");
    println!("带高 28、步进 12；对**两种抓法**各扫一遍，只打印 OCR 出非空文字的带");
    let band_h = 28i32;
    let step = 12i32;
    let scan_from = (client_h - 600).max(0);

    let mut scans: Vec<(&str, font::Pixels<'_>)> = vec![("后台截屏", full_pixels)];
    if let Some(frame) = screen_frame.as_ref() {
        scans.push(("屏幕 DC", frame.pixels()));
    }
    for (label, source) in scans {
        println!("\n--- {label} ---");
        let mut y = client_h - band_h;
        let mut hits = 0usize;
        while y >= scan_from {
            let (bw, bh, buf) =
                source.crop(0, y as usize, client_w as usize, band_h as usize);
            if bw > 0 && bh > 0 {
                let band = font::Pixels { width: bw, height: bh, bgra: &buf };
                if let Some(reading) = ocr::recognize_line(&band) {
                    let text = reading.text.trim();
                    if !text.is_empty() {
                        let parsed = ocr::parse_exp_ocr(text);
                        let suffix = match parsed {
                            Some((exp, pct)) => format!(
                                "  ★解析成功 exp={exp} pct={pct} 校验={:?}",
                                table::validate(exp, pct)
                            ),
                            None => String::new(),
                        };
                        println!(
                            "y={y:>4} 高{band_h} 「{text}」conf {:.2}{suffix}",
                            reading.confidence
                        );
                        hits += 1;
                    }
                }
            }
            y -= step;
        }
        if hits == 0 {
            println!("（没有任何 OCR 文字输出）");
        }
    }

    // ---------- 底栏 1:1 ----------
    println!("\n==================== 底栏逐行细图（横向每 2 列取 1 列）====================");
    let top = strip_pixels.height.saturating_sub(160);
    for line in band_map(&strip_pixels, top, 2) {
        println!("{line}");
    }
}

/// 真机探针：完整读一次经验（含等级框兜底），把 [`crate::exp::reader::Sample`]
/// 的每个字段摊开 —— 判「定不出等级」卡在哪一环时用。
///
/// ```text
/// cd src-tauri
/// cargo test --lib -- --ignored probe_exp_sample --nocapture
/// ```
#[test]
#[ignore]
fn probe_exp_sample() {
    capture::ensure_dpi_aware();
    let mut reader = crate::exp::ExpReader::new(font::Font::builtin());
    match reader.read() {
        Ok(sample) => println!(
            "读到了：raw=「{}」exp={} pct={:.2} 经验表等级={:?} 等级框={:?} 抓法={:?}",
            sample.raw,
            sample.exp,
            sample.percent,
            sample.level,
            sample.screen_level,
            sample.method
        ),
        Err(failure) => println!("没读到：{}（{}）", failure.message(), failure.code()),
    }
}

/// 把游戏当前画面原样存盘（开发时对着真机画面调算法用）。
///
/// ```text
/// DUMP_DIR=某个目录 cargo test --lib -- --ignored dump_game_frames --nocapture
/// ```
///
/// 存一张整幅客户区 + 连续若干帧的 HUD 经验条那一块（看帧与帧之间怎么变）。
/// 文件是裸 BGRA，文件名里带宽高。
#[test]
#[ignore]
fn dump_game_frames() {
    capture::ensure_dpi_aware();
    let dir = std::path::PathBuf::from(std::env::var("DUMP_DIR").expect("要设 DUMP_DIR"));
    std::fs::create_dir_all(&dir).expect("建目录失败");
    let window = capture::find_game_window().expect("没找到游戏窗口");
    let hwnd = window.hwnd;
    println!("标题 {} · 最小化 {}", window.title, capture::is_minimized(hwnd));
    let (w, h) = capture::client_size(hwnd).expect("拿不到客户区");
    println!("客户区 {w}×{h} · 形态 {:?} · 前台 {}", capture::screen_mode(hwnd), capture::is_foreground(hwnd));
    let (full, method) = capture::grab_client(hwnd, 0, 0, w, h).expect("整幅抓不到");
    std::fs::write(dir.join(format!("full_{w}x{h}.bgra")), &full.pixels().bgra).unwrap();
    println!("整幅抓法 {method:?}");
    let pixels = full.pixels();
    let layout = crate::exp::hud::locate(&pixels).expect("HUD 定位失败");
    println!("锚点 {:?}", layout.anchor);
    let (sx, sy, sw, sh) = layout.read_strip(w, h);
    println!("strip {sx},{sy} {sw}x{sh}");
    let count: usize = std::env::var("DUMP_FRAMES").ok().and_then(|v| v.parse().ok()).unwrap_or(12);
    for i in 0..count {
        for (label, m) in [("win", capture::Method::Window), ("scr", capture::Method::Screen)] {
            if let Some(frame) = capture::grab_client_by_method(hwnd, sx, sy, sw, sh, m) {
                std::fs::write(
                    dir.join(format!("strip_{i:02}_{label}_{sw}x{sh}.bgra")),
                    &frame.pixels().bgra,
                )
                .unwrap();
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(400));
    }
    println!("完成");
}

/// 对着正在运行的游戏，用产品里同一条读数流程连读若干次（看稳不稳）。
///
/// ```text
/// cargo test --release --lib -- --ignored live_read_loop --nocapture
/// ```
#[test]
#[ignore]
fn live_read_loop() {
    capture::ensure_dpi_aware();
    let mut reader = crate::exp::reader::ExpReader::new(font::Font::builtin());
    let rounds: usize = std::env::var("LIVE_ROUNDS").ok().and_then(|v| v.parse().ok()).unwrap_or(20);
    let (mut ok, mut failed) = (0, 0);
    for i in 0..rounds {
        let started = std::time::Instant::now();
        match reader.read() {
            Ok(sample) => {
                ok += 1;
                println!(
                    "{i:02} ✓ {} → {} / {}% · 等级 {:?} · {:?}",
                    sample.raw,
                    sample.exp,
                    sample.percent,
                    sample.level,
                    started.elapsed()
                );
            }
            Err(failure) => {
                failed += 1;
                println!("{i:02} ✗ {} · {:?}", failure.message(), started.elapsed());
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(500));
    }
    println!("成功 {ok} / 失败 {failed}");
}

/// 模拟校准窗口里手动框一个框、点「测试读数」（松框：标签 + 数字 + 经验条）。
#[test]
#[ignore]
fn live_manual_box() {
    capture::ensure_dpi_aware();
    let window = capture::find_game_window().expect("没找到游戏窗口");
    let (w, h) = capture::client_size(window.hwnd).expect("拿不到客户区");
    let (full, _) = capture::grab_client(window.hwnd, 0, 0, w, h).expect("抓不到");
    let layout = crate::exp::hud::locate(&full.pixels()).expect("HUD 定位失败");
    let (x, y, tw, th) = layout.exp_text;
    let mut reader = crate::exp::reader::ExpReader::new(font::Font::builtin());
    for (label, (bx, by, bw, bh)) in [
        ("松框", (x - 20, y - 10, tw + 60, th + 40)),
        ("紧框", (x + tw / 5, y, tw * 3 / 4, th)),
    ] {
        println!("客户区 {w}×{h} · 缩放 {:.3} · 框 ({bx},{by}) {bw}×{bh}", layout.anchor.sx);
        let pixel = crate::exp::region::PixelRect { x: bx, y: by, w: bw, h: bh }.clamp_to(w, h);
        let Some(rect) = crate::exp::region::NormRect::from_pixel(pixel, w, h) else {
            println!("{label}：框换算失败");
            continue;
        };
        let result = reader.test_region(crate::exp::region::KIND_EXP_LINE, rect);
        println!("{label} {bw}×{bh}: ok={} · {}", result.ok, result.message);
    }
}

/// 存一块 BGRA 为 PNG（探针用，方便直接打开看）。
fn save_png(path: &std::path::Path, pixels: &font::Pixels<'_>) {
    let mut rgba = Vec::with_capacity(pixels.width * pixels.height * 4);
    for chunk in pixels.bgra.chunks_exact(4) {
        rgba.extend_from_slice(&[chunk[2], chunk[1], chunk[0], 255]);
    }
    let file = std::fs::File::create(path).expect("建文件失败");
    let mut encoder = png::Encoder::new(file, pixels.width as u32, pixels.height as u32);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder
        .write_header()
        .and_then(|mut writer| writer.write_image_data(&rgba))
        .expect("写 PNG 失败");
}

/// 对着正在运行的游戏看「地图名那一行」：框在哪、多宽、两种喂法各读出什么。
///
/// 用来查「地名里的数字读丢了」这类问题（`地铁二号线<第3地区>` 读成 `地铁二号线-第地区`）。
///
/// ```text
/// DUMP_DIR=... cargo test --lib -- --ignored probe_map_name --nocapture
/// ```
#[test]
#[ignore]
fn probe_map_name() {
    capture::ensure_dpi_aware();
    let dir = std::path::PathBuf::from(std::env::var("DUMP_DIR").expect("要设 DUMP_DIR"));
    std::fs::create_dir_all(&dir).expect("建目录失败");
    let mut reader = crate::exp::reader::ExpReader::new(font::Font::builtin());
    let hwnd = reader.window().expect("没找到游戏窗口");
    let (w, h) = capture::client_size(hwnd).expect("拿不到客户区");
    println!("客户区 {w}×{h}");
    let rect = reader.ensure_auto_map_rect(w, h).expect("没定位到小地图");
    println!("地图名区域 ({}, {}) {}×{}", rect.x, rect.y, rect.w, rect.h);
    let (frame, method) = capture::grab_client(hwnd, rect.x, rect.y, rect.w, rect.h).expect("抓不到");
    println!("抓法 {method:?}");
    let pixels = frame.pixels();
    save_png(&dir.join("map_name.png"), &pixels);
    // 顺手存一块大一点的左上角，看框落在小地图的什么位置
    let corner_w = (rect.x + rect.w + 80).min(w);
    let corner_h = (rect.y + rect.h + 60).min(h);
    if let Some((corner, _)) = capture::grab_client(hwnd, 0, 0, corner_w, corner_h) {
        save_png(&dir.join("map_corner.png"), &corner.pixels());
    }
    println!("旧（压进 320 宽）   : {:?}", ocr::recognize_line(&pixels));
    println!("不压扁、不裁        : {:?}", ocr::recognize_wide_line(&pixels));
    println!("产品里现在的读法    : {:?}", crate::exp::map::read_map_name_pixels(&pixels));
}
