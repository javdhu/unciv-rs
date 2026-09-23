//! Complete, versioned base rules. Gameplay must never load the fingerprint fixture subset.
use civ_core::ruleset::Ruleset;
use sha2::{Digest, Sha256};
use std::sync::OnceLock;

pub const ENGINE_VERSION: &str = "civ-core/0.1-session-4";

pub const SOURCES: &[(&str, &str)] = &[
    ("Beliefs.json", include_str!("../../civ-core/rules/gods-and-kings/Beliefs.json")),
    ("Buildings.json", include_str!("../../civ-core/rules/gods-and-kings/Buildings.json")),
    ("CityStateTypes.json", include_str!("../../civ-core/rules/gods-and-kings/CityStateTypes.json")),
    ("Difficulties.json", include_str!("../../civ-core/rules/gods-and-kings/Difficulties.json")),
    ("Eras.json", include_str!("../../civ-core/rules/gods-and-kings/Eras.json")),
    ("Events.json", include_str!("../../civ-core/rules/gods-and-kings/Events.json")),
    ("GlobalUniques.json", include_str!("../../civ-core/rules/gods-and-kings/GlobalUniques.json")),
    ("ModOptions.json", include_str!("../../civ-core/rules/gods-and-kings/ModOptions.json")),
    ("Nations.json", include_str!("../../civ-core/rules/gods-and-kings/Nations.json")),
    ("Personalities.json", include_str!("../../civ-core/rules/gods-and-kings/Personalities.json")),
    ("Policies.json", include_str!("../../civ-core/rules/gods-and-kings/Policies.json")),
    ("Quests.json", include_str!("../../civ-core/rules/gods-and-kings/Quests.json")),
    ("Religions.json", include_str!("../../civ-core/rules/gods-and-kings/Religions.json")),
    ("Ruins.json", include_str!("../../civ-core/rules/gods-and-kings/Ruins.json")),
    ("Specialists.json", include_str!("../../civ-core/rules/gods-and-kings/Specialists.json")),
    ("Speeds.json", include_str!("../../civ-core/rules/gods-and-kings/Speeds.json")),
    ("Techs.json", include_str!("../../civ-core/rules/gods-and-kings/Techs.json")),
    ("Terrains.json", include_str!("../../civ-core/rules/gods-and-kings/Terrains.json")),
    ("TileImprovements.json", include_str!("../../civ-core/rules/gods-and-kings/TileImprovements.json")),
    ("TileResources.json", include_str!("../../civ-core/rules/gods-and-kings/TileResources.json")),
    ("UnitNameGroups.json", include_str!("../../civ-core/rules/gods-and-kings/UnitNameGroups.json")),
    ("UnitPromotions.json", include_str!("../../civ-core/rules/gods-and-kings/UnitPromotions.json")),
    ("UnitTypes.json", include_str!("../../civ-core/rules/gods-and-kings/UnitTypes.json")),
    ("Units.json", include_str!("../../civ-core/rules/gods-and-kings/Units.json")),
    ("VictoryTypes.json", include_str!("../../civ-core/rules/gods-and-kings/VictoryTypes.json")),
];

pub fn load() -> Result<Ruleset, String> {
    let sources: Vec<_> = SOURCES
        .iter()
        .map(|(name, data)| (name.to_string(), data.to_string()))
        .collect();
    Ruleset::from_sources(&sources).map_err(|e| format!("ruleset failed to load: {e}"))
}

/// One parsed copy of the bundled rules, for callers that only read them.
pub fn shared() -> Result<&'static Ruleset, String> {
    static RULES: OnceLock<Result<Ruleset, String>> = OnceLock::new();
    RULES.get_or_init(load).as_ref().map_err(Clone::clone)
}

pub fn hash() -> String {
    static HASH: OnceLock<String> = OnceLock::new();
    HASH.get_or_init(compute_hash).clone()
}

fn compute_hash() -> String {
    let mut digest = Sha256::new();
    for (name, data) in SOURCES {
        digest.update((name.len() as u64).to_le_bytes());
        digest.update(name.as_bytes());
        digest.update((data.len() as u64).to_le_bytes());
        digest.update(data.as_bytes());
    }
    format!("{:x}", digest.finalize())
}
