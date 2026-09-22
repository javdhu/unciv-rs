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
    }
    city.owner = conqueror;
    city.turn_acquired = turn;
    city.health = crate::world::city::max_health(r, city) / 2;
    city.avoid_growth = false;
    // Two to four citizens lose one, five to eight lose two, and so on.
    let population_lost = if city.population > 1 { 1 + city.population / 4 } else { 0 };
    city.population -= population_lost;
    city.worked.clear();
    city.specialists.clear();
    // What was banked toward the old owner's plans goes with them. Halving the
    // population while keeping the food banked for the larger city would let a
    // captured city grow back on the turn after it falls.
    city.production_stored = 0;
    city.food_stored = 0;
    // Retaking your own city ends its resistance; taking anyone else's starts it.
    city.resistance = if city.founder == conqueror { 0 } else { city.population };
    // A puppet first, whatever comes after — and a puppet's governor picks its
    // own work, so the old queue goes.
    city.puppet = true;
    city.razing = false;
    city.queue.clear();
    crate::world::city::rebuild_city_uniques(r, city);

    if let Some(old) = world.civ_mut(old_owner) {
        old.cities.retain(|c| *c != city_id);
    }
    if let Some(new) = world.civ_mut(conqueror) {
        new.cities.push(city_id);
    }
    // Its whole territory changes hands with it — every tile it claimed, not
    // a ring around the centre.
    for t in world.cities.get(city_id).map(|c| c.tiles.clone()).unwrap_or_default() {
        world.grid.owner[t.0 as usize] = Some(conqueror);
    }
    // A civ that takes back its own first city takes it back whole.
    if world.cities.get(city_id).is_some_and(|c| c.original_capital && c.founder == conqueror) {
        annex(world, city_id);
    }
    Some(Conquest { plundered, buildings_lost, population_lost })
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
    for t in &city.tiles {
        world.grid.owner[t.0 as usize] = None;
    }
    if let Some(c) = world.civ_mut(city.owner) {
        c.cities.retain(|id| *id != city_id);
    }
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
            destroy(world, city_id);
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
        Ruleset::load_dir("../android/assets/jsons/Civ V - Gods & Kings").expect("readable").expect("loads")
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
        w.cities.get_mut(city).unwrap().population = 3;
        crate::world::combat::capture_city(&mut w, &r, city, CivId(0));
        assert!(w.cities.get(city).unwrap().razing, "two citizens left: burn it");

        let (mut w, city) = world(&r);
        w.civ_mut(CivId(0)).unwrap().is_ai = true;
        crate::world::combat::capture_city(&mut w, &r, city, CivId(0));
        let c = w.cities.get(city).unwrap();
        assert!(!c.razing && c.puppet, "five left: keep it as a puppet");
    }

    #[test]
    fn barbarians_burn_what_they_take() {
        let r = ruleset();
        let (mut w, city) = world(&r);
        w.civ_mut(CivId(0)).unwrap().is_barbarian = true;
        crate::world::combat::capture_city(&mut w, &r, city, CivId(0));
        assert!(w.cities.get(city).is_none());
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
}
