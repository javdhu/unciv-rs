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
    pub resources: Vec<Named>,
    pub improvements: Vec<Named>,
    pub buildings: Vec<Named>,
    pub technologies: Vec<Named>,
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
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct UnitRow {
    pub id: u32,
    pub name: String,
    pub unit_type: String,
    pub movement: i32,
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
fn rule_text(u: &civ_core::Unique) -> String {
    let mut text = u.text.clone();
    for c in &u.conditionals {
        text.push_str(&format!(" <{}>", rule_text(c)));
    }
    text
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
            })
            .collect(),
        resources: r
            .resources
            .iter()
            .map(|(id, x)| Named {
                id: id.index(),
                name: x.name.clone(),
            })
            .collect(),
        improvements: r
            .improvements
            .iter()
            .map(|(id, x)| Named {
                id: id.index(),
                name: x.name.clone(),
            })
            .collect(),
        buildings: r
            .buildings
            .iter()
            .map(|(id, x)| Named {
                id: id.index(),
                name: x.name.clone(),
            })
            .collect(),
        technologies: r
            .technologies
            .iter()
            .map(|(id, x)| Named {
                id: id.index(),
                name: x.name.clone(),
            })
            .collect(),
    }
}
