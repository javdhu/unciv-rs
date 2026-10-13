# Play quality spec (M7)

Status: K1-K6 decided by Javed on 2026-10-13, and the HUD arrangement below confirmed by him as written; M7
built (How M7 is built, at the end); the playtest loop is on hold at Javed's call (2026-10-13).

| | Choice | Pick |
|---|---|---|
| K1 | Tooltips | What each thing is, plus where numbers come from: civ-sim adds yield breakdowns to the view |
| K2 | Keys | A small set of this client's own, listed in the key map |
| K3 | Minimap | The whole HUD arranged like Civilization VI's, minimap included (layout below) |
| K4 | Units | Next and previous, plus a units panel |
| K5 | UI scale | Automatic from window size and display density, with a setting |
| K6 | Playtest issues | Written in chat |

The options not picked stay at the end for the record.

## Problem

M7 is everything a player meets in their first hour that the screen does not yet explain:
- what each number means;
- which keys do what;
- where things are on a map larger than the window;
- which units still need orders;
- text that stays readable on small and large windows.

The milestone ends with a loop where Javed plays and lists what feels wrong, and the fixes inside existing designs follow.

## What there is now

- **Mouse:**
  - The wheel or a pinch zooms toward the cursor; a drag pans.
  - A click selects, aims or targets (M3 P1).
  - Hovering a tile fills the inspector at the bottom left.
- **Keys:**
  - WASD and the arrows pan.
  - Enter is End turn, or the decision it leads with.
  - Escape clears the selection.
  - Ctrl or Cmd with Z undoes.
  - F fortifies, S waits, B founds a city.
  - F3 shows timings.
  - Watching: Space pauses, 1 to 4 set the speed, the full stop plays one round, K skips animations, F follows.
  - Nothing lists them.
- **Units:** Next unit (and Enter) visits units that still need orders. There is no previous unit and no list of units.
- **Size:** every panel is laid out in fixed pixels, made for a 1600x1000 window.
- **Not there at all:** tooltips, a minimap, settings.

## The pieces

1. **Tooltips.** Hovering any button, number or label for 0.4 s shows a card beside the pointer, saying what it is and what it does in one or two lines. Cards never cover the thing hovered and stay inside the window. How much they say is K1.
2. **Keys and a key map.**
   - A help overlay (key `?` and a Help button) lists every key and mouse action by context: map, unit, city, screens, watching.
   - New keys open each screen (tech tree, policies, civilizations, empire, log) and cycle units.
   - Escape closes the topmost panel before clearing the selection.
   - Which conventions, and whether keys can be rebound: K2.
3. **Minimap.**
   - The known world drawn small from tile data (terrain colour, owner tint, fog as in the map), with the camera's view outlined.
   - A click or drag looks there.
   - Where and how it shows: K3.
4. **Units.** Next and previous unit by key and button, the camera gliding to each. Whether there is also a list of all units: K4.
5. **UI scale.** One scale for every panel and font. How it is set: K5.
6. **Settings.** UI scale, edge-of-window panning on or off, camera speed and key bindings (if K2 allows rebinding), kept per player: a file in the config directory natively, localStorage in the browser. Opened from Home and from the top bar.
7. **The playtest loop.** Javed plays native and browser builds; what feels wrong comes back as a list; fixes inside existing designs follow, and anything needing a decision is put to him. How issues are captured: K6.

## The HUD, arranged like Civilization VI (K3)

Placements marked *sourced* are from Civ VI guides; those marked *remembered* are from memory and wait for Javed's
correction.

| Where | Civ VI | Here |
|---|---|---|
| Top edge, full width | Top bar: yields on the left, turn, date and menus on the right (*remembered*; faith sits top right, *sourced*) | Gold, science, culture, faith, happiness on the left with their per-turn change; turn, year, Undo and Home on the right |
| Under the top bar, left | A launch bar of round buttons for the trees and empire screens, and the world tracker with current research and civic (*remembered*; the world tracker shows current tech and civic, *sourced*) | Launch bar: Tech tree, Policies, Civilizations, Empire, Units, Log, Help, Settings. Below it the tracker: research with turns left (click opens the tree) and culture toward the next policy (click opens policies); in a room, the seats and deadline beneath |
| Under the top bar, right | The diplomacy ribbon: portraits of the leaders met (*remembered*) | A ribbon of met civs' colour badges with their nation's initial; at war ringed red; click opens the civilizations panel at that civ |
| Right edge, above End Turn | Notifications slide down the right-hand side (*sourced*) | Decisions and the turn's notes as a stack of short cards, newest at the bottom; click opens the decision or looks at the place |
| Bottom right | The action panel: End Turn, naming what it waits on (*sourced*) | The End Turn button (M6a), larger, with its prompt above it and Later beside |
| Bottom left corner | The minimap with its map options: filters, grid, markers, strategic view (*sourced*) | The minimap (known world, owners, fog, the camera's view outlined; click or drag to look), with buttons along its top: zoom to fit, yields on or off, the grid on or off |
| Bottom, beside the minimap | The selected unit's panel (*remembered*) | The unit panel: name, health, moves, experience, then its actions |
| Right side | The city panel when a city is open (*remembered*) | The city panel (M4), under the diplomacy ribbon |
| At the pointer | – | What the hover inspector shows now becomes a tooltip on the tile under the pointer |

## Non-goals

- Visual changes to the map and pieces (M8).
- Touch and phone layouts (M9).
- Sound.
- A tutorial.
- Advice or automation beyond the engine's own.

## How it will be verified

- Unit tests for tooltip placement inside the window, key map coverage (every bound key appears in the overlay), and the minimap's tile colours against the view.
- A recording that hovers each kind of control, opens the key map, uses the minimap and cycles units.
- The playtest loop itself.

## Choices (decided; see the table at the top)

K1. What tooltips say.
- (a) What each control and number is, in words. Client-only.
- (b) That, plus where a number comes from (gold, science, culture, faith and happiness by source; a city's yields by tile, building and specialist). civ-sim adds those breakdowns to the view (additive fields).

K2. Keys.
- (a) Unciv's default bindings, read from its source, so Unciv players feel at home.
- (b) A small set of this client's own, listed in the key map.
- (c) Either, with every key rebindable in settings.

K3. The minimap.
- (a) Always shown at the bottom left, the inspector moving above it.
- (b) Shown and hidden with a key (M), off at first.
- (c) No minimap; the zoomed-out map serves.

K4. Units.
- (a) Next and previous through units that need orders.
- (b) That, plus a units panel listing every unit by kind and state (needs orders, fortified, automated, travelling) with a click to look at each.

K5. UI scale.
- (a) Automatic from the window's size and the display's density, with a setting to adjust.
- (b) A setting only, 100% at first.

K6. Capturing playtest issues.
- (a) Javed writes them in chat.
- (b) A report key (F8) saves a screenshot, the turn, the selection and a line Javed types into `playtest/` beside the build (natively) or downloads it (browser), so each issue can be reproduced exactly.

## How M7 is built

- **Breakdowns (K1, civ-sim and civ-core):**
  - The own view carries `economy.breakdown` (gold, science, culture, faith and happiness) and each own city's `own.yieldBreakdown` (food, production, gold, science, culture, faith) as `{ stat, source, amount }`. Every stat's parts sum exactly to the figure shown; a test checks this every turn over 80 rounds.
  - civ-core's economy now builds each total from tagged parts (`YieldSource`, `city_output_parts`, `civ_wide_yield_parts`, `happiness::breakdown`), so the parts and the total are one computation.
  - Policies, wonders, technologies, the nation and city-state friendship show as one source, `empire`, because the unique index does not record where a civ-wide unique came from.
  - The release persistence gate (`complete_matches_save_and_replay`) and all 1,154 workspace tests pass.
- **The HUD** (`playui.rs`, `screens.rs`, `log.rs`, `minimap.rs`, `inspect.rs`), as the table above:
  - Yields on the left of the top bar, the turn, Undo and Home on the right.
  - The launch bar (Tech, Policies, Civilizations, Empire, Units, Log, Settings, ?) and the tracker (research and policy progress) under it.
  - Met civs' badges on the right, ringed red at war.
  - The notification stack above End Turn: decisions, then up to four of the turn's notes that matter; these replace the toasts that M6a gave notes.
  - End Turn at the bottom right with Later beside it.
  - The minimap in the bottom-left corner with the unit panel beside it.
  - The tile inspector is now a tooltip at the pointer, after a 0.35 s rest.
- **End Turn** also leads with "Choose research" (nothing researched) and "Adopt a policy" (culture enough), the prompts a player expects; the tracker says when a policy is ready.
- **Tooltips** (`tips.rs`): any node with a `Tip` shows a card after 0.4 s, below what is hovered or above it, never over it, inside the window. They are on:
  - the top bar's numbers, with their sources largest first;
  - a city's yields, by source;
  - the tracker, the launch bar and the badges;
  - unit actions, End Turn, Home and Undo;
  - the minimap's buttons.
- **Keys (K2):**
  - `keys.rs` is one table that the input systems read and the key map (H or ?) lists, so the map cannot leave a key out.
  - Wait moved from S to Space: S both panned the camera and made the unit wait.
  - New: N and Shift N cycle units that need orders; T P C E U L open screens; G shows the hex grid; Esc closes the topmost panel first, then what is aimed, then the selection.
- **Minimap (K3):**
  - Drawn from the view at 4 by 3 pixels a tile: ground colours, land tinted by owner, remembered tiles dimmed, cities as light dots.
  - The camera's footprint is outlined; a click or drag looks there.
  - Whole map zooms out; Grid (or G) draws hex edges in the map shader.
- **Units panel (K4):** every unit, those that need orders first, then waiting, on its own, fortified and moved; a click selects it and looks at it.
- **UI scale (K5):** automatic from the window (laid out for 1600 by 1000, clamped to 80% to 160%), times a setting of 75% to 150%. Review shots keep 100%.
- **Settings:** interface size, camera speed, panning at the window's edge. They are kept in `<config>/settings.json` natively and localStorage in the browser, and opened from Home and the launch bar.
- **Verified:**
  - 40 client tests. New ones cover tooltip placement, the key map listing every key once without clashes, the minimap's tile and ground mapping, its tile colours against a real view, breakdown lines summing to the figures shown, settings reading back, and the decision order with research and policies.
  - A 30 s recording (`CIV_SCENE=quality` on a developed save) shows the HUD, the gold tooltip with its sources, the key map, the units panel, the grid and unit cycling.
  - Both wasm builds check.
- **Not verified by hand:**
  - Hovering with a real pointer: the recording forces the tooltip it shows.
  - Edge panning, the automatic scale on other window sizes, and settings kept in a browser.
  - Clicking and dragging the minimap.
  - The keys.

