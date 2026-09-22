//! One number that summarises a whole simulation.
//!
//! The crate's central claim is that it computes the same answers on a native
//! server and in a browser. That claim is worth nothing unverified, and
//! comparing two engines field by field is impractical, so instead a fixed
//! scenario runs every layer — parsing, fixed-point arithmetic, powers,
//! modifier composition, yields, growth, movement, combat, the generator — and
//! folds every observable into a single `u64`.
//!
//! Native and `wasm32` must produce the same value. If one bit of one
//! calculation differs anywhere, this number differs, and the difference is
//! caught by a build rather than by a desynced multiplayer game forty turns in.
//!
//! The ruleset is embedded rather than read from disk, because `wasm32` has no
//! filesystem — and because that makes the parser part of what is checked.

use crate::json::Json;
use crate::modifiers::{Modifier, ModifierStack, Source, SourceKind};
use crate::rng::Rng;
use crate::ruleset::Ruleset;
use crate::stats::{Fixed, Stat};
use crate::world::city::{apply_food, food_to_next_population, City, GrowthEvent, GrowthModifiers};
use crate::world::combat::{damage_modifier, health_ratio};
use crate::world::grid::{CivId, TileGrid, TileId};
use crate::world::movement::reachable;
use crate::world::yields::tile_yield;

const RULESET: &str = "../android/assets/jsons/Civ V - Gods & Kings/";

macro_rules! embed {
    ($($f:literal),* $(,)?) => {
        &[$(($f, include_str!(concat!("../../android/assets/jsons/Civ V - Gods & Kings/", $f)))),*]
    };
}

/// The ruleset files the scenario needs, carried into the binary.
pub fn embedded_sources() -> Vec<(String, String)> {
    let files: &[(&str, &str)] = embed![
        "Buildings.json",
        "Units.json",
        "UnitTypes.json",
        "Terrains.json",
        "TileResources.json",
        "TileImprovements.json",
        "Techs.json",
        "Policies.json",
        "Eras.json",
        "Speeds.json",
        "Nations.json",
    ];
    files.iter().map(|(n, c)| (n.to_string(), c.to_string())).collect()
}

/// Accumulates values into one number, order-sensitively.
struct Digest(u64);

impl Digest {
    fn new() -> Digest {
        Digest(0x243F_6A88_85A3_08D3)
    }

    fn add(&mut self, v: i64) -> &mut Self {
        self.0 = self.0.wrapping_add(v as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15);
        self.0 ^= self.0 >> 29;
        self
    }

    fn add_fixed(&mut self, v: Fixed) -> &mut Self {
        self.add(v.raw())
    }
}

/// Run the scenario and fold everything it produces into one number.
pub fn fingerprint() -> u64 {
    let mut d = Digest::new();
    let _ = RULESET; // documented above; the macro carries the real path

    // --- parsing ----------------------------------------------------------
    let sources = embedded_sources();
    let Ok(r) = Ruleset::from_sources(&sources) else { return 0 };
    d.add(r.object_count() as i64)
        .add(r.buildings.len() as i64)
        .add(r.technologies.len() as i64)
        .add(r.terrains.len() as i64);

    // Number parsing has to be exact, not merely close.
    if let Some(v) = Json::parse(r#"{"a": 0.95, "b": 1.4813, "c": -2.5}"#).ok() {
        for k in ["a", "b", "c"] {
            d.add_fixed(v.get(k).and_then(|x| x.as_fixed()).unwrap_or(Fixed::ZERO));
        }
    }

    // --- derived relationships -------------------------------------------
    for t in r.technologies.ids() {
        d.add(r.technologies[t].cost.unwrap_or(0) as i64);
    }
    for b in r.buildings.ids() {
        d.add(r.buildings[b].cost.unwrap_or(0) as i64);
    }

    // --- fixed-point powers ----------------------------------------------
    for n in 1..40i32 {
        let x = Fixed::whole(n);
        d.add_fixed(x.pow(Fixed::permille(1_500)).unwrap_or(Fixed::ZERO));
        d.add_fixed(x.pow(Fixed::permille(1_300)).unwrap_or(Fixed::ZERO));
        d.add_fixed(x.sqrt().unwrap_or(Fixed::ZERO));
        d.add_fixed(x.log2().unwrap_or(Fixed::ZERO));
    }

    // --- modifier composition --------------------------------------------
    let mut stack = ModifierStack::new();
    for (i, layer) in [1i32, 2, 3, 5, 7, 11].into_iter().enumerate() {
        let src = Source::new(SourceKind::Building, "m");
        let amount = Fixed::whole(layer);
        stack.add(match i % 3 {
            0 => Modifier::base(amount, src),
            1 => Modifier::additive_percent(amount, src),
            _ => Modifier::multiplicative_percent(amount, src),
        });
    }
    for start in 0..20i32 {
        d.add_fixed(stack.evaluate(Fixed::whole(start)));
    }

    // --- growth curve -----------------------------------------------------
    let g = GrowthModifiers::default();
    for pop in 1..40u32 {
        d.add(food_to_next_population(pop, g) as i64);
    }

    // --- the map, yields and movement -------------------------------------
    let Some(grassland) = r.terrain("Grassland") else { return d.0 };
    let mut grid = TileGrid::new(40, 40, grassland);
    if let (Some(forest), Some(hill), Some(marsh)) =
        (r.terrain("Forest"), r.terrain("Hill"), r.terrain("Marsh"))
    {
        for t in grid.ids() {
            match t.0 % 4 {
                1 => grid.features[t.0 as usize].push(forest),
                2 => grid.features[t.0 as usize].push(hill),
                3 => grid.features[t.0 as usize].push(marsh),
                _ => {}
            }
        }
    }
    for t in grid.ids().step_by(7) {
        let y = tile_yield(&grid, &r, t);
        for s in Stat::ALL {
            d.add_fixed(y[s]);
        }
    }

    let start = grid.id_at(20, 20).unwrap_or(TileId(0));
    for budget in 1..6i32 {
        let set = reachable(&grid, &r, start, Fixed::whole(budget), &[]);
        d.add(set.len() as i64);
        for (tile, cost) in set {
            d.add(tile.0 as i64).add_fixed(cost);
        }
    }

    // --- a city living for a while ----------------------------------------
    let mut city = City::found("Fingerprint", CivId(0), start);
    for turn in 0..60i32 {
        let food = (turn % 7) - 2; // feast and famine
        d.add(match apply_food(&mut city, food, g) {
            GrowthEvent::Grew { population, carried_over } => population as i64 * 1_000 + carried_over as i64,
            GrowthEvent::Starved { population } => -(population as i64),
            GrowthEvent::Stored { food } => food as i64,
            GrowthEvent::Held { food } => food as i64 * 7,
        });
    }

    // --- combat -----------------------------------------------------------
    for att in 1..12i32 {
        for def in 1..12i32 {
            let ratio = Fixed::whole(att).div(Fixed::whole(def)).unwrap_or(Fixed::ONE);
            for roll in [0i64, 250, 500, 750, 999] {
                let rnd = Fixed::permille(roll);
                d.add_fixed(damage_modifier(ratio, false, rnd));
                d.add_fixed(damage_modifier(ratio, true, rnd));
            }
        }
    }
    for health in 1..=100u8 {
        d.add_fixed(health_ratio(health));
    }

    // --- the generator ----------------------------------------------------
    let mut rng = Rng::from_parts(&[17, 4_000, 9]);
    for _ in 0..200 {
        d.add(rng.next_u64() as i64);
        d.add_fixed(rng.unit_interval());
    }

    // --- map generation ---------------------------------------------------
    //
    // The system most at risk here, and the one with the least to fall back
    // on: a map that differs by one tile between two clients is a game they
    // cannot play together, and no differential test against Unciv can catch
    // it because Unciv generates a different map on purpose. Folding a whole
    // generated map in means the existing cross-target check covers it.
    {
        use crate::world::mapgen::{generate, MapParameters};
        let map = generate(
            &r,
            MapParameters { width: 24, height: 16, seed: 0x1234_5678, ..Default::default() },
            4,
        );
        for tile in map.grid.ids() {
            let i = tile.0 as usize;
            d.add(map.grid.terrain[i].index() as i64);
            for f in &map.grid.features[i] {
                d.add(f.index() as i64);
            }
            d.add(map.grid.resource[i].map_or(-1, |x| x.index() as i64));
            d.add(map.grid.resource_amount[i] as i64);
        }
        for start in &map.starts {
            d.add(start.0 as i64);
        }
    }

    // --- a whole game ------------------------------------------------------
    //
    // Everything above checks a layer in isolation. This runs the engine:
    // forty turns of four civs on a generated map, folding in every civ's
    // holdings and every city's state at the end.
    //
    // It is the only check that covers the systems whose behaviour depends on
    // state rather than on the ruleset — combat, religion's spread, city-state
    // influence, espionage's rolls — and the only one that would catch two
    // machines diverging on the third turn of an actual game rather than on
    // an arithmetic edge case.
    {
        use crate::world::assignment::CityFocus;
        use crate::world::construction::CostModifiers;
        use crate::world::entities::{Civilization, Unit};
        use crate::world::mapgen::{generate, MapParameters};
        use crate::world::city::GrowthModifiers;
        use crate::world::policies::PolicyModifiers;
        use crate::world::research::ScienceModifiers;
        use crate::world::turn::advance_turn;
        use crate::world::world::World;

        let map = generate(
            &r,
            MapParameters { width: 24, height: 16, seed: 0x1234_5678, ..Default::default() },
            4,
        );
        if let Some(nation) = r.nations.ids().next() {
            let civs: Vec<_> =
                (0..4u8).map(|i| Civilization::new(CivId(i), nation, true)).collect();
            let mut w = World::new(map.grid, civs);
            let settler = r
                .units
                .iter()
                .find(|(id, _)| crate::world::settling::is_settler(&r, *id))
                .map(|(id, _)| id);
            let worker = r.units.iter().find(|(_, u)| u.name == "Worker").map(|(id, _)| id);
            for (i, start) in map.starts.iter().enumerate() {
                for base in [settler, worker].into_iter().flatten() {
                    w.place_unit(Unit::new(base, CivId(i as u8), *start, 2));
                }
            }
            for _ in 0..40 {
                advance_turn(
                    &mut w,
                    &r,
                    CityFocus::None,
                    GrowthModifiers::default(),
                    CostModifiers::default(),
                    ScienceModifiers::default(),
                    None,
                    PolicyModifiers::default(),
                );
            }

            d.add(w.turn as i64).add(w.cities.len() as i64).add(w.units.len() as i64);
            for civ in &w.civs {
                d.add(civ.gold as i64)
                    .add(civ.science_stored as i64)
                    .add(civ.culture_stored as i64)
                    .add(civ.techs.len() as i64)
                    .add(civ.adopted_policies.len() as i64)
                    .add(civ.faith.stored as i64)
                    .add(civ.cities.len() as i64)
                    .add(civ.units.len() as i64)
                    .add(civ.espionage.spies.len() as i64)
                    .add(civ.defeated as i64);
            }
            for (_, city) in w.cities.iter() {
                d.add(city.population as i64)
                    .add(city.food_stored as i64)
                    .add(city.production_stored as i64)
                    .add(city.health as i64)
                    .add(city.built.len() as i64)
                    .add(city.centre.0 as i64)
                    .add(city.faith.total_pressure() as i64);
            }
            for (_, unit) in w.units.iter() {
                d.add(unit.position.0 as i64).add(unit.health as i64);
                d.add_fixed(unit.movement_left);
            }
        }
    }

    d.0
}

/// C ABI export, for loading the wasm build directly.
///
/// # Safety
/// Takes nothing and returns a plain integer; there is nothing to misuse.
#[no_mangle]
pub extern "C" fn civ_core_fingerprint() -> u64 {
    fingerprint()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_scenario_actually_runs() {
        let f = fingerprint();
        println!("fingerprint: {f:#018x}");
        assert_ne!(f, 0, "zero means the ruleset failed to load");
        assert_ne!(f, Digest::new().0, "and this means nothing was folded in");
    }

    #[test]
    fn it_is_stable_within_a_process() {
        assert_eq!(fingerprint(), fingerprint());
    }
}
