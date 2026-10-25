// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Advisory options derived from owned state and public rules. Queries do not
//! probe hidden world-wide uniqueness or resource conditions to explain why an
//! action might fail; execution remains authoritative.
use crate::protocol::{AttackTarget, ConstructionRef, EntityRef};
use civ_core::arena::{Arena, Id};
use civ_core::ruleset::Ruleset;
use civ_core::stats::Stat;
use civ_core::world::commands::{self, Command};
use civ_core::world::construction::{self, Construction, Rejection};
use civ_core::modifiers::Layer;
use civ_core::stats::Fixed;
use civ_core::world::purchase::Bearing;
use civ_core::world::{air, combat, policies, purchase, research, CivId, TileId, World};
use civ_core::{Unique, UniqueType};
use serde::{Deserialize, Serialize};

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub enum Availability {
    Available,
    NeedsTarget,
    NeedsConfirmation,
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub enum UnitActionKind {
    Move,
    Attack,
    NuclearStrike,
    Fortify,
    Skip,
    FoundCity,
    Improve,
    Pillage,
    Rebase,
    AirSweep,
    Paradrop,
    AssembleSpaceshipPart,
    Disband,
    SetUp,
    Gift,
    /// `automateUnit`: work tiles on its own.
    Automate,
    /// `exploreUnit`: explore on its own.
    Explore,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS, schemars::JsonSchema)]
pub struct UnitActionChoice {
    pub action: UnitActionKind,
    pub availability: Availability,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS, schemars::JsonSchema)]
pub struct PromotionChoice {
    pub id: u32,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct UpgradeChoice {
    pub unit_type: u32,
    pub gold_cost: String,
    pub availability: Availability,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS, schemars::JsonSchema)]
pub struct UnitOptions {
    pub unit: EntityRef,
    pub actions: Vec<UnitActionChoice>,
    pub promotions: Vec<PromotionChoice>,
    pub upgrade: Option<UpgradeChoice>,
    pub improvements: Vec<u32>,
    pub disband_gold: Option<i32>,
    pub religion: Option<ReligionOptions>,
    pub great_person_actions: Vec<crate::protocol::GreatPersonAction>,
    pub religious_actions: Vec<crate::protocol::ReligiousAction>,
    /// Where each `needsTarget` mission may aim now, from what the player
    /// knows: `rebase`, `airSweep`, `paradrop` and `nuclearStrike`. A nuclear
    /// target can still be refused for a nation the player has not met whose
    /// land or units lie unseen in the blast. Absent for units with none.
    #[ts(optional)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub targets: Option<Vec<ActionTargets>>,
    /// Every ability a great person has, whether or not it may be used where
    /// it stands, with why not (`great_person_actions` lists only the legal
    /// ones). Absent for units that are not great people.
    #[ts(optional)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub great_person_abilities: Option<Vec<GreatPersonAbility>>,
    /// What a nuclear strike would do on the tile the query named in
    /// `nuclearTarget`. Absent when none was named, or for other units.
    #[ts(optional)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nuclear_preview: Option<NuclearPreviewView>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS, schemars::JsonSchema)]
pub struct ActionTargets {
    pub action: UnitActionKind,
    pub tiles: Vec<u16>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct GreatPersonAbility {
    pub action: crate::protocol::GreatPersonAction,
    /// It may be used where the unit stands now (it is in `great_person_actions`).
    pub available: bool,
    /// Why not, when it may not.
    #[ts(optional)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub why: Option<GreatPersonRefusal>,
    /// What it would bring now: science for `hurryResearch`, hammers for
    /// `hurryProduction` (here), gold for `tradeMission`. Absent otherwise.
    #[ts(optional)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub amount: Option<i32>,
}

/// Why a great person's ability cannot be used where it stands.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub enum GreatPersonRefusal {
    /// The unit has no movement left this turn.
    NoMovement,
    /// Nothing is being researched.
    NothingResearched,
    /// What is being researched or built cannot be hurried.
    CannotBeHurried,
    /// It must stand in one of the player's own cities.
    NotInOwnCity,
    /// The city there is not building a building.
    NotABuilding,
    /// The building would finish anyway.
    NothingToAdd,
    /// It must stand in a city-state's land.
    NotACityState,
    /// That city-state is at war with the player.
    AtWar,
    /// The improvement cannot stand on this tile.
    CannotBuildHere,
    /// This unit cannot do it any more (a prophet that has spread its faith).
    NotAble,
}

/// What a nuclear strike on a tile would do, as far as the player can see.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct NuclearPreviewView {
    pub blast_radius: u32,
    /// Every tile in the blast.
    pub tiles: Vec<u16>,
    /// Nations the strike declares war on: those holding land in the blast,
    /// as last seen, or with units seen there. Unseen units can add more.
    pub victims: Vec<u8>,
    /// Percent chance a defender the player can see intercepts it; 0 when
    /// none is seen or the weapon cannot be intercepted.
    pub intercept_chance: i32,
    /// Whether `nuclearStrike` would be accepted here, from what the player
    /// knows.
    pub legal: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS, schemars::JsonSchema)]
pub struct ReligionOptions {
    pub enhancing: bool,
    pub requirements: Vec<BeliefRequirement>,
    pub choices: Vec<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS, schemars::JsonSchema)]
pub struct BeliefRequirement {
    pub kind: String,
    pub count: u32,
}

/// Why an option shown to the player would be refused now, so the choice can
/// say so. Only reasons the player's own knowledge supports: a refusal that
/// could depend on something unseen carries no reason, and the action itself
/// stays the authority.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub enum Refusal {
    /// A puppet's governor decides this; annex the city first.
    Puppet,
    /// The city is still resisting its conqueror.
    Resistance,
    /// It can be bought here but not built.
    PurchaseOnly,
    /// A unit of the same kind already stands in the city.
    Occupied,
    /// The treasury holds too little.
    CannotAfford,
    /// A founder's first city is never razed.
    OriginalCapital,
    /// A religion's holy city is never razed.
    HolyCity,
}

impl Refusal {
    pub fn raze(why: civ_core::world::conquest::RazeRejection) -> Option<Self> {
        use civ_core::world::conquest::RazeRejection as R;
        match why {
            R::OriginalCapital => Some(Self::OriginalCapital),
            R::HolyCity => Some(Self::HolyCity),
            R::NoSuchCity => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS, schemars::JsonSchema)]
pub struct PriceQuote {
    /// Exact decimal amount, or unknown when a conditional price could depend
    /// on information outside this player's knowledge.
    pub amount: Option<String>,
    pub affordable: Option<bool>,
    pub availability: Availability,
    /// Why `purchase` would refuse it now, when a known price says it would.
    #[ts(optional)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refusal: Option<Refusal>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ConstructionChoice {
    pub construction: ConstructionRef,
    pub name: String,
    pub production_cost: Option<i32>,
    pub availability: Availability,
    pub gold: Option<PriceQuote>,
    pub faith: Option<PriceQuote>,
    /// For a wonder or building that stands on a tile: every tile it may
    /// stand on, and the one it will (the player's choice, else the best).
    /// Choose another with `chooseSite`.
    #[ts(optional)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sites: Option<SiteChoice>,
    /// Why `setProduction` would refuse it now: `puppet`, `resistance` or
    /// `purchaseOnly`. Absent when it may be queued.
    #[ts(optional)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refusal: Option<Refusal>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SiteChoice {
    pub tiles: Vec<u16>,
    pub selected: Option<u16>,
    /// A world wonder takes its tile (nobody works it again).
    pub takes_tile: bool,
    /// The engine's own pick, whatever the player chose: the suggestion.
    #[ts(optional)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub best: Option<u16>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS, schemars::JsonSchema)]
pub struct CityOptions {
    pub city: EntityRef,
    pub choices: Vec<ConstructionChoice>,
    pub sellable_buildings: Vec<SellBuildingChoice>,
    pub purchasable_tiles: Vec<TilePurchaseChoice>,
    pub production_conversions: Vec<crate::protocol::ProductionConversion>,
    /// Tiles the city may work (its centre always works for free), with their yields to the owner.
    pub workable_tiles: Vec<WorkableTile>,
    pub specialist_slots: Vec<SpecialistSlot>,
    /// Enemy units the city can bombard this turn, empty once its attack is spent.
    pub bombard: Vec<BombardTarget>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS, schemars::JsonSchema)]
pub struct BombardTarget {
    pub unit: EntityRef,
    pub health: i32,
    /// Damage at the lowest and the highest roll.
    pub damage_min: i32,
    pub damage_max: i32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS, schemars::JsonSchema)]
pub struct WorkableTile {
    pub tile: u16,
    pub food: i32,
    pub production: i32,
    pub gold: i32,
    pub science: i32,
    pub culture: i32,
    pub faith: i32,
    pub worked: bool,
    /// Another of the owner's cities works it.
    pub taken: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS, schemars::JsonSchema)]
pub struct SpecialistSlot {
    pub specialist: u32,
    pub slots: u32,
    /// What one specialist of this kind yields here: its own yields
    /// (`RuleTables.specialists`) plus what the empire's rules add to every
    /// specialist, before the city's percentage bonuses.
    #[serde(default)]
    pub yields: crate::rules::Yields,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS, schemars::JsonSchema)]
pub struct SellBuildingChoice {
    pub building: u32,
    pub gold: i32,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS, schemars::JsonSchema)]
pub struct TilePurchaseChoice {
    pub tile: u16,
    pub gold: String,
    /// The treasury holds the price. Tiles the city could buy but cannot
    /// afford are listed too, with `false`.
    #[serde(default)]
    pub affordable: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ResearchChoice {
    pub id: u32,
    pub name: String,
    pub science_cost: i32,
    pub invested: i32,
    pub selected: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS, schemars::JsonSchema)]
pub struct PolicyChoice {
    pub id: u32,
    pub name: String,
    pub affordable: bool,
    pub availability: Availability,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct CivOptions {
    pub research: Vec<ResearchChoice>,
    pub policies: Vec<PolicyChoice>,
    pub next_policy_cost: i32,
    pub free_technologies: Vec<u32>,
    pub free_great_people: Vec<u32>,
    pub pantheon_cost: i32,
    pub pantheons: Vec<u32>,
    /// When faiths arise by era: the state faith and what may replace it.
    #[ts(optional)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state_faith: Option<StateFaithOptions>,
}

/// The one religious choice a nation makes (docs/CONTENT.md, Religion).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct StateFaithOptions {
    /// Its key in `RuleTables.faiths`, if one is adopted.
    pub current: Option<u8>,
    /// Faiths the majority of at least one of our cities follows.
    pub adoptable: Vec<u8>,
    /// Turns before the state faith may change again; 0 when it may.
    pub locked_turns: u32,
}

/// A modifier on a previewed strength: a label and percentage points (25 means +25%).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ModifierLine {
    pub label: String,
    pub percent: i32,
}

/// One side of a previewed attack. Strengths are tenths: 165 means 16.5.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct CombatSideView {
    pub strength: i32,
    pub base: i32,
    pub modifiers: Vec<ModifierLine>,
    pub health: i32,
    pub max_health: i32,
    /// Damage taken at the lowest and the highest attack roll.
    pub damage_min: i32,
    pub damage_max: i32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct CombatPreviewView {
    pub ranged: bool,
    /// A melee attack on a civilian, or on a city at one health, captures it outright.
    pub capture: bool,
    pub attacker: CombatSideView,
    pub defender: CombatSideView,
    /// For an air strike: percent chance a defender the player can see
    /// intercepts it first. Absent for other attacks.
    #[ts(optional)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub intercept_chance: Option<i32>,
}

/// The engine's attack preview, for the attacker's owner and a target it can see now. It spans the attack
/// roll rather than giving the seeded outcome, and states the modifiers the fight itself will apply.
pub fn combat_preview(
    world: &World,
    r: &Ruleset,
    civ: CivId,
    unit: EntityRef,
    target: &AttackTarget,
) -> Option<CombatPreviewView> {
    let (attacker, _) = entity(&world.units, unit.id, unit.gen).filter(|(_, u)| u.owner == civ)?;
    let target = match target {
        AttackTarget::Unit { unit } => {
            let (id, _) = entity(&world.units, unit.id, unit.gen)?;
            civ_core::world::fog::can_see_unit(world, r, civ, id).then_some(air::Target::Unit(id))?
        }
        AttackTarget::City { city } => {
            let (id, c) = entity(&world.cities, city.id, city.gen)?;
            world.visibility.can_see(civ, c.centre).then_some(air::Target::City(id))?
        }
    };
    let p = combat::preview_attack(world, r, attacker, target)?;
    let intercept_chance = world.units.get(attacker).filter(|u| u.is_air(r)).map(|_| {
        let (owner, tile) = match target {
            air::Target::Unit(id) => world.units.get(id).map(|u| (u.owner, u.position)),
            air::Target::City(id) => world.cities.get(id).map(|c| (c.owner, c.centre)),
        }
        .unwrap_or((civ, TileId(0)));
        let seen = |id| civ_core::world::fog::can_see_unit(world, r, civ, id);
        air::visible_intercept_chance(world, r, attacker, tile, Some(target), Some(owner), &seen)
    });
    Some(CombatPreviewView {
        ranged: p.ranged,
        capture: p.capture,
        attacker: preview_side(&p.attacker),
        defender: preview_side(&p.defender),
        intercept_chance,
    })
}

/// A nuclear strike on `target` by the player's own unit, previewed from
/// what the player knows. None when the unit is not the player's nuclear
/// weapon or the tile is beyond its reach.
pub fn nuclear_preview(world: &World, r: &Ruleset, civ: CivId, unit: EntityRef, target: u16) -> Option<NuclearPreviewView> {
    let (id, u) = entity(&world.units, unit.id, unit.gen).filter(|(_, u)| u.owner == civ)?;
    if !u.has_unique(r, UniqueType::NuclearWeapon) || usize::from(target) >= world.grid.len() {
        return None;
    }
    let target = TileId(target);
    let radius = civ_core::world::nuclear::blast_radius(world, r, id);
    let tiles = world.grid.within(target, radius);
    let victims = Sight::of(world, r, civ).victims(civ, &tiles);
    let seen = |other| civ_core::world::fog::can_see_unit(world, r, civ, other);
    // Each nation whose seen units stand in the blast may send one interceptor.
    let mut spared = Fixed::ONE;
    for owner in &victims {
        let chance = air::visible_intercept_chance(world, r, id, target, None, Some(*owner), &seen);
        spared = spared * Fixed::from_fraction(i64::from(100 - chance.clamp(0, 100)), 100);
    }
    let intercept_chance =
        if u.is_air(r) { (Fixed::whole(100) - spared * Fixed::whole(100)).round() as i32 } else { 0 };
    Some(NuclearPreviewView {
        blast_radius: radius,
        tiles: tiles.iter().map(|t| t.0).collect(),
        legal: nuclear_target(world, r, civ, id, target, &victims),
        victims: victims.into_iter().map(|c| c.0).collect(),
        intercept_chance,
    })
}

/// Who holds each tile as the player last saw it, and whose units it sees.
struct Sight {
    owners: std::collections::BTreeMap<TileId, CivId>,
    units: std::collections::BTreeMap<TileId, std::collections::BTreeSet<CivId>>,
}

impl Sight {
    fn of(world: &World, r: &Ruleset, civ: CivId) -> Self {
        let owners =
            world.visibility.known_tiles(civ).filter_map(|(tile, seen)| seen.owner.map(|owner| (tile, owner))).collect();
        let mut units: std::collections::BTreeMap<TileId, std::collections::BTreeSet<CivId>> = Default::default();
        for (id, u) in world.units.iter() {
            if civ_core::world::fog::can_see_unit(world, r, civ, id) {
                units.entry(u.position).or_default().insert(u.owner);
            }
        }
        Self { owners, units }
    }

    /// Other nations with land in these tiles, as last seen, or units seen there.
    fn victims(&self, civ: CivId, tiles: &[TileId]) -> std::collections::BTreeSet<CivId> {
        tiles
            .iter()
            .flat_map(|t| self.owners.get(t).copied().into_iter().chain(self.units.get(t).into_iter().flatten().copied()))
            .filter(|owner| *owner != civ)
            .collect()
    }
}

/// Explored tiles in reach this nuclear weapon may strike, from what the player knows.
fn nuclear_targets(world: &World, r: &Ruleset, civ: CivId, id: Id<civ_core::world::Unit>) -> Vec<TileId> {
    let Some(unit) = world.units.get(id) else { return Vec::new() };
    let sight = Sight::of(world, r, civ);
    let radius = civ_core::world::nuclear::blast_radius(world, r, id);
    world
        .grid
        .within(unit.position, air::attack_range(world, r, id).max(0) as u32)
        .into_iter()
        .filter(|t| {
            world.visibility.has_explored(civ, *t)
                && nuclear_target(world, r, civ, id, *t, &sight.victims(civ, &world.grid.within(*t, radius)))
        })
        .collect()
}

/// `nuclear::can_strike`, with the nations in the blast those the player knows of.
fn nuclear_target(
    world: &World,
    r: &Ruleset,
    civ: CivId,
    id: Id<civ_core::world::Unit>,
    target: TileId,
    victims: &std::collections::BTreeSet<CivId>,
) -> bool {
    let Some(unit) = world.units.get(id) else { return false };
    unit.can_move()
        && unit.attacks_left > 0
        && target != unit.position
        && world.visibility.has_explored(civ, target)
        && world.grid.distance(unit.position, target) <= air::attack_range(world, r, id).max(0) as u32
        && victims.iter().all(|other| {
            world.civ(*other).is_none_or(|c| !c.is_alive() || c.is_barbarian)
                || (world.diplomacy.have_met(civ, *other)
                    && (world.diplomacy.at_war(civ, *other) || world.diplomacy.can_declare_war(civ, *other)))
        })
}

fn preview_side(s: &combat::PreviewSide) -> CombatSideView {
    let tenths = |v: Fixed| (v * Fixed::whole(10)).round() as i32;
    CombatSideView {
        strength: tenths(s.strength),
        base: tenths(s.base),
        modifiers: s
            .modifiers
            .iter()
            .filter(|m| listed_modifier(m))
            .map(|m| ModifierLine { label: modifier_label(&m.source.name), percent: m.amount.round() as i32 })
            .collect(),
        health: s.health,
        max_health: s.max_health,
        damage_min: s.damage.0,
        damage_max: s.damage.1,
    }
}

/// A percentage that moved a strength. Floors, and a mission bonus of nothing, are not shown.
pub(crate) fn listed_modifier(m: &civ_core::modifiers::Modifier) -> bool {
    matches!(m.layer, Layer::Additive | Layer::Multiplicative) && m.amount != Fixed::ZERO
}

/// Engine sources are short phrases ("Flanking") or ability text with bracketed parameters
/// ("+[50]% Strength vs [Mounted] units"). The percentage travels separately, so drop it here.
pub(crate) fn modifier_label(name: &str) -> String {
    let plain: String = name.chars().filter(|c| !matches!(c, '[' | ']')).collect();
    let plain = plain.trim_start_matches(|c: char| c == '+' || c == '-' || c.is_ascii_digit()).trim_start_matches('%').trim();
    match plain {
        "Tile" => "Terrain".into(),
        "" => "Other".into(),
        p => p.to_string(),
    }
}

fn entity<T>(arena: &Arena<T>, index: u32, generation: u32) -> Option<(Id<T>, &T)> {
    arena
        .iter()
        .find(|(id, _)| id.index() == index && id.generation() == generation)
}

pub fn unit_options(
    world: &World,
    r: &Ruleset,
    civ: CivId,
    index: u32,
    generation: u32,
) -> Option<UnitOptions> {
    let (id, unit) =
        entity(&world.units, index, generation).filter(|(_, unit)| unit.owner == civ)?;
    let mut actions = Vec::new();
    let mut offer = |action, availability| {
        actions.push(UnitActionChoice {
            action,
            availability,
        })
    };
    if unit.can_move() {
        offer(
            if unit.is_air(r) {
                UnitActionKind::Rebase
            } else {
                UnitActionKind::Move
            },
            Availability::NeedsTarget,
        );
        if unit.has_unique(r, UniqueType::NuclearWeapon) && unit.attacks_left > 0 {
            offer(UnitActionKind::NuclearStrike, Availability::NeedsTarget);
        }
        if unit.is_military(r) && unit.attacks_left > 0 {
            offer(UnitActionKind::Attack, Availability::NeedsTarget);
        }
        if civ_core::world::settling::is_settler(r, unit.base) {
            // Settlement spacing can depend on an unseen foreign city.
            offer(UnitActionKind::FoundCity, Availability::NeedsConfirmation);
        }
        if r.improvements
            .ids()
            .any(|imp| civ_core::world::improvements::can_work(world, r, id, imp))
        {
            offer(UnitActionKind::Improve, Availability::NeedsTarget);
        }
        if unit
            .all_uniques(r)
            .any(|u| u.unique_type() == Some(UniqueType::CanAirsweep))
        {
            offer(UnitActionKind::AirSweep, Availability::NeedsTarget);
        }
        if unit
            .all_uniques(r)
            .any(|u| u.unique_type() == Some(UniqueType::MayParadrop))
        {
            offer(UnitActionKind::Paradrop, Availability::NeedsTarget);
        }
        if civ_core::world::victory_actions::can_assemble(world, r, id) {
            offer(
                UnitActionKind::AssembleSpaceshipPart,
                Availability::Available,
            );
        }
        // These targetless operations read only the owned unit and the tile
        // it currently observes. Reuse the action boundary instead of a
        // second implementation of its fortification/pillage restrictions.
        for (kind, command) in [
            (UnitActionKind::Skip, Command::SkipUnit { unit: id }),
            (UnitActionKind::Fortify, Command::Fortify { unit: id }),
            (UnitActionKind::Pillage, Command::Pillage { unit: id }),
            (UnitActionKind::Disband, Command::DisbandUnit { unit: id }),
            (UnitActionKind::SetUp, Command::SetUp { unit: id }),
            (UnitActionKind::Gift, Command::GiftUnit { unit: id }),
        ] {
            if commands::execute(&mut world.clone(), r, civ, command).is_ok() {
                offer(kind, Availability::Available);
            }
        }
    }
    // Standing orders need no movement now; the unit acts when it next has some.
    for (kind, mode) in [
        (UnitActionKind::Automate, civ_core::world::automation::Automation::Work),
        (UnitActionKind::Explore, civ_core::world::automation::Automation::Explore),
    ] {
        if unit.automation != Some(mode) && civ_core::world::automation::can_automate(world, r, id, mode) {
            offer(kind, Availability::Available);
        }
    }
    let targets = {
        let offered = |kind: UnitActionKind| actions.iter().any(|a| a.action == kind);
        let mut out = Vec::new();
        for (kind, tiles) in [
            (UnitActionKind::Rebase, offered(UnitActionKind::Rebase).then(|| air::rebase_targets(world, r, id))),
            (UnitActionKind::AirSweep, offered(UnitActionKind::AirSweep).then(|| air::sweep_targets(world, r, id))),
            (UnitActionKind::Paradrop, offered(UnitActionKind::Paradrop).then(|| air::paradrop_targets(world, r, id))),
            (
                UnitActionKind::NuclearStrike,
                offered(UnitActionKind::NuclearStrike).then(|| nuclear_targets(world, r, civ, id)),
            ),
        ] {
            if let Some(tiles) = tiles {
                out.push(ActionTargets { action: kind, tiles: tiles.into_iter().map(|t| t.0).collect() });
            }
        }
        (!out.is_empty()).then_some(out)
    };
    let great_person_abilities = {
        use crate::protocol::GreatPersonAction as A;
        use civ_core::world::great_person_actions as gp;
        let mods = civ_core::world::TurnModifiers::for_world(world, r);
        let scaling = gp::Scaling { costs: mods.costs, science: mods.science };
        let abilities = gp::abilities(r, unit);
        (!abilities.is_empty()).then(|| {
            abilities
                .into_iter()
                .map(|ability| {
                    // The same check `great_person_actions` is filtered by.
                    let check = if unit.can_move() {
                        gp::can(world, r, id, ability, gp::Scaling::default()).map_err(|e| match e {
                            gp::ActionRejection::NothingResearched => GreatPersonRefusal::NothingResearched,
                            gp::ActionRejection::CannotBeHurried => GreatPersonRefusal::CannotBeHurried,
                            gp::ActionRejection::NotInOwnCity => GreatPersonRefusal::NotInOwnCity,
                            gp::ActionRejection::NotABuilding => GreatPersonRefusal::NotABuilding,
                            gp::ActionRejection::NothingToAdd => GreatPersonRefusal::NothingToAdd,
                            gp::ActionRejection::NotACityState => GreatPersonRefusal::NotACityState,
                            gp::ActionRejection::AtWar => GreatPersonRefusal::AtWar,
                            gp::ActionRejection::CannotBuildHere => GreatPersonRefusal::CannotBuildHere,
                            gp::ActionRejection::NoSuchUnit | gp::ActionRejection::NotAble => {
                                GreatPersonRefusal::NotAble
                            }
                        })
                    } else {
                        Err(GreatPersonRefusal::NoMovement)
                    };
                    let here = world
                        .cities
                        .iter()
                        .find(|(_, c)| c.centre == unit.position && c.owner == civ)
                        .map(|(city, _)| city);
                    let amount = match ability {
                        gp::Action::HurryResearch => Some(gp::science_from_scientist(world, civ, scaling.science)),
                        gp::Action::HurryProduction => {
                            here.and_then(|city| gp::hurry_amount(world, r, city, scaling.costs).ok())
                        }
                        gp::Action::TradeMission => Some(gp::gold_from_trade_mission(world, r, civ, scaling.costs)),
                        _ => None,
                    };
                    GreatPersonAbility {
                        action: match ability {
                            gp::Action::HurryResearch => A::HurryResearch,
                            gp::Action::HurryProduction => A::HurryProduction,
                            gp::Action::TradeMission => A::TradeMission,
                            gp::Action::GoldenAge => A::GoldenAge,
                            gp::Action::Construct(improvement) => A::Construct { improvement: improvement.index() },
                        },
                        available: check.is_ok(),
                        why: check.err(),
                        amount,
                    }
                })
                .collect()
        })
    };
    let promotions = if combat::can_promote(unit) {
        combat::available_promotions(r, unit)
            .into_iter()
            .map(|id| PromotionChoice {
                id: id.index(),
                name: r.promotions[id].name.clone(),
            })
            .collect()
    } else {
        Vec::new()
    };
    let upgrade =
        combat::upgrade_quote(world, r, id, civ_core::stats::Fixed::ONE).map(|(target, cost)| {
            UpgradeChoice {
                unit_type: target.index(),
                gold_cost: cost.to_string(),
                availability: Availability::NeedsConfirmation,
            }
        });
    let great_person_actions = {
        use crate::protocol::GreatPersonAction as A;
        use civ_core::world::great_person_actions as gp;
        let candidates = [
            (gp::Action::HurryResearch, A::HurryResearch),
            (gp::Action::HurryProduction, A::HurryProduction),
            (gp::Action::TradeMission, A::TradeMission),
            (gp::Action::GoldenAge, A::GoldenAge),
        ]
        .into_iter()
        .chain(r.improvements.ids().map(|id| {
            (
                gp::Action::Construct(id),
                A::Construct {
                    improvement: id.index(),
                },
            )
        }));
        candidates
            .filter(|(action, _)| {
                unit.can_move() && gp::can(world, r, id, *action, gp::Scaling::default()).is_ok()
            })
            .map(|(_, action)| action)
            .collect()
    };
    let religious_actions = {
        use crate::protocol::ReligiousAction as A;
        use civ_core::world::religious_units as ru;
        let mut choices = Vec::new();
        if ru::can(world, r, id, ru::ReligiousAction::ConstructHolySite).is_ok() {
            choices.push(A::ConstructHolySite);
        }
        for (city, _) in world
            .cities
            .iter()
            .filter(|(_, value)| world.visibility.can_see(civ, value.centre))
        {
            for (core, wire) in [
                (
                    ru::ReligiousAction::SpreadReligion(city),
                    A::SpreadReligion {
                        city: EntityRef {
                            id: city.index(),
                            gen: city.generation(),
                        },
                    },
                ),
                (
                    ru::ReligiousAction::RemoveHeresy(city),
                    A::RemoveHeresy {
                        city: EntityRef {
                            id: city.index(),
                            gen: city.generation(),
                        },
                    },
                ),
            ] {
                if ru::can(world, r, id, core).is_ok() {
                    choices.push(wire);
                }
            }
        }
        choices
    };
    Some(UnitOptions {
        targets,
        great_person_abilities,
        nuclear_preview: None,
        great_person_actions,
        religious_actions,
        religion: [false, true]
            .into_iter()
            .find(|enhancing| {
                civ_core::world::religious_units::can(
                    world,
                    r,
                    id,
                    if *enhancing {
                        civ_core::world::religious_units::ReligiousAction::EnhanceReligion
                    } else {
                        civ_core::world::religious_units::ReligiousAction::FoundReligion
                    },
                )
                .is_ok()
            })
            .map(|enhancing| ReligionOptions {
                enhancing,
                requirements: civ_core::world::religious_units::belief_requirements(
                    world, r, civ, enhancing,
                )
                .into_iter()
                .map(|(kind, count)| BeliefRequirement { kind, count })
                .collect(),
                choices: civ_core::world::religious_units::available_belief_choices(world, r)
                    .into_iter()
                    .map(|id| id.index())
                    .collect(),
            }),
        unit: EntityRef {
            id: index,
            gen: generation,
        },
        actions,
        promotions,
        upgrade,
        improvements: r
            .improvements
            .ids()
            .filter(|imp| civ_core::world::improvements::can_work(world, r, id, *imp))
            .map(|id| id.index())
            .collect(),
        disband_gold: civ_core::world::administration::disband_gold(
            world,
            r,
            id,
            civ_core::world::TurnModifiers::for_world(world, r).costs,
        ),
    })
}

/// Whether what a unique does could hinge on something outside this player's
/// knowledge: any conditional at all, or the city filter that consults the
/// creed founder's live religious status even without one.
fn private_condition(unique: &Unique) -> bool {
    !unique.conditionals.is_empty()
        || unique.params.iter().any(|p| {
            p.trim().trim_start_matches("in ")
                == "all cities in which the majority religion is a major religion"
        })
}

/// One of an item's own uniques that changes its production cost
/// (`construction::production_cost_in`) under a private condition.
fn private_production_cost(unique: &Unique) -> bool {
    matches!(
        unique.unique_type(),
        Some(UniqueType::CostIncreasesPerCity | UniqueType::CostPercentageChange)
    ) && private_condition(unique)
}

pub fn city_options(
    world: &World,
    r: &Ruleset,
    civ: CivId,
    index: u32,
    generation: u32,
) -> Option<CityOptions> {
    let (id, city) =
        entity(&world.cities, index, generation).filter(|(_, city)| city.owner == civ)?;
    let owner = world.civ(civ)?;
    let modifiers = civ_core::world::TurnModifiers::for_world(world, r).costs; // Core derives current city/empire context.
    let puppet_may_buy = city.uniques.has(UniqueType::MayBuyConstructionsInPuppets)
        || owner.uniques.has(UniqueType::MayBuyConstructionsInPuppets);
    // What `purchase::cost` reads besides the item's own uniques, narrowed to
    // those whose effect could hinge on something this player cannot see.
    let private_in_force: Vec<&Unique> = civ_core::world::religion::city_uniques(world, r, city)
        .into_iter()
        .filter(|u| private_condition(u))
        .collect();
    let mut choices = Vec::new();
    for what in r
        .buildings
        .ids()
        .map(Construction::Building)
        .chain(r.units.ids().map(Construction::Unit))
    {
        let local = construction::can_construct(r, owner, city, what);
        if local
            .as_ref()
            .is_err_and(|reason| !matches!(reason, Rejection::NotProducible))
        {
            continue;
        }
        let (reference, name, own, global_wonder, national_wonder) = match what {
            Construction::Building(id) => {
                let base = &r.buildings[id];
                (
                    ConstructionRef::Building { id: id.index() },
                    base.name.clone(),
                    &base.uniques,
                    base.is_wonder == Some(true),
                    base.is_national_wonder == Some(true),
                )
            }
            Construction::Unit(id) => (
                ConstructionRef::Unit { id: id.index() },
                r.units[id].name.clone(),
                &r.units[id].uniques,
                false,
                false,
            ),
        };
        // National wonders depend only on our own cities, which are known.
        if national_wonder
            && !global_wonder
            && construction::can_construct_in(world, r, civ, city, what).is_err()
        {
            continue;
        }
        // The queue's own check: policies, eras, uniques, terrain and tiles
        // are all ours to know, so a construction they rule out is not
        // offered (World Charter before Internationalism). Only whether a
        // world wonder stands somewhere unseen stays for the queue to answer,
        // and a purchase-only item is offered while it can be bought.
        let unseen = |e: &Rejection| global_wonder && matches!(e, Rejection::WonderAlreadyBuilt);
        let full = construction::can_construct_in(world, r, civ, city, what);
        if full.as_ref().is_err_and(|e| !unseen(e) && !matches!(e, Rejection::NotProducible))
            || (matches!(full, Err(Rejection::NotProducible))
                && construction::can_purchase_in(world, r, civ, city, what).is_err_and(|e| !unseen(&e)))
        {
            continue;
        }
        // A bought unit needs its kind's place in the city centre free, as
        // `purchase` checks; only our own units stand there.
        let occupied = match what {
            Construction::Unit(base) => r.units.get(base).is_some_and(|row| {
                let prospective = civ_core::world::Unit::new(base, civ, city.centre, row.movement);
                civ_core::world::movement::is_tile_occupied_for(world, r, city.centre, &prospective)
            }),
            Construction::Building(_) => false,
        };
        // How far a unique with a private condition reaches into this item's
        // price in `stat`. Only one that names this item (or all items) and
        // this stat can hide anything; the rest leave the quote exact.
        let hidden = |stat: Stat| -> Bearing {
            own.iter()
                .filter(|u| private_condition(u))
                .map(|u| purchase::own_bearing(u, stat))
                .chain(
                    private_in_force
                        .iter()
                        .map(|u| purchase::bearing(r, u, what, stat)),
                )
                .max()
                .unwrap_or(Bearing::Unrelated)
        };
        let unquoted = PriceQuote {
            amount: None,
            affordable: None,
            availability: Availability::NeedsConfirmation,
            refusal: None,
        };
        let quote = |stat: Stat| -> Option<PriceQuote> {
            let hidden = hidden(stat);
            // Whether it is for sale at all may be the hidden part.
            if hidden == Bearing::Sale {
                return Some(unquoted.clone());
            }
            // Not for sale in this stat, whatever the condition says.
            let amount = purchase::cost(world, r, id, what, stat, modifiers)?;
            if hidden == Bearing::Price {
                return Some(unquoted.clone());
            }
            let balance = match stat {
                Stat::Gold => owner.gold,
                Stat::Faith => owner.faith.stored,
                _ => 0,
            };
            let affordable = i64::from(balance) >= amount;
            Some(PriceQuote {
                amount: Some(amount.to_string()),
                affordable: Some(affordable),
                availability: Availability::NeedsConfirmation,
                refusal: if city.puppet && !puppet_may_buy {
                    Some(Refusal::Puppet)
                } else if city.resistance > 0 {
                    Some(Refusal::Resistance)
                } else if occupied {
                    Some(Refusal::Occupied)
                } else if !affordable {
                    Some(Refusal::CannotAfford)
                } else {
                    None
                },
            })
        };
        let gold = quote(Stat::Gold);
        let faith = quote(Stat::Faith);
        if local.is_err() && gold.is_none() && faith.is_none() {
            continue;
        }
        let needs_confirmation = local.is_err()
            || city.puppet
            || city.resistance > 0
            || global_wonder
            || matches!(what, Construction::Unit(_))
            || own
                .iter()
                .any(|u| u.unique_type() == Some(UniqueType::OnlyAvailable));
        choices.push(ConstructionChoice {
            construction: reference,
            name,
            // Hidden only when the item's own cost rides on a private
            // condition; a purchase price's condition never hides it.
            production_cost: if local.is_ok() && !own.iter().any(private_production_cost) {
                construction::production_cost_in(world, r, id, what, modifiers)
            } else {
                None
            },
            availability: if needs_confirmation {
                Availability::NeedsConfirmation
            } else {
                Availability::Available
            },
            gold,
            faith,
            sites: match what {
                Construction::Building(b) if civ_core::world::placement::is_placed(r, b) => Some(SiteChoice {
                    tiles: civ_core::world::placement::valid_sites(world, r, id, b).into_iter().map(|t| t.0).collect(),
                    selected: civ_core::world::placement::site_for(world, r, id, b).map(|t| t.0),
                    takes_tile: civ_core::world::placement::takes_tile(r, b),
                    best: civ_core::world::placement::best_site(world, r, id, b).map(|t| t.0),
                }),
                _ => None,
            },
            // The queue refuses a puppet's and a resisting city's work first.
            refusal: if city.puppet {
                Some(Refusal::Puppet)
            } else if city.resistance > 0 {
                Some(Refusal::Resistance)
            } else if local.is_err() {
                Some(Refusal::PurchaseOnly)
            } else {
                None
            },
        });
    }
    Some(CityOptions {
        production_conversions: [
            crate::protocol::ProductionConversion::Gold,
            crate::protocol::ProductionConversion::Science,
        ]
        .into_iter()
        .filter(|s| {
            civ_core::world::administration::can_convert_production(world, r, id, Some(s.stat()))
        })
        .collect(),
        city: EntityRef {
            id: index,
            gen: generation,
        },
        choices,
        workable_tiles: civ_core::world::city::workable_tiles(&world.grid, city)
            .into_iter()
            .map(|tile| {
                let y = civ_core::world::yields::tile_yield_for(&world.grid, r, tile, owner);
                WorkableTile {
                    tile: tile.0,
                    food: y.food.round() as i32,
                    production: y.production.round() as i32,
                    gold: y.gold.round() as i32,
                    science: y.science.round() as i32,
                    culture: y.culture.round() as i32,
                    faith: y.faith.round() as i32,
                    worked: city.worked.contains(&tile),
                    taken: world.cities.iter().any(|(other, c)| other != id && c.worked.contains(&tile)),
                }
            })
            .collect(),
        bombard: if city.attacks_left > 0 && city.resistance == 0 {
            combat::bombard_targets(world, r, id)
                .into_iter()
                .filter_map(|target| {
                    let (low, high) = combat::preview_bombard(world, r, id, target)?;
                    Some(BombardTarget {
                        unit: EntityRef { id: target.index(), gen: target.generation() },
                        health: i32::from(world.units.get(target)?.health),
                        damage_min: low,
                        damage_max: high,
                    })
                })
                .collect()
        } else {
            Vec::new()
        },
        specialist_slots: {
            let bonus = civ_core::world::specialists::bonus_per_specialist(world, r, id);
            civ_core::world::specialists::slots(r, city)
                .into_iter()
                .map(|(specialist, slots)| SpecialistSlot {
                    specialist: specialist.index(),
                    slots,
                    yields: crate::rules::Yields::of(civ_core::world::specialists::base_stats(r, specialist) + bonus),
                })
                .collect()
        },
        sellable_buildings: r
            .buildings
            .ids()
            .filter_map(|building| {
                civ_core::world::administration::sell_price(world, r, id, building).map(|gold| {
                    SellBuildingChoice {
                        building: building.index(),
                        gold,
                    }
                })
            })
            .collect(),
        // Priced as `buyTile` charges, with every tile it would accept but
        // for the gold.
        purchasable_tiles: world
            .grid
            .within(city.centre, 3)
            .into_iter()
            .filter(|tile| world.visibility.can_see(civ, *tile))
            .filter_map(|tile| {
                civ_core::world::expansion::tile_price(world, r, id, tile, modifiers.gold_cost)
                    .ok()
                    .map(|gold| TilePurchaseChoice {
                        tile: tile.0,
                        gold: gold.to_string(),
                        affordable: i64::from(owner.gold) >= gold,
                    })
            })
            .collect(),
    })
}

pub fn civ_options(world: &World, r: &Ruleset, civ: CivId) -> CivOptions {
    let Some(owner) = world.civ(civ) else {
        return CivOptions {
            state_faith: None,
            pantheon_cost: 0,
            pantheons: Vec::new(),
            research: Vec::new(),
            policies: Vec::new(),
            next_policy_cost: 0,
            free_technologies: Vec::new(),
            free_great_people: Vec::new(),
        };
    };
    let research = research::available_techs(r, owner)
        .into_iter()
        .filter_map(|id| {
            Some(ResearchChoice {
                id: id.index(),
                name: r.technologies[id].name.clone(),
                selected: owner.researching == Some(id),
                science_cost: research::tech_cost_in(
                    world,
                    r,
                    civ,
                    id,
                    civ_core::world::TurnModifiers::for_world(world, r).science,
                )?,
                invested: research::invested(owner, id),
            })
        })
        .collect();
    let next_policy_cost =
        policies::policy_cost_in(world, r, civ, civ_core::world::TurnModifiers::for_world(world, r).policies);
    let policies = r
        .policies
        .iter()
        .filter(|(id, _)| !owner.adopted_policies.contains(id))
        .filter_map(|(id, policy)| {
            let conditional = policy
                .uniques
                .iter()
                .any(|u| u.unique_type() == Some(UniqueType::OnlyAvailable));
            if !conditional && policies::is_adoptable(world, r, civ, id).is_err() {
                return None;
            }
            Some(PolicyChoice {
                id: id.index(),
                name: policy.name.clone(),
                affordable: owner.pending_rewards.free_policies > 0
                    || owner.culture_stored >= next_policy_cost,
                availability: if conditional {
                    Availability::NeedsConfirmation
                } else {
                    Availability::Available
                },
            })
        })
        .collect();
    CivOptions {
        state_faith: r.faith_mode().then(|| {
            let current = owner.faith.state;
            let free_at = owner.faith.state_since.saturating_add(civ_core::world::faiths::STATE_FAITH_LOCK);
            StateFaithOptions {
                current: current.map(|k| k.0),
                adoptable: civ_core::world::faiths::adoptable(world, r, civ)
                    .into_iter()
                    .filter(|k| civ_core::world::faiths::can_adopt(world, r, civ, *k).is_ok())
                    .map(|k| k.0)
                    .collect(),
                locked_turns: if current.is_some() { free_at.saturating_sub(world.turn) } else { 0 },
            }
        }),
        pantheon_cost: civ_core::world::religion::pantheon_cost(world, civ_core::stats::Fixed::ONE),
        pantheons: if civ_core::world::religion::can_found_pantheon(world, r, civ) {
            civ_core::world::religion::available_beliefs(world, r, "Pantheon")
                .into_iter()
                .map(|id| id.index())
                .collect()
        } else {
            Vec::new()
        },
        research,
        policies,
        next_policy_cost,
        free_technologies: if owner.pending_rewards.free_techs > 0 {
            research::available_techs(r, owner)
                .into_iter()
                .map(|id| id.index())
                .collect()
        } else {
            Vec::new()
        },
        free_great_people: if owner.pending_rewards.free_great_people > 0 {
            r.units
                .ids()
                .filter(|id| {
                    civ_core::world::effects::claim_free_great_person(
                        &mut world.clone(),
                        r,
                        civ,
                        *id,
                    )
                    .is_some()
                })
                .map(|id| id.index())
                .collect()
        } else {
            Vec::new()
        },
    }
}
