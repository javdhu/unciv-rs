# Performance changes

Phase 2 continues the graphics work from `b885ce6`.

- Terrain dressing retains unchanged 8×8 blocks. The land surface is stored a block at a time and
  `Land.grow` resamples only faces within three rings of a changed tile (frontier, foundation, river,
  ridge, cliff), from a sample two rings wider; other blocks keep their shape and GPU positions and
  normals. On an 80×52 map with 2,841 tiles known, a scout's step costs 16.5 ms instead of a 650 ms
  full mesh, keeping 309 of 329 blocks (`world/test/land.test.ts` checks every face bit for bit
  against a full mesh while the frontier grows).
- Protocol 9 sends deltas between filtered player views. Welcome/resync remain full snapshots;
  unchanged browser entities retain their references.
- Staged world copies share large stores until mutation. Transaction and undo isolation remain intact.
- Line-of-sight results are cached by exact local inputs. Visibility masks and full player projections
  are still assembled per transition.
- Game layout reads baked model measurements. Geometry generation stays in art workers; development
  tools can generate directly. `bun run art:measure` refreshes the committed metadata.
- Core/session save format 4 embeds JSON directly and omits derived seat projections. Older saves
  are rejected; no migration is provided.

## Rendering

- The ground's per-tile data is two textures (`COLOURS` in sRGB, `DATA` linear, several texels a tile) instead of
  nine. The shared art material with painted ground slots, wind and the veil binds 10 texture units instead of 16;
  the land 9 instead of 14, the sea 6 instead of 9.
- The ground, sea, wind and veil shader patches chain through one helper (`world/patch.ts`). Program cache keys carry
  the material's own key and each patch's name; the compiled programs are the same.
- The unit layer reads the frame's view from `ViewCull` instead of taking its own. City banners, counters, resource
  icons and yield chips share one keyed label pool; crowds and troupes share one slot pool (`world/slots.ts`).

Checked in the title scene against the old ground textures, in one page: the texels read back from the GPU match
byte for byte, and the frame differs by at most 1/255 on 78 of 3.1 million pixels, shader compiler rounding (a
null edit to the old shader moved 1,320 pixels by up to 6/255). Median frame 44.7 → 40.1 ms, interleaved, on a
heavily loaded machine: read it as no regression. Draw calls and program count are unchanged.

## Measurements

The existing `civ-sim` `save_profile` example, on the same machine and release settings
(`CARGO_PROFILE_RELEASE_LTO=false`, `CARGO_PROFILE_RELEASE_CODEGEN_UNITS=16`):

| Measurement | Baseline | Phase 2 |
|---|---:|---:|
| Session save | 3.360 ms | 1.261 ms |
| Snapshot size | 385,312 bytes | 344,637 bytes |
| First 12 rounds, total | 61.4 ms | 47.5 ms |

These are one-run local samples, not FPS or general workload guarantees. Reproduce with
`cargo run --release -p civ-sim --example save_profile` and the settings above.

### Art between visits

An IndexedDB cache of finished parts (kept on branch `perf/art-cache`) was measured against production builds
in headless Chrome: the title's 402 parts drained in 434–447 ms on a second visit instead of 481–559, no faster
under a 4× CPU throttle, where the main thread building the scene is the cost, and it stored 24 MB (up to
128 MB) per player. Not worth the disk; not merged.

### The client after deltas

The architecture review proposed one id-indexed client model in place of the stores and layers that each walk
the view. Measured on 70e0f13 in a solo game (Chrome's long-animation-frame entries while founding a city and
playing turns with a roaming unit): the session listeners take about 2 ms per update, and no frame blocked during
turns, AI turns included; founding a city blocked one frame for 94 ms, mostly the first draw of the new town. With
unchanged entities keeping their identity and the land rebuilt per block, the model would save a few
milliseconds per update, so it is not built. Revisit it if a profile shows the stores again.

## CI

A job whose inputs (sources, lockfile, toolchain, workflow) match a tree that already passed skips its work, and
the browser engine is cached by the same inputs: a client-only push takes about 2 minutes (the web shards), one
that changes Rust about 7 (engine tests 6 minutes with each crate's integration tests one binary, down from 9;
the browser engine build 4). The determinism check runs as its own job beside them.

## Validation and known limitation

The Rust workspace (1,125 tests) and browser suite (4,579 tests) pass. Native and Wasm
fingerprints match (`0x3b7e53530a3e8b62`); Linux containers were not checked. The suites cover transaction isolation, delta reconstruction and
resync, saves, exact partial/full terrain mesh equality, block-boundary dressing, overlapping terrain
updates, and baked bounds. Desktop and phone smoke scenes exercise starting, founding a city and turns.

The separately ignored `complete_matches_save_and_replay` gate fails on both the baseline and phase 2:
seed 1, turn 150, `diplomacy references an invalid civilization pair`. The captured world contains
self-influence `(5, 5)`. This pre-existing simulation defect is outside the performance changes; the
long-match save/replay gate is not green.

## Client leftovers

- The end screen's history (`state/history.ts`) records a turn from the owners as of its last entry, cached per
  history, instead of replaying the whole border log. It is kept in chunks: a turn rewrites only the open chunk
  (at most 33 turns) and older turns are sealed once under keys of their own. A history kept whole reads as one
  open chunk and is sealed at its next turn.
- A hidden tab idles. The Blitz clock ticks four times a second only while the page shows; hidden, it wakes when a
  count runs out or the match moves, so the turn still ends on time, and it counts from timestamps. The music's
  conductor rests with the suspended audio context and resumes where it stopped (while shown it keeps time even
  when silent, for the clock's ticks). The lobby keeps reading while hidden: the reads are its presence (the host
  sees a member away after three missed ones) and how it learns the match opened.
- The observer engine path (`engine/worker.ts`, `transport.ts`) stays: it runs the all-bot matches of the spectator
  landing page, the cinematic page and art review (`?turns`). `HostedGame` always seats a human at 0 and has no
  see-all view, so moving them onto the session path would leave the watched civ standing still; that needs an
  engine change first.
- Art workers finish their parts (`art/finish.ts`): the float slot and contact-shade attributes and the bounding
  sphere, the latter step for step as three.js computes it. The main thread only wraps the buffers. A test checks
  every catalogue entry, near and far, is bit-identical to the old main-thread path.

| Measurement | Before | After |
|---|---:|---:|
| History, record + keep, turns 350–400 of a 60×38 map | 0.85–1.07 ms/turn | 0.05–0.12 ms/turn |
| Hidden tab, Blitz clock + conductor timer wakeups | 1/s each (browser-throttled 4/s and 25/s) | none until due |
| Main-thread finishing, 1,890 catalogue parts (all variants, detail 0 and 2) | 33.5–35.6 ms | 2.0–3.8 ms |

Local samples (bun, M4 Pro, storage in memory, so real `localStorage` writes widen the gap). History:
`bun <scratch>/bench-history.ts` beside `web/src/state/`, comparing the base commit's `history.ts` with this one.
