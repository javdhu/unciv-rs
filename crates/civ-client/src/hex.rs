//! Hex geometry for civ-core's grid (docs/VISUAL-SPEC.md, World geometry).
//!
//! Pointy-top hexes of circumradius 1 m, odd rows shifted east (odd-r), `id = row * width + col`, row 0 at
//! the north edge. World axes: +X east, +Y up, −Z north. Directions are civ-core's, in its order: E, NE, NW,
//! W, SW, SE; direction `d` points at angle 60°·d from east, counter-clockwise seen from above.

use bevy::math::{Vec2, Vec3};

/// Circumradius.
pub const R: f32 = 1.0;
pub const SQRT3: f32 = 1.732_050_8;
/// Distance between neighbouring centres.
pub const PITCH: f32 = SQRT3 * R;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Grid {
    pub width: u16,
    pub height: u16,
    pub wrap_x: bool,
}

/// Ground-plane point: x east, y north (world −Z).
pub fn ground(world: Vec3) -> Vec2 {
    Vec2::new(world.x, -world.z)
}

pub fn world(ground: Vec2, height: f32) -> Vec3 {
    Vec3::new(ground.x, height, -ground.y)
}

/// Unit vector of direction `d` on the ground plane.
pub fn direction(d: usize) -> Vec2 {
    let a = (60.0 * d as f32).to_radians();
    Vec2::new(a.cos(), a.sin())
}

/// Corner between directions `d` and `d + 1`, relative to the centre.
pub fn corner(d: usize) -> Vec2 {
    let a = (60.0 * d as f32 + 30.0).to_radians();
    Vec2::new(a.cos(), a.sin()) * R
}

impl Grid {
    pub fn len(&self) -> usize {
        self.width as usize * self.height as usize
    }

    pub fn coords(&self, id: u16) -> (u16, u16) {
        (id % self.width, id / self.width)
    }

    pub fn id(&self, col: i32, row: i32) -> Option<u16> {
        let col = if self.wrap_x { col.rem_euclid(self.width as i32) } else { col };
        (col >= 0 && row >= 0 && col < self.width as i32 && row < self.height as i32)
            .then(|| (row * self.width as i32 + col) as u16)
    }

    /// Centre on the ground plane, unwrapped (columns 0..width).
    pub fn centre(&self, id: u16) -> Vec2 {
        let (col, row) = self.coords(id);
        Vec2::new(PITCH * (col as f32 + 0.5 * (row % 2) as f32), -1.5 * R * row as f32)
    }

    /// The neighbour in direction `d`, as civ-core computes it (`Grid::compute_neighbours`).
    pub fn neighbour(&self, id: u16, d: usize) -> Option<u16> {
        let (col, row) = self.coords(id);
        let odd = row % 2 == 1;
        let (dc, dr) = match (d % 6, odd) {
            (0, _) => (1, 0),
            (1, true) => (1, -1),
            (1, false) => (0, -1),
            (2, true) => (0, -1),
            (2, false) => (-1, -1),
            (3, _) => (-1, 0),
            (4, true) => (0, 1),
            (4, false) => (-1, 1),
            (5, true) => (1, 1),
            _ => (0, 1),
        };
        self.id(col as i32 + dc, row as i32 + dr)
    }

    /// Centre of the neighbour in direction `d` as seen from `id`: across a wrap seam it lies beside `id`,
    /// not at its own unwrapped column.
    pub fn neighbour_centre(&self, id: u16, d: usize) -> Vec2 {
        self.centre(id) + direction(d) * PITCH
    }

    /// Map extent on the ground plane: west, east, south, north edges, including hex overhang.
    pub fn bounds(&self) -> (f32, f32, f32, f32) {
        let east = PITCH * (self.width as f32 + 0.5);
        let south = -1.5 * R * (self.height as f32 - 1.0) - R;
        (-PITCH / 2.0, east - PITCH / 2.0, south, R)
    }

    /// Width of one wrap of the map.
    pub fn span(&self) -> f32 {
        PITCH * self.width as f32
    }

    /// The tile under a ground point (columns wrapped when the map wraps), or none off the map.
    pub fn tile_at(&self, p: Vec2) -> Option<u16> {
        // Axial coordinates of a pointy-top layout, rounded in cube space.
        let q = (SQRT3 / 3.0 * p.x - 1.0 / 3.0 * -p.y) / R;
        let r = (2.0 / 3.0 * -p.y) / R;
        let (x, z) = (q, r);
        let y = -x - z;
        let (mut rx, ry, mut rz) = (x.round(), y.round(), z.round());
        let (dx, dy, dz) = ((rx - x).abs(), (ry - y).abs(), (rz - z).abs());
        if dx > dy && dx > dz {
            rx = -ry - rz;
        } else if dy <= dz {
            rz = -rx - ry;
        }
        let row = rz as i32;
        let col = rx as i32 + (row - (row & 1)) / 2;
        self.id(col, row)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GRID: Grid = Grid { width: 12, height: 8, wrap_x: false };

    #[test]
    fn neighbours_sit_one_pitch_away_in_their_direction() {
        for id in 0..GRID.len() as u16 {
            for d in 0..6 {
                if let Some(n) = GRID.neighbour(id, d) {
                    let offset = GRID.centre(n) - GRID.centre(id);
                    assert!((offset - direction(d) * PITCH).length() < 1e-4, "{id} dir {d} -> {n}");
                }
            }
        }
    }

    #[test]
    fn every_centre_and_point_near_it_picks_its_own_tile() {
        for id in 0..GRID.len() as u16 {
            let c = GRID.centre(id);
            assert_eq!(GRID.tile_at(c), Some(id));
            for d in 0..6 {
                assert_eq!(GRID.tile_at(c + corner(d) * 0.95), Some(id), "{id} corner {d}");
            }
        }
    }

    #[test]
    fn wrapped_neighbours_cross_the_seam() {
        let grid = Grid { wrap_x: true, ..GRID };
        let east_edge = grid.id(11, 2).unwrap();
        assert_eq!(grid.neighbour(east_edge, 0), grid.id(0, 2));
        assert_eq!(GRID.neighbour(east_edge, 0), None);
    }
}
