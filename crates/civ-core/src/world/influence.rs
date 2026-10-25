// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Cultural influence (Lingua Franca) and shares of world trade (Reserve
//! Currency): docs/CONTENT.md, Victories.
//!
//! **Influence.** Each turn a share of a major civ's new culture reaches
//! every rival it has met, as influence over that rival: half as much at
//! war, more through open borders, a running trade deal and a shared state
//! faith (the rates are `VictoryKnobs`, data in ModOptions.json).
//! A civ is influential over a rival once its accumulated influence there
//! exceeds all the culture that rival has ever made (and [`INFLUENCE_FLOOR`],
//! so a rival that has barely begun is not won over by a village's songs).
//! At the default rate, making more culture than a rival (since meeting it)
//! does it; open borders, trade and a shared faith bring that down to 0.57.
//!
//! **Trade.** A civ's share of world trade is its trade-route income (its
//! cities' connections to its capital) over every major civ's. The last
//! [`TRADE_HISTORY`] turns' shares are kept, so a victory can ask for a share
//! held over any run of turns.
//!
//! Both run only when a victory in the ruleset names them, so the fixtures
//! never carry either.

use crate::ruleset::Ruleset;
use crate::world::grid::CivId;
use crate::world::victory::Milestone;
use crate::world::world::World;
use std::collections::BTreeMap;

/// Turns of trade share kept per civ: the longest run a milestone can ask for.
pub const TRADE_HISTORY: usize = 100;

// The rates and thresholds are data: `ruleset::civgame::VictoryKnobs`, read
// from ModOptions.json, with engine defaults.

#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Influence {
    /// Lifetime culture each civ had when last counted.
    #[cfg_attr(feature = "serde", serde(with = "crate::serialization::map_as_entries"))]
    pub culture_seen: BTreeMap<CivId, i64>,
    /// Accumulated influence, by (from, over).
    #[cfg_attr(feature = "serde", serde(with = "crate::serialization::map_as_entries"))]
    pub over: BTreeMap<(CivId, CivId), i64>,
    /// Each major civ's share of world trade, in tenths of a percent, oldest
    /// first, at most [`TRADE_HISTORY`] turns.
    #[cfg_attr(feature = "serde", serde(with = "crate::serialization::map_as_entries"))]
    pub trade_shares: BTreeMap<CivId, Vec<u16>>,
}

fn milestones(r: &Ruleset) -> impl Iterator<Item = Milestone> + '_ {
    r.victories.values().flat_map(|v| v.milestones.iter()).map(|m| crate::world::victory::parse_milestone(m))
}

pub fn tracks_influence(r: &Ruleset) -> bool {
    milestones(r).any(|m| m == Milestone::InfluentialOverAll)
}

pub fn tracks_trade(r: &Ruleset) -> bool {
    milestones(r).any(|m| matches!(m, Milestone::WorldTradeShare { .. }))
}

/// All the culture a civ has ever made.
pub fn lifetime_culture(world: &World, civ: CivId) -> i64 {
    world.civ(civ).map_or(0, |c| i64::from(c.culture_for_contests))
}

fn majors(world: &World) -> Vec<CivId> {
    world.civs.iter().filter(|c| c.is_major() && c.is_alive()).map(|c| c.id).collect()
}

/// Percent of `from`'s culture that reaches `over` as influence this turn:
/// nothing before meeting, half the base rate at war, else the base rate plus
/// the bonuses (`VictoryKnobs`).
pub fn influence_rate(world: &World, r: &Ruleset, from: CivId, over: CivId) -> i64 {
    let d = &world.diplomacy;
    if from == over || !d.have_met(from, over) {
        return 0;
    }
    let knobs = crate::world::slices::victory_knobs(world, r);
    // War does not stop a culture reaching its enemy, but halves it and
    // closes every channel that carries more.
    if d.at_war(from, over) {
        return knobs.influence_percent / 2;
    }
    let mut rate = knobs.influence_percent;
    if d.may_enter_borders_of(from, over) {
        rate += knobs.influence_bonus_percent;
    }
    if d.deals.iter().any(|deal| (deal.a == from && deal.b == over) || (deal.a == over && deal.b == from)) {
        rate += knobs.influence_bonus_percent;
    }
    let state = |c: CivId| world.civ(c).and_then(|c| c.faith.state);
    if state(from).is_some() && state(from) == state(over) {
        rate += knobs.influence_bonus_percent;
    }
    rate
}

pub fn influence_over(world: &World, from: CivId, over: CivId) -> i64 {
    world.influence.over.get(&(from, over)).copied().unwrap_or(0)
}

/// `from`'s influence over `over` has passed all the culture `over` ever made.
pub fn is_influential(world: &World, r: &Ruleset, from: CivId, over: CivId) -> bool {
    let influence = influence_over(world, from, over);
    from != over && influence >= crate::world::slices::victory_knobs(world, r).influence_floor && influence > lifetime_culture(world, over)
}

/// How far `from` is toward being influential over `over`, in tenths of a
/// percent (1000 and above is there).
pub fn influence_progress(world: &World, r: &Ruleset, from: CivId, over: CivId) -> i64 {
    let need = lifetime_culture(world, over).max(crate::world::slices::victory_knobs(world, r).influence_floor - 1) + 1;
    influence_over(world, from, over).max(0) * 1000 / need.max(1)
}

/// Influential over every other living major civ (and there is at least one).
pub fn influential_over_all(world: &World, r: &Ruleset, civ: CivId) -> bool {
    let rivals: Vec<_> = majors(world).into_iter().filter(|c| *c != civ).collect();
    !rivals.is_empty() && rivals.iter().all(|x| is_influential(world, r, civ, *x))
}

/// A civ's trade-route income this turn, in hundredths of gold (a small
/// town's route earns less than a whole coin).
pub fn trade_volume(world: &World, r: &Ruleset, civ: CivId) -> i64 {
    let connections = crate::world::connections::find(world, r, civ);
    connections
        .trade_routes()
        .map(|city| crate::world::connections::trade_route_stats(world, r, civ, &connections, city).gold)
        .map(|gold| (gold * crate::stats::Fixed::whole(100)).floor().max(0))
        .sum()
}

/// Every major civ's share of world trade right now, in tenths of a percent.
/// All zero while world trade is below its minimum (`worldTradePerMajor`,
/// plus `worldTradePerMajorPerEra` for each era the leading civ has
/// reached), and zero for a civ before `tradeShareFromEra`.
pub fn trade_shares(world: &World, r: &Ruleset) -> BTreeMap<CivId, u16> {
    let knobs = crate::world::slices::victory_knobs(world, r);
    let civs = majors(world);
    let era = |c: CivId| crate::world::policies::era_number(world, r, c);
    let leading = civs.iter().map(|c| era(*c)).max().unwrap_or(0);
    let from_era = knobs.trade_share_from_era.as_deref().and_then(|e| r.eras.by_name(e)).map(|e| e.era_number);
    let volumes: Vec<_> = civs.into_iter().map(|c| (c, trade_volume(world, r, c))).collect();
    let total: i64 = volumes.iter().map(|(_, v)| v).sum();
    let minimum = (knobs.world_trade_per_major + knobs.world_trade_per_major_per_era * i64::from(leading.max(0)))
        * 100
        * volumes.len() as i64;
    let established = total > 0 && total >= minimum;
    volumes
        .into_iter()
        .map(|(c, v)| {
            let counts = established && from_era.is_none_or(|e| era(c) >= e);
            (c, if counts { (v * 1000 / total).clamp(0, 1000) as u16 } else { 0 })
        })
        .collect()
}

/// Held at least `percent` of world trade for each of the last `turns` turns.
pub fn held_trade_share(world: &World, civ: CivId, percent: u32, turns: u32) -> bool {
    let Some(history) = world.influence.trade_shares.get(&civ) else { return false };
    let turns = turns.max(1) as usize;
    history.len() >= turns && history[history.len() - turns..].iter().all(|s| u32::from(*s) >= percent * 10)
}

/// One turn: influence flows from each civ's new culture, and trade shares
/// are recorded.
pub fn tick(world: &mut World, r: &Ruleset) {
    if tracks_influence(r) {
        let civs = majors(world);
        // Culture made before a civ was first counted (an older save, a
        // ruleset switched mid-game) reaches nobody.
        let gained: Vec<(CivId, i64)> = civs
            .iter()
            .map(|c| {
                let now = lifetime_culture(world, *c);
                let seen = world.influence.culture_seen.get(c).copied().unwrap_or(now);
                (*c, (now - seen).max(0))
            })
            .collect();
        let flows: Vec<((CivId, CivId), i64)> = gained
            .iter()
            .flat_map(|(from, culture)| civs.iter().map(move |over| ((*from, *over), *culture)))
            .filter_map(|((from, over), culture)| {
                let rate = influence_rate(world, r, from, over);
                (rate > 0 && culture > 0).then_some(((from, over), culture * rate / 100))
            })
            .collect();
        for (pair, amount) in flows {
            let slot = world.influence.over.entry(pair).or_insert(0);
            *slot = slot.saturating_add(amount);
        }
        for c in civs {
            let now = lifetime_culture(world, c);
            world.influence.culture_seen.insert(c, now);
        }
    }
    if tracks_trade(r) {
        for (civ, share) in trade_shares(world, r) {
            let history = world.influence.trade_shares.entry(civ).or_default();
            history.push(share);
            if history.len() > TRADE_HISTORY {
                history.remove(0);
            }
        }
    }
}
