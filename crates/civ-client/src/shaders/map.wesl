// The map's one material (src/paint.rs): ground, objects on it, and water, told apart by `params.kind`.
//
// Shape comes from the meshes. What lies on the land (fields, roads, river water, a mine's pit floor), the
// owner's tint, the fog and the reveal are painted here per pixel from a texture of per-tile data, so a change
// in sight, ownership or improvement rewrites that texture and rebuilds no mesh. Tile data, two texels per
// tile at (2·col + k, row):
//   k = 0: r flags (1 known, 2 visible, 4 known before this reveal), g owner + 1, b improvement kind (bits 0-2:
//          1 farm, 2 mine, 3 pasture, 4 other; bit 3 pillaged) and road (bits 4-5: 1 road, 2 railroad; bit 6
//          pillaged), a reveal progress.
//   k = 1: r river edges (bit d: the edge facing direction d), g ground flags (1 water, 2 lake, 4 ice).

import bevy_pbr::render::{
    pbr_fragment::pbr_input_from_standard_material,
    pbr_functions::alpha_discard,
    mesh_view_bindings::globals,
};

@if(PREPASS_PIPELINE)
import bevy_pbr::{
    prepass::io::{VertexOutput, FragmentOutput},
    deferred::functions::deferred_output,
};
@else
import bevy_pbr::render::{
    forward_io::{VertexOutput, FragmentOutput},
    pbr_functions::{apply_pbr_lighting, main_pass_post_lighting_processing},
};

struct MapParams {
    width: u32,
    height: u32,
    wrap_x: u32,
    // 0 ground, 1 objects (trees, borders), 2 water.
    kind: u32,
}

@group(constants::MATERIAL_BIND_GROUP) @binding(100) var<uniform> params: MapParams;
@group(constants::MATERIAL_BIND_GROUP) @binding(101) var tiles: texture_2d<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(102) var palette: texture_2d<f32>;

const SQRT3: f32 = 1.7320508;
const PITCH: f32 = 1.7320508;
const SLATE: vec3<f32> = vec3<f32>(0.0129, 0.0152, 0.0203);

fn srgb(r: f32, g: f32, b: f32) -> vec3<f32> {
    return pow(vec3<f32>(r, g, b) / 255.0, vec3<f32>(2.2));
}

struct Tile {
    col: i32,
    row: i32,
    // Centre in the same wrap of the map as the point being shaded.
    centre: vec2<f32>,
    on_map: bool,
}

fn wrap_col(col: i32) -> i32 {
    let w = i32(params.width);
    if (params.wrap_x != 0u) {
        return ((col % w) + w) % w;
    }
    return col;
}

fn centre_of(col: i32, row: i32) -> vec2<f32> {
    return vec2<f32>(PITCH * (f32(col) + 0.5 * f32(row & 1)), -1.5 * f32(row));
}

// The tile under a ground point (x east, y north): axial coordinates rounded in cube space, as hex.rs does.
fn locate(p: vec2<f32>) -> Tile {
    let x = SQRT3 / 3.0 * p.x - 1.0 / 3.0 * -p.y;
    let z = 2.0 / 3.0 * -p.y;
    let y = -x - z;
    var rx = round(x);
    let ry = round(y);
    var rz = round(z);
    let dx = abs(rx - x);
    let dy = abs(ry - y);
    let dz = abs(rz - z);
    if (dx > dy && dx > dz) {
        rx = -ry - rz;
    } else if (dy <= dz) {
        rz = -rx - ry;
    }
    let row = i32(rz);
    let col = i32(rx) + (row - (row & 1)) / 2;
    var t: Tile;
    t.row = row;
    t.centre = centre_of(col, row);
    t.col = wrap_col(col);
    t.on_map = row >= 0 && row < i32(params.height) && t.col >= 0 && t.col < i32(params.width);
    return t;
}

fn texel(col: i32, row: i32, k: i32) -> vec4<u32> {
    return vec4<u32>(round(textureLoad(tiles, vec2<i32>(2 * col + k, row), 0) * 255.0));
}

fn direction(d: i32) -> vec2<f32> {
    let a = radians(60.0 * f32(d));
    return vec2<f32>(cos(a), sin(a));
}

fn corner(k: i32) -> vec2<f32> {
    let a = radians(60.0 * f32(k) + 30.0);
    return vec2<f32>(cos(a), sin(a));
}

// The neighbour in direction d (civ-core's odd-r offsets), or (-1, -1) off the map.
fn neighbour(t: Tile, d: i32) -> vec2<i32> {
    let odd = (t.row & 1) == 1;
    var dc = 0;
    var dr = 0;
    switch d {
        case 0: { dc = 1; }
        case 1: { dr = -1; if (odd) { dc = 1; } }
        case 2: { dr = -1; if (!odd) { dc = -1; } }
        case 3: { dc = -1; }
        case 4: { dr = 1; if (!odd) { dc = -1; } }
        default: { dr = 1; if (odd) { dc = 1; } }
    }
    let col = wrap_col(t.col + dc);
    let row = t.row + dr;
    if (row < 0 || row >= i32(params.height) || col < 0 || col >= i32(params.width)) {
        return vec2<i32>(-1, -1);
    }
    return vec2<i32>(col, row);
}

fn segment_distance(p: vec2<f32>, a: vec2<f32>, b: vec2<f32>) -> vec2<f32> {
    let ab = b - a;
    let t = clamp(dot(p - a, ab) / dot(ab, ab), 0.0, 1.0);
    return vec2<f32>(length(p - (a + ab * t)), t);
}

fn hash(n: u32) -> f32 {
    var x = n * 2654435761u;
    x = x ^ (x >> 15u);
    x = x * 2246822519u;
    x = x ^ (x >> 13u);
    return f32(x & 0xffffu) / 65535.0;
}

// Saturation × 0.25, value × 0.60 (docs/VISUAL-SPEC.md, Fog), near enough in linear light.
fn remembered(c: vec3<f32>) -> vec3<f32> {
    let grey = dot(c, vec3<f32>(0.2126, 0.7152, 0.0722));
    return mix(vec3<f32>(grey), c, 0.25) * 0.36;
}

// What lies on the land: river water in its bed, roads, fields and pits.
fn paint_ground(base: vec3<f32>, p: vec2<f32>, t: Tile, a: vec4<u32>, b: vec4<u32>) -> vec3<f32> {
    var c = base;
    let local = p - t.centre;
    // Rivers along this tile's edges; the tile across paints its own half.
    for (var d = 0; d < 6; d++) {
        if ((b.r & (1u << u32(d))) != 0u) {
            let s = segment_distance(p, t.centre + corner((d + 5) % 6), t.centre + corner(d)).x;
            c = mix(c, c * 0.8, 1.0 - smoothstep(0.07, 0.1, s));
            c = mix(c, srgb(58.0, 127.0, 162.0), 1.0 - smoothstep(0.045, 0.062, s));
        }
    }
    // Roads to each neighbour with a road, to the shared edge; the neighbour draws its half.
    let road = (a.b >> 4u) & 3u;
    if (road > 0u) {
        let pillaged = (a.b & 64u) != 0u;
        var linked = false;
        for (var d = 0; d < 6; d++) {
            let n = neighbour(t, d);
            if (n.x < 0) { continue; }
            let other = (texel(n.x, n.y, 0).b >> 4u) & 3u;
            if (other == 0u) { continue; }
            linked = true;
            let st = segment_distance(p, t.centre, t.centre + direction(d) * PITCH * 0.5);
            if (pillaged && fract(st.y * 4.0) > 0.5) { continue; }
            if (road == 2u && other == 2u) {
                c = mix(c, srgb(110.0, 106.0, 99.0), 1.0 - smoothstep(0.06, 0.075, st.x));
                c = mix(c, srgb(59.0, 58.0, 56.0), 1.0 - smoothstep(0.004, 0.009, abs(st.x - 0.035)));
            } else {
                c = mix(c, srgb(138.0, 117.0, 86.0), 1.0 - smoothstep(0.04, 0.055, st.x));
            }
        }
        if (linked) {
            c = mix(c, srgb(138.0, 117.0, 86.0), 1.0 - smoothstep(0.05, 0.065, length(local)));
        }
    }
    // Improvements.
    let kind = a.b & 7u;
    let dim = select(1.0, 0.5, (a.b & 8u) != 0u);
    let id = u32(t.row) * params.width + u32(t.col);
    if (kind == 1u) {
        // Farm: furrowed fields at an angle fixed per tile.
        let angle = hash(id) * 3.14159;
        let along = vec2<f32>(cos(angle), sin(angle));
        let u = dot(local, along);
        let v = dot(local, vec2<f32>(-along.y, along.x));
        let inside = (1.0 - smoothstep(0.4, 0.44, abs(u))) * (1.0 - smoothstep(0.4, 0.44, abs(v)));
        let stripe = select(srgb(143.0, 162.0, 76.0), srgb(185.0, 167.0, 90.0), fract(u * 4.2) < 0.5);
        c = mix(c, stripe * dim, inside * 0.9);
    } else if (kind == 2u) {
        // Mine: the pit is cut into the land (relief.rs); its floor is dark spoil.
        c = mix(c, srgb(74.0, 69.0, 64.0) * dim, 1.0 - smoothstep(0.16, 0.24, length(local)));
    } else if (kind == 3u) {
        // Pasture: a fence ring.
        c = mix(c, srgb(138.0, 117.0, 86.0) * dim, 1.0 - smoothstep(0.012, 0.02, abs(length(local) - 0.45)));
    } else if (kind == 4u) {
        c = mix(c, srgb(138.0, 117.0, 86.0) * dim, 0.6 * (1.0 - smoothstep(0.1, 0.14, length(local - vec2<f32>(0.25, -0.2)))));
    }
    // The owner's tint on land.
    let owner = a.g;
    if (owner > 0u && (b.g & 1u) == 0u) {
        let outer = textureLoad(palette, vec2<i32>(i32(owner - 1u), 0), 0).rgb;
        c = mix(c, outer, 0.10);
    }
    return c;
}

// Water by depth (carried in the vertex colour's alpha), with foam at the shore and small moving waves.
fn paint_water(depth: f32, b: vec4<u32>) -> vec3<f32> {
    var c = mix(srgb(60.0, 143.0, 169.0), srgb(30.0, 76.0, 120.0), smoothstep(0.2, 0.55, depth));
    if ((b.g & 2u) != 0u) { c = srgb(58.0, 127.0, 162.0); }
    if ((b.g & 4u) != 0u) { c = srgb(220.0, 232.0, 239.0); }
    return mix(c, srgb(232.0, 238.0, 240.0), 1.0 - smoothstep(0.0, 0.025, depth));
}

fn waves(p: vec2<f32>) -> vec3<f32> {
    let t = globals.time * 6.2831853 * 0.3;
    let dx = 0.6 * cos(p.x * 7.0 + t) + 0.4 * cos((p.x + p.y) * 11.0 - t * 1.3);
    let dz = 0.6 * cos(p.y * 8.0 - t * 0.8) + 0.4 * cos((p.x - p.y) * 9.0 + t * 1.1);
    return vec3<f32>(dx, 0.0, -dz) * 0.03;
}

@fragment
fn fragment(in: VertexOutput, @builtin(front_facing) is_front: bool) -> FragmentOutput {
    var pbr_input = pbr_input_from_standard_material(in, is_front);
    let p = vec2<f32>(in.world_position.x, -in.world_position.z);
    let t = locate(p);
    var fade = 1.0;
    var visible = false;
    var from_slate = 0.0;
    if (t.on_map) {
        let a = texel(t.col, t.row, 0);
        let b = texel(t.col, t.row, 1);
        var c = pbr_input.material.base_color.rgb;
        if (params.kind == 0u) {
            c = paint_ground(c, p, t, a, b);
        }
        visible = (a.r & 2u) != 0u;
        if (params.kind == 2u) {
            c = paint_water(in.color.a, b);
            // Remembered water lies still.
            if (visible) {
                pbr_input.N = normalize(pbr_input.N + waves(p));
            }
        }
        let reveal = f32(a.a) / 255.0;
        let known_before = (a.r & 4u) != 0u;
        if (!visible) {
            c = remembered(c);
        } else if (known_before) {
            c = mix(remembered(c), c, reveal);
        }
        if (!known_before) {
            from_slate = 1.0 - reveal;
        }
        pbr_input.material.base_color = vec4<f32>(c, pbr_input.material.base_color.a);
        // Fade to slate toward edges shared with unexplored tiles (or the map's edge).
        fade = 0.0;
        for (var d = 0; d < 6; d++) {
            let n = neighbour(t, d);
            var unknown = n.x < 0;
            if (!unknown) {
                unknown = (texel(n.x, n.y, 0).r & 1u) == 0u;
            }
            if (unknown) {
                let to_edge = PITCH / 2.0 - dot(p - t.centre, direction(d));
                fade = max(fade, 1.0 - clamp(to_edge / 0.25, 0.0, 1.0));
            }
        }
        fade = max(fade, from_slate);
    }
    pbr_input.material.base_color = alpha_discard(
        pbr_input.material.flags,
        pbr_input.material.alpha_cutoff,
        pbr_input.material.base_color
    );
    var out: FragmentOutput;
@if(PREPASS_PIPELINE) {
    out = deferred_output(in, pbr_input);
} @else {
    out.color = apply_pbr_lighting(pbr_input);
    out.color = vec4<f32>(mix(out.color.rgb, SLATE, fade), out.color.a);
    out.color = main_pass_post_lighting_processing(pbr_input, out.color);
}
    return out;
}
