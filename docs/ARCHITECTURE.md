# civ.game architecture

The selected communication contract is [CLIENT-PROTOCOL.md](CLIENT-PROTOCOL.md). The shared human
session and private-room server are implemented locally. The Svelte/Three.js UI still uses its original
AI observer adapter; integrating human controls is unfinished.

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
  protocol.ts, worker.ts, transport.ts   original observer adapter used by the existing UI
```

The engine remains a path dependency at `~/Downloads/Unciv-master/civ-core-rs`. Its default build has no
runtime dependencies; optional `serde`/`save` features support authoritative persistence. Moving or
publishing the engine is not part of the current local integration.

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

The observer demo retains the Svelte stores (`rules`, current/previous `view`, selection, busy) and the
Three.js renderer. The renderer does not calculate game rules. Terrain, cities, units and overlays have
separate owners; art bindings map ruleset names to model assets. Static geometry is instanced and units
use individual animated meshes. No human-control UI, layout, artwork or leader media was changed in
this integration.

New controls should use `openSolo`/`openRemote` and remove the old observer adapter when that UI is
replaced. The session provides commands for currently implemented engine actions, including economy,
unit/city actions, religion and diplomacy. Human bilateral agreements require recipient consent,
expire at turn boundaries, and are included only in the parties' updates and durable session state.
The supported engine content is the bundled Gods & Kings rules. Unciv editor/mod-only behavior is excluded. Public matchmaking, accounts, rankings and full browser match validation remain separate work.

## Local validation

See the root README for build and run commands. `tools/check_protocol.py` detects generated-schema
drift. Native tests cover human round access, hidden information, staged writes, idempotent purchases,
snapshot continuation and replay. Browser engine tests cover the pending outbox, update ordering,
runtime validation, IndexedDB compare-and-swap and actual Wasm human commands. Server tests exercise
real HTTP/WebSocket clients plus failed storage, restart, deadlines and connection fencing.
