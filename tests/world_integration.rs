//! The ruleset and the map meeting: a grid built from real terrain, with real
//! sight radii, at the size Civ actually plays at.

use civ_core::ruleset::Ruleset;
use civ_core::world::{CivId, TileGrid, TileId, Visibility};

const GK: &str = "../android/assets/jsons/Civ V - Gods & Kings";

fn ruleset() -> Ruleset {
    Ruleset::load_dir(GK).expect("readable").expect("loads")
}

#[test]
fn a_full_size_map_of_real_terrain() {
    let r = ruleset();
    let grassland = r.terrain("Grassland").expect("Grassland exists");

    // Civ5Huge — the largest standard map.
    let g = TileGrid::new(128, 80, grassland);
    assert_eq!(g.len(), 10_240);
    assert!(g.len() <= u16::MAX as usize, "TileId must index the largest map");

    // Every tile resolves back to a real ruleset terrain.
    for t in g.ids().take(100) {
        assert_eq!(r.terrains.name_of(g.terrain[t.0 as usize]), Some("Grassland"));
    }
}

#[test]
fn sight_from_a_unit_covers_the_expected_ring() {
    let r = ruleset();
    let g = TileGrid::new(128, 80, r.terrain("Grassland").unwrap());
    let mut v = Visibility::new(g.len(), 8);
    let civ = CivId(0);

    // A scout sees two tiles out: 1 + 3r(r+1) = 19.
    let unit_at = g.id_at(64, 40).unwrap();
    for t in g.within(unit_at, 2) {
        v.observe(civ, t);
    }
    assert_eq!(v.explored_count(civ), 19);
    assert!(v.can_see(civ, unit_at));

    // Beyond the radius, nothing.
    let far = g.id_at(10, 10).unwrap();
    assert!(!v.can_see(civ, far));
    assert!(!v.has_explored(civ, far));
}

#[test]
fn eight_civs_on_a_huge_map_cost_twenty_kilobytes() {
    let r = ruleset();
    let g = TileGrid::new(128, 80, r.terrain("Grassland").unwrap());
    let v = Visibility::new(g.len(), 8);
    // The whole point of inverting Unciv's per-tile HashSet<String>.
    assert!(v.mask_bytes() < 21_000, "{} bytes", v.mask_bytes());
}

#[test]
fn neighbours_stay_symmetric_at_full_size() {
    let r = ruleset();
    let g = TileGrid::new(128, 80, r.terrain("Grassland").unwrap());
    // Sample rather than sweep 10,240 tiles; both row parities, both edges.
    for t in (0..g.len()).step_by(37).map(|i| TileId(i as u16)) {
        for n in g.neighbours(t) {
            assert!(g.neighbours(n).any(|x| x == t), "{t:?} -> {n:?} not mutual");
        }
    }
}
