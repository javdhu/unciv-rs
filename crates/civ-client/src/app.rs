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
struct MenuCamera;

/// Where the app goes once started: straight into a match (shots, `CIV_WATCH`, `?match=`), or home.
#[derive(Resource, Default)]
pub struct Launch(pub Option<Start>);

/// What a match is played against.
#[derive(Clone, Debug)]
pub enum Start {
    Solo(Settings),
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
}

/// Open a match: its link and mode, then the state change runs every plugin's `OnEnter(AppState::Match)`.
pub fn enter(commands: &mut Commands, next: &mut NextState<AppState>, start: Start) {
    commands.queue(move |world: &mut World| open(world, start));
    next.set(AppState::Match);
}

/// The match's resources, before any `OnEnter(AppState::Match)` system runs.
pub fn open(world: &mut World, start: Start) {
    use crate::play::Mode;
    let (link, mode, sight) = match start {
        Start::Solo(settings) => (Link::play(settings), Mode::Play, Sight::All),
        Start::Watch(settings, sight) => (Link::watch(settings, sight), Mode::Watch, sight),
        Start::Room { server, match_id } => (Link::remote(server, match_id), Mode::Play, Sight::Seat),
    };
    world.insert_resource(link);
    world.insert_resource(mode);
    world.insert_resource(crate::hud::Eyes(sight));
    // A played match's rounds close when its seats finish, not on the client's pace.
    world.insert_resource(if mode == Mode::Play { crate::client::Pace::Paused } else { crate::client::Pace::Continuous });
}

fn launch(mut commands: Commands, mut launch: ResMut<Launch>, mut next: ResMut<NextState<AppState>>) {
    match launch.0.take() {
        Some(start) => enter(&mut commands, &mut next, start),
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
}

fn menu_camera(mut commands: Commands) {
    commands.spawn((Camera2d, MenuCamera, Persist));
}

fn menu_camera_on(on: bool) -> impl FnMut(Query<&mut Camera, With<MenuCamera>>) {
    move |mut cameras| {
        for mut camera in &mut cameras {
            camera.is_active = on;
        }
    }
}
