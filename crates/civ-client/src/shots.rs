//! The screenshot script for review (docs/CLIENT-BRIEF.md, M1): play a watched match to a set turn, pause,
//! let the map settle, then shoot fixed views of it and quit. The camera renders into a 1600x1000 image rather
//! than the window, so a shot never depends on whether the window is on screen.
//!
//! Natively `CIV_SHOTS=<dir> CIV_SCENE=<scene>` writes `<dir>/<scene>-<view>.png`. On the page `?shots=<scene>`
//! posts each PNG to the local server (`web/serve.py`), which writes `web/shots/<flavour>-<scene>-<view>.png`.

use crate::camera::{MapCamera, DEFAULT_DISTANCE};
use crate::client::{LinkState, Pace};
use crate::map::{Dirty, Land};
use crate::rules::Ground;
use crate::view_state::ViewState;
use bevy::prelude::*;
use bevy::camera::{ImageRenderTarget, RenderTarget};
use bevy::render::render_resource::{TextureFormat, TextureUsages};
use bevy::render::view::screenshot::{Screenshot, ScreenshotCaptured};
use civ_sim::session::Sight;
use civ_sim::Settings;

#[derive(Debug, Clone, Copy)]
pub enum Focus {
    Whole,
    Centre,
    Mountains,
    Coast,
    Seat,
}

pub struct Scene {
    pub name: String,
    pub settings: Settings,
    pub sight: Sight,
    pub turn: u32,
    pub views: Vec<(&'static str, Focus, f32)>,
}

/// The review scenes: a developed world seen whole, its fog from one seat, and other seeds' terrain.
pub fn scene(name: &str) -> Option<Scene> {
    let seed = |seed| Settings { seed, ..Settings::default() };
    let (settings, sight, turn, views) = match name {
        "world" => (seed(1), Sight::All, 60, vec![
            ("whole", Focus::Whole, 0.0),
            ("centre", Focus::Centre, DEFAULT_DISTANCE),
            ("mountains", Focus::Mountains, 12.0),
            ("coast", Focus::Coast, 12.0),
        ]),
        "fog" => (seed(1), Sight::Seat, 40, vec![
            ("seat", Focus::Seat, DEFAULT_DISTANCE),
            ("seat-wide", Focus::Seat, 48.0),
            ("whole", Focus::Whole, 0.0),
        ]),
        "seed2" => (seed(2), Sight::All, 1, vec![("whole", Focus::Whole, 0.0), ("mountains", Focus::Mountains, 12.0)]),
        "seed3" => (seed(3), Sight::All, 120, vec![
            ("whole", Focus::Whole, 0.0),
            ("centre", Focus::Centre, 16.0),
            ("coast", Focus::Coast, 10.0),
        ]),
        _ => return None,
    };
    Some(Scene { name: name.into(), settings, sight, turn, views })
}

pub enum Out {
    #[cfg(not(target_arch = "wasm32"))]
    Dir(std::path::PathBuf),
    #[cfg(target_arch = "wasm32")]
    Page,
}

#[derive(Resource)]
pub struct Shots {
    pub scene: Scene,
    out: Out,
    target: Option<Handle<Image>>,
    stage: Stage,
    frames: u32,
    index: usize,
}

#[derive(Resource, Default)]
struct Captured(usize);

#[derive(PartialEq)]
enum Stage {
    Advancing,
    Settling,
    Aiming,
    Waiting,
    Done,
}

impl Shots {
    /// The script asked for by the environment or the page, if any.
    pub fn requested() -> Option<Shots> {
        #[cfg(not(target_arch = "wasm32"))]
        let (name, out) = {
            let dir = std::env::var("CIV_SHOTS").ok()?;
            let name = std::env::var("CIV_SCENE").unwrap_or_else(|_| "world".into());
            std::fs::create_dir_all(&dir).ok()?;
            (name, Out::Dir(dir.into()))
        };
        #[cfg(target_arch = "wasm32")]
        let (name, out) = {
            let name = js_sys::Reflect::get(&js_sys::global(), &"civShots".into()).ok()?.as_string()?;
            (name, Out::Page)
        };
        let scene = scene(&name)?;
        Some(Shots { scene, out, target: None, stage: Stage::Advancing, frames: 0, index: 0 })
    }
}

pub fn plugin(app: &mut App) {
    app.init_resource::<Captured>()
        .add_systems(PostStartup, offscreen)
        .add_systems(Update, shoot.after(crate::map::MapSet::Rebuild));
}

/// Point the map camera at an image of the review size.
fn offscreen(
    mut commands: Commands,
    shots: Option<ResMut<Shots>>,
    cameras: Query<Entity, With<MapCamera>>,
    mut images: ResMut<Assets<Image>>,
) {
    let (Some(mut shots), Ok(camera)) = (shots, cameras.single()) else { return };
    let mut image = Image::new_target_texture(1600, 1000, TextureFormat::Rgba8UnormSrgb, None);
    image.texture_descriptor.usage |= TextureUsages::COPY_SRC;
    let handle = images.add(image);
    commands.entity(camera).insert(RenderTarget::Image(ImageRenderTarget::from(handle.clone())));
    shots.target = Some(handle);
}

#[allow(clippy::too_many_arguments)]
fn shoot(
    mut commands: Commands,
    shots: Option<ResMut<Shots>>,
    captured: Res<Captured>,
    view: Res<ViewState>,
    link: Res<LinkState>,
    dirty: Res<Dirty>,
    land: Option<Res<Land>>,
    mut pace: ResMut<Pace>,
    mut cameras: Query<&mut MapCamera>,
    mut exit: MessageWriter<AppExit>,
) {
    let Some(mut shots) = shots else { return };
    let turn = view.view().map_or(0, |v| v.turn);
    shots.frames += 1;
    match shots.stage {
        Stage::Advancing => {
            if turn >= shots.scene.turn || link.stopped.is_some() {
                *pace = Pace::Paused;
                shots.stage = Stage::Settling;
                shots.frames = 0;
            }
        }
        Stage::Settling => {
            if dirty.is_empty() && land.is_some() && !link.awaiting_round && shots.frames >= 90 {
                shots.stage = Stage::Aiming;
            }
        }
        Stage::Aiming => {
            if captured.0 < shots.index {
                return;
            }
            let Some(&(_, focus, distance)) = shots.scene.views.get(shots.index) else {
                shots.stage = Stage::Done;
                shots.frames = 0;
                return;
            };
            let (Some(land), Ok(mut camera), Some(v)) = (land, cameras.single_mut(), view.view()) else { return };
            let target = aim(&land, v, focus);
            let distance = if distance == 0.0 { camera.max_distance } else { distance };
            camera.jump(target, distance);
            shots.stage = Stage::Waiting;
            shots.frames = 0;
        }
        Stage::Waiting => {
            if shots.frames < 30 {
                return;
            }
            let name = format!("{}-{}", shots.scene.name, shots.scene.views[shots.index].0);
            let Some(target) = shots.target.clone() else { return };
            let mut screenshot = commands.spawn(Screenshot::image(target));
            screenshot.observe(|_: On<ScreenshotCaptured>, mut c: ResMut<Captured>| c.0 += 1);
            match &shots.out {
                #[cfg(not(target_arch = "wasm32"))]
                Out::Dir(dir) => {
                    screenshot.observe(bevy::render::view::screenshot::save_to_disk(dir.join(format!("{name}.png"))));
                }
                #[cfg(target_arch = "wasm32")]
                Out::Page => {
                    screenshot.observe(move |shot: On<ScreenshotCaptured>| post(&name, &shot.image));
                }
            }
            shots.index += 1;
            shots.stage = Stage::Aiming;
        }
        Stage::Done => {
            if captured.0 >= shots.scene.views.len() && shots.frames > 30 {
                #[cfg(target_arch = "wasm32")]
                post_done(&shots.scene.name);
                exit.write(AppExit::Success);
                commands.remove_resource::<Shots>();
            }
        }
    }
}

fn aim(land: &Land, view: &civ_sim::PlayerView, focus: Focus) -> Vec2 {
    let relief = &land.0;
    let grid = relief.grid;
    let (west, east, south, north) = grid.bounds();
    let centre = Vec2::new((west + east) / 2.0, (north + south) / 2.0);
    let known = || relief.cells.iter().enumerate().filter_map(|(i, c)| c.map(|c| (i as u16, c)));
    let best = |score: &dyn Fn(u16) -> i32| known().max_by_key(|(id, _)| (score(*id), -(*id as i32))).map(|(id, _)| grid.centre(id));
    match focus {
        Focus::Whole | Focus::Centre => centre,
        Focus::Mountains => best(&|id| {
            let mountain = |t: u16| relief.cell(t).is_some_and(|c| c.ground == Ground::Mountain) as i32;
            mountain(id) + (0..6).filter_map(|d| grid.neighbour(id, d)).map(mountain).sum::<i32>()
        })
        .unwrap_or(centre),
        Focus::Coast => best(&|id| {
            let Some(c) = relief.cell(id) else { return 0 };
            if c.is_water() {
                return 0;
            }
            let wet = (0..6).filter_map(|d| grid.neighbour(id, d)).filter(|n| relief.cell(*n).is_some_and(|c| c.is_water())).count() as i32;
            wet.min(3) + 3 * (c.rivers != 0) as i32 + 2 * (c.road > 0) as i32 + 2 * (c.improvement != crate::relief::Improvement::None) as i32
        })
        .unwrap_or(centre),
        Focus::Seat => {
            let owned: Vec<Vec2> = known().filter(|(_, c)| c.owner == Some(view.me)).map(|(id, _)| grid.centre(id)).collect();
            let seen: Vec<Vec2> = known().filter(|(_, c)| c.visible).map(|(id, _)| grid.centre(id)).collect();
            let pick = if owned.is_empty() { seen } else { owned };
            if pick.is_empty() { centre } else { pick.iter().sum::<Vec2>() / pick.len() as f32 }
        }
    }
}

#[cfg(target_arch = "wasm32")]
#[wasm_bindgen::prelude::wasm_bindgen(inline_js = "
export function civ_shot(name, bytes) {
  return fetch('/shot?name=' + encodeURIComponent((globalThis.civFlavor || 'web') + '-' + name), { method: 'POST', body: bytes });
}
export function civ_shots_done(scene) {
  return fetch('/stats', { method: 'POST', body: JSON.stringify({ shotsDone: scene, flavor: globalThis.civFlavor }) });
}")]
extern "C" {
    fn civ_shot(name: &str, bytes: &[u8]) -> js_sys::Promise;
    fn civ_shots_done(scene: &str) -> js_sys::Promise;
}

#[cfg(target_arch = "wasm32")]
fn post(name: &str, image: &Image) {
    use image::ImageEncoder;
    let Ok(dynamic) = image.clone().try_into_dynamic() else { return };
    let rgb = dynamic.to_rgb8();
    let mut png = Vec::new();
    let encoder = image::codecs::png::PngEncoder::new(&mut png);
    if encoder.write_image(rgb.as_raw(), rgb.width(), rgb.height(), image::ExtendedColorType::Rgb8).is_ok() {
        let _ = civ_shot(name, &png);
    }
}

#[cfg(target_arch = "wasm32")]
fn post_done(scene: &str) {
    let _ = civ_shots_done(scene);
}
