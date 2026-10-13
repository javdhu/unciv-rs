//! The map drawn from the view (docs/VISUAL-SPEC.md): the land's shape, water, trees, and an overlay of
//! borders, in chunks of 8x8 tiles, shaded by the map material (src/paint.rs), which paints what lies on the
//! land, the owner's tint and the fog from per-tile data. A chunk rebuilds only when a tile's shape changes in
//! it or beside it (heights blend across edges): becoming known, its ground, features, rivers, owner (borders)
//! or a mine; sight, roads and other improvements only rewrite the tile texture. At most three chunks rebuild a
//! frame.

use crate::client::ViewChanged;
use crate::hex::{self, Grid, PITCH, R};
use crate::paint::Paint;
use crate::relief::{hash1, srgb, Cell, Improvement, Relief, SLATE};
use crate::rules::{Feature, Ground, Rules};
use crate::view_state::ViewState;
use bevy::asset::RenderAssetUsages;
use bevy::color::LinearRgba;
use bevy::light::NotShadowCaster;
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::prelude::*;
use std::collections::BTreeSet;

pub const CHUNK: u16 = 8;
const PER_FRAME: usize = 3;
/// Lattice rings of a hex and their radii (fractions of the corner distance): four rings, 96 triangles a hex,
/// crowded toward the edge, where shores and blends change.
const RINGS: [f32; 5] = [0.0, 0.42, 0.72, 0.9, 1.0];

/// The known world the meshes were last built from; picking, the inspector and the tile texture read it too.
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
struct Slate(Handle<StandardMaterial>);

#[derive(Component)]
struct ChunkPart((u16, u16));

#[derive(Component)]
struct SlatePlane;

/// Where other plugins hook into the map's frame: after the relief is refreshed, before chunks rebuild.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub enum MapSet {
    Track,
    Rebuild,
}

pub fn plugin(app: &mut App) {
    app.init_resource::<Dirty>()
        .configure_sets(Update, (MapSet::Track, MapSet::Rebuild).chain())
        .add_systems(Startup, slate)
        .add_systems(Update, (track.in_set(MapSet::Track), rebuild.in_set(MapSet::Rebuild)));
}

fn slate(mut commands: Commands, mut materials: ResMut<Assets<StandardMaterial>>) {
    commands.insert_resource(Slate(materials.add(StandardMaterial { base_color: SLATE, unlit: true, ..default() })));
}

fn chunk_of(grid: &Grid, id: u16) -> (u16, u16) {
    let (col, row) = grid.coords(id);
    (col / CHUNK, row / CHUNK)
}

/// What a tile's meshes depend on: whether it is known, its ground, features, rivers, owner and a mine's pit.
fn shape(cell: &Option<Cell>) -> Option<(Ground, u16, u8, Option<u8>, bool)> {
    cell.map(|c| (c.ground, c.features, c.rivers, c.owner, c.improvement == Improvement::Mine))
}

/// Refresh the relief when the view or the rules change, and mark the chunks whose shape changed.
#[allow(clippy::too_many_arguments)]
fn track(
    mut commands: Commands,
    view: Res<ViewState>,
    rules: Option<Res<Rules>>,
    mut changes: MessageReader<ViewChanged>,
    mut dirty: ResMut<Dirty>,
    land: Option<Res<Land>>,
    parts: Query<Entity, Or<(With<ChunkPart>, With<SlatePlane>)>>,
    slate: Res<Slate>,
    mut meshes: ResMut<Assets<Mesh>>,
) {
    let (Some(rules), Some(v)) = (rules, view.view()) else {
        changes.clear();
        return;
    };
    let any = changes.read().count() > 0;
    if !any && !rules.is_added() && land.is_some() {
        return;
    }
    let relief = Relief::new(v, &rules);
    let grid = relief.grid;
    let all = rules.is_added() || land.as_ref().is_none_or(|l| l.0.grid != grid || l.0.liveries != relief.liveries);
    if all {
        for e in &parts {
            commands.entity(e).despawn();
        }
        dirty.chunks.clear();
        for cy in 0..grid.height.div_ceil(CHUNK) {
            for cx in 0..grid.width.div_ceil(CHUNK) {
                dirty.chunks.insert((cx, cy));
            }
        }
        spawn_slate(&mut commands, &grid, &slate, &mut meshes);
    } else if let Some(land) = &land {
        for id in 0..grid.len() as u16 {
            if shape(&land.0.cells[id as usize]) != shape(&relief.cells[id as usize]) {
                dirty.chunks.insert(chunk_of(&grid, id));
                for d in 0..6 {
                    if let Some(n) = grid.neighbour(id, d) {
                        dirty.chunks.insert(chunk_of(&grid, n));
                    }
                }
            }
        }
    }
    commands.insert_resource(Land(relief));
}

fn spawn_slate(commands: &mut Commands, grid: &Grid, slate: &Slate, meshes: &mut Assets<Mesh>) {
    let (west, east, south, north) = grid.bounds();
    let margin = 400.0;
    let size = Vec2::new(east - west + 2.0 * margin, north - south + 2.0 * margin);
    let centre = Vec2::new((west + east) / 2.0, (north + south) / 2.0);
    commands.spawn((
        SlatePlane,
        Mesh3d(meshes.add(Plane3d::default().mesh().size(size.x, size.y))),
        MeshMaterial3d(slate.0.clone()),
        Transform::from_translation(hex::world(centre, -0.6)),
        NotShadowCaster,
    ));
}

fn rebuild(
    mut commands: Commands,
    land: Option<Res<Land>>,
    paint: Option<Res<Paint>>,
    mut dirty: ResMut<Dirty>,
    parts: Query<(Entity, &ChunkPart)>,
    mut meshes: ResMut<Assets<Mesh>>,
) {
    let (Some(land), Some(paint)) = (land, paint) else { return };
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
        let built = build_chunk(relief, &tiles);
        dirty.built_ms.push(started.elapsed().as_secs_f32() * 1000.0);
        let copies: &[f32] = if relief.grid.wrap_x { &[-1.0, 0.0, 1.0] } else { &[0.0] };
        for (mesh, material, shadows) in [
            (built.land, &paint.ground, true),
            (built.water, &paint.water, false),
            (built.trees, &paint.objects, true),
            (built.overlay, &paint.objects, false),
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
    pub(crate) fn frustum(&mut self, base: Vec3, r0: f32, r1: f32, height: f32, sides: usize, colour: LinearRgba) {
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

    /// An axis-aligned box standing on `base`: `sides` colour on the walls, `top` on the roof.
    pub(crate) fn cuboid(&mut self, base: Vec3, size: Vec3, sides: LinearRgba, top: LinearRgba) {
        let (x, y, z) = (size.x / 2.0, size.y, size.z / 2.0);
        let faces: [(Vec3, [Vec3; 4], LinearRgba); 5] = [
            (Vec3::Y, [Vec3::new(-x, y, -z), Vec3::new(-x, y, z), Vec3::new(x, y, z), Vec3::new(x, y, -z)], top),
            (Vec3::X, [Vec3::new(x, 0.0, z), Vec3::new(x, 0.0, -z), Vec3::new(x, y, -z), Vec3::new(x, y, z)], sides),
            (-Vec3::X, [Vec3::new(-x, 0.0, -z), Vec3::new(-x, 0.0, z), Vec3::new(-x, y, z), Vec3::new(-x, y, -z)], sides),
            (Vec3::Z, [Vec3::new(-x, 0.0, z), Vec3::new(x, 0.0, z), Vec3::new(x, y, z), Vec3::new(-x, y, z)], sides),
            (-Vec3::Z, [Vec3::new(x, 0.0, -z), Vec3::new(-x, 0.0, -z), Vec3::new(-x, y, -z), Vec3::new(x, y, -z)], sides),
        ];
        for (normal, corners, colour) in faces {
            let v = corners.map(|c| self.vertex(base + c, normal, colour));
            self.triangle(v[0], v[1], v[2]);
            self.triangle(v[0], v[2], v[3]);
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

fn build_chunk(relief: &Relief, tiles: &[u16]) -> Built {
    let mut land = Builder::default();
    let mut water = Builder::default();
    let mut trees = Builder::default();
    let mut overlay = Builder::default();
    for &id in tiles {
        let Some(cell) = relief.cell(id).copied() else { continue };
        hex_patch(relief, id, &mut land, &mut water);
        features(relief, id, &cell, &mut trees, &mut overlay);
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
            // Forward differences. The edge ring is shared with the neighbouring hex, so it evaluates the exact
            // function of position (beside an unexplored tile the land continues flat); inside the hex the
            // sample's own triangle serves.
            let at = |q: Vec2| {
                if r == n { relief.height(q).unwrap_or(h) } else { relief.height_near(&sample, q) }
            };
            let e = 0.04;
            let (dx, dy) = (at(p + Vec2::X * e) - h, at(p + Vec2::Y * e) - h);
            // Ground gradient (x east, y north) to a world normal (z south).
            let normal = Vec3::new(-dx, e, dy).normalize();
            let colour = relief.colour_of(&sample, h, normal.y);
            // Water carries the depth below it in its alpha, for the shader's colour and foam.
            let depth = LinearRgba::new(1.0, 1.0, 1.0, (-h).clamp(0.0, 1.0));
            ring.push((
                land.vertex(hex::world(p, h), normal, colour),
                water.vertex(hex::world(p, 0.0), Vec3::Y, depth),
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
    let trunk = srgb(0x5A4632);
    // `h` is the tree's whole height.
    let tree = |b: &mut Builder, p: Vec2, kind: u32, h: f32| {
        let y = relief.height(p).unwrap_or(0.0);
        let base = hex::world(p, y);
        match kind {
            // Conifer.
            0 => {
                b.frustum(base, 0.05 * h, 0.035 * h, 0.15 * h, 5, trunk);
                b.frustum(base + Vec3::Y * 0.1 * h, 0.30 * h, 0.0, 0.9 * h, 7, srgb(0x2F4A34));
            }
            // Broadleaf.
            1 => {
                b.frustum(base, 0.05 * h, 0.04 * h, 0.5 * h, 5, trunk);
                b.ball(base + Vec3::Y * 0.62 * h, 0.38 * h, srgb(0x3F5F2F));
            }
            // Jungle broadleaf.
            2 => {
                b.frustum(base, 0.05 * h, 0.04 * h, 0.5 * h, 5, trunk);
                b.ball(base + Vec3::Y * 0.6 * h, 0.42 * h, srgb(0x2E5A2A));
            }
            // Palm.
            3 => {
                b.frustum(base, 0.04 * h, 0.03 * h, 0.85 * h, 5, trunk);
                b.frustum(base + Vec3::Y * 0.82 * h, 0.45 * h, 0.0, 0.18 * h, 7, srgb(0x4F7A35));
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
        overlay.post(relief, relief.grid.centre(id), 0.08, 0.6, srgb(0xC9A94B));
    }
}

/// The edge facing direction `d`, moved `inset` toward the centre; its ends move along the corner bisectors,
/// so insets of neighbouring edges meet at the corner.
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
        overlay.ribbon(relief, &line(0.05 + 0.035), 0.07, 0.015, outer);
        overlay.ribbon(relief, &line(0.05 + 0.07 + 0.015), 0.03, 0.016, inner);
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
