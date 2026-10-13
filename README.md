# unciv-rs

The Rust port of Unciv's rules engine and the match server built on it, restored from civ.game
(`the private civ.game repository` main `20416e0`, 2026-09-30, the build deployed to Fly as `civ-game`). The web client,
site, art, marketing and client design docs were removed on 2026-10-13; they stay in git history before
that commit. Private: the engine is a close port of Unciv's MPL-2.0 code.

- `crates/civ-core`: rules engine, map generation and AI. Rulesets in `crates/civ-core/rules/`: `base` and
  `blitz` (civ.game's own content, [docs/CONTENT.md](docs/CONTENT.md), [docs/BLITZ.md](docs/BLITZ.md)), and
  Unciv's `vanilla` and `gods-and-kings`.
- `crates/civ-sim`: the session every client plays through: commands, queries, receipts, fog-filtered
  player views, deltas, saves and replay. Wire types in `civ_sim::protocol` (protocol 9).
- `crates/civ-server`: the private-room host: guest cookies, rooms, authenticated WebSocket seats, SQLite,
  turn clocks, reconnect and bot takeover. API in [its README](crates/civ-server/README.md).
- `crates/civ-client`: the Bevy 0.20 client, native and wasm, in progress: design in
  [docs/CLIENT-BRIEF.md](docs/CLIENT-BRIEF.md), look in [docs/VISUAL-SPEC.md](docs/VISUAL-SPEC.md).

The client contract is [docs/CLIENT-PROTOCOL.md](docs/CLIENT-PROTOCOL.md); engine layout and persistence are
in [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) (its client sections describe the removed web client).

## Run it

```sh
cargo run --release -p civ-server        # 127.0.0.1:3001, civ-server.sqlite3 in the working directory
cargo test --workspace --release --locked
```

The complete-match persistence gate plays three seeded bot matches, restores every 25 turns and checks that
replaying the commands gives the same game:

```sh
cargo test --release -p civ-sim complete_matches_save_and_replay -- --ignored --nocapture
```

The determinism check is `crates/civ-core/tools/check_determinism.sh` (its Linux legs need Docker; on the
Mac, Colima). `cargo run -p civ-sim --example export_protocol <dir>` writes the protocol's JSON schemas and
TypeScript types.

## The client

Plain `cargo build` and `cargo test` cover the engine and server only; the client builds with `-p civ-client`.

```sh
cargo client                                   # native dev build, Bevy linked as a dylib (alias in .cargo/config.toml)
CIV_SETTINGS='{"seed":3,"width":64,"height":40}' cargo client
cargo test -p civ-client --features bevy/dynamic_linking
cargo run -p civ-client --profile client       # optimised native build, for measuring
PROFILE=web-dev crates/civ-client/web/build.sh webgpu   # quick wasm build of one flavour
crates/civ-client/web/build.sh                 # both measured wasm builds into crates/civ-client/web/pkg
python3 crates/civ-client/web/serve.py         # http://127.0.0.1:5310, cross-origin isolated
```

Review screenshots: `CIV_SHOTS=<dir> CIV_SCENE=<world|fog|seed2|seed3> cargo client` plays a watched match to a
set turn, shoots fixed views into `<dir>` and quits; on the page, `?shots=<scene>` posts them to `serve.py`, which
writes `crates/civ-client/web/shots/`. Mouse: wheel or pinch zooms toward the cursor, drag pans, a click pins a
tile in the inspector; WASD or arrows pan; F3 shows frame and round timings. `CIV_SCENE=watch` records 45 s of
play from turn 30 as numbered PNGs at a fixed 30 frames a second (`ffmpeg -framerate 30 -i watch-%05d.png ...`).
Playback keys: space pauses, 1 to 4 pick 1x to 8x, full stop plays one round, K skips animations, F follows.

Dev builds optimise every dependency and the engine (`civ-core`, `civ-sim`) once, so the first build takes about
six minutes on this Mac and later edits to the client rebuild in one to two seconds.

The wasm builds run the engine on a second thread over shared memory, which needs the pinned nightly
(`rustup toolchain install nightly-2026-10-08 --profile minimal --component rust-src --target
wasm32-unknown-unknown`) and `wasm-bindgen-cli` at the version in `Cargo.lock`. The page must be served with
`Cross-Origin-Opener-Policy: same-origin` and `Cross-Origin-Embedder-Policy: require-corp`; `serve.py` does that
and appends what the page reports to `web/stats.jsonl`.
