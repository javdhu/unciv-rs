//! What a city builds.
//!
//! The production half of the loop, and the first system gated by the tech
//! tree — a city cannot build a Library before Writing, which is the whole
//! reason research matters.
//!
//! Costs are not in the json for most buildings. Only 61 of 224 state one; the
//! rest inherit their tech column's `buildingCost`, or `wonderCost` if they are
//! a wonder. `Ruleset::derive` fills that in, following Unciv
//! (`Ruleset.kt:618`), whose comment explains the intent: editing a column's
//! costs should move every building in it, so a mod adding a building inherits
//! the original values rather than restating them.

use crate::arena::Id;
use crate::ruleset::{BaseUnit, Building, Ruleset};
use crate::stats::{Fixed, Precise};
use crate::world::city::City;
use crate::world::conditionals::{applies, GameContext, Verdict};
use crate::world::entities::Civilization;
use crate::world::world::World;
use crate::UniqueType;

#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Construction {
    Building(Id<Building>),
    Unit(Id<BaseUnit>),
}

/// Why a city cannot build something.
///
/// Named rather than boolean, because "you cannot build this" is a question
/// the interface has to answer in words, and because the AI picks differently
/// depending on whether a thing is impossible or merely not yet unlocked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Rejection {
    /// The unlocking tech has not been researched.
    NotUnlocked(String),
    /// This city already has it.
    AlreadyBuilt,
    /// A world wonder exists somewhere, or this civ already has its national wonder.
    WonderAlreadyBuilt,
    /// Available strategic resources cannot support the additional unit.
    MissingResources,
    /// The owner has researched the technology that replaces this unit.
    Obsolete,
    /// A prerequisite building is missing from this city.
    MissingBuilding(String),
    /// Restricted to another civilization.
    UniqueToAnother(String),
    /// Carries no cost and cannot be produced — the Palace, or a
    /// faith-purchased religious building.
    NotProducible,
    /// The city is below the construction's required population.
    NeedsPopulation(u32),
    /// A puppet's governor does not build wonders.
    PuppetWonder,
    /// City-states build no wonders (Building.kt:385, `CityStateWonder`).
    CityStateWonder,
    /// A ship needs a city on the coast.
    NotCoastal,
    /// The id does not resolve in this ruleset.
    Unknown,
    NotAvailable,
    LimitReached,
    TerrainRequirement,
}

impl std::fmt::Display for Rejection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Rejection::NotUnlocked(tech) => write!(f, "needs {tech}"),
            Rejection::AlreadyBuilt => write!(f, "already built in this city"),
            Rejection::WonderAlreadyBuilt => write!(f, "this wonder has already been built"),
            Rejection::MissingResources => write!(f, "not enough strategic resources"),
            Rejection::Obsolete => write!(f, "replaced by a newer construction"),
            Rejection::MissingBuilding(b) => write!(f, "needs a {b} in this city"),
            Rejection::UniqueToAnother(civ) => write!(f, "unique to {civ}"),
            Rejection::NotProducible => write!(f, "cannot be produced"),
            Rejection::NeedsPopulation(n) => write!(f, "needs a city of size {n}"),
            Rejection::PuppetWonder => write!(f, "a puppet does not build wonders"),
            Rejection::CityStateWonder => write!(f, "city-states do not build wonders"),
            Rejection::NotCoastal => write!(f, "a ship needs a coastal city"),
            Rejection::Unknown => write!(f, "not in this ruleset"),
            Rejection::NotAvailable => write!(f, "not available in this city"),
            Rejection::LimitReached => write!(f, "this nation has reached its limit for this"),
            Rejection::TerrainRequirement => write!(f, "requires different surrounding terrain"),
        }
    }
}

impl std::error::Error for Rejection {}

/// Scaling applied on top of a base cost.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CostModifiers {
    pub speed: Fixed,
    /// Difficulty, and ×1.5 for city-states.
    pub civ: Fixed,
    /// How many cities the owner has. National wonders carry
    /// `Cost increases by [n] per owned city`, so a wide empire pays more for
    /// them — the mechanism Civ uses to keep them from being free wins for
    /// whoever expanded most. Used by standalone algebra only; live-world
    /// production and purchase functions derive the current count themselves.
    pub cities: usize,
    /// How many turns a game at this speed runs for. Unit upkeep needs it:
    /// Civ makes soldiers dearer as a fraction of the whole game elapsed.
    pub game_length: i32,
    /// How much influence a gift of gold to a city-state buys, scaled by
    /// speed — a quick game gives more per coin, so diplomacy still resolves
    /// inside it.
    pub gold_gift_modifier: Fixed,
    /// Speed's `goldCostModifier`, for anything bought with gold.
    pub gold_cost: Fixed,
    /// Speed's `faithCostModifier`, for anything bought with faith.
    pub faith_cost: Fixed,
    /// Speed's `cityStateTributeScalingInterval`: turns between each five
    /// gold more that a tribute yields.
    pub tribute_interval: Fixed,
    /// Speed's `dealDuration`: how many turns a trade runs.
    pub deal_duration: u32,
    /// Speed's `goldenAgeLengthModifier`.
    pub golden_age_length: Fixed,
    /// Base religious pressure one city puts on its neighbours each turn.
    /// Nine on Quick, six on Standard, two on Marathon — a shorter game
    /// converts faster so religion still resolves inside it.
    pub religious_pressure: i32,
}

// `game_length` and `religious_pressure` are not costs. They are here because
// they come from the same place as `speed` — the game speed object — and the
// turn loop needs all three together. The alternative was a ninth and tenth
// argument on every turn function, which buys tidiness in the type name and
// pays for it at every call site.

impl Default for CostModifiers {
    fn default() -> Self {
        CostModifiers {
            speed: Fixed::ONE,
            civ: Fixed::ONE,
            cities: 1,
            // Standard speed, so a default-constructed game ages and converts
            // at the rate most of the ruleset was balanced for.
            game_length: 500,
            religious_pressure: 6,
            gold_gift_modifier: Fixed::ONE,
            gold_cost: Fixed::ONE,
            faith_cost: Fixed::ONE,
            tribute_interval: Fixed::permille(6_500),
            deal_duration: 30,
            golden_age_length: Fixed::ONE,
        }
    }
}

/// Production needed to finish something.
///
/// Truncated, not rounded — Unciv's `getProductionCost` ends in `.toInt()`,
/// and Kotlin truncates toward zero.
/// Without world context, conditional cost uniques remain unapplied. Use
/// [`production_cost_in`] for a city's displayed or executable price.
pub fn production_cost(r: &Ruleset, what: Construction, m: CostModifiers) -> Option<i32> {
    production_cost_with(r, what, m, &GameContext::new(r))
}

/// Price in a live city. Empire size and conditional discounts are derived
/// from its current owner and state, never from a caller's cached city count.
pub fn production_cost_in(
    world: &World,
    r: &Ruleset,
    city: Id<City>,
    what: Construction,
    mut m: CostModifiers,
) -> Option<i32> {
    let c = world.cities.get(city)?;
    world.civ(c.owner)?;
    m.cities = world.cities.iter().filter(|(_, other)| other.owner == c.owner).count();
    let ctx = GameContext::new(r).with_world(world).for_civ(c.owner).in_city(city).on_tile(c.centre);
    production_cost_with(r, what, m, &ctx)
}

fn production_cost_with(r: &Ruleset, what: Construction, m: CostModifiers, ctx: &GameContext) -> Option<i32> {
    let (base, uniques) = match what {
        Construction::Building(id) => {
            let b = r.buildings.get(id)?;
            (b.cost?, &b.uniques)
        }
        Construction::Unit(id) => {
            let u = r.units.get(id)?;
            (u.cost?, &u.uniques)
        }
    };
    if base <= 0 {
        return None;
    }

    // Flat additions land before any scaling, as they do in Unciv's
    // `getProductionCost`.
    let per_city = uniques
        .iter()
        .filter(|u| u.unique_type() == Some(UniqueType::CostIncreasesPerCity) && applies(u, ctx) == Verdict::True)
        .filter_map(|u| u.params.first())
        .filter_map(|p| p.parse::<i64>().ok())
        .try_fold(0i64, i64::checked_add)?;
    let base = i64::from(base).checked_add(per_city.checked_mul(i64::try_from(m.cities).ok()?)?)?;
    let mut cost = Precise::from_ratio(base, 1);
    for unique in uniques
        .iter()
        .filter(|u| u.unique_type() == Some(UniqueType::CostPercentageChange) && applies(u, ctx) == Verdict::True)
    {
        if let Some(percent) = unique.params.first().and_then(|p| Fixed::parse(p)) {
            cost = cost * Precise::from_ratio(100_000i64.checked_add(percent.raw())?, 100_000);
        }
    }

    let cost = (cost * Precise::from_fixed(m.speed) * Precise::from_fixed(m.civ)).floor();
    Some(cost.clamp(1, i32::MAX as i64) as i32)
}

/// Whether this city may start on something now.
pub fn can_construct(r: &Ruleset, civ: &Civilization, city: &City, what: Construction) -> Result<(), Rejection> {
    can_construct_impl(r, civ, city, what, false, false)
}

fn is_obsolete(r: &Ruleset, civ: &Civilization, what: Construction, ctx: &GameContext) -> bool {
    match what {
        Construction::Unit(id) => r
            .units
            .get(id)
            .and_then(|u| u.obsolete_tech.as_deref())
            .and_then(|name| r.technologies.id(name))
            .is_some_and(|tech| civ.has_tech(tech)),
        Construction::Building(id) => r.buildings.get(id).is_some_and(|building| {
            building.uniques.iter().any(|unique| {
                unique.unique_type() == Some(UniqueType::ObsoleteWith)
                    && applies(unique, ctx) == Verdict::True
                    && unique
                        .params
                        .first()
                        .and_then(|name| r.technologies.id(name))
                        .is_some_and(|tech| civ.has_tech(tech))
            })
        }),
    }
}

fn can_construct_impl(
    r: &Ruleset,
    civ: &Civilization,
    city: &City,
    what: Construction,
    ignore_obsolete: bool,
    purchase: bool,
) -> Result<(), Rejection> {
    let (required_tech, unique_to) = match what {
        Construction::Building(id) => {
            let b = r.buildings.get(id).ok_or(Rejection::Unknown)?;
            if city.built.contains(&id) {
                return Err(Rejection::AlreadyBuilt);
            }
            if !purchase && b.cost.unwrap_or(0) <= 0 {
                return Err(Rejection::NotProducible);
            }
            if city.puppet && (b.is_wonder == Some(true) || b.is_national_wonder == Some(true)) {
                return Err(Rejection::PuppetWonder);
            }
            if civ.city_state_type.is_some() && (b.is_wonder == Some(true) || b.is_national_wonder == Some(true)) {
                return Err(Rejection::CityStateWonder);
            }
            if let Some(req) = b.required_building.as_deref() {
                let have = r.buildings.id(req).is_some_and(|rid| city.built.contains(&rid));
                if !have {
                    return Err(Rejection::MissingBuilding(req.to_string()));
                }
            }
            (b.required_tech.clone(), b.unique_to.clone())
        }
        Construction::Unit(id) => {
            let u = r.units.get(id).ok_or(Rejection::Unknown)?;
            if !purchase && u.cost.unwrap_or(0) <= 0 {
                return Err(Rejection::NotProducible);
            }
            // `WaterUnitsInCoastalCities`: a ship built inland has no sea.
            if r.unit_types.by_name(&u.unit_type).is_some_and(|t| t.movement_type == "Water") && !city.coastal {
                return Err(Rejection::NotCoastal);
            }
            for unique in u.uniques.iter().filter(|x| x.unique_type() == Some(UniqueType::RequiresPopulation)) {
                let needed = unique.params.first().and_then(|p| p.parse::<u32>().ok()).unwrap_or(0);
                if city.population < needed {
                    return Err(Rejection::NeedsPopulation(needed));
                }
            }
            (u.required_tech.clone(), u.unique_to.clone())
        }
    };

    if let Some(tech) = required_tech {
        let known = r.technologies.id(&tech).is_some_and(|t| civ.has_tech(t));
        if !known {
            return Err(Rejection::NotUnlocked(tech));
        }
    }
    if let Some(nation) = unique_to {
        let theirs = r.nations.name_of(civ.nation) == Some(nation.as_str());
        if !theirs {
            return Err(Rejection::UniqueToAnother(nation));
        }
    }
    let nation = r.nations.name_of(civ.nation);
    let replaced = match what {
        Construction::Unit(id) => r
            .units
            .iter()
            .any(|(_, u)| u.unique_to.as_deref() == nation && u.replaces.as_deref() == Some(r.units[id].name.as_str())),
        Construction::Building(id) => r.buildings.iter().any(|(_, b)| {
            b.unique_to.as_deref() == nation && b.replaces.as_deref() == Some(r.buildings[id].name.as_str())
        }),
    };
    if replaced {
        return Err(Rejection::NotAvailable);
    }
    if !ignore_obsolete && is_obsolete(r, civ, what, &GameContext::new(r).knowing(&civ.techs)) {
        return Err(Rejection::Obsolete);
    }
    Ok(())
}

/// Construction legality that depends on other cities or units. Keep the
/// city-only helper for planning fragments, but use this at execution time.
pub fn can_construct_in(
    world: &crate::world::world::World,
    r: &Ruleset,
    civ: crate::world::grid::CivId,
    city: &City,
    what: Construction,
) -> Result<(), Rejection> {
    can_construct_mode(world, r, civ, city, what, false)
}

pub fn can_purchase_in(
    world: &World,
    r: &Ruleset,
    civ: crate::world::grid::CivId,
    city: &City,
    what: Construction,
) -> Result<(), Rejection> {
    can_construct_mode(world, r, civ, city, what, true)
}

fn can_construct_mode(
    world: &World,
    r: &Ruleset,
    civ: crate::world::grid::CivId,
    city: &City,
    what: Construction,
    purchase: bool,
) -> Result<(), Rejection> {
    let owner = world.civ(civ).ok_or(Rejection::Unknown)?;
    can_construct_impl(r, owner, city, what, false, purchase)?;
    let id = world.cities.iter().find(|(_, c)| c.centre == city.centre).map(|(id, _)| id);
    let mut ctx = GameContext::new(r).with_world(world).for_civ(civ).on_tile(city.centre);
    if let Some(id) = id {
        ctx = ctx.in_city(id);
    }
    if let Construction::Unit(base) = what {
        let unit = crate::world::Unit::new(base, civ, city.centre, 0);
        let h = crate::world::happiness::happiness(world, r, civ, None);
        let context = ctx.with_happiness(h).for_unit_value(&unit);
        if r.global_uniques.iter().chain(owner.uniques.all()).any(|u| {
            u.unique_type() == Some(UniqueType::CannotBuildUnits)
                && applies(u, &context).holds()
                && u.params.first().is_some_and(|f| crate::world::filters::unit_matches(r, &unit, f).holds())
        }) {
            return Err(Rejection::NotAvailable);
        }
    }
    let uniques = match what {
        Construction::Building(id) => &r.buildings[id].uniques,
        Construction::Unit(id) => &r.units[id].uniques,
    };
    for unique in uniques {
        let active = applies(unique, &ctx);
        match unique.unique_type() {
            Some(UniqueType::OnlyAvailable | UniqueType::CanOnlyBeBuiltWhen) if active != Verdict::True => {
                return Err(Rejection::NotAvailable)
            }
            Some(UniqueType::Unavailable) if active == Verdict::True => return Err(Rejection::NotAvailable),
            Some(UniqueType::Unbuildable) if !purchase && active == Verdict::True => {
                return Err(Rejection::NotProducible)
            }
            Some(UniqueType::MaxNumberBuildable) if active == Verdict::True => {
                let limit = unique.params.first().and_then(|p| p.parse::<u32>().ok()).unwrap_or(0);
                let mut count = world
                    .cities
                    .iter()
                    .filter(|(_, c)| {
                        c.owner == civ
                            && c.centre != city.centre
                            && (c.queue.contains(&what)
                                || matches!(what,Construction::Building(b) if c.built.contains(&b)))
                    })
                    .count() as u32;
                if let Construction::Unit(base) = what {
                    count = count
                        .saturating_add(
                            world.units.iter().filter(|(_, u)| u.owner == civ && u.base == base).count() as u32
                        )
                        .saturating_add(crate::world::victory_actions::assembled_parts(world, civ, base));
                }
                if count >= limit {
                    return Err(Rejection::LimitReached);
                }
            }
            Some(UniqueType::MustHaveOwnedWithinTiles) if active == Verdict::True => {
                let radius = unique.params.get(1).and_then(|p| p.parse::<u32>().ok()).unwrap_or(0);
                if !world.grid.within(city.centre, radius).into_iter().any(|tile| {
                    world.grid.owner[tile.0 as usize] == Some(civ)
                        && unique.params.first().is_some_and(|filter| {
                            crate::world::conditionals::tile_matches_in(&world.grid, r, tile, filter, Some(civ))
                                == Verdict::True
                        })
                }) {
                    return Err(Rejection::TerrainRequirement);
                }
            }
            Some(
                UniqueType::MustBeOn | UniqueType::MustNotBeOn | UniqueType::MustBeNextTo | UniqueType::MustNotBeNextTo,
            ) if active == Verdict::True => {
                let Some(filter) = unique.params.first() else { return Err(Rejection::TerrainRequirement) };
                let matches = |tile| {
                    crate::world::conditionals::tile_matches_in(&world.grid, r, tile, filter, Some(civ))
                        == Verdict::True
                };
                let on = matches(city.centre);
                let adjacent = world.grid.neighbours(city.centre).any(matches)
                    || (matches!(filter.as_str(), "Fresh water" | "Fresh Water")
                        && world.grid.is_fresh_water(r, city.centre));
                let holds = match unique.unique_type().unwrap() {
                    UniqueType::MustBeOn => on,
                    UniqueType::MustNotBeOn => !on,
                    UniqueType::MustBeNextTo => adjacent,
                    _ => !adjacent,
                };
                if !holds {
                    return Err(Rejection::TerrainRequirement);
                }
            }
            _ => {}
        }
    }
    // A building that stands on a tile needs a tile to stand on.
    if let (Construction::Building(base), Some(city_id)) = (what, id) {
        if crate::world::placement::is_placed(r, base)
            && !city.built.contains(&base)
            && crate::world::placement::valid_sites(world, r, city_id, base).is_empty()
        {
            return Err(Rejection::TerrainRequirement);
        }
    }
    if let Construction::Building(base) = what {
        let building = &r.buildings[base];
        let required = crate::world::resources::building_requirements(world, r, city, base);
        if !required.is_empty() {
            let stock = crate::world::resources::available_stock(world, r, civ);
            if required.into_iter().any(|(res, n)| stock.get(&res).copied().unwrap_or(0) < n) {
                return Err(Rejection::MissingResources);
            }
        }
        if !building.required_nearby_improved_resources.is_empty()
            && !city.tiles.iter().any(|&tile| {
                world.grid.resource[tile.0 as usize]
                    .and_then(|resource| r.resources.name_of(resource))
                    .is_some_and(|name| building.required_nearby_improved_resources.iter().any(|r| r == name))
                    && crate::world::resources::provides_resource(world, r, owner, tile)
            })
        {
            return Err(Rejection::MissingResources);
        }
    }

    match what {
        Construction::Building(id) => {
            let building = r.buildings.get(id).ok_or(Rejection::Unknown)?;
            if building.uniques.iter().any(|u| u.unique_type() == Some(UniqueType::ObsoleteWith)) {
                let ctx = GameContext::new(r).with_world(world).for_civ(civ).on_tile(city.centre);
                let ctx = if let Some((id, _)) = world.cities.iter().find(|(_, c)| c.centre == city.centre) {
                    ctx.in_city(id)
                } else {
                    ctx
                };
                if is_obsolete(r, owner, what, &ctx) {
                    return Err(Rejection::Obsolete);
                }
            }
            if crate::world::slices::is_gone(world, r, id) {
                return Err(Rejection::Obsolete);
            }
            if world.cities.iter().any(|(_, other)| {
                other.built.contains(&id)
                    && (building.is_wonder == Some(true)
                        || (building.is_national_wonder == Some(true) && other.owner == civ))
            }) {
                return Err(Rejection::WonderAlreadyBuilt);
            }
        }
        Construction::Unit(base) => {
            if !crate::world::resources::can_supply_unit(world, r, civ, base, None) {
                return Err(Rejection::MissingResources);
            }
        }
    }
    Ok(())
}

/// Everything this city could start on right now.
pub fn available(r: &Ruleset, civ: &Civilization, city: &City) -> Vec<Construction> {
    let mut out: Vec<Construction> = r
        .buildings
        .ids()
        .map(Construction::Building)
        .chain(r.units.ids().map(Construction::Unit))
        .filter(|c| can_construct(r, civ, city, *c).is_ok())
        .collect();
    out.sort();
    out
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConstructionEvent {
    /// Unavailable or unable to place the finished unit. Investment is retained.
    Blocked {
        what: Construction,
    },
    /// Permanently unavailable investment was discarded, with a wonder refund.
    Cancelled {
        what: Construction,
        gold: i32,
    },
    /// Obsolete unit investment follows its currently buildable upgrade.
    Transferred {
        from: Construction,
        to: Construction,
        production: i32,
    },
    Progressed {
        stored: i32,
    },
    Completed {
        what: Construction,
        carried_over: i32,
    },
    /// Ordinary production is not stored while idle; existing overflow remains.
    Idle {
        stored: i32,
    },
}

/// An explicit production grant (for example an Engineer), distinct from a
/// turn's income. Idle cities retain grants as overflow, as in Unciv.
pub fn add_production_points(city: &mut City, amount: i32) {
    if amount <= 0 {
        return;
    }
    if let Some(&what) = city.queue.first() {
        let stored = city.production_progress.entry(what).or_default();
        *stored = stored.saturating_add(amount);
    } else {
        city.production_overflow = city.production_overflow.saturating_add(amount);
    }
}

pub fn converts_food(r: &Ruleset, what: Construction) -> bool {
    let uniques = match what {
        Construction::Unit(id) => &r.units[id].uniques,
        Construction::Building(id) => &r.buildings[id].uniques,
    };
    uniques.iter().any(|u| u.unique_type() == Some(UniqueType::ConvertFoodToProductionWhenConstructed))
}

pub fn production_from_food(food: i32) -> i32 {
    if food >= 4 {
        2 + food / 4
    } else if food >= 2 {
        2
    } else if food >= 1 {
        1
    } else {
        0
    }
}

fn upgrade_construction(world: &World, r: &Ruleset, city: &City, base: Id<BaseUnit>) -> Option<Construction> {
    let nation = world.civ(city.owner).and_then(|c| r.nations.name_of(c.nation))?;
    let mut seen = std::collections::BTreeSet::new();
    let mut next = r.units.get(base)?.upgrades_to.as_deref();
    let mut candidates = Vec::new();
    while let Some(id) = next.and_then(|name| r.units.id(name)) {
        if !seen.insert(id) {
            break;
        }
        let unit = r.units.get(id)?;
        let equivalent = r
            .units
            .iter()
            .find(|(_, u)| u.replaces.as_deref() == Some(unit.name.as_str()) && u.unique_to.as_deref() == Some(nation))
            .map_or(id, |(id, _)| id);
        let what = Construction::Unit(equivalent);
        if can_construct_in(world, r, city.owner, city, what).is_ok() {
            candidates.push((r.units.get(equivalent)?.cost.unwrap_or(i32::MAX), what));
        }
        next = unit.upgrades_to.as_deref();
    }
    candidates.into_iter().min().map(|(_, what)| what)
}

/// Recheck paused work as well as the queue. Temporary restrictions remove an
/// item from the queue without destroying its work; there is no timed decay.
/// Permanent invalidation refunds world wonders once and transfers obsolete
/// unit investment where an available upgrade exists.
pub fn revalidate_production(world: &mut World, r: &Ruleset, city: Id<City>) -> Vec<ConstructionEvent> {
    let Some(c) = world.cities.get(city) else { return Vec::new() };
    let items: std::collections::BTreeSet<_> = c.queue.iter().chain(c.production_progress.keys()).copied().collect();
    let mut events = Vec::new();
    for what in items {
        let Some(c) = world.cities.get(city) else { break };
        let Some(civ) = world.civ(c.owner) else { continue };
        // A temporary reason (lost prerequisite after conquest, for example)
        // must not hide permanent wonder loss or obsolescence.
        let ctx = GameContext::new(r).with_world(world).for_civ(c.owner).in_city(city).on_tile(c.centre);
        let wonder_lost = matches!(what, Construction::Building(id) if r.buildings.get(id).is_some_and(|b|
            world.cities.iter().any(|(_, other)| other.built.contains(&id)
                && (b.is_wonder == Some(true) || (b.is_national_wonder == Some(true) && other.owner == c.owner)))));
        let reason = if wonder_lost {
            Rejection::WonderAlreadyBuilt
        } else if is_obsolete(r, civ, what, &ctx) {
            Rejection::Obsolete
        } else if let Err(reason) = can_construct_in(world, r, c.owner, c, what) {
            reason
        } else {
            continue;
        };
        let owner = c.owner;
        let production = c.production_for(what);
        let queued = c.queue.contains(&what);
        let permanent = matches!(
            reason,
            Rejection::AlreadyBuilt | Rejection::WonderAlreadyBuilt | Rejection::Obsolete | Rejection::Unknown
        );
        if !permanent {
            if queued {
                world.cities.get_mut(city).unwrap().queue.retain(|item| *item != what);
                events.push(ConstructionEvent::Blocked { what });
            }
            continue;
        }
        let upgrade = match (reason, what) {
            (Rejection::Obsolete, Construction::Unit(base))
                if can_construct_impl(r, civ, c, what, true, false).is_ok()
                    && crate::world::resources::can_supply_unit(world, r, c.owner, base, None) =>
            {
                upgrade_construction(world, r, c, base)
            }
            _ => None,
        };
        let gold = if upgrade.is_none()
            && matches!(what, Construction::Building(id) if r.buildings.get(id).is_some_and(|b| b.is_wonder == Some(true)))
        {
            production.max(0)
        } else {
            0
        };
        let c = world.cities.get_mut(city).unwrap();
        c.production_progress.remove(&what);
        if let Some(to) = upgrade {
            if production > 0 {
                let stored = c.production_progress.entry(to).or_default();
                *stored = stored.saturating_add(production);
            }
            for item in &mut c.queue {
                if *item == what {
                    *item = to;
                }
            }
            events.push(ConstructionEvent::Transferred { from: what, to, production });
        } else {
            c.queue.retain(|item| *item != what);
            if gold > 0 {
                if let Some(civ) = world.civ_mut(owner) {
                    civ.gold = civ.gold.saturating_add(gold);
                }
            }
            events.push(ConstructionEvent::Cancelled { what, gold });
        }
    }
    events
}

/// Compatibility wrapper for the shared air basing rules.
pub fn air_capacity_in(world: &World, r: &Ruleset, city: Id<City>) -> Option<usize> {
    crate::world::air::city_capacity(world, r, city)
}

/// Deterministic nearest legal placement, searching connected tiles up to ten
/// steps as Unciv's placeUnitNearTile does. Friendly units may be traversed;
/// they must not share the newborn's destination slot.
fn production_unit(
    world: &World,
    r: &Ruleset,
    city: Id<City>,
    base: Id<BaseUnit>,
) -> Option<crate::world::entities::Unit> {
    let c = world.cities.get(city)?;
    let mut unit = prepare_unit_near(world, r, c.owner, base, c.centre, Some(city))?;
    crate::world::military::train(world, r, city, &mut unit);
    unit.movement_left = Fixed::whole(crate::world::movement::max_movement(world, r, &unit));
    Some(unit)
}

/// Prepare a new unit in a legal nearby slot, without mutating the world.
/// Initial placement, rewards, purchases and production share this operation.
pub fn prepare_unit_near(
    world: &World,
    r: &Ruleset,
    owner: crate::world::grid::CivId,
    base: Id<BaseUnit>,
    origin: crate::world::grid::TileId,
    city: Option<Id<City>>,
) -> Option<crate::world::entities::Unit> {
    use crate::world::movement;
    if !world.grid.contains(origin) {
        return None;
    }
    let mut unit = crate::world::entities::Unit::new(base, owner, origin, r.units.get(base)?.movement);
    crate::world::military::initialize_promotions(world, r, &mut unit);
    crate::world::religious_units::initialize_unit(world, r, &mut unit, city);
    let can_stop = |tile| {
        movement::can_traverse(world, r, &unit, tile, tile)
            && movement::can_pass_through(world, r, tile, &unit)
            && (!unit.is_land_unit(r) || !world.grid.is_water(r, tile) || movement::can_embark(world, r, &unit))
            && (!unit.is_water_unit(r)
                || world.grid.is_water(r, tile)
                || world.grid.neighbours(tile).any(|next| world.grid.is_water(r, next)))
            && !world.units.iter().any(|(_, other)| {
                other.position == tile
                    && (other.owner != unit.owner
                        || (!unit.is_air(r) && !other.is_air(r) && unit.is_civilian(r) == other.is_civilian(r)))
            })
            && !world.cities.iter().any(|(_, other)| other.centre == tile && other.owner != unit.owner)
    };
    if unit.is_air(r) {
        let mut candidates: Vec<_> = world
            .cities
            .iter()
            .filter(|(_, c)| c.owner == owner)
            .map(|(_, c)| c.centre)
            .chain(world.units.iter().filter(|(_, u)| u.owner == owner).map(|(_, u)| u.position))
            .collect();
        candidates.sort_by_key(|tile| (world.grid.distance(origin, *tile), *tile));
        candidates.dedup();
        for tile in candidates {
            if world.grid.distance(origin, tile) > 10 {
                break;
            }
            if let Some(base) = crate::world::air::base_for(world, r, &unit, tile, None) {
                unit.position = tile;
                unit.transport = base.transport();
                unit.movement_left = Fixed::whole(movement::max_movement(world, r, &unit));
                return Some(unit);
            }
        }
        return None;
    }
    let mut visited = std::collections::BTreeSet::new();
    let mut frontier = vec![origin];
    for _ in 0..=10 {
        frontier.sort_by_key(|&tile| (unit.is_land_unit(r) && world.grid.is_water(r, tile), tile));
        if let Some(&tile) = frontier.iter().find(|&&tile| can_stop(tile)) {
            unit.position = tile;
            unit.embarked = unit.is_land_unit(r) && world.grid.is_water(r, tile);
            unit.promotions.extend(crate::world::military::adjacent_promotions(world, r, &unit));
            unit.movement_left = Fixed::whole(movement::max_movement(world, r, &unit));
            return Some(unit);
        }
        visited.extend(frontier.iter().copied());
        let mut next = std::collections::BTreeSet::new();
        for from in frontier {
            for to in world.grid.neighbours(from) {
                if !visited.contains(&to)
                    && movement::can_traverse(world, r, &unit, from, to)
                    && movement::can_pass_through(world, r, to, &unit)
                {
                    next.insert(to);
                }
            }
        }
        frontier = next.into_iter().collect();
        if frontier.is_empty() {
            break;
        }
    }
    None
}

/// Settle one round's production using live prices and legal unit placement.
/// Maintenance can emit several refunds/transfers; at most one item completes.
pub fn apply_production_in(
    world: &mut World,
    r: &Ruleset,
    city: Id<City>,
    production: i32,
    m: CostModifiers,
) -> Option<Vec<ConstructionEvent>> {
    let science = crate::world::research::ScienceModifiers {
        speed: world.speed_rules(r, m.speed).map_or(m.speed, |s| s.science_cost_modifier),
        ..Default::default()
    };
    apply_production_with_science(world, r, city, production, m, science)
}
pub fn apply_production_with_science(
    world: &mut World,
    r: &Ruleset,
    city: Id<City>,
    production: i32,
    m: CostModifiers,
    science: crate::world::research::ScienceModifiers,
) -> Option<Vec<ConstructionEvent>> {
    world.cities.get(city)?;
    let mut events = revalidate_production(world, r, city);
    let c = world.cities.get(city)?;
    let Some(&head) = c.queue.first() else {
        events.push(ConstructionEvent::Idle { stored: c.production_overflow });
        return Some(events);
    };
    let Some(cost) = production_cost_in(world, r, city, head, m) else {
        events.push(ConstructionEvent::Blocked { what: head });
        return Some(events);
    };
    let c = world.cities.get_mut(city)?;
    let income = production.max(0).saturating_add(std::mem::take(&mut c.production_overflow));
    add_production_points(c, income);
    if c.current_production() < cost {
        events.push(ConstructionEvent::Progressed { stored: c.current_production() });
        return Some(events);
    }
    let born = if let Construction::Unit(base) = head {
        let Some(unit) = production_unit(world, r, city, base) else {
            events.push(ConstructionEvent::Blocked { what: head });
            return Some(events);
        };
        Some(unit)
    } else {
        None
    };
    let c = world.cities.get_mut(city)?;
    let excess = c.current_production().saturating_sub(cost);
    c.production_overflow = excess.min(cost.max(production.max(0)));
    c.production_progress.remove(&head);
    c.queue.remove(0);
    if let Construction::Building(id) = head {
        c.built.insert(id);
        crate::world::city::rebuild_city_uniques(r, c);
    }
    events.push(ConstructionEvent::Completed { what: head, carried_over: c.production_overflow });
    if let Some(unit) = born {
        let id = world.place_unit(unit);
        crate::world::luminaries::name_unit(world, r, id);
    }
    crate::world::effects::on_construction_completed_with_science(world, r, city, head, m.speed, science);
    events.extend(revalidate_production(world, r, city));
    Some(events)
}

/// Standalone city algebra for callers without a world. Live games must use
/// apply_production_in so legality, refunds and unit placement are authoritative.
pub fn apply_production(city: &mut City, r: &Ruleset, production: i32, m: CostModifiers) -> ConstructionEvent {
    let Some(&head) = city.queue.first() else {
        return ConstructionEvent::Idle { stored: city.production_overflow };
    };
    let Some(cost) = production_cost(r, head, m) else {
        return ConstructionEvent::Blocked { what: head };
    };
    let income = production.max(0).saturating_add(std::mem::take(&mut city.production_overflow));
    add_production_points(city, income);
    if city.current_production() < cost {
        return ConstructionEvent::Progressed { stored: city.current_production() };
    }
    city.production_overflow = city.current_production().saturating_sub(cost).min(cost.max(production.max(0)));
    city.production_progress.remove(&head);
    city.queue.remove(0);
    if let Construction::Building(id) = head {
        city.built.insert(id);
        crate::world::city::rebuild_city_uniques(r, city);
    }
    ConstructionEvent::Completed { what: head, carried_over: city.production_overflow }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::grid::{CivId, TileId};

    fn ruleset() -> Ruleset {
        Ruleset::load_dir("rules/gods-and-kings").expect("readable").expect("loads")
    }

    fn civ(r: &Ruleset) -> Civilization {
        Civilization::new(CivId(0), r.nations.ids().next().unwrap(), false)
    }

    #[test]
    fn a_building_is_gated_on_its_tech() {
        let r = ruleset();
        let mut c = civ(&r);
        let city = City::found("Rome", CivId(0), TileId(0));
        let library = Construction::Building(r.building("Library").unwrap());

        assert_eq!(can_construct(&r, &c, &city, library), Err(Rejection::NotUnlocked("Writing".into())));

        c.techs.insert(r.technologies.id("Writing").unwrap());
        assert_eq!(can_construct(&r, &c, &city, library), Ok(()));
    }

    #[test]
    fn what_cannot_be_produced_is_rejected_as_such() {
        let r = ruleset();
        let c = civ(&r);
        let city = City::found("Rome", CivId(0), TileId(0));
        // The Palace is free in the capital and carries no cost.
        let palace = Construction::Building(r.building("Palace").unwrap());
        assert_eq!(can_construct(&r, &c, &city, palace), Err(Rejection::NotProducible));
    }

    #[test]
    fn a_building_already_here_is_not_offered_again() {
        let r = ruleset();
        let mut c = civ(&r);
        c.techs.insert(r.technologies.id("Writing").unwrap());
        let mut city = City::found("Rome", CivId(0), TileId(0));
        let id = r.building("Library").unwrap();
        city.built.insert(id);
        assert_eq!(can_construct(&r, &c, &city, Construction::Building(id)), Err(Rejection::AlreadyBuilt));
    }

    #[test]
    fn unique_units_belong_to_their_nation() {
        let r = ruleset();
        let c = civ(&r);
        let city = City::found("Rome", CivId(0), TileId(0));
        let owner = r.nations.name_of(c.nation).unwrap().to_string();

        let mut c = c;

        let foreign = r
            .units
            .iter()
            .find(|(_, u)| u.unique_to.as_ref().is_some_and(|n| *n != owner))
            .map(|(id, _)| id)
            .expect("the ruleset has unique units");
        if let Some(tech) =
            r.units.get(foreign).unwrap().required_tech.as_deref().and_then(|name| r.technologies.id(name))
        {
            c.techs.insert(tech);
        }
        assert!(matches!(
            can_construct(&r, &c, &city, Construction::Unit(foreign)),
            Err(Rejection::UniqueToAnother(_))
        ));
    }

    #[test]
    fn costs_scale_and_truncate() {
        let r = ruleset();
        let library = Construction::Building(r.building("Library").unwrap());
        let base = production_cost(&r, library, CostModifiers::default()).unwrap();

        let slow =
            production_cost(&r, library, CostModifiers { speed: Fixed::whole(3), ..Default::default() }).unwrap();
        assert_eq!(slow, base * 3);

        // 1.5× truncates rather than rounds, as everywhere else.
        let city_state =
            production_cost(&r, library, CostModifiers { civ: Fixed::permille(1_500), ..Default::default() }).unwrap();
        assert_eq!(city_state, (base * 3) / 2);
    }

    #[test]
    fn conditional_unit_costs_use_the_live_city_and_keep_speed_scaling() {
        use crate::world::grid::TileGrid;
        let mut r = ruleset();
        let warrior = r.units.id("Warrior").unwrap();
        let unit = r.units.get_mut(warrior).unwrap();
        unit.cost = Some(100);
        unit.uniques.extend([
            crate::Unique::parse("Cost increases by [20] per owned city <in cities with at least [2] [Population]>"),
            crate::Unique::parse("[-25]% production cost <in cities with at least [2] [Population]>"),
        ]);
        let mut world = World::new(TileGrid::new(12, 12, r.terrain("Grassland").unwrap()), vec![civ(&r)]);
        let city = world.found_city_in(&r, City::found("Home", CivId(0), TileId(20)));
        world.found_city_in(&r, City::found("Second", CivId(0), TileId(60)));
        let m = CostModifiers {
            speed: Fixed::permille(1_500),
            civ: Fixed::permille(1_250),
            cities: 0,
            ..Default::default()
        };
        let what = Construction::Unit(warrior);
        assert_eq!(production_cost(&r, what, m), Some(187), "unknown city conditions do not apply");
        assert_eq!(production_cost_in(&world, &r, city, what, m), Some(187));
        crate::world::city::apply_food(world.cities.get_mut(city).unwrap(), 15, Default::default());
        // (100 + 20 * 2 cities) * 0.75 discount * 1.5 speed * 1.25 civ = 196.875
        assert_eq!(production_cost_in(&world, &r, city, what, m), Some(196));
    }

    #[test]
    fn production_accumulates_until_the_thing_is_paid_for() {
        let r = ruleset();
        let m = CostModifiers::default();
        let library = Construction::Building(r.building("Library").unwrap());
        let cost = production_cost(&r, library, m).unwrap();

        let mut city = City::found("Rome", CivId(0), TileId(0));
        city.queue.push(library);

        assert_eq!(apply_production(&mut city, &r, cost - 1, m), ConstructionEvent::Progressed { stored: cost - 1 });
        assert!(city.built.is_empty());

        assert_eq!(
            apply_production(&mut city, &r, 5, m),
            ConstructionEvent::Completed { what: library, carried_over: 4 }
        );
        assert!(city.queue.is_empty());
        assert!(city.built.contains(&r.building("Library").unwrap()));
        assert_eq!(city.production_overflow, 4, "the excess carries over");
    }

    #[test]
    fn an_empty_queue_does_not_bank_turn_production() {
        let r = ruleset();
        let mut city = City::found("Rome", CivId(0), TileId(0));
        assert_eq!(apply_production(&mut city, &r, 7, CostModifiers::default()), ConstructionEvent::Idle { stored: 0 });
    }

    #[test]
    fn a_fresh_civ_can_build_something_and_not_everything() {
        let r = ruleset();
        let c = civ(&r);
        let city = City::found("Rome", CivId(0), TileId(0));
        let options = available(&r, &c, &city);
        println!("with no techs: {} options", options.len());
        assert!(!options.is_empty(), "a new city must be able to build something");
        assert!(options.len() < r.buildings.len() + r.units.len(), "but not everything");
    }
}
