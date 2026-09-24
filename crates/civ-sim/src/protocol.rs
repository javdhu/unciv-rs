//! Versioned wire values. Engine structs, world snapshots and actor authority never cross this boundary.
use crate::view::{EventView, PlayerView, Reach};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

pub const PROTOCOL_VERSION: u32 = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EntityRef {
    pub id: u32,
    pub gen: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, schemars::JsonSchema)]
#[serde(tag = "type", rename_all = "camelCase", deny_unknown_fields)]
pub enum ConstructionRef {
    Building { id: u32 },
    Unit { id: u32 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum ProductionConversion {
    Gold,
    Science,
}

impl ProductionConversion {
    pub fn stat(self) -> civ_core::stats::Stat {
        match self {
            Self::Gold => civ_core::stats::Stat::Gold,
            Self::Science => civ_core::stats::Stat::Science,
        }
    }
}

/// What a city's governor works its citizens for (Unciv's `CityFocus`). A
/// single yield counts three times; the two growth foci count their yield
/// twice and food one and a half times. `balanced` is Unciv's default.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, TS, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum CityFocus {
    #[default]
    Balanced,
    Food,
    Production,
    Gold,
    Science,
    Culture,
    Faith,
    GoldGrowth,
    ProductionGrowth,
}

impl CityFocus {
    pub fn engine(self) -> civ_core::world::assignment::CityFocus {
        use civ_core::stats::Stat;
        use civ_core::world::assignment::CityFocus as Core;
        match self {
            Self::Balanced => Core::None,
            Self::Food => Core::Single(Stat::Food),
            Self::Production => Core::Single(Stat::Production),
            Self::Gold => Core::Single(Stat::Gold),
            Self::Science => Core::Single(Stat::Science),
            Self::Culture => Core::Single(Stat::Culture),
            Self::Faith => Core::Single(Stat::Faith),
            Self::GoldGrowth => Core::GoldGrowth,
            Self::ProductionGrowth => Core::ProductionGrowth,
        }
    }

    /// The wire name of an engine focus; one no player can choose reads as balanced.
    pub fn of(focus: civ_core::world::assignment::CityFocus) -> Self {
        use civ_core::stats::Stat;
        use civ_core::world::assignment::CityFocus as Core;
        match focus {
            Core::Single(Stat::Food) => Self::Food,
            Core::Single(Stat::Production) => Self::Production,
            Core::Single(Stat::Gold) => Self::Gold,
            Core::Single(Stat::Science) => Self::Science,
            Core::Single(Stat::Culture) => Self::Culture,
            Core::Single(Stat::Faith) => Self::Faith,
            Core::GoldGrowth => Self::GoldGrowth,
            Core::ProductionGrowth => Self::ProductionGrowth,
            Core::None | Core::Single(_) => Self::Balanced,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum Currency {
    Gold,
    Faith,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, schemars::JsonSchema)]
#[serde(tag = "type", rename_all = "camelCase", deny_unknown_fields)]
pub enum AttackTarget {
    Unit { unit: EntityRef },
    City { city: EntityRef },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, schemars::JsonSchema)]
#[serde(tag = "type", rename_all = "camelCase", deny_unknown_fields)]
pub enum GreatPersonAction {
    HurryResearch,
    HurryProduction,
    TradeMission,
    GoldenAge,
    Construct { improvement: u32 },
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, schemars::JsonSchema)]
#[serde(tag = "type", rename_all = "camelCase", deny_unknown_fields)]
pub enum ReligiousAction {
    FoundReligion,
    EnhanceReligion,
    SpreadReligion { city: EntityRef },
    RemoveHeresy { city: EntityRef },
    ConstructHolySite,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum Demand {
    DontSpyOnUs,
    DoNotSpreadReligion,
    DoNotSettleNearUs,
    DoNotAttackUs,
}

impl Demand {
    pub fn of(demand: civ_core::world::diplomacy::Demand) -> Self {
        use civ_core::world::diplomacy::Demand as Core;
        match demand {
            Core::DontSpyOnUs => Self::DontSpyOnUs,
            Core::DoNotSpreadReligion => Self::DoNotSpreadReligion,
            Core::DoNotSettleNearUs => Self::DoNotSettleNearUs,
            Core::DoNotAttackUs => Self::DoNotAttackUs,
        }
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, schemars::JsonSchema)]
#[serde(tag = "type", rename_all = "camelCase", deny_unknown_fields)]
pub enum TradeItem {
    City { city: EntityRef },
    Gold { amount: i32 },
    GoldPerTurn { amount: i32 },
    Luxury { resource: u32 },
    Strategic { resource: u32, amount: i32 },
    OpenBorders,
    Embassy,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, schemars::JsonSchema)]
#[serde(tag = "type", rename_all = "camelCase", deny_unknown_fields)]
pub enum AgreementTerms {
    Peace,
    PeaceTreaty {
        give: Vec<TradeItem>,
        get: Vec<TradeItem>,
    },
    Friendship,
    ResearchAgreement,
    DefensivePact,
    Trade {
        give: Vec<TradeItem>,
        get: Vec<TradeItem>,
    },
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DiplomaticOffer {
    pub id: String,
    pub from: u8,
    pub to: u8,
    /// The offer expires when this shared turn closes.
    pub turn: u32,
    pub terms: AgreementTerms,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SpecialistAssignment {
    pub specialist: u32,
    pub count: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, schemars::JsonSchema)]
#[serde(tag = "type", rename_all = "camelCase", deny_unknown_fields)]
pub enum Action {
    AnswerOffer {
        offer: String,
        accept: bool,
    },
    WithdrawOffer {
        offer: String,
    },
    MoveUnit {
        unit: EntityRef,
        to: u16,
    },
    Attack {
        unit: EntityRef,
        target: AttackTarget,
    },
    FoundCity {
        unit: EntityRef,
        name: String,
    },
    SetProductionConversion {
        city: EntityRef,
        stat: Option<ProductionConversion>,
    },
    SetAvoidGrowth {
        city: EntityRef,
        avoid: bool,
    },
    SetProduction {
        city: EntityRef,
        queue: Vec<ConstructionRef>,
    },
    SetResearch {
        technology: u32,
    },
    Purchase {
        city: EntityRef,
        what: ConstructionRef,
        currency: Currency,
    },
    Improve {
        unit: EntityRef,
        improvement: u32,
    },
    Fortify {
        unit: EntityRef,
    },
    /// Hand a worker to automation: it improves tiles on its own until given
    /// another order or an enemy appears next to it.
    AutomateUnit {
        unit: EntityRef,
    },
    /// Send a unit exploring on its own, with the same stopping rules.
    ExploreUnit {
        unit: EntityRef,
    },
    SkipUnit {
        unit: EntityRef,
    },
    Promote {
        unit: EntityRef,
        promotion: u32,
    },
    Upgrade {
        unit: EntityRef,
    },
    DisbandUnit {
        unit: EntityRef,
    },
    SetUp {
        unit: EntityRef,
    },
    GiftUnit {
        unit: EntityRef,
    },
    BuyCityState {
        target: u8,
    },
    PledgeProtection {
        target: u8,
    },
    WithdrawProtection {
        target: u8,
    },
    LiberateCity {
        city: EntityRef,
    },
    SellBuilding {
        city: EntityRef,
        building: u32,
    },
    BuyTile {
        city: EntityRef,
        tile: u16,
    },
    StopRazing {
        city: EntityRef,
    },
    WithdrawSpy {
        spy: usize,
    },
    AttemptCoup {
        target: u8,
    },
    Pillage {
        unit: EntityRef,
    },
    Bombard {
        city: EntityRef,
        target: EntityRef,
    },
    Rebase {
        unit: EntityRef,
        to: u16,
    },
    AirSweep {
        unit: EntityRef,
        target: u16,
    },
    Paradrop {
        unit: EntityRef,
        to: u16,
    },
    AssembleSpaceshipPart {
        unit: EntityRef,
    },
    VoteDiplomatic {
        candidate: Option<u8>,
    },
    ClaimFreeTechnology {
        technology: u32,
    },
    ClaimFreeGreatPerson {
        unit_type: u32,
    },
    NuclearStrike {
        unit: EntityRef,
        target: u16,
    },
    AdoptPolicy {
        policy: u32,
    },
    AssignCitizens {
        city: EntityRef,
        worked: Vec<u16>,
        specialists: Vec<SpecialistAssignment>,
    },
    /// Choose what the city's governor works its citizens for. They are
    /// placed afresh at once (pinned tiles first, specialists too), and the
    /// unpinned ones again every turn while the focus is not `balanced` or
    /// the city has a pin. Refused for a puppet.
    SetCityFocus {
        city: EntityRef,
        focus: CityFocus,
    },
    /// Pin one of the city's workable tiles so it is always worked
    /// (`locked: true`), or unpin it. Either way the citizens are placed
    /// afresh around the pins at once, and every turn after while the city
    /// has a pin or a focus. At most one pin per citizen; refused for a
    /// puppet, a tile another of your cities works, and unpinning a tile
    /// that is not pinned.
    LockTile {
        city: EntityRef,
        tile: u16,
        locked: bool,
    },
    /// Keep a city just taken as a puppet, answering its capture (one of
    /// `PlayerView.pendingCaptures`). `annex`, `raze` and `liberateCity`
    /// answer it too; an unanswered capture stays a puppet when the round
    /// closes.
    KeepPuppet {
        city: EntityRef,
    },
    GreatPerson {
        unit: EntityRef,
        action: GreatPersonAction,
    },
    Religious {
        unit: EntityRef,
        action: ReligiousAction,
    },
    ChooseReligionBeliefs {
        unit: EntityRef,
        beliefs: Vec<u32>,
        enhancing: bool,
    },
    ChoosePantheon {
        belief: u32,
    },
    /// Take an arisen faith as the nation's state faith; `religion` is its key
    /// from `RuleTables.faiths`.
    AdoptStateFaith {
        religion: u8,
    },
    /// Choose the tile a wonder or terrain-bound building will stand on once
    /// built (one of `ConstructionChoice.sites.tiles`).
    ChooseSite {
        city: EntityRef,
        building: u32,
        tile: u16,
    },
    Annex {
        city: EntityRef,
    },
    Raze {
        city: EntityRef,
    },
    AssignSpy {
        spy: u16,
        city: EntityRef,
    },
    DeclareWar {
        target: u8,
    },
    OfferFriendship {
        target: u8,
    },
    OfferDefensivePact {
        target: u8,
    },
    OfferResearchAgreement {
        target: u8,
    },
    OfferPeace {
        target: u8,
    },
    OfferPeaceTreaty {
        target: u8,
        give: Vec<TradeItem>,
        get: Vec<TradeItem>,
    },
    Denounce {
        target: u8,
    },
    Demand {
        target: u8,
        demand: Demand,
    },
    AnswerDemand {
        by: u8,
        demand: Demand,
        agree: bool,
    },
    GiftGold {
        target: u8,
        gold: i32,
    },
    OfferTrade {
        target: u8,
        give: Vec<TradeItem>,
        get: Vec<TradeItem>,
    },
    DemandTribute {
        target: u8,
        worker: bool,
    },
    FinishTurn,
    /// Take back this seat's last command, if it may be (see the `undo…`
    /// rejection codes). Rooms allow only a unit's move.
    Undo,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CommandRequest {
    #[schemars(length(min = 1, max = 100), regex(pattern = r"^[A-Za-z0-9_:-]+$"))]
    pub command_id: String,
    #[schemars(length(min = 1, max = 100), regex(pattern = r"^[A-Za-z0-9_:-]+$"))]
    pub epoch: String,
    #[schemars(length(max = 160))]
    pub turn_token: String,
    pub action: Action,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum RejectionCode {
    InvalidRequest,
    CommandIdReused,
    WrongEpoch,
    StaleTurn,
    TurnFinished,
    MatchFinished,
    NotControlled,
    Unavailable,
    IllegalAction,
}

/// Why an `undo` was refused. Its receipt's code is `unavailable`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum UndoRefusal {
    /// No command of this seat's to take back.
    NothingToUndo,
    /// That kind of command cannot be taken back (in rooms, only moves can).
    NotAllowed,
    /// It showed tiles, units or cities not in sight before, or met a civ.
    Revealed,
    /// It involved combat.
    Combat,
    /// It involved chance, such as ancient ruins.
    Chance,
    /// The turn has ended since.
    TurnEnded,
    /// Something else has changed the world since.
    OthersActed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, schemars::JsonSchema)]
#[serde(tag = "status", rename_all = "camelCase")]
pub enum CommandStatus {
    Applied,
    Rejected { code: RejectionCode },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ViewCursor {
    pub epoch: String,
    /// Decimal integer string; revisions remain exact beyond JavaScript's integer range.
    pub revision: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct CommandReceipt {
    pub command_id: String,
    pub result: CommandStatus,
    pub cursor: ViewCursor,
    /// Set when an `undo` was refused, saying why.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub undo_refusal: Option<UndoRefusal>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, schemars::JsonSchema)]
pub struct PresentedEvent {
    pub id: String,
    pub event: EventView,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct GameUpdate {
    pub cursor: ViewCursor,
    pub turn_token: String,
    pub ready: bool,
    pub controlled: bool,
    pub pending_control: Option<bool>,
    /// Server epoch milliseconds; absent for untimed solo sessions.
    pub deadline_ms: Option<String>,
    pub view: PlayerView,
    /// Only this seat's incoming and outgoing offers; no global offer counter.
    pub offers: Vec<DiplomaticOffer>,
    pub events: Vec<PresentedEvent>,
    /// The other civilizations' turns in the round that just closed, in the
    /// order they acted, limited to what this seat saw as it happened. For a
    /// fast replay before `view`, which is already the round's end state.
    /// Absent when there is nothing to show.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub replay: Option<Vec<crate::view::ReplayBatch>>,
    /// Whether this seat's last command may be taken back with `undo`, and
    /// why not. In a room another seat may still act in between, and the
    /// undo is then refused `othersActed`; that is never announced ahead.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub undo: Option<UndoState>,
    /// In a room: every human seat's readiness and control, and, from the
    /// host, its connection and display name. Absent in solo play.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub seats: Option<Vec<SeatStatusView>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct UndoState {
    pub available: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub refusal: Option<UndoRefusal>,
}

/// One human seat of a room, as every member sees it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SeatStatusView {
    pub seat: u8,
    /// It has finished this turn.
    pub ready: bool,
    /// A person plays it; false while a bot has taken over.
    pub controlled: bool,
    /// The host holds a live connection for it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub connected: Option<bool>,
    /// The player's display name, from the host.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub name: Option<String>,
    /// The player left the room for good; a bot plays the seat. From the host.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub left: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, schemars::JsonSchema)]
#[serde(tag = "type", rename_all = "camelCase", deny_unknown_fields)]
pub enum Query {
    Reachable { unit: EntityRef },
    UnitOptions {
        unit: EntityRef,
        /// For a nuclear weapon: also preview a strike on this tile
        /// (`UnitOptions.nuclear_preview`).
        #[ts(optional)]
        #[serde(default, rename = "nuclearTarget", skip_serializing_if = "Option::is_none")]
        nuclear_target: Option<u16>,
    },
    CityOptions { city: EntityRef },
    CivOptions,
    View,
    Rules,
    /// What attacking a visible target would do, across the attack roll.
    CombatPreview { unit: EntityRef, target: AttackTarget },
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, schemars::JsonSchema)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum QueryResult {
    UnitOptions {
        options: Option<crate::queries::UnitOptions>,
    },
    CityOptions {
        options: Option<crate::queries::CityOptions>,
    },
    CivOptions {
        options: crate::queries::CivOptions,
    },
    Reachable {
        reach: Vec<Reach>,
    },
    View {
        view: Box<PlayerView>,
    },
    Rules {
        #[serde(rename = "contentHash")]
        content_hash: String,
        tables: crate::RuleTables,
    },
    CombatPreview {
        preview: Option<crate::queries::CombatPreviewView>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, schemars::JsonSchema)]
pub struct QueryReply {
    pub cursor: ViewCursor,
    pub result: QueryResult,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, schemars::JsonSchema)]
#[serde(tag = "type", rename_all = "camelCase", deny_unknown_fields)]
pub enum ClientMessage {
    Hello {
        version: u32,
    },
    Command {
        #[serde(rename = "requestId")]
        request_id: String,
        command: CommandRequest,
    },
    Query {
        #[serde(rename = "requestId")]
        request_id: String,
        query: Query,
    },
    Resync,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, schemars::JsonSchema)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum ServerMessage {
    Welcome {
        version: u32,
        #[serde(rename = "contentHash")]
        content_hash: String,
        update: GameUpdate,
    },
    Receipt {
        #[serde(rename = "requestId")]
        request_id: String,
        receipt: CommandReceipt,
    },
    QueryReply {
        #[serde(rename = "requestId")]
        request_id: String,
        reply: QueryReply,
    },
    Update {
        update: GameUpdate,
    },
    Error {
        code: String,
    },
}
