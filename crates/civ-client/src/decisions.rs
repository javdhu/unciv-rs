//! What the engine waits on the player for (docs/ACTIONS-SPEC.md, A1): captured cities, free technologies,
//! great people and policies, promotions, the state faith, demands and the vote.
//!
//! Decisions are derived from each update and the options answers, never stored. The End turn button names the
//! first one not put off this turn and opens it: a screen (technology, policies), the unit (promotions), or a
//! card here. "Later" puts one off until the next turn; the engine's default applies if it is never answered.

use crate::client::{Outbox, QueryAnswered, ViewChanged};
use crate::inspect::UiFont;
use crate::link::{Link, Up};
use crate::pieces::Key;
use crate::play::{entity, Selection};
use crate::playui::{clear, text, Fate, BUTTON, BUTTON_ON, DIM, TEXT, URGENT};
use crate::rules::Rules;
use crate::view_state::ViewState;
use bevy::picking::hover::Hovered;
use bevy::prelude::*;
use bevy::ui_widgets::{observe, Activate, Button};
use civ_sim::protocol::{Action, ClientMessage, Demand, Query as Ask, QueryResult};
use civ_sim::CaptureChoice;
use std::collections::HashSet;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Decision {
    Capture(Key),
    FreeTechnology,
    FreeGreatPerson,
    FreePolicy,
    Promote(Key),
    StateFaith,
    /// A demand by a civ, by its kind (`demand_code`).
    Demand(u8, u8),
    Vote,
}

fn demand_code(d: Demand) -> u8 {
    match d {
        Demand::DontSpyOnUs => 0,
        Demand::DoNotSpreadReligion => 1,
        Demand::DoNotSettleNearUs => 2,
        Demand::DoNotAttackUs => 3,
    }
}

impl Decision {
    /// What the End turn button says while this decision leads.
    pub fn label(&self, v: &civ_sim::PlayerView, rules: Option<&Rules>) -> String {
        let city = |k: &Key| v.cities.iter().find(|c| (c.id, c.r#gen) == *k).map_or("a city".into(), |c| c.name.clone());
        let unit = |k: &Key| {
            v.units
                .iter()
                .find(|u| (u.id, u.r#gen) == *k)
                .and_then(|u| rules.and_then(|r| r.tables.units.iter().find(|row| row.id == u.base)))
                .map_or("a unit".into(), |row| row.name.clone())
        };
        match self {
            Decision::Capture(k) => format!("Decide {}'s fate", city(k)),
            Decision::FreeTechnology => "Choose a free technology".into(),
            Decision::FreeGreatPerson => "Choose a great person".into(),
            Decision::FreePolicy => "Adopt a free policy".into(),
            Decision::Promote(k) => format!("Promote {}", unit(k)),
            Decision::StateFaith => "Choose a state faith".into(),
            Decision::Demand(..) => "Answer a demand".into(),
            Decision::Vote => "Cast your vote".into(),
        }
    }
}

/// The decisions pending, the one open, and those put off this turn.
#[derive(Resource, Default)]
pub struct Decisions {
    pub list: Vec<Decision>,
    pub open: Option<Decision>,
    later: HashSet<Decision>,
    turn: u32,
    /// Own units whose `unitOptions` offer promotions, from the last probe.
    promotable: HashSet<Key>,
    probe: u64,
    /// What the last probe asked about: each veteran with its experience and promotions held.
    probed: Vec<(Key, u16, usize)>,
}

impl Decisions {
    /// The decision End turn leads with.
    pub fn next(&self) -> Option<&Decision> {
        self.list.iter().find(|d| !self.later.contains(d))
    }

    pub fn put_off(&mut self, d: Decision) {
        if self.open.as_ref() == Some(&d) {
            self.open = None;
        }
        self.later.insert(d);
    }
}

#[derive(Component)]
struct Card;

#[derive(Component, Clone, PartialEq, Debug)]
enum Press {
    Fate(Key, Fate),
    GreatPerson(u32),
    Faith(u8),
    Answer(u8, Demand, bool),
    Vote(Option<u8>),
    Later,
    Close,
}

pub fn plugin(app: &mut App) {
    app.init_resource::<Decisions>()
        .add_systems(OnEnter(crate::app::AppState::Match), card.run_if(crate::play::playing))
        .add_systems(OnExit(crate::app::AppState::Match), crate::app::reset::<Decisions>)
        .add_systems(
            Update,
            (probe, answers, derive, draw, shade)
                .chain()
                .run_if(crate::play::playing)
                .after(crate::map::MapSet::Rebuild)
                .in_set(crate::app::InMatch),
        );
}

/// Ask which own units can be promoted: those with experience, again only when a veteran's experience or
/// promotions change (a room's updates come often).
fn probe(link: Res<Link>, view: Res<ViewState>, mut changes: MessageReader<ViewChanged>, mut decisions: ResMut<Decisions>) {
    if changes.read().count() == 0 {
        return;
    }
    let Some(v) = view.view() else { return };
    let veterans: Vec<(Key, u16, usize)> = v
        .units
        .iter()
        .filter(|u| u.owner == v.me)
        .filter_map(|u| u.own.as_ref().filter(|o| o.experience > 0).map(|o| ((u.id, u.r#gen), o.experience, o.promotions.len())))
        .collect();
    if veterans == decisions.probed {
        return;
    }
    decisions.probed = veterans.clone();
    decisions.probe += 1;
    let n = decisions.probe;
    let keys: Vec<Key> = veterans.iter().map(|v| v.0).collect();
    decisions.promotable.retain(|k| keys.contains(k));
    for k in keys {
        link.send(Up::Client(ClientMessage::Query {
            request_id: format!("promo-{}-{}:{n}", k.0, k.1),
            query: Ask::UnitOptions { unit: entity(k), nuclear_target: None },
        }));
    }
}

fn answers(mut answers: MessageReader<QueryAnswered>, mut decisions: ResMut<Decisions>) {
    for answer in answers.read() {
        let Some((what, n)) = answer.request_id.split_once(':') else { continue };
        let Some(unit) = what.strip_prefix("promo-") else { continue };
        if n.parse::<u64>().ok() != Some(decisions.probe) {
            continue;
        }
        let Some((id, r#gen)) = unit.split_once('-').and_then(|(a, b)| Some((a.parse().ok()?, b.parse().ok()?))) else { continue };
        let can = matches!(&answer.reply.result, QueryResult::UnitOptions { options: Some(o) } if !o.promotions.is_empty());
        if can {
            decisions.promotable.insert((id, r#gen));
        } else {
            decisions.promotable.remove(&(id, r#gen));
        }
    }
}

/// The pending decisions, in the order they are put: captures, rewards, demands, the vote, the faith, promotions.
fn derive(view: Res<ViewState>, selection: Res<Selection>, mut decisions: ResMut<Decisions>) {
    let Some(update) = view.update() else { return };
    let v = &update.view;
    if decisions.turn != v.turn {
        decisions.turn = v.turn;
        decisions.later.clear();
    }
    let list = pending(v, selection.civ_options.as_ref(), &decisions.promotable);
    if decisions.open.as_ref().is_some_and(|d| !list.contains(d)) {
        decisions.open = None;
    }
    if decisions.list != list {
        decisions.list = list;
    }
}

/// The decisions a view and the civ's options leave pending, in the order they are put.
pub(crate) fn pending(v: &civ_sim::PlayerView, options: Option<&civ_sim::queries::CivOptions>, promotable: &HashSet<Key>) -> Vec<Decision> {
    let mut list = Vec::new();
    for c in v.pending_captures.iter().flatten() {
        list.push(Decision::Capture((c.city.id, c.city.r#gen)));
    }
    if v.rewards.technologies > 0 && options.is_some_and(|o| !o.free_technologies.is_empty()) {
        list.push(Decision::FreeTechnology);
    }
    if v.rewards.great_people > 0 && options.is_some_and(|o| !o.free_great_people.is_empty()) {
        list.push(Decision::FreeGreatPerson);
    }
    if v.rewards.policies > 0 {
        list.push(Decision::FreePolicy);
    }
    for d in v.demands.iter().flatten() {
        list.push(Decision::Demand(d.by, demand_code(d.demand)));
    }
    if v.endgame.voting.as_ref().is_some_and(|vote| vote.may_vote && !vote.has_voted) {
        list.push(Decision::Vote);
    }
    if options.and_then(|o| o.state_faith.as_ref()).is_some_and(|f| f.current.is_none() && f.locked_turns == 0 && !f.adoptable.is_empty()) {
        list.push(Decision::StateFaith);
    }
    let mut promote: Vec<Key> = promotable.iter().copied().collect();
    promote.sort_unstable();
    list.extend(promote.into_iter().map(Decision::Promote));
    list
}

/// Open a decision: its screen, its unit, or its card.
pub fn open(d: Decision, decisions: &mut Decisions, selection: &mut Selection, screens: &mut crate::screens::Screens) {
    match &d {
        Decision::FreeTechnology => selection.research_open = true,
        Decision::FreePolicy => screens.policies = true,
        Decision::Promote(k) => crate::play::select_unit(selection, *k),
        _ => {}
    }
    decisions.open = Some(d);
}

fn card(mut commands: Commands) {
    commands.spawn((
        Card,
        Node {
            position_type: PositionType::Absolute,
            top: Val::Px(120.0),
            left: Val::Percent(50.0),
            width: Val::Px(380.0),
            padding: UiRect::all(Val::Px(14.0)),
            flex_direction: FlexDirection::Column,
            row_gap: Val::Px(8.0),
            ..default()
        },
        UiTransform { translation: Val2::percent(-50.0, 0.0), ..default() },
        BackgroundColor(crate::screens::SOLID_PANEL),
        GlobalZIndex(25),
        Visibility::Hidden,
    ));
}

#[allow(clippy::too_many_arguments)]
fn draw(
    mut commands: Commands,
    decisions: Res<Decisions>,
    view: Res<ViewState>,
    rules: Option<Res<Rules>>,
    selection: Res<Selection>,
    font: Res<UiFont>,
    mut card: Query<(Entity, &mut Visibility, Option<&Children>), With<Card>>,
) {
    let Ok((e, mut visibility, children)) = card.single_mut() else { return };
    if !decisions.is_changed() && !view.is_changed() {
        return;
    }
    clear(&mut commands, children);
    let (Some(v), Some(d)) = (view.view(), decisions.open.clone()) else {
        *visibility = Visibility::Hidden;
        return;
    };
    let rules = rules.as_deref();
    let nation = |civ: u8| {
        v.civs
            .iter()
            .find(|c| c.id == civ)
            .and_then(|c| rules.and_then(|r| r.nation(c.nation)))
            .map_or("A civilization".to_string(), |n| n.0.to_string())
    };
    let mut rows: Vec<(String, Color)> = Vec::new();
    let mut buttons: Vec<(Press, String, bool)> = Vec::new();
    match &d {
        Decision::Capture(k) => {
            let name = v.cities.iter().find(|c| (c.id, c.r#gen) == *k).map_or("The city".into(), |c| c.name.clone());
            rows.push((format!("{name} is yours. What becomes of it?"), TEXT));
            rows.push(("Left unanswered, it stays a puppet.".into(), DIM));
            let key = civ_sim::protocol::EntityRef { id: k.0, r#gen: k.1 };
            for choice in v.pending_captures.iter().flatten().filter(|c| c.city == key).flat_map(|c| c.choices.iter()) {
                let fate = match choice {
                    CaptureChoice::Liberate => Fate::Liberate,
                    CaptureChoice::Annex => Fate::Annex,
                    CaptureChoice::Keep => Fate::Keep,
                    CaptureChoice::Raze => Fate::Raze,
                };
                buttons.push((Press::Fate(*k, fate), fate.label().into(), true));
            }
        }
        Decision::FreeGreatPerson => {
            rows.push(("A great person of your choice joins you.".into(), TEXT));
            for unit in selection.civ_options.iter().flat_map(|o| o.free_great_people.iter()) {
                let name = rules.and_then(|r| r.tables.units.iter().find(|row| row.id == *unit)).map_or("?".into(), |row| row.name.clone());
                buttons.push((Press::GreatPerson(*unit), name, true));
            }
        }
        Decision::StateFaith => {
            rows.push(("A faith followed in your cities may become your state faith; its traits then apply across your nation. It cannot change again for 10 turns.".into(), TEXT));
            let faiths = rules.and_then(|r| r.tables.faiths.as_ref());
            for key in selection.civ_options.iter().flat_map(|o| o.state_faith.iter()).flat_map(|f| f.adoptable.iter()) {
                let name = faiths.and_then(|f| f.iter().find(|row| row.key == *key)).map_or("A faith".into(), |row| row.name.clone());
                buttons.push((Press::Faith(*key), name, true));
            }
        }
        Decision::Demand(by, code) => {
            let view = v.demands.iter().flatten().find(|x| x.by == *by && demand_code(x.demand) == *code);
            let demand = view.map_or(Demand::DoNotSettleNearUs, |x| x.demand);
            let what = match demand {
                Demand::DontSpyOnUs => "stop spying on them",
                Demand::DoNotSpreadReligion => "stop spreading your faith to them",
                Demand::DoNotSettleNearUs => "stop settling near them",
                Demand::DoNotAttackUs => "not attack them",
            };
            rows.push((format!("{} demands that you {what}.", nation(*by)), TEXT));
            if let Some(view) = view {
                rows.push((format!("Agreeing is a promise until turn {}. Refusing is resented more.", view.promise_until), DIM));
                if view.refusal_declares_war {
                    rows.push(("Refusing means war.".into(), URGENT));
                }
                buttons.push((Press::Answer(*by, demand, true), "Agree".into(), true));
                buttons.push((Press::Answer(*by, demand, false), "Refuse".into(), view.may_refuse));
            }
        }
        Decision::Vote => {
            rows.push(("The world votes for its leader. Who has your vote?".into(), TEXT));
            for civ in v.endgame.voting.iter().flat_map(|vote| vote.candidates.iter()) {
                buttons.push((Press::Vote(Some(*civ)), nation(*civ), true));
            }
            buttons.push((Press::Vote(None), "Abstain".into(), true));
        }
        // These open a screen or a unit; the card only says so.
        Decision::FreeTechnology => rows.push(("Choose any technology you may research: it is yours at once.".into(), TEXT)),
        Decision::FreePolicy => rows.push(("Adopt any open policy at no cost.".into(), TEXT)),
        Decision::Promote(_) => rows.push(("Choose a promotion in the unit panel.".into(), TEXT)),
    }
    *visibility = Visibility::Inherited;
    commands.entity(e).with_children(|c| {
        c.spawn(text(&font, d.label(v, rules), 16.0, TEXT));
        for (row, colour) in rows {
            c.spawn(text(&font, row, 13.0, colour));
        }
        c.spawn(Node { flex_wrap: FlexWrap::Wrap, column_gap: Val::Px(6.0), row_gap: Val::Px(6.0), ..default() }).with_children(|b| {
            for (press, label, enabled) in buttons {
                b.spawn(button(&font, press, label, enabled));
            }
            b.spawn(button(&font, Press::Later, "Later", true));
            b.spawn(button(&font, Press::Close, "Close", true));
        });
    });
}

fn button(font: &UiFont, press: Press, label: impl Into<String>, enabled: bool) -> impl Bundle {
    (
        press,
        Button,
        Hovered::default(),
        observe(on_press),
        Node {
            min_height: Val::Px(28.0),
            padding: UiRect::axes(Val::Px(10.0), Val::Px(4.0)),
            align_items: AlignItems::Center,
            border_radius: BorderRadius::all(Val::Px(4.0)),
            ..default()
        },
        BackgroundColor(BUTTON),
        children![text(font, label, 13.0, if enabled { TEXT } else { DIM })],
    )
}

fn on_press(event: On<Activate>, presses: Query<&Press>, mut decisions: ResMut<Decisions>, mut outbox: ResMut<Outbox>) {
    let Ok(press) = presses.get(event.entity) else { return };
    let action = match press.clone() {
        Press::Fate(k, fate) => Some(fate.action(entity(k))),
        Press::GreatPerson(unit_type) => Some(Action::ClaimFreeGreatPerson { unit_type }),
        Press::Faith(religion) => Some(Action::AdoptStateFaith { religion }),
        Press::Answer(by, demand, agree) => Some(Action::AnswerDemand { by, demand, agree }),
        Press::Vote(candidate) => Some(Action::VoteDiplomatic { candidate }),
        Press::Later => {
            if let Some(d) = decisions.open.clone() {
                decisions.put_off(d);
            }
            None
        }
        Press::Close => {
            decisions.open = None;
            None
        }
    };
    if let Some(action) = action {
        outbox.submit(action);
        decisions.open = None;
    }
}

fn shade(mut buttons: Query<(&Hovered, &mut BackgroundColor), (With<Press>, Changed<Hovered>)>) {
    for (hovered, mut colour) in &mut buttons {
        colour.0 = if hovered.get() { BUTTON_ON } else { BUTTON };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use civ_sim::session::Session;
    use civ_sim::Settings;

    /// A real view and civ options, with decisions written into their JSON as the server would send them.
    #[test]
    fn decisions_come_in_the_engine_order_and_only_when_answerable() {
        let session = Session::new(Settings { seed: 2, ..Settings::default() }, "d".into(), vec![0]).unwrap();
        let mut view = serde_json::to_value(session.current_update(0).unwrap().view).unwrap();
        let QueryResult::CivOptions { options } = session.query(0, Ask::CivOptions).unwrap().result else { panic!() };
        let mut options = serde_json::to_value(options).unwrap();
        let base: civ_sim::PlayerView = serde_json::from_value(view.clone()).unwrap();
        assert!(pending(&base, None, &HashSet::new()).is_empty(), "a new match asks nothing");

        view["pendingCaptures"] = serde_json::json!([{ "city": { "id": 7, "gen": 1 }, "choices": ["annex", "keep", "raze"] }]);
        view["rewards"] = serde_json::json!({ "technologies": 1, "policies": 1, "greatPeople": 1 });
        view["demands"] = serde_json::json!([{ "by": 1, "demand": "doNotSettleNearUs", "promiseUntil": 40, "refusalDeclaresWar": false, "mayRefuse": true }]);
        options["freeTechnologies"] = serde_json::json!([3]);
        options["stateFaith"] = serde_json::json!({ "current": null, "adoptable": [128], "lockedTurns": 0 });
        let v: civ_sim::PlayerView = serde_json::from_value(view).unwrap();
        let o: civ_sim::queries::CivOptions = serde_json::from_value(options).unwrap();
        let promotable: HashSet<Key> = [(4, 1)].into();
        assert_eq!(
            pending(&v, Some(&o), &promotable),
            vec![
                Decision::Capture((7, 1)),
                Decision::FreeTechnology,
                Decision::FreePolicy,
                Decision::Demand(1, 2),
                Decision::StateFaith,
                Decision::Promote((4, 1)),
            ],
            "a free great person with none to choose from is not asked"
        );
    }
}
