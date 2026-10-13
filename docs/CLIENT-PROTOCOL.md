# Client communication contract

Status: shared session, save/replay, solo Worker and private-room host implemented locally.
The web client plays solo sessions through the Worker and room matches over the room's WebSocket
([PLAYABLE.md](PLAYABLE.md)). Broader rule parity, complete match playtests, public matchmaking and ranked play
remain unfinished.

Product direction: become the Colonist.io of Civ. The first outcome is a room people can join from a
link, play through direct control of their civilizations, and return to after connection loss.
Competitive formats build on that working match. Match length, scoring and balance remain playtest
decisions; accepting this architecture does not settle them.

This document owns the selected communication design. [ARCHITECTURE.md](ARCHITECTURE.md) describes the
existing application; [PLATFORM.md](PLATFORM.md) contains broader product proposals.

## Selected design

- One authoritative host per match: Rust/Wasm in a Worker for solo; Rust on the server for multiplayer.
- Typed commands, read-only queries, receipts and unsolicited updates through one gameplay interface.
- Full player views plus typed presentation events initially. Views establish state; events animate it.
- Worker objects locally, JSON over WebSocket remotely. HTTP handles authentication and room discovery.
- One executor validates and applies each action together. Multiplayer receipts follow durable commit.
- Authoritative snapshots and an ordered simulation log support recovery. UI events are not save data.
- Generate TypeScript from Rust wire definitions; validate network messages at runtime. Wire types and
  save formats are distinct from internal engine structs.

The working turn model is the platform draft's shared turns with immediate actions. This is a prototype
default; timing advantages and competitive pacing are unresolved gameplay questions.

## Ownership

| Owner | Responsibility |
|---|---|
| `civ-core` | Legal actions, effects, player knowledge, AI decisions, deterministic round processing |
| `civ-sim` | Gameplay session interface, player-view/event projection, queries, wire serialization |
| Solo Worker | Own local session, serialize requests, save/load locally |
| Multiplayer host | Bind identities to seats, serialize all match inputs, enforce clocks, persist outcomes, publish views |
| Browser | Input, pending intentions, latest received view, animation, camera, selection and sound |

Clients send intentions, never calculated damage, yields, discovery rewards or an engine `TurnPlan`.
The latter contains derived outcomes and remains internal. Humans and bots use the same legality;
only the host performs automatic round processing. The session determines the acting civilization.

Keep wire definitions in `civ-sim` initially. Add a protocol crate only when another Rust consumer
needs the schema independently. Preserve the core's dependency boundary.

## Interface

Implemented browser interface in `web/src/engine/session-client.ts`:

```ts
interface GameSession {
  submit(request: CommandRequest): Promise<CommandReceipt>;
  query(request: QueryRequest): Promise<QueryReply>;
  subscribe(onUpdate: (update: GameUpdate) => void): () => void;
  close(): void;
  // Remote sessions: 'reconnecting' once a retry is scheduled, 'live' when ready again; the stop code once.
  onLink?(listener: (link: 'live' | 'reconnecting') => void): () => void;
  onStop?(listener: (code: string) => void): () => void;
}
```

Opening a session performs a version handshake and supplies the initial full view. Subscription must
deliver that view even if it arrived before the listener attached. Creation/joining happens before
this interface and differs between solo and remote hosting.

| Message | Examples | Meaning |
|---|---|---|
| Command | Move, attack, found city, production/research choice, purchase, policy, finish turn | Request an action |
| Query | Inspect unit/city, reachable tiles, combat preview, purchases | Inspect permitted knowledge; reserve nothing |
| Receipt | Applied with outcome; rejected with safe reason | Resolve one command |
| Update | Player view and visible movement/battle/discovery events | Publish current knowledge and presentation |

Updates are the only path that changes client game state. Receipts resolve pending input without
separately patching units or balances. Each receipt identifies its resulting player-view cursor; it
may arrive before or after that update, and a newer received view already satisfies that cursor.
Presentation events have viewer-scoped IDs so repeats cannot play an animation twice.

## Commands and retries

Commands carry a stable `commandId`, match `epoch`, seat `turnToken` and tagged action. The session
already identifies the match and authenticated seat. Preserve entity index plus generation to prevent
an old command from addressing a new unit that reused an arena slot. Transport correlation IDs are
separate from durable command IDs.

```ts
{
  commandId: "client-generated-uuid",
  epoch: "current-match-epoch",
  turnToken: "current-seat-turn-token",
  action: { type: "moveUnit", unit: { id: 42, gen: 3 }, to: 219 }
}
```

One match executor processes each command without another game input interleaving:

1. Authenticate the seat and look up a recorded result for this command ID.
2. Return the original result for an identical retry. Reject ID reuse with different content.
3. Check epoch, turn access, ownership, preconditions and current legality.
4. Execute, capture observations including intermediate movement steps, and derive permitted results
   and resulting player-view cursors.
5. Commit the transition and command result durably together.
6. Publish permitted updates and return the receipt.

Deduplicate before checking current-turn eligibility: a successful old-turn retry must still recover
its result. Scope identity to match, epoch, seat and command ID. Retain results for the supported retry
lifetime; if history is unavailable, require resynchronization rather than executing an uncertain old
command again. Persist pending requests locally before sending when page-reload recovery is promised.

Stage changes or recover before further commands if persistence fails; live state must not outrun its
durable commit. Rejections change no game state. Movement that discovers a blocker and stops partway
is an applied action with a partial outcome. Previews are advisory, then execution validates again.

A disconnect/timeout means the outcome is unknown, not rejected. Retry the same ID and payload.
Sequence dependent intentions: finish-turn must not race a preceding move or purchase. Deduplication
does not repair reordered requests.

`{ type: "undo" }` takes back the seat's last applied command by restoring the world from just before
it. It is itself a recorded command, so replays reproduce it. When refused, the receipt's code is
`unavailable` and its optional `undoRefusal` says why: `nothingToUndo` (also after a restore),
`notAllowed` (solo: a move, fortify, skip, set-up, production, research or citizen change; in rooms,
a unit's move only), `revealed` (it showed tiles, units or cities not in sight before, or met a civ, so
an undo is never a free look), `combat`, `chance` (e.g. ruins), `turnEnded`, or `othersActed` (anything
else has changed the world since).

## Turn access and ordering

| Identifier | Purpose |
|---|---|
| Match epoch | Identify match history; changes on deliberate reset/fork, not ordinary restart |
| Seat turn token | Authorize this seat's current action window |
| Player view revision | Order this viewer's updates and identify the view behind query results |
| Internal simulation sequence | Order durable transitions; never a public counter of hidden activity |

Do not reject all actions whenever a global world revision changes: unrelated opponents act during
shared turns. Validate current legality and relevant action-specific expectations. Discard late query
answers when their view/selection is obsolete.

Lifecycle: `Lobby → TurnOpen → Resolving → TurnOpen → … → Finished`.

- `FinishTurn` locks only the caller's game actions. Initially readiness cannot be undone. Only the
  host transitions to `Resolving` when everyone is ready or the host deadline expires.
- Commands, readiness, clock expiration and control changes share the executor. Close the window,
  process AI and automatic rules exactly once, restore budgets and issue new tokens together.
- Server clocks decide deadlines; client countdowns are advisory. Record clock-triggered decisions.
- A short disconnect gets a visible grace period. Bot takeover and return to human control happen at
  host-controlled turn boundaries, with exactly one controller. A lost socket alone must not let a bot
  immediately spend the human's remaining actions.
- Arrival order is the prototype default. The turn-boundary lock prevents interleaving during
  resolution; it does not prevent a late attack followed by an early attack next round. Latency and
  consecutive-action advantages need an explicit rules decision before ranked play.

Split current `advance_turn` responsibilities into action execution, AI decisions and once-per-round
economy/growth/research/refresh. Never AI-plan a human seat or charge its economy twice.

## Knowledge, previews and events

Project live information only where observed. Elsewhere, show last-known tile/city state, discovery
and resource-revelation information, with observation turns where useful. Keep private economy and
diplomatic knowledge scoped to the player. A full world, hidden generation seed or authoritative replay
log must not reach multiplayer clients. A solo Worker is not a security boundary from its owner.

Previews use permitted knowledge with unknowns represented explicitly. Full-world pathfinding can
reveal hidden terrain and blockers even if returned destinations are filtered. A known-state preview
can predict a path that stops during authoritative execution when something new is discovered. Combat
previews disclose permitted estimates, not hidden units or future random rolls. Errors follow the same
knowledge policy.

Capture visibility while an action occurs. End-of-round filtering misses transient sightings and can
expose unseen routes. Explicit player event types contain visible path segments and permitted fields.
Unhandled internal events stay internal; never export debug strings by default.

The facade now reads persistent tile/city/civilization observations, gates resources by technology,
and previews movement against permitted knowledge. Hidden-state invariance is tested. Presentation
projection uses an explicit whitelist; foreign movement and battles without event-time attribution
are conservatively omitted from `events`.

The update that closes a round carries `replay`: the other civilizations' turns as ordered per-civ
batches (`{ civ, events, units }`), for a fast replay before the new `view`. The engine records, for
each AI command, what every human could see just before it ran; an event is kept only if its tiles
were in the viewer's sight then, never by end-of-round visibility. A move step needs both of its tiles
in sight, a third-party battle both its tiles (it carries the same `attacked` payload), and a city
founded or taken its centre. Units that hide appear only if still seen at the end. Battles the viewer
fought in are always kept and also stay in `events`. `units` names the owner and type of every unit in
the batch, so units that ended out of view can still be drawn. The field is absent when there is
nothing to replay, and older clients may ignore it.

## Reconnect, persistence and updates

An invite discovers a room; a durable, unguessable guest credential owns a seat. Reconnecting verifies
that credential and fences the older connection so it cannot continue issuing fresh commands. Joining
without an account is compatible with secure seat recovery; room codes alone are not credentials.

Reconnect provides a current player snapshot with its view cursor and any subsequent sanitized updates.
Recover pending command outcomes using their original IDs. Do not send the authoritative command log
or require replaying every old animation to restore state.

Persist snapshots and ordered accepted inputs/results, including AI actions and host decisions such
as timeout/control changes. Include engine/ruleset versions, settings and required RNG state or seed
inputs. Pin versions for active matches. Restore the same epoch, turn access and deduplication history
after restart. Presentation events are a separately projected output, not sufficient replay inputs.
Solo uses the same save/action semantics with a different storage implementation.

Welcome and resync send full views; static rules tables travel once with a content hash. Keep camera and selection
local. WebSocket lacks automatic application backpressure: bound queues and replace obsolete pending
snapshots with a current resync snapshot for slow clients. Reset obsolete presentation playback, but
keep command results recoverable. Publish only permitted knowledge changes, not a pulse for every
hidden action.

Protocol 9 sends `delta` messages between snapshots. `base` must exactly match the client cursor;
`update.view.tiles`, `units`, `cities` and `civs` contain upserts, and `removed` lists deleted keys. Unit
and city keys include both ID and generation. All other update fields are absolute. Deltas compare
already-filtered seat views; they never diff the hidden world. A missing base requests full resync;
stale revisions are ignored. Core and session save format 5 are binary (ARCHITECTURE.md, Persistence), reject older saves and omit derived projections. JSON quantities have explicit units;
integers beyond JavaScript's exact range use strings. Protocol tags never depend on engine enum order.

## First playable room

1. Load the complete ruleset, implement player knowledge and atomic actions: move, attack, found city,
   set production, set research and finish turn. Choose a small finishable scenario and fill the rules
   gaps it requires.
2. Connect human controls through the Worker; AI uses the same legal actions.
3. Add snapshot/restore, durable command identity and deterministic replay.
4. Host the same session remotely. Two people join separate guest seats by link and finish a match;
   implement host clocks, reconnect, grace periods and controlled bot takeover.
5. Test phone and desktop play, then use real matches to select pacing and the first public ruleset.

Acceptance: two browsers finish a match; simultaneous spending cannot overspend; a retried purchase
cannot buy twice; a lost receipt is recoverable; restart preserves acknowledged actions. Changes to
information unavailable to a player must not alter their views or previews except through permitted
observations or public consequences. Expanding the command catalogue must preserve these tests.

Public matchmaking, ranked ladders, accounts and monetization follow the reliable playable room.

## Deferred alternatives

| Alternative | Tradeoff |
|---|---|
| Full-world lockstep | Reveals secrets and adds desync recovery; suitable for trusted/offline uses |
| View patches | Less transfer, more ordering and recovery complexity; measure first |
| Event-only replication | Requires complete client reducers and every fog transition to be correct |
| HTTP commands plus SSE | Viable for asynchronous play; splits commands and updates across channels |
| Binary encoding | Less bandwidth, extra tooling and inspection cost |
| Simultaneous order resolution | Changes gameplay; requires conflict rules and received-versus-executed semantics |

References: [WebSocket API](https://developer.mozilla.org/en-US/docs/Web/API/WebSockets_API),
[Worker structured clone](https://developer.mozilla.org/en-US/docs/Web/API/Web_Workers_API/Structured_clone_algorithm),
[Colonist's play and room flow](https://colonist.io/). The product reference does not select our rules,
match length or technical implementation.

## Implementation entry points

- `civ-sim::session::Session`: prepares atomic transitions, orders seat readiness/controller changes,
  retains receipts, encodes authoritative saves and verifies deterministic replay.
- `civ-sim::protocol`: Rust wire types and JSON schemas. `tools/check_protocol.py` verifies generated
  browser definitions; Ajv validates incoming network messages at runtime.
- `HostedGame` and `session-worker.ts`: solo authority, staged writes and IndexedDB commit before
  acknowledgment. `snapshot()`/`pendingSnapshot()` return the save as a `Uint8Array`, which `HostedGame.restore`
  takes back; the worker stores it as is. A string save from before format 5 reads as no save.
  Compare-and-swap prevents two tabs from overwriting the same save.
- `SessionClient`: buffers the first view, orders revisions, serializes commands and writes a durable
  pending-command outbox. Opening the same session recovers old outcomes before accepting new input.
- `civ-server`: private rooms, guest cookies, authenticated WebSocket seats, SQLite transactions,
  reconnect fencing, deadlines and grace-period controller changes. It runs as one process owning
  its database. No deployment or distributed routing is included.

`GameUpdate` includes own readiness, controller/pending controller, private diplomatic offers and an optional server deadline
(epoch milliseconds as a decimal string). Solo deadlines are absent. Static rules are also available
through the `rules` query and the host's `/api/rules` endpoint.

Human recipients receive immutable offers for trade, peace (including compensation), friendship, research agreements and defensive pacts. `answerOffer` requires
the addressed seat; `withdrawOffer` requires the proposer. A new offer to the same recipient replaces
the old ID. Acceptance rechecks current resources and commits both sides atomically. Offers reserve
no money and expire at the shared turn boundary or when the parties' war/peace status invalidates them.
Finishing a turn leaves consent to an existing offer valid until that boundary. Only the two parties
receive offers or view revisions caused by them. Hidden recipient wealth, technology and elimination
do not affect proposal results or expiry. Saves retain offers plus the applied command establishing
their exact terms; replay reconstructs consent from player inputs. AI recipients still use the existing
AI evaluation path. Peace compensation and noncapital city transfers commit atomically after both parties consent. Protocol version 4 and engine session version 4 reject earlier incompatible protocols; session and core snapshots use format version 3.

Owned city views retain `productionStored` as the current item's work and separately expose
`productionProgress` (typed construction IDs and saved work) and `productionOverflow`. These values
are absent for foreign cities, including visible ones. A finished view retains `result` with the
winning civilization, victory-condition ID and original winning turn; reconnect does not depend on
receiving the one-time victory event. Victory names are included in the static rules table.

### City management, captures and refusals

These fields and actions are additive; the protocol version stays 4.

- `CityView.queueRefs?: ConstructionRef[]` is the queue as typed references, front first, beside the older
  `queue` of names (own cities only; absent for foreign ones).
- The own-city part (`CityStateView`) also carries `capital` (it holds the palace), its origin (`founder`,
  `previousOwner: number | null`, `originalCapital`), `mayRaze` with `razeRefusal?: 'originalCapital' | 'holyCity'`,
  and its citizen management: `focus: CityFocus` and `lockedTiles: number[]` (pinned tiles, in pinning order,
  always among `worked`).
- `CityFocus` is `'balanced' | 'food' | 'production' | 'gold' | 'science' | 'culture' | 'faith' | 'goldGrowth' |
  'productionGrowth'` (Unciv's foci: one yield counts three times; a growth focus counts its yield twice and food
  one and a half times). `{ type: "setCityFocus", city, focus }` sets it and places every citizen afresh at once:
  pins first, then tiles and specialists by the focus. `{ type: "lockTile", city, tile, locked }` pins a workable
  tile (at most one pin per citizen) or unpins it, and places the citizens afresh around the pins at once. A city
  with a focus other than `balanced` or with any pin is managed: every turn its unpinned citizens are placed again
  by the focus (specialists already placed stay), as Unciv's `reassignPopulation` does, so a direct
  `assignCitizens` pick there lasts until the turn ends. An `assignCitizens` that leaves a pinned tile out unpins it.
  A balanced city without pins keeps the player's own picks and places only idle citizens, as before. Both actions
  are refused for a puppet, a pin is refused on a tile another of the player's cities works or stands on, and an
  unpin on a tile that is not pinned. A
  pin that can no longer be worked (the tile was lost, or the city shrank below its pins) lapses. A new owner
  starts with neither. In solo play both can be undone like any citizen change.
- `PlayerView.pendingCaptures?: PendingCaptureView[]` lists cities this seat took this turn and has not decided
  about: `{ city: EntityRef, choices: CaptureChoice[] }`, `CaptureChoice = 'liberate' | 'annex' | 'keep' | 'raze'`
  in Unciv's order. The answers are `liberateCity`, `annex`, `{ type: "keepPuppet", city }` and `raze`; any of
  them settles the capture. Unanswered, the city stays the puppet it already is when the round closes. The field
  is absent when nothing waits; only the conqueror's view carries it, and AI conquerors decide at once.
- `SiteChoice.best?: number` is the engine's own pick for a placed building, whatever the player chose.
- `TilePurchaseChoice.affordable: boolean`. `purchasable_tiles` now lists every tile `buyTile` would accept but for
  the gold, priced as `buyTile` charges it (with the game speed's gold modifier).
- `SpecialistSlot.yields` is what one specialist of that kind yields in this city (its own yields plus what the
  empire's rules add to every specialist, before the city's percentage bonuses). `RuleTables.specialists` rows are
  `SpecialistRow { id, name, yields, greatPersonPoints: { unit, points }[] }`, a superset of the old `Named`.
- `ConstructionChoice.refusal?` says why `setProduction` would refuse the item now (`puppet`, `resistance`,
  `purchaseOnly`), and `PriceQuote.refusal?` why `purchase` would (`puppet`, `resistance`, `occupied` when a unit of
  that kind already stands in the city, `cannotAfford`). Both are absent when nothing the player knows stands in
  the way; the action remains the authority, and a price that depends on unseen information carries no reason.
  Unit options still list only legal actions, so they carry no reasons.

### Demands, undo, targets, city-states, spies, great people, eras and rooms

Also additive; the protocol version stays 4. Every new field is optional in TypeScript unless marked otherwise.

- **Demands.** `PlayerView.demands?: DemandView[]` lists what other nations asked of this player and it has not
  answered: `{ by, demand: Demand, promiseUntil, refusalDeclaresWar, mayRefuse }`. `answerDemand { by, demand,
  agree }` answers one. Agreeing makes a promise that runs through turn `promiseUntil`; refusing a
  `doNotAttackUs` declares war on the asker (`refusalDeclaresWar`), which is refused while a treaty forbids war
  (`mayRefuse: false`). A demand waits until answered or its asker is destroyed. `PlayerView.promises?:
  PromiseView[]` lists promises in force either way, `{ by, to, demand, until }` (`until` is the last turn), and
  `PlayerView.spyingForbidden?: number[]` the nations whose cities `assignSpy` refuses while a `dontSpyOnUs`
  promise runs. Events: `demandMade { by, of, demand }`, `demandAnswered { by, to, demand, agreed }`,
  `promiseBroken { by, to, demand }` and `promiseKept { by, to, demand }`, sent to the two nations involved.
- **Undo.** `GameUpdate.undo?: { available: boolean, refusal?: UndoRefusal }` rides on every update (solo and
  rooms): whether `undo` would take back this seat's last command, and the reason when not (`nothingToUndo` when
  there is none). In a room another seat may act in between, and `undo` is then refused `othersActed`; the
  field never announces that ahead, since it would leak others' actions. A change in `undo` alone does not send
  an update; it rides on the next one.
- **Mission targets.** `UnitOptions.targets?: ActionTargets[]`, `{ action: UnitActionKind, tiles: number[] }`,
  lists where each `needsTarget` mission may aim now: `rebase` (own cities, and own units that can
  carry it, in range), `airSweep` (visible tiles in attack range), `paradrop` (tiles within its drop range where
  it may land) and `nuclearStrike`. Nuclear targets use what the player knows: a nation last seen holding a blast
  tile, or whose units it sees there, must have been met; a nation it has not seen there can still make the
  strike refused.
- **Nuclear preview.** The `unitOptions` query takes an optional `nuclearTarget: tile`; for a nuclear weapon the
  reply's `UnitOptions.nuclear_preview?: NuclearPreviewView` then says what a strike there would do: `{
  blastRadius, tiles, victims, interceptChance, legal }`, the blast tiles, the nations the strike declares war on
  as far as the player can see, the percent chance an interceptor the player can see stops it (0 when none is
  seen or the weapon cannot be intercepted), and whether `nuclearStrike` would be accepted. It is absent for
  other units and for a tile that does not exist. (A separate query kind was avoided so that code keyed on every
  `Query['type']` keeps compiling.)
- **Interception.** `CombatPreviewView.interceptChance?` is the percent chance an interceptor the player can see
  stops an air strike first (absent for other attacks). An `airSweep` against a defended tile now produces an
  `attacked` event for the sweep's dogfight, with the interceptor as the defender.
- **City-states.** `CityStateRelationsView` gains `mayPledge`, `mayWithdraw` (the `pledgeProtection` and
  `withdrawProtection` checks), `tributeGold`, `tributeWorker` and `tributeAmount` (what `demandTribute` would get
  now), `mayCoup` with `coupChance?` (percent, not counting an unseen defending spy), and `influencePerTurn` and
  `restingPoint` as decimal strings. These are required fields: the view is built only by the engine.
  `RuleTables.influenceThresholds?: { friend, ally, unforgivable }` is 30, 60 and -30: `Ally` from `ally` (only
  the major furthest ahead), `Friend` from `friend`, `Neutral` from 0, `Enemy` below 0, `Unforgivable` at
  `unforgivable` or below, or at war.
- **Trade view.** `RelationView.tradable?: TradableView` is what another major could give this player, as Unciv's
  trade screen shows it, but only while this player holds an embassy in their capital (`ourEmbassy`) and the two
  are at peace (R7 in ROOMS-SPEC.md): `{ gold, goldPerTurn, luxuries, strategics, cities, openBorders, embassy }`,
  resources as `{ resource, amount }` and cities as `EntityRef`, each list sorted by id. Each item, asked for
  alone, is one the engine's acceptance takes from them: cities, open borders and embassy by `trade::can_give`
  (cities only those the player knows of, never the capital), amounts up to their treasury,
  `available_gold_per_turn` and `available_stock`. Proposals still reveal nothing beyond this. Absent for
  city-states and for a civ's own relation.
- **Spies.** `SpyView.stealCost?` is the science a stealing spy's `progress` must reach for the next theft.
  Coup odds are `CityStateRelationsView.coupChance`; forbidden cities come from `spyingForbidden`.
- **Great people.** `UnitOptions.great_person_abilities?: GreatPersonAbility[]` lists every ability a great person
  has, `{ action: GreatPersonAction, available, why?: GreatPersonRefusal, amount? }`, legal or not.
  `GreatPersonRefusal` is `noMovement | nothingResearched | cannotBeHurried | notInOwnCity | notABuilding |
  nothingToAdd | notACityState | atWar | cannotBuildHere | notAble`. `amount` is the science (`hurryResearch`),
  hammers here (`hurryProduction`) or gold (`tradeMission`) it would bring now. `great_person_actions` keeps
  listing only the legal ones.
- **Great person progress.** `KnowledgeView.greatPeople?: GreatPersonProgressView[]` is this player's own progress
  (no other nation's is ever sent), one entry per kind it can earn by points, sorted by `unit` then civilian before
  combat: `{ unit, points, perTurn, threshold, pool?, combat }`. `unit` is the kind's unit id in `RuleTables.units`
  (as `greatPersonPoints` rows name it; a nation's own version is what is born). Civilian kinds are those a
  building or specialist earns points toward: `perTurn` is what its cities earn now, and `threshold` is shared by
  every kind with the same `pool` (the unit's great person group, `""` for none) and doubles at each birth in it.
  Combat kinds (`combat: true`, no `pool`, `perTurn` 0) are earned from fighting anyone but barbarians, each with
  its own threshold, 200 then 50 more a birth. The faith-born prophet is not listed. Absent for a city-state.
- **Defender's owner.** `attacked.defenderOwner?: number` is the nation that owned the defending unit or city when
  the blow landed. Every `attacked` sent carries it, so a fight whose defender died still says whose it was.
- **Attacker's owner.** `attacked.attackerOwner?: number` is the attacker's nation, present when this player
  attacked or could see the attacker (the sight that filters `flankers`), absent when an unseen unit attacked it.
- **Eras.** `PlayerView.eraStarts?: number[]` is the turn this player reached each era, indexed by era number (in
  `RuleTables` era order) up to its current era. Starting eras and skipped eras read the turn it got there, so the
  list never decreases. The engine records it at each round end and saves keep it; a save made before
  this reads the turn it was loaded on for the eras already reached.
- **Rooms.** `GameUpdate.seats?: SeatStatusView[]` appears when the session has more than one human seat:
  `{ seat, ready, controlled, connected?, name?, left? }`. `ready` and `controlled` come from the session;
  `connected`, `name` and `left` from the host (`connected` is recorded as a session input, so a connect or
  drop sends the room an update and replays deterministically). Solo updates carry neither.
- **Events.** `PresentationEvent` gains `greatPersonBorn { name, unit: EntityRef | null }`, `greatPersonActed {
  outcome: GreatPersonOutcome }`, `cityCompleted { city, construction: ConstructionRef }`, `questOffered {
  cityState, quest, name }`, `questCompleted { cityState, quest, name, influence }`, `questExpired { cityState,
  quest, name }`, `cityStateRelationship { cityState, from, to }` (names as in `relationship`: `Unforgivable`,
  `Enemy`, `Neutral`, `Friend`, `Ally`), `goldGifted { cityState, gold, influence }` (influence as a decimal
  string), `tributePaid { cityState, gold, worker }`, `electionRigged { cityState }`, `denounced { by, target }`,
  `cityConverted { city, religion }`, `weLoveTheKingDay { city, resource }`, `weLoveTheKingDayEnded { city }`,
  `bordersExpanded { city, tile }`, `cityBurned { city, population }` and `cityRazed { city }`, plus the demand
  events above. `rebelsSpawned` gains `tile?` and `unit?` when the rebel is seen. `GreatPersonOutcome` is
  `science { amount, learned: number | null } | production { city, hammers } | tradeMission { cityState, gold,
  influence } | goldenAge { turns } | constructed { tile, improvement }`.
- **Puppets.** A human's puppet whose queue is empty now picks a building each turn as the AI would choose one
  (buildings only: a puppet raises no units for a master who did not ask). AI puppets are unchanged.
- **Restore.** `Session::restore` recomputes each seat's published projection instead of refusing a save whose
  stored projection differs, so saves written before a view gains a field still open.

Matches no one plays (the spectator landing page, the cinematic page and the art review) are sessions too:
`Session::watch` deals every civilization to a bot and seats a watcher at 0, who sees seat 0's view or, asked for,
every tile. It takes no commands and is never saved; `stage_round` closes each round by `advance_turn`, the all-AI
driver those pages always played (docs/AI.md), and the watcher's update is an ordinary update, sent as a delta.
`HostedGame.watch`/`step` host it in `watch-worker.ts`; `watch.ts` applies the deltas. The wire is unchanged.
Human controls use `openSolo`/`openRemote` in `session-transports.ts`.
`openRemote` returns one `RemoteSession` across replacement sockets. Transient failures retry up to
eight times, using exponential backoff from a 250 ms ceiling up to 10 seconds, with 50–100% jitter.
The retry budget resets after 30 seconds of stable connection. Each replacement socket receives a
fresh welcome and recovers the same durable outbox before queued commands resume; retries retain
their original command IDs and payloads. Existing subscribers receive the fresh view without
replayed transient events. An older tab fenced by `connectionReplaced`, incompatible data, or an
exhausted retry budget stops permanently and preserves unresolved commands. `close()` cancels
reconnect; the optional fourth argument `{ signal: AbortSignal }` also cancels an initial opening.
The dial's hub says when a room's socket is being replaced (`web/src/state/pulse.ts`).

Competitive pacing remains product work. Engine coverage targets the game's own bundled rules (`base`, and `blitz`
over it); arbitrary mod-only rules and editor tools are excluded. Server tests use two real WebSocket clients;
finishing a full match in two browser windows remains an acceptance test (PLAYABLE.md).

The planner suppresses unnecessary worker/settler commitments, values population-dependent building
yields, and budgets military strength using currently available units. Production and gold/faith
purchases share prices derived from the current city, owner and empire size. Production keeps each
item's investment when switching, caps completion overflow, revalidates paused work, refunds a lost
wonder once, and waits for a legal unit placement. Save/restore preserves that state.

Ordinary unit recovery now uses territory, action history, healing modifiers and nearby medics;
cities recover at round boundaries. Domination checks original major capitals and surviving majors
that have not founded yet. The original winning turn is persisted. These rules have regression tests. The explicit release match gate exercises complete sessions, periodic save/restore and replay; competitive balance needs playtesting.

Barbarian city attacks raid gold and leave the city standing. Losing the last city of an established
civilization removes its remaining units and releases its human turn slot. Capital-ownership history
is saved so a remaining worker cannot keep that civilization active after reconnect. The release match gate runs three four-major, two-city-state sessions with bot takeover, saving and restoring every 25 rounds and verifying full replay at the end. This checks headless match continuity; browser play and balance need separate validation.

The command boundary includes every victory path, policy finishers, construction gates, air combat and carriers, nuclear actions, religious choices, city-state diplomacy/quests, spies, city administration and national rule effects. Initial setup grants Agriculture and nation-equivalent starting units in legal positions. Private queries expose owned decisions and observations; they do not expose foreign treasury, technology, quests, production or spies.

Public matchmaking,
accounts and rankings remain platform work.
