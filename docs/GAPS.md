# Gaps: what stands between this build and a finished game

Status 2026-09-23. This lists what is missing; [PLAYABLE.md](PLAYABLE.md) holds the phase order. Sources:
four read-only code audits (UI coverage, gameplay and AI, game feel, UX), plus a playtest from turn 0 to
turn 33 on a laptop viewport and a 375×812 phone viewport.

Tags: **[blocks]** a full game or a new player cannot get past it · **[friction]** noticeable every
session · **[polish]** noticed by some players. Sizes are agent-days.

## The short version

- **The engine is well ahead of the UI.** 30 of the engine's 59 actions have a control. The missing 29
  are all diplomacy, city-states, religion, spies and the late game.
- **Presentation is ahead of controls.** 34 of 35 event types already reach the Chronicle and the
  renderer.
- **A human cannot finish every kind of game.** There is no way to make peace (`offerPeace`,
  `answerOffer`), assemble spaceship parts (science victory), or vote in the World Congress (diplomatic
  victory).
- **The turn is invisible.** The AI's whole turn arrives as one update, so enemy moves, fights and
  builds pop into place.
- **A new player gets no help.** There is no tutorial, no rules lookup, no empire overview, and no
  "play again" on the victory screen.

## G. Gameplay a human cannot reach

| ID | Gap | Tag | Size |
|---|---|---|---|
| G1 | **Diplomacy.** No leader screen, war or peace, friendship, denounce, demands, tribute, gifts, trade table or offer inbox. That is 14 actions. AI offers can never be answered. | blocks | 3–4 |
| G2 | **Victory paths.** No spaceship assembly (`assembleSpaceshipPart`), no World Congress vote (`voteDiplomatic`), no victory-progress panel. `view.endgame` is never read. | blocks | 2 |
| G3 | **Religion.** No pantheon, founding or enhancing, and no missionary, inquisitor or prophet actions (3 actions). The AI uses religion; the human can't. | friction | 2 |
| G4 | **City-states.** No gift, pledge, buy or coup, and influence, quests and ally are never shown (`view.cityStates` unread). | friction | 1.5 |
| G5 | **Great people and spies.** Only free claims are wired. Great-person actions (`greatPerson`) and the spy panel (`assignSpy`, `withdrawSpy`, `attemptCoup`) are missing. | friction | 1.5 |
| G6 | **Air and nukes.** `rebase`, `airSweep`, `paradrop` and `nuclearStrike` are missing. | polish | 1 |
| G7 | **Automation.** No worker automation or auto-explore for humans; only bots can use them (engine change). Every worker is micromanaged for 300+ turns. | friction | 1 |
| G8 | **Coverage test.** No test fails when the engine gains an action with no UI (PLAYABLE P3.5). | polish | 0.25 |

## A. Rules and AI

The rules are close to complete. Unique-string parse coverage is gated above 99.5%, `tools/unparsed.json`
is empty, and all five victories are implemented. The gaps are mostly in how the AI treats a human.

| ID | Gap | Tag | Size |
|---|---|---|---|
| A1 | **The AI never approaches a human.** `choose_trades` only asks other AIs (`crates/civ-core/src/world/ai.rs:770`), and no AI path sends friendship. Every deal must start with the player, so diplomacy feels one-sided even once G1 exists. | friction | 1.5 |
| A2 | **Every AI leader plays alike.** `Personalities.json` drives city-states but has no references in `ai.rs`. Montezuma and Gandhi use the same war, peace and wonder thresholds. | friction | 1.5 |
| A3 | **The AI's tactics are thin.** It attacks the weakest reachable target, marches on the three nearest cities, never retreats to heal (`ai.rs:1442`), ignores wonder races (`ai.rs:401`), and takes the first valid belief (`religious_units.rs:241`). War and peace thresholds are faithful Unciv ports. | polish | 2 |
| A4 | **World Congress is only the UN vote.** It has no resolutions: embargoes, world religion, ideology and the like (`world/victory_actions.rs`). | polish | 2 |
| A5 | **Game length ignores speed.** `max_turns` is 500 for every speed (`crates/civ-sim/src/setup.rs:40`) although the ruleset's `Speed.total_turns` is parsed. | friction | 0.25 |

## X. UX: learning, information, flow

| ID | Gap | Tag | Size |
|---|---|---|---|
| X1 | **No tutorial or onboarding.** The playtest's first prompt is "Choose research" while the settler sits in a forest. Nothing says "found your city first", and there are no city-site hints. | blocks | 2–3 |
| X2 | **No rules lookup.** No civilopedia for units, buildings, techs, policies or terrain. City-sheet rows name a building but never say what it does. Tech unlock icons are unlabelled unless long-pressed. | blocks | 2 |
| X3 | **No empire overview.** No cities list, units list, score, demographics or victory progress. | blocks | 1.5 |
| X4 | **The end screen is a dead end.** Victory or defeat shows the winner, with no play again, new game, stats or replay (`hud/Hud.svelte:41`). | blocks | 0.5 |
| X5 | **New-game choices are thin.** Only map size, rivals, city-states and nation. There is no difficulty, speed, victory types or turn limit; `maxTurns: 500` is hardcoded (`state/session.ts:65`). | friction | 0.5 |
| X6 | **One save slot.** No save list, no delete, no naming (`SAVE_KEY`, `state/session.ts:95`). | friction | 1 |
| X7 | **No threat awareness.** At turn 33 barbarians stood beside the capital with no alert. There are no map lenses (yields, resources, religion), no minimap and no search. | friction | 2 |
| X8 | **Safety and speed.** No undo. Raze is one tap with no confirm (`CapturePrompt.svelte:37`). No desktop keyboard shortcuts. | friction | 1 |
| X9 | **Settings.** Only audio. No animation speed, in-app reduced motion, UI scale or colour-blind palette. | friction | 1 |
| X10 | **Platform promises.** There is a manifest but no service worker, so the game is not actually offline-capable. Localisation isn't ready, and ARIA and live regions are sparse. | polish | 1.5 |
| X11 | **Phone details.** The city sheet covers the city it describes. The buy-price column clips at the right edge. There is one breakpoint and no landscape layout. | polish | 1 |
| X12 | **Multiplayer has no UI.** Transport exists (`engine/session-transports.ts`) but there is no room creation, invite link or lobby (PLAYABLE P4). | blocks* | 3–4 |

\* blocks acceptance test 2 only.

## F. Game feel

What works: unit hops with dust, attack lunges, projectiles and hit flashes, selection and hover rings,
fortify marks, the positional soundscape, fog clouds that part, and zoom that retilts the camera.

| ID | Gap | Tag | Size |
|---|---|---|---|
| F1 | **You can't watch the AI play.** `finishTurn` returns one final update (`engine/session-client.ts:71`). Enemy moves, battles and new cities pop in. Needs per-civ event batches from civ-sim and a replay queue in the renderer. | blocks | 2–3 |
| F2 | **The camera never reacts.** No follow on your own moves, no cut to a battle or an attacked city, no shake, no wonder or victory moment. | friction | 1.5 |
| F3 | **Combat has no numbers.** No floating damage, no hit-stop, and deaths puff away at once. | friction | 1 |
| F4 | **Growth is silent.** City founding, growth, border expansion, improvements, roads and completed buildings are instant repaints (`world/cities.ts:41`, `world/terrain.ts:499`). | friction | 2 |
| F5 | **Milestones share one cue.** Golden age, era change, great person, war declared and promotion get a generic sound and a Chronicle line. There is no banner, animation or distinct cue. | friction | 1.5 |
| F6 | **No path preview.** Tapping a reachable tile moves at once; there is no route line and no multi-turn move. | friction | 1 |
| F7 | **Readability.** Starting units and the capital are hidden inside dense forest at play zoom (playtest turns 0 and 33). | friction | 1 |
| F8 | **A still world.** The sun is fixed, water is static, and there are no people or animals moving when nothing happens. | polish | 2 |
| F9 | **Thin transitions.** Loading is plain text, sheets fade in and vanish on close, and there is no start-of-turn summary. | polish | 1 |

## P. Product vision (PLATFORM.md) vs the build

PLATFORM.md promises "a world history in one evening": a colonist.io of Civ V. The build plays a
standard-length G&K game, which runs 6–20 hours.

| ID | Gap | Tag | Size |
|---|---|---|---|
| P1 | **No Blitz pacing.** Only the `gods-and-kings` and `vanilla` rules ship. The planned `rulesets/blitz/` (PLATFORM.md §157, §404) doesn't exist, and PLAYABLE.md scopes balance out. This is the product's core promise. | blocks | 5+ |
| P2 | **Room play has server support and no client.** civ-server has seat readiness and a deadline clock (`crates/civ-server/src/host.rs:105`), but no UI shows either (see X12). Reconnect and bot takeover (PLATFORM M2) are future work. | blocks* | with X12 |
| P3 | **Leader encounters have no player.** content/leaders defines photoreal phone-call portraits, but no overlay plays them, and the ruleset still names the Unciv leaders (Ramesses, Gandhi, Darius). | friction | 2 |

## How we plan each

We go group by group: G, then F, then X, then A and P. For each gap we pick one of four answers: **now**
(it joins the next phase), **later** (dated), **cut**, or **redesign** (the Civ V answer doesn't fit
a phone-first evening game). Decisions are recorded here beside each row.
