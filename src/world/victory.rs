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

/// One requirement of a victory condition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Milestone {
    /// This civ has the building somewhere.
    BuiltBuilding(String),
    /// Anyone has it — a race, not a personal goal.
    BuildingBuiltGlobally(String),
    /// Every capital on the map is this civ's.
    CaptureAllCapitals,
    /// Every other civ is out.
    DestroyAllPlayers,
    /// This many policy branches finished to the last policy.
    CompletePolicyBranches(i32),
    /// Recognised by the ruleset but not judgeable here: spaceship parts,
    /// religion, diplomatic votes, score. Kept as text so the gap is visible
    /// rather than silently dropped.
    Unsupported(String),
}

/// Parse a milestone line. Unrecognised forms are preserved, not discarded.
pub fn parse_milestone(text: &str) -> Milestone {
    let t = text.trim();
    let param = |prefix: &str, suffix: &str| -> Option<&str> {
        t.strip_prefix(prefix)?.strip_suffix(suffix)
    };

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
    match t {
        "Capture all capitals" => Milestone::CaptureAllCapitals,
        "Destroy all players" => Milestone::DestroyAllPlayers,
        _ => Milestone::Unsupported(t.to_string()),
    }
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
            let members: Vec<_> = r
                .policies
                .iter()
                .filter(|(_, p)| p.branch == Some(*branch))
                .map(|(id, _)| id)
                .collect();
            !members.is_empty() && members.iter().all(|m| c.adopted_policies.contains(m))
        })
        .count()
}

/// Every city that is somebody's capital.
fn capitals(world: &World, r: &Ruleset) -> Vec<(CivId, Id<crate::world::city::City>)> {
    let palace = r.buildings.iter().find(|(_, b)| {
        b.uniques
            .iter()
            .any(|u| u.unique_type() == Some(crate::UniqueType::IndicatesCapital))
    });
    let Some((palace, _)) = palace else { return Vec::new() };
    world
        .cities
        .iter()
        .filter(|(_, c)| c.built.contains(&palace))
        .map(|(id, c)| (c.owner, id))
        .collect()
}

/// Has this civ met the milestone?
pub fn evaluate(m: &Milestone, world: &World, r: &Ruleset, civ: CivId) -> Verdict {
    let yes = |b: bool| if b { Verdict::True } else { Verdict::False };
    let has_building = |owner: Option<CivId>, name: &str| {
        let Some(id) = r.buildings.id(name) else { return false };
        world
            .cities
            .iter()
            .filter(|(_, c)| owner.is_none_or(|o| c.owner == o))
            .any(|(_, c)| c.built.contains(&id))
    };

    match m {
        Milestone::BuiltBuilding(name) => yes(has_building(Some(civ), name)),
        Milestone::BuildingBuiltGlobally(name) => yes(has_building(None, name)),
        Milestone::CompletePolicyBranches(n) => yes(completed_branches(world, r, civ) >= *n as usize),
        Milestone::DestroyAllPlayers => yes(world
            .civs
            .iter()
            .filter(|c| c.id != civ)
            .all(|c| !c.is_alive())),
        Milestone::CaptureAllCapitals => {
            let all = capitals(world, r);
            // No capitals at all is not a victory, it is an empty map.
            yes(!all.is_empty() && all.iter().all(|(owner, _)| *owner == civ))
        }
        Milestone::Unsupported(_) => Verdict::Unknown,
    }
}

/// Progress toward one victory condition, milestone by milestone.
pub fn progress(
    world: &World,
    r: &Ruleset,
    civ: CivId,
    victory: &Victory,
) -> Vec<(Milestone, Verdict)> {
    victory
        .milestones
        .iter()
        .map(|text| {
            let m = parse_milestone(text);
            let v = evaluate(&m, world, r, civ);
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
            if achieved(world, r, civ, victory) {
                return Some((civ, id));
            }
        }
    }
    None
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
    use crate::world::entities::Civilization;
    use crate::world::grid::TileGrid;

    fn ruleset() -> Ruleset {
        Ruleset::load_dir("../android/assets/jsons/Civ V - Gods & Kings")
            .expect("readable")
            .expect("loads")
    }

    fn world(r: &Ruleset) -> World {
        let g = TileGrid::new(16, 16, r.terrain("Grassland").unwrap());
        let nation = r.nations.ids().next().unwrap();
        World::new(
            g,
            vec![
                Civilization::new(CivId(0), nation, false),
                Civilization::new(CivId(1), nation, true),
            ],
        )
    }

    #[test]
    fn milestones_parse_from_the_ruleset() {
        assert_eq!(
            parse_milestone("Build [Apollo Program]"),
            Milestone::BuiltBuilding("Apollo Program".into())
        );
        assert_eq!(
            parse_milestone("Anyone should build [United Nations]"),
            Milestone::BuildingBuiltGlobally("United Nations".into())
        );
        assert_eq!(parse_milestone("Complete [5] Policy branches"), Milestone::CompletePolicyBranches(5));
        assert_eq!(parse_milestone("Capture all capitals"), Milestone::CaptureAllCapitals);
        // Kept, not dropped.
        assert_eq!(
            parse_milestone("Win diplomatic vote"),
            Milestone::Unsupported("Win diplomatic vote".into())
        );
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
        assert!(judgeable > 0 && judgeable < total, "some but not all, at this stage");
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
        let members: Vec<_> = r
            .policies
            .iter()
            .filter(|(_, p)| p.branch == Some(branch))
            .map(|(id, _)| id)
            .collect();
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

    /// An unjudgeable milestone blocks the award rather than being ignored.
    #[test]
    fn a_victory_with_an_unsupported_milestone_is_not_awarded() {
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
        assert!(steps.iter().any(|(_, v)| *v == Verdict::Unknown));
        assert!(!achieved(&w, &r, CivId(0), scientific), "an unjudged step blocks it");
    }
}
