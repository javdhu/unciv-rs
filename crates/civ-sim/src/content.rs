//! Complete, versioned base rules. Gameplay must never load the fingerprint fixture subset.
use civ_core::ruleset::Ruleset;
use sha2::{Digest, Sha256};
use std::sync::OnceLock;

pub const ENGINE_VERSION: &str = "civ-core/0.1-session-4";

pub const SOURCES: &[(&str, &str)] = &[
    ("Beliefs.json", include_str!("../../civ-core/rules/base/Beliefs.json")),
    ("Buildings.json", include_str!("../../civ-core/rules/base/Buildings.json")),
    ("CityStateTypes.json", include_str!("../../civ-core/rules/base/CityStateTypes.json")),
    ("Difficulties.json", include_str!("../../civ-core/rules/base/Difficulties.json")),
    ("Eras.json", include_str!("../../civ-core/rules/base/Eras.json")),
    ("Events.json", include_str!("../../civ-core/rules/base/Events.json")),
    ("GlobalUniques.json", include_str!("../../civ-core/rules/base/GlobalUniques.json")),
    ("ModOptions.json", include_str!("../../civ-core/rules/base/ModOptions.json")),
    ("Nations.json", include_str!("../../civ-core/rules/base/Nations.json")),
    ("Personalities.json", include_str!("../../civ-core/rules/base/Personalities.json")),
    ("Policies.json", include_str!("../../civ-core/rules/base/Policies.json")),
    ("Quests.json", include_str!("../../civ-core/rules/base/Quests.json")),
    ("Religions.json", include_str!("../../civ-core/rules/base/Religions.json")),
    ("Ruins.json", include_str!("../../civ-core/rules/base/Ruins.json")),
    ("Specialists.json", include_str!("../../civ-core/rules/base/Specialists.json")),
    ("Speeds.json", include_str!("../../civ-core/rules/base/Speeds.json")),
    ("Techs.json", include_str!("../../civ-core/rules/base/Techs.json")),
    ("Terrains.json", include_str!("../../civ-core/rules/base/Terrains.json")),
    ("TileImprovements.json", include_str!("../../civ-core/rules/base/TileImprovements.json")),
    ("TileResources.json", include_str!("../../civ-core/rules/base/TileResources.json")),
    ("UnitNameGroups.json", include_str!("../../civ-core/rules/base/UnitNameGroups.json")),
    ("UnitPromotions.json", include_str!("../../civ-core/rules/base/UnitPromotions.json")),
    ("UnitTypes.json", include_str!("../../civ-core/rules/base/UnitTypes.json")),
    ("Units.json", include_str!("../../civ-core/rules/base/Units.json")),
    ("VictoryTypes.json", include_str!("../../civ-core/rules/base/VictoryTypes.json")),
];

/// The full game. Every other ruleset is a mod layered over it.
pub const DEFAULT_RULESET: &str = "base";
/// The one-evening game (docs/BLITZ.md).
pub const BLITZ: &str = "blitz";
pub const RULESETS: &[&str] = &[DEFAULT_RULESET, BLITZ];

/// Blitz's files, loaded after the full game's and overriding its objects by
/// name (`Ruleset::from_sources`).
pub const BLITZ_SOURCES: &[(&str, &str)] = &[
    ("Difficulties.json", include_str!("../../civ-core/rules/blitz/Difficulties.json")),
    ("Eras.json", include_str!("../../civ-core/rules/blitz/Eras.json")),
    ("ModOptions.json", include_str!("../../civ-core/rules/blitz/ModOptions.json")),
    ("Speeds.json", include_str!("../../civ-core/rules/blitz/Speeds.json")),
];

fn overlay(ruleset: &str) -> Result<&'static [(&'static str, &'static str)], String> {
    match ruleset {
        DEFAULT_RULESET => Ok(&[]),
        BLITZ => Ok(BLITZ_SOURCES),
        other => Err(format!("unknown ruleset: {other}")),
    }
}

/// The speed a ruleset plays at unless the settings choose another.
pub fn default_speed(ruleset: &str) -> &'static str {
    if ruleset == BLITZ {
        "Blitz"
    } else {
        "Standard"
    }
}

pub fn load() -> Result<Ruleset, String> {
    load_ruleset(DEFAULT_RULESET)
}

pub fn load_ruleset(ruleset: &str) -> Result<Ruleset, String> {
    let sources: Vec<_> = SOURCES
        .iter()
        .chain(overlay(ruleset)?)
        .map(|(name, data)| (name.to_string(), data.to_string()))
        .collect();
    Ruleset::from_sources(&sources).map_err(|e| format!("ruleset failed to load: {e}"))
}

/// One parsed copy of the bundled rules, for callers that only read them.
pub fn shared() -> Result<&'static Ruleset, String> {
    shared_ruleset(DEFAULT_RULESET)
}

pub fn shared_ruleset(ruleset: &str) -> Result<&'static Ruleset, String> {
    static FULL: OnceLock<Result<Ruleset, String>> = OnceLock::new();
    static BLITZED: OnceLock<Result<Ruleset, String>> = OnceLock::new();
    let cell = match ruleset {
        BLITZ => &BLITZED,
        DEFAULT_RULESET => &FULL,
        other => return Err(format!("unknown ruleset: {other}")),
    };
    cell.get_or_init(|| load_ruleset(ruleset)).as_ref().map_err(Clone::clone)
}

/// The full game's content hash, unchanged by the rulesets layered over it.
pub fn hash() -> String {
    static HASH: OnceLock<String> = OnceLock::new();
    HASH.get_or_init(compute_hash).clone()
}

/// A ruleset's content hash: the full game's, then the ruleset's name and
/// its own files. A Blitz match never shares a hash with a full game.
pub fn hash_of(ruleset: &str) -> Result<String, String> {
    let files = overlay(ruleset)?;
    if files.is_empty() {
        return Ok(hash());
    }
    let mut digest = Sha256::new();
    digest.update(hash().as_bytes());
    digest.update((ruleset.len() as u64).to_le_bytes());
    digest.update(ruleset.as_bytes());
    for (name, data) in files {
        digest.update((name.len() as u64).to_le_bytes());
        digest.update(name.as_bytes());
        digest.update((data.len() as u64).to_le_bytes());
        digest.update(data.as_bytes());
    }
    Ok(format!("{:x}", digest.finalize()))
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
