//! 内置的 BUFF 技能图标（来自小册子「技能图鉴」）。
//!
//! `assets/buff-skill-icons.json` 是从 `https://mxdc.dvg.cn/api/skill-list.php` 拉下来的：
//! 类型为「增益」的技能，加上说明里写了持续时间的「主动」技能，去掉停用的；
//! 同名同图的（比如各职业的「冒险岛勇士」）合成一条，职业名拼在一起。
//! 图标都是 32×32 的游戏原图，整份约 120 KB，所以直接打包进程序、不联网。
//!
//! 这些图标是原生 32×32。识别前画面会先按界面倍数缩回原生像素（见 `scale.rs`），
//! 模板不用再缩放。

use serde::{Deserialize, Serialize};
use std::sync::OnceLock;

/// 一个可选的技能。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillIcon {
    pub id: String,
    pub name: String,
    /// 会这个技能的职业（多个用 ` / ` 隔开）
    pub job: String,
    /// 冒险家 / 骑士团 / 英雄职业 / 通用
    pub group: String,
    /// 32×32 PNG 的 base64
    pub png: String,
}

#[derive(Deserialize)]
struct Catalog {
    skills: Vec<SkillIcon>,
}

pub fn skills() -> &'static [SkillIcon] {
    static SKILLS: OnceLock<Vec<SkillIcon>> = OnceLock::new();
    SKILLS.get_or_init(|| {
        serde_json::from_str::<Catalog>(include_str!("../../assets/buff-skill-icons.json"))
            .map(|c| c.skills)
            .unwrap_or_else(|err| {
                log::error!("内置技能图标解析失败：{err}");
                Vec::new()
            })
    })
}

pub fn find(id: &str) -> Option<&'static SkillIcon> {
    skills().iter().find(|s| s.id == id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_icons_decode_as_32px_templates() {
        let all = skills();
        assert!(all.len() > 50, "内置图标太少：{}", all.len());
        for skill in all {
            let (rgba, w, h) = super::super::decode_png(&skill.png).expect(&skill.name);
            assert_eq!((w, h), (32, 32), "{}", skill.name);
            let (gray, opaque) = super::super::image::Gray::from_rgba(&rgba, w, h);
            assert!(
                super::super::matcher::IconTemplate::new(&gray, &opaque).is_some(),
                "「{}」做不成模板",
                skill.name
            );
        }
        assert!(find("2301004").is_some_and(|s| s.name == "祝福"));
    }

    /// 真图标放大 1.5 倍贴进画面：只有它自己（或图案相同的条目）认得出来，别的内置图标都不认。
    #[test]
    fn a_real_icon_is_not_mistaken_for_another_one() {
        use super::super::image::{resize, Gray};
        use super::super::matcher::{tests::scene, FramePyramid, IconTemplate};
        let decode = |s: &SkillIcon| {
            let (rgba, w, h) = super::super::decode_png(&s.png).unwrap();
            Gray::from_rgba(&rgba, w, h)
        };
        let target = find("2301004").unwrap();
        let (gray, _) = decode(target);
        let frame = FramePyramid::new(scene(420, 150, &resize(&gray, 48, 48), 211, 37));
        for skill in skills() {
            let (g, opaque) = decode(skill);
            let hit = IconTemplate::scaled(&g, &opaque, 1.5).unwrap().discover(&frame);
            if skill.png == target.png {
                let hit = hit.expect("自己要认得出来");
                assert!(hit.x.abs_diff(211) <= 1 && hit.y.abs_diff(37) <= 1, "{hit:?}");
            } else {
                assert!(hit.is_none(), "「{}」被认成了「{}」：{hit:?}", target.name, skill.name);
            }
        }
    }
}
