//! Choosing a leader at setup (docs/REBUILD.md, Engine step 1).

use civ_core::ruleset::Ruleset;
use civ_sim::{content, new_game_in, player_view, tables, Settings};

fn rules() -> Ruleset {
    let mut sources: Vec<_> = content::SOURCES
        .iter()
        .map(|(f, t)| (f.to_string(), t.to_string()))
        .collect();
    sources.push((
        "Nations.json".into(),
        include_str!("../../civ-core/tests/data/rebuild/Nations.json").into(),
    ));
    Ruleset::from_sources(&sources).unwrap()
}

fn rome(leader: Option<&str>) -> Settings {
    Settings {
        nation: Some("Rome".into()),
        leader: leader.map(String::from),
        ..Settings::default()
    }
}

#[test]
fn the_player_leads_with_the_leader_they_chose() {
    let (w, r, me) = new_game_in(&rome(Some("Galla Placidia")), &[0], rules()).unwrap();
    let civ = w.civ(me).unwrap();
    assert!(civ.alt_leader);
    assert!(
        civ.techs.contains(&r.technologies.id("Pottery").unwrap()),
        "her ability applies from the start"
    );
    let view = player_view(&w, &r, me);
    let mine = view.civs.iter().find(|c| c.id == me.0).unwrap();
    assert_eq!(mine.leader.as_deref(), Some("Galla Placidia"));

    let (w, r, me) = new_game_in(&rome(Some("Marcus Aurelius")), &[0], rules()).unwrap();
    assert!(!w.civ(me).unwrap().alt_leader);
    assert!(!w
        .civ(me)
        .unwrap()
        .techs
        .contains(&r.technologies.id("Pottery").unwrap()));
    let (w, _, me) = new_game_in(&rome(None), &[0], rules()).unwrap();
    assert!(
        !w.civ(me).unwrap().alt_leader,
        "a chosen nation without a chosen leader gets its default"
    );
}

#[test]
fn a_leader_from_another_nation_is_refused() {
    assert!(new_game_in(&rome(Some("Hatshepsut")), &[0], rules()).is_err());
    let greece = Settings {
        nation: Some("Greece".into()),
        leader: Some("Galla Placidia".into()),
        ..Settings::default()
    };
    assert!(new_game_in(&greece, &[0], rules()).is_err());
}

#[test]
fn computer_leaders_are_dealt_by_seed() {
    let deal = |seed| {
        let settings = Settings {
            seed,
            majors: 8,
            ..Settings::default()
        };
        let (w, r, _) = new_game_in(&settings, &[], rules()).unwrap();
        let rome = r.nations.id("Rome").unwrap();
        w.civs
            .iter()
            .find(|c| c.nation == rome)
            .map(|c| c.alt_leader)
    };
    let dealt: Vec<_> = (1..40).filter_map(deal).collect();
    assert!(
        dealt.contains(&true) && dealt.contains(&false),
        "both leaders come up: {dealt:?}"
    );
    assert_eq!(
        (1..40).filter_map(deal).collect::<Vec<_>>(),
        dealt,
        "the same seed deals the same leader"
    );
}

#[test]
fn rule_tables_list_leaders_regions_and_the_raiders() {
    let t = tables(&rules());
    let rome = t.nations.iter().find(|n| n.name == "Rome").unwrap();
    let leaders = rome.leaders.as_ref().unwrap();
    assert_eq!(
        leaders.iter().map(|l| l.name.as_str()).collect::<Vec<_>>(),
        ["Marcus Aurelius", "Galla Placidia"]
    );
    assert_eq!(leaders[1].ability.as_deref(), Some("Regency"));
    assert_eq!(leaders[1].effects, ["Starts with [Pottery]"]);
    assert_eq!(rome.region.as_deref(), Some("Europe"));
    let barbarians = t
        .nations
        .iter()
        .find(|n| n.barbarian == Some(true))
        .unwrap();
    assert_eq!(barbarians.name, "Raiders");
    assert!(barbarians.leaders.is_none());
}

#[test]
fn every_human_seat_chooses_its_own_nation_and_leader() {
    let seats = |a: &str, b: &str| {
        Some(vec![
            civ_sim::SeatChoice {
                seat: 0,
                nation: Some(a.into()),
                leader: Some("Galla Placidia".into()),
            },
            civ_sim::SeatChoice {
                seat: 1,
                nation: Some(b.into()),
                leader: None,
            },
        ])
    };
    let settings = Settings {
        majors: 3,
        seats: seats("Rome", "Greece"),
        ..Settings::default()
    };
    let (w, r, _) = new_game_in(&settings, &[0, 1], rules()).unwrap();
    assert_eq!(r.nations[w.civs[0].nation].name, "Rome");
    assert!(w.civs[0].alt_leader);
    assert_eq!(r.nations[w.civs[1].nation].name, "Greece");
    assert!(
        !w.civs[1].alt_leader,
        "a chosen nation without a chosen leader gets its default"
    );
    assert!(w.civs[2].nation != w.civs[0].nation && w.civs[2].nation != w.civs[1].nation);

    let clash = Settings {
        majors: 3,
        seats: seats("Rome", "Rome"),
        ..Settings::default()
    };
    assert!(
        new_game_in(&clash, &[0, 1], rules()).is_err(),
        "two seats cannot take one nation"
    );
}
