# civ.game

A free, instant, browser-first Civ V-style 4X. A 3D miniature world generated entirely in code, with bots
in every empty seat and historical leader encounters.

- Platform design: [docs/PLATFORM.md](docs/PLATFORM.md)
- Code architecture: [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md)
- Accepted client contract: [docs/CLIENT-PROTOCOL.md](docs/CLIENT-PROTOCOL.md)
- Visual direction: [docs/DESIGN-SYSTEM.md](docs/DESIGN-SYSTEM.md)
- Procedural art plan and coverage: [docs/PROCEDURAL.md](docs/PROCEDURAL.md)

## Run it

Needs Rust with the `wasm32-unknown-unknown` target, `wasm-pack`, Node 20+, the engine checked out at
`~/Downloads/Unciv-master/civ-core-rs`.

```sh
cargo test --workspace                # facade and local multiplayer host
npm --prefix web install
npm --prefix web run wasm              # civ-sim → web/src/engine/pkg
npm --prefix web run dev               # http://localhost:5199
```

`npm --prefix web run check` type-checks the client; `npm --prefix web run build` makes a production bundle.

## State

The 3D observer renders the map, units and growing cities. Every visual is generated in code by
`web/src/art/`; imported-model work was archived on 2026-09-23 (see [the procedural plan](docs/PROCEDURAL.md)).
Terrain and resources have their own recipes; units, improvements, natural wonders and cities use
placeholders until their phases land. `npm --prefix web run art -- sheet` renders a contact sheet of the
art entries, and `/art-review.html` is a deterministic map study.
Human commands, persistent player knowledge, save/replay,
solo storage and private multiplayer rooms now exist below it; the UI has not yet been connected to
that interface. The engine targets the bundled Gods & Kings rules and normal matches. Unciv editor/mod-only behavior and public-platform features are outside this engine work.

Run the local multiplayer host in a second terminal:

```sh
cargo run -p civ-server
```

It listens on `127.0.0.1:3001`; Vite proxies `/api` and WebSocket requests to it. Guest cookies own seats,
invite codes locate rooms, and SQLite stores acknowledged transitions. Configuration and API details
are in [the server README](crates/civ-server/README.md). Nothing is deployed by these commands.

For new gameplay controls, use `openSolo` or `openRemote` from `web/src/engine/session-transports.ts`.
The production build also exports this non-UI API at `engine/session.js`, including its Worker and Wasm assets.
The old `transport.ts` remains only for the observer UI.

```sh
cargo run -p civ-sim --example export_protocol  # regenerate wire types and validation schemas
python3 tools/check_protocol.py                 # detect protocol drift
cargo test --workspace
npm --prefix web run wasm
npm --prefix web run check
npm --prefix web test
npm --prefix web run build
```

The complete-match persistence gate runs three seeded bot matches, restores every 25 turns, and verifies command replay:

```sh
cargo test --release -p civ-sim complete_matches_save_and_replay -- --ignored --nocapture
```
