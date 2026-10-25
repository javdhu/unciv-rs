// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! City names that change with the era (docs/CONTENT.md, History on the
//! surface).
//!
//! A nation's `cityRenames` are steps of history: Edo becomes Tokyo once its
//! owner reaches the Age of Steam. A city is founded under the name its
//! founder's era would give it, and renames when its owner enters a later era
//! or when a city changes hands to an owner already there. Nothing here runs
//! for a ruleset without renames, which is every fixture.

use crate::arena::Id;
use crate::ruleset::{Nation, Ruleset};
use crate::world::city::City;
use crate::world::grid::CivId;
use crate::world::world::World;

/// `name` as a civ at `era` (an era number) of `nation` would call it,
/// following chained steps (Byzantion, Constantinople, Istanbul).
pub fn name_in_era(r: &Ruleset, nation: &Nation, name: &str, era: i32) -> String {
    let mut current = name.to_string();
    // Each step can fire once, which also stops a cyclic list.
    let mut used = vec![false; nation.city_renames.len()];
    while let Some(i) = nation.city_renames.iter().enumerate().position(|(i, step)| {
        !used[i] && step.from == current && r.eras.by_name(&step.era).is_some_and(|e| e.era_number <= era)
    }) {
        used[i] = true;
        current.clone_from(&nation.city_renames[i].to);
    }
    current
}

fn name_taken(world: &World, name: &str, except: Option<Id<City>>) -> bool {
    world.cities.iter().any(|(id, c)| Some(id) != except && c.name == name)
}

/// The era name for a new city of `civ`, unless another city already has it.
pub fn founding_name(world: &World, r: &Ruleset, civ: CivId, name: &str) -> String {
    let Some(nation) = world.civ(civ).and_then(|c| r.nations.get(c.nation)) else { return name.to_string() };
    if nation.city_renames.is_empty() {
        return name.to_string();
    }
    let era = crate::world::policies::era_number(world, r, civ);
    let renamed = name_in_era(r, nation, name, era);
    if renamed != name && !name_taken(world, &renamed, None) {
        renamed
    } else {
        name.to_string()
    }
}

/// The next name from `civ`'s own list, in its era's form, skipping any name
/// (or later form of one) a city already has or `pending` holds.
pub fn next_city_name(world: &World, r: &Ruleset, civ: CivId, pending: &[String]) -> Option<String> {
    let nation = r.nations.get(world.civ(civ)?.nation)?;
    let era = crate::world::policies::era_number(world, r, civ);
    let latest = r.eras.iter().map(|(_, e)| e.era_number).max().unwrap_or(era);
    let taken = |n: &str| name_taken(world, n, None) || pending.iter().any(|p| p == n);
    nation
        .cities
        .iter()
        .filter(|base| !taken(base) && !taken(&name_in_era(r, nation, base, era)))
        .find(|base| !taken(&name_in_era(r, nation, base, latest)))
        .map(|base| name_in_era(r, nation, base, era))
}

/// A city that took a new name this turn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Renamed {
    pub city: Id<City>,
    pub from: String,
    pub to: String,
}

/// Rename every city whose owner's era calls it something else, in city id
/// order. A name another city already holds is left alone.
pub fn apply_era_renames(world: &mut World, r: &Ruleset) -> Vec<Renamed> {
    if r.nations.iter().all(|(_, n)| n.city_renames.is_empty()) {
        return Vec::new();
    }
    let ids: Vec<_> = world.cities.iter().map(|(id, _)| id).collect();
    let mut out = Vec::new();
    for id in ids {
        let Some(city) = world.cities.get(id) else { continue };
        let Some(nation) = world.civ(city.owner).and_then(|c| r.nations.get(c.nation)) else { continue };
        if nation.city_renames.is_empty() {
            continue;
        }
        let era = crate::world::policies::era_number(world, r, city.owner);
        let to = name_in_era(r, nation, &city.name, era);
        if to == city.name || name_taken(world, &to, Some(id)) {
            continue;
        }
        let city = world.cities.get_mut(id).expect("listed city");
        let from = std::mem::replace(&mut city.name, to.clone());
        out.push(Renamed { city: id, from, to });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ruleset::civgame::CityRename;

    fn rules() -> Ruleset {
        Ruleset::load_dir("rules/gods-and-kings").unwrap().unwrap()
    }

    fn step(from: &str, to: &str, era: &str) -> CityRename {
        CityRename { from: from.into(), to: to.into(), era: era.into() }
    }

    #[test]
    fn steps_chain_in_era_order() {
        let r = rules();
        let mut n = r.nations.by_name("Rome").unwrap().clone();
        n.city_renames = vec![
            step("Constantinople", "Istanbul", "Renaissance era"),
            step("Byzantion", "Constantinople", "Classical era"),
        ];
        assert_eq!(name_in_era(&r, &n, "Byzantion", 0), "Byzantion");
        assert_eq!(name_in_era(&r, &n, "Byzantion", 1), "Constantinople");
        assert_eq!(name_in_era(&r, &n, "Byzantion", 3), "Istanbul");
        assert_eq!(name_in_era(&r, &n, "Rome", 3), "Rome");
    }

    #[test]
    fn a_cyclic_list_ends() {
        let r = rules();
        let mut n = r.nations.by_name("Rome").unwrap().clone();
        n.city_renames = vec![step("A", "B", "Ancient era"), step("B", "A", "Ancient era")];
        assert_eq!(name_in_era(&r, &n, "A", 0), "A");
    }
}
