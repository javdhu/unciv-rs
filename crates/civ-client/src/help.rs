// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The key map (docs/QUALITY-SPEC.md, K2), drawn from `keys::KEYS` so it lists every key, and the keys that
//! open screens, cycle units and close panels.

use crate::camera::MapCamera;
use crate::inspect::UiFont;
use crate::keys::{just, Act, KEYS, MOUSE};
use crate::play::{awaiting, awaiting_before, select_unit, Selection};
use crate::playui::{clear, text, BUTTON, BUTTON_ON, DIM, TEXT};
use crate::view_state::ViewState;
use bevy::picking::hover::Hovered;
use bevy::prelude::*;
use bevy::ui_widgets::{observe, Activate, Button};

#[derive(Resource, Default)]
pub struct Help {
    pub open: bool,
}

#[derive(Component)]
struct HelpPanel;
#[derive(Component)]
struct CloseHelp;

pub fn plugin(app: &mut App) {
    app.init_resource::<Help>()
        .add_systems(OnEnter(crate::app::AppState::Match), panel)
        .add_systems(OnExit(crate::app::AppState::Match), crate::app::reset::<Help>)
        .add_systems(Update, (shortcuts, draw, shade).chain().in_set(crate::app::InMatch));
}

/// The panels on the left side open one at a time.
pub fn only(which: Act, screens: &mut crate::screens::Screens, empire: &mut crate::empire::Empire, log: &mut crate::log::NoteLog, units: &mut crate::units::UnitsPanel) {
    let was = match which {
        Act::Civilizations => screens.civs,
        Act::Empire => empire.open,
        Act::Log => log.open,
        Act::Units => units.open,
        _ => false,
    };
    screens.civs = false;
    empire.open = false;
    log.open = false;
    units.open = false;
    match which {
        Act::Civilizations => screens.civs = !was,
        Act::Empire => empire.open = !was,
        Act::Log => log.open = !was,
        Act::Units => units.open = !was,
        _ => {}
    }
}

#[allow(clippy::too_many_arguments)]
fn shortcuts(
    keys: Res<ButtonInput<KeyCode>>,
    view: Res<ViewState>,
    land: Option<Res<crate::map::Land>>,
    mut help: ResMut<Help>,
    mut selection: ResMut<Selection>,
    mut screens: ResMut<crate::screens::Screens>,
    mut empire: ResMut<crate::empire::Empire>,
    mut log: ResMut<crate::log::NoteLog>,
    mut units: ResMut<crate::units::UnitsPanel>,
    mut decisions: ResMut<crate::decisions::Decisions>,
    mut cameras: Query<&mut MapCamera>,
    mode: Option<Res<crate::play::Mode>>,
) {
    if just(&keys, Act::Help) {
        help.open = !help.open;
    }
    // Watching has only the key map; the rest is play.
    if mode.is_none_or(|m| *m != crate::play::Mode::Play) {
        if just(&keys, Act::Close) {
            help.open = false;
        }
        return;
    }
    if just(&keys, Act::Close) {
        // The topmost panel first, then what is aimed, then the selection.
        if help.open {
            help.open = false;
        } else if decisions.open.is_some() {
            decisions.open = None;
        } else if screens.trade.is_some() {
            screens.trade = None;
        } else if selection.research_open {
            selection.research_open = false;
        } else if screens.policies {
            screens.policies = false;
        } else if screens.civs || empire.open || log.open || units.open {
            only(Act::Close, &mut screens, &mut empire, &mut log, &mut units);
        } else if selection.siting || selection.mission.is_some() || selection.armed.is_some() || selection.end_armed {
            selection.siting = false;
            selection.mission = None;
            selection.armed = None;
            selection.end_armed = false;
        } else if selection.city.is_some() {
            selection.city = None;
        } else {
            selection.unit = None;
        }
    }
    if just(&keys, Act::Tree) {
        selection.research_open = !selection.research_open;
    }
    if just(&keys, Act::Policies) {
        screens.policies = !screens.policies;
    }
    for act in [Act::Civilizations, Act::Empire, Act::Log, Act::Units] {
        if just(&keys, act) {
            only(act, &mut screens, &mut empire, &mut log, &mut units);
        }
    }
    let next = if just(&keys, Act::NextUnit) {
        awaiting(&view, &selection, selection.unit)
    } else if just(&keys, Act::PreviousUnit) {
        awaiting_before(&view, &selection, selection.unit)
    } else {
        None
    };
    if let Some(k) = next {
        select_unit(&mut selection, k);
        let tile = view.view().and_then(|v| v.units.iter().find(|u| (u.id, u.r#gen) == k)).map(|u| u.tile);
        if let (Some(tile), Some(land), Ok(mut camera)) = (tile, land, cameras.single_mut()) {
            let distance = camera.distance;
            camera.jump(land.0.grid.centre(tile), distance);
        }
    }
}

fn panel(mut commands: Commands) {
    commands.spawn((
        HelpPanel,
        Node {
            position_type: PositionType::Absolute,
            top: Val::Percent(10.0),
            left: Val::Percent(50.0),
            width: Val::Px(760.0),
            max_width: Val::Percent(94.0),
            max_height: Val::Percent(80.0),
            padding: UiRect::all(Val::Px(16.0)),
            flex_direction: FlexDirection::Column,
            row_gap: Val::Px(8.0),
            overflow: Overflow::scroll_y(),
            ..default()
        },
        UiTransform { translation: Val2::percent(-50.0, 0.0), ..default() },
        BackgroundColor(crate::screens::SOLID_PANEL),
        GlobalZIndex(40),
        Visibility::Hidden,
    ));
}

fn draw(
    mut commands: Commands,
    help: Res<Help>,
    font: Res<UiFont>,
    mode: Option<Res<crate::play::Mode>>,
    mut panel: Query<(Entity, &mut Visibility, Option<&Children>), With<HelpPanel>>,
) {
    let Ok((e, mut visibility, children)) = panel.single_mut() else { return };
    if !help.is_changed() {
        return;
    }
    clear(&mut commands, children);
    if !help.open {
        *visibility = Visibility::Hidden;
        return;
    }
    *visibility = Visibility::Inherited;
    let watching = mode.is_some_and(|m| *m == crate::play::Mode::Watch);
    let contexts: Vec<&str> = if watching { vec!["Map", "Watching", "Screens"] } else { vec!["Map", "Turn", "Units", "Screens"] };
    commands.entity(e).with_children(|p| {
        p.spawn((Node { justify_content: JustifyContent::SpaceBetween, ..default() }, children![
            text(&font, "Keys and mouse", 18.0, TEXT),
            (CloseHelp, Button, Hovered::default(), observe(close), Node {
                padding: UiRect::axes(Val::Px(10.0), Val::Px(4.0)),
                border_radius: BorderRadius::all(Val::Px(4.0)),
                ..default()
            }, BackgroundColor(BUTTON), children![text(&font, "Close (Esc)", 13.0, TEXT)]),
        ]));
        p.spawn(Node { flex_wrap: FlexWrap::Wrap, column_gap: Val::Px(28.0), row_gap: Val::Px(12.0), ..default() }).with_children(|cols| {
            for context in contexts {
                cols.spawn(Node { flex_direction: FlexDirection::Column, row_gap: Val::Px(3.0), width: Val::Px(330.0), ..default() }).with_children(|col| {
                    col.spawn(text(&font, context, 14.0, DIM));
                    for k in KEYS.iter().filter(|k| k.context == context) {
                        col.spawn(row(&font, k.keys, k.what));
                    }
                });
            }
            cols.spawn(Node { flex_direction: FlexDirection::Column, row_gap: Val::Px(3.0), width: Val::Px(330.0), ..default() }).with_children(|col| {
                col.spawn(text(&font, "Mouse", 14.0, DIM));
                for (what, does) in MOUSE {
                    col.spawn(row(&font, what, does));
                }
            });
        });
    });
}

fn row(font: &UiFont, keys: &str, what: &str) -> impl Bundle {
    (
        Node { column_gap: Val::Px(10.0), ..default() },
        children![
            (text(font, keys, 13.0, TEXT), Node { width: Val::Px(96.0), flex_shrink: 0.0, ..default() }),
            text(font, what, 13.0, DIM),
        ],
    )
}

fn close(_: On<Activate>, mut help: ResMut<Help>) {
    help.open = false;
}

fn shade(mut buttons: Query<(&Hovered, &mut BackgroundColor), (With<CloseHelp>, Changed<Hovered>)>) {
    for (hovered, mut colour) in &mut buttons {
        colour.0 = if hovered.get() { BUTTON_ON } else { BUTTON };
    }
}
