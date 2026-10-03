//! 经验报告：历史纵向对比与分地图效率聚合。
//!
//! # 两个不可违背的数据口径约定
//!
//! 1. **老记录的 `ended_at` 绝对不可信**：
//!    早期版本的会话结束结算存在 bug，曾把 `ended_at` 错误计算为 `started_at + active_secs`，
//!    把玩家中途暂停的所有时间全部抹杀。因此横向对比、时速折算一律基于真实的有效时间 `active_secs`
//!    和净增经验 `gained_exp` 这两个相对量，**绝不能拿 `ended_at - started_at` 算时长或画时间轴**。
//! 2. **历史记录的 `map_name` 可能为空**：
//!    `map_name` 是后续版本迁移加入的列，老数据中是空串或 NULL。未填地图的数据一律归类到
//!    `"未记录地图"`，保留其统计价值，不应作为坏数据丢弃。
//! 3. **不可信数据不参与对比**：
//!    仅统计 `quality > 0`（即「可参考」与「数据完整」）的会话；被标记为「不建议用于比较」的脏数据
//!    （画面丢失严重、数学拒绝帧过多）必须排除，否则会拉低整张地图的均值。

use serde::Serialize;
use std::collections::HashMap;

/// 默认历史会话提取条数。
const DEFAULT_RECENT_LIMIT: u32 = 20;
/// 最短有效采样时间（秒）：不足 1 分钟的时速属于纯高频噪声，不予折算。
const MIN_RATE_SECS: f64 = 60.0;

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ExpReport {
    pub by_map: Vec<MapStat>,
    pub recent: Vec<SessionStat>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct MapStat {
    /// 地图名称（为空时归入 "未记录地图"）
    pub map_name: String,
    /// 该地图上完成的有效会话总数
    pub sessions: u32,
    /// 累计有效时长（秒）
    pub total_active_secs: f64,
    /// 累计获得经验
    pub total_gained: u64,
    /// 该地图的历史综合平均时速（净经验 ÷ 净时长）
    pub per_hour: Option<f64>,
    /// 该地图上录得的最佳单次会话时速
    pub best_per_hour: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SessionStat {
    pub id: i64,
    pub started_unix: i64,
    pub map_name: String,
    pub active_secs: f64,
    pub gained_exp: u64,
    pub start_level: Option<u32>,
    pub end_level: Option<u32>,
    pub per_hour: Option<f64>,
    /// 与同地图历史均值之差（正 = 比平时快，负 = 比平时慢）
    pub per_hour_delta_vs_map: Option<f64>,
}

/// 数据库中读取的单条历史记录原始映射。
#[derive(Debug, Clone)]
pub struct SessionRecord {
    pub id: i64,
    pub started_at: i64,
    pub active_secs: f64,
    pub start_level: Option<u32>,
    pub end_level: Option<u32>,
    pub gained_exp: u64,
    pub quality: u8,
    pub map_name: String,
}

/// 时速换算：不足 60 秒或零经验均返回 None。
pub fn calculate_per_hour(active_secs: f64, gained_exp: u64) -> Option<f64> {
    if !active_secs.is_finite() || active_secs < MIN_RATE_SECS || gained_exp == 0 {
        None
    } else {
        Some(gained_exp as f64 * 3600.0 / active_secs)
    }
}

/// 地图名规范化：空串或纯空白归为 "未记录地图"。
pub fn normalize_map_name(raw: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        "未记录地图".to_string()
    } else {
        trimmed.to_string()
    }
}

/// 纯函数：根据会话记录列表构建纵向对比报告。
pub fn generate_report(records: &[SessionRecord], limit: u32) -> ExpReport {
    let effective_limit = if limit == 0 {
        DEFAULT_RECENT_LIMIT as usize
    } else {
        limit as usize
    };

    // 1. 过滤质量不可信的数据（quality == 0 排除）
    let trusted_records: Vec<&SessionRecord> = records
        .iter()
        .filter(|r| r.quality > 0)
        .collect();

    // 2. 按地图分组统计
    struct MapAccumulator {
        map_name: String,
        sessions: u32,
        total_active_secs: f64,
        total_gained: u64,
        best_per_hour: Option<f64>,
    }

    let mut map_groups: HashMap<String, MapAccumulator> = HashMap::new();

    for r in &trusted_records {
        let name = normalize_map_name(&r.map_name);
        let session_rate = calculate_per_hour(r.active_secs, r.gained_exp);

        let entry = map_groups.entry(name.clone()).or_insert_with(|| MapAccumulator {
            map_name: name,
            sessions: 0,
            total_active_secs: 0.0,
            total_gained: 0,
            best_per_hour: None,
        });

        entry.sessions += 1;
        entry.total_active_secs += r.active_secs;
        entry.total_gained += r.gained_exp;

        if let Some(rate) = session_rate {
            entry.best_per_hour = match entry.best_per_hour {
                Some(current) => Some(current.max(rate)),
                None => Some(rate),
            };
        }
    }

    // 3. 构建 MapStat 列表并按平均时速倒序排序
    let mut by_map: Vec<MapStat> = map_groups
        .values()
        .map(|acc| {
            let per_hour = calculate_per_hour(acc.total_active_secs, acc.total_gained);
            MapStat {
                map_name: acc.map_name.clone(),
                sessions: acc.sessions,
                total_active_secs: acc.total_active_secs,
                total_gained: acc.total_gained,
                per_hour,
                best_per_hour: acc.best_per_hour,
            }
        })
        .collect();

    by_map.sort_by(|a, b| {
        match (b.per_hour, a.per_hour) {
            (Some(rate_b), Some(rate_a)) => {
                rate_b.partial_cmp(&rate_a).unwrap_or(std::cmp::Ordering::Equal)
            }
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => b.total_gained.cmp(&a.total_gained),
        }
    });

    // 4. 构建最近会话列表（按 started_at DESC 截取 limit 条）
    let recent: Vec<SessionStat> = trusted_records
        .iter()
        .take(effective_limit)
        .map(|r| {
            let map_name = normalize_map_name(&r.map_name);
            let session_rate = calculate_per_hour(r.active_secs, r.gained_exp);

            // 差值口径（排除自身，且地图有效会话数 >= 2）：
            // 绝不能拿「本段时速 - 含本段在内的总均值」去比：
            // 1. 若地图仅有 1 段，总均值就是自己，差值恒为 0，会误导用户显示「与平时持平」；
            // 2. 若地图只有 2 段，把自身算进分母会把真实差值凭空稀释一半（(n-1)/n）；
            // 因此必须从地图总量中扣除本段的 (active_secs, gained_exp)，求出其余历史段的平均时速；
            // 且当同图可信会话少于 2 段时直接返回 None（表示「暂无基准历史」），不拿假 0 糊弄用户。
            let per_hour_delta_vs_map = match (session_rate, map_groups.get(&map_name)) {
                (Some(sr), Some(acc)) if acc.sessions >= 2 => {
                    let other_secs = (acc.total_active_secs - r.active_secs).max(0.0);
                    let other_gained = acc.total_gained.saturating_sub(r.gained_exp);
                    calculate_per_hour(other_secs, other_gained).map(|baseline| sr - baseline)
                }
                _ => None,
            };

            SessionStat {
                id: r.id,
                started_unix: r.started_at,
                map_name,
                active_secs: r.active_secs,
                gained_exp: r.gained_exp,
                start_level: r.start_level,
                end_level: r.end_level,
                per_hour: session_rate,
                per_hour_delta_vs_map,
            }
        })
        .collect();

    ExpReport { by_map, recent }
}

/// 执行只读查询并生成经验对比报告。
///
/// 调用方是 `commands.rs` 的 `exp_report` 命令（它只做转发，业务都在这里）。
pub fn build(db: &crate::db::Database, limit: u32) -> Result<ExpReport, String> {
    let conn = db.conn.lock();
    let mut stmt = conn
        .prepare(
            "SELECT id, started_at, active_secs, start_level, end_level, gained_exp, quality, map_name \
             FROM exp_sessions \
             WHERE quality > 0 \
             ORDER BY started_at DESC",
        )
        .map_err(|e| format!("查询历史会话失败: {e}"))?;

    let rows = stmt
        .query_map([], |row| {
            Ok(SessionRecord {
                id: row.get(0)?,
                started_at: row.get(1)?,
                active_secs: row.get(2)?,
                start_level: row.get(3)?,
                end_level: row.get(4)?,
                gained_exp: row.get(5)?,
                quality: row.get(6)?,
                map_name: row.get(7).unwrap_or_default(),
            })
        })
        .map_err(|e| format!("读取历史会话数据失败: {e}"))?;

    let mut records = Vec::new();
    for row in rows {
        if let Ok(rec) = row {
            records.push(rec);
        }
    }

    Ok(generate_report(&records, limit))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn calculate_per_hour_enforces_minimum_window_and_non_zero_gain() {
        // 短于 60 秒为噪声，不折算
        assert_eq!(calculate_per_hour(59.9, 5000), None);
        // 0 经验无时速
        assert_eq!(calculate_per_hour(120.0, 0), None);
        // 正常折算：120 秒获得 10,000 经验 -> 300,000 / 时
        let rate = calculate_per_hour(120.0, 10_000).expect("应成功计算时速");
        assert!((rate - 300_000.0).abs() < 1e-6);
        // 异常浮点防护
        assert_eq!(calculate_per_hour(f64::NAN, 1000), None);
        assert_eq!(calculate_per_hour(-10.0, 1000), None);
    }

    #[test]
    fn normalize_map_name_handles_empty_and_whitespace() {
        assert_eq!(normalize_map_name(""), "未记录地图");
        assert_eq!(normalize_map_name("   "), "未记录地图");
        assert_eq!(normalize_map_name(" 蚂蚁洞 "), "蚂蚁洞");
    }

    #[test]
    fn generate_report_filters_untrusted_and_sorts_by_rate() {
        let records = vec![
            // 会话 1：蚂蚁洞，耗时 120s，涨 20,000（时速 600,000）
            SessionRecord {
                id: 1,
                started_at: 1000,
                active_secs: 120.0,
                start_level: Some(25),
                end_level: Some(25),
                gained_exp: 20_000,
                quality: 2,
                map_name: "蚂蚁洞".to_string(),
            },
            // 会话 2：蚂蚁洞，耗时 120s，涨 10,000（时速 300,000）
            SessionRecord {
                id: 2,
                started_at: 2000,
                active_secs: 120.0,
                start_level: Some(25),
                end_level: Some(26),
                gained_exp: 10_000,
                quality: 1,
                map_name: "蚂蚁洞".to_string(),
            },
            // 会话 3：废弃都市，耗时 120s，涨 30,000（时速 900,000）
            SessionRecord {
                id: 3,
                started_at: 3000,
                active_secs: 120.0,
                start_level: Some(26),
                end_level: Some(26),
                gained_exp: 30_000,
                quality: 2,
                map_name: "废弃都市".to_string(),
            },
            // 会话 4：质量为 0（不可信），必须被彻底排除！
            SessionRecord {
                id: 4,
                started_at: 4000,
                active_secs: 120.0,
                start_level: Some(26),
                end_level: Some(26),
                gained_exp: 999_999,
                quality: 0,
                map_name: "废弃都市".to_string(),
            },
            // 会话 5：老记录未填地图名（归入 "未记录地图"）
            SessionRecord {
                id: 5,
                started_at: 5000,
                active_secs: 120.0,
                start_level: Some(20),
                end_level: Some(20),
                gained_exp: 5_000,
                quality: 2,
                map_name: "".to_string(),
            },
        ];

        let report = generate_report(&records, 10);

        // 验证地图排序：废弃都市 (900k) > 蚂蚁洞 (450k) > 未记录地图 (150k)
        assert_eq!(report.by_map.len(), 3);
        assert_eq!(report.by_map[0].map_name, "废弃都市");
        assert_eq!(report.by_map[0].per_hour, Some(900_000.0));
        assert_eq!(report.by_map[0].best_per_hour, Some(900_000.0));
        assert_eq!(report.by_map[0].sessions, 1);

        assert_eq!(report.by_map[1].map_name, "蚂蚁洞");
        // 蚂蚁洞均值：(20,000 + 10,000) * 3600 / 240 = 450,000
        assert_eq!(report.by_map[1].per_hour, Some(450_000.0));
        // 蚂蚁洞最好一段：600,000
        assert_eq!(report.by_map[1].best_per_hour, Some(600_000.0));
        assert_eq!(report.by_map[1].sessions, 2);

        assert_eq!(report.by_map[2].map_name, "未记录地图");
        assert_eq!(report.by_map[2].per_hour, Some(150_000.0));
        assert_eq!(report.by_map[2].sessions, 1);

        // 验证 recent 列表：已排除不可信会话 4，按输入顺序展示
        assert_eq!(report.recent.len(), 4);
        assert!(report.recent.iter().all(|s| s.id != 4));

        // 验证与同图均值之差（扣除自身）：
        // 蚂蚁洞有 2 段（600k 与 300k）：
        // - 会话 1（600k）：扣除自身后的历史基准是会话 2（300k），差值为 600,000 - 300,000 = +300,000（比平时快）
        let s1 = report.recent.iter().find(|s| s.id == 1).unwrap();
        assert!((s1.per_hour_delta_vs_map.unwrap() - 300_000.0).abs() < 1e-6);

        // - 会话 2（300k）：扣除自身后的历史基准是会话 1（600k），差值为 300,000 - 600,000 = -300,000（比平时慢）
        let s2 = report.recent.iter().find(|s| s.id == 2).unwrap();
        assert!((s2.per_hour_delta_vs_map.unwrap() - (-300_000.0)).abs() < 1e-6);

        // 废弃都市仅有 1 段：无对照历史，必须为 None，绝不显示假 0
        let s3 = report.recent.iter().find(|s| s.id == 3).unwrap();
        assert_eq!(s3.per_hour_delta_vs_map, None);

        // 未记录地图仅有 1 段：同样为 None
        let s5 = report.recent.iter().find(|s| s.id == 5).unwrap();
        assert_eq!(s5.per_hour_delta_vs_map, None);
    }

    /// 差值计算必须扣除自身，且单次会话地图必须返回 None（P2 回归测试）。
    #[test]
    fn delta_excludes_self_and_requires_at_least_two_sessions() {
        let solo = vec![SessionRecord {
            id: 1,
            started_at: 1000,
            active_secs: 120.0,
            start_level: Some(20),
            end_level: Some(20),
            gained_exp: 10_000,
            quality: 2,
            map_name: "独享地图".to_string(),
        }];
        let report_solo = generate_report(&solo, 10);
        assert_eq!(
            report_solo.recent[0].per_hour_delta_vs_map, None,
            "仅有 1 段会话的地图没有可比历史，delta 必须为 None，不可假冒为 0"
        );

        let duo = vec![
            SessionRecord {
                id: 1,
                started_at: 1000,
                active_secs: 120.0,
                start_level: Some(20),
                end_level: Some(20),
                gained_exp: 20_000, // 600k / 时
                quality: 2,
                map_name: "双人地图".to_string(),
            },
            SessionRecord {
                id: 2,
                started_at: 2000,
                active_secs: 120.0,
                start_level: Some(20),
                end_level: Some(20),
                gained_exp: 10_000, // 300k / 时
                quality: 2,
                map_name: "双人地图".to_string(),
            },
        ];
        let report_duo = generate_report(&duo, 10);
        let s1 = report_duo.recent.iter().find(|s| s.id == 1).unwrap();
        let s2 = report_duo.recent.iter().find(|s| s.id == 2).unwrap();
        assert!((s1.per_hour_delta_vs_map.unwrap() - 300_000.0).abs() < 1e-6);
        assert!((s2.per_hour_delta_vs_map.unwrap() - (-300_000.0)).abs() < 1e-6);
    }

    #[test]
    fn generate_report_honours_limit() {
        let records: Vec<SessionRecord> = (0..10)
            .map(|i| SessionRecord {
                id: i,
                started_at: 1000 + i,
                active_secs: 100.0,
                start_level: Some(30),
                end_level: Some(30),
                gained_exp: 1000,
                quality: 2,
                map_name: "测试地图".to_string(),
            })
            .collect();

        let report = generate_report(&records, 3);
        assert_eq!(report.recent.len(), 3);
        assert_eq!(report.by_map[0].sessions, 10, "by_map 应包含全量可信会话");
    }
}
