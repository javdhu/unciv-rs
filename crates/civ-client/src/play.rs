//! A turn you play (docs/PLAY-SPEC.md): selection, the queries that answer what a unit can do and where it can
//! go, orders by left-click (a second click confirms an attack), and the reach and target highlights.
//!
//! The server says what is possible; the client never works out legality itself. Every answer is asked again
//! when the view changes, and an answer to an older question is dropped.

use crate::camera::{MapCamera, CLICK_SLOP};
use crate::client::{Outbox, QueryAnswered, ReceiptArrived, ViewChanged};
use crate::hex;
use crate::inspect::Hover;
use crate::link::{Link, Up};
use crate::map::{Builder, Land};
use crate::pieces::Key;
use crate::rules::Rules;
use crate::view_state::ViewState;
use bevy::color::LinearRgba;
use bevy::light::NotShadowCaster;
use bevy::picking::hover::HoverMap;
use bevy::prelude::*;
use civ_sim::protocol::{Action, AttackTarget, ClientMessage, CommandStatus, EntityRef, Query as Ask, QueryResult};
use civ_sim::queries::{CityOptions, CivOptions, CombatPreviewView, UnitActionKind, UnitOptions};
use std::collections::HashSet;

/// Watching bots, or playing seat 0.
#[derive(Resource, Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    Watch,
    Play,
}

pub fn playing(mode: Option<Res<Mode>>) -> bool {
    mode.is_some_and(|m| *m == Mode::Play)
}

#[derive(Resource, Default)]
pub struct Selection {
    pub unit: Option<Key>,
    pub city: Option<Key>,
    pub reach: Vec<civ_sim::Reach>,
    pub options: Option<UnitOptions>,
    /// Tiles the selected unit may attack.
    pub targets: Vec<u16>,
    /// An enemy clicked once: the preview is up, a second click attacks.
    pub armed: Option<u16>,
    pub preview: Option<(u16, CombatPreviewView)>,
    pub city_options: Option<CityOptions>,
    pub civ_options: Option<CivOptions>,
    /// Units told to wait this turn: they no longer count as awaiting orders.
    pub skipped: HashSet<Key>,
    pub research_open: bool,
    /// The turn the skips belong to.
    turn: u32,
    /// Bumped with every new question, so older answers are dropped.
    asked: u64,
    previewing: Option<u16>,
}

/// A short message at the bottom centre (docs/PLAY-SPEC.md, Orders).
#[derive(Message, Clone, Debug)]
pub struct Toast(pub String);

#[derive(Component)]
struct Highlight;

#[derive(Resource)]
struct HighlightMaterial(Handle<StandardMaterial>);

pub fn plugin(app: &mut App) {
    app.init_resource::<Selection>()
        .add_message::<Toast>()
        .add_systems(Startup, |mut c: Commands, mut m: ResMut<Assets<StandardMaterial>>| {
            c.insert_resource(HighlightMaterial(m.add(StandardMaterial {
                base_color: Color::WHITE,
                unlit: true,
                alpha_mode: AlphaMode::Blend,
                ..default()
            })));
        })
        .add_systems(
            Update,
            (refresh, answers, receipts, click, preview, highlight)
                .chain()
                .run_if(playing)
                .after(crate::map::MapSet::Rebuild),
        );
}

pub fn entity(k: Key) -> EntityRef {
    EntityRef { id: k.0, r#gen: k.1 }
}

fn ask(link: &Link, selection: &mut Selection, what: &str, query: Ask) {
    link.send(Up::Client(ClientMessage::Query { request_id: format!("{what}:{}", selection.asked), query }));
}

/// Ask again about the selected unit, city and the civ whenever the view changes.
fn refresh(
    link: Res<Link>,
    view: Res<ViewState>,
    mut changes: MessageReader<ViewChanged>,
    mut selection: ResMut<Selection>,
) {
    let Some(v) = view.view() else { return };
    if changes.read().count() == 0 && selection.asked > 0 {
        return;
    }
    if v.turn != selection.turn {
        selection.turn = v.turn;
        selection.skipped.clear();
    }
    // A unit or city that left the view leaves the selection.
    if let Some(k) = selection.unit {
        if !v.units.iter().any(|u| (u.id, u.r#gen) == k) {
            selection.unit = None;
        }
    }
    if let Some(k) = selection.city {
        if !v.cities.iter().any(|c| (c.id, c.r#gen) == k) {
            selection.city = None;
        }
    }
    selection.asked += 1;
    selection.armed = None;
    selection.preview = None;
    selection.previewing = None;
    if let Some(k) = selection.unit {
        ask(&link, &mut selection, "reach", Ask::Reachable { unit: entity(k) });
        ask(&link, &mut selection, "unit", Ask::UnitOptions { unit: entity(k), nuclear_target: None });
    } else {
        selection.reach.clear();
        selection.options = None;
        selection.targets.clear();
    }
    if let Some(k) = selection.city {
        ask(&link, &mut selection, "city", Ask::CityOptions { city: entity(k) });
    }
    ask(&link, &mut selection, "civ", Ask::CivOptions);
}

fn answers(mut answers: MessageReader<QueryAnswered>, mut selection: ResMut<Selection>) {
    for answer in answers.read() {
        let Some((what, n)) = answer.request_id.split_once(':') else { continue };
        if n.parse::<u64>().ok() != Some(selection.asked) {
            continue;
        }
        match (what, &answer.reply.result) {
            ("reach", QueryResult::Reachable { reach }) => selection.reach = reach.clone(),
            ("unit", QueryResult::UnitOptions { options }) => {
                selection.targets = options
                    .as_ref()
                    .and_then(|o| o.targets.as_ref())
                    .map(|t| t.iter().filter(|t| t.action == UnitActionKind::Attack).flat_map(|t| t.tiles.clone()).collect())
                    .unwrap_or_default();
                selection.options = options.clone();
            }
            ("city", QueryResult::CityOptions { options }) => selection.city_options = options.clone(),
            ("civ", QueryResult::CivOptions { options }) => selection.civ_options = Some(options.clone()),
            ("preview", QueryResult::CombatPreview { preview }) => {
                if let (Some(tile), Some(p)) = (selection.previewing, preview.clone()) {
                    selection.preview = Some((tile, p));
                }
            }
            _ => {}
        }
    }
}

/// Rejections and undo refusals become toasts, in plain words.
fn receipts(mut receipts: MessageReader<ReceiptArrived>, mut toasts: MessageWriter<Toast>) {
    for r in receipts.read() {
        if let CommandStatus::Rejected { code } = &r.receipt.result {
            let why = match r.receipt.undo_refusal {
                Some(refusal) => format!("Can't undo: {}", undo_reason(refusal)),
                None => format!("Not done: {}", rejection(*code)),
            };
            toasts.write(Toast(why));
        }
    }
}

fn rejection(code: civ_sim::protocol::RejectionCode) -> &'static str {
    use civ_sim::protocol::RejectionCode as R;
    match code {
        R::InvalidRequest => "the order was malformed",
        R::CommandIdReused => "that order was already given",
        R::WrongEpoch => "the match changed",
        R::StaleTurn => "the turn has moved on",
        R::TurnFinished => "your turn is over",
        R::MatchFinished => "the match is over",
        R::NotControlled => "a bot plays this seat now",
        R::Unavailable => "it isn't possible now",
        R::IllegalAction => "the rules don't allow it",
    }
}

fn undo_reason(refusal: civ_sim::protocol::UndoRefusal) -> &'static str {
    use civ_sim::protocol::UndoRefusal as U;
    match refusal {
        U::NothingToUndo => "there is nothing to take back",
        U::NotAllowed => "that kind of order can't be taken back",
        U::Revealed => "it showed you new tiles",
        U::Combat => "a battle happened",
        U::Chance => "chance decided something",
        U::TurnEnded => "the turn has ended",
        U::OthersActed => "something else has changed since",
    }
}

/// Whether the mouse is over a UI node, where a click belongs to the UI.
fn over_ui(hover: &HoverMap, nodes: &Query<(), With<Node>>) -> bool {
    hover.values().flat_map(|m| m.keys()).any(|e| nodes.contains(*e))
}

#[allow(clippy::too_many_arguments)]
fn click(
    buttons: Res<ButtonInput<MouseButton>>,
    windows: Query<&Window, With<bevy::window::PrimaryWindow>>,
    cameras: Query<&MapCamera>,
    hover: Res<Hover>,
    hover_map: Res<HoverMap>,
    nodes: Query<(), With<Node>>,
    view: Res<ViewState>,
    link: Res<Link>,
    mut outbox: ResMut<Outbox>,
    mut selection: ResMut<Selection>,
    mut pressed_at: Local<Option<Vec2>>,
) {
    let cursor = windows.single().ok().and_then(|w| w.cursor_position());
    if buttons.just_pressed(MouseButton::Left) {
        *pressed_at = cursor;
    }
    if !buttons.just_released(MouseButton::Left) {
        return;
    }
    let clicked = matches!((*pressed_at, cursor), (Some(a), Some(b)) if a.distance(b) <= CLICK_SLOP);
    if !clicked || cameras.single().is_ok_and(|c| c.dragging()) || over_ui(&hover_map, &nodes) {
        return;
    }
    let (Some(v), Some(tile)) = (view.view(), hover.tile) else { return };
    let mine: Vec<Key> = v.units.iter().filter(|u| u.owner == v.me && u.tile == tile).map(|u| (u.id, u.r#gen)).collect();
    let enemy_unit = v.units.iter().find(|u| u.owner != v.me && u.tile == tile).map(|u| (u.id, u.r#gen));
    let city = v.cities.iter().find(|c| c.centre == tile);
    let enemy_city = city.filter(|c| c.owner != v.me).map(|c| (c.id, c.r#gen));
    if let Some(unit) = selection.unit {
        if selection.targets.contains(&tile) && (enemy_unit.is_some() || enemy_city.is_some()) {
            if selection.armed == Some(tile) {
                let target = match enemy_unit {
                    Some(k) => AttackTarget::Unit { unit: entity(k) },
                    None => AttackTarget::City { city: entity(enemy_city.expect("checked above")) },
                };
                outbox.submit(Action::Attack { unit: entity(unit), target });
                selection.armed = None;
            } else {
                selection.armed = Some(tile);
            }
            return;
        }
        let reachable = selection.reach.iter().any(|r| r.tile == tile);
        if reachable && !mine.contains(&unit) && enemy_unit.is_none() {
            outbox.submit(Action::MoveUnit { unit: entity(unit), to: tile });
            return;
        }
    }
    if !mine.is_empty() {
        // A second click on the same tile cycles its stack.
        let next = match selection.unit.and_then(|u| mine.iter().position(|k| *k == u)) {
            Some(i) => mine[(i + 1) % mine.len()],
            None => mine[0],
        };
        select_unit(&mut selection, next);
        return;
    }
    if let Some(c) = city.filter(|c| c.owner == v.me) {
        selection.unit = None;
        selection.city = Some((c.id, c.r#gen));
        selection.city_options = None;
        selection.asked += 1;
        ask(&link, &mut selection, "city", Ask::CityOptions { city: entity((c.id, c.r#gen)) });
        return;
    }
    selection.unit = None;
    selection.city = None;
    selection.armed = None;
}

/// Select a unit and ask what it can do (the next view change asks again).
pub fn select_unit(selection: &mut Selection, unit: Key) {
    selection.unit = Some(unit);
    selection.city = None;
    selection.armed = None;
    selection.reach.clear();
    selection.options = None;
    selection.targets.clear();
    // Ask on the next refresh: `asked` = 0 forces it.
    selection.asked = 0;
}

/// Ask for the combat preview of the target under the cursor, or the armed one.
fn preview(hover: Res<Hover>, view: Res<ViewState>, link: Res<Link>, mut selection: ResMut<Selection>) {
    let (Some(v), Some(unit)) = (view.view(), selection.unit) else { return };
    let tile = selection.armed.or(hover.tile).filter(|t| selection.targets.contains(t));
    if tile == selection.previewing {
        return;
    }
    selection.previewing = tile;
    selection.preview = None;
    let Some(tile) = tile else { return };
    let target = match v.units.iter().find(|u| u.owner != v.me && u.tile == tile) {
        Some(u) => AttackTarget::Unit { unit: EntityRef { id: u.id, r#gen: u.r#gen } },
        None => match v.cities.iter().find(|c| c.centre == tile && c.owner != v.me) {
            Some(c) => AttackTarget::City { city: EntityRef { id: c.id, r#gen: c.r#gen } },
            None => return,
        },
    };
    ask(&link, &mut selection, "preview", Ask::CombatPreview { unit: entity(unit), target });
}

/// Reach and targets: a white 18% fill with a 0.03 m outline (dashed where uncertain), red for targets; a ring
/// under the selected unit.
#[allow(clippy::too_many_arguments)]
fn highlight(
    mut commands: Commands,
    selection: Res<Selection>,
    land: Option<Res<Land>>,
    view: Res<ViewState>,
    material: Res<HighlightMaterial>,
    existing: Query<Entity, With<Highlight>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut last: Local<Option<(Option<Key>, usize, usize, Option<u16>, u32)>>,
) {
    let Some(land) = land else { return };
    let unit_tile = selection
        .unit
        .and_then(|k| view.view().and_then(|v| v.units.iter().find(|u| (u.id, u.r#gen) == k)))
        .map(|u| u.tile as u32)
        .unwrap_or(u32::MAX);
    let key = (selection.unit, selection.reach.len(), selection.targets.len(), selection.armed, unit_tile);
    if *last == Some(key) && !land.is_changed() {
        return;
    }
    *last = Some(key);
    for e in &existing {
        commands.entity(e).despawn();
    }
    let relief = &land.0;
    let mut b = Builder::default();
    let fill = |b: &mut Builder, tile: u16, colour: LinearRgba| {
        let c = relief.grid.centre(tile);
        let ring: Vec<Vec2> = (0..=6).map(|k| c + hex::corner((k + 5) % 6) * 0.94).collect();
        for pair in ring.windows(2) {
            b.fan(relief, c, pair[0], pair[1], 0.02, colour);
        }
    };
    let outline = |b: &mut Builder, tile: u16, colour: LinearRgba, dashed: bool| {
        let c = relief.grid.centre(tile);
        for k in 0..6 {
            let (a, z) = (c + hex::corner((k + 5) % 6) * 0.94, c + hex::corner(k) * 0.94);
            if dashed {
                for i in (0..6).step_by(2) {
                    let (s, e) = (a.lerp(z, i as f32 / 6.0), a.lerp(z, (i + 1) as f32 / 6.0));
                    b.ribbon(relief, &[s, e], 0.03, 0.03, colour);
                }
            } else {
                b.ribbon(relief, &[a, z], 0.03, 0.03, colour);
            }
        }
    };
    for r in &selection.reach {
        if selection.targets.contains(&r.tile) {
            continue;
        }
        fill(&mut b, r.tile, LinearRgba::new(1.0, 1.0, 1.0, 0.18));
        outline(&mut b, r.tile, LinearRgba::new(1.0, 1.0, 1.0, 0.6), r.uncertain);
    }
    for t in &selection.targets {
        let strong = selection.armed == Some(*t);
        fill(&mut b, *t, LinearRgba::new(0.88, 0.29, 0.23, if strong { 0.4 } else { 0.25 }));
        outline(&mut b, *t, LinearRgba::new(0.88, 0.29, 0.23, 0.9), false);
    }
    if unit_tile != u32::MAX {
        let c = relief.grid.centre(unit_tile as u16);
        let ring: Vec<Vec2> = (0..=24).map(|i| c + Vec2::from_angle(std::f32::consts::TAU * i as f32 / 24.0) * 0.42).collect();
        b.ribbon(relief, &ring, 0.035, 0.035, LinearRgba::new(1.0, 1.0, 1.0, 0.9));
    }
    if let Some(mesh) = b.finish() {
        commands.spawn((Highlight, Mesh3d(meshes.add(mesh)), MeshMaterial3d(material.0.clone()), NotShadowCaster));
    }
}

/// The next own unit awaiting orders after `after`, in id order: moves left, not fortified, not automated, not
/// told to wait.
pub fn awaiting(view: &ViewState, selection: &Selection, after: Option<Key>) -> Option<Key> {
    let v = view.view()?;
    let mut ready: Vec<Key> = v
        .units
        .iter()
        .filter(|u| u.owner == v.me && u.moves > 0.0 && !u.fortified)
        .filter(|u| u.own.as_ref().is_none_or(|o| o.automation.is_none()))
        .map(|u| (u.id, u.r#gen))
        .filter(|k| !selection.skipped.contains(k))
        .collect();
    ready.sort();
    match after.and_then(|a| ready.iter().position(|k| *k == a)) {
        Some(i) => ready.get(i + 1).or(ready.first()).copied(),
        None => ready.first().copied(),
    }
}

/// The next name in the nation's list not yet used by any city in sight.
pub fn city_name(view: &ViewState, rules: &Rules) -> String {
    let Some(v) = view.view() else { return "New City".into() };
    let nation = v.civs.iter().find(|c| c.id == v.me).map(|c| c.nation);
    let names = nation.and_then(|n| rules.tables.nations.iter().find(|row| row.id == n)).map(|n| n.cities.clone()).unwrap_or_default();
    names
        .into_iter()
        .find(|name| !v.cities.iter().any(|c| &c.name == name))
        .unwrap_or_else(|| format!("City {}", v.cities.len() + 1))
}

/// The action a unit-panel button sends, for the kinds M3 offers.
pub fn unit_action(kind: UnitActionKind, unit: Key, view: &ViewState, rules: &Rules) -> Option<Action> {
    let unit = entity(unit);
    Some(match kind {
        UnitActionKind::Fortify => Action::Fortify { unit },
        UnitActionKind::Skip => Action::SkipUnit { unit },
        UnitActionKind::FoundCity => Action::FoundCity { unit, name: city_name(view, rules) },
        UnitActionKind::Automate => Action::AutomateUnit { unit },
        UnitActionKind::Explore => Action::ExploreUnit { unit },
        UnitActionKind::Disband => Action::DisbandUnit { unit },
        UnitActionKind::Pillage => Action::Pillage { unit },
        UnitActionKind::SetUp => Action::SetUp { unit },
        _ => return None,
    })
}
