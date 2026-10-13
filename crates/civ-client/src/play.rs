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
    /// The tech tree is open.
    pub research_open: bool,
    /// A tile for sale clicked once: a second click buys it.
    pub armed_tile: Option<u16>,
    /// In a room, End turn clicked once: a second click ends it, since it cannot be taken back there.
    pub end_armed: bool,
    /// Home clicked once in a solo match: a second click leaves it, and the match is lost.
    pub home_armed: bool,
    /// The open city's next map click picks where its current wonder stands.
    pub siting: bool,
    /// A mission the selected unit is aiming (`rebase`, `airSweep`, `paradrop`, `nuclearStrike`): its targets
    /// are marked and the next click on one sends it.
    pub mission: Option<UnitActionKind>,
    /// What a nuclear strike on the armed tile would do.
    pub nuclear: Option<civ_sim::queries::NuclearPreviewView>,
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

/// A city tile's yields (or price), over the tile while the city panel is open.
#[derive(Component)]
struct TileLabel {
    at: Vec3,
}

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
            (refresh, answers, aim, receipts, click, preview, highlight, tile_labels, place_labels)
                .chain()
                .run_if(playing)
                .after(crate::map::MapSet::Rebuild)
                .in_set(crate::app::InMatch),
        )
        .add_systems(OnExit(crate::app::AppState::Match), crate::app::reset::<Selection>);
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
    selection.nuclear = None;
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
            ("unit", QueryResult::UnitOptions { options }) => selection.options = options.clone(),
            ("nuke", QueryResult::UnitOptions { options }) => {
                selection.nuclear = options.as_ref().and_then(|o| o.nuclear_preview.clone());
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

/// What the selected unit could attack from where it stands: hostile units and cities (at war, or raiders)
/// next to it, or for a ranged unit within its range; the engine attacks without moving first. `unitOptions`
/// offers `attack` without targets (its `targets` are for air and nuclear missions), so the client aims and the
/// engine decides (`tests::aimed_targets_are_attacks_the_engine_takes`).
fn aim(view: Res<ViewState>, rules: Option<Res<Rules>>, land: Option<Res<Land>>, mut selection: ResMut<Selection>) {
    if !selection.is_changed() && !view.is_changed() {
        return;
    }
    let mission = selection.mission.and_then(|kind| {
        selection.options.as_ref()?.targets.as_ref()?.iter().find(|t| t.action == kind).map(|t| t.tiles.clone())
    });
    let targets = match (view.view(), rules, land, selection.unit) {
        (_, _, _, Some(_)) if mission.is_some() => mission.unwrap_or_default(),
        (Some(v), Some(rules), Some(land), Some(key)) => attackable(v, &rules, &land.0.grid, &selection, key),
        // An open city aims its bombardment at what `cityOptions` lists.
        (Some(v), ..) if selection.city.is_some() => bombardable(v, &selection).into_iter().map(|(t, _)| t).collect(),
        _ => Vec::new(),
    };
    if selection.targets != targets {
        selection.targets = targets;
    }
    // Choosing a wonder's site marks its tiles as the reach.
    if selection.unit.is_none() {
        let sites: Vec<civ_sim::Reach> = match (selection.siting, view.view()) {
            (true, Some(v)) => sites(v, &selection)
                .map(|(_, s)| s.tiles.iter().map(|&tile| civ_sim::Reach { tile, moves_left: 0.0, uncertain: false }).collect())
                .unwrap_or_default(),
            _ => Vec::new(),
        };
        if selection.reach != sites {
            selection.reach = sites;
        }
    }
}

/// Enemy units the open city may bombard, by tile.
fn bombardable(v: &civ_sim::PlayerView, selection: &Selection) -> Vec<(u16, EntityRef)> {
    let Some(options) = &selection.city_options else { return Vec::new() };
    options
        .bombard
        .iter()
        .filter_map(|b| v.units.iter().find(|u| (u.id, u.r#gen) == (b.unit.id, b.unit.r#gen)).map(|u| (u.tile, b.unit.clone())))
        .collect()
}

/// The open city's current wonder and where it may stand.
fn sites<'a>(v: &civ_sim::PlayerView, selection: &'a Selection) -> Option<(u32, &'a civ_sim::queries::SiteChoice)> {
    let key = selection.city?;
    let city = v.cities.iter().find(|c| (c.id, c.r#gen) == key)?;
    let front = city.queue_refs.as_ref()?.first()?.clone();
    let civ_sim::protocol::ConstructionRef::Building { id } = front else { return None };
    let choice = selection.city_options.as_ref()?.choices.iter().find(|c| c.construction == front)?;
    Some((id, choice.sites.as_ref()?))
}

fn attackable(v: &civ_sim::PlayerView, rules: &Rules, grid: &crate::hex::Grid, selection: &Selection, key: Key) -> Vec<u16> {
    let offers_attack = selection.options.as_ref().is_some_and(|o| o.actions.iter().any(|a| a.action == UnitActionKind::Attack));
    let Some(unit) = v.units.iter().find(|u| (u.id, u.r#gen) == key) else { return Vec::new() };
    if !offers_attack {
        return Vec::new();
    }
    let raiders = |civ: u8| {
        let nation = v.civs.iter().find(|c| c.id == civ).map(|c| c.nation);
        rules.tables.nations.iter().any(|n| Some(n.id) == nation && n.barbarian == Some(true))
    };
    let hostile = |civ: u8| civ != v.me && (raiders(civ) || v.relations.iter().any(|r| r.civ == civ && r.at_war));
    let range = rules.tables.units.iter().find(|r| r.id == unit.base).filter(|r| r.ranged_strength > 0).map(|r| r.range);
    let near = |tile: u16| grid.distance(unit.tile, tile) <= range.unwrap_or(1);
    let mut out: Vec<u16> = v
        .units
        .iter()
        .filter(|u| hostile(u.owner))
        .map(|u| u.tile)
        .chain(v.cities.iter().filter(|c| hostile(c.owner)).map(|c| c.centre))
        .filter(|t| near(*t))
        .collect();
    out.sort_unstable();
    out.dedup();
    out
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
pub(crate) fn over_ui(hover: &HoverMap, nodes: &Query<(), With<Node>>) -> bool {
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
    mut outbox: ResMut<Outbox>,
    mut selection: ResMut<Selection>,
    mut pressed_at: Local<Option<Vec2>>,
    link: Res<Link>,
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
    // An open city first: placing its wonder, then bombarding.
    if let (None, Some(city)) = (selection.unit, selection.city) {
        if selection.siting {
            if let Some((building, choice)) = sites(v, &selection).filter(|(_, s)| s.tiles.contains(&tile)) {
                let _ = choice;
                outbox.submit(Action::ChooseSite { city: entity(city), building, tile });
            }
            selection.siting = false;
            return;
        }
        if let Some((_, target)) = bombardable(v, &selection).into_iter().find(|(t, _)| *t == tile) {
            if selection.armed == Some(tile) {
                outbox.submit(Action::Bombard { city: entity(city), target });
                selection.armed = None;
            } else {
                selection.armed = Some(tile);
            }
            return;
        }
    }
    // While an own city's panel is open, clicks on its tiles manage the city (docs/PLAY-SPEC.md, M4, Q2).
    if let Some(action) = city_tile_click(&mut selection, v, tile) {
        if let Some(action) = action {
            outbox.submit(action);
        }
        return;
    }
    let mine: Vec<Key> = v.units.iter().filter(|u| u.owner == v.me && u.tile == tile).map(|u| (u.id, u.r#gen)).collect();
    let enemy_unit = v.units.iter().find(|u| u.owner != v.me && u.tile == tile).map(|u| (u.id, u.r#gen));
    let city = v.cities.iter().find(|c| c.centre == tile);
    let enemy_city = city.filter(|c| c.owner != v.me).map(|c| (c.id, c.r#gen));
    if let (Some(unit), Some(kind)) = (selection.unit, selection.mission) {
        if selection.targets.contains(&tile) {
            let u = entity(unit);
            match kind {
                UnitActionKind::Rebase => outbox.submit(Action::Rebase { unit: u, to: tile }),
                UnitActionKind::AirSweep => outbox.submit(Action::AirSweep { unit: u, target: tile }),
                UnitActionKind::Paradrop => outbox.submit(Action::Paradrop { unit: u, to: tile }),
                // A strike asks first what it would do, and goes on the second click.
                UnitActionKind::NuclearStrike if selection.armed == Some(tile) => {
                    outbox.submit(Action::NuclearStrike { unit: u, target: tile })
                }
                UnitActionKind::NuclearStrike => {
                    selection.armed = Some(tile);
                    selection.nuclear = None;
                    ask(&link, &mut selection, "nuke", Ask::UnitOptions { unit: u, nuclear_target: Some(tile) });
                    return;
                }
                _ => String::new(),
            };
        }
        selection.mission = None;
        selection.armed = None;
        return;
    }
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
        select_city(&mut selection, (c.id, c.r#gen));
        return;
    }
    selection.unit = None;
    selection.city = None;
    selection.armed = None;
}

/// What a click on `tile` does while a city panel is open: `None` when the tile is not the city's to manage;
/// `Some(None)` when it only arms a purchase.
fn city_tile_click(selection: &mut Selection, v: &civ_sim::PlayerView, tile: u16) -> Option<Option<Action>> {
    let key = selection.city?;
    let options = selection.city_options.as_ref()?;
    let city = v.cities.iter().find(|c| (c.id, c.r#gen) == key)?;
    let own = city.own.as_ref()?;
    if let Some(sale) = options.purchasable_tiles.iter().find(|t| t.tile == tile) {
        if selection.armed_tile == Some(tile) && sale.affordable {
            selection.armed_tile = None;
            return Some(Some(Action::BuyTile { city: entity(key), tile }));
        }
        selection.armed_tile = Some(tile);
        return Some(None);
    }
    let workable = options.workable_tiles.iter().find(|t| t.tile == tile)?;
    selection.armed_tile = None;
    if workable.worked {
        let locked = own.locked_tiles.contains(&tile);
        return Some(Some(Action::LockTile { city: entity(key), tile, locked: !locked }));
    }
    if workable.taken {
        return Some(None);
    }
    Some(reassign(city, own, options, Change::Work(tile)).map(|(worked, specialists)| Action::AssignCitizens {
        city: entity(key),
        worked,
        specialists,
    }))
}

/// A change to who a city's citizens work.
pub enum Change {
    Work(u16),
    AddSpecialist(u32),
    RemoveSpecialist(u32),
}

/// The city's new worked tiles and specialists after `change`: a citizen comes from the weakest unpinned worked
/// tile when none is free, and a specialist sent back works the best free tile.
pub fn reassign(
    city: &civ_sim::CityView,
    own: &civ_sim::CityStateView,
    options: &CityOptions,
    change: Change,
) -> Option<(Vec<u16>, Vec<civ_sim::protocol::SpecialistAssignment>)> {
    let total = |t: &civ_sim::queries::WorkableTile| t.food + t.production + t.gold + t.science + t.culture + t.faith;
    let yield_of = |tile: u16| options.workable_tiles.iter().find(|t| t.tile == tile).map_or(0, total);
    let mut worked = own.worked.clone();
    let mut specialists: Vec<civ_sim::protocol::SpecialistAssignment> = own
        .specialists
        .iter()
        .map(|s| civ_sim::protocol::SpecialistAssignment { specialist: s.specialist, count: s.count })
        .collect();
    let employed = worked.len() as u32 + specialists.iter().map(|s| s.count).sum::<u32>();
    let mut free = city.population.saturating_sub(employed);
    let mut free_one = |worked: &mut Vec<u16>| -> bool {
        if free > 0 {
            free -= 1;
            return true;
        }
        let weakest = worked.iter().copied().filter(|t| !own.locked_tiles.contains(t)).min_by_key(|t| yield_of(*t));
        match weakest {
            Some(t) => {
                worked.retain(|w| *w != t);
                true
            }
            None => false,
        }
    };
    match change {
        Change::Work(tile) => {
            if !free_one(&mut worked) {
                return None;
            }
            worked.push(tile);
        }
        Change::AddSpecialist(kind) => {
            if !free_one(&mut worked) {
                return None;
            }
            match specialists.iter_mut().find(|s| s.specialist == kind) {
                Some(s) => s.count += 1,
                None => specialists.push(civ_sim::protocol::SpecialistAssignment { specialist: kind, count: 1 }),
            }
        }
        Change::RemoveSpecialist(kind) => {
            let s = specialists.iter_mut().find(|s| s.specialist == kind && s.count > 0)?;
            s.count -= 1;
            let best = options.workable_tiles.iter().filter(|t| !t.worked && !t.taken && !worked.contains(&t.tile)).max_by_key(|t| total(t));
            if let Some(t) = best {
                worked.push(t.tile);
            }
        }
    }
    specialists.retain(|s| s.count > 0);
    Some((worked, specialists))
}

/// Open an own city's panel; its options are asked on the next refresh.
pub fn select_city(selection: &mut Selection, city: Key) {
    selection.unit = None;
    selection.city = Some(city);
    selection.city_options = None;
    selection.armed = None;
    selection.asked = 0;
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
    let city_key = selection.city_options.as_ref().map(|o| o.workable_tiles.iter().filter(|t| t.worked).map(|t| t.tile as u32 + 1).sum::<u32>() + o.purchasable_tiles.len() as u32 * 7919);
    let key = (selection.unit, selection.reach.len(), selection.targets.len(), selection.armed.or(selection.armed_tile), unit_tile ^ city_key.unwrap_or(0));
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
    // An open city's tiles: worked in gold, pinned brighter, for sale in green.
    if let (Some(options), Some(city)) = (selection.city_options.as_ref(), selection.city.and_then(|k| view.view().and_then(|v| v.cities.iter().find(|c| (c.id, c.r#gen) == k)))) {
        let locked: Vec<u16> = city.own.as_ref().map(|o| o.locked_tiles.clone()).unwrap_or_default();
        for t in &options.workable_tiles {
            if t.worked {
                let alpha = if locked.contains(&t.tile) { 0.38 } else { 0.22 };
                fill(&mut b, t.tile, LinearRgba::new(0.95, 0.78, 0.33, alpha));
                outline(&mut b, t.tile, LinearRgba::new(0.95, 0.78, 0.33, 0.85), false);
            } else if !t.taken {
                outline(&mut b, t.tile, LinearRgba::new(1.0, 1.0, 1.0, 0.35), true);
            }
        }
        for t in &options.purchasable_tiles {
            let strong = selection.armed_tile == Some(t.tile);
            fill(&mut b, t.tile, LinearRgba::new(0.45, 0.85, 0.5, if strong { 0.35 } else { 0.15 }));
            outline(&mut b, t.tile, LinearRgba::new(0.45, 0.85, 0.5, 0.8), !strong);
        }
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
    cycle(view, selection, after, false)
}

/// The unit that needs orders before `before`, wrapping around.
pub fn awaiting_before(view: &ViewState, selection: &Selection, before: Option<Key>) -> Option<Key> {
    cycle(view, selection, before, true)
}

fn cycle(view: &ViewState, selection: &Selection, from: Option<Key>, back: bool) -> Option<Key> {
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
    if back {
        ready.reverse();
    }
    match from.and_then(|a| ready.iter().position(|k| *k == a)) {
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
        UnitActionKind::Gift => Action::GiftUnit { unit },
        UnitActionKind::AssembleSpaceshipPart => Action::AssembleSpaceshipPart { unit },
        _ => return None,
    })
}

/// Yields over an open city's workable tiles (food, production, gold, science, culture, faith; zeros left out),
/// and prices over the tiles for sale.
#[allow(clippy::too_many_arguments)]
fn tile_labels(
    mut commands: Commands,
    selection: Res<Selection>,
    land: Option<Res<Land>>,
    font: Res<crate::inspect::UiFont>,
    existing: Query<Entity, With<TileLabel>>,
) {
    let Some(land) = land else { return };
    if !selection.is_changed() {
        return;
    }
    for e in &existing {
        commands.entity(e).despawn();
    }
    let Some(options) = selection.city.and(selection.city_options.as_ref()) else { return };
    let relief = &land.0;
    let at = |tile: u16| {
        let g = relief.grid.centre(tile);
        hex::world(g, relief.height(g).unwrap_or(0.0).max(0.0) + 0.25)
    };
    let span = |s: String, c: Color| (TextSpan::new(s), TextFont { font: font.semibold.clone().into(), font_size: FontSize::Px(12.0), ..default() }, TextColor(c));
    for t in &options.workable_tiles {
        let parts = [
            (t.food, "F", Color::srgb(0.55, 0.85, 0.4)),
            (t.production, "P", Color::srgb(0.95, 0.6, 0.3)),
            (t.gold, "G", Color::srgb(0.98, 0.85, 0.35)),
            (t.science, "S", Color::srgb(0.45, 0.7, 1.0)),
            (t.culture, "C", Color::srgb(0.8, 0.55, 0.95)),
            (t.faith, "Fa", Color::srgb(0.95, 0.95, 0.95)),
        ];
        let mut label = commands.spawn((
            TileLabel { at: at(t.tile) },
            Text::default(),
            TextFont { font: font.semibold.clone().into(), font_size: FontSize::Px(12.0), ..default() },
            Node { position_type: PositionType::Absolute, ..default() },
            TextShadow::default(),
            Visibility::Hidden,
        ));
        label.with_children(|l| {
            for (n, letter, colour) in parts {
                if n != 0 {
                    l.spawn(span(format!("{n}{letter} "), colour));
                }
            }
        });
    }
    for t in &options.purchasable_tiles {
        commands.spawn((
            TileLabel { at: at(t.tile) + Vec3::Y * 0.15 },
            Text::new(format!("{} gold", t.gold)),
            TextFont { font: font.semibold.clone().into(), font_size: FontSize::Px(12.0), ..default() },
            TextColor(if t.affordable { Color::srgb(0.6, 0.95, 0.6) } else { Color::srgb(0.6, 0.65, 0.6) }),
            TextShadow::default(),
            Node { position_type: PositionType::Absolute, ..default() },
            Visibility::Hidden,
        ));
    }
}

fn place_labels(
    cameras: Query<(&Camera, &GlobalTransform), With<MapCamera>>,
    mut labels: Query<(&TileLabel, &mut Node, &mut Visibility, &ComputedNode)>,
) {
    let Ok((camera, at)) = cameras.single() else { return };
    for (label, mut node, mut visibility, computed) in &mut labels {
        match camera.world_to_viewport(at, label.at) {
            Ok(s) => {
                let size = computed.size() * computed.inverse_scale_factor();
                node.left = Val::Px(s.x - size.x / 2.0);
                node.top = Val::Px(s.y - size.y / 2.0);
                *visibility = Visibility::Inherited;
            }
            Err(_) => *visibility = Visibility::Hidden,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use civ_sim::protocol::{CommandRequest, CommandStatus};
    use civ_sim::session::Session;
    use civ_sim::Settings;

    /// Seat 0 idles while bots and raiders play; whenever one of its units has something to aim at, every
    /// aimed target is attacked through the engine. The aim is a guess the engine checks, so it must be right
    /// most of the time and must find the fights that happen.
    #[test]
    fn aimed_targets_are_attacks_the_engine_takes() {
        let settings = Settings { seed: 7, width: 24, height: 16, majors: 2, city_states: 1, ..Settings::default() };
        let mut session = Session::new(settings, "aim".into(), vec![0]).unwrap();
        let tables = civ_sim::tables(&civ_sim::content::load().unwrap());
        let rules = Rules::new(session.content_hash(), tables);
        let (mut tried, mut taken) = (0, 0);
        let mut n = 0;
        let mut send = |session: &mut Session, action: Action| {
            n += 1;
            let update = session.current_update(0).unwrap();
            let request = CommandRequest {
                command_id: format!("c{n}"),
                epoch: update.cursor.epoch.clone(),
                turn_token: update.turn_token.clone(),
                action,
            };
            let transition = session.stage_command(0, request).unwrap();
            let status = transition.receipt.result.clone();
            *session = transition.next;
            status
        };
        for _ in 0..150 {
            let v = session.current_update(0).unwrap().view;
            let grid = crate::hex::Grid { width: v.width, height: v.height, wrap_x: v.wrap_x };
            for unit in v.units.iter().filter(|u| u.owner == v.me) {
                let key = (unit.id, unit.r#gen);
                let ask = |q| session.query(0, q).unwrap().result;
                let QueryResult::UnitOptions { options } = ask(Ask::UnitOptions { unit: entity(key), nuclear_target: None }) else { panic!() };
                let QueryResult::Reachable { reach } = ask(Ask::Reachable { unit: entity(key) }) else { panic!() };
                let selection = Selection { options, reach, ..Selection::default() };
                for tile in attackable(&v, &rules, &grid, &selection, key) {
                    let target = match v.units.iter().find(|u| u.owner != v.me && u.tile == tile) {
                        Some(u) => AttackTarget::Unit { unit: EntityRef { id: u.id, r#gen: u.r#gen } },
                        None => {
                            let c = v.cities.iter().find(|c| c.centre == tile).unwrap();
                            AttackTarget::City { city: EntityRef { id: c.id, r#gen: c.r#gen } }
                        }
                    };
                    tried += 1;
                    if send(&mut session, Action::Attack { unit: entity(key), target }) == CommandStatus::Applied {
                        taken += 1;
                    }
                    break;
                }
            }
            if tried >= 10 {
                break;
            }
            send(&mut session, Action::FinishTurn);
            if session.finished() {
                break;
            }
        }
        assert!(tried >= 3, "only {tried} chances to attack came up");
        assert!(taken * 10 >= tried * 8, "the engine took {taken} of {tried} aimed attacks");
        // The fights earned experience: a promotion offered is one the engine takes.
        let v = session.current_update(0).unwrap().view;
        let mut promoted = 0;
        for unit in v.units.iter().filter(|u| u.owner == v.me) {
            let QueryResult::UnitOptions { options: Some(o) } =
                session.query(0, Ask::UnitOptions { unit: entity((unit.id, unit.r#gen)), nuclear_target: None }).unwrap().result
            else {
                continue;
            };
            if let Some(p) = o.promotions.first() {
                assert_eq!(send(&mut session, Action::Promote { unit: entity((unit.id, unit.r#gen)), promotion: p.id }), CommandStatus::Applied);
                promoted += 1;
            }
        }
        assert!(promoted > 0, "no unit earned a promotion");
    }

    /// The city panel's queue edits: append, raise and remove send whole queues the engine keeps as sent.
    #[test]
    fn queue_edits_are_kept_as_sent() {
        let mut session = Session::new(Settings { seed: 3, ..Settings::default() }, "q".into(), vec![0]).unwrap();
        let tables = civ_sim::tables(&civ_sim::content::load().unwrap());
        let mut n = 0;
        let mut send = |session: &mut Session, action: Action| {
            n += 1;
            let update = session.current_update(0).unwrap();
            let request = CommandRequest { command_id: format!("q{n}"), epoch: update.cursor.epoch.clone(), turn_token: update.turn_token.clone(), action };
            let t = session.stage_command(0, request).unwrap();
            let status = t.receipt.result.clone();
            *session = t.next;
            status
        };
        let v = session.current_update(0).unwrap().view;
        let settler = tables.units.iter().find(|u| u.name == "Settler").unwrap().id;
        let unit = v.units.iter().find(|u| u.owner == v.me && u.base == settler).unwrap();
        assert_eq!(send(&mut session, Action::FoundCity { unit: entity((unit.id, unit.r#gen)), name: "Queueburg".into() }), CommandStatus::Applied);
        let city = session.current_update(0).unwrap().view.cities.iter().find(|c| c.name == "Queueburg").unwrap().clone();
        let key = entity((city.id, city.r#gen));
        let QueryResult::CityOptions { options: Some(o) } = session.query(0, Ask::CityOptions { city: key.clone() }).unwrap().result else { panic!() };
        let choices: Vec<_> = o.choices.iter().filter(|c| c.refusal.is_none()).map(|c| c.construction.clone()).take(3).collect();
        assert_eq!(choices.len(), 3);
        let queue = |s: &Session| s.current_update(0).unwrap().view.cities.iter().find(|c| c.name == "Queueburg").unwrap().queue_refs.clone().unwrap();
        assert_eq!(send(&mut session, Action::SetProduction { city: key.clone(), queue: choices.clone() }), CommandStatus::Applied);
        assert_eq!(queue(&session), choices);
        let mut raised = choices.clone();
        raised.swap(1, 2);
        assert_eq!(send(&mut session, Action::SetProduction { city: key.clone(), queue: raised.clone() }), CommandStatus::Applied);
        assert_eq!(queue(&session), raised);
        raised.remove(0);
        assert_eq!(send(&mut session, Action::SetProduction { city: key, queue: raised.clone() }), CommandStatus::Applied);
        assert_eq!(queue(&session), raised);
    }
}
