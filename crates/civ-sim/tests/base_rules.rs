//! The shipped `rules/base` against the engine's civ.game systems: faiths
//! arise in the real eras, and every victory milestone is one the engine can
//! judge.

use civ_core::world::victory::{parse_milestone, Milestone};
use civ_sim::{advance, content, new_game_in, Settings};

#[test]
fn base_faiths_arise_in_the_age_of_bronze() {
    let rules = content::load().unwrap();
    assert!(rules.faith_mode());
    let hinduism = rules.faiths[0].clone();
    assert_eq!(
        rules.eras.by_name(&hinduism.era).map(|e| e.era_number),
        Some(0),
        "Hinduism's era is the first"
    );
    let settings = Settings {
        seed: 5,
        majors: 4,
        city_states: 2,
        ..Settings::default()
    };
    let (mut w, r, _) = new_game_in(&settings, &[], rules).unwrap();
    for _ in 0..(hinduism.delay + 3) {
        advance(&mut w, &r);
    }
    let arisen = w
        .faiths
        .arisen
        .get(&0)
        .expect("Hinduism arose after its delay");
    assert!(arisen.turn as i32 >= hinduism.delay);
    let holy = w.cities.get(arisen.city).unwrap();
    assert_eq!(
        holy.faith.holy_city_of,
        Some(civ_core::world::faiths::key(0))
    );
}

#[test]
fn every_base_milestone_is_judged() {
    let r = content::load().unwrap();
    let (judged, total) = civ_core::world::victory::milestone_coverage(&r);
    assert_eq!(judged, total, "no milestone is left unsupported");
    let agi = r.victories.by_name("AGI").unwrap();
    assert_eq!(
        parse_milestone(&agi.milestones[0]),
        Milestone::BuiltBuilding("The Turing Project".into())
    );
    assert!(r.buildings.by_name("The Turing Project").is_some());
    for part in &agi.required_spaceship_parts {
        let unit = r
            .units
            .by_name(part)
            .unwrap_or_else(|| panic!("no part {part}"));
        assert!(
            unit.uniques
                .iter()
                .any(|u| u.unique_type() == Some(civ_core::UniqueType::SpaceshipPart)),
            "{part}"
        );
    }
    let peace = r.victories.by_name("Perpetual Peace").unwrap();
    let charter = r
        .buildings
        .by_name("World Charter")
        .expect("the charter building");
    assert_eq!(
        parse_milestone(&peace.milestones[0]),
        Milestone::BuiltBuilding("World Charter".into())
    );
    assert!(
        charter
            .uniques
            .iter()
            .any(|u| u.unique_type() == Some(civ_core::UniqueType::OneTimeTriggerVoting)),
        "the charter opens the vote: {:?}",
        charter.uniques.iter().map(|u| &u.text).collect::<Vec<_>>()
    );
}
