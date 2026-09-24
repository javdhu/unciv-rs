//! Voluntary disposal and city management use the same ownership boundary as
//! production; prices are read from the bundled rules, never supplied by clients.
use crate::world::construction::{Construction, CostModifiers};

pub fn can_convert_production(world: &World, r: &Ruleset, city: Id<City>, stat: Option<crate::stats::Stat>) -> bool {
    let Some(c) = world.cities.get(city) else {
        return false;
    };
    if c.puppet || c.resistance > 0 {
        return false;
    }
    let Some(stat) = stat else {
        return true;
    };
    if !matches!(stat, crate::stats::Stat::Gold | crate::stats::Stat::Science) {
        return false;
    }
    let ctx = crate::world::conditionals::GameContext::new(r).with_world(world).for_civ(c.owner).in_city(city);
    world.civ(c.owner).is_some_and(|owner| {
        owner.uniques.matching(UniqueType::EnablesStatProduction).any(|u| {
            crate::world::conditionals::applies(u, &ctx).holds()
                && u.params.first().is_some_and(|s| crate::stats::Stat::parse(s) == Some(stat))
        })
    })
}
use crate::{
    arena::Id,
    ruleset::{Building, Ruleset},
    world::{City, Unit, World},
    UniqueType,
};

pub fn sell_price(world: &World, r: &Ruleset, city: Id<City>, building: Id<Building>) -> Option<i32> {
    let c = world.cities.get(city)?;
    let b = r.buildings.get(building)?;
    if !c.built.contains(&building)
        || c.free_buildings.contains(&building)
        || c.puppet
        || c.sold_building_this_turn
        || b.is_wonder == Some(true)
        || b.is_national_wonder == Some(true)
        || b.uniques.iter().any(|u| u.unique_type() == Some(UniqueType::Unsellable))
    {
        return None;
    }
    Some(b.cost.unwrap_or(0).max(0) / 10)
}

pub fn sell(world: &mut World, r: &Ruleset, city: Id<City>, building: Id<Building>) -> Option<i32> {
    let price = sell_price(world, r, city, building)?;
    let c = world.cities.get_mut(city)?;
    let owner = c.owner;
    c.built.remove(&building);
    c.sold_building_this_turn = true;
    let slots = crate::world::specialists::slots(r, c);
    c.specialists.retain(|kind, n| {
        *n = (*n).min(slots.get(kind).copied().unwrap_or(0));
        *n > 0
    });
    crate::world::city::rebuild_city_uniques(r, c);
    world.civ_mut(owner)?.gold += price;
    crate::world::policies::rebuild_uniques(world, r, owner);
    Some(price)
}

pub fn disband_gold(world: &World, r: &Ruleset, unit: Id<Unit>, mods: CostModifiers) -> Option<i32> {
    let u = world.units.get(unit)?;
    if !u.can_move() {
        return None;
    }
    disband_value(world, r, unit, mods)
}

fn disband_value(world: &World, r: &Ruleset, unit: Id<Unit>, mods: CostModifiers) -> Option<i32> {
    let u = world.units.get(unit)?;
    if world.grid.owner[u.position.0 as usize] != Some(u.owner) {
        return Some(0);
    }
    let base = &r.units[u.base];
    let production = crate::world::construction::production_cost(r, Construction::Unit(u.base), mods).unwrap_or(0);
    Some(
        (crate::world::purchase::gold_price(i64::from(production), base.hurry_cost_modifier.unwrap_or(0)).floor() / 20)
            .max(0) as i32,
    )
}

pub fn disband(world: &mut World, r: &Ruleset, unit: Id<Unit>, mods: CostModifiers) -> Option<i32> {
    let gold = disband_gold(world, r, unit, mods)?;
    let owner = world.units.get(unit)?.owner;
    let stranded = crate::world::air::evacuate_payload_before_disband(world, r, unit);
    let cargo_gold: i32 = stranded.iter().filter_map(|id| disband_value(world, r, *id, mods)).sum();
    world.remove_unit(unit)?;
    world.civ_mut(owner)?.gold += gold + cargo_gold;
    Some(gold + cargo_gold)
}

pub fn gift_unit(world: &mut World, r: &Ruleset, unit: Id<Unit>) -> Option<crate::world::CivId> {
    use crate::{
        stats::Fixed,
        world::{
            conditionals::{applies, GameContext},
            diplomacy::Sentiment,
        },
    };
    let u = world.units.get(unit)?;
    let from = u.owner;
    let to = world.grid.owner[u.position.0 as usize]?;
    if to == from || !u.can_move() || u.transport.is_some() || world.diplomacy.at_war(from, to) {
        return None;
    }
    let target = world.civ(to)?;
    if !target.is_alive() {
        return None;
    }
    let city_state = target.is_city_state();
    let great = crate::world::filters::unit_matches(r, u, "Great Person").holds();
    if city_state {
        let ctx = GameContext::new(r).with_world(world).for_civ(from).for_unit(unit).on_tile(u.position);
        let bonus = u
            .all_uniques(r)
            .chain(world.civ(from)?.uniques.all())
            .find(|unique| {
                unique.unique_type() == Some(UniqueType::GainInfluenceWithUnitGiftToCityState)
                    && applies(unique, &ctx).holds()
                    && unique.params.get(1).is_some_and(|f| crate::world::filters::unit_matches(r, u, f).holds())
            })
            .and_then(|u| u.params.first().and_then(|p| Fixed::parse(p)));
        if u.is_civilian(r) && bonus.is_none() {
            return None;
        }
        world.diplomacy.add_influence(to, from, bonus.unwrap_or(Fixed::whole(5)));
    } else {
        if !crate::world::movement::is_friendly_territory(world, from, u.position) {
            return None;
        }
        world.diplomacy.attitude_mut(to, from).add(Sentiment::GaveUsUnits, Fixed::whole(5), None);
    }
    if city_state && great {
        world.remove_unit(unit);
    } else {
        world.transfer_unit(unit, to);
    }
    Some(to)
}

/// Whether `major` may pledge to protect `minor` now (`pledge`), or withdraw
/// its pledge: met, at peace, and outside the waits a change of heart costs
/// (twenty turns after a withdrawal, ten after a pledge). A pledge also needs
/// influence of at least zero.
pub fn may_protect(world: &World, major: crate::world::CivId, minor: crate::world::CivId, pledge: bool) -> bool {
    use crate::{stats::Fixed, world::diplomacy::Flag};
    if !world.civ(major).is_some_and(|c| c.is_major() && c.is_alive())
        || !world.civ(minor).is_some_and(|c| c.is_city_state() && c.is_alive())
        || !world.diplomacy.have_met(major, minor)
        || world.diplomacy.at_war(major, minor)
    {
        return false;
    }
    if pledge {
        !(world.diplomacy.is_protector(major, minor)
            || world.diplomacy.has_flag(minor, major, Flag::RecentlyWithdrewProtection)
            || world.diplomacy.influence(minor, major) < Fixed::ZERO)
    } else {
        world.diplomacy.is_protector(major, minor)
            && !world.diplomacy.has_flag(minor, major, Flag::RecentlyPledgedProtection)
    }
}

pub fn protection(world: &mut World, major: crate::world::CivId, minor: crate::world::CivId, pledge: bool) -> bool {
    use crate::{stats::Fixed, world::diplomacy::Flag};
    if !may_protect(world, major, minor, pledge) {
        return false;
    }
    if pledge {
        world.diplomacy.set_protector(major, minor, true);
        world.diplomacy.set_flag(minor, major, Flag::RecentlyPledgedProtection, 10);
    } else {
        world.diplomacy.set_protector(major, minor, false);
        world.diplomacy.set_flag(minor, major, Flag::RecentlyWithdrewProtection, 20);
        world.diplomacy.add_influence(minor, major, Fixed::whole(-20));
    }
    true
}
