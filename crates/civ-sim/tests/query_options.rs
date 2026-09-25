use civ_core::arena::Id;
use civ_core::ruleset::Ruleset;
use civ_core::stats::Stat;
use civ_core::world::city::City;
use civ_core::world::commands::{self, Command};
use civ_core::world::construction::{self, Construction, CostModifiers};
use civ_core::world::entities::Civilization;
use civ_core::world::grid::TileGrid;
use civ_core::world::{combat, policies, purchase, research, CivId, TurnModifiers, Unit, World};
use civ_core::{Unique, UniqueType};
use civ_sim::protocol::ConstructionRef;
use civ_sim::queries::{self, Availability, Refusal, UnitActionKind};

const ME: CivId = CivId(0);
const OTHER: CivId = CivId(1);

fn fixture() -> (World, Ruleset, Id<Unit>, Id<City>, Id<City>) {
    let r = civ_sim::content::load().unwrap();
    let civs = ["Rome", "Persia"]
        .into_iter()
        .enumerate()
        .map(|(i, nation)| Civilization::new(CivId(i as u8), r.nations.id(nation).unwrap(), false))
        .collect();
    let mut w = World::new(TileGrid::new(20, 8, r.terrain("Grassland").unwrap()), civs);
    let home = w.grid.id_at(2, 3).unwrap();
    let away = w.grid.id_at(14, 3).unwrap();
    let city = w.found_city(City::found("Home", ME, home));
    let enemy_city = w.found_city(City::found("Hidden", OTHER, away));
    let unit = w.place_unit(Unit::new(r.units.id("Warrior").unwrap(), ME, home, 2));
    w.civ_mut(ME).unwrap().gold = 1_000;
    w.civ_mut(ME).unwrap().faith.stored = 500;
    w.civ_mut(ME)
        .unwrap()
        .techs
        .insert(r.technologies.id("Agriculture").unwrap());
    (w, r, unit, city, enemy_city)
}

#[test]
fn foreign_and_missing_entities_both_have_no_options() {
    let (mut w, r, unit, city, enemy_city) = fixture();
    let enemy_tile = w.cities.get(enemy_city).unwrap().centre;
    let enemy_unit = w.place_unit(Unit::new(
        r.units.id("Warrior").unwrap(),
        OTHER,
        enemy_tile,
        2,
    ));
    assert!(queries::unit_options(&w, &r, ME, unit.index(), unit.generation()).is_some());
    assert!(queries::city_options(&w, &r, ME, city.index(), city.generation()).is_some());
    assert_eq!(
        queries::unit_options(&w, &r, ME, enemy_unit.index(), enemy_unit.generation()),
        None
    );
    assert_eq!(
        queries::city_options(&w, &r, ME, enemy_city.index(), enemy_city.generation()),
        None
    );
    assert_eq!(
        queries::unit_options(&w, &r, ME, unit.index(), unit.generation() + 1),
        None
    );
    assert_eq!(queries::city_options(&w, &r, ME, u32::MAX, 1), None);
}

#[test]
fn purchase_quotes_use_the_same_cost_as_execution() {
    let (mut w, r, _, city, _) = fixture();
    w.civ_mut(ME).unwrap().uniques.insert(Unique::parse(
        "May buy [Warrior] units for [120] [Faith] [in all cities]",
    ));
    let warrior = r.units.id("Warrior").unwrap();
    let options = queries::city_options(&w, &r, ME, city.index(), city.generation()).unwrap();
    let choice = options
        .choices
        .iter()
        .find(|c| {
            c.construction
                == ConstructionRef::Unit {
                    id: warrior.index(),
                }
        })
        .unwrap();
    for (stat, quote) in [
        (Stat::Gold, choice.gold.as_ref().unwrap()),
        (Stat::Faith, choice.faith.as_ref().unwrap()),
    ] {
        let expected = purchase::cost(
            &w,
            &r,
            city,
            Construction::Unit(warrior),
            stat,
            CostModifiers::default(),
        )
        .unwrap();
        assert_eq!(quote.amount, Some(expected.to_string()));
        assert_eq!(quote.affordable, Some(true));
        assert_eq!(quote.availability, Availability::NeedsConfirmation);
    }
    assert_eq!(
        choice.faith.as_ref().unwrap().amount.as_deref(),
        Some("120")
    );
}

#[test]
fn multi_city_quote_matches_the_actual_command_debit() {
    let (mut w, mut r, _, city, _) = fixture();
    let second = w.grid.id_at(6, 3).unwrap();
    w.found_city(City::found("Second", ME, second));
    let monument = r.buildings.id("Monument").unwrap();
    r.buildings
        .get_mut(monument)
        .unwrap()
        .uniques
        .push(Unique::parse("Cost increases by [30] per owned city"));
    let choices = queries::city_options(&w, &r, ME, city.index(), city.generation()).unwrap();
    let choice = choices
        .choices
        .iter()
        .find(|c| {
            c.construction
                == ConstructionRef::Building {
                    id: monument.index(),
                }
        })
        .unwrap();
    let quoted: i32 = choice
        .gold
        .as_ref()
        .unwrap()
        .amount
        .as_ref()
        .unwrap()
        .parse()
        .unwrap();
    assert_eq!(
        choice.production_cost,
        Some(r.buildings[monument].cost.unwrap() + 60)
    );
    let stale_single_city = civ_core::world::construction::production_cost(
        &r,
        Construction::Building(monument),
        CostModifiers::default(),
    )
    .unwrap();
    assert!(choice.production_cost.unwrap() > stale_single_city);
    let before = w.civ(ME).unwrap().gold;
    commands::execute(
        &mut w,
        &r,
        ME,
        Command::Purchase {
            city,
            what: Construction::Building(monument),
            currency: Stat::Gold,
        },
    )
    .unwrap();
    assert_eq!(before - w.civ(ME).unwrap().gold, quoted);
    assert!(w.cities.get(city).unwrap().built.contains(&monument));
}

#[test]
fn hidden_wonders_units_resources_and_foreign_economy_do_not_change_options() {
    let (mut w, r, unit, city, enemy_city) = fixture();
    w.civ_mut(ME).unwrap().techs.extend(r.technologies.ids());
    // Wonders stand on a tile of their city, so give it some.
    let home = w.cities.get(city).unwrap().centre;
    for t in w.grid.within(home, 1) {
        w.claim_tile(city, t);
    }
    let expected_city = queries::city_options(&w, &r, ME, city.index(), city.generation());
    let expected_unit = queries::unit_options(&w, &r, ME, unit.index(), unit.generation());
    let expected_civ = queries::civ_options(&w, &r, ME);
    let wonder = r
        .buildings
        .iter()
        .find(|(_, b)| b.is_wonder == Some(true) && b.required_building.is_none())
        .map(|(id, _)| id)
        .unwrap();
    assert!(expected_city
        .as_ref()
        .unwrap()
        .choices
        .iter()
        .any(|c| c.construction == ConstructionRef::Building { id: wonder.index() }));
    w.cities.get_mut(enemy_city).unwrap().built.insert(wonder);
    w.cities.get_mut(enemy_city).unwrap().population = 30;
    w.civ_mut(OTHER).unwrap().techs.extend(r.technologies.ids());
    w.civ_mut(OTHER).unwrap().gold = 999_999;
    let hidden = w.cities.get(enemy_city).unwrap().centre;
    w.place_unit(Unit::new(
        r.units.id("Swordsman").unwrap(),
        OTHER,
        hidden,
        2,
    ));
    w.grid.resource[hidden.0 as usize] = r.resources.id("Iron");
    assert_eq!(
        queries::city_options(&w, &r, ME, city.index(), city.generation()),
        expected_city
    );
    assert_eq!(
        queries::unit_options(&w, &r, ME, unit.index(), unit.generation()),
        expected_unit
    );
    assert_eq!(queries::civ_options(&w, &r, ME), expected_civ);
}

#[test]
fn potentially_private_conditional_prices_are_explicitly_unknown() {
    let (mut w, r, _, city, _) = fixture();
    let conditional = Unique::parse("[Gold] cost of purchasing items in cities [-25]% <if no other Civilization has researched this>");
    assert!(!conditional.conditionals.is_empty());
    w.civ_mut(ME).unwrap().uniques.insert(conditional);
    let expected = queries::city_options(&w, &r, ME, city.index(), city.generation()).unwrap();
    let warrior = expected
        .choices
        .iter()
        .find(|c| c.name == "Warrior")
        .unwrap();
    assert_eq!(warrior.gold.as_ref().unwrap().amount, None);
    assert_eq!(warrior.gold.as_ref().unwrap().affordable, None);
    assert_eq!(
        warrior.gold.as_ref().unwrap().availability,
        Availability::NeedsConfirmation
    );
    w.civ_mut(OTHER).unwrap().techs.extend(r.technologies.ids());
    assert_eq!(
        queries::city_options(&w, &r, ME, city.index(), city.generation()).unwrap(),
        expected
    );
}

/// A finished policy branch adds a conditional price to the civ's uniques:
/// `Commerce Complete` sells Magnates for faith, from the Age of Steam, where
/// the majority religion is a major one. That hides the one price it names
/// and nothing else: every build keeps its production cost, so it can still
/// be queued, and every price that was quoted before is quoted after, as
/// `purchase` would charge it.
#[test]
fn a_finished_policy_branch_hides_only_the_price_it_names() {
    let (mut w, r, _, city, _) = fixture();
    // Commerce opens in the Age of Paper; know that era's techs and those before.
    let paper = r.eras.by_name("Age of Paper").unwrap().era_number;
    let known: Vec<_> = r
        .technologies
        .iter()
        .filter(|(_, t)| {
            t.era(&r)
                .and_then(|e| r.eras.get(e))
                .is_some_and(|e| e.era_number <= paper)
        })
        .map(|(id, _)| id)
        .collect();
    w.civ_mut(ME).unwrap().techs.extend(known);
    let before = queries::city_options(&w, &r, ME, city.index(), city.generation()).unwrap();
    let finisher = r.policies.id("Commerce Complete").unwrap();
    let branch = r.policies[finisher].branch;
    let members = r
        .policies
        .iter()
        .filter(|(id, p)| p.branch == branch && !policies::is_finisher(&r, *id))
        .count();
    w.civ_mut(ME).unwrap().pending_rewards.free_policies = members as _;
    while let Some(policy) = r.policies.ids().find(|id| {
        r.policies[*id].branch == branch && policies::is_adoptable(&w, &r, ME, *id).is_ok()
    }) {
        commands::execute(&mut w, &r, ME, Command::AdoptPolicy { policy }).unwrap();
    }
    let owner = w.civ(ME).unwrap();
    assert!(owner.adopted_policies.contains(&finisher));
    assert!(owner.uniques.all().any(|u| {
        u.unique_type() == Some(UniqueType::BuyUnitsIncreasingCost)
            && u.params.first().is_some_and(|p| p == "Magnate")
            && !u.conditionals.is_empty()
    }));

    let after = queries::city_options(&w, &r, ME, city.index(), city.generation()).unwrap();
    let modifiers = TurnModifiers::for_world(&w, &r).costs;
    let magnate = r.units.id("Magnate").unwrap();
    let what = |c: &ConstructionRef| match *c {
        ConstructionRef::Building { id } => {
            Construction::Building(r.buildings.ids().find(|b| b.index() == id).unwrap())
        }
        ConstructionRef::Unit { id } => {
            Construction::Unit(r.units.ids().find(|u| u.index() == id).unwrap())
        }
    };
    assert!(after.choices.iter().any(|c| c.refusal.is_none()));
    for choice in &after.choices {
        let what = what(&choice.construction);
        if choice.refusal.is_none() {
            assert!(
                choice.production_cost.is_some(),
                "{} can be queued",
                choice.name
            );
            assert_eq!(
                choice.production_cost,
                construction::production_cost_in(&w, &r, city, what, modifiers),
                "{}",
                choice.name
            );
        }
        if what == Construction::Unit(magnate) {
            continue;
        }
        let was = before
            .choices
            .iter()
            .find(|c| c.construction == choice.construction);
        let was = was.unwrap_or_else(|| panic!("{} is offered only after the branch", choice.name));
        for (stat, quote, old) in [
            (Stat::Gold, &choice.gold, &was.gold),
            (Stat::Faith, &choice.faith, &was.faith),
        ] {
            // Hidden before the branch too: the item's own price has a
            // condition (a Missionary is sold where the majority religion is
            // a major one).
            if old.as_ref().is_some_and(|q| q.amount.is_none()) {
                assert_eq!(quote, old, "{}'s {stat:?} price", choice.name);
                continue;
            }
            assert_eq!(
                quote.as_ref().map(|q| q.amount.clone()),
                purchase::cost(&w, &r, city, what, stat, modifiers).map(|c| Some(c.to_string())),
                "{}'s {stat:?} price is quoted exactly",
                choice.name
            );
        }
    }
    let monument = after.choices.iter().find(|c| c.name == "Monument").unwrap();
    assert!(monument.production_cost.is_some());
    assert!(monument.gold.as_ref().unwrap().amount.is_some());
    // The Magnate is bought, never built. Its faith price may hinge on the
    // condition, so it is offered unquoted; it has no gold price at all.
    let bought = after
        .choices
        .iter()
        .find(|c| {
            c.construction
                == ConstructionRef::Unit {
                    id: magnate.index(),
                }
        })
        .unwrap();
    assert!(!before
        .choices
        .iter()
        .any(|c| c.construction == bought.construction));
    assert_eq!(bought.production_cost, None);
    assert_eq!(bought.refusal, Some(Refusal::PurchaseOnly));
    assert_eq!(bought.gold, None);
    let faith = bought.faith.as_ref().unwrap();
    assert_eq!((faith.amount.as_deref(), faith.affordable), (None, None));
    assert_eq!(faith.availability, Availability::NeedsConfirmation);
}

#[test]
fn own_research_policies_and_promotions_use_core_eligibility() {
    let (mut w, r, unit, _, _) = fixture();
    let before = queries::unit_options(&w, &r, ME, unit.index(), unit.generation()).unwrap();
    assert!(before.promotions.is_empty());
    assert!(before
        .actions
        .iter()
        .any(|a| a.action == UnitActionKind::Fortify && a.availability == Availability::Available));
    w.units.get_mut(unit).unwrap().xp = 100;
    let upgraded = queries::unit_options(&w, &r, ME, unit.index(), unit.generation()).unwrap();
    let promotion_ids: Vec<_> = combat::available_promotions(&r, w.units.get(unit).unwrap())
        .into_iter()
        .map(|p| p.index())
        .collect();
    assert!(!promotion_ids.is_empty());
    assert_eq!(
        upgraded.promotions.iter().map(|p| p.id).collect::<Vec<_>>(),
        promotion_ids
    );
    assert!(upgraded.upgrade.is_some());
    let options = queries::civ_options(&w, &r, ME);
    let owner = w.civ(ME).unwrap();
    let available = research::available_techs(&r, owner);
    assert_eq!(
        options.research.iter().map(|t| t.id).collect::<Vec<_>>(),
        available.iter().map(|t| t.index()).collect::<Vec<_>>()
    );
    for tech in options.research {
        let id = available.iter().find(|id| id.index() == tech.id).unwrap();
        assert_eq!(
            Some(tech.science_cost),
            research::tech_cost(
                &r,
                *id,
                owner.cities.len(),
                research::ScienceModifiers::default()
            )
        );
    }
    assert_eq!(
        options.next_policy_cost,
        policies::policy_cost(0, 1, policies::PolicyModifiers::default())
    );
    assert!(!options.policies.is_empty());
    assert!(options.policies.iter().all(|p| !p.affordable));
    w.civ_mut(ME).unwrap().culture_stored = 1_000;
    assert!(queries::civ_options(&w, &r, ME)
        .policies
        .iter()
        .all(|p| p.affordable));
}

#[test]
fn querying_never_changes_authoritative_state() {
    let (w, r, unit, city, _) = fixture();
    let before = serde_json::to_value(&w).unwrap();
    queries::unit_options(&w, &r, ME, unit.index(), unit.generation());
    queries::city_options(&w, &r, ME, city.index(), city.generation());
    queries::civ_options(&w, &r, ME);
    assert_eq!(serde_json::to_value(&w).unwrap(), before);
}
