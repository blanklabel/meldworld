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
    rings: array<vec4<f32>, 32>,
    count: u32,
    uv_scale: f32,
    blend_half: f32,
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
    _pad_pc0: u32, _pad_pc1: u32, _pad_pc2: u32,
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
    basins: array<vec4<f32>, 10>,
    rivers: array<vec4<f32>, 28>,
    basin_count: u32,
    river_count: u32,
    _pad_wc0: u32, _pad_wc1: u32,
    // The Shift's tell (CANON D20/§W2): (inner_radius, outer_radius, intensity, 0).
    // A region is a radius ring in the WG-4 fan and this ground is already painted in
    // rings, so the doomed region draws as an annulus in the same frame as everything
    // else — no second coordinate system to keep in sync. Intensity 0 = nothing pending.
    shift: vec4<f32>,
    // Open-water animation: `(seconds, 0, 0, 0)`. The sea needs a clock and this shader had
    // none — which is why the ocean was a static tile while every pond prop drifted its own
    // material UVs from `animate_water`. A vec4 rather than a bare f32 so it lands 16-byte
    // aligned after `shift` and needs no new padding on either side of the mirror.
    sea_anim: vec4<f32>,
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
// `meld_proto::coast::peninsula_half_width`.
fn spit_half_width(d: f32) -> f32 {
    let neck_reach = params.coast.y;
    let penin_len = params.coast.z;
    let neck_half = params.coast_w.x;
    let city_half = params.coast_w.y;
    let tip_taper = params.coast_w.z;
    if (d <= neck_reach) { return neck_half; }
    if (d >= penin_len) { return 0.0; }
    let t = (d - neck_reach) / (penin_len - neck_reach);
    let swell = sin(3.14159265 * t);
    var w = neck_half + (city_half - neck_half) * swell;
    w = w * smoothstep(1.0, 1.0 - tip_taper, t);
    let gap_half = max(3.14159265 - params.coast.x, 0.0);
    return min(w, d * tan(gap_half) * params.coast.w);
}

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
        let ground = terrain_height_wgsl(wxz + params.terrain_off) + peak_dome(wxz);
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
fn sea_depth_at(wxz: vec2<f32>) -> f32 {
    // LAST CITY IS THE SAME SEA, DRAWN BY THE SAME SHADER. The city is its own scene in
    // its own coordinates and cannot use the world's radial fan (that shoreline, expressed
    // in city space, runs straight through the plaza), so it hands its OWN spit down:
    // `sea_anim.yz` is (shore half-width, tip reach), nonzero only in the City.
    //
    // It used to be three hand-placed water planes instead, sitting a hair ABOVE the lawn
    // because the flat plaza had nothing to dip into — the exact "two hand-placed
    // shorelines that drift" this module was written to prevent, and it had already drifted
    // (the city's sea missed every fix the world's sea got, because they were not the same
    // water). One shoreline, one shader, both scenes.
    if (params.sea_anim.y > 0.0) {
        let past_flank = abs(wxz.x) - params.sea_anim.y;   // out past either flank
        let past_tip = wxz.y - params.sea_anim.z;          // out past the tip (+z)
        return max(past_flank, past_tip);
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
    let past_fan = (theta - arc_half) * d;
    let past_spit = abs(wxz.y) - spit_half_width(d);
    let past_neck = d - params.coast.y;
    var sea = min(min(past_fan, past_spit), past_neck);
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
    let land = terrain_height_wgsl(wxz + params.terrain_off) + peak_dome(wxz);
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

fn biome_color(bi: i32, uv: vec2<f32>) -> vec4<f32> {
    if (bi <= 0) {
        return textureSample(t_forest, samp, uv);
    }
    if (bi == 1) {
        return textureSample(t_desert, samp, uv);
    }
    if (bi == 2) {
        let ash = textureSample(t_ashfall, samp, uv);
        let ember = (1.0 - ash.r) * 0.5; // darkest cracks glow hottest
        return vec4<f32>(ash.rgb * vec3<f32>(0.95, 0.24, 0.18) + vec3<f32>(ember, ember * 0.18, 0.02), ash.a);
    }
    if (bi == 3) {
        return textureSample(t_tundra, samp, uv) * vec4<f32>(0.72, 0.86, 1.15, 1.0);
    }
    // The mire's sour green, left as AUTHORED. This was briefly raised to (1.0, 1.35, 0.85)
    // to compensate for the swamp reading as permanent dusk — but the dusk was the GROUND
    // SHADOWING ITSELF (see `NotShadowCaster` on `WorldGround`), not the tint. Lifting a
    // tint to pay for a lighting bug is how a biome ends up looking like a sunny meadow the
    // moment the real bug is fixed.
    return textureSample(t_mire, samp, uv) * vec4<f32>(0.75, 0.95, 0.7, 1.0);
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

@fragment
fn fragment(in: VertexOutput, @builtin(front_facing) is_front: bool) -> FragmentOutput {
    var pbr_input = pbr_input_from_standard_material(in, is_front);

    let uv = in.world_position.xz * params.uv_scale;
    let r = length(in.world_position.xz);
    let hw = max(params.blend_half, 0.001);

    var blended: vec4<f32>;
    // Hoisted: the SEA needs the biome it borders too, to pick its tile (ice off a tundra
    // shore, bog off a mire one), and `here` is otherwise scoped to the ring branch.
    var here_biome: i32 = 0;
    if (params.count == 0u) {
        // No sections yet (menus): plain forest floor.
        blended = biome_color(0, uv);
    } else {
        // Find the ring containing r: the first whose OUTER radius exceeds r, else the
        // last (deepest known) ring.
        var idx = params.count - 1u;
        for (var i = 0u; i < params.count; i = i + 1u) {
            if (r < params.rings[i].x) {
                idx = i;
                break;
            }
        }
        let prev_i = max(idx, 1u) - 1u;
        let next_i = min(idx + 1u, params.count - 1u);
        let here = i32(params.rings[idx].y);
        here_biome = here;
        let prev = i32(params.rings[prev_i].y);
        let next = i32(params.rings[next_i].y);
        let inner = select(0.0, params.rings[prev_i].x, idx > 0u); // this ring's inner edge
        let outer = params.rings[idx].x;                           // this ring's outer edge
        // Cross-fade toward the previous biome across the inner edge, and toward the
        // next biome across the outer edge (each neighbour ring paints the other half,
        // so transitions are seamless and gradual — a forest fades into desert ahead).
        let s_in = smoothstep(inner - hw, inner + hw, r);
        let s_out = smoothstep(outer - hw, outer + hw, r);
        var c = mix(biome_color(prev, uv), biome_color(here, uv), s_in);
        c = mix(c, biome_color(next, uv), s_out);
        blended = c;
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
    if (params.shift.z > 0.0 && r >= params.shift.x && r < params.shift.y) {
        let edge = min(r - params.shift.x, params.shift.y - r);
        let lip = 1.0 - smoothstep(0.0, 7.0, edge);
        let k = clamp(params.shift.z * (0.30 + 0.70 * lip), 0.0, 0.92);
        blended = mix(blended, vec4<f32>(1.0, 0.40, 0.10, blended.a), k);
    }

    pbr_input.material.base_color = pbr_input.material.base_color * blended;

    var out: FragmentOutput;
    out.color = apply_pbr_lighting(pbr_input);
    out.color = main_pass_post_lighting_processing(pbr_input, out.color);
    return out;
}
