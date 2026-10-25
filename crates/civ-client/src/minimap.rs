// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The minimap (docs/QUALITY-SPEC.md, K3): the known world drawn small in the bottom-left corner, as in
//! Civilization VI. Terrain by ground, tinted by owner, remembered tiles dimmed, unknown ones left dark; the
//! camera's view is outlined, and a click or drag looks there. Buttons along its top fit the map in view and
//! show or hide the hex grid.
//!
//! The image has 4 pixels across and 3 down per tile, odd rows shifted 2 pixels east (odd-r), so a hex's 1.5
//! row pitch and √3 column pitch keep nearly their proportion.

use crate::camera::MapCamera;
use crate::hex::{self, Grid};
use crate::inspect::UiFont;
use crate::map::Land;
use crate::playui::{text, BUTTON, BUTTON_ON, MINIMAP_SIZE, PANEL, TEXT};
use crate::rules::{Feature, Ground, Rules};
use crate::view_state::ViewState;
use bevy::asset::RenderAssetUsages;
use bevy::picking::hover::Hovered;
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
use bevy::ui_widgets::{observe, Activate, Button};
use bevy::window::PrimaryWindow;

const PX_W: u32 = 4;
const PX_H: u32 = 3;
const STRIP: f32 = 26.0;

#[derive(Resource, Default)]
pub struct Minimap {
    image: Option<Handle<Image>>,
    pub grid: bool,
    /// The picture's size on screen: the panel's width, and the map's proportion.
    display: Vec2,
}

/// The picture's size for a map: as wide as the panel, as tall as the map's shape asks, at most the panel's height.
fn display(grid: &Grid) -> Vec2 {
    let (w, h) = size(grid);
    Vec2::new(MINIMAP_SIZE.x, (MINIMAP_SIZE.x * h as f32 / w as f32).min(MINIMAP_SIZE.y))
}

#[derive(Component)]
struct Frame;
#[derive(Component)]
struct Picture;
#[derive(Component)]
struct ViewBox;

#[derive(Component, Clone, Copy, PartialEq)]
enum Press {
    Fit,
    Grid,
}

pub fn plugin(app: &mut App) {
    app.init_resource::<Minimap>()
        .add_systems(OnEnter(crate::app::AppState::Match), frame)
        .add_systems(OnExit(crate::app::AppState::Match), crate::app::reset::<Minimap>)
        .add_systems(Update, (paint, view_box, look, grid_key, shade).chain().after(crate::camera::place).in_set(crate::app::InMatch));
}

fn frame(mut commands: Commands, font: Res<UiFont>) {
    commands
        .spawn((
            Frame,
            Node {
                position_type: PositionType::Absolute,
                left: Val::Px(12.0),
                bottom: Val::Px(12.0),
                width: Val::Px(MINIMAP_SIZE.x),
                height: Val::Px(MINIMAP_SIZE.y + STRIP),
                flex_direction: FlexDirection::Column,
                ..default()
            },
            BackgroundColor(PANEL),
            crate::screens::PANEL_Z,
        ))
        .with_children(|f| {
            f.spawn(Node { height: Val::Px(STRIP), column_gap: Val::Px(4.0), padding: UiRect::all(Val::Px(3.0)), ..default() })
                .with_children(|b| {
                    b.spawn((button(&font, Press::Fit, "Whole map"), crate::tips::Tip::new("Zoom out to see the whole map")));
                    b.spawn((button(&font, Press::Grid, "Grid (G)"), crate::tips::Tip::new("Show or hide the hex grid on the map")));
                });
            f.spawn((
                Picture,
                Node { width: Val::Px(MINIMAP_SIZE.x), height: Val::Px(MINIMAP_SIZE.y), overflow: Overflow::clip(), ..default() },
            ))
            .with_child((
                ViewBox,
                Node { position_type: PositionType::Absolute, border: UiRect::all(Val::Px(1.0)), ..default() },
                BorderColor::all(Color::WHITE),
                Visibility::Hidden,
            ));
        });
}

fn button(font: &UiFont, press: Press, label: &str) -> impl Bundle {
    (
        press,
        Button,
        Hovered::default(),
        observe(on_press),
        Node { padding: UiRect::axes(Val::Px(8.0), Val::Px(2.0)), border_radius: BorderRadius::all(Val::Px(3.0)), ..default() },
        BackgroundColor(BUTTON),
        children![text(font, label, 11.0, TEXT)],
    )
}

/// A tile's colour on the minimap: its ground, its owner's colour over land, dimmed when only remembered.
pub fn colour(tile: &civ_sim::TileView, rules: &Rules, owner: Option<Color>) -> [u8; 4] {
    let ground = match rules.ground(tile.terrain) {
        Ground::Ocean => [34, 62, 96],
        Ground::Coast | Ground::Lake => [58, 104, 142],
        Ground::Grassland => [86, 128, 58],
        Ground::Plains => [146, 140, 76],
        Ground::Desert => [196, 180, 120],
        Ground::Tundra => [126, 130, 112],
        Ground::Snow => [226, 232, 236],
        Ground::Mountain => [108, 104, 100],
    };
    let hill = tile.features.iter().any(|f| rules.feature(*f) == Feature::Hill);
    let mut c = ground.map(|v| if hill { (v as f32 * 0.82) as u8 } else { v });
    let water = matches!(rules.ground(tile.terrain), Ground::Ocean | Ground::Coast | Ground::Lake);
    if let (Some(o), false) = (owner, water) {
        let o = o.to_srgba();
        let mix = |a: u8, b: f32| (a as f32 * 0.5 + b * 255.0 * 0.5) as u8;
        c = [mix(c[0], o.red), mix(c[1], o.green), mix(c[2], o.blue)];
    }
    let dim = if tile.visible { 1.0 } else { 0.55 };
    [(c[0] as f32 * dim) as u8, (c[1] as f32 * dim) as u8, (c[2] as f32 * dim) as u8, 255]
}

/// The image's pixel box for a tile: its 4x3 block, odd rows shifted half a tile.
fn block(grid: &Grid, id: u16) -> (u32, u32) {
    let (col, row) = grid.coords(id);
    (col as u32 * PX_W + (row as u32 % 2) * PX_W / 2, row as u32 * PX_H)
}

fn size(grid: &Grid) -> (u32, u32) {
    (grid.width as u32 * PX_W + PX_W / 2, grid.height as u32 * PX_H)
}

/// Repaint the image when the view changes.
fn paint(
    mut commands: Commands,
    view: Res<ViewState>,
    rules: Option<Res<Rules>>,
    land: Option<Res<Land>>,
    mut minimap: ResMut<Minimap>,
    mut images: ResMut<Assets<Image>>,
    mut picture: Query<(Entity, &mut Node), With<Picture>>,
    mut frames: Query<&mut Node, (With<Frame>, Without<Picture>)>,
) {
    let (Some(v), Some(rules), Some(land), Ok((picture, mut node))) = (view.view(), rules, land, picture.single_mut()) else { return };
    if !view.is_changed() && minimap.image.is_some() {
        return;
    }
    let grid = land.0.grid;
    minimap.display = display(&grid);
    node.height = Val::Px(minimap.display.y);
    if let Ok(mut frame) = frames.single_mut() {
        frame.height = Val::Px(minimap.display.y + STRIP);
    }
    let (w, h) = size(&grid);
    let mut data = vec![0u8; (w * h * 4) as usize];
    for t in &v.tiles {
        let owner = t.owner.and_then(|o| v.civs.iter().find(|c| c.id == o)).and_then(|c| rules.nation(c.nation)).map(|n| n.1);
        let c = colour(t, &rules, owner);
        let (x0, y0) = block(&grid, t.id);
        for y in y0..(y0 + PX_H).min(h) {
            for x in x0..(x0 + PX_W).min(w) {
                let i = ((y * w + x) * 4) as usize;
                data[i..i + 4].copy_from_slice(&c);
            }
        }
    }
    // Cities as a light dot in their tile.
    for c in &v.cities {
        let (x0, y0) = block(&grid, c.centre);
        let (x, y) = (x0 + PX_W / 2 - 1, y0 + 1);
        for (dx, dy) in [(0, 0), (1, 0)] {
            let i = (((y + dy) * w + x + dx) * 4) as usize;
            if i + 4 <= data.len() {
                data[i..i + 4].copy_from_slice(&[245, 240, 225, 255]);
            }
        }
    }
    let mut image = Image::new(
        Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        TextureDimension::D2,
        data,
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::RENDER_WORLD,
    );
    image.sampler = bevy::image::ImageSampler::nearest();
    let handle = images.add(image);
    if let Some(old) = minimap.image.replace(handle.clone()) {
        images.remove(&old);
    }
    commands.entity(picture).insert(ImageNode::new(handle).with_mode(bevy::ui::widget::NodeImageMode::Stretch));
}

/// Ground point to minimap node coordinates, and back.
fn to_map(grid: &Grid, ground: Vec2) -> Vec2 {
    let ((w, h), shown) = (size(grid), display(grid));
    let col = ground.x / hex::PITCH;
    let row = -ground.y / 1.5;
    Vec2::new((col * PX_W as f32 + PX_W as f32 / 2.0) / w as f32 * shown.x, (row * PX_H as f32 + PX_H as f32 / 2.0) / h as f32 * shown.y)
}

fn to_ground(grid: &Grid, at: Vec2) -> Vec2 {
    let ((w, h), shown) = (size(grid), display(grid));
    let col = (at.x / shown.x * w as f32 - PX_W as f32 / 2.0) / PX_W as f32;
    let row = (at.y / shown.y * h as f32 - PX_H as f32 / 2.0) / PX_H as f32;
    Vec2::new(col * hex::PITCH, -row * 1.5)
}

/// Outline what the camera sees, from where its window's corners meet the ground.
fn view_box(
    land: Option<Res<Land>>,
    windows: Query<&Window, With<PrimaryWindow>>,
    cameras: Query<(&Camera, &GlobalTransform), With<MapCamera>>,
    mut boxes: Query<(&mut Node, &mut Visibility), With<ViewBox>>,
) {
    let (Some(land), Ok(window), Ok((camera, transform)), Ok((mut node, mut visibility))) =
        (land, windows.single(), cameras.single(), boxes.single_mut())
    else {
        return;
    };
    let (ww, wh) = (window.width(), window.height());
    let corners = [Vec2::new(0.0, wh * 0.15), Vec2::new(ww, wh * 0.15), Vec2::new(0.0, wh), Vec2::new(ww, wh)];
    let points: Vec<Vec2> = corners.iter().filter_map(|c| crate::camera::ground_under(camera, transform, *c)).map(|g| to_map(&land.0.grid, g)).collect();
    if points.len() < 2 {
        *visibility = Visibility::Hidden;
        return;
    }
    let shown = display(&land.0.grid);
    let min = points.iter().fold(Vec2::MAX, |a, p| a.min(*p)).max(Vec2::ZERO);
    let max = points.iter().fold(Vec2::MIN, |a, p| a.max(*p)).min(shown);
    if max.x <= min.x || max.y <= min.y {
        *visibility = Visibility::Hidden;
        return;
    }
    *visibility = Visibility::Inherited;
    node.left = Val::Px(min.x);
    node.top = Val::Px(min.y);
    node.width = Val::Px(max.x - min.x);
    node.height = Val::Px(max.y - min.y);
}

/// A press or drag on the picture looks there.
fn look(
    buttons: Res<ButtonInput<MouseButton>>,
    windows: Query<&Window, With<PrimaryWindow>>,
    land: Option<Res<Land>>,
    picture: Query<(&ComputedNode, &UiGlobalTransform), With<Picture>>,
    mut cameras: Query<&mut MapCamera>,
) {
    if !buttons.pressed(MouseButton::Left) {
        return;
    }
    let (Some(land), Ok(window), Ok((node, transform)), Ok(mut camera)) = (land, windows.single(), picture.single(), cameras.single_mut()) else {
        return;
    };
    let Some(cursor) = window.cursor_position() else { return };
    // The node's box in physical pixels; the cursor is logical.
    let scale = 1.0 / window.scale_factor();
    let centre = transform.translation * scale;
    let half = node.size() * scale / 2.0;
    let at = cursor - (centre - half);
    if at.x < 0.0 || at.y < 0.0 || at.x > half.x * 2.0 || at.y > half.y * 2.0 {
        return;
    }
    let at = at / (half * 2.0) * display(&land.0.grid);
    let distance = camera.distance;
    camera.jump(to_ground(&land.0.grid, at), distance);
}

fn grid_key(keys: Res<ButtonInput<KeyCode>>, mut minimap: ResMut<Minimap>, paint: Option<Res<crate::paint::Paint>>, mut materials: ResMut<Assets<crate::paint::MapMaterial>>) {
    if crate::keys::just(&keys, crate::keys::Act::Grid) {
        minimap.grid = !minimap.grid;
    }
    if !minimap.is_changed() {
        return;
    }
    if let Some(paint) = paint {
        if let Some(mut m) = materials.get_mut(&paint.ground) {
            m.extension.params.grid = minimap.grid as u32;
        }
    }
}

fn on_press(
    event: On<Activate>,
    presses: Query<&Press>,
    mut minimap: ResMut<Minimap>,
    land: Option<Res<Land>>,
    mut cameras: Query<&mut MapCamera>,
) {
    let Ok(press) = presses.get(event.entity) else { return };
    match press {
        Press::Grid => minimap.grid = !minimap.grid,
        Press::Fit => {
            if let (Some(land), Ok(mut camera)) = (land, cameras.single_mut()) {
                let grid = land.0.grid;
                let middle = (grid.centre(0) + grid.centre(grid.len() as u16 - 1)) / 2.0;
                let far = camera.max_distance;
                camera.jump(middle, far);
            }
        }
    }
}

fn shade(mut buttons: Query<(&Hovered, &mut BackgroundColor), (With<Press>, Changed<Hovered>)>) {
    for (hovered, mut colour) in &mut buttons {
        colour.0 = if hovered.get() { BUTTON_ON } else { BUTTON };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Tiles of a real view colour as the spec says: water and land apart, owners tinting land only, tiles out of
    /// sight dimmer than those in it.
    #[test]
    fn tile_colours_follow_the_view() {
        let session = civ_sim::session::Session::new(civ_sim::Settings { seed: 4, ..civ_sim::Settings::default() }, "m".into(), vec![0]).unwrap();
        let v = session.current_update(0).unwrap().view;
        let rules = Rules::new(session.content_hash(), civ_sim::tables(&civ_sim::content::load().unwrap()));
        let water = |t: &civ_sim::TileView| matches!(rules.ground(t.terrain), Ground::Ocean | Ground::Coast | Ground::Lake);
        let land = v.tiles.iter().find(|t| !water(t) && t.visible).expect("land in sight");
        let sea = v.tiles.iter().find(|t| water(t) && t.visible).expect("water in sight");
        let red = Some(Color::srgb(1.0, 0.0, 0.0));
        assert_ne!(colour(land, &rules, red), colour(land, &rules, None), "an owner tints land");
        assert_eq!(colour(sea, &rules, red), colour(sea, &rules, None), "but not water");
        let mut remembered = land.clone();
        remembered.visible = false;
        let (seen, dim) = (colour(land, &rules, None), colour(&remembered, &rules, None));
        assert!(dim[..3].iter().zip(&seen[..3]).all(|(d, s)| d <= s) && dim != seen, "remembered tiles are dimmer");
    }

    /// A tile's block and the ground point at its centre map onto each other.
    #[test]
    fn tiles_and_ground_meet_on_the_minimap() {
        let grid = Grid { width: 32, height: 20, wrap_x: false };
        for id in [0u16, 31, 32, 33, 300, 639] {
            let at = to_map(&grid, grid.centre(id));
            let (x0, y0) = block(&grid, id);
            let (w, h) = size(&grid);
            let shown = display(&grid);
            let px = Vec2::new(at.x / shown.x * w as f32, at.y / shown.y * h as f32);
            assert!(px.x >= x0 as f32 && px.x <= (x0 + PX_W) as f32, "{id}: x {} not in {x0}..", px.x);
            assert!(px.y >= y0 as f32 && px.y <= (y0 + PX_H) as f32, "{id}: y {} not in {y0}..", px.y);
            assert!((to_ground(&grid, at) - grid.centre(id)).length() < 1e-3);
        }
    }
}
