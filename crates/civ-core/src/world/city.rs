//! Cities: which tiles they work, and the food loop that grows them.
//!
//! The growth curve is Civ V's, and it is why [`Fixed::pow`](crate::stats) had
//! to exist: `15 + 8(p−1) + ⌊(p−1)^1.5⌋` has a fractional exponent, and the
//! platform's `powf` is not reproducible between native and wasm.
//!
//! Growth is written as a pure transition — `(city, food) -> (city, event)` —
//! rather than a method that mutates and notifies. The engine is
//! event-sourced, so the event is the thing that gets recorded, replayed and
//! sent to clients; a version that only mutated would have to have the event
//! reconstructed from a diff afterwards.

use crate::arena::Id;
use crate::modifiers::{Modifier, ModifierStack, Source, SourceKind};
use crate::ruleset::{Ruleset, Speed};
use crate::stats::{Fixed, Stat, Stats};
use crate::world::grid::{CivId, TileGrid, TileId};
use crate::world::yields::{tile_yield, Contribution};
use crate::UniqueType;

/// A city's health before any buildings add to it (`City.kt:361`).
pub const BASE_CITY_HEALTH: i32 = 200;

/// Defence before population and buildings (`ModConstants.kt:30`).
pub const CITY_STRENGTH_BASE: i64 = 8_000;

/// Added per citizen — five citizens are worth two defence.
pub const CITY_STRENGTH_PER_POP: i64 = 400;

/// How far a city may work. Three in Civ V.
pub const WORK_RADIUS: u32 = 3;

/// Food each citizen eats per turn. Two in Civ V (`CityStats.kt:655`).
pub const FOOD_PER_CITIZEN: i32 = 2;

#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, PartialEq)]
pub struct City {
    pub name: String,
    pub owner: CivId,
    pub centre: TileId,
    pub population: u32,
    pub food_stored: i32,
    /// Tiles assigned to citizens. The centre is worked for free and is not
    /// listed here.
    pub worked: Vec<TileId>,
    /// The "avoid growth" toggle: stores cap instead of spilling into a citizen.
    pub avoid_growth: bool,
    /// Investment belongs to the construction, even while it is not queued.
    #[cfg_attr(feature = "serde", serde(with = "crate::serialization::map_as_entries"))]
    pub production_progress: std::collections::BTreeMap<crate::world::construction::Construction, i32>,
    /// Excess from the last completion, available to the next active item.
    pub production_overflow: i32,
    /// What this city is working on, front first.
    pub queue: Vec<crate::world::construction::Construction>,
    /// Continuous Wealth or Research work, mutually exclusive with the queue.
    pub production_conversion: Option<crate::stats::Stat>,
    /// Ordered so iteration and serialisation are reproducible.
    pub built: std::collections::BTreeSet<crate::arena::Id<crate::ruleset::Building>>,
    /// Buildings granted without an ongoing maintenance charge.
    pub free_buildings: std::collections::BTreeSet<crate::arena::Id<crate::ruleset::Building>>,
    /// The tile the player chose for a wonder or placed building still to be
    /// built (`world::placement`). Absent from older saves.
    #[cfg_attr(feature = "serde", serde(default, with = "crate::serialization::map_as_entries"))]
    pub sites: std::collections::BTreeMap<crate::arena::Id<crate::ruleset::Building>, TileId>,
    /// Damage taken. A city is never destroyed — it floors at 1 and is then
    /// captured, which is why `health` is separate from existing at all.
    pub health: i32,
    /// City bombardments remaining in the current action window.
    pub attacks_left: u8,
    pub sold_building_this_turn: bool,
    /// Uniques in force in this city, from its buildings. Rebuilt whenever
    /// something finishes — never patched.
    pub uniques: crate::world::unique_index::UniqueIndex,
    /// What this city believes. Pressure is stored, followers are derived.
    pub faith: crate::world::religion::CityFaith,
    /// The tiles inside this city's borders, sorted. `TileGrid::owner` says
    /// which civ holds a tile; this says which of its cities claimed it,
    /// which is what border growth is priced by and what a conqueror takes.
    pub tiles: Vec<TileId>,
    /// Culture banked toward the next border tile. Separate from the civ's
    /// policy pool: the same culture fills both.
    pub culture_stored: i32,
    /// Citizens working in buildings rather than on tiles, by kind.
    #[cfg_attr(feature = "serde", serde(with = "crate::serialization::map_as_entries"))]
    pub specialists: crate::world::specialists::Allocation,
    /// Who built it. An annexed city is unhappy about anyone else holding it.
    pub founder: CivId,
    /// The first city its founder built — never razed, and taken back whole.
    pub original_capital: bool,
    /// The turn it was founded or last changed hands.
    pub turn_acquired: u32,
    /// Previous holder, for the immediate post-conquest liberation decision.
    pub previous_owner: Option<CivId>,
    /// Run by its own governor for gold, after a conquest; see `conquest`.
    pub puppet: bool,
    /// Burning, a citizen a turn, until nothing is left.
    pub razing: bool,
    /// Turns of resistance left after a conquest, during which it makes nothing.
    pub resistance: u32,
    /// The luxury this city wants, for a We Love The King Day; see `wltkd`.
    pub demanded_resource: Option<Id<crate::ruleset::TileResource>>,
    /// Turns until the city tires of waiting and asks for something else.
    pub demand_countdown: u32,
    /// Turns of celebration left.
    pub wltkd: u32,
    /// Whether the centre touches the coast (`isCoastal`) — set once, when it
    /// is founded, since the land does not move. Only a coastal city builds
    /// ships.
    pub coastal: bool,
}

impl City {
    pub fn found(name: impl Into<String>, owner: CivId, centre: TileId) -> City {
        City {
            name: name.into(),
            owner,
            centre,
            population: 1,
            food_stored: 0,
            worked: Vec::new(),
            avoid_growth: false,
            health: BASE_CITY_HEALTH,
            attacks_left: 1,
            sold_building_this_turn: false,
            production_progress: Default::default(),
            production_overflow: 0,
            queue: Vec::new(),
            production_conversion: None,
            built: std::collections::BTreeSet::new(),
            free_buildings: Default::default(),
            sites: Default::default(),
            uniques: Default::default(),
            // A new city is atheist, and atheism is a positive quantity here
            // rather than the absence of one.
            faith: crate::world::religion::CityFaith::new(),
            tiles: Vec::new(),
            culture_stored: 0,
            specialists: Default::default(),
            founder: owner,
            original_capital: false,
            turn_acquired: 0,
            previous_owner: None,
            puppet: false,
            razing: false,
            resistance: 0,
            demanded_resource: None,
            demand_countdown: 0,
            wltkd: 0,
            coastal: false,
        }
    }

    /// Citizens not assigned to a tile or a specialist slot.
    pub fn idle_population(&self) -> u32 {
        self.population.saturating_sub(self.worked.len() as u32 + crate::world::specialists::count(self))
    }

    pub fn production_for(&self, what: crate::world::construction::Construction) -> i32 {
        self.production_progress.get(&what).copied().unwrap_or(0)
    }

    /// Invested work on the current item; overflow is a separate reserve.
    pub fn current_production(&self) -> i32 {
        self.queue.first().map_or(0, |&what| self.production_for(what))
    }
}

/// What a turn of food did to a city.
///
/// Returned rather than applied, so the caller records it before it takes
/// effect — this is what a client replays and what a save is made of.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GrowthEvent {
    /// Stores filled; a citizen was added.
    Grew { population: u32, carried_over: i32 },
    /// Stores went negative; a citizen was lost.
    Starved { population: u32 },
    /// Stores changed, nothing else did.
    Stored { food: i32 },
    /// Growth was available but suppressed by `avoid_growth`.
    Held { food: i32 },
}

/// Modifiers applied on top of the base growth curve.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GrowthModifiers {
    /// Game speed. Marathon needs more food per citizen, Quick less.
    pub speed: Fixed,
    /// City-states need half again as much; AI civs vary by difficulty.
    pub civ: Fixed,
}

impl Default for GrowthModifiers {
    fn default() -> Self {
        GrowthModifiers { speed: Fixed::ONE, civ: Fixed::ONE }
    }
}

impl GrowthModifiers {
    /// Reads the speed's own modifier, falling back to unmodified.
    pub fn for_speed(r: &Ruleset, speed: Id<Speed>) -> GrowthModifiers {
        GrowthModifiers {
            speed: r.speeds.get(speed).map_or(Fixed::ONE, |s| s.growth_modifier.unwrap_or(s.modifier)),
            civ: Fixed::ONE,
        }
    }
}

/// Food needed to add the next citizen.
///
/// `15 + 8(p−1) + ⌊(p−1)^1.5⌋`, then scaled
/// (`CityPopulationManager.kt:51`). The floor lands before the scaling, as it
/// does in Unciv.
pub fn food_to_next_population(population: u32, m: GrowthModifiers) -> i32 {
    let p = population.saturating_sub(1) as i32;
    let curve = Fixed::whole(p).pow(Fixed::permille(1_500)).map_or(0, |v| v.floor() as i32);
    let base = Fixed::whole(15 + 8 * p + curve);
    // Kotlin's `Double.toInt()` truncates toward zero; Unciv ends the function
    // with `foodRequired.toInt()`, so a city-state's 82.5 is 82, not 83. The
    // requirement is always positive, so `floor` is that truncation. Rounding
    // here would cost one food per growth and compound over a whole game.
    (base * m.speed * m.civ).floor().max(1) as i32
}

/// What the citizens eat.
pub fn food_consumed(city: &City) -> i32 {
    city.population as i32 * FOOD_PER_CITIZEN
}

/// The tiles a city could work: within [`WORK_RADIUS`] and owned by its civ.
///
/// The centre is excluded — it is worked for free and never assigned — and
/// so is a tile a wonder stands on (`world::placement`).
pub fn workable_tiles(grid: &TileGrid, city: &City) -> Vec<TileId> {
    grid.within(city.centre, WORK_RADIUS)
        .into_iter()
        .filter(|t| *t != city.centre && grid.owner[t.0 as usize] == Some(city.owner) && !grid.is_taken(*t))
        .collect()
}

/// Tiles yielding to this city, including automatic wonders/improvements.
pub fn yield_tiles(grid: &TileGrid, r: &Ruleset, city: &City) -> Vec<TileId> {
    let mut tiles: std::collections::BTreeSet<_> =
        city.worked.iter().copied().chain(std::iter::once(city.centre)).collect();
    for tile in &city.tiles {
        if grid.owner[tile.0 as usize] != Some(city.owner) || grid.distance(city.centre, *tile) > WORK_RADIUS {
            continue;
        }
        let automatic = grid
            .terrains(*tile)
            .filter_map(|id| r.terrains.get(id))
            .flat_map(|t| &t.uniques)
            .chain(
                grid.unpillaged_improvement(*tile)
                    .and_then(|id| r.improvements.get(id))
                    .into_iter()
                    .flat_map(|i| &i.uniques),
            )
            .any(|u| u.unique_type() == Some(UniqueType::TileProvidesYieldWithoutPopulation));
        if automatic {
            tiles.insert(*tile);
        }
    }
    tiles.into_iter().collect()
}

/// Everything the city produces, attributed by tile.
pub fn city_yield_breakdown(grid: &TileGrid, r: &Ruleset, city: &City) -> Vec<Contribution> {
    city_yield_parts(grid, r, city, true, None)
}

fn city_yield_parts(
    grid: &TileGrid,
    r: &Ruleset,
    city: &City,
    legacy_uniques: bool,
    observer: Option<&crate::world::entities::Civilization>,
) -> Vec<Contribution> {
    let tile_output = |tile| {
        observer
            .map_or_else(|| tile_yield(grid, r, tile), |civ| crate::world::yields::tile_yield_for(grid, r, tile, civ))
    };
    let centre = tile_output(city.centre);
    let mut out = vec![(Source::new(SourceKind::Terrain, format!("{} centre", city.name)), centre)];
    // A city centre is topped up to a floor whatever it sits on, so founding
    // on poor terrain is survivable.
    if let Some(gap) = crate::world::yields::missing_from_minimum(centre, crate::world::yields::city_centre_minimum(r))
    {
        out.push((Source::new(SourceKind::Terrain, "city centre minimum"), gap));
    }
    // Citizens produce regardless of what they stand on: one science each,
    // and one production each for those with no tile to work
    // (`CityStats.kt:451`). The second is why an over-large city is not simply
    // wasted.
    let population = Stats {
        science: Fixed::whole(city.population as i32),
        production: Fixed::whole(city.idle_population() as i32),
        ..Stats::ZERO
    };
    if !population.is_zero() {
        out.push((Source::new(SourceKind::Other, "Population"), population));
    }
    // Specialists yield as a building's flat stats do, before percentages.
    let specialists = crate::world::specialists::yields(r, city);
    if !specialists.is_zero() {
        out.push((Source::new(SourceKind::Other, "Specialists"), specialists));
    }

    // Buildings contribute their flat yields before any percentage applies.
    for id in &city.built {
        let Some(b) = r.buildings.get(*id) else { continue };
        let stats = crate::ruleset::queries::HasStats::stats(b);
        if !stats.is_zero() {
            out.push((Source::new(SourceKind::Building, b.name.clone()), stats));
        }
    }

    // Uniques that grant yields. These are how buildings mostly work: a
    // Library's science is `[+1 Science] per [2] population`, not a field, and
    // 28 building uniques in Gods & Kings are `[stats] from [tileFilter] tiles`.
    for u in city.uniques.all().filter(|_| legacy_uniques) {
        match u.unique_type() {
            Some(UniqueType::StatsPerPopulation) => {
                let (Some(stats), Some(per)) = (
                    u.params.first().and_then(|p| Stats::parse(p)),
                    u.params.get(1).and_then(|p| p.parse::<i32>().ok()),
                ) else {
                    continue;
                };
                if per <= 0 {
                    continue;
                }
                let times = city.population as i32 / per;
                if times > 0 {
                    let mut scaled = Stats::ZERO;
                    for st in Stat::ALL {
                        scaled[st] = stats[st] * Fixed::whole(times);
                    }
                    out.push((Source::new(SourceKind::Building, u.text.clone()), scaled));
                }
            }
            Some(UniqueType::StatsFromTiles) => {
                let (Some(stats), Some(filter)) = (u.params.first().and_then(|p| Stats::parse(p)), u.params.get(1))
                else {
                    continue;
                };
                let matching = city
                    .worked
                    .iter()
                    .chain(std::iter::once(&city.centre))
                    .filter(|t| {
                        crate::world::conditionals::tile_matches_in(grid, r, **t, filter, Some(city.owner))
                            == crate::world::conditionals::Verdict::True
                    })
                    .count() as i32;
                if matching > 0 {
                    let mut scaled = Stats::ZERO;
                    for st in Stat::ALL {
                        scaled[st] = stats[st] * Fixed::whole(matching);
                    }
                    out.push((Source::new(SourceKind::Building, u.text.clone()), scaled));
                }
            }
            _ => {}
        }
    }

    for t in yield_tiles(grid, r, city).into_iter().filter(|t| *t != city.centre) {
        let (col, row) = grid.coords(t);
        out.push((Source::new(SourceKind::Terrain, format!("({col}, {row})")), tile_output(t)));
    }
    out
}

/// Percent bonuses a city's buildings grant, one stack per stat.
///
/// Separate from the flat contributions because they compose differently: the
/// flat ones sum, these multiply what the sum produced. That is exactly the
/// base/additive split `ModifierStack` exists for, so the ordering question is
/// answered by construction rather than by whoever writes the next building.
pub fn city_percent_bonuses(r: &Ruleset, city: &City) -> std::collections::BTreeMap<Stat, ModifierStack> {
    let mut stacks: std::collections::BTreeMap<Stat, ModifierStack> = Default::default();
    for id in &city.built {
        let Some(b) = r.buildings.get(*id) else { continue };
        for (stat_name, percent) in &b.percent_stat_bonus {
            let Some(stat) = Stat::parse(stat_name) else { continue };
            stacks.entry(stat).or_default().add(Modifier::additive_percent(
                Fixed::whole(*percent),
                Source::new(SourceKind::Building, b.name.clone()),
            ));
        }
    }
    // A puppet's governor neglects its science and culture
    // (`getStatPercentBonusesFromPuppetCity`).
    if city.puppet {
        for stat in [Stat::Science, Stat::Culture] {
            stacks
                .entry(stat)
                .or_default()
                .add(Modifier::additive_percent(Fixed::whole(-25), Source::new(SourceKind::Other, "Puppet")));
        }
    }
    stacks
}

/// Everything the city produces, after its buildings have had their say.
pub fn city_yield(grid: &TileGrid, r: &Ruleset, city: &City) -> Stats {
    city_yield_with(grid, r, city, false)
}

/// [`city_yield`], for a city whose owner may be in a golden age: a gold more
/// on every tile it works that yields gold at all (`TileStatFunctions`), and
/// a fifth more production and culture, added to its other percentages
/// (`CityStats.getStatPercentBonusesFromGoldenAge`).
pub fn city_yield_with(grid: &TileGrid, r: &Ruleset, city: &City, golden_age: bool) -> Stats {
    finish_city_yield(grid, r, city, golden_age, Stats::ZERO)
}

/// City output with its owner's tile-yield modifiers, including Spain's
/// natural-wonder bonus. Tile percentages precede building percentages.
pub fn city_yield_in(world: &crate::world::world::World, r: &Ruleset, city: &City, golden_age: bool) -> Stats {
    city_yield_with_happiness(world, r, city, golden_age, None)
}
pub fn city_yield_with_happiness(
    world: &crate::world::world::World,
    r: &Ruleset,
    city: &City,
    golden_age: bool,
    happiness: Option<i32>,
) -> Stats {
    let (flat, percent) = crate::world::city_effects::bonuses(world, r, city, happiness);
    let bonus = tile_percentage_bonus(world, r, city, golden_age) + flat;
    let base: Stats =
        city_yield_parts(&world.grid, r, city, false, world.civ(city.owner)).into_iter().map(|(_, s)| s).sum();
    let mut rate = Fixed::from_fraction(1, 4);
    if let Some(stat) = city.production_conversion {
        let ctx = crate::world::conditionals::GameContext::new(r).with_world(world).for_civ(city.owner);
        if let Some(u) = world
            .civ(city.owner)
            .into_iter()
            .flat_map(|c| c.uniques.matching(UniqueType::ProductionToStatConversionBonus))
            .find(|u| {
                u.params.first().is_some_and(|s| s == stat.name())
                    && crate::world::conditionals::applies(u, &ctx) == crate::world::conditionals::Verdict::True
            })
        {
            rate = rate
                * Fixed::from_fraction(100 + u.params.get(1).and_then(|p| p.parse::<i64>().ok()).unwrap_or(0), 100);
        }
    }
    let golden_tiles = golden_age.then(|| {
        yield_tiles(&world.grid, r, city)
            .into_iter()
            .filter(|&tile| {
                world
                    .civ(city.owner)
                    .is_some_and(|civ| !crate::world::yields::tile_yield_for(&world.grid, r, tile, civ).gold.is_zero())
            })
            .count()
    });
    finish_output(r, city, golden_tiles, base + bonus, percent, rate)
}

fn tile_percentage_bonus(world: &crate::world::world::World, r: &Ruleset, city: &City, golden_age: bool) -> Stats {
    use crate::world::conditionals::{applies, tile_matches, GameContext, Verdict};
    let modifiers: Vec<_> = crate::world::religion::city_uniques(world, r, city)
        .into_iter()
        .filter(|u| {
            matches!(u.unique_type(), Some(UniqueType::AllStatsPercentFromObject | UniqueType::StatPercentFromObject))
        })
        .collect();
    if modifiers.is_empty() {
        return Stats::ZERO;
    }
    let mut ctx = GameContext::new(r).with_world(world).for_civ(city.owner);
    ctx.city = world.cities.iter().find(|(_, c)| c.centre == city.centre).map(|(id, _)| id);
    let mut bonus = Stats::ZERO;
    for tile in yield_tiles(&world.grid, r, city) {
        let context = ctx.on_tile(tile);
        let improvement = world.grid.unpillaged_improvement(tile).and_then(|id| r.improvements.get(id));
        let breakdown = world.civ(city.owner).map_or_else(
            || crate::world::yields::tile_yield_breakdown(&world.grid, r, tile),
            |civ| crate::world::yields::tile_yield_breakdown_for(&world.grid, r, tile, civ),
        );
        let mut terrain: Stats =
            breakdown.iter().filter(|(source, _)| source.kind != SourceKind::Improvement).map(|(_, s)| *s).sum();
        let mut improved: Stats =
            breakdown.iter().filter(|(source, _)| source.kind == SourceKind::Improvement).map(|(_, s)| *s).sum();
        improved += crate::world::improvements::conditional_yields(world, r, city.owner, tile);
        for flat in crate::world::religion::city_uniques(world, r, city) {
            if applies(flat, &context) != Verdict::True {
                continue;
            }
            let valid = match flat.unique_type() {
                Some(UniqueType::StatsFromObject) => true,
                Some(UniqueType::StatsFromTiles) => flat.params.get(2).is_some_and(|f| {
                    crate::world::filters::city_matches(world, r, city, Some(city), f) == Verdict::True
                }),
                Some(UniqueType::StatsFromTilesWithout) => {
                    flat.params.get(3).is_some_and(|f| {
                        crate::world::filters::city_matches(world, r, city, Some(city), f) == Verdict::True
                    }) && flat.params.get(2).is_some_and(|f| tile_matches(&context, tile, f) == Verdict::False)
                }
                _ => false,
            };
            if !valid {
                continue;
            }
            if let (Some(filter), Some(stats)) = (flat.params.get(1), flat.params.first().and_then(|p| Stats::parse(p)))
            {
                if improvement.is_some_and(|imp| {
                    crate::world::filters::multi_filter(filter, &|f| {
                        if f == imp.name || f == "All" || f == "Improvement" || imp.uniques.iter().any(|u| u.text == f)
                        {
                            Verdict::True
                        } else {
                            Verdict::False
                        }
                    }) == Verdict::True
                }) {
                    improved += stats;
                } else if tile_matches(&context, tile, filter) == Verdict::True {
                    terrain += stats;
                }
            }
        }
        if tile == city.centre {
            if let Some(gap) = crate::world::yields::missing_from_minimum(
                breakdown.iter().map(|(_, stats)| *stats).sum(),
                crate::world::yields::city_centre_minimum(r),
            ) {
                terrain += gap;
            }
        }
        if golden_age && !breakdown.iter().map(|(_, stats)| stats.gold).sum::<Fixed>().is_zero() {
            terrain.gold += Fixed::ONE;
        }
        for u in &modifiers {
            if applies(u, &context) != Verdict::True {
                continue;
            }
            let all = u.unique_type() == Some(UniqueType::AllStatsPercentFromObject);
            let Some(filter) = u.params.get(if all { 1 } else { 2 }) else { continue };
            let on_improvement = improvement.is_some_and(|imp| {
                crate::world::filters::multi_filter(filter, &|f| {
                    if f == imp.name || f == "All" || f == "Improvement" || imp.uniques.iter().any(|u| u.text == f) {
                        Verdict::True
                    } else {
                        Verdict::False
                    }
                }) == Verdict::True
            });
            let affected = if on_improvement {
                improved
            } else if tile_matches(&context, tile, filter) == Verdict::True {
                terrain
            } else {
                continue;
            };
            let pct = u
                .params
                .first()
                .and_then(|p| p.parse::<i64>().ok())
                .map(|p| Fixed::from_fraction(p, 100))
                .unwrap_or_default();
            for stat in Stat::ALL {
                if all || u.params.get(1).and_then(|s| Stat::parse(s)) == Some(stat) {
                    bonus[stat] += affected[stat] * pct;
                }
            }
        }
    }
    bonus
}

fn finish_city_yield(grid: &TileGrid, r: &Ruleset, city: &City, golden_age: bool, tile_bonus: Stats) -> Stats {
    let base: Stats = city_yield_breakdown(grid, r, city).into_iter().map(|(_, s)| s).sum();
    let golden_tiles = golden_age.then(|| {
        yield_tiles(grid, r, city)
            .into_iter()
            .filter(|&tile| crate::world::golden_age::gains_gold(r, grid, tile))
            .count()
    });
    finish_output(r, city, golden_tiles, base + tile_bonus, Stats::ZERO, Fixed::from_fraction(1, 4))
}

fn finish_output(
    r: &Ruleset,
    city: &City,
    golden_tiles: Option<usize>,
    mut base: Stats,
    percent: Stats,
    conversion_rate: Fixed,
) -> Stats {
    let mut bonuses = city_percent_bonuses(r, city);
    for stat in Stat::ALL {
        if !percent[stat].is_zero() {
            bonuses
                .entry(stat)
                .or_default()
                .add(Modifier::additive_percent(percent[stat], Source::new(SourceKind::Other, "Ruleset")));
        }
    }
    if let Some(gold_tiles) = golden_tiles {
        base.gold += Fixed::whole(gold_tiles as i32);
        for stat in [Stat::Production, Stat::Culture] {
            bonuses.entry(stat).or_default().add(Modifier::additive_percent(
                Fixed::whole(crate::world::golden_age::PERCENT_BONUS),
                Source::new(SourceKind::Other, "Golden Age"),
            ));
        }
    }

    let mut out = base;
    out.production = bonuses.get(&Stat::Production).map_or(base.production, |stack| stack.evaluate(base.production));
    if let Some(stat) = city.production_conversion {
        base[stat] += out.production * conversion_rate;
        out[stat] = base[stat];
    }
    for (stat, stack) in &bonuses {
        if *stat != Stat::Production {
            out[*stat] = stack.evaluate(base[*stat]);
        }
    }
    out
}

/// The most damage a city can absorb.
pub fn max_health(r: &Ruleset, city: &City) -> i32 {
    BASE_CITY_HEALTH
        + city.built.iter().filter_map(|id| r.buildings.get(*id)).filter_map(|b| b.city_health).sum::<i32>()
}

/// How hard a city is to take.
///
/// Base, plus its citizens, plus whatever its walls add. Unciv also folds in a
/// curve over technology known; that needs a global view of research and is
/// left out here, so a late-game city is easier to storm than it should be.
pub fn city_strength(r: &Ruleset, city: &City) -> Fixed {
    let from_buildings: Fixed =
        city.built.iter().filter_map(|id| r.buildings.get(*id)).filter_map(|b| b.city_strength).sum();
    Fixed::permille(CITY_STRENGTH_BASE)
        + (Fixed::permille(CITY_STRENGTH_PER_POP) * Fixed::whole(city.population as i32))
        + from_buildings
}

/// Gold a city's buildings cost to keep, per turn.
pub fn city_maintenance(r: &Ruleset, city: &City) -> i32 {
    city.built
        .iter()
        .filter(|id| !city.free_buildings.contains(id))
        .filter_map(|id| r.buildings.get(*id))
        .filter_map(|b| b.maintenance)
        .sum()
}

/// Rebuild the city's unique index from what it has built.
pub fn rebuild_city_uniques(r: &Ruleset, city: &mut City) {
    let uniques: Vec<_> =
        city.built.iter().filter_map(|id| r.buildings.get(*id)).flat_map(|b| b.uniques.iter().cloned()).collect();
    city.uniques = crate::world::unique_index::UniqueIndex::build(&uniques);
}

/// Food left after the citizens eat. Negative means starving.
pub fn food_surplus(grid: &TileGrid, r: &Ruleset, city: &City) -> i32 {
    city_yield(grid, r, city).food.round() as i32 - food_consumed(city)
}

/// Apply one turn of food. Mirrors `CityPopulationManager.nextTurn`.
pub fn apply_food(city: &mut City, food: i32, m: GrowthModifiers) -> GrowthEvent {
    apply_food_with_rules(city, food, m, 0, false)
}
pub fn apply_food_with_rules(
    city: &mut City,
    food: i32,
    m: GrowthModifiers,
    carry_percent: i32,
    nullified: bool,
) -> GrowthEvent {
    city.food_stored = city.food_stored.saturating_add(food);

    if city.food_stored < 0 {
        // Starvation costs a citizen, never the last one, and empties the store.
        if city.population > 1 {
            city.population -= 1;
        }
        city.food_stored = 0;
        return GrowthEvent::Starved { population: city.population };
    }

    let needed = food_to_next_population(city.population, m);
    if city.food_stored < needed {
        return GrowthEvent::Stored { food: city.food_stored };
    }
    if nullified {
        return GrowthEvent::Held { food: city.food_stored };
    }
    if city.avoid_growth {
        city.food_stored = needed;
        return GrowthEvent::Held { food: city.food_stored };
    }

    city.food_stored -= needed;
    city.food_stored += (i64::from(needed) * i64::from(carry_percent.clamp(0, 95)) / 100) as i32;
    city.population += 1;
    GrowthEvent::Grew { population: city.population, carried_over: city.food_stored }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::grid::TileGrid;

    fn ruleset() -> Ruleset {
        Ruleset::load_dir("rules/gods-and-kings").expect("readable").expect("loads")
    }

    fn owned_grid(r: &Ruleset, terrain: &str, owner: CivId) -> TileGrid {
        let mut g = TileGrid::new(16, 16, r.terrain(terrain).unwrap());
        g.owner.iter_mut().for_each(|o| *o = Some(owner));
        g
    }

    /// The curve from civilization.wikia, which Unciv cites in the same place.
    #[test]
    fn the_growth_curve_matches_civ_v() {
        let m = GrowthModifiers::default();
        for (pop, want) in [(1, 15), (2, 24), (3, 33), (5, 55), (10, 114), (20, 249)] {
            assert_eq!(food_to_next_population(pop, m), want, "population {pop}");
        }
    }

    #[test]
    fn speed_and_difficulty_scale_the_curve() {
        let base = food_to_next_population(5, GrowthModifiers::default());
        let slow = food_to_next_population(5, GrowthModifiers { speed: Fixed::whole(3), civ: Fixed::ONE });
        let city_state = food_to_next_population(5, GrowthModifiers { speed: Fixed::ONE, civ: Fixed::permille(1_500) });
        assert_eq!(slow, base * 3, "marathon triples the requirement");
        // 55 × 1.5 = 82.5, truncated to 82 — matching Kotlin's `toInt()`.
        assert_eq!(city_state, 82, "city-states need half again, truncated");
    }

    #[test]
    fn a_city_works_its_centre_for_free() {
        let r = ruleset();
        let g = owned_grid(&r, "Grassland", CivId(0));
        let c = City::found("Rome", CivId(0), g.id_at(8, 8).unwrap());
        assert!(c.worked.is_empty());
        assert_eq!(city_yield(&g, &r, &c).food, Fixed::whole(2), "the centre still yields");
    }

    #[test]
    fn workable_tiles_are_the_owned_ones_in_range() {
        let r = ruleset();
        let mut g = owned_grid(&r, "Grassland", CivId(0));
        let centre = g.id_at(8, 8).unwrap();
        let c = City::found("Rome", CivId(0), centre);

        // Radius 3 is 1 + 3·3·4 = 37 tiles, less the centre.
        assert_eq!(workable_tiles(&g, &c).len(), 36);

        // A neighbour owned by someone else drops out.
        let stolen = g.neighbours(centre).next().unwrap();
        g.owner[stolen.0 as usize] = Some(CivId(1));
        let workable = workable_tiles(&g, &c);
        assert_eq!(workable.len(), 35);
        assert!(!workable.contains(&stolen));
    }

    #[test]
    fn a_city_grows_when_the_stores_fill() {
        let m = GrowthModifiers::default();
        let mut c = City::found("Rome", CivId(0), TileId(0));
        assert_eq!(apply_food(&mut c, 14, m), GrowthEvent::Stored { food: 14 });
        assert_eq!(c.population, 1);

        assert_eq!(
            apply_food(&mut c, 3, m),
            GrowthEvent::Grew { population: 2, carried_over: 2 },
            "15 needed, 17 stored, 2 carries over"
        );
        assert_eq!(c.food_stored, 2);
    }

    #[test]
    fn starvation_costs_a_citizen_but_never_the_last() {
        let m = GrowthModifiers::default();
        let mut c = City::found("Rome", CivId(0), TileId(0));
        c.population = 3;

        assert_eq!(apply_food(&mut c, -1, m), GrowthEvent::Starved { population: 2 });
        assert_eq!(c.food_stored, 0, "the store empties");

        c.population = 1;
        assert_eq!(apply_food(&mut c, -5, m), GrowthEvent::Starved { population: 1 });
        assert_eq!(c.population, 1, "a city never starves out of existence");
    }

    #[test]
    fn avoid_growth_caps_the_store() {
        let m = GrowthModifiers::default();
        let mut c = City::found("Rome", CivId(0), TileId(0));
        c.avoid_growth = true;
        assert_eq!(apply_food(&mut c, 100, m), GrowthEvent::Held { food: 15 });
        assert_eq!(c.population, 1, "held at size 1");
        assert_eq!(c.food_stored, 15, "capped at the requirement, not 100");
    }

    #[test]
    fn citizens_eat_and_the_surplus_is_what_is_left() {
        let r = ruleset();
        let g = owned_grid(&r, "Grassland", CivId(0));
        let mut c = City::found("Rome", CivId(0), g.id_at(8, 8).unwrap());

        // Size 1 on grassland: centre gives 2 food, the citizen eats 2.
        assert_eq!(food_consumed(&c), 2);
        assert_eq!(food_surplus(&g, &r, &c), 0);

        // Working another grassland tile: +2 food in, +0 eaten (still one
        // citizen doing the working).
        c.worked.push(workable_tiles(&g, &c)[0]);
        assert_eq!(food_surplus(&g, &r, &c), 2);
    }

    /// A city on grassland should reach size 2 in a predictable number of turns.
    #[test]
    fn a_whole_growth_cycle_runs() {
        let r = ruleset();
        let g = owned_grid(&r, "Grassland", CivId(0));
        let m = GrowthModifiers::default();
        let mut c = City::found("Rome", CivId(0), g.id_at(8, 8).unwrap());
        c.worked.push(workable_tiles(&g, &c)[0]);

        let mut turns = 0;
        loop {
            let surplus = food_surplus(&g, &r, &c);
            assert!(surplus > 0, "a worked grassland tile must feed growth");
            if let GrowthEvent::Grew { population, .. } = apply_food(&mut c, surplus, m) {
                assert_eq!(population, 2);
                break;
            }
            turns += 1;
            assert!(turns < 50, "should not take this long");
        }
        // 15 food needed at +2 a turn.
        assert_eq!(turns, 7, "eight turns of +2 food reaches 15");
    }
}

#[cfg(test)]
mod building_tests {
    use super::*;
    use crate::ruleset::queries::HasStats;
    use crate::world::construction::{
        apply_production, production_cost, Construction, ConstructionEvent, CostModifiers,
    };
    use crate::world::grid::CivId;

    fn ruleset() -> Ruleset {
        Ruleset::load_dir("rules/gods-and-kings").expect("readable").expect("loads")
    }

    fn grid(r: &Ruleset) -> TileGrid {
        let mut g = TileGrid::new(16, 16, r.terrain("Grassland").unwrap());
        g.owner.iter_mut().for_each(|o| *o = Some(CivId(0)));
        g
    }

    #[test]
    fn a_buildings_flat_yields_reach_the_city() {
        let r = ruleset();
        let g = grid(&r);
        let mut c = City::found("Rome", CivId(0), g.id_at(8, 8).unwrap());
        let before = city_yield(&g, &r, &c);

        // Any building with flat output of its own.
        let (id, b) = r.buildings.iter().find(|(_, b)| !b.stats().is_zero()).expect("some building yields something");
        c.built.insert(id);
        let after = city_yield(&g, &r, &c);
        println!("{}: {} -> {}", b.name, before, after);
        assert_ne!(after, before, "a built building must change the city's output");
    }

    /// Percentages apply to whatever the flat contributions produced — and a
    /// building usually supplies both, so the base moves when it is added.
    /// The contract is that `city_yield` is the breakdown, then the stacks.
    #[test]
    fn percentage_bonuses_apply_to_the_summed_base() {
        let r = ruleset();
        let g = grid(&r);
        let mut c = City::found("Rome", CivId(0), g.id_at(8, 8).unwrap());
        c.population = 4;

        // The json writes these keys in lower case; matching only "Science"
        // silently found nothing, which is what `Stat::parse` now handles.
        let boosters: Vec<_> = r
            .buildings
            .iter()
            .filter_map(|(id, b)| {
                b.percent_stat_bonus
                    .iter()
                    .find(|(k, _)| Stat::parse(k) == Some(Stat::Science))
                    .map(|(_, pct)| (id, b.name.clone(), *pct))
            })
            .take(2)
            .collect();
        assert!(!boosters.is_empty(), "the ruleset has science buildings");

        for (id, name, pct) in &boosters {
            c.built.insert(*id);
            rebuild_city_uniques(&r, &mut c);

            let flat: Stats = city_yield_breakdown(&g, &r, &c).into_iter().map(|(_, s)| s).sum();
            let total = city_yield(&g, &r, &c);
            let stacks = city_percent_bonuses(&r, &c);
            println!("{name} (+{pct}%): flat {} -> total {}", flat.science, total.science);
            assert_eq!(
                total.science,
                stacks[&Stat::Science].evaluate(flat.science),
                "the total must be the flat sum through the stack"
            );
        }

        // And the stack is additive, so two +50%s are +100%, not +125%.
        if boosters.len() == 2 {
            let stacks = city_percent_bonuses(&r, &c);
            let sum: i32 = boosters.iter().map(|(_, _, p)| *p).sum();
            assert_eq!(
                stacks[&Stat::Science].evaluate(Fixed::whole(100)),
                Fixed::whole(100 + sum),
                "science buildings stack additively"
            );
        }
    }

    #[test]
    fn finishing_a_building_puts_its_uniques_into_force() {
        let r = ruleset();
        let g = grid(&r);
        let m = CostModifiers::default();

        // A building that carries a recognised unique.
        let (id, kind) = r
            .buildings
            .iter()
            .filter(|(_, b)| b.cost.unwrap_or(0) > 0)
            .find_map(|(id, b)| b.uniques.iter().find_map(|u| u.unique_type()).map(|t| (id, t)))
            .expect("some building carries a unique");

        let mut c = City::found("Rome", CivId(0), g.id_at(8, 8).unwrap());
        c.queue.push(Construction::Building(id));
        assert!(!c.uniques.has(kind), "nothing is in force before it is built");

        let cost = production_cost(&r, Construction::Building(id), m).unwrap();
        let event = apply_production(&mut c, &r, cost, m);
        assert!(matches!(event, ConstructionEvent::Completed { .. }));
        assert!(c.uniques.has(kind), "{:?} should be live once built", kind);
        println!("{}: {:?} in force", r.buildings[id].name, kind);
    }

    #[test]
    fn buildings_cost_gold_to_keep() {
        let r = ruleset();
        let mut c = City::found("Rome", CivId(0), TileId(0));
        assert_eq!(city_maintenance(&r, &c), 0);

        let (id, b) =
            r.buildings.iter().find(|(_, b)| b.maintenance.unwrap_or(0) > 0).expect("some building has upkeep");
        c.built.insert(id);
        assert_eq!(city_maintenance(&r, &c), b.maintenance.unwrap());
    }
}

#[cfg(test)]
mod library_test {
    use super::*;
    use crate::world::grid::CivId;

    /// The canonical case. A Library's science is not a field — it is
    /// `[+1 Science] per [2] population [in this city]`, so it does nothing at
    /// all unless that unique is evaluated.
    #[test]
    fn a_library_gives_science_per_two_citizens() {
        let r = Ruleset::load_dir("rules/gods-and-kings").expect("readable").expect("loads");
        let mut g = TileGrid::new(16, 16, r.terrain("Grassland").unwrap());
        g.owner.iter_mut().for_each(|o| *o = Some(CivId(0)));

        let mut c = City::found("Rome", CivId(0), g.id_at(8, 8).unwrap());
        c.population = 6;
        let before = city_yield(&g, &r, &c).science;

        c.built.insert(r.building("Library").expect("Library exists"));
        rebuild_city_uniques(&r, &mut c);
        let after = city_yield(&g, &r, &c).science;

        println!("Library at population 6: {before} -> {after} science");
        assert_eq!(after - before, Fixed::whole(3), "six citizens, one science per two");

        c.population = 7;
        assert_eq!(
            city_yield(&g, &r, &c).science - Fixed::whole(7),
            Fixed::whole(3),
            "seven citizens still give three — the division floors"
        );
    }
}
