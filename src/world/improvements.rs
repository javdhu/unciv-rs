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
}

/// Is this improvement a road? Asked by name, because a road is a road in the
/// ruleset's vocabulary and an axis of its own in the world's.
fn road_level_of(r: &Ruleset, improvement: Id<TileImprovement>) -> Option<crate::world::grid::RoadLevel> {
    crate::world::grid::RoadLevel::from_improvement_name(r.improvements.name_of(improvement)?)
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
    // Only things with a build time are worker jobs; the rest are placed by
    // other means (ruins, city centres, great people).
    if imp.turns_to_build.unwrap_or(-1) <= 0 {
        return Err(ImprovementRejection::NotBuildable);
    }
    // Roads sit on their own axis, so a tile can take a road *and* a farm.
    // What blocks a road is a road, not a farm.
    match road_level_of(r, improvement) {
        Some(level) => {
            if world.grid.unpillaged_road(tile) >= level {
                return Err(ImprovementRejection::AlreadyPaved);
            }
        }
        None => {
            if world.grid.improvement[tile.0 as usize].is_some() {
                return Err(ImprovementRejection::AlreadyImproved);
            }
        }
    }
    if let Some(tech) = imp.tech_required.as_deref() {
        let known = r
            .technologies
            .id(tech)
            .zip(world.civ(civ))
            .is_some_and(|(t, c)| c.has_tech(t));
        if !known {
            return Err(ImprovementRejection::NotUnlocked(tech.to_string()));
        }
    }
    let fits = imp.terrains_can_be_built_on.iter().any(|t| {
        tile_matches_in(&world.grid, r, tile, t, Some(civ)) == Verdict::True
    });
    if !fits {
        return Err(ImprovementRejection::WrongTerrain);
    }
    Ok(())
}

/// Everything a worker could build here, in id order.
pub fn buildable(world: &World, r: &Ruleset, civ: CivId, tile: TileId) -> Vec<Id<TileImprovement>> {
    r.improvements
        .ids()
        .filter(|i| can_build(world, r, civ, tile, *i).is_ok())
        .collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkEvent {
    Progressed { tile: TileId, done: u32, needed: u32 },
    Finished { tile: TileId, improvement: Id<TileImprovement> },
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
    let needed = r
        .improvements
        .get(improvement)
        .and_then(|i| i.turns_to_build)
        .unwrap_or(1)
        .max(1) as u32;

    let slot = &mut world.grid.improvement_progress[tile.0 as usize];
    let done = match slot {
        Some((what, done)) if *what == improvement => *done + 1,
        _ => 1,
    };

    if done >= needed {
        *slot = None;
        match road_level_of(r, improvement) {
            Some(level) => world.grid.set_road(tile, level),
            None => world.grid.improvement[tile.0 as usize] = Some(improvement),
        }
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
pub fn conditional_yields(
    world: &World,
    r: &Ruleset,
    civ: CivId,
    tile: TileId,
) -> crate::stats::Stats {
    use crate::stats::Stats;
    let Some(imp) = world.grid.improvement[tile.0 as usize].and_then(|i| r.improvements.get(i))
    else {
        return Stats::ZERO;
    };
    let ctx = GameContext::new(r).with_world(world).for_civ(civ).on_tile(tile);
    let mut out = Stats::ZERO;
    for u in &imp.uniques {
        if u.unique_type() != Some(crate::UniqueType::Stats) {
            continue;
        }
        if crate::world::conditionals::applies(u, &ctx) != Verdict::True {
            continue;
        }
        if let Some(stats) = u.params.first().and_then(|p| Stats::parse(p)) {
            out += stats;
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
        Ruleset::load_dir("../android/assets/jsons/Civ V - Gods & Kings")
            .expect("readable")
            .expect("loads")
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
    fn an_improvement_needs_the_right_terrain_and_the_tech() {
        let r = ruleset();
        let mut w = world(&r);
        let (farm, mine) = (r.improvements.id("Farm").unwrap(), r.improvements.id("Mine").unwrap());
        let flat = TileId(9);

        assert!(matches!(
            can_build(&w, &r, CivId(0), flat, farm),
            Err(ImprovementRejection::NotUnlocked(_))
        ));
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
        assert_eq!(
            work_on(&mut w, &r, CivId(0), tile, farm),
            Ok(WorkEvent::Finished { tile, improvement: farm })
        );
        assert_eq!(w.grid.improvement[9], Some(farm));
        assert!(w.grid.improvement_progress[9].is_none(), "progress is cleared");
        assert_eq!(
            can_build(&w, &r, CivId(0), tile, farm),
            Err(ImprovementRejection::AlreadyImproved)
        );
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
            assert_eq!(
                w.grid.improvement_progress[9],
                Some((other, 1)),
                "a half-dug mine is not a half-built farm"
            );
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
