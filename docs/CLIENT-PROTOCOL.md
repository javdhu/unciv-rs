# Client communication contract

Status: shared session, save/replay, solo Worker and private-room host implemented locally.
The main UI plays solo sessions through the Worker; remote rooms are not in the UI yet. Broader rule parity,
complete match playtests, public matchmaking and ranked play remain unfinished.

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

Start with full views; static rules tables travel once with a content hash. Keep camera and selection
local. WebSocket lacks automatic application backpressure: bound queues and replace obsolete pending
snapshots with a current resync snapshot for slow clients. Reset obsolete presentation playback, but
keep command results recoverable. Publish only permitted knowledge changes, not a pulse for every
hidden action.

Measure encoded bytes, serialization, update frequency and rendering before adding patches or binary
encoding. Later patches diff already-projected views, use explicit upserts/removals and a base revision,
and fall back to full resync. Do not diff the world then redact. JSON quantities have explicit units;
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
  acknowledgment. Compare-and-swap prevents two tabs from overwriting the same save.
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

The original `Game`/`worker.ts`/`transport.ts` path now serves only the art review page. Human controls use
`openSolo`/`openRemote` in `session-transports.ts`.
`openRemote` returns one `RemoteSession` across replacement sockets. Transient failures retry up to
eight times, using exponential backoff from a 250 ms ceiling up to 10 seconds, with 50–100% jitter.
The retry budget resets after 30 seconds of stable connection. Each replacement socket receives a
fresh welcome and recovers the same durable outbox before queued commands resume; retries retain
their original command IDs and payloads. Existing subscribers receive the fresh view without
replayed transient events. An older tab fenced by `connectionReplaced`, incompatible data, or an
exhausted retry budget stops permanently and preserves unresolved commands. `close()` cancels
reconnect; the optional fourth argument `{ signal: AbortSignal }` also cancels an initial opening.
The connection-status display remains a UI task.

Competitive pacing and browser controls remain product work. Engine coverage targets the bundled Gods & Kings rules; arbitrary mod-only rules and editor tools are excluded. Server tests use two real WebSocket
clients; finishing a full match in two browser UIs remains an acceptance test after UI integration.

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
