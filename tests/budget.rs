//! What the engine costs, measured rather than assumed.
//!
//! These are the numbers any decision about search, netcode or a client has to
//! be judged against. How much compute an AI can spend per turn depends on how
//! fast a turn is; whether tree search is even possible depends on how cheap a
//! clone is; whether rollback netcode is sane depends on how big the state is.
//!
//! Measured on an M-series Mac, September 2026, for scale rather than as a
//! target:
//!
//! | map | turn | World::clone | clones per 100 ms |
//! |---|---|---|---|
//! | 24x16, 4 civs  | 2.4 ms  | 71 us  | ~1,400 |
//! | 64x40, 8 civs  | 37 ms   | 288 us | ~350   |
//!
//! A whole mid-game state is ~33 KB. The wasm build is 1.2 MB raw and 327 KB
//! gzipped, of which 456 KB is the ruleset JSON embedded by `include_str!` —
//! so the engine *code* is a small fraction of that, and fetching the ruleset
//! separately would cut the download a long way.
//!
//! What those numbers decide: a few hundred to a few thousand full-state
//! clones fit in a 100 ms turn budget, which is the ceiling on any tree search
//! that works by cloning and simulating. And a game worth 33 KB is small
//! enough to send whole — though a deterministic engine never needs to, since
//! a seed plus a command log reconstructs it exactly.
//!
//! Reported, not asserted at fixed values — they are measurements, and a
//! machine-dependent assertion would be a flaky test. The loose bounds that
//! *are* asserted only catch an order-of-magnitude regression.

use civ_core::ruleset::Ruleset;
use civ_core::world::assignment::CityFocus;
use civ_core::world::entities::{Civilization, Unit};
use civ_core::world::mapgen::{generate, MapParameters};
use civ_core::world::turn::{advance_turn, TurnModifiers};
use civ_core::world::world::World;
use civ_core::world::grid::CivId;
use std::time::Instant;

fn ruleset() -> Ruleset {
    Ruleset::load_dir("../android/assets/jsons/Civ V - Gods & Kings")
        .expect("readable")
        .expect("loads")
}

fn game(r: &Ruleset, width: u16, height: u16, civs: usize) -> World {
    let map = generate(r, MapParameters { width, height, seed: 0xB0D9E7, ..Default::default() }, civs);
    let nation = r.nations.ids().next().unwrap();
    let list: Vec<_> = (0..civs).map(|i| Civilization::new(CivId(i as u8), nation, true)).collect();
    let mut w = World::new(map.grid, list);
    let settler = r
        .units
        .iter()
        .find(|(id, _)| civ_core::world::settling::is_settler(r, *id))
        .map(|(id, _)| id)
        .unwrap();
    let worker = r.units.iter().find(|(_, u)| u.name == "Worker").map(|(id, _)| id).unwrap();
    for (i, start) in map.starts.iter().enumerate() {
        w.place_unit(Unit::new(settler, CivId(i as u8), *start, 2));
        w.place_unit(Unit::new(worker, CivId(i as u8), *start, 2));
    }
    w
}

fn play(w: &mut World, r: &Ruleset, turns: u32) {
    for _ in 0..turns {
        advance_turn(
            w,
            r,
            CityFocus::None,
            TurnModifiers::default(),
        );
    }
}

/// The three numbers that decide what kind of AI is affordable.
#[test]
fn what_a_turn_costs() {
    let r = ruleset();
    println!("\n--- engine budget ---");
    println!("World struct (stack): {} bytes", std::mem::size_of::<World>());

    for (w_, h_, civs, label) in
        [(24u16, 16u16, 4usize, "small"), (40, 26, 6, "standard"), (64, 40, 8, "large")]
    {
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

/// How big a game actually is, which decides what can be sent over a wire and
/// what can be snapshotted for rollback or search.
#[test]
fn what_a_game_weighs() {
    let r = ruleset();
    let mut w = game(&r, 40, 26, 6);
    play(&mut w, &r, 150);

    // No serializer in the core crate by design, so this measures the live
    // footprint of the parts that would have to be sent.
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
    let total = grid_bytes + features + civ_bytes + city_bytes + unit_bytes;

    println!("\n--- game weight, 40x26, 6 civs, turn 150 ---");
    println!("grid      {grid_bytes:7} bytes ({tiles} tiles)");
    println!("features  {features:7} bytes");
    println!("civs      {civ_bytes:7} bytes ({} civs)", w.civs.len());
    println!("cities    {city_bytes:7} bytes ({})", w.cities.len());
    println!("units     {unit_bytes:7} bytes ({})", w.units.len());
    println!("TOTAL     {total:7} bytes (~{} KB)", total / 1024);
    println!("a deterministic game is really: seed + command log, not this");

    assert!(total < 2_000_000, "a game should not weigh megabytes: {total}");
}
