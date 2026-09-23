//! What a great person does once born (`UnitActionsGreatPerson`,
//! `UnitActionsFromUniques`).
//!
//! Each is spent in the doing:
//!
//! - a **Scientist** pours the last eight turns' science into the current
//!   research (`TechManager.getScienceFromGreatScientist`);
//! - an **Engineer**, standing in a city building a building, adds
//!   `(300 + 30 × population) × speed` hammers — stopping one short of
//!   finishing it, so it completes on the city's own turn;
//! - a **Merchant**, on a city-state's land and not at war with it, brings
//!   home `(350 + 50 × era) × speed` gold and 30 influence;
//! - an **Artist** starts an eight-turn golden age;
//! - and any of them may instead build their landmark improvement — Academy,
//!   Manufactory, Customs house, Landmark, Holy site, Citadel — on the spot.
//!
//! Which a person can do is read from its uniques, not its name, so a mod's
//! great person works if it carries the same ones.

use crate::arena::Id;
use crate::ruleset::{Ruleset, TileImprovement};
use crate::stats::Fixed;
use crate::world::construction::{Construction, CostModifiers};
use crate::world::entities::Unit;
use crate::world::grid::{CivId, TileId};
use crate::world::research::ScienceModifiers;
use crate::world::world::World;
use crate::UniqueType;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    HurryResearch,
    HurryProduction,
    TradeMission,
    GoldenAge,
    Construct(Id<TileImprovement>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActionRejection {
    NoSuchUnit,
    /// The unit has no unique granting this.
    NotAble,
    NothingResearched,
    CannotBeHurried,
    NotInOwnCity,
    NotABuilding,
    /// It would already finish; there is nothing to hurry.
    NothingToAdd,
    NotACityState,
    AtWar,
    CannotBuildHere,
}

impl std::fmt::Display for ActionRejection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}

impl std::error::Error for ActionRejection {}

/// What came of it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Science { amount: i32, learned: Option<Id<crate::ruleset::Technology>> },
    Production { city: Id<crate::world::city::City>, hammers: i32 },
    TradeMission { city_state: CivId, gold: i32, influence: i32 },
    GoldenAge { turns: u32 },
    Constructed { tile: TileId, improvement: Id<TileImprovement> },
}

/// Speed modifiers the actions scale by.
#[derive(Debug, Clone, Copy, Default)]
pub struct Scaling {
    pub costs: CostModifiers,
    pub science: ScienceModifiers,
}

fn unique_param<'a>(r: &'a Ruleset, unit: &Unit, t: UniqueType) -> Option<Option<&'a str>> {
    let b = r.units.get(unit.base)?;
    b.uniques.iter().find(|u| u.unique_type() == Some(t)).map(|u| u.params.first().map(String::as_str))
}

fn able(r: &Ruleset, unit: &Unit, t: UniqueType) -> bool {
    unique_param(r, unit, t).is_some()
}

/// What this unit's uniques let it do at all, wherever it stands.
pub fn abilities(r: &Ruleset, unit: &Unit) -> Vec<Action> {
    let mut out = Vec::new();
    if able(r, unit, UniqueType::CanHurryResearch) {
        out.push(Action::HurryResearch);
    }
    if able(r, unit, UniqueType::CanSpeedupConstruction) {
        out.push(Action::HurryProduction);
    }
    if able(r, unit, UniqueType::CanTradeWithCityStateForGoldAndInfluence) {
        out.push(Action::TradeMission);
    }
    if able(r, unit, UniqueType::OneTimeEnterGoldenAgeTurns) || able(r, unit, UniqueType::OneTimeEnterGoldenAge) {
        out.push(Action::GoldenAge);
    }
    if let Some(Some(name)) = unique_param(r, unit, UniqueType::ConstructImprovementInstantly) {
        if let Some(id) = r.improvements.id(name) {
            out.push(Action::Construct(id));
        }
    }
    out
}

/// Science a Scientist would give now.
pub fn science_from_scientist(world: &World, civ: CivId, science: ScienceModifiers) -> i32 {
    let sum: i32 = world.civ(civ).map_or(0, |c| c.recent_science.iter().sum());
    (Fixed::whole(sum) * science.speed).floor() as i32
}

/// Hammers an Engineer would add to this city, before the cap.
pub fn hammers_from_engineer(world: &World, city: Id<crate::world::city::City>, costs: CostModifiers) -> i32 {
    let pop = world.cities.get(city).map_or(0, |c| c.population as i32);
    (Fixed::whole(300 + 30 * pop) * costs.speed).floor() as i32
}

/// Gold a Merchant's trade mission would bring.
pub fn gold_from_trade_mission(world: &World, r: &Ruleset, civ: CivId, costs: CostModifiers) -> i32 {
    let era = crate::world::policies::era_number(world, r, civ);
    let mut gold = Fixed::whole(350 + 50 * era) * costs.gold_cost;
    if let Some(c) = world.civ(civ) {
        for u in c.uniques.matching(UniqueType::PercentGoldFromTradeMissions) {
            if let Some(p) = u.params.first().and_then(|p| p.parse::<i32>().ok()) {
                gold = (gold * Fixed::whole(100 + p)).checked_div(Fixed::whole(100)).unwrap_or(gold);
            }
        }
    }
    gold.floor() as i32
}

fn city_centre_at(world: &World, tile: TileId) -> Option<Id<crate::world::city::City>> {
    world.cities.iter().find(|(_, c)| c.centre == tile).map(|(id, _)| id)
}

/// Whether a great improvement may be placed here by `civ`.
///
/// Land, not a city centre, the improvement's terrain, and inside the civ's
/// borders — or just outside them, for one that says so (a Citadel).
pub fn can_place(
    world: &World,
    r: &Ruleset,
    civ: CivId,
    tile: TileId,
    improvement: Id<TileImprovement>,
) -> Result<(), ActionRejection> {
    let Some(imp) = r.improvements.get(improvement) else { return Err(ActionRejection::CannotBuildHere) };
    if !crate::world::improvements::terrain_allows(world, r, civ, tile, improvement)
        || city_centre_at(world, tile).is_some()
    {
        return Err(ActionRejection::CannotBuildHere);
    }
    let fits = imp.terrains_can_be_built_on.iter().any(|t| {
        crate::world::conditionals::tile_matches_in(&world.grid, r, tile, t, Some(civ))
            == crate::world::conditionals::Verdict::True
    });
    if !fits {
        return Err(ActionRejection::CannotBuildHere);
    }
    let owner = world.grid.owner[tile.0 as usize];
    let outside_ok = imp.uniques.iter().any(|u| u.unique_type() == Some(UniqueType::CanBuildJustOutsideBorders));
    let ours = owner == Some(civ);
    let just_outside = owner.is_none()
        && outside_ok
        && world.grid.neighbours(tile).any(|n| world.grid.owner[n.0 as usize] == Some(civ));
    if !(ours || just_outside) {
        return Err(ActionRejection::CannotBuildHere);
    }
    Ok(())
}

/// Whether this unit could do this where it stands.
pub fn can(world: &World, r: &Ruleset, unit: Id<Unit>, action: Action, s: Scaling) -> Result<(), ActionRejection> {
    let u = world.units.get(unit).ok_or(ActionRejection::NoSuchUnit)?;
    if !abilities(r, u).contains(&action) {
        return Err(ActionRejection::NotAble);
    }
    let civ = u.owner;
    match action {
        Action::HurryResearch => {
            let tech = world.civ(civ).and_then(|c| c.researching).ok_or(ActionRejection::NothingResearched)?;
            let hurriable = r
                .technologies
                .get(tech)
                .is_some_and(|t| !t.uniques.iter().any(|u| u.unique_type() == Some(UniqueType::CannotBeHurried)));
            if !hurriable {
                return Err(ActionRejection::CannotBeHurried);
            }
        }
        Action::HurryProduction => {
            let city = city_centre_at(world, u.position)
                .filter(|c| world.cities.get(*c).is_some_and(|c| c.owner == civ))
                .ok_or(ActionRejection::NotInOwnCity)?;
            hurry_amount(world, r, city, s.costs)?;
        }
        Action::TradeMission => {
            let owner = world.grid.owner[u.position.0 as usize];
            let cs = owner.filter(|o| world.civ(*o).is_some_and(|c| c.is_city_state()));
            let cs = cs.ok_or(ActionRejection::NotACityState)?;
            if world.diplomacy.at_war(civ, cs) {
                return Err(ActionRejection::AtWar);
            }
        }
        Action::GoldenAge => {}
        Action::Construct(imp) => {
            if u.has_unique(r, UniqueType::MayFoundReligion)
                && u.religious_uses_left != crate::world::religious_units::initial_uses(r, u)
            {
                return Err(ActionRejection::NotAble);
            }
            can_place(world, r, civ, u.position, imp)?;
        }
    }
    Ok(())
}

fn hurry_amount(
    world: &World,
    r: &Ruleset,
    city: Id<crate::world::city::City>,
    costs: CostModifiers,
) -> Result<i32, ActionRejection> {
    let c = world.cities.get(city).ok_or(ActionRejection::NotInOwnCity)?;
    let Some(&Construction::Building(b)) = c.queue.first() else { return Err(ActionRejection::NotABuilding) };
    let building = r.buildings.get(b).ok_or(ActionRejection::NotABuilding)?;
    if building.uniques.iter().any(|u| u.unique_type() == Some(UniqueType::CannotBeHurried)) {
        return Err(ActionRejection::CannotBeHurried);
    }
    let cost = crate::world::construction::production_cost_in(world, r, city, Construction::Building(b), costs)
        .ok_or(ActionRejection::NotABuilding)?;
    let add = hammers_from_engineer(world, city, costs).min(cost - c.current_production() - 1);
    if add <= 0 {
        return Err(ActionRejection::NothingToAdd);
    }
    Ok(add)
}

/// Do it, spending the unit.
pub fn perform(
    world: &mut World,
    r: &Ruleset,
    unit: Id<Unit>,
    action: Action,
    s: Scaling,
) -> Result<Outcome, ActionRejection> {
    can(world, r, unit, action, s)?;
    let u = world.units.get(unit).ok_or(ActionRejection::NoSuchUnit)?;
    let (civ, tile, base) = (u.owner, u.position, u.base);
    let outcome = match action {
        Action::HurryResearch => {
            let amount = science_from_scientist(world, civ, s.science);
            let income = crate::world::turn::plan_economy(
                world,
                r,
                civ,
                crate::world::CityFocus::None,
                crate::world::turn::TurnModifiers { costs: s.costs, science: s.science, ..Default::default() },
            )
            .science;
            let learned =
                match crate::world::research::apply_science_with_income_in(world, r, civ, amount, income, s.science) {
                    crate::world::research::ResearchEvent::Completed { tech, .. } => Some(tech),
                    _ => None,
                };
            if let Some(tech) = learned {
                crate::world::effects::on_technology_researched(world, r, civ, tech, s.costs.speed);
            }
            Outcome::Science { amount, learned }
        }
        Action::HurryProduction => {
            let city = city_centre_at(world, tile).ok_or(ActionRejection::NotInOwnCity)?;
            let hammers = hurry_amount(world, r, city, s.costs)?;
            if let Some(c) = world.cities.get_mut(city) {
                crate::world::construction::add_production_points(c, hammers);
            }
            Outcome::Production { city, hammers }
        }
        Action::TradeMission => {
            let cs = world.grid.owner[tile.0 as usize].ok_or(ActionRejection::NotACityState)?;
            let gold = gold_from_trade_mission(world, r, civ, s.costs);
            let influence = unique_param(r, u, UniqueType::CanTradeWithCityStateForGoldAndInfluence)
                .flatten()
                .and_then(|p| p.parse::<i32>().ok())
                .unwrap_or(0);
            if let Some(c) = world.civ_mut(civ) {
                c.gold += gold;
            }
            world.diplomacy.add_influence(cs, civ, Fixed::whole(influence));
            Outcome::TradeMission { city_state: cs, gold, influence }
        }
        Action::GoldenAge => {
            let turns = unique_param(r, u, UniqueType::OneTimeEnterGoldenAgeTurns)
                .flatten()
                .and_then(|p| p.parse::<i32>().ok())
                .unwrap_or(crate::world::golden_age::TURNS);
            let turns = crate::world::golden_age::length(world, civ, turns, s.costs.golden_age_length);
            if let Some(c) = world.civ_mut(civ) {
                c.golden_age.turns_left += turns;
            }
            Outcome::GoldenAge { turns }
        }
        Action::Construct(improvement) => {
            construct(world, r, civ, tile, improvement);
            Outcome::Constructed { tile, improvement }
        }
    };
    crate::world::effects::trigger(
        world,
        r,
        civ,
        city_centre_at(world, tile),
        UniqueType::TriggerUponExpendingUnit,
        &r.units[base].name,
        s.costs.speed,
    );
    world.remove_unit(unit);
    Ok(outcome)
}

/// Place a great improvement: clear what can be cleared, and for one that
/// takes the ground around it, take it for the nearest city.
fn construct(world: &mut World, r: &Ruleset, civ: CivId, tile: TileId, improvement: Id<TileImprovement>) {
    let Some(imp) = r.improvements.get(improvement) else { return };
    let has = |t: UniqueType| imp.uniques.iter().any(|u| u.unique_type() == Some(t));
    let i = tile.0 as usize;
    if has(UniqueType::RemovesFeaturesIfBuilt) {
        world.grid.features[i].retain(|f| {
            r.terrains
                .get(*f)
                .is_some_and(|t| t.uniques.iter().any(|u| u.unique_type() == Some(UniqueType::Irremovable)))
        });
    }
    world.grid.improvement[i] = Some(improvement);
    world.grid.improvement_progress[i] = None;
    world.grid.improvement_pillaged[i] = false;
    if has(UniqueType::OneTimeTakeOverTilesInRadius) {
        let nearest = world
            .civ(civ)
            .into_iter()
            .flat_map(|c| c.cities.iter().copied())
            .filter_map(|id| world.cities.get(id).map(|c| (world.grid.distance(c.centre, tile), id)))
            .min();
        if let Some((_, city)) = nearest {
            let ring: Vec<TileId> = world.grid.within(tile, 1);
            for t in ring {
                if city_centre_at(world, t).is_none() {
                    world.claim_tile(city, t);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::city::City;
    use crate::world::entities::Civilization;
    use crate::world::grid::TileGrid;

    fn ruleset() -> Ruleset {
        Ruleset::load_dir("../android/assets/jsons/Civ V - Gods & Kings").expect("readable").expect("loads")
    }

    /// A major civ with one city, and a city-state beside it.
    fn world(r: &Ruleset) -> (World, Id<crate::world::city::City>) {
        let g = TileGrid::new(20, 12, r.terrain("Grassland").unwrap());
        let nation = r.nations.ids().next().unwrap();
        let mut cs = Civilization::new(CivId(1), nation, true);
        cs.city_state_type = r.city_state_types.ids().next();
        let mut w = World::new(g, vec![Civilization::new(CivId(0), nation, true), cs]);
        w.diplomacy.meet(CivId(0), CivId(1));
        let home = w.found_city_in(r, City::found("Home", CivId(0), w.grid.id_at(3, 5).unwrap()));
        let there = w.found_city(City::found("There", CivId(1), w.grid.id_at(12, 5).unwrap()));
        for t in w.grid.within(w.grid.id_at(3, 5).unwrap(), 2) {
            w.claim_tile(home, t);
        }
        for t in w.grid.within(w.grid.id_at(12, 5).unwrap(), 1) {
            w.claim_tile(there, t);
        }
        (w, home)
    }

    fn person(w: &mut World, r: &Ruleset, name: &str, at: TileId) -> Id<Unit> {
        let id = r.units.iter().find(|(_, u)| u.name == name).map(|(id, _)| id).unwrap();
        w.place_unit(Unit::new(id, CivId(0), at, 2))
    }

    #[test]
    fn a_scientist_pours_in_eight_turns_of_science() {
        let r = ruleset();
        let (mut w, _) = world(&r);
        let c = w.civ_mut(CivId(0)).unwrap();
        c.recent_science = [5; 8];
        let at = w.grid.id_at(4, 5).unwrap();
        let s = person(&mut w, &r, "Great Scientist", at);
        let scale = Scaling::default();
        assert_eq!(can(&w, &r, s, Action::HurryResearch, scale), Err(ActionRejection::NothingResearched));
        let tech = crate::world::research::available_techs(&r, w.civ(CivId(0)).unwrap())[0];
        w.civ_mut(CivId(0)).unwrap().researching = Some(tech);
        let out = perform(&mut w, &r, s, Action::HurryResearch, scale).unwrap();
        assert!(matches!(out, Outcome::Science { amount: 40, .. }), "{out:?}");
        assert!(w.units.get(s).is_none(), "spent");
    }

    #[test]
    fn an_engineer_stops_one_hammer_short() {
        let r = ruleset();
        let (mut w, home) = world(&r);
        let monument = r.buildings.id("Monument").unwrap();
        w.cities.get_mut(home).unwrap().queue.push(Construction::Building(monument));
        let centre = w.cities.get(home).unwrap().centre;
        let e = person(&mut w, &r, "Great Engineer", centre);
        let cost =
            crate::world::construction::production_cost(&r, Construction::Building(monument), CostModifiers::default())
                .unwrap();
        let out = perform(&mut w, &r, e, Action::HurryProduction, Scaling::default()).unwrap();
        assert_eq!(out, Outcome::Production { city: home, hammers: cost - 1 });
        assert_eq!(w.cities.get(home).unwrap().current_production(), cost - 1);
        assert_eq!(hammers_from_engineer(&w, home, CostModifiers::default()), 330, "300 + 30 a citizen");
    }

    #[test]
    fn a_merchant_trades_only_on_a_city_state_s_land() {
        let r = ruleset();
        let (mut w, _) = world(&r);
        let home = w.grid.id_at(4, 5).unwrap();
        let m = person(&mut w, &r, "Great Merchant", home);
        assert_eq!(can(&w, &r, m, Action::TradeMission, Scaling::default()), Err(ActionRejection::NotACityState));
        w.units.get_mut(m).unwrap().position = w.grid.id_at(13, 5).unwrap();
        let gold_before = w.civ(CivId(0)).unwrap().gold;
        let out = perform(&mut w, &r, m, Action::TradeMission, Scaling::default()).unwrap();
        assert_eq!(out, Outcome::TradeMission { city_state: CivId(1), gold: 350, influence: 30 });
        assert_eq!(w.civ(CivId(0)).unwrap().gold, gold_before + 350);
        assert_eq!(w.diplomacy.influence(CivId(1), CivId(0)), Fixed::whole(30));
    }

    #[test]
    fn an_artist_starts_eight_turns_of_gold() {
        let r = ruleset();
        let (mut w, _) = world(&r);
        let at = w.grid.id_at(4, 5).unwrap();
        let a = person(&mut w, &r, "Great Artist", at);
        let out = perform(&mut w, &r, a, Action::GoldenAge, Scaling::default()).unwrap();
        assert_eq!(out, Outcome::GoldenAge { turns: 8 });
        assert!(crate::world::golden_age::is_golden(&w, CivId(0)));
    }

    #[test]
    fn the_ai_spends_a_scientist_on_an_academy_early_and_on_research_late() {
        use crate::world::ai::{great_person_order, UnitOrder};
        let r = ruleset();
        let (mut w, _) = world(&r);
        let centre = w.grid.id_at(3, 5).unwrap();
        let s = person(&mut w, &r, "Great Scientist", centre);
        let academy = r.improvements.id("Academy").unwrap();
        let site = crate::world::ai::landmark_site(&w, &r, CivId(0), academy).expect("room for an academy");
        assert_eq!(great_person_order(&w, &r, CivId(0), s, Scaling::default()), Some(UnitOrder::MoveTo(site)));
        w.units.get_mut(s).unwrap().position = site;
        assert_eq!(
            great_person_order(&w, &r, CivId(0), s, Scaling::default()),
            Some(UnitOrder::GreatPerson(Action::Construct(academy)))
        );

        // Past 55% of the tree, with something to research, it hurries it.
        let all: Vec<_> = r.technologies.ids().collect();
        let c = w.civ_mut(CivId(0)).unwrap();
        c.techs.extend(all.iter().take(all.len() * 56 / 100).copied());
        let next = crate::world::research::available_techs(&r, w.civ(CivId(0)).unwrap())[0];
        w.civ_mut(CivId(0)).unwrap().researching = Some(next);
        assert_eq!(
            great_person_order(&w, &r, CivId(0), s, Scaling::default()),
            Some(UnitOrder::GreatPerson(Action::HurryResearch))
        );
    }

    #[test]
    fn an_artist_with_nowhere_for_a_landmark_starts_a_golden_age() {
        use crate::world::ai::{great_person_order, UnitOrder};
        let r = ruleset();
        let (mut w, _) = world(&r);
        // Every tile of ours already holds a great improvement.
        let academy = r.improvements.id("Academy").unwrap();
        for t in 0..w.grid.len() {
            if w.grid.owner[t] == Some(CivId(0)) {
                w.grid.improvement[t] = Some(academy);
            }
        }
        let at = w.grid.id_at(3, 5).unwrap();
        let a = person(&mut w, &r, "Great Artist", at);
        assert_eq!(
            great_person_order(&w, &r, CivId(0), a, Scaling::default()),
            Some(UnitOrder::GreatPerson(Action::GoldenAge))
        );
    }

    #[test]
    fn a_landmark_goes_inside_the_borders_and_a_citadel_just_outside_takes_the_ring() {
        let r = ruleset();
        let (mut w, home) = world(&r);
        let academy = r.improvements.id("Academy").unwrap();
        let citadel = r.improvements.id("Citadel").unwrap();
        let inside = w.grid.id_at(4, 5).unwrap();
        let outside = w.grid.id_at(6, 5).unwrap();
        assert_eq!(w.grid.owner[outside.0 as usize], None);
        let s = person(&mut w, &r, "Great Scientist", outside);
        assert_eq!(
            can(&w, &r, s, Action::Construct(academy), Scaling::default()),
            Err(ActionRejection::CannotBuildHere)
        );
        w.units.get_mut(s).unwrap().position = inside;
        perform(&mut w, &r, s, Action::Construct(academy), Scaling::default()).unwrap();
        assert_eq!(w.grid.improvement[inside.0 as usize], Some(academy));
        let y = crate::world::yields::tile_yield(&w.grid, &r, inside);
        assert!(y.science >= Fixed::whole(8), "{y:?}");

        let g = person(&mut w, &r, "Great General", outside);
        perform(&mut w, &r, g, Action::Construct(citadel), Scaling::default()).unwrap();
        for t in w.grid.within(outside, 1) {
            assert_eq!(w.grid.owner[t.0 as usize], Some(CivId(0)), "the citadel holds its ring");
        }
        let _ = home;
    }
}
