//! The AI's army, decided on the live map (docs/AI.md, level 1).
//!
//! The legacy military order ([`ai::military_order`]) is planned against the
//! start-of-round world: every unit decides before any of them has moved, and
//! a later civ acts on positions an earlier one has already changed. Here each
//! command is chosen just before it runs, so a unit sees what the unit before
//! it did.
//!
//! The order is the one a player uses. Cities bombard first. Ranged units
//! soften targets, then melee units finish them and take cities. Every attack
//! is scored with the combat preview: what it deals, whether it kills, what it
//! costs and whether the attacker could die. Wounded units fall back to heal.
//! An army marching on a city waits just outside the city's range until it is
//! strong enough to take it, instead of walking in one unit at a time.
//! Threatened cities pull the nearest unit home as a garrison.
//!
//! Everything is integer arithmetic over ids in order, so it is deterministic.

use std::collections::{BTreeMap, BTreeSet};

use crate::arena::Id;
use crate::ruleset::Ruleset;
use crate::stats::Fixed;
use crate::world::air::Target;
use crate::world::city::City;
use crate::world::commands::{AttackTarget, Command};
use crate::world::entities::Unit;
use crate::world::grid::{CivId, TileId};
use crate::world::World;
use crate::UniqueType;

/// Decisions one unit may make in a turn. Each is one or two commands (a
/// move, then the attack it brings into reach).
const MAX_STEPS: u8 = 3;

/// Below this health a unit heals unless it can land a kill or take a city.
const RETREAT_HEALTH: u8 = 40;

/// Below this, a unit standing in reach of more enemy strength than its own
/// falls back too.
const WARY_HEALTH: u8 = 65;

/// Damage dealt counts this much more than damage taken, in thousandths. The
/// side that attacks chooses the fight, and a unit that holds back is
/// attacked anyway.
const AGGRESSION: i64 = 1500;

/// An army closes on a city from this far.
const MUSTER_RADIUS: u32 = 4;

/// Enemy military this close to a city makes it want a garrison.
const THREAT_RADIUS: u32 = 3;

/// How far a unit will travel to garrison a threatened city.
const GARRISON_REACH: u32 = 5;

/// Whether the live tactics command this unit. Aircraft, missiles and nukes
/// keep their own planners ([`crate::world::air::order`]); civilians have jobs.
pub fn takes(world: &World, r: &Ruleset, unit: Id<Unit>) -> bool {
    world.units.get(unit).is_some_and(|u| {
        u.is_military(r)
            && !u.is_air(r)
            && !u.has_unique(r, UniqueType::NuclearWeapon)
            && !crate::world::settling::is_settler(r, u.base)
            && !crate::world::ai::is_worker(r, u.base)
    })
}

/// One civ's army for one turn. Call [`Tactics::next`] until it returns
/// `None`, running each command before asking for the next.
pub struct Tactics {
    civ: CivId,
    /// Cities with a bombard still to try, one entry per attack.
    bombards: Vec<Id<City>>,
    /// Units still to act, in the order they act.
    queue: Vec<Id<Unit>>,
    /// The unit acting now and how many commands it has issued.
    current: Option<(Id<Unit>, u8)>,
    /// A command that follows the one just issued, such as the attack after
    /// the move that brings a unit into reach.
    follow_up: Option<Command>,
    /// The enemy city the army marches on this turn.
    objective: Option<Id<City>>,
    /// Cities that want a defender, and the unit sent to each.
    garrisons: BTreeMap<Id<City>, Id<Unit>>,
    /// How far each enemy unit can strike, which costs a unique lookup, so
    /// it is worked out once per unit per turn.
    reach: BTreeMap<Id<Unit>, u32>,
}

/// What the civ can see of the enemy, gathered once per decision.
struct Enemies {
    /// Visible military units of civs at war with us: id, tile, strength and
    /// the distance they can threaten (movement plus range).
    units: Vec<(Id<Unit>, TileId, i32, u32)>,
    /// Every tile any unit stands on, and whether that unit is ours.
    occupied: BTreeMap<TileId, bool>,
}

impl Enemies {
    fn gather(world: &World, r: &Ruleset, civ: CivId, reach: &mut BTreeMap<Id<Unit>, u32>) -> Self {
        let mut units = Vec::new();
        let mut occupied = BTreeMap::new();
        for (id, u) in world.units.iter() {
            occupied.insert(u.position, u.owner == civ);
            if u.owner != civ
                && u.is_military(r)
                && world.diplomacy.at_war(civ, u.owner)
                && crate::world::fog::can_see_unit(world, r, civ, id)
            {
                let reach = *reach.entry(id).or_insert_with(|| {
                    crate::world::movement::max_movement(world, r, u).max(1) as u32
                        + crate::world::air::attack_range(world, r, id).max(1) as u32
                });
                units.push((id, u.position, strength(r, u), reach));
            }
        }
        Self { units, occupied }
    }

    /// Enemy strength that could strike this tile next turn.
    fn threat(&self, world: &World, tile: TileId) -> i32 {
        self.units.iter().filter(|(_, at, _, reach)| world.grid.distance(*at, tile) <= *reach).map(|u| u.2).sum()
    }

    /// Steps to the nearest visible enemy unit.
    fn nearest(&self, world: &World, tile: TileId) -> u32 {
        self.units.iter().map(|(_, at, _, _)| world.grid.distance(*at, tile)).min().unwrap_or(u32::MAX)
    }

    fn near(&self, world: &World, tile: TileId, radius: u32) -> bool {
        self.units.iter().any(|(_, at, _, _)| world.grid.distance(*at, tile) <= radius)
    }
}

/// A unit's fighting strength: the larger of melee and ranged, scaled by health.
fn strength(r: &Ruleset, u: &Unit) -> i32 {
    let base = r.units.get(u.base).map_or(0, |b| b.strength.unwrap_or(0).max(b.ranged_strength.unwrap_or(0)));
    base * i32::from(u.health) / 100
}

/// What a unit is worth, in production.
fn worth(r: &Ruleset, u: &Unit) -> i64 {
    i64::from(r.units.get(u.base).and_then(|b| b.cost).unwrap_or(40).max(10))
}

/// What a city is worth to take.
fn city_worth(city: &City) -> i64 {
    300 + 60 * i64::from(city.population)
}

fn at_war_with_anyone_who_has_cities(world: &World, civ: CivId) -> bool {
    world.cities.iter().any(|(_, c)| c.owner != civ && world.diplomacy.at_war(civ, c.owner))
}

impl Tactics {
    pub fn new(world: &World, r: &Ruleset, civ: CivId) -> Self {
        let mut bombards = Vec::new();
        let mut own_cities: Vec<Id<City>> = Vec::new();
        if let Some(c) = world.civ(civ) {
            for &city in &c.cities {
                own_cities.push(city);
                if let Some(ct) = world.cities.get(city) {
                    for _ in 0..ct.attacks_left {
                        bombards.push(city);
                    }
                }
            }
        }
        bombards.reverse();
        let mut units: Vec<Id<Unit>> = world
            .civ(civ)
            .map(|c| c.units.iter().copied().filter(|u| takes(world, r, *u)).collect())
            .unwrap_or_default();
        // Ranged first, so melee units can finish what they wounded.
        units.sort_by_key(|u| (!world.units.get(*u).is_some_and(|u| u.is_ranged(r)), *u));
        units.reverse();
        let mut reach = BTreeMap::new();
        let enemies = Enemies::gather(world, r, civ, &mut reach);
        let garrisons = assign_garrisons(world, r, civ, &own_cities, &units, &enemies);
        Self {
            civ,
            bombards,
            queue: units,
            current: None,
            follow_up: None,
            objective: choose_objective(world, r, civ),
            garrisons,
            reach,
        }
    }

    /// The next command to run, or `None` when the army is done for the turn.
    pub fn next(&mut self, world: &World, r: &Ruleset) -> Option<Command> {
        if let Some(command) = self.follow_up.take() {
            return Some(command);
        }
        while let Some(city) = self.bombards.pop() {
            if let Some(command) = bombard(world, r, city) {
                return Some(command);
            }
        }
        loop {
            let (unit, steps) = match self.current {
                Some((unit, steps)) if steps < MAX_STEPS => (unit, steps),
                _ => (self.queue.pop()?, 0),
            };
            self.current = Some((unit, steps + 1));
            let enemies = Enemies::gather(world, r, self.civ, &mut self.reach);
            match self.decide(world, r, unit, &enemies) {
                Some(mut commands) if !commands.is_empty() => {
                    let first = commands.remove(0);
                    self.follow_up = commands.pop();
                    return Some(first);
                }
                _ => self.current = None,
            }
        }
    }

    /// This unit's next commands (a move, then perhaps an attack), or `None`
    /// when it has nothing worth doing and rests where it is.
    fn decide(&self, world: &World, r: &Ruleset, unit: Id<Unit>, enemies: &Enemies) -> Option<Vec<Command>> {
        let u = world.units.get(unit)?;
        if u.owner != self.civ || u.movement_left <= Fixed::ZERO {
            return None;
        }
        if let Some(to) = crate::world::air::paradrop_order(world, r, unit) {
            return Some(vec![Command::Paradrop { unit, to }]);
        }
        let attack = best_attack(world, r, self.civ, unit, enemies);
        let here = enemies.threat(world, u.position);
        let decisive = attack.as_ref().is_some_and(|a| a.decisive);
        let hurt = u.health < RETREAT_HEALTH || (u.health < WARY_HEALTH && here > strength(r, u));
        if hurt && !decisive && !in_own_city(world, u) {
            return retreat(world, r, unit, enemies);
        }
        if let Some(a) = attack {
            return Some(a.commands);
        }
        if let Some((&city, _)) = self.garrisons.iter().find(|(_, g)| **g == unit) {
            let centre = world.cities.get(city)?.centre;
            if u.position == centre {
                return None;
            }
            return step(world, r, unit, centre);
        }
        if let Some(objective) = self.objective {
            return self.march(world, r, unit, objective, enemies);
        }
        match crate::world::ai::explore_order(world, r, self.civ, unit)? {
            crate::world::ai::UnitOrder::MoveTo(to) => Some(vec![Command::MoveUnit { unit, to }]),
            _ => None,
        }
    }

    /// Close on the objective, but hold short of its guns until the army
    /// around it is strong enough to take it.
    fn march(
        &self,
        world: &World,
        r: &Ruleset,
        unit: Id<Unit>,
        objective: Id<City>,
        enemies: &Enemies,
    ) -> Option<Vec<Command>> {
        let u = world.units.get(unit)?;
        let city = world.cities.get(objective)?;
        let distance = world.grid.distance(u.position, city.centre);
        if distance > MUSTER_RADIUS {
            return step(world, r, unit, city.centre);
        }
        let mustered: i32 = world
            .civ(self.civ)?
            .units
            .iter()
            .filter_map(|id| world.units.get(*id))
            .filter(|o| o.is_military(r) && !o.is_air(r) && o.health >= WARY_HEALTH)
            .filter(|o| world.grid.distance(o.position, city.centre) <= MUSTER_RADIUS)
            .map(|o| strength(r, o))
            .sum();
        let defence = crate::world::combat::city_defense_strength(world, r, objective).round() as i32;
        let guard: i32 = enemies
            .units
            .iter()
            .filter(|(_, at, _, _)| world.grid.distance(*at, city.centre) <= 1)
            .map(|e| e.2)
            .sum();
        let ready = mustered * 2 >= (defence + guard) * 3;
        if !ready {
            // Wait outside the city's bombard range (2).
            return (distance < 3).then(|| step_away(world, r, unit, city.centre, 3)).flatten();
        }
        // Ready: stand where this unit can strike next turn.
        let want = if u.is_ranged(r) { crate::world::air::attack_range(world, r, unit).max(1) as u32 } else { 1 };
        if distance <= want {
            return None;
        }
        let reachable = crate::world::movement::reachable_for_unit(world, r, unit);
        reachable
            .keys()
            .copied()
            .filter(|t| *t != u.position && !enemies.occupied.contains_key(t))
            .filter(|t| world.grid.distance(*t, city.centre) >= want.min(distance))
            .map(|t| {
                let d = world.grid.distance(t, city.centre);
                ((d.abs_diff(want), enemies.threat(world, t)), t)
            })
            .min()
            .map(|(_, to)| vec![Command::MoveUnit { unit, to }])
            .or_else(|| step(world, r, unit, city.centre))
    }
}

/// A wounded unit already in one of its own cities heals there; it has
/// nowhere better to go.
fn in_own_city(world: &World, u: &Unit) -> bool {
    world.cities.iter().any(|(_, c)| c.owner == u.owner && c.centre == u.position)
}

/// A move toward `target` by a route that exists.
fn step(world: &World, r: &Ruleset, unit: Id<Unit>, target: TileId) -> Option<Vec<Command>> {
    let to = crate::world::movement::step_toward(world, r, unit, target)?;
    Some(vec![Command::MoveUnit { unit, to }])
}

/// A move to the nearest reachable tile at least `at_least` from `from`.
fn step_away(world: &World, r: &Ruleset, unit: Id<Unit>, from: TileId, at_least: u32) -> Option<Vec<Command>> {
    let u = world.units.get(unit)?;
    let occupied: BTreeSet<TileId> = world.units.iter().map(|(_, o)| o.position).collect();
    crate::world::movement::reachable_for_unit(world, r, unit)
        .into_iter()
        .filter(|(t, _)| *t != u.position && !occupied.contains(t))
        .filter(|(t, _)| world.grid.distance(*t, from) >= at_least)
        .map(|(t, cost)| ((world.grid.distance(t, from), cost), t))
        .min()
        .map(|(_, to)| vec![Command::MoveUnit { unit, to }])
}

/// Fall back to where the unit heals best and is least in reach, and rest.
fn retreat(world: &World, r: &Ruleset, unit: Id<Unit>, enemies: &Enemies) -> Option<Vec<Command>> {
    let u = world.units.get(unit)?;
    let civ = u.owner;
    let rest = |t: TileId| -> i32 {
        let city = world.cities.iter().any(|(_, c)| c.owner == civ && c.centre == t);
        let friendly = crate::world::movement::is_friendly_territory(world, civ, t);
        let heal = if city {
            25
        } else if friendly {
            15
        } else {
            5
        };
        heal * 4 - enemies.threat(world, t)
    };
    // Ties go to the tile farthest from the nearest enemy.
    let key = |t: TileId| (rest(t), enemies.nearest(world, t));
    let here = key(u.position);
    let best = crate::world::movement::reachable_for_unit(world, r, unit)
        .into_keys()
        .filter(|t| *t != u.position && !enemies.occupied.contains_key(t))
        .map(|t| (std::cmp::Reverse(key(t)), t))
        .min();
    match best {
        Some((std::cmp::Reverse(score), to)) if score > here => Some(vec![Command::MoveUnit { unit, to }]),
        // Already where it heals best: stay put, which is what heals.
        _ => None,
    }
}

/// A scored attack: the commands that make it and whether it is worth
/// taking even when wounded (a likely kill with no risk, or a capture).
struct Attack {
    score: i64,
    decisive: bool,
    commands: Vec<Command>,
}

fn best_attack(world: &World, r: &Ruleset, civ: CivId, unit: Id<Unit>, enemies: &Enemies) -> Option<Attack> {
    let u = world.units.get(unit)?;
    if u.attacks_left == 0 {
        return None;
    }
    let ranged = u.is_ranged(r);
    let range = crate::world::air::attack_range(world, r, unit).max(1) as u32;
    let indirect = u.has_unique(r, UniqueType::IndirectFire);
    let fixed_gun = u.has_unique(r, UniqueType::MustSetUp) && ranged;
    let reach = crate::world::movement::max_movement(world, r, u).max(1) as u32 + range;
    // Tiles the unit could strike from: where it stands, and anywhere it can
    // reach this turn with movement to spare.
    let mut from = vec![u.position];
    if !fixed_gun {
        let reachable = crate::world::movement::reachable_for_unit(world, r, unit);
        from.extend(
            reachable
                .into_iter()
                .filter(|(t, cost)| {
                    *t != u.position && u.movement_left - *cost > Fixed::ZERO && !enemies.occupied.contains_key(t)
                })
                .map(|(t, _)| t),
        );
    }
    let strikes = |origin: TileId, tile: TileId| {
        let d = world.grid.distance(origin, tile);
        if ranged {
            d <= range && crate::world::combat::has_line_of_sight(&world.grid, r, origin, tile, indirect)
        } else {
            d == 1
        }
    };
    let mut targets: Vec<(Target, TileId, i64)> = Vec::new();
    for (id, city) in world.cities.iter() {
        if city.owner != civ
            && world.diplomacy.at_war(civ, city.owner)
            && world.visibility.can_see(civ, city.centre)
            && world.grid.distance(u.position, city.centre) <= reach
        {
            targets.push((Target::City(id), city.centre, city_worth(city)));
        }
    }
    for &(id, at, _, _) in &enemies.units {
        if world.grid.distance(u.position, at) <= reach {
            targets.push((Target::Unit(id), at, world.units.get(id).map_or(40, |o| worth(r, o))));
        }
    }
    // Civilians of civs at war: a free capture for melee units.
    if !ranged {
        for (id, o) in world.units.iter() {
            if o.owner != civ
                && o.is_civilian(r)
                && world.diplomacy.at_war(civ, o.owner)
                && world.grid.distance(u.position, o.position) <= reach
                && crate::world::fog::can_see_unit(world, r, civ, id)
                && !world.units.iter().any(|(_, g)| g.position == o.position && g.is_military(r))
            {
                targets.push((Target::Unit(id), o.position, worth(r, o)));
            }
        }
    }
    let mine = worth(r, u);
    let mut best: Option<Attack> = None;
    for (target, tile, value) in targets {
        if !crate::world::military::may_attack(world, r, unit, target) {
            continue;
        }
        // Where to strike from: here if possible, else the reachable tile
        // least in reach of the enemy.
        let origin = if strikes(u.position, tile) {
            Some(u.position)
        } else {
            from.iter()
                .copied()
                .filter(|t| strikes(*t, tile))
                .map(|t| (enemies.threat(world, t), t))
                .min()
                .map(|(_, t)| t)
        };
        let Some(origin) = origin else { continue };
        let Some(preview) = crate::world::combat::preview_attack(world, r, unit, target) else { continue };
        let health = i64::from(preview.defender.health.max(1));
        let max_health = i64::from(preview.defender.max_health.max(1));
        let dealt = i64::from(preview.defender.damage.0 + preview.defender.damage.1) / 2;
        let taken = i64::from(preview.attacker.damage.0 + preview.attacker.damage.1) / 2;
        let own = i64::from(preview.attacker.health.max(1));
        let is_city = matches!(target, Target::City(_));
        let kills = !is_city && dealt >= health;
        let sure_kill = !is_city && i64::from(preview.defender.damage.0) >= health;
        let may_die = i64::from(preview.attacker.damage.1) >= own;
        let will_die = i64::from(preview.attacker.damage.0) >= own;
        let mut score = dealt.min(health) * value * AGGRESSION / (max_health * 1000) - taken * mine / 100;
        if kills {
            score += value;
        }
        if preview.capture {
            score += value * 2;
        }
        if will_die {
            score -= mine * 2;
        } else if may_die {
            score -= mine;
        }
        if origin != u.position {
            // Stepping up exposes the unit to what can reach that tile.
            let exposure = i64::from(enemies.threat(world, origin));
            let own_strength = i64::from(strength(r, u).max(1));
            score -= mine * exposure.min(own_strength * 3) / (own_strength * 6);
        }
        // Melee on a city that will not fall trades health for little.
        if is_city && !ranged && !preview.capture && dealt * 3 < health {
            score -= mine / 2;
        }
        if score <= 0 {
            continue;
        }
        let decisive = preview.capture || (sure_kill && !may_die);
        let mut commands = Vec::new();
        if origin != u.position {
            commands.push(Command::MoveUnit { unit, to: origin });
        }
        if fixed_gun && !u.set_up {
            commands.push(Command::SetUp { unit });
        }
        commands.push(Command::Attack {
            unit,
            target: match target {
                Target::Unit(id) => AttackTarget::Unit(id),
                Target::City(id) => AttackTarget::City(id),
            },
        });
        let better = best.as_ref().is_none_or(|b| score > b.score);
        if better {
            best = Some(Attack { score, decisive, commands });
        }
    }
    best
}

/// A city's bombard: the kill if there is one, else the most damage by worth.
fn bombard(world: &World, r: &Ruleset, city: Id<City>) -> Option<Command> {
    let c = world.cities.get(city)?;
    if c.attacks_left == 0 {
        return None;
    }
    crate::world::combat::bombard_targets(world, r, city)
        .into_iter()
        .filter_map(|target| {
            let u = world.units.get(target)?;
            let (low, high) = crate::world::combat::preview_bombard(world, r, city, target)?;
            let health = i64::from(u.health);
            let dealt = i64::from(low + high) / 2;
            let value = worth(r, u);
            let score = dealt.min(health) * value / 100 + if dealt >= health { value } else { 0 };
            Some((std::cmp::Reverse(score), target))
        })
        .min()
        .map(|(_, target)| Command::Bombard { city, target })
}

/// The enemy city the army marches on: the nearest to any of our cities,
/// weaker ones first when two are equally near.
fn choose_objective(world: &World, r: &Ruleset, civ: CivId) -> Option<Id<City>> {
    if !at_war_with_anyone_who_has_cities(world, civ) {
        return None;
    }
    let ours: Vec<TileId> =
        world.civ(civ)?.cities.iter().filter_map(|c| world.cities.get(*c)).map(|c| c.centre).collect();
    world
        .cities
        .iter()
        .filter(|(_, c)| c.owner != civ && world.diplomacy.at_war(civ, c.owner))
        .filter(|(_, c)| world.visibility.has_explored(civ, c.centre))
        .map(|(id, c)| {
            let near = ours.iter().map(|t| world.grid.distance(*t, c.centre)).min().unwrap_or(u32::MAX / 2);
            let defence = crate::world::combat::city_defense_strength(world, r, id).round();
            ((near, defence, id), id)
        })
        .min()
        .map(|(_, id)| id)
}

/// Threatened cities with no military unit inside each claim the strongest
/// unit that can get there soon.
fn assign_garrisons(
    world: &World,
    r: &Ruleset,
    civ: CivId,
    cities: &[Id<City>],
    units: &[Id<Unit>],
    enemies: &Enemies,
) -> BTreeMap<Id<City>, Id<Unit>> {
    let mut taken: BTreeSet<Id<Unit>> = BTreeSet::new();
    let mut out = BTreeMap::new();
    for &city in cities {
        let Some(c) = world.cities.get(city) else { continue };
        if !enemies.near(world, c.centre, THREAT_RADIUS) {
            continue;
        }
        let inside = units.iter().copied().find(|u| world.units.get(*u).is_some_and(|u| u.position == c.centre));
        let pick = inside.or_else(|| {
            units
                .iter()
                .copied()
                .filter(|u| !taken.contains(u))
                .filter_map(|id| world.units.get(id).map(|u| (id, u)))
                .filter(|(_, u)| u.owner == civ && world.grid.distance(u.position, c.centre) <= GARRISON_REACH)
                .map(|(id, u)| ((std::cmp::Reverse(strength(r, u)), world.grid.distance(u.position, c.centre), id), id))
                .min()
                .map(|(_, id)| id)
        });
        if let Some(unit) = pick {
            taken.insert(unit);
            out.insert(city, unit);
        }
    }
    out
}
