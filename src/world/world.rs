//! Everything that changes during a game.
//!
//! The split from [`Ruleset`](crate::ruleset) is the important one: the ruleset
//! is what a game is played *with* and is immutable once loaded, so it can be
//! shared freely — including across threads during the read phase of a turn.
//! This is what mutates.

use crate::arena::{Arena, Id};
use crate::world::city::City;
use crate::world::entities::{Civilization, Unit};
use crate::world::fog::Visibility;
use crate::world::grid::{CivId, TileGrid, TileId};

#[derive(Debug, Clone, PartialEq)]
pub struct World {
    pub grid: TileGrid,
    pub visibility: Visibility,
    pub cities: Arena<City>,
    pub units: Arena<Unit>,
    /// Indexed by [`CivId`]; civs are fixed at game start and never removed, so
    /// a defeated one stays in place with `defeated` set.
    pub civs: Vec<Civilization>,
    /// Relations between civs. Symmetric facts live here once, rather than
    /// twice inside each civ where they could disagree.
    pub diplomacy: crate::world::diplomacy::Diplomacy,
    pub turn: u32,
    /// Who has won, once someone has. A game is won once; without this the
    /// victory event fires again every turn afterwards.
    pub winner: Option<(CivId, Id<crate::ruleset::Victory>)>,
}

impl World {
    pub fn new(grid: TileGrid, civs: Vec<Civilization>) -> World {
        let tiles = grid.len();
        World {
            visibility: Visibility::new(tiles, civs.len()),
            grid,
            cities: Arena::new(),
            units: Arena::new(),
            civs,
            diplomacy: Default::default(),
            turn: 0,
            winner: None,
        }
    }

    pub fn civ(&self, id: CivId) -> Option<&Civilization> {
        self.civs.get(id.0 as usize)
    }

    pub fn civ_mut(&mut self, id: CivId) -> Option<&mut Civilization> {
        self.civs.get_mut(id.0 as usize)
    }

    /// Civs in turn order — id order, which is also the order plans are
    /// applied in, so the sequence is the same on every machine.
    pub fn civ_ids(&self) -> impl Iterator<Item = CivId> + '_ {
        self.civs.iter().map(|c| c.id)
    }

    pub fn living_civs(&self) -> impl Iterator<Item = CivId> + '_ {
        self.civs.iter().filter(|c| c.is_alive()).map(|c| c.id)
    }

    /// Place a city and register it with its owner.
    ///
    /// Without a ruleset the capital gets no palace; see [`found_city_in`].
    pub fn found_city(&mut self, city: City) -> Id<City> {
        let (owner, centre) = (city.owner, city.centre);
        let id = self.cities.insert(city);
        if let Some(c) = self.civ_mut(owner) {
            c.cities.push(id);
        }
        // A city owns its centre outright.
        self.grid.owner[centre.0 as usize] = Some(owner);
        id
    }

    /// Found a city, granting the capital its palace.
    ///
    /// A civ's first city gets whichever building carries `IndicatesCapital` —
    /// identified by the unique rather than by the name "Palace", because a
    /// ruleset may call it something else. It matters more than it sounds: the
    /// palace is where a young empire's gold and science come from, and without
    /// it a capital on plain terrain has no income at all.
    pub fn found_city_in(&mut self, r: &crate::ruleset::Ruleset, city: City) -> Id<City> {
        let owner = city.owner;
        let first = self.civ(owner).is_none_or(|c| c.cities.is_empty());
        let id = self.found_city(city);

        if first {
            let palace = r
                .buildings
                .iter()
                .find(|(_, b)| {
                    b.uniques
                        .iter()
                        .any(|u| u.unique_type() == Some(crate::UniqueType::IndicatesCapital))
                })
                .map(|(id, _)| id);
            if let (Some(palace), Some(city)) = (palace, self.cities.get_mut(id)) {
                city.built.insert(palace);
                crate::world::city::rebuild_city_uniques(r, city);
            }
        }
        id
    }

    pub fn place_unit(&mut self, unit: Unit) -> Id<Unit> {
        let owner = unit.owner;
        let id = self.units.insert(unit);
        if let Some(c) = self.civ_mut(owner) {
            c.units.push(id);
        }
        id
    }

    /// Hand a unit to another civ, moving it between the two registries.
    ///
    /// Changing `owner` alone leaves the old civ listing a unit it no longer
    /// controls — it would count it, pay for it, and be kept alive by it.
    pub fn transfer_unit(&mut self, id: Id<Unit>, to: CivId) {
        let Some(from) = self.units.get(id).map(|u| u.owner) else { return };
        if from == to {
            return;
        }
        if let Some(u) = self.units.get_mut(id) {
            u.owner = to;
        }
        if let Some(c) = self.civ_mut(from) {
            c.units.retain(|u| *u != id);
        }
        if let Some(c) = self.civ_mut(to) {
            c.units.push(id);
        }
    }

    /// Remove a unit and unregister it. The id stops resolving afterwards.
    pub fn remove_unit(&mut self, id: Id<Unit>) -> Option<Unit> {
        let unit = self.units.remove(id)?;
        if let Some(c) = self.civ_mut(unit.owner) {
            c.units.retain(|u| *u != id);
        }
        Some(unit)
    }

    /// Units standing on a tile, in id order.
    pub fn units_at(&self, tile: TileId) -> Vec<Id<Unit>> {
        self.units.iter().filter(|(_, u)| u.position == tile).map(|(id, _)| id).collect()
    }
}
