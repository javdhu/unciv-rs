//! Compiled rule parameters answer exactly as the string code they replaced.
//!
//! `civ_core::params` reads every unique parameter once — numbers, yields,
//! filters split into terms with their names resolved — and the matchers in
//! `world::filters` and `world::conditionals` read the compiled form. The old
//! matchers parsed the text on every call; they are kept below, verbatim but for
//! paths, as the oracle. Every parameter string in the bundled rulesets (base
//! and blitz) is compiled and matched against the objects of a game the AI has
//! played for a while: every building and unit type, every unit, city and tile.

use civ_core::arena::Id;
use civ_core::params::Filter;
use civ_core::ruleset::{BaseUnit, Building, Ruleset};
use civ_core::stats::{Fixed, Stat, Stats};
use civ_core::world::conditionals::{matches_tile, matches_tile_in, GameContext, Verdict};
use civ_core::world::filters::{
    city_matches, matches_base_unit, matches_building, matches_city, matches_unit, matches_unit_in,
};
use civ_core::world::grid::{CivId, TileId};
use civ_core::world::resources::matches_resource;
use civ_core::world::{finish_round, CityFocus, TurnModifiers, World};
use civ_core::Unique;
use civ_sim::Settings;
use std::collections::BTreeSet;

/// The string matchers as they were before compilation.
mod legacy {
    use civ_core::arena::Id;
    use civ_core::ruleset::{BaseUnit, Ruleset, Technology};
    use civ_core::world::conditionals::{GameContext, Verdict};
    use civ_core::world::entities::Unit;
    use civ_core::world::grid::{CivId, TileId};
    use civ_core::UniqueType;

    /// `Technology::era` before the derived column table.
    fn era(t: &Technology, r: &Ruleset) -> Option<Id<civ_core::ruleset::Era>> {
        r.eras.id(t.era_name(r)?)
    }

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

    fn category(r: &Ruleset, base: &BaseUnit, term: &str) -> Verdict {
        let ranged = base.ranged_strength.unwrap_or(0) > 0;
        let melee = !ranged && base.strength.unwrap_or(0) > 0;
        let military = ranged || melee;
        let movement = r.unit_types.by_name(&base.unit_type).map(|t| t.movement_type.as_str());
        let yes = |b: bool| if b { Verdict::True } else { Verdict::False };
        if let Some(era_name) = term.strip_prefix("pre-[").and_then(|s| s.strip_suffix(']')) {
            return r.eras.by_name(era_name).map_or(Verdict::Unknown, |e| {
                yes(base
                    .required_tech
                    .as_deref()
                    .and_then(|n| r.technologies.by_name(n))
                    .and_then(|t| era(t, r))
                    .and_then(|id| r.eras.get(id))
                    .map_or(0, |e| e.era_number)
                    < e.era_number)
            });
        }
        match term {
            "all" | "All" => Verdict::True,
            "Ranged" => yes(ranged),
            "Melee" => yes(melee),
            "Military" => yes(military),
            "Civilian" => yes(!military),
            "Land" => movement.map_or(Verdict::Unknown, |m| yes(m == "Land")),
            "Water" => movement.map_or(Verdict::Unknown, |m| yes(m == "Water")),
            "Air" => movement.map_or(Verdict::Unknown, |m| yes(m == "Air")),
            "Great Person" | "Luminary" => {
                yes(base.uniques.iter().any(|u| u.unique_type() == Some(UniqueType::GreatPerson)))
            }
            "non-air" => movement.map_or(Verdict::Unknown, |m| yes(m != "Air")),
            _ => {
                if base
                    .uniques
                    .iter()
                    .chain(r.unit_types.by_name(&base.unit_type).into_iter().flat_map(|t| &t.uniques))
                    .any(|u| u.kind == civ_core::UniqueKind::Tag && u.text == term)
                {
                    return Verdict::True;
                }
                if term == base.name || term == base.unit_type {
                    return Verdict::True;
                }
                if base.replaces.as_deref() == Some(term) {
                    return Verdict::True;
                }
                if r.unit_types.id(term).is_some() || r.units.id(term).is_some() {
                    Verdict::False
                } else {
                    Verdict::Unknown
                }
            }
        }
    }

    pub fn unit_matches(r: &Ruleset, unit: &Unit, filter: &str) -> Verdict {
        let Some(base) = r.units.get(unit.base) else { return Verdict::Unknown };
        multi_filter(filter, &|term| match term {
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
            "Barbarian" | "barbarians" | "City-State" => Verdict::Unknown,
            _ => category(r, base, term),
        })
    }

    pub fn unit_matches_in(world: &civ_core::world::World, r: &Ruleset, unit: &Unit, filter: &str) -> Verdict {
        let yes = |value| if value { Verdict::True } else { Verdict::False };
        multi_filter(filter, &|term| match term {
            "Barbarian" | "barbarians" => yes(world.civ(unit.owner).is_some_and(|c| c.is_barbarian)),
            "City-State" | "City-States" => yes(world.civ(unit.owner).is_some_and(|c| c.is_city_state())),
            _ if r.promotions.id(term).is_some_and(|id| unit.promotions.contains(&id)) => Verdict::True,
            _ => unit_matches(r, unit, term),
        })
    }

    pub fn base_unit_matches(r: &Ruleset, base: Id<BaseUnit>, filter: &str) -> Verdict {
        let Some(b) = r.units.get(base) else { return Verdict::Unknown };
        multi_filter(filter, &|term| category(r, b, term))
    }

    pub fn building_matches(r: &Ruleset, id: Id<civ_core::ruleset::Building>, filter: &str) -> Verdict {
        let Some(b) = r.buildings.get(id) else { return Verdict::Unknown };
        let yes = |v: bool| if v { Verdict::True } else { Verdict::False };
        let world_wonder = b.is_wonder == Some(true);
        let national_wonder = b.is_national_wonder == Some(true);
        multi_filter(filter, &|term| match term {
            "All" | "all" | "Building" | "Buildings" => Verdict::True,
            "Wonder" | "Wonders" => yes(world_wonder || national_wonder),
            "World Wonder" | "World Wonders" => yes(world_wonder),
            "National Wonder" | "National Wonders" => yes(national_wonder),
            "Food" => yes(b.food.unwrap_or(0) > 0),
            "Production" => yes(b.production.unwrap_or(0) > 0),
            "Gold" => yes(b.gold.unwrap_or(0) > 0),
            "Science" => yes(b.science.unwrap_or(0) > 0),
            "Culture" => yes(b.culture.unwrap_or(0) > 0),
            "Happiness" => yes(b.happiness.unwrap_or(0) > 0),
            "Faith" => yes(b.faith.unwrap_or(0) > 0),
            t if r.eras.by_name(t).is_some() => yes(b
                .required_tech
                .as_deref()
                .and_then(|t| r.technologies.by_name(t))
                .and_then(|t| era(t, r))
                .and_then(|id| r.eras.get(id))
                .is_some_and(|e| e.name == t)),
            t if t == b.name || b.replaces.as_deref() == Some(t) => Verdict::True,
            t if r.buildings.id(t).is_some() => Verdict::False,
            _ => Verdict::Unknown,
        })
    }

    pub fn city_matches(
        world: &civ_core::world::World,
        r: &Ruleset,
        subject: &civ_core::world::city::City,
        origin: Option<&civ_core::world::city::City>,
        filter: &str,
    ) -> Verdict {
        let yes = |b: bool| if b { Verdict::True } else { Verdict::False };
        let f = filter.trim();
        let f = f.strip_prefix("in ").unwrap_or(f);
        let f = if f.starts_with("all cities ") || f == "all coastal cities" || f == "all non-occupied cities" {
            f.strip_prefix("all ").unwrap_or(f)
        } else {
            f
        };
        match f {
            "all cities" | "All" | "in all cities" | "cities following this religion" => Verdict::True,
            "coastal cities" | "Coastal" => yes(subject.coastal),
            "capital" => yes(world
                .civ(subject.owner)
                .and_then(|c| c.cities.first())
                .and_then(|id| world.cities.get(*id))
                .is_some_and(|c| c.centre == subject.centre)),
            "non-occupied cities" => yes(subject.founder == subject.owner
                || subject.puppet
                || subject.uniques.has(UniqueType::RemovesAnnexUnhappiness)),
            "cities with a garrison" => yes(world
                .units
                .iter()
                .any(|(_, u)| u.owner == subject.owner && u.position == subject.centre && !u.is_civilian(r))),
            "cities with a world wonder" => {
                yes(subject.built.iter().any(|id| r.buildings.get(*id).is_some_and(|b| b.is_wonder == Some(true))))
            }
            "Holy cities" | "Holy" => yes(subject.faith.holy_city_of.is_some()),
            "cities connected to the capital" | "cities connected to capital" => {
                yes(world.cities.iter().find(|(_, c)| c.centre == subject.centre).is_some_and(|(id, _)| {
                    civ_core::world::connections::find(world, r, subject.owner).has_trade_route(id)
                }))
            }
            "cities in which the majority religion is a major religion" => yes(subject
                .faith
                .majority(subject.population)
                .is_some_and(|creed| civ_core::world::religion::religion_exists(world, creed))),
            "puppeted cities" | "Puppeted" => yes(subject.puppet),
            "annexed cities" | "Annexed" => yes(subject.founder != subject.owner && !subject.puppet),
            "resisting cities" | "Resisting" => yes(subject.resistance > 0),
            "cities being razed" | "Razing" => yes(subject.razing),
            "this city" => match origin {
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
            "City-State cities" | "City-State" => yes(world.civ(subject.owner).is_some_and(|c| c.is_city_state())),
            _ => Verdict::Unknown,
        }
    }

    pub fn resource_matches(r: &Ruleset, id: Id<civ_core::ruleset::TileResource>, filter: &str) -> bool {
        let Some(res) = r.resources.get(id) else { return false };
        multi_filter(filter, &|f| {
            if f == "All"
                || f == "all"
                || f == res.name
                || f == res.resource_type
                || res.uniques.iter().any(|u| u.text == f)
            {
                Verdict::True
            } else {
                Verdict::False
            }
        }) == Verdict::True
    }

    pub fn tile_matches(ctx: &GameContext, tile: TileId, filter: &str) -> Verdict {
        let Some(world) = ctx.world else { return Verdict::Unknown };
        multi_filter(filter, &|term| {
            if let Some(resource) = ctx.ruleset.resources.id(term) {
                if let Some(civ) = ctx.civ.and_then(|id| world.civ(id)) {
                    return if world.grid.resource[tile.0 as usize] == Some(resource)
                        && civ_core::world::resources::is_visible(ctx.ruleset, civ, resource)
                    {
                        Verdict::True
                    } else {
                        Verdict::False
                    };
                }
            }
            let resource_word = matches!(term, "resource" | "Water resource")
                || term
                    .strip_suffix(" resource")
                    .is_some_and(|k| ctx.ruleset.resources.iter().any(|(_, x)| x.resource_type == k));
            if resource_word {
                if let (Some(civ), Some(resource)) =
                    (ctx.civ.and_then(|id| world.civ(id)), world.grid.resource[tile.0 as usize])
                {
                    if !civ_core::world::resources::is_visible(ctx.ruleset, civ, resource) {
                        return Verdict::False;
                    }
                }
            }
            if matches!(term, "Enemy Land" | "Enemy") {
                let Some(civ) = ctx.civ else { return Verdict::Unknown };
                let enemy = world.grid.owner[tile.0 as usize].is_some_and(|owner| world.diplomacy.at_war(civ, owner));
                return if enemy { Verdict::True } else { Verdict::False };
            }
            if term == "City center" {
                return if world.cities.iter().any(|(_, city)| city.centre == tile) {
                    Verdict::True
                } else {
                    Verdict::False
                };
            }
            if matches!(term, "Friendly Land" | "Friendly" | "Foreign Land" | "Foreign") {
                let Some(civ) = ctx.civ else { return Verdict::Unknown };
                let friendly = civ_core::world::movement::is_friendly_territory(world, civ, tile);
                let matches = if matches!(term, "Friendly Land" | "Friendly") { friendly } else { !friendly };
                return if matches { Verdict::True } else { Verdict::False };
            }
            tile_matches_in(&world.grid, ctx.ruleset, tile, term, ctx.civ)
        })
    }

    pub fn tile_matches_in(
        grid: &civ_core::world::grid::TileGrid,
        ruleset: &Ruleset,
        tile: TileId,
        filter: &str,
        civ: Option<CivId>,
    ) -> Verdict {
        multi_filter(filter, &|term| tile_matches_term(grid, ruleset, tile, term, civ))
    }

    fn tile_matches_term(
        grid: &civ_core::world::grid::TileGrid,
        r: &Ruleset,
        tile: TileId,
        filter: &str,
        civ: Option<CivId>,
    ) -> Verdict {
        let terrains: Vec<&civ_core::ruleset::Terrain> =
            grid.terrains(tile).filter_map(|id| r.terrains.get(id)).collect();
        let yes = |b: bool| if b { Verdict::True } else { Verdict::False };
        if terrains.iter().any(|t| t.name == filter) {
            return Verdict::True;
        }
        let rough = |t: &civ_core::ruleset::Terrain| {
            t.uniques.iter().any(|u| u.unique_type() == Some(UniqueType::RoughTerrain))
        };
        match filter {
            "Coastal" => {
                return yes(!grid.is_water(r, tile)
                    && grid.neighbours(tile).any(|next| {
                        r.terrains.get(grid.terrain[next.0 as usize]).is_some_and(|terrain| terrain.name == "Coast")
                    }))
            }
            "your" => return civ.map_or(Verdict::Unknown, |owner| yes(grid.owner[tile.0 as usize] == Some(owner))),
            "River" => return yes(grid.has_river(tile)),
            "Fresh water" | "Fresh Water" => return yes(grid.is_fresh_water(r, tile)),
            "non-fresh water" => return yes(!grid.is_fresh_water(r, tile)),
            "resource" => return yes(grid.resource[tile.0 as usize].is_some()),
            "Water resource" => {
                return yes(grid.resource[tile.0 as usize].is_some() && terrains.iter().any(|t| t.r#type == "Water"))
            }
            "All" | "all" => return Verdict::True,
            "Land" => return yes(terrains.iter().any(|t| t.r#type == "Land")),
            "Water" => return yes(terrains.iter().any(|t| t.r#type == "Water")),
            "Natural Wonder" => return yes(grid.natural_wonder[tile.0 as usize].is_some()),
            "Isthmus" => return yes(grid.is_isthmus(r, tile)),
            "unimproved" => return yes(grid.unpillaged_improvement(tile).is_none()),
            "Featureless" => return yes(grid.features[tile.0 as usize].is_empty()),
            "Rough terrain" => return yes(terrains.iter().copied().any(rough)),
            "Open terrain" => return yes(terrains.iter().copied().all(|t| !rough(t))),
            "Friendly Land" | "Foreign Land" => {
                let (Some(civ), owner) = (civ, grid.owner[tile.0 as usize]) else {
                    return Verdict::Unknown;
                };
                let friendly = owner == Some(civ);
                return yes(if filter == "Friendly Land" { friendly } else { !friendly });
            }
            _ => {}
        }
        if let Some(id) = r.resources.id(filter) {
            return yes(grid.resource[tile.0 as usize] == Some(id));
        }
        if let Some(kind) = filter.strip_suffix(" resource") {
            if r.resources.iter().any(|(_, x)| x.resource_type == kind) {
                let here = grid.resource[tile.0 as usize].and_then(|id| r.resources.get(id));
                return yes(here.is_some_and(|x| x.resource_type == kind));
            }
        }
        if let Some(id) = r.buildings.id(filter) {
            return yes(grid.placed_at(tile).is_some_and(|p| p.building == id));
        }
        if let Some(id) = r.improvements.id(filter) {
            return yes(grid.unpillaged_improvement(tile) == Some(id));
        }
        if r.terrains.id(filter).is_some() {
            Verdict::False
        } else {
            Verdict::Unknown
        }
    }
}

/// Every unique the ruleset holds, conditionals included.
fn every_unique(r: &Ruleset) -> Vec<Unique> {
    fn walk(u: &Unique, out: &mut Vec<Unique>) {
        out.push(u.clone());
        for c in &u.conditionals {
            walk(c, out);
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
    let mut out = Vec::new();
    for u in all {
        walk(u, &mut out);
    }
    // City-state bonuses are kept as text and parsed when granted.
    for text in r.city_state_types.values().flat_map(|t| t.friend_bonus_uniques.iter().chain(&t.ally_bonus_uniques)) {
        walk(&Unique::parse(text), &mut out);
    }
    out
}

/// Every parameter string, plus the filters code writes as constants and a
/// few shapes the language allows that the rulesets happen not to use.
fn filter_strings(uniques: &[Unique]) -> Vec<String> {
    let mut all: BTreeSet<String> = uniques.iter().flat_map(|u| u.params.iter().cloned()).collect();
    for extra in [
        "Rough terrain",
        "Hill",
        "All",
        " non-[Land] ",
        "non-[{Military} {Water}]",
        "{Military} {non-[Water]}",
        "{Land} {Barbarian}",
        "{",
        "non-[",
        "{}",
        "pre-[Medieval era]",
        "pre-[Age of Tin]",
        "in all cities",
        "in all coastal cities",
        "all cities connected to capital",
        "in this city",
        "Bonus resource",
        "Luxury resource",
        "Water resource",
        "City-States",
        "wounded units",
        "Friendly",
        "Enemy",
        "City center",
        "your",
        "Isthmus",
    ] {
        all.insert(extra.to_string());
    }
    all.into_iter().collect()
}

fn play(ruleset: &str, turns: u32) -> (World, Ruleset) {
    let rules = civ_sim::content::load_ruleset(ruleset).unwrap();
    let settings = Settings {
        seed: 11,
        width: 33,
        height: 21,
        majors: 4,
        city_states: 2,
        ruleset: Some(ruleset.into()),
        ..Settings::default()
    };
    let (mut world, r, _) = civ_sim::new_game_in(&settings, &[], rules).unwrap();
    for _ in 0..turns {
        let mods = TurnModifiers::for_world(&world, &r);
        finish_round(&mut world, &r, CityFocus::default(), mods);
    }
    (world, r)
}

fn check_params(r: &Ruleset, uniques: &[Unique]) {
    // A deterministic three-way test, so the two readings of the filter
    // language are compared on structure rather than on any one matcher.
    let arbitrary = |salt: u64, text: &str| {
        let h = text
            .bytes()
            .fold(salt.wrapping_mul(0x9E37_79B9_7F4A_7C15), |h, b| (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01B3));
        [Verdict::True, Verdict::False, Verdict::Unknown][(h % 3) as usize]
    };
    for u in uniques {
        let c = u.compiled(r);
        for (i, p) in u.params.iter().enumerate() {
            assert_eq!(c.i32(i), p.parse::<i32>().ok(), "{p:?}");
            assert_eq!(c.i64(i), p.parse::<i64>().ok(), "{p:?}");
            assert_eq!(c.u32(i), p.parse::<u32>().ok(), "{p:?}");
            assert_eq!(c.usize(i), p.parse::<usize>().ok(), "{p:?}");
            assert_eq!(c.number(i), p.trim().parse::<i32>().ok(), "{p:?}");
            assert_eq!(c.fixed(i), Fixed::parse(p), "{p:?}");
            assert_eq!(c.stats(i), Stats::parse(p), "{p:?}");
            assert_eq!(c.stat(i), Stat::parse(p), "{p:?}");
            let term = c.term(i).unwrap();
            assert_eq!(&*term.text, p.as_str());
            let names = &term.names;
            assert_eq!(names.building, r.buildings.id(p), "{p:?}");
            assert_eq!(names.unit, r.units.id(p), "{p:?}");
            assert_eq!(names.unit_type, r.unit_types.id(p), "{p:?}");
            assert_eq!(names.promotion, r.promotions.id(p), "{p:?}");
            assert_eq!(names.terrain, r.terrains.id(p), "{p:?}");
            assert_eq!(names.resource, r.resources.id(p), "{p:?}");
            assert_eq!(names.improvement, r.improvements.id(p), "{p:?}");
            assert_eq!(names.tech, r.technologies.id(p), "{p:?}");
            assert_eq!(names.policy, r.policies.id(p), "{p:?}");
            assert_eq!(names.belief, r.beliefs.id(p), "{p:?}");
            assert_eq!(names.era, r.eras.id(p), "{p:?}");
            let filter = c.filter(i).unwrap();
            for salt in 0..4 {
                assert_eq!(
                    filter.test(&|t| arbitrary(salt, &t.text)),
                    legacy::multi_filter(p, &|t| arbitrary(salt, t)),
                    "{p:?}"
                );
            }
        }
        // A copy made before compiling, as a save or a runtime grant makes
        // one, compiles to the same answers.
        let fresh = Unique::parse(&u.text);
        if fresh.params == u.params {
            let f = fresh.compiled(r);
            for i in 0..u.params.len() {
                assert_eq!(f.stats(i), c.stats(i));
                assert_eq!(f.term(i).unwrap().names.building, c.term(i).unwrap().names.building);
            }
        }
    }
}

#[derive(Default)]
struct Tally {
    checked: usize,
    decided: usize,
}

impl Tally {
    fn add(&mut self, new: Verdict, old: Verdict, what: impl FnOnce() -> String) {
        assert_eq!(new, old, "{}", what());
        self.checked += 1;
        if new != Verdict::Unknown {
            self.decided += 1;
        }
    }
}

fn check_matchers(world: &World, r: &Ruleset, strings: &[String]) -> Tally {
    let mut tally = Tally::default();
    let filters: Vec<(&str, Filter)> = strings.iter().map(|s| (s.as_str(), Filter::compile(r, s))).collect();
    let buildings: Vec<Id<Building>> = r.buildings.ids().collect();
    let bases: Vec<Id<BaseUnit>> = r.units.ids().collect();
    let cities: Vec<_> = world.cities.iter().map(|(_, c)| c).collect();
    let tiles: Vec<TileId> = world.grid.ids().collect();
    let mut civs: Vec<Option<CivId>> = vec![None];
    civs.extend(world.civs.iter().filter(|c| !c.is_barbarian).map(|c| Some(c.id)).take(3));
    civs.extend(world.civs.iter().filter(|c| c.is_city_state()).map(|c| Some(c.id)).take(1));
    assert!(cities.len() >= 4 && world.units.iter().count() >= 8, "the game should have grown by now");

    for (s, f) in &filters {
        for &b in &buildings {
            tally.add(matches_building(r, b, f), legacy::building_matches(r, b, s), || format!("building {b:?} {s:?}"));
        }
        for res in r.resources.ids() {
            let verdict = |b: bool| if b { Verdict::True } else { Verdict::False };
            tally.add(verdict(matches_resource(r, res, f)), verdict(legacy::resource_matches(r, res, s)), || {
                format!("resource {res:?} {s:?}")
            });
        }
        for &b in &bases {
            tally.add(matches_base_unit(r, b, f), legacy::base_unit_matches(r, b, s), || format!("base {b:?} {s:?}"));
        }
        for (id, unit) in world.units.iter() {
            tally.add(matches_unit(r, unit, f), legacy::unit_matches(r, unit, s), || format!("unit {id:?} {s:?}"));
            tally.add(matches_unit_in(world, r, unit, f), legacy::unit_matches_in(world, r, unit, s), || {
                format!("unit in {id:?} {s:?}")
            });
        }
        for (n, subject) in cities.iter().enumerate() {
            let other = cities[(n + 1) % cities.len()];
            for origin in [None, Some(*subject), Some(other)] {
                let old = legacy::multi_filter(s, &|term| legacy::city_matches(world, r, subject, origin, term));
                tally.add(matches_city(world, r, subject, origin, f), old, || format!("city {} {s:?}", subject.name));
                tally.add(
                    city_matches(world, r, subject, origin, s),
                    legacy::city_matches(world, r, subject, origin, s),
                    || format!("city term {} {s:?}", subject.name),
                );
            }
        }
        for &civ in &civs {
            let mut ctx = GameContext::new(r).with_world(world);
            ctx.civ = civ;
            for &tile in &tiles {
                tally.add(matches_tile(&ctx, tile, f), legacy::tile_matches(&ctx, tile, s), || {
                    format!("tile {tile:?} {civ:?} {s:?}")
                });
                tally.add(
                    matches_tile_in(&world.grid, r, tile, f, civ),
                    legacy::tile_matches_in(&world.grid, r, tile, s, civ),
                    || format!("tile in {tile:?} {civ:?} {s:?}"),
                );
            }
        }
    }
    tally
}

#[test]
fn compiled_parameters_read_as_the_text_did() {
    for ruleset in civ_sim::content::RULESETS {
        let r = civ_sim::content::load_ruleset(ruleset).unwrap();
        let uniques = every_unique(&r);
        assert!(uniques.len() > 1000, "{ruleset}: {} uniques", uniques.len());
        check_params(&r, &uniques);
    }
}

#[test]
fn compiled_filters_match_as_the_string_matchers_did() {
    for ruleset in civ_sim::content::RULESETS {
        let (world, r) = play(ruleset, 70);
        let strings = filter_strings(&every_unique(&r));
        let tally = check_matchers(&world, &r, &strings);
        println!("{ruleset}: {} filters, {} answers compared, {} decided", strings.len(), tally.checked, tally.decided);
        assert!(tally.decided * 4 > tally.checked, "{ruleset}: most answers should be decided");
    }
}
