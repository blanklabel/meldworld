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
    forward_io::{VertexOutput, FragmentOutput},
}

struct BiomeParams {
    rings: array<vec4<f32>, 32>,
    count: u32,
    uv_scale: f32,
    blend_half: f32,
    _pad0: f32,
}

@group(2) @binding(100) var t_forest: texture_2d<f32>;
@group(2) @binding(101) var t_desert: texture_2d<f32>;
@group(2) @binding(102) var t_ashfall: texture_2d<f32>;
@group(2) @binding(103) var t_tundra: texture_2d<f32>;
@group(2) @binding(104) var t_mire: texture_2d<f32>;
@group(2) @binding(105) var samp: sampler;
@group(2) @binding(106) var<uniform> params: BiomeParams;

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

@fragment
fn fragment(in: VertexOutput, @builtin(front_facing) is_front: bool) -> FragmentOutput {
    var pbr_input = pbr_input_from_standard_material(in, is_front);

    let uv = in.world_position.xz * params.uv_scale;
    let r = length(in.world_position.xz);
    let hw = max(params.blend_half, 0.001);

    var blended: vec4<f32>;
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

    pbr_input.material.base_color = pbr_input.material.base_color * blended;

    var out: FragmentOutput;
    out.color = apply_pbr_lighting(pbr_input);
    out.color = main_pass_post_lighting_processing(pbr_input, out.color);
    return out;
}
