//! M6b's actions against the engine on a developed civ: seat 0 is handed to a bot for a couple of hundred
//! rounds, taken back, and then every great person ability, religious action, faith purchase, state faith
//! and spy station the engine offers is used, as the client's panels send them. A round can also be saved
//! and restored on the way, as the solo host does.

use civ_sim::protocol::{Action, CommandRequest, CommandStatus, Currency, EntityRef, Query, QueryResult};
use civ_sim::session::Session;
use civ_sim::Settings;

struct Seat {
    session: Session,
    n: u32,
}

impl Seat {
    fn send(&mut self, action: Action) -> CommandStatus {
        self.n += 1;
        let update = self.session.current_update(0).unwrap();
        let request =
            CommandRequest { command_id: format!("late{}", self.n), epoch: update.cursor.epoch.clone(), turn_token: update.turn_token.clone(), action };
        let t = self.session.stage_command(0, request).unwrap();
        let status = t.receipt.result.clone();
        self.session = t.next;
        status
    }

    fn ask(&self, query: Query) -> QueryResult {
        self.session.query(0, query).unwrap().result
    }
}

#[test]
fn a_developed_civ_uses_its_great_people_faith_and_spies() {
    let settings = Settings { seed: 11, width: 40, height: 25, majors: 4, city_states: 3, ..Settings::default() };
    let mut session = Session::new(settings, "late".into(), vec![0]).unwrap();
    session = session.stage_controller(0, false).unwrap().next;
    // The bot plays seat 0; rounds close as at a room's deadline.
    for _ in 0..140 {
        let turn = session.turn();
        let Some(t) = session.stage_timeout(turn).unwrap() else { break };
        if t.next.finished() {
            break;
        }
        session = t.next;
    }
    assert!(!session.finished());
    // A save of the developed match restores to the same view (what the solo host's autosave relies on).
    let restored = Session::restore(&session.save().unwrap()).unwrap();
    assert_eq!(restored.current_update(0).unwrap().view, session.current_update(0).unwrap().view);
    // Taking the seat back applies at the next round's close.
    session = session.stage_controller(0, true).unwrap().next;
    let turn = session.turn();
    session = session.stage_timeout(turn).unwrap().expect("a round").next;
    assert!(session.current_update(0).unwrap().controlled);
    let mut seat = Seat { session, n: 0 };
    let mut used: std::collections::BTreeSet<String> = Default::default();
    // Play the seat on, idle, using what each turn brings: great people are born on their own, faith gathers.
    for _ in 0..120 {
        use_what_comes(&mut seat, &mut used);
        if used.iter().any(|u| u.starts_with("great person")) && used.iter().any(|u| u.starts_with("faith purchase")) {
            break;
        }
        let turn = seat.session.turn();
        let status = seat.send(Action::FinishTurn);
        assert_eq!(status, CommandStatus::Applied);
        if seat.session.turn() == turn || seat.session.finished() {
            break;
        }
    }
    let v = seat.session.current_update(0).unwrap().view;
    eprintln!("turn {} with {} cities", v.turn, v.cities.iter().filter(|c| c.owner == v.me).count());
    eprintln!("used: {used:?}");
    let kinds: std::collections::BTreeSet<&str> = used.iter().map(|u| u.split(':').next().unwrap_or("")).collect();
    assert!(kinds.len() >= 3, "too little came up to test: {used:?}");
}

/// Use one of everything the engine offers this turn, as the client's panels would, checking each is taken.
fn use_what_comes(seat: &mut Seat, used: &mut std::collections::BTreeSet<String>) {
    let v = seat.session.current_update(0).unwrap().view;
    for unit in v.units.iter().filter(|u| u.owner == v.me) {
        let key = EntityRef { id: unit.id, r#gen: unit.r#gen };
        let QueryResult::UnitOptions { options: Some(o) } = seat.ask(Query::UnitOptions { unit: key.clone(), nuclear_target: None }) else { continue };
        if let Some(ability) = o.great_person_abilities.iter().flatten().find(|a| a.available) {
            let status = seat.send(Action::GreatPerson { unit: key.clone(), action: ability.action.clone() });
            assert_eq!(status, CommandStatus::Applied, "{:?}", ability.action);
            used.insert(format!("great person: {:?}", ability.action));
            continue;
        }
        if let Some(action) = o.religious_actions.first() {
            assert_eq!(seat.send(Action::Religious { unit: key, action: action.clone() }), CommandStatus::Applied, "{action:?}");
            used.insert(format!("religious: {action:?}"));
        }
    }
    let QueryResult::CivOptions { options } = seat.ask(Query::CivOptions) else { panic!() };
    if let Some(f) = options.state_faith.as_ref().filter(|f| f.locked_turns == 0) {
        if let Some(key) = f.adoptable.iter().find(|k| Some(**k) != f.current) {
            assert_eq!(seat.send(Action::AdoptStateFaith { religion: *key }), CommandStatus::Applied);
            used.insert("state faith".into());
        }
    }
    for city in v.cities.iter().filter(|c| c.owner == v.me) {
        let key = EntityRef { id: city.id, r#gen: city.r#gen };
        let QueryResult::CityOptions { options: Some(o) } = seat.ask(Query::CityOptions { city: key.clone() }) else { continue };
        if let Some(choice) = o.choices.iter().find(|c| c.faith.as_ref().is_some_and(|f| f.affordable == Some(true))) {
            let status = seat.send(Action::Purchase { city: key, what: choice.construction.clone(), currency: Currency::Faith });
            assert_eq!(status, CommandStatus::Applied, "faith purchase of {}", choice.name);
            used.insert(format!("faith purchase: {}", choice.name));
            break;
        }
    }
    if let Some(spy) = v.spies.iter().find(|s| s.station.is_none() && s.action != civ_sim::SpyActionView::Dead) {
        // The first station the empire panel would offer.
        if let Some(c) = civ_client::empire::stations(&v, spy.id).first() {
            let status = seat.send(Action::AssignSpy { spy: spy.id as u16, city: EntityRef { id: c.id, r#gen: c.r#gen } });
            assert_eq!(status, CommandStatus::Applied, "spy to {}", c.name);
            used.insert("spy".into());
        }
    }
}

/// `CIV_WRITE_SAVE=<file> cargo test -p civ-client --test late -- --ignored`: a developed match for the
/// review scenes (`CIV_SAVE`), seat 0 played by a bot for 140 rounds and then handed back.
#[test]
#[ignore]
fn write_a_developed_save() {
    let path = std::env::var("CIV_WRITE_SAVE").expect("CIV_WRITE_SAVE names the file");
    let settings = Settings { seed: 11, width: 40, height: 25, majors: 4, city_states: 3, ..Settings::default() };
    let mut session = Session::new(settings, "late".into(), vec![0]).unwrap();
    session = session.stage_controller(0, false).unwrap().next;
    for _ in 0..140 {
        let turn = session.turn();
        let Some(t) = session.stage_timeout(turn).unwrap() else { break };
        if t.next.finished() {
            break;
        }
        session = t.next;
    }
    session = session.stage_controller(0, true).unwrap().next;
    let turn = session.turn();
    session = session.stage_timeout(turn).unwrap().expect("a round").next;
    std::fs::write(path, session.save().unwrap()).unwrap();
}
