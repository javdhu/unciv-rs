//! We Love The King Day: a city that asks for a luxury, and celebrates when
//! its civ has one.
//!
//! Each city names a luxury it wants — one that exists somewhere on the map
//! but not within its own reach, and one its civ lacks if there is any such.
//! It waits fifteen to twenty-four turns; once the civ holds the luxury, the
//! city celebrates for twenty (scaled by game speed), during which its surplus
//! food is a quarter larger, provided the empire is not unhappy. When the
//! celebration ends, or the wait runs out unmet, it asks for something else.
//! (`CityTurnManager.tryWeLoveTheKing`, `demandNewResource`,
//! `setWltkResourceDemandCooldown`; the growth bonus is `CityStats`.)
//!
//! The order within a turn is Unciv's and matters: a city tries to celebrate
//! first, then its countdowns run, so a city that gets its luxury this turn
//! celebrates from the next, and the celebration's first turn is counted off
//! in the same step that started it.

use crate::arena::Id;
use crate::rng::Rng;
use crate::ruleset::{Ruleset, TileResource};
use crate::stats::Fixed;
use crate::unique_type::UniqueType;
use crate::world::city::{City, WORK_RADIUS};
use crate::world::grid::CivId;
use crate::world::world::World;

/// Turns of celebration at standard speed.
const CELEBRATION_TURNS: i64 = 20;

/// What a turn of wanting and celebrating did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WltkdEvent {
    Demanded { city: Id<City>, resource: Id<TileResource> },
    Started { city: Id<City>, resource: Id<TileResource> },
    Ended { city: Id<City> },
}

/// Luxuries a city might ask for: not a city-state's own, not what it asked
/// for last, present somewhere on the map, and not already within its reach.
fn candidates(world: &World, r: &Ruleset, city: &City) -> Vec<Id<TileResource>> {
    let near = world.grid.within(city.centre, WORK_RADIUS);
    r.resources
        .iter()
        .filter(|(_, res)| res.resource_type == "Luxury")
        .filter(|(_, res)| !res.uniques.iter().any(|u| u.unique_type() == Some(UniqueType::CityStateOnlyResource)))
        .map(|(id, _)| id)
        .filter(|id| city.demanded_resource != Some(*id))
        .filter(|id| world.grid.resource.contains(&Some(*id)))
        .filter(|id| near.iter().all(|t| world.grid.resource[t.0 as usize] != Some(*id)))
        .collect()
}

fn has(world: &World, civ: CivId, resource: Id<TileResource>) -> bool {
    world.civ(civ).and_then(|c| c.resources.get(&resource)).is_some_and(|n| *n > 0)
}

fn rng(world: &World, city: &City, tag: u64) -> Rng {
    Rng::from_parts(&[tag, city.centre.0 as u64, world.turn as u64])
}

/// How long a city waits for what it asked for: 15 to 24 turns, ten more for
/// a capital just founded.
fn wait(world: &World, r: &Ruleset, city_id: Id<City>, new_city: bool) -> u32 {
    let Some(city) = world.cities.get(city_id) else { return 0 };
    let mut turns = 15 + rng(world, city, 0x0005_7A17).range(0, 9) as u32;
    if new_city && crate::world::connections::capital_of(world, r, city.owner) == Some(city_id) {
        turns += 10;
    }
    turns
}

/// Ask for a new luxury (`demandNewResource`). One the civ lacks if any; if
/// it has every one already, any candidate, and no wait — it is content.
fn demand(world: &mut World, r: &Ruleset, city_id: Id<City>) -> Option<WltkdEvent> {
    let city = world.cities.get(city_id)?;
    let all = candidates(world, r, city);
    let missing: Vec<_> = all.iter().copied().filter(|res| !has(world, city.owner, *res)).collect();
    let mut dice = rng(world, city, 0xDE_4A2D);
    let pick = |from: &[Id<TileResource>], dice: &mut Rng| {
        (!from.is_empty()).then(|| from[dice.range(0, from.len() as i64 - 1) as usize])
    };
    if missing.is_empty() {
        let chosen = pick(&all, &mut dice);
        world.cities.get_mut(city_id)?.demanded_resource = chosen;
        return None;
    }
    let chosen = pick(&missing, &mut dice)?;
    let turns = wait(world, r, city_id, false);
    let c = world.cities.get_mut(city_id)?;
    c.demanded_resource = Some(chosen);
    c.demand_countdown = turns;
    Some(WltkdEvent::Demanded { city: city_id, resource: chosen })
}

/// One turn of wanting and celebrating for a civ's cities.
pub fn tick(world: &mut World, r: &Ruleset, civ: CivId, speed: Fixed) -> Vec<WltkdEvent> {
    let mut events = Vec::new();
    let celebration = (Fixed::whole(CELEBRATION_TURNS as i32) * speed).round() as u32 + 1;
    let cities: Vec<Id<City>> = world.civ(civ).map(|c| c.cities.clone()).unwrap_or_default();
    for city_id in cities {
        let Some(c) = world.cities.get(city_id) else { continue };

        // Celebrate, if what it asked for has arrived.
        if c.wltkd == 0 {
            if let Some(res) = c.demanded_resource.filter(|res| has(world, civ, *res)) {
                let c = world.cities.get_mut(city_id).unwrap();
                c.wltkd = celebration;
                c.demand_countdown = 0;
                events.push(WltkdEvent::Started { city: city_id, resource: res });
            }
        }

        // The countdowns.
        let c = world.cities.get_mut(city_id).unwrap();
        let mut ask = false;
        if c.demand_countdown > 0 {
            c.demand_countdown -= 1;
            ask |= c.demand_countdown == 0 && c.wltkd == 0;
        }
        if c.wltkd > 0 {
            c.wltkd -= 1;
            if c.wltkd == 0 {
                events.push(WltkdEvent::Ended { city: city_id });
                ask = true;
            }
        }
        if ask {
            events.extend(demand(world, r, city_id));
        }

        // A city that wants nothing and is waiting for nothing starts waiting.
        let c = world.cities.get(city_id).unwrap();
        if c.demanded_resource.is_none() && c.demand_countdown == 0 && c.wltkd == 0 {
            let turns = wait(world, r, city_id, true);
            world.cities.get_mut(city_id).unwrap().demand_countdown = turns;
        }
    }
    events
}

/// A quarter more surplus food while the city celebrates, if the empire is
/// not unhappy and there is a surplus to add to (`CityStats`).
pub fn food_bonus(city: &City, surplus: i32, happiness: i32) -> Fixed {
    if city.wltkd > 0 && happiness >= 0 && surplus > 0 {
        Fixed::whole(surplus) * Fixed::permille(250)
    } else {
        Fixed::ZERO
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::entities::Civilization;
    use crate::world::grid::TileGrid;

    fn ruleset() -> Ruleset {
        Ruleset::load_dir("../android/assets/jsons/Civ V - Gods & Kings").expect("readable").expect("loads")
    }

    /// A capital at (8, 8), with Silk far away at (1, 1) and Wine near at (9, 8).
    fn world(r: &Ruleset) -> (World, Id<City>) {
        let g = TileGrid::new(16, 16, r.terrain("Grassland").unwrap());
        let nation = r.nations.ids().next().unwrap();
        let mut w = World::new(g, vec![Civilization::new(CivId(0), nation, false)]);
        let city = w.found_city_in(r, City::found("Home", CivId(0), w.grid.id_at(8, 8).unwrap()));
        let (silk, wine) = (r.resources.id("Silk").unwrap(), r.resources.id("Wine").unwrap());
        let far = w.grid.id_at(1, 1).unwrap();
        let near = w.grid.id_at(9, 8).unwrap();
        w.grid.resource[far.0 as usize] = Some(silk);
        w.grid.resource[near.0 as usize] = Some(wine);
        (w, city)
    }

    fn run(w: &mut World, r: &Ruleset, turns: u32) -> Vec<(u32, WltkdEvent)> {
        let mut out = Vec::new();
        for t in 1..=turns {
            w.turn += 1;
            out.extend(tick(w, r, CivId(0), Fixed::ONE).into_iter().map(|e| (t, e)));
        }
        out
    }

    #[test]
    fn a_city_asks_for_a_luxury_it_cannot_reach() {
        let r = ruleset();
        let (mut w, city) = world(&r);
        let silk = r.resources.id("Silk").unwrap();
        let events = run(&mut w, &r, 40);
        let asked: Vec<_> = events
            .iter()
            .filter_map(|(_, e)| match e {
                WltkdEvent::Demanded { resource, .. } => Some(*resource),
                _ => None,
            })
            .collect();
        // The only luxury on the map outside its reach.
        assert_eq!(asked.first(), Some(&silk), "{events:?}");
        assert_eq!(w.cities.get(city).unwrap().demanded_resource, Some(silk));
    }

    #[test]
    fn having_it_starts_twenty_turns_of_celebration() {
        let r = ruleset();
        let (mut w, city) = world(&r);
        let silk = r.resources.id("Silk").unwrap();
        w.cities.get_mut(city).unwrap().demanded_resource = Some(silk);
        w.cities.get_mut(city).unwrap().demand_countdown = 12;
        w.civ_mut(CivId(0)).unwrap().resources.insert(silk, 1);

        let events = run(&mut w, &r, 30);
        let started = events.iter().find(|(_, e)| matches!(e, WltkdEvent::Started { .. })).map(|(t, _)| *t);
        let ended = events.iter().find(|(_, e)| matches!(e, WltkdEvent::Ended { .. })).map(|(t, _)| *t);
        assert_eq!(started, Some(1));
        assert_eq!(ended, Some(21), "twenty turns, counted from the one it began in");
    }

    #[test]
    fn the_celebration_grows_the_surplus_by_a_quarter_unless_unhappy() {
        let mut c = City::found("Home", CivId(0), crate::world::grid::TileId(0));
        c.wltkd = 5;
        assert_eq!(food_bonus(&c, 8, 0), Fixed::whole(2));
        assert_eq!(food_bonus(&c, 8, -1), Fixed::ZERO, "an unhappy empire does not celebrate");
        assert_eq!(food_bonus(&c, -3, 5), Fixed::ZERO, "nothing to add to a deficit");
        c.wltkd = 0;
        assert_eq!(food_bonus(&c, 8, 5), Fixed::ZERO);
    }

    #[test]
    fn an_unmet_demand_gives_way_to_another() {
        let r = ruleset();
        let (mut w, city) = world(&r);
        let (silk, pearls) = (r.resources.id("Silk").unwrap(), r.resources.id("Pearls").unwrap());
        let far2 = w.grid.id_at(14, 14).unwrap();
        w.grid.resource[far2.0 as usize] = Some(pearls);
        w.cities.get_mut(city).unwrap().demanded_resource = Some(silk);
        w.cities.get_mut(city).unwrap().demand_countdown = 3;
        run(&mut w, &r, 3);
        assert_eq!(w.cities.get(city).unwrap().demanded_resource, Some(pearls), "it asks for what it did not ask last");
    }

    #[test]
    fn the_conditional_is_true_only_while_celebrating() {
        use crate::world::conditionals::{applies, GameContext, Verdict};
        let r = ruleset();
        let (mut w, city) = world(&r);
        let u = crate::unique::Unique::parse("[+1 Food] [in this city] <during We Love The King Day>");
        let during = |w: &World| applies(&u, &GameContext::new(&r).with_world(w).for_civ(CivId(0)).in_city(city));
        assert_eq!(during(&w), Verdict::False);
        w.cities.get_mut(city).unwrap().wltkd = 3;
        assert_eq!(during(&w), Verdict::True);
    }
}
