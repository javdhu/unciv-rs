// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Leader personalities (`Personality.kt`, `PersonalityValue.kt`).
//!
//! Each AI leader rates fifteen traits from 0 to 10, with 5 as neutral, in
//! `Personalities.json`, and its nation names the leader. Humans, and nations
//! without a known personality, get the neutral one (`Civilization.kt:471`),
//! so every factor here is exactly 1 for them and nothing changes.

use crate::ruleset::{Nation, Personality, Ruleset};
use crate::stats::{Fixed, Stat};
use crate::world::grid::CivId;
use crate::world::World;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Trait {
    Production,
    Food,
    Gold,
    Science,
    Culture,
    Happiness,
    Faith,
    /// Building a military, not necessarily using it.
    Military,
    /// Using units aggressively; war buildings.
    Aggressive,
    /// 0: never declares war.
    DeclareWar,
    /// Trades and open borders.
    Commerce,
    /// Friendship, pacts and peace.
    Diplomacy,
    /// Long alliances; joining friends' wars.
    Loyal,
    Expansion,
    DenounceWillingness,
}

impl Trait {
    /// `PersonalityValue[stat]`.
    pub fn of_stat(stat: Stat) -> Option<Trait> {
        Some(match stat {
            Stat::Production => Trait::Production,
            Stat::Food => Trait::Food,
            Stat::Gold => Trait::Gold,
            Stat::Science => Trait::Science,
            Stat::Culture => Trait::Culture,
            Stat::Happiness => Trait::Happiness,
            Stat::Faith => Trait::Faith,
        })
    }
}

/// One civ's leader, or the neutral one.
#[derive(Debug, Clone, Copy)]
pub struct Leader<'r>(Option<&'r Personality>);

impl<'r> Leader<'r> {
    /// A named leader from the ruleset, or the neutral one.
    pub fn named(r: &'r Ruleset, name: &str) -> Leader<'r> {
        Leader(r.personalities.by_name(name))
    }
}

pub const NEUTRAL: Leader<'static> = Leader(None);

/// The personality this civ plays with: its leader's for an AI major civ,
/// the neutral one for a human, a city-state or an unknown name.
///
/// Unciv names it with `Nation.personality` (Civilization.kt:471). A nation
/// with two leaders (`altLeader`) has no such field, so the leader who leads
/// is looked up by name, and without an entry plays neutral.
pub fn of<'r>(world: &World, r: &'r Ruleset, civ: CivId) -> Leader<'r> {
    let Some(c) = world.civ(civ).filter(|c| c.is_ai && c.is_major()) else { return NEUTRAL };
    r.nations.get(c.nation).map_or(NEUTRAL, |nation| for_leader(r, nation, c.alt_leader))
}

/// The personality of a nation's default leader, or its second one.
pub fn for_leader<'r>(r: &'r Ruleset, nation: &Nation, alt_leader: bool) -> Leader<'r> {
    let name = match (alt_leader, nation.personality.as_deref()) {
        (false, Some(name)) => Some(name),
        _ => nation.leader(alt_leader),
    };
    Leader(name.and_then(|name| r.personalities.by_name(name)))
}

impl Leader<'_> {
    /// The raw 0..10 value (`Personality.get`).
    pub fn value(&self, t: Trait) -> i32 {
        let Some(p) = self.0 else { return 5 };
        match t {
            Trait::Production => p.production,
            Trait::Food => p.food,
            Trait::Gold => p.gold,
            Trait::Science => p.science,
            Trait::Culture => p.culture,
            Trait::Happiness => p.happiness,
            Trait::Faith => p.faith,
            Trait::Military => p.military,
            Trait::Aggressive => p.aggressive,
            Trait::DeclareWar => p.declare_war,
            Trait::Commerce => p.commerce,
            Trait::Diplomacy => p.diplomacy,
            Trait::Loyal => p.loyal,
            Trait::Expansion => p.expansion,
            Trait::DenounceWillingness => p.denounce_willingness,
        }
        .clamp(0, 10)
    }

    /// `scaledFocus`: 0 at 0, 1 at 5, 2 at 10.
    pub fn scaled(&self, t: Trait) -> Fixed {
        Fixed::from_fraction(self.value(t) as i64, 5)
    }

    /// `inverseScaledFocus`: 2 at 0, 1 at 5, 0 at 10.
    pub fn inverse(&self, t: Trait) -> Fixed {
        Fixed::from_fraction((10 - self.value(t)) as i64, 5)
    }

    /// `modifierFocus(value, weight)`: `1 + (scaled - 1) * weight`.
    pub fn modifier(&self, t: Trait, weight: Fixed) -> Fixed {
        Fixed::ONE + (self.scaled(t) - Fixed::ONE) * weight
    }

    /// `inverseModifierFocus(value, weight)`.
    pub fn inverse_modifier(&self, t: Trait, weight: Fixed) -> Fixed {
        Fixed::ONE + (self.inverse(t) - Fixed::ONE) * weight
    }

    /// How much this leader favours a policy branch (`priorities`,
    /// `PolicyManager.kt:52`); 0 when it says nothing.
    pub fn branch_priority(&self, branch: &str) -> i32 {
        self.0.and_then(|p| p.priorities.get(branch)).copied().unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_neutral_leader_changes_nothing() {
        for t in [Trait::Military, Trait::DeclareWar, Trait::Science] {
            assert_eq!(NEUTRAL.value(t), 5);
            assert_eq!(NEUTRAL.scaled(t), Fixed::ONE);
            assert_eq!(NEUTRAL.inverse(t), Fixed::ONE);
            assert_eq!(NEUTRAL.modifier(t, Fixed::permille(300)), Fixed::ONE);
        }
    }

    #[test]
    fn leaders_differ_where_their_traits_do() {
        use crate::world::ai::{city_target, war_ratio_needed};
        use crate::world::opinion::RelationshipLevel as L;
        let r = Ruleset::load_dir("rules/gods-and-kings").expect("readable").expect("loads");
        let (gandhi, napoleon, shaka) =
            (Leader::named(&r, "Gandhi"), Leader::named(&r, "Napoleon"), Leader::named(&r, "Shaka"));
        // Eager leaders go to war with a smaller edge; the neutral one keeps the old table.
        assert!(war_ratio_needed(napoleon, L::Neutral) < war_ratio_needed(NEUTRAL, L::Neutral));
        assert!(war_ratio_needed(gandhi, L::Neutral) > war_ratio_needed(NEUTRAL, L::Neutral));
        assert_eq!(war_ratio_needed(NEUTRAL, L::Neutral), Fixed::permille(1_500));
        assert_eq!(war_ratio_needed(NEUTRAL, L::Ally), Fixed::whole(2));
        // Expansion sets the size of the empire.
        assert_eq!(city_target(NEUTRAL), 5);
        assert!(city_target(gandhi) < 5 && city_target(shaka) > 5);
    }

    #[test]
    fn leaders_are_read_from_the_ruleset() {
        let r = Ruleset::load_dir("rules/gods-and-kings").expect("readable").expect("loads");
        let gandhi = Leader(r.personalities.by_name("Gandhi"));
        let genghis = Leader(r.personalities.by_name("Genghis Khan"));
        assert!(gandhi.0.is_some() && genghis.0.is_some(), "Personalities.json is loaded");
        assert!(genghis.value(Trait::Aggressive) > gandhi.value(Trait::Aggressive));
        assert_eq!(gandhi.scaled(Trait::Food), Fixed::whole(2));
        assert_eq!(gandhi.inverse(Trait::Food), Fixed::ZERO);
    }

    #[test]
    fn a_second_leader_is_looked_up_by_name() {
        let r = Ruleset::load_dir("rules/base").expect("readable").expect("loads");
        let mongols = r.nations.by_name("Mongols").expect("the Mongols are in base");
        assert!(mongols.personality.is_none() && mongols.alt_leader.is_some());
        let genghis = for_leader(&r, mongols, false);
        assert!(genghis.0.is_some_and(|p| p.name == "Genghis Khan"));
        assert!(genghis.value(Trait::Aggressive) > 5);
        // Mandukhai has no entry yet, so she plays neutral.
        let second = for_leader(&r, mongols, true);
        assert!(second.0.is_none() || second.0.is_some_and(|p| p.name == "Mandukhai"));
    }
}
