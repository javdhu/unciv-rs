//! Civilizations and units.
//!
//! Both live in [`Arena`](crate::arena::Arena)s rather than being indexed
//! densely like tiles, because both are created and destroyed during a game and
//! the generational id is what stops a dead unit's handle resolving to whatever
//! took its slot.

use crate::arena::Id;
use crate::ruleset::{BaseUnit, Nation, Technology};
use crate::stats::Fixed;
use crate::world::city::City;
use crate::world::grid::{CivId, TileId};
use std::collections::BTreeSet;

/// A player, human or otherwise.
#[derive(Debug, Clone, PartialEq)]
pub struct Civilization {
    pub id: CivId,
    pub nation: Id<Nation>,
    pub is_ai: bool,
    /// Barbarians are a civilization in the data model and not one in the
    /// game: permanently hostile, never at peace, never diplomatic.
    pub is_barbarian: bool,
    pub gold: i32,
    /// Science accumulated toward the current research.
    pub science_stored: i32,
    /// Ordered, so iteration and serialisation are reproducible.
    pub techs: BTreeSet<Id<Technology>>,
    pub researching: Option<Id<Technology>>,
    /// Culture accumulated toward the next policy.
    pub culture_stored: i32,
    /// Ordered, so iteration and serialisation are reproducible.
    pub adopted_policies: std::collections::BTreeSet<Id<crate::ruleset::Policy>>,
    /// Points toward the next great person of each kind, and what it costs.
    pub great_people: crate::world::great_people::GreatPeople,
    /// Every unique currently in force for this civ, indexed for lookup.
    /// Rebuilt whenever policies or techs change — never patched.
    pub uniques: crate::world::unique_index::UniqueIndex,
    /// Net stock of each strategic resource: what is owned less what standing
    /// units consume. Negative means a shortfall, and units that need it fight
    /// at a penalty.
    pub resources: crate::world::resources::ResourceStock,
    pub cities: Vec<Id<City>>,
    pub units: Vec<Id<Unit>>,
    /// A civ with no cities and no units is out, but stays in the list so its
    /// id never gets reused and old references stay meaningful.
    pub defeated: bool,
}

impl Civilization {
    pub fn new(id: CivId, nation: Id<Nation>, is_ai: bool) -> Civilization {
        Civilization {
            id,
            nation,
            is_ai,
            is_barbarian: false,
            gold: 0,
            science_stored: 0,
            techs: BTreeSet::new(),
            researching: None,
            culture_stored: 0,
            adopted_policies: Default::default(),
            great_people: Default::default(),
            uniques: Default::default(),
            resources: Default::default(),
            cities: Vec::new(),
            units: Vec::new(),
            defeated: false,
        }
    }

    pub fn is_alive(&self) -> bool {
        !self.defeated && (!self.cities.is_empty() || !self.units.is_empty())
    }

    pub fn has_tech(&self, t: Id<Technology>) -> bool {
        self.techs.contains(&t)
    }
}

/// A unit on the map.
#[derive(Debug, Clone, PartialEq)]
pub struct Unit {
    pub base: Id<BaseUnit>,
    pub owner: CivId,
    pub position: TileId,
    /// Out of 100.
    pub health: u8,
    /// Movement points left this turn, in the same thousandths as everything
    /// else — Civ has half-moves.
    pub movement_left: Fixed,
    pub fortified: bool,
}

impl Unit {
    pub fn new(base: Id<BaseUnit>, owner: CivId, position: TileId, movement: i32) -> Unit {
        Unit {
            base,
            owner,
            position,
            health: 100,
            movement_left: Fixed::whole(movement),
            fortified: false,
        }
    }

    pub fn is_hurt(&self) -> bool {
        self.health < 100
    }

    pub fn can_move(&self) -> bool {
        self.movement_left > Fixed::ZERO
    }

    /// Called at the start of each of the owner's turns.
    pub fn refresh(&mut self, movement: i32) {
        self.movement_left = Fixed::whole(movement);
    }
}
