# Blitz

Blitz is civ.game's one-evening mode. It plays a **slice of eras at the full game's pace** (CONTENT.md,
Spine), not the whole tree compressed. There are three slices of 60 turns each, chosen at setup
(`Settings.slice`): **Cradles** (Bronze and Iron), **Crossroads** (Paper and Sail) and **Engines** (Steam and
Electricity). A slice starts in its first era with every tech of the eras before it, cannot research past
its last era, and ends at turn 60, when The Chronicle decides if nobody has won. It is a ruleset layered over
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
| Speed | `Blitz` | `Standard` | Standard's costs and knobs, so an era takes the same 30 turns. |
| Turn limit | 60 | 240 | Two eras of 30 turns. |
| Deals, peace treaties | 15 and 5 turns | 30 and 10 | Scaled to the slice's length. |
| Map | small (the default 32×20), 4 nations, 2 city-states | same defaults | The brief asks for small, 4 nations and 2 to 4 city-states. |
| Diplomatic victory | off (Perpetual Peace and the World Charter removed) | on | The charter needs the Internationalism tree of the Age of the Atom. |
| Victories | those that can be won within the slice's eras, then The Chronicle | all six | The engine filters them per slice. |

Measured with the balance example (8 seeds each, every game reached turn 60 and ended by The Chronicle):

| Slice | First nation enters | Second era reached by turn 60 |
|---|---|---|
| Cradles | Iron at turn 28 | all games |
| Crossroads | Paper at 11, Sail at 56 | half the games |
| Engines | Steam at 19, Electricity at 55 | one game in eight |

Crossroads and Engines start in the era *before* their first, because a nation enters an era by
discovering its namesake tech and the start grants only the earlier eras' techs. Once the start also grants
the first era's namesake tech (an engine ask), both slices should reach their second era around turn 30.

## Tuning

Numbers come from the balance runner:

```sh
cargo run --release -p civ-sim --example balance -- --ruleset blitz --seeds 1..=200 --threads 4
```

`--overlay DIR` layers trial files over the bundled rules, so a change can be tried before it is
committed. The full game's pace lives in its content costs (CONTENT.md, As built); Blitz inherits it.
