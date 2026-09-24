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
    /// The turn limit. Absent uses the speed's length (Speed.kt numTotalTurns): 160 on Quick,
    /// 240 on Standard.
    pub max_turns: Option<u32>,
    /// Game speed by name; the ruleset's own when absent (Standard, or Blitz
    /// for the Blitz ruleset).
    #[ts(optional)]
    pub speed: Option<String>,
    /// `base` (the default) or `blitz`.
    #[ts(optional)]
    pub ruleset: Option<String>,
    /// Absent enables every bundled victory. Names are validated against the ruleset.
    pub victories: Option<Vec<String>>,
    /// The local player's nation by name; the first major in seeded order when absent.
    pub nation: Option<String>,
    /// The local player's leader by name: the nation's own leader or its
    /// `altLeader`. Absent leads with the default. AI civs draw one by seed.
    #[ts(optional)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub leader: Option<String>,
    /// A Blitz slice by name (`RuleTables.slices`: Cradles, Crossroads,
    /// Engines): the game starts in its first era, researches no further
    /// than its last, offers only the victories reachable within it, and
    /// ends after its turns.
    #[ts(optional)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub slice: Option<String>,
    /// Each human seat's own nation and leader, for a multiplayer room. Seat
    /// 0's entry, if any, overrides `nation` and `leader`.
    #[ts(optional)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub seats: Option<Vec<SeatChoice>>,
}

/// What one seat chose in the lobby.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SeatChoice {
    pub seat: u8,
    #[ts(optional)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nation: Option<String>,
    #[ts(optional)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub leader: Option<String>,
}

impl Settings {
    /// The nation and leader a seat chose: its `seats` entry, or for seat 0
    /// the top-level `nation` and `leader`.
    pub fn choice(&self, seat: usize) -> (Option<&str>, Option<&str>) {
        if let Some(c) = self.seats.iter().flatten().find(|c| usize::from(c.seat) == seat) {
            return (c.nation.as_deref(), c.leader.as_deref());
        }
        if seat == 0 {
            (self.nation.as_deref(), self.leader.as_deref())
        } else {
            (None, None)
        }
    }
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
            ruleset: None,
            victories: None,
            nation: None,
            leader: None,
            slice: None,
            seats: None,
        }
    }
}

impl Settings {
    /// The chosen ruleset's name.
    pub fn ruleset_name(&self) -> &str {
        self.ruleset.as_deref().unwrap_or(content::DEFAULT_RULESET)
    }

    /// The chosen speed, validated against the ruleset.
    pub fn speed_in(&self, r: &Ruleset) -> Result<Id<Speed>, String> {
        let name = self
            .speed
            .as_deref()
            .unwrap_or_else(|| content::default_speed(self.ruleset_name()));
        r.speeds.id(name).ok_or_else(|| format!("unknown speed: {name}"))
    }

    /// The explicit `maxTurns`, or else the chosen speed's length.
    pub fn turn_limit(&self, r: &Ruleset) -> Result<u32, String> {
        let speed = self.speed_in(r)?;
        Ok(self.max_turns.unwrap_or_else(|| r.speeds[speed].total_turns.max(1) as u32))
    }
}

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

/// Whether the civ in seat `seat` is led by its nation's second leader.
///
/// The local player (seat 0) leads with `Settings::leader`; everyone else
/// draws by seed, so a seed always deals the same leaders.
fn choose_alt_leader(
    rules: &Ruleset,
    nation: Id<civ_core::ruleset::Nation>,
    seat: usize,
    s: &Settings,
) -> Result<bool, String> {
    let n = &rules.nations[nation];
    let (chosen_nation, chosen_leader) = s.choice(seat);
    let Some(alt) = &n.alt_leader else {
        return match chosen_leader {
            Some(name) if n.leader_name.as_deref() != Some(name) => Err("unknown leader for this nation".into()),
            _ => Ok(false),
        };
    };
    if let Some(name) = chosen_leader {
        return if name == alt.leader_name {
            Ok(true)
        } else if n.leader_name.as_deref() == Some(name) {
            Ok(false)
        } else {
            Err("unknown leader for this nation".into())
        };
    }
    if chosen_nation.is_some() {
        return Ok(false);
    }
    Ok(seeded_order(&[false, true], s.seed ^ (seat as u64 + 1).wrapping_mul(0x2545_F491_4F6C_DD1D))[0])
}

pub fn new_game(s: &Settings) -> Result<(World, Ruleset, CivId), String> {
    new_game_for(s, &[])
}

/// [`new_game`], knowing which major civilizations humans will play: they
/// start with the difficulty's `playerBonusStartingUnits`, the other majors
/// with its `aiMajorCivBonusStartingUnits` (GameStarter.kt:454).
pub fn new_game_for(s: &Settings, humans: &[u8]) -> Result<(World, Ruleset, CivId), String> {
    validate_settings(s)?;
    new_game_in(s, humans, content::load_ruleset(s.ruleset_name())?)
}

/// [`new_game_for`] with rules the caller loaded, for trusted tools such as
/// the balance runner trying out a ruleset before it is bundled.
pub fn new_game_in(s: &Settings, humans: &[u8], rules: Ruleset) -> Result<(World, Ruleset, CivId), String> {
    validate_settings(s)?;
    let wanted = usize::from(s.majors) + usize::from(s.city_states);

    let majors: Vec<_> = rules
        .nations
        .iter()
        .filter(|(_, n)| n.is_major() && n.in_waves(rules.max_wave))
        .map(|(id, _)| id)
        .collect();
    let mut pool = seeded_order(&majors, s.seed);
    // Seats that chose a nation take it; the rest are dealt in seeded order.
    let mut chosen: Vec<Option<Id<civ_core::ruleset::Nation>>> = Vec::new();
    for seat in 0..usize::from(s.majors) {
        chosen.push(match s.choice(seat).0 {
            Some(name) => {
                let pos = pool
                    .iter()
                    .position(|id| rules.nations[*id].name == name)
                    .ok_or_else(|| "unknown playable nation, or chosen by two seats".to_string())?;
                Some(pool.remove(pos))
            }
            None => None,
        });
    }
    let mut rest = pool.into_iter();
    let mut majors: Vec<_> = chosen.into_iter().filter_map(|c| c.or_else(|| rest.next())).collect();
    majors.extend(rest);
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
        return Err("the requested nations do not fit this map or ruleset".into());
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
        if i < n_majors {
            civ.alt_leader = choose_alt_leader(&rules, nation, i, s)?;
        }
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
    let slice = s
        .slice
        .as_deref()
        .map(|name| civ_core::world::slices::resolve(&rules, name))
        .transpose()?;
    let era = match &slice {
        Some(slice) => rules
            .eras
            .values()
            .find(|e| e.era_number == slice.first_era)
            .ok_or("missing starting era")?,
        None => rules.first_era().ok_or("missing starting era")?,
    };
    let (worker, warrior) = (unit("Worker"), unit(&era.starting_military_unit));

    let count = civs.len();
    let barbarian = CivId(count as u8);
    let barbarian_nation = rules
        .barbarian_nation()
        .ok_or("ruleset has no barbarians")?;
    let mut barbarians = Civilization::new(barbarian, barbarian_nation, true);
    barbarians.is_barbarian = true;
    civs.push(barbarians);
    let mut world = World::new(map.grid, civs);
    world.difficulty = rules.default_difficulty_id();
    world.speed = Some(s.speed_in(&rules)?);
    world.endgame.max_turns = s.turn_limit(&rules)?;
    world.endgame.enabled_victories = enabled_victories;
    if let Some(slice) = &slice {
        let turns = rules.slices.iter().find(|d| d.name == slice.name).map_or(60, |d| d.turns);
        world.endgame.max_turns = s.max_turns.unwrap_or(turns);
        // Only victories that can be won within the slice's eras.
        let within: std::collections::BTreeSet<_> =
            civ_core::world::slices::victories_within(&rules, slice).into_iter().collect();
        world.endgame.enabled_victories = Some(match world.endgame.enabled_victories.take() {
            Some(chosen) => chosen.intersection(&within).copied().collect(),
            None => within,
        });
        for c in world.civs.iter_mut() {
            c.tech_era_cap = Some(slice.last_era);
            if c.is_major() {
                c.gold = c.gold.saturating_add(era.starting_gold.unwrap_or(0));
                c.add_culture(era.starting_culture.unwrap_or(0));
            }
        }
        world.slice = Some(slice.clone());
    }
    world.diplomacy.mark_hostile(barbarian);
    for i in 0..count {
        let civ = CivId(i as u8);
        let starting: Vec<_> = rules
            .technologies
            .iter()
            .filter(|(_, t)| {
                t.uniques
                    .iter()
                    .any(|u| u.unique_type() == Some(civ_core::UniqueType::StartingTech))
            })
            .map(|(id, _)| id)
            // A slice starts with every tech of the eras before it.
            .chain(
                rules
                    .technologies
                    .iter()
                    .filter(|(_, t)| {
                        slice.as_ref().is_some_and(|slice| {
                            t.era(&rules)
                                .and_then(|e| rules.eras.get(e))
                                .is_some_and(|e| e.era_number < slice.first_era)
                        })
                    })
                    .map(|(id, _)| id),
            )
            .chain(
                world
                    .civ(civ)
                    .expect("created civ")
                    .leader_uniques(&rules)
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
    let difficulty = world.game_difficulty(&rules);
    for (i, start) in map.starts.iter().take(count).enumerate() {
        let kit: Vec<_> = if i < n_majors {
            let bonus = difficulty.map_or(&[][..], |d| {
                if humans.contains(&(i as u8)) {
                    &d.player_bonus_starting_units[..]
                } else {
                    &d.ai_major_civ_bonus_starting_units[..]
                }
            });
            std::iter::repeat_n(settler, era.starting_settler_count.max(0) as usize)
                .chain(std::iter::repeat_n(
                    worker,
                    era.starting_worker_count.max(0) as usize,
                ))
                .chain(std::iter::repeat_n(
                    warrior,
                    era.starting_military_unit_count.max(0) as usize,
                ))
                // `Constants.eraSpecificUnit` stands for the era's military unit.
                .chain(bonus.iter().map(|name| {
                    if name == "Era Starting Unit" {
                        warrior
                    } else {
                        unit(name)
                    }
                }))
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
        return Err("choose 1 to 16 nations and at most 16 city-states".into());
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
    if !content::RULESETS.contains(&s.ruleset_name()) {
        return Err(format!("unknown ruleset: {}", s.ruleset_name()));
    }
    if s.speed.as_ref().is_some_and(|n| n.len() > 100) {
        return Err("speed name is too long".into());
    }
    if s.nation.as_ref().is_some_and(|n| n.len() > 100) {
        return Err("nation name is too long".into());
    }
    if s.leader.as_ref().is_some_and(|n| n.len() > 100) {
        return Err("leader name is too long".into());
    }
    if s.slice.as_ref().is_some_and(|n| n.len() > 100) {
        return Err("slice name is too long".into());
    }
    if let Some(seats) = &s.seats {
        let mut seen = std::collections::BTreeSet::new();
        if seats.len() > 16
            || seats.iter().any(|c| {
                c.seat >= s.majors
                    || !seen.insert(c.seat)
                    || c.nation.as_ref().is_some_and(|n| n.len() > 100)
                    || c.leader.as_ref().is_some_and(|n| n.len() > 100)
            })
        {
            return Err("seat choices must name distinct seats of this game".into());
        }
    }
    Ok(())
}
