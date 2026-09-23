//! Complete, versioned base rules. Gameplay must never load the fingerprint fixture subset.
use civ_core::ruleset::Ruleset;
use sha2::{Digest, Sha256};

pub const ENGINE_VERSION: &str = "civ-core/0.1-session-4";

pub const SOURCES: &[(&str, &str)] = &[
    ("Beliefs.json", include_str!("../../../../../Downloads/Unciv-master/android/assets/jsons/Civ V - Gods & Kings/Beliefs.json")),
    ("Buildings.json", include_str!("../../../../../Downloads/Unciv-master/android/assets/jsons/Civ V - Gods & Kings/Buildings.json")),
    ("CityStateTypes.json", include_str!("../../../../../Downloads/Unciv-master/android/assets/jsons/Civ V - Gods & Kings/CityStateTypes.json")),
    ("Difficulties.json", include_str!("../../../../../Downloads/Unciv-master/android/assets/jsons/Civ V - Gods & Kings/Difficulties.json")),
    ("Eras.json", include_str!("../../../../../Downloads/Unciv-master/android/assets/jsons/Civ V - Gods & Kings/Eras.json")),
    ("Events.json", include_str!("../../../../../Downloads/Unciv-master/android/assets/jsons/Civ V - Gods & Kings/Events.json")),
    ("GlobalUniques.json", include_str!("../../../../../Downloads/Unciv-master/android/assets/jsons/Civ V - Gods & Kings/GlobalUniques.json")),
    ("ModOptions.json", include_str!("../../../../../Downloads/Unciv-master/android/assets/jsons/Civ V - Gods & Kings/ModOptions.json")),
    ("Nations.json", include_str!("../../../../../Downloads/Unciv-master/android/assets/jsons/Civ V - Gods & Kings/Nations.json")),
    ("Personalities.json", include_str!("../../../../../Downloads/Unciv-master/android/assets/jsons/Civ V - Gods & Kings/Personalities.json")),
    ("Policies.json", include_str!("../../../../../Downloads/Unciv-master/android/assets/jsons/Civ V - Gods & Kings/Policies.json")),
    ("Quests.json", include_str!("../../../../../Downloads/Unciv-master/android/assets/jsons/Civ V - Gods & Kings/Quests.json")),
    ("Religions.json", include_str!("../../../../../Downloads/Unciv-master/android/assets/jsons/Civ V - Gods & Kings/Religions.json")),
    ("Ruins.json", include_str!("../../../../../Downloads/Unciv-master/android/assets/jsons/Civ V - Gods & Kings/Ruins.json")),
    ("Specialists.json", include_str!("../../../../../Downloads/Unciv-master/android/assets/jsons/Civ V - Gods & Kings/Specialists.json")),
    ("Speeds.json", include_str!("../../../../../Downloads/Unciv-master/android/assets/jsons/Civ V - Gods & Kings/Speeds.json")),
    ("Techs.json", include_str!("../../../../../Downloads/Unciv-master/android/assets/jsons/Civ V - Gods & Kings/Techs.json")),
    ("Terrains.json", include_str!("../../../../../Downloads/Unciv-master/android/assets/jsons/Civ V - Gods & Kings/Terrains.json")),
    ("TileImprovements.json", include_str!("../../../../../Downloads/Unciv-master/android/assets/jsons/Civ V - Gods & Kings/TileImprovements.json")),
    ("TileResources.json", include_str!("../../../../../Downloads/Unciv-master/android/assets/jsons/Civ V - Gods & Kings/TileResources.json")),
    ("UnitNameGroups.json", include_str!("../../../../../Downloads/Unciv-master/android/assets/jsons/Civ V - Gods & Kings/UnitNameGroups.json")),
    ("UnitPromotions.json", include_str!("../../../../../Downloads/Unciv-master/android/assets/jsons/Civ V - Gods & Kings/UnitPromotions.json")),
    ("UnitTypes.json", include_str!("../../../../../Downloads/Unciv-master/android/assets/jsons/Civ V - Gods & Kings/UnitTypes.json")),
    ("Units.json", include_str!("../../../../../Downloads/Unciv-master/android/assets/jsons/Civ V - Gods & Kings/Units.json")),
    ("VictoryTypes.json", include_str!("../../../../../Downloads/Unciv-master/android/assets/jsons/Civ V - Gods & Kings/VictoryTypes.json")),
];

pub fn load() -> Result<Ruleset, String> {
    let sources: Vec<_> = SOURCES
        .iter()
        .map(|(name, data)| (name.to_string(), data.to_string()))
        .collect();
    Ruleset::from_sources(&sources).map_err(|e| format!("ruleset failed to load: {e}"))
}

pub fn hash() -> String {
    let mut digest = Sha256::new();
    for (name, data) in SOURCES {
        digest.update((name.len() as u64).to_le_bytes());
        digest.update(name.as_bytes());
        digest.update((data.len() as u64).to_le_bytes());
        digest.update(data.as_bytes());
    }
    format!("{:x}", digest.finalize())
}
