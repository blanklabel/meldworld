// Biome-blending ground material (an ExtendedMaterial extension over StandardMaterial).
//
// The overworld floor is a single big plane. Instead of hot-swapping its texture to
// the player's *current* biome (which snaps the whole ground at once when you cross a
// band), this shader picks the biome from the fragment's own WORLD position and
// cross-fades between adjacent biomes across a band around each boundary — so as you
// approach a border you see the next biome's ground gradually take over ahead of you.
//
// Biome is a function of RADIAL distance from the hub, keyed off the ACTUAL per-section
// biomes (each section is a concentric radius ring, radius = corridor x in the radial
// world) sent by the server — NOT the old fixed distance bands. So the ground finally
// matches each section's real creatures/obstacles. `rings[i] = (outer_radius, biome,
// _, _)`, sorted ascending, `count` live entries; `update_ground_biome_rings` fills it.

#import bevy_pbr::{
    pbr_fragment::pbr_input_from_standard_material,
    pbr_functions::{apply_pbr_lighting, main_pass_post_lighting_processing},
    forward_io::{Vertex, VertexOutput, FragmentOutput},
    mesh_functions,
    view_transformations::position_world_to_clip,
}
// The wave field lives in one place — see `water_wave.wgsl`. The sea, the maze's pools and
// Last City's water all read the same crests.
#import meld::water_wave::{wave_height, water_normal, sea_swell}

// Continuous overworld terrain height — MUST match `world_render::terrain_height` in
// Rust exactly (that places entities/camera; this displaces the ground vertices).
// MUST match `meld_proto::terrain::height` (Rust) exactly.
fn terrain_height_wgsl(p: vec2<f32>) -> f32 {
    let base = 9.0 * sin(p.x * 0.0063 + 0.4) * cos(p.y * 0.0071 - 0.3)
        + 4.5 * sin(p.x * 0.015 - 0.8) * cos(p.y * 0.013 + 0.5)
        + 2.2 * sin(p.x * 0.033 + 1.7) * cos(p.y * 0.037 - 0.9)
        + 0.9 * sin((p.x + p.y) * 0.061 + 2.3);
    // Isolated steep mesas = the CLIFFS (the A*-routed backbone bends around them).
    // Amplitude MUST match `meld_proto::terrain::height`'s CLIFF_HEIGHT.
    let m = sin(p.x * 0.03 + 1.1) * cos(p.y * 0.028 - 0.6)
        + 0.5 * sin(p.x * 0.051 - 2.0) * cos(p.y * 0.047 + 1.4);
    return base + 0.0 * smoothstep(1.15, 1.30, m);
}

struct BiomeParams {
    // THE REGION DECOMPOSITION (`meld_proto::regions`): (arc_half, ring_step, cell_width,
    // boundary_warp). A biome is a property of a CELL, not of a radius ring — so the ground
    // asks which cell a fragment stands in rather than which band, and the world paints as a
    // patchwork. This replaces the 32-slot radial biome LUT that used to head this struct.
    region: vec4<f32>,
    // `[biome_gate]` in `BIOMES` order, four at a time because a uniform wants `vec4`s:
    // gate = field, forest, desert, ashfall; gate_hi = tundra, mire, amber_wood,
    // seized_engine; gate_hi2 = nestiphian_cradle, hearth_plains, seraphic_oubliette.
    // In the uniform because the gate decides WHICH themes a
    // cell may draw, and a shader that does not know it paints a biome the server does not
    // spawn — the same failure the coast constants are passed in to avoid.
    gate: vec4<f32>,
    gate_hi: vec4<f32>,
    gate_hi2: vec4<f32>,
    // World units the ground cross-fades across a cell boundary. A boundary is 2D now, so
    // this is a distance from the nearest edge rather than a radial band.
    region_blend: f32,
    region_seed: u32,
    // DEV/QA `MELD_BIOME`: the biome index every cell is forced to, or -1 in play. The shader
    // derives a cell's biome itself, so without this the ground paints the decomposition's
    // answer while the server spawns the forced one.
    region_force: i32,
    uv_scale: f32,
    // Displacement amplitude: 1.0 in the Overworld (rolling hills + cliffs), 0.0 in the
    // City/menus (flat ground — those scenes are hand-placed for a level plaza, and the
    // rolling heightmap would tilt every prop and shade the troughs into blue ribbons).
    terrain_amp: f32,
    // This run's terrain offset (matches `world_render::terrain_offset`), so the field —
    // and the route through it — differs every run instead of the same hills at the hub.
    terrain_off: vec2<f32>,
    _pad_peaks: vec2<f32>,                 // align `peaks` to 16 (matches the Rust struct)
    peaks: array<vec4<f32>, 24>,           // authored mountains [cx, cz, radius, height]
    peak_count: u32,
    // 1 underground: the ground draws flagstones instead of the biome's outdoor tile.
    dungeon: u32, _pad_pc1: u32, _pad_pc2: u32,
    // THE RANGES (`terrain::Ridge`): TWO vec4s each — slot 2k is (x0, z0, x1, z1)
    // and slot 2k+1 is (half_width, height, 0, 0). A range is a WALL, and the ground
    // has to draw it or it is an invisible one.
    // THE BRIDGES (`coast::Bridge`): two vec4s each — slot 2k is (x0, z0, x1, z1) and
    // 2k+1 is (half_width, 0, 0, 0).
    bridges: array<vec4<f32>, 16>,
    bridge_count: u32,
    _pad_bc0: u32, _pad_bc1: u32, _pad_bc2: u32,
    ridges: array<vec4<f32>, 32>,
    ridge_count: u32,
    _pad_rc0: u32, _pad_rc1: u32, _pad_rc2: u32,
    // The COASTLINE (`meld_proto::coast`): (arc_half_rad, neck_reach, peninsula_length,
    // channel_land_share). Passed in rather than baked, so the sea the player SEES is the
    // sea the server collides with — the shoreline is authored in two scenes that cannot
    // see each other, and two hand-placed shorelines drift.
    coast: vec4<f32>,
    // Peninsula widths: (neck_half, city_half, tip_taper, sea_depth).
    coast_w: vec4<f32>,
    // CONTINENTS (WG-7): this world's STRAITS — the inland seas that separate one landmass
    // from the next. TWO vec4s each, packed with the same eight numbers as
    // `meld_proto::coast::Strait`: slot 2k is (r_center, r_half, theta_center, theta_half)
    // and slot 2k+1 is (bridge0_theta, bridge0_half, bridge1_theta, bridge1_half). The
    // `peaks` precedent — an explicit table rather than noise, because a barrier has to be
    // STRUCTURED: an isotropic threshold over a sum of sines cannot make a long connected
    // channel with a pass in it at any amplitude.
    straits: array<vec4<f32>, 16>,
    strait_count: u32,
    _pad_sc0: u32, _pad_sc1: u32, _pad_sc2: u32,
    // The coast's own shape: BAYS (water bitten into the fan's rim) and ISLES (land standing
    // offshore). One vec4 each, `[cx, cz, radius, kind]`, kind 0 = bay and 1 = isle — one
    // array for both because they are one primitive, a disc that edits the shoreline.
    lobes: array<vec4<f32>, 12>,
    lobe_count: u32,
    _pad_lc0: u32, _pad_lc1: u32, _pad_lc2: u32,
    // INLAND WATER. `basins` is [cx, cz, radius, LEVEL] — that fourth number, the water
    // surface elevation, is what makes inland water a different thing from the sea, whose
    // level is globally zero. `rivers` is a chain of [x, z, half_width, chain_start]; a node
    // with chain_start >= 0.5 begins a new chain and the gap before it is a FORD.
    basins: array<vec4<f32>, 16>,
    rivers: array<vec4<f32>, 40>,
    basin_count: u32,
    river_count: u32,
    _pad_wc0: u32, _pad_wc1: u32,
    // The Shift's tell (CANON D20/§W2): (inner_radius, outer_radius, intensity, 0).
    // ⚠️ This used to read "a region is a radius ring ... so the doomed region draws as an
    // annulus". A region is a PATCH OF CELLS now (`WG-11`) — the radii are its band and
    // `shift_arc` is its bearing wedge, and burning the annulus alone told every party at
    // that depth to run from weather coming for a wedge of it. Intensity 0 = nothing pending.
    shift: vec4<f32>,
    // The tell's bearing wedge: `(arc_center, arc_half, 0, 0)`. `arc_half <= 0` = no wedge,
    // burn the whole ring.
    shift_arc: vec4<f32>,
    // Open-water animation: `(seconds, 0, 0, 0)`. The sea needs a clock and this shader had
    // none — which is why the ocean was a static tile while every pond prop drifted its own
    // material UVs from `animate_water`. A vec4 rather than a bare f32 so it lands 16-byte
    // aligned after `shift` and needs no new padding on either side of the mirror.
    sea_anim: vec4<f32>,
    // A SHIFT'S REPAINTED CELLS — `[cell_key, biome_index, 0, 0]` each, `repaint_count`
    // live, windowed nearest-first around the player. ⚠️ Mirrors `regions::Repaints`: the
    // biome here is DERIVED from the grid and the gate, which makes the floor a pure
    // function of the seed — this delta is the only thing that can move it, and a Shift
    // without it changed the props and the banner and left the ground alone.
    repaints: array<vec4<f32>, 32>,
    repaint_count: u32,
    _pad_rp0: u32,
    _pad_rp1: u32,
    _pad_rp2: u32,
    city: vec4<f32>,
}

@group(#{MATERIAL_BIND_GROUP}) @binding(100) var t_forest: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(101) var t_desert: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(102) var t_ashfall: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(103) var t_tundra: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(104) var t_mire: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(105) var samp: sampler;
@group(#{MATERIAL_BIND_GROUP}) @binding(106) var<uniform> params: BiomeParams;
@group(#{MATERIAL_BIND_GROUP}) @binding(107) var t_water_clear: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(108) var t_water_bog: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(109) var t_water_ice: texture_2d<f32>;
// Side-view rock for the steep parts of the same plane. See `cliff_color`.
@group(#{MATERIAL_BIND_GROUP}) @binding(110) var t_cliff_forest: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(111) var t_cliff_desert: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(112) var t_cliff_ashfall: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(113) var t_cliff_tundra: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(114) var t_cliff_mire: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(115) var t_dungeon_floor: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(116) var t_amber_wood: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(117) var t_seized_engine: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(118) var t_nestiphian_cradle: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(119) var t_hearth_plains: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(120) var t_seraphic_oubliette: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(121) var t_cliff_amber_wood: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(122) var t_cliff_seized_engine: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(123) var t_cliff_nestiphian_cradle: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(124) var t_cliff_hearth_plains: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(125) var t_cliff_seraphic_oubliette: texture_2d<f32>;
// A bridge's deck (worn flagstone) and its parapets (a rampart wall) — both ground textures
// the tiling work already shipped, so a bridge needs no art of its own.
@group(#{MATERIAL_BIND_GROUP}) @binding(126) var t_bridge_deck: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(127) var t_bridge_parapet: texture_2d<f32>;

// The sea's tile for the biome it borders — the same mapping the pond/bog-pool/
// frozen-pond props use (`WorldAssets::water_mats`), so a tundra shore is ice and a mire
// shore is bog rather than every coast being the same blue.
/// The colour deep water takes on, per biome. **The tile alone cannot carry this**: depth
/// is a multiplier over the bed, so a bog tile multiplied by a blue deep tint comes out
/// blue — which is what made every mire's open water the same slate as the forest coast
/// while the bog PONDS beside it (mesh water, `water_mats`) were correctly green. The two
/// halves of "the swamps are greenish" lived in two places and only one of them was true.
/// Frozen shores barely darken at all: ice is a surface, not a depth.
fn deep_tint_of(bi: i32) -> vec3<f32> {
    if (bi == 3) { return vec3<f32>(0.62, 0.72, 0.80); }   // tundra — pale, shallow ice
    if (bi == 4) { return vec3<f32>(0.13, 0.29, 0.16); }   // mire — peat green
    if (bi == 2) { return vec3<f32>(0.20, 0.20, 0.26); }   // ashfall — slick grey water
    return vec3<f32>(0.09, 0.30, 0.46);                    // open sea
}

/// **A SWAMP IS NOT ONE GREEN, AND IT IS NOT BLUE.** Fresh water takes its deep colour from
/// its own body's `variant` (see `body_variant`), so a mire holds a peat-green mere, a
/// tannin-stained purple-brown one, a dark blue one and a near-black one — the colours
/// standing water actually takes when it is full of rotting vegetation, iron and shade.
///
/// The stops are mixed rather than snapped so that neighbouring bodies differ without the set
/// looking like four presets. Each BODY is one colour, though: a mere shading through three
/// hues across its own width reads as a rendering fault, not as depth.
fn fresh_tint_of(bi: i32, v: f32) -> vec3<f32> {
    if (bi == 4) {
        // Mire: green -> tannin purple -> peat black -> bog blue.
        let a = vec3<f32>(0.09, 0.24, 0.13);
        let b = vec3<f32>(0.19, 0.11, 0.23);
        let c = vec3<f32>(0.05, 0.07, 0.06);
        let d = vec3<f32>(0.07, 0.14, 0.21);
        let t = v * 3.0;
        if (t < 1.0) { return mix(a, b, t); }
        if (t < 2.0) { return mix(b, c, t - 1.0); }
        return mix(c, d, t - 2.0);
    }
    if (bi == 3) { return mix(vec3<f32>(0.58, 0.70, 0.79), vec3<f32>(0.68, 0.78, 0.84), v); }
    if (bi == 2) { return mix(vec3<f32>(0.15, 0.15, 0.19), vec3<f32>(0.24, 0.20, 0.20), v); }
    if (bi == 1) { return mix(vec3<f32>(0.16, 0.34, 0.40), vec3<f32>(0.22, 0.42, 0.44), v); }
    // Field/forest: a woodland lake is tea-brown to weed-green, not sea blue.
    return mix(vec3<f32>(0.11, 0.26, 0.24), vec3<f32>(0.16, 0.30, 0.19), v);
}

/// How much sky a biome's water mirrors. **This is the other half of "a swamp is not blue":**
/// the reflection is `mix(water, sky, fres * 0.30 * openness)` and this shader's own note
/// records that the camera's fixed pitch keeps `fres` around 0.5-0.6 — so a strongly blue sky
/// was being laid over every pool at 15-18%, whatever colour the depth tint had just
/// computed. Turbid water does not do that: a bog is full of suspended peat, it absorbs and
/// scatters rather than mirroring, and it usually sits under canopy shade as well.
fn sky_reflect_of(bi: i32) -> f32 {
    if (bi == 4) { return 0.20; }   // mire — turbid and shaded; almost no sky in it
    if (bi == 2) { return 0.55; }   // ashfall — slick, but under an ash haze
    if (bi == 3) { return 0.80; }   // tundra — ice does mirror, but it is not water
    return 1.0;                     // open sea and woodland lakes
}

fn water_color(bi: i32, uv: vec2<f32>) -> vec4<f32> {
    if (bi == 3) { return textureSample(t_water_ice, samp, uv); }   // tundra
    if (bi == 4) { return textureSample(t_water_bog, samp, uv); }   // mire
    return textureSample(t_water_clear, samp, uv);
}

// Half-width of the land on the western spit at `d` units west of the hub. MUST match

// Signed difference between two bearings, wrapped to [-PI, PI] — MUST match
// `meld_proto::coast::ang_diff`. Without the wrap a strait centred near due west is
// silently a strait spanning the whole world the other way round.
fn ang_diff(a: f32, b: f32) -> f32 {
    let TAU = 6.28318530718;
    var d = a - b;
    d = d - TAU * floor((d + 3.14159265359) / TAU);
    return d;
}

// How far INSIDE strait `k` a point is, in world units (negative on the land around it).
// MUST match `meld_proto::coast::strait_depth`. Every term is a world-unit margin — the
// angular span is multiplied by `r` into an ARC so it composes with the radial one — which
// is what makes this a continuous field the beach can ramp over rather than three booleans
// wearing a float (the bug the fan's own edge already shipped once).
fn strait_depth_at(wxz: vec2<f32>, k: i32) -> f32 {
    let a = params.straits[k * 2];
    let b = params.straits[k * 2 + 1];
    let r_half = a.y;
    let th_half = a.w;
    if (r_half <= 0.0 || th_half <= 0.0) { return -1000.0; }
    let r = length(wxz);
    let theta = atan2(wxz.y, wxz.x);
    let in_band = r_half - abs(r - a.x);
    let in_span = (th_half - abs(ang_diff(theta, a.z))) * r;
    // …and not standing on one of its isthmuses.
    var off_bridge = 1e9;
    if (b.y > 0.0) { off_bridge = min(off_bridge, abs(ang_diff(theta, b.x)) * r - b.y); }
    if (b.w > 0.0) { off_bridge = min(off_bridge, abs(ang_diff(theta, b.z)) * r - b.w); }
    return min(min(in_band, in_span), off_bridge);
}

// How far INSIDE inland water a point is — positive in a lake or a channel, negative on the
// land around them. MUST match `coast::Shore::inland`.
//
// ⚠️ This is deliberately NOT part of `sea_depth_at`. That field is what `total_height` dips
// the ground toward the sea floor over, and sea level is globally zero — a basin sits at its
// OWN elevation and its hollow is already in the heightmap, which is what makes it a basin.
// Folding this in would excavate every lake a second time, below its own bed.
// A stable 0..1 key for one body of water, hashed from its own centre. Deterministic, so a
// mere keeps its colour from frame to frame and between sessions — a lake that changed hue as
// you walked would read as a bug, not as variety.
fn body_variant(centre: vec2<f32>) -> f32 {
    return fract(sin(dot(centre, vec2<f32>(12.9898, 78.233))) * 43758.5453);
}

// Authored CLIMBABLE peaks: smooth raised-cosine domes summed onto the ground — MUST
// match `meld_proto::terrain::peak_height`. World-space (NOT offset-shifted).
// A RANGE, mirroring `terrain::ridge_height` line for line. A capsule of raised ground with a
// LINEAR falloff — so its slope is exactly `height / half_width` at every point on the flank,
// which is what makes "this is a wall" an identity rather than something to sample for.
//
// ⚠️ `max`, NOT `+` (peaks sum, ranges do not). Segments of one range overlap end to end by
// design, and summing them would stack a wall to twice its authored height at every joint.
fn rg_seg_dist(p: vec2<f32>, a: vec2<f32>, b: vec2<f32>) -> f32 {
    let d = b - a;
    let len2 = dot(d, d);
    var t = 0.0;
    if (len2 > 1e-6) {
        t = clamp(dot(p - a, d) / len2, 0.0, 1.0);
    }
    return distance(p, a + d * t);
}

// THE WESTERN APPROACH — `meld_proto::coast::approach_bridge`, ridden down as its two
// endpoints (`coast_w.xy`, both on the z = 0 axis) and its half-width (`coast.w`).
//
// ⚠️ THERE WAS A `spit_half_width` HERE, MIRRORING A PENINSULA, AND THE PENINSULA IS GONE.
// Its binding term was `d · tan(gap_half) · share` — linear in radius, which in polar
// coordinates draws two perfectly STRAIGHT rays, so the west end of the world rendered as a
// machined triangle with a castle on it. A capsule's edges are parallel and its ends are
// round by construction, which is the whole reason the crossing is a bridge now.
fn approach_dist(wxz: vec2<f32>) -> f32 {
    let a = vec2<f32>(params.coast_w.x, 0.0);
    let b = vec2<f32>(params.coast_w.y, 0.0);
    return rg_seg_dist(wxz, a, b) - params.coast.w;
}

// A BRIDGE's surface, mirroring `terrain::bridge_surface`. Returns (height above sea level,
// 1.0 on a parapet, 1.0 if on a span at all).
//
// ⚠️ **THIS IS WHAT SEPARATES A BRIDGE FROM AN ISTHMUS.** `coast::Bridge` makes the span LAND,
// which is what keeps `is_land` a pure function of position for the pathfinder and every
// mover — but land alone renders as the sea simply not being there. The deck standing ABOVE
// the waterline, with water still drawn under its parapets, is what makes it read as a bridge,
// and that lives here.
fn bridge_at(wxz: vec2<f32>) -> vec3<f32> {
    var best = vec3<f32>(0.0, 0.0, 0.0);
    let n = i32(params.bridge_count);
    for (var i = 0; i < n; i = i + 1) {
        let b0 = params.bridges[2 * i];
        let hw = params.bridges[2 * i + 1].x;
        if (hw <= 0.0) {
            continue;
        }
        let d = rg_seg_dist(wxz, b0.xy, b0.zw);
        if (d >= hw) {
            continue;
        }
        let inner = hw * (1.0 - 0.28);
        var h = 2.6;
        var par = 0.0;
        if (d > inner) {
            h = h + 1.8;
            par = 1.0;
        }
        if (h > best.x) {
            best = vec3<f32>(h, par, 1.0);
        }
    }
    return best;
}

fn ridge_wedge(wxz: vec2<f32>) -> f32 {
    var h = 0.0;
    let n = i32(params.ridge_count);
    for (var i = 0; i < n; i = i + 1) {
        let r0 = params.ridges[2 * i];
        let r1 = params.ridges[2 * i + 1];
        let hw = r1.x;
        if (hw > 0.0) {
            let d = rg_seg_dist(wxz, r0.xy, r0.zw);
            if (d < hw) {
                h = max(h, r1.y * (1.0 - d / hw));
            }
        }
    }
    return h;
}

fn peak_dome(wxz: vec2<f32>) -> f32 {
    var h = 0.0;
    let n = i32(params.peak_count);
    for (var i = 0; i < n; i = i + 1) {
        let p = params.peaks[i];
        let r = p.z;
        if (r > 0.0) {
            let d = distance(wxz, p.xy);
            if (d < r) {
                h = h + p.w * 0.5 * (1.0 + cos(3.14159265 * d / r));
            }
        }
    }
    return h;
}

// `(depth, variant)` — how far inside inland water this point is, and WHICH body it belongs
// to. The variant is what lets a swamp hold a black mere, a tannin-purple one and a green one
// instead of one flat green everywhere: colour is per-BODY, not per-biome.
fn inland_water_at(wxz: vec2<f32>) -> vec2<f32> {
    var d = -1000.0;
    var v = 0.0;
    // Standing water: inside the radius bound AND below the surface level. The vertical
    // margin is divided by a nominal shore slope so it shares world units with the radial
    // one — `coast::BASIN_SHORE_SLOPE`, and it must match.
    let nb = i32(params.basin_count);
    for (var k = 0; k < nb; k = k + 1) {
        let b = params.basins[k];
        if (b.z <= 0.0) { continue; }
        let within = b.z - length(wxz - b.xy);
        // `terrain_height_wgsl` takes an ALREADY-OFFSET position, like every other caller.
        // The divisor is `coast::BASIN_SHORE_SLOPE`, held against this file by
        // `the_basin_shore_slope_matches_the_shader`.
        let ground = terrain_height_wgsl(wxz + params.terrain_off) + peak_dome(wxz) + ridge_wedge(wxz);
        let below = (b.w - ground) / 0.12;
        let dd = min(within, below);
        if (dd > d) { d = dd; v = body_variant(b.xy); }
    }
    // Flowing water: distance to each chain segment, minus its half-width.
    let nr = i32(params.river_count);
    for (var k = 1; k < nr; k = k + 1) {
        let a = params.rivers[k - 1];
        let b = params.rivers[k];
        if (b.w >= 0.5) { continue; }   // a new chain starts here — the gap is the ford
        let half = (a.z + b.z) * 0.5;
        if (half <= 0.0) { continue; }
        let p = wxz - a.xy;
        let s = b.xy - a.xy;
        let len2 = dot(s, s);
        var t = 0.0;
        if (len2 > 1e-6) { t = clamp(dot(p, s) / len2, 0.0, 1.0); }
        let dd = half - length(p - s * t);
        if (dd > d) { d = dd; v = body_variant(a.xy); }
    }
    return vec2<f32>(d, v);
}

// How far INTO the sea a point is, in world units (negative on land). Mirrors
// `meld_proto::coast::is_ocean` but signed, so the shoreline can fade instead of snapping
// to a hard edge one texel wide.
// The fan's half-angle at radius `d`. Mirrors `meld_proto::coast::arc_half_at` — held to it
// by `the_taper_matches_the_shader`.
fn arc_half_at(d: f32, arc_half: f32) -> f32 {
    let taper_start = 1200.0;
    let taper_end = 3200.0;
    let end_width = 200.0;
    let coast_wander = 0.06;
    let coast_wander_wavelength = 520.0;
    if (arc_half <= 0.0) { return arc_half; }
    var tapered = arc_half;
    if (d > taper_start) {
        let t = clamp((d - taper_start) / max(taper_end - taper_start, 1.0), 0.0, 1.0);
        let s = t * t * (3.0 - 2.0 * t);
        let end_half = min(end_width * 0.5 / max(d, 1.0), arc_half);
        tapered = arc_half + (end_half - arc_half) * s;
    }
    // The coast WANDERS, and only ever bites inward — mirrors `coast::arc_half_at`.
    let w = d / coast_wander_wavelength;
    let harmonic = 0.63 * sin(w) + 0.37 * cos(w * 2.7 + 1.9);
    let bite = coast_wander * 0.5 * (1.0 + clamp(harmonic, -1.0, 1.0));
    return tapered * (1.0 - bite);
}

fn sea_depth_at(wxz: vec2<f32>) -> f32 {
    // LAST CITY IS THE SAME SEA, DRAWN BY THE SAME SHADER. The city is its own scene in
    // its own coordinates and cannot use the world's radial fan (that shoreline, expressed
    // in city space, runs straight through the plaza), so it hands its OWN spit down:
    // `city` is (shore half-width, shelf reach, mainland back, causeway half-width), nonzero
    // only in the City — see `world_render::city_sea_uniform`.
    //
    // It used to be three hand-placed water planes instead, sitting a hair ABOVE the lawn
    // because the flat plaza had nothing to dip into — the exact "two hand-placed
    // shorelines that drift" this module was written to prevent, and it had already drifted
    // (the city's sea missed every fix the world's sea got, because they were not the same
    // water). One shoreline, one shader, both scenes.
    //
    // ⚠️ AND IT DRIFTED AGAIN ON THE MAINLAND TERM, WHICH IS WHY THE CITY DREW AS A PLANK ON
    // THE OCEAN. This was `max(past_flank, past_tip)`, which makes land the strip
    // `|x| <= shore` for EVERY z — including z running to minus infinity behind the city.
    // `city_sea_depth` grew its MAINLAND term to fix exactly that and the drawing side never
    // got it, so the shader painted open sea over ground the game was standing things on.
    //
    // Land is the SHELF, the CAUSEWAY out of town, or the MAINLAND that causeway reaches, so
    // the sea is however far you are from the nearest of the three — `min`, as the ocean's
    // own branch below takes the min of its fan, spit and neck, and a `min` of signed
    // distances is what keeps the field CONTINUOUS so every smoothstep over it still gets a
    // beach instead of a step. MUST match `coast::city_sea_depth` term for term.
    if (params.city.x > 0.0) {
        let past_shelf = max(abs(wxz.x) - params.city.x, abs(wxz.y) - params.city.y);
        let past_causeway = max(abs(wxz.x) - params.city.w, wxz.y - params.city.y);
        let past_mainland = wxz.y + params.city.z;
        return min(min(past_shelf, past_causeway), past_mainland);
    }
    let arc_half = params.coast.x;
    if (arc_half <= 0.0) { return -1000.0; }          // corridor mode: no gap, no sea
    let d = length(wxz);
    let theta = abs(atan2(wxz.y, wxz.x));
    // ⚠️ A SHORELINE IS A DISTANCE, NOT A BOOLEAN, AND THIS USED TO BE THREE BOOLEANS
    // WEARING A FLOAT. Land inside the fan returned a flat `-1000` — so the field jumped
    // from -1000 to about +26 across the fan's edge with nothing in between, and every
    // consumer that smoothsteps over it (the beach ramp, the depth tint, the swell) got a
    // STEP where it asked for a gradient. That is the vertical wall of water on the fan
    // boundary: no beach could form there because there was no band to form it in.
    //
    // Three land shapes, each as a signed distance in WORLD UNITS, and the sea is however
    // far you are from the nearest of them:
    //   * the FAN — its edge is a ray, so the distance past it is an ARC LENGTH (`* d`),
    //     which is why a fixed angular margin would be metres at the hub and kilometres out;
    //   * the SPIT that Last City stands on, across its width;
    //   * the NECK, the land bridge that closes the gap near the hub.
    // `min` of the three, so the sign still agrees with `meld_proto::coast::is_ocean`
    // exactly (sea iff past ALL THREE) while the magnitude is now continuous everywhere.
    // WG-11: the fan's half-angle is a FUNCTION OF RADIUS — the world is a teardrop that
    // closes to a corridor at the end. Mirrors `meld_proto::coast::arc_half_at`; the server
    // and the ground must agree about where the sea is, or we paint a coastline nothing
    // collides with.
    let past_fan = (theta - arc_half_at(d, arc_half)) * d;
    let past_shore = d - params.coast.y;
    var sea = min(min(past_fan, approach_dist(wxz)), past_shore);
    // CONTINENTS (WG-7): the sea is the OCEAN *union* every strait, and a signed depth's
    // union is a `max` — past the ocean's land, or inside an inland sea. On open ground far
    // from either, the ocean's own (negative) distance survives, so the beach at the fan's
    // rim is unchanged. Mirrors `meld_proto::coast::sea_depth_with`.
    let ns = i32(params.strait_count);
    for (var k = 0; k < ns; k = k + 1) {
        sea = max(sea, strait_depth_at(wxz, k));
    }
    // Then the coast's own shape, in list order — MUST match `coast::Shore::depth`. A bay is
    // a `max` (water wins over land) and an isle a `min` (land wins over water), so a later
    // isle stands inside an earlier bay. Both are signed distances, so both get a beach.
    let nl = i32(params.lobe_count);
    for (var k = 0; k < nl; k = k + 1) {
        let l = params.lobes[k];
        if (l.z <= 0.0) { continue; }
        let inside = l.z - length(wxz - l.xy);
        if (l.w < 0.5) { sea = max(sea, inside); } else { sea = min(sea, -inside); }
    }
    return sea;
}


// TOTAL ground height at world `wxz`: base rolling field (through the run offset) + the
// authored peak domes, all scaled by `terrain_amp` (0 flattens City/menus). This is the
// single source the vertex displaces by and the normal differentiates.
fn total_height(wxz: vec2<f32>) -> f32 {
    let land = terrain_height_wgsl(wxz + params.terrain_off) + peak_dome(wxz) + ridge_wedge(wxz);
    // A SEA IS A LEVEL, NOT AN OFFSET. This used to subtract a constant depth from the
    // land — which left the sea surface carrying the terrain's rolling hills, so the ocean
    // visibly went up and down like a field. Water finds its own level: past the shoreline
    // the surface IS `sea_level`, flat, regardless of what the heightmap underneath says.
    // The blend band is the beach — land ramps down to the waterline over a few units
    // instead of ending in a step the coarse ground grid would stair-step.
    let sea = sea_depth_at(wxz);
    let level = -params.coast_w.w;
    // ⚠️ THE RAMP BELONGS ON THE LAND SIDE OF THE WATERLINE, ALL OF IT. This was
    // `smoothstep(-6, 10)`, which put TEN UNITS OF IT PAST THE SHORE — so the first stretch
    // of every body of water was still sloping downhill while already being painted as
    // water, and what you saw was the BANK of the depression tinted blue, running down to a
    // point. Water read as a pit because it was being drawn as one: we have a single
    // surface here, so if it ramps, it is the bed, and there is no water surface left.
    //
    // Water finds its own level. Past the waterline the surface IS `level`, flat, from the
    // very first fragment; the blend band is the BEACH, and a beach is land.
    let t = smoothstep(-14.0, 0.0, sea);
    // …and the SWELL rides on that flat level as real displaced geometry (see `sea_swell`).
    // It fades in over the first few units rather than the twenty-six `openness` uses,
    // because the waves have to reach the SHORE — a flat dead margin around every coast is
    // the other half of what made this read as a basin instead of a sea.
    let swell = sea_swell(wxz, params.sea_anim.x) * smoothstep(0.0, 9.0, max(sea, 0.0));
    // ⚠️ `terrain_amp` FLATTENS THE LAND, NOT THE SEA. It used to scale the whole
    // expression, which is right for the hills (the City and the menus are hand-placed for
    // a level plaza) and wrong for the water: at amp 0 the sea level got multiplied to zero
    // too, so the city's ground could not dip and its water had to be laid ON TOP of the
    // grass. Flatten the land, let the water find its level, and the City gets a real bay.
    let span = bridge_at(wxz);
    if (span.z > 0.5) {
        // A flat deck at its own level, so the span does not inherit the sea floor's dip.
        return level + span.x;
    }
    return mix(params.terrain_amp * land, level + swell, t);
}

// Surface normal by finite differences over `total_height`, so both the rolling base and
// the mountain domes light naturally (flat → up-normal at amp 0).
fn terrain_normal(p: vec2<f32>) -> vec3<f32> {
    let e = 1.5;
    let hl = total_height(p - vec2<f32>(e, 0.0));
    let hr = total_height(p + vec2<f32>(e, 0.0));
    let hd = total_height(p - vec2<f32>(0.0, e));
    let hu = total_height(p + vec2<f32>(0.0, e));
    return normalize(vec3<f32>(hl - hr, 2.0 * e, hd - hu));
}

// Displace the sliding ground plane into rolling hills. Keyed off WORLD xz (like the
// biome/texture below), so the hills stay world-fixed even as the plane slides under
// the player — no swimming. Scaled by `terrain_amp` so non-overworld scenes stay flat.
@vertex
fn vertex(vertex: Vertex) -> VertexOutput {
    var out: VertexOutput;
    let world_from_local = mesh_functions::get_world_from_local(vertex.instance_index);
    var world_position = mesh_functions::mesh_position_local_to_world(
        world_from_local, vec4<f32>(vertex.position, 1.0));
    // Displace by the TOTAL height (rolling base through the run offset + authored peak
    // domes, `terrain_amp`-gated). `world_position` itself is unchanged — only where we
    // READ the field — so ground + entities (same functions) stay in lock-step.
    world_position.y += total_height(world_position.xz);
    out.world_position = world_position;
    out.position = position_world_to_clip(world_position.xyz);
    out.world_normal = terrain_normal(world_position.xz);
    out.uv = vertex.uv;
    out.instance_index = vertex.instance_index;
    return out;
}

/// 1.0 where a biome's water is ice. Tundra (3) is the only frozen coast; kept as a function
/// so the sea and its drift cannot disagree about which shores are solid.
fn frozen_of(bi: i32) -> f32 {
    return select(0.0, 1.0, bi == 3);
}

// The tinted ground colour for biome index `bi` at `uv`. Tints make each biome read
// distinctly under the cool ambient: forest/desert as-authored, Ashfall a charred
// burnt-red with ember-glow crevices, Tundra a cold frost-blue, Mire a sickly green.
// THE STRAND: what a coast is made of on the LAND side. Grass does not run into the sea —
// every real shoreline is sand, shingle or bare rock, and a lawn meeting water at a line
// is the single loudest tell that a coast was painted rather than built.
//
// Keyed off the bordering biome so the strand belongs to its place: pale sand where the
// ground is grass or dune, wet dark shingle in the mire, and cold bare ROCK on the ashfall
// and tundra coasts, where a soft sandy beach would look imported. The tile's own
// luminance is kept as texture (`uv` is still sampled) so the strand has grain and is not
// a flat wash.
fn shore_color(bi: i32, uv: vec2<f32>) -> vec4<f32> {
    let g = biome_color(bi, uv);
    // Luminance only — the strand takes the ground's TEXTURE and its own hue.
    let lum = clamp(dot(g.rgb, vec3<f32>(0.299, 0.587, 0.114)) * 1.15, 0.25, 1.0);
    if (bi == 2 || bi == 3) {
        // Ashfall + tundra: bare rock, faintly cool, no yellow at all.
        return vec4<f32>(vec3<f32>(0.52, 0.54, 0.58) * lum, g.a);
    }
    if (bi == 4) {
        // The mire: silt, not a beach — darker and greener than sand.
        return vec4<f32>(vec3<f32>(0.44, 0.42, 0.30) * lum, g.a);
    }
    // Grass and dune coasts: pale sand.
    return vec4<f32>(vec3<f32>(0.84, 0.76, 0.56) * lum, g.a);
}

// ---------------------------------------------------------------------------------------
// BREAKING THE TILE GRID
//
// Every biome is ONE 64px tile sampled at `world.xz * uv_scale`, so the same square
// repeated to the horizon and the eye locked onto the grid immediately — the ground read
// as wallpaper rather than terrain. Two cheap fixes, both pure shader:
//
//   1. Per-cell ROTATION. Each large cell picks one of four 90-degree turns from a hash of
//      its own coordinate, which destroys the lattice without touching the art. It is
//      hashed rather than random so a patch of ground looks the same every time you walk
//      back over it — ground that reshuffled itself would read as a bug.
//   2. Macro TONE variation. Low-frequency noise over a much larger scale than the tile,
//      lightening and darkening whole stretches, which is what makes real ground read as
//      having history instead of being a flat swatch.
//
// Deliberately NOT a second texture blend: that needs another five bindings and muddies
// the palette, and it turns out the grid itself was most of the problem.

fn hash2(c: vec2<f32>) -> f32 {
    return fract(sin(dot(floor(c), vec2<f32>(127.1, 311.7))) * 43758.5453);
}

// Smooth value noise, used at a scale far below the tile so it varies ACROSS tiles rather
// than inside one.
fn vnoise(p: vec2<f32>) -> f32 {
    let i = floor(p);
    let f = fract(p);
    let u = f * f * (3.0 - 2.0 * f);
    let a = hash2(i);
    let b = hash2(i + vec2<f32>(1.0, 0.0));
    let c = hash2(i + vec2<f32>(0.0, 1.0));
    let d = hash2(i + vec2<f32>(1.0, 1.0));
    return mix(mix(a, b, u.x), mix(c, d, u.x), u.y);
}

// ⚠️ PER-CELL ROTATION WAS TRIED HERE AND IS WRONG FOR THIS ART.
//
// Turning each cell's tile by a hashed quarter did destroy the lattice — and replaced it
// with something worse, because these tiles have DIRECTIONAL detail: grass leans, strata
// band, sand ripples. Rotated, the ground read as patches "pointing in different
// directions", which is a louder artifact than the grid it was hiding. A texture can only
// be rotated invisibly if it has no orientation, and almost none of ours qualify.
//
// So the repeat is broken WITHOUT touching orientation: the same tile is sampled a second
// time at an unrelated scale and mixed in gently. The two never line up, so the eye finds
// no lattice, and because both samples are the same art at the same angle nothing points
// anywhere new. The weight is deliberately low — this is meant to read as variation, and
// pushed further it just reads as blur.
// ---------------------------------------------------------------------------------------
// SIXTEEN VARIATIONS, BLENDED
//
// Each ground texture is a 4x4 ATLAS: sixteen variations of one material, which is what
// `create_tiles_pro` actually produces and what it is for. Picking one and shipping it
// threw away fifteen and left the world a single 64px stamp repeated to the horizon —
// the monotony we then tried to paper over by sampling the same stamp twice.
//
// They are BLENDED, not switched per cell. A hard switch would only trade a repeating
// grid for a mosaic of visible borders, because these variations are independent and do
// not share edges. Cross-fading two of them over a smooth low-frequency field gives
// ground that changes character across a hillside with no boundary anywhere in it.

const ATLAS_GRID: f32 = 4.0;
const ATLAS_CELL: f32 = 64.0;    // the tile itself
const ATLAS_PAD: f32 = 1.0;      // its wrap gutter (see `pack_ground_atlas.py`)
const ATLAS_STRIDE: f32 = 66.0;  // ATLAS_CELL + 2 * ATLAS_PAD
const ATLAS_SIDE: f32 = 264.0;   // ATLAS_STRIDE * ATLAS_GRID
// Sixteen drawn variations, each usable at four quarter-turns: SIXTY-FOUR.
const ATLAS_VARIANTS: f32 = 64.0;

// One variation of the material, addressed inside the atlas and optionally turned.
//
// ROTATION IS FREE VARIETY HERE, and it is worth saying why it works now when it did not
// before. Turning a SINGLE tile per cell failed badly — the same grass leaning four ways
// read as patches "pointing in different directions", because the features were
// identical and only their angle changed. With sixteen distinct variations in play a
// quarter-turn is no longer recognisable as the same tile turned, so the same trick that
// was a defect becomes 4x the variety for nothing.
//
// Quarter-turns only, and written out rather than built from a rotation matrix: 90
// degrees maps texel centres onto texel centres exactly, while an arbitrary angle
// resamples pixel art off its own grid.
//
// ⚠️ A TURN SWAPS WHICH PAIR OF EDGES MEETS AT THE JOIN, so a tile that wraps
// left-to-right but not top-to-bottom GROWS a seam grid the moment it is turned.
// Measured per atlas as the edge-wrap difference over ordinary interior variation
// (`client/scripts/atlas_seams.py`), and it is not uniform:
//
//   field 1.4/4.9 and dungeon 2.0/3.1  -- seamless, well under the noise floor
//   mire 22.9/26.6, forest 22.1/26.8   -- marginal, symmetric: a turn costs nothing
//   desert 21.4/22.4 (noise 4.6)       -- SEAMY on both axes, so it shows turned or not
//   tundra 55.8/56.3 (noise 17.3)      -- the worst, and symmetric: rotation is neutral
//   ashfall 32.3/58.0                  -- ASYMMETRIC 1.8x: this is the one a turn HURTS
//
// So rotation is free on six of the seven and ashfall is the exception. The fix is to
// regenerate that material as tileable rather than to special-case it here -- a per-biome
// rotation flag is a rule about the ART hidden inside the renderer, where nobody looking
// at a seamy tile would think to find it. Re-run the script rather than trusting this
// block: these are numbers about the current drawings, not a law about the shader.
fn atlas_sample(t: texture_2d<f32>, uv: vec2<f32>, variant: f32) -> vec4<f32> {
    var f = fract(uv) - vec2<f32>(0.5, 0.5);
    let turns = floor(variant / 16.0);
    if (turns == 1.0) { f = vec2<f32>(-f.y, f.x); }
    else if (turns == 2.0) { f = vec2<f32>(-f.x, -f.y); }
    else if (turns == 3.0) { f = vec2<f32>(f.y, -f.x); }
    f = f + vec2<f32>(0.5, 0.5);
    // A cell is addressed INSIDE its gutter. Sub-rects of an atlas cannot use the hardware
    // REPEAT wrap — it would wrap the whole ATLAS rather than the cell — so the wrapped
    // neighbour is baked around each tile as a one-pixel border: the texel REPEAT would
    // have fetched, sitting where a filter will look for it.
    //
    // ⚠️ IT BUYS NOTHING TODAY AND IS STILL THE RIGHT SHAPE. `load_tiled` samples this
    // atlas NEAREST (pixel art), and a nearest sample takes exactly one texel, so there
    // is no filtering here to drag a neighbouring variation across the join. The gutter
    // is what makes the cell correct under a filter, and it is the precondition for ever
    // turning one on — which distant ground will eventually want, because nearest with no
    // mips is what makes the far field crawl.
    //
    // It replaces a CLAMP, which was the wrong tool twice: it repeated the edge texel
    // rather than wrapping it (breaking the join on any tile that genuinely was seamless),
    // and its inset was computed against the atlas width while being applied in cell
    // space — an eighth of a texel where the comment claimed a half. Under nearest that
    // only ever guarded an off-by-one exactly on the boundary, which is real but is not
    // the fringe the comment described.
    let tile = variant % 16.0;
    let g = vec2<f32>(floor(tile % ATLAS_GRID), floor(tile / ATLAS_GRID));
    return textureSample(t, samp, (g * ATLAS_STRIDE + ATLAS_PAD + f * ATLAS_CELL) / ATLAS_SIDE);
}

// DE-TILING. Three things together, because each one alone was tried and each one alone
// looked worse than doing nothing:
//
//   1. the atlas is ORDERED BY SIMILARITY (`pack_ground_atlas.py`), so index n and n+1
//      are the two most-alike drawings in the set rather than an arbitrary pair;
//   2. a cell's variation is the smooth field's index JITTERED BY ONE, which — given (1)
//      — means neighbouring cells hold tiles that already look alike;
//   3. and the three cells overlapping any point are blended WIDELY.
//
// (3) is only affordable because of (1) and (2). Blending arbitrary variations is how the
// first attempt turned the ground into a patchwork quilt: these sixteen differ by a mean
// 25.1 (desert) to 76.1 (tundra) in generator order, and no amount of cross-fading hides
// a border between two tiles that look nothing alike. Ordering drops that adjacency cost
// by 20-48% (tundra 1188 -> 621), and jittering by one keeps every blend between near
// neighbours in that ordering — so the transition has almost nothing to hide.
//
// A triangular lattice, not square cells: a square grid of random offsets replaces one
// visible grid with another, while three overlapping lobes leave no axis-aligned edge.
const DETILE_CELL: f32 = 0.22;   // hex cells per tile — a cell spans several tiles
const DETILE_SHARP: f32 = 1.5;   // 1.0 = pure linear blend; higher narrows the seams

struct Detile { n0: vec2<f32>, n1: vec2<f32>, n2: vec2<f32>, w: vec3<f32> }

fn detile_nodes(p: vec2<f32>) -> Detile {
    let q = vec2<f32>(p.x - p.y * 0.57735027, p.y * 1.15470054);   // skew to triangles
    let i = floor(q);
    let f = q - i;
    var d: Detile;
    if (f.x + f.y < 1.0) {
        d.n0 = i;
        d.n1 = i + vec2<f32>(1.0, 0.0);
        d.n2 = i + vec2<f32>(0.0, 1.0);
        d.w = vec3<f32>(1.0 - f.x - f.y, f.x, f.y);
    } else {
        d.n0 = i + vec2<f32>(1.0, 1.0);
        d.n1 = i + vec2<f32>(1.0, 0.0);
        d.n2 = i + vec2<f32>(0.0, 1.0);
        d.w = vec3<f32>(f.x + f.y - 1.0, 1.0 - f.y, 1.0 - f.x);
    }
    return d;
}

// Where in the material this cell reads from. Randomising the ORIGIN is what stops one
// drawing repeating on a lattice; it costs nothing, because a cell is one tile turned and
// shifted, not a different tile.
fn node_offset(n: vec2<f32>) -> vec2<f32> {
    return vec2<f32>(hash2(n * 1.7 + 3.1), hash2(n * 2.3 + 11.9));
}

// The smooth field's variation, so the ground still reads as broad patches of one ground.
fn variant_at(uv: vec2<f32>, seed: f32) -> f32 {
    return floor(vnoise(uv * 0.037 + vec2<f32>(seed, seed * 1.7)) * 15.999);
}

// This cell's variation and quarter-turn, packed as `atlas_sample` wants them. The jitter
// is ±1 IN THE SIMILARITY ORDERING, which is the whole reason the ordering exists.
fn node_variant(n: vec2<f32>, base: f32) -> f32 {
    let jitter = floor(hash2(n * 5.9 + 17.3) * 2.999) - 1.0;
    let turn = floor(hash2(n * 3.3 + 29.7) * 3.999);
    return clamp(base + jitter, 0.0, 15.0) + turn * 16.0;
}

fn ground_sample(t: texture_2d<f32>, uv: vec2<f32>) -> vec4<f32> {
    let base = variant_at(uv, 0.0);
    let d = detile_nodes(uv * DETILE_CELL);
    var w = pow(d.w, vec3<f32>(DETILE_SHARP, DETILE_SHARP, DETILE_SHARP));
    w = w / (w.x + w.y + w.z);
    return atlas_sample(t, uv + node_offset(d.n0), node_variant(d.n0, base)) * w.x
         + atlas_sample(t, uv + node_offset(d.n1), node_variant(d.n1, base)) * w.y
         + atlas_sample(t, uv + node_offset(d.n2), node_variant(d.n2, base)) * w.z;
}

// Whole-stretch light and shade, at a scale much larger than one tile.
fn macro_tone(world_xz: vec2<f32>) -> f32 {
    let n = vnoise(world_xz * 0.012) * 0.65 + vnoise(world_xz * 0.043) * 0.35;
    return mix(0.86, 1.14, n);
}

// ---------------------------------------------------------------------------------------
// CLIFFS: THE STEEP PART OF THE SAME PLANE
//
// The overworld is one displaced grid — `total_height` pushes it into hills, peaks and
// the dip toward the sea floor — so a cliff is not a separate mesh, it is simply a patch
// where the surface is steep. And because the ground's uv is the fragment's world XZ, a
// near-vertical face was sampling a TOP-DOWN texture stretched down its entire length:
// the grass smeared, the strata impossible.
//
// Triplanar fixes it without a single new triangle. Project along each axis, weight by
// the surface normal, and a face is always textured along an axis it actually faces:
//
//   - a flat surface is nearly all Y-weight  -> the ground tile, exactly as before
//   - a steep surface is nearly all X/Z      -> the cliff tile, at true scale, no stretch
//
// The vertical projections use world Y as one coordinate, so rock is the same size on a
// two-unit step and a sixty-unit sea cliff. Nothing about the mesh changes; this is only
// where the colour is READ.

// The biome's side-view rock. Sampled through `ground_sample` like the ground is, because
// these tiles are NOT seamless — they are the least-mismatched of a batch of independent
// variations — and the same dual-scale mix that hides the ground's grid also softens a
// cliff's wrap.
fn cliff_tex(bi: i32, uv: vec2<f32>) -> vec4<f32> {
    if (bi <= 0) { return ground_sample(t_cliff_forest, uv); }
    if (bi == 1) { return ground_sample(t_cliff_desert, uv); }
    if (bi == 2) { return ground_sample(t_cliff_ashfall, uv); }
    if (bi == 3) { return ground_sample(t_cliff_tundra, uv); }
    if (bi == 4) { return ground_sample(t_cliff_mire, uv); }
    if (bi == 5) { return ground_sample(t_cliff_amber_wood, uv); }
    if (bi == 6) { return ground_sample(t_cliff_seized_engine, uv); }
    if (bi == 7) { return ground_sample(t_cliff_nestiphian_cradle, uv); }
    if (bi == 8) { return ground_sample(t_cliff_hearth_plains, uv); }
    return ground_sample(t_cliff_seraphic_oubliette, uv);
}

// Rock projected from the two SIDE axes and mixed by which one the face points along, so
// there is no seam where a cliff turns a corner.
fn cliff_color(bi: i32, wp: vec3<f32>, n: vec3<f32>, scale: f32) -> vec4<f32> {
    let ax = abs(n);
    // Guard the divide: a normal with no horizontal component never reaches here, but a
    // NaN would spread across the whole surface if one ever did.
    let wsum = max(ax.x + ax.z, 0.0001);
    let zx = cliff_tex(bi, vec2<f32>(wp.x, -wp.y) * scale);
    let xz = cliff_tex(bi, vec2<f32>(wp.z, -wp.y) * scale);
    return zx * (ax.z / wsum) + xz * (ax.x / wsum);
}

// How much of this fragment is CLIFF rather than ground, from the surface normal.
// `terrain_normal` is already computed per-vertex, so this costs nothing to obtain.
// The band is deliberately narrow and high: below it the world should look exactly as it
// did, and a gentle hill wearing rock would be a worse bug than a cliff wearing grass.
fn cliff_weight(n: vec3<f32>) -> f32 {
    return smoothstep(0.72, 0.42, n.y);
}

fn biome_color(bi: i32, uv: vec2<f32>) -> vec4<f32> {
    // UNDERGROUND IS A FLOOR, not the outdoors dimmed. Checked here, in the one place
    // every ground sample already passes through, so the shore/cliff/water paths inherit
    // it without each remembering to ask. The theme lighting a dungeon already applies
    // does the per-biome colouring, which is why one flagstone serves all five.
    if (params.dungeon != 0u) {
        return ground_sample(t_dungeon_floor, uv);
    }
    // One place, so no biome can be left reading as wallpaper because its arm was missed.
    if (bi <= 0) {
        return ground_sample(t_forest, uv);
    }
    if (bi == 1) {
        return ground_sample(t_desert, uv);
    }
    if (bi == 2) {
        let ash = ground_sample(t_ashfall, uv);
        let ember = (1.0 - ash.r) * 0.5; // darkest cracks glow hottest
        return vec4<f32>(ash.rgb * vec3<f32>(0.95, 0.24, 0.18) + vec3<f32>(ember, ember * 0.18, 0.02), ash.a);
    }
    if (bi == 3) {
        return ground_sample(t_tundra, uv) * vec4<f32>(0.72, 0.86, 1.15, 1.0);
    }
    // The mire's sour green, left as AUTHORED. This was briefly raised to (1.0, 1.35, 0.85)
    // to compensate for the swamp reading as permanent dusk — but the dusk was the GROUND
    // SHADOWING ITSELF (see `NotShadowCaster` on `WorldGround`), not the tint. Lifting a
    // tint to pay for a lighting bug is how a biome ends up looking like a sunny meadow the
    // moment the real bug is fixed.
    if (bi == 4) {
        return ground_sample(t_mire, uv) * vec4<f32>(0.75, 0.95, 0.7, 1.0);
    }
    if (bi == 5) {
        return ground_sample(t_amber_wood, uv);
    }
    if (bi == 6) {
        return ground_sample(t_seized_engine, uv);
    }
    if (bi == 7) {
        return ground_sample(t_nestiphian_cradle, uv);
    }
    if (bi == 8) {
        return ground_sample(t_hearth_plains, uv);
    }
    // ⚠️ TINT THE OUBLIETTE DOWN, NOT UP. Its art is already near-white ceramic, so the
    // "casts no shadows" idea — which I first wrote as a brightening multiply — blew the
    // whole biome out against the sky's own white fog and lost the eyes and the gold
    // entirely. Pulling it down and slightly blue keeps it cold and legible, and the
    // shadowless feel has to come from the lighting rather than from the albedo.
    return ground_sample(t_seraphic_oubliette, uv) * vec4<f32>(0.82, 0.83, 0.90, 1.0);
}

// ---------------------------------------------------------------------------------------
// OPEN WATER
//
// The sea is shaded here, per fragment, rather than by a water crate over a water mesh —
// and the reason is `sea_depth_at`. Every mesh-based attempt in this game founders on the
// same thing: our water is centimetres deep. A pond basin is half a unit; the city's sea
// plane sits five centimetres over its own grass. Anything that shades by measuring the
// distance from the surface to the bed (Beer's law over a depth buffer, which is what
// `bevy_water` does) has almost nothing to measure and resolves to "no water".
//
// This shader already knows the answer analytically. `sea_depth_at` returns how far into
// the sea a point is IN WORLD UNITS, unbounded, straight from `meld_proto::coast` — the
// same function the server collides against. So depth here is tens of units where geometry
// offered fractions, and it costs no prepass, no depth texture, and no mesh at all.
//
// Detail is per-pixel for the same reason Seascape is: the wave field and its normal are
// evaluated at the fragment, so ripple density is independent of how finely the ground
// plane happens to be tessellated.

// ═══ THE REGION DECOMPOSITION ═══
// A hand-mirror of `meld_proto::regions`, function for function. It has to be a mirror
// rather than a lookup table sent per frame: a cell is derivable from a position in
// constant time at any radius, so the shader can just ask — no windowing, no re-send as the
// player moves, nothing to go stale. The price is that the arithmetic is written twice, and
// that is why every hash here is 32-bit: WGSL has no 64-bit integer, so the Rust side uses
// u32 throughout for exactly this mirror to be possible.

fn rg_hash32(seed: u32) -> u32 {
    var h = seed;
    h = h ^ (h >> 16u);
    h = h * 0x7feb352du;
    h = h ^ (h >> 15u);
    h = h * 0x846ca68bu;
    return h ^ (h >> 16u);
}

fn rg_sectors(ring: u32) -> u32 {
    let r_mid = (f32(ring) + 0.5) * params.region.y;
    // WG-11: the TAPERED arc — mirrors `regions::Grid::sectors`. Asking the constant
    // half-angle here cuts the 200-unit end corridor into 65 three-unit slivers.
    let arc = 2.0 * arc_half_at(r_mid, params.region.x) * r_mid;
    let n = round(arc / max(params.region.z, 1.0));
    return min(u32(max(n, 1.0)), 128u);
}

fn rg_ring_offset(ring: u32) -> f32 {
    return f32(rg_hash32(params.region_seed ^ (ring * 0x9E3779B9u)) & 0xffffu) / 65536.0;
}

// Depends on BEARING alone, which is what keeps the ring index monotone along every ray —
// the partition stays well-defined at any warp magnitude.
fn rg_warp_at(bearing: f32) -> f32 {
    let phase =
        f32(rg_hash32(params.region_seed ^ 0x5F356495u) & 0xffffu) / 65536.0 * 6.28318531;
    return params.region.w
        * (0.62 * sin(bearing * 3.0 + phase) + 0.38 * cos(bearing * 7.0 - phase * 2.0));
}

fn rg_ring_at(r: f32, bearing: f32) -> u32 {
    return u32(floor(max(r + rg_warp_at(bearing), 0.0) / max(params.region.y, 1.0)));
}

fn rg_fan_t(bearing: f32) -> f32 {
    return clamp((bearing + params.region.x) / (2.0 * params.region.x), 0.0, 1.0);
}

fn rg_sector_in(ring: u32, bearing: f32) -> u32 {
    let n = max(rg_sectors(ring), 1u);
    let idx = floor(rg_fan_t(bearing) * f32(n) + rg_ring_offset(ring));
    return min(u32(max(idx, 0.0)), n - 1u);
}

// A cell's biome as an index into `BIOMES`. Gated on the cell's INNER radius, so a cell
// straddling a gate is held until it is wholly past — matching `regions::biome_of`.
fn rg_biome_of(ring: u32, sector: u32) -> i32 {
    let inner = f32(ring) * params.region.y;
    let gates = array<f32, 11>(
        params.gate.x, params.gate.y, params.gate.z, params.gate.w,
        params.gate_hi.x, params.gate_hi.y, params.gate_hi.z, params.gate_hi.w,
        params.gate_hi2.x, params.gate_hi2.y, params.gate_hi2.z);
    // ⚠️ THE CAPSTONE TAKES THE WHOLE BAND — mirrors `regions::EXCLUSIVE`. Past its gate
    // the roll below is skipped entirely, so the last stretch of the world is ONE place.
    // Index 10 is `seraphic_oubliette`; hard-coded here because WGSL has no list to walk
    // and the Rust side owns the contract.
    if (gates[10] <= inner) {
        return 10;
    }
    // A SHIFT'S REPAINT, in the same order `regions::Grid::biome_of` asks it: it beats the
    // seed's roll below and the CAPSTONE above beats it, so the end of the world stays one
    // place whatever the weather does. Asked HERE rather than in `rg_biome_at` so the
    // boundary cross-fade in `rg_edge` — which resolves the neighbouring cell's biome
    // through this same function — fades toward the repainted theme for free.
    let key = (ring << 7u) | (sector & 127u);
    for (var ri = 0u; ri < params.repaint_count; ri = ri + 1u) {
        if (u32(params.repaints[ri].x) == key) {
            return i32(params.repaints[ri].y);
        }
    }
    var open = array<i32, 11>(0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0);
    var count = 0u;
    for (var i = 0u; i < 11u; i = i + 1u) {
        if (gates[i] <= inner) {
            open[count] = i32(i);
            count = count + 1u;
        }
    }
    if (count == 0u) {
        return 0;
    }
    return open[rg_hash32(params.region_seed ^ rg_hash32(key ^ 0x2545F491u)) % count];
}

fn rg_biome_at(wxz: vec2<f32>) -> i32 {
    // The harness override wins, mirroring `regions::Regions::biome_at`.
    if (params.region_force >= 0) {
        return params.region_force;
    }
    let r = length(wxz);
    let bearing = atan2(wxz.y, wxz.x);
    let ring = rg_ring_at(r, bearing);
    return rg_biome_of(ring, rg_sector_in(ring, bearing));
}

// (distance to the nearest cell boundary in world units, the biome across it). A negative
// second component means the nearest boundary is the fan's own rim — open sea, not a
// boundary between cells, so nothing to fade toward.
fn rg_edge(wxz: vec2<f32>) -> vec2<f32> {
    let r = length(wxz);
    let bearing = atan2(wxz.y, wxz.x);
    let ring = rg_ring_at(r, bearing);
    let n = max(rg_sectors(ring), 1u);
    let step = max(params.region.y, 1.0);
    let r_eff = max(r + rg_warp_at(bearing), 0.0);

    var best = 1.0e9;
    var across = -1.0;
    if (ring > 0u) {
        let d = r_eff - f32(ring) * step;
        if (d < best) {
            best = d;
            across = f32(rg_biome_of(ring - 1u, rg_sector_in(ring - 1u, bearing)));
        }
    }
    let d_out = f32(ring + 1u) * step - r_eff;
    if (d_out < best) {
        best = d_out;
        across = f32(rg_biome_of(ring + 1u, rg_sector_in(ring + 1u, bearing)));
    }
    // An angular gap costs `r` world units per radian, so the same wedge is a short step
    // near the hub and a long walk at the frontier.
    let sector = rg_sector_in(ring, bearing);
    let to_bearing = 2.0 * params.region.x / f32(n);
    let frac = rg_fan_t(bearing) * f32(n) + rg_ring_offset(ring);
    let within = frac - floor(frac);
    if (sector > 0u) {
        let d = within * to_bearing * r;
        if (d < best) {
            best = d;
            across = f32(rg_biome_of(ring, sector - 1u));
        }
    }
    if (sector + 1u < n) {
        let d = (1.0 - within) * to_bearing * r;
        if (d < best) {
            best = d;
            across = f32(rg_biome_of(ring, sector + 1u));
        }
    }
    return vec2<f32>(max(best, 0.0), across);
}

// `BIOMES` index → ground TEXTURE index. Field and forest share grass: a meadow and a wood
// stand on the same ground and the only difference is how many trees are in the way.
// Mirrors `world_render::biome_ring_index`.
fn rg_tex_of(bi: i32) -> i32 {
    if (bi <= 1) {
        return 0;
    }
    return bi - 1;
}

@fragment
fn fragment(in: VertexOutput, @builtin(front_facing) is_front: bool) -> FragmentOutput {
    var pbr_input = pbr_input_from_standard_material(in, is_front);

    let uv = in.world_position.xz * params.uv_scale;
    let r = length(in.world_position.xz);

    var blended: vec4<f32>;
    // Hoisted: the SEA needs the biome it borders too, to pick its tile (ice off a tundra
    // shore, bog off a mire one), and it is otherwise scoped to the branch below.
    var here_biome: i32 = 0;
    if (params.region.y <= 0.0) {
        // No world (menus / city): plain forest floor.
        blended = biome_color(0, uv);
    } else {
        here_biome = rg_tex_of(rg_biome_at(in.world_position.xz));
        blended = biome_color(here_biome, uv);
        // Fade toward whatever is across the nearest cell boundary. Weight peaks at HALF on
        // the boundary itself, so both cells reach the same colour there and the edge is a
        // gradient from either side rather than a seam — which the old asymmetric
        // fade-to-prev-then-to-next could only manage in one dimension.
        let e = rg_edge(in.world_position.xz);
        if (e.y >= 0.0) {
            let w = 0.5 * (1.0 - smoothstep(0.0, max(params.region_blend, 0.001), e.x));
            blended = mix(blended, biome_color(rg_tex_of(i32(e.y)), uv), w);
        }
    }

    // THE SEA. Painted over whatever biome the ring says, because the coast is a fact
    // about the world rather than a property of the biome it borders — a tundra shore and
    // a forest shore are the same water. Shallows near the shoreline read lighter and let
    // the ground beneath show through, so the beach is a gradient rather than a hard
    // outline; open water deepens and hides it. Mirrors `meld_proto::coast`, which is what
    // movement and path routing collide against.
    // ⚠️ **THE TINT ASKS FOR ALL WATER; THE DISPLACEMENT ASKS ONLY FOR THE SEA.**
    // `total_height` dips the ground toward the sea floor over `sea_depth_at` ALONE, because
    // sea level is globally zero — an inland basin sits at its own elevation and its hollow
    // is already in the heightmap, so dipping there would excavate every lake below its own
    // bed. But a lake, a bog and a river still have to be PAINTED, and this is the stage
    // that paints. Mirrors `coast::Shore::water` — `max(sea, inland)`.
    //
    // ⚠️ THIS `max` IS THE LINE THAT WAS MISSING, and its absence is instructive:
    // `inland_depth_at` was defined in both shaders, carried through the uniform, filled by
    // the client and fed by the server — and never CALLED. So every lake and river in the
    // game existed in the world model, blocked movement, and drew absolutely nothing. The
    // mirror test could not catch it either, because it compares the two shaders to EACH
    // OTHER and both were equally unwired. `every_coast_helper_is_actually_called` does now.
    let salt = sea_depth_at(in.world_position.xz);
    let fresh_wv = inland_water_at(in.world_position.xz);
    let sea = max(salt, fresh_wv.x);
    // Which one is painting this fragment: 1.0 where an inland body won. Fresh water takes a
    // per-BODY colour out of its biome's palette; the sea keeps the biome's own single tint,
    // because an ocean is one body and has no siblings to differ from.
    let is_fresh = select(0.0, 1.0, fresh_wv.x > salt);
    // THE STRAND, first — the land side of the shoreline, under the water blend below. It
    // rides the SAME band the ground's beach ramp uses (`smoothstep(-14, 0)` in
    // `total_height`), so the sand appears exactly where the ground starts falling toward
    // the water: the strand IS the beach, not a decal near it.
    // CLIFFS, before the shore and the water get their say — rock belongs under the
    // strand and under the waterline, not painted over them. A steep face takes its
    // biome's side-view rock, projected from whichever horizontal axis it points along.
    blended = mix(
        blended,
        cliff_color(here_biome, in.world_position.xyz, normalize(in.world_normal), params.uv_scale),
        cliff_weight(normalize(in.world_normal)),
    );

    // THE BRIDGE, painted before the shore and the water get their say only in the sense that
    // it wins outright: a span is a built thing standing over both. Deck at a fixed scale so
    // the flagstones read as flagstones rather than stretching with the biome's uv.
    let span_paint = bridge_at(in.world_position.xz);
    blended = mix(blended, shore_color(here_biome, uv), smoothstep(-14.0, -1.0, sea));
    if (sea > -0.5) {
        // The real water TILE, not a flat colour — the same art the city's sea and every
        // pond in the game uses. It was two hardcoded RGB constants at first, which meant
        // the arena and Last City drew the same sea two different ways, in exactly the two
        // scenes `coast` exists to keep from disagreeing.
        //
        // Static, unlike the pool props: `animate_water` drifts THEIR material UVs from the
        // clock, and this shader has no time uniform to do the same. The tile is the fix
        // that mattered; a moving surface wants a `time` binding and is its own change.
        let t = params.sea_anim.x;
        let wxz2 = in.world_position.xz;
        let wuv = wxz2 * params.uv_scale * 0.5;

        // How much of a sea this fragment is: shallows keep the tile and the bed, open
        // water becomes surface. Everything below fades in on this, so the shoreline is a
        // gradient rather than a rim where one material stops and another starts.
        // Reaches its depth colour over 14 units rather than 26: with the ramp moved onto
        // the land side, `sea == 0` is now the actual waterline instead of the top of a
        // ten-unit underwater slope, so a margin this wide left every coast ringed in bare
        // pale tile — the "edge and nothing inside it" look.
        let openness = smoothstep(0.0, 14.0, max(sea, 0.0));

        // The tile still underlies it — this is OUR sea, not a generic blue — but it is the
        // BED seen through water now rather than the surface itself, so it darkens with
        // depth and the surface terms below sit on top.
        let drift = (1.0 - frozen_of(here_biome)) * t;
        var water = water_color(here_biome, wuv + vec2<f32>(drift * 0.004, drift * 0.006));
        // Open water is BLUE-GREEN, not grey. Multiplying the bed's tile toward a neutral
        // slate is what made the sea read as wet concrete: the tile carries its own hue and
        // a desaturated multiplier drags everything toward it. Keeping green well above red
        // holds the sea on the cyan side of the ground it borders, which is what separates
        // water from wet sand at a glance.
        let body_tint = mix(
            deep_tint_of(here_biome),
            fresh_tint_of(here_biome, fresh_wv.y),
            is_fresh,
        );
        let deep_tint = mix(vec3<f32>(1.0, 1.0, 1.0), body_tint, openness);
        water = vec4<f32>(water.rgb * deep_tint, 1.0);

        // The surface: a wave normal, steeper out in open water than in the shallows where
        // the bed drags. Ripples are per-pixel, so their size does not depend on how the
        // ground plane is tessellated.
        //
        // ⚠️ EXCEPT WHERE THE SEA IS FROZEN. A tundra coast draws with the ice tile, and ice
        // does not swell — the same rule the frozen ponds follow in `water_surface.wgsl`. The
        // shore keeps its foam line (a frozen sea still meets the land somewhere) but the
        // surface holds still, so the ice fields read as solid rather than as a blue ocean
        // wearing a white texture.
        let frozen = frozen_of(here_biome);
        let n_water = mix(
            water_normal(wxz2, t, mix(0.12, 0.55, openness)),
            vec3<f32>(0.0, 1.0, 0.0),
            frozen,
        );

        // Fresnel against the actual view vector — the reason water reads as water at a
        // glancing angle and as its own depth from overhead.
        // Exponent 3, not 5: a physical Fresnel is nearly nothing until the view is very
        // glancing, and our camera looks DOWN at a fixed pitch — a true curve leaves the sea
        // matte from every angle the player actually has. This is a look, not a measurement.
        let fres = pow(clamp(1.0 - max(dot(n_water, pbr_input.V), 0.0), 0.0, 1.0), 3.0);
        // We have no skybox to sample, so the sky is reconstructed from the same colour the
        // frame is cleared to, brightened toward the horizon.
        //
        // ⚠️ AND THE REFLECTION IS A GLAZE, NOT A COAT. This mixed `fres * 0.9` of a
        // near-WHITE sky over the water, and our camera's fixed 22-30 degree pitch keeps
        // the view permanently glancing — so `fres` sits around 0.5-0.6 the whole time and
        // open water came out a flat slate `(0.44, 0.59, 0.65)` instead of the deep
        // `(0.07, 0.26, 0.42)` the depth tint had just computed one line above. The entire
        // depth ramp was being painted over by a colour a fragment away from white.
        //
        // What made this hard to catch is that it does not look BROKEN, it looks HAZY: the
        // wave normals still perturb `fres`, so the swell survives as faint pale streaks in
        // the reflection. Water with visible ripples that is nonetheless the wrong colour
        // reads as "the sea is far away", not as a bug — and it read that way through
        // several passes that went looking at fog, at depth, and at mesh density instead.
        // It was found by rendering the sea's UNLIT BASE COLOUR, which is the only way to
        // tell a bad surface from bad lighting on a good one; every probe that measured the
        // lit frame was measuring the two multiplied together.
        let sky = mix(vec3<f32>(0.30, 0.48, 0.70), vec3<f32>(0.62, 0.76, 0.92), fres);
        water = vec4<f32>(
            mix(water.rgb, sky, fres * 0.30 * openness * sky_reflect_of(here_biome)),
            1.0,
        );

        // Hand the wave normal to the PBR pass so the SUN does the specular. A hand-rolled
        // glint would not track the day/night cycle; this one is lit by the same light
        // everything else is, and goes out at dusk because the sun does.
        // ⚠️ THE CHOP PERTURBS THE SWELL, IT DOES NOT REPLACE IT. This used to `mix` the
        // geometric normal toward the ripple normal, which at full openness threw the
        // surface normal away entirely — harmless while the sea was a flat plane (the
        // normal it discarded was straight up), and destructive the moment the swell became
        // real geometry: every wave the vertex stage had just displaced would have shaded
        // as though it were still flat, which is the exact "texture on a sheet" look the
        // displacement is there to end. Adding the ripple's lateral slope keeps both.
        pbr_input.N = normalize(pbr_input.N + vec3<f32>(n_water.x, 0.0, n_water.z) * openness);
        pbr_input.world_normal = pbr_input.N;
        pbr_input.material.perceptual_roughness =
            mix(pbr_input.material.perceptual_roughness, 0.06, openness);
        // 0.55 is mirror-bright — water's real F0 is nearer 0.02, and a smooth surface at
        // that reflectance adds a second pale wash on top of the glaze above.
        pbr_input.material.reflectance = mix(pbr_input.material.reflectance, vec3<f32>(0.22), openness);

        // Foam where the waves break on the shore: the waterline, modulated by the wave
        // field so it moves with the swell instead of ringing the coast at a fixed radius.
        let swell = wave_height(wxz2 * 1.7, t * 1.3) * 0.5;
        let surf = 1.0 - smoothstep(0.0, 3.2, abs(sea - swell));
        let wet = mix(water, vec4<f32>(0.86, 0.93, 0.96, 1.0), surf * 0.55);
        blended = mix(blended, wet, clamp(smoothstep(-0.5, 2.5, sea), 0.0, 1.0));
    }

    // The tell. The ground inside the doomed ring burns, brightest at the two edges so
    // the boundary is a LINE you can see and run across rather than a vague glow — the
    // whole point of warning you is that leaving has to be a thing you can aim at.
    // The doomed patch: a radius band AND a bearing wedge. Burning the whole annulus told
    // every party at that depth to run from a Shift that was only coming for a wedge of it.
    let shift_bearing = atan2(in.world_position.z, in.world_position.x);
    let in_wedge = params.shift_arc.y <= 0.0
        || abs(shift_bearing - params.shift_arc.x) <= params.shift_arc.y;
    if (in_wedge && params.shift.z > 0.0 && r >= params.shift.x && r < params.shift.y) {
        let edge = min(r - params.shift.x, params.shift.y - r);
        let lip = 1.0 - smoothstep(0.0, 7.0, edge);
        let k = clamp(params.shift.z * (0.30 + 0.70 * lip), 0.0, 0.92);
        blended = mix(blended, vec4<f32>(1.0, 0.40, 0.10, blended.a), k);
    }

    // Whole-stretch light and shade over the LAND only — applied here, after the shore and
    // the water have had their say, because tinting open sea by a ground-noise field is
    // how an ocean ends up looking like a badly-lit field. Under water the factor is 1.
    let tone = mix(macro_tone(in.world_position.xz), 1.0, clamp(smoothstep(-1.0, 2.0, sea), 0.0, 1.0));
    blended = vec4<f32>(blended.rgb * tone, blended.a);

    // ⚠️ THE SPAN WINS OUTRIGHT, and it is applied LAST for that reason: a bridge is a built
    // thing standing over the water, so neither the shore blend nor the sea tint may show
    // through it. Sampled at a fixed scale rather than the biome's `uv_scale`, so flagstones
    // read as flagstones instead of stretching with whatever ground they cross.
    if (span_paint.z > 0.5) {
        let deck_uv = in.world_position.xz * 0.34;
        let deck = ground_sample(t_bridge_deck, deck_uv);
        let rail = ground_sample(t_bridge_parapet, deck_uv);
        blended = mix(deck, rail, span_paint.y);
    }
    pbr_input.material.base_color = pbr_input.material.base_color * blended;

    var out: FragmentOutput;
    out.color = apply_pbr_lighting(pbr_input);
    out.color = main_pass_post_lighting_processing(pbr_input, out.color);
    return out;
}
