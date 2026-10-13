//! M4's screens (docs/PLAY-SPEC.md, M4): the full-screen tech tree, the policy branches, the Civilizations panel
//! and its trade window, and the offers other civs make you.
//!
//! Each screen is rebuilt from `ViewState`, the rule tables and the civ's options when any of them changes, or
//! when a screen opens or closes; every button sends its order through the Outbox.

use crate::client::{Outbox, ViewChanged};
use crate::inspect::UiFont;
use crate::play::{playing, Selection};
use crate::playui::{clear, text, turns, BUTTON, BUTTON_ON, DIM, PANEL, TEXT};
use crate::rules::{ResourceKind, Rules};
use crate::view_state::ViewState;
use bevy::picking::hover::Hovered;
use bevy::prelude::*;
use bevy::ui_widgets::{observe, Activate, Button};
use civ_sim::protocol::{Action, AgreementTerms, TradeItem};
use std::collections::{HashMap, HashSet};

const COLUMN: f32 = 190.0;
const ROW: f32 = 46.0;
const CARD: Vec2 = Vec2::new(170.0, 38.0);

/// Which screens are open, the trade being drafted, the order awaiting a second click, and the research path.
#[derive(Resource, Default)]
pub struct Screens {
    pub policies: bool,
    pub civs: bool,
    pub trade: Option<Trade>,
    armed: Option<Press>,
    /// Techs still to research toward the one clicked, in order.
    path: Vec<u32>,
    /// The tech last sent to research, until the civ's options show it: options answered before the command
    /// applied still name the old research, and would send it again.
    sent: Option<u32>,
    /// The civ whose demands are listed in the civilizations panel.
    demands: Option<u8>,
    stale: bool,
}

#[derive(Clone, Default)]
pub struct Trade {
    pub with: u8,
    pub give: Vec<TradeItem>,
    pub get: Vec<TradeItem>,
    /// Sent as a peace treaty (`offerPeaceTreaty`) rather than a trade.
    pub treaty: bool,
}

/// What a screen button does.
#[derive(Component, Clone, PartialEq, Debug)]
enum Press {
    OpenPolicies,
    OpenCivs,
    /// Show or hide the notification log (`log.rs`).
    Log,
    Close,
    Tech(u32),
    Policy(u32),
    War(u8),
    Peace(u8),
    Friendship(u8),
    Research(u8),
    Pact(u8),
    Denounce(u8),
    /// Gold to a civ or city-state, by amount.
    Gift(u8, i32),
    Pledge(u8, bool),
    Trade(u8),
    /// A peace treaty with terms, drafted in the trade window.
    Treaty(u8),
    /// Show or hide the demands that can be made of a civ.
    Demands(u8),
    Demand(u8, civ_sim::protocol::Demand),
    /// A city-state's tribute: gold, or a worker.
    Tribute(u8, bool),
    Marriage(u8),
    Give(Item),
    Get(Item),
    Drop(bool, usize),
    Send,
    Answer(String, bool),
    Withdraw(String),
}

/// A trade item before amounts are merged.
#[derive(Clone, PartialEq, Debug)]
enum Item {
    Gold,
    GoldPerTurn,
    Luxury(u32),
    Strategic(u32),
    OpenBorders,
    Embassy,
    /// A city, by id and generation.
    City(u32, u32),
}

#[derive(Component)]
struct TreeScreen;
#[derive(Component)]
struct PolicyScreen;
#[derive(Component)]
struct CivPanel;
#[derive(Component)]
struct TradeWindow;
#[derive(Component)]
struct OfferCards;
#[derive(Component)]
struct ScreenButtons;

pub fn plugin(app: &mut App) {
    app.init_resource::<Screens>()
        .add_systems(OnEnter(crate::app::AppState::Match), frames.run_if(playing))
        .add_systems(OnExit(crate::app::AppState::Match), crate::app::reset::<Screens>)
        .add_systems(
            Update,
            (mark, follow_path, tree, policies, civs, trade, offers, buttons, shade)
                .chain()
                .run_if(playing)
                .after(crate::map::MapSet::Rebuild)
                .in_set(crate::app::InMatch),
        );
}

fn button(font: &UiFont, press: Press, label: impl Into<String>, colour: Color) -> impl Bundle {
    (
        press,
        Button,
        Hovered::default(),
        observe(on_press),
        Node {
            min_height: Val::Px(26.0),
            padding: UiRect::axes(Val::Px(9.0), Val::Px(3.0)),
            align_items: AlignItems::Center,
            justify_content: JustifyContent::Center,
            border_radius: BorderRadius::all(Val::Px(4.0)),
            ..default()
        },
        BackgroundColor(BUTTON),
        children![text(font, label, 12.0, colour)],
    )
}

/// Screens and panels draw above the map's badges, banners and labels.
pub(crate) const SCREEN_Z: GlobalZIndex = GlobalZIndex(20);
pub(crate) const PANEL_Z: GlobalZIndex = GlobalZIndex(10);

fn hidden(node: Node) -> impl Bundle {
    (node, BackgroundColor(PANEL), Visibility::Hidden, PANEL_Z)
}

fn hidden_solid(node: Node) -> impl Bundle {
    (node, BackgroundColor(SOLID), Visibility::Hidden, SCREEN_Z)
}

/// Full-screen views hide the map behind them.
const SOLID: Color = Color::srgba(0x14 as f32 / 255.0, 0x16 as f32 / 255.0, 0x1B as f32 / 255.0, 0.97);
pub(crate) const SOLID_PANEL: Color = SOLID;

fn frames(mut commands: Commands) {
    // The top bar's screen buttons sit beside the research chip.
    commands.spawn((
        ScreenButtons,
        GlobalZIndex(21),
        Node {
            position_type: PositionType::Absolute,
            top: Val::Px(6.0),
            left: Val::Px(280.0),
            column_gap: Val::Px(6.0),
            ..default()
        },
    ));
    commands.spawn((
        TreeScreen,
        hidden_solid(Node {
            position_type: PositionType::Absolute,
            top: Val::Px(40.0),
            left: Val::Px(0.0),
            right: Val::Px(0.0),
            bottom: Val::Px(0.0),
            overflow: Overflow::scroll(),
            ..default()
        }),
        ScrollPosition::default(),
    ));
    commands.spawn((
        PolicyScreen,
        hidden_solid(Node {
            position_type: PositionType::Absolute,
            top: Val::Px(40.0),
            left: Val::Px(0.0),
            right: Val::Px(0.0),
            bottom: Val::Px(0.0),
            padding: UiRect::all(Val::Px(16.0)),
            flex_direction: FlexDirection::Column,
            row_gap: Val::Px(10.0),
            overflow: Overflow::scroll(),
            ..default()
        }),
    ));
    commands.spawn((
        CivPanel,
        hidden(Node {
            position_type: PositionType::Absolute,
            top: Val::Px(52.0),
            left: Val::Px(12.0),
            width: Val::Px(300.0),
            max_height: Val::Percent(80.0),
            padding: UiRect::all(Val::Px(12.0)),
            flex_direction: FlexDirection::Column,
            row_gap: Val::Px(8.0),
            overflow: Overflow::scroll_y(),
            ..default()
        }),
    ));
    commands.spawn((
        TradeWindow,
        hidden(Node {
            position_type: PositionType::Absolute,
            top: Val::Px(90.0),
            left: Val::Percent(50.0),
            width: Val::Px(640.0),
            padding: UiRect::all(Val::Px(14.0)),
            flex_direction: FlexDirection::Column,
            row_gap: Val::Px(10.0),
            ..default()
        }),
        UiTransform { translation: Val2::percent(-50.0, 0.0), ..default() },
    ));
    commands.spawn((
        OfferCards,
        PANEL_Z,
        Node {
            position_type: PositionType::Absolute,
            top: Val::Px(52.0),
            right: Val::Px(344.0),
            width: Val::Px(280.0),
            flex_direction: FlexDirection::Column,
            row_gap: Val::Px(8.0),
            ..default()
        },
    ));
}

fn mark(mut changes: MessageReader<ViewChanged>, selection: Res<Selection>, mut screens: ResMut<Screens>) {
    if changes.read().count() > 0 || selection.is_changed() {
        screens.stale = true;
    }
}

fn nation(rules: &Rules, v: &civ_sim::PlayerView, civ: u8) -> (String, Color, Color) {
    v.civs
        .iter()
        .find(|c| c.id == civ)
        .and_then(|c| rules.nation(c.nation))
        .map(|(n, o, i)| (n.to_string(), o, i))
        .unwrap_or(("Unknown".into(), Color::srgb(0.4, 0.4, 0.4), Color::WHITE))
}

/// After a tech on the path completes, research the next one that can be researched.
fn follow_path(selection: Res<Selection>, view: Res<ViewState>, mut screens: ResMut<Screens>, mut outbox: ResMut<Outbox>) {
    let (Some(v), Some(options)) = (view.view(), &selection.civ_options) else { return };
    if !selection.is_changed() {
        return;
    }
    screens.path.retain(|t| !v.knowledge.technologies.contains(t));
    let researching = options.research.iter().find(|r| r.selected).map(|r| r.id);
    if researching.is_some() && researching == screens.sent {
        screens.sent = None;
    }
    if let Some(next) = screens.path.iter().find(|t| options.research.iter().any(|r| r.id == **t)).copied() {
        if researching != Some(next) && screens.sent != Some(next) && researching.is_none_or(|r| !screens.path.contains(&r)) {
            screens.sent = Some(next);
            outbox.submit(Action::SetResearch { technology: next });
        }
    }
}

/// The techs missing on the way to `goal`, prerequisites first.
fn path_to(rules: &Rules, known: &HashSet<u32>, goal: u32) -> Vec<u32> {
    let mut out = Vec::new();
    fn visit(rules: &Rules, known: &HashSet<u32>, t: u32, out: &mut Vec<u32>) {
        if known.contains(&t) || out.contains(&t) {
            return;
        }
        if let Some(row) = rules.tables.technologies.iter().find(|r| r.id == t) {
            for p in &row.prerequisites {
                visit(rules, known, *p, out);
            }
        }
        out.push(t);
    }
    visit(rules, known, goal, &mut out);
    out
}

#[allow(clippy::too_many_arguments)]
fn tree(
    mut commands: Commands,
    selection: Res<Selection>,
    view: Res<ViewState>,
    rules: Option<Res<Rules>>,
    font: Res<UiFont>,
    screens: Res<Screens>,
    mut screen: Query<(Entity, &mut Visibility, Option<&Children>), With<TreeScreen>>,
    mut shown: Local<bool>,
) {
    let (Some(v), Some(rules), Ok((e, mut visibility, children))) = (view.view(), rules, screen.single_mut()) else { return };
    let open = selection.research_open;
    if !(screens.stale || open != *shown) {
        return;
    }
    *shown = open;
    clear(&mut commands, children);
    if !open {
        *visibility = Visibility::Hidden;
        return;
    }
    *visibility = Visibility::Inherited;
    let known: HashSet<u32> = v.knowledge.technologies.iter().copied().collect();
    let available: HashMap<u32, (i32, i32, bool)> = selection
        .civ_options
        .as_ref()
        .map(|o| o.research.iter().map(|r| (r.id, (r.science_cost, r.invested, r.selected))).collect())
        .unwrap_or_default();
    let science = v.economy.science;
    let techs = &rules.tables.technologies;
    let at = |col: i32, row: i32| Vec2::new(24.0 + col as f32 * COLUMN, 16.0 + row as f32 * ROW);
    let width = techs.iter().map(|t| t.column).max().unwrap_or(0) as f32 * COLUMN + COLUMN + 48.0;
    let height = techs.iter().map(|t| t.row).max().unwrap_or(0) as f32 * ROW + ROW + 32.0;
    commands.entity(e).with_children(|s| {
        s.spawn(Node { width: Val::Px(width), height: Val::Px(height), position_type: PositionType::Relative, ..default() })
            .with_children(|canvas| {
                // Prerequisite lines: out of the prerequisite's right edge, across, into the tech's left edge.
                for t in techs {
                    let to = at(t.column, t.row) + Vec2::new(0.0, CARD.y / 2.0);
                    for p in &t.prerequisites {
                        let Some(pre) = techs.iter().find(|r| r.id == *p) else { continue };
                        let from = at(pre.column, pre.row) + Vec2::new(CARD.x, CARD.y / 2.0);
                        let mid = (from.x + to.x) / 2.0;
                        let colour = if known.contains(p) { Color::srgba(1.0, 1.0, 1.0, 0.45) } else { Color::srgba(1.0, 1.0, 1.0, 0.15) };
                        for (x, y, w, h) in [
                            (from.x, from.y - 1.0, mid - from.x, 2.0),
                            (mid - 1.0, from.y.min(to.y) - 1.0, 2.0, (to.y - from.y).abs() + 2.0),
                            (mid, to.y - 1.0, to.x - mid, 2.0),
                        ] {
                            canvas.spawn((
                                Node { position_type: PositionType::Absolute, left: Val::Px(x), top: Val::Px(y), width: Val::Px(w.max(1.0)), height: Val::Px(h.max(1.0)), ..default() },
                                BackgroundColor(colour),
                            ));
                        }
                    }
                }
                for t in techs {
                    let p = at(t.column, t.row);
                    let (fill, colour, label) = if known.contains(&t.id) {
                        (Color::srgba(0.42, 0.62, 0.36, 0.55), TEXT, t.name.clone())
                    } else if let Some((cost, invested, selected)) = available.get(&t.id) {
                        let mark = if *selected { "▸ " } else { "" };
                        (if *selected { BUTTON_ON } else { BUTTON }, TEXT, format!("{mark}{} · {} t", t.name, turns(*cost, *invested, science)))
                    } else {
                        (Color::srgba(1.0, 1.0, 1.0, 0.03), DIM, format!("{} · {}", t.name, t.cost))
                    };
                    let on_path = screens.path.contains(&t.id);
                    canvas.spawn((
                        Press::Tech(t.id),
                        Button,
                        Hovered::default(),
                        observe(on_press),
                        Node {
                            position_type: PositionType::Absolute,
                            left: Val::Px(p.x),
                            top: Val::Px(p.y),
                            width: Val::Px(CARD.x),
                            height: Val::Px(CARD.y),
                            padding: UiRect::horizontal(Val::Px(8.0)),
                            align_items: AlignItems::Center,
                            border: UiRect::all(Val::Px(if on_path { 2.0 } else { 0.0 })),
                            border_radius: BorderRadius::all(Val::Px(5.0)),
                            ..default()
                        },
                        BorderColor::all(Color::srgb(0.95, 0.82, 0.45)),
                        BackgroundColor(fill),
                        children![text(&font, label, 12.0, colour)],
                    ));
                }
            });
    });
}

#[allow(clippy::too_many_arguments)]
fn policies(
    mut commands: Commands,
    selection: Res<Selection>,
    view: Res<ViewState>,
    rules: Option<Res<Rules>>,
    font: Res<UiFont>,
    screens: Res<Screens>,
    mut screen: Query<(Entity, &mut Visibility, Option<&Children>), With<PolicyScreen>>,
    mut shown: Local<bool>,
) {
    let (Some(v), Some(rules), Ok((e, mut visibility, children))) = (view.view(), rules, screen.single_mut()) else { return };
    if !(screens.stale || screens.policies != *shown) {
        return;
    }
    *shown = screens.policies;
    clear(&mut commands, children);
    if !screens.policies {
        *visibility = Visibility::Hidden;
        return;
    }
    *visibility = Visibility::Inherited;
    let adopted: HashSet<u32> = v.knowledge.policies.iter().copied().collect();
    let open: HashMap<u32, bool> = selection
        .civ_options
        .as_ref()
        .map(|o| o.policies.iter().map(|p| (p.id, p.affordable)).collect())
        .unwrap_or_default();
    let cost = selection.civ_options.as_ref().map_or(0, |o| o.next_policy_cost);
    let culture = v.civs.iter().find(|c| c.id == v.me).and_then(|c| c.culture).unwrap_or(0);
    let rows = rules.tables.policy_rows.clone().unwrap_or_default();
    let mut branches: Vec<String> = Vec::new();
    for r in &rows {
        if let Some(b) = &r.branch {
            if !branches.contains(b) {
                branches.push(b.clone());
            }
        }
    }
    commands.entity(e).with_children(|s| {
        s.spawn((
            Node { justify_content: JustifyContent::SpaceBetween, ..default() },
            children![
                text(
                    &font,
                    match v.rewards.policies {
                        0 => format!("Policies · culture {culture} of {cost} (+{} a turn)", v.economy.culture),
                        n => format!("Policies · {n} free: adopt any open policy at no cost"),
                    },
                    16.0,
                    TEXT,
                ),
                button(&font, Press::Close, "Close", TEXT),
            ],
        ));
        s.spawn(Node { column_gap: Val::Px(10.0), align_items: AlignItems::FlexStart, ..default() }).with_children(|cols| {
            for branch in &branches {
                let mut members: Vec<_> = rows.iter().filter(|r| r.branch.as_ref() == Some(branch)).collect();
                members.sort_by_key(|r| (r.name != *branch, r.row.unwrap_or(0), r.column.unwrap_or(0)));
                let era = members.first().and_then(|r| r.era.clone()).unwrap_or_default();
                cols.spawn((
                    Node { flex_direction: FlexDirection::Column, row_gap: Val::Px(4.0), width: Val::Px(150.0), ..default() },
                ))
                .with_children(|col| {
                    col.spawn(text(&font, era, 11.0, DIM));
                    for p in members {
                        let (label, colour) = if adopted.contains(&p.id) {
                            (format!("✓ {}", p.name), Color::srgb(0.7, 0.9, 0.6))
                        } else if let Some(affordable) = open.get(&p.id) {
                            let armed = screens.armed == Some(Press::Policy(p.id));
                            let name = if armed { format!("Adopt {}?", p.name) } else { p.name.clone() };
                            (name, if *affordable { TEXT } else { DIM })
                        } else {
                            (p.name.clone(), Color::srgb(0.42, 0.44, 0.47))
                        };
                        col.spawn(button(&font, Press::Policy(p.id), label, colour));
                    }
                });
            }
        });
    });
}

#[allow(clippy::too_many_arguments)]
fn civs(
    mut commands: Commands,
    view: Res<ViewState>,
    rules: Option<Res<Rules>>,
    font: Res<UiFont>,
    screens: Res<Screens>,
    mut panel: Query<(Entity, &mut Visibility, Option<&Children>), With<CivPanel>>,
    mut shown: Local<bool>,
) {
    let (Some(update), Some(rules), Ok((e, mut visibility, children))) = (view.update(), rules, panel.single_mut()) else { return };
    let v = &update.view;
    if !(screens.stale || screens.civs != *shown) {
        return;
    }
    *shown = screens.civs;
    clear(&mut commands, children);
    if !screens.civs {
        *visibility = Visibility::Hidden;
        return;
    }
    *visibility = Visibility::Inherited;
    let armed = |p: &Press| screens.armed.as_ref() == Some(p);
    let treasury = v.civs.iter().find(|c| c.id == v.me).and_then(|c| c.gold).unwrap_or(0);
    commands.entity(e).with_children(|panel| {
        panel.spawn((
            Node { justify_content: JustifyContent::SpaceBetween, ..default() },
            children![text(&font, "Civilizations", 16.0, TEXT), button(&font, Press::Close, "Close", TEXT)],
        ));
        // Major civs only: city-states have their own list below, and raiders take no diplomacy.
        let major = |civ: u8| {
            v.civs.iter().find(|c| c.id == civ).is_some_and(|c| {
                !c.city_state && !rules.tables.nations.iter().any(|n| n.id == c.nation && n.barbarian == Some(true))
            })
        };
        if !v.relations.iter().any(|r| major(r.civ)) && v.city_states.is_empty() {
            panel.spawn(text(&font, "You have met no one yet.", 12.0, DIM));
        }
        for r in v.relations.iter().filter(|r| major(r.civ)) {
            let (name, outer, _) = nation(&rules, v, r.civ);
            let mut status = vec![if r.at_war { "At war".to_string() } else { "At peace".to_string() }];
            status.push(format!("attitude {}", r.attitude.to_lowercase()));
            status.push(format!("score {}", r.score));
            if r.friendship_turns > 0 {
                status.push(format!("friends {} t", r.friendship_turns));
            }
            if r.defensive_pact_turns > 0 {
                status.push(format!("pact {} t", r.defensive_pact_turns));
            }
            if let Some(t) = r.research_agreement_turns {
                status.push(format!("research agreement {t} t"));
            }
            panel
                .spawn((Node { flex_direction: FlexDirection::Column, row_gap: Val::Px(4.0), padding: UiRect::vertical(Val::Px(4.0)), ..default() },))
                .with_children(|card| {
                    card.spawn((
                        Node { column_gap: Val::Px(6.0), align_items: AlignItems::Center, ..default() },
                        children![
                            (Node { width: Val::Px(10.0), height: Val::Px(10.0), ..default() }, BackgroundColor(outer)),
                            text(&font, name, 14.0, TEXT),
                        ],
                    ));
                    card.spawn(text(&font, status.join(" · "), 11.0, DIM));
                    card.spawn(Node { flex_wrap: FlexWrap::Wrap, column_gap: Val::Px(4.0), row_gap: Val::Px(4.0), ..default() }).with_children(|row| {
                        if r.at_war {
                            row.spawn(button(&font, Press::Peace(r.civ), "Offer peace", TEXT));
                            row.spawn(button(&font, Press::Treaty(r.civ), "Peace treaty…", TEXT));
                        } else {
                            let war = Press::War(r.civ);
                            let label = if armed(&war) { "Declare war?" } else { "Declare war" };
                            row.spawn(button(&font, war, label, Color::srgb(1.0, 0.6, 0.55)));
                            if r.friendship_turns == 0 {
                                row.spawn(button(&font, Press::Friendship(r.civ), "Friendship", TEXT));
                            }
                            if r.research_agreement_turns.is_none() {
                                row.spawn(button(&font, Press::Research(r.civ), "Research agreement", TEXT));
                            }
                            if r.defensive_pact_turns == 0 {
                                row.spawn(button(&font, Press::Pact(r.civ), "Defensive pact", TEXT));
                            }
                            row.spawn(button(&font, Press::Denounce(r.civ), "Denounce", TEXT));
                            row.spawn(button(&font, Press::Trade(r.civ), "Trade…", TEXT));
                            row.spawn(button(&font, Press::Demands(r.civ), "Demand…", TEXT));
                        }
                        if !r.at_war {
                            gifts(row, &font, r.civ, treasury);
                        }
                    });
                    if screens.demands == Some(r.civ) && !r.at_war {
                        card.spawn(Node { flex_wrap: FlexWrap::Wrap, column_gap: Val::Px(4.0), row_gap: Val::Px(4.0), ..default() }).with_children(|row| {
                            use civ_sim::protocol::Demand as D;
                            for (demand, label) in [
                                (D::DoNotSettleNearUs, "Stop settling near us"),
                                (D::DoNotAttackUs, "Do not attack us"),
                                (D::DontSpyOnUs, "Stop spying on us"),
                                (D::DoNotSpreadReligion, "Stop spreading your faith"),
                            ] {
                                row.spawn(button(&font, Press::Demand(r.civ, demand), label, TEXT));
                            }
                        });
                    }
                    for offer in update.offers.iter().filter(|o| o.from == v.me && o.to == r.civ) {
                        card.spawn((
                            Node { column_gap: Val::Px(6.0), align_items: AlignItems::Center, ..default() },
                            children![
                                text(&font, format!("You offered {}", terms(&offer.terms, &rules, v)), 11.0, DIM),
                                button(&font, Press::Withdraw(offer.id.clone()), "Withdraw", TEXT),
                            ],
                        ));
                    }
                });
        }
        if !v.city_states.is_empty() {
            panel.spawn(text(&font, "City-states", 13.0, DIM));
        }
        for cs in &v.city_states {
            let (name, outer, _) = nation(&rules, v, cs.civ);
            panel
                .spawn((Node { flex_direction: FlexDirection::Column, row_gap: Val::Px(4.0), ..default() },))
                .with_children(|card| {
                    card.spawn((
                        Node { column_gap: Val::Px(6.0), align_items: AlignItems::Center, ..default() },
                        children![
                            (Node { width: Val::Px(10.0), height: Val::Px(10.0), ..default() }, BackgroundColor(outer)),
                            text(&font, name, 13.0, TEXT),
                        ],
                    ));
                    card.spawn(text(&font, format!("{} · influence {} ({} a turn)", cs.relationship, cs.influence, cs.influence_per_turn), 11.0, DIM));
                    card.spawn(Node { column_gap: Val::Px(4.0), ..default() }).with_children(|row| {
                        if cs.may_pledge {
                            row.spawn(button(&font, Press::Pledge(cs.civ, true), "Pledge protection", TEXT));
                        }
                        if cs.may_withdraw {
                            row.spawn(button(&font, Press::Pledge(cs.civ, false), "Withdraw protection", TEXT));
                        }
                        if cs.tribute_gold {
                            row.spawn(button(&font, Press::Tribute(cs.civ, false), format!("Demand {} gold", cs.tribute_amount), TEXT));
                        }
                        if cs.tribute_worker {
                            row.spawn(button(&font, Press::Tribute(cs.civ, true), "Demand a worker", TEXT));
                        }
                        if cs.may_request_marriage {
                            row.spawn(button(&font, Press::Marriage(cs.civ), "Request marriage", TEXT));
                        }
                        gifts(row, &font, cs.civ, treasury);
                    });
                    for q in &cs.quests {
                        let until = q.expires_on.map_or(String::new(), |t| format!(", until turn {t}"));
                        card.spawn(text(&font, format!("{}: {} (+{} influence{until})", q.name, q.description, q.influence_reward), 11.0, DIM));
                    }
                });
        }
    });
}

/// Gift gold buttons for the amounts the treasury covers.
fn gifts(row: &mut ChildSpawnerCommands, font: &UiFont, civ: u8, treasury: i32) {
    for gold in [50, 100, 250, 500].into_iter().filter(|g| *g <= treasury) {
        row.spawn(button(font, Press::Gift(civ, gold), format!("Gift {gold} gold"), TEXT));
    }
}

fn item_label(item: &TradeItem, rules: &Rules, v: &civ_sim::PlayerView) -> String {
    let resource = |r: &u32| rules.resource(*r).map_or("?".to_string(), |x| x.0.to_string());
    match item {
        TradeItem::City { city } => v
            .cities
            .iter()
            .find(|c| (c.id, c.r#gen) == (city.id, city.r#gen))
            .map_or("A city".into(), |c| c.name.clone()),
        TradeItem::Gold { amount } => format!("{amount} gold"),
        TradeItem::GoldPerTurn { amount } => format!("{amount} gold a turn"),
        TradeItem::Luxury { resource: r } => resource(r),
        TradeItem::Strategic { resource: r, amount } => format!("{amount} {}", resource(r)),
        TradeItem::OpenBorders => "Open borders".into(),
        TradeItem::Embassy => "Embassy".into(),
    }
}

fn terms(t: &AgreementTerms, rules: &Rules, v: &civ_sim::PlayerView) -> String {
    let list = |items: &[TradeItem]| items.iter().map(|i| item_label(i, rules, v)).collect::<Vec<_>>().join(", ");
    match t {
        AgreementTerms::Peace => "peace".into(),
        AgreementTerms::PeaceTreaty { give, get } => format!("a peace treaty (they give {}; you give {})", list(give), list(get)),
        AgreementTerms::Friendship => "friendship".into(),
        AgreementTerms::ResearchAgreement => "a research agreement".into(),
        AgreementTerms::DefensivePact => "a defensive pact".into(),
        AgreementTerms::Trade { give, get } => format!("a trade (they give {}; you give {})", list(give), list(get)),
    }
}

/// Add an item to one side, merging amounts.
fn add(side: &mut Vec<TradeItem>, item: &Item) {
    for existing in side.iter_mut() {
        match (existing, item) {
            (TradeItem::Gold { amount }, Item::Gold) => {
                *amount += 50;
                return;
            }
            (TradeItem::GoldPerTurn { amount }, Item::GoldPerTurn) => {
                *amount += 1;
                return;
            }
            (TradeItem::Strategic { resource, amount }, Item::Strategic(r)) if *resource == *r => {
                *amount += 1;
                return;
            }
            (TradeItem::Luxury { resource }, Item::Luxury(r)) if *resource == *r => return,
            (TradeItem::OpenBorders, Item::OpenBorders) | (TradeItem::Embassy, Item::Embassy) => return,
            (TradeItem::City { city }, Item::City(id, r#gen)) if (city.id, city.r#gen) == (*id, *r#gen) => return,
            _ => {}
        }
    }
    side.push(match item {
        Item::Gold => TradeItem::Gold { amount: 50 },
        Item::GoldPerTurn => TradeItem::GoldPerTurn { amount: 1 },
        Item::Luxury(r) => TradeItem::Luxury { resource: *r },
        Item::Strategic(r) => TradeItem::Strategic { resource: *r, amount: 1 },
        Item::OpenBorders => TradeItem::OpenBorders,
        Item::Embassy => TradeItem::Embassy,
        Item::City(id, r#gen) => TradeItem::City { city: civ_sim::protocol::EntityRef { id: *id, r#gen: *r#gen } },
    });
}

#[allow(clippy::too_many_arguments)]
fn trade(
    mut commands: Commands,
    view: Res<ViewState>,
    rules: Option<Res<Rules>>,
    font: Res<UiFont>,
    screens: Res<Screens>,
    mut window: Query<(Entity, &mut Visibility, Option<&Children>), With<TradeWindow>>,
    mut shown: Local<bool>,
) {
    let (Some(v), Some(rules), Ok((e, mut visibility, children))) = (view.view(), rules, window.single_mut()) else { return };
    let open = screens.trade.is_some();
    if !(screens.stale || open != *shown || screens.is_changed()) {
        return;
    }
    *shown = open;
    clear(&mut commands, children);
    let Some(draft) = &screens.trade else {
        *visibility = Visibility::Hidden;
        return;
    };
    *visibility = Visibility::Inherited;
    let (them, _, _) = nation(&rules, v, draft.with);
    let mine: Vec<(Item, String)> = {
        let mut out = vec![(Item::Gold, "+50 gold".to_string()), (Item::GoldPerTurn, "+1 gold a turn".into())];
        for stock in v.knowledge.resources.iter().filter(|s| s.amount > 0) {
            match rules.resource(stock.resource) {
                Some((name, ResourceKind::Luxury)) => out.push((Item::Luxury(stock.resource), name.to_string())),
                Some((name, ResourceKind::Strategic)) => out.push((Item::Strategic(stock.resource), format!("+1 {name}"))),
                _ => {}
            }
        }
        out.push((Item::OpenBorders, "Open borders".into()));
        out.push((Item::Embassy, "Embassy".into()));
        // The engine refuses a capital, a city in resistance, and a last city.
        for c in v.cities.iter().filter(|c| c.owner == v.me && c.own.as_ref().is_some_and(|o| !o.capital)) {
            out.push((Item::City(c.id, c.r#gen), c.name.clone()));
        }
        out
    };
    // What they could give shows once you have an embassy with them (docs/ROOMS-SPEC.md, R7); without one,
    // the offer is made blind and the engine refuses what they do not have.
    let relation = v.relations.iter().find(|r| r.civ == draft.with);
    let theirs: Vec<(Item, String)> = match relation.and_then(|r| r.tradable.as_ref()) {
        Some(t) => {
            let mut out = Vec::new();
            if t.gold >= 50 {
                out.push((Item::Gold, format!("+50 gold (has {})", t.gold)));
            }
            if t.gold_per_turn >= 1 {
                out.push((Item::GoldPerTurn, format!("+1 gold a turn (up to {})", t.gold_per_turn)));
            }
            for s in &t.luxuries {
                out.push((Item::Luxury(s.resource), format!("{} ({})", rules.resource(s.resource).map_or("?", |r| r.0), s.amount)));
            }
            for s in &t.strategics {
                out.push((Item::Strategic(s.resource), format!("+1 {} (has {})", rules.resource(s.resource).map_or("?", |r| r.0), s.amount)));
            }
            for c in &t.cities {
                let name = v.cities.iter().find(|x| (x.id, x.r#gen) == (c.id, c.r#gen)).map_or("A city", |x| x.name.as_str());
                out.push((Item::City(c.id, c.r#gen), name.to_string()));
            }
            if t.open_borders {
                out.push((Item::OpenBorders, "Open borders".into()));
            }
            if t.embassy {
                out.push((Item::Embassy, "Embassy".into()));
            }
            out
        }
        None => vec![
            (Item::Gold, "+50 gold".to_string()),
            (Item::GoldPerTurn, "+1 gold a turn".into()),
            (Item::OpenBorders, "Open borders".into()),
            (Item::Embassy, "Embassy".into()),
        ],
    };
    let blind = relation.is_none_or(|r| r.tradable.is_none());
    commands.entity(e).with_children(|w| {
        w.spawn((
            Node { justify_content: JustifyContent::SpaceBetween, ..default() },
            children![
                text(&font, if draft.treaty { format!("Peace treaty with {them}") } else { format!("Trade with {them}") }, 16.0, TEXT),
                button(&font, Press::Close, "Cancel", TEXT)
            ],
        ));
        w.spawn(Node { column_gap: Val::Px(16.0), ..default() }).with_children(|cols| {
            for (side, title, offers, table) in [(false, "You give", &mine, &draft.give), (true, "They give", &theirs, &draft.get)] {
                cols.spawn(Node { flex_direction: FlexDirection::Column, row_gap: Val::Px(4.0), width: Val::Px(296.0), ..default() }).with_children(|col| {
                    col.spawn(text(&font, title, 13.0, DIM));
                    if side && blind {
                        col.spawn(text(&font, "What they have shows once you have an embassy with them.", 11.0, DIM));
                    }
                    for (i, item) in table.iter().enumerate() {
                        col.spawn(button(&font, Press::Drop(side, i), format!("{} ×", item_label(item, &rules, v)), TEXT));
                    }
                    col.spawn(Node { flex_wrap: FlexWrap::Wrap, column_gap: Val::Px(4.0), row_gap: Val::Px(4.0), ..default() }).with_children(|add| {
                        for (item, label) in offers.iter() {
                            let press = if side { Press::Get(item.clone()) } else { Press::Give(item.clone()) };
                            add.spawn(button(&font, press, label.clone(), DIM));
                        }
                    });
                });
            }
        });
        w.spawn(button(&font, Press::Send, "Send offer", TEXT));
    });
}

#[allow(clippy::too_many_arguments)]
fn offers(
    mut commands: Commands,
    view: Res<ViewState>,
    rules: Option<Res<Rules>>,
    font: Res<UiFont>,
    screens: Res<Screens>,
    cards: Query<(Entity, Option<&Children>), With<OfferCards>>,
) {
    let (Some(update), Some(rules), Ok((e, children))) = (view.update(), rules, cards.single()) else { return };
    if !screens.stale {
        return;
    }
    clear(&mut commands, children);
    let v = &update.view;
    commands.entity(e).with_children(|list| {
        for offer in update.offers.iter().filter(|o| o.to == v.me) {
            let (from, outer, _) = nation(&rules, v, offer.from);
            list.spawn((
                Node { flex_direction: FlexDirection::Column, row_gap: Val::Px(6.0), padding: UiRect::all(Val::Px(10.0)), border: UiRect::left(Val::Px(3.0)), ..default() },
                BorderColor::all(outer),
                BackgroundColor(PANEL),
                children![
                    text(&font, format!("{from} offers {}", terms(&offer.terms, &rules, &update.view)), 12.0, TEXT),
                    (
                        Node { column_gap: Val::Px(6.0), ..default() },
                        children![
                            button(&font, Press::Answer(offer.id.clone(), true), "Accept", TEXT),
                            button(&font, Press::Answer(offer.id.clone(), false), "Decline", TEXT),
                        ],
                    ),
                ],
            ));
        }
    });
}

/// The top bar's Policies and Civilizations buttons.
fn buttons(
    mut commands: Commands,
    font: Res<UiFont>,
    slot: Query<(Entity, Option<&Children>), With<ScreenButtons>>,
    mut made: Local<bool>,
    mut screens: ResMut<Screens>,
) {
    let Ok((e, children)) = slot.single() else { return };
    if *made && children.is_some() {
        screens.stale = false;
        return;
    }
    *made = true;
    commands.entity(e).with_children(|b| {
        b.spawn(button(&font, Press::OpenPolicies, "Policies", TEXT));
        b.spawn(button(&font, Press::OpenCivs, "Civilizations", TEXT));
        b.spawn(button(&font, Press::Log, "Log", TEXT));
    });
    screens.stale = false;
}

fn shade(mut buttons: Query<(&Hovered, &mut BackgroundColor, &Press), Without<TreeScreen>>, screens: Res<Screens>) {
    for (hovered, mut colour, press) in &mut buttons {
        if matches!(press, Press::Tech(_)) {
            continue;
        }
        let armed = screens.armed.as_ref() == Some(press);
        colour.0 = if hovered.get() || armed { BUTTON_ON } else { BUTTON };
    }
}

#[allow(clippy::too_many_arguments)]
fn on_press(
    event: On<Activate>,
    presses: Query<&Press>,
    mut screens: ResMut<Screens>,
    mut selection: ResMut<Selection>,
    mut outbox: ResMut<Outbox>,
    view: Res<ViewState>,
    rules: Option<Res<Rules>>,
    mut log: ResMut<crate::log::NoteLog>,
) {
    let Ok(press) = presses.get(event.entity) else { return };
    let press = press.clone();
    let confirmed = screens.armed.as_ref() == Some(&press);
    screens.stale = true;
    // Orders that cannot be taken back wait for a second click.
    let needs_confirmation = matches!(press, Press::War(_) | Press::Policy(_));
    if needs_confirmation && !confirmed {
        screens.armed = Some(press);
        return;
    }
    screens.armed = None;
    match press {
        Press::OpenPolicies => {
            screens.policies = !screens.policies;
            selection.research_open = false;
        }
        Press::OpenCivs => {
            screens.civs = !screens.civs;
            log.open = false;
        }
        // The log and the civilizations panel share the left side.
        Press::Log => {
            log.open = !log.open;
            screens.civs = false;
        }
        Press::Close => {
            if screens.trade.is_some() {
                screens.trade = None;
            } else {
                screens.policies = false;
                screens.civs = false;
                selection.research_open = false;
            }
        }
        Press::Tech(t) => {
            let (Some(v), Some(rules)) = (view.view(), rules) else { return };
            let known: HashSet<u32> = v.knowledge.technologies.iter().copied().collect();
            if known.contains(&t) {
                return;
            }
            // A free technology pending takes any it may have.
            if v.rewards.technologies > 0 && selection.civ_options.as_ref().is_some_and(|o| o.free_technologies.contains(&t)) {
                outbox.submit(Action::ClaimFreeTechnology { technology: t });
                return;
            }
            screens.path = path_to(&rules, &known, t);
            let available = selection.civ_options.as_ref().is_some_and(|o| o.research.iter().any(|r| r.id == t));
            let first = if available {
                Some(t)
            } else {
                screens
                    .path
                    .iter()
                    .find(|p| selection.civ_options.as_ref().is_some_and(|o| o.research.iter().any(|r| r.id == **p)))
                    .copied()
            };
            if let Some(first) = first {
                screens.sent = Some(first);
                outbox.submit(Action::SetResearch { technology: first });
            }
        }
        Press::Policy(policy) => {
            outbox.submit(Action::AdoptPolicy { policy });
        }
        Press::War(target) => {
            outbox.submit(Action::DeclareWar { target });
        }
        Press::Peace(target) => {
            outbox.submit(Action::OfferPeace { target });
        }
        Press::Friendship(target) => {
            outbox.submit(Action::OfferFriendship { target });
        }
        Press::Research(target) => {
            outbox.submit(Action::OfferResearchAgreement { target });
        }
        Press::Pact(target) => {
            outbox.submit(Action::OfferDefensivePact { target });
        }
        Press::Denounce(target) => {
            outbox.submit(Action::Denounce { target });
        }
        Press::Gift(target, gold) => {
            outbox.submit(Action::GiftGold { target, gold });
        }
        Press::Treaty(with) => screens.trade = Some(Trade { with, treaty: true, ..default() }),
        Press::Demands(civ) => {
            screens.demands = if screens.demands == Some(civ) { None } else { Some(civ) };
            screens.stale = true;
        }
        Press::Demand(target, demand) => {
            outbox.submit(Action::Demand { target, demand });
            screens.demands = None;
        }
        Press::Tribute(target, worker) => {
            outbox.submit(Action::DemandTribute { target, worker });
        }
        Press::Marriage(target) => {
            outbox.submit(Action::BuyCityState { target });
        }
        Press::Pledge(target, true) => {
            outbox.submit(Action::PledgeProtection { target });
        }
        Press::Pledge(target, false) => {
            outbox.submit(Action::WithdrawProtection { target });
        }
        Press::Trade(with) => screens.trade = Some(Trade { with, ..default() }),
        Press::Give(item) => {
            if let Some(t) = screens.trade.as_mut() {
                add(&mut t.give, &item);
            }
        }
        Press::Get(item) => {
            if let Some(t) = screens.trade.as_mut() {
                add(&mut t.get, &item);
            }
        }
        Press::Drop(side, i) => {
            if let Some(t) = screens.trade.as_mut() {
                let list = if side { &mut t.get } else { &mut t.give };
                if i < list.len() {
                    list.remove(i);
                }
            }
        }
        Press::Send => {
            if let Some(t) = screens.trade.take() {
                outbox.submit(match t.treaty {
                    true => Action::OfferPeaceTreaty { target: t.with, give: t.give, get: t.get },
                    false => Action::OfferTrade { target: t.with, give: t.give, get: t.get },
                });
            }
        }
        Press::Answer(offer, accept) => {
            outbox.submit(Action::AnswerOffer { offer, accept });
        }
        Press::Withdraw(offer) => {
            outbox.submit(Action::WithdrawOffer { offer });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trade_items_merge_amounts_and_keep_one_of_each_kind() {
        let mut side = Vec::new();
        for item in [Item::Gold, Item::Gold, Item::GoldPerTurn, Item::Strategic(4), Item::Strategic(4), Item::OpenBorders, Item::OpenBorders, Item::Luxury(2)] {
            add(&mut side, &item);
        }
        assert_eq!(
            side,
            vec![
                TradeItem::Gold { amount: 100 },
                TradeItem::GoldPerTurn { amount: 1 },
                TradeItem::Strategic { resource: 4, amount: 2 },
                TradeItem::OpenBorders,
                TradeItem::Luxury { resource: 2 },
            ]
        );
    }
}
