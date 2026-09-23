//! Strategic resources, and what a civilization lacks.
//!
//! Civ's resources are a stock, not a flag: a civ has some number of Iron, and
//! each Swordsman standing consumes one. The number that matters is the
//! remainder, because a unit whose resource has run out keeps existing — it
//! just fights badly.
//!
//! Unciv applies a flat **-25%** when any resource a unit requires is in
//! deficit (`BattleDamage.kt:52`, `BattleConstants.MISSING_RESOURCES_MALUS`).
//! Flat, and once: a unit short of two different resources is penalised the
//! same as one short of a single resource, because the modifier is assigned
//! into a map rather than accumulated.

use crate::arena::Id;
use crate::ruleset::{BaseUnit, Ruleset, TileResource};
use crate::world::conditionals::{applies, GameContext, Verdict};
use crate::world::entities::Civilization;
use crate::world::entities::Unit;
use crate::world::grid::CivId;
use crate::world::world::World;
use crate::UniqueType;
use std::collections::BTreeMap;

/// Percent applied when a unit's resource is in deficit.
pub const MISSING_RESOURCE_MALUS: i32 = -25;

/// A civilization's net stock of each resource.
///
/// Ordered, so iteration and serialisation are reproducible.
pub type ResourceStock = BTreeMap<Id<TileResource>, i32>;

/// A deposit's yields and tile filters become visible with its revealing technology.
pub fn is_visible(r: &Ruleset, civ: &Civilization, resource: Id<TileResource>) -> bool {
    r.resources.get(resource).is_some_and(|resource| {
        resource
            .revealed_by
            .as_deref()
            .is_none_or(|name| r.technologies.id(name).is_some_and(|tech| civ.has_tech(tech)))
    })
}

/// Resources a unit needs to be at full strength.
///
/// Both the `requiredResource` field and any `Consumes [amount] [resource]`
/// unique; Unciv reads the same two places.
pub fn required_resources(r: &Ruleset, unit: Id<BaseUnit>) -> Vec<Id<TileResource>> {
    resource_requirements(r, unit).into_keys().collect()
}

/// Static declared quantities. `requiredResource` contributes one and repeated
/// consumption uniques add to it, matching BaseUnit.getResourceRequirementsPerTurn.
pub fn resource_requirements(r: &Ruleset, unit: Id<BaseUnit>) -> ResourceStock {
    requirements(r, unit, None)
}

fn requirements(r: &Ruleset, unit: Id<BaseUnit>, context: Option<&GameContext<'_>>) -> ResourceStock {
    let Some(u) = r.units.get(unit) else { return ResourceStock::new() };
    let mut out = ResourceStock::new();

    if let Some(name) = u.required_resource.as_deref() {
        if let Some(id) = r.resources.id(name) {
            out.insert(id, 1);
        }
    }
    for unique in &u.uniques {
        if unique.unique_type() == Some(UniqueType::ConsumesResources)
            && context.is_none_or(|ctx| applies(unique, ctx) != Verdict::False)
        {
            // "Consumes [amount] [resource]" — the resource is the second slot.
            if let (Some(amount), Some(id)) = (
                unique.params.first().and_then(|n| n.parse::<i32>().ok()),
                unique.params.get(1).and_then(|n| r.resources.id(n)),
            ) {
                if amount > 0 {
                    adjust(&mut out, id, i64::from(amount));
                }
            }
        }
    }
    out
}

fn adjust(stock: &mut ResourceStock, resource: Id<TileResource>, amount: i64) {
    let value = stock.entry(resource).or_default();
    *value = (i64::from(*value) + amount).clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32;
}

fn fielded_requirements(world: &World, r: &Ruleset, id: Id<Unit>) -> ResourceStock {
    let Some(unit) = world.units.get(id) else { return ResourceStock::new() };
    if r.units.get(unit.base).is_some_and(|base| {
        base.uniques.iter().all(|u| u.unique_type() != Some(UniqueType::ConsumesResources) || u.conditionals.is_empty())
    }) {
        return resource_requirements(r, unit.base);
    }
    let mut ctx = GameContext::new(r).with_world(world).for_civ(unit.owner).for_unit(id).on_tile(unit.position);
    if let Some((city, _)) = world.cities.iter().find(|(_, c)| c.centre == unit.position) {
        ctx = ctx.in_city(city);
    }
    requirements(r, unit.base, Some(&ctx))
}

/// Fresh spendable stock, including active imports/exports. This deliberately
/// does not trust the turn-start cache: two purchases in one turn cannot both
/// reserve the same last Iron.
pub fn available_stock(world: &World, r: &Ruleset, civ: CivId) -> ResourceStock {
    let mut stock = recompute_stock(world, r, civ);
    let ctx = GameContext::new(r).with_world(world).for_civ(civ);
    let bonus: i64 = world
        .civ(civ)
        .into_iter()
        .flat_map(|c| c.uniques.matching(UniqueType::CityStateResources))
        .filter(|u| applies(u, &ctx) == Verdict::True)
        .filter_map(|u| u.params.first()?.parse::<i64>().ok())
        .sum();
    // Allies provide their positive domestic supplies once. Raw supplier stock
    // excludes diplomacy, so this cannot recurse through allied resource sharing.
    for cs in crate::world::city_states::city_states(world)
        .into_iter()
        .filter(|cs| crate::world::city_states::ally_of(world, *cs) == Some(civ))
    {
        for (res, n) in recompute_stock(world, r, cs).into_iter().filter(|(_, n)| *n > 0) {
            adjust(&mut stock, res, (i64::from(n) * (100 + bonus) / 100).max(0));
        }
    }
    for deal in &world.diplomacy.deals {
        for (giver, receiver, items) in [(deal.a, deal.b, &deal.a_gives), (deal.b, deal.a, &deal.b_gives)] {
            if giver != civ && receiver != civ {
                continue;
            }
            for running in items.iter().filter(|item| item.turns_left > 0) {
                let (resource, amount) = match running.item {
                    crate::world::trade::Item::Luxury(id) => (id, 1),
                    crate::world::trade::Item::Strategic(id, amount) => (id, amount),
                    _ => continue,
                };
                adjust(&mut stock, resource, if giver == civ { -i64::from(amount) } else { i64::from(amount) });
            }
        }
    }
    stock
}

/// Refresh cached stocks only after every result was derived from the same
/// world, so updates for one civilization cannot influence another's result.
pub fn refresh_stocks(world: &mut World, r: &Ruleset) {
    let stocks: Vec<_> = world.civ_ids().map(|civ| (civ, available_stock(world, r, civ))).collect();
    for (civ, stock) in stocks {
        if let Some(c) = world.civ_mut(civ) {
            c.resources = stock;
        }
    }
}

/// Can the civilization sustain this new unit? An upgrade releases the old
/// unit's actual requirements before reserving the replacement's quantities.
pub fn can_supply_unit(
    world: &World,
    r: &Ruleset,
    civ: CivId,
    base: Id<BaseUnit>,
    replacing: Option<Id<Unit>>,
) -> bool {
    let (Some(owner), Some(definition)) = (world.civ(civ), r.units.get(base)) else { return false };
    if replacing.is_some_and(|id| world.units.get(id).is_none_or(|u| u.owner != civ)) {
        return false;
    }
    if owner.is_barbarian {
        return true;
    }
    // Broken resource references must not turn a requirement into a free unit.
    if definition.required_resource.as_ref().is_some_and(|name| r.resources.id(name).is_none()) {
        return false;
    }
    let mut ctx = GameContext::new(r).with_world(world).for_civ(civ);
    if let Some(unit) = replacing.and_then(|id| world.units.get(id)) {
        ctx = ctx.on_tile(unit.position);
    }
    if definition.uniques.iter().any(|unique| {
        unique.unique_type() == Some(UniqueType::ConsumesResources)
            && applies(unique, &ctx) != Verdict::False
            && (unique.params.first().and_then(|n| n.parse::<i32>().ok()).is_none_or(|n| n <= 0)
                || unique.params.get(1).and_then(|name| r.resources.id(name)).is_none())
    }) {
        return false;
    }
    let needed = requirements(r, base, Some(&ctx));
    if needed.is_empty() {
        return true;
    }
    let mut stock = available_stock(world, r, civ);
    if let Some(old) = replacing {
        for (resource, amount) in fielded_requirements(world, r, old) {
            adjust(&mut stock, resource, i64::from(amount));
        }
    }
    needed.into_iter().all(|(resource, amount)| stock.get(&resource).copied().unwrap_or(0) >= amount)
}

/// Whether this civ is short of anything this unit needs.
pub fn is_missing_resources(r: &Ruleset, civ: &Civilization, unit: Id<BaseUnit>) -> bool {
    required_resources(r, unit).into_iter().any(|res| civ.resources.get(&res).copied().unwrap_or(0) < 0)
}

/// Improvements that extract a resource — the named one plus anything in
/// `improvedBy`.
pub fn extracting_improvements(r: &Ruleset, res: Id<TileResource>) -> Vec<String> {
    let Some(x) = r.resources.get(res) else { return Vec::new() };
    let mut out: Vec<String> = x.improvement.iter().cloned().collect();
    out.extend(x.improved_by.iter().cloned());
    out.sort();
    out.dedup();
    out
}

/// Whether a tile actually supplies its resource to this civ.
///
/// Three gates, in Unciv's order (`Tile.kt:511`):
///
/// 1. the revealing tech must be researched — an unknown resource is invisible,
///    not merely unusable;
/// 2. on a city centre, the *possibility* of extraction is enough. Civ V gives
///    you resources under your city without making you build a mine on your
///    own streets, provided some improvement exists that you could have built;
/// 3. anywhere else, the extracting improvement must actually be there, and its
///    own tech researched.
pub fn provides_resource(
    world: &crate::world::world::World,
    r: &Ruleset,
    civ: &Civilization,
    tile: crate::world::grid::TileId,
) -> bool {
    let Some(res) = world.grid.resource[tile.0 as usize] else { return false };
    // 1. revealed?
    if !is_visible(r, civ, res) {
        return false;
    }

    let improvements = extracting_improvements(r, res);
    let buildable_with_known_tech = |name: &str| {
        r.improvements.by_name(name).is_some_and(|imp| {
            imp.turns_to_build.is_some_and(|t| t != -1)
                && imp.tech_required.as_deref().is_none_or(|t| r.technologies.id(t).is_some_and(|id| civ.has_tech(id)))
        })
    };

    // 2. city centre: could you extract it, in principle?
    if world.cities.iter().any(|(_, c)| c.centre == tile) {
        return improvements.is_empty() || improvements.iter().any(|i| buildable_with_known_tech(i));
    }

    // 3. otherwise the improvement has to be standing.
    let Some(built) = world.grid.unpillaged_improvement(tile) else { return false };
    let Some(built) = r.improvements.get(built) else { return false };
    improvements.contains(&built.name)
        && built.tech_required.as_deref().is_none_or(|t| r.technologies.id(t).is_some_and(|id| civ.has_tech(id)))
}

pub fn resource_matches(r: &Ruleset, id: Id<TileResource>, filter: &str) -> bool {
    let Some(res) = r.resources.get(id) else { return false };
    crate::world::filters::multi_filter(filter, &|f| {
        if f == "All"
            || f == "all"
            || f == res.name
            || f == res.resource_type
            || res.uniques.iter().any(|u| u.text == f)
        {
            Verdict::True
        } else {
            Verdict::False
        }
    }) == Verdict::True
}
pub fn building_requirements(
    world: &crate::world::world::World,
    r: &Ruleset,
    city: &crate::world::city::City,
    id: Id<crate::ruleset::Building>,
) -> ResourceStock {
    let mut out = ResourceStock::new();
    let Some(b) = r.buildings.get(id) else { return out };
    if let Some(res) = b.required_resource.as_deref().and_then(|n| r.resources.id(n)) {
        adjust(&mut out, res, 1);
    }
    let mut ctx = GameContext::new(r).with_world(world).for_civ(city.owner);
    ctx.city = world.cities.iter().find(|(_, c)| c.centre == city.centre).map(|(id, _)| id);
    for u in b
        .uniques
        .iter()
        .filter(|u| u.unique_type() == Some(UniqueType::ConsumesResources) && applies(u, &ctx) == Verdict::True)
    {
        if let (Some(amount), Some(res)) =
            (u.params.first().and_then(|p| p.parse::<i64>().ok()), u.params.get(1).and_then(|p| r.resources.id(p)))
        {
            adjust(&mut out, res, amount.max(0));
        }
    }
    out
}

/// Recompute a civ's net stock: what its territory yields, less what its
/// standing units consume.
///
/// The deficit is the whole point. A civ with no Iron and one Swordsman is at
/// -1, which is why that Swordsman fights at -25% — not because the civ "has no
/// Iron" in the abstract, but because it is one short of what it fielded.
///
/// A tile only counts once [`provides_resource`] is satisfied, and a strategic
/// resource supplies its whole deposit rather than one unit — which is why a
/// single Iron tile can support several Swordsmen.
pub fn recompute_stock(
    world: &crate::world::world::World,
    r: &Ruleset,
    civ_id: crate::world::grid::CivId,
) -> ResourceStock {
    let mut stock = ResourceStock::new();
    if let Some(resource) = crate::world::city_state_actions::unique_luxury(world, civ_id) {
        adjust(&mut stock, resource, 1);
    }
    let Some(civ) = world.civ(civ_id) else { return stock };

    let mut produced: BTreeMap<(Option<Id<crate::world::city::City>>, Id<TileResource>), i32> = BTreeMap::new();
    for tile in world.grid.ids() {
        if world.grid.owner[tile.0 as usize] != Some(civ_id) {
            continue;
        }
        let city =
            world.cities.iter().find(|(_, city)| city.owner == civ_id && city.tiles.contains(&tile)).map(|(id, _)| id);
        if let Some(res) = world.grid.resource[tile.0 as usize].filter(|_| provides_resource(world, r, civ, tile)) {
            let resource = &r.resources[res];
            let mut amount =
                if resource.resource_type == "Strategic" { world.grid.resource_amount[tile.0 as usize] } else { 1 };
            if resource.resource_type == "Luxury"
                && city
                    .and_then(|id| world.cities.get(id))
                    .is_some_and(|c| c.uniques.has(UniqueType::ProvidesExtraLuxuryFromCityResources))
            {
                amount += 1;
            }
            *produced.entry((city, res)).or_default() += amount.max(0);
        }
        if let Some(imp) = world.grid.unpillaged_improvement(tile).and_then(|id| r.improvements.get(id)) {
            let mut ctx = GameContext::new(r).with_world(world).for_civ(civ_id).on_tile(tile);
            ctx.city = city;
            for u in imp.uniques.iter().filter(|u| applies(u, &ctx) == Verdict::True) {
                if let (Some(amount), Some(res)) = (
                    u.params.first().and_then(|p| p.parse::<i32>().ok()),
                    u.params.get(1).and_then(|p| r.resources.id(p)),
                ) {
                    match u.unique_type() {
                        Some(UniqueType::ProvidesResources) => {
                            *produced.entry((city, res)).or_default() += amount.max(0)
                        }
                        Some(UniqueType::ConsumesResources) => adjust(&mut stock, res, -i64::from(amount.max(0))),
                        _ => {}
                    }
                }
            }
        }
    }
    let ctx = GameContext::new(r).with_world(world).for_civ(civ_id);
    for u in civ.uniques.matching(UniqueType::ProvidesResources).filter(|u| applies(u, &ctx) == Verdict::True) {
        if let (Some(amount), Some(res)) =
            (u.params.first().and_then(|p| p.parse::<i32>().ok()), u.params.get(1).and_then(|p| r.resources.id(p)))
        {
            *produced.entry((None, res)).or_default() += amount.max(0);
        }
    }
    for city in &civ.cities {
        let Some(c) = world.cities.get(*city) else { continue };
        let ctx = ctx.in_city(*city);
        for u in c
            .uniques
            .matching(UniqueType::ProvidesResources)
            .filter(|u| u.is_local() && applies(u, &ctx) == Verdict::True)
        {
            if let (Some(amount), Some(res)) =
                (u.params.first().and_then(|p| p.parse::<i32>().ok()), u.params.get(1).and_then(|p| r.resources.id(p)))
            {
                *produced.entry((Some(*city), res)).or_default() += amount.max(0);
            }
        }
        for building in c.built.iter().filter(|id| !c.free_buildings.contains(id)) {
            for (res, n) in building_requirements(world, r, c, *building) {
                adjust(&mut stock, res, -i64::from(n));
            }
        }
    }
    for ((city, res), amount) in produced {
        let mut ctx = ctx;
        ctx.city = city;
        let modifiers: Vec<_> = city
            .and_then(|id| world.cities.get(id))
            .map(|c| crate::world::religion::city_uniques(world, r, c))
            .unwrap_or_else(|| civ.uniques.all().collect());
        let percent: i64 = modifiers
            .into_iter()
            .filter(|u| {
                u.unique_type() == Some(UniqueType::PercentResourceProduction)
                    && applies(u, &ctx) == Verdict::True
                    && u.params.get(1).is_some_and(|f| resource_matches(r, res, f))
            })
            .filter_map(|u| u.params.first()?.parse::<i64>().ok())
            .sum();
        adjust(&mut stock, res, (i64::from(amount) * (100 + percent) / 100).max(0));
    }

    for (id, unit) in world.units.iter() {
        if unit.owner != civ_id {
            continue;
        }
        for (res, amount) in fielded_requirements(world, r, id) {
            adjust(&mut stock, res, -i64::from(amount));
        }
    }
    stock
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::grid::CivId;

    fn ruleset() -> Ruleset {
        Ruleset::load_dir("rules/gods-and-kings").expect("readable").expect("loads")
    }

    fn civ(r: &Ruleset) -> Civilization {
        Civilization::new(CivId(0), r.nations.ids().next().unwrap(), false)
    }

    #[test]
    fn units_declare_the_resources_they_need() {
        let r = ruleset();
        let iron = r.resources.id("Iron").expect("Iron exists");

        let swordsman = r.unit("Swordsman").expect("Swordsman exists");
        assert_eq!(required_resources(&r, swordsman), vec![iron]);

        let warrior = r.unit("Warrior").expect("Warrior exists");
        assert!(required_resources(&r, warrior).is_empty(), "a Warrior needs nothing");
    }

    #[test]
    fn a_deficit_is_what_counts_not_an_absence() {
        let r = ruleset();
        let mut c = civ(&r);
        let iron = r.resources.id("Iron").unwrap();
        let swordsman = r.unit("Swordsman").unwrap();

        // No entry at all reads as zero, which is not a deficit — a civ with no
        // Swordsmen and no Iron is not penalised for anything.
        assert!(!is_missing_resources(&r, &c, swordsman));

        c.resources.insert(iron, 2);
        assert!(!is_missing_resources(&r, &c, swordsman), "two spare Iron");

        c.resources.insert(iron, -1);
        assert!(is_missing_resources(&r, &c, swordsman), "one Swordsman too many");

        let warrior = r.unit("Warrior").unwrap();
        assert!(!is_missing_resources(&r, &c, warrior), "a Warrior does not care");
    }

    #[test]
    fn a_fielded_unit_puts_a_civ_into_deficit() {
        use crate::world::entities::Unit;
        use crate::world::grid::{TileGrid, TileId};
        use crate::world::world::World;

        let r = ruleset();
        let g = TileGrid::new(8, 8, r.terrain("Grassland").unwrap());
        let mut w = World::new(g, vec![civ(&r)]);
        let iron = r.resources.id("Iron").unwrap();
        let swordsman = r.unit("Swordsman").unwrap();

        // No Iron anywhere, no units: nothing owed, nothing owned.
        assert!(recompute_stock(&w, &r, CivId(0)).is_empty());

        // Field a Swordsman and the civ is one Iron short — which is exactly
        // the state Unciv's own test scenario is in.
        w.place_unit(Unit::new(swordsman, CivId(0), TileId(0), 2));
        let stock = recompute_stock(&w, &r, CivId(0));
        assert_eq!(stock.get(&iron), Some(&-1));

        // Owning the Iron is not enough. Unimproved, and with the revealing
        // tech unresearched, the tile supplies nothing.
        w.grid.resource[5] = Some(iron);
        w.grid.resource_amount[5] = 2;
        w.grid.owner[5] = Some(CivId(0));
        assert_eq!(recompute_stock(&w, &r, CivId(0)).get(&iron), Some(&-1), "an unworked deposit is not a supply");

        // Research what reveals it and build what extracts it, and the deposit
        // arrives whole — strategic resources give their amount, not one, which
        // is why one Iron tile can field several Swordsmen.
        let res = &r.resources[iron];
        if let Some(tech) = res.revealed_by.as_deref().and_then(|t| r.technologies.id(t)) {
            w.civ_mut(CivId(0)).unwrap().techs.insert(tech);
        }
        let mine = extracting_improvements(&r, iron);
        let mine = mine.first().expect("Iron is extracted by something");
        println!("Iron: revealed by {:?}, extracted by {mine}", res.revealed_by);
        if let Some(imp) = r.improvements.id(mine) {
            w.grid.improvement[5] = Some(imp);
            if let Some(tech) = r.improvements[imp].tech_required.as_deref().and_then(|t| r.technologies.id(t)) {
                w.civ_mut(CivId(0)).unwrap().techs.insert(tech);
            }
        }
        assert_eq!(
            recompute_stock(&w, &r, CivId(0)).get(&iron),
            Some(&1),
            "a deposit of 2 less the one Swordsman consuming it"
        );
    }

    /// A city sits on top of its resources without needing a mine on its
    /// own streets — Civ asks only that extraction be possible.
    #[test]
    fn a_city_centre_supplies_what_is_under_it() {
        use crate::world::city::City;
        use crate::world::grid::{TileGrid, TileId};
        use crate::world::world::World;

        let r = ruleset();
        let g = TileGrid::new(8, 8, r.terrain("Grassland").unwrap());
        let mut w = World::new(g, vec![civ(&r)]);
        let iron = r.resources.id("Iron").unwrap();

        let centre = TileId(20);
        w.grid.resource[centre.0 as usize] = Some(iron);
        w.grid.resource_amount[centre.0 as usize] = 3;
        w.found_city(City::found("Rome", CivId(0), centre));

        // Still needs the revealing tech.
        assert!(!recompute_stock(&w, &r, CivId(0)).contains_key(&iron));

        let res = &r.resources[iron];
        for tech in [res.revealed_by.as_deref()].into_iter().flatten() {
            if let Some(t) = r.technologies.id(tech) {
                w.civ_mut(CivId(0)).unwrap().techs.insert(t);
            }
        }
        // And the tech for something that could extract it.
        for name in extracting_improvements(&r, iron) {
            if let Some(t) = r
                .improvements
                .by_name(&name)
                .and_then(|i| i.tech_required.as_deref())
                .and_then(|t| r.technologies.id(t))
            {
                w.civ_mut(CivId(0)).unwrap().techs.insert(t);
            }
        }
        assert_eq!(
            recompute_stock(&w, &r, CivId(0)).get(&iron),
            Some(&3),
            "no improvement built, yet the city has its Iron"
        );
    }

    #[test]
    fn how_many_units_need_a_resource() {
        let r = ruleset();
        let needy: Vec<&str> = r
            .units
            .iter()
            .filter(|(id, _)| !required_resources(&r, *id).is_empty())
            .map(|(_, u)| u.name.as_str())
            .collect();
        println!("{} of {} units need a resource", needy.len(), r.units.len());
        assert!(needy.len() > 20, "only {} found", needy.len());
        assert!(needy.contains(&"Swordsman"));
    }

    #[test]
    fn resource_quantities_add_the_field_and_every_consumption_unique() {
        let mut r = ruleset();
        let swordsman = r.units.id("Swordsman").unwrap();
        r.units.get_mut(swordsman).unwrap().uniques.extend([
            crate::Unique::parse("Consumes [2] [Iron]"),
            crate::Unique::parse("Consumes [3] [Iron]"),
            crate::Unique::parse("Consumes [4] [Horses]"),
        ]);
        let iron = r.resources.id("Iron").unwrap();
        let horses = r.resources.id("Horses").unwrap();
        assert_eq!(resource_requirements(&r, swordsman), BTreeMap::from([(iron, 6), (horses, 4)]));
        let mut w = World::new(crate::world::grid::TileGrid::new(4, 4, r.terrain("Grassland").unwrap()), vec![civ(&r)]);
        w.place_unit(Unit::new(swordsman, CivId(0), crate::world::grid::TileId(0), 2));
        assert_eq!(recompute_stock(&w, &r, CivId(0)), BTreeMap::from([(iron, -6), (horses, -4)]));
    }

    #[test]
    fn current_stock_includes_live_trades_and_ignores_stale_cache() {
        use crate::world::trade::{Deal, Item, Running};
        let r = ruleset();
        let mut other = civ(&r);
        other.id = CivId(1);
        let mut w =
            World::new(crate::world::grid::TileGrid::new(4, 4, r.terrain("Grassland").unwrap()), vec![civ(&r), other]);
        let swordsman = r.units.id("Swordsman").unwrap();
        let iron = r.resources.id("Iron").unwrap();
        w.civ_mut(CivId(0)).unwrap().resources.insert(iron, 99);
        assert!(!can_supply_unit(&w, &r, CivId(0), swordsman, None));
        w.diplomacy.deals.push(Deal {
            a: CivId(1),
            b: CivId(0),
            a_gives: vec![Running { item: Item::Strategic(iron, 1), turns_left: 3 }],
            b_gives: vec![],
        });
        assert!(can_supply_unit(&w, &r, CivId(0), swordsman, None));
        w.place_unit(Unit::new(swordsman, CivId(0), crate::world::grid::TileId(0), 2));
        assert!(!can_supply_unit(&w, &r, CivId(0), swordsman, None));
        w.diplomacy.deals[0].a_gives[0].turns_left = 0;
        assert_eq!(available_stock(&w, &r, CivId(0)).get(&iron), Some(&-1));
    }

    #[test]
    fn a_pillaged_extractor_stops_supplying_resources_immediately() {
        let r = ruleset();
        let mut w = World::new(crate::world::grid::TileGrid::new(4, 4, r.terrain("Grassland").unwrap()), vec![civ(&r)]);
        let iron = r.resources.id("Iron").unwrap();
        w.civ_mut(CivId(0)).unwrap().techs.extend(r.technologies.ids());
        w.grid.owner[5] = Some(CivId(0));
        w.grid.resource[5] = Some(iron);
        w.grid.resource_amount[5] = 3;
        w.grid.improvement[5] = r.improvements.id("Mine");
        assert_eq!(available_stock(&w, &r, CivId(0)).get(&iron), Some(&3));
        w.grid.pillage_improvement(crate::world::grid::TileId(5));
        assert!(!available_stock(&w, &r, CivId(0)).contains_key(&iron));
    }

    #[test]
    fn resource_conditions_use_the_owners_current_technology() {
        let mut r = ruleset();
        let warrior = r.units.id("Warrior").unwrap();
        let iron = r.resources.id("Iron").unwrap();
        r.units
            .get_mut(warrior)
            .unwrap()
            .uniques
            .push(crate::Unique::parse("Consumes [2] [Iron] <after discovering [Mining]>"));
        let mut w = World::new(crate::world::grid::TileGrid::new(4, 4, r.terrain("Grassland").unwrap()), vec![civ(&r)]);
        assert!(can_supply_unit(&w, &r, CivId(0), warrior, None));
        w.place_unit(Unit::new(warrior, CivId(0), crate::world::grid::TileId(0), 2));
        assert!(!recompute_stock(&w, &r, CivId(0)).contains_key(&iron));
        w.civ_mut(CivId(0)).unwrap().techs.insert(r.technologies.id("Mining").unwrap());
        assert!(!can_supply_unit(&w, &r, CivId(0), warrior, None));
        assert_eq!(recompute_stock(&w, &r, CivId(0)).get(&iron), Some(&-2));
    }
}
