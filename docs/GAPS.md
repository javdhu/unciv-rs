# Gaps: what stands between this build and a finished game

First written 2026-09-23 from four read-only code audits (UI coverage, gameplay and AI, game feel, UX) and a
playtest to turn 33 on a laptop and a 375×812 phone. Checked again against the code on 2026-09-25, at the start of
phase 3 of the UI rebuild (UI-REBUILD.md), after the old HUD was replaced by the glass interface.
[PLAYABLE.md](PLAYABLE.md) says what a player can do today.

Tags: **[blocks]** a full game or a new player cannot get past it · **[friction]** noticeable every
session · **[polish]** noticed by some players. Status: **done**, **part** (what is left is said), or **open**.

## The short version

- **Every engine action has a control.** `ui/test/coverage.test.ts` checks the interface against all 67 actions;
  only the pantheon and religious beliefs have none, because the shipped rulesets raise faiths by era and never
  offer them.
- **A human can reach every victory.** Peace and offers are answered in the leader's call, spaceship parts are
  added from the fan, the world vote is a choice on the dial, and the Empire sheet shows the race.
- **The others' turns are watched.** Each round's moves are played back on the map before the next turn opens.
- **A new player has help.** The guided turn hands over one decision at a time with a suggested move, seven coach
  tips explain the first moments, and the civilopedia (H) holds every rule.
- **What is left** is mostly feel and reach: moments for big events, performance tiers, a phone pass, lenses and a
  minimap, offline play, leader media, and World Congress resolutions.

## G. Gameplay a human cannot reach

| ID | Gap | Tag | Status |
|---|---|---|---|
| G1 | **Diplomacy.** A leader screen with war, peace, friendship, denouncing, demands, tribute, gifts, a trade table and an offer inbox. | blocks | **done**: the leader's call (`ui/sheets/Leader.svelte`, `LeaderSay`, `Trade`), a rival's demand as a decision |
| G2 | **Victory paths.** Spaceship assembly, the World Congress vote and a victory-progress panel. | blocks | **done**: the part is added from the unit's fan, the vote is a choice on the dial, the Empire sheet's Victory tab |
| G3 | **Religion.** Pantheon, founding and enhancing, and missionary, inquisitor and prophet actions. | friction | **done** for the shipped rules: faiths arise by era, the state faith is adopted from the dial or the Faith tab, religious units act from their fan, buildings are bought with faith. The pantheon and beliefs are never offered |
| G4 | **City-states.** Gift, pledge, buy and coup; influence, quests and ally. | friction | **done**: the envoy mode, the city-state sheet and the Empire's City-states tab |
| G5 | **Great people and spies.** Great-person actions and the spy panel. | friction | **done**: abilities on the unit's fan, the Empire's Spies tab |
| G6 | **Air and nukes.** Rebase, air sweep, paradrop and nuclear strike. | polish | **done**: one aim for all four, with the engine's preview of a blast |
| G7 | **Automation.** Worker automation and auto-explore for humans. | friction | **done**: the engine's `automateUnit` and `exploreUnit`, Stop ends them |
| G8 | **Coverage test.** A test that fails when the engine gains an action with no UI. | polish | **done**: `ui/test/coverage.test.ts` and `ui/coverage-pending.ts` |

## A. Rules and AI

The rules are close to complete. Unique-string parse coverage is gated above 99.5%, and all victories are
implemented.

| ID | Gap | Tag | Status |
|---|---|---|---|
| A1 | **The AI never approaches a human.** | friction | **done**: each AI puts at most one offer a turn in a human's inbox (`civ-core` `world/ai_offers.rs`) |
| A2 | **Every AI leader plays alike.** | friction | **done**: leaders play by their personalities (`world/personality.rs`) |
| A3 | **The AI's tactics are thin.** | polish | **part**: the army fights on the live map, each attack scored with the combat preview, cities bombarding and hurt units falling back to heal (`world/tactics.rs`); each AI plays to a victory goal (`world/goals.rs`) and weighs whether it can win a wonder (`world/ai.rs`). The goals are a first pass, and few games end in conquest ([AI.md](AI.md)) |
| A4 | **World Congress is only the diplomatic vote.** No resolutions: embargoes, world religion, ideology and the like. | polish | **open** |
| A5 | **Game length ignores speed.** | friction | **done**: without a turn limit the game lasts its speed's length (160 turns Quick, 240 Standard, 360 Epic) |

## X. UX: learning, information, flow

| ID | Gap | Tag | Status |
|---|---|---|---|
| X1 | **No tutorial or onboarding.** | blocks | **part**: the guided turn with a suggested move on every decision, and seven coach tips that retire once learned. No scripted first game |
| X2 | **No rules lookup.** | blocks | **done**: the civilopedia, every rule as an entry with its numbers, links and one fact, searchable, by H |
| X3 | **No empire overview.** | blocks | **done**: the Empire sheet (cities and units, city-states, spies, faith, victory) and the rivals' scores on their discs. No demographics |
| X4 | **The end screen is a dead end.** | blocks | **done**: the score chart, the standings, the moments, the replay on the map, Rematch, New game and Share |
| X5 | **New-game choices are thin.** | friction | **part**: map size, rivals, city-states, wrap, speed or Blitz with its clock and slices, the turn limit, victories, seed, nation and leader. No difficulty |
| X6 | **One save slot.** | friction | **done**: every game autosaves, and the saved games can be carried on or deleted. They are named by nation and turn, not by the player |
| X7 | **No threat awareness.** | friction | **part**: dashed red rings with a pointer on units that can strike next turn, the hub's Defend and Strike, a coach tip, and the yields lens. No other lenses, minimap or map search |
| X8 | **Safety and speed.** No undo, one-tap raze, no keyboard shortcuts. | friction | **done**: undo where the engine allows it (Ctrl+Z), raze in red and stoppable in the city, the keyboard of BRAND.md |
| X9 | **Settings.** | friction | **part**: music and effects, the Blitz clock, the yields lens, tips again, and motion follows the device. No UI scale or animation speed |
| X10 | **Platform promises.** No service worker, localisation or ARIA. | polish | **part**: controls have accessible names, and notices (the toasts), the coach, the fan's plate and the attack preview's verdict are announced; the turn's news (the rim's pips, the Chronicle) is not. The ARIA and keyboard pass is phase 3. No service worker or localisation |
| X11 | **Phone details.** | polish | **part**: bottom sheets that stop above the fan, a dial in the corner on a phone held sideways, 50px keys. The phone pass is phase 3 |
| X12 | **Multiplayer has no UI.** | blocks* | **done**: names, rooms, invite links, the lobby, your rooms, seats on the hub, a bot in an absent seat |

\* blocks acceptance test 2 only.

## F. Game feel

| ID | Gap | Tag | Status |
|---|---|---|---|
| F1 | **You can't watch the AI play.** | blocks | **done**: `state/replay.ts` plays each round's moves back, fights acted out, in a few seconds |
| F2 | **The camera never reacts.** | friction | **part**: it glides to each decision and, in a playback, to what happened to the player. No shake, no wonder or victory moment |
| F3 | **Combat has no numbers.** | friction | **done**: floating damage numbers (`world/floats.ts`), a flash on a hit, deaths that shrink away |
| F4 | **Growth is silent.** | friction | **part**: houses fill lots as a city grows and its label's rule fills, and the chronicle tells it. No founding or growth moment |
| F5 | **Milestones share one cue.** | friction | **open**: moments for discoveries, wonders, first contact and a new age are phase 3 |
| F6 | **No path preview.** | friction | **done**: a tap plans the route with each turn's stop, a second tap walks it |
| F7 | **Readability.** Units and the capital hidden in forest. | friction | **done**: plants clear round a unit, and counters fly above units close in |
| F8 | **A still world.** | polish | **part**: trees sway in the wind, herds graze, cloud shadows drift; seasons and a day and night each behind a switch in Settings, off by default |
| F9 | **Thin transitions.** | polish | **part**: sheets and fans move as they open and close, the rim turns once a turn. Loading is still a line of text |

## P. Product vision (PLATFORM.md) vs the build

| ID | Gap | Tag | Status |
|---|---|---|---|
| P1 | **No Blitz pacing.** | blocks | **done** in the rules: `crates/civ-core/rules/blitz` plays one of three 60-turn slices of two eras ([BLITZ.md](BLITZ.md)), chosen in the custom game |
| P2 | **Room play has server support and no client.** | blocks* | **done**: see X12. Reconnect and bot takeover show on the hub |
| P3 | **Leader encounters have no player.** | friction | **open**: the call shows the rival's disc; portraits and video wait for the leader media pipeline (PLATFORM.md §7) |

## Decisions

The decisions that shaped the plan, as they were taken.

1. **2026-09-23: both at once.** One track specs and tunes Blitz (P1). The UI track builds what both
   games need, starting with diplomacy (G1).
2. **2026-09-23: the Blitz shape.** About 60 minutes, start to victory. The full tech tree stays, with
   cheap techs, so air and nukes (G6) are in scope for Blitz too. Victories: domination, score at the
   turn limit, a short culture victory, and a new **wonder race** (first to N wonders). That last one is
   a new engine victory type. **Science is also allowed**, as a short race with fewer, cheaper parts.
   Blitz became a slice of two eras rather than a compressed tree (BLITZ.md).
3. **2026-09-23: the UI track's order.** G1 diplomacy, then G2 victory paths with X4 (end screen and play
   again).
4. **2026-09-23: AI turns (F1) are a fast, capped replay.** Only what the player can see, with every
   civ moving at once, capped near 3 s. The camera cuts to battles involving the player.
5. **2026-09-23: diplomacy (G1) uses a placeholder portrait,** with the video slot left open, so leader clips drop
   in later. The glass interface shows the rival's disc there.
6. **2026-09-23: learning.** X1 is a guided first game: prompts through the first 15 turns of a normal
   game, then first-time hints. X2 is tap-anything encyclopedia plates; a full index screen comes later.
7. **2026-09-23: AI.** The Blitz track takes A1 (the AI approaches the human), A2 (personalities) and A3
   (tactics).
8. **2026-09-23: the art track's next feel work.** F7 readability, F3 combat numbers, F4 growth moments,
   F5 milestone moments.
9. **2026-09-23: rooms (X12) come after victory paths.**
10. **2026-09-23: friction fixes join the UI track now:** X7 (threat alerts, lenses, minimap), X3
    (empire overview), X8 (undo, confirm raze, keys) and G7 (worker automation and explore).
11. **2026-09-23: leader media lives in Cloudflare R2.** It is kept out of git (`.gitignore`); the repo
    holds manifests only. This is a devops task.
12. **2026-09-23: World Congress resolutions (A4) are for the full game only,** after P4.
13. **2026-09-24: the UI rebuild.** The HUD is rebuilt as the glass interface of BRAND.md, in phases
    (UI-REBUILD.md), and by the end of phase 2 every engine action a shipped ruleset offers has a control.

## What remains

In rough order: the acceptance tests of PLAYABLE.md played through; phase 3's moments (F2, F4, F5), performance
tiers, ARIA and keyboard with the turn's news announced (X10) and phone pass (X11); a scripted first game (X1);
difficulty levels (X5); more lenses and a minimap (X7); a service worker and localisation (X10); World Congress
resolutions (A4); leader media (P3).
