//! Matching things against the ruleset's filter vocabulary.
//!
//! Uniques are written against categories rather than lists —
//! `<vs [Mounted] units>`, `<for [{Military} {Water}] units>` — and those
//! strings are a small language. It has three forms:
//!
//! * a plain name: a unit type (`Mounted`), a category (`Melee`, `Water`), or a
//!   unit's own name (`Great General`);
//! * a conjunction in braces: `{Military} {Water}` matches a military naval
//!   unit and nothing else;
//! * a negation: `non-[Natural Wonder]`.
//!
//! Every answer is a [`Verdict`], so a filter this engine does not understand
//! stays *unknown* rather than quietly becoming false. The reasoning is the
//! same as in [`conditionals`](crate::world::conditionals): a rule that fails to
//! fire because its filter is unimplemented should be countable, not invisible.

use crate::arena::Id;
use crate::ruleset::{BaseUnit, Ruleset};
use crate::world::conditionals::Verdict;
use crate::world::entities::Unit;

/// Split `{A} {B}` into its parts, or `None` if this is not a conjunction.
fn conjunction(filter: &str) -> Option<Vec<&str>> {
    let trimmed = filter.trim();
    if !trimmed.starts_with('{') {
        return None;
    }
    let mut parts = Vec::new();
    let mut depth = 0usize;
    let mut start = 0usize;
    for (i, c) in trimmed.char_indices() {
        match c {
            '{' => {
                if depth == 0 {
                    start = i + 1;
                }
                depth += 1;
            }
            '}' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    parts.push(&trimmed[start..i]);
                }
            }
            _ => {}
        }
    }
    (!parts.is_empty()).then_some(parts)
}

/// Apply the filter language on top of a single-term test.
///
/// Unknown wins over true and loses to false: a conjunction with one false term
/// is definitely false, but one with an unknown term and no false one cannot be
/// decided.
pub fn multi_filter(filter: &str, test: &impl Fn(&str) -> Verdict) -> Verdict {
    let filter = filter.trim();

    if let Some(inner) = filter.strip_prefix("non-[").and_then(|s| s.strip_suffix(']')) {
        return match multi_filter(inner, test) {
            Verdict::True => Verdict::False,
            Verdict::False => Verdict::True,
            Verdict::Unknown => Verdict::Unknown,
        };
    }

    if let Some(parts) = conjunction(filter) {
        let mut verdict = Verdict::True;
        for part in parts {
            match multi_filter(part, test) {
                Verdict::True => {}
                Verdict::False => return Verdict::False,
                Verdict::Unknown => verdict = Verdict::Unknown,
            }
        }
        return verdict;
    }

    test(filter)
}

/// Categories derived from a unit's own numbers, as Unciv derives them
/// (`BaseUnit.kt:500`).
fn category(r: &Ruleset, base: &BaseUnit, term: &str) -> Verdict {
    let ranged = base.ranged_strength.unwrap_or(0) > 0;
    let melee = !ranged && base.strength.unwrap_or(0) > 0;
    let military = ranged || melee;
    let movement = r.unit_types.by_name(&base.unit_type).map(|t| t.movement_type.as_str());

    let yes = |b: bool| if b { Verdict::True } else { Verdict::False };

    match term {
        "all" | "All" => Verdict::True,
        "Ranged" => yes(ranged),
        "Melee" => yes(melee),
        "Military" => yes(military),
        "Civilian" => yes(!military),
        "Land" => movement.map_or(Verdict::Unknown, |m| yes(m == "Land")),
        "Water" => movement.map_or(Verdict::Unknown, |m| yes(m == "Water")),
        "Air" => movement.map_or(Verdict::Unknown, |m| yes(m == "Air")),
        "non-air" => movement.map_or(Verdict::Unknown, |m| yes(m != "Air")),
        _ => {
            if term == base.name || term == base.unit_type {
                return Verdict::True;
            }
            if base.replaces.as_deref() == Some(term) {
                return Verdict::True;
            }
            // A term naming a unit type or unit this ruleset has is a definite
            // no; anything else is beyond this function.
            if r.unit_types.id(term).is_some() || r.units.id(term).is_some() {
                Verdict::False
            } else {
                Verdict::Unknown
            }
        }
    }
}

/// Does a unit match a filter, given its state as well as its type?
pub fn unit_matches(r: &Ruleset, unit: &Unit, filter: &str) -> Verdict {
    let Some(base) = r.units.get(unit.base) else { return Verdict::Unknown };
    multi_filter(filter, &|term| match term {
        // Stateful terms, which a bare BaseUnit cannot answer.
        "Wounded" | "wounded" | "wounded units" => {
            if unit.is_hurt() {
                Verdict::True
            } else {
                Verdict::False
            }
        }
        "Embarked" | "embarked" => {
            if unit.embarked {
                Verdict::True
            } else {
                Verdict::False
            }
        }
        "Barbarian" | "barbarians" | "City-State" => Verdict::Unknown, // needs civ kind
        _ => category(r, base, term),
    })
}

/// The type-only form, for questions about a unit that does not exist yet —
/// what a city may build, what a promotion could apply to.
pub fn base_unit_matches(r: &Ruleset, base: Id<BaseUnit>, filter: &str) -> Verdict {
    let Some(b) = r.units.get(base) else { return Verdict::Unknown };
    multi_filter(filter, &|term| category(r, b, term))
}

/// Does a building type match a building filter?
///
/// Names, "All", and the wonder categories — the terms the purchase uniques in
/// the shipped rulesets use. A term that is none of these, and not some other
/// building's name either, is [`Verdict::Unknown`]: a stat category like
/// "Culture" needs knowing what the building yields, which this does not ask.
pub fn building_matches(r: &Ruleset, id: Id<crate::ruleset::Building>, filter: &str) -> Verdict {
    let Some(b) = r.buildings.get(id) else { return Verdict::Unknown };
    let yes = |v: bool| if v { Verdict::True } else { Verdict::False };
    let world_wonder = b.is_wonder == Some(true);
    let national_wonder = b.is_national_wonder == Some(true);
    multi_filter(filter, &|term| match term {
        "All" | "all" | "Building" | "Buildings" => Verdict::True,
        "Wonder" | "Wonders" => yes(world_wonder || national_wonder),
        "World Wonder" | "World Wonders" => yes(world_wonder),
        "National Wonder" | "National Wonders" => yes(national_wonder),
        t if t == b.name => Verdict::True,
        t if r.buildings.id(t).is_some() => Verdict::False,
        _ => Verdict::Unknown,
    })
}

/// Does a city match a city filter?
///
/// The filters that appear in the shipped rulesets are few and blunt — "in all
/// cities", "in this city", "in City-State cities". Anything else is
/// [`Verdict::Unknown`] rather than a guess, so a filter this does not
/// understand withholds its bonus and stays countable.
///
/// `subject` is the city being asked about; `origin` is the city the effect
/// comes from, which is what "this city" means.
pub fn city_matches(
    world: &crate::world::world::World,
    r: &Ruleset,
    subject: &crate::world::city::City,
    origin: Option<&crate::world::city::City>,
    filter: &str,
) -> Verdict {
    let yes = |b: bool| if b { Verdict::True } else { Verdict::False };
    let f = filter.trim();
    let f = f.strip_prefix("in ").unwrap_or(f);
    let _ = r;
    match f {
        "all cities" | "All" | "in all cities" => Verdict::True,
        // What a conquest made of it (`City.matchesFilter`).
        "puppeted cities" | "Puppeted" => yes(subject.puppet),
        "annexed cities" | "Annexed" => yes(subject.founder != subject.owner && !subject.puppet),
        "resisting cities" | "Resisting" => yes(subject.resistance > 0),
        "cities being razed" | "Razing" => yes(subject.razing),
        "this city" => match origin {
            // Two cities are the same city when they stand on the same tile —
            // names are not unique and ids are not in hand here.
            Some(o) => yes(o.centre == subject.centre),
            None => Verdict::Unknown,
        },
        "other cities" => match origin {
            Some(o) => yes(o.centre != subject.centre),
            None => Verdict::Unknown,
        },
        "your cities" | "cities" => match origin {
            Some(o) => yes(o.owner == subject.owner),
            None => Verdict::Unknown,
        },
        "foreign cities" | "enemy cities" => match origin {
            Some(o) => yes(o.owner != subject.owner),
            None => Verdict::Unknown,
        },
        // City-states are not modelled yet. Saying "no" would quietly hand out
        // a bonus that should be withheld and hide the gap.
        "City-State cities" | "City-State" => {
            let _ = world;
            Verdict::Unknown
        }
        _ => Verdict::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::grid::{CivId, TileId};

    fn ruleset() -> Ruleset {
        Ruleset::load_dir("../android/assets/jsons/Civ V - Gods & Kings").expect("readable").expect("loads")
    }

    fn unit(r: &Ruleset, name: &str) -> Unit {
        Unit::new(r.unit(name).unwrap_or_else(|| panic!("{name}")), CivId(0), TileId(0), 2)
    }

    #[test]
    fn categories_come_from_the_units_own_numbers() {
        let r = ruleset();
        let warrior = unit(&r, "Warrior");
        assert_eq!(unit_matches(&r, &warrior, "Melee"), Verdict::True);
        assert_eq!(unit_matches(&r, &warrior, "Ranged"), Verdict::False);
        assert_eq!(unit_matches(&r, &warrior, "Military"), Verdict::True);
        assert_eq!(unit_matches(&r, &warrior, "Civilian"), Verdict::False);
        assert_eq!(unit_matches(&r, &warrior, "Land"), Verdict::True);
        assert_eq!(unit_matches(&r, &warrior, "Water"), Verdict::False);
        assert_eq!(unit_matches(&r, &warrior, "all"), Verdict::True);

        let archer = unit(&r, "Archer");
        assert_eq!(unit_matches(&r, &archer, "Ranged"), Verdict::True);
        assert_eq!(unit_matches(&r, &archer, "Melee"), Verdict::False);
    }

    #[test]
    fn a_unit_matches_its_own_name_and_its_type() {
        let r = ruleset();
        let warrior = unit(&r, "Warrior");
        assert_eq!(unit_matches(&r, &warrior, "Warrior"), Verdict::True);
        assert_eq!(unit_matches(&r, &warrior, "Sword"), Verdict::True, "its unitType");
        assert_eq!(unit_matches(&r, &warrior, "Mounted"), Verdict::False);
    }

    #[test]
    fn a_conjunction_needs_every_term() {
        let r = ruleset();
        let warrior = unit(&r, "Warrior");
        assert_eq!(unit_matches(&r, &warrior, "{Military} {Land}"), Verdict::True);
        assert_eq!(unit_matches(&r, &warrior, "{Military} {Water}"), Verdict::False);
        assert_eq!(unit_matches(&r, &warrior, "{Civilian} {Land}"), Verdict::False);
    }

    #[test]
    fn negation_inverts_a_known_answer_only() {
        let r = ruleset();
        let warrior = unit(&r, "Warrior");
        assert_eq!(unit_matches(&r, &warrior, "non-[Water]"), Verdict::True);
        assert_eq!(unit_matches(&r, &warrior, "non-[Land]"), Verdict::False);
        assert_eq!(
            unit_matches(&r, &warrior, "non-[Barbarian]"),
            Verdict::Unknown,
            "negating something undecidable stays undecidable"
        );
    }

    #[test]
    fn stateful_terms_read_the_unit_not_the_type() {
        let r = ruleset();
        let mut warrior = unit(&r, "Warrior");
        assert_eq!(unit_matches(&r, &warrior, "Wounded"), Verdict::False);
        warrior.health = 40;
        assert_eq!(unit_matches(&r, &warrior, "Wounded"), Verdict::True);

        warrior.health = 100;
        assert_eq!(unit_matches(&r, &warrior, "Embarked"), Verdict::False);
        warrior.embarked = true;
        assert_eq!(unit_matches(&r, &warrior, "Embarked"), Verdict::True);
    }

    #[test]
    fn an_unknown_term_poisons_a_conjunction_but_a_false_one_settles_it() {
        let r = ruleset();
        let warrior = unit(&r, "Warrior");
        // Land is true, Barbarian is undecidable — so the pair is undecidable.
        assert_eq!(unit_matches(&r, &warrior, "{Land} {Barbarian}"), Verdict::Unknown);
        // Water is definitely false, so nothing else needs deciding.
        assert_eq!(unit_matches(&r, &warrior, "{Water} {Barbarian}"), Verdict::False);
    }

    /// Every filter the shipped ruleset actually uses should be decidable, or
    /// the uniques carrying them do nothing.
    #[test]
    fn the_filters_the_ruleset_uses_are_understood() {
        let r = ruleset();
        let warrior = unit(&r, "Warrior");
        let mut undecidable = Vec::new();
        for filter in [
            "Mounted",
            "Water",
            "Armored",
            "Helicopter",
            "Bomber",
            "Land",
            "Wounded",
            "Ranged",
            "Air",
            "Submarine",
            "All",
            "Military",
            "Melee",
            "{Military} {Water}",
            "Great General",
            "Great Admiral",
            "Civilian",
            "non-air",
        ] {
            if unit_matches(&r, &warrior, filter) == Verdict::Unknown {
                undecidable.push(filter);
            }
        }
        println!("undecidable: {undecidable:?}");
        assert!(undecidable.is_empty(), "these filters do nothing: {undecidable:?}");
    }
}
