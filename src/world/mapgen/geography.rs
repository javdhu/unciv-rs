//! Deterministic water basins and rivers along hex edges.
use crate::world::grid::{TileGrid, TileId};
use crate::{rng::Rng, ruleset::Ruleset, stats::Fixed};
use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet, BinaryHeap};

pub(super) fn lakes(r: &Ruleset, grid: &mut TileGrid) {
    let Some(lake) = r.terrains.id("Lakes").or_else(|| r.terrains.id("Lake")) else { return };
    let mut seen = vec![false; grid.len()];
    for start in grid.ids().collect::<Vec<_>>() {
        if seen[start.0 as usize] || !grid.is_water(r, start) {
            continue;
        }
        let mut basin = vec![start];
        seen[start.0 as usize] = true;
        let mut at = 0;
        let mut boundary = false;
        while at < basin.len() {
            let tile = basin[at];
            at += 1;
            let (x, y) = grid.coords(tile);
            boundary |= y == 0 || y + 1 == grid.height() || (!grid.wraps_x() && (x == 0 || x + 1 == grid.width()));
            for next in grid.neighbours(tile) {
                if !seen[next.0 as usize] && grid.is_water(r, next) {
                    seen[next.0 as usize] = true;
                    basin.push(next);
                }
            }
        }
        if !boundary && basin.len() <= 10 {
            for tile in basin {
                grid.terrain[tile.0 as usize] = lake;
            }
        }
    }
}

/// Vertices are the sorted triples of mutually adjacent hexes. Adjacent
/// vertices share one edge, so generated rivers cannot jump across a hex.
pub(super) fn rivers(r: &Ruleset, grid: &mut TileGrid, heights: &[Fixed], seed: u64) {
    let mut triples = BTreeSet::new();
    for a in grid.ids() {
        for b in grid.neighbours(a).filter(|b| *b > a) {
            for c in grid.neighbours(a).filter(|c| *c > b) {
                if grid.direction_to(b, c).is_some() {
                    triples.insert([a, b, c]);
                }
            }
        }
    }
    let vertices: Vec<_> = triples.into_iter().collect();
    let mut edges: BTreeMap<(TileId, TileId), Vec<usize>> = BTreeMap::new();
    for (v, [a, b, c]) in vertices.iter().copied().enumerate() {
        for pair in [(a, b), (a, c), (b, c)] {
            edges.entry(pair).or_default().push(v);
        }
    }
    let mut neighbours = vec![Vec::new(); vertices.len()];
    for (&edge, ends) in &edges {
        if let [a, b] = ends.as_slice() {
            neighbours[*a].push((*b, edge));
            neighbours[*b].push((*a, edge));
        }
    }
    let elevation: Vec<i64> =
        vertices.iter().map(|v| v.iter().map(|t| heights[t.0 as usize].raw()).sum::<i64>() / 3).collect();
    let outlet: Vec<bool> = vertices.iter().map(|v| v.iter().any(|t| grid.is_water(r, *t))).collect();
    // Priority flood gives every inland vertex a draining parent, including
    // depressions. The secondary distance ensures parent chains never cycle.
    let mut best = vec![(i64::MAX, u32::MAX); vertices.len()];
    let mut parent = vec![None; vertices.len()];
    let mut queue = BinaryHeap::new();
    for v in 0..vertices.len() {
        if outlet[v] {
            best[v] = (elevation[v], 0);
            queue.push(Reverse((best[v], v)));
        }
    }
    while let Some(Reverse((cost, v))) = queue.pop() {
        if best[v] != cost {
            continue;
        }
        for &(next, edge) in &neighbours[v] {
            let candidate = (cost.0.max(elevation[next]), cost.1 + 1);
            if candidate < best[next] {
                best[next] = candidate;
                parent[next] = Some((v, edge));
                queue.push(Reverse((candidate, next)));
            }
        }
    }
    let mut sources: Vec<_> =
        (0..vertices.len()).filter(|&v| !outlet[v] && best[v].1 >= 3 && parent[v].is_some()).collect();
    sources.sort_by_key(|&v| (Reverse(elevation[v]), v));
    let mut chosen = Vec::new();
    let wanted = (grid.len() / 100).max(1);
    for source in sources {
        if chosen.len() >= wanted {
            break;
        }
        let tile = vertices[source][0];
        if chosen.iter().any(|&other| grid.distance(tile, other) < 4) {
            continue;
        }
        if Rng::from_parts(&[seed, 0x52495645, source as u64]).range(0, 2) != 0 {
            continue;
        }
        chosen.push(tile);
        let mut at = source;
        while !outlet[at] {
            let Some((next, (a, b))) = parent[at] else {
                break;
            };
            if grid.has_river_between(a, b) {
                break;
            }
            grid.set_river_between(a, b);
            at = next;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enclosed_small_water_becomes_a_lake_but_open_seas_do_not() {
        let r = Ruleset::load_dir("../android/assets/jsons/Civ V - Gods & Kings").unwrap().unwrap();
        let mut grid = TileGrid::new(10, 10, r.terrain("Grassland").unwrap());
        let enclosed = grid.id_at(4, 4).unwrap();
        let sea = grid.id_at(0, 4).unwrap();
        for tile in [enclosed, sea] {
            grid.terrain[tile.0 as usize] = r.terrain("Coast").unwrap();
        }
        lakes(&r, &mut grid);
        assert_eq!(r.terrains.name_of(grid.terrain[enclosed.0 as usize]), Some("Lakes"));
        assert_eq!(r.terrains.name_of(grid.terrain[sea.0 as usize]), Some("Coast"));
        assert!(grid.neighbours(enclosed).all(|tile| grid.is_fresh_water(&r, tile)));
    }

    #[test]
    fn generated_rivers_are_deterministic_symmetric_and_reach_water() {
        let r = Ruleset::load_dir("../android/assets/jsons/Civ V - Gods & Kings").unwrap().unwrap();
        for wrap_x in [false, true] {
            let p = crate::world::mapgen::MapParameters { seed: 16, wrap_x, ..Default::default() };
            let a = crate::world::mapgen::generate(&r, p, 4);
            let b = crate::world::mapgen::generate(&r, p, 4);
            assert_eq!(a, b);
            let grid = &a.grid;
            assert!(grid.ids().any(|tile| grid.has_river(tile)));
            assert!(grid.ids().any(|tile| grid.has_river(tile) && grid.neighbours(tile).any(|t| grid.is_water(&r, t))));
            for tile in grid.ids() {
                for next in grid.neighbours(tile) {
                    assert_eq!(grid.has_river_between(tile, next), grid.has_river_between(next, tile));
                }
            }
        }
    }
}
