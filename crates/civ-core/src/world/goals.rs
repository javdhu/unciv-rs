//! What each AI civ is playing to win (docs/AI.md, level 2).
//!
//! A goal is read off the board every turn rather than stored: the victory
//! this civ stands best placed for, against its strongest rival. The board
//! changes slowly, so the goal does too, and a save needs nothing new.
//!
//! - **Conquest** when its army clearly outweighs every rival's, or it
//!   already holds a rival's capital.
//! - **Trade** (Reserve Currency) when it carries a large share of world trade.
//! - **Influence** (Lingua Franca) when its culture is winning rivals over.
//! - **Science** when it leads the tech race, and as the builder's default.
//! - **Score** in the last stretch of the game, unless a conquest is nearly done.
//!
//! Only victories the game has enabled count. Each goal then bends the
//! decisions that make it: how readily to go to war and whether to make peace
//! ([`crate::world::ai::should_declare_war`], [`crate::world::ai::wants_peace`]),
//! what to research ([`beeline`]), how big an army to keep and whether to
//! keep a captured city.

use std::collections::BTreeSet;

use crate::arena::Id;
use crate::ruleset::{Ruleset, Technology};
use crate::stats::Fixed;
use crate::world::grid::CivId;
use crate::world::victory::{parse_milestone, Milestone};
use crate::world::World;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Goal {
    Conquest,
    Trade,
    Influence,
    Science,
    Score,
}

/// The last stretch of the game, as a share of its length, in which a civ
/// plays for score.
const SCORE_STRETCH_PERCENT: u32 = 15;

/// Which goals the game's enabled victories allow.
fn enabled(world: &World, r: &Ruleset) -> BTreeSet<Goal> {
    let mut out = BTreeSet::new();
    for (id, v) in r.victories.iter() {
        if world.endgame.enabled_victories.as_ref().is_some_and(|e| !e.contains(&id)) {
            continue;
        }
        for m in &v.milestones {
            out.insert(match parse_milestone(m) {
                Milestone::CaptureAllCapitals | Milestone::DestroyAllPlayers => Goal::Conquest,
                Milestone::WorldTradeShare { .. } => Goal::Trade,
                Milestone::InfluentialOverAll => Goal::Influence,
                Milestone::SpaceshipParts | Milestone::BuiltBuilding(_) => Goal::Science,
                _ => continue,
            });
        }
    }
    out
}

fn rivals(world: &World, civ: CivId) -> Vec<CivId> {
    world.civs.iter().filter(|c| c.id != civ && c.is_major() && c.is_alive()).map(|c| c.id).collect()
}

/// Original major capitals this civ holds, its own included.
pub fn capitals_held(world: &World, civ: CivId) -> usize {
    world
        .cities
        .iter()
        .filter(|(_, c)| c.original_capital && c.owner == civ && world.civ(c.founder).is_some_and(|f| f.is_major()))
        .count()
}

/// This civ's goal this turn.
pub fn goal(world: &World, r: &Ruleset, civ: CivId) -> Goal {
    let Some(c) = world.civ(civ) else { return Goal::Score };
    let rivals = rivals(world, civ);
    if !c.is_major() || rivals.is_empty() {
        return Goal::Score;
    }
    let allowed = enabled(world, r);
    let held = capitals_held(world, civ);
    let near_conquest = allowed.contains(&Goal::Conquest) && held + 1 >= rivals.len() + 1 && held >= 2;
    let max = world.endgame.max_turns.max(1);
    let left = max.saturating_sub(world.turn);
    if left * 100 <= max * SCORE_STRETCH_PERCENT && !near_conquest {
        return Goal::Score;
    }

    // Each standing is scored so an even footing reads about 50, and a goal
    // outranks the builder's default only when this civ is actually ahead.
    // Even is 1000; nothing against nothing is even too, not zero.
    let per_mille = |mine: i64, theirs: i64| if theirs <= 0 { if mine <= 0 { 1000 } else { 2000 } } else { mine * 1000 / theirs };
    let mut options: Vec<(i64, Goal)> = Vec::new();
    if allowed.contains(&Goal::Conquest) {
        let mine = crate::world::ai::military_strength(world, r, civ);
        let strongest = rivals.iter().map(|o| crate::world::ai::military_strength(world, r, *o)).max();
        let ratio = per_mille(mine.round(), strongest.unwrap_or(Fixed::ZERO).round());
        // Even is 40; half again as strong is 140; each rival capital held adds 60.
        options.push((40 + (ratio - 1000) / 5 + 60 * (held.saturating_sub(1) as i64), Goal::Conquest));
    }
    if allowed.contains(&Goal::Trade) {
        // Tenths of a percent of world trade; an even share among four is 50.
        let share = world.influence.trade_shares.get(&civ).and_then(|h| h.last()).copied().unwrap_or(0);
        options.push((i64::from(share) / 5, Goal::Trade));
    }
    if allowed.contains(&Goal::Influence) {
        let won = rivals.iter().filter(|o| crate::world::influence::is_influential(world, r, civ, **o)).count() as i64;
        let reach: i64 = rivals
            .iter()
            .map(|o| {
                let over = crate::world::influence::influence_over(world, civ, *o);
                let theirs = crate::world::influence::lifetime_culture(world, *o).max(1);
                (over * 100 / theirs).min(100)
            })
            .min()
            .unwrap_or(0);
        options.push((reach / 2 + 100 * won / rivals.len() as i64, Goal::Influence));
    }
    if allowed.contains(&Goal::Science) {
        let mine = c.techs.len() as i64;
        let best = rivals.iter().filter_map(|o| world.civ(*o)).map(|o| o.techs.len() as i64).max().unwrap_or(0);
        // Even is 50, and it stays the default: a builder civ is a science civ.
        options.push((50 + (per_mille(mine, best) - 1000) / 4, Goal::Science));
    }
    options.into_iter().max_by(|a, b| a.0.cmp(&b.0).then(b.1.cmp(&a.1))).map_or(Goal::Score, |(_, g)| g)
}

/// Every tech `target` needs that this civ does not have, `target` included.
fn still_needed(r: &Ruleset, known: &BTreeSet<Id<Technology>>, target: Id<Technology>) -> BTreeSet<Id<Technology>> {
    let mut out = BTreeSet::new();
    let mut stack = vec![target];
    while let Some(t) = stack.pop() {
        if known.contains(&t) || !out.insert(t) {
            continue;
        }
        if let Some(tech) = r.technologies.get(t) {
            stack.extend(tech.prerequisites.iter().filter_map(|p| r.technologies.id(p)));
        }
    }
    out
}

/// The techs this civ's goal leads through: the route to the next clearly
/// stronger unit for a conqueror, to the victory's buildings for a
/// scientist. Empty when the goal has no route, or the route is done.
pub fn beeline(world: &World, r: &Ruleset, civ: CivId, goal: Goal) -> BTreeSet<Id<Technology>> {
    let Some(c) = world.civ(civ) else { return BTreeSet::new() };
    let known: BTreeSet<Id<Technology>> = c.techs.iter().copied().collect();
    let has = |tech: &Option<String>| tech.as_ref().is_none_or(|t| r.technologies.id(t).is_some_and(|id| known.contains(&id)));
    let targets: Vec<Id<Technology>> = match goal {
        Goal::Conquest => {
            let strength = |b: &crate::ruleset::BaseUnit| b.strength.unwrap_or(0).max(b.ranged_strength.unwrap_or(0));
            let best = r.units.values().filter(|b| has(&b.required_tech)).map(strength).max().unwrap_or(0);
            r.units
                .values()
                .filter(|b| !has(&b.required_tech) && strength(b) * 4 > best * 5)
                .filter_map(|b| b.required_tech.as_ref().and_then(|t| r.technologies.id(t)))
                .collect()
        }
        Goal::Science => {
            let (buildings, parts) = crate::world::victory::victory_constructions(r);
            let mut techs: Vec<_> = buildings
                .iter()
                .filter_map(|b| r.buildings.get(*b))
                .filter_map(|b| b.required_tech.as_ref().and_then(|t| r.technologies.id(t)))
                .collect();
            techs.extend(
                parts
                    .iter()
                    .filter_map(|p| r.units.id(p).and_then(|u| r.units.get(u)))
                    .filter_map(|u| u.required_tech.as_ref().and_then(|t| r.technologies.id(t))),
            );
            techs
        }
        Goal::Trade | Goal::Influence | Goal::Score => Vec::new(),
    };
    // The nearest target: fewest techs still to research, then lowest id.
    targets
        .into_iter()
        .map(|t| still_needed(r, &known, t))
        .filter(|route| !route.is_empty())
        .min_by(|a, b| a.len().cmp(&b.len()).then(a.iter().next().cmp(&b.iter().next())))
        .unwrap_or_default()
}
