//! 前端约定检查：**不要在界面里写 `<a href=...>` / `window.open`**。
//!
//! 用户报过一次：「主窗口左下角的小册子网站点击没有打开」。根因不是链接写错了，
//! 而是 Tauri 的 webview 里**没有「新窗口」这件事** —— `<a target="_blank">`
//! 点下去既不报错也不做任何事。整个界面上有三处这样的链接（主窗口页脚、查价
//! 悬浮窗页脚、基础配置的「访问源站」），全都是点了没反应。
//!
//! 正确的写法是 `invoke("open_external_url", { url })`（Rust 侧交给系统浏览器），
//! 页面地址统一放在 `src/lib/links.ts`。
//!
//! 检查方式很朴素（和 `ui_thread_callbacks.rs` 同一个套路）：先**去掉注释**
//! （因为我们正是在注释里写这句话提醒后来人的），再看代码里有没有
//! `<a ... href=` 或 `window.open(`。

use std::path::{Path, PathBuf};

/// 把 `/* ... */`、`{/* ... */}`（在前者之内）和 `// ...` 都换成空白。
///
/// 注意 `//` 要排除 `https://` 这种情况：冒号后面的双斜杠是 URL 的一部分。
fn strip_comments(source: &str) -> String {
    let chars: Vec<char> = source.chars().collect();
    let mut out = String::with_capacity(source.len());
    let mut index = 0;
    let mut in_line = false;
    let mut in_block = false;

    while index < chars.len() {
        let current = chars[index];
        let next = chars.get(index + 1).copied();

        if in_line {
            if current == '\n' {
                in_line = false;
                out.push('\n');
            } else {
                out.push(' ');
            }
            index += 1;
            continue;
        }

        if in_block {
            if current == '*' && next == Some('/') {
                in_block = false;
                out.push_str("  ");
                index += 2;
            } else {
                out.push(if current == '\n' { '\n' } else { ' ' });
                index += 1;
            }
            continue;
        }

        if current == '/' && next == Some('*') {
            in_block = true;
            out.push_str("  ");
            index += 2;
            continue;
        }

        if current == '/' && next == Some('/') {
            let previous = if index == 0 { ' ' } else { chars[index - 1] };
            if previous != ':' {
                in_line = true;
                out.push_str("  ");
                index += 2;
                continue;
            }
        }

        out.push(current);
        index += 1;
    }

    out
}

/// 返回源码里「用 `<a href>` 或 `window.open` 打开外部链接」的可疑片段。
fn violations_in(source: &str) -> Vec<String> {
    let code = strip_comments(source);
    let chars: Vec<char> = code.chars().collect();
    let mut found = Vec::new();

    // `<a` 后面 200 个字符内出现 `href=` 就算一个链接元素（属性可以换行写）
    for (index, window) in chars.windows(2).enumerate() {
        if window[0] == '<' && window[1] == 'a' {
            let after: String = chars[index..].iter().take(200).collect();
            if after.contains("href=") {
                let line = code[..code
                    .char_indices()
                    .nth(index)
                    .map(|(byte, _)| byte)
                    .unwrap_or(0)]
                    .matches('\n')
                    .count()
                    + 1;
                found.push(format!("第 {line} 行有一个 <a href=...> 链接"));
            }
        }
    }

    if code.contains("window.open(") {
        found.push("用了 window.open()".to_string());
    }

    found
}

fn frontend_sources() -> Vec<PathBuf> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("src");
    let mut files = Vec::new();
    let mut stack = vec![root];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if matches!(
                path.extension().and_then(|ext| ext.to_str()),
                Some("ts") | Some("tsx")
            ) {
                files.push(path);
            }
        }
    }
    files.sort();
    files
}

#[test]
fn external_links_go_through_the_backend_command() {
    let mut problems = Vec::new();

    for path in frontend_sources() {
        let Ok(source) = std::fs::read_to_string(&path) else {
            continue;
        };
        for violation in violations_in(&source) {
            problems.push(format!("{}：{violation}", path.display()));
        }
    }

    assert!(
        problems.is_empty(),
        "界面里不能直接用 `<a href>` / `window.open` —— Tauri 的 webview 没有「新窗口」，\
         点了不会有任何反应：\n{}\n\
         请改成 invoke(\"open_external_url\", {{ url }})，地址放在 src/lib/links.ts。",
        problems.join("\n")
    );
}

/// 自证：真写了链接会被抓出来，注释里提到它不会被误报。
#[test]
fn scanner_catches_links_but_ignores_comments() {
    let broken = r#"
        const Link = () => (
            <a
              href="https://mxdc.dvg.cn/"
              target="_blank"
              rel="noreferrer"
            >
              mxdc.dvg.cn
            </a>
        );
    "#;
    assert_eq!(violations_in(broken).len(), 1, "换行写的 <a href> 也要抓到");

    let inline = r#"<a href="https://x" target="_blank">x</a>"#;
    assert_eq!(violations_in(inline).len(), 1);

    let window_open = r#"const go = () => window.open("https://mxdc.dvg.cn/");"#;
    assert_eq!(violations_in(window_open).len(), 1);

    let fixed = r#"
        {/* `<a href>` 在 webview 里点不开，统一走后端命令 */}
        <button onClick={() => invoke("open_external_url", { url: MXDC_HOME })}>源站</button>
    "#;
    assert!(
        violations_in(fixed).is_empty(),
        "注释里提到 <a href> 不该被误报（这正是提醒后来人的地方）"
    );

    // `https://` 里的双斜杠不能被当成行注释
    let with_url = r#"const u = "https://mxdc.dvg.cn/"; const x = () => window.open(u);"#;
    assert_eq!(violations_in(with_url).len(), 1, "URL 后面的代码不能被注释规则吃掉");
}
