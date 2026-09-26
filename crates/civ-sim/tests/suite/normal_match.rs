use civ_core::world::{CivId, TurnModifiers};
use civ_sim::session::Session;
use civ_sim::{new_game, player_view, Settings};

#[test]
fn initial_world_grants_starting_rules_and_national_units_in_legal_slots() {
    let (world, rules, me) = new_game(&Settings {
        nation: Some("Triple Alliance".into()),
        wrap_x: true,
        ..Settings::default()
    })
    .unwrap();
    let civ = world.civ(me).unwrap();
    assert!(civ.has_tech(rules.technologies.id("Agriculture").unwrap()));
    assert!(civ
        .units
        .iter()
        .any(|id| rules.units[world.units.get(*id).unwrap().base].name == "Jaguar"));
    assert_eq!(
        civ.units.len(),
        2,
        "the bundled Age of Bronze starts with one Settler and one military unit"
    );
    for a in &civ.units {
        for b in &civ.units {
            if a >= b {
                continue;
            }
            let a = world.units.get(*a).unwrap();
            let b = world.units.get(*b).unwrap();
            assert!(a.position != b.position || a.is_civilian(&rules) != b.is_civilian(&rules));
        }
    }
    assert!(player_view(&world, &rules, me).wrap_x);
    let save = civ_core::save::encode(&world, &rules).unwrap();
    let restored = civ_core::save::decode(&save, &rules).unwrap();
    assert_eq!(restored, world);
}

#[test]
fn chosen_victory_and_turn_limit_survive_session_save_and_replay() {
    let rules = civ_sim::content::load().unwrap();
    let time = rules
        .victories
        .iter()
        .find(|(_, v)| v.milestones.iter().any(|m| m.contains("highest score")))
        .unwrap();
    let mut session = Session::new(
        Settings {
            majors: 2,
            city_states: 0,
            wrap_x: true,
            max_turns: Some(2),
            victories: Some(vec![time.1.name.clone()]),
            ..Settings::default()
        },
        "time-match".into(),
        vec![0, 1],
    )
    .unwrap();
    for _ in 0..2 {
        session = session.stage_timeout(session.turn()).unwrap().unwrap().next;
        session = Session::restore(&session.save().unwrap()).unwrap();
    }
    assert!(session.finished());
    let update = session.current_update(0).unwrap();
    assert_eq!(
        update.view.result.as_ref().unwrap().condition,
        time.0.index()
    );
    assert_eq!(update.view.endgame.max_turns, 2);
    assert_eq!(update.view.endgame.enabled_victories, vec![time.0.index()]);
    session.verify_replay().unwrap();
    assert!(session.stage_timeout(session.turn()).unwrap().is_none());
}

#[test]
fn worker_progress_survives_rounds_and_a_world_save() {
    use civ_core::world::{
        commands::{execute, Command},
        City, Unit,
    };
    let (mut w, r, me) = new_game(&Settings {
        majors: 2,
        city_states: 0,
        ..Settings::default()
    })
    .unwrap();
    let start = w.units.get(w.civ(me).unwrap().units[0]).unwrap().position;
    let city = w.found_city_in(&r, City::found("Home", me, start));
    let tile = w
        .grid
        .neighbours(start)
        .find(|tile| {
            !w.grid.is_water(&r, *tile)
                && civ_core::world::movement::is_passable(&w.grid, &r, *tile)
        })
        .unwrap();
    w.grid.terrain[tile.0 as usize] = r.terrain("Grassland").unwrap();
    w.grid.features[tile.0 as usize].clear();
    w.grid.set_improvement(tile, None);
    w.grid.owner[tile.0 as usize] = Some(me);
    w.cities.get_mut(city).unwrap().tiles.push(tile);
    w.cities.get_mut(city).unwrap().tiles.sort();
    w.cities.get_mut(city).unwrap().tiles.dedup();
    let worker = w.place_unit(Unit::new(r.units.id("Worker").unwrap(), me, tile, 2));
    let farm = r.improvements.id("Farm").unwrap();
    execute(
        &mut w,
        &r,
        me,
        Command::Improve {
            unit: worker,
            improvement: farm,
        },
    )
    .unwrap();
    assert_eq!(
        w.grid.improvement_progress[tile.0 as usize],
        Some((farm, 1))
    );
    w.civ_mut(CivId(0)).unwrap().is_ai = false;
    w.civ_mut(CivId(1)).unwrap().is_ai = false;
    for round in 0..7 {
        civ_core::world::turn::finish_round(
            &mut w,
            &r,
            Default::default(),
            TurnModifiers::default(),
        );
        if round == 3 {
            w = civ_core::save::decode(&civ_core::save::encode(&w, &r).unwrap(), &r).unwrap();
        }
    }
    assert_eq!(w.grid.unpillaged_improvement(tile), Some(farm));
}

#[test]
fn expanded_owner_state_does_not_expose_foreign_city_unit_or_spy_details() {
    use civ_core::world::{espionage::Spy, City};
    let (mut w, r, me) = new_game(&Settings {
        majors: 2,
        city_states: 0,
        ..Settings::default()
    })
    .unwrap();
    let ours = w.units.get(w.civ(me).unwrap().units[0]).unwrap().position;
    let other = CivId(1);
    let theirs = w
        .units
        .get(w.civ(other).unwrap().units[0])
        .unwrap()
        .position;
    let own_city = w.found_city_in(&r, City::found("Home", me, ours));
    let foreign_city = w.found_city_in(&r, City::found("Away", other, theirs));
    let city = w.cities.get_mut(own_city).unwrap();
    city.faith.add_pantheon_pressure(me, 500);
    city.faith.recount(city.population);
    w.cities.get_mut(foreign_city).unwrap().food_stored = 9321;
    w.civ_mut(me).unwrap().espionage.spies.push(Spy::new());
    w.civ_mut(other)
        .unwrap()
        .espionage
        .spies
        .extend([Spy::new(), Spy::new(), Spy::new()]);
    for tile in w.grid.within(theirs, 2) {
        w.visibility.observe(me, tile);
    }
    civ_core::world::fog::capture_observations(&mut w, &r);
    let view = player_view(&w, &r, me);
    assert_eq!(view.spies.len(), 1);
    let home = view
        .cities
        .iter()
        .find(|c| c.id == own_city.index())
        .unwrap();
    let religion = &home.own.as_ref().unwrap().religion;
    assert_eq!(religion.majority, None);
    assert_eq!(religion.majority_pantheon, Some(me.0));
    assert!(religion
        .followers
        .iter()
        .any(|f| f.pantheon && f.founder == me.0 && f.followers > 0));
    let away = view
        .cities
        .iter()
        .find(|c| c.id == foreign_city.index())
        .unwrap();
    assert!(away.own.is_none());
    assert!(away.food_stored.is_none());
    assert!(away.queue.is_none());
    assert!(view
        .units
        .iter()
        .filter(|u| u.owner == other.0)
        .all(|u| u.own.is_none()));
    assert!(view
        .units
        .iter()
        .filter(|u| u.owner == me.0)
        .all(|u| u.own.is_some()));
    assert!(!serde_json::to_string(&view).unwrap().contains("9321"));
}

#[test]
fn city_state_view_shares_only_our_quests_and_relationship() {
    use civ_core::world::{
        quests::{Assigned, Target},
        City,
    };
    let (mut w, r, me) = new_game(&Settings {
        majors: 2,
        city_states: 1,
        ..Settings::default()
    })
    .unwrap();
    let cs = CivId(2);
    let position = w.units.get(w.civ(cs).unwrap().units[0]).unwrap().position;
    w.found_city_in(&r, City::found("State", cs, position));
    civ_core::world::city_state_actions::initialize(&mut w, &r);
    let q = r.quests.id("Acquire Great Person").unwrap();
    let make = |assignee, target| Assigned {
        quest: q,
        assignee,
        target,
        contest: None,
        assigned_on: 0,
        expires_on: 25,
        baseline: 0,
    };
    let artist = r.units.id("Virtuoso").unwrap();
    let scientist = r.units.id("Scholar").unwrap();
    w.civ_mut(cs).unwrap().quests.active.extend([
        make(me, Target::Unit(artist)),
        make(CivId(1), Target::Unit(scientist)),
    ]);
    assert!(player_view(&w, &r, me).city_states.is_empty());
    w.diplomacy.meet(me, cs);
    w.diplomacy
        .set_influence(cs, CivId(1), civ_core::stats::Fixed::whole(73));
    let view = player_view(&w, &r, me);
    let state = &view.city_states[0];
    assert_eq!(state.quests.len(), 1);
    assert_eq!(state.known_ally, None);
    assert!(state.has_ally);
    assert_eq!(
        serde_json::to_value(&state.quests[0].target).unwrap(),
        serde_json::json!({"kind":"unit","id":artist.index()})
    );
    let before = serde_json::to_value(&view).unwrap();
    w.civ_mut(cs).unwrap().quests.active[1].baseline = 98273;
    w.diplomacy
        .set_influence(cs, CivId(1), civ_core::stats::Fixed::whole(79));
    assert_eq!(
        before,
        serde_json::to_value(player_view(&w, &r, me)).unwrap()
    );
}

/// Full-match persistence gate, run explicitly in release mode.
#[test]
#[ignore = "long match gate: cargo test --release -p civ-sim complete_matches_save_and_replay -- --ignored --nocapture"]
fn complete_matches_save_and_replay() {
    for (seed, nation) in [(1, "Mali"), (7, "Majapahit"), (19, "Chola")] {
        let mut s = Session::new(
            Settings {
                seed,
                nation: Some(nation.into()),
                majors: 4,
                city_states: 2,
                ..Settings::default()
            },
            format!("complete-{seed}"),
            vec![0, 1, 2, 3],
        )
        .unwrap();
        for seat in 0..4 {
            s = s.stage_controller(seat, false).unwrap().next;
        }
        let mut restores = 0;
        while !s.finished() && s.turn() < 500 {
            s = s.stage_timeout(s.turn()).unwrap().unwrap().next;
            if s.turn().is_multiple_of(25) {
                let snapshot = s.save().unwrap_or_else(|error| {
                    let path = std::env::temp_dir().join("civ-failed-world.json");
                    std::fs::write(&path, serde_json::to_vec_pretty(s.world()).unwrap()).unwrap();
                    panic!(
                        "seed={seed} turn={} save failed: {error:?}; world={}",
                        s.turn(),
                        path.display()
                    );
                });
                let restored = Session::restore(&snapshot).unwrap_or_else(|error| {
                    let path = std::env::temp_dir().join("civ-failed-match.json");
                    std::fs::write(&path, &snapshot).unwrap();
                    panic!(
                        "seed={seed} turn={} restore failed: {error:?}; snapshot={}",
                        s.turn(),
                        path.display()
                    );
                });
                assert_eq!(s.world(), restored.world());
                s = restored;
                restores += 1;
            }
        }
        assert!(s.finished(), "seed{seed} did not finish by the turn limit");
        s.verify_replay().unwrap();
        assert!(s.stage_timeout(s.turn()).unwrap().is_none());
        let (winner, condition) = s.world().winner.unwrap();
        println!("seed={seed} nation={nation} turn={} victory={} winner={} restores={restores} replay=ok",s.turn(),s.rules().victories[condition].name,winner.0);
    }
}

#[test]
fn the_turn_limit_follows_speed_unless_set_explicitly() {
    let limit = |speed: Option<&str>, max_turns: Option<u32>| {
        let (world, rules, me) = new_game(&Settings {
            speed: speed.map(Into::into),
            max_turns,
            ..Settings::default()
        })
        .unwrap();
        (world, rules, me)
    };
    let (quick, rules, me) = limit(Some("Quick"), None);
    assert_eq!(quick.endgame.max_turns, 160, "Speeds.json: Quick ends at turn 160");
    let (standard, _, _) = limit(None, None);
    assert_eq!(standard.endgame.max_turns, 240, "absent speed is Standard");
    assert_eq!(standard.speed, rules.speeds.id("Standard"));
    let (explicit, _, _) = limit(Some("Quick"), Some(42));
    assert_eq!(explicit.endgame.max_turns, 42, "an explicit maxTurns overrides the speed");
    assert_eq!(player_view(&explicit, &rules, me).endgame.max_turns, 42);

    // The speed is the game's, not only its length: Quick research costs two thirds.
    let tech = rules.technologies.id("Pottery").unwrap();
    let cost = |w: &civ_core::world::World| {
        civ_core::world::research::tech_cost_in(w, &rules, me, tech, TurnModifiers::for_world(w, &rules).science).unwrap()
    };
    assert!(cost(&quick) < cost(&standard), "{} vs {}", cost(&quick), cost(&standard));

    // Saves keep it.
    let save = civ_core::save::encode(&quick, &rules).unwrap();
    assert_eq!(civ_core::save::decode(&save, &rules).unwrap().speed, rules.speeds.id("Quick"));

    let unknown = new_game(&Settings { speed: Some("Ludicrous".into()), ..Settings::default() });
    assert_eq!(unknown.unwrap_err(), "unknown speed: Ludicrous");
    let zero = new_game(&Settings { max_turns: Some(0), ..Settings::default() });
    assert!(zero.is_err());
}

#[test]
fn standard_speed_changes_nothing_from_the_defaults() {
    let rules = civ_sim::content::load().unwrap();
    let standard = TurnModifiers::for_speed(&rules, rules.speeds.id("Standard"));
    let default = TurnModifiers::default();
    assert_eq!(standard.barbarian, default.barbarian);
    // Growth and policy costs follow Speeds.json's growthModifier and cultureCostModifier, tuned for 240 turns.
    assert_ne!(standard.growth, default.growth);
    assert_ne!(standard.policies, default.policies);
    // Standard is now 240 turns; CostModifiers::default() still assumes Unciv's 500 (an Engine ask).
    let costs = civ_core::world::construction::CostModifiers { game_length: default.costs.game_length, ..standard.costs };
    assert_eq!(standard.costs.game_length, 240);
    assert_eq!(costs, default.costs);
    assert_eq!(standard.science, default.science);
}

#[test]
fn blitz_layers_over_the_full_game() {
    use civ_sim::content;
    let full = content::load().unwrap();
    let blitz = content::load_ruleset(content::BLITZ).unwrap();
    // Its own speed, sixty turns long (the Cradles slice); the full game's speeds are still there.
    let speed = blitz.speeds.by_name("Blitz").unwrap();
    assert_eq!(speed.total_turns, 60);
    assert!(blitz.speeds.by_name("Standard").is_some() && full.speeds.by_name("Blitz").is_none());
    // No diplomatic victory, and no United Nations to build towards one.
    assert!(blitz.victories.by_name("Perpetual Peace").is_none() && full.victories.by_name("Perpetual Peace").is_some());
    assert!(blitz.building("World Charter").is_none() && full.building("World Charter").is_some());
    assert!(blitz.dangling_refs().len() <= full.dangling_refs().len(), "{:?}", blitz.dangling_refs());
    // Everything not overridden is the full game's.
    assert_eq!(blitz.technologies.len(), full.technologies.len());
    assert_eq!(content::hash_of(content::DEFAULT_RULESET).unwrap(), content::hash());
    assert_ne!(content::hash_of(content::BLITZ).unwrap(), content::hash());
    assert!(content::load_ruleset("chess").is_err());
}

#[test]
fn a_blitz_game_starts_small_and_fast() {
    let settings = Settings {
        ruleset: Some("blitz".into()),
        ..Settings::default()
    };
    let (world, rules, me) = civ_sim::new_game_for(&settings, &[0]).unwrap();
    assert_eq!(world.endgame.max_turns, 60);
    assert_eq!(world.speed, rules.speeds.id("Blitz"));
    let names = |civ: CivId| {
        let mut v: Vec<_> = world
            .civ(civ)
            .unwrap()
            .units
            .iter()
            .map(|id| rules.units[world.units.get(*id).unwrap().base].name.clone())
            .collect();
        v.sort();
        v
    };
    // Settler, warrior, worker and scout (or their national replacements), for the human and the AI alike.
    for civ in [me, CivId(1)] {
        let units = names(civ);
        assert_eq!(units.len(), 4, "{units:?}");
        assert!(units.iter().any(|u| u == "Worker") && units.iter().any(|u| u == "Settler"), "{units:?}");
    }
    // The full game's start is untouched.
    let (full, full_rules, _) = civ_sim::new_game_for(&Settings::default(), &[0]).unwrap();
    assert_eq!(full.civ(CivId(0)).unwrap().units.len(), 2);
    assert_eq!(full.endgame.max_turns, 240);
    assert!(full_rules.victories.by_name("Perpetual Peace").is_some());
}

#[test]
fn a_blitz_save_never_loads_as_the_full_game() {
    let session = Session::new(
        Settings {
            ruleset: Some("blitz".into()),
            majors: 2,
            city_states: 0,
            ..Settings::default()
        },
        "blitz".into(),
        vec![0],
    )
    .unwrap();
    let save = session.save().unwrap();
    assert!(Session::restore(&save).is_ok());
    // Relabel it as a full game: the content hash no longer matches.
    let mut json: serde_json::Value = serde_json::from_str(&save).unwrap();
    json["settings"]["ruleset"] = serde_json::Value::Null;
    assert!(Session::restore(&json.to_string()).is_err());
    // And the world snapshot itself names Blitz's rules.
    let full = civ_sim::content::load().unwrap();
    let world = civ_core::save::encode(session.world(), session.rules()).unwrap();
    assert!(civ_core::save::decode(&world, &full).is_err());
}

#[test]
#[ignore = "long match gate: cargo test --release -p civ-sim --test normal_match a_blitz_game_plays_to_a_victory -- --ignored"]
fn a_blitz_game_plays_to_a_victory() {
    let (mut world, rules, _) = civ_sim::new_game(&Settings {
        ruleset: Some("blitz".into()),
        seed: 3,
        ..Settings::default()
    })
    .unwrap();
    while world.winner.is_none() && world.turn <= 81 {
        let mods = TurnModifiers::for_world(&world, &rules);
        civ_core::world::finish_round(&mut world, &rules, civ_core::world::CityFocus::default(), mods);
    }
    let (_, victory) = world.winner.expect("a Blitz game ends by its turn limit");
    assert!(world.victory_turn.is_some_and(|t| t <= 80), "{:?} {}", world.victory_turn, rules.victories[victory].name);
}
