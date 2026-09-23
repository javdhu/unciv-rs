//! Validated settings and the complete content bundle produce a deterministic initial world.

use crate::content;
use civ_core::arena::Id;
use civ_core::ruleset::{Ruleset, Speed};
use civ_core::world::entities::Civilization;
use civ_core::world::mapgen::{generate_with_biases, MapParameters};
use civ_core::world::settling::is_settler;
use civ_core::world::turn::update_visibility_in;
use civ_core::world::{CivId, World};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", default, deny_unknown_fields)]
pub struct Settings {
    /// JSON settings accept only JavaScript-safe integer seeds. Authoritative saves preserve the seed.
    #[ts(type = "number")]
    pub seed: u64,
    pub width: u16,
    pub height: u16,
    pub majors: u8,
    pub city_states: u8,
    pub wrap_x: bool,
    /// The turn limit. Absent uses the speed's length (Speed.kt numTotalTurns): 330 on Quick,
    /// 500 on Standard.
    pub max_turns: Option<u32>,
    /// Game speed by name; Standard when absent.
    #[ts(optional)]
    pub speed: Option<String>,
    /// Absent enables every bundled victory. Names are validated against the ruleset.
    pub victories: Option<Vec<String>>,
    /// The local player's nation by name; the first major in seeded order when absent.
    pub nation: Option<String>,
}

impl Default for Settings {
    fn default() -> Settings {
        Settings {
            seed: 1,
            width: 32,
            height: 20,
            majors: 4,
            city_states: 2,
            wrap_x: false,
            max_turns: None,
            speed: None,
            victories: None,
            nation: None,
        }
    }
}

const DEFAULT_SPEED: &str = "Standard";

impl Settings {
    /// The chosen speed, validated against the ruleset.
    pub fn speed_in(&self, r: &Ruleset) -> Result<Id<Speed>, String> {
        let name = self.speed.as_deref().unwrap_or(DEFAULT_SPEED);
        r.speeds.id(name).ok_or_else(|| format!("unknown speed: {name}"))
    }

    /// The explicit `maxTurns`, or else the chosen speed's length.
    pub fn turn_limit(&self, r: &Ruleset) -> Result<u32, String> {
        let speed = self.speed_in(r)?;
        Ok(self.max_turns.unwrap_or_else(|| r.speeds[speed].total_turns.max(1) as u32))
    }
}

const NOT_PLAYABLE: [&str; 2] = ["Barbarians", "Spectator"];

/// A small deterministic shuffle, so a seed always deals the same nations.
fn seeded_order<T: Copy>(items: &[T], seed: u64) -> Vec<T> {
    let mut v = items.to_vec();
    let mut s = seed ^ 0x9E37_79B9_7F4A_7C15;
    for i in (1..v.len()).rev() {
        s ^= s << 13;
        s ^= s >> 7;
        s ^= s << 17;
        v.swap(i, (s % (i as u64 + 1)) as usize);
    }
    v
}

pub fn new_game(s: &Settings) -> Result<(World, Ruleset, CivId), String> {
    validate_settings(s)?;
    let rules = content::load()?;
    let wanted = usize::from(s.majors) + usize::from(s.city_states);

    let majors: Vec<_> = rules
        .nations
        .iter()
        .filter(|(_, n)| n.city_state_type.is_none() && !NOT_PLAYABLE.contains(&n.name.as_str()))
        .map(|(id, _)| id)
        .collect();
    let mut majors = seeded_order(&majors, s.seed);
    if let Some(name) = &s.nation {
        let pos = majors
            .iter()
            .position(|id| rules.nations[*id].name == *name)
            .ok_or_else(|| "unknown playable nation".to_string())?;
        let chosen = majors.remove(pos);
        majors.insert(0, chosen);
    }
    let city_states = seeded_order(
        &rules
            .nations
            .iter()
            .filter(|(_, n)| n.city_state_type.is_some())
            .map(|(id, _)| id)
            .collect::<Vec<_>>(),
        s.seed.rotate_left(17),
    );

    let biases: Vec<_> = majors
        .iter()
        .take(usize::from(s.majors))
        .chain(city_states.iter().take(usize::from(s.city_states)))
        .map(|n| rules.nations[*n].start_bias.clone())
        .collect();
    let map = generate_with_biases(
        &rules,
        MapParameters {
            width: s.width,
            height: s.height,
            seed: s.seed,
            wrap_x: s.wrap_x,
            ..Default::default()
        },
        &biases,
    );
    let enabled_victories = s
        .victories
        .as_ref()
        .map(|names| {
            names
                .iter()
                .map(|name| {
                    rules
                        .victories
                        .id(name)
                        .ok_or_else(|| format!("unknown victory: {name}"))
                })
                .collect::<Result<std::collections::BTreeSet<_>, _>>()
        })
        .transpose()?;
    if map.starts.len() < wanted
        || majors.len() < usize::from(s.majors)
        || city_states.len() < usize::from(s.city_states)
    {
        return Err("the requested civilizations do not fit this map or ruleset".into());
    }
    let starts = wanted;
    let n_majors = usize::from(s.majors);
    let mut civs = Vec::with_capacity(starts);
    for (i, &nation) in majors
        .iter()
        .take(n_majors)
        .chain(city_states.iter().take(usize::from(s.city_states)))
        .enumerate()
    {
        let id = CivId(i as u8);
        // Session creation assigns human controllers. This helper also supports trusted AI simulations.
        let mut civ = Civilization::new(id, nation, true);
        if i >= n_majors {
            let kind = rules.nations[nation].city_state_type.as_deref();
            civ.city_state_type = Some(
                rules
                    .city_state_types
                    .iter()
                    .find(|(_, t)| Some(t.name.as_str()) == kind)
                    .map(|(id, _)| id)
                    .ok_or("city-state type missing from complete ruleset")?,
            );
        }
        civs.push(civ);
    }

    let unit = |name: &str| {
        rules
            .units
            .iter()
            .find(|(_, u)| u.name == name)
            .map(|(id, _)| id)
    };
    let settler = rules
        .units
        .iter()
        .find(|(id, _)| is_settler(&rules, *id))
        .map(|(id, _)| id);
    let era = rules
        .eras
        .by_name("Ancient era")
        .ok_or("missing starting era")?;
    let (worker, warrior) = (unit("Worker"), unit(&era.starting_military_unit));

    let count = civs.len();
    let barbarian = CivId(count as u8);
    let barbarian_nation = rules
        .nations
        .id("Barbarians")
        .ok_or("ruleset has no barbarians")?;
    let mut barbarians = Civilization::new(barbarian, barbarian_nation, true);
    barbarians.is_barbarian = true;
    civs.push(barbarians);
    let mut world = World::new(map.grid, civs);
    world.difficulty = rules.difficulties.id("Prince");
    world.speed = Some(s.speed_in(&rules)?);
    world.endgame.max_turns = s.turn_limit(&rules)?;
    world.endgame.enabled_victories = enabled_victories;
    world.diplomacy.mark_hostile(barbarian);
    for i in 0..count {
        let civ = CivId(i as u8);
        let nation = world.civ(civ).expect("created civ").nation;
        let starting: Vec<_> = rules
            .technologies
            .iter()
            .filter(|(_, t)| {
                t.uniques
                    .iter()
                    .any(|u| u.unique_type() == Some(civ_core::UniqueType::StartingTech))
            })
            .map(|(id, _)| id)
            .chain(
                rules.nations[nation]
                    .uniques
                    .iter()
                    .filter(|u| u.unique_type() == Some(civ_core::UniqueType::StartsWithTech))
                    .filter_map(|u| {
                        u.params
                            .first()
                            .and_then(|name| rules.technologies.id(name))
                    }),
            )
            .collect();
        for tech in starting {
            if world.civ_mut(civ).expect("created civ").techs.insert(tech) {
                civ_core::world::effects::on_technology_researched(
                    &mut world,
                    &rules,
                    civ,
                    tech,
                    civ_core::stats::Fixed::ONE,
                );
            }
        }
        civ_core::world::policies::rebuild_uniques(&mut world, &rules, civ);
    }
    for (i, start) in map.starts.iter().take(count).enumerate() {
        let kit: Vec<_> = if i < n_majors {
            std::iter::repeat_n(settler, era.starting_settler_count.max(0) as usize)
                .chain(std::iter::repeat_n(
                    worker,
                    era.starting_worker_count.max(0) as usize,
                ))
                .chain(std::iter::repeat_n(
                    warrior,
                    era.starting_military_unit_count.max(0) as usize,
                ))
                .collect()
        } else {
            vec![settler]
        };
        for base in kit.iter().flatten() {
            let civ = CivId(i as u8);
            let base = civ_core::world::effects::equivalent_unit(&world, &rules, civ, *base);
            let unit = civ_core::world::construction::prepare_unit_near(
                &world, &rules, civ, base, *start, None,
            )
            .ok_or("no legal starting unit position")?;
            world.place_unit(unit);
        }
    }
    update_visibility_in(&mut world, &rules);
    civ_core::world::fog::capture_observations(&mut world, &rules);
    Ok((world, rules, CivId(0)))
}

/// Host resource bounds, checked before allocating or generating a map.
pub fn validate_settings(s: &Settings) -> Result<(), String> {
    if !(16..=128).contains(&s.width) || !(10..=80).contains(&s.height) {
        return Err("map dimensions must be between 16x10 and 128x80".into());
    }
    if !(1..=16).contains(&s.majors) || s.city_states > 16 {
        return Err("choose 1 to 16 civilizations and at most 16 city-states".into());
    }
    if s.max_turns.is_some_and(|t| t == 0 || t > 10_000) {
        return Err("maximum turns must be between 1 and 10000".into());
    }
    if s.victories
        .as_ref()
        .is_some_and(|v| v.is_empty() || v.len() > 8 || v.iter().any(|n| n.len() > 100))
    {
        return Err("choose at least one bundled victory".into());
    }
    if s.seed > 9_007_199_254_740_991 {
        return Err("seed exceeds the exact JSON integer range".into());
    }
    if s.speed.as_ref().is_some_and(|n| n.len() > 100) {
        return Err("speed name is too long".into());
    }
    if s.nation.as_ref().is_some_and(|n| n.len() > 100) {
        return Err("nation name is too long".into());
    }
    Ok(())
}
