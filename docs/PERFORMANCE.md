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

## Validation and known limitation

The Rust workspace (1,125 tests) and browser suite (4,579 tests) pass. Native and Wasm
fingerprints match (`0x3b7e53530a3e8b62`); Linux containers were not checked. The suites cover transaction isolation, delta reconstruction and
resync, saves, exact partial/full terrain mesh equality, block-boundary dressing, overlapping terrain
updates, and baked bounds. Desktop and phone smoke scenes exercise starting, founding a city and turns.

The separately ignored `complete_matches_save_and_replay` gate fails on both the baseline and phase 2:
seed 1, turn 150, `diplomacy references an invalid civilization pair`. The captured world contains
self-influence `(5, 5)`. This pre-existing simulation defect is outside the performance changes; the
long-match save/replay gate is not green.
