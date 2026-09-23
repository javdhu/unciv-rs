# Content

What the game contains beyond the nations ([ROSTER.md](ROSTER.md)): eras, victories, techs, policies, religion, great people, wonders, buildings, units, city-states and names. Agreed 2026-09-23 in one design pass so the pieces fit together. Nothing here is built yet.

## Lenses

1. **Historical accuracy.** Things sit in the era they first appeared anywhere, carry their real names, and are balanced across regions (about 20% cap per region, as the roster).
2. **No obvious Civ terms.** Rename only terms a Civ player would recognise as Civ's own coinage; plain history words stay (see Names).
3. **Low player complexity.** Complexity goes into the engine, not the UI.

## Spine

The full game spans all of history at full length and ends in the present day; a future era can be added later. **30 turns per era**: about 240 turns for the full game, 60 for a Blitz slice.

| # | Era | Rough dates | Opens with |
|---|---|---|---|
| 1 | Age of Bronze | 4000–1200 BC | Bronze Working |
| 2 | Age of Iron | 1200 BC–AD 500 | Iron Working |
| 3 | Age of Paper | 500–1450 | Paper |
| 4 | Age of Sail | 1450–1750 | Navigation |
| 5 | Age of Steam | 1750–1900 | Steam Power |
| 6 | Age of Electricity | 1900–1945 | Electricity |
| 7 | Age of the Atom | 1945–1990 | Nuclear Fission |
| 8 | Age of Information | 1990– | The Internet |

Each era opens with its namesake tech: discovering it moves you into the era.

**Blitz plays a slice of eras**, not a compressed game. The engine's starting-era support sets up the start.

| Slice | Eras |
|---|---|
| Cradles | Bronze + Iron |
| Crossroads | Paper + Sail |
| Engines | Steam + Electricity |

Atom and Information stay out of Blitz for now. Slices play the same victories, scoped to their eras.

## Victories

| Victory | How it is won | Precedents |
|---|---|---|
| **Science — AGI** | The AGI tech unlocks **The Turing Project**; build its four parts in your cities: Compute (data centres; aluminium, energy), Data (libraries, universities, population), Models (research), Alignment. | Turing 1950, Dartmouth 1956, deep learning 2012, transformers 2017 |
| **Culture — Lingua Franca** | Your cultural influence over every rival exceeds that rival's own culture. Your language visibly spreads through their cities. | Aramaic, Greek, Latin, Arabic, Persian, French, English |
| **Diplomacy — Perpetual Peace** | The Internationalism tree unlocks a world charter; win a two-thirds ratification vote (city-states vote). | Westphalia 1648, Kant 1795, Vienna 1815, the UN 1945 |
| **Economy — Reserve Currency** | Your coin carries at least half of world trade for 10 turns. | Athenian owl, Spanish dollar, sterling, the dollar |
| **Conquest** | Hold every original capital. | |
| **Time — The Chronicle** | Highest score at the last turn. | |

No religious victory: a real faith "winning the world" is the likeliest thing to offend.

## Knowledge — the tech tree

About 83 techs, 8–12 per era (about 3 turns each). Changes from the Civ 5 tree:

| Change | Techs |
|---|---|
| Moved earlier | Astronomy → Iron · Gunpowder, Printing → Paper · Scientific Theory → Sail (as Scientific Method) · Railroads → Steam · Satellites, Lasers → Atom |
| Moved later | Electricity → Electricity era · Atomic Theory, Radar → Electricity · Nuclear Fusion cut (not yet achieved; future era) |
| Renamed | Chivalry → Stirrups · Physics → Algebra · Scientific Theory → Scientific Method · Metal Casting merged into Steel · Acoustics, Future Tech cut |
| New | Paper (Paper) · Firearms (Sail) · Telegraph (Steam) · Thinking Machines (Atom) · The Internet, Machine Learning, Genomics, AGI (Information) |

AI path: Computers → Thinking Machines → The Internet → Machine Learning → AGI.

## Society

### Policies

- Trees of five cards stay. Tradition, Liberty, Honor, Piety, Patronage, Commerce and Rationalism keep their names.
- The modern three become **Democracy / Socialism / Militarism**, with cards named for real institutions (Universal Suffrage, Planned Economy, Conscription, Total War). No "Fascism", "Communism" or "Police State" cards.
- New trees: **Internationalism** (Atom, backs Perpetual Peace) and **Openness** (Information, backs AGI and Lingua Franca). Electricity is left for later.
- Every victory has a tree: Science — Rationalism, Openness · Culture — Patronage · Diplomacy — Internationalism · Economy — Commerce · Conquest — Honor, Militarism.
- Cards name the best-known real instance from anywhere, each with an almanac entry. Mandate of Heaven moves to Tradition; Scholasticism → House of Wisdom; Reformation → Religious Reform; Philantropy spelled Philanthropy.

### Religion

Real faiths **arise in their historical era**; nations adopt them. No founding, naming or belief picking.

| Era | Faiths |
|---|---|
| Bronze | Hinduism |
| Iron | Judaism, Zoroastrianism, Buddhism, Jainism, Confucianism, Taoism, Christianity (late) |
| Paper | Islam, Shinto |
| Sail | Sikhism |

- A faith arises in its homeland nation if present, otherwise in the city with the most faith at that moment.
- Each faith has fixed traits from real teachings (zakat, monasteries, langar, hospitals), reviewed for stereotypes before shipping.
- The player's one choice is the state faith (one tap). Faiths spread along trade routes and between neighbours; **missionaries** stay as units you move. Inquisitors are cut.
- Christian-only belief names (Papal Primacy, Church Property, Messiah, Defender of the Faith) go with the belief picker.

## Culture

### Great people

- **Region first**: a nation draws from its own region's pool (8 pools), then the world.
- Real people only, in the right category; fabricated or legendary figures removed (Merit-Ptah, Xi Ling Shi), misfiled ones moved (Shah Jahan, Adam Smith), duplicates and typos fixed.
- Prophets become **Sages**: real teachers and scholars (Rumi, Adi Shankara, Kūkai, Hildegard). Religious founders never appear as units.

### Wonders

Significance first, ease of modelling second. About 37 wonders; Europe and North America fall from about 60% to about 35%. Every wonder sits in the era it was built.

| Change | Wonders |
|---|---|
| Cut | Hanging Gardens (→ Ishtar Gate), Leaning Tower of Pisa, Neuschwanstein, Pentagon, CN Tower, Brandenburg Gate, Apollo Program, United Nations |
| Fixed | Forbidden Palace → Forbidden City; each wonder re-gated to its build era |
| Added | Ishtar Gate (Bronze) · Nalanda (Iron) · House of Wisdom, Borobudur, Great Zimbabwe, Brihadeeswarar, Lalibela, Grand Canal (Paper) · Registan (Sail) · Suez Canal (Steam) · Panama Canal (Electricity) · CERN, International Space Station (Information) |

### Wonders and buildings change the map

- **A wonder takes a real tile**, chosen by the player with the best tile pre-highlighted (one tap). Placement follows geography: Pyramids on desert or floodplain, Machu Picchu beside a mountain, Great Lighthouse and Colossus on the coast.
- Some reshape the map: the **Great Wall** runs along border tiles and slows enemies; **Suez and Panama** let ships cross their land tile; the **Grand Canal** adds trade along its route; coastal wonders speed nearby ships.
- **Buildings are visible and placed**: libraries and markets in town, walls around it, a harbour on a coast tile, an aqueduct from a river or mountain. Terrain-bound buildings (harbour, aqueduct, observatory) change their tile; the rest keep city-wide effects. No districts.
- The client already places wonder art on neighbouring tiles (`web/src/world/plots.ts`); the engine must make placement authoritative.

## Power

### Units

| Change | Units |
|---|---|
| Re-gated | Caravel → Sail (Navigation) · Musketman → **Musketeer** (Firearms) · Gunpowder unlocks a **Bombard** |
| Renamed | Galleass → **Dhow** · Great War Infantry → **Trench Infantry** · Great War Bomber → **Biplane Bomber** |
| Cut | Giant Death Robot · Inquisitor · SS Booster, Cockpit, Engine, Stasis Chamber (replaced by the Turing Project parts) |

- Nuclear weapons stay and can be fired (Atom and Information; full game only).
- Barbarians become **Raiders** (camps unchanged).
- Each nation's unique unit and building is redone with the roster: achievements, not stereotypes.
- Combat unchanged: one unit per tile, preview → confirm → clash.

### City-states

24 real historic city-states, 3 per region, typed by what they were famous for, none a roster nation. Modern cities (Hong Kong, Vancouver, Kuala Lumpur) are out.

| Region | City-states |
|---|---|
| Europe | Venice (Maritime), Florence (Cultured), Sparta (Militaristic) |
| West Asia | Tyre (Mercantile), Byblos (Cultured), Ur (Religious) |
| Africa | Zanzibar (Mercantile), Mombasa (Maritime), Harar (Religious) |
| South Asia | Calicut (Mercantile), Taxila (Cultured), Varanasi (Religious) |
| East Asia | Sakai (Mercantile), Shuri (Maritime), Dunhuang (Religious) |
| Southeast Asia | Malacca (Mercantile), Hội An (Maritime), Bagan (Religious) |
| Steppe | Bukhara (Cultured), Kashgar (Mercantile), Sarai (Militaristic) |
| Americas | Tikal (Cultured), Tlaxcala (Militaristic), Cahokia (Religious) |

## Names

Only obvious Civ coinages change; the rest stay (City-State, Golden Age, policy tree names, Open Borders).

| Civ term | civ.game |
|---|---|
| Great People / Great Person | Luminaries / Luminary |
| Great Prophet, Scientist, Engineer, Merchant, Artist, General, Admiral | Sage, Scholar, Master Builder, Magnate, Virtuoso, Marshal, Admiral |
| We Love the King Day | Feast Days |
| Declaration of Friendship | Treaty of Friendship |
| Research Agreement | Scholarly Exchange |
| Settler … Deity | Pupil, Student, Graduate, Tutor, Lecturer, Professor, Dean, Chancellor |
| civilization (in copy) | nation |

## Engine work this implies

| Work | Size |
|---|---|
| New ruleset (eras, techs, policies, wonders, units, city-states, great people, roster) | largest content job; days |
| Religion arising by era, fixed traits, no belief picker | medium |
| Wonder tile placement and map effects (walls, canals); placed buildings | about a week |
| Influence system for Lingua Franca | about a week |
| Reserve Currency | medium, new |
| Perpetual Peace charter vote | small, reuses the vote |
| Turing Project | small, reuses the spaceship project |
| Leader abilities split from nation uniques (two leaders) | medium |

## Open

- The new ruleset **replaces** `crates/civ-core/rules/gods-and-kings` (decided 2026-09-23). Build order still to agree.
- 30 turns per era and the three Blitz slices were proposed and not vetoed; confirm before balancing.
- Policy card lists, faith traits, unique units and buildings, and regional great-people pools are content still to write.
- Parked: `web/src/content/labels.ts` and its HUD wiring (uncommitted) implement the Names table at display time; keep, fold into the ruleset, or revert.
