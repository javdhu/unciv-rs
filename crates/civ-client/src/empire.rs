//! The empire panel (docs/ACTIONS-SPEC.md, M6b): the player's own nation at a glance, opened from the top bar.
//! Great people toward their next birth, the state faith and its choice, spies and where they work, and how
//! far each victory, the spaceship and the world vote have come.

use crate::client::Outbox;
use crate::inspect::UiFont;
use crate::play::{entity, Selection};
use crate::playui::{clear, text, BUTTON, BUTTON_ON, DIM, TEXT};
use crate::rules::Rules;
use crate::view_state::ViewState;
use bevy::picking::hover::Hovered;
use bevy::prelude::*;
use bevy::ui_widgets::{observe, Activate, Button};
use civ_sim::protocol::Action;

#[derive(Resource, Default)]
pub struct Empire {
    pub open: bool,
    /// The spy whose possible stations are listed.
    sending: Option<usize>,
}

#[derive(Component)]
struct EmpirePanel;

#[derive(Component, Clone, PartialEq, Debug)]
enum Press {
    Faith(u8),
    Withdraw(usize),
    Send(usize),
    Station(usize, crate::pieces::Key),
    Close,
}

pub fn plugin(app: &mut App) {
    app.init_resource::<Empire>()
        .add_systems(OnEnter(crate::app::AppState::Match), panel.run_if(crate::play::playing))
        .add_systems(OnExit(crate::app::AppState::Match), crate::app::reset::<Empire>)
        .add_systems(Update, (draw, shade).chain().run_if(crate::play::playing).in_set(crate::app::InMatch));
}

fn panel(mut commands: Commands) {
    commands.spawn((
        EmpirePanel,
        Node {
            position_type: PositionType::Absolute,
            top: Val::Px(52.0),
            left: Val::Px(12.0),
            width: Val::Px(360.0),
            max_height: Val::Percent(75.0),
            padding: UiRect::all(Val::Px(12.0)),
            flex_direction: FlexDirection::Column,
            row_gap: Val::Px(5.0),
            overflow: Overflow::scroll_y(),
            ..default()
        },
        BackgroundColor(crate::screens::SOLID_PANEL),
        crate::screens::PANEL_Z,
        Visibility::Hidden,
    ));
}

fn row() -> Node {
    Node { flex_wrap: FlexWrap::Wrap, column_gap: Val::Px(4.0), row_gap: Val::Px(4.0), align_items: AlignItems::Center, ..default() }
}

#[allow(clippy::too_many_arguments)]
fn draw(
    mut commands: Commands,
    empire: Res<Empire>,
    view: Res<ViewState>,
    rules: Option<Res<Rules>>,
    selection: Res<Selection>,
    font: Res<UiFont>,
    mut panel: Query<(Entity, &mut Visibility, Option<&Children>), With<EmpirePanel>>,
) {
    let Ok((e, mut visibility, children)) = panel.single_mut() else { return };
    if !empire.is_changed() && !(empire.open && (view.is_changed() || selection.is_changed())) {
        return;
    }
    clear(&mut commands, children);
    let (Some(v), Some(rules), true) = (view.view(), rules, empire.open) else {
        *visibility = Visibility::Hidden;
        return;
    };
    *visibility = Visibility::Inherited;
    let faith_name = |key: u8| rules.tables.faiths.iter().flatten().find(|f| f.key == key).map_or("a faith".to_string(), |f| f.name.clone());
    let unit_name = |id: u32| rules.tables.units.iter().find(|u| u.id == id).map_or("?".to_string(), |u| u.name.clone());
    let city_name = |k: (u32, u32)| v.cities.iter().find(|c| (c.id, c.r#gen) == k).map_or("a city".to_string(), |c| c.name.clone());
    commands.entity(e).with_children(|p| {
        p.spawn((Node { justify_content: JustifyContent::SpaceBetween, ..default() }, children![
            text(&font, "Your empire", 16.0, TEXT),
            button(&font, Press::Close, "Close", true),
        ]));

        p.spawn(text(&font, "Great people", 13.0, DIM));
        great_people(p, &font, v, &rules);

        p.spawn(text(&font, "Faith", 13.0, DIM));
        let state = v.civs.iter().find(|c| c.id == v.me).and_then(|c| c.state_faith);
        p.spawn(text(
            &font,
            format!(
                "{} · {} faith stored, {:+} a turn",
                state.map_or("No state faith".to_string(), |k| format!("State faith: {}", faith_name(k))),
                v.knowledge.faith,
                v.economy.faith
            ),
            12.0,
            TEXT,
        ));
        if let Some(options) = selection.civ_options.as_ref().and_then(|o| o.state_faith.as_ref()) {
            if options.locked_turns > 0 {
                p.spawn(text(&font, format!("It may change in {} turns.", options.locked_turns), 12.0, DIM));
            } else {
                p.spawn(row()).with_children(|r| {
                    for key in options.adoptable.iter().filter(|k| Some(**k) != options.current) {
                        r.spawn(button(&font, Press::Faith(*key), format!("Adopt {}", faith_name(*key)), true));
                    }
                });
            }
        }

        if !v.spies.is_empty() {
            p.spawn(text(&font, "Spies", 13.0, DIM));
        }
        for spy in &v.spies {
            let at = spy.station.as_ref().map_or("in hiding".to_string(), |s| format!("in {}", city_name((s.id, s.r#gen))));
            let doing = format!("{:?}", spy.action);
            let turns = if spy.turns_remaining > 0 { format!(" · {} turns", spy.turns_remaining) } else { String::new() };
            p.spawn(row()).with_children(|r| {
                r.spawn(text(&font, format!("Spy, rank {}, {at} · {}{turns}", spy.rank, words(&doing)), 12.0, TEXT));
                if spy.station.is_some() {
                    r.spawn(button(&font, Press::Withdraw(spy.id), "Withdraw", true));
                }
                if spy.action != civ_sim::SpyActionView::Dead {
                    r.spawn(button(&font, Press::Send(spy.id), "Send to…", true));
                }
            });
            if empire.sending == Some(spy.id) {
                p.spawn(row()).with_children(|r| {
                    for c in stations(v, spy.id).into_iter().take(24) {
                        let label = if c.owner == v.me { format!("{} (guard)", c.name) } else { c.name.clone() };
                        r.spawn(button(&font, Press::Station(spy.id, (c.id, c.r#gen)), label, true));
                    }
                });
            }
        }

        p.spawn(text(&font, "Victory", 13.0, DIM));
        let end = &v.endgame;
        for progress in &end.victories {
            let name = rules.tables.victories.iter().find(|x| x.id == progress.victory).map_or("Victory", |x| x.name.as_str());
            p.spawn(text(&font, format!("{name}: {} of {}", progress.completed, progress.total), 12.0, TEXT));
        }
        for part in &end.spaceship_parts {
            p.spawn(text(&font, format!("{}: {} of {} added", unit_name(part.part), part.assembled, part.needed), 12.0, TEXT));
        }
        if let Some(vote) = &end.voting {
            let state = match (vote.opens_on, vote.may_vote, vote.has_voted) {
                (_, true, false) => "the world vote is open: cast yours from End turn".to_string(),
                (_, _, true) => "you have voted".to_string(),
                (Some(t), _, _) => format!("the world vote opens on turn {t}"),
                (None, _, _) => "no world vote yet".to_string(),
            };
            p.spawn(text(&font, format!("World leader: {state}"), 12.0, TEXT));
            if let Some(last) = &vote.last_result {
                let winner = last.winner.map_or("nobody".to_string(), |w| {
                    v.civs.iter().find(|c| c.id == w).and_then(|c| rules.nation(c.nation)).map_or("a nation".into(), |n| n.0.to_string())
                });
                p.spawn(text(&font, format!("Last vote, turn {}: {winner} won ({} needed)", last.turn, last.needed), 12.0, DIM));
            }
        }
        p.spawn(text(&font, format!("Score {} · the match ends after turn {}", end.score, end.max_turns), 12.0, DIM));
    });
}

/// Where a spy may be sent, as the engine allows (`espionage::assign`, `Command::AssignSpy`): own cities to
/// guard them, and known cities of others, except those of a nation promised not to be spied on, and any
/// another spy of ours holds.
pub fn stations(v: &civ_sim::PlayerView, spy: usize) -> Vec<&civ_sim::CityView> {
    let forbidden = v.spying_forbidden.clone().unwrap_or_default();
    let held = |c: &civ_sim::CityView| v.spies.iter().any(|s| s.id != spy && s.station.as_ref().is_some_and(|k| (k.id, k.r#gen) == (c.id, c.r#gen)));
    let mut out: Vec<_> = v.cities.iter().filter(|c| (c.owner == v.me || !forbidden.contains(&c.owner)) && !held(c)).collect();
    // Own cities last: spies are mostly sent abroad.
    out.sort_by_key(|c| c.owner == v.me);
    out
}

/// Progress toward each great person (`knowledge.greatPeople`): points of the threshold, and the turns to go at
/// the present rate. Kinds in one pool share their threshold, so any birth raises it for all.
fn great_people(p: &mut ChildSpawnerCommands, font: &UiFont, v: &civ_sim::PlayerView, rules: &Rules) {
    let Some(kinds) = v.knowledge.great_people.as_ref().filter(|k| !k.is_empty()) else {
        p.spawn(text(font, "None yet: buildings, specialists and battles earn them.", 12.0, DIM));
        return;
    };
    for k in kinds {
        let name = rules.tables.units.iter().find(|u| u.id == k.unit).map_or("A great person", |u| u.name.as_str());
        let to_go = (k.threshold - k.points).max(0);
        let when = match (k.combat, k.per_turn) {
            (true, _) => "from battles".to_string(),
            (_, 0) => "nothing earns it now".to_string(),
            (_, n) => format!("+{n} a turn, about {} turns", (to_go + n - 1) / n),
        };
        p.spawn(text(font, format!("{name}: {} of {} · {when}", k.points, k.threshold), 12.0, TEXT));
    }
    if kinds.iter().filter(|k| k.pool.is_some()).count() > 1 {
        p.spawn(text(font, "The civilian kinds share one threshold: each birth raises it for all.", 11.0, DIM));
    }
}

/// `EstablishingNetwork` as "establishing network".
fn words(name: &str) -> String {
    let mut out = String::new();
    for (i, ch) in name.chars().enumerate() {
        if ch.is_uppercase() && i > 0 {
            out.push(' ');
        }
        out.push(ch.to_ascii_lowercase());
    }
    out
}

fn button(font: &UiFont, press: Press, label: impl Into<String>, enabled: bool) -> impl Bundle {
    (
        press,
        Button,
        Hovered::default(),
        observe(on_press),
        Node {
            min_height: Val::Px(26.0),
            padding: UiRect::axes(Val::Px(8.0), Val::Px(3.0)),
            align_items: AlignItems::Center,
            border_radius: BorderRadius::all(Val::Px(4.0)),
            ..default()
        },
        BackgroundColor(BUTTON),
        children![text(font, label, 12.0, if enabled { TEXT } else { DIM })],
    )
}

fn on_press(event: On<Activate>, presses: Query<&Press>, mut empire: ResMut<Empire>, mut outbox: ResMut<Outbox>) {
    let Ok(press) = presses.get(event.entity) else { return };
    match press.clone() {
        Press::Faith(religion) => {
            outbox.submit(Action::AdoptStateFaith { religion });
        }
        Press::Withdraw(spy) => {
            outbox.submit(Action::WithdrawSpy { spy });
        }
        Press::Send(spy) => empire.sending = if empire.sending == Some(spy) { None } else { Some(spy) },
        Press::Station(spy, city) => {
            outbox.submit(Action::AssignSpy { spy: spy as u16, city: entity(city) });
            empire.sending = None;
        }
        Press::Close => empire.open = false,
    }
}

fn shade(mut buttons: Query<(&Hovered, &mut BackgroundColor), (With<Press>, Changed<Hovered>)>) {
    for (hovered, mut colour) in &mut buttons {
        colour.0 = if hovered.get() { BUTTON_ON } else { BUTTON };
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn spy_actions_read_as_words() {
        assert_eq!(super::words("EstablishingNetwork"), "establishing network");
        assert_eq!(super::words("Hideout"), "hideout");
    }
}
