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

#[cfg(test)]
mod tests {
    use super::*;
    use civ_core::ruleset::Technology;
    use std::collections::BTreeSet;

    const ERAS: [&str; 8] = [
        "Age of Bronze",
        "Age of Iron",
        "Age of Paper",
        "Age of Sail",
        "Age of Steam",
        "Age of Electricity",
        "Age of the Atom",
        "Age of Information",
    ];
    /// The tech each era after the first opens with (docs/CONTENT.md, Spine).
    const OPENERS: [(&str, &str); 7] = [
        ("Age of Iron", "Iron Working"),
        ("Age of Paper", "Paper"),
        ("Age of Sail", "Navigation"),
        ("Age of Steam", "Steam Power"),
        ("Age of Electricity", "Electricity"),
        ("Age of the Atom", "Nuclear Fission"),
        ("Age of Information", "The Internet"),
    ];

    #[test]
    fn every_ruleset_resolves_its_references() {
        for name in RULESETS {
            let r = load_ruleset(name).unwrap();
            assert!(r.dangling_refs().is_empty(), "{name}: {:?}", r.dangling_refs());
        }
    }

    #[test]
    fn discovering_the_namesake_tech_is_the_only_way_into_an_era() {
        let r = load().unwrap();
        let eras: Vec<_> = r.eras.iter().map(|(_, e)| e.name.as_str()).collect();
        assert_eq!(eras, ERAS);
        let era_of = |t: &Technology| {
            t.column.and_then(|c| r.tech_columns.get(c)).map(|c| c.era.clone()).unwrap_or_default()
        };
        for (era, opener) in OPENERS {
            let tech = r.technologies.by_name(opener).unwrap_or_else(|| panic!("no {opener}"));
            assert_eq!(era_of(tech), era, "{opener}");
            for (_, t) in r.technologies.iter().filter(|(_, t)| era_of(t) == era && t.name != opener) {
                let mut needs = BTreeSet::new();
                let mut open = t.prerequisites.clone();
                while let Some(p) = open.pop() {
                    if needs.insert(p.clone()) {
                        open.extend(r.technologies.by_name(&p).unwrap().prerequisites.iter().cloned());
                    }
                }
                assert!(needs.contains(opener), "{} is in the {era} without requiring {opener}", t.name);
            }
        }
    }

    #[test]
    fn victories_name_things_that_exist() {
        let r = load().unwrap();
        for (_, v) in r.victories.iter() {
            for part in &v.required_spaceship_parts {
                assert!(r.units.id(part).is_some(), "{}: no unit {part}", v.name);
            }
            for m in &v.milestones {
                if let Some(b) = m.strip_prefix("Build [").and_then(|m| m.strip_suffix(']')) {
                    assert!(r.buildings.id(b).is_some(), "{}: no building {b}", v.name);
                }
            }
        }
    }
}
