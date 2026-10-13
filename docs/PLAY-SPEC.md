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
