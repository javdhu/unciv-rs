//! Real faiths that arise in their era (docs/CONTENT.md, Religion).
//!
//! In `rules/base` nobody founds a religion. Each faith in `Religions.json`
//! arises once the first major civ has spent its `delay` in the faith's era:
//! in its homeland nation if one is in the game, otherwise in the city with
//! the most faith at that moment. From its holy city it spreads by the same
//! pressure as any religion, and missionaries carry it further. A civ's one
//! choice is its state faith, which brings the faith's `State` traits; cities
//! that follow a faith get its `Follower` traits.
//!
//! A religion is identified by the civ that founded it ([`Creed`]). A faith
//! that arose by era has no founder, so it is identified by
//! [`key`]: a `CivId` above any real civ, which `world.civ` never answers.
//! [`crate::world::religion::religion_exists`] and
//! [`crate::world::religion::civ_religion`] are the questions every system
//! asks instead of looking a founder up.
//!
//! [`Creed`]: crate::world::religion::Creed

use crate::arena::Id;
use crate::ruleset::civgame::FaithDef;
use crate::ruleset::{Belief, Ruleset};
use crate::stats::Fixed;
use crate::world::city::City;
use crate::world::grid::CivId;
use crate::world::world::World;
use std::collections::BTreeMap;

/// Faith keys start here, clear of every real civ (at most 16 majors, 16
/// city-states and the barbarians).
pub const FAITH_KEY_BASE: u8 = 128;

/// Turns a civ keeps a state faith before it may change it.
pub const STATE_FAITH_LOCK: u32 = 10;

/// Pressure a faith's new holy city starts with, per citizen — what a
/// prophet's founding gives (`religious_units`).
pub const ARISING_PRESSURE: i32 = 500;

/// The religion key of the faith at `index` in `Religions.json`.
pub fn key(index: usize) -> CivId {
    CivId(FAITH_KEY_BASE.saturating_add(index.min(127) as u8))
}

/// Which faith a religion key names, if it names one.
pub fn index_of(key: CivId) -> Option<usize> {
    key.0.checked_sub(FAITH_KEY_BASE).map(usize::from)
}

pub fn def(r: &Ruleset, key: CivId) -> Option<&FaithDef> {
    r.faiths.get(index_of(key)?)
}

/// Where the faiths of a game stand.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FaithWorld {
    /// The turn the first major civ reached each era, by era number.
    pub era_reached: BTreeMap<i32, u32>,
    /// Faiths that have arisen, by index in `Religions.json`.
    pub arisen: BTreeMap<u8, Arisen>,
}

#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Arisen {
    pub turn: u32,
    /// Its holy city when it arose.
    pub city: Id<City>,
}

pub fn has_arisen(world: &World, key: CivId) -> bool {
    index_of(key).is_some_and(|i| world.faiths.arisen.contains_key(&(i as u8)))
}

/// The traits of `kind` (`Follower` or `State`) a faith carries.
pub fn traits<'r>(r: &'r Ruleset, key: CivId, kind: &'r str) -> impl Iterator<Item = &'r Belief> + 'r {
    def(r, key)
        .into_iter()
        .flat_map(|f| f.beliefs.iter())
        .filter_map(move |name| r.beliefs.by_name(name))
        .filter(move |b| b.r#type == kind)
}

/// A faith arose this turn, or a civ took one as its state faith.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FaithNews {
    Arose { religion: CivId, city: Id<City> },
    StateFaithAdopted { civ: CivId, religion: CivId },
}

/// One turn of the faiths: note which eras have begun, raise every faith
/// that is due, and let computer players take up a state faith.
pub fn tick(world: &mut World, r: &Ruleset, speed: Fixed) -> Vec<FaithNews> {
    if !r.faith_mode() {
        return Vec::new();
    }
    let reached = world
        .civs
        .iter()
        .filter(|c| c.is_major() && !c.defeated)
        .map(|c| crate::world::policies::era_number(world, r, c.id))
        .max()
        .unwrap_or(0);
    let turn = world.turn;
    world.faiths.era_reached.entry(reached).or_insert(turn);

    let mut news = Vec::new();
    for (i, faith) in r.faiths.iter().enumerate() {
        if world.faiths.arisen.contains_key(&(i as u8)) || !is_due(world, r, faith, reached, speed) {
            continue;
        }
        let Some(city) = holy_city_for(world, r, faith) else { continue };
        arise(world, i, city);
        news.push(FaithNews::Arose { religion: key(i), city });
    }

    let civs: Vec<_> =
        world.civs.iter().filter(|c| c.is_ai && c.is_major() && c.faith.state.is_none()).map(|c| c.id).collect();
    for civ in civs {
        if let Some(religion) = preferred(world, r, civ) {
            if adopt(world, r, civ, religion).is_ok() {
                news.push(FaithNews::StateFaithAdopted { civ, religion });
            }
        }
    }
    news
}

/// A faith is due once the world is past its era, or has spent `delay`
/// turns (scaled by game speed) in it. Eras skipped over (a game started
/// later) raise their faiths at once.
fn is_due(world: &World, r: &Ruleset, faith: &FaithDef, reached: i32, speed: Fixed) -> bool {
    let Some(era) = r.eras.by_name(&faith.era).map(|e| e.era_number) else { return false };
    if reached > era {
        return true;
    }
    if reached < era {
        return false;
    }
    let since = world.faiths.era_reached.get(&era).copied().unwrap_or(world.turn);
    let delay = (Fixed::whole(faith.delay) * speed).round().max(0) as u32;
    world.turn >= since.saturating_add(delay)
}

/// A living city of the homeland nation, the capital first; else the city
/// with the most faith anywhere. Holy cities and the barbarians' are passed
/// over. Ties go to the lower city id.
fn holy_city_for(world: &World, r: &Ruleset, faith: &FaithDef) -> Option<Id<City>> {
    let eligible = |id: Id<City>, c: &City| {
        c.faith.holy_city_of.is_none()
            && world.civ(c.owner).is_some_and(|o| !o.is_barbarian && !o.defeated)
            && world.cities.get(id).is_some()
    };
    let faith_of = |c: &City| crate::world::city::city_yield_in(world, r, c, false).faith;
    for nation in &faith.homeland {
        let Some(nation) = r.nations.id(nation) else { continue };
        let Some(civ) = world.civs.iter().find(|c| c.nation == nation && !c.defeated) else { continue };
        let capital = crate::world::connections::capital_of(world, r, civ.id);
        let best = civ
            .cities
            .iter()
            .filter_map(|id| Some((*id, world.cities.get(*id)?)))
            .filter(|(id, c)| eligible(*id, c))
            .max_by_key(|(id, c)| (Some(*id) == capital, faith_of(c), std::cmp::Reverse(*id)));
        if let Some((id, _)) = best {
            return Some(id);
        }
    }
    world
        .cities
        .iter()
        .filter(|(id, c)| eligible(*id, c))
        .max_by_key(|(id, c)| (faith_of(c), c.population, std::cmp::Reverse(*id)))
        .map(|(id, _)| id)
}

fn arise(world: &mut World, index: usize, city: Id<City>) {
    let religion = key(index);
    let turn = world.turn;
    world.faiths.arisen.insert(index as u8, Arisen { turn, city });
    let c = world.cities.get_mut(city).expect("chosen holy city");
    c.faith.holy_city_of = Some(religion);
    c.faith.holy_city_blocked = false;
    c.faith.add_pressure(Some(religion), c.population.max(1) as i32 * ARISING_PRESSURE);
    c.faith.recount(c.population);
}

/// Why a state faith cannot be adopted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StateFaithRejection {
    NotFaithMode,
    NotMajor,
    NoSuchFaith,
    NotFollowed,
    AlreadyAdopted,
    TooSoon { turns_left: u32 },
}

/// Faiths `civ` may take as its state faith: arisen, and the majority in at
/// least one of its cities.
pub fn adoptable(world: &World, r: &Ruleset, civ: CivId) -> Vec<CivId> {
    let Some(c) = world.civ(civ) else { return Vec::new() };
    let mut out: Vec<_> = c
        .cities
        .iter()
        .filter_map(|id| world.cities.get(*id))
        .filter_map(|city| city.faith.majority(city.population))
        .filter(|religion| index_of(*religion).is_some_and(|i| i < r.faiths.len()) && has_arisen(world, *religion))
        .collect();
    out.sort();
    out.dedup();
    out
}

pub fn can_adopt(world: &World, r: &Ruleset, civ: CivId, religion: CivId) -> Result<(), StateFaithRejection> {
    use StateFaithRejection as E;
    if !r.faith_mode() {
        return Err(E::NotFaithMode);
    }
    let c = world.civ(civ).filter(|c| c.is_major() && !c.defeated).ok_or(E::NotMajor)?;
    if def(r, religion).is_none() || !has_arisen(world, religion) {
        return Err(E::NoSuchFaith);
    }
    if c.faith.state == Some(religion) {
        return Err(E::AlreadyAdopted);
    }
    if c.faith.state.is_some() {
        let free_at = c.faith.state_since.saturating_add(STATE_FAITH_LOCK);
        if world.turn < free_at {
            return Err(E::TooSoon { turns_left: free_at - world.turn });
        }
    }
    if !adoptable(world, r, civ).contains(&religion) {
        return Err(E::NotFollowed);
    }
    Ok(())
}

/// Take `religion` as `civ`'s state faith: its `State` traits apply from now.
pub fn adopt(world: &mut World, r: &Ruleset, civ: CivId, religion: CivId) -> Result<(), StateFaithRejection> {
    can_adopt(world, r, civ, religion)?;
    let turn = world.turn;
    let c = world.civ_mut(civ).expect("validated civ");
    c.faith.state = Some(religion);
    c.faith.state_since = turn;
    crate::world::policies::rebuild_uniques(world, r, civ);
    Ok(())
}

/// What a computer player adopts: the faith most of its citizens follow,
/// ties to the lower key.
fn preferred(world: &World, r: &Ruleset, civ: CivId) -> Option<CivId> {
    let options = adoptable(world, r, civ);
    let followers = |religion: CivId| -> u32 {
        world.civ(civ).map_or(0, |c| {
            c.cities.iter().filter_map(|id| world.cities.get(*id)).map(|city| city.faith.followers_of(religion)).sum()
        })
    };
    options.into_iter().max_by_key(|k| (followers(*k), std::cmp::Reverse(*k)))
}
