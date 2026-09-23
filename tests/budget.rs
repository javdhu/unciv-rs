//! Performance and state-size measurements for the deterministic engine.
//!
//! The timing gate requires an optimized build. Run it explicitly with:
//! `cargo test --release --features save --test budget -- --nocapture`.
//! Normal debug test runs ignore only `what_a_turn_costs`.
//!
//! The manual size tally counts selected structs and payloads. It is a lower
//! bound, excluding allocation overhead, spare capacity, observation history,
//! and other nested state. With `save`, the test also reports the complete
//! encoded snapshot size; encoded JSON bytes are not a heap measurement.

use civ_core::ruleset::Ruleset;
use civ_core::world::assignment::CityFocus;
use civ_core::world::entities::{Civilization, Unit};
use civ_core::world::grid::CivId;
use civ_core::world::mapgen::{generate, MapParameters};
use civ_core::world::turn::{advance_turn, TurnModifiers};
use civ_core::world::world::World;
use std::time::Instant;

fn ruleset() -> Ruleset {
    Ruleset::load_dir("rules/gods-and-kings").expect("readable").expect("loads")
}

fn game(r: &Ruleset, width: u16, height: u16, civs: usize) -> World {
    let map = generate(r, MapParameters { width, height, seed: 0xB0D9E7, ..Default::default() }, civs);
    let nation = r.nations.ids().next().unwrap();
    let list: Vec<_> = (0..civs).map(|i| Civilization::new(CivId(i as u8), nation, true)).collect();
    let mut w = World::new(map.grid, list);
    let settler =
        r.units.iter().find(|(id, _)| civ_core::world::settling::is_settler(r, *id)).map(|(id, _)| id).unwrap();
    let worker = r.units.iter().find(|(_, u)| u.name == "Worker").map(|(id, _)| id).unwrap();
    for (i, start) in map.starts.iter().enumerate() {
        w.place_unit(Unit::new(settler, CivId(i as u8), *start, 2));
        w.place_unit(Unit::new(worker, CivId(i as u8), *start, 2));
    }
    w
}

fn play(w: &mut World, r: &Ruleset, turns: u32) {
    for _ in 0..turns {
        advance_turn(w, r, CityFocus::None, TurnModifiers::default());
    }
}

/// Turn latency and clone cost, guarded only in optimized release builds.
#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "release timing gate: cargo test --release --features save --test budget -- --nocapture"
)]
fn what_a_turn_costs() {
    let r = ruleset();
    println!("\n--- engine budget ---");
    println!("World struct (stack): {} bytes", std::mem::size_of::<World>());

    for (w_, h_, civs, label) in [(24u16, 16u16, 4usize, "small"), (40, 26, 6, "standard"), (64, 40, 8, "large")] {
        let mut w = game(&r, w_, h_, civs);
        // Warm it up into a mid-game state, which is when a turn is dearest.
        play(&mut w, &r, 80);

        let t = Instant::now();
        play(&mut w, &r, 100);
        let per_turn = t.elapsed().as_secs_f64() * 1000.0 / 100.0;

        let t = Instant::now();
        let mut sink = 0usize;
        for _ in 0..1000 {
            sink += w.clone().cities.len();
        }
        let per_clone = t.elapsed().as_secs_f64() * 1_000_000.0 / 1000.0;
        assert!(sink > 0);

        println!(
            "{label:9} {w_}x{h_} {civs} civs | tiles {:5} cities {:3} units {:3} | \
             turn {per_turn:7.3} ms | clone {per_clone:8.2} us | clones per turn-budget(100ms) {:.0}",
            w.grid.len(),
            w.cities.len(),
            w.units.len(),
            100.0 / (per_clone / 1000.0)
        );

        // Only an order-of-magnitude guard. A turn that takes a second is a
        // game nobody will play over a network.
        assert!(per_turn < 200.0, "{label}: {per_turn:.1} ms per turn");
    }
}

/// A partial in-memory payload tally and, when enabled, encoded save bytes.
#[test]
fn what_a_game_weighs() {
    let r = ruleset();
    let mut w = game(&r, 40, 26, 6);
    play(&mut w, &r, 150);

    // This deliberately partial tally is a lower bound. In particular it
    // does not account for visibility/knowledge histories, allocator overhead,
    // unused collection capacity, or every nested collection in entities.
    let tiles = w.grid.len();
    let grid_bytes = tiles
        * (std::mem::size_of::<civ_core::arena::Id<civ_core::ruleset::Terrain>>() * 2
            + std::mem::size_of::<Option<civ_core::world::grid::CivId>>()
            + std::mem::size_of::<i32>()
            + 2);
    let features: usize = w.grid.features.iter().map(|f| f.len() * 4).sum();
    let civ_bytes: usize = w
        .civs
        .iter()
        .map(|c| {
            std::mem::size_of::<Civilization>()
                + c.techs.len() * 8
                + c.adopted_policies.len() * 8
                + c.cities.len() * 8
                + c.units.len() * 8
        })
        .sum();
    let city_bytes: usize = w
        .cities
        .iter()
        .map(|(_, c)| std::mem::size_of_val(c) + c.built.len() * 8 + c.worked.len() * 2 + c.name.len())
        .sum();
    let unit_bytes = w.units.len() * std::mem::size_of::<Unit>();
    let payload_lower_bound = grid_bytes + features + civ_bytes + city_bytes + unit_bytes;

    println!("\n--- state size, 40x26, 6 civs, turn 150 ---");
    println!("grid      {grid_bytes:7} bytes ({tiles} tiles)");
    println!("features  {features:7} bytes");
    println!("civs      {civ_bytes:7} bytes ({} civs)", w.civs.len());
    println!("cities    {city_bytes:7} bytes ({})", w.cities.len());
    println!("units     {unit_bytes:7} bytes ({})", w.units.len());
    println!("SELECTED PAYLOAD LOWER BOUND {payload_lower_bound} bytes (~{} KiB)", payload_lower_bound / 1024);
    println!("excludes knowledge/history allocations, collection capacity and allocator overhead");

    #[cfg(feature = "save")]
    {
        let snapshot = civ_core::save::encode(&w, &r).expect("mid-game world can be saved");
        println!(
            "COMPLETE ENCODED SNAPSHOT {} UTF-8 bytes (~{} KiB), not heap bytes",
            snapshot.len(),
            snapshot.len() / 1024
        );
    }
    #[cfg(not(feature = "save"))]
    println!("enable --features save to measure the complete encoded snapshot");
}
