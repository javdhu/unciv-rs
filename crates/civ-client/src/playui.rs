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
use civ_sim::protocol::{Action, CityFocus, ConstructionRef, Currency, GreatPersonAction, ProductionConversion, ReligiousAction};
use civ_sim::queries::{Availability, UnitActionKind};

pub(crate) const PANEL: Color = Color::srgba(0x14 as f32 / 255.0, 0x16 as f32 / 255.0, 0x1B as f32 / 255.0, 0.85);
pub(crate) const TEXT: Color = Color::srgb(0.93, 0.94, 0.95);
pub(crate) const DIM: Color = Color::srgb(0.6, 0.63, 0.67);
pub(crate) const BUTTON: Color = Color::srgba(1.0, 1.0, 1.0, 0.07);
pub(crate) const BUTTON_ON: Color = Color::srgba(1.0, 1.0, 1.0, 0.22);
const TOAST_SECONDS: f32 = 3.0;
pub(crate) const URGENT: Color = Color::srgb(1.0, 0.45, 0.4);

/// What a play button does.
#[derive(Component, Clone, PartialEq, Debug)]
enum Order {
    Turn,
    Undo,
    Research,
    Act(UnitActionKind),
    Build(u32),
    Produce(ConstructionRef),
    Buy(ConstructionRef),
    CloseCity,
    Focus(CityFocus),
    AvoidGrowth(bool),
    /// A specialist kind, and whether one is added (or sent back to the tiles).
    Specialist(u32, bool),
    Sell(u32),
    /// Leave a solo match for the home screen.
    Home,
    Promote(u32),
    Upgrade,
    /// Add to the end of the open city's queue.
    Queue(ConstructionRef),
    /// Take the queue's nth item out, or move it one up.
    Unqueue(usize),
    Raise(usize),
    Convert(Option<ProductionConversion>),
    Fate(Fate),
    /// Choose where the open city's current wonder stands: the next click on one of its sites.
    Site,
    /// Put off the decision End turn leads with.
    Later,
    GreatPerson(GreatPersonAction),
    Religious(ReligiousAction),
    /// Aim a mission at one of its targets on the map.
    Mission(UnitActionKind),
    BuyWithFaith(ConstructionRef),
}

/// What becomes of a city taken or held as a puppet.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) enum Fate {
    Keep,
    Annex,
    Raze,
    StopRazing,
    Liberate,
}

impl Fate {
    pub(crate) fn action(self, city: civ_sim::protocol::EntityRef) -> Action {
        match self {
            Fate::Keep => Action::KeepPuppet { city },
            Fate::Annex => Action::Annex { city },
            Fate::Raze => Action::Raze { city },
            Fate::StopRazing => Action::StopRazing { city },
            Fate::Liberate => Action::LiberateCity { city },
        }
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Fate::Keep => "Keep as a puppet",
            Fate::Annex => "Annex",
            Fate::Raze => "Raze",
            Fate::StopRazing => "Stop razing",
            Fate::Liberate => "Liberate",
        }
    }
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
struct PreviewCard;
#[derive(Component)]
struct ToastBox(f32);

/// Asks a panel to rebuild.
#[derive(Resource, Default)]
struct Stale(bool);

pub fn plugin(app: &mut App) {
    app.init_resource::<Stale>()
        .add_systems(OnEnter(crate::app::AppState::Match), frame.run_if(playing))
        .add_systems(OnExit(crate::app::AppState::Match), crate::app::reset::<Stale>)
        .add_systems(
            Update,
            (mark, top_bar, unit_panel, turn_button, city_panel, preview_card, toasts, keys, shade)
                .chain()
                .run_if(playing)
                .after(crate::map::MapSet::Rebuild)
                .in_set(crate::app::InMatch),
        );
}

pub(crate) fn text(font: &UiFont, s: impl Into<String>, size: f32, colour: Color) -> impl Bundle {
    (
        Text::new(s),
        TextFont { font: font.regular.clone().into(), font_size: FontSize::Px(size), ..default() },
        TextColor(colour),
    )
}

fn button(font: &UiFont, order: Order, label: impl Into<String>, enabled: bool) -> impl Bundle {
    styled(
        font,
        order,
        label,
        enabled,
        Node {
            min_height: Val::Px(28.0),
            padding: UiRect::axes(Val::Px(10.0), Val::Px(4.0)),
            align_items: AlignItems::Center,
            justify_content: JustifyContent::Center,
            border_radius: BorderRadius::all(Val::Px(4.0)),
            ..default()
        },
    )
}

fn styled(font: &UiFont, order: Order, label: impl Into<String>, enabled: bool, node: Node) -> impl Bundle {
    (
        order,
        Button,
        Hovered::default(),
        observe(press),
        node,
        BackgroundColor(BUTTON),
        children![text(font, label, 13.0, if enabled { TEXT } else { DIM })],
    )
}

fn panel(node: Node) -> impl Bundle {
    (node, BackgroundColor(PANEL), Visibility::Hidden, crate::screens::PANEL_Z)
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
        GlobalZIndex(21),
        children![
            (TopLeft, Node { width: Val::Px(260.0), ..default() }),
            (TopCentre, Node { column_gap: Val::Px(6.0), ..default() }),
            (
                TopRight,
                Node { width: Val::Px(620.0), justify_content: JustifyContent::FlexEnd, column_gap: Val::Px(10.0), align_items: AlignItems::Center, ..default() }
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
        crate::screens::PANEL_Z,
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

pub(crate) fn clear(commands: &mut Commands, children: Option<&Children>) {
    if let Some(children) = children {
        for c in children.iter() {
            commands.entity(c).despawn();
        }
    }
}

fn year(y: i32) -> String {
    if y < 0 { format!("{} BC", -y) } else { format!("AD {y}") }
}

/// A city's faiths: its majority, holy city, and followers of each.
fn faith_line(own: &civ_sim::CityStateView, rules: &Rules) -> Option<String> {
    let religion = &own.religion;
    let name = |key: u8| {
        rules.tables.faiths.iter().flatten().find(|f| f.key == key).map_or("a faith".to_string(), |f| f.name.clone())
    };
    if religion.followers.is_empty() {
        return None;
    }
    let mut parts: Vec<String> = religion.followers.iter().map(|f| format!("{} {}", name(f.founder), f.followers)).collect();
    parts.sort();
    let mut line = format!("Faith: {}", parts.join(", "));
    if let Some(m) = religion.majority {
        line += &format!("; mostly {}", name(m));
    }
    if let Some(h) = religion.holy_city_of {
        line += &format!("; holy city of {}", name(h));
    }
    Some(line)
}

/// Why a great person's ability cannot be used here, in a few words.
fn refusal(why: civ_sim::queries::GreatPersonRefusal) -> &'static str {
    use civ_sim::queries::GreatPersonRefusal as R;
    match why {
        R::NoMovement => "no moves left",
        R::NothingResearched => "nothing being researched",
        R::CannotBeHurried => "cannot be hurried",
        R::NotInOwnCity => "stand in your own city",
        R::NotABuilding => "the city is not building a building",
        R::NothingToAdd => "it would finish anyway",
        R::NotACityState => "stand in a city-state's land",
        R::AtWar => "at war with that city-state",
        R::CannotBuildHere => "not on this tile",
        R::NotAble => "no longer able",
    }
}

/// A unit's experience and the promotions it holds.
fn experience(u: &civ_sim::UnitView, rules: &Rules) -> String {
    let Some(own) = u.own.as_ref() else { return String::new() };
    let names: Vec<&str> = own
        .promotions
        .iter()
        .filter_map(|p| rules.tables.promotions.iter().find(|r| r.id == *p).map(|r| r.name.as_str()))
        .collect();
    match names.is_empty() {
        true => format!("Experience {}", own.experience),
        false => format!("Experience {} · {}", own.experience, names.join(", ")),
    }
}

pub(crate) fn turns(cost: i32, done: i32, per_turn: i32) -> String {
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
    link: Option<Res<crate::link::Link>>,
) {
    let Some(v) = view.view() else { return };
    let room = link.is_some_and(|l| l.is_remote());
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
            let e = &v.economy;
            r.spawn(text(&font, format!("Gold {treasury} ({})", signed(e.gold)), 13.0, TEXT));
            r.spawn(text(&font, format!("Science {}", signed(e.science)), 13.0, TEXT));
            r.spawn(text(&font, format!("Culture {}", signed(e.culture)), 13.0, TEXT));
            r.spawn(text(&font, format!("Faith {} ({})", v.knowledge.faith, signed(e.faith)), 13.0, TEXT));
            let mood = if e.happiness < 0 { URGENT } else { TEXT };
            r.spawn(text(&font, format!("Happiness {}", e.happiness), 13.0, mood));
            let undo = view.update().and_then(|u| u.undo.as_ref()).is_none_or(|u| u.available);
            r.spawn(button(&font, Order::Undo, "Undo", undo));
            // A room's match is left from its own panel (`roomui.rs`); a solo one is lost on leaving.
            if !room {
                r.spawn(button(&font, Order::Home, if selection.home_armed { "Leave? Click again" } else { "Home" }, true));
            }
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
                text(&font, experience(u, &rules), 12.0, DIM),
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
                    UnitActionKind::Gift => "Gift to the city-state",
                    UnitActionKind::AssembleSpaceshipPart => "Add to the spaceship",
                    UnitActionKind::Rebase | UnitActionKind::AirSweep | UnitActionKind::Paradrop | UnitActionKind::NuclearStrike => {
                        let label = match choice.action {
                            UnitActionKind::Rebase => "Rebase…",
                            UnitActionKind::AirSweep => "Air sweep…",
                            UnitActionKind::Paradrop => "Paradrop…",
                            _ => "Nuclear strike…",
                        };
                        let aiming = selection.mission == Some(choice.action);
                        let label = if aiming { "Click a marked tile" } else { label };
                        p.spawn(button(&font, Order::Mission(choice.action), label, true));
                        continue;
                    }
                    _ => continue,
                };
                let label = if choice.availability == Availability::NeedsConfirmation { format!("{label}…") } else { label.to_string() };
                p.spawn(button(&font, Order::Act(choice.action), label, true));
            }
            for improvement in &options.improvements {
                p.spawn(button(&font, Order::Build(*improvement), format!("Build {}", rules.improvement(*improvement)), true));
            }
            for ability in options.great_person_abilities.iter().flatten() {
                let what = match &ability.action {
                    GreatPersonAction::HurryResearch => "Hurry research".to_string(),
                    GreatPersonAction::HurryProduction => "Hurry production".to_string(),
                    GreatPersonAction::TradeMission => "Trade mission".to_string(),
                    GreatPersonAction::GoldenAge => "Start a golden age".to_string(),
                    GreatPersonAction::Construct { improvement } => format!("Build {}", rules.improvement(*improvement)),
                };
                let unit = match &ability.action {
                    GreatPersonAction::HurryResearch => " science",
                    GreatPersonAction::HurryProduction => " production",
                    GreatPersonAction::TradeMission => " gold",
                    _ => "",
                };
                let label = match (ability.amount, ability.why) {
                    (_, Some(why)) if !ability.available => format!("{what} ({})", refusal(why)),
                    (Some(n), _) => format!("{what} (+{n}{unit})"),
                    _ => what,
                };
                p.spawn(button(&font, Order::GreatPerson(ability.action.clone()), label, ability.available));
            }
            for action in &options.religious_actions {
                let label = match action {
                    ReligiousAction::SpreadReligion { city } => {
                        format!("Spread faith to {}", v.cities.iter().find(|c| c.id == city.id && c.r#gen == city.r#gen).map_or("a city", |c| c.name.as_str()))
                    }
                    ReligiousAction::RemoveHeresy { city } => {
                        format!("Remove heresy in {}", v.cities.iter().find(|c| c.id == city.id && c.r#gen == city.r#gen).map_or("a city", |c| c.name.as_str()))
                    }
                    ReligiousAction::ConstructHolySite => "Build a holy site".into(),
                    ReligiousAction::FoundReligion => "Found a religion".into(),
                    ReligiousAction::EnhanceReligion => "Enhance the religion".into(),
                };
                p.spawn(button(&font, Order::Religious(action.clone()), label, true));
            }
            if let Some(n) = &selection.nuclear {
                p.spawn(text(
                    &font,
                    format!("Blast of {} tiles, radius {}; {}% chance to be shot down. Click again to strike.", n.tiles.len(), n.blast_radius, n.intercept_chance),
                    12.0,
                    URGENT,
                ));
            }
            for promotion in &options.promotions {
                p.spawn(button(&font, Order::Promote(promotion.id), format!("Promote: {}", promotion.name), true));
            }
            if let Some(up) = &options.upgrade {
                let to = rules.tables.units.iter().find(|r| r.id == up.unit_type).map_or("?", |r| r.name.as_str());
                p.spawn(button(&font, Order::Upgrade, format!("Upgrade to {to} ({} gold)", up.gold_cost), true));
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
    decisions: Res<crate::decisions::Decisions>,
    rules: Option<Res<Rules>>,
) {
    let Ok((e, children)) = slot.single() else { return };
    if !stale.0 && !outbox.is_changed() && !selection.is_changed() && !view.is_changed() && !decisions.is_changed() {
        return;
    }
    clear(&mut commands, children);
    let ready = view.update().is_some_and(|u| u.ready);
    let label = if ready {
        // Rooms: the round closes when every person has finished, or at the deadline.
        let others = view.update().and_then(|u| u.seats.as_ref().map(|s| (u.view.me, s))).map(|(me, seats)| {
            seats.iter().filter(|s| s.seat != me && s.controlled && !s.ready && s.left != Some(true)).count()
        });
        match others {
            Some(1) => "Waiting for 1 player".to_string(),
            Some(n) if n > 1 => format!("Waiting for {n} players"),
            _ => "Waiting for the round".to_string(),
        }
    } else if let (Some(d), Some(v)) = (decisions.next(), view.view()) {
        // Decisions lead (docs/ACTIONS-SPEC.md, A1), with a way to put each off.
        commands.entity(e).with_child((
            Node { position_type: PositionType::Absolute, bottom: Val::Px(50.0), right: Val::Px(0.0), ..default() },
            children![button(&font, Order::Later, "Later", true)],
        ));
        d.label(v, rules.as_deref())
    } else if awaiting(&view, &selection, None).is_some() {
        "Next unit".into()
    } else if selection.end_armed {
        "Click again to end turn".into()
    } else {
        "End turn".into()
    };
    commands.entity(e).with_child(styled(
        &font,
        Order::Turn,
        label,
        !ready,
        Node {
            width: Val::Percent(100.0),
            height: Val::Percent(100.0),
            justify_content: JustifyContent::Center,
            align_items: AlignItems::Center,
            border_radius: BorderRadius::all(Val::Px(6.0)),
            ..default()
        },
    ));
}

#[allow(clippy::too_many_arguments)]
fn city_panel(
    mut commands: Commands,
    stale: Res<Stale>,
    view: Res<ViewState>,
    rules: Option<Res<Rules>>,
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
    commands.entity(e).with_children(|p| {
        p.spawn((
            Node { justify_content: JustifyContent::SpaceBetween, ..default() },
            children![text(&font, format!("{}  ·  {}", c.name, c.population), 16.0, TEXT), button(&font, Order::CloseCity, "Close", true)],
        ));
        p.spawn(text(&font, format!("Food +{food} · Production +{production}"), 12.0, DIM));
        if let Some(own) = own {
            let mut status = Vec::new();
            if own.puppet {
                status.push("a puppet".to_string());
            }
            if own.razing {
                status.push(format!("being razed, a citizen a turn"));
            }
            if own.resistance > 0 {
                status.push(format!("in resistance for {} turns", own.resistance));
            }
            if !status.is_empty() {
                p.spawn(text(&font, format!("This city is {}", status.join(", ")), 12.0, URGENT));
            }
            if let Some(line) = rules.as_deref().and_then(|r| faith_line(own, r)) {
                p.spawn(text(&font, line, 12.0, DIM));
            }
            let key = civ_sim::protocol::EntityRef { id: c.id, r#gen: c.r#gen };
            let pending = v.pending_captures.iter().flatten().any(|pc| pc.city == key);
            let mut fates = Vec::new();
            if own.puppet && !pending {
                fates.push(Fate::Annex);
            }
            if own.razing {
                fates.push(Fate::StopRazing);
            } else if own.may_raze && !pending {
                fates.push(Fate::Raze);
            }
            if own.may_liberate && !pending {
                fates.push(Fate::Liberate);
            }
            if !fates.is_empty() {
                p.spawn(Node { flex_wrap: FlexWrap::Wrap, column_gap: Val::Px(4.0), row_gap: Val::Px(4.0), ..default() })
                    .with_children(|row| {
                        for fate in fates {
                            row.spawn(button(&font, Order::Fate(fate), fate.label(), true));
                        }
                    });
            }
            // The queue, front first: the first item is being built.
            let names = c.queue.clone().unwrap_or_default();
            if names.is_empty() {
                p.spawn(text(&font, "Building nothing", 13.0, TEXT));
            }
            for (i, name) in names.iter().enumerate() {
                p.spawn(Node { column_gap: Val::Px(4.0), align_items: AlignItems::Center, ..default() }).with_children(|row| {
                    let label = if i == 0 { format!("Building {name}") } else { format!("{}. {name}", i + 1) };
                    row.spawn((text(&font, label, 13.0, TEXT), Node { flex_grow: 1.0, ..default() }));
                    if i > 0 {
                        row.spawn(button(&font, Order::Raise(i), "↑", true));
                    }
                    row.spawn(button(&font, Order::Unqueue(i), "×", true));
                });
            }
            if let Some(options) = &selection.city_options {
                let current = own.production_conversion;
                for stat in &options.production_conversions {
                    let label = match stat {
                        ProductionConversion::Gold => "Turn production into gold",
                        ProductionConversion::Science => "Turn production into science",
                    };
                    if current != Some(*stat) {
                        p.spawn(button(&font, Order::Convert(Some(*stat)), label, true));
                    }
                }
                if current.is_some() {
                    p.spawn(button(&font, Order::Convert(None), "Stop converting production", true));
                }
                let front = c.queue_refs.as_ref().and_then(|q| q.first());
                let sites = front.and_then(|f| options.choices.iter().find(|ch| &ch.construction == f)).and_then(|ch| ch.sites.as_ref());
                if let Some(sites) = sites.filter(|s| s.tiles.len() > 1) {
                    let label = if selection.siting { "Click one of the marked tiles".to_string() } else { "Choose where it stands".to_string() };
                    let _ = sites;
                    p.spawn(button(&font, Order::Site, label, true));
                }
                if !options.bombard.is_empty() {
                    p.spawn(text(&font, format!("Can bombard {} enemy units: click one on the map", options.bombard.len()), 12.0, URGENT));
                }
            }
        }
        if let Some(own) = own {
            p.spawn(text(&font, "Focus", 12.0, DIM));
            p.spawn(Node { flex_wrap: FlexWrap::Wrap, column_gap: Val::Px(4.0), row_gap: Val::Px(4.0), ..default() }).with_children(|row| {
                for (focus, label) in [
                    (CityFocus::Balanced, "Balanced"),
                    (CityFocus::Food, "Food"),
                    (CityFocus::Production, "Production"),
                    (CityFocus::Gold, "Gold"),
                    (CityFocus::Science, "Science"),
                    (CityFocus::Culture, "Culture"),
                    (CityFocus::Faith, "Faith"),
                    (CityFocus::GoldGrowth, "Gold and growth"),
                    (CityFocus::ProductionGrowth, "Production and growth"),
                ] {
                    let label = if own.focus == focus { format!("▸ {label}") } else { label.to_string() };
                    row.spawn(button(&font, Order::Focus(focus), label, true));
                }
            });
            let avoid = if own.avoid_growth { "Avoiding growth (click to grow)" } else { "Growing (click to avoid growth)" };
            p.spawn(button(&font, Order::AvoidGrowth(!own.avoid_growth), avoid, true));
            if let (Some(options), Some(rules)) = (&selection.city_options, rules.as_ref()) {
                if !options.specialist_slots.is_empty() {
                    p.spawn(text(&font, "Specialists", 12.0, DIM));
                }
                for slot in &options.specialist_slots {
                    let have = own.specialists.iter().find(|s| s.specialist == slot.specialist).map_or(0, |s| s.count);
                    let name = rules.tables.specialists.iter().find(|r| r.id == slot.specialist).map_or("Specialist", |r| r.name.as_str());
                    p.spawn((
                        Node { column_gap: Val::Px(6.0), align_items: AlignItems::Center, ..default() },
                        children![
                            text(&font, format!("{name} {have} / {}", slot.slots), 12.0, TEXT),
                            button(&font, Order::Specialist(slot.specialist, false), "−", have > 0),
                            button(&font, Order::Specialist(slot.specialist, true), "+", have < slot.slots),
                        ],
                    ));
                }
                for sale in &options.sellable_buildings {
                    let name = rules.tables.buildings.iter().find(|b| b.id == sale.building).map_or("Building", |b| b.name.as_str());
                    p.spawn(button(&font, Order::Sell(sale.building), format!("Sell {name} (+{} gold)", sale.gold), true));
                }
            }
        }
        p.spawn(text(&font, "Choose production", 12.0, DIM));
        if let Some(options) = &selection.city_options {
            for choice in &options.choices {
                let turns = choice.production_cost.map_or("–".into(), |cost| turns(cost, c.production_stored.unwrap_or(0), production));
                p.spawn(Node { column_gap: Val::Px(6.0), align_items: AlignItems::Center, ..default() })
                .with_children(|row| {
                    row.spawn(styled(
                        &font,
                        Order::Produce(choice.construction.clone()),
                        format!("{} · {turns} turns", choice.name),
                        true,
                        Node {
                            flex_grow: 1.0,
                            min_height: Val::Px(28.0),
                            align_items: AlignItems::Center,
                            justify_content: JustifyContent::FlexStart,
                            padding: UiRect::axes(Val::Px(10.0), Val::Px(4.0)),
                            border_radius: BorderRadius::all(Val::Px(4.0)),
                            ..default()
                        },
                    ));
                    row.spawn(button(&font, Order::Queue(choice.construction.clone()), "+", choice.refusal.is_none()));
                    if let Some(price) = choice.gold.as_ref().and_then(|g| g.amount.clone().map(|a| (a, g.affordable.unwrap_or(false)))) {
                        row.spawn(button(&font, Order::Buy(choice.construction.clone()), format!("{} gold", price.0), price.1));
                    }
                    if let Some(price) = choice.faith.as_ref().and_then(|f| f.amount.clone().map(|a| (a, f.affordable.unwrap_or(false)))) {
                        row.spawn(button(&font, Order::BuyWithFaith(choice.construction.clone()), format!("{} faith", price.0), price.1));
                    }
                });
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
            GlobalZIndex(30),
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
    link: Option<Res<crate::link::Link>>,
    mut decisions: ResMut<crate::decisions::Decisions>,
    mut screens: ResMut<crate::screens::Screens>,
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
        selection.end_armed = false;
    }
    if keys.just_pressed(KeyCode::Enter) {
        match decisions.next().cloned() {
            Some(d) if !view.update().is_some_and(|u| u.ready) => crate::decisions::open(d, &mut decisions, &mut selection, &mut screens),
            _ => {
                let room = link.is_some_and(|l| l.is_remote());
                turn(&mut selection, &mut outbox, &view, &mut cameras, land.as_deref(), room);
            }
        }
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
fn turn(
    selection: &mut Selection,
    outbox: &mut Outbox,
    view: &ViewState,
    cameras: &mut Query<&mut MapCamera>,
    land: Option<&crate::map::Land>,
    room: bool,
) {
    if view.update().is_some_and(|u| u.ready) {
        return;
    }
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
        None if room && !selection.end_armed => selection.end_armed = true,
        None => {
            outbox.submit(Action::FinishTurn);
            selection.unit = None;
            selection.end_armed = false;
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
    link: Option<Res<crate::link::Link>>,
    mut next: ResMut<NextState<crate::app::AppState>>,
    mut decisions: ResMut<crate::decisions::Decisions>,
    mut screens: ResMut<crate::screens::Screens>,
) {
    let Ok(order) = orders.get(event.entity) else { return };
    match order.clone() {
        Order::Turn => match decisions.next().cloned() {
            Some(d) if !view.update().is_some_and(|u| u.ready) => crate::decisions::open(d, &mut decisions, &mut selection, &mut screens),
            _ => {
                let room = link.is_some_and(|l| l.is_remote());
                turn(&mut selection, &mut outbox, &view, &mut cameras, land.as_deref(), room);
            }
        },
        Order::Later => {
            if let Some(d) = decisions.next().cloned() {
                decisions.put_off(d);
            }
        }
        Order::Undo => {
            outbox.submit(Action::Undo);
        }
        Order::GreatPerson(action) => {
            if let Some(unit) = selection.unit {
                outbox.submit(Action::GreatPerson { unit: entity(unit), action });
            }
        }
        Order::Religious(action) => {
            if let Some(unit) = selection.unit {
                outbox.submit(Action::Religious { unit: entity(unit), action });
            }
        }
        Order::Mission(kind) => {
            selection.mission = if selection.mission == Some(kind) { None } else { Some(kind) };
            selection.armed = None;
            selection.nuclear = None;
            stale.0 = true;
        }
        Order::BuyWithFaith(what) => {
            if let Some(city) = selection.city {
                outbox.submit(Action::Purchase { city: entity(city), what, currency: Currency::Faith });
            }
        }
        Order::Promote(promotion) => {
            if let Some(unit) = selection.unit {
                outbox.submit(Action::Promote { unit: entity(unit), promotion });
            }
        }
        Order::Upgrade => {
            if let Some(unit) = selection.unit {
                outbox.submit(Action::Upgrade { unit: entity(unit) });
            }
        }
        Order::Queue(_) | Order::Unqueue(_) | Order::Raise(_) | Order::Convert(_) | Order::Fate(_) | Order::Site => {
            let Some(key) = selection.city else { return };
            let city = entity(key);
            let refs = view
                .view()
                .and_then(|v| v.cities.iter().find(|c| (c.id, c.r#gen) == key))
                .and_then(|c| c.queue_refs.clone())
                .unwrap_or_default();
            match order.clone() {
                Order::Queue(what) => {
                    let mut queue = refs;
                    queue.push(what);
                    outbox.submit(Action::SetProduction { city, queue });
                }
                Order::Unqueue(i) if i < refs.len() => {
                    let mut queue = refs;
                    queue.remove(i);
                    outbox.submit(Action::SetProduction { city, queue });
                }
                Order::Raise(i) if i > 0 && i < refs.len() => {
                    let mut queue = refs;
                    queue.swap(i - 1, i);
                    outbox.submit(Action::SetProduction { city, queue });
                }
                Order::Convert(stat) => {
                    outbox.submit(Action::SetProductionConversion { city, stat });
                }
                Order::Fate(fate) => {
                    outbox.submit(fate.action(city));
                }
                Order::Site => {
                    selection.siting = !selection.siting;
                    stale.0 = true;
                }
                _ => {}
            }
        }
        Order::Home if selection.home_armed => crate::app::leave(&mut next, crate::app::AppState::Home),
        Order::Home => {
            selection.home_armed = true;
            stale.0 = true;
        }
        Order::Research => {
            selection.research_open = !selection.research_open;
            stale.0 = true;
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
        Order::Focus(focus) => {
            if let Some(city) = selection.city {
                outbox.submit(Action::SetCityFocus { city: entity(city), focus });
            }
        }
        Order::AvoidGrowth(avoid) => {
            if let Some(city) = selection.city {
                outbox.submit(Action::SetAvoidGrowth { city: entity(city), avoid });
            }
        }
        Order::Specialist(kind, add) => {
            let found = selection.city.and_then(|k| view.view().and_then(|v| v.cities.iter().find(|c| (c.id, c.r#gen) == k)).cloned());
            if let (Some(city), Some(options)) = (found, selection.city_options.clone()) {
                if let Some(own) = &city.own {
                    let change = if add { crate::play::Change::AddSpecialist(kind) } else { crate::play::Change::RemoveSpecialist(kind) };
                    if let Some((worked, specialists)) = crate::play::reassign(&city, own, &options, change) {
                        outbox.submit(Action::AssignCitizens { city: entity((city.id, city.r#gen)), worked, specialists });
                    }
                }
            }
        }
        Order::Sell(building) => {
            if let Some(city) = selection.city {
                outbox.submit(Action::SellBuilding { city: entity(city), building });
            }
        }
    }
    stale.0 = true;
}

/// Buttons light up under the pointer.
fn shade(mut buttons: Query<(&Hovered, &mut BackgroundColor), With<Order>>) {
    for (hovered, mut colour) in &mut buttons {
        colour.0 = if hovered.get() { BUTTON_ON } else { BUTTON };
    }
}
