// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Units and cities drawn from the view (docs/VISUAL-SPEC.md, Cities; Units).
//!
//! One entity per unit and per city, keyed by (id, generation) and kept in step with `ViewState` through the
//! keys each `ViewChanged` names. A unit stands where the view puts it unless presentation (src/present.rs) is
//! moving it; badges and banners are screen-space UI placed over the 3D pieces every frame.

use crate::camera::{MapCamera, FOV_DEGREES};
use crate::client::ViewChanged;
use crate::hex;
use crate::inspect::UiFont;
use crate::map::{Builder, Land};
use crate::paint::Paint;
use crate::relief::{hash1, srgb, Relief};
use crate::rules::Rules;
use crate::view_state::ViewState;
use bevy::color::LinearRgba;
use bevy::prelude::*;
use std::collections::HashMap;

pub type Key = (u32, u32);

/// Unit classes, from the rules' unit types (docs/VISUAL-SPEC.md, Units).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Class {
    Melee,
    Ranged,
    Mounted,
    Siege,
    Naval,
    Air,
    Civilian,
    Great,
}

impl Class {
    pub fn of(rules: &Rules, base: u32) -> Class {
        let Some(row) = rules.tables.units.iter().find(|u| u.id == base) else { return Class::Melee };
        match row.unit_type.as_str() {
            _ if row.name.starts_with("Great ") => Class::Great,
            "Archery" | "Ranged Gunpowder" => Class::Ranged,
            "Mounted" => Class::Mounted,
            "Siege" => Class::Siege,
            "Melee Water" | "Ranged Water" | "Submarine" | "Aircraft Carrier" | "Civilian Water" => Class::Naval,
            "Fighter" | "Bomber" | "Atomic Bomber" | "Helicopter" | "Missile" => Class::Air,
            "Civilian" => Class::Civilian,
            _ => Class::Melee,
        }
    }

    fn military(self) -> bool {
        !matches!(self, Class::Civilian | Class::Great)
    }

    fn glyph(self) -> &'static str {
        match self {
            Class::Melee => "M",
            Class::Ranged => "R",
            Class::Mounted => "H",
            Class::Siege => "S",
            Class::Naval => "N",
            Class::Air => "A",
            Class::Civilian => "C",
            Class::Great => "G",
        }
    }
}

#[derive(Component)]
pub struct UnitPiece {
    pub key: Key,
    pub owner: u8,
    pub class: Class,
    pub tile: u16,
    pub health: u8,
    pub embarked: bool,
    /// Where the unit stands on its tile, beside others sharing it.
    pub offset: Vec2,
    /// Which way it faces, radians about +Y.
    pub heading: f32,
}

#[derive(Component)]
pub struct CityPiece {
    pub centre: u16,
}

/// Presentation holds a unit here instead of on its view tile while it plays its moves.
#[derive(Component)]
pub struct Held(pub Vec3);

#[derive(Component)]
struct Badge(Entity);

#[derive(Component)]
struct BadgeHealth;

#[derive(Component)]
struct Banner(Entity);

/// Entities by key, for presentation to find.
#[derive(Resource, Default)]
pub struct Pieces {
    pub units: HashMap<Key, Entity>,
    pub cities: HashMap<Key, Entity>,
}

#[derive(Resource, Default)]
struct Shapes {
    meshes: HashMap<Class, (Handle<Mesh>, Handle<Mesh>)>,
    hull: Option<Handle<Mesh>>,
    band: Option<Handle<Mesh>>,
    liveries: HashMap<u8, (Handle<StandardMaterial>, Handle<StandardMaterial>)>,
}

/// Where presentation hooks in: before and after the projection, and before units are stood in place.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub enum PiecesSet {
    Project,
    Stand,
}

pub fn plugin(app: &mut App) {
    app.init_resource::<Pieces>()
        .init_resource::<Shapes>()
        .configure_sets(
            Update,
            (PiecesSet::Project, PiecesSet::Stand).chain().after(crate::map::MapSet::Rebuild).in_set(crate::app::InMatch),
        )
        .add_systems(OnExit(crate::app::AppState::Match), crate::app::reset::<Pieces>)
        .add_systems(
            Update,
            (project.in_set(PiecesSet::Project), (stand, badges, banners).chain().in_set(PiecesSet::Stand)),
        );
}

fn unit_meshes(shapes: &mut Shapes, meshes: &mut Assets<Mesh>, class: Class) -> (Handle<Mesh>, Handle<Mesh>) {
    if let Some(m) = shapes.meshes.get(&class) {
        return m.clone();
    }
    let body: Mesh = match class {
        Class::Melee => Cuboid::new(0.22, 0.36, 0.22).mesh().build().translated_by(Vec3::Y * 0.18),
        Class::Ranged => Cylinder::new(0.10, 0.36).mesh().resolution(10).build().translated_by(Vec3::Y * 0.18),
        Class::Mounted => Cuboid::new(0.34, 0.30, 0.18).mesh().build().translated_by(Vec3::Y * 0.15),
        Class::Siege => {
            let mut m = Cuboid::new(0.32, 0.18, 0.22).mesh().build().translated_by(Vec3::Y * 0.09);
            let barrel = Cylinder::new(0.035, 0.28)
                .mesh()
                .resolution(8)
                .build()
                .rotated_by(Quat::from_rotation_z(-1.2))
                .translated_by(Vec3::new(0.12, 0.24, 0.0));
            m.merge(&barrel).expect("same attributes");
            m
        }
        Class::Naval => Extrusion::new(Triangle2d::new(Vec2::new(0.25, 0.0), Vec2::new(-0.2, 0.09), Vec2::new(-0.2, -0.09)), 0.12)
            .mesh()
            .build()
            .rotated_by(Quat::from_rotation_x(-std::f32::consts::FRAC_PI_2))
            .translated_by(Vec3::Y * 0.06),
        Class::Air => {
            let wing = |angle: f32| {
                Cuboid::new(0.24, 0.03, 0.06)
                    .mesh()
                    .build()
                    .translated_by(Vec3::new(-0.1, 0.0, 0.0))
                    .rotated_by(Quat::from_rotation_y(angle))
            };
            let mut m = wing(0.5);
            m.merge(&wing(-0.5)).expect("same attributes");
            m.translated_by(Vec3::Y * 0.05)
        }
        Class::Civilian => Sphere::new(0.12).mesh().ico(2).expect("small ico").translated_by(Vec3::Y * 0.12),
        Class::Great => Cone::new(0.12, 0.40).mesh().resolution(12).build().translated_by(Vec3::Y * 0.20),
    };
    let band = shapes
        .band
        .get_or_insert_with(|| meshes.add(Torus::new(0.115, 0.14).mesh().minor_resolution(6).major_resolution(16)))
        .clone();
    let entry = (meshes.add(body), band);
    shapes.meshes.insert(class, entry.clone());
    entry
}

fn livery(
    shapes: &mut Shapes,
    materials: &mut Assets<StandardMaterial>,
    relief: &Relief,
    owner: u8,
) -> (Handle<StandardMaterial>, Handle<StandardMaterial>) {
    shapes
        .liveries
        .entry(owner)
        .or_insert_with(|| {
            let (outer, inner) = relief.liveries.get(&owner).copied().unwrap_or((LinearRgba::gray(0.5), LinearRgba::WHITE));
            let mut make = |c: LinearRgba| materials.add(StandardMaterial { base_color: c.into(), perceptual_roughness: 0.6, ..default() });
            (make(outer), make(inner))
        })
        .clone()
}

/// Spawn, update and despawn pieces for the keys the view changed.
#[allow(clippy::too_many_arguments)]
fn project(
    mut commands: Commands,
    view: Res<ViewState>,
    land: Option<Res<Land>>,
    rules: Option<Res<Rules>>,
    paint: Option<Res<Paint>>,
    font: Res<UiFont>,
    mut changes: MessageReader<ViewChanged>,
    mut pieces: ResMut<Pieces>,
    mut shapes: ResMut<Shapes>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut units: Query<&mut UnitPiece>,
    doomed: Query<(), With<crate::present::Doomed>>,
) {
    let (Some(v), Some(land), Some(rules), Some(paint)) = (view.view(), land, rules, paint) else {
        changes.clear();
        return;
    };
    let mut unit_keys = Vec::new();
    let mut city_keys = Vec::new();
    let mut all = rules.is_added() || paint.is_added();
    for ViewChanged(c) in changes.read() {
        all |= c.all;
        unit_keys.extend(c.units.iter().copied());
        city_keys.extend(c.cities.iter().copied());
    }
    if all {
        unit_keys = pieces.units.keys().copied().chain(v.units.iter().map(|u| (u.id, u.r#gen))).collect();
        city_keys = pieces.cities.keys().copied().chain(v.cities.iter().map(|c| (c.id, c.r#gen))).collect();
    }
    if unit_keys.is_empty() && city_keys.is_empty() {
        return;
    }
    let relief = &land.0;
    for k in unit_keys {
        let unit = v.units.binary_search_by_key(&k, |u| (u.id, u.r#gen)).ok().map(|i| &v.units[i]);
        match (unit, pieces.units.get(&k).copied()) {
            (None, Some(e)) => {
                // A unit whose fall is still to be played stays until presentation lets it go.
                if !doomed.contains(e) {
                    commands.entity(e).despawn();
                }
                pieces.units.remove(&k);
            }
            (Some(u), Some(e)) => {
                if let Ok(mut piece) = units.get_mut(e) {
                    piece.tile = u.tile;
                    piece.health = u.health;
                    piece.embarked = u.embarked;
                }
            }
            (Some(u), None) => {
                let class = Class::of(&rules, u.base);
                let (body, band) = unit_meshes(&mut shapes, &mut meshes, class);
                let (outer, inner) = livery(&mut shapes, &mut materials, relief, u.owner);
                let hull = shapes.hull.get_or_insert_with(|| meshes.add(Cuboid::new(0.42, 0.08, 0.2).mesh().build().translated_by(Vec3::Y * 0.02))).clone();
                let e = commands
                    .spawn((
                        UnitPiece {
                            key: k,
                            owner: u.owner,
                            class,
                            tile: u.tile,
                            health: u.health,
                            embarked: u.embarked,
                            offset: Vec2::ZERO,
                            heading: 0.0,
                        },
                        Transform::default(),
                        Visibility::default(),
                    ))
                    .with_children(|p| {
                        p.spawn((Mesh3d(body), MeshMaterial3d(outer.clone())));
                        if class != Class::Air && class != Class::Naval {
                            p.spawn((Mesh3d(band), MeshMaterial3d(inner.clone()), Transform::from_xyz(0.0, 0.1, 0.0)));
                        }
                        if u.embarked {
                            p.spawn((Mesh3d(hull), MeshMaterial3d(inner)));
                        }
                    })
                    .id();
                let (outer_c, inner_c) =
                    relief.liveries.get(&u.owner).copied().unwrap_or((LinearRgba::gray(0.5), LinearRgba::WHITE));
                commands.spawn((
                    Badge(e),
                    Node {
                        position_type: PositionType::Absolute,
                        width: Val::Px(18.0),
                        height: Val::Px(18.0),
                        justify_content: JustifyContent::Center,
                        align_items: AlignItems::Center,
                        border_radius: BorderRadius::MAX,
                        ..default()
                    },
                    BackgroundColor(outer_c.into()),
                    Visibility::Hidden,
                    children![
                        (
                            Text::new(class.glyph()),
                            TextFont { font: font.semibold.clone().into(), font_size: FontSize::Px(10.0), ..default() },
                            TextColor(inner_c.into()),
                        ),
                        (
                            BadgeHealth,
                            Node {
                                position_type: PositionType::Absolute,
                                top: Val::Px(20.0),
                                left: Val::Px(-2.0),
                                width: Val::Px(22.0),
                                height: Val::Px(3.0),
                                ..default()
                            },
                            BackgroundColor(Color::srgb(0.3, 0.8, 0.3)),
                        )
                    ],
                ));
                pieces.units.insert(k, e);
            }
            (None, None) => {}
        }
    }
    for k in city_keys {
        let city = v.cities.binary_search_by_key(&k, |c| (c.id, c.r#gen)).ok().map(|i| &v.cities[i]);
        if let Some(e) = pieces.cities.remove(&k) {
            commands.entity(e).despawn();
        }
        let Some(c) = city else { continue };
        let capital = c.own.as_ref().is_some_and(|o| o.capital);
        let walls = c.own.as_ref().is_some_and(|o| {
            o.buildings.iter().any(|b| rules.tables.buildings.iter().any(|row| row.id == *b && row.name == "Walls"))
        });
        let Some(mesh) = city_mesh(relief, c.centre, c.id, c.population, capital, walls) else { continue };
        let e = commands
            .spawn((
                CityPiece { centre: c.centre },
                Mesh3d(meshes.add(mesh)),
                MeshMaterial3d(paint.objects.clone()),
                Transform::default(),
            ))
            .id();
        let (outer, inner) = relief.liveries.get(&c.owner).copied().unwrap_or((LinearRgba::gray(0.5), LinearRgba::WHITE));
        let health = c.own.as_ref().map(|o| (o.health, o.max_health));
        let label = if capital { format!("★ {}", c.name) } else { c.name.clone() };
        let banner = commands
            .spawn((
                Banner(e),
                Node {
                    position_type: PositionType::Absolute,
                    height: Val::Px(22.0),
                    padding: UiRect::right(Val::Px(8.0)),
                    column_gap: Val::Px(6.0),
                    align_items: AlignItems::Center,
                    border_radius: BorderRadius::all(Val::Px(4.0)),
                    ..default()
                },
                BackgroundColor(outer.into()),
                Visibility::Hidden,
            ))
            .with_children(|b| {
                b.spawn((
                    Node {
                        width: Val::Px(22.0),
                        height: Val::Px(22.0),
                        justify_content: JustifyContent::Center,
                        align_items: AlignItems::Center,
                        border_radius: BorderRadius::MAX,
                        ..default()
                    },
                    BackgroundColor(inner.into()),
                    children![(
                        Text::new(c.population.to_string()),
                        TextFont { font: font.semibold.clone().into(), font_size: FontSize::Px(12.0), ..default() },
                        TextColor(outer.into()),
                    )],
                ));
                b.spawn((
                    Text::new(label),
                    TextFont { font: font.semibold.clone().into(), font_size: FontSize::Px(14.0), ..default() },
                    TextColor(inner.into()),
                ));
                if let Some((hp, max)) = health.filter(|(hp, max)| hp < max && *max > 0) {
                    b.spawn((
                        Node {
                            position_type: PositionType::Absolute,
                            bottom: Val::Px(-5.0),
                            left: Val::Px(0.0),
                            width: Val::Percent(100.0 * hp as f32 / max as f32),
                            height: Val::Px(3.0),
                            ..default()
                        },
                        BackgroundColor(Color::srgb(0.85, 0.3, 0.25)),
                    ));
                }
            })
            .id();
        let _ = banner;
        pieces.cities.insert(k, e);
    }
}

/// Blocks within 0.78 m of the centre, fixed by the city id; a capital's tower; walls.
pub fn city_mesh(relief: &Relief, centre: u16, id: u32, population: u32, capital: bool, walls: bool) -> Option<Mesh> {
    let c = relief.grid.centre(centre);
    let mut b = Builder::default();
    let (wall, roof) = (srgb(0xCDBFA6), srgb(0x9C5B42));
    let count = (4 + 2 * population as usize).min(30);
    let at = |p: Vec2| hex::world(p, relief.height(p).unwrap_or(0.0).max(0.0));
    let mut placed: Vec<(Vec2, f32)> = Vec::new();
    let mut tries = 0;
    while placed.len() < count && tries < count * 12 {
        tries += 1;
        let n = id.wrapping_mul(131).wrapping_add(tries as u32);
        let r = 0.78 * hash1(n, 41).sqrt();
        let a = std::f32::consts::TAU * hash1(n, 43);
        let p = c + Vec2::from_angle(a) * r;
        let size = 0.08 + 0.08 * hash1(n, 47);
        if placed.iter().any(|(q, s)| q.distance(p) < (s + size) * 0.75) {
            continue;
        }
        let height = 0.08 + 0.22 * hash1(n, 53) * (1.0 - r / 0.9);
        b.cuboid(at(p), Vec3::new(size, height, size * (0.8 + 0.4 * hash1(n, 59))), wall, roof);
        placed.push((p, size));
    }
    if capital {
        b.cuboid(at(c), Vec3::new(0.12, 0.42, 0.12), wall, roof);
        b.frustum(at(c) + Vec3::Y * 0.42, 0.1, 0.0, 0.1, 6, roof);
    }
    if walls {
        let ring: Vec<Vec2> = (0..=36).map(|i| c + Vec2::from_angle(std::f32::consts::TAU * i as f32 / 36.0) * 0.85).collect();
        for pair in ring.windows(2) {
            let mid = (pair[0] + pair[1]) / 2.0;
            b.cuboid(at(mid), Vec3::new(0.16, 0.10, 0.04), wall, wall);
        }
    }
    b.finish()
}

/// Spread units sharing a tile, and stand each on its tile unless presentation holds it.
fn stand(
    land: Option<Res<Land>>,
    mut units: Query<(&mut UnitPiece, &mut Transform, Option<&Held>)>,
) {
    let Some(land) = land else { return };
    let relief = &land.0;
    let mut by_tile: HashMap<u16, Vec<(bool, Key)>> = HashMap::new();
    for (piece, _, _) in &units {
        by_tile.entry(piece.tile).or_default().push((!piece.class.military(), piece.key));
    }
    for list in by_tile.values_mut() {
        list.sort();
    }
    for (mut piece, mut transform, held) in &mut units {
        let list = &by_tile[&piece.tile];
        let i = list.iter().position(|(_, k)| *k == piece.key).unwrap_or(0);
        piece.offset = match list.len() {
            1 => Vec2::ZERO,
            2 => Vec2::new(if i == 0 { -0.22 } else { 0.22 }, 0.0),
            n => Vec2::from_angle(std::f32::consts::TAU * i as f32 / n as f32) * 0.25,
        };
        let at = match held {
            Some(Held(p)) => *p,
            None => {
                let g = relief.grid.centre(piece.tile) + piece.offset;
                let lift = if piece.class == Class::Air { 0.35 } else { 0.0 };
                hex::world(g, relief.height(g).unwrap_or(0.0).max(0.0) + lift)
            }
        };
        transform.translation = at;
        transform.rotation = Quat::from_rotation_y(piece.heading);
    }
}

/// Screen size of `metres` at a world point.
fn pixels(camera: &Camera, camera_at: &GlobalTransform, world: Vec3, metres: f32) -> f32 {
    let depth = (world - camera_at.translation()).length().max(0.1);
    let height = camera.logical_viewport_size().map_or(1000.0, |s| s.y);
    metres / (2.0 * depth * (FOV_DEGREES / 2.0).to_radians().tan()) * height
}

fn badges(
    cameras: Query<(&Camera, &GlobalTransform), With<MapCamera>>,
    units: Query<(&UnitPiece, &GlobalTransform)>,
    mut badges: Query<(Entity, &Badge, &mut Node, &mut Visibility, &Children)>,
    mut bars: Query<(&mut Node, &mut BackgroundColor), (With<BadgeHealth>, Without<Badge>)>,
    mut commands: Commands,
) {
    let Ok((camera, at)) = cameras.single() else { return };
    for (e, badge, mut node, mut visibility, children) in &mut badges {
        let Ok((unit, transform)) = units.get(badge.0) else {
            commands.entity(e).despawn();
            continue;
        };
        let world = transform.translation() + Vec3::Y * 0.55;
        let Ok(screen) = camera.world_to_viewport(at, world) else {
            *visibility = Visibility::Hidden;
            continue;
        };
        let size = pixels(camera, at, world, 0.24).clamp(10.0, 28.0);
        node.width = Val::Px(size);
        node.height = Val::Px(size);
        node.left = Val::Px(screen.x - size / 2.0);
        node.top = Val::Px(screen.y - size / 2.0);
        *visibility = Visibility::Inherited;
        for child in children.iter() {
            if let Ok((mut bar, mut colour)) = bars.get_mut(child) {
                let hurt = unit.health < 100;
                bar.display = if hurt { Display::Flex } else { Display::None };
                bar.top = Val::Px(size + 2.0);
                bar.width = Val::Px(size * 1.3 * unit.health as f32 / 100.0);
                bar.left = Val::Px(-size * 0.15);
                colour.0 = Color::srgb(1.0 - unit.health as f32 / 100.0, 0.75 * unit.health as f32 / 100.0 + 0.1, 0.25);
            }
        }
    }
}

fn banners(
    cameras: Query<(&Camera, &GlobalTransform), With<MapCamera>>,
    land: Option<Res<Land>>,
    cities: Query<&CityPiece>,
    mut banners: Query<(Entity, &Banner, &mut Node, &mut Visibility, &ComputedNode)>,
    mut commands: Commands,
) {
    let (Ok((camera, at)), Some(land)) = (cameras.single(), land) else { return };
    for (e, banner, mut node, mut visibility, computed) in &mut banners {
        let Ok(city) = cities.get(banner.0) else {
            commands.entity(e).despawn();
            continue;
        };
        let g = land.0.grid.centre(city.centre);
        let world = hex::world(g, land.0.height(g).unwrap_or(0.0).max(0.0) + 0.9);
        let Ok(screen) = camera.world_to_viewport(at, world) else {
            *visibility = Visibility::Hidden;
            continue;
        };
        let width = computed.size().x * computed.inverse_scale_factor();
        node.left = Val::Px(screen.x - width / 2.0);
        node.top = Val::Px(screen.y - 11.0);
        *visibility = Visibility::Inherited;
    }
}
