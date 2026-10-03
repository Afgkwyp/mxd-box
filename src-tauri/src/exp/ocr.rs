//! 地图名识别：跑 PP-OCR 的识别模型（rec）。
//!
//! # 为什么是它
//!
//! 小地图上那一行是 **11 像素高、抗锯齿、带描边和渐变的中文**。这个字号下：
//!
//! * Windows 自带的 `Windows.Media.Ocr` 读不出来（连干净的 12 像素黑字都读成「金裉島」）；
//! * 「拿字体渲染候选再比对」也没有字体可用。
//!
//! 但**网页版枫记（fj.need.run）证明这件事可行** —— 它用的是 **PaddleOCR PP-OCR**：
//! 先用像素模板定位小地图，再对那一小块跑**真正的中文识别神经网络**。
//! 这里走同一条路，只取「识别」那半步（位置我们是固定的，不需要检测模型）：
//!
//! ```text
//! 小地图里截两行 → 每行等比缩放到高 48 → 归一化到 [-1,1] → PP-OCR rec 模型 → CTC 解码
//! ```
//!
//! 实测（真机、`PP-OCRv6_tiny_rec`）：上一行地名 `金银岛` 置信度 **0.999**，
//! 下一行地图名 `废弃都市` 置信度 **0.941**。够用了。
//!
//! # 为什么用 `tract` 而不是 onnxruntime
//!
//! onnxruntime 要随包带一个二十多兆的 `onnxruntime.dll`，而且它是 C++ 运行时 ——
//! 这个工具是「拷一个 exe 就能用」的，多一个 DLL 就多一种坏法。
//! `tract` 是纯 Rust，编译进 exe，单文件不变。代价是慢一点，
//! 但我们只在**换地图**时跑一次（一小块 48×W 的图），几十到几百毫秒都可以接受。
//!
//! # 模型/字典从哪来
//!
//! `assets/ppocr_rec.onnx`（4.3MB）与 `assets/ppocr_dict.txt`（6906 项），
//! 来自 PaddleOCR 官方的 `PP-OCRv6_tiny_rec_onnx_infer`（Apache-2.0，免费）。
//! 打包进 exe：不联网、首次运行也不用下载。

use crate::exp::font::Pixels;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use tract_onnx::prelude::*;

/// 模型要求的输入高度（PP-OCR rec 固定 48）。
const INPUT_HEIGHT: usize = 48;
/// 最宽按 320 处理（PP-OCR 的惯例），再宽就压扁。
const MAX_INPUT_WIDTH: usize = 320;
/// CTC 空白类的下标。
const BLANK_INDEX: usize = 0;
/// 认出来的字太少/太多都当成没认出来（地名不会超过 12 个字）。
const MIN_CHARS: usize = 1;
const MAX_CHARS: usize = 12;

const MODEL_BYTES: &[u8] = include_bytes!("../../assets/ppocr_rec.onnx");
const DICT_TEXT: &str = include_str!("../../assets/ppocr_dict.txt");

/// 已优化、可直接跑的计划。`into_runnable()` 返回的是 `Arc<SimplePlan<..>>`。
type Model = Arc<TypedSimplePlan>;



/// 一次识别结果。
#[derive(Debug, Clone, PartialEq)]
pub struct Reading {
    pub text: String,
    /// 每个字的平均置信度（0~1）。太低就当没认出来。
    pub confidence: f32,
}

/// 字表：`blank + 字典 + 空格`，共 6906 项，和模型输出维度一一对应。
fn characters() -> &'static Vec<String> {
    static TABLE: OnceLock<Vec<String>> = OnceLock::new();
    TABLE.get_or_init(|| {
        let table: Vec<String> = DICT_TEXT.lines().map(|line| line.to_string()).collect();
        debug_assert_eq!(table.len(), 6906, "字典项数必须和模型输出维度一致");
        table
    })
}

/// 按宽度缓存已优化的模型。
///
/// `into_optimized()` 是一次性的图优化（几百毫秒到几秒），不能每帧都做；
/// 而输入宽度随地名长度变化（2~6 个字），所以按宽度缓存几份就够。
fn models() -> &'static Mutex<HashMap<usize, Model>> {
    static MODELS: OnceLock<Mutex<HashMap<usize, Model>>> = OnceLock::new();
    MODELS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn build_model(width: usize) -> TractResult<Model> {
    let mut cursor = std::io::Cursor::new(MODEL_BYTES);
    let model = tract_onnx::onnx().model_for_read(&mut cursor)?.into_typed()?;

    // 这个 ONNX 的输入是 `[DynamicDimension.0, 3, 48, DynamicDimension.1]`，
    // 而图里别的节点在导出时已经按**具体值**做过形状推断（比如某一层写死了 109），
    // 于是 tract 会报 `Impossible to unify Sym(DynamicDimension.0) with Val(1)`。
    // 所以必须把两个符号显式绑成具体值：批次 1、宽度按这一行实际算出来的宽度。
    let mut substitutions = HashMap::new();
    substitutions.insert(model.sym("DynamicDimension.0"), 1.into());
    substitutions.insert(model.sym("DynamicDimension.1"), width.into());
    let model = model.set_symbols(&substitutions)?;

    model
        .with_input_fact(0, f32::fact([1, 3, INPUT_HEIGHT, width]).into())?
        .into_optimized()?
        .into_runnable()
}

/// 经验行这种「一长串数字」的输入宽度上限：`427096[46.09%]` 在高 48 时约 420 宽，
/// 压进 320 会把字挤扁（数字间距缩到 0.75，`1` 和 `.` 最先糊掉）。
const MAX_WIDE_INPUT_WIDTH: usize = 768;
/// 宽行的输入宽度按这个粒度向上取整、右侧补底色：tract 按宽度编译计划
/// （每个宽度几百毫秒），数字位数一变宽度就变，不取整会每隔几帧重编译一次。
const WIDE_WIDTH_STEP: usize = 64;

/// 普通单行的输入宽度按这个粒度向上取整（右侧补底色）。
///
/// 真机日志：通用路径一帧里 OCR 五六块宽度各异的候选（34、47、52、61、119、147…），
/// 每个新宽度都要现编译一次计划（几百毫秒到几秒），一帧拖到 20 秒。取整之后
/// 320 以内最多 10 个宽度，编译一次就一直复用。
const WIDTH_STEP: usize = 32;

/// 等比缩放到高 48（宽度按 [`WIDTH_STEP`] 取整补底色），返回 `[1, 3, 48, W]` 的归一化输入。
fn prepare(pixels: &Pixels<'_>) -> (Vec<f32>, usize) {
    let content = ((INPUT_HEIGHT as f64 * pixels.width as f64 / pixels.height as f64).round()
        as usize)
        .clamp(1, MAX_INPUT_WIDTH);
    let width = (content.div_ceil(WIDTH_STEP) * WIDTH_STEP).min(MAX_INPUT_WIDTH);
    let buffer = prepare_scaled(pixels, content, width);
    (buffer, width)
}

/// 缩放到 `content_width × 48` 放在左边，右侧补到 `total_width`（补的是最右一列的平均底色）。
fn prepare_scaled(pixels: &Pixels<'_>, content_width: usize, total_width: usize) -> Vec<f32> {
    let width = total_width;
    // 双线性：比最近邻平滑，也贴近 PaddleOCR 训练时的预处理
    let mut buffer = vec![0f32; 3 * INPUT_HEIGHT * width];
    for y in 0..INPUT_HEIGHT {
        // 目标像素中心映射回源坐标
        let source_y = ((y as f64 + 0.5) * pixels.height as f64 / INPUT_HEIGHT as f64 - 0.5)
            .clamp(0.0, (pixels.height - 1) as f64);
        let y0 = source_y.floor() as usize;
        let y1 = (y0 + 1).min(pixels.height - 1);
        let fy = source_y - y0 as f64;
        let luma = |sx: usize, sy: usize| pixels.luma(sx, sy) as f64;
        // 补宽那一段用这一行最右一列的底色（不是纯黑/纯白：那会被认成一道竖线）
        let pad_value = luma(pixels.width - 1, y0) * (1.0 - fy) + luma(pixels.width - 1, y1) * fy;
        for x in 0..width {
            let value = if x >= content_width {
                pad_value
            } else {
                let source_x = ((x as f64 + 0.5) * pixels.width as f64 / content_width as f64
                    - 0.5)
                    .clamp(0.0, (pixels.width - 1) as f64);
                let x0 = source_x.floor() as usize;
                let x1 = (x0 + 1).min(pixels.width - 1);
                let fx = source_x - x0 as f64;
                let top = luma(x0, y0) * (1.0 - fx) + luma(x1, y0) * fx;
                let bottom = luma(x0, y1) * (1.0 - fx) + luma(x1, y1) * fx;
                top * (1.0 - fy) + bottom * fy
            };
            // 0~255 → [-1, 1]（PaddleOCR 就是 (x/255 - 0.5)/0.5）
            let normalized = (value / 255.0 - 0.5) / 0.5;
            for channel in 0..3 {
                buffer[channel * INPUT_HEIGHT * width + y * width + x] = normalized as f32;
            }
        }
    }
    buffer
}

/// 认一行字。认不出来返回 `None`（**不要**拿低置信度的结果去当地图名）。
pub fn recognize_line(pixels: &Pixels<'_>) -> Option<Reading> {
    // 高 4 就够：模型内部会把它等比缩放到 48 像素高，5px 的 768p 小字也能喂。
    // （旧实现的 8px 下限会把「缩放后只有 5~7 像素高」的经验行整条挡在门外。）
    if pixels.width < 8 || pixels.height < 4 {
        return None;
    }
    let (data, width) = prepare(pixels);
    run(&data, width)
}

/// 认**一长串**的单行（经验行）：不压扁，宽度按 64 取整补底色。
///
/// 和 [`recognize_line`] 的区别只在输入宽度：地图名 2~6 个字，320 足够；
/// 经验行十几个字符，压进 320 会把字距挤掉四分之一（见 [`MAX_WIDE_INPUT_WIDTH`]）。
pub fn recognize_wide_line(pixels: &Pixels<'_>) -> Option<Reading> {
    if pixels.width < 8 || pixels.height < 4 {
        return None;
    }
    let content = ((INPUT_HEIGHT as f64 * pixels.width as f64 / pixels.height as f64).round()
        as usize)
        .clamp(1, MAX_WIDE_INPUT_WIDTH);
    let width = (content.div_ceil(WIDE_WIDTH_STEP) * WIDE_WIDTH_STEP).min(MAX_WIDE_INPUT_WIDTH);
    let data = prepare_scaled(pixels, content, width);
    run(&data, width)
}

/// 跑一次模型（按宽度缓存编译好的计划）。
fn run(data: &[f32], width: usize) -> Option<Reading> {
    // 用 `Tensor::from_shape` 直接造张量，不依赖 ndarray（少一个依赖少一处坑）
    let tensor = Tensor::from_shape(&[1, 3, INPUT_HEIGHT, width], data).ok()?;

    let plan = {
        let mut cache = models().lock().ok()?;
        if !cache.contains_key(&width) {
            log::info!("文字识别：正在准备 {width} 宽的模型（只做一次）");
            let built = build_model(width).ok()?;
            cache.insert(width, built);
        }
        cache.get(&width)?.clone()
    };

    let outputs = plan.run(tvec!(tensor.into())).ok()?;
    let output = outputs.first()?;
    let view = output.to_plain_array_view::<f32>().ok()?;
    Some(decode(&view))
}

/// CTC 贪心解码：取每一帧的最大类 → 去掉相邻重复 → 去掉 blank。
///
/// 入参是形状 `[1, T, C]` 的视图（批次恒为 1）。
fn decode(view: &tract_ndarray::ArrayViewD<'_, f32>) -> Reading {
    let table = characters();
    let shape = view.shape();
    let (frames, classes) = (shape[1], shape[2]);
    let at = |frame: usize, class: usize| view[[0, frame, class]];
    let mut text = String::new();
    let mut confidences: Vec<f32> = Vec::new();
    let mut previous = usize::MAX;
    for frame in 0..frames {
        let mut best = 0usize;
        let mut best_score = f32::MIN;
        for class in 0..classes {
            let score = at(frame, class);
            if score > best_score {
                best_score = score;
                best = class;
            }
        }
        if best != previous && best != BLANK_INDEX && best < table.len() {
            text.push_str(&table[best]);
            confidences.push(best_score);
        }
        previous = best;
    }
    let confidence = if confidences.is_empty() {
        0.0
    } else {
        confidences.iter().sum::<f32>() / confidences.len() as f32
    };
    Reading { text, confidence }
}

/// 认出来的东西像不像一个地图名。
///
/// 光看置信度不够：模型对「一小块噪声」也可能给出高置信度的怪东西。
/// 这里再卡一道「字数」和「不能全是标点/字母」的常识。
pub fn looks_like_map_name(reading: &Reading) -> bool {
    let count = reading.text.chars().count();
    if count < MIN_CHARS || count > MAX_CHARS {
        return false;
    }
    if reading.confidence < 0.60 {
        return false;
    }
    let chinese = reading
        .text
        .chars()
        .filter(|ch| ('\u{4e00}'..='\u{9fff}').contains(ch))
        .count();
    // 地图名基本是汉字；允许夹杂少量数字/字母
    chinese * 2 >= count
}

/// 从 OCR 原始识别文本中解析出自洽的 `(exp, percent)`。
///
/// # 为什么需要容忍 OCR 的脏输出
///
/// 神经网络对小字号/抗锯齿文本识别时，常见标点漏读、空格穿插与近形字混淆：
/// - 空格穿插：`EXP 427 096 ( 46.09 % )`
/// - 全角标点：`EXP 427096（46.09%）`、`％`
/// - 常见形近字混淆：`O/o -> 0`、`l/I/| -> 1`、`S/s -> 5`、`Z/z -> 2`
/// - 标点丢失：漏读左括号 `(`（如 `EXP 427096 46.09%)`）、漏读 `%`（如 `427096(46.09)`）
/// - 漏读小数点：如 `427096(4609%)`（因冒险岛百分比固定两位小数，纯4位数字可还原为 46.09）
/// - 数字夹带千分位或误读点：`427,096` 或 `427.096`
/// - 左括号被读成数字：`427096146.09%`（真实帧上出现过，`(` → `1`）
///
/// # 安全防线
///
/// 解析出的 (exp, percent) **必须通过既有的 `table::validate` 自洽校验**才算有效：
/// `经验 ÷ 本级所需 ≈ 百分比`。只要错了一位数字几乎必然无法自洽，通不过即返回 None。
pub fn parse_exp_ocr(raw: &str) -> Option<(u64, f64)> {
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }

    // 1. 全角符号与常见 OCR 字符混淆标准化
    let mut normalized = String::with_capacity(raw.len());
    for ch in raw.chars() {
        match ch {
            '（' | '【' | '[' | '{' => normalized.push('('),
            '）' | '】' | ']' | '}' => normalized.push(')'),
            '％' => normalized.push('%'),
            'O' | 'o' => normalized.push('0'),
            'l' | 'I' | '|' => normalized.push('1'),
            'S' | 's' => normalized.push('5'),
            'Z' | 'z' => normalized.push('2'),
            _ => normalized.push(ch),
        }
    }

    parse_exp_ocr_strict(&normalized).or_else(|| parse_exp_ocr_by_splitting(&normalized))
}

/// 常规路径：按 `(整数.两位小数%)` 的结构切分（容忍空格 / 全角 / 脏符号）。
fn parse_exp_ocr_strict(normalized: &str) -> Option<(u64, f64)> {
    // 2. 切分「经验数字部分」与「百分比部分」
    //
    // 分隔符只认空白和冒号：**不认 `-`** —— 真机上 OCR 会把小数点读成 `-`
    // （`76.33` → `76-33`），拿它当分隔符会把百分比切成两半（真机帧：
    // `EXP707365/76-33%]` 必须解析成 exp=707365 / pct=76.33）。
    let (exp_raw, percent_raw) = if let Some(open_idx) = normalized.rfind('(') {
        let left = &normalized[..open_idx];
        let right = &normalized[open_idx + 1..];
        (left, right)
    } else if let Some(close_idx) = normalized.rfind(')') {
        let before_close = &normalized[..close_idx];
        let trimmed_before = before_close.trim_end();
        let split_pos = trimmed_before
            .rfind(|c: char| c.is_whitespace() || c == ':')
            .map(|i| i + 1)
            .unwrap_or_else(|| trimmed_before.len().saturating_sub(6));
        (&trimmed_before[..split_pos], &trimmed_before[split_pos..])
    } else if let Some(pct_idx) = normalized.rfind('%') {
        let before_pct = &normalized[..pct_idx];
        let trimmed_before = before_pct.trim_end();
        let split_pos = trimmed_before
            .rfind(|c: char| c.is_whitespace() || c == ':')
            .map(|i| i + 1)
            .unwrap_or_else(|| trimmed_before.len().saturating_sub(6));
        (&trimmed_before[..split_pos], &trimmed_before[split_pos..])
    } else {
        let trimmed = normalized.trim();
        let split_pos = trimmed.rfind(char::is_whitespace)?;
        (&trimmed[..split_pos], &trimmed[split_pos + 1..])
    };

    // 3. 提取经验纯数字（过滤 EXP 标签、逗号、点、空格）
    let exp_digits: String = exp_raw
        .chars()
        .filter(|c| c.is_ascii_digit())
        .collect();
    if exp_digits.is_empty() || exp_digits.len() > 10 {
        return None;
    }
    let exp = exp_digits.parse::<u64>().ok()?;

    // 4. 解析百分比
    //
    // 只看 `%` / `)` 之前的部分：后面跟着的东西（右括号被认成 `7` 之类）拼进来
    // 会变成 `46.707` 这种三位小数 —— 游戏里百分比永远是两位小数，多一位就是认错了。
    let percent_raw = percent_raw
        .split(['%', ')'])
        .next()
        .unwrap_or_default();
    let p_clean: String = percent_raw
        .chars()
        .filter(|c| c.is_ascii_digit() || *c == '.' || *c == ',')
        .map(|c| if c == ',' { '.' } else { c })
        .collect();
    if p_clean.is_empty() {
        return None;
    }
    if p_clean
        .split_once('.')
        .map(|(_, fraction)| fraction.len() > 2 || fraction.contains('.'))
        .unwrap_or(false)
    {
        return None;
    }

    let percent: f64 = if p_clean.contains('.') {
        p_clean.parse::<f64>().ok()?
    } else {
        // 漏读小数点的处理：游戏里百分比固定保留 2 位小数
        let val = p_clean.parse::<f64>().ok()?;
        if p_clean.len() >= 3 {
            val / 100.0
        } else {
            val
        }
    };

    if !percent.is_finite() || !(0.0..=100.0).contains(&percent) {
        return None;
    }

    // 5. 经验表强校验自检：只有完全与经验表自洽才放行
    match crate::exp::table::validate(exp, percent) {
        crate::exp::table::Verdict::Known(_) | crate::exp::table::Verdict::Ambiguous => {
            Some((exp, percent))
        }
        crate::exp::table::Verdict::Contradiction => None,
    }
}

/// 兜底路径：不依赖括号，直接在数字里枚举最合理的切分。
///
/// 用在「标点被读成数字」这类错误上：真机 OCR 把 `427096(46.09%)` 读成
/// `427096146.09%`（`(` → `1`）时，常规路径拿到的百分比是 146.09（非法）；
/// 枚举可以「丢掉中间被误读的一个数字」，得到 exp=427096 / pct=46.09，
/// 再交给经验表定夺（验证不过就继续枚举，全都不行就返回 None）。
///
/// **它不会降低安全性**：每一个候选都必须过 `table::validate` 的双数自洽，
/// 而错一位数字几乎必然对不上。
fn parse_exp_ocr_by_splitting(normalized: &str) -> Option<(u64, f64)> {
    let chars: Vec<char> = normalized.chars().collect();
    for dot in (0..chars.len()).rev() {
        // OCR 常把小数点读成 `-` / `,` / `·`；这几个都当分隔符试
        if !matches!(chars[dot], '.' | ',' | '-' | '·' | '\'') {
            continue;
        }
        if dot + 2 >= chars.len()
            || !chars[dot + 1].is_ascii_digit()
            || !chars[dot + 2].is_ascii_digit()
        {
            continue;
        }
        // 小数位后面还有数字：不是「两位小数」的百分比，跳过
        if dot + 3 < chars.len() && chars[dot + 3].is_ascii_digit() {
            continue;
        }
        let mut start = dot;
        while start > 0 && chars[start - 1].is_ascii_digit() {
            start -= 1;
        }
        let digits: String = chars[start..dot].iter().collect();
        if digits.len() < 4 {
            continue;
        }
        let frac: String = chars[dot + 1..dot + 3].iter().collect();
        for exp_len in 4..=digits.len().min(9) {
            // skip = 允许丢弃 exp 与百分比之间被误读的 0~2 个数字
            for skip in 0..=2usize {
                let pct_start = exp_len + skip;
                if pct_start >= digits.len() {
                    continue;
                }
                let pct_text = format!("{}.{}", &digits[pct_start..], frac);
                let Ok(percent) = pct_text.parse::<f64>() else {
                    continue;
                };
                if !(0.0..=100.0).contains(&percent) {
                    continue;
                }
                let Ok(exp) = digits[..exp_len].parse::<u64>() else {
                    continue;
                };
                if matches!(
                    crate::exp::table::validate(exp, percent),
                    crate::exp::table::Verdict::Known(_) | crate::exp::table::Verdict::Ambiguous
                ) {
                    return Some((exp, percent));
                }
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 字典必须和模型输出维度一一对应。
    ///
    /// 这一条是踩出来的：字典少一个 blank，所有下标整体偏一位，
    /// 读出来是「釜铸岢」这种乱码 —— 而「乱码」看起来很像「模型不行」，
    /// 会把人往错的方向带。
    #[test]
    fn the_table_matches_the_model_output() {
        let table = characters();
        assert_eq!(table.len(), 6906);
        assert_eq!(table[0], "blank");
        for probe in "坠落主义金银岛废弃都市蚂蚁洞".chars() {
            assert!(table.iter().any(|entry| entry == &probe.to_string()), "字典缺 {probe}");
        }
    }

    /// 验证 OCR 脏输出容忍与经验表强校验拦截。
    #[test]
    fn parse_exp_ocr_handles_various_dirty_outputs_and_enforces_validation() {
        // 1. 标准输出 (55 级: 427096 / 926689 = 46.09%)
        assert_eq!(parse_exp_ocr("EXP 427096(46.09%)"), Some((427_096, 46.09)));

        // 2. 空格穿插
        assert_eq!(parse_exp_ocr("EXP  427 096 ( 46.09 % )"), Some((427_096, 46.09)));

        // 3. 全角括号
        assert_eq!(parse_exp_ocr("EXP 427096（46.09%）"), Some((427_096, 46.09)));

        // 4. O/o 误当 0
        assert_eq!(parse_exp_ocr("EXP 427O96(46.o9%)"), Some((427_096, 46.09)));

        // 5. l/I 误当 1 (20 级: 12145 / 20216 = 60.08%)
        assert_eq!(parse_exp_ocr("EXP l2l45(60.08%)"), Some((12_145, 60.08)));
        assert_eq!(parse_exp_ocr("EXP I2I45(60.08%)"), Some((12_145, 60.08)));

        // 6. S/s 误当 5
        assert_eq!(parse_exp_ocr("EXP 1214S(60.08%)"), Some((12_145, 60.08)));

        // 7. 漏掉小数点 (4609 -> 46.09)
        assert_eq!(parse_exp_ocr("EXP 427096(4609%)"), Some((427_096, 46.09)));

        // 8. % 变为全角 ％ 或缺失
        assert_eq!(parse_exp_ocr("EXP 427096(46.09％)"), Some((427_096, 46.09)));
        assert_eq!(parse_exp_ocr("EXP 427096(46.09)"), Some((427_096, 46.09)));

        // 9. 缺失左括号 (
        assert_eq!(parse_exp_ocr("EXP 427096 46.09%)"), Some((427_096, 46.09)));

        // 10. 数字内夹带千分位逗号或点
        assert_eq!(parse_exp_ocr("EXP 427,096(46.09%)"), Some((427_096, 46.09)));
        assert_eq!(parse_exp_ocr("EXP 427.096(46.09%)"), Some((427_096, 46.09)));

        // 11. 左括号被读成数字（真机现场：「(」→「1」，百分比变成 146.09）
        assert_eq!(parse_exp_ocr("EXP 427096146.09%'"), Some((427_096, 46.09)));
        assert_eq!(parse_exp_ocr("FYD 427096146.09%'"), Some((427_096, 46.09)));

        // 11.5 **真机现场**：小数点被读成 `-`、左括号被读成 `/`
        //      （`EXP 707365(76.33%)` → `EXP707365/76-33%]`，55 级帧）
        assert_eq!(parse_exp_ocr("EXP707365/76-33%]"), Some((707_365, 76.33)));

        // 12. 反例（与经验表矛盾的错读必须被既有自检拦死，返回 None）
        assert_eq!(parse_exp_ocr("EXP 427196(46.09%)"), None); // 数字差100
        assert_eq!(parse_exp_ocr("EXP 999999(50.00%)"), None); // 凭空捏造
        assert_eq!(parse_exp_ocr("暂无在售"), None);
        assert_eq!(parse_exp_ocr(""), None);
        assert_eq!(parse_exp_ocr("1000000000(99.99%)"), None); // 位数超上限 / 表里无解
    }

    /// **和 onnxruntime 对齐**：同一份输入张量，tract 必须读出一模一样的结果。
    ///
    /// 素材是 Python 那边落盘的（`line1_input.f32` / `line1_expected.txt`），
    /// 所以这一条同时验证了「预处理一致」和「推理引擎一致」。
    ///
    /// ```text
    /// cd src-tauri && cargo test --lib -- --ignored tract_matches_onnxruntime --nocapture
    /// ```
    #[test]
    #[ignore = "需要 .tmp 里的对照素材（由 Python 侧导出）"]
    fn tract_matches_onnxruntime() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../.tmp");
        let raw = std::fs::read(root.join("line1_input.f32")).expect("读不到输入张量");
        let expected = std::fs::read_to_string(root.join("line1_expected.txt")).expect("读不到期望结果");
        let data: Vec<f32> = raw
            .chunks_exact(4)
            .map(|bytes| f32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
            .collect();
        let width = data.len() / (3 * INPUT_HEIGHT);
        println!("输入 {} 个 float，宽 {width}", data.len());

        let plan = match build_model(width) {
            Ok(plan) => plan,
            Err(error) => {
                // tract 的报错是链式的，Debug 才看得到根因（顶层只说「某个节点分析失败」）
                println!("tract 加载模型失败：{error:?}");
                for cause in error.chain() {
                    println!("  起因：{cause}");
                }
                panic!("tract 加载模型失败");
            }
        };
        let input = Tensor::from_shape(&[1, 3, INPUT_HEIGHT, width], &data).expect("形状不对");
        let outputs = plan.run(tvec!(input.into())).expect("推理失败");
        let output = outputs.first().expect("没有输出").clone();
        let view = output.to_plain_array_view::<f32>().unwrap();
        let reading = decode(&view);
        println!("tract 读出：{:?} 置信度 {:.3}", reading.text, reading.confidence);
        println!("onnxruntime 读出：{expected:?}");
        assert_eq!(reading.text, expected, "tract 和 onnxruntime 的结果必须一致");
    }

    /// 真机自检：抓小地图那两行，看能不能读出地名和地图名。
    #[test]
    #[ignore = "需要游戏正在运行，且人在角色里"]
    fn reads_the_real_map_name() {
        use crate::exp::capture;
        capture::ensure_dpi_aware();
        let Some(window) = capture::find_game_window() else {
            panic!("没找到游戏窗口");
        };
        // 上一行是地名，下一行是地图名
        for (label, x, y, w, h) in [("地名", 50, 27, 68, 15), ("地图名", 50, 43, 68, 16)] {
            let (frame, _) = capture::grab_client(window.hwnd, x, y, w, h).expect("抓不到");
            let reading = recognize_line(&frame.pixels());
            println!("{label}: {reading:?}");
        }
    }
}
