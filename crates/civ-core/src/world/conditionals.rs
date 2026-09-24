//! Deciding whether a unique applies right now.
//!
//! The algebra parses 638 unique types and every one of the 1,158 conditionals
//! in the shipped rulesets. Parsing is not applying: a unique like
//!
//! ```text
//! [-50]% [Production] [in all cities] <when below [-10] [Happiness]>
//! ```
//!
//! carries its trigger as data, and until something evaluates that trigger the
//! rule does nothing. Most of Civ's behaviour is shaped this way — the
//! happiness penalties, the terrain combat bonuses, the era gates — so this is
//! the layer that turns a parsed ruleset into a playable one.
//!
//! # Unknown is not false
//!
//! The decision that matters here. A conditional this engine cannot yet
//! evaluate returns [`Verdict::Unknown`], never `false`, and a unique with an
//! unknown conditional does **not** apply. Two reasons:
//!
//! * failing closed is the safe direction — an unapplied bonus is a visible
//!   imbalance, while a wrongly applied one looks like the rules working;
//! * and it stays *countable*. `conditional_coverage` reports how much of the
//!   ruleset is actually live, so the gap is a number rather than a feeling.
//!
//! Collapsing unknown into false would make an unimplemented conditional
//! indistinguishable from one that legitimately did not fire, which is the same
//! mistake as a silently dropped relationship — and those cost three bugs
//! earlier in this port.

use crate::arena::Id;
use crate::ruleset::{Ruleset, Terrain};
use crate::world::city::City;
use crate::world::entities::Unit;
use crate::world::grid::{CivId, TileId};
use crate::world::world::World;
use crate::{Unique, UniqueType};

/// What is known when a unique is being considered.
///
/// Everything is optional because most call sites know only part of it: a
/// ruleset query has no world, a tile yield has no combat.
#[derive(Clone, Copy)]
pub struct GameContext<'a> {
    pub ruleset: &'a Ruleset,
    pub world: Option<&'a World>,
    pub civ: Option<CivId>,
    pub city: Option<Id<City>>,
    pub tile: Option<TileId>,
    pub unit: Option<Id<Unit>>,
    /// A unit being prepared for placement can be evaluated before it has an arena id.
    pub unit_value: Option<&'a Unit>,
    pub combat: Option<CombatRole>,
    /// What this unit is fighting, when it is fighting something.
    pub opponent: Option<Opponent>,
    pub opponent_civ: Option<CivId>,
    /// The civ's current happiness, when it is known.
    pub happiness: Option<i32>,
    pub difficulty: Option<&'a crate::ruleset::Difficulty>,
    /// Whether religion is turned on for this game. A game setting, not a
    /// rule, so nothing in the world can answer it.
    pub religion_enabled: bool,
    /// What the civ knows, supplied directly.
    ///
    /// Tech conditionals otherwise need a whole `World` to look the civ up,
    /// and a city computing its own yields has a ruleset and a grid but no
    /// world. Supplying the set directly keeps that call site honest instead
    /// of forcing it to fabricate one.
    pub techs: Option<&'a std::collections::BTreeSet<Id<crate::ruleset::Technology>>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CombatRole {
    Attacking,
    Defending,
}

/// The other side of a fight. Cities are not units and are filtered
/// differently, so they are a separate case rather than a special unit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Opponent {
    Unit(Id<Unit>),
    City,
}

impl<'a> GameContext<'a> {
    pub fn new(ruleset: &'a Ruleset) -> GameContext<'a> {
        GameContext {
            ruleset,
            world: None,
            civ: None,
            city: None,
            tile: None,
            unit: None,
            unit_value: None,
            combat: None,
            opponent: None,
            opponent_civ: None,
            happiness: None,
            difficulty: None,
            religion_enabled: true,
            techs: None,
        }
    }

    pub fn with_world(mut self, w: &'a World) -> Self {
        self.world = Some(w);
        self
    }
    pub fn for_civ(mut self, c: CivId) -> Self {
        self.civ = Some(c);
        self
    }
    pub fn on_tile(mut self, t: TileId) -> Self {
        self.tile = Some(t);
        self
    }
    pub fn in_combat(mut self, role: CombatRole) -> Self {
        self.combat = Some(role);
        self
    }
    pub fn against(mut self, opponent: Opponent) -> Self {
        self.opponent = Some(opponent);
        self
    }
    pub fn in_city(mut self, city: Id<City>) -> Self {
        self.city = Some(city);
        self
    }
    pub fn for_unit_value(mut self, unit: &'a Unit) -> Self {
        self.unit_value = Some(unit);
        self
    }
    pub fn for_unit(mut self, unit: Id<Unit>) -> Self {
        self.unit = Some(unit);
        self
    }
    pub fn with_happiness(mut self, h: i32) -> Self {
        self.happiness = Some(h);
        self
    }
    pub fn knowing(mut self, techs: &'a std::collections::BTreeSet<Id<crate::ruleset::Technology>>) -> Self {
        self.techs = Some(techs);
        self
    }
}

/// The three-valued answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    True,
    False,
    /// Either the conditional is not implemented, or the context lacks what it
    /// needs. Both mean "do not apply", and both are worth counting.
    Unknown,
}

impl Verdict {
    pub fn holds(self) -> bool {
        self == Verdict::True
    }
}

/// Does a tile match a filter?
///
/// Goes through the same filter language as units, because the ruleset uses it
/// here too: of 129 tile-filter uses in Gods & Kings, 84 are not plain terrain
/// names, and most of those are conjunctions like `{Featureless} {Tundra}` or
/// `{Grassland} {Hill}`.
pub fn tile_matches(ctx: &GameContext, tile: TileId, filter: &str) -> Verdict {
    let Some(world) = ctx.world else { return Verdict::Unknown };
    crate::world::filters::multi_filter(filter, &|term| {
        if let Some(resource) = ctx.ruleset.resources.id(term) {
            if let Some(civ) = ctx.civ.and_then(|id| world.civ(id)) {
                return if world.grid.resource[tile.0 as usize] == Some(resource)
                    && crate::world::resources::is_visible(ctx.ruleset, civ, resource)
                {
                    Verdict::True
                } else {
                    Verdict::False
                };
            }
        }
        if term == "City center" {
            return if world.cities.iter().any(|(_, city)| city.centre == tile) {
                Verdict::True
            } else {
                Verdict::False
            };
        }
        if matches!(term, "Friendly Land" | "Friendly" | "Foreign Land" | "Foreign") {
            let Some(civ) = ctx.civ else { return Verdict::Unknown };
            let friendly = crate::world::movement::is_friendly_territory(world, civ, tile);
            let matches = if matches!(term, "Friendly Land" | "Friendly") { friendly } else { !friendly };
            return if matches { Verdict::True } else { Verdict::False };
        }
        tile_matches_in(&world.grid, ctx.ruleset, tile, term, ctx.civ)
    })
}

/// The same question asked of a grid directly, for callers that have no
/// `World` — city yields are computed from a grid and a ruleset alone.
pub fn tile_matches_in(
    grid: &crate::world::grid::TileGrid,
    ruleset: &Ruleset,
    tile: TileId,
    filter: &str,
    civ: Option<CivId>,
) -> Verdict {
    crate::world::filters::multi_filter(filter, &|term| tile_matches_term(grid, ruleset, tile, term, civ))
}

fn tile_matches_term(
    world_grid: &crate::world::grid::TileGrid,
    r: &Ruleset,
    tile: TileId,
    filter: &str,
    civ: Option<CivId>,
) -> Verdict {
    struct G<'a>(&'a crate::world::grid::TileGrid);
    let world = G(world_grid);
    let terrains: Vec<&crate::ruleset::Terrain> =
        world.0.terrains(tile).filter_map(|id: Id<Terrain>| r.terrains.get(id)).collect();
    let yes = |b: bool| if b { Verdict::True } else { Verdict::False };

    if terrains.iter().any(|t| t.name == filter) {
        return Verdict::True;
    }

    // "Rough" is declared on the terrain as a unique, not as a field.
    let rough =
        |t: &crate::ruleset::Terrain| t.uniques.iter().any(|u| u.unique_type() == Some(UniqueType::RoughTerrain));

    match filter {
        "Coastal" => {
            return yes(!world.0.is_water(r, tile)
                && world.0.neighbours(tile).any(|next| {
                    r.terrains.get(world.0.terrain[next.0 as usize]).is_some_and(|terrain| terrain.name == "Coast")
                }))
        }
        "your" => return civ.map_or(Verdict::Unknown, |owner| yes(world.0.owner[tile.0 as usize] == Some(owner))),
        "River" => return yes(world.0.has_river(tile)),
        "Fresh water" => return yes(world.0.is_fresh_water(r, tile)),
        "All" | "all" => return Verdict::True,
        "Land" => return yes(terrains.iter().any(|t| t.r#type == "Land")),
        "Water" => return yes(terrains.iter().any(|t| t.r#type == "Water")),
        "Natural Wonder" => return yes(world.0.natural_wonder[tile.0 as usize].is_some()),
        "unimproved" => return yes(world.0.unpillaged_improvement(tile).is_none()),
        // Features are everything after the base terrain.
        "Featureless" => return yes(world.0.features[tile.0 as usize].is_empty()),
        "Rough terrain" => return yes(terrains.iter().copied().any(rough)),
        // Deliberately `all`, not `!any`: Unciv notes that one open terrain
        // among rough ones does not make the tile open.
        "Open terrain" => return yes(terrains.iter().copied().all(|t| !rough(t))),
        "Friendly Land" | "Foreign Land" => {
            let (Some(civ), owner) = (civ, world.0.owner[tile.0 as usize]) else {
                return Verdict::Unknown;
            };
            let friendly = owner == Some(civ);
            return yes(if filter == "Friendly Land" { friendly } else { !friendly });
        }
        _ => {}
    }

    if let Some(id) = r.resources.id(filter) {
        return yes(world.0.resource[tile.0 as usize] == Some(id));
    }
    if let Some(id) = r.improvements.id(filter) {
        return yes(world.0.unpillaged_improvement(tile) == Some(id));
    }
    // A term naming a terrain this ruleset has is a definite no; anything else
    // is beyond this function and stays countable.
    if r.terrains.id(filter).is_some() {
        Verdict::False
    } else {
        Verdict::Unknown
    }
}

fn number(param: Option<&String>) -> Option<i32> {
    param.and_then(|p| p.trim().parse::<i32>().ok())
}

/// Evaluate one conditional.
pub fn evaluate(cond: &Unique, ctx: &GameContext) -> Verdict {
    let Some(kind) = cond.unique_type() else { return Verdict::Unknown };
    let p = |i: usize| cond.params.get(i);

    match kind {
        // --- combat posture ---------------------------------------------
        UniqueType::ConditionalAttacking => match ctx.combat {
            Some(CombatRole::Attacking) => Verdict::True,
            Some(CombatRole::Defending) => Verdict::False,
            None => Verdict::Unknown,
        },
        UniqueType::ConditionalDefending => match ctx.combat {
            Some(CombatRole::Defending) => Verdict::True,
            Some(CombatRole::Attacking) => Verdict::False,
            None => Verdict::Unknown,
        },

        // --- where the unit is ------------------------------------------
        UniqueType::ConditionalFightingInTiles => match (ctx.tile, p(0)) {
            (Some(t), Some(f)) => tile_matches(ctx, t, f),
            _ => Verdict::Unknown,
        },
        UniqueType::ConditionalInTiles => match (
            ctx.unit_value
                .or_else(|| ctx.world.and_then(|w| ctx.unit.and_then(|id| w.units.get(id))))
                .map(|u| u.position)
                .or(ctx.tile),
            p(0),
        ) {
            (Some(t), Some(f)) => tile_matches(ctx, t, f),
            _ => Verdict::Unknown,
        },
        UniqueType::ConditionalInTilesNot => match (
            ctx.unit_value
                .or_else(|| ctx.world.and_then(|w| ctx.unit.and_then(|id| w.units.get(id))))
                .map(|u| u.position)
                .or(ctx.tile),
            p(0),
        ) {
            (Some(t), Some(f)) => match tile_matches(ctx, t, f) {
                Verdict::True => Verdict::False,
                Verdict::False => Verdict::True,
                Verdict::Unknown => Verdict::Unknown,
            },
            _ => Verdict::Unknown,
        },

        // --- who is fighting whom ---------------------------------------
        // `for [] units` filters the unit carrying the rule; `vs [] units`
        // filters what it is up against. Both go through the same filter
        // language, so `{Military} {Water}` works in either position.
        UniqueType::ConditionalOurUnit | UniqueType::ConditionalOurUnitOnUnit => {
            let (Some(world), Some(filter)) = (ctx.world, p(0)) else {
                return Verdict::Unknown;
            };
            ctx.unit_value
                .or_else(|| ctx.unit.and_then(|id| world.units.get(id)))
                .map_or(Verdict::Unknown, |u| crate::world::filters::unit_matches_in(world, ctx.ruleset, u, filter))
        }
        UniqueType::ConditionalVsUnits | UniqueType::ConditionalVsCombatant => {
            let (Some(world), Some(filter)) = (ctx.world, p(0)) else { return Verdict::Unknown };
            match ctx.opponent {
                Some(Opponent::Unit(id)) => world.units.get(id).map_or(Verdict::Unknown, |u| {
                    crate::world::filters::unit_matches_in(world, ctx.ruleset, u, filter)
                }),
                // A city is not a unit, so no unit filter matches it.
                Some(Opponent::City) if cond.unique_type() == Some(UniqueType::ConditionalVsCombatant) => {
                    let matches = filter == "City"
                        || filter == "All"
                        || (matches!(filter.as_str(), "City-State" | "City-States")
                            && ctx.opponent_civ.and_then(|c| world.civ(c)).is_some_and(|c| c.is_city_state()));
                    if matches {
                        Verdict::True
                    } else {
                        Verdict::False
                    }
                }
                Some(Opponent::City) => Verdict::False,
                None => Verdict::Unknown,
            }
        }
        UniqueType::ConditionalVsCity => match ctx.opponent {
            Some(Opponent::City) => Verdict::True,
            Some(Opponent::Unit(_)) => Verdict::False,
            None => Verdict::Unknown,
        },

        UniqueType::ConditionalVsLargerCiv => {
            let Some(world) = ctx.world else { return Verdict::Unknown };
            let other = ctx.opponent_civ.or_else(|| match ctx.opponent {
                Some(Opponent::Unit(id)) => world.units.get(id).map(|u| u.owner),
                _ => None,
            });
            match (ctx.civ.and_then(|c| world.civ(c)), other.and_then(|c| world.civ(c))) {
                (Some(ours), Some(theirs)) => {
                    if ours.cities.len() < theirs.cities.len() {
                        Verdict::True
                    } else {
                        Verdict::False
                    }
                }
                _ => Verdict::Unknown,
            }
        }
        UniqueType::ConditionalWhenGarrisoned => {
            let Some(world) = ctx.world else { return Verdict::Unknown };
            let Some(city) = ctx.city.and_then(|id| world.cities.get(id)) else { return Verdict::Unknown };
            if world.units.iter().any(|(_, u)| {
                u.owner == city.owner
                    && u.position == city.centre
                    && u.is_military(ctx.ruleset)
                    && u.is_land_unit(ctx.ruleset)
            }) {
                Verdict::True
            } else {
                Verdict::False
            }
        }
        UniqueType::ConditionalAdjacentUnit
        | UniqueType::ConditionalStackedWithUnit
        | UniqueType::ConditionalNotStackedWithUnit => {
            let (Some(world), Some(filter)) = (ctx.world, p(0)) else { return Verdict::Unknown };
            let Some(unit) = ctx.unit_value.or_else(|| ctx.unit.and_then(|id| world.units.get(id))) else {
                return Verdict::Unknown;
            };
            let adjacent = cond.unique_type() == Some(UniqueType::ConditionalAdjacentUnit);
            let matched = world.units.iter().any(|(id, other)| {
                Some(id) != ctx.unit
                    && (if adjacent {
                        other.owner == unit.owner && world.grid.neighbours(unit.position).any(|t| t == other.position)
                    } else {
                        other.position == unit.position
                    })
                    && crate::world::filters::unit_matches_in(world, ctx.ruleset, other, filter).holds()
            });
            if matched != (cond.unique_type() == Some(UniqueType::ConditionalNotStackedWithUnit)) {
                Verdict::True
            } else {
                Verdict::False
            }
        }
        UniqueType::ConditionalNeighborTiles => {
            let (Some(world), Some(min), Some(max), Some(filter)) = (
                ctx.world,
                p(0).and_then(|s| s.parse::<usize>().ok()),
                p(1).and_then(|s| s.parse::<usize>().ok()),
                p(2),
            ) else {
                return Verdict::Unknown;
            };
            let tile = ctx
                .tile
                .or_else(|| ctx.unit_value.or_else(|| ctx.unit.and_then(|id| world.units.get(id))).map(|u| u.position))
                .or_else(|| ctx.city.and_then(|id| world.cities.get(id)).map(|c| c.centre));
            let Some(tile) = tile else { return Verdict::Unknown };
            let count = world.grid.neighbours(tile).filter(|&tile| tile_matches(ctx, tile, filter).holds()).count();
            if (min..=max).contains(&count) {
                Verdict::True
            } else {
                Verdict::False
            }
        }
        UniqueType::ConditionalNearTiles => {
            let (Some(world), Some(distance), Some(filter)) =
                (ctx.world, p(0).and_then(|s| s.parse::<u32>().ok()), p(1))
            else {
                return Verdict::Unknown;
            };
            let Some(tile) =
                ctx.unit_value.or_else(|| ctx.unit.and_then(|id| world.units.get(id))).map(|u| u.position).or(ctx.tile)
            else {
                return Verdict::Unknown;
            };
            if world.grid.within(tile, distance).into_iter().any(|t| tile_matches(ctx, t, filter).holds()) {
                Verdict::True
            } else {
                Verdict::False
            }
        }

        // --- what the civ has done ---------------------------------------
        UniqueType::ConditionalAfterPolicyOrBelief => match adopted(ctx, p(0)) {
            Some(true) => Verdict::True,
            Some(false) => Verdict::False,
            None => Verdict::Unknown,
        },
        UniqueType::ConditionalBeforePolicyOrBelief => match adopted(ctx, p(0)) {
            Some(true) => Verdict::False,
            Some(false) => Verdict::True,
            None => Verdict::Unknown,
        },

        // --- what has been built -----------------------------------------
        UniqueType::ConditionalBuildingBuilt
        | UniqueType::ConditionalBuildingNotBuilt
        | UniqueType::ConditionalCityWithBuilding
        | UniqueType::ConditionalCityWithoutBuilding => {
            let built =
                if matches!(kind, UniqueType::ConditionalBuildingBuilt | UniqueType::ConditionalBuildingNotBuilt) {
                    let (Some(world), Some(civ), Some(filter)) = (ctx.world, ctx.civ, p(0)) else {
                        return Verdict::Unknown;
                    };
                    Some(world.cities.iter().filter(|(_, c)| c.owner == civ).any(|(_, c)| {
                        c.built
                            .iter()
                            .any(|id| crate::world::filters::building_matches(ctx.ruleset, *id, filter).holds())
                    }))
                } else {
                    built_here(ctx, p(0))
                };
            let Some(built) = built else { return Verdict::Unknown };
            let positive =
                matches!(kind, UniqueType::ConditionalBuildingBuilt | UniqueType::ConditionalCityWithBuilding);
            verdict(built == positive)
        }
        UniqueType::ConditionalBuildingBuiltAll | UniqueType::ConditionalBuildingBuiltAmount => {
            let (Some(world), Some(civ), Some(filter)) = (ctx.world, ctx.civ, p(0)) else { return Verdict::Unknown };
            let Some(owner) = world.civ(civ) else { return Verdict::Unknown };
            let city_filter =
                p(if kind == UniqueType::ConditionalBuildingBuiltAll { 1 } else { 2 }).map_or("All", String::as_str);
            let selected: Vec<_> = owner
                .cities
                .iter()
                .filter_map(|id| world.cities.get(*id))
                .filter(|c| {
                    crate::world::filters::multi_filter(city_filter, &|term| {
                        crate::world::filters::city_matches(world, ctx.ruleset, c, Some(c), term)
                    })
                    .holds()
                })
                .collect();
            let present = |c: &&City| {
                c.built.iter().any(|id| crate::world::filters::building_matches(ctx.ruleset, *id, filter).holds())
            };
            if kind == UniqueType::ConditionalBuildingBuiltAll {
                verdict(selected.iter().all(present))
            } else {
                verdict(
                    selected.iter().filter(|c| present(c)).count() >= number(p(1)).unwrap_or(i32::MAX).max(0) as usize,
                )
            }
        }
        UniqueType::ConditionalBuildingBuiltByAnybody | UniqueType::ConditionalBuildingNotBuiltByAnybody => {
            let (Some(world), Some(filter)) = (ctx.world, p(0)) else { return Verdict::Unknown };
            let built = world.cities.iter().any(|(_, c)| {
                c.built.iter().any(|id| crate::world::filters::building_matches(ctx.ruleset, *id, filter).holds())
            });
            verdict(built == (kind == UniqueType::ConditionalBuildingBuiltByAnybody))
        }

        // --- the city itself ----------------------------------------------
        UniqueType::ConditionalCityFilter => {
            let (Some(world), Some(city), Some(filter)) = (ctx.world, ctx.city, p(0)) else {
                return Verdict::Unknown;
            };
            let Some(city) = world.cities.get(city) else { return Verdict::Unknown };
            crate::world::filters::multi_filter(filter, &|term| {
                crate::world::filters::city_matches(world, ctx.ruleset, city, Some(city), term)
            })
        }
        UniqueType::ConditionalWLTKD => match (ctx.world, ctx.city) {
            (Some(world), Some(city)) => match world.cities.get(city) {
                Some(c) if c.wltkd > 0 => Verdict::True,
                Some(_) => Verdict::False,
                None => Verdict::Unknown,
            },
            _ => Verdict::Unknown,
        },
        UniqueType::ConditionalPopulationFilter
        | UniqueType::ConditionalBelowPopulationFilter
        | UniqueType::ConditionalBetweenPopulationFilter => {
            let (Some(world), Some(city), Some(amount)) = (ctx.world, ctx.city, number(p(0))) else {
                return Verdict::Unknown;
            };
            let Some(c) = world.cities.get(city) else { return Verdict::Unknown };
            let filter =
                p(if kind == UniqueType::ConditionalBetweenPopulationFilter { 2 } else { 1 }).map(String::as_str);
            let n = match filter {
                Some("Population") => c.population,
                Some("Specialists") => crate::world::specialists::count(c),
                Some("Followers of this Religion") => c.faith.majority_followers(c.population),
                _ => return Verdict::Unknown,
            } as i32;
            verdict(match kind {
                UniqueType::ConditionalBelowPopulationFilter => n < amount,
                UniqueType::ConditionalBetweenPopulationFilter => n >= amount && n <= number(p(1)).unwrap_or(i32::MIN),
                _ => n >= amount,
            })
        }
        UniqueType::ConditionalCityMajorReligion | UniqueType::ConditionalCityEnhancedReligion => {
            let Some(c) = ctx.world.zip(ctx.city).and_then(|(w, id)| w.cities.get(id)) else { return Verdict::Unknown };
            let Some(world) = ctx.world else { return Verdict::Unknown };
            verdict(c.faith.majority(c.population).is_some_and(|creed| {
                if kind == UniqueType::ConditionalCityEnhancedReligion {
                    world.civ(creed).is_some_and(|f| f.faith.enhanced)
                } else {
                    crate::world::religion::religion_exists(world, creed)
                }
            }))
        }
        UniqueType::ConditionalInThisCity => verdict(ctx.city.is_some()),
        UniqueType::ConditionalCityConnected => {
            let (Some(world), Some(civ), Some(city)) = (ctx.world, ctx.civ, ctx.city) else { return Verdict::Unknown };
            let links = crate::world::connections::find(world, ctx.ruleset, civ);
            verdict(links.has_trade_route(city) || links.capital() == Some(city))
        }

        // --- how far the civ has come -------------------------------------
        UniqueType::ConditionalForeignContinent => {
            let (Some(world), Some(civ), Some(tile)) = (ctx.world, ctx.civ, ctx.tile) else {
                return Verdict::Unknown;
            };
            verdict(
                crate::world::connections::capital_of(world, ctx.ruleset, civ)
                    .and_then(|id| world.cities.get(id))
                    .is_none_or(|city| {
                        !crate::world::settling::same_continent(&world.grid, ctx.ruleset, city.centre, tile)
                    }),
            )
        }
        UniqueType::ConditionalStartingFromEra => {
            let (Some(world), Some(civ), Some(era)) = (ctx.world, ctx.civ, p(0)) else {
                return Verdict::Unknown;
            };
            let Some(target) = ctx.ruleset.eras.by_name(era) else { return Verdict::Unknown };
            let reached = crate::world::policies::era_number(world, ctx.ruleset, civ);
            if reached >= target.era_number {
                Verdict::True
            } else {
                Verdict::False
            }
        }

        UniqueType::ConditionalBeforeEra => {
            let (Some(world), Some(civ), Some(era)) = (ctx.world, ctx.civ, p(0)) else {
                return Verdict::Unknown;
            };
            let Some(target) = ctx.ruleset.eras.by_name(era) else { return Verdict::Unknown };
            let reached = crate::world::policies::era_number(world, ctx.ruleset, civ);
            if reached < target.era_number {
                Verdict::True
            } else {
                Verdict::False
            }
        }

        // --- the game's own settings ---------------------------------------
        // --- the clock and the civ's religious progress --------------------
        UniqueType::ConditionalAfterTurns | UniqueType::ConditionalBeforeTurns => {
            let (Some(world), Some(n)) = (ctx.world, p(0).and_then(|x| x.parse::<u32>().ok())) else {
                return Verdict::Unknown;
            };
            let after = world.turn >= n;
            let want = cond.unique_type() == Some(UniqueType::ConditionalAfterTurns);
            if after == want {
                Verdict::True
            } else {
                Verdict::False
            }
        }
        UniqueType::ConditionalBeforePantheon
        | UniqueType::ConditionalAfterPantheon
        | UniqueType::ConditionalBeforeReligion
        | UniqueType::ConditionalAfterGeneratingGreatProphet => {
            let Some(c) = ctx.world.zip(ctx.civ).and_then(|(w, c)| w.civ(c)) else { return Verdict::Unknown };
            let holds = match cond.unique_type() {
                Some(UniqueType::ConditionalBeforePantheon) => !c.faith.has_pantheon(),
                Some(UniqueType::ConditionalAfterPantheon) => c.faith.has_pantheon(),
                Some(UniqueType::ConditionalBeforeReligion) => !c.faith.founded && c.faith.state.is_none(),
                _ => c.faith.prophets_earned > 0,
            };
            if holds {
                Verdict::True
            } else {
                Verdict::False
            }
        }
        UniqueType::ConditionalDifficulty
        | UniqueType::ConditionalDifficultyOrHigher
        | UniqueType::ConditionalDifficultyOrLower => {
            let (Some(difficulty), Some(name)) = (ctx.difficulty, p(0)) else { return Verdict::Unknown };
            let Some(wanted) = ctx.ruleset.difficulties.id(name) else { return Verdict::False };
            let Some(current) = ctx.ruleset.difficulties.id(&difficulty.name) else { return Verdict::Unknown };
            let holds = match cond.unique_type() {
                Some(UniqueType::ConditionalDifficultyOrHigher) => current.index() >= wanted.index(),
                Some(UniqueType::ConditionalDifficultyOrLower) => current.index() <= wanted.index(),
                _ => current == wanted,
            };
            if holds {
                Verdict::True
            } else {
                Verdict::False
            }
        }
        UniqueType::ConditionalWar | UniqueType::ConditionalNotWar => {
            let (Some(world), Some(civ)) = (ctx.world, ctx.civ) else { return Verdict::Unknown };
            let war = world.civs.iter().any(|other| {
                other.id != civ && other.is_major() && !other.defeated && world.diplomacy.at_war(civ, other.id)
            });
            verdict(war == (kind == UniqueType::ConditionalWar))
        }
        UniqueType::ConditionalGoldenAge | UniqueType::ConditionalNotGoldenAge => {
            let (Some(world), Some(civ)) = (ctx.world, ctx.civ) else { return Verdict::Unknown };
            verdict(crate::world::golden_age::is_golden(world, civ) == (kind == UniqueType::ConditionalGoldenAge))
        }
        UniqueType::ConditionalHappy => ctx.happiness.map_or(Verdict::Unknown, |n| verdict(n >= 0)),
        UniqueType::ConditionalVictoryEnabled | UniqueType::ConditionalVictoryDisabled => {
            let (Some(world), Some(name)) = (ctx.world, p(0)) else { return Verdict::Unknown };
            verdict(
                crate::world::victory::is_enabled(world, ctx.ruleset, name)
                    == (kind == UniqueType::ConditionalVictoryEnabled),
            )
        }
        // The current game model enables these systems for normal bundled-rules games.
        UniqueType::ConditionalEspionageEnabled | UniqueType::ConditionalNuclearWeaponsEnabled => Verdict::True,
        UniqueType::ConditionalEspionageDisabled | UniqueType::ConditionalNuclearWeaponsDisabled => Verdict::False,
        UniqueType::ConditionalCountableEqualTo
        | UniqueType::ConditionalCountableDifferentThan
        | UniqueType::ConditionalCountableMoreThan
        | UniqueType::ConditionalCountableLessThan
        | UniqueType::ConditionalCountableBetween => {
            let (Some(a), Some(b)) = (p(0).and_then(|v| countable(ctx, v)), p(1).and_then(|v| countable(ctx, v)))
            else {
                return Verdict::Unknown;
            };
            verdict(match kind {
                UniqueType::ConditionalCountableEqualTo => a == b,
                UniqueType::ConditionalCountableDifferentThan => a != b,
                UniqueType::ConditionalCountableMoreThan => a > b,
                UniqueType::ConditionalCountableLessThan => a < b,
                _ => p(2).and_then(|v| countable(ctx, v)).is_some_and(|high| a >= b && a <= high),
            })
        }
        UniqueType::ConditionalReligionEnabled => {
            if ctx.religion_enabled {
                Verdict::True
            } else {
                Verdict::False
            }
        }
        UniqueType::ConditionalReligionDisabled => {
            if ctx.religion_enabled {
                Verdict::False
            } else {
                Verdict::True
            }
        }

        // --- what the civ knows -----------------------------------------
        UniqueType::ConditionalTech => match knows(ctx, p(0)) {
            Some(true) => Verdict::True,
            Some(false) => Verdict::False,
            None => Verdict::Unknown,
        },
        UniqueType::ConditionalNoTech => match knows(ctx, p(0)) {
            Some(true) => Verdict::False,
            Some(false) => Verdict::True,
            None => Verdict::Unknown,
        },

        // --- thresholds on a civ-wide stat ------------------------------
        // Happiness is the one that matters: the growth and production
        // penalties of an unhappy empire are declared in GlobalUniques.json as
        // `<when below [-10] [Happiness]>`, not written in code anywhere.
        UniqueType::ConditionalWhenBelowAmountStatResource => threshold(ctx, p(0), p(1), |value, bound| value < bound),
        UniqueType::ConditionalWhenAboveAmountStatResource => threshold(ctx, p(0), p(1), |value, bound| value > bound),
        UniqueType::ConditionalWhenBetweenStatResource => {
            let (Some(low), Some(high), Some(stat)) = (number(p(0)), number(p(1)), p(2)) else {
                return Verdict::Unknown;
            };
            let Some(value) = civ_wide_stat(ctx, stat) else { return Verdict::Unknown };
            if value >= low && value <= high {
                Verdict::True
            } else {
                Verdict::False
            }
        }

        _ => Verdict::Unknown,
    }
}

/// Has the civ adopted this policy or belief? `None` when unanswerable.
///
/// One conditional covers both, because Civ treats them the same way: a
/// belief and a policy are both things a civ has taken.
fn adopted(ctx: &GameContext, name: Option<&String>) -> Option<bool> {
    let name = name?;
    let civ = ctx.world?.civ(ctx.civ?)?;
    if let Some(p) = ctx.ruleset.policies.id(name) {
        return Some(civ.adopted_policies.contains(&p));
    }
    if let Some(b) = ctx.ruleset.beliefs.id(name) {
        return Some(civ.faith.beliefs.contains(&b));
    }
    None
}

/// Is this building standing in the city under consideration?
fn built_here(ctx: &GameContext, name: Option<&String>) -> Option<bool> {
    let filter = name?;
    let world = ctx.world?;
    let built = |city: &City| {
        city.built.iter().any(|id| crate::world::filters::building_matches(ctx.ruleset, *id, filter).holds())
    };
    if let Some(city) = ctx.city {
        return Some(built(world.cities.get(city)?));
    }
    Some(world.civ(ctx.civ?)?.cities.iter().filter_map(|id| world.cities.get(*id)).any(built))
}

fn verdict(value: bool) -> Verdict {
    if value {
        Verdict::True
    } else {
        Verdict::False
    }
}

fn countable(ctx: &GameContext, value: &str) -> Option<i64> {
    if let Ok(n) = value.parse() {
        return Some(n);
    }
    let world = ctx.world?;
    let civ = ctx.civ?;
    Some(match value {
        "Completed Policy branches" => crate::world::victory::completed_branches(world, ctx.ruleset, civ) as i64,
        "Cities" => world.civ(civ)?.cities.len() as i64,
        "Population" => {
            world.civ(civ)?.cities.iter().filter_map(|id| world.cities.get(*id)).map(|c| i64::from(c.population)).sum()
        }
        "Remaining [City-State] Civilizations" => {
            world.civs.iter().filter(|c| c.is_city_state() && !c.defeated).count() as i64
        }
        "Remaining [Major] Civilizations" => world.civs.iter().filter(|c| c.is_major() && !c.defeated).count() as i64,
        _ => return None,
    })
}

/// Does the civ know this tech? `None` when the context cannot say.
///
/// Prefers a directly supplied tech set over a world lookup, so a city can
/// answer without one.
fn knows(ctx: &GameContext, tech: Option<&String>) -> Option<bool> {
    let id = ctx.ruleset.technologies.id(tech?)?;
    if let Some(techs) = ctx.techs {
        return Some(techs.contains(&id));
    }
    let world = ctx.world?;
    Some(world.civ(ctx.civ?)?.has_tech(id))
}

fn threshold(
    ctx: &GameContext,
    amount: Option<&String>,
    stat: Option<&String>,
    test: impl Fn(i32, i32) -> bool,
) -> Verdict {
    let (Some(bound), Some(stat)) = (number(amount), stat) else { return Verdict::Unknown };
    let Some(value) = civ_wide_stat(ctx, stat) else { return Verdict::Unknown };
    if test(value, bound) {
        Verdict::True
    } else {
        Verdict::False
    }
}

/// The civ-wide quantities a threshold can be written against. Only happiness
/// so far; gold and faith are stored state and belong here once they exist.
fn civ_wide_stat(ctx: &GameContext, stat: &str) -> Option<i32> {
    match stat {
        "Happiness" => ctx.happiness,
        _ => None,
    }
}

/// Does this unique apply, given everything its conditionals require?
///
/// Every conditional must hold. One that cannot be evaluated makes the whole
/// unique inapplicable — deliberately, see the module note.
pub fn applies(unique: &Unique, ctx: &GameContext) -> Verdict {
    let mut verdict = Verdict::True;
    for cond in &unique.conditionals {
        // Presentation and scaling annotations are not eligibility predicates.
        if matches!(
            cond.unique_type(),
            Some(
                UniqueType::SuppressWarnings
                    | UniqueType::ModifierHiddenFromUsers
                    | UniqueType::ModifiedByGameSpeed
                    | UniqueType::CivilopediaLink
            )
        ) {
            continue;
        }
        match evaluate(cond, ctx) {
            Verdict::True => {}
            Verdict::False => return Verdict::False,
            Verdict::Unknown => verdict = Verdict::Unknown,
        }
    }
    verdict
}

/// Which conditionals remain undecidable, and how often each appears.
///
/// The counterpart to [`conditional_coverage`]: that says how much is live,
/// this says what to implement next. Ranked by frequency, because a conditional
/// used once is worth less than one used fifty times.
pub fn undecidable_conditionals(ctx: &GameContext) -> Vec<(String, usize)> {
    let mut counts: std::collections::BTreeMap<String, usize> = Default::default();
    for_each_conditional(ctx.ruleset, &mut |c| {
        if evaluate(c, ctx) == Verdict::Unknown {
            let name = c
                .unique_type()
                .map(|t| t.placeholder().to_string())
                .unwrap_or_else(|| format!("(unrecognised) {}", c.text));
            *counts.entry(name).or_insert(0) += 1;
        }
    });
    let mut out: Vec<(String, usize)> = counts.into_iter().collect();
    out.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    out
}

/// Visit every conditional attached to every unique in the ruleset.
fn for_each_conditional(r: &Ruleset, f: &mut impl FnMut(&Unique)) {
    let all = r
        .buildings
        .values()
        .map(|b| &b.uniques)
        .chain(r.units.values().map(|u| &u.uniques))
        .chain(r.terrains.values().map(|t| &t.uniques))
        .chain(r.improvements.values().map(|i| &i.uniques))
        .chain(r.resources.values().map(|x| &x.uniques))
        .chain(r.promotions.values().map(|p| &p.uniques))
        .chain(r.policies.values().map(|p| &p.uniques))
        .chain(r.technologies.values().map(|t| &t.uniques))
        .chain(r.nations.values().map(|n| &n.uniques))
        .chain(r.nations.values().filter_map(|n| n.alt_leader.as_ref()).map(|a| &a.uniques))
        .chain(r.beliefs.values().map(|b| &b.uniques));
    // City-state bonuses are stored as raw strings rather than parsed uniques,
    // because they are granted to a *different* civ than the one that holds
    // them. They carry era conditionals like everything else, so they belong
    // in the count.
    let city_state_bonuses: Vec<Unique> = r
        .city_state_types
        .values()
        .flat_map(|t| t.friend_bonus_uniques.iter().chain(t.ally_bonus_uniques.iter()))
        .map(|text| Unique::parse(text))
        .collect();
    for uniques in all.chain(std::iter::once(&city_state_bonuses)) {
        for u in uniques {
            for c in &u.conditionals {
                f(c);
            }
        }
    }
}

/// How much of a ruleset's conditional surface this engine can actually decide.
///
/// Returns `(decidable, total)` over every conditional attached to every unique
/// in the ruleset, evaluated against a context rich enough to try. A coverage
/// number rather than a pass/fail, because it will climb as conditionals are
/// implemented and should be visible while it does.
pub fn conditional_coverage(ctx: &GameContext) -> (usize, usize) {
    let (mut decidable, mut total) = (0, 0);
    for_each_conditional(ctx.ruleset, &mut |c| {
        total += 1;
        if evaluate(c, ctx) != Verdict::Unknown {
            decidable += 1;
        }
    });
    (decidable, total)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::entities::{Civilization, Unit};
    use crate::world::grid::TileGrid;

    fn ruleset() -> Ruleset {
        Ruleset::load_dir("rules/gods-and-kings").expect("readable").expect("loads")
    }

    fn world(r: &Ruleset) -> World {
        let g = TileGrid::new(8, 8, r.terrain("Grassland").unwrap());
        let nation = r.nations.ids().next().unwrap();
        World::new(g, vec![Civilization::new(CivId(0), nation, false)])
    }

    #[test]
    fn combat_posture_is_decided_only_when_it_is_known() {
        let r = ruleset();
        let attacking = Unique::parse("when attacking");
        let ctx = GameContext::new(&r);
        assert_eq!(evaluate(&attacking, &ctx), Verdict::Unknown, "no combat in context");
        assert_eq!(evaluate(&attacking, &ctx.in_combat(CombatRole::Attacking)), Verdict::True);
        assert_eq!(evaluate(&attacking, &ctx.in_combat(CombatRole::Defending)), Verdict::False);
    }

    #[test]
    fn terrain_conditionals_read_the_tile() {
        let r = ruleset();
        let mut w = world(&r);
        let tile = TileId(9);
        w.grid.features[9].push(r.terrain("Forest").unwrap());

        let ctx = GameContext::new(&r).with_world(&w).on_tile(tile);
        assert_eq!(evaluate(&Unique::parse("in [Forest] tiles"), &ctx), Verdict::True);
        assert_eq!(evaluate(&Unique::parse("in [Hill] tiles"), &ctx), Verdict::False);
        assert_eq!(evaluate(&Unique::parse("in [Land] tiles"), &ctx), Verdict::True);
        assert_eq!(
            evaluate(&Unique::parse("in tiles without [Forest]"), &ctx),
            Verdict::False,
            "negation inverts a known answer"
        );
    }

    #[test]
    fn tile_filters_understand_conjunctions() {
        let r = ruleset();
        let mut w = world(&r);
        let tile = TileId(9);
        w.grid.features[9].push(r.terrain("Hill").unwrap());
        let ctx = GameContext::new(&r).with_world(&w).on_tile(tile);

        // The form the ruleset actually uses, 84 times out of 129.
        assert_eq!(evaluate(&Unique::parse("in [{Grassland} {Hill}] tiles"), &ctx), Verdict::True);
        assert_eq!(evaluate(&Unique::parse("in [{Plains} {Hill}] tiles"), &ctx), Verdict::False);
    }

    #[test]
    fn featureless_means_bare_base_terrain() {
        let r = ruleset();
        let mut w = world(&r);
        let ctx = GameContext::new(&r).with_world(&w).on_tile(TileId(9));
        assert_eq!(evaluate(&Unique::parse("in [Featureless] tiles"), &ctx), Verdict::True);

        w.grid.features[9].push(r.terrain("Forest").unwrap());
        let ctx = GameContext::new(&r).with_world(&w).on_tile(TileId(9));
        assert_eq!(evaluate(&Unique::parse("in [Featureless] tiles"), &ctx), Verdict::False);
        assert_eq!(
            evaluate(&Unique::parse("in [{Featureless} {Grassland}] tiles"), &ctx),
            Verdict::False,
            "the conjunction fails on the first term"
        );
    }

    /// Open terrain is `all`, not `!any` — Unciv notes the distinction, because
    /// one open terrain among rough ones does not make a tile open.
    #[test]
    fn rough_and_open_are_not_simple_opposites() {
        let r = ruleset();
        let mut w = world(&r);
        let bare = GameContext::new(&r).with_world(&w).on_tile(TileId(9));
        assert_eq!(evaluate(&Unique::parse("in [Open terrain] tiles"), &bare), Verdict::True);
        assert_eq!(evaluate(&Unique::parse("in [Rough terrain] tiles"), &bare), Verdict::False);

        // Grassland is open, Forest is rough; the tile is rough and not open.
        w.grid.features[9].push(r.terrain("Forest").unwrap());
        let wooded = GameContext::new(&r).with_world(&w).on_tile(TileId(9));
        assert_eq!(evaluate(&Unique::parse("in [Rough terrain] tiles"), &wooded), Verdict::True);
        assert_eq!(
            evaluate(&Unique::parse("in [Open terrain] tiles"), &wooded),
            Verdict::False,
            "one open terrain among rough ones does not make it open"
        );
    }

    #[test]
    fn territory_is_read_relative_to_the_civ_asking() {
        let r = ruleset();
        let mut w = world(&r);
        w.grid.owner[9] = Some(CivId(0));
        let ctx = GameContext::new(&r).with_world(&w).on_tile(TileId(9)).for_civ(CivId(0));
        assert_eq!(evaluate(&Unique::parse("in [Friendly Land] tiles"), &ctx), Verdict::True);
        assert_eq!(evaluate(&Unique::parse("in [Foreign Land] tiles"), &ctx), Verdict::False);

        w.grid.owner[9] = Some(CivId(1));
        let ctx = GameContext::new(&r).with_world(&w).on_tile(TileId(9)).for_civ(CivId(0));
        assert_eq!(evaluate(&Unique::parse("in [Foreign Land] tiles"), &ctx), Verdict::True);
    }

    #[test]
    fn an_unknown_filter_stays_unknown_rather_than_becoming_false() {
        let r = ruleset();
        let w = world(&r);
        let ctx = GameContext::new(&r).with_world(&w).on_tile(TileId(0));
        // Not a terrain this ruleset has, so the honest answer is "cannot say".
        assert_eq!(evaluate(&Unique::parse("in [Unspecified custom terrain] tiles"), &ctx), Verdict::Unknown);
        assert_eq!(
            evaluate(&Unique::parse("in tiles without [Unspecified custom terrain]"), &ctx),
            Verdict::Unknown,
            "and negating an unknown is still unknown"
        );
    }

    #[test]
    fn tech_conditionals_read_the_civ() {
        let r = ruleset();
        let mut w = world(&r);
        let cond = Unique::parse("after discovering [Writing]");
        let ctx = GameContext::new(&r).with_world(&w).for_civ(CivId(0));
        // Borrow ends before the mutation below.
        let before = evaluate(&cond, &GameContext { world: Some(&w), ..ctx });
        assert_eq!(before, Verdict::False);

        let writing = r.technologies.id("Writing").unwrap();
        w.civ_mut(CivId(0)).unwrap().techs.insert(writing);
        let ctx = GameContext::new(&r).with_world(&w).for_civ(CivId(0));
        assert_eq!(evaluate(&cond, &ctx), Verdict::True);
        assert_eq!(evaluate(&Unique::parse("before discovering [Writing]"), &ctx), Verdict::False);
    }

    /// The rule this whole layer exists to make live.
    #[test]
    fn unhappiness_penalties_fire_at_their_thresholds() {
        let r = ruleset();
        let nullify = Unique::parse("Nullifies Growth [in all cities] <when below [-10] [Happiness]>");
        let ctx = GameContext::new(&r);

        assert_eq!(applies(&nullify, &ctx), Verdict::Unknown, "happiness unknown");
        assert_eq!(applies(&nullify, &ctx.with_happiness(5)), Verdict::False);
        assert_eq!(applies(&nullify, &ctx.with_happiness(-9)), Verdict::False);
        assert_eq!(applies(&nullify, &ctx.with_happiness(-11)), Verdict::True);

        // The milder penalty is a band, not a floor.
        let slow = Unique::parse("[-75]% growth [in all cities] <when between [-10] and [-1] [Happiness]>");
        assert_eq!(applies(&slow, &ctx.with_happiness(0)), Verdict::False);
        assert_eq!(applies(&slow, &ctx.with_happiness(-5)), Verdict::True);
        assert_eq!(applies(&slow, &ctx.with_happiness(-10)), Verdict::True);
        assert_eq!(applies(&slow, &ctx.with_happiness(-11)), Verdict::False);
    }

    #[test]
    fn a_unique_with_no_conditionals_always_applies() {
        let r = ruleset();
        let ctx = GameContext::new(&r);
        assert_eq!(applies(&Unique::parse("[+2 Food] from [Forest] tiles [in all cities]"), &ctx), Verdict::True);
    }

    #[test]
    fn one_unknown_conditional_blocks_the_whole_unique() {
        let r = ruleset();
        let ctx = GameContext::new(&r).with_happiness(-20);
        // First holds, second cannot be judged — so the unique does not apply.
        let u = Unique::parse("[+1 Food] <when below [-10] [Happiness]> <when at war>");
        assert_eq!(applies(&u, &ctx), Verdict::Unknown);
    }

    #[test]
    fn policy_and_belief_conditionals_share_one_form() {
        let r = ruleset();
        let mut w = world(&r);
        let before = Unique::parse("before adopting [Tradition]");
        let after = Unique::parse("after adopting [Tradition]");

        let ctx = GameContext::new(&r).with_world(&w).for_civ(CivId(0));
        assert_eq!(evaluate(&before, &ctx), Verdict::True);
        assert_eq!(evaluate(&after, &ctx), Verdict::False);

        let tradition = r.policies.id("Tradition").unwrap();
        w.civ_mut(CivId(0)).unwrap().adopted_policies.insert(tradition);
        let ctx = GameContext::new(&r).with_world(&w).for_civ(CivId(0));
        assert_eq!(evaluate(&after, &ctx), Verdict::True);

        // The same conditional answers for beliefs, because Civ treats a
        // belief and a policy as the same kind of thing: something taken.
        let belief = r.beliefs.ids().next().unwrap();
        let name = r.beliefs.name_of(belief).unwrap().to_string();
        let taken = Unique::parse(&format!("after adopting [{name}]"));
        assert_eq!(evaluate(&taken, &ctx), Verdict::False);
        w.civ_mut(CivId(0)).unwrap().faith.beliefs.insert(belief);
        let ctx = GameContext::new(&r).with_world(&w).for_civ(CivId(0));
        assert_eq!(evaluate(&taken, &ctx), Verdict::True);
    }

    #[test]
    fn building_conditionals_distinguish_here_from_everywhere() {
        use crate::world::city::City;
        let r = ruleset();
        let mut w = world(&r);
        let a = w.found_city(City::found("Rome", CivId(0), TileId(20)));
        let b = w.found_city(City::found("Ostia", CivId(0), TileId(40)));
        let monument = r.building("Monument").unwrap();

        let here = Unique::parse("if [Monument] is constructed");
        let everywhere = Unique::parse("if [Monument] is constructed in all [in all cities] cities");

        w.cities.get_mut(a).unwrap().built.insert(monument);
        let ctx = GameContext::new(&r).with_world(&w).for_civ(CivId(0)).in_city(a);
        assert_eq!(evaluate(&here, &ctx), Verdict::True);
        assert_eq!(evaluate(&everywhere, &ctx), Verdict::False, "Ostia has none");

        w.cities.get_mut(b).unwrap().built.insert(monument);
        let ctx = GameContext::new(&r).with_world(&w).for_civ(CivId(0)).in_city(a);
        assert_eq!(evaluate(&everywhere, &ctx), Verdict::True);
    }

    #[test]
    fn a_city_can_be_asked_its_size() {
        use crate::world::city::City;
        let r = ruleset();
        let mut w = world(&r);
        let city = w.found_city(City::found("Rome", CivId(0), TileId(20)));
        w.cities.get_mut(city).unwrap().population = 4;

        let u = Unique::parse("in cities with at least [3] [Population]");
        let ctx = GameContext::new(&r).with_world(&w).for_civ(CivId(0)).in_city(city);
        assert_eq!(evaluate(&u, &ctx), Verdict::True);

        let bigger = Unique::parse("in cities with at least [9] [Population]");
        assert_eq!(evaluate(&bigger, &ctx), Verdict::False);

        // Specialists are evaluated from actual assigned citizens.
        let specialists = Unique::parse("in cities with at least [1] [Specialists]");
        assert_eq!(evaluate(&specialists, &ctx), Verdict::False);
    }

    #[test]
    fn a_game_setting_is_not_something_the_world_can_answer() {
        let r = ruleset();
        let on = GameContext::new(&r);
        assert_eq!(evaluate(&Unique::parse("when religion is enabled"), &on), Verdict::True);

        let mut off = GameContext::new(&r);
        off.religion_enabled = false;
        assert_eq!(evaluate(&Unique::parse("when religion is enabled"), &off), Verdict::False);
    }

    /// Reported, not asserted at a fixed value — this number should climb.
    /// Measured against a context rich enough to try everything: a civ, a
    /// tile, a unit, an opponent, a combat posture and a happiness figure.
    /// A thinner context would under-report, and a richer one does not exist.
    #[test]
    fn report_conditional_coverage() {
        let r = ruleset();
        let mut w = world(&r);
        let base = r.unit("Warrior").unwrap();
        let mine = w.place_unit(Unit::new(base, CivId(0), TileId(0), 2));
        let theirs = w.place_unit(Unit::new(base, CivId(0), TileId(1), 2));

        let city = w.found_city(crate::world::city::City::found("Rome", CivId(0), TileId(20)));
        let ctx = GameContext::new(&r)
            .with_world(&w)
            .for_civ(CivId(0))
            .on_tile(TileId(0))
            .for_unit(mine)
            .in_city(city)
            .against(Opponent::Unit(theirs))
            .in_combat(CombatRole::Attacking)
            .with_happiness(0);
        let (decidable, total) = conditional_coverage(&ctx);
        println!("conditionals decidable: {decidable}/{total} ({:.1}%)", decidable as f64 / total as f64 * 100.0);
        println!("still undecidable, by frequency:");
        for (name, count) in undecidable_conditionals(&ctx).iter().take(12) {
            println!("  {count:4}  <{name}>");
        }
        assert!(total > 300, "expected a substantial conditional surface, got {total}");
        assert!(decidable > 0, "nothing is decidable, which cannot be right");
    }
}
