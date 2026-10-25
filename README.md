# unciv-rs

An unofficial Rust port of the rules engine of [Unciv](https://github.com/yairm210/Unciv), the open-source
Civilization V remake, with a private-room match server and a Bevy client built on it. Not affiliated with the
Unciv project, Firaxis or 2K.

The engine is deterministic: rules arithmetic is fixed-point, and a fingerprint of a fixed scenario must be
bit-identical on native and `wasm32`, so a server and a browser client cannot disagree about the rules. Its
answers are checked value by value against Unciv's own (`crates/civ-core/tests/suite/differential.rs`, 1,811
values, 0 differences against the committed Unciv dump).

## Status

| Part | State |
| --- | --- |
| `crates/civ-core` | Rules engine, map generation and AI. Complete for the rulesets below. |
| `crates/civ-sim` | Session layer: commands, queries, receipts, fog-filtered player views, deltas, saves and replay. Wire types in `civ_sim::protocol` (protocol 9). |
| `crates/civ-server` | Private-room host: guest cookies, rooms, authenticated WebSocket seats, SQLite, turn clocks, reconnect and bot takeover. API in [its README](crates/civ-server/README.md). |
| `crates/civ-client` | Bevy 0.20 client, native and wasm. In progress. |

Design notes live in [docs/](docs/README.md). Several were written for an earlier web client that is not part of
this repository; the docs index says which.

## Rulesets

`crates/civ-core/rules/` holds the project's own rulesets, `base` and `blitz`
([docs/CONTENT.md](docs/CONTENT.md), [docs/BLITZ.md](docs/BLITZ.md)). These are what the server and client play.

The test suite and the determinism fingerprint also use Unciv's `Civ V - Gods & Kings` and `Civ V - Vanilla`
rulesets as fixtures. Their names and texts derive from Civilization V, so this repository does not include them;
`crates/civ-core/tools/fetch_unciv_rulesets.sh` downloads them from Unciv at a pinned commit and verifies each
file. The library and server build without them; the tests need them.

## Build and test

Requires stable Rust (the client's wasm builds need the nightly noted below), `git` and `curl`.

```sh
crates/civ-core/tools/fetch_unciv_rulesets.sh             # once: Unciv fixture rulesets for the tests
cargo build --release --locked                            # engine, session layer and server
cargo test --workspace --release --locked                 # all crates, including the client
cargo run --release -p civ-server                         # 127.0.0.1:3001, civ-server.sqlite3 in the working directory
```

Plain `cargo build` and `cargo test` cover the engine and server only (the workspace's default members); the
client builds with `-p civ-client` or `--workspace`. On Linux the client needs `libasound2-dev libudev-dev
libwayland-dev libxkbcommon-dev`.

The complete-match persistence gate (ignored by default; slow) plays three seeded bot matches, restores every 25
turns and checks that replaying the commands gives the same game:

```sh
cargo test --release -p civ-sim complete_matches_save_and_replay -- --ignored --nocapture
```

The determinism check is `crates/civ-core/tools/check_determinism.sh`. It compares the fingerprint on native and
`wasm32` (needs the `wasm32-unknown-unknown` target and [Bun](https://bun.sh)) and, with Docker available, in
Linux containers. `cargo run -p civ-sim --example export_protocol <dir>` writes the protocol's JSON schemas and
TypeScript types.

The generators in `crates/civ-core/tools/generate_*.py` read an Unciv checkout named by `UNCIV_DIR`.

## The client

```sh
cargo client                                   # native dev build, Bevy linked as a dylib (alias in .cargo/config.toml)
CIV_SETTINGS='{"seed":3,"width":64,"height":40}' cargo client
cargo test -p civ-client --features bevy/dynamic_linking
cargo run -p civ-client --profile client       # optimised native build, for measuring
PROFILE=web-dev crates/civ-client/web/build.sh webgpu   # quick wasm build of one flavour
crates/civ-client/web/build.sh                 # both measured wasm builds into crates/civ-client/web/pkg
python3 crates/civ-client/web/serve.py         # http://127.0.0.1:5310, cross-origin isolated
```

Keys (the key map in the client, H or ?, lists them all): WASD or arrows pan, Enter leads the turn, Esc closes the
top panel, N and Shift N cycle units that need orders, F fortify, Space wait, B found a city, T P C E U L open the
tech tree, policies, civilizations, empire, units and log, G shows the hex grid, Ctrl or Cmd Z undoes.

The client opens on home: play against bots, watch bots, or rooms. `CIV_SOLO=1` and `CIV_WATCH=1` (`civSolo`,
`civWatch` on the page) go straight into a match.

Rooms ([docs/ROOMS-SPEC.md](docs/ROOMS-SPEC.md)): civ-server serves the browser build beside its API when
`CIV_STATIC_DIR` names it.

```sh
crates/civ-client/web/build.sh webgpu
CIV_STATIC_DIR=crates/civ-client/web cargo run --release -p civ-server   # page and API on http://127.0.0.1:3001
```

The native client plays on `CIV_SERVER` (default `http://127.0.0.1:3001`) as a guest whose token it keeps in its
config directory (`CIV_CONFIG_DIR`), per server and `CIV_PROFILE`: two clients on one machine need two profiles.
`CIV_INVITE=<code>` (`?invite=` on the page) opens on the rooms and joins; `CIV_MATCH=<id>` (`?match=`) opens a
room's match.

Review screenshots: `CIV_SHOTS=<dir> CIV_SCENE=<world|fog|seed2|seed3> cargo client` plays a watched match to a
set turn, shoots fixed views into `<dir>` and quits. Mouse: wheel or pinch zooms toward the cursor, drag pans, a
click pins a tile in the inspector; F3 shows frame and round timings.

Dev builds optimise every dependency and the engine (`civ-core`, `civ-sim`) once, so the first build is slow and
later edits to the client rebuild in seconds.

The wasm builds run the engine on a second thread over shared memory, which needs the pinned nightly
(`rustup toolchain install nightly-2026-10-08 --profile minimal --component rust-src --target
wasm32-unknown-unknown`) and `wasm-bindgen-cli` at the version in `Cargo.lock`. The page must be served with
`Cross-Origin-Opener-Policy: same-origin` and `Cross-Origin-Embedder-Policy: require-corp`; `serve.py` does that.

## Licence

[Mozilla Public License 2.0](LICENSE), the same licence as Unciv, for the whole workspace. `crates/civ-core` is
a port of Unciv's rules and is a Modification of Unciv's MPL-2.0 code; [NOTICE](NOTICE) has the attribution and
the third-party files (the Inter font, under the SIL Open Font License).
