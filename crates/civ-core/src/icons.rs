// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Resolving a ruleset object's name to a packed sprite.
//!
//! Unciv mostly names sprites after the object (`BuildingIcons/Alhambra`), but
//! three categories use fallback chains. Those chains live here rather than in
//! the renderer, because the server needs the same answers for tooltips and the
//! civilopedia, and a second copy would drift.

use crate::generated::assets::Sprite;

fn region(group: &str, name: &str) -> Option<Sprite> {
    Sprite::from_region(&format!("{group}/{name}"))
}

/// Split a promotion name the way `Promotion.getBaseNameAndLevel` does:
/// brackets dropped, a trailing ` I`/` II`/` III` read as a level.
///
/// `"Accuracy II"` -> `("Accuracy II", 2, "Accuracy")`.
pub fn promotion_base_and_level(name: &str) -> (String, u8, String) {
    let without_brackets: String = name.chars().filter(|c| *c != '[' && *c != ']').collect();
    let level = if without_brackets.ends_with(" III") {
        3
    } else if without_brackets.ends_with(" II") {
        2
    } else if without_brackets.ends_with(" I") {
        1
    } else {
        0
    };
    let base = if level == 0 {
        without_brackets.clone()
    } else {
        without_brackets[..without_brackets.len() - (level as usize + 1)].to_string()
    };
    (without_brackets, level, base)
}

/// `UnitPromotionIcons/<full>` → `UnitPromotionIcons/<base>` → `UnitIcons/<base>`.
pub fn promotion_icon(name: &str) -> Option<Sprite> {
    let (full, _, base) = promotion_base_and_level(name);
    region("UnitPromotionIcons", &full)
        .or_else(|| region("UnitPromotionIcons", &base))
        .or_else(|| region("UnitIcons", base.trim_end_matches(" ability")))
}

/// A branch and its `… Complete` capstone both resolve to the branch icon.
pub fn policy_icon(name: &str) -> Option<Sprite> {
    region("PolicyIcons", name)
        .or_else(|| region("PolicyBranchIcons", name))
        .or_else(|| name.strip_suffix(" Complete").and_then(|b| region("PolicyBranchIcons", b)))
}

/// Major civs get their own flag; city-states share one icon per type.
pub fn nation_icon(name: &str, city_state_type: Option<&str>) -> Option<Sprite> {
    region("NationIcons", name).or_else(|| city_state_type.and_then(|t| region("CityStateIcons", t)))
}

/// Last tier of every chain: `<Group>Icons/Fallback`, packed for a few groups only.
/// Portrait.kt ends here rather than rendering nothing.
pub fn fallback(group: &str) -> Option<Sprite> {
    region(group, "Fallback")
}

/// Total resolution: the dedicated icon, else the group's fallback.
/// `true` means the object has no art of its own.
pub fn promotion_icon_or_fallback(name: &str) -> Option<(Sprite, bool)> {
    match promotion_icon(name) {
        Some(s) => Some((s, false)),
        None => fallback("UnitPromotionIcons").map(|s| (s, true)),
    }
}

pub fn building_icon(name: &str) -> Option<Sprite> {
    region("BuildingIcons", name)
}
pub fn unit_icon(name: &str) -> Option<Sprite> {
    region("UnitIcons", name)
}
pub fn tech_icon(name: &str) -> Option<Sprite> {
    region("TechIcons", name)
}
pub fn resource_icon(name: &str) -> Option<Sprite> {
    region("ResourceIcons", name)
}
pub fn improvement_icon(name: &str) -> Option<Sprite> {
    region("ImprovementIcons", name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_levels_like_kotlin_does() {
        assert_eq!(promotion_base_and_level("Accuracy II"), ("Accuracy II".into(), 2, "Accuracy".into()));
        assert_eq!(promotion_base_and_level("Scouting"), ("Scouting".into(), 0, "Scouting".into()));
        assert_eq!(
            promotion_base_and_level("[Mounted] ability"),
            ("Mounted ability".into(), 0, "Mounted ability".into())
        );
    }

    #[test]
    fn levelled_promotions_share_one_icon() {
        assert_eq!(promotion_icon("Accuracy I"), promotion_icon("Accuracy III"));
        assert!(promotion_icon("Accuracy I").is_some());
    }

    #[test]
    fn icon_less_promotions_land_on_the_fallback() {
        let (_, is_fallback) = promotion_icon_or_fallback("Devout").expect("fallback is packed");
        assert!(is_fallback);
        let (_, is_fallback) = promotion_icon_or_fallback("Accuracy I").unwrap();
        assert!(!is_fallback);
    }

    #[test]
    fn branch_capstones_fall_back_to_the_branch() {
        assert_eq!(policy_icon("Tradition Complete"), policy_icon("Tradition"));
    }
}
