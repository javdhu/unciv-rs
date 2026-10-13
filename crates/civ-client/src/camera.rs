//! The map camera and the light (docs/VISUAL-SPEC.md, Camera; Light and image).
//!
//! Fixed pitch and yaw, north up. The wheel and pinch zoom toward the cursor, easing with a 0.08 s
//! half-life; a drag holds the grabbed ground point under the cursor; keys pan at one camera distance per
//! second; two-finger scroll pans. Haze starts 30 m beyond the target and ends 200 m beyond it.

use crate::hex::{self, PITCH};
use crate::map::Land;
use bevy::input::gestures::PinchGesture;
use bevy::input::mouse::{MouseScrollUnit, MouseWheel};
use bevy::light::{AmbientLight, CascadeShadowConfigBuilder, DirectionalLightShadowMap};
use bevy::pbr::{DistanceFog, FogFalloff};
use bevy::prelude::*;
use bevy::window::PrimaryWindow;

pub const PITCH_DEGREES: f32 = 52.0;
pub const FOV_DEGREES: f32 = 35.0;
pub const MIN_DISTANCE: f32 = 8.0;
pub const DEFAULT_DISTANCE: f32 = 24.0;
pub const MAX_DISTANCE: f32 = 200.0;
const ZOOM_STEP: f32 = 1.15;
const HALF_LIFE: f32 = 0.08;
/// A press that moves less than this many pixels is a click, not a drag.
pub const CLICK_SLOP: f32 = 4.0;

#[derive(Component)]
pub struct MapCamera {
    /// The ground point looked at.
    pub target: Vec2,
    pub distance: f32,
    /// The distance the zoom eases toward.
    pub goal: f32,
    pub max_distance: f32,
    /// Ground point held under a screen point while a zoom eases.
    anchor: Option<(Vec2, Vec2)>,
    /// Ground point grabbed, the screen point pressed, and whether it has moved past a click.
    drag: Option<(Vec2, Vec2, bool)>,
    framed: bool,
}

impl MapCamera {
    /// Set the view at once, with no easing: the screenshot script uses it.
    pub fn jump(&mut self, target: Vec2, distance: f32) {
        self.target = target;
        self.distance = distance.clamp(MIN_DISTANCE, self.max_distance);
        self.goal = self.distance;
        self.anchor = None;
    }

    /// Whether the last press has turned into a drag.
    pub fn dragging(&self) -> bool {
        self.drag.is_some_and(|d| d.2)
    }
}

pub fn plugin(app: &mut App) {
    let shadow_map = if cfg!(target_arch = "wasm32") { 1024 } else { 2048 };
    app.insert_resource(DirectionalLightShadowMap { size: shadow_map })
        .add_systems(Startup, spawn)
        .add_systems(Update, (frame, input, place).chain());
}

fn spawn(mut commands: Commands) {
    commands.spawn((
        Camera3d::default(),
        // The UI (inspector, resource markers) follows this camera, also when it renders into the review
        // shots' image.
        IsDefaultUiCamera,
        Projection::Perspective(PerspectiveProjection { fov: FOV_DEGREES.to_radians(), ..default() }),
        MapCamera {
            target: Vec2::ZERO,
            distance: DEFAULT_DISTANCE,
            goal: DEFAULT_DISTANCE,
            max_distance: MAX_DISTANCE,
            anchor: None,
            drag: None,
            framed: false,
        },
        DistanceFog {
            color: Color::srgb_u8(0xC7, 0xD3, 0xDF),
            falloff: FogFalloff::Linear { start: DEFAULT_DISTANCE + 30.0, end: DEFAULT_DISTANCE + 200.0 },
            directional_light_color: Color::NONE,
            ..default()
        },
        // With the sun below, shadowed slopes keep about 40% of a lit face's luminance.
        AmbientLight { color: Color::srgb_u8(0xC9, 0xD6, 0xE8), brightness: 400.0, ..default() },
        Transform::default(),
    ));
    // Sun from the north-west (azimuth 315°), 42° up.
    let (az, el) = (315f32.to_radians(), 42f32.to_radians());
    let toward_sun = Vec3::new(az.sin() * el.cos(), el.sin(), -az.cos() * el.cos());
    commands.spawn((
        DirectionalLight { illuminance: 3_000.0, shadow_maps_enabled: true, ..default() },
        Transform::from_translation(toward_sun * 50.0).looking_at(Vec3::ZERO, Vec3::Y),
        CascadeShadowConfigBuilder {
            num_cascades: 2,
            first_cascade_far_bound: 30.0,
            maximum_distance: 120.0,
            ..default()
        }
        .build(),
    ));
}

/// Centre on the map the first time it is known, and set how far out the zoom may go.
fn frame(land: Option<Res<Land>>, mut cameras: Query<&mut MapCamera>) {
    let Some(land) = land else { return };
    let Ok(mut camera) = cameras.single_mut() else { return };
    let (west, east, south, north) = land.0.grid.bounds();
    // The whole map's width fits at 16:9.
    let fit = (east - west) / (2.0 * (FOV_DEGREES / 2.0).to_radians().tan() * 16.0 / 9.0);
    camera.max_distance = fit.clamp(DEFAULT_DISTANCE, MAX_DISTANCE);
    if !camera.framed {
        camera.framed = true;
        let centre = Vec2::new((west + east) / 2.0, (north + south) / 2.0);
        camera.jump(centre, DEFAULT_DISTANCE);
    }
}

/// The ground point (at land height) under a screen point.
pub fn ground_under(camera: &Camera, transform: &GlobalTransform, screen: Vec2) -> Option<Vec2> {
    let ray = camera.viewport_to_world(transform, screen).ok()?;
    let t = (0.1 - ray.origin.y) / ray.direction.y;
    (t > 0.0).then(|| hex::ground(ray.origin + *ray.direction * t))
}

#[allow(clippy::too_many_arguments)]
fn input(
    time: Res<Time<Real>>,
    keys: Res<ButtonInput<KeyCode>>,
    buttons: Res<ButtonInput<MouseButton>>,
    mut wheel: MessageReader<MouseWheel>,
    mut pinch: MessageReader<PinchGesture>,
    window: Query<&Window, With<PrimaryWindow>>,
    mut cameras: Query<(&Camera, &GlobalTransform, &mut MapCamera)>,
) {
    let Ok((camera, transform, mut map)) = cameras.single_mut() else { return };
    let cursor = window.single().ok().and_then(|w| w.cursor_position());
    let height = window.single().map_or(1000.0, |w| w.height());
    let metres_per_pixel = 2.0 * map.distance * (FOV_DEGREES / 2.0).to_radians().tan() / height;
    let mut zoom = 1.0f32;
    for event in wheel.read() {
        match event.unit {
            MouseScrollUnit::Line => zoom *= ZOOM_STEP.powf(-event.y),
            // Trackpads scroll in pixels: two fingers pan.
            MouseScrollUnit::Pixel => {
                map.target += Vec2::new(-event.x, event.y) * metres_per_pixel;
            }
        }
    }
    for PinchGesture(delta) in pinch.read() {
        zoom *= 1.0 - delta;
    }
    if zoom != 1.0 {
        map.goal = (map.goal * zoom).clamp(MIN_DISTANCE, map.max_distance);
        map.anchor = cursor.and_then(|c| ground_under(camera, transform, c).map(|g| (g, c)));
    }
    // Drag with the left or middle button: the grabbed point stays under the cursor.
    let held = buttons.pressed(MouseButton::Left) || buttons.pressed(MouseButton::Middle);
    if buttons.just_pressed(MouseButton::Left) || buttons.just_pressed(MouseButton::Middle) {
        map.drag = cursor.and_then(|c| ground_under(camera, transform, c).map(|g| (g, c, false)));
    }
    if let (true, Some((grab, pressed, moved)), Some(c)) = (held, map.drag, cursor) {
        let moved = moved || c.distance(pressed) > CLICK_SLOP;
        if moved {
            if let Some(now) = ground_under(camera, transform, c) {
                map.target += grab - now;
            }
            map.anchor = None;
        }
        map.drag = Some((grab, pressed, moved));
    }
    if !held && map.drag.is_some() && !buttons.just_released(MouseButton::Left) {
        map.drag = None;
    }
    let mut pan = Vec2::ZERO;
    for (key, dir) in [
        (KeyCode::KeyW, Vec2::Y),
        (KeyCode::ArrowUp, Vec2::Y),
        (KeyCode::KeyS, -Vec2::Y),
        (KeyCode::ArrowDown, -Vec2::Y),
        (KeyCode::KeyA, -Vec2::X),
        (KeyCode::ArrowLeft, -Vec2::X),
        (KeyCode::KeyD, Vec2::X),
        (KeyCode::ArrowRight, Vec2::X),
    ] {
        if keys.pressed(key) {
            pan += dir;
        }
    }
    if pan != Vec2::ZERO {
        let distance = map.distance;
        map.target += pan.normalize() * distance * time.delta_secs();
        map.anchor = None;
    }
}

/// Ease the zoom, keep the zoom anchor under its screen point, hold the target on the map, place the camera.
pub fn place(
    time: Res<Time<Real>>,
    land: Option<Res<Land>>,
    mut cameras: Query<(&Camera, &mut Transform, &mut MapCamera, &mut DistanceFog)>,
) {
    let Ok((camera, mut transform, mut map, mut fog)) = cameras.single_mut() else { return };
    let ease = 0.5f32.powf(time.delta_secs() / HALF_LIFE);
    map.distance = map.goal + (map.distance - map.goal) * ease;
    if (map.distance - map.goal).abs() < 0.001 {
        map.distance = map.goal;
    }
    *transform = pose(map.target, map.distance);
    if let Some((ground, screen)) = map.anchor {
        let global = GlobalTransform::from(*transform);
        if let Some(now) = ground_under(camera, &global, screen) {
            map.target += ground - now;
            *transform = pose(map.target, map.distance);
        }
        if map.distance == map.goal {
            map.anchor = None;
        }
    }
    if let Some(land) = land {
        let grid = land.0.grid;
        let (west, east, south, north) = grid.bounds();
        if grid.wrap_x {
            map.target.x = (map.target.x - west).rem_euclid(grid.span()) + west;
        } else {
            map.target.x = map.target.x.clamp(west - 2.0 * PITCH, east + 2.0 * PITCH);
        }
        map.target.y = map.target.y.clamp(south - 3.0, north + 3.0);
        *transform = pose(map.target, map.distance);
    }
    fog.falloff = FogFalloff::Linear { start: map.distance + 30.0, end: map.distance + 200.0 };
}

pub fn pose(target: Vec2, distance: f32) -> Transform {
    let pitch = PITCH_DEGREES.to_radians();
    let look = hex::world(target, 0.0);
    // South of the target (+Z), looking north and down.
    let eye = look + Vec3::new(0.0, distance * pitch.sin(), distance * pitch.cos());
    Transform::from_translation(eye).looking_at(look, Vec3::Y)
}
