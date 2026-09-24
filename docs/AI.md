# The AI

How the major-civ AI gets better, and how we know it did. Agreed 2026-09-24 ("your pick" on the
review): measure first, then fight, then play to win, then give leaders a face.

## Baseline (2026-09-24, main 896961f, 20 seeds, small map, 4 civs, 2 city-states)

| | Standard | Blitz |
|---|---|---|
| How games end | 60% Reserve Currency, 35% turn cap, 5% conquest | 100% score at turn 60 |
| Cities per civ | median 2 at turn 25, 3 at 50 and 75 | |
| Wonders per game | 5.2 | 1.8 |
| Engine time per turn (M-series Mac) | 181 ms | 48 ms |

What the review found, by how much a player notices it:

1. **It fights blind.** Attacks the lowest-HP target without `preview_attack`, never retreats or heals,
   cities never bombard, melee only hits what it already stands next to, units trickle in one by one.
   Every AI civ plans against the start-of-round world, so later civs act on stale positions.
2. **It has no plan.** Research and policies are cheapest-first, no victory goal, no wonder races, a
   flat target of 5 cities, unescorted settlers.
3. **Every leader plays the same.** `Personalities.json` is unread by the major-civ AI; difficulty
   scales the economy but no decision.
4. **Wars have no story.** War whenever strong enough and a city is within 8 tiles; no target choice,
   no allies, no dogpile on the leader, peace ignores war goals.
5. **Per-unit whole-world scans** (every unit rescans all cities and units and re-pathfinds).

Already good: grudges (`opinion.rs`), promises and border demands, the air AI, worker priorities,
city-site scoring, victory-building urgency, determinism.

## Levels

| # | Level | Done when |
|---|---|---|
| 4 | **Measure.** `balance --ab` seats the old AI against the new in the same games. | Mixed-game win share per side is reported. |
| 1 | **Fight properly.** Scored attacks, move-then-attack, retreat and heal, bombard, siege massing, garrisons. | New beats old in mixed games (>55%) on Standard. |
| 2 | **Play to win.** A victory goal per civ steers research, policies and wonders; wonder races; expansion scaled to the map; escorts. | Blitz games end in real victories, not only on score. |
| 3 | **Leaders with a face.** Personalities and difficulty in every decision; wars with warnings, targets, allies and a dogpile on the leader. | Per-leader differences show in 500 seeds (brief B6). |

## Results

**Level 4 (2026-09-24).** `balance --ab` seats old against new, alternating halves with the seed;
`--legacy` puts every seat on the old AI for baselines and timing.

**Level 1 (2026-09-24), `world/tactics.rs`.** Mixed games, small map, 4 civs:

| | Blitz (40 seeds) | Standard (20 seeds) |
|---|---|---|
| New AI wins | 35 (87.5%) | 19 (95%) |
| Score share, new : old | 60 : 40 | 76 : 24 |
| Old-AI civs wiped out | 16% | 62% |
| Engine time per turn | about 8% over the old AI (load-noisy) | |

Still true after level 1: no game ends in conquest; Standard ends by Reserve Currency or the turn cap,
Blitz on score. Level 2 is for that.

**Level 2, first pass (2026-09-24), `world/goals.rs`.** Each AI reads its goal off the board every
turn (Conquest, Trade, Influence, Science, Score in the last 15%). The goal moves the war threshold
(conqueror −0.3×, builder +0.25×), keeps a winning conqueror at war, beelines research (route techs
at 55% of cost), sizes the army (3 per city for a conqueror), stops razing, and aims the march at
original capitals. Units are weighted by the role the army lacks (`ai::army_need`); before this a
coastal "army" was nine ships and a pikeman. A melee hit that brings a city to 1 health now counts as
decisive, so wounded units take a falling city instead of retreating from it.

All-new AI, 20 Standard seeds (after level 1 → after this pass):

| How games end | After level 1 | Level 2, first pass |
|---|---|---|
| Turn cap (The Chronicle) | 80% | 60% |
| Reserve Currency | 15% | 35% |
| Conquest | 0% | 5% (turn 107) |

Against the old AI: 87.5% of 40 Blitz games, 90% of 20 Standard games. Goals at turn 50: about 22%
Conquest, 75% Science, a few Trade. Engine time is unchanged within noise.

**Found on the way:** `victory::score_leader` ranked city-states too, so a city-state that took two
majors' cities held the lead and the game never ended (seed 8). It now ranks living majors only.

**Blitz is a rules problem, not an AI one.** In 60 turns no civ ever gets a world trade share (the
15-gold-per-major floor is never reached) or influences one rival, and the best conqueror holds 2 of
4 capitals. Every Blitz game ends on score whatever the AI does, until the Blitz victories (brief B4)
or the floors are tuned.

## Method

- The old AI stays callable while levels 1–3 land: `Civilization::legacy_ai` (never saved) marks the
  seats that play the pre-2026-09-24 AI. Only the balance runner sets it. It is deleted once level 3
  is in.
- `balance --trace N` prints each major's goal, strength, wars and march objective every N turns;
  `TRACE_UNITS=1` adds its units. That is how the ship army and the stalled sieges were found.
- Every level keeps determinism and "planning never touches the world".
- The live tactics run in `finish_round` (the game and the balance runner). `advance_turn`, the
  all-AI test driver behind the determinism fingerprint, still plans the old way until the legacy AI
  is removed.
