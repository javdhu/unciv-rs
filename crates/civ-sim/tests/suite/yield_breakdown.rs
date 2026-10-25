// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Where the numbers come from (`EconomyView.breakdown`, `CityStateView.yieldBreakdown`): every stat's entries sum
//! to the figure the view shows beside them, civ-core's parts sum to what its plan banks, and only the player's
//! own figures are broken down.

use civ_core::ruleset::Ruleset;
use civ_core::stats::{Fixed, Stat};
use civ_core::world::assignment::CityFocus;
use civ_core::world::turn::{plan_economy, TurnModifiers};
use civ_core::world::{CivId, World};
use civ_sim::session::Session;
use civ_sim::{PlayerView, Settings, YieldSourceView, YieldStat};
use std::collections::{BTreeMap, BTreeSet};

const ME: CivId = CivId(0);
const ROUNDS: usize = 80;

fn sums(entries: &[YieldSourceView]) -> BTreeMap<YieldStat, i32> {
    let mut out = BTreeMap::new();
    for e in entries {
        *out.entry(e.stat).or_insert(0) += e.amount;
    }
    out
}

/// Sorted by stat then source, one entry a pair, none of them zero, and only the stats allowed.
fn well_formed(entries: &[YieldSourceView], allowed: &[YieldStat], what: &str) {
    let pairs: Vec<_> = entries.iter().map(|e| (e.stat, e.source.as_str())).collect();
    let mut by_stat = pairs.clone();
    by_stat.sort_by_key(|(stat, _)| *stat);
    assert_eq!(pairs, by_stat, "{what}: sorted by stat");
    assert_eq!(pairs.iter().collect::<BTreeSet<_>>().len(), pairs.len(), "{what}: one entry a source: {entries:?}");
    for e in entries {
        assert_ne!(e.amount, 0, "{what}: {e:?}");
        assert!(allowed.contains(&e.stat), "{what}: {e:?}");
        assert!(!e.source.is_empty() && e.source.chars().all(|c| c.is_ascii_alphabetic()), "{what}: {e:?}");
    }
}

/// The view's breakdowns sum to its figures; the keys seen are gathered for the coverage check.
fn sums_match(view: &PlayerView, seen: &mut BTreeSet<(YieldStat, String)>) {
    let economy = &view.economy;
    let entries = economy.breakdown.as_ref().expect("the player's own economy is broken down");
    let civ_stats = [YieldStat::Gold, YieldStat::Science, YieldStat::Culture, YieldStat::Faith, YieldStat::Happiness];
    well_formed(entries, &civ_stats, "economy");
    let got = sums(entries);
    let want = [
        (YieldStat::Gold, economy.gold),
        (YieldStat::Science, economy.science),
        (YieldStat::Culture, economy.culture),
        (YieldStat::Faith, economy.faith),
        (YieldStat::Happiness, economy.happiness),
    ];
    for (stat, total) in want {
        assert_eq!(got.get(&stat).copied().unwrap_or(0), total, "turn {}: {stat:?} {entries:?}", view.turn);
    }
    seen.extend(entries.iter().map(|e| (e.stat, e.source.clone())));

    let city_stats = [
        YieldStat::Food,
        YieldStat::Production,
        YieldStat::Gold,
        YieldStat::Science,
        YieldStat::Culture,
        YieldStat::Faith,
    ];
    for city in &view.cities {
        let Some(own) = &city.own else {
            assert_ne!(city.owner, view.me, "an own city carries its own part");
            continue;
        };
        assert_eq!(city.owner, view.me);
        let entries = own.yield_breakdown.as_ref().expect("an own city is broken down");
        well_formed(entries, &city_stats, &city.name);
        let got = sums(entries);
        assert_eq!(got.get(&YieldStat::Food).copied().unwrap_or(0), own.food_per_turn, "{}: {entries:?}", city.name);
        assert_eq!(
            got.get(&YieldStat::Production).copied().unwrap_or(0),
            own.production_per_turn,
            "{}: {entries:?}",
            city.name
        );
        seen.extend(entries.iter().map(|e| (e.stat, e.source.clone())));
    }
}

/// civ-core's own parts: a city's production parts sum exactly to the production it banks (unless it converts its
/// production), its food parts to the food it banks before the engine's rounding, and the cities' parts with the
/// empire's sum to the reserves the plan truncates.
fn parts_match_the_plan(w: &World, r: &Ruleset) {
    let plan = plan_economy(w, r, ME, CityFocus::None, TurnModifiers::for_world(w, r));
    assert_eq!(plan.city_yields.len(), w.civ(ME).unwrap().cities.len(), "every city is tallied");
    let mut reserves = plan.income.total();
    for (city, parts) in &plan.city_yields {
        let made = parts.total();
        reserves += made;
        let banked = |of: &[(civ_core::arena::Id<civ_core::world::city::City>, i32)]| {
            of.iter().find(|(id, _)| id == city).map(|(_, n)| *n).unwrap()
        };
        let converts = w.cities.get(*city).unwrap().production_conversion.is_some();
        let production = if converts { 0 } else { made.production.round() as i32 };
        assert_eq!(made.production.round() as i32, production, "{parts:?}");
        assert_eq!(banked(&plan.production), production, "{parts:?}");
        let food = banked(&plan.food);
        assert!((made.food - Fixed::whole(food)).raw().abs() < 1_500, "food {food} from {parts:?}");
    }
    let truncate = |v: Fixed| (v.raw() / civ_core::stats::SCALE) as i32;
    assert_eq!(truncate(reserves[Stat::Gold]), plan.gold);
    assert_eq!(truncate(reserves[Stat::Science]), plan.science);
    assert_eq!(truncate(reserves[Stat::Culture]), plan.culture);
    assert_eq!(truncate(reserves[Stat::Faith]), plan.faith);
    assert_eq!(plan.happiness_parts.iter().map(|(_, n)| n).sum::<i32>(), plan.happiness);
}

/// No other nation's breakdown reaches the view: the only breakdowns in it are its economy's and its own cities'.
fn only_its_own(view: &PlayerView) {
    fn find(v: &serde_json::Value, path: &str, out: &mut Vec<String>) {
        match v {
            serde_json::Value::Object(m) => {
                for (k, x) in m {
                    let here = format!("{path}.{k}");
                    if k.to_lowercase().contains("breakdown") {
                        out.push(here.clone());
                    }
                    find(x, &here, out);
                }
            }
            serde_json::Value::Array(a) => a.iter().enumerate().for_each(|(i, x)| find(x, &format!("{path}[{i}]"), out)),
            _ => {}
        }
    }
    let json = serde_json::to_value(view).unwrap();
    let mut found = Vec::new();
    find(&json, "", &mut found);
    let mut want = vec![".economy.breakdown".to_owned()];
    for (i, city) in json["cities"].as_array().unwrap().iter().enumerate() {
        if city["owner"] == serde_json::json!(view.me) {
            want.push(format!(".cities[{i}].own.yieldBreakdown"));
        } else {
            assert!(city["own"].is_null(), "another nation's city carries its own part");
        }
    }
    found.sort();
    want.sort();
    assert_eq!(found, want);
}

#[test]
fn breakdowns_sum_to_the_figures_shown_and_only_for_its_own() {
    let settings = Settings { seed: 23, width: 28, height: 18, majors: 3, city_states: 2, ..Settings::default() };
    let mut session = Session::new(settings, "yield-breakdown".into(), vec![0]).unwrap();
    // A bot plays the seat, so the empire grows buildings, routes, upkeep and an army to account for.
    session = session.stage_controller(0, false).unwrap().next;
    let mut seen = BTreeSet::new();
    let mut most_cities = 0;
    for round in 0..=ROUNDS {
        if round == ROUNDS {
            // The player takes the seat back for the last round.
            session = session.stage_controller(0, true).unwrap().next;
        }
        let view = session.current_update(0).unwrap().view;
        sums_match(&view, &mut seen);
        only_its_own(&view);
        parts_match_the_plan(session.world(), session.rules());
        most_cities = most_cities.max(view.cities.iter().filter(|c| c.owner == view.me).count());
        if round < ROUNDS {
            session = session.stage_timeout(session.turn()).unwrap().expect("the match goes on").next;
        }
    }
    assert!(most_cities >= 2, "the empire never grew past {most_cities} cities");
    let keys = |stat: YieldStat| seen.iter().filter(|(s, _)| *s == stat).map(|(_, k)| k.as_str()).collect::<BTreeSet<_>>();
    for (stat, wanted) in [
        (YieldStat::Food, &["centre", "tiles", "consumption"][..]),
        (YieldStat::Production, &["centre", "tiles", "buildings"][..]),
        (YieldStat::Gold, &["buildings", "buildingUpkeep", "unitUpkeep", "tradeRoutes"][..]),
        (YieldStat::Science, &["population", "buildings"][..]),
        (YieldStat::Culture, &["buildings"][..]),
        (YieldStat::Happiness, &["difficulty", "cities", "population", "cityHappiness"][..]),
    ] {
        let have = keys(stat);
        for key in wanted {
            assert!(have.contains(key), "{stat:?} never came from {key}: {have:?}");
        }
    }
}
