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
use crate::world::entities::Civilization;
use crate::UniqueType;
use std::collections::BTreeMap;

/// Percent applied when a unit's resource is in deficit.
pub const MISSING_RESOURCE_MALUS: i32 = -25;

/// A civilization's net stock of each resource.
///
/// Ordered, so iteration and serialisation are reproducible.
pub type ResourceStock = BTreeMap<Id<TileResource>, i32>;

/// Resources a unit needs to be at full strength.
///
/// Both the `requiredResource` field and any `Consumes [amount] [resource]`
/// unique; Unciv reads the same two places.
pub fn required_resources(r: &Ruleset, unit: Id<BaseUnit>) -> Vec<Id<TileResource>> {
    let Some(u) = r.units.get(unit) else { return Vec::new() };
    let mut out = Vec::new();

    if let Some(name) = u.required_resource.as_deref() {
        if let Some(id) = r.resources.id(name) {
            out.push(id);
        }
    }
    for unique in &u.uniques {
        if unique.unique_type() == Some(UniqueType::ConsumesResources) {
            // "Consumes [amount] [resource]" — the resource is the second slot.
            if let Some(id) = unique.params.get(1).and_then(|n| r.resources.id(n)) {
                out.push(id);
            }
        }
    }
    out.sort();
    out.dedup();
    out
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
    let Some(resource) = r.resources.get(res) else { return false };

    // 1. revealed?
    if let Some(tech) = resource.revealed_by.as_deref() {
        if !r.technologies.id(tech).is_some_and(|t| civ.has_tech(t)) {
            return false;
        }
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
    let Some(built) = world.grid.improvement[tile.0 as usize] else { return false };
    let Some(built) = r.improvements.get(built) else { return false };
    improvements.contains(&built.name)
        && built.tech_required.as_deref().is_none_or(|t| r.technologies.id(t).is_some_and(|id| civ.has_tech(id)))
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
    let Some(civ) = world.civ(civ_id) else { return stock };

    for tile in world.grid.ids() {
        if world.grid.owner[tile.0 as usize] != Some(civ_id) {
            continue;
        }
        let Some(res) = world.grid.resource[tile.0 as usize] else { continue };
        if !provides_resource(world, r, civ, tile) {
            continue;
        }
        let strategic = r.resources.get(res).is_some_and(|x| x.resource_type == "Strategic");
        let amount = if strategic { world.grid.resource_amount[tile.0 as usize] } else { 1 };
        *stock.entry(res).or_insert(0) += amount;
    }

    for (_, unit) in world.units.iter() {
        if unit.owner != civ_id {
            continue;
        }
        for res in required_resources(r, unit.base) {
            *stock.entry(res).or_insert(0) -= 1;
        }
    }
    stock
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::grid::CivId;

    fn ruleset() -> Ruleset {
        Ruleset::load_dir("../android/assets/jsons/Civ V - Gods & Kings").expect("readable").expect("loads")
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
}
