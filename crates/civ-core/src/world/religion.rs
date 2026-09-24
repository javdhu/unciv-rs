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
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Faith {
    pub stored: i32,
    /// Ordered, so iteration and serialisation are reproducible.
    pub beliefs: std::collections::BTreeSet<Id<Belief>>,
    /// The original pantheon survives separately when its founder creates a religion.
    pub pantheon_beliefs: std::collections::BTreeSet<Id<Belief>>,
    pub prophets_earned: i32,
    /// A religion exists once a prophet has founded one.
    pub founded: bool,
    /// An enhancer and a second follower belief have been selected.
    pub enhanced: bool,
    /// The faith this civ has adopted as its own, when faiths arise by era
    /// (`world::faiths`): its religion key.
    #[cfg_attr(feature = "serde", serde(default))]
    pub state: Option<CivId>,
    /// The turn the state faith was adopted.
    #[cfg_attr(feature = "serde", serde(default))]
    pub state_since: u32,
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
    let taken = world.civs.iter().filter(|c| c.is_major() && c.faith.has_pantheon()).count() as i32;
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

pub fn prophet_cost_in(world: &World, r: &Ruleset, civ: CivId, speed: Fixed) -> i32 {
    let Some(c) = world.civ(civ) else { return PROPHET_BASE };
    let n = c.faith.prophets_earned.max(0);
    let faith_speed = world.speed_rules(r, speed).map_or(speed, |s| s.faith_cost_modifier);
    let mut cost = Fixed::whole(PROPHET_BASE + PROPHET_STEP * n * (n + 1) / 2) * faith_speed;
    let ctx = crate::world::conditionals::GameContext::new(r).with_world(world).for_civ(civ);
    for u in c
        .uniques
        .matching(crate::UniqueType::FaithCostOfGreatProphetChange)
        .filter(|u| crate::world::conditionals::applies(u, &ctx) == Verdict::True)
    {
        cost =
            cost * Fixed::from_fraction(100 + u.params.first().and_then(|p| p.parse::<i64>().ok()).unwrap_or(0), 100);
    }
    cost.floor().max(1) as i32
}

/// Beliefs of a kind that nobody has taken.
pub fn available_beliefs(world: &World, r: &Ruleset, kind: &str) -> Vec<Id<Belief>> {
    let taken: std::collections::BTreeSet<_> =
        world.civs.iter().flat_map(|c| c.faith.beliefs.iter().copied()).collect();
    r.beliefs.iter().filter(|(id, b)| b.r#type == kind && !taken.contains(id)).map(|(id, _)| id).collect()
}

/// Whether a religion key names a religion that exists: one a civ founded, or
/// a faith that has arisen by era (`world::faiths`).
pub fn religion_exists(world: &World, creed: CivId) -> bool {
    if crate::world::faiths::index_of(creed).is_some() {
        crate::world::faiths::has_arisen(world, creed)
    } else {
        world.civ(creed).is_some_and(|c| c.faith.founded)
    }
}

/// The religion a civ calls its own: the one it founded, or else its state
/// faith. What "your religion" means to quests, missionaries and beliefs.
pub fn civ_religion(world: &World, civ: CivId) -> Option<CivId> {
    let c = world.civ(civ)?;
    if c.faith.founded {
        Some(civ)
    } else {
        c.faith.state
    }
}

/// Bundled rules: one religion plus half the original major-civilization count,
/// bounded by available symbols. Defeated civilizations still count.
pub fn max_religions(world: &World, r: &Ruleset) -> usize {
    r.religions.len().min(1 + world.civs.iter().filter(|c| c.is_major()).count() / 2)
}

/// Founding also needs an unclaimed founder and follower belief.
pub fn remaining_foundable_religions(world: &World, r: &Ruleset) -> usize {
    if r.faith_mode() {
        return 0;
    }
    max_religions(world, r)
        .saturating_sub(world.civs.iter().filter(|c| c.faith.founded).count())
        .min(available_beliefs(world, r, "Founder").len())
        .min(available_beliefs(world, r, "Follower").len())
}

/// Eligibility for an initial pantheon, excluding faith affordability.
/// Once any religion is enhanced, the religion limit also closes late pantheons.
pub fn can_found_pantheon(world: &World, r: &Ruleset, civ: CivId) -> bool {
    !r.faith_mode()
        && world.civ(civ).is_some_and(|c| c.is_major() && !c.faith.has_pantheon())
        && !available_beliefs(world, r, "Pantheon").is_empty()
        && !(world.civs.iter().any(|c| c.faith.enhanced)
            && world.civs.iter().filter(|c| c.faith.has_pantheon()).count() >= max_religions(world, r))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FaithEvent {
    PantheonFounded { belief: Id<Belief> },
    ProphetBorn { unit: Id<crate::world::entities::Unit> },
    Stored { faith: i32 },
}

/// Spend a turn's faith: a pantheon first, then a physical great prophet.
///
/// Returns what happened so the caller can record it; the civ's unique index
/// is rebuilt by the caller, since beliefs are uniques and have to reach it to
/// do anything.
pub fn apply_faith(world: &mut World, r: &Ruleset, civ_id: CivId, gained: i32, speed: Fixed) -> FaithEvent {
    if let Some(c) = world.civ_mut(civ_id) {
        c.add_faith(gained.max(0));
    }
    let stored = world.civ(civ_id).map_or(0, |c| c.faith.stored);
    // Faiths that arise by era take no founding: faith is only spent.
    if r.faith_mode() {
        return FaithEvent::Stored { faith: stored };
    }
    let has_pantheon = world.civ(civ_id).is_some_and(|c| c.faith.has_pantheon());

    // A pantheon comes first and is cheap.
    if !has_pantheon {
        let cost = pantheon_cost(world, speed);
        if can_found_pantheon(world, r, civ_id) && stored >= cost {
            let choices = available_beliefs(world, r, "Pantheon");
            // ReligionAutomation.choosePantheon picks the best rated; the
            // classic AI took the first.
            let pick = if world.is_legacy_ai(civ_id) {
                choices.iter().copied().min()
            } else {
                crate::world::belief_rating::best(world, r, civ_id, &choices)
            };
            if let Some(belief) = pick {
                if let Some(c) = world.civ_mut(civ_id) {
                    c.faith.stored -= cost;
                    c.faith.beliefs.insert(belief);
                }
                on_pantheon_founded(world, civ_id);
                return FaithEvent::PantheonFounded { belief };
            }
        }
        return FaithEvent::Stored { faith: stored };
    }

    // Each turn gets a reproducible roll, with five percent at the threshold
    // and one percentage point per excess faith (ReligionManager.generateProphet).
    let Some(civ) = world.civ(civ_id) else { return FaithEvent::Stored { faith: stored } };
    let ctx = crate::world::conditionals::GameContext::new(r).with_world(world).for_civ(civ_id);
    if civ.is_major()
        && (civ.faith.founded || remaining_foundable_religions(world, r) > 0)
        && !civ
            .uniques
            .matching(crate::UniqueType::MayNotGenerateGreatProphet)
            .any(|u| crate::world::conditionals::applies(u, &ctx) == Verdict::True)
    {
        let cost = prophet_cost_in(world, r, civ_id, speed);
        let chance = Fixed::from_fraction(5 + i64::from(stored) - i64::from(cost), 100);
        if stored >= cost && crate::rng::Rng::seeded(u64::from(world.turn)).unit_interval() < chance {
            if let Some(unit) = crate::world::religious_units::spawn_prophet(world, r, civ_id) {
                if let Some(civ) = world.civ_mut(civ_id) {
                    civ.faith.stored -= cost;
                    civ.faith.prophets_earned += 1;
                }
                return FaithEvent::ProphetBorn { unit };
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

/// Pressure a trade route carries each way between a city and its capital,
/// in percent of one city's push, when faiths arise by era.
pub const TRADE_ROUTE_PRESSURE_PERCENT: i32 = 50;

/// Pressure a state faith adds in each of its adopter's cities, as a multiple
/// of one city's push: a fifth of what a holy city applies.
pub const STATE_FAITH_PRESSURE: i32 = 1;

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
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CityFaith {
    /// Sparse: a religion that has never reached this city has no entry, and
    /// most cities in most games touch two or three religions at most.
    #[cfg_attr(feature = "serde", serde(with = "crate::serialization::map_as_entries"))]
    pressure: std::collections::BTreeMap<Creed, i32>,
    /// Derived from pressure by [`CityFaith::recount`]. Atheists are not
    /// listed — they are the remainder.
    #[cfg_attr(feature = "serde", serde(with = "crate::serialization::map_as_entries"))]
    followers: std::collections::BTreeMap<CivId, u32>,
    #[cfg_attr(feature = "serde", serde(with = "crate::serialization::map_as_entries"))]
    pantheon_pressure: std::collections::BTreeMap<CivId, i32>,
    #[cfg_attr(feature = "serde", serde(with = "crate::serialization::map_as_entries"))]
    pantheon_followers: std::collections::BTreeMap<CivId, u32>,
    /// The city a religion was founded in pushes harder and cannot be
    /// converted away by ordinary pressure.
    pub holy_city_of: Option<CivId>,
    /// An inquisitor can suppress a conquered holy city until it is restored.
    pub holy_city_blocked: bool,
    /// Permanent first-adoption history, including earlier owners of this city.
    pub adopted_religions: std::collections::BTreeSet<CivId>,
}

impl Default for CityFaith {
    fn default() -> CityFaith {
        CityFaith {
            pressure: std::collections::BTreeMap::from([(None, ATHEIST_PRESSURE)]),
            followers: std::collections::BTreeMap::new(),
            pantheon_pressure: std::collections::BTreeMap::new(),
            pantheon_followers: std::collections::BTreeMap::new(),
            holy_city_of: None,
            holy_city_blocked: false,
            adopted_religions: Default::default(),
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
        self.pressure.values().chain(self.pantheon_pressure.values()).sum()
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

    pub fn pantheon_pressures(&self) -> impl Iterator<Item = (CivId, i32)> + '_ {
        self.pantheon_pressure.iter().map(|(c, p)| (*c, *p))
    }

    pub fn pantheon_followers(&self) -> impl Iterator<Item = (CivId, u32)> + '_ {
        self.pantheon_followers.iter().map(|(c, n)| (*c, *n))
    }

    pub fn add_pantheon_pressure(&mut self, civ: CivId, amount: i32) {
        *self.pantheon_pressure.entry(civ).or_insert(0) += amount;
    }

    /// Pantheons are local to their founder's empire, unlike full religions.
    pub fn retain_owner_pantheon(&mut self, owner: CivId, population: u32) {
        self.pantheon_pressure.retain(|civ, _| *civ == owner);
        self.recount(population);
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
        self.pantheon_pressure.clear();
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
        self.majority_kind(population).filter(|(_, pantheon)| !pantheon).map(|(civ, _)| civ)
    }

    pub fn majority_pantheon(&self, population: u32) -> Option<CivId> {
        self.majority_kind(population).filter(|(_, pantheon)| *pantheon).map(|(civ, _)| civ)
    }

    pub fn majority_followers(&self, population: u32) -> u32 {
        self.majority_kind(population).map_or(0, |(civ, pantheon)| {
            if pantheon {
                self.pantheon_followers[&civ]
            } else {
                self.followers[&civ]
            }
        })
    }

    fn majority_kind(&self, population: u32) -> Option<(CivId, bool)> {
        let (civ, pantheon, count) = self
            .followers()
            .map(|(c, n)| (c, false, n))
            .chain(self.pantheon_followers().map(|(c, n)| (c, true, n)))
            .max_by_key(|(civ, pantheon, n)| (*n, std::cmp::Reverse((*civ, *pantheon))))?;
        if count >= population / 2 && count > 0 {
            Some((civ, pantheon))
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
        self.pantheon_followers.clear();
        if population == 0 {
            return;
        }
        let total: i32 = self.total_pressure();
        let per_follower = total / population as i32;
        if per_follower <= 0 {
            return; // less pressure than people; nobody has convinced anyone
        }

        let mut remainders: Vec<(Creed, bool, i32)> = Vec::new();
        let mut allocated: u32 = 0;
        let pressures = self
            .pressures()
            .map(|(c, p)| (c, false, p))
            .chain(self.pantheon_pressures().map(|(c, p)| (Some(c), true, p)))
            .collect::<Vec<_>>();
        for (creed, pantheon, pressure) in pressures {
            let share = (pressure / per_follower).max(0) as u32;
            if let Some(civ) = creed {
                if share > 0 {
                    if pantheon {
                        self.pantheon_followers.insert(civ, share);
                    } else {
                        self.followers.insert(civ, share);
                    }
                }
            }
            allocated += share;
            remainders.push((creed, pantheon, pressure - share as i32 * per_follower));
        }

        // Largest remainder first, ties to the lower civ id, atheism last of
        // all so a citizen goes to a religion before going to nobody.
        remainders.sort_by_key(|(creed, pantheon, rem)| {
            (std::cmp::Reverse(*rem), creed.is_none(), creed.map(|c| c.0), *pantheon)
        });
        let mut unallocated = population.saturating_sub(allocated);
        for (creed, pantheon, _) in &remainders {
            if unallocated == 0 {
                break;
            }
            if let Some(civ) = creed {
                if *pantheon {
                    *self.pantheon_followers.entry(*civ).or_insert(0) += 1;
                } else {
                    *self.followers.entry(*civ).or_insert(0) += 1;
                }
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
            match self.majority_kind(population.saturating_sub(change as u32)) {
                Some((civ, true)) => self.add_pantheon_pressure(civ, PRESSURE_PER_CITIZEN * change),
                creed => self.add_pressure(creed.map(|(civ, _)| civ), PRESSURE_PER_CITIZEN * change),
            }
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
    spread_with_speed(world, r, pressure_per_city, Fixed::ONE)
}
pub fn spread_with_speed(world: &mut World, r: &Ruleset, pressure_per_city: i32, speed: Fixed) -> Vec<Conversion> {
    let before: std::collections::BTreeMap<_, _> =
        world.cities.iter().map(|(id, c)| (id, c.faith.majority(c.population))).collect();

    // Plan every push before applying any, so no city's contribution depends
    // on whether it was visited before or after another.
    let mut pushes: Vec<(Id<crate::world::city::City>, Creed, i32)> = Vec::new();
    let ids: Vec<_> = world.cities.iter().map(|(id, _)| id).collect();

    for &id in &ids {
        let Some(city) = world.cities.get(id) else { continue };
        // A state promotes its faith at home (Ashoka, Constantine), more gently
        // than a holy city pushes: one city's worth of pressure in each of its own.
        if let Some(state) = world.civ(city.owner).and_then(|c| c.faith.state) {
            pushes.push((id, Some(state), STATE_FAITH_PRESSURE * pressure_per_city));
        }
        if !city.faith.holy_city_blocked {
            if let Some(holy) = city.faith.holy_city_of {
                pushes.push((id, Some(holy), HOLY_CITY_MULTIPLIER * pressure_per_city));
            }
        }
        let Some(majority) = city.faith.majority(city.population) else { continue };
        if !religion_exists(world, majority) {
            continue;
        }
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

    // Faiths travel with trade (docs/CONTENT.md, Religion): each city's
    // route to its capital carries the majority faith of either end to the
    // other, at half a city's push. Faiths by era only.
    if r.faith_mode() {
        let amount = pressure_per_city * TRADE_ROUTE_PRESSURE_PERCENT / 100;
        let civs: Vec<CivId> = world.civs.iter().filter(|c| !c.is_barbarian && !c.defeated).map(|c| c.id).collect();
        for civ in civs {
            let connections = crate::world::connections::find(world, r, civ);
            let Some(capital) = connections.capital() else { continue };
            let majority = |id: Id<crate::world::city::City>| {
                world.cities.get(id).and_then(|c| c.faith.majority(c.population)).filter(|m| religion_exists(world, *m))
            };
            for city in connections.trade_routes().filter(|c| *c != capital) {
                if let Some(faith) = majority(city) {
                    pushes.push((capital, Some(faith), amount));
                }
                if let Some(faith) = majority(capital) {
                    pushes.push((city, Some(faith), amount));
                }
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
    settle_adoptions(world, r, speed);
    converted
}

/// Each city pays its founder's first-conversion reward at most once per religion.
pub fn settle_adoptions(world: &mut World, r: &Ruleset, speed: Fixed) {
    use crate::stats::{Stat, Stats};
    use crate::world::conditionals::{applies, GameContext, Verdict};
    let first: Vec<_> = world
        .cities
        .iter()
        .filter_map(|(id, c)| {
            let creed = c.faith.majority(c.population)?;
            (world.civ(creed).is_some_and(|f| f.faith.founded) && !c.faith.adopted_religions.contains(&creed))
                .then_some((id, creed))
        })
        .collect();
    for (city, creed) in first {
        world.cities.get_mut(city).unwrap().faith.adopted_religions.insert(creed);
        let mut reward = Stats::ZERO;
        let ctx = GameContext::new(r).with_world(world).for_civ(creed).in_city(city);
        for u in world.civ(creed).unwrap().uniques.matching(crate::UniqueType::StatsWhenAdoptingReligion) {
            let mut conditional = u.clone();
            conditional.conditionals.retain(|c| c.unique_type() != Some(crate::UniqueType::ModifiedByGameSpeed));
            if applies(&conditional, &ctx) != Verdict::True {
                continue;
            }
            if let Some(s) = u.params.first().and_then(|p| Stats::parse(p)) {
                let factor =
                    if u.conditionals.iter().any(|c| c.unique_type() == Some(crate::UniqueType::ModifiedByGameSpeed)) {
                        speed
                    } else {
                        Fixed::ONE
                    };
                for stat in Stat::ALL {
                    reward[stat] += s[stat] * factor;
                }
            }
        }
        let c = world.civ_mut(creed).unwrap();
        c.gold = c.gold.saturating_add(reward.gold.floor() as i32);
        c.add_culture(reward.culture.floor() as i32);
        c.add_faith(reward.faith.floor() as i32);
        c.science_stored = c.science_stored.saturating_add(reward.science.floor() as i32);
    }
}

/// Every unique a civ's beliefs grant, for the index.
pub fn belief_uniques<'a>(r: &'a Ruleset, faith: &Faith) -> Vec<&'a crate::Unique> {
    faith.beliefs.iter().filter_map(|id| r.beliefs.get(*id)).flat_map(|b| b.uniques.iter()).collect()
}

/// Founder and enhancer effects belong to the civilization that founded the religion.
pub fn founder_uniques<'a>(r: &'a Ruleset, faith: &Faith) -> Vec<&'a crate::Unique> {
    faith
        .beliefs
        .iter()
        .filter_map(|id| r.beliefs.get(*id))
        .filter(|b| matches!(b.r#type.as_str(), "Founder" | "Enhancer"))
        .flat_map(|b| b.uniques.iter())
        .collect()
}

/// Pantheon and follower effects belong to the religion followed by this city,
/// including foreign religions. They never enter the owner's empire index.
pub fn city_belief_uniques<'a>(
    world: &World,
    r: &'a Ruleset,
    city: &crate::world::city::City,
) -> Vec<&'a crate::Unique> {
    let major = city.faith.majority(city.population);
    if let Some(faith) = major.filter(|m| crate::world::faiths::index_of(*m).is_some()) {
        return crate::world::faiths::traits(r, faith, "Follower").flat_map(|b| b.uniques.iter()).collect();
    }
    let Some(creed) = major.or_else(|| city.faith.majority_pantheon(city.population)) else { return Vec::new() };
    let Some(civ) = world.civ(creed) else { return Vec::new() };
    let beliefs = if major.is_some() || civ.faith.pantheon_beliefs.is_empty() && !civ.faith.founded {
        &civ.faith.beliefs
    } else {
        &civ.faith.pantheon_beliefs
    };
    beliefs
        .iter()
        .filter_map(|id| r.beliefs.get(*id))
        .filter(|b| b.r#type == "Pantheon" || major.is_some() && b.r#type == "Follower")
        .flat_map(|b| b.uniques.iter())
        .collect()
}

pub fn on_pantheon_founded(world: &mut World, civ: CivId) {
    if let Some(c) = world.civ_mut(civ) {
        c.faith.pantheon_beliefs.clone_from(&c.faith.beliefs);
    }
    for (_, city) in world.cities.iter_mut().filter(|(_, c)| c.owner == civ) {
        city.faith.add_pantheon_pressure(civ, 200i32.saturating_mul(city.population as i32));
        city.faith.recount(city.population);
    }
}

pub fn on_city_founded(world: &mut World, city: Id<crate::world::city::City>) {
    let Some(c) = world.cities.get(city) else { return };
    if !world.civ(c.owner).is_some_and(|c| c.faith.has_pantheon() && !c.faith.founded) {
        return;
    }
    let c = world.cities.get_mut(city).unwrap();
    c.faith.add_pantheon_pressure(c.owner, 200i32.saturating_mul(c.population as i32));
    c.faith.recount(c.population);
}

/// All effects in a city, without duplicating global building effects.
pub fn city_uniques<'a>(
    world: &'a World,
    r: &'a Ruleset,
    city: &'a crate::world::city::City,
) -> Vec<&'a crate::Unique> {
    city.uniques
        .all()
        .filter(|u| !u.is_global() || u.unique_type() == Some(crate::UniqueType::Stats))
        .chain(world.civ(city.owner).into_iter().flat_map(|c| c.uniques.all()))
        .chain(city_belief_uniques(world, r, city))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::entities::Civilization;
    use crate::world::grid::TileGrid;

    #[test]
    fn pantheon_and_religion_compete_separately_for_the_same_population() {
        let mut faith = CityFaith::new();
        faith.add_pantheon_pressure(CivId(0), 600);
        faith.recount(3);
        assert_eq!(faith.majority_pantheon(3), Some(CivId(0)));
        assert_eq!(faith.majority(3), None);
        assert_eq!(faith.followers_of(CivId(0)), 0);
        faith.on_population_change(1, 4);
        assert_eq!(faith.pantheon_pressures().collect::<Vec<_>>(), vec![(CivId(0), 700)]);
        assert_eq!(faith.pressure_of(Some(CivId(0))), 0);
        faith.add_pressure(Some(CivId(0)), 1000);
        faith.recount(4);
        assert_eq!(faith.followers().map(|(_, n)| n).sum::<u32>(), 2);
        assert_eq!(faith.pantheon_followers().map(|(_, n)| n).sum::<u32>(), 2);
        assert_eq!(faith.majority(4), Some(CivId(0)));
        assert_eq!(faith.majority_pantheon(4), None);
        faith.remove_all_pressures_except(CivId(0));
        faith.recount(4);
        assert_eq!(faith.pantheon_pressures().count(), 0);
        assert_eq!(faith.pantheon_followers().count(), 0);
    }

    #[test]
    fn changing_owners_removes_foreign_pantheons_but_preserves_full_religions() {
        let mut faith = CityFaith::new();
        faith.add_pantheon_pressure(CivId(0), 600);
        faith.add_pantheon_pressure(CivId(1), 200);
        faith.add_pressure(Some(CivId(0)), 500);
        faith.retain_owner_pantheon(CivId(1), 4);
        assert_eq!(faith.pantheon_pressures().collect::<Vec<_>>(), vec![(CivId(1), 200)]);
        assert_eq!(faith.pressure_of(Some(CivId(0))), 500);
        assert_eq!(faith.majority(4), Some(CivId(0)));
    }

    fn ruleset() -> Ruleset {
        Ruleset::load_dir("rules/gods-and-kings").expect("readable").expect("loads")
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

        // Faith earns a physical prophet. Founding is its map action.
        let tile = w.grid.id_at(3, 3).unwrap();
        w.found_city_in(&r, crate::world::city::City::found("Home", CivId(0), tile));
        let e = apply_faith(&mut w, &r, CivId(0), 250, Fixed::ONE);
        let FaithEvent::ProphetBorn { unit } = e else { panic!("{e:?}") };
        assert!(!w.civ(CivId(0)).unwrap().faith.founded);
        crate::world::religious_units::perform(
            &mut w,
            &r,
            unit,
            crate::world::religious_units::ReligiousAction::FoundReligion,
        )
        .unwrap();
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
        w.civ_mut(to).unwrap().faith.founded = true;
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
