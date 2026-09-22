# The systems that have to be designed

Notes for the second half of the port. Dated 2026-09-21.

Sizes below are stub counts from `tools/stub_manifest.json` — an indication of
surface area in Unciv, not an estimate of the work here, since a system's shape
matters more than its line count.

| | stubs | |
|---|---|---|
| AI / automation | 206 | no reference obligation |
| map generation | 189 | aesthetic, not rules |
| diplomacy | 167 | the hardest state shape |
| quests / city-states | 158 | directed pair matrix, like diplomacy |
| religion | 104 | sparse matrix + diffusion |
| trade & connectivity | 57 | a graph problem |
| espionage | 56 | hidden state, per-player views |
| policies | 34 | mostly already generated |
| victory | 31 | predicates, not state |
| barbarians | 22 | seeded spawning |
| great people | 15 | reuses the growth pattern |

---

## What generation has and has not been able to do

Worth stating plainly, because it predicts which of the above is cheap.

**Generation worked** where the thing was already an enumeration or a schema:
638 unique types, 2,441 sprite ids, 61 wire structs, 23 ruleset structs, 3,939
signature stubs. All mechanical, all verified.

**Generation could not see** three kinds of thing, each of which cost a bug:

1. **Relationships that exist as structure.** A tech's era is its *nesting* in
   Techs.json; an era's number is its *position*; a policy branch is the object
   that contains its policies. Schema inference reads fields, and these are not
   fields.
2. **Fields the format allows but the data never uses.** No shipped improvement
   grants happiness, so inference omitted the field — and a mod could set it.
3. **Semantics carried in behaviour.** That terrain *folds* rather than sums;
   that a movement cost over budget is clamped rather than refused; that Kotlin
   truncates where I would have rounded. None of these are visible in any
   schema.

So the rule for what follows: **anything with a table is generatable, anything
with an invariant is not.** Each system below is judged on that.

---

## Cross-cutting decisions already made, which the rest must obey

These were paid for once and should not be relitigated per system.

1. **Derived state gets one pass, declared in one place.** `Ruleset::derive` and
   the `DERIVED` table. Unciv has 185 `@Transient` fields across 43 files and 28
   `setTransients()` functions; that is what happens without the discipline, and
   it is why it needs a rebuild step after every load.
2. **Order-independence where composition allows it, an explicit fixed order
   where it does not.** `ModifierStack` guarantees the first across 720
   permutations. The terrain fold is deliberately the second, because
   `overrideStats` makes order meaningful — and the sequence is pinned rather
   than left to iteration.
3. **Truncate where the reference truncates.** Kotlin's `toInt()` truncates
   toward zero. Rounding instead cost one food per city per growth, compounding
   all game, and was caught only by a differential.
4. **Deficits, not absences.** A civ with no Iron and no Swordsmen is at zero,
   which is fine. The penalty is for being *short*.
5. **Provenance on anything a player can question.** Yields, rankings and
   combat strengths all carry their sources. Bolting it on later never works.
6. **Events, not mutations.** A turn produces `TurnEvent`s that are the save
   format and the wire format. A system that only mutates has to have its event
   reconstructed from a diff.
7. **Ids, never pointers. Generations only where things die.** Tiles are a dense
   index; units and cities are arenas.

---

## Diplomacy — the state shape is the whole problem

**The decision.** A relationship is between a *pair*, so it is an N×N structure,
not per-civ state. Unciv gives each `Civilization` a map of `DiplomacyManager`
keyed by the other civ's name — which means every symmetric fact is **stored
twice** and has to be kept in step by hand. War is the obvious one: A at war
with B and B not at war with A is representable, and therefore eventually
happens.

**Recommendation: split symmetric from directed.**

```
symmetric: BTreeMap<(CivId, CivId), Pact>       // key always (min, max)
directed:  BTreeMap<(CivId, CivId), Attitude>   // A's view of B
```

War, peace treaties, embassies, open borders and defensive pacts are facts about
the pair and live in the first, stored once under a canonically ordered key.
Opinion, grudges, promises and trade memory are A's view of B and live in the
second. The asymmetry is real — B's resentment need not match A's — and the
symmetry is then true *by construction* rather than by maintenance.

**What is generatable:** nothing much. The uniques that diplomacy consults are
already in the algebra; the structure is a design choice.

**The invariant to test:** it should be impossible to express a one-sided war.
If the type system allows it, the design is wrong.

**City-states and quests** (158 stubs) are the same shape — influence is
directed, from a major civ toward a city-state — so they should reuse whatever
this becomes rather than inventing a parallel structure.

---

## Happiness — derived, never stored

**The decision.** Happiness is a global pool with per-city contributions and
threshold effects with cliffs: below zero, growth slows; far below, cities
revolt. The temptation is to cache it, because it is read constantly.

**Do not.** It is a pure function of the world, and a cached copy is a
`@Transient` by another name. Recompute it in the query layer each turn. The map
is ten thousand tiles and the arithmetic is integer; this is not the expensive
part of a turn.

**The composition rule that will bite:** the cliffs are thresholds applied
*after* summation, which is exactly `ModifierStack`'s clamp layer. Use it rather
than writing the comparisons inline, or the ordering will drift between the
places that consult happiness.

**Invariant:** computing happiness twice in a row returns the same value, and no
code path outside the computation writes to it.

---

## Policies — almost entirely generated already

**The decision.** A policy's *effects* are uniques, and uniques already work. So
the only authored parts are the adoption gating (cost, branch prerequisites,
era) and — the real question — **where a civ's active uniques live**.

**Recommendation.** A per-civ `UniqueIndex`: a 638-bit presence bitset plus a
payload vector, rebuilt from scratch whenever policies, buildings or techs
change. Rebuild, not incremental update. Incremental is where staleness lives,
and `hasUnique` is the hottest read in the engine — 122 call sites in the AI
alone — so it wants a bit test, not a hash lookup. 80 bytes against the ~5 KB
Unciv spends on an `EnumMap` per object, which its own source comments on.

---

## Religion — pressure is state, followers are not — **done**

**The decision.** Religious pressure accumulates per (city, religion) — a sparse
matrix, since most cities feel two or three religions at most. Follower *counts*
are a function of pressure and population.

Store the pressure. Derive the followers. Storing both invites them to disagree.

**Spread** is diffusion over the city adjacency graph, which needs the same
determinism care as pathfinding: a fixed iteration order, or two machines reach
different religions. Iterate the sparse map in key order, not hash order.

**What the build added.** The determinism warning was right and lands in a
sharper place than expected: the split of a population between religions is
*largest-remainder apportionment*, the rule used for parliamentary seats, and
Unciv breaks ties between equal remainders by hash order. Two cities with equal
pressure from two religions is not a rare case — it is the ordinary case on the
turn a second religion arrives. Ties break by civ id here.

Two further shapes:

* Atheism is a positive quantity, not an absence. Every city holds 100 pressure
  for no religion. It keeps the denominator off zero and gives a city something
  concrete to convert *from*.
* A religion is identified by the civ that founded it, which is Civ V's own
  rule — one religion per civ — and saves a separate arena.

Spread runs once after every civ has moved, not inside a civ's turn, so the civ
that goes first cannot push before the others have grown. Pressure from every
source is gathered before any follower count is recomputed, or a city pushed by
three religions would report converting three times in one turn.

**Not yet reached in a real game.** Faith income needs a shrine, and the AI at
permanent war on a bare map builds soldiers instead, so the 300-turn self-play
founds no religion at all. The end-to-end test grants the faith building
directly. That is an AI priority question, not a rules one, and it is the
honest state of it: the system is correct and the AI does not yet reach for it.

---

## Trade routes and connectivity — a graph, recomputed — **done**

Implemented in `src/world/connections.rs`.

**The decision.** City connection is reachability over roads, harbours and
territory. It is derived, and the tempting optimisation is a dirty flag.

**Recommend recomputing per turn instead.** A BFS over 10,240 tiles is
microseconds, and invalidation bugs are not. Revisit only with a measurement.

**What the build added to the design.** Three things the sketch did not have:

* Roads are a *second axis* on the tile, not a kind of improvement. A tile
  carries a farm and a road at once, so `TileGrid` grew `road` and
  `road_pillaged` beside `improvement`.
* Open borders is the one genuinely one-sided fact about a pair of civs. It is
  stored as two bits inside the single `Pact` record — the asymmetry is real,
  the duplication would not be.
* The medium a city was reached by has to be remembered, not just *whether* it
  was reached, because a harbour hop keeps the grade of the road that fed it.
  That is the whole reason for the `Mediums` bitmask.

**What it cost elsewhere.** Making roads worth building exposed that the worker
AI had no commitment: it re-decided from scratch every turn and treated roads
as the job of last resort, so a growing city's endless supply of unimproved
tiles meant roads were never laid. Joining two cities five tiles apart took
110 turns. Putting connection ahead of tile yield took it to 19, which is the
optimum. The lesson generalises — a system that is merely *present* is not
finished until something in the game actually reaches for it.

---

## The treasury — unit upkeep, and what debt does — **done**

Implemented in `src/world/economy.rs`. Not on the original list, and it should
have been: it was found by running the game rather than by reading Unciv.

Three hundred turns of self-play ended with every civ between −474 and −1425
gold. Two things were missing and one was wrong.

**Missing: unit upkeep.** Charged on a curve, not per soldier —
`(0.5·units·(1+progress))^(1+progress/3)`, with the first three free. A flat
price per unit makes a large empire's army free, because income scales with
cities and the bill scales with units. The fractional power is computed as
`2^(y·log2 x)` in fixed point, because `powf` differs between machines in the
last bits and that is what makes a replay diverge.

**Missing: any consequence for debt.** Gold could only fall. Civ disbands
military units once the treasury is below −200 *and* income is still negative —
both conditions, so reserves buy time and a balanced budget is left alone. If
the whole army is worth less than the shortfall the whole army goes and the civ
is still broke; that is not a bug, it means the money is going somewhere the
army cannot fix.

**Wrong: the road AI ignored what roads cost.** It connected every city it
could reach. One civ was paying seventeen gold a turn for roads that earned it
eight. A link is now built only if the route it opens is worth more than the
upkeep of the tiles inside our own borders — which means two size-one towns
four tiles apart correctly refuse to build one.

With all three, the same self-play ends with a clear winner by Domination and
the defeated civs solvent.

---

## Victory — an event, not a flag

**The decision.** Victory conditions are predicates over world state. Evaluate
them at end of turn in civ id order, and emit a `TurnEvent` when one is met.
Do not store a `hasWon` boolean: a stored flag has to be cleared correctly when
a condition stops holding, and an event does not.

---

## Great people — reuse the growth pattern

Points accumulate, a threshold is crossed, the threshold rises. That is
`apply_food` and `apply_science` again. Write it as
`(state, points) -> (state, Event)` and it will be right for free.

---

## Barbarians — seeded from state

Spawning needs randomness, and the pattern exists: seed from `(turn, tile)` via
`Rng::from_parts`, never from an ambient generator. The only new decision is
*what* goes into the seed, and it must be something both server and client can
compute.

---

## Map generation — the one place to diverge deliberately — **done**

Implemented in `src/world/mapgen.rs`.

189 stubs, and none of them are rules. Map generation is an aesthetic and
balance exercise, and Unciv's answer is one option rather than a specification.
It is also the system where a differential harness has nothing to say.

**The one hard constraint:** it must be a pure function of a seed, or saved
games cannot be regenerated and multiplayer cannot agree on a map. That rules
out floating point, so the noise is hashed rather than trigonometric and every
field is fixed point.

**Three scalar fields** — elevation, temperature, humidity — each fractal value
noise, with terrain a lookup on all three. Two decisions the sketch did not
anticipate:

* **Sea level is a quantile, not a threshold.** A fixed cutoff on noise gives
  whatever water fraction that seed happens to produce: measured between 42%
  and 84% across eight seeds, which is the difference between a continent and
  an archipelago nobody asked for. Sorting the elevations and cutting at the
  asked-for fraction costs one sort and removes the variance entirely.
* **The edge falloff is east–west only.** Pulling the north and south edges
  down too drowns the poles, and then the temperature bands have no polar land
  to put ice on — the first draft produced a world with no snow anywhere,
  because everywhere cold enough for it was underwater.

---

## What a generated map exposed about the AI

Running a real game on real ground, rather than on the uniform grassland the
other self-play uses, found four faults in one assertion — that cities should
*grow*. Twenty-one cities, twenty-three citizens, two hundred and fifty turns.

1. **`recompute_stock` was never called from the turn loop.** Every civ's
   resource stock stayed empty for the whole game. The luxury happiness bonus
   had nothing to count and the missing-strategic-resource combat penalty never
   fired: two systems correct, tested, and unreachable.
2. **The worker never extracted resources.** It ranked improvements by their
   yields, and a Plantation on Silk adds a little food — it also adds four
   happiness to the entire empire. It also only considered *worked* tiles, so a
   luxury two tiles over that nobody was standing on was invisible.
3. **Building maintenance was a hard gate.** Any building costing more upkeep
   than the current surplus was struck off, which deadlocks: a poor civ cannot
   buy the buildings that would make it rich. Upkeep is priced against the
   building's worth now, twice as sharply when the books are in the red.
4. **Expansion ignored happiness.** The flat five-city target's own comment
   said "a real one would weigh happiness". Below zero happiness the ruleset
   cuts growth by three quarters and then stops it, so city eight in an unhappy
   empire does not add an eighth of an empire — it freezes the other seven.

With all four, the 300-turn grassland game went from three bankrupt empires
with fourteen techs to three solvent ones with twenty-odd, and takes five
hundred turns to resolve because the survivors are now hard to kill.

The pattern, again: **self-play finds missing connections; unit tests find
wrong answers.** Every one of these four systems passed its own tests.

---

## AI — the system with no reference obligation

206 stubs, and the only major system where matching Unciv is explicitly *not*
the goal. It is heuristics, not rules; it may diverge freely.

**The decision that matters is already made.** The turn loop's phase split means
AI is a pure function `(world, civ) -> TurnPlan` taking `&World`. That gives
three things at once: it can be swapped and compared, it can be tested without a
mutable world, and it can run for every civ at the same time. Unciv cannot do
any of the three — it retrofitted threading with 21 `synchronized` blocks.

**The constraint that will shape it:** for the browser-multiplayer target, eight
AI civs must plan in the time a human tolerates between turns. That is a budget,
and it should be measured before the heuristics get clever.

---

## The genuinely new work

Unciv is no reference for any of this, because it is a different product.

**Netcode.** Server-authoritative, event-sourced. The `TurnEvent` log is already
the wire format, which was the point of writing growth as a transition rather
than a mutation. What is missing is the per-player *view*: deriving what one civ
may know, once per turn, on the server. That is a security boundary, and the
research note is emphatic that a filter applied per query will eventually be
forgotten at one call site — and that call site leaks the map.

**The client.** React for panels, PixiJS for the map, the generated `Sprite`
enum and wire structs already in TypeScript. The rules run in the browser as
wasm, which is why the determinism check exists.

**Lobby, matchmaking, spectating, ranking.** Ordinary web-service work, and the
only part of this project where the interesting problems are not Civ's.

---

## Found by self-play

A civilization left to run itself for a hundred turns reached population 6, 8
technologies, 3 policies — and **-168 gold**. Two things were missing, and
neither was visible to any unit test.

**The rule.** Civ does not sell your buildings when the treasury empties; it
takes the shortfall out of *research*, floored so at least one science remains
(`CivInfoStatsForNextTurn.kt:216`). Implemented.

**The input.** A capital starts with a palace, and nothing granted one — so the
city had no gold income whatsoever. The palace is identified by the
`IndicatesCapital` unique rather than by the name "Palace", because a ruleset
may rename it.

| | techs | buildings | policies | gold |
|---|---|---|---|---|
| as found | 8 | 3 | 3 | -168 |
| solvency rule alone | 8 | 1 | 0 | 0 |
| with the palace | **13** | **6** | **4** | **+56** |

Worth recording as a pattern rather than a bug. Self-play surfaces missing
*consequences* and missing *inputs*; unit tests check that a system behaves in
isolation, and every one of these did. The holes only appeared when they ran
together long enough to compound.

It also shows where a rule ends and an opinion begins. The science penalty is a
rule and belongs in the turn loop; *not committing to upkeep you cannot cover*
is an opinion and belongs in `ai.rs`.

## Suggested order

1. **Happiness**, because growth, policies and city-states all read it, and it
   is small.
2. **Policies**, because the `UniqueIndex` it needs is the hottest read in the
   engine and everything after benefits.
3. **Diplomacy**, because city-states and quests should be built on its shape
   rather than beside it.
4. **Trade, religion, great people, barbarians** — each self-contained once the
   above exist.
5. **Victory**, which needs most of the others to be meaningful.
6. **AI**, last among the rules systems, because it consumes all of them.

Netcode and the client are independent of this order and can proceed in
parallel; map generation can happen any time and is the natural place for
someone who wants a self-contained piece.
