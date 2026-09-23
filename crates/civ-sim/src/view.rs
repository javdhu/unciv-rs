//! One civ's view of the world, and the turn report. This is the fog-of-war boundary: nothing a civ may not
//! know leaves through here. When the engine grows its own `view(world, civ)`, this module delegates to it.

use civ_core::arena::Id;
use civ_core::ruleset::Ruleset;
use civ_core::world::assignment::CityFocus;
use civ_core::world::construction::Construction;
use civ_core::world::fog::{capture_observations, reachable_preview};
use civ_core::world::grid::RoadLevel;
use civ_core::world::turn::{advance_turn, TurnEvent, TurnModifiers};
use civ_core::world::{CivId, World};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct PlayerView {
    pub turn: u32,
    /// The calendar year the turn falls in, negative for BC, from the game speed's table of years per turn.
    pub year: i32,
    pub width: u16,
    pub height: u16,
    pub wrap_x: bool,
    pub me: u8,
    pub winner: Option<u8>,
    pub result: Option<MatchResultView>,
    pub endgame: EndgameView,
    pub rewards: RewardsView,
    pub knowledge: KnowledgeView,
    pub economy: EconomyView,
    pub relations: Vec<RelationView>,
    pub city_states: Vec<crate::city_states::CityStateRelationsView>,
    pub spies: Vec<SpyView>,
    pub tiles: Vec<TileView>,
    pub units: Vec<UnitView>,
    pub cities: Vec<CityView>,
    pub civs: Vec<CivView>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS, schemars::JsonSchema)]
pub struct EconomyView {
    pub gold: i32,
    pub science: i32,
    pub culture: i32,
    pub faith: i32,
    pub happiness: i32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct KnowledgeView {
    pub technologies: Vec<u32>,
    pub policies: Vec<u32>,
    pub beliefs: Vec<u32>,
    pub timed_policies: Vec<TimedPolicyView>,
    pub faith: i32,
    pub religion_founded: bool,
    pub religion_enhanced: bool,
    pub research_progress: Vec<ResearchProgressView>,
    pub resources: Vec<ResourceStockView>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SpyView {
    pub id: usize,
    pub rank: u8,
    pub station: Option<EntityRef>,
    pub action: SpyActionView,
    pub turns_remaining: u32,
    pub progress: i32,
}

#[derive(
    Debug, Clone, Copy, PartialEq, Serialize, Deserialize, ts_rs::TS, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub enum SpyActionView {
    Hideout,
    Moving,
    Dead,
    Surveillance,
    EstablishingNetwork,
    StealingTech,
    CounterIntelligence,
    RiggingElections,
    Coup,
}

fn spy_view(id: usize, spy: &civ_core::world::espionage::Spy) -> SpyView {
    use civ_core::world::espionage::SpyAction as A;
    SpyView {
        id,
        rank: spy.rank,
        station: spy.station.map(Into::into),
        turns_remaining: spy.turns_remaining,
        progress: spy.progress,
        action: match spy.action {
            A::Hideout => SpyActionView::Hideout,
            A::Moving => SpyActionView::Moving,
            A::Dead => SpyActionView::Dead,
            A::Surveillance => SpyActionView::Surveillance,
            A::EstablishingNetwork => SpyActionView::EstablishingNetwork,
            A::StealingTech => SpyActionView::StealingTech,
            A::CounterIntelligence => SpyActionView::CounterIntelligence,
            A::RiggingElections => SpyActionView::RiggingElections,
            A::Coup => SpyActionView::Coup,
        },
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TimedPolicyView {
    pub policy: u32,
    pub expires_on: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS, schemars::JsonSchema)]
pub struct ResearchProgressView {
    pub technology: u32,
    pub stored: i32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS, schemars::JsonSchema)]
pub struct ResourceStockView {
    pub resource: u32,
    pub amount: i32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RelationView {
    pub civ: u8,
    pub at_war: bool,
    pub our_embassy: bool,
    pub their_embassy: bool,
    pub friendship_turns: u32,
    pub defensive_pact_turns: u32,
    pub peace_treaty_turns: u32,
    pub research_agreement_turns: Option<u32>,
    pub protecting: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RewardsView {
    pub technologies: u32,
    pub policies: u32,
    pub great_people: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct EndgameView {
    pub max_turns: u32,
    pub enabled_victories: Vec<u32>,
    /// Only this player's progress is published.
    pub victories: Vec<VictoryProgressView>,
    pub spaceship_parts: Vec<SpaceshipPartView>,
    pub voting: Option<VotingView>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS, schemars::JsonSchema)]
pub struct VictoryProgressView {
    pub victory: u32,
    pub completed: usize,
    pub total: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS, schemars::JsonSchema)]
pub struct SpaceshipPartView {
    pub part: u32,
    pub assembled: u32,
    pub needed: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct VotingView {
    pub opens_on: Option<u32>,
    pub may_vote: bool,
    pub has_voted: bool,
    pub candidates: Vec<u8>,
    pub last_result: Option<VoteResultView>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS, schemars::JsonSchema)]
pub struct VoteResultView {
    pub turn: u32,
    pub votes: Vec<VoteTotalView>,
    pub needed: u32,
    pub winner: Option<u8>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS, schemars::JsonSchema)]
pub struct VoteTotalView {
    pub candidate: u8,
    pub votes: u32,
}

fn endgame_view(w: &World, r: &Ruleset, me: CivId) -> EndgameView {
    use civ_core::world::{victory, victory_actions as actions};
    let enabled: Vec<_> = r
        .victories
        .iter()
        .filter(|(_, v)| victory::is_enabled(w, r, &v.name))
        .collect();
    EndgameView {
        max_turns: w.endgame.max_turns,
        enabled_victories: enabled.iter().map(|(id, _)| id.index()).collect(),
        victories: enabled
            .iter()
            .map(|(id, v)| {
                let progress = victory::progress(w, r, me, v);
                VictoryProgressView {
                    victory: id.index(),
                    completed: progress.iter().filter(|(_, v)| v.holds()).count(),
                    total: progress.len(),
                }
            })
            .collect(),
        spaceship_parts: enabled
            .iter()
            .flat_map(|(_, v)| victory::required_parts(r, v))
            .map(|(part, needed)| SpaceshipPartView {
                part: part.index(),
                assembled: actions::assembled_parts(w, me, part),
                needed,
            })
            .collect(),
        voting: w.endgame.voting.as_ref().map(|v| VotingView {
            opens_on: v.opens_on,
            may_vote: actions::may_vote(w, me),
            has_voted: v.ballots.contains_key(&me),
            candidates: actions::candidates(w, me)
                .into_iter()
                .map(|c| c.0)
                .collect(),
            last_result: v.last_result.as_ref().map(|result| VoteResultView {
                turn: result.turn,
                needed: result.needed,
                winner: result.winner.map(|c| c.0),
                votes: result
                    .votes
                    .iter()
                    .map(|(candidate, votes)| VoteTotalView {
                        candidate: candidate.0,
                        votes: *votes,
                    })
                    .collect(),
            }),
        }),
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct MatchResultView {
    pub winner: u8,
    pub condition: u32,
    pub turn: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TileView {
    pub id: u16,
    pub terrain: u32,
    pub features: Vec<u32>,
    pub natural_wonder: Option<u32>,
    pub resource: Option<u32>,
    pub improvement: Option<u32>,
    pub improvement_pillaged: bool,
    /// 0 none, 1 road, 2 railroad.
    pub road: u8,
    pub road_pillaged: bool,
    pub river_edges: u8,
    pub owner: Option<u8>,
    pub visible: bool,
    pub observed_turn: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct UnitView {
    pub id: u32,
    pub gen: u32,
    pub base: u32,
    pub owner: u8,
    pub tile: u16,
    pub health: u8,
    pub moves: f64,
    pub fortified: bool,
    pub embarked: bool,
    /// Private action and progression state is supplied only to the owner.
    pub own: Option<UnitStateView>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct UnitStateView {
    pub attacks_left: u8,
    pub set_up: bool,
    pub fortification_turns: u8,
    pub promotions_earned: u16,
    pub experience: u16,
    pub movement_limit: i32,
    pub promotions: Vec<u32>,
    pub carrier: Option<crate::protocol::EntityRef>,
    pub religion: Option<u8>,
    pub religious_uses_left: u8,
    /// The unit's standing order, if it has one (`automateUnit`/`exploreUnit`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub automation: Option<UnitAutomation>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum UnitAutomation {
    Work,
    Explore,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct CityView {
    pub id: u32,
    pub gen: u32,
    pub name: String,
    pub owner: u8,
    pub centre: u16,
    pub population: u32,
    pub tiles: Vec<u16>,
    /// Own cities only.
    pub queue: Option<Vec<String>>,
    pub food_stored: Option<i32>,
    pub production_stored: Option<i32>,
    pub production_overflow: Option<i32>,
    pub production_progress: Option<Vec<ProductionProgressView>>,
    pub own: Option<CityStateView>,
    pub visible: bool,
    pub observed_turn: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct CityStateView {
    pub religion: CityReligionView,
    pub buildings: Vec<u32>,
    pub free_buildings: Vec<u32>,
    pub worked: Vec<u16>,
    pub specialists: Vec<SpecialistAllocationView>,
    pub health: i32,
    pub max_health: i32,
    /// Food the city needs stored to grow by one citizen.
    pub food_to_grow: i32,
    pub attacks_left: u8,
    pub resistance: u32,
    pub puppet: bool,
    pub razing: bool,
    pub avoid_growth: bool,
    pub sold_building_this_turn: bool,
    pub may_liberate: bool,
    pub production_conversion: Option<crate::protocol::ProductionConversion>,
    pub food_per_turn: i32,
    pub production_per_turn: i32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct CityReligionView {
    pub majority: Option<u8>,
    pub majority_pantheon: Option<u8>,
    pub holy_city_of: Option<u8>,
    pub holy_city_blocked: bool,
    pub followers: Vec<CityFollowersView>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS, schemars::JsonSchema)]
pub struct CityFollowersView {
    pub founder: u8,
    pub pantheon: bool,
    pub followers: u32,
    pub pressure: i32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS, schemars::JsonSchema)]
pub struct SpecialistAllocationView {
    pub specialist: u32,
    pub count: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProductionProgressView {
    pub construction: crate::protocol::ConstructionRef,
    pub stored: i32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct CivView {
    pub id: u8,
    pub nation: u32,
    pub city_state: bool,
    pub alive: bool,
    /// Own civ only, from here down.
    pub gold: Option<i32>,
    pub science: Option<i32>,
    pub culture: Option<i32>,
    pub researching: Option<String>,
    pub techs: Option<usize>,
    pub observed_turn: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TurnReport {
    pub turn: u32,
    pub events: Vec<EventView>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct EventView {
    /// Stable presentation name, independent of engine Debug output.
    pub kind: String,
    pub text: String,
    pub event: PresentationEvent,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Reach {
    pub tile: u16,
    /// Estimated remaining movement; uncertain routes may stop early.
    pub moves_left: f64,
    pub uncertain: bool,
}

fn moves(raw: i64) -> f64 {
    raw as f64 / 1000.0
}

pub fn player_view(w: &World, r: &Ruleset, me: CivId) -> PlayerView {
    let economy =
        civ_core::world::turn::plan_economy(w, r, me, CityFocus::None, TurnModifiers::default());
    let tiles = w
        .visibility
        .known_tiles(me)
        .map(|(tile, seen)| TileView {
            id: tile.0,
            terrain: seen.terrain.index(),
            features: seen.features.iter().map(|f| f.index()).collect(),
            natural_wonder: seen.natural_wonder.map(|n| n.index()),
            resource: seen.revealed_resource(w, r, me).map(|id| id.index()),
            improvement: seen.improvement.map(|id| id.index()),
            improvement_pillaged: seen.improvement_pillaged,
            road_pillaged: seen.road_pillaged,
            road: match seen.road {
                RoadLevel::None => 0,
                RoadLevel::Road => 1,
                RoadLevel::Railroad => 2,
            },
            river_edges: seen.river_edges,
            owner: seen.owner.map(|c| c.0),
            visible: w.visibility.can_see(me, tile),
            observed_turn: seen.observed_turn,
        })
        .collect();
    let units = w
        .units
        .iter()
        .filter(|(id, _)| civ_core::world::fog::can_see_unit(w, r, me, *id))
        .map(|(id, u)| UnitView {
            id: id.index(),
            gen: id.generation(),
            base: u.base.index(),
            owner: u.owner.0,
            tile: u.position.0,
            health: u.health,
            moves: moves(u.movement_left.raw()),
            fortified: u.fortified,
            embarked: u.embarked,
            own: (u.owner == me).then(|| UnitStateView {
                attacks_left: u.attacks_left,
                set_up: u.set_up,
                fortification_turns: u.fortification_turns,
                promotions_earned: u.promotions_earned,
                experience: u.xp,
                movement_limit: civ_core::world::movement::max_movement(w, r, u),
                promotions: u.promotions.iter().map(|id| id.index()).collect(),
                carrier: u.transport.map(|id| crate::protocol::EntityRef {
                    id: id.index(),
                    gen: id.generation(),
                }),
                religion: u.religion.map(|c| c.0),
                religious_uses_left: u.religious_uses_left,
                automation: u.automation.map(|a| match a {
                    civ_core::world::automation::Automation::Work => UnitAutomation::Work,
                    civ_core::world::automation::Automation::Explore => UnitAutomation::Explore,
                }),
            }),
        })
        .collect();
    let mut cities: Vec<_> = w
        .visibility
        .known_cities(me)
        .filter(|c| c.owner != me)
        .map(|c| CityView {
            id: c.id.index(),
            gen: c.id.generation(),
            name: c.name.clone(),
            owner: c.owner.0,
            centre: c.centre.0,
            population: c.population,
            tiles: c.tiles.iter().map(|t| t.0).collect(),
            queue: None,
            food_stored: None,
            production_stored: None,
            production_overflow: None,
            production_progress: None,
            own: None,
            visible: w.visibility.can_see(me, c.centre),
            observed_turn: c.observed_turn,
        })
        .collect();
    for (id, c) in w.cities.iter().filter(|(_, c)| c.owner == me) {
        cities.retain(|known| known.centre != c.centre.0);
        cities.push(CityView {
            id: id.index(),
            gen: id.generation(),
            name: c.name.clone(),
            owner: me.0,
            centre: c.centre.0,
            population: c.population,
            tiles: c.tiles.iter().map(|t| t.0).collect(),
            queue: Some(
                c.queue
                    .iter()
                    .map(|q| match q {
                        Construction::Building(b) => r.buildings[*b].name.clone(),
                        Construction::Unit(u) => r.units[*u].name.clone(),
                    })
                    .collect(),
            ),
            own: Some(CityStateView {
                religion: CityReligionView {
                    majority: c.faith.majority(c.population).map(|id| id.0),
                    majority_pantheon: c.faith.majority_pantheon(c.population).map(|id| id.0),
                    holy_city_of: c.faith.holy_city_of.map(|id| id.0),
                    holy_city_blocked: c.faith.holy_city_blocked,
                    followers: c
                        .faith
                        .pressures()
                        .filter_map(|(creed, pressure)| {
                            creed.map(|id| CityFollowersView {
                                founder: id.0,
                                pantheon: false,
                                followers: c.faith.followers_of(id),
                                pressure,
                            })
                        })
                        .chain(c.faith.pantheon_pressures().map(|(id, pressure)| {
                            CityFollowersView {
                                founder: id.0,
                                pantheon: true,
                                followers: c
                                    .faith
                                    .pantheon_followers()
                                    .find(|(c, _)| *c == id)
                                    .map_or(0, |(_, n)| n),
                                pressure,
                            }
                        }))
                        .collect(),
                },
                buildings: c.built.iter().map(|id| id.index()).collect(),
                free_buildings: c.free_buildings.iter().map(|id| id.index()).collect(),
                worked: c.worked.iter().map(|t| t.0).collect(),
                specialists: c
                    .specialists
                    .iter()
                    .map(|(id, count)| SpecialistAllocationView {
                        specialist: id.index(),
                        count: *count,
                    })
                    .collect(),
                health: c.health,
                max_health: civ_core::world::city::max_health(r, c),
                food_to_grow: civ_core::world::city::food_to_next_population(c.population, TurnModifiers::default().growth),
                attacks_left: c.attacks_left,
                resistance: c.resistance,
                puppet: c.puppet,
                razing: c.razing,
                avoid_growth: c.avoid_growth,
                sold_building_this_turn: c.sold_building_this_turn,
                may_liberate: civ_core::world::conquest::can_liberate(w, id),
                production_conversion: c.production_conversion.and_then(|s| match s {
                    civ_core::stats::Stat::Gold => {
                        Some(crate::protocol::ProductionConversion::Gold)
                    }
                    civ_core::stats::Stat::Science => {
                        Some(crate::protocol::ProductionConversion::Science)
                    }
                    _ => None,
                }),
                food_per_turn: economy
                    .food
                    .iter()
                    .find(|(city, _)| *city == id)
                    .map_or(0, |(_, food)| *food),
                production_per_turn: economy
                    .production
                    .iter()
                    .find(|(city, _)| *city == id)
                    .map_or(0, |(_, production)| *production),
            }),
            food_stored: Some(c.food_stored),
            production_stored: Some(c.current_production()),
            production_overflow: Some(c.production_overflow),
            production_progress: Some(
                c.production_progress
                    .iter()
                    .map(|(what, stored)| ProductionProgressView {
                        construction: match what {
                            Construction::Building(id) => {
                                crate::protocol::ConstructionRef::Building { id: id.index() }
                            }
                            Construction::Unit(id) => {
                                crate::protocol::ConstructionRef::Unit { id: id.index() }
                            }
                        },
                        stored: *stored,
                    })
                    .collect(),
            ),
            visible: true,
            observed_turn: w.turn,
        });
    }
    cities.sort_by_key(|c| (c.id, c.gen));
    let mut civs: Vec<_> = w
        .visibility
        .known_civs(me)
        .filter(|c| c.id != me)
        .map(|c| CivView {
            id: c.id.0,
            nation: c.nation.index(),
            city_state: c.city_state,
            alive: c.alive,
            gold: None,
            science: None,
            culture: None,
            researching: None,
            techs: None,
            observed_turn: c.observed_turn,
        })
        .collect();
    if let Some(c) = w.civ(me) {
        civs.push(CivView {
            id: me.0,
            nation: c.nation.index(),
            city_state: c.is_city_state(),
            alive: c.is_alive(),
            gold: Some(c.gold),
            science: Some(
                c.researching
                    .map_or(0, |tech| civ_core::world::research::invested(c, tech))
                    + c.science_stored,
            ),
            culture: Some(c.culture_stored),
            researching: c.researching.map(|t| r.technologies[t].name.clone()),
            techs: Some(c.techs.len()),
            observed_turn: w.turn,
        });
    }
    civs.sort_by_key(|c| c.id);
    PlayerView {
        economy: EconomyView {
            gold: economy.gold,
            science: economy.science,
            culture: economy.culture,
            faith: economy.faith,
            happiness: economy.happiness,
        },
        turn: w.turn,
        // the session plays at the default speed, so the calendar reads the same modifier the turns use
        year: civ_core::world::great_people::calendar_year(r, TurnModifiers::default().costs.speed, w.turn),
        width: w.grid.width(),
        height: w.grid.height(),
        wrap_x: w.grid.wraps_x(),
        me: me.0,
        endgame: endgame_view(w, r, me),
        spies: w.civ(me).map_or_else(Vec::new, |c| {
            c.espionage
                .spies
                .iter()
                .enumerate()
                .map(|(id, spy)| spy_view(id, spy))
                .collect()
        }),
        city_states: crate::city_states::for_player(w, r, me),
        relations: w
            .civs
            .iter()
            .filter(|c| c.id != me && w.diplomacy.have_met(me, c.id))
            .map(|c| {
                let pact = w.diplomacy.pact(me, c.id);
                RelationView {
                    civ: c.id.0,
                    at_war: w.diplomacy.at_war(me, c.id),
                    our_embassy: w.diplomacy.has_embassy(me, c.id),
                    their_embassy: w.diplomacy.has_embassy(c.id, me),
                    friendship_turns: pact.map_or(0, |p| p.friendship_turns),
                    defensive_pact_turns: pact.map_or(0, |p| p.defensive_pact_turns),
                    peace_treaty_turns: pact.map_or(0, |p| p.peace_treaty_turns),
                    research_agreement_turns: pact
                        .and_then(|p| p.research.as_ref())
                        .map(|p| p.turns_left),
                    protecting: w.diplomacy.is_protector(me, c.id),
                }
            })
            .collect(),
        knowledge: {
            let c = w.civ(me);
            KnowledgeView {
                technologies: c
                    .into_iter()
                    .flat_map(|c| c.techs.iter())
                    .map(|id| id.index())
                    .collect(),
                policies: c
                    .into_iter()
                    .flat_map(|c| c.adopted_policies.iter())
                    .map(|id| id.index())
                    .collect(),
                beliefs: c
                    .into_iter()
                    .flat_map(|c| c.faith.beliefs.iter())
                    .map(|id| id.index())
                    .collect(),
                timed_policies: c
                    .into_iter()
                    .flat_map(|c| &c.timed_policy_expirations)
                    .filter(|(_, expires)| **expires > w.turn)
                    .map(|(policy, expires)| TimedPolicyView {
                        policy: policy.index(),
                        expires_on: *expires,
                    })
                    .collect(),
                faith: c.map_or(0, |c| c.faith.stored),
                religion_founded: c.is_some_and(|c| c.faith.founded),
                religion_enhanced: c.is_some_and(|c| c.faith.enhanced),
                research_progress: c
                    .into_iter()
                    .flat_map(|c| &c.science_invested)
                    .map(|(tech, stored)| ResearchProgressView {
                        technology: tech.index(),
                        stored: *stored,
                    })
                    .collect(),
                resources: civ_core::world::resources::available_stock(w, r, me)
                    .into_iter()
                    .map(|(resource, amount)| ResourceStockView {
                        resource: resource.index(),
                        amount,
                    })
                    .collect(),
            }
        },
        rewards: w
            .civ(me)
            .map(|c| RewardsView {
                technologies: c.pending_rewards.free_techs,
                policies: c.pending_rewards.free_policies,
                great_people: c.pending_rewards.free_great_people,
            })
            .unwrap_or(RewardsView {
                technologies: 0,
                policies: 0,
                great_people: 0,
            }),
        winner: w.winner.map(|(c, _)| c.0),
        result: w
            .winner
            .zip(w.victory_turn)
            .map(|((winner, condition), turn)| MatchResultView {
                winner: winner.0,
                condition: condition.index(),
                turn,
            }),
        tiles,
        units,
        cities,
        civs,
    }
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS, schemars::JsonSchema,
)]
pub struct EntityRef {
    pub id: u32,
    pub gen: u32,
}
impl<T> From<Id<T>> for EntityRef {
    fn from(id: Id<T>) -> Self {
        Self {
            id: id.index(),
            gen: id.generation(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS, schemars::JsonSchema)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum PresentationEvent {
    UnitMoved {
        unit: EntityRef,
        from: u16,
        to: u16,
    },
    CityFounded {
        city: EntityRef,
        by: u8,
    },
    CityCaptured {
        city: EntityRef,
        by: u8,
    },
    CityPopulation {
        city: EntityRef,
        population: u32,
    },
    TechLearned {
        tech: u32,
    },
    RuinsExplored {
        tile: u16,
        reward: u32,
    },
    RuinsCleared {
        tile: u16,
    },
    NaturalWonderDiscovered {
        tile: u16,
        wonder: u32,
        first: bool,
    },
    ReligiousUnitActed {
        unit: EntityRef,
        action: ReligiousEventKind,
    },
    ProphetBorn {
        unit: EntityRef,
    },
    FaithPurchased {
        city: EntityRef,
        faith: String,
    },
    Purchased {
        city: EntityRef,
        gold: String,
    },
    CivsMet {
        other: u8,
    },
    WarDeclared {
        aggressor: u8,
        against: u8,
    },
    PeaceMade {
        other: u8,
    },
    Victory {
        civ: u8,
        condition: u32,
    },
    PartAssembled {
        part: u32,
        total: u32,
    },
    VotingOpened {
        turn: u32,
    },
    VoteResolved {
        result: VoteResultView,
    },
    DefensivePactSigned {
        civ: u8,
    },
    FriendshipSigned {
        other: u8,
    },
    ResearchAgreementSigned {
        other: u8,
    },
    ResearchAgreementCompleted {
        other: u8,
    },
    CoupResolved {
        city_state: u8,
        succeeded: bool,
    },
    TechStolen {
        technology: u32,
        other: Option<u8>,
        as_thief: bool,
    },
    SpyCaught {
        city: EntityRef,
    },
    UnitDamaged {
        unit: EntityRef,
        damage: u8,
        died: bool,
    },
    /// A fight this player took part in, as either side.
    Attacked {
        /// This player struck the blow.
        viewer_attacked: bool,
        attacker: EntityRef,
        target: crate::protocol::AttackTarget,
        from: u16,
        at: u16,
        ranged: bool,
        damage_to_attacker: i32,
        damage_to_defender: i32,
        attacker_died: bool,
        defender_died: bool,
    },
    CampCleared {
        tile: u16,
        gold: i32,
        recruited: Option<EntityRef>,
    },
    RebelsSpawned,
    BarbarianCampAppeared {
        tile: u16,
    },
    CityStateGift {
        city_state: u8,
        unit: EntityRef,
    },
    CityStateMarried {
        city_state: u8,
        gold: i32,
    },
    Notice {
        notice: NoticeKind,
    },
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub enum ReligiousEventKind {
    Founded,
    Enhanced,
    Spread,
    HeresyRemoved,
    HolySiteConstructed,
    Attrition,
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub enum NoticeKind {
    ReligionFounded,
    ReligionEnhanced,
    PantheonFounded,
    CivDefeated,
    GoldenAgeStarted,
    GoldenAgeEnded,
}

/// A fight in the player's words: "Your Warrior hit a Brute for 40", "A Brute destroyed your Warrior".
fn battle_text(
    before: &World,
    r: &Ruleset,
    me: CivId,
    b: &civ_core::world::combat::BattleReport,
    attacker_died: bool,
    defender_died: bool,
) -> String {
    let a = r.units[b.attacker_base].name.as_str();
    let defender = match (b.target, b.defender_base) {
        (civ_core::world::air::Target::City(id), _) => before.cities.get(id).map_or("a city".to_string(), |c| c.name.clone()),
        (_, Some(base)) => r.units[base].name.clone(),
        _ => "a unit".to_string(),
    };
    let city = matches!(b.target, civ_core::world::air::Target::City(_));
    let an = |name: &str| if name.starts_with(['A', 'E', 'I', 'O', 'U']) { "an" } else { "a" };
    let d = if city { defender.clone() } else { format!("{} {defender}", an(&defender)) };
    if b.attacker_owner == me {
        if defender_died {
            format!("Your {a} destroyed {d}")
        } else if attacker_died {
            format!("Your {a} fell attacking {d}")
        } else if b.damage_to_defender == 0 {
            format!("Your {a} attacked {d} to no effect")
        } else {
            format!("Your {a} hit {d} for {}", b.damage_to_defender)
        }
    } else {
        let yours = if city { defender.clone() } else { format!("your {defender}") };
        let them = format!("{} {a}", an(a));
        let them = them[..1].to_uppercase() + &them[1..];
        if defender_died {
            format!("{them} destroyed {yours}")
        } else if attacker_died {
            format!("{} destroyed {} {a}", if city { defender.clone() } else { format!("Your {defender}") }, an(a))
        } else if b.damage_to_defender == 0 {
            format!("{them} attacked {yours} to no effect")
        } else {
            format!("{them} attacked {yours} for {}", b.damage_to_defender)
        }
    }
}

fn presentation(kind: &str, text: String, event: PresentationEvent) -> EventView {
    EventView {
        kind: kind.into(),
        text,
        event,
    }
}

/// Project only explicitly supported events. Before/after snapshots establish
/// ownership, not historical visibility: foreign movement is omitted until
/// the action executor supplies event-time observer records.
pub fn report_for(
    before: &World,
    after: &World,
    r: &Ruleset,
    me: CivId,
    events: &[TurnEvent],
) -> TurnReport {
    let own_unit = |id| {
        before.units.get(id).is_some_and(|u| u.owner == me)
            && after.units.get(id).is_some_and(|u| u.owner == me)
    };
    let own_city = |id| {
        before.cities.get(id).is_some_and(|c| c.owner == me)
            && after.cities.get(id).is_some_and(|c| c.owner == me)
    };
    let mut out = Vec::new();
    for event in events {
        let (kind, text, payload) = match event {
            TurnEvent::Battle { attacker_died, defender_died, report: Some(b) }
                if b.attacker_owner == me || b.defender_owner == me =>
            {
                ("Attacked", battle_text(before, r, me, b, *attacker_died, *defender_died), PresentationEvent::Attacked {
                    viewer_attacked: b.attacker_owner == me,
                    attacker: b.attacker.into(),
                    target: match b.target {
                        civ_core::world::air::Target::Unit(id) => crate::protocol::AttackTarget::Unit {
                            unit: crate::protocol::EntityRef { id: id.index(), gen: id.generation() },
                        },
                        civ_core::world::air::Target::City(id) => crate::protocol::AttackTarget::City {
                            city: crate::protocol::EntityRef { id: id.index(), gen: id.generation() },
                        },
                    },
                    from: b.from.0,
                    at: b.at.0,
                    ranged: b.ranged,
                    damage_to_attacker: b.damage_to_attacker,
                    damage_to_defender: b.damage_to_defender,
                    attacker_died: *attacker_died,
                    defender_died: *defender_died,
                })
            }
            TurnEvent::UnitMoved(m) if own_unit(m.unit) => (
                "UnitMoved",
                "Unit moved".into(),
                PresentationEvent::UnitMoved {
                    unit: m.unit.into(),
                    from: m.from.0,
                    to: m.to.0,
                },
            ),
            TurnEvent::CityFounded { city, by } if *by == me => (
                "CityFounded",
                "City founded".into(),
                PresentationEvent::CityFounded {
                    city: (*city).into(),
                    by: by.0,
                },
            ),
            TurnEvent::CityCaptured { city, by }
                if *by == me || before.cities.get(*city).is_some_and(|c| c.owner == me) =>
            {
                (
                    "CityCaptured",
                    "City changed hands".into(),
                    PresentationEvent::CityCaptured {
                        city: (*city).into(),
                        by: by.0,
                    },
                )
            }
            TurnEvent::CityGrew { city, population }
            | TurnEvent::CityStarved { city, population }
                if own_city(*city) =>
            {
                (
                    "CityPopulation",
                    "City population changed".into(),
                    PresentationEvent::CityPopulation {
                        city: (*city).into(),
                        population: *population,
                    },
                )
            }
            TurnEvent::TechLearned { civ, tech } if *civ == me => (
                "TechLearned",
                format!("Learned {}", r.technologies[*tech].name),
                PresentationEvent::TechLearned { tech: tech.index() },
            ),
            TurnEvent::RuinsExplored { civ, tile, reward } if *civ == me => (
                "RuinsExplored",
                "Ancient ruins explored".into(),
                PresentationEvent::RuinsExplored {
                    tile: tile.0,
                    reward: reward.index(),
                },
            ),
            TurnEvent::RuinsCleared { civ, tile } if *civ == me => (
                "RuinsCleared",
                "Ancient ruins cleared".into(),
                PresentationEvent::RuinsCleared { tile: tile.0 },
            ),
            TurnEvent::NaturalWonderDiscovered(d) if d.civ == me => (
                "NaturalWonderDiscovered",
                format!("Discovered {}", r.terrains[d.wonder].name),
                PresentationEvent::NaturalWonderDiscovered {
                    tile: d.tile.0,
                    wonder: d.wonder.index(),
                    first: d.first,
                },
            ),
            TurnEvent::ReligiousUnitActed { civ, event } if *civ == me => {
                use civ_core::world::religious_units::ReligiousEvent as R;
                let (unit, action) = match event {
                    R::Founded { unit, .. } => (*unit, ReligiousEventKind::Founded),
                    R::Enhanced { unit, .. } => (*unit, ReligiousEventKind::Enhanced),
                    R::Spread { unit, .. } => (*unit, ReligiousEventKind::Spread),
                    R::HeresyRemoved { unit, .. } => (*unit, ReligiousEventKind::HeresyRemoved),
                    R::HolySiteConstructed { unit, .. } => {
                        (*unit, ReligiousEventKind::HolySiteConstructed)
                    }
                    R::Attrition { unit, .. } => (*unit, ReligiousEventKind::Attrition),
                };
                (
                    "ReligiousUnitActed",
                    "Religious unit acted".into(),
                    PresentationEvent::ReligiousUnitActed {
                        unit: unit.into(),
                        action,
                    },
                )
            }
            TurnEvent::ProphetBorn { civ, unit } if *civ == me => (
                "ProphetBorn",
                "A Great Prophet was born".into(),
                PresentationEvent::ProphetBorn {
                    unit: (*unit).into(),
                },
            ),
            TurnEvent::FaithPurchased { city, faith, .. } if own_city(*city) => (
                "FaithPurchased",
                "Purchased with faith".into(),
                PresentationEvent::FaithPurchased {
                    city: (*city).into(),
                    faith: faith.to_string(),
                },
            ),
            TurnEvent::Purchased { city, gold, .. } if own_city(*city) => (
                "Purchased",
                "Purchased with gold".into(),
                PresentationEvent::Purchased {
                    city: (*city).into(),
                    gold: gold.to_string(),
                },
            ),
            TurnEvent::CivsMet { a, b } if *a == me || *b == me => (
                "CivsMet",
                "Met a civilization".into(),
                PresentationEvent::CivsMet {
                    other: if *a == me { b.0 } else { a.0 },
                },
            ),
            TurnEvent::WarDeclared { aggressor, against } if *aggressor == me || *against == me => {
                (
                    "WarDeclared",
                    "War declared".into(),
                    PresentationEvent::WarDeclared {
                        aggressor: aggressor.0,
                        against: against.0,
                    },
                )
            }
            TurnEvent::PeaceMade { a, b } if *a == me || *b == me => (
                "PeaceMade",
                "Peace made".into(),
                PresentationEvent::PeaceMade {
                    other: if *a == me { b.0 } else { a.0 },
                },
            ),
            TurnEvent::Endgame(civ_core::world::victory_actions::Event::PartAssembled {
                civ,
                part,
                total,
            }) if *civ == me => (
                "PartAssembled",
                "Spaceship part assembled".into(),
                PresentationEvent::PartAssembled {
                    part: part.index(),
                    total: *total,
                },
            ),
            TurnEvent::Endgame(civ_core::world::victory_actions::Event::VotingOpened { turn }) => (
                "VotingOpened",
                "Diplomatic voting is open".into(),
                PresentationEvent::VotingOpened { turn: *turn },
            ),
            TurnEvent::Endgame(civ_core::world::victory_actions::Event::VoteResolved(result)) => (
                "VoteResolved",
                "Diplomatic voting resolved".into(),
                PresentationEvent::VoteResolved {
                    result: VoteResultView {
                        turn: result.turn,
                        needed: result.needed,
                        winner: result.winner.map(|c| c.0),
                        votes: result
                            .votes
                            .iter()
                            .map(|(candidate, votes)| VoteTotalView {
                                candidate: candidate.0,
                                votes: *votes,
                            })
                            .collect(),
                    },
                },
            ),
            TurnEvent::DefensivePactSigned { a, b } if *a == me || *b == me => (
                "DefensivePactSigned",
                "Defensive pact signed".into(),
                PresentationEvent::DefensivePactSigned {
                    civ: if *a == me { b.0 } else { a.0 },
                },
            ),
            TurnEvent::FriendshipSigned { a, b } if *a == me || *b == me => (
                "FriendshipSigned",
                "Declaration of friendship signed".into(),
                PresentationEvent::FriendshipSigned {
                    other: if *a == me { b.0 } else { a.0 },
                },
            ),
            TurnEvent::ResearchAgreementSigned { a, b } if *a == me || *b == me => (
                "ResearchAgreementSigned",
                "Research agreement signed".into(),
                PresentationEvent::ResearchAgreementSigned {
                    other: if *a == me { b.0 } else { a.0 },
                },
            ),
            TurnEvent::ResearchAgreementCompleted { a, b } if *a == me || *b == me => (
                "ResearchAgreementCompleted",
                "Research agreement completed".into(),
                PresentationEvent::ResearchAgreementCompleted {
                    other: if *a == me { b.0 } else { a.0 },
                },
            ),
            TurnEvent::Coup(civ_core::world::city_states::CoupResult::Succeeded {
                city_state,
                plotter,
                ..
            }) if *plotter == me => (
                "CoupSucceeded",
                "Coup succeeded".into(),
                PresentationEvent::CoupResolved {
                    city_state: city_state.0,
                    succeeded: true,
                },
            ),
            TurnEvent::Coup(civ_core::world::city_states::CoupResult::Failed {
                city_state,
                plotter,
            }) if *plotter == me => (
                "CoupFailed",
                "Coup failed".into(),
                PresentationEvent::CoupResolved {
                    city_state: city_state.0,
                    succeeded: false,
                },
            ),
            TurnEvent::TechStolen {
                thief,
                victim,
                tech,
                identified,
            } if *thief == me || *victim == me => (
                "TechStolen",
                "A technology was stolen".into(),
                PresentationEvent::TechStolen {
                    technology: tech.index(),
                    as_thief: *thief == me,
                    other: if *thief == me {
                        Some(victim.0)
                    } else if *identified {
                        Some(thief.0)
                    } else {
                        None
                    },
                },
            ),
            TurnEvent::SpyCaught { civ, city } if *civ == me => (
                "SpyCaught",
                "Your spy was caught".into(),
                PresentationEvent::SpyCaught {
                    city: (*city).into(),
                },
            ),
            TurnEvent::UnitDamaged {
                civ,
                unit,
                damage,
                died,
            } if *civ == me => (
                "UnitDamaged",
                "A unit suffered damage".into(),
                PresentationEvent::UnitDamaged {
                    unit: (*unit).into(),
                    damage: *damage,
                    died: *died,
                },
            ),
            TurnEvent::BarbarianCampAppeared { civ, tile } if *civ == me => (
                "BarbarianCampAppeared",
                "A barbarian camp has appeared".into(),
                PresentationEvent::BarbarianCampAppeared { tile: tile.0 },
            ),
            TurnEvent::CampCleared {
                civ,
                tile,
                gold,
                recruited,
            } if *civ == me => (
                "CampCleared",
                "Barbarian camp cleared".into(),
                PresentationEvent::CampCleared {
                    tile: tile.0,
                    gold: *gold,
                    recruited: recruited.map(Into::into),
                },
            ),
            TurnEvent::RebelSpawned { civ, .. } if *civ == me => (
                "RebelsSpawned",
                "Unhappiness has caused a rebellion".into(),
                PresentationEvent::RebelsSpawned,
            ),
            TurnEvent::CityStateGift {
                city_state,
                to,
                unit,
            } if *to == me => (
                "CityStateGift",
                "A city-state has gifted a unit".into(),
                PresentationEvent::CityStateGift {
                    city_state: city_state.0,
                    unit: (*unit).into(),
                },
            ),
            TurnEvent::CityStateMarried {
                city_state,
                by,
                gold,
            } if *by == me => (
                "CityStateMarried",
                "A city-state has joined your civilization".into(),
                PresentationEvent::CityStateMarried {
                    city_state: city_state.0,
                    gold: *gold,
                },
            ),
            TurnEvent::Victory { civ, condition } => (
                "Victory",
                "The match has ended".into(),
                PresentationEvent::Victory {
                    civ: civ.0,
                    condition: condition.index(),
                },
            ),
            TurnEvent::ReligionFounded { civ } if *civ == me => (
                "ReligionFounded",
                "Religion founded".into(),
                PresentationEvent::Notice {
                    notice: NoticeKind::ReligionFounded,
                },
            ),
            TurnEvent::ReligionEnhanced { civ } if *civ == me => (
                "ReligionEnhanced",
                "Religion enhanced".into(),
                PresentationEvent::Notice {
                    notice: NoticeKind::ReligionEnhanced,
                },
            ),
            TurnEvent::PantheonFounded { civ, .. } if *civ == me => (
                "PantheonFounded",
                "Pantheon founded".into(),
                PresentationEvent::Notice {
                    notice: NoticeKind::PantheonFounded,
                },
            ),
            TurnEvent::CivDefeated { civ } if *civ == me => (
                "CivDefeated",
                "Your civilization was defeated".into(),
                PresentationEvent::Notice {
                    notice: NoticeKind::CivDefeated,
                },
            ),
            TurnEvent::GoldenAgeStarted { civ, .. } if *civ == me => (
                "GoldenAgeStarted",
                "Golden age started".into(),
                PresentationEvent::Notice {
                    notice: NoticeKind::GoldenAgeStarted,
                },
            ),
            TurnEvent::GoldenAgeEnded { civ } if *civ == me => (
                "GoldenAgeEnded",
                "Golden age ended".into(),
                PresentationEvent::Notice {
                    notice: NoticeKind::GoldenAgeEnded,
                },
            ),
            _ => continue,
        };
        out.push(presentation(kind, text, payload));
    }
    TurnReport {
        turn: after.turn,
        events: out,
    }
}

/// Trusted simulation callers receive the supported reports from every seat.
/// The raw core log remains internal; debug strings are never wire payloads.
pub fn advance(w: &mut World, r: &Ruleset) -> TurnReport {
    capture_observations(w, r);
    let before = w.clone();
    let events = advance_turn(w, r, CityFocus::None, TurnModifiers::default());
    capture_observations(w, r);
    let mut reports = Vec::new();
    for me in w.civ_ids() {
        reports.extend(report_for(&before, w, r, me, &events).events);
    }
    TurnReport {
        turn: w.turn,
        events: reports,
    }
}

pub fn advance_for(w: &mut World, r: &Ruleset, me: CivId) -> TurnReport {
    capture_observations(w, r);
    let before = w.clone();
    let events = advance_turn(w, r, CityFocus::None, TurnModifiers::default());
    capture_observations(w, r);
    report_for(&before, w, r, me, &events)
}

pub fn reachable(w: &World, r: &Ruleset, me: CivId, index: u32, generation: u32) -> Vec<Reach> {
    let Some((id, _)) = w
        .units
        .iter()
        .find(|(id, u)| id.index() == index && id.generation() == generation && u.owner == me)
    else {
        return Vec::new();
    };
    reachable_preview(w, r, me, id)
        .into_iter()
        .map(|p| Reach {
            tile: p.tile.0,
            moves_left: moves(p.moves_left.raw()),
            uncertain: p.uncertain,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn foreign_and_unattributed_events_never_fall_through_as_debug_text() {
        let (before, r, me) = crate::new_game(&crate::Settings::default()).unwrap();
        let mut after = before.clone();
        let (unit, foreign) = before.units.iter().find(|(_, u)| u.owner != me).unwrap();
        let from = foreign.position;
        let to = before.grid.neighbours(from).next().unwrap();
        // End snapshots cannot establish whether the route was visible when
        // movement happened, even if both endpoint tiles are visible now.
        after.visibility.observe(me, from);
        after.visibility.observe(me, to);
        let events = [
            TurnEvent::UnitMoved(civ_core::world::movement::MoveEvent {
                unit,
                from,
                to,
                spent: civ_core::stats::Fixed::ONE,
            }),
            TurnEvent::TechLearned {
                civ: foreign.owner,
                tech: r.technologies.ids().next().unwrap(),
            },
            TurnEvent::Battle {
                attacker_died: true,
                defender_died: false,
                report: None,
            },
            TurnEvent::RuinsExplored {
                civ: foreign.owner,
                tile: to,
                reward: r.ruin_rewards.ids().next().unwrap(),
            },
        ];
        assert!(report_for(&before, &after, &r, me, &events)
            .events
            .is_empty());
    }
}
