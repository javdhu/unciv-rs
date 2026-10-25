// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Save and restore costs. With no arguments: twelve rounds of the default game. `--turns N` plays on, `--map
//! small|medium|large` and `--civs`/`--city-states` size the game (every seat an AI), and `--out FILE` writes the save.
use civ_sim::session::Session;
use civ_sim::{content, Settings};
use std::hint::black_box;
use std::time::Instant;

fn measure<T>(name: &str, count: usize, mut action: impl FnMut() -> T) {
    let start = Instant::now();
    for _ in 0..count {
        black_box(action());
    }
    println!(
        "{name}: {:.3} ms",
        start.elapsed().as_secs_f64() * 1000.0 / count as f64
    );
}

fn main() {
    let mut settings = Settings::default();
    let (mut turns, mut out, mut automated) = (12, None, false);
    let mut args = std::env::args().skip(1);
    while let Some(flag) = args.next() {
        let value = args.next().expect("every flag takes a value");
        match flag.as_str() {
            "--turns" => turns = value.parse().unwrap(),
            "--seed" => settings.seed = value.parse().unwrap(),
            "--civs" => (settings.majors, automated) = (value.parse().unwrap(), true),
            "--city-states" => settings.city_states = value.parse().unwrap(),
            "--map" => {
                (settings.width, settings.height) = match value.as_str() {
                    "small" => (33, 21),
                    "medium" => (44, 29),
                    "large" => (66, 42),
                    _ => panic!("unknown map size {value}"),
                };
                automated = true;
            }
            "--out" => out = Some(value),
            _ => panic!("unknown flag {flag}"),
        }
    }
    let mut session = Session::new(settings, "profile".into(), vec![0]).unwrap();
    if automated {
        session = session.stage_controller(0, false).unwrap().next;
    }
    let start = Instant::now();
    for turn in 0..turns {
        let round = Instant::now();
        session = session.stage_timeout(session.turn()).unwrap().unwrap().next;
        if turns <= 12 {
            println!(
                "round {}: {:.1} ms",
                turn + 1,
                round.elapsed().as_secs_f64() * 1000.0
            );
        }
    }
    if turns > 12 {
        println!(
            "{turns} rounds: {:.1} s",
            start.elapsed().as_secs_f64()
        );
    }
    let world = session.world();
    let rules = session.rules();
    let saved = session.save().unwrap();
    println!("save bytes: {}", saved.len());
    if let Some(path) = out {
        std::fs::write(path, &saved).unwrap();
    }
    measure("validate", 20, || {
        black_box(civ_core::save::validate(world, rules).unwrap())
    });
    measure("ruleset identity", 20, || {
        black_box(civ_core::save::ruleset_identity(rules))
    });
    let identity = civ_core::save::RulesetIdentity::new(rules);
    measure("core encode (cached identity)", 20, || {
        black_box(civ_core::save::encode_with_identity(world, rules, &identity).unwrap())
    });
    let core = civ_core::save::encode_with_identity(world, rules, &identity).unwrap();
    println!("core bytes: {}", core.len());
    measure("core decode (cached identity)", 20, || {
        black_box(civ_core::save::decode_with_identity(&core, rules, &identity).unwrap())
    });
    measure("content hash", 20, || black_box(content::hash()));
    measure("session save", 20, || black_box(session.save().unwrap()));
    measure("session restore", 5, || {
        black_box(Session::restore(&saved).unwrap())
    });
}
