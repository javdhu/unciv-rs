//! Blitz slices: a game that plays two eras and ends after 60 turns
//! (docs/CONTENT.md, Spine; `civ_core::world::slices`).

use civ_core::ruleset::Ruleset;
use civ_core::world::research::{can_research, ResearchRejection};
use civ_sim::{advance, content, new_game_in, player_view, tables, Settings};

fn rules() -> Ruleset {
    let sources: Vec<(String, String)> = content::SOURCES
        .iter()
        .chain(content::BLITZ_SOURCES)
        .map(|(f, t)| (f.to_string(), t.to_string()))
        .collect();
    Ruleset::from_sources(&sources).unwrap()
}

fn victories(w: &civ_core::world::World, r: &Ruleset) -> Vec<String> {
    w.endgame.enabled_victories.as_ref().unwrap().iter().map(|v| r.victories[*v].name.clone()).collect()
}

fn settings(slice: &str) -> Settings {
    Settings {
        seed: 9,
        majors: 3,
        city_states: 1,
        ruleset: Some("blitz".into()),
        slice: Some(slice.into()),
        ..Settings::default()
    }
}

fn era_of(r: &Ruleset, tech: civ_core::arena::Id<civ_core::ruleset::Technology>) -> i32 {
    r.eras[r.technologies[tech].era(r).unwrap()].era_number
}

#[test]
fn a_crossroads_game_starts_in_the_age_of_paper_and_stops_at_the_age_of_sail() {
    let (w, r, me) = new_game_in(&settings("Crossroads"), &[], rules()).unwrap();
    let paper = r.eras.by_name("Age of Paper").unwrap().era_number;
    let sail = r.eras.by_name("Age of Sail").unwrap().era_number;
    let civ = w.civ(me).unwrap();
    for (id, _) in r.technologies.iter() {
        let era = era_of(&r, id);
        if era < paper {
            assert!(
                civ.techs.contains(&id),
                "{} is known from the start",
                r.technologies[id].name
            );
        }
        if era > sail {
            assert_eq!(
                can_research(&r, civ, id),
                Err(ResearchRejection::BeyondSlice),
                "{}",
                r.technologies[id].name
            );
        }
    }
    assert_eq!(w.endgame.max_turns, 60);
    let enabled = w.endgame.enabled_victories.as_ref().unwrap();
    let names: Vec<_> = enabled
        .iter()
        .map(|v| r.victories[*v].name.as_str())
        .collect();
    assert!(
        !names.contains(&"AGI"),
        "the Turing Project lies in the Age of Information: {names:?}"
    );
    for name in [
        "Conquest",
        "The Chronicle",
        "Lingua Franca",
        "Reserve Currency",
    ] {
        assert!(
            names.contains(&name),
            "{name} can be won in any slice: {names:?}"
        );
    }
    // The Age of Paper's starting kit: two settlers each, and its gold.
    let settlers = w
        .units
        .iter()
        .filter(|(_, u)| u.owner == me && civ_core::world::settling::is_settler(&r, u.base))
        .count();
    assert_eq!(settlers, 2);
    assert!(
        civ.gold
            >= r.eras
                .by_name("Age of Paper")
                .unwrap()
                .starting_gold
                .unwrap()
    );
    let v = player_view(&w, &r, me);
    assert_eq!(
        v.slice.as_ref().map(|s| s.name.as_str()),
        Some("Crossroads")
    );
    assert_eq!(v.year, 500);
}

#[test]
fn an_engines_city_is_founded_with_its_eras_population() {
    let (mut w, r, me) = new_game_in(&settings("Engines"), &[0], rules()).unwrap();
    let settler = w
        .units
        .iter()
        .find(|(_, u)| u.owner == me && civ_core::world::settling::is_settler(&r, u.base))
        .unwrap()
        .0;
    let city =
        civ_core::world::settling::found_city_with(&mut w, &r, settler, "Manchester").unwrap();
    assert_eq!(
        w.cities.get(city).unwrap().population,
        3,
        "Age of Steam's settlerPopulation"
    );
}

#[test]
fn a_slice_ends_after_its_turns_with_a_winner() {
    let (mut w, r, _) = new_game_in(&settings("Cradles"), &[], rules()).unwrap();
    for _ in 0..61 {
        advance(&mut w, &r);
        if w.winner.is_some() {
            break;
        }
    }
    let (_, victory) = w.winner.expect("someone has won when the slice ends");
    assert!(w.turn <= 61);
    let enabled = w.endgame.enabled_victories.as_ref().unwrap();
    assert!(enabled.contains(&victory));
}

#[test]
fn rule_tables_offer_the_slices_and_what_each_can_be_won_by() {
    let r = rules();
    let t = tables(&r);
    let slices = t.slices.as_ref().unwrap();
    assert_eq!(
        slices.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(),
        ["Cradles", "Crossroads", "Engines"]
    );
    let agi = r.victories.id("AGI").map(|v| v.index());
    assert!(slices
        .iter()
        .all(|s| agi.is_none_or(|a| !s.victories.contains(&a))));
    assert!(
        new_game_in(&settings("Renaissance"), &[], rules()).is_err(),
        "an unknown slice is refused"
    );
}

#[test]
fn a_slice_begins_in_its_own_era_with_the_namesake_tech() {
    for (slice, era, opener) in [("Crossroads", "Age of Paper", "Paper"), ("Engines", "Age of Steam", "Steam Power")] {
        let (w, r, me) = new_game_in(&settings(slice), &[], rules()).unwrap();
        let tech = r.technologies.id(opener).unwrap();
        for c in w.civs.iter().filter(|c| c.is_major()) {
            assert!(c.techs.contains(&tech), "{slice}: {opener} is known from the start");
        }
        assert_eq!(
            civ_core::world::policies::era_number(&w, &r, me),
            r.eras.by_name(era).unwrap().era_number,
            "{slice} begins in the {era}"
        );
        // only the namesake: the rest of the era is still to research
        let known_in_era = w.civ(me).unwrap().techs.iter().filter(|t| era_of(&r, **t) == r.eras.by_name(era).unwrap().era_number).count();
        assert_eq!(known_in_era, 1, "{slice}");
    }
    let (w, r, me) = new_game_in(&settings("Cradles"), &[], rules()).unwrap();
    let first = r.eras.values().map(|e| e.era_number).min().unwrap();
    assert_eq!(civ_core::world::policies::era_number(&w, &r, me), first, "Cradles starts as a full game does");
}

#[test]
fn a_victory_gated_past_the_slice_is_not_offered() {
    let r = rules();
    let sail = r.victory_knobs.trade_share_from_era.as_deref().expect("base sets tradeShareFromEra");
    assert_eq!(sail, "Age of Sail");
    let (w, r, _) = new_game_in(&settings("Cradles"), &[], rules()).unwrap();
    let names = victories(&w, &r);
    assert!(!names.iter().any(|n| n == "Reserve Currency"), "trade counts only from the Age of Sail: {names:?}");
    assert!(names.iter().any(|n| n == "Lingua Franca"), "{names:?}");
    for slice in ["Crossroads", "Engines"] {
        let (w, r, _) = new_game_in(&settings(slice), &[], rules()).unwrap();
        assert!(victories(&w, &r).iter().any(|n| n == "Reserve Currency"), "{slice}");
    }
    let rc = r.victories.by_name("Reserve Currency").unwrap();
    assert_eq!(civ_core::world::slices::victory_era(&r, rc), r.eras.by_name("Age of Sail").unwrap().era_number);
}
