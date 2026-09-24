# Blitz

Blitz is civ.game's one-evening mode. It plays a **slice of eras at the full game's pace** (CONTENT.md,
Spine), not the whole tree compressed: three slices are planned, Cradles (Bronze and Iron), Crossroads
(Paper and Sail) and Engines (Steam and Electricity), each 60 turns. Until the engine can start a game in
a later era, Blitz is the **Cradles** slice. It is a ruleset layered over the full game, not a fork of it.

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

## The Cradles slice, as built (2026-09-24)

| Rule | Blitz | Full game (Standard) | Why |
|---|---|---|---|
| Speed | `Blitz`, the ruleset's default | `Standard` | Blitz keeps Standard's costs (every modifier 1.0), so an era takes the same 30 turns as in the full game. |
| Turn limit | 60, from 4000 BCE to 500 CE | 240, from 4000 BCE to 2026 | Two eras of 30 turns; the years follow the first two eras of Standard's calendar. |
| Deals, peace treaties | 15 and 5 turns | 30 and 10 | Scaled to the slice's length. |
| Map | small (the default 32×20), 4 nations, 2 city-states | same defaults | The brief asks for small, 4 nations and 2 to 4 city-states. |
| Start | settler, warrior, worker, scout | settler, warrior | The worker comes from Blitz's Age of Bronze; the scout from Blitz's Tutor difficulty (`playerBonusStartingUnits`, `aiMajorCivBonusStartingUnits`). |
| Diplomatic victory | off (Perpetual Peace and the World Charter removed) | on | The charter needs the Internationalism tree of the Age of the Atom, out of reach in the slice. |
| Victories | The Chronicle at turn 60, and any other that can be met | all six | The slice's own victories wait for the engine's slice support. |

Measured with the balance example on 12 seeds: the first nation enters the Age of Iron at turn 29 and
the Age of Paper at turn 57, so the slice ends as the next age opens. Every game ended by The Chronicle.

The engine is adding slice support (a starting era, the 60-turn span, The Chronicle as the fallback);
Crossroads and Engines, and the slices' own victories, follow its data shape.

## Tuning

Numbers come from the balance runner:

```sh
cargo run --release -p civ-sim --example balance -- --ruleset blitz --seeds 1..=200 --threads 4
```

`--overlay DIR` layers trial files over the bundled rules, so a change can be tried before it is
committed. The full game's pace lives in its content costs (CONTENT.md, As built); Blitz inherits it.
