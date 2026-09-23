# Playable plan: wire the UI to the engine and finish the game

Status 2026-09-23: P0, P1 and P2 are built; P2 waits for your playtest to a victory on a laptop. P2 covered the
ink-and-paper HUD with RhosGFX icons, every basic unit order, the combat preview and `attacked` events, the
tech web with research goals, the city sheet (queue, citizens, specialists, buildings, tiles, bombard), the
capture prompt, policies and free claims, banner growth bars, and the engine fix that puts human cities'
idle citizens to work. A scripted 100-turn soak game reached the end screen with no errors. Next: P3,
diplomacy and the rest of Gods & Kings. This is the UI track; the art track (procedural graphics) runs in
parallel under [PROCEDURAL.md](PROCEDURAL.md).

## Goal

Someone opens civ.game, plays a full Gods & Kings game from the 3D map against AIs or friends, and
reaches a victory or defeat screen.

Done when both acceptance tests pass:

1. **Solo.** A full small-map game against 3 AIs, played to a victory screen through the UI, on a laptop
   and on a phone.
2. **Room.** Two browsers finish a private-room match from one link; one reloads mid-game and carries on.

Out of scope here: art (art track), leader videos, original ruleset text, matchmaking, accounts, ranked
play, Blitz pacing and balance.

## Where things stand

- **Engine and session are done.** civ-sim protocol v4 carries 59 typed actions and 6 queries (reachable,
  unit, city and civ options, view, rules), receipts, full player views, private offers, save and replay.
  civ-server hosts private rooms. `SessionClient`, the IndexedDB outbox, the solo Worker and the WebSocket
  transport are built and tested.
- **Checks are green.** `cargo check --workspace --tests` passes, svelte-check reports 0 errors, and all
  1,062 web tests pass.
- **The UI plays the real session (P1).** `state/session.ts` opens solo games through `openSolo`. The old
  observer adapter remains only for the art review page.
- **The renderer reads the wire view.** The generated `PlayerView` is a superset of the renderer's
  `engine/protocol.ts` types, so terrain, units and cities read it without logic changes.
- **The engine work is committed.** The 23k lines that civ.game builds against (`world/commands.rs`,
  `save.rs` and the rest) are Unciv-master `f8f72f5`.

## Working beside the art track

Neither track edits the other's files.

| Track | Owns |
|---|---|
| UI | `web/src/{state,hud,engine}/**`, `App.svelte`, `main.ts`, `app.css`, `world/overlays.ts`, new `world/{input,banners}.ts`, `crates/civ-sim`, `crates/civ-server`, this file |
| Art | `web/src/{art,content}/**`, `world/{scene,units,terrain,cities,ink,animator,combat,fx}.ts`, `web/tools`, the review pages, PROCEDURAL.md |

Three seams, agreed once:

1. **Types.** `engine/protocol.ts` stays as the renderer's view contract, a subset of the wire view. Art
   fixtures (`art/review-world.ts`) build it directly, and `state/session.ts` fails to compile if the
   generated `PlayerView` stops fitting it.
2. **Input and overlays.** `WorldScene` already has `onTile` and `setSelection`. P2 adds hover, path,
   target and yield overlays in `overlays.ts`, and `project(tile)` for HTML banners. Changes to
   `scene.ts` are additive hooks only, announced to the art track.
3. **Motion.** Each update's `events`, with the views before and after, go to one renderer entry point.
   The art track turns them into hops, lunges and topples. A real `attacked` event (civ-sim work below)
   replaces the guess `combat.ts` makes from health differences.

Git: both tracks share `~/Projects/civ.game` on `main` (the user's choice), so each commits only its own
files. The UI track's dev server is `civ-game-play` on port 5189; the art track's is `civ-game-web` on 5199.

## Phases

### P0. Safety (about 30 minutes)

Done 2026-09-23. The engine's 878 tests passed and its working tree was committed as Unciv-master
`f8f72f5`. The `civ-game-play` launch entry was added. The user chose the shared folder over a worktree.

### P1. Wire the real session (about 1 day)

Replace the observer with `openSolo`. No art files change.

1. The renderer keeps `engine/protocol.ts` (see the seams). The observer adapter (`engine/worker.ts`,
   `engine/transport.ts`, civ-sim's `Game`) stays while `art-review.ts` uses it; `state/game.ts` is gone.
2. A new `state/session.ts` owns one `GameSession` and stores the latest `GameUpdate`, `view`, `rules`
   and pending commands. `act(action)` fills `commandId`, `epoch` and `turnToken`, awaits the receipt,
   and shows a rejection as a short toast.
3. A title screen over the live 3D scene offers New game (map size, AIs, city-states, nation) and
   Continue (the IndexedDB save).
4. Tapping an own unit runs `reachable` and shows the reach overlay; tapping a reachable tile sends
   `moveUnit`. End turn sends `finishTurn` and shows "Other civilizations are moving" until the next
   update.

Done when: in the browser, found a city, set production and research, move units, end 10 turns, reload,
and continue on the same turn.

Done 2026-09-23 and verified in the browser: Persepolis founded, Warrior then Worker built, Mining then
Pottery researched, turn 14 reached (about 110 ms a turn), and reload plus Continue resumed turn 14 with
the same gold and research. Added: nation city names in `NationRow.cities`, and a static `ruleTables()`
Wasm export for the title screen.

### P2. The core loop (4–6 days)

Everything needed to play a whole game. Every control is built phone-first: bottom sheets, tap only, no
hover-only information. P2 opens with a one-screen HUD mock for the user to choose from, like the art
style checkpoint.

1. **Units.** A bottom bar built from `unitOptions`: fortify, skip, found city, build (the worker menu),
   pillage, promote, upgrade, disband, set up and gift. It cycles to the next unit and counts units
   needing orders. Sleep is client-side.
2. **Combat.** Tap an enemy to see the preview card, tap again to attack. Ranged and bombard targets are
   highlighted.
3. **Cities.** World-space banners show name, population, growth and production bars, and health. A city
   sheet built from `cityOptions` covers production with turns left, gold and faith purchase, worked
   tiles and specialists, tile buying, building sales, avoid growth and conversions. A capture prompt
   offers annex, puppet, raze or liberate.
4. **Empire.** A top bar shows stored and per-turn gold, science, culture and faith, plus happiness and
   golden age. Add the research picker, the tech web, the policy picker, and free tech and great person
   claims.
5. **Turn flow.** The end-turn button becomes the next blocking prompt: choose research, choose
   production, or unit needs orders. A notification feed comes from `events`; tapping one moves the
   camera there. The victory or defeat screen reads `view.result`.

Done when: a full small-map game against 3 AIs reaches a victory screen on a laptop.

### P3. Diplomacy and the rest of Gods & Kings (1–1.5 weeks)

Once P2's HUD kit exists the panels are independent, so they can run as parallel agent tasks.

1. **Diplomacy.** A leader screen, with a placeholder portrait until leader media exists. War, peace,
   friendship, denouncing, demands, tribute and gold gifts. A trade table and an offer inbox where
   offers are answered or withdrawn.
2. **City-states.** Gifts, pledges, purchase, quests and coups.
3. **Religion and spies.** Pantheon, founding and enhancing beliefs, and missionary, inquisitor and
   prophet actions. Assigning and withdrawing spies.
4. **Late game.** Great-person actions, air units (rebase, sweep, paradrop), nukes, spaceship parts,
   World Congress votes and a victory-progress panel.
5. **Coverage test.** A table maps every `Action` variant to its UI control. The test fails when the
   engine adds an action with no UI.

Done when: the coverage test passes, and scripted saves reach each victory type through the UI.

### P4. Multiplayer rooms (3–4 days)

1. Create a room and join by link through civ-server's HTTP API.
2. Add a lobby over the 3D scene with seats and nations; bots fill empty seats.
3. Show the readiness and deadline clock, other seats' readiness, connection status and reconnects, and
   the bot-takeover notice.

Done when: acceptance test 2 passes.

### P5. Phone pass (about 1 week)

1. Play the full solo game on a phone and fix what breaks.
2. Hold 30 fps on a mid-range phone and 60 fps on a 2020 laptop.
3. Add loading, error and empty states, and sound hooks.

Done when: acceptance test 1 passes on a phone.

Total: about 4–5 weeks of agent work.

## civ-sim additions (UI track, needed in P2)

Each stays knowledge-safe like the rest of the protocol and gets its hidden-state tests.

1. **Full rule tables.** Tech era, cost, prerequisites and unlocks. Unit strength, ranged strength, range,
   cost and domain. Building cost, maintenance, wonder flag and a one-line effect. Policy branch and
   prerequisites, promotion prerequisites, improvement yields and resource kind.
2. **`combatPreview { attacker, target }`.** Expected damage both ways and the modifier lines, computed
   from what the attacker can see.
3. **Tile yields.** Yields for a city's tiles in `cityOptions`, and a `tile { id }` inspect query for
   any known tile, computed for the viewer.
4. **Paths and growth.** A predecessor on each `Reach` entry so the client can draw the route, a `path`
   query for multi-turn move orders, and turns-to-grow in the city view.
5. **An `attacked` event.** For battles the viewer fought in, it carries the attacker, defender, damage
   and deaths.

## Engine needs (civ-core; the user allowed engine edits on 2026-09-23)

- Worker automation and auto-explore for human units. The bot routines exist, but human seats cannot use
  them.
- Event-time observation, so a viewer who saw a third party's battle can be shown it.

## Verification

Every phase runs `cargo test --workspace`, `npm --prefix web run check` and `npm --prefix web test`.
State-layer tests run against a fake `GameSession`. Each phase's done-when is played in the browser
preview and shown with a screenshot.
