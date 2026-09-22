//! What a tile produces.
//!
//! The most-asked question in the engine — the AI evaluates it for every tile
//! it might settle, work or take — so it is also the one whose shape every
//! later system will copy.
//!
//! Terrain does **not** simply sum. Unciv folds over `[base, natural wonder,
//! ...features]` in that fixed order with three outcomes per step
//! (`TileStatFunctions.kt:164`):
//!
//! * a terrain with `NullifyYields` discards everything, including itself
//! * a terrain with `overrideStats` replaces whatever accumulated before it
//! * otherwise it adds
//!
//! That fold is order-*dependent* by design, which is why the sequence is
//! pinned rather than left to iteration order. It is not a contradiction of
//! [`ModifierStack`](crate::modifiers)'s order-independence: this establishes
//! the base value, and the stack handles the bonuses layered on top, where
//! order genuinely should not matter.
//!
//! Every step is attributed. Unciv returns `List<Pair<String, Stats>>` for the
//! same reason — "why is this tile worth 4 food" has to be answerable without
//! recomputing it.

use crate::modifiers::{Source, SourceKind};
use crate::ruleset::queries::HasStats;
use crate::ruleset::Ruleset;
use crate::stats::{Fixed, Stats};
use crate::world::grid::{TileGrid, TileId};
use crate::UniqueType;

/// One contribution to a tile's output.
pub type Contribution = (Source, Stats);

/// Terrain only: the fold described above.
fn terrain_yield(grid: &TileGrid, r: &Ruleset, tile: TileId) -> Vec<Contribution> {
    let mut acc: Vec<Contribution> = Vec::new();
    for id in grid.terrains(tile) {
        let Some(t) = r.terrains.get(id) else { continue };
        let kind = if id == grid.terrain[tile.0 as usize] {
            SourceKind::Terrain
        } else {
            SourceKind::Feature
        };
        let entry = (Source::new(kind, t.name.clone()), t.stats());

        if t.uniques.iter().any(|u| u.unique_type() == Some(UniqueType::NullifyYields)) {
            // Nothing before or after this counts. Ice, for instance.
            return vec![entry];
        }
        if t.override_stats == Some(true) {
            acc = vec![entry];
        } else {
            acc.push(entry);
        }
    }
    acc
}

/// Every contribution to a tile's yield, in the order they apply.
pub fn tile_yield_breakdown(grid: &TileGrid, r: &Ruleset, tile: TileId) -> Vec<Contribution> {
    let mut out = terrain_yield(grid, r, tile);
    let i = tile.0 as usize;

    let improvement = grid.improvement[i].and_then(|id| r.improvements.get(id));

    if let Some(res) = grid.resource[i].and_then(|id| r.resources.get(id)) {
        let mut stats = res.stats();

        // A resource's improvement bonus lands only once the matching
        // improvement is built — `improvementStats` in the json.
        if let Some(imp) = improvement {
            let matches = res.improvement.as_deref() == Some(imp.name.as_str())
                || res.improved_by.iter().any(|n| n == &imp.name);
            if matches {
                for (name, amount) in &res.improvement_stats {
                    if let Some(stat) = crate::stats::Stat::parse(name) {
                        stats[stat] += crate::stats::Fixed::whole(*amount);
                    }
                }
            }
        }
        if !stats.is_zero() {
            out.push((Source::new(SourceKind::Resource, res.name.clone()), stats));
        }
    }

    if let Some(imp) = improvement {
        let stats = imp.stats();
        if !stats.is_zero() {
            out.push((Source::new(SourceKind::Improvement, imp.name.clone()), stats));
        }
    }

    out
}

/// A city centre is never worse than this, whatever it is built on
/// (`Stats.kt:278`). Founding on snow still gives you something to work with.
pub const CITY_CENTRE_MINIMUM: Stats = Stats {
    food: Fixed::whole(2),
    production: Fixed::whole(1),
    gold: Fixed::ZERO,
    science: Fixed::ZERO,
    culture: Fixed::ZERO,
    faith: Fixed::ZERO,
    happiness: Fixed::ZERO,
};

/// The shortfall against a per-stat floor, or `None` if nothing is missing.
///
/// Returned as its own contribution rather than folded into the tile, so a
/// breakdown shows the top-up for what it is — matching Unciv's
/// `missingFromMinimum`.
pub fn missing_from_minimum(have: Stats, minimum: Stats) -> Option<Stats> {
    let mut gap = Stats::ZERO;
    for s in crate::stats::Stat::ALL {
        if have[s] < minimum[s] {
            gap[s] = minimum[s] - have[s];
        }
    }
    (!gap.is_zero()).then_some(gap)
}

/// The tile's total output.
pub fn tile_yield(grid: &TileGrid, r: &Ruleset, tile: TileId) -> Stats {
    tile_yield_breakdown(grid, r, tile).into_iter().map(|(_, s)| s).sum()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stats::{Fixed, Stat};

    fn ruleset() -> Ruleset {
        Ruleset::load_dir("../android/assets/jsons/Civ V - Gods & Kings")
            .expect("readable")
            .expect("loads")
    }

    fn one_tile(r: &Ruleset, terrain: &str) -> TileGrid {
        TileGrid::new(3, 3, r.terrain(terrain).unwrap_or_else(|| panic!("{terrain} exists")))
    }

    #[test]
    fn bare_terrain_yields_its_own_stats() {
        let r = ruleset();
        let g = one_tile(&r, "Grassland");
        let y = tile_yield(&g, &r, TileId(0));
        assert_eq!(y.food, Fixed::whole(2), "Grassland is 2 food");
        assert_eq!(y.production, Fixed::ZERO);
    }

    /// Marsh carries `food: -1` and no `overrideStats`, so it adds.
    #[test]
    fn a_plain_feature_adds_to_the_base() {
        let r = ruleset();
        let mut g = one_tile(&r, "Grassland");
        assert_eq!(tile_yield(&g, &r, TileId(0)).food, Fixed::whole(2));

        g.features[0].push(r.terrain("Marsh").expect("Marsh exists"));
        assert_eq!(tile_yield(&g, &r, TileId(0)).food, Fixed::whole(1), "2 food less Marsh's 1");

        let b = tile_yield_breakdown(&g, &r, TileId(0));
        assert_eq!(b.len(), 2);
        assert_eq!(b[1].0.name, "Marsh");
    }

    /// Forest carries `overrideStats: true`, so it replaces the base entirely —
    /// a forested Grassland yields Forest's 1/1, not Grassland's 2 food plus it.
    #[test]
    fn an_overriding_feature_replaces_the_base() {
        let r = ruleset();
        let mut g = one_tile(&r, "Grassland");
        g.features[0].push(r.terrain("Forest").expect("Forest exists"));

        let y = tile_yield(&g, &r, TileId(0));
        assert_eq!(y.food, Fixed::whole(1), "Grassland's 2 food is gone, not added to");
        assert_eq!(y.production, Fixed::whole(1));

        let b = tile_yield_breakdown(&g, &r, TileId(0));
        assert_eq!(b.len(), 1, "only Forest remains");
        assert_eq!(b[0].0.name, "Forest");
    }

    /// The rule that makes this a fold rather than a sum.
    #[test]
    fn override_stats_replaces_everything_before_it() {
        let r = ruleset();
        let overriding: Vec<&str> = r
            .terrains
            .values()
            .filter(|t| t.override_stats == Some(true))
            .map(|t| t.name.as_str())
            .collect();
        println!("overrideStats terrains: {overriding:?}");
        assert!(!overriding.is_empty(), "the shipped ruleset uses this rule");

        let feature = overriding[0].to_string();
        let mut g = one_tile(&r, "Plains");
        g.features[0].push(r.terrain(&feature).unwrap());

        let b = tile_yield_breakdown(&g, &r, TileId(0));
        assert_eq!(b.len(), 1, "the base terrain was replaced, not added to");
        assert_eq!(b[0].0.name, feature);
    }

    #[test]
    fn resources_and_improvements_stack_on_top() {
        let r = ruleset();
        let mut g = one_tile(&r, "Grassland");
        let base = tile_yield(&g, &r, TileId(0));

        // Wheat is a Plains resource in Civ V, so pick whatever this ruleset
        // says can sit on Grassland rather than hard-coding a guess.
        let (res_id, res) = r
            .resources
            .iter()
            .find(|(_, res)| {
                res.terrains_can_be_found_on.iter().any(|t| t == "Grassland") && !res.stats().is_zero()
            })
            .expect("some resource yields on Grassland");
        g.resource[0] = Some(res_id);

        let with_res = tile_yield(&g, &r, TileId(0));
        println!("{} on Grassland: {base} -> {with_res}", res.name);
        assert!(Stat::ALL.into_iter().any(|s| with_res[s] > base[s]));

        let b = tile_yield_breakdown(&g, &r, TileId(0));
        assert!(b.iter().any(|(src, _)| src.kind == SourceKind::Resource));
    }

    #[test]
    fn the_breakdown_sums_to_the_total() {
        let r = ruleset();
        let mut g = one_tile(&r, "Plains");
        g.features[0].push(r.terrain("Forest").unwrap());
        let tile = TileId(0);
        let summed: Stats = tile_yield_breakdown(&g, &r, tile).into_iter().map(|(_, s)| s).sum();
        assert_eq!(summed, tile_yield(&g, &r, tile));
    }

    /// Every terrain in the ruleset resolves, and the fold never loses the
    /// base when it should not.
    ///
    /// Negative yields are legitimate — Marsh is `food: -1` — so this checks
    /// resolution and attribution rather than sign.
    #[test]
    fn every_shipped_terrain_resolves() {
        let r = ruleset();
        let mut negative = Vec::new();
        for (id, t) in r.terrains.iter() {
            let g = TileGrid::new(1, 1, id);
            let b = tile_yield_breakdown(&g, &r, TileId(0));
            assert_eq!(b.len(), 1, "{} produced {} contributions alone", t.name, b.len());
            assert_eq!(b[0].0.name, t.name, "a contribution must name its source");
            let y = tile_yield(&g, &r, TileId(0));
            if Stat::ALL.into_iter().any(|s| y[s] < Fixed::ZERO) {
                negative.push(t.name.as_str());
            }
        }
        println!("terrain with a negative yield: {negative:?}");
        assert!(negative.contains(&"Marsh"), "Marsh costs food; that is the rule, not a bug");
    }
}
