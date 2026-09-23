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
use crate::world::construction::{available, production_cost, production_cost_in, Construction, CostModifiers};
use crate::world::entities::Unit;
use crate::world::grid::{CivId, TileId};
use crate::world::policies::{is_adoptable, PolicyModifiers};
use crate::world::research::{available_techs, tech_cost_in, ScienceModifiers};
use crate::world::world::World;
use crate::UniqueType;

/// What to research next.
///
/// Cheapest first. A crude opinion, but a defensible one: early techs unlock
/// the things later techs build on, so rushing an expensive one usually costs
/// more than it returns. Ties break on id.
pub fn choose_research(world: &World, r: &Ruleset, civ: CivId, m: ScienceModifiers) -> Option<Id<Technology>> {
    available_techs(r, world.civ(civ)?)
        .into_iter()
        .filter_map(|t| tech_cost_in(world, r, civ, t, m).map(|c| (c, t)))
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
    // Computed once per call rather than once per candidate: it walks every
    // victory's milestone list, and this closure runs for every building in
    // the ruleset, in every city, every turn.
    let wins_the_game = victory_buildings(r);
    // What this city makes in a turn, for deciding whether a victory building
    // is a plan or a daydream.
    let golden = crate::world::golden_age::is_golden(world, civ);
    let current_output = crate::world::city::city_yield_in(world, r, city, golden);
    let production = current_output.production.floor().max(0) as i32;
    let city_id = world.cities.iter().find(|(_, c)| c.owner == civ && c.centre == city.centre).map(|(id, _)| id);

    // Once this city can finish the last outstanding milestone within the
    // existing build horizon, winning outranks marginal infrastructure yields.
    let winning = if production > 0 {
        wins_the_game.iter().filter_map(|&building| {
        let what=Construction::Building(building);
        crate::world::construction::can_construct_in(world,r,civ,city,what).ok()?;
        let cost=city_id.map_or_else(||production_cost(r,what,m),|id|production_cost_in(world,r,id,what,m))?;
        let remaining=cost.saturating_sub(city.production_for(what)).max(0);
        if (remaining+production-1)/production>VICTORY_BUILD_HORIZON {return None;}
        let ends=crate::world::victory::victories_needing(r,building).into_iter().filter_map(|id|r.victories.get(id)).any(|v|
            crate::world::victory::is_enabled(world,r,&v.name) && crate::world::victory::progress(world,r,civ,v).iter().all(|(milestone,verdict)|
                *verdict==crate::world::conditionals::Verdict::True || matches!(milestone,crate::world::victory::Milestone::BuiltBuilding(name) if *name==r.buildings[building].name)));
        ends.then_some((remaining,what))
    }).min_by_key(|(cost,what)|(*cost,*what)).map(|(_,what)|what)
    } else {
        None
    };
    if winning.is_some() {
        return winning;
    }

    available(r, civ_ref, city)
        .into_iter()
        .filter(|what| crate::world::construction::can_construct_in(world, r, civ, city, *what).is_ok())
        .filter_map(|what| {
            let cost =
                city_id.map_or_else(|| production_cost(r, what, m), |id| production_cost_in(world, r, id, what, m))?;
            let value = match what {
                // A settler is worth more than any building while the empire
                // is small — land compounds and a granary does not.
                // Settlers and workers need a job. Giving an unneeded worker
                // a small positive fallback score produced hundreds of idle
                // workers after the useful buildings were exhausted. Other
                // noncombat roles retain their own fallback below.
                Construction::Unit(id) => {
                    let u = r.units.get(id)?;
                    let strength = unit_strength(r, id);
                    if u.uniques.iter().any(|u| u.unique_type() == Some(UniqueType::AddInCapital)) {
                        crate::world::victory_actions::part_priority(world, r, civ, id)?
                    } else if crate::world::settling::is_settler(r, id) {
                        // Land compounds and a granary does not — but only
                        // while there is somewhere to put it.
                        if want_settler {
                            Fixed::whole(400)
                        } else {
                            return None;
                        }
                    } else if is_worker(r, id) {
                        if want_worker {
                            Fixed::whole(200)
                        } else {
                            return None;
                        }
                    } else if is_work_boat(r, id) {
                        let jobs = world
                            .grid
                            .ids()
                            .filter(|tile| {
                                world.grid.owner[tile.0 as usize] == Some(civ) && world.grid.is_water(r, *tile)
                            })
                            .filter(|tile| {
                                world.grid.resource[tile.0 as usize]
                                    .and_then(|res| r.resources[res].improvement.as_deref())
                                    .and_then(|name| r.improvements.id(name))
                                    .is_some_and(|imp| {
                                        crate::world::improvements::can_build(world, r, civ, *tile, imp).is_ok()
                                    })
                            })
                            .count();
                        let boats =
                            world.units.iter().filter(|(_, u)| u.owner == civ && is_work_boat(r, u.base)).count()
                                + world
                                    .cities
                                    .iter()
                                    .filter(|(_, c)| c.owner == civ)
                                    .flat_map(|(_, c)| &c.queue)
                                    .filter(|what| matches!(what,Construction::Unit(base) if is_work_boat(r,*base)))
                                    .count();
                        if jobs <= boats {
                            return None;
                        }
                        Fixed::whole(180)
                    } else if want_scout {
                        // Something — anything — that can walk and look. The
                        // point is eyes, not strength, so a warrior counts:
                        // narrowing this to weak units cost the early game its
                        // exploration and with it its first contacts.
                        Fixed::whole(100 + u.movement * 10)
                    } else if strength > 0 {
                        // A stronger unit uses the same frontline tile and
                        // maintenance slot as a scout. Value that strength as
                        // well as its cost; raw strength/cost ties Scout with
                        // Warrior and otherwise keeps obsolete armies forever.
                        if want_military {
                            military_value(strength)
                        } else {
                            return None;
                        }
                    } else {
                        // Something with a job rather than a fight: a work
                        // boat, a spaceship part, a missionary. Low enough to
                        // lose to anything with a present use, never zero,
                        // because zero means "does not exist".
                        Fixed::whole(5)
                    }
                }
                Construction::Building(id) => {
                    let b = r.buildings.get(id)?;
                    // Evaluate the same yields the city will receive. Many
                    // buildings (notably a Library's science per population)
                    // put their benefit in uniques rather than flat fields.
                    let mut hypothetical = city.clone();
                    hypothetical.built.insert(id);
                    crate::world::city::rebuild_city_uniques(r, &mut hypothetical);
                    let output = crate::world::city::city_yield_in(world, r, &hypothetical, golden);
                    let mut stats = crate::stats::Stats::ZERO;
                    for stat in Stat::ALL {
                        stats[stat] = output[stat] - current_output[stat];
                    }
                    let mut flat: Fixed = Stat::ALL.into_iter().map(|s| stats[s] * weights[s]).sum();

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
                    let upkeep = (Fixed::whole(b.maintenance.unwrap_or(0)) * weights.gold) * pressure;
                    flat -= upkeep;

                    // A building that wins the game is worth building even
                    // when it yields nothing at all — but only if this city
                    // can actually finish it.
                    //
                    // Scoring is otherwise entirely goal-agnostic: an Apollo
                    // Program produces no food, no hammers and no science, so
                    // it scored zero and was never built. A civ two techs from
                    // a science victory kept making soldiers, because the war
                    // made soldiers score well and the thing that ends the
                    // game scored nothing.
                    //
                    // The feasibility gate is not a refinement, it is the
                    // whole difference between working and not. Without it a
                    // one-hammer town commits to a fifteen-hundred-hammer
                    // wonder on turn ten and completes nothing at all for the
                    // rest of the game — which is what the first draft did.
                    if wins_the_game.contains(&id) && production > 0 {
                        let turns = cost / production;
                        if turns <= VICTORY_BUILD_HORIZON {
                            flat += victory_urgency(world, r, civ, id);
                        }
                    }

                    // An unhappy empire is not merely poorer, it is stalled.
                    // Weight what fixes that by how bad it has got, so a
                    // colosseum beats a library exactly when it should.
                    if happiness < 0 && stats.happiness > Fixed::ZERO {
                        let urgency = Fixed::whole((-happiness).min(20));
                        flat += stats.happiness * urgency;
                    }
                    flat
                }
            };
            let score = value.checked_div(Fixed::whole(cost.max(1)))?;
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
    if settlers > 0
        || c.cities.iter().filter_map(|id| world.cities.get(*id)).any(|city| {
            city.queue
                .iter()
                .any(|what| matches!(what, Construction::Unit(base) if crate::world::settling::is_settler(r, *base)))
        })
    {
        return false;
    }
    c.cities
        .iter()
        .filter_map(|id| world.cities.get(*id))
        .any(|city| crate::world::settling::has_site(world, r, civ, city.centre, 6))
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
    if crate::world::settling::can_found_with(world, r, unit).is_ok() {
        return Some(UnitOrder::FoundCity);
    }
    // The best site there is a way to, and one step of that way. A site
    // behind someone else's closed borders, or over water without Optics, is
    // no site at all; walking at it only strands the settler at the border.
    let step = crate::world::settling::ranked_sites(world, r, civ, u.position, 8)
        .into_iter()
        .filter(|(site, _)| crate::world::settling::can_found_on(world, r, u, *site).is_ok())
        .take(6)
        .find_map(|(site, _)| crate::world::movement::step_toward(world, r, unit, site))?;
    Some(UnitOrder::MoveTo(step))
}

/// Whether the civ should make a worker.
///
/// One per city, and only while there is unimproved land being worked. A
/// worker with nothing to do is 70 production spent on standing still.
pub fn wants_worker(world: &World, r: &Ruleset, civ: CivId) -> bool {
    let Some(c) = world.civ(civ) else { return false };
    let workers = c.units.iter().filter_map(|u| world.units.get(*u)).filter(|u| is_worker(r, u.base)).count()
        + c.cities
            .iter()
            .filter_map(|id| world.cities.get(*id))
            .flat_map(|city| &city.queue)
            .filter(|what| matches!(what, Construction::Unit(base) if is_worker(r, *base)))
            .count();
    if workers >= c.cities.len() {
        return false;
    }
    c.cities.iter().filter_map(|id| world.cities.get(*id)).any(|city| !improvable_tiles(world, r, civ, city).is_empty())
}

/// Does this unit build improvements?
///
/// By the unique, not the name — the same reasoning as settlers and the
/// palace.
pub fn is_worker(r: &Ruleset, base: Id<crate::ruleset::BaseUnit>) -> bool {
    r.units
        .get(base)
        .is_some_and(|u| u.uniques.iter().any(|x| x.unique_type() == Some(crate::UniqueType::BuildImprovements)))
}

fn is_work_boat(r: &Ruleset, base: Id<crate::ruleset::BaseUnit>) -> bool {
    r.units
        .get(base)
        .is_some_and(|u| u.uniques.iter().any(|u| u.unique_type() == Some(crate::UniqueType::CreateWaterImprovements)))
}

/// Worked tiles that could be improved and are not, best first.
///
/// Worked tiles only: improving land nobody stands on helps nothing this
/// century.
/// How many turns of production a victory building may take before the AI
/// treats it as out of reach. Beyond this it is not a goal, it is a way to
/// build nothing for the rest of the game.
const VICTORY_BUILD_HORIZON: i32 = 30;

/// How badly this civ wants a building that is a victory milestone.
///
/// Scaled by how near the relevant victory already is, and that scaling is the
/// whole design. A flat bonus is wrong in both directions: large enough to
/// matter when a civ is one milestone from winning, it also makes a civ with
/// no progress at all drop everything for a wonder it has no use for; small
/// enough to be safe early, it never changes the outcome late.
///
/// Rising from a nudge at no progress to overwhelming at the last milestone is
/// what makes it behave like a goal rather than a preference.
///
/// The nearness measured is *after* building it, not before. Measuring before
/// inverts the whole thing: the last milestone of a victory is unbuilt by
/// definition, so a civ one building from winning scores lowest of all. The
/// question is not "how close am I", it is "how close would this put me".
fn victory_urgency(world: &World, r: &Ruleset, civ: CivId, building: Id<crate::ruleset::Building>) -> Fixed {
    let mut best = Fixed::ZERO;
    for id in crate::world::victory::victories_needing(r, building) {
        let Some(v) = r.victories.get(id) else { continue };
        let (met, judgeable) = crate::world::victory::nearness(world, r, civ, v);
        if judgeable == 0 {
            continue;
        }
        // 50 at no progress, 1050 for the one that finishes it.
        let after = (met + 1).min(judgeable);
        let share = Fixed::whole(after as i32).checked_div(Fixed::whole(judgeable as i32)).unwrap_or(Fixed::ZERO);
        let value = Fixed::whole(50) + (share * Fixed::whole(1000));
        best = best.max(value);
    }
    best
}

/// Buildings that appear in some victory's milestones, cached per ruleset
/// pointer so scoring does not re-parse every milestone for every candidate in
/// every city on every turn.
fn victory_buildings(r: &Ruleset) -> std::collections::BTreeSet<Id<crate::ruleset::Building>> {
    crate::world::victory::victory_constructions(r).0.into_iter().collect()
}

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
        .filter(|t| world.grid.improvement[t.0 as usize].is_none() || world.grid.improvement_pillaged[t.0 as usize])
        .filter_map(|t| {
            // Best by what it yields, weighted as a citizen would weigh it.
            let weights = ranking_weights();
            crate::world::improvements::buildable(world, r, civ, *t)
                .into_iter()
                .map(|i| {
                    let stats = crate::ruleset::queries::HasStats::stats(&r.improvements[i]);
                    let mut worth: Fixed = Stat::ALL.into_iter().map(|s| stats[s] * weights[s]).sum();
                    if r.improvements[i].name == "Repair" {
                        worth += Fixed::whole(30);
                    }
                    if r.improvements[i].name.starts_with("Remove ") && !r.improvements[i].name.contains("Road") {
                        worth += Fixed::whole(2);
                    }
                    worth += resource_worth(world, r, civ_ref, *t, i);
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
/// A job for a worker: a tile, and what to build on it.
pub type WorkerJob = (crate::world::grid::TileId, Id<crate::ruleset::TileImprovement>);

/// Every job this civ's workers could take, in no particular order.
///
/// The same list for every worker of the civ — planning reads one world — so
/// the turn loop works it out once per civ and hands it to each
/// [`worker_order`], rather than each worker rebuilding it. With borders that
/// grow, a city has five times the tiles it was founded with, and rebuilding
/// the list per worker was most of the cost of a turn.
pub fn worker_jobs(world: &World, r: &Ruleset, civ: CivId) -> Vec<WorkerJob> {
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
            return wanted.into_iter().map(|t| (t, road)).collect();
        }
    }
    let Some(c) = world.civ(civ) else { return Vec::new() };
    c.cities
        .iter()
        .filter_map(|id| world.cities.get(*id))
        .flat_map(|city| improvable_tiles(world, r, civ, city))
        .collect()
}

pub fn worker_order(
    world: &World,
    r: &Ruleset,
    civ: CivId,
    unit: Id<crate::world::entities::Unit>,
    jobs: &std::cell::OnceCell<Vec<WorkerJob>>,
) -> Option<UnitOrder> {
    let step = |target| crate::world::movement::step_toward(world, r, unit, target);
    worker_order_with(world, r, civ, unit, jobs, WorkerSight { job: &|_| true, dig: &|_| true, step: &step })
}

/// What a worker may base its choice on. The AI reads the whole map; a
/// human's automated worker reads only what its civ knows
/// ([`crate::world::automation`]).
pub struct WorkerSight<'a> {
    /// Whether a job's tile may be considered.
    pub job: &'a dyn Fn(crate::world::grid::TileId) -> bool,
    /// Whether a half-dug tile may be seen as one.
    pub dig: &'a dyn Fn(crate::world::grid::TileId) -> bool,
    /// This turn's move toward a target, if there is one.
    pub step: &'a dyn Fn(crate::world::grid::TileId) -> Option<crate::world::grid::TileId>,
}

pub fn worker_order_with(
    world: &World,
    r: &Ruleset,
    civ: CivId,
    unit: Id<crate::world::entities::Unit>,
    jobs: &std::cell::OnceCell<Vec<WorkerJob>>,
    sight: WorkerSight,
) -> Option<UnitOrder> {
    let u = world.units.get(unit)?;
    if r.units[u.base]
        .uniques
        .iter()
        .any(|unique| unique.unique_type() == Some(crate::UniqueType::CreateWaterImprovements))
    {
        let mut targets: Vec<_> = world
            .grid
            .ids()
            .filter(|tile| world.grid.is_water(r, *tile) && (sight.job)(*tile))
            .filter_map(|tile| {
                world.grid.resource[tile.0 as usize]
                    .and_then(|res| r.resources[res].improvement.as_deref())
                    .and_then(|name| r.improvements.id(name))
                    .filter(|imp| crate::world::improvements::can_build(world, r, civ, tile, *imp).is_ok())
                    .map(|imp| (tile, imp))
            })
            .collect();
        targets.sort_by_key(|(tile, _)| (world.grid.distance(u.position, *tile), *tile));
        return targets.into_iter().find_map(|(tile, imp)| {
            if tile == u.position {
                Some(UnitOrder::Improve(imp))
            } else {
                (sight.step)(tile).map(UnitOrder::MoveTo)
            }
        });
    }
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

    let jobs = jobs.get_or_init(|| worker_jobs(world, r, civ));

    // Already standing on a job? Dig.
    if let Some((_, imp)) = jobs
        .iter()
        .find(|(t, imp)| *t == u.position && (sight.job)(*t) && crate::world::improvements::can_work(world, r, unit, *imp))
    {
        return Some(UnitOrder::Improve(*imp));
    }

    // A dig someone abandoned is worth more than a fresh one of equal value,
    // because the turns already spent are only kept if it is finished. The
    // worker has no memory of its own, so the half-dug tile is the memory —
    // without this two jobs at similar distance make it pace between them.
    let unfinished = world
        .grid
        .ids()
        .filter(|t| world.grid.improvement_progress[t.0 as usize].is_some() && (sight.dig)(*t))
        .filter(|t| {
            world.grid.improvement_progress[t.0 as usize]
                .is_some_and(|(what, _)| crate::world::improvements::can_build(world, r, civ, *t, what).is_ok())
        })
        .min_by_key(|t| (world.grid.distance(u.position, *t), t.0));

    // Otherwise head for the nearest one there is a way to, ties by tile id.
    let mut nearest: Vec<crate::world::grid::TileId> =
        jobs.iter().filter(|(t, _)| !world.grid.is_water(r, *t) && (sight.job)(*t)).map(|(t, _)| *t).collect();
    nearest.sort_by_key(|t| (world.grid.distance(u.position, *t), t.0));
    nearest.dedup();
    let step = unfinished
        .into_iter()
        .chain(nearest.into_iter().take(5))
        .find_map(|t| (sight.step)(t))?;
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
    if crate::world::settling::is_settler(r, u.base) || is_worker(r, u.base) || u.is_air(r) {
        return None;
    }
    if !u.can_move() {
        return None;
    }

    let reachable = crate::world::movement::reachable_for_unit(world, r, unit);

    // Ruins we can see and reach this turn come first
    // (`UnitAutomation.tryGoToRuinAndEncampment`), cheapest then by id.
    if world.civ(civ).is_some_and(|c| c.is_major()) {
        let ruin = reachable
            .iter()
            .filter(|(t, _)| {
                **t != u.position
                    && world.grid.improvement[t.0 as usize].is_some_and(|i| crate::world::ruins::is_ruins(r, i))
            })
            .filter(|(t, _)| world.visibility.can_see(civ, **t))
            .min_by(|a, b| a.1.cmp(b.1).then(a.0.cmp(b.0)));
        if let Some((t, _)) = ruin {
            return Some(UnitOrder::MoveTo(*t));
        }
    }

    // Step toward whichever reachable tile reveals the most that is unseen.
    // Ties break on tile id, so two machines wander identically.
    let (best, gain) = reachable
        .keys()
        .copied()
        .filter(|t| *t != u.position)
        .map(|t| {
            let unseen =
                world.grid.within(t, 2).into_iter().filter(|n| !world.visibility.has_explored(civ, *n)).count();
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

/// Which city-states to squeeze for tribute this turn, and whether for a
/// worker (`NextTurnAutomation.bullyCityStates`).
///
/// Unciv's rule: any city-state we know, are at peace with and are not yet
/// friends with, if it would pay — a worker when it would pay even that,
/// otherwise gold. Unciv also spares a city-state whose alliance it values
/// (`valueCityStateAlliance`, which weighs what each type of city-state
/// gives); here a city-state whose favour we already hold above nothing is
/// taken to be one we are courting, not robbing.
pub fn choose_tribute(world: &World, r: &Ruleset, civ: CivId) -> Vec<(CivId, bool)> {
    use crate::world::city_states::Relationship;
    if !world.civ(civ).is_some_and(|c| c.is_major() && c.is_ai) {
        return Vec::new();
    }
    let mut out = Vec::new();
    for cs in crate::world::city_states::city_states(world) {
        if !world.diplomacy.have_met(civ, cs) || world.diplomacy.at_war(civ, cs) {
            continue;
        }
        if crate::world::city_states::relationship(world, cs, civ) >= Relationship::Friend {
            continue;
        }
        if world.diplomacy.influence(cs, civ) > Fixed::ZERO {
            continue;
        }
        if crate::world::tribute::willingness(world, r, cs, civ, false) < 0 {
            continue;
        }
        out.push((cs, crate::world::tribute::willingness(world, r, cs, civ, true) > 0));
    }
    out
}

/// Trades to propose this turn: what we would give, and get, from whom.
///
/// Two of Unciv's AI habits (`TradeAutomation.exchangeLuxuries`,
/// `DiplomacyAutomation.offerOpenBorders`): a spare luxury for one we lack,
/// one for one, with anyone not an enemy; and open borders both ways with
/// anyone we think well of. Only other AIs are asked — a human answers
/// through the trade functions directly, not by being proposed to.
pub fn choose_trades(
    world: &World,
    r: &Ruleset,
    civ: CivId,
) -> Vec<(CivId, Vec<crate::world::trade::Item>, Vec<crate::world::trade::Item>)> {
    use crate::world::opinion::{relationship, RelationshipLevel};
    use crate::world::trade::Item;
    let _ = r;
    let Some(c) = world.civ(civ).filter(|c| c.is_major() && c.is_ai) else { return Vec::new() };
    let luxury =
        |id: &Id<crate::ruleset::TileResource>| r.resources.get(*id).is_some_and(|x| x.resource_type == "Luxury");
    let held = |civ: CivId, res: Id<crate::ruleset::TileResource>| {
        world.civ(civ).and_then(|c| c.resources.get(&res)).copied().unwrap_or(0)
    };
    let mut out = Vec::new();
    for other in world.civ_ids() {
        let Some(o) = world.civ(other) else { continue };
        if other == civ || !o.is_major() || !o.is_ai || !o.is_alive() || !world.diplomacy.have_met(civ, other) {
            continue;
        }
        if world.diplomacy.at_war(civ, other) || relationship(world, civ, other) <= RelationshipLevel::Enemy {
            continue;
        }
        let spare: Vec<_> = c
            .resources
            .keys()
            .copied()
            .filter(|res| luxury(res) && held(civ, *res) > 1 && held(other, *res) == 0)
            .collect();
        let wanted: Vec<_> = o
            .resources
            .keys()
            .copied()
            .filter(|res| luxury(res) && held(other, *res) > 1 && held(civ, *res) == 0)
            .collect();
        if let (Some(give), Some(get)) = (spare.first(), wanted.first()) {
            out.push((other, vec![Item::Luxury(*give)], vec![Item::Luxury(*get)]));
        }
        let both_may = c.uniques.has(crate::UniqueType::EnablesOpenBorders)
            && o.uniques.has(crate::UniqueType::EnablesOpenBorders);
        let open = world.diplomacy.may_enter_borders_of(civ, other) || world.diplomacy.may_enter_borders_of(other, civ);
        if both_may && !open && relationship(world, civ, other) >= RelationshipLevel::Favorable {
            out.push((other, vec![Item::OpenBorders], vec![Item::OpenBorders]));
        }
    }
    out
}

/// Whom to denounce this turn (`DiplomacyAutomation.denounce`).
///
/// Unciv's rule: denounce a civ whose standing with us has fallen fast — the
/// opinion against its thirty-turn average — by more than
/// `65 × 1.005^opinion × 1.1^(denunciations we already have running)`. The
/// first factor makes a friend's fall need to be steeper and an enemy's
/// shallower; the second makes an AI slow to denounce everyone at once. So a
/// civ drops from 65 to nothing, or from nothing to −50, before it speaks up.
/// Not at war, and not twice at once.
pub fn choose_denunciations(world: &World, r: &Ruleset, civ: CivId) -> Vec<CivId> {
    use crate::stats::Precise;
    use crate::world::diplomacy::Flag;
    let _ = r;
    if !world.civ(civ).is_some_and(|c| c.is_major() && c.is_ai) {
        return Vec::new();
    }
    let known: Vec<CivId> = world
        .civ_ids()
        .filter(|o| *o != civ && world.civ(*o).is_some_and(|c| c.is_major()) && world.diplomacy.have_met(civ, *o))
        .collect();
    let mut active = known.iter().filter(|o| world.diplomacy.has_flag(civ, **o, Flag::Denunciation)).count() as u32;
    let mut out = Vec::new();
    for other in known {
        if world.diplomacy.at_war(civ, other) || world.diplomacy.has_flag(civ, other, Flag::Denunciation) {
            continue;
        }
        let opinion = crate::world::opinion::opinion(world, civ, other);
        let Some(friendliness) = Precise::from_ratio(1005, 1000).pow(opinion) else { continue };
        let required = Precise::whole(-65) * friendliness * Precise::from_ratio(11, 10).powi(active);
        if Precise::from_fixed(crate::world::opinion::smoothed_delta(world, civ, other)) <= required {
            out.push(other);
            active += 1;
        }
    }
    out
}

/// How dangerous `assessed` looks to `assessor` (`Automation.threatAssessment`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ThreatLevel {
    VeryLow,
    Low,
    Medium,
    High,
    VeryHigh,
}

/// Their military might over ours: past double is very high, past one and a
/// half high, under a half very low, under two thirds low.
pub fn threat(world: &World, r: &Ruleset, assessor: CivId, assessed: CivId) -> ThreatLevel {
    let (theirs, ours) =
        (crate::world::tribute::might(world, r, assessed), crate::world::tribute::might(world, r, assessor).max(1));
    if theirs > 2 * ours {
        ThreatLevel::VeryHigh
    } else if 2 * theirs > 3 * ours {
        ThreatLevel::High
    } else if 2 * theirs < ours {
        ThreatLevel::VeryLow
    } else if 3 * theirs < 2 * ours {
        ThreatLevel::Low
    } else {
        ThreatLevel::Medium
    }
}

/// Fewest soldiers on our border, and least share of their army, that is
/// worth asking a human about (`MIN_UNITS_NEAR_BORDER_TO_ISSUE_DEMAND`,
/// `MIN_PERCENT_FORCE_VALUE_NEAR_BORDER_TO_ISSUE_DEMAND`).
const BORDER_UNITS: usize = 8;
const BORDER_FORCE_PERCENT: i64 = 50;

/// Humans whose armies sit on our border (`checkMilitaryPresenceNearBorder`).
///
/// Soldiers we can see, within two tiles of our land but not on it, at
/// least eight of them and at least half their army by force, from a civ
/// neither too weak to matter nor too strong to provoke. Friends are not
/// doubted: not those with a defensive pact, and not those we let in.
pub fn border_alarm(world: &World, r: &Ruleset, civ: CivId) -> Vec<CivId> {
    use crate::world::diplomacy::{Demand, Flag};
    let humans: Vec<CivId> = world
        .civ_ids()
        .filter(|o| *o != civ && world.civ(*o).is_some_and(|c| c.is_major() && !c.is_ai && c.is_alive()))
        .filter(|o| world.diplomacy.have_met(civ, *o) && !world.diplomacy.at_war(civ, *o))
        .filter(|o| !world.diplomacy.has_flag(civ, *o, Flag::Agreed(Demand::DoNotAttackUs)))
        .filter(|o| !world.diplomacy.pact(civ, *o).is_some_and(|p| p.defensive_pact))
        .filter(|o| !world.diplomacy.may_enter_borders_of(*o, civ))
        .collect();
    if humans.is_empty() || !world.civ(civ).is_some_and(|c| c.is_ai && c.is_major()) {
        return Vec::new();
    }
    let mut near = std::collections::BTreeSet::new();
    for t in 0..world.grid.len() {
        let t = TileId(t as u16);
        if world.grid.owner[t.0 as usize] != Some(civ) {
            continue;
        }
        for n in world.grid.within(t, 2) {
            if world.grid.owner[n.0 as usize] != Some(civ) && world.visibility.can_see(civ, n) {
                near.insert(n);
            }
        }
    }
    let mut out = Vec::new();
    for other in humans {
        let soldiers: Vec<&Unit> =
            world.units.iter().map(|(_, u)| u).filter(|u| u.owner == other && u.is_military(r)).collect();
        let nearby: Vec<&&Unit> = soldiers.iter().filter(|u| near.contains(&u.position)).collect();
        if nearby.len() < BORDER_UNITS {
            continue;
        }
        if matches!(threat(world, r, civ, other), ThreatLevel::VeryLow | ThreatLevel::VeryHigh) {
            continue;
        }
        let force = |u: &Unit| crate::world::tribute::unit_force(r, u);
        let nearby_force: i64 = nearby.iter().map(|u| force(u)).sum();
        let total: i64 = soldiers.iter().map(|u| force(u)).sum();
        if total * BORDER_FORCE_PERCENT <= nearby_force * 100 {
            out.push(other);
        }
    }
    out
}

/// What to ask of whom this turn (`NextTurnAutomation.issueRequests`).
///
/// Whatever we caught someone doing that they have not promised against and
/// we have not given up on — unless they are strong enough that asking would
/// only anger them. And any human massing troops on our border is asked
/// what it means.
pub fn choose_demands(world: &World, r: &Ruleset, civ: CivId) -> Vec<(CivId, crate::world::diplomacy::Demand)> {
    use crate::world::diplomacy::{Demand, Flag};
    if !world.civ(civ).is_some_and(|c| c.is_major() && c.is_ai) {
        return Vec::new();
    }
    let mut out = Vec::new();
    for other in world.civ_ids().filter(|o| *o != civ) {
        for d in Demand::ALL {
            let d_flag = |f: Flag| world.diplomacy.has_flag(civ, other, f);
            if !d_flag(Flag::Violated(d)) || d_flag(Flag::IgnoreViolation(d)) || d_flag(Flag::Agreed(d)) {
                continue;
            }
            if d != Demand::DoNotAttackUs && threat(world, r, civ, other) >= ThreatLevel::High {
                continue; // "don't piss them off for no reason please"
            }
            if crate::world::promises::may_demand(world, civ, other, d).is_ok() {
                out.push((other, d));
            }
        }
    }
    for other in border_alarm(world, r, civ) {
        if !out.contains(&(other, Demand::DoNotAttackUs))
            && crate::world::promises::may_demand(world, civ, other, Demand::DoNotAttackUs).is_ok()
        {
            out.push((other, Demand::DoNotAttackUs));
        }
    }
    out
}

/// How an AI answers what it was asked (`respondToDemandAlert`): it gives in
/// to anyone it fears, and to no one else.
pub fn answer_demands(world: &World, r: &Ruleset, civ: CivId) -> Vec<(CivId, crate::world::diplomacy::Demand, bool)> {
    if !world.civ(civ).is_some_and(|c| c.is_major() && c.is_ai) {
        return Vec::new();
    }
    crate::world::promises::asked_of(world, civ)
        .into_iter()
        .map(|(by, d)| (by, d, threat(world, r, civ, by) >= ThreatLevel::High))
        .collect()
}

/// What a great person does this turn (`CivilianUnitAutomation`, the
/// great-person half).
///
/// Unciv's order: late in the game — past 55% of the tech tree — a
/// Scientist hurries research and a Merchant walks to a city-state to trade;
/// an Engineer walks to a city building a wonder and hurries it; failing
/// those, it builds its landmark on the best tile it can find
/// (`automateImprovementPlacer`); and an Artist with nowhere to put a
/// Landmark starts a golden age. Prophets and generals have work of their
/// own and are not handled here.
pub fn great_person_order(
    world: &World,
    r: &Ruleset,
    civ: CivId,
    unit: Id<Unit>,
    s: crate::world::great_person_actions::Scaling,
) -> Option<UnitOrder> {
    use crate::world::great_person_actions::{abilities, can, Action};
    let u = world.units.get(unit)?;
    let b = r.units.get(u.base)?;
    let has = |t: UniqueType| b.uniques.iter().any(|x| x.unique_type() == Some(t));
    if !has(UniqueType::GreatPerson)
        || has(UniqueType::CanSpreadReligion)
        || has(UniqueType::MayFoundReligion)
        || has(UniqueType::StrengthBonusInRadius)
    {
        return None;
    }
    let c = world.civ(civ)?;
    let able = abilities(r, u);
    let here = |a: Action| able.contains(&a) && can(world, r, unit, a, s).is_ok();
    let act = |a: Action| Some(UnitOrder::GreatPerson(a));
    let late = c.techs.len() * 100 >= 55 * r.technologies.len();

    if late && here(Action::HurryResearch) {
        return act(Action::HurryResearch);
    }
    if late && able.contains(&Action::TradeMission) {
        if here(Action::TradeMission) {
            return act(Action::TradeMission);
        }
        let target = (0..world.grid.len())
            .map(|t| TileId(t as u16))
            .filter(|t| {
                world.grid.owner[t.0 as usize].is_some_and(|o| {
                    world.civ(o).is_some_and(|x| x.is_city_state())
                        && world.diplomacy.have_met(civ, o)
                        && !world.diplomacy.at_war(civ, o)
                })
            })
            .filter(|t| !world.cities.iter().any(|(_, city)| city.centre == *t))
            .min_by_key(|t| (world.grid.distance(u.position, *t), t.0));
        if let Some(t) = target {
            return Some(UnitOrder::MoveTo(t));
        }
    }
    if able.contains(&Action::HurryProduction) {
        let wonder = c
            .cities
            .iter()
            .filter_map(|id| world.cities.get(*id).map(|city| (*id, city)))
            .filter(|(_, city)| {
                matches!(city.queue.first(), Some(Construction::Building(b))
                    if r.buildings.get(*b).is_some_and(|b| b.is_wonder == Some(true)))
            })
            .min_by_key(|(id, city)| (world.grid.distance(u.position, city.centre), id.index()));
        if let Some((_, city)) = wonder {
            if u.position == city.centre && here(Action::HurryProduction) {
                return act(Action::HurryProduction);
            }
            if u.position != city.centre {
                return Some(UnitOrder::MoveTo(city.centre));
            }
        }
    }
    if let Some(&Action::Construct(imp)) = able.iter().find(|a| matches!(a, Action::Construct(_))) {
        if let Some(tile) = landmark_site(world, r, civ, imp) {
            if u.position == tile {
                if here(Action::Construct(imp)) {
                    return act(Action::Construct(imp));
                }
            } else {
                return Some(UnitOrder::MoveTo(tile));
            }
        }
    }
    if able.contains(&Action::GoldenAge) {
        return act(Action::GoldenAge);
    }
    None
}

/// Where a great improvement would do most (`automateImprovementPlacer`).
///
/// Cities in order of how much they multiply the stat it yields most; within
/// the first that has room, the best tile it could work, inside its borders,
/// not already holding a great improvement, and on better ground than the
/// average land terrain — a landmark on tundra wastes the person.
pub fn landmark_site(
    world: &World,
    r: &Ruleset,
    civ: CivId,
    improvement: Id<crate::ruleset::TileImprovement>,
) -> Option<TileId> {
    use crate::ruleset::queries::HasStats;
    let imp = r.improvements.get(improvement)?;
    let stats = imp.stats();
    let stat = Stat::ALL.into_iter().max_by_key(|s| (stats[*s], std::cmp::Reverse(*s as u8))).unwrap_or(Stat::Culture);
    let weights = ranking_weights();
    let value = |st: crate::stats::Stats| Stat::ALL.into_iter().map(|s| st[s] * weights[s]).sum::<Fixed>();
    let land: Vec<Fixed> =
        r.terrains.iter().filter(|(_, t)| t.r#type == "Land").map(|(_, t)| value(t.stats())).collect();
    let average = if land.is_empty() {
        Fixed::ZERO
    } else {
        land.iter().copied().sum::<Fixed>().checked_div(Fixed::whole(land.len() as i32)).unwrap_or(Fixed::ZERO)
    };
    let great = |t: TileId| {
        world.grid.improvement[t.0 as usize]
            .and_then(|i| r.improvements.get(i))
            .is_some_and(|i| i.uniques.iter().any(|u| u.unique_type() == Some(UniqueType::GreatImprovement)))
    };
    let mut cities: Vec<(Fixed, Id<City>)> = world
        .civ(civ)?
        .cities
        .iter()
        .filter_map(|id| world.cities.get(*id).map(|c| (*id, c)))
        .map(|(id, c)| {
            let bonus = crate::world::city::city_percent_bonuses(r, c)
                .get(&stat)
                .map_or(Fixed::whole(100), |s| s.evaluate(Fixed::whole(100)));
            (bonus, id)
        })
        .collect();
    cities.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.index().cmp(&b.1.index())));
    for (_, id) in cities {
        let Some(city) = world.cities.get(id) else { continue };
        let mut tiles: Vec<(Fixed, TileId)> = world
            .grid
            .within(city.centre, 3)
            .into_iter()
            .filter(|t| world.grid.owner[t.0 as usize] == Some(civ) && !great(*t))
            .filter(|t| crate::world::great_person_actions::can_place(world, r, civ, *t, improvement).is_ok())
            .filter(|t| {
                r.terrains.get(world.grid.terrain[t.0 as usize]).is_some_and(|base| value(base.stats()) > average)
            })
            .map(|t| (crate::world::settling::tile_worth(world, r, t), t))
            .collect();
        tiles.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
        if let Some((_, t)) = tiles.first() {
            return Some(*t);
        }
    }
    None
}

/// Whether to burn a city just taken (`Battle.automateCityConquer`).
///
/// Unciv's rule: a small one — under four citizens — or any city at all if
/// the conqueror is a city-state, provided it was not the conqueror's own and
/// may be razed. Everything else is kept as a puppet. Unciv's AI also frees a
/// city whose founder is a city-state worth allying with; nothing here frees
/// a city yet, so that one is kept too.
pub fn should_raze(world: &World, r: &Ruleset, civ: CivId, city: Id<City>) -> bool {
    let _ = r;
    let Some(c) = world.cities.get(city) else { return false };
    let city_state = world.civ(civ).is_some_and(|x| x.is_city_state());
    (c.population < 4 || city_state) && c.founder != civ && crate::world::conquest::can_raze(world, city).is_ok()
}

/// What to buy outright with gold this turn (`UseGoldAutomation.useGold`).
///
/// Unciv's rule, kept as it is: after any city-state gift, go through the
/// cities from the most productive down and buy each one's current build if
/// the treasury covers it — unless work has already gone into it, which
/// buying would throw away, or its `hurryCostModifier` is above a hundredth of
/// the treasury. That last clause is how a civ that keeps saving starts
/// accepting worse prices rather than hoarding forever.
///
/// `production` is the planned hammers per city, which is what orders them;
/// ties by city id.
pub fn choose_purchases(
    world: &World,
    r: &Ruleset,
    civ: CivId,
    costs: CostModifiers,
    gift: i32,
    production: &[(Id<City>, i32)],
) -> Vec<(Id<City>, Construction)> {
    let Some(c) = world.civ(civ) else { return Vec::new() };
    let mut gold = (c.gold - gift) as i64;
    let acceptable_hurry = gold.max(0) / 100;
    let mut order: Vec<(Id<City>, i32)> = production.to_vec();
    order.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));

    let mut out = Vec::new();
    for (city_id, _) in order {
        let Some(city) = world.cities.get(city_id) else { continue };
        let Some(&what) = city.queue.first() else { continue };
        if city.production_for(what) > 0 {
            continue;
        }
        let hurry = match what {
            Construction::Unit(id) => r.units.get(id).and_then(|u| u.hurry_cost_modifier),
            Construction::Building(id) => r.buildings.get(id).and_then(|b| b.hurry_cost_modifier),
        };
        if hurry.unwrap_or(0) as i64 > acceptable_hurry {
            continue;
        }
        let Ok(cost) = crate::world::purchase::can_purchase(world, r, city_id, what, Stat::Gold, costs) else {
            continue;
        };
        if gold < cost {
            continue;
        }
        gold -= cost;
        out.push((city_id, what));
    }
    out
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
        let gained =
            crate::world::city_states::influence_from_gift(world, gold, game_length, gold_gift_modifier, civ, cs);
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
            c.units.iter().filter_map(|u| world.units.get(*u)).map(|u| unit_strength(r, u.base)).map(Fixed::whole).sum()
        })
        .unwrap_or(Fixed::ZERO)
}

fn unit_strength(r: &Ruleset, base: Id<crate::ruleset::BaseUnit>) -> i32 {
    r.units.get(base).map_or(0, |u| u.strength.unwrap_or(0).max(u.ranged_strength.unwrap_or(0))).max(0)
}

fn military_value(strength: i32) -> Fixed {
    // Preserve the old Warrior value (24) while rewarding stronger units
    // for limited frontage. All scoring stays in deterministic fixed point.
    Fixed::whole(strength) * Fixed::whole(strength) * Fixed::permille(375)
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
    // Size the force in current soldiers' strength, not a count in which an
    // ancient scout permanently substitutes for a modern infantry unit.
    // Legality matters: unavailable iron/oil must not create an unreachable
    // target that drains every city's production into weaker replacements.
    let best = r
        .units
        .ids()
        .filter(|base| unit_strength(r, *base) > 0)
        .filter_map(|base| {
            let what = Construction::Unit(base);
            let cost = c
                .cities
                .iter()
                .filter_map(|id| {
                    let city = world.cities.get(*id)?;
                    crate::world::construction::can_construct_in(world, r, civ, city, what).ok()?;
                    production_cost_in(world, r, *id, what, CostModifiers::default())
                })
                .min()?;
            Some((military_value(unit_strength(r, base)).checked_div(Fixed::whole(cost.max(1)))?, base))
        })
        .max_by(|a, b| a.0.cmp(&b.0).then(b.1.cmp(&a.1)))
        .map(|(_, base)| unit_strength(r, base));
    let Some(strength) = best else { return false };
    let queued: Fixed = c
        .cities
        .iter()
        .filter_map(|id| world.cities.get(*id))
        .flat_map(|city| &city.queue)
        .filter_map(|what| match what {
            Construction::Unit(base) => Some(Fixed::whole(unit_strength(r, *base))),
            _ => None,
        })
        .sum();
    military_strength(world, r, civ) + queued
        < Fixed::whole(strength) * Fixed::whole(target_military(world, r, civ) as i32)
}

/// Whether to declare war on a civ already met.
///
/// Only with a clear advantage and something to take. An AI that declares war
/// it cannot win is worse than one that never declares at all, and a threshold
/// makes the decision legible: at 1.5x strength with an adjacent target,
/// attack.
pub fn should_declare_war(world: &World, r: &Ruleset, civ: CivId, other: CivId) -> bool {
    if !world.diplomacy.have_met(civ, other) || !world.diplomacy.can_declare_war(civ, other) {
        return false;
    }
    let (mine, theirs) = (military_strength(world, r, civ), military_strength(world, r, other));
    // How much stronger we must be depends on what we think of them: half
    // again for a neutral, as much as twice for an ally, merely equal for the
    // unforgivable. Unciv adds the same ten-to-minus-ten relationship term to
    // its war motivation (`MotivationToAttackAutomation.getRelationshipModifier`).
    use crate::world::opinion::RelationshipLevel as L;
    let needed = match crate::world::opinion::relationship(world, civ, other) {
        L::Unforgivable => Fixed::ONE,
        L::Enemy => Fixed::permille(1_250),
        L::Competitor => Fixed::permille(1_400),
        L::Neutral => Fixed::permille(1_500),
        L::Favorable => Fixed::permille(1_600),
        L::Friend => Fixed::permille(1_750),
        L::Ally => Fixed::whole(2),
    };
    if mine < theirs * needed || mine.is_zero() {
        return false;
    }
    // Something of theirs must be within reach, or the war is a gesture.
    let their_cities: Vec<_> = world.cities.iter().filter(|(_, c)| c.owner == other).map(|(_, c)| c.centre).collect();
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
    if (mine * Fixed::whole(2)) < theirs {
        return true;
    }
    // Winning clearly: press on.
    if mine > (theirs * Fixed::whole(2)) {
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

    if u.has_unique(r, crate::UniqueType::NuclearWeapon) {
        if let Some(target) = crate::world::nuclear::order(world, r, unit) {
            return Some(UnitOrder::NuclearStrike(target));
        }
    }
    if u.is_air(r) {
        return crate::world::air::order(world, r, unit);
    }
    if let Some(to) = crate::world::air::paradrop_order(world, r, unit) {
        return Some(UnitOrder::Paradrop(to));
    }

    // Use the same target restrictions and range as command execution. Siege
    // setup is scheduled by the turn adapter before an otherwise legal shot.
    let range = crate::world::air::attack_range(world, r, unit);
    let can_reach = |tile| {
        world.grid.distance(u.position, tile) <= range as u32
            && (!u.is_ranged(r)
                || crate::world::combat::has_line_of_sight(
                    &world.grid,
                    r,
                    u.position,
                    tile,
                    u.has_unique(r, crate::UniqueType::IndirectFire),
                ))
    };
    if u.attacks_left > 0 {
        if let Some((city, _)) = world
            .cities
            .iter()
            .filter(|(id, city)| {
                city.owner != civ
                    && world.diplomacy.at_war(civ, city.owner)
                    && world.visibility.can_see(civ, city.centre)
                    && can_reach(city.centre)
                    && crate::world::military::may_attack(world, r, unit, crate::world::air::Target::City(*id))
            })
            .min_by_key(|(id, city)| (city.health, *id))
        {
            return Some(UnitOrder::Assault(city));
        }
        if let Some((target, _)) = world
            .units
            .iter()
            .filter(|(id, other)| {
                other.owner != civ
                    && world.diplomacy.at_war(civ, other.owner)
                    && crate::world::fog::can_see_unit(world, r, civ, *id)
                    && can_reach(other.position)
                    && crate::world::military::may_attack(world, r, unit, crate::world::air::Target::Unit(*id))
            })
            .min_by_key(|(id, other)| (other.health, *id))
        {
            return Some(UnitOrder::Attack(target));
        }
    }

    // At war and nothing to hit? Walk toward their nearest city.
    let enemy_cities: Vec<_> = world
        .cities
        .iter()
        .filter(|(_, c)| c.owner != civ && world.diplomacy.at_war(civ, c.owner))
        .map(|(_, c)| c.centre)
        .collect();
    let mut targets = enemy_cities;
    targets.sort_by_key(|t| (world.grid.distance(u.position, *t), t.0));
    targets
        .into_iter()
        .take(3)
        .find_map(|t| crate::world::movement::step_toward(world, r, unit, t))
        .map(UnitOrder::MoveTo)
}

/// What a unit is told to do. Deliberately small: the turn loop applies these,
/// so anything here has to be expressible as a single reversible step.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnitOrder {
    MoveTo(crate::world::grid::TileId),
    Rebase(crate::world::grid::TileId),
    AirSweep(crate::world::grid::TileId),
    NuclearStrike(crate::world::grid::TileId),
    Paradrop(crate::world::grid::TileId),
    FoundCity,
    Improve(Id<crate::ruleset::TileImprovement>),
    Attack(Id<crate::world::entities::Unit>),
    Assault(Id<crate::world::city::City>),
    /// A great person's one act, where it stands.
    GreatPerson(crate::world::great_person_actions::Action),
    Religious(crate::world::religious_units::ReligiousAction),
}

/// Which policy to adopt, if one is affordable.
///
/// Cheapest adoptable, which in practice means opening branches before
/// deepening them — a reasonable default, and the place a real opinion would go.
pub fn choose_policy(world: &World, r: &Ruleset, civ: CivId, m: PolicyModifiers) -> Option<Id<Policy>> {
    let c = world.civ(civ)?;
    let cost = crate::world::policies::policy_cost_in(world, r, civ, m);
    if c.culture_stored < cost && c.pending_rewards.free_policies == 0 {
        return None;
    }
    r.policies.ids().filter(|p| is_adoptable(world, r, civ, *p).is_ok()).min()
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
        Ruleset::load_dir("rules/gods-and-kings").expect("readable").expect("loads")
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

    #[test]
    fn idle_workers_do_not_outrank_a_libraries_population_science() {
        let mut r = ruleset();
        let library = r.buildings.id("Library").unwrap();
        // Isolate the real late-game choice: a useful population-scaled
        // building and units whose jobs are already covered.
        for id in r.buildings.ids().collect::<Vec<_>>() {
            if id != library {
                r.buildings.get_mut(id).unwrap().cost = None;
            }
        }
        let mut w = world(&r);
        let centre = w.grid.id_at(8, 8).unwrap();
        let city_id = w.found_city_in(&r, City::found("Established", CivId(0), centre));
        w.cities.get_mut(city_id).unwrap().population = 9;
        for name in ["Agriculture", "Writing"] {
            w.civ_mut(CivId(0)).unwrap().techs.insert(r.technologies.id(name).unwrap());
        }
        for tile in w.grid.ids() {
            w.grid.improvement[tile.0 as usize] = r.improvements.id("Farm");
        }
        for tile in 0..10 {
            w.place_unit(Unit::new(r.units.id("Worker").unwrap(), CivId(0), TileId(tile), 2));
        }
        assert!(!wants_worker(&w, &r, CivId(0)));
        assert!(worker_jobs(&w, &r, CivId(0)).is_empty());
        let before = w.clone();
        let choice = choose_construction(
            &w,
            &r,
            CivId(0),
            w.cities.get(city_id).unwrap(),
            CostModifiers::default(),
            3,
            Priorities { happiness: -3, ..Default::default() },
        );
        assert_eq!(choice, Some(Construction::Building(library)));
        assert_eq!(w, before, "hypothetical yields must not change the city");

        // Exercise repeated real planning and production after the first
        // choice. A city-state keeps this fixture about one established city.
        w.civ_mut(CivId(0)).unwrap().city_state_type = r.city_state_types.ids().next();
        // Seed the construction selected above; work belongs to this item
        // even if subsequent choices change.
        w.cities.get_mut(city_id).unwrap().queue.push(choice.unwrap());
        w.cities.get_mut(city_id).unwrap().production_progress.insert(choice.unwrap(), 500);
        for _ in 0..30 {
            let mods = crate::world::turn::TurnModifiers::default();
            let plan =
                crate::world::turn::plan_civ_turn(&w, &r, CivId(0), crate::world::assignment::CityFocus::None, mods);
            crate::world::turn::apply_plan(&mut w, &r, &plan, mods);
            crate::world::movement::refresh_movement(&mut w, &r);
            w.turn += 1;
        }
        assert!(w.cities.get(city_id).unwrap().built.contains(&library));
        let workers = w.units.iter().filter(|(_, unit)| unit.owner == CivId(0) && is_worker(&r, unit.base)).count();
        assert_eq!(workers, 10, "thirty rounds must not grow an idle workforce");
    }

    #[test]
    fn scouts_do_not_fill_an_established_armys_strength_budget() {
        let r = ruleset();
        let mut w = world(&r);
        let centre = w.grid.id_at(8, 8).unwrap();
        let city_id = w.found_city_in(&r, City::found("Rome", CivId(0), centre));
        for name in ["Agriculture", "Bronze Working", "Civil Service", "Iron Working"] {
            w.civ_mut(CivId(0)).unwrap().techs.insert(r.technologies.id(name).unwrap());
        }
        for tile in 0..4 {
            w.place_unit(Unit::new(r.units.id("Scout").unwrap(), CivId(0), TileId(tile), 2));
        }
        assert!(wants_military(&w, &r, CivId(0)), "four scouts do not replace two pikemen");
        let city = w.cities.get(city_id).unwrap();
        let choice = choose_construction(
            &w,
            &r,
            CivId(0),
            city,
            CostModifiers::default(),
            10,
            Priorities { want_military: true, ..Default::default() },
        )
        .unwrap();
        let Construction::Unit(base) = choice else { panic!("needed a fighting unit: {choice:?}") };
        assert!(unit_strength(&r, base) >= 11, "an established army should train a stronger unit");
        assert!(crate::world::construction::can_construct_in(&w, &r, CivId(0), city, choice).is_ok());
        assert!(crate::world::resources::resource_requirements(&r, base).is_empty(), "no iron on this map");
        w.cities.get_mut(city_id).unwrap().queue.push(choice);
        assert!(!wants_military(&w, &r, CivId(0)), "the queued reinforcement already covers the deficit");
    }

    #[test]
    fn queued_workers_and_settlers_count_toward_their_jobs() {
        let r = ruleset();
        let mut w = world(&r);
        let centre = w.grid.id_at(8, 8).unwrap();
        let city_id = w.found_city_in(&r, City::found("Rome", CivId(0), centre));
        w.civ_mut(CivId(0)).unwrap().techs.insert(r.technologies.id("Agriculture").unwrap());
        assert!(wants_worker(&w, &r, CivId(0)));
        w.cities.get_mut(city_id).unwrap().queue.push(Construction::Unit(r.units.id("Worker").unwrap()));
        assert!(!wants_worker(&w, &r, CivId(0)));
        w.cities.get_mut(city_id).unwrap().queue = vec![Construction::Unit(r.units.id("Settler").unwrap())];
        assert!(!wants_settler(&w, &r, CivId(0)));
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
        assert!(upkeep(broke) <= upkeep(rich), "being in debt should not make dear buildings more attractive");
    }

    /// A thing that wins the game is worth building — when it can be built.
    #[test]
    fn a_city_that_can_finish_a_victory_building_prefers_it() {
        let r = ruleset();
        let (wins, _) = crate::world::victory::victory_constructions(&r);
        assert!(!wins.is_empty(), "the ruleset should name some buildings as milestones");

        let mut w = world(&r);
        let city_id = w.found_city_in(&r, City::found("Rome", CivId(0), w.grid.id_at(8, 8).unwrap()));
        // Everything researched, so the victory buildings are actually
        // available to choose.
        for t in r.technologies.ids() {
            w.civ_mut(CivId(0)).unwrap().techs.insert(t);
        }
        // A city with enormous production, so any of them is within reach.
        let hill = r.terrain("Hill").unwrap();
        let centre = w.cities.get(city_id).unwrap().centre;
        for t in w.grid.within(centre, 2) {
            w.grid.features[t.0 as usize].push(hill);
            w.grid.improvement[t.0 as usize] = r.improvements.id("Mine");
        }
        {
            let c = w.cities.get_mut(city_id).unwrap();
            c.population = 20;
            c.worked = w.grid.within(centre, 2).into_iter().take(20).collect();
        }

        let city = w.cities.get(city_id).unwrap().clone();
        let pick = |w: &World| {
            choose_construction(w, &r, CivId(0), &city, CostModifiers::default(), 50, Priorities::default())
        };

        // With no progress toward any victory, the nudge is small and ordinary
        // infrastructure still wins. That is correct: a civ that is nowhere
        // near winning should build what makes it stronger.
        let early = pick(&w);
        assert!(matches!(early, Some(Construction::Building(_))));

        // The building that would finish a victory outright — the one whose
        // completion takes some condition from unmet to met — should win.
        // Note "outright": a wonder that is one of several milestones is not
        // the last thing between this civ and the end of the game, and it is
        // correct for ordinary infrastructure to beat it.
        for policy in r.policies.ids() {
            w.civ_mut(CivId(0)).unwrap().adopted_policies.insert(policy);
        }
        crate::world::policies::rebuild_uniques(&mut w, &r, CivId(0));
        let target = *wins.iter().max_by_key(|b| victory_urgency(&w, &r, CivId(0), **b).raw()).expect("at least one");
        assert!(
            victory_urgency(&w, &r, CivId(0), target) > Fixed::whole(1000),
            "{} should be a game-ender",
            r.buildings[target].name
        );

        let city = w.cities.get(city_id).unwrap().clone();
        let near = choose_construction(&w, &r, CivId(0), &city, CostModifiers::default(), 50, Priorities::default());
        // Any game-ender will do — several victories may each be one
        // building away, and picking the cheapest of them is correct.
        match near {
            Some(Construction::Building(id)) => assert!(
                victory_urgency(&w, &r, CivId(0), id) > Fixed::whole(1000),
                "chose {}, which ends nothing, over {} which would have ended the game",
                r.buildings[id].name,
                r.buildings[target].name
            ),
            other => panic!("expected a victory building, got {other:?}"),
        }

        // And the urgency really is scaled, not flat: a milestone that only
        // gets a victory halfway is wanted far less than one that finishes it.
        if let Some(partial) =
            wins.iter().map(|b| victory_urgency(&w, &r, CivId(0), *b)).filter(|u| *u < Fixed::whole(1000)).max()
        {
            assert!(
                partial < victory_urgency(&w, &r, CivId(0), target),
                "halfway and finished should not be worth the same"
            );
        }
    }

    /// And is not worth starting when it cannot be finished.
    #[test]
    fn a_one_hammer_town_does_not_commit_to_a_wonder_it_cannot_build() {
        let r = ruleset();
        let mut w = world(&r);
        let city_id = w.found_city_in(&r, City::found("Rome", CivId(0), w.grid.id_at(8, 8).unwrap()));
        for t in r.technologies.ids() {
            w.civ_mut(CivId(0)).unwrap().techs.insert(t);
        }
        let city = w.cities.get(city_id).unwrap().clone();
        let (wins, _) = crate::world::victory::victory_constructions(&r);
        let choice = choose_construction(&w, &r, CivId(0), &city, CostModifiers::default(), 10, Priorities::default());
        if let Some(Construction::Building(id)) = choice {
            assert!(
                !wins.contains(&id),
                "a size-one city on grassland committed to {}, which it cannot finish",
                r.buildings[id].name
            );
        }
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
            Some(Construction::Building(id)) => crate::ruleset::queries::HasStats::stats(&r.buildings[id]).happiness,
            _ => Fixed::ZERO,
        };

        let a = choose_construction(&w, &r, CivId(0), &city, m, 20, content);
        let b = choose_construction(&w, &r, CivId(0), &city, m, 20, miserable);
        assert!(happiness_of(b) >= happiness_of(a), "a miserable empire should not pick the *less* cheering building");
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
            assert_eq!(choose_research(&w, &r, CivId(0), s), choose_research(&w, &r, CivId(0), s));
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
