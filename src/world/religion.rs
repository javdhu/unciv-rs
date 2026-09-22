//! Faith, pantheons and religions.
//!
//! Faith accumulates like every other yield, but it buys two different things
//! at two different prices, and both prices move:
//!
//! * a **pantheon** costs `10 + 5 × (civs that already have one)`, so the
//!   longer you wait the dearer it is — the pressure is to commit early;
//! * a **great prophet**, who founds a religion, costs
//!   `200 + 100 × n(n+1)/2` where `n` is how many you have already. That is
//!   triangular growth: 200, 300, 500, 800, 1200. The first is cheap and the
//!   fourth is most of an era.
//!
//! Beliefs are the payload, and they are uniques — `[+1 Culture] from every
//! [Shrine]` — so adopting one is a matter of putting it into the civ's index
//! and letting the existing machinery apply it.

use crate::arena::Id;
use crate::ruleset::{Belief, Ruleset};
use crate::stats::Fixed;
use crate::world::conditionals::Verdict;
use crate::world::grid::CivId;
use crate::world::world::World;

/// A pantheon's price before other civs have one (`ModConstants.kt:100`).
pub const PANTHEON_BASE: i32 = 10;

/// Added per civ that already has a pantheon.
pub const PANTHEON_GROWTH: i32 = 5;

/// The first great prophet's price.
pub const PROPHET_BASE: i32 = 200;

/// Step in the triangular growth of later prophets.
pub const PROPHET_STEP: i32 = 100;

/// What a civ has done with its faith.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Faith {
    pub stored: i32,
    /// Ordered, so iteration and serialisation are reproducible.
    pub beliefs: std::collections::BTreeSet<Id<Belief>>,
    pub prophets_earned: i32,
    /// A religion exists once a prophet has founded one.
    pub founded: bool,
}

impl Faith {
    pub fn has_pantheon(&self) -> bool {
        !self.beliefs.is_empty()
    }
}

/// What a pantheon costs this civ right now.
///
/// Dearer the more civs already have one, so waiting is paid for.
pub fn pantheon_cost(world: &World, speed: Fixed) -> i32 {
    let taken = world.civs.iter().filter(|c| c.faith.has_pantheon()).count() as i32;
    (Fixed::whole(PANTHEON_BASE + taken * PANTHEON_GROWTH) * speed).round().max(1) as i32
}

/// What the next great prophet costs.
///
/// `200 + 100·n(n+1)/2` — triangular, so 200, 300, 500, 800, 1200.
pub fn prophet_cost(earned: i32, speed: Fixed) -> i32 {
    let n = earned.max(0);
    let triangular = PROPHET_STEP * n * (n + 1) / 2;
    (Fixed::whole(PROPHET_BASE + triangular) * speed).round().max(1) as i32
}

/// Beliefs of a kind that nobody has taken.
pub fn available_beliefs(world: &World, r: &Ruleset, kind: &str) -> Vec<Id<Belief>> {
    let taken: std::collections::BTreeSet<_> =
        world.civs.iter().flat_map(|c| c.faith.beliefs.iter().copied()).collect();
    r.beliefs.iter().filter(|(id, b)| b.r#type == kind && !taken.contains(id)).map(|(id, _)| id).collect()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FaithEvent {
    PantheonFounded { belief: Id<Belief> },
    ReligionFounded { beliefs: Vec<Id<Belief>> },
    Stored { faith: i32 },
}

/// Spend a turn's faith: a pantheon first, then a religion when one is due.
///
/// Returns what happened so the caller can record it; the civ's unique index
/// is rebuilt by the caller, since beliefs are uniques and have to reach it to
/// do anything.
pub fn apply_faith(world: &mut World, r: &Ruleset, civ_id: CivId, gained: i32, speed: Fixed) -> FaithEvent {
    if let Some(c) = world.civ_mut(civ_id) {
        c.faith.stored += gained.max(0);
    }
    let stored = world.civ(civ_id).map_or(0, |c| c.faith.stored);
    let has_pantheon = world.civ(civ_id).is_some_and(|c| c.faith.has_pantheon());

    // A pantheon comes first and is cheap.
    if !has_pantheon {
        let cost = pantheon_cost(world, speed);
        if stored >= cost {
            if let Some(belief) = available_beliefs(world, r, "Pantheon").into_iter().min() {
                if let Some(c) = world.civ_mut(civ_id) {
                    c.faith.stored -= cost;
                    c.faith.beliefs.insert(belief);
                }
                return FaithEvent::PantheonFounded { belief };
            }
        }
        return FaithEvent::Stored { faith: stored };
    }

    // Then a prophet, who founds a religion with a founder and a follower
    // belief. Later prophets would enhance it; that is not modelled yet.
    let founded = world.civ(civ_id).is_some_and(|c| c.faith.founded);
    if !founded {
        let earned = world.civ(civ_id).map_or(0, |c| c.faith.prophets_earned);
        let cost = prophet_cost(earned, speed);
        if stored >= cost {
            let mut chosen: Vec<Id<Belief>> = Vec::new();
            for kind in ["Founder", "Follower"] {
                if let Some(b) = available_beliefs(world, r, kind).into_iter().min() {
                    chosen.push(b);
                }
            }
            if !chosen.is_empty() {
                if let Some(c) = world.civ_mut(civ_id) {
                    c.faith.stored -= cost;
                    c.faith.prophets_earned += 1;
                    c.faith.founded = true;
                    c.faith.beliefs.extend(chosen.iter().copied());
                }
                return FaithEvent::ReligionFounded { beliefs: chosen };
            }
        }
    }
    FaithEvent::Stored { faith: stored }
}

// ---------------------------------------------------------------------------
// Pressure and spread
// ---------------------------------------------------------------------------

/// Which religion, identified by the civ that founded it.
///
/// `None` is atheism, and it is a real entry in the pressure table rather than
/// an absence. Unciv seeds every city with 100 atheist pressure for two
/// reasons: the sum is then never zero, so the follower split never divides by
/// it, and a city with no religion has something concrete to convert *from*.
pub type Creed = Option<CivId>;

/// Pressure a city applies to its neighbours, before the speed multiplier.
/// A holy city pushes five times as hard as an ordinary one.
pub const HOLY_CITY_MULTIPLIER: i32 = 5;

/// How far a religion reaches, in tiles, before uniques adjust it.
pub const BASE_SPREAD_RANGE: u32 = 10;

/// Pressure one new citizen brings to the city's existing majority.
pub const PRESSURE_PER_CITIZEN: i32 = 100;

/// The atheist pressure every city starts with.
pub const ATHEIST_PRESSURE: i32 = 100;

/// One city's religious state.
///
/// Pressure is the stored quantity and followers are derived from it, never
/// the other way round. Storing followers directly would make conversion a
/// matter of editing a number, and then two cities could disagree about how
/// many people a religion has.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CityFaith {
    /// Sparse: a religion that has never reached this city has no entry, and
    /// most cities in most games touch two or three religions at most.
    pressure: std::collections::BTreeMap<Creed, i32>,
    /// Derived from pressure by [`CityFaith::recount`]. Atheists are not
    /// listed — they are the remainder.
    followers: std::collections::BTreeMap<CivId, u32>,
    /// The city a religion was founded in pushes harder and cannot be
    /// converted away by ordinary pressure.
    pub holy_city_of: Option<CivId>,
}

impl Default for CityFaith {
    fn default() -> CityFaith {
        CityFaith {
            pressure: std::collections::BTreeMap::from([(None, ATHEIST_PRESSURE)]),
            followers: std::collections::BTreeMap::new(),
            holy_city_of: None,
        }
    }
}

impl CityFaith {
    pub fn new() -> CityFaith {
        CityFaith::default()
    }

    pub fn pressure_of(&self, creed: Creed) -> i32 {
        self.pressure.get(&creed).copied().unwrap_or(0)
    }

    pub fn total_pressure(&self) -> i32 {
        self.pressure.values().sum()
    }

    pub fn pressures(&self) -> impl Iterator<Item = (Creed, i32)> + '_ {
        self.pressure.iter().map(|(c, p)| (*c, *p))
    }

    pub fn followers_of(&self, civ: CivId) -> u32 {
        self.followers.get(&civ).copied().unwrap_or(0)
    }

    pub fn followers(&self) -> impl Iterator<Item = (CivId, u32)> + '_ {
        self.followers.iter().map(|(c, n)| (*c, *n))
    }

    /// Add pressure. The follower count is *not* refreshed — a turn of spread
    /// adds pressure from many cities at once, and recounting after each one
    /// would announce a conversion for every intermediate state.
    pub fn add_pressure(&mut self, creed: Creed, amount: i32) {
        *self.pressure.entry(creed).or_insert(0) += amount;
    }

    /// What an inquisitor does: everything but one religion is swept away.
    /// Atheism survives, because you cannot convert someone to nothing.
    pub fn remove_all_pressures_except(&mut self, keep: CivId) {
        let kept = self.pressure_of(Some(keep));
        let atheists = self.pressure_of(None);
        self.pressure.clear();
        self.pressure.insert(None, atheists.max(ATHEIST_PRESSURE));
        if kept > 0 {
            self.pressure.insert(Some(keep), kept);
        }
    }

    /// The religion most of the city follows, if any.
    ///
    /// Unciv's bar is `followers >= population / 2` with integer division, so
    /// in a city of five, two followers is a majority. That is lower than the
    /// word suggests and it is deliberate — it makes a city flip while the
    /// contest is still live rather than at a knife edge.
    pub fn majority(&self, population: u32) -> Option<CivId> {
        let (&civ, &count) = self.followers.iter().max_by_key(|(civ, n)| (**n, std::cmp::Reverse(**civ)))?;
        if count >= population / 2 && count > 0 {
            Some(civ)
        } else {
            None
        }
    }

    /// Recompute followers from pressure.
    ///
    /// Each religion gets its share of the population rounded down, and the
    /// citizens left over go one at a time to whoever was rounded down hardest
    /// — largest-remainder apportionment, the same rule used for seats in a
    /// parliament, and for the same reason: every citizen must belong to
    /// exactly one religion and the shares rarely divide evenly.
    ///
    /// Unciv breaks ties between equal remainders by hash order, which is not
    /// reproducible. This breaks them by civ id.
    pub fn recount(&mut self, population: u32) {
        self.followers.clear();
        if population == 0 {
            return;
        }
        let total: i32 = self.total_pressure();
        let per_follower = total / population as i32;
        if per_follower <= 0 {
            return; // less pressure than people; nobody has convinced anyone
        }

        let mut remainders: Vec<(Creed, i32)> = Vec::new();
        let mut allocated: u32 = 0;
        for (creed, pressure) in self.pressure.iter() {
            let share = (*pressure / per_follower).max(0) as u32;
            if let Some(civ) = creed {
                if share > 0 {
                    self.followers.insert(*civ, share);
                }
            }
            allocated += share;
            remainders.push((*creed, *pressure - share as i32 * per_follower));
        }

        // Largest remainder first, ties to the lower civ id, atheism last of
        // all so a citizen goes to a religion before going to nobody.
        remainders.sort_by_key(|(creed, rem)| (std::cmp::Reverse(*rem), creed.is_none(), creed.map(|c| c.0)));
        let mut unallocated = population.saturating_sub(allocated);
        for (creed, _) in &remainders {
            if unallocated == 0 {
                break;
            }
            if let Some(civ) = creed {
                *self.followers.entry(*civ).or_insert(0) += 1;
            }
            unallocated -= 1;
        }
        // Whatever is still unallocated is atheist, and atheists are not
        // listed — they are what is left.
    }

    /// Population grew or shrank. Growth converts the newcomers to whatever
    /// the city already believes; shrinking only redistributes.
    pub fn on_population_change(&mut self, change: i32, population: u32) {
        if change > 0 {
            let creed = self.majority(population.saturating_sub(change as u32));
            self.add_pressure(creed, PRESSURE_PER_CITIZEN * change);
        }
        self.recount(population);
    }
}

/// How far this city's religion reaches.
fn spread_range(world: &World, r: &Ruleset, city: &crate::world::city::City) -> u32 {
    let mut range = BASE_SPREAD_RANGE as i32;
    let extra = |index: &crate::world::unique_index::UniqueIndex| -> i32 {
        index
            .matching(crate::UniqueType::ReligionSpreadDistance)
            .filter_map(|u| u.params.first().and_then(|p| p.parse::<i32>().ok()))
            .sum()
    };
    range += extra(&city.uniques);
    // The religion's *founder* extends its reach everywhere it has spread,
    // not only at home — so the range depends on whose religion this city
    // follows, not on who owns the city.
    if let Some(founder) = city.faith.majority(city.population) {
        if let Some(c) = world.civ(founder) {
            range += extra(&c.uniques);
        }
    }
    let _ = r;
    range.max(0) as u32
}

/// How hard this city pushes its religion onto another.
fn pressure_toward(
    world: &World,
    r: &Ruleset,
    from: &crate::world::city::City,
    onto: &crate::world::city::City,
    base: i32,
) -> i32 {
    let mut pressure = Fixed::whole(base);
    let mut apply = |index: &crate::world::unique_index::UniqueIndex| {
        for u in index.matching(crate::UniqueType::NaturalReligionSpreadStrength) {
            let matches = u
                .params
                .get(1)
                .map(|f| crate::world::filters::city_matches(world, r, onto, Some(from), f) == Verdict::True)
                .unwrap_or(false);
            if !matches {
                continue;
            }
            if let Some(pct) = u.params.first().and_then(|p| Fixed::parse(p)) {
                if let Some(next) = (pressure * (Fixed::whole(100) + pct)).checked_div(Fixed::whole(100)) {
                    pressure = next;
                }
            }
        }
    };
    apply(&from.uniques);
    if let Some(founder) = from.faith.majority(from.population) {
        if let Some(c) = world.civ(founder) {
            apply(&c.uniques);
        }
    }
    pressure.floor().max(0) as i32
}

/// What changed when religions spread this turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Conversion {
    pub city: Id<crate::world::city::City>,
    pub from: Option<CivId>,
    pub to: CivId,
}

/// One turn of religious spread across the whole map.
///
/// Every city with a majority religion pushes it onto every city within reach,
/// including its neighbours' and its enemies' — a border does not stop a
/// missionary. Holy cities push five times as hard onto everyone, including
/// themselves, which is what keeps a religion's birthplace loyal.
///
/// Pressure from every source is gathered before any follower count is
/// touched. Recounting between sources would announce a conversion for each
/// intermediate state, and a city pushed by three religions at once would
/// report converting three times in one turn.
pub fn spread(world: &mut World, r: &Ruleset, pressure_per_city: i32) -> Vec<Conversion> {
    let before: std::collections::BTreeMap<_, _> =
        world.cities.iter().map(|(id, c)| (id, c.faith.majority(c.population))).collect();

    // Plan every push before applying any, so no city's contribution depends
    // on whether it was visited before or after another.
    let mut pushes: Vec<(Id<crate::world::city::City>, Creed, i32)> = Vec::new();
    let ids: Vec<_> = world.cities.iter().map(|(id, _)| id).collect();

    for &id in &ids {
        let Some(city) = world.cities.get(id) else { continue };
        if let Some(holy) = city.faith.holy_city_of {
            pushes.push((id, Some(holy), HOLY_CITY_MULTIPLIER * pressure_per_city));
        }
        let Some(majority) = city.faith.majority(city.population) else { continue };
        let range = spread_range(world, r, city);
        for &other in &ids {
            if other == id {
                continue;
            }
            let Some(target) = world.cities.get(other) else { continue };
            if world.grid.distance(city.centre, target.centre) > range {
                continue;
            }
            let amount = pressure_toward(world, r, city, target, pressure_per_city);
            if amount > 0 {
                pushes.push((other, Some(majority), amount));
            }
        }
    }

    for (id, creed, amount) in pushes {
        if let Some(c) = world.cities.get_mut(id) {
            c.faith.add_pressure(creed, amount);
        }
    }

    let mut converted = Vec::new();
    for &id in &ids {
        let Some(c) = world.cities.get_mut(id) else { continue };
        let population = c.population;
        c.faith.recount(population);
        let now = c.faith.majority(population);
        let was = before.get(&id).copied().flatten();
        if let Some(to) = now {
            if was != Some(to) {
                converted.push(Conversion { city: id, from: was, to });
            }
        }
    }
    converted
}

/// Every unique a civ's beliefs grant, for the index.
pub fn belief_uniques<'a>(r: &'a Ruleset, faith: &Faith) -> Vec<&'a crate::Unique> {
    faith.beliefs.iter().filter_map(|id| r.beliefs.get(*id)).flat_map(|b| b.uniques.iter()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::entities::Civilization;
    use crate::world::grid::TileGrid;

    fn ruleset() -> Ruleset {
        Ruleset::load_dir("../android/assets/jsons/Civ V - Gods & Kings").expect("readable").expect("loads")
    }

    fn world(r: &Ruleset) -> World {
        let g = TileGrid::new(8, 8, r.terrain("Grassland").unwrap());
        let nation = r.nations.ids().next().unwrap();
        World::new(g, vec![Civilization::new(CivId(0), nation, true), Civilization::new(CivId(1), nation, true)])
    }

    #[test]
    fn a_pantheon_gets_dearer_as_others_take_theirs() {
        let r = ruleset();
        let mut w = world(&r);
        assert_eq!(pantheon_cost(&w, Fixed::ONE), 10, "the first is cheap");

        let belief = available_beliefs(&w, &r, "Pantheon")[0];
        w.civ_mut(CivId(1)).unwrap().faith.beliefs.insert(belief);
        assert_eq!(pantheon_cost(&w, Fixed::ONE), 15, "and dearer once taken");
    }

    /// Triangular, not linear: the fourth prophet costs four times the first.
    #[test]
    fn prophets_get_much_dearer() {
        let costs: Vec<i32> = (0..5).map(|n| prophet_cost(n, Fixed::ONE)).collect();
        assert_eq!(costs, vec![200, 300, 500, 800, 1200]);
        assert_eq!(prophet_cost(0, Fixed::whole(3)), 600, "and speed scales it");
    }

    #[test]
    fn faith_buys_a_pantheon_then_a_religion() {
        let r = ruleset();
        let mut w = world(&r);

        assert!(matches!(apply_faith(&mut w, &r, CivId(0), 5, Fixed::ONE), FaithEvent::Stored { .. }));
        assert!(!w.civ(CivId(0)).unwrap().faith.has_pantheon());

        let e = apply_faith(&mut w, &r, CivId(0), 10, Fixed::ONE);
        assert!(matches!(e, FaithEvent::PantheonFounded { .. }), "{e:?}");
        assert!(w.civ(CivId(0)).unwrap().faith.has_pantheon());

        // A religion needs far more.
        let e = apply_faith(&mut w, &r, CivId(0), 250, Fixed::ONE);
        assert!(matches!(e, FaithEvent::ReligionFounded { .. }), "{e:?}");
        let faith = &w.civ(CivId(0)).unwrap().faith;
        assert!(faith.founded);
        assert_eq!(faith.prophets_earned, 1);
        assert!(faith.beliefs.len() >= 3, "a pantheon, a founder and a follower");
    }

    #[test]
    fn two_civs_cannot_hold_the_same_belief() {
        let r = ruleset();
        let mut w = world(&r);
        apply_faith(&mut w, &r, CivId(0), 100, Fixed::ONE);
        let theirs: Vec<_> = w.civ(CivId(0)).unwrap().faith.beliefs.iter().copied().collect();
        assert!(!theirs.is_empty());

        let left = available_beliefs(&w, &r, "Pantheon");
        for taken in &theirs {
            assert!(!left.contains(taken), "a taken belief is off the table");
        }
    }

    #[test]
    fn beliefs_carry_uniques_for_the_index() {
        let r = ruleset();
        let mut w = world(&r);
        apply_faith(&mut w, &r, CivId(0), 100, Fixed::ONE);
        let faith = &w.civ(CivId(0)).unwrap().faith;
        let uniques = belief_uniques(&r, faith);
        println!("{} uniques from {} beliefs", uniques.len(), faith.beliefs.len());
        assert!(!uniques.is_empty(), "a belief that grants nothing is decoration");
    }

    // --- pressure and spread ---

    fn city_at(
        w: &mut World,
        r: &Ruleset,
        name: &str,
        owner: CivId,
        col: u16,
        row: u16,
        pop: u32,
    ) -> Id<crate::world::city::City> {
        let t = w.grid.id_at(col, row).unwrap();
        let id = w.found_city(crate::world::city::City::found(name, owner, t));
        let c = w.cities.get_mut(id).unwrap();
        c.population = pop;
        c.faith.recount(pop);
        let _ = r;
        id
    }

    /// Make a city wholly devout, the way founding a religion there does.
    fn convert(w: &mut World, city: Id<crate::world::city::City>, to: CivId) {
        let c = w.cities.get_mut(city).unwrap();
        let pop = c.population;
        c.faith.add_pressure(Some(to), PRESSURE_PER_CITIZEN * pop as i32 * 10);
        c.faith.recount(pop);
    }

    #[test]
    fn a_new_city_believes_nothing_but_is_not_empty() {
        let f = CityFaith::new();
        assert_eq!(f.total_pressure(), ATHEIST_PRESSURE, "atheism is a quantity, not an absence");
        assert_eq!(f.majority(5), None);
        assert_eq!(f.followers().count(), 0);
    }

    #[test]
    fn followers_are_shared_out_by_pressure() {
        let mut f = CityFaith::new();
        f.add_pressure(Some(CivId(0)), 300);
        f.recount(4);
        // 400 pressure over 4 citizens is 100 each: three follow, one does not.
        assert_eq!(f.followers_of(CivId(0)), 3);
        assert_eq!(f.followers().map(|(_, n)| n).sum::<u32>(), 3, "the atheist is not listed");
    }

    #[test]
    fn every_citizen_is_accounted_for_when_the_shares_do_not_divide() {
        // Two religions and an awkward population: largest remainder decides
        // who gets the citizen left over, and nobody is counted twice.
        let mut f = CityFaith::new();
        f.add_pressure(Some(CivId(0)), 250);
        f.add_pressure(Some(CivId(1)), 150);
        f.recount(5);
        let total: u32 = f.followers().map(|(_, n)| n).sum();
        assert!(total <= 5, "{total} followers in a city of five");
        assert!(f.followers_of(CivId(0)) > f.followers_of(CivId(1)), "more pressure, more people");
    }

    #[test]
    fn the_split_is_the_same_whichever_order_the_pressure_arrived_in() {
        let mut a = CityFaith::new();
        a.add_pressure(Some(CivId(1)), 170);
        a.add_pressure(Some(CivId(0)), 170);
        a.recount(6);

        let mut b = CityFaith::new();
        b.add_pressure(Some(CivId(0)), 170);
        b.add_pressure(Some(CivId(1)), 170);
        b.recount(6);
        assert_eq!(a, b, "equal remainders must break the same way every time");
    }

    #[test]
    fn a_majority_needs_only_half_the_city_rounded_down() {
        let mut f = CityFaith::new();
        f.add_pressure(Some(CivId(0)), 400);
        f.recount(5);
        // Unciv's bar is `followers >= population / 2` in integer division,
        // so two of five is enough. Lower than the word suggests, on purpose.
        assert!(f.followers_of(CivId(0)) >= 2);
        assert_eq!(f.majority(5), Some(CivId(0)));
    }

    #[test]
    fn growth_converts_the_newcomers_to_what_the_city_already_believes() {
        let mut f = CityFaith::new();
        f.add_pressure(Some(CivId(0)), 900);
        f.recount(4);
        assert_eq!(f.majority(4), Some(CivId(0)));

        let before = f.pressure_of(Some(CivId(0)));
        f.on_population_change(2, 6);
        assert_eq!(
            f.pressure_of(Some(CivId(0))),
            before + 2 * PRESSURE_PER_CITIZEN,
            "two citizens born into the faith"
        );
    }

    #[test]
    fn a_city_with_no_faith_breeds_atheists() {
        let mut f = CityFaith::new();
        let before = f.pressure_of(None);
        f.on_population_change(3, 4);
        assert_eq!(f.pressure_of(None), before + 3 * PRESSURE_PER_CITIZEN);
        assert_eq!(f.majority(4), None);
    }

    #[test]
    fn an_inquisitor_sweeps_out_everything_but_one_faith() {
        let mut f = CityFaith::new();
        f.add_pressure(Some(CivId(0)), 500);
        f.add_pressure(Some(CivId(1)), 400);
        f.remove_all_pressures_except(CivId(0));
        assert_eq!(f.pressure_of(Some(CivId(1))), 0);
        assert_eq!(f.pressure_of(Some(CivId(0))), 500);
        assert!(f.pressure_of(None) > 0, "you cannot convert someone to nothing");
    }

    #[test]
    fn a_religion_reaches_its_neighbours_and_not_the_far_side_of_the_map() {
        let r = ruleset();
        let mut w = World::new(
            TileGrid::new(40, 8, r.terrain("Grassland").unwrap()),
            vec![
                Civilization::new(CivId(0), r.nations.ids().next().unwrap(), true),
                Civilization::new(CivId(1), r.nations.ids().next().unwrap(), true),
            ],
        );
        let rome = city_at(&mut w, &r, "Rome", CivId(0), 1, 4, 6);
        let near = city_at(&mut w, &r, "Near", CivId(1), 6, 4, 6);
        let far = city_at(&mut w, &r, "Far", CivId(1), 35, 4, 6);
        convert(&mut w, rome, CivId(0));

        for _ in 0..40 {
            spread(&mut w, &r, 6);
        }
        assert!(w.cities.get(near).unwrap().faith.pressure_of(Some(CivId(0))) > 0, "five tiles is within reach");
        assert_eq!(w.cities.get(far).unwrap().faith.pressure_of(Some(CivId(0))), 0, "thirty-four tiles is not");
    }

    #[test]
    fn a_neighbour_converts_eventually_and_says_so_once() {
        let r = ruleset();
        let mut w = World::new(
            TileGrid::new(20, 8, r.terrain("Grassland").unwrap()),
            vec![
                Civilization::new(CivId(0), r.nations.ids().next().unwrap(), true),
                Civilization::new(CivId(1), r.nations.ids().next().unwrap(), true),
            ],
        );
        let rome = city_at(&mut w, &r, "Rome", CivId(0), 1, 4, 8);
        let target = city_at(&mut w, &r, "Target", CivId(1), 5, 4, 6);
        convert(&mut w, rome, CivId(0));

        let mut announcements = 0;
        let mut converted_on = None;
        for turn in 1..=60 {
            for c in spread(&mut w, &r, 6) {
                if c.city == target {
                    announcements += 1;
                    converted_on.get_or_insert(turn);
                }
            }
        }
        let turn = converted_on.expect("a neighbour should convert inside sixty turns");
        assert_eq!(w.cities.get(target).unwrap().faith.majority(6), Some(CivId(0)));
        assert_eq!(announcements, 1, "converted on turn {turn}, and announced once");
    }

    #[test]
    fn a_holy_city_holds_out_against_its_neighbour() {
        let r = ruleset();
        let mut w = World::new(
            TileGrid::new(20, 8, r.terrain("Grassland").unwrap()),
            vec![
                Civilization::new(CivId(0), r.nations.ids().next().unwrap(), true),
                Civilization::new(CivId(1), r.nations.ids().next().unwrap(), true),
            ],
        );
        let big = city_at(&mut w, &r, "Big", CivId(0), 1, 4, 10);
        let holy = city_at(&mut w, &r, "Holy", CivId(1), 4, 4, 4);
        convert(&mut w, big, CivId(0));
        convert(&mut w, holy, CivId(1));
        w.cities.get_mut(holy).unwrap().faith.holy_city_of = Some(CivId(1));

        for _ in 0..200 {
            spread(&mut w, &r, 6);
        }
        assert_eq!(
            w.cities.get(holy).unwrap().faith.majority(4),
            Some(CivId(1)),
            "a religion's birthplace does not fall to the faith next door"
        );
    }

    #[test]
    fn spreading_does_not_depend_on_the_order_cities_were_founded() {
        let r = ruleset();
        let build = |order: [(&str, u8, u16); 3]| {
            let mut w = World::new(
                TileGrid::new(20, 8, r.terrain("Grassland").unwrap()),
                vec![
                    Civilization::new(CivId(0), r.nations.ids().next().unwrap(), true),
                    Civilization::new(CivId(1), r.nations.ids().next().unwrap(), true),
                ],
            );
            let mut founded = Vec::new();
            for (name, owner, col) in order {
                founded.push((col, city_at(&mut w, &r, name, CivId(owner), col, 4, 6)));
            }
            for (col, id) in &founded {
                if *col == 1 {
                    convert(&mut w, *id, CivId(0));
                }
            }
            for _ in 0..30 {
                spread(&mut w, &r, 6);
            }
            let mut out: Vec<(u16, i32)> = founded
                .iter()
                .map(|(col, id)| (*col, w.cities.get(*id).unwrap().faith.pressure_of(Some(CivId(0)))))
                .collect();
            out.sort();
            out
        };

        let a = build([("Rome", 0, 1), ("B", 1, 5), ("C", 1, 9)]);
        let b = build([("C", 1, 9), ("B", 1, 5), ("Rome", 0, 1)]);
        assert_eq!(a, b, "the same map must spread the same way whatever order it was built in");
    }

    /// A religion outlives the civ that founded it.
    ///
    /// An audit proposed wiping a defeated civ's religious pressure from every
    /// city, on the grounds that a dead civ should not keep influencing the
    /// game. That is the wrong reading. A religion is not its founder: Rome
    /// falling does not make Romans stop believing, and in Civ V a religion
    /// whose founder is gone carries on spreading and is still the majority
    /// faith of every city that holds it. Wiping the pressure would instantly
    /// de-convert dozens of cities for a reason no rule states.
    ///
    /// Asserted here because it is a deliberate choice that looks like an
    /// oversight, and the next person to read `spread` may well try to "fix"
    /// it too.
    #[test]
    fn a_religion_outlives_the_civ_that_founded_it() {
        let r = ruleset();
        let mut w = World::new(
            TileGrid::new(20, 8, r.terrain("Grassland").unwrap()),
            vec![
                Civilization::new(CivId(0), r.nations.ids().next().unwrap(), true),
                Civilization::new(CivId(1), r.nations.ids().next().unwrap(), true),
            ],
        );
        let rome = city_at(&mut w, &r, "Rome", CivId(0), 1, 4, 8);
        let neighbour = city_at(&mut w, &r, "Veii", CivId(1), 5, 4, 6);
        convert(&mut w, rome, CivId(0));

        // The founder is wiped out; the city it converted changes hands.
        w.civ_mut(CivId(0)).unwrap().defeated = true;
        w.cities.get_mut(rome).unwrap().owner = CivId(1);

        for _ in 0..80 {
            spread(&mut w, &r, 6);
        }
        assert_eq!(
            w.cities.get(rome).unwrap().faith.majority(8),
            Some(CivId(0)),
            "the congregation does not disband because the founder did"
        );
        assert!(w.cities.get(neighbour).unwrap().faith.pressure_of(Some(CivId(0))) > 0, "and it keeps spreading");
    }
}
