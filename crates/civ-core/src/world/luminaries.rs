//! Luminaries' names, region first (docs/CONTENT.md, Great people).
//!
//! A unit whose rules say `[This Unit] gets a name from the [X] group` takes
//! the next name of group X nobody in the game has been given: first from
//! its nation's own region, then from the group's shared pool, then from the
//! other regions in turn, one name from each. Pools are written in
//! historical order, so an early Luminary is an early figure.

use crate::arena::Id;
use crate::ruleset::civgame::{NameGroup, REGIONS};
use crate::ruleset::Ruleset;
use crate::world::entities::Unit;
use crate::world::world::World;
use crate::UniqueType;

/// The group a unit draws its name from, if it gets one.
pub fn group_of<'r>(r: &'r Ruleset, unit: &Unit) -> Option<&'r NameGroup> {
    let base = r.units.get(unit.base)?;
    let name = base
        .uniques
        .iter()
        .find(|u| u.unique_type() == Some(UniqueType::OneTimeUnitGetsName))
        .and_then(|u| u.params.get(1))?;
    r.name_groups.iter().find(|g| g.name == *name)
}

/// Every name `group` offers a nation of `region`, in the order they are given.
pub fn candidates<'g>(group: &'g NameGroup, region: Option<&str>) -> Vec<&'g str> {
    let own = region.and_then(|r| group.regions.get(r));
    let mut out: Vec<&str> = own.into_iter().flatten().map(String::as_str).collect();
    out.extend(group.unit_names.iter().map(String::as_str));
    // The rest of the world, a name from each region in turn; regions the
    // list of eight does not know come after, in the file's order.
    let others: Vec<&Vec<String>> = REGIONS
        .iter()
        .filter_map(|name| group.regions.get(*name).filter(|_| Some(*name) != region))
        .chain(
            group
                .regions
                .iter()
                .filter(|(name, _)| !REGIONS.contains(&name.as_str()) && Some(name.as_str()) != region)
                .map(|(_, v)| v),
        )
        .collect();
    let longest = others.iter().map(|v| v.len()).max().unwrap_or(0);
    for i in 0..longest {
        out.extend(others.iter().filter_map(|v| v.get(i)).map(String::as_str));
    }
    out
}

/// Give a newly placed unit its name, if its rules call for one.
pub fn name_unit(world: &mut World, r: &Ruleset, id: Id<Unit>) {
    let Some(unit) = world.units.get(id) else { return };
    if unit.name.is_some() {
        return;
    }
    let Some(group) = group_of(r, unit) else { return };
    let region = world.civ(unit.owner).and_then(|c| r.nations.get(c.nation)).and_then(|n| n.region.as_deref());
    let Some(name) = candidates(group, region).into_iter().find(|n| !world.given_names.contains(*n)) else { return };
    let name = name.to_string();
    world.given_names.insert(name.clone());
    world.units.get_mut(id).expect("placed unit").name = Some(name);
}
