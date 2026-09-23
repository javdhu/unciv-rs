//! The player action boundary. Failed commands never change the world.
//!
//! Commands express intent; callers cannot supply derived income or combat
//! outcomes. The staging copy also contains mutations made by entry effects.
use std::collections::BTreeSet;

use crate::arena::Id;
use crate::ruleset::{BaseUnit, Belief, Policy, Promotion, Ruleset, Technology, TileImprovement};
use crate::stats::{Fixed, Stat};
use crate::world::city::City;
use crate::world::construction::Construction;
use crate::world::diplomacy::Demand;
use crate::world::entities::Unit;
use crate::world::grid::{CivId, TileId};
use crate::world::specialists::Allocation;
use crate::world::trade::Item;
use crate::world::turn::{TurnEvent, TurnModifiers};
use crate::world::world::World;
use crate::UniqueType;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttackTarget {
    Unit(Id<Unit>),
    City(Id<City>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    MoveUnit { unit: Id<Unit>, to: TileId },
    Attack { unit: Id<Unit>, target: AttackTarget },
    NuclearStrike { unit: Id<Unit>, target: TileId },
    Rebase { unit: Id<Unit>, to: TileId },
    AirSweep { unit: Id<Unit>, target: TileId },
    Paradrop { unit: Id<Unit>, to: TileId },
    Bombard { city: Id<City>, target: Id<Unit> },
    FoundCity { unit: Id<Unit>, name: String },
    Improve { unit: Id<Unit>, improvement: Id<TileImprovement> },
    Fortify { unit: Id<Unit> },
    SetUp { unit: Id<Unit> },
    SkipUnit { unit: Id<Unit> },
    Promote { unit: Id<Unit>, promotion: Id<Promotion> },
    Upgrade { unit: Id<Unit> },
    Pillage { unit: Id<Unit> },
    DisbandUnit { unit: Id<Unit> },
    GiftUnit { unit: Id<Unit> },
    PledgeProtection { target: CivId },
    WithdrawProtection { target: CivId },
    LiberateCity { city: Id<City> },
    SellBuilding { city: Id<City>, building: Id<crate::ruleset::Building> },
    BuyTile { city: Id<City>, tile: TileId },
    StopRazing { city: Id<City> },
    WithdrawSpy { spy: usize },
    AttemptCoup { target: CivId },
    SetProduction { city: Id<City>, queue: Vec<Construction> },
    SetProductionConversion { city: Id<City>, stat: Option<Stat> },
    SetAvoidGrowth { city: Id<City>, avoid: bool },
    SetResearch { technology: Id<Technology> },
    Purchase { city: Id<City>, what: Construction, currency: Stat },
    AdoptPolicy { policy: Id<Policy> },
    AssignCitizens { city: Id<City>, worked: Vec<TileId>, specialists: Allocation },
    GreatPerson { unit: Id<Unit>, action: crate::world::great_person_actions::Action },
    Religious { unit: Id<Unit>, action: crate::world::religious_units::ReligiousAction },
    ChoosePantheon { belief: Id<Belief> },
    ChooseReligionBeliefs { unit: Id<Unit>, beliefs: Vec<Id<Belief>>, enhancing: bool },
    ClaimFreeTechnology { technology: Id<Technology> },
    ClaimFreeGreatPerson { unit: Id<BaseUnit> },
    AssembleSpaceshipPart { unit: Id<Unit> },
    VoteDiplomatic { candidate: Option<CivId> },
    Annex { city: Id<City> },
    Raze { city: Id<City> },
    AssignSpy { spy: usize, city: Id<City> },
    DeclareWar { target: CivId },
    OfferPeace { target: CivId },
    OfferPeaceTreaty { target: CivId, give: Vec<Item>, get: Vec<Item> },
    OfferFriendship { target: CivId },
    OfferDefensivePact { target: CivId },
    OfferResearchAgreement { target: CivId },
    Denounce { target: CivId },
    Demand { target: CivId, demand: Demand },
    AnswerDemand { by: CivId, demand: Demand, agree: bool },
    GiftGold { target: CivId, gold: i32 },
    BuyCityState { target: CivId },
    OfferTrade { target: CivId, give: Vec<Item>, get: Vec<Item> },
    DemandTribute { target: CivId, worker: bool },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandRejection {
    UnknownCiv,
    Defeated,
    GameFinished,
    NotOwner,
    InvalidTarget,
    NoMovement,
    NotAllowed,
    Unavailable,
    NotEnoughResources,
}
impl std::fmt::Display for CommandRejection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for CommandRejection {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActionOutcome {
    pub events: Vec<TurnEvent>,
}

pub fn execute(
    world: &mut World,
    r: &Ruleset,
    civ: CivId,
    command: Command,
) -> Result<ActionOutcome, CommandRejection> {
    execute_with(world, r, civ, command, TurnModifiers::for_world(world, r))
}

pub fn execute_with(
    world: &mut World,
    r: &Ruleset,
    civ: CivId,
    command: Command,
    mods: TurnModifiers,
) -> Result<ActionOutcome, CommandRejection> {
    let mods = TurnModifiers { difficulty: mods.difficulty.or_else(|| world.game_difficulty(r)), ..mods };
    validate_actor(world, civ, &command)?;
    let mut staged = world.clone();
    let events = apply(&mut staged, r, civ, command, mods)?;
    *world = staged;
    Ok(ActionOutcome { events })
}

fn validate_actor(world: &World, civ: CivId, command: &Command) -> Result<(), CommandRejection> {
    use Command::*;
    let player = world.civ(civ).ok_or(CommandRejection::UnknownCiv)?;
    if !player.is_alive() {
        return Err(CommandRejection::Defeated);
    }
    if world.winner.is_some() {
        return Err(CommandRejection::GameFinished);
    }
    match command {
        MoveUnit { unit, .. }
        | Attack { unit, .. }
        | NuclearStrike { unit, .. }
        | Rebase { unit, .. }
        | AirSweep { unit, .. }
        | Paradrop { unit, .. }
        | FoundCity { unit, .. }
        | Improve { unit, .. }
        | Fortify { unit }
        | SkipUnit { unit }
        | SetUp { unit }
        | Promote { unit, .. }
        | Upgrade { unit }
        | Pillage { unit }
        | DisbandUnit { unit }
        | GiftUnit { unit }
        | GreatPerson { unit, .. }
        | Religious { unit, .. }
        | ChooseReligionBeliefs { unit, .. }
        | AssembleSpaceshipPart { unit } => {
            if world.units.get(*unit).is_none_or(|u| u.owner != civ) {
                return Err(CommandRejection::NotOwner);
            }
        }
        Bombard { city, .. }
        | SetProduction { city, .. }
        | SetProductionConversion { city, .. }
        | SetAvoidGrowth { city, .. }
        | Purchase { city, .. }
        | AssignCitizens { city, .. }
        | Annex { city }
        | Raze { city }
        | StopRazing { city }
        | LiberateCity { city }
        | SellBuilding { city, .. }
        | BuyTile { city, .. }
            if world.cities.get(*city).is_none_or(|c| c.owner != civ) =>
        {
            return Err(CommandRejection::NotOwner);
        }
        _ => {}
    }
    Ok(())
}
fn target_tile(world: &World, tile: TileId) -> Result<(), CommandRejection> {
    if tile.0 as usize >= world.grid.len() {
        Err(CommandRejection::InvalidTarget)
    } else {
        Ok(())
    }
}
fn moving(world: &World, unit: Id<Unit>) -> Result<(), CommandRejection> {
    if world.units.get(unit).is_some_and(|u| u.can_move()) {
        Ok(())
    } else {
        Err(CommandRejection::NoMovement)
    }
}
fn enemy(world: &World, civ: CivId, other: CivId, tile: TileId) -> Result<(), CommandRejection> {
    if !world.visibility.can_see(civ, tile) {
        return Err(CommandRejection::InvalidTarget);
    }
    if civ == other || !world.diplomacy.at_war(civ, other) {
        return Err(CommandRejection::NotAllowed);
    }
    Ok(())
}
fn known_party(world: &World, civ: CivId, target: CivId) -> Result<(), CommandRejection> {
    if civ == target || !world.civ(target).is_some_and(|c| c.is_alive()) || !world.diplomacy.have_met(civ, target) {
        return Err(CommandRejection::InvalidTarget);
    }
    Ok(())
}
fn trade_items(items: &[Item]) -> bool {
    // Reject duplicate categories, not merely duplicate values: the legacy
    // helper checks items independently and cannot safely sum repeated gold.
    let mut categories = BTreeSet::new();
    items.iter().all(|item| {
        let key = match item {
            Item::Gold(n) if *n > 0 => (0, None),
            Item::GoldPerTurn(n) if *n > 0 => (1, None),
            Item::Luxury(id) => (2, Some((id.index(), id.generation()))),
            Item::Strategic(id, n) if *n > 0 => (2, Some((id.index(), id.generation()))),
            Item::OpenBorders => (3, None),
            Item::Embassy => (4, None),
            Item::City(id) => (5, Some((id.index(), id.generation()))),
            _ => return false,
        };
        categories.insert(key)
    })
}

fn apply(
    world: &mut World,
    r: &Ruleset,
    civ: CivId,
    command: Command,
    mods: TurnModifiers,
) -> Result<Vec<TurnEvent>, CommandRejection> {
    use Command::*;
    use CommandRejection::*;
    let mut events = Vec::new();
    match command {
        MoveUnit { unit, to } => {
            target_tile(world, to)?;
            moving(world, unit)?;
            if world.units.get(unit).is_some_and(|u| u.is_air(r)) {
                return Err(NotAllowed);
            }
            let path = crate::world::fog::preview_path(world, r, civ, unit, to).ok_or(InvalidTarget)?;
            if path.is_empty() {
                return Err(InvalidTarget);
            }
            let sites = crate::world::natural_wonders::sites(&world.grid);
            // Occupied tiles may be crossed, but never become a stopping
            // point. Keep the last legal stop until a transit segment exits;
            // an unseen obstacle or entry effect may end movement early.
            let mut transit_checkpoint: Option<(World, usize)> = None;
            for next in path {
                let Some(u) = world.units.get(unit) else { break };
                if !u.can_move() {
                    break;
                }
                let from = u.position;
                if world.grid.distance(from, next) != 1 {
                    break;
                }
                // Use the actual adjacent step only. Routing again with the
                // full world could silently detour around an unseen army.
                if !crate::world::movement::can_traverse(world, r, u, from, next)
                    || !crate::world::movement::can_pass_through(world, r, next, u)
                    || (next == to && crate::world::movement::is_tile_occupied_for(world, r, next, u))
                {
                    break;
                }
                let Some(cost) = crate::world::movement::step_cost_for_unit(world, r, u, from, next, u.movement_left)
                else {
                    break;
                };
                let cost = cost.min(u.movement_left);
                let embarked = u.is_land_unit(r)
                    && world.grid.is_water(r, next)
                    && !u.has_unique(r, crate::UniqueType::CanMoveOnWater);
                if transit_checkpoint.is_none()
                    && (!world.units_at(next).is_empty()
                        || world.cities.iter().any(|(_, c)| c.centre == next && c.owner != civ))
                {
                    transit_checkpoint = Some((world.clone(), events.len()));
                }
                let u = world.units.get_mut(unit).expect("validated unit");
                u.position = next;
                u.movement_left -= cost;
                u.moved_this_turn = true;
                u.embarked = embarked;
                u.fortified = false;
                u.fortification_turns = 0;
                u.set_up = false;
                crate::world::air::move_payload(world, unit, next);
                crate::world::military::on_enter(world, r, unit);
                events.push(TurnEvent::UnitMoved(crate::world::movement::MoveEvent {
                    unit,
                    from,
                    to: next,
                    spent: cost,
                }));
                crate::world::turn::explore_on_entry(world, r, unit, mods, &sites, &mut events);
                let can_stop = world.units.get(unit).is_none_or(|moving| {
                    !world.units.iter().any(|(other_id, other)| {
                        other_id != unit
                            && !other.is_air(r)
                            && other.position == moving.position
                            && (other.owner != moving.owner || other.is_civilian(r) == moving.is_civilian(r))
                    }) && !world.cities.iter().any(|(_, c)| c.centre == moving.position && c.owner != moving.owner)
                });
                if can_stop {
                    transit_checkpoint = None;
                }
            }
            if let Some((checkpoint, event_count)) = transit_checkpoint {
                *world = checkpoint;
                events.truncate(event_count);
            }
        }
        Attack { unit, target } => {
            moving(world, unit)?;
            let u = world.units.get(unit).expect("validated unit");
            if !u.is_military(r) || u.attacks_left == 0 || u.has_unique(r, UniqueType::NuclearWeapon) {
                return Err(NotAllowed);
            }
            let before = (u.position, u.movement_left);
            let air = u.is_air(r);
            let ranged = u.is_ranged(r);
            let (tile, owner) = match target {
                AttackTarget::Unit(id) => world
                    .units
                    .get(id)
                    .filter(|_| crate::world::fog::can_see_unit(world, r, civ, id))
                    .map(|u| (u.position, u.owner)),
                AttackTarget::City(id) => world.cities.get(id).map(|c| (c.centre, c.owner)),
            }
            .ok_or(InvalidTarget)?;
            enemy(world, civ, owner, tile)?;
            let watch = crate::world::combat::BattleWatch::before(world, r, unit, match target {
                AttackTarget::Unit(id) => crate::world::air::Target::Unit(id),
                AttackTarget::City(id) => crate::world::air::Target::City(id),
            });
            match target {
                AttackTarget::Unit(target) => {
                    // Tile-based helpers choose the first defender. Require
                    // this exact target to prevent an order hitting another unit.
                    if (air || ranged)
                        && world
                            .units_at(tile)
                            .into_iter()
                            .find(|id| world.units.get(*id).is_some_and(|u| u.owner != civ))
                            != Some(target)
                    {
                        return Err(InvalidTarget);
                    }
                    if air && world.cities.iter().any(|(_, c)| c.centre == tile) {
                        return Err(InvalidTarget);
                    }
                    let result = if air {
                        crate::world::combat::air_strike(world, r, unit, tile)
                    } else if ranged {
                        crate::world::combat::resolve_ranged_attack(world, r, unit, tile)
                    } else {
                        crate::world::combat::resolve_attack(world, r, unit, target)
                    }
                    .map_err(|_| NotAllowed)?;
                    events.push(TurnEvent::Battle {
                        attacker_died: result.attacker_died,
                        defender_died: result.defender_died,
                        report: watch.map(|w| w.report(world)),
                    });
                    if let Some(after) = world.units.get(target).filter(|u| u.position != tile) {
                        events.push(TurnEvent::UnitMoved(crate::world::movement::MoveEvent {
                            unit: target,
                            from: tile,
                            to: after.position,
                            spent: Fixed::ZERO,
                        }));
                        let sites = crate::world::natural_wonders::sites(&world.grid);
                        crate::world::turn::explore_on_entry(world, r, target, mods, &sites, &mut events);
                    }
                }
                AttackTarget::City(city) => {
                    if air {
                        let result = crate::world::combat::air_strike(world, r, unit, tile).map_err(|_| NotAllowed)?;
                        events.push(TurnEvent::Battle {
                            attacker_died: result.attacker_died,
                            defender_died: result.defender_died,
                            report: watch.map(|w| w.report(world)),
                        });
                    } else {
                        let result = if ranged {
                            crate::world::combat::attack_city_ranged(world, r, unit, city)
                        } else {
                            crate::world::combat::attack_city(world, r, unit, city)
                        }
                        .map_err(|_| NotAllowed)?;
                        events.push(TurnEvent::Battle {
                            attacker_died: result.attacker_died,
                            defender_died: false,
                            report: watch.map(|w| w.report(world)),
                        });
                        if result.captured {
                            events.push(TurnEvent::CityCaptured { city, by: civ });
                        }
                    }
                }
            }
            if let Some(after) = world.units.get(unit) {
                if before.0 != after.position {
                    events.push(TurnEvent::UnitMoved(crate::world::movement::MoveEvent {
                        unit,
                        from: before.0,
                        to: after.position,
                        spent: (before.1 - after.movement_left).max(Fixed::ZERO),
                    }));
                    let sites = crate::world::natural_wonders::sites(&world.grid);
                    crate::world::turn::explore_on_entry(world, r, unit, mods, &sites, &mut events);
                }
            }
        }
        NuclearStrike { unit, target } => {
            target_tile(world, target)?;
            moving(world, unit)?;
            let u = world.units.get(unit).expect("validated unit");
            if !u.has_unique(r, UniqueType::NuclearWeapon)
                || u.attacks_left == 0
                || !world.visibility.has_explored(civ, target)
            {
                return Err(NotAllowed);
            }
            crate::world::nuclear::can_strike(world, r, unit, target).map_err(|_| NotAllowed)?;
            // Launching is an explicit hostile action; diplomatic victims
            // include collateral damage, and declarations precede the blast.
            let base = r.units.get(u.base).ok_or(Unavailable)?;
            let radius = base
                .uniques
                .iter()
                .find(|u| u.text.starts_with("Blast radius ["))
                .and_then(|u| u.params.first())
                .and_then(|p| p.parse().ok())
                .unwrap_or(2);
            let tiles: BTreeSet<_> = world.grid.within(target, radius).into_iter().collect();
            let victims: BTreeSet<_> = world
                .units
                .iter()
                .filter(|(_, u)| tiles.contains(&u.position))
                .map(|(_, u)| u.owner)
                .chain(world.grid.ids().filter(|t| tiles.contains(t)).filter_map(|t| world.grid.owner[t.0 as usize]))
                .filter(|c| *c != civ)
                .collect();
            for target in victims {
                events.extend(crate::world::diplomacy_actions::declare_war(world, r, civ, target));
            }
            crate::world::combat::nuclear_strike(world, r, unit, target).map_err(|_| NotAllowed)?;
        }
        Rebase { unit, to } => {
            target_tile(world, to)?;
            crate::world::combat::rebase_air_unit(world, r, unit, to).map_err(|_| NotAllowed)?;
        }
        AirSweep { unit, target } => {
            target_tile(world, target)?;
            let result = crate::world::air::sweep(world, r, unit, target).map_err(|_| NotAllowed)?;
            events.push(TurnEvent::Battle { attacker_died: result.attacker_died, defender_died: result.defender_died, report: None });
        }
        Paradrop { unit, to } => {
            target_tile(world, to)?;
            let moved = crate::world::air::paradrop(world, r, unit, to).map_err(|_| NotAllowed)?;
            events.push(TurnEvent::UnitMoved(moved));
            let sites = crate::world::natural_wonders::sites(&world.grid);
            crate::world::turn::explore_on_entry(world, r, unit, mods, &sites, &mut events);
        }
        Bombard { city, target } => {
            let c = world.cities.get(city).expect("validated city");
            if c.attacks_left == 0 || c.resistance > 0 {
                return Err(NotAllowed);
            }
            let u = world
                .units
                .get(target)
                .filter(|_| crate::world::fog::can_see_unit(world, r, civ, target))
                .ok_or(InvalidTarget)?;
            enemy(world, civ, u.owner, u.position)?;
            let result = crate::world::combat::city_bombard(world, r, city, target).map_err(|_| NotAllowed)?;
            world.cities.get_mut(city).expect("validated city").attacks_left -= 1;
            events.push(TurnEvent::Battle { attacker_died: false, defender_died: result.defender_died, report: None });
        }
        FoundCity { unit, name } => {
            moving(world, unit)?;
            if name.trim().is_empty() || name.len() > 128 {
                return Err(InvalidTarget);
            }
            let city = crate::world::settling::found_city_with(world, r, unit, name).map_err(|_| NotAllowed)?;
            crate::world::promises::on_city_founded(world, city);
            // A human's new city works its first tile now; the AI's governor places it when it plans.
            if world.civ(civ).is_some_and(|c| !c.is_ai) {
                crate::world::assignment::place_idle_citizens(world, r, city);
            }
            events.push(TurnEvent::CityFounded { city, by: civ });
        }
        Improve { unit, improvement } => {
            moving(world, unit)?;
            let u = world.units.get(unit).expect("validated unit");
            let tile = u.position;
            let result =
                crate::world::improvements::perform_work(world, r, unit, improvement).map_err(|_| NotAllowed)?;
            if let crate::world::improvements::WorkEvent::Finished { improvement, .. } = result {
                events.push(TurnEvent::TileImproved { tile, improvement });
            }
        }
        Fortify { unit } => {
            moving(world, unit)?;
            let u = world.units.get_mut(unit).expect("validated unit");
            if !u.is_military(r)
                || !u.is_land_unit(r)
                || u.embarked
                || u.has_unique(r, UniqueType::NoDefensiveTerrainBonus)
            {
                return Err(NotAllowed);
            }
            u.fortified = true;
            u.movement_left = Fixed::ZERO;
        }
        SkipUnit { unit } => {
            moving(world, unit)?;
            world.units.get_mut(unit).expect("validated unit").movement_left = Fixed::ZERO;
        }
        SetUp { unit } => {
            if !crate::world::military::set_up(world, r, unit) {
                return Err(NotAllowed);
            }
        }
        Promote { unit, promotion } => {
            crate::world::military::promote(world, r, unit, promotion).map_err(|_| Unavailable)?;
        }
        Upgrade { unit } => {
            moving(world, unit)?;
            crate::world::combat::upgrade_unit_with(world, r, unit, mods.costs.speed).map_err(|_| Unavailable)?;
        }
        Pillage { unit } => {
            moving(world, unit)?;
            let u = world.units.get(unit).expect("validated unit");
            let tile = u.position;
            if !u.is_military(r)
                || u.is_air(r)
                || world.cities.iter().any(|(_, c)| c.centre == tile)
                || world.grid.owner[tile.0 as usize]
                    .is_some_and(|owner| owner == civ || !world.diplomacy.at_war(civ, owner))
            {
                return Err(NotAllowed);
            }
            crate::world::combat::pillage(world, r, unit).ok_or(NotAllowed)?;
        }
        SetProductionConversion { city, stat } => {
            if !crate::world::administration::can_convert_production(world, r, city, stat) {
                return Err(NotAllowed);
            }
            let c = world.cities.get_mut(city).unwrap();
            c.queue.clear();
            c.production_conversion = stat;
        }
        SetAvoidGrowth { city, avoid } => {
            let c = world.cities.get_mut(city).unwrap();
            if c.puppet {
                return Err(NotAllowed);
            }
            c.avoid_growth = avoid;
        }
        SetProduction { city, queue } => {
            if queue.len() > 32 {
                return Err(InvalidTarget);
            }
            let c = world.cities.get(city).expect("validated city");
            if c.puppet || c.resistance > 0 {
                return Err(NotAllowed);
            }
            for what in &queue {
                crate::world::construction::can_construct_in(world, r, civ, c, *what).map_err(|_| Unavailable)?;
            }
            if queue
                .iter()
                .filter_map(|w| if let Construction::Building(b) = w { Some(b) } else { None })
                .collect::<BTreeSet<_>>()
                .len()
                != queue.iter().filter(|w| matches!(w, Construction::Building(_))).count()
            {
                return Err(InvalidTarget);
            }
            let c = world.cities.get_mut(city).expect("validated city");
            // Queue order never transfers or destroys item-specific investment.
            c.queue = queue;
            c.production_conversion = None;
        }
        SetResearch { technology } => {
            let c = world.civ_mut(civ).expect("validated civ");
            crate::world::research::can_research(r, c, technology).map_err(|_| Unavailable)?;
            c.researching = Some(technology);
            let income = crate::world::turn::plan_economy(world, r, civ, crate::world::CityFocus::None, mods).science;
            if let Some(crate::world::research::ResearchEvent::Completed { tech, .. }) =
                crate::world::research::update_research_progress(world, r, civ, income, mods.science)
            {
                crate::world::effects::on_technology_researched(world, r, civ, tech, mods.costs.speed);
                events.push(TurnEvent::TechLearned { civ, tech });
            }
        }
        Purchase { city, what, currency } => {
            if !matches!(currency, Stat::Gold | Stat::Faith) {
                return Err(NotAllowed);
            }
            if let Construction::Unit(base) = what {
                let centre = world.cities.get(city).expect("validated city").centre;
                let prospective = Unit::new(base, civ, centre, r.units.get(base).ok_or(Unavailable)?.movement);
                if crate::world::movement::is_tile_occupied_for(world, r, centre, &prospective) {
                    return Err(NotAllowed);
                }
            }
            let paid =
                crate::world::purchase::purchase_with_science(world, r, city, what, currency, mods.costs, mods.science)
                    .map_err(|_| Unavailable)?;
            events.push(if currency == Stat::Gold {
                TurnEvent::Purchased { city, what, gold: paid }
            } else {
                TurnEvent::FaithPurchased { city, what, faith: paid }
            });
        }
        AdoptPolicy { policy } => {
            let result = crate::world::policies::apply_culture(world, r, civ, 0, Some(policy), mods.policies)
                .map_err(|_| Unavailable)?;
            if !matches!(result, crate::world::policies::PolicyEvent::Adopted { .. }) {
                return Err(NotEnoughResources);
            }
        }
        AssignCitizens { city, worked, specialists } => {
            let c = world.cities.get(city).expect("validated city");
            if c.puppet {
                return Err(NotAllowed);
            }
            let count = specialists
                .values()
                .try_fold(worked.len() as u32, |sum, n| sum.checked_add(*n))
                .ok_or(InvalidTarget)?;
            let slots = crate::world::specialists::slots(r, c);
            let workable = crate::world::city::workable_tiles(&world.grid, c);
            if count > c.population
                || worked.iter().copied().collect::<BTreeSet<_>>().len() != worked.len()
                || worked.iter().any(|t| {
                    !workable.contains(t)
                        || world.cities.iter().any(|(id, c)| id != city && (c.centre == *t || c.worked.contains(t)))
                })
                || specialists.iter().any(|(kind, n)| *n == 0 || *n > slots.get(kind).copied().unwrap_or(0))
            {
                return Err(InvalidTarget);
            }
            let c = world.cities.get_mut(city).expect("validated city");
            c.worked = worked;
            c.specialists = specialists;
        }
        GreatPerson { unit, action } => {
            moving(world, unit)?;
            let scaling = crate::world::great_person_actions::Scaling { costs: mods.costs, science: mods.science };
            let outcome =
                crate::world::great_person_actions::perform(world, r, unit, action, scaling).map_err(|_| NotAllowed)?;
            events.push(TurnEvent::GreatPersonActed { civ, outcome });
        }
        Religious { unit, action } => {
            moving(world, unit)?;
            use crate::world::religious_units::ReligiousAction;
            if let ReligiousAction::SpreadReligion(city) | ReligiousAction::RemoveHeresy(city) = action {
                let tile = world.cities.get(city).ok_or(InvalidTarget)?.centre;
                if !world.visibility.can_see(civ, tile) {
                    return Err(InvalidTarget);
                }
            }
            let event = crate::world::religious_units::perform_with_speed(world, r, unit, action, mods.costs.speed)
                .map_err(|_| NotAllowed)?;
            events.extend(crate::world::turn::religious_events(civ, [event]));
        }
        ChooseReligionBeliefs { unit, beliefs, enhancing } => {
            moving(world, unit)?;
            let event =
                crate::world::religious_units::choose_beliefs(world, r, unit, beliefs, enhancing, mods.costs.speed)
                    .map_err(|_| NotAllowed)?;
            events.extend(crate::world::turn::religious_events(civ, [event]));
        }
        AssembleSpaceshipPart { unit } => {
            let event = crate::world::victory_actions::assemble(world, r, unit).ok_or(NotAllowed)?;
            events.push(TurnEvent::Endgame(event));
        }
        VoteDiplomatic { candidate } => {
            if !crate::world::victory_actions::cast_vote(world, civ, candidate) {
                return Err(NotAllowed);
            }
        }
        ClaimFreeTechnology { technology } => {
            if !crate::world::effects::claim_free_technology(world, r, civ, technology, mods.costs.speed) {
                return Err(NotAllowed);
            }
            events.push(TurnEvent::TechLearned { civ, tech: technology });
        }
        ClaimFreeGreatPerson { unit } => {
            crate::world::effects::claim_free_great_person(world, r, civ, unit).ok_or(NotAllowed)?;
            events.push(TurnEvent::GreatPersonBorn { civ, kind: r.units[unit].name.clone() });
        }
        ChoosePantheon { belief } => {
            let cost = crate::world::religion::pantheon_cost(world, mods.costs.faith_cost);
            let c = world.civ(civ).expect("validated civ");
            if !crate::world::religion::can_found_pantheon(world, r, civ)
                || !crate::world::religion::available_beliefs(world, r, "Pantheon").contains(&belief)
            {
                return Err(Unavailable);
            }
            if c.faith.stored < cost {
                return Err(NotEnoughResources);
            }
            let c = world.civ_mut(civ).expect("validated civ");
            c.faith.stored -= cost;
            c.faith.beliefs.insert(belief);
            crate::world::religion::on_pantheon_founded(world, civ);
            crate::world::policies::rebuild_uniques(world, r, civ);
            events.push(TurnEvent::PantheonFounded { civ, belief });
        }
        Annex { city } => {
            crate::world::conquest::annex(world, city);
        }
        DisbandUnit { unit } => {
            crate::world::administration::disband(world, r, unit, mods.costs).ok_or(NotAllowed)?;
            events.push(TurnEvent::UnitDisbanded { civ, unit });
        }
        GiftUnit { unit } => {
            crate::world::administration::gift_unit(world, r, unit).ok_or(NotAllowed)?;
        }
        PledgeProtection { target } | WithdrawProtection { target } => {
            if !crate::world::administration::protection(
                world,
                civ,
                target,
                matches!(command, Command::PledgeProtection { .. }),
            ) {
                return Err(NotAllowed);
            }
        }
        LiberateCity { city } => {
            crate::world::conquest::liberate(world, r, city).ok_or(NotAllowed)?;
        }
        SellBuilding { city, building } => {
            crate::world::administration::sell(world, r, city, building).ok_or(NotAllowed)?;
        }
        BuyTile { city, tile } => {
            target_tile(world, tile)?;
            if !world.visibility.can_see(civ, tile) {
                return Err(InvalidTarget);
            }
            crate::world::expansion::buy_tile(world, r, city, tile, mods.costs.gold_cost).map_err(|_| NotAllowed)?;
        }
        StopRazing { city } => {
            let c = world.cities.get_mut(city).ok_or(InvalidTarget)?;
            if !c.razing {
                return Err(NotAllowed);
            }
            c.razing = false;
        }
        WithdrawSpy { spy } => {
            if !crate::world::espionage::recall(world, civ, spy) {
                return Err(NotAllowed);
            }
        }
        AttemptCoup { target } => {
            known_party(world, civ, target)?;
            if !crate::world::espionage::queue_coup(world, r, civ, target) {
                return Err(NotAllowed);
            }
        }
        Raze { city } => {
            crate::world::conquest::raze(world, city).map_err(|_| NotAllowed)?;
        }
        AssignSpy { spy, city } => {
            let own = world.cities.get(city).is_some_and(|c| c.owner == civ);
            if !own {
                let seen = world.visibility.known_cities(civ).find(|seen| seen.id == city).ok_or(InvalidTarget)?;
                if crate::world::espionage::keeps_promise_not_to_spy(world, r, civ, seen.owner) {
                    return Err(NotAllowed);
                }
            }
            if !crate::world::espionage::assign(world, civ, spy, city) {
                return Err(Unavailable);
            }
        }
        DeclareWar { target } => {
            known_party(world, civ, target)?;
            if !world.diplomacy.can_declare_war(civ, target) {
                return Err(NotAllowed);
            }
            events.extend(crate::world::diplomacy_actions::declare_war(world, r, civ, target));
        }
        OfferPeace { target } => {
            known_party(world, civ, target)?;
            if !world.civ(target).is_some_and(|c| c.is_ai) || !crate::world::ai::wants_peace(world, r, target, civ) {
                return Err(NotAllowed);
            }
            let peace = crate::world::diplomacy_actions::make_peace(world, r, civ, target);
            if peace.is_empty() {
                return Err(NotAllowed);
            }
            events.extend(peace);
        }
        OfferPeaceTreaty { target, give, get } => {
            known_party(world, civ, target)?;
            if !world.civ(target).is_some_and(|c| c.is_ai) || !crate::world::ai::wants_peace(world, r, target, civ) {
                return Err(NotAllowed);
            }
            let terms = crate::world::agreements::Terms::PeaceTreaty { give: give.clone(), get: get.clone() };
            crate::world::agreements::validate_proposal(world, r, civ, target, &terms, mods)?;
            let mut peaceful = world.clone();
            peaceful.diplomacy.make_peace(civ, target);
            if crate::world::trade::acceptability(&peaceful, r, target, civ, &get, &give, mods.costs.deal_duration) < 0
            {
                return Err(NotAllowed);
            }
            events.extend(crate::world::agreements::accept(world, r, civ, target, &terms, mods)?.events);
        }
        BuyCityState { target } => {
            known_party(world, civ, target)?;
            let gold = crate::world::city_state_actions::marry(world, r, civ, target, mods.costs).ok_or(NotAllowed)?;
            events.push(TurnEvent::CityStateMarried { city_state: target, by: civ, gold });
        }
        OfferFriendship { target } => {
            known_party(world, civ, target)?;
            if !world.civ(target).is_some_and(|c| c.is_ai)
                || !crate::world::research_agreements::wants_friendship(world, target, civ)
            {
                return Err(NotAllowed);
            }
            let out = crate::world::agreements::accept(
                world,
                r,
                civ,
                target,
                &crate::world::agreements::Terms::Friendship,
                mods,
            )?;
            events.extend(out.events);
        }
        OfferDefensivePact { target } => {
            known_party(world, civ, target)?;
            if !world.civ(target).is_some_and(|c| c.is_ai)
                || !crate::world::defensive_pacts::wants_to_sign(world, r, target, civ)
            {
                return Err(NotAllowed);
            }
            let out = crate::world::agreements::accept(
                world,
                r,
                civ,
                target,
                &crate::world::agreements::Terms::DefensivePact,
                mods,
            )?;
            events.extend(out.events);
        }
        OfferResearchAgreement { target } => {
            known_party(world, civ, target)?;
            if !world.civ(target).is_some_and(|c| c.is_ai) {
                return Err(NotAllowed);
            }
            let out = crate::world::agreements::accept(
                world,
                r,
                civ,
                target,
                &crate::world::agreements::Terms::ResearchAgreement,
                mods,
            )?;
            events.extend(out.events);
        }
        Denounce { target } => {
            known_party(world, civ, target)?;
            crate::world::opinion::denounce(world, civ, target);
            events.push(TurnEvent::Denounced { by: civ, target });
        }
        Demand { target, demand } => {
            known_party(world, civ, target)?;
            let e = crate::world::promises::demand(world, civ, target, demand).map_err(|_| NotAllowed)?;
            events.extend(crate::world::turn::promise_events([e]));
        }
        AnswerDemand { by, demand, agree } => {
            known_party(world, civ, by)?;
            let (e, war) = crate::world::promises::answer(world, r, civ, by, demand, agree).map_err(|_| NotAllowed)?;
            events.extend(crate::world::turn::promise_events([e]));
            events.extend(war);
        }
        GiftGold { target, gold } => {
            known_party(world, civ, target)?;
            if gold <= 0 || world.diplomacy.at_war(civ, target) {
                return Err(NotAllowed);
            }
            let influence = crate::world::city_states::gift_gold(
                world,
                civ,
                target,
                gold,
                mods.costs.game_length,
                mods.costs.gold_gift_modifier,
            )
            .ok_or(NotAllowed)?;
            events.push(TurnEvent::GoldGifted { from: civ, to: target, gold, influence });
            events.extend(crate::world::turn::quest_events(crate::world::quests::on_gold_gift(world, r, target, civ)));
        }
        OfferTrade { target, give, get } => {
            known_party(world, civ, target)?;
            if !world.civ(target).is_some_and(|c| c.is_ai)
                || !trade_items(&give)
                || !trade_items(&get)
                || (give.is_empty() && get.is_empty())
            {
                return Err(NotAllowed);
            }
            crate::world::trade::validate_proposal(world, r, civ, target, (&give, &get), mods)
                .map_err(|_| NotAllowed)?;
            let turns = mods.costs.deal_duration;
            if crate::world::trade::acceptability(world, r, target, civ, &get, &give, turns) < 0 {
                return Err(NotAllowed);
            }
            crate::world::trade::accept_with(world, r, civ, target, (&give, &get), mods).map_err(|_| NotAllowed)?;
            events.push(TurnEvent::TradeMade { a: civ, b: target });
        }
        DemandTribute { target, worker } => {
            known_party(world, civ, target)?;
            let gold = if worker {
                crate::world::tribute::demand_worker(world, r, target, civ).map(|_| 0)
            } else {
                crate::world::tribute::demand_gold(
                    world,
                    r,
                    target,
                    civ,
                    mods.costs.gold_gift_modifier,
                    mods.costs.tribute_interval,
                )
            }
            .map_err(|_| NotAllowed)?;
            events.push(TurnEvent::TributePaid { city_state: target, to: civ, gold, worker });
            events.extend(crate::world::turn::quest_events(crate::world::quests::on_bullied(world, r, target, civ)));
        }
    }
    finish_action(world, r, &mut events);
    Ok(events)
}

/// Shared settlement for unilateral actions and mutually accepted agreements.
pub(crate) fn finish_action(world: &mut World, r: &Ruleset, events: &mut Vec<TurnEvent>) {
    crate::world::barbarians::synchronize(world, r);
    events.extend(world.resolve_defeats().into_iter().map(|civ| TurnEvent::CivDefeated { civ }));
    crate::world::city_state_actions::sync_alliances(world);
    crate::world::resources::refresh_stocks(world, r);
    crate::world::turn::update_visibility_in(world, r);
    crate::world::fog::capture_observations(world, r);
    events.extend(
        crate::world::natural_wonders::discover_visible(world, r).into_iter().map(TurnEvent::NaturalWonderDiscovered),
    );
    events.extend(crate::world::turn::detect_first_contact_in(world, r));
}
