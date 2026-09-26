use civ_core::world::city_state_actions as cs;
use civ_core::world::construction::CostModifiers;
use civ_core::world::{City, CivId, Civilization, TileGrid, TileId, TurnEvent, TurnModifiers, Unit, World};
use civ_core::{ruleset::Ruleset, stats::Fixed, UniqueType};
const MAJOR: CivId = CivId(0);
const OTHER: CivId = CivId(1);
const CS: CivId = CivId(2);
fn setup(kind: &str) -> (Ruleset, World) {
    let r = Ruleset::load_dir("rules/gods-and-kings").unwrap().unwrap();
    let mut civs = vec![
        Civilization::new(MAJOR, r.nations.id("Austria").unwrap(), false),
        Civilization::new(OTHER, r.nations.id("Rome").unwrap(), false),
        Civilization::new(CS, r.nations.id("Geneva").unwrap(), true),
    ];
    civs[2].city_state_type = r.city_state_types.id(kind);
    let mut w = World::new(TileGrid::new(30, 10, r.terrain("Grassland").unwrap()), civs);
    for (c, t) in [(MAJOR, TileId(31)), (OTHER, TileId(56)), (CS, TileId(166))] {
        w.found_city_in(&r, City::found("Capital", c, t));
        civ_core::world::policies::rebuild_uniques(&mut w, &r, c);
    }
    for c in &mut w.civs {
        c.techs.insert(r.technologies.id("Agriculture").unwrap());
    }
    w.diplomacy.meet(MAJOR, CS);
    w.diplomacy.meet(OTHER, CS);
    w.diplomacy.set_influence(CS, MAJOR, Fixed::whole(100));
    cs::initialize(&mut w, &r);
    cs::sync_alliances(&mut w);
    (r, w)
}
fn round(w: &mut World, r: &Ruleset) -> Vec<TurnEvent> {
    w.turn += 1;
    cs::end_round(w, r, CostModifiers::default(), false)
}

#[test]
fn mercantile_identity_is_stable_and_supplies_only_its_ally() {
    let (r, mut w) = setup("Mercantile");
    let resource = cs::unique_luxury(&w, CS).unwrap();
    assert!(r.resources[resource].uniques.iter().any(|u| u.unique_type() == Some(UniqueType::CityStateOnlyResource)));
    let before = w.clone();
    cs::initialize(&mut w, &r);
    assert_eq!(w, before);
    assert_eq!(civ_core::world::resources::available_stock(&w, &r, MAJOR).get(&resource), Some(&1));
    assert_eq!(civ_core::world::resources::available_stock(&w, &r, OTHER).get(&resource), None);
    w.diplomacy.set_influence(CS, OTHER, Fixed::whole(120));
    assert_eq!(civ_core::world::resources::available_stock(&w, &r, MAJOR).get(&resource), None);
    assert_eq!(civ_core::world::resources::available_stock(&w, &r, OTHER).get(&resource), Some(&1));
}

#[test]
fn military_clock_gifts_a_legal_unit_with_city_training_and_siam_experience() {
    let (r, mut w) = setup("Militaristic");
    let capital = w.civ(CS).unwrap().cities[0];
    let b = r.buildings.id("Barracks").unwrap();
    w.cities.get_mut(capital).unwrap().built.insert(b);
    civ_core::world::city::rebuild_city_uniques(&r, w.cities.get_mut(capital).unwrap());
    // Use the actual Siam unique, preserving Austria's harmless unrelated trait.
    for u in &r.nations[r.nations.id("Siam").unwrap()].uniques {
        if u.unique_type() == Some(UniqueType::CityStateGiftedUnitsStartWithXp) {
            w.civ_mut(MAJOR).unwrap().uniques.insert(u.clone());
        }
    }
    w.civ_mut(CS).unwrap().city_state.military_gifts.insert(MAJOR, 1);
    let events = round(&mut w, &r);
    let id = events
        .iter()
        .find_map(|e| if let TurnEvent::CityStateGift { to: MAJOR, unit, .. } = e { Some(*unit) } else { None })
        .unwrap();
    let u = w.units.get(id).unwrap();
    assert_eq!(u.owner, MAJOR);
    assert!(!u.is_civilian(&r));
    assert!(u.is_land_unit(&r));
    assert_eq!(u.xp, 25);
    let left = w.civ(CS).unwrap().city_state.military_gifts[&MAJOR];
    assert!((16..=18).contains(&left));
    w.diplomacy.set_influence(CS, MAJOR, Fixed::ZERO);
    round(&mut w, &r);
    assert!(!w.civ(CS).unwrap().city_state.military_gifts.contains_key(&MAJOR));
}

#[test]
fn common_enemy_accelerates_military_gifts() {
    let (r, mut w) = setup("Militaristic");
    let unique = r
        .policies
        .iter()
        .flat_map(|(_, p)| &p.uniques)
        .find(|u| u.unique_type() == Some(UniqueType::CityStateMoreGiftedUnits))
        .unwrap()
        .clone();
    w.civ_mut(MAJOR).unwrap().uniques.insert(unique);
    w.diplomacy.declare_war(MAJOR, OTHER);
    w.diplomacy.declare_war(CS, OTHER);
    w.civ_mut(CS).unwrap().city_state.military_gifts.insert(MAJOR, 2);
    assert!(round(&mut w, &r).iter().any(|e| matches!(e, TurnEvent::CityStateGift { to: MAJOR, .. })));
}

#[test]
fn patronage_has_one_recipient_clock_and_pauses_without_allies() {
    let (r, mut w) = setup("Cultured");
    let unique = r
        .policies
        .iter()
        .flat_map(|(_, p)| &p.uniques)
        .find(|u| u.unique_type() == Some(UniqueType::CityStateCanGiftGreatPeople))
        .unwrap()
        .clone();
    w.civ_mut(MAJOR).unwrap().uniques.insert(unique);
    w.civ_mut(MAJOR).unwrap().city_state.great_person_gift = Some(1);
    w.diplomacy.set_influence(CS, MAJOR, Fixed::ZERO);
    round(&mut w, &r);
    assert_eq!(w.civ(MAJOR).unwrap().city_state.great_person_gift, Some(1));
    w.diplomacy.set_influence(CS, MAJOR, Fixed::whole(100));
    let events = round(&mut w, &r);
    let id = events
        .iter()
        .find_map(|e| if let TurnEvent::CityStateGift { to: MAJOR, unit, .. } = e { Some(*unit) } else { None })
        .unwrap();
    let base = &r.units[w.units.get(id).unwrap().base];
    assert!(base.uniques.iter().any(|u| u.unique_type() == Some(UniqueType::GreatPerson)));
    assert!(!base.uniques.iter().any(|u| u.unique_type() == Some(UniqueType::MayFoundReligion)));
    assert!((37..=43).contains(&w.civ(MAJOR).unwrap().city_state.great_person_gift.unwrap()));
}

#[test]
fn marriage_requires_continuous_alliance_and_preserves_the_city_and_troops() {
    let (r, mut w) = setup("Cultured");
    w.civ_mut(MAJOR).unwrap().gold = 10000;
    let capital = w.civ(CS).unwrap().cities[0];
    w.cities.get_mut(capital).unwrap().population = 7;
    w.cities.get_mut(capital).unwrap().health = 143;
    let mut soldier = Unit::new(r.unit("Warrior").unwrap(), CS, TileId(167), 2);
    soldier.health = 49;
    soldier.movement_left = Fixed::ONE;
    let id = w.place_unit(soldier.clone());
    for _ in 0..4 {
        round(&mut w, &r);
    }
    assert!(!cs::can_marry(&w, &r, MAJOR, CS, CostModifiers::default()));
    w.diplomacy.set_influence(CS, OTHER, Fixed::whole(110));
    cs::sync_alliances(&mut w);
    w.diplomacy.set_influence(CS, MAJOR, Fixed::whole(120));
    cs::sync_alliances(&mut w);
    for _ in 0..4 {
        round(&mut w, &r);
    }
    assert!(!cs::can_marry(&w, &r, MAJOR, CS, CostModifiers::default()));
    round(&mut w, &r);
    let gold = cs::marriage_cost(&w, &r, CS, CostModifiers::default()).unwrap();
    assert!(gold > 500);
    assert_eq!(gold % 5, 0);
    assert!(civ_core::world::commands::execute(
        &mut w,
        &r,
        MAJOR,
        civ_core::world::commands::Command::BuyCityState { target: CS }
    )
    .is_ok());
    let city = w.cities.get(capital).unwrap();
    assert_eq!(city.owner, MAJOR);
    assert_eq!(city.population, 7);
    assert_eq!(city.health, 143);
    assert!(city.puppet);
    assert!(!city.original_capital);
    let unit = w.units.get(id).unwrap();
    assert_eq!(unit.owner, MAJOR);
    assert_eq!(unit.health, soldier.health);
    assert_eq!(unit.movement_left, soldier.movement_left);
    assert_eq!(w.civ(MAJOR).unwrap().gold, 10000 - gold);
    assert!(w.civ(CS).unwrap().city_state.absorbed);
    assert!(w.civ(CS).unwrap().defeated);
    // Capturing the married city later must never offer a resurrection.
    civ_core::world::conquest::transfer_city(&mut w, &r, capital, OTHER).unwrap();
    w.cities.get_mut(capital).unwrap().puppet = true;
    assert!(!civ_core::world::conquest::can_liberate(&w, capital));
}

#[test]
fn scholasticism_scales_the_allied_city_states_own_income() {
    let (r, mut w) = setup("Cultured");
    let before = civ_core::world::city_states::civ_wide_yields(&w, &r, MAJOR);
    let unique = r
        .policies
        .iter()
        .flat_map(|(_, p)| &p.uniques)
        .find(|u| u.unique_type() == Some(UniqueType::CityStateStatPercent))
        .unwrap()
        .clone();
    w.civ_mut(MAJOR).unwrap().uniques.insert(unique);
    let income = civ_core::world::turn::plan_economy(&w, &r, CS, Default::default(), TurnModifiers::default());
    let after = civ_core::world::city_states::civ_wide_yields(&w, &r, MAJOR);
    assert_eq!(after.science - before.science, Fixed::from_fraction(i64::from(income.science), 4));
}

#[cfg(feature = "save")]
#[test]
fn city_state_clocks_and_identity_roundtrip_and_reject_bad_references() {
    let (r, mut w) = setup("Mercantile");
    w.civ_mut(MAJOR).unwrap().city_state.great_person_gift = Some(29);
    w.civ_mut(CS).unwrap().city_state.allied_turns = 3;
    w.civ_mut(CS).unwrap().city_state.military_gifts.insert(MAJOR, 14);
    let text = civ_core::save::encode(&w, &r).unwrap();
    let mut restored = civ_core::save::decode(&text, &r).unwrap();
    assert_eq!(restored, w);
    assert_eq!(round(&mut restored, &r), round(&mut w, &r));
    assert_eq!(restored, w);
    w.civ_mut(CS).unwrap().city_state.military_gifts.insert(CivId(250), 1);
    assert!(civ_core::save::encode(&w, &r).is_err());
}

#[test]
fn first_contact_rewards_and_capital_memory_do_not_reveal_live_units() {
    let (r, mut w) = setup("Religious");
    let city = w.civ(CS).unwrap().cities[0];
    let centre = w.cities.get(city).unwrap().centre;
    let unit = w.place_unit(Unit::new(r.unit("Warrior").unwrap(), CS, centre, 2));
    let before = w.civ(MAJOR).unwrap().gold;
    let first = cs::on_met(&mut w, &r, CS, MAJOR, true);
    assert_eq!(first.gold, Fixed::whole(30));
    assert_eq!(first.faith, Fixed::whole(4));
    assert_eq!(w.civ(MAJOR).unwrap().gold, before + 30);
    assert_eq!(w.civ(MAJOR).unwrap().faith_for_contests, 4);
    assert!(w.visibility.has_explored(MAJOR, centre));
    assert!(!w.visibility.can_see(MAJOR, centre));
    assert!(w.visibility.known_cities(MAJOR).any(|c| c.id == city));
    assert!(w.grid.neighbours(centre).all(|t| !w.visibility.has_explored(MAJOR, t)));
    assert!(w.units.get(unit).is_some());
    let second = cs::on_met(&mut w, &r, CS, OTHER, false);
    assert_eq!(second.gold, Fixed::whole(15));
    assert_eq!(second.faith, Fixed::whole(4));
    w.civ_mut(MAJOR).unwrap().city_state.minor_attacks = 2;
    assert_eq!(cs::on_met(&mut w, &r, CS, MAJOR, false), Default::default());
}

#[test]
fn repeated_direct_aggression_creates_wary_resting_penalty_and_source_expiry() {
    let (r, mut w) = setup("Cultured");
    w.civ_mut(MAJOR).unwrap().city_state.minor_attacks = 3;
    w.civ_mut(CS).unwrap().city_state.personality = cs::Personality::Neutral;
    civ_core::world::quests::on_city_state_attacked(&mut w, &r, CS, MAJOR);
    assert_eq!(w.civ(MAJOR).unwrap().city_state.minor_attacks, 4);
    assert_eq!(civ_core::world::city_states::resting_point_in(&w, &r, CS, MAJOR), Fixed::whole(-20));
    assert_eq!(civ_core::world::city_states::drift_in(&w, &r, CS, MAJOR), Fixed::whole(-2));
    w.turn = 10;
    assert_eq!(civ_core::world::city_states::resting_point_in(&w, &r, CS, MAJOR), Fixed::ZERO);
}

#[test]
fn global_contests_start_together_only_with_enough_participants_and_scale_duration() {
    let (mut r, mut w) = setup("Cultured");
    let data = r.quests.by_name("Contest Culture").unwrap().clone();
    r.quests = Default::default();
    let id = r.quests.insert(data.name.clone(), data);
    w.turn = 50;
    w.civ_mut(CS).unwrap().quests.global_seeded = true;
    w.civ_mut(CS).unwrap().quests.next_turn = 50;
    assert!(civ_core::world::quests::tick_with_speed(&mut w, &r, Fixed::whole(2)).is_empty());
    let c = CivId(3);
    w.civs.push(Civilization::new(c, r.nations.id("Greece").unwrap(), false));
    w.visibility = civ_core::world::fog::Visibility::new(w.grid.len(), 4).into();
    w.found_city_in(&r, City::found("Athens", c, TileId(286)));
    w.diplomacy.meet(CS, c);
    let events = civ_core::world::quests::tick_with_speed(&mut w, &r, Fixed::whole(2));
    assert_eq!(events.len(), 3);
    let active = &w.civ(CS).unwrap().quests.active;
    assert_eq!(active.len(), 3);
    assert!(active.iter().all(|q| q.quest == id && q.assigned_on == 50 && q.expires_on == 110));
    w.civ_mut(MAJOR).unwrap().add_culture(10);
    w.civ_mut(OTHER).unwrap().add_culture(10);
    w.civ_mut(c).unwrap().add_culture(100);
    w.diplomacy.declare_war(c, CS);
    w.turn = 110;
    let events = civ_core::world::quests::tick(&mut w, &r);
    assert_eq!(
        events.iter().filter(|e| matches!(e, civ_core::world::quests::QuestEvent::Completed { .. })).count(),
        2,
        "removed wartime contestant cannot beat either tied eligible major"
    );
}

#[test]
fn combat_assistance_counts_military_kills_once_and_nearby_barbarians_only() {
    let (r, mut w) = setup("Cultured");
    w.diplomacy.set_influence(CS, MAJOR, Fixed::ZERO);
    let barb = CivId(3);
    let mut c = Civilization::new(barb, r.nations.id("Barbarians").unwrap(), true);
    c.is_barbarian = true;
    w.civs.push(c);
    let centre = w.cities.get(w.civ(CS).unwrap().cities[0]).unwrap().centre;
    civ_core::world::quests::on_unit_killed(&mut w, &r, MAJOR, barb, centre, false);
    assert_eq!(w.diplomacy.influence(CS, MAJOR), Fixed::ZERO);
    civ_core::world::quests::on_unit_killed(&mut w, &r, MAJOR, barb, centre, true);
    assert_eq!(w.diplomacy.influence(CS, MAJOR), Fixed::whole(12));
    civ_core::world::quests::on_unit_killed(&mut w, &r, MAJOR, barb, TileId(0), true);
    assert_eq!(w.diplomacy.influence(CS, MAJOR), Fixed::whole(12));
    w.diplomacy.declare_war(CS, OTHER);
    civ_core::world::quests::on_city_state_attacked(&mut w, &r, CS, OTHER);
    for _ in 0..3 {
        civ_core::world::quests::on_unit_killed(&mut w, &r, MAJOR, OTHER, centre, true);
    }
    assert_eq!(w.diplomacy.influence(CS, MAJOR), Fixed::whole(112));
    assert!(!w.civ(CS).unwrap().quests.assistance.contains_key(&OTHER));
    civ_core::world::quests::on_unit_killed(&mut w, &r, MAJOR, OTHER, centre, true);
    assert_eq!(w.diplomacy.influence(CS, MAJOR), Fixed::whole(112));
}

#[test]
fn fractional_quest_weights_are_preserved() {
    let (r, _) = setup("Cultured");
    assert_eq!(r.quests.by_name("Route").unwrap().weight_for_city_state_type["Hostile"], Fixed::permille(200));
}
