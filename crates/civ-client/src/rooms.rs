//! The screens outside a match (docs/ROOMS-SPEC.md, R1, component 7): home (solo, watch, rooms), the rooms
//! this guest is in on a server (join by invite, create), and a room's lobby (members, nation, start).
//!
//! Every screen is rebuilt from the `Lobby` resource when an answer changes it. A lobby is read again every
//! 2 s, which is also how the server knows its members are there; once the room's match exists, the lobby
//! opens it.

use crate::api::{Answer, ApiError, Call};
use crate::app::{enter, AppState, Start};
use crate::inspect::UiFont;
use crate::net::{Http, Server};
use crate::playui::{clear, text, BUTTON, BUTTON_ON, DIM, TEXT};
use crate::roomui::ClockOffset;
use bevy::picking::hover::Hovered;
use bevy::prelude::*;
use bevy::text::{EditableText, TextReadWriteMode};
use bevy::ui_widgets::{observe, Activate, Button, TextInput};
use civ_sim::rooms::{GuestResponse, RoomRequest, RoomResponse, RoomSummary, SeatRequest};
use std::collections::HashMap;

const BACKGROUND: Color = Color::srgb(0x14 as f32 / 255.0, 0x16 as f32 / 255.0, 0x1B as f32 / 255.0);
const FIELD: Color = Color::srgba(1.0, 1.0, 1.0, 0.1);
const ERROR: Color = Color::srgb(1.0, 0.55, 0.5);

/// Map sizes offered when creating a room (the server takes 16-64 by 10-40).
const SIZES: [(u16, u16, &str); 4] = [(24, 16, "Small"), (32, 20, "Standard"), (40, 25, "Large"), (48, 30, "Huge")];
const TURN_SECONDS: [u32; 6] = [30, 60, 120, 300, 600, 1800];

/// What the rooms screens know: the server, this guest, its rooms, the open room, and calls in flight.
#[derive(Resource)]
pub struct Lobby {
    http: Http,
    next: u64,
    pending: HashMap<u64, Call>,
    guest: Option<GuestResponse>,
    rooms: Vec<RoomSummary>,
    /// The room whose lobby is open, by id, and what the server last said about it.
    open: Option<String>,
    room: Option<RoomResponse>,
    /// Playable nations of the server's rules, with their colour.
    nations: Vec<(String, Color)>,
    /// The settings a new room is created with.
    settings: RoomRequest,
    /// An invite from the launch (`?invite=`, `CIV_INVITE`), joined once the guest is known.
    invite: Option<String>,
    error: Option<String>,
    /// Seconds since the last read of the open lobby or of the room list.
    since_poll: f32,
    dirty: bool,
}

impl Lobby {
    fn new(server: Server) -> Lobby {
        Lobby {
            http: Http::new(server),
            next: 0,
            pending: HashMap::new(),
            guest: None,
            rooms: Vec::new(),
            open: None,
            room: None,
            nations: Vec::new(),
            settings: RoomRequest::default(),
            invite: invited(),
            error: None,
            since_poll: 0.0,
            dirty: true,
        }
    }

    /// Say why something failed, on the rooms screens.
    pub(crate) fn set_error(&mut self, why: String) {
        self.error = Some(why);
        self.dirty = true;
    }

    fn call(&mut self, call: Call) {
        self.next += 1;
        call.send(&self.http, self.next);
        self.pending.insert(self.next, call);
    }

    fn asking(&self, f: fn(&Call) -> bool) -> bool {
        self.pending.values().any(f)
    }
}

#[derive(Component)]
struct Root;
#[derive(Component)]
struct Content;
#[derive(Component)]
struct NameField;
#[derive(Component)]
struct InviteField;
#[derive(Component)]
struct HomeSaves;
/// The option currently chosen among several.
#[derive(Component)]
struct Chosen;

#[derive(Component, Clone, PartialEq, Debug)]
enum Press {
    Solo,
    Watch,
    Rooms,
    Home,
    SaveName,
    Join,
    Size,
    Majors,
    CityStates,
    TurnTime,
    Create,
    Open(String),
    Play(String),
    LeaveRoom(String),
    Nation(Option<String>),
    Start,
    Back,
    Load(String),
    Delete(String),
}

pub fn plugin(app: &mut App) {
    #[cfg(not(target_arch = "wasm32"))]
    app.add_systems(Update, drive.after(receive));
    app.add_systems(OnEnter(AppState::Home), home)
        .add_systems(OnEnter(AppState::Rooms), (open_rooms, frame).chain())
        .add_systems(OnEnter(AppState::Lobby), (open_lobby, frame).chain())
        .add_systems(
            Update,
            (receive, poll, draw, shade)
                .chain()
                .run_if(|s: Res<State<AppState>>| matches!(s.get(), AppState::Rooms | AppState::Lobby)),
        )
        .add_systems(Update, (home_saves, shade).run_if(in_state(AppState::Home)));
}

/// An invite to join at once: `?invite=` on the page, `CIV_INVITE` natively.
pub(crate) fn invited() -> Option<String> {
    #[cfg(not(target_arch = "wasm32"))]
    let invite = std::env::var("CIV_INVITE").ok();
    #[cfg(target_arch = "wasm32")]
    let invite = web_sys::window()
        .and_then(|w| w.location().search().ok())
        .and_then(|q| q.trim_start_matches('?').split('&').find_map(|p| p.strip_prefix("invite=").map(str::to_string)));
    invite.filter(|i| !i.is_empty() && i.chars().all(|c| c.is_ascii_alphanumeric()))
}

fn root(state: AppState) -> impl Bundle {
    (
        Root,
        DespawnOnExit(state),
        Node {
            width: Val::Percent(100.0),
            height: Val::Percent(100.0),
            justify_content: JustifyContent::Center,
            align_items: AlignItems::FlexStart,
            padding: UiRect::top(Val::Px(80.0)),
            overflow: Overflow::scroll_y(),
            ..default()
        },
        BackgroundColor(BACKGROUND),
    )
}

fn column() -> Node {
    Node {
        width: Val::Px(560.0),
        max_width: Val::Percent(100.0),
        padding: UiRect::all(Val::Px(16.0)),
        flex_direction: FlexDirection::Column,
        row_gap: Val::Px(10.0),
        ..default()
    }
}

fn row() -> Node {
    Node { column_gap: Val::Px(8.0), align_items: AlignItems::Center, flex_wrap: FlexWrap::Wrap, ..default() }
}

fn button(font: &UiFont, press: Press, label: impl Into<String>) -> impl Bundle {
    (
        press,
        Button,
        Hovered::default(),
        observe(on_press),
        Node {
            min_height: Val::Px(30.0),
            padding: UiRect::axes(Val::Px(12.0), Val::Px(5.0)),
            align_items: AlignItems::Center,
            justify_content: JustifyContent::Center,
            border_radius: BorderRadius::all(Val::Px(4.0)),
            ..default()
        },
        BackgroundColor(BUTTON),
        children![text(font, label, 14.0, TEXT)],
    )
}

fn field(font: &UiFont, value: &str, width: f32, max: usize) -> impl Bundle {
    let mut editable = EditableText::new(value);
    editable.max_characters = Some(max);
    (
        TextInput,
        editable,
        TextFont { font: font.regular.clone().into(), font_size: FontSize::Px(14.0), ..default() },
        TextColor(TEXT),
        Node { width: Val::Px(width), padding: UiRect::axes(Val::Px(8.0), Val::Px(6.0)), ..default() },
        BackgroundColor(FIELD),
    )
}

fn home(mut commands: Commands, font: Res<UiFont>) {
    commands.spawn(root(AppState::Home)).with_children(|r| {
        r.spawn(column()).with_children(|c| {
            c.spawn(text(&font, "civ", 40.0, TEXT));
            c.spawn((HomeSaves, Node { flex_direction: FlexDirection::Column, row_gap: Val::Px(8.0), ..default() }));
            c.spawn(button(&font, Press::Solo, "New game against bots"));
            c.spawn(button(&font, Press::Watch, "Watch bots play"));
            c.spawn(button(&font, Press::Rooms, "Rooms"));
        });
    });
}

/// Home's saved matches: Continue for the newest, then each with Open and Delete, and what went wrong last.
fn home_saves(
    mut commands: Commands,
    mut saves: ResMut<crate::saves::Saves>,
    font: Res<UiFont>,
    slot: Query<(Entity, Option<&Children>), With<HomeSaves>>,
    mut next: ResMut<NextState<AppState>>,
    mut drawn: Local<Option<usize>>,
) {
    for (id, save) in saves.receive() {
        enter(&mut commands, &mut next, Start::Resume { id, save });
    }
    let Ok((e, children)) = slot.single() else { return };
    let key = saves.list.len() * 1000 + saves.notice.as_ref().map_or(0, |n| n.len());
    if !saves.is_changed() && children.is_some() && *drawn == Some(key) {
        return;
    }
    *drawn = Some(key);
    clear(&mut commands, children);
    commands.entity(e).with_children(|c| {
        if let Some(notice) = &saves.notice {
            c.spawn(text(&font, notice.clone(), 13.0, ERROR));
        }
        if let Some(latest) = saves.list.first() {
            c.spawn(button(&font, Press::Load(latest.id.clone()), format!("Continue: {} · turn {}", latest.nation, latest.turn)));
        }
        if saves.list.len() > 1 {
            c.spawn(text(&font, "Saved games", 14.0, DIM));
        }
        for m in saves.list.iter().skip(1).take(8) {
            c.spawn(row()).with_children(|r| {
                r.spawn(text(&font, format!("{} · turn {} · {}×{}", m.nation, m.turn, m.width, m.height), 13.0, TEXT));
                r.spawn(button(&font, Press::Load(m.id.clone()), "Open"));
                r.spawn(button(&font, Press::Delete(m.id.clone()), "Delete"));
            });
        }
    });
}

/// The rooms and lobby screens share one frame, filled by `draw`.
fn frame(mut commands: Commands, state: Res<State<AppState>>) {
    commands.spawn(root(*state.get())).with_child((Content, column()));
}

fn open_rooms(mut commands: Commands, lobby: Option<ResMut<Lobby>>) {
    let lobby = match lobby {
        Some(lobby) => lobby.into_inner(),
        None => {
            commands.insert_resource(Lobby::new(Server::from_env()));
            return;
        }
    };
    lobby.open = None;
    lobby.room = None;
    lobby.dirty = true;
    lobby.since_poll = f32::MAX;
}

fn open_lobby(mut lobby: ResMut<Lobby>) {
    lobby.room = None;
    lobby.dirty = true;
    lobby.since_poll = f32::MAX;
}

/// Read the answers: keep what they say, open the match once a lobby's room has one.
fn receive(
    mut commands: Commands,
    mut lobby: ResMut<Lobby>,
    mut offset: ResMut<ClockOffset>,
    state: Res<State<AppState>>,
    mut next: ResMut<NextState<AppState>>,
) {
    let lobby = lobby.as_mut();
    if lobby.guest.is_none() && !lobby.asking(|c| matches!(c, Call::Guest)) {
        lobby.call(Call::Guest);
    }
    if lobby.nations.is_empty() && !lobby.asking(|c| matches!(c, Call::Rules)) {
        lobby.call(Call::Rules);
    }
    while let Some(reply) = lobby.http.receive() {
        if let Some(date) = reply.date {
            offset.0 = date - now_secs();
        }
        let Some(call) = lobby.pending.remove(&reply.id) else { continue };
        lobby.dirty = true;
        match call.read(&reply) {
            Ok(Answer::Guest(guest)) => {
                lobby.guest = Some(guest);
                lobby.error = None;
                if let Some(invite) = lobby.invite.take() {
                    lobby.call(Call::Join(invite));
                }
            }
            Ok(Answer::Rooms(rooms)) => lobby.rooms = rooms.rooms,
            Ok(Answer::Rules(rules)) => lobby.nations = nations(&rules),
            Ok(Answer::Room(room)) => {
                let started = room.match_id.clone();
                let id = room.room_id.clone();
                // Created or joined: its lobby opens.
                if matches!(call, Call::Create(_) | Call::Join(_)) {
                    lobby.open = Some(id.clone());
                    if *state.get() != AppState::Lobby {
                        next.set(AppState::Lobby);
                    }
                }
                if lobby.open.as_deref() == Some(id.as_str()) {
                    lobby.room = Some(room);
                    if let (Some(match_id), AppState::Lobby) = (started, state.get()) {
                        let server = lobby.http.server().clone();
                        enter(&mut commands, &mut next, Start::Room { server, match_id });
                    }
                }
            }
            Ok(Answer::Left) => {
                lobby.open = None;
                lobby.room = None;
                lobby.call(Call::Rooms);
                if *state.get() == AppState::Lobby {
                    next.set(AppState::Rooms);
                }
            }
            Err(e) => {
                // A guest the server forgot (a new database): make a new one.
                if matches!(&e, ApiError::Server { status: 401, .. }) && !matches!(call, Call::Guest) {
                    lobby.guest = None;
                }
                lobby.error = Some(e.to_string());
            }
        }
    }
}

/// The server's playable nations, from `/api/rules`.
fn nations(rules: &civ_sim::rooms::RulesResponse) -> Vec<(String, Color)> {
    rules
        .tables
        .nations
        .iter()
        .filter(|n| !n.city_state && n.barbarian != Some(true) && n.leader.is_some())
        .map(|n| {
            let c = |i: usize| n.outer_color.get(i).copied().unwrap_or(200) as f32 / 255.0;
            (n.name.clone(), Color::srgb(c(0), c(1), c(2)))
        })
        .collect()
}

fn now_secs() -> f64 {
    #[cfg(not(target_arch = "wasm32"))]
    return std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0.0, |d| d.as_secs_f64());
    #[cfg(target_arch = "wasm32")]
    return js_sys::Date::now() / 1000.0;
}

/// Read the open lobby every 2 s (the server's presence rule) and the room list every 5 s.
fn poll(time: Res<Time>, mut lobby: ResMut<Lobby>, state: Res<State<AppState>>) {
    lobby.since_poll += time.delta_secs();
    if lobby.guest.is_none() {
        return;
    }
    let busy = |lobby: &Lobby, f: fn(&Call) -> bool| lobby.pending.values().any(f);
    match state.get() {
        AppState::Lobby if lobby.since_poll >= 2.0 => {
            if let Some(id) = lobby.open.clone() {
                if !busy(&lobby, |c| matches!(c, Call::Room(_))) {
                    lobby.since_poll = 0.0;
                    lobby.call(Call::Room(id));
                }
            }
        }
        AppState::Rooms if lobby.since_poll >= 5.0 => {
            if !busy(&lobby, |c| matches!(c, Call::Rooms)) {
                lobby.since_poll = 0.0;
                lobby.call(Call::Rooms);
            }
        }
        _ => {}
    }
}

/// Rebuild the screen when what it shows changed; what is being typed in its fields survives.
#[allow(clippy::too_many_arguments)]
fn draw(
    mut commands: Commands,
    mut lobby: ResMut<Lobby>,
    state: Res<State<AppState>>,
    font: Res<UiFont>,
    content: Query<(Entity, Option<&Children>), With<Content>>,
    names: Query<&EditableText, With<NameField>>,
    invites: Query<&EditableText, With<InviteField>>,
    mut shown: Local<String>,
) {
    let Ok((e, children)) = content.single() else { return };
    if !lobby.dirty {
        return;
    }
    lobby.dirty = false;
    let typed = Typed {
        name: names.single().ok().map(|t| t.value().to_string()),
        invite: invites.single().ok().map(|t| t.value().to_string()).unwrap_or_default(),
    };
    let key = format!(
        "{:?}|{:?}|{:?}|{:?}|{:?}|{:?}|{}|{:?}",
        state.get(),
        lobby.guest.as_ref().map(|g| (&g.guest_id, &g.name)),
        lobby.rooms.iter().map(|r| (&r.room_id, &r.match_id, r.occupied_seats, r.turn, r.finished)).collect::<Vec<_>>(),
        lobby.room.as_ref().map(|r| serde_json::to_string(r).unwrap_or_default()),
        lobby.error,
        (lobby.settings.width, lobby.settings.majors, lobby.settings.city_states, lobby.settings.turn_seconds),
        lobby.nations.len(),
        lobby.open,
    );
    if *shown == key {
        return;
    }
    *shown = key;
    clear(&mut commands, children);
    let lobby = lobby.as_ref();
    commands.entity(e).with_children(|c| match state.get() {
        AppState::Lobby => draw_lobby(c, lobby, &font),
        _ => draw_rooms(c, lobby, &font, &typed),
    });
}

/// What the player had typed before a redraw.
struct Typed {
    name: Option<String>,
    invite: String,
}

fn draw_rooms(c: &mut ChildSpawnerCommands, lobby: &Lobby, font: &UiFont, typed: &Typed) {
    c.spawn(row()).with_children(|r| {
        r.spawn(button(font, Press::Home, "Back"));
        r.spawn(text(font, format!("Rooms on {}", lobby.http.server().base), 18.0, TEXT));
    });
    if let Some(error) = &lobby.error {
        c.spawn(text(font, error.clone(), 13.0, ERROR));
    }
    let Some(guest) = &lobby.guest else {
        c.spawn(text(font, "Connecting…", 14.0, DIM));
        return;
    };
    c.spawn(row()).with_children(|r| {
        r.spawn(text(font, "Your name", 14.0, DIM));
        let name = typed.name.as_deref().filter(|n| !n.is_empty()).or(guest.name.as_deref()).unwrap_or("");
        r.spawn((NameField, field(font, name, 220.0, 32)));
        r.spawn(button(font, Press::SaveName, "Save"));
    });

    c.spawn(text(font, "Your rooms", 16.0, TEXT));
    if lobby.rooms.is_empty() {
        c.spawn(text(font, "None yet: create one, or join with an invite.", 13.0, DIM));
    }
    for room in &lobby.rooms {
        let state = match (&room.match_id, room.finished, room.turn) {
            (_, true, _) => "finished".to_string(),
            (Some(_), false, Some(turn)) => format!("turn {turn}"),
            (Some(_), false, None) => "started".into(),
            (None, ..) => "in the lobby".into(),
        };
        c.spawn(row()).with_children(|r| {
            r.spawn(text(
                font,
                format!("{} · seat {} · {}/{} · {state}", short(&room.room_id), room.own_seat, room.occupied_seats, room.capacity),
                14.0,
                TEXT,
            ));
            match (&room.match_id, room.finished) {
                (Some(m), false) => {
                    r.spawn(button(font, Press::Play(m.clone()), "Play"));
                }
                (None, _) => {
                    r.spawn(button(font, Press::Open(room.room_id.clone()), "Open"));
                }
                _ => {}
            }
            r.spawn(button(font, Press::LeaveRoom(room.room_id.clone()), "Leave"));
        });
    }

    c.spawn(text(font, "Join a room", 16.0, TEXT));
    c.spawn(row()).with_children(|r| {
        r.spawn((InviteField, field(font, &typed.invite, 300.0, 32)));
        r.spawn(button(font, Press::Join, "Join"));
    });

    c.spawn(text(font, "Create a room", 16.0, TEXT));
    let s = &lobby.settings;
    let size = SIZES.iter().find(|z| (z.0, z.1) == (s.width, s.height)).map_or("Custom", |z| z.2);
    c.spawn(row()).with_children(|r| {
        r.spawn(button(font, Press::Size, format!("Map: {size} {}×{}", s.width, s.height)));
        r.spawn(button(font, Press::Majors, format!("Civilizations: {}", s.majors)));
        r.spawn(button(font, Press::CityStates, format!("City-states: {}", s.city_states)));
        r.spawn(button(font, Press::TurnTime, format!("Turn: {}", seconds(s.turn_seconds))));
    });
    c.spawn(text(font, "Seats nobody takes are played by bots.", 12.0, DIM));
    c.spawn(button(font, Press::Create, "Create"));
}

fn draw_lobby(c: &mut ChildSpawnerCommands, lobby: &Lobby, font: &UiFont) {
    c.spawn(row()).with_children(|r| {
        r.spawn(button(font, Press::Back, "Back"));
        r.spawn(text(font, format!("Room {}", lobby.open.as_deref().map(short).unwrap_or("")), 18.0, TEXT));
    });
    if let Some(error) = &lobby.error {
        c.spawn(text(font, error.clone(), 13.0, ERROR));
    }
    let Some(room) = &lobby.room else {
        c.spawn(text(font, "Loading…", 14.0, DIM));
        return;
    };
    c.spawn(text(
        font,
        format!(
            "{}×{} · {} civilizations · {} city-states · {} a turn",
            room.width,
            room.height,
            room.capacity,
            room.city_states,
            seconds(room.turn_seconds)
        ),
        13.0,
        DIM,
    ));
    if let Some(invite) = &room.invite {
        c.spawn(row()).with_children(|r| {
            r.spawn(text(font, "Invite", 14.0, DIM));
            r.spawn((field(font, invite, 320.0, 64), TextReadWriteMode::ReadOnly));
        });
        c.spawn(text(font, "Others join with this code; seats nobody takes are played by bots.", 12.0, DIM));
    }
    c.spawn(text(font, format!("Players {}/{}", room.occupied_seats, room.capacity), 16.0, TEXT));
    for m in &room.members {
        let choice = room.seats.iter().find(|s| s.seat == m.seat).and_then(|s| s.nation.clone()).unwrap_or_else(|| "any nation".into());
        let here = if m.connected == Some(true) { "here" } else { "away" };
        let owner = if m.is_owner { " · owner" } else { "" };
        let me = if m.seat == room.own_seat { " (you)" } else { "" };
        let name = m.name.clone().unwrap_or_else(|| format!("Seat {}", m.seat));
        c.spawn(text(font, format!("{name}{me} · {choice} · {here}{owner}"), 14.0, TEXT));
    }

    let mine = room.seats.iter().find(|s| s.seat == room.own_seat).and_then(|s| s.nation.clone());
    let taken: Vec<&str> = room.seats.iter().filter(|s| s.seat != room.own_seat).filter_map(|s| s.nation.as_deref()).collect();
    c.spawn(text(font, "Your nation", 16.0, TEXT));
    c.spawn(row()).with_children(|r| {
        let mut any = r.spawn(button_on(font, Press::Nation(None), "Any", mine.is_none()));
        if mine.is_none() {
            any.insert(Chosen);
        }
        for (name, colour) in lobby.nations.iter().filter(|(n, _)| !taken.contains(&n.as_str())) {
            let on = mine.as_deref() == Some(name);
            let mut b = r.spawn(button_on(font, Press::Nation(Some(name.clone())), name.clone(), on));
            b.insert(BorderColor::all(*colour));
            if on {
                b.insert(Chosen);
            }
        }
    });
    c.spawn(row()).with_children(|r| {
        if room.is_owner {
            r.spawn(button(font, Press::Start, "Start the match"));
        } else {
            r.spawn(text(font, "Waiting for the owner to start", 14.0, DIM));
        }
        r.spawn(button(font, Press::LeaveRoom(room.room_id.clone()), "Leave the room"));
    });
}

fn button_on(font: &UiFont, press: Press, label: impl Into<String>, on: bool) -> impl Bundle {
    let mut node = Node {
        min_height: Val::Px(28.0),
        padding: UiRect::axes(Val::Px(10.0), Val::Px(4.0)),
        align_items: AlignItems::Center,
        border: UiRect::left(Val::Px(4.0)),
        border_radius: BorderRadius::all(Val::Px(4.0)),
        ..default()
    };
    if on {
        node.border = UiRect::all(Val::Px(2.0));
    }
    (
        press,
        Button,
        Hovered::default(),
        observe(on_press),
        node,
        BackgroundColor(if on { BUTTON_ON } else { BUTTON }),
        BorderColor::all(Color::NONE),
        children![text(font, label, 13.0, TEXT)],
    )
}

fn short(id: &str) -> &str {
    &id[..id.len().min(6)]
}

fn seconds(s: u32) -> String {
    if s % 60 == 0 { format!("{} min", s / 60) } else { format!("{s} s") }
}

fn next_of<T: PartialEq + Copy>(options: &[T], current: T) -> T {
    let i = options.iter().position(|o| *o == current).map_or(0, |i| (i + 1) % options.len());
    options[i]
}

#[allow(clippy::too_many_arguments)]
fn on_press(
    event: On<Activate>,
    presses: Query<&Press>,
    mut commands: Commands,
    lobby: Option<ResMut<Lobby>>,
    mut next: ResMut<NextState<AppState>>,
    names: Query<&EditableText, With<NameField>>,
    invites: Query<&EditableText, With<InviteField>>,
    mut saves: ResMut<crate::saves::Saves>,
) {
    let Ok(press) = presses.get(event.entity) else { return };
    // Each new game gets its own map.
    let seed = (now_secs() * 1000.0) as u64 % 1_000_000_007;
    let solo = civ_sim::Settings { seed, ..civ_sim::Settings::default() };
    saves.notice = None;
    match press {
        Press::Load(id) => return saves.request_load(id),
        Press::Delete(id) => return saves.delete(&id.clone()),
        Press::Solo => return enter(&mut commands, &mut next, Start::Solo(solo)),
        Press::Watch => return enter(&mut commands, &mut next, Start::Watch(solo, civ_sim::session::Sight::All)),
        Press::Rooms => return next.set(AppState::Rooms),
        Press::Home => return next.set(AppState::Home),
        _ => {}
    }
    let Some(mut lobby) = lobby else { return };
    lobby.dirty = true;
    lobby.error = None;
    match press.clone() {
        Press::SaveName => {
            if let Ok(name) = names.single() {
                let name = name.value().to_string().trim().to_string();
                if !name.is_empty() {
                    lobby.call(Call::Rename(name));
                }
            }
        }
        Press::Join => {
            if let Ok(invite) = invites.single() {
                let invite = invite.value().to_string().trim().to_string();
                if !invite.is_empty() {
                    lobby.call(Call::Join(invite));
                }
            }
        }
        Press::Size => {
            let s = &mut lobby.settings;
            let (w, h, _) = SIZES[SIZES.iter().position(|z| (z.0, z.1) == (s.width, s.height)).map_or(0, |i| (i + 1) % SIZES.len())];
            (s.width, s.height) = (w, h);
        }
        Press::Majors => lobby.settings.majors = next_of(&[2, 3, 4, 5, 6, 7, 8], lobby.settings.majors),
        Press::CityStates => lobby.settings.city_states = next_of(&[0, 1, 2, 3, 4, 6, 8], lobby.settings.city_states),
        Press::TurnTime => lobby.settings.turn_seconds = next_of(&TURN_SECONDS, lobby.settings.turn_seconds),
        Press::Create => {
            let settings = lobby.settings.clone();
            lobby.call(Call::Create(settings));
        }
        Press::Open(id) => {
            lobby.open = Some(id);
            next.set(AppState::Lobby);
        }
        Press::Play(match_id) => {
            let server = lobby.http.server().clone();
            enter(&mut commands, &mut next, Start::Room { server, match_id });
        }
        Press::LeaveRoom(id) => lobby.call(Call::Leave(id)),
        Press::Nation(nation) => {
            if let Some(id) = lobby.open.clone() {
                lobby.call(Call::Seat(id, SeatRequest { nation, leader: None }));
            }
        }
        Press::Start => {
            if let Some(id) = lobby.open.clone() {
                lobby.call(Call::Start(id));
            }
        }
        Press::Back => next.set(AppState::Rooms),
        Press::Solo | Press::Watch | Press::Rooms | Press::Home | Press::Load(_) | Press::Delete(_) => {}
    }
}

fn shade(mut buttons: Query<(&Hovered, &mut BackgroundColor, Has<Chosen>), (With<Press>, Changed<Hovered>)>) {
    for (hovered, mut colour, chosen) in &mut buttons {
        colour.0 = if hovered.get() || chosen { BUTTON_ON } else { BUTTON };
    }
}

/// The room scene's host (`CIV_SCENE=room`): through these screens as a player would, shooting each, and
/// starting the match once someone has joined with the invite (written to `invite.txt` beside the shots).
#[cfg(not(target_arch = "wasm32"))]
fn drive(
    mut commands: Commands,
    shots: Option<Res<crate::shots::Shots>>,
    lobby: Option<ResMut<Lobby>>,
    state: Res<State<AppState>>,
    mut next: ResMut<NextState<AppState>>,
    mut step: Local<(u32, u32)>,
) {
    let Some(shots) = shots.filter(|s| s.scene.room) else { return };
    if state.is_changed() {
        step.1 = 0;
    }
    step.1 += 1;
    let frames = step.1;
    match (step.0, state.get()) {
        (0, AppState::Home) if frames == 30 => shots.shoot_menu(&mut commands, "home"),
        (0, AppState::Home) if frames == 45 => {
            next.set(AppState::Rooms);
            *step = (1, 0);
        }
        (1, AppState::Rooms) => {
            let Some(mut lobby) = lobby else { return };
            if lobby.guest.is_none() || lobby.nations.is_empty() {
                step.1 = 0;
                return;
            }
            match frames {
                1 => {
                    lobby.call(Call::Rename("Host".into()));
                    lobby.settings =
                        RoomRequest { width: 24, height: 16, majors: 2, city_states: 2, turn_seconds: 20, ..RoomRequest::default() };
                    lobby.dirty = true;
                }
                40 => shots.shoot_menu(&mut commands, "rooms"),
                55 => {
                    let settings = lobby.settings.clone();
                    lobby.call(Call::Create(settings));
                    *step = (2, 0);
                }
                _ => {}
            }
        }
        (2, AppState::Lobby) => {
            let Some(mut lobby) = lobby else { return };
            let Some(room) = lobby.room.clone() else {
                step.1 = 0;
                return;
            };
            if frames == 1 {
                let invite = room.invite.clone().unwrap_or_default();
                if let Some(dir) = shots.dir() {
                    let _ = std::fs::write(dir.join("invite.txt"), &invite);
                }
                println!("room invite: {invite}");
                if let Some((nation, _)) = lobby.nations.first().cloned() {
                    lobby.call(Call::Seat(room.room_id.clone(), SeatRequest { nation: Some(nation), leader: None }));
                }
            }
            if frames == 40 {
                shots.shoot_menu(&mut commands, "lobby");
            }
            if frames >= 40 && room.occupied_seats >= 2 {
                *step = (3, 0);
            }
        }
        (3, AppState::Lobby) => {
            let Some(mut lobby) = lobby else { return };
            // Two lobby reads, so the joiner shows as here.
            if frames == 150 {
                shots.shoot_menu(&mut commands, "lobby-full");
            }
            if frames == 170 {
                if let Some(id) = lobby.open.clone() {
                    lobby.call(Call::Start(id));
                }
                *step = (4, 0);
            }
        }
        _ => {}
    }
}
