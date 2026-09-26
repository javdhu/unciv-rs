//! The shadow check for live sight (`civ_core::world::fog::audit`): AI games in which every sight update and
//! capture, after every step of every move, is also recomputed plainly on a copy of the world and must agree.

use civ_core::world::fog::audit;
use civ_core::world::turn::{finish_round, TurnModifiers};
use civ_core::world::{CityFocus, World};
use civ_sim::Settings;

/// Rounds in which each thing sight depends on came up, so a pass can be read as covering it.
#[derive(Debug, Default)]
struct Seen {
    embarked: u32,
    promoted: u32,
    allied: u32,
    spying: u32,
    lighthouse: u32,
}

fn play(seed: u64, width: u16, height: u16, majors: u8, city_states: u8, turns: u32) -> (u64, Seen) {
    let settings = Settings { seed, width, height, majors, city_states, ..Settings::default() };
    let rules = civ_sim::content::load().unwrap();
    let (mut world, rules, _) = civ_sim::new_game_in(&settings, &[], rules).unwrap();
    let r = &rules;
    let lighthouse = r.buildings.id("The Great Lighthouse");
    let mut seen = Seen::default();
    audit::enable(true);
    let before = audit::steps();
    while world.winner.is_none() && world.turn < turns {
        let mods = TurnModifiers::for_world(&world, r);
        finish_round(&mut world, r, CityFocus::default(), mods);
        tally(&world, lighthouse, &mut seen);
    }
    audit::enable(false);
    (audit::steps() - before, seen)
}

fn tally(w: &World, lighthouse: Option<civ_core::arena::Id<civ_core::ruleset::Building>>, seen: &mut Seen) {
    seen.embarked += w.units.iter().any(|(_, u)| u.embarked) as u32;
    seen.promoted += w.units.iter().any(|(_, u)| !u.promotions.is_empty()) as u32;
    seen.allied += civ_core::world::city_states::city_states(w)
        .into_iter()
        .any(|cs| civ_core::world::city_states::ally_of(w, cs).is_some()) as u32;
    seen.spying += w.civs.iter().any(|c| c.espionage.spies.iter().any(|s| s.is_established())) as u32;
    seen.lighthouse += lighthouse.is_some_and(|b| w.cities.iter().any(|(_, c)| c.built.contains(&b))) as u32;
}

#[test]
fn live_sight_agrees_with_its_plain_recomputation() {
    let (steps, seen) = play(7, 36, 24, 4, 3, 120);
    println!("{steps} steps audited; rounds with {seen:?}");
    assert!(steps > 10_000, "the audit ran");
    assert!(
        seen.embarked > 0 && seen.promoted > 0 && seen.allied > 0 && seen.spying > 0,
        "embarkation, promotions, alliances and spies came up"
    );
}

/// Whole games on several seeds, each on its own thread: `cargo test --release -p civ-sim sight_audit -- --ignored`.
#[test]
#[ignore = "whole games; minutes in release"]
fn live_sight_agrees_over_whole_games() {
    let games: Vec<_> = (1..=8u64)
        .map(|seed| std::thread::spawn(move || (seed, play(seed, 44, 29, 6, 4, 400))))
        .collect();
    let mut total = Seen::default();
    for game in games {
        let (seed, (steps, seen)) = game.join().unwrap();
        println!("seed {seed}: {steps} steps audited; rounds with {seen:?}");
        total.embarked += seen.embarked;
        total.promoted += seen.promoted;
        total.allied += seen.allied;
        total.spying += seen.spying;
        total.lighthouse += seen.lighthouse;
    }
    println!("all seeds: rounds with {total:?}");
    assert!(total.embarked > 0 && total.promoted > 0 && total.allied > 0 && total.spying > 0 && total.lighthouse > 0);
}
