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
- Line-of-sight results are cached by exact inputs (see AI turns below). Full player projections are
  still assembled per transition.
- Game layout reads baked model measurements. Geometry generation stays in art workers; development
  tools can generate directly. `bun run art:measure` refreshes the committed metadata.
- Core/session save format 5 is binary: postcard, with fog memories as a tile bitset and run-length
  columns per civ, and the session's history as JSON inside (ARCHITECTURE.md, Persistence and
  networking). Derived seat projections are omitted. Older saves are rejected; no migration is provided.

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

## AI turns

An AI game on a medium map with 8 civs (seed 21, whole game, `sample`) spent 32% of engine time in
construction checks (every city asking about every unit and building, each ask summing the civ's happiness),
28% in sight (every step of every move recomputed every unit's sight and rewrote every civ's records), 9% copying
stores a staged world shared, and about 20% in pathfinding. Now, with no change to any game:

- Construction checks share one happiness and resource stock per civ (`construction::CivChecks`).
- A search gathers city centres, units by tile and its unit's rules once (`movement::Search`), not at every step.
- Line of sight is cached against a whole map of terrain heights, checked once per sweep; each unit's rules are
  walked once. Live masks and observation records are worked out first and written only where they change,
  records are shared per civ, and a capture skips explored tiles already on record. Needless writes after every
  action (alliances, resource stocks, contact) no longer copy shared stores.
- `world::fog::audit` (feature `sight-audit`) recomputes sight and records plainly beside every update and capture;
  civ-sim's `sight_audit` test plays a game under it, and an ignored one plays eight whole games (about 785,000
  audited steps covering embarkation, promotions, city-state alliances, spies and the Great Lighthouse).

`balance` engine time, back to back against a build of `1b94b3e`, outputs identical once `engineMs` is dropped:

| Run | Before | After |
|---|---:|---:|
| `--map small --civs 4 --city-states 2 --seeds 1..=12 --threads 6` | 172.5 ms/turn | 63.5 ms/turn |
| `--map medium --civs 8 --city-states 4 --seeds 20..=27 --threads 6` | 494.4 ms/turn | 144.0 ms/turn |

Session saves at turns 30 to 120 of two games are byte-identical to the base's.

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

### Compact saves (format 5)

The same example, and a late game with `--turns 150 --map medium --civs 6 --city-states 4` (every
seat an AI). Best of three runs on a loaded machine; the worlds are identical in both formats.

| Measurement | Format 4 (JSON) | Format 5 |
|---|---:|---:|
| Turn 12, save size | 344,637 bytes | 34,103 bytes |
| Turn 12, session save | 1.70 ms | 0.18 ms |
| Turn 12, session restore | 12.8 ms | 7.5 ms |
| Turn 150, save size | 2,339,638 bytes | 242,014 bytes |
| Turn 150, session save | 10.5 ms | 1.34 ms |
| Turn 150, session restore | 24.4 ms | 10.4 ms |
| Turn 150, browser checkpoint (`HostedGame.snapshot`, Wasm in Bun) | 10.0 ms | 1.9 ms |
| Turn 150, browser resume (`HostedGame.restore`, Wasm in Bun) | 24.9 ms | 11.1 ms |

Fog memories were 79% of the turn-150 save: 1,858,491 bytes as JSON entries, 170,914 as postcard
entries, 53,841 as columns. Without them the rest of the world is still 476,182 bytes as JSON, twice the
whole binary save, and postcard is 3–6× faster at both ends (the whole world: 0.6 ms to encode and
1.7 ms to decode, against 4.0 and 5.2 ms for JSON); that decided binary over a compact JSON. Restore
time is now mostly loading the rules and rebuilding each seat's view. The largest part left is the
civs' unique indexes (87 KB at turn 150, much of it repeated between civs). The browser row is a
similar game played through `HostedGame` with a human seat that only ends its turns (a
2,154,514-byte save before, 239,660 after), built with the same settings.

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

The separately ignored `complete_matches_save_and_replay` gate failed on both the baseline and phase 2:
seed 1, turn 150, `diplomacy references an invalid civilization pair`, from self-influence `(5, 5)`.
City-states earned great person points, and a city-state's Magnate traded on its own land. Both are
fixed at the source; the gate passes.

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
- One engine path. The all-bot matches of the spectator landing page, the cinematic page and art review (`?turns`)
  are watched sessions (`HostedGame.watch`, `engine/watch.ts`), and the observer `Game`, `worker.ts` and
  `transport.ts` are gone. Their views and turn reports are identical to the old path's, turn for turn, to the
  end of a match (a bun script against both engine builds). A turn costs within 3% of what it did (the delta is
  the extra): 58 ms over the landing page's first 80 turns, 306 ms before and 316 ms after over a whole match.
  The landing page's main-thread cost per update was never the transport: its fogged view is small (68 KB at turn
  35, 0.2 ms to clone; the delta is 42 KB) and the worker's message handler with what follows it took a median
  27 ms before and 29 ms after (long-animation-frame entries 32–61 ms before, 24–59 after). That is the world
  redrawing.
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

## Rules compiled at load

Unique parameters used to be read from text on every question: numbers parsed, `[+2 Food]` split,
filters like `{Military} {Water}` or `non-[Land]` broken up and each term looked up in every registry
it might name — for every building, tile and unit, each time the AI priced a construction.

- `civ_core::params`: each unique carries its parameters compiled — typed numbers, yields and stats,
  and a `Filter` of `Term`s classified against each matcher's vocabulary (`UnitWord`, `BuildingWord`,
  `CityWord`, `TileWord`) with their names resolved to ids. A ruleset compiles its own uniques at load;
  a unique read from a save or built at runtime compiles on first use. Clones share the result. Each
  compile remembers the ruleset's `Stamp`, so a unique asked about another ruleset compiles afresh.
- The matchers (`filters::matches_*`, `conditionals::matches_tile*`, `resources::matches_resource`) take
  a compiled `Filter`; the `&str` forms compile on the spot, so there is one reading of the language.
  `conditionals::evaluate` and the unique call sites on the yield, resource, combat, movement, purchase
  and placement paths read compiled parameters.
- Tech column eras and each unit's type are resolved to ids in `Ruleset::derive`.
- Contract: the ruleset is immutable once loaded. An entry added to a registry afterwards is not seen
  by uniques already compiled; edit a unique's parameters with `Unique::set_param`.

`crates/civ-sim/tests/suite/compiled_filters.rs` keeps the old string matchers as an oracle and
compares them with the compiled ones over every parameter string in base and blitz (about 400) against
every building, unit type, resource, unit, city and tile of an AI game at turn 70: about 3 million
answers per ruleset, all equal. It also checks every typed accessor against the parse it replaced.

Measured against main's balance runner (`5d5f340`, built at `opt-level = 3` without LTO), with this
branch built the same way and run concurrently on the same loaded machine (engine time only; ruleset
load gains about 1–2 ms for the compile):

| Run | Before | After |
|---|---:|---:|
| `--map medium --civs 8 --city-states 4 --seeds 21..=21 --threads 1` | 65.3 ms/turn | 55.6 ms/turn |
| `--map medium --civs 8 --city-states 4 --seeds 20..=27 --threads 3` | 89.8 ms/turn | 79.2 ms/turn |
| `--map small --civs 4 --city-states 2 --seeds 1..=12 --threads 3` | 30.4 ms/turn | 26.8 ms/turn |
| `--ruleset blitz --map small --civs 4 --city-states 2 --seeds 1..=12 --threads 3` | 9.83 ms/turn | 9.49 ms/turn |

The small game of seed 3 retires 15% fewer instructions (88.3 G to 74.9 G, `/usr/bin/time -l`).
Build settings move this as much: the same main at the repository's `opt-level = "s"`, without LTO,
retires 24% more. Every game is identical to main's with `engineMs` removed, including the small-map
reference set, and Session saves are byte-identical at turns 30–120.
