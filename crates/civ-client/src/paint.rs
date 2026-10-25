// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The map material (src/shaders/map.wesl) and its per-tile data texture (docs/VISUAL-SPEC.md; Javed,
//! 2026-10-13: "things that are terrain should be terrain not stickers").
//!
//! Meshes carry shape. What lies on the land, the owner's tint, the fog and the 0.35 s reveal are painted per
//! pixel from a texture of two texels per tile, rewritten whenever the land changes and every frame a reveal
//! runs. A change in sight, ownership or improvement therefore costs a texture upload, not a mesh.

use crate::map::{Land, MapSet};
use crate::relief::{Cell, Improvement, Relief};
use bevy::asset::{embedded_asset, RenderAssetUsages};
use bevy::pbr::{ExtendedMaterial, MaterialExtension};
use bevy::prelude::*;
use bevy::render::render_resource::{AsBindGroup, Extent3d, ShaderType, TextureDimension, TextureFormat};
use bevy::shader::ShaderRef;

pub type MapMaterial = ExtendedMaterial<StandardMaterial, MapExtension>;

const REVEAL_SECONDS: f32 = 0.35;

#[derive(Clone, Copy, Debug, Default, ShaderType, Reflect)]
pub struct MapParams {
    pub width: u32,
    pub height: u32,
    pub wrap_x: u32,
    /// 0 ground, 1 objects (trees, borders), 2 water.
    pub kind: u32,
    /// The hex grid over the ground, on or off.
    pub grid: u32,
}

#[derive(Asset, AsBindGroup, Reflect, Debug, Clone)]
pub struct MapExtension {
    #[uniform(100)]
    pub params: MapParams,
    #[texture(101, sample_type = "float", filterable = false)]
    pub tiles: Handle<Image>,
    #[texture(102, sample_type = "float", filterable = false)]
    pub palette: Handle<Image>,
}

impl MaterialExtension for MapExtension {
    fn fragment_shader() -> ShaderRef {
        "embedded://civ_client/shaders/map.wesl".into()
    }
}

/// The three material instances and the textures they share.
#[derive(Resource)]
pub struct Paint {
    pub ground: Handle<MapMaterial>,
    pub objects: Handle<MapMaterial>,
    pub water: Handle<MapMaterial>,
    tiles: Handle<Image>,
    palette: Handle<Image>,
    size: (u16, u16),
}

/// Per-tile reveal progress: 1 when settled; a tile coming into sight runs from 0 to 1.
#[derive(Resource, Default)]
struct Reveal {
    progress: Vec<f32>,
    /// Whether the tile was known before its current reveal began.
    known_before: Vec<bool>,
    seen: Vec<(bool, bool)>,
    running: bool,
}

pub fn plugin(app: &mut App) {
    embedded_asset!(app, "shaders/map.wesl");
    app.add_plugins(MaterialPlugin::<MapMaterial>::default())
        .init_resource::<Reveal>()
        .add_systems(Update, (setup, reveal, write).chain().after(MapSet::Track).before(MapSet::Rebuild))
        .add_systems(OnExit(crate::app::AppState::Match), (crate::app::reset::<Reveal>, crate::app::drop::<Paint>));
}

fn data_image(width: u32, height: u32, format: TextureFormat) -> Image {
    Image::new_fill(
        Extent3d { width: width.max(1), height: height.max(1), depth_or_array_layers: 1 },
        TextureDimension::D2,
        &[0, 0, 0, 0],
        format,
        RenderAssetUsages::RENDER_WORLD | RenderAssetUsages::MAIN_WORLD,
    )
}

/// Make the materials and textures for this map size (again, if the size changes).
fn setup(
    mut commands: Commands,
    land: Option<Res<Land>>,
    paint: Option<Res<Paint>>,
    mut images: ResMut<Assets<Image>>,
    mut materials: ResMut<Assets<MapMaterial>>,
) {
    let Some(land) = land else { return };
    let grid = land.0.grid;
    if paint.is_some_and(|p| p.size == (grid.width, grid.height)) {
        return;
    }
    let tiles = images.add(data_image(2 * grid.width as u32, grid.height as u32, TextureFormat::Rgba8Unorm));
    // Nation colours by civ id; sRGB, so the shader reads them linear.
    let palette = images.add(data_image(256, 1, TextureFormat::Rgba8UnormSrgb));
    let make = |kind: u32, base: StandardMaterial, materials: &mut Assets<MapMaterial>| {
        materials.add(ExtendedMaterial {
            base,
            extension: MapExtension {
                params: MapParams {
                    width: grid.width as u32,
                    height: grid.height as u32,
                    wrap_x: grid.wrap_x as u32,
                    kind,
                    grid: 0,
                },
                tiles: tiles.clone(),
                palette: palette.clone(),
            },
        })
    };
    let matte = StandardMaterial { perceptual_roughness: 0.9, reflectance: 0.2, ..default() };
    let ground = make(0, matte.clone(), &mut materials);
    let objects = make(1, matte, &mut materials);
    let water = make(2, StandardMaterial { perceptual_roughness: 0.45, reflectance: 0.3, ..default() }, &mut materials);
    commands.insert_resource(Paint { ground, objects, water, tiles, palette, size: (grid.width, grid.height) });
}

impl Reveal {
    /// Note what the land shows now: every tile that came into sight, or became known, starts its reveal.
    fn observe(&mut self, relief: &Relief) {
        let n = relief.cells.len();
        let now: Vec<(bool, bool)> = relief.cells.iter().map(|c| (c.is_some(), c.is_some_and(|c| c.visible))).collect();
        if self.progress.len() != n {
            // A new map: everything already known shows at once.
            *self = Reveal { progress: vec![1.0; n], known_before: vec![true; n], seen: now, running: false };
            return;
        }
        for (i, &(known, visible)) in now.iter().enumerate() {
            let (was_known, was_visible) = self.seen[i];
            if (visible && !was_visible) || (known && !was_known) {
                self.progress[i] = 0.0;
                self.known_before[i] = was_known;
                self.running = true;
            }
        }
        self.seen = now;
    }

    fn advance(&mut self, seconds: f32) {
        if !self.running {
            return;
        }
        let step = seconds / REVEAL_SECONDS;
        let mut running = false;
        for p in self.progress.iter_mut().filter(|p| **p < 1.0) {
            *p = (*p + step).min(1.0);
            running |= *p < 1.0;
        }
        self.running = running;
    }
}

fn reveal(time: Res<Time>, land: Option<Res<Land>>, mut reveal: ResMut<Reveal>) {
    let Some(land) = land else { return };
    if land.is_changed() {
        reveal.observe(&land.0);
    }
    reveal.advance(time.delta_secs());
}

fn texels(cell: &Cell, reveal: f32, known_before: bool) -> [u8; 8] {
    let flags = 1 | (cell.visible as u8) << 1 | (known_before as u8) << 2;
    let improvement = match cell.improvement {
        Improvement::None => 0,
        Improvement::Farm => 1,
        Improvement::Mine => 2,
        Improvement::Pasture => 3,
        Improvement::Other => 4,
    } | (cell.improvement_pillaged as u8) << 3
        | (cell.road.min(2)) << 4
        | (cell.road_pillaged as u8) << 6;
    let ground = cell.is_water() as u8
        | ((cell.ground == crate::rules::Ground::Lake) as u8) << 1
        | (cell.has(crate::rules::Feature::Ice) as u8) << 2;
    let owner = cell.owner.map_or(0, |o| o.saturating_add(1));
    [flags, owner, improvement, (reveal * 255.0).round() as u8, cell.rivers, ground, 0, 0]
}

/// Rewrite the tile texture when the land changed or a reveal is running, and the palette with the land.
fn write(
    land: Option<Res<Land>>,
    paint: Option<Res<Paint>>,
    reveal: Res<Reveal>,
    mut images: ResMut<Assets<Image>>,
    mut was_running: Local<bool>,
) {
    let (Some(land), Some(paint)) = (land, paint) else { return };
    let changed = land.is_changed() || paint.is_added();
    if !(changed || reveal.running || *was_running) {
        return;
    }
    *was_running = reveal.running;
    let relief: &Relief = &land.0;
    if let Some(mut image) = images.get_mut(&paint.tiles) {
        let mut data = vec![0u8; relief.cells.len() * 8];
        for (i, cell) in relief.cells.iter().enumerate() {
            if let Some(cell) = cell {
                let progress = reveal.progress.get(i).copied().unwrap_or(1.0);
                let before = reveal.known_before.get(i).copied().unwrap_or(true);
                data[i * 8..i * 8 + 8].copy_from_slice(&texels(cell, progress, before));
            }
        }
        image.data = Some(data);
    }
    if changed {
        if let Some(mut image) = images.get_mut(&paint.palette) {
            let mut data = vec![0u8; 256 * 4];
            for (civ, (outer, _)) in &relief.liveries {
                let c = Color::from(*outer).to_srgba().to_u8_array();
                data[*civ as usize * 4..*civ as usize * 4 + 4].copy_from_slice(&c);
            }
            image.data = Some(data);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hex::Grid;
    use crate::relief::Cell;
    use crate::rules::Ground;

    fn relief(cells: Vec<Option<Cell>>) -> Relief {
        Relief { grid: Grid { width: cells.len() as u16, height: 1, wrap_x: false }, cells, liveries: Default::default() }
    }

    fn cell(visible: bool) -> Option<Cell> {
        Some(Cell {
            ground: Ground::Plains,
            features: 0,
            rivers: 0,
            road: 0,
            road_pillaged: false,
            improvement: Improvement::None,
            improvement_pillaged: false,
            resource: None,
            owner: None,
            visible,
            peak: 1.2,
        })
    }

    #[test]
    fn tiles_coming_into_sight_reveal_over_the_spec_time() {
        let mut r = Reveal::default();
        // First sight of the map: no reveal.
        r.observe(&relief(vec![cell(true), cell(false), None]));
        assert_eq!(r.progress, vec![1.0; 3]);
        // A remembered tile comes into sight, an unexplored one becomes known.
        r.observe(&relief(vec![cell(true), cell(true), cell(true)]));
        assert_eq!(r.progress, vec![1.0, 0.0, 0.0]);
        assert_eq!(r.known_before, vec![true, true, false]);
        r.advance(REVEAL_SECONDS / 2.0);
        assert!((r.progress[1] - 0.5).abs() < 1e-5);
        r.advance(REVEAL_SECONDS);
        assert_eq!(r.progress, vec![1.0; 3]);
        assert!(!r.running);
        // Going out of sight starts nothing.
        r.observe(&relief(vec![cell(false), cell(false), cell(false)]));
        assert!(!r.running);
    }
}
