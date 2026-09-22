//! Questions asked of the ruleset alone, with no game in progress.
//!
//! This is where systems accumulate. Each one reads the registries and the
//! derived relationships; none of them reach for global state, so they stay
//! testable against the shipped files on their own.

use super::generated::*;
use super::load::Ruleset;
use crate::arena::Id;

impl Technology {
    /// The era this tech belongs to, by name.
    ///
    /// Reads the column back-reference, which exists only because
    /// [`Ruleset::derive`](super::load) establishes it — Techs.json states the
    /// relationship as nesting, never as a field.
    pub fn era_name<'r>(&self, ruleset: &'r Ruleset) -> Option<&'r str> {
        let col = ruleset.tech_columns.get(self.column?)?;
        Some(&col.era)
    }

    pub fn era(&self, ruleset: &Ruleset) -> Option<Id<Era>> {
        ruleset.eras.id(self.era_name(ruleset)?)
    }
}

impl Policy {
    /// The branch this policy belongs to.
    ///
    /// A branch is also a policy in its own right — Unciv sets
    /// `branch.branch = branch` (`Ruleset.kt:458`) — so asking a branch which
    /// branch it is in answers with itself. Branches live in their own registry
    /// here, hence the fallback.
    pub fn branch_name<'r>(&self, ruleset: &'r Ruleset) -> Option<&'r str> {
        match self.branch {
            Some(b) => ruleset.policy_branches.name_of(b),
            None => ruleset.policy_branches.id(&self.name).and_then(|b| ruleset.policy_branches.name_of(b)),
        }
    }
}

/// Where an object sorts in the civilopedia: by the era that unlocks it.
pub trait SortGroup {
    /// Objects with no unlocking tech sort last.
    fn sort_group(&self, ruleset: &Ruleset) -> i32;
}

impl SortGroup for BaseUnit {
    fn sort_group(&self, ruleset: &Ruleset) -> i32 {
        self.required_tech
            .as_deref()
            .and_then(|t| ruleset.technologies.by_name(t))
            .and_then(|t| t.era(ruleset))
            .and_then(|e| ruleset.eras.get(e))
            .map_or(100, |e| e.era_number)
    }
}

impl SortGroup for Building {
    fn sort_group(&self, ruleset: &Ruleset) -> i32 {
        self.required_tech
            .as_deref()
            .and_then(|t| ruleset.technologies.by_name(t))
            .and_then(|t| t.era(ruleset))
            .and_then(|e| ruleset.eras.get(e))
            .map_or(100, |e| e.era_number)
    }
}

/// Ruleset objects carrying flat yields.
///
/// Unciv models these as `RulesetStatsObject`; in the json they are loose
/// `food`/`gold`/`production`/... keys repeated across terrain, resources,
/// improvements and specialists. One accessor beats spelling out seven
/// `Option` unwraps at every call site.
pub trait HasStats {
    fn stats(&self) -> crate::stats::Stats;
}

macro_rules! has_stats {
    ($($t:ty),*) => {$(
        impl HasStats for $t {
            fn stats(&self) -> crate::stats::Stats {
                use crate::stats::{Fixed, Stat, Stats};
                let mut s = Stats::ZERO;
                for (stat, v) in [
                    (Stat::Food, self.food), (Stat::Production, self.production),
                    (Stat::Gold, self.gold), (Stat::Science, self.science),
                    (Stat::Culture, self.culture), (Stat::Faith, self.faith),
                    (Stat::Happiness, self.happiness),
                ] {
                    if let Some(v) = v { s[stat] = Fixed::whole(v) }
                }
                s
            }
        }
    )*};
}

has_stats!(Terrain, TileImprovement, TileResource, Building);
