# Originality and history: what to change in the inherited content

Status: exploration, 2026-09-23. Nothing here is decided. It proposes changes to the civs, leaders, names and
text the engine inherited from Unciv's Gods & Kings rules (`crates/civ-core/rules/gods-and-kings/`), for two
goals that one rewrite pass can serve together:

1. **Originality.** PLATFORM.md §5 already commits to keeping the mechanics and rewriting
   "every civ, unique ability, name and line of text". Today almost every name is Civ V's: ability names,
   unique units, policy trees, wonders and city-states.
2. **Historical accuracy and breadth.** Long-running Civ community discussion (forums, reviews, the
   reaction to each game's roster) returns to the same complaints. The inherited data has most of them.

The community themes below are a summary of recurring discussion, not a fresh survey. Before we commit to
the launch list, gather sourced examples for the ones we act on.

## What players keep asking for

| Theme | What players say | What it means here |
|---|---|---|
| **Actual rulers** | Leaders who never led a state (Gandhi) or whose inclusion is mostly a meme draw steady criticism. Civ VII's non-ruler leaders split the community. | Heads of state or of a recognised polity only. PLATFORM.md already rules out the 20th century and religious founders. |
| **Eurocentrism** | Europe and the Mediterranean get many slots and the rest of the world one each, often late or in DLC. | The launch 8 has no civ from sub-Saharan Africa, the Americas or Oceania (below). |
| **Monoliths** | One "Celts", "Polynesia" or "Native American" civ merges distinct peoples; one civ spanning 6,000 years yields modern city names beside ancient leaders. | Name a specific polity and period. Take city names from the leader's era. |
| **Stereotype abilities** | Non-European civs often get "nature", "spiritual" or "savage warrior" bonuses; European ones get science, trade and navies. | Base each ability on a documented institution or achievement. |
| **Consulting living peoples** | Civ VI's Cree leader, Poundmaker, drew objections from members of the Poundmaker Cree Nation. | For a living Indigenous nation, consult before shipping, or prefer an ancient polity. |
| **Native languages** | Civ V's leaders speaking their own languages is still one of its most praised features. | PLATFORM.md §11 already lists "the historical language with subtitles" as open. This is the case for it. |
| **"Barbarians"** | The term flattens peoples outside the player civs into enemies. Civ VII renamed them "Independent Powers". | Rename the faction and its units (Brute, Marauder). |

## The launch roster

A from-scratch alternative that ignores these selections is in [ROSTER.md](ROSTER.md).

Current 8 (roster options v3 (not published)): Egypt–Cleopatra VII,
Rome–Marcus Aurelius, England–Elizabeth I, China–Wu Zetian, Mongolia–Genghis Khan, Persia–Cyrus the Great,
India–Ashoka, Greece–Alexander the Great.

- **Coverage.** Four are Mediterranean or European, four are Asian. None are from sub-Saharan Africa, the
  Americas or Oceania.
- **Two Macedonians.** Cleopatra (a Ptolemaic, Macedonian-Greek dynasty) and Alexander both come from Macedonian
  dynasties. Cleopatra was a user selection, so it stays; the overlap is an argument for looking again at the
  Greece slot.
- **Three women** (Cleopatra, Elizabeth, Wu Zetian). This is good and a swap should keep it.

Options, from least to most change:

| Option | Change | Why |
|---|---|---|
| A | Greece → **Mali–Mansa Musa** | Content already exists (`content/leaders/mansa-musa/`, previous launch draft). Removes the second Macedonian. Adds sub-Saharan Africa and a trade and scholarship civ. |
| B | A, plus England → **Inca–Pachacuti** or **Mexica–Itzcoatl/Moctezuma I** | Adds the Americas. The Inca had roads, terraces and khipu administration; the Mexica had a triple alliance, chinampas and a planned island city. |
| C | Grow to 10: A and B while keeping Greece and England | Nothing is dropped, but it costs two more leader media sets. |

Other swaps worth a look: **Hatshepsut** for Egypt (native dynasty, strong monuments, keeps a woman),
**Kamehameha I** as **Hawaiʻi** rather than "Polynesia", and **Ewuare** of **Benin** or **Menelik II** of
**Ethiopia** for Africa.

**Recommendation:** Option A now, because it costs almost nothing. Decide B or C when the leader
pipeline's real per-leader cost is known.

## The engine's other 26 civs

Bots can still pick any Gods & Kings nation. Either ship `rulesets/base/` (PLATFORM.md §4) with only the
launch civs and rewritten city-states, or fix these. The list is the accuracy problems visible in
`Nations.json`, `Units.json` and `Buildings.json`, not an exhaustive audit.

| Civ | Problem | Suggested fix |
|---|---|---|
| India | Gandhi never led a state. Cities mix Mauryan, Mughal and modern names (Mumbai, Calcutta, Bangalore). Mughal Fort is 1,800 years after Ashoka. | Ashoka (already chosen). Mauryan cities: Pataliputra, Ujjayini, Takshashila, Tosali, Suvarnagiri, Vidisha, Kaushambi. A Mauryan unique building. |
| China | Wu Zetian leads Beijing, Shanghai, Tianjin, Macau and "Shandong" (a province). | Tang/Zhou cities: Chang'an, Luoyang, Yangzhou, Chengdu, Guangzhou, Kaifeng (Bianzhou), Taiyuan. |
| Persia | Typo "Parsagadae". Cyrus is replacing Darius. | Pasargadae first (Cyrus's capital), then Ecbatana, Susa, Babylon (conquered 539 BCE), Sardis. |
| Egypt | "Burial Tomb" and "Monument Builders" are Civ V verbatim. | Rename. Choose cities for the chosen leader's era (Alexandria first for Cleopatra). |
| Ethiopia | Haile Selassie is 20th-century and a figure of Rastafari veneration, so he breaks both roster rules. | Menelik II (already listed as the replacement) or Ezana of Aksum. |
| Celts | A monolith: Boudicca (Iceni, 1st-century Britain) with modern Edinburgh, Dublin, Cardiff, Glasgow and a *Pictish* unique unit. Edinburgh and Dublin are also city-states. | Make it a specific people and period: the **Iceni/Britons** under Boudicca with sites in Iceni territory (Venta Icenorum), or **Gaul** under Vercingetorix (Gergovia, Alesia, Bibracte). |
| Polynesia | Kamehameha (Hawaiʻi) with "cities" Samoa, Tonga, Tuvalu, Aotearoa (island nations and countries), a Māori unit and Rapa Nui's moai. | **Hawaiʻi** with Hawaiian places, or **Tonga** (the Tuʻi Tonga maritime empire). Keep Wayfinding. It is the best ability in the set. |
| Iroquois | An exonym. Hiawatha co-founded a confederacy that has no single head. Typo "Akwesasme" (Akwesasne). Colonial town names (Montreal, Cuba, Akron). | **Haudenosaunee** only after consultation, otherwise drop it. The Great Law of Peace suggests a diplomacy and alliance ability, not forest movement. |
| Aztecs | The people called themselves Mexica. "Sacrificial Captives" reduces a civ to sacrifice. Teotihuacan had been abandoned for centuries. | **Mexica**, Motecuhzoma I. An ability built on chinampas, the Triple Alliance or tribute logistics. |
| Songhai | Its unique unit, Mandekalu Cavalry, is named for Mali's Mande. Typo "Motpi" (Mopti). | Rename the unit (Songhai cavalry). Fix the spellings. |
| Mongolia | "Hsia" (old romanisation of Xi Xia, a conquered state) and Turfan are in its city list. | Karakorum, Avarga, Khara Khoto (captured), plus Mongol-era names. |
| Japan | Oda Nobunaga with Tokyo (named 1868) and a WWII Zero fighter. | Period names (Azuchi, Kyoto, Sakai, Edo). Replace the Zero, which is sensitive in East Asian markets. |
| Arabia | Harun al-Rashid ruled from Baghdad, but the list starts with Mecca and Medina. The Oil bonus is an anachronism. | **Abbasids**, with Baghdad first. An ability built on the House of Wisdom, translation or trade. |
| Germany | "Furor Teutonicus" is a Roman insult. Panzer is a WWII unit. | Rename both. |
| America | B-17 (WWII). "Manifest Destiny" celebrates expansion over Indigenous land. | Rename the ability, replace the unit. |
| Sweden | Its ability "Nobel Prize" dates from 1901 under a leader who died in 1632. Helsinki is also a city-state. | A period ability name. Drop the duplicate city. |
| Siam | Ramkhamhaeng was king of Sukhothai; Naresuan's Elephant is from Ayutthaya, 300 years later. | Choose one period. |
| Maya | A single "Maya" empire mixes Classic Palenque with Postclassic Chichen Itza. | Name the city-state (Palenque/B'aakal) or keep the civ but order cities by era. |
| Inca | "Machu" (a cut-off Machu Picchu). Tiwanaku fell 400 years before the Inca. | Fix the names. |
| Byzantium | An exonym invented later. | **Eastern Rome** / *Rhōmaíōn*, or keep "Byzantium" with a note. It is recognisable, so this is low priority. |

## City-states

47 city-states. Some problems:

- **Collisions.** Helsinki, Edinburgh and Dublin appear both as city-states and in a civ's city list.
- **Colonial-era or modern cities.** Sydney, Vancouver, Quebec City, Rio de Janeiro, Cape Town, Hong Kong,
  Singapore and Kuala Lumpur. A game that starts in 4000 BCE can use older polities.
- **Living holy sites as capturable religious city-states.** Jerusalem, Vatican City, Lhasa and Wittenberg.
  Jerusalem in particular is politically charged in any conquest game.

Candidates that widen coverage and suit the period: Kilwa, Great Zimbabwe, Ife, Benin City, Aksum, Malacca,
Srivijaya (Palembang), Hormuz, Samarkand, Bukhara, Anuradhapura, Thăng Long (for Hanoi), Chan Chan, Cholula,
Byblos, Uruk. Keep the good ones: Ur, Tyre, Sidon, Cahokia, La Venta, Venice, Genoa, Ragusa, Zanzibar,
Mombasa, Kathmandu. For the religious type, use sites of past learning and pilgrimage rather than contested
present-day holy cities: Nalanda, Ife, Cholula, Anuradhapura.

## Wonders

54 wonders and national wonders, heavy on Europe and the modern West. Keep the effects and change the names and places:

- **Replace nation-specific modern projects** with generic names: Manhattan Project → *Atomic Research*,
  Pentagon → a period arsenal (the Arsenale of Venice), Apollo Program → *Space Program*,
  United Nations → *World Assembly*.
- **Drop Great Firewall.** It is a present-day censorship system and a market risk in China. Its effect
  (spy defence) can go on a cipher-bureau wonder.
- **Check image rights on modern landmarks.** Some carry them (the Eiffel Tower's night lighting is under
  copyright; Sydney Opera House restricts commercial use of its image). Prefer landmarks built before 1900.
- **Add underrepresented ones** by swapping effects: Great Zimbabwe, Borobudur, the churches of Lalibela,
  Sankore at Timbuktu, the walls of Benin City, Göbekli Tepe, Ellora, Tikal, Hampi, the Great Mosque of Kilwa.

## Names that are Civ V's, not ours

These need new names even where the history is fine. The mechanics stay.

- **Unique ability names**: all 34 (Monument Builders, Sun Never Sets, Bushido, Seven Cities of Gold…).
- **Policy trees**: Tradition, Liberty, Honor, Piety, Patronage, Commerce, Rationalism, Freedom, Autocracy,
  Order (Civ V's exact set and order). Choose our own names, perhaps period-neutral ideas such as
  *Kinship, Charter, Valour, Devotion, Alliance, Markets, Inquiry*, and have the three ideologies describe
  what they do instead of borrowing Civ V's labels.
- **Late-game terms**: World Congress, Utopia Project, Great Admiral/General/Scientist. The generic ones can
  stay; the distinctive ones should change.
- **Unique unit and building choices**: the same set as Civ V for each civ. Choosing different
  documented units is originality and history at once. Examples: Mauryan war elephants stay, and Mali gets
  Mande archers or its heavy cavalry instead of a unit copied from a Civ roster.
- **All introductions, greetings and quest text**, as PLATFORM.md already plans.

## Ideas that would make the game its own

Not required for launch, but they come straight from the complaints above:

1. **Names that change with the era.** Cities are founded under period names and rename when the owner reaches
   the right era: Edo → Tokyo, Byzantion → Constantinople → Istanbul, Thăng Long → Hà Nội. The game shows
   history moving without Civ VII's civ switching, which is what split players.
2. **Sources on the card.** Every leader and wonder card gets a one-line "What we know" with the
   attested / reconstructed / invented split the leader bibles already record
   (PLATFORM.md §7). No mainstream 4X does this, and it is ready-made from our pipeline.
3. **Abilities from institutions.** Each civ's ability names a real institution or practice (khipu
   accounting, the House of Wisdom, the Great Law of Peace, Polynesian wayfinding) and does what it did.
4. **The leader's language by default**, English subtitles on. It pairs with the phone-call premise.
5. **"Independent peoples", not barbarians.** Camps can be bargained with or hired as well as cleared.
   This rewrites Unciv behaviour, so it is post-launch.

## Decisions needed

1. Roster: Option A, B or C (or keep the current 8)?
2. Scope: ship a trimmed `rulesets/base/` with only launch civs, or fix all 34 engine civs?
3. Religions: keep the 11 real religions (educational, as Civ does, with custom names allowed) or abstract
   them? Either way, keep holy-war flavour out of the text.
4. Should Byzantium and Iroquois keep their familiar exonyms, with a note?
5. Should the game say BCE/CE or BC/AD?
6. Which ideas (1–5 above) belong in the launch scope?
