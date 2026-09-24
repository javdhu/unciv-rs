//! Great-person points, births and the Maya calendar.
//!
//! Civilian births double their shared point-pool threshold, starting at 100.
//! Combat births use separate General/Admiral counters starting at 200 and
//! increasing by 50. Blocked placement retains the points for a later turn.

use crate::arena::Id;
use crate::ruleset::Ruleset;
use crate::stats::Fixed;
use crate::world::conditionals::{applies, GameContext, Verdict};
use crate::world::entities::Unit;
use crate::world::grid::CivId;
use crate::world::world::World;
use crate::UniqueType;
use std::collections::BTreeMap;

/// Points needed for the first great person of a kind.
pub const FIRST_THRESHOLD: i32 = 100;

/// Points a civ has gathered, and what the next one costs.
///
/// Ordered, so iteration and serialisation are reproducible.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GreatPeople {
    pub points: BTreeMap<String, i32>,
    /// Absent means the first one, which costs [`FIRST_THRESHOLD`].
    pub thresholds: BTreeMap<String, i32>,
    /// Combat awards use a separate, unscaled 200, 250, 300... progression.
    pub combat_points: BTreeMap<String, i32>,
    pub combat_thresholds: BTreeMap<String, i32>,
    pub maya_restricted: u32,
    pub maya_available: std::collections::BTreeSet<Id<crate::ruleset::BaseUnit>>,
    pub maya_last_award_turn: Option<u32>,
}

impl GreatPeople {
    pub fn threshold(&self, kind: &str, speed: Fixed) -> i32 {
        let base = self.thresholds.get(kind).copied().unwrap_or(FIRST_THRESHOLD);
        (Fixed::whole(base) * speed).floor().max(1) as i32
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GreatPersonEvent {
    Born { kind: String, next_threshold: i32 },
    Progressed { kind: String, points: i32, needed: i32 },
}

/// Points a civ's cities generate this turn, by kind.
///
/// From buildings' `greatPersonPoints`, and from the specialists at work.
pub fn points_per_turn(world: &World, r: &Ruleset, civ: CivId) -> BTreeMap<String, i32> {
    let mut out: BTreeMap<String, i32> = BTreeMap::new();
    let Some(c) = world.civ(civ) else { return out };
    for id in &c.cities {
        let Some(city) = world.cities.get(*id) else { continue };
        let mut base: BTreeMap<String, i32> = BTreeMap::new();
        for building in city.built.iter().filter_map(|id| r.buildings.get(*id)) {
            for (kind, n) in &building.great_person_points {
                *base.entry(kind.clone()).or_default() += *n;
            }
        }
        for (kind, n) in crate::world::specialists::points(r, city) {
            *base.entry(kind).or_default() += n;
        }
        let ctx = GameContext::new(r).with_world(world).for_civ(civ).in_city(*id);
        let uniques = crate::world::religion::city_uniques(world, r, city);
        let general: i32 = uniques
            .iter()
            .filter(|u| {
                u.unique_type() == Some(UniqueType::GreatPersonPointPercentage)
                    && applies(u, &ctx) == Verdict::True
                    && u.params.get(1).is_some_and(|f| {
                        crate::world::filters::city_matches(world, r, city, Some(city), f) == Verdict::True
                    })
            })
            .filter_map(|u| u.params.first()?.parse::<i32>().ok())
            .sum();
        let friendship: i32 = world
            .civs
            .iter()
            .filter(|other| {
                other.id != civ && world.diplomacy.pact(civ, other.id).is_some_and(|p| p.friendship_turns > 0)
            })
            .flat_map(|other| {
                c.uniques
                    .matching(UniqueType::GreatPersonBoostWithFriendship)
                    .chain(other.uniques.matching(UniqueType::GreatPersonBoostWithFriendship))
            })
            .filter_map(|u| u.params.first()?.parse::<i32>().ok())
            .sum();
        for (kind, n) in base {
            let Some(unit) = r.units.id(&kind) else { continue };
            let specific: i32 = uniques
                .iter()
                .filter(|u| {
                    u.unique_type() == Some(UniqueType::GreatPersonEarnedFaster)
                        && applies(u, &ctx) == Verdict::True
                        && u.params
                            .first()
                            .is_some_and(|f| crate::world::filters::base_unit_matches(r, unit, f) == Verdict::True)
                })
                .filter_map(|u| u.params.get(1)?.parse::<i32>().ok())
                .sum();
            let amount = (Fixed::whole(n) * Fixed::from_fraction(i64::from(100 + general + specific + friendship), 100))
                .round()
                .max(0) as i32;
            *out.entry(kind).or_default() += amount;
        }
    }
    out
}

/// Military calls this with actual awarded XP before a defeated unit is removed.
pub fn on_combat_xp(world: &mut World, r: &Ruleset, unit: Id<Unit>, amount: u16, enemy_is_barbarian: bool) {
    let Some(u) = world.units.get(unit) else { return };
    let owner = u.owner;
    if enemy_is_barbarian || amount == 0 || !world.civ(owner).is_some_and(|c| c.is_major()) {
        return;
    }
    let ctx = GameContext::new(r).with_world(world).for_civ(owner).for_unit(unit).on_tile(u.position);
    let civ = world.civ(owner).unwrap();
    let bonuses: Vec<_> = civ
        .uniques
        .matching(UniqueType::GreatPersonEarnedFaster)
        .chain(
            r.units
                .get(u.base)
                .into_iter()
                .flat_map(|b| b.uniques.iter())
                .filter(|u| u.unique_type() == Some(UniqueType::GreatPersonEarnedFaster)),
        )
        .chain(
            u.promotions
                .iter()
                .filter_map(|id| r.promotions.get(*id))
                .flat_map(|p| p.uniques.iter())
                .filter(|u| u.unique_type() == Some(UniqueType::GreatPersonEarnedFaster)),
        )
        .filter(|u| applies(u, &ctx) == Verdict::True)
        .collect();
    let candidates: Vec<_> = r
        .units
        .iter()
        .filter(|(id, b)| {
            crate::world::effects::equivalent_unit(world, r, owner, *id) == *id
                && b.unique_to.as_deref().is_none_or(|n| r.nations.name_of(civ.nation) == Some(n))
                && b.uniques.iter().any(|u| {
                    u.unique_type() == Some(UniqueType::GreatPersonFromCombat) && applies(u, &ctx) == Verdict::True
                })
        })
        .map(|(id, b)| {
            let percent: i32 = bonuses
                .iter()
                .filter(|u| {
                    u.params
                        .first()
                        .is_some_and(|f| crate::world::filters::base_unit_matches(r, id, f) == Verdict::True)
                })
                .filter_map(|u| u.params.get(1)?.parse::<i32>().ok())
                .sum();
            (
                b.name.clone(),
                (Fixed::whole(i32::from(amount)) * Fixed::from_fraction(i64::from(100 + percent), 100)).floor().max(0)
                    as i32,
            )
        })
        .collect();
    let people = &mut world.civ_mut(owner).unwrap().great_people;
    for (kind, n) in candidates {
        let value = people.combat_points.entry(kind).or_default();
        *value = value.saturating_add(n);
    }
}

fn pool_key(r: &Ruleset, kind: &str) -> String {
    r.units
        .by_name(kind)
        .and_then(|b| b.uniques.iter().find(|u| u.unique_type() == Some(UniqueType::GPPointPool)))
        .and_then(|u| u.params.first())
        .cloned()
        .unwrap_or_default()
}

/// Live births consume points only when a legal nearby slot exists. Combat
/// counters precede civilian pools, which use the ruleset's shared GP group.
pub fn process_turn(world: &mut World, r: &Ruleset, civ: CivId, speed: Fixed) -> Vec<String> {
    let gained = points_per_turn(world, r, civ);
    let Some(c) = world.civ_mut(civ) else { return Vec::new() };
    for (kind, n) in gained {
        let value = c.great_people.points.entry(kind).or_default();
        *value = value.saturating_add(n);
    }
    let mut births = Vec::new();
    while let Some(c) = world.civ(civ) {
        let combat = c
            .great_people
            .combat_points
            .iter()
            .find(|(kind, n)| **n >= c.great_people.combat_thresholds.get(*kind).copied().unwrap_or(200))
            .map(|(kind, _)| (kind.clone(), true));
        let next = combat.or_else(|| {
            c.great_people
                .points
                .iter()
                .find(|(kind, n)| **n >= c.great_people.threshold(&pool_key(r, kind), speed))
                .map(|(kind, _)| (kind.clone(), false))
        });
        let Some((kind, combat)) = next else { break };
        let Some(base) = r.units.id(&kind) else { break };
        let base = crate::world::effects::equivalent_unit(world, r, civ, base);
        let sea = r.unit_types.by_name(&r.units[base].unit_type).is_some_and(|t| t.movement_type == "Water");
        let prepared =
            c.cities.iter().copied().filter(|id| world.cities.get(*id).is_some_and(|c| !sea || c.coastal)).find_map(
                |id| {
                    let city = world.cities.get(id)?;
                    crate::world::construction::prepare_unit_near(world, r, civ, base, city.centre, Some(id))
                },
            );
        let Some(unit) = prepared else { break };
        let id = world.place_unit(unit);
        crate::world::luminaries::name_unit(world, r, id);
        let people = &mut world.civ_mut(civ).unwrap().great_people;
        if combat {
            let needed = people.combat_thresholds.get(&kind).copied().unwrap_or(200);
            *people.combat_points.get_mut(&kind).unwrap() -= needed;
            people.combat_thresholds.insert(kind.clone(), needed.saturating_add(50));
        } else {
            let pool = pool_key(r, &kind);
            let needed = people.threshold(&pool, speed);
            *people.points.get_mut(&kind).unwrap() -= needed;
            let base = people.thresholds.get(&pool).copied().unwrap_or(FIRST_THRESHOLD);
            people.thresholds.insert(pool, base.saturating_mul(2));
        }
        births.push(kind);
    }
    births
}

/// Calendar used by the bundled Long Count ability. Fractional years truncate
/// toward zero, matching Speed.turnToYear(...).toInt().
pub fn calendar_year(r: &Ruleset, speed: Fixed, turn: u32) -> i32 {
    let Some(s) = r.speeds.iter().find(|(_, s)| s.modifier == speed).map(|(_, s)| s) else { return -4000 };
    speed_year(s, turn)
}

/// The year on `turn` of this game, by its own speed (speeds are told apart
/// by id: Blitz and Standard may share a modifier).
pub fn calendar_year_in(world: &World, r: &Ruleset, turn: u32) -> i32 {
    let modifier = crate::world::turn::TurnModifiers::for_world(world, r).costs.speed;
    world.speed_rules(r, modifier).map_or(-4000, |s| speed_year(s, turn))
}

fn speed_year(s: &crate::ruleset::Speed, turn: u32) -> i32 {
    let mut year = Fixed::whole(s.start_year);
    let mut start = 0;
    for (length, end) in &s.year_intervals {
        let end = if *end == s.total_turns { turn as i32 } else { *end };
        let n = (turn as i32).min(end).saturating_sub(start).max(0);
        year += *length * Fixed::whole(n);
        if turn as i32 <= end {
            break;
        }
        start = end;
    }
    (year.raw() / 1000) as i32
}
fn maya_cycle(year: i32) -> i32 {
    let days = (year + 3000) * 365 + (year + 3000) / 4 + 41291;
    let tuns = if days >= 0 { days / 360 } else { 5200 + days / 360 };
    tuns / 400
}
pub fn begin_round(world: &mut World, r: &Ruleset, speed: Fixed) {
    if world.turn == 0
        || {
            let year = |turn| world.speed_rules(r, speed).map_or(-4000, |s| speed_year(s, turn));
            maya_cycle(year(world.turn)) == maya_cycle(year(world.turn - 1))
        }
    {
        return;
    }
    let turn = world.turn;
    let candidates: Vec<_> = world
        .civs
        .iter()
        .filter(|c| {
            c.is_alive()
                && c.is_major()
                && c.great_people.maya_last_award_turn != Some(turn)
                && c.uniques
                    .matching(UniqueType::MayanGainGreatPerson)
                    .any(|u| u.params.get(1).and_then(|t| r.technologies.id(t)).is_some_and(|t| c.techs.contains(&t)))
        })
        .map(|c| c.id)
        .collect();
    for civ in candidates {
        let options = crate::world::effects::great_person_types(world, r, civ);
        let c = world.civ_mut(civ).unwrap();
        if c.great_people.maya_available.is_empty() {
            c.great_people.maya_available.extend(options);
        }
        c.great_people.maya_restricted = c.great_people.maya_restricted.saturating_add(1);
        c.pending_rewards.free_great_people = c.pending_rewards.free_great_people.saturating_add(1);
        c.great_people.maya_last_award_turn = Some(turn);
    }
}

/// Bank a turn's points and see who turns up.
///
/// At most one birth per kind per turn, which matches Unciv returning a single
/// great person from `getNewGreatPerson`.
pub fn accumulate(people: &mut GreatPeople, gained: &BTreeMap<String, i32>, speed: Fixed) -> Vec<GreatPersonEvent> {
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
            vec![GreatPersonEvent::Progressed { kind: "Great Scientist".into(), points: 60, needed: 100 }]
        );

        assert_eq!(
            accumulate(&mut p, &gained("Great Scientist", 50), Fixed::ONE),
            vec![GreatPersonEvent::Born { kind: "Great Scientist".into(), next_threshold: 200 }]
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

        let r = Ruleset::load_dir("rules/gods-and-kings").unwrap().unwrap();
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
