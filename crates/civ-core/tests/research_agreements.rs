use civ_core::ruleset::Ruleset;
use civ_core::stats::Fixed;
use civ_core::world::agreements::{accept, validate_proposal, Terms};
use civ_core::world::commands::{execute, Command};
use civ_core::world::diplomacy::Sentiment;
use civ_core::world::research_agreements as ra;
use civ_core::world::trade::Item;
use civ_core::world::{City, CivId, Civilization, TileGrid, TileId, TurnEvent, TurnModifiers, World};
use civ_core::{Unique, UniqueType};

fn setup() -> (Ruleset, World) {
    let r = Ruleset::load_dir("rules/gods-and-kings").unwrap().unwrap();
    let mut w = World::new(
        TileGrid::new(20, 20, r.terrain("Grassland").unwrap()),
        (0..2).map(|i| Civilization::new(CivId(i), r.nations.id("Rome").unwrap(), false)).collect(),
    );
    for (i, tile) in [(0, TileId(21)), (1, TileId(321))] {
        w.found_city_in(&r, City::found("Capital", CivId(i), tile));
        let c = w.civ_mut(CivId(i)).unwrap();
        c.gold = 1000;
        c.techs.insert(r.technologies.id("Writing").unwrap());
        c.techs.insert(r.technologies.id("Education").unwrap());
        civ_core::world::policies::rebuild_uniques(&mut w, &r, CivId(i));
    }
    w.diplomacy.meet(CivId(0), CivId(1));
    (r, w)
}
fn establish(w: &mut World, r: &Ruleset) {
    accept(w, r, CivId(0), CivId(1), &Terms::Friendship, TurnModifiers::default()).unwrap();
    accept(
        w,
        r,
        CivId(0),
        CivId(1),
        &Terms::Trade { give: vec![Item::Embassy], get: vec![Item::Embassy] },
        TurnModifiers::default(),
    )
    .unwrap();
}

#[test]
fn agreements_require_mutual_embassies_friendship_and_consent_before_payment() {
    let (r, mut w) = setup();
    let mods = TurnModifiers::default();
    assert!(!ra::can_sign(&w, &r, CivId(0), CivId(1), mods));
    let before = w.clone();
    assert!(execute(&mut w, &r, CivId(0), Command::OfferFriendship { target: CivId(1) }).is_err());
    assert_eq!(before, w, "a human cannot unilaterally make the other human agree");
    accept(&mut w, &r, CivId(0), CivId(1), &Terms::Friendship, mods).unwrap();
    assert!(!ra::can_sign(&w, &r, CivId(0), CivId(1), mods));
    accept(&mut w, &r, CivId(0), CivId(1), &Terms::Trade { give: vec![Item::Embassy], get: vec![] }, mods).unwrap();
    assert!(w.diplomacy.has_embassy(CivId(1), CivId(0)));
    assert!(!w.diplomacy.has_embassy(CivId(0), CivId(1)));
    assert!(!ra::can_sign(&w, &r, CivId(0), CivId(1), mods));
    accept(&mut w, &r, CivId(1), CivId(0), &Terms::Trade { give: vec![Item::Embassy], get: vec![] }, mods).unwrap();
    assert!(w.visibility.has_explored(CivId(0), TileId(321)));
    let fee = ra::cost(&w, &r, CivId(0), CivId(1), Fixed::ONE).unwrap();
    assert_eq!(fee, 250);
    let before = w.clone();
    assert!(execute(&mut w, &r, CivId(0), Command::OfferResearchAgreement { target: CivId(1) }).is_err());
    assert_eq!(before, w);
    accept(&mut w, &r, CivId(0), CivId(1), &Terms::ResearchAgreement, mods).unwrap();
    assert_eq!(w.civ(CivId(0)).unwrap().gold, 1000 - fee);
    assert_eq!(w.civ(CivId(1)).unwrap().gold, 1000 - fee);
    assert_eq!(w.diplomacy.pact(CivId(0), CivId(1)).unwrap().research.as_ref().unwrap().turns_left, 30);
    let before = w.clone();
    assert!(accept(&mut w, &r, CivId(0), CivId(1), &Terms::ResearchAgreement, mods).is_err());
    assert_eq!(before, w);
}

#[test]
fn proposal_does_not_probe_private_recipient_treasury_era_or_technology() {
    let (r, mut w) = setup();
    establish(&mut w, &r);
    let mods = TurnModifiers::default();
    w.civ_mut(CivId(0)).unwrap().gold = 250;
    let mut hidden = w.clone();
    hidden.civ_mut(CivId(1)).unwrap().gold = 0;
    hidden.civ_mut(CivId(1)).unwrap().techs.insert(r.technologies.id("Nanotechnology").unwrap());
    civ_core::world::policies::rebuild_uniques(&mut hidden, &r, CivId(1));
    assert_eq!(
        validate_proposal(&w, &r, CivId(0), CivId(1), &Terms::ResearchAgreement, mods),
        validate_proposal(&hidden, &r, CivId(0), CivId(1), &Terms::ResearchAgreement, mods)
    );
    assert!(ra::can_propose(&hidden, &r, CivId(0), CivId(1), mods));
    let before = hidden.clone();
    assert!(accept(&mut hidden, &r, CivId(0), CivId(1), &Terms::ResearchAgreement, mods).is_err());
    assert_eq!(before, hidden, "acceptance alone checks both actual prices atomically");
    hidden.civ_mut(CivId(1)).unwrap().gold = 1000;
    let before = hidden.clone();
    assert!(accept(&mut hidden, &r, CivId(0), CivId(1), &Terms::ResearchAgreement, mods).is_err());
    assert_eq!(before, hidden, "the poorer proposer must also afford the actual higher-era fee");
}

#[test]
fn expiry_uses_lower_accumulated_science_once_and_waits_for_selected_research() {
    let (r, mut w) = setup();
    establish(&mut w, &r);
    let mut mods = TurnModifiers::default();
    mods.costs.deal_duration = 2;
    accept(&mut w, &r, CivId(0), CivId(1), &Terms::ResearchAgreement, mods).unwrap();
    w.civ_mut(CivId(0)).unwrap().recent_science[0] = 90;
    w.civ_mut(CivId(1)).unwrap().recent_science[0] = 60;
    assert!(ra::tick(&mut w).is_empty());
    w.turn += 1;
    w.civ_mut(CivId(0)).unwrap().recent_science[1] = 30;
    w.civ_mut(CivId(1)).unwrap().recent_science[1] = 72;
    assert!(matches!(ra::tick(&mut w).as_slice(), [TurnEvent::ResearchAgreementCompleted { .. }]));
    assert_eq!(w.civ(CivId(0)).unwrap().science_from_agreements, 120);
    assert_eq!(w.civ(CivId(1)).unwrap().science_from_agreements, 120);
    assert!(ra::tick(&mut w).is_empty());
    assert_eq!(ra::take_science(&mut w, &r, CivId(0)), 0);
    assert_eq!(w.civ(CivId(0)).unwrap().science_from_agreements, 120);
    w.civ_mut(CivId(0)).unwrap().researching = r.technologies.id("Agriculture");
    assert_eq!(ra::take_science(&mut w, &r, CivId(0)), 20);
    assert_eq!(ra::take_science(&mut w, &r, CivId(0)), 0);
    w.civ_mut(CivId(1)).unwrap().researching = r.technologies.id("Agriculture");
    w.civ_mut(CivId(1)).unwrap().uniques.insert(Unique::parse("Science gained from research agreements [50]%"));
    assert!(w.civ(CivId(1)).unwrap().uniques.has(UniqueType::ScienceFromResearchAgreements));
    assert_eq!(ra::take_science(&mut w, &r, CivId(1)), 30);
}

#[test]
fn war_cancels_embassies_friendship_and_unmatured_science_without_refund() {
    let (r, mut w) = setup();
    establish(&mut w, &r);
    accept(&mut w, &r, CivId(0), CivId(1), &Terms::ResearchAgreement, TurnModifiers::default()).unwrap();
    w.civ_mut(CivId(0)).unwrap().recent_science[0] = 90;
    w.civ_mut(CivId(1)).unwrap().recent_science[0] = 90;
    ra::tick(&mut w);
    w.diplomacy.declare_war(CivId(0), CivId(1));
    let pact = w.diplomacy.pact(CivId(0), CivId(1)).unwrap();
    assert!(pact.research.is_none());
    assert_eq!(pact.friendship_turns, 0);
    assert_eq!(pact.embassies, [false, false]);
    assert!(ra::tick(&mut w).is_empty());
    assert_eq!(w.civ(CivId(0)).unwrap().science_from_agreements, 0);
    assert_eq!(w.civ(CivId(0)).unwrap().gold, 750);
    assert_eq!(w.diplomacy.attitude(CivId(1), CivId(0)).sentiments[&Sentiment::BetrayedFriendship], Fixed::whole(-40));
}

#[test]
fn ai_signs_research_with_friends_but_automatic_rounds_never_sign_for_humans() {
    let (r, mut w) = setup();
    for (a, b) in [(CivId(0), CivId(1)), (CivId(1), CivId(0))] {
        w.diplomacy.attitude_mut(a, b).set(Sentiment::SharedEnemy, Fixed::whole(50));
    }
    let before = w.clone();
    assert!(ra::automate(&mut w, &r, TurnModifiers::default(), false).is_empty());
    assert_eq!(w, before);
    for c in &mut w.civs {
        c.is_ai = true;
    }
    let events = ra::automate(&mut w, &r, TurnModifiers::default(), false);
    assert!(events.iter().any(|e| matches!(e, TurnEvent::ResearchAgreementSigned { .. })));
    assert!(w.diplomacy.pact(CivId(0), CivId(1)).unwrap().research.is_some());
}

#[cfg(feature = "save")]
#[test]
fn active_and_pending_research_agreements_roundtrip_and_invalid_timer_is_rejected() {
    let (r, mut w) = setup();
    establish(&mut w, &r);
    accept(&mut w, &r, CivId(0), CivId(1), &Terms::ResearchAgreement, TurnModifiers::default()).unwrap();
    w.civ_mut(CivId(0)).unwrap().recent_science[0] = 13;
    w.civ_mut(CivId(1)).unwrap().recent_science[0] = 23;
    ra::tick(&mut w);
    w.civ_mut(CivId(0)).unwrap().science_from_agreements = 19;
    let text = civ_core::save::encode(&w, &r).unwrap();
    let mut restored = civ_core::save::decode(&text, &r).unwrap();
    assert_eq!(w, restored);
    assert_eq!(ra::tick(&mut w), ra::tick(&mut restored));
    assert_eq!(w, restored);
    // Deserialize through the ordinary serde shape, then structural validator.
    let mut json = serde_json::to_value(&restored).unwrap();
    json["diplomacy"]["pacts"][0][1]["research"]["turns_left"] = 0.into();
    let invalid: World = serde_json::from_value(json).unwrap();
    assert!(civ_core::save::validate(&invalid, &r).is_err());
}

#[test]
fn embassy_remembers_capital_without_exposing_units_or_live_city_changes() {
    let (r, mut w) = setup();
    let capital = w.civ(CivId(1)).unwrap().cities[0];
    let unit = w.place_unit(civ_core::world::Unit::new(r.unit("Warrior").unwrap(), CivId(1), TileId(321), 2));
    civ_core::world::turn::update_visibility_in(&mut w, &r);
    let before = w.visibility.visible_tiles(CivId(0));
    accept(
        &mut w,
        &r,
        CivId(1),
        CivId(0),
        &Terms::Trade { give: vec![Item::Embassy], get: vec![] },
        TurnModifiers::default(),
    )
    .unwrap();
    assert_eq!(w.visibility.visible_tiles(CivId(0)), before);
    assert!(!civ_core::world::fog::can_see_unit(&w, &r, CivId(0), unit));
    let seen = w.visibility.known_cities(CivId(0)).find(|c| c.id == capital).unwrap().clone();
    assert_eq!(seen.owner, CivId(1));
    w.cities.get_mut(capital).unwrap().population += 1;
    civ_core::world::fog::capture_observations(&mut w, &r);
    assert_eq!(w.visibility.known_cities(CivId(0)).find(|c| c.id == capital), Some(&seen));
}
