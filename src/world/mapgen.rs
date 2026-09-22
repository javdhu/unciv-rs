//! Making a map from a seed.
//!
//! This is the one system with no reference obligation. Unciv's generator is
//! an answer, not a specification, and a differential harness has nothing to
//! say about whether a continent is in the right place. What it *can* say —
//! and what matters — is that the same seed gives the same map everywhere.
//!
//! # The one hard constraint
//!
//! Generation must be a pure function of the seed, or a saved game cannot be
//! regenerated from it and two clients cannot agree on the world they are
//! playing in. That rules out floating point, because `sin` and `powf` differ
//! between platforms in the last mantissa bit — measured on 3 of 199 inputs
//! between macOS libm and glibc. Everything here is integer and fixed point,
//! and the noise is hashed rather than trigonometric.
//!
//! # How the map is built
//!
//! Three scalar fields over the map, each fractal value noise:
//!
//! * **elevation** decides sea, land and mountain,
//! * **temperature**, which is mostly latitude with noise on top, decides
//!   snow, tundra and desert,
//! * **humidity** decides grassland against plains, and where forest grows.
//!
//! Terrain is then a lookup on the three. This is the standard approach and it
//! is standard because it produces recognisable geography — deserts in bands
//! either side of the equator, ice at the poles, forest where it is warm and
//! wet — from three cheap fields rather than from a simulation.

use crate::arena::Id;
use crate::ruleset::{Ruleset, Terrain};
use crate::rng::Rng;
use crate::stats::Fixed;
use crate::world::grid::{TileGrid, TileId};

/// What kind of world to make.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MapParameters {
    pub width: u16,
    pub height: u16,
    pub seed: u64,
    /// Fraction of the map below sea level, in thousandths. Higher makes
    /// islands, lower makes a supercontinent.
    pub water: Fixed,
    /// How large the features are. A bigger number means fewer, broader
    /// landmasses; a smaller one means a scatter of islands.
    pub landmass_scale: i64,
    /// Roughly one resource per this many land tiles.
    pub resource_sparsity: i64,
}

impl Default for MapParameters {
    fn default() -> MapParameters {
        MapParameters {
            width: 40,
            height: 26,
            seed: 0,
            // Earth is 71% water; a Civ map wants rather less, because ocean
            // tiles are mostly somewhere to not be.
            water: Fixed::permille(420),
            landmass_scale: 9,
            resource_sparsity: 14,
        }
    }
}

/// A generated world: the grid, and where the players start.
#[derive(Debug, Clone, PartialEq)]
pub struct GeneratedMap {
    pub grid: TileGrid,
    /// One per civ asked for, in order, far apart and on decent ground.
    pub starts: Vec<TileId>,
}

// ---------------------------------------------------------------------------
// Noise
// ---------------------------------------------------------------------------

/// A value in `[0, 1)` for a lattice point, from the seed alone.
///
/// Hashed rather than looked up in a permutation table, so there is no state
/// to initialise and no table to get wrong — the same four numbers always give
/// the same result, on any machine.
fn lattice(seed: u64, octave: u32, x: i64, y: i64) -> Fixed {
    Rng::from_parts(&[seed, octave as u64, x as u64, y as u64]).unit_interval()
}

/// `t*t*(3 - 2t)`: an interpolation curve with zero slope at both ends, so
/// neighbouring lattice cells meet without a visible crease.
fn smoothstep(t: Fixed) -> Fixed {
    (t * t) * (Fixed::whole(3) - (Fixed::whole(2) * t))
}

fn lerp(a: Fixed, b: Fixed, t: Fixed) -> Fixed {
    a + ((b - a) * t)
}

/// Value noise at one frequency: interpolate the four lattice corners around
/// this point.
fn value_noise(seed: u64, octave: u32, x: i64, y: i64, period: i64) -> Fixed {
    let period = period.max(1);
    let (cx, cy) = (x.div_euclid(period), y.div_euclid(period));
    let (fx, fy) = (x.rem_euclid(period), y.rem_euclid(period));
    let tx = smoothstep(Fixed::whole(fx as i32).checked_div(Fixed::whole(period as i32)).unwrap_or(Fixed::ZERO));
    let ty = smoothstep(Fixed::whole(fy as i32).checked_div(Fixed::whole(period as i32)).unwrap_or(Fixed::ZERO));

    let top = lerp(lattice(seed, octave, cx, cy), lattice(seed, octave, cx + 1, cy), tx);
    let bottom =
        lerp(lattice(seed, octave, cx, cy + 1), lattice(seed, octave, cx + 1, cy + 1), tx);
    lerp(top, bottom, ty)
}

/// Several octaves summed, each at half the amplitude and half the wavelength
/// of the last. One octave is smooth blobs; four is a coastline.
fn fractal(seed: u64, x: i64, y: i64, base_period: i64) -> Fixed {
    let mut total = Fixed::ZERO;
    let mut amplitude = Fixed::ONE;
    let mut sum = Fixed::ZERO;
    let mut period = base_period.max(1);
    for octave in 0..4u32 {
        total += value_noise(seed, octave, x, y, period) * amplitude;
        sum += amplitude;
        amplitude = amplitude.checked_div(Fixed::whole(2)).unwrap_or(Fixed::ZERO);
        period = (period / 2).max(1);
    }
    total.checked_div(sum).unwrap_or(Fixed::ZERO)
}

// ---------------------------------------------------------------------------
// Generation
// ---------------------------------------------------------------------------

/// Look a terrain up by name, or fall back to the first of its type. A mod
/// that renames Grassland should still produce a map.
fn terrain_or_any(r: &Ruleset, name: &str, kind: &str) -> Option<Id<Terrain>> {
    r.terrains
        .id(name)
        .or_else(|| r.terrains.iter().find(|(_, t)| t.r#type == kind).map(|(id, _)| id))
}

/// Build a world from a seed.
///
/// The same parameters always give the same map — that is the whole contract,
/// and [`same_seed_same_map`] is the test that holds it.
pub fn generate(r: &Ruleset, p: MapParameters, civs: usize) -> GeneratedMap {
    let ocean = terrain_or_any(r, "Ocean", "Water").expect("a ruleset needs water");
    let coast = terrain_or_any(r, "Coast", "Water").unwrap_or(ocean);
    let grassland = terrain_or_any(r, "Grassland", "Land").expect("a ruleset needs land");
    let plains = terrain_or_any(r, "Plains", "Land").unwrap_or(grassland);
    let desert = terrain_or_any(r, "Desert", "Land").unwrap_or(plains);
    let tundra = terrain_or_any(r, "Tundra", "Land").unwrap_or(plains);
    let snow = terrain_or_any(r, "Snow", "Land").unwrap_or(tundra);
    let mountain = terrain_or_any(r, "Mountain", "Land").unwrap_or(grassland);

    let mut grid = TileGrid::new(p.width, p.height, grassland);

    // Pass 1: elevation everywhere, then a sea level chosen from the
    // *distribution* rather than fixed.
    //
    // A fixed threshold on noise gives whatever water fraction that seed
    // happens to produce — measured between 42% and 84% across eight seeds,
    // which is the difference between a continent and an archipelago nobody
    // asked for. Taking the quantile instead gives the asked-for fraction on
    // every seed, and costs one sort of a few thousand numbers.
    let mut elevation = vec![Fixed::ZERO; grid.len()];
    for tile in grid.ids() {
        let (col, row) = grid.coords(tile);
        elevation[tile.0 as usize] = shaped_elevation(&p, &grid, col as i64, row as i64);
    }
    let sea_level = {
        let mut sorted = elevation.clone();
        sorted.sort();
        let cut = (sorted.len() as i64 * p.water.raw() / 1000).clamp(0, sorted.len() as i64 - 1);
        sorted[cut as usize]
    };

    for tile in grid.ids() {
        let (col, row) = grid.coords(tile);
        let (x, y) = (col as i64, row as i64);
        let e = elevation[tile.0 as usize];

        if e < sea_level {
            grid.terrain[tile.0 as usize] = ocean;
            continue;
        }
        let temperature = temperature_at(&p, &grid, x, y);
        let humidity = fractal(p.seed ^ 0x48554d49, x, y, p.landmass_scale);

        grid.terrain[tile.0 as usize] = if e > high_ground(sea_level) {
            mountain
        } else if temperature < Fixed::permille(150) {
            snow
        } else if temperature < Fixed::permille(300) {
            tundra
        } else if humidity < Fixed::permille(280) && temperature > Fixed::permille(550) {
            desert
        } else if humidity < Fixed::permille(500) {
            plains
        } else {
            grassland
        };
    }

    // Pass 2: water next to land is coast, not open ocean. Needs every tile's
    // terrain settled first, so it cannot be folded into pass 1.
    let is_water = |g: &TileGrid, t: TileId| {
        r.terrains.get(g.terrain[t.0 as usize]).is_some_and(|x| x.r#type == "Water")
    };
    let shallow: Vec<TileId> = grid
        .ids()
        .filter(|t| is_water(&grid, *t) && grid.neighbours(*t).any(|n| !is_water(&grid, n)))
        .collect();
    for t in shallow {
        grid.terrain[t.0 as usize] = coast;
    }

    place_features(r, &mut grid, &p, &elevation);
    place_resources(r, &mut grid, &p);
    let starts = choose_starts(r, &grid, &p, civs);
    place_camps(r, &mut grid, &p, &starts);
    GeneratedMap { grid, starts }
}

/// Where mountains start, as a band above sea level rather than an absolute
/// height — sea level moves with the seed, and mountains should follow it.
fn high_ground(sea_level: Fixed) -> Fixed {
    sea_level + ((Fixed::ONE - sea_level) * Fixed::permille(780))
}

/// Elevation, pulled down towards the east and west edges so land does not run
/// off the side of the map.
///
/// Only east and west. Pulling the north and south edges down too drowns the
/// poles, and then there is no polar land for the temperature bands to put ice
/// on — the first draft produced a world with no snow anywhere, because
/// everywhere cold enough for it was underwater.
fn shaped_elevation(p: &MapParameters, grid: &TileGrid, x: i64, y: i64) -> Fixed {
    let raw = fractal(p.seed, x, y, p.landmass_scale);
    let margin = x.min(grid.width() as i64 - 1 - x);
    let reach = (grid.width() as i64 / 6).max(1);
    let near = margin.min(reach);
    let falloff = Fixed::whole(near as i32).checked_div(Fixed::whole(reach as i32)).unwrap_or(Fixed::ONE);
    raw * falloff
}

/// Warm at the equator, cold at the poles, with noise so the bands are not
/// stripes.
fn temperature_at(p: &MapParameters, grid: &TileGrid, x: i64, y: i64) -> Fixed {
    let half = (grid.height() as i64).max(2) / 2;
    let from_equator = (y - half).abs().min(half);
    let latitude = Fixed::ONE
        - Fixed::whole(from_equator as i32).checked_div(Fixed::whole(half as i32)).unwrap_or(Fixed::ZERO);
    // Three parts latitude to one part noise: enough to bend the bands without
    // putting a desert at the pole.
    let noise = fractal(p.seed ^ 0x54454d50, x, y, p.landmass_scale.max(2) / 2);
    (latitude * Fixed::whole(3) + noise).checked_div(Fixed::whole(4)).unwrap_or(latitude)
}

/// Hills, forest and jungle, each where its own ruleset says it can go.
fn place_features(r: &Ruleset, grid: &mut TileGrid, p: &MapParameters, elevation: &[Fixed]) {
    let feature = |name: &str| {
        r.terrains.id(name).filter(|id| {
            r.terrains.get(*id).is_some_and(|t| t.r#type == "TerrainFeature")
        })
    };
    let hill = feature("Hill");
    let forest = feature("Forest");
    let jungle = feature("Jungle");

    for tile in grid.ids().collect::<Vec<_>>() {
        let i = tile.0 as usize;
        let Some(base) = r.terrains.get(grid.terrain[i]) else { continue };
        if base.r#type != "Land" {
            continue;
        }
        let base_name = r.terrains.name_of(grid.terrain[i]).unwrap_or_default().to_string();
        let (col, row) = grid.coords(tile);
        let (x, y) = (col as i64, row as i64);

        // Hills follow elevation: the shoulders of the mountains, not a
        // scatter, which is what makes a range look like a range.
        if let Some(hill) = hill {
            let e = elevation[i];
            if e > Fixed::permille(700)
                && e <= Fixed::permille(880)
                && occurs_on(r, hill, &base_name)
            {
                grid.features[i].push(hill);
                continue;
            }
        }

        let humidity = fractal(p.seed ^ 0x48554d49, x, y, p.landmass_scale);
        let temperature = temperature_at(p, grid, x, y);
        let wood = if temperature > Fixed::permille(720) { jungle } else { forest };
        if let Some(wood) = wood {
            if humidity > Fixed::permille(620) && occurs_on(r, wood, &base_name) {
                grid.features[i].push(wood);
            }
        }
    }
}

/// Does this feature or resource belong on that base terrain? An empty list
/// means the ruleset did not say, and we do not place it rather than guessing.
fn occurs_on(r: &Ruleset, feature: Id<Terrain>, base: &str) -> bool {
    r.terrains.get(feature).is_some_and(|t| t.occurs_on.iter().any(|o| o == base))
}

/// Scatter resources on the terrain each one is allowed on.
///
/// Every tile is offered a resource with a fixed chance, and the resource is
/// drawn from those the tile could carry. Doing it per tile rather than per
/// resource means a map with a lot of desert has a lot of what grows in
/// desert, without anything having to know that.
fn place_resources(r: &Ruleset, grid: &mut TileGrid, p: &MapParameters) {
    let sparsity = p.resource_sparsity.max(1);
    for tile in grid.ids().collect::<Vec<_>>() {
        let i = tile.0 as usize;
        let mut rng = Rng::from_parts(&[p.seed, 0x5245534f, tile.0 as u64]);
        // `range` is inclusive at both ends, so this is one chance in
        // `sparsity`, not one in `sparsity + 1`.
        if rng.range(1, sparsity) != 1 {
            continue;
        }
        let names: Vec<String> = std::iter::once(grid.terrain[i])
            .chain(grid.features[i].iter().copied())
            .filter_map(|t| r.terrains.name_of(t).map(|s| s.to_string()))
            .collect();

        // In id order, so the candidate list does not depend on how the
        // ruleset happened to be iterated.
        let candidates: Vec<_> = r
            .resources
            .iter()
            .filter(|(_, res)| {
                res.terrains_can_be_found_on.iter().any(|t| names.iter().any(|n| n == t))
            })
            .collect();
        if candidates.is_empty() {
            continue;
        }
        let pick = rng.range(0, candidates.len() as i64 - 1) as usize;
        let (id, res) = &candidates[pick];
        grid.resource[i] = Some(*id);
        // Strategic resources come in deposits; everything else is one.
        grid.resource_amount[i] = if res.resource_type == "Strategic" {
            res.major_deposit_amount.values().copied().max().unwrap_or(2).max(1)
        } else {
            1
        };
    }
}

/// Scatter barbarian encampments over the empty parts of the map.
///
/// Camps have to exist before anything can come out of them, and nothing
/// placed any: `barbarians::place_camp` was as unreachable as
/// `barbarians::spawn` itself. A map with no camps and a spawner that is never
/// called are two halves of the same missing system.
///
/// Kept away from the starting positions, because a camp two tiles from a
/// capital on turn one is not pressure, it is a coin flip.
fn place_camps(r: &Ruleset, grid: &mut TileGrid, p: &MapParameters, starts: &[TileId]) {
    let Some(camp) = crate::world::barbarians::camp_improvement(r) else { return };
    let clearance = 6;
    for tile in grid.ids().collect::<Vec<_>>() {
        let i = tile.0 as usize;
        if grid.improvement[i].is_some() || grid.owner[i].is_some() {
            continue;
        }
        let Some(t) = r.terrains.get(grid.terrain[i]) else { continue };
        if t.r#type != "Land" || t.impassable == Some(true) {
            continue;
        }
        if starts.iter().any(|s| grid.distance(*s, tile) < clearance) {
            continue;
        }
        let mut rng = Rng::from_parts(&[p.seed, 0xCA_47, tile.0 as u64]);
        // Roughly one tile in thirty-five, which on a standard map is a
        // dozen-odd camps — enough to be a nuisance, not a siege.
        if rng.range(1, 35) == 1 {
            grid.improvement[i] = Some(camp);
        }
    }
}

/// Where the players begin.
///
/// Scored on what a city there would actually have — food and production in
/// the inner ring — and then spread out, taking the best remaining site that
/// is far enough from every site already taken. Picking the best sites first
/// and spacing second would put two civs in the same river valley.
fn choose_starts(r: &Ruleset, grid: &TileGrid, p: &MapParameters, civs: usize) -> Vec<TileId> {
    if civs == 0 {
        return Vec::new();
    }
    let passable = |t: TileId| crate::world::movement::is_passable(grid, r, t);
    let land = |t: TileId| {
        r.terrains.get(grid.terrain[t.0 as usize]).is_some_and(|x| x.r#type == "Land")
    };

    let mut scored: Vec<(i64, TileId)> = grid
        .ids()
        .filter(|t| land(*t) && passable(*t))
        .map(|t| {
            let here = crate::world::yields::tile_yield(grid, r, t);
            let ring: Fixed = grid
                .neighbours(t)
                .map(|n| {
                    let y = crate::world::yields::tile_yield(grid, r, n);
                    y.food + y.production
                })
                .sum();
            // Food and hammers only: a start is about whether the city lives,
            // not whether it is rich.
            let score = (here.food + here.production) * Fixed::whole(2) + ring;
            (score.raw(), t)
        })
        .collect();
    // Best first, ties by tile id so the choice does not depend on iteration.
    scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1 .0.cmp(&b.1 .0)));

    // Start generous and relax until everyone fits — a crowded map should
    // still produce a game rather than no starts at all.
    let mut spacing = (grid.width().min(grid.height()) as u32 / 2).max(2);
    loop {
        let mut chosen: Vec<TileId> = Vec::new();
        for (_, t) in &scored {
            if chosen.len() == civs {
                break;
            }
            if chosen.iter().all(|c| grid.distance(*c, *t) >= spacing) {
                chosen.push(*t);
            }
        }
        if chosen.len() == civs || spacing <= 1 {
            let _ = p;
            if chosen.len() < civs {
                // A map too small to space everyone out still has to produce a
                // game. Fill from the best remaining ground, wherever it is.
                for (_, t) in &scored {
                    if chosen.len() == civs {
                        break;
                    }
                    if !chosen.contains(t) {
                        chosen.push(*t);
                    }
                }
            }
            return chosen;
        }
        spacing -= 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ruleset() -> Ruleset {
        Ruleset::load_dir("../android/assets/jsons/Civ V - Gods & Kings")
            .expect("readable")
            .expect("loads")
    }

    fn counts(r: &Ruleset, m: &GeneratedMap) -> std::collections::BTreeMap<String, usize> {
        let mut out = std::collections::BTreeMap::new();
        for t in m.grid.ids() {
            let name = r.terrains.name_of(m.grid.terrain[t.0 as usize]).unwrap_or("?").to_string();
            *out.entry(name).or_insert(0) += 1;
        }
        out
    }

    /// The whole contract.
    #[test]
    fn the_same_seed_gives_the_same_map() {
        let r = ruleset();
        let p = MapParameters { seed: 0xC1F5, ..Default::default() };
        let a = generate(&r, p, 4);
        let b = generate(&r, p, 4);
        assert_eq!(a, b);
    }

    #[test]
    fn a_different_seed_gives_a_different_map() {
        let r = ruleset();
        let a = generate(&r, MapParameters { seed: 1, ..Default::default() }, 4);
        let b = generate(&r, MapParameters { seed: 2, ..Default::default() }, 4);
        assert_ne!(a.grid.terrain, b.grid.terrain);
    }

    #[test]
    fn a_map_has_land_and_sea_in_roughly_the_asked_proportion() {
        let r = ruleset();
        for seed in 0..8u64 {
            let m = generate(&r, MapParameters { seed, ..Default::default() }, 4);
            let water = m
                .grid
                .ids()
                .filter(|t| {
                    r.terrains
                        .get(m.grid.terrain[t.0 as usize])
                        .is_some_and(|x| x.r#type == "Water")
                })
                .count();
            let fraction = water * 100 / m.grid.len();
            assert!(
                (25..=80).contains(&fraction),
                "seed {seed}: {fraction}% water is not a world worth playing on"
            );
        }
    }

    #[test]
    fn the_poles_are_cold_and_the_equator_is_not() {
        let r = ruleset();
        let m = generate(&r, MapParameters { seed: 7, height: 30, ..Default::default() }, 4);
        let name_at = |col: u16, row: u16| {
            m.grid
                .id_at(col, row)
                .and_then(|t| r.terrains.name_of(m.grid.terrain[t.0 as usize]))
                .unwrap_or("?")
                .to_string()
        };
        let mut cold_at_pole = 0;
        let mut cold_at_equator = 0;
        for col in 0..m.grid.width() {
            for row in [0u16, 1, m.grid.height() - 2, m.grid.height() - 1] {
                if matches!(name_at(col, row).as_str(), "Snow" | "Tundra") {
                    cold_at_pole += 1;
                }
            }
            let mid = m.grid.height() / 2;
            for row in [mid - 1, mid, mid + 1] {
                if matches!(name_at(col, row).as_str(), "Snow" | "Tundra") {
                    cold_at_equator += 1;
                }
            }
        }
        assert!(
            cold_at_pole > cold_at_equator,
            "{cold_at_pole} cold tiles at the poles against {cold_at_equator} at the equator"
        );
    }

    #[test]
    fn a_map_is_more_than_one_kind_of_ground() {
        let r = ruleset();
        let m = generate(&r, MapParameters { seed: 3, ..Default::default() }, 4);
        let kinds = counts(&r, &m);
        assert!(kinds.len() >= 4, "only {} kinds of terrain: {kinds:?}", kinds.len());
        let features =
            m.grid.features.iter().filter(|f| !f.is_empty()).count();
        assert!(features > 0, "a map with no hills and no forest is a table, not a world");
    }

    #[test]
    fn resources_land_only_where_the_ruleset_allows_them() {
        let r = ruleset();
        let m = generate(&r, MapParameters { seed: 11, ..Default::default() }, 4);
        let mut found = 0;
        for t in m.grid.ids() {
            let Some(res) = m.grid.resource[t.0 as usize] else { continue };
            found += 1;
            let res = r.resources.get(res).expect("a real resource");
            let names: Vec<&str> = std::iter::once(m.grid.terrain[t.0 as usize])
                .chain(m.grid.features[t.0 as usize].iter().copied())
                .filter_map(|x| r.terrains.name_of(x))
                .collect();
            assert!(
                res.terrains_can_be_found_on.iter().any(|a| names.contains(&a.as_str())),
                "{} on {names:?}",
                res.name
            );
        }
        assert!(found > 10, "only {found} resources on a whole map");
    }

    #[test]
    fn everyone_gets_a_start_and_nobody_shares_one() {
        let r = ruleset();
        for civs in [2usize, 4, 8] {
            let m = generate(&r, MapParameters { seed: 5, ..Default::default() }, civs);
            assert_eq!(m.starts.len(), civs, "{civs} civs should get {civs} starts");
            let unique: std::collections::BTreeSet<_> = m.starts.iter().collect();
            assert_eq!(unique.len(), civs, "two civs were put on the same tile");
            for t in &m.starts {
                let terrain = r.terrains.get(m.grid.terrain[t.0 as usize]).unwrap();
                assert_eq!(terrain.r#type, "Land", "a start in the sea");
                assert!(
                    crate::world::movement::is_passable(&m.grid, &r, *t),
                    "a start on impassable ground"
                );
            }
        }
    }

    #[test]
    fn starts_are_spread_out_rather_than_huddled_on_the_best_ground() {
        let r = ruleset();
        let m = generate(&r, MapParameters { seed: 9, ..Default::default() }, 4);
        for (i, a) in m.starts.iter().enumerate() {
            for b in &m.starts[i + 1..] {
                assert!(
                    m.grid.distance(*a, *b) >= 4,
                    "two starts {} tiles apart",
                    m.grid.distance(*a, *b)
                );
            }
        }
    }

    #[test]
    fn a_small_crowded_map_still_produces_a_game() {
        let r = ruleset();
        // Eight civs on a map with barely room: the spacing has to relax
        // rather than return nothing.
        let m = generate(&r, MapParameters { width: 16, height: 12, seed: 4, ..Default::default() }, 8);
        assert_eq!(m.starts.len(), 8);
    }

    #[test]
    fn the_noise_stays_inside_its_range() {
        // Everything downstream classifies on thresholds between 0 and 1. A
        // field that escaped that range would put snow in the tropics.
        for seed in 0..4u64 {
            for x in 0..40i64 {
                for y in 0..26i64 {
                    let n = fractal(seed, x, y, 9);
                    assert!(n >= Fixed::ZERO && n <= Fixed::ONE, "{n:?} at {x},{y}");
                }
            }
        }
    }
}
