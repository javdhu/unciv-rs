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

## Victories (B4)

| Victory | Blitz | Full game |
|---|---|---|
| Domination | capture every original capital, as G&K | same |
| Score | highest score at the turn limit (turn 80), as G&K | same, at the speed's limit |
| Culture | **3** policy branches, then the Utopia Project (which now needs 3) | 5 branches |
| Science | the Apollo Program, then **3** parts: booster, cockpit, engine | 6 parts |
| Wonder race | **new:** the first civ to hold **9** world wonders (`Build [9] [World Wonders]`) | none |
| Diplomatic | removed | UN and a vote |

The wonder race uses a new milestone form, `Build [amount] [buildingFilter]`, which counts the
buildings a civ holds that match the filter (`World Wonders` here). A wonder taken with a city counts
for its new owner. National wonders don't count.

Short science also moves the three parts earlier and drops their Aluminum: the booster to Rocketry,
the cockpit to Advanced Ballistics, the engine to Satellites. As G&K has them (Particle Physics and
Aluminum), science never won inside 80 turns.

## Tuning

Numbers come from the balance runner:

```sh
cargo run --release -p civ-sim --example balance -- --ruleset blitz --seeds 1..=1000 --threads 4
```

`--overlay DIR` layers trial files over the bundled rules, so a change can be tried before it is
committed.

Targets (ENGINE-BRIEF.md B3): median game length 65 to 85 turns; no nation wins more than 1.5× its
fair share; every enabled victory type occurs in at least 10% of games.

## Results: 1,000 seeds

Blitz as bundled (research ×0.03, culture ×0.13, production and growth ×0.15; 9 wonders to win), small map,
4 civs and 2 city-states, run with:
`cargo run --release -p civ-sim --example balance -- --ruleset blitz --seeds 1..=1000 --threads 2`.
All AI leaders play alike here; leader personalities are task B6.

```

1000 games (0 panicked) · blitz · Blitz · 33x21 · 4 civs + 2 city-states

Game length: median 80 turns, mean 74.6, min 45, max 80
Engine time: mean 14773 ms/game, median 14118 ms/game, 198.01 ms/turn; wall 7395.2 s on 2 threads
First 60 turns: mean 9949 ms, median 9296 ms

Victory                 games   share  median turn
Cultural                   86    8.6%           74
Scientific                 77    7.7%           78
Time                      508   50.8%           80
Wonders                   329   32.9%           67
Ended early by capture: 0 (0.0%)

Nation               played  wins win rate  × fair   wars/g  wonders  founded
America                 106    28    26.4%    1.06     3.77     2.72     4.89
Arabia                  115    35    30.4%    1.22     3.94     3.33     5.03
Austria                 126    49    38.9%    1.56     4.15     4.08     5.25
Aztecs                  137    55    40.1%    1.61     3.80     2.74     4.64
Babylon                 130    28    21.5%    0.86     4.11     2.09     4.48
Byzantium               138    23    16.7%    0.67     3.58     1.88     4.91
Carthage                115    22    19.1%    0.77     3.71     2.77     4.96
Celts                   116    31    26.7%    1.07     4.21     2.26     4.99
China                   110    43    39.1%    1.56     3.87     4.04     5.19
Denmark                 132    26    19.7%    0.79     3.81     2.45     5.13
Egypt                   126    38    30.2%    1.21     4.02     2.87     5.08
England                 116    12    10.3%    0.41     3.53     1.77     4.85
Ethiopia                121    26    21.5%    0.86     3.51     1.83     5.29
France                  130    48    36.9%    1.48     4.18     3.21     4.72
Germany                 103    40    38.8%    1.55     4.15     3.65     5.14
Greece                  118    24    20.3%    0.81     4.04     2.32     4.76
Inca                    126    42    33.3%    1.33     4.58     3.55     5.32
India                   117    11     9.4%    0.38     2.62     1.06     4.34
Iroquois                136    30    22.1%    0.88     3.64     2.74     5.05
Japan                   114    29    25.4%    1.02     3.65     2.62     4.79
Korea                   111    18    16.2%    0.65     4.10     2.12     4.76
Mongolia                115    32    27.8%    1.11     4.00     2.99     4.97
Persia                  130    31    23.8%    0.95     3.73     2.65     4.88
Polynesia                94    19    20.2%    0.81     3.81     2.62     5.17
Rome                     99    18    18.2%    0.73     3.69     1.88     4.66
Russia                  121    37    30.6%    1.22     3.55     3.07     4.78
Siam                    129    33    25.6%    1.02     3.95     2.64     4.74
Songhai                 115    24    20.9%    0.83     3.67     2.15     4.67
Spain                   105    16    15.2%    0.61     3.60     1.63     5.02
Sweden                  108    27    25.0%    1.00     3.74     2.88     4.72
The Huns                125    38    30.4%    1.22     3.62     2.89     4.83
The Maya                116    29    25.0%    1.00     3.37     1.76     5.01
The Netherlands          95    11    11.6%    0.46     3.17     1.38     4.37
The Ottomans            105    27    25.7%    1.03     4.06     2.61     5.22

Era (first civ)       reached  median turn
Ancient era            100.0%            1
Classical era          100.0%           11
Medieval era           100.0%           20
Renaissance era        100.0%           30
Industrial era         100.0%           40
Modern era              98.9%           51
Atomic era              91.1%           62
Information era         62.3%           71
Future era               6.7%           79

World wonders built: mean 10.3 per game, median 10
  Hanging Gardens              built in 100.0% of games, median turn 19
  Notre Dame                   built in 100.0% of games, median turn 32
  The Great Library            built in 100.0% of games, median turn 16
  Chichen Itza                 built in  97.8% of games, median turn 42
  Terracotta Army              built in  95.4% of games, median turn 41
  Stonehenge                   built in  93.0% of games, median turn 44
  Colossus                     built in  71.7% of games, median turn 38
  Alhambra                     built in  66.8% of games, median turn 62

Turn         cities/civ     median/civ      games
25                 2.30              2       1000
50                 3.40              3        993
75                 3.80              4        647
```

Against the targets:

| Target | Result |
|---|---|
| Median game length 65 to 85 turns | **Met:** median 80, mean 74.6 |
| No nation above 1.5× its fair share | **Missed narrowly:** Aztecs 1.61×, Austria and China 1.56×, Germany 1.55× (95 to 138 games each, so about ±0.35× is noise) |
| Every enabled victory in at least 10% of games | **Missed:** Wonders 32.9%, Time 50.8%, Culture 8.6%, Science 7.7%, **Domination 0%** |

Why they miss, and what would move them:
- **Domination.** Taking every capital doesn't happen in 80 turns with today's AI: no game ended early by
  capture. That is the AI's aggression (B6 personalities, B7 tactics), not a rules knob. A Blitz-only
  form, such as holding more than half the original capitals, would be a design change for you to decide.
- **Culture and Science** were at 12.5% and 10.4% in the 48-seed tuning run, and landed just under 10% at
  1,000 seeds. Culture ×0.12 and science ×0.028 should lift each by a couple of points, at some cost to Time.
- **Nations.** Before B6, leaders differ only through their unique abilities and start positions, which is what these ratios show.

Engine time: about 15 s per Blitz game (200 ms a turn).
