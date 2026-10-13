# Visual spec: the 3D terrain map

Status: direction (D7) and choices V1-V5 decided by Javed on 2026-10-13; the numbers are starting values that
M1 and M2 screenshots will test, and changes after review go back into this file. Nothing here is built.

## What the screen has to say

The first playable is spectator mode on `base` (D5, D6), so the spec starts from what a watcher must read at a
glance, in this order:

1. Where civs and armies are, and whose they are.
2. Who owns which land.
3. Where cities are and how large they are.
4. What the land is for movement and war: water, hills, mountains, forests, jungle, marsh, rivers.
5. What changed this round.
6. Resources and improvements, when zoomed in.

When two elements compete for attention, the higher one on this list wins: it gets the saturated colour,
the contrast and the motion. Terrain stays muted so nation colours and pieces carry the screen [carried, V4].

Standing rule: no arthropod creatures or spider-like silhouettes anywhere: units, flora, effects or icons.

## World geometry

| Item | Value |
|---|---|
| Hex | Pointy-top, circumradius R = 1.0 m |
| Tile centre | x = √3 · (col + 0.5 · (row mod 2)), z = 1.5 · row; row 0 is the north edge (civ-core's NorthEast and NorthWest are row − 1) |
| Axes | Bevy's +Y up; north is −Z; east is +X |
| Map extent | 16x10 to 128x80 tiles; the default 32x20 map is about 56 m by 30 m |
| Wrap | When `wrap_x`, the chunk strip is drawn twice and the camera's x wraps modulo the map width |
| Chunks | 8x8 tiles per terrain mesh; a changed tile rebuilds only its chunk |
| Hex mesh | 3 rings: 37 vertices, 54 triangles per hex; 553k triangles on the largest map |

## Terrain surface

The land is one continuous heightfield, not hex prisms. Each vertex height blends the heights of the up to
three tiles that meet near it, weighted by distance to their centres. Fog rule: a tile absent from the view
never contributes; its place in the blend is taken by the nearest known tile, so the edge of the known world
reveals nothing about what lies past it.

| Terrain | Height (m, sea level 0) | Albedo (sRGB) |
|---|---|---|
| Ocean | floor −0.55 | water #1E4C78 |
| Coast | floor −0.20 | water #3C8FA9 |
| Lakes | floor −0.25 | water #3A7FA2 |
| Grassland | 0.10 | #6F8F3E |
| Plains | 0.10 | #A6A05A |
| Desert | 0.10 | #D9C48E |
| Tundra | 0.10 | #8E8C76 |
| Snow | 0.10 | #EDF1F3 |
| Hill (feature) | + 0.40 cosine bump, falling to 0 at 0.85 R | the base terrain's colour |
| Mountain | ridged peak 1.20 ± 0.20 (hash of tile id), slopes to 0.7 R | rock #7C766D; snow #F3F5F6 above 0.95 m |

- Neighbouring mountains join along a ridge through their shared edge, so a range reads as one landform
  rather than separate cones [carried, V4].
- Detail noise of ±0.04 m comes from a hash of world position, with no seed, so a tile always looks the same.
- Terrain colours blend across edges over 0.20 R. Slopes steeper than 35° switch to the rock colour.
- Materials are matte: roughness 0.9, metallic 0, vertex colour plus the noise in the shader. No textures until V1.

## Water

- One water plane at y = 0 across the whole map (doubled when wrapping). Its colour comes from the floor depth
  under it: Coast's shallow colour at −0.20 m, deepening to Ocean's at −0.55 m.
- Waves are normal-map only, 0.3 Hz, with no vertex motion. A shore foam band 0.08 R wide runs where land meets
  water.
- Ice is a flat sheet #DCE8EF at y = 0.02 over the water. An atoll is a 0.5 R ring of sand at y = 0.04.

## Features

| Feature | Look |
|---|---|
| Forest | 7-11 instanced trees per hex within 0.8 R, 0.25-0.40 m tall; conifers #2F4A34 on Tundra and Snow, broadleaf #3F5F2F elsewhere. Trees also stand on edges shared with another forest, so woods are continuous [carried, V4] |
| Jungle | 9-13 denser trees #2E5A2A with palms |
| Marsh | Height pulled down to 0.04 m; pools of the lake colour over 30% of the hex; reeds |
| Oasis | A 0.3 R pool and three palms |
| Flood plains | Ground #93A84E in a band 0.5 R wide along the tile's river edges |
| Fallout | Ground tinted #7F8A62; two to four bare trunks |
| Natural wonder | A generic 0.6 m marker until V1 |

## Rivers, roads and improvements

- Rivers follow hex edges, from `river_edges` (one bit per edge in Direction order E, NE, NW, W, SW, SE, set
  on both tiles). The channel is 0.14 m wide, cut 0.06 m below the land, in the lake colour. A river edge that
  doesn't continue tapers out over 0.2 m.
- A road is a dirt ribbon #8A7556, 0.10 m wide, draped 0.01 m above the ground from centre to centre with each
  known neighbour that also has a road. A railroad is two rails #3B3A38 on ballast #6E6A63 0.14 m wide, with ties
  every 0.06 m. Pillaged roads break into dashes with 40% gaps.
- Farm: five ground stripes alternating #B9A75A and #8FA24C over a 0.6 m square. Mine: a dark cut #4A4540 in
  the slope. Pasture: a fence ring at 0.45 R. Other improvements: a 0.15 m marker post until V1. Pillaged:
  colour value halved, the marker tilted 20°.
- Resources: a billboard 0.30 m above the ground at offset (+0.35, +0.35) m from the tile centre, shown at
  camera distance 40 m or less. Until V1 it is a diamond (bonus green #6FA35A, luxury violet #8C6BB1, strategic
  grey #8A8F96) with the resource's initial.

## Borders

- A line along every edge between a civ's tile and a tile that isn't its own: 0.07 m in the civ's outer colour,
  with a 0.03 m stripe of its inner colour on the inside, draped 0.015 m above the ground. Its width never drops
  below 2 px on screen.
- Owned land gets the outer colour at 10% opacity. Water gets no fill.
- Nation colours are the ruleset's own (`outerColor`, `innerColor` in `Nations.json`: 52 majors, 24
  city-states). If two civs in a game have outer colours closer than ΔE2000 12, the later one swaps its outer
  and inner colours.

## Cities

- Buildings: min(4 + 2 × population, 30) blocks with footprints of 0.08-0.16 m and heights of 0.08-0.30 m,
  placed within 0.78 m of the centre and clear of river channels. Placement is fixed by the city id. Walls
  #CDBFA6, roofs #9C5B42; nation colour stays off the buildings.
- A capital adds a 0.5 m landmark. City walls (`walls` in the view) are a ring at 0.85 R, 0.10 m high.
- Wonders and buildings the view places on tiles (`placed`) are a 0.6 m obelisk in #C9A94B (wonder) or a
  0.2 m block (building) until V1.
- Banner: screen space, anchored 0.9 m above the centre. 22 px tall. Population in a circle (inner colour on
  outer), then the name at 0.95 rem, with a star before a capital's name. A health bar 3 px tall below it
  appears only when the city is damaged.

## Units

Placeholders until V1, one shape per unit class, 0.36 m tall, in the owner's outer colour with a band of its
inner colour:

| Class | Placeholder |
|---|---|
| Melee | Box 0.22 x 0.36 x 0.22 m |
| Ranged | Cylinder 0.20 m across |
| Mounted | Box 0.34 m long, 0.30 m tall |
| Siege | Low box 0.32 x 0.18 m with a barrel |
| Naval | Hull wedge 0.45 m long |
| Air | Flat chevron 0.40 m across |
| Civilian | Sphere 0.24 m |
| Great person | Cone 0.40 m |

- Alone on a tile, a unit stands at the centre. A military and a civilian unit sharing a tile stand at −0.22 m
  and +0.22 m on x. An embarked unit sits on a small hull.
- Badge: a disc 0.24 m across, 0.55 m above the ground, facing the camera: outer colour fill, class glyph in
  the inner colour. A health bar 0.32 m wide under the badge appears only when the unit is damaged.

## Fog

| State | Source in the view | Look |
|---|---|---|
| Unexplored | Tile absent | No geometry. A flat slate plane #1E2127 at y = −0.6 shows through (V2). Known tiles fade to slate across their outer 0.25 R where they border unexplored tiles, computed from the known tile alone |
| Remembered | `visible: false` | Saturation × 0.25, value × 0.60. No units and no animation. Cities and improvements as last seen; the inspector shows `observed_turn` |
| Visible | `visible: true` | Full colour |

A tile coming into sight fades from its previous state to full colour over 0.35 s.

## Camera

| Item | Value |
|---|---|
| Projection | Perspective, vertical FOV 35° |
| Angle | Pitch fixed at 52° below the horizon; yaw fixed, north up |
| Distance | 8 m (about 5 hexes across at 16:9) to the smaller of the whole map's width and 200 m; default 24 m (about 15 hexes across and 13 rows deep) |
| Zoom | One wheel notch multiplies or divides the distance by 1.15, toward the cursor; smoothed with a 0.08 s half-life. Trackpad pinch zooms continuously |
| Pan | Drag with the left or middle button tracks the ground 1:1. WASD and arrows move at the camera distance per second. Two-finger scroll pans. Edge panning off |
| Bounds | z held within the map plus two rows; x wraps when `wrap_x`, otherwise held within the map plus two columns |
| Haze | Linear, colour #C7D3DF, from the camera distance + 30 m to the camera distance + 200 m |

## Light and image

- Sun from the north-west: azimuth 315°, elevation 42°. Shadowed slopes keep about 40% of a lit face's
  luminance; fill light is tuned to that in M1.
- Shadows: two cascades, the first ending at 30 m, the last at 120 m. Maps 2048² native, 1024² wasm. Casters:
  mountains, trees, buildings and units.
- Tone mapping TonyMcMapface (Bevy's default), fixed exposure, MSAA 4x on both targets.

## Picking and the spectator HUD

- Hover outlines the hex the cursor ray hits first (mountains occlude what's behind them) with a 0.04 m line
  #F5F1E6 at 80%. A click pins it in the inspector.
- UI font: Inter (OFL), 1 rem = 16 px times the OS scale factor. Panels are solid #14161B at 85% opacity, with
  no gradients and no cards.
- Top bar, 40 px: turn and year at the left ("Turn 34 · 2350 BC"); pause/play, speed 1x 2x 4x 8x, step one
  round and skip-animations in the centre; whose eyes (all-seeing or any one civ) and follow-events at the right.
- Right panel, 220 px: one row per civ: a colour chip (outer colour with an inner-colour border), the name,
  then the CivView fields that tell the game's state; the exact columns are picked in M2 from what CivView
  carries.
- Inspector at the bottom left, 280 px: terrain and features, resource, improvement (and whether pillaged),
  road, rivers, owner, city, and the last seen turn for remembered tiles.

## Motion

Durations are at 1x; 2x, 4x and 8x divide them.

| Event | Motion |
|---|---|
| Move | 0.22 s per tile step, ease-in-out; turn to face 0.10 s |
| Melee attack | Lunge 0.30 m toward the target over 0.18 s; the defender flashes white at 60% for 0.10 s; the damage number rises 0.4 m over 0.7 s; return over 0.18 s |
| Ranged attack | Projectile on an arc 0.6 m high, 0.35 s |
| Death | Sink 0.15 m and fade over 0.45 s |
| City founded | Blocks rise from −0.3 m over 0.6 s, each starting 0.02 s after the last; the banner fades in over 0.25 s |
| City captured | Banner colour wipes left to right over 0.5 s |
| Border change | New edges draw in over 0.4 s |

- A round's replay plays civ by civ. At 1x it is capped at 8 s; a longer one scales every duration down to fit,
  with no step shorter than 0.05 s. Skip-animations snaps to the view.
- The next round starts 0.5 s after the presentation queue empties.
- Follow-events glides the camera to an off-screen event over 0.6 s, ease-in-out, and the event waits for it.
  Any manual pan or zoom suspends following for 3 s.

## Budgets

Estimates, measured in M1 and M2:

| Target | Budget |
|---|---|
| Native, this Mac (MacBookPro18,3) at its full display resolution | 120 fps at default zoom on 32x20; 60 fps or better fully zoomed out on 128x80; main thread at most 5 ms per frame |
| Wasm, Chrome with WebGPU on this Mac, 1920x1080 canvas | 60 fps at default zoom on 32x20; 30 fps or better fully zoomed out on 128x80 |
| Terrain | At most 2 ms per chunk rebuild and 3 rebuilds per frame; the rest wait |
| Draw calls | At most 300 at default zoom (trees, buildings and units instanced) |
| Wasm download | 15 MB brotli or less, first frame within 4 s on a fast connection |

## Out of scope here

The city screen, research, policies and diplomacy (M3 and later), natural-wonder models, combat close-ups.

## How M1 draws it

Javed's review, 2026-10-13: the look stands as M2's base; "things that are terrain should be terrain not
stickers"; the mesh, light and mountain changes below stand.

- Shape is geometry: the heightfield carries hills, mountains with ridged crags, river beds (0.06 m deep along
  river edges, both banks agreeing), mine pits, marsh and oasis pools and atoll rings.
- What lies on the land is painted per pixel by the map shader (`crates/civ-client/src/shaders/map.wesl`) from
  a texture of per-tile data: river water in its bed with wet banks, roads and railroads to each neighbour that
  has one, furrowed fields at an angle fixed per tile, a mine's dark floor, a pasture's fence ring, the owner's
  10% tint, remembered tiles' colour, the edge fade to slate and the 0.35 s reveal. A change in sight,
  ownership, roads or improvements rewrites the texture; only a change of shape rebuilds a chunk.
- Water takes its colour, shore foam and small moving waves (0.3 Hz) in the same shader, from the depth each
  water vertex carries; remembered water lies still.
- Hex mesh: four rings at 0.42, 0.72, 0.9 and 1.0 of the corner distance (96 triangles a hex), crowded toward
  the edge where shores and blends change.
- Light: sun 3,000 lux, ambient 400 (cd/m²), holding the 40% shadow ratio. Water roughness 0.45.
- Rock needs height above 0.40 m as well as a steep slope, so hills keep their ground's colour.
- Measured 2026-10-13: chunk build 2.05 ms median (budget 2 ms), 3.3 ms worst; native 112-120 fps with a round
  closing every frame; wasm download 7.8 MB (WebGPU) and 8.4 MB (WebGL2) brotli.
- Not built yet: natural-wonder models (a marker post stands in), city meshes and units (M2).

## How M2 draws it

Built 2026-10-13 for review (spectator mode).

- Units: the placeholder shapes per class above, in the owner's outer colour with an inner-colour band; an
  embarked unit stands on a hull; units sharing a tile stand apart. The badge is screen-space UI sized to 0.24 m
  at the unit's depth (10 to 28 px) with the class glyph; a health bar shows only when hurt.
- Cities: blocks fixed by the city id, the capital's tower, walls when the city has them, all on the objects
  material (remembered cities grey with their tiles). Banners as specified: population, star, name, a health
  bar when damaged.
- Motion as specified, played as beats, one per acting civ in order: its units walk at once, then its battles
  play one by one; new and taken cities rise over 0.6 s. Rounds are capped at 8 s at 1x. Keys: space pauses,
  1 to 4 pick 1x to 8x, full stop plays one round, K skips animations, F toggles following.
- HUD: the top bar and the civ panel as specified; the panel's columns are cities and techs (the view carries
  techs only for the watcher's own civ, so others show a dash). Raiders are left out.
- Sound: a `Cue` message for each round, move, battle, fall, founding and capture; nothing plays it.
- Whose eyes: all-seeing or seat 0's eyes. Switching replays the match to the same turn under the other
  sight. Any other civ's eyes needs civ-sim to watch through a chosen civ.
- Not built: border edges drawing in over 0.4 s, and the banner colour wipe on capture (the taken city rises
  instead).

## Choices

Taken by Javed on 2026-10-13. The options he did not pick are in git history (637b789).

| | Choice | Pick |
|---|---|---|
| V1 | Models and icons | Procedural meshes in code. The placeholders above are the first versions of those meshes |
| V2 | Unexplored | Dark slate, as specified under Fog |
| V3 | Sound | Hooks, no sound: presentation emits a typed sound cue for each event (move, attack, death, founding, capture, border change, round start) through a Bevy message nothing consumes yet; no audio plugin is compiled in |
| V4 | Carried-over looks | Both kept: continuous mountain ranges and forests, quiet world with loud pieces |
| V5 | Wasm graphics | WebGPU with a WebGL2 fallback. Bevy 0.20's `webgpu` feature overrides `webgl2` at compile time, so there are two wasm builds and the page loads the WebGPU one when `navigator.gpu` gives an adapter, the WebGL2 one otherwise |
