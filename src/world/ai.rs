//! Deciding what a civilization does.
//!
//! The only system here with no obligation to match Unciv. Rules are rules and
//! must agree; heuristics are opinions and may differ freely, so this is
//! written for clarity rather than fidelity — and a differential harness has
//! nothing to say about it.
//!
//! Everything is a pure function of `&World`. That is not incidental: the turn
//! loop's phase split means planning cannot mutate, which makes these testable
//! without a mutable world, swappable for a better opinion later, and runnable
//! for every civ at once. Unciv can do none of the three — it retrofitted
//! threading with 21 `synchronized` blocks.
//!
//! Every choice breaks ties on id. An AI that picked differently depending on
//! hash order would desync two machines running the same game.

use crate::arena::Id;
use crate::ruleset::{Policy, Ruleset, Technology};
use crate::stats::{Fixed, Stat};
use crate::world::assignment::ranking_weights;
use crate::world::city::City;
use crate::world::construction::{available, production_cost, Construction, CostModifiers};
use crate::world::grid::CivId;
use crate::world::policies::{is_adoptable, policy_cost, PolicyModifiers};
use crate::world::research::{available_techs, tech_cost, ScienceModifiers};
use crate::world::world::World;

/// What to research next.
///
/// Cheapest first. A crude opinion, but a defensible one: early techs unlock
/// the things later techs build on, so rushing an expensive one usually costs
/// more than it returns. Ties break on id.
pub fn choose_research(
    world: &World,
    r: &Ruleset,
    civ: CivId,
    m: ScienceModifiers,
) -> Option<Id<Technology>> {
    let cities = world.civ(civ).map_or(0, |c| c.cities.len());
    available_techs(r, world.civ(civ)?)
        .into_iter()
        .filter_map(|t| tech_cost(r, t, cities, m).map(|c| (c, t)))
        .min_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)))
        .map(|(_, t)| t)
}

/// What a city should build.
///
/// Scored as yield-per-hammer: what the thing produces, weighted the same way a
/// citizen's tile is weighted, divided by what it costs. Reusing
/// [`ranking_weights`] keeps one definition of "valuable" in the engine rather
/// than two that drift.
///
/// Units score zero under this, which is correct for a builder and wrong for a
/// general — a military AI belongs on top of this, not inside it.
///
/// `net_gold` is what the civ makes per turn after existing upkeep. Anything
/// costing more than that to keep is refused. Self-play otherwise reaches -168
/// gold in a hundred turns, because the rules only *penalise* debt — out of
/// research — and never prevent it. Staying solvent is an opinion, so it lives
/// here rather than in the rules.
pub fn choose_construction(
    world: &World,
    r: &Ruleset,
    civ: CivId,
    city: &City,
    m: CostModifiers,
    net_gold: i32,
    wants: Priorities,
) -> Option<Construction> {
    let Priorities { want_settler, want_worker, want_scout, want_military, happiness } = wants;
    let civ_ref = world.civ(civ)?;
    let weights = ranking_weights();

    available(r, civ_ref, city)
        .into_iter()
        .filter_map(|what| {
            let cost = production_cost(r, what, m)?;
            let value = match what {
                // A settler is worth more than any building while the empire
                // is small — land compounds and a granary does not.
                Construction::Unit(id) if crate::world::settling::is_settler(r, id) && want_settler => {
                    Fixed::whole(400)
                }
                // Worth less than a settler — land first, then make it
                // productive — but more than any single building.
                Construction::Unit(id) if is_worker(r, id) && want_worker => Fixed::whole(200),
                // Something — anything — that can walk and look. Cheap and
                // early: the point is eyes, not strength.
                Construction::Unit(id) if want_scout && !crate::world::settling::is_settler(r, id) && !is_worker(r, id) => {
                    let u = r.units.get(id)?;
                    let fast = u.movement as i32;
                    Fixed::whole(100 + fast * 10)
                }
                // Soldiers, scored by what they bring to a fight per hammer.
                Construction::Unit(id) if want_military => {
                    let u = r.units.get(id)?;
                    let strength = u.strength.unwrap_or(0).max(u.ranged_strength.unwrap_or(0));
                    if strength == 0 {
                        return None;
                    }
                    Fixed::whole(strength * 3)
                }
                Construction::Building(id) => {
                    let b = r.buildings.get(id)?;
                    let stats = crate::ruleset::queries::HasStats::stats(b);
                    let mut flat: Fixed =
                        Stat::ALL.into_iter().map(|s| stats[s].mul(weights[s])).sum();
                    // A percentage is worth something even with nothing to
                    // multiply yet, or nothing would ever build a Library.
                    let pct: i32 = b.percent_stat_bonus.values().sum();
                    flat = flat + Fixed::whole(pct);

                    // Upkeep is priced, not gated.
                    //
                    // This used to be a hard refusal: any building costing
                    // more upkeep than the civ's current surplus was struck
                    // off. That deadlocks. A poor civ cannot afford buildings,
                    // and it is poor *because* it has no buildings — measured
                    // over 250 turns, one civ ended with nineteen cities, one
                    // building between them and −71 happiness, which stops
                    // growth everywhere and ends the game as a slideshow.
                    //
                    // Charging the upkeep against the value instead lets a
                    // building earn its keep, and a civ already in the red
                    // feel it twice as sharply. Three times was enough to make
                    // every building in the ruleset net-negative for a civ in
                    // debt, which is the old deadlock wearing a new hat.
                    let pressure = if net_gold < 0 { Fixed::whole(2) } else { Fixed::ONE };
                    let upkeep =
                        Fixed::whole(b.maintenance.unwrap_or(0)).mul(weights.gold).mul(pressure);
                    flat = flat - upkeep;

                    // An unhappy empire is not merely poorer, it is stalled.
                    // Weight what fixes that by how bad it has got, so a
                    // colosseum beats a library exactly when it should.
                    if happiness < 0 && stats.happiness > Fixed::ZERO {
                        let urgency = Fixed::whole((-happiness).min(20));
                        flat = flat + stats.happiness.mul(urgency);
                    }
                    flat
                }
                Construction::Unit(_) => Fixed::ZERO,
            };
            let score = value.div(Fixed::whole(cost.max(1)))?;
            Some((score, what))
        })
        .filter(|(score, _)| *score > Fixed::ZERO)
        .max_by(|a, b| a.0.cmp(&b.0).then(b.1.cmp(&a.1)))
        .map(|(_, what)| what)
}

/// How many cities to aim for before stopping expansion, when happy.
///
/// The flat number is a floor on ambition, not the whole rule — see
/// [`wants_settler`], which stops sooner if the empire is already unhappy.
const TARGET_CITIES: usize = 5;

/// Whether the civ should be making a settler.
///
/// Only if it is below its target *and* somewhere legal exists to put one:
/// building a settler with nowhere to go wastes 106 production and two
/// citizens.
pub fn wants_settler(world: &World, r: &Ruleset, civ: CivId) -> bool {
    let Some(c) = world.civ(civ) else { return false };
    // A city-state is its one city and nothing more.
    if !c.is_major() {
        return false;
    }
    // An unhappy empire does not want another city. Each one costs happiness
    // outright, and below zero the ruleset cuts growth by three quarters and
    // then stops it entirely — so city eight in an unhappy empire does not
    // add an eighth of an empire, it freezes the other seven.
    //
    // A flat target alone produced civs with eight cities, two buildings
    // between them and −25 happiness, every city stuck at one citizen for two
    // hundred turns. This is the "weigh happiness" the constant's comment
    // asked for.
    if crate::world::happiness::happiness(world, r, civ, None) < 0 {
        return false;
    }
    if c.cities.len() >= TARGET_CITIES {
        return false;
    }
    // One in flight at a time.
    let settlers = c
        .units
        .iter()
        .filter_map(|u| world.units.get(*u))
        .filter(|u| crate::world::settling::is_settler(r, u.base))
        .count();
    if settlers > 0 {
        return false;
    }
    c.cities
        .iter()
        .filter_map(|id| world.cities.get(*id))
        .any(|city| crate::world::settling::best_site(world, r, civ, city.centre, 6).is_some())
}

/// What a settler should do this turn: walk toward the best site, or found.
pub fn settler_order(
    world: &World,
    r: &Ruleset,
    civ: CivId,
    unit: Id<crate::world::entities::Unit>,
) -> Option<UnitOrder> {
    let u = world.units.get(unit)?;
    if !crate::world::settling::is_settler(r, u.base) {
        return None;
    }
    // Standing somewhere legal? Then stop walking.
    if crate::world::settling::can_settle(world, r, civ, u.position).is_ok() {
        return Some(UnitOrder::FoundCity);
    }
    let (target, _) = crate::world::settling::best_site(world, r, civ, u.position, 8)?;
    // One step of the way, as far as this turn's movement allows.
    let reachable = crate::world::movement::reachable(&world.grid, r, u.position, u.movement_left, &[]);
    let step = reachable
        .keys()
        .copied()
        .filter(|t| *t != u.position)
        .min_by_key(|t| (world.grid.distance(*t, target), t.0))?;
    Some(UnitOrder::MoveTo(step))
}

/// Whether the civ should make a worker.
///
/// One per city, and only while there is unimproved land being worked. A
/// worker with nothing to do is 70 production spent on standing still.
pub fn wants_worker(world: &World, r: &Ruleset, civ: CivId) -> bool {
    let Some(c) = world.civ(civ) else { return false };
    let workers = c
        .units
        .iter()
        .filter_map(|u| world.units.get(*u))
        .filter(|u| is_worker(r, u.base))
        .count();
    if workers >= c.cities.len() {
        return false;
    }
    c.cities
        .iter()
        .filter_map(|id| world.cities.get(*id))
        .any(|city| !improvable_tiles(world, r, civ, city).is_empty())
}

/// Does this unit build improvements?
///
/// By the unique, not the name — the same reasoning as settlers and the
/// palace.
pub fn is_worker(r: &Ruleset, base: Id<crate::ruleset::BaseUnit>) -> bool {
    r.units.get(base).is_some_and(|u| {
        u.uniques
            .iter()
            .any(|x| x.unique_type() == Some(crate::UniqueType::BuildImprovements))
    })
}

/// Worked tiles that could be improved and are not, best first.
///
/// Worked tiles only: improving land nobody stands on helps nothing this
/// century.
/// How far out a city can work, and so how far a worker will go for it.
const CITY_WORK_RADIUS: u32 = 3;

/// What an improvement is worth beyond its yields, because it unlocks the
/// resource underneath.
///
/// A Plantation on Silk adds a little food. It also adds four happiness to the
/// entire empire, and an empire below zero happiness does not grow at all.
/// Valuing the tile by its yields alone is why a civ could hold four luxury
/// tiles, extract none of them, and sit at −29 happiness for two hundred
/// turns with every city stuck at one citizen.
///
/// A second copy of a luxury already held is worth nothing — happiness counts
/// distinct luxuries — so this asks what the civ has, not what is on the map.
fn resource_worth(
    world: &World,
    r: &Ruleset,
    civ: Option<&crate::world::entities::Civilization>,
    tile: crate::world::grid::TileId,
    improvement: Id<crate::ruleset::TileImprovement>,
) -> Fixed {
    let Some(civ) = civ else { return Fixed::ZERO };
    let Some(res_id) = world.grid.resource[tile.0 as usize] else { return Fixed::ZERO };
    let Some(res) = r.resources.get(res_id) else { return Fixed::ZERO };
    let Some(name) = r.improvements.name_of(improvement) else { return Fixed::ZERO };
    if !crate::world::resources::extracting_improvements(r, res_id).iter().any(|i| i == name) {
        return Fixed::ZERO;
    }
    let already = civ.resources.get(&res_id).copied().unwrap_or(0) > 0;
    match res.resource_type.as_str() {
        // The first of a luxury is the whole of its value.
        "Luxury" if !already => Fixed::whole(60),
        "Luxury" => Fixed::whole(2),
        // Strategic resources gate whole unit lines; a second deposit still
        // supports more of them, so this does not fall away to nothing.
        "Strategic" if !already => Fixed::whole(40),
        "Strategic" => Fixed::whole(10),
        _ => Fixed::ZERO,
    }
}

pub fn improvable_tiles(
    world: &World,
    r: &Ruleset,
    civ: CivId,
    city: &City,
) -> Vec<(crate::world::grid::TileId, Id<crate::ruleset::TileImprovement>)> {
    // Tiles a citizen works, plus every other tile the city owns in its
    // radius. The worked set alone misses the luxury two tiles over that
    // nobody is standing on — and an unextracted luxury is four happiness the
    // whole empire never gets.
    let mut candidates: Vec<crate::world::grid::TileId> = city.worked.clone();
    candidates.extend(
        world
            .grid
            .within(city.centre, CITY_WORK_RADIUS)
            .into_iter()
            .filter(|t| world.grid.owner[t.0 as usize] == Some(civ)),
    );
    candidates.sort();
    candidates.dedup();

    let civ_ref = world.civ(civ);
    let mut out: Vec<_> = candidates
        .iter()
        .filter(|t| world.grid.improvement[t.0 as usize].is_none())
        .filter_map(|t| {
            // Best by what it yields, weighted as a citizen would weigh it.
            let weights = ranking_weights();
            crate::world::improvements::buildable(world, r, civ, *t)
                .into_iter()
                .map(|i| {
                    let stats = crate::ruleset::queries::HasStats::stats(&r.improvements[i]);
                    let mut worth: Fixed =
                        Stat::ALL.into_iter().map(|s| stats[s].mul(weights[s])).sum();
                    worth = worth + resource_worth(world, r, civ_ref, *t, i);
                    (worth, i)
                })
                .filter(|(worth, _)| *worth > Fixed::ZERO)
                .max_by(|a, b| a.0.cmp(&b.0).then(b.1.cmp(&a.1)))
                .map(|(worth, i)| (worth, *t, i))
        })
        .collect();
    out.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    out.into_iter().map(|(_, t, i)| (t, i)).collect()
}

/// What a worker should do: walk to the best unimproved tile, or dig.
pub fn worker_order(
    world: &World,
    r: &Ruleset,
    civ: CivId,
    unit: Id<crate::world::entities::Unit>,
) -> Option<UnitOrder> {
    let u = world.units.get(unit)?;
    if !is_worker(r, u.base) {
        return None;
    }
    // A job already under way on this tile is finished first, whatever else
    // has become attractive since. Switching discards the progress, so a
    // worker that re-decides every turn can dig for a century and finish
    // nothing — which is exactly what it did before this line existed.
    if let Some((what, _)) = world.grid.improvement_progress[u.position.0 as usize] {
        if crate::world::improvements::can_build(world, r, civ, u.position, what).is_ok() {
            return Some(UnitOrder::Improve(what));
        }
    }

    let c = world.civ(civ)?;
    let mut jobs: Vec<_> = c
        .cities
        .iter()
        .filter_map(|id| world.cities.get(*id))
        .flat_map(|city| improvable_tiles(world, r, civ, city))
        .collect();

    // Connecting a city comes before improving tiles, and replaces the list
    // rather than joining the end of it.
    //
    // A trade route is worth several gold every turn for the rest of the
    // game; one farm is worth one food. Leaving roads as the job of last
    // resort means they are never done at all, because a growing city always
    // has one more unimproved tile — measured at 110 turns to join two cities
    // five tiles apart, against 19 — the optimum — once roads come first.
    if let Some(road) = r.improvements.id("Road") {
        let wanted = crate::world::connections::road_plan(world, r, civ);
        if !wanted.is_empty() {
            jobs = wanted.into_iter().map(|t| (t, road)).collect();
        }
    }

    // Already standing on a job? Dig.
    if let Some((_, imp)) = jobs.iter().find(|(t, _)| *t == u.position) {
        return Some(UnitOrder::Improve(*imp));
    }

    // A dig someone abandoned is worth more than a fresh one of equal value,
    // because the turns already spent are only kept if it is finished. The
    // worker has no memory of its own, so the half-dug tile is the memory —
    // without this two jobs at similar distance make it pace between them.
    let unfinished = world
        .grid
        .ids()
        .filter(|t| world.grid.improvement_progress[t.0 as usize].is_some())
        .filter(|t| {
            world.grid.improvement_progress[t.0 as usize]
                .is_some_and(|(what, _)| {
                    crate::world::improvements::can_build(world, r, civ, *t, what).is_ok()
                })
        })
        .min_by_key(|t| (world.grid.distance(u.position, *t), t.0));

    // Otherwise head for the nearest one, ties by tile id.
    let target = &match unfinished {
        Some(t) => t,
        None => jobs.iter().min_by_key(|(t, _)| (world.grid.distance(u.position, *t), t.0))?.0,
    };
    let reachable =
        crate::world::movement::reachable(&world.grid, r, u.position, u.movement_left, &[]);
    let step = reachable
        .keys()
        .copied()
        .filter(|t| *t != u.position)
        .min_by_key(|t| (world.grid.distance(*t, *target), t.0))?;
    Some(UnitOrder::MoveTo(step))
}

/// Send an idle military unit toward the nearest unexplored ground.
///
/// Without this a civilization never sees anything but its own land, so it
/// never meets a neighbour and the whole contested half of the game — contact,
/// diplomacy, war — cannot start. Three civs on one map behaved as three
/// separate games until something wandered.
pub fn explore_order(
    world: &World,
    r: &Ruleset,
    civ: CivId,
    unit: Id<crate::world::entities::Unit>,
) -> Option<UnitOrder> {
    let u = world.units.get(unit)?;
    // Settlers and workers have jobs; only the idle military wanders.
    if crate::world::settling::is_settler(r, u.base) || is_worker(r, u.base) {
        return None;
    }
    if !u.can_move() {
        return None;
    }

    let reachable =
        crate::world::movement::reachable(&world.grid, r, u.position, u.movement_left, &[]);

    // Step toward whichever reachable tile reveals the most that is unseen.
    // Ties break on tile id, so two machines wander identically.
    let (best, gain) = reachable
        .keys()
        .copied()
        .filter(|t| *t != u.position)
        .map(|t| {
            let unseen = world
                .grid
                .within(t, 2)
                .into_iter()
                .filter(|n| !world.visibility.has_explored(civ, *n))
                .count();
            (t, unseen)
        })
        .max_by(|a, b| a.1.cmp(&b.1).then(b.0.cmp(&a.0)))?;

    (gain > 0).then_some(UnitOrder::MoveTo(best))
}

/// Whether the civ should make something to explore with.
///
/// One early wanderer is enough to find the neighbours; after that the
/// military exists for other reasons.
pub fn wants_scout(world: &World, r: &Ruleset, civ: CivId) -> bool {
    let Some(c) = world.civ(civ) else { return false };
    let military = c
        .units
        .iter()
        .filter_map(|u| world.units.get(*u))
        .filter(|u| !crate::world::settling::is_settler(r, u.base) && !is_worker(r, u.base))
        .count();
    military == 0 && !c.cities.is_empty()
}

/// What a civ is short of this turn.
///
/// Gathered once per civ rather than threaded as separate flags: five
/// booleans in a signature is a sign they belong together, and it keeps the
/// decision about *what to want* in one place, separate from *what to build*.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Priorities {
    pub want_settler: bool,
    pub want_worker: bool,
    pub want_scout: bool,
    pub want_military: bool,
    /// The empire's happiness. Not a want but the thing most wants bend
    /// around: an unhappy empire stops growing, and a civ that cannot grow
    /// cannot earn its way out of anything.
    pub happiness: i32,
}

impl Priorities {
    pub fn assess(world: &World, r: &Ruleset, civ: CivId) -> Priorities {
        Priorities {
            want_settler: wants_settler(world, r, civ),
            want_worker: wants_worker(world, r, civ),
            want_scout: wants_scout(world, r, civ),
            want_military: wants_military(world, r, civ),
            happiness: crate::world::happiness::happiness(world, r, civ, None),
        }
    }
}

/// How much gold to send a city-state, and which one.
///
/// A major civ with money and no ally buys one. The bar is deliberately high —
/// influence decays a point a turn, so a gift that does not reach a threshold
/// is spent and gone — and reserves are kept back rather than emptied, because
/// a treasury at zero disbands the army.
///
/// Returns nothing when there is nothing worth buying: already allied
/// everywhere, or too poor to buy a friendship that would last.
pub fn city_state_gift(
    world: &World,
    r: &Ruleset,
    civ: CivId,
    game_length: i32,
    gold_gift_modifier: Fixed,
) -> Option<(CivId, i32)> {
    let c = world.civ(civ)?;
    if !c.is_major() {
        return None;
    }
    // Keep a working balance. Everything above it is diplomacy money.
    const RESERVE: i32 = 250;
    let spare = c.gold - RESERVE;
    if spare < 50 {
        return None;
    }

    // The city-state where the next threshold is closest, so the gold buys a
    // rank rather than a number. Ties by id.
    let mut best: Option<(Fixed, CivId)> = None;
    for cs in crate::world::city_states::city_states(world) {
        if world.diplomacy.at_war(cs, civ) {
            continue;
        }
        let have = world.diplomacy.influence(cs, civ);
        let target = if have < crate::world::city_states::FRIEND_THRESHOLD {
            crate::world::city_states::FRIEND_THRESHOLD
        } else if crate::world::city_states::ally_of(world, cs) != Some(civ) {
            crate::world::city_states::ALLY_THRESHOLD
        } else {
            continue; // already ours
        };
        let gap = target - have;
        if best.is_none_or(|(g, id)| gap < g || (gap == g && cs.0 < id.0)) {
            best = Some((gap, cs));
        }
    }
    let (gap, cs) = best?;

    // Smallest gift that closes the gap, capped by what we can spare. A gift
    // that falls short is worse than none — it decays away.
    let mut gold = 50;
    while gold < spare {
        let gained = crate::world::city_states::influence_from_gift(
            world,
            gold,
            game_length,
            gold_gift_modifier,
            civ,
        );
        if gained >= gap {
            return Some((cs, gold));
        }
        gold += 50;
    }
    let _ = r;
    None
}

/// Total fighting strength a civ can field.
///
/// Base strength, not effective — a threat assessment made on flat ground is
/// close enough, and computing terrain bonuses for every unit of every rival
/// every turn is not.
pub fn military_strength(world: &World, r: &Ruleset, civ: CivId) -> Fixed {
    world
        .civ(civ)
        .map(|c| {
            c.units
                .iter()
                .filter_map(|u| world.units.get(*u))
                .filter_map(|u| r.units.get(u.base))
                .filter_map(|b| b.strength)
                .map(Fixed::whole)
                .sum()
        })
        .unwrap_or(Fixed::ZERO)
}

/// How many military units the civ should keep.
///
/// Two per city as a floor, and more if a neighbour it has met is visibly
/// stronger. Crude, but it means a civ that ignores its army gets punished
/// rather than merely noticed.
pub fn target_military(world: &World, r: &Ruleset, civ: CivId) -> usize {
    let Some(c) = world.civ(civ) else { return 0 };
    let base = c.cities.len() * 2;
    let mine = military_strength(world, r, civ);
    let strongest_rival = world
        .civ_ids()
        .filter(|o| *o != civ && world.diplomacy.have_met(civ, *o))
        .map(|o| military_strength(world, r, o))
        .max()
        .unwrap_or(Fixed::ZERO);
    if strongest_rival > mine {
        base + 2
    } else {
        base
    }
}

pub fn wants_military(world: &World, r: &Ruleset, civ: CivId) -> bool {
    let Some(c) = world.civ(civ) else { return false };
    let have = c
        .units
        .iter()
        .filter_map(|u| world.units.get(*u))
        .filter(|u| {
            !crate::world::settling::is_settler(r, u.base)
                && !is_worker(r, u.base)
                && r.units.get(u.base).and_then(|b| b.strength).unwrap_or(0) > 0
        })
        .count();
    have < target_military(world, r, civ)
}

/// Whether to declare war on a civ already met.
///
/// Only with a clear advantage and something to take. An AI that declares war
/// it cannot win is worse than one that never declares at all, and a threshold
/// makes the decision legible: at 1.5x strength with an adjacent target,
/// attack.
pub fn should_declare_war(world: &World, r: &Ruleset, civ: CivId, other: CivId) -> bool {
    if !world.diplomacy.have_met(civ, other) || world.diplomacy.at_war(civ, other) {
        return false;
    }
    let (mine, theirs) = (military_strength(world, r, civ), military_strength(world, r, other));
    if mine < theirs.mul(Fixed::permille(1_500)) || mine.is_zero() {
        return false;
    }
    // Something of theirs must be within reach, or the war is a gesture.
    let their_cities: Vec<_> = world
        .cities
        .iter()
        .filter(|(_, c)| c.owner == other)
        .map(|(_, c)| c.centre)
        .collect();
    world
        .civ(civ)
        .map(|c| {
            c.units
                .iter()
                .filter_map(|u| world.units.get(*u))
                .any(|u| their_cities.iter().any(|t| world.grid.distance(u.position, *t) <= 8))
        })
        .unwrap_or(false)
}

/// Whether a civ would accept peace with an enemy.
///
/// Wars in this engine were permanent, which is why a three-hundred-turn game
/// saw forty-two cities change hands: nobody could ever stop. A war ends when
/// the losing side wants out and the winner is not actually winning.
///
/// Both conditions matter. Without the first, wars end arbitrarily; without
/// the second, a civ about to take a capital would hand it back.
pub fn wants_peace(world: &World, r: &Ruleset, civ: CivId, other: CivId) -> bool {
    if !world.diplomacy.at_war(civ, other) {
        return false;
    }
    // A fresh war has not been tested yet.
    let turns = world.diplomacy.pact(civ, other).map_or(0, |p| p.turns_in_stance);
    if turns < MINIMUM_WAR_LENGTH {
        return false;
    }

    let (mine, theirs) = (military_strength(world, r, civ), military_strength(world, r, other));
    // Losing badly: sue for peace.
    if mine.mul(Fixed::whole(2)) < theirs {
        return true;
    }
    // Winning clearly: press on.
    if mine > theirs.mul(Fixed::whole(2)) {
        return false;
    }
    // A grinding stalemate is worth ending.
    turns > STALEMATE_LENGTH
}

/// A war must run this long before anyone considers stopping.
const MINIMUM_WAR_LENGTH: u32 = 10;

/// And this long before an even fight counts as going nowhere.
const STALEMATE_LENGTH: u32 = 30;

/// What a soldier should do: hit something adjacent, march toward the enemy,
/// or hold the nearest city.
pub fn military_order(
    world: &World,
    r: &Ruleset,
    civ: CivId,
    unit: Id<crate::world::entities::Unit>,
) -> Option<UnitOrder> {
    let u = world.units.get(unit)?;
    if crate::world::settling::is_settler(r, u.base) || is_worker(r, u.base) {
        return None;
    }
    if !u.can_move() {
        return None;
    }

    // A city in reach is worth more than any unit beside it: killing an army
    // achieves nothing, taking a city ends a civilization.
    let adjacent_city = world
        .grid
        .neighbours(u.position)
        .find_map(|t| {
            world
                .cities
                .iter()
                .find(|(_, c)| {
                    c.centre == t && c.owner != civ && world.diplomacy.at_war(civ, c.owner)
                })
                .map(|(id, _)| id)
        });
    if let Some(city) = adjacent_city {
        return Some(UnitOrder::Assault(city));
    }

    // An adjacent enemy is the whole decision.
    let adjacent: Vec<_> = world
        .grid
        .neighbours(u.position)
        .flat_map(|t| world.units_at(t))
        .filter(|id| {
            world
                .units
                .get(*id)
                .is_some_and(|o| o.owner != civ && world.diplomacy.at_war(civ, o.owner))
        })
        .collect();
    if let Some(target) = adjacent.into_iter().min() {
        return Some(UnitOrder::Attack(target));
    }

    // At war and nothing to hit? Walk toward their nearest city.
    let enemy_cities: Vec<_> = world
        .cities
        .iter()
        .filter(|(_, c)| c.owner != civ && world.diplomacy.at_war(civ, c.owner))
        .map(|(_, c)| c.centre)
        .collect();
    if let Some(target) = enemy_cities
        .iter()
        .min_by_key(|t| (world.grid.distance(u.position, **t), t.0))
    {
        let reachable =
            crate::world::movement::reachable(&world.grid, r, u.position, u.movement_left, &[]);
        if let Some(step) = reachable
            .keys()
            .copied()
            .filter(|t| *t != u.position)
            .min_by_key(|t| (world.grid.distance(*t, *target), t.0))
        {
            return Some(UnitOrder::MoveTo(step));
        }
    }
    None
}

/// What a unit is told to do. Deliberately small: the turn loop applies these,
/// so anything here has to be expressible as a single reversible step.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnitOrder {
    MoveTo(crate::world::grid::TileId),
    FoundCity,
    Improve(Id<crate::ruleset::TileImprovement>),
    Attack(Id<crate::world::entities::Unit>),
    Assault(Id<crate::world::city::City>),
}

/// Which policy to adopt, if one is affordable.
///
/// Cheapest adoptable, which in practice means opening branches before
/// deepening them — a reasonable default, and the place a real opinion would go.
pub fn choose_policy(
    world: &World,
    r: &Ruleset,
    civ: CivId,
    m: PolicyModifiers,
) -> Option<Id<Policy>> {
    let c = world.civ(civ)?;
    let cost = policy_cost(c.adopted_policies.len(), c.cities.len(), m);
    if c.culture_stored < cost {
        return None;
    }
    r.policies
        .ids()
        .filter(|p| is_adoptable(world, r, civ, *p).is_ok())
        .min()
}

/// What a city should work toward, given what its owner lacks.
///
/// Food while small, production once established — the same shape as the
/// citizen-assignment focus, so the two do not pull against each other.
pub fn choose_focus(city: &City) -> crate::world::assignment::CityFocus {
    use crate::world::assignment::CityFocus;
    if city.population < 5 {
        CityFocus::None // the default already favours growth below size 5
    } else {
        CityFocus::ProductionGrowth
    }
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
        let mut g = TileGrid::new(16, 16, r.terrain("Grassland").unwrap());
        g.owner.iter_mut().for_each(|o| *o = Some(CivId(0)));
        let nation = r.nations.ids().next().unwrap();
        World::new(g, vec![Civilization::new(CivId(0), nation, false)])
    }

    #[test]
    fn research_starts_at_the_cheapest_available_tech() {
        let r = ruleset();
        let mut w = world(&r);
        let m = ScienceModifiers::default();

        let first = choose_research(&w, &r, CivId(0), m).expect("something is researchable");
        println!("first choice: {}", r.technologies.name_of(first).unwrap());
        // Only Agriculture has no prerequisites at the start.
        assert_eq!(r.technologies.name_of(first), Some("Agriculture"));

        w.civ_mut(CivId(0)).unwrap().techs.insert(first);
        let second = choose_research(&w, &r, CivId(0), m).expect("the tree widened");
        assert_ne!(second, first, "it does not pick what it already knows");
    }

    #[test]
    fn a_city_builds_something_worth_building() {
        let r = ruleset();
        let w = world(&r);
        let city = City::found("Rome", CivId(0), w.grid.id_at(8, 8).unwrap());
        let choice = choose_construction(&w, &r, CivId(0), &city, CostModifiers::default(), 10, Priorities::default())
            .expect("a new city can build something");
        match choice {
            Construction::Building(id) => println!("chose {}", r.buildings[id].name),
            Construction::Unit(id) => panic!("chose a unit: {}", r.units[id].name),
        }
        assert!(crate::world::construction::can_construct(&r, w.civ(CivId(0)).unwrap(), &city, choice).is_ok());
    }

    /// Poverty shifts the choice toward cheap upkeep; it does not forbid
    /// building.
    ///
    /// This used to assert that a civ with no income would only ever pick a
    /// building with zero maintenance. That rule deadlocks — a poor civ cannot
    /// buy the buildings that would make it rich — and produced empires of
    /// nineteen cities with one building between them. Upkeep is priced
    /// against the building's worth now, and priced three times as sharply
    /// when the books are already in the red.
    #[test]
    fn poverty_makes_upkeep_dear_rather_than_forbidden() {
        let r = ruleset();
        let w = world(&r);
        let city = City::found("Rome", CivId(0), w.grid.id_at(8, 8).unwrap());
        let m = CostModifiers::default();
        let upkeep = |c: Option<Construction>| match c {
            Some(Construction::Building(id)) => r.buildings[id].maintenance.unwrap_or(0),
            _ => 0,
        };

        let broke = choose_construction(&w, &r, CivId(0), &city, m, -10, Priorities::default());
        let rich = choose_construction(&w, &r, CivId(0), &city, m, 40, Priorities::default());
        assert!(broke.is_some(), "a poor civ still has to build something");
        assert!(rich.is_some());
        assert!(
            upkeep(broke) <= upkeep(rich),
            "being in debt should not make dear buildings more attractive"
        );
    }

    /// An unhappy empire reaches for what fixes that, ahead of what it would
    /// otherwise prefer.
    #[test]
    fn unhappiness_pulls_the_choice_toward_happiness() {
        let r = ruleset();
        let w = world(&r);
        let city = City::found("Rome", CivId(0), w.grid.id_at(8, 8).unwrap());
        let m = CostModifiers::default();

        let content = Priorities { happiness: 5, ..Default::default() };
        let miserable = Priorities { happiness: -15, ..Default::default() };
        let happiness_of = |c: Option<Construction>| match c {
            Some(Construction::Building(id)) => {
                crate::ruleset::queries::HasStats::stats(&r.buildings[id]).happiness
            }
            _ => Fixed::ZERO,
        };

        let a = choose_construction(&w, &r, CivId(0), &city, m, 20, content);
        let b = choose_construction(&w, &r, CivId(0), &city, m, 20, miserable);
        assert!(
            happiness_of(b) >= happiness_of(a),
            "a miserable empire should not pick the *less* cheering building"
        );
    }

    #[test]
    fn choices_are_the_same_every_time() {
        let r = ruleset();
        let w = world(&r);
        let city = City::found("Rome", CivId(0), w.grid.id_at(8, 8).unwrap());
        let m = CostModifiers::default();
        let s = ScienceModifiers::default();

        for _ in 0..5 {
            assert_eq!(
                choose_construction(&w, &r, CivId(0), &city, m, 10, Priorities::default()),
                choose_construction(&w, &r, CivId(0), &city, m, 10, Priorities::default())
            );
            assert_eq!(
                choose_research(&w, &r, CivId(0), s),
                choose_research(&w, &r, CivId(0), s)
            );
        }
    }

    #[test]
    fn a_policy_is_only_chosen_when_it_can_be_paid_for() {
        let r = ruleset();
        let mut w = world(&r);
        let m = PolicyModifiers::default();
        assert_eq!(choose_policy(&w, &r, CivId(0), m), None, "no culture yet");

        w.civ_mut(CivId(0)).unwrap().culture_stored = 1_000;
        let choice = choose_policy(&w, &r, CivId(0), m).expect("something is adoptable");
        assert!(is_adoptable(&w, &r, CivId(0), choice).is_ok());
        println!("policy: {}", r.policies.name_of(choice).unwrap());
    }

    #[test]
    fn planning_never_touches_the_world() {
        let r = ruleset();
        let w = world(&r);
        let before = w.clone();
        let city = City::found("Rome", CivId(0), w.grid.id_at(8, 8).unwrap());
        choose_research(&w, &r, CivId(0), ScienceModifiers::default());
        choose_construction(&w, &r, CivId(0), &city, CostModifiers::default(), 10, Priorities::default());
        choose_policy(&w, &r, CivId(0), PolicyModifiers::default());
        assert_eq!(w, before, "every decision is a pure read");
    }
}
