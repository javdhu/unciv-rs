// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The spectator HUD (docs/VISUAL-SPEC.md, Picking and the spectator HUD): a 40 px top bar with the turn and
//! year, playback (pause, speed, one round, skip) and whose eyes and following; a 220 px panel of the civs.

use crate::client::LinkState;
use crate::inspect::UiFont;
use crate::link::{Link, Up};
use crate::present::Playback;
use crate::relief::Relief;
use crate::rules::Rules;
use crate::view_state::ViewState;
use bevy::picking::hover::Hovered;
use bevy::prelude::*;
use bevy::ui_widgets::{observe, Activate, Button};
use civ_sim::session::Sight;

const PANEL: Color = Color::srgba(0x14 as f32 / 255.0, 0x16 as f32 / 255.0, 0x1B as f32 / 255.0, 0.85);
const TEXT: Color = Color::srgb(0.93, 0.94, 0.95);
const DIM: Color = Color::srgb(0.6, 0.63, 0.67);
const BUTTON: Color = Color::srgba(1.0, 1.0, 1.0, 0.06);
const BUTTON_ON: Color = Color::srgba(1.0, 1.0, 1.0, 0.22);

/// Through whose eyes the watched match is seen.
#[derive(Resource, Clone, Copy, PartialEq, Eq)]
pub struct Eyes(pub Sight);

#[derive(Component, Clone, Copy, PartialEq)]
enum Control {
    Pause,
    Speed(f32),
    Step,
    Skip,
    Eyes,
    Follow,
    /// Back to the home screen.
    Home,
}

#[derive(Component)]
struct TurnText;

#[derive(Component)]
struct CivList;

pub fn plugin(app: &mut App) {
    let watching = |mode: Option<Res<crate::play::Mode>>| mode.is_some_and(|m| *m == crate::play::Mode::Watch);
    app.add_systems(OnEnter(crate::app::AppState::Match), bar.run_if(watching))
        .add_systems(Update, (show_controls, show_turn, show_civs).run_if(watching).in_set(crate::app::InMatch));
}

fn text(font: &UiFont, s: impl Into<String>, size: f32, colour: Color) -> impl Bundle {
    (
        Text::new(s),
        TextFont { font: font.regular.clone().into(), font_size: FontSize::Px(size), ..default() },
        TextColor(colour),
    )
}

fn button(font: &UiFont, control: Control, label: &str) -> impl Bundle {
    (
        control,
        Button,
        Hovered::default(),
        observe(activate),
        Node {
            height: Val::Px(26.0),
            padding: UiRect::horizontal(Val::Px(10.0)),
            align_items: AlignItems::Center,
            justify_content: JustifyContent::Center,
            border_radius: BorderRadius::all(Val::Px(4.0)),
            ..default()
        },
        BackgroundColor(BUTTON),
        children![text(font, label, 13.0, TEXT)],
    )
}

fn bar(mut commands: Commands, font: Res<UiFont>, link: Option<Res<Link>>) {
    let _ = link;
    commands.spawn((
        Node {
            position_type: PositionType::Absolute,
            top: Val::Px(0.0),
            left: Val::Px(0.0),
            right: Val::Px(0.0),
            height: Val::Px(40.0),
            padding: UiRect::horizontal(Val::Px(12.0)),
            align_items: AlignItems::Center,
            justify_content: JustifyContent::SpaceBetween,
            ..default()
        },
        BackgroundColor(PANEL),
        children![
            (TurnText, text(&font, "", 14.0, TEXT), Node { width: Val::Px(220.0), ..default() }),
            (
                Node { column_gap: Val::Px(6.0), align_items: AlignItems::Center, ..default() },
                children![
                    button(&font, Control::Pause, "Pause"),
                    button(&font, Control::Speed(1.0), "1x"),
                    button(&font, Control::Speed(2.0), "2x"),
                    button(&font, Control::Speed(4.0), "4x"),
                    button(&font, Control::Speed(8.0), "8x"),
                    button(&font, Control::Step, "One round"),
                    button(&font, Control::Skip, "Skip animations"),
                ],
            ),
            (
                Node {
                    width: Val::Px(220.0),
                    column_gap: Val::Px(6.0),
                    justify_content: JustifyContent::FlexEnd,
                    align_items: AlignItems::Center,
                    ..default()
                },
                children![
                    button(&font, Control::Eyes, "All-seeing"),
                    button(&font, Control::Follow, "Follow"),
                    button(&font, Control::Home, "Home"),
                ],
            ),
        ],
    ));
    commands.spawn((
        CivList,
        Node {
            position_type: PositionType::Absolute,
            top: Val::Px(52.0),
            right: Val::Px(12.0),
            width: Val::Px(220.0),
            padding: UiRect::all(Val::Px(10.0)),
            flex_direction: FlexDirection::Column,
            row_gap: Val::Px(6.0),
            ..default()
        },
        BackgroundColor(PANEL),
    ));
}

#[allow(clippy::too_many_arguments)]
fn activate(
    event: On<Activate>,
    controls: Query<&Control>,
    mut playback: ResMut<Playback>,
    eyes: Option<Res<Eyes>>,
    view: Res<ViewState>,
    rules: Option<Res<Rules>>,
    link: Res<Link>,
    mut commands: Commands,
    mut next: ResMut<NextState<crate::app::AppState>>,
) {
    let Ok(control) = controls.get(event.entity) else { return };
    {
        match control {
            Control::Home => crate::app::leave(&mut next, crate::app::AppState::Home),
            Control::Pause => playback.paused = !playback.paused,
            Control::Speed(s) => playback.speed = *s,
            Control::Step => playback.step = true,
            Control::Skip => playback.skip = !playback.skip,
            Control::Follow => playback.follow = !playback.follow,
            Control::Eyes => {
                // All-seeing, then each living major civ in turn, then all-seeing again.
                let now = eyes.as_deref().map_or(Sight::All, |e| e.0);
                let majors: Vec<u8> = view
                    .view()
                    .map(|v| {
                        v.civs
                            .iter()
                            .filter(|c| !c.city_state && c.alive)
                            .filter(|c| {
                                rules.as_ref().is_none_or(|r| {
                                    r.tables.nations.iter().find(|n| n.id == c.nation).and_then(|n| n.barbarian)
                                        != Some(true)
                                })
                            })
                            .map(|c| c.id)
                            .collect()
                    })
                    .unwrap_or_default();
                let current = match now {
                    Sight::All => None,
                    Sight::Seat => Some(0),
                    Sight::Civ(c) => Some(c),
                };
                let next = match current.and_then(|c| majors.iter().position(|m| *m == c)) {
                    None if current.is_none() => majors.first().map_or(Sight::All, |c| Sight::Civ(*c)),
                    Some(i) if i + 1 < majors.len() => Sight::Civ(majors[i + 1]),
                    _ => Sight::All,
                };
                link.send(Up::Watch(next));
                commands.insert_resource(Eyes(next));
            }
        }
    }
}

fn show_controls(
    playback: Res<Playback>,
    eyes: Option<Res<Eyes>>,
    view: Res<ViewState>,
    rules: Option<Res<Rules>>,
    mut buttons: Query<(&Control, &Hovered, &mut BackgroundColor, &Children)>,
    mut texts: Query<&mut Text>,
) {
    for (control, hovered, mut colour, children) in &mut buttons {
        let on = match control {
            Control::Pause => playback.paused,
            Control::Speed(s) => playback.speed == *s,
            Control::Skip => playback.skip,
            Control::Follow => playback.follow,
            Control::Step | Control::Eyes | Control::Home => false,
        };
        colour.0 = if on || hovered.get() { BUTTON_ON } else { BUTTON };
        let label = match control {
            Control::Pause => Some(if playback.paused { "Play".to_string() } else { "Pause".to_string() }),
            Control::Eyes => Some(match eyes.as_deref().map_or(Sight::All, |e| e.0) {
                Sight::All => "All-seeing".to_string(),
                Sight::Seat | Sight::Civ(_) => view
                    .view()
                    .and_then(|v| v.civs.iter().find(|c| c.id == v.me))
                    .and_then(|c| rules.as_ref().and_then(|r| r.nation(c.nation)).map(|n| format!("{}'s eyes", n.0)))
                    .unwrap_or_else(|| "Seat's eyes".into()),
            }),
            _ => None,
        };
        if let (Some(label), Some(child)) = (label, children.first()) {
            if let Ok(mut t) = texts.get_mut(*child) {
                if t.0 != label {
                    t.0 = label;
                }
            }
        }
    }
}

fn show_turn(view: Res<ViewState>, link: Res<LinkState>, mut text: Query<&mut Text, With<TurnText>>) {
    let (Some(v), Ok(mut t)) = (view.view(), text.single_mut()) else { return };
    let year = if v.year < 0 { format!("{} BC", -v.year) } else { format!("AD {}", v.year) };
    let mut s = format!("Turn {} · {year}", v.turn);
    if link.stopped.is_some() || v.result.is_some() {
        s.push_str(" · over");
    }
    if t.0 != s {
        t.0 = s;
    }
}

/// One row per living major civ (raiders left out): colour chip, name, cities, techs.
fn show_civs(
    mut commands: Commands,
    view: Res<ViewState>,
    rules: Option<Res<Rules>>,
    land: Option<Res<crate::map::Land>>,
    font: Res<UiFont>,
    list: Query<(Entity, Option<&Children>), With<CivList>>,
    mut last: Local<String>,
) {
    let (Some(v), Some(rules), Some(land), Ok((list, children))) = (view.view(), rules, land, list.single()) else {
        return;
    };
    let relief: &Relief = &land.0;
    let rows: Vec<(u8, String, usize, Option<usize>)> = v
        .civs
        .iter()
        .filter(|c| !c.city_state && c.alive)
        .filter(|c| rules.tables.nations.iter().find(|n| n.id == c.nation).and_then(|n| n.barbarian) != Some(true))
        .map(|c| {
            let name = rules.nation(c.nation).map_or("?".to_string(), |n| n.0.to_string());
            let cities = v.cities.iter().filter(|city| city.owner == c.id).count();
            (c.id, name, cities, c.techs)
        })
        .collect();
    let signature = format!("{rows:?}");
    if *last == signature {
        return;
    }
    *last = signature;
    if let Some(children) = children {
        for c in children.iter() {
            commands.entity(c).despawn();
        }
    }
    commands.entity(list).with_children(|l| {
        l.spawn((
            Node { column_gap: Val::Px(8.0), ..default() },
            children![
                (text(&font, "Civilisation", 12.0, DIM), Node { width: Val::Px(126.0), ..default() }),
                (text(&font, "Cities", 12.0, DIM), Node { width: Val::Px(36.0), ..default() }),
                text(&font, "Techs", 12.0, DIM),
            ],
        ));
        for (id, name, cities, techs) in rows {
            let (outer, inner) = relief.liveries.get(&id).copied().unwrap_or_default();
            l.spawn((Node { column_gap: Val::Px(8.0), align_items: AlignItems::Center, ..default() },)).with_children(|r| {
                r.spawn((
                    Node {
                        width: Val::Px(12.0),
                        height: Val::Px(12.0),
                        border: UiRect::all(Val::Px(2.0)),
                        ..default()
                    },
                    BackgroundColor(outer.into()),
                    BorderColor::all(Color::from(inner)),
                ));
                r.spawn((text(&font, name, 13.0, TEXT), Node { width: Val::Px(106.0), ..default() }));
                r.spawn((text(&font, cities.to_string(), 13.0, TEXT), Node { width: Val::Px(36.0), ..default() }));
                r.spawn(text(&font, techs.map_or("–".into(), |t| t.to_string()), 13.0, TEXT));
            });
        }
    });
}
