//! The notification log (docs/ACTIONS-SPEC.md, A1): every presented event the player should read, as a line of
//! text (`notes.rs`), kept for the match. Decisions and losses also show as toasts when they arrive. The log
//! opens from the top bar; clicking a line with a place looks at it.

use crate::camera::MapCamera;
use crate::client::RoundEvents;
use crate::inspect::UiFont;
use crate::notes::{describe, Note, NoteKind};
use crate::play::Toast;
use crate::playui::{clear, text, BUTTON_ON, DIM, TEXT, URGENT};
use crate::rules::Rules;
use crate::view_state::ViewState;
use bevy::picking::hover::Hovered;
use bevy::prelude::*;
use bevy::ui_widgets::{observe, Activate, Button};
use std::collections::HashSet;

/// Lines kept, newest last; older ones fall off.
const KEEP: usize = 300;
const GOOD: Color = Color::srgb(0.6, 0.9, 0.6);

#[derive(Resource, Default)]
pub struct NoteLog {
    pub open: bool,
    notes: Vec<(u32, Note)>,
    seen: HashSet<String>,
}

#[derive(Component)]
struct LogPanel;

/// A line that looks at its place when clicked.
#[derive(Component, Clone, Copy)]
struct LookAt(u16);

pub fn plugin(app: &mut App) {
    app.init_resource::<NoteLog>()
        .add_systems(OnEnter(crate::app::AppState::Match), panel.run_if(crate::play::playing))
        .add_systems(OnExit(crate::app::AppState::Match), crate::app::reset::<NoteLog>)
        .add_systems(
            Update,
            (collect, draw, shade).chain().run_if(crate::play::playing).in_set(crate::app::InMatch),
        );
}

fn collect(
    mut rounds: MessageReader<RoundEvents>,
    view: Res<ViewState>,
    rules: Option<Res<Rules>>,
    mut log: ResMut<NoteLog>,
    mut toasts: MessageWriter<Toast>,
) {
    let (Some(v), Some(rules)) = (view.view(), rules) else {
        rounds.clear();
        return;
    };
    for RoundEvents(events) in rounds.read() {
        for e in events {
            if !log.seen.insert(e.id.clone()) {
                continue;
            }
            let Some(note) = describe(&e.event, v, &rules) else { continue };
            if matches!(note.kind, NoteKind::Decision | NoteKind::Bad) {
                toasts.write(Toast(note.text.clone()));
            }
            log.notes.push((v.turn, note));
        }
    }
    let over = log.notes.len().saturating_sub(KEEP);
    if over > 0 {
        log.notes.drain(..over);
    }
}

fn panel(mut commands: Commands) {
    commands.spawn((
        LogPanel,
        Node {
            position_type: PositionType::Absolute,
            top: Val::Px(52.0),
            left: Val::Px(12.0),
            width: Val::Px(340.0),
            max_height: Val::Percent(70.0),
            padding: UiRect::all(Val::Px(10.0)),
            flex_direction: FlexDirection::Column,
            row_gap: Val::Px(3.0),
            overflow: Overflow::scroll_y(),
            ..default()
        },
        BackgroundColor(crate::screens::SOLID_PANEL),
        crate::screens::PANEL_Z,
        Visibility::Hidden,
    ));
}

fn draw(
    mut commands: Commands,
    log: Res<NoteLog>,
    font: Res<UiFont>,
    mut panel: Query<(Entity, &mut Visibility, Option<&Children>), With<LogPanel>>,
) {
    let Ok((e, mut visibility, children)) = panel.single_mut() else { return };
    if !log.is_changed() {
        return;
    }
    clear(&mut commands, children);
    if !log.open {
        *visibility = Visibility::Hidden;
        return;
    }
    *visibility = Visibility::Inherited;
    commands.entity(e).with_children(|p| {
        p.spawn(text(&font, "Log", 16.0, TEXT));
        if log.notes.is_empty() {
            p.spawn(text(&font, "Nothing has happened yet.", 12.0, DIM));
        }
        let mut turn = None;
        // Newest first.
        for (t, note) in log.notes.iter().rev() {
            if turn != Some(*t) {
                turn = Some(*t);
                p.spawn(text(&font, format!("Turn {t}"), 12.0, DIM));
            }
            let colour = match note.kind {
                NoteKind::Decision => TEXT,
                NoteKind::Bad => URGENT,
                NoteKind::Good => GOOD,
                NoteKind::Info => DIM,
            };
            let mut line = p.spawn((
                Node { padding: UiRect::axes(Val::Px(6.0), Val::Px(2.0)), border_radius: BorderRadius::all(Val::Px(3.0)), ..default() },
                BackgroundColor(Color::NONE),
                children![text(&font, note.text.clone(), 12.0, colour)],
            ));
            if let Some(tile) = note.tile {
                line.insert((LookAt(tile), Button, Hovered::default(), observe(look)));
            }
        }
    });
}

fn look(event: On<Activate>, lines: Query<&LookAt>, land: Option<Res<crate::map::Land>>, mut cameras: Query<&mut MapCamera>) {
    let (Ok(LookAt(tile)), Some(land), Ok(mut camera)) = (lines.get(event.entity), land, cameras.single_mut()) else { return };
    let distance = camera.distance;
    camera.jump(land.0.grid.centre(*tile), distance);
}

fn shade(mut lines: Query<(&Hovered, &mut BackgroundColor), (With<LookAt>, Changed<Hovered>)>) {
    for (hovered, mut colour) in &mut lines {
        colour.0 = if hovered.get() { BUTTON_ON } else { Color::NONE };
    }
}
