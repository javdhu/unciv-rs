// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use civ_core::ruleset::Ruleset;
use civ_core::stats::Fixed;
use civ_core::world::city::City;
use civ_core::world::commands::{execute, Command, CommandRejection};
use civ_core::world::entities::{Civilization, Unit};
use civ_core::world::grid::{CivId, TileGrid, TileId};
use civ_core::world::research::{apply_science, can_research, ScienceModifiers};
use civ_core::world::turn::{finish_round, TurnEvent, TurnModifiers};
use civ_core::world::{victory, victory_actions as va, CityFocus, World};

fn setup(n: u8) -> (Ruleset, World) {
    let r = Ruleset::load_dir("rules/gods-and-kings").unwrap().unwrap();
    let mut w = World::new(
        TileGrid::new(20, 20, r.terrain("Grassland").unwrap()),
        (0..n).map(|i| Civilization::new(CivId(i), r.nations.ids().next().unwrap(), false)).collect(),
    );
    for i in 0..n {
        w.found_city_in(&r, City::found(format!("City {i}"), CivId(i), TileId(21 + u16::from(i) * 100)));
        for j in 0..i {
            w.diplomacy.meet(CivId(i), CivId(j));
        }
    }
    (r, w)
}

fn only(w: &mut World, r: &Ruleset, name: &str) {
    w.endgame.enabled_victories = Some([r.victories.id(name).unwrap()].into_iter().collect());
}

fn open_vote(w: &mut World) {
    va::start_voting(w, Fixed::ONE);
    w.turn = w.endgame.voting.as_ref().unwrap().opens_on.unwrap();
}

#[test]
fn six_physical_parts_are_consumed_in_the_capital_before_scientific_victory() {
    let (r, mut w) = setup(2);
    only(&mut w, &r, "Scientific");
    let capital = w.civ(CivId(0)).unwrap().cities[0];
    w.cities.get_mut(capital).unwrap().built.insert(r.building("Apollo Program").unwrap());
    let scientific = r.victories.get(r.victories.id("Scientific").unwrap()).unwrap();
    assert!(!victory::achieved(&w, &r, CivId(0), scientific));
    for (i, name) in scientific.required_spaceship_parts.iter().enumerate() {
        let part = r.unit(name).unwrap();
        let unit = w.place_unit(Unit::new(part, CivId(0), TileId(21), 2));
        let result = execute(&mut w, &r, CivId(0), Command::AssembleSpaceshipPart { unit }).unwrap();
        assert!(matches!(result.events.first(), Some(TurnEvent::Endgame(va::Event::PartAssembled { .. }))));
        assert!(w.units.get(unit).is_none());
        assert!(execute(&mut w, &r, CivId(0), Command::AssembleSpaceshipPart { unit }).is_err());
        assert_eq!(victory::achieved(&w, &r, CivId(0), scientific), i == 5);
    }
    assert_eq!(va::assembled_parts(&w, CivId(0), r.unit("SS Booster").unwrap()), 3);
    finish_round(&mut w, &r, CityFocus::None, TurnModifiers::default());
    assert_eq!(w.winner, Some((CivId(0), r.victories.id("Scientific").unwrap())));
    assert_eq!(w.victory_turn, Some(1));
}

#[test]
fn assembly_rejects_foreign_units_wrong_city_and_ordinary_civilians_atomically() {
    let (r, mut w) = setup(2);
    for (name, owner, tile, actor) in [("SS Engine", 0, 22, 0), ("Worker", 0, 21, 0), ("SS Engine", 1, 21, 0)] {
        let unit = w.place_unit(Unit::new(r.unit(name).unwrap(), CivId(owner), TileId(tile), 2));
        let before = w.clone();
        assert!(execute(&mut w, &r, CivId(actor), Command::AssembleSpaceshipPart { unit }).is_err());
        assert_eq!(w, before);
        w.remove_unit(unit);
    }
}

#[test]
fn ai_delivers_parts_and_swaps_a_blocking_worker_but_human_parts_stay_put() {
    let (r, mut w) = setup(2);
    only(&mut w, &r, "Scientific");
    let from = w.grid.neighbours(TileId(21)).next().unwrap();
    let worker = w.place_unit(Unit::new(r.unit("Worker").unwrap(), CivId(0), TileId(21), 2));
    let part = w.place_unit(Unit::new(r.unit("SS Booster").unwrap(), CivId(0), from, 2));
    va::automate_parts(&mut w, &r, CivId(0));
    assert!(w.units.get(part).is_none());
    assert_eq!(w.units.get(worker).unwrap().position, from);
    assert_eq!(va::assembled_parts(&w, CivId(0), r.unit("SS Booster").unwrap()), 1);
    let human = w.place_unit(Unit::new(r.unit("SS Cockpit").unwrap(), CivId(1), TileId(122), 2));
    finish_round(&mut w, &r, CityFocus::None, TurnModifiers::default());
    assert_eq!(w.units.get(human).unwrap().position, TileId(122));
}

#[test]
fn voting_interval_scales_and_one_ballot_window_resolves_once_with_un_double_vote() {
    let (r, mut w) = setup(3);
    only(&mut w, &r, "Diplomatic");
    let city = w.civ(CivId(0)).unwrap().cities[0];
    w.cities.get_mut(city).unwrap().built.insert(r.building("United Nations").unwrap());
    va::start_voting(&mut w, Fixed::permille(670));
    assert_eq!(w.endgame.voting.as_ref().unwrap().opens_on, Some(10));
    assert!(!va::may_vote(&w, CivId(0)));
    w.turn = 9;
    finish_round(&mut w, &r, CityFocus::None, TurnModifiers::default());
    assert!(va::may_vote(&w, CivId(0)));
    assert_eq!(va::votes_needed(&w, &r), 3);
    execute(&mut w, &r, CivId(0), Command::VoteDiplomatic { candidate: Some(CivId(1)) }).unwrap();
    execute(&mut w, &r, CivId(2), Command::VoteDiplomatic { candidate: Some(CivId(1)) }).unwrap();
    execute(&mut w, &r, CivId(1), Command::VoteDiplomatic { candidate: None }).unwrap();
    let before = w.clone();
    assert_eq!(
        execute(&mut w, &r, CivId(0), Command::VoteDiplomatic { candidate: None }),
        Err(CommandRejection::NotAllowed)
    );
    assert_eq!(before, w);
    let events = finish_round(&mut w, &r, CityFocus::None, TurnModifiers::default());
    assert_eq!(events.iter().filter(|e| matches!(e, TurnEvent::Endgame(va::Event::VoteResolved(_)))).count(), 1);
    let result = w.endgame.voting.as_ref().unwrap().last_result.as_ref().unwrap();
    assert_eq!(result.votes[&CivId(1)], 3);
    assert_eq!(result.winner, Some(CivId(1)));
    assert_eq!(w.winner, Some((CivId(1), r.victories.id("Diplomatic").unwrap())));
    assert_eq!(w.victory_turn, Some(11));
    assert!(finish_round(&mut w, &r, CityFocus::None, TurnModifiers::default()).is_empty());
}

#[test]
fn tied_vote_and_abstention_do_not_create_a_false_winner() {
    let (r, mut w) = setup(4);
    only(&mut w, &r, "Diplomatic");
    open_vote(&mut w);
    for (voter, target) in [(0, 1), (1, 0), (2, 1), (3, 0)] {
        assert!(va::cast_vote(&mut w, CivId(voter), Some(CivId(target))));
    }
    assert_eq!(va::votes_needed(&w, &r), 3);
    finish_round(&mut w, &r, CityFocus::None, TurnModifiers::default());
    let vote = w.endgame.voting.as_ref().unwrap();
    assert_eq!(vote.last_result.as_ref().unwrap().winner, None);
    assert_eq!(vote.opens_on, Some(31));
    assert!(vote.ballots.is_empty());
    assert!(w.winner.is_none());
    w.turn = 31;
    for id in 0..4 {
        assert!(va::cast_vote(&mut w, CivId(id), None));
    }
    finish_round(&mut w, &r, CityFocus::None, TurnModifiers::default());
    assert!(w.endgame.voting.as_ref().unwrap().last_result.as_ref().unwrap().votes.is_empty());
    assert!(w.winner.is_none());
}

#[test]
fn voters_cannot_vote_for_self_unmet_minor_or_dead_candidates() {
    let (r, mut w) = setup(3);
    open_vote(&mut w);
    assert!(!va::cast_vote(&mut w, CivId(0), Some(CivId(0))));
    w.civ_mut(CivId(2)).unwrap().city_state_type = r.city_state_types.ids().next();
    assert!(!va::cast_vote(&mut w, CivId(0), Some(CivId(2))));
    w.civ_mut(CivId(1)).unwrap().defeated = true;
    assert!(!va::cast_vote(&mut w, CivId(0), Some(CivId(1))));
    assert!(!va::may_vote(&w, CivId(0)), "no other living major");
}

#[test]
fn exact_time_score_counts_population_land_wonders_and_repeatable_techs() {
    let (r, mut w) = setup(2);
    only(&mut w, &r, "Time");
    w.endgame.max_turns = 2;
    let city = w.civ(CivId(1)).unwrap().cities[0];
    w.cities.get_mut(city).unwrap().population = 4;
    w.cities.get_mut(city).unwrap().built.insert(r.building("The Pyramids").unwrap());
    w.civ_mut(CivId(1)).unwrap().techs.insert(r.technologies.id("Agriculture").unwrap());
    w.civ_mut(CivId(1)).unwrap().victory.repeated_techs = 2;
    let land = w.cities.get(city).unwrap().tiles.len() as i64;
    assert_eq!(victory::score_fraction(&w, &r, CivId(1)), ((10 + 12 + land) * 2076 + 64 * 1200, 1200));
    assert!(victory::score(&w, &r, CivId(1)) > victory::score(&w, &r, CivId(0)));
    finish_round(&mut w, &r, CityFocus::None, TurnModifiers::default());
    assert!(w.winner.is_none());
    finish_round(&mut w, &r, CityFocus::None, TurnModifiers::default());
    assert_eq!(w.winner, Some((CivId(1), r.victories.id("Time").unwrap())));
}

#[test]
fn future_tech_remains_researchable_and_counts_each_completion() {
    let (r, mut w) = setup(2);
    let tech = r.technologies.id("Future Tech").unwrap();
    let c = w.civ_mut(CivId(0)).unwrap();
    c.techs.extend(r.technologies.ids().filter(|id| *id != tech));
    c.researching = Some(tech);
    for count in 1..=2 {
        assert!(can_research(&r, c, tech).is_ok());
        apply_science(c, &r, 1_000_000, ScienceModifiers::default());
        assert_eq!(c.victory.repeated_techs, count);
        assert_eq!(c.researching, Some(tech));
    }
}

#[cfg(feature = "save")]
#[test]
fn mid_ballot_and_assembled_parts_survive_save_and_invalid_state_is_rejected() {
    let (r, mut w) = setup(3);
    only(&mut w, &r, "Diplomatic");
    let part = w.place_unit(Unit::new(r.unit("SS Booster").unwrap(), CivId(0), TileId(21), 2));
    execute(&mut w, &r, CivId(0), Command::AssembleSpaceshipPart { unit: part }).unwrap();
    open_vote(&mut w);
    assert!(va::cast_vote(&mut w, CivId(0), Some(CivId(1))));
    let text = civ_core::save::encode(&w, &r).unwrap();
    let mut restored = civ_core::save::decode(&text, &r).unwrap();
    assert_eq!(w, restored);
    assert!(!va::cast_vote(&mut restored, CivId(0), None));
    let a = finish_round(&mut w, &r, CityFocus::None, TurnModifiers::default());
    let b = finish_round(&mut restored, &r, CityFocus::None, TurnModifiers::default());
    assert_eq!(a, b);
    assert_eq!(w, restored);
    restored.endgame.voting.as_mut().unwrap().interval = 0;
    assert!(civ_core::save::validate(&restored, &r).is_err());
    restored = w.clone();
    restored.endgame.voting.as_mut().unwrap().ballots.insert(CivId(255), None);
    assert!(civ_core::save::validate(&restored, &r).is_err());
    restored = w.clone();
    restored.civ_mut(CivId(0)).unwrap().victory.spaceship_parts.insert(r.unit("Worker").unwrap(), 1);
    assert!(civ_core::save::validate(&restored, &r).is_err());
}

#[test]
fn city_state_ally_and_ai_opinion_ballots_are_deterministic() {
    let (r, mut w) = setup(3);
    only(&mut w, &r, "Diplomatic");
    w.civ_mut(CivId(0)).unwrap().is_ai = true;
    w.civ_mut(CivId(2)).unwrap().city_state_type = r.city_state_types.ids().next();
    w.diplomacy.set_influence(CivId(2), CivId(1), Fixed::whole(100));
    let city = w.civ(CivId(0)).unwrap().cities[0];
    w.cities.get_mut(city).unwrap().built.insert(r.building("United Nations").unwrap());
    open_vote(&mut w);
    assert!(va::cast_vote(&mut w, CivId(1), None));
    let mut duplicate = w.clone();
    let events = finish_round(&mut w, &r, CityFocus::None, TurnModifiers::default());
    let repeated = finish_round(&mut duplicate, &r, CityFocus::None, TurnModifiers::default());
    assert_eq!(events, repeated);
    assert_eq!(w, duplicate);
    assert_eq!(w.endgame.voting.as_ref().unwrap().last_result.as_ref().unwrap().votes[&CivId(1)], 3);
}

#[test]
fn captured_un_changes_vote_weight_at_resolution() {
    let (r, mut w) = setup(3);
    only(&mut w, &r, "Diplomatic");
    let un = w.civ(CivId(0)).unwrap().cities[0];
    w.found_city_in(&r, City::found("Survivor", CivId(0), TileId(321)));
    w.cities.get_mut(un).unwrap().built.insert(r.building("United Nations").unwrap());
    open_vote(&mut w);
    assert!(va::cast_vote(&mut w, CivId(0), Some(CivId(1))));
    assert!(va::cast_vote(&mut w, CivId(1), Some(CivId(0))));
    assert!(va::cast_vote(&mut w, CivId(2), None));
    civ_core::world::conquest::conquer(&mut w, &r, un, CivId(1)).unwrap();
    finish_round(&mut w, &r, CityFocus::None, TurnModifiers::default());
    let result = w.endgame.voting.as_ref().unwrap().last_result.as_ref().unwrap();
    assert_eq!(result.votes[&CivId(0)], 2);
    assert_eq!(result.votes[&CivId(1)], 1);
    assert_eq!(result.winner, None);
}

#[test]
fn exact_score_ties_use_civilization_order_and_disabled_victories_stay_disabled() {
    let (r, mut w) = setup(2);
    // Identical one-tile cities have the same rational score.
    assert_eq!(victory::score_fraction(&w, &r, CivId(0)), victory::score_fraction(&w, &r, CivId(1)));
    assert_eq!(victory::score_leader(&w, &r), Some(CivId(0)));
    w.endgame.max_turns = 1;
    w.turn = 1;
    w.endgame.enabled_victories = Some(Default::default());
    assert_eq!(victory::winner(&w, &r), None);
    only(&mut w, &r, "Time");
    assert_eq!(victory::winner(&w, &r), Some((CivId(0), r.victories.id("Time").unwrap())));
}

#[test]
fn a_city_state_ahead_on_score_does_not_block_the_time_victory() {
    let (mut r, mut w) = setup(2);
    // civ.game's rule (`chronicleAmongLivingMajors`); G&K ranks every civ.
    r.chronicle_among_living_majors = true;
    w.civ_mut(CivId(1)).unwrap().city_state_type = r.city_state_types.ids().next();
    w.civ_mut(CivId(1)).unwrap().techs.extend(r.technologies.ids().take(20));
    assert!(w.civ(CivId(1)).unwrap().is_city_state());
    assert_eq!(victory::score_leader(&w, &r), Some(CivId(0)));
    w.endgame.max_turns = 1;
    w.turn = 1;
    only(&mut w, &r, "Time");
    assert_eq!(victory::winner(&w, &r), Some((CivId(0), r.victories.id("Time").unwrap())));
}

#[test]
fn switching_research_preserves_separate_work_and_overflow_is_consumed_once() {
    use civ_core::world::research::{apply_science_in, invested, tech_cost_in};
    let (r, mut w) = setup(2);
    let a = r.technologies.id("Agriculture").unwrap();
    let p = r.technologies.id("Pottery").unwrap();
    let mine = r.technologies.id("Mining").unwrap();
    w.civ_mut(CivId(0)).unwrap().techs.insert(a);
    execute(&mut w, &r, CivId(0), Command::SetResearch { technology: p }).unwrap();
    apply_science_in(&mut w, &r, CivId(0), 20, ScienceModifiers::default());
    execute(&mut w, &r, CivId(0), Command::SetResearch { technology: mine }).unwrap();
    apply_science_in(&mut w, &r, CivId(0), 10, ScienceModifiers::default());
    assert_eq!(invested(w.civ(CivId(0)).unwrap(), p), 20);
    assert_eq!(invested(w.civ(CivId(0)).unwrap(), mine), 10);
    execute(&mut w, &r, CivId(0), Command::SetResearch { technology: p }).unwrap();
    let cost = tech_cost_in(&w, &r, CivId(0), p, ScienceModifiers::default()).unwrap();
    apply_science_in(&mut w, &r, CivId(0), cost - 20 + 3, ScienceModifiers::default());
    assert_eq!(w.civ(CivId(0)).unwrap().science_stored, 3);
    assert_eq!(invested(w.civ(CivId(0)).unwrap(), p), 0);
    execute(&mut w, &r, CivId(0), Command::SetResearch { technology: mine }).unwrap();
    apply_science_in(&mut w, &r, CivId(0), 1, ScienceModifiers::default());
    assert_eq!(invested(w.civ(CivId(0)).unwrap(), mine), 14);
    assert_eq!(w.civ(CivId(0)).unwrap().science_stored, 0);
    apply_science_in(&mut w, &r, CivId(0), 1, ScienceModifiers::default());
    assert_eq!(invested(w.civ(CivId(0)).unwrap(), mine), 15);
}

#[test]
fn live_research_price_uses_known_majors_excludes_puppets_and_never_rounds_diffusion_early() {
    use civ_core::world::research::tech_cost_in;
    let (r, mut w) = setup(3);
    let tech = r.technologies.id("Agriculture").unwrap();
    let m = ScienceModifiers::default();
    let base = r.technologies.get(tech).unwrap().cost.unwrap();
    w.civ_mut(CivId(1)).unwrap().techs.insert(tech);
    // Medium map cost multiplier 1.1 exactly cancels one of three known majors' 10% discount.
    assert_eq!(tech_cost_in(&w, &r, CivId(0), tech, m), Some(base));
    let puppet = w.found_city_in(&r, City::found("Puppet", CivId(0), TileId(321)));
    w.cities.get_mut(puppet).unwrap().puppet = true;
    assert_eq!(tech_cost_in(&w, &r, CivId(0), tech, m), Some(base));
    w.cities.get_mut(puppet).unwrap().puppet = false;
    assert_eq!(tech_cost_in(&w, &r, CivId(0), tech, m), Some((base * 105) / 100));
}

#[test]
fn scientist_overflow_is_capped_and_free_tech_clears_only_its_own_investment() {
    use civ_core::world::research::{apply_science_with_income_in, grant_technology};
    let (r, mut w) = setup(2);
    let a = r.technologies.id("Agriculture").unwrap();
    let p = r.technologies.id("Pottery").unwrap();
    let c = w.civ_mut(CivId(0)).unwrap();
    c.researching = Some(a);
    c.science_invested.insert(p, 8);
    apply_science_with_income_in(&mut w, &r, CivId(0), 10_000, 5, ScienceModifiers::default());
    assert_eq!(w.civ(CivId(0)).unwrap().science_stored, r.technologies.get(a).unwrap().cost.unwrap().max(25));
    let c = w.civ_mut(CivId(0)).unwrap();
    assert_eq!(c.science_invested[&p], 8);
    assert!(grant_technology(c, &r, p));
    assert!(c.science_invested.is_empty());
}

#[cfg(feature = "save")]
#[test]
fn paused_research_roundtrips_and_invalid_known_tech_investment_is_rejected() {
    let (r, mut w) = setup(2);
    let a = r.technologies.id("Agriculture").unwrap();
    w.civ_mut(CivId(0)).unwrap().science_invested.insert(a, 8);
    let text = civ_core::save::encode(&w, &r).unwrap();
    assert_eq!(w, civ_core::save::decode(&text, &r).unwrap());
    w.civ_mut(CivId(0)).unwrap().techs.insert(a);
    assert!(civ_core::save::validate(&w, &r).is_err());
}

#[test]
fn selecting_fully_paid_research_completes_immediately() {
    let (r, mut w) = setup(2);
    let tech = r.technologies.id("Agriculture").unwrap();
    let cost = civ_core::world::research::tech_cost_in(&w, &r, CivId(0), tech, ScienceModifiers::default()).unwrap();
    w.civ_mut(CivId(0)).unwrap().science_invested.insert(tech, cost - 2);
    w.civ_mut(CivId(0)).unwrap().science_stored = 4;
    let out = execute(&mut w, &r, CivId(0), Command::SetResearch { technology: tech }).unwrap();
    assert!(out.events.iter().any(|e| matches!(e, TurnEvent::TechLearned { .. })));
    assert!(w.civ(CivId(0)).unwrap().has_tech(tech));
    assert_eq!(w.civ(CivId(0)).unwrap().science_stored, 2);
    assert_eq!(w.turn, 0);
}

#[test]
fn ai_prioritizes_missing_ship_parts_and_avoids_already_committed_parts() {
    use civ_core::world::{
        ai,
        construction::{Construction, CostModifiers},
    };
    let (r, mut w) = setup(2);
    only(&mut w, &r, "Scientific");
    let civ = CivId(0);
    let capital = w.civ(civ).unwrap().cities[0];
    w.civ_mut(civ).unwrap().techs.extend(r.technologies.ids());
    w.cities.get_mut(capital).unwrap().built.insert(r.building("Apollo Program").unwrap());
    civ_core::world::city::rebuild_city_uniques(&r, w.cities.get_mut(capital).unwrap());
    civ_core::world::policies::rebuild_uniques(&mut w, &r, civ);
    // A real domestic aluminium source supplies construction eligibility.
    let aluminium = r.resources.id("Aluminum").unwrap();
    let tile = w.cities.get(capital).unwrap().centre;
    w.grid.resource[tile.0 as usize] = Some(aluminium);
    w.grid.resource_amount[tile.0 as usize] = 20;
    let what = ai::choose_construction(
        &w,
        &r,
        civ,
        w.cities.get(capital).unwrap(),
        CostModifiers::default(),
        100,
        ai::Priorities {
            want_settler: false,
            want_worker: false,
            want_scout: false,
            want_military: true,
            happiness: 10,
        },
    );
    let Some(Construction::Unit(part)) = what else { panic!("expected a spaceship component, got {what:?}") };
    assert!(r.units[part].uniques.iter().any(|u| u.unique_type() == Some(civ_core::UniqueType::AddInCapital)));
    let needed = victory::required_parts(&r, &r.victories[r.victories.id("Scientific").unwrap()])[&part];
    w.civ_mut(civ).unwrap().victory.spaceship_parts.insert(part, needed - 1);
    assert!(va::part_priority(&w, &r, civ, part).is_some());
    w.cities.get_mut(capital).unwrap().queue.push(Construction::Unit(part));
    assert!(va::part_priority(&w, &r, civ, part).is_none());
    w.cities.get_mut(capital).unwrap().queue.clear();
    w.place_unit(Unit::new(part, civ, tile, 2));
    assert!(va::part_priority(&w, &r, civ, part).is_none());
}
