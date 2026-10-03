//! 等级经验表 + **非负账本**。
//!
//! ## 这张表是整套东西的地基
//!
//! 游戏在同一行里同时给了「本级经验」和「括号里的百分比」，两者必须满足
//! `经验 ÷ 本级所需 ≈ 百分比`。于是：
//!
//! * 读对了 → 能**唯一**确定等级（不读等级徽章也能知道等级，少一个失败点）；
//! * 读错了 → 几乎必然无解，当场判「这一帧不算数」，而不是把错数字喂进统计。
//!
//! 真机上验过的两个等级（各自精确成立）：
//!
//! | 读数 | 结论 |
//! | --- | --- |
//! | `427096(46.09%)` | 55 级（`926689`，`427096/926689 = 46.0887%`） |
//! | `12145(60.08%)` | 20 级（`20216`，`12145/20216 = 60.0812%`） |
//!
//! 索引约定：`EXP_TABLE[level - 1]` = 从 `level` 升到 `level + 1` 所需的经验。
//! 差一位会把 55 级说成 54 级，`index_off_by_one_is_pinned` 钉着它。
//!
//! ## 升级那一瞬间为什么不会出现负数
//!
//! 本级经验**每升一级就归零**，直接拿 `新经验 - 旧经验` 去算就是一个巨大的负数
//! （同类工具被吐槽的就是这个）。这里的做法是把 `(等级, 本级经验)` 换算成
//! **跨级单调的累计经验**，所有加减法都在累计值上做：
//!
//! ```text
//! 增量 = 累计(新等级, 新经验) - 累计(旧等级, 旧经验)      // 永远 ≥ 0
//! ```
//!
//! 升级、连升两级、跨整级自动对。而「这一帧到底发生了什么」被收成**一个函数**、
//! 几种出口（见 [`advance`] 与 [`Advance`]），每种出口的值都是非负的。

/// `EXP_TABLE[level - 1]` = 从 `level` 升到 `level + 1` 所需的经验。
pub const EXP_TABLE: [u64; 120] = [
    15, 34, 57, 92, 135, 372, 560, 840, 1242, 1716, 2360, 3216, 4200, 5460, 7050, 8840, 11040,
    13716, 16680, 20216, 24402, 28980, 34320, 40512, 47216, 54900, 63666, 73080, 83720, 95700,
    108480, 122760, 138666, 155540, 174216, 194832, 216600, 240500, 266682, 294216, 324240,
    356916, 391160, 428280, 468450, 510420, 555680, 604416, 655200, 709716, 748608, 789631,
    832902, 878545, 926689, 977471, 1031036, 1087536, 1147132, 1209994, 1276301, 1346242,
    1420016, 1497832, 1579913, 1666492, 1757815, 1854143, 1955750, 2062925, 2175973, 2295216,
    2420993, 2553663, 2693603, 2841212, 2996910, 3161140, 3334370, 3517093, 3709829, 3913127,
    4127566, 4353756, 4592341, 4844001, 5109452, 5389449, 5684790, 5996316, 6324914, 6671519,
    7037118, 7422752, 7829518, 8258575, 8711144, 9188514, 9692044, 10223168, 10783397, 11374327,
    11997640, 12655110, 13348610, 14080113, 14851703, 15665576, 16524049, 17429566, 18384706,
    19392187, 20454878, 21575805, 22758159, 24005306, 25320796, 26708375, 28171993, 29715818,
];

/// 升到下一级需要多少经验（`level` 从 1 开始）。
pub fn requirement(level: u32) -> Option<u64> {
    if level == 0 {
        return None;
    }
    EXP_TABLE.get((level - 1) as usize).copied()
}

/// 游戏里百分比显示两位小数，四舍五入带来的最大偏差就是 0.005；
/// 留一点余量（0.006）给「游戏到底是四舍五入还是截断」这个未知。
const PERCENT_TOLERANCE: f64 = 0.006;

/// 一次（经验, 百分比）读数交给表之后的结论。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// 唯一一个等级能同时满足这两个数 —— 这次读数可信，而且顺带知道了等级。
    Known(u32),
    /// 多个等级都能满足（例如经验恰好是 0 时所有等级都是 `0.00%`）：
    /// 读数本身不矛盾，但**定不出等级**。
    Ambiguous,
    /// 没有任何等级能同时满足 —— 这一帧读错了。
    Contradiction,
}

/// 用经验表校验一组读数。
pub fn validate(exp: u64, percent: f64) -> Verdict {
    if !percent.is_finite() || percent < 0.0 {
        return Verdict::Contradiction;
    }
    let mut hits: Vec<u32> = Vec::new();
    for (index, need) in EXP_TABLE.iter().enumerate() {
        let level = index as u32 + 1;
        // 经验超过本级所需是不可能的（够升级就升了），这种组合直接不算候选
        if exp > *need {
            continue;
        }
        let expected = exp as f64 / *need as f64 * 100.0;
        if (expected - percent).abs() <= PERCENT_TOLERANCE {
            hits.push(level);
            if hits.len() > 1 {
                return Verdict::Ambiguous;
            }
        }
    }
    match hits.first() {
        Some(level) => Verdict::Known(*level),
        None => Verdict::Contradiction,
    }
}

/// 距离升级还差多少经验（定不出等级时 `None`）。
pub fn remaining(level: u32, exp: u64) -> Option<u64> {
    requirement(level).map(|need| need.saturating_sub(exp))
}

/// 从「1 级 0 经验」到 `(level, exp)` 的**累计经验**。
///
/// 这是「升级不再出现负数」的全部秘密：本级经验每升一级归零，而累计值跨级单调，
/// 于是所有的加减法都变成普通减法，升级/连升两级/跨整级都自动对。
/// 累计值变小只可能意味着**换了角色**。
pub fn cumulative(level: u32, exp: u64) -> Option<u64> {
    if level == 0 || level as usize > EXP_TABLE.len() {
        return None;
    }
    let floor: u64 = EXP_TABLE[..(level as usize - 1)].iter().sum();
    Some(floor + exp)
}

/// 找一个能容纳 `exp` 的等级，从 `from` 一直扫到表尾。
///
/// 用在**重立基准**的场合：那时要回答的不是「有没有升级」，而是
/// 「这个经验值落在哪一级里」—— 必须给得出答案，否则基准就自相矛盾
/// （等级 1 却装着 3 万点经验，而 1 级只需 15 点）。这种自相矛盾的基准有多毒：
/// 下一帧从它出发，累计经验比谁都大，于是**之后每一帧都接不上**，
/// 统计从此一句数都不记 —— 一次任务奖励就能触发它。
pub fn level_holding(from: u32, exp: u64) -> Option<u32> {
    (from..=EXP_TABLE.len() as u32).find(|level| {
        requirement(*level)
            .map(|need| exp <= need)
            .unwrap_or(false)
    })
}

/// 一次采样里**最多可能**得到多少经验。
///
/// 判据只有一条常识：一帧（1~2 秒）里的增量不可能超过「升一级所需的全部经验」
/// （低等级时这个值太小，所以再给一个 100 万的兜底）。
///
/// 它挡的不是「打得快」，而是**认错了一位数字**：那种帧能过经验表校验
/// （数字和百分比自洽），和上一帧一比却是天文数字。
pub fn max_plausible_gain(level: Option<u32>) -> u64 {
    const FLOOR: u64 = 1_000_000;
    level
        .and_then(requirement)
        .map(|need| need.max(FLOOR))
        .unwrap_or(FLOOR)
}

/// 最多允许一次采样跨几级。低等级刷怪一帧连升两级是真的，三级已经很夸张；
/// 再多就不是升级，而是换了角色或者认错了。
pub const MAX_LEVEL_JUMP: u32 = 3;

/// 从上一帧推进到这一帧的结论。**每一种出口的数值都是非负的。**
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Advance {
    /// 同一个角色的正常推进。`gained` 保证 ≥ 0。
    Gained {
        level: u32,
        /// 这一帧**生效的**本级经验（调用方要拿它当下一帧的基准）
        exp: u64,
        gained: u64,
        /// 这一次跨了几级（0 = 没升级）
        level_ups: u32,
    },
    /// 同一级里经验变少了：**死亡掉经验**。
    ///
    /// 不算成负收益（那会把这一段拖下水），也不算成换角色（那会白丢一段），
    /// 而是如实报出来、把基准挪到新值上继续统计。
    Loss { level: u32, exp: u64, lost: u64 },
    /// 和上一帧接不上，但这一帧**自己**是自洽的。
    ///
    /// * `too_big = false`：换角色了；
    /// * `too_big = true`：增量大得不像真的 —— 这一帧能过经验表校验，
    ///   所以多半是「认错了一位数字」那种最危险的情况。
    ///
    /// 两种都**不计增量、把基准挪到当下**：不挪的话下一帧还是接不上，
    /// 于是一帧错之后就永远接不上（游戏不会退回去），统计永久停摆。
    Shifted { too_big: bool },
    /// 这一帧自己就自相矛盾 → **什么都不记**，而且基准不动。
    /// 下一帧接着按上一帧的好值算，不会因为一帧坏数据丢掉一段。
    Unclear,
}

/// 一种「这一帧到底发生了什么」的**合法解释**。
///
/// 升级、同级增长、死亡掉经验都会产出这个结构，然后放在一起比 ——
/// 而不是按 if/else 的先后顺序让某一种天然优先。
#[derive(Debug, Clone, Copy)]
struct Reading {
    level: u32,
    exp: u64,
    /// 净变化：正数是涨，负数是跌
    delta: i64,
    level_ups: u32,
}

/// 用「上一帧的（等级, 本级经验）」和「这一帧读到的（本级经验, 百分比）」推进一笔账。
///
/// ## 搜索规则
///
/// 候选等级**只从上一帧的等级往上找**（最多 [`MAX_LEVEL_JUMP`] 级），
/// 每个候选都必须同时满足：`经验 ≤ 本级所需`、百分比自洽、累计不倒退、
/// 增量不超过一帧的上限。
///
/// ## 升级和死亡会互相冒充，所以它们同台竞争
///
/// 同一份「上一帧 46.09% → 这一帧 0.44%」的读数，有两种都成立的故事：
/// 当成升到下一级要凭空多出 50 万经验，当成死亡只少了 42 万。
/// 让某一种天然优先都是错的 —— 一次死亡会被记成一笔凭空的 50 万经验
/// （用户看到的时速直接翻倍，而这一段的其它数字全是好的，根本查不出来）。
///
/// 所以三种解释放进同一张表里比，按**离预期增量最近**挑一个；
/// 拿不到预期值（刚开统计、还没攒出速率）时退化成「挑变化最小的那个」。
/// 挑选器**不改变任何解释的合法性**：它只在都合法的解释里选一个，
/// 永远不会让一个合法解释变成「不记」。
pub fn advance(
    prev_level: u32,
    prev_exp: u64,
    exp: u64,
    percent: f64,
    expected_gain: Option<f64>,
) -> Advance {
    let Some(before) = cumulative(prev_level, prev_exp) else {
        return Advance::Unclear;
    };
    // 上限跟着**上一帧的等级**取：跨级之后本级所需只会更大，用小的那个当闸门才是真的严
    let ceiling = max_plausible_gain(Some(prev_level));

    let mut options: Vec<Reading> = Vec::new();
    let mut blocked_by_ceiling = false;
    for level in prev_level..=(prev_level + MAX_LEVEL_JUMP) {
        let Some(need) = requirement(level) else {
            break; // 表到头了
        };
        let exp_here = exp;
        if exp_here > need {
            continue; // 够升级就升了，不可能停在这一级
        }
        if (exp_here as f64 / need as f64 * 100.0 - percent).abs() > PERCENT_TOLERANCE {
            continue; // 百分比对不上：认错了别的数字
        }
        let Some(after) = cumulative(level, exp_here) else {
            break;
        };
        if after < before {
            continue; // 累计倒退：不是升级
        }
        let gained = after - before;
        if gained > ceiling {
            blocked_by_ceiling = true;
            continue;
        }
        options.push(Reading {
            level,
            exp: exp_here,
            delta: gained as i64,
            level_ups: level - prev_level,
        });
        if options.len() >= MAX_LEVEL_JUMP as usize {
            break;
        }
    }

    // 读数正好落在 0(0.00%) 的**零点帧**：这一帧要单独处理（见本节末尾）。
    // 在零点上，「本级经验掉光」这个死亡解释几乎总是合法，而且 |Δ| 往往比
    // 「升到下一级从 0 起记」还小，于是拿不到预期值时 `pick` 必选它 ——
    // 真实升级被吞、等级卡住，还会弹一句「这一级里经验掉了 N（死亡？）」。
    let zero_boundary = exp == 0 && percent <= PERCENT_TOLERANCE;

    // 同一级里经验变少也是一种解释：死亡掉经验
    // （零点帧先不收，交给本节末尾按「离满级还有多远」判断）
    if !zero_boundary && exp < prev_exp {
        if let (Some(need), Some(after)) = (requirement(prev_level), cumulative(prev_level, exp)) {
            if after < before && (exp as f64 / need as f64 * 100.0 - percent).abs() <= PERCENT_TOLERANCE
            {
                options.push(Reading {
                    level: prev_level,
                    exp,
                    delta: -((before - after) as i64),
                    level_ups: 0,
                });
            }
        }
    }

    // **零点帧**：只有本级快满的时候，归零才是升级 ——
    // 升级前的最后一段会把本级填满、然后清零从下一级的 0 重新开始；
    // 离满级还远就突然归零，更像一次真死亡。
    //
    // 这条硬常识是这里唯一的判据，分界取本级所需约 90%：
    // 快满了就明确按升级解释（不再让 |Δ| 去比大小 —— 140 涨和 140 跌的
    // 绝对值一样大，比大小是掷硬币）；离得远就把死亡候选补回来，
    // 交回普通的 `pick` 流程如实报 Loss。
    //
    // 两种情况下都**没有取消任何合法解释**，只是在等价的解释之间定了先后。
    if zero_boundary {
        let near_full = requirement(prev_level)
            .map(|need| prev_exp >= need.saturating_mul(90) / 100)
            .unwrap_or(false);

        if near_full {
            if let Some(upgraded) = options.iter().find(|reading| reading.level > prev_level) {
                return Advance::Gained {
                    level: upgraded.level,
                    exp: upgraded.exp,
                    gained: upgraded.delta as u64,
                    level_ups: upgraded.level_ups,
                };
            }
        }

        // 没升级候选（或离满级还远）：把死亡候选补回来。
        if let (Some(need), Some(after)) = (requirement(prev_level), cumulative(prev_level, exp)) {
            if after < before && (exp as f64 / need as f64 * 100.0 - percent).abs() <= PERCENT_TOLERANCE
            {
                options.push(Reading {
                    level: prev_level,
                    exp,
                    delta: -((before - after) as i64),
                    level_ups: 0,
                });
            }
        }
    }

    if let Some(picked) = pick(&options, expected_gain) {
        return if picked.delta >= 0 {
            Advance::Gained {
                level: picked.level,
                exp: picked.exp,
                gained: picked.delta as u64,
                level_ups: picked.level_ups,
            }
        } else {
            Advance::Loss {
                level: picked.level,
                exp: picked.exp,
                lost: (-picked.delta) as u64,
            }
        };
    }
    if blocked_by_ceiling {
        return Advance::Shifted { too_big: true };
    }
    // 这一帧自己自洽吗（自洽 = 表和这两个数不矛盾）
    if validate(exp, percent) != Verdict::Contradiction {
        Advance::Shifted { too_big: false }
    } else {
        Advance::Unclear
    }
}

/// 在多个合法解释里挑一个：**离预期增量最近**；没有预期值就挑变化最小的。
fn pick(options: &[Reading], expected_gain: Option<f64>) -> Option<Reading> {
    let expected = expected_gain.filter(|value| value.is_finite() && *value > 0.0);
    options
        .iter()
        .min_by(|a, b| {
            let score = |reading: &Reading| match expected {
                Some(expected) => (reading.delta as f64 - expected).abs(),
                None => (reading.delta as f64).abs(),
            };
            score(a)
                .partial_cmp(&score(b))
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .copied()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn real_frames_resolve_to_their_levels() {
        // 55 级（旧机架上那一帧）
        assert_eq!(requirement(55), Some(926_689));
        assert_eq!(validate(427_096, 46.09), Verdict::Known(55));
        assert!((427_096f64 / 926_689f64 * 100.0 - 46.09).abs() < 0.006);
        // 20 级（2026-09-25 真机帧：12145 / 20216 = 60.08%）
        assert_eq!(requirement(20), Some(20_216));
        assert_eq!(validate(12_145, 60.08), Verdict::Known(20));
        assert!((12_145f64 / 20_216f64 * 100.0 - 60.08).abs() < 0.006);
    }

    /// 索引差一位就会把 55 级说成 54 级 —— 钉住它。
    #[test]
    fn index_off_by_one_is_pinned() {
        assert_eq!(EXP_TABLE[54], 926_689);
        assert_eq!(requirement(54), Some(878_545));
    }

    /// 把字形 `6` 认成 `8` 的那一帧：表里没有任何等级成立。
    #[test]
    fn a_mislabelled_frame_is_rejected() {
        assert_eq!(validate(427_098, 48.09), Verdict::Contradiction);
    }

    /// 经验恰好为 0 时所有等级都成立 → 只能判「定不出等级」，不能说它错。
    #[test]
    fn zero_exp_is_ambiguous_not_a_contradiction() {
        assert_eq!(validate(0, 0.0), Verdict::Ambiguous);
    }

    /// 升级那一帧：本级剩余 + 新一级已有的，**绝不是负数**。
    #[test]
    fn advance_counts_both_sides_of_a_level_up() {
        let need = requirement(55).unwrap();
        let percent = 50.0 / requirement(56).unwrap() as f64 * 100.0;
        assert_eq!(
            advance(55, need - 100, 50, percent, None),
            Advance::Gained { level: 56, exp: 50, gained: 150, level_ups: 1 },
        );
    }

    /// 一帧里连升两级（低等级真的会发生）。
    ///
    /// 注意百分比要按**游戏显示的两位小数**来给：新一级只有 7 点时游戏显示
    /// `0.02%`，而它在 21 级会显示 `0.03%` —— 差 0.01 已经超过容差 0.006，
    /// 所以这一步唯一可解。要是手写一个没舍入的 `0.0241`，两级的差距就落回容差里，
    /// 反而变成多解，测试就会证明错的东西。
    #[test]
    fn two_levels_in_one_sample_are_counted_exactly() {
        let need20 = requirement(20).unwrap();
        let need21 = requirement(21).unwrap();
        let percent = (7.0 / requirement(22).unwrap() as f64 * 10000.0).round() / 100.0;
        assert_eq!(percent, 0.02);
        let truth = 100 + need21 + 7;
        assert_eq!(
            advance(20, need20 - 100, 7, percent, None),
            Advance::Gained { level: 22, exp: 7, gained: truth, level_ups: 2 },
        );
    }

    /// 升级那一瞬游戏显示 `0.00%`，而 `0/0.00%` 在很多级都成立。
    /// 账本用「累计经验必须单调」把它收窄：上一帧在 55 级顶部时只有 56 级成立。
    #[test]
    fn zero_exp_at_a_level_up_resolves_to_the_next_level() {
        let need = requirement(55).unwrap();
        assert_eq!(
            advance(55, need - 100, 0, 0.0, None),
            Advance::Gained { level: 56, exp: 0, gained: 100, level_ups: 1 },
        );
    }

    /// **死亡和升级会互相冒充**：一次大跌要被判成死亡，不能凭空多出一大笔经验。
    #[test]
    fn a_big_drop_is_a_death_not_a_free_level_up() {
        for expected in [None, Some(2_000.0)] {
            match advance(55, 427_096, 4_077, 0.44, expected) {
                Advance::Loss { level, .. } => assert_eq!(level, 55),
                other => panic!("一次大跌被读成了 {other:?}"),
            }
        }
    }

    /// 反过来：真的升级不能被当成掉经验。
    #[test]
    fn a_real_level_up_is_read_as_a_level_up() {
        let need = requirement(55).unwrap();
        let new_exp = (0.50 / 100.0 * requirement(56).unwrap() as f64).round() as u64;
        assert_eq!(
            advance(55, need - 100, new_exp, 0.50, None),
            Advance::Gained { level: 56, exp: new_exp, gained: 100 + new_exp, level_ups: 1 },
        );
    }

    /// 死亡掉经验：如实报出来，不算成负收益，也不算成换角色。
    #[test]
    fn advance_reports_a_death_as_a_loss() {
        let need = requirement(55).unwrap();
        let percent = 850_000.0 / need as f64 * 100.0;
        assert_eq!(
            advance(55, 900_000, 850_000, percent, None),
            Advance::Loss { level: 55, exp: 850_000, lost: 50_000 },
        );
    }

    /// **升级到 0.00% 的极值帧不能被判成死亡**（P1 回归）。
    ///
    /// 上一帧本级只剩一点经验、这一帧读到 0(0.00%) 时，两种解释都合法：
    /// 死亡（本级掉光 −140）和升级（跨到 56 级从 0 起记 +140）。两者的 |Δ| 一样大，
    /// 拿不到预期值时旧 `pick` 靠比大小是掷硬币 —— 真升级被吞、等级卡住，
    /// 还弹「经验掉了（死亡？）」。现在按硬常识定先后：**只有本级快满时归零才是升级**。
    #[test]
    fn zero_percent_right_after_a_full_level_is_a_level_up_not_a_death() {
        let need = requirement(55).unwrap();

        // 上一帧剩 140 点（本级已满 90% 以上）→ 0(0.00%) 是升级，不该报 Loss
        assert_eq!(
            advance(55, need - 140, 0, 0.0, None),
            Advance::Gained {
                level: 56,
                exp: 0,
                gained: 140,
                level_ups: 1,
            },
        );

        // 给一个很小的预期值也必须稳：只按 |Δ − expected| 比，死亡 −140 反而更近，
        // 常识分界必须赢过它。
        assert_eq!(
            advance(55, need - 140, 0, 0.0, Some(150.0)),
            Advance::Gained {
                level: 56,
                exp: 0,
                gained: 140,
                level_ups: 1,
            },
        );

        // 反例钉住：本级还差得远时归零更像真死亡，照常报 Loss（分界不介入）
        let far = need / 10;
        assert_eq!(
            advance(55, far, 0, 0.0, None),
            Advance::Loss {
                level: 55,
                exp: 0,
                lost: far,
            },
        );
    }

    /// 认错了的一帧（百分比和任何等级都对不上）：判「说不清」，什么都不记。
    #[test]
    fn advance_refuses_a_frame_that_contradicts_the_table() {
        assert_eq!(advance(55, 427_096, 427_596, 4.61, None), Advance::Unclear);
        let need = requirement(55).unwrap();
        assert_eq!(advance(55, 1_000, need * 40, 99.99, None), Advance::Unclear);
    }

    /// 换角色：这一帧自己自洽，只是和上一帧接不上。
    #[test]
    fn advance_detects_a_character_switch() {
        let percent = 9_000.0 / requirement(20).unwrap() as f64 * 100.0;
        assert_eq!(
            advance(55, 427_096, 9_000, percent, None),
            Advance::Shifted { too_big: false },
        );
    }

    /// **核心不变量**：任何输入下增量都不可能为负、等级都不会倒退。
    ///
    /// 这是「升级后数据全部变成负数」那句话的机器证明。
    #[test]
    fn advance_never_produces_a_negative_number() {
        let mut checks = 0u32;
        for level in 1..EXP_TABLE.len() as u32 {
            let need = requirement(level).unwrap();
            for prev_exp in [0, 1, need / 3, need / 2, need - 2, need - 1, need] {
                for exp in [0, 1, need / 4, need - 1, need, need + 1] {
                    for percent in [0.0, 0.01, 1.0, 50.0, 99.99, 100.0] {
                        for expected in [None, Some(0.0), Some(1.0), Some(50_000.0), Some(1e12)] {
                            checks += 1;
                            match advance(level, prev_exp, exp, percent, expected) {
                                Advance::Gained { level: to, gained, level_ups, .. } => {
                                    assert!(gained <= max_plausible_gain(Some(level)));
                                    assert!(to >= level, "等级不能倒退：{level} → {to}");
                                    assert_eq!(level_ups, to - level);
                                    assert!(to - level <= MAX_LEVEL_JUMP);
                                }
                                Advance::Loss { lost, level: at, .. } => {
                                    assert!(lost > 0);
                                    assert_eq!(at, level);
                                }
                                Advance::Shifted { .. } | Advance::Unclear => {}
                            }
                        }
                    }
                }
            }
        }
        assert!(checks > 5_000, "覆盖太少了：{checks}");
    }

    /// 重立基准时等级必须**装得下**这个经验值 —— 找不到就继续往上扫。
    #[test]
    fn rebasing_always_finds_a_level_that_holds_the_exp() {
        assert_eq!(level_holding(1, 30_000), Some(23));
        for exp in [0u64, 15, 92, 1_020, 30_000, 29_715_817] {
            let level = level_holding(1, exp).expect("表里总能找到");
            assert!(exp <= requirement(level).unwrap(), "{exp} 点装不进 {level} 级");
        }
        assert_eq!(remaining(55, 427_096), Some(926_689 - 427_096));
        assert_eq!(remaining(121, 0), None);
    }
}
