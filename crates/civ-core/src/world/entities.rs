// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

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
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, PartialEq)]
pub struct Civilization {
    pub id: CivId,
    pub nation: Id<Nation>,
    pub is_ai: bool,
    /// Plays the AI as it stood before docs/AI.md's levels, so the balance
    /// runner can seat old against new in one game. Never saved; false in play.
    #[cfg_attr(feature = "serde", serde(skip))]
    pub legacy_ai: bool,
    /// Led by the nation's second leader (`altLeader`), whose ability
    /// replaces the nation's own uniques. Chosen at setup.
    #[cfg_attr(feature = "serde", serde(default))]
    pub alt_leader: bool,
    /// The last era this civ may research into: a Blitz slice's
    /// (`world::slices`). None is no limit.
    #[cfg_attr(feature = "serde", serde(default))]
    pub tech_era_cap: Option<i32>,
    /// Barbarians are a civilization in the data model and not one in the
    /// game: permanently hostile, never at peace, never diplomatic.
    pub is_barbarian: bool,
    /// City-states are a civilization that never wins and never expands. They
    /// are a *source* of things — culture, food, happiness, units — that major
    /// civs compete over, which is why they are modelled as civs at all rather
    /// than as a kind of city.
    pub city_state_type: Option<Id<crate::ruleset::CityStateType>>,
    pub city_state: crate::world::city_state_actions::CityStateData,
    pub gold: i32,
    /// Unassigned overflow from a completed technology.
    pub science_stored: i32,
    pub science_from_agreements: i32,
    /// Investment belongs to a technology and survives a change in selection.
    #[cfg_attr(feature = "serde", serde(with = "crate::serialization::map_as_entries"))]
    pub science_invested: std::collections::BTreeMap<Id<Technology>, i32>,
    /// Ordered, so iteration and serialisation are reproducible.
    pub techs: BTreeSet<Id<Technology>>,
    pub researching: Option<Id<Technology>>,
    /// Culture accumulated toward the next policy.
    pub culture_stored: i32,
    pub culture_for_contests: i32,
    pub faith_for_contests: i32,
    /// Ordered, so iteration and serialisation are reproducible.
    pub adopted_policies: std::collections::BTreeSet<Id<crate::ruleset::Policy>>,
    /// Only culture-paid policies advance the policy cost curve.
    pub paid_policies: u32,
    #[cfg_attr(feature = "serde", serde(with = "crate::serialization::map_as_entries"))]
    pub timed_policy_expirations: std::collections::BTreeMap<Id<crate::ruleset::Policy>, u32>,
    pub pending_rewards: crate::world::effects::PendingRewards,
    /// Persistent city grants keyed by their source category (for example Culture).
    pub free_building_grants: std::collections::BTreeMap<String, BTreeSet<Id<City>>>,
    /// Faith, pantheon and religion.
    pub faith: crate::world::religion::Faith,
    pub victory: crate::world::victory_actions::CivVictory,
    /// How many of each item this civ has bought at a price that rises with
    /// each purchase — a faith-bought Great Prophet, say — by name.
    pub bought_with_increasing_price: std::collections::BTreeMap<String, u32>,
    /// Turns since a city-state last paid tribute, counting down from twenty
    /// (`CivFlags.RecentlyBullied`); see `tribute`.
    pub recently_bullied: u32,
    /// Points toward the next great person of each kind, and what it costs.
    pub great_people: crate::world::great_people::GreatPeople,
    /// Happiness banked toward the next golden age, and the one running.
    pub golden_age: crate::world::golden_age::GoldenAge,
    /// Science banked in each of the last eight turns, indexed by turn mod
    /// eight (`scienceOfLast8Turns`). What a Great Scientist is worth.
    pub recent_science: [i32; 8],
    /// The last rewards ancient ruins gave, oldest first, not to be repeated.
    pub recent_ruins: Vec<String>,
    /// Natural wonders this civilization has discovered, counted once per kind.
    pub known_natural_wonders: BTreeSet<Id<crate::ruleset::Terrain>>,
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
    #[cfg_attr(feature = "serde", serde(with = "crate::serialization::map_as_entries"))]
    pub resources: crate::world::resources::ResourceStock,
    pub cities: Vec<Id<City>>,
    pub units: Vec<Id<Unit>>,
    /// Retained after losses and expending units, for permanent unlocks.
    pub units_gained: BTreeSet<Id<BaseUnit>>,
    pub revolt_countdown: Option<u32>,
    /// Once established, losing every city ends the civilization even when
    /// military units or Settlers remain (`Civilization.isDefeated`).
    pub has_ever_owned_original_capital: bool,
    /// Defeat has been settled and remaining units and agreements removed.
    /// The civilization stays registered so old references remain meaningful.
    pub defeated: bool,
    /// The turn it stood in each era by, indexed by era number: an era it
    /// started in, or skipped, reads the turn it got there. Recorded as each
    /// round closes ([`crate::world::turn`]); absent from older saves, whose
    /// eras are recorded from the next round on.
    #[cfg_attr(feature = "serde", serde(default))]
    pub era_starts: Vec<u32>,
}

impl Civilization {
    pub fn add_culture(&mut self, amount: i32) {
        self.culture_stored = self.culture_stored.saturating_add(amount);
        self.culture_for_contests = self.culture_for_contests.saturating_add(amount.max(0));
    }
    pub fn add_faith(&mut self, amount: i32) {
        self.faith.stored = self.faith.stored.saturating_add(amount);
        self.faith_for_contests = self.faith_for_contests.saturating_add(amount.max(0));
    }

    pub fn new(id: CivId, nation: Id<Nation>, is_ai: bool) -> Civilization {
        Civilization {
            id,
            nation,
            is_ai,
            legacy_ai: false,
            alt_leader: false,
            tech_era_cap: None,
            is_barbarian: false,
            city_state_type: None,
            city_state: Default::default(),
            espionage: Default::default(),
            quests: Default::default(),
            gold: 0,
            science_stored: 0,
            science_from_agreements: 0,
            science_invested: Default::default(),
            techs: BTreeSet::new(),
            researching: None,
            culture_stored: 0,
            culture_for_contests: 0,
            faith_for_contests: 0,
            adopted_policies: Default::default(),
            paid_policies: 0,
            timed_policy_expirations: Default::default(),
            pending_rewards: Default::default(),
            free_building_grants: Default::default(),
            faith: Default::default(),
            victory: Default::default(),
            bought_with_increasing_price: Default::default(),
            recently_bullied: 0,
            great_people: Default::default(),
            golden_age: Default::default(),
            recent_science: [0; 8],
            recent_ruins: Vec::new(),
            known_natural_wonders: BTreeSet::new(),
            uniques: Default::default(),
            resources: Default::default(),
            cities: Vec::new(),
            units: Vec::new(),
            units_gained: Default::default(),
            revolt_countdown: None,
            has_ever_owned_original_capital: false,
            defeated: false,
            era_starts: Vec::new(),
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
        !self.defeated
            && if self.has_ever_owned_original_capital { !self.cities.is_empty() } else { !self.units.is_empty() }
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

    /// The ability of whoever leads this civ: the nation's uniques, or its
    /// second leader's.
    pub fn leader_uniques<'r>(&self, r: &'r crate::ruleset::Ruleset) -> &'r [crate::Unique] {
        r.nations.get(self.nation).map_or(&[], |n| n.leader_uniques(self.alt_leader))
    }
}

/// A unit on the map.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
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
    /// Movement, or movement-consuming actions such as pillaging/upgrading.
    /// Ending orders (Skip/Fortify) and working in place do not count.
    pub moved_this_turn: bool,
    /// An attack or interception attempt; ordinary defense does not count.
    pub attacked_this_turn: bool,
    /// Normal attacks and interception attempts share the per-turn allowance.
    pub attacks_this_turn: u8,
    /// The carrier actually carrying this aircraft, independent of its tile.
    pub transport: Option<Id<Unit>>,
    pub fortified: bool,
    pub fortification_turns: u8,
    pub set_up: bool,
    pub embarked: bool,
    pub attacks_left: u8,
    pub xp: u16,
    /// Paid promotion choices, excluding starting/granted promotions.
    pub promotions_earned: u16,
    pub promotions: BTreeSet<Id<Promotion>>,
    /// The creed carried by this unit, independent of its current owner.
    pub religion: Option<CivId>,
    pub religious_strength: i32,
    pub religious_uses_left: u8,
    /// A human's standing order to work or explore on its own. AI units
    /// never carry it; their plans already do this. Absent from older saves.
    #[cfg_attr(feature = "serde", serde(default))]
    pub automation: Option<crate::world::automation::Automation>,
    /// A Luminary's own name (`world::luminaries`). Absent from older saves.
    #[cfg_attr(feature = "serde", serde(default))]
    pub name: Option<String>,
}

impl Unit {
    pub fn new(base: Id<BaseUnit>, owner: CivId, position: TileId, movement: i32) -> Unit {
        Unit {
            base,
            owner,
            position,
            health: 100,
            movement_left: Fixed::whole(movement),
            moved_this_turn: false,
            attacked_this_turn: false,
            attacks_this_turn: 0,
            transport: None,
            fortified: false,
            fortification_turns: 0,
            set_up: false,
            embarked: false,
            attacks_left: 1,
            xp: 0,
            promotions_earned: 0,
            promotions: BTreeSet::new(),
            religion: None,
            religious_strength: 0,
            religious_uses_left: 0,
            automation: None,
            name: None,
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
        r.unit_type_of(self.base).map(|ut| ut.movement_type.as_str())
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

    /// Unit, inherited type and promotion rules, with starting promotions
    /// included once even for a unit created through the low-level constructor.
    pub fn all_uniques<'a>(&'a self, r: &'a Ruleset) -> impl Iterator<Item = &'a crate::Unique> {
        let base = r.units.get(self.base);
        base.into_iter()
            .flat_map(|b| &b.uniques)
            .chain(r.unit_type_of(self.base).into_iter().flat_map(|t| &t.uniques))
            .chain(self.promotions.iter().filter_map(|id| r.promotions.get(*id)).flat_map(|p| &p.uniques))
            .chain(
                base.into_iter()
                    .flat_map(|b| &b.promotions)
                    .filter_map(|name| r.promotions.id(name))
                    .filter(|id| !self.promotions.contains(id))
                    .filter_map(|id| r.promotions.get(id))
                    .flat_map(|p| &p.uniques),
            )
    }

    pub fn has_unique(&self, r: &Ruleset, ut: UniqueType) -> bool {
        self.all_uniques(r).any(|u| u.unique_type() == Some(ut))
    }

    pub fn is_hurt(&self) -> bool {
        self.health < 100
    }

    pub fn can_move(&self) -> bool {
        self.movement_left > Fixed::ZERO
    }

    pub fn spend_attack(&mut self) {
        self.attacks_left = self.attacks_left.saturating_sub(1);
        self.attacks_this_turn = self.attacks_this_turn.saturating_add(1);
        self.attacked_this_turn = true;
    }

    /// Called at the start of each of the owner's turns.
    pub fn refresh(&mut self, movement: i32) {
        self.movement_left = Fixed::whole(movement);
        self.attacks_left = 1;
        self.moved_this_turn = false;
        self.attacked_this_turn = false;
        self.attacks_this_turn = 0;
    }
}
