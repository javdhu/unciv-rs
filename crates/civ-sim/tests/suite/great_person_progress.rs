// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! A player's own great person progress (`KnowledgeView.greatPeople`): civ-core's numbers for that civ, and
//! nobody else's.

use civ_core::ruleset::Ruleset;
use civ_core::world::great_people::points_per_turn;
use civ_core::world::{CivId, World};
use civ_sim::session::Session;
use civ_sim::{player_view, GreatPersonProgressView, PlayerView, Settings};
use std::collections::BTreeSet;

const ME: CivId = CivId(0);

fn unit_name(r: &Ruleset, unit: u32) -> &str {
    r.units.iter().find(|(id, _)| id.index() == unit).map(|(_, b)| b.name.as_str()).expect("a unit in the rules")
}

/// Each entry is what civ-core holds for that kind, and the kinds are the ones points can earn.
fn matches_engine(w: &World, r: &Ruleset, entries: &[GreatPersonProgressView]) {
    let people = &w.civ(ME).unwrap().great_people;
    let earned = points_per_turn(w, r, ME);
    let speed = w
        .speed
        .and_then(|id| r.speeds.get(id))
        .and_then(|s| s.great_person_modifier)
        .expect("the bundled speeds price great people");
    let keys: Vec<_> = entries.iter().map(|e| (e.unit, e.combat)).collect();
    let mut sorted = keys.clone();
    sorted.sort();
    sorted.dedup();
    assert_eq!(keys, sorted, "sorted by unit, one entry a kind");
    let mut civilian = BTreeSet::new();
    let mut combat = BTreeSet::new();
    for e in entries {
        let name = unit_name(r, e.unit);
        if e.combat {
            combat.insert(name);
            assert_eq!(e.points, people.combat_points.get(name).copied().unwrap_or(0), "{name}");
            assert_eq!(e.threshold, people.combat_thresholds.get(name).copied().unwrap_or(200), "{name}");
            assert_eq!((e.per_turn, e.pool.as_deref()), (0, None), "{name}");
        } else {
            civilian.insert(name);
            let group = r
                .units
                .by_name(name)
                .unwrap()
                .uniques
                .iter()
                .find(|u| u.text.starts_with("Is part of Great Person group"))
                .map(|u| u.params[0].clone())
                .unwrap_or_default();
            assert_eq!(e.pool.as_deref(), Some(group.as_str()), "{name}");
            assert_eq!(e.points, people.points.get(name).copied().unwrap_or(0), "{name}");
            assert_eq!(e.per_turn, earned.get(name).copied().unwrap_or(0), "{name}");
            assert_eq!(e.threshold, people.threshold(&group, speed), "{name}");
        }
    }
    let from_cities: BTreeSet<&str> = r
        .buildings
        .iter()
        .flat_map(|(_, b)| b.great_person_points.keys())
        .chain(r.specialists.iter().flat_map(|(_, s)| s.great_person_points.keys()))
        .map(String::as_str)
        .collect();
    assert_eq!(civilian, from_cities, "every kind a building or specialist earns, and only those");
    assert!(!combat.is_empty());
    for name in &combat {
        assert!(r.units.by_name(name).unwrap().uniques.iter().any(|u| u.text == "Can be earned through combat"));
    }
    assert!(
        !civilian.contains("Sage") && !combat.contains("Sage"),
        "the prophet comes from faith, not points"
    );
}

/// Other nations' progress changes nothing in this player's view, and the only progress list in it is its own.
fn shows_no_one_elses(w: &World, r: &Ruleset, view: &PlayerView) {
    assert_eq!(&player_view(w, r, ME), view);
    let mut changed = w.clone();
    for c in changed.civs.iter_mut().filter(|c| c.id != ME) {
        let gp = &mut c.great_people;
        gp.points.insert("Scholar".into(), 9_999);
        gp.thresholds.insert("Great Person".into(), 7_777);
        gp.combat_points.insert("Marshal".into(), 5_555);
        gp.combat_thresholds.insert("Marshal".into(), 4_444);
    }
    assert_eq!(&player_view(&changed, r, ME), view, "another nation's great people leaked into the view");

    fn lists(v: &serde_json::Value, path: &str, out: &mut Vec<String>) {
        match v {
            serde_json::Value::Object(m) => {
                for (k, x) in m {
                    let here = format!("{path}.{k}");
                    if k == "greatPeople" && x.is_array() {
                        out.push(here.clone());
                    }
                    lists(x, &here, out);
                }
            }
            serde_json::Value::Array(a) => a.iter().enumerate().for_each(|(i, x)| lists(x, &format!("{path}[{i}]"), out)),
            _ => {}
        }
    }
    let json = serde_json::to_value(view).unwrap();
    let mut found = Vec::new();
    lists(&json, "", &mut found);
    assert_eq!(found, [".knowledge.greatPeople"]);
    for entry in json["knowledge"]["greatPeople"].as_array().unwrap() {
        let keys: BTreeSet<&str> = entry.as_object().unwrap().keys().map(String::as_str).collect();
        let combat = entry["combat"].as_bool().unwrap();
        let mut want = BTreeSet::from(["unit", "points", "perTurn", "threshold", "combat"]);
        if !combat {
            want.insert("pool");
        }
        assert_eq!(keys, want);
    }
}

#[test]
fn own_great_person_progress_is_civ_cores_count_for_that_civ_alone() {
    let settings = Settings { seed: 11, width: 24, height: 16, majors: 2, city_states: 1, ..Settings::default() };
    let mut session = Session::new(settings, "great-people".into(), vec![0]).unwrap();
    // A bot plays the seat, so it builds what earns points.
    session = session.stage_controller(0, false).unwrap().next;
    let (mut grown, mut births) = (0, 0);
    let mut last: Option<Vec<GreatPersonProgressView>> = None;
    for _ in 0..ROUNDS {
        let view = session.current_update(0).unwrap().view;
        let entries = view.knowledge.great_people.clone().expect("a major nation can earn great people");
        matches_engine(session.world(), session.rules(), &entries);
        shows_no_one_elses(session.world(), session.rules(), &view);
        if let Some(last) = &last {
            assert_eq!(
                last.iter().map(|e| (e.unit, e.combat)).collect::<Vec<_>>(),
                entries.iter().map(|e| (e.unit, e.combat)).collect::<Vec<_>>(),
                "the same kinds every turn"
            );
            for (was, now) in last.iter().zip(&entries).filter(|(e, _)| !e.combat) {
                // The round's gain is the rate the turn found; the civ's own turn may change it first.
                let gains = [was.per_turn, now.per_turn];
                if now.threshold == was.threshold {
                    assert!(gains.contains(&(now.points - was.points)), "{was:?} -> {now:?}");
                    grown += usize::from(was.per_turn > 0 && now.points - was.points == was.per_turn);
                } else {
                    // A birth in the pool: this kind's, which spends the threshold it had, or a pool-mate's.
                    assert!(now.threshold > was.threshold, "{was:?} -> {now:?}");
                    let born = gains.iter().any(|g| now.points == was.points + g - was.threshold && was.points + g >= was.threshold);
                    assert!(born || gains.iter().any(|g| now.points == was.points + g), "{was:?} -> {now:?}");
                    births += usize::from(born);
                }
            }
        }
        last = Some(entries);
        session = session.stage_timeout(session.turn()).unwrap().expect("the match goes on").next;
    }
    assert!(grown > 0, "points never grew by the rate shown");
    assert!(births > 0, "no great person was born to check the threshold by");
}

const ROUNDS: usize = 100;
