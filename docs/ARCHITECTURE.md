# civ.game architecture

The communication contract is [CLIENT-PROTOCOL.md](CLIENT-PROTOCOL.md). The engine, the shared session and the
private-room server run locally, and the Svelte/three.js client plays both solo games (`openSolo`) and room games
(`openRemote`) through them. [PLAYABLE.md](PLAYABLE.md) says what a player can do today and what is left.

## Ownership

| Module | Owns |
|---|---|
| `civ-core` | World, rules, legal actions, AI, knowledge and round processing |
| `civ-sim::session` | Authenticated-seat interface, readiness, retries, views, snapshots and replay |
| `civ-sim::protocol` | Wire DTOs and generated TypeScript/JSON Schema |
| Solo Worker | One session, serial input, IndexedDB persistence before publication |
| `civ-server` | Guest/seat identity, private rooms, ordered match input, SQLite, clocks and reconnect |
| `SessionClient` | Validated messages, pending outbox, view revisions, query correlation and subscriptions |
| `state/session.ts` | The client's one match: its stores and the actions that send commands |
| `ui/` | The glass interface: what the player sees and presses, read from the session's stores |
| `world/` | The 3D map: the latest view, the selection, the camera, animation and rendering |

Commands carry intentions and generational entity IDs. The host supplies the actor; clients cannot
choose another civilization's authority. `FinishTurn` locks one seat. The host closes the shared round
when all human seats are ready or the deadline expires. Bots use the same command legality; human
seats receive automatic economy/refresh without AI decisions.

The session prepares a replacement state without mutating the live state. Its host commits the snapshot
and receipt, replaces the live session, then publishes filtered updates. Duplicate command IDs recover
the original receipt before current-turn eligibility is checked. Reused IDs with altered content reject.

## Source map

```
crates/civ-sim/src/
  content.rs       the embedded rules (base, and blitz layered over it), identity and engine version
  setup.rs         bounded settings, seeded map and initial civilizations
  protocol.rs      versioned commands, queries, receipts, snapshots and deltas
  delta.rs         upserts/removals between already seat-filtered views
  session.rs       staged transitions, access, receipt history, save/restore and replay
  offers.rs        immutable human consent, private offer projection and expiry
  queries.rs       advisory options from owned state and public rules
  city_states.rs   what a major is told about a city-state
  hosted.rs        Wasm staging/commit adapter for the solo Worker
  view.rs          persistent player knowledge and typed presentation projection
  rules.rs         public static content tables
  lib.rs           exports and the original observer Game API
crates/civ-server/src/
  host.rs          per-match executor, connection generations, deadlines and controllers
  store.rs         SQLite identity, rooms, authoritative snapshots and receipt transactions
  http.rs          guest/room HTTP API, authenticated WebSocket and Origin checks
web/src/
  main.ts          mounts App; in dev builds puts the session module on window.civ
  App.svelte       the world and the interface, and the wiring between them
  engine/          the session client and its transports (below)
  state/           the match and the pure game logic the interface reads
  ui/              the glass interface
  world/           the 3D world
  art/, content/   procedural models and the recipes that bind ruleset names to them (docs/PROCEDURAL.md)
  audio/           procedural music and sound (docs/SOUND.md)
  brand/           glyphs, nation liveries and the world's palette
web/src/engine/
  generated/       Rust-generated types, runtime schemas and the bundled rule tables; do not hand-edit
  validation.ts    incoming protocol validation and integer/cursor checks
  session-client.ts shared submit/query/subscribe interface and durable retry recovery
  storage.ts       IndexedDB transactions, outbox and solo compare-and-swap
  session-worker.ts solo host; persist staged state before acknowledge/publish
  session-transports.ts openSolo (the Worker) and openRemote (a room's WebSocket, remote-session.ts)
  rules.ts         the bundled rule tables, for the title and the civilopedia before a game opens
  protocol.ts      the renderer's view contract, a subset of the wire view
  worker.ts, transport.ts   the original observer adapter, used only by art-review.ts
```

The engine is the workspace crate `crates/civ-core`. Its default build has no runtime dependencies;
optional `serde`/`save` features support authoritative persistence.

## Information boundary

Tile, city and civilization observations persist at the time of sight. Fogged entities use those
observations instead of today's hidden state. Resources require the observer's technology. Reachability
uses a world reconstructed from player knowledge and marks uncertain routes. Execution can therefore
stop after a legal prefix when movement reveals a blocker.

Views expose own economy and visible units. A whitelist projects typed presentation events; it never
falls back to engine Debug text. Foreign movement/battles without event-time attribution are omitted
from `events`. The update that closes a round carries `replay` instead: the others' turns as per-civ batches,
each event kept only if its tiles were in the viewer's sight when it happened (CLIENT-PROTOCOL.md), which the
client plays back before the new view. Tests compare views and previews across hidden-state
changes. These protections do not establish complete rule parity or a finished combat presentation API.

Only updates replace client game state. Receipts settle pending input and name a view cursor. The
client buffers its current view for late subscriptions and ignores old revisions. Welcome and resync carry
full views; later messages carry entity upserts/removals against an exact base cursor. The client retains
unchanged entity objects and requests a full resync if it missed the base revision.

## Persistence and networking

Native saves include complete arena generations/free-slot order, world/knowledge, versions and content
identity. Session saves add settings, epoch, turn access, receipts and ordered player/host inputs. Replay
reconstructs the game from its initial settings and those inputs; deterministic AI decisions remain
reconstructable under the pinned engine. Presentation events are not the authoritative save format. Core and session format 4 embed the world
as a JSON object, omit derived seat projections, and reject older snapshots. Typed projections are rebuilt
on restore. Staged worlds share their large stores through copy-on-write ownership; mutating a store
detaches it without changing the live transaction or undo snapshot.

Visibility caches line-of-sight results by their complete local terrain and topology inputs. Visibility
masks and player projections are still assembled for each transition; this is not a mutation journal.

Remote guests hold an opaque HttpOnly cookie; only its hash is stored. Invite codes cannot reclaim a
seat. Reconnect issues a new connection generation and fences the previous socket. Disconnect grace,
controller changes and round deadlines are persisted. Commands and clock decisions use one executor.
The local server owns its SQLite database exclusively; multi-process distributed hosting is deferred.

Browser pending requests are saved before sending. Reopening a session retries them in order with the
original IDs. Solo uses the same semantics, with an IndexedDB comparison against the previous snapshot
so a stale second tab cannot overwrite progress. Network failure means an unknown outcome; a receipt
must be recovered before dependent commands proceed. Remote sessions reconnect with bounded backoff,
hydrate a fresh view, and recover the outbox before queued commands resume. Fenced tabs stop permanently.

Every solo game autosaves under its own id, and `ui/model/saves.ts` keeps an index beside the saves so the
title can list them, carry one on and delete one.

## The client

### The session

`state/session.ts` is the one owner of the match. It holds the stores everything else reads (the rules, the
latest update and its view, the selected unit, the open city, the previewed attack, civ options, the agenda of
decisions, the board's marks, the chronicle, the sheet open) and the actions that send commands through `act()`
(or `actReceipt()`, which returns the engine's receipt) and queries through `ask()`. A rejection becomes a short
notice. The interface never talks to the engine itself.

Beside it, modules built on its actions and exported stores carry what later features needed, so the session
stays small: `abilities` (great people, religious units, spaceship parts), `aim` (rebase, air sweep, paradrop,
nuclear strike), `envoy` (the city-state looked at), `demands`, `undo`, `replay` (the others' turns played back),
`rooms` and `pulse` (rooms, their lobby and a socket's health), `placement` (a wonder's site) and `clock` (the Blitz
clock). The pure game logic beside them is `board` (who needs orders, what threatens what), `decisions` (the guided
turn, most urgent first, each unit with a suggested move), `orders` and `paths` (go-to routes and their turns),
`cityplan` (suggested builds with reasons), `yields`, `diplomacy`, `history` and `coach`.

Every engine action is reachable from the interface or listed in `ui/coverage-pending.ts`, and
`ui/test/coverage.test.ts` fails when one is in neither place. Today only the pantheon and beliefs are listed, as
unreachable: the shipped rulesets raise faiths by era.

In dev builds `window.civ` is the session module and `window.__civ` a handful of its actions and stores, for the
console and for the smoke run (`web/tools/smoke`).

### The interface

`web/src/ui/` is the glass interface of [BRAND.md](BRAND.md). [UI-REBUILD.md](UI-REBUILD.md) is its plan, its
contracts and its decisions.

```
ui/
  tokens.css, tokens.ts   colours by meaning, the glass, type, sizes and motion; scoped to .ui
  motion.ts               the spring as a CSS linear() easing, and reduced motion
  fonts.ts                Geist and Geist Mono, bundled
  keys.ts                 the keyboard: Space/Enter the hub, 1–6 the fan, Esc back, R P E C L H, Shift+Enter, Ctrl+Z
  kit/                    every part: Glass, Btn, Chip, Num, Dot, Bar, Row, Seg, Scale, Sheet, Key, Icon, Disc,
                          Pop, Field, Toast and toasts, Tip and tip.ts, and the sheet stack (layers.ts)
  dial/                   the dial, its fan and plate, and their pure geometry
  model/                  pure view models: the mode contract (types.ts), the mode stack (dial.ts), the rim,
                          the top chips, the city, research, policies, the empire, the call, the end, the rooms…
  modes/                  one file per dial mode, each a pure function of a ModeInput snapshot
  play/                   what is on screen while playing: Yields, Studies, Rivals, Tools, Coach, Combat,
                          Chronicle, Fatal
  sheets/                 City, Research, Policies, Empire (with City-states, Faith, Spies, Victory), Leader and
                          Trade, CityState, the civilopedia (Pedia)
  front/                  the title, custom game and nation pickers, saved games, rooms and their lobby,
                          settings, and the end with its score chart and replay scrubber
  lib/                    pure helpers: the fan keys' one-word captions, and those kept from the old interface
                          (years, combat verdicts, icons and unit glyphs, events, specialists, leaders and slices)
  gallery/                every part in every state (web/gallery.html), one section per lane
  Game.svelte             the layout: the top chips, the dial and its fan, one sheet or panel at a time
  coverage-pending.ts     the actions no part sends, and why
  test/                   action coverage, no colour literals, tokens in step, the keyboard
```

The dial is a stack of modes. `ui/model/dial.ts` takes a snapshot of the session's stores and asks each mode
in `ui/modes/index.ts` in turn, and shows the first that answers: the rooms sheet over the title, the title,
the stopped session, a room's seat out of the player's hands or waiting on the others, the others moving, a
capture, a wonder's site, an aim, an attack, a unit, a city, a city-state, a choice owed (research, a free
technology or great person, the state faith, a world vote), and at rest the turn's next decision or End turn.
A mode is a hub (what pressing the centre does), up to five keys plus More, a plate that names the subject, and
lamps for what waits. The rim, its news pips and the date come from `ui/model/rim.ts` whatever the mode. Presses
call the session's actions.

Colour literals are not allowed anywhere in `ui/**` outside the tokens (`ui/test/literals.test.ts`); nation
colours are data from `brand/livery.ts` and the ruleset.

### The world

`world/scene.ts` owns the renderer, the camera, the render loop and picking. App feeds it the view
(`show`), the selection, the planned route, the board's marks, yields, a wonder's sites, an attack or a nuclear
blast, and past borders for the end's replay, and moves its camera (`panTo`, `glideTo`, `frameMap`). It
reports taps (`onTile`, `onCity`), camera moves (`onCamera`, which the rim's pips follow) and the soundscape
under the camera, and `project(tile)` puts a tile on screen. It never talks to the engine.

The land is one continuous surface (`land`) painted per pixel (`ground`), with one sea (`water`); forests,
resources, herds, cities, towns and wonder plots have their own owners. Static geometry
is instanced, and so are units: every unit of one shape is a slot in one instanced mesh, posed in the vertex
shader (`world/troupe.ts`), walking routes found through the world's streets, gates, fords and woods
(`world/way.ts`). An `attacked` event names both sides, and `world/combat.ts` acts fights out from it. Fog is a
linen haze (`world/veil.ts`), light a warm tabletop grade (`world/light.ts`, with an optional day and night in
`world/daynight.ts`), and outlines a thin ink pass
(`world/ink.ts`); [DESIGN-SYSTEM.md](DESIGN-SYSTEM.md) has the look. The map's marks (counters, city labels,
route stops, yields, floating numbers) follow the interface's grammar in `world/world.css`. Quality follows the
device and steps down when frames run slow (`world/perf.ts`).

### Sound

`audio/audio.ts` is the one import the rest of the game uses. `audio/bind.ts` listens to the session's and the
interface's stores and plays the cues; [SOUND.md](SOUND.md) has the triggers.

## Local validation

See the root README for build and run commands. `tools/check_protocol.py` detects generated-schema
drift. Native tests cover human round access, hidden information, staged writes, idempotent purchases,
snapshot continuation and replay. Browser engine tests cover the pending outbox, update ordering,
runtime validation, IndexedDB compare-and-swap and actual Wasm human commands. Server tests exercise
real HTTP/WebSocket clients plus failed storage, restart, deadlines and connection fencing. The client's
view models and modes are unit-tested without a DOM, and the smoke run plays a fixed game in a headless browser
at desktop and phone sizes.
