use civ_core::arena::Registry;
use civ_core::ruleset::{RuinReward, Ruleset};
use civ_core::world::ai::UnitOrder;
use civ_core::world::assignment::CityFocus;
use civ_core::world::entities::{Civilization, Unit};
use civ_core::world::grid::{CivId, TileGrid};
use civ_core::world::turn::{advance_turn, apply_plan, plan_civ_turn, update_visibility, TurnEvent, TurnModifiers};
use civ_core::world::world::World;
use civ_core::Unique;

fn ruleset() -> Ruleset {
    let mut r = Ruleset::load_dir("rules/gods-and-kings").unwrap().unwrap();
    // A single reward makes route behavior independent of reward selection.
    r.ruin_rewards = Registry::new();
    r.ruin_rewards.insert(
        "Inscription",
        RuinReward {
            name: "Inscription".into(),
            uniques: vec![Unique::parse("Gain [17] [Culture]")],
            ..Default::default()
        },
    );
    r
}

fn world(r: &Ruleset) -> World {
    World::new(
        TileGrid::new(12, 8, r.terrain("Grassland").unwrap()),
        vec![Civilization::new(CivId(0), r.nations.id("Rome").unwrap(), true)],
    )
}

#[test]
fn a_move_collects_ruins_between_its_start_and_destination() {
    let r = ruleset();
    let mut w = world(&r);
    let start = w.grid.id_at(3, 3).unwrap();
    let middle = w.grid.id_at(4, 3).unwrap();
    let end = w.grid.id_at(5, 3).unwrap();
    w.grid.improvement[middle.0 as usize] = civ_core::world::ruins::ruins_improvement(&r);
    let unit = w.place_unit(Unit::new(r.units.id("Scout").unwrap(), CivId(0), start, 2));
    let mods = TurnModifiers::default();
    let mut plan = plan_civ_turn(&w, &r, CivId(0), CityFocus::None, mods);
    plan.orders = vec![(unit, UnitOrder::MoveTo(end))];
    let events = apply_plan(&mut w, &r, &plan, mods);
    assert_eq!(w.units.get(unit).unwrap().position, end);
    assert_eq!(w.grid.improvement[middle.0 as usize], None);
    assert!(events.iter().any(|e| matches!(e, TurnEvent::RuinsExplored { tile, .. } if *tile == middle)));
    let route: Vec<_> = events
        .iter()
        .filter_map(|e| match e {
            TurnEvent::UnitMoved(m) => Some(m.to),
            _ => None,
        })
        .collect();
    assert_eq!(route, [middle, end]);
    assert_eq!(w.civ(CivId(0)).unwrap().culture_stored, 17);
}

#[test]
fn an_ai_explorer_collects_a_visible_ruin_through_the_turn_loop() {
    let r = ruleset();
    let mut w = world(&r);
    let start = w.grid.id_at(3, 3).unwrap();
    let ruin = w.grid.id_at(4, 3).unwrap();
    w.grid.improvement[ruin.0 as usize] = civ_core::world::ruins::ruins_improvement(&r);
    w.place_unit(Unit::new(r.units.id("Scout").unwrap(), CivId(0), start, 2));
    update_visibility(&mut w);
    let events = advance_turn(&mut w, &r, CityFocus::None, TurnModifiers::default());
    assert!(events.iter().any(|e| matches!(e, TurnEvent::RuinsExplored { tile, .. } if *tile == ruin)));
    assert_eq!(w.civ(CivId(0)).unwrap().recent_ruins, ["Inscription"]);
}

#[test]
fn a_unit_can_stop_at_an_intermediate_tile_when_entry_consumes_its_moves() {
    let r = ruleset();
    let mut w = world(&r);
    let start = w.grid.id_at(3, 3).unwrap();
    let middle = w.grid.id_at(4, 3).unwrap();
    let end = w.grid.id_at(5, 3).unwrap();
    let unit = w.place_unit(Unit::new(r.units.id("Scout").unwrap(), CivId(0), start, 2));
    let event = civ_core::world::movement::move_unit_with(&mut w, &r, unit, end, |w, moved| {
        w.units.get_mut(moved.unit).unwrap().movement_left = civ_core::stats::Fixed::ZERO;
    })
    .unwrap();
    assert_eq!(event.to, middle);
    assert_eq!(w.units.get(unit).unwrap().position, middle);
}

#[test]
fn a_ruins_technology_cannot_be_reselected_by_the_earlier_turn_plan() {
    let mut r = ruleset();
    r.ruin_rewards = Registry::new();
    r.ruin_rewards.insert(
        "Knowledge",
        RuinReward {
            name: "Knowledge".into(),
            uniques: vec![Unique::parse("[1] free random researchable Tech(s) from the [Ancient era]")],
            ..Default::default()
        },
    );
    let mut w = world(&r);
    let mining = r.technologies.id("Mining").unwrap();
    w.civ_mut(CivId(0)).unwrap().techs.extend(r.technologies.ids().filter(|id| *id != mining));
    let start = w.grid.id_at(3, 3).unwrap();
    let end = w.grid.id_at(4, 3).unwrap();
    w.grid.improvement[end.0 as usize] = civ_core::world::ruins::ruins_improvement(&r);
    let unit = w.place_unit(Unit::new(r.units.id("Scout").unwrap(), CivId(0), start, 2));
    let mods = TurnModifiers::default();
    let mut plan = plan_civ_turn(&w, &r, CivId(0), CityFocus::None, mods);
    plan.orders = vec![(unit, UnitOrder::MoveTo(end))];
    plan.research = Some(mining);
    let events = apply_plan(&mut w, &r, &plan, mods);
    assert!(events.iter().any(|e| matches!(e, TurnEvent::RuinsExplored { .. })));
    let c = w.civ(CivId(0)).unwrap();
    assert!(c.techs.contains(&mining));
    assert_eq!(c.researching, None, "the completed technology must not be researched a second time");
}

#[test]
fn a_unit_spawned_by_an_entry_effect_can_block_the_remaining_route() {
    let r = ruleset();
    let mut w = world(&r);
    let start = w.grid.id_at(3, 3).unwrap();
    let middle = w.grid.id_at(4, 3).unwrap();
    let end = w.grid.id_at(5, 3).unwrap();
    let base = r.units.id("Worker").unwrap();
    let unit = w.place_unit(Unit::new(base, CivId(0), start, 2));
    let event = civ_core::world::movement::move_unit_with(&mut w, &r, unit, end, |w, moved| {
        if moved.to == middle {
            w.place_unit(Unit::new(base, CivId(0), end, 2));
        }
    })
    .unwrap();
    assert_eq!(event.to, middle);
    assert_eq!(w.units.iter().filter(|(_, u)| u.position == end).count(), 1);
}
