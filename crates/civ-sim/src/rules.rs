//! Static ruleset tables, sent to a client once. Ids are registry indices, matching every id in a view.

use civ_core::ruleset::Ruleset;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[derive(Debug, Clone, Serialize, Deserialize, TS, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RuleTables {
    pub terrains: Vec<TerrainRow>,
    pub units: Vec<UnitRow>,
    pub nations: Vec<NationRow>,
    pub resources: Vec<ResourceRow>,
    pub improvements: Vec<ImprovementRow>,
    pub buildings: Vec<BuildingRow>,
    pub technologies: Vec<TechRow>,
    pub victories: Vec<Named>,
    pub policies: Vec<Named>,
    pub promotions: Vec<Named>,
    pub specialists: Vec<Named>,
    pub beliefs: Vec<BeliefRow>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TerrainRow {
    pub id: u32,
    pub name: String,
    /// `Land`, `Water`, `TerrainFeature` or `NaturalWonder`.
    pub kind: String,
    pub impassable: bool,
    /// Movement points to enter; features add theirs to the base terrain's.
    pub movement_cost: i32,
    /// Base yields; a feature with `overrideStats` replaces the base terrain's.
    pub yields: Yields,
    pub override_stats: bool,
}

/// A stats object's six tile yields.
#[derive(Debug, Clone, Default, Serialize, Deserialize, TS, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Yields {
    pub food: i32,
    pub production: i32,
    pub gold: i32,
    pub science: i32,
    pub culture: i32,
    pub faith: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ResourceRow {
    pub id: u32,
    pub name: String,
    /// `Strategic`, `Luxury` or `Bonus`.
    pub kind: String,
    pub yields: Yields,
    /// The improvement that works it, by name.
    pub improvement: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ImprovementRow {
    pub id: u32,
    pub name: String,
    pub yields: Yields,
    pub turns_to_build: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct UnitRow {
    pub id: u32,
    pub name: String,
    pub unit_type: String,
    pub movement: i32,
    /// Melee strength; 0 for civilians.
    pub strength: i32,
    /// 0 for units without a ranged attack.
    pub ranged_strength: i32,
    pub range: i32,
    /// Base production cost; 0 when it cannot be built.
    pub cost: i32,
    /// The nation whose unique unit this is, by name.
    pub unique_to: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct BuildingRow {
    pub id: u32,
    pub name: String,
    /// Base production cost; 0 when it cannot be built.
    pub cost: i32,
    pub wonder: bool,
    pub national_wonder: bool,
    /// Flat yields the building adds to its city.
    pub yields: Yields,
    pub happiness: i32,
    /// Gold a turn to keep it.
    pub maintenance: i32,
    /// Percentage bonuses by stat name, as `Growth` or `Science`.
    pub percent: Vec<(String, i32)>,
    /// Its rule text, one line per unique, in the ruleset's words.
    pub effects: Vec<String>,
    /// The building it needs in the city first, by name.
    pub requires: Option<String>,
}

/// A technology's place in the tree (Unciv's column and row) and what it opens.
#[derive(Debug, Clone, Serialize, Deserialize, TS, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TechRow {
    pub id: u32,
    pub name: String,
    pub era: String,
    pub column: i32,
    pub row: i32,
    /// Base science cost before map and speed scaling; the real cost of a choice is in `civOptions`.
    pub cost: i32,
    pub prerequisites: Vec<u32>,
    pub unlocks: Vec<Unlock>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Unlock {
    /// `unit`, `building`, `wonder` or `improvement`.
    pub kind: String,
    pub id: u32,
    pub name: String,
    /// Set for a nation's unique unit or building, by nation name.
    pub unique_to: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct NationRow {
    pub id: u32,
    pub name: String,
    pub leader: Option<String>,
    pub outer_color: Vec<i32>,
    pub inner_color: Vec<i32>,
    pub city_state: bool,
    /// Suggested names for new cities, in the nation's order.
    pub cities: Vec<String>,
    /// The raiders' nation (`isBarbarian`): never playable, always hostile.
    #[ts(optional)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub barbarian: Option<bool>,
    /// One of Europe, WestAsia, Africa, SouthAsia, EastAsia, SoutheastAsia,
    /// Steppe, Americas.
    #[ts(optional)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub region: Option<String>,
    /// Who may lead a major nation, the default first; choose one with
    /// `Settings.leader`.
    #[ts(optional)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub leaders: Option<Vec<LeaderRow>>,
}

/// A leader and the ability they bring (the nation's unique units and
/// buildings stay with the nation).
#[derive(Debug, Clone, Serialize, Deserialize, TS, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct LeaderRow {
    pub name: String,
    pub ability: Option<String>,
    pub effects: Vec<String>,
    /// The leader's nearest living language.
    pub voice: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, schemars::JsonSchema)]
pub struct Named {
    pub id: u32,
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, schemars::JsonSchema)]
pub struct BeliefRow {
    pub id: u32,
    pub name: String,
    pub kind: String,
    pub effects: Vec<String>,
}
macro_rules! yields {
    ($x:expr) => {
        Yields {
            food: $x.food.unwrap_or(0),
            production: $x.production.unwrap_or(0),
            gold: $x.gold.unwrap_or(0),
            science: $x.science.unwrap_or(0),
            culture: $x.culture.unwrap_or(0),
            faith: $x.faith.unwrap_or(0),
        }
    };
}

fn rule_text(u: &civ_core::Unique) -> String {
    let mut text = u.text.clone();
    for c in &u.conditionals {
        text.push_str(&format!(" <{}>", rule_text(c)));
    }
    text
}

fn tech_row(r: &Ruleset, id: civ_core::arena::Id<civ_core::ruleset::Technology>, t: &civ_core::ruleset::Technology) -> TechRow {
    let column = t.column.and_then(|c| r.tech_columns.get(c));
    let opens = |required: &Option<String>| required.as_deref() == Some(t.name.as_str());
    let mut unlocks: Vec<Unlock> = r
        .units
        .iter()
        .filter(|(_, u)| opens(&u.required_tech))
        .map(|(id, u)| Unlock { kind: "unit".into(), id: id.index(), name: u.name.clone(), unique_to: u.unique_to.clone() })
        .collect();
    unlocks.extend(r.buildings.iter().filter(|(_, b)| opens(&b.required_tech)).map(|(id, b)| Unlock {
        kind: if b.is_wonder.unwrap_or(false) { "wonder" } else { "building" }.into(),
        id: id.index(),
        name: b.name.clone(),
        unique_to: b.unique_to.clone(),
    }));
    unlocks.extend(r.improvements.iter().filter(|(_, i)| opens(&i.tech_required)).map(|(id, i)| Unlock {
        kind: "improvement".into(),
        id: id.index(),
        name: i.name.clone(),
        unique_to: i.unique_to.clone(),
    }));
    TechRow {
        id: id.index(),
        name: t.name.clone(),
        era: column.map(|c| c.era.clone()).unwrap_or_default(),
        column: column.map_or(0, |c| c.column_number),
        row: t.row,
        cost: t.cost.or(column.map(|c| c.tech_cost)).unwrap_or(0),
        prerequisites: t.prerequisites.iter().filter_map(|name| r.technologies.id(name)).map(|p| p.index()).collect(),
        unlocks,
    }
}

pub fn tables(r: &Ruleset) -> RuleTables {
    RuleTables {
        policies: r
            .policies
            .iter()
            .map(|(id, x)| Named {
                id: id.index(),
                name: x.name.clone(),
            })
            .collect(),
        promotions: r
            .promotions
            .iter()
            .map(|(id, x)| Named {
                id: id.index(),
                name: x.name.clone(),
            })
            .collect(),
        specialists: r
            .specialists
            .iter()
            .map(|(id, x)| Named {
                id: id.index(),
                name: x.name.clone(),
            })
            .collect(),
        beliefs: r
            .beliefs
            .iter()
            .map(|(id, b)| BeliefRow {
                id: id.index(),
                name: b.name.clone(),
                kind: b.r#type.clone(),
                effects: b.uniques.iter().map(rule_text).collect(),
            })
            .collect(),
        victories: r
            .victories
            .iter()
            .map(|(id, victory)| Named {
                id: id.index(),
                name: victory.name.clone(),
            })
            .collect(),
        terrains: r
            .terrains
            .iter()
            .map(|(id, t)| TerrainRow {
                id: id.index(),
                name: t.name.clone(),
                kind: t.r#type.clone(),
                impassable: t.impassable.unwrap_or(false),
                movement_cost: t.movement_cost.unwrap_or(1),
                yields: yields!(t),
                override_stats: t.override_stats.unwrap_or(false),
            })
            .collect(),
        units: r
            .units
            .iter()
            .map(|(id, u)| UnitRow {
                id: id.index(),
                name: u.name.clone(),
                unit_type: u.unit_type.clone(),
                movement: u.movement,
                strength: u.strength.unwrap_or(0),
                ranged_strength: u.ranged_strength.unwrap_or(0),
                range: u.range.unwrap_or(0),
                cost: u.cost.unwrap_or(0),
                unique_to: u.unique_to.clone(),
            })
            .collect(),
        nations: r
            .nations
            .iter()
            .map(|(id, n)| NationRow {
                id: id.index(),
                name: n.name.clone(),
                leader: n.leader_name.clone(),
                outer_color: n.outer_color.clone(),
                inner_color: n.inner_color.clone(),
                city_state: n.city_state_type.is_some(),
                cities: n.cities.clone(),
                barbarian: n.is_barbarian.then_some(true),
                region: n.region.clone(),
                leaders: n.is_major().then(|| {
                    std::iter::once(LeaderRow {
                        name: n.leader_name.clone().unwrap_or_default(),
                        ability: n.unique_name.clone(),
                        effects: n.uniques.iter().map(rule_text).collect(),
                        voice: n.voice.clone(),
                    })
                    .chain(n.alt_leader.as_ref().map(|a| LeaderRow {
                        name: a.leader_name.clone(),
                        ability: a.unique_name.clone(),
                        effects: a.uniques.iter().map(rule_text).collect(),
                        voice: a.voice.clone().or_else(|| n.voice.clone()),
                    }))
                    .collect()
                }),
            })
            .collect(),
        resources: r
            .resources
            .iter()
            .map(|(id, x)| ResourceRow {
                id: id.index(),
                name: x.name.clone(),
                kind: x.resource_type.clone(),
                yields: yields!(x),
                improvement: x.improvement.clone(),
            })
            .collect(),
        improvements: r
            .improvements
            .iter()
            .map(|(id, x)| ImprovementRow {
                id: id.index(),
                name: x.name.clone(),
                yields: yields!(x),
                turns_to_build: x.turns_to_build.unwrap_or(0),
            })
            .collect(),
        buildings: r
            .buildings
            .iter()
            .map(|(id, x)| BuildingRow {
                id: id.index(),
                name: x.name.clone(),
                cost: x.cost.unwrap_or(0),
                wonder: x.is_wonder.unwrap_or(false),
                national_wonder: x.is_national_wonder.unwrap_or(false),
                yields: yields!(x),
                happiness: x.happiness.unwrap_or(0),
                maintenance: x.maintenance.unwrap_or(0),
                percent: x.percent_stat_bonus.iter().map(|(k, v)| (k.clone(), *v)).collect(),
                effects: x.uniques.iter().map(rule_text).collect(),
                requires: x.required_building.clone(),
            })
            .collect(),
        technologies: r.technologies.iter().map(|(id, t)| tech_row(r, id, t)).collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_tech_tree_is_laid_out_with_prerequisites_and_unlocks() {
        let r = crate::content::load().unwrap();
        let t = tables(&r);
        let tech = |name: &str| t.technologies.iter().find(|x| x.name == name).unwrap();
        let ids: std::collections::BTreeSet<u32> = t.technologies.iter().map(|x| x.id).collect();
        for row in &t.technologies {
            assert!(!row.era.is_empty() && row.cost > 0, "{}", row.name);
            assert!(row.prerequisites.iter().all(|p| ids.contains(p)), "{}", row.name);
            let later = row.prerequisites.iter().all(|p| t.technologies.iter().find(|x| x.id == *p).unwrap().column < row.column);
            assert!(later, "{} sits right of its prerequisites", row.name);
        }
        assert!(tech("Agriculture").prerequisites.is_empty());
        assert_eq!(tech("Agriculture").column, 0);
        let opens = |t: &TechRow, name: &str| t.unlocks.iter().any(|u| u.name == name);
        assert!(opens(tech("Mining"), "Mine"));
        assert!(opens(tech("Bronze Working"), "Spearman"));
        assert!(tech("Masonry").unlocks.iter().any(|u| u.kind == "wonder"));
        let spearman = t.units.iter().find(|u| u.name == "Spearman").unwrap();
        assert!(spearman.strength > 0 && spearman.cost > 0 && spearman.ranged_strength == 0);
        let resource = |name: &str| t.resources.iter().find(|x| x.name == name).unwrap();
        assert_eq!(resource("Iron").kind, "Strategic");
        assert_eq!(resource("Wheat").kind, "Bonus");
        assert_eq!(resource("Silk").kind, "Luxury");
        let terrain = |name: &str| t.terrains.iter().find(|x| x.name == name).unwrap();
        assert_eq!(terrain("Grassland").yields.food, 2);
        assert_eq!(terrain("Hill").movement_cost, 2);
        assert!(t.improvements.iter().any(|i| i.name == "Farm" && i.yields.food == 1));
        let monument = t.buildings.iter().find(|b| b.name == "Monument").unwrap();
        assert!(monument.yields.culture > 0 && monument.maintenance >= 0);
    }
}
