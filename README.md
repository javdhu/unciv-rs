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
