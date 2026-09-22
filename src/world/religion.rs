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
    Fixed::whole(PANTHEON_BASE + taken * PANTHEON_GROWTH)
        .mul(speed)
        .round()
        .max(1) as i32
}

/// What the next great prophet costs.
///
/// `200 + 100·n(n+1)/2` — triangular, so 200, 300, 500, 800, 1200.
pub fn prophet_cost(earned: i32, speed: Fixed) -> i32 {
    let n = earned.max(0);
    let triangular = PROPHET_STEP * n * (n + 1) / 2;
    Fixed::whole(PROPHET_BASE + triangular).mul(speed).round().max(1) as i32
}

/// Beliefs of a kind that nobody has taken.
pub fn available_beliefs(world: &World, r: &Ruleset, kind: &str) -> Vec<Id<Belief>> {
    let taken: std::collections::BTreeSet<_> =
        world.civs.iter().flat_map(|c| c.faith.beliefs.iter().copied()).collect();
    r.beliefs
        .iter()
        .filter(|(id, b)| b.r#type == kind && !taken.contains(id))
        .map(|(id, _)| id)
        .collect()
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
pub fn apply_faith(
    world: &mut World,
    r: &Ruleset,
    civ_id: CivId,
    gained: i32,
    speed: Fixed,
) -> FaithEvent {
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

/// Every unique a civ's beliefs grant, for the index.
pub fn belief_uniques<'a>(r: &'a Ruleset, faith: &Faith) -> Vec<&'a crate::Unique> {
    faith
        .beliefs
        .iter()
        .filter_map(|id| r.beliefs.get(*id))
        .flat_map(|b| b.uniques.iter())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::entities::Civilization;
    use crate::world::grid::TileGrid;

    fn ruleset() -> Ruleset {
        Ruleset::load_dir("../android/assets/jsons/Civ V - Gods & Kings")
            .expect("readable")
            .expect("loads")
    }

    fn world(r: &Ruleset) -> World {
        let g = TileGrid::new(8, 8, r.terrain("Grassland").unwrap());
        let nation = r.nations.ids().next().unwrap();
        World::new(
            g,
            vec![
                Civilization::new(CivId(0), nation, true),
                Civilization::new(CivId(1), nation, true),
            ],
        )
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

        assert!(matches!(
            apply_faith(&mut w, &r, CivId(0), 5, Fixed::ONE),
            FaithEvent::Stored { .. }
        ));
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
}
