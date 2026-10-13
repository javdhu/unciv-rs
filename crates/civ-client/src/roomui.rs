//! What a match in a room adds to the play UI (docs/ROOMS-SPEC.md, component 6): the deadline, every human
//! seat's state, the connection, a notice when a bot is about to play your seat or give it back, and the way
//! back to the rooms. Shown only while the link is remote; the turn button's waiting state is in `playui.rs`.

use crate::app::{AppState, InMatch};
use crate::client::LinkState;
use crate::inspect::UiFont;
use crate::link::{Link, Status};
use crate::net::Retry;
use crate::playui::{clear, text, BUTTON, BUTTON_ON, DIM, PANEL, TEXT};
use crate::rules::Rules;
use crate::view_state::ViewState;
use bevy::picking::hover::Hovered;
use bevy::prelude::*;
use bevy::ui_widgets::{observe, Activate, Button};

/// The server's clock minus this one's, in seconds, from the `Date` of the last HTTP reply (to a second).
#[derive(Resource, Default, Clone, Copy)]
pub struct ClockOffset(pub f64);

#[derive(Component)]
struct RoomPanel;
#[derive(Component)]
struct Deadline;
#[derive(Component)]
struct Lines;

#[derive(Component, Clone, Copy, PartialEq, Debug)]
enum RoomOrder {
    Leave,
}

const URGENT: Color = Color::srgb(1.0, 0.45, 0.4);

pub fn remote(link: Option<Res<Link>>) -> bool {
    link.is_some_and(|l| l.is_remote())
}

pub fn plugin(app: &mut App) {
    app.init_resource::<ClockOffset>()
        .add_systems(OnEnter(AppState::Match), panel.run_if(remote).after(crate::camera::spawn))
        .add_systems(Update, (deadline, lines, shade).run_if(remote).in_set(InMatch));
}

fn panel(mut commands: Commands, font: Res<UiFont>) {
    commands.spawn((
        RoomPanel,
        Node {
            position_type: PositionType::Absolute,
            top: Val::Px(52.0),
            left: Val::Px(12.0),
            width: Val::Px(240.0),
            padding: UiRect::all(Val::Px(10.0)),
            flex_direction: FlexDirection::Column,
            row_gap: Val::Px(6.0),
            ..default()
        },
        BackgroundColor(PANEL),
        // Under the side panels and screens, which open over it.
        GlobalZIndex(9),
        children![
            (Deadline, text(&font, "", 15.0, TEXT)),
            (Lines, Node { flex_direction: FlexDirection::Column, row_gap: Val::Px(3.0), ..default() }),
            (
                RoomOrder::Leave,
                Button,
                Hovered::default(),
                observe(press),
                Node {
                    padding: UiRect::axes(Val::Px(10.0), Val::Px(4.0)),
                    justify_content: JustifyContent::Center,
                    border_radius: BorderRadius::all(Val::Px(4.0)),
                    ..default()
                },
                BackgroundColor(BUTTON),
                children![text(&font, "Back to rooms", 13.0, TEXT)],
            ),
        ],
    ));
}

/// Seconds left to the deadline, by the server's clock.
pub fn seconds_left(deadline_ms: &str, offset: f64, now: f64) -> Option<f64> {
    let deadline = deadline_ms.parse::<f64>().ok()? / 1000.0;
    Some((deadline - (now + offset)).max(0.0))
}

fn now_secs() -> f64 {
    #[cfg(not(target_arch = "wasm32"))]
    return std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0.0, |d| d.as_secs_f64());
    #[cfg(target_arch = "wasm32")]
    return js_sys::Date::now() / 1000.0;
}

fn deadline(
    view: Res<ViewState>,
    offset: Res<ClockOffset>,
    mut shown: Local<Option<(u64, bool)>>,
    mut line: Query<(&mut Text, &mut TextColor), With<Deadline>>,
) {
    let Ok((mut text, mut colour)) = line.single_mut() else { return };
    let Some(update) = view.update() else { return };
    let left = update.deadline_ms.as_deref().and_then(|d| seconds_left(d, offset.0, now_secs()));
    let key = left.map_or((u64::MAX, update.ready), |s| (s.ceil() as u64, update.ready));
    if *shown == Some(key) {
        return;
    }
    *shown = Some(key);
    text.0 = match left {
        Some(s) => {
            let s = s.ceil() as u64;
            format!("Turn ends in {}:{:02}", s / 60, s % 60)
        }
        None => "No time limit".into(),
    };
    colour.0 = if left.is_some_and(|s| s < 15.0) && !update.ready { URGENT } else { TEXT };
}

/// The connection, the control notice and the seats, rebuilt when any of them changes.
fn lines(
    mut commands: Commands,
    view: Res<ViewState>,
    link: Res<LinkState>,
    rules: Option<Res<Rules>>,
    font: Res<UiFont>,
    mut shown: Local<Option<String>>,
    slot: Query<(Entity, Option<&Children>), With<Lines>>,
) {
    let Ok((e, children)) = slot.single() else { return };
    let Some(update) = view.update() else { return };
    // A row's text, and the colour of the mark before it (its civ), if any.
    let mut rows: Vec<(String, Color, Option<Color>)> = Vec::new();
    match (&link.stopped, link.connection) {
        (Some(reason), _) => rows.push((format!("Disconnected: {}", stopped(reason)), URGENT, None)),
        (None, Status::Retrying(n)) => rows.push((format!("Reconnecting ({n} of {})", Retry::TRIES), URGENT, None)),
        (None, Status::Live) => {}
    }
    match (update.controlled, update.pending_control) {
        (true, Some(false)) => rows.push(("A bot plays your seat from next turn".into(), DIM, None)),
        (false, Some(true)) => rows.push(("You take your seat back next turn".into(), DIM, None)),
        (false, _) => rows.push(("A bot is playing your seat".into(), DIM, None)),
        _ => {}
    }
    for seat in update.seats.iter().flatten() {
        let civ = update.view.civs.iter().find(|c| c.id == seat.seat);
        let nation = civ.and_then(|c| rules.as_ref().and_then(|r| r.nation(c.nation)));
        let name = seat.name.clone().or_else(|| nation.map(|n| n.0.to_string())).unwrap_or_else(|| format!("Seat {}", seat.seat));
        let state = if seat.left.unwrap_or(false) {
            "left"
        } else if !seat.controlled {
            "bot"
        } else if seat.connected == Some(false) {
            "away"
        } else if seat.ready {
            "finished"
        } else {
            "playing"
        };
        let me = if seat.seat == update.view.me { " (you)" } else { "" };
        let colour = if state == "playing" || state == "finished" { TEXT } else { DIM };
        rows.push((format!("{name}{me} · {state}"), colour, Some(nation.map_or(DIM, |n| n.1))));
    }
    let key = rows.iter().map(|(s, ..)| s.as_str()).collect::<Vec<_>>().join("\n");
    if shown.as_deref() == Some(key.as_str()) {
        return;
    }
    *shown = Some(key);
    clear(&mut commands, children);
    commands.entity(e).with_children(|c| {
        for (row, colour, mark) in rows {
            c.spawn(Node { column_gap: Val::Px(6.0), align_items: AlignItems::Center, ..default() }).with_children(|r| {
                if let Some(mark) = mark {
                    r.spawn((
                        Node { width: Val::Px(10.0), height: Val::Px(10.0), border_radius: BorderRadius::all(Val::Px(2.0)), ..default() },
                        BackgroundColor(mark),
                    ));
                }
                r.spawn(text(&font, row, 13.0, colour));
            });
        }
    });
}

/// A stop reason in words.
fn stopped(reason: &str) -> &str {
    match reason {
        "connectionReplaced" => "this seat was opened somewhere else",
        "protocolVersion" => "this client is out of date",
        "left" => "you left",
        other => other,
    }
}

fn press(event: On<Activate>, orders: Query<&RoomOrder>, mut next: ResMut<NextState<AppState>>) {
    if let Ok(RoomOrder::Leave) = orders.get(event.entity) {
        crate::app::leave(&mut next, AppState::Rooms);
    }
}

fn shade(mut buttons: Query<(&Hovered, &mut BackgroundColor), With<RoomOrder>>) {
    for (hovered, mut colour) in &mut buttons {
        colour.0 = if hovered.get() { BUTTON_ON } else { BUTTON };
    }
}
