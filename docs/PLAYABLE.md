# Playable: what a player can do, and what is left

Status 2026-09-25, from the code at the start of phase 3 of the UI rebuild ([UI-REBUILD.md](UI-REBUILD.md)). The
whole game plays in the glass interface ([BRAND.md](BRAND.md)): every engine action a shipped ruleset offers has
a control. [GAPS.md](GAPS.md) keeps the audit of what stands between this build and a finished game.

## Goal

Someone opens civ.game, plays a full game from the 3D map against bots or friends, and reaches a victory or
defeat screen. Done when both acceptance tests pass:

1. **Solo.** A full small-map game against 3 bots, played to a victory screen through the interface, on a laptop
   and on a phone.
2. **Room.** Two browsers finish a private-room match from one link; one reloads mid-game and carries on.

Neither has been recorded end to end through the new interface yet. The smoke run (`bun --cwd web run smoke`)
plays a fixed game's first turns at desktop and phone sizes, plays a six-turn game out to its end screen, and,
against a local civ-server, plays a room from one link with two browsers, a dropped socket and a bot taking an
absent seat.

## Where things stand

- **Engine and session.** civ-sim protocol v4 carries 67 typed actions and 7 queries (reachable, unit, city and
  civ options, view, rules, combat preview), receipts, full player views, private offers, undo state, the others'
  moves for playback, saves and replay. The session embeds the game's own `base` rules and `blitz` over them.
  civ-server hosts private rooms. `SessionClient`, the IndexedDB outbox, the solo Worker and the WebSocket
  transport are built and tested.
- **The interface.** `web/src/ui/` over `web/src/world/`, with `web/src/state/session.ts` the one path to the
  match (docs/ARCHITECTURE.md). The old HUD (`web/src/hud/`) was deleted on 2026-09-25.
- **Coverage.** `ui/test/coverage.test.ts` checks every engine action against the interface. All are surfaced
  except the pantheon and religious beliefs, which the shipped rulesets never offer (faiths arise by era).

## What a player can do

**Start.** The title's dial is Play: a small map, three bots, two city-states and a random nation. Beside it:
Continue, Custom game (map size, rivals, city-states, east-west wrap, speed or Blitz with its clock and a slice of
eras, the turn limit, the victories, a seed, the nation and its leader), Play with friends, the saved games (carry
one on or delete it), the civilopedia and Settings. Every solo game autosaves.

**A turn.** The dial's hub names the next decision, most urgent first, and hands it over: a captured city, a unit,
a rival's demand, research, a free technology or great person, a world vote, the state faith, an idle city, a
policy (EXPERIENCE.md, section 3). Its lamps count what waits. End turn is the hub once nothing is left, and
Shift+Enter ends the turn anyway. With the Blitz clock on, a turn has 45 seconds and the game a 90-second bank. The
others' turns are played back on the map before the next turn opens, and a tap, Esc, Shift+Enter or the hub skips
them. Ctrl+Z takes the last order back when the engine allows it.

**Units.** The fan holds the selected unit's orders, the suggested one first: move (a tap plans a route with its
turns, a second tap or Go walks it, and a longer route becomes a go-to order walked each turn), attack with a
preview of both sides, found a city, build an improvement, promote, fortify, sleep, skip, pillage, set up, upgrade,
disband, gift, and the engine's auto-work and auto-explore. Great people use their abilities, religious units
spread their faith, and a spaceship part is added in the capital. Rebase, air sweep, paradrop and nuclear strikes
aim at a tile, the strike with the engine's preview of the blast. A foreign unit shows a plate.

**Cities.** The city sheet: the build queue with suggested builds and their reasons, buying with gold or faith,
avoid growth, wealth or research in place of a build, the governor's focus, pinning worked tiles (a tap on the map
or a row), specialists, buildings with selling, buying tiles, and bombarding a target in range. A wonder that
takes a tile is placed through the fan or on the map. A captured city is annexed, kept as a puppet, razed or
liberated from the fan; razing can be stopped.

**The empire.** The top chips show gold, science, culture, faith and happiness with their change a turn, and
the technology and policy under study. The Empire sheet lists cities and units, city-states, spies, the state
faith and victory progress with the spaceship and the world vote. The research sheet is the tech web with a
research goal; the policies sheet adopts policies. The chronicle (C, or the dial's rim) is the game's news; the
rim's pips turn the camera to where each happened. L turns on the yields lens and H opens the civilopedia.

**Other nations.** The rival discs, by score, open a leader's call: attitude, what is agreed, an offer or a demand
to answer, and war, peace, peace on terms, friendship, a defensive pact, a scholarly exchange (a research
agreement), trade (gold, gold a turn, luxuries, open borders, an embassy), demands and denouncing. A city-state
looked at puts an envoy on the dial: gifts of gold, tribute in gold or a worker, protection, marriage (buying it
out), and its sheet shows its influence, ally and quests. Spies are posted from the Empire sheet, where a spy in
a city-state can attempt a coup.

**Rooms.** A name, a room made with its settings, an invite link, the lobby's seats and nations, the rooms this
browser holds, and a start. In the match the hub says when the socket is being replaced or a bot is playing the
seat, and waits on the others once the turn is finished, with the room's deadline as the clock.

**The end.** The result, the score chart, the standings and the moments that shaped the game; the rim becomes a
scrubber that replays the borders on the map. Rematch, New game and Share.

**Settings.** Music and effects levels, the Blitz clock, the yields lens, the tips again, and the civilopedia.
Motion follows the device's reduced-motion setting.

## What is left

- **The acceptance tests** above, played through the new interface on a laptop and a phone.
- **Phase 3 polish** (UI-REBUILD.md): moments for discoveries, wonders, first contact and a new age; the glass
  fallback and blur off on low performance tiers; ARIA and keyboard, with the turn's news announced to a screen
  reader (today only notices, the coach, the fan's plate and the attack preview are); a phone pass.
- **Aircraft on the map.** An aircraft's tap on a base does not arm its rebase yet: `ui/model/aim.ts` `mapTap`
  does, but `App.svelte` sends map taps to `state/aim.ts` `aimTap`, which only picks while an aim is armed.
- **Leader media.** The call shows the rival's disc; portraits and video wait for the leader media pipeline
  (PLATFORM.md §7).
- **Not built:** difficulty levels, other map lenses than yields, a minimap, a service worker for offline play,
  localisation, accounts, public matchmaking and ranked play. The World Congress holds only the diplomatic vote.
  Go-to orders are the client's own, walked each turn by `state/session.ts`.

## Verification

Every change runs `bun --cwd web run check`, `bun --cwd web run test` and `bun --cwd web run build`, and
after an engine change `cargo test --workspace --release` and `python3 tools/check_protocol.py`. The view models
and modes are tested without a DOM against fixture views and the real Wasm engine; the smoke run's shots at
1280×800 and 390×844 are looked at against `docs/interface/`.
