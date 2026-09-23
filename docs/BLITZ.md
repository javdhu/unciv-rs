# Blitz

Blitz is civ.game's one-evening mode: about **60 minutes** over the **full tech tree, compressed**
(GAPS.md, 2026-09-23). It is a ruleset layered over the full game, not a fork of it.

## How it is built

`crates/civ-core/rules/blitz/` holds only what Blitz changes. The loader reads the Gods & Kings files
first and Blitz's after them (`content::load_ruleset("blitz")`). A Blitz object with the same name
replaces the G&K one whole, and a new name adds an object, the way an Unciv mod layers over its base
ruleset. Blitz's `ModOptions.json` removes objects by name with Unciv's `buildingsToRemove`,
`unitsToRemove` and `nationsToRemove`, plus `victoryTypesToRemove`.

A match picks its ruleset with the additive `ruleset` setting (`gods-and-kings` by default, or
`blitz`). The content hash covers the full game's files, the ruleset's name and its own files, so a
Blitz match never shares a hash with a full one. Saves carry that hash and the world's ruleset
identity, so a Blitz save never loads as G&K (tested both ways).

## First cut, and why

| Rule | Blitz | Full game (Standard) | Why |
|---|---|---|---|
| Speed | `Blitz`, the ruleset's default | `Standard` | One knob set for the whole game. |
| Turn limit | 80 | 500 | 60 minutes at about 45 s a turn. PLATFORM.md's older first cut said 60. |
| Research cost | see *Tuning* | ×1 | The whole tree must be reachable: the median game should reach the Information era near the limit. |
| Production, growth, gold | see *Tuning* | ×1 | Scaled with research, or cities would sit on one building while techs fly past. |
| Culture, faith, great people | see *Tuning* | ×1 | Great people follow the speed modifier, as in Unciv. |
| Religious pressure | 18 per city | 6 | Religion is on, and compressed: it has to spread inside 80 turns. |
| Deals, peace treaties | 10 and 5 turns | 30 and 10 | Scaled to the game's length. |
| Golden ages, barbarians | ×0.4, ×0.3 | ×1 | Same. |
| Map | small (the default 32×20), 4 civs, 2 city-states | same defaults | The brief asks for small, 4 civs and 2 to 4 city-states. These are already the setting defaults. |
| Start | settler, warrior, worker, scout | settler, warrior | The worker comes from Blitz's Ancient era. The scout comes from Prince's `playerBonusStartingUnits` and `aiMajorCivBonusStartingUnits`, which the setup now honours (GameStarter.kt:454). |
| Diplomatic victory | off (removed, with the United Nations) | on | Decided in GAPS.md. G&K has no World Congress to turn off. |
| Espionage | on | on | **Not decided by the user yet.** Kept on unless the runner shows it adds nothing. |

The victories Blitz ends on (wonder race, short culture, short science) are task B4. Until then Blitz
plays G&K's domination, science, culture and time victories without the diplomatic one.

## Tuning

Numbers come from the balance runner:

```sh
cargo run --release -p civ-sim --example balance -- --ruleset blitz --seeds 1..=1000 --threads 4
```

`--overlay DIR` layers trial files over the bundled rules, so a change can be tried before it is
committed.

Targets (ENGINE-BRIEF.md B3): median game length 65 to 85 turns; no nation wins more than 1.5× its
fair share; every enabled victory type occurs in at least 10% of games.
