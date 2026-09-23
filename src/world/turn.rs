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
use crate::modifiers::{Modifier, ModifierStack, Source, SourceKind};
use crate::ruleset::Ruleset;
use crate::ruleset::Technology;
use crate::stats::{Fixed, Stats};
use crate::world::ai::UnitOrder;
use crate::world::assignment::CityFocus;
use crate::world::city::{City, GrowthEvent, GrowthModifiers};
use crate::world::conditionals::{applies, GameContext, Verdict};
use crate::world::construction::{Construction, ConstructionEvent, CostModifiers};
use crate::world::entities::Unit;
use crate::world::grid::{CivId, TileId};
use crate::world::policies::PolicyModifiers;
use crate::world::research::{ResearchEvent, ScienceModifiers};
use crate::world::world::World;
use crate::UniqueType;

/// What one civ intends to do, computed without touching the world.
#[derive(Debug, Clone, PartialEq)]
pub struct TurnPlan {
    pub civ: CivId,
    /// Which tiles each city's citizens should work.
    pub assignments: Vec<(Id<City>, Vec<TileId>)>,
    /// Which specialist slots each city's other citizens fill.
    pub specialists: Vec<(Id<City>, crate::world::specialists::Allocation)>,
    /// Food each city will bank, computed against the planned assignment.
    pub food: Vec<(Id<City>, i32)>,
    /// Production each city will bank.
    pub production: Vec<(Id<City>, i32)>,
    /// Culture each city banks toward its next border tile — the same culture
    /// that also goes to the civ's policies.
    pub border_culture: Vec<(Id<City>, i32)>,
    /// What to buy outright with gold, city by city.
    pub purchases: Vec<(Id<City>, Construction)>,
    /// One religious unit to buy with the civilization's faith.
    pub faith_purchase: Option<(Id<City>, Construction)>,
    /// Science the civ will bank, summed across its cities.
    pub science: i32,
    /// Faith the civ will bank, toward a pantheon and then a religion.
    pub faith: i32,
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
    /// What each unit should do.
    pub orders: Vec<(Id<Unit>, UnitOrder)>,
    /// Civs to declare war on this turn.
    pub declare_war_on: Vec<CivId>,
    /// Gold to send a city-state, and which one. Decided in the read phase
    /// like every other choice, and paid in the write phase.
    pub gift: Option<(CivId, i32)>,
    /// Civs to settle with, where both sides want it.
    pub make_peace_with: Vec<CivId>,
    /// Civs to denounce publicly.
    pub denounce: Vec<CivId>,
    /// City-states to demand tribute of, and whether a worker rather than gold.
    pub tribute: Vec<(CivId, bool)>,
    /// Trades to offer: to whom, what we give, what we get.
    pub trades: Vec<(CivId, Vec<crate::world::trade::Item>, Vec<crate::world::trade::Item>)>,
    /// Demands we were made, answered: who asked, what, and whether we agree.
    pub answers: Vec<(CivId, crate::world::diplomacy::Demand, bool)>,
    /// Demands to make: of whom, and what.
    pub demands: Vec<(CivId, crate::world::diplomacy::Demand)>,
}

/// Something that happened, in the order it happened.
///
/// The log is the save format and the thing clients replay, so every state
/// change a player could notice has to appear here.
// Not `Copy`: a great person is named rather than identified, because the
// kinds come from `greatPersonPoints` keys and need not be units the ruleset
// declares. Forcing an id there would drop the ones that are not.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TurnEvent {
    CityGrew {
        city: Id<City>,
        population: u32,
    },
    CityStarved {
        city: Id<City>,
        population: u32,
    },
    CityStored {
        city: Id<City>,
        food: i32,
    },
    CityHeld {
        city: Id<City>,
        food: i32,
    },
    CityCompleted {
        city: Id<City>,
        what: Construction,
    },
    /// Reported every turn, because an empire sliding toward revolt should be
    /// visible before it revolts.
    Happiness {
        civ: CivId,
        value: i32,
    },
    TechLearned {
        civ: CivId,
        tech: Id<Technology>,
    },
    CivDefeated {
        civ: CivId,
    },
    CityFounded {
        city: Id<City>,
        by: CivId,
    },
    CivsMet {
        a: CivId,
        b: CivId,
    },
    WarDeclared {
        aggressor: CivId,
        against: CivId,
    },
    PeaceMade {
        a: CivId,
        b: CivId,
    },
    /// A unit cleared ancient ruins and was rewarded.
    RuinsExplored {
        civ: CivId,
        tile: TileId,
        reward: Id<crate::ruleset::RuinReward>,
    },
    /// The site was cleared but this ruleset had no applicable reward.
    RuinsCleared {
        civ: CivId,
        tile: TileId,
    },
    UnitMoved(crate::world::movement::MoveEvent),
    UnitDamaged {
        unit: Id<Unit>,
        civ: CivId,
        damage: u8,
        died: bool,
    },
    Endgame(crate::world::victory_actions::Event),
    FriendshipSigned {
        a: CivId,
        b: CivId,
    },
    DefensivePactSigned {
        a: CivId,
        b: CivId,
    },
    ResearchAgreementSigned {
        a: CivId,
        b: CivId,
    },
    ResearchAgreementCompleted {
        a: CivId,
        b: CivId,
    },
    NaturalWonderDiscovered(crate::world::natural_wonders::Discovery),
    ReligiousUnitActed {
        civ: CivId,
        event: crate::world::religious_units::ReligiousEvent,
    },
    ReligionEnhanced {
        civ: CivId,
    },
    ProphetBorn {
        civ: CivId,
        unit: Id<Unit>,
    },
    FaithPurchased {
        city: Id<City>,
        what: Construction,
        faith: i64,
    },
    /// A great person was spent.
    GreatPersonActed {
        civ: CivId,
        outcome: crate::world::great_person_actions::Outcome,
    },
    GreatPersonBorn {
        civ: CivId,
        kind: String,
    },
    PantheonFounded {
        civ: CivId,
        belief: Id<crate::ruleset::Belief>,
    },
    ReligionFounded {
        civ: CivId,
    },
    CityCaptured {
        city: Id<City>,
        by: CivId,
    },
    /// A city named the luxury it wants.
    ResourceDemanded {
        city: Id<City>,
        resource: Id<crate::ruleset::TileResource>,
    },
    /// A city has what it asked for, and celebrates.
    WeLoveTheKingDay {
        city: Id<City>,
        resource: Id<crate::ruleset::TileResource>,
    },
    WeLoveTheKingDayEnded {
        city: Id<City>,
    },
    /// Two civs made a trade.
    TradeMade {
        a: CivId,
        b: CivId,
    },
    /// A trade ran its course.
    TradeEnded {
        a: CivId,
        b: CivId,
    },
    /// A trade was called off: a side could no longer give what it promised.
    TradeCancelled {
        a: CivId,
        b: CivId,
    },
    /// A city-state paid tribute: gold, or a worker when `worker` is set.
    TributePaid {
        city_state: CivId,
        to: CivId,
        gold: i32,
        worker: bool,
    },
    /// One civ publicly denounced another.
    Denounced {
        by: CivId,
        target: CivId,
    },
    /// One civ asked another to stop doing something.
    DemandMade {
        by: CivId,
        of: CivId,
        demand: crate::world::diplomacy::Demand,
    },
    /// And was answered: `by` agreed, or refused, to do what `to` asked.
    DemandAnswered {
        by: CivId,
        to: CivId,
        demand: crate::world::diplomacy::Demand,
        agreed: bool,
    },
    /// `by` was caught breaking what it promised `to`.
    PromiseBroken {
        by: CivId,
        to: CivId,
        demand: crate::world::diplomacy::Demand,
    },
    /// Or kept it to the end.
    PromiseKept {
        by: CivId,
        to: CivId,
        demand: crate::world::diplomacy::Demand,
    },
    /// A golden age began, or a running one was lengthened, by `turns`.
    GoldenAgeStarted {
        civ: CivId,
        turns: u32,
    },
    GoldenAgeEnded {
        civ: CivId,
    },
    /// A conquered city stopped resisting and works again.
    ResistanceEnded {
        city: Id<City>,
    },
    /// A city being razed lost another citizen to the fire.
    CityBurned {
        city: Id<City>,
        population: u32,
    },
    /// A city was razed to the ground. Its id no longer resolves.
    CityRazed {
        city: Id<City>,
        owner: CivId,
    },
    /// Gold bought a city what it was building.
    Purchased {
        city: Id<City>,
        what: Construction,
        gold: i64,
    },
    /// A city's culture bought it another tile.
    BordersExpanded {
        city: Id<City>,
        tile: TileId,
    },
    Battle {
        attacker_died: bool,
        defender_died: bool,
        /// Participants and damage, where one unit attacked one target.
        report: Option<crate::world::combat::BattleReport>,
    },
    TileImproved {
        tile: crate::world::grid::TileId,
        improvement: Id<crate::ruleset::TileImprovement>,
    },
    /// Someone has met every milestone of a victory condition. Emitted once;
    /// what a caller does about it is not this layer's business.
    Victory {
        civ: CivId,
        condition: Id<crate::ruleset::Victory>,
    },
    /// A technology changed hands without being researched. `identified` is
    /// whether the victim learned whose spy took it, which is what turns a
    /// theft into a grievance rather than a mystery.
    TechStolen {
        thief: CivId,
        victim: CivId,
        tech: Id<Technology>,
        identified: bool,
    },
    Coup(crate::world::city_states::CoupResult),
    /// A spy was caught in the act and killed.
    SpyCaught {
        civ: CivId,
        city: Id<City>,
    },
    CampCleared {
        civ: CivId,
        tile: TileId,
        gold: i32,
        recruited: Option<Id<Unit>>,
    },
    /// A camp produced a unit. Reported because barbarians are the early
    /// game's only pressure, and a game where they never appear looks like a
    /// game where the AI is simply winning.
    BarbarianCampAppeared {
        civ: CivId,
        tile: TileId,
    },
    BarbarianSpawned {
        camp: crate::world::grid::TileId,
    },
    /// A major civ's spy carried a city-state's election.
    ElectionRigged {
        city_state: CivId,
        winner: CivId,
    },
    /// A city-state asked something of a major civ.
    QuestOffered {
        city_state: CivId,
        quest: Id<crate::ruleset::Quest>,
        to: CivId,
    },
    /// And was given it, and paid for it.
    QuestCompleted {
        city_state: CivId,
        quest: Id<crate::ruleset::Quest>,
        by: CivId,
        influence: i32,
    },
    /// Or was not, in time.
    QuestExpired {
        city_state: CivId,
        quest: Id<crate::ruleset::Quest>,
        on: CivId,
    },
    /// A major civ bought favour.
    CityStateGift {
        city_state: CivId,
        to: CivId,
        unit: Id<Unit>,
    },
    CityStateMarried {
        city_state: CivId,
        by: CivId,
        gold: i32,
    },
    GoldGifted {
        from: CivId,
        to: CivId,
        gold: i32,
        influence: Fixed,
    },
    /// A city-state's opinion of a major civ crossed a threshold — gained an
    /// ally, lost a friend. Reported on the change, not every turn, because
    /// the influence number itself moves constantly and the *rank* is what
    /// anyone acts on.
    CityStateRelationship {
        city_state: CivId,
        major: CivId,
        from: crate::world::city_states::Relationship,
        to: crate::world::city_states::Relationship,
    },
    /// A city changed its majority religion. `from` is `None` when it had no
    /// majority before, which is the common case — most conversions are of
    /// the uncommitted, not of the faithful.
    CityConverted {
        city: Id<City>,
        from: Option<CivId>,
        to: CivId,
    },
    /// A soldier the treasury could not pay for. Not a battle loss — a
    /// bookkeeping one, which is why it is a separate event.
    RebelSpawned {
        civ: CivId,
        unit: Id<crate::world::Unit>,
    },
    UnitDisbanded {
        civ: CivId,
        unit: Id<crate::world::entities::Unit>,
    },
    TurnEnded {
        turn: u32,
    },
}

/// Decide what a civ will do. Reads only.
/// Everything a turn runs under that is not part of the world: game speed and
/// the civ-wide modifiers, and the difficulty.
///
/// One value rather than five parameters, because every caller threads all
/// five through together and most want the defaults.
#[derive(Debug, Clone, Copy, Default)]
pub struct TurnModifiers<'a> {
    pub barbarian: crate::world::barbarians::CampModifiers,
    pub growth: GrowthModifiers,
    pub costs: CostModifiers,
    pub science: ScienceModifiers,
    pub policies: PolicyModifiers,
    pub difficulty: Option<&'a crate::ruleset::Difficulty>,
}

pub fn plan_civ_turn(world: &World, r: &Ruleset, civ: CivId, focus: CityFocus, mods: TurnModifiers) -> TurnPlan {
    let mods = TurnModifiers { difficulty: mods.difficulty.or_else(|| world.game_difficulty(r)), ..mods };
    plan_civ(world, r, civ, focus, mods, true)
}

/// Compute a round's income without choosing actions for the player.
pub fn plan_economy(world: &World, r: &Ruleset, civ: CivId, focus: CityFocus, mods: TurnModifiers) -> TurnPlan {
    let mods = TurnModifiers { difficulty: mods.difficulty.or_else(|| world.game_difficulty(r)), ..mods };
    plan_civ(world, r, civ, focus, mods, false)
}

fn plan_civ(
    world: &World,
    r: &Ruleset,
    civ: CivId,
    focus: CityFocus,
    mods: TurnModifiers,
    automated: bool,
) -> TurnPlan {
    let TurnModifiers { costs, science, policies, difficulty, .. } = mods;
    let mut plan = TurnPlan {
        civ,
        assignments: Vec::new(),
        specialists: Vec::new(),
        food: Vec::new(),
        production: Vec::new(),
        border_culture: Vec::new(),
        purchases: Vec::new(),
        faith_purchase: None,
        science: 0,
        faith: 0,
        culture: 0,
        gold: 0,
        happiness: 0,
        research: None,
        policy: None,
        construction: Vec::new(),
        orders: Vec::new(),
        declare_war_on: Vec::new(),
        make_peace_with: Vec::new(),
        denounce: Vec::new(),
        tribute: Vec::new(),
        trades: Vec::new(),
        answers: Vec::new(),
        demands: Vec::new(),
        gift: None,
    };
    let Some(c) = world.civ(civ) else { return plan };
    plan.happiness = crate::world::happiness::happiness(world, r, civ, difficulty);

    let wants = automated.then(|| crate::world::ai::Priorities::assess(world, r, civ));
    if automated {
        // Decisions. All pure reads, so every civ could plan at once.
        if c.researching.is_none() {
            plan.research = crate::world::ai::choose_research(world, r, civ, science);
        }
        plan.policy = crate::world::ai::choose_policy(world, r, civ, policies);

        plan.gift = crate::world::ai::city_state_gift(world, r, civ, costs.game_length, costs.gold_gift_modifier);

        // War and peace are decided before orders, so soldiers act on this turn's
        // stance rather than last turn's.
        for other in world.civ_ids().collect::<Vec<_>>() {
            if other == civ {
                continue;
            }
            if crate::world::ai::should_declare_war(world, r, civ, other) {
                plan.declare_war_on.push(other);
            } else if crate::world::ai::wants_peace(world, r, civ, other)
                && crate::world::ai::wants_peace(world, r, other, civ)
            {
                // Both have to want it. A one-sided wish is a surrender, which is
                // a different thing and not modelled.
                plan.make_peace_with.push(other);
            }
        }
        plan.denounce = crate::world::ai::choose_denunciations(world, r, civ);
        plan.tribute = crate::world::ai::choose_tribute(world, r, civ);
        plan.trades = crate::world::ai::choose_trades(world, r, civ);
        plan.answers = crate::world::ai::answer_demands(world, r, civ);
        plan.demands = crate::world::ai::choose_demands(world, r, civ);
        plan.faith_purchase = crate::world::religious_units::choose_purchase(world, r, civ, costs);
        // Worked out on the first worker's turn and shared by the rest.
        let worker_jobs = std::cell::OnceCell::new();
        for unit in &c.units {
            let scaling = crate::world::great_person_actions::Scaling { costs, science };
            let order = if world.units.get(*unit).is_some_and(|u| u.has_unique(r, UniqueType::AddInCapital)) {
                None // Dedicated physical delivery before the endgame check.
            } else if world.units.get(*unit).is_some_and(|u| u.has_unique(r, UniqueType::ReligiousUnit)) {
                crate::world::religious_units::plan_action(world, r, civ, *unit)
            } else {
                crate::world::ai::settler_order(world, r, civ, *unit)
                    .or_else(|| crate::world::ai::great_person_order(world, r, civ, *unit, scaling))
                    .or_else(|| crate::world::ai::worker_order(world, r, civ, *unit, &worker_jobs))
                    .or_else(|| crate::world::ai::military_order(world, r, civ, *unit))
                    .or_else(|| crate::world::ai::explore_order(world, r, civ, *unit))
            };
            if let Some(order) = order {
                plan.orders.push((*unit, order));
            }
        }
    }

    // Which cities are joined to the capital, and by what. Derived from the
    // map, the treaties and the tech tree, so it is recomputed rather than
    // cached — see `connections` for why.
    let connections = crate::world::connections::find(world, r, civ);
    let trade_gold =
        |id: Id<City>| -> Stats { crate::world::connections::trade_route_stats(world, r, civ, &connections, id) };
    // Reserves are truncated once for the whole empire (TurnManager.endTurn),
    // after fractional city yields and global contributions have been added.
    let mut reserves = Stats::ZERO;

    for &city_id in &c.cities {
        let Some(city) = world.cities.get(city_id) else { continue };
        // Work on a copy: planning must not disturb the world the other civs
        // are reading at the same time.
        let mut hypothetical = city.clone();
        let bonus = crate::world::specialists::bonus_per_specialist(world, r, city_id);
        // A puppet's governor works its citizens for gold (`CityTurnManager.startTurn`).
        let focus = if city.puppet { CityFocus::Single(crate::stats::Stat::Gold) } else { focus };
        crate::world::assignment::release_extra_citizens_for(&world.grid, r, &mut hypothetical, focus, bonus, c);
        // Every governor places idle citizens; a human player's own picks stay where they are.
        crate::world::assignment::assign_idle_citizens_for(&world.grid, r, &mut hypothetical, focus, bonus, c);
        let golden = crate::world::golden_age::is_golden(world, civ);
        let base_output =
            crate::world::city::city_yield_with_happiness(world, r, &hypothetical, golden, Some(plan.happiness));
        let surplus =
            (base_output.food - crate::world::city_effects::food_consumed(world, r, &hypothetical)).round() as i32;
        let mut output = base_output + trade_gold(city_id);
        // A railroad to the capital is worth a quarter more hammers. The
        // capital gets it for being the capital.
        let rail = crate::world::connections::railroad_production_bonus(world, r, civ, &connections, city_id);
        if !rail.is_zero() {
            if let Some(boosted) = (output.production * (Fixed::whole(100) + rail)).checked_div(Fixed::whole(100)) {
                output.production = boosted;
            }
        }

        // A city resisting its conqueror makes nothing — not food, not gold,
        // not science — beyond the one hammer every city keeps so that
        // something moves (`CityStats`: "newFinalStatList.clear()").
        let surplus = if city.resistance > 0 {
            output = Stats { production: Fixed::ONE, ..Stats::ZERO };
            0
        } else {
            surplus
        };

        // An unhappy empire grows slowly or not at all, and the thresholds for
        // that are the ruleset's, not this engine's.
        let mut growth = growth_modifier(world, r, civ, plan.happiness);
        let ctx = GameContext::new(r).with_world(world).for_civ(civ).in_city(city_id).with_happiness(plan.happiness);
        for u in crate::world::religion::city_uniques(world, r, city)
            .into_iter()
            .filter(|u| applies(u, &ctx) == Verdict::True)
        {
            if u.unique_type() == Some(UniqueType::GrowthPercentBonus)
                && u.params.get(1).is_some_and(|filter| {
                    crate::world::filters::city_matches(world, r, city, Some(city), filter) == Verdict::True
                })
            {
                if let Some(pct) = u.params.first().and_then(|p| Fixed::parse(p)) {
                    growth.add(Modifier::additive_percent(pct, Source::new(SourceKind::Other, "City growth")));
                }
            }
        }
        let grown = if surplus <= 0 || growth.is_empty() {
            Fixed::whole(surplus)
        } else {
            growth.evaluate(Fixed::whole(surplus))
        };
        // A celebrating city's surplus is a quarter larger, added after the
        // other growth modifiers rather than multiplied with them.
        let mut effective = (grown + crate::world::wltkd::food_bonus(city, surplus, plan.happiness)).floor() as i32;

        if automated && hypothetical.queue.is_empty() && hypothetical.production_conversion.is_none() {
            // What the civ nets now, so it does not commit to upkeep it
            // cannot cover.
            let net = c
                .cities
                .iter()
                .filter_map(|id| world.cities.get(*id))
                .map(|city| {
                    crate::world::city::city_yield_in(world, r, city, golden).gold.round() as i32
                        - crate::world::city_effects::maintenance(world, r, city)
                })
                .sum::<i32>()
                + c.cities.iter().map(|id| trade_gold(*id).gold.round() as i32).sum::<i32>()
                - crate::world::connections::transportation_upkeep(world, r, civ).gold.round() as i32
                - crate::world::economy::unit_maintenance(world, r, civ, costs.game_length, difficulty);
            if let Some(what) = crate::world::ai::choose_construction(
                world,
                r,
                civ,
                &hypothetical,
                costs,
                net,
                wants.expect("automated priorities"),
            ) {
                plan.construction.push((city_id, what));
            }
        }
        let current = hypothetical
            .queue
            .first()
            .copied()
            .or_else(|| plan.construction.iter().find(|(id, _)| *id == city_id).map(|(_, what)| *what));
        if effective > 0 && current.is_some_and(|what| crate::world::construction::converts_food(r, what)) {
            output.production += Fixed::whole(crate::world::construction::production_from_food(effective));
            effective = 0;
        }
        output.production = output.production.max(Fixed::ONE);
        let upkeep = crate::world::city_effects::maintenance(world, r, &hypothetical);
        plan.assignments.push((city_id, std::mem::take(&mut hypothetical.worked)));
        plan.specialists.push((city_id, std::mem::take(&mut hypothetical.specialists)));
        plan.food.push((city_id, effective));
        plan.production
            .push((city_id, if city.production_conversion.is_some() { 0 } else { output.production.round() as i32 }));
        // Truncated, not rounded: `cultureStored += culture.toInt()`.
        plan.border_culture.push((city_id, output.culture.floor() as i32));
        reserves += output;
        reserves.gold -= Fixed::whole(upkeep);
    }
    // Gold is spent on what the cities are already building, from what is
    // left once the gift is set aside.
    let gift_gold = plan.gift.map_or(0, |(_, g)| g);
    if automated {
        plan.purchases = crate::world::ai::choose_purchases(world, r, civ, costs, gift_gold, &plan.production);
    }

    // Flat yields the empire is owed that no tile produces: city-state
    // friendship, and any policy or belief that grants stats outright.
    let owed = crate::world::city_states::civ_wide_yields_with_modifiers(world, r, civ, mods)
        + crate::world::city_effects::happiness_conversion(world, r, civ, plan.happiness);
    reserves += owed;
    // Wonder happiness is already included by happiness(). Other per-wonder
    // reserves reach the empire here once, like other flat income.
    let wonders = crate::world::natural_wonders::known_stats(world, r, civ);
    reserves += wonders;

    // Roads and soldiers are charged to the empire, not to a city — a road
    // between two cities belongs to neither, and an army belongs to none.
    reserves.gold -= crate::world::connections::transportation_upkeep(world, r, civ).gold;
    reserves.gold -=
        Fixed::whole(crate::world::economy::unit_maintenance(world, r, civ, costs.game_length, difficulty));
    let truncate = |value: Fixed| (value.raw() / 1000).clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32;
    plan.science = truncate(reserves.science);
    plan.culture = truncate(reserves.culture);
    plan.faith = truncate(reserves.faith);
    plan.gold = truncate(reserves.gold);
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

/// Demands and promises, as turn events.
pub(crate) fn promise_events(
    events: impl IntoIterator<Item = crate::world::promises::PromiseEvent>,
) -> impl Iterator<Item = TurnEvent> {
    use crate::world::promises::PromiseEvent as P;
    events.into_iter().map(|e| match e {
        P::Demanded { by, of, demand } => TurnEvent::DemandMade { by, of, demand },
        P::Agreed { by, to, demand } => TurnEvent::DemandAnswered { by, to, demand, agreed: true },
        P::Refused { by, to, demand } => TurnEvent::DemandAnswered { by, to, demand, agreed: false },
        P::Broken { by, to, demand } => TurnEvent::PromiseBroken { by, to, demand },
        P::Kept { by, to, demand } => TurnEvent::PromiseKept { by, to, demand },
    })
}

/// A quest book's events, as turn events.
pub(crate) fn quest_events(events: Vec<crate::world::quests::QuestEvent>) -> impl Iterator<Item = TurnEvent> {
    use crate::world::quests::QuestEvent as Q;
    events.into_iter().map(|e| match e {
        Q::Offered { city_state, quest, to } => TurnEvent::QuestOffered { city_state, quest, to },
        Q::Completed { city_state, quest, by, influence } => {
            TurnEvent::QuestCompleted { city_state, quest, by, influence }
        }
        Q::Expired { city_state, quest, on } => TurnEvent::QuestExpired { city_state, quest, on },
    })
}

/// Carry out a plan. Mutates.
/// Does this plan still own the city it planned for?
///
/// Plans are all made against the same world and then applied one civ at a
/// time, so a city can change hands *between* a plan being made and being
/// applied. Without this check the previous owner still feeds it, still builds
/// in it and still assigns its citizens — for the rest of the turn it was
/// taken on.
fn still_ours(world: &World, plan: &TurnPlan, city: Id<City>) -> bool {
    world.cities.get(city).is_some_and(|c| c.owner == plan.civ)
}

pub(crate) fn religious_events(
    civ: CivId,
    input: impl IntoIterator<Item = crate::world::religious_units::ReligiousEvent>,
) -> Vec<TurnEvent> {
    use crate::world::religious_units::ReligiousEvent as R;
    let mut events = Vec::new();
    for event in input {
        match &event {
            R::Founded { .. } => events.push(TurnEvent::ReligionFounded { civ }),
            R::Enhanced { .. } => events.push(TurnEvent::ReligionEnhanced { civ }),
            R::Spread { city, from, to: Some(to), .. } | R::HeresyRemoved { city, from, to: Some(to), .. }
                if *from != Some(*to) =>
            {
                events.push(TurnEvent::CityConverted { city: *city, from: *from, to: *to });
            }
            _ => {}
        }
        events.push(TurnEvent::ReligiousUnitActed { civ, event });
    }
    events
}

pub(crate) fn explore_on_entry(
    world: &mut World,
    r: &Ruleset,
    unit: Id<Unit>,
    mods: TurnModifiers,
    wonder_sites: &[(TileId, Id<crate::ruleset::Terrain>)],
    events: &mut Vec<TurnEvent>,
) {
    crate::world::military::on_enter(world, r, unit);
    if let Some(camp) = crate::world::barbarians::on_enter(world, r, unit, mods) {
        events.extend(quest_events(camp.quests));
        events.push(TurnEvent::CampCleared {
            civ: camp.civ,
            tile: camp.tile,
            gold: camp.gold,
            recruited: camp.recruited,
        });
    }
    let Some(u) = world.units.get(unit) else { return };
    let (civ, tile) = (u.owner, u.position);
    // Previous entry observations are already persisted. Discard transient
    // sight before capturing this step so old route tiles stay memories.
    update_visibility_in(world, r);
    let had_ruins = world.grid.improvement[tile.0 as usize].is_some_and(|i| crate::world::ruins::is_ruins(r, i));
    crate::world::fog::capture_observations(world, r);
    let setting = crate::world::ruins::Setting {
        difficulty: mods.difficulty,
        speed: mods.costs.speed,
        faith_cost: mods.costs.faith_cost,
        gold_cost: mods.costs.gold_cost,
        culture_cost: mods.policies.speed,
        science_cost: mods.science.speed,
    };
    if let Some(f) = crate::world::ruins::on_enter(world, r, unit, setting) {
        crate::world::policies::rebuild_uniques(world, r, civ);
        events.push(TurnEvent::RuinsExplored { civ: f.civ, tile: f.tile, reward: f.reward });
    } else if had_ruins && world.grid.improvement[tile.0 as usize].is_none() {
        events.push(TurnEvent::RuinsCleared { civ, tile });
    }
    events.extend(
        crate::world::natural_wonders::discover_at(world, r, wonder_sites)
            .into_iter()
            .map(TurnEvent::NaturalWonderDiscovered),
    );
}

pub fn apply_plan(world: &mut World, r: &Ruleset, plan: &TurnPlan, mods: TurnModifiers) -> Vec<TurnEvent> {
    let mods = TurnModifiers { difficulty: mods.difficulty.or_else(|| world.game_difficulty(r)), ..mods };
    let owned_units = world.civ(plan.civ).map_or_else(Vec::new, |c| c.units.clone());
    for unit in owned_units {
        if let Some(promotion) = crate::world::military::promotion_choice(world, r, unit) {
            let _ = crate::world::military::promote(world, r, unit, promotion);
        }
        if crate::world::combat::can_upgrade_with(world, r, unit, mods.costs.speed).is_ok() {
            let _ = crate::world::combat::upgrade_unit_with(world, r, unit, mods.costs.speed);
        }
    }
    apply_plan_inner(world, r, plan, mods, true)
}

fn apply_plan_inner(
    world: &mut World,
    r: &Ruleset,
    plan: &TurnPlan,
    mods: TurnModifiers,
    automatic_choices: bool,
) -> Vec<TurnEvent> {
    let TurnModifiers { growth: m, costs, science, policies, difficulty, .. } = mods;
    if !world.civ(plan.civ).is_some_and(|c| c.is_alive()) {
        return Vec::new();
    }
    let mut events = Vec::new();

    for other in &plan.declare_war_on {
        events.extend(crate::world::diplomacy_actions::declare_war(world, r, plan.civ, *other));
    }
    for other in &plan.make_peace_with {
        events.extend(crate::world::diplomacy_actions::make_peace(world, r, plan.civ, *other));
    }

    for target in &plan.denounce {
        crate::world::opinion::denounce(world, plan.civ, *target);
        events.push(TurnEvent::Denounced { by: plan.civ, target: *target });
    }

    // What we were asked, answered; then what we caught others at, judged;
    // then what we ask in turn.
    for (by, demand, agree) in &plan.answers {
        if let Ok((e, war)) = crate::world::promises::answer(world, r, plan.civ, *by, *demand, *agree) {
            events.extend(promise_events([e]));
            events.extend(war);
        }
    }
    events.extend(promise_events(crate::world::promises::review(world, plan.civ)));
    for (of, demand) in &plan.demands {
        if let Ok(e) = crate::world::promises::demand(world, plan.civ, *of, *demand) {
            events.extend(promise_events([e]));
        }
    }

    for (cs, worker) in &plan.tribute {
        let paid = if *worker {
            crate::world::tribute::demand_worker(world, r, *cs, plan.civ).map(|_| 0)
        } else {
            crate::world::tribute::demand_gold(
                world,
                r,
                *cs,
                plan.civ,
                costs.gold_gift_modifier,
                costs.tribute_interval,
            )
        };
        if let Ok(gold) = paid {
            events.push(TurnEvent::TributePaid { city_state: *cs, to: plan.civ, gold, worker: *worker });
            events.extend(quest_events(crate::world::quests::on_bullied(world, r, *cs, plan.civ)));
        }
    }

    // Offers go to the other side, who judges them now, against the world
    // as it stands after everything applied so far.
    for (other, give, get) in &plan.trades {
        let turns = costs.deal_duration;
        if crate::world::trade::acceptability(world, r, *other, plan.civ, get, give, turns) < 0 {
            continue;
        }
        if crate::world::trade::accept_with(world, r, plan.civ, *other, (give, get), mods).is_ok() {
            events.push(TurnEvent::TradeMade { a: plan.civ, b: *other });
        }
    }

    // Orders first: a settler that founds this turn should be counted as a
    // city for everything that follows.
    // Wonder locations stay fixed while these orders run. Find them once,
    // rather than scanning every map tile after every movement step.
    let wonder_sites = crate::world::natural_wonders::sites(&world.grid);
    for (unit, order) in &plan.orders {
        // A unit captured by an earlier plan must not obey its former owner.
        if world.units.get(*unit).is_none_or(|u| u.owner != plan.civ) {
            continue;
        }
        if matches!(order, UnitOrder::Attack(_) | UnitOrder::Assault(_)) {
            crate::world::military::set_up(world, r, *unit);
        }
        let before = world.units.get(*unit).map(|u| (u.position, u.movement_left));
        match order {
            UnitOrder::Rebase(_) | UnitOrder::AirSweep(_) | UnitOrder::Paradrop(_) | UnitOrder::NuclearStrike(_) => {
                use crate::world::commands::Command;
                let command = match order {
                    UnitOrder::NuclearStrike(target) => Command::NuclearStrike { unit: *unit, target: *target },
                    UnitOrder::Rebase(to) => Command::Rebase { unit: *unit, to: *to },
                    UnitOrder::AirSweep(target) => Command::AirSweep { unit: *unit, target: *target },
                    UnitOrder::Paradrop(to) => Command::Paradrop { unit: *unit, to: *to },
                    _ => unreachable!(),
                };
                if let Ok(outcome) = crate::world::commands::execute_with(world, r, plan.civ, command, mods) {
                    events.extend(outcome.events);
                }
            }
            UnitOrder::MoveTo(tile) => {
                let _ = crate::world::movement::move_unit_with(world, r, *unit, *tile, |world, moved| {
                    events.push(TurnEvent::UnitMoved(moved));
                    explore_on_entry(world, r, *unit, mods, &wonder_sites, &mut events);
                });
            }
            UnitOrder::Attack(target) => {
                let defender_before = world.units.get(*target).map(|u| u.position);
                let watch =
                    crate::world::combat::BattleWatch::before(world, r, *unit, crate::world::air::Target::Unit(*target));
                let result = if world.units.get(*unit).is_some_and(|u| u.is_air(r)) {
                    crate::world::air::strike(world, r, *unit, crate::world::air::Target::Unit(*target))
                        .map_err(Into::into)
                } else if world.units.get(*unit).is_some_and(|u| u.is_ranged(r)) {
                    if let Some(tile) = world.units.get(*target).map(|u| u.position) {
                        crate::world::combat::resolve_ranged_attack(world, r, *unit, tile)
                    } else {
                        Err(crate::world::combat::CombatRejection::NoSuchUnit)
                    }
                } else {
                    crate::world::combat::resolve_attack(world, r, *unit, *target)
                };
                if let Ok(result) = result {
                    if let (Some(from), Some(after)) = (defender_before, world.units.get(*target)) {
                        if from != after.position {
                            events.push(TurnEvent::UnitMoved(crate::world::movement::MoveEvent {
                                unit: *target,
                                from,
                                to: after.position,
                                spent: Fixed::ZERO,
                            }));
                            explore_on_entry(world, r, *target, mods, &wonder_sites, &mut events);
                        }
                    }
                    if let (Some((from, movement)), Some(after)) = (before, world.units.get(*unit)) {
                        if from != after.position {
                            events.push(TurnEvent::UnitMoved(crate::world::movement::MoveEvent {
                                unit: *unit,
                                from,
                                to: after.position,
                                spent: (movement - after.movement_left).max(Fixed::ZERO),
                            }));
                            explore_on_entry(world, r, *unit, mods, &wonder_sites, &mut events);
                        }
                    }
                    events.push(TurnEvent::Battle {
                        attacker_died: result.attacker_died,
                        defender_died: result.defender_died,
                        report: watch.map(|w| w.report(world)),
                    });
                }
            }
            UnitOrder::Assault(city) => {
                let watch =
                    crate::world::combat::BattleWatch::before(world, r, *unit, crate::world::air::Target::City(*city));
                if world.units.get(*unit).is_some_and(|u| u.is_air(r)) {
                    if let Ok(result) =
                        crate::world::air::strike(world, r, *unit, crate::world::air::Target::City(*city))
                    {
                        events.push(TurnEvent::Battle {
                            attacker_died: result.attacker_died,
                            defender_died: result.defender_died,
                            report: watch.map(|w| w.report(world)),
                        });
                    }
                    continue;
                }
                let result = if world.units.get(*unit).is_some_and(|u| u.is_ranged(r)) {
                    crate::world::combat::attack_city_ranged(world, r, *unit, *city)
                } else {
                    crate::world::combat::attack_city(world, r, *unit, *city)
                };
                if let Ok(result) = result {
                    events.push(TurnEvent::Battle {
                        attacker_died: result.attacker_died,
                        defender_died: false,
                        report: watch.map(|w| w.report(world)),
                    });
                    if result.captured {
                        events.push(TurnEvent::CityCaptured { city: *city, by: plan.civ });
                        if let (Some((from, movement)), Some(after)) = (before, world.units.get(*unit)) {
                            if from != after.position {
                                events.push(TurnEvent::UnitMoved(crate::world::movement::MoveEvent {
                                    unit: *unit,
                                    from,
                                    to: after.position,
                                    spent: (movement - after.movement_left).max(Fixed::ZERO),
                                }));
                                explore_on_entry(world, r, *unit, mods, &wonder_sites, &mut events);
                            }
                        }
                    }
                }
            }
            UnitOrder::Improve(improvement) => {
                if let Some(tile) = world.units.get(*unit).map(|u| u.position) {
                    if let Ok(crate::world::improvements::WorkEvent::Finished { improvement, .. }) =
                        crate::world::improvements::perform_work(world, r, *unit, *improvement)
                    {
                        events.push(TurnEvent::TileImproved { tile, improvement });
                    }
                }
            }
            UnitOrder::GreatPerson(action) => {
                let scaling = crate::world::great_person_actions::Scaling { costs, science };
                if let Ok(outcome) = crate::world::great_person_actions::perform(world, r, *unit, *action, scaling) {
                    use crate::world::great_person_actions::Outcome;
                    match outcome {
                        Outcome::GoldenAge { turns } => {
                            events.push(TurnEvent::GoldenAgeStarted { civ: plan.civ, turns })
                        }
                        Outcome::Science { learned: Some(tech), .. } => {
                            events.push(TurnEvent::TechLearned { civ: plan.civ, tech })
                        }
                        _ => {}
                    }
                    events.push(TurnEvent::GreatPersonActed { civ: plan.civ, outcome });
                }
            }
            UnitOrder::Religious(action) => {
                if let Ok(event) =
                    crate::world::religious_units::perform_with_speed(world, r, *unit, *action, costs.speed)
                {
                    events.extend(religious_events(plan.civ, [event]));
                }
            }
            UnitOrder::FoundCity => {
                let name = format!("City {}", world.cities.len() + 1);
                if let Ok(city) = crate::world::settling::found_city_with(world, r, *unit, name) {
                    crate::world::promises::on_city_founded(world, city);
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
            if civ.researching.is_none() && crate::world::research::can_research(r, civ, tech).is_ok() {
                civ.researching = Some(tech);
            }
        }
    }
    if let Some(policy) = plan.policy {
        let _ = crate::world::policies::apply_culture(world, r, plan.civ, 0, Some(policy), policies);
    }

    for (city_id, worked) in &plan.assignments {
        if !still_ours(world, plan, *city_id) {
            continue;
        }
        if let Some(city) = world.cities.get_mut(*city_id) {
            city.worked = worked.clone();
        }
    }
    for (city_id, specialists) in &plan.specialists {
        if !still_ours(world, plan, *city_id) {
            continue;
        }
        if let Some(city) = world.cities.get_mut(*city_id) {
            city.specialists = specialists.clone();
        }
    }

    for (city_id, food) in &plan.food {
        if !still_ours(world, plan, *city_id) {
            continue;
        }
        let Some(city) = world.cities.get_mut(*city_id) else { continue };
        let before = city.population;
        let (carry, nullified) =
            crate::world::city_effects::growth_rules(world, r, world.cities.get(*city_id).unwrap(), plan.happiness);
        let city = world.cities.get_mut(*city_id).unwrap();
        let growth = crate::world::city::apply_food_with_rules(city, *food, m, carry, nullified);
        // Citizens are born into the city's existing faith, so growth is a
        // source of religious pressure and not only of people.
        let after = city.population;
        if after != before {
            city.faith.on_population_change(after as i32 - before as i32, after);
        }
        // A human city's newborn citizen starts work at once instead of idling until the next round.
        if matches!(growth, GrowthEvent::Grew { .. }) && world.civ(plan.civ).is_some_and(|c| !c.is_ai) {
            crate::world::assignment::place_idle_citizens(world, r, *city_id);
        }
        events.push(match growth {
            GrowthEvent::Grew { population, .. } => TurnEvent::CityGrew { city: *city_id, population },
            GrowthEvent::Starved { population } => TurnEvent::CityStarved { city: *city_id, population },
            GrowthEvent::Stored { food } => TurnEvent::CityStored { city: *city_id, food },
            GrowthEvent::Held { food } => TurnEvent::CityHeld { city: *city_id, food },
        });
    }

    crate::world::religion::settle_adoptions(world, r, costs.speed);
    for (city_id, production) in &plan.production {
        if !still_ours(world, plan, *city_id) {
            continue;
        }
        let Some(outcomes) =
            crate::world::construction::apply_production_with_science(world, r, *city_id, *production, costs, science)
        else {
            continue;
        };
        for done in outcomes {
            if let ConstructionEvent::Completed { what, .. } = done {
                events.push(TurnEvent::CityCompleted { city: *city_id, what });
            }
        }
    }

    // Borders grow after production, as `CityTurnManager.endTurn` orders it.
    for (city_id, culture) in &plan.border_culture {
        if !still_ours(world, plan, *city_id) {
            continue;
        }
        if let Some(tile) = crate::world::expansion::grow(world, r, *city_id, *culture, policies.speed) {
            events.push(TurnEvent::BordersExpanded { city: *city_id, tile });
        }
    }

    // Conquered cities: resistance runs down, and what is being razed burns.
    for e in crate::world::conquest::tick(world, r, plan.civ) {
        events.push(match e {
            crate::world::conquest::ConquestEvent::ResistanceEnded { city } => TurnEvent::ResistanceEnded { city },
            crate::world::conquest::ConquestEvent::Burned { city, population } => {
                TurnEvent::CityBurned { city, population }
            }
            crate::world::conquest::ConquestEvent::Razed { city, owner } => TurnEvent::CityRazed { city, owner },
        });
    }

    // Cities that want a luxury, and those celebrating one.
    for e in crate::world::wltkd::tick(world, r, plan.civ, m.speed) {
        events.push(match e {
            crate::world::wltkd::WltkdEvent::Demanded { city, resource } => {
                TurnEvent::ResourceDemanded { city, resource }
            }
            crate::world::wltkd::WltkdEvent::Started { city, resource } => {
                TurnEvent::WeLoveTheKingDay { city, resource }
            }
            crate::world::wltkd::WltkdEvent::Ended { city } => TurnEvent::WeLoveTheKingDayEnded { city },
        });
    }

    // Debt is paid out of research before the science is banked.
    let science_after_debt = {
        let gold = world.civ(plan.civ).map_or(0, |c| c.gold);
        plan.science + treasury_deficit(gold, plan.gold, plan.science)
    };
    if let Some(civ) = world.civ_mut(plan.civ) {
        civ.gold += plan.gold;
        civ.add_culture(plan.culture.max(0));
    }
    // Courting a city-state is paid out of the same treasury as everything
    // else, and before the debt check, so a civ cannot gift itself into
    // disbanding its own army.
    if let Some((cs, gold)) = plan.gift {
        if let Some(gained) =
            crate::world::city_states::gift_gold(world, plan.civ, cs, gold, costs.game_length, costs.gold_gift_modifier)
        {
            events.push(TurnEvent::GoldGifted { from: plan.civ, to: cs, gold, influence: gained });
            events.extend(quest_events(crate::world::quests::on_gold_gift(world, r, cs, plan.civ)));
        }
    }
    // Purchases after the gift, as Unciv's AI spends: a purchase the
    // treasury can no longer cover is simply not made.
    for (city_id, what) in &plan.purchases {
        if !still_ours(world, plan, *city_id) {
            continue;
        }
        if let Ok(cost) = crate::world::purchase::purchase_with_science(
            world,
            r,
            *city_id,
            *what,
            crate::stats::Stat::Gold,
            costs,
            science,
        ) {
            events.push(TurnEvent::Purchased { city: *city_id, what: *what, gold: cost });
        }
    }
    if let Some((city, what)) = plan.faith_purchase {
        if still_ours(world, plan, city) {
            if let Ok(faith) = crate::world::purchase::purchase_with_science(
                world,
                r,
                city,
                what,
                crate::stats::Stat::Faith,
                costs,
                science,
            ) {
                events.push(TurnEvent::FaithPurchased { city, what, faith });
            }
        }
    }

    // Debt has to bite, or gold is a number that only falls. This is the one
    // place the army shrinks without a battle.
    for unit in crate::world::economy::disband_for_debt(world, r, plan.civ, costs.game_length, difficulty, plan.gold) {
        events.push(TurnEvent::UnitDisbanded { civ: plan.civ, unit });
    }
    events.push(TurnEvent::Happiness { civ: plan.civ, value: plan.happiness });
    match crate::world::golden_age::end_turn(world, plan.civ, plan.happiness, costs.speed, costs.golden_age_length) {
        Some(crate::world::golden_age::GoldenAgeEvent::Started { civ, turns }) => {
            events.push(TurnEvent::GoldenAgeStarted { civ, turns })
        }
        Some(crate::world::golden_age::GoldenAgeEvent::Ended { civ }) => events.push(TurnEvent::GoldenAgeEnded { civ }),
        None => {}
    }

    // Bank even zero-income turns: ruins and gifts can already have supplied
    // enough faith for a pantheon or a physical prophet.
    let event = if automatic_choices || world.civ(plan.civ).is_some_and(|c| c.faith.has_pantheon()) {
        crate::world::religion::apply_faith(world, r, plan.civ, plan.faith, costs.faith_cost)
    } else {
        let c = world.civ_mut(plan.civ).expect("living civ");
        c.add_faith(plan.faith.max(0));
        crate::world::religion::FaithEvent::Stored { faith: c.faith.stored }
    };
    match event {
        crate::world::religion::FaithEvent::PantheonFounded { belief } => {
            crate::world::policies::rebuild_uniques(world, r, plan.civ);
            events.push(TurnEvent::PantheonFounded { civ: plan.civ, belief });
        }
        crate::world::religion::FaithEvent::ProphetBorn { unit } => {
            events.push(TurnEvent::ProphetBorn { civ: plan.civ, unit });
        }
        crate::world::religion::FaithEvent::Stored { .. } => {}
    }
    events.extend(religious_events(plan.civ, crate::world::religious_units::end_turn(world, r, plan.civ)));

    for kind in crate::world::great_people::process_turn(world, r, plan.civ, m.speed) {
        events.push(TurnEvent::GreatPersonBorn { civ: plan.civ, kind });
    }

    let turn = world.turn;
    if let Some(civ) = world.civ_mut(plan.civ) {
        civ.recent_science[(turn % 8) as usize] = science_after_debt;
    }
    let agreement_science = crate::world::research_agreements::take_science(world, r, plan.civ);
    if let ResearchEvent::Completed { tech, .. } = crate::world::research::apply_science_with_income_in(
        world,
        r,
        plan.civ,
        science_after_debt.saturating_add(agreement_science),
        science_after_debt,
        science,
    ) {
        crate::world::effects::on_technology_researched(world, r, plan.civ, tech, costs.speed);
        events.push(TurnEvent::TechLearned { civ: plan.civ, tech });
    }

    events.extend(
        crate::world::barbarians::revolt(world, r, plan.civ, plan.happiness, costs.speed)
            .into_iter()
            .map(|unit| TurnEvent::RebelSpawned { civ: plan.civ, unit }),
    );
    if automatic_choices {
        crate::world::effects::resolve_ai_rewards(world, r, plan.civ, costs.speed);
    }
    events.extend(world.resolve_defeats().into_iter().map(|civ| TurnEvent::CivDefeated { civ }));

    events
}

/// First contact: any civ that can see another's city or unit has met them.
///
/// Nothing was calling this, so three civs on one map ran as three separate
/// games — identical outcomes, no diplomacy, no possibility of war. Meeting is
/// what turns neighbours into a relationship.
pub fn detect_first_contact(world: &mut World) -> Vec<TurnEvent> {
    detect_first_contact_impl(world, None)
}

pub fn detect_first_contact_in(world: &mut World, r: &Ruleset) -> Vec<TurnEvent> {
    detect_first_contact_impl(world, Some(r))
}

fn detect_first_contact_impl(world: &mut World, rules: Option<&Ruleset>) -> Vec<TurnEvent> {
    // Who is standing where, gathered first so the borrow ends.
    let mut occupants = Vec::new();
    for (id, unit) in world.units.iter() {
        occupants.push((unit.position, unit.owner, Some(id)));
    }
    for (_, city) in world.cities.iter() {
        occupants.push((city.centre, city.owner, None));
    }

    let civs: Vec<CivId> = world.civ_ids().collect();
    let mut meetings = Vec::new();
    for observer in &civs {
        for (tile, owner, unit) in &occupants {
            if owner == observer || !world.visibility.can_see(*observer, *tile) {
                continue;
            }
            if let (Some(r), Some(unit)) = (rules, unit) {
                if !crate::world::fog::can_see_unit(world, r, *observer, *unit) {
                    continue;
                }
            }
            meetings.push((*observer, *owner));
        }
    }

    let mut events = Vec::new();
    for (a, b) in meetings {
        let contact = [(a, b), (b, a)].into_iter().find(|(cs, major)| {
            world.civ(*cs).is_some_and(|c| c.is_city_state()) && world.civ(*major).is_some_and(|c| c.is_major())
        });
        let first = contact
            .is_some_and(|(cs, _)| !world.civs.iter().any(|c| c.is_major() && world.diplomacy.have_met(cs, c.id)));
        if let Some(crate::world::diplomacy::DiplomacyEvent::Met { pair }) = world.diplomacy.meet(a, b) {
            let (x, y) = pair.civs();
            if let (Some(r), Some((cs, major))) = (rules, contact) {
                crate::world::city_state_actions::on_met(world, r, cs, major, first);
            }
            events.push(TurnEvent::CivsMet { a: x, b: y });
        }
    }
    events
}

/// Live-game vision uses each unit's sight rules and the civilization's
/// territory and border ring. The rules-free wrapper remains for fixtures.
pub fn update_visibility_in(world: &mut World, r: &Ruleset) {
    let seen: Vec<_> =
        world.units.iter().map(|(id, u)| (u.owner, crate::world::fog::unit_visible_tiles(world, r, id))).collect();
    for civ in world.civ_ids().collect::<Vec<_>>() {
        world.visibility.clear_visible(civ);
    }
    for (civ, tiles) in seen {
        for tile in tiles {
            world.visibility.observe(civ, tile);
        }
    }
    crate::world::espionage::update_visibility(world);
    for tile in world.grid.ids() {
        if let Some(owner) = world.grid.owner[tile.0 as usize] {
            world.visibility.observe(owner, tile);
            for next in world.grid.neighbours(tile) {
                world.visibility.observe(owner, next);
            }
        }
    }
    for (_, city) in world.cities.iter() {
        let ally = if world.civ(city.owner).is_some_and(|c| c.is_city_state()) {
            crate::world::city_states::ally_of(world, city.owner)
        } else {
            None
        };
        if let Some(ally) = ally {
            for tile in &city.tiles {
                world.visibility.observe(ally, *tile);
            }
        }
    }
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
pub fn advance_turn(world: &mut World, r: &Ruleset, focus: CityFocus, mods: TurnModifiers) -> Vec<TurnEvent> {
    let mods = TurnModifiers { difficulty: mods.difficulty.or_else(|| world.game_difficulty(r)), ..mods };
    update_visibility_in(world, r);
    crate::world::fog::capture_observations(world, r);
    let standings_before = crate::world::city_states::standings(world);

    // Phase 1 — pure. Every plan sees the same world; none of them change it.
    // A parallel map here would produce the identical `plans` vector.
    let plans: Vec<TurnPlan> = world
        .living_civs()
        .collect::<Vec<_>>()
        .into_iter()
        .map(|civ| plan_civ_turn(world, r, civ, focus, mods))
        .collect();

    // Phase 2 — sequential, in id order, which is what makes it reproducible.
    let mut events = Vec::new();
    for plan in &plans {
        events.extend(apply_plan(world, r, plan, mods));
    }

    finish_automatic(world, r, mods, standings_before, true, &mut events);
    events
}

/// Close one shared action window. Human actions have already happened;
/// only AI seats choose new commands. Income is derived after those actions
/// and banked once, then the world's automatic systems advance once.
pub fn finish_round(world: &mut World, r: &Ruleset, focus: CityFocus, mods: TurnModifiers) -> Vec<TurnEvent> {
    let mods = TurnModifiers { difficulty: mods.difficulty.or_else(|| world.game_difficulty(r)), ..mods };
    if world.winner.is_some() {
        return Vec::new();
    }
    update_visibility_in(world, r);
    crate::world::fog::capture_observations(world, r);
    let standings_before = crate::world::city_states::standings(world);
    let plans: Vec<_> = world
        .living_civs()
        .filter(|id| world.civ(*id).is_some_and(|c| c.is_ai))
        .map(|id| plan_civ_turn(world, r, id, focus, mods))
        .collect();
    let mut events = Vec::new();
    for plan in &plans {
        for command in ai_commands(world, r, plan, mods) {
            if let Ok(outcome) = crate::world::commands::execute_with(world, r, plan.civ, command, mods) {
                events.extend(outcome.events);
            }
        }
    }
    // Derive every civilization's economy from the same post-action state.
    let economies: Vec<_> = world.living_civs().map(|id| plan_economy(world, r, id, focus, mods)).collect();
    for plan in &economies {
        let automated = world.civ(plan.civ).is_some_and(|c| c.is_ai);
        events.extend(apply_plan_inner(world, r, plan, mods, automated));
    }
    finish_automatic(world, r, mods, standings_before, false, &mut events);
    events
}

fn ai_commands(
    world: &World,
    r: &Ruleset,
    plan: &TurnPlan,
    mods: TurnModifiers,
) -> Vec<crate::world::commands::Command> {
    use crate::world::commands::{AttackTarget, Command};
    let mut commands = Vec::new();
    commands.extend(plan.declare_war_on.iter().map(|target| Command::DeclareWar { target: *target }));
    commands.extend(plan.make_peace_with.iter().map(|target| Command::OfferPeace { target: *target }));
    commands.extend(plan.denounce.iter().map(|target| Command::Denounce { target: *target }));
    commands.extend(plan.answers.iter().map(|(by, demand, agree)| Command::AnswerDemand {
        by: *by,
        demand: *demand,
        agree: *agree,
    }));
    commands.extend(plan.demands.iter().map(|(target, demand)| Command::Demand { target: *target, demand: *demand }));
    commands.extend(
        plan.tribute.iter().map(|(target, worker)| Command::DemandTribute { target: *target, worker: *worker }),
    );
    commands.extend(plan.trades.iter().map(|(target, give, get)| Command::OfferTrade {
        target: *target,
        give: give.clone(),
        get: get.clone(),
    }));
    for (unit, _) in world.units.iter().filter(|(_, u)| u.owner == plan.civ) {
        if let Some(promotion) = crate::world::military::promotion_choice(world, r, unit) {
            commands.push(Command::Promote { unit, promotion });
        }
        if crate::world::combat::can_upgrade_with(world, r, unit, mods.costs.speed).is_ok() {
            commands.push(Command::Upgrade { unit });
        }
    }
    for (unit, order) in &plan.orders {
        if matches!(order, UnitOrder::Attack(_) | UnitOrder::Assault(_))
            && world.units.get(*unit).is_some_and(|u| u.has_unique(r, crate::UniqueType::MustSetUp) && !u.set_up)
        {
            commands.push(Command::SetUp { unit: *unit });
        }
        commands.push(match order {
            UnitOrder::MoveTo(to) => Command::MoveUnit { unit: *unit, to: *to },
            UnitOrder::NuclearStrike(target) => Command::NuclearStrike { unit: *unit, target: *target },
            UnitOrder::Rebase(to) => Command::Rebase { unit: *unit, to: *to },
            UnitOrder::AirSweep(target) => Command::AirSweep { unit: *unit, target: *target },
            UnitOrder::Paradrop(to) => Command::Paradrop { unit: *unit, to: *to },
            UnitOrder::Attack(target) => Command::Attack { unit: *unit, target: AttackTarget::Unit(*target) },
            UnitOrder::Assault(city) => Command::Attack { unit: *unit, target: AttackTarget::City(*city) },
            UnitOrder::FoundCity => {
                Command::FoundCity { unit: *unit, name: format!("City {}", world.cities.len() + commands.len() + 1) }
            }
            UnitOrder::Improve(improvement) => Command::Improve { unit: *unit, improvement: *improvement },
            UnitOrder::GreatPerson(action) => Command::GreatPerson { unit: *unit, action: *action },
            UnitOrder::Religious(action) => Command::Religious { unit: *unit, action: *action },
        });
    }
    commands.extend(
        plan.construction.iter().map(|(city, what)| Command::SetProduction { city: *city, queue: vec![*what] }),
    );
    if let Some(technology) = plan.research {
        commands.push(Command::SetResearch { technology });
    }
    if let Some(policy) = plan.policy {
        commands.push(Command::AdoptPolicy { policy });
    }
    for (city, worked) in &plan.assignments {
        let specialists =
            plan.specialists.iter().find(|(id, _)| id == city).map(|(_, s)| s.clone()).unwrap_or_default();
        commands.push(Command::AssignCitizens { city: *city, worked: worked.clone(), specialists });
    }
    if let Some((target, gold)) = plan.gift {
        commands.push(Command::GiftGold { target, gold });
    }
    commands.extend(plan.purchases.iter().map(|(city, what)| Command::Purchase {
        city: *city,
        what: *what,
        currency: crate::stats::Stat::Gold,
    }));
    if let Some((city, what)) = plan.faith_purchase {
        commands.push(Command::Purchase { city, what, currency: crate::stats::Stat::Faith });
    }
    commands
}

fn finish_automatic(
    world: &mut World,
    r: &Ruleset,
    mods: TurnModifiers,
    standings_before: Vec<(CivId, CivId, crate::world::city_states::Relationship)>,
    all_ai: bool,
    events: &mut Vec<TurnEvent>,
) {
    let TurnModifiers { costs, science, difficulty, .. } = mods;
    for work in crate::world::improvements::continue_work(world, r) {
        if let crate::world::improvements::WorkEvent::Finished { tile, improvement } = work {
            events.push(TurnEvent::TileImproved { tile, improvement });
        }
    }
    // Barbarians spawn from their camps.
    //
    // This was written, tested, and never called — the fourth time in this
    // project that a whole subsystem was correct and unreachable. Every camp
    // on every map produced nothing, so the early game had no pressure on it
    // at all and a civ could expand unopposed forever. The ruleset even ships
    // units marked `uniqueTo: Barbarians` that could never appear.
    //
    // Note the barbarian civ is found by `is_barbarian` alone and not by
    // `is_alive`: a civ with no cities and no units counts as out, and a
    // barbarian civ starts with neither. Requiring it to be alive to spawn
    // means it can never spawn anything, which is a deadlock rather than a
    // rule.
    if let Some(barbarians) = world.civs.iter().find(|c| c.is_barbarian).map(|c| c.id) {
        let settings = crate::world::barbarians::CampModifiers {
            difficulty_delay: difficulty.map_or(0, |d| d.barbarian_spawn_delay),
            ..mods.barbarian
        };
        events.extend(crate::world::barbarians::tick(world, r, barbarians, settings));
    }

    // Spies. New eras grant them, idle ones are posted, and the posted ones
    // work — in that order, so a spy granted this turn is on its way this turn
    // rather than idling for one.
    crate::world::espionage::grant_global_era_spies(world, r);
    for civ in world.civ_ids().collect::<Vec<_>>() {
        if !world.civ(civ).is_some_and(|c| c.is_major() && c.is_alive()) {
            continue;
        }

        if all_ai || world.civ(civ).is_some_and(|c| c.is_ai) {
            for (i, city) in crate::world::espionage::postings(world, r, civ) {
                crate::world::espionage::assign(world, civ, i, city);
            }
            crate::world::espionage::automate_coups(world, r, civ);
        }
    }
    for e in crate::world::espionage::tick_in(world, r, science.speed, difficulty) {
        match e {
            crate::world::espionage::SpyEvent::Coup(result) => events.push(TurnEvent::Coup(result)),
            crate::world::espionage::SpyEvent::TechStolen { thief, victim, tech, identified } => {
                if identified {
                    crate::world::promises::on_spy_discovered(world, victim, thief, false);
                }
                events.push(TurnEvent::TechStolen { thief, victim, tech, identified });
            }
            crate::world::espionage::SpyEvent::Caught { civ, city } => {
                if let Some(victim) = world.cities.get(city).map(|c| c.owner) {
                    crate::world::promises::on_spy_discovered(world, victim, civ, true);
                }
                events.push(TurnEvent::SpyCaught { civ, city });
            }
            _ => {}
        }
    }

    // Elections are held on the city-state's own clock, before quests are
    // judged — winning one can complete a quest that asked for influence.
    for e in crate::world::city_states::hold_elections(world, r) {
        match e {
            crate::world::city_states::ElectionResult::Rigged { city_state, winner } => {
                events.push(TurnEvent::ElectionRigged { city_state, winner });
            }
            crate::world::city_states::ElectionResult::Neutral { .. } => {}
        }
    }

    // Quests are judged and paid before favour decays, so a quest completed
    // this turn is paid at its full value rather than at one point less.
    events.extend(quest_events(crate::world::quests::tick_with_speed(world, r, mods.costs.speed)));

    // City-state favour slides back toward where it belongs, and the bonuses
    // it buys are rebuilt from the new numbers. A friendship that lapsed this
    // turn has to stop paying this turn.
    //
    // Ranks are reported against the start of the turn, not against the decay
    // step alone: a war, a peace or a quest moves a rank as surely as decay
    // does, and a friendship ended by a war declaration is still news.
    crate::world::city_states::tick_in(world, r);
    events.extend(crate::world::city_state_actions::end_round(world, r, mods.costs, all_ai));
    for change in crate::world::city_states::changes_since(world, &standings_before) {
        events.push(TurnEvent::CityStateRelationship {
            city_state: change.city_state,
            major: change.major,
            from: change.from,
            to: change.to,
        });
    }
    for civ in world.civ_ids().collect::<Vec<_>>() {
        if world.civ(civ).is_some_and(|c| c.is_major()) {
            crate::world::policies::rebuild_uniques(world, r, civ);
        }
    }

    for civ in world.civ_ids().collect::<Vec<_>>() {
        if world.civ(civ).is_some_and(|c| c.is_alive() && (all_ai || c.is_ai)) {
            events.extend(crate::world::victory_actions::automate_parts(world, r, civ));
        }
    }

    events.extend(crate::world::research_agreements::automate(world, r, mods, all_ai));

    // What each civ actually holds, recomputed from the map rather than
    // patched when a worker finishes.
    //
    // Nothing called this. Every civ's resource stock stayed empty for the
    // whole game, which meant the luxury happiness bonus had nothing to count
    // and the missing-strategic-resource combat penalty never fired — two
    // systems that were correct, tested, and unreachable. Territory changes
    // hands, improvements are pillaged and techs reveal new deposits, so this
    // is derived per turn like city connection rather than invalidated.
    for civ in world.civ_ids().collect::<Vec<_>>() {
        let stock = crate::world::resources::recompute_stock(world, r, civ);
        if let Some(c) = world.civ_mut(civ) {
            c.resources = stock;
        }
    }
    // Traded resources move from the stock of the giver to the receiver's,
    // and then the deals run a turn: gold a turn paid, clocks down, and
    // anything a side can no longer give called off.
    crate::world::trade::apply_to_stock(world);
    for e in crate::world::trade::tick(world, r) {
        events.push(match e {
            crate::world::trade::TradeEvent::Ended { a, b } => TurnEvent::TradeEnded { a, b },
            crate::world::trade::TradeEvent::Cancelled { a, b } => TurnEvent::TradeCancelled { a, b },
        });
    }

    // Religions spread once the whole map has moved, not inside any one civ's
    // turn — a missionary does not care whose turn it is, and doing it per civ
    // would let the civ that goes first push before the others had grown.
    for c in crate::world::religion::spread_with_speed(world, r, costs.religious_pressure, costs.speed) {
        events.push(TurnEvent::CityConverted { city: c.city, from: c.from, to: c.to });
    }

    events.extend(crate::world::victory_actions::finish_voting(world, r, all_ai).into_iter().map(TurnEvent::Endgame));

    // Units get their movement back for the turn that is starting.
    crate::world::healing::recover(world, r);
    for (unit, civ, damage, died) in crate::world::military::end_round(world, r) {
        events.push(TurnEvent::UnitDamaged { unit, civ, damage, died });
    }
    crate::world::movement::refresh_movement(world, r);
    for (_, city) in world.cities.iter_mut() {
        city.attacks_left = 1;
        city.sold_building_this_turn = false;
    }
    update_visibility_in(world, r);
    crate::world::fog::capture_observations(world, r);
    events.extend(
        crate::world::natural_wonders::discover_visible(world, r).into_iter().map(TurnEvent::NaturalWonderDiscovered),
    );
    events.extend(detect_first_contact_in(world, r));
    crate::world::opinion::tick(world, mods.growth.speed);
    crate::world::tribute::tick(world);
    let expired = world.diplomacy.tick();
    events.extend(promise_events(crate::world::promises::on_flags_expired(world, &expired)));
    events.extend(crate::world::research_agreements::tick(world));
    world.turn += 1;
    for civ in world.civ_ids().collect::<Vec<_>>() {
        if world.civ(civ).is_some_and(|c| c.timed_policy_expirations.values().any(|expiry| *expiry == world.turn)) {
            crate::world::policies::rebuild_uniques(world, r, civ);
        }
    }
    crate::world::great_people::begin_round(world, r, costs.speed);
    events
        .extend(crate::world::victory_actions::begin_round(world, r, costs.speed).into_iter().map(TurnEvent::Endgame));

    // Checked after everything has settled, so a turn that completes the last
    // milestone is the turn it is won on.
    if world.winner.is_none() {
        if let Some((civ, condition)) = crate::world::victory::winner(world, r) {
            world.winner = Some((civ, condition));
            world.victory_turn = Some(world.turn);
            events.push(TurnEvent::Victory { civ, condition });
        }
    }
    events.push(TurnEvent::TurnEnded { turn: world.turn });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ruleset::Ruleset;
    use crate::world::entities::{Civilization, Unit};
    use crate::world::grid::TileGrid;

    fn ruleset() -> Ruleset {
        Ruleset::load_dir("../android/assets/jsons/Civ V - Gods & Kings").expect("readable").expect("loads")
    }

    fn two_civ_world(r: &Ruleset) -> World {
        let mut g = TileGrid::new(24, 24, r.terrain("Grassland").unwrap());
        g.owner.iter_mut().for_each(|o| *o = Some(CivId(0)));
        let rome = r.nations.id("Rome").or_else(|| r.nations.ids().next()).unwrap();
        let civs = vec![Civilization::new(CivId(0), rome, false), Civilization::new(CivId(1), rome, true)];
        let mut w = World::new(g, civs);
        w.found_city(City::found("Rome", CivId(0), w.grid.id_at(8, 8).unwrap()));
        w
    }

    /// The whole road loop, driven by nothing but the turn engine: the plan
    /// notices an unconnected city, a worker walks to the gap, digs, and the
    /// treasury starts earning a trade route.
    ///
    /// Each piece has a unit test. This is here because the pieces have been
    /// connected wrongly before — production that finished into nothing,
    /// income that was earned and never banked — and only running the game
    /// catches that.
    #[test]
    fn a_worker_connects_a_second_city_and_the_gold_starts_coming_in() {
        use crate::world::connections;
        let r = ruleset();
        let mut w = two_civ_world(&r);
        let rome = w.civ(CivId(0)).unwrap().cities[0];
        // Give Rome a palace so it is a capital at all.
        let palace = connections::palace(&r).expect("the ruleset has a palace");
        {
            let c = w.cities.get_mut(rome).unwrap();
            c.built.insert(palace);
            crate::world::city::rebuild_city_uniques(&r, c);
        }
        let ostia = w.found_city(City::found("Ostia", CivId(0), w.grid.id_at(13, 8).unwrap()));
        w.civ_mut(CivId(0)).unwrap().techs.insert(r.technologies.id("The Wheel").unwrap());
        // Cities big enough for the route to be worth its upkeep. Two size-one
        // towns four tiles apart genuinely are not — the road would cost four
        // gold a turn and earn a quarter of one — and the worker is right to
        // refuse. See `connections::link_pays`.
        w.cities.get_mut(rome).unwrap().population = 8;
        w.cities.get_mut(ostia).unwrap().population = 4;

        let worker =
            r.units.iter().find(|(_, u)| u.name == "Worker").map(|(id, _)| id).expect("the ruleset has a worker");
        w.place_unit(Unit::new(worker, CivId(0), w.grid.id_at(9, 8).unwrap(), 2));

        assert!(!connections::find(&w, &r, CivId(0)).has_trade_route(ostia), "not yet");
        let plan = connections::road_plan(&w, &r, CivId(0));
        assert!(!plan.is_empty(), "the civ should want a road");

        let mut connected_on = None;
        for turn in 1..=120 {
            advance_turn(&mut w, &r, CityFocus::None, TurnModifiers::default());
            if connections::find(&w, &r, CivId(0)).has_trade_route(ostia) {
                connected_on = Some(turn);
                break;
            }
        }

        let turn = connected_on.expect("one worker should join two cities inside 120 turns");
        // Four bare tiles at four worker-turns each, plus the walk. If this
        // creeps up, the worker has started dithering between jobs again.
        assert!(turn <= 25, "took {turn} turns, which is dithering, not working");
        let c = connections::find(&w, &r, CivId(0));
        let earned = connections::trade_route_stats(&w, &r, CivId(0), &c, ostia);
        assert!(earned.gold > Fixed::ZERO, "a route that earns nothing is not a route");
        let paid = connections::transportation_upkeep(&w, &r, CivId(0));
        assert!(paid.gold > Fixed::ZERO, "and the road costs something to keep");
    }

    /// Religion, from faith income to a converted neighbour, through the real
    /// turn loop rather than by calling `spread` directly.
    ///
    /// The unit tests cover the pressure arithmetic. This is here because the
    /// wiring has several joints — faith has to reach `apply_faith`, founding
    /// has to mark a holy city, and spread has to run after every civ has
    /// moved — and a system that works in isolation and is never called is
    /// indistinguishable from one that does not exist.
    #[test]
    fn a_religion_is_founded_and_reaches_the_city_next_door() {
        let r = ruleset();
        let mut w = two_civ_world(&r);
        let rome = w.civ(CivId(0)).unwrap().cities[0];
        let centre = w.cities.get(rome).unwrap().centre;
        // Earned prophets are born in the capital before a holy city exists.
        w.cities.get_mut(rome).unwrap().built.insert(crate::world::connections::palace(&r).unwrap());

        // Faith has to come from somewhere. Every building in the ruleset that
        // yields it will do; the shipped one is the Shrine.
        let faith_buildings: Vec<_> = r
            .buildings
            .iter()
            .filter(|(_, b)| crate::ruleset::queries::HasStats::stats(*b).faith > Fixed::ZERO)
            .map(|(id, _)| id)
            .collect();
        assert!(!faith_buildings.is_empty(), "the ruleset should have a faith building");
        {
            let c = w.cities.get_mut(rome).unwrap();
            c.population = 6;
            c.built.extend(faith_buildings.iter().copied());
            crate::world::city::rebuild_city_uniques(&r, c);
        }
        // A neighbour four tiles off, inside the spread range.
        let (col, row) = w.grid.coords(centre);
        let neighbour = w.found_city(City::found("Veii", CivId(1), w.grid.id_at(col + 4, row).unwrap()));
        w.cities.get_mut(neighbour).unwrap().population = 4;

        let (mut founded, mut converted) = (None, None);
        for turn in 1..=200 {
            for e in advance_turn(&mut w, &r, CityFocus::None, TurnModifiers::default()) {
                match e {
                    TurnEvent::ReligionFounded { civ: CivId(0) } => {
                        founded.get_or_insert(turn);
                    }
                    TurnEvent::CityConverted { city, to, .. } if city == neighbour => {
                        converted.get_or_insert((turn, to));
                    }
                    _ => {}
                }
            }
        }

        let founded = founded.expect("a civ with faith income should found a religion");
        let (turn, to) = converted.expect("and it should reach the city four tiles away");
        assert_eq!(to, CivId(0), "the neighbour follows Rome's religion, not its own");
        assert!(turn > founded, "founded on {founded}, converted on {turn}");
        assert_eq!(
            w.cities.get(rome).unwrap().faith.holy_city_of,
            Some(CivId(0)),
            "the city it was founded in is its holy city"
        );
    }

    /// A city that changes hands mid-turn stops taking orders from its old
    /// owner.
    ///
    /// Every civ plans against the same world and then applies one at a time,
    /// so a city can be captured *between* a plan being made and that plan
    /// being applied. The loser's plan still named it, and still fed it, built
    /// in it and assigned its citizens for the rest of that turn.
    #[test]
    fn a_stale_plan_does_not_command_a_city_that_was_just_taken() {
        let r = ruleset();
        let mut w = two_civ_world(&r);
        let theirs = w.found_city(City::found("Veii", CivId(1), w.grid.id_at(12, 12).unwrap()));
        {
            let c = w.cities.get_mut(theirs).unwrap();
            c.population = 6;
            c.food_stored = 0;
            c.worked.clear();
        }

        // A plan civ 1 made while it still held the city.
        let plan = TurnPlan {
            civ: CivId(1),
            food: vec![(theirs, 40)],
            production: vec![(theirs, 60)],
            assignments: vec![(theirs, vec![w.grid.id_at(11, 12).unwrap()])],
            ..TurnPlan {
                civ: CivId(1),
                assignments: Vec::new(),
                specialists: Vec::new(),
                food: Vec::new(),
                production: Vec::new(),
                border_culture: Vec::new(),
                purchases: Vec::new(),
                faith_purchase: None,
                science: 0,
                faith: 0,
                culture: 0,
                gold: 0,
                happiness: 0,
                research: None,
                policy: None,
                construction: Vec::new(),
                orders: Vec::new(),
                declare_war_on: Vec::new(),
                make_peace_with: Vec::new(),
                denounce: Vec::new(),
                tribute: Vec::new(),
                trades: Vec::new(),
                answers: Vec::new(),
                demands: Vec::new(),
                gift: None,
            }
        };

        // Civ 0 takes it first.
        crate::world::combat::capture_city(&mut w, &r, theirs, CivId(0));
        assert_eq!(w.cities.get(theirs).unwrap().owner, CivId(0));

        let before = w.cities.get(theirs).unwrap().clone();
        apply_plan(&mut w, &r, &plan, TurnModifiers::default());
        let after = w.cities.get(theirs).unwrap();
        assert_eq!(after.food_stored, before.food_stored, "the old owner fed a city it had lost");
        assert_eq!(after.production_progress, before.production_progress, "and built in it");
        assert_eq!(after.worked, before.worked, "and told its citizens where to stand");
    }

    /// Generated maps receive camps only after live visibility and capitals exist.
    #[test]
    fn barbarians_come_out_of_camps_during_a_real_turn_loop() {
        use crate::world::mapgen::{generate, MapParameters};
        let r = ruleset();
        let map = generate(&r, MapParameters { width: 32, height: 22, seed: 0xBA48, ..Default::default() }, 2);
        assert!(
            map.grid.improvement.iter().all(|i| *i != crate::world::barbarians::camp_improvement(&r)),
            "map generation leaves camp placement to the live turn loop"
        );

        let nation = r.nations.ids().next().unwrap();
        let mut civs = vec![Civilization::new(CivId(0), nation, true), Civilization::new(CivId(1), nation, true)];
        civs[1].is_barbarian = true;
        let mut w = World::new(map.grid, civs);
        w.diplomacy.mark_hostile(CivId(1));
        let settler =
            r.units.iter().find(|(id, _)| crate::world::settling::is_settler(&r, *id)).map(|(id, _)| id).unwrap();
        w.place_unit(Unit::new(settler, CivId(0), map.starts[0], 2));

        let first = advance_turn(&mut w, &r, CityFocus::None, TurnModifiers::default());
        let camps = crate::world::barbarians::camps(&w, &r);
        assert!(!camps.is_empty(), "the first resolved round seeds camps");
        let capital = crate::world::connections::capital_of(&w, &r, CivId(0)).unwrap();
        let centre = w.cities.get(capital).unwrap().centre;
        for tile in &camps {
            assert!(w.grid.distance(*tile, centre) > 4);
            assert!(w.grid.resource[tile.0 as usize].is_none());
            assert!(!w.visibility.can_see(CivId(0), *tile));
            assert!(camps.iter().all(|other| other == tile || w.grid.distance(*tile, *other) > 7));
        }
        let mut spawned = first.iter().filter(|e| matches!(e, TurnEvent::BarbarianSpawned { .. })).count();
        for _ in 1..60 {
            spawned += advance_turn(&mut w, &r, CityFocus::None, TurnModifiers::default())
                .iter()
                .filter(|e| matches!(e, TurnEvent::BarbarianSpawned { .. }))
                .count();
        }
        assert!(spawned > 0, "camps produce units in the normal turn loop");
        assert!(!w.civ(CivId(1)).unwrap().units.is_empty(), "and the barbarian civ should actually hold them");
    }

    /// A whole game on a generated map with city-states in it.
    ///
    /// The other generated-map test has four majors and nothing else. This one
    /// adds city-states, which is what quests, influence and elections need to
    /// exist at all — and asks whether any of it actually happens rather than
    /// merely compiles.
    #[test]
    fn city_states_ask_for_things_and_get_them() {
        use crate::world::mapgen::{generate, MapParameters};
        let r = ruleset();
        let p = MapParameters { width: 40, height: 26, seed: 0xC157A7E, ..Default::default() };
        let map = generate(&r, p, 6);
        assert_eq!(map.starts.len(), 6);

        let nation = r.nations.ids().next().unwrap();
        let cs_types: Vec<_> = r.city_state_types.ids().collect();
        let mut civs = Vec::new();
        for i in 0..6u8 {
            let mut c = Civilization::new(CivId(i), nation, true);
            // The last two are city-states.
            if i >= 4 {
                c.city_state_type = Some(cs_types[(i as usize - 4) % cs_types.len()]);
            }
            civs.push(c);
        }
        let mut w = World::new(map.grid, civs);
        let settler =
            r.units.iter().find(|(id, _)| crate::world::settling::is_settler(&r, *id)).map(|(id, _)| id).unwrap();
        let worker = r.units.iter().find(|(_, u)| u.name == "Worker").map(|(id, _)| id).unwrap();
        for (i, start) in map.starts.iter().enumerate() {
            w.place_unit(Unit::new(settler, CivId(i as u8), *start, 2));
            if i < 4 {
                w.place_unit(Unit::new(worker, CivId(i as u8), *start, 2));
            }
        }

        let (mut offered, mut completed, mut gifts, mut ranks) = (0, 0, 0, 0);
        for _ in 1..=250 {
            for e in advance_turn(&mut w, &r, CityFocus::None, TurnModifiers::default()) {
                match e {
                    TurnEvent::QuestOffered { .. } => offered += 1,
                    TurnEvent::QuestCompleted { .. } => completed += 1,
                    TurnEvent::GoldGifted { .. } => gifts += 1,
                    TurnEvent::CityStateRelationship { .. } => ranks += 1,
                    TurnEvent::RuinsExplored { civ, reward, .. } => {
                        println!("PROBE t{} {:?} {}", w.turn, civ, r.ruin_rewards.get(reward).unwrap().name)
                    }
                    _ => {}
                }
            }
        }
        println!("quests offered {offered}, completed {completed}, gifts {gifts}, rank changes {ranks}");

        assert!(offered > 0, "a city-state that never asks for anything is a decoration");
        assert!(ranks > 0, "nobody's standing with a city-state ever changed in 250 turns");
        // Every quest handed out must be one the engine can judge, or it would
        // stand until it expired and teach nobody anything.
        let (_, unjudgeable) = crate::world::quests::judgeable(&r);
        for cs in crate::world::city_states::city_states(&w) {
            for q in &w.civ(cs).unwrap().quests.active {
                let name = r.quests.name_of(q.quest).unwrap_or("?");
                assert!(!unjudgeable.iter().any(|u| u == name), "offered unjudgeable {name}");
            }
        }
    }

    /// A whole game on a generated map, which is the only test that asks
    /// whether the engine plays rather than whether it computes.
    ///
    /// The other self-play runs on uniform grassland, which is a fair test of
    /// the rules and an unfair one of the economy — grassland yields no gold
    /// and no faith, so every civ is poor and nobody builds a shrine. On real
    /// ground the numbers should be recognisably a game.
    #[test]
    fn a_game_on_a_generated_map_behaves_like_a_game() {
        use crate::world::mapgen::{generate, MapParameters};
        let r = ruleset();
        let p = MapParameters { width: 40, height: 26, seed: 0x5EED, ..Default::default() };
        let map = generate(&r, p, 4);
        assert_eq!(map.starts.len(), 4);

        let nation = r.nations.ids().next().unwrap();
        let civs: Vec<_> = (0..4u8).map(|i| Civilization::new(CivId(i), nation, true)).collect();
        let mut w = World::new(map.grid, civs);
        let settler = r
            .units
            .iter()
            .find(|(id, _)| crate::world::settling::is_settler(&r, *id))
            .map(|(id, _)| id)
            .expect("the ruleset has a settler");
        let worker = r.units.iter().find(|(_, u)| u.name == "Worker").map(|(id, _)| id).unwrap();
        for (i, start) in map.starts.iter().enumerate() {
            w.place_unit(Unit::new(settler, CivId(i as u8), *start, 2));
            w.place_unit(Unit::new(worker, CivId(i as u8), *start, 2));
        }

        let (mut founded, mut wars, mut converted, mut roads_built) = (0, 0, 0, 0);
        for _ in 1..=250 {
            for e in advance_turn(&mut w, &r, CityFocus::None, TurnModifiers::default()) {
                match e {
                    TurnEvent::CityFounded { .. } => founded += 1,
                    TurnEvent::WarDeclared { .. } => wars += 1,
                    TurnEvent::CityConverted { .. } => converted += 1,
                    TurnEvent::TileImproved { improvement, .. }
                        if r.improvements.name_of(improvement) == Some("Road") =>
                    {
                        roads_built += 1;
                    }
                    _ => {}
                }
            }
        }

        let alive = w.civs.iter().filter(|c| c.is_alive()).count();
        let solvent = w.civs.iter().filter(|c| c.is_alive() && c.gold >= 0).count();
        let techs: usize = w.civs.iter().map(|c| c.techs.len()).sum();
        println!(
            "founded {founded}, wars {wars}, conversions {converted}, roads {roads_built}, \
             alive {alive}, solvent {solvent}, techs {techs}, cities {}",
            w.cities.len()
        );

        assert!(founded >= 4, "every civ should at least settle its start: {founded}");
        assert!(w.cities.len() > 4, "and someone should expand beyond it");
        assert!(techs > 20, "250 turns should buy more than {techs} techs across four civs");
        assert!(solvent > 0, "every surviving civ is bankrupt, which is not a game");

        // Cities have to *grow*. This is the assertion that caught four
        // separate holes at once: the resource stock was never recomputed, so
        // no civ ever held a luxury; luxuries are the only real source of
        // happiness, so every empire sat below zero; below zero the ruleset
        // cuts growth by three quarters and then stops it; and the AI kept
        // founding cities anyway. Twenty-one cities, twenty-three citizens,
        // two hundred and fifty turns.
        let largest = w.cities.iter().map(|(_, c)| c.population).max().unwrap_or(0);
        let total: u32 = w.cities.iter().map(|(_, c)| c.population).sum();
        assert!(largest >= 3, "the biggest city on the map is size {largest}");
        assert!(
            total as usize > w.cities.len(),
            "{total} citizens across {} cities averages less than one each",
            w.cities.len()
        );
    }

    /// A major civ buys a city-state's friendship through the real turn loop
    /// and starts collecting on it.
    ///
    /// The joints: the AI has to want the gift, the plan has to carry it, the
    /// gold has to move, influence has to cross a threshold, the bonus
    /// uniques have to reach the civ's index, and something has to *read*
    /// them. The last one was missing — every Cultured city-state in the game
    /// paid out nothing, because a civ-level `[+3 Culture]` had no consumer.
    #[test]
    fn a_city_state_can_be_bought_and_then_pays_out() {
        use crate::world::city_states::{ally_of, relationship, Relationship};
        let r = ruleset();
        let mut w = two_civ_world(&r);
        // Turn civ 1 into a Cultured city-state with a city of its own.
        {
            let c = w.civ_mut(CivId(1)).unwrap();
            c.city_state_type = Some(
                crate::world::city_states::type_named(&r, "Cultured")
                    .or_else(|| r.city_state_types.ids().next())
                    .expect("the ruleset has city-state types"),
            );
        }
        w.found_city(City::found("Venice", CivId(1), w.grid.id_at(16, 16).unwrap()));
        w.diplomacy.meet(CivId(0), CivId(1));
        w.civ_mut(CivId(0)).unwrap().gold = 4000;

        let mut became = None;
        for turn in 1..=120 {
            for e in advance_turn(&mut w, &r, CityFocus::None, TurnModifiers::default()) {
                if let TurnEvent::CityStateRelationship { to, major, .. } = e {
                    if major == CivId(0) && to >= Relationship::Friend {
                        became.get_or_insert((turn, to));
                    }
                }
            }
            if became.is_some() {
                break;
            }
        }

        let (turn, level) = became.expect("a civ with four thousand gold should buy a friend");
        println!("{level:?} on turn {turn}");
        assert!(matches!(relationship(&w, CivId(1), CivId(0)), Relationship::Friend | Relationship::Ally));

        // And the friendship pays. Cultured city-states grant culture.
        let owed = crate::world::city_states::civ_wide_yields(&w, &r, CivId(0));
        assert!(owed.culture > Fixed::ZERO, "a Cultured ally that grants no culture is a unique nothing reads");

        // Left alone, it lapses — influence is rent.
        let held = w.diplomacy.influence(CivId(1), CivId(0));
        w.civ_mut(CivId(0)).unwrap().gold = 0; // no more gifts
        for _ in 0..200 {
            advance_turn(&mut w, &r, CityFocus::None, TurnModifiers::default());
        }
        assert!(
            w.diplomacy.influence(CivId(1), CivId(0)) < held,
            "favour that never lapses is a purchase, not a relationship"
        );
        assert_eq!(ally_of(&w, CivId(1)), None, "and the alliance is gone with it");
    }

    /// Espionage through the real turn loop: a spy is granted by reaching an
    /// era, posts itself, and brings home a technology nobody researched.
    ///
    /// The module's own tests drive `tick` directly. This one exists because
    /// the granting, the posting and the unique rebuild after a stolen tech
    /// are all wiring, and wiring is where this engine's bugs have lived.
    ///
    /// Espionage does not appear in the 250-turn generated-map game at all:
    /// it starts in the Renaissance and four civs at permanent war reach the
    /// Classical era. That is Civ's own pacing, not a fault, so the era is
    /// granted here rather than waited for.
    #[test]
    fn a_spy_earns_its_era_posts_itself_and_steals() {
        let r = ruleset();
        let mut w = two_civ_world(&r);
        let babylon = w.found_city(City::found("Babylon", CivId(1), w.grid.id_at(16, 16).unwrap()));
        {
            // Babylon needs land of its own to work, or it starves to one
            // citizen and produces one science — `two_civ_world` hands the
            // whole map to civ 0.
            let centre = w.cities.get(babylon).unwrap().centre;
            for t in w.grid.within(centre, 3) {
                w.grid.owner[t.0 as usize] = Some(CivId(1));
            }
            w.cities.get_mut(babylon).unwrap().population = 14;
        }
        // Give the rival a lead worth stealing, and both sides the eras that
        // grant spies.
        for t in r.technologies.ids().take(20) {
            w.civ_mut(CivId(1)).unwrap().techs.insert(t);
        }
        for civ in [CivId(0), CivId(1)] {
            w.civ_mut(civ).unwrap().espionage.grant_for_era(crate::world::espionage::FIRST_SPY_ERA);
        }
        assert_eq!(w.civ(CivId(0)).unwrap().espionage.spies.len(), 1);
        // A spy needs an explored destination; distant cities are not known automatically.
        w.visibility.explore(CivId(0), w.cities.get(babylon).unwrap().centre);
        w.diplomacy.meet(CivId(0), CivId(1));

        let before = w.civ(CivId(0)).unwrap().techs.len();
        // Eighty turns: a few to travel and set up, then the theft. Kept
        // short deliberately — a rival handed twenty technologies also has
        // the units that come with them, and given three hundred turns it
        // conquers the spy's owner and the test stops being about espionage.
        let mut stolen = None;
        for turn in 1..=80 {
            for e in advance_turn(&mut w, &r, CityFocus::None, TurnModifiers::default()) {
                if let TurnEvent::TechStolen { thief, tech, .. } = e {
                    if thief == CivId(0) {
                        stolen.get_or_insert((turn, tech));
                    }
                }
            }
            if stolen.is_some() {
                break;
            }
        }

        let (turn, tech) = stolen.expect("a spy in a rival capital should bring something home");
        println!("stole {} on turn {turn}", r.technologies.name_of(tech).unwrap_or("?"));
        assert!(w.civ(CivId(0)).unwrap().techs.contains(&tech));
        assert!(w.civ(CivId(0)).unwrap().techs.len() > before, "and the count went up");
        // It posted itself without being told where to go.
        let spy = &w.civ(CivId(0)).unwrap().espionage.spies[0];
        assert!(spy.station.is_some() || spy.rank == 1, "still working, or caught and home");
    }

    /// The property the phase split exists for.
    #[test]
    fn planning_does_not_touch_the_world() {
        let r = ruleset();
        let w = two_civ_world(&r);
        let before = w.clone();
        for civ in w.living_civs().collect::<Vec<_>>() {
            let _ = plan_civ_turn(&w, &r, civ, CityFocus::None, TurnModifiers::default());
        }
        assert_eq!(w, before, "the read phase must be pure or it cannot be parallelised");
    }

    #[test]
    fn a_turn_advances_and_reports_what_happened() {
        let r = ruleset();
        let mut w = two_civ_world(&r);

        let events = advance_turn(&mut w, &r, CityFocus::None, TurnModifiers::default());
        assert_eq!(w.turn, 1);
        assert!(matches!(events.last(), Some(TurnEvent::TurnEnded { turn: 1 })));
        assert!(events.iter().any(|e| matches!(e, TurnEvent::CityStored { .. })));
    }

    #[test]
    fn a_city_grows_over_several_turns() {
        let r = ruleset();
        let mut w = two_civ_world(&r);

        let mut grew_on = None;
        for turn in 1..=20 {
            for e in advance_turn(&mut w, &r, CityFocus::None, TurnModifiers::default()) {
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
        let run = || {
            let mut w = two_civ_world(&r);
            let mut log = Vec::new();
            for _ in 0..12 {
                log.extend(advance_turn(&mut w, &r, CityFocus::None, TurnModifiers::default()));
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

        let events = advance_turn(&mut w, &r, CityFocus::None, TurnModifiers::default());
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
            for e in advance_turn(&mut w, &r, CityFocus::None, TurnModifiers::default()) {
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
        advance_turn(&mut w, &r, CityFocus::None, TurnModifiers::default());
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

        let before = w.civ(CivId(0)).unwrap().culture_stored;
        let events = advance_turn(&mut w, &r, CityFocus::None, TurnModifiers::default());
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
        Ruleset::load_dir("../android/assets/jsons/Civ V - Gods & Kings").expect("readable").expect("loads")
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

        let (mut techs, mut builds, mut growths, mut improved) = (0, 0, 0, 0);
        for _ in 0..100 {
            for e in
                advance_turn(&mut w, &r, CityFocus::None, TurnModifiers { difficulty: prince, ..Default::default() })
            {
                match e {
                    TurnEvent::TechLearned { .. } => techs += 1,
                    TurnEvent::CityCompleted { .. } => builds += 1,
                    TurnEvent::CityGrew { .. } => growths += 1,
                    TurnEvent::TileImproved { .. } => improved += 1,
                    _ => {}
                }
            }
        }

        let civ = w.civ(CivId(0)).unwrap();
        let city = w.cities.get(civ.cities[0]).unwrap();
        println!(
            "after 100 turns: {techs} techs, {builds} buildings, {growths} growths, \
             {} cities, capital population {}, {} policies, {} gold, {improved} tiles improved",
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
        assert!(improved > 0, "and improved its land, got {improved}");
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
            TurnModifiers { difficulty: r.difficulties.by_name("Prince"), ..Default::default() },
        );

        let won = events.iter().find_map(|e| match e {
            TurnEvent::Victory { civ, condition } => Some((*civ, *condition)),
            _ => None,
        });
        let (civ, condition) = won.expect("the only capital on the map is a domination victory");
        println!("{:?} wins by {}", civ, r.victories.name_of(condition).unwrap());
        assert_eq!(civ, CivId(0));
        assert_eq!(r.victories.name_of(condition), Some("Domination"));
        let victory_turn = w.turn;
        assert_eq!(victory_turn, 1, "the first completed round is round one");
        assert_eq!(w.victory_turn, Some(victory_turn));
        for _ in 0..3 {
            let later = advance_turn(&mut w, &r, CityFocus::None, TurnModifiers::default());
            assert!(!later.iter().any(|e| matches!(e, TurnEvent::Victory { .. })), "victory is reported once");
            assert_eq!(w.winner, Some((civ, condition)));
            assert_eq!(w.victory_turn, Some(victory_turn), "later rounds preserve the original result date");
        }
        assert_eq!(w.turn, victory_turn + 3, "the legacy API can continue past the win");
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
                    TurnModifiers { difficulty: r.difficulties.by_name("Prince"), ..Default::default() },
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
        let r = Ruleset::load_dir("../android/assets/jsons/Civ V - Gods & Kings").unwrap().unwrap();
        let mut g = TileGrid::new(8, 8, r.terrain("Grassland").unwrap());
        g.owner.iter_mut().for_each(|o| *o = Some(CivId(0)));
        let nation = r.nations.ids().next().unwrap();
        let mut w = World::new(g, vec![Civilization::new(CivId(0), nation, false)]);
        let centre = w.grid.id_at(4, 4).unwrap();
        let city_id = w.found_city_in(&r, City::found("Rome", CivId(0), centre));

        w.civ_mut(CivId(0)).unwrap().techs.insert(r.technologies.id("Agriculture").unwrap());
        let warrior = r.unit("Warrior").unwrap();
        let cost =
            crate::world::construction::production_cost(&r, Construction::Unit(warrior), CostModifiers::default())
                .unwrap();
        w.cities.get_mut(city_id).unwrap().queue.push(Construction::Unit(warrior));

        assert!(w.civ(CivId(0)).unwrap().units.is_empty());
        let plan = TurnPlan {
            civ: CivId(0),
            production: vec![(city_id, cost)],
            ..TurnPlan {
                civ: CivId(0),
                assignments: Vec::new(),
                specialists: Vec::new(),
                food: Vec::new(),
                production: Vec::new(),
                border_culture: Vec::new(),
                purchases: Vec::new(),
                faith_purchase: None,
                science: 0,
                faith: 0,
                culture: 0,
                gold: 0,
                happiness: 0,
                research: None,
                policy: None,
                construction: Vec::new(),
                orders: Vec::new(),
                declare_war_on: Vec::new(),
                make_peace_with: Vec::new(),
                denounce: Vec::new(),
                tribute: Vec::new(),
                trades: Vec::new(),
                answers: Vec::new(),
                demands: Vec::new(),
                gift: None,
            }
        };
        let events = apply_plan(&mut w, &r, &plan, TurnModifiers::default());

        assert!(events.iter().any(|e| matches!(e, TurnEvent::CityCompleted { .. })));
        let units = &w.civ(CivId(0)).unwrap().units;
        assert_eq!(units.len(), 1, "the warrior should exist");
        assert_eq!(w.units.get(units[0]).unwrap().position, centre, "and stand in the city");
    }
}

#[cfg(test)]
mod multi_civ {
    use super::*;
    use crate::world::entities::Civilization;
    use crate::world::grid::TileGrid;

    fn ruleset() -> Ruleset {
        Ruleset::load_dir("../android/assets/jsons/Civ V - Gods & Kings").expect("readable").expect("loads")
    }

    /// Three civs on one map, left to themselves. Everything up to now has
    /// been one civ alone, so contention for land, meeting each other and the
    /// turn order across civs are all untested in a running game.
    #[test]
    fn three_civs_share_a_map() {
        let r = ruleset();
        let mut g = TileGrid::new(20, 20, r.terrain("Grassland").unwrap());
        let hill = r.terrain("Hill").unwrap();
        for t in g.ids().filter(|t| t.0 % 4 == 0) {
            g.features[t.0 as usize].push(hill);
        }
        let nation = r.nations.ids().next().unwrap();
        let civs: Vec<_> = (0..3u8).map(|i| Civilization::new(CivId(i), nation, true)).collect();
        let mut w = World::new(g, civs);

        // Spread them out, each owning its own starting ring.
        for (i, (x, y)) in [(4u16, 4u16), (15, 4), (9, 15)].into_iter().enumerate() {
            let centre = w.grid.id_at(x, y).unwrap();
            for t in w.grid.within(centre, 3) {
                w.grid.owner[t.0 as usize] = Some(CivId(i as u8));
            }
            w.found_city_in(&r, City::found(format!("Capital {i}"), CivId(i as u8), centre));
        }

        let prince = r.difficulties.by_name("Prince");

        let mut founded = 0;
        let mut victories = 0;
        let mut meetings = 0;
        let mut wars = 0;
        let mut captures = 0;
        let mut peaces = 0;
        let mut battles = 0;
        let mut losses = 0;
        for _ in 0..500 {
            for e in
                advance_turn(&mut w, &r, CityFocus::None, TurnModifiers { difficulty: prince, ..Default::default() })
            {
                match e {
                    TurnEvent::CityFounded { .. } => founded += 1,
                    TurnEvent::Victory { .. } => victories += 1,
                    TurnEvent::CivsMet { .. } => meetings += 1,
                    TurnEvent::WarDeclared { .. } => wars += 1,
                    TurnEvent::PeaceMade { .. } => peaces += 1,
                    TurnEvent::CityCaptured { .. } => captures += 1,
                    TurnEvent::Battle { attacker_died, defender_died, .. } => {
                        battles += 1;
                        losses += attacker_died as u32 + defender_died as u32;
                    }
                    _ => {}
                }
            }
        }

        for civ in w.civs.iter() {
            println!(
                "civ {:?}: {} cities, {} techs, {} policies, {} gold, {} units",
                civ.id,
                civ.cities.len(),
                civ.techs.len(),
                civ.adopted_policies.len(),
                civ.gold,
                civ.units.len()
            );
        }
        println!(
            "cities founded: {founded}, first contacts: {meetings}, wars: {wars}, \
             battles: {battles}, units lost: {losses}, cities taken: {captures}, \
             peaces: {peaces}, \
             victories: {victories}"
        );
        for a in 0..3u8 {
            for b in (a + 1)..3u8 {
                println!(
                    "  {a} and {b}: met={} at war={}",
                    w.diplomacy.have_met(CivId(a), CivId(b)),
                    w.diplomacy.at_war(CivId(a), CivId(b))
                );
            }
        }
        println!("total cities on the map: {}", w.cities.len());

        assert_eq!(w.civs.len(), 3, "everyone is still in the list, alive or not");
        // The three starting capitals precede the event counter. Require
        // actual new settlement during play even if conquests later raze it.
        assert!(founded > 0, "someone should have expanded");
        // This shared-map scenario exercises expansion, contact, warfare,
        // capture and peace over five hundred rounds.
        assert!(captures > 0, "and some changed hands");
        // Wars used to be permanent, so one declaration ground on forever.
        // Now they start and stop: roughly as many settlements as outbreaks.
        assert!(wars > 1 && peaces > 0, "{wars} wars, {peaces} peaces");
        // Legal wonder/resource contention can leave two balanced survivors.
        // A fixed number of AI rounds need not force a winner. The explicit
        // victory fixture above verifies the mandatory exactly-once event.
        assert_eq!(victories, usize::from(w.winner.is_some()), "a win is reported once");
        if let Some((winner, condition)) = w.winner {
            println!("{winner:?} wins by {}", r.victories.name_of(condition).unwrap());
            assert_eq!(crate::world::victory::winner(&w, &r), Some((winner, condition)));
        }

        // Everyone finds everyone. Before exploration existed this was zero:
        // settlers and workers stay near home, so sight never reached a
        // neighbour and three civs on one map were three separate games.
        assert_eq!(meetings, 3, "three civs means three pairs, each meeting once");
        for a in 0..3u8 {
            for b in (a + 1)..3u8 {
                assert!(w.diplomacy.have_met(CivId(a), CivId(b)));
            }
        }
        // Meeting is not war, but a clear advantage invites one.
        assert!(wars > 0, "someone should have taken their chance");
    }
}

#[cfg(test)]
mod contact {
    use super::*;
    use crate::world::entities::{Civilization, Unit};
    use crate::world::grid::TileGrid;

    #[test]
    fn seeing_a_neighbour_is_meeting_them() {
        let r = Ruleset::load_dir("../android/assets/jsons/Civ V - Gods & Kings").unwrap().unwrap();
        let g = TileGrid::new(16, 16, r.terrain("Grassland").unwrap());
        let nation = r.nations.ids().next().unwrap();
        let mut w =
            World::new(g, vec![Civilization::new(CivId(0), nation, true), Civilization::new(CivId(1), nation, true)]);
        let base = r.unit("Warrior").unwrap();
        let here = w.grid.id_at(8, 8).unwrap();
        let far = w.grid.id_at(1, 1).unwrap();
        w.place_unit(Unit::new(base, CivId(0), here, 2));
        let theirs = w.place_unit(Unit::new(base, CivId(1), far, 2));

        update_visibility(&mut w);
        assert!(detect_first_contact(&mut w).is_empty(), "too far apart to see");
        assert!(!w.diplomacy.have_met(CivId(0), CivId(1)));

        // Walk the stranger into view.
        let next_door = w.grid.neighbours(here).next().unwrap();
        w.units.get_mut(theirs).unwrap().position = next_door;
        update_visibility(&mut w);

        let events = detect_first_contact(&mut w);
        assert_eq!(events.len(), 1, "one meeting, reported once");
        assert!(matches!(events[0], TurnEvent::CivsMet { .. }));
        assert!(w.diplomacy.have_met(CivId(0), CivId(1)));
        assert!(!w.diplomacy.at_war(CivId(0), CivId(1)), "meeting is not war");

        // And it does not keep firing.
        update_visibility(&mut w);
        assert!(detect_first_contact(&mut w).is_empty(), "already acquainted");
    }
}

#[cfg(test)]
mod warfare {
    use super::*;
    use crate::world::entities::{Civilization, Unit};
    use crate::world::grid::TileGrid;

    /// Two civs at war with soldiers in reach should fight, through the turn
    /// loop rather than by calling combat directly.
    #[test]
    fn armies_at_war_actually_fight() {
        let r = Ruleset::load_dir("../android/assets/jsons/Civ V - Gods & Kings").unwrap().unwrap();
        let mut g = TileGrid::new(12, 12, r.terrain("Grassland").unwrap());
        g.owner.iter_mut().for_each(|o| *o = Some(CivId(0)));
        let nation = r.nations.ids().next().unwrap();
        let mut w =
            World::new(g, vec![Civilization::new(CivId(0), nation, true), Civilization::new(CivId(1), nation, true)]);
        w.found_city_in(&r, City::found("Rome", CivId(0), w.grid.id_at(3, 3).unwrap()));
        w.found_city_in(&r, City::found("Carthage", CivId(1), w.grid.id_at(9, 9).unwrap()));

        let warrior = r.unit("Warrior").unwrap();
        let movement = r.units.get(warrior).unwrap().movement;
        let a = w.grid.id_at(6, 6).unwrap();
        let b = w.grid.neighbours(a).next().unwrap();
        w.place_unit(Unit::new(warrior, CivId(0), a, movement));
        w.place_unit(Unit::new(warrior, CivId(1), b, movement));

        w.diplomacy.meet(CivId(0), CivId(1));
        w.diplomacy.declare_war(CivId(0), CivId(1));

        let prince = r.difficulties.by_name("Prince");

        let mut battles = 0;
        let mut deaths = 0;
        for _ in 0..30 {
            for e in
                advance_turn(&mut w, &r, CityFocus::None, TurnModifiers { difficulty: prince, ..Default::default() })
            {
                if let TurnEvent::Battle { attacker_died, defender_died, .. } = e {
                    battles += 1;
                    deaths += attacker_died as u32 + defender_died as u32;
                }
            }
        }
        println!("{battles} battles, {deaths} units destroyed");
        assert!(battles > 0, "adjacent enemies at war should fight");
        assert!(deaths > 0, "and someone should eventually lose");
    }
}
