//! Civilizations and units.
//!
//! Both live in [`Arena`](crate::arena::Arena)s rather than being indexed
//! densely like tiles, because both are created and destroyed during a game and
//! the generational id is what stops a dead unit's handle resolving to whatever
//! took its slot.

use crate::arena::Id;
use crate::ruleset::{BaseUnit, Nation, Promotion, Ruleset, Technology};
use crate::stats::Fixed;
use crate::unique_type::UniqueType;
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
    /// City-states are a civilization that never wins and never expands. They
    /// are a *source* of things — culture, food, happiness, units — that major
    /// civs compete over, which is why they are modelled as civs at all rather
    /// than as a kind of city.
    pub city_state_type: Option<Id<crate::ruleset::CityStateType>>,
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
    /// Faith, pantheon and religion.
    pub faith: crate::world::religion::Faith,
    /// How many of each item this civ has bought at a price that rises with
    /// each purchase — a faith-bought Great Prophet, say — by name.
    pub bought_with_increasing_price: std::collections::BTreeMap<String, u32>,
    /// Points toward the next great person of each kind, and what it costs.
    pub great_people: crate::world::great_people::GreatPeople,
    /// Spies, and which eras have already granted one.
    pub espionage: crate::world::espionage::Espionage,
    /// What this city-state is asking of the major civs. Empty for everyone
    /// else — a major civ sets no quests.
    pub quests: crate::world::quests::QuestBook,
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
            city_state_type: None,
            espionage: Default::default(),
            quests: Default::default(),
            gold: 0,
            science_stored: 0,
            techs: BTreeSet::new(),
            researching: None,
            culture_stored: 0,
            adopted_policies: Default::default(),
            faith: Default::default(),
            bought_with_increasing_price: Default::default(),
            great_people: Default::default(),
            uniques: Default::default(),
            resources: Default::default(),
            cities: Vec::new(),
            units: Vec::new(),
            defeated: false,
        }
    }

    /// A city-state is one that has been given a type. Nothing else
    /// distinguishes it — it is an ordinary civ that happens never to be
    /// playing to win.
    pub fn is_city_state(&self) -> bool {
        self.city_state_type.is_some()
    }

    /// A civ that is playing the game: not barbarians, not a city-state.
    pub fn is_major(&self) -> bool {
        !self.is_barbarian && !self.is_city_state()
    }

    pub fn is_alive(&self) -> bool {
        !self.defeated && (!self.cities.is_empty() || !self.units.is_empty())
    }

    pub fn owns_city(&self, city: Id<City>) -> bool {
        self.cities.contains(&city)
    }

    pub fn owns_unit(&self, unit: Id<Unit>) -> bool {
        self.units.contains(&unit)
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
    pub embarked: bool,
    pub attacks_left: u8,
    pub xp: u16,
    pub promotions: BTreeSet<Id<Promotion>>,
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
            embarked: false,
            attacks_left: 1,
            xp: 0,
            promotions: BTreeSet::new(),
        }
    }

    pub fn is_civilian(&self, r: &Ruleset) -> bool {
        r.units.get(self.base).is_some_and(|b| b.strength.unwrap_or(0) == 0 && b.ranged_strength.unwrap_or(0) == 0)
    }

    pub fn is_military(&self, r: &Ruleset) -> bool {
        !self.is_civilian(r)
    }

    pub fn is_ranged(&self, r: &Ruleset) -> bool {
        r.units.get(self.base).is_some_and(|b| b.ranged_strength.unwrap_or(0) > 0)
    }

    pub fn movement_type<'a>(&self, r: &'a Ruleset) -> Option<&'a str> {
        r.units.get(self.base).and_then(|b| r.unit_types.by_name(&b.unit_type)).map(|ut| ut.movement_type.as_str())
    }

    pub fn is_water_unit(&self, r: &Ruleset) -> bool {
        self.movement_type(r) == Some("Water")
    }

    pub fn is_land_unit(&self, r: &Ruleset) -> bool {
        self.movement_type(r) == Some("Land")
    }

    pub fn is_air(&self, r: &Ruleset) -> bool {
        self.movement_type(r) == Some("Air")
    }

    pub fn has_unique(&self, r: &Ruleset, ut: UniqueType) -> bool {
        if let Some(base) = r.units.get(self.base) {
            if base.uniques.iter().any(|u| u.unique_type() == Some(ut)) {
                return true;
            }
        }
        for promo_id in &self.promotions {
            if let Some(p) = r.promotions.get(*promo_id) {
                if p.uniques.iter().any(|u| u.unique_type() == Some(ut)) {
                    return true;
                }
            }
        }
        false
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
        self.attacks_left = 1;
    }
}
