//! Winning.
//!
//! Victory conditions are not code in Civ — they are lists of *milestones*
//! written in the ruleset as strings:
//!
//! ```text
//! Scientific  -> Build [Apollo Program], Add all [spaceship parts] in capital
//! Cultural    -> Complete [5] Policy branches, Build [Utopia Project]
//! Domination  -> Capture all capitals
//! ```
//!
//! So this is another small language, parsed rather than hard-coded, and a
//! ruleset that adds a victory type gets it for free.
//!
//! Milestones evaluate to a [`Verdict`], not a bool, for the same reason
//! conditionals do: a milestone this engine cannot judge must not read as
//! "not yet achieved", because that is indistinguishable from an unimplemented
//! one. A victory whose milestones cannot all be judged is simply not awarded,
//! and the gap stays countable.

use crate::arena::Id;
use crate::ruleset::{Ruleset, Victory};
use crate::world::conditionals::Verdict;
use crate::world::grid::CivId;
use crate::world::world::World;
use std::collections::BTreeSet;

/// One requirement of a victory condition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Milestone {
    /// This civ has the building somewhere.
    BuiltBuilding(String),
    /// Anyone has it — a race, not a personal goal.
    BuildingBuiltGlobally(String),
    /// Every original major capital is this civ's, with no living major still
    /// waiting to found its original capital.
    CaptureAllCapitals,
    /// Every other civ is out.
    DestroyAllPlayers,
    /// This many policy branches finished to the last policy.
    CompletePolicyBranches(i32),
    SpaceshipParts,
    DiplomaticVote,
    HighestScore,
    /// Lingua Franca (civ.game): influential over every rival nation
    /// (`world::influence`).
    InfluentialOverAll,
    /// Reserve Currency (civ.game): at least `percent` of world trade for
    /// each of the last `turns` turns (`world::influence`).
    WorldTradeShare {
        percent: u32,
        turns: u32,
    },
    /// Milestones outside the bundled rules remain explicitly unsupported.
    Unsupported(String),
}

/// Parse a milestone line. Unrecognised forms are preserved, not discarded.
pub fn parse_milestone(text: &str) -> Milestone {
    let t = text.trim();
    let param = |prefix: &str, suffix: &str| -> Option<&str> { t.strip_prefix(prefix)?.strip_suffix(suffix) };

    if let Some(b) = param("Build [", "]") {
        return Milestone::BuiltBuilding(b.to_string());
    }
    if let Some(b) = param("Anyone should build [", "]") {
        return Milestone::BuildingBuiltGlobally(b.to_string());
    }
    if let Some(n) = param("Complete [", "] Policy branches") {
        if let Ok(n) = n.parse() {
            return Milestone::CompletePolicyBranches(n);
        }
    }
    if let Some((percent, turns)) = param("Carry [", "] turns").and_then(|p| p.split_once("]% of world trade for [")) {
        if let (Ok(percent), Ok(turns)) = (percent.parse::<u32>(), turns.parse::<u32>()) {
            if percent <= 100 && (1..=crate::world::influence::TRADE_HISTORY as u32).contains(&turns) {
                return Milestone::WorldTradeShare { percent, turns };
            }
        }
    }
    match t {
        "Capture all capitals" => Milestone::CaptureAllCapitals,
        "Destroy all players" => Milestone::DestroyAllPlayers,
        "Add all [spaceship parts] in capital" => Milestone::SpaceshipParts,
        "Win diplomatic vote" => Milestone::DiplomaticVote,
        "Have highest score after max turns" => Milestone::HighestScore,
        "Become influential over all rival nations" => Milestone::InfluentialOverAll,
        _ => Milestone::Unsupported(t.to_string()),
    }
}

/// Whether the selected match enables this named victory.
pub fn is_enabled(world: &World, r: &Ruleset, name: &str) -> bool {
    r.victories.id(name).is_some_and(|id| world.endgame.enabled_victories.as_ref().is_none_or(|v| v.contains(&id)))
}

pub fn required_parts(r: &Ruleset, victory: &Victory) -> std::collections::BTreeMap<Id<crate::ruleset::BaseUnit>, u32> {
    let mut parts = std::collections::BTreeMap::new();
    for name in &victory.required_spaceship_parts {
        if let Some(id) = r.units.id(name) {
            *parts.entry(id).or_default() += 1;
        }
    }
    parts
}

fn spaceship_complete(world: &World, r: &Ruleset, civ: CivId, victory: &Victory) -> bool {
    // Missing rule references cannot silently shorten the required ship.
    victory.required_spaceship_parts.iter().all(|name| r.units.id(name).is_some())
        && required_parts(r, victory)
            .into_iter()
            .all(|(part, n)| crate::world::victory_actions::assembled_parts(world, civ, part) >= n)
}

/// Exact rational score. The denominator is common across civilizations, so
/// time-victory tie breaking never depends on floating-point rounding.
pub fn score_fraction(world: &World, r: &Ruleset, civ: CivId) -> (i64, i64) {
    let Some(c) = world.civ(civ) else { return (0, 1) };
    let tiles = world.grid.len().max(1) as i64;
    let (num, den) = if tiles < 1276 { (1276 + 2 * tiles, 3 * tiles) } else { (1276, tiles) };
    let mut base = 0i64;
    let mut wonders = 0i64;
    for city in c.cities.iter().filter_map(|id| world.cities.get(*id)) {
        base += 10
            + 3 * i64::from(city.population)
            + city.tiles.iter().filter(|t| !world.grid.is_water(r, **t)).count() as i64;
        wonders +=
            city.built.iter().filter(|id| r.buildings.get(**id).is_some_and(|b| b.is_wonder == Some(true))).count()
                as i64;
    }
    (base * num + (40 * wonders + 4 * c.techs.len() as i64 + 10 * i64::from(c.victory.repeated_techs)) * den, den)
}

pub fn score(world: &World, r: &Ruleset, civ: CivId) -> crate::stats::Fixed {
    let (num, den) = score_fraction(world, r, civ);
    crate::stats::Fixed::from_fraction(num, den)
}

pub fn score_leader(world: &World, r: &Ruleset) -> Option<CivId> {
    // Kotlin maxByOrNull keeps the first entry on a tie, including when a
    // defeated civilization still leads through its technologies.
    let mut best = None;
    let mut highest = i64::MIN;
    for civ in &world.civs {
        let value = score_fraction(world, r, civ.id).0;
        if value > highest {
            highest = value;
            best = Some(civ.id);
        }
    }
    best
}

/// How many policy branches this civ has taken to completion.
///
/// A branch counts when every policy in it is adopted — including the branch
/// itself, which is a policy (`Ruleset.kt:458`).
pub fn completed_branches(world: &World, r: &Ruleset, civ: CivId) -> usize {
    let Some(c) = world.civ(civ) else { return 0 };
    r.policy_branches
        .ids()
        .filter(|branch| {
            let members: Vec<_> =
                r.policies.iter().filter(|(_, p)| p.branch == Some(*branch)).map(|(id, _)| id).collect();
            !members.is_empty() && members.iter().all(|m| c.adopted_policies.contains(m))
        })
        .count()
}

/// Original capitals survive a Palace move and their founder's defeat. A
/// living major without an original capital still counts: it can found one
/// later (`Victory.kt:civsWithPotentialCapitalsToOwn`).
fn owns_all_original_capitals(world: &World, civ: CivId) -> bool {
    let mut required: BTreeSet<_> = world.civs.iter().filter(|c| c.is_major() && c.is_alive()).map(|c| c.id).collect();
    let mut owned = BTreeSet::new();
    for (_, city) in world.cities.iter() {
        if !city.original_capital || !world.civ(city.founder).is_some_and(|c| c.is_major()) {
            continue;
        }
        required.insert(city.founder);
        if city.owner != civ {
            return false;
        }
        owned.insert(city.founder);
    }
    // Compare founders, so duplicate flags in an older save cannot stand in
    // for a different civilization's still-unfounded capital.
    !owned.is_empty() && owned == required
}

/// Has this civ met the milestone?
pub fn evaluate(m: &Milestone, world: &World, r: &Ruleset, civ: CivId) -> Verdict {
    let yes = |b: bool| if b { Verdict::True } else { Verdict::False };
    let has_building = |owner: Option<CivId>, name: &str| {
        let Some(id) = r.buildings.id(name) else { return false };
        world.cities.iter().filter(|(_, c)| owner.is_none_or(|o| c.owner == o)).any(|(_, c)| c.built.contains(&id))
    };

    match m {
        Milestone::BuiltBuilding(name) => yes(has_building(Some(civ), name)),
        Milestone::BuildingBuiltGlobally(name) => yes(has_building(None, name)),
        Milestone::CompletePolicyBranches(n) => yes(completed_branches(world, r, civ) >= *n as usize),
        Milestone::DestroyAllPlayers => {
            yes(world.civs.iter().filter(|c| c.id != civ && c.is_major()).all(|c| !c.is_alive()))
        }
        Milestone::SpaceshipParts => yes(r
            .victories
            .values()
            .any(|v| !v.required_spaceship_parts.is_empty() && spaceship_complete(world, r, civ, v))),
        Milestone::DiplomaticVote => yes(world.civ(civ).is_some_and(|c| c.victory.diplomatic_win)),
        Milestone::HighestScore => yes(world.turn >= world.endgame.max_turns && score_leader(world, r) == Some(civ)),
        Milestone::CaptureAllCapitals => yes(owns_all_original_capitals(world, civ)),
        Milestone::InfluentialOverAll => yes(crate::world::influence::influential_over_all(world, r, civ)),
        Milestone::WorldTradeShare { percent, turns } => {
            yes(crate::world::influence::held_trade_share(world, civ, *percent, *turns))
        }
        Milestone::Unsupported(_) => Verdict::Unknown,
    }
}

/// Progress toward one victory condition, milestone by milestone.
pub fn progress(world: &World, r: &Ruleset, civ: CivId, victory: &Victory) -> Vec<(Milestone, Verdict)> {
    victory
        .milestones
        .iter()
        .map(|text| {
            let m = parse_milestone(text);
            let v = if m == Milestone::SpaceshipParts {
                if spaceship_complete(world, r, civ, victory) {
                    Verdict::True
                } else {
                    Verdict::False
                }
            } else {
                evaluate(&m, world, r, civ)
            };
            (m, v)
        })
        .collect()
}

/// Whether a civ has achieved a victory condition outright.
///
/// Every milestone must be `True`. One that cannot be judged blocks the award,
/// which is the safe direction: declaring a winner wrongly ends the game.
pub fn achieved(world: &World, r: &Ruleset, civ: CivId, victory: &Victory) -> bool {
    let steps = progress(world, r, civ, victory);
    !steps.is_empty() && steps.iter().all(|(_, v)| *v == Verdict::True)
}

/// The first civ to have won, in id order, and which condition it won by.
///
/// Returns the victory's id rather than its name: events reference ruleset
/// objects by id everywhere else, and a name would either allocate or leak.
pub fn winner(world: &World, r: &Ruleset) -> Option<(CivId, Id<Victory>)> {
    for civ in world.living_civs().collect::<Vec<_>>() {
        // City-states and barbarians are not playing to win, and a domination
        // condition that counted them would never be met — nobody sets out to
        // capture every city-state capital on the map.
        if !world.civ(civ).is_some_and(|c| c.is_major()) {
            continue;
        }
        for (id, victory) in r.victories.iter() {
            if world.endgame.enabled_victories.as_ref().is_none_or(|enabled| enabled.contains(&id))
                && achieved(world, r, civ, victory)
            {
                return Some((civ, id));
            }
        }
    }
    None
}

/// Buildings and units that are a step toward some victory.
///
/// The AI needs this because its construction scoring is otherwise entirely
/// goal-agnostic: it values a building by its yields, and an Apollo Program
/// yields nothing at all. A civ two techs from a science victory would keep
/// building soldiers because the war made soldiers score highly, and the thing
/// that actually wins the game scored zero.
///
/// Both lists are by name, because a milestone names a thing rather than
/// pointing at one, and a ruleset may not have every name a victory mentions.
pub fn victory_constructions(r: &Ruleset) -> (Vec<Id<crate::ruleset::Building>>, Vec<String>) {
    victory_constructions_inner(r)
}

/// How near a civ is to one victory, as `(met, judgeable)`.
///
/// Milestones this engine cannot judge are left out of *both* numbers rather
/// than counted as unmet: a victory whose conditions are half-unreadable
/// should not read as "barely started", because that would make every civ look
/// permanently far from winning and the AI would never reach for any of them.
pub fn nearness(world: &World, r: &Ruleset, civ: CivId, victory: &Victory) -> (usize, usize) {
    let mut met = 0;
    let mut judgeable = 0;
    for (_, verdict) in progress(world, r, civ, victory) {
        match verdict {
            Verdict::Unknown => {}
            Verdict::True => {
                met += 1;
                judgeable += 1;
            }
            Verdict::False => judgeable += 1,
        }
    }
    (met, judgeable)
}

/// The victories a given building is a milestone of.
pub fn victories_needing(r: &Ruleset, building: Id<crate::ruleset::Building>) -> Vec<Id<Victory>> {
    let Some(name) = r.buildings.name_of(building) else { return Vec::new() };
    r.victories
        .iter()
        .filter(|(_, v)| {
            v.milestones.iter().any(|m| {
                matches!(
                    parse_milestone(m),
                    Milestone::BuiltBuilding(ref n) | Milestone::BuildingBuiltGlobally(ref n)
                        if n == name
                )
            })
        })
        .map(|(id, _)| id)
        .collect()
}

fn victory_constructions_inner(r: &Ruleset) -> (Vec<Id<crate::ruleset::Building>>, Vec<String>) {
    let mut buildings = Vec::new();
    let mut spaceship_parts = Vec::new();
    for (_, v) in r.victories.iter() {
        for m in &v.milestones {
            match parse_milestone(m) {
                Milestone::BuiltBuilding(name) | Milestone::BuildingBuiltGlobally(name) => {
                    if let Some(id) = r.buildings.id(&name) {
                        buildings.push(id);
                    }
                }
                _ => {}
            }
        }
        spaceship_parts.extend(v.required_spaceship_parts.iter().cloned());
    }
    buildings.sort_by_key(|i| i.index());
    buildings.dedup();
    spaceship_parts.sort();
    spaceship_parts.dedup();
    (buildings, spaceship_parts)
}

/// How many of the ruleset's milestones this engine can judge.
pub fn milestone_coverage(r: &Ruleset) -> (usize, usize) {
    let mut judgeable = 0;
    let mut total = 0;
    for (_, v) in r.victories.iter() {
        for text in &v.milestones {
            total += 1;
            if !matches!(parse_milestone(text), Milestone::Unsupported(_)) {
                judgeable += 1;
            }
        }
    }
    (judgeable, total)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::city::City;
    use crate::world::conquest;
    use crate::world::entities::{Civilization, Unit};
    use crate::world::grid::TileGrid;

    fn ruleset() -> Ruleset {
        Ruleset::load_dir("rules/gods-and-kings").expect("readable").expect("loads")
    }

    fn world(r: &Ruleset) -> World {
        let g = TileGrid::new(16, 16, r.terrain("Grassland").unwrap());
        let nation = r.nations.ids().next().unwrap();
        World::new(g, vec![Civilization::new(CivId(0), nation, false), Civilization::new(CivId(1), nation, true)])
    }

    #[test]
    fn milestones_parse_from_the_ruleset() {
        assert_eq!(parse_milestone("Build [Apollo Program]"), Milestone::BuiltBuilding("Apollo Program".into()));
        assert_eq!(
            parse_milestone("Anyone should build [United Nations]"),
            Milestone::BuildingBuiltGlobally("United Nations".into())
        );
        assert_eq!(parse_milestone("Complete [5] Policy branches"), Milestone::CompletePolicyBranches(5));
        assert_eq!(parse_milestone("Capture all capitals"), Milestone::CaptureAllCapitals);
        // Kept, not dropped.
        assert_eq!(parse_milestone("Win diplomatic vote"), Milestone::DiplomaticVote);
    }

    #[test]
    fn report_milestone_coverage() {
        let r = ruleset();
        let (judgeable, total) = milestone_coverage(&r);
        println!("milestones judgeable: {judgeable}/{total}");
        for (_, v) in r.victories.iter() {
            let states: Vec<String> = v
                .milestones
                .iter()
                .map(|t| match parse_milestone(t) {
                    Milestone::Unsupported(s) => format!("[?] {s}"),
                    other => format!("[x] {other:?}"),
                })
                .collect();
            println!("  {}: {}", v.name, states.join(", "));
        }
        assert_eq!(judgeable, total, "all bundled victory milestones are implemented");
    }

    #[test]
    fn building_milestones_read_the_right_civs_cities() {
        let r = ruleset();
        let mut w = world(&r);
        let wonder = r
            .buildings
            .iter()
            .find(|(_, b)| b.is_wonder == Some(true))
            .map(|(id, b)| (id, b.name.clone()))
            .expect("the ruleset has wonders");

        let mine = w.found_city(City::found("Rome", CivId(0), TileIdOf(&w, 4, 4)));
        let theirs = w.found_city(City::found("Antium", CivId(1), TileIdOf(&w, 10, 10)));

        let built = Milestone::BuiltBuilding(wonder.1.clone());
        let anyone = Milestone::BuildingBuiltGlobally(wonder.1.clone());
        assert_eq!(evaluate(&built, &w, &r, CivId(0)), Verdict::False);

        // Built by the other civ: global yes, personal no.
        w.cities.get_mut(theirs).unwrap().built.insert(wonder.0);
        assert_eq!(evaluate(&anyone, &w, &r, CivId(0)), Verdict::True);
        assert_eq!(evaluate(&built, &w, &r, CivId(0)), Verdict::False);

        w.cities.get_mut(mine).unwrap().built.insert(wonder.0);
        assert_eq!(evaluate(&built, &w, &r, CivId(0)), Verdict::True);
    }

    #[allow(non_snake_case)]
    fn TileIdOf(w: &World, x: u16, y: u16) -> crate::world::grid::TileId {
        w.grid.id_at(x, y).unwrap()
    }

    #[test]
    fn a_branch_counts_only_when_every_policy_in_it_is_taken() {
        let r = ruleset();
        let mut w = world(&r);
        assert_eq!(completed_branches(&w, &r, CivId(0)), 0);

        // Adopt every policy of one branch.
        let branch = r.policy_branches.ids().next().unwrap();
        let members: Vec<_> = r.policies.iter().filter(|(_, p)| p.branch == Some(branch)).map(|(id, _)| id).collect();
        assert!(members.len() > 1, "a branch has several policies");

        for m in &members[..members.len() - 1] {
            w.civ_mut(CivId(0)).unwrap().adopted_policies.insert(*m);
        }
        assert_eq!(completed_branches(&w, &r, CivId(0)), 0, "one short is not complete");

        w.civ_mut(CivId(0)).unwrap().adopted_policies.insert(members[members.len() - 1]);
        assert_eq!(completed_branches(&w, &r, CivId(0)), 1);
    }

    #[test]
    fn capturing_all_capitals_means_owning_every_one() {
        let r = ruleset();
        let mut w = world(&r);
        // No capitals yet — an empty map is not a conquest.
        assert_eq!(evaluate(&Milestone::CaptureAllCapitals, &w, &r, CivId(0)), Verdict::False);

        let a = TileIdOf(&w, 4, 4);
        let b = TileIdOf(&w, 10, 10);
        w.found_city_in(&r, City::found("Rome", CivId(0), a));
        assert_eq!(evaluate(&Milestone::CaptureAllCapitals, &w, &r, CivId(0)), Verdict::True);

        w.found_city_in(&r, City::found("Antium", CivId(1), b));
        assert_eq!(
            evaluate(&Milestone::CaptureAllCapitals, &w, &r, CivId(0)),
            Verdict::False,
            "someone else has a capital now"
        );
    }

    #[test]
    fn unfounded_living_majors_block_domination_until_eliminated() {
        let r = ruleset();
        let mut w = world(&r);
        w.found_city_in(&r, City::found("Rome", CivId(0), TileIdOf(&w, 4, 4)));
        let settler = Unit::new(r.unit("Settler").unwrap(), CivId(1), TileIdOf(&w, 10, 10), 2);
        let unit = w.place_unit(settler.clone());
        assert_eq!(evaluate(&Milestone::CaptureAllCapitals, &w, &r, CivId(0)), Verdict::False);
        assert!(winner(&w, &r).is_none());

        // An eliminated civilization which never founded has no capital to take.
        w.remove_unit(unit);
        assert_eq!(evaluate(&Milestone::CaptureAllCapitals, &w, &r, CivId(0)), Verdict::True);

        // If it returns before the result is awarded, its potential capital counts again.
        w.place_unit(settler);
        assert_eq!(evaluate(&Milestone::CaptureAllCapitals, &w, &r, CivId(0)), Verdict::False);
    }

    #[test]
    fn minor_capitals_and_a_moved_palace_are_not_domination_targets() {
        let r = ruleset();
        let mut w = world(&r);
        w.civ_mut(CivId(1)).unwrap().city_state_type = r.city_state_types.ids().next();
        let mut barbarians = Civilization::new(CivId(2), w.civ(CivId(0)).unwrap().nation, true);
        barbarians.is_barbarian = true;
        w.civs.push(barbarians);
        let original = w.found_city_in(&r, City::found("Rome", CivId(0), TileIdOf(&w, 4, 4)));
        w.found_city_in(&r, City::found("City-state", CivId(1), TileIdOf(&w, 10, 10)));
        w.found_city_in(&r, City::found("Barbarian city", CivId(2), TileIdOf(&w, 1, 12)));
        let palace = r.building("Palace").unwrap();
        w.cities.get_mut(original).unwrap().built.remove(&palace);
        assert_eq!(evaluate(&Milestone::CaptureAllCapitals, &w, &r, CivId(0)), Verdict::True);
    }

    #[test]
    fn capturing_originals_wins_despite_palace_moves_and_reverses_on_recapture() {
        let r = ruleset();
        let mut w = world(&r);
        w.found_city_in(&r, City::found("Rome", CivId(0), TileIdOf(&w, 4, 4)));
        let original = w.found_city_in(&r, City::found("Antium", CivId(1), TileIdOf(&w, 10, 10)));
        let replacement = w.found_city_in(&r, City::found("Cumae", CivId(1), TileIdOf(&w, 12, 12)));
        let palace = r.building("Palace").unwrap();
        w.cities.get_mut(original).unwrap().built.remove(&palace);
        w.cities.get_mut(replacement).unwrap().built.insert(palace);
        assert_eq!(evaluate(&Milestone::CaptureAllCapitals, &w, &r, CivId(0)), Verdict::False);

        conquest::conquer(&mut w, &r, original, CivId(0)).unwrap();
        assert_eq!(conquest::can_raze(&w, original), Err(conquest::RazeRejection::OriginalCapital));
        assert_eq!(evaluate(&Milestone::CaptureAllCapitals, &w, &r, CivId(0)), Verdict::True);
        assert_eq!(winner(&w, &r), Some((CivId(0), r.victories.id("Domination").unwrap())));

        conquest::conquer(&mut w, &r, original, CivId(1)).unwrap();
        assert_eq!(evaluate(&Milestone::CaptureAllCapitals, &w, &r, CivId(0)), Verdict::False);
    }

    #[test]
    fn defeated_founders_still_count_while_their_original_capitals_exist() {
        let r = ruleset();
        let mut w = world(&r);
        let mut minor = Civilization::new(CivId(2), w.civ(CivId(0)).unwrap().nation, true);
        minor.city_state_type = r.city_state_types.ids().next();
        w.civs.push(minor);
        w.found_city_in(&r, City::found("Rome", CivId(0), TileIdOf(&w, 4, 4)));
        let original = w.found_city_in(&r, City::found("Antium", CivId(1), TileIdOf(&w, 10, 10)));
        conquest::conquer(&mut w, &r, original, CivId(2)).unwrap();
        w.civ_mut(CivId(1)).unwrap().defeated = true;
        assert_eq!(evaluate(&Milestone::CaptureAllCapitals, &w, &r, CivId(0)), Verdict::False);
        conquest::conquer(&mut w, &r, original, CivId(0)).unwrap();
        assert_eq!(evaluate(&Milestone::CaptureAllCapitals, &w, &r, CivId(0)), Verdict::True);

        // A map/mod may remove an original capital outright. Its dead founder
        // no longer contributes a target, but a restored city makes it a live
        // major again and restores the original-capital requirement.
        conquest::destroy(&mut w, original);
        assert_eq!(evaluate(&Milestone::CaptureAllCapitals, &w, &r, CivId(0)), Verdict::True);
        w.civ_mut(CivId(1)).unwrap().defeated = false;
        let restored = w.found_city(City::found("Restored city", CivId(1), TileIdOf(&w, 10, 10)));
        w.cities.get_mut(restored).unwrap().original_capital = false;
        assert_eq!(evaluate(&Milestone::CaptureAllCapitals, &w, &r, CivId(0)), Verdict::False);
    }

    #[test]
    fn founding_a_replacement_capital_does_not_create_another_original() {
        let r = ruleset();
        let mut w = world(&r);
        w.found_city_in(&r, City::found("Rome", CivId(0), TileIdOf(&w, 4, 4)));
        let original = w.found_city_in(&r, City::found("Antium", CivId(1), TileIdOf(&w, 10, 10)));
        w.place_unit(Unit::new(r.unit("Settler").unwrap(), CivId(1), TileIdOf(&w, 12, 12), 2));
        conquest::conquer(&mut w, &r, original, CivId(0)).unwrap();
        let replacement = w.found_city_in(&r, City::found("Cumae", CivId(1), TileIdOf(&w, 12, 12)));
        let city = w.cities.get(replacement).unwrap();
        assert!(!city.original_capital);
        assert!(city.built.contains(&r.building("Palace").unwrap()));
        assert!(conquest::can_raze(&w, replacement).is_ok());
        assert_eq!(evaluate(&Milestone::CaptureAllCapitals, &w, &r, CivId(0)), Verdict::True);
    }

    #[test]
    fn owning_a_captured_city_does_not_prevent_founding_an_original_capital() {
        let r = ruleset();
        let mut w = world(&r);
        let original = w.found_city_in(&r, City::found("Antium", CivId(1), TileIdOf(&w, 10, 10)));
        conquest::conquer(&mut w, &r, original, CivId(0)).unwrap();
        assert_eq!(evaluate(&Milestone::CaptureAllCapitals, &w, &r, CivId(0)), Verdict::False);
        let mine = w.found_city_in(&r, City::found("Rome", CivId(0), TileIdOf(&w, 4, 4)));
        assert!(w.cities.get(mine).unwrap().original_capital);
        assert_eq!(evaluate(&Milestone::CaptureAllCapitals, &w, &r, CivId(0)), Verdict::True);
    }

    /// An unjudgeable milestone blocks the award rather than being ignored.
    #[test]
    fn an_incomplete_spaceship_is_not_awarded() {
        let r = ruleset();
        let mut w = world(&r);
        let scientific = r
            .victories
            .values()
            .find(|v| v.milestones.iter().any(|m| m.contains("spaceship")))
            .expect("the scientific victory needs spaceship parts");

        // Give the civ everything that *can* be judged.
        if let Some(apollo) = r.building("Apollo Program") {
            let city = w.found_city(City::found("Rome", CivId(0), TileIdOf(&w, 4, 4)));
            w.cities.get_mut(city).unwrap().built.insert(apollo);
        }
        let steps = progress(&w, &r, CivId(0), scientific);
        assert!(steps.iter().any(|(_, v)| *v == Verdict::False));
        assert!(!achieved(&w, &r, CivId(0), scientific), "the unassembled ship blocks it");
    }
}
