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
    return params.terrain_amp * mix(land, level, t);
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

// The tinted ground colour for biome index `bi` at `uv`. Tints make each biome read
// distinctly under the cool ambient: forest/desert as-authored, Ashfall a charred
// burnt-red with ember-glow crevices, Tundra a cold frost-blue, Mire a sickly green.
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

// Summed directional waves. Each octave is rotated off the last so the crests never line
// up into a visible grid, which is the tell that gives away cheap procedural water.
fn wave_height(p_in: vec2<f32>, t: f32) -> f32 {
    var q = p_in;
    var h = 0.0;
    var amp = 1.0;
    var freq = 0.075;
    for (var i = 0; i < 4; i = i + 1) {
        let a = sin(q.x * freq + t * 1.05) * cos(q.y * freq * 0.87 - t * 0.71);
        let b = sin((q.x + q.y) * freq * 1.31 - t * 0.93);
        h = h + (a + b * 0.6) * amp;
        amp = amp * 0.5;
        freq = freq * 1.93;
        // ~16 degrees per octave.
        q = vec2<f32>(q.x * 0.961 - q.y * 0.276, q.x * 0.276 + q.y * 0.961);
    }
    return h;
}

// The surface normal, by finite-differencing the same field. `steep` scales how much the
// slope tilts the normal — the wave field is unitless, so this is where it becomes a look.
fn water_normal(p: vec2<f32>, t: f32, steep: f32) -> vec3<f32> {
    let e = 0.75;
    let h = wave_height(p, t);
    let hx = wave_height(p + vec2<f32>(e, 0.0), t);
    let hz = wave_height(p + vec2<f32>(0.0, e), t);
    return normalize(vec3<f32>((h - hx) * steep, e, (h - hz) * steep));
}

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
    let sea = sea_depth_at(in.world_position.xz);
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
        let openness = smoothstep(0.0, 26.0, max(sea, 0.0));

        // The tile still underlies it — this is OUR sea, not a generic blue — but it is the
        // BED seen through water now rather than the surface itself, so it darkens with
        // depth and the surface terms below sit on top.
        var water = water_color(here_biome, wuv + vec2<f32>(t * 0.004, t * 0.006));
        // Open water is BLUE-GREEN, not grey. Multiplying the bed's tile toward a neutral
        // slate is what made the sea read as wet concrete: the tile carries its own hue and
        // a desaturated multiplier drags everything toward it. Keeping green well above red
        // holds the sea on the cyan side of the ground it borders, which is what separates
        // water from wet sand at a glance.
        let deep_tint = mix(vec3<f32>(1.0, 1.0, 1.0), vec3<f32>(0.09, 0.30, 0.46), openness);
        water = vec4<f32>(water.rgb * deep_tint, 1.0);

        // The surface: a wave normal, steeper out in open water than in the shallows where
        // the bed drags. Ripples are per-pixel, so their size does not depend on how the
        // ground plane is tessellated.
        let n_water = water_normal(wxz2, t, mix(0.12, 0.55, openness));

        // Fresnel against the actual view vector — the reason water reads as water at a
        // glancing angle and as its own depth from overhead.
        // Exponent 3, not 5: a physical Fresnel is nearly nothing until the view is very
        // glancing, and our camera looks DOWN at a fixed pitch — a true curve leaves the sea
        // matte from every angle the player actually has. This is a look, not a measurement.
        let fres = pow(clamp(1.0 - max(dot(n_water, pbr_input.V), 0.0), 0.0, 1.0), 3.0);
        // We have no skybox to sample, so the sky is reconstructed from the same colour the
        // frame is cleared to, brightened toward the horizon.
        let sky = mix(vec3<f32>(0.46, 0.66, 0.90), vec3<f32>(0.84, 0.92, 1.0), fres);
        water = vec4<f32>(mix(water.rgb, sky, fres * 0.9 * openness), 1.0);

        // Hand the wave normal to the PBR pass so the SUN does the specular. A hand-rolled
        // glint would not track the day/night cycle; this one is lit by the same light
        // everything else is, and goes out at dusk because the sun does.
        pbr_input.N = normalize(mix(pbr_input.N, n_water, openness));
        pbr_input.world_normal = pbr_input.N;
        pbr_input.material.perceptual_roughness =
            mix(pbr_input.material.perceptual_roughness, 0.06, openness);
        pbr_input.material.reflectance = mix(pbr_input.material.reflectance, vec3<f32>(0.55), openness);

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
