# Actions spec (M6)

Status: draft, 2026-10-13. Choices A1-A6 are open; nothing here is built.

## Problem

A played match still leaves the player unable to do much of what the engine offers, and some of it the engine
waits on. The client sends 35 of the protocol's 67 actions, and its unit panel reaches 9 of the 17 unit action
kinds. Nobody answers a captured city, a free technology or great person, a promotion, a demand or a vote,
and the engine falls back to its defaults (a captured city stays a puppet). Events other than moves, battles
and foundings are never shown. Great people and religion are invisible, and a solo match cannot be saved.

M6 is the rest of the action catalogue, great people and religion (decided with M4), and solo saves.

## What the engine offers

Read from civ-sim on 2026-10-13.

**Decisions the engine asks for**, with the signal the client can read:

| Decision | Signal | Answer |
|---|---|---|
| Captured city | `PlayerView.pendingCaptures[].choices` | `keepPuppet`, `annex`, `raze`, `liberateCity` |
| Free technology | `rewards.technologies`, `CivOptions.freeTechnologies` | `claimFreeTechnology` |
| Free great person | `rewards.greatPeople`, `CivOptions.freeGreatPeople` | `claimFreeGreatPerson` |
| Free policy | `rewards.policies` | `adoptPolicy` (works today; nothing says it is free) |
| Promotion | `canPromote` event, `UnitOptions.promotions` | `promote` |
| State faith | `CivOptions.stateFaith { current, adoptable, lockedTurns }` | `adoptStateFaith` |
| A demand made of you | `PlayerView.demands` | `answerDemand` |
| Diplomatic vote | `endgame.voting` | `voteDiplomatic` |
| Wonder site | `ConstructionChoice.sites` | `chooseSite` |

**Actions the client does not send yet:**
- Unit:
  - `upgrade` (`UnitOptions.upgrade` gives the cost).
  - `giftUnit` to a city-state.
  - `greatPerson`.
  - `religious` (spread religion, holy site).
  - `rebase`, `airSweep`, `paradrop`, `nuclearStrike` (targets in `UnitOptions.targets`, with a nuclear preview).
  - `assembleSpaceshipPart`.
- City:
  - `bombard` (`CityOptions.bombard`).
  - `setProductionConversion`.
  - A production queue longer than one item.
  - Faith purchases (`PriceQuote` in `ConstructionChoice.faith`).
  - `stopRazing`.
- Diplomacy:
  - `offerPeaceTreaty` with terms.
  - `demand`.
  - `demandTribute`, `buyCityState` and `attemptCoup` (city-state influence).
  - `assignSpy` and `withdrawSpy`.
- Gift gold is fixed at 50 today.

**Information never shown:**
- Event text, as a notification log: technologies learned, wars declared, great people born, faiths arising, cities razed, and so on.
- Unit experience and promotions.
- City status: puppet, razing, resistance, religion.
- Culture, faith and happiness in the top bar.
- Victory progress (`endgame`).
- Whether Undo is available (`GameUpdate.undo`).

**Great people.** Births are automatic at turn end; the only choice is a free great person.
- One command carries every ability: `greatPerson { unit, action }`. The actions are hurry research, hurry production, trade mission, golden age, and building the unit's own improvement on its tile.
- `UnitOptions.greatPersonAbilities` lists every ability with whether it is available, why not, and what it would bring.
- The view carries no great-person points or progress, only the count of free ones pending. Showing progress needs a civ-sim change: A2.

**Religion in `base`.** Faiths arise on their own by era; no player founds one.
- Pantheons, founding and enhancing are refused in `base`, and no unit can remove heresy.
- The player's choices are:
  - Adopting a state faith. It must be the majority faith in one of their cities, and it is locked for 10 turns after a change.
  - Missionaries and Sages spreading their faith to a city.
  - A Sage's holy site.
  - Faith purchases.
- The view carries faith, each own city's majority religion, followers and pressure, every civ's state faith, and the rule tables' faiths and traits.

**Saves.** `Session::save()` writes a snapshot plus a command log; `Session::restore(&[u8])` takes it back.
- Size and speed on the default 32x20 match at turn 100:
  - 88 KB.
  - 0.6 ms to save.
  - 13.5 ms to restore.
- `docs/PERFORMANCE.md` gives 242 KB at turn 150 on a medium map.
- Each applied command adds about 500 bytes of history (the engine's own sizing comment, `session.rs:21`).
- A save restores only on a build with the same engine version string and the same rules content hash. There is no migration.
- Watched matches are not saved. The undo stack is not saved.
- The Bevy solo host never calls `save` or `restore` today.

## Fixed during the survey

Click-to-attack never armed. The client read attack targets from `UnitOptions.targets`, which lists only air and nuclear missions. It now aims at units and cities of civs at war with it, and raiders: adjacent for melee, within range for ranged (the engine attacks from where the unit stands). A test plays a solo session and checks that the engine takes the aimed attacks (6fada96).

## Non-goals

- Founding, enhancing and pantheons (not in `base`).
- Other rulesets.
- Advice or automation beyond the engine's own.
- Room changes.
- Saves for rooms (the server keeps those) or for watched matches.
- A tutorial.

## Architecture

```
GameUpdate ──▶ ViewState ──┬─▶ Decisions (derived each update: captures, rewards, demands, votes, faith,
                           │              promotions, sites) ──▶ A1: how they are put to the player
                           ├─▶ Notifications (event text, with a tile to look at)
                           └─▶ panels: unit (promote, upgrade, great person, religious, air, gift)
                                       city (queue, conversion, bombard, faith, sites, capture status)
                                       civ  (state faith, free tech and great person, demands,
                                             city-states, spies, peace treaty, victory)
Solo host thread: Up::Save → bytes ──▶ store (A3, A4) ──▶ Up::Load(bytes) → restore, welcome
```

## Components and contracts

1. **Decisions.** Derived from the view and the options answers each update, never stored. Each decision is `{ kind, subject, label, opens }`, in the engine's order: captures, then rewards, demands, votes, state faith, promotions, then sites. How they reach the player, and whether End turn waits for them, is A1.
2. **Notifications.** `GameUpdate.events` as one-line text in plain words. Each line has a tile or subject to jump to. The log is kept for the last few turns.
3. **Unit panel.** Adds:
   - Promotions (each with its effect).
   - Upgrade with its cost.
   - Gift to a city-state.
   - Great person abilities, with why not and what each would bring.
   - Religious actions aimed at a city.
   - Air missions and nuclear strikes aimed at the tiles `targets` lists, with the nuclear preview.
   - Spaceship parts.
   - Experience and promotions held.
4. **City panel.** Adds:
   - A production queue with add, remove and reorder.
   - Conversion to gold or science.
   - Faith purchases.
   - Bombard, aimed like an attack.
   - Wonder sites (`chooseSite`, a click on one of the offered tiles).
   - Puppet, razing and resistance status with annex, raze, stop razing and liberate.
   - Religion: majority, followers, pressure.
5. **Civ screens.**
   - State faith choice.
   - Free technology from the tech tree.
   - Free great person chooser.
   - A mark on policies that are free.
   - Demands made of you, and your own.
   - City-state actions: tribute, buy, coup, quests.
   - Spies.
   - Peace treaties with terms, through the trade window.
   - Gift gold with an amount.
   - The diplomatic vote.
   - A victory screen from `endgame`.
6. **Top bar.** Culture, faith and happiness beside gold and science. Undo is disabled when `GameUpdate.undo` says so.
7. **Great people.** Abilities through the unit panel, births as notifications, and progress per A2.
8. **Saves.**
   - `Up::Save` asks the solo host for `Session::save()` bytes; `Up::Load(bytes)` restores and welcomes as a new connection.
   - A save from another build is refused by the engine. The client says so and offers nothing else.
   - Where saves live, and when they are written: A3, A4.

## Data flow and state ownership

The engine owns every decision and its default; the client derives what to show from each update and asks the
options queries it already asks. The client owns only the notification log, which notifications are read,
and the saves it writes.

## Risks and unknowns

- **Saves break with any build that changes the rules data or the engine version.** The engine refuses them and there is no migration, so a save is good for one build. The engine's design owns this; the client can only say so.
- **History grows.** About 500 bytes per applied command; a long game's history can outgrow its world. The engine caps at 100,000 records and 80 MB.
- **Late-game features are hard to reach in tests.** Air, nuclear, spaceship, spies and votes need matches that get there. Saves made by seeded bot matches can start those tests.
- **Promotions cost queries.** Knowing which units can promote takes either the `canPromote` event or one `unitOptions` query per unit.
- **The attack aim is a guess the engine checks.** A unit that cannot attack this turn still shows targets, and the engine refuses with a toast.

## How it will be verified

- Headless tests per decision and action, on civ-sim sessions as the attack test does: play seeded matches to the state, act, check the receipt and the view. Seeded saves stand in for late-game states.
- A save round trip test: save, restore, equal view, commands keep working; the browser store checked in headless Chrome.
- A scripted recording that meets each decision and opens each new panel.

## Rollback

The client changes are additive. The A2 view field is optional in the protocol. Saves are files or browser entries the client can ignore.

## Open choices

A1. How decisions reach the player.
- (a) End turn becomes the next decision ("Choose a promotion", "Captured city: decide") and opens its panel, as Unciv's next-turn button does. A notification log lists events.
- (b) A notification column on the left with decisions pinned on top. End turn stays End turn, and unanswered decisions take the engine's defaults.
- (c) A prompt in the centre as each decision arrives.

A2. Great person progress.
- (a) civ-sim adds the own civ's points per kind, the shared thresholds and points per turn to the view (additive).
- (b) No progress shown; births come as notifications.

A3. Saves.
- (a) Autosave at every turn end, one slot per match, with Continue and a list of saved matches on Home.
- (b) Autosave plus named manual saves.
- (c) Manual saves only.

A4. Where the browser keeps saves.
- (a) IndexedDB.
- (b) The Origin Private File System.
- (c) localStorage: about 5 MB a site; a save is 90 KB at turn 100 on a small map and 250 KB at turn 150 on a medium one, a third more as text.

A5. Late-game features: air missions, nuclear strikes, spaceship parts, spies and coups, the diplomatic vote.
- (a) In M6.
- (b) A later milestone; M6 covers what a match meets before the industrial era.

A6. Milestone size.
- (a) One M6.
- (b) M6a: decisions, notifications, unit and city actions, top bar. M6b: great people, religion, saves, and the late game per A5.
