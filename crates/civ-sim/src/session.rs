//! Transport-independent authoritative session. Prepare on a clone, persist, then publish the replacement.
use crate::protocol::*;
use crate::view::report_for;
use crate::{content, player_view, reachable, Settings};
use civ_core::arena::{Arena, Id, Registry};
use civ_core::ruleset::Ruleset;
use civ_core::stats::Stat;
use civ_core::world::assignment::CityFocus;
use civ_core::world::commands::{self, Command};
use civ_core::world::construction::Construction;
use civ_core::world::turn::TurnModifiers;
use civ_core::world::{CivId, TileId, World};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

const SAVE_VERSION: u32 = 3;
const MAX_RECORDS: usize = 100_000;
const MAX_SAVE_BYTES: usize = 128 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionError {
    InvalidSettings(String),
    InvalidSeat,
    InvalidSave(String),
    Capacity,
    Finished,
}
impl std::fmt::Display for SessionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidSettings(s) | Self::InvalidSave(s) => f.write_str(s),
            Self::InvalidSeat => f.write_str("seat is not part of this session"),
            Self::Capacity => f.write_str("session history limit reached"),
            Self::Finished => f.write_str("match has finished"),
        }
    }
}
impl std::error::Error for SessionError {}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Seat {
    civ: u8,
    human: bool,
    ready: bool,
    next_human: Option<bool>,
    revision: u64,
    projection: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct RecordedCommand {
    seat: u8,
    request: CommandRequest,
    receipt: CommandReceipt,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
enum Input {
    Command { seat: u8, request: CommandRequest },
    Timeout { turn: u32 },
    Controller { seat: u8, human: bool },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Save {
    version: u32,
    engine: String,
    content: String,
    settings: Settings,
    epoch: String,
    original_seats: Vec<u8>,
    world: String,
    seats: Vec<Seat>,
    records: Vec<RecordedCommand>,
    inputs: Vec<Input>,
    offers: crate::offers::Offers,
}

#[derive(Clone)]
pub struct Session {
    world: World,
    rules: Arc<Ruleset>,
    ruleset_identity: civ_core::save::RulesetIdentity,
    settings: Settings,
    epoch: String,
    original_seats: Vec<u8>,
    seats: Vec<Seat>,
    records: Vec<RecordedCommand>,
    inputs: Vec<Input>,
    offers: crate::offers::Offers,
    /// The last round's AI commands, for the next refresh's replays. Not saved:
    /// a replay is shown once, as the round closes.
    replay: Vec<civ_core::world::ReplayStep>,
    /// Each seat's last command, for `undo`. Not saved: a restored session
    /// has nothing to take back.
    undo: BTreeMap<u8, UndoPoint>,
    /// Counts every change to the world, so an undo can tell that nothing
    /// else happened after the command it takes back.
    version: u64,
    /// Why the command being staged, an undo, was refused; read into its receipt.
    undo_refused: Option<UndoRefusal>,
}

#[derive(Clone)]
struct UndoPoint {
    /// The world before the command, kept only when it may be taken back.
    before: Option<World>,
    /// `Session::version` right after it.
    version: u64,
    /// Why it may not be taken back.
    refusal: Option<UndoRefusal>,
}

/// The old session stays authoritative until its owner has persisted `next` successfully.
pub struct CommandTransition {
    pub next: Session,
    pub receipt: CommandReceipt,
    pub updates: Vec<(u8, GameUpdate)>,
    pub duplicate: bool,
}

pub struct HostTransition {
    pub next: Session,
    pub updates: Vec<(u8, GameUpdate)>,
}

impl Session {
    /// Seat IDs are established by the trusted host. Requests never choose their acting civilization.
    pub fn new(settings: Settings, epoch: String, humans: Vec<u8>) -> Result<Self, SessionError> {
        if !valid_identifier(&epoch) {
            return Err(SessionError::InvalidSettings("invalid match epoch".into()));
        }
        let unique: BTreeSet<_> = humans.iter().copied().collect();
        if unique.is_empty()
            || unique.len() != humans.len()
            || unique.iter().any(|&s| s >= settings.majors)
        {
            return Err(SessionError::InvalidSettings(
                "human seats must be distinct major nations".into(),
            ));
        }
        let (mut world, rules, _) =
            crate::setup::new_game_for(&settings, &humans).map_err(SessionError::InvalidSettings)?;
        let ruleset_identity = civ_core::save::RulesetIdentity::new(&rules);
        let seats: Vec<_> = unique
            .into_iter()
            .map(|civ| {
                world.civs[usize::from(civ)].is_ai = false;
                Seat {
                    civ,
                    human: true,
                    ready: false,
                    next_human: None,
                    revision: 0,
                    projection: String::new(),
                }
            })
            .collect();
        let mut session = Self {
            world,
            rules: Arc::new(rules),
            ruleset_identity,
            settings,
            epoch,
            original_seats: humans,
            seats,
            records: Vec::new(),
            inputs: Vec::new(),
            offers: crate::offers::Offers::default(),
            replay: Vec::new(),
            undo: BTreeMap::new(),
            version: 0,
            undo_refused: None,
        };
        session.refresh(&[], None)?;
        Ok(session)
    }

    /// The content hash of this match's ruleset, as clients compare it.
    pub fn content_hash(&self) -> String {
        content::hash_of(self.settings.ruleset_name()).unwrap_or_else(|_| content::hash())
    }
    pub fn epoch(&self) -> &str {
        &self.epoch
    }
    pub fn turn(&self) -> u32 {
        self.world.turn
    }
    pub fn finished(&self) -> bool {
        self.world.winner.is_some()
    }
    pub fn settings(&self) -> &Settings {
        &self.settings
    }
    pub fn rules(&self) -> &Ruleset {
        &self.rules
    }
    /// Trusted in-process access for hosts and verification. Never serialize it to clients.
    pub fn world(&self) -> &World {
        &self.world
    }
    pub fn seats(&self) -> impl Iterator<Item = u8> + '_ {
        self.seats.iter().map(|s| s.civ)
    }

    fn seat(&self, seat: u8) -> Result<&Seat, SessionError> {
        self.seats
            .iter()
            .find(|s| s.civ == seat)
            .ok_or(SessionError::InvalidSeat)
    }
    fn token(&self, seat: u8) -> String {
        format!("{}/{}/{}", self.epoch, self.world.turn, seat)
    }
    fn cursor(&self, seat: u8) -> Result<ViewCursor, SessionError> {
        Ok(ViewCursor {
            epoch: self.epoch.clone(),
            revision: self.seat(seat)?.revision.to_string(),
        })
    }
    pub fn current_update(&self, seat: u8) -> Result<GameUpdate, SessionError> {
        let s = self.seat(seat)?;
        Ok(GameUpdate {
            cursor: self.cursor(seat)?,
            turn_token: self.token(seat),
            ready: s.ready,
            controlled: s.human,
            pending_control: s.next_human,
            deadline_ms: None,
            view: player_view(&self.world, &self.rules, CivId(seat)),
            offers: self.offers.visible(seat),
            events: Vec::new(),
            replay: None,
        })
    }
    pub fn query(&self, seat: u8, query: Query) -> Result<QueryReply, SessionError> {
        let cursor = self.cursor(seat)?;
        let result = match query {
            Query::UnitOptions { unit } => QueryResult::UnitOptions {
                options: crate::queries::unit_options(
                    &self.world,
                    &self.rules,
                    CivId(seat),
                    unit.id,
                    unit.gen,
                ),
            },
            Query::CityOptions { city } => QueryResult::CityOptions {
                options: crate::queries::city_options(
                    &self.world,
                    &self.rules,
                    CivId(seat),
                    city.id,
                    city.gen,
                ),
            },
            Query::CivOptions => QueryResult::CivOptions {
                options: crate::queries::civ_options(&self.world, &self.rules, CivId(seat)),
            },
            Query::Reachable { unit } => QueryResult::Reachable {
                reach: reachable(&self.world, &self.rules, CivId(seat), unit.id, unit.gen),
            },
            Query::View => QueryResult::View {
                view: Box::new(player_view(&self.world, &self.rules, CivId(seat))),
            },
            Query::CombatPreview { unit, target } => QueryResult::CombatPreview {
                preview: crate::queries::combat_preview(&self.world, &self.rules, CivId(seat), unit, &target),
            },
            Query::Rules => QueryResult::Rules {
                content_hash: self.content_hash(),
                tables: crate::tables(&self.rules),
            },
        };
        Ok(QueryReply { cursor, result })
    }

    pub fn stage_command(
        &self,
        seat: u8,
        request: CommandRequest,
    ) -> Result<CommandTransition, SessionError> {
        let owner = self.seat(seat)?;
        // Recover the original result before evaluating epoch, turn or controller changes.
        if let Some(record) = self.records.iter().find(|r| {
            r.seat == seat
                && r.request.epoch == request.epoch
                && r.request.command_id == request.command_id
        }) {
            let receipt = if record.request == request {
                record.receipt.clone()
            } else {
                self.reject(&request, seat, RejectionCode::CommandIdReused)?
            };
            return Ok(CommandTransition {
                next: self.clone(),
                receipt,
                updates: Vec::new(),
                duplicate: true,
            });
        }
        if self.records.len() >= MAX_RECORDS || self.inputs.len() >= MAX_RECORDS {
            return Err(SessionError::Capacity);
        }
        let failure = if !valid_identifier(&request.command_id)
            || !valid_identifier(&request.epoch)
            || request.turn_token.len() > 160
            || !valid_action_shape(&request.action)
        {
            Some(RejectionCode::InvalidRequest)
        } else if request.epoch != self.epoch {
            Some(RejectionCode::WrongEpoch)
        } else if self.finished() {
            Some(RejectionCode::MatchFinished)
        } else if !owner.human || self.world.civs[usize::from(seat)].defeated {
            Some(RejectionCode::NotControlled)
        } else if request.turn_token != self.token(seat) {
            Some(RejectionCode::StaleTurn)
        } else if owner.ready {
            Some(RejectionCode::TurnFinished)
        } else {
            None
        };
        let mut next = self.clone();
        let mut events = Vec::new();
        let result = if let Some(code) = failure {
            CommandStatus::Rejected { code }
        } else {
            let applied = next
                .apply_action(seat, &request)
                .map(|outcome| events = outcome);
            match applied {
                Ok(()) => {
                    next.version += 1;
                    if request.action != Action::Undo {
                        let refusal = next.undo_refusal(seat, &self.world, &request.action, &events);
                        next.undo.insert(
                            seat,
                            UndoPoint {
                                before: refusal.is_none().then(|| self.world.clone()),
                                version: next.version,
                                refusal,
                            },
                        );
                    }
                    next.inputs.push(Input::Command {
                        seat,
                        request: request.clone(),
                    });
                    if next.all_ready() && !next.finished() {
                        events.extend(next.resolve_round());
                    }
                    next.offers.prune(&next.world);
                    CommandStatus::Applied
                }
                Err(code) => CommandStatus::Rejected { code },
            }
        };
        let updates = if result == CommandStatus::Applied {
            next.refresh(&events, Some(&self.world))?
        } else {
            Vec::new()
        };
        let receipt = CommandReceipt {
            command_id: request.command_id.clone(),
            result,
            cursor: next.cursor(seat)?,
            undo_refusal: next.undo_refused.take(),
        };
        // Malformed requests are never retained: an attacker cannot fill durable history with oversized strings.
        if receipt.result
            != (CommandStatus::Rejected {
                code: RejectionCode::InvalidRequest,
            })
        {
            next.records.push(RecordedCommand {
                seat,
                request,
                receipt: receipt.clone(),
            });
        }
        Ok(CommandTransition {
            next,
            receipt,
            updates,
            duplicate: false,
        })
    }

    fn reject(
        &self,
        request: &CommandRequest,
        seat: u8,
        code: RejectionCode,
    ) -> Result<CommandReceipt, SessionError> {
        Ok(CommandReceipt {
            command_id: request.command_id.clone(),
            result: CommandStatus::Rejected { code },
            cursor: self.cursor(seat)?,
            undo_refusal: None,
        })
    }
    fn apply_action(
        &mut self,
        seat: u8,
        request: &CommandRequest,
    ) -> Result<Vec<civ_core::world::TurnEvent>, RejectionCode> {
        let proposal = match &request.action {
            Action::OfferDefensivePact { target } => Some((*target, AgreementTerms::DefensivePact)),
            Action::OfferFriendship { target } => Some((*target, AgreementTerms::Friendship)),
            Action::OfferResearchAgreement { target } => {
                Some((*target, AgreementTerms::ResearchAgreement))
            }
            Action::OfferPeace { target } => Some((*target, AgreementTerms::Peace)),
            Action::OfferPeaceTreaty { target, give, get } => Some((
                *target,
                AgreementTerms::PeaceTreaty {
                    give: give.clone(),
                    get: get.clone(),
                },
            )),
            Action::OfferTrade { target, give, get } => Some((
                *target,
                AgreementTerms::Trade {
                    give: give.clone(),
                    get: get.clone(),
                },
            )),
            _ => None,
        };
        if let Some((target, terms)) = proposal {
            if self.seats.iter().any(|s| s.civ == target && s.human) {
                // `ai:` ids belong to AI offers (`offers::ai_offer_id`).
                if request.command_id.starts_with("ai:") || request.command_id.contains(":ai:") {
                    return Err(RejectionCode::InvalidRequest);
                }
                self.offers.propose(
                    &self.world,
                    &self.rules,
                    DiplomaticOffer {
                        id: format!("{seat}:{}", request.command_id),
                        from: seat,
                        to: target,
                        turn: self.world.turn,
                        terms,
                    },
                )?;
                return Ok(Vec::new());
            }
        }
        match &request.action {
            Action::FinishTurn => {
                self.seats.iter_mut().find(|s| s.civ == seat).unwrap().ready = true;
                Ok(Vec::new())
            }
            Action::AnswerOffer { offer, accept } => {
                self.offers
                    .answer(&mut self.world, &self.rules, seat, offer, *accept)
            }
            Action::WithdrawOffer { offer } => {
                self.offers.withdraw(seat, offer)?;
                Ok(Vec::new())
            }
            Action::Undo => match self.take_back(seat) {
                Ok(()) => Ok(Vec::new()),
                Err(reason) => {
                    self.undo_refused = Some(reason);
                    Err(RejectionCode::Unavailable)
                }
            },
            action => {
                let command = self.translate(action)?;
                commands::execute(&mut self.world, &self.rules, CivId(seat), command)
                    .map(|outcome| outcome.events)
                    .map_err(|_| RejectionCode::IllegalAction)
            }
        }
    }

    fn all_ready(&self) -> bool {
        self.seats
            .iter()
            .filter(|s| s.human && !self.world.civs[usize::from(s.civ)].defeated)
            .all(|s| s.ready)
    }
    /// Restore the world from before `seat`'s last command.
    fn take_back(&mut self, seat: u8) -> Result<(), UndoRefusal> {
        let point = self.undo.get(&seat).ok_or(UndoRefusal::NothingToUndo)?;
        if let Some(reason) = point.refusal {
            return Err(reason);
        }
        if point.version != self.version {
            return Err(UndoRefusal::OthersActed);
        }
        self.world = point.before.clone().ok_or(UndoRefusal::NothingToUndo)?;
        self.undo.remove(&seat);
        Ok(())
    }
    /// Why `seat`'s just-applied command may not be taken back, if it may not.
    /// It must be a kind that touches only the seat's own affairs (in a room,
    /// a unit's move), and must have shown the seat nothing new, fought
    /// nothing and rolled nothing: taking it back must not be a way to peek.
    fn undo_refusal(
        &self,
        seat: u8,
        before: &World,
        action: &Action,
        events: &[civ_core::world::TurnEvent],
    ) -> Option<UndoRefusal> {
        use civ_core::world::TurnEvent as E;
        let room = self.original_seats.len() > 1;
        let kind_ok = match action {
            Action::MoveUnit { .. } => true,
            Action::Fortify { .. }
            | Action::SkipUnit { .. }
            | Action::SetUp { .. }
            | Action::SetProduction { .. }
            | Action::SetProductionConversion { .. }
            | Action::SetAvoidGrowth { .. }
            | Action::SetResearch { .. }
            | Action::AssignCitizens { .. }
            | Action::SetCityFocus { .. }
            | Action::LockTile { .. } => !room,
            _ => false,
        };
        if events.iter().any(|e| matches!(e, E::Battle { .. } | E::UnitDamaged { .. } | E::CityCaptured { .. })) {
            return Some(UndoRefusal::Combat);
        }
        if events.iter().any(|e| matches!(e, E::RuinsExplored { .. } | E::RuinsCleared { .. })) {
            return Some(UndoRefusal::Chance);
        }
        let me = CivId(seat);
        let (was, now) = (&before.visibility, &self.world.visibility);
        let tiles = self.world.grid.ids().any(|t| {
            (now.can_see(me, t) && !was.can_see(me, t)) || (now.has_explored(me, t) && !was.has_explored(me, t))
        });
        let seen = |w: &World| -> BTreeSet<_> {
            w.units
                .iter()
                .filter(|(id, u)| u.owner != me && civ_core::world::fog::can_see_unit(w, &self.rules, me, *id))
                .map(|(id, _)| id)
                .collect()
        };
        let units = !seen(&self.world).is_subset(&seen(before));
        let cities = now.known_cities(me).count() != was.known_cities(me).count();
        let met = events.iter().any(|e| matches!(e, E::CivsMet { .. } | E::NaturalWonderDiscovered(_)));
        if tiles || units || cities || met {
            return Some(UndoRefusal::Revealed);
        }
        (!kind_ok).then_some(UndoRefusal::NotAllowed)
    }
    fn resolve_round(&mut self) -> Vec<civ_core::world::TurnEvent> {
        // A turn end is final for every command before it.
        self.version += 1;
        for point in self.undo.values_mut() {
            point.before = None;
            point.refusal = Some(UndoRefusal::TurnEnded);
        }
        self.replay.clear();
        let mods = TurnModifiers::for_world(&self.world, &self.rules);
        let events = civ_core::world::finish_round_recorded(
            &mut self.world,
            &self.rules,
            CityFocus::default(),
            mods,
            &mut self.replay,
        );
        for s in &mut self.seats {
            s.ready = false;
            if let Some(human) = s.next_human.take() {
                s.human = human;
            }
            self.world.civs[usize::from(s.civ)].is_ai = !s.human;
        }
        self.offers.expire(&mut self.world, &self.rules);
        let humans: Vec<u8> = self
            .seats
            .iter()
            .filter(|s| s.human && !self.world.civs[usize::from(s.civ)].defeated)
            .map(|s| s.civ)
            .collect();
        self.offers.propose_from_ais(&self.world, &self.rules, &humans);
        events
    }
    /// The executor compares the recorded deadline's turn before issuing this host decision.
    pub fn stage_timeout(
        &self,
        expected_turn: u32,
    ) -> Result<Option<HostTransition>, SessionError> {
        if self.finished() || self.turn() != expected_turn {
            return Ok(None);
        }
        if self.inputs.len() >= MAX_RECORDS {
            return Err(SessionError::Capacity);
        }
        let mut next = self.clone();
        next.inputs.push(Input::Timeout {
            turn: expected_turn,
        });
        let events = next.resolve_round();
        let updates = next.refresh(&events, Some(&self.world))?;
        Ok(Some(HostTransition { next, updates }))
    }
    /// Request a controller change for the next boundary; it cannot spend actions in this turn.
    pub fn stage_controller(&self, seat: u8, human: bool) -> Result<HostTransition, SessionError> {
        self.seat(seat)?;
        if self.inputs.len() >= MAX_RECORDS {
            return Err(SessionError::Capacity);
        }
        let mut next = self.clone();
        next.seats
            .iter_mut()
            .find(|s| s.civ == seat)
            .unwrap()
            .next_human = Some(human);
        next.inputs.push(Input::Controller { seat, human });
        let updates = next.refresh(&[], None)?;
        Ok(HostTransition { next, updates })
    }

    fn refresh(
        &mut self,
        events: &[civ_core::world::TurnEvent],
        before: Option<&World>,
    ) -> Result<Vec<(u8, GameUpdate)>, SessionError> {
        let mut updates = Vec::new();
        for index in 0..self.seats.len() {
            let seat = self.seats[index].civ;
            let view = player_view(&self.world, &self.rules, CivId(seat));
            let offers = self.offers.visible(seat);
            let projection = serde_json::to_string(&(
                &view,
                self.seats[index].ready,
                self.seats[index].human,
                self.seats[index].next_human,
                &offers,
            ))
            .expect("finite player view");
            let visible_events = before
                .map(|w| report_for(w, &self.world, &self.rules, CivId(seat), events).events)
                .unwrap_or_default();
            let replay = before
                .map(|w| crate::view::replay_for(w, &self.world, &self.rules, CivId(seat), &self.replay))
                .unwrap_or_default();
            if self.seats[index].projection == projection && visible_events.is_empty() && replay.is_empty() {
                continue;
            }
            let s = &mut self.seats[index];
            s.revision = s.revision.checked_add(1).ok_or(SessionError::Capacity)?;
            s.projection = projection;
            let revision = s.revision;
            let events = visible_events
                .into_iter()
                .enumerate()
                .map(|(i, event)| PresentedEvent {
                    id: format!("{revision}.{i}"),
                    event,
                })
                .collect();
            updates.push((
                seat,
                GameUpdate {
                    cursor: self.cursor(seat)?,
                    turn_token: self.token(seat),
                    ready: self.seats[index].ready,
                    controlled: self.seats[index].human,
                    pending_control: self.seats[index].next_human,
                    deadline_ms: None,
                    view,
                    offers,
                    events,
                    replay: (!replay.is_empty()).then_some(replay),
                },
            ));
        }
        self.replay.clear();
        Ok(updates)
    }

    pub fn save(&self) -> Result<String, SessionError> {
        let world =
            civ_core::save::encode_with_identity(&self.world, &self.rules, &self.ruleset_identity)
                .map_err(|e| SessionError::InvalidSave(e.to_string()))?;
        let encoded = serde_json::to_string(&Save {
            version: SAVE_VERSION,
            engine: content::ENGINE_VERSION.into(),
            content: self.content_hash(),
            settings: self.settings.clone(),
            epoch: self.epoch.clone(),
            original_seats: self.original_seats.clone(),
            world,
            seats: self.seats.clone(),
            records: self.records.clone(),
            inputs: self.inputs.clone(),
            offers: self.offers.clone(),
        })
        .map_err(|e| SessionError::InvalidSave(e.to_string()))?;
        if encoded.len() > MAX_SAVE_BYTES {
            return Err(SessionError::Capacity);
        }
        Ok(encoded)
    }
    pub fn restore(data: &str) -> Result<Self, SessionError> {
        let invalid = |s: &str| SessionError::InvalidSave(s.into());
        if data.len() > MAX_SAVE_BYTES {
            return Err(invalid("save exceeds size limit"));
        }
        let save: Save =
            serde_json::from_str(data).map_err(|_| invalid("malformed session save"))?;
        if save.version != SAVE_VERSION
            || save.engine != content::ENGINE_VERSION
            || content::hash_of(save.settings.ruleset_name()).ok().as_ref() != Some(&save.content)
        {
            return Err(invalid("incompatible session version or content"));
        }
        crate::setup::validate_settings(&save.settings).map_err(SessionError::InvalidSave)?;
        if !valid_identifier(&save.epoch)
            || save.records.len() > MAX_RECORDS
            || save.inputs.len() > MAX_RECORDS
        {
            return Err(invalid("invalid session history"));
        }
        let rules = Arc::new(
            content::load_ruleset(save.settings.ruleset_name()).map_err(SessionError::InvalidSave)?,
        );
        let world =
            civ_core::save::decode(&save.world, &rules).map_err(|e| invalid(&e.to_string()))?;
        let ruleset_identity = civ_core::save::RulesetIdentity::new(&rules);
        let expected: BTreeSet<_> = save.original_seats.iter().copied().collect();
        let actual: BTreeSet<_> = save.seats.iter().map(|s| s.civ).collect();
        if expected.is_empty()
            || expected.len() != save.original_seats.len()
            || actual != expected
            || actual.len() != save.seats.len()
            || save.seats.iter().any(|s| {
                usize::from(s.civ) >= world.civs.len()
                    || s.civ >= save.settings.majors
                    || world.civs[usize::from(s.civ)].is_ai == s.human
                    || s.revision == 0
            })
        {
            return Err(invalid("invalid session seats"));
        }
        let mut identities = BTreeSet::new();
        for r in &save.records {
            if !actual.contains(&r.seat)
                || r.receipt.command_id != r.request.command_id
                || r.receipt.cursor.epoch != save.epoch
                || r.receipt.cursor.revision.parse::<u64>().is_err()
                || !identities.insert((r.seat, &r.request.epoch, &r.request.command_id))
            {
                return Err(invalid("invalid command history"));
            }
        }
        if !save.offers.validate_saved(&world, &rules, &actual) {
            return Err(invalid("invalid diplomatic offers"));
        }
        if !save.offers.consent_is_active(
            save.records
                .iter()
                .filter(|record| record.receipt.result == CommandStatus::Applied)
                .map(|record| (record.seat, &record.request)),
        ) {
            return Err(invalid("diplomatic consent was revoked"));
        }
        for seat in &actual {
            for offer in save
                .offers
                .visible(*seat)
                .into_iter()
                .filter(|o| o.from == *seat && !crate::offers::is_ai_offer(&world, o))
            {
                let recorded = save.records.iter().any(|record| {
                    record.seat == offer.from
                        && record.request.epoch == save.epoch
                        && record.receipt.result == CommandStatus::Applied
                        && offer.id == format!("{}:{}", record.seat, record.request.command_id)
                        && record.request.turn_token
                            == format!("{}/{}/{}", save.epoch, offer.turn, offer.from)
                        && match (&record.request.action, &offer.terms) {
                            (
                                Action::OfferDefensivePact { target },
                                AgreementTerms::DefensivePact,
                            )
                            | (Action::OfferFriendship { target }, AgreementTerms::Friendship)
                            | (
                                Action::OfferResearchAgreement { target },
                                AgreementTerms::ResearchAgreement,
                            ) => *target == offer.to,
                            (Action::OfferPeace { target }, AgreementTerms::Peace) => {
                                *target == offer.to
                            }
                            (
                                Action::OfferPeaceTreaty { target, give, get },
                                AgreementTerms::PeaceTreaty {
                                    give: offered,
                                    get: requested,
                                },
                            )
                            | (
                                Action::OfferTrade { target, give, get },
                                AgreementTerms::Trade {
                                    give: offered,
                                    get: requested,
                                },
                            ) => *target == offer.to && give == offered && get == requested,
                            _ => false,
                        }
                });
                if !recorded {
                    return Err(invalid("offer has no matching recorded consent"));
                }
            }
        }
        let session = Self {
            world,
            rules,
            ruleset_identity,
            settings: save.settings,
            epoch: save.epoch,
            original_seats: save.original_seats,
            seats: save.seats,
            records: save.records,
            inputs: save.inputs,
            offers: save.offers,
            replay: Vec::new(),
            undo: BTreeMap::new(),
            version: 0,
            undo_refused: None,
        };
        for s in &session.seats {
            let view = player_view(&session.world, &session.rules, CivId(s.civ));
            if s.projection
                != serde_json::to_string(&(
                    &view,
                    s.ready,
                    s.human,
                    s.next_human,
                    session.offers.visible(s.civ),
                ))
                .map_err(|_| invalid("invalid projection"))?
            {
                return Err(invalid("saved view does not match player knowledge"));
            }
        }
        Ok(session)
    }

    /// Independent reconstruction from the initial seed and ordered commands/host inputs.
    /// Rejected requests have no simulation effect; their receipts remain in the durable snapshot.
    pub fn verify_replay(&self) -> Result<(), SessionError> {
        let mut replay = Self::new(
            self.settings.clone(),
            self.epoch.clone(),
            self.original_seats.clone(),
        )?;
        for input in &self.inputs {
            replay = match input {
                Input::Command { seat, request } => {
                    let transition = replay.stage_command(*seat, request.clone())?;
                    if transition.receipt.result != CommandStatus::Applied {
                        return Err(SessionError::InvalidSave("replay command rejected".into()));
                    }
                    transition.next
                }
                Input::Timeout { turn } => {
                    replay
                        .stage_timeout(*turn)?
                        .ok_or_else(|| SessionError::InvalidSave("replay turn mismatch".into()))?
                        .next
                }
                Input::Controller { seat, human } => replay.stage_controller(*seat, *human)?.next,
            };
        }
        if replay.world != self.world {
            return Err(SessionError::InvalidSave("replay world mismatch".into()));
        }
        if replay.offers != self.offers {
            return Err(SessionError::InvalidSave("replay offers mismatch".into()));
        }
        if serde_json::to_string(&replay.seats).ok() != serde_json::to_string(&self.seats).ok() {
            return Err(SessionError::InvalidSave("replay access mismatch".into()));
        }
        Ok(())
    }

    fn translate(&self, action: &Action) -> Result<Command, RejectionCode> {
        let w = &self.world;
        let r = &self.rules;
        let unit = |reference| entity(&w.units, reference);
        let city = |reference| entity(&w.cities, reference);
        let tile = |index: u16| {
            if usize::from(index) < w.grid.len() {
                Ok(TileId(index))
            } else {
                Err(RejectionCode::IllegalAction)
            }
        };
        let construction = |c: &ConstructionRef| match c {
            ConstructionRef::Building { id } => {
                registry(&r.buildings, *id).map(Construction::Building)
            }
            ConstructionRef::Unit { id } => registry(&r.units, *id).map(Construction::Unit),
        };
        Ok(match action {
            Action::MoveUnit { unit: u, to } => Command::MoveUnit {
                unit: unit(*u)?,
                to: tile(*to)?,
            },
            Action::Attack { unit: u, target } => Command::Attack {
                unit: unit(*u)?,
                target: match target {
                    AttackTarget::Unit { unit: u } => commands::AttackTarget::Unit(unit(*u)?),
                    AttackTarget::City { city: c } => commands::AttackTarget::City(city(*c)?),
                },
            },
            Action::FoundCity { unit: u, name } => Command::FoundCity {
                unit: unit(*u)?,
                name: name.clone(),
            },
            Action::SetProductionConversion { city: c, stat } => Command::SetProductionConversion {
                city: city(*c)?,
                stat: stat.map(|s| s.stat()),
            },
            Action::SetAvoidGrowth { city: c, avoid } => Command::SetAvoidGrowth {
                city: city(*c)?,
                avoid: *avoid,
            },
            Action::SetProduction { city: c, queue } => Command::SetProduction {
                city: city(*c)?,
                queue: queue.iter().map(construction).collect::<Result<_, _>>()?,
            },
            Action::SetResearch { technology } => Command::SetResearch {
                technology: registry(&r.technologies, *technology)?,
            },
            Action::Purchase {
                city: c,
                what,
                currency,
            } => Command::Purchase {
                city: city(*c)?,
                what: construction(what)?,
                currency: match currency {
                    Currency::Gold => Stat::Gold,
                    Currency::Faith => Stat::Faith,
                },
            },
            Action::Improve {
                unit: u,
                improvement,
            } => Command::Improve {
                unit: unit(*u)?,
                improvement: registry(&r.improvements, *improvement)?,
            },
            Action::Fortify { unit: u } => Command::Fortify { unit: unit(*u)? },
            Action::AutomateUnit { unit: u } => Command::Automate {
                unit: unit(*u)?,
                mode: civ_core::world::automation::Automation::Work,
            },
            Action::ExploreUnit { unit: u } => Command::Automate {
                unit: unit(*u)?,
                mode: civ_core::world::automation::Automation::Explore,
            },
            Action::SkipUnit { unit: u } => Command::SkipUnit { unit: unit(*u)? },
            Action::Promote { unit: u, promotion } => Command::Promote {
                unit: unit(*u)?,
                promotion: registry(&r.promotions, *promotion)?,
            },
            Action::Upgrade { unit: u } => Command::Upgrade { unit: unit(*u)? },
            Action::DisbandUnit { unit: u } => Command::DisbandUnit { unit: unit(*u)? },
            Action::SetUp { unit: u } => Command::SetUp { unit: unit(*u)? },
            Action::GiftUnit { unit: u } => Command::GiftUnit { unit: unit(*u)? },
            Action::BuyCityState { target } => Command::BuyCityState {
                target: CivId(*target),
            },
            Action::PledgeProtection { target } => Command::PledgeProtection {
                target: CivId(*target),
            },
            Action::WithdrawProtection { target } => Command::WithdrawProtection {
                target: CivId(*target),
            },
            Action::LiberateCity { city: c } => Command::LiberateCity { city: city(*c)? },
            Action::SellBuilding { city: c, building } => Command::SellBuilding {
                city: city(*c)?,
                building: registry(&r.buildings, *building)?,
            },
            Action::BuyTile { city: c, tile: t } => Command::BuyTile {
                city: city(*c)?,
                tile: tile(*t)?,
            },
            Action::StopRazing { city: c } => Command::StopRazing { city: city(*c)? },
            Action::WithdrawSpy { spy } => Command::WithdrawSpy { spy: *spy },
            Action::AttemptCoup { target } => Command::AttemptCoup {
                target: CivId(*target),
            },
            Action::Pillage { unit: u } => Command::Pillage { unit: unit(*u)? },
            Action::Bombard { city: c, target } => Command::Bombard {
                city: city(*c)?,
                target: unit(*target)?,
            },
            Action::Rebase { unit: u, to } => Command::Rebase {
                unit: unit(*u)?,
                to: tile(*to)?,
            },
            Action::AirSweep { unit: u, target } => Command::AirSweep {
                unit: unit(*u)?,
                target: tile(*target)?,
            },
            Action::Paradrop { unit: u, to } => Command::Paradrop {
                unit: unit(*u)?,
                to: tile(*to)?,
            },
            Action::AssembleSpaceshipPart { unit: u } => {
                Command::AssembleSpaceshipPart { unit: unit(*u)? }
            }
            Action::VoteDiplomatic { candidate } => Command::VoteDiplomatic {
                candidate: candidate.map(CivId),
            },
            Action::ClaimFreeTechnology { technology } => Command::ClaimFreeTechnology {
                technology: registry(&r.technologies, *technology)?,
            },
            Action::ClaimFreeGreatPerson { unit_type } => Command::ClaimFreeGreatPerson {
                unit: registry(&r.units, *unit_type)?,
            },
            Action::NuclearStrike { unit: u, target } => Command::NuclearStrike {
                unit: unit(*u)?,
                target: tile(*target)?,
            },
            Action::AdoptPolicy { policy } => Command::AdoptPolicy {
                policy: registry(&r.policies, *policy)?,
            },
            Action::AssignCitizens {
                city: c,
                worked,
                specialists,
            } => Command::AssignCitizens {
                city: city(*c)?,
                worked: worked
                    .iter()
                    .map(|&id| tile(id))
                    .collect::<Result<_, _>>()?,
                specialists: specialists
                    .iter()
                    .map(|s| Ok((registry(&r.specialists, s.specialist)?, s.count)))
                    .collect::<Result<_, RejectionCode>>()?,
            },
            Action::SetCityFocus { city: c, focus } => Command::SetCityFocus {
                city: city(*c)?,
                focus: focus.engine(),
            },
            Action::LockTile { city: c, tile: t, locked } => Command::LockTile {
                city: city(*c)?,
                tile: tile(*t)?,
                locked: *locked,
            },
            Action::KeepPuppet { city: c } => Command::KeepPuppet { city: city(*c)? },
            Action::GreatPerson { unit: u, action } => Command::GreatPerson {
                unit: unit(*u)?,
                action: match action {
                    GreatPersonAction::HurryResearch => {
                        civ_core::world::great_person_actions::Action::HurryResearch
                    }
                    GreatPersonAction::HurryProduction => {
                        civ_core::world::great_person_actions::Action::HurryProduction
                    }
                    GreatPersonAction::TradeMission => {
                        civ_core::world::great_person_actions::Action::TradeMission
                    }
                    GreatPersonAction::GoldenAge => {
                        civ_core::world::great_person_actions::Action::GoldenAge
                    }
                    GreatPersonAction::Construct { improvement } => {
                        civ_core::world::great_person_actions::Action::Construct(registry(
                            &r.improvements,
                            *improvement,
                        )?)
                    }
                },
            },
            Action::Religious { unit: u, action } => Command::Religious {
                unit: unit(*u)?,
                action: match action {
                    ReligiousAction::FoundReligion => {
                        civ_core::world::religious_units::ReligiousAction::FoundReligion
                    }
                    ReligiousAction::EnhanceReligion => {
                        civ_core::world::religious_units::ReligiousAction::EnhanceReligion
                    }
                    ReligiousAction::SpreadReligion { city: c } => {
                        civ_core::world::religious_units::ReligiousAction::SpreadReligion(city(*c)?)
                    }
                    ReligiousAction::RemoveHeresy { city: c } => {
                        civ_core::world::religious_units::ReligiousAction::RemoveHeresy(city(*c)?)
                    }
                    ReligiousAction::ConstructHolySite => {
                        civ_core::world::religious_units::ReligiousAction::ConstructHolySite
                    }
                },
            },
            Action::ChooseReligionBeliefs {
                unit: u,
                beliefs,
                enhancing,
            } => Command::ChooseReligionBeliefs {
                unit: unit(*u)?,
                beliefs: beliefs
                    .iter()
                    .map(|id| registry(&r.beliefs, *id))
                    .collect::<Result<_, _>>()?,
                enhancing: *enhancing,
            },
            Action::ChoosePantheon { belief } => Command::ChoosePantheon {
                belief: registry(&r.beliefs, *belief)?,
            },
            Action::AdoptStateFaith { religion } => Command::AdoptStateFaith {
                religion: CivId(*religion),
            },
            Action::ChooseSite {
                city: c,
                building,
                tile: t,
            } => Command::ChooseSite {
                city: city(*c)?,
                building: registry(&r.buildings, *building)?,
                tile: tile(*t)?,
            },
            Action::Annex { city: c } => Command::Annex { city: city(*c)? },
            Action::Raze { city: c } => Command::Raze { city: city(*c)? },
            Action::AssignSpy { spy, city: c } => Command::AssignSpy {
                spy: usize::from(*spy),
                city: city(*c)?,
            },
            Action::DeclareWar { target } => Command::DeclareWar {
                target: CivId(*target),
            },
            Action::OfferDefensivePact { target } => Command::OfferDefensivePact {
                target: CivId(*target),
            },
            Action::OfferFriendship { target } => Command::OfferFriendship {
                target: CivId(*target),
            },
            Action::OfferResearchAgreement { target } => Command::OfferResearchAgreement {
                target: CivId(*target),
            },
            Action::OfferPeace { target } => Command::OfferPeace {
                target: CivId(*target),
            },
            Action::Denounce { target } => Command::Denounce {
                target: CivId(*target),
            },
            Action::Demand { target, demand: d } => Command::Demand {
                target: CivId(*target),
                demand: demand(*d),
            },
            Action::AnswerDemand {
                by,
                demand: d,
                agree,
            } => Command::AnswerDemand {
                by: CivId(*by),
                demand: demand(*d),
                agree: *agree,
            },
            Action::GiftGold { target, gold } => Command::GiftGold {
                target: CivId(*target),
                gold: *gold,
            },
            Action::OfferPeaceTreaty { target, give, get } => Command::OfferPeaceTreaty {
                target: CivId(*target),
                give: give
                    .iter()
                    .map(|item| trade_item(r, item))
                    .collect::<Result<_, _>>()?,
                get: get
                    .iter()
                    .map(|item| trade_item(r, item))
                    .collect::<Result<_, _>>()?,
            },
            Action::OfferTrade { target, give, get } => Command::OfferTrade {
                target: CivId(*target),
                give: give
                    .iter()
                    .map(|item| trade_item(r, item))
                    .collect::<Result<_, _>>()?,
                get: get
                    .iter()
                    .map(|item| trade_item(r, item))
                    .collect::<Result<_, _>>()?,
            },
            Action::DemandTribute { target, worker } => Command::DemandTribute {
                target: CivId(*target),
                worker: *worker,
            },
            Action::FinishTurn
            | Action::Undo
            | Action::AnswerOffer { .. }
            | Action::WithdrawOffer { .. } => {
                return Err(RejectionCode::InvalidRequest)
            }
        })
    }
}

fn entity<T>(arena: &Arena<T>, reference: EntityRef) -> Result<Id<T>, RejectionCode> {
    arena
        .iter()
        .find(|(id, _)| id.index() == reference.id && id.generation() == reference.gen)
        .map(|(id, _)| id)
        .ok_or(RejectionCode::IllegalAction)
}
fn registry<T>(registry: &Registry<T>, index: u32) -> Result<Id<T>, RejectionCode> {
    registry
        .iter()
        .find(|(id, _)| id.index() == index)
        .map(|(id, _)| id)
        .ok_or(RejectionCode::IllegalAction)
}
pub(crate) fn valid_identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 100
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_:".contains(&b))
}
fn valid_action_shape(action: &Action) -> bool {
    match action {
        Action::AnswerOffer { offer, .. } | Action::WithdrawOffer { offer } => {
            !offer.is_empty()
                && offer.len() <= 104
                && offer
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"-_:".contains(&b))
        }
        Action::FoundCity { name, .. } => {
            !name.trim().is_empty() && name.len() <= 100 && !name.chars().any(char::is_control)
        }
        Action::SetProduction { queue, .. } => queue.len() <= 32,
        Action::ChooseReligionBeliefs { beliefs, .. } => !beliefs.is_empty() && beliefs.len() <= 8,
        Action::AssignCitizens {
            worked,
            specialists,
            ..
        } => {
            worked.len() <= 128
                && specialists.len() <= 64
                && specialists.iter().all(|s| s.count <= 1_000_000)
                && specialists
                    .iter()
                    .map(|s| s.specialist)
                    .collect::<BTreeSet<_>>()
                    .len()
                    == specialists.len()
        }
        Action::OfferPeaceTreaty { give, get, .. } | Action::OfferTrade { give, get, .. } => {
            give.len() <= 32 && get.len() <= 32
        }
        _ => true,
    }
}

#[cfg(test)]
#[path = "session_tests.rs"]
mod tests;

fn demand(value: Demand) -> civ_core::world::promises::Demand {
    use civ_core::world::promises::Demand as Core;
    match value {
        Demand::DontSpyOnUs => Core::DontSpyOnUs,
        Demand::DoNotSpreadReligion => Core::DoNotSpreadReligion,
        Demand::DoNotSettleNearUs => Core::DoNotSettleNearUs,
        Demand::DoNotAttackUs => Core::DoNotAttackUs,
    }
}
pub(crate) fn trade_item(
    r: &Ruleset,
    item: &TradeItem,
) -> Result<civ_core::world::trade::Item, RejectionCode> {
    use civ_core::world::trade::Item;
    Ok(match item {
        TradeItem::City { city } => {
            Item::City(Id::from_parts(city.id, city.gen).ok_or(RejectionCode::InvalidRequest)?)
        }
        TradeItem::Gold { amount } => Item::Gold(*amount),
        TradeItem::GoldPerTurn { amount } => Item::GoldPerTurn(*amount),
        TradeItem::Luxury { resource } => Item::Luxury(registry(&r.resources, *resource)?),
        TradeItem::Strategic { resource, amount } => {
            Item::Strategic(registry(&r.resources, *resource)?, *amount)
        }
        TradeItem::OpenBorders => Item::OpenBorders,
        TradeItem::Embassy => Item::Embassy,
    })
}
