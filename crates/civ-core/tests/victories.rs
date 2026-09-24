//! Lingua Franca influence and the Reserve Currency (docs/CONTENT.md,
//! Victories; `world::influence`).

use civ_core::ruleset::Ruleset;
use civ_core::world::city::City;
use civ_core::world::entities::Civilization;
use civ_core::world::grid::{CivId, RoadLevel, TileGrid};
use civ_core::world::influence::{self, BASE_INFLUENCE, INFLUENCE_BONUS};
use civ_core::world::victory::{self, parse_milestone, Milestone};
use civ_core::world::world::World;

mod common;

const A: CivId = CivId(0);
const B: CivId = CivId(1);
const C: CivId = CivId(2);

fn rules() -> Ruleset {
    common::rebuild_rules(&["VictoryTypes.json"])
}

fn world(r: &Ruleset) -> World {
    let civs = ["Rome", "Greece", "Egypt"]
        .iter()
        .enumerate()
        .map(|(i, n)| Civilization::new(CivId(i as u8), r.nations.id(n).unwrap(), true))
        .collect();
    let mut w = World::new(TileGrid::new(40, 12, r.terrain("Grassland").unwrap()), civs);
    for (i, x) in [2u16, 16, 30].into_iter().enumerate() {
        let centre = w.grid.id_at(x, 3).unwrap();
        w.found_city_in(r, City::found(format!("Capital {i}"), CivId(i as u8), centre));
    }
    w
}

fn culture(w: &mut World, civ: CivId, amount: i32) {
    w.civ_mut(civ).unwrap().add_culture(amount);
}

#[test]
fn the_new_milestones_parse_and_bad_ones_stay_unsupported() {
    assert_eq!(parse_milestone("Become influential over all rival nations"), Milestone::InfluentialOverAll);
    assert_eq!(
        parse_milestone("Carry [50]% of world trade for [10] turns"),
        Milestone::WorldTradeShare { percent: 50, turns: 10 }
    );
    for bad in ["Carry [150]% of world trade for [10] turns", "Carry [50]% of world trade for [0] turns"] {
        assert!(matches!(parse_milestone(bad), Milestone::Unsupported(_)), "{bad}");
    }
    let r = rules();
    assert!(influence::tracks_influence(&r) && influence::tracks_trade(&r));
    let gk = Ruleset::load_dir("rules/gods-and-kings").unwrap().unwrap();
    assert!(!influence::tracks_influence(&gk) && !influence::tracks_trade(&gk));
}

#[test]
fn a_quarter_of_new_culture_becomes_influence_over_met_rivals_at_peace() {
    let r = rules();
    let mut w = world(&r);
    culture(&mut w, A, 500);
    influence::tick(&mut w, &r); // culture made before counting began reaches nobody
    assert_eq!(influence::influence_over(&w, A, B), 0);
    w.diplomacy.meet(A, B);
    culture(&mut w, A, 100);
    influence::tick(&mut w, &r);
    assert_eq!(influence::influence_over(&w, A, B), BASE_INFLUENCE);
    assert_eq!(influence::influence_over(&w, A, C), 0, "not yet met");

    // Open borders add to it.
    w.diplomacy.set_open_borders(B, A, true);
    culture(&mut w, A, 100);
    influence::tick(&mut w, &r);
    assert_eq!(influence::influence_over(&w, A, B), 2 * BASE_INFLUENCE + INFLUENCE_BONUS);

    // War stops it.
    w.diplomacy.declare_war(A, B);
    culture(&mut w, A, 100);
    influence::tick(&mut w, &r);
    assert_eq!(influence::influence_over(&w, A, B), 2 * BASE_INFLUENCE + INFLUENCE_BONUS);
}

#[test]
fn influence_over_every_rival_wins_lingua_franca() {
    let r = rules();
    let mut w = world(&r);
    influence::tick(&mut w, &r);
    w.diplomacy.meet(A, B);
    w.diplomacy.meet(A, C);
    let lingua = r.victories.by_name("Lingua Franca").unwrap();
    culture(&mut w, B, 300);
    culture(&mut w, C, 1500);
    culture(&mut w, A, 4400); // 1100 influence over each
    influence::tick(&mut w, &r);
    assert!(influence::is_influential(&w, A, B), "past the floor and past B's 300");
    assert!(!influence::is_influential(&w, A, C), "short of C's 1500");
    assert!(!influence::is_influential(&w, B, A));
    assert!(!victory::achieved(&w, &r, A, lingua));
    culture(&mut w, A, 2000);
    influence::tick(&mut w, &r);
    assert!(influence::influential_over_all(&w, A));
    assert!(victory::achieved(&w, &r, A, lingua));
    assert!(!victory::achieved(&w, &r, B, lingua));
}

#[test]
fn a_rival_that_has_barely_begun_is_not_won_over_by_a_little() {
    let r = rules();
    let mut w = world(&r);
    influence::tick(&mut w, &r);
    w.diplomacy.meet(A, B);
    culture(&mut w, A, 400);
    influence::tick(&mut w, &r);
    assert_eq!(influence::lifetime_culture(&w, B), 0);
    assert!(!influence::is_influential(&w, A, B), "{} is under the floor", influence::influence_over(&w, A, B));
    assert!(influence::INFLUENCE_FLOOR > 100);
}

#[test]
fn carrying_half_of_world_trade_for_long_enough_wins_reserve_currency() {
    let r = rules();
    let mut w = world(&r);
    for name in ["The Wheel", "Railroads"] {
        w.civ_mut(A).unwrap().techs.insert(r.technologies.id(name).unwrap());
    }
    // A second city on a road to Rome is the only trade route in the world.
    let ostia = w.grid.id_at(8, 3).unwrap();
    w.found_city_in(&r, City::found("Ostia", A, ostia));
    for col in 2..=8 {
        let t = w.grid.id_at(col, 3).unwrap();
        w.grid.set_road(t, RoadLevel::Road);
        w.grid.owner[t.0 as usize] = Some(A);
    }
    assert!(influence::trade_volume(&w, &r, A) > 0);
    assert_eq!(influence::trade_shares(&w, &r)[&A], 0, "a village road is not yet world trade");
    // A grown city on the road makes it real: 3 majors need 45 gold of trade.
    let ostia = w.cities.iter().find(|(_, c)| c.name == "Ostia").unwrap().0;
    w.cities.get_mut(ostia).unwrap().population = 50;
    assert!(influence::trade_volume(&w, &r, A) >= 45 * 100);
    assert_eq!(influence::trade_shares(&w, &r)[&A], 1000);

    let reserve = r.victories.by_name("Reserve Currency").unwrap();
    for turn in 1..=3 {
        assert!(!victory::achieved(&w, &r, A, reserve), "not before turn {turn}");
        influence::tick(&mut w, &r);
    }
    assert!(victory::achieved(&w, &r, A, reserve));
    assert!(!victory::achieved(&w, &r, B, reserve));
    assert_eq!(victory::nearness(&w, &r, B, reserve), (0, 1), "judged, and unmet");
}
