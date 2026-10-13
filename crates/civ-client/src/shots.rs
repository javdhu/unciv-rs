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
    /// Record this many seconds of play from `turn` instead of shooting views.
    pub record: Option<f32>,
    /// Played on seat 0 by a script (`turn_script`) rather than watched.
    pub play: bool,
    /// Hosted in a civ-server room (`CIV_SERVER`) by a script through the rooms screens (`rooms::drive`):
    /// the screens are shot on the way, and the match recorded once another player has joined.
    pub room: bool,
}

/// Frames a second in a recording; the clock steps exactly this much per frame while recording.
pub const RECORD_FPS: f32 = 30.0;

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
        // M2's review: 45 seconds of a watched match from turn 30, at 1x, following the action.
        "watch" => {
            return Some(Scene {
                name: name.into(),
                settings: seed(1),
                sight: Sight::All,
                turn: 30,
                views: Vec::new(),
                record: Some(45.0),
                play: false,
                room: false,
            });
        }
        // M3's review: one played turn from the start, driven by a script, then the other civs' turns.
        "turn" => {
            return Some(Scene {
                name: name.into(),
                settings: seed(3),
                sight: Sight::Seat,
                turn: 0,
                views: Vec::new(),
                record: Some(32.0),
                play: true,
                room: false,
            });
        }
        // M4's review: a played match fast-forwarded until neighbours are met, then each new screen in turn.
        "tour" => {
            return Some(Scene {
                name: name.into(),
                settings: seed(3),
                sight: Sight::Seat,
                turn: 0,
                views: Vec::new(),
                record: Some(36.0),
                play: true,
                room: false,
            });
        }
        // M6a's review: a match played fast until the engine waits on a decision, then End turn leading to it,
        // the city's queue, the log and the civilizations panel.
        "decide" => {
            return Some(Scene {
                name: name.into(),
                settings: Settings { seed: 7, width: 24, height: 16, majors: 2, city_states: 1, ..Settings::default() },
                sight: Sight::Seat,
                turn: 0,
                views: Vec::new(),
                record: Some(40.0),
                play: true,
                room: false,
            });
        }
        // M5's review: a room hosted here and joined from elsewhere (a browser on the invite link), then
        // its match with a deadline, seats and the waiting turn button.
        "room" => {
            return Some(Scene {
                name: name.into(),
                settings: seed(3),
                sight: Sight::Seat,
                turn: 0,
                views: Vec::new(),
                record: Some(40.0),
                play: true,
                room: true,
            });
        }
        _ => return None,
    };
    Some(Scene { name: name.into(), settings, sight, turn, views, record: None, play: false, room: false })
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
    /// Where the rooms screens' camera draws while shooting.
    menu: Option<Handle<Image>>,
    stage: Stage,
    frames: u32,
    index: usize,
}

#[derive(Resource, Default)]
struct Captured(usize);

#[derive(PartialEq)]
enum Stage {
    Advancing,
    Recording,
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
        Some(Shots { scene, out, target: None, menu: None, stage: Stage::Advancing, frames: 0, index: 0 })
    }

    /// Shoot what the rooms screens show, as `<scene>-<name>.png`.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn shoot_menu(&self, commands: &mut Commands, name: &str) {
        let (Some(target), Out::Dir(dir)) = (self.menu.clone(), &self.out) else { return };
        let path = dir.join(format!("{}-{name}.png", self.scene.name));
        commands.spawn(Screenshot::image(target)).observe(bevy::render::view::screenshot::save_to_disk(path));
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn dir(&self) -> Option<&std::path::Path> {
        match &self.out {
            Out::Dir(dir) => Some(dir),
        }
    }
}

pub fn plugin(app: &mut App) {
    app.init_resource::<Captured>()
        .add_systems(PostStartup, menu_offscreen)
        .add_systems(OnEnter(crate::app::AppState::Match), offscreen.after(crate::camera::spawn))
        .add_systems(
            Update,
            (shoot, turn_script, tour_script, room_script, decide_script)
                .chain()
                .after(crate::map::MapSet::Rebuild)
                .in_set(crate::app::InMatch),
        );
}

/// Point the rooms screens' camera at an image of the review size too.
fn menu_offscreen(
    mut commands: Commands,
    shots: Option<ResMut<Shots>>,
    cameras: Query<Entity, With<crate::app::MenuCamera>>,
    mut images: ResMut<Assets<Image>>,
) {
    let (Some(mut shots), Ok(camera)) = (shots, cameras.single()) else { return };
    let mut image = Image::new_target_texture(1600, 1000, TextureFormat::Rgba8UnormSrgb, None);
    image.texture_descriptor.usage |= TextureUsages::COPY_SRC;
    let handle = images.add(image);
    commands.entity(camera).insert(RenderTarget::Image(ImageRenderTarget::from(handle.clone())));
    shots.menu = Some(handle);
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
    mut playback: ResMut<crate::present::Playback>,
    mut cameras: Query<&mut MapCamera>,
    #[cfg_attr(not(target_arch = "wasm32"), allow(unused_mut, unused_variables))] mut exit: MessageWriter<AppExit>,
) {
    let Some(mut shots) = shots else { return };
    let turn = view.view().map_or(0, |v| v.turn);
    shots.frames += 1;
    match shots.stage {
        Stage::Advancing => {
            // Reach the start turn as fast as the engine goes.
            playback.skip = true;
            if turn >= shots.scene.turn || link.stopped.is_some() {
                playback.skip = false;
                shots.frames = 0;
                if shots.scene.record.is_some() {
                    shots.stage = Stage::Recording;
                } else {
                    *pace = Pace::Paused;
                    shots.stage = Stage::Settling;
                }
            }
        }
        Stage::Recording => {
            let total = (shots.scene.record.unwrap_or(0.0) * RECORD_FPS) as u32;
            if shots.frames > total {
                shots.stage = Stage::Done;
                shots.frames = 0;
                shots.index = total as usize;
                return;
            }
            let Some(target) = shots.target.clone() else { return };
            let name = format!("{}-{:05}", shots.scene.name, shots.frames);
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
            let expected = shots.scene.record.map_or(shots.scene.views.len(), |s| (s * RECORD_FPS) as usize);
            if captured.0 >= expected && shots.frames > 30 {
                #[cfg(target_arch = "wasm32")]
                post_done(&shots.scene.name);
                // Every shot is on disk (saved inside its capture): leave at once natively, rather than wait on
                // the render thread's shutdown, which can hang behind a readback.
                #[cfg(not(target_arch = "wasm32"))]
                std::process::exit(0);
                #[allow(unreachable_code)]
                {
                    exit.write(AppExit::Success);
                    commands.remove_resource::<Shots>();
                }
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


/// The played turn the "turn" scene records: each step at a set frame of the recording.
#[allow(clippy::too_many_arguments)]
fn turn_script(
    shots: Option<Res<Shots>>,
    view: Res<ViewState>,
    rules: Option<Res<crate::rules::Rules>>,
    land: Option<Res<Land>>,
    mut selection: ResMut<crate::play::Selection>,
    mut outbox: ResMut<crate::client::Outbox>,
    mut cameras: Query<&mut MapCamera>,
) {
    use crate::play::{entity, select_unit, unit_action};
    use civ_sim::protocol::Action;
    use civ_sim::queries::UnitActionKind;
    let (Some(shots), Some(rules), Some(land), Some(v)) = (shots, rules, land, view.view()) else { return };
    if !shots.scene.play || shots.scene.name != "turn" || shots.stage != Stage::Recording {
        return;
    }
    let mine = |name: &str| {
        v.units
            .iter()
            .filter(|u| u.owner == v.me)
            .find(|u| rules.tables.units.iter().any(|r| r.id == u.base && r.name == name))
            .map(|u| ((u.id, u.r#gen), u.tile))
    };
    let military = || {
        v.units
            .iter()
            .filter(|u| u.owner == v.me)
            .find(|u| crate::pieces::Class::of(&rules, u.base) != crate::pieces::Class::Civilian)
            .map(|u| ((u.id, u.r#gen), u.tile))
    };
    let look = |cameras: &mut Query<&mut MapCamera>, tile: u16| {
        if let Ok(mut c) = cameras.single_mut() {
            c.jump(land.0.grid.centre(tile), 14.0);
        }
    };
    match shots.frames {
        20 => {
            if let Some((settler, tile)) = mine("Settler") {
                select_unit(&mut selection, settler);
                look(&mut cameras, tile);
            }
        }
        110 => {
            if let Some(settler) = selection.unit {
                if let Some(action) = unit_action(UnitActionKind::FoundCity, settler, &view, &rules) {
                    outbox.submit(action);
                }
            }
        }
        190 => {
            if let Some((warrior, tile)) = military() {
                select_unit(&mut selection, warrior);
                look(&mut cameras, tile);
            }
        }
        260 => {
            // The reachable tile farthest from where the unit stands.
            if let (Some(unit), Some(here)) = (selection.unit, military().map(|m| m.1)) {
                let from = land.0.grid.centre(here);
                let far = selection.reach.iter().max_by(|a, b| {
                    let da = land.0.grid.centre(a.tile).distance(from);
                    let db = land.0.grid.centre(b.tile).distance(from);
                    da.total_cmp(&db)
                });
                if let Some(r) = far {
                    outbox.submit(Action::MoveUnit { unit: entity(unit), to: r.tile });
                }
            }
        }
        340 => {
            if let Some(city) = v.cities.iter().find(|c| c.owner == v.me) {
                crate::play::select_city(&mut selection, (city.id, city.r#gen));
                look(&mut cameras, city.centre);
            }
        }
        400 => {
            if let (Some(city), Some(options)) = (selection.city, selection.city_options.clone()) {
                if let Some(choice) = options.choices.first() {
                    outbox.submit(Action::SetProduction { city: entity(city), queue: vec![choice.construction.clone()] });
                }
            }
        }
        460 => {
            selection.city = None;
            selection.research_open = true;
        }
        540 => {
            if let Some(tech) = selection.civ_options.as_ref().and_then(|o| o.research.first()).map(|r| r.id) {
                outbox.submit(Action::SetResearch { technology: tech });
            }
            selection.research_open = false;
        }
        600 => {
            outbox.submit(Action::FinishTurn);
            selection.unit = None;
            if let Ok(mut c) = cameras.single_mut() {
                let d = 30.0;
                let t = c.target;
                c.jump(t, d);
            }
        }
        _ => {}
    }
}


/// The "tour" scene: found a city, end turns quickly until a neighbour is met (or turn 40), then show the tech
/// tree, the policies, the city's tiles, the Civilizations panel and a trade window, four seconds each.
#[allow(clippy::too_many_arguments)]
/// The decide scene: found a city, end turns quickly (raiders come to the idle warrior) until a decision is
/// pending, then show it through End turn, a city with a queue, the log and the civilizations panel.
#[allow(clippy::too_many_arguments)]
fn decide_script(
    mut shots: Option<ResMut<Shots>>,
    view: Res<ViewState>,
    rules: Option<Res<crate::rules::Rules>>,
    land: Option<Res<Land>>,
    mut selection: ResMut<crate::play::Selection>,
    mut outbox: ResMut<crate::client::Outbox>,
    mut screens: ResMut<crate::screens::Screens>,
    mut decisions: ResMut<crate::decisions::Decisions>,
    mut log: ResMut<crate::log::NoteLog>,
    mut playback: ResMut<crate::present::Playback>,
    mut cameras: Query<&mut MapCamera>,
    mut phase: Local<Option<(u32, u32)>>,
) {
    use crate::play::{select_city, select_unit, unit_action};
    use civ_sim::protocol::Action;
    use civ_sim::queries::UnitActionKind;
    let (Some(shots), Some(rules), Some(land), Some(v)) = (shots.as_deref_mut(), rules, land, view.view()) else { return };
    if shots.scene.name != "decide" || shots.stage != Stage::Recording {
        return;
    }
    let frame = shots.frames;
    let (step, began) = *phase.get_or_insert((0, frame));
    let since = frame - began;
    let next = |phase: &mut Option<(u32, u32)>| *phase = Some((step + 1, frame));
    let look = |cameras: &mut Query<&mut MapCamera>, tile: u16, d: f32| {
        if let Ok(mut c) = cameras.single_mut() {
            c.jump(land.0.grid.centre(tile), d);
        }
    };
    match step {
        0 if since >= 15 => {
            let settler = v.units.iter().filter(|u| u.owner == v.me).find(|u| rules.tables.units.iter().any(|r| r.id == u.base && r.name == "Settler"));
            if let Some(u) = settler {
                select_unit(&mut selection, (u.id, u.r#gen));
                if let Some(a) = unit_action(UnitActionKind::FoundCity, (u.id, u.r#gen), &view, &rules) {
                    outbox.submit(a);
                }
            }
            next(&mut phase);
        }
        1 => {
            // Fast turns until the engine waits on something, or turn 80.
            playback.skip = true;
            selection.unit = None;
            if !decisions.list.is_empty() || v.turn >= 80 {
                playback.skip = false;
                next(&mut phase);
            } else if !outbox.busy() && since % 3 == 0 {
                if v.civs.iter().find(|c| c.id == v.me).is_some_and(|c| c.researching.is_none()) {
                    if let Some(t) = selection.civ_options.as_ref().and_then(|o| o.research.first()).map(|r| r.id) {
                        outbox.submit(Action::SetResearch { technology: t });
                    }
                }
                outbox.submit(Action::FinishTurn);
            }
        }
        // End turn now names the decision; then it is opened as a click would.
        2 if since >= 90 => {
            if let Some(d) = decisions.next().cloned() {
                crate::decisions::open(d, &mut decisions, &mut selection, &mut screens);
                if let Some(t) = selection.unit.and_then(|k| v.units.iter().find(|u| (u.id, u.r#gen) == k)) {
                    look(&mut cameras, t.tile, 12.0);
                }
            }
            next(&mut phase);
        }
        3 if since >= 120 => {
            decisions.open = None;
            selection.unit = None;
            selection.research_open = false;
            screens.policies = false;
            if let Some(city) = v.cities.iter().find(|c| c.owner == v.me) {
                select_city(&mut selection, (city.id, city.r#gen));
                look(&mut cameras, city.centre, 12.0);
            }
            next(&mut phase);
        }
        // Three items queued once the city's options are in, as the + buttons would.
        4 if since >= 30 => {
            if let (Some(key), Some(o)) = (selection.city, &selection.city_options) {
                let queue: Vec<_> = o.choices.iter().filter(|c| c.refusal.is_none()).map(|c| c.construction.clone()).take(3).collect();
                outbox.submit(Action::SetProduction { city: crate::play::entity(key), queue });
            }
            next(&mut phase);
        }
        5 if since >= 120 => {
            selection.city = None;
            log.open = true;
            next(&mut phase);
        }
        6 if since >= 120 => {
            log.open = false;
            screens.civs = true;
            next(&mut phase);
        }
        7 if since >= 120 => {
            screens.civs = false;
            next(&mut phase);
        }
        _ => {}
    }
    // Stop the recording once the scene is over.
    if *phase == Some((8, frame)) {
        shots.scene.record = Some(frame as f32 / RECORD_FPS);
    }
}

/// The room scene's turns: found the capital, keep research going, and end each turn (the other seat's
/// player may not, so rounds also close at the deadline).
#[allow(clippy::too_many_arguments)]
fn room_script(
    shots: Option<Res<Shots>>,
    view: Res<ViewState>,
    rules: Option<Res<crate::rules::Rules>>,
    land: Option<Res<Land>>,
    mut selection: ResMut<crate::play::Selection>,
    mut outbox: ResMut<crate::client::Outbox>,
    mut cameras: Query<&mut MapCamera>,
    mut founded: Local<bool>,
) {
    use crate::play::{select_unit, unit_action};
    use civ_sim::protocol::Action;
    use civ_sim::queries::UnitActionKind;
    let (Some(shots), Some(rules), Some(land), Some(update)) = (shots, rules, land, view.update()) else { return };
    if !shots.scene.room || shots.stage != Stage::Recording {
        return;
    }
    let v = &update.view;
    if !*founded && shots.frames >= 20 {
        *founded = true;
        let settler = v.units.iter().filter(|u| u.owner == v.me).find(|u| rules.tables.units.iter().any(|r| r.id == u.base && r.name == "Settler"));
        if let Some(u) = settler {
            select_unit(&mut selection, (u.id, u.r#gen));
            if let Ok(mut c) = cameras.single_mut() {
                c.jump(land.0.grid.centre(u.tile), 18.0);
            }
            if let Some(a) = unit_action(UnitActionKind::FoundCity, (u.id, u.r#gen), &view, &rules) {
                outbox.submit(a);
            }
        }
        return;
    }
    if *founded && !update.ready && !outbox.busy() && shots.frames % 90 == 0 {
        if v.civs.iter().find(|c| c.id == v.me).is_some_and(|c| c.researching.is_none()) {
            if let Some(t) = selection.civ_options.as_ref().and_then(|o| o.research.first()).map(|r| r.id) {
                outbox.submit(Action::SetResearch { technology: t });
            }
        }
        selection.unit = None;
        outbox.submit(Action::FinishTurn);
    }
}

fn tour_script(
    mut shots: Option<ResMut<Shots>>,
    view: Res<ViewState>,
    rules: Option<Res<crate::rules::Rules>>,
    land: Option<Res<Land>>,
    mut selection: ResMut<crate::play::Selection>,
    mut outbox: ResMut<crate::client::Outbox>,
    mut screens: ResMut<crate::screens::Screens>,
    mut playback: ResMut<crate::present::Playback>,
    mut cameras: Query<&mut MapCamera>,
    mut phase: Local<Option<(u32, u32)>>,
) {
    use crate::play::{select_city, select_unit, unit_action};
    use civ_sim::protocol::Action;
    use civ_sim::queries::UnitActionKind;
    let (Some(shots), Some(rules), Some(land), Some(v)) = (shots.as_deref_mut(), rules, land, view.view()) else { return };
    if shots.scene.name != "tour" || shots.stage != Stage::Recording {
        return;
    }
    let frame = shots.frames;
    let look = |cameras: &mut Query<&mut MapCamera>, tile: u16, d: f32| {
        if let Ok(mut c) = cameras.single_mut() {
            c.jump(land.0.grid.centre(tile), d);
        }
    };
    // Phase: (index, frame it began).
    let (step, began) = phase.get_or_insert((0, frame));
    let since = frame - *began;
    let next = |phase: &mut Option<(u32, u32)>| *phase = Some((phase.unwrap().0 + 1, frame));
    match *step {
        0 if since >= 20 => {
            let settler = v.units.iter().filter(|u| u.owner == v.me).find(|u| rules.tables.units.iter().any(|r| r.id == u.base && r.name == "Settler"));
            if let Some(u) = settler {
                select_unit(&mut selection, (u.id, u.r#gen));
                look(&mut cameras, u.tile, 18.0);
                if let Some(a) = unit_action(UnitActionKind::FoundCity, (u.id, u.r#gen), &view, &rules) {
                    outbox.submit(a);
                }
            }
            next(&mut phase);
        }
        1 => {
            // End turns quickly, setting research when asked, until someone is met.
            playback.skip = true;
            selection.unit = None;
            let met = !v.relations.is_empty();
            if (met && v.turn >= 8) || v.turn >= 40 {
                playback.skip = false;
                next(&mut phase);
            } else if !outbox.busy() && since % 3 == 0 {
                if v.civs.iter().find(|c| c.id == v.me).is_some_and(|c| c.researching.is_none()) {
                    if let Some(t) = selection.civ_options.as_ref().and_then(|o| o.research.first()).map(|r| r.id) {
                        outbox.submit(Action::SetResearch { technology: t });
                    }
                }
                outbox.submit(Action::FinishTurn);
            }
        }
        2 => {
            selection.research_open = true;
            screens.policies = false;
            if since >= 120 {
                next(&mut phase);
            }
        }
        3 => {
            selection.research_open = false;
            screens.policies = true;
            if since >= 120 {
                next(&mut phase);
            }
        }
        4 => {
            screens.policies = false;
            if since == 1 {
                if let Some(city) = v.cities.iter().find(|c| c.owner == v.me) {
                    select_city(&mut selection, (city.id, city.r#gen));
                    look(&mut cameras, city.centre, 12.0);
                }
            }
            if since >= 120 {
                next(&mut phase);
            }
        }
        5 => {
            selection.city = None;
            screens.civs = true;
            if since >= 120 {
                next(&mut phase);
            }
        }
        6 => {
            if since == 1 {
                if let Some(r) = v.relations.first() {
                    screens.trade = Some(crate::screens::Trade {
                        with: r.civ,
                        give: vec![civ_sim::protocol::TradeItem::Gold { amount: 50 }],
                        get: vec![civ_sim::protocol::TradeItem::OpenBorders],
                        treaty: false,
                    });
                }
            }
            if since >= 120 {
                screens.trade = None;
                screens.civs = false;
                next(&mut phase);
            }
        }
        _ => {}
    }
    // Stop the recording once the tour is over.
    if *phase == Some((7, frame)) {
        shots.scene.record = Some(frame as f32 / RECORD_FPS);
    }
}
