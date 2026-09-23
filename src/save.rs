//! Versioned complete-world snapshots. Requires the optional `save` feature;
//! default rules builds remain free of runtime dependencies.
//!
//! JSON maps with structured keys are encoded as ordered entry arrays. Arenas
//! preserve vacant generations and their reuse order. The checksum detects
//! accidental corruption; it is not a signature or an authenticity guarantee.

use crate::arena::{Id, Registry};
use crate::ruleset::Ruleset;
use crate::world::grid::CivId;
use crate::world::world::World;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

pub const FORMAT_VERSION: u32 = 3;
pub const MAX_SNAPSHOT_BYTES: usize = 64 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SaveError {
    TooLarge,
    InvalidJson(String),
    UnsupportedVersion(u32),
    RulesetMismatch,
    ChecksumMismatch,
    InvalidWorld(String),
}

impl std::fmt::Display for SaveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TooLarge => write!(f, "snapshot exceeds the size limit"),
            Self::InvalidJson(e) => write!(f, "invalid snapshot JSON: {e}"),
            Self::UnsupportedVersion(v) => write!(f, "unsupported snapshot version {v}"),
            Self::RulesetMismatch => write!(f, "snapshot uses a different ruleset"),
            Self::ChecksumMismatch => write!(f, "snapshot checksum does not match"),
            Self::InvalidWorld(e) => write!(f, "invalid snapshot world: {e}"),
        }
    }
}
impl std::error::Error for SaveError {}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Envelope {
    version: u32,
    ruleset: String,
    checksum: String,
    world: serde_json::Value,
}

fn hash(bytes: &[u8]) -> String {
    let value = bytes.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, b| (h ^ u64::from(*b)).wrapping_mul(0x100_0000_01b3));
    format!("{value:016x}")
}

/// Identity includes every loaded, derived rule in registry order, including
/// names and all fields. Registry lookup HashMaps are deliberately excluded.
/// This format is tied to FORMAT_VERSION and must change with model schemas.
pub fn ruleset_identity(r: &Ruleset) -> String {
    let mut input = String::from("civ-core-rules-v2\n");
    macro_rules! registry {
        ($($field:ident),+ $(,)?) => {$(
            input.push_str(stringify!($field)); input.push('\n');
            for (id, value) in r.$field.iter() {
                use std::fmt::Write;
                writeln!(&mut input, "{:?}:{:?}:{:?}", id, r.$field.name_of(id), value).unwrap();
            }
        )+};
    }
    registry!(
        buildings,
        units,
        unit_types,
        terrains,
        resources,
        improvements,
        technologies,
        tech_columns,
        policies,
        policy_branches,
        promotions,
        nations,
        beliefs,
        difficulties,
        eras,
        speeds,
        specialists,
        victories,
        city_state_types,
        quests,
        ruin_rewards
    );
    use std::fmt::Write;
    write!(&mut input, "{:?}", r.global_uniques).unwrap();
    hash(input.as_bytes())
}

/// Precomputed identity for an immutable ruleset used by repeated snapshots.
#[derive(Clone)]
pub struct RulesetIdentity(String);

impl RulesetIdentity {
    pub fn new(ruleset: &Ruleset) -> Self {
        Self(ruleset_identity(ruleset))
    }
}

pub fn encode(world: &World, ruleset: &Ruleset) -> Result<String, SaveError> {
    encode_with_identity(world, ruleset, &RulesetIdentity::new(ruleset))
}

/// The identity must have been made from `ruleset`, which must remain immutable.
pub fn encode_with_identity(world: &World, ruleset: &Ruleset, identity: &RulesetIdentity) -> Result<String, SaveError> {
    validate(world, ruleset)?;
    let value = serde_json::to_value(world).map_err(|e| SaveError::InvalidJson(e.to_string()))?;
    let bytes = serde_json::to_vec(&value).map_err(|e| SaveError::InvalidJson(e.to_string()))?;
    let envelope =
        Envelope { version: FORMAT_VERSION, ruleset: identity.0.clone(), checksum: hash(&bytes), world: value };
    let text = serde_json::to_string(&envelope).map_err(|e| SaveError::InvalidJson(e.to_string()))?;
    if text.len() > MAX_SNAPSHOT_BYTES {
        return Err(SaveError::TooLarge);
    }
    Ok(text)
}

pub fn decode(text: &str, ruleset: &Ruleset) -> Result<World, SaveError> {
    if text.len() > MAX_SNAPSHOT_BYTES {
        return Err(SaveError::TooLarge);
    }
    let envelope: Envelope = serde_json::from_str(text).map_err(|e| SaveError::InvalidJson(e.to_string()))?;
    if envelope.version != FORMAT_VERSION {
        return Err(SaveError::UnsupportedVersion(envelope.version));
    }
    if envelope.ruleset != ruleset_identity(ruleset) {
        return Err(SaveError::RulesetMismatch);
    }
    let bytes = serde_json::to_vec(&envelope.world).map_err(|e| SaveError::InvalidJson(e.to_string()))?;
    if envelope.checksum != hash(&bytes) {
        return Err(SaveError::ChecksumMismatch);
    }
    let world = serde_json::from_value(envelope.world).map_err(|e| SaveError::InvalidJson(e.to_string()))?;
    validate(&world, ruleset)?;
    Ok(world)
}

fn rule<T>(registry: &Registry<T>, id: Id<T>, field: &str) -> Result<(), SaveError> {
    if id.generation() != 1 || registry.get(id).is_none() {
        return Err(SaveError::InvalidWorld(format!("invalid {field} ruleset reference")));
    }
    Ok(())
}

/// Validate deserialized state before exposing it to simulation. Facades that
/// use another serde codec must also call this function before resuming play.
pub fn validate(world: &World, r: &Ruleset) -> Result<(), SaveError> {
    let invalid = |message: &str| SaveError::InvalidWorld(message.into());
    crate::world::victory_actions::validate_snapshot(world, r).map_err(SaveError::InvalidWorld)?;
    crate::world::city_state_actions::validate(world, r).map_err(SaveError::InvalidWorld)?;
    if let Some(id) = world.difficulty {
        rule(&r.difficulties, id, "difficulty")?;
    }
    world.grid.validate_snapshot().map_err(SaveError::InvalidWorld)?;
    crate::world::barbarians::validate(world, r).map_err(SaveError::InvalidWorld)?;
    world.cities.validate_snapshot().map_err(SaveError::InvalidWorld)?;
    world.units.validate_snapshot().map_err(SaveError::InvalidWorld)?;
    if world.turn > 1_000_000_000 {
        return Err(invalid("turn count is outside snapshot bounds"));
    }
    let n = world.civs.len();
    if n == 0 || n > 256 {
        return Err(invalid("invalid civilization count"));
    }
    world.visibility.validate_snapshot(world.grid.len(), n).map_err(SaveError::InvalidWorld)?;
    world.visibility.validate_snapshot_rules(r).map_err(SaveError::InvalidWorld)?;
    world.diplomacy.validate_snapshot(n).map_err(SaveError::InvalidWorld)?;
    let civ = |c: CivId| if (c.0 as usize) < n { Ok(()) } else { Err(invalid("invalid civilization reference")) };
    let tile = |t: crate::world::grid::TileId| {
        if (t.0 as usize) < world.grid.len() {
            Ok(())
        } else {
            Err(invalid("invalid tile reference"))
        }
    };
    for i in 0..world.grid.len() {
        rule(&r.terrains, world.grid.terrain[i], "terrain")?;
        if let Some(id) = world.grid.natural_wonder[i] {
            rule(&r.terrains, id, "natural wonder")?;
        }
        for id in &world.grid.features[i] {
            rule(&r.terrains, *id, "terrain feature")?;
        }
        if let Some(id) = world.grid.resource[i] {
            rule(&r.resources, id, "resource")?;
        }
        if let Some(id) = world.grid.improvement[i] {
            rule(&r.improvements, id, "improvement")?;
        }
        if let Some((id, done)) = world.grid.improvement_progress[i] {
            rule(&r.improvements, id, "improvement work")?;
            if done == 0 || done > 1_000_000 || r.improvements[id].turns_to_build.unwrap_or(0) <= 0 {
                return Err(invalid("invalid improvement work"));
            }
        }
        if let Some(owner) = world.grid.owner[i] {
            civ(owner)?;
        }
        if !(0..=1_000_000).contains(&world.grid.resource_amount[i]) {
            return Err(invalid("invalid resource quantity"));
        }
    }
    let mut listed_cities = BTreeSet::new();
    let mut listed_units = BTreeSet::new();
    for (i, c) in world.civs.iter().enumerate() {
        if c.id.0 as usize != i {
            return Err(invalid("civilization IDs must match their index"));
        }
        if c.defeated && (!c.units.is_empty() || !c.espionage.spies.is_empty()) {
            return Err(invalid("defeated civilization retains active units or spies"));
        }
        if !c.is_barbarian && c.has_ever_owned_original_capital && c.cities.is_empty() && !c.defeated {
            return Err(invalid("established civilization lost its cities without settled defeat"));
        }
        rule(&r.nations, c.nation, "nation")?;
        if let Some(id) = c.city_state_type {
            rule(&r.city_state_types, id, "city-state type")?;
        }
        for (tech, work) in &c.science_invested {
            rule(&r.technologies, *tech, "science investment")?;
            if !(0..=1_000_000_000).contains(work)
                || (c.has_tech(*tech)
                    && r.technologies.get(*tech).is_none_or(|t| {
                        !t.uniques.iter().any(|u| u.unique_type() == Some(crate::UniqueType::ResearchableMultipleTimes))
                    }))
            {
                return Err(invalid("invalid technology investment"));
            }
        }
        if !(0..=1_000_000_000).contains(&c.science_from_agreements) {
            return Err(invalid("invalid pending research-agreement science"));
        }
        if c.science_stored < 0 {
            return Err(invalid("negative science overflow"));
        }
        for id in &c.techs {
            rule(&r.technologies, *id, "technology")?;
        }
        if let Some(id) = c.researching {
            rule(&r.technologies, id, "research")?;
        }
        if c.great_people.maya_restricted > c.pending_rewards.free_great_people
            || c.great_people.maya_last_award_turn.is_some_and(|t| t > world.turn)
        {
            return Err(invalid("invalid Maya reward state"));
        }
        for id in &c.great_people.maya_available {
            rule(&r.units, *id, "Maya great person")?;
        }
        if c.great_people.points.values().chain(c.great_people.combat_points.values()).any(|n| *n < 0)
            || c.great_people.thresholds.values().chain(c.great_people.combat_thresholds.values()).any(|n| *n <= 0)
        {
            return Err(invalid("invalid great person counters"));
        }
        if c.paid_policies as usize > c.adopted_policies.len()
            || c.pending_rewards.free_techs > 1_000_000
            || c.pending_rewards.free_policies > 1_000_000
            || c.pending_rewards.free_great_people > 1_000_000
            || c.free_building_grants.len() > 100_000
        {
            return Err(invalid("invalid progression rewards"));
        }
        for (key, cities) in &c.free_building_grants {
            if key.len() > 256 || cities.len() > 100_000 || cities.iter().any(|id| id.index() > 1_000_000) {
                return Err(invalid("invalid free building history"));
            }
        }
        for (policy, expiry) in &c.timed_policy_expirations {
            rule(&r.policies, *policy, "timed policy")?;
            if !c.adopted_policies.contains(policy) || *expiry > 1_000_000_000 {
                return Err(invalid("invalid timed policy"));
            }
        }
        for id in &c.adopted_policies {
            rule(&r.policies, *id, "policy")?;
        }
        for id in &c.faith.beliefs {
            rule(&r.beliefs, *id, "belief")?;
        }
        for id in &c.faith.pantheon_beliefs {
            rule(&r.beliefs, *id, "pantheon belief")?;
            if r.beliefs[*id].r#type != "Pantheon" || !c.faith.beliefs.contains(id) {
                return Err(invalid("invalid original pantheon belief"));
            }
        }
        for id in &c.known_natural_wonders {
            rule(&r.terrains, *id, "known natural wonder")?;
        }
        for id in c.resources.keys() {
            rule(&r.resources, *id, "resource stock")?;
        }
        if !(0..=1_000_000_000).contains(&c.culture_for_contests)
            || !(0..=1_000_000_000).contains(&c.faith_for_contests)
        {
            return Err(invalid("invalid contest income counters"));
        }
        if c.faith.prophets_earned < 0 || c.faith.prophets_earned > 1_000 {
            return Err(invalid("invalid prophet counter"));
        }
        if [c.gold, c.science_stored, c.culture_stored, c.faith.stored, c.golden_age.stored_happiness]
            .into_iter()
            .chain(c.great_people.points.values().copied())
            .chain(c.great_people.thresholds.values().copied())
            .chain(c.great_people.combat_points.values().copied())
            .chain(c.great_people.combat_thresholds.values().copied())
            .chain(c.recent_science)
            .chain(c.resources.values().copied())
            .any(|v| !(-1_000_000_000..=1_000_000_000).contains(&v))
            || c.golden_age.count > 1_000_000
            || c.golden_age.turns_left > 1_000_000_000
            || c.bought_with_increasing_price.values().any(|v| *v > 1_000_000)
        {
            return Err(invalid("civilization counters are outside snapshot bounds"));
        }
        for id in &c.cities {
            if world.cities.get(*id).is_none_or(|city| city.owner != c.id) || !listed_cities.insert(*id) {
                return Err(invalid("city ownership registry is inconsistent"));
            }
        }
        if c.revolt_countdown.is_some_and(|n| n > 100_000) {
            return Err(invalid("invalid revolt countdown"));
        }
        for base in &c.units_gained {
            rule(&r.units, *base, "gained unit")?;
        }
        for id in &c.units {
            if world.units.get(*id).is_none_or(|unit| unit.owner != c.id) || !listed_units.insert(*id) {
                return Err(invalid("unit ownership registry is inconsistent"));
            }
        }
        let mut stations = BTreeSet::new();
        for spy in &c.espionage.spies {
            use crate::world::espionage::SpyAction;
            if spy.rank == 0 || spy.rank > 3 || !(0..=1_000_000_000).contains(&spy.progress) {
                return Err(invalid("invalid spy rank or progress"));
            }
            let valid = match spy.action {
                SpyAction::Hideout => spy.station.is_none() && spy.turns_remaining == 0 && spy.progress == 0,
                SpyAction::Dead => spy.station.is_none() && (1..=5).contains(&spy.turns_remaining) && spy.progress == 0,
                SpyAction::Moving => spy.station.is_some() && spy.turns_remaining == 1 && spy.progress == 0,
                SpyAction::EstablishingNetwork => {
                    spy.station.is_some() && (1..=3).contains(&spy.turns_remaining) && spy.progress == 0
                }
                _ => spy.station.is_some() && spy.turns_remaining <= 1_000_000,
            };
            if !valid {
                return Err(invalid("inconsistent spy action"));
            }
            if let Some(id) = spy.station {
                if !stations.insert(id) {
                    return Err(invalid("multiple spies assigned to one city"));
                }
                let remembered_trip =
                    spy.action == SpyAction::Moving && world.visibility.known_cities(c.id).any(|seen| seen.id == id);
                if world.cities.get(id).is_none() && !remembered_trip {
                    return Err(invalid("spy station is missing"));
                }
            }
        }
        for id in c.quests.individual_next.keys() {
            civ(*id)?;
        }
        for (attacker, q) in &c.quests.assistance {
            civ(*attacker)?;
            if q.needed < 3 || q.needed > 1_000_000 || !c.is_city_state() {
                return Err(invalid("invalid city-state war assistance"));
            }
            for (killer, n) in &q.kills {
                civ(*killer)?;
                if *n >= q.needed {
                    return Err(invalid("completed war assistance was not cleared"));
                }
            }
        }
        for quest in &c.quests.active {
            rule(&r.quests, quest.quest, "quest")?;
            civ(quest.assignee)?;
            use crate::world::quests::Target;
            match quest.target {
                Target::Resource(id) => rule(&r.resources, id, "quest resource")?,
                Target::Building(id) => rule(&r.buildings, id, "quest building")?,
                Target::Unit(id) => rule(&r.units, id, "quest unit")?,
                Target::Civ(id) => civ(id)?,
                Target::Tile(id) => tile(id)?,
                _ => {}
            }
        }
    }
    if listed_cities.len() != world.cities.len() || listed_units.len() != world.units.len() {
        return Err(invalid("an entity is absent from its owner's registry"));
    }
    let mut centres = BTreeSet::new();
    let mut claimed = BTreeSet::new();
    for (_, city) in world.cities.iter() {
        civ(city.owner)?;
        if city
            .production_conversion
            .is_some_and(|s| !matches!(s, crate::stats::Stat::Gold | crate::stats::Stat::Science))
            || (city.production_conversion.is_some() && !city.queue.is_empty())
        {
            return Err(invalid("invalid city production conversion"));
        }
        civ(city.founder)?;
        if let Some(previous) = city.previous_owner {
            civ(previous)?;
        }
        if city.original_capital
            && (!world.civ(city.owner).is_some_and(|c| c.has_ever_owned_original_capital)
                || !world.civ(city.founder).is_some_and(|c| c.has_ever_owned_original_capital))
        {
            return Err(invalid("original capital is missing civilization ownership history"));
        }
        tile(city.centre)?;
        if city.population == 0 || city.population > 1_000_000 {
            return Err(invalid("invalid city population"));
        }
        if [city.food_stored, city.culture_stored, city.health]
            .into_iter()
            .any(|v| !(-1_000_000_000..=1_000_000_000).contains(&v))
            || city.specialists.values().any(|v| *v > city.population)
        {
            return Err(invalid("city counters are outside snapshot bounds"));
        }
        if !centres.insert(city.centre)
            || world.grid.owner[city.centre.0 as usize] != Some(city.owner)
            || !city.tiles.contains(&city.centre)
        {
            return Err(invalid("city centre ownership is inconsistent"));
        }
        if !city.tiles.windows(2).all(|w| w[0] < w[1]) {
            return Err(invalid("city tile list is not ordered and unique"));
        }
        for t in &city.tiles {
            tile(*t)?;
            if world.grid.owner[t.0 as usize] != Some(city.owner) || !claimed.insert(*t) {
                return Err(invalid("city territory ownership is inconsistent"));
            }
        }
        let mut worked = BTreeSet::new();
        for t in &city.worked {
            tile(*t)?;
            if !worked.insert(*t) {
                return Err(invalid("duplicate worked tile"));
            }
        }
        for id in &city.built {
            rule(&r.buildings, *id, "building")?;
        }
        for id in city.specialists.keys() {
            rule(&r.specialists, *id, "specialist")?;
        }
        if !(0..=1_000_000_000).contains(&city.production_overflow)
            || city.production_progress.values().any(|v| !(1..=1_000_000_000).contains(v))
        {
            return Err(invalid("invalid construction investment or overflow"));
        }
        if !city.free_buildings.is_subset(&city.built) {
            return Err(invalid("free building is not present"));
        }
        for what in city.queue.iter().chain(city.production_progress.keys()) {
            match what {
                crate::world::construction::Construction::Unit(id) => rule(&r.units, *id, "construction unit")?,
                crate::world::construction::Construction::Building(id) => {
                    rule(&r.buildings, *id, "construction building")?
                }
            }
        }
        if let Some(id) = city.demanded_resource {
            rule(&r.resources, id, "demanded resource")?;
        }
        for id in &city.faith.adopted_religions {
            civ(*id)?;
        }
        if let Some(id) = city.faith.holy_city_of {
            civ(id)?;
        }
        let mut total = 0i64;
        for (creed, pressure) in
            city.faith.pressures().chain(city.faith.pantheon_pressures().map(|(c, p)| (Some(c), p)))
        {
            if let Some(c) = creed {
                civ(c)?;
            }
            if pressure < 0 {
                return Err(invalid("negative religious pressure"));
            }
            total += pressure as i64;
        }
        if total == 0 || total > i32::MAX as i64 {
            return Err(invalid("invalid religious pressure total"));
        }
        let mut total_followers = 0u64;
        for (creed, followers) in city.faith.followers().chain(city.faith.pantheon_followers()) {
            civ(creed)?;
            if followers > city.population {
                return Err(invalid("invalid follower count"));
            }
            total_followers += u64::from(followers);
        }
        if total_followers > u64::from(city.population) {
            return Err(invalid("combined religious followers exceed population"));
        }
    }
    for (_, unit) in world.units.iter() {
        civ(unit.owner)?;
        tile(unit.position)?;
        rule(&r.units, unit.base, "unit")?;
        for id in &unit.promotions {
            rule(&r.promotions, *id, "promotion")?;
        }
        if let Some(id) = unit.religion {
            civ(id)?;
        }
        if unit.health > 100 || unit.movement_left.raw() < 0 || unit.movement_left.raw() > 1_000_000_000 {
            return Err(invalid("invalid unit health or movement"));
        }
        if unit.fortification_turns > 2 {
            return Err(invalid("invalid fortification turns"));
        }
        if u64::from(unit.promotions_earned) * u64::from(unit.promotions_earned.saturating_add(1)) * 5
            > u64::from(unit.xp)
        {
            return Err(invalid("paid promotions exceed earned experience"));
        }
    }
    crate::world::air::validate_transports(world, r).map_err(|message| invalid(&message))?;
    for deal in &world.diplomacy.deals {
        civ(deal.a)?;
        civ(deal.b)?;
        if deal.a == deal.b {
            return Err(invalid("trade has identical parties"));
        }
        for running in deal.a_gives.iter().chain(&deal.b_gives) {
            use crate::world::trade::Item;
            match running.item {
                Item::Luxury(id) | Item::Strategic(id, _) => rule(&r.resources, id, "trade resource")?,
                Item::City(_) => return Err(invalid("city transfer cannot be a recurring trade")),
                _ => {}
            }
        }
    }
    if world.winner.is_some() != world.victory_turn.is_some()
        || world.victory_turn.is_some_and(|turn| turn > world.turn)
    {
        return Err(invalid("victory result and original round are inconsistent"));
    }
    if let Some((winner, victory)) = world.winner {
        civ(winner)?;
        rule(&r.victories, victory, "victory")?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stats::Fixed;
    use crate::world::city::City;
    use crate::world::entities::{Civilization, Unit};
    use crate::world::grid::{TileGrid, TileId};

    fn populated() -> (Ruleset, World, Id<Unit>) {
        let r = Ruleset::load_dir("../android/assets/jsons/Civ V - Gods & Kings").unwrap().unwrap();
        let nation = r.nations.ids().next().unwrap();
        let mut world = World::new(
            TileGrid::new(16, 12, r.terrain("Grassland").unwrap()),
            vec![Civilization::new(CivId(0), nation, true), Civilization::new(CivId(1), nation, true)],
        );
        let cities = [(CivId(0), 3, 4), (CivId(1), 11, 4)].map(|(owner, x, y)| {
            let centre = world.grid.id_at(x, y).unwrap();
            let mut city = City::found(format!("Home {}", owner.0), owner, centre);
            city.population = 4;
            city.food_stored = 17;
            city.production_progress
                .insert(crate::world::construction::Construction::Building(r.building("Monument").unwrap()), 21);
            city.production_overflow = 7;
            let id = world.found_city_in(&r, city);
            for tile in world.grid.within(centre, 1) {
                world.claim_tile(id, tile);
            }
            id
        });
        let first = cities[0];
        let second = cities[1];
        let centre = world.cities.get(first).unwrap().centre;
        let vacant = world.place_unit(Unit::new(r.unit("Warrior").unwrap(), CivId(0), centre, 2));
        world.remove_unit(vacant);
        let active = world.place_unit(Unit::new(r.unit("Scout").unwrap(), CivId(0), centre, 2));
        let spare = world.place_unit(Unit::new(r.unit("Warrior").unwrap(), CivId(0), centre, 2));
        let spare2 = world.place_unit(Unit::new(r.unit("Warrior").unwrap(), CivId(0), centre, 2));
        world.remove_unit(spare);
        world.remove_unit(spare2);
        let unit = world.units.get_mut(active).unwrap();
        unit.movement_left = Fixed::permille(750);
        unit.health = 71;
        unit.xp = 23;
        unit.promotions.insert(r.promotions.ids().next().unwrap());
        unit.religion = Some(CivId(0));
        unit.religious_strength = 750;
        unit.religious_uses_left = 1;
        let resource = r.resources.ids().next().unwrap();
        world.grid.resource[centre.0 as usize] = Some(resource);
        world.grid.resource_amount[centre.0 as usize] = 3;
        let c = world.civ_mut(CivId(0)).unwrap();
        c.gold = 456;
        c.science_stored = 82;
        c.culture_stored = 51;
        c.faith.stored = 199;
        c.faith.founded = true;
        c.faith.enhanced = true;
        c.faith.beliefs.insert(r.beliefs.ids().next().unwrap());
        c.faith.prophets_earned = 2;
        c.resources.insert(resource, 2);
        c.adopted_policies.insert(r.policies.ids().next().unwrap());
        c.techs.insert(r.technologies.ids().next().unwrap());
        c.researching = r.technologies.ids().nth(1);
        c.bought_with_increasing_price.insert("Great Scientist".into(), 3);
        c.great_people.points.insert("Great Scientist".into(), 17);
        c.great_people.thresholds.insert("Great Scientist".into(), 400);
        c.recent_science = [1, 2, 3, 4, 5, 6, 7, 8];
        c.recent_ruins = vec!["Ancient Treasury".into()];
        c.golden_age.turns_left = 4;
        c.espionage.spies.push(crate::world::espionage::Spy {
            rank: 2,
            station: Some(second),
            action: crate::world::espionage::SpyAction::StealingTech,
            turns_remaining: 3,
            progress: 27,
        });
        c.espionage.eras_rewarded.insert(3);
        c.quests.active.push(crate::world::quests::Assigned {
            quest: r.quests.ids().next().unwrap(),
            assignee: CivId(1),
            target: crate::world::quests::Target::Resource(resource),
            contest: Some(crate::world::quests::Contest::Culture),
            assigned_on: 2,
            expires_on: 50,
            baseline: 9,
        });
        let city = world.cities.get_mut(first).unwrap();
        city.faith.holy_city_of = Some(CivId(0));
        city.faith.add_pressure(Some(CivId(0)), 1200);
        city.faith.add_pressure(Some(CivId(1)), 200);
        city.faith.recount(city.population);
        city.specialists.insert(r.specialists.ids().next().unwrap(), 1);
        city.queue.push(crate::world::construction::Construction::Unit(r.unit("Worker").unwrap()));
        city.demanded_resource = Some(resource);
        world.diplomacy.meet(CivId(0), CivId(1));
        world.diplomacy.set_open_borders(CivId(0), CivId(1), true);
        world.diplomacy.add_influence(CivId(1), CivId(0), Fixed::permille(30500));
        world
            .diplomacy
            .attitude_mut(CivId(0), CivId(1))
            .set(crate::world::diplomacy::Sentiment::Denunciation, Fixed::whole(-15));
        world.diplomacy.set_flag(
            CivId(0),
            CivId(1),
            crate::world::diplomacy::Flag::Agreed(crate::world::promises::Demand::DoNotSpreadReligion),
            30,
        );
        world.diplomacy.deals.push(crate::world::trade::Deal {
            a: CivId(0),
            b: CivId(1),
            a_gives: vec![crate::world::trade::Running {
                item: crate::world::trade::Item::Strategic(resource, 1),
                turns_left: 20,
            }],
            b_gives: vec![],
        });
        world.diplomacy.demands.insert((CivId(0), CivId(1), crate::world::promises::Demand::DontSpyOnUs));
        for civ in [CivId(0), CivId(1)] {
            crate::world::policies::rebuild_uniques(&mut world, &r, civ);
        }
        crate::world::turn::update_visibility(&mut world);
        crate::world::fog::capture_observations(&mut world, &r);
        world.turn = 12;
        (r, world, vacant)
    }

    fn mutate(text: &str, change: impl FnOnce(&mut serde_json::Value)) -> String {
        let mut value: serde_json::Value = serde_json::from_str(text).unwrap();
        change(&mut value["world"]);
        value["checksum"] = hash(&serde_json::to_vec(&value["world"]).unwrap()).into();
        serde_json::to_string(&value).unwrap()
    }

    #[test]
    fn full_world_roundtrip_preserves_generations_maps_caches_and_reuse_order() {
        let (r, mut world, stale) = populated();
        let text = encode(&world, &r).unwrap();
        assert_eq!(encode_with_identity(&world, &r, &RulesetIdentity::new(&r)).unwrap(), text);
        assert_eq!(encode(&world, &r).unwrap(), text, "snapshots must have canonical ordering");
        let mut restored = decode(&text, &r).unwrap();
        assert_eq!(restored, world);
        assert!(restored.units.get(stale).is_none());
        for _ in 0..3 {
            let unit = Unit::new(r.unit("Warrior").unwrap(), CivId(0), TileId(1), 2);
            assert_eq!(world.place_unit(unit.clone()), restored.place_unit(unit));
        }
        assert_eq!(restored, world);
        assert_eq!(serde_json::to_string(&world).unwrap(), serde_json::to_string(&restored).unwrap());
        assert_eq!(world.civ(CivId(0)).unwrap().uniques, restored.civ(CivId(0)).unwrap().uniques);
    }

    #[test]
    fn continued_turns_after_restore_are_identical() {
        let (r, mut world, _) = populated();
        let mut restored = decode(&encode(&world, &r).unwrap(), &r).unwrap();
        for _ in 0..3 {
            let left = crate::world::turn::advance_turn(
                &mut world,
                &r,
                crate::world::assignment::CityFocus::None,
                Default::default(),
            );
            let right = crate::world::turn::advance_turn(
                &mut restored,
                &r,
                crate::world::assignment::CityFocus::None,
                Default::default(),
            );
            assert_eq!(left, right);
            assert_eq!(world, restored);
        }
    }

    #[test]
    fn corrupted_truncated_version_and_ruleset_mismatches_return_errors() {
        let (r, world, _) = populated();
        let text = encode(&world, &r).unwrap();
        for end in [0, 1, text.len() / 2, text.len() - 1] {
            assert!(decode(&text[..end], &r).is_err());
        }
        let mut value: serde_json::Value = serde_json::from_str(&text).unwrap();
        value["world"]["turn"] = 13.into();
        assert_eq!(decode(&serde_json::to_string(&value).unwrap(), &r), Err(SaveError::ChecksumMismatch));
        value["version"] = 999.into();
        assert_eq!(decode(&serde_json::to_string(&value).unwrap(), &r), Err(SaveError::UnsupportedVersion(999)));
        let mut other = r.clone();
        let base = other.unit("Warrior").unwrap();
        other.units.get_mut(base).unwrap().movement += 1;
        assert_eq!(decode(&text, &other), Err(SaveError::RulesetMismatch));
    }

    #[test]
    fn invalid_arena_grid_ids_and_owner_registries_are_rejected_even_with_valid_checksums() {
        let (r, world, _) = populated();
        let text = encode(&world, &r).unwrap();
        let cases = [
            mutate(&text, |w| w["units"]["free"] = serde_json::json!([1, 1])),
            mutate(&text, |w| w["units"]["len"] = 99.into()),
            mutate(&text, |w| w["grid"]["width"] = 0.into()),
            mutate(&text, |w| w["grid"]["owner"] = serde_json::json!([])),
            mutate(&text, |w| w["grid"]["terrain"][0] = serde_json::json!([999999, 1])),
            mutate(&text, |w| w["civs"][0]["cities"] = serde_json::json!([])),
            mutate(&text, |w| w["civs"][0]["units"][0] = serde_json::json!([0, 1])),
            mutate(&text, |w| w["civs"][0]["nation"] = serde_json::json!([0, 0])),
            mutate(&text, |w| w["civs"][0]["nation"] = serde_json::json!([0, 2])),
            mutate(&text, |w| w["civs"][0]["id"] = 7.into()),
        ];
        for case in cases {
            assert!(decode(&case, &r).is_err());
        }
    }

    #[test]
    fn tuple_map_duplicate_keys_are_rejected_instead_of_overwritten() {
        let (r, world, _) = populated();
        let text = encode(&world, &r).unwrap();
        let duplicated = mutate(&text, |w| {
            let map = w["diplomacy"]["pacts"].as_array_mut().unwrap();
            map.push(map[0].clone());
        });
        assert!(decode(&duplicated, &r).is_err());
    }

    #[test]
    fn saved_victory_preserves_its_original_round_and_rejects_partial_results() {
        let (r, mut world, _) = populated();
        world.winner = Some((CivId(0), r.victories.id("Domination").unwrap()));
        world.victory_turn = Some(world.turn - 2);
        let original_result = (world.winner, world.victory_turn);
        let text = encode(&world, &r).unwrap();
        let mut restored = decode(&text, &r).unwrap();
        assert_eq!(restored, world);
        for _ in 0..2 {
            let left = crate::world::turn::advance_turn(
                &mut world,
                &r,
                crate::world::assignment::CityFocus::None,
                Default::default(),
            );
            let right = crate::world::turn::advance_turn(
                &mut restored,
                &r,
                crate::world::assignment::CityFocus::None,
                Default::default(),
            );
            assert_eq!(left, right);
            assert_eq!(restored, world);
            assert_eq!((restored.winner, restored.victory_turn), original_result);
        }

        for malformed in [
            mutate(&text, |w| w["victory_turn"] = serde_json::Value::Null),
            mutate(&text, |w| w["winner"] = serde_json::Value::Null),
            mutate(&text, |w| w["victory_turn"] = 13.into()),
        ] {
            assert!(matches!(decode(&malformed, &r), Err(SaveError::InvalidWorld(_))));
        }
        world.victory_turn = None;
        assert!(matches!(encode(&world, &r), Err(SaveError::InvalidWorld(_))));
    }
}
