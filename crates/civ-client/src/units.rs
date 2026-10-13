//! The units panel (docs/QUALITY-SPEC.md, K4): every own unit by kind and state, those that need orders first;
//! a click selects one and looks at it.

use crate::camera::MapCamera;
use crate::inspect::UiFont;
use crate::pieces::Key;
use crate::play::{select_unit, Selection};
use crate::playui::{clear, text, BUTTON_ON, DIM, TEXT};
use crate::rules::Rules;
use crate::view_state::ViewState;
use bevy::picking::hover::Hovered;
use bevy::prelude::*;
use bevy::ui_widgets::{observe, Activate, Button};

#[derive(Resource, Default)]
pub struct UnitsPanel {
    pub open: bool,
}

#[derive(Component)]
struct Panel;

#[derive(Component, Clone, Copy)]
struct Row(Key);

/// A unit's state, in the order the panel lists them.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum State {
    NeedsOrders,
    Waiting,
    Automated,
    Fortified,
    Done,
}

impl State {
    fn words(self) -> &'static str {
        match self {
            State::NeedsOrders => "needs orders",
            State::Waiting => "waiting this turn",
            State::Automated => "on its own",
            State::Fortified => "fortified",
            State::Done => "moved",
        }
    }
}

/// What a unit is doing, as the panel says it.
pub fn state(u: &civ_sim::UnitView, selection: &Selection) -> State {
    let automated = u.own.as_ref().is_some_and(|o| o.automation.is_some());
    if automated {
        State::Automated
    } else if u.fortified {
        State::Fortified
    } else if selection.skipped.contains(&(u.id, u.r#gen)) {
        State::Waiting
    } else if u.moves > 0.0 {
        State::NeedsOrders
    } else {
        State::Done
    }
}

pub fn plugin(app: &mut App) {
    app.init_resource::<UnitsPanel>()
        .add_systems(OnEnter(crate::app::AppState::Match), panel.run_if(crate::play::playing))
        .add_systems(OnExit(crate::app::AppState::Match), crate::app::reset::<UnitsPanel>)
        .add_systems(Update, (draw, shade).chain().run_if(crate::play::playing).in_set(crate::app::InMatch));
}

fn panel(mut commands: Commands) {
    commands.spawn((
        Panel,
        Node {
            position_type: PositionType::Absolute,
            top: Val::Px(crate::playui::LEFT_PANEL_TOP),
            left: Val::Px(12.0),
            width: Val::Px(340.0),
            max_height: Val::Percent(65.0),
            padding: UiRect::all(Val::Px(10.0)),
            flex_direction: FlexDirection::Column,
            row_gap: Val::Px(2.0),
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
    units: Res<UnitsPanel>,
    view: Res<ViewState>,
    rules: Option<Res<Rules>>,
    selection: Res<Selection>,
    font: Res<UiFont>,
    mut panel: Query<(Entity, &mut Visibility, Option<&Children>), With<Panel>>,
) {
    let Ok((e, mut visibility, children)) = panel.single_mut() else { return };
    if !units.is_changed() && !(units.open && (view.is_changed() || selection.is_changed())) {
        return;
    }
    clear(&mut commands, children);
    let (Some(v), Some(rules), true) = (view.view(), rules, units.open) else {
        *visibility = Visibility::Hidden;
        return;
    };
    *visibility = Visibility::Inherited;
    let mut rows: Vec<(State, String, Key)> = v
        .units
        .iter()
        .filter(|u| u.owner == v.me)
        .map(|u| {
            let name = rules.tables.units.iter().find(|r| r.id == u.base).map_or("Unit".to_string(), |r| r.name.clone());
            (state(u, &selection), name, (u.id, u.r#gen))
        })
        .collect();
    rows.sort();
    let waiting = rows.iter().filter(|r| r.0 == State::NeedsOrders).count();
    commands.entity(e).with_children(|p| {
        p.spawn(text(&font, format!("Units · {} · {waiting} need orders", rows.len()), 16.0, TEXT));
        for (state, name, key) in rows {
            let colour = if state == State::NeedsOrders { TEXT } else { DIM };
            let picked = selection.unit == Some(key);
            p.spawn((
                Row(key),
                Button,
                Hovered::default(),
                observe(look),
                Node { padding: UiRect::axes(Val::Px(6.0), Val::Px(3.0)), border_radius: BorderRadius::all(Val::Px(3.0)), ..default() },
                BackgroundColor(if picked { BUTTON_ON } else { Color::NONE }),
                children![text(&font, format!("{name} · {}", state.words()), 13.0, colour)],
            ));
        }
    });
}

fn look(
    event: On<Activate>,
    rows: Query<&Row>,
    view: Res<ViewState>,
    land: Option<Res<crate::map::Land>>,
    mut selection: ResMut<Selection>,
    mut cameras: Query<&mut MapCamera>,
) {
    let Ok(Row(key)) = rows.get(event.entity) else { return };
    select_unit(&mut selection, *key);
    let tile = view.view().and_then(|v| v.units.iter().find(|u| (u.id, u.r#gen) == *key)).map(|u| u.tile);
    if let (Some(tile), Some(land), Ok(mut camera)) = (tile, land, cameras.single_mut()) {
        let distance = camera.distance;
        camera.jump(land.0.grid.centre(tile), distance);
    }
}

fn shade(mut rows: Query<(&Hovered, &mut BackgroundColor), (With<Row>, Changed<Hovered>)>) {
    for (hovered, mut colour) in &mut rows {
        colour.0 = if hovered.get() { BUTTON_ON } else { Color::NONE };
    }
}
