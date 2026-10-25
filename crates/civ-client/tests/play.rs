// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! A played turn through the Outbox and the in-process host, headless: found a city, set research, end the
//! turn; a retried command id gets the original receipt and a reused one with other content is refused.

use bevy::prelude::*;
use civ_client::client::{self, HostError, Outbox, QueryAnswered, ReceiptArrived};
use civ_client::link::{Link, Up};
use civ_client::view_state::ViewState;
use civ_sim::protocol::{Action, ClientMessage, CommandRequest, CommandStatus, Query, QueryResult, RejectionCode};
use civ_sim::Settings;
use std::time::{Duration, Instant};

#[derive(Resource, Default)]
struct Seen {
    receipts: Vec<ReceiptArrived>,
    answers: Vec<QueryAnswered>,
    errors: Vec<String>,
}

fn collect(
    mut receipts: MessageReader<ReceiptArrived>,
    mut answers: MessageReader<QueryAnswered>,
    mut errors: MessageReader<HostError>,
    mut seen: ResMut<Seen>,
) {
    seen.receipts.extend(receipts.read().cloned());
    seen.answers.extend(answers.read().cloned());
    seen.errors.extend(errors.read().map(|e| e.0.clone()));
}

fn app() -> App {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins)
        .add_plugins(client::plugin)
        .init_resource::<Seen>()
        .add_systems(Update, collect)
        .insert_resource(Link::play(Settings { seed: 3, ..Settings::default() }));
    app
}

fn pump_until(app: &mut App, what: &str, done: impl Fn(&App) -> bool) {
    let started = Instant::now();
    while !done(app) {
        app.update();
        assert!(started.elapsed() < Duration::from_secs(60), "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(1));
    }
}

fn receipt(app: &App, id: &str) -> Option<CommandStatus> {
    app.world().resource::<Seen>().receipts.iter().find(|r| r.request_id == id).map(|r| r.receipt.result.clone())
}

#[test]
fn a_played_turn_founds_a_city_sets_research_and_ends() {
    let mut app = app();
    pump_until(&mut app, "the welcome", |a| a.world().resource::<ViewState>().view().is_some());
    let rules = civ_sim::tables(&civ_sim::content::load().unwrap());
    let settler = rules.units.iter().find(|u| u.name == "Settler").unwrap().id;
    let view = app.world().resource::<ViewState>().view().unwrap().clone();
    let unit = view.units.iter().find(|u| u.owner == view.me && u.base == settler).expect("a settler to start with");
    let unit = civ_sim::protocol::EntityRef { id: unit.id, r#gen: unit.r#gen };

    let found = app.world_mut().resource_mut::<Outbox>().submit(Action::FoundCity { unit: unit.clone(), name: "Testburg".into() });
    pump_until(&mut app, "the founding's receipt", |a| receipt(a, &found).is_some());
    assert_eq!(receipt(&app, &found), Some(CommandStatus::Applied));
    pump_until(&mut app, "the city in the view", |a| {
        a.world().resource::<ViewState>().view().unwrap().cities.iter().any(|c| c.name == "Testburg")
    });

    app.world().resource::<Link>().send(Up::Client(ClientMessage::Query { request_id: "civ".into(), query: Query::CivOptions }));
    pump_until(&mut app, "the civ options", |a| a.world().resource::<Seen>().answers.iter().any(|q| q.request_id == "civ"));
    let tech = match &app.world().resource::<Seen>().answers.iter().find(|q| q.request_id == "civ").unwrap().reply.result {
        QueryResult::CivOptions { options } => options.research.first().expect("something to research").id,
        other => panic!("not civ options: {other:?}"),
    };
    let research = app.world_mut().resource_mut::<Outbox>().submit(Action::SetResearch { technology: tech });

    let turn = app.world().resource::<ViewState>().view().unwrap().turn;
    let finish = app.world_mut().resource_mut::<Outbox>().submit(Action::FinishTurn);
    pump_until(&mut app, "the turn to end", |a| receipt(a, &finish).is_some());
    assert_eq!(receipt(&app, &research), Some(CommandStatus::Applied));
    assert_eq!(receipt(&app, &finish), Some(CommandStatus::Applied));
    pump_until(&mut app, "the next turn", |a| a.world().resource::<ViewState>().view().unwrap().turn > turn);
    assert!(app.world().resource::<Seen>().errors.is_empty());
}

#[test]
fn retries_get_the_original_receipt_and_reused_ids_are_refused() {
    let mut app = app();
    pump_until(&mut app, "the welcome", |a| a.world().resource::<ViewState>().view().is_some());
    let update = app.world().resource::<ViewState>().update().unwrap().clone();
    let request = |id: &str, action: Action| CommandRequest {
        command_id: "fixed".into(),
        epoch: update.cursor.epoch.clone(),
        turn_token: update.turn_token.clone(),
        action,
    }
    .with_request(id);
    for (id, action) in [
        ("first", Action::SetResearch { technology: 0 }),
        ("retry", Action::SetResearch { technology: 0 }),
        ("reuse", Action::FinishTurn),
    ] {
        app.world().resource::<Link>().send(request(id, action));
        pump_until(&mut app, id, |a| receipt(a, id).is_some());
    }
    let first = receipt(&app, "first").unwrap();
    assert_eq!(receipt(&app, "retry"), Some(first));
    assert_eq!(receipt(&app, "reuse"), Some(CommandStatus::Rejected { code: RejectionCode::CommandIdReused }));
}

trait WithRequest {
    fn with_request(self, id: &str) -> Up;
}

impl WithRequest for CommandRequest {
    fn with_request(self, id: &str) -> Up {
        Up::Client(ClientMessage::Command { request_id: id.into(), command: self })
    }
}

/// Working a free tile moves a citizen there, as the engine applies it.
#[test]
fn working_a_tile_moves_a_citizen_there() {
    use civ_client::play::{reassign, Change};
    let mut app = app();
    pump_until(&mut app, "the welcome", |a| a.world().resource::<ViewState>().view().is_some());
    let rules = civ_sim::tables(&civ_sim::content::load().unwrap());
    let settler = rules.units.iter().find(|u| u.name == "Settler").unwrap().id;
    let view = app.world().resource::<ViewState>().view().unwrap().clone();
    let unit = view.units.iter().find(|u| u.owner == view.me && u.base == settler).unwrap();
    let unit = civ_sim::protocol::EntityRef { id: unit.id, r#gen: unit.r#gen };
    let found = app.world_mut().resource_mut::<Outbox>().submit(Action::FoundCity { unit, name: "Workburg".into() });
    pump_until(&mut app, "the founding", |a| receipt(a, &found).is_some());
    pump_until(&mut app, "the city", |a| a.world().resource::<ViewState>().view().unwrap().cities.iter().any(|c| c.name == "Workburg"));
    let city = app.world().resource::<ViewState>().view().unwrap().cities.iter().find(|c| c.name == "Workburg").unwrap().clone();
    let key = civ_sim::protocol::EntityRef { id: city.id, r#gen: city.r#gen };
    app.world().resource::<Link>().send(Up::Client(ClientMessage::Query { request_id: "city".into(), query: Query::CityOptions { city: key.clone() } }));
    pump_until(&mut app, "the city options", |a| a.world().resource::<Seen>().answers.iter().any(|q| q.request_id == "city"));
    let options = match &app.world().resource::<Seen>().answers.iter().find(|q| q.request_id == "city").unwrap().reply.result {
        QueryResult::CityOptions { options } => options.clone().expect("own city options"),
        other => panic!("not city options: {other:?}"),
    };
    let tile = options.workable_tiles.iter().find(|t| !t.worked && !t.taken).expect("a free tile").tile;
    let own = city.own.as_ref().expect("own city");
    let (worked, specialists) = reassign(&city, own, &options, Change::Work(tile)).expect("a citizen to move");
    assert!(worked.contains(&tile));
    let assign = app.world_mut().resource_mut::<Outbox>().submit(Action::AssignCitizens { city: key, worked, specialists });
    pump_until(&mut app, "the assignment", |a| receipt(a, &assign).is_some());
    assert_eq!(receipt(&app, &assign), Some(CommandStatus::Applied));
    pump_until(&mut app, "the city working the tile", |a| {
        a.world().resource::<ViewState>().view().unwrap().cities.iter().find(|c| c.name == "Workburg").and_then(|c| c.own.as_ref()).is_some_and(|o| o.worked.contains(&tile))
    });
}

/// The solo host saves when a round closes; a host resumed from that save welcomes at the same turn and
/// plays on. A save it cannot read stops the host with why.
#[test]
fn a_turns_autosave_resumes_where_it_left_off() {
    use civ_client::client::{Autosaved, LinkState};
    #[derive(Resource, Default)]
    struct Saved(Vec<Vec<u8>>);
    fn keep(mut saved: MessageReader<Autosaved>, mut out: ResMut<Saved>) {
        out.0.extend(saved.read().map(|s| s.0.clone()));
    }
    let mut first = app();
    first.init_resource::<Saved>().add_systems(Update, keep);
    pump_until(&mut first, "the welcome", |a| a.world().resource::<ViewState>().view().is_some());
    first.world_mut().resource_mut::<Outbox>().submit(Action::FinishTurn);
    pump_until(&mut first, "the autosave", |a| !a.world().resource::<Saved>().0.is_empty());
    let turn = first.world().resource::<ViewState>().view().unwrap().turn;
    let save = first.world().resource::<Saved>().0[0].clone();

    let mut resumed = App::new();
    resumed
        .add_plugins(MinimalPlugins)
        .add_plugins(client::plugin)
        .init_resource::<Seen>()
        .add_systems(Update, collect)
        .insert_resource(Link::resume(save));
    pump_until(&mut resumed, "the resumed welcome", |a| a.world().resource::<ViewState>().view().is_some());
    assert_eq!(resumed.world().resource::<ViewState>().view().unwrap().turn, turn);
    let finish = resumed.world_mut().resource_mut::<Outbox>().submit(Action::FinishTurn);
    pump_until(&mut resumed, "a turn after resuming", |a| receipt(a, &finish).is_some());
    assert_eq!(receipt(&resumed, &finish), Some(CommandStatus::Applied));

    let mut broken = App::new();
    broken.add_plugins(MinimalPlugins).add_plugins(client::plugin).insert_resource(Link::resume(b"not a save".to_vec()));
    pump_until(&mut broken, "the refusal", |a| a.world().resource::<LinkState>().stopped.is_some());
    let why = broken.world().resource::<LinkState>().stopped.clone().unwrap();
    assert!(why.contains("cannot be opened"), "{why}");
}
