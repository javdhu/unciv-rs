//! Blitz's five ways to win, each reached from a scripted world (ENGINE-BRIEF B4),
//! and the full game's untouched.
use civ_core::ruleset::Ruleset;
use civ_core::world::victory::{parse_milestone, winner, Milestone};
use civ_core::world::{CivId, World};
use civ_sim::Settings;

fn game(ruleset: Option<&str>) -> (World, Ruleset) {
    let (mut world, rules, me) = civ_sim::new_game(&Settings {
        ruleset: ruleset.map(Into::into),
        majors: 2,
        city_states: 0,
        ..Settings::default()
    })
    .unwrap();
    // Found every major's capital where its settler stands.
    for civ in [me, CivId(1)] {
        let settler = world
            .units
            .iter()
            .find(|(_, u)| u.owner == civ && civ_core::world::settling::is_settler(&rules, u.base))
            .unwrap()
            .0;
        civ_core::world::commands::execute(
            &mut world,
            &rules,
            civ,
            civ_core::world::commands::Command::FoundCity { unit: settler, name: format!("Capital {}", civ.0) },
        )
        .unwrap();
    }
    assert_eq!(winner(&world, &rules), None, "nobody has won at the start");
    (world, rules)
}

fn capital(world: &World, civ: CivId) -> civ_core::arena::Id<civ_core::world::City> {
    world.civ(civ).unwrap().cities[0]
}

fn won(world: &World, r: &Ruleset) -> Option<(u8, String)> {
    winner(world, r).map(|(civ, v)| (civ.0, r.victories[v].name.clone()))
}

fn build(world: &mut World, r: &Ruleset, civ: CivId, names: &[&str]) {
    let city = capital(world, civ);
    for name in names {
        let id = r.building(name).unwrap_or_else(|| panic!("{name}"));
        world.cities.get_mut(city).unwrap().built.insert(id);
    }
}

fn wonders(r: &Ruleset, n: usize) -> Vec<String> {
    r.buildings
        .iter()
        .filter(|(_, b)| b.is_wonder == Some(true) && b.name != "Utopia Project")
        .map(|(_, b)| b.name.clone())
        .take(n)
        .collect()
}

#[test]
fn the_wonder_race_goes_to_the_first_with_nine() {
    assert_eq!(parse_milestone("Build [9] [World Wonders]"), Milestone::BuildCount(9, "World Wonders".into()));
    assert_eq!(parse_milestone("Build [Apollo Program]"), Milestone::BuiltBuilding("Apollo Program".into()));
    let (mut world, r) = game(Some("blitz"));
    let names = wonders(&r, 9);
    let names: Vec<&str> = names.iter().map(String::as_str).collect();
    build(&mut world, &r, CivId(1), &names[..8]);
    assert_eq!(won(&world, &r), None, "eight is not nine");
    // National wonders do not count.
    build(&mut world, &r, CivId(1), &["Heroic Epic"]);
    assert_eq!(won(&world, &r), None);
    build(&mut world, &r, CivId(1), &names[8..]);
    assert_eq!(won(&world, &r), Some((1, "Wonders".into())));

    // The full game has no wonder race.
    let (mut world, r) = game(None);
    let names = wonders(&r, 12);
    let names: Vec<&str> = names.iter().map(String::as_str).collect();
    build(&mut world, &r, CivId(1), &names);
    assert_eq!(won(&world, &r), None);
}

fn complete_branches(world: &mut World, r: &Ruleset, civ: CivId, n: usize) {
    let branches: Vec<_> = r.policy_branches.ids().take(n).collect();
    let c = world.civ_mut(civ).unwrap();
    for (id, p) in r.policies.iter() {
        if p.branch.is_some_and(|b| branches.contains(&b)) {
            c.adopted_policies.insert(id);
        }
    }
}

#[test]
fn short_culture_is_three_branches_and_utopia() {
    let (mut world, r) = game(Some("blitz"));
    complete_branches(&mut world, &r, CivId(0), 3);
    assert_eq!(won(&world, &r), None, "Utopia still to build");
    build(&mut world, &r, CivId(0), &["Utopia Project"]);
    assert_eq!(won(&world, &r), Some((0, "Cultural".into())));

    // The full game still asks for five.
    let (mut world, r) = game(None);
    complete_branches(&mut world, &r, CivId(0), 3);
    build(&mut world, &r, CivId(0), &["Utopia Project"]);
    assert_eq!(won(&world, &r), None);
    complete_branches(&mut world, &r, CivId(0), 5);
    assert_eq!(won(&world, &r), Some((0, "Cultural".into())));
}

fn assemble(world: &mut World, r: &Ruleset, civ: CivId, parts: &[&str]) {
    for part in parts {
        *world.civ_mut(civ).unwrap().victory.spaceship_parts.entry(r.unit(part).unwrap()).or_default() += 1;
    }
}

#[test]
fn short_science_is_three_parts() {
    let (mut world, r) = game(Some("blitz"));
    build(&mut world, &r, CivId(0), &["Apollo Program"]);
    assemble(&mut world, &r, CivId(0), &["SS Booster", "SS Cockpit"]);
    assert_eq!(won(&world, &r), None);
    assemble(&mut world, &r, CivId(0), &["SS Engine"]);
    assert_eq!(won(&world, &r), Some((0, "Scientific".into())));

    // The full game still wants six.
    let (mut world, r) = game(None);
    build(&mut world, &r, CivId(0), &["Apollo Program"]);
    assemble(&mut world, &r, CivId(0), &["SS Booster", "SS Cockpit", "SS Engine"]);
    assert_eq!(won(&world, &r), None);
    assemble(&mut world, &r, CivId(0), &["SS Booster", "SS Booster", "SS Stasis Chamber"]);
    assert_eq!(won(&world, &r), Some((0, "Scientific".into())));
}

#[test]
fn domination_and_score_are_the_full_games() {
    for ruleset in [Some("blitz"), None] {
        let (mut world, r) = game(ruleset);
        let theirs = capital(&world, CivId(1));
        civ_core::world::combat::capture_city(&mut world, &r, theirs, CivId(0));
        assert_eq!(won(&world, &r), Some((0, "Domination".into())), "{ruleset:?}");

        let (mut world, r) = game(ruleset);
        build(&mut world, &r, CivId(1), &["Monument", "Granary"]);
        world.turn = world.endgame.max_turns;
        let leader = civ_core::world::victory::score_leader(&world, &r).unwrap();
        assert_eq!(won(&world, &r), Some((leader.0, "Time".into())), "{ruleset:?}");
    }
    // Blitz has no diplomatic victory at all.
    let (_, blitz) = game(Some("blitz"));
    assert!(blitz.victories.by_name("Diplomatic").is_none());
}
