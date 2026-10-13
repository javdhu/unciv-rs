//! The play UI (docs/PLAY-SPEC.md, Layout): the top bar's research and treasury, the unit panel, the turn
//! button, the city panel, the research list, toasts and the combat preview card.
//!
//! Everything here is rebuilt from `ViewState` and the `Selection`'s answers when they change; a button sends
//! its order through the Outbox.

use crate::camera::MapCamera;
use crate::client::{Outbox, ViewChanged};
use crate::inspect::UiFont;
use crate::pieces::{Class, Key};
use crate::play::{awaiting, entity, playing, select_unit, unit_action, Selection, Toast};
use crate::rules::Rules;
use crate::view_state::ViewState;
use bevy::picking::hover::Hovered;
use bevy::prelude::*;
use bevy::ui_widgets::{observe, Activate, Button};
use bevy::window::PrimaryWindow;
use civ_sim::protocol::{Action, ConstructionRef, Currency};
use civ_sim::queries::{Availability, UnitActionKind};

const PANEL: Color = Color::srgba(0x14 as f32 / 255.0, 0x16 as f32 / 255.0, 0x1B as f32 / 255.0, 0.85);
const TEXT: Color = Color::srgb(0.93, 0.94, 0.95);
const DIM: Color = Color::srgb(0.6, 0.63, 0.67);
const BUTTON: Color = Color::srgba(1.0, 1.0, 1.0, 0.07);
const BUTTON_ON: Color = Color::srgba(1.0, 1.0, 1.0, 0.22);
const TOAST_SECONDS: f32 = 3.0;

/// What a play button does.
#[derive(Component, Clone, PartialEq, Debug)]
enum Order {
    Turn,
    Undo,
    Research,
    Pick(u32),
    Act(UnitActionKind),
    Build(u32),
    Produce(ConstructionRef),
    Buy(ConstructionRef),
    CloseCity,
}

#[derive(Component)]
struct TopLeft;
#[derive(Component)]
struct TopCentre;
#[derive(Component)]
struct TopRight;
#[derive(Component)]
struct UnitPanel;
#[derive(Component)]
struct TurnButton;
#[derive(Component)]
struct CityPanel;
#[derive(Component)]
struct ResearchList;
#[derive(Component)]
struct PreviewCard;
#[derive(Component)]
struct ToastBox(f32);

/// Asks a panel to rebuild.
#[derive(Resource, Default)]
struct Stale(bool);

pub fn plugin(app: &mut App) {
    app.init_resource::<Stale>()
        .add_systems(Startup, frame.run_if(playing))
        .add_systems(
            Update,
            (mark, top_bar, unit_panel, turn_button, city_panel, research_list, preview_card, toasts, keys, shade)
                .chain()
                .run_if(playing)
                .after(crate::map::MapSet::Rebuild),
        );
}

fn text(font: &UiFont, s: impl Into<String>, size: f32, colour: Color) -> impl Bundle {
    (
        Text::new(s),
        TextFont { font: font.regular.clone().into(), font_size: FontSize::Px(size), ..default() },
        TextColor(colour),
    )
}

fn button(font: &UiFont, order: Order, label: impl Into<String>, enabled: bool) -> impl Bundle {
    (
        order,
        Button,
        Hovered::default(),
        observe(press),
        Node {
            min_height: Val::Px(28.0),
            padding: UiRect::axes(Val::Px(10.0), Val::Px(4.0)),
            align_items: AlignItems::Center,
            justify_content: JustifyContent::Center,
            border_radius: BorderRadius::all(Val::Px(4.0)),
            ..default()
        },
        BackgroundColor(BUTTON),
        children![text(font, label, 13.0, if enabled { TEXT } else { DIM })],
    )
}

fn panel(node: Node) -> impl Bundle {
    (node, BackgroundColor(PANEL), Visibility::Hidden)
}

fn frame(mut commands: Commands) {
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
            (TopLeft, Node { width: Val::Px(260.0), ..default() }),
            (TopCentre, Node { column_gap: Val::Px(6.0), ..default() }),
            (
                TopRight,
                Node { width: Val::Px(360.0), justify_content: JustifyContent::FlexEnd, column_gap: Val::Px(10.0), align_items: AlignItems::Center, ..default() }
            ),
        ],
    ));
    commands.spawn((
        UnitPanel,
        panel(Node {
            position_type: PositionType::Absolute,
            bottom: Val::Px(12.0),
            left: Val::Percent(50.0),
            min_height: Val::Px(64.0),
            padding: UiRect::all(Val::Px(10.0)),
            column_gap: Val::Px(8.0),
            align_items: AlignItems::Center,
            ..default()
        }),
        UiTransform { translation: Val2::percent(-50.0, 0.0), ..default() },
    ));
    commands.spawn((
        TurnButton,
        Node {
            position_type: PositionType::Absolute,
            bottom: Val::Px(12.0),
            right: Val::Px(12.0),
            width: Val::Px(180.0),
            height: Val::Px(44.0),
            ..default()
        },
    ));
    commands.spawn((
        CityPanel,
        panel(Node {
            position_type: PositionType::Absolute,
            top: Val::Px(52.0),
            right: Val::Px(12.0),
            width: Val::Px(320.0),
            max_height: Val::Percent(75.0),
            padding: UiRect::all(Val::Px(12.0)),
            flex_direction: FlexDirection::Column,
            row_gap: Val::Px(6.0),
            overflow: Overflow::scroll_y(),
            ..default()
        }),
    ));
    commands.spawn((
        ResearchList,
        panel(Node {
            position_type: PositionType::Absolute,
            top: Val::Px(52.0),
            left: Val::Percent(50.0),
            width: Val::Px(420.0),
            max_height: Val::Percent(75.0),
            padding: UiRect::all(Val::Px(12.0)),
            flex_direction: FlexDirection::Column,
            row_gap: Val::Px(4.0),
            overflow: Overflow::scroll_y(),
            ..default()
        }),
        UiTransform { translation: Val2::percent(-50.0, 0.0), ..default() },
    ));
    commands.spawn((
        PreviewCard,
        panel(Node {
            position_type: PositionType::Absolute,
            width: Val::Px(260.0),
            padding: UiRect::all(Val::Px(10.0)),
            flex_direction: FlexDirection::Column,
            row_gap: Val::Px(4.0),
            ..default()
        }),
    ));
}

/// Panels rebuild when the view, the selection's answers or the selection itself change.
fn mark(mut changes: MessageReader<ViewChanged>, selection: Res<Selection>, mut stale: ResMut<Stale>) {
    if changes.read().count() > 0 || selection.is_changed() {
        stale.0 = true;
    }
}

fn clear(commands: &mut Commands, children: Option<&Children>) {
    if let Some(children) = children {
        for c in children.iter() {
            commands.entity(c).despawn();
        }
    }
}

fn year(y: i32) -> String {
    if y < 0 { format!("{} BC", -y) } else { format!("AD {y}") }
}

fn turns(cost: i32, done: i32, per_turn: i32) -> String {
    let left = (cost - done).max(0);
    if per_turn <= 0 { "–".into() } else { format!("{}", (left + per_turn - 1) / per_turn) }
}

#[allow(clippy::too_many_arguments)]
fn top_bar(
    mut commands: Commands,
    stale: Res<Stale>,
    view: Res<ViewState>,
    selection: Res<Selection>,
    font: Res<UiFont>,
    left: Query<(Entity, Option<&Children>), With<TopLeft>>,
    centre: Query<(Entity, Option<&Children>), (With<TopCentre>, Without<TopLeft>)>,
    right: Query<(Entity, Option<&Children>), (With<TopRight>, Without<TopLeft>, Without<TopCentre>)>,
) {
    let Some(v) = view.view() else { return };
    if !stale.0 {
        return;
    }
    let me = v.civs.iter().find(|c| c.id == v.me);
    if let Ok((e, children)) = left.single() {
        clear(&mut commands, children);
        commands.entity(e).with_child(text(&font, format!("Turn {} · {}", v.turn, year(v.year)), 14.0, TEXT));
    }
    if let Ok((e, children)) = centre.single() {
        clear(&mut commands, children);
        let label = match me.and_then(|c| c.researching.clone()) {
            Some(name) => {
                let left = selection
                    .civ_options
                    .as_ref()
                    .and_then(|o| o.research.iter().find(|r| r.selected))
                    .map(|r| turns(r.science_cost, r.invested, v.economy.science));
                match left {
                    Some(t) => format!("Researching {name} · {t} turns"),
                    None => format!("Researching {name}"),
                }
            }
            None => "Choose research".to_string(),
        };
        commands.entity(e).with_child(button(&font, Order::Research, label, true));
    }
    if let Ok((e, children)) = right.single() {
        clear(&mut commands, children);
        let treasury = me.and_then(|c| c.gold).map_or("–".into(), |g| g.to_string());
        let signed = |n: i32| if n >= 0 { format!("+{n}") } else { n.to_string() };
        commands.entity(e).with_children(|r| {
            r.spawn(text(&font, format!("Gold {treasury} ({})", signed(v.economy.gold)), 13.0, TEXT));
            r.spawn(text(&font, format!("Science {}", signed(v.economy.science)), 13.0, TEXT));
            r.spawn(button(&font, Order::Undo, "Undo", true));
        });
    }
}

#[allow(clippy::too_many_arguments)]
fn unit_panel(
    mut commands: Commands,
    stale: Res<Stale>,
    view: Res<ViewState>,
    rules: Option<Res<Rules>>,
    selection: Res<Selection>,
    font: Res<UiFont>,
    mut panel: Query<(Entity, &mut Visibility, Option<&Children>), With<UnitPanel>>,
) {
    let (Some(v), Some(rules), Ok((e, mut visibility, children))) = (view.view(), rules, panel.single_mut()) else { return };
    if !stale.0 {
        return;
    }
    clear(&mut commands, children);
    let unit = selection.unit.and_then(|k| v.units.iter().find(|u| (u.id, u.r#gen) == k));
    let Some(u) = unit else {
        *visibility = Visibility::Hidden;
        return;
    };
    *visibility = Visibility::Inherited;
    let row = rules.tables.units.iter().find(|r| r.id == u.base);
    let name = row.map_or("Unit".to_string(), |r| r.name.clone());
    let max_moves = row.map_or(0, |r| r.movement);
    let class = Class::of(&rules, u.base);
    let glyph = match class {
        Class::Melee => "M",
        Class::Ranged => "R",
        Class::Mounted => "H",
        Class::Siege => "S",
        Class::Naval => "N",
        Class::Air => "A",
        Class::Civilian => "C",
        Class::Great => "G",
    };
    let moves = if u.moves.fract() == 0.0 { format!("{}", u.moves as i64) } else { format!("{:.1}", u.moves) };
    commands.entity(e).with_children(|p| {
        p.spawn((
            Node { flex_direction: FlexDirection::Column, margin: UiRect::right(Val::Px(6.0)), ..default() },
            children![
                text(&font, format!("{glyph}  {name}"), 15.0, TEXT),
                text(&font, format!("Health {} · Moves {moves} / {max_moves}", u.health), 12.0, DIM),
            ],
        ));
        if let Some(options) = &selection.options {
            for choice in &options.actions {
                let label = match choice.action {
                    UnitActionKind::Fortify => "Fortify (F)",
                    UnitActionKind::Skip => "Wait (S)",
                    UnitActionKind::FoundCity => "Found city (B)",
                    UnitActionKind::Automate => "Automate",
                    UnitActionKind::Explore => "Explore",
                    UnitActionKind::Disband => "Disband",
                    UnitActionKind::Pillage => "Pillage",
                    UnitActionKind::SetUp => "Set up",
                    _ => continue,
                };
                let label = if choice.availability == Availability::NeedsConfirmation { format!("{label}…") } else { label.to_string() };
                p.spawn(button(&font, Order::Act(choice.action), label, true));
            }
            for improvement in &options.improvements {
                p.spawn(button(&font, Order::Build(*improvement), format!("Build {}", rules.improvement(*improvement)), true));
            }
        }
    });
}

fn turn_button(
    mut commands: Commands,
    stale: Res<Stale>,
    view: Res<ViewState>,
    selection: Res<Selection>,
    outbox: Res<Outbox>,
    font: Res<UiFont>,
    slot: Query<(Entity, Option<&Children>), With<TurnButton>>,
) {
    let Ok((e, children)) = slot.single() else { return };
    if !stale.0 && !outbox.is_changed() {
        return;
    }
    clear(&mut commands, children);
    let label = if awaiting(&view, &selection, None).is_some() { "Next unit" } else { "End turn" };
    commands.entity(e).with_child((
        button(&font, Order::Turn, label, true),
        Node { width: Val::Percent(100.0), height: Val::Percent(100.0), justify_content: JustifyContent::Center, align_items: AlignItems::Center, border_radius: BorderRadius::all(Val::Px(6.0)), ..default() },
    ));
}

#[allow(clippy::too_many_arguments)]
fn city_panel(
    mut commands: Commands,
    stale: Res<Stale>,
    view: Res<ViewState>,
    selection: Res<Selection>,
    font: Res<UiFont>,
    mut panel: Query<(Entity, &mut Visibility, Option<&Children>), With<CityPanel>>,
) {
    let (Some(v), Ok((e, mut visibility, children))) = (view.view(), panel.single_mut()) else { return };
    if !stale.0 {
        return;
    }
    clear(&mut commands, children);
    let city = selection.city.and_then(|k| v.cities.iter().find(|c| (c.id, c.r#gen) == k));
    let Some(c) = city else {
        *visibility = Visibility::Hidden;
        return;
    };
    *visibility = Visibility::Inherited;
    let own = c.own.as_ref();
    let (food, production) = own.map_or((0, 0), |o| (o.food_per_turn, o.production_per_turn));
    let building = c.queue.as_ref().and_then(|q| q.first().cloned()).unwrap_or_else(|| "Nothing".into());
    commands.entity(e).with_children(|p| {
        p.spawn((
            Node { justify_content: JustifyContent::SpaceBetween, ..default() },
            children![text(&font, format!("{}  ·  {}", c.name, c.population), 16.0, TEXT), button(&font, Order::CloseCity, "Close", true)],
        ));
        p.spawn(text(&font, format!("Food +{food} · Production +{production}"), 12.0, DIM));
        p.spawn(text(&font, format!("Building {building}"), 13.0, TEXT));
        p.spawn(text(&font, "Choose production", 12.0, DIM));
        if let Some(options) = &selection.city_options {
            for choice in &options.choices {
                let turns = choice.production_cost.map_or("–".into(), |cost| turns(cost, c.production_stored.unwrap_or(0), production));
                p.spawn(Node { column_gap: Val::Px(6.0), align_items: AlignItems::Center, ..default() })
                .with_children(|row| {
                    row.spawn((
                        button(&font, Order::Produce(choice.construction.clone()), format!("{} · {turns} turns", choice.name), true),
                        Node { flex_grow: 1.0, justify_content: JustifyContent::FlexStart, padding: UiRect::axes(Val::Px(10.0), Val::Px(4.0)), border_radius: BorderRadius::all(Val::Px(4.0)), ..default() },
                    ));
                    if let Some(price) = choice.gold.as_ref().and_then(|g| g.amount.clone().map(|a| (a, g.affordable.unwrap_or(false)))) {
                        row.spawn(button(&font, Order::Buy(choice.construction.clone()), format!("{} gold", price.0), price.1));
                    }
                });
            }
        }
    });
}

#[allow(clippy::too_many_arguments)]
fn research_list(
    mut commands: Commands,
    stale: Res<Stale>,
    view: Res<ViewState>,
    selection: Res<Selection>,
    font: Res<UiFont>,
    mut panel: Query<(Entity, &mut Visibility, Option<&Children>), With<ResearchList>>,
) {
    let (Some(v), Ok((e, mut visibility, children))) = (view.view(), panel.single_mut()) else { return };
    if !stale.0 {
        return;
    }
    clear(&mut commands, children);
    if !selection.research_open {
        *visibility = Visibility::Hidden;
        return;
    }
    *visibility = Visibility::Inherited;
    commands.entity(e).with_children(|p| {
        p.spawn(text(&font, "Research", 16.0, TEXT));
        if let Some(options) = &selection.civ_options {
            for r in &options.research {
                let label = format!(
                    "{}{} · {} turns ({}/{})",
                    if r.selected { "▸ " } else { "" },
                    r.name,
                    turns(r.science_cost, r.invested, v.economy.science),
                    r.invested,
                    r.science_cost
                );
                p.spawn(button(&font, Order::Pick(r.id), label, true));
            }
        }
    });
}

#[allow(clippy::too_many_arguments)]
fn preview_card(
    mut commands: Commands,
    selection: Res<Selection>,
    font: Res<UiFont>,
    windows: Query<&Window, With<PrimaryWindow>>,
    mut card: Query<(Entity, &mut Node, &mut Visibility, Option<&Children>), With<PreviewCard>>,
    mut shown: Local<Option<(u16, bool)>>,
) {
    let Ok((e, mut node, mut visibility, children)) = card.single_mut() else { return };
    let Some((tile, p)) = &selection.preview else {
        *visibility = Visibility::Hidden;
        *shown = None;
        return;
    };
    if let Some(c) = windows.single().ok().and_then(|w| w.cursor_position()) {
        node.left = Val::Px(c.x + 18.0);
        node.top = Val::Px(c.y + 18.0);
    }
    *visibility = Visibility::Inherited;
    let armed = selection.armed == Some(*tile);
    if *shown == Some((*tile, armed)) {
        return;
    }
    *shown = Some((*tile, armed));
    clear(&mut commands, children);
    let side = |name: &str, s: &civ_sim::queries::CombatSideView| {
        format!("{name}: strength {} · health {}/{} · takes {}–{}", s.strength, s.health, s.max_health, s.damage_min, s.damage_max)
    };
    commands.entity(e).with_children(|c| {
        c.spawn(text(&font, if p.ranged { "Ranged attack" } else { "Attack" }, 14.0, TEXT));
        c.spawn(text(&font, side("You", &p.attacker), 12.0, TEXT));
        c.spawn(text(&font, side("Them", &p.defender), 12.0, TEXT));
        if p.capture {
            c.spawn(text(&font, "Captures", 12.0, Color::srgb(1.0, 0.85, 0.5)));
        }
        c.spawn(text(&font, if armed { "Click again to attack" } else { "Click to aim" }, 12.0, DIM));
    });
}

fn toasts(
    mut commands: Commands,
    time: Res<Time>,
    font: Res<UiFont>,
    mut toasts: MessageReader<Toast>,
    mut boxes: Query<(Entity, &mut ToastBox)>,
) {
    for (e, mut t) in &mut boxes {
        t.0 -= time.delta_secs();
        if t.0 <= 0.0 {
            commands.entity(e).despawn();
        }
    }
    for Toast(message) in toasts.read() {
        commands.spawn((
            ToastBox(TOAST_SECONDS),
            Node {
                position_type: PositionType::Absolute,
                bottom: Val::Px(92.0),
                left: Val::Percent(50.0),
                padding: UiRect::axes(Val::Px(14.0), Val::Px(8.0)),
                border_radius: BorderRadius::all(Val::Px(6.0)),
                ..default()
            },
            UiTransform { translation: Val2::percent(-50.0, 0.0), ..default() },
            BackgroundColor(Color::srgba(0.5, 0.15, 0.12, 0.92)),
            children![text(&font, message.clone(), 13.0, TEXT)],
        ));
    }
}

/// Enter = the turn button; F fortify, S wait, B found a city; Esc clears; Ctrl+Z undo.
#[allow(clippy::too_many_arguments)]
fn keys(
    keys: Res<ButtonInput<KeyCode>>,
    mut selection: ResMut<Selection>,
    mut outbox: ResMut<Outbox>,
    view: Res<ViewState>,
    rules: Option<Res<Rules>>,
    mut cameras: Query<&mut MapCamera>,
    land: Option<Res<crate::map::Land>>,
    mut stale: ResMut<Stale>,
) {
    let ctrl = keys.pressed(KeyCode::ControlLeft) || keys.pressed(KeyCode::SuperLeft);
    if ctrl && keys.just_pressed(KeyCode::KeyZ) {
        outbox.submit(Action::Undo);
    }
    if keys.just_pressed(KeyCode::Escape) {
        selection.unit = None;
        selection.city = None;
        selection.research_open = false;
        selection.armed = None;
    }
    if keys.just_pressed(KeyCode::Enter) {
        turn(&mut selection, &mut outbox, &view, &mut cameras, land.as_deref());
    }
    let (Some(unit), Some(rules)) = (selection.unit, rules) else {
        stale.0 = false;
        return;
    };
    for (key, kind) in [(KeyCode::KeyF, UnitActionKind::Fortify), (KeyCode::KeyS, UnitActionKind::Skip), (KeyCode::KeyB, UnitActionKind::FoundCity)] {
        let offered = selection.options.as_ref().is_some_and(|o| o.actions.iter().any(|a| a.action == kind));
        if !ctrl && keys.just_pressed(key) && offered {
            act(&mut selection, &mut outbox, &view, &rules, unit, kind);
        }
    }
    stale.0 = false;
}

fn act(selection: &mut Selection, outbox: &mut Outbox, view: &ViewState, rules: &Rules, unit: Key, kind: UnitActionKind) {
    if let Some(action) = unit_action(kind, unit, view, rules) {
        if kind == UnitActionKind::Skip {
            selection.skipped.insert(unit);
        }
        outbox.submit(action);
    }
}

/// "Next unit" selects the next unit awaiting orders and glides to it; "End turn" ends it.
fn turn(selection: &mut Selection, outbox: &mut Outbox, view: &ViewState, cameras: &mut Query<&mut MapCamera>, land: Option<&crate::map::Land>) {
    match awaiting(view, selection, selection.unit) {
        Some(next) => {
            select_unit(selection, next);
            let tile = view.view().and_then(|v| v.units.iter().find(|u| (u.id, u.r#gen) == next)).map(|u| u.tile);
            if let (Some(tile), Some(land), Ok(mut camera)) = (tile, land, cameras.single_mut()) {
                let target = land.0.grid.centre(tile);
                let distance = camera.distance;
                camera.jump(target, distance);
            }
        }
        None => {
            outbox.submit(Action::FinishTurn);
            selection.unit = None;
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn press(
    event: On<Activate>,
    orders: Query<&Order>,
    mut selection: ResMut<Selection>,
    mut outbox: ResMut<Outbox>,
    view: Res<ViewState>,
    rules: Option<Res<Rules>>,
    mut cameras: Query<&mut MapCamera>,
    land: Option<Res<crate::map::Land>>,
    mut stale: ResMut<Stale>,
) {
    let Ok(order) = orders.get(event.entity) else { return };
    match order.clone() {
        Order::Turn => turn(&mut selection, &mut outbox, &view, &mut cameras, land.as_deref()),
        Order::Undo => {
            outbox.submit(Action::Undo);
        }
        Order::Research => {
            selection.research_open = !selection.research_open;
        }
        Order::Pick(technology) => {
            outbox.submit(Action::SetResearch { technology });
            selection.research_open = false;
        }
        Order::Act(kind) => {
            if let (Some(unit), Some(rules)) = (selection.unit, rules) {
                act(&mut selection, &mut outbox, &view, &rules, unit, kind);
            }
        }
        Order::Build(improvement) => {
            if let Some(unit) = selection.unit {
                outbox.submit(Action::Improve { unit: entity(unit), improvement });
            }
        }
        Order::Produce(what) => {
            if let Some(city) = selection.city {
                outbox.submit(Action::SetProduction { city: entity(city), queue: vec![what] });
            }
        }
        Order::Buy(what) => {
            if let Some(city) = selection.city {
                outbox.submit(Action::Purchase { city: entity(city), what, currency: Currency::Gold });
            }
        }
        Order::CloseCity => selection.city = None,
    }
    stale.0 = true;
}

/// Buttons light up under the pointer.
fn shade(mut buttons: Query<(&Hovered, &mut BackgroundColor), With<Order>>) {
    for (hovered, mut colour) in &mut buttons {
        colour.0 = if hovered.get() { BUTTON_ON } else { BUTTON };
    }
}
