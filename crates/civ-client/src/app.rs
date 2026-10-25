// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Where the app is: the rooms screens or a match (docs/ROOMS-SPEC.md, R1).
//!
//! A match lives from entering `AppState::Match` to leaving it, and the next one starts clean in the same
//! process. Invariant: leaving a match despawns every top-level entity with a `Transform` or a `Node` that is
//! not marked `Persist`, and every plugin resets its own match resources on `OnExit(AppState::Match)`
//! (`tests/rooms.rs` checks that the entity count returns to what it was before the match).

use crate::link::Link;
use bevy::prelude::*;
use civ_sim::session::Sight;
use civ_sim::Settings;

#[derive(States, Default, Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum AppState {
    /// Until startup is done: the initial state's `OnEnter` runs before `PreStartup`, before fonts and cameras.
    #[default]
    Boot,
    Home,
    Rooms,
    Lobby,
    Match,
}

/// Every system that needs an open match runs in this set.
#[derive(SystemSet, Clone, PartialEq, Eq, Hash, Debug)]
pub struct InMatch;

/// Kept when a match closes.
#[derive(Component)]
pub struct Persist;

/// The camera the rooms screens draw with; the match brings its own.
#[derive(Component)]
pub(crate) struct MenuCamera;

/// Where the app goes once started: straight into a match (shots, `CIV_WATCH`, `?match=`), or home.
#[derive(Resource, Default)]
pub struct Launch(pub Option<Start>);

/// What a match is played against.
#[derive(Clone, Debug)]
pub enum Start {
    Solo(Settings),
    /// Play on from a solo save.
    Resume { id: String, save: Vec<u8> },
    Watch(Settings, Sight),
    Room { server: crate::net::Server, match_id: String },
}

pub fn plugin(app: &mut App) {
    app.init_state::<AppState>()
        .configure_sets(PreUpdate, InMatch.run_if(in_state(AppState::Match)))
        .configure_sets(Update, InMatch.run_if(in_state(AppState::Match)))
        .configure_sets(PostUpdate, InMatch.run_if(in_state(AppState::Match)))
        .init_resource::<Launch>()
        .add_systems(Startup, menu_camera)
        .add_systems(PostStartup, launch)
        .add_systems(OnEnter(AppState::Match), menu_camera_on(false))
        .add_systems(OnExit(AppState::Match), (sweep, menu_camera_on(true)));
    #[cfg(not(target_arch = "wasm32"))]
    if let Some(rounds) = std::env::var("CIV_REJOIN").ok().and_then(|n| n.parse::<u32>().ok()) {
        app.insert_resource(Rejoin { rounds, frame: 0, counts: Vec::new() })
            .add_systems(Update, rejoin);
    }
}

/// `CIV_REJOIN=n`: open a solo match and leave it n times, counting entities at home after each, and exit
/// with an error if the count grew after the first (a match left something behind).
#[cfg(not(target_arch = "wasm32"))]
#[derive(Resource)]
struct Rejoin {
    rounds: u32,
    frame: u32,
    counts: Vec<usize>,
}

#[cfg(not(target_arch = "wasm32"))]
fn rejoin(
    mut commands: Commands,
    mut rejoin: ResMut<Rejoin>,
    state: Res<State<AppState>>,
    mut next: ResMut<NextState<AppState>>,
    entities: Query<Entity>,
) {
    rejoin.frame += 1;
    match state.get() {
        AppState::Match if rejoin.frame >= 240 => {
            rejoin.frame = 0;
            leave(&mut next, AppState::Home);
        }
        AppState::Home if rejoin.frame == 30 => {
            let count = entities.iter().count();
            rejoin.counts.push(count);
            info!("rejoin: {count} entities at home after {} matches", rejoin.counts.len() - 1);
            if rejoin.counts.len() as u32 > rejoin.rounds {
                let grew = rejoin.counts.windows(2).skip(1).any(|w| w[1] > w[0]);
                println!("rejoin counts {:?}: {}", rejoin.counts, if grew { "GREW" } else { "steady" });
                std::process::exit(if grew { 1 } else { 0 });
            }
            rejoin.frame = 0;
            enter(&mut commands, &mut next, Start::Solo(Settings::default()));
        }
        _ => {}
    }
}

/// Open a match: its link and mode, then the state change runs every plugin's `OnEnter(AppState::Match)`.
pub fn enter(commands: &mut Commands, next: &mut NextState<AppState>, start: Start) {
    commands.queue(move |world: &mut World| open(world, start));
    next.set(AppState::Match);
}

/// The match's resources, before any `OnEnter(AppState::Match)` system runs.
pub fn open(world: &mut World, start: Start) {
    use crate::play::Mode;
    let mut current = None;
    let (link, mode, sight) = match start {
        Start::Solo(settings) => {
            current = Some(crate::saves::new_id(settings.seed));
            (Link::play(settings), Mode::Play, Sight::All)
        }
        Start::Resume { id, save } => {
            current = Some(id);
            (Link::resume(save), Mode::Play, Sight::All)
        }
        Start::Watch(settings, sight) => (Link::watch(settings, sight), Mode::Watch, sight),
        Start::Room { server, match_id } => {
            address(Some(&match_id));
            (Link::remote(server, match_id), Mode::Play, Sight::Seat)
        }
    };
    if let Some(mut saves) = world.get_resource_mut::<crate::saves::Saves>() {
        saves.current = current;
    }
    world.insert_resource(link);
    world.insert_resource(mode);
    world.insert_resource(crate::hud::Eyes(sight));
    // A played match's rounds close when its seats finish, not on the client's pace.
    world.insert_resource(if mode == Mode::Play { crate::client::Pace::Paused } else { crate::client::Pace::Continuous });
}

fn launch(mut commands: Commands, mut launch: ResMut<Launch>, mut next: ResMut<NextState<AppState>>) {
    match launch.0.take() {
        Some(start) => enter(&mut commands, &mut next, start),
        // An invite link opens on the rooms, which join it.
        None if crate::rooms::invited().is_some() => next.set(AppState::Rooms),
        None => next.set(AppState::Home),
    }
}

/// Leave the match for the rooms screens.
pub fn leave(next: &mut NextState<AppState>, to: AppState) {
    debug_assert_ne!(to, AppState::Match);
    next.set(to);
}

/// Put a match resource back to how a new match finds it.
pub fn reset<T: Resource + Default>(mut commands: Commands) {
    commands.insert_resource(T::default());
}

/// Drop a match resource that a new match makes again.
pub fn drop<T: Resource>(mut commands: Commands) {
    commands.remove_resource::<T>();
}

fn sweep(
    mut commands: Commands,
    roots: Query<Entity, (Or<(With<Transform>, With<Node>)>, Without<ChildOf>, Without<Persist>)>,
) {
    for e in &roots {
        commands.entity(e).despawn();
    }
    commands.remove_resource::<Link>();
    commands.remove_resource::<crate::play::Mode>();
    address(None);
}

/// In the browser, the page's address names the room's match while it is open, so a reload returns to it.
fn address(_match_id: Option<&str>) {
    #[cfg(target_arch = "wasm32")]
    if let Some(history) = web_sys::window().and_then(|w| w.history().ok()) {
        let url = match _match_id {
            Some(id) => format!("?match={id}"),
            None => web_sys::window().and_then(|w| w.location().pathname().ok()).unwrap_or_else(|| "/".into()),
        };
        let _ = history.replace_state_with_url(&wasm_bindgen::JsValue::NULL, "", Some(&url));
    }
}

fn menu_camera(mut commands: Commands) {
    commands.spawn((Camera2d, MenuCamera, IsDefaultUiCamera, Persist));
}

/// The rooms screens' camera draws, and takes the UI, only outside a match.
fn menu_camera_on(on: bool) -> impl FnMut(Commands, Query<(Entity, &mut Camera), With<MenuCamera>>) {
    move |mut commands, mut cameras| {
        for (e, mut camera) in &mut cameras {
            camera.is_active = on;
            if on {
                commands.entity(e).insert(IsDefaultUiCamera);
            } else {
                commands.entity(e).remove::<IsDefaultUiCamera>();
            }
        }
    }
}
