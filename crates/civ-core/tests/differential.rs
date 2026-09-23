//! Compare this engine's answers against Unciv's, value by value.
//!
//! Reading both implementations and concluding they agree is how subtle ports
//! go wrong. This reads Unciv's own output instead.
//!
//! The reference file is produced by the Kotlin side:
//!
//! ```text
//! cd ../Unciv
//! ./gradlew :tests:test --tests '*DifferentialDump*'
//! ```
//!
//! which writes `tests/data/unciv_ruleset.json`. Without it these tests skip
//! rather than fail, so the suite still runs on a machine with no JDK — but a
//! skip is reported loudly, because a differential that quietly does nothing is
//! worse than none.

use civ_core::json::Json;
use civ_core::ruleset::{queries::SortGroup, Ruleset};
use std::path::Path;

const REFERENCE: &str = "tests/data/unciv_ruleset.json";
const GK: &str = "rules/gods-and-kings";

/// Unciv's answers, or `None` if the dump has not been generated.
fn reference() -> Option<Json> {
    let p = Path::new(REFERENCE);
    if !p.exists() {
        eprintln!(
            "\n  SKIPPED: {REFERENCE} is absent.\n  \
             Generate it with:  ./gradlew :tests:test --tests '*DifferentialDump*'\n"
        );
        return None;
    }
    Some(Json::parse(&std::fs::read_to_string(p).expect("readable")).expect("the dump parses"))
}

fn ours() -> Ruleset {
    Ruleset::load_dir(GK).expect("readable").expect("loads")
}

/// Collect `(name, value)` for one field of one section of the reference.
fn field<'a>(doc: &'a Json, section: &str, key: &str) -> Vec<(String, Option<&'a Json>)> {
    doc.get(section)
        .and_then(|s| s.as_array())
        .map(|rows| {
            rows.iter()
                .filter_map(|row| {
                    let name = row.get("name")?.as_str()?.to_string();
                    Some((name, row.get(key)))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Report every disagreement, not just the first — one mismatch usually means
/// a whole category is wrong, and seeing all of them says which.
fn compare<T: std::fmt::Debug + PartialEq>(what: &str, rows: Vec<(String, Option<T>, Option<T>)>) {
    compare_at_least(what, rows, 8)
}

/// As [`compare`], but for sections that are legitimately small — a radius-4
/// map has only five rings. The floor exists to catch a dump that produced
/// nothing, so it has to be stated per section rather than guessed.
fn compare_at_least<T: std::fmt::Debug + PartialEq>(
    what: &str,
    rows: Vec<(String, Option<T>, Option<T>)>,
    minimum: usize,
) {
    let mut wrong = Vec::new();
    for (name, theirs, ours) in &rows {
        if theirs != ours {
            wrong.push(format!("  {name}: unciv {theirs:?}, ours {ours:?}"));
        }
    }
    println!("{what}: {} compared, {} differ", rows.len(), wrong.len());
    assert!(
        rows.len() >= minimum,
        "{what}: only {} rows, expected at least {minimum} — the dump looks empty",
        rows.len()
    );
    assert!(wrong.is_empty(), "{what} disagrees:\n{}", wrong.join("\n"));
}

#[test]
fn building_costs_match() {
    let Some(doc) = reference() else { return };
    let r = ours();
    let rows = field(&doc, "buildings", "cost")
        .into_iter()
        .map(|(name, theirs)| {
            let mine = r.buildings.by_name(&name).and_then(|b| b.cost);
            // Unciv leaves unbuildable things at -1; we leave them absent.
            let theirs = theirs.and_then(|v| v.as_i32()).filter(|c| *c > 0);
            (name, theirs, mine.filter(|c| *c > 0))
        })
        .collect();
    compare("building costs", rows);
}

#[test]
fn tech_costs_match() {
    let Some(doc) = reference() else { return };
    let r = ours();
    let rows = field(&doc, "technologies", "cost")
        .into_iter()
        .map(|(name, theirs)| {
            let mine = r.technologies.by_name(&name).and_then(|t| t.cost);
            (name, theirs.and_then(|v| v.as_i32()), mine)
        })
        .collect();
    compare("tech costs", rows);
}

/// The relationship that json states only as nesting.
#[test]
fn every_tech_lands_in_the_same_era() {
    let Some(doc) = reference() else { return };
    let r = ours();
    let rows = field(&doc, "technologies", "era")
        .into_iter()
        .map(|(name, theirs)| {
            let mine = r.technologies.by_name(&name).and_then(|t| t.era_name(&r)).map(String::from);
            (name, theirs.and_then(|v| v.as_str()).map(String::from), mine)
        })
        .collect();
    compare("tech eras", rows);
}

/// The relationship that json states only as position.
#[test]
fn era_numbers_match() {
    let Some(doc) = reference() else { return };
    let r = ours();
    let rows = field(&doc, "eras", "eraNumber")
        .into_iter()
        .map(|(name, theirs)| {
            let mine = r.eras.by_name(&name).map(|e| e.era_number);
            (name, theirs.and_then(|v| v.as_i32()), mine)
        })
        .collect();
    compare("era numbers", rows);
}

#[test]
fn policies_land_in_the_same_branch() {
    let Some(doc) = reference() else { return };
    let r = ours();
    let rows = field(&doc, "policies", "branch")
        .into_iter()
        .map(|(name, theirs)| {
            let mine = r.policies.by_name(&name).and_then(|p| p.branch_name(&r)).map(String::from);
            (name, theirs.and_then(|v| v.as_str()).map(String::from), mine)
        })
        .collect();
    compare("policy branches", rows);
}

#[test]
fn terrain_movement_and_defence_match() {
    let Some(doc) = reference() else { return };
    let r = ours();

    let movement = field(&doc, "terrains", "movementCost")
        .into_iter()
        .map(|(name, theirs)| {
            // `movementCost` defaults to 1 in Unciv and is absent here for
            // the natural wonders that never state it. Compare the *effective*
            // cost, which is what movement actually uses.
            let mine = r.terrains.by_name(&name).map(|t| t.movement_cost.unwrap_or(1));
            (name, theirs.and_then(|v| v.as_i32()), mine)
        })
        .collect();
    compare("terrain movement", movement);

    // Defence is fractional; compare in thousandths so the exactness shows.
    let defence = field(&doc, "terrains", "defenceBonus")
        .into_iter()
        .map(|(name, theirs)| {
            let mine = r.terrains.by_name(&name).and_then(|t| t.defence_bonus).map(|f| f.raw()).unwrap_or(0);
            let t = theirs.and_then(|v| v.as_fixed()).map(|f| f.raw()).unwrap_or(0);
            (name, Some(t), Some(mine))
        })
        .collect();
    compare("terrain defence", defence);
}

#[test]
fn terrain_override_and_damage_match() {
    let Some(doc) = reference() else { return };
    let r = ours();

    let overrides = field(&doc, "terrains", "overrideStats")
        .into_iter()
        .map(|(name, theirs)| {
            let mine = r.terrains.by_name(&name).and_then(|t| t.override_stats).unwrap_or(false);
            (name, theirs.and_then(|v| v.as_bool()).or(Some(false)), Some(mine))
        })
        .collect();
    compare("terrain overrideStats", overrides);

    // Derived from the DamagesContainingUnits uniques rather than read.
    let damage = field(&doc, "terrains", "damagePerTurn")
        .into_iter()
        .map(|(name, theirs)| {
            let mine = r.terrains.by_name(&name).map(|t| t.damage_per_turn);
            (name, theirs.and_then(|v| v.as_i32()), mine)
        })
        .collect();
    compare("terrain damage per turn", damage);
}

#[test]
fn unit_stats_match() {
    let Some(doc) = reference() else { return };
    let r = ours();

    for key in ["cost", "movement", "strength"] {
        let rows = field(&doc, "units", key)
            .into_iter()
            .map(|(name, theirs)| {
                let u = r.units.by_name(&name);
                let mine = match key {
                    "cost" => u.and_then(|u| u.cost),
                    "movement" => u.map(|u| u.movement),
                    _ => u.and_then(|u| u.strength),
                };
                // Unciv defaults an absent number to 0, and marks "cannot be
                // built" with -1; this crate leaves both absent. Great People
                // and Missionaries are the -1 cases.
                let t = theirs.and_then(|v| v.as_i32()).filter(|v| *v > 0);
                (name, t, mine.filter(|v| *v > 0))
            })
            .collect();
        compare(&format!("unit {key}"), rows);
    }
}

/// Sort groups exercise the whole chain: unit to tech to column to era number.
#[test]
fn unit_sort_groups_match() {
    let Some(doc) = reference() else { return };
    let r = ours();
    let by_tech: std::collections::HashMap<String, i32> = doc
        .get("technologies")
        .and_then(|s| s.as_array())
        .map(|rows| {
            rows.iter()
                .filter_map(|row| Some((row.get("name")?.as_str()?.to_string(), row.get("eraNumber")?.as_i32()?)))
                .collect()
        })
        .unwrap_or_default();

    let rows = field(&doc, "units", "requiredTech")
        .into_iter()
        .map(|(name, tech)| {
            let expected = tech.and_then(|v| v.as_str()).and_then(|t| by_tech.get(t).copied()).unwrap_or(100);
            let mine = r.units.by_name(&name).map(|u| u.sort_group(&r)).unwrap_or(100);
            (name, Some(expected), Some(mine))
        })
        .collect();
    compare("unit sort groups", rows);
}

// ---------------------------------------------------------------------------
// Traces: the same inputs driven through both engines, compared step by step.
//
// The comparisons above check that the two agree about the rules. These check
// that they agree about what happens when the rules are *run* — which is where
// an off-by-one in a loop, or a rounding applied at the wrong moment, shows up.

use civ_core::stats::{Fixed, Stat, Stats};
use civ_core::world::city::{apply_food, food_to_next_population, City, GrowthEvent, GrowthModifiers};
use civ_core::world::grid::{CivId, TileGrid, TileId};
use civ_core::world::yields::tile_yield;

const TRACES: &str = "tests/data/unciv_traces.json";

fn traces() -> Option<Json> {
    let p = Path::new(TRACES);
    if !p.exists() {
        eprintln!(
            "\n  SKIPPED: {TRACES} is absent.\n  \
             Generate it with:  ./gradlew :tests:test --tests '*DifferentialDump*'\n"
        );
        return None;
    }
    Some(Json::parse(&std::fs::read_to_string(p).expect("readable")).expect("the dump parses"))
}

/// The modifiers Unciv used, so the trace is driven from the same inputs
/// rather than from an assumption about them.
fn trace_modifiers(doc: &Json) -> GrowthModifiers {
    GrowthModifiers {
        speed: doc.get("speedModifier").and_then(|v| v.as_fixed()).unwrap_or(Fixed::ONE),
        // `isHuman` is dumped as true, and Unciv applies its growth modifier
        // only to non-human civs, so there is nothing to apply here.
        civ: Fixed::ONE,
    }
}

#[test]
fn tile_yields_match_for_every_terrain_combination() {
    let Some(doc) = traces() else { return };
    let r = ours();

    let mut rows = Vec::new();
    for entry in doc.get("tileYields").and_then(|v| v.as_array()).unwrap_or_default() {
        let Some(name) = entry.get("terrain").and_then(|v| v.as_str()) else { continue };
        let parts: Vec<&str> = name.split('+').collect();

        let Some(base) = r.terrain(parts[0]) else { continue };
        let mut grid = TileGrid::new(1, 1, base);
        for feature in &parts[1..] {
            if let Some(f) = r.terrain(feature) {
                grid.features[0].push(f);
            }
        }
        let mine = tile_yield(&grid, &r, TileId(0));

        let mut theirs = Stats::ZERO;
        for (stat, key) in [
            (Stat::Food, "food"),
            (Stat::Production, "production"),
            (Stat::Gold, "gold"),
            (Stat::Science, "science"),
            (Stat::Culture, "culture"),
            (Stat::Faith, "faith"),
        ] {
            theirs[stat] = entry.get(key).and_then(|v| v.as_fixed()).unwrap_or(Fixed::ZERO);
        }
        rows.push((name.to_string(), Some(theirs.to_string()), Some(mine.to_string())));
    }
    compare("tile yields", rows);
}

#[test]
fn growth_thresholds_match() {
    let Some(doc) = traces() else { return };
    let m = trace_modifiers(&doc);

    let rows = doc
        .get("growthThresholds")
        .and_then(|v| v.as_array())
        .unwrap_or_default()
        .iter()
        .filter_map(|e| {
            let pop = e.get("population")?.as_i32()? as u32;
            let theirs = e.get("foodRequired")?.as_i32()?;
            Some((format!("pop {pop}"), Some(theirs), Some(food_to_next_population(pop, m))))
        })
        .collect();
    compare("growth thresholds", rows);
}

/// Sixty turns of scripted feast and famine, compared turn by turn.
///
/// This is the one that would catch a wrong rounding or an off-by-one in the
/// growth loop — a threshold that agrees at rest can still disagree once it is
/// being crossed repeatedly with food carried over.
#[test]
fn the_growth_trace_matches_turn_by_turn() {
    let Some(doc) = traces() else { return };
    let m = trace_modifiers(&doc);

    let mut city = City::found("Trace", CivId(0), TileId(0));
    let mut rows = Vec::new();

    for entry in doc.get("growthTrace").and_then(|v| v.as_array()).unwrap_or_default() {
        let (Some(turn), Some(food)) =
            (entry.get("turn").and_then(|v| v.as_i32()), entry.get("food").and_then(|v| v.as_i32()))
        else {
            continue;
        };
        let theirs = (
            entry.get("population").and_then(|v| v.as_i32()).unwrap_or(-1),
            entry.get("stored").and_then(|v| v.as_i32()).unwrap_or(-1),
        );

        let event = apply_food(&mut city, food, m);
        let mine = (city.population as i32, city.food_stored);

        rows.push((
            format!(
                "turn {turn} (food {food:+}, {})",
                match event {
                    GrowthEvent::Grew { .. } => "grew",
                    GrowthEvent::Starved { .. } => "starved",
                    GrowthEvent::Held { .. } => "held",
                    GrowthEvent::Stored { .. } => "stored",
                }
            ),
            Some(theirs),
            Some(mine),
        ));
    }
    compare("growth trace (population, stored)", rows);
}

// ---------------------------------------------------------------------------
// Costs and combat.

use civ_core::stats::Precise;
use civ_core::world::combat::{
    attacking_strength, damage_modifier_precise, defending_strength, health_ratio, strength_ratio,
};
use civ_core::world::construction::{production_cost, Construction, CostModifiers};
use civ_core::world::entities::{Civilization, Unit};
use civ_core::world::research::{tech_cost, MapScaling, ScienceModifiers};
use civ_core::world::resources::recompute_stock;
use civ_core::world::world::World;

const COMBAT: &str = "tests/data/unciv_combat.json";

fn combat_dump() -> Option<Json> {
    let p = Path::new(COMBAT);
    if !p.exists() {
        eprintln!(
            "\n  SKIPPED: {COMBAT} is absent.\n  \
             Generate it with:  ./gradlew :tests:test --tests '*DifferentialDump*'\n"
        );
        return None;
    }
    Some(Json::parse(&std::fs::read_to_string(p).expect("readable")).expect("the dump parses"))
}

/// Read the scenario's modifiers out of the dump and out of the ruleset, rather
/// than assuming the defaults happen to match. The difficulty is named in the
/// dump; its numbers come from the same json both engines read.
fn scenario_modifiers(doc: &Json, r: &Ruleset) -> (CostModifiers, ScienceModifiers) {
    let speed_prod = doc.get("productionCostModifier").and_then(|v| v.as_fixed()).unwrap_or(Fixed::ONE);
    let speed_sci = doc.get("scienceCostModifier").and_then(|v| v.as_fixed()).unwrap_or(Fixed::ONE);
    let speed_obj = doc.get("speed").and_then(|v| v.as_str()).and_then(|n| r.speeds.by_name(n));
    let difficulty = doc.get("difficulty").and_then(|v| v.as_str()).unwrap_or("Prince");
    let d = r.difficulties.by_name(difficulty);

    (
        CostModifiers {
            speed: speed_prod,
            civ: d.map_or(Fixed::ONE, |d| d.building_cost_modifier),
            cities: doc.get("cities").and_then(|v| v.as_i32()).unwrap_or(1) as usize,
            game_length: speed_obj.map_or(500, |s| s.total_turns),
            religious_pressure: speed_obj.map_or(6, |s| s.religious_pressure_adjacent_city),
            gold_gift_modifier: speed_obj.map_or(Fixed::ONE, |s| s.gold_gift_modifier),
            gold_cost: speed_obj.map_or(Fixed::ONE, |s| s.gold_cost_modifier),
            faith_cost: speed_obj.map_or(Fixed::ONE, |s| s.faith_cost_modifier),
            tribute_interval: speed_obj.map_or(Fixed::permille(6_500), |s| s.city_state_tribute_scaling_interval),
            deal_duration: speed_obj.map_or(30, |s| s.deal_duration.max(1) as u32),
            golden_age_length: speed_obj.map_or(Fixed::ONE, |s| s.golden_age_length_modifier),
        },
        ScienceModifiers {
            difficulty: d.map_or(Fixed::ONE, |d| d.research_cost_modifier),
            speed: speed_sci,
            // The test map is a radius-6 hexagon, below Unciv's Medium, so the
            // tech multiplier is one — which the Agriculture row confirms.
            map: MapScaling::TINY,
        },
    )
}

#[test]
fn building_production_costs_match() {
    let Some(doc) = combat_dump() else { return };
    let r = ours();
    let (costs, _) = scenario_modifiers(&doc, &r);

    let rows = field(&doc, "buildingProductionCosts", "cost")
        .into_iter()
        .map(|(name, theirs)| {
            let mine = r.building(&name).and_then(|id| production_cost(&r, Construction::Building(id), costs));
            (name, theirs.and_then(|v| v.as_i32()).filter(|c| *c > 0), mine)
        })
        .collect();
    compare("building production costs", rows);
}

#[test]
fn unit_production_costs_match() {
    let Some(doc) = combat_dump() else { return };
    let r = ours();
    let (costs, _) = scenario_modifiers(&doc, &r);

    let rows = field(&doc, "unitProductionCosts", "cost")
        .into_iter()
        .map(|(name, theirs)| {
            let mine = r.unit(&name).and_then(|id| production_cost(&r, Construction::Unit(id), costs));
            (name, theirs.and_then(|v| v.as_i32()).filter(|c| *c > 0), mine)
        })
        .collect();
    compare("unit production costs", rows);
}

#[test]
fn researched_tech_costs_match() {
    let Some(doc) = combat_dump() else { return };
    let r = ours();
    let (_, science) = scenario_modifiers(&doc, &r);
    let cities = doc.get("cities").and_then(|v| v.as_i32()).unwrap_or(1) as usize;

    let rows = field(&doc, "techCosts", "cost")
        .into_iter()
        .map(|(name, theirs)| {
            let mine = r.technologies.id(&name).and_then(|t| tech_cost(&r, t, cities, science));
            (name, theirs.and_then(|v| v.as_i32()), mine)
        })
        .collect();
    compare("researched tech costs", rows);
}

/// The damage curve across a grid of matchups, at fixed randomness.
///
/// Randomness is dictated rather than rolled, so this compares the formula
/// instead of comparing two generators — the generators are checked separately
/// by the determinism fingerprint.
///
/// Strengths are this engine's own, computed from the same scenario Unciv ran:
/// two civs, one unit each, no resource tiles. That puts both civs one short of
/// whatever their unit consumes, which is why a Swordsman fights at 10.5
/// instead of 14 — `MISSING_RESOURCES_MALUS` is -25%.
///
/// Reconstructing the scenario rather than reading Unciv's strengths back is
/// the stronger test: it checks the strength calculation and the damage curve
/// together, which is what a battle actually uses.
#[test]
fn combat_damage_matches_across_the_matchup_grid() {
    let Some(doc) = combat_dump() else { return };
    let r = ours();

    let mut rows = Vec::new();
    for e in doc.get("combat").and_then(|v| v.as_array()).unwrap_or_default() {
        let (Some(att), Some(def)) =
            (e.get("attacker").and_then(|v| v.as_str()), e.get("defender").and_then(|v| v.as_str()))
        else {
            continue;
        };
        let health = e.get("attackerHealth").and_then(|v| v.as_i32()).unwrap_or(100) as u8;
        let rnd = e.get("randomness").and_then(|v| v.as_fixed()).unwrap_or(Fixed::ZERO);
        // Rebuild Unciv's scenario: bare grassland, one unit each, no
        // resources in either territory.
        let Some(base_a) = r.unit(att) else { continue };
        let Some(base_d) = r.unit(def) else { continue };
        let grid = TileGrid::new(4, 4, r.terrain("Grassland").unwrap());
        let nation = r.nations.ids().next().unwrap();
        let mut w = World::new(
            grid,
            vec![Civilization::new(CivId(0), nation, false), Civilization::new(CivId(1), nation, false)],
        );
        let ua = w.place_unit(Unit::new(base_a, CivId(0), TileId(0), 2));
        let ud = w.place_unit(Unit::new(base_d, CivId(1), TileId(1), 2));
        for c in [CivId(0), CivId(1)] {
            let stock = recompute_stock(&w, &r, c);
            if let Some(civ) = w.civ_mut(c) {
                civ.resources = stock;
            }
        }

        let (Some(a), Some(d)) = (attacking_strength(&w, &r, ua), defending_strength(&w, &r, ud)) else {
            continue;
        };

        // Check the strengths themselves, not only what they produce.
        let (their_a, their_d) =
            (e.get("attackStrength").and_then(|v| v.as_fixed()), e.get("defendStrength").and_then(|v| v.as_fixed()));
        if their_a != Some(a) || their_d != Some(d) {
            rows.push((
                format!("{att} vs {def} STRENGTH"),
                Some((their_a.map_or(-1, |f| f.raw()), their_d.map_or(-1, |f| f.raw()))),
                Some((a.raw(), d.raw())),
            ));
            continue;
        }

        let ratio = strength_ratio(a, d);
        // Kept wide until the last step, matching how Unciv holds a float
        // through the whole expression and rounds once.
        let to_defender =
            (damage_modifier_precise(ratio, false, rnd) * Precise::from_fixed(health_ratio(health))).round();
        // The defender is at full health throughout, so its output is unscaled.
        let to_attacker = (damage_modifier_precise(ratio, true, rnd) * Precise::from_fixed(health_ratio(100))).round();

        let theirs = (
            e.get("toDefender").and_then(|v| v.as_i64()).unwrap_or(-1),
            e.get("toAttacker").and_then(|v| v.as_i64()).unwrap_or(-1),
        );
        rows.push((format!("{att}({health}hp) vs {def} @ {rnd}"), Some(theirs), Some((to_defender, to_attacker))));
    }
    compare("combat damage", rows);
}

/// The strengths themselves, independently of the damage they produce.
///
/// This began as an inventory of what was missing — a Swordsman defending at
/// 10.5 against a base of 14 — and is now a check that the reason is modelled.
#[test]
fn combat_strengths_match() {
    let Some(doc) = combat_dump() else { return };
    let r = ours();

    let mut seen: std::collections::BTreeMap<String, (Fixed, Fixed)> = Default::default();
    for e in doc.get("combat").and_then(|v| v.as_array()).unwrap_or_default() {
        let (Some(def), Some(theirs)) =
            (e.get("defender").and_then(|v| v.as_str()), e.get("defendStrength").and_then(|v| v.as_fixed()))
        else {
            continue;
        };
        let Some(base) = r.units.by_name(def).and_then(|u| u.strength).map(Fixed::whole) else {
            continue;
        };
        seen.insert(def.to_string(), (base, theirs));
    }
    for (unit, (base, effective)) in &seen {
        let note = if base == effective { "" } else { "  (short of its resource)" };
        println!("  {unit}: base {base}, fights at {effective}{note}");
    }
    // Iron-hungry units are penalised; the rest are not.
    assert_eq!(seen.get("Swordsman").map(|(_, e)| *e), Some(Fixed::permille(10_500)));
    assert_eq!(seen.get("Warrior").map(|(b, e)| b == e), Some(true));
}

// ---------------------------------------------------------------------------
// Map geometry.
//
// Unciv addresses hexes as `HexCoord(x, y)`; this crate uses odd-r offsets on a
// rectangle. The labels differ and the map shapes differ, so tiles cannot be
// matched one to one. What must agree is the *geometry*: how many tiles sit at
// each distance, and how many neighbours a tile has.
//
// Worth recording why this is checked against raw coordinates. Unciv's
// `HexMath.getRow`/`getColumn` are layout helpers and are not injective —
// `row = (x+y)/2` truncates toward zero, so `x+y` of -1 and of 0 both give row
// zero. Keying tiles by them merges four of sixty-one on a radius-4 map, and a
// port that used them as identities would lose those tiles with no error at all.

use civ_core::world::grid::Direction;
use std::collections::BTreeMap;

const HEXES: &str = "tests/data/unciv_hexes.json";

fn hexes() -> Option<Json> {
    let p = Path::new(HEXES);
    if !p.exists() {
        eprintln!("\n  SKIPPED: {HEXES} is absent.\n");
        return None;
    }
    Some(Json::parse(&std::fs::read_to_string(p).expect("readable")).expect("parses"))
}

/// The reference is a hex grid: every ring holds `6r` tiles.
#[test]
fn uncivs_map_has_the_geometry_a_hex_grid_must_have() {
    let Some(doc) = hexes() else { return };
    let tiles = doc.get("tiles").and_then(|v| v.as_array()).unwrap_or_default();

    let mut by_distance: BTreeMap<i32, usize> = BTreeMap::new();
    let mut ids = std::collections::BTreeSet::new();
    for t in tiles {
        let (Some(x), Some(y)) = (t.get("x").and_then(|v| v.as_i32()), t.get("y").and_then(|v| v.as_i32())) else {
            continue;
        };
        ids.insert((x, y));
        if let Some(d) = t.get("distanceToOrigin").and_then(|v| v.as_i32()) {
            *by_distance.entry(d).or_insert(0) += 1;
        }
    }
    assert_eq!(ids.len(), tiles.len(), "raw coordinates must identify tiles uniquely");

    println!("Unciv rings: {by_distance:?}");
    for (distance, count) in &by_distance {
        let expected = if *distance == 0 { 1 } else { 6 * *distance as usize };
        assert_eq!(*count, expected, "ring {distance} should hold {expected}");
    }
}

/// And this crate's grid must have the same geometry in its interior.
#[test]
fn our_grid_matches_that_geometry() {
    let Some(doc) = hexes() else { return };
    let r = ours();

    let reference: BTreeMap<i32, usize> = doc
        .get("tiles")
        .and_then(|v| v.as_array())
        .unwrap_or_default()
        .iter()
        .filter_map(|t| t.get("distanceToOrigin").and_then(|v| v.as_i32()))
        .fold(BTreeMap::new(), |mut acc, d| {
            *acc.entry(d).or_insert(0) += 1;
            acc
        });

    // Large enough that a radius-4 neighbourhood is nowhere near an edge.
    let grid = TileGrid::new(24, 24, r.terrain("Grassland").unwrap());
    let centre = grid.id_at(12, 12).unwrap();

    let mut rows = Vec::new();
    for (distance, count) in &reference {
        rows.push((format!("ring {distance}"), Some(*count), Some(grid.ring(centre, *distance as u32).len())));
    }
    compare_at_least("hex ring sizes", rows, 5);

    // Interior degree, against Unciv's own interior tiles.
    let their_interior = doc
        .get("tiles")
        .and_then(|v| v.as_array())
        .unwrap_or_default()
        .iter()
        .filter(|t| t.get("neighbours").and_then(|v| v.as_array()).map(|n| n.len()) == Some(6))
        .count();
    assert_eq!(their_interior, 37, "a radius-4 hexagon has 37 interior tiles");
    assert!(
        grid.ids().all(|t| {
            let (c, rw) = grid.coords(t);
            let edge = c == 0 || rw == 0 || c == grid.width() - 1 || rw == grid.height() - 1;
            edge || grid.neighbours(t).count() == 6
        }),
        "every non-edge tile must have six neighbours"
    );

    // And the directions must be genuinely opposite, both grids agreeing that a
    // hex has three axes.
    for d in Direction::ALL {
        assert_eq!(d.opposite().opposite(), d);
        assert_ne!(d.opposite(), d);
    }
}
