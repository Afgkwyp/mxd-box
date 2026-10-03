//! 约定检查：`MonitorManager` 的两把锁必须**先 `config`、后 `status`**。
//!
//! `mxdc_monitor.rs` 里同时存在两把 parking_lot 的 `RwLock`，持有者分布在两个
//! 不同的 OS 线程上：
//!
//! * `update_config` —— Tauri 的命令线程（设置页点「保存拉取间隔」时进来）；
//! * `refresh_once` —— tokio 的轮询任务（每 20–30 秒一次）。
//!
//! 它俩以前的加锁顺序正好相反（一个 `config.write → status.write`，
//! 另一个 `status.write → config.read`）。parking_lot 的写者排队会让后到的读者
//! 排在写者后面等，于是 A 等 B 的写锁、B 等 A 的写锁，**双向永久阻塞、不会自解**：
//! 开服监控从此不再采样且没有任何日志/界面提示，设置页保存转圈卡死，只能重启。
//!
//! 为什么是扫源码而不是写个并发测试去真复现：死锁要两个线程恰好交错在
//! 「一个已持 A 在等 B、另一个已持 B 在等 A」的那几微秒窗口里。要让它**确定性**发生，
//! 只能往生产代码里塞 barrier / hook 去注入同步点 —— 那样的测试守的是注入点、不是真代码，
//! 而不注入就只能靠 sleep 撞运气，跑慢了还假绿。所以这里学 `tests/ui_thread_callbacks.rs`
//! 的做法：文本级扫每个函数体，检查「同一次调用里要碰两把锁时，config 的获取位置
//! 必须全部排在 status 之前」。它不理解语义，但足以拦住这次真出现过的那种写法 ——
//! `scanner_catches_the_reversed_order` 就是这个自证。

use std::path::Path;

/// 唯一同时持有这两把锁的文件；别的地方只通过方法调用碰它们（字段是私有的）。
fn monitor_source() -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/mxdc_monitor.rs");
    std::fs::read_to_string(&path).unwrap_or_else(|err| panic!("读不到 {}: {err}", path.display()))
}

/// 把源码里所有带函数体的 `fn`（名字 + 花括号配对的函数体）取出来。
///
/// 和 `ui_thread_callbacks.rs` 一样按花括号配对，另外多挡两件事：
/// * 前缀看着像**注释**的 `fn `（`/// fn foo()`、` * fn foo()`）不算 —— 文档里
///   经常拿函数名举例；前缀是 `pub` / `async` 这类修饰词的要算（真代码基本都带），
/// * 签名里先碰到 `;` 就跳过（trait 方法声明没有函数体）。
fn function_bodies(source: &str) -> Vec<(String, String)> {
    let mut found = Vec::new();
    let mut from = 0;

    while let Some(offset) = source[from..].find("fn ") {
        let at = from + offset;
        from = at + "fn ".len();

        // 注释里的 `fn xxx()` 不算：行内前缀是 `//` 或 `*` 就跳过。
        // （不能要求 fn 在行首：真代码写的是 `pub async fn refresh_once`，
        // 卡行首会把要检查的那几个函数全漏掉。）
        let line_start = source[..at].rfind('\n').map(|i| i + 1).unwrap_or(0);
        let prefix = source[line_start..at].trim();
        if prefix.starts_with("//") || prefix.starts_with('*') || prefix.starts_with("/*") {
            continue;
        }

        let name: String = source[from..]
            .chars()
            .take_while(|ch| ch.is_alphanumeric() || *ch == '_')
            .collect();
        if name.is_empty() {
            continue;
        }

        // 签名扫到 `{`（有函数体）或 `;`（声明，跳过）
        let mut sig_at = from + name.len();
        let mut open = None;
        while let Some(byte) = source.as_bytes().get(sig_at) {
            match byte {
                b'{' => {
                    open = Some(sig_at);
                    break;
                }
                b';' => break,
                _ => sig_at += 1,
            }
        }
        let Some(open) = open else { continue };

        let mut depth = 0usize;
        let mut end = None;
        for (index, ch) in source[open..].char_indices() {
            match ch {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        end = Some(open + index);
                        break;
                    }
                }
                _ => {}
            }
        }

        match end {
            Some(end) => {
                found.push((name, source[open..=end].to_string()));
                from = end;
            }
            None => break,
        }
    }

    found
}

/// 这些模式出现的字节偏移。
///
/// 模式都带前导点：`self.last_status.read()` 里 `status` 前面是下划线，
/// 匹配不上 `.status.read(` —— 那把是另一把锁，不在这条约定的管辖范围内。
fn positions(body: &str, patterns: &[&str]) -> Vec<usize> {
    let mut hits = Vec::new();
    for pattern in patterns {
        let mut at = 0;
        while let Some(offset) = body[at..].find(*pattern) {
            let hit = at + offset;
            hits.push(hit);
            at = hit + pattern.len();
        }
    }
    hits.sort_unstable();
    hits
}

const CONFIG_LOCKS: [&str; 2] = [".config.read(", ".config.write("];
const STATUS_LOCKS: [&str; 2] = [".status.read(", ".status.write("];

/// 这个函数体违规吗：**同一次调用里先碰了 status、后碰 config**（含交错）。
fn violations_in(body: &str) -> bool {
    let config = positions(body, &CONFIG_LOCKS);
    let status = positions(body, &STATUS_LOCKS);
    if config.is_empty() || status.is_empty() {
        return false; // 只碰一把锁的函数不归这条规矩管
    }
    // 约定：config 的**每一次**获取都必须排在 status 之前。
    *config.last().unwrap() >= *status.first().unwrap()
}

fn violations(source: &str) -> Vec<String> {
    function_bodies(source)
        .into_iter()
        .filter(|(_, body)| violations_in(body))
        .map(|(name, _)| format!("fn {name}()"))
        .collect()
}

#[test]
fn monitor_locks_are_always_taken_config_before_status() {
    let source = monitor_source();
    let bodies = function_bodies(&source);

    // 先自检：要检查的那两个函数真的被取出来了 —— 否则下面那个空断言就是
    // 「因为没扫到而通过」，等于这道检查根本不存在。
    for expected in ["refresh_once", "update_config", "record_error"] {
        assert!(
            bodies.iter().any(|(name, _)| name == expected),
            "没扫到 fn {expected}()，扫描逻辑和源码结构对不上，这道检查已经失效"
        );
    }

    let bad: Vec<String> = bodies
        .iter()
        .filter(|(_, body)| violations_in(body))
        .map(|(name, _)| format!("fn {name}()"))
        .collect();
    assert!(
        bad.is_empty(),
        "mxdc_monitor.rs 里有函数先拿 status 的锁、再拿 config 的锁：{}。\n\
         加锁顺序必须是「先 config 后 status」—— 两个线程方向相反会互相等下去，\n\
         监控静默停摆 + 设置页保存卡死。约定与背景见 mxdc_monitor.rs 里 config 字段上的注释。",
        bad.join("、")
    );
}

/// 自证：评审里那对真实写法（原样摘出来），反序的必须被抓、正序的必须放行。
#[test]
fn scanner_catches_the_reversed_order() {
    // 出事前的 refresh_once：先 status.write，后 config.read
    let reversed = r#"
        pub async fn refresh_once(&self) {
            {
                let mut status = self.status.write();
                let config = self.config.read().clone();
                let _ = (status, config);
            }
        }
    "#;
    assert_eq!(
        violations(reversed),
        vec!["fn refresh_once()"],
        "status.write → config.read 必须被报出来"
    );

    // 修好之后：先读 config（持到写完 status），再拿 status 的写锁
    let fixed = r#"
        pub async fn refresh_once(&self) {
            {
                let config = self.config.read();
                let mut status = self.status.write();
                let _ = (status, config);
            }
        }
    "#;
    assert!(
        violations(fixed).is_empty(),
        "config 在前的写法不该被报出来"
    );

    // update_config 原本就是对的（config.write → status.write）
    let config_first = r#"
        pub fn update_config(&self) {
            let mut current = self.config.write();
            *current = MonitorConfig::default();
            let mut status = self.status.write();
            status.enabled = true;
        }
    "#;
    assert!(
        violations(config_first).is_empty(),
        "config.write 在前不该被报出来"
    );
}

/// 只碰一把锁的函数不归这条规矩管 —— 别把 `record_error` / `broadcast` 这类误伤了。
#[test]
fn scanner_ignores_single_lock_functions() {
    let single = r#"
        fn record_error(&self, err: &str) {
            let mut slot = self.last_error.write();
            *slot = Some(err.to_string());
            self.status.write().error = Some(err.to_string());
        }
        fn status(&self) -> MonitorStatus {
            self.status.read().clone()
        }
        fn config(&self) -> MonitorConfig {
            self.config.read().clone()
        }
    "#;
    assert!(
        violations(single).is_empty(),
        "只碰一把锁的函数不该被报出来"
    );
}
