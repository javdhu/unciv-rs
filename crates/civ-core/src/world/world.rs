// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Everything that changes during a game.
//!
//! The split from [`Ruleset`](crate::ruleset) is the important one: the ruleset
//! is what a game is played *with* and is immutable once loaded, so it can be
//! shared freely — including across threads during the read phase of a turn.
//! This is what mutates.

use crate::arena::{Arena, Id};
use crate::shared::Shared;
use crate::world::city::City;
use crate::world::entities::{Civilization, Unit};
use crate::world::fog::Visibility;
use crate::world::grid::{CivId, TileGrid, TileId};

#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, PartialEq)]
pub struct World {
    pub grid: Shared<TileGrid>,
    pub visibility: Shared<Visibility>,
    pub cities: Shared<Arena<City>>,
    pub units: Shared<Arena<Unit>>,
    /// Indexed by [`CivId`]; civs are fixed at game start and never removed, so
    /// a defeated one stays in place with `defeated` set.
    pub civs: Shared<Vec<Civilization>>,
    /// Relations between civs. Symmetric facts live here once, rather than
    /// twice inside each civ where they could disagree.
    pub diplomacy: Shared<crate::world::diplomacy::Diplomacy>,
    pub turn: u32,
    /// Match difficulty. None uses the bundled Prince default.
    pub difficulty: Option<Id<crate::ruleset::Difficulty>>,
    /// Game speed. None is Standard. Absent from older saves, which were all Standard.
    #[cfg_attr(feature = "serde", serde(default))]
    pub speed: Option<Id<crate::ruleset::Speed>>,
    /// Who has won, once someone has. A game is won once; without this the
    /// victory event fires again every turn afterwards.
    pub winner: Option<(CivId, Id<crate::ruleset::Victory>)>,
    /// The completed round on which the result was first awarded. This stays
    /// fixed if a caller continues simulating after the game has been won.
    pub victory_turn: Option<u32>,
    pub endgame: crate::world::victory_actions::Endgame,
    #[cfg_attr(feature = "serde", serde(with = "crate::serialization::map_as_entries"))]
    pub barbarian_camps: std::collections::BTreeMap<TileId, crate::world::barbarians::Camp>,
    /// Every name a unit has been given (`world::luminaries`), so no two
    /// Luminaries in a game share one.
    #[cfg_attr(feature = "serde", serde(default))]
    pub given_names: std::collections::BTreeSet<String>,
    /// Cultural influence and trade shares (civ.game victories); empty unless
    /// a victory in the ruleset names them.
    #[cfg_attr(feature = "serde", serde(default))]
    pub influence: crate::world::influence::Influence,
    /// The Blitz slice this game plays (`world::slices`), if any.
    #[cfg_attr(feature = "serde", serde(default))]
    pub slice: Option<crate::world::slices::Slice>,
    /// Faiths that arise by era (civ.game); empty unless the ruleset has them.
    #[cfg_attr(feature = "serde", serde(default))]
    pub faiths: crate::world::faiths::FaithWorld,
}

impl World {
    pub fn new(grid: TileGrid, civs: Vec<Civilization>) -> World {
        let tiles = grid.len();
        World {
            visibility: Visibility::new(tiles, civs.len()).into(),
            grid: grid.into(),
            cities: Arena::new().into(),
            units: Arena::new().into(),
            civs: civs.into(),
            diplomacy: Default::default(),
            turn: 0,
            difficulty: None,
            speed: None,
            winner: None,
            victory_turn: None,
            endgame: Default::default(),
            barbarian_camps: Default::default(),
            given_names: Default::default(),
            influence: Default::default(),
            slice: None,
            faiths: Default::default(),
        }
    }

    pub fn game_difficulty<'r>(&self, r: &'r crate::ruleset::Ruleset) -> Option<&'r crate::ruleset::Difficulty> {
        self.difficulty.and_then(|id| r.difficulties.get(id)).or_else(|| r.default_difficulty())
    }

    /// The game's speed: the one it was set up with, or else (a world built
    /// by hand) the speed whose modifier is `modifier`. Speeds are told apart
    /// by id, not by modifier: Blitz and Standard may share one.
    pub fn speed_rules<'r>(
        &self,
        r: &'r crate::ruleset::Ruleset,
        modifier: crate::stats::Fixed,
    ) -> Option<&'r crate::ruleset::Speed> {
        self.speed.and_then(|id| r.speeds.get(id)).or_else(|| r.speeds.values().find(|s| s.modifier == modifier))
    }

    /// Whether this civ plays the old AI (`Civilization::legacy_ai`).
    pub fn is_legacy_ai(&self, id: CivId) -> bool {
        self.civ(id).is_some_and(|c| c.legacy_ai)
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
    pub fn found_city(&mut self, mut city: City) -> Id<City> {
        let (owner, centre) = (city.owner, city.centre);
        // The ruleset-free entry point still founds a real city. A captured
        // original capital does not become replaceable when its founder loses
        // its last city; only Palace assignment requires the ruleset below.
        city.original_capital = !self.cities.iter().any(|(_, c)| c.founder == owner && c.original_capital);
        let original_capital = city.original_capital;
        city.turn_acquired = self.turn;
        let id = self.cities.insert(city);
        if let Some(c) = self.civ_mut(owner) {
            c.cities.push(id);
            c.has_ever_owned_original_capital |= original_capital;
        }
        // A city owns its centre outright.
        self.claim_tile(id, centre);
        id
    }

    /// Bring a tile inside a city's borders, for the city's owner.
    ///
    /// The one place a tile changes hands to a city, so the civ-level owner
    /// column and the city's own list cannot disagree about it.
    pub fn claim_tile(&mut self, city: Id<City>, tile: TileId) {
        if !self.grid.contains(tile) || self.cities.get(city).is_none() {
            return;
        }
        // Founding inside an existing city's borders and acquiring tiles both
        // transfer the city-level claim, even when the civilization is unchanged.
        for (other, value) in self.cities.iter_mut() {
            if other != city {
                value.tiles.retain(|t| *t != tile);
                value.worked.retain(|t| *t != tile);
                value.locked_tiles.retain(|t| *t != tile);
            }
        }
        let Some(c) = self.cities.get_mut(city) else { return };
        if let Err(at) = c.tiles.binary_search(&tile) {
            c.tiles.insert(at, tile);
        }
        self.grid.owner[tile.0 as usize] = Some(c.owner);
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
        let coastal = self
            .grid
            .neighbours(self.cities.get(id).map_or(TileId(0), |c| c.centre))
            .any(|n| r.terrains.get(self.grid.terrain[n.0 as usize]).is_some_and(|t| t.name == "Coast"));
        if let Some(c) = self.cities.get_mut(id) {
            c.coastal = coastal;
        }

        if first {
            let palace = r
                .buildings
                .iter()
                .find(|(_, b)| b.uniques.iter().any(|u| u.unique_type() == Some(crate::UniqueType::IndicatesCapital)))
                .map(|(id, _)| id);
            if let (Some(palace), Some(city)) = (palace, self.cities.get_mut(id)) {
                city.built.insert(palace);
                crate::world::city::rebuild_city_uniques(r, city);
            }
        }
        crate::world::religion::on_city_founded(self, id);
        crate::world::policies::rebuild_uniques(self, r, owner);
        crate::world::effects::refresh_free_buildings(self, r, owner);
        id
    }

    pub fn place_unit(&mut self, unit: Unit) -> Id<Unit> {
        let owner = unit.owner;
        let base = unit.base;
        let id = self.units.insert(unit);
        if let Some(c) = self.civ_mut(owner) {
            c.units.push(id);
            c.units_gained.insert(base);
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
            // BattleUnitCapture.kt:117: a captured unit takes no standing orders along.
            u.automation = None;
        }
        let carrier = self.units.get(id).and_then(|u| u.transport);
        if carrier.is_some_and(|carrier| self.units.get(carrier).is_none_or(|u| u.owner != to)) {
            self.units.get_mut(id).expect("unit exists").transport = None;
        }
        if let Some(c) = self.civ_mut(from) {
            c.units.retain(|u| *u != id);
        }
        let base = self.units.get(id).expect("transferred unit").base;
        if let Some(c) = self.civ_mut(to) {
            c.units.push(id);
            c.units_gained.insert(base);
        }
        let payload: Vec<_> = self.units.iter().filter(|(_, u)| u.transport == Some(id)).map(|(id, _)| id).collect();
        for cargo in payload {
            self.transfer_unit(cargo, to);
        }
    }

    /// Remove a unit and unregister it. The id stops resolving afterwards.
    pub fn remove_unit(&mut self, id: Id<Unit>) -> Option<Unit> {
        let unit = self.units.remove(id)?;
        if let Some(c) = self.civ_mut(unit.owner) {
            c.units.retain(|u| *u != id);
        }
        let payload: Vec<_> = self.units.iter().filter(|(_, u)| u.transport == Some(id)).map(|(id, _)| id).collect();
        for cargo in payload {
            self.remove_unit(cargo);
        }
        Some(unit)
    }

    /// Settle defeat after an action has finished, never during intermediate
    /// mutations such as consuming a Settler before founding its city.
    pub fn resolve_defeats(&mut self) -> Vec<CivId> {
        let defeated: Vec<_> =
            self.civs.iter().filter(|c| !c.is_barbarian && !c.defeated && !c.is_alive()).map(|c| c.id).collect();
        for &id in &defeated {
            let units: Vec<_> = self.units.iter().filter(|(_, u)| u.owner == id).map(|(uid, _)| uid).collect();
            for unit in units {
                self.remove_unit(unit);
            }
            if let Some(c) = self.civ_mut(id) {
                c.defeated = true;
                c.espionage.spies.clear();
                c.quests.active.clear();
                c.quests.assistance.clear();
                c.quests.individual_next.clear();
                c.quests.global_seeded = false;
            }
            self.diplomacy.end_agreements_for(id);
            self.visibility.clear_visible(id);
        }
        defeated
    }

    /// Units standing on a tile, in id order.
    pub fn units_at(&self, tile: TileId) -> Vec<Id<Unit>> {
        self.units.iter().filter(|(_, u)| u.position == tile).map(|(id, _)| id).collect()
    }
}
