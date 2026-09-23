# civ.game architecture

The selected communication contract is [CLIENT-PROTOCOL.md](CLIENT-PROTOCOL.md). The shared human
session and private-room server are implemented locally. The Svelte/Three.js UI plays solo sessions
through `openSolo`; [PLAYABLE.md](PLAYABLE.md) tracks the controls still to build.

## Ownership

| Module | Owns |
|---|---|
| `civ-core` | World, rules, legal actions, AI, knowledge and round processing |
| `civ-sim::session` | Authenticated-seat interface, readiness, retries, views, snapshots and replay |
| `civ-sim::protocol` | Wire DTOs and generated TypeScript/JSON Schema |
| Solo Worker | One session, serial input, IndexedDB persistence before publication |
| `civ-server` | Guest/seat identity, private rooms, ordered match input, SQLite, clocks and reconnect |
| `SessionClient` | Validated messages, pending outbox, view revisions, query correlation and subscriptions |
| UI | Intentions, latest received view, selection, camera, animation and rendering |

Commands carry intentions and generational entity IDs. The host supplies the actor; clients cannot
choose another civilization's authority. `FinishTurn` locks one seat. The host closes the shared round
when all human seats are ready or the deadline expires. Bots use the same command legality; human
seats receive automatic economy/refresh without AI decisions.

The session prepares a replacement state without mutating the live state. Its host commits the snapshot
and receipt, replaces the live session, then publishes filtered updates. Duplicate command IDs recover
the original receipt before current-turn eligibility is checked. Reused IDs with altered content reject.

## Source map

```
crates/civ-sim/src/
  content.rs       complete embedded rules bundle, identity and engine compatibility version
  setup.rs         bounded settings, seeded map and initial civilizations
  protocol.rs      versioned commands, queries, receipts and full-view updates
  session.rs       staged transitions, access, receipt history, save/restore and replay
  offers.rs        immutable human consent, private offer projection and expiry
  hosted.rs        Wasm staging/commit adapter for the solo Worker
  view.rs          persistent player knowledge and typed presentation projection
  rules.rs         public static content tables
  lib.rs           exports and the original observer Game API
crates/civ-server/src/
  host.rs          per-match executor, connection generations, deadlines and controllers
  store.rs         SQLite identity, rooms, authoritative snapshots and receipt transactions
  http.rs          guest/room HTTP API, authenticated WebSocket and Origin checks
web/src/engine/
  generated/       Rust-generated types and runtime schemas; do not hand-edit
  validation.ts    incoming protocol validation and integer/cursor checks
  session-client.ts shared submit/query/subscribe interface and durable retry recovery
  storage.ts       IndexedDB transactions, outbox and solo compare-and-swap
  session-worker.ts solo host; persist staged state before acknowledge/publish
  session-transports.ts Worker and WebSocket adapters
  protocol.ts      the renderer's view contract, a subset of the wire view
  worker.ts, transport.ts   original observer adapter, still used by art-review.ts
web/src/state/
  session.ts       live match stores; the UI's only path to the session
```

The engine is the workspace crate `crates/civ-core`. Its default build has no runtime dependencies;
optional `serde`/`save` features support authoritative persistence.

## Information boundary

Tile, city and civilization observations persist at the time of sight. Fogged entities use those
observations instead of today's hidden state. Resources require the observer's technology. Reachability
uses a world reconstructed from player knowledge and marks uncertain routes. Execution can therefore
stop after a legal prefix when movement reveals a blocker.

Views expose own economy and visible units. A whitelist projects typed presentation events; it never
falls back to engine Debug text. Foreign movement/battles without event-time attribution are omitted
until the engine can prove who observed them. Tests compare views and previews across hidden-state
changes. These protections do not establish complete rule parity or a finished combat presentation API.

Only updates replace client game state. Receipts settle pending input and name a view cursor. The
client buffers its current view for late subscriptions and ignores old revisions. Full views are used
initially; patches and binary encoding require measurements first.

## Persistence and networking

Native saves include complete arena generations/free-slot order, world/knowledge, versions and content
identity. Session saves add settings, epoch, turn access, receipts and ordered player/host inputs. Replay
reconstructs the game from its initial settings and those inputs; deterministic AI decisions remain
reconstructable under the pinned engine. Presentation events are not the authoritative save format.

Remote guests hold an opaque HttpOnly cookie; only its hash is stored. Invite codes cannot reclaim a
seat. Reconnect issues a new connection generation and fences the previous socket. Disconnect grace,
controller changes and round deadlines are persisted. Commands and clock decisions use one executor.
The local server owns its SQLite database exclusively; multi-process distributed hosting is deferred.

Browser pending requests are saved before sending. Reopening a session retries them in order with the
original IDs. Solo uses the same semantics, with an IndexedDB comparison against the previous snapshot
so a stale second tab cannot overwrite progress. Network failure means an unknown outcome; a receipt
must be recovered before dependent commands proceed. Remote sessions reconnect with bounded backoff,
hydrate a fresh view, and recover the outbox before queued commands resume. Fenced tabs stop permanently.

## Existing UI

`state/session.ts` holds the Svelte stores (rules, the latest update and view, the selected unit, the
open city, civ options) beside the Three.js renderer. The renderer does not calculate game rules. Terrain, cities, units and overlays have
separate owners; art entries map ruleset names to procedural recipes (docs/PROCEDURAL.md). Static
geometry is instanced, and so are units: every unit of one shape is a slot in one instanced mesh, posed in
the vertex shader from its state (`world/troupe.ts`). Quality follows the device (`world/perf.ts`). An
`attacked` presentation event names the attacker, the target, both tiles, whether it was ranged, the damage and
any deaths; it reaches the owners of both sides, and `world/combat.ts` animates fights from it instead of
inferring them from consecutive views. Fights between third parties still wait for event-time observation.

Human controls use `openSolo`; remote rooms will use `openRemote`. The old observer adapter remains only
for `art-review.ts`. The session provides commands for currently implemented engine actions, including economy,
unit/city actions, religion and diplomacy. Human bilateral agreements require recipient consent,
expire at turn boundaries, and are included only in the parties' updates and durable session state.
The supported engine content is the bundled Gods & Kings rules. Unciv editor/mod-only behavior is excluded. Public matchmaking, accounts, rankings and full browser match validation remain separate work.

## Local validation

See the root README for build and run commands. `tools/check_protocol.py` detects generated-schema
drift. Native tests cover human round access, hidden information, staged writes, idempotent purchases,
snapshot continuation and replay. Browser engine tests cover the pending outbox, update ordering,
runtime validation, IndexedDB compare-and-swap and actual Wasm human commands. Server tests exercise
real HTTP/WebSocket clients plus failed storage, restart, deadlines and connection fencing.
