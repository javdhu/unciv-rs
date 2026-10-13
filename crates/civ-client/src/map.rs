//! The map drawn from the view (docs/VISUAL-SPEC.md): land, water, trees and an overlay of rivers, roads,
//! improvements and borders, in chunks of 8x8 tiles. A view change marks the chunks of the tiles it touched
//! and of their neighbours (heights and fades blend across edges); at most three chunks rebuild per frame.

use crate::client::ViewChanged;
use crate::hex::{self, Grid, PITCH, R};
use crate::relief::{hash1, remembered, srgb, Cell, Relief, SLATE};
use crate::rules::{Feature, Ground, Rules};
use crate::view_state::ViewState;
use bevy::asset::RenderAssetUsages;
use bevy::color::{LinearRgba, Mix};
use bevy::light::NotShadowCaster;
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::prelude::*;
use std::collections::BTreeSet;

pub const CHUNK: u16 = 8;
const PER_FRAME: usize = 3;
/// Lattice rings of a hex and their radii (fractions of the corner distance): four rings, 96 triangles a hex,
/// crowded toward the edge, where colours, fades and shores change.
const RINGS: [f32; 5] = [0.0, 0.42, 0.72, 0.9, 1.0];

/// The known world the meshes were last built from; picking and the inspector read it too.
#[derive(Resource)]
pub struct Land(pub Relief);

#[derive(Resource, Default)]
pub struct Dirty {
    chunks: BTreeSet<(u16, u16)>,
    /// Milliseconds each chunk took to build, since the harness last read them.
    pub built_ms: Vec<f32>,
}

impl Dirty {
    pub fn is_empty(&self) -> bool {
        self.chunks.is_empty()
    }
}

#[derive(Resource)]
pub(crate) struct Materials {
    land: Handle<StandardMaterial>,
    water: Handle<StandardMaterial>,
    overlay: Handle<StandardMaterial>,
    slate: Handle<StandardMaterial>,
}

#[derive(Component)]
pub(crate) struct ChunkPart((u16, u16));

#[derive(Component)]
struct SlatePlane;

pub fn plugin(app: &mut App) {
    app.init_resource::<Dirty>()
        .add_systems(Startup, materials)
        .add_systems(Update, (track, rebuild).chain());
}

fn materials(mut commands: Commands, mut materials: ResMut<Assets<StandardMaterial>>) {
    commands.insert_resource(Materials {
        land: materials.add(StandardMaterial { perceptual_roughness: 0.9, reflectance: 0.2, ..default() }),
        water: materials.add(StandardMaterial { perceptual_roughness: 0.45, reflectance: 0.3, ..default() }),
        overlay: materials.add(StandardMaterial { perceptual_roughness: 0.85, reflectance: 0.2, ..default() }),
        slate: materials.add(StandardMaterial { base_color: SLATE, unlit: true, ..default() }),
    });
}

fn chunk_of(grid: &Grid, id: u16) -> (u16, u16) {
    let (col, row) = grid.coords(id);
    (col / CHUNK, row / CHUNK)
}

/// Refresh the relief and mark chunks when the view or the rules change.
fn track(
    mut commands: Commands,
    view: Res<ViewState>,
    rules: Option<Res<Rules>>,
    mut changes: MessageReader<ViewChanged>,
    mut dirty: ResMut<Dirty>,
    land: Option<Res<Land>>,
    parts: Query<Entity, Or<(With<ChunkPart>, With<SlatePlane>)>>,
    materials: Res<Materials>,
    mut meshes: ResMut<Assets<Mesh>>,
) {
    let (Some(rules), Some(v)) = (rules, view.view()) else {
        changes.clear();
        return;
    };
    let relief = Relief::new(v, &rules);
    let grid = relief.grid;
    let mut all = rules.is_added() || land.as_ref().is_none_or(|l| l.0.grid != grid || l.0.liveries != relief.liveries);
    let mut any = false;
    let mut touched = Vec::new();
    for ViewChanged(c) in changes.read() {
        any = true;
        all |= c.all;
        touched.extend(c.tiles.iter().copied());
    }
    if !(any || all) {
        return;
    }
    if all {
        // A new frame of reference: start over.
        for e in &parts {
            commands.entity(e).despawn();
        }
        dirty.chunks.clear();
        let (cw, ch) = (grid.width.div_ceil(CHUNK), grid.height.div_ceil(CHUNK));
        for cy in 0..ch {
            for cx in 0..cw {
                dirty.chunks.insert((cx, cy));
            }
        }
        spawn_slate(&mut commands, &grid, &materials, &mut meshes);
    } else {
        for id in touched {
            dirty.chunks.insert(chunk_of(&grid, id));
            for d in 0..6 {
                if let Some(n) = grid.neighbour(id, d) {
                    dirty.chunks.insert(chunk_of(&grid, n));
                }
            }
        }
    }
    commands.insert_resource(Land(relief));
}

fn spawn_slate(commands: &mut Commands, grid: &Grid, materials: &Materials, meshes: &mut Assets<Mesh>) {
    let (west, east, south, north) = grid.bounds();
    let margin = 400.0;
    let size = Vec2::new(east - west + 2.0 * margin, north - south + 2.0 * margin);
    let centre = Vec2::new((west + east) / 2.0, (north + south) / 2.0);
    commands.spawn((
        SlatePlane,
        Mesh3d(meshes.add(Plane3d::default().mesh().size(size.x, size.y))),
        MeshMaterial3d(materials.slate.clone()),
        Transform::from_translation(hex::world(centre, -0.6)),
        NotShadowCaster,
    ));
}

pub(crate) fn rebuild(
    mut commands: Commands,
    land: Option<Res<Land>>,
    rules: Option<Res<Rules>>,
    mut dirty: ResMut<Dirty>,
    parts: Query<(Entity, &ChunkPart)>,
    materials: Res<Materials>,
    mut meshes: ResMut<Assets<Mesh>>,
) {
    let (Some(land), Some(rules)) = (land, rules) else { return };
    let relief = &land.0;
    let batch: Vec<_> = dirty.chunks.iter().copied().take(PER_FRAME).collect();
    for key in batch {
        dirty.chunks.remove(&key);
        for (e, part) in &parts {
            if part.0 == key {
                commands.entity(e).despawn();
            }
        }
        let tiles = chunk_tiles(&relief.grid, key);
        let started = bevy::platform::time::Instant::now();
        let built = build_chunk(relief, &rules, &tiles);
        dirty.built_ms.push(started.elapsed().as_secs_f32() * 1000.0);
        let copies: &[f32] = if relief.grid.wrap_x { &[-1.0, 0.0, 1.0] } else { &[0.0] };
        for (mesh, material, shadows) in [
            (built.land, &materials.land, true),
            (built.water, &materials.water, false),
            (built.trees, &materials.land, true),
            (built.overlay, &materials.overlay, false),
        ] {
            let Some(mesh) = mesh else { continue };
            let handle = meshes.add(mesh);
            for copy in copies {
                let mut e = commands.spawn((
                    ChunkPart(key),
                    Mesh3d(handle.clone()),
                    MeshMaterial3d(material.clone()),
                    Transform::from_xyz(copy * relief.grid.span(), 0.0, 0.0),
                ));
                if !shadows {
                    e.insert(NotShadowCaster);
                }
            }
        }
    }
}

fn chunk_tiles(grid: &Grid, (cx, cy): (u16, u16)) -> Vec<u16> {
    let mut out = Vec::new();
    for row in cy * CHUNK..((cy + 1) * CHUNK).min(grid.height) {
        for col in cx * CHUNK..((cx + 1) * CHUNK).min(grid.width) {
            out.push(row * grid.width + col);
        }
    }
    out
}

/// Vertex buffers for one mesh.
#[derive(Default)]
pub(crate) struct Builder {
    positions: Vec<[f32; 3]>,
    normals: Vec<[f32; 3]>,
    colours: Vec<[f32; 4]>,
    indices: Vec<u32>,
}

impl Builder {
    fn vertex(&mut self, p: Vec3, n: Vec3, c: LinearRgba) -> u32 {
        self.positions.push(p.to_array());
        self.normals.push(n.to_array());
        self.colours.push(c.to_f32_array());
        (self.positions.len() - 1) as u32
    }

    fn triangle(&mut self, a: u32, b: u32, c: u32) {
        self.indices.extend([a, b, c]);
    }

    pub(crate) fn finish(self) -> Option<Mesh> {
        if self.indices.is_empty() {
            return None;
        }
        Some(
            Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::RENDER_WORLD)
                .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, self.positions)
                .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, self.normals)
                .with_inserted_attribute(Mesh::ATTRIBUTE_COLOR, self.colours)
                .with_inserted_indices(Indices::U32(self.indices)),
        )
    }

    /// A flat strip along `points` on the ground, `width` wide, `lift` above the land (or the water surface).
    pub(crate) fn ribbon(&mut self, relief: &Relief, points: &[Vec2], width: f32, lift: f32, colour: LinearRgba) {
        if points.len() < 2 {
            return;
        }
        let mut prev: Option<(u32, u32)> = None;
        for (i, p) in points.iter().enumerate() {
            let ahead = points[(i + 1).min(points.len() - 1)] - points[i.saturating_sub(1)];
            let side = Vec2::new(-ahead.y, ahead.x).normalize_or_zero() * (width / 2.0);
            let y = surface(relief, *p) + lift;
            let a = self.vertex(hex::world(*p - side, y), Vec3::Y, colour);
            let b = self.vertex(hex::world(*p + side, y), Vec3::Y, colour);
            if let Some((pa, pb)) = prev {
                self.triangle(pa, a, b);
                self.triangle(pa, b, pb);
            }
            prev = Some((a, b));
        }
    }

    /// A vertical cross of two quads, for posts and markers.
    fn post(&mut self, relief: &Relief, p: Vec2, width: f32, height: f32, colour: LinearRgba) {
        let y = surface(relief, p);
        for (dx, dz) in [(width / 2.0, 0.0), (0.0, width / 2.0)] {
            let o = Vec3::new(dx, 0.0, dz);
            let base = hex::world(p, y);
            let n = Vec3::new(dz, 0.0, -dx).normalize();
            let a = self.vertex(base - o, n, colour);
            let b = self.vertex(base + o, n, colour);
            let c = self.vertex(base + o + Vec3::Y * height, n, colour);
            let d = self.vertex(base - o + Vec3::Y * height, n, colour);
            self.triangle(a, b, c);
            self.triangle(a, c, d);
            self.triangle(a, c, b);
            self.triangle(a, d, c);
        }
    }

    /// A low-poly cone or cylinder band, for trees.
    fn frustum(&mut self, base: Vec3, r0: f32, r1: f32, height: f32, sides: usize, colour: LinearRgba) {
        let slope = (r0 - r1) / height;
        for i in 0..sides {
            let (a0, a1) = (
                std::f32::consts::TAU * i as f32 / sides as f32,
                std::f32::consts::TAU * (i + 1) as f32 / sides as f32,
            );
            let ring = |a: f32, r: f32, y: f32| base + Vec3::new(a.cos() * r, y, a.sin() * r);
            let normal = |a: f32| Vec3::new(a.cos(), slope, a.sin()).normalize();
            let v0 = self.vertex(ring(a0, r0, 0.0), normal(a0), colour);
            let v1 = self.vertex(ring(a1, r0, 0.0), normal(a1), colour);
            let v2 = self.vertex(ring(a1, r1, height), normal(a1), colour);
            let v3 = self.vertex(ring(a0, r1, height), normal(a0), colour);
            self.triangle(v0, v2, v1);
            self.triangle(v0, v3, v2);
        }
    }

    /// A small faceted ball, for broadleaf crowns.
    fn ball(&mut self, centre: Vec3, r: f32, colour: LinearRgba) {
        let (rings, sides) = (3usize, 7usize);
        let point = |i: usize, j: usize| {
            let lat = std::f32::consts::PI * (i as f32 / rings as f32) - std::f32::consts::FRAC_PI_2;
            let lon = std::f32::consts::TAU * j as f32 / sides as f32;
            Vec3::new(lat.cos() * lon.cos(), lat.sin() * 0.8, lat.cos() * lon.sin())
        };
        for i in 0..rings {
            for j in 0..sides {
                let quad = [point(i, j), point(i, j + 1), point(i + 1, j + 1), point(i + 1, j)];
                let v: Vec<u32> = quad.iter().map(|q| self.vertex(centre + *q * r, q.normalize(), colour)).collect();
                self.triangle(v[0], v[2], v[1]);
                self.triangle(v[0], v[3], v[2]);
            }
        }
    }
}

/// Land height, or the water surface where the land lies under it.
fn surface(relief: &Relief, p: Vec2) -> f32 {
    relief.height(p).unwrap_or(0.0).max(0.0)
}

struct Built {
    land: Option<Mesh>,
    water: Option<Mesh>,
    trees: Option<Mesh>,
    overlay: Option<Mesh>,
}

fn build_chunk(relief: &Relief, rules: &Rules, tiles: &[u16]) -> Built {
    let mut land = Builder::default();
    let mut water = Builder::default();
    let mut trees = Builder::default();
    let mut overlay = Builder::default();
    for &id in tiles {
        let Some(cell) = relief.cell(id).copied() else { continue };
        hex_patch(relief, id, &mut land, &mut water);
        features(relief, id, &cell, &mut trees, &mut overlay);
        rivers(relief, id, &cell, &mut overlay);
        roads(relief, id, &cell, &mut overlay);
        improvement(relief, rules, id, &cell, &mut overlay);
        borders(relief, id, &cell, &mut overlay);
    }
    Built { land: land.finish(), water: water.finish(), trees: trees.finish(), overlay: overlay.finish() }
}

fn hex_patch(relief: &Relief, id: u16, land: &mut Builder, water: &mut Builder) {
    let c = relief.grid.centre(id);
    let slate = SLATE.to_linear();
    let n = RINGS.len() - 1;
    // Evaluate a hair inside the hex, so the home tile is always this one.
    let inside = |p: Vec2| c + (p - c) * 0.9995;
    // Each lattice point once: ring r holds 6r points, counter-clockwise from corner 0's spoke.
    let mut points: Vec<Vec<(u32, u32, bool)>> = Vec::with_capacity(n + 1);
    for r in 0..=n {
        let count = (6 * r).max(1);
        let mut ring = Vec::with_capacity(count);
        for q in 0..count {
            let p = if r == 0 {
                c
            } else {
                let (k, j) = (q / r, q % r);
                let (a, b) = (hex::corner(k), hex::corner((k + 1) % 6));
                c + (a * (r - j) as f32 + b * j as f32) / r as f32 * RINGS[r]
            };
            let Some(sample) = relief.sample(inside(p)) else {
                let flat = hex::world(p, 0.0);
                ring.push((land.vertex(flat, Vec3::Y, slate), water.vertex(flat, Vec3::Y, slate), false));
                continue;
            };
            let h = relief.height_of(&sample);
            // Beside an unexplored tile the land continues flat. Forward differences: the same function of
            // position on both sides of a seam.
            let at = |q: Vec2| relief.height(q).unwrap_or(h);
            let e = 0.04;
            let (dx, dy) = (at(p + Vec2::X * e) - h, at(p + Vec2::Y * e) - h);
            // Ground gradient (x east, y north) to a world normal (z south).
            let normal = Vec3::new(-dx, e, dy).normalize();
            let fade = relief.fade_of(&sample);
            let colour = relief.colour_of(&sample, h, normal.y).mix(&slate, fade);
            let wc = relief.water_of(&sample, h).mix(&slate, fade);
            ring.push((
                land.vertex(hex::world(p, h), normal, colour),
                water.vertex(hex::world(p, 0.0), Vec3::Y, wc),
                h < 0.02,
            ));
        }
        points.push(ring);
    }
    // Triangle k's lattice point (i, j), i + j = r, is ring r's point k·r + j.
    let at = |k: usize, i: usize, j: usize| {
        let r = i + j;
        if r == 0 { points[0][0] } else { points[r][(k * r + j) % (6 * r)] }
    };
    for k in 0..6 {
        for i in 0..n {
            for j in 0..n - i {
                let mut tris = vec![[(i, j), (i + 1, j), (i, j + 1)]];
                if i + j < n - 1 {
                    tris.push([(i + 1, j), (i + 1, j + 1), (i, j + 1)]);
                }
                for t in tris {
                    let v = t.map(|(x, y)| at(k, x, y));
                    land.triangle(v[0].0, v[1].0, v[2].0);
                    if v.iter().any(|v| v.2) {
                        water.triangle(v[0].1, v[1].1, v[2].1);
                    }
                }
            }
        }
    }
}

fn shade(cell: &Cell, c: LinearRgba) -> LinearRgba {
    if cell.visible { c } else { remembered(c) }
}

/// Points scattered over a hex, fixed per tile and salt.
fn scatter(relief: &Relief, id: u16, count: usize, radius: f32, salt: u32) -> Vec<Vec2> {
    let c = relief.grid.centre(id);
    (0..count)
        .map(|i| {
            let a = std::f32::consts::TAU * hash1(id as u32 * 31 + i as u32, salt);
            let r = radius * hash1(id as u32 * 31 + i as u32, salt + 1).sqrt();
            c + Vec2::new(a.cos(), a.sin()) * r
        })
        .collect()
}

fn features(relief: &Relief, id: u16, cell: &Cell, trees: &mut Builder, overlay: &mut Builder) {
    let trunk = shade(cell, srgb(0x5A4632));
    // `h` is the tree's whole height.
    let tree = |b: &mut Builder, p: Vec2, kind: u32, h: f32| {
        let y = relief.height(p).unwrap_or(0.0);
        let base = hex::world(p, y);
        match kind {
            // Conifer.
            0 => {
                b.frustum(base, 0.05 * h, 0.035 * h, 0.15 * h, 5, trunk);
                b.frustum(base + Vec3::Y * 0.1 * h, 0.30 * h, 0.0, 0.9 * h, 7, shade(cell, srgb(0x2F4A34)));
            }
            // Broadleaf.
            1 => {
                b.frustum(base, 0.05 * h, 0.04 * h, 0.5 * h, 5, trunk);
                b.ball(base + Vec3::Y * 0.62 * h, 0.38 * h, shade(cell, srgb(0x3F5F2F)));
            }
            // Jungle broadleaf.
            2 => {
                b.frustum(base, 0.05 * h, 0.04 * h, 0.5 * h, 5, trunk);
                b.ball(base + Vec3::Y * 0.6 * h, 0.42 * h, shade(cell, srgb(0x2E5A2A)));
            }
            // Palm.
            3 => {
                b.frustum(base, 0.04 * h, 0.03 * h, 0.85 * h, 5, trunk);
                b.frustum(base + Vec3::Y * 0.82 * h, 0.45 * h, 0.0, 0.18 * h, 7, shade(cell, srgb(0x4F7A35)));
            }
            // Bare trunk.
            _ => b.frustum(base, 0.05 * h, 0.02 * h, h, 5, trunk),
        }
    };
    let cold = matches!(cell.ground, Ground::Tundra | Ground::Snow);
    let woods = [(Feature::Forest, if cold { 0 } else { 1 }, 7, 11), (Feature::Jungle, 2, 9, 13)];
    for (feature, kind, lo, hi) in woods {
        if !cell.has(feature) {
            continue;
        }
        let count = lo + (hash1(id as u32, 11) * (hi - lo + 1) as f32) as usize;
        for (i, p) in scatter(relief, id, count, 0.8 * R, 13).into_iter().enumerate() {
            let h = 0.25 + 0.15 * hash1(id as u32 * 97 + i as u32, 17);
            let kind = if feature == Feature::Jungle && i % 4 == 0 { 3 } else { kind };
            tree(trees, p, kind, h);
        }
        // Woods run on across edges shared with more of the same: the lower id plants the edge.
        for d in 0..6 {
            let Some(n) = relief.grid.neighbour(id, d) else { continue };
            if n > id && relief.cell(n).is_some_and(|c| c.has(feature)) {
                let c = relief.grid.centre(id);
                let mid = c + hex::direction(d) * (PITCH / 2.0);
                let along = Vec2::new(-hex::direction(d).y, hex::direction(d).x);
                for s in [-0.28f32, 0.28] {
                    tree(trees, mid + along * s, kind, 0.3);
                }
            }
        }
    }
    if cell.has(Feature::Oasis) {
        for p in scatter(relief, id, 3, 0.45 * R, 19) {
            tree(trees, p, 3, 0.35);
        }
    }
    if cell.has(Feature::Fallout) {
        let count = 2 + (hash1(id as u32, 23) * 3.0) as usize;
        for p in scatter(relief, id, count, 0.7 * R, 29) {
            tree(trees, p, 4, 0.3);
        }
    }
    if cell.has(Feature::Wonder) {
        overlay.post(relief, relief.grid.centre(id), 0.08, 0.6, shade(cell, srgb(0xC9A94B)));
    }
}

/// The edge facing direction `d`, moved `inset` toward the centre; its ends move along the corner bisectors,
/// so insets of neighbouring edges meet at the corner.
fn edge(relief: &Relief, id: u16, d: usize, samples: usize, inset: f32) -> Vec<Vec2> {
    let c = relief.grid.centre(id);
    let shrink = 1.0 - inset / (R * (60f32).to_radians().sin());
    let (a, b) = (hex::corner((d + 5) % 6) * shrink, hex::corner(d) * shrink);
    (0..=samples).map(|i| c + a + (b - a) * (i as f32 / samples as f32)).collect()
}

fn rivers(relief: &Relief, id: u16, cell: &Cell, overlay: &mut Builder) {
    for d in 0..6 {
        if cell.rivers & (1 << d) == 0 {
            continue;
        }
        let other = relief.grid.neighbour(id, d).filter(|n| relief.cell(*n).is_some());
        if other.is_some_and(|n| n < id) {
            continue;
        }
        overlay.ribbon(relief, &edge(relief, id, d, 6, 0.0), 0.14, 0.012, shade(cell, srgb(0x3A7FA2)));
    }
}

fn roads(relief: &Relief, id: u16, cell: &Cell, overlay: &mut Builder) {
    if cell.road == 0 {
        return;
    }
    let c = relief.grid.centre(id);
    for d in 0..6 {
        let Some(n) = relief.grid.neighbour(id, d) else { continue };
        let Some(other) = relief.cell(n) else { continue };
        if other.road == 0 || n < id {
            continue;
        }
        let step = hex::direction(d) * PITCH;
        let rail = cell.road == 2 && other.road == 2;
        let pillaged = cell.road_pillaged || other.road_pillaged;
        let points: Vec<Vec2> = (0..=8).map(|i| c + step * (i as f32 / 8.0)).collect();
        let pieces: Vec<&[Vec2]> =
            if pillaged { points.chunks(2).filter(|p| p.len() == 2).collect() } else { vec![&points[..]] };
        let side = Vec2::new(-step.y, step.x).normalize();
        for piece in pieces {
            if rail {
                overlay.ribbon(relief, piece, 0.14, 0.01, shade(cell, srgb(0x6E6A63)));
                for s in [-0.035f32, 0.035] {
                    let line: Vec<Vec2> = piece.iter().map(|p| *p + side * s).collect();
                    overlay.ribbon(relief, &line, 0.012, 0.02, shade(cell, srgb(0x3B3A38)));
                }
            } else {
                overlay.ribbon(relief, piece, 0.10, 0.01, shade(cell, srgb(0x8A7556)));
            }
        }
    }
}

fn improvement(relief: &Relief, rules: &Rules, id: u16, cell: &Cell, overlay: &mut Builder) {
    let Some(improvement) = cell.improvement else { return };
    let c = relief.grid.centre(id);
    let dim = |x: LinearRgba| {
        let x = shade(cell, x);
        if cell.improvement_pillaged { x * 0.5 } else { x }
    };
    match rules.improvement(improvement) {
        "Farm" => {
            for i in 0..5 {
                let y = -0.24 + 0.12 * i as f32;
                let row = [c + Vec2::new(-0.3, y), c + Vec2::new(0.0, y), c + Vec2::new(0.3, y)];
                let colour = if i % 2 == 0 { srgb(0xB9A75A) } else { srgb(0x8FA24C) };
                overlay.ribbon(relief, &row, 0.12, 0.008, dim(colour));
            }
        }
        "Mine" => {
            let cut = [c + Vec2::new(-0.15, 0.0), c, c + Vec2::new(0.15, 0.0)];
            overlay.ribbon(relief, &cut, 0.3, 0.01, dim(srgb(0x4A4540)));
        }
        "Pasture" => {
            let ring: Vec<Vec2> =
                (0..=16).map(|i| c + Vec2::from_angle(std::f32::consts::TAU * i as f32 / 16.0) * 0.45 * R).collect();
            overlay.ribbon(relief, &ring, 0.025, 0.03, dim(srgb(0x8A7556)));
        }
        _ => overlay.post(relief, c + Vec2::new(0.25, -0.2), 0.03, 0.15, dim(srgb(0x8A7556))),
    }
}

fn borders(relief: &Relief, id: u16, cell: &Cell, overlay: &mut Builder) {
    let Some(owner) = cell.owner else { return };
    let Some((outer, inner)) = relief.liveries.get(&owner).copied() else { return };
    let ours = |d: usize| {
        relief.grid.neighbour(id, d % 6).and_then(|n| relief.cell(n)).is_some_and(|n| n.owner == Some(owner))
    };
    for d in 0..6 {
        if ours(d) {
            continue;
        }
        let line = |inset: f32| border_edge(relief, id, d, inset, ours(d + 5), ours(d + 1));
        overlay.ribbon(relief, &line(0.05 + 0.035), 0.07, 0.015, shade(cell, outer));
        overlay.ribbon(relief, &line(0.05 + 0.07 + 0.015), 0.03, 0.016, shade(cell, inner));
    }
}

/// The edge facing `d`, inset toward the owner's land. Where the border turns onto a neighbour's edge (that
/// neighbour is the owner's too) the inset ends on the edge between them, where it meets the neighbour's inset;
/// where it turns onto this tile's next edge, on the corner bisector.
fn border_edge(relief: &Relief, id: u16, d: usize, inset: f32, start_turns: bool, end_turns: bool) -> Vec<Vec2> {
    let c = relief.grid.centre(id);
    let along = inset / (60f32).to_radians().sin();
    let shrink = 1.0 - along / R;
    let (prev, start, end, next) =
        (hex::corner((d + 4) % 6), hex::corner((d + 5) % 6), hex::corner(d), hex::corner((d + 1) % 6));
    let a = if start_turns { start + (prev - start).normalize() * along } else { start * shrink };
    let b = if end_turns { end + (next - end).normalize() * along } else { end * shrink };
    (0..=4).map(|i| c + a + (b - a) * (i as f32 / 4.0)).collect()
}
