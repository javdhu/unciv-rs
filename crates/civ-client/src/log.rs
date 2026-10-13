//! The notification log (docs/ACTIONS-SPEC.md, A1): every presented event the player should read, as a line of
//! text (`notes.rs`), kept for the match. The turn's decisions and notes that matter also stack above End Turn.
//! The log opens from the launch bar; clicking a line with a place looks at it.

use crate::camera::MapCamera;
use crate::client::RoundEvents;
use crate::inspect::UiFont;
use crate::notes::{describe, Note, NoteKind};
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

/// Notifications above End Turn: what the engine waits on, then this turn's notes that matter.
#[derive(Component)]
struct Stack;

#[derive(Component, Clone)]
struct OpenDecision(crate::decisions::Decision);

/// A card in the stack, which keeps a panel behind it over the map.
#[derive(Component)]
struct InStack;

/// A line that looks at its place when clicked.
#[derive(Component, Clone, Copy)]
struct LookAt(u16);

pub fn plugin(app: &mut App) {
    app.init_resource::<NoteLog>()
        .add_systems(OnEnter(crate::app::AppState::Match), (panel, stack_frame).run_if(crate::play::playing))
        .add_systems(OnExit(crate::app::AppState::Match), crate::app::reset::<NoteLog>)
        .add_systems(
            Update,
            (collect, draw, stack, shade).chain().run_if(crate::play::playing).in_set(crate::app::InMatch),
        );
}

fn collect(
    mut rounds: MessageReader<RoundEvents>,
    view: Res<ViewState>,
    rules: Option<Res<Rules>>,
    mut log: ResMut<NoteLog>,
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
            top: Val::Px(crate::playui::LEFT_PANEL_TOP),
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

fn stack_frame(mut commands: Commands) {
    commands.spawn((
        Stack,
        Node {
            position_type: PositionType::Absolute,
            right: Val::Px(12.0),
            bottom: Val::Px(12.0 + 56.0 + 30.0),
            width: Val::Px(300.0),
            flex_direction: FlexDirection::Column,
            align_items: AlignItems::FlexEnd,
            row_gap: Val::Px(4.0),
            ..default()
        },
        crate::screens::PANEL_Z,
    ));
}

/// The stack's cards: decisions first (a click opens one), then up to four of this turn's notes that are not
/// plain information (a click looks at the place).
#[allow(clippy::too_many_arguments)]
fn stack(
    mut commands: Commands,
    log: Res<NoteLog>,
    decisions: Res<crate::decisions::Decisions>,
    view: Res<ViewState>,
    rules: Option<Res<Rules>>,
    font: Res<UiFont>,
    slot: Query<(Entity, Option<&Children>), With<Stack>>,
) {
    let (Some(v), Ok((e, children))) = (view.view(), slot.single()) else { return };
    if !log.is_changed() && !decisions.is_changed() {
        return;
    }
    clear(&mut commands, children);
    let notes: Vec<&Note> = log
        .notes
        .iter()
        .rev()
        .take_while(|(t, _)| *t == v.turn)
        .map(|(_, n)| n)
        .filter(|n| !matches!(n.kind, NoteKind::Info))
        .take(4)
        .collect();
    commands.entity(e).with_children(|s| {
        for d in decisions.list.iter().take(5) {
            s.spawn((
                OpenDecision(d.clone()),
                LookAt(u16::MAX),
                Button,
                Hovered::default(),
                observe(open_decision),
                card(),
                InStack,
                BackgroundColor(crate::playui::PANEL),
                children![text(&font, d.label(v, rules.as_deref()), 13.0, TEXT)],
            ));
        }
        for note in notes.into_iter().rev() {
            let colour = match note.kind {
                NoteKind::Bad => URGENT,
                NoteKind::Good => GOOD,
                _ => TEXT,
            };
            let mut c = s.spawn((card(), InStack, BackgroundColor(crate::playui::PANEL), children![text(&font, note.text.clone(), 12.0, colour)]));
            if let Some(tile) = note.tile {
                c.insert((LookAt(tile), Button, Hovered::default(), observe(look)));
            }
        }
    });
}

fn card() -> (Node, Outline) {
    (
        Node { padding: UiRect::axes(Val::Px(8.0), Val::Px(4.0)), border_radius: BorderRadius::all(Val::Px(4.0)), ..default() },
        Outline::new(Val::Px(1.0), Val::ZERO, Color::srgba(1.0, 1.0, 1.0, 0.15)),
    )
}

fn open_decision(
    event: On<Activate>,
    cards: Query<&OpenDecision>,
    mut decisions: ResMut<crate::decisions::Decisions>,
    mut selection: ResMut<crate::play::Selection>,
    mut screens: ResMut<crate::screens::Screens>,
) {
    if let Ok(OpenDecision(d)) = cards.get(event.entity) {
        crate::decisions::open(d.clone(), &mut decisions, &mut selection, &mut screens);
    }
}

fn look(event: On<Activate>, lines: Query<&LookAt>, land: Option<Res<crate::map::Land>>, mut cameras: Query<&mut MapCamera>) {
    let (Ok(LookAt(tile)), Some(land), Ok(mut camera)) = (lines.get(event.entity), land, cameras.single_mut()) else { return };
    if *tile == u16::MAX {
        return;
    }
    let distance = camera.distance;
    camera.jump(land.0.grid.centre(*tile), distance);
}

fn shade(mut lines: Query<(&Hovered, &mut BackgroundColor, Has<InStack>), (With<LookAt>, Changed<Hovered>)>) {
    for (hovered, mut colour, stacked) in &mut lines {
        colour.0 = if hovered.get() { BUTTON_ON } else if stacked { crate::playui::PANEL } else { Color::NONE };
    }
}
