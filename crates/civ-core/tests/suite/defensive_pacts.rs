// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use civ_core::{
    ruleset::Ruleset,
    stats::Fixed,
    world::{
        self,
        agreements::{self, Terms},
        commands::{self, Command},
        defensive_pacts as dp,
        diplomacy::Sentiment,
        City, CivId, Civilization, TileGrid, TileId, TurnEvent, TurnModifiers, World,
    },
};
const A: CivId = CivId(0);
const B: CivId = CivId(1);
const C: CivId = CivId(2);
fn setup() -> (Ruleset, World) {
    let r = Ruleset::load_dir("rules/gods-and-kings").unwrap().unwrap();
    let mut w = World::new(
        TileGrid::new(30, 12, r.terrain("Grassland").unwrap()),
        (0..4).map(|i| Civilization::new(CivId(i), r.nations.id("Rome").unwrap(), false)).collect(),
    );
    for i in 0..4u8 {
        w.found_city_in(&r, City::found(format!("Capital{i}"), CivId(i), TileId(31 + u16::from(i) * 7)));
        for tech in ["Writing", "Chivalry"] {
            w.civ_mut(CivId(i)).unwrap().techs.insert(r.technologies.id(tech).unwrap());
        }
        world::policies::rebuild_uniques(&mut w, &r, CivId(i));
        for j in 0..i {
            w.diplomacy.meet(CivId(i), CivId(j));
        }
    }
    (r, w)
}
fn relation(w: &mut World, r: &Ruleset, a: CivId, b: CivId) {
    agreements::accept(w, r, a, b, &Terms::Friendship, TurnModifiers::default()).unwrap();
    agreements::accept(
        w,
        r,
        a,
        b,
        &Terms::Trade { give: vec![world::trade::Item::Embassy], get: vec![world::trade::Item::Embassy] },
        TurnModifiers::default(),
    )
    .unwrap();
}
fn sign(w: &mut World, r: &Ruleset, a: CivId, b: CivId) {
    relation(w, r, a, b);
    agreements::accept(w, r, a, b, &Terms::DefensivePact, TurnModifiers::default()).unwrap();
}
#[test]
fn technology_embassies_friendship_and_bilateral_consent_are_required() {
    let (r, mut w) = setup();
    let m = TurnModifiers::default();
    assert!(!dp::can_sign(&w, &r, A, B, m));
    agreements::accept(&mut w, &r, A, B, &Terms::Friendship, m).unwrap();
    assert!(!dp::can_sign(&w, &r, A, B, m));
    agreements::accept(&mut w, &r, A, B, &Terms::Trade { give: vec![world::trade::Item::Embassy], get: vec![] }, m)
        .unwrap();
    assert!(!dp::can_sign(&w, &r, A, B, m));
    agreements::accept(&mut w, &r, B, A, &Terms::Trade { give: vec![world::trade::Item::Embassy], get: vec![] }, m)
        .unwrap();
    assert!(dp::can_sign(&w, &r, A, B, m));
    let before = w.clone();
    assert!(commands::execute(&mut w, &r, A, Command::OfferDefensivePact { target: B }).is_err());
    assert_eq!(w, before, "a player command cannot consent for another human");
    let events = agreements::accept(&mut w, &r, A, B, &Terms::DefensivePact, m).unwrap().events;
    assert!(events.iter().any(|e| matches!(e,TurnEvent::DefensivePactSigned{a,b} if *a==A&&*b==B)));
    assert!(!dp::can_sign(&w, &r, A, B, m));
    assert_eq!(w.diplomacy.attitude(A, B).get(Sentiment::DefensivePact), Fixed::whole(10));
}
#[test]
fn proposal_does_not_probe_hidden_recipient_technology_or_life() {
    let (r, mut w) = setup();
    relation(&mut w, &r, A, B);
    let m = TurnModifiers::default();
    let mut hidden = w.clone();
    hidden.civ_mut(B).unwrap().techs.remove(&r.technologies.id("Chivalry").unwrap());
    world::policies::rebuild_uniques(&mut hidden, &r, B);
    assert_eq!(
        agreements::validate_proposal(&w, &r, A, B, &Terms::DefensivePact, m),
        agreements::validate_proposal(&hidden, &r, A, B, &Terms::DefensivePact, m)
    );
    assert!(dp::can_propose(&hidden, &r, A, B, m));
    let before = hidden.clone();
    assert!(agreements::accept(&mut hidden, &r, A, B, &Terms::DefensivePact, m).is_err());
    assert_eq!(hidden, before, "recipient technology is revalidated atomically on acceptance");
    hidden.civ_mut(B).unwrap().cities.clear();
    assert!(dp::can_propose(&hidden, &r, A, B, m));
}
#[test]
fn expiry_uses_deal_duration_and_bonus_fades_only_after_expiry() {
    let (r, mut w) = setup();
    relation(&mut w, &r, A, B);
    let mut m = TurnModifiers::default();
    m.costs.speed = Fixed::permille(670);
    m.costs.deal_duration = 25;
    agreements::accept(&mut w, &r, A, B, &Terms::DefensivePact, m).unwrap();
    for _ in 0..24 {
        w.diplomacy.tick();
        world::opinion::tick(&mut w, m.costs.speed);
    }
    let pact = w.diplomacy.pact(A, B).unwrap();
    assert!(pact.defensive_pact);
    assert_eq!(pact.defensive_pact_turns, 1);
    assert_eq!(w.diplomacy.attitude(A, B).get(Sentiment::DefensivePact), Fixed::whole(10));
    w.diplomacy.tick();
    world::opinion::tick(&mut w, m.costs.speed);
    assert!(!w.diplomacy.pact(A, B).unwrap().defensive_pact);
    assert_eq!(w.diplomacy.attitude(A, B).get(Sentiment::DefensivePact), Fixed::whole(9));
}
#[test]
fn defender_calls_pact_partner_without_recursive_pact_cascade() {
    let (r, mut w) = setup();
    sign(&mut w, &r, B, C);
    sign(&mut w, &r, C, CivId(3));
    let events = world::diplomacy_actions::declare_war(&mut w, &r, A, B);
    assert!(w.diplomacy.at_war(A, B));
    assert!(w.diplomacy.at_war(A, C));
    assert!(!w.diplomacy.at_war(A, CivId(3)));
    assert!(events.iter().any(|e| matches!(e,TurnEvent::WarDeclared{aggressor,against} if *aggressor==A&&*against==C)));
    assert!(w.diplomacy.pact(B, C).unwrap().defensive_pact);
}
#[test]
fn aggression_cancels_own_pacts_and_direct_betrayal_is_remembered() {
    let (r, mut w) = setup();
    sign(&mut w, &r, A, B);
    sign(&mut w, &r, A, C);
    world::diplomacy_actions::declare_war(&mut w, &r, A, B);
    for other in [B, C] {
        let p = w.diplomacy.pact(A, other).unwrap();
        assert!(!p.defensive_pact);
        assert_eq!(p.defensive_pact_turns, 0);
    }
    assert_eq!(w.diplomacy.attitude(B, A).get(Sentiment::BetrayedDefensivePact), Fixed::whole(-20));
    assert_eq!(w.diplomacy.attitude(C, A).get(Sentiment::BetrayedDefensivePact), Fixed::whole(-10));
}
#[test]
fn ai_consent_and_initiation_use_willingness_without_controlling_humans() {
    let (r, mut w) = setup();
    relation(&mut w, &r, A, B);
    w.civ_mut(B).unwrap().is_ai = true;
    assert!(commands::execute(&mut w, &r, A, Command::OfferDefensivePact { target: B }).is_err());
    w.diplomacy.attitude_mut(B, A).set(Sentiment::YearsOfPeace, Fixed::whole(60));
    assert!(dp::wants_to_sign(&w, &r, B, A));
    commands::execute(&mut w, &r, A, Command::OfferDefensivePact { target: B }).unwrap();
    let (r, mut w) = setup();
    relation(&mut w, &r, A, B);
    for (a, b) in [(A, B), (B, A)] {
        w.diplomacy.attitude_mut(a, b).set(Sentiment::YearsOfPeace, Fixed::whole(60));
    }
    w.civ_mut(B).unwrap().is_ai = true;
    world::research_agreements::automate(&mut w, &r, TurnModifiers::default(), false);
    assert!(!w.diplomacy.pact(A, B).unwrap().defensive_pact, "human consent cannot be automated");
    w.civ_mut(A).unwrap().is_ai = true;
    world::research_agreements::automate(&mut w, &r, TurnModifiers::default(), false);
    assert!(w.diplomacy.pact(A, B).unwrap().defensive_pact);
}
#[test]
fn common_known_partners_gain_correct_third_party_opinions_and_clear_on_expiry() {
    let (r, mut w) = setup();
    w.civ_mut(A).unwrap().is_ai = true;
    w.diplomacy.attitude_mut(A, C).set(Sentiment::YearsOfPeace, Fixed::whole(90));
    sign(&mut w, &r, B, C);
    assert_eq!(w.diplomacy.attitude(A, B).get(Sentiment::DefensivePactWithAllies), Fixed::whole(5));
    w.diplomacy.attitude_mut(A, C).set(Sentiment::YearsOfPeace, Fixed::whole(-50));
    dp::refresh_opinions(&mut w);
    assert_eq!(w.diplomacy.attitude(A, B).get(Sentiment::DefensivePactWithEnemies), Fixed::whole(-10));
    dp::cancel(&mut w, B, C);
    dp::refresh_opinions(&mut w);
    assert_eq!(w.diplomacy.attitude(A, B).get(Sentiment::DefensivePactWithEnemies), Fixed::ZERO);
}
#[cfg(feature = "save")]
#[test]
fn treaty_lifetime_roundtrips_and_invalid_boolean_timer_pairs_are_rejected() {
    let (r, mut w) = setup();
    sign(&mut w, &r, A, B);
    w.diplomacy.tick();
    let encoded = civ_core::save::encode(&w, &r).unwrap();
    let copy = civ_core::save::decode(&encoded, &r).unwrap();
    assert_eq!(copy, w);
    let mut value = serde_json::to_value(&w).unwrap();
    value["diplomacy"]["pacts"][0][1]["defensive_pact_turns"] = serde_json::json!(0);
    let w: World = serde_json::from_value(value).unwrap();
    assert!(civ_core::save::encode(&w, &r).is_err());
}

#[test]
fn refusing_attack_demand_uses_full_war_transition_and_reports_coalition() {
    use world::{
        promises::{self, Demand},
        trade::{Deal, Item, Running},
        Unit,
    };
    let (r, mut w) = setup();
    w.civ_mut(B).unwrap().is_ai = true;
    sign(&mut w, &r, B, C);
    let centre = w.cities.get(w.civ(B).unwrap().cities[0]).unwrap().centre;
    let inside = w.grid.neighbours(centre).next().unwrap();
    w.grid.owner[inside.0 as usize] = Some(B);
    w.diplomacy.set_open_borders(B, A, true);
    w.diplomacy.deals.push(Deal {
        a: A,
        b: B,
        a_gives: vec![],
        b_gives: vec![Running { item: Item::OpenBorders, turns_left: 30 }],
    });
    let unit = w.place_unit(Unit::new(r.unit("Warrior").unwrap(), A, inside, 2));
    promises::demand(&mut w, B, A, Demand::DoNotAttackUs).unwrap();
    let outcome =
        commands::execute(&mut w, &r, A, Command::AnswerDemand { by: B, demand: Demand::DoNotAttackUs, agree: false })
            .unwrap();
    assert!(w.diplomacy.at_war(A, B));
    assert!(w.diplomacy.at_war(A, C));
    assert!(w.diplomacy.deals.is_empty());
    assert!(!w.diplomacy.may_enter_borders_of(A, B));
    assert_ne!(w.units.get(unit).unwrap().position, inside, "the visitor is expelled before war opens enemy territory");
    for target in [B, C] {
        assert!(outcome
            .events
            .iter()
            .any(|e| matches!(e,TurnEvent::WarDeclared{aggressor,against} if *aggressor==A&&*against==target)));
    }
    assert!(outcome
        .events
        .iter()
        .any(|e| matches!(e,TurnEvent::DemandAnswered{by,to,agreed:false,..} if *by==A&&*to==B)));
}

#[test]
fn refusing_attack_demand_cannot_bypass_a_new_peace_treaty_or_mutate_on_rejection() {
    use world::promises::{self, Demand};
    let (r, mut w) = setup();
    w.civ_mut(B).unwrap().is_ai = true;
    promises::demand(&mut w, B, A, Demand::DoNotAttackUs).unwrap();
    w.diplomacy.declare_war(A, B);
    w.diplomacy.make_peace(A, B);
    let before = w.clone();
    assert!(promises::answer(&mut w, &r, A, B, Demand::DoNotAttackUs, false).is_err());
    assert_eq!(w, before);
    assert!(commands::execute(
        &mut w,
        &r,
        A,
        Command::AnswerDemand { by: B, demand: Demand::DoNotAttackUs, agree: false }
    )
    .is_err());
    assert_eq!(w, before);
}
