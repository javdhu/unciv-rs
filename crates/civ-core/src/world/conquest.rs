//! What happens to a city that changes hands, and in the turns after.
//!
//! Taking a city (`CityConquestFunctions.conquerCity`) plunders its treasury,
//! burns some of its buildings, costs it a quarter of its people, and leaves it
//! in resistance — producing nothing — for as many turns as it has citizens.
//! It then becomes a *puppet*, governed for gold by its own governor, which is
//! what Unciv makes of every conquest before anything else is decided.
//!
//! From a puppet the conqueror may *annex* it, taking the governing back at the
//! price of unhappiness until a Courthouse stands, or *raze* it, burning a
//! citizen a turn until it is gone. The AI decides straight away, as Unciv's
//! does (`Battle.automateCityConquer`); a human player decides with [`annex`]
//! and [`raze`].
//!
//! The random parts — the plunder's spread and which buildings burn — are
//! seeded from the city's tile and the turn, like every roll in the engine.
//! Unciv seeds from the tile alone through a JVM `Random`, so the distribution
//! matches and the individual rolls do not.

use crate::arena::Id;
use crate::rng::Rng;
use crate::ruleset::Ruleset;
use crate::stats::{Fixed, Precise};
use crate::unique_type::UniqueType;
use crate::world::city::City;
use crate::world::grid::CivId;
use crate::world::world::World;

/// A regular building's chance, in percent, of burning when its city falls.
const BUILDING_BURN_CHANCE: i64 = 34;

/// What a conquest took.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Conquest {
    pub plundered: i32,
    pub buildings_lost: Vec<Id<crate::ruleset::Building>>,
    pub population_lost: u32,
}

/// Gold for taking a city (`getGoldForCapturingCity`).
///
/// `20 + 10 × population + 0..39`, scaled by how long the loser has held it —
/// nothing for a city founded this turn, in full after fifty — and by any
/// plunder uniques on either side.
fn plunder(world: &World, city: &City, conqueror: CivId, rng: &mut Rng) -> i32 {
    let base = 20 + 10 * city.population as i64 + rng.range(0, 39);
    let held = (world.turn as i64 - city.turn_acquired as i64).clamp(0, 50);
    let mut gold = Precise::from_ratio(base * held, 50);
    let pct = |p: &str| Fixed::parse(p).map(|v| Precise::from_ratio(100 + v.round(), 100));
    for u in city.uniques.matching(UniqueType::GoldFromCapturingCity) {
        if let Some(f) = u.params.first().and_then(|p| pct(p)) {
            gold = gold * f;
        }
    }
    if let Some(c) = world.civ(conqueror) {
        for u in c.uniques.matching(UniqueType::GoldFromEncampmentsAndCities) {
            if let Some(f) = u.params.first().and_then(|p| pct(p)) {
                gold = gold * f;
            }
        }
    }
    gold.floor() as i32
}

/// Which buildings burn (`destroyBuildingsOnCapture`): wonders never, the
/// palace and anything that moves to a new capital not here, anything marked
/// to always go always, and a third of the rest.
fn burn_buildings(r: &Ruleset, city: &City, rng: &mut Rng) -> Vec<Id<crate::ruleset::Building>> {
    let mut lost = Vec::new();
    for id in &city.built {
        let Some(b) = r.buildings.get(*id) else { continue };
        let has = |t: UniqueType| b.uniques.iter().any(|u| u.unique_type() == Some(t));
        if has(UniqueType::NotDestroyedWhenCityCaptured)
            || b.is_wonder == Some(true)
            || has(UniqueType::IndicatesCapital)
            || has(UniqueType::MovesToNewCapital)
        {
            continue;
        }
        if has(UniqueType::DestroyedWhenCityCaptured) || rng.range(0, 99) < BUILDING_BURN_CHANCE {
            lost.push(*id);
        }
    }
    lost
}

/// Take a city: plunder it, move it to the conqueror, and leave it a
/// resisting puppet.
pub fn conquer(world: &mut World, r: &Ruleset, city_id: Id<City>, conqueror: CivId) -> Option<Conquest> {
    let city = world.cities.get(city_id)?;
    let old_owner = city.owner;
    let centre = city.centre;
    let population = city.population;
    // Capture yields use the city's output before resistance and population
    // loss, and credit the conquering civilization once.
    let income =
        crate::world::city::city_yield_in(world, r, city, crate::world::golden_age::is_golden(world, old_owner));
    let ctx = crate::world::conditionals::GameContext::new(r)
        .with_world(world)
        .for_civ(conqueror)
        .in_city(city_id)
        .on_tile(centre);
    let mut capture = crate::stats::Stats::ZERO;
    if let Some(c) = world.civ(conqueror) {
        for unique in c
            .uniques
            .matching(UniqueType::CaptureCityPlunder)
            .filter(|u| crate::world::conditionals::applies(u, &ctx).holds())
        {
            if let (Some(times), Some(source), Some(target)) = (
                unique.params.first().and_then(|p| p.parse::<i32>().ok()),
                unique.params.get(1).and_then(|p| crate::stats::Stat::parse(p)),
                unique.params.get(2).and_then(|p| crate::stats::Stat::parse(p)),
            ) {
                capture[target] += crate::stats::Fixed::whole(times.saturating_mul(income[source].floor() as i32));
            }
        }
    }
    if let Some(c) = world.civ_mut(conqueror) {
        c.gold += capture.gold.floor() as i32;
        c.add_culture(capture.culture.floor() as i32);
        c.add_faith(capture.faith.floor() as i32);
        c.science_stored += capture.science.floor() as i32;
    }

    let aircraft: Vec<_> = world
        .units
        .iter()
        .filter(|(_, u)| u.position == centre && u.owner != conqueror && u.is_air(r))
        .map(|(id, _)| id)
        .collect();
    for unit in aircraft {
        world.remove_unit(unit);
    }
    // The loser's anger is weighed before the conquest takes its toll.
    let loser_population: u32 = world
        .civ(old_owner)
        .map(|c| c.cities.iter().filter_map(|id| world.cities.get(*id)).map(|c| c.population).sum())
        .unwrap_or(0);
    crate::world::opinion::on_city_captured(world, conqueror, old_owner, population, loser_population);
    let city = world.cities.get(city_id)?;
    let mut rng = Rng::from_parts(&[0xC0_4E57, city.centre.0 as u64, world.turn as u64]);
    let plundered = plunder(world, city, conqueror, &mut rng);
    let buildings_lost = burn_buildings(r, city, &mut rng);

    if let Some(c) = world.civ_mut(conqueror) {
        c.gold += plundered;
    }
    let turn = world.turn;
    let city = world.cities.get_mut(city_id)?;
    for id in &buildings_lost {
        city.built.remove(id);
        city.free_buildings.remove(id);
    }
    city.turn_acquired = turn;
    city.health = crate::world::city::max_health(r, city) / 2;
    city.avoid_growth = false;
    // Two to four citizens lose one, five to eight lose two, and so on.
    let population_lost = if city.population > 1 { 1 + city.population / 4 } else { 0 };
    city.population -= population_lost;
    city.worked.clear();
    city.specialists.clear();
    // Construction investment and overflow belong to the city. Its new
    // governor replaces the queue, while paused work remains available.
    // Food is cleared so the reduced population cannot immediately grow back.
    city.food_stored = 0;
    // Retaking your own city ends its resistance; taking anyone else's starts it.
    city.resistance = if city.founder == conqueror { 0 } else { city.population };
    // A puppet first, whatever comes after — and a puppet's governor picks its
    // own work, so the old queue goes.
    city.puppet = true;
    city.razing = false;
    city.queue.clear();
    city.production_conversion = None;
    crate::world::city::rebuild_city_uniques(r, city);

    transfer_city(world, r, city_id, conqueror)?;
    // A civ that takes back its own first city takes it back whole.
    if world.cities.get(city_id).is_some_and(|c| c.original_capital && c.founder == conqueror) {
        annex(world, city_id);
    }
    Some(Conquest { plundered, buildings_lost, population_lost })
}

/// Ownership transfer is separate from conquest damage. Capitals relocate,
/// former-owner free buildings leave, and national replacements follow the
/// receiving civilization for both conquest and liberation.
pub fn transfer_city(world: &mut World, r: &Ruleset, city: Id<City>, to: CivId) -> Option<()> {
    let source = world.cities.get(city)?;
    let from = source.owner;
    if from == to || world.civ(to).is_none() {
        return None;
    }
    let original = source.original_capital;
    let moving: Vec<_> = source
        .built
        .iter()
        .copied()
        .filter(|b| {
            r.buildings[*b]
                .uniques
                .iter()
                .any(|u| matches!(u.unique_type(), Some(UniqueType::IndicatesCapital | UniqueType::MovesToNewCapital)))
        })
        .collect();
    let next = world
        .cities
        .iter()
        .filter(|(id, c)| *id != city && c.owner == from)
        .max_by_key(|(id, c)| (c.population, std::cmp::Reverse(*id)))
        .map(|(id, _)| id);
    {
        let c = world.cities.get_mut(city)?;
        c.built.retain(|b| !moving.contains(b) && !c.free_buildings.contains(b));
        c.free_buildings.clear();
        c.previous_owner = Some(from);
        c.owner = to;
        c.faith.retain_owner_pantheon(to, c.population);
        c.turn_acquired = world.turn;
        c.razing = false;
        c.wltkd = 0;
    }
    if let Some(id) = next {
        let c = world.cities.get_mut(id)?;
        c.built.extend(moving);
        crate::world::city::rebuild_city_uniques(r, c);
    }
    world.civ_mut(from)?.cities.retain(|id| *id != city);
    let target = world.civ_mut(to)?;
    target.cities.push(city);
    target.has_ever_owned_original_capital |= original;
    if target.cities.len() == 1 {
        if let Some(palace) = r
            .buildings
            .iter()
            .find(|(_, b)| b.uniques.iter().any(|u| u.unique_type() == Some(UniqueType::IndicatesCapital)))
            .map(|(id, _)| id)
        {
            world.cities.get_mut(city)?.built.insert(palace);
        }
    }
    let converted: Vec<_> = world
        .cities
        .get(city)?
        .built
        .iter()
        .map(|id| {
            let base = r.buildings[*id].replaces.as_deref().and_then(|name| r.buildings.id(name)).unwrap_or(*id);
            crate::world::effects::equivalent_building(world, r, to, base)
        })
        .collect();
    let c = world.cities.get_mut(city)?;
    c.built = converted.into_iter().collect();
    crate::world::city::rebuild_city_uniques(r, c);
    for tile in c.tiles.clone() {
        world.grid.owner[tile.0 as usize] = Some(to);
    }
    for civ in &mut world.civs {
        for spy in &mut civ.espionage.spies {
            if spy.station == Some(city) {
                spy.station = None;
                spy.action = crate::world::espionage::SpyAction::Hideout;
                spy.turns_remaining = 0;
                spy.progress = 0;
            }
        }
    }
    for civ in [from, to] {
        crate::world::policies::rebuild_uniques(world, r, civ);
        crate::world::effects::refresh_free_buildings(world, r, civ);
    }
    Some(())
}

/// Liberation is a decision in the same action window as a conquest. A
/// voluntary later transfer must not repeatedly award liberation influence.
pub fn can_liberate(world: &World, city: Id<City>) -> bool {
    world.cities.get(city).is_some_and(|c| {
        c.puppet
            && c.turn_acquired == world.turn
            && c.founder != c.owner
            && c.previous_owner.is_some_and(|previous| previous != c.founder)
            && world.civ(c.founder).is_some_and(|founder| !founder.is_barbarian && !founder.city_state.absorbed)
    })
}

pub fn liberate(world: &mut World, r: &Ruleset, city: Id<City>) -> Option<()> {
    use crate::world::diplomacy::Sentiment;
    if !can_liberate(world, city) {
        return None;
    }
    let c = world.cities.get(city)?;
    let liberator = c.owner;
    let founder = c.founder;
    let former = c.previous_owner?;
    let population = c.population;
    let centre = c.centre;
    let other_population: u32 =
        world.cities.iter().filter(|(_, c)| c.owner == founder).map(|(_, c)| c.population).sum();
    let respect =
        10 + Precise::from_ratio(i64::from(population) * 100, i64::from(population + other_population)).round();
    let was_dead = !world.civ(founder)?.is_alive();
    if was_dead {
        world.diplomacy.revive(founder);
    }
    world.civ_mut(founder)?.defeated = false;
    world.diplomacy.meet(founder, liberator);
    if world.civ(founder)?.is_city_state() {
        let strongest = world
            .civs
            .iter()
            .filter(|c| c.is_major() && c.is_alive())
            .map(|c| world.diplomacy.influence(founder, c.id))
            .max()
            .unwrap_or(Fixed::whole(-60));
        world.diplomacy.set_influence(founder, liberator, (strongest + Fixed::whole(105)).max(Fixed::whole(60)));
        world.diplomacy.make_peace(founder, liberator);
    } else {
        world.diplomacy.attitude_mut(founder, liberator).add(
            Sentiment::CapturedOurCities,
            Fixed::whole(respect as i32),
            None,
        );
        world.diplomacy.set_open_borders(founder, liberator, true);
    }
    let witnesses: Vec<_> = world
        .civs
        .iter()
        .filter(|c| c.is_major() && c.id != liberator && c.id != former && world.diplomacy.have_met(c.id, liberator))
        .map(|c| c.id)
        .collect();
    for witness in witnesses {
        world.diplomacy.attitude_mut(witness, liberator).add(
            Sentiment::LiberatedCity,
            Fixed::whole(Precise::from_ratio(respect, 10).round() as i32),
            None,
        );
    }
    transfer_city(world, r, city, founder)?;
    let c = world.cities.get_mut(city)?;
    c.resistance = 0;
    c.puppet = false;
    let displaced: Vec<_> = world
        .units
        .iter()
        .filter(|(_, u)| u.transport.is_none() && u.position == centre && u.owner != founder)
        .map(|(id, _)| id)
        .collect();
    for id in displaced {
        let old = world.units.get(id)?.clone();
        if let Some(placement) =
            crate::world::construction::prepare_unit_near(world, r, old.owner, old.base, centre, None)
        {
            let u = world.units.get_mut(id)?;
            u.position = placement.position;
            u.embarked = placement.embarked;
            u.transport = placement.transport;
            u.set_up = false;
            crate::world::air::move_payload(world, id, placement.position);
        } else {
            world.remove_unit(id);
        }
    }
    let visitors: Vec<_> = world.civs.iter().filter(|c| c.id != founder).map(|c| c.id).collect();
    for visitor in visitors {
        crate::world::movement::expel(world, r, visitor, founder);
    }
    Some(())
}

/// Take over a puppet's governing (`annexCity`). Its people resent it until
/// a Courthouse stands; see [`annexed_unhappiness`].
pub fn annex(world: &mut World, city_id: Id<City>) {
    if let Some(c) = world.cities.get_mut(city_id) {
        c.puppet = false;
        c.avoid_growth = false;
    }
}

/// Why a city cannot be razed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RazeRejection {
    NoSuchCity,
    /// Its founder's first city — never, without a rule saying otherwise.
    OriginalCapital,
    /// The city a religion was founded in.
    HolyCity,
}

impl std::fmt::Display for RazeRejection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            RazeRejection::NoSuchCity => "no such city",
            RazeRejection::OriginalCapital => "an original capital cannot be razed",
            RazeRejection::HolyCity => "a holy city cannot be razed",
        })
    }
}

impl std::error::Error for RazeRejection {}

/// Whether a city may be razed at all (`canBeDestroyed`).
pub fn can_raze(world: &World, city_id: Id<City>) -> Result<(), RazeRejection> {
    let c = world.cities.get(city_id).ok_or(RazeRejection::NoSuchCity)?;
    if c.original_capital {
        return Err(RazeRejection::OriginalCapital);
    }
    if c.faith.holy_city_of.is_some() {
        return Err(RazeRejection::HolyCity);
    }
    Ok(())
}

/// Start burning a city. It is annexed first — a governor does not burn his
/// own city — and loses a citizen a turn from the next turn on.
pub fn raze(world: &mut World, city_id: Id<City>) -> Result<(), RazeRejection> {
    can_raze(world, city_id)?;
    annex(world, city_id);
    if let Some(c) = world.cities.get_mut(city_id) {
        c.razing = true;
    }
    Ok(())
}

/// Whether an annexed city's people still resent their new rulers
/// (`hasExtraAnnexUnhappiness`): held by someone other than its founder, not a
/// puppet, and without a Courthouse.
pub fn annexed_unhappiness(city: &City) -> bool {
    city.owner != city.founder && !city.puppet && !city.uniques.has(UniqueType::RemovesAnnexUnhappiness)
}

/// Remove a city from the world: its territory goes back to no one.
pub fn destroy(world: &mut World, city_id: Id<City>) {
    let Some(city) = world.cities.remove(city_id) else { return };
    for civ in &mut world.civs {
        for spy in &mut civ.espionage.spies {
            if spy.station == Some(city_id) {
                spy.station = None;
                spy.action = crate::world::espionage::SpyAction::Hideout;
                spy.turns_remaining = 0;
                spy.progress = 0;
            }
        }
    }
    for t in &city.tiles {
        world.grid.owner[t.0 as usize] = None;
    }
    if let Some(c) = world.civ_mut(city.owner) {
        c.cities.retain(|id| *id != city_id);
    }
}

/// Live-game destruction also removes aircraft stationed in the lost base.
pub fn destroy_in(world: &mut World, r: &Ruleset, city_id: Id<City>) {
    if let Some(centre) = world.cities.get(city_id).map(|c| c.centre) {
        let aircraft: Vec<_> =
            world.units.iter().filter(|(_, u)| u.position == centre && u.is_air(r)).map(|(id, _)| id).collect();
        for unit in aircraft {
            world.remove_unit(unit);
        }
    }
    destroy(world, city_id);
}

/// What a turn did to the cities a civ took.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConquestEvent {
    ResistanceEnded { city: Id<City> },
    Razed { city: Id<City>, owner: CivId },
    Burned { city: Id<City>, population: u32 },
}

/// One turn for a civ's conquered cities: resistance runs down, and razing
/// burns (`CityTurnManager.endTurn`).
pub fn tick(world: &mut World, r: &Ruleset, civ: CivId) -> Vec<ConquestEvent> {
    let mut events = Vec::new();
    let faster: u32 = world.civ(civ).map_or(0, |c| {
        c.uniques
            .matching(UniqueType::CitiesAreRazedXTimesFaster)
            .filter_map(|u| u.params.first().and_then(|p| p.parse::<u32>().ok()))
            .map(|x| x.saturating_sub(1))
            .sum()
    });
    let cities: Vec<Id<City>> = world.civ(civ).map(|c| c.cities.clone()).unwrap_or_default();
    let burned = 1 + faster;
    for city_id in cities {
        let Some((razing, population)) = world.cities.get(city_id).map(|c| (c.razing, c.population)) else {
            continue;
        };
        if razing && population <= burned {
            events.push(ConquestEvent::Razed { city: city_id, owner: civ });
            destroy_in(world, r, city_id);
            continue;
        }
        let Some(c) = world.cities.get_mut(city_id) else { continue };
        if razing {
            c.population -= burned;
            // The granary cannot hold more than the smaller city could use.
            let cap = crate::world::city::food_to_next_population(c.population, Default::default()) - 1;
            c.food_stored = c.food_stored.min(cap.max(0));
            events.push(ConquestEvent::Burned { city: city_id, population: c.population });
        }
        if c.resistance > 0 {
            c.resistance -= 1;
            if c.resistance == 0 {
                events.push(ConquestEvent::ResistanceEnded { city: city_id });
            }
        }
    }
    let _ = r;
    events
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::entities::Civilization;
    use crate::world::grid::TileGrid;

    fn ruleset() -> Ruleset {
        Ruleset::load_dir("rules/gods-and-kings").expect("readable").expect("loads")
    }

    /// Two civs; civ 1 holds a city of eight at (8, 8), founded as its second
    /// city, with its founding ring, fifty turns ago.
    fn world(r: &Ruleset) -> (World, Id<City>) {
        let g = TileGrid::new(16, 16, r.terrain("Grassland").unwrap());
        let nation = r.nations.ids().next().unwrap();
        let mut w =
            World::new(g, vec![Civilization::new(CivId(0), nation, false), Civilization::new(CivId(1), nation, false)]);
        w.found_city_in(r, City::found("Capital", CivId(1), w.grid.id_at(2, 2).unwrap()));
        let centre = w.grid.id_at(8, 8).unwrap();
        let city = w.found_city_in(r, City::found("Prize", CivId(1), centre));
        for t in w.grid.within(centre, 1) {
            w.claim_tile(city, t);
        }
        w.cities.get_mut(city).unwrap().population = 8;
        w.turn = 50;
        (w, city)
    }

    #[test]
    fn a_fallen_city_loses_a_quarter_and_resists_for_what_is_left() {
        let r = ruleset();
        let (mut w, city) = world(&r);
        let taken = conquer(&mut w, &r, city, CivId(0)).unwrap();
        let c = w.cities.get(city).unwrap();
        assert_eq!(taken.population_lost, 3, "eight loses one plus a quarter");
        assert_eq!(c.population, 5);
        assert_eq!(c.resistance, 5);
        assert!(c.puppet, "every conquest starts as a puppet");
        assert_eq!(c.owner, CivId(0));
        assert!(w.civ(CivId(0)).unwrap().cities.contains(&city));
        assert!(!w.civ(CivId(1)).unwrap().cities.contains(&city));
    }

    #[test]
    fn plunder_is_the_formula_and_lands_in_the_treasury() {
        let r = ruleset();
        let (mut w, city) = world(&r);
        let before = w.civ(CivId(0)).unwrap().gold;
        let taken = conquer(&mut w, &r, city, CivId(0)).unwrap();
        // Held fifty turns, so in full: 20 + 80 + 0..39.
        assert!((100..=139).contains(&taken.plundered), "{}", taken.plundered);
        assert_eq!(w.civ(CivId(0)).unwrap().gold, before + taken.plundered);
    }

    #[test]
    fn a_city_founded_this_turn_has_nothing_to_plunder() {
        let r = ruleset();
        let (mut w, city) = world(&r);
        w.cities.get_mut(city).unwrap().turn_acquired = 50;
        assert_eq!(conquer(&mut w, &r, city, CivId(0)).unwrap().plundered, 0);
    }

    #[test]
    fn wonders_and_the_palace_never_burn() {
        let r = ruleset();
        let (mut w, _) = world(&r);
        let capital = w.civ(CivId(1)).unwrap().cities[0];
        let pyramids = r.buildings.id("The Pyramids").unwrap();
        let palace = crate::world::connections::palace(&r).unwrap();
        let ordinary: Vec<_> = ["Monument", "Granary", "Library", "Barracks", "Walls", "Market"]
            .iter()
            .map(|n| r.buildings.id(n).unwrap())
            .collect();
        {
            let c = w.cities.get_mut(capital).unwrap();
            c.built.insert(pyramids);
            c.built.extend(ordinary.iter().copied());
        }
        let taken = conquer(&mut w, &r, capital, CivId(0)).unwrap();
        let c = w.cities.get(capital).unwrap();
        assert!(c.built.contains(&pyramids) && c.built.contains(&palace));
        assert!(taken.buildings_lost.iter().all(|b| ordinary.contains(b)));
        println!("burned {} of {}", taken.buildings_lost.len(), ordinary.len());
    }

    #[test]
    fn retaking_your_own_capital_takes_it_back_whole() {
        let r = ruleset();
        let (mut w, _) = world(&r);
        let capital = w.civ(CivId(1)).unwrap().cities[0];
        conquer(&mut w, &r, capital, CivId(0));
        conquer(&mut w, &r, capital, CivId(1));
        let c = w.cities.get(capital).unwrap();
        assert!(!c.puppet, "annexed");
        assert_eq!(c.resistance, 0, "and no resistance at home");
    }

    #[test]
    fn an_annexed_city_resents_its_rulers_until_a_courthouse() {
        let r = ruleset();
        let (mut w, city) = world(&r);
        let calm = crate::world::happiness::happiness(&w, &r, CivId(0), None);
        conquer(&mut w, &r, city, CivId(0));
        let puppet = crate::world::happiness::happiness(&w, &r, CivId(0), None);
        annex(&mut w, city);
        let annexed = crate::world::happiness::happiness(&w, &r, CivId(0), None);
        // Five citizens: as a puppet -3 and -5; annexed, -5, -10 and -2.
        assert_eq!(calm - puppet, 3 + 5);
        assert_eq!(calm - annexed, 5 + 10 + 2);

        let courthouse = r.buildings.id("Courthouse").unwrap();
        let c = w.cities.get_mut(city).unwrap();
        c.built.insert(courthouse);
        crate::world::city::rebuild_city_uniques(&r, c);
        assert!(!annexed_unhappiness(c));
    }

    #[test]
    fn razing_burns_a_citizen_a_turn_and_then_the_city() {
        let r = ruleset();
        let (mut w, city) = world(&r);
        conquer(&mut w, &r, city, CivId(0));
        raze(&mut w, city).expect("not a capital");
        let owned: Vec<_> = w.cities.get(city).unwrap().tiles.clone();

        let mut turns = 0;
        while w.cities.get(city).is_some() {
            tick(&mut w, &r, CivId(0));
            turns += 1;
            assert!(turns < 10);
        }
        assert_eq!(turns, 5, "five citizens, five turns");
        assert!(owned.iter().all(|t| w.grid.owner[t.0 as usize].is_none()), "the land is no one's");
        assert!(!w.civ(CivId(0)).unwrap().cities.contains(&city));
    }

    #[test]
    fn an_original_capital_cannot_be_razed() {
        let r = ruleset();
        let (mut w, _) = world(&r);
        let capital = w.civ(CivId(1)).unwrap().cities[0];
        conquer(&mut w, &r, capital, CivId(0));
        assert_eq!(raze(&mut w, capital), Err(RazeRejection::OriginalCapital));
    }

    #[test]
    fn resistance_runs_down_and_says_so() {
        let r = ruleset();
        let (mut w, city) = world(&r);
        conquer(&mut w, &r, city, CivId(0));
        let mut ended = None;
        for turn in 1..=10 {
            if tick(&mut w, &r, CivId(0)).contains(&ConquestEvent::ResistanceEnded { city }) {
                ended = Some(turn);
            }
        }
        assert_eq!(ended, Some(5));
    }

    #[test]
    fn a_resisting_city_makes_one_hammer_and_nothing_else() {
        use crate::world::assignment::CityFocus;
        use crate::world::turn::{plan_civ_turn, TurnModifiers};
        let r = ruleset();
        let (mut w, city) = world(&r);
        conquer(&mut w, &r, city, CivId(0));
        let plan = plan_civ_turn(&w, &r, CivId(0), CityFocus::None, TurnModifiers::default());
        let of = |v: &[(Id<City>, i32)]| v.iter().find(|(c, _)| *c == city).map(|(_, n)| *n);
        assert_eq!(of(&plan.production), Some(1));
        assert_eq!(of(&plan.food), Some(0), "no growth, and no famine either");
        assert_eq!(of(&plan.border_culture), Some(0));
        assert!(
            crate::world::filters::city_matches(&w, &r, w.cities.get(city).unwrap(), None, "in resisting cities")
                == crate::world::conditionals::Verdict::True
        );
    }

    #[test]
    fn a_small_conquest_is_razed_by_the_ai_and_a_large_one_kept() {
        let r = ruleset();
        let (mut w, city) = world(&r);
        w.civ_mut(CivId(0)).unwrap().is_ai = true;
        // Unciv's rule, which the old AI keeps. The new one keeps every city
        // while it plays for conquest or score (`ai::should_raze`).
        w.civ_mut(CivId(0)).unwrap().legacy_ai = true;
        w.cities.get_mut(city).unwrap().population = 3;
        let mut kept = w.clone();
        crate::world::combat::capture_city(&mut w, &r, city, CivId(0));
        assert!(w.cities.get(city).unwrap().razing, "two citizens left: burn it");
        kept.civ_mut(CivId(0)).unwrap().legacy_ai = false;
        if matches!(
            crate::world::goals::goal(&kept, &r, CivId(0)),
            crate::world::goals::Goal::Conquest | crate::world::goals::Goal::Score
        ) {
            crate::world::combat::capture_city(&mut kept, &r, city, CivId(0));
            assert!(!kept.cities.get(city).unwrap().razing, "a conqueror keeps what it takes");
        }

        let (mut w, city) = world(&r);
        w.civ_mut(CivId(0)).unwrap().is_ai = true;
        crate::world::combat::capture_city(&mut w, &r, city, CivId(0));
        let c = w.cities.get(city).unwrap();
        assert!(!c.razing && c.puppet, "five left: keep it as a puppet");
    }

    #[test]
    fn barbarians_cannot_bypass_raid_rules_through_the_conquest_helper() {
        let r = ruleset();
        let (mut w, city) = world(&r);
        w.civ_mut(CivId(0)).unwrap().is_barbarian = true;
        let before = w.clone();
        crate::world::combat::capture_city(&mut w, &r, city, CivId(0));
        assert_eq!(w, before, "barbarian attacks resolve through the raid handler");
    }

    #[test]
    fn a_puppet_neglects_science_and_builds_no_wonders() {
        let r = ruleset();
        let (mut w, city) = world(&r);
        let library = r.buildings.id("Library").unwrap();
        w.cities.get_mut(city).unwrap().built.insert(library);
        conquer(&mut w, &r, city, CivId(0));
        let c = w.cities.get(city).unwrap();
        let science = crate::world::city::city_percent_bonuses(&r, c);
        assert!(science.contains_key(&crate::stats::Stat::Science));
        let civ = w.civ(CivId(0)).unwrap();
        let pyramids = crate::world::construction::Construction::Building(r.buildings.id("The Pyramids").unwrap());
        assert_eq!(
            crate::world::construction::can_construct(&r, civ, c, pyramids),
            Err(crate::world::construction::Rejection::PuppetWonder)
        );
    }

    #[test]
    fn city_states_build_no_wonders() {
        let r = ruleset();
        let (mut w, city) = world(&r);
        let owner = w.cities.get(city).unwrap().owner;
        let pyramids = crate::world::construction::Construction::Building(r.buildings.id("The Pyramids").unwrap());
        w.civ_mut(owner).unwrap().city_state_type = r.city_state_types.ids().next();
        let (civ, c) = (w.civ(owner).unwrap(), w.cities.get(city).unwrap());
        assert_eq!(
            crate::world::construction::can_construct(&r, civ, c, pyramids),
            Err(crate::world::construction::Rejection::CityStateWonder)
        );
    }
}
