//! Great people.
//!
//! The last of the accumulation loops, and the same shape as the others:
//! points gather, a threshold is crossed, something is born. Food grows a
//! citizen, production finishes a building, science learns a tech, culture
//! buys a policy — and specialists and wonders produce a great person.
//!
//! One rule is distinctive enough to be worth stating: the threshold
//! **doubles** after each birth (`GreatPersonManager.kt:83`), so they come
//! 100, 200, 400, 800 points apart rather than at a steady rate. An empire
//! does not get steadily better at producing them; it gets worse, and has to
//! grow to keep up.

use crate::ruleset::Ruleset;
use crate::stats::Fixed;
use crate::world::grid::CivId;
use crate::world::world::World;
use std::collections::BTreeMap;

/// Points needed for the first great person of a kind.
pub const FIRST_THRESHOLD: i32 = 100;

/// Points a civ has gathered, and what the next one costs.
///
/// Ordered, so iteration and serialisation are reproducible.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GreatPeople {
    pub points: BTreeMap<String, i32>,
    /// Absent means the first one, which costs [`FIRST_THRESHOLD`].
    pub thresholds: BTreeMap<String, i32>,
}

impl GreatPeople {
    pub fn threshold(&self, kind: &str, speed: Fixed) -> i32 {
        let base = self.thresholds.get(kind).copied().unwrap_or(FIRST_THRESHOLD);
        Fixed::whole(base).mul(speed).floor().max(1) as i32
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GreatPersonEvent {
    Born { kind: String, next_threshold: i32 },
    Progressed { kind: String, points: i32, needed: i32 },
}

/// Points a civ's cities generate this turn, by kind.
///
/// From buildings' `greatPersonPoints`. Specialists also produce them in Civ;
/// specialist slots are loaded but nobody is assigned to them yet, so that
/// source is missing rather than wrong.
pub fn points_per_turn(world: &World, r: &Ruleset, civ: CivId) -> BTreeMap<String, i32> {
    let mut out: BTreeMap<String, i32> = BTreeMap::new();
    let Some(c) = world.civ(civ) else { return out };
    for city in c.cities.iter().filter_map(|id| world.cities.get(*id)) {
        for building in city.built.iter().filter_map(|id| r.buildings.get(*id)) {
            for (kind, amount) in &building.great_person_points {
                *out.entry(kind.clone()).or_insert(0) += amount;
            }
        }
    }
    out
}

/// Bank a turn's points and see who turns up.
///
/// At most one birth per kind per turn, which matches Unciv returning a single
/// great person from `getNewGreatPerson`.
pub fn accumulate(
    people: &mut GreatPeople,
    gained: &BTreeMap<String, i32>,
    speed: Fixed,
) -> Vec<GreatPersonEvent> {
    for (kind, amount) in gained {
        *people.points.entry(kind.clone()).or_insert(0) += amount;
    }

    let mut events = Vec::new();
    let kinds: Vec<String> = people.points.keys().cloned().collect();
    for kind in kinds {
        let needed = people.threshold(&kind, speed);
        let have = people.points.get(&kind).copied().unwrap_or(0);
        if have >= needed {
            people.points.insert(kind.clone(), have - needed);
            // Doubling, not a fixed step: the second costs twice the first.
            let raw = people.thresholds.get(&kind).copied().unwrap_or(FIRST_THRESHOLD);
            people.thresholds.insert(kind.clone(), raw * 2);
            events.push(GreatPersonEvent::Born { kind, next_threshold: raw * 2 });
        } else if have > 0 {
            events.push(GreatPersonEvent::Progressed { kind, points: have, needed });
        }
    }
    events
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gained(kind: &str, n: i32) -> BTreeMap<String, i32> {
        BTreeMap::from([(kind.to_string(), n)])
    }

    #[test]
    fn points_gather_until_someone_is_born() {
        let mut p = GreatPeople::default();
        let none = gained("Great Scientist", 60);
        assert_eq!(
            accumulate(&mut p, &none, Fixed::ONE),
            vec![GreatPersonEvent::Progressed {
                kind: "Great Scientist".into(),
                points: 60,
                needed: 100
            }]
        );

        assert_eq!(
            accumulate(&mut p, &gained("Great Scientist", 50), Fixed::ONE),
            vec![GreatPersonEvent::Born {
                kind: "Great Scientist".into(),
                next_threshold: 200
            }]
        );
        assert_eq!(p.points["Great Scientist"], 10, "the excess carries over");
    }

    /// The rule worth remembering: each one costs twice the last.
    #[test]
    fn the_threshold_doubles_rather_than_climbing_steadily() {
        let mut p = GreatPeople::default();
        let mut thresholds = Vec::new();
        for _ in 0..4 {
            thresholds.push(p.threshold("Great Artist", Fixed::ONE));
            // Enough to trigger exactly one birth.
            let need = p.threshold("Great Artist", Fixed::ONE);
            accumulate(&mut p, &gained("Great Artist", need), Fixed::ONE);
        }
        assert_eq!(thresholds, vec![100, 200, 400, 800]);
    }

    #[test]
    fn game_speed_scales_the_cost() {
        let p = GreatPeople::default();
        assert_eq!(p.threshold("Great Engineer", Fixed::ONE), 100);
        assert_eq!(p.threshold("Great Engineer", Fixed::whole(3)), 300, "marathon");
        assert_eq!(p.threshold("Great Engineer", Fixed::permille(670)), 67, "quick");
    }

    #[test]
    fn kinds_accumulate_independently() {
        let mut p = GreatPeople::default();
        accumulate(&mut p, &gained("Great Scientist", 100), Fixed::ONE);
        assert_eq!(p.threshold("Great Scientist", Fixed::ONE), 200);
        assert_eq!(p.threshold("Great Merchant", Fixed::ONE), 100, "a different pool");
    }

    #[test]
    fn buildings_are_where_the_points_come_from() {
        use crate::world::city::City;
        use crate::world::entities::Civilization;
        use crate::world::grid::TileGrid;

        let r = Ruleset::load_dir("../android/assets/jsons/Civ V - Gods & Kings")
            .unwrap()
            .unwrap();
        let g = TileGrid::new(8, 8, r.terrain("Grassland").unwrap());
        let nation = r.nations.ids().next().unwrap();
        let mut w = World::new(g, vec![Civilization::new(CivId(0), nation, false)]);
        let city = w.found_city(City::found("Rome", CivId(0), crate::world::grid::TileId(10)));
        assert!(points_per_turn(&w, &r, CivId(0)).is_empty());

        let (id, b) = r
            .buildings
            .iter()
            .find(|(_, b)| !b.great_person_points.is_empty())
            .expect("some building produces great people");
        w.cities.get_mut(city).unwrap().built.insert(id);
        let points = points_per_turn(&w, &r, CivId(0));
        println!("{}: {:?}", b.name, points);
        assert!(!points.is_empty());
    }
}
