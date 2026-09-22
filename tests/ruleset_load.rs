//! Load the shipped rulesets for real.
//!
//! The parser, the generated readers, the registries and the name resolution
//! all meet here. If this passes, the ruleset is queryable by id.

use civ_core::ruleset::Ruleset;
use civ_core::stats::Fixed;
use civ_core::UniqueKind;

const GK: &str = "../android/assets/jsons/Civ V - Gods & Kings";
const VANILLA: &str = "../android/assets/jsons/Civ V - Vanilla";

fn load(dir: &str) -> Ruleset {
    Ruleset::load_dir(dir)
        .expect("directory is readable")
        .unwrap_or_else(|e| panic!("ruleset failed to load: {e}"))
}

#[test]
fn loads_gods_and_kings() {
    let r = load(GK);
    println!(
        "buildings {}  units {}  techs {}  terrains {}  resources {}  improvements {}\n\
         policies {} (in {} branches)  promotions {}  nations {}  beliefs {}  eras {}",
        r.buildings.len(), r.units.len(), r.technologies.len(), r.terrains.len(),
        r.resources.len(), r.improvements.len(), r.policies.len(), r.policy_branches.len(),
        r.promotions.len(), r.nations.len(), r.beliefs.len(), r.eras.len(),
    );
    println!("total objects: {}", r.object_count());

    // Counts cross-checked against the icon coverage test, which walks the same
    // files by a completely different route.
    assert_eq!(r.buildings.len(), 124);
    assert_eq!(r.units.len(), 127);
    assert_eq!(r.technologies.len(), 80);
    assert_eq!(r.promotions.len(), 106);
    assert_eq!(r.nations.len(), 83);
    assert_eq!(r.improvements.len(), 35);
    assert_eq!(r.resources.len(), 35);
    assert!(r.object_count() > 700);
}

#[test]
fn loads_vanilla_too() {
    let r = load(VANILLA);
    assert!(r.buildings.len() > 80, "{}", r.buildings.len());
    assert!(r.technologies.len() > 60);
}

#[test]
fn names_resolve_to_ids_and_back() {
    let r = load(GK);
    let id = r.building("Library").expect("Library exists");
    assert_eq!(r.buildings.name_of(id), Some("Library"));
    assert_eq!(r.buildings[id].name, "Library");
    assert_eq!(r.building("Not A Building"), None);
}

#[test]
fn fields_survive_the_round_trip() {
    let r = load(GK);
    let lib = &r.buildings[r.building("Library").unwrap()];
    assert_eq!(lib.required_tech.as_deref(), Some("Writing"));
    assert!(!lib.uniques.is_empty(), "Library carries uniques");

    let grassland = &r.terrains[r.terrain("Grassland").unwrap()];
    assert_eq!(grassland.r#type, "Land");
    assert_eq!(grassland.food, Some(2));

    let warrior = &r.units[r.unit("Warrior").unwrap()];
    assert_eq!(warrior.unit_type, "Sword");
    assert_eq!(warrior.movement, 2);
    assert_eq!(warrior.strength, Some(8));
}

#[test]
fn uniques_are_parsed_not_left_as_strings() {
    let r = load(GK);
    let total: usize = r.buildings.values().map(|b| b.uniques.len()).sum();
    let known = r
        .buildings
        .values()
        .flat_map(|b| &b.uniques)
        .filter(|u| matches!(u.kind, UniqueKind::Known(_)))
        .count();
    println!("building uniques: {known}/{total} resolved to a UniqueType");
    assert!(total > 150);
    assert_eq!(known, total, "every building unique should be a declared type");
}

#[test]
fn decimals_load_without_float_error() {
    let r = load(GK);
    // Difficulties carry the fractional modifiers; they must be exact.
    let chieftain = r.difficulties.by_name("Chieftain").expect("Chieftain exists");
    let m = chieftain.research_cost_modifier;
    assert_eq!(m, Fixed::parse("0.95").unwrap(), "got {m}");
    assert_eq!(m.raw(), 950, "exact, not the nearest double");
}

#[test]
fn cross_references_resolve() {
    let r = load(GK);
    let dangling = r.dangling_refs();
    for d in dangling.iter().take(10) {
        println!("{d}");
    }
    println!("{} dangling references", dangling.len());
    assert!(dangling.is_empty(), "{} unresolved names in the shipped ruleset", dangling.len());
}

// ---------------------------------------------------------------------------
// Relationships the json states as structure or position, not as fields.
// Schema inference cannot see these; `Ruleset::derive` establishes them.

#[test]
fn era_numbers_follow_load_order() {
    let r = load(GK);
    let mut seen: Vec<(i32, &str)> =
        r.eras.iter().map(|(id, e)| (e.era_number, r.eras.name_of(id).unwrap())).collect();
    seen.sort();
    println!("{seen:?}");
    assert_eq!(seen.first().map(|(_, n)| *n), Some("Ancient era"));
    assert_eq!(
        seen.iter().map(|(n, _)| *n).collect::<Vec<_>>(),
        (0..r.eras.len() as i32).collect::<Vec<_>>(),
        "era numbers must be a dense 0..n in file order"
    );
}

#[test]
fn techs_know_their_column_and_era() {
    let r = load(GK);
    assert!(r.tech_columns.len() >= 15, "columns: {}", r.tech_columns.len());
    let archery = r.technologies.by_name("Archery").expect("Archery exists");
    assert_eq!(archery.era_name(&r), Some("Ancient era"));
    assert!(archery.era(&r).is_some(), "the era name must resolve to an era");

    // Every tech must reach an era — this is the relationship that was missing.
    let orphans: Vec<_> = r
        .technologies
        .iter()
        .filter(|(_, t)| t.era(&r).is_none())
        .map(|(id, _)| r.technologies.name_of(id).unwrap())
        .collect();
    assert!(orphans.is_empty(), "techs with no era: {orphans:?}");
}

#[test]
fn policies_know_their_branch() {
    let r = load(GK);
    let orphans: Vec<_> = r
        .policies
        .iter()
        .filter(|(_, p)| p.branch.is_none())
        .map(|(id, _)| r.policies.name_of(id).unwrap())
        .collect();
    assert!(orphans.is_empty(), "policies with no branch: {orphans:?}");
    let tradition = r.policies.by_name("Aristocracy").expect("Aristocracy exists");
    assert_eq!(tradition.branch_name(&r), Some("Tradition"));
}

#[test]
fn damaging_terrain_carries_its_damage() {
    let r = load(GK);
    let damaging: Vec<(&str, i32)> = r
        .terrains
        .iter()
        .filter(|(_, t)| t.damage_per_turn != 0)
        .map(|(id, t)| (r.terrains.name_of(id).unwrap(), t.damage_per_turn))
        .collect();
    println!("{damaging:?}");
    assert!(!damaging.is_empty(), "some terrain damages units");
}

#[test]
fn sort_group_orders_by_unlocking_era() {
    use civ_core::ruleset::SortGroup;
    let r = load(GK);
    let unit = |n: &str| r.units.by_name(n).unwrap_or_else(|| panic!("{n} exists")).sort_group(&r);

    let (archer, musketman, infantry) = (unit("Archer"), unit("Musketman"), unit("Infantry"));
    println!("Archer {archer}  Musketman {musketman}  Infantry {infantry}");
    assert!(archer < musketman, "{archer} !< {musketman}");
    assert!(musketman < infantry, "{musketman} !< {infantry}");

    // Warrior unlocks at Agriculture, the first tech, so it sorts first.
    assert_eq!(unit("Warrior"), 0);

    // A unit needing no tech has nothing to sort by and goes last.
    let (id, untechd) = r
        .units
        .iter()
        .find(|(_, u)| u.required_tech.is_none())
        .expect("some units unlock without a tech");
    println!("no required tech: {}", r.units.name_of(id).unwrap());
    assert_eq!(untechd.sort_group(&r), 100);
}

#[test]
fn costs_are_derived_from_tech_columns() {
    let r = load(GK);

    let techs_priced = r.technologies.values().filter(|t| t.cost.unwrap_or(0) > 0).count();
    let buildings_priced = r.buildings.values().filter(|b| b.cost.unwrap_or(0) > 0).count();
    println!(
        "techs priced {}/{}, buildings priced {}/{}",
        techs_priced,
        r.technologies.len(),
        buildings_priced,
        r.buildings.len()
    );

    // Only 61 of 224 buildings state a cost in the json across both rulesets;
    // the rest inherit their tech column's. Anything left unpriced should be a
    // building that genuinely cannot be built.
    let unpriced: Vec<&str> = r
        .buildings
        .values()
        .filter(|b| b.cost.unwrap_or(0) <= 0)
        .map(|b| b.name.as_str())
        .collect();
    println!("unpriced buildings: {unpriced:?}");

    assert_eq!(techs_priced, r.technologies.len(), "every tech must have a cost");
    assert!(buildings_priced > 100, "only {buildings_priced} buildings priced");

    // The five above are Palace (free in the capital) and the faith-purchased
    // religious buildings — none of them are produced, which is why Unciv skips
    // unbuildables when filling costs in.
    assert_eq!(unpriced.len(), 5, "{unpriced:?}");

    // A wonder reads `wonderCost` and an ordinary building `buildingCost` from
    // the same column, so within one column the wonder is dearer. Compare a
    // real pair rather than assuming which wonders this ruleset ships.
    let wonder = r
        .buildings
        .values()
        .find(|b| b.is_wonder == Some(true) && b.required_tech.is_some() && b.cost.is_some())
        .expect("the ruleset has wonders");
    let tech = wonder.required_tech.clone().unwrap();
    let peer = r
        .buildings
        .values()
        .find(|b| b.is_wonder != Some(true) && b.required_tech.as_ref() == Some(&tech) && b.cost.is_some());
    println!("{} ({:?}) vs peer {:?}", wonder.name, wonder.cost, peer.map(|p| (&p.name, p.cost)));
    if let Some(peer) = peer {
        assert!(wonder.cost.unwrap() > peer.cost.unwrap(), "wonders cost more than their column's buildings");
    }
}

#[test]
fn global_uniques_are_loaded() {
    let r = load(GK);
    println!("{} global uniques", r.global_uniques.len());
    for u in &r.global_uniques {
        println!("  {}", u.text);
    }
    assert!(!r.global_uniques.is_empty(), "GlobalUniques.json must be read");
    // The unhappiness penalties live here and nowhere else.
    assert!(
        r.global_uniques.iter().any(|u| u.conditionals.iter().any(|c| c.text.contains("Happiness"))),
        "the happiness thresholds should be among them"
    );
}

/// The year table is nested structure, not a field, so schema inference reads
/// it as opaque and the loader has to pick the number out by hand.
#[test]
fn a_speed_knows_how_long_its_game_runs() {
    let r = load(GK);
    let standard = r.speeds.by_name("Standard").expect("Standard exists");
    assert_eq!(standard.total_turns, 500, "the last untilTurn in the year table");
    let quick = r.speeds.by_name("Quick").expect("Quick exists");
    assert!(quick.total_turns < standard.total_turns, "a quick game is shorter");
    assert!(
        r.speeds.values().all(|s| s.total_turns > 0),
        "every speed needs a length, or unit upkeep divides by zero"
    );
}
