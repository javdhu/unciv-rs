# civ.game

A free, instant, browser-first strategy game in the manner of Civ V: a 3D miniature world generated entirely in
code, bots in every empty seat, and private rooms to play with friends from one link. The tagline is "A world
history in one evening".

- Platform design: [docs/PLATFORM.md](docs/PLATFORM.md)
- Code architecture: [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md)
- The interface (colour, marks, glass, the dial): [docs/BRAND.md](docs/BRAND.md)
- The world's look: [docs/DESIGN-SYSTEM.md](docs/DESIGN-SYSTEM.md)
- Client contract: [docs/CLIENT-PROTOCOL.md](docs/CLIENT-PROTOCOL.md)
- What is done and what is not: [docs/PLAYABLE.md](docs/PLAYABLE.md) and [docs/GAPS.md](docs/GAPS.md)
- Marketing and social: [docs/MARKETING.md](docs/MARKETING.md), with the kit in [marketing/](marketing/)
- Art tool research and proposal: [docs/STUDIO.md](docs/STUDIO.md)

## What is here

- `crates/civ-core`: the rules engine, map generation and AI. It plays the game's own rules, `base` (the whole
  game, [docs/CONTENT.md](docs/CONTENT.md)) and `blitz` layered over it ([docs/BLITZ.md](docs/BLITZ.md)), from
  `crates/civ-core/rules/`.
- `crates/civ-sim`: the session every client plays through (commands, queries, receipts, player views, saves and
  replay), its wire protocol with generated TypeScript types, and the Wasm build the browser runs.
- `crates/civ-server`: the host for private multiplayer rooms (guest seats, SQLite, WebSocket, turn clocks).
- `web/`: the client, Svelte 5 and three.js. The world (`web/src/world`) is drawn from procedural models
  (`web/src/art`, [docs/PROCEDURAL.md](docs/PROCEDURAL.md)); the glass interface (`web/src/ui`) sits over it; the
  music and sound are made in code (`web/src/audio`, [docs/SOUND.md](docs/SOUND.md)).

The client plays solo games in a Worker that saves each change to IndexedDB before it answers, and room games over
a WebSocket to civ-server. Both go through `web/src/state/session.ts`, the interface's one path to the match.

## Run it

Needs Rust with the `wasm32-unknown-unknown` target, `wasm-pack` and Bun 1.3 or newer (it installs, runs and tests everything; Node is not needed).

```sh
bun --cwd web install --frozen-lockfile
bun --cwd web run wasm              # civ-sim → web/src/engine/pkg, then the bundled rule tables
bun --cwd web run dev               # http://localhost:5199
```

For rooms, run the local multiplayer host in a second terminal:

```sh
cargo run -p civ-server
```

It listens on `127.0.0.1:3001`, and the dev server proxies `/api` and its WebSocket to it. Guest cookies own
seats, invite links find rooms, and SQLite stores every acknowledged step. Configuration and the API are in
[the server README](crates/civ-server/README.md). Deploying is in [docs/DEPLOY.md](docs/DEPLOY.md); nothing is
deployed by these commands.

## Scripts

From `web/`:

| script | does |
|---|---|
| `bun run dev` | the dev server on port 5199 |
| `bun run wasm` | builds civ-sim to `src/engine/pkg` with wasm-pack, then runs `rules` |
| `bun run rules` | writes the bundled rule tables (`src/engine/generated/rule-tables.json`) the title and the civilopedia read before a game, and the art's list of needs (`content/art/needs.json`) |
| `bun run check` | svelte-check over the client and its tools |
| `bun run test` | vitest: the engine adapters, the session, the view models and modes, the world, the art and the audio |
| `bun run build` | the production bundle in `web/dist`, with the tool pages and the non-UI session API at `engine/session.js` |
| `bun run smoke` | plays a fixed game in headless Chromium at desktop and phone sizes and shoots every scene ([web/tools/smoke/README.md](web/tools/smoke/README.md)) |
| `bun run art -- sheet [filter]` | a contact sheet of the procedural art entries, without a browser |
| `bun run dsp` | compiles the Faust instruments to `src/audio/dsp/gen` |

The dev server also serves the tool pages: `/gallery.html` (every part of the interface in every state),
`/art-review.html` (a fixed map study), `/wardrobe.html`, `/architecture.html`, `/contact.html?filter=unit/` (a
labelled contact sheet drawn by the game's renderer) and `/sound-lab.html`.

## Checks

```sh
cargo test --workspace --release
python3 tools/check_protocol.py                  # the generated wire types match civ-sim
cargo run -p civ-sim --example export_protocol   # regenerate them after a protocol change
bun --cwd web run check
bun --cwd web run test
bun --cwd web run build
bun --cwd web run smoke                       # then look at the shots in web/.smoke
```

CI (`.github/workflows/ci.yml`) runs the Rust tests, the determinism check, the protocol check, and the web
client's tests, type check and build.

The complete-match persistence gate plays three seeded bot matches, restores every 25 turns and checks that
replaying the commands gives the same game:

```sh
cargo test --release -p civ-sim complete_matches_save_and_replay -- --ignored --nocapture
```
