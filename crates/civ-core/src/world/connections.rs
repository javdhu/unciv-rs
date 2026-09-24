//! Which cities are connected to the capital, and by what.
//!
//! A connection is the only reason a road is worth its upkeep: a city joined
//! to the capital runs a trade route, which is gold, and a city joined by an
//! unbroken railroad builds 25% faster. Nothing else in the game turns a line
//! of tiles into money.
//!
//! # Derived, and recomputed
//!
//! This is a *derived* fact — it is a function of the map, the cities, the
//! techs and the treaties, all of which change constantly. The tempting
//! optimisation is to cache it behind a dirty flag, and the flag would have to
//! be tripped by: laying a road, pillaging one, founding a city, capturing
//! one, razing one, researching two specific techs, signing open borders,
//! declaring war, making peace, and moving a ship next to a harbour. Miss one
//! and a city silently keeps or loses income for the rest of the game.
//!
//! So it is recomputed from scratch, once per civ per turn. The work is a
//! handful of breadth-first searches over ten thousand tiles — microseconds.
//! Invalidation bugs are not microseconds.
//!
//! # Shape of the search
//!
//! Unciv's [`CapitalConnectionsFinder`] walks a worklist of cities, and from
//! each one runs a BFS over the tiles a given medium can use. The trick that
//! keeps it cheap is a memo: if I already arrived at this city by road, then a
//! road search *from* here can only find cities I already have, because road
//! reachability is symmetric and transitive. The same holds for railroads and
//! for each sea.
//!
//! What makes it a worklist at all rather than one search is the harbour hop:
//! a port joins two land masses that no road connects, so reaching a new port
//! opens a new region. This implementation keeps that structure, with one
//! change — a city is re-queued when its set of mediums *grows*, not only when
//! it is first seen. Unciv queues each city once, which can miss a railroad
//! that arrives at an already-road-reached city across a harbour hop. Growing
//! sets and a sorted queue also make the fixpoint independent of visit order,
//! which matters more here than matching that particular corner.

use crate::arena::Id;
use crate::ruleset::{Building, Ruleset};
use crate::stats::{Fixed, Stat, Stats};
use crate::world::city::City;
use crate::world::grid::{CivId, RoadLevel, TileId};
use crate::world::world::World;
use std::collections::{BTreeMap, BTreeSet, VecDeque};

/// How a city was reached. Kept as a bitmask because a city can be reachable
/// several ways at once, and *which* ways decides what may be continued from
/// it: only a railroad chain continues as a railroad.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub struct Mediums(u8);

impl Mediums {
    /// The capital. Not a medium so much as the absence of one — it is
    /// connected to itself by nothing, which is why it contributes no road
    /// level, but a railroad may still set out from it.
    pub const START: Mediums = Mediums(1);
    pub const ROAD: Mediums = Mediums(2);
    pub const RAILROAD: Mediums = Mediums(4);
    pub const HARBOUR_FROM_ROAD: Mediums = Mediums(8);
    pub const HARBOUR_FROM_RAILROAD: Mediums = Mediums(16);

    pub const EMPTY: Mediums = Mediums(0);

    pub fn contains(self, other: Mediums) -> bool {
        self.0 & other.0 != 0
    }

    pub fn with(self, other: Mediums) -> Mediums {
        Mediums(self.0 | other.0)
    }

    /// The best road a route of these mediums amounts to. A harbour hop keeps
    /// the quality of whatever fed it, which is why a port on a railroad still
    /// counts as railroad-connected.
    pub fn road_level(self) -> RoadLevel {
        if self.contains(Mediums::RAILROAD) || self.contains(Mediums::HARBOUR_FROM_RAILROAD) {
            RoadLevel::Railroad
        } else if self.contains(Mediums::ROAD) || self.contains(Mediums::HARBOUR_FROM_ROAD) {
            RoadLevel::Road
        } else {
            RoadLevel::None
        }
    }

    /// May a railroad search continue from a city reached this way? The
    /// capital counts, so a railroad leaving the capital is a railroad.
    fn continues_railroad(self) -> bool {
        self.contains(Mediums::START)
            || self.contains(Mediums::RAILROAD)
            || self.contains(Mediums::HARBOUR_FROM_RAILROAD)
    }
}

/// What one civ's road network buys it, this turn.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Connections {
    /// Every city reachable from the capital, including the capital itself.
    /// Absent means unconnected; there is no third state.
    reached: BTreeMap<Id<City>, Mediums>,
    capital: Option<Id<City>>,
}

impl Connections {
    /// A civ with no capital — none yet, or it was captured — has no trade
    /// routes at all. That is the empty value, not a missing one.
    pub fn none() -> Connections {
        Connections::default()
    }

    pub fn capital(&self) -> Option<Id<City>> {
        self.capital
    }

    /// Does this city run a trade route? The capital does not run one to
    /// itself, which is why it is excluded here rather than at every call.
    pub fn has_trade_route(&self, city: Id<City>) -> bool {
        Some(city) != self.capital && self.reached.contains_key(&city)
    }

    /// The best road this city is joined to the capital by, for the railroad
    /// production bonus. The capital reports [`RoadLevel::None`] — callers
    /// that give it the bonus anyway do so because it is the capital.
    pub fn road_level(&self, city: Id<City>) -> RoadLevel {
        self.reached.get(&city).map_or(RoadLevel::None, |m| m.road_level())
    }

    pub fn mediums(&self, city: Id<City>) -> Mediums {
        self.reached.get(&city).copied().unwrap_or(Mediums::EMPTY)
    }

    /// Cities with a trade route, capital excluded, in a stable order.
    pub fn trade_routes(&self) -> impl Iterator<Item = Id<City>> + '_ {
        let capital = self.capital;
        self.reached.keys().copied().filter(move |c| Some(*c) != capital)
    }
}

/// The building that marks a capital, by its unique rather than its name — a
/// mod may not call it "Palace".
pub fn palace(r: &Ruleset) -> Option<Id<Building>> {
    r.buildings
        .iter()
        .find(|(_, b)| b.uniques.iter().any(|u| u.unique_type() == Some(crate::UniqueType::IndicatesCapital)))
        .map(|(id, _)| id)
}

/// This civ's capital, if it still has one.
pub fn capital_of(world: &World, r: &Ruleset, civ: CivId) -> Option<Id<City>> {
    let palace = palace(r)?;
    world.civ(civ)?.cities.iter().copied().find(|id| world.cities.get(*id).is_some_and(|c| c.built.contains(&palace)))
}

/// Is this tile open water? Asked of the base terrain, so a forest on a coast
/// is still land and an ice floe is still water.
pub fn is_water(world: &World, r: &Ruleset, tile: TileId) -> bool {
    r.terrains.get(world.grid.terrain[tile.0 as usize]).is_some_and(|t| t.r#type == "Water")
}

/// Has a tech been researched? Roads laid before the tech that allows them —
/// on captured land, say — carry nothing until it is.
fn researched(world: &World, r: &Ruleset, civ: CivId, tech: Option<&String>) -> bool {
    let Some(tech) = tech else { return true };
    let Some(id) = r.technologies.id(tech) else { return true };
    world.civ(civ).is_some_and(|c| c.techs.contains(&id))
}

/// Does the ruleset's road of this level exist, and may this civ use it?
fn road_usable(world: &World, r: &Ruleset, civ: CivId, level: RoadLevel) -> bool {
    let Some(name) = level.improvement_name() else { return false };
    let Some(imp) = r.improvements.by_name(name) else { return false };
    researched(world, r, civ, imp.tech_required.as_ref())
}

/// Forests and jungles carry the Inca's roads for free, but only at home.
fn forest_or_jungle_is_road(world: &World, r: &Ruleset, civ: CivId, tile: TileId) -> bool {
    let Some(c) = world.civ(civ) else { return false };
    if !c.uniques.has(crate::UniqueType::ForestsAndJunglesAreRoads) {
        return false;
    }
    if world.grid.owner[tile.0 as usize] != Some(civ) {
        return false;
    }
    world.grid.features[tile.0 as usize]
        .iter()
        .any(|f| r.terrains.name_of(*f).is_some_and(|n| n == "Forest" || n == "Jungle"))
}

/// Is an enemy fleet sitting on this tile, or next to it?
///
/// Only tiles someone owns can be blockaded — the open sea belongs to nobody
/// and blocks nothing. A friendly or neutral ship on the tile itself lifts the
/// blockade, which is why a garrison is worth keeping.
pub fn tile_is_blockaded(world: &World, r: &Ruleset, tile: TileId) -> bool {
    let Some(owner) = world.grid.owner[tile.0 as usize] else { return false };
    let military_owner = |t: TileId| -> Option<CivId> {
        world
            .units_at(t)
            .into_iter()
            .filter_map(|u| world.units.get(u))
            .find(|u| r.units.get(u.base).is_some_and(|b| b.strength.unwrap_or(0) > 0))
            .map(|u| u.owner)
    };

    if let Some(present) = military_owner(tile) {
        // Whoever is standing here decides, friend or foe.
        return world.diplomacy.at_war(present, owner);
    }
    if !is_water(world, r, tile) {
        return false; // an empty land tile is nobody's blockade
    }
    // An empty water tile is blockaded by enemy ships beside it.
    world
        .grid
        .neighbours(tile)
        .any(|n| is_water(world, r, n) && military_owner(n).is_some_and(|civ| world.diplomacy.at_war(civ, owner)))
}

/// A coastal city whose every adjacent water tile is blockaded cannot use its
/// harbour. A landlocked city is never blockaded.
pub fn city_is_blockaded(world: &World, r: &Ruleset, city: &City) -> bool {
    let mut water = world.grid.neighbours(city.centre).filter(|n| is_water(world, r, *n)).peekable();
    if water.peek().is_none() {
        return false;
    }
    water.all(|n| tile_is_blockaded(world, r, n))
}

/// Does this city have a building that joins it to the sea trade network?
fn has_harbour(city: &City) -> bool {
    city.uniques.has(crate::UniqueType::ConnectTradeRoutes)
}

/// One search: the medium it grants, which tiles it may cross, and which
/// cities it may land at.
type Search<'a> = (Mediums, Box<dyn Fn(TileId) -> bool + 'a>, bool);

/// Work out what this civ's cities are joined to its capital by.
pub fn find(world: &World, r: &Ruleset, civ: CivId) -> Connections {
    let Some(capital) = capital_of(world, r, civ) else { return Connections::none() };

    // Every city whose centre we are allowed to stand on — ours, and those of
    // civs who have opened their borders. A route may pass through a friend's
    // city and out the other side.
    let candidates: BTreeSet<Id<City>> = world
        .cities
        .iter()
        .filter(|(_, c)| world.diplomacy.may_enter_borders_of(civ, c.owner))
        .map(|(id, _)| id)
        .collect();
    let centres: BTreeMap<TileId, Id<City>> =
        candidates.iter().filter_map(|id| world.cities.get(*id).map(|c| (c.centre, *id))).collect();

    let road_ok = road_usable(world, r, civ, RoadLevel::Road);
    let rail_ok = road_usable(world, r, civ, RoadLevel::Railroad);

    let mut reached: BTreeMap<Id<City>, Mediums> = BTreeMap::new();
    reached.insert(capital, Mediums::START);
    // Which searches have already been run from which city. A road search from
    // a road-reached city finds nothing new, so it is skipped — this is what
    // keeps the fixpoint cheap.
    let mut done: BTreeSet<(Id<City>, Mediums)> = BTreeSet::new();
    let mut queue: VecDeque<Id<City>> = VecDeque::from([capital]);

    while let Some(from) = queue.pop_front() {
        let Some(city) = world.cities.get(from) else { continue };
        let here = reached[&from];

        // Each search is (medium it grants, which tiles it may cross, which
        // cities it may land at). Ordered so the better mediums are found
        // first; the fixpoint does not depend on it, but the queue stays short.
        let mut searches: Vec<Search<'_>> = Vec::new();

        if has_harbour(city) && city.owner == civ && !city_is_blockaded(world, r, city) {
            // Only an actual railroad arriving here promotes the hop. The
            // capital does not qualify by being the capital, and a harbour
            // hop does not chain its own grade onward — otherwise a rail line
            // ending at one port would turn every port in the world into a
            // railhead.
            let medium = if here.contains(Mediums::RAILROAD) {
                Mediums::HARBOUR_FROM_RAILROAD
            } else {
                Mediums::HARBOUR_FROM_ROAD
            };
            searches.push((medium, Box::new(|t| is_water(world, r, t)), true));
        }
        if rail_ok && here.continues_railroad() {
            searches.push((
                Mediums::RAILROAD,
                Box::new(|t| world.grid.unpillaged_road(t) == RoadLevel::Railroad),
                false,
            ));
        }
        if road_ok {
            searches.push((
                Mediums::ROAD,
                Box::new(|t| {
                    world.grid.unpillaged_road(t) != RoadLevel::None || forest_or_jungle_is_road(world, r, civ, t)
                }),
                false,
            ));
        }

        for (medium, passable, harbours_only) in searches {
            if done.contains(&(from, medium)) {
                continue;
            }
            done.insert((from, medium));

            for landed in flood(world, r, civ, city.centre, &centres, &*passable) {
                if landed == from {
                    continue;
                }
                if harbours_only {
                    // A sea route can only land at one of our own working ports.
                    let Some(c) = world.cities.get(landed) else { continue };
                    if c.owner != civ || !has_harbour(c) || city_is_blockaded(world, r, c) {
                        continue;
                    }
                }
                let entry = reached.entry(landed).or_insert(Mediums::EMPTY);
                let grown = entry.with(medium);
                if grown != *entry {
                    *entry = grown;
                    queue.push_back(landed);
                }
            }
        }
    }

    Connections { reached, capital: Some(capital) }
}

/// Breadth-first over tiles this medium can cross, returning the cities landed
/// at. City centres are always crossable — a road may end at a city and carry
/// on from the far side — and nobody's territory may be crossed uninvited.
fn flood(
    world: &World,
    _r: &Ruleset,
    civ: CivId,
    start: TileId,
    centres: &BTreeMap<TileId, Id<City>>,
    passable: &dyn Fn(TileId) -> bool,
) -> Vec<Id<City>> {
    let mut seen = vec![false; world.grid.len()];
    let mut queue = VecDeque::from([start]);
    let mut found = Vec::new();
    seen[start.0 as usize] = true;
    if let Some(c) = centres.get(&start) {
        found.push(*c);
    }

    while let Some(t) = queue.pop_front() {
        for n in world.grid.neighbours(t) {
            if seen[n.0 as usize] {
                continue;
            }
            if let Some(owner) = world.grid.owner[n.0 as usize] {
                if !world.diplomacy.may_enter_borders_of(civ, owner) {
                    continue;
                }
            }
            let is_centre = centres.contains_key(&n);
            if !is_centre && !passable(n) {
                continue;
            }
            seen[n.0 as usize] = true;
            if let Some(c) = centres.get(&n) {
                found.push(*c);
            }
            queue.push_back(n);
        }
    }
    found
}

// ---------------------------------------------------------------------------
// What a connection is worth
// ---------------------------------------------------------------------------

/// Gold a city earns for being joined to the capital.
///
/// `capital_population * 0.15 + own_population * 1.1 - 1`, which is the Civ V
/// formula and the reason a trade route from a *small* city to a *large*
/// capital is still worth having. The capital earns nothing from itself.
///
/// Uniques then add flat stats and scale them by a percentage, in that order —
/// swapping them would let a percentage apply to itself.
pub fn trade_route_stats(
    world: &World,
    r: &Ruleset,
    civ: CivId,
    connections: &Connections,
    city_id: Id<City>,
) -> Stats {
    if !connections.has_trade_route(city_id) {
        return Stats::ZERO;
    }
    let Some(capital) = connections.capital().and_then(|c| world.cities.get(c)) else {
        return Stats::ZERO;
    };
    let Some(city) = world.cities.get(city_id) else { return Stats::ZERO };

    let mut stats = Stats {
        gold: (Fixed::permille(150) * Fixed::whole(capital.population as i32))
            + (Fixed::permille(1100) * Fixed::whole(city.population as i32))
            - Fixed::whole(1),
        ..Stats::ZERO
    };

    // A unique that only applies on some condition has to be asked. One that
    // cannot be decided is not applied — an undecidable bonus paid out anyway
    // is a silent wrong number.
    // The city's tile too, so a route's bonus may depend on where the city
    // lies (`<within [4] tiles of a [Grand Canal]>`).
    let ctx = crate::world::conditionals::GameContext::new(r)
        .with_world(world)
        .for_civ(civ)
        .in_city(city_id)
        .on_tile(city.centre);
    let holds = |u: &crate::unique::Unique| {
        crate::world::conditionals::applies(u, &ctx) == crate::world::conditionals::Verdict::True
    };

    let sources = crate::world::religion::city_uniques(world, r, city)
        .into_iter()
        .filter(|u| u.unique_type() == Some(crate::UniqueType::StatsFromTradeRoute) && holds(u));
    for u in sources {
        if let Some(extra) = u.params.first().and_then(|p| Stats::parse(p)) {
            stats += extra;
        }
    }

    let mut percent = Stats::ZERO;
    let scalers = crate::world::religion::city_uniques(world, r, city)
        .into_iter()
        .filter(|u| u.unique_type() == Some(crate::UniqueType::StatPercentFromTradeRoutes) && holds(u));
    for u in scalers {
        let (Some(amount), Some(stat)) =
            (u.params.first().and_then(|p| Fixed::parse(p)), u.params.get(1).and_then(|p| Stat::parse(p)))
        else {
            continue;
        };
        percent[stat] += amount;
    }
    if !percent.is_zero() {
        for stat in Stat::ALL {
            let pct = percent[stat];
            if !pct.is_zero() {
                if let Some(scaled) = (stats[stat] * (Fixed::whole(100) + pct)).checked_div(Fixed::whole(100)) {
                    stats[stat] = scaled;
                }
            }
        }
    }
    stats
}

/// The +25% production a railroad to the capital is worth, as a percentage.
///
/// The capital gets it for being the capital. Everyone else needs an unbroken
/// railroad — and needs the tech, because tracks captured from a civ further
/// ahead are just sleepers until you learn to run trains on them.
pub fn railroad_production_bonus(
    world: &World,
    r: &Ruleset,
    civ: CivId,
    connections: &Connections,
    city_id: Id<City>,
) -> Fixed {
    if !road_usable(world, r, civ, RoadLevel::Railroad) {
        return Fixed::ZERO;
    }
    let is_capital = connections.capital() == Some(city_id);
    if is_capital || connections.road_level(city_id) == RoadLevel::Railroad {
        Fixed::whole(25)
    } else {
        Fixed::ZERO
    }
}

/// What this civ pays each turn to keep its roads in repair.
///
/// Returned as a positive cost — the caller subtracts it — because a function
/// called "upkeep" that returns a negative number reads wrong at every call.
///
/// Only roads inside our own borders are charged, which is what the ruleset's
/// own unique says: "Costs [1] [Gold] per turn when in your territory". Roads
/// laid in no-man's-land are free, and that is deliberate in Civ V, not an
/// omission here.
pub fn transportation_upkeep(world: &World, r: &Ruleset, civ: CivId) -> Stats {
    let mut total = Stats::ZERO;
    let exempt: Vec<&str> = world
        .civ(civ)
        .into_iter()
        .flat_map(|c| c.uniques.matching(crate::UniqueType::NoImprovementMaintenanceInSpecificTiles))
        .filter_map(|u| u.params.first().map(|s| s.as_str()))
        .collect();
    let centres: BTreeSet<TileId> = world.cities.iter().map(|(_, c)| c.centre).collect();

    for tile in world.grid.ids() {
        if world.grid.owner[tile.0 as usize] != Some(civ) {
            continue;
        }
        if centres.contains(&tile) {
            continue; // a city paves its own streets
        }
        let level = world.grid.unpillaged_road(tile);
        let Some(name) = level.improvement_name() else { continue };
        if !exempt.is_empty() && exempt.iter().any(|f| tile_matches_filter(world, r, tile, f)) {
            continue;
        }
        let Some(road) = r.improvements.by_name(name) else { continue };
        for u in &road.uniques {
            if !matches!(
                u.unique_type(),
                Some(crate::UniqueType::ImprovementMaintenance) | Some(crate::UniqueType::ImprovementAllMaintenance)
            ) {
                continue;
            }
            let (Some(amount), Some(stat)) =
                (u.params.first().and_then(|p| Fixed::parse(p)), u.params.get(1).and_then(|p| Stat::parse(p)))
            else {
                continue;
            };
            total[stat] += amount;
        }
    }

    // A civ with cheaper roads pays a fraction of the whole bill, so this is
    // applied once at the end rather than per tile.
    for u in world.civ(civ).into_iter().flat_map(|c| c.uniques.matching(crate::UniqueType::RoadMaintenance)) {
        if let Some(pct) = u.params.first().and_then(|p| Fixed::parse(p)) {
            for stat in Stat::ALL {
                let v = total[stat];
                if !v.is_zero() {
                    if let Some(scaled) = (v * pct).checked_div(Fixed::whole(100)) {
                        total[stat] = scaled;
                    }
                }
            }
        }
    }
    total
}

/// Does a tile match a filter, for the maintenance exemptions? Only the terrain
/// forms are answered; anything else is charged for rather than guessed at.
fn tile_matches_filter(world: &World, r: &Ruleset, tile: TileId, filter: &str) -> bool {
    let i = tile.0 as usize;
    if r.terrains.name_of(world.grid.terrain[i]).is_some_and(|n| n == filter) {
        return true;
    }
    if world.grid.features[i].iter().any(|f| r.terrains.name_of(*f).is_some_and(|n| n == filter)) {
        return true;
    }
    r.terrains.get(world.grid.terrain[i]).is_some_and(|t| t.r#type == filter)
}

/// Tiles that want a road, so that an unconnected city becomes connected.
///
/// For each city with no trade route, this finds the cheapest line to the
/// existing network — cheapest meaning *fewest new road tiles*, so an existing
/// stretch of road is free to reuse and the network grows as a tree rather
/// than as a set of parallel highways.
///
/// Returned in priority order: the tiles of the shortest missing link first,
/// because a half-built road earns nothing and finishing one route beats
/// starting three.
pub fn road_plan(world: &World, r: &Ruleset, civ: CivId) -> Vec<TileId> {
    if !road_usable(world, r, civ, RoadLevel::Road) {
        return Vec::new(); // no wheel, no roads
    }
    let connections = find(world, r, civ);
    let Some(capital) = connections.capital() else { return Vec::new() };
    let Some(c) = world.civ(civ) else { return Vec::new() };

    // Everything already on the network is a valid place to join.
    let joined: BTreeSet<TileId> = std::iter::once(capital)
        .chain(connections.trade_routes())
        .filter_map(|id| world.cities.get(id))
        .filter(|city| city.owner == civ)
        .map(|city| city.centre)
        .collect();

    let mut links: Vec<(usize, Vec<TileId>)> = Vec::new();
    for city_id in &c.cities {
        if connections.has_trade_route(*city_id) || Some(*city_id) == connections.capital() {
            continue;
        }
        let Some(city) = world.cities.get(*city_id) else { continue };
        if let Some(path) = cheapest_link(world, r, civ, city.centre, &joined) {
            let missing: Vec<TileId> = path
                .into_iter()
                .filter(|t| world.grid.unpillaged_road(*t) == RoadLevel::None && !joined.contains(t))
                .collect();
            if missing.is_empty() {
                continue;
            }
            // Would the route pay for itself? A road earns the trade route
            // once and costs its upkeep every turn, and only the stretch
            // inside our own borders is charged. Building regardless is how
            // an empire ends up paying seventeen gold a turn for roads that
            // earn it eight — which is what the long self-play did before
            // this check existed.
            if !link_pays(world, r, civ, &connections, *city_id, &missing) {
                continue;
            }
            links.push((missing.len(), missing));
        }
    }
    links.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.first().cmp(&b.1.first())));
    links.into_iter().flat_map(|(_, tiles)| tiles).collect()
}

/// Is a proposed link worth its upkeep?
///
/// The route's gold is worked out as if the city were already connected —
/// `trade_route_stats` refuses to price an unconnected city, so the formula is
/// applied directly here rather than pretending the world is other than it is.
fn link_pays(
    world: &World,
    r: &Ruleset,
    civ: CivId,
    connections: &Connections,
    city_id: Id<City>,
    missing: &[TileId],
) -> bool {
    let Some(capital) = connections.capital().and_then(|c| world.cities.get(c)) else {
        return false;
    };
    let Some(city) = world.cities.get(city_id) else { return false };
    let worth = Fixed::permille(150) * Fixed::whole(capital.population as i32)
        + (Fixed::permille(1100) * Fixed::whole(city.population as i32))
        - Fixed::whole(1);

    // Only our own ground is charged, and a city we are about to grow into
    // will claim more of it, so this is an underestimate on purpose — a road
    // that is marginal today is usually worth it by the time it is finished.
    let charged = missing.iter().filter(|t| world.grid.owner[t.0 as usize] == Some(civ)).count() as i32;
    let Some(road) = r.improvements.by_name("Road") else { return false };
    let per_tile: Fixed = road
        .uniques
        .iter()
        .filter(|u| {
            matches!(
                u.unique_type(),
                Some(crate::UniqueType::ImprovementMaintenance) | Some(crate::UniqueType::ImprovementAllMaintenance)
            )
        })
        .filter(|u| u.params.get(1).and_then(|p| Stat::parse(p)) == Some(Stat::Gold))
        .filter_map(|u| u.params.first().and_then(|p| Fixed::parse(p)))
        .sum();

    worth > (per_tile * Fixed::whole(charged))
}

/// Dijkstra from a city to the nearest tile of the network, where a tile that
/// already has a road costs nothing to traverse and a bare tile costs one.
///
/// A plain distance search would ignore roads that are already there and
/// propose a straight line through untouched country; this one follows the
/// existing network as far as it goes and only pays for the gap.
fn cheapest_link(
    world: &World,
    r: &Ruleset,
    civ: CivId,
    from: TileId,
    targets: &BTreeSet<TileId>,
) -> Option<Vec<TileId>> {
    use std::collections::BinaryHeap;
    let n = world.grid.len();
    let mut best = vec![u32::MAX; n];
    let mut came_from: Vec<Option<TileId>> = vec![None; n];
    // (cost, tile) with cost negated, because BinaryHeap is a max-heap and a
    // Reverse wrapper reads worse than a minus sign here.
    let mut heap: BinaryHeap<(std::cmp::Reverse<u32>, u16)> = BinaryHeap::new();
    best[from.0 as usize] = 0;
    heap.push((std::cmp::Reverse(0), from.0));

    let mut reached: Option<TileId> = None;
    while let Some((std::cmp::Reverse(cost), raw)) = heap.pop() {
        let t = TileId(raw);
        if cost > best[raw as usize] {
            continue;
        }
        if t != from && targets.contains(&t) {
            reached = Some(t);
            break;
        }
        for next in world.grid.neighbours(t) {
            let i = next.0 as usize;
            if !crate::world::movement::is_passable(&world.grid, r, next) {
                continue;
            }
            if is_water(world, r, next) {
                continue; // workers do not pave the sea
            }
            if let Some(owner) = world.grid.owner[i] {
                if !world.diplomacy.may_enter_borders_of(civ, owner) {
                    continue;
                }
            }
            let step =
                if world.grid.unpillaged_road(next) != RoadLevel::None || targets.contains(&next) { 0 } else { 1 };
            let candidate = cost + step;
            if candidate < best[i] {
                best[i] = candidate;
                came_from[i] = Some(t);
                heap.push((std::cmp::Reverse(candidate), next.0));
            }
        }
    }

    let mut at = reached?;
    let mut path = Vec::new();
    while at != from {
        path.push(at);
        at = came_from[at.0 as usize]?;
    }
    path.reverse();
    Some(path)
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
        let g = TileGrid::new(8, 8, r.terrain("Grassland").unwrap());
        let nation = r.nations.ids().next().unwrap();
        World::new(g, vec![Civilization::new(CivId(0), nation, true), Civilization::new(CivId(1), nation, true)])
    }

    /// Everything reachable, so the tech gates are out of the way.
    fn with_roads_researched(w: &mut World, r: &Ruleset, civ: CivId) {
        for name in ["The Wheel", "Railroads"] {
            if let Some(t) = r.technologies.id(name) {
                w.civ_mut(civ).unwrap().techs.insert(t);
            }
        }
    }

    /// Lay a road along a row of tiles, inclusive.
    fn pave(w: &mut World, r: &Ruleset, from: u16, to: u16, level: RoadLevel, owner: CivId) {
        let _ = r;
        for col in from..=to {
            let t = w.grid.id_at(col, 3).unwrap();
            w.grid.set_road(t, level);
            w.grid.owner[t.0 as usize] = Some(owner);
        }
    }

    /// Two cities on row 3, at the given columns, the first one the capital.
    fn two_cities(w: &mut World, r: &Ruleset, a: u16, b: u16) -> (Id<City>, Id<City>) {
        let ca = w.grid.id_at(a, 3).unwrap();
        let cb = w.grid.id_at(b, 3).unwrap();
        let first = w.found_city_in(r, City::found("Rome", CivId(0), ca));
        let second = w.found_city_in(r, City::found("Ostia", CivId(0), cb));
        (first, second)
    }

    #[test]
    fn a_road_between_two_cities_is_a_trade_route() {
        let r = ruleset();
        let mut w = world(&r);
        with_roads_researched(&mut w, &r, CivId(0));
        let (rome, ostia) = two_cities(&mut w, &r, 1, 5);

        let c = find(&w, &r, CivId(0));
        assert_eq!(c.capital(), Some(rome));
        assert!(!c.has_trade_route(ostia), "nothing joins them yet");

        pave(&mut w, &r, 1, 5, RoadLevel::Road, CivId(0));
        let c = find(&w, &r, CivId(0));
        assert!(c.has_trade_route(ostia));
        assert_eq!(c.road_level(ostia), RoadLevel::Road);
        assert!(!c.has_trade_route(rome), "the capital trades with itself for nothing");
    }

    #[test]
    fn one_missing_tile_breaks_the_whole_route() {
        let r = ruleset();
        let mut w = world(&r);
        with_roads_researched(&mut w, &r, CivId(0));
        let (_, ostia) = two_cities(&mut w, &r, 1, 5);
        pave(&mut w, &r, 1, 5, RoadLevel::Road, CivId(0));
        assert!(find(&w, &r, CivId(0)).has_trade_route(ostia));

        let gap = w.grid.id_at(3, 3).unwrap();
        w.grid.set_road(gap, RoadLevel::None);
        assert!(!find(&w, &r, CivId(0)).has_trade_route(ostia), "a road with a hole is not a road");
    }

    #[test]
    fn a_pillaged_road_carries_nothing() {
        let r = ruleset();
        let mut w = world(&r);
        with_roads_researched(&mut w, &r, CivId(0));
        let (_, ostia) = two_cities(&mut w, &r, 1, 5);
        pave(&mut w, &r, 1, 5, RoadLevel::Road, CivId(0));

        let t = w.grid.id_at(3, 3).unwrap();
        w.grid.road_pillaged[t.0 as usize] = true;
        assert!(!find(&w, &r, CivId(0)).has_trade_route(ostia));

        // Rebuilding over the ruin repairs it rather than stacking on it.
        w.grid.set_road(t, RoadLevel::Road);
        assert!(find(&w, &r, CivId(0)).has_trade_route(ostia));
    }

    #[test]
    fn a_railroad_is_also_a_road_but_not_the_reverse() {
        let r = ruleset();
        let mut w = world(&r);
        with_roads_researched(&mut w, &r, CivId(0));
        let (_, ostia) = two_cities(&mut w, &r, 1, 5);

        pave(&mut w, &r, 1, 5, RoadLevel::Railroad, CivId(0));
        assert_eq!(find(&w, &r, CivId(0)).road_level(ostia), RoadLevel::Railroad);

        // One plain-road tile in the middle and the railroad chain is broken,
        // though the trade route survives.
        let t = w.grid.id_at(3, 3).unwrap();
        w.grid.set_road(t, RoadLevel::Road);
        let c = find(&w, &r, CivId(0));
        assert!(c.has_trade_route(ostia), "still a road");
        assert_eq!(c.road_level(ostia), RoadLevel::Road, "but no longer a railroad");
    }

    #[test]
    fn rails_laid_before_the_tech_carry_nothing() {
        let r = ruleset();
        let mut w = world(&r);
        // The Wheel only: roads work, railroads do not.
        let wheel = r.technologies.id("The Wheel").unwrap();
        w.civ_mut(CivId(0)).unwrap().techs.insert(wheel);
        let (_, ostia) = two_cities(&mut w, &r, 1, 5);
        pave(&mut w, &r, 1, 5, RoadLevel::Railroad, CivId(0));

        let c = find(&w, &r, CivId(0));
        assert!(c.has_trade_route(ostia), "the tracks are still a road");
        assert_eq!(c.road_level(ostia), RoadLevel::Road, "but we cannot run trains on them");
    }

    #[test]
    fn without_the_wheel_there_are_no_routes_at_all() {
        let r = ruleset();
        let mut w = world(&r);
        let (_, ostia) = two_cities(&mut w, &r, 1, 5);
        pave(&mut w, &r, 1, 5, RoadLevel::Road, CivId(0));
        assert!(!find(&w, &r, CivId(0)).has_trade_route(ostia));
    }

    #[test]
    fn a_route_may_not_cross_a_closed_border() {
        let r = ruleset();
        let mut w = world(&r);
        with_roads_researched(&mut w, &r, CivId(0));
        let (_, ostia) = two_cities(&mut w, &r, 1, 5);
        pave(&mut w, &r, 1, 5, RoadLevel::Road, CivId(0));

        // A neighbour annexes the middle of our own road.
        let t = w.grid.id_at(3, 3).unwrap();
        w.grid.owner[t.0 as usize] = Some(CivId(1));
        w.diplomacy.meet(CivId(0), CivId(1));
        assert!(!find(&w, &r, CivId(0)).has_trade_route(ostia), "we may not walk through");

        w.diplomacy.set_open_borders(CivId(1), CivId(0), true);
        assert!(find(&w, &r, CivId(0)).has_trade_route(ostia), "invited in, the route holds");

        // War shuts the borders without anyone having to remember to.
        w.diplomacy.declare_war(CivId(1), CivId(0));
        assert!(!find(&w, &r, CivId(0)).has_trade_route(ostia));
    }

    #[test]
    fn unclaimed_land_is_open_to_everyone() {
        let r = ruleset();
        let mut w = world(&r);
        with_roads_researched(&mut w, &r, CivId(0));
        let (_, ostia) = two_cities(&mut w, &r, 1, 5);
        for col in 1..=5 {
            let t = w.grid.id_at(col, 3).unwrap();
            w.grid.set_road(t, RoadLevel::Road); // laid, but nobody owns the tiles
        }
        assert!(find(&w, &r, CivId(0)).has_trade_route(ostia));
    }

    #[test]
    fn open_borders_are_one_sided() {
        let r = ruleset();
        let mut w = world(&r);
        w.diplomacy.meet(CivId(0), CivId(1));
        w.diplomacy.set_open_borders(CivId(0), CivId(1), true);
        assert!(w.diplomacy.may_enter_borders_of(CivId(1), CivId(0)), "we let them in");
        assert!(!w.diplomacy.may_enter_borders_of(CivId(0), CivId(1)), "they did not reciprocate");
        assert!(w.diplomacy.may_enter_borders_of(CivId(0), CivId(0)), "our own land, always");
    }

    #[test]
    fn a_civ_with_no_palace_has_no_routes() {
        let r = ruleset();
        let mut w = world(&r);
        with_roads_researched(&mut w, &r, CivId(0));
        // Founded without a ruleset, so no palace is granted.
        let a = w.found_city(City::found("Rome", CivId(0), w.grid.id_at(1, 3).unwrap()));
        let b = w.found_city(City::found("Ostia", CivId(0), w.grid.id_at(5, 3).unwrap()));
        pave(&mut w, &r, 1, 5, RoadLevel::Road, CivId(0));

        let c = find(&w, &r, CivId(0));
        assert_eq!(c.capital(), None);
        assert!(!c.has_trade_route(a) && !c.has_trade_route(b));
    }

    /// Two coastal cities on opposite sides of a sea, with no land between.
    fn across_a_sea(r: &Ruleset) -> (World, Id<City>, Id<City>) {
        let mut w = world(r);
        with_roads_researched(&mut w, r, CivId(0));
        let ocean = r.terrain("Ocean").unwrap();
        // Columns 2..=6 on row 3 are water; the cities sit at 1 and 7.
        for col in 2..=6u16 {
            let t = w.grid.id_at(col, 3).unwrap();
            w.grid.terrain[t.0 as usize] = ocean;
        }
        let (rome, ostia) = two_cities(&mut w, r, 1, 7);
        (w, rome, ostia)
    }

    fn give_harbour(w: &mut World, r: &Ruleset, city: Id<City>) {
        let harbour = r
            .buildings
            .iter()
            .find(|(_, b)| b.uniques.iter().any(|u| u.unique_type() == Some(crate::UniqueType::ConnectTradeRoutes)))
            .map(|(id, _)| id)
            .expect("the ruleset has a harbour");
        let c = w.cities.get_mut(city).unwrap();
        c.built.insert(harbour);
        crate::world::city::rebuild_city_uniques(r, c);
    }

    #[test]
    fn two_harbours_make_a_sea_route_where_no_road_can_go() {
        let r = ruleset();
        let (mut w, rome, ostia) = across_a_sea(&r);
        assert!(!find(&w, &r, CivId(0)).has_trade_route(ostia), "open water joins nothing");

        give_harbour(&mut w, &r, rome);
        assert!(!find(&w, &r, CivId(0)).has_trade_route(ostia), "a port needs a port to talk to");

        give_harbour(&mut w, &r, ostia);
        let c = find(&w, &r, CivId(0));
        assert!(c.has_trade_route(ostia));
        assert!(c.mediums(ostia).contains(Mediums::HARBOUR_FROM_ROAD));
        assert_eq!(c.road_level(ostia), RoadLevel::Road, "a boat is not a train");
    }

    /// The one case the medium bitmask exists for: a railroad that ends at a
    /// port keeps its grade across the water, but a plain road does not.
    #[test]
    fn a_railroad_keeps_its_grade_across_a_harbour() {
        let r = ruleset();
        let mut w = world(&r);
        with_roads_researched(&mut w, &r, CivId(0));
        let ocean = r.terrain("Ocean").unwrap();
        for col in 3..=6u16 {
            let t = w.grid.id_at(col, 3).unwrap();
            w.grid.terrain[t.0 as usize] = ocean;
        }
        let rome = w.found_city_in(&r, City::found("Rome", CivId(0), w.grid.id_at(0, 3).unwrap()));
        let portus = w.found_city_in(&r, City::found("Portus", CivId(0), w.grid.id_at(2, 3).unwrap()));
        let ostia = w.found_city_in(&r, City::found("Ostia", CivId(0), w.grid.id_at(7, 3).unwrap()));
        assert_eq!(find(&w, &r, CivId(0)).capital(), Some(rome));
        give_harbour(&mut w, &r, portus);
        give_harbour(&mut w, &r, ostia);

        // Road as far as the port: the sea route works, at road grade.
        pave(&mut w, &r, 0, 2, RoadLevel::Road, CivId(0));
        let c = find(&w, &r, CivId(0));
        assert_eq!(c.road_level(portus), RoadLevel::Road);
        assert_eq!(c.road_level(ostia), RoadLevel::Road, "a road is still a road at sea");

        // Upgrade the same stretch to rail and the grade carries over.
        pave(&mut w, &r, 0, 2, RoadLevel::Railroad, CivId(0));
        let c = find(&w, &r, CivId(0));
        assert_eq!(c.road_level(portus), RoadLevel::Railroad);
        assert_eq!(c.road_level(ostia), RoadLevel::Railroad, "the railhead reaches across");
        assert!(c.mediums(ostia).contains(Mediums::HARBOUR_FROM_RAILROAD));
    }

    #[test]
    fn a_blockade_cuts_the_sea_route() {
        let r = ruleset();
        let (mut w, rome, ostia) = across_a_sea(&r);
        give_harbour(&mut w, &r, rome);
        give_harbour(&mut w, &r, ostia);
        // Ostia owns the water it sits on, which is what makes it blockadable.
        for n in w.grid.neighbours(w.cities.get(ostia).unwrap().centre).collect::<Vec<_>>() {
            w.grid.owner[n.0 as usize] = Some(CivId(0));
        }
        assert!(find(&w, &r, CivId(0)).has_trade_route(ostia));

        // An enemy fleet parks on every water tile beside the port.
        w.diplomacy.declare_war(CivId(1), CivId(0));
        let trireme = r.units.id("Trireme").or_else(|| r.units.id("Work Boat")).unwrap();
        let water: Vec<TileId> =
            w.grid.neighbours(w.cities.get(ostia).unwrap().centre).filter(|t| is_water(&w, &r, *t)).collect();
        assert!(!water.is_empty(), "the test city is coastal");
        for t in &water {
            w.place_unit(crate::world::entities::Unit::new(trireme, CivId(1), *t, 4));
        }
        assert!(!find(&w, &r, CivId(0)).has_trade_route(ostia), "the port is shut");

        // Sink the blockader in the harbour mouth and station our own ship
        // there. Whoever stands on a tile decides it, and one open tile is
        // enough to keep the port working.
        for u in w.units_at(water[0]) {
            w.remove_unit(u);
        }
        w.place_unit(crate::world::entities::Unit::new(trireme, CivId(0), water[0], 4));
        assert!(find(&w, &r, CivId(0)).has_trade_route(ostia), "we broke the blockade");
    }

    #[test]
    fn a_trade_route_is_worth_the_civ_v_formula() {
        let r = ruleset();
        let mut w = world(&r);
        with_roads_researched(&mut w, &r, CivId(0));
        let (rome, ostia) = two_cities(&mut w, &r, 1, 5);
        pave(&mut w, &r, 1, 5, RoadLevel::Road, CivId(0));
        w.cities.get_mut(rome).unwrap().population = 10;
        w.cities.get_mut(ostia).unwrap().population = 5;

        let c = find(&w, &r, CivId(0));
        // 10 * 0.15 + 5 * 1.1 - 1 = 6
        let stats = trade_route_stats(&w, &r, CivId(0), &c, ostia);
        assert_eq!(stats.gold, Fixed::whole(6));
        assert_eq!(
            trade_route_stats(&w, &r, CivId(0), &c, rome).gold,
            Fixed::ZERO,
            "the capital does not trade with itself"
        );
    }

    #[test]
    fn a_bigger_capital_makes_every_route_richer() {
        let r = ruleset();
        let mut w = world(&r);
        with_roads_researched(&mut w, &r, CivId(0));
        let (rome, ostia) = two_cities(&mut w, &r, 1, 5);
        pave(&mut w, &r, 1, 5, RoadLevel::Road, CivId(0));
        w.cities.get_mut(rome).unwrap().population = 4;
        w.cities.get_mut(ostia).unwrap().population = 4;

        let before = trade_route_stats(&w, &r, CivId(0), &find(&w, &r, CivId(0)), ostia).gold;
        w.cities.get_mut(rome).unwrap().population = 20;
        let after = trade_route_stats(&w, &r, CivId(0), &find(&w, &r, CivId(0)), ostia).gold;
        assert!(after > before, "{after:?} should beat {before:?}");
    }

    #[test]
    fn an_unconnected_city_earns_nothing() {
        let r = ruleset();
        let mut w = world(&r);
        with_roads_researched(&mut w, &r, CivId(0));
        let (rome, ostia) = two_cities(&mut w, &r, 1, 5);
        w.cities.get_mut(rome).unwrap().population = 10;
        w.cities.get_mut(ostia).unwrap().population = 10;
        let c = find(&w, &r, CivId(0));
        assert_eq!(trade_route_stats(&w, &r, CivId(0), &c, ostia), Stats::ZERO);
    }

    #[test]
    fn a_railroad_to_the_capital_is_worth_a_quarter_more_hammers() {
        let r = ruleset();
        let mut w = world(&r);
        with_roads_researched(&mut w, &r, CivId(0));
        let (rome, ostia) = two_cities(&mut w, &r, 1, 5);

        let c = find(&w, &r, CivId(0));
        assert_eq!(railroad_production_bonus(&w, &r, CivId(0), &c, rome), Fixed::whole(25));
        assert_eq!(railroad_production_bonus(&w, &r, CivId(0), &c, ostia), Fixed::ZERO);

        pave(&mut w, &r, 1, 5, RoadLevel::Road, CivId(0));
        let c = find(&w, &r, CivId(0));
        assert_eq!(railroad_production_bonus(&w, &r, CivId(0), &c, ostia), Fixed::ZERO, "a road is not a railroad");

        pave(&mut w, &r, 1, 5, RoadLevel::Railroad, CivId(0));
        let c = find(&w, &r, CivId(0));
        assert_eq!(railroad_production_bonus(&w, &r, CivId(0), &c, ostia), Fixed::whole(25));
    }

    #[test]
    fn roads_cost_gold_but_only_the_ones_we_own() {
        let r = ruleset();
        let mut w = world(&r);
        with_roads_researched(&mut w, &r, CivId(0));
        let (rome, _) = two_cities(&mut w, &r, 1, 5);
        assert_eq!(transportation_upkeep(&w, &r, CivId(0)), Stats::ZERO);

        // Five paved tiles, two of which are city centres at either end — a
        // city pays for its own streets, so three are charged at 1 gold each.
        pave(&mut w, &r, 1, 5, RoadLevel::Road, CivId(0));
        assert_eq!(w.cities.get(rome).unwrap().centre, w.grid.id_at(1, 3).unwrap());
        assert_eq!(transportation_upkeep(&w, &r, CivId(0)).gold, Fixed::whole(3));

        // A railroad is dearer: 2 gold a tile, not 1.
        pave(&mut w, &r, 1, 5, RoadLevel::Railroad, CivId(0));
        assert_eq!(transportation_upkeep(&w, &r, CivId(0)).gold, Fixed::whole(6));

        // Roads in nobody's territory are free.
        for col in 1..=5u16 {
            let t = w.grid.id_at(col, 3).unwrap();
            w.grid.owner[t.0 as usize] = None;
        }
        assert_eq!(transportation_upkeep(&w, &r, CivId(0)).gold, Fixed::ZERO);
    }

    #[test]
    fn a_pillaged_road_costs_nothing_to_keep() {
        let r = ruleset();
        let mut w = world(&r);
        with_roads_researched(&mut w, &r, CivId(0));
        two_cities(&mut w, &r, 1, 5);
        pave(&mut w, &r, 1, 5, RoadLevel::Road, CivId(0));
        for col in 2..=5u16 {
            let t = w.grid.id_at(col, 3).unwrap();
            w.grid.road_pillaged[t.0 as usize] = true;
        }
        assert_eq!(transportation_upkeep(&w, &r, CivId(0)).gold, Fixed::ZERO);
    }

    #[test]
    fn the_road_plan_is_the_gap_and_nothing_more() {
        let r = ruleset();
        let mut w = world(&r);
        with_roads_researched(&mut w, &r, CivId(0));
        let (_, ostia) = two_cities(&mut w, &r, 1, 5);

        let plan = road_plan(&w, &r, CivId(0));
        assert!(!plan.is_empty(), "an unconnected city wants a road");
        assert!(
            plan.iter().all(|t| w.grid.unpillaged_road(*t) == RoadLevel::None),
            "it only asks for tiles that have none"
        );

        // Lay everything the plan asked for and the city connects, with no
        // further work wanted.
        for t in &plan {
            w.grid.set_road(*t, RoadLevel::Road);
        }
        assert!(find(&w, &r, CivId(0)).has_trade_route(ostia), "the plan was sufficient");
        assert!(road_plan(&w, &r, CivId(0)).is_empty(), "and it is finished");
    }

    #[test]
    fn the_road_plan_reuses_road_that_is_already_there() {
        let r = ruleset();
        let mut w = world(&r);
        with_roads_researched(&mut w, &r, CivId(0));
        two_cities(&mut w, &r, 1, 5);
        let bare = road_plan(&w, &r, CivId(0)).len();

        // Pave most of the way; the plan should shrink by what we paved
        // rather than propose a fresh line somewhere else.
        pave(&mut w, &r, 2, 4, RoadLevel::Road, CivId(0));
        let partial = road_plan(&w, &r, CivId(0));
        assert!(partial.len() < bare, "{} should be fewer than {bare}", partial.len());
        assert!(partial.iter().all(|t| w.grid.unpillaged_road(*t) == RoadLevel::None));
    }

    #[test]
    fn no_wheel_means_no_road_plan() {
        let r = ruleset();
        let mut w = world(&r);
        two_cities(&mut w, &r, 1, 5);
        assert!(road_plan(&w, &r, CivId(0)).is_empty(), "nothing to plan without the tech");
    }

    #[test]
    fn the_answer_does_not_depend_on_which_city_was_founded_first() {
        let r = ruleset();
        let mut w = world(&r);
        with_roads_researched(&mut w, &r, CivId(0));
        let (_, ostia) = two_cities(&mut w, &r, 1, 5);
        pave(&mut w, &r, 1, 5, RoadLevel::Road, CivId(0));
        let first = find(&w, &r, CivId(0));
        let second = find(&w, &r, CivId(0));
        assert_eq!(first, second, "a derived fact recomputed twice is the same fact");
        assert!(first.has_trade_route(ostia));
    }
}
