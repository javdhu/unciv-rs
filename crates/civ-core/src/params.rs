// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! A unique's parameters, compiled once.
//!
//! Every parameter is text — `[+2 Food]`, `[3]`, `[{Military} {Water}]`,
//! `[Writing]` — and reading it is not free: a number is parsed, a yield list
//! split, a filter broken into its terms and each term looked up in every
//! registry it might name. The rules used to do that on every question, so a
//! city's yield re-read the same `[in cities connected to the capital]` for
//! each of its buildings, every time the AI priced a construction.
//!
//! [`Unique::compiled`] reads each parameter once and keeps the result beside
//! the unique: numbers and yields parsed, filters split into [`Term`]s, and
//! each term classified against every matcher's vocabulary with its names
//! resolved to ids. What depends on the world — which tile, which unit, whose
//! city — stays in the matchers (`world::filters`, `world::conditionals`);
//! what depends only on the text and the ruleset is decided here.
//!
//! # Once per ruleset
//!
//! Names mean different things in different rulesets, so a compiled unique
//! remembers the [`Stamp`] of the ruleset it was resolved against. A ruleset
//! warms its own uniques at load; anything else — a unique read from a save,
//! or built at runtime — compiles on first use. Clones share the result, so
//! the copies a civ's unique index takes every turn do not compile again.
//! Asked about a different ruleset, a unique compiles afresh rather than
//! answering with the wrong names.
//!
//! The ruleset is immutable once loaded (`ruleset` module note): a registry
//! entry added afterwards is not seen by uniques already compiled.

use crate::arena::Id;
use crate::ruleset::{
    BaseUnit, Belief, Building, Era, Policy, Promotion, Ruleset, Technology, Terrain, TileImprovement, TileResource,
    UnitType,
};
use crate::stats::{Fixed, Stat, Stats};
use crate::world::conditionals::{TileWord, Verdict};
use crate::world::filters::{BuildingWord, CityWord, UnitWord};
use crate::Unique;
use std::borrow::Cow;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};

/// Which ruleset a compiled unique was resolved against.
///
/// Every ruleset draws its own when it is created; a clone keeps it, as it
/// keeps the same names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Stamp(u64);

impl Stamp {
    pub(crate) fn fresh() -> Stamp {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        Stamp(NEXT.fetch_add(1, Ordering::Relaxed))
    }
}

impl Default for Stamp {
    fn default() -> Stamp {
        Stamp::fresh()
    }
}

/// The compiled form a unique carries. Not part of its identity: equality,
/// debug output and saves see only the text.
///
/// Shared, filled or not, by every clone: world stores are copied whole when a
/// staged world first writes to them, so a clone must cost one count, not a
/// fresh cell.
#[derive(Clone, Default)]
pub(crate) struct Cell(Arc<OnceLock<Params>>);

impl Unique {
    /// This unique's parameters, compiled against `r`.
    ///
    /// Borrowed whenever the unique has met `r` before, which is every time
    /// after the first; owned only when asked about a ruleset other than the
    /// one it was compiled for.
    #[inline]
    pub fn compiled(&self, r: &Ruleset) -> Cow<'_, Params> {
        match self.compiled.0.get() {
            Some(params) if params.stamp == r.stamp => Cow::Borrowed(params),
            _ => self.compile(r),
        }
    }

    #[cold]
    fn compile(&self, r: &Ruleset) -> Cow<'_, Params> {
        let params = self.compiled.0.get_or_init(|| Params::compile(self, r));
        if params.stamp == r.stamp {
            Cow::Borrowed(params)
        } else {
            Cow::Owned(Params::compile(self, r))
        }
    }
}

/// Every parameter of one unique, in order.
#[derive(Clone)]
pub struct Params {
    stamp: Stamp,
    params: Box<[Param]>,
}

impl Params {
    fn compile(unique: &Unique, r: &Ruleset) -> Params {
        Params { stamp: r.stamp, params: unique.params.iter().map(|p| Param::compile(r, p)).collect() }
    }

    pub fn get(&self, i: usize) -> Option<&Param> {
        self.params.get(i)
    }

    /// `text.parse::<i32>()`.
    pub fn i32(&self, i: usize) -> Option<i32> {
        self.get(i).and_then(Param::i32)
    }

    /// `text.parse::<i64>()`.
    pub fn i64(&self, i: usize) -> Option<i64> {
        self.get(i).and_then(|p| p.int)
    }

    /// `text.parse::<u32>()`.
    pub fn u32(&self, i: usize) -> Option<u32> {
        self.get(i).and_then(|p| p.uint).and_then(|n| u32::try_from(n).ok())
    }

    /// `text.parse::<usize>()`.
    pub fn usize(&self, i: usize) -> Option<usize> {
        self.get(i).and_then(|p| p.uint).and_then(|n| usize::try_from(n).ok())
    }

    /// `text.trim().parse::<i32>()`.
    pub fn number(&self, i: usize) -> Option<i32> {
        self.get(i).and_then(|p| p.number)
    }

    /// [`Fixed::parse`].
    pub fn fixed(&self, i: usize) -> Option<Fixed> {
        self.get(i).and_then(|p| p.fixed)
    }

    /// [`Stats::parse`].
    pub fn stats(&self, i: usize) -> Option<Stats> {
        self.get(i).and_then(|p| p.stats)
    }

    /// [`Stat::parse`].
    pub fn stat(&self, i: usize) -> Option<Stat> {
        self.get(i).and_then(|p| p.stat)
    }

    /// The parameter read as a filter.
    pub fn filter(&self, i: usize) -> Option<&Filter> {
        self.get(i).map(|p| &p.filter)
    }

    /// The parameter read whole, as one name.
    pub fn term(&self, i: usize) -> Option<&Term> {
        self.get(i).map(Param::term)
    }
}

/// One parameter, read every way the rules read parameters.
#[derive(Clone)]
pub struct Param {
    int: Option<i64>,
    uint: Option<u64>,
    number: Option<i32>,
    fixed: Option<Fixed>,
    stats: Option<Stats>,
    stat: Option<Stat>,
    filter: Filter,
    /// The untrimmed text as a single term, when it is not already the
    /// filter's only term.
    whole: Option<Box<Term>>,
}

impl Param {
    fn compile(r: &Ruleset, text: &str) -> Param {
        let filter = Filter::compile(r, text);
        let whole = match &filter {
            Filter::Term(term) if *term.text == *text => None,
            _ => Some(Box::new(Term::new(r, text))),
        };
        Param {
            int: text.parse().ok(),
            uint: text.parse().ok(),
            number: text.trim().parse().ok(),
            fixed: Fixed::parse(text),
            stats: Stats::parse(text),
            stat: Stat::parse(text),
            filter,
            whole,
        }
    }

    pub fn i32(&self) -> Option<i32> {
        self.int.and_then(|n| i32::try_from(n).ok())
    }

    pub fn filter(&self) -> &Filter {
        &self.filter
    }

    pub fn term(&self) -> &Term {
        match (&self.whole, &self.filter) {
            (Some(whole), _) | (None, Filter::Term(whole)) => whole,
            (None, _) => unreachable!("a parameter that is not one term keeps its whole text"),
        }
    }
}

/// The filter language, parsed: a term, a negation `non-[X]`, or a
/// conjunction `{A} {B}`. The same reading as
/// [`multi_filter`](crate::world::filters::multi_filter), done once.
#[derive(Debug, Clone)]
pub enum Filter {
    Term(Box<Term>),
    Not(Box<Filter>),
    All(Box<[Filter]>),
}

impl Filter {
    /// Compile filter text against a ruleset. The string-taking matchers go
    /// through this, so there is one reading of the language.
    pub fn compile(r: &Ruleset, text: &str) -> Filter {
        let text = text.trim();
        if let Some(inner) = text.strip_prefix("non-[").and_then(|s| s.strip_suffix(']')) {
            return Filter::Not(Box::new(Filter::compile(r, inner)));
        }
        if let Some(parts) = crate::world::filters::conjunction(text) {
            return Filter::All(parts.into_iter().map(|part| Filter::compile(r, part)).collect());
        }
        Filter::Term(Box::new(Term::new(r, text)))
    }

    /// Apply the filter to a single-term test. Unknown wins over true and
    /// loses to false, as in `multi_filter`.
    pub fn test(&self, test: &impl Fn(&Term) -> Verdict) -> Verdict {
        match self {
            Filter::Term(term) => test(term),
            Filter::Not(inner) => match inner.test(test) {
                Verdict::True => Verdict::False,
                Verdict::False => Verdict::True,
                Verdict::Unknown => Verdict::Unknown,
            },
            Filter::All(parts) => {
                let mut verdict = Verdict::True;
                for part in parts.iter() {
                    match part.test(test) {
                        Verdict::True => {}
                        Verdict::False => return Verdict::False,
                        Verdict::Unknown => verdict = Verdict::Unknown,
                    }
                }
                verdict
            }
        }
    }
}

/// One term of a filter: its text, what each matcher's vocabulary makes of
/// it, and every registry entry it names.
#[derive(Debug, Clone)]
pub struct Term {
    pub text: Box<str>,
    pub unit: UnitWord,
    pub building: BuildingWord,
    pub tile: TileWord,
    pub city: CityWord,
    /// `pre-[Era]`: the era, or `Some(None)` for an era this ruleset lacks.
    pub pre_era: Option<Option<Id<Era>>>,
    /// `Bonus resource`, `Luxury resource`…: a resource type some resource has.
    pub resource_kind: bool,
    pub names: Names,
}

/// The registry entries a term's text names, one per registry.
#[derive(Debug, Clone, Default)]
pub struct Names {
    pub building: Option<Id<Building>>,
    pub unit: Option<Id<BaseUnit>>,
    pub unit_type: Option<Id<UnitType>>,
    pub promotion: Option<Id<Promotion>>,
    pub terrain: Option<Id<Terrain>>,
    pub resource: Option<Id<TileResource>>,
    pub improvement: Option<Id<TileImprovement>>,
    pub tech: Option<Id<Technology>>,
    pub policy: Option<Id<Policy>>,
    pub belief: Option<Id<Belief>>,
    pub era: Option<Id<Era>>,
}

impl Term {
    pub fn new(r: &Ruleset, text: &str) -> Term {
        Term {
            text: text.into(),
            unit: UnitWord::of(text),
            building: BuildingWord::of(text),
            tile: TileWord::of(text),
            city: CityWord::of(text),
            pre_era: text.strip_prefix("pre-[").and_then(|s| s.strip_suffix(']')).map(|era| r.eras.id(era)),
            resource_kind: text
                .strip_suffix(" resource")
                .is_some_and(|kind| r.resources.values().any(|x| x.resource_type == kind)),
            names: Names {
                building: r.buildings.id(text),
                unit: r.units.id(text),
                unit_type: r.unit_types.id(text),
                promotion: r.promotions.id(text),
                terrain: r.terrains.id(text),
                resource: r.resources.id(text),
                improvement: r.improvements.id(text),
                tech: r.technologies.id(text),
                policy: r.policies.id(text),
                belief: r.beliefs.id(text),
                era: r.eras.id(text),
            },
        }
    }
}

/// Compile every unique a ruleset holds, so play never pays for it.
pub(crate) fn warm(r: &Ruleset) {
    fn visit(r: &Ruleset, u: &Unique) {
        let _ = u.compiled(r);
        for c in &u.conditionals {
            visit(r, c);
        }
    }
    let all = r
        .buildings
        .values()
        .flat_map(|b| &b.uniques)
        .chain(r.units.values().flat_map(|u| &u.uniques))
        .chain(r.unit_types.values().flat_map(|t| &t.uniques))
        .chain(r.terrains.values().flat_map(|t| &t.uniques))
        .chain(r.improvements.values().flat_map(|i| &i.uniques))
        .chain(r.resources.values().flat_map(|x| &x.uniques))
        .chain(r.promotions.values().flat_map(|p| &p.uniques))
        .chain(r.policies.values().flat_map(|p| &p.uniques))
        .chain(r.policy_branches.values().flat_map(|p| &p.uniques))
        .chain(r.technologies.values().flat_map(|t| &t.uniques))
        .chain(r.nations.values().flat_map(|n| &n.uniques))
        .chain(r.nations.values().filter_map(|n| n.alt_leader.as_ref()).flat_map(|a| &a.uniques))
        .chain(r.beliefs.values().flat_map(|b| &b.uniques))
        .chain(r.eras.values().flat_map(|e| &e.uniques))
        .chain(r.city_state_types.values().flat_map(|t| &t.uniques))
        .chain(r.ruin_rewards.values().flat_map(|x| &x.uniques))
        .chain(&r.global_uniques);
    for u in all {
        visit(r, u);
    }
}
