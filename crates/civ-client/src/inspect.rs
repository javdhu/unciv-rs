//! Picking and the tile inspector (docs/VISUAL-SPEC.md, Picking and the spectator HUD), and the resource
//! markers (Rivers, roads and improvements).
//!
//! The cursor ray marches the relief, so a mountain hides the tiles behind it. Hover outlines a hex; a click
//! pins it in the inspector, and a click on the pinned tile lets it go.

use crate::camera::{MapCamera, CLICK_SLOP};
use crate::hex;
use crate::map::{Builder, Land};
use crate::relief::srgb;
use crate::rules::{ResourceKind, Rules};
use crate::view_state::ViewState;
use bevy::color::Alpha;
use bevy::light::NotShadowCaster;
use bevy::prelude::*;
use bevy::text::Font;
use bevy::window::PrimaryWindow;

const PANEL: Color = Color::srgba(0x14 as f32 / 255.0, 0x16 as f32 / 255.0, 0x1B as f32 / 255.0, 0.85);
const TEXT: Color = Color::srgb(0.93, 0.94, 0.95);
/// Resources show at this camera distance or closer.
const RESOURCE_DISTANCE: f32 = 40.0;

#[derive(Resource, Default)]
pub struct Hover {
    pub tile: Option<u16>,
    pub pinned: Option<u16>,
}

#[derive(Resource)]
pub struct UiFont {
    pub regular: Handle<Font>,
    pub semibold: Handle<Font>,
}

#[derive(Component)]
struct Outline(u16);

#[derive(Component)]
struct Inspector;

#[derive(Component)]
struct InspectorText;

#[derive(Component)]
struct Marker {
    at: Vec3,
}

pub fn plugin(app: &mut App) {
    app.init_resource::<Hover>()
        .add_systems(PreStartup, fonts)
        .add_systems(Startup, panel)
        .add_systems(Update, (pick, outline, inspector, markers, place_markers).chain().after(crate::camera::place));
}

fn fonts(mut commands: Commands, mut fonts: ResMut<Assets<Font>>) {
    let load = |bytes: &[u8]| Font::from_bytes(bytes.to_vec());
    commands.insert_resource(UiFont {
        regular: fonts.add(load(include_bytes!("../assets/fonts/Inter-Regular.ttf"))),
        semibold: fonts.add(load(include_bytes!("../assets/fonts/Inter-SemiBold.ttf"))),
    });
}

fn panel(mut commands: Commands, font: Res<UiFont>) {
    commands
        .spawn((
            Inspector,
            Node {
                position_type: PositionType::Absolute,
                left: Val::Px(12.0),
                bottom: Val::Px(12.0),
                width: Val::Px(280.0),
                padding: UiRect::all(Val::Px(12.0)),
                display: Display::None,
                ..default()
            },
            BackgroundColor(PANEL),
        ))
        .with_child((
            InspectorText,
            Text::default(),
            TextFont { font: font.regular.clone().into(), font_size: FontSize::Px(14.0), ..default() },
            TextColor(TEXT),
        ));
}

/// March the cursor ray over the relief to the first ground it meets.
fn pick(
    window: Query<&Window, With<PrimaryWindow>>,
    cameras: Query<(&Camera, &GlobalTransform, &MapCamera)>,
    buttons: Res<ButtonInput<MouseButton>>,
    land: Option<Res<Land>>,
    mut hover: ResMut<Hover>,
    mut pressed_at: Local<Option<Vec2>>,
    shots: Option<Res<crate::shots::Shots>>,
    mode: Option<Res<crate::play::Mode>>,
) {
    // Review shots show the map alone, whatever the cursor rests on.
    if shots.is_some() {
        hover.tile = None;
        return;
    }
    let (Some(land), Ok((camera, transform, map))) = (land, cameras.single()) else { return };
    let cursor = window.single().ok().and_then(|w| w.cursor_position());
    let hit = cursor.and_then(|c| {
        let ray = camera.viewport_to_world(transform, c).ok()?;
        march(&land, ray.origin, *ray.direction)
    });
    hover.tile = hit;
    if buttons.just_pressed(MouseButton::Left) {
        *pressed_at = cursor;
    }
    if buttons.just_released(MouseButton::Left) {
        let click = matches!((*pressed_at, cursor), (Some(a), Some(b)) if a.distance(b) <= CLICK_SLOP);
        // While you play, a click is an order (src/play.rs), not a pin.
        let playing = mode.is_some_and(|m| *m == crate::play::Mode::Play);
        if click && !map.dragging() && !playing {
            hover.pinned = if hover.pinned == hit { None } else { hit };
        }
    }
}

pub fn march(land: &Land, origin: Vec3, dir: Vec3) -> Option<u16> {
    let relief = &land.0;
    if dir.y >= 0.0 {
        return None;
    }
    // From where the ray drops below the highest peak to where it reaches the slate.
    let (top, bottom) = (2.0f32, -0.6f32);
    let (t0, t1) = (((top - origin.y) / dir.y).max(0.0), (bottom - origin.y) / dir.y);
    let above = |t: f32| {
        let p = origin + dir * t;
        relief.height(hex::ground(p)).is_none_or(|h| p.y > h.max(0.0))
    };
    let step = 0.1;
    let mut t = t0;
    while t < t1 {
        if !above(t) {
            let (mut lo, mut hi) = (t - step, t);
            for _ in 0..8 {
                let mid = (lo + hi) / 2.0;
                if above(mid) { lo = mid } else { hi = mid }
            }
            let g = hex::ground(origin + dir * hi);
            return relief.grid.tile_at(g).filter(|id| relief.cell(*id).is_some());
        }
        t += step;
    }
    None
}

fn outline(
    mut commands: Commands,
    hover: Res<Hover>,
    land: Option<Res<Land>>,
    existing: Query<(Entity, &Outline)>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut material: Local<Option<Handle<StandardMaterial>>>,
) {
    let Some(land) = land else { return };
    let want = hover.tile.or(hover.pinned);
    let current = existing.iter().next().map(|(_, o)| o.0);
    if current == want && !land.is_changed() {
        return;
    }
    for (e, _) in &existing {
        commands.entity(e).despawn();
    }
    let Some(id) = want else { return };
    let material = material
        .get_or_insert_with(|| {
            materials.add(StandardMaterial {
                base_color: srgb(0xF5F1E6).with_alpha(0.8).into(),
                unlit: true,
                alpha_mode: AlphaMode::Blend,
                ..default()
            })
        })
        .clone();
    let relief = &land.0;
    let mut b = Builder::default();
    let c = relief.grid.centre(id);
    let ring: Vec<Vec2> = (0..=6).map(|k| c + hex::corner((k + 5) % 6) * 0.97).collect();
    b.ribbon(relief, &ring, 0.04, 0.03, LinearRgba::WHITE);
    if let Some(mesh) = b.finish() {
        commands.spawn((Outline(id), Mesh3d(meshes.add(mesh)), MeshMaterial3d(material), NotShadowCaster));
    }
}

fn inspector(
    hover: Res<Hover>,
    rules: Option<Res<Rules>>,
    view: Res<ViewState>,
    mut panel: Query<&mut Node, With<Inspector>>,
    mut text: Query<&mut Text, With<InspectorText>>,
) {
    let (Ok(mut node), Ok(mut text)) = (panel.single_mut(), text.single_mut()) else { return };
    let tile = hover.pinned.or(hover.tile);
    let (Some(rules), Some(view), Some(id)) = (rules, view.view(), tile) else {
        node.display = Display::None;
        return;
    };
    let Some(t) = view.tiles.binary_search_by_key(&id, |t| t.id).ok().map(|i| &view.tiles[i]) else {
        node.display = Display::None;
        return;
    };
    node.display = Display::Flex;
    let mut lines = Vec::new();
    let mut ground = vec![rules.terrain_name(t.terrain).to_string()];
    ground.extend(t.features.iter().map(|f| rules.terrain_name(*f).to_string()));
    if let Some(w) = t.natural_wonder {
        ground.push(rules.terrain_name(w).to_string());
    }
    lines.push(ground.join(" · "));
    if let Some((name, kind)) = t.resource.and_then(|r| rules.resource(r)) {
        let kind = match kind {
            ResourceKind::Bonus => "bonus",
            ResourceKind::Luxury => "luxury",
            ResourceKind::Strategic => "strategic",
        };
        lines.push(format!("{name}, {kind}"));
    }
    if let Some(i) = t.improvement {
        let pillaged = if t.improvement_pillaged { ", pillaged" } else { "" };
        lines.push(format!("{}{pillaged}", rules.improvement(i)));
    }
    if t.road > 0 {
        let kind = if t.road == 2 { "Railroad" } else { "Road" };
        let pillaged = if t.road_pillaged { ", pillaged" } else { "" };
        lines.push(format!("{kind}{pillaged}"));
    }
    if t.river_edges != 0 {
        lines.push("River".into());
    }
    if let Some(owner) = t.owner {
        let civ = view.civs.iter().find(|c| c.id == owner);
        let name = civ.and_then(|c| rules.nation(c.nation)).map_or("Unknown", |n| n.0);
        lines.push(format!("Owned by {name}"));
    }
    if let Some(city) = view.cities.iter().find(|c| c.centre == id) {
        lines.push(format!("City of {}", city.name));
    }
    if !t.visible {
        lines.push(format!("Last seen on turn {}", t.observed_turn));
    }
    text.0 = lines.join("\n");
}

/// One marker per known tile with a resource; rebuilt when the land changes.
fn markers(
    mut commands: Commands,
    land: Option<Res<Land>>,
    rules: Option<Res<Rules>>,
    font: Res<UiFont>,
    existing: Query<Entity, With<Marker>>,
) {
    let (Some(land), Some(rules)) = (land, rules) else { return };
    if !land.is_changed() {
        return;
    }
    for e in &existing {
        commands.entity(e).despawn();
    }
    let relief = &land.0;
    for (id, cell) in relief.cells.iter().enumerate() {
        let Some(cell) = cell else { continue };
        let Some((name, kind)) = cell.resource.and_then(|r| rules.resource(r)) else { continue };
        let colour = match kind {
            ResourceKind::Bonus => 0x6FA35A,
            ResourceKind::Luxury => 0x8C6BB1,
            ResourceKind::Strategic => 0x8A8F96,
        };
        let mut colour = Color::from(srgb(colour));
        if !cell.visible {
            colour = Color::from(crate::relief::remembered(colour.to_linear()));
        }
        let ground = relief.grid.centre(id as u16) + Vec2::new(0.35, -0.35);
        let at = hex::world(ground, relief.height(ground).unwrap_or(0.0).max(0.0) + 0.30);
        let initial = name.chars().next().unwrap_or('?').to_string();
        commands
            .spawn((
                Marker { at },
                Node {
                    position_type: PositionType::Absolute,
                    width: Val::Px(18.0),
                    height: Val::Px(18.0),
                    justify_content: JustifyContent::Center,
                    align_items: AlignItems::Center,
                    ..default()
                },
                Visibility::Hidden,
            ))
            .with_children(|m| {
                m.spawn((
                    Node {
                        position_type: PositionType::Absolute,
                        width: Val::Px(13.0),
                        height: Val::Px(13.0),
                        ..default()
                    },
                    UiTransform { rotation: Rot2::degrees(45.0), ..default() },
                    BackgroundColor(colour),
                ));
                m.spawn((
                    Text::new(initial),
                    TextFont { font: font.semibold.clone().into(), font_size: FontSize::Px(10.0), ..default() },
                    TextColor(Color::WHITE),
                ));
            });
    }
}

fn place_markers(
    cameras: Query<(&Camera, &GlobalTransform, &MapCamera)>,
    mut markers: Query<(&Marker, &mut Node, &mut Visibility)>,
) {
    let Ok((camera, transform, map)) = cameras.single() else { return };
    let show = map.distance <= RESOURCE_DISTANCE;
    for (marker, mut node, mut visibility) in &mut markers {
        let screen = show.then(|| camera.world_to_viewport(transform, marker.at).ok()).flatten();
        match screen {
            Some(s) => {
                node.left = Val::Px(s.x - 9.0);
                node.top = Val::Px(s.y - 9.0);
                *visibility = Visibility::Inherited;
            }
            None => *visibility = Visibility::Hidden,
        }
    }
}
