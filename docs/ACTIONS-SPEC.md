# Actions spec (M6)

Status: A1-A6 decided by Javed on 2026-10-13; M6a and M6b built and approved (How M6a/M6b is built, at the end). Javed also decided that civ-sim's `attacked` event carries the defender's owner.

| | Choice | Pick |
|---|---|---|
| A1 | How decisions reach the player | End turn leads: it names the next decision and opens its panel; a notification log lists events |
| A2 | Great person progress | civ-sim adds the own civ's points per kind, the shared thresholds and points per turn to the view |
| A3 | Solo saves | Autosave at every turn end, one slot per match; Continue and saved matches on Home |
| A4 | Browser saves | IndexedDB |
| A5 | Late game (air, nuclear, spaceship, spies, vote) | In M6 |
| A6 | Milestone size | M6a then M6b, a debrief after each |

M6a: every decision through End turn (captures, free technology, free great person, free policy, promotions,
state faith, demands, the vote, wonder sites), notifications, the top bar, and the unit, city and diplomacy
actions outside great people, religion and the late game: promote, upgrade, gift a unit, the production queue,
conversion, bombard, capture status and its actions, peace treaties with terms, demands, city-state tribute,
purchase and quests, and gift gold by amount. M6b: great people (with A2's view field), religion, saves (A3, A4),
and the late game (air missions, nuclear strikes, spaceship parts, spies and coups, the vote's screen, victory).

The options not picked stay below for the record.

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

## Choices (decided; see the table at the top)

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

## How M6a is built

- **Decisions** (`decisions.rs`):
  - `pending()` derives them from the view and the civ's options, in the order captures, free technology, free great person, free policy, demands, the vote, state faith, promotions.
  - Promotions come from a `unitOptions` query per own unit with experience, asked again only when a veteran's experience or promotions change.
  - End turn names the first decision not put off and opens it:
    - the tech tree, where any technology the free one may take is claimed with a click;
    - the policies screen, whose header then says how many are free;
    - the unit, for promotions;
    - a card for captures, great people, the state faith, demands and the vote.
  - Later puts a decision off until the next turn; Enter does what End turn does.
  - Wonder sites are not a decision, since the engine picks the best one. The city panel offers "Choose where it stands", and a click on a marked tile sends `chooseSite`.
- **Notifications** (`notes.rs`, `log.rs`):
  - Every one of the 66 event kinds is matched by type, the payload enums too, which civ-sim now re-exports. A kind the engine adds fails to compile.
  - Moves, fortifying, other nations' promotions and fights between others give no line, since the map shows them.
  - The log opens from the top bar, newest first, keeps 300 lines, and jumps to a line's place on click. Decisions and losses also show as toasts.
- **Top bar:** culture, faith (stored and per turn), happiness (red below zero). Undo is disabled when the update says so.
- **Unit panel:** experience and promotions held, Promote, Upgrade with its cost, gift to a city-state.
- **City panel:**
  - Status (puppet, being razed, in resistance), with Annex, Raze, Stop razing and Liberate as the view allows.
  - The queue, with ↑ and × per item and + beside each choice.
  - Turning production into gold or science.
  - Bombarding the units `cityOptions` lists: the first click on one aims, the second fires.
- **Diplomacy:**
  - The civilizations panel lists major civs only. Raiders and city-states had been listed among them since M4; fixed.
  - Peace treaty… opens the trade window as a treaty (`offerPeaceTreaty`).
  - Demand… offers the four demands.
  - City-states: tribute in gold or a worker, marriage where the view allows it, and their quests.
  - Gift gold in the amounts the treasury covers, not to civs you are at war with.
- **civ-sim:** re-exports `CaptureChoice`, `GreatPersonOutcome`, `NoticeKind` and `ReligiousEventKind`. Nothing on the wire changed.
- **Fixed on the way:** click-to-attack (6fada96).
- **Verified:**
  - 30 client tests, among them:
    - The decision order on a real view with decisions written into its JSON.
    - Aimed attacks the engine takes, and a promotion earned in them, taken by the engine.
    - Queue append, raise and remove kept as sent.
    - The notes module's six tests, one of which plays three seeds for 60 rounds and checks that every event reads cleanly; 17 of the 66 kinds came up.
  - civ-sim's 142 tests.
  - A 24 s recording (`CIV_SCENE=decide`) plays until the engine waits on a state faith, then shows:
    - End turn leading to it, and the card;
    - a three-item queue;
    - the log;
    - the civilizations panel.
- **Not verified:**
  - Captures, free technologies, great people and policies, demands and the vote in a live match. Only their derivation is tested.
  - Bombard and site clicks.
  - Upgrade, gift, tribute, marriage, treaty and demand against the engine.
  - Conversion.
  - Mouse paths by hand.
  - Decisions in a room.
- **Gaps:**
  - When the defending unit died, whether a fight was this player's is read from the engine's own line (it says "your"). The event does not carry the defender's owner; civ-sim could add it.
  - Purchases do not say what was bought, and a razed city cannot be named.
  - Gifts are still offered to a city-state you are at war with.

## How M6b is built

- **civ-sim** (additive, no protocol bump):
  - `knowledge.greatPeople` lists, for the own civ only, each great person kind it can earn: `{ unit, points, perTurn, threshold, pool?, combat }`. These are the civilian kinds fed by buildings and specialists, which share one pool in `base`, and the combat-born Marshal and Admiral. The Sage is left out: faith buys it.
  - `attacked` carries `defenderOwner` always, and `attackerOwner` when the viewer could see the attacker.
  - civ-core now exposes the three rules the view needs (`great_people::pool_key`, `combat_threshold`, `birth_speed`), so the view and the turn share one definition.
- **Saves** (A3, A4; `saves.rs`, `link.rs`):
  - The solo host saves when a round closes (`Down::Saved`); `Link::resume` restores and welcomes.
  - Each match keeps one slot: natively `<config>/saves/<id>.civsave` plus a `.json` with turn, year, nation and map; in the browser the IndexedDB `civ` database, stores `saves` and `meta`.
  - Home shows Continue for the newest, up to eight more with Open and Delete, and why a save or a match would not open (a match stopping before its first view returns home, or to the rooms for a room, with the reason). `CIV_SAVE=<file>` opens a save natively.
  - New games from Home get their own seed. The default seed had made every new game the same map.
- **Command ids:** the Outbox's ids now carry a prefix of its own (clock and count). A resumed save or a room rejoined after a reload holds receipts for an earlier client's `c1`, `c2`… and refused new orders as reused; this affected rooms since M5.
- **Great people:**
  - The unit panel lists every ability with what it would bring (science, production, gold) or why not, and sends `greatPerson`.
  - The empire panel shows progress per kind: points of threshold, per turn, and turns to go.
- **Religion:**
  - Missionaries and Sages spread their faith to the cities `unitOptions` lists, and a Sage can build a holy site (`religious`).
  - Cities show their faiths: followers, majority, holy city.
  - Faith prices sit beside gold prices (`purchase` with faith).
  - The empire panel shows the state faith, faith stored and per turn, and the faiths that may be adopted.
- **Late game:**
  - Rebase, air sweep, paradrop and nuclear strike are aimed at the tiles `unitOptions.targets` lists. A strike's first click asks `unitOptions` with `nuclearTarget` and shows the blast and interception chance; the second click strikes.
  - Spaceship parts are added from the unit panel.
  - Spies are listed with rank, station, task and turns, and can be withdrawn or sent where the engine allows: own cities to guard, others' known cities except those of a nation promised not to be spied on, never by a dead spy.
  - City-states offer a coup with its chance.
  - The empire panel shows each victory's progress, spaceship parts, the world vote (the vote itself is a decision from M6a) and the score.
- **The log** reads whose fight it was from `defenderOwner`, not from the engine's sentence.
- **Verified:**
  - 1,153 workspace tests (civ-sim's new ones check great person progress against civ-core over 100 rounds, and battle owners over 169 fights) and 34 client tests, among them:
    - A save made at a turn's end resumes at that turn and plays on, and a save from elsewhere is refused with why.
    - A developed civ (seat 0 played by a bot for 140 rounds, then handed back) used a great person's improvement, sent a spy and adopted a state faith, each taken by the engine.
    - A second client for a room's seat gives orders that are not taken as reused.
  - A 25 s recording (`CIV_SCENE=empire` on a save of that developed civ) shows the empire panel, a unit with an upgrade, a puppet city's faiths and Annex, the log, and a turn ended. Its autosave then appears on Home as "Continue: Mali · turn 142" (`CIV_SCENE=home`). Both wasm builds check.
- **Not verified:**
  - Saves in a browser (IndexedDB) at run time.
  - Faith purchases, missionaries spreading, holy sites, air missions, nuclear strikes, spaceship parts and coups against the engine: none came up in 174 turns of the developed civ.
  - Delete and Open from Home by hand.
- **Gaps:**
  - Great people are drawn as civilians on the map. `pieces::Class` knows them by a "Great " name prefix that `base` does not use.
  - The empire panel offers up to 24 spy stations, unordered by distance.

