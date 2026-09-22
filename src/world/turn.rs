//! Advancing the game one turn.
//!
//! Split into two phases on purpose, following the reasoning in
//! `docs/state-architecture.md`:
//!
//! 1. **Plan** — each civ reads the world and produces a [`TurnPlan`]. Pure:
//!    it takes `&World` and mutates nothing, so every civ's plan can be
//!    computed at the same time. This is where the expensive work lives; Unciv
//!    has roughly eight thousand lines of automation deciding these things.
//! 2. **Apply** — plans are consumed in [`CivId`] order and mutate the world.
//!    Cheap, sequential, and deterministic because the order is fixed.
//!
//! This crate stays dependency-free, so nothing here actually runs in parallel
//! yet. That is deliberate: the *shape* is what has to be right. Turning the
//! planning loop into a parallel one is a change at the call site, and because
//! the phases are already separated it cannot change the result — which is
//! exactly the property that lets the same log replay single-threaded.
//!
//! Unciv cannot do this. It retrofitted threading with 21 `synchronized`
//! blocks and 8 `ConcurrentHashMap`s inside `logic/`, including one on the hot
//! unique-lookup path.

use crate::arena::Id;
use crate::ruleset::Ruleset;
use crate::world::assignment::{assign_idle_citizens, CityFocus};
use crate::modifiers::{Modifier, ModifierStack, Source, SourceKind};
use crate::ruleset::Technology;
use crate::stats::Fixed;
use crate::world::conditionals::{applies, GameContext, Verdict};
use crate::UniqueType;
use crate::world::ai::UnitOrder;
use crate::world::entities::Unit;
use crate::world::city::{apply_food, city_yield, food_surplus, City, GrowthEvent, GrowthModifiers};
use crate::world::construction::{apply_production, ConstructionEvent, Construction, CostModifiers};
use crate::world::policies::PolicyModifiers;
use crate::world::research::{apply_science, ResearchEvent, ScienceModifiers};
use crate::world::grid::{CivId, TileId};
use crate::world::world::World;

/// What one civ intends to do, computed without touching the world.
#[derive(Debug, Clone, PartialEq)]
pub struct TurnPlan {
    pub civ: CivId,
    /// Which tiles each city's citizens should work.
    pub assignments: Vec<(Id<City>, Vec<TileId>)>,
    /// Food each city will bank, computed against the planned assignment.
    pub food: Vec<(Id<City>, i32)>,
    /// Production each city will bank.
    pub production: Vec<(Id<City>, i32)>,
    /// Science the civ will bank, summed across its cities.
    pub science: i32,
    /// Culture the civ will bank, toward its next policy.
    pub culture: i32,
    /// Net gold: what the cities make, less what their buildings cost to keep.
    pub gold: i32,
    /// The civ's happiness this turn. Carried in the plan because the growth
    /// penalties are conditional on it, and both phases need the same figure.
    pub happiness: i32,
    /// What to research, if nothing is under way.
    pub research: Option<Id<Technology>>,
    /// What to adopt, if the culture is there.
    pub policy: Option<Id<crate::ruleset::Policy>>,
    /// What each idle city should start on.
    pub construction: Vec<(Id<City>, Construction)>,
    /// What each unit should do. Settlers walk and found; everything else has
    /// no opinion yet.
    pub orders: Vec<(Id<Unit>, UnitOrder)>,
}

/// Something that happened, in the order it happened.
///
/// The log is the save format and the thing clients replay, so every state
/// change a player could notice has to appear here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TurnEvent {
    CityGrew { city: Id<City>, population: u32 },
    CityStarved { city: Id<City>, population: u32 },
    CityStored { city: Id<City>, food: i32 },
    CityHeld { city: Id<City>, food: i32 },
    CityCompleted { city: Id<City>, what: Construction },
    /// Reported every turn, because an empire sliding toward revolt should be
    /// visible before it revolts.
    Happiness { civ: CivId, value: i32 },
    TechLearned { civ: CivId, tech: Id<Technology> },
    CivDefeated { civ: CivId },
    CityFounded { city: Id<City>, by: CivId },
    /// Someone has met every milestone of a victory condition. Emitted once;
    /// what a caller does about it is not this layer's business.
    Victory { civ: CivId, condition: Id<crate::ruleset::Victory> },
    TurnEnded { turn: u32 },
}

/// Decide what a civ will do. Reads only.
pub fn plan_civ_turn(
    world: &World,
    r: &Ruleset,
    civ: CivId,
    focus: CityFocus,
    difficulty: Option<&crate::ruleset::Difficulty>,
    costs: CostModifiers,
    science: ScienceModifiers,
    policies: PolicyModifiers,
) -> TurnPlan {
    let mut plan = TurnPlan {
        civ,
        assignments: Vec::new(),
        food: Vec::new(),
        production: Vec::new(),
        science: 0,
        culture: 0,
        gold: 0,
        happiness: 0,
        research: None,
        policy: None,
        construction: Vec::new(),
        orders: Vec::new(),
    };
    let Some(c) = world.civ(civ) else { return plan };
    plan.happiness = crate::world::happiness::happiness(world, r, civ, difficulty);

    // Decisions. All pure reads, so every civ could plan at once.
    if c.researching.is_none() {
        plan.research = crate::world::ai::choose_research(world, r, civ, science);
    }
    plan.policy = crate::world::ai::choose_policy(world, r, civ, policies);

    let want_settler = crate::world::ai::wants_settler(world, r, civ);
    for unit in &c.units {
        if let Some(order) = crate::world::ai::settler_order(world, r, civ, *unit) {
            plan.orders.push((*unit, order));
        }
    }

    for &city_id in &c.cities {
        let Some(city) = world.cities.get(city_id) else { continue };
        // Work on a copy: planning must not disturb the world the other civs
        // are reading at the same time.
        let mut hypothetical = city.clone();
        assign_idle_citizens(&world.grid, r, &mut hypothetical, focus);
        let surplus = food_surplus(&world.grid, r, &hypothetical);
        let output = city_yield(&world.grid, r, &hypothetical);

        // An unhappy empire grows slowly or not at all, and the thresholds for
        // that are the ruleset's, not this engine's.
        let growth = growth_modifier(world, r, civ, plan.happiness);
        let effective = if growth.is_empty() {
            surplus
        } else {
            growth.evaluate(Fixed::whole(surplus)).floor() as i32
        };

        if hypothetical.queue.is_empty() {
            // What the civ nets now, so it does not commit to upkeep it
            // cannot cover.
            let net = c
                .cities
                .iter()
                .filter_map(|id| world.cities.get(*id))
                .map(|city| {
                    city_yield(&world.grid, r, city).gold.round() as i32
                        - crate::world::city::city_maintenance(r, city)
                })
                .sum::<i32>();
            if let Some(what) =
                crate::world::ai::choose_construction(world, r, civ, &hypothetical, costs, net, want_settler)
            {
                plan.construction.push((city_id, what));
            }
        }
        let upkeep = crate::world::city::city_maintenance(r, &hypothetical);
        plan.assignments.push((city_id, std::mem::take(&mut hypothetical.worked)));
        plan.food.push((city_id, effective));
        plan.production.push((city_id, output.production.round() as i32));
        plan.science += output.science.round() as i32;
        plan.culture += output.culture.round() as i32;
        plan.gold += output.gold.round() as i32 - upkeep;
    }
    plan
}

/// How much science a civ loses to its debts.
///
/// Civ does not sell your buildings when the treasury runs dry — it takes the
/// shortfall out of research (`CivInfoStatsForNextTurn.kt:216`). The condition
/// is both being *in* debt and still *losing* gold, so a civ with reserves
/// running a deficit is fine until the reserves are gone.
///
/// Returns a non-positive number, and never takes the last point of science:
/// an empire in ruin still learns something.
pub fn treasury_deficit(gold: i32, gold_per_turn: i32, science: i32) -> i32 {
    if gold_per_turn >= 0 || gold >= 0 {
        return 0;
    }
    gold_per_turn.max(1 - science).min(0)
}

/// Growth modifiers in force for a civ, from the ruleset's global uniques.
///
/// Only surplus food is affected — a starving city is not made hungrier by
/// unhappiness, and `NullifiesGrowth` stops accumulation rather than reversing
/// it. Returns an empty stack when nothing applies, so the common case costs
/// nothing.
pub fn growth_modifier(world: &World, r: &Ruleset, civ: CivId, happiness: i32) -> ModifierStack {
    let ctx = GameContext::new(r).with_world(world).for_civ(civ).with_happiness(happiness);
    let mut stack = ModifierStack::new();

    for u in &r.global_uniques {
        if applies(u, &ctx) != Verdict::True {
            continue;
        }
        match u.unique_type() {
            Some(UniqueType::NullifiesGrowth) => {
                stack.ceil_at(Fixed::ZERO);
            }
            Some(UniqueType::GrowthPercentBonus) => {
                if let Some(pct) = u.params.first().and_then(|p| p.parse::<i32>().ok()) {
                    stack.add(Modifier::additive_percent(
                        Fixed::whole(pct),
                        Source::new(SourceKind::Other, "Unhappiness"),
                    ));
                }
            }
            _ => {}
        }
    }
    stack
}

/// Carry out a plan. Mutates.
pub fn apply_plan(
    world: &mut World,
    r: &Ruleset,
    plan: &TurnPlan,
    m: GrowthModifiers,
    costs: CostModifiers,
    science: ScienceModifiers,
    policies: PolicyModifiers,
) -> Vec<TurnEvent> {
    let mut events = Vec::new();

    // Orders first: a settler that founds this turn should be counted as a
    // city for everything that follows.
    for (unit, order) in &plan.orders {
        match order {
            UnitOrder::MoveTo(tile) => {
                let _ = crate::world::movement::move_unit(world, r, *unit, *tile);
            }
            UnitOrder::FoundCity => {
                let name = format!("City {}", world.cities.len() + 1);
                if let Ok(city) = crate::world::settling::found_city_with(world, r, *unit, name) {
                    events.push(TurnEvent::CityFounded { city, by: plan.civ });
                }
            }
        }
    }

    for (city_id, what) in &plan.construction {
        if let Some(city) = world.cities.get_mut(*city_id) {
            if city.queue.is_empty() {
                city.queue.push(*what);
            }
        }
    }
    if let Some(tech) = plan.research {
        if let Some(civ) = world.civ_mut(plan.civ) {
            if civ.researching.is_none() {
                civ.researching = Some(tech);
            }
        }
    }
    if let Some(policy) = plan.policy {
        let _ = crate::world::policies::apply_culture(world, r, plan.civ, 0, Some(policy), policies);
    }

    for (city_id, worked) in &plan.assignments {
        if let Some(city) = world.cities.get_mut(*city_id) {
            city.worked = worked.clone();
        }
    }

    for (city_id, food) in &plan.food {
        let Some(city) = world.cities.get_mut(*city_id) else { continue };
        events.push(match apply_food(city, *food, m) {
            GrowthEvent::Grew { population, .. } => TurnEvent::CityGrew { city: *city_id, population },
            GrowthEvent::Starved { population } => TurnEvent::CityStarved { city: *city_id, population },
            GrowthEvent::Stored { food } => TurnEvent::CityStored { city: *city_id, food },
            GrowthEvent::Held { food } => TurnEvent::CityHeld { city: *city_id, food },
        });
    }

    for (city_id, production) in &plan.production {
        let Some(city) = world.cities.get_mut(*city_id) else { continue };
        let (done, centre, owner) = {
            let event = apply_production(city, r, *production, costs);
            (event, city.centre, city.owner)
        };
        if let ConstructionEvent::Completed { what, .. } = done {
            events.push(TurnEvent::CityCompleted { city: *city_id, what });
            // A finished unit has to be put on the map. Buildings are recorded
            // inside the city; units are not, and without this the production
            // spent on them simply disappears.
            if let Construction::Unit(base) = what {
                let movement = r.units.get(base).map_or(2, |u| u.movement);
                world.place_unit(Unit::new(base, owner, centre, movement));
            }
        }
    }

    // Debt is paid out of research before the science is banked.
    let science_after_debt = {
        let gold = world.civ(plan.civ).map_or(0, |c| c.gold);
        plan.science + treasury_deficit(gold, plan.gold, plan.science)
    };
    if let Some(civ) = world.civ_mut(plan.civ) {
        civ.gold += plan.gold;
        civ.culture_stored += plan.culture.max(0);
    }
    events.push(TurnEvent::Happiness { civ: plan.civ, value: plan.happiness });

    if let Some(civ) = world.civ_mut(plan.civ) {
        if let ResearchEvent::Completed { tech, .. } = apply_science(civ, r, science_after_debt, science) {
            events.push(TurnEvent::TechLearned { civ: plan.civ, tech });
        }
    }

    if let Some(civ) = world.civ_mut(plan.civ) {
        if !civ.defeated && civ.cities.is_empty() && civ.units.is_empty() {
            civ.defeated = true;
            events.push(TurnEvent::CivDefeated { civ: plan.civ });
        }
    }

    events
}

/// Recompute what each civ can see. Exploration is permanent, sight is not.
pub fn update_visibility(world: &mut World) {
    let civ_ids: Vec<CivId> = world.civ_ids().collect();
    for civ in civ_ids {
        world.visibility.clear_visible(civ);
    }

    let seen: Vec<(CivId, Vec<TileId>)> = {
        let mut out = Vec::new();
        for (_, unit) in world.units.iter() {
            out.push((unit.owner, world.grid.within(unit.position, 2)));
        }
        for (_, city) in world.cities.iter() {
            out.push((city.owner, world.grid.within(city.centre, 3)));
        }
        out
    };
    for (civ, tiles) in seen {
        for t in tiles {
            world.visibility.observe(civ, t);
        }
    }
}

/// One full turn: plan for every living civ, then apply in id order.
pub fn advance_turn(
    world: &mut World,
    r: &Ruleset,
    focus: CityFocus,
    m: GrowthModifiers,
    costs: CostModifiers,
    science: ScienceModifiers,
    difficulty: Option<&crate::ruleset::Difficulty>,
    policies: PolicyModifiers,
) -> Vec<TurnEvent> {
    // Phase 1 — pure. Every plan sees the same world; none of them change it.
    // A parallel map here would produce the identical `plans` vector.
    let plans: Vec<TurnPlan> = world
        .living_civs()
        .collect::<Vec<_>>()
        .into_iter()
        .map(|civ| plan_civ_turn(world, r, civ, focus, difficulty, costs, science, policies))
        .collect();

    // Phase 2 — sequential, in id order, which is what makes it reproducible.
    let mut events = Vec::new();
    for plan in &plans {
        events.extend(apply_plan(world, r, plan, m, costs, science, policies));
    }

    // Units get their movement back for the turn that is starting.
    crate::world::movement::refresh_movement(world, r);
    update_visibility(world);
    world.turn += 1;

    // Checked after everything has settled, so a turn that completes the last
    // milestone is the turn it is won on.
    if let Some((civ, condition)) = crate::world::victory::winner(world, r) {
        events.push(TurnEvent::Victory { civ, condition });
    }
    events.push(TurnEvent::TurnEnded { turn: world.turn });
    events
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ruleset::Ruleset;
    use crate::world::entities::{Civilization, Unit};
    use crate::world::grid::TileGrid;

    fn ruleset() -> Ruleset {
        Ruleset::load_dir("../android/assets/jsons/Civ V - Gods & Kings")
            .expect("readable")
            .expect("loads")
    }

    fn two_civ_world(r: &Ruleset) -> World {
        let mut g = TileGrid::new(24, 24, r.terrain("Grassland").unwrap());
        g.owner.iter_mut().for_each(|o| *o = Some(CivId(0)));
        let rome = r.nations.id("Rome").or_else(|| r.nations.ids().next()).unwrap();
        let civs = vec![
            Civilization::new(CivId(0), rome, false),
            Civilization::new(CivId(1), rome, true),
        ];
        let mut w = World::new(g, civs);
        w.found_city(City::found("Rome", CivId(0), w.grid.id_at(8, 8).unwrap()));
        w
    }

    /// The property the phase split exists for.
    #[test]
    fn planning_does_not_touch_the_world() {
        let r = ruleset();
        let w = two_civ_world(&r);
        let before = w.clone();
        for civ in w.living_civs().collect::<Vec<_>>() {
            let _ = plan_civ_turn(&w, &r, civ, CityFocus::None, None, CostModifiers::default(), ScienceModifiers::default(), PolicyModifiers::default());
        }
        assert_eq!(w, before, "the read phase must be pure or it cannot be parallelised");
    }

    #[test]
    fn a_turn_advances_and_reports_what_happened() {
        let r = ruleset();
        let mut w = two_civ_world(&r);
        let m = GrowthModifiers::default();

        let events = advance_turn(&mut w, &r, CityFocus::None, m, CostModifiers::default(), ScienceModifiers::default(), None, PolicyModifiers::default());
        assert_eq!(w.turn, 1);
        assert!(matches!(events.last(), Some(TurnEvent::TurnEnded { turn: 1 })));
        assert!(events.iter().any(|e| matches!(e, TurnEvent::CityStored { .. })));
    }

    #[test]
    fn a_city_grows_over_several_turns() {
        let r = ruleset();
        let mut w = two_civ_world(&r);
        let m = GrowthModifiers::default();

        let mut grew_on = None;
        for turn in 1..=20 {
            for e in advance_turn(&mut w, &r, CityFocus::None, m, CostModifiers::default(), ScienceModifiers::default(), None, PolicyModifiers::default()) {
                if let TurnEvent::CityGrew { population, .. } = e {
                    grew_on.get_or_insert((turn, population));
                }
            }
        }
        let (turn, population) = grew_on.expect("a grassland city should grow");
        assert_eq!(population, 2);
        println!("reached size 2 on turn {turn}");
        assert!(turn <= 10, "took {turn} turns");
    }

    #[test]
    fn the_same_world_advances_the_same_way_twice() {
        let r = ruleset();
        let m = GrowthModifiers::default();
        let run = || {
            let mut w = two_civ_world(&r);
            let mut log = Vec::new();
            for _ in 0..12 {
                log.extend(advance_turn(&mut w, &r, CityFocus::None, m, CostModifiers::default(), ScienceModifiers::default(), None, PolicyModifiers::default()));
            }
            (w, log)
        };
        let (w1, log1) = run();
        let (w2, log2) = run();
        assert_eq!(log1, log2, "the event log must be reproducible");
        assert_eq!(w1, w2, "and so must the resulting state");
    }

    #[test]
    fn plans_are_applied_in_civ_order() {
        let r = ruleset();
        let mut w = two_civ_world(&r);
        w.found_city(City::found("Antium", CivId(1), w.grid.id_at(16, 16).unwrap()));

        let events = advance_turn(&mut w, &r, CityFocus::None, GrowthModifiers::default(), CostModifiers::default(), ScienceModifiers::default(), None, PolicyModifiers::default());
        let cities: Vec<_> = events
            .iter()
            .filter_map(|e| match e {
                TurnEvent::CityStored { city, .. } => Some(*city),
                _ => None,
            })
            .collect();
        assert_eq!(cities.len(), 2);
        // Civ 0's city was founded first and is applied first.
        assert!(cities[0].index() < cities[1].index());
    }

    #[test]
    fn sight_follows_units_and_does_not_unexplore() {
        let r = ruleset();
        let mut w = two_civ_world(&r);
        let warrior = r.unit("Warrior").unwrap();
        let start = w.grid.id_at(2, 2).unwrap();
        let id = w.place_unit(Unit::new(warrior, CivId(0), start, 2));

        update_visibility(&mut w);
        assert!(w.visibility.can_see(CivId(0), start));
        let explored_before = w.visibility.explored_count(CivId(0));

        // March away; the old tile stays explored but stops being visible.
        w.units.get_mut(id).unwrap().position = w.grid.id_at(20, 20).unwrap();
        update_visibility(&mut w);
        assert!(!w.visibility.can_see(CivId(0), start));
        assert!(w.visibility.has_explored(CivId(0), start));
        assert!(w.visibility.explored_count(CivId(0)) > explored_before);
    }

    /// Everything at once: a city works tiles, the yields feed growth,
    /// production and science, and both a building and a tech land.
    #[test]
    fn a_game_runs_and_everything_completes() {
        let r = ruleset();
        let mut w = two_civ_world(&r);
        let (m, costs, science) =
            (GrowthModifiers::default(), CostModifiers::default(), ScienceModifiers::default());

        // Grassland alone yields no production, so nothing could ever be
        // built on it — give Rome a realistic site with hills in reach.
        let city_id = w.civ(CivId(0)).unwrap().cities[0];
        let centre = w.cities.get(city_id).unwrap().centre;
        let hill = r.terrain("Hill").expect("Hill exists");
        for t in w.grid.neighbours(centre).collect::<Vec<_>>() {
            w.grid.features[t.0 as usize].push(hill);
        }

        let monument = r.building("Monument").expect("Monument exists");
        w.cities.get_mut(city_id).unwrap().queue.push(Construction::Building(monument));
        let agriculture = r.technologies.id("Agriculture").unwrap();
        w.civ_mut(CivId(0)).unwrap().researching = Some(agriculture);

        let mut built = None;
        let mut learned = None;
        for turn in 1..=120 {
            for e in advance_turn(&mut w, &r, CityFocus::None, m, costs, science, None, PolicyModifiers::default()) {
                match e {
                    TurnEvent::CityCompleted { what, .. } => {
                        built.get_or_insert((turn, what));
                    }
                    TurnEvent::TechLearned { tech, .. } => {
                        learned.get_or_insert((turn, tech));
                    }
                    _ => {}
                }
            }
        }

        let (build_turn, what) = built.expect("the Monument should finish");
        let (tech_turn, tech) = learned.expect("Agriculture should finish");
        println!("Monument on turn {build_turn}, Agriculture on turn {tech_turn}");
        assert_eq!(what, Construction::Building(monument));
        assert_eq!(tech, agriculture);
        assert!(w.cities.get(city_id).unwrap().built.contains(&monument));
        assert!(w.civ(CivId(0)).unwrap().has_tech(agriculture));
        assert!(w.cities.get(city_id).unwrap().population > 1, "and it grew along the way");
    }

    #[test]
    fn units_get_their_movement_back_each_turn() {
        let r = ruleset();
        let mut w = two_civ_world(&r);
        let base = r.unit("Warrior").unwrap();
        let movement = r.units.get(base).unwrap().movement;
        let id = w.place_unit(Unit::new(base, CivId(0), w.grid.id_at(4, 4).unwrap(), movement));

        w.units.get_mut(id).unwrap().movement_left = crate::stats::Fixed::ZERO;
        advance_turn(&mut w, &r, CityFocus::None, GrowthModifiers::default(), CostModifiers::default(), ScienceModifiers::default(), None, PolicyModifiers::default());
        assert_eq!(
            w.units.get(id).unwrap().movement_left,
            crate::stats::Fixed::whole(movement),
            "a spent unit is ready again next turn"
        );
    }

    /// The whole chain: cities make an empire unhappy, the ruleset's own
    /// conditional uniques read that number, and growth slows accordingly.
    /// Nothing in this engine states the thresholds.
    #[test]
    fn unhappiness_slows_growth_by_the_rulesets_own_rules() {
        let r = ruleset();
        let mut w = two_civ_world(&r);
        let prince = r.difficulties.by_name("Prince");

        let content = crate::world::happiness::happiness(&w, &r, CivId(0), prince);
        let calm = growth_modifier(&w, &r, CivId(0), content);
        assert!(calm.is_empty(), "a contented empire has no growth penalty");
        assert_eq!(calm.evaluate(Fixed::whole(10)), Fixed::whole(10));

        // Sprawl until miserable.
        for i in 0..6u16 {
            let centre = w.grid.id_at(2 + i * 2, 14).unwrap();
            let id = w.found_city(City::found(format!("C{i}"), CivId(0), centre));
            w.cities.get_mut(id).unwrap().population = 4;
        }
        let unhappy = crate::world::happiness::happiness(&w, &r, CivId(0), prince);
        assert!(unhappy < -10, "expected revolt territory, got {unhappy}");

        let stack = growth_modifier(&w, &r, CivId(0), unhappy);
        let surplus = stack.evaluate(Fixed::whole(10));
        println!("happiness {unhappy}: 10 surplus food becomes {surplus}");
        assert_eq!(surplus, Fixed::ZERO, "below -10 the ruleset nullifies growth");

        // And in the milder band, growth is reduced rather than stopped.
        let mild = growth_modifier(&w, &r, CivId(0), -5);
        let reduced = mild.evaluate(Fixed::whole(10));
        println!("happiness -5: 10 surplus food becomes {reduced}");
        assert!(reduced > Fixed::ZERO && reduced < Fixed::whole(10), "got {reduced}");
    }

    #[test]
    fn gold_and_culture_accumulate_over_a_turn() {
        let r = ruleset();
        let mut w = two_civ_world(&r);
        let (m, costs, science) =
            (GrowthModifiers::default(), CostModifiers::default(), ScienceModifiers::default());

        let before = w.civ(CivId(0)).unwrap().culture_stored;
        let events = advance_turn(&mut w, &r, CityFocus::None, m, costs, science, None, PolicyModifiers::default());
        let civ = w.civ(CivId(0)).unwrap();
        println!("after a turn: {} gold, {} culture", civ.gold, civ.culture_stored);
        assert!(civ.culture_stored >= before);
        assert!(
            events.iter().any(|e| matches!(e, TurnEvent::Happiness { .. })),
            "happiness should be reported every turn"
        );
    }

    #[test]
    fn a_dead_units_id_stops_resolving() {
        let r = ruleset();
        let mut w = two_civ_world(&r);
        let warrior = r.unit("Warrior").unwrap();
        let id = w.place_unit(Unit::new(warrior, CivId(0), TileId(0), 2));
        assert_eq!(w.civ(CivId(0)).unwrap().units.len(), 1);

        w.remove_unit(id);
        assert!(w.units.get(id).is_none());
        assert!(w.civ(CivId(0)).unwrap().units.is_empty(), "and it is unregistered");
    }
}

#[cfg(test)]
mod self_play {
    use super::*;
    use crate::world::entities::Civilization;
    use crate::world::grid::TileGrid;

    fn ruleset() -> Ruleset {
        Ruleset::load_dir("../android/assets/jsons/Civ V - Gods & Kings")
            .expect("readable")
            .expect("loads")
    }

    /// A civilization left alone should get on with it: research, build, grow
    /// and adopt policies without being told to. Everything up to this point
    /// had to be hand-fed.
    #[test]
    fn a_civ_plays_itself_for_a_hundred_turns() {
        let r = ruleset();
        let mut g = TileGrid::new(24, 24, r.terrain("Grassland").unwrap());
        // A site with something to work: hills for hammers.
        let hill = r.terrain("Hill").unwrap();
        for t in g.ids().filter(|t| t.0 % 3 == 0) {
            g.features[t.0 as usize].push(hill);
        }
        g.owner.iter_mut().for_each(|o| *o = Some(CivId(0)));

        let nation = r.nations.ids().next().unwrap();
        let mut w = World::new(g, vec![Civilization::new(CivId(0), nation, false)]);
        let centre = w.grid.id_at(12, 12).unwrap();
        w.found_city_in(&r, City::found("Rome", CivId(0), centre));

        let prince = r.difficulties.by_name("Prince");
        let (m, costs, science, policies) = (
            GrowthModifiers::default(),
            CostModifiers::default(),
            ScienceModifiers::default(),
            PolicyModifiers::default(),
        );

        let (mut techs, mut builds, mut growths) = (0, 0, 0);
        for _ in 0..100 {
            for e in advance_turn(&mut w, &r, CityFocus::None, m, costs, science, prince, policies) {
                match e {
                    TurnEvent::TechLearned { .. } => techs += 1,
                    TurnEvent::CityCompleted { .. } => builds += 1,
                    TurnEvent::CityGrew { .. } => growths += 1,
                    _ => {}
                }
            }
        }

        let civ = w.civ(CivId(0)).unwrap();
        let city = w.cities.get(civ.cities[0]).unwrap();
        println!(
            "after 100 turns: {techs} techs, {builds} buildings, {growths} growths, \
             {} cities, capital population {}, {} policies, {} gold",
            civ.cities.len(),
            city.population,
            civ.adopted_policies.len(),
            civ.gold
        );

        assert!(techs > 0, "it should have researched something");
        assert!(builds > 0, "it should have built something");
        assert!(growths > 0, "it should have grown");
        assert!(city.population > 1);
        assert_eq!(civ.techs.len(), techs, "every learned tech is recorded");

        // An empire, not a city. Expansion needs a settler built, walked and
        // consumed — three systems agreeing, and the last place a completed
        // unit was silently discarded.
        assert!(civ.cities.len() > 1, "it should have expanded, got {}", civ.cities.len());
        assert!(civ.gold >= 0, "and stayed solvent, got {}", civ.gold);
    }

    /// Civ's answer to bankruptcy: research pays for the debt.
    #[test]
    fn debt_is_taken_out_of_science_but_never_all_of_it() {
        // Solvent, or merely running a deficit with reserves: nothing happens.
        assert_eq!(treasury_deficit(100, -10, 20), 0, "reserves cover it");
        assert_eq!(treasury_deficit(-50, 5, 20), 0, "in debt but earning");

        // In debt and losing: the shortfall comes off research.
        assert_eq!(treasury_deficit(-50, -10, 20), -10);

        // And it stops short of the last point.
        assert_eq!(treasury_deficit(-50, -10, 5), -4, "5 science becomes 1, not -5");
        assert_eq!(treasury_deficit(-50, -100, 1), 0, "already at one");
    }

    /// A game can now end. The lone civ owns the only capital, so it has
    /// already met Domination's single milestone — which is correct rather
    /// than odd: a Civ game with one player is over before it starts.
    #[test]
    fn a_victory_is_detected_and_reported() {
        let r = ruleset();
        let mut g = TileGrid::new(16, 16, r.terrain("Grassland").unwrap());
        g.owner.iter_mut().for_each(|o| *o = Some(CivId(0)));
        let nation = r.nations.ids().next().unwrap();
        let mut w = World::new(g, vec![Civilization::new(CivId(0), nation, false)]);
        let centre = w.grid.id_at(8, 8).unwrap();
        w.found_city_in(&r, City::found("Rome", CivId(0), centre));

        let events = advance_turn(
            &mut w,
            &r,
            CityFocus::None,
            GrowthModifiers::default(),
            CostModifiers::default(),
            ScienceModifiers::default(),
            r.difficulties.by_name("Prince"),
            PolicyModifiers::default(),
        );

        let won = events.iter().find_map(|e| match e {
            TurnEvent::Victory { civ, condition } => Some((*civ, *condition)),
            _ => None,
        });
        let (civ, condition) = won.expect("the only capital on the map is a domination victory");
        println!("{:?} wins by {}", civ, r.victories.name_of(condition).unwrap());
        assert_eq!(civ, CivId(0));
        assert_eq!(r.victories.name_of(condition), Some("Domination"));
    }

    /// And it should do exactly the same thing twice.
    #[test]
    fn self_play_is_reproducible() {
        let r = ruleset();
        let run = || {
            let mut g = TileGrid::new(16, 16, r.terrain("Grassland").unwrap());
            g.owner.iter_mut().for_each(|o| *o = Some(CivId(0)));
            let nation = r.nations.ids().next().unwrap();
            let mut w = World::new(g, vec![Civilization::new(CivId(0), nation, false)]);
            let centre = w.grid.id_at(8, 8).unwrap();
            w.found_city_in(&r, City::found("Rome", CivId(0), centre));
            let mut log = Vec::new();
            for _ in 0..40 {
                log.extend(advance_turn(
                    &mut w,
                    &r,
                    CityFocus::None,
                    GrowthModifiers::default(),
                    CostModifiers::default(),
                    ScienceModifiers::default(),
                    r.difficulties.by_name("Prince"),
                    PolicyModifiers::default(),
                ));
            }
            (w, log)
        };
        let (w1, log1) = run();
        let (w2, log2) = run();
        assert_eq!(log1, log2, "the same game plays out the same way");
        assert_eq!(w1, w2);
    }
}

#[cfg(test)]
mod expansion {
    use super::*;
    use crate::world::entities::Civilization;
    use crate::world::grid::TileGrid;

    /// A completed unit has to reach the map. Buildings are recorded inside
    /// the city; units are not, and for a while the production spent on them
    /// simply vanished — the city paid 106 hammers for a settler that never
    /// existed. Self-play found it; no unit test would have.
    #[test]
    fn a_finished_unit_is_placed_on_the_map() {
        let r = Ruleset::load_dir("../android/assets/jsons/Civ V - Gods & Kings")
            .unwrap()
            .unwrap();
        let mut g = TileGrid::new(8, 8, r.terrain("Grassland").unwrap());
        g.owner.iter_mut().for_each(|o| *o = Some(CivId(0)));
        let nation = r.nations.ids().next().unwrap();
        let mut w = World::new(g, vec![Civilization::new(CivId(0), nation, false)]);
        let centre = w.grid.id_at(4, 4).unwrap();
        let city_id = w.found_city_in(&r, City::found("Rome", CivId(0), centre));

        let warrior = r.unit("Warrior").unwrap();
        let cost = crate::world::construction::production_cost(
            &r,
            Construction::Unit(warrior),
            CostModifiers::default(),
        )
        .unwrap();
        w.cities.get_mut(city_id).unwrap().queue.push(Construction::Unit(warrior));

        assert!(w.civ(CivId(0)).unwrap().units.is_empty());
        let plan = TurnPlan {
            civ: CivId(0),
            production: vec![(city_id, cost)],
            ..TurnPlan {
                civ: CivId(0),
                assignments: Vec::new(),
                food: Vec::new(),
                production: Vec::new(),
                science: 0,
                culture: 0,
                gold: 0,
                happiness: 0,
                research: None,
                policy: None,
                construction: Vec::new(),
                orders: Vec::new(),
            }
        };
        let events = apply_plan(
            &mut w,
            &r,
            &plan,
            GrowthModifiers::default(),
            CostModifiers::default(),
            ScienceModifiers::default(),
            PolicyModifiers::default(),
        );

        assert!(events.iter().any(|e| matches!(e, TurnEvent::CityCompleted { .. })));
        let units = &w.civ(CivId(0)).unwrap().units;
        assert_eq!(units.len(), 1, "the warrior should exist");
        assert_eq!(w.units.get(units[0]).unwrap().position, centre, "and stand in the city");
    }
}
