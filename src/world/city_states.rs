//! City-states: the civs nobody plays, that everybody courts.
//!
//! A city-state is an ordinary [`Civilization`] with a
//! [`city_state_type`](crate::world::entities::Civilization::city_state_type)
//! set. It never expands, never wins, and exists to be a *source* — of
//! culture, food, happiness or units, depending on its type — that the major
//! civs compete over. Modelling it as a civ rather than as a kind of city is
//! what lets it hold territory, fight, be conquered, and appear in diplomacy
//! without any of those systems needing a special case.
//!
//! # Influence is directed, and the ally is derived
//!
//! Influence is not a relationship between two civs; it is a city-state's
//! opinion of one major civ, and there is no reciprocal value. So it is stored
//! directed, keyed `(city_state, major)`, next to the attitudes rather than
//! among the symmetric pacts.
//!
//! Who a city-state's *ally* is, on the other hand, is not stored at all. It
//! is whoever holds the most influence above the threshold. Unciv keeps an
//! `allyCiv` field, and a stored answer to a derived question is a thing that
//! can be wrong: it has to be updated when influence rises, when it decays,
//! when a rival overtakes you, when the ally is destroyed, and when the ally
//! declares war. Deriving it cannot fall out of step with the numbers it comes
//! from.

use crate::arena::Id;
use crate::rng::Rng;
use crate::ruleset::{CityStateType, Ruleset};
use crate::stats::{Fixed, Precise};
use crate::unique::Unique;
use crate::world::grid::CivId;
use crate::world::world::World;

/// Influence at or above which a major civ is the city-state's ally — if
/// nobody else has more.
pub const ALLY_THRESHOLD: Fixed = Fixed::permille(60_000);
/// Influence at or above which it counts you a friend.
pub const FRIEND_THRESHOLD: Fixed = Fixed::permille(30_000);
/// Below this it will not deal with you at all.
pub const UNFORGIVABLE_THRESHOLD: Fixed = Fixed::permille(-30_000);
/// Influence a protector adds to where the relationship settles.
pub const PROTECTOR_RESTING_BONUS: Fixed = Fixed::permille(10_000);

/// How a city-state sees one major civ.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Relationship {
    Unforgivable,
    Enemy,
    Neutral,
    Friend,
    Ally,
}

/// What a city-state thinks of a major civ.
///
/// `Ally` needs more than influence: only one civ can be the ally, so this
/// asks who is actually in front. A second civ at 70 influence is a Friend,
/// not a co-ally.
pub fn relationship(world: &World, cs: CivId, major: CivId) -> Relationship {
    if world.diplomacy.at_war(cs, major) {
        return Relationship::Unforgivable;
    }
    let influence = world.diplomacy.influence(cs, major);
    if influence <= UNFORGIVABLE_THRESHOLD {
        Relationship::Unforgivable
    } else if influence < Fixed::ZERO {
        Relationship::Enemy
    } else if influence >= ALLY_THRESHOLD && ally_of(world, cs) == Some(major) {
        Relationship::Ally
    } else if influence >= FRIEND_THRESHOLD {
        Relationship::Friend
    } else {
        Relationship::Neutral
    }
}

/// Whoever holds the most influence over this city-state, if it is enough.
///
/// Ties break on the lower civ id — not because that is fair, but because two
/// civs at exactly 60 has to resolve the same way on every machine.
pub fn ally_of(world: &World, cs: CivId) -> Option<CivId> {
    world
        .diplomacy
        .influencers(cs)
        .into_iter()
        .filter(|(major, _)| !world.diplomacy.at_war(cs, *major))
        .filter(|(major, _)| world.civ(*major).is_some_and(|c| c.is_major() && c.is_alive()))
        .filter(|(_, v)| *v >= ALLY_THRESHOLD)
        .max_by(|a, b| a.1.cmp(&b.1).then(std::cmp::Reverse(a.0 .0).cmp(&std::cmp::Reverse(b.0 .0))))
        .map(|(major, _)| major)
}

/// Every city-state still standing, in id order.
pub fn city_states(world: &World) -> Vec<CivId> {
    world.civs.iter().filter(|c| c.is_city_state() && c.is_alive()).map(|c| c.id).collect()
}

/// Where this relationship settles if left alone.
///
/// Influence always slides back here — down from a gift, up from a war — which
/// is what makes courting a city-state an ongoing cost rather than a purchase.
pub fn resting_point(world: &World, cs: CivId, major: CivId) -> Fixed {
    let mut point = Fixed::ZERO;
    if let Some(c) = world.civ(major) {
        for u in c.uniques.matching(crate::UniqueType::CityStateRestingPoint) {
            if let Some(v) = u.params.first().and_then(|p| Fixed::parse(p)) {
                point += v;
            }
        }
    }
    if world.diplomacy.is_protector(major, cs) {
        point += PROTECTOR_RESTING_BONUS;
    }
    point
}

/// One turn of drift toward the resting point.
///
/// A point a turn, modified — slow enough that an alliance bought at 60 lasts
/// thirty turns, which is the whole design of the mechanic: influence is rent,
/// not a purchase.
pub fn drift(world: &World, cs: CivId, major: CivId) -> Fixed {
    let resting = resting_point(world, cs, major);
    let influence = world.diplomacy.influence(cs, major);
    if influence == resting {
        return Fixed::ZERO;
    }
    let step = Fixed::ONE;
    if influence > resting {
        let mut percent = Fixed::ZERO;
        if let Some(c) = world.civ(major) {
            for u in c.uniques.matching(crate::UniqueType::CityStateInfluenceDegradation) {
                if let Some(v) = u.params.first().and_then(|p| Fixed::parse(p)) {
                    percent += v;
                }
            }
        }
        let scaled =
            (step * (Fixed::whole(100) + percent)).checked_div(Fixed::whole(100)).unwrap_or(step).max(Fixed::ZERO);
        -(scaled.min(influence - resting))
    } else {
        let mut percent = Fixed::ZERO;
        if world.civ(major).is_some_and(|c| c.uniques.has(crate::UniqueType::CityStateInfluenceRecoversTwiceNormalRate))
        {
            percent += Fixed::whole(100);
        }
        let scaled =
            (step * (Fixed::whole(100) + percent)).checked_div(Fixed::whole(100)).unwrap_or(step).max(Fixed::ZERO);
        scaled.min(resting - influence)
    }
}

/// A relationship that changed rank this turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RelationshipChange {
    pub city_state: CivId,
    pub major: CivId,
    pub from: Relationship,
    pub to: Relationship,
}

/// Every standing city-state's relationship with every major, in id order.
///
/// Taken at the start of a turn and compared at the end with
/// [`changes_since`]. Decay is only one of the things that moves a rank: a war
/// declared turns a Friend `Unforgivable` on the spot, a quest paid out makes
/// one, and a peace treaty hands back whatever the influence says. Watching
/// only the decay step misses all of those.
pub fn standings(world: &World) -> Vec<(CivId, CivId, Relationship)> {
    let majors: Vec<CivId> = world.civs.iter().filter(|c| c.is_major()).map(|c| c.id).collect();
    city_states(world)
        .into_iter()
        .flat_map(|cs| majors.iter().map(move |&major| (cs, major)))
        .map(|(cs, major)| (cs, major, relationship(world, cs, major)))
        .collect()
}

/// Every rank that differs from `before`.
///
/// A city-state that fell during the turn has no rank left to report, so it
/// is skipped rather than reported as a change to nothing.
pub fn changes_since(world: &World, before: &[(CivId, CivId, Relationship)]) -> Vec<RelationshipChange> {
    let standing = city_states(world);
    before
        .iter()
        .filter(|(cs, ..)| standing.contains(cs))
        .filter_map(|&(cs, major, from)| {
            let to = relationship(world, cs, major);
            (to != from).then_some(RelationshipChange { city_state: cs, major, from, to })
        })
        .collect()
}

/// Slide every relationship one step toward where it belongs.
///
/// Planned in full before anything is written, because an ally is decided by
/// comparing influences — applying one civ's decay before reading another's
/// would let the order of iteration decide who the ally is.
pub fn tick(world: &mut World) -> Vec<RelationshipChange> {
    let states = city_states(world);
    let majors: Vec<CivId> = world.civs.iter().filter(|c| c.is_major()).map(|c| c.id).collect();

    let mut before = Vec::new();
    let mut steps = Vec::new();
    for &cs in &states {
        for &major in &majors {
            before.push((cs, major, relationship(world, cs, major)));
            let step = drift(world, cs, major);
            if !step.is_zero() {
                steps.push((cs, major, step));
            }
        }
    }
    for (cs, major, step) in steps {
        world.diplomacy.add_influence(cs, major, step);
    }

    before
        .into_iter()
        .filter_map(|(cs, major, from)| {
            let to = relationship(world, cs, major);
            (to != from).then_some(RelationshipChange { city_state: cs, major, from, to })
        })
        .collect()
}

/// Influence bought by a gift of gold.
///
/// `amount^1.01 / 9.8`, falling to a third of that by the end of the game, then
/// rounded *down* to a multiple of five with a floor of five. The exponent is
/// barely above one — it exists to make very large gifts slightly better than
/// proportional, and it is the reason this needs a real fractional power
/// rather than a multiplication.
pub fn influence_from_gift(
    world: &World,
    gold: i32,
    game_length: i32,
    gold_gift_modifier: Fixed,
    donor: CivId,
) -> Fixed {
    if gold <= 0 {
        return Fixed::ZERO;
    }
    let base = Fixed::whole(gold)
        .pow(Fixed::permille(1_010))
        .unwrap_or(Fixed::whole(gold))
        .checked_div(Fixed::permille(9_800))
        .unwrap_or(Fixed::ZERO);

    // Gifts are worth less as the game goes on: a third as much by the end.
    let progress = if game_length > 0 {
        Fixed::whole(world.turn as i32).checked_div(Fixed::whole(game_length)).unwrap_or(Fixed::ONE).min(Fixed::ONE)
    } else {
        Fixed::ONE
    };
    let decay = Fixed::ONE - (Fixed::permille(667) * progress);

    let mut gained = ((Precise::from_fixed(base) * Precise::from_fixed(decay))
        * Precise::from_fixed(if gold_gift_modifier.is_zero() { Fixed::ONE } else { gold_gift_modifier }))
    .to_fixed();

    if let Some(c) = world.civ(donor) {
        for u in c.uniques.matching(crate::UniqueType::CityStateGoldGiftsProvideMoreInfluence) {
            if let Some(pct) = u.params.first().and_then(|p| Fixed::parse(p)) {
                gained = (gained * (Fixed::whole(100) + pct)).checked_div(Fixed::whole(100)).unwrap_or(gained);
            }
        }
    }

    // Down to a multiple of five, never below five. Civ does this so the
    // number on the button is a round one.
    let fives = (gained.floor() / 5).max(1);
    Fixed::whole((fives * 5) as i32)
}

/// Hand over gold and gain the influence it buys.
///
/// Returns what was gained, or `None` if the gift could not be made — you
/// cannot gift what you do not have, and you cannot buy the favour of someone
/// you are at war with.
pub fn gift_gold(
    world: &mut World,
    major: CivId,
    cs: CivId,
    gold: i32,
    game_length: i32,
    gold_gift_modifier: Fixed,
) -> Option<Fixed> {
    if gold <= 0 || world.diplomacy.at_war(cs, major) {
        return None;
    }
    if !world.civ(cs).is_some_and(|c| c.is_city_state() && c.is_alive()) {
        return None;
    }
    if world.civ(major).map_or(0, |c| c.gold) < gold {
        return None;
    }
    let gained = influence_from_gift(world, gold, game_length, gold_gift_modifier, major);
    if let Some(c) = world.civ_mut(major) {
        c.gold -= gold;
    }
    if let Some(c) = world.civ_mut(cs) {
        c.gold += gold;
    }
    world.diplomacy.add_influence(cs, major, gained);
    Some(gained)
}

/// The uniques a major civ is currently owed by the city-states it has
/// befriended.
///
/// A friend grants the friend list; an ally grants the ally list *instead*,
/// not as well — the ally uniques already restate the friend ones at a higher
/// value, and adding both would pay twice.
pub fn bonus_uniques(world: &World, r: &Ruleset, major: CivId) -> Vec<Unique> {
    let mut out = Vec::new();
    for cs in city_states(world) {
        let level = relationship(world, cs, major);
        let Some(kind) = world.civ(cs).and_then(|c| c.city_state_type) else { continue };
        let Some(kind) = r.city_state_types.get(kind) else { continue };
        let source = match level {
            Relationship::Ally => &kind.ally_bonus_uniques,
            Relationship::Friend => &kind.friend_bonus_uniques,
            _ => continue,
        };
        out.extend(source.iter().map(|text| Unique::parse(text)));
    }
    out
}

/// Flat yields a civ is owed each turn from things that are not tiles.
///
/// City-state friendship, and anything else that grants `[+3 Culture]` or
/// `[+2 Food] [in capital]` at the civ level. Nothing read these before: the
/// uniques reached the civ's index and sat there, which meant every Cultured
/// city-state in the game paid out nothing at all.
///
/// `[stats]` lands once on the empire. `[stats] [cityFilter]` lands once per
/// city that matches — "in all cities" really does mean each of them.
pub fn civ_wide_yields(world: &World, r: &Ruleset, civ: CivId) -> crate::stats::Stats {
    use crate::stats::Stats;
    let Some(c) = world.civ(civ) else { return Stats::ZERO };
    let mut total = Stats::ZERO;

    let ctx = crate::world::conditionals::GameContext::new(r).with_world(world).for_civ(civ);
    let holds = |u: &Unique| crate::world::conditionals::applies(u, &ctx) == crate::world::conditionals::Verdict::True;

    for u in c.uniques.matching(crate::UniqueType::Stats).filter(|u| holds(u)) {
        if let Some(s) = u.params.first().and_then(|p| Stats::parse(p)) {
            total += s;
        }
    }

    let capital = crate::world::connections::capital_of(world, r, civ);
    for u in c.uniques.matching(crate::UniqueType::StatsPerCity).filter(|u| holds(u)) {
        let (Some(stats), Some(filter)) = (u.params.first().and_then(|p| Stats::parse(p)), u.params.get(1)) else {
            continue;
        };
        for city_id in &c.cities {
            let Some(city) = world.cities.get(*city_id) else { continue };
            // "in capital" is the one filter that is about which city this is
            // rather than whose it is, so it is answered here.
            let matched = if filter.contains("capital") {
                Some(*city_id) == capital
            } else {
                crate::world::filters::city_matches(world, r, city, Some(city), filter)
                    == crate::world::conditionals::Verdict::True
            };
            if matched {
                total += stats;
            }
        }
    }
    total
}

// ---------------------------------------------------------------------------
// Elections and coups — the two spy actions that are really city-state systems
// ---------------------------------------------------------------------------

/// Turns between elections in a city-state.
pub const ELECTION_TURNS: u32 = 15;
/// The election's own candidate: the outcome where nobody's bribe carried.
pub const NEUTRAL_VOTES: i32 = 20;
/// Influence the winner of a rigged election gains.
pub const ELECTION_WIN_INFLUENCE: Fixed = Fixed::permille(20_000);
/// And what everyone else loses by it.
pub const ELECTION_LOSS_INFLUENCE: Fixed = Fixed::permille(-5_000);

/// Influence a successful coup hands the plotter when there was no ally to
/// depose — you cannot inherit a position nobody held.
pub const COUP_UNOPPOSED_INFLUENCE: Fixed = Fixed::permille(80_000);
/// What the deposed ally loses.
pub const COUP_ALLY_PENALTY: Fixed = Fixed::permille(-20_000);
/// And what every other onlooker loses, for having watched it happen.
pub const COUP_ONLOOKER_PENALTY: Fixed = Fixed::permille(-10_000);
/// What a failed coup costs the plotter.
pub const COUP_FAILURE_PENALTY: Fixed = Fixed::permille(-20_000);
/// A coup is never a certainty, however far ahead you are.
pub const COUP_MAX_CHANCE: i32 = 85;

/// What an election did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ElectionResult {
    /// A major civ's spy carried it.
    Rigged { city_state: CivId, winner: CivId },
    /// Nobody's did, and everyone who tried paid for trying.
    Neutral { city_state: CivId },
}

/// Hold the elections that are due this turn.
///
/// Every spy stationed in a city-state and set to rig buys votes in proportion
/// to the influence its civ already holds and to its own skill. The election
/// itself is a candidate too, weighted at twenty — which is what makes rigging
/// a gamble rather than a purchase, and what makes a *second* rigger worth
/// having even when you are behind.
///
/// Picked by weighted draw from a seed made of the turn and the city-state, so
/// the same game elects the same government everywhere.
pub fn hold_elections(world: &mut World, r: &Ruleset) -> Vec<ElectionResult> {
    let mut results = Vec::new();
    if world.turn == 0 || !world.turn.is_multiple_of(ELECTION_TURNS) {
        return results;
    }

    for cs in city_states(world) {
        let Some(capital) = crate::world::connections::capital_of(world, r, cs)
            .or_else(|| world.civ(cs).and_then(|c| c.cities.first().copied()))
        else {
            continue;
        };

        // Who is rigging, in civ-id order so the draw is reproducible.
        let mut riggers: Vec<(CivId, i32)> = Vec::new();
        for major in world.civ_ids().collect::<Vec<_>>() {
            if !world.civ(major).is_some_and(|c| c.is_major() && c.is_alive()) {
                continue;
            }
            let Some(spy) = world.civ(major).and_then(|c| {
                c.espionage.spies.iter().find(|s| {
                    s.station == Some(capital) && s.action == crate::world::espionage::SpyAction::RiggingElections
                })
            }) else {
                continue;
            };
            // Half the influence already held, plus the spy's own skill. A civ
            // the city-state already likes finds rigging easy, which is how
            // an ally stays an ally.
            let votes = world.diplomacy.influence(cs, major).floor() as i32 / 2 + spy.skill();
            riggers.push((major, votes.max(0)));
        }
        if riggers.is_empty() {
            continue;
        }

        let total: i32 = riggers.iter().map(|(_, v)| v).sum::<i32>() + NEUTRAL_VOTES;
        let mut rng = Rng::from_parts(&[0xE1EC, world.turn as u64, cs.0 as u64]);
        let mut draw = rng.range(0, total as i64 - 1) as i32;
        let mut winner = None;
        for (civ, votes) in &riggers {
            if draw < *votes {
                winner = Some(*civ);
                break;
            }
            draw -= *votes;
        }

        match winner {
            Some(w) => {
                // Everyone the city-state knows is moved by the result, not
                // only those who took part — a rigged election is a public fact.
                for major in world.civ_ids().collect::<Vec<_>>() {
                    if !world.civ(major).is_some_and(|c| c.is_major() && c.is_alive()) {
                        continue;
                    }
                    if !world.diplomacy.have_met(cs, major) {
                        continue;
                    }
                    let delta = if major == w { ELECTION_WIN_INFLUENCE } else { ELECTION_LOSS_INFLUENCE };
                    world.diplomacy.add_influence(cs, major, delta);
                }
                results.push(ElectionResult::Rigged { city_state: cs, winner: w });
            }
            None => {
                for (civ, _) in &riggers {
                    world.diplomacy.add_influence(cs, *civ, ELECTION_LOSS_INFLUENCE);
                }
                results.push(ElectionResult::Neutral { city_state: cs });
            }
        }
    }
    results
}

/// Chance a coup succeeds, as a percentage.
///
/// Even odds, less half the influence gap to the sitting ally, plus half the
/// skill gap between the plotting spy and whatever guards the place. Capped at
/// 85: however far ahead you are, a coup can fail.
pub fn coup_chance(world: &World, r: &Ruleset, plotter: CivId, cs: CivId) -> i32 {
    let ally = ally_of(world, cs);
    // With no ally there is no incumbent to beat, so the bar is the ally
    // threshold itself.
    let ally_influence = ally.map_or(ALLY_THRESHOLD, |a| world.diplomacy.influence(cs, a));
    let gap = (ally_influence - world.diplomacy.influence(cs, plotter)).floor() as i32;

    let Some(capital) = crate::world::connections::capital_of(world, r, cs)
        .or_else(|| world.civ(cs).and_then(|c| c.cities.first().copied()))
    else {
        return 0;
    };
    let mine = world
        .civ(plotter)
        .and_then(|c| c.espionage.spies.iter().find(|s| s.station == Some(capital)))
        .map_or(0, |s| s.skill());
    // The incumbent's own spy defends what it bought.
    let theirs = ally
        .and_then(|a| world.civ(a))
        .and_then(|c| c.espionage.spies.iter().find(|s| s.station == Some(capital)))
        .map_or(0, |s| s.skill());

    (50 - gap / 2 + (mine - theirs) / 2).clamp(0, COUP_MAX_CHANCE)
}

/// Can this civ even attempt a coup here?
///
/// It needs a spy in place, and a reason: you do not depose yourself.
pub fn can_coup(world: &World, r: &Ruleset, plotter: CivId, cs: CivId) -> bool {
    if !world.civ(cs).is_some_and(|c| c.is_city_state() && c.is_alive()) {
        return false;
    }
    if ally_of(world, cs) == Some(plotter) {
        return false;
    }
    let Some(capital) = crate::world::connections::capital_of(world, r, cs)
        .or_else(|| world.civ(cs).and_then(|c| c.cities.first().copied()))
    else {
        return false;
    };
    world
        .civ(plotter)
        .is_some_and(|c| c.espionage.spies.iter().any(|s| s.station == Some(capital) && s.is_established()))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CoupResult {
    /// The plotter inherits the deposed ally's standing.
    Succeeded { city_state: CivId, plotter: CivId, deposed: Option<CivId> },
    /// The spy is dead and the plotter is worse off than before.
    Failed { city_state: CivId, plotter: CivId },
    /// No spy in place, or nothing to overthrow.
    Impossible,
}

/// Try to overthrow a city-state's government.
///
/// Succeed and you take the deposed ally's influence outright — not a gift of
/// gold's worth, the whole position. Fail and the spy is dead and you are
/// twenty worse off than when you started. That asymmetry is the point: a coup
/// is what you do when you cannot outbid someone, and it is supposed to be
/// frightening.
pub fn attempt_coup(world: &mut World, r: &Ruleset, plotter: CivId, cs: CivId) -> CoupResult {
    if !can_coup(world, r, plotter, cs) {
        return CoupResult::Impossible;
    }
    let chance = coup_chance(world, r, plotter, cs);
    let mut rng = Rng::from_parts(&[0xC0_11, world.turn as u64, cs.0 as u64, plotter.0 as u64]);
    let roll = rng.range(0, 99) as i32;

    let ally = ally_of(world, cs);
    let capital = crate::world::connections::capital_of(world, r, cs)
        .or_else(|| world.civ(cs).and_then(|c| c.cities.first().copied()));

    if roll < chance {
        // The plotter inherits exactly what the incumbent had.
        let inherited = ally.map(|a| world.diplomacy.influence(cs, a)).unwrap_or(COUP_UNOPPOSED_INFLUENCE);
        world.diplomacy.set_influence(cs, plotter, inherited);
        if let Some(a) = ally {
            world.diplomacy.add_influence(cs, a, COUP_ALLY_PENALTY);
        }
        for other in world.civ_ids().collect::<Vec<_>>() {
            if other == plotter || Some(other) == ally {
                continue;
            }
            if !world.civ(other).is_some_and(|c| c.is_major() && c.is_alive()) {
                continue;
            }
            if world.diplomacy.have_met(cs, other) {
                world.diplomacy.add_influence(cs, other, COUP_ONLOOKER_PENALTY);
            }
        }
        // The spy stays and goes back to buying votes for its new government.
        if let (Some(capital), Some(c)) = (capital, world.civ_mut(plotter)) {
            if let Some(s) = c.espionage.spies.iter_mut().find(|s| s.station == Some(capital)) {
                s.action = crate::world::espionage::SpyAction::RiggingElections;
            }
        }
        CoupResult::Succeeded { city_state: cs, plotter, deposed: ally }
    } else {
        world.diplomacy.add_influence(cs, plotter, COUP_FAILURE_PENALTY);
        if let (Some(capital), Some(c)) = (capital, world.civ_mut(plotter)) {
            if let Some(s) = c.espionage.spies.iter_mut().find(|s| s.station == Some(capital)) {
                *s = crate::world::espionage::Spy::new();
            }
        }
        // Whoever was guarding the place learns from it.
        if let Some(a) = ally {
            if let (Some(capital), Some(c)) = (capital, world.civ_mut(a)) {
                if let Some(s) = c.espionage.spies.iter_mut().find(|s| s.station == Some(capital)) {
                    s.rank = s.rank.saturating_add(1).min(3);
                }
            }
        }
        CoupResult::Failed { city_state: cs, plotter }
    }
}

/// A city-state's type, by name, for setting one up.
pub fn type_named(r: &Ruleset, name: &str) -> Option<Id<CityStateType>> {
    r.city_state_types.id(name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::entities::Civilization;
    use crate::world::grid::TileGrid;

    fn ruleset() -> Ruleset {
        Ruleset::load_dir("../android/assets/jsons/Civ V - Gods & Kings").expect("readable").expect("loads")
    }

    /// Two major civs and one city-state.
    fn world(r: &Ruleset) -> World {
        let g = TileGrid::new(12, 12, r.terrain("Grassland").unwrap());
        let nation = r.nations.ids().next().unwrap();
        let mut civs = vec![
            Civilization::new(CivId(0), nation, true),
            Civilization::new(CivId(1), nation, true),
            Civilization::new(CivId(2), nation, true),
        ];
        civs[2].city_state_type = Some(
            type_named(r, "Cultured")
                .or_else(|| r.city_state_types.ids().next())
                .expect("the ruleset has city-state types"),
        );
        let mut w = World::new(g, civs);
        // A city-state is its one city; without it the civ counts as dead.
        for (i, (col, row)) in [(2u16, 2u16), (8, 2), (5, 8)].into_iter().enumerate() {
            let t = w.grid.id_at(col, row).unwrap();
            w.found_city(crate::world::city::City::found(format!("City{i}"), CivId(i as u8), t));
        }
        w.diplomacy.meet(CivId(0), CivId(2));
        w.diplomacy.meet(CivId(1), CivId(2));
        w
    }

    const CS: CivId = CivId(2);

    #[test]
    fn a_city_state_is_a_civ_that_is_not_playing_to_win() {
        let r = ruleset();
        let w = world(&r);
        assert!(w.civ(CS).unwrap().is_city_state());
        assert!(!w.civ(CS).unwrap().is_major());
        assert!(w.civ(CivId(0)).unwrap().is_major());
        assert_eq!(city_states(&w), vec![CS]);
    }

    #[test]
    fn influence_buys_rank_at_the_thresholds() {
        let r = ruleset();
        let mut w = world(&r);
        assert_eq!(relationship(&w, CS, CivId(0)), Relationship::Neutral);

        w.diplomacy.set_influence(CS, CivId(0), Fixed::whole(30));
        assert_eq!(relationship(&w, CS, CivId(0)), Relationship::Friend);

        w.diplomacy.set_influence(CS, CivId(0), Fixed::whole(60));
        assert_eq!(relationship(&w, CS, CivId(0)), Relationship::Ally);

        w.diplomacy.set_influence(CS, CivId(0), Fixed::whole(-1));
        assert_eq!(relationship(&w, CS, CivId(0)), Relationship::Enemy);

        w.diplomacy.set_influence(CS, CivId(0), Fixed::whole(-30));
        assert_eq!(relationship(&w, CS, CivId(0)), Relationship::Unforgivable);
    }

    #[test]
    fn only_one_civ_can_be_the_ally() {
        let r = ruleset();
        let mut w = world(&r);
        w.diplomacy.set_influence(CS, CivId(0), Fixed::whole(70));
        w.diplomacy.set_influence(CS, CivId(1), Fixed::whole(65));

        assert_eq!(ally_of(&w, CS), Some(CivId(0)));
        assert_eq!(relationship(&w, CS, CivId(0)), Relationship::Ally);
        assert_eq!(
            relationship(&w, CS, CivId(1)),
            Relationship::Friend,
            "second place at 65 influence is a friend, not a co-ally"
        );

        // Overtake and the title moves with the number, because it was never
        // stored anywhere it could get stuck.
        w.diplomacy.set_influence(CS, CivId(1), Fixed::whole(80));
        assert_eq!(ally_of(&w, CS), Some(CivId(1)));
        assert_eq!(relationship(&w, CS, CivId(0)), Relationship::Friend);
    }

    #[test]
    fn a_tie_for_ally_resolves_the_same_way_every_time() {
        let r = ruleset();
        let mut a = world(&r);
        a.diplomacy.set_influence(CS, CivId(0), Fixed::whole(60));
        a.diplomacy.set_influence(CS, CivId(1), Fixed::whole(60));
        let mut b = world(&r);
        b.diplomacy.set_influence(CS, CivId(1), Fixed::whole(60));
        b.diplomacy.set_influence(CS, CivId(0), Fixed::whole(60));
        assert_eq!(ally_of(&a, CS), ally_of(&b, CS));
        assert_eq!(ally_of(&a, CS), Some(CivId(0)));
    }

    #[test]
    fn war_ends_the_friendship_whatever_the_number_says() {
        let r = ruleset();
        let mut w = world(&r);
        w.diplomacy.set_influence(CS, CivId(0), Fixed::whole(90));
        assert_eq!(relationship(&w, CS, CivId(0)), Relationship::Ally);

        w.diplomacy.declare_war(CivId(0), CS);
        assert_eq!(relationship(&w, CS, CivId(0)), Relationship::Unforgivable);
        assert_eq!(ally_of(&w, CS), None, "you are not allied to what you are invading");

        // And the influence is still there underneath, so peace restores it.
        w.diplomacy.make_peace(CivId(0), CS);
        assert_eq!(relationship(&w, CS, CivId(0)), Relationship::Ally);
    }

    #[test]
    fn a_war_declared_is_a_rank_change_even_though_nothing_decayed() {
        let r = ruleset();
        let mut w = world(&r);
        w.diplomacy.set_influence(CS, CivId(0), Fixed::whole(40));
        let before = standings(&w);

        w.diplomacy.declare_war(CivId(0), CS);
        assert_eq!(
            changes_since(&w, &before),
            vec![RelationshipChange {
                city_state: CS,
                major: CivId(0),
                from: Relationship::Friend,
                to: Relationship::Unforgivable,
            }],
            "the decay step saw Unforgivable on both sides of it, so only a turn-long view catches this"
        );
    }

    #[test]
    fn favour_slides_back_to_where_it_belongs() {
        let r = ruleset();
        let mut w = world(&r);
        w.diplomacy.set_influence(CS, CivId(0), Fixed::whole(60));

        // A point a turn, so an alliance bought at sixty lasts sixty turns
        // and not a turn longer. Influence is rent, not a purchase.
        for _ in 0..59 {
            tick(&mut w);
        }
        assert_eq!(w.diplomacy.influence(CS, CivId(0)), Fixed::ONE, "still just about ours");
        tick(&mut w);
        assert_eq!(w.diplomacy.influence(CS, CivId(0)), Fixed::ZERO);

        // And it comes back up from below, too.
        w.diplomacy.set_influence(CS, CivId(0), Fixed::whole(-20));
        for _ in 0..30 {
            tick(&mut w);
        }
        assert_eq!(w.diplomacy.influence(CS, CivId(0)), Fixed::ZERO);
    }

    #[test]
    fn it_never_overshoots_the_resting_point() {
        let r = ruleset();
        let mut w = world(&r);
        w.diplomacy.set_influence(CS, CivId(0), Fixed::permille(500));
        tick(&mut w);
        assert_eq!(
            w.diplomacy.influence(CS, CivId(0)),
            Fixed::ZERO,
            "half a point from home, it stops at home rather than passing it"
        );
    }

    #[test]
    fn a_protector_is_owed_a_better_resting_point() {
        let r = ruleset();
        let mut w = world(&r);
        assert_eq!(resting_point(&w, CS, CivId(0)), Fixed::ZERO);
        w.diplomacy.set_protector(CivId(0), CS, true);
        assert_eq!(resting_point(&w, CS, CivId(0)), PROTECTOR_RESTING_BONUS);

        // Which means influence settles there rather than at zero.
        for _ in 0..40 {
            tick(&mut w);
        }
        assert_eq!(w.diplomacy.influence(CS, CivId(0)), PROTECTOR_RESTING_BONUS);
    }

    #[test]
    fn protection_is_one_sided_and_war_ends_it() {
        let r = ruleset();
        let mut w = world(&r);
        w.diplomacy.set_protector(CivId(0), CS, true);
        assert!(w.diplomacy.is_protector(CivId(0), CS));
        assert!(!w.diplomacy.is_protector(CS, CivId(0)), "a city-state protects nobody");

        w.diplomacy.declare_war(CivId(0), CS);
        assert!(!w.diplomacy.is_protector(CivId(0), CS), "you do not protect what you invade");
    }

    #[test]
    fn a_gift_of_gold_buys_influence_in_round_numbers() {
        let r = ruleset();
        let mut w = world(&r);
        w.civ_mut(CivId(0)).unwrap().gold = 1000;

        let gained = gift_gold(&mut w, CivId(0), CS, 500, 500, Fixed::ONE).expect("a legal gift");
        assert!(gained >= Fixed::whole(5), "never less than five");
        assert_eq!(gained.raw() % 5000, 0, "{gained:?} should be a multiple of five, as the button says");
        assert_eq!(w.civ(CivId(0)).unwrap().gold, 500, "the gold left our treasury");
        assert_eq!(w.civ(CS).unwrap().gold, 500, "and arrived in theirs");
        assert_eq!(w.diplomacy.influence(CS, CivId(0)), gained);
    }

    #[test]
    fn a_bigger_gift_buys_more() {
        let r = ruleset();
        let w = world(&r);
        let small = influence_from_gift(&w, 100, 500, Fixed::ONE, CivId(0));
        let large = influence_from_gift(&w, 1000, 500, Fixed::ONE, CivId(0));
        assert!(large > small, "{large:?} should beat {small:?}");
    }

    #[test]
    fn gold_is_worth_less_late_in_the_game() {
        let r = ruleset();
        let mut w = world(&r);
        let early = influence_from_gift(&w, 1000, 500, Fixed::ONE, CivId(0));
        w.turn = 500;
        let late = influence_from_gift(&w, 1000, 500, Fixed::ONE, CivId(0));
        assert!(late < early, "a thousand gold late ({late:?}) should buy less than early ({early:?})");
    }

    #[test]
    fn you_cannot_gift_what_you_do_not_have() {
        let r = ruleset();
        let mut w = world(&r);
        w.civ_mut(CivId(0)).unwrap().gold = 10;
        assert_eq!(gift_gold(&mut w, CivId(0), CS, 500, 500, Fixed::ONE), None);
        assert_eq!(w.civ(CivId(0)).unwrap().gold, 10, "and nothing was taken trying");

        // Nor buy the favour of someone you are invading.
        w.civ_mut(CivId(0)).unwrap().gold = 1000;
        w.diplomacy.declare_war(CivId(0), CS);
        assert_eq!(gift_gold(&mut w, CivId(0), CS, 500, 500, Fixed::ONE), None);

        // Nor gift a major civ, which is a trade and not this.
        w.diplomacy.make_peace(CivId(0), CS);
        assert_eq!(gift_gold(&mut w, CivId(0), CivId(1), 500, 500, Fixed::ONE), None);
    }

    #[test]
    fn a_friend_pays_and_an_ally_pays_more() {
        let r = ruleset();
        let mut w = world(&r);
        assert!(bonus_uniques(&w, &r, CivId(0)).is_empty(), "a stranger is owed nothing");

        w.diplomacy.set_influence(CS, CivId(0), Fixed::whole(30));
        let friend = bonus_uniques(&w, &r, CivId(0));
        assert!(!friend.is_empty(), "a friend is owed something");

        w.diplomacy.set_influence(CS, CivId(0), Fixed::whole(60));
        let ally = bonus_uniques(&w, &r, CivId(0));
        assert!(!ally.is_empty());
        // The ally list replaces the friend list rather than adding to it —
        // the ally uniques already restate the friend ones at a higher value,
        // so granting both would pay twice.
        let kind = r.city_state_types.get(w.civ(CS).unwrap().city_state_type.unwrap()).unwrap();
        assert_eq!(ally.len(), kind.ally_bonus_uniques.len());
        assert_eq!(friend.len(), kind.friend_bonus_uniques.len());
    }

    // --- elections and coups ---

    /// Put a rigging spy from `major` in the city-state's city.
    fn rig(w: &mut World, r: &Ruleset, major: CivId, rank: u8) {
        let capital = crate::world::connections::capital_of(w, r, CS)
            .or_else(|| w.civ(CS).and_then(|c| c.cities.first().copied()))
            .expect("the city-state has a city");
        let mut spy = crate::world::espionage::Spy::new();
        spy.rank = rank;
        spy.station = Some(capital);
        spy.action = crate::world::espionage::SpyAction::RiggingElections;
        w.civ_mut(major).unwrap().espionage.spies.push(spy);
    }

    #[test]
    fn elections_are_held_on_a_clock_and_only_when_someone_is_rigging() {
        let r = ruleset();
        let mut w = world(&r);
        w.turn = ELECTION_TURNS;
        assert!(hold_elections(&mut w, &r).is_empty(), "nobody is rigging anything");

        rig(&mut w, &r, CivId(0), 1);
        w.turn = ELECTION_TURNS - 1;
        assert!(hold_elections(&mut w, &r).is_empty(), "not election day");
        w.turn = ELECTION_TURNS;
        assert!(!hold_elections(&mut w, &r).is_empty(), "election day");
    }

    #[test]
    fn rigging_an_election_moves_influence_for_and_against() {
        let r = ruleset();
        let mut w = world(&r);
        rig(&mut w, &r, CivId(0), 3);
        w.turn = ELECTION_TURNS;

        let results = hold_elections(&mut w, &r);
        match results.first() {
            Some(ElectionResult::Rigged { winner, .. }) => {
                assert_eq!(*winner, CivId(0));
                assert_eq!(w.diplomacy.influence(CS, CivId(0)), ELECTION_WIN_INFLUENCE);
                assert_eq!(
                    w.diplomacy.influence(CS, CivId(1)),
                    ELECTION_LOSS_INFLUENCE,
                    "a rigged election is a public fact — the bystander loses too"
                );
            }
            Some(ElectionResult::Neutral { .. }) => {
                // The neutral party is a real candidate; losing to it costs.
                assert!(w.diplomacy.influence(CS, CivId(0)) < Fixed::ZERO);
            }
            None => panic!("an election should have been held"),
        }
    }

    #[test]
    fn the_neutral_party_is_a_real_candidate() {
        // Over many city-states with one weak rigger each, the election should
        // sometimes go to nobody — otherwise rigging is a purchase and not a
        // gamble, and a single cheap spy would own every city-state.
        let r = ruleset();
        let mut neutral = 0;
        for seed in 0..40u8 {
            let mut w = world(&r);
            rig(&mut w, &r, CivId(0), 1);
            w.turn = ELECTION_TURNS * (1 + seed as u32);
            if matches!(hold_elections(&mut w, &r).first(), Some(ElectionResult::Neutral { .. })) {
                neutral += 1;
            }
        }
        assert!(neutral > 0, "the neutral party never won in forty elections");
        assert!(neutral < 40, "and never lost either");
    }

    #[test]
    fn influence_already_held_makes_rigging_easier() {
        // The whole reason an ally stays an ally: half their influence counts
        // as votes, so the incumbent is hard to unseat by spycraft alone.
        let r = ruleset();
        let mut w = world(&r);
        rig(&mut w, &r, CivId(0), 1);
        rig(&mut w, &r, CivId(1), 1);
        w.diplomacy.set_influence(CS, CivId(1), Fixed::whole(100));

        let mut wins = [0, 0];
        for round in 1..=40u32 {
            let mut t = w.clone();
            t.turn = ELECTION_TURNS * round;
            if let Some(ElectionResult::Rigged { winner, .. }) = hold_elections(&mut t, &r).first() {
                wins[winner.0 as usize] += 1;
            }
        }
        assert!(wins[1] > wins[0], "the incumbent with a hundred influence lost {}-{} to a newcomer", wins[1], wins[0]);
    }

    #[test]
    fn the_same_turn_elects_the_same_government() {
        let r = ruleset();
        let mut a = world(&r);
        rig(&mut a, &r, CivId(0), 2);
        rig(&mut a, &r, CivId(1), 2);
        a.turn = ELECTION_TURNS * 3;
        let mut b = a.clone();
        assert_eq!(hold_elections(&mut a, &r), hold_elections(&mut b, &r));
        assert_eq!(a, b);
    }

    #[test]
    fn a_coup_needs_a_spy_in_place_and_someone_to_depose() {
        let r = ruleset();
        let mut w = world(&r);
        assert!(!can_coup(&w, &r, CivId(0), CS), "no spy, no coup");

        rig(&mut w, &r, CivId(0), 1);
        assert!(can_coup(&w, &r, CivId(0), CS));

        // You do not overthrow your own government.
        w.diplomacy.set_influence(CS, CivId(0), Fixed::whole(90));
        assert_eq!(ally_of(&w, CS), Some(CivId(0)));
        assert!(!can_coup(&w, &r, CivId(0), CS));

        assert_eq!(attempt_coup(&mut w, &r, CivId(1), CS), CoupResult::Impossible, "and civ 1 has no spy");
    }

    #[test]
    fn the_coup_chance_narrows_as_the_gap_widens() {
        let r = ruleset();
        let mut w = world(&r);
        rig(&mut w, &r, CivId(0), 1);
        w.diplomacy.set_influence(CS, CivId(1), Fixed::whole(60));

        let far_behind = coup_chance(&w, &r, CivId(0), CS);
        w.diplomacy.set_influence(CS, CivId(0), Fixed::whole(50));
        let close = coup_chance(&w, &r, CivId(0), CS);
        assert!(close > far_behind, "{close} should beat {far_behind}");

        // Never a certainty, however close and however good the spy.
        //
        // The plotter has to stay *below* the incumbent for any of this to
        // mean anything: the ally is derived from whoever holds the most
        // influence, so a plotter who overtakes the ally has already won and
        // has nothing left to overthrow. Setting the plotter to 500 does not
        // make a coup certain — it makes them the government.
        w.civ_mut(CivId(0)).unwrap().espionage.spies[0].rank = 3;
        w.diplomacy.set_influence(CS, CivId(0), Fixed::whole(59));
        assert_eq!(ally_of(&w, CS), Some(CivId(1)), "civ 1 still holds it at 60");
        assert_eq!(coup_chance(&w, &r, CivId(0), CS), COUP_MAX_CHANCE);

        // And never worse than hopeless.
        w.civ_mut(CivId(0)).unwrap().espionage.spies[0].rank = 1;
        w.diplomacy.set_influence(CS, CivId(1), Fixed::whole(500));
        w.diplomacy.set_influence(CS, CivId(0), crate::world::diplomacy::MINIMUM_INFLUENCE);
        assert_eq!(coup_chance(&w, &r, CivId(0), CS), 0);
    }

    #[test]
    fn a_veteran_spy_makes_a_coup_likelier_and_a_guard_makes_it_harder() {
        let r = ruleset();
        let mut w = world(&r);
        w.diplomacy.set_influence(CS, CivId(1), Fixed::whole(60));
        rig(&mut w, &r, CivId(0), 1);
        let raw = coup_chance(&w, &r, CivId(0), CS);

        w.civ_mut(CivId(0)).unwrap().espionage.spies[0].rank = 3;
        let veteran = coup_chance(&w, &r, CivId(0), CS);
        assert!(veteran > raw, "{veteran} should beat {raw}");

        // The incumbent's own spy defends what it bought.
        rig(&mut w, &r, CivId(1), 3);
        let guarded = coup_chance(&w, &r, CivId(0), CS);
        assert!(guarded < veteran, "{guarded} should be under {veteran}");
    }

    #[test]
    fn a_successful_coup_inherits_the_deposed_allys_standing() {
        let r = ruleset();
        let mut w = world(&r);
        w.diplomacy.set_influence(CS, CivId(1), Fixed::whole(70));
        assert_eq!(ally_of(&w, CS), Some(CivId(1)));
        rig(&mut w, &r, CivId(0), 3);
        // Stack the odds so the outcome is near-certain and the test is about
        // the consequences rather than about the roll. The plotter stays just
        // *below* the incumbent — overtake them and you are the ally already.
        w.diplomacy.set_influence(CS, CivId(0), Fixed::whole(65));
        assert_eq!(ally_of(&w, CS), Some(CivId(1)), "civ 1 is still in charge");
        assert_eq!(coup_chance(&w, &r, CivId(0), CS), COUP_MAX_CHANCE);

        let mut succeeded = false;
        for turn in 1..=40u32 {
            let mut t = w.clone();
            t.turn = turn;
            if let CoupResult::Succeeded { deposed, .. } = attempt_coup(&mut t, &r, CivId(0), CS) {
                assert_eq!(deposed, Some(CivId(1)));
                assert_eq!(
                    t.diplomacy.influence(CS, CivId(0)),
                    Fixed::whole(70),
                    "the plotter takes the position outright, not a share of it"
                );
                assert_eq!(t.diplomacy.influence(CS, CivId(1)), Fixed::whole(50), "70 less 20");
                assert_eq!(ally_of(&t, CS), Some(CivId(0)));
                succeeded = true;
                break;
            }
        }
        assert!(succeeded, "at 85% a coup should land inside forty attempts");
    }

    #[test]
    fn a_failed_coup_kills_the_spy_and_costs_influence() {
        let r = ruleset();
        let mut w = world(&r);
        w.diplomacy.set_influence(CS, CivId(1), Fixed::whole(100));
        rig(&mut w, &r, CivId(0), 1);
        w.diplomacy.set_influence(CS, CivId(0), Fixed::whole(10));
        assert!(coup_chance(&w, &r, CivId(0), CS) < 50, "the odds are against us");

        let mut failed = false;
        for turn in 1..=60u32 {
            let mut t = w.clone();
            t.turn = turn;
            if let CoupResult::Failed { .. } = attempt_coup(&mut t, &r, CivId(0), CS) {
                assert_eq!(t.diplomacy.influence(CS, CivId(0)), Fixed::whole(-10), "10 less 20");
                let spy = &t.civ(CivId(0)).unwrap().espionage.spies[0];
                assert_eq!(spy.station, None, "the spy is dead");
                assert_eq!(spy.rank, 1);
                failed = true;
                break;
            }
        }
        assert!(failed, "a coup against the odds should fail at least once in sixty");
    }

    #[test]
    fn the_same_coup_resolves_the_same_way_twice() {
        let r = ruleset();
        let mut w = world(&r);
        w.diplomacy.set_influence(CS, CivId(1), Fixed::whole(60));
        rig(&mut w, &r, CivId(0), 2);
        w.turn = 77;
        let mut a = w.clone();
        let mut b = w.clone();
        assert_eq!(attempt_coup(&mut a, &r, CivId(0), CS), attempt_coup(&mut b, &r, CivId(0), CS));
        assert_eq!(a, b);
    }

    #[test]
    fn a_city_state_cannot_win_the_game() {
        let r = ruleset();
        let mut w = world(&r);
        // Give the city-state everything a domination victory needs: it is
        // the only civ left standing.
        w.civ_mut(CivId(0)).unwrap().defeated = true;
        w.civ_mut(CivId(1)).unwrap().defeated = true;
        assert_eq!(
            crate::world::victory::winner(&w, &r),
            None,
            "a city-state is not playing, so it cannot win by outlasting those who are"
        );
    }
}
