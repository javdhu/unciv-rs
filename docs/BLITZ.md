# Blitz

Blitz is civ.game's one-evening mode. It plays a **slice of eras**, two eras of about 30 turns each (CONTENT.md,
Spine), not the whole tree compressed. There are three slices of 60 turns each, chosen at setup
(`Settings.slice`): **Cradles** (Bronze and Iron), **Crossroads** (Paper and Sail) and **Engines** (Steam and
Electricity). A slice starts in its first era with every tech of the eras before it and that era's namesake
tech, cannot research past its last era, and ends at turn 60, when The Chronicle decides if nobody has won. It is a ruleset layered over
the full game, not a fork of it.

## How it is built

`crates/civ-core/rules/blitz/` holds only what Blitz changes. The loader reads the full game's files
(`rules/base`) first and Blitz's after them (`content::load_ruleset("blitz")`). A Blitz object with the
same name replaces the base one whole, and a new name adds an object, the way an Unciv mod layers over its
base ruleset. Blitz's `ModOptions.json` removes objects by name with Unciv's `buildingsToRemove`,
`unitsToRemove` and `nationsToRemove`, plus `victoryTypesToRemove`.

A match picks its ruleset with the additive `ruleset` setting (`base` by default, or `blitz`). The
content hash covers the full game's files, the ruleset's name and its own files, so a Blitz match never
shares a hash with a full one. Saves carry that hash and the world's ruleset identity, so a Blitz save
never loads as the full game (tested both ways).

## The slices, as built (2026-09-24)

`rules/blitz/ModOptions.json` lists the slices (`slices`: name, eras, turns, and the calendar years for
Crossroads, 500 to 1750, and Engines, 1750 to 1945). The start comes from the first era's row in
`Eras.json`: Cradles begins with a settler, warrior, worker and scout; Crossroads with two settlers, a
worker, three spearmen, a shrine and monument in new cities and the Bronze wonders gone; Engines with three
settlers of three population, two workers, five musketeers and nine buildings in new cities.

| Rule | Blitz | Full game (Standard) | Why |
|---|---|---|---|
| Speed | `Blitz` | `Standard` | Standard's costs and knobs; each slice's `scienceCostPercent` (below) then fits its two eras into 60 turns. |
| Turn limit | 60 | 240 | Two eras of 30 turns. |
| Deals, peace treaties | 15 and 5 turns | 30 and 10 | Scaled to the slice's length. |
| Map | small (the default 32×20), 4 nations, 2 city-states | same defaults | The brief asks for small, 4 nations and 2 to 4 city-states. |
| Diplomatic victory | off (Perpetual Peace and the World Charter removed) | on | The charter needs the Internationalism tree of the Age of the Atom. |
| Victories | those that can be won within the slice's eras, then The Chronicle | all six | The engine filters them per slice, counting data gates too: Reserve Currency needs `tradeShareFromEra` (the Age of Sail), so Cradles leaves it out. |

Each slice entry may also set its own pace and victory reach. `scienceCostPercent` scales tech costs while
the slice plays (later eras' techs cost far more, so later slices need a bigger cut to fit two eras in 60
turns). Any of ModOptions' victory knob keys (`influencePercent`, `influenceFloor`, `worldTradePerMajor`,
`worldTradePerMajorPerEra`, `tradeShareFromEra`) on a slice entry replaces the ruleset's value in that slice
only. As built:

| Slice | `scienceCostPercent` | `influencePercent` | `influenceFloor` | `worldTradePerMajor` / `PerEra` |
|---|---|---|---|---|
| Cradles | 100 | 230 | 800 | 15 / 3 (ruleset's) |
| Crossroads | 31 | 300 | 1500 (ruleset's) | 4 / 0 |
| Engines | 19 | 180 | 1500 (ruleset's) | 4 / 0 |

Measured with the balance example (8 seeds, 1 to 8; 16 seeds in brackets), 4 nations, 60 turns:

| Slice | Second era first reached | Lingua Franca | Reserve Currency | The Chronicle | Earliest win |
|---|---|---|---|---|---|
| Cradles | Iron at turn 28 (29) | 4 of 8 (9 of 16) | not offered | 4 (7) | turn 53 (51) |
| Crossroads | Sail at 30 (29) | 2 (7) | 1 (1) | 5 (8) | turn 51 (51) |
| Engines | Electricity at 30 (28) | 4 (9) | 0 (0) | 4 (7) | turn 52 (52) |

Before these knobs (after the start began granting the namesake tech): Sail was first reached at turn 56 in half
the Crossroads games, Electricity at 53 in one Engines game in eight, and The Chronicle won every game of every
slice. Reserve Currency stays rare: the best trader often holds 60% of world trade (up to 79% in Crossroads and
100% in Engines) but seldom for the 15 turns `VictoryTypes.json` asks, which a slice cannot change.

## Tuning

Numbers come from the balance runner:

```sh
cargo run --release -p civ-sim --example balance -- --ruleset blitz --slice Crossroads --seeds 1..=8 --threads 4
```

`--overlay DIR` layers trial files over the bundled rules, so a change can be tried before it is
committed. The full game's pace lives in its content costs (CONTENT.md, As built); Blitz inherits it and scales tech costs per slice.
