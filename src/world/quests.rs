//! What a city-state asks of you, and what it pays.
//!
//! A quest is the only way a city-state *initiates* anything. Everything else
//! about the relationship is done to it — gold pushed at it, armies marched
//! past it. A quest is the city-state naming a thing it wants and paying
//! influence when it gets it, which is what turns a row of numbers into
//! something that behaves like a neighbour.
//!
//! # A quest is only real if the engine can judge it
//!
//! The ruleset names eighteen quests. Some of them this engine can decide —
//! "connect this resource" is a lookup in the resource stock, "build a road to
//! our city" is a question the connection graph already answers. Others depend
//! on systems that are not built: denouncing a civ needs denunciations,
//! bullying a city-state needs tribute.
//!
//! Rather than half-implement those, [`judge`] returns the same three-valued
//! [`Verdict`] the rest of the engine uses, and an `Unknown` quest is simply
//! never assigned. The gap stays countable — [`judgeable`] reports exactly
//! which of the ruleset's quests this engine can run — instead of turning into
//! a quest that is handed out and can never be completed.
//!
//! # Contests are why a quest carries a baseline
//!
//! "Gain more culture than anyone else by turn N" cannot be judged from the
//! world alone: it needs to know what each contestant had when the clock
//! started. So an assigned quest stores that snapshot. It is the one piece of
//! state here that is not derivable, and it is stored for exactly that reason.

use crate::arena::Id;
use crate::rng::Rng;
use crate::ruleset::{Quest, Ruleset};
use crate::stats::Fixed;
use crate::world::conditionals::Verdict;
use crate::world::grid::{CivId, TileId};
use crate::world::world::World;

/// No quests before this turn, so the early game is about expanding rather
/// than about errands.
pub const FIRST_QUEST_TURN: u32 = 30;
/// Turns between one city-state's quests.
pub const MIN_TURNS_BETWEEN: u32 = 20;
/// And a random spread on top, so every city-state does not ask at once.
pub const RAND_TURNS_BETWEEN: u32 = 25;
/// How many a single city-state runs at a time.
pub const MAX_ACTIVE: usize = 2;
/// Influence paid when the ruleset does not say.
pub const DEFAULT_INFLUENCE: i32 = 40;
/// How long a quest stands when the ruleset does not say.
pub const DEFAULT_DURATION: u32 = 50;

/// The thing a quest names.
///
/// A quest without a target is not a quest — "connect a resource" is not a
/// task until it says *which*. Held as a resolved id rather than as the
/// parameter text, so a quest cannot outlive the thing it asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Target {
    /// The city-state's own city — "build a road to *us*".
    Self_,
    Resource(Id<crate::ruleset::TileResource>),
    Building(Id<crate::ruleset::Building>),
    Civ(CivId),
    Tile(TileId),
    /// A contest against the other contestants, judged on the baseline.
    Contest,
    /// A bonus the quest grants while it runs, in percent — Invest's extra
    /// influence on gifts. Nothing to achieve; it simply runs out.
    Percent(i32),
}

/// What is being contested, for the three contest quests.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Contest {
    Culture,
    Faith,
    Technologies,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Assigned {
    pub quest: Id<Quest>,
    pub assignee: CivId,
    pub target: Target,
    pub contest: Option<Contest>,
    pub assigned_on: u32,
    pub expires_on: u32,
    /// What the assignee held when the clock started. Only meaningful for a
    /// contest, and the one thing here that cannot be recomputed.
    pub baseline: i32,
}

/// A city-state's quest book.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct QuestBook {
    pub active: Vec<Assigned>,
    /// Turn the next quest may be handed out. Zero means "as soon as allowed".
    pub next_turn: u32,
}

/// Which of the ruleset's quests this engine can actually decide.
///
/// Reported rather than filtered silently: a ruleset that adds a quest this
/// engine cannot judge should show up as a number, not as a quest that is
/// never completed and never explained.
pub fn judgeable(r: &Ruleset) -> (Vec<Id<Quest>>, Vec<String>) {
    let mut can = Vec::new();
    let mut cannot = Vec::new();
    for (id, q) in r.quests.iter() {
        if kind_of(&q.name).is_some() {
            can.push(id);
        } else {
            cannot.push(q.name.clone());
        }
    }
    (can, cannot)
}

/// The quests this engine knows how to judge, by the ruleset's own names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Route,
    ConnectResource,
    ConstructWonder,
    AcquireGreatPerson,
    ConquerCityState,
    FindPlayer,
    FindNaturalWonder,
    PledgeToProtect,
    ClearBarbarianCamp,
    SpreadReligion,
    TradeRoute,
    ContestCulture,
    ContestFaith,
    ContestTechnologies,
    GiveGold,
    Invest,
    BullyCityState,
    DenounceCivilization,
}

fn kind_of(name: &str) -> Option<Kind> {
    Some(match name {
        "Route" => Kind::Route,
        "Connect Resource" => Kind::ConnectResource,
        "Construct Wonder" => Kind::ConstructWonder,
        "Acquire Great Person" => Kind::AcquireGreatPerson,
        "Conquer City State" => Kind::ConquerCityState,
        "Find Player" => Kind::FindPlayer,
        "Find Natural Wonder" => Kind::FindNaturalWonder,
        "Pledge to Protect" => Kind::PledgeToProtect,
        "Clear Barbarian Camp" => Kind::ClearBarbarianCamp,
        "Spread Religion" => Kind::SpreadReligion,
        "Trade Route" => Kind::TradeRoute,
        "Contest Culture" => Kind::ContestCulture,
        "Contest Faith" => Kind::ContestFaith,
        "Contest Technologies" => Kind::ContestTechnologies,
        "Give Gold" => Kind::GiveGold,
        "Invest" => Kind::Invest,
        "Bully City State" => Kind::BullyCityState,
        "Denounce Civilization" => Kind::DenounceCivilization,
        _ => return None,
    })
}

/// What a contest is measured in, for the baseline snapshot.
fn contest_score(world: &World, civ: CivId, what: Contest) -> i32 {
    let Some(c) = world.civ(civ) else { return 0 };
    match what {
        Contest::Culture => c.culture_stored,
        Contest::Faith => c.faith.stored,
        Contest::Technologies => c.techs.len() as i32,
    }
}

/// Has this quest been met?
///
/// `True` pays out, `False` keeps waiting, `Unknown` means the engine cannot
/// tell — which never happens for an assigned quest, because [`judgeable`]
/// keeps the undecidable ones out of the pool in the first place. It is
/// returned rather than asserted so a mod's unknown quest degrades to "never
/// completes" instead of panicking.
pub fn judge(world: &World, r: &Ruleset, city_state: CivId, q: &Assigned) -> Verdict {
    let Some(quest) = r.quests.get(q.quest) else { return Verdict::Unknown };
    let Some(kind) = kind_of(&quest.name) else { return Verdict::Unknown };
    let yes = |b: bool| if b { Verdict::True } else { Verdict::False };
    let Some(who) = world.civ(q.assignee) else { return Verdict::Unknown };

    match kind {
        // Completed when it happens — a gift, a tribute — by `on_gold_gift`
        // and `on_bullied`; Invest never completes, it runs out.
        Kind::GiveGold | Kind::BullyCityState | Kind::Invest => Verdict::False,
        Kind::DenounceCivilization => match q.target {
            Target::Civ(bully) => {
                yes(world.diplomacy.has_flag(q.assignee, bully, crate::world::diplomacy::Flag::Denunciation))
            }
            _ => Verdict::Unknown,
        },
        // The connection graph already answers this: is the city-state's city
        // reachable from our capital by road, rail or harbour?
        Kind::Route | Kind::TradeRoute => {
            let Some(city) = world.civ(city_state).and_then(|c| c.cities.first().copied()) else {
                return Verdict::Unknown;
            };
            let conn = crate::world::connections::find(world, r, q.assignee);
            yes(conn.has_trade_route(city))
        }
        Kind::ConnectResource => match q.target {
            Target::Resource(res) => yes(who.resources.get(&res).copied().unwrap_or(0) > 0),
            _ => Verdict::Unknown,
        },
        Kind::ConstructWonder => match q.target {
            Target::Building(b) => yes(world.cities.iter().any(|(_, c)| c.owner == q.assignee && c.built.contains(&b))),
            _ => Verdict::Unknown,
        },
        // Any great person at all, measured against what they had. Points are
        // spent when one is born, so the count is the honest measure.
        Kind::AcquireGreatPerson => yes(great_person_count(world, q.assignee) > q.baseline),
        Kind::ConquerCityState => match q.target {
            Target::Civ(other) => yes(world.civ(other).is_some_and(|c| !c.is_alive())),
            _ => Verdict::Unknown,
        },
        Kind::FindPlayer => match q.target {
            Target::Civ(other) => yes(world.diplomacy.have_met(q.assignee, other)),
            _ => Verdict::Unknown,
        },
        Kind::FindNaturalWonder => match q.target {
            Target::Tile(t) => yes(world.visibility.has_explored(q.assignee, t)),
            _ => Verdict::Unknown,
        },
        Kind::PledgeToProtect => yes(world.diplomacy.is_protector(q.assignee, city_state)),
        // The camp is gone when the tile no longer carries the improvement —
        // whoever cleared it. The quest does not ask who.
        Kind::ClearBarbarianCamp => match q.target {
            Target::Tile(t) => {
                let camp = crate::world::barbarians::camp_improvement(r);
                yes(world.grid.improvement[t.0 as usize] != camp)
            }
            _ => Verdict::Unknown,
        },
        Kind::SpreadReligion => {
            let Some(city) =
                world.civ(city_state).and_then(|c| c.cities.first().copied()).and_then(|id| world.cities.get(id))
            else {
                return Verdict::Unknown;
            };
            yes(city.faith.majority(city.population) == Some(q.assignee))
        }
        // A contest is won by having gained the most since the clock started,
        // and is only decided when the clock runs out — leading on turn three
        // is not winning.
        Kind::ContestCulture | Kind::ContestFaith | Kind::ContestTechnologies => {
            let Some(what) = q.contest else { return Verdict::Unknown };
            if world.turn < q.expires_on {
                return Verdict::False;
            }
            let Some(book) = world.civ(city_state).map(|c| &c.quests) else {
                return Verdict::Unknown;
            };
            let mine = contest_score(world, q.assignee, what) - q.baseline;
            let beaten = book.active.iter().filter(|o| o.quest == q.quest && o.assignee != q.assignee).any(|o| {
                let theirs = contest_score(world, o.assignee, what) - o.baseline;
                theirs > mine || (theirs == mine && o.assignee.0 < q.assignee.0)
            });
            yes(!beaten)
        }
    }
}

fn great_person_count(world: &World, civ: CivId) -> i32 {
    world.units.iter().filter(|(_, u)| u.owner == civ).count() as i32
}

/// What happened to a quest.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuestEvent {
    Offered { city_state: CivId, quest: Id<Quest>, to: CivId },
    Completed { city_state: CivId, quest: Id<Quest>, by: CivId, influence: i32 },
    Expired { city_state: CivId, quest: Id<Quest>, on: CivId },
}

/// Pick a target for a quest, or `None` if there is nothing to ask for.
///
/// A quest with no available target is not offered at all, which is why this
/// returns an `Option` rather than a placeholder: "conquer the city-state of
/// nobody" is not a task.
fn choose_target(
    world: &World,
    r: &Ruleset,
    city_state: CivId,
    assignee: CivId,
    kind: Kind,
    rng: &mut Rng,
) -> Option<(Target, Option<Contest>)> {
    // Everything here picks from a sorted list by index, never by iteration
    // order, so two machines choose the same target.
    let pick = |rng: &mut Rng, n: usize| -> Option<usize> {
        if n == 0 {
            None
        } else {
            Some(rng.range(0, n as i64 - 1) as usize)
        }
    };

    Some(match kind {
        // Pay us, since that civ took from us.
        Kind::GiveGold => (Target::Civ(crate::world::tribute::most_recent_bully(world, city_state)?), None),
        Kind::DenounceCivilization => {
            let bully = crate::world::tribute::most_recent_bully(world, city_state)?;
            let human = |c: CivId| world.civ(c).is_some_and(|x| !x.is_ai);
            if bully == assignee
                || world.diplomacy.has_flag(assignee, bully, crate::world::diplomacy::Flag::Denunciation)
                || world.diplomacy.at_war(assignee, bully)
                || (human(assignee) && human(bully))
            {
                return None;
            }
            (Target::Civ(bully), None)
        }
        Kind::Invest => {
            // "... any Gold donation will yield [50]% extra Influence."
            let quest = r.quests.iter().find(|(_, q)| q.name == "Invest").map(|(_, q)| q)?;
            let percent = quest.description.split('[').nth(1)?.split(']').next()?.parse::<i32>().ok()?;
            (Target::Percent(percent), None)
        }
        // Bully our nearest rival — whichever city-states are nearest, if any
        // is nearer than distant (`getCityStateTarget`).
        Kind::BullyCityState => {
            let others: Vec<(Proximity, CivId)> = crate::world::city_states::city_states(world)
                .into_iter()
                .filter(|c| *c != city_state)
                .map(|c| (proximity(world, city_state, c), c))
                .filter(|(p, _)| *p != Proximity::None)
                .collect();
            let nearest = others.iter().map(|(p, _)| *p).min().filter(|p| *p != Proximity::Distant)?;
            let mut options: Vec<CivId> = others
                .into_iter()
                .filter(|(p, c)| {
                    *p == nearest && world.diplomacy.have_met(city_state, *c) && world.diplomacy.have_met(assignee, *c)
                })
                .map(|(_, c)| c)
                .collect();
            options.sort();
            (Target::Civ(*options.get(pick(rng, options.len())?)?), None)
        }
        Kind::Route | Kind::TradeRoute | Kind::PledgeToProtect | Kind::SpreadReligion => (Target::Self_, None),
        Kind::AcquireGreatPerson => (Target::Contest, None),
        Kind::ConnectResource => {
            // Something they do not already have, or the quest is already done.
            let mut options: Vec<_> = r
                .resources
                .iter()
                .filter(|(_, res)| res.resource_type == "Luxury" || res.resource_type == "Strategic")
                .map(|(id, _)| id)
                .filter(|id| world.civ(assignee).is_some_and(|c| c.resources.get(id).copied().unwrap_or(0) == 0))
                .collect();
            options.sort_by_key(|i| i.index());
            (Target::Resource(*options.get(pick(rng, options.len())?)?), None)
        }
        Kind::ConstructWonder => {
            let mut options: Vec<_> = r
                .buildings
                .iter()
                .filter(|(_, b)| b.is_wonder == Some(true))
                .map(|(id, _)| id)
                .filter(|id| !world.cities.iter().any(|(_, c)| c.built.contains(id)))
                .collect();
            options.sort_by_key(|i| i.index());
            (Target::Building(*options.get(pick(rng, options.len())?)?), None)
        }
        Kind::ConquerCityState => {
            let mut options: Vec<_> =
                crate::world::city_states::city_states(world).into_iter().filter(|c| *c != city_state).collect();
            options.sort();
            (Target::Civ(*options.get(pick(rng, options.len())?)?), None)
        }
        Kind::FindPlayer => {
            let mut options: Vec<_> = world
                .civ_ids()
                .filter(|c| *c != assignee)
                .filter(|c| world.civ(*c).is_some_and(|x| x.is_major() && x.is_alive()))
                .filter(|c| !world.diplomacy.have_met(assignee, *c))
                .collect();
            options.sort();
            (Target::Civ(*options.get(pick(rng, options.len())?)?), None)
        }
        Kind::FindNaturalWonder => {
            let mut options: Vec<_> = world
                .grid
                .ids()
                .filter(|t| world.grid.natural_wonder[t.0 as usize].is_some())
                .filter(|t| !world.visibility.has_explored(assignee, *t))
                .collect();
            options.sort();
            (Target::Tile(*options.get(pick(rng, options.len())?)?), None)
        }
        Kind::ClearBarbarianCamp => {
            let mut options = crate::world::barbarians::camps(world, r);
            options.sort();
            (Target::Tile(*options.get(pick(rng, options.len())?)?), None)
        }
        Kind::ContestCulture => (Target::Contest, Some(Contest::Culture)),
        Kind::ContestFaith => (Target::Contest, Some(Contest::Faith)),
        Kind::ContestTechnologies => (Target::Contest, Some(Contest::Technologies)),
    })
}

/// One turn of quests for every city-state: judge what is running, pay what is
/// finished, expire what has run out, then hand out what is due.
///
/// Judged before assigned, so a quest cannot be offered and completed on the
/// same turn by accident.
pub fn tick(world: &mut World, r: &Ruleset) -> Vec<QuestEvent> {
    let mut events = Vec::new();
    let turn = world.turn;

    for cs in crate::world::city_states::city_states(world) {
        // --- judge what is running ---
        let active = world.civ(cs).map(|c| c.quests.active.clone()).unwrap_or_default();
        let mut keep = Vec::new();
        let mut payouts = Vec::new();
        for q in active {
            match judge(world, r, cs, &q) {
                Verdict::True => {
                    let influence = r.quests.get(q.quest).and_then(|x| x.influence).unwrap_or(DEFAULT_INFLUENCE);
                    payouts.push((q.assignee, influence));
                    events.push(QuestEvent::Completed { city_state: cs, quest: q.quest, by: q.assignee, influence });
                }
                _ if turn >= q.expires_on => {
                    events.push(QuestEvent::Expired { city_state: cs, quest: q.quest, on: q.assignee });
                }
                _ => keep.push(q),
            }
        }
        if let Some(c) = world.civ_mut(cs) {
            c.quests.active = keep;
        }
        for (major, influence) in payouts {
            world.diplomacy.add_influence(cs, major, Fixed::whole(influence));
        }

        // --- hand out what is due ---
        if turn < FIRST_QUEST_TURN {
            continue;
        }
        let due = world.civ(cs).map_or(0, |c| c.quests.next_turn);
        if turn < due.max(FIRST_QUEST_TURN) {
            continue;
        }

        let mut rng = Rng::from_parts(&[0x9_0E57, turn as u64, cs.0 as u64]);
        let (pool, _) = judgeable(r);
        let candidates: Vec<CivId> = world
            .civ_ids()
            .filter(|c| world.civ(*c).is_some_and(|x| x.is_major() && x.is_alive()))
            .filter(|c| world.diplomacy.have_met(cs, *c))
            .filter(|c| !world.diplomacy.at_war(cs, *c))
            // A city-state asks nothing of whoever just bullied it.
            .filter(|c| !world.diplomacy.has_flag(cs, *c, crate::world::diplomacy::Flag::Bullied))
            .collect();

        for major in candidates {
            let running = world.civ(cs).map_or(0, |c| c.quests.active.iter().filter(|q| q.assignee == major).count());
            if running >= MAX_ACTIVE || pool.is_empty() {
                continue;
            }
            let idx = rng.range(0, pool.len() as i64 - 1) as usize;
            let quest_id = pool[idx];
            let Some(quest) = r.quests.get(quest_id) else { continue };
            let Some(kind) = kind_of(&quest.name) else { continue };
            // Never offer the same quest twice at once.
            if world.civ(cs).is_some_and(|c| c.quests.active.iter().any(|q| q.assignee == major && q.quest == quest_id))
            {
                continue;
            }
            let Some((target, contest)) = choose_target(world, r, cs, major, kind, &mut rng) else {
                continue; // nothing to ask for; try again next time
            };
            let duration = quest.duration.unwrap_or(DEFAULT_DURATION as i32).max(1) as u32;
            let baseline = match contest {
                Some(what) => contest_score(world, major, what),
                None if kind == Kind::AcquireGreatPerson => great_person_count(world, major),
                None => 0,
            };
            let assigned = Assigned {
                quest: quest_id,
                assignee: major,
                target,
                contest,
                assigned_on: turn,
                expires_on: turn + duration,
                baseline,
            };
            if let Some(c) = world.civ_mut(cs) {
                c.quests.active.push(assigned);
            }
            events.push(QuestEvent::Offered { city_state: cs, quest: quest_id, to: major });
        }

        let gap = MIN_TURNS_BETWEEN + rng.range(0, RAND_TURNS_BETWEEN as i64) as u32;
        if let Some(c) = world.civ_mut(cs) {
            c.quests.next_turn = turn + gap;
        }
    }
    events
}

/// How near two civs are (`CivInfoTransientCache.getProximity`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Proximity {
    Neighbors,
    Close,
    Far,
    Distant,
    /// One of them has no cities.
    None,
}

/// Neighbors within seven tiles; Close if their nearest cities are within
/// eleven and their cities average within a quarter of the map; Far within
/// forty-five percent; otherwise Distant. Unciv also counts civs on different
/// continents one step further apart, which this map does not model.
pub fn proximity(world: &World, a: CivId, b: CivId) -> Proximity {
    let centres = |c: CivId| -> Vec<TileId> {
        world
            .civ(c)
            .map(|x| x.cities.iter().filter_map(|id| world.cities.get(*id)).map(|c| c.centre).collect())
            .unwrap_or_default()
    };
    let (ours, theirs) = (centres(a), centres(b));
    if ours.is_empty() || theirs.is_empty() {
        return Proximity::None;
    }
    let (mut min, mut total, mut n) = (u32::MAX, 0u32, 0u32);
    for x in &ours {
        for y in &theirs {
            let d = world.grid.distance(*x, *y);
            min = min.min(d);
            total += d;
            n += 1;
        }
    }
    if min <= 7 {
        return Proximity::Neighbors;
    }
    let average = total / n;
    let map = (world.grid.width() as u32 + world.grid.height() as u32) / 2;
    let close = (map * 25 / 100).clamp(10, 20);
    let far = (map * 45 / 100).clamp(20, 50);
    if min <= 11 && average <= close {
        Proximity::Close
    } else if average <= far {
        Proximity::Far
    } else {
        Proximity::Distant
    }
}

/// Pay out and remove every quest `matches` picks, at one city-state.
fn complete(world: &mut World, r: &Ruleset, cs: CivId, matches: impl Fn(&Assigned, Kind) -> bool) -> Vec<QuestEvent> {
    let active = world.civ(cs).map(|c| c.quests.active.clone()).unwrap_or_default();
    let (done, keep): (Vec<_>, Vec<_>) = active
        .into_iter()
        .partition(|q| r.quests.get(q.quest).and_then(|x| kind_of(&x.name)).is_some_and(|k| matches(q, k)));
    if let Some(c) = world.civ_mut(cs) {
        c.quests.active = keep;
    }
    let mut events = Vec::new();
    for q in done {
        let influence = r.quests.get(q.quest).and_then(|x| x.influence).unwrap_or(DEFAULT_INFLUENCE);
        world.diplomacy.add_influence(cs, q.assignee, Fixed::whole(influence));
        events.push(QuestEvent::Completed { city_state: cs, quest: q.quest, by: q.assignee, influence });
    }
    events
}

/// A major gave this city-state gold (`QuestManager.receivedGoldGift`).
pub fn on_gold_gift(world: &mut World, r: &Ruleset, cs: CivId, donor: CivId) -> Vec<QuestEvent> {
    complete(world, r, cs, |q, k| k == Kind::GiveGold && q.assignee == donor)
}

/// A city-state paid tribute (`QuestManager.cityStateBullied`), as every
/// city-state hears it: whoever asked for exactly that is paid, and the
/// victim takes back every quest it had given the bully.
pub fn on_bullied(world: &mut World, r: &Ruleset, victim: CivId, bully: CivId) -> Vec<QuestEvent> {
    let mut events = Vec::new();
    for cs in crate::world::city_states::city_states(world) {
        events.extend(complete(world, r, cs, |q, k| {
            k == Kind::BullyCityState && q.assignee == bully && q.target == Target::Civ(victim)
        }));
    }
    let revoked: Vec<Assigned> = world
        .civ(victim)
        .map(|c| c.quests.active.iter().filter(|q| q.assignee == bully).cloned().collect())
        .unwrap_or_default();
    if let Some(c) = world.civ_mut(victim) {
        c.quests.active.retain(|q| q.assignee != bully);
    }
    events.extend(revoked.into_iter().map(|q| QuestEvent::Expired { city_state: victim, quest: q.quest, on: bully }));
    events
}

/// How much more influence a gift from `donor` buys here, while an Invest
/// quest runs for it (`QuestManager.getInvestmentMultiplier`).
pub fn investment_multiplier(world: &World, cs: CivId, donor: CivId) -> Fixed {
    world
        .civ(cs)
        .and_then(|c| {
            c.quests.active.iter().find_map(|q| match q.target {
                Target::Percent(p) if q.assignee == donor => Some(Fixed::whole(100 + p).checked_div(Fixed::whole(100))),
                _ => None,
            })
        })
        .flatten()
        .unwrap_or(Fixed::ONE)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::city::City;
    use crate::world::entities::Civilization;
    use crate::world::grid::TileGrid;

    fn ruleset() -> Ruleset {
        Ruleset::load_dir("../android/assets/jsons/Civ V - Gods & Kings").expect("readable").expect("loads")
    }

    const CS: CivId = CivId(2);

    /// Two majors, one city-state, each with a city.
    fn world(r: &Ruleset) -> World {
        let g = TileGrid::new(16, 16, r.terrain("Grassland").unwrap());
        let nation = r.nations.ids().next().unwrap();
        let mut civs = vec![
            Civilization::new(CivId(0), nation, true),
            Civilization::new(CivId(1), nation, true),
            Civilization::new(CivId(2), nation, true),
        ];
        civs[2].city_state_type = r.city_state_types.ids().next();
        let mut w = World::new(g, civs);
        for (i, (col, row)) in [(2u16, 2u16), (12, 2), (7, 12)].into_iter().enumerate() {
            let t = w.grid.id_at(col, row).unwrap();
            w.found_city_in(r, City::found(format!("City{i}"), CivId(i as u8), t));
        }
        w.diplomacy.meet(CivId(0), CS);
        w.diplomacy.meet(CivId(1), CS);
        w
    }

    fn quest_named(r: &Ruleset, name: &str) -> Id<Quest> {
        r.quests.id(name).unwrap_or_else(|| panic!("the ruleset has a {name} quest"))
    }

    fn assign(w: &mut World, quest: Id<Quest>, to: CivId, target: Target) {
        let a = Assigned {
            quest,
            assignee: to,
            target,
            contest: None,
            assigned_on: w.turn,
            expires_on: w.turn + DEFAULT_DURATION,
            baseline: 0,
        };
        w.civ_mut(CS).unwrap().quests.active.push(a);
    }

    /// Make civ 1 the city-state's most recent bully.
    fn bullied_by_one(w: &mut World) {
        w.diplomacy.set_flag(CS, CivId(1), crate::world::diplomacy::Flag::Bullied, 20);
    }

    #[test]
    fn a_bullied_city_state_asks_for_gold_and_a_gift_pays_it() {
        let r = ruleset();
        let mut w = world(&r);
        let mut rng = Rng::from_parts(&[1]);
        assert_eq!(choose_target(&w, &r, CS, CivId(0), Kind::GiveGold, &mut rng), None, "nobody has bullied it yet");
        bullied_by_one(&mut w);
        assert_eq!(choose_target(&w, &r, CS, CivId(0), Kind::GiveGold, &mut rng), Some((Target::Civ(CivId(1)), None)));

        let q = quest_named(&r, "Give Gold");
        assign(&mut w, q, CivId(0), Target::Civ(CivId(1)));
        assert_eq!(judge(&w, &r, CS, &w.civ(CS).unwrap().quests.active[0]), Verdict::False, "waiting on a gift");
        let events = on_gold_gift(&mut w, &r, CS, CivId(0));
        assert!(matches!(events.as_slice(), [QuestEvent::Completed { by: CivId(0), .. }]), "{events:?}");
        assert!(w.civ(CS).unwrap().quests.active.is_empty());
    }

    #[test]
    fn an_invest_quest_makes_a_gift_worth_half_again() {
        let r = ruleset();
        let mut w = world(&r);
        w.civ_mut(CivId(0)).unwrap().gold = 10_000;
        let plain = crate::world::city_states::influence_from_gift(&w, 500, 500, Fixed::ONE, CivId(0), CS);
        let mut rng = Rng::from_parts(&[1]);
        let (target, _) = choose_target(&w, &r, CS, CivId(0), Kind::Invest, &mut rng).expect("always offerable");
        assert_eq!(target, Target::Percent(50), "the percentage is the ruleset's");
        assign(&mut w, quest_named(&r, "Invest"), CivId(0), target);
        assert_eq!(investment_multiplier(&w, CS, CivId(0)), Fixed::permille(1_500));
        assert_eq!(investment_multiplier(&w, CS, CivId(1)), Fixed::ONE, "only for whoever it was given to");
        let boosted = crate::world::city_states::influence_from_gift(&w, 500, 500, Fixed::ONE, CivId(0), CS);
        assert!(boosted > plain, "{boosted} should beat {plain}");
    }

    #[test]
    fn denouncing_the_bully_completes_the_denounce_quest() {
        let r = ruleset();
        let mut w = world(&r);
        w.diplomacy.meet(CivId(0), CivId(1));
        bullied_by_one(&mut w);
        let mut rng = Rng::from_parts(&[1]);
        let (target, _) = choose_target(&w, &r, CS, CivId(0), Kind::DenounceCivilization, &mut rng).expect("offerable");
        assert_eq!(target, Target::Civ(CivId(1)));
        assert_eq!(choose_target(&w, &r, CS, CivId(1), Kind::DenounceCivilization, &mut rng), None, "not of itself");
        assign(&mut w, quest_named(&r, "Denounce Civilization"), CivId(0), target);
        let q = w.civ(CS).unwrap().quests.active[0].clone();
        assert_eq!(judge(&w, &r, CS, &q), Verdict::False);
        crate::world::opinion::denounce(&mut w, CivId(0), CivId(1));
        assert_eq!(judge(&w, &r, CS, &q), Verdict::True);
    }

    #[test]
    fn bullying_the_named_rival_pays_and_the_victim_revokes_its_quests() {
        let r = ruleset();
        let mut w = world(&r);
        // A second city-state, near the first, that civ 0 has asked to be bullied.
        let nation = r.nations.ids().next().unwrap();
        let mut other = Civilization::new(CivId(3), nation, true);
        other.city_state_type = r.city_state_types.ids().next();
        w.civs.push(other);
        w.found_city_in(&r, City::found("Rival", CivId(3), w.grid.id_at(12, 12).unwrap()));
        w.diplomacy.meet(CivId(0), CivId(3));
        w.diplomacy.meet(CS, CivId(3));
        let mut rng = Rng::from_parts(&[1]);
        let (target, _) = choose_target(&w, &r, CS, CivId(0), Kind::BullyCityState, &mut rng).expect("a neighbour");
        assert_eq!(target, Target::Civ(CivId(3)));
        assign(&mut w, quest_named(&r, "Bully City State"), CivId(0), target);
        // The rival had given civ 0 a quest of its own.
        let route = quest_named(&r, "Route");
        let a = Assigned {
            quest: route,
            assignee: CivId(0),
            target: Target::Self_,
            contest: None,
            assigned_on: 0,
            expires_on: 99,
            baseline: 0,
        };
        w.civ_mut(CivId(3)).unwrap().quests.active.push(a);

        let events = on_bullied(&mut w, &r, CivId(3), CivId(0));
        assert!(
            events.iter().any(|e| matches!(e, QuestEvent::Completed { city_state: CS, by: CivId(0), .. })),
            "{events:?}"
        );
        assert!(events.iter().any(|e| matches!(e, QuestEvent::Expired { city_state: CivId(3), on: CivId(0), .. })));
        assert!(w.civ(CivId(3)).unwrap().quests.active.is_empty(), "the victim takes its quests back");
    }

    #[test]
    fn a_bully_is_offered_nothing() {
        let r = ruleset();
        let mut w = world(&r);
        bullied_by_one(&mut w);
        w.turn = 40;
        let events = tick(&mut w, &r);
        let offered_to: Vec<_> = events
            .iter()
            .filter_map(|e| if let QuestEvent::Offered { to, .. } = e { Some(*to) } else { None })
            .collect();
        assert!(!offered_to.contains(&CivId(1)), "{offered_to:?}");
    }

    #[test]
    fn the_ruleset_quests_load_and_most_are_judgeable() {
        let r = ruleset();
        assert!(r.quests.len() >= 15, "only {} quests loaded", r.quests.len());
        let (can, cannot) = judgeable(&r);
        println!("judgeable: {}, not: {cannot:?}", can.len());
        assert!(cannot.is_empty(), "every quest in the ruleset should be judgeable, not {cannot:?}");
        assert_eq!(can.len(), r.quests.len());
    }

    #[test]
    fn the_quests_tribute_and_denunciation_made_possible_are_in_the_pool() {
        let r = ruleset();
        let (can, _) = judgeable(&r);
        let pool: std::collections::BTreeSet<_> = can.into_iter().collect();
        for name in ["Give Gold", "Invest", "Bully City State", "Denounce Civilization"] {
            let id = r.quests.id(name).unwrap_or_else(|| panic!("the ruleset has {name}"));
            assert!(pool.contains(&id), "{name} should be offered now");
        }
    }

    #[test]
    fn connect_resource_is_met_by_holding_the_resource() {
        let r = ruleset();
        let mut w = world(&r);
        let res = r
            .resources
            .iter()
            .find(|(_, x)| x.resource_type == "Luxury")
            .map(|(id, _)| id)
            .expect("the ruleset has a luxury");
        let q = quest_named(&r, "Connect Resource");
        assign(&mut w, q, CivId(0), Target::Resource(res));

        let a = w.civ(CS).unwrap().quests.active[0].clone();
        assert_eq!(judge(&w, &r, CS, &a), Verdict::False, "we do not have it yet");
        w.civ_mut(CivId(0)).unwrap().resources.insert(res, 1);
        assert_eq!(judge(&w, &r, CS, &a), Verdict::True);
    }

    #[test]
    fn construct_wonder_asks_for_a_specific_wonder() {
        let r = ruleset();
        let mut w = world(&r);
        let wonder = r
            .buildings
            .iter()
            .find(|(_, b)| b.is_wonder == Some(true))
            .map(|(id, _)| id)
            .expect("the ruleset has a wonder");
        let q = quest_named(&r, "Construct Wonder");
        assign(&mut w, q, CivId(0), Target::Building(wonder));
        let a = w.civ(CS).unwrap().quests.active[0].clone();
        assert_eq!(judge(&w, &r, CS, &a), Verdict::False);

        // Someone else building it does not count.
        let theirs = w.civ(CivId(1)).unwrap().cities[0];
        w.cities.get_mut(theirs).unwrap().built.insert(wonder);
        assert_eq!(judge(&w, &r, CS, &a), Verdict::False, "the quest is ours to finish");

        let ours = w.civ(CivId(0)).unwrap().cities[0];
        w.cities.get_mut(ours).unwrap().built.insert(wonder);
        assert_eq!(judge(&w, &r, CS, &a), Verdict::True);
    }

    #[test]
    fn pledge_to_protect_is_the_protection_itself() {
        let r = ruleset();
        let mut w = world(&r);
        let q = quest_named(&r, "Pledge to Protect");
        assign(&mut w, q, CivId(0), Target::Self_);
        let a = w.civ(CS).unwrap().quests.active[0].clone();
        assert_eq!(judge(&w, &r, CS, &a), Verdict::False);
        w.diplomacy.set_protector(CivId(0), CS, true);
        assert_eq!(judge(&w, &r, CS, &a), Verdict::True);
        // And it is one-sided: the city-state protecting us is not this.
        w.diplomacy.set_protector(CivId(0), CS, false);
        w.diplomacy.set_protector(CS, CivId(0), true);
        assert_eq!(judge(&w, &r, CS, &a), Verdict::False);
    }

    #[test]
    fn a_route_quest_is_answered_by_the_connection_graph() {
        let r = ruleset();
        let mut w = world(&r);
        w.civ_mut(CivId(0)).unwrap().techs.insert(r.technologies.id("The Wheel").unwrap());
        let q = quest_named(&r, "Route");
        assign(&mut w, q, CivId(0), Target::Self_);
        let a = w.civ(CS).unwrap().quests.active[0].clone();
        assert_eq!(judge(&w, &r, CS, &a), Verdict::False, "no road yet");

        // Pave a line from our capital to theirs, and open the border so a
        // route may cross it.
        w.diplomacy.set_open_borders(CS, CivId(0), true);
        let ours = w.cities.get(w.civ(CivId(0)).unwrap().cities[0]).unwrap().centre;
        let theirs = w.cities.get(w.civ(CS).unwrap().cities[0]).unwrap().centre;
        // Step by actual neighbours — a straight line in column/row space is
        // not a connected path on a hex grid.
        let mut at = ours;
        let mut guard = 0;
        while at != theirs && guard < 200 {
            let next = w
                .grid
                .neighbours(at)
                .min_by_key(|n| (w.grid.distance(*n, theirs), n.0))
                .expect("every tile has a neighbour");
            w.grid.set_road(next, crate::world::grid::RoadLevel::Road);
            at = next;
            guard += 1;
        }
        assert_eq!(at, theirs, "the walk reached their city");
        assert_eq!(judge(&w, &r, CS, &a), Verdict::True, "the road reaches them");
    }

    #[test]
    fn spread_religion_asks_for_the_assignees_own_faith() {
        let r = ruleset();
        let mut w = world(&r);
        let q = quest_named(&r, "Spread Religion");
        assign(&mut w, q, CivId(0), Target::Self_);
        let a = w.civ(CS).unwrap().quests.active[0].clone();
        assert_eq!(judge(&w, &r, CS, &a), Verdict::False);

        let city = w.civ(CS).unwrap().cities[0];
        {
            let c = w.cities.get_mut(city).unwrap();
            c.population = 4;
            // Civ 1's religion, not ours — the wrong faith does not count.
            c.faith.add_pressure(Some(CivId(1)), 5000);
            c.faith.recount(4);
        }
        assert_eq!(judge(&w, &r, CS, &a), Verdict::False, "somebody else's religion");

        {
            let c = w.cities.get_mut(city).unwrap();
            c.faith.remove_all_pressures_except(CivId(0));
            c.faith.add_pressure(Some(CivId(0)), 5000);
            c.faith.recount(4);
        }
        assert_eq!(judge(&w, &r, CS, &a), Verdict::True);
    }

    #[test]
    fn clear_barbarian_camp_does_not_ask_who_cleared_it() {
        let r = ruleset();
        let mut w = world(&r);
        let camp = crate::world::barbarians::camp_improvement(&r).expect("camps exist");
        let t = w.grid.id_at(9, 9).unwrap();
        w.grid.improvement[t.0 as usize] = Some(camp);

        let q = quest_named(&r, "Clear Barbarian Camp");
        assign(&mut w, q, CivId(0), Target::Tile(t));
        let a = w.civ(CS).unwrap().quests.active[0].clone();
        assert_eq!(judge(&w, &r, CS, &a), Verdict::False);
        w.grid.improvement[t.0 as usize] = None;
        assert_eq!(judge(&w, &r, CS, &a), Verdict::True);
    }

    #[test]
    fn find_player_is_met_by_meeting_them() {
        let r = ruleset();
        let mut w = world(&r);
        let q = quest_named(&r, "Find Player");
        assign(&mut w, q, CivId(0), Target::Civ(CivId(1)));
        let a = w.civ(CS).unwrap().quests.active[0].clone();
        assert_eq!(judge(&w, &r, CS, &a), Verdict::False);
        w.diplomacy.meet(CivId(0), CivId(1));
        assert_eq!(judge(&w, &r, CS, &a), Verdict::True);
    }

    #[test]
    fn a_contest_is_not_decided_until_the_clock_runs_out() {
        let r = ruleset();
        let mut w = world(&r);
        let q = quest_named(&r, "Contest Culture");
        for civ in [CivId(0), CivId(1)] {
            w.civ_mut(CS).unwrap().quests.active.push(Assigned {
                quest: q,
                assignee: civ,
                target: Target::Contest,
                contest: Some(Contest::Culture),
                assigned_on: 0,
                expires_on: 20,
                baseline: 0,
            });
        }
        w.civ_mut(CivId(0)).unwrap().culture_stored = 500;
        w.civ_mut(CivId(1)).unwrap().culture_stored = 10;

        let leading = w.civ(CS).unwrap().quests.active[0].clone();
        assert_eq!(judge(&w, &r, CS, &leading), Verdict::False, "leading on turn one is not winning");

        w.turn = 20;
        assert_eq!(judge(&w, &r, CS, &leading), Verdict::True);
        let trailing = w.civ(CS).unwrap().quests.active[1].clone();
        assert_eq!(judge(&w, &r, CS, &trailing), Verdict::False, "and only one wins");
    }

    #[test]
    fn a_contest_measures_the_gain_not_the_total() {
        let r = ruleset();
        let mut w = world(&r);
        let q = quest_named(&r, "Contest Faith");
        // Civ 0 starts far ahead but gains nothing; civ 1 starts at nothing
        // and gains more. The quest is about what you did, not what you had.
        w.civ_mut(CivId(0)).unwrap().faith.stored = 1000;
        w.civ_mut(CivId(1)).unwrap().faith.stored = 0;
        for (civ, baseline) in [(CivId(0), 1000), (CivId(1), 0)] {
            w.civ_mut(CS).unwrap().quests.active.push(Assigned {
                quest: q,
                assignee: civ,
                target: Target::Contest,
                contest: Some(Contest::Faith),
                assigned_on: 0,
                expires_on: 10,
                baseline,
            });
        }
        w.civ_mut(CivId(0)).unwrap().faith.stored = 1005; // +5
        w.civ_mut(CivId(1)).unwrap().faith.stored = 300; // +300
        w.turn = 10;

        let ahead = w.civ(CS).unwrap().quests.active[0].clone();
        let gained = w.civ(CS).unwrap().quests.active[1].clone();
        assert_eq!(judge(&w, &r, CS, &ahead), Verdict::False);
        assert_eq!(judge(&w, &r, CS, &gained), Verdict::True);
    }

    #[test]
    fn nothing_is_offered_before_the_game_has_started_properly() {
        let r = ruleset();
        let mut w = world(&r);
        w.turn = FIRST_QUEST_TURN - 1;
        assert!(tick(&mut w, &r).is_empty());
        assert!(w.civ(CS).unwrap().quests.active.is_empty());
    }

    #[test]
    fn quests_are_offered_paid_and_expire() {
        let r = ruleset();
        let mut w = world(&r);
        w.turn = FIRST_QUEST_TURN;
        let events = tick(&mut w, &r);
        assert!(
            events.iter().any(|e| matches!(e, QuestEvent::Offered { .. })),
            "a city-state should ask for something once the game is going"
        );
        let running = w.civ(CS).unwrap().quests.active.len();
        assert!(running > 0 && running <= MAX_ACTIVE * 2, "{running} quests running");

        // Nothing more until the cooldown is up.
        w.turn += 1;
        tick(&mut w, &r);
        assert_eq!(w.civ(CS).unwrap().quests.active.len(), running, "no double-dipping");

        // And everything expires eventually rather than standing forever.
        for t in 0..(DEFAULT_DURATION + 10) {
            w.turn = FIRST_QUEST_TURN + 1 + t;
            for e in tick(&mut w, &r) {
                if let QuestEvent::Completed { by, influence, .. } = e {
                    assert!(influence > 0, "a completed quest pays");
                    assert!(w.diplomacy.influence(CS, by) > Fixed::ZERO);
                }
            }
        }
        assert!(
            w.civ(CS).unwrap().quests.active.iter().all(|q| q.expires_on > w.turn),
            "nothing outlives its deadline"
        );
    }

    #[test]
    fn the_same_turn_offers_the_same_quests_twice() {
        let r = ruleset();
        let mut a = world(&r);
        a.turn = FIRST_QUEST_TURN;
        let mut b = a.clone();
        let first = tick(&mut a, &r);
        let second = tick(&mut b, &r);
        assert_eq!(format!("{first:?}"), format!("{second:?}"));
        assert_eq!(a, b, "quest assignment must not depend on iteration order");
    }

    #[test]
    fn a_major_civ_sets_no_quests() {
        let r = ruleset();
        let mut w = world(&r);
        w.turn = FIRST_QUEST_TURN;
        tick(&mut w, &r);
        assert!(w.civ(CivId(0)).unwrap().quests.active.is_empty());
        assert!(w.civ(CivId(1)).unwrap().quests.active.is_empty());
    }

    #[test]
    fn a_city_state_asks_nothing_of_someone_it_is_fighting() {
        let r = ruleset();
        let mut w = world(&r);
        w.diplomacy.declare_war(CivId(0), CS);
        w.turn = FIRST_QUEST_TURN;
        tick(&mut w, &r);
        assert!(
            w.civ(CS).unwrap().quests.active.iter().all(|q| q.assignee != CivId(0)),
            "you do not run errands for someone you are at war with"
        );
    }
}
