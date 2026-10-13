# Bevy client: design brief

Status, 2026-10-13: decisions D1-D9 taken (D4 deferred), visual choices V1-V5 taken. S0 passed on native,
Chromium (WebGPU and WebGL2); Safari's frame timing is still unmeasured. M0 built and verified. Next: M1.

## Problem

A new client for this engine and server, in Bevy 0.20 (released 2026-10-08; there is no Bevy 2.0). It is designed
from two inputs only: the server's contract and what a player has to see and do in a turn. It does not port the
removed Svelte/three.js client, and it does not continue or read the Bevy 0.19.1 client started on 2026-09-25
in clockwork (tag `archive/civ-client`, D8).

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

## Decisions

Taken by Javed on 2026-10-13. The options he did not pick and their trade-offs are in git history (5faae54).

| | Decision | Pick |
|---|---|---|
| D1 | Targets | Native and wasm from one codebase |
| D2 | Solo play | In-process `Session` |
| D3 | Wire types | The client depends on civ-sim as it is |
| D4 | Native auth for rooms | Deferred until a native client joins rooms |
| D5 | First ruleset | `base` |
| D6 | First playable | Watch bots play (`Session::watch`) |
| D7 | Visual direction | 3D terrain map at a fixed oblique angle; spec in [VISUAL-SPEC.md](VISUAL-SPEC.md) |
| D8 | The 09-25 Bevy client | Ignored, not read |
| D9 | Where the engine runs in wasm | On a wasm thread inside the client (shared memory, atomics) |

### What D1, D2 and D9 mean for the build

- One `Link` implementation for solo on both targets: the Session runs on its own thread and talks to the
  Bevy world through channels. Native uses `std::thread`. Wasm uses a Web Worker that shares the module's
  memory, because `std::thread::spawn` panics on `wasm32-unknown-unknown` even with atomics; a small spawn
  shim (or a crate such as `wasm_thread`, chosen in the spike) stands in for it.
- The wasm build needs a pinned nightly toolchain, `-C target-feature=+atomics,+bulk-memory`,
  `-Z build-std=std,panic_abort` and wasm-bindgen's `--target web`. Native stays on stable. As of the sources
  found on 2026-10-13, none of this is stable Rust.
- Found in S0: on this nightly `+atomics` no longer makes the linker share memory. Without
  `--shared-memory --import-memory` and the `__wasm_init_tls`, `__tls_size`, `__tls_align`, `__tls_base`
  exports, wasm-bindgen emits an unshared memory and posting it to a worker fails with a DataCloneError.
  `web/build.sh` passes them to the final link only.
- The page that serves the wasm build must be cross-origin isolated: `Cross-Origin-Opener-Policy: same-origin`
  and `Cross-Origin-Embedder-Policy: require-corp` (or `credentialless`). Every resource it loads must allow
  that.
- The browser's main thread may not block on a lock or a channel: an atomic wait there traps. The Bevy side
  only ever uses non-blocking receives; the engine thread may block freely.
- Bevy's own task pools stay single-threaded on wasm. Only the Session gets a second thread.
- D3 with wasm: civ-sim's `#[wasm_bindgen]` exports (`HostedGame`, `ruleTables`) would compile into the
  client. A cargo feature in civ-sim, off for the client, keeps them out. That is a small server-crate change,
  made in its own commit.

## Milestones

| | Milestone | Verified by |
|---|---|---|
| S0 | Spike, the gate for D9: a Bevy 0.20 wasm build with atomics; one engine thread runs `Session::watch` on a `base` 32x20 map while the main thread renders | 60 fps held through AI rounds in Chrome and Safari on this Mac; same binary logic native; build steps written down |
| M0 | `crates/civ-client`: Link (solo, both targets), ViewState, the delta test; headless | Test green over seeded bot games, native and under wasm |
| M1 | The map per VISUAL-SPEC: terrain, water, features, rivers, roads, borders, fog, camera, picking, tile inspector, drawn from a welcome | Screenshot sets at fixed seeds for your review, native and wasm |
| M2 | First playable: spectator. Bots play; units, cities, presentation, replay, speed controls, follow events | A 100-round bot game with no resync; frame budgets met on both targets |
| M3 | Your turn: move, attack, found, production, research, finish; Outbox, receipts, undo | Scripted games; duplicate and retry tests |
| M4 | Cities, research, policies, diplomacy offers | Query-driven UI tests |
| M5 | Rooms: remote Link, lobby, reconnect, deadlines (D4 decided here) | Two clients finish a match on a local civ-server |
| M6 | The remaining actions, victory, saves | Every action reachable from a control |

If S0 fails, D9 comes back to you with what broke; nothing after it starts.

Results, 2026-10-13, 32x20 `base` match (seed 1), host thread closing rounds back to back:

| Run | Frames while the host works | Rounds | Link |
|---|---|---|---|
| Native, dev build, M1 Pro (Metal) | 119.6 fps at 120 Hz, p99 9.3 ms, one 22 ms frame | to turn 203 (the end), about 28 ms apart | no resync |
| Chromium 152 pane, WebGPU (S0 code) | 79 fps, p99 11.7 ms, in the one window it was shown | about 29 ms per round | n/a |
| Headless Chrome, WebGPU (M0 code) | 60 fps (headless rate), p99 17.8 ms, max 18.1 ms | to turn 203, 35-76 ms apart | no resync, nothing stale |
| Headless Chrome, WebGL2 (M0 code) | 60 fps, p99 17.4 ms, max 18.7 ms after start-up | to turn 203 | no resync, nothing stale |
| Safari 27.0.1, WebGPU (S0 code) | not measured: the window was hidden, so WebKit drew 2 frames | 559 rounds over three matches on the host thread | n/a |

## Risks and unknowns

- Wasm threads on nightly: a pinned nightly can break with Bevy or wgpu updates; `-Z build-std` lengthens wasm
  builds. Some crates misbehave on shared memory (getrandom once crashed there).
- No source found that confirms Bevy 0.20 itself on a threaded wasm build. S0 settles it.
- Cross-origin isolation constrains hosting: any third-party script, font or image must send CORP or CORS
  headers.
- Bevy 0.20 is five days old: BSN and Feathers APIs may still move, and community crates may not support it yet.
- Welcome size on real map sizes is unmeasured; 7 KB is a 20x14 map. Map sizes run from 16x10 to 128x80.
- In-process round time on large maps on this Mac is unmeasured.
- MPL-2.0: a shipped client that links civ-core distributes MPL code; private use is unaffected.

## Verification

Headless tests run Bevy with minimal plugins plus ViewState and Projection, against in-process Sessions, native
and under wasm (wasm-bindgen-test in a browser). Screenshot sets at fixed seeds for visual review. Frame-time
budgets from VISUAL-SPEC measured on this Mac.

## Rollback

The client is one new crate and its workspace entry; removing both restores the server-only tree. The civ-sim
feature for its wasm exports lands as its own commit and reverts alone.
