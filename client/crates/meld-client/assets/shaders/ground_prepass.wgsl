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

// How far INTO the sea a point is, in world units (negative on land). Mirrors
// `meld_proto::coast::is_ocean` but signed, so the shoreline can fade instead of snapping
// to a hard edge one texel wide.
fn sea_depth_at(wxz: vec2<f32>) -> f32 {
    let arc_half = params.coast.x;
    if (arc_half <= 0.0) { return -1000.0; }          // corridor mode: no gap, no sea
    let theta = abs(atan2(wxz.y, wxz.x));
    if (theta <= arc_half) { return -1000.0; }        // inside the fan: land, always
    let d = length(wxz);
    let inland = params.coast.y - d;                  // the neck's land bridge
    if (inland >= 0.0) { return -max(inland, 0.001); }
    return abs(wxz.y) - spit_half_width(d);
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
    let t = smoothstep(-6.0, 10.0, sea);
    // …and then the SWELL rides on top of that level, as real displaced geometry rather
    // than as a normal (see `sea_swell`). Faded in on the same 0..26 ramp the fragment
    // shader calls `openness`, so the waterline itself stays flat and the beach does not
    // develop a heaving edge; the sea only starts to breathe once it is properly open.
    let open = smoothstep(0.0, 26.0, max(sea, 0.0));
    let swell = sea_swell(wxz, params.sea_anim.x) * open;
    return params.terrain_amp * (mix(land, level, t) + swell);
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
