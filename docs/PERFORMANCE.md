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
