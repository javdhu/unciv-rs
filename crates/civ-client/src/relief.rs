//! The land as functions of ground position (docs/VISUAL-SPEC.md, Terrain surface, Water, Fog).
//!
//! Every height and colour is a pure function of a ground point and the known tiles around it, so two chunks
//! meshing the same point get the same answer and seams close by themselves. A point is weighed over the dual
//! triangle of the three tile centres around it; a tile the view does not hold never contributes (its weight
//! goes to the known tiles), so nothing about unexplored land reaches the screen.

use crate::hex::{self, Grid, PITCH, R};
use crate::rules::{Feature, Ground, Rules};
use bevy::color::{Color, Hsva, LinearRgba, Mix};
use bevy::math::Vec2;
use civ_sim::PlayerView;

pub const SLATE: Color = Color::srgb(0x1E as f32 / 255.0, 0x21 as f32 / 255.0, 0x27 as f32 / 255.0);
const LAND: f32 = 0.10;
const SNOW_LINE: f32 = 0.95;

/// What the map needs of one known tile.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Cell {
    pub ground: Ground,
    pub features: u16,
    pub rivers: u8,
    pub road: u8,
    pub road_pillaged: bool,
    pub improvement: Option<u32>,
    pub improvement_pillaged: bool,
    pub resource: Option<u32>,
    pub owner: Option<u8>,
    pub visible: bool,
    /// Mountain peak height, fixed per tile.
    pub peak: f32,
}

impl Cell {
    pub fn has(&self, f: Feature) -> bool {
        self.features & bit(f) != 0
    }

    pub fn is_water(&self) -> bool {
        matches!(self.ground, Ground::Ocean | Ground::Coast | Ground::Lake)
    }

    fn base(&self) -> f32 {
        match self.ground {
            Ground::Ocean => -0.55,
            Ground::Coast => -0.20,
            Ground::Lake => -0.25,
            _ if self.has(Feature::Marsh) => 0.04,
            _ => LAND,
        }
    }

    fn albedo(&self) -> LinearRgba {
        let hex = match self.ground {
            Ground::Ocean => 0x2D3B44,
            Ground::Coast => 0x9C9472,
            Ground::Lake => 0x5E6A55,
            Ground::Grassland => 0x6F8F3E,
            Ground::Plains => 0xA6A05A,
            Ground::Desert => 0xD9C48E,
            Ground::Tundra => 0x8E8C76,
            Ground::Snow => 0xEDF1F3,
            Ground::Mountain => 0x7C766D,
        };
        let mut c = srgb(hex);
        if self.has(Feature::Fallout) {
            c = c.mix(&srgb(0x7F8A62), 0.6);
        }
        if self.has(Feature::Marsh) {
            c = c.mix(&srgb(0x556F4C), 0.7);
        }
        c
    }
}

pub fn bit(f: Feature) -> u16 {
    1 << f as u16
}

pub fn srgb(hex: u32) -> LinearRgba {
    Color::srgb_u8((hex >> 16) as u8, (hex >> 8) as u8, hex as u8).to_linear()
}

/// Nation colours by civ id: outer, inner.
pub type Liveries = std::collections::HashMap<u8, (LinearRgba, LinearRgba)>;

/// The known world, as the view holds it.
pub struct Relief {
    pub grid: Grid,
    pub cells: Vec<Option<Cell>>,
    pub liveries: Liveries,
}

impl Relief {
    pub fn new(view: &PlayerView, rules: &Rules) -> Relief {
        let grid = Grid { width: view.width, height: view.height, wrap_x: view.wrap_x };
        let mut cells = vec![None; grid.len()];
        for t in &view.tiles {
            let ground = rules.ground(t.terrain);
            let mut features = 0;
            for f in t.features.iter().copied().chain(t.natural_wonder) {
                features |= bit(rules.feature(f));
            }
            if let Some(slot) = cells.get_mut(t.id as usize) {
                *slot = Some(Cell {
                    ground,
                    features,
                    rivers: t.river_edges,
                    road: t.road,
                    road_pillaged: t.road_pillaged,
                    improvement: t.improvement,
                    improvement_pillaged: t.improvement_pillaged,
                    resource: t.resource,
                    owner: t.owner,
                    visible: t.visible,
                    peak: 1.20 + 0.20 * (2.0 * hash1(t.id as u32, 7) - 1.0),
                });
            }
        }
        let liveries = view
            .civs
            .iter()
            .filter_map(|c| rules.nation(c.nation).map(|(_, o, i)| (c.id, (o.to_linear(), i.to_linear()))))
            .collect();
        Relief { grid, cells, liveries }
    }

    pub fn cell(&self, id: u16) -> Option<&Cell> {
        self.cells.get(id as usize).and_then(Option::as_ref)
    }

    /// The tile under a ground point and the dual triangle around the point: three (tile, centre) corners, a
    /// corner `None` off the map, and barycentric weights. Centres are placed relative to the home tile, so
    /// across a wrap seam they sit beside it.
    fn triangle(&self, p: Vec2) -> Option<(u16, [(Option<u16>, Vec2); 3], [f32; 3])> {
        let home = self.grid.tile_at(self.unwrap(p))?;
        let c = self.grid.centre(home);
        // `p` relative to the home centre, in this wrap of the map.
        let mut rel = self.unwrap(p) - c;
        if self.grid.wrap_x {
            let span = self.grid.span();
            rel.x = (rel.x + span / 2.0).rem_euclid(span) - span / 2.0;
        }
        let angle = rel.y.atan2(rel.x).to_degrees().rem_euclid(360.0);
        let k = ((angle / 60.0) as usize).min(5);
        let corners = [
            (Some(home), Vec2::ZERO),
            (self.grid.neighbour(home, k), hex::direction(k) * PITCH),
            (self.grid.neighbour(home, (k + 1) % 6), hex::direction((k + 1) % 6) * PITCH),
        ];
        let weights = barycentric(rel, corners[0].1, corners[1].1, corners[2].1);
        Some((home, corners.map(|(t, v)| (t, v + c)), weights))
    }

    fn unwrap(&self, p: Vec2) -> Vec2 {
        if self.grid.wrap_x {
            Vec2::new(p.x.rem_euclid(self.grid.span()), p.y)
        } else {
            p
        }
    }

    /// The known corners of the triangle around `p`, weights renormalised over them; `None` where the home tile
    /// is not known. One sample serves a vertex's height, colour, water and fade.
    pub fn sample(&self, p: Vec2) -> Option<Sample> {
        let (home, corners, weights) = self.triangle(p)?;
        self.cell(home)?;
        let mut sample = Sample { p, home, corners: [None; 3] };
        let mut total = 0.0;
        for (slot, ((tile, centre), w)) in sample.corners.iter_mut().zip(corners.iter().zip(weights)) {
            if let Some(cell) = tile.and_then(|t| self.cell(t)) {
                let w = w.max(0.0);
                *slot = Some(Corner { cell: *cell, centre: *centre, weight: w });
                total += w;
            }
        }
        if total > 0.0 {
            sample.corners.iter_mut().flatten().for_each(|c| c.weight /= total);
        }
        Some(sample)
    }

    /// Ground height at `p`; `None` where the home tile is not known.
    pub fn height(&self, p: Vec2) -> Option<f32> {
        self.sample(p).map(|s| self.height_of(&s))
    }

    pub fn height_of(&self, s: &Sample) -> f32 {
        let p = s.p;
        let sharp = s.sharpened(3);
        let mut base = 0.0;
        let mut relief = 0.0f32;
        let mut dip = 0.0f32;
        let corners = s.corners;
        for (i, corner) in corners.iter().enumerate() {
            let Some(k) = corner else { continue };
            base += k.cell.base() * sharp[i];
            let d = (p - k.centre).length();
            if k.cell.has(Feature::Hill) {
                relief = relief.max(0.40 * cosine_bump(d / (0.85 * R)));
            }
            if k.cell.ground == Ground::Mountain {
                // Ridged, so a peak reads as rock rather than a cone.
                let crags = 0.8 + 0.4 * ridged(self.unwrap(p) * 2.5);
                relief = relief.max(k.cell.peak * crags * (1.0 - d / (0.70 * R)).max(0.0).powf(1.3));
                // Ranges: neighbouring mountains join along a ridge through their shared edge.
                for other in corners[i + 1..].iter().flatten() {
                    if other.cell.ground == Ground::Mountain {
                        relief = relief.max(ridge(p, k.centre, other.centre, k.cell.peak.min(other.cell.peak)));
                    }
                }
            }
            // Hollows: an oasis pool, marsh pools; and an atoll's ring of sand.
            if k.cell.has(Feature::Oasis) {
                dip = dip.max(0.16 * cosine_bump(d / (0.3 * R)));
            }
            if k.cell.has(Feature::Marsh) {
                let pools = smoothstep(0.55, 0.75, noise(self.unwrap(p) * 3.0));
                dip = dip.max(0.08 * pools * cosine_bump(d / (0.85 * R)));
            }
            if k.cell.has(Feature::Atoll) {
                relief = relief.max(0.25 * cosine_bump((d - 0.5 * R).abs() / 0.1));
            }
        }
        let base = base - dip;
        let land = base.max(0.0) / LAND;
        let rough = if relief > 0.3 { 0.10 } else { 0.04 };
        base + relief + rough * land.min(1.0) * (2.0 * noise(self.unwrap(p) * 2.0) - 1.0)
    }

    /// Linear colour of a sample at `height` with a normal whose upward component is `normal_y`, before the
    /// edge fade.
    pub fn colour_of(&self, s: &Sample, height: f32, normal_y: f32) -> LinearRgba {
        let p = s.p;
        let sharp = s.sharpened(8);
        let mut colour = LinearRgba::NONE;
        for (corner, w) in s.corners.iter().zip(sharp) {
            let Some(k) = corner else { continue };
            let mut c = k.cell.albedo();
            if k.cell.has(Feature::FloodPlains) && near_river(k.centre, p, k.cell.rivers, 0.5) {
                c = srgb(0x93A84E);
            }
            if let Some(owner) = k.cell.owner.filter(|_| !k.cell.is_water()) {
                if let Some((outer, _)) = self.liveries.get(&owner) {
                    c = c.mix(outer, 0.10);
                }
            }
            if !k.cell.visible {
                c = remembered(c);
            }
            colour = colour + c * w;
        }
        let mut colour = LinearRgba { alpha: 1.0, ..colour };
        let home = s.home_cell();
        // Hills stay their ground's colour: rock needs height as well as slope.
        let rock = smoothstep(0.86, 0.78, normal_y) * smoothstep(0.40, 0.60, height);
        if rock > 0.0 && !home.is_water() {
            let r = srgb(0x7C766D);
            colour = colour.mix(&if home.visible { r } else { remembered(r) }, rock);
        }
        if height > SNOW_LINE {
            let snow = srgb(0xF3F5F6);
            let snow = if home.visible { snow } else { remembered(snow) };
            colour = colour.mix(&snow, smoothstep(SNOW_LINE, SNOW_LINE + 0.08, height));
        }
        colour
    }

    /// Water colour over a sample whose floor lies at `floor` (negative under water).
    pub fn water_of(&self, s: &Sample, floor: f32) -> LinearRgba {
        let home = s.home_cell();
        let depth = (-floor).max(0.0);
        let mut c = srgb(0x3C8FA9).mix(&srgb(0x1E4C78), smoothstep(0.20, 0.55, depth));
        if home.ground == Ground::Lake {
            c = srgb(0x3A7FA2);
        }
        if home.has(Feature::Ice) {
            c = srgb(0xDCE8EF);
        }
        // Foam where the floor meets the surface.
        c = c.mix(&srgb(0xE8EEF0), 1.0 - smoothstep(0.0, 0.025, depth));
        if home.visible { c } else { remembered(c) }
    }

    /// How far toward slate a point fades: 1 on an edge shared with an unexplored tile, 0 from 0.25 R in.
    pub fn fade(&self, p: Vec2) -> f32 {
        match self.sample(p) {
            Some(s) => self.fade_of(&s),
            None => 1.0,
        }
    }

    pub fn fade_of(&self, s: &Sample) -> f32 {
        let (p, home) = (s.p, s.home);
        let c = self.grid.centre(home);
        let rel = {
            let mut r = self.unwrap(p) - c;
            if self.grid.wrap_x {
                let span = self.grid.span();
                r.x = (r.x + span / 2.0).rem_euclid(span) - span / 2.0;
            }
            r
        };
        let mut fade = 0.0f32;
        for d in 0..6 {
            let unknown = self.grid.neighbour(home, d).is_none_or(|n| self.cell(n).is_none());
            if unknown {
                let toward = hex::direction(d);
                // Distance from the shared edge, measured toward the home centre.
                let to_edge = PITCH / 2.0 - rel.dot(toward);
                fade = fade.max(1.0 - (to_edge / (0.25 * R)).clamp(0.0, 1.0));
            }
        }
        fade
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Corner {
    pub cell: Cell,
    pub centre: Vec2,
    pub weight: f32,
}

/// A ground point and the known corners of the dual triangle around it.
#[derive(Debug, Clone, Copy)]
pub struct Sample {
    pub p: Vec2,
    pub home: u16,
    /// Corner 0 is always the home tile.
    pub corners: [Option<Corner>; 3],
}

impl Sample {
    fn home_cell(&self) -> &Cell {
        &self.corners[0].as_ref().expect("the home tile is known").cell
    }

    /// Weights raised to `power` and renormalised: higher powers keep each tile's own value further out.
    fn sharpened(&self, power: i32) -> [f32; 3] {
        let mut w = self.corners.map(|c| c.map_or(0.0, |c| c.weight.powi(power)));
        let total: f32 = w.iter().sum();
        if total > 0.0 {
            w.iter_mut().for_each(|x| *x /= total);
        }
        w
    }
}

/// Saturation × 0.25, value × 0.60 (docs/VISUAL-SPEC.md, Fog).
pub fn remembered(c: LinearRgba) -> LinearRgba {
    let mut hsva = Hsva::from(c);
    hsva.saturation *= 0.25;
    hsva.value *= 0.60;
    LinearRgba::from(hsva)
}

fn barycentric(p: Vec2, a: Vec2, b: Vec2, c: Vec2) -> [f32; 3] {
    let v0 = b - a;
    let v1 = c - a;
    let v2 = p - a;
    let den = v0.x * v1.y - v1.x * v0.y;
    let v = (v2.x * v1.y - v1.x * v2.y) / den;
    let w = (v0.x * v2.y - v2.x * v0.y) / den;
    [1.0 - v - w, v, w]
}

fn cosine_bump(t: f32) -> f32 {
    if t >= 1.0 { 0.0 } else { 0.5 * (1.0 + (std::f32::consts::PI * t).cos()) }
}

fn ridge(p: Vec2, a: Vec2, b: Vec2, peak: f32) -> f32 {
    let ab = b - a;
    let t = ((p - a).dot(ab) / ab.length_squared()).clamp(0.0, 1.0);
    let off = (p - (a + ab * t)).length();
    let saddle = 0.55 * peak;
    let along = saddle + (peak - saddle) * (2.0 * t - 1.0).powi(2);
    along * (1.0 - (off / (0.45 * R)).powi(2)).max(0.0)
}

fn near_river(centre: Vec2, p: Vec2, rivers: u8, within: f32) -> bool {
    (0..6).any(|d| rivers & (1 << d) != 0 && {
        let a = centre + hex::corner((d + 5) % 6);
        let b = centre + hex::corner(d);
        let ab = b - a;
        let t = ((p - a).dot(ab) / ab.length_squared()).clamp(0.0, 1.0);
        (p - (a + ab * t)).length() < within
    })
}

pub fn smoothstep(edge0: f32, edge1: f32, x: f32) -> f32 {
    let t = ((x - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// A stable pseudo-random value in [0, 1) for an integer and a salt.
pub fn hash1(n: u32, salt: u32) -> f32 {
    let mut x = n.wrapping_mul(0x9E37_79B1) ^ salt.wrapping_mul(0x85EB_CA77);
    x ^= x >> 15;
    x = x.wrapping_mul(0x2C1B_3C6D);
    x ^= x >> 12;
    x = x.wrapping_mul(0x297A_2D39);
    x ^= x >> 15;
    (x >> 8) as f32 / (1 << 24) as f32
}

/// Ridged noise in [0, 1]: sharp crests where value noise crosses its middle.
fn ridged(p: Vec2) -> f32 {
    1.0 - (2.0 * noise(p) - 1.0).abs()
}

/// Smooth value noise in [0, 1].
fn noise(p: Vec2) -> f32 {
    let i = p.floor();
    let f = p - i;
    let u = f * f * (Vec2::splat(3.0) - 2.0 * f);
    let h = |x: f32, y: f32| hash1((x as i32 as u32).wrapping_mul(73_856_093) ^ (y as i32 as u32).wrapping_mul(19_349_663), 3);
    let (a, b, c, d) = (h(i.x, i.y), h(i.x + 1.0, i.y), h(i.x, i.y + 1.0), h(i.x + 1.0, i.y + 1.0));
    a + (b - a) * u.x + (c - a) * u.y + (a - b - c + d) * u.x * u.y
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cell(ground: Ground) -> Cell {
        Cell {
            ground,
            features: 0,
            rivers: 0,
            road: 0,
            road_pillaged: false,
            improvement: None,
            improvement_pillaged: false,
            resource: None,
            owner: None,
            visible: true,
            peak: 1.2,
        }
    }

    fn relief(wrap_x: bool) -> Relief {
        let grid = Grid { width: 10, height: 6, wrap_x };
        let cells = (0..grid.len())
            .map(|i| {
                Some(cell(match i % 7 {
                    0 => Ground::Mountain,
                    1 | 2 => Ground::Coast,
                    3 => Ground::Ocean,
                    _ => Ground::Plains,
                }))
            })
            .collect();
        Relief { grid, cells, liveries: Default::default() }
    }

    /// The two tiles on either side of an edge agree on every point of it.
    #[test]
    fn heights_agree_across_every_edge() {
        for wrap in [false, true] {
            let r = relief(wrap);
            for id in 0..r.grid.len() as u16 {
                let c = r.grid.centre(id);
                for d in 0..6 {
                    for s in [0.05f32, 0.3, 0.5, 0.7, 0.95] {
                        let a = c + hex::corner((d + 5) % 6);
                        let b = c + hex::corner(d);
                        let p = a + (b - a) * s;
                        // Nudge to either side of the edge and compare.
                        let n = hex::direction(d) * 1e-4;
                        let (h1, h2) = (r.height(p - n), r.height(p + n));
                        if let (Some(h1), Some(h2)) = (h1, h2) {
                            assert!((h1 - h2).abs() < 2e-3, "tile {id} dir {d} at {s}: {h1} vs {h2} (wrap {wrap})");
                        }
                    }
                }
            }
        }
    }

    /// A tile the view lacks has no ground, and its known neighbours fade to slate at the shared edge only.
    #[test]
    fn unknown_tiles_have_no_ground_and_fade_their_edge() {
        let mut r = relief(false);
        let hidden = r.grid.id(4, 3).unwrap();
        r.cells[hidden as usize] = None;
        assert_eq!(r.height(r.grid.centre(hidden)), None);
        let next = r.grid.neighbour(hidden, 0).unwrap();
        let toward_hidden = -hex::direction(0);
        let near_edge = r.grid.centre(next) + toward_hidden * (PITCH / 2.0 - 0.01);
        assert!(r.fade(near_edge) > 0.9);
        assert_eq!(r.fade(r.grid.centre(next)), 0.0);
    }
}
