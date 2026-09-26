# civ-core

Deterministic Civ rules core in Rust. Compiles to native (authoritative server)
and `wasm32` (browser client) from one source, so client and server cannot
disagree about the rules.

## Gameplay scope

The supported match rules are the bundled **Civ V: Gods & Kings** content.
Unciv editor tools and arbitrary mod-only rules are outside this scope. AI is
an authored deterministic policy, not a port of Unciv's strategic decisions.

Player actions enter through `world::commands`; mutually accepted agreements
use `world::agreements`. Those boundaries own validation and action settlement.
Air and naval movement, carrier cargo, setup and fortification, nuclear attacks,
city administration, explicit belief selection, spies, city-state quests and
gifts, alliances, and victory actions participate in that same world state.
The optional save feature stores and validates their durable state. The
`civ.game` facade projects player-specific information and generates the shared
native-server / browser-Wasm protocol from Rust.

## Design notes

[docs/remaining-systems.md](docs/remaining-systems.md) — design notes for the
historical port design decisions and why schema generation did not establish
gameplay completeness.

[docs/state-architecture.md](docs/state-architecture.md) — researched state
representation, determinism budget, parallelisation and the content pipeline,
grounded in measurements of Unciv and cited to primary sources.

## Approach

Not a transliteration. Porting Unciv's 3939 functions one-to-one would inherit
its object graph — aliased references, the `transients` rebuild, `UncivGame`
reaching into 41 files of game logic — and fight the borrow checker on every one
of them, throwing away the ID-indirection that made Rust the right choice.

Instead: **systems are authored here, and Unciv is the reference.** Three kinds
of code, kept clearly apart.

| | what | why |
|---|---|---|
| `src/generated/`, `src/ruleset/generated.rs`, `src/unique_type.rs` | derived from Kotlin and from the shipped json | schemas and enumerations are data; transcribing them by hand only adds errors |
| `src/stats.rs`, `src/modifiers.rs`, `src/unique.rs`, `src/icons.rs` | authored | the design decisions — these are what other systems attach to |
| `reference/kotlin-index/` | not compiled | a checklist of every behaviour the real engine has |

## The authored spine

**`stats.rs` — exact arithmetic.** `Fixed` is an `i64` in thousandths. Unciv
uses `Float`; floats are wrong here for one specific reason, which is that this
crate compiles to native *and* `wasm32` and the entire point is those two
agreeing bit for bit. Ten tenths sum to exactly one, and that is a test.

**`arena.rs` — entities by id, never by pointer.** Unciv's hardest bug class is
the dangling reference, and it keeps a whole `transients` layer alive to rebuild
the object graph after a load. That shape does not compile in Rust at all, so
the choice is forced early. Two containers, because ruleset data and world state
want opposite things: `Registry` is filled once and never mutated (names resolve
to ids at load, so no hot path compares strings), `Arena` reuses slots and
carries a generation, so a stale id returns `None` rather than whatever now
occupies the slot. Both iterate in index order — a `HashMap` would iterate
differently in two processes, and any rule that sums or picks-first over
entities would then desync.

**`json.rs` — the dialect the rulesets are actually written in.** libGDX's
relaxed syntax (`//` and `/* */` comments, trailing commas) that strict parsers
reject, and numbers kept as their original text so `Fixed` reads `0.95` as
exactly 950 thousandths. Going through `f64` first would quietly reintroduce the
non-determinism everything else is arranged to avoid.

**`ruleset/load.rs` — json to something queryable.** Takes already-read text
rather than paths, because the wasm client fetches over the network and has no
filesystem; `load_dir` is the native convenience on top. `dangling_refs()`
reports cross-references that point at nothing, which turns a mod's typo into a
message instead of a missing building at turn 40.

**`modifiers.rs` — how rules compose.** The bugs in a mature 4X are almost never
"this modifier is wrong", they are "these two interacted and the answer depended
on which the engine visited first". With 638 unique types, nobody tests the
pairs. So ordering is removed as a variable: contributions go into four ordered
layers (base → additive → multiplicative → clamp), and within a layer the result
*cannot* depend on insertion order — sums are integer sums, products accumulate
as exact rationals and round once. `result_never_depends_on_insertion_order`
asserts it across all 720 permutations of six modifiers.

Every contribution carries its `Source`, so `breakdown()` answers "where did
this 17 come from" without recomputing anything.

## What is generated

```
python3 tools/generate_unique_types.py   # 638 UniqueType variants
python3 tools/generate_ruleset.py        # 23 ruleset structs, from the json itself
python3 tools/generate_assets.py         # 2441 sprite ids (Rust + TS)
python3 tools/generate_api.py            # 61 wire structs, 3 enums (Rust + TS)
python3 tools/generate_stubs.py          # the reference index (not compiled)
cargo test
```

## What the tests prove

Run the current behavioral suite with `cargo test --features save --all-targets`.
The timing budget is intentionally ignored in debug builds; run
`cargo test --release --features save --test budget -- --nocapture` for that gate.
`tools/check_determinism.sh` also runs the release budget. Its state-size report
separates a partial in-memory payload lower bound from complete encoded snapshot
bytes; neither number claims to measure total heap usage.

The tables below record the original spike and parsing gates; they are not
current behavioral test counts or a claim of complete gameplay coverage.

**Rules algebra**

| check | result |
|---|---|
| unique strings in shipped rulesets resolving to a declared type | **2926 / 2932 (99.80%)** |
| `<conditionals>` resolving | **1158 / 1158 (100%)** |
| placeholder collisions across all 638 templates | **0** — unambiguous grammar |
| `parse(t.template()) == t` | passes for all 638 |
| fall-throughs | `{"Aircraft": 6}`, an intentional filter flag |

**Assets**

| check | result |
|---|---|
| sprite regions across 13 atlases | **2441**, 0 identifier collisions |
| `from_region(s.region()) == s` | passes for all 2441 |
| ruleset objects resolving to a packed icon | **660 / 660** |
| of those, landing on a group fallback | 2 (`Devout`, `Unable to pillage tiles`) |

`UniqueKind::Tag` and the icon fallback tier are design features, not gaps:
rulesets may use a bare string as a filter flag, and a few objects ship no art.
Modelling both explicitly is what keeps mods loadable without a code change.

## Layout

```
src/unique.rs             parser: bracket-balanced params, nested conditionals
src/icons.rs              icon resolution chains (promotion levels, branches, city-states)
src/generated/assets.rs   2441 sprite ids
src/generated/api.rs      wire contract, serde behind the `serde` feature
../civ-web/src/generated/ the same assets + api as TypeScript
```

The library has **zero runtime dependencies** by default — the rules layer must
stay deterministic and wasm-portable. `serde` is opt-in; `serde_json` is a
dev-dependency used only to read fixtures.

The legacy `civ-web` generated index is reference data. The live client
contract is generated and checked in `civ.game/web/src/engine/generated`.

## The reference index

`reference/kotlin-index/` holds 638 types and 3939 functions from Unciv's
`logic` + `models`, each with its signature, `file:line` and full body, wrapped
in `@kotlin-ref:begin` / `@kotlin-ref:end` markers that
`tools/strip_kotlin_refs.py` removes.

It is **not compiled and not the port**. It is the coverage checklist:
`tools/stub_manifest.json` enumerates every behaviour the real engine has, and
work proceeds by authoring systems that cover those behaviours — not by
translating functions. `tests/stub_integrity.rs` still guards it against silent
loss, because a checklist that quietly shrinks is worse than no checklist.

## Derived relationships

Some of what the ruleset means is not in the json. A tech's column is its
*nesting*; an era's number is its *position*; a policy's branch is the object
that contains it. Unciv establishes all of these while loading
(`Ruleset.kt:395`, `:425`, `:468`, `Terrain.kt:212`).

Schema inference cannot see any of them — nothing in the files mentions them —
so they live in a hand-maintained `DERIVED` table in `tools/generate_ruleset.py`
and are populated by `Ruleset::derive`. This is the seam between the two kinds
of work: generation gets the shape, authoring gets the relationships.

Found by sampling stub sites at random and trying to complete them; the first
one that needed an era number could not be written at all.

## The ruleset loads

`tests/suite/ruleset_load.rs` reads the shipped files for real:

| | Gods & Kings |
|---|---|
| objects loaded | **812** |
| buildings / units / techs | 124 / 127 / 80 |
| promotions / nations / policies | 106 / 83 / 60 in 10 branches |
| terrains / resources / improvements | 33 / 35 / 35 |
| building uniques resolving to a `UniqueType` | **218 / 218** |
| **dangling cross-references** | **0** |
| tech columns / techs reaching an era | 36 / **80 of 80** |
| policies reaching a branch | **60 of 60** |

Counts are cross-checked against the icon coverage test, which walks the same
files by an entirely different route. Vanilla loads too.

## The world

`src/world/` — the map and who can see it.

`TileGrid` is structure-of-arrays over a dense `TileId(u16)`, with adjacency
computed once at construction. Tiles are the one thing in a Civ game that is
never created or destroyed, so `TileId` carries **no generation counter** —
`Arena`'s generations exist to catch use-after-free, and paying for them where
nothing can die is the mirror of the bug they prevent. Units and cities keep
their arenas.

`Visibility` inverts Unciv's fog storage. Unciv gives each tile a
`HashSet<String>` of civ ids (`Tile.kt:54`); one bitset per civ over dense tile
indices holds the same information in **1.28 KB per civ**, and a full 128×80 map
with eight players costs under 21 KB for both masks. Three states are kept
apart — never explored, explored but unobserved, currently visible — because the
middle one is what makes remembered terrain work.

## First vertical: tile yield

`world/yields.rs` answers the most-asked question in the engine, and sets the
shape later systems copy.

Terrain does **not** sum. Unciv folds over `[base, natural wonder, ...features]`
with three outcomes per step (`TileStatFunctions.kt:164`): `NullifyYields`
discards everything, `overrideStats` replaces what came before, otherwise it
adds. The shipped ruleset uses all of it — Forest, Jungle and Hill override, so
a forested Grassland yields Forest's 1/1 rather than Grassland's 2 food plus it,
while Marsh is a plain `food: -1` that subtracts.

That fold is order-*dependent* on purpose, which is why the sequence is pinned
rather than left to iteration order. It does not contradict `ModifierStack`:
this establishes the base, the stack handles the bonuses layered on top, where
order genuinely should not matter.

Every step is attributed, so "why is this tile worth 4 food" is answerable
without recomputing it — Unciv returns `List<Pair<String, Stats>>` for the same
reason.

## Cities

`world/city.rs` — the first system that *consumes* tile yield rather than
producing it, and the first with a turn loop.

Growth is written as a pure transition, `(city, food) -> (city, GrowthEvent)`,
rather than a method that mutates and notifies. The engine is event-sourced, so
the event is what gets recorded, replayed and sent to clients; a version that
only mutated would have to have the event reconstructed from a diff afterwards.

The curve is `15 + 8(p−1) + ⌊(p−1)^1.5⌋`, scaled by game speed and by the civ.
This is what `Fixed::pow` was built for.

**Truncation, not rounding.** Unciv ends `getFoodToNextPopulation` with
`foodRequired.toInt()`, and Kotlin's `Double.toInt()` truncates toward zero — so
a city-state's 82.5 is 82. Rounding instead would cost one food per growth and
compound across a whole game. Exactly the kind of quiet divergence a
differential harness is meant to catch, caught here by writing the expectation
down first.

## Citizen assignment

`world/assignment.rs` — the first genuine *decision*. Everything before it
computed a value; this one chooses.

It is a weighted sum, **not** a `ModifierStack`. The stack exists because game
rules compose in ways where ordering must not matter; ranking is one dot product
against a fixed weight vector, and dressing it up as layered modifiers would add
ceremony without adding a guarantee.

The weights are Unciv's (`Stats.kt:157`), fractional parts included, because
those parts are the design. 2 gold weighs 12.000 against production's 12.010 —
worth the same to within a hundredth, with production taking the tie. 4 science
is 36.04 against 3 production's 36.03. Both gaps are exactly 0.01, and both are
asserted.

Food is not weighted like the others. It splits into the amount that stops a
city starving and the surplus beyond it, and only the first gets the emergency
multiplier. Unciv says why: scaling only what is needed lets a starving city
"pick a tile that gives exactly as much Food as we need to Not Starve that also
has other good yields instead of always picking the Highest Food tile".

Rankings carry a per-stat breakdown, so a choice can be explained rather than
trusted, and ties break on `TileId` so two machines assign identically.

**Deferred:** happiness penalties on growth, We-Love-The-King-Day, food-to-
production conversion and specialist slots. Each needs city or civ state that
does not exist yet; the shape here has a place for all of them.

## The turn loop

`world/turn.rs` splits a turn in two, following `docs/state-architecture.md`:

1. **Plan** — each civ reads the world and returns a `TurnPlan`. Takes `&World`
   and mutates nothing, so every civ's plan could be computed at once. This is
   where the expensive work lives; Unciv has ~8k lines of automation deciding
   these things.
2. **Apply** — plans are consumed in `CivId` order and mutate. Cheap,
   sequential, reproducible because the order is fixed.

Nothing here runs in parallel yet, and that is deliberate — the crate stays
dependency-free, and the *shape* is what has to be right. Making the planning
loop parallel is a change at the call site, and because the phases are already
separated it cannot change the result. That property is what lets the same log
replay single-threaded.

`planning_does_not_touch_the_world` asserts phase one's purity by cloning the
world, planning for every civ, and comparing. Without it the split is a
convention; with it, it is checked.

Unciv cannot do this. It retrofitted threading with 21 `synchronized` blocks and
8 `ConcurrentHashMap`s inside `logic/`, one of them on the hot unique path.

Units and civs live in `Arena`s, so a dead unit's id stops resolving; tiles stay
a dense index. A full game loop runs: a grassland city reaches size 2 on turn 8,
which is the same answer the standalone growth maths gives by a different route.

## Construction and research

`world/construction.rs` and `world/research.rs` close the loop: tile yields feed
food, production and science; production builds; science unlocks what may be
built.

**Costs are not in the json.** Only 61 of 224 buildings state one — the rest
inherit their tech column's `buildingCost`, or `wonderCost` if they are a
wonder, and techs take `techCost` the same way. `Ruleset::derive` fills both in,
following Unciv (`Ruleset.kt:394`, `:618`), whose comment gives the reason:
editing a column's costs should move every building in it. The result is 80/80
techs and 119/124 buildings priced; the five left over are Palace, Cathedral,
Monastery, Mosque and Pagoda — none of which are produced.

Rejections are named rather than boolean (`NotUnlocked`, `AlreadyBuilt`,
`MissingBuilding`, `UniqueToAnother`, `NotProducible`), because the interface
has to answer in words and the AI chooses differently between impossible and
not-yet.

Two map-scaling constants live in Kotlin rather than json and are reproduced in
`MapScaling`. They are counterintuitive: a larger map makes each tech dearer but
*softens* the per-city penalty — 0.025 per city on Huge against 0.05 on Tiny —
because a bigger map means more cities. For nine cities that is a 20% surcharge
instead of 40%.

The tech tree is 18 layers deep and reachable in full from a standing start.

## Movement

`world/movement.rs` — the first search, and the first spatial decision.

Terrain cost comes from the *topmost* terrain on a tile (Unciv reads
`to.lastTerrain.movementCost`), consistent with how yields resolve: a forested
hill is a forest. Two movement crosses 19 tiles of open grassland and 7 of
forest.

**The clamp** (`UnitMovement.kt:133`) is the rule worth naming. If a step costs
more than a unit has left, the cost is reduced to exactly what remains rather
than refused — so a Warrior with one point can still enter a marsh costing
three, and simply ends its turn there. A naive "the cost must fit in the budget"
check gets this backwards and strands units on rough ground.

Search order is fixed. The frontier sorts by `(cost, TileId)` rather than
relying on a hash, because two machines walking the same map have to expand the
same tiles in the same order or their units end up in different places.

**Deferred:** roads and railroads, river crossings, zones of control,
embarkation. Each is a branch in `getMovementCostBetweenAdjacentTiles` that
needs state this crate does not carry yet; the cost function has the shape for
all of them.

## Founding cities

`world/settling.rs` — the first decision that looks at a *region* rather than a
tile, and the first that looks ahead: a settler is choosing what its citizens
will work for the next two hundred turns.

Sites are scored by summing the surrounding tiles with a weight of `3/(ring+1)`
out to ring two — the centre counts triple, its neighbours one and a half, the
ring beyond once — then subtracting 2.4 for every tile of that 19-tile hexagon
that is missing. That last term is what makes a map edge or a narrow peninsula
unattractive, and it is not subtle: an open site scores 67.2 where a corner
scores −0.8.

The flat bonuses are large relative to terrain. A hill is worth 14 against a
grassland tile's handful, which is why a hill site scores 80.0 to flat ground's
67.2. Civ rewards defensible, well-watered ground far more than fertile ground,
and the numbers say so plainly. Building over a resource costs 4, for the waste.

Nearby tiles are valued with the same weights that decide which tile a citizen
works, so the engine has one definition of "good tile" rather than two that can
drift apart.

**Deferred:** coast and river adjacency (+3 and +20), continent-aware minimum
distance, and the diplomatic agreement not to settle near a neighbour. Each
needs map or diplomacy state this crate does not carry yet.

## Combat

`world/combat.rs` — the system with the most rule surface, and the one place
where `ModifierStack` is the right tool rather than a forced one. Citizen
assignment was a dot product against fixed weights; defence genuinely composes,
and the answer must not depend on which bonus the engine applied first. A
Warrior on a hill is ×1.25; fortified as well it is ×1.45, not ×1.50 — Civ
stacks those additively, and the stack says so.

The damage curve is Civ V's, via Unciv (`BattleDamage.kt:335`):

```text
stronger = max(ratio, 1/ratio)
modifier = (((stronger + 3) / 4)^4 + 1) / 2
damage   = (24 + 12·random) · modifier · health_ratio
```

Evenly matched units trade exactly 30 at the median roll. Twice the strength
deals 51.600 and takes 17.430 — a threefold swing from a twofold advantage,
which is why Civ combat punishes attacking uphill so sharply. Wounded units hit
softer, one percent per three points of health missing, which is what makes
finishing a damaged unit worth a turn.

## Exploration and religious units

`world/ruins.rs` loads rewards from `Ruins.json`, including modded weights and
availability conditions. Movement resolves every entered tile, so a ruin on the
way to a destination is collected. Rewards update research, population pressure,
unit upgrades and map knowledge; a revealed map does not grant live sight.

`world/natural_wonders.rs` places wonders using terrain and spacing constraints,
supports connected groups such as the reef, and records discovery once per civ
and wonder. Discoveries grant happiness and ruleset bonuses. City output applies
Spain's natural-wonder tile multiplier before city building percentages.

Faith earns physical prophets. The first founds a religion and the second adds
an Enhancer and another Follower belief. Missionaries carry their birth city's
religion, use limited spread charges and suffer border attrition; inquisitors
block rival spread, remove heresy and suppress or restore holy-city pressure.
The AI buys and directs religious units through the normal plan/apply phases.
Partly used prophets cannot also construct a Holy Site.

The existing deterministic prophet-cost threshold is retained; this does not
implement Unciv's probabilistic prophet-birth timing or global religion cap.
The funded scenario in `tests/suite/religion_turns.rs` verifies the whole path from
stored faith to founding, enhancement, purchase and a missionary reaching a
distant city. It checks reachability without depending on the economic AI
earning enough faith in an ordinary short game.

## Randomness

`rng.rs` — splitmix64, seeded explicitly from the state that produced the roll
(turn, attacker tile, defender tile). There is no ambient generator in this
crate, because a thread-local or clock-seeded one makes the same action resolve
differently on the server and the client. A replayed log rolls the same numbers,
and `a_battle_replays_identically` asserts it.

Unciv seeds from game state too, but feeds `Vector2.hashCode()` into the seed —
and a JVM hash code is not a portable value. Here every part of the seed is an
integer whose meaning is fixed.

## Determinism, checked

```
./tools/check_determinism.sh
```

The script compares macOS/native, wasm32, Linux ARM64 and Linux AMD64, and
reports unavailable targets explicitly. The scenario embeds ruins, beliefs and
unit promotions so exploration rewards and religious actions participate in
the check. It also runs the funded two-city religion scenario described above;
the ordinary short game is not enough to guarantee that religion is exercised.

The scenario ends by playing a game: sixty turns of four civs and two
city-states on a map generated from a fixed seed, folding in every civ's
holdings, every city's and unit's state, and every directed influence value at
the end.

The city-states are there because they are what make quests, elections and
coups reachable, and those are the parts of the engine that actually roll
dice — an election is a weighted draw and a coup is a percentage. Sixty turns
rather than forty because no quest is offered before turn thirty.

That is the only check that covers the systems whose behaviour depends on state
rather than on the ruleset — combat, religious spread, city-state influence,
espionage's rolls — and the only one that would catch two machines diverging on
the third turn of an actual game rather than on an arithmetic edge case. The
map itself is folded in too: a map differing by one tile between two clients is
a game they cannot play together, and no differential test against Unciv can
catch that, because Unciv generates a different map on purpose.

Two architectures, three OS and libc combinations, two code generators. Each
target breaks something different: `wasm32` changes the code generator, Linux
changes the libm from Apple's to glibc, and `amd64` changes the instruction set
as well. A container leg runs under a watchdog and reports a skip rather than
hanging — Rust under QEMU user emulation has been seen to stall at ~0% CPU.

`src/fingerprint.rs` runs a fixed scenario over every layer — parsing,
fixed-point arithmetic, powers, modifier composition, yields, growth, movement,
combat, the generator — and folds each observable into one `u64`. The ruleset is
embedded rather than read from disk, because `wasm32` has no filesystem and
because that puts the parser inside what is checked.

If one bit of one calculation differs between targets, the numbers differ, and a
build catches it instead of a desynced multiplayer game forty turns in.

**The hazard is real, and now demonstrated.** Comparing macOS against wasm32
alone showed nothing, which was an honest negative but the wrong experiment: the
documented difference is between *platform libms*, and both of those resolve to
compiled-in code that happens to agree. Against glibc it reproduces at once. Of
199 inputs through `f64::sin`:

| input | macOS libm | glibc |
|---|---|---|
| `sin(22.94)` | `0xbfea0205b4d95137` | `…36` |
| `sin(51.43)` | `0x3fed65272c5a7c27` | `…28` |
| `sin(57.35)` | `0x3fe6fc2df43d8695` | `…94` |

Three of 199 differ, in the last bit of the mantissa. `powf`, `exp` and `ln`
agreed for these inputs and `sqrt` is IEEE-guaranteed — but one function is
enough. An engine using `sin` in its rules would desync a macOS client from a
Linux server on one call in sixty-six. This is why the arithmetic is
fixed-point.

## Differential against Unciv

Reading two implementations and concluding they agree is how subtle ports go
wrong. This reads Unciv's own answers instead.

```
cd ../Unciv
./gradlew :tests:test --tests '*DifferentialDump*'      # writes the reference
cd civ-core-rs && cargo test --test suite differential::
```

`tests/src/com/unciv/testing/DifferentialDump.kt` runs inside Unciv and dumps
what its own engine computes; `tests/suite/differential.rs` compares value by value
and reports *every* disagreement, not the first. Without the dump the tests skip
loudly, so the suite still runs on a machine with no JDK.

**1,811 values compared, 0 differences.**

Rules, at rest:

| | |
|---|---|
| building costs | 124 |
| tech costs / eras | 80 / 80 |
| era numbers | 9 |
| policy branches | 70 |
| terrain movement / defence / overrideStats / damage | 33 each |
| unit cost / movement / strength / sort group | 127 each |

And the rules being *run*, which is where a loop's off-by-one or a rounding
applied at the wrong moment shows up:

| | |
|---|---|
| tile yields, 12 terrain combinations | 12 |
| growth thresholds, populations 1–30 | 30 |
| **growth trace, 60 scripted turns** | **60** |

The trace is the valuable one. Sixty turns of feast and famine — crossing the
growth threshold repeatedly, carrying food over, starving back down — compared
turn by turn on population and stored food. A threshold that agrees at rest can
still disagree once it is being crossed with a remainder, and this is what would
catch the truncation bug found earlier if it were reintroduced.

Unciv's own modifiers are dumped alongside the trace (`speedModifier`,
`isHuman`) so both engines are driven from the same inputs rather than from an
assumption that they match.

Unit sort groups are the most valuable row: they exercise the whole chain —
unit, to required tech, to tech column, to era, to era number — against an
independently computed answer.

Two things the comparison found:

- **Unciv fills building costs in at game setup, not at ruleset load.**
  `updateBuildingCosts()` runs inside `getComplexRuleset()`, so a ruleset taken
  straight from `RulesetCache` still has `cost = -1`. Comparing against the
  wrong path would have looked like a total mismatch.
- **A policy branch is also a policy.** Unciv keeps branches in the same
  collection as the policies inside them and sets `branch.branch = branch`,
  because a branch is adopted before anything in it. This crate kept them apart,
  so "which policies can I adopt" had two answers. Fixed in the loader, not in
  the test.

Everything else that differed was spelling: Unciv marks "cannot be produced"
with `-1` and defaults `movementCost` to 1, where this crate leaves both absent.
The comparison normalises those explicitly rather than pretending either side is
wrong.

Production costs (124 buildings, 127 units), researched tech costs (80) and a
**375-row combat grid** are compared too — 5 melee types against 5 opponents, at
3 health levels and 5 fixed randomness values. Randomness is dictated rather
than rolled, so the grid tests the formula; the generator is checked separately
by the determinism fingerprint.

### What the combat grid found

Three defects, none of which a careful reading would have caught.

**A unique that was never implemented.** Circus Maximus costs 125 raw and 155 to
produce, with a difficulty modifier of 1. The difference is
`Cost increases by [30] per owned city` — how Civ stops national wonders being a
free win for whoever expanded most.

**A missing rule, then the rule.** A Swordsman defends at 10.5 rather than 14 —
`MISSING_RESOURCES_MALUS` is -25%, and the scenario's civ is one Iron short
because it fielded a Swordsman and owns no Iron. That is now modelled:
`world/resources.rs` tracks the net stock, territory less consumption, and the
grid is compared against **this engine's own strengths** rather than Unciv's.
A deficit is the trigger, not an absence — a civ with no Iron and no Swordsmen
is at zero, and zero is fine.

**Two precision bugs, from nine values out of 375.** All off by exactly one.
`Fixed` rounds to thousandths on every multiply, and the damage curve has six
steps; and the strength ratio itself was being quantised, so 8 against 10.5
became 0.762 instead of 0.761904… with a `^4` to magnify it. Both fixed by
[`Precise`](src/stats.rs), a Q32 intermediate for chains that must round once —
and by noting that an integer exponent wants repeated squaring, not `log2`/
`exp2`. 183 → 9 → 8 → 0.

## Resources

`world/resources.rs`. Owning a deposit is not the same as having the resource,
and the distinction decides whether your Swordsmen fight at full strength.

Three gates, in Unciv's order (`Tile.kt:511`):

1. the revealing tech must be researched — Iron Working for Iron. An unknown
   resource is invisible, not merely unusable;
2. on a **city centre**, the *possibility* of extraction is enough. Civ gives you
   what is under your city without asking you to build a mine on your own
   streets, provided some improvement exists that you could have built;
3. anywhere else the extracting improvement has to be standing, with its own
   tech researched.

A strategic resource then supplies its **whole deposit**, not one unit
(`CityResources.kt:205`) — which is why a single Iron tile can field several
Swordsmen. The stock is net: territory less what standing units consume, so the
penalty triggers on a *deficit* rather than an absence. A civ with no Iron and no
Swordsmen sits at zero, and zero is fine.

## Map geometry

Unciv addresses hexes as `HexCoord(x, y)`; this crate uses odd-r offsets on a
rectangle. The labels differ and the shapes differ, so tiles cannot be matched
one to one — what must agree is the geometry, and it does: rings of
**1, 6, 12, 18, 24**, six neighbours in the interior, three axes.

**A trap worth recording.** `HexMath.getRow`/`getColumn` look like a coordinate
mapping and are layout helpers. `row = (x+y)/2` truncates toward zero, so `x+y`
of -1 and of 0 both give row zero. Keying tiles by `(column, row)` merges **four
of sixty-one** on a radius-4 map — and merges them silently, leaving holes no
error would report. The geometry dump is keyed on raw `(x, y)` for that reason,
and the comparison says so.

Found by picking a stub site at random: `HexMath.getRow` is a one-line function
that looked like nothing.

## Next

1. **A second host.** The determinism check compares two targets on one machine;
   running it on Linux and Windows is what would close the gap noted above.
