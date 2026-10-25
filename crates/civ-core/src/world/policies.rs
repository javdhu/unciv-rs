// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Social policies.
//!
//! Almost all of a policy's *behaviour* is already in the ruleset as uniques,
//! so what has to be written here is the gating — what may be adopted, and what
//! it costs — plus keeping the civ's [`UniqueIndex`] current.
//!
//! The cost curve is worth reading carefully, because it rounds differently
//! from every other cost in the game:
//!
//! ```text
//! cost = 25 + (adopted × 3)^2.01
//! cost = round(cost × difficulty × speed × (1 + perCity × (cities - 1)))
//! cost = cost - (cost % 5)
//! ```
//!
//! `roundToInt`, not `toInt` — policies round where techs and buildings
//! truncate (`PolicyManager.kt:179`). Then the result is floored to a multiple
//! of five, so the number a player sees is always tidy. Getting either wrong
//! drifts a few culture per policy and compounds across a game.

use crate::arena::Id;
use crate::ruleset::{Difficulty, Policy, Ruleset};
use crate::stats::{Fixed, Precise};
use crate::world::conditionals::{applies, GameContext, Verdict};
use crate::world::grid::CivId;
use crate::world::research::MapScaling;
use crate::world::unique_index::UniqueIndex;
use crate::world::world::World;
use crate::UniqueType;

/// Culture for the first policy, before any scaling.
const BASE_POLICY_COST: i32 = 25;

/// Policy costs are always a multiple of this.
const COST_GRANULARITY: i32 = 5;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PolicyModifiers {
    pub difficulty: Fixed,
    /// Speed's `cultureCostModifier`.
    pub speed: Fixed,
    pub map: MapScaling,
}

impl Default for PolicyModifiers {
    fn default() -> Self {
        PolicyModifiers { difficulty: Fixed::ONE, speed: Fixed::ONE, map: MapScaling::default() }
    }
}

impl PolicyModifiers {
    pub fn from_difficulty(d: Option<&Difficulty>, speed: Fixed, map: MapScaling) -> PolicyModifiers {
        PolicyModifiers { difficulty: d.map_or(Fixed::ONE, |d| d.policy_cost_modifier), speed, map }
    }
}

/// Culture needed for the next policy.
///
/// Kept in [`Precise`] through the curve: `^2.01` is a fractional exponent, and
/// quantising the intermediate moves the answer across the five-culture
/// granularity often enough to matter.
pub fn policy_cost(adopted: usize, cities: usize, m: PolicyModifiers) -> i32 {
    let curve = Fixed::whole(adopted as i32 * 3).pow(Fixed::permille(2_010)).unwrap_or(Fixed::ZERO);
    let base = Precise::from_fixed(Fixed::whole(BASE_POLICY_COST) + curve);

    let city_modifier =
        Precise::whole(cities.saturating_sub(1) as i32) * Precise::from_fixed(m.map.policy_cost_per_city);
    let scaled = ((base * Precise::from_fixed(m.difficulty)) * Precise::from_fixed(m.speed))
        * (Precise::whole(1) + city_modifier);

    // Rounds, unlike tech and production, then drops to a multiple of five.
    let cost = scaled.round().max(1) as i32;
    (cost - (cost % COST_GRANULARITY)).max(COST_GRANULARITY)
}

/// Live cost excludes free choices and branch finishers, and uses current
/// non-puppet cities and active policy discounts (`PolicyManager.kt`).
pub fn policy_cost_in(world: &World, r: &Ruleset, civ: CivId, mut m: PolicyModifiers) -> i32 {
    let Some(c) = world.civ(civ) else { return BASE_POLICY_COST };
    let ctx = GameContext::new(r).with_world(world).for_civ(civ);
    for u in c.uniques.matching(UniqueType::LessPolicyCostFromCities).filter(|u| applies(u, &ctx) == Verdict::True) {
        if let Some(p) = u.params.first().and_then(|p| Fixed::parse(p)) {
            m.map.policy_cost_per_city =
                m.map.policy_cost_per_city * (Fixed::ONE - p.checked_div(Fixed::whole(100)).unwrap_or(Fixed::ZERO));
        }
    }
    if c.is_ai {
        m.difficulty = Fixed::ONE;
    }
    for u in c.uniques.matching(UniqueType::LessPolicyCost).filter(|u| applies(u, &ctx) == Verdict::True) {
        if let Some(p) = u.params.first().and_then(|p| Fixed::parse(p)) {
            m.difficulty = m.difficulty * (Fixed::ONE + p.checked_div(Fixed::whole(100)).unwrap_or(Fixed::ZERO));
        }
    }
    let cities = world.cities.iter().filter(|(_, city)| city.owner == civ && !city.puppet).count();
    policy_cost(c.paid_policies as usize, cities, m)
}

pub fn is_finisher(r: &Ruleset, policy: Id<Policy>) -> bool {
    r.policies.get(policy).is_some_and(|p| {
        p.branch.and_then(|b| r.policy_branches.get(b)).is_some_and(|b| p.name == format!("{} Complete", b.name))
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PolicyRejection {
    AlreadyAdopted,
    /// A prerequisite policy has not been adopted.
    MissingPrerequisite(String),
    /// The branch belongs to a later era.
    EraTooEarly {
        needs: String,
    },
    /// An `Only available <…>` conditional does not hold.
    NotAvailable,
    NotEnoughCulture {
        needed: i32,
        stored: i32,
    },
    Unknown,
}

impl std::fmt::Display for PolicyRejection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PolicyRejection::AlreadyAdopted => write!(f, "already adopted"),
            PolicyRejection::MissingPrerequisite(p) => write!(f, "needs {p} first"),
            PolicyRejection::EraTooEarly { needs } => write!(f, "needs the {needs}"),
            PolicyRejection::NotAvailable => write!(f, "not available to this civ now"),
            PolicyRejection::NotEnoughCulture { needed, stored } => {
                write!(f, "needs {needed} culture, has {stored}")
            }
            PolicyRejection::Unknown => write!(f, "not in this ruleset"),
        }
    }
}

impl std::error::Error for PolicyRejection {}

/// The highest era this civ has reached, by researched techs.
pub fn era_number(world: &World, r: &Ruleset, civ: CivId) -> i32 {
    world.civ(civ).map_or(0, |c| {
        c.techs
            .iter()
            .filter_map(|t| r.technologies.get(*t))
            .filter_map(|t| t.era(r))
            .filter_map(|e| r.eras.get(e))
            .map(|e| e.era_number)
            .max()
            .unwrap_or(0)
    })
}

/// The turn `civ` stood in each era by, indexed by era number, up to the era
/// it is in: what the round-end record holds, and this turn for an era
/// reached since the last round closed.
pub fn era_starts(world: &World, r: &Ruleset, civ: CivId) -> Vec<u32> {
    let Some(c) = world.civ(civ) else { return Vec::new() };
    let era = era_number(world, r, civ).max(0) as usize;
    let mut starts = c.era_starts.clone();
    while starts.len() <= era {
        starts.push(world.turn);
    }
    starts
}

/// Record, as a round closes, the eras every civ has reached.
pub fn record_eras(world: &mut World, r: &Ruleset) {
    for civ in world.civ_ids().collect::<Vec<_>>() {
        let starts = era_starts(world, r, civ);
        if let Some(c) = world.civ_mut(civ) {
            c.era_starts = starts;
        }
    }
}

/// Whether this policy may be adopted, ignoring whether the culture is there.
pub fn is_adoptable(world: &World, r: &Ruleset, civ_id: CivId, policy: Id<Policy>) -> Result<(), PolicyRejection> {
    let Some(civ) = world.civ(civ_id) else { return Err(PolicyRejection::Unknown) };
    let Some(p) = r.policies.get(policy) else { return Err(PolicyRejection::Unknown) };

    if civ.adopted_policies.contains(&policy) {
        return Err(PolicyRejection::AlreadyAdopted);
    }
    if is_finisher(r, policy) {
        return Err(PolicyRejection::NotAvailable);
    }
    for required in &p.requires {
        let have = r.policies.id(required).is_some_and(|id| civ.adopted_policies.contains(&id));
        if !have {
            return Err(PolicyRejection::MissingPrerequisite(required.clone()));
        }
    }
    // A branch is gated on its own era; a policy inside one inherits that.
    if let Some(branch) = p.branch.and_then(|b| r.policy_branches.get(b)) {
        if let Some(era) = r.eras.by_name(&branch.era) {
            if era.era_number > era_number(world, r, civ_id) {
                return Err(PolicyRejection::EraTooEarly { needs: branch.era.clone() });
            }
        }
    }
    // `Only available <…>` gates the policy on its conditionals holding.
    let ctx = GameContext::new(r).with_world(world).for_civ(civ_id);
    for u in p.uniques.iter().filter(|u| u.unique_type() == Some(UniqueType::OnlyAvailable)) {
        if applies(u, &ctx) != Verdict::True {
            return Err(PolicyRejection::NotAvailable);
        }
    }
    if p.uniques.iter().any(|u| u.unique_type() == Some(UniqueType::Unavailable) && applies(u, &ctx) == Verdict::True) {
        return Err(PolicyRejection::NotAvailable);
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PolicyEvent {
    Adopted {
        policy: Id<Policy>,
        culture_left: i32,
    },
    /// Culture banked toward the next one.
    Stored {
        culture: i32,
    },
}

/// Bank culture, and adopt `choice` if it is affordable and legal.
pub fn apply_culture(
    world: &mut World,
    r: &Ruleset,
    civ_id: CivId,
    culture: i32,
    choice: Option<Id<Policy>>,
    m: PolicyModifiers,
) -> Result<PolicyEvent, PolicyRejection> {
    if let Some(civ) = world.civ_mut(civ_id) {
        civ.add_culture(culture.max(0));
    }
    let Some(policy) = choice else {
        let stored = world.civ(civ_id).map_or(0, |c| c.culture_stored);
        return Ok(PolicyEvent::Stored { culture: stored });
    };

    is_adoptable(world, r, civ_id, policy)?;

    let Some(civ) = world.civ(civ_id) else { return Err(PolicyRejection::Unknown) };
    let stored = civ.culture_stored;
    let free = civ.pending_rewards.free_policies > 0;
    let cost = policy_cost_in(world, r, civ_id, m);
    if !free && stored < cost {
        return Ok(PolicyEvent::Stored { culture: stored });
    }
    let civ = world.civ_mut(civ_id).unwrap();
    if free {
        civ.pending_rewards.free_policies -= 1;
    } else {
        civ.culture_stored -= cost;
        civ.paid_policies = civ.paid_policies.saturating_add(1);
    }
    civ.adopted_policies.insert(policy);
    register_timed_effects(world, r, civ_id, policy, m.speed);
    rebuild_uniques(world, r, civ_id);
    crate::world::effects::on_policy_adopted(world, r, civ_id, policy, m.speed);
    // A branch's final reward is automatic and never charges culture.
    let finishers: Vec<_> = r
        .policies
        .iter()
        .filter(|(id, p)| {
            is_finisher(r, *id)
                && !world.civ(civ_id).unwrap().adopted_policies.contains(id)
                && r.policies
                    .iter()
                    .filter(|(id, other)| other.branch == p.branch && !is_finisher(r, *id))
                    .all(|(id, _)| world.civ(civ_id).unwrap().adopted_policies.contains(&id))
        })
        .map(|(id, _)| id)
        .collect();
    for finisher in finishers {
        world.civ_mut(civ_id).unwrap().adopted_policies.insert(finisher);
        register_timed_effects(world, r, civ_id, finisher, m.speed);
        rebuild_uniques(world, r, civ_id);
        crate::world::effects::on_policy_adopted(world, r, civ_id, finisher, m.speed);
    }
    let left = world.civ(civ_id).map_or(0, |c| c.culture_stored);
    Ok(PolicyEvent::Adopted { policy, culture_left: left })
}

fn register_timed_effects(world: &mut World, r: &Ruleset, civ: CivId, policy: Id<Policy>, speed: Fixed) {
    let turns = r.policies[policy]
        .uniques
        .iter()
        .flat_map(|u| &u.conditionals)
        .filter(|u| u.unique_type() == Some(UniqueType::ConditionalTimedUnique))
        .filter_map(|u| u.params.first()?.parse::<i32>().ok())
        .max();
    if let Some(turns) = turns {
        let expiry = world.turn.saturating_add((Fixed::whole(turns) * speed).floor().max(1) as u32);
        world.civ_mut(civ).unwrap().timed_policy_expirations.insert(policy, expiry);
    }
}

/// Rebuild a civ's active uniques from scratch.
///
/// Called whenever a source changes. Rebuilding rather than patching is
/// deliberate: a stale index is indistinguishable from a rule that does not
/// exist, and this walks a few hundred uniques.
pub fn rebuild_uniques(world: &mut World, r: &Ruleset, civ_id: CivId) {
    let Some(civ) = world.civ(civ_id) else { return };

    let mut uniques = Vec::new();
    uniques.extend(civ.leader_uniques(r).iter().cloned());
    for policy in &civ.adopted_policies {
        if let Some(p) = r.policies.get(*policy) {
            for unique in &p.uniques {
                let timed =
                    unique.conditionals.iter().any(|u| u.unique_type() == Some(UniqueType::ConditionalTimedUnique));
                if timed && civ.timed_policy_expirations.get(policy).is_none_or(|expiry| *expiry <= world.turn) {
                    continue;
                }
                let mut active = unique.clone();
                active.conditionals.retain(|u| u.unique_type() != Some(UniqueType::ConditionalTimedUnique));
                uniques.push(active);
            }
        }
    }
    for tech in &civ.techs {
        if let Some(t) = r.technologies.get(*tech) {
            uniques.extend(t.uniques.iter().cloned());
        }
    }
    // Beliefs are uniques too, so a pantheon has to reach the index or it
    // grants nothing.
    uniques.extend(crate::world::religion::founder_uniques(r, &civ.faith).into_iter().cloned());
    // A state faith's traits belong to every civ that adopts it.
    if let Some(state) = civ.faith.state {
        uniques.extend(crate::world::faiths::traits(r, state, "State").flat_map(|b| b.uniques.iter()).cloned());
    }
    for city in &civ.cities {
        if let Some(city) = world.cities.get(*city) {
            for building in &city.built {
                if let Some(b) = r.buildings.get(*building) {
                    uniques.extend(
                        b.uniques
                            .iter()
                            .filter(|u| u.is_global() && u.unique_type() != Some(UniqueType::Stats))
                            .cloned(),
                    );
                }
            }
        }
    }
    // So are the bonuses a city-state owes its friends. They come and go as
    // influence rises and falls, which is why this index is rebuilt every
    // turn for a major civ rather than only when something is adopted.
    if civ.is_major() {
        let bonuses = crate::world::city_states::bonus_uniques_with_modifiers(world, r, civ_id, &uniques);
        uniques.extend(bonuses);
    }

    let index = UniqueIndex::build(&uniques);
    if let Some(civ) = world.civ_mut(civ_id) {
        civ.uniques = index;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::city::City;
    use crate::world::entities::Civilization;
    use crate::world::grid::{TileGrid, TileId};

    fn ruleset() -> Ruleset {
        Ruleset::load_dir("rules/gods-and-kings").expect("readable").expect("loads")
    }

    fn world(r: &Ruleset) -> World {
        let g = TileGrid::new(16, 16, r.terrain("Grassland").unwrap());
        let nation = r.nations.ids().next().unwrap();
        World::new(g, vec![Civilization::new(CivId(0), nation, false)])
    }

    #[test]
    fn the_cost_curve_climbs_and_stays_tidy() {
        let m = PolicyModifiers::default();
        let costs: Vec<i32> = (0..8).map(|n| policy_cost(n, 1, m)).collect();
        println!("{costs:?}");

        assert!(costs.windows(2).all(|w| w[1] > w[0]), "each policy costs more");
        assert!(costs.iter().all(|c| c % 5 == 0), "always a multiple of five");
        assert_eq!(costs[0], 25, "the first policy is the base cost");
    }

    #[test]
    fn a_wider_empire_pays_more_and_a_bigger_map_softens_it() {
        let m = PolicyModifiers::default();
        assert!(policy_cost(4, 8, m) > policy_cost(4, 1, m), "cities cost culture");

        let tiny = PolicyModifiers { map: MapScaling::TINY, ..m };
        let huge = PolicyModifiers { map: MapScaling::HUGE, ..m };
        let growth = |x: PolicyModifiers| policy_cost(4, 9, x) - policy_cost(4, 1, x);
        println!("penalty for 9 cities: tiny {}, huge {}", growth(tiny), growth(huge));
        assert!(growth(huge) < growth(tiny), "a bigger map expects more cities");
    }

    #[test]
    fn a_branch_must_be_taken_before_what_is_inside_it() {
        let r = ruleset();
        let mut w = world(&r);
        let aristocracy = r.policies.id("Aristocracy").expect("Aristocracy exists");

        assert!(matches!(is_adoptable(&w, &r, CivId(0), aristocracy), Err(PolicyRejection::MissingPrerequisite(_))));

        let tradition = r.policies.id("Tradition").expect("the branch is a policy too");
        assert_eq!(is_adoptable(&w, &r, CivId(0), tradition), Ok(()));

        w.civ_mut(CivId(0)).unwrap().adopted_policies.insert(tradition);
        assert_eq!(is_adoptable(&w, &r, CivId(0), aristocracy), Ok(()));
        assert_eq!(is_adoptable(&w, &r, CivId(0), tradition), Err(PolicyRejection::AlreadyAdopted));
    }

    #[test]
    fn later_branches_wait_for_their_era() {
        let r = ruleset();
        let w = world(&r);
        // Find a branch whose era is not the first.
        let late = r
            .policy_branches
            .iter()
            .filter(|(_, b)| r.eras.by_name(&b.era).is_some_and(|e| e.era_number > 0))
            .map(|(_, b)| b.name.clone())
            .next()
            .expect("some branch is gated on a later era");
        let id = r.policies.id(&late).expect("the branch is a policy");
        println!("gated branch: {late}");
        assert!(matches!(is_adoptable(&w, &r, CivId(0), id), Err(PolicyRejection::EraTooEarly { .. })));
    }

    #[test]
    fn culture_accumulates_then_buys_a_policy() {
        let r = ruleset();
        let mut w = world(&r);
        let m = PolicyModifiers::default();
        let tradition = r.policies.id("Tradition").unwrap();

        let e = apply_culture(&mut w, &r, CivId(0), 20, Some(tradition), m).unwrap();
        assert_eq!(e, PolicyEvent::Stored { culture: 20 }, "25 needed");
        assert!(w.civ(CivId(0)).unwrap().adopted_policies.is_empty());

        let e = apply_culture(&mut w, &r, CivId(0), 10, Some(tradition), m).unwrap();
        assert_eq!(e, PolicyEvent::Adopted { policy: tradition, culture_left: 5 });
        assert!(w.civ(CivId(0)).unwrap().adopted_policies.contains(&tradition));
    }

    /// Adopting a policy has to make its uniques live, or the whole layer is
    /// decorative.
    #[test]
    fn adoption_puts_the_policys_uniques_into_force() {
        let r = ruleset();
        let mut w = world(&r);
        let m = PolicyModifiers::default();

        // A branch policy that actually carries uniques.
        let (id, kind) = r
            .policies
            .iter()
            .filter(|(id, p)| {
                p.requires.is_empty()
                    && is_adoptable(&w, &r, CivId(0), *id).is_ok()
                    && p.uniques.iter().any(|u| u.unique_type().is_some())
            })
            .find_map(|(id, p)| p.uniques.iter().find_map(|u| u.unique_type()).map(|t| (id, t)))
            .expect("some adoptable branch carries a unique");

        rebuild_uniques(&mut w, &r, CivId(0));
        let before = w.civ(CivId(0)).unwrap().uniques.has(kind);

        apply_culture(&mut w, &r, CivId(0), 1_000, Some(id), m).unwrap();
        let civ = w.civ(CivId(0)).unwrap();
        assert!(civ.adopted_policies.contains(&id));
        assert!(civ.uniques.has(kind), "the policy's unique should now be in force");
        assert!(!civ.uniques.is_empty());
        println!(
            "{}: {:?} live, index holds {} uniques (before: {before})",
            r.policies[id].name,
            kind,
            civ.uniques.len()
        );
    }

    #[test]
    fn era_rises_with_research() {
        let r = ruleset();
        let mut w = world(&r);
        assert_eq!(era_number(&w, &r, CivId(0)), 0);

        // Research something from a later era and the civ advances.
        let late = r
            .technologies
            .iter()
            .max_by_key(|(_, t)| t.era(&r).and_then(|e| r.eras.get(e)).map_or(0, |e| e.era_number))
            .map(|(id, _)| id)
            .unwrap();
        w.civ_mut(CivId(0)).unwrap().techs.insert(late);
        assert!(era_number(&w, &r, CivId(0)) > 0);
    }

    #[test]
    fn a_civ_with_a_city_pays_the_same_as_one_without_until_the_second() {
        let r = ruleset();
        let mut w = world(&r);
        let m = PolicyModifiers::default();
        w.found_city(City::found("Rome", CivId(0), TileId(30)));
        assert_eq!(policy_cost(0, 1, m), policy_cost(0, 0, m), "the first city is free");
    }
}
