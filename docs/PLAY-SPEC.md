# Play spec: a turn you play (M3)

Status: P1-P3 decided by Javed on 2026-10-13 (below). Builds on [VISUAL-SPEC.md](VISUAL-SPEC.md) (map, pieces, motion,
HUD) and [CLIENT-BRIEF.md](CLIENT-BRIEF.md) (Link, ViewState, Outbox). M3 built 2026-10-13; see "How M3 is built" below.

## What the screen has to say

In order of weight: which of your units still need orders, what the selected unit can do and where it can go,
what a battle will cost before you commit to it, what your cities are building and what you are researching,
and that the turn is yours to end.

## The match

A solo match against bots on `base`, 32x20, four majors, two city-states (civ-sim's defaults), you on seat 0.
The in-process host stages each command and answers with a receipt and the updates it caused; ending your turn
closes the round, and the round's update carries the other civs' turns as replay batches, which presentation
plays civ by civ before the new turn opens (VISUAL-SPEC, Motion).

## Layout

| Place | Content |
|---|---|
| Top bar, 40 px | Left: turn and year. Centre: what you research, its turns left (click opens the research list). Right: gold (total and per turn), science per turn, Undo |
| Bottom centre, 64 px tall | The selected unit: class glyph, name, health, moves left as "1.5 / 2"; then one button per action the unit has (from `unitOptions`), greyed with the reason when unavailable |
| Bottom right, 180 x 44 px | The turn button: "Next unit" while a unit awaits orders (selects it and glides to it), "End turn" when none do |
| Bottom left | The tile inspector, as in M1 |
| Right side, 320 px | The city panel when an own city is selected: name, population, food and production per turn, current production with turns left, then every construction choice (name, turns, gold price; a click sets it, a gold button buys it) |
| Centre, 420 px | The research list: each choice with its cost, progress and turns; a click sets it; Esc or a click outside closes it |

The spectator controls (speed, one round, eyes) leave the top bar while you play; presentation keeps 1x and skip
(K).

## Orders

- Selecting: a click on your unit selects it (a second click on the same tile cycles a stack); a click on your
  city opens its panel; Esc or a click on empty land clears the selection.
- Reach: the selected unit's reachable tiles (`reachable` query) get a white 18% fill and a 0.03 m outline;
  tiles it reaches only uncertainly (fog) get a dashed outline. The hovered target gets the 0.04 m hover line.
- Moving and attacking: see P1.
- Combat preview: hovering an enemy unit or city in attack range asks `combatPreview` and shows a 260 px card by
  the cursor: both sides' strength with their modifiers, health bars with the damage range shaded, "Captures" when
  the target would be taken.
- A command shows as pending only if its receipt takes longer than 150 ms (it is in-process; it should not).
  A rejection shows a toast 3 s at the bottom centre with the reason in plain words.
- Undo (Ctrl+Z or the button) takes back your last command when the engine allows it; a refusal says why
  ("it showed you new tiles", "a battle happened", "something else changed since").
- Keys: Enter = the turn button; F fortify, S skip, B found a city when the unit can; Esc clears; Ctrl+Z undo.

## How M3 is built

- `cargo client` plays (seat 0 against bots); `CIV_WATCH=1` watches. `Link::play` stages each command in-process
  and answers with its receipt and updates; the Outbox sends one command at a time with a stable id.
- `src/play.rs` asks the host for the selected unit's reach and options, the open city's choices, the civ's
  research and the combat preview, again after every view change, dropping answers to older questions. Clicks
  follow P1; highlights are a white 18% fill and 0.03 m outline (dashed where uncertain), red on targets, and a
  ring under the selected unit.
- `src/playui.rs` lays out the top bar, unit panel, turn button, city panel, research list, toasts and the combat
  card as above. Founding names the city from the nation's list.
- Presentation plays the round's replay batches (the other civs' turns) and then your own events.
- Verified: headless tests play a turn (found, research, end) and check retried and reused command ids; a
  scripted 32 s recording (`CIV_SCENE=turn`) plays one turn through the same selection and Outbox paths.
- Not verified by hand: the mouse and keyboard paths (the script drives the same state the clicks drive).
- Not built: units seen only in a replay are not drawn; promotions, upgrades and the other actions wait for
  M4 and M6.

## Out of M3

Policies, diplomacy, great people, religion and city tile management (M4); rooms (M5); saves and the Outbox
written to disk (with M5/M6, when there is a session to recover); the rest of the action catalogue (M6).

## Choices

Taken by Javed on 2026-10-13: P1 (b) left-click a target after selecting, a second click confirming an attack;
P2 (a) "Next unit" until no unit awaits orders, then "End turn"; P3 (a) side panels over the map.

The options as they were put:

P1. Orders by mouse. (a) Right-click on a reachable tile moves, on an enemy attacks (Civ V); left-click only
selects. (b) Left-click a target after selecting: one button, works on a trackpad and a touchscreen; a second
click confirms an attack. (c) Both: right-click acts at once; left-click on a target acts after the preview.

P2. The turn button. (a) "Next unit" cycles through units awaiting orders, then becomes "End turn" (Civ V). (b)
Always "End turn", with a count of units still awaiting orders beside it; ending with units idle asks once.

P3. Cities and research. (a) Side panels over the map, as laid out above. (b) A full-screen city view with the
city's tiles, and a full-screen tech list.

# M4: cities, research, policies and diplomacy

Status: Q1-Q4 decided by Javed on 2026-10-13: Q1 (a) branch trees, with civ-sim sending each policy's branch;
Q2 (a) city tiles on the map; Q3 (a) a full-screen tech tree; Q4 (a) a trade window. Built: see How M4 is built.

## What the screens have to say

What each city works and could work, and what a change would cost or bring; where research is going and what it
opens; which policies are open now and how they fit together; where you stand with each civ you have met, and
what they are offering you.

## Cities

- The city panel (M3) gains: focus (balanced, food, production, gold, science, culture, faith, and the two growth
  foci) as a row of toggles; avoid growth; specialists per building slot with + and - (sends `assignCitizens`);
  buildings to sell with their refund.
- City tiles: see Q2. Either way, each workable tile shows its yields as small figures (food, production, gold,
  science, culture, faith; zeros left out), worked tiles are marked, pinned tiles carry a pin, and tiles for
  sale show their price. Clicking a worked tile pins or unpins it (`lockTile`); clicking an unworked one works it
  in place of the weakest unpinned tile; clicking a tile for sale buys it after a second click.

## Research

- The tech tree from the rule tables (each tech's era, column and row, prerequisites and unlocks): see Q3. Each
  tech shows its name, cost, turns at the current science, and what it unlocks; researched techs are filled,
  the current one outlined, available ones bright, the rest dim. Clicking an available tech researches it;
  clicking a later one sets it and queues its missing prerequisites in order (by repeated `setResearch` as each
  completes).

## Policies

- Culture toward the next policy, its cost, and the policies open now (`civOptions`): see Q1 for how they are
  arranged. Clicking an affordable one adopts it (`adoptPolicy`) after a second click.

## Diplomacy

- A Civilizations panel (left side, 300 px, opened from the top bar): every civ met, with its colour, leader,
  score, war or peace, embassies, friendship, pact and research-agreement turns, and attitude; city-states with
  influence and protection. Buttons per civ: declare war, offer peace (or a peace treaty), friendship, research
  agreement, defensive pact, denounce, gift gold, trade.
- Trade: see Q4.
- Offers to you (`GameUpdate.offers`) appear as a card on the right with Accept and Decline (`answerOffer`);
  your own open offers list under the civ with Withdraw.

## How M4 is built

- civ-sim's rule tables carry `policyRows` (each policy's branch, era, column, row and prerequisites); nothing
  else in civ-sim changed for M4.
- City panel (`src/playui.rs`): focus toggles, avoid growth, specialists per building with + and - (one
  `assignCitizens` each, via `play::reassign`), buildings to sell with their refund, production and purchase as
  in M3.
- City tiles (`src/play.rs`, Q2 a): while a city is open its workable tiles show their non-zero yields; worked
  tiles are drawn in gold, pinned ones brighter, tiles for sale in green with their price. Clicks on the map act
  on tiles: a worked tile toggles its pin (`lockTile`), a free one is worked by an idle citizen, or by one
  taken from the weakest unpinned worked tile (`assignCitizens`), a tile for sale is bought on a second
  click (`buyTile`).
- Tech tree (`src/screens.rs`, Q3 a): full screen, scrollable, columns of 190 px and rows of 46 px from the
  rules, prerequisite lines, filled when researched, outlined when current, bright when available. Clicking a
  later tech stores the path of its missing prerequisites; `follow_path` sends the next `setResearch` each time
  research comes free.
- Policies (Q1 a): one column per branch, adopted policies filled, adoptable ones bright; adopting takes a
  second click.
- Civilizations panel: every civ met with colour, leader, score, war or peace, embassies, agreement turns and
  attitude; city-states with influence and protection. Buttons: declare war (second click), peace, friendship,
  research agreement, defensive pact, denounce, gift gold, pledge or withdraw protection, trade. Offers to you
  show as a card with Accept and Decline; your own open offers list with Withdraw.
- Trade window (Q4 a): two columns. You give: gold, gold per turn, your luxuries and strategic resources, open
  borders, embassy. They give: gold, gold per turn, open borders, embassy. Repeated clicks add to an item
  (`screens::add`, unit-tested); `offerTrade` sends it.
- Layering: screens on `GlobalZIndex` 20 over a 97% slate, side panels on 10, the top bar on 21, toasts on 30.
- Verified: 16 client tests, including working a free tile through the engine (`tests/play.rs`); a scripted
  36 s recording (`CIV_SCENE=tour`) founds a city, plays turns until it meets a civ, then shows the tech tree,
  the policies, the city's tiles, the civilizations panel and a drafted trade (not sent); native and wasm
  (webgpu) builds check clean.
- Not verified: the mouse paths by hand; every diplomacy button, offers and a sent trade (no test sends
  them or reads a bot's answer); the research path across several completions; tile pins and purchases.
- Not built: what the other side holds is not in your view, so "they give" cannot list their luxuries,
  strategic resources or cities; cities as trade items either way. Great people and religion were listed for
  M4 under Out of M3 but are not in M4's spec; where they go is open.

## Open choices (decided)

Q1. Policies. (a) Branch trees, as the rules group them; this needs civ-sim to add each policy's branch to the
rule tables it sends (a small, additive change). (b) A flat list of what can be adopted now and what has been,
with no tree.

Q2. City tiles. (a) On the map while the city panel is open: yields drawn over the city's tiles, clicks act on
tiles instead of units. (b) Inside the city panel as a small hex grid of the city's tiles.

Q3. The tech tree. (a) Full-screen, scrollable, laid out by the rules' columns and rows, with prerequisite lines.
(b) Keep the side list of M3 and add the tree as a toggle in it.

Q4. Trade. (a) A trade window: two columns, yours and theirs, items added from what each side has (gold, gold per
turn, luxuries, strategic resources, open borders, embassy, cities), sent with `offerTrade`. (b)
Only the fixed agreements above for now (peace, friendship, research agreement, pact, gift gold), with trade
later.
