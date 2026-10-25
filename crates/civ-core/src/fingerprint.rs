// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! One number that summarises a whole simulation.
//!
//! The crate's central claim is that it computes the same answers on a native
//! server and in a browser. That claim is worth nothing unverified, and
//! comparing two engines field by field is impractical, so instead a fixed
//! scenario runs every layer — parsing, fixed-point arithmetic, powers,
//! modifier composition, yields, growth, movement, combat, the generator — and
//! folds every observable into a single `u64`.
//!
//! Native and `wasm32` must produce the same value. If one bit of one
//! calculation differs anywhere, this number differs, and the difference is
//! caught by a build rather than by a desynced multiplayer game forty turns in.
//!
//! The ruleset is embedded rather than read from disk, because `wasm32` has no
//! filesystem — and because that makes the parser part of what is checked.

use crate::json::Json;
use crate::modifiers::{Modifier, ModifierStack, Source, SourceKind};
use crate::rng::Rng;
use crate::ruleset::Ruleset;
use crate::stats::{Fixed, Stat};
use crate::world::city::{apply_food, food_to_next_population, City, GrowthEvent, GrowthModifiers};
use crate::world::combat::{damage_modifier, health_ratio};
use crate::world::grid::{CivId, TileGrid, TileId};
use crate::world::movement::reachable;
use crate::world::yields::tile_yield;

const RULESET: &str = "rules/gods-and-kings/";

#[cfg(unciv_rulesets)]
macro_rules! embed {
    ($($f:literal),* $(,)?) => {
        &[$(($f, include_str!(concat!("../rules/gods-and-kings/", $f)))),*]
    };
}

/// The ruleset files the scenario needs, carried into the binary.
///
/// They are Unciv's Gods & Kings files, fetched by
/// `tools/fetch_unciv_rulesets.sh` rather than kept in the repository; the
/// build script enables `cfg(unciv_rulesets)` when they are present.
#[cfg(unciv_rulesets)]
pub fn embedded_sources() -> Vec<(String, String)> {
    let files: &[(&str, &str)] = embed![
        "Buildings.json",
        "Units.json",
        "UnitTypes.json",
        "Terrains.json",
        "TileResources.json",
        "TileImprovements.json",
        "Techs.json",
        "Policies.json",
        "Eras.json",
        "Speeds.json",
        "Nations.json",
        "Ruins.json",
        "Beliefs.json",
        "Religions.json",
        "UnitPromotions.json",
    ];
    files.iter().map(|(n, c)| (n.to_string(), c.to_string())).collect()
}

/// Built without the Unciv rulesets: there is nothing to embed.
#[cfg(not(unciv_rulesets))]
pub fn embedded_sources() -> Vec<(String, String)> {
    panic!(
        "civ-core was built without rules/gods-and-kings; run \
         crates/civ-core/tools/fetch_unciv_rulesets.sh and rebuild"
    )
}

/// Accumulates values into one number, order-sensitively.
struct Digest(u64);

impl Digest {
    fn new() -> Digest {
        Digest(0x243F_6A88_85A3_08D3)
    }

    fn add(&mut self, v: i64) -> &mut Self {
        self.0 = self.0.wrapping_add(v as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15);
        self.0 ^= self.0 >> 29;
        self
    }

    fn add_fixed(&mut self, v: Fixed) -> &mut Self {
        self.add(v.raw())
    }

    fn add_pantheons(&mut self, world: &crate::world::world::World) {
        for civ in &world.civs {
            self.add(civ.faith.pantheon_beliefs.len() as i64);
            for belief in &civ.faith.pantheon_beliefs {
                self.add(belief.index() as i64).add(belief.generation() as i64);
            }
        }
        for (_, city) in world.cities.iter() {
            self.add(city.faith.pantheon_pressures().count() as i64);
            for (creed, pressure) in city.faith.pantheon_pressures() {
                self.add(creed.0 as i64).add(pressure as i64);
            }
            self.add(city.faith.pantheon_followers().count() as i64);
            for (creed, followers) in city.faith.pantheon_followers() {
                self.add(creed.0 as i64).add(followers as i64);
            }
        }
    }

    fn add_observations(&mut self, world: &crate::world::world::World, viewer: CivId) {
        self.add(viewer.0 as i64);
        for tile in world.grid.ids() {
            self.add(world.visibility.has_explored(viewer, tile) as i64)
                .add(world.visibility.can_see(viewer, tile) as i64);
            match world.visibility.recalls(viewer, tile) {
                None => {
                    self.add(0);
                }
                Some(seen) => {
                    self.add(1)
                        .add(seen.improvement.map_or(-1, |id| id.index() as i64))
                        .add(seen.resource.map_or(-1, |id| id.index() as i64))
                        .add(seen.owner.map_or(-1, |c| c.0 as i64));
                }
            }
        }
        self.add(world.visibility.known_tiles(viewer).count() as i64);
        for (tile, seen) in world.visibility.known_tiles(viewer) {
            self.add(tile.0 as i64).add(seen.terrain.index() as i64).add(seen.features.len() as i64);
            for feature in &seen.features {
                self.add(feature.index() as i64);
            }
            self.add(seen.natural_wonder.map_or(-1, |id| id.index() as i64))
                .add(seen.recorded_resource().map_or(-1, |id| id.index() as i64))
                .add(seen.improvement.map_or(-1, |id| id.index() as i64))
                .add(seen.improvement_pillaged as i64)
                .add(seen.road as i64)
                .add(seen.road_pillaged as i64)
                .add(seen.river_edges as i64)
                .add(seen.owner.map_or(-1, |c| c.0 as i64))
                .add(seen.observed_turn as i64);
        }
        self.add(world.visibility.known_cities(viewer).count() as i64);
        for seen in world.visibility.known_cities(viewer) {
            self.add(seen.id.index() as i64)
                .add(seen.id.generation() as i64)
                .add_text(&seen.name)
                .add(seen.owner.0 as i64)
                .add(seen.centre.0 as i64)
                .add(seen.population as i64)
                .add(seen.tiles.len() as i64);
            for tile in &seen.tiles {
                self.add(tile.0 as i64);
            }
            self.add(seen.observed_turn as i64);
        }
        self.add(world.visibility.known_civs(viewer).count() as i64);
        for seen in world.visibility.known_civs(viewer) {
            self.add(seen.id.0 as i64)
                .add(seen.nation.index() as i64)
                .add(seen.city_state as i64)
                .add(seen.barbarian as i64)
                .add(seen.alive as i64)
                .add(seen.observed_turn as i64);
        }
    }

    fn add_text(&mut self, text: &str) -> &mut Self {
        self.add(text.len() as i64);
        for byte in text.bytes() {
            self.add(byte as i64);
        }
        self
    }
}

/// Run the scenario and fold everything it produces into one number.
pub fn fingerprint() -> u64 {
    let mut d = Digest::new();
    let _ = RULESET; // documented above; the macro carries the real path

    // --- parsing ----------------------------------------------------------
    let sources = embedded_sources();
    let Ok(r) = Ruleset::from_sources(&sources) else { return 0 };
    d.add(r.object_count() as i64)
        .add(r.buildings.len() as i64)
        .add(r.technologies.len() as i64)
        .add(r.terrains.len() as i64);

    // Number parsing has to be exact, not merely close.
    if let Ok(v) = Json::parse(r#"{"a": 0.95, "b": 1.4813, "c": -2.5}"#) {
        for k in ["a", "b", "c"] {
            d.add_fixed(v.get(k).and_then(|x| x.as_fixed()).unwrap_or(Fixed::ZERO));
        }
    }

    // --- derived relationships -------------------------------------------
    for t in r.technologies.ids() {
        d.add(r.technologies[t].cost.unwrap_or(0) as i64);
    }
    for b in r.buildings.ids() {
        d.add(r.buildings[b].cost.unwrap_or(0) as i64);
    }

    // --- fixed-point powers ----------------------------------------------
    for n in 1..40i32 {
        let x = Fixed::whole(n);
        d.add_fixed(x.pow(Fixed::permille(1_500)).unwrap_or(Fixed::ZERO));
        d.add_fixed(x.pow(Fixed::permille(1_300)).unwrap_or(Fixed::ZERO));
        d.add_fixed(x.sqrt().unwrap_or(Fixed::ZERO));
        d.add_fixed(x.log2().unwrap_or(Fixed::ZERO));
    }

    // --- modifier composition --------------------------------------------
    let mut stack = ModifierStack::new();
    for (i, layer) in [1i32, 2, 3, 5, 7, 11].into_iter().enumerate() {
        let src = Source::new(SourceKind::Building, "m");
        let amount = Fixed::whole(layer);
        stack.add(match i % 3 {
            0 => Modifier::base(amount, src),
            1 => Modifier::additive_percent(amount, src),
            _ => Modifier::multiplicative_percent(amount, src),
        });
    }
    for start in 0..20i32 {
        d.add_fixed(stack.evaluate(Fixed::whole(start)));
    }

    // --- growth curve -----------------------------------------------------
    let g = GrowthModifiers::default();
    for pop in 1..40u32 {
        d.add(food_to_next_population(pop, g) as i64);
    }

    // --- the map, yields and movement -------------------------------------
    let Some(grassland) = r.terrain("Grassland") else { return d.0 };
    let mut grid = TileGrid::new(40, 40, grassland);
    if let (Some(forest), Some(hill), Some(marsh)) = (r.terrain("Forest"), r.terrain("Hill"), r.terrain("Marsh")) {
        for t in grid.ids() {
            match t.0 % 4 {
                1 => grid.features[t.0 as usize].push(forest),
                2 => grid.features[t.0 as usize].push(hill),
                3 => grid.features[t.0 as usize].push(marsh),
                _ => {}
            }
        }
    }
    for t in grid.ids().step_by(7) {
        let y = tile_yield(&grid, &r, t);
        for s in Stat::ALL {
            d.add_fixed(y[s]);
        }
    }

    let start = grid.id_at(20, 20).unwrap_or(TileId(0));
    for budget in 1..6i32 {
        let set = reachable(&grid, &r, start, Fixed::whole(budget), &[]);
        d.add(set.len() as i64);
        for (tile, cost) in set {
            d.add(tile.0 as i64).add_fixed(cost);
        }
    }

    // --- a city living for a while ----------------------------------------
    let mut city = City::found("Fingerprint", CivId(0), start);
    for turn in 0..60i32 {
        let food = (turn % 7) - 2; // feast and famine
        d.add(match apply_food(&mut city, food, g) {
            GrowthEvent::Grew { population, carried_over } => population as i64 * 1_000 + carried_over as i64,
            GrowthEvent::Starved { population } => -(population as i64),
            GrowthEvent::Stored { food } => food as i64,
            GrowthEvent::Held { food } => food as i64 * 7,
        });
    }

    // --- combat -----------------------------------------------------------
    for att in 1..12i32 {
        for def in 1..12i32 {
            let ratio = Fixed::whole(att).checked_div(Fixed::whole(def)).unwrap_or(Fixed::ONE);
            for roll in [0i64, 250, 500, 750, 999] {
                let rnd = Fixed::permille(roll);
                d.add_fixed(damage_modifier(ratio, false, rnd));
                d.add_fixed(damage_modifier(ratio, true, rnd));
            }
        }
    }
    for health in 1..=100u8 {
        d.add_fixed(health_ratio(health));
    }

    // --- the generator ----------------------------------------------------
    let mut rng = Rng::from_parts(&[17, 4_000, 9]);
    for _ in 0..200 {
        d.add(rng.next_u64() as i64);
        d.add_fixed(rng.unit_interval());
    }

    // --- map generation ---------------------------------------------------
    //
    // The system most at risk here, and the one with the least to fall back
    // on: a map that differs by one tile between two clients is a game they
    // cannot play together, and no differential test against Unciv can catch
    // it because Unciv generates a different map on purpose. Folding a whole
    // generated map in means the existing cross-target check covers it.
    {
        use crate::world::mapgen::{generate, MapParameters};
        let map = generate(
            &r,
            MapParameters { width: 24, height: 16, seed: 0x1234_5678, wrap_x: true, ..Default::default() },
            4,
        );
        d.add(map.grid.wraps_x() as i64);
        for tile in map.grid.ids() {
            let i = tile.0 as usize;
            d.add(map.grid.terrain[i].index() as i64);
            for f in &map.grid.features[i] {
                d.add(f.index() as i64);
            }
            d.add(map.grid.resource[i].map_or(-1, |x| x.index() as i64));
            d.add(map.grid.resource_amount[i] as i64);
            // Improvements and roads were missing here, which meant barbarian
            // encampments — placed by generation — were invisible to this
            // check. Adding camps to the generator left the fingerprint
            // completely unchanged, which is how the gap was found.
            d.add(map.grid.improvement[i].map_or(-1, |x| x.index() as i64));
            d.add(map.grid.natural_wonder[i].map_or(-1, |x| x.index() as i64));
            d.add(map.grid.road[i] as i64);
            d.add(map.grid.road_pillaged[i] as i64);
            d.add(map.grid.river_edges[i] as i64);
        }
        for start in &map.starts {
            d.add(start.0 as i64);
        }
    }

    // --- a whole game ------------------------------------------------------
    //
    // Everything above checks a layer in isolation. This runs the engine:
    // forty turns of four civs on a generated map, folding in every civ's
    // holdings and every city's state at the end.
    //
    // It is the only check that covers the systems whose behaviour depends on
    // state rather than on the ruleset — combat, religion's spread, city-state
    // influence, espionage's rolls — and the only one that would catch two
    // machines diverging on the third turn of an actual game rather than on
    // an arithmetic edge case.
    {
        use crate::world::assignment::CityFocus;
        use crate::world::entities::{Civilization, Unit};
        use crate::world::mapgen::{generate, MapParameters};
        use crate::world::turn::{advance_turn, TurnModifiers};
        use crate::world::world::World;

        let map = generate(&r, MapParameters { width: 24, height: 16, seed: 0x1234_5678, ..Default::default() }, 6);
        if let Some(nation) = r.nations.ids().next() {
            // Four majors and two city-states. The city-states are what make
            // quests, influence, elections and coups reachable at all, and
            // those are the systems that actually roll dice — an election is a
            // weighted draw and a coup is a percentage. A scenario without
            // them leaves the engine's randomness untested across targets.
            let cs_types: Vec<_> = r.city_state_types.ids().collect();
            let civs: Vec<_> = (0..6u8)
                .map(|i| {
                    let mut c = Civilization::new(CivId(i), nation, true);
                    if i >= 4 && !cs_types.is_empty() {
                        c.city_state_type = Some(cs_types[(i as usize - 4) % cs_types.len()]);
                    }
                    c
                })
                .collect();
            let mut w = World::new(map.grid, civs);
            let settler = r.units.iter().find(|(id, _)| crate::world::settling::is_settler(&r, *id)).map(|(id, _)| id);
            let worker = r.units.iter().find(|(_, u)| u.name == "Worker").map(|(id, _)| id);
            for (i, start) in map.starts.iter().enumerate() {
                for base in [settler, worker].into_iter().flatten() {
                    w.place_unit(Unit::new(base, CivId(i as u8), *start, 2));
                }
            }
            // Sixty turns rather than forty: quests are not offered before
            // turn thirty and elections are held every fifteen, so a shorter
            // game would fold in nothing from either.
            for _ in 0..60 {
                for event in advance_turn(&mut w, &r, CityFocus::None, TurnModifiers::default()) {
                    match event {
                        crate::world::turn::TurnEvent::RuinsExplored { civ, tile, reward } => {
                            d.add(1).add(civ.0 as i64).add(tile.0 as i64).add(reward.index() as i64);
                        }
                        crate::world::turn::TurnEvent::NaturalWonderDiscovered(found) => {
                            d.add(2).add(found.civ.0 as i64).add(found.wonder.index() as i64).add(found.first as i64);
                        }
                        _ => {}
                    }
                }
            }

            d.add(w.turn as i64).add(w.cities.len() as i64).add(w.units.len() as i64);
            d.add(w.difficulty.map_or(-1, |id| id.index() as i64))
                .add(w.difficulty.map_or(0, |id| id.generation() as i64));
            if let Some((civ, victory)) = w.winner {
                d.add(1).add(civ.0 as i64).add(victory.index() as i64).add(victory.generation() as i64);
            } else {
                d.add(0);
            }
            d.add(w.victory_turn.map_or(-1, i64::from));
            d.add(w.endgame.max_turns as i64);
            d.add(w.endgame.enabled_victories.as_ref().map_or(-1, |v| v.len() as i64));
            if let Some(enabled) = &w.endgame.enabled_victories {
                for id in enabled {
                    d.add(id.index() as i64).add(id.generation() as i64);
                }
            }
            d.add(w.endgame.voting.is_some() as i64);
            if let Some(vote) = &w.endgame.voting {
                d.add(vote.interval as i64).add(vote.opens_on.map_or(-1, i64::from));
                d.add(vote.ballots.len() as i64);
                for (voter, target) in &vote.ballots {
                    d.add(voter.0 as i64).add(target.map_or(-1, |c| c.0 as i64));
                }
                d.add(vote.last_result.is_some() as i64);
                if let Some(result) = &vote.last_result {
                    d.add(result.turn as i64).add(result.needed as i64).add(result.winner.map_or(-1, |c| c.0 as i64));
                    d.add(result.votes.len() as i64);
                    for (civ, n) in &result.votes {
                        d.add(civ.0 as i64).add(*n as i64);
                    }
                }
            }
            d.add_pantheons(&w);
            d.add(w.barbarian_camps.len() as i64);
            for (tile, camp) in &w.barbarian_camps {
                d.add(tile.0 as i64)
                    .add(camp.countdown as i64)
                    .add(camp.spawned_units as i64)
                    .add(camp.destroyed as i64);
            }
            for civ in &w.civs {
                let cs = &civ.city_state;
                d.add(cs.initialized as i64)
                    .add(cs.personality as i64)
                    .add(cs.absorbed as i64)
                    .add(cs.minor_attacks as i64)
                    .add(cs.tracked_ally.map_or(-1, |c| c.0 as i64))
                    .add(cs.allied_turns as i64)
                    .add(cs.great_person_gift.map_or(-1, i64::from));
                for id in [
                    cs.unique_luxury.map(|id| (id.index(), id.generation())),
                    cs.unique_unit.map(|id| (id.index(), id.generation())),
                ] {
                    d.add(id.map_or(-1, |v| v.0 as i64)).add(id.map_or(-1, |v| v.1 as i64));
                }
                d.add(cs.wary_until.len() as i64);
                for (c, n) in &cs.wary_until {
                    d.add(c.0 as i64).add(*n as i64);
                }
                d.add(cs.military_gifts.len() as i64);
                for (c, n) in &cs.military_gifts {
                    d.add(c.0 as i64).add(*n as i64);
                }
                d.add(civ.revolt_countdown.map_or(-1, i64::from))
                    .add(civ.gold as i64)
                    .add(civ.has_ever_owned_original_capital as i64)
                    .add(civ.defeated as i64)
                    .add(civ.science_stored as i64)
                    .add(civ.science_from_agreements as i64)
                    .add(civ.culture_stored as i64)
                    .add(civ.culture_for_contests as i64)
                    .add(civ.faith_for_contests as i64)
                    .add(civ.techs.len() as i64)
                    .add(civ.adopted_policies.len() as i64)
                    .add(civ.faith.stored as i64)
                    .add(civ.faith.founded as i64)
                    .add(civ.faith.enhanced as i64)
                    .add(civ.faith.prophets_earned as i64)
                    .add(civ.cities.len() as i64)
                    .add(civ.units.len() as i64)
                    .add(civ.espionage.spies.len() as i64)
                    .add(civ.quests.active.len() as i64)
                    .add(civ.quests.next_turn as i64)
                    .add(civ.defeated as i64);
                for pool in [
                    &civ.great_people.points,
                    &civ.great_people.thresholds,
                    &civ.great_people.combat_points,
                    &civ.great_people.combat_thresholds,
                ] {
                    d.add(pool.len() as i64);
                    for (kind, n) in pool {
                        d.add_text(kind);
                        d.add(*n as i64);
                    }
                }
                d.add(civ.great_people.maya_restricted as i64)
                    .add(civ.great_people.maya_last_award_turn.map_or(-1, i64::from))
                    .add(civ.great_people.maya_available.len() as i64);
                for id in &civ.great_people.maya_available {
                    d.add(id.index() as i64).add(id.generation() as i64);
                }
                d.add(civ.paid_policies as i64)
                    .add(civ.pending_rewards.free_techs as i64)
                    .add(civ.pending_rewards.free_policies as i64)
                    .add(civ.pending_rewards.free_great_people as i64);
                d.add(civ.free_building_grants.len() as i64);
                for (key, cities) in &civ.free_building_grants {
                    d.add_text(key);
                    d.add(cities.len() as i64);
                    for city in cities {
                        d.add(city.index() as i64).add(city.generation() as i64);
                    }
                }
                for spy in &civ.espionage.spies {
                    d.add(spy.rank as i64)
                        .add(spy.action as i64)
                        .add(spy.turns_remaining as i64)
                        .add(spy.progress as i64);
                    d.add(spy.station.map_or(-1, |id| id.index() as i64))
                        .add(spy.station.map_or(-1, |id| id.generation() as i64));
                }
                d.add(civ.espionage.eras_rewarded.len() as i64);
                for era in &civ.espionage.eras_rewarded {
                    d.add(*era as i64);
                }
                d.add(civ.science_invested.len() as i64);
                for (tech, work) in &civ.science_invested {
                    d.add(tech.index() as i64).add(tech.generation() as i64).add(*work as i64);
                }
                d.add(civ.victory.diplomatic_win as i64).add(civ.victory.repeated_techs as i64);
                d.add(civ.victory.spaceship_parts.len() as i64);
                for (part, n) in &civ.victory.spaceship_parts {
                    d.add(part.index() as i64).add(part.generation() as i64).add(*n as i64);
                }
                for reward in &civ.recent_ruins {
                    d.add_text(reward);
                }
                for wonder in &civ.known_natural_wonders {
                    d.add(wonder.index() as i64);
                }
                d.add_observations(&w, civ.id);
                for other in w.civ_ids().filter(|id| *id > civ.id) {
                    if let Some(pact) = w.diplomacy.pact(civ.id, other) {
                        d.add(other.0 as i64)
                            .add(pact.embassies[0] as i64)
                            .add(pact.embassies[1] as i64)
                            .add(pact.friendship_turns as i64)
                            .add(pact.peace_treaty_turns as i64)
                            .add(pact.defensive_pact as i64)
                            .add(pact.defensive_pact_turns as i64);
                        d.add(pact.research.is_some() as i64);
                        if let Some(ra) = &pact.research {
                            d.add(ra.turns_left as i64).add(ra.science[0] as i64).add(ra.science[1] as i64);
                        }
                    }
                }
                // Influence is directed and lives outside the civ, so it has
                // to be folded in by hand or the whole city-state layer is
                // invisible to this check.
                for other in 0..6u8 {
                    d.add_fixed(w.diplomacy.influence(civ.id, CivId(other)));
                }
                d.add(civ.quests.global_seeded as i64).add(civ.quests.individual_next.len() as i64);
                for (id, n) in &civ.quests.individual_next {
                    d.add(id.0 as i64).add(*n as i64);
                }
                d.add(civ.quests.assistance.len() as i64);
                for (id, q) in &civ.quests.assistance {
                    d.add(id.0 as i64).add(q.needed as i64).add(q.kills.len() as i64);
                    for (who, n) in &q.kills {
                        d.add(who.0 as i64).add(*n as i64);
                    }
                }
                for q in &civ.quests.active {
                    d.add(q.assignee.0 as i64)
                        .add(q.assigned_on as i64)
                        .add(q.expires_on as i64)
                        .add(q.baseline as i64);
                }
            }
            d.add_pantheons(&w);
            d.add(w.barbarian_camps.len() as i64);
            for (tile, camp) in &w.barbarian_camps {
                d.add(tile.0 as i64)
                    .add(camp.countdown as i64)
                    .add(camp.spawned_units as i64)
                    .add(camp.destroyed as i64);
            }
            for civ in &w.civs {
                for (policy, expiry) in &civ.timed_policy_expirations {
                    d.add(policy.index() as i64).add(*expiry as i64);
                }
                for base in &civ.units_gained {
                    d.add(base.index() as i64).add(base.generation() as i64);
                }
            }
            for (_, city) in w.cities.iter() {
                d.add(city.founder.0 as i64)
                    .add(city.original_capital as i64)
                    .add(city.previous_owner.map_or(-1, |c| c.0 as i64));
                d.add(city.population as i64)
                    .add(city.food_stored as i64)
                    .add(city.production_overflow as i64)
                    .add(city.health as i64)
                    .add(city.attacks_left as i64)
                    .add(city.sold_building_this_turn as i64)
                    .add(city.production_conversion.map_or(-1, |s| s as i64))
                    .add(city.built.len() as i64)
                    .add(city.centre.0 as i64)
                    .add(city.faith.total_pressure() as i64);
                d.add(city.free_buildings.len() as i64);
                for building in &city.free_buildings {
                    d.add(building.index() as i64).add(building.generation() as i64);
                }
                d.add(city.queue.len() as i64);
                for what in &city.queue {
                    match what {
                        crate::world::construction::Construction::Building(id) => {
                            d.add(0).add(id.index() as i64).add(id.generation() as i64);
                        }
                        crate::world::construction::Construction::Unit(id) => {
                            d.add(1).add(id.index() as i64).add(id.generation() as i64);
                        }
                    }
                }
                d.add(city.production_progress.len() as i64);
                for (what, stored) in &city.production_progress {
                    match what {
                        crate::world::construction::Construction::Building(id) => {
                            d.add(0).add(id.index() as i64).add(id.generation() as i64);
                        }
                        crate::world::construction::Construction::Unit(id) => {
                            d.add(1).add(id.index() as i64).add(id.generation() as i64);
                        }
                    }
                    d.add(*stored as i64);
                }
                d.add(city.faith.holy_city_blocked as i64);
                d.add(city.faith.adopted_religions.len() as i64);
                for creed in &city.faith.adopted_religions {
                    d.add(creed.0 as i64);
                }
            }
            for (_, unit) in w.units.iter() {
                d.add(unit.position.0 as i64).add(unit.health as i64);
                d.add(unit.base.index() as i64).add(unit.xp as i64);
                d.add(unit.promotions_earned as i64)
                    .add(unit.set_up as i64)
                    .add(unit.fortified as i64)
                    .add(unit.fortification_turns as i64);
                d.add(unit.owner.0 as i64).add(unit.attacks_left as i64);
                d.add(unit.promotions.len() as i64);
                for promotion in &unit.promotions {
                    d.add(promotion.index() as i64).add(promotion.generation() as i64);
                }
                d.add(unit.religion.map_or(-1, |c| c.0 as i64))
                    .add(unit.religious_strength as i64)
                    .add(unit.religious_uses_left as i64);
                d.add_fixed(unit.movement_left);
                d.add(unit.moved_this_turn as i64).add(unit.attacked_this_turn as i64);
                d.add(unit.attacks_this_turn as i64);
                d.add(unit.transport.map_or(-1, |id| id.index() as i64))
                    .add(unit.transport.map_or(-1, |id| id.generation() as i64));
            }
        }
    }

    // Religion needs faith income that the short general game may never
    // produce. A funded two-city scenario exercises physical prophets,
    // enhancement, faith purchases and missionary travel on every target.
    {
        use crate::world::assignment::CityFocus;
        use crate::world::entities::Civilization;
        use crate::world::religious_units::ReligiousEvent;
        use crate::world::turn::{advance_turn, update_visibility, TurnEvent, TurnModifiers};
        use crate::world::world::World;
        if let (Some(grass), Some(nation)) = (r.terrain("Grassland"), r.nations.id("Rome")) {
            let mut w = World::new(TileGrid::new(22, 10, grass), vec![Civilization::new(CivId(0), nation, true)]);
            for (name, col) in [("Home", 3), ("Distant", 15)] {
                let centre = w.grid.id_at(col, 5).unwrap();
                let city = w.found_city_in(&r, City::found(name, CivId(0), centre));
                for tile in w.grid.within(centre, 2) {
                    w.claim_tile(city, tile);
                }
            }
            w.civ_mut(CivId(0)).unwrap().faith.stored = 1500;
            update_visibility(&mut w);
            for _ in 0..25 {
                for event in advance_turn(&mut w, &r, CityFocus::None, TurnModifiers::default()) {
                    match event {
                        TurnEvent::ProphetBorn { unit, .. } => {
                            d.add(3).add(unit.index() as i64);
                        }
                        TurnEvent::ReligionFounded { .. } => {
                            d.add(4);
                        }
                        TurnEvent::ReligionEnhanced { .. } => {
                            d.add(5);
                        }
                        TurnEvent::FaithPurchased { faith, .. } => {
                            d.add(6).add(faith);
                        }
                        TurnEvent::ReligiousUnitActed {
                            event: ReligiousEvent::Spread { city, pressure, uses_left, .. },
                            ..
                        } => {
                            d.add(7).add(city.index() as i64).add(pressure as i64).add(uses_left as i64);
                        }
                        _ => {}
                    }
                }
            }
            let faith = &w.civ(CivId(0)).unwrap().faith;
            d.add(faith.stored as i64).add(faith.founded as i64).add(faith.enhanced as i64);
            for belief in &faith.beliefs {
                d.add(belief.index() as i64);
            }
            d.add_pantheons(&w);
            d.add(w.barbarian_camps.len() as i64);
            for (tile, camp) in &w.barbarian_camps {
                d.add(tile.0 as i64)
                    .add(camp.countdown as i64)
                    .add(camp.spawned_units as i64)
                    .add(camp.destroyed as i64);
            }
            for civ in &w.civs {
                for (policy, expiry) in &civ.timed_policy_expirations {
                    d.add(policy.index() as i64).add(*expiry as i64);
                }
                for base in &civ.units_gained {
                    d.add(base.index() as i64).add(base.generation() as i64);
                }
            }
            for (_, city) in w.cities.iter() {
                for (creed, pressure) in city.faith.pressures() {
                    d.add(creed.map_or(-1, |c| c.0 as i64)).add(pressure as i64);
                }
            }
        }
    }
    // The authoritative action window is separate from trusted all-AI play.
    // Explicit human selections survive AI rounds, and failed commands leave
    // the staged game untouched on both native and WebAssembly targets.
    {
        use crate::world::commands::{execute, Command};
        use crate::world::entities::{Civilization, Unit};
        use crate::world::turn::{finish_round, update_visibility, TurnModifiers};
        use crate::world::{CityFocus, World};
        if let (Some(grass), Some(nation), Some(warrior), Some(agriculture), Some(pottery)) = (
            r.terrain("Grassland"),
            r.nations.id("Rome"),
            r.unit("Warrior"),
            r.technologies.id("Agriculture"),
            r.technologies.id("Pottery"),
        ) {
            let mut w = World::new(
                TileGrid::new(16, 10, grass),
                vec![Civilization::new(CivId(0), nation, false), Civilization::new(CivId(1), nation, true)],
            );
            for (owner, col) in [(CivId(0), 3), (CivId(1), 12)] {
                w.civ_mut(owner).unwrap().techs.insert(agriculture);
                let centre = w.grid.id_at(col, 5).unwrap();
                w.found_city_in(&r, City::found("Window", owner, centre));
            }
            let unit = w.place_unit(Unit::new(warrior, CivId(0), w.grid.id_at(4, 5).unwrap(), 2));
            update_visibility(&mut w);
            crate::world::fog::capture_observations(&mut w, &r);
            d.add(execute(&mut w, &r, CivId(0), Command::SetResearch { technology: pottery }).is_ok() as i64);
            d.add(execute(&mut w, &r, CivId(1), Command::SkipUnit { unit }).is_err() as i64);
            d.add(execute(&mut w, &r, CivId(0), Command::Fortify { unit }).is_ok() as i64);
            for _ in 0..4 {
                finish_round(&mut w, &r, CityFocus::None, TurnModifiers::default());
            }
            d.add_pantheons(&w);
            d.add(w.barbarian_camps.len() as i64);
            for (tile, camp) in &w.barbarian_camps {
                d.add(tile.0 as i64)
                    .add(camp.countdown as i64)
                    .add(camp.spawned_units as i64)
                    .add(camp.destroyed as i64);
            }
            for civ in &w.civs {
                let cs = &civ.city_state;
                d.add(cs.initialized as i64)
                    .add(cs.personality as i64)
                    .add(cs.absorbed as i64)
                    .add(cs.minor_attacks as i64)
                    .add(cs.tracked_ally.map_or(-1, |c| c.0 as i64))
                    .add(cs.allied_turns as i64)
                    .add(cs.great_person_gift.map_or(-1, i64::from));
                for id in [
                    cs.unique_luxury.map(|id| (id.index(), id.generation())),
                    cs.unique_unit.map(|id| (id.index(), id.generation())),
                ] {
                    d.add(id.map_or(-1, |v| v.0 as i64)).add(id.map_or(-1, |v| v.1 as i64));
                }
                d.add(cs.wary_until.len() as i64);
                for (c, n) in &cs.wary_until {
                    d.add(c.0 as i64).add(*n as i64);
                }
                d.add(cs.military_gifts.len() as i64);
                for (c, n) in &cs.military_gifts {
                    d.add(c.0 as i64).add(*n as i64);
                }
                d.add(civ.revolt_countdown.map_or(-1, i64::from))
                    .add(civ.gold as i64)
                    .add(civ.science_stored as i64)
                    .add(civ.researching.map_or(-1, |id| id.index() as i64));
                d.add_observations(&w, civ.id);
            }
            d.add(w.turn as i64).add(w.units.get(unit).is_some_and(|u| u.fortified) as i64);
        }
    }
    d.0
}

/// C ABI export, for loading the wasm build directly.
///
/// # Safety
/// Takes nothing and returns a plain integer; there is nothing to misuse.
#[no_mangle]
pub extern "C" fn civ_core_fingerprint() -> u64 {
    fingerprint()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_scenario_actually_runs() {
        let f = fingerprint();
        println!("fingerprint: {f:#018x}");
        assert_ne!(f, 0, "zero means the ruleset failed to load");
        assert_ne!(f, Digest::new().0, "and this means nothing was folded in");
    }

    #[test]
    fn it_is_stable_within_a_process() {
        assert_eq!(fingerprint(), fingerprint());
    }
}
