# State architecture

Research note for `civ-core`. Findings are marked **[spec]** (normative source),
**[measured]** (counted in this repo), or **[inference]** (my reasoning from the
other two). Dated 2026-09-21.

---

## 0. The determinism budget

Everything else follows from what is actually guaranteed, and the common
framing — "floats differ between wasm and native" — is wrong in a way that
matters.

**[spec]** WebAssembly mandates IEEE-754, so `+ − × ÷ sqrt` produce identical
bits on every target Rust supports (x86-64 SSE2, aarch64, wasm32). Wasm's own
list of nondeterminism sources is short: NaN bit patterns, NaN sign bits,
relaxed SIMD, shared-memory concurrency, feature-support variation, and resource
exhaustion.[^wasm-nd]

**[spec]** What is *not* covered by IEEE-754: `sin`, `cos`, `exp`, `ln`, `powf`,
`cbrt`, `hypot`. These come from the platform libm and differ in the last ulp
between macOS, glibc, MSVC and wasm.[^libm] `mul_add` is also unsafe — it fuses
to one rounding on FMA hardware and two elsewhere.

**[measured]** This is not theoretical for Civ. The core growth and expansion
formulas use **fractional** exponents:

| formula | source |
|---|---|
| `6 * (tilesClaimed + 1.4813).pow(1.3)` | `CityExpansionManager.kt:46` |
| `15 + 8*(pop-1) + floor((pop-1).pow(1.5))` | `CityPopulationManager.kt:53` |
| `(techsPercentKnown * mult).pow(exponent)` | `CityCombatant.kt:59` — exponent is *ruleset data* |

53 `pow` and 40 `sqrt` call sites across `logic/` and `models/`; 294 `: Float`
declarations against 733 `: Int`.

**[inference]** So fixed-point was the right call, but for a narrower reason
than I first gave: the hazard is `powf`, not arithmetic.

**Resolved — and the first answer was wrong.** I originally recommended
precomputing these into tables at ruleset load. That does not work: a table
computed with the platform's `powf` differs between machines, so the
nondeterminism just moves from the runtime into the table. Whatever fills the
table has to be deterministic, at which point tables are an optimisation rather
than the fix.

`Fixed::pow` is now built from integer operations only (`stats.rs`):
`x^y = 2^(y·log2 x)`, with `log2` by repeated squaring and `exp2` by one
constant per set fractional bit, those constants being exact integer square
roots of 2. Internals are Q32 in `u128`, so there is a single rounding back to
thousandths. No float appears anywhere in the path, which makes it bit-identical
by construction rather than by assumption — and it handles `CityCombatant`,
where the exponent is ruleset data and so cannot be specialised in advance.

**[measured, 2026-09-21]** The claim is now checked rather than asserted.
`src/fingerprint.rs` runs a fixed scenario across every layer and folds each
observable into one `u64`; `tools/check_determinism.sh` compares native against
`wasm32`. They agree: `0x8de2f07f5a65d0f8` on both.

**[measured] The hazard, reproduced.** Comparing macOS against wasm32 alone
showed no divergence — an honest negative, but the wrong experiment: the
documented difference is between *platform libms*, and both of those targets
resolve to compiled-in code that happens to agree. Run against glibc in a
container and it appears at once. Of 199 inputs through `f64::sin`, **three
differ in the last bit of the mantissa** — `sin(22.94)` is `0x…37` on macOS and
`0x…36` on glibc. `powf`, `exp` and `ln` agreed for these inputs and `sqrt` is
IEEE-guaranteed, but one function is enough: an engine using `sin` in its rules
would desync a macOS client from a Linux server on one call in sixty-six.

The check now covers four targets — macOS/aarch64, wasm32, linux/arm64 and
linux/amd64 — and they are bit-identical.

What the check does establish is narrower and still worth having: every
integer-and-fixed-point path in the crate — parsing, powers, modifier
composition, growth, yields, movement, combat, the generator — produces
identical bits on two different instruction sets and two different code
generators.

---

## 1. State representation: not ECS

**[measured]** Unciv's `TileMap` keeps both a `tileList` and a `tileMatrix`,
carrying the comment: *"this works several times faster than a hashmap, the
performance difference is really astounding"* (`TileMap.kt:93`). Dense indexed
access is the measured winner in the original engine.

**[measured]** What the AI actually reads, by call site across `logic/automation/`:

| call | sites |
|---|---|
| `hasUnique` | **122** |
| `aerialDistanceTo` | 62 |
| `getTilesInDistance` | 37 |
| `getOwner` | 36 |
| `neighbors` | 18 |
| `getMatchingUniques` | 12 |

**[inference]** Archetypal ECS (bevy_ecs, hecs) earns its keep when component
sets *vary* and you iterate over subsets. Every Civ tile has the same component
set — terrain, features, resource, improvement, owner — so there is effectively
one archetype, and the archetype machinery is pure overhead. Bevy's own docs note
it "is not optimized for use in worlds with a very large number of archetypes",
which is the opposite problem but the same warning: the structure has to match
the data.[^bevy] It would also add a dependency to a crate kept dep-free
specifically so its determinism is auditable.

**Recommendation.** Structure-of-arrays over a dense grid, plus the arenas that
already exist:

```
TileId(u16)        dense index, 0..w*h — 10,240 tiles fits u16 with room over
  terrain:     Vec<Id<Terrain>>
  feature:     Vec<SmallVec<[Id<Terrain>; 2]>>
  resource:    Vec<Option<Id<TileResource>>>
  improvement: Vec<Option<Id<TileImprovement>>>
  owner:       Vec<Option<CivId>>
  neighbours:  Vec<[Option<TileId>; 6]>   // precomputed once
```

**[inference]** A refinement to what is already built: **tiles should not use
`Arena`**. Tiles are never created or destroyed, so the generation counter is
dead weight and `TileId` should be a plain dense index like `Registry` uses.
`Arena`'s generations are exactly right for units and cities, which do come and
go. The distinction is worth keeping sharp — paying for generations where
nothing dies is the mirror of the bug they prevent.

Precomputing `neighbours` once turns the 18 `neighbors` and 37
`getTilesInDistance` call sites into array walks with no coordinate maths.

---

## 2. The `hasUnique` fast path

**[measured]** `UniqueMap` holds a `HashMap<String, ArrayList<Unique>>` *and* an
`EnumMap<UniqueType, ArrayList<Unique>>` sharing the same lists, with a comment
explaining the memory tradeoff: *"there are \*600 unique types\*, 750 including
deprecated, and EnumMap creates a N-sized array"* (`UniqueMap.kt:5`). At 638
types that is ~5 KB of pointers per object that owns a unique map.

**[inference]** `hasUnique` is a *presence* question 122 times over, and presence
fits in a bitset. 638 types is 80 bytes:

```
struct UniqueIndex {
    present: [u64; 10],       // 638 bits — hasUnique is a bit test
    entries: Vec<Unique>,     // sorted by type, for the 12 sites that need payloads
}
```

80 bytes against ~5 KB, and an O(1) bit test against a hash lookup. This is the
single highest-leverage state decision, because it is the hottest read in the
engine by a factor of two.

---

## 3. Fog of war and per-player views

**[measured]** Unciv stores exploration as `private var exploredBy = HashSet<String>`
**on each Tile** (`Tile.kt:54`) — a hash set of civ ID strings per tile. It also
keeps `lastSeenImprovement` on each Civilization (`Civilization.kt:228`), so fog
is not a visibility mask but a per-civ *memory* of how the world looked when last
observed.

**[inference]** Inverted, this is far cheaper: one bitset per civ over dense tile
indices. 10,240 tiles is **1.28 KB per civ**, against 10,240 hash sets. Visibility
becomes a bit test, and "which tiles can this civ see" becomes a word-at-a-time
scan.

The memory model still needs its own storage — a sparse per-civ overlay of
remembered improvements, cities and owners. Keep it sparse: it is small, and
making it dense would cost more than the bitset saves.

Because this is server-authoritative, **view filtering is the security boundary**,
not a rendering concern. Derive each player's view once per turn from
`(world, civ) -> PlayerView` and send deltas of that. A filter applied per-query
will eventually be forgotten at one call site, and that call site leaks the map.

---

## 4. Event sourcing and snapshots

**[inference]** State as a fold over an action log gives replays, spectating and
desync forensics for free, and turn boundaries are natural checkpoints. Unciv's
save files reach 32 MB compressed, but that is a whole-game blob; a single turn's
actions are kilobytes.

Persistent/immutable structures (`im`, `rpds`) are tempting for cheap snapshots
and **not worth it here**. They buy O(1) structural sharing, but turn-based play
snapshots at most once per turn per player, and a full world is single-digit
megabytes. Plain `Clone` on an SoA grid is a memcpy — faster in practice than
pointer-chasing through shared trees, and vastly easier to keep deterministic.

---

## 5. Parallelisation

**[measured]** Unciv retrofitted threading: 21 `synchronized` blocks, 8
`ConcurrentHashMap`s and 1 `@Volatile` across `logic/`, including
`tileUniqueMapCache` on the hot unique path. That is the shape to avoid.

**[spec]** Rayon does not specify reduction order, so non-associative operations
give non-deterministic results.[^rayon] Float addition is non-associative;
**integer addition is associative**, so fixed-point sums parallelise with
bit-identical results for free. One caveat: `saturating_add` is *not* associative,
so a reduction that saturates is order-dependent. Civ magnitudes never approach
`i64` saturation, but the sum path should use checked arithmetic and treat
overflow as a bug rather than clamping silently.

**Recommendation — phase separation, not locks:**

1. **Read phase, parallel.** Every AI civ reads the same immutable world and
   emits a proposed action list. No mutation, so no synchronisation.
2. **Apply phase, sequential, in civ-id order.** Deterministic by construction,
   and cheap — applying actions is trivial next to deciding them.

The 8k lines of automation live entirely in phase 1. This also preserves
single-threaded replay: applying the log without the parallel read reproduces the
same state, which is what makes the differential harness against Kotlin possible.

---

## 6. Expansion and modding

**[measured]** 906 named ruleset objects carrying **470 distinct tags** (type,
unitType, era, resourceType, beliefType, plus normalised unique templates).

**[measured]** 185 `@Transient` fields across 43 files, with 28 `setTransients()`
functions. This is the derived-cache problem at full scale; `Ruleset::derive`
currently handles four.

**[inference]** Two rules, both learned the expensive way already:

- **Derived state gets one pass, declared in one place.** The `DERIVED` table plus
  `Ruleset::derive` should stay the only route. 185 ad-hoc transients is what
  happens without that discipline, and it is why Unciv needs a rebuild step after
  every load.
- **Uniques stay data.** The 638 types are a closed algebra with a validated
  parser and an order-independent `ModifierStack`. A mod adding behaviour should
  compose existing uniques; new `UniqueType`s are engine changes and should feel
  like it.

---

## 7. Content authoring

**[measured]** All **2,459** atlas regions carry `index: -1`. libGDX's atlas
format supports animation frame indices; Unciv uses none. Animation is greenfield,
and the format does not need changing to add it.

**[measured]** Tilesets are already a complete art-swap interface — the same
region names rendered in two full styles:

| tileset | tiles | units |
|---|---|---|
| HexaRealm | 341 | 152 |
| FantasyHex | 160 | 152 |

**[inference]** This is the hook for generated content, and it already exists. An
AI-generated art pack is another `TileSets/<Name>/` folder with the same region
names — no engine change at all.

The generated `Sprite` enum makes completeness *mechanically checkable*: "what
does a full tileset need?" is a set difference against the 2,441 known regions,
so a generated pack can be gated by a coverage test exactly like
`ruleset_objects_have_packed_icons` gates the shipped one.

For tag-driven generation, the 470-tag vocabulary is the conditioning input, and
the ruleset already joins to art by name (`BuildingIcons/Alhambra` ↔ building
`Alhambra`) — a relationship this crate proved holds for **660/660** objects. So
the generation contract is well-defined in both directions: tags in, named region
out, coverage verifiable.

Worth stating plainly: generated art needs a **provenance field** in the pack
manifest. Style consistency across 341 tiles is the hard part of this problem,
and knowing which assets came from which model and prompt is what makes a
regeneration reproducible rather than a fresh roll of the dice.

---

## Summary of recommendations

| decision | choice | why |
|---|---|---|
| tile storage | dense SoA, `TileId(u16)`, no generations | one archetype; Unciv measured hashmaps slower |
| unit/city storage | existing generational `Arena` | these are created and destroyed |
| `hasUnique` | 638-bit bitset + sorted payload vec | 122 call sites; 80 B vs ~5 KB |
| fog of war | per-civ bitset over tile indices | 1.28 KB/civ vs a hash set per tile |
| player views | derived once per turn, server-side | it is a security boundary |
| snapshots | plain clone of SoA | persistent structures do not pay off at turn cadence |
| `pow` | integer `2^(y·log2 x)`, Q32 internals | platform libm is not bit-reproducible; tables would inherit the same problem |
| parallelism | parallel read, sequential apply in id order | integer sums are associative; locks are not needed |
| ECS | no | uniform component set, and a dependency costs auditability |

### Open

- Whether `saturating_add` should be replaced by checked arithmetic throughout.
  It is not associative, so a saturating reduction is order-dependent — harmless
  at Civ magnitudes, but it weakens the parallel-sum guarantee above.
- The grid uses odd-r offset coordinates. Unciv's own encoding differs; the
  mapping between them has to be pinned before the differential harness can
  compare tile-indexed state.

### Closed since writing

- `Fixed::pow` — see §0.
- Tile storage, fog masks and the memory model — implemented in `src/world/`.

[^wasm-nd]: [WebAssembly/design — Nondeterminism.md](https://github.com/WebAssembly/design/blob/main/Nondeterminism.md)
[^libm]: IEEE-754 covers `+ − × ÷ sqrt` only; transcendentals come from platform libm and differ in the last ulp. See [Gaffer On Games — Floating Point Determinism](https://gafferongames.com/post/floating_point_determinism/) and [ALICE-DetMath](https://github.com/ext-sakamoro/ALICE-DetMath), which exists to provide bit-exact transcendentals built from basic ops.
[^bevy]: [bevy_ecs documentation](https://docs.rs/bevy_ecs/latest/bevy_ecs/) and [Bevy archetype internals](https://github.com/bevyengine/bevy/blob/main/crates/bevy_ecs/src/archetype.rs)
[^rayon]: [rayon::iter::ParallelIterator](https://docs.rs/rayon/latest/rayon/iter/trait.ParallelIterator.html) — reduction order is unspecified, so non-associative operators are not deterministic.
