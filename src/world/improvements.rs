//! Building farms and mines.
//!
//! The main way a Civ city gets stronger: a worked grassland gives 2 food, and
//! a farmed one gives 3. Everything needed was already here — `TileImprovement`
//! loads, `grid.improvement` exists, and `tile_yield` reads it — but nothing
//! ever put one down.
//!
//! Improvements also carry conditional bonuses that arrive with technology:
//! a Mine is `[+1 Production] <after discovering [Chemistry]>`, so the same
//! mine is worth more later. Those are uniques, evaluated rather than
//! hard-coded, which means a tile's yield depends on what its owner knows.

use crate::arena::Id;
use crate::ruleset::{Ruleset, TileImprovement};
use crate::world::conditionals::{tile_matches_in, GameContext, Verdict};
use crate::world::grid::{CivId, TileId};
use crate::world::world::World;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImprovementRejection {
    /// The terrain does not accept it — a Mine wants a hill.
    WrongTerrain,
    /// The unlocking tech is not researched.
    NotUnlocked(String),
    /// Something is already here.
    AlreadyImproved,
    /// This tile already carries this road, or a better one.
    AlreadyPaved,
    /// Not a thing a worker can build.
    NotBuildable,
    Unknown,
    OutsideBorders,
}

impl std::fmt::Display for ImprovementRejection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ImprovementRejection::WrongTerrain => write!(f, "the terrain does not accept it"),
            ImprovementRejection::NotUnlocked(tech) => write!(f, "needs {tech}"),
            ImprovementRejection::AlreadyImproved => write!(f, "the tile is already improved"),
            ImprovementRejection::AlreadyPaved => write!(f, "the tile already has this road or a better one"),
            ImprovementRejection::NotBuildable => write!(f, "not something a worker can build"),
            ImprovementRejection::OutsideBorders => write!(f, "outside permitted territory"),
            ImprovementRejection::Unknown => write!(f, "not in this ruleset"),
        }
    }
}

impl std::error::Error for ImprovementRejection {}

/// Is this improvement a road? Asked by name, because a road is a road in the
/// ruleset's vocabulary and an axis of its own in the world's.
fn road_level_of(r: &Ruleset, improvement: Id<TileImprovement>) -> Option<crate::world::grid::RoadLevel> {
    crate::world::grid::RoadLevel::from_improvement_name(r.improvements.name_of(improvement)?)
}

/// Terrain restrictions apply to both worker and great-person improvements.
pub fn terrain_allows(world: &World, r: &Ruleset, civ: CivId, tile: TileId, improvement: Id<TileImprovement>) -> bool {
    if !world.grid.contains(tile) {
        return false;
    }
    let Some(imp) = r.improvements.get(improvement) else {
        return false;
    };
    let ctx = GameContext::new(r).with_world(world).for_civ(civ).on_tile(tile);
    world
        .grid
        .terrains(tile)
        .flat_map(|id| &r.terrains[id].uniques)
        .filter(|u| {
            u.unique_type() == Some(crate::UniqueType::RestrictedBuildableImprovements)
                && crate::world::conditionals::applies(u, &ctx).holds()
        })
        .all(|u| {
            u.params.first().is_some_and(|filter| match filter.as_str() {
                "All" => true,
                "All Road" => road_level_of(r, improvement).is_some(),
                name => imp.name == name,
            })
        })
}

/// Whether a worker of this civ could start this improvement here.
pub fn can_build(
    world: &World,
    r: &Ruleset,
    civ: CivId,
    tile: TileId,
    improvement: Id<TileImprovement>,
) -> Result<(), ImprovementRejection> {
    let Some(imp) = r.improvements.get(improvement) else {
        return Err(ImprovementRejection::Unknown);
    };
    use crate::UniqueType as U;
    let i = tile.0 as usize;
    let c = world.civ(civ).ok_or(ImprovementRejection::NotBuildable)?;
    if i >= world.grid.len() {
        return Err(ImprovementRejection::WrongTerrain);
    }
    let ctx = GameContext::new(r).with_world(world).for_civ(civ).on_tile(tile);
    let has = |kind| {
        imp.uniques
            .iter()
            .any(|u| u.unique_type() == Some(kind) && crate::world::conditionals::applies(u, &ctx).holds())
    };
    if imp.unique_to.as_deref().is_some_and(|n| r.nations.name_of(c.nation) != Some(n)) {
        return Err(ImprovementRejection::NotBuildable);
    }
    if world.grid.owner[i] != Some(civ) && !has(U::CanBuildOutsideBorders) {
        return Err(ImprovementRejection::OutsideBorders);
    }
    if world.grid.owner[i].is_some_and(|owner| owner != civ && !world.civ(owner).is_some_and(|c| c.is_city_state())) {
        return Err(ImprovementRejection::OutsideBorders);
    }
    if world.cities.iter().any(|(_, city)| city.centre == tile) {
        return Err(ImprovementRejection::WrongTerrain);
    }
    if imp.name == "Cancel improvement order" {
        return if world.grid.improvement_progress[i].is_some() {
            Ok(())
        } else {
            Err(ImprovementRejection::NotBuildable)
        };
    }
    if imp.turns_to_build.unwrap_or(-1) <= 0 || (has(U::Unbuildable) && imp.name != "Repair") || has(U::Unavailable) {
        return Err(ImprovementRejection::NotBuildable);
    }
    if let Some(tech) = imp.tech_required.as_deref() {
        if !r.technologies.id(tech).is_some_and(|t| c.has_tech(t)) {
            return Err(ImprovementRejection::NotUnlocked(tech.to_string()));
        }
    }
    if imp.name == "Repair" {
        return if world.grid.improvement_pillaged[i] || world.grid.road_pillaged[i] {
            Ok(())
        } else {
            Err(ImprovementRejection::NotBuildable)
        };
    }
    if let Some(target) = imp.name.strip_prefix("Remove ") {
        let present = crate::world::grid::RoadLevel::from_improvement_name(target).map_or_else(
            || world.grid.features[i].iter().any(|f| r.terrains.name_of(*f) == Some(target)),
            |road| world.grid.road[i] == road,
        );
        return if present { Ok(()) } else { Err(ImprovementRejection::WrongTerrain) };
    }
    if let Some(level) = road_level_of(r, improvement) {
        if world.grid.is_water(r, tile) {
            return Err(ImprovementRejection::WrongTerrain);
        }
        return if world.grid.unpillaged_road(tile) < level { Ok(()) } else { Err(ImprovementRejection::AlreadyPaved) };
    }
    if !terrain_allows(world, r, civ, tile, improvement) {
        return Err(ImprovementRejection::WrongTerrain);
    }
    if world.grid.improvement[i] == Some(improvement) && !world.grid.improvement_pillaged[i] {
        return Err(ImprovementRejection::AlreadyImproved);
    }
    if world.grid.improvement[i]
        .and_then(|id| r.improvements.get(id))
        .is_some_and(|old| old.uniques.iter().any(|u| u.unique_type() == Some(U::Irremovable)))
    {
        return Err(ImprovementRejection::NotBuildable);
    }
    // A feature masks the terrain below it: forests must be cleared before
    // planting a farm, while camps explicitly coexist with forests/jungle.
    let top = world.grid.features[i].last().copied().unwrap_or(world.grid.terrain[i]);
    let top_name = r.terrains.name_of(top).unwrap_or_default();
    let on_feature = imp.terrains_can_be_built_on.iter().any(|t| t == top_name)
        || imp.uniques.iter().any(|u| {
            u.unique_type() == Some(U::NoFeatureRemovalNeeded) && u.params.first().is_some_and(|p| p == top_name)
        });
    if r.terrains[top].unbuildable == Some(true) && !on_feature {
        return Err(ImprovementRejection::WrongTerrain);
    }
    for u in &imp.uniques {
        if !crate::world::conditionals::applies(u, &ctx).holds() {
            continue;
        }
        let matches = u.params.first().is_some_and(|f| tile_matches_in(&world.grid, r, tile, f, Some(civ)).holds());
        match u.unique_type() {
            Some(U::CannotBuildOnTile) if matches => return Err(ImprovementRejection::WrongTerrain),
            Some(U::CanOnlyBeBuiltOnTile) if !matches => return Err(ImprovementRejection::WrongTerrain),
            Some(U::MustBeNextTo)
                if !u.params.first().is_some_and(|f| {
                    world.grid.neighbours(tile).any(|t| tile_matches_in(&world.grid, r, t, f, Some(civ)).holds())
                }) =>
            {
                return Err(ImprovementRejection::WrongTerrain)
            }
            _ => {}
        }
    }
    let resource = world.grid.resource[i].and_then(|id| r.resources.get(id));
    let improves_resource = resource.is_some_and(|res| {
        let visible =
            res.revealed_by.as_deref().is_none_or(|name| r.technologies.id(name).is_some_and(|t| c.has_tech(t)));
        visible && (res.improvement.as_deref() == Some(imp.name.as_str()) || res.improved_by.contains(&imp.name))
    });
    if has(U::CanOnlyImproveResource) && !improves_resource {
        return Err(ImprovementRejection::WrongTerrain);
    }
    let water = world.grid.is_water(r, tile);
    let resource_domain = imp.terrains_can_be_built_on.is_empty()
        || imp.terrains_can_be_built_on.iter().any(|name| {
            name == if water { "Water" } else { "Land" }
                || r.terrains.by_name(name).is_some_and(|t| {
                    t.r#type == if water { "Water" } else { "Land" }
                        || t.occurs_on.iter().any(|base| {
                            r.terrains.by_name(base).is_some_and(|b| b.r#type == if water { "Water" } else { "Land" })
                        })
                })
        });
    let fits = on_feature
        || imp.terrains_can_be_built_on.iter().any(|f| f == if water { "Water" } else { "Land" })
        || (has(U::ImprovementBuildableByFreshWater) && world.grid.is_fresh_water(r, tile))
        || (improves_resource && resource_domain);
    if !fits {
        return Err(ImprovementRejection::WrongTerrain);
    }
    Ok(())
}

/// Everything a worker could build here, in id order.
pub fn buildable(world: &World, r: &Ruleset, civ: CivId, tile: TileId) -> Vec<Id<TileImprovement>> {
    r.improvements.ids().filter(|i| can_build(world, r, civ, tile, *i).is_ok()).collect()
}

fn improvement_matches(r: &Ruleset, improvement: Id<TileImprovement>, filter: &str) -> bool {
    let imp = &r.improvements[improvement];
    crate::world::filters::multi_filter(filter, &|f| {
        if f == "All" || f == "all" || f == "Improvement" || f == imp.name || imp.uniques.iter().any(|u| u.text == f) {
            Verdict::True
        } else {
            Verdict::False
        }
    })
    .holds()
}

/// Whether this unit can perform a legal job, including expendable Work Boats.
pub fn can_work(world: &World, r: &Ruleset, unit: Id<crate::world::Unit>, improvement: Id<TileImprovement>) -> bool {
    use crate::UniqueType as U;
    let Some(u) = world.units.get(unit) else { return false };
    if can_build(world, r, u.owner, u.position, improvement).is_err() {
        return false;
    }
    let Some(base) = r.units.get(u.base) else { return false };
    let ctx = GameContext::new(r).with_world(world).for_civ(u.owner).for_unit(unit).on_tile(u.position);
    base.uniques.iter().any(|unique| {
        crate::world::conditionals::applies(unique, &ctx).holds()
            && match unique.unique_type() {
                Some(U::BuildImprovements) => unique.params.first().is_some_and(|f| {
                    improvement_matches(r, improvement, f)
                        || tile_matches_in(&world.grid, r, u.position, f, Some(u.owner)).holds()
                }),
                Some(U::CreateWaterImprovements) => {
                    world.grid.is_water(r, u.position)
                        && world.grid.resource[u.position.0 as usize].and_then(|res| r.resources.get(res)).is_some_and(
                            |res| {
                                res.improvement.as_deref() == Some(r.improvements[improvement].name.as_str())
                                    || res.improved_by.contains(&r.improvements[improvement].name)
                            },
                        )
                }
                _ => false,
            }
    }) && !u.embarked
}

pub fn build_turns(
    world: &World,
    r: &Ruleset,
    civ: CivId,
    unit: Option<Id<crate::world::Unit>>,
    improvement: Id<TileImprovement>,
) -> u32 {
    use crate::{stats::Fixed, UniqueType as U};
    let mut ctx = GameContext::new(r).with_world(world).for_civ(civ);
    if let Some(id) = unit {
        ctx = ctx.for_unit(id);
    }
    let unit = unit.and_then(|id| world.units.get(id));
    let uniques: Vec<_> = world
        .civ(civ)
        .into_iter()
        .flat_map(|c| c.uniques.all())
        .chain(unit.and_then(|u| r.units.get(u.base)).into_iter().flat_map(|b| &b.uniques))
        .chain(
            unit.into_iter().flat_map(|u| &u.promotions).filter_map(|p| r.promotions.get(*p)).flat_map(|p| &p.uniques),
        )
        .filter(|u| crate::world::conditionals::applies(u, &ctx).holds())
        .collect();
    let increase: Fixed = uniques
        .iter()
        .filter(|u| {
            u.unique_type() == Some(U::ImprovementTimeIncrease)
                && u.params.first().is_some_and(|f| improvement_matches(r, improvement, f))
        })
        .filter_map(|u| u.params.get(1).and_then(|p| Fixed::parse(p)))
        .sum();
    let mut turns = Fixed::whole(r.improvements[improvement].turns_to_build.unwrap_or(1).max(1)) * Fixed::whole(100);
    turns = turns.checked_div(Fixed::whole(100) + increase).unwrap_or(Fixed::ONE);
    for unique in uniques.iter().filter(|u| {
        u.unique_type() == Some(U::SpecificImprovementTime)
            && u.params.get(1).is_some_and(|f| improvement_matches(r, improvement, f))
    }) {
        if let Some(percent) = unique.params.first().and_then(|p| Fixed::parse(p)) {
            turns = turns * (Fixed::ONE + percent.checked_div(Fixed::whole(100)).unwrap_or_default());
        }
    }
    turns.round().max(1) as u32
}

/// The command and automatic continuation share this boundary so a worker
/// contributes at most once per round. Working in place still permits healing.
pub fn perform_work(
    world: &mut World,
    r: &Ruleset,
    unit: Id<crate::world::Unit>,
    improvement: Id<TileImprovement>,
) -> Result<WorkEvent, ImprovementRejection> {
    if !can_work(world, r, unit, improvement) {
        return Err(ImprovementRejection::NotBuildable);
    }
    let u = world.units.get(unit).expect("validated worker");
    if u.movement_left <= crate::stats::Fixed::ZERO {
        return Err(ImprovementRejection::NotBuildable);
    }
    let (civ, tile) = (u.owner, u.position);
    let boat =
        r.units[u.base].uniques.iter().any(|u| u.unique_type() == Some(crate::UniqueType::CreateWaterImprovements));
    let needed = if boat { 1 } else { build_turns(world, r, civ, Some(unit), improvement) };
    let result = work(world, r, civ, tile, improvement, needed)?;
    if !matches!(result, WorkEvent::Cancelled { .. }) {
        if boat {
            world.remove_unit(unit);
        } else if let Some(u) = world.units.get_mut(unit) {
            u.movement_left = crate::stats::Fixed::ZERO;
        }
    }
    Ok(result)
}

pub fn continue_work(world: &mut World, r: &Ruleset) -> Vec<WorkEvent> {
    let jobs: Vec<_> = world
        .units
        .iter()
        .filter_map(|(id, u)| world.grid.improvement_progress[u.position.0 as usize].map(|(imp, _)| (id, imp)))
        .collect();
    jobs.into_iter().filter_map(|(unit, imp)| perform_work(world, r, unit, imp).ok()).collect()
}

fn complete(world: &mut World, r: &Ruleset, civ: CivId, tile: TileId, improvement: Id<TileImprovement>) {
    let i = tile.0 as usize;
    let name = &r.improvements[improvement].name;
    if name == "Repair" {
        world.grid.repair_improvement(tile);
        world.grid.repair_road(tile);
    } else if let Some(target) = name.strip_prefix("Remove ") {
        if crate::world::grid::RoadLevel::from_improvement_name(target).is_some() {
            world.grid.set_road(tile, crate::world::grid::RoadLevel::None);
        } else if let Some(feature) = r.terrains.id(target) {
            if let Some((city_id, distance)) = world
                .cities
                .iter()
                .filter(|(_, c)| c.owner == civ)
                .map(|(id, c)| (id, world.grid.distance(tile, c.centre)))
                .min_by_key(|(id, d)| (*d, *id))
                .filter(|(_, d)| *d <= 5)
            {
                let production: crate::stats::Fixed = r.terrains[feature]
                    .uniques
                    .iter()
                    .filter(|u| u.unique_type() == Some(crate::UniqueType::ProductionBonusWhenRemoved))
                    .filter_map(|u| u.params.first().and_then(|s| crate::stats::Stats::parse(s)))
                    .map(|s| s.production)
                    .sum();
                let distance_factor = if distance == 1 {
                    crate::stats::Fixed::ONE
                } else {
                    crate::stats::Fixed::from_fraction(i64::from(6 - distance), 4)
                };
                let ownership_factor = if world.grid.owner[i] == Some(civ) {
                    crate::stats::Fixed::ONE
                } else {
                    crate::stats::Fixed::from_fraction(2, 3)
                };
                if let Some(city) = world.cities.get_mut(city_id) {
                    city.production_overflow += (production * distance_factor * ownership_factor).floor() as i32;
                }
            }
            let old = world.grid.improvement[i].and_then(|id| r.improvements.get(id));
            if old.is_some_and(|imp| {
                imp.terrains_can_be_built_on.contains(&target.to_string())
                    && !imp.terrains_can_be_built_on.contains(&r.terrains[world.grid.terrain[i]].name)
            }) {
                world.grid.set_improvement(tile, None);
            }
            world.grid.features[i].retain(|f| *f != feature);
        }
    } else if let Some(level) = road_level_of(r, improvement) {
        world.grid.set_road(tile, level);
    } else {
        world.grid.set_improvement(tile, Some(improvement));
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkEvent {
    Progressed { tile: TileId, done: u32, needed: u32 },
    Finished { tile: TileId, improvement: Id<TileImprovement> },
    Cancelled { tile: TileId },
}

/// Put one worker-turn into a tile.
///
/// Starting something different discards the old progress, as Unciv does —
/// a half-dug mine is not a half-built farm.
pub fn work_on(
    world: &mut World,
    r: &Ruleset,
    civ: CivId,
    tile: TileId,
    improvement: Id<TileImprovement>,
) -> Result<WorkEvent, ImprovementRejection> {
    can_build(world, r, civ, tile, improvement)?;
    let needed = build_turns(world, r, civ, None, improvement);
    work(world, r, civ, tile, improvement, needed)
}

fn work(
    world: &mut World,
    r: &Ruleset,
    civ: CivId,
    tile: TileId,
    improvement: Id<TileImprovement>,
    needed: u32,
) -> Result<WorkEvent, ImprovementRejection> {
    if r.improvements[improvement].name == "Cancel improvement order" {
        world.grid.improvement_progress[tile.0 as usize] = None;
        return Ok(WorkEvent::Cancelled { tile });
    }

    let slot = &mut world.grid.improvement_progress[tile.0 as usize];
    let done = match slot {
        Some((what, done)) if *what == improvement => *done + 1,
        _ => 1,
    };

    if done >= needed {
        *slot = None;
        complete(world, r, civ, tile, improvement);
        Ok(WorkEvent::Finished { tile, improvement })
    } else {
        *slot = Some((improvement, done));
        Ok(WorkEvent::Progressed { tile, done, needed })
    }
}

/// Extra yields an improvement grants once the owner knows enough.
///
/// A Mine is `[+1 Production] <after discovering [Chemistry]>`. The same mine
/// is worth more later, so this depends on the civ, not only the tile.
pub fn conditional_yields(world: &World, r: &Ruleset, civ: CivId, tile: TileId) -> crate::stats::Stats {
    use crate::stats::Stats;
    let Some(imp) = world.grid.unpillaged_improvement(tile).and_then(|i| r.improvements.get(i)) else {
        return Stats::ZERO;
    };
    let ctx = GameContext::new(r).with_world(world).for_civ(civ).on_tile(tile);
    let mut out = Stats::ZERO;
    for u in &imp.uniques {
        use crate::UniqueType as U;
        if !crate::world::conditionals::applies(u, &ctx).holds() {
            continue;
        }
        let multiplier = match u.unique_type() {
            Some(U::Stats) => 1,
            Some(U::ImprovementStatsOnTile) => {
                i32::from(u.params.get(1).is_some_and(|f| tile_matches_in(&world.grid, r, tile, f, Some(civ)).holds()))
            }
            Some(U::ImprovementStatsForAdjacencies) => u.params.get(1).map_or(0, |f| {
                world
                    .grid
                    .neighbours(tile)
                    .filter(|t| tile_matches_in(&world.grid, r, *t, f, Some(civ)).holds())
                    .count() as i32
            }),
            _ => 0,
        };
        if let Some(stats) = u.params.first().and_then(|p| Stats::parse(p)) {
            for stat in crate::stats::Stat::ALL {
                out[stat] += stats[stat] * crate::stats::Fixed::whole(multiplier);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::entities::Civilization;
    use crate::world::grid::TileGrid;

    fn ruleset() -> Ruleset {
        Ruleset::load_dir("../android/assets/jsons/Civ V - Gods & Kings").expect("readable").expect("loads")
    }

    fn world(r: &Ruleset) -> World {
        let mut g = TileGrid::new(8, 8, r.terrain("Grassland").unwrap());
        g.owner.iter_mut().for_each(|o| *o = Some(CivId(0)));
        let nation = r.nations.ids().next().unwrap();
        World::new(g, vec![Civilization::new(CivId(0), nation, false)])
    }

    fn know(w: &mut World, r: &Ruleset, tech: &str) {
        if let Some(t) = r.technologies.id(tech) {
            w.civ_mut(CivId(0)).unwrap().techs.insert(t);
        }
    }

    #[test]
    fn resources_features_national_rules_and_repairs_are_enforced() {
        let r = ruleset();
        let mut w = world(&r);
        let civ = CivId(0);
        let tile = TileId(9);
        for tech in ["Animal Husbandry", "Calendar", "Mining", "Trapping", "Construction"] {
            know(&mut w, &r, tech);
        }
        let pasture = r.improvements.id("Pasture").unwrap();
        assert!(can_build(&w, &r, civ, tile, pasture).is_err());
        w.grid.resource[9] = r.resources.id("Horses");
        assert_eq!(can_build(&w, &r, civ, tile, pasture), Ok(()));
        w.grid.features[9].push(r.terrain("Forest").unwrap());
        assert!(can_build(&w, &r, civ, tile, pasture).is_err());
        w.grid.resource[9] = r.resources.id("Deer");
        assert!(can_build(&w, &r, civ, tile, r.improvements.id("Camp").unwrap()).is_ok());
        assert!(can_build(&w, &r, civ, tile, r.improvements.id("Moai").unwrap()).is_err());
        let remove = r.improvements.id("Remove Forest").unwrap();
        for _ in 0..4 {
            work_on(&mut w, &r, civ, tile, remove).unwrap();
        }
        assert!(w.grid.features[9].is_empty());
        assert!(w.grid.improvement[9].is_none());
        w.grid.set_improvement(tile, Some(pasture));
        w.grid.pillage_improvement(tile);
        let repair = r.improvements.id("Repair").unwrap();
        work_on(&mut w, &r, civ, tile, repair).unwrap();
        work_on(&mut w, &r, civ, tile, repair).unwrap();
        assert_eq!(w.grid.unpillaged_improvement(tile), Some(pasture));
    }

    #[test]
    fn worker_continues_once_per_round_and_boats_are_consumed() {
        use crate::world::Unit;
        let r = ruleset();
        let mut w = world(&r);
        let civ = CivId(0);
        let tile = TileId(9);
        know(&mut w, &r, "Agriculture");
        know(&mut w, &r, "Sailing");
        let id = w.place_unit(Unit::new(r.units.id("Worker").unwrap(), civ, tile, 2));
        let farm = r.improvements.id("Farm").unwrap();
        perform_work(&mut w, &r, id, farm).unwrap();
        assert!(continue_work(&mut w, &r).is_empty());
        for _ in 1..7 {
            w.units.get_mut(id).unwrap().movement_left = crate::stats::Fixed::whole(2);
            assert_eq!(continue_work(&mut w, &r).len(), 1);
        }
        assert_eq!(w.grid.improvement[9], Some(farm));
        let sea = TileId(12);
        w.grid.terrain[12] = r.terrain("Coast").unwrap();
        w.grid.resource[12] = r.resources.id("Fish");
        let boat = w.place_unit(Unit::new(r.units.id("Work Boats").unwrap(), civ, sea, 2));
        perform_work(&mut w, &r, boat, r.improvements.id("Fishing Boats").unwrap()).unwrap();
        assert!(w.units.get(boat).is_none());
        assert_eq!(w.grid.improvement[12], r.improvements.id("Fishing Boats"));
    }

    #[test]
    fn an_improvement_needs_the_right_terrain_and_the_tech() {
        let r = ruleset();
        let mut w = world(&r);
        let (farm, mine) = (r.improvements.id("Farm").unwrap(), r.improvements.id("Mine").unwrap());
        let flat = TileId(9);

        assert!(matches!(can_build(&w, &r, CivId(0), flat, farm), Err(ImprovementRejection::NotUnlocked(_))));
        know(&mut w, &r, "Agriculture");
        assert_eq!(can_build(&w, &r, CivId(0), flat, farm), Ok(()));

        // A mine wants a hill, whatever you know.
        know(&mut w, &r, "Mining");
        assert_eq!(can_build(&w, &r, CivId(0), flat, mine), Err(ImprovementRejection::WrongTerrain));
        w.grid.features[9].push(r.terrain("Hill").unwrap());
        assert_eq!(can_build(&w, &r, CivId(0), flat, mine), Ok(()));
    }

    #[test]
    fn work_accumulates_and_finishes() {
        let r = ruleset();
        let mut w = world(&r);
        know(&mut w, &r, "Agriculture");
        let farm = r.improvements.id("Farm").unwrap();
        let tile = TileId(9);
        let needed = r.improvements[farm].turns_to_build.unwrap() as u32;

        for turn in 1..needed {
            assert_eq!(
                work_on(&mut w, &r, CivId(0), tile, farm),
                Ok(WorkEvent::Progressed { tile, done: turn, needed })
            );
            assert!(w.grid.improvement[9].is_none(), "not finished yet");
        }
        assert_eq!(work_on(&mut w, &r, CivId(0), tile, farm), Ok(WorkEvent::Finished { tile, improvement: farm }));
        assert_eq!(w.grid.improvement[9], Some(farm));
        assert!(w.grid.improvement_progress[9].is_none(), "progress is cleared");
        assert_eq!(can_build(&w, &r, CivId(0), tile, farm), Err(ImprovementRejection::AlreadyImproved));
    }

    #[test]
    fn switching_jobs_discards_the_work_done() {
        let r = ruleset();
        let mut w = world(&r);
        know(&mut w, &r, "Agriculture");
        know(&mut w, &r, "Mining");
        w.grid.features[9].push(r.terrain("Hill").unwrap());
        let (farm, mine) = (r.improvements.id("Farm").unwrap(), r.improvements.id("Mine").unwrap());
        let tile = TileId(9);

        // Farm can go on a hill? Only if the terrain allows; use whichever two
        // are both legal here.
        let legal = buildable(&w, &r, CivId(0), tile);
        assert!(legal.contains(&mine));
        let other = legal.into_iter().find(|i| *i != mine);

        work_on(&mut w, &r, CivId(0), tile, mine).unwrap();
        work_on(&mut w, &r, CivId(0), tile, mine).unwrap();
        assert_eq!(w.grid.improvement_progress[9], Some((mine, 2)));

        if let Some(other) = other {
            work_on(&mut w, &r, CivId(0), tile, other).unwrap();
            assert_eq!(w.grid.improvement_progress[9], Some((other, 1)), "a half-dug mine is not a half-built farm");
        } else {
            let _ = farm;
        }
    }

    /// The same mine is worth more once you know Chemistry.
    #[test]
    fn improvements_grow_with_technology() {
        let r = ruleset();
        let mut w = world(&r);
        know(&mut w, &r, "Mining");
        w.grid.features[9].push(r.terrain("Hill").unwrap());
        let mine = r.improvements.id("Mine").unwrap();
        let tile = TileId(9);
        w.grid.improvement[9] = Some(mine);

        let before = conditional_yields(&w, &r, CivId(0), tile);
        assert_eq!(before, crate::stats::Stats::ZERO, "nothing extra yet");

        know(&mut w, &r, "Chemistry");
        let after = conditional_yields(&w, &r, CivId(0), tile);
        println!("Mine after Chemistry: {after}");
        assert_eq!(after.production, crate::stats::Fixed::whole(1));
    }
}
