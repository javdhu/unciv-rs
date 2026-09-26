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
//!
//! The language is parsed once, into a [`Filter`] of [`Term`]s
//! ([`crate::params`]): a unique's parameters are compiled at ruleset load, and
//! the functions here that take a `&str` compile it on the spot. Each term
//! arrives already classified — [`UnitWord`], [`BuildingWord`], [`CityWord`] —
//! and with its names resolved, so matching reads the object and the world and
//! nothing else.

use crate::arena::Id;
use crate::params::{Filter, Term};
use crate::ruleset::{BaseUnit, Ruleset};
use crate::world::conditionals::Verdict;
use crate::world::entities::Unit;

/// Split `{A} {B}` into its parts, or `None` if this is not a conjunction.
pub(crate) fn conjunction(filter: &str) -> Option<Vec<&str>> {
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
/// decided. [`Filter::test`] is the same reading, compiled.
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

fn yes(b: bool) -> Verdict {
    if b {
        Verdict::True
    } else {
        Verdict::False
    }
}

/// The words the unit filters know.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnitWord {
    All,
    Ranged,
    Melee,
    Military,
    Civilian,
    Land,
    Water,
    Air,
    GreatPerson,
    NonAir,
    Wounded,
    Embarked,
    Barbarian,
    CityState,
    CityStates,
    Other,
}

impl UnitWord {
    pub(crate) fn of(term: &str) -> UnitWord {
        match term {
            "all" | "All" => UnitWord::All,
            "Ranged" => UnitWord::Ranged,
            "Melee" => UnitWord::Melee,
            "Military" => UnitWord::Military,
            "Civilian" => UnitWord::Civilian,
            "Land" => UnitWord::Land,
            "Water" => UnitWord::Water,
            "Air" => UnitWord::Air,
            "Great Person" | "Luminary" => UnitWord::GreatPerson,
            "non-air" => UnitWord::NonAir,
            "Wounded" | "wounded" | "wounded units" => UnitWord::Wounded,
            "Embarked" | "embarked" => UnitWord::Embarked,
            "Barbarian" | "barbarians" => UnitWord::Barbarian,
            "City-State" => UnitWord::CityState,
            "City-States" => UnitWord::CityStates,
            _ => UnitWord::Other,
        }
    }
}

/// Categories derived from a unit's own numbers, as Unciv derives them
/// (`BaseUnit.kt:500`).
fn category(r: &Ruleset, base: &BaseUnit, term: &Term) -> Verdict {
    let ranged = base.ranged_strength.unwrap_or(0) > 0;
    let melee = !ranged && base.strength.unwrap_or(0) > 0;
    let military = ranged || melee;
    let unit_type = || r.unit_types.by_name(&base.unit_type);
    let movement = || unit_type().map(|t| t.movement_type.as_str());

    if let Some(era) = term.pre_era {
        return era.and_then(|id| r.eras.get(id)).map_or(Verdict::Unknown, |era| {
            yes(base
                .required_tech
                .as_deref()
                .and_then(|n| r.technologies.by_name(n))
                .and_then(|t| t.era(r))
                .and_then(|id| r.eras.get(id))
                .map_or(0, |e| e.era_number)
                < era.era_number)
        });
    }
    match term.unit {
        UnitWord::All => Verdict::True,
        UnitWord::Ranged => yes(ranged),
        UnitWord::Melee => yes(melee),
        UnitWord::Military => yes(military),
        UnitWord::Civilian => yes(!military),
        UnitWord::Land => movement().map_or(Verdict::Unknown, |m| yes(m == "Land")),
        UnitWord::Water => movement().map_or(Verdict::Unknown, |m| yes(m == "Water")),
        UnitWord::Air => movement().map_or(Verdict::Unknown, |m| yes(m == "Air")),
        UnitWord::GreatPerson => {
            yes(base.uniques.iter().any(|u| u.unique_type() == Some(crate::UniqueType::GreatPerson)))
        }
        UnitWord::NonAir => movement().map_or(Verdict::Unknown, |m| yes(m != "Air")),
        _ => {
            let text = &*term.text;
            if base
                .uniques
                .iter()
                .chain(unit_type().into_iter().flat_map(|t| &t.uniques))
                .any(|u| u.kind == crate::unique::UniqueKind::Tag && u.text == text)
            {
                return Verdict::True;
            }
            if text == base.name || text == base.unit_type {
                return Verdict::True;
            }
            if base.replaces.as_deref() == Some(text) {
                return Verdict::True;
            }
            // A term naming a unit type or unit this ruleset has is a definite
            // no; anything else is beyond this function.
            if term.names.unit_type.is_some() || term.names.unit.is_some() {
                Verdict::False
            } else {
                Verdict::Unknown
            }
        }
    }
}

/// A unit's own state, then its type.
fn unit_term(r: &Ruleset, base: &BaseUnit, unit: &Unit, term: &Term) -> Verdict {
    match term.unit {
        // Stateful terms, which a bare BaseUnit cannot answer.
        UnitWord::Wounded => yes(unit.is_hurt()),
        UnitWord::Embarked => yes(unit.embarked),
        UnitWord::Barbarian | UnitWord::CityState => Verdict::Unknown, // needs civ kind
        _ => category(r, base, term),
    }
}

/// Does a unit match a filter, given its state as well as its type?
pub fn unit_matches(r: &Ruleset, unit: &Unit, filter: &str) -> Verdict {
    matches_unit(r, unit, &Filter::compile(r, filter))
}

/// [`unit_matches`], compiled.
pub fn matches_unit(r: &Ruleset, unit: &Unit, filter: &Filter) -> Verdict {
    let Some(base) = r.units.get(unit.base) else { return Verdict::Unknown };
    filter.test(&|term| unit_term(r, base, unit, term))
}

/// Live unit filters can also identify the owner's kind and granted promotions.
pub fn unit_matches_in(world: &crate::world::World, r: &Ruleset, unit: &Unit, filter: &str) -> Verdict {
    matches_unit_in(world, r, unit, &Filter::compile(r, filter))
}

/// [`unit_matches_in`], compiled.
pub fn matches_unit_in(world: &crate::world::World, r: &Ruleset, unit: &Unit, filter: &Filter) -> Verdict {
    let base = r.units.get(unit.base);
    filter.test(&|term| match term.unit {
        UnitWord::Barbarian => yes(world.civ(unit.owner).is_some_and(|c| c.is_barbarian)),
        UnitWord::CityState | UnitWord::CityStates => yes(world.civ(unit.owner).is_some_and(|c| c.is_city_state())),
        _ if term.names.promotion.is_some_and(|id| unit.promotions.contains(&id)) => Verdict::True,
        _ => base.map_or(Verdict::Unknown, |base| unit_term(r, base, unit, term)),
    })
}

/// The type-only form, for questions about a unit that does not exist yet —
/// what a city may build, what a promotion could apply to.
pub fn base_unit_matches(r: &Ruleset, base: Id<BaseUnit>, filter: &str) -> Verdict {
    matches_base_unit(r, base, &Filter::compile(r, filter))
}

/// [`base_unit_matches`], compiled.
pub fn matches_base_unit(r: &Ruleset, base: Id<BaseUnit>, filter: &Filter) -> Verdict {
    let Some(b) = r.units.get(base) else { return Verdict::Unknown };
    filter.test(&|term| category(r, b, term))
}

/// The words the building filter knows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuildingWord {
    All,
    Wonder,
    WorldWonder,
    NationalWonder,
    Food,
    Production,
    Gold,
    Science,
    Culture,
    Happiness,
    Faith,
    Other,
}

impl BuildingWord {
    pub(crate) fn of(term: &str) -> BuildingWord {
        match term {
            "All" | "all" | "Building" | "Buildings" => BuildingWord::All,
            "Wonder" | "Wonders" => BuildingWord::Wonder,
            "World Wonder" | "World Wonders" => BuildingWord::WorldWonder,
            "National Wonder" | "National Wonders" => BuildingWord::NationalWonder,
            "Food" => BuildingWord::Food,
            "Production" => BuildingWord::Production,
            "Gold" => BuildingWord::Gold,
            "Science" => BuildingWord::Science,
            "Culture" => BuildingWord::Culture,
            "Happiness" => BuildingWord::Happiness,
            "Faith" => BuildingWord::Faith,
            _ => BuildingWord::Other,
        }
    }
}

/// Does a building type match a building filter?
///
/// Names, "All", and the wonder categories — the terms the purchase uniques in
/// the shipped rulesets use. A term that is none of these, and not some other
/// building's name either, is [`Verdict::Unknown`]: a stat category like
/// "Culture" needs knowing what the building yields, which this does not ask.
pub fn building_matches(r: &Ruleset, id: Id<crate::ruleset::Building>, filter: &str) -> Verdict {
    matches_building(r, id, &Filter::compile(r, filter))
}

/// [`building_matches`], compiled.
pub fn matches_building(r: &Ruleset, id: Id<crate::ruleset::Building>, filter: &Filter) -> Verdict {
    let Some(b) = r.buildings.get(id) else { return Verdict::Unknown };
    let world_wonder = b.is_wonder == Some(true);
    let national_wonder = b.is_national_wonder == Some(true);
    filter.test(&|term| match term.building {
        BuildingWord::All => Verdict::True,
        BuildingWord::Wonder => yes(world_wonder || national_wonder),
        BuildingWord::WorldWonder => yes(world_wonder),
        BuildingWord::NationalWonder => yes(national_wonder),
        BuildingWord::Food => yes(b.food.unwrap_or(0) > 0),
        BuildingWord::Production => yes(b.production.unwrap_or(0) > 0),
        BuildingWord::Gold => yes(b.gold.unwrap_or(0) > 0),
        BuildingWord::Science => yes(b.science.unwrap_or(0) > 0),
        BuildingWord::Culture => yes(b.culture.unwrap_or(0) > 0),
        BuildingWord::Happiness => yes(b.happiness.unwrap_or(0) > 0),
        BuildingWord::Faith => yes(b.faith.unwrap_or(0) > 0),
        BuildingWord::Other if term.names.era.is_some() => yes(b
            .required_tech
            .as_deref()
            .and_then(|t| r.technologies.by_name(t))
            .and_then(|t| t.era(r))
            .and_then(|id| r.eras.get(id))
            .is_some_and(|e| *e.name == *term.text)),
        BuildingWord::Other if *term.text == b.name || b.replaces.as_deref() == Some(&*term.text) => Verdict::True,
        BuildingWord::Other if term.names.building.is_some() => Verdict::False,
        BuildingWord::Other => Verdict::Unknown,
    })
}

/// The words the city filter knows, after its `in `/`all ` prefixes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CityWord {
    All,
    Coastal,
    Capital,
    NonOccupied,
    Garrisoned,
    WorldWonder,
    Holy,
    Connected,
    MajorReligion,
    Puppeted,
    Annexed,
    Resisting,
    Razing,
    This,
    Other,
    Yours,
    Foreign,
    CityState,
    Unknown,
}

impl CityWord {
    pub(crate) fn of(filter: &str) -> CityWord {
        let f = filter.trim();
        let f = f.strip_prefix("in ").unwrap_or(f);
        let f = if f.starts_with("all cities ") || f == "all coastal cities" || f == "all non-occupied cities" {
            f.strip_prefix("all ").unwrap_or(f)
        } else {
            f
        };
        match f {
            "all cities" | "All" | "in all cities" | "cities following this religion" => CityWord::All,
            "coastal cities" | "Coastal" => CityWord::Coastal,
            "capital" => CityWord::Capital,
            "non-occupied cities" => CityWord::NonOccupied,
            "cities with a garrison" => CityWord::Garrisoned,
            "cities with a world wonder" => CityWord::WorldWonder,
            "Holy cities" | "Holy" => CityWord::Holy,
            "cities connected to the capital" | "cities connected to capital" => CityWord::Connected,
            "cities in which the majority religion is a major religion" => CityWord::MajorReligion,
            "puppeted cities" | "Puppeted" => CityWord::Puppeted,
            "annexed cities" | "Annexed" => CityWord::Annexed,
            "resisting cities" | "Resisting" => CityWord::Resisting,
            "cities being razed" | "Razing" => CityWord::Razing,
            "this city" => CityWord::This,
            "other cities" => CityWord::Other,
            "your cities" | "cities" => CityWord::Yours,
            "foreign cities" | "enemy cities" => CityWord::Foreign,
            "City-State cities" | "City-State" => CityWord::CityState,
            _ => CityWord::Unknown,
        }
    }
}

/// Does a city match a city filter?
///
/// The filters that appear in the shipped rulesets are few and blunt — "in all
/// cities", "in this city", "in City-State cities". Anything else is
/// [`Verdict::Unknown`] rather than a guess, so a filter this does not
/// understand withholds its bonus and stays countable.
///
/// `subject` is the city being asked about; `origin` is the city the effect
/// comes from, which is what "this city" means. One term: callers wanting the
/// full language use [`matches_city`].
pub fn city_matches(
    world: &crate::world::world::World,
    r: &Ruleset,
    subject: &crate::world::city::City,
    origin: Option<&crate::world::city::City>,
    filter: &str,
) -> Verdict {
    city_word(world, r, subject, origin, CityWord::of(filter))
}

/// A whole city filter, compiled: [`city_matches`] for each term.
pub fn matches_city(
    world: &crate::world::world::World,
    r: &Ruleset,
    subject: &crate::world::city::City,
    origin: Option<&crate::world::city::City>,
    filter: &Filter,
) -> Verdict {
    filter.test(&|term| city_word(world, r, subject, origin, term.city))
}

/// [`city_matches`] for an already classified term.
pub fn city_word(
    world: &crate::world::world::World,
    r: &Ruleset,
    subject: &crate::world::city::City,
    origin: Option<&crate::world::city::City>,
    word: CityWord,
) -> Verdict {
    match word {
        CityWord::All => Verdict::True,
        CityWord::Coastal => yes(subject.coastal),
        CityWord::Capital => yes(world
            .civ(subject.owner)
            .and_then(|c| c.cities.first())
            .and_then(|id| world.cities.get(*id))
            .is_some_and(|c| c.centre == subject.centre)),
        CityWord::NonOccupied => yes(subject.founder == subject.owner
            || subject.puppet
            || subject.uniques.has(crate::UniqueType::RemovesAnnexUnhappiness)),
        CityWord::Garrisoned => yes(world
            .units
            .iter()
            .any(|(_, u)| u.owner == subject.owner && u.position == subject.centre && !u.is_civilian(r))),
        CityWord::WorldWonder => {
            yes(subject.built.iter().any(|id| r.buildings.get(*id).is_some_and(|b| b.is_wonder == Some(true))))
        }
        CityWord::Holy => yes(subject.faith.holy_city_of.is_some()),
        CityWord::Connected => yes(world
            .cities
            .iter()
            .find(|(_, c)| c.centre == subject.centre)
            .is_some_and(|(id, _)| crate::world::connections::find(world, r, subject.owner).has_trade_route(id))),
        CityWord::MajorReligion => yes(subject
            .faith
            .majority(subject.population)
            .is_some_and(|creed| crate::world::religion::religion_exists(world, creed))),
        // What a conquest made of it (`City.matchesFilter`).
        CityWord::Puppeted => yes(subject.puppet),
        CityWord::Annexed => yes(subject.founder != subject.owner && !subject.puppet),
        CityWord::Resisting => yes(subject.resistance > 0),
        CityWord::Razing => yes(subject.razing),
        CityWord::This => match origin {
            // Two cities are the same city when they stand on the same tile —
            // names are not unique and ids are not in hand here.
            Some(o) => yes(o.centre == subject.centre),
            None => Verdict::Unknown,
        },
        CityWord::Other => match origin {
            Some(o) => yes(o.centre != subject.centre),
            None => Verdict::Unknown,
        },
        CityWord::Yours => match origin {
            Some(o) => yes(o.owner == subject.owner),
            None => Verdict::Unknown,
        },
        CityWord::Foreign => match origin {
            Some(o) => yes(o.owner != subject.owner),
            None => Verdict::Unknown,
        },
        // City-states are not modelled yet. Saying "no" would quietly hand out
        // a bonus that should be withheld and hide the gap.
        CityWord::CityState => yes(world.civ(subject.owner).is_some_and(|c| c.is_city_state())),
        CityWord::Unknown => Verdict::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::grid::{CivId, TileId};

    fn ruleset() -> Ruleset {
        Ruleset::load_dir("rules/gods-and-kings").expect("readable").expect("loads")
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
