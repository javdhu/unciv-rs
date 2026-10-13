# Bevy client: design brief

Status: proposal for Javed's decision, 2026-10-13. Nothing is built.

## Problem

A new client for this engine and server, in Bevy 0.20 (released 2026-10-08; there is no Bevy 2.0). It is designed
from two inputs only: the server's contract and what a player has to see and do in a turn. It does not port the
removed Svelte/three.js client, and it does not continue the Bevy 0.19.1 client started on 2026-09-25 in
clockwork (tag `archive/civ-client`), unless D8 says otherwise.

## What the server gives

These are fixed. Changing any of them is a server change.

| Area | Contract |
|---|---|
| Transport | HTTP for guests and rooms (`civ_guest` cookie, exact `Origin` allowlist). One WebSocket per seat per match: `hello {version: 9}` gets `welcome {contentHash, update}`. Up: `command`, `query`, `resync`. Down: `receipt`, `queryReply`, `update`, `delta`, `error`. |
| State | `GameUpdate {cursor {epoch, revision}, turnToken, ready, controlled, pendingControl, deadlineMs, view, offers, events, replay, undo, seats}`. `PlayerView` has 27 top-level fields; `tiles`, `units`, `cities`, `civs` are keyed (tile `u16`, unit and city `(id, gen)`, civ `u8`). 53 view types in `civ-sim/src/view.rs`. |
| Deltas | Upserts for those four collections plus removed keys; every other field is absolute. `base` must equal the client's cursor, else the client sends `resync`. civ-sim computes deltas (`delta.rs`); no Rust code applies them yet (the TS client did). |
| Verbs | 67 actions, 7 queries: `reachable`, `unitOptions`, `cityOptions`, `civOptions`, `view`, `rules`, `combatPreview`. |
| Grid | Pointy-top hexes, odd rows shifted right (odd-r), `id = row * width + col`, optional wrap in x. Neighbour order E, NE, NW, W, SW, SE. |
| Solo authority | `civ_sim::session::Session` runs natively: `new`, `watch` (bots only), `query`, `prepare`/`commit`, `snapshot`/`restore`. The server is the same Session behind SQLite and a socket. |
| Rules | Static tables once per content hash (`rules` query or `GET /api/rules`). A session loads `base` or `blitz`; Unciv's `vanilla` and `gods-and-kings` in `rules/` are only used by tests. |
| Measured 2026-10-13 | 20x14 map, two seats: welcome 7.0 KB JSON; the delta after one round 1.9 KB. |

## Constraints

- Updates are the only path that changes client game state. Receipts only resolve pending input.
- In rooms the client never holds the hidden world: fog is applied on the server. An in-process solo Session
  holds the full world in memory, which is no security boundary (the web Worker wasn't either).
- Commands carry a stable `commandId`. After an unknown outcome, retry the same ID and payload.
  `finishTurn` must not overtake earlier commands.
- AI rounds took about 420 ms of engine time per round in the browser (perf notes, 2026-09-25). In-process
  play runs the engine off the main thread.
- Bevy 0.20 brings BSN scenes, Feathers widgets and WESL in place of the WGSL dialect. Third-party Bevy crates
  usually trail a release by weeks, so the plan assumes first-party Bevy only.

## Non-goals for the first milestones

No server changes beyond what D3 or D4 pick. No art pipeline, audio, matchmaking or accounts. No client-side
prediction.

## Architecture

```
                      ┌─────────────────────────── civ-client (Bevy 0.20) ───────────────────────────┐
 mouse / keys / touch─▶ Input and intent ──orders──▶ Outbox (pending commands, stable ids, in order)    │
                      │      ▲  queries                     │                                          │
                      │      │                              ▼                                          │
                      │      │          ┌──────────────── Link (one interface) ───────────────┐       │
                      │      │          │ in-process: Session on a worker thread       (D2)   │       │
                      │      │          │ remote: WebSocket JSON to civ-server                │       │
                      │      │          └──────────────────────────────────────────────────────┘       │
                      │      │                  │ welcome / update / delta / receipt / queryReply      │
                      │      │                  ▼                                                      │
                      │   ViewState (resource): PlayerView + cursor + token; deltas applied here       │
                      │      │ changed keys                              │ events, replay batches      │
                      │      ▼                                           ▼                             │
                      │   Projection: entities keyed by          Presentation: timelines that          │
                      │   tile / unit(id,gen) / city(id,gen)     animate from old state to new         │
                      │      │                                           │                             │
                      │      ▼                                           ▼                             │
                      │   Render: hex chunks, fog, pieces; UI (BSN + Feathers), driven by queries      │
                      └───────────────────────────────────────────────────────────────────────────────┘
```

## Components and contracts

1. Link. `submit(CommandRequest)`, `query(Query) -> requestId`, `resync()`, an inbound stream of
   `ServerMessage`, and a status (live, reconnecting, stopped with a code). Both implementations emit the same
   `ServerMessage` values, so everything above Link has one code path. In-process wraps a Session on a thread
   with channels; remote is a WebSocket client.
2. ViewState. Owns the latest `PlayerView`, cursor, turn token, deadline, offers, undo and seats.
   `apply(ServerMessage) -> Changed {tiles, units, cities, civs, scalars}`. Invariant: after any run of welcome
   and deltas, ViewState equals the server's full view at that cursor. Enforced by a test that drives real
   Sessions through seeded bot games, pushes `delta::message` output through `apply`, and compares with
   `current_update` every round. A base mismatch sends `resync`; the client never guesses.
3. Projection. Spawns, updates and despawns entities from `Changed` keys only, through a
   `HashMap<Key, Entity>`. Entities carry render state (transform, mesh handles, animation state); game facts are
   read from ViewState, never copied into components, so there is one source of truth.
4. Presentation. A queue of `PresentedEvent`s (deduplicated by their viewer-scoped ids) and replay batches,
   played as timelines: moves along their path segments, battles, founding, captures. During playback a piece
   may be drawn behind the view; when the queue drains, visuals equal the view. Skippable; a resync clears it.
   Input always reads ViewState, never the visuals.
5. Outbox. Pending commands keyed by `commandId`, sent in order; `finishTurn` waits for earlier receipts.
   On reconnect it resends unresolved commands with the same id and payload. Written to disk before sending, so
   a crash cannot lose an outcome.
6. Map renderer. Odd-r pointy-top geometry. Terrain in chunks, rebuilt only when a changed tile falls in
   one. Three fog states from the view: absent (never seen), remembered (`observed_turn`), visible. Wrap in x
   by drawing the chunk strip twice and wrapping the camera.
7. UI. Every control asks the server what is possible (`unitOptions`, `cityOptions`, `civOptions`,
   `reachable`, `combatPreview`) instead of re-deriving legality in the client.
8. Rules cache. Tables on disk keyed by content hash.

### Data flow and state ownership

The server (or in-process Session) owns all game state. ViewState holds the last received knowledge.
Projection and Presentation own visuals only. The client alone owns camera, selection, pending intentions,
settings and animation clocks.

One move: select a unit, `reachable` query, hover draws the path, click, Outbox submits `moveUnit`, the receipt
(applied, partial, rejected) clears the pending marker, the delta lands in ViewState, Projection moves the
entity and Presentation animates the step events. `finishTurn` closes the round; the update carries replay
batches, Presentation plays the other civs' turns, and the visuals settle on the new view.

## Alternatives considered

| Alternative | Why it loses |
|---|---|
| Mirror all game state into ECS components and write deltas into them | Two copies of the truth and a hand-written reducer per field. ViewState plus a keyed projection applies deltas to the server's own types. |
| Rebuild the scene on every update | Breaks animation continuity; rebuilds terrain for one moved unit. |
| Client-side prediction | Hides latency that a turn-based game barely has, and needs rollback. Local receipts arrive within a frame or two. |
| Re-derive legality in the client from civ-core | Duplicates authority. The options queries already answer it. |
| Generate types from the JSON schemas | Unneeded in Rust; linking the protocol types removes drift. |

## Open decisions

Each one is yours. They are listed with their trade-offs and no pick.

D1. Targets.
(a) Native macOS only: simplest build, threads for in-process play, no binary size limit.
(b) Native and wasm from one codebase: browser play like civ.game had; WebGPU or WebGL2 limits, no
std threads without extra setup (in-process AI would block a frame), wasm size (the old engine alone was
0.84 MB brotli; Bevy adds several MB), browser handles cookies and Origin for rooms.
(c) Wasm first: same costs as (b), with native as a by-product.

D2. Solo path.
(a) In-process Session on a worker thread: no network stack for solo, saves are Session snapshots; two Link
implementations.
(b) Spawn a local civ-server and always speak WebSocket: one Link implementation; process management, the
cookie and Origin handshake, and a SQLite file per solo game.
(c) Rooms only, no solo: smallest client; nothing to play offline.

D3. Where the wire types live.
(a) The client depends on civ-sim as it is: pulls civ-core and wasm-bindgen into the client. Free if D2 is (a).
(b) Extract a wire-types crate (protocol plus view types, serde only) that civ-sim, civ-server and the client
share. View types sit in `view.rs` beside the projection code, so this is a real refactor of the server's crate
graph.

D4. Native client auth for rooms (only if a native client joins rooms).
(a) The client sends an allowed `Origin` and keeps the `civ_guest` cookie itself: no server change.
(b) Add a bearer-token mode to civ-server for non-browser clients: a server interface change.

D5. Ruleset to build against first. `base` (the whole game), `blitz` (one evening), or Unciv's `vanilla` or
`gods-and-kings`, which a session cannot load today and would need content work.

D6. First playable milestone.
(a) Spectator: `Session::watch`, bots play, nothing to click. Proves map, fog and presentation first.
(b) Your turn: move, attack, found, produce, research, finish, against bots.
(c) Rooms first: remote Link and lobby before solo.

D7. Visual direction. Pick an axis (for example a flat 2D hex board, a tilted 3D tabletop, or one you name);
I then write the full spec with exact numbers for your approval before any art is built.

D8. The Bevy 0.19.1 client of 09-25 (clockwork tag `archive/civ-client`: terrain, camera, picking,
inspector). (a) Ignore it. (b) Read it for engineering facts only (hex maths, wrap handling), never its look.

## Milestones

Contents shift with D1 to D6.

| | Milestone | Verified by |
|---|---|---|
| M0 | `crates/civ-client`, Link, ViewState, the delta test; headless | Test green over seeded bot games |
| M1 | Map, camera, picking, fog, drawn from a welcome | Screenshots at fixed seeds for your review |
| M2 | Spectator: watch mode, presentation, replay batches | A 100-round bot game with no resync; frame time measured on this Mac |
| M3 | Your turn: the six first verbs, Outbox, receipts, undo | Scripted games; duplicate and retry tests |
| M4 | Cities, research, policies, diplomacy offers | Query-driven UI tests |
| M5 | Rooms: remote Link, lobby, reconnect, deadlines | Two clients finish a match on a local civ-server |
| M6 | The remaining actions, victory, saves | Every action reachable from a control |
| M7 | Art to the approved visual spec | Your review |

## Risks and unknowns

- Bevy 0.20 is five days old: BSN and Feathers APIs may still move, and community crates may not support it yet.
- Welcome size on real map sizes is unmeasured; 7 KB is a 20x14 map.
- If the client links civ-sim on wasm, civ-sim's own `wasm_bindgen` exports come along: size and symbol clashes
  untested.
- In-process round time on large maps on this Mac is unmeasured.
- MPL-2.0: a shipped client that links civ-core distributes MPL code; private use is unaffected.

## Verification

Headless tests run Bevy with minimal plugins plus ViewState and Projection, against in-process Sessions and a
civ-server started inside the test. Screenshot sets at fixed seeds for visual review. Frame-time budgets
measured on this Mac.

## Rollback

The client is one new crate and its workspace entry; removing both restores the server-only tree. Any server
change from D3 or D4 lands as its own commit.
