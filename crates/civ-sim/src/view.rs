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
    #[ts(optional)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trade: Option<TradeView>,
    /// The Blitz slice this game plays, if any.
    #[ts(optional)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub slice: Option<SliceView>,
    /// Wall wonders' runs along their borders, where this player knows the tiles.
    #[ts(optional)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub walls: Option<Vec<WallView>>,
    /// Cities this player took this turn and has not yet decided about: a
    /// decision to put to the player. Absent when none waits.
    #[ts(optional)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending_captures: Option<Vec<PendingCaptureView>>,
}

/// A city just taken, waiting for its new owner to say what becomes of it.
/// It is a puppet meanwhile, and stays one if the round closes unanswered.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS, schemars::JsonSchema)]
pub struct PendingCaptureView {
    pub city: crate::protocol::EntityRef,
    /// The answers allowed now, in the order Unciv offers them. Keep and
    /// annex always; liberate only for a city whose founder is someone else
    /// who can take it back; raze unless `CityStateView.razeRefusal` says why.
    pub choices: Vec<CaptureChoice>,
}

/// One answer to a capture, and the action that gives it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum CaptureChoice {
    /// `liberateCity`: return it to its founder.
    Liberate,
    /// `annex`: govern it yourself.
    Annex,
    /// `keepPuppet`: leave it to its governor.
    Keep,
    /// `raze`: burn it, a citizen a turn.
    Raze,
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
    /// How they feel about this player: `unforgivable`, `enemy`, `competitor`, `neutral`, `favorable`,
    /// `friend` or `ally`. Another human always reads `neutral`.
    pub attitude: String,
    /// Their public score, rounded, as a met civilization's score is shown in Civ V.
    pub score: i32,
    /// Cultural influence both ways, when a victory counts it (Lingua Franca).
    #[ts(optional)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub influence: Option<InfluenceView>,
}

/// Influence between this player and one rival, as progress rather than raw
/// culture (a rival's culture is private): tenths of a percent of the way to
/// being influential, which is reached at 1000.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct InfluenceView {
    pub ours: u32,
    pub theirs: u32,
    /// We are influential over them.
    pub influential: bool,
    /// They are influential over us.
    pub influenced: bool,
    /// Percent of our new culture reaching them this turn (0 at war or before meeting).
    pub rate: u32,
}

/// Shares of world trade, when a victory counts them (Reserve Currency).
/// Tenths of a percent; zero while world trade is still too small to count.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TradeView {
    /// Our share, this turn and on each recorded turn before it (oldest first,
    /// at most 20).
    pub share: u16,
    pub history: Vec<u16>,
    /// Our own and every met major's current share.
    pub shares: Vec<TradeShareView>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TradeShareView {
    pub civ: u8,
    pub share: u16,
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
    /// This player's own score, rounded.
    pub score: i32,
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

fn attitude(w: &World, holder: CivId, subject: CivId) -> String {
    use civ_core::world::opinion::{relationship, RelationshipLevel as L};
    match relationship(w, holder, subject) {
        L::Unforgivable => "unforgivable",
        L::Enemy => "enemy",
        L::Competitor => "competitor",
        L::Neutral => "neutral",
        L::Favorable => "favorable",
        L::Friend => "friend",
        L::Ally => "ally",
    }
    .into()
}

fn rounded_score(w: &World, r: &Ruleset, civ: CivId) -> i32 {
    let (num, den) = civ_core::world::victory::score_fraction(w, r, civ);
    ((num + den / 2) / den.max(1)) as i32
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
        score: rounded_score(w, r, me),
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
    /// The wonder or building seen standing on this tile.
    #[ts(optional)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub placed: Option<PlacedView>,
}

/// A wonder or terrain-bound building standing on a tile (the renderer draws
/// it there, rather than guessing a tile).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct PlacedView {
    /// Its id in `RuleTables.buildings`.
    pub building: u32,
    /// A world wonder, which takes the tile: nobody works it.
    pub wonder: bool,
    /// Ships pass through this land tile (Suez, Panama).
    pub canal: bool,
}

/// A Blitz slice: the eras it plays (era numbers) and when it ends
/// (`EndgameView.maxTurns`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SliceView {
    pub name: String,
    pub first_era: i32,
    pub last_era: i32,
}

/// A wall wonder's run along its city's land border (the Great Wall).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct WallView {
    pub owner: u8,
    /// Its id in `RuleTables.buildings`.
    pub building: u32,
    /// Border tiles the wall runs along, those this player knows.
    pub tiles: Vec<u16>,
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
    /// A Luminary's own name.
    #[ts(optional)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
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
    /// Own cities only: the same queue as typed references, front first.
    #[ts(optional)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub queue_refs: Option<Vec<crate::protocol::ConstructionRef>>,
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
    /// It holds its owner's palace.
    #[serde(default)]
    pub capital: bool,
    /// The civ that founded it.
    #[serde(default)]
    pub founder: u8,
    /// Who held it before its owner, if it has changed hands.
    #[serde(default)]
    pub previous_owner: Option<u8>,
    /// The first city its founder built.
    #[serde(default)]
    pub original_capital: bool,
    /// Whether `raze` would be accepted.
    #[serde(default)]
    pub may_raze: bool,
    /// Why `raze` would be refused, when it would.
    #[ts(optional)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raze_refusal: Option<crate::queries::Refusal>,
    /// What its governor works the citizens for (`setCityFocus`).
    #[serde(default)]
    pub focus: crate::protocol::CityFocus,
    /// Tiles pinned with `lockTile`, in the order they were pinned; each is
    /// also in `worked`.
    #[serde(default)]
    pub locked_tiles: Vec<u16>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
/// Religions are named by key: a founder's civ id, or (when faiths arise by
/// era) 128 and up, a faith in `RuleTables.faiths`.
pub struct CityReligionView {
    pub majority: Option<u8>,
    pub majority_pantheon: Option<u8>,
    pub holy_city_of: Option<u8>,
    pub holy_city_blocked: bool,
    pub followers: Vec<CityFollowersView>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS, schemars::JsonSchema)]
pub struct CityFollowersView {
    /// The religion's key (see `CityReligionView`); the wire name predates faiths.
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
    /// Who leads: the nation's leader or its second leader.
    #[ts(optional)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub leader: Option<String>,
    /// The nation's state faith, by its key in `RuleTables.faiths`. Public.
    #[ts(optional)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state_faith: Option<u8>,
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
        civ_core::world::turn::plan_economy(w, r, me, CityFocus::None, TurnModifiers::for_world(w, r));
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
            placed: seen.placed.map(|p| PlacedView {
                building: p.building.index(),
                wonder: p.takes_tile,
                canal: p.canal,
            }),
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
            name: u.name.clone(),
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
            queue_refs: None,
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
            queue_refs: Some(c.queue.iter().map(|q| construction_ref(*q)).collect()),
            own: Some(CityStateView {
                capital: c.uniques.has(civ_core::UniqueType::IndicatesCapital),
                founder: c.founder.0,
                previous_owner: c.previous_owner.map(|p| p.0),
                original_capital: c.original_capital,
                may_raze: civ_core::world::conquest::can_raze(w, id).is_ok(),
                raze_refusal: civ_core::world::conquest::can_raze(w, id).err().and_then(crate::queries::Refusal::raze),
                focus: crate::protocol::CityFocus::of(c.focus),
                locked_tiles: c.locked_tiles.iter().map(|t| t.0).collect(),
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
                food_to_grow: civ_core::world::city::food_to_next_population(c.population, TurnModifiers::for_world(w, r).growth),
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
                        construction: construction_ref(*what),
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
            leader: leader_of(w, r, c.id),
            state_faith: w.civ(c.id).and_then(|c| c.faith.state).map(|k| k.0),
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
            leader: leader_of(w, r, me),
            state_faith: c.faith.state.map(|k| k.0),
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
    let pending_captures: Vec<_> = w
        .cities
        .iter()
        .filter(|(_, c)| c.owner == me && c.capture_pending)
        .map(|(id, _)| PendingCaptureView {
            city: crate::protocol::EntityRef { id: id.index(), gen: id.generation() },
            choices: civ_core::world::conquest::capture_choices(w, id)
                .into_iter()
                .map(|choice| {
                    use civ_core::world::conquest::CaptureChoice as Core;
                    match choice {
                        Core::Liberate => CaptureChoice::Liberate,
                        Core::Annex => CaptureChoice::Annex,
                        Core::Keep => CaptureChoice::Keep,
                        Core::Raze => CaptureChoice::Raze,
                    }
                })
                .collect(),
        })
        .collect();
    PlayerView {
        pending_captures: (!pending_captures.is_empty()).then_some(pending_captures),
        slice: w.slice.as_ref().map(|s| SliceView {
            name: s.name.clone(),
            first_era: s.first_era,
            last_era: s.last_era,
        }),
        walls: wall_views(w, r, me),
        trade: trade_view(w, r, me),
        economy: EconomyView {
            gold: economy.gold,
            science: economy.science,
            culture: economy.culture,
            faith: economy.faith,
            happiness: economy.happiness,
        },
        turn: w.turn,
        // the calendar reads the same speed modifier the turns use
        year: w
            .slice
            .as_ref()
            .and_then(|s| civ_core::world::slices::year(r, s, w.turn))
            .unwrap_or_else(|| {
                civ_core::world::great_people::calendar_year_in(w, r, w.turn)
            }),
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
                    attitude: attitude(w, c.id, me),
                    score: rounded_score(w, r, c.id),
                    influence: (c.is_major() && civ_core::world::influence::tracks_influence(r))
                        .then(|| influence_view(w, r, me, c.id)),
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

pub(crate) fn construction_ref(what: Construction) -> crate::protocol::ConstructionRef {
    match what {
        Construction::Building(id) => crate::protocol::ConstructionRef::Building { id: id.index() },
        Construction::Unit(id) => crate::protocol::ConstructionRef::Unit { id: id.index() },
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
    /// A faith arose in its era. `religion` is its key (the faith's index
    /// in the rules plus 128); `city` is its holy city, when it is yours.
    FaithArose {
        religion: u8,
        name: String,
        city: Option<EntityRef>,
    },
    StateFaithAdopted {
        religion: u8,
        name: String,
    },
    /// The owner entered an era that calls the city by another name.
    CityRenamed {
        city: EntityRef,
        from: String,
        to: String,
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

/// Another civilization's share of a round, as this player saw it happen.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ReplayBatch {
    pub civ: u8,
    /// In the order they happened: `unitMoved` steps and `attacked`,
    /// `cityFounded` and `cityCaptured` events.
    pub events: Vec<EventView>,
    /// Who moves and fights in these events, so the renderer can draw units
    /// that are no longer in the final view.
    pub units: Vec<ReplayUnit>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ReplayUnit {
    pub unit: EntityRef,
    pub owner: u8,
    pub base: u32,
}

/// The other civilizations' turns as `me` saw them happen: an event is kept
/// only if its tiles were in `me`'s sight when it happened (each step's
/// `seen`), never by what is visible once the round is over. Units that hide
/// (`Invisible`, `InvisibleToNonAdjacent`) appear only if `me` still sees them
/// at the end. Battles `me` fought in are always kept; they are also in its
/// ordinary events.
pub fn replay_for(
    before: &World,
    after: &World,
    r: &Ruleset,
    me: CivId,
    steps: &[civ_core::world::ReplayStep],
) -> Vec<ReplayBatch> {
    use civ_core::world::air::Target;
    let unit_of = |id: Id<civ_core::world::Unit>| after.units.get(id).or_else(|| before.units.get(id));
    let hides = |id: Id<civ_core::world::Unit>| {
        unit_of(id).is_some_and(|u| {
            u.all_uniques(r).any(|x| {
                matches!(
                    x.unique_type(),
                    Some(civ_core::UniqueType::Invisible | civ_core::UniqueType::InvisibleToNonAdjacent)
                )
            })
        }) && !civ_core::world::fog::can_see_unit(after, r, me, id)
    };
    let centre = |id| after.cities.get(id).or_else(|| before.cities.get(id)).map(|c| c.centre);
    let nation = |c: CivId| {
        after.civ(c).map_or_else(
            || "Someone".to_string(),
            |c| {
                let n = &r.nations[c.nation];
                n.adjective.first().cloned().unwrap_or_else(|| n.name.clone())
            },
        )
    };
    let mut batches: Vec<ReplayBatch> = Vec::new();
    for step in steps.iter().filter(|s| s.actor != me) {
        let Some((_, seen)) = step.seen.iter().find(|(civ, _)| *civ == me) else { continue };
        let sees = |t: civ_core::world::TileId| seen.get(t.0 as usize).copied().unwrap_or(false);
        let mut events = Vec::new();
        let mut units = Vec::new();
        let note = |units: &mut Vec<ReplayUnit>, id: Id<civ_core::world::Unit>| {
            if let Some(u) = unit_of(id) {
                let entry = ReplayUnit { unit: id.into(), owner: u.owner.0, base: u.base.index() };
                if !units.contains(&entry) {
                    units.push(entry);
                }
            }
        };
        for event in &step.events {
            match event {
                TurnEvent::UnitMoved(m)
                    if sees(m.from) && sees(m.to) && !hides(m.unit) && unit_of(m.unit).is_some_and(|u| u.owner != me) =>
                {
                    note(&mut units, m.unit);
                    events.push(presentation(
                        "UnitMoved",
                        "Unit moved".into(),
                        PresentationEvent::UnitMoved { unit: m.unit.into(), from: m.from.0, to: m.to.0 },
                    ));
                }
                TurnEvent::Battle { attacker_died, defender_died, report: Some(b) } => {
                    let ours = b.attacker_owner == me || b.defender_owner == me;
                    let target_hides = matches!(b.target, Target::Unit(id) if hides(id));
                    if !ours && !(sees(b.from) && sees(b.at) && !hides(b.attacker) && !target_hides) {
                        continue;
                    }
                    note(&mut units, b.attacker);
                    if let Target::Unit(id) = b.target {
                        note(&mut units, id);
                    }
                    let text = if ours {
                        battle_text(before, r, me, b, *attacker_died, *defender_died)
                    } else {
                        format!("{} {} attacked {}", nation(b.attacker_owner), r.units[b.attacker_base].name, nation(b.defender_owner))
                    };
                    events.push(presentation(
                        "Attacked",
                        text,
                        PresentationEvent::Attacked {
                            viewer_attacked: b.attacker_owner == me,
                            attacker: b.attacker.into(),
                            target: match b.target {
                                Target::Unit(id) => crate::protocol::AttackTarget::Unit {
                                    unit: crate::protocol::EntityRef { id: id.index(), gen: id.generation() },
                                },
                                Target::City(id) => crate::protocol::AttackTarget::City {
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
                        },
                    ));
                }
                TurnEvent::CityFounded { city, by } if *by != me && centre(*city).is_some_and(sees) => {
                    events.push(presentation(
                        "CityFounded",
                        "City founded".into(),
                        PresentationEvent::CityFounded { city: (*city).into(), by: by.0 },
                    ));
                }
                TurnEvent::CityCaptured { city, by } if centre(*city).is_some_and(sees) => {
                    events.push(presentation(
                        "CityCaptured",
                        "City changed hands".into(),
                        PresentationEvent::CityCaptured { city: (*city).into(), by: by.0 },
                    ));
                }
                _ => {}
            }
        }
        if events.is_empty() {
            continue;
        }
        match batches.last_mut() {
            Some(last) if last.civ == step.actor.0 => {
                last.events.extend(events);
                for u in units {
                    if !last.units.contains(&u) {
                        last.units.push(u);
                    }
                }
            }
            _ => batches.push(ReplayBatch { civ: step.actor.0, events, units }),
        }
    }
    batches
}

/// Progress toward influence, both ways, in tenths of a percent.
fn influence_view(w: &World, r: &Ruleset, me: CivId, them: CivId) -> InfluenceView {
    use civ_core::world::influence as inf;
    let progress = |from, over| inf::influence_progress(w, r, from, over).clamp(0, i64::from(u32::MAX)) as u32;
    InfluenceView {
        ours: progress(me, them),
        theirs: progress(them, me),
        influential: inf::is_influential(w, r, me, them),
        influenced: inf::is_influential(w, r, them, me),
        rate: inf::influence_rate(w, r, me, them).max(0) as u32,
    }
}

fn wall_views(w: &World, r: &Ruleset, me: CivId) -> Option<Vec<WallView>> {
    use civ_core::world::placement;
    let walls: Vec<WallView> = w
        .cities
        .iter()
        .filter_map(|(id, c)| {
            let tiles: Vec<u16> = placement::wall_segments(w, r, id)
                .into_iter()
                .filter(|t| w.visibility.has_explored(me, *t))
                .map(|t| t.0)
                .collect();
            let building = c.built.iter().copied().find(|b| {
                placement::is_placed(r, *b)
                    && r.buildings[*b]
                        .uniques
                        .iter()
                        .any(|u| u.unique_type() == Some(civ_core::UniqueType::EnemyUnitsSpendExtraMovement))
            })?;
            (!tiles.is_empty()).then(|| WallView { owner: c.owner.0, building: building.index(), tiles })
        })
        .collect();
    (!walls.is_empty()).then_some(walls)
}

fn trade_view(w: &World, r: &Ruleset, me: CivId) -> Option<TradeView> {
    if !civ_core::world::influence::tracks_trade(r) {
        return None;
    }
    let recorded = |c: CivId| w.influence.trade_shares.get(&c).and_then(|h| h.last().copied()).unwrap_or(0);
    let history = w.influence.trade_shares.get(&me).cloned().unwrap_or_default();
    let history = history[history.len().saturating_sub(20)..].to_vec();
    Some(TradeView {
        share: recorded(me),
        history,
        shares: w
            .civs
            .iter()
            .filter(|c| c.is_major() && (c.id == me || w.diplomacy.have_met(me, c.id)))
            .map(|c| TradeShareView { civ: c.id.0, share: recorded(c.id) })
            .collect(),
    })
}

/// A unit's kind as a player reads it, for event text.
fn unit_kind(w: &World, r: &Ruleset, unit: Id<civ_core::world::entities::Unit>, fallback: &str) -> String {
    let name = w.units.get(unit).and_then(|u| r.units.get(u.base)).map_or(fallback, |b| b.name.as_str());
    civ_core::names::player_text(name)
}

/// A leader is public once the civ is known; it never changes after setup.
fn leader_of(w: &World, r: &Ruleset, civ: CivId) -> Option<String> {
    let c = w.civ(civ)?;
    r.nations.get(c.nation)?.leader(c.alt_leader).map(String::from)
}

fn presentation(kind: &str, text: String, event: PresentationEvent) -> EventView {
    EventView {
        kind: kind.into(),
        text,
        event,
    }
}

/// Project only explicitly supported events. Before/after snapshots establish
/// ownership, not historical visibility, so foreign movement is omitted here;
/// [`replay_for`] reports it from event-time sight records.
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
            TurnEvent::FaithArose { religion, city } => {
                let name = civ_core::world::faiths::def(r, *religion).map_or_else(String::new, |f| f.name.clone());
                let mine = after.cities.get(*city).filter(|c| c.owner == me);
                (
                    "FaithArose",
                    match mine {
                        Some(c) => format!("{name} arose in {}", c.name),
                        None => format!("{name} arose"),
                    },
                    PresentationEvent::FaithArose {
                        religion: religion.0,
                        name,
                        city: mine.map(|_| (*city).into()),
                    },
                )
            }
            TurnEvent::StateFaithAdopted { civ, religion } if *civ == me => {
                let name = civ_core::world::faiths::def(r, *religion).map_or_else(String::new, |f| f.name.clone());
                (
                    "StateFaithAdopted",
                    format!("{name} is now the state faith"),
                    PresentationEvent::StateFaithAdopted { religion: religion.0, name },
                )
            }
            TurnEvent::CityRenamed { city, from, to } if own_city(*city) => (
                "CityRenamed",
                format!("{from} is now {to}"),
                PresentationEvent::CityRenamed {
                    city: (*city).into(),
                    from: from.clone(),
                    to: to.clone(),
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
                format!("A {} was born", unit_kind(after, r, *unit, "Great Prophet")),
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
                "Met a nation".into(),
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
                format!("{} assembled", r.units.get(*part).map_or("Project part", |u| u.name.as_str())),
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
                "Treaty of friendship signed".into(),
                PresentationEvent::FriendshipSigned {
                    other: if *a == me { b.0 } else { a.0 },
                },
            ),
            TurnEvent::ResearchAgreementSigned { a, b } if *a == me || *b == me => (
                "ResearchAgreementSigned",
                "Scholarly exchange agreed".into(),
                PresentationEvent::ResearchAgreementSigned {
                    other: if *a == me { b.0 } else { a.0 },
                },
            ),
            TurnEvent::ResearchAgreementCompleted { a, b } if *a == me || *b == me => (
                "ResearchAgreementCompleted",
                "Scholarly exchange completed".into(),
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
                "A raider camp has appeared".into(),
                PresentationEvent::BarbarianCampAppeared { tile: tile.0 },
            ),
            TurnEvent::CampCleared {
                civ,
                tile,
                gold,
                recruited,
            } if *civ == me => (
                "CampCleared",
                "Raider camp cleared".into(),
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
                "A city-state has joined your nation".into(),
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
                "Your nation was defeated".into(),
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
    let mods = TurnModifiers::for_world(w, r);
    let events = advance_turn(w, r, CityFocus::None, mods);
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
    let mods = TurnModifiers::for_world(w, r);
    let events = advance_turn(w, r, CityFocus::None, mods);
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

    /// A replay keeps what the viewer saw when it happened: not what is
    /// visible when the round is over, and not what was hidden at the time.
    #[test]
    fn replays_follow_sight_at_event_time_not_at_the_end() {
        use civ_core::world::combat::BattleReport;
        use civ_core::world::ReplayStep;
        let (before, r, me) = crate::new_game(&crate::Settings::default()).unwrap();
        let (unit, foreign) = before.units.iter().find(|(_, u)| u.owner != me && u.is_military(&r)).unwrap();
        let actor = foreign.owner;
        let (a, b) = (foreign.position, before.grid.neighbours(foreign.position).next().unwrap());
        let (c, d) = (
            before.grid.ids().find(|t| before.grid.distance(*t, a) > 4).unwrap(),
            before.grid.ids().find(|t| before.grid.distance(*t, a) > 6).unwrap(),
        );
        let moved = |from, to| {
            TurnEvent::UnitMoved(civ_core::world::movement::MoveEvent { unit, from, to, spent: civ_core::stats::Fixed::ONE })
        };
        let battle = |from, at| TurnEvent::Battle {
            attacker_died: false,
            defender_died: true,
            report: Some(BattleReport {
                attacker: unit,
                attacker_base: foreign.base,
                attacker_owner: actor,
                target: civ_core::world::air::Target::Unit(unit),
                defender_base: Some(foreign.base),
                defender_owner: CivId(99),
                from,
                at,
                ranged: false,
                damage_to_attacker: 3,
                damage_to_defender: 100,
            }),
        };
        let sight = |tiles: &[civ_core::world::TileId]| {
            let mut v = vec![false; before.grid.len()];
            for t in tiles {
                v[t.0 as usize] = true;
            }
            vec![(me, v)]
        };
        // Seen then: a and b. By the end, only c and d are visible.
        let mut after = before.clone();
        after.visibility.clear_visible(me);
        after.visibility.observe(me, c);
        after.visibility.observe(me, d);
        let steps = vec![
            ReplayStep { actor, events: vec![moved(a, b), battle(a, b)], seen: sight(&[a, b]) },
            ReplayStep { actor, events: vec![moved(c, d), battle(c, d)], seen: sight(&[a, b]) },
            // Half in view is not in view.
            ReplayStep { actor, events: vec![moved(b, c)], seen: sight(&[b]) },
        ];
        let replay = replay_for(&before, &after, &r, me, &steps);
        assert_eq!(replay.len(), 1, "one batch for one acting civ");
        let kinds: Vec<_> = replay[0].events.iter().map(|e| (e.kind.as_str(), &e.event)).collect();
        assert_eq!(kinds.len(), 2, "{kinds:?}");
        assert!(matches!(kinds[0].1, PresentationEvent::UnitMoved { from, to, .. } if *from == a.0 && *to == b.0));
        assert!(matches!(kinds[1].1, PresentationEvent::Attacked { from, at, viewer_attacked: false, .. } if *from == a.0 && *at == b.0));
        assert_eq!(replay[0].units, vec![ReplayUnit { unit: unit.into(), owner: actor.0, base: foreign.base.index() }]);
        // Our own share of the round is never replayed back to us.
        let own = vec![ReplayStep { actor: me, events: vec![moved(a, b)], seen: sight(&[a, b]) }];
        assert!(replay_for(&before, &after, &r, me, &own).is_empty());
    }

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
