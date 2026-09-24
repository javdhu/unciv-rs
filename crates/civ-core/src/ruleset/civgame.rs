//! Fields civ.game adds to the Unciv ruleset schema.
//!
//! `rules/base` writes these; the frozen `gods-and-kings` and `vanilla`
//! fixtures never do, so every mechanic that reads them is switched off there
//! by the data's absence rather than by a flag. The generated structs carry
//! them as load-time fields (see `DERIVED` in `tools/generate_ruleset.py`),
//! filled here from the object still in hand.

use super::generated::Nation;
use crate::json::Json;
use crate::unique::Unique;

/// The eight regions a nation or city-state belongs to (docs/REBUILD.md).
pub const REGIONS: [&str; 8] =
    ["Europe", "WestAsia", "Africa", "SouthAsia", "EastAsia", "SoutheastAsia", "Steppe", "Americas"];

/// A nation's second leader, chosen at game setup. Their uniques replace the
/// nation's own, which are the default leader's ability; unique units and
/// buildings stay with the nation.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AltLeader {
    pub leader_name: String,
    pub unique_name: Option<String>,
    pub uniques: Vec<Unique>,
    pub voice: Option<String>,
}

/// One step of a city's history: `from` becomes `to` once its owner reaches
/// `era`. Steps may chain (Byzantion, Constantinople, Istanbul).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CityRename {
    pub from: String,
    pub to: String,
    pub era: String,
}

fn text(j: &Json, key: &str) -> Option<String> {
    j.get(key).and_then(|v| v.as_str()).map(String::from)
}

fn uniques(j: &Json) -> Vec<Unique> {
    j.get("uniques").map(|v| v.strings().iter().map(|s| Unique::parse(s)).collect()).unwrap_or_default()
}

/// The name the fixtures give their barbarians, which carry no flag.
pub const LEGACY_BARBARIANS: &str = "Barbarians";

/// Read civ.game's nation fields from the object `n` was parsed from.
pub fn read_nation(j: &Json, n: &mut Nation) {
    n.region = text(j, "region");
    n.voice = text(j, "voice");
    n.alt_leader = j.get("altLeader").and_then(|a| {
        Some(AltLeader {
            leader_name: text(a, "leaderName")?,
            unique_name: text(a, "uniqueName"),
            uniques: uniques(a),
            voice: text(a, "voice"),
        })
    });
    n.city_renames = j
        .get("cityRenames")
        .and_then(|v| v.as_array())
        .unwrap_or_default()
        .iter()
        .filter_map(|r| Some(CityRename { from: text(r, "from")?, to: text(r, "to")?, era: text(r, "era")? }))
        .collect();
    n.is_barbarian = j.get("isBarbarian").and_then(|v| v.as_bool()).unwrap_or(n.name == LEGACY_BARBARIANS);
}

impl Nation {
    /// A nation a player or the AI may lead: not a city-state, not the
    /// barbarians, and with someone to lead it (which rules out Spectator).
    pub fn is_major(&self) -> bool {
        self.city_state_type.is_none() && !self.is_barbarian && self.leader_name.is_some()
    }

    /// Every leader this nation offers, the default first.
    pub fn leaders(&self) -> impl Iterator<Item = &str> {
        self.leader_name.as_deref().into_iter().chain(self.alt_leader.as_ref().map(|a| a.leader_name.as_str()))
    }

    /// The ability of whoever leads: the nation's own uniques, or the second
    /// leader's in their place.
    pub fn leader_uniques(&self, alt_leader: bool) -> &[Unique] {
        match (&self.alt_leader, alt_leader) {
            (Some(alt), true) => &alt.uniques,
            _ => &self.uniques,
        }
    }

    /// The name of whoever leads.
    pub fn leader(&self, alt_leader: bool) -> Option<&str> {
        match (&self.alt_leader, alt_leader) {
            (Some(alt), true) => Some(&alt.leader_name),
            _ => self.leader_name.as_deref(),
        }
    }
}
