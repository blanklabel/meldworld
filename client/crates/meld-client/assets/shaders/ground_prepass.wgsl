// The ground's SHADOW / DEPTH vertex stage.
//
// ⚠️ THIS FILE IS WHY THE TERRAIN NO LONGER SHADOWS ITSELF. `MaterialExtension` takes the
// prepass vertex stage from `prepass_vertex_shader()` — a DIFFERENT hook from
// `vertex_shader()`. Override only the latter, which is what shipped, and the shadow map is
// rasterized from the UNDISPLACED plane while the visible ground rolls into hills: every
// part of the real ground below that flat sheet fails the shadow test. Measured, the mire's
// ground read 21.9 mean luminance against 157.8 once it stopped, and raising the sun from
// 9,200 to 21,000 lux had moved it by 1.1 — the light was never the problem.
//
// ⚠️ IT IS A SEPARATE FILE, AND THE FIELD BELOW IS A DELIBERATE COPY. Two things forced it:
// Bevy refuses two `@vertex` entry points in one module ("multiple entry points were found
// ... but no entry point was specified"), and moving the field into an imported library fails
// at pipeline creation with "Bindings for [32] conflict with other resource" — a material
// uniform declared inside an imported module collides rather than resolving.
//
// So the duplication is CHECKED rather than trusted: `the_two_ground_shaders_share_one_height_field`
// lifts this block out of both files and asserts they are byte-identical. That is the same
// discipline the field already lives under against `meld_proto::terrain` and
// `world_render::terrain_height` — the ground you see must be the ground you walk on, and now
// also the ground that casts.
//
// ==== SHARED FIELD: byte-identical to ground_biome.wgsl, enforced by test ====

#import bevy_pbr::{
    mesh_functions,
    prepass_io::{Vertex, VertexOutput},
    view_transformations::position_world_to_clip,
}
#import meld::water_wave::sea_swell

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

@group(#{MATERIAL_BIND_GROUP}) @binding(106) var<uniform> params: BiomeParams;

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

// ⚠️ THERE IS DELIBERATELY NO `inland_depth_at` HERE. This file is the ground's DEPTH and
// SHADOW pass — vertex-stage only — and inland water must never displace the ground: a basin
// sits at its own elevation and its hollow is already in the heightmap, so dipping it would
// excavate every lake below its own bed. Painting inland water is a fragment-stage job and
// lives in `ground_biome.wgsl` alone. The uniform still declares `basins`/`rivers` because
// the two files must agree on the buffer LAYOUT, not on what they read from it.

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


// ==== END SHARED FIELD ====

@vertex
fn vertex(vertex: Vertex) -> VertexOutput {
    var out: VertexOutput;
    let world_from_local = mesh_functions::get_world_from_local(vertex.instance_index);
    var world_position = mesh_functions::mesh_position_local_to_world(
        world_from_local, vec4<f32>(vertex.position, 1.0));
    // THE LINE THIS WHOLE FILE EXISTS FOR — identical to the main pass, so the depth the
    // shadow map records is the depth of the ground that actually gets drawn.
    world_position.y += total_height(world_position.xz);
    out.world_position = world_position;
    out.position = position_world_to_clip(world_position.xyz);
#ifdef UNCLIPPED_DEPTH_ORTHO_EMULATION
    out.unclipped_depth = out.position.z;
    out.position.z = min(out.position.z, 1.0);
#endif
#ifdef VERTEX_UVS_A
    out.uv = vertex.uv;
#endif
#ifdef NORMAL_PREPASS_OR_DEFERRED_PREPASS
    out.world_normal = terrain_normal(world_position.xz);
#endif
#ifdef MOTION_VECTOR_PREPASS
    out.previous_world_position = world_position;
#endif
#ifdef VERTEX_OUTPUT_INSTANCE_INDEX
    out.instance_index = vertex.instance_index;
#endif
    return out;
}
