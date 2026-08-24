// Standing water that is a MESH rather than a stretch of ground: the maze's ponds, bog
// pools and frozen ponds, and Last City's sea.
//
// The open ocean is shaded by `ground_biome.wgsl` instead, because out there the depth is
// analytic (`sea_depth_at`) and there is no water mesh at all. These surfaces have no such
// luxury — a pool basin is half a unit deep and the city's sea plane sits five centimetres
// over its own grass — so depth is taken from the SHAPE instead of measured: a pool is
// deepest at its middle and shallows to nothing at the rim, which is what a basin is.
//
// That is the whole reason this is not a water crate. Every one of them shades by Beer's
// law over the distance from the surface to whatever the depth buffer says is behind it,
// which for our water is a couple of centimetres and resolves to "no water at all".
//
// The wave field is shared with the ocean (`meld::water_wave`) so the two never drift.

#import bevy_pbr::{
    pbr_fragment::pbr_input_from_standard_material,
    pbr_functions::{apply_pbr_lighting, main_pass_post_lighting_processing, alpha_discard},
    forward_io::{VertexOutput, FragmentOutput},
}
#import meld::water_wave::{wave_height, water_normal}

struct WaterSurface {
    /// `(seconds, wave_scale, steepness, mode)` — mode 0 is a basin (deep in the middle),
    /// mode 1 is open water (deep everywhere, e.g. the city's sea planes).
    params: vec4<f32>,
    deep: vec4<f32>,
    shallow: vec4<f32>,
    edge: vec4<f32>,
}

@group(#{MATERIAL_BIND_GROUP}) @binding(100) var<uniform> water: WaterSurface;

@fragment
fn fragment(in: VertexOutput, @builtin(front_facing) is_front: bool) -> FragmentOutput {
    var pbr_input = pbr_input_from_standard_material(in, is_front);

    let t = water.params.x;
    let scale = water.params.y;
    let steep = water.params.z;
    let wxz = in.world_position.xz;

    // Depth from the shape. A basin's UVs run 0..1 across the disc, so distance from the
    // middle IS the shallowing — no depth buffer, no prepass, and correct at any camera
    // angle, which is what the measured version never managed here.
    var depth = 1.0;
    if (water.params.w < 0.5) {
        let d = clamp(length(in.uv - vec2<f32>(0.5, 0.5)) * 2.0, 0.0, 1.0);
        depth = 1.0 - d;
    }
    let openness = smoothstep(0.04, 0.55, depth);

    // ⚠️ THE WATER'S COLOUR IS A COLOUR, NOT A MULTIPLIER. The first version multiplied the
    // bed tile BY `deep`, and since every channel of a deep colour is below 1 that drove
    // deep water toward black — a bog tile at (85, 82, 51) times (0.10, 0.20, 0.09) lands on
    // (9, 16, 5), which is what turned an entire swamp into a void.
    //
    // Water occludes its bed rather than tinting it: shallows show the ground through them,
    // depth replaces it with the body's own colour. So this is a MIX between the two, and
    // the deep colour is what you see when the bed is no longer visible at all.
    let bed = pbr_input.material.base_color.rgb;
    let body = mix(water.shallow.rgb, water.deep.rgb, openness);
    var col = mix(bed, body, openness * 0.85);

    // ⚠️ ICE DOES NOT RIPPLE. `steep <= 0` marks a FROZEN body, and it keeps the mesh's own
    // flat normal instead of a wave one — no crests, no motion. A frozen pond with a swell
    // rolling across it is the single most obviously wrong thing water can do, and it was
    // doing it, because "water" was one material behaviour with three palettes.
    //
    // Everything else about it stays: ice is still smooth, still catches the sky, still has a
    // rim where it thins at the bank. It is a mirror, not a puddle.
    var n_water = in.world_normal;
    if (steep > 0.0) {
        n_water = water_normal(wxz * scale, t, mix(0.10, 1.0, openness) * steep);
    }

    // Fresnel against the real view vector. Exponent 3 rather than a physical 5 for the
    // same reason the ocean uses 3: our camera looks DOWN at a fixed pitch, and a true
    // curve leaves water matte from every angle a player actually has.
    let fres = pow(clamp(1.0 - max(dot(n_water, pbr_input.V), 0.0), 0.0, 1.0), 3.0);
    // Dimmed with the open sea's (see `ground_biome.wgsl`): a near-white sky reflected at
    // our permanently-glancing camera pitch is what made every body of water in the game
    // read as pale slate. A pond keeps MORE of it than the sea does — it is shallow, so the
    // sky on its surface is most of what says "liquid" — but 0.85 of near-white was a
    // mirror, which is why ponds and mires showed an edge and nothing inside it.
    let sky = mix(vec3<f32>(0.30, 0.48, 0.70), vec3<f32>(0.62, 0.76, 0.92), fres);
    // ⚠️ A DARK POOL READS BY WHAT IT REFLECTS. Deep water that is nearly black and matte is
    // indistinguishable from a hole; it is the sky caught on its surface that says "liquid".
    // So the sky term carries more weight here than on the open sea, where depth colour and
    // surf already do that work.
    col = mix(col, sky, fres * 0.45 * openness);

    // A rim where the water thins to nothing — the bank, not a beach.
    let rim = 1.0 - smoothstep(0.0, 0.22, depth);
    col = mix(col, water.edge.rgb, rim * water.edge.a);

    // Hand the wave normal to the PBR pass so the SUN makes the glint; a hand-rolled
    // highlight would not go out at dusk, and this one does.
    pbr_input.N = normalize(mix(pbr_input.N, n_water, openness));
    pbr_input.world_normal = pbr_input.N;
    pbr_input.material.perceptual_roughness =
        mix(pbr_input.material.perceptual_roughness, 0.06, openness);

    pbr_input.material.base_color = vec4<f32>(col, pbr_input.material.base_color.a);
    pbr_input.material.base_color = alpha_discard(pbr_input.material, pbr_input.material.base_color);

    var out: FragmentOutput;
    out.color = apply_pbr_lighting(pbr_input);
    out.color = main_pass_post_lighting_processing(pbr_input, out.color);
    return out;
}
