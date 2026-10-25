// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! How often each thing is on the map: self-play exposure counts, used to rank art by value.
//!
//!     cargo run --release -p civ-sim --example exposure -- [games] [turns]
//!
//! Units and improvements count per turn they exist (unit-turns, tile-turns); buildings per turn a city has
//! them; terrain, features and resources per tile at the start. Tab-separated: class, name, count.

use civ_core::world::TileId;
use civ_sim::{advance, new_game, Settings};
use std::collections::BTreeMap;

fn bump(m: &mut BTreeMap<(String, String), u64>, class: &str, name: &str, n: u64) {
    *m.entry((class.to_string(), name.to_string())).or_default() += n;
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let games: u64 = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(8);
    let turns: u32 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(150);
    let mut counts = BTreeMap::new();
    let (mut played, mut cities_peak) = (0u64, 0u64);
    for seed in 1..=games {
        let settings = Settings {
            seed,
            width: 40,
            height: 26,
            majors: 6,
            city_states: 4,
            nation: None,
            ..Settings::default()
        };
        let (mut w, r, _) = new_game(&settings).expect("game");
        let g = &w.grid;
        for i in 0..g.len() {
            let t = TileId(i as u16);
            bump(&mut counts, "terrain", &r.terrains[g.terrain[i]].name, 1);
            for f in &g.features[i] {
                bump(&mut counts, "feature", &r.terrains[*f].name, 1);
            }
            if let Some(res) = g.resource[i] {
                bump(&mut counts, "resource", &r.resources[res].name, 1);
            }
            let _ = t;
        }
        for _ in 0..turns {
            advance(&mut w, &r);
            played += 1;
            for (_, u) in w.units.iter() {
                bump(&mut counts, "unit", &r.units[u.base].name, 1);
            }
            for imp in w.grid.improvement.iter().flatten() {
                bump(&mut counts, "improvement", &r.improvements[*imp].name, 1);
            }
            cities_peak = cities_peak.max(w.cities.len() as u64);
            for (_, c) in w.cities.iter() {
                bump(&mut counts, "city", "city-turns", 1);
                bump(&mut counts, "city", "population-turns", c.population as u64);
                for b in &c.built {
                    bump(&mut counts, "building", &r.buildings[*b].name, 1);
                }
            }
            if w.winner.is_some() {
                break;
            }
        }
        let techs: usize = w.civs.iter().map(|c| c.techs.len()).max().unwrap_or(0);
        eprintln!(
            "game {seed}: turn {}, {} cities, most techs {techs}",
            w.turn,
            w.cities.len()
        );
    }
    println!(
        "meta\tgames\t{games}\nmeta\tturns played\t{played}\nmeta\tpeak cities\t{cities_peak}"
    );
    let mut rows: Vec<_> = counts.into_iter().collect();
    rows.sort_by(|a, b| a.0 .0.cmp(&b.0 .0).then(b.1.cmp(&a.1)));
    for ((class, name), n) in rows {
        println!("{class}\t{name}\t{n}");
    }
}
