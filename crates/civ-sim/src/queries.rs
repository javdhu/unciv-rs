//! Advisory options derived from owned state and public rules. Queries do not
//! probe hidden world-wide uniqueness or resource conditions to explain why an
//! action might fail; execution remains authoritative.
use crate::protocol::{AttackTarget, ConstructionRef, EntityRef};
use civ_core::arena::{Arena, Id};
use civ_core::ruleset::Ruleset;
use civ_core::stats::Stat;
use civ_core::world::commands::{self, Command};
use civ_core::world::construction::{self, Construction, CostModifiers, Rejection};
use civ_core::modifiers::Layer;
use civ_core::stats::Fixed;
use civ_core::world::{air, combat, policies, purchase, research, CivId, World};
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS, schemars::JsonSchema)]
pub struct PriceQuote {
    /// Exact decimal amount, or unknown when a conditional price could depend
    /// on information outside this player's knowledge.
    pub amount: Option<String>,
    pub affordable: Option<bool>,
    pub availability: Availability,
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
    Some(CombatPreviewView { ranged: p.ranged, capture: p.capture, attacker: preview_side(&p.attacker), defender: preview_side(&p.defender) })
}

fn preview_side(s: &combat::PreviewSide) -> CombatSideView {
    let tenths = |v: Fixed| (v * Fixed::whole(10)).round() as i32;
    CombatSideView {
        strength: tenths(s.strength),
        base: tenths(s.base),
        modifiers: s
            .modifiers
            .iter()
            .filter(|m| matches!(m.layer, Layer::Additive | Layer::Multiplicative) && m.amount != Fixed::ZERO)
            .map(|m| ModifierLine { label: modifier_label(&m.source.name), percent: m.amount.round() as i32 })
            .collect(),
        health: s.health,
        max_health: s.max_health,
        damage_min: s.damage.0,
        damage_max: s.damage.1,
    }
}

/// Engine sources are short phrases ("Flanking") or ability text with bracketed parameters
/// ("+[50]% Strength vs [Mounted] units"). The percentage travels separately, so drop it here.
fn modifier_label(name: &str) -> String {
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
            CostModifiers::default(),
        ),
    })
}

fn conditional_price(unique: &Unique) -> bool {
    let price_rule = matches!(
        unique.unique_type(),
        Some(
            UniqueType::BuyItemsDiscount
                | UniqueType::CostIncreasesPerCity
                | UniqueType::CostPercentageChange
                | UniqueType::BuyUnitsDiscount
                | UniqueType::BuyBuildingsDiscount
                | UniqueType::BuyUnitsIncreasingCost
                | UniqueType::BuyBuildingsIncreasingCost
                | UniqueType::BuyUnitsByProductionCost
                | UniqueType::BuyBuildingsByProductionCost
                | UniqueType::BuyUnitsWithStat
                | UniqueType::BuyBuildingsWithStat
                | UniqueType::BuyUnitsForAmountStat
                | UniqueType::BuyBuildingsForAmountStat
                | UniqueType::CanBePurchasedForAmountStat
                | UniqueType::CanBePurchasedWithStat
                | UniqueType::CannotBePurchased
                | UniqueType::Unbuildable
        )
    );
    // This filter consults the creed founder's live religious status, even
    // without an explicit conditional. Keep that status out of a quote.
    price_rule
        && (!unique.conditionals.is_empty()
            || unique.params.iter().any(|p| {
                p.trim().trim_start_matches("in ")
                    == "all cities in which the majority religion is a major religion"
            }))
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
    let modifiers = CostModifiers::default(); // Core derives current city/empire context.
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
        let has_conditional_price = own.iter().any(conditional_price)
            || owner.uniques.all().any(conditional_price)
            || city.uniques.all().any(conditional_price);
        let quote = |stat: Stat| -> Option<PriceQuote> {
            if has_conditional_price {
                return Some(PriceQuote {
                    amount: None,
                    affordable: None,
                    availability: Availability::NeedsConfirmation,
                });
            }
            let amount = purchase::cost(world, r, id, what, stat, modifiers)?;
            let balance = match stat {
                Stat::Gold => owner.gold,
                Stat::Faith => owner.faith.stored,
                _ => 0,
            };
            Some(PriceQuote {
                amount: Some(amount.to_string()),
                affordable: Some(i64::from(balance) >= amount),
                availability: Availability::NeedsConfirmation,
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
            production_cost: if local.is_ok() && !has_conditional_price {
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
        specialist_slots: civ_core::world::specialists::slots(r, city)
            .into_iter()
            .map(|(specialist, slots)| SpecialistSlot { specialist: specialist.index(), slots })
            .collect(),
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
        purchasable_tiles: world
            .grid
            .within(city.centre, 3)
            .into_iter()
            .filter(|tile| world.visibility.can_see(civ, *tile))
            .filter_map(|tile| {
                civ_core::world::expansion::buy_tile(
                    &mut world.clone(),
                    r,
                    id,
                    tile,
                    civ_core::stats::Fixed::ONE,
                )
                .ok()
                .map(|gold| TilePurchaseChoice {
                    tile: tile.0,
                    gold: gold.to_string(),
                })
            })
            .collect(),
    })
}

pub fn civ_options(world: &World, r: &Ruleset, civ: CivId) -> CivOptions {
    let Some(owner) = world.civ(civ) else {
        return CivOptions {
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
                    research::ScienceModifiers::default(),
                )?,
                invested: research::invested(owner, id),
            })
        })
        .collect();
    let next_policy_cost =
        policies::policy_cost_in(world, r, civ, policies::PolicyModifiers::default());
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
