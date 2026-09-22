//! Turning a directory of json into a queryable [`Ruleset`].
//!
//! Everything a rule might look up happens once, here. Names become
//! [`Id`](crate::arena::Id)s at load, so no hot path ever does a string
//! comparison — and the AI asks for a tile's yield often enough that this is
//! the difference between a fast turn and a slow one.
//!
//! The parsing entry point takes already-read text rather than paths, because
//! the wasm client fetches these over the network and has no filesystem.
//! [`Ruleset::load_dir`] is the native convenience on top.

use super::generated::*;
use crate::arena::{Id, Registry};
use crate::json::Json;
use crate::UniqueType;
use std::fmt;

/// Everything a game is played with. Immutable once loaded.
#[derive(Debug, Default, Clone)]
pub struct Ruleset {
    pub buildings: Registry<Building>,
    pub units: Registry<BaseUnit>,
    pub unit_types: Registry<UnitType>,
    pub terrains: Registry<Terrain>,
    pub resources: Registry<TileResource>,
    pub improvements: Registry<TileImprovement>,
    pub technologies: Registry<Technology>,
    pub tech_columns: Registry<TechColumn>,
    pub policies: Registry<Policy>,
    pub policy_branches: Registry<PolicyBranch>,
    pub promotions: Registry<Promotion>,
    pub nations: Registry<Nation>,
    pub beliefs: Registry<Belief>,
    pub difficulties: Registry<Difficulty>,
    pub eras: Registry<Era>,
    pub speeds: Registry<Speed>,
    pub specialists: Registry<Specialist>,
    pub victories: Registry<Victory>,
    pub city_state_types: Registry<CityStateType>,
    /// What city-states ask of major civs. See `world::quests`.
    pub quests: Registry<Quest>,
    /// Rules that apply to everyone, from `GlobalUniques.json`.
    ///
    /// This is where Civ keeps the consequences of unhappiness — the growth and
    /// production penalties are conditional uniques here, not code — so a
    /// ruleset that does not load this file has an empire that cannot suffer.
    pub global_uniques: Vec<crate::Unique>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum LoadError {
    Json { file: String, error: crate::json::Error },
    Shape { file: String, message: String },
}

impl fmt::Display for LoadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LoadError::Json { file, error } => write!(f, "{file}: {error}"),
            LoadError::Shape { file, message } => write!(f, "{file}: {message}"),
        }
    }
}

impl std::error::Error for LoadError {}

/// A cross-reference in the ruleset that points at something absent.
///
/// Not fatal — a mod may deliberately leave dangling names, and Unciv loads
/// them — so these are reported rather than raised.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct DanglingRef {
    pub from_kind: &'static str,
    pub from: String,
    pub field: &'static str,
    pub to: String,
}

impl fmt::Display for DanglingRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} `{}`: {} -> `{}` does not exist", self.from_kind, self.from, self.field, self.to)
    }
}

/// Read every object in a top-level array into a registry.
fn flat<T>(reg: &mut Registry<T>, doc: &Json, read: impl Fn(&Json) -> Option<T>) {
    for item in doc.as_array().unwrap_or_default() {
        if let (Some(name), Some(v)) = (item.get("name").and_then(|n| n.as_str()), read(item)) {
            reg.insert(name, v);
        }
    }
}

impl Ruleset {
    /// Parse a ruleset from `(filename, contents)` pairs. Unknown filenames are
    /// ignored, so a ruleset carrying extra files still loads.
    pub fn from_sources(sources: &[(String, String)]) -> Result<Ruleset, LoadError> {
        let mut r = Ruleset::default();
        for (file, text) in sources {
            let doc = Json::parse(text).map_err(|error| LoadError::Json { file: file.clone(), error })?;
            let stem = file.rsplit('/').next().unwrap_or(file);
            match stem {
                "Buildings.json" => flat(&mut r.buildings, &doc, Building::from_json),
                "Units.json" => flat(&mut r.units, &doc, BaseUnit::from_json),
                "UnitTypes.json" => flat(&mut r.unit_types, &doc, UnitType::from_json),
                "Terrains.json" => flat(&mut r.terrains, &doc, Terrain::from_json),
                "TileResources.json" => flat(&mut r.resources, &doc, TileResource::from_json),
                "TileImprovements.json" => flat(&mut r.improvements, &doc, TileImprovement::from_json),
                "UnitPromotions.json" => flat(&mut r.promotions, &doc, Promotion::from_json),
                "Nations.json" => flat(&mut r.nations, &doc, Nation::from_json),
                "Beliefs.json" => flat(&mut r.beliefs, &doc, Belief::from_json),
                "Difficulties.json" => flat(&mut r.difficulties, &doc, Difficulty::from_json),
                "Eras.json" => flat(&mut r.eras, &doc, Era::from_json),
                "Speeds.json" => {
                    flat(&mut r.speeds, &doc, Speed::from_json);
                    // How long a game at this speed runs. Unciv reads it as
                    // the last `untilTurn` of the year table, which is nested
                    // structure rather than a field, so it has to be picked
                    // out here where the json is still in hand.
                    for item in doc.as_array().unwrap_or_default() {
                        let (Some(name), Some(turns)) = (item.get("name").and_then(|n| n.as_str()), item.get("turns"))
                        else {
                            continue;
                        };
                        let last = turns
                            .as_array()
                            .unwrap_or_default()
                            .last()
                            .and_then(|t| t.get("untilTurn"))
                            .and_then(|t| t.as_i32());
                        if let (Some(id), Some(last)) = (r.speeds.id(name), last) {
                            if let Some(s) = r.speeds.get_mut(id) {
                                s.total_turns = last;
                            }
                        }
                    }
                }
                "Specialists.json" => flat(&mut r.specialists, &doc, Specialist::from_json),
                "VictoryTypes.json" => flat(&mut r.victories, &doc, Victory::from_json),
                "CityStateTypes.json" => flat(&mut r.city_state_types, &doc, CityStateType::from_json),
                "Quests.json" => flat(&mut r.quests, &doc, Quest::from_json),
                "GlobalUniques.json" => {
                    r.global_uniques = doc
                        .get("uniques")
                        .map(|v| v.strings().iter().map(|s| crate::Unique::parse(s)).collect())
                        .unwrap_or_default();
                }
                "Techs.json" => {
                    for group in doc.as_array().unwrap_or_default() {
                        // Techs.json gives columns no name, and several share an
                        // era, so the key has to include the column number.
                        let Some(mut c) = TechColumn::from_json(group) else { continue };
                        c.name = format!("{} #{}", c.era, c.column_number);
                        let key = c.name.clone();
                        let col = Some(r.tech_columns.insert(key, c));
                        for item in group.get("techs").and_then(|v| v.as_array()).unwrap_or_default() {
                            if let (Some(name), Some(mut t)) =
                                (item.get("name").and_then(|n| n.as_str()), Technology::from_json(item))
                            {
                                t.column = col;
                                r.technologies.insert(name, t);
                            }
                        }
                    }
                }
                "Policies.json" => {
                    for group in doc.as_array().unwrap_or_default() {
                        let Some(bname) = group.get("name").and_then(|n| n.as_str()) else { continue };
                        let branch = PolicyBranch::from_json(group).map(|b| r.policy_branches.insert(bname, b));
                        // A branch is a policy too: it is adopted before any
                        // of the policies inside it, and Unciv keeps it in the
                        // same collection (`Ruleset.kt:458`, `branch.branch =
                        // branch`). Registering it here means "which policies
                        // can I adopt" has one answer, not two.
                        if let Some(mut p) = Policy::from_json(group) {
                            p.branch = branch;
                            r.policies.insert(bname, p);
                        }
                        for item in group.get("policies").and_then(|v| v.as_array()).unwrap_or_default() {
                            if let (Some(name), Some(mut p)) =
                                (item.get("name").and_then(|n| n.as_str()), Policy::from_json(item))
                            {
                                p.branch = branch;
                                // A policy that states no prerequisite requires
                                // its branch. Unciv fills this in at load
                                // (`Ruleset.kt:470`) rather than repeating the
                                // branch name in every first-tier policy — so
                                // the gate exists as structure, not as a field.
                                if p.requires.is_empty() {
                                    p.requires.push(bname.to_string());
                                }
                                r.policies.insert(name, p);
                            }
                        }
                    }
                }
                _ => {}
            }
        }
        r.derive();
        Ok(r)
    }

    /// Fill in what the json states only as structure or position.
    ///
    /// Unciv does the same work at load (`Ruleset.kt:395`, `:425`, `:468`,
    /// `Terrain.kt:212`). These fields are invisible to schema inference —
    /// nothing in the json mentions them — so they are established here.
    fn derive(&mut self) {
        // An era's number is its position in Eras.json.
        let era_ids: Vec<_> = self.eras.ids().collect();
        for (i, id) in era_ids.into_iter().enumerate() {
            if let Some(e) = self.eras.get_mut(id) {
                e.era_number = i as i32;
            }
        }

        // Costs live on the tech column, not the object. Unciv fills them the
        // same way (`Ruleset.kt:394` and `:618`) and says why: editing a
        // column's costs should move every building in it, so a mod adding a
        // building inherits the original values rather than restating them.
        let tech_ids: Vec<_> = self.technologies.ids().collect();
        for id in &tech_ids {
            let column_cost = self
                .technologies
                .get(*id)
                .and_then(|t| t.column)
                .and_then(|c| self.tech_columns.get(c))
                .map(|c| c.tech_cost);
            if let (Some(t), Some(cost)) = (self.technologies.get_mut(*id), column_cost) {
                if t.cost.unwrap_or(0) == 0 {
                    t.cost = Some(cost);
                }
            }
        }

        // A building's cost comes from the column of the tech that unlocks it,
        // and wonders read a different figure from the same column.
        let building_ids: Vec<_> = self.buildings.ids().collect();
        for id in &building_ids {
            let Some(b) = self.buildings.get(*id) else { continue };
            if b.cost.is_some() {
                continue;
            }
            let is_wonder = b.is_wonder == Some(true) || b.is_national_wonder == Some(true);
            let column_cost = b
                .required_tech
                .as_deref()
                .and_then(|t| self.technologies.by_name(t))
                .and_then(|t| t.column)
                .and_then(|c| self.tech_columns.get(c))
                .map(|c| if is_wonder { c.wonder_cost } else { c.building_cost });
            if let (Some(b), Some(cost)) = (self.buildings.get_mut(*id), column_cost) {
                b.cost = Some(cost);
            }
        }

        // Terrain damage is the sum of its DamagesContainingUnits uniques.
        let terrain_ids: Vec<_> = self.terrains.ids().collect();
        for id in terrain_ids {
            let Some(t) = self.terrains.get_mut(id) else { continue };
            t.damage_per_turn = t
                .uniques
                .iter()
                .filter(|u| u.unique_type() == Some(UniqueType::DamagesContainingUnits))
                .filter_map(|u| u.params.first())
                .filter_map(|p| p.parse::<i32>().ok())
                .sum();
        }
    }

    /// Read every `*.json` in a directory. Native only — wasm has no filesystem.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn load_dir(dir: impl AsRef<std::path::Path>) -> std::io::Result<Result<Ruleset, LoadError>> {
        let mut sources = Vec::new();
        let mut entries: Vec<_> = std::fs::read_dir(dir)?.flatten().map(|e| e.path()).collect();
        entries.sort(); // deterministic load order, so overrides resolve the same way every time
        for p in entries {
            if p.extension().is_some_and(|x| x == "json") {
                let name = p.file_name().unwrap_or_default().to_string_lossy().into_owned();
                sources.push((name, std::fs::read_to_string(&p)?));
            }
        }
        Ok(Ruleset::from_sources(&sources))
    }

    pub fn object_count(&self) -> usize {
        self.tech_columns.len()
            + self.buildings.len()
            + self.units.len()
            + self.unit_types.len()
            + self.terrains.len()
            + self.resources.len()
            + self.improvements.len()
            + self.technologies.len()
            + self.policies.len()
            + self.policy_branches.len()
            + self.promotions.len()
            + self.nations.len()
            + self.beliefs.len()
            + self.difficulties.len()
            + self.eras.len()
            + self.speeds.len()
            + self.specialists.len()
            + self.victories.len()
            + self.city_state_types.len()
            + self.quests.len()
    }

    /// Every name referenced by one object that no object answers to.
    ///
    /// Unciv's own validator does the equivalent; running it at load turns a
    /// mod's typo into a message instead of a missing building at turn 40.
    pub fn dangling_refs(&self) -> Vec<DanglingRef> {
        let mut out = Vec::new();

        let mut check = |kind: &'static str, from: &str, field: &'static str, to: Option<&String>, exists: bool| {
            if let Some(to) = to {
                if !to.is_empty() && !exists {
                    out.push(DanglingRef { from_kind: kind, from: from.into(), field, to: to.clone() });
                }
            }
        };

        for (_, b) in self.buildings.iter() {
            let t = b.required_tech.as_ref();
            check("Building", &b.name, "requiredTech", t, t.is_none_or(|n| self.technologies.id(n).is_some()));
            let rb = b.required_building.as_ref();
            check("Building", &b.name, "requiredBuilding", rb, rb.is_none_or(|n| self.buildings.id(n).is_some()));
            let rp = b.replaces.as_ref();
            check("Building", &b.name, "replaces", rp, rp.is_none_or(|n| self.buildings.id(n).is_some()));
        }
        for (_, u) in self.units.iter() {
            let t = u.required_tech.as_ref();
            check("Unit", &u.name, "requiredTech", t, t.is_none_or(|n| self.technologies.id(n).is_some()));
            let up = u.upgrades_to.as_ref();
            check("Unit", &u.name, "upgradesTo", up, up.is_none_or(|n| self.units.id(n).is_some()));
            let rp = u.replaces.as_ref();
            check("Unit", &u.name, "replaces", rp, rp.is_none_or(|n| self.units.id(n).is_some()));
            let ut = Some(&u.unit_type);
            check("Unit", &u.name, "unitType", ut, self.unit_types.id(&u.unit_type).is_some());
        }
        for (_, t) in self.terrains.iter() {
            let ti = t.turns_into.as_ref();
            check("Terrain", &t.name, "turnsInto", ti, ti.is_none_or(|n| self.terrains.id(n).is_some()));
        }
        out.sort();
        out.dedup();
        out
    }

    /// Resolve a name in whichever registry is meant. Convenience for tests and
    /// for the civilopedia, not for hot paths.
    pub fn building(&self, name: &str) -> Option<Id<Building>> {
        self.buildings.id(name)
    }
    pub fn terrain(&self, name: &str) -> Option<Id<Terrain>> {
        self.terrains.id(name)
    }
    pub fn unit(&self, name: &str) -> Option<Id<BaseUnit>> {
        self.units.id(name)
    }
}
