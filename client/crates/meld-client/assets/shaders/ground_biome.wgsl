// Biome-blending ground material (an ExtendedMaterial extension over StandardMaterial).
//
// The overworld floor is a single big plane. Instead of hot-swapping its texture to
// the player's *current* biome (which snaps the whole ground at once when you cross a
// band), this shader picks the biome from the fragment's own WORLD position and
// cross-fades between adjacent biome textures across a band around each boundary — so
// as you approach a border you see the next biome's ground gradually take over ahead
// of you, "corridor" style. Biome is a function of radial distance from the origin,
// matching the server's `biome_for_distance` (forest < 100 < desert < 300 < ashfall <
// 500 < tundra < 1000 < mire).

#import bevy_pbr::{
    pbr_fragment::pbr_input_from_standard_material,
    pbr_functions::{apply_pbr_lighting, main_pass_post_lighting_processing},
    forward_io::{VertexOutput, FragmentOutput},
}

struct BiomeParams {
    // Radial distances of the four band boundaries (forest|desert|ashfall|tundra|mire).
    boundaries: vec4<f32>,
    // World units per texture tile is 1/uv_scale; larger uv_scale = smaller tiles.
    uv_scale: f32,
    // Half-width (world units) of the cross-fade band centred on each boundary.
    blend_half: f32,
    _pad0: f32,
    _pad1: f32,
}

@group(2) @binding(100) var t_forest: texture_2d<f32>;
@group(2) @binding(101) var t_desert: texture_2d<f32>;
@group(2) @binding(102) var t_ashfall: texture_2d<f32>;
@group(2) @binding(103) var t_tundra: texture_2d<f32>;
@group(2) @binding(104) var t_mire: texture_2d<f32>;
@group(2) @binding(105) var samp: sampler;
@group(2) @binding(106) var<uniform> params: BiomeParams;

@fragment
fn fragment(in: VertexOutput, @builtin(front_facing) is_front: bool) -> FragmentOutput {
    // Standard PBR setup (normals, lighting inputs). Base has no colour texture, so
    // this leaves base_color at the material's flat tint — we replace it below.
    var pbr_input = pbr_input_from_standard_material(in, is_front);

    let uv = in.world_position.xz * params.uv_scale;
    let c_forest = textureSample(t_forest, samp, uv);
    let c_desert = textureSample(t_desert, samp, uv);
    // Ashfall reads as CHARRED VOLCANIC GROUND: tint the dirt a hot red-orange and
    // deepen it, with a faint ember glow in the texture's dark crevices, so it never
    // gets mistaken for the forest floor.
    let ash_raw = textureSample(t_ashfall, samp, uv);
    let ember = (1.0 - ash_raw.r) * 0.5; // darkest cracks glow hottest
    // Deep burnt-red charred earth (not orange sand): redden hard, darken, then let
    // the ember term glow the crevices hot — a molten-crack look, clearly volcanic.
    let c_ashfall = vec4<f32>(
        ash_raw.rgb * vec3<f32>(0.95, 0.24, 0.18) + vec3<f32>(ember, ember * 0.18, 0.02),
        ash_raw.a,
    );
    // Tundra reads cold: a frost-blue cast over the dark grass.
    let c_tundra = textureSample(t_tundra, samp, uv) * vec4<f32>(0.72, 0.86, 1.15, 1.0);
    let c_mire = textureSample(t_mire, samp, uv) * vec4<f32>(0.75, 0.95, 0.7, 1.0);

    // Partition-of-unity weights across the four boundaries: each `s` ramps 0->1
    // through its boundary's blend band, so only two adjacent biomes ever overlap.
    let r = length(in.world_position.xz);
    let hw = max(params.blend_half, 0.001);
    let s0 = smoothstep(params.boundaries.x - hw, params.boundaries.x + hw, r);
    let s1 = smoothstep(params.boundaries.y - hw, params.boundaries.y + hw, r);
    let s2 = smoothstep(params.boundaries.z - hw, params.boundaries.z + hw, r);
    let s3 = smoothstep(params.boundaries.w - hw, params.boundaries.w + hw, r);
    let blended = c_forest * (1.0 - s0)
        + c_desert * (s0 - s1)
        + c_ashfall * (s1 - s2)
        + c_tundra * (s2 - s3)
        + c_mire * s3;

    pbr_input.material.base_color = pbr_input.material.base_color * blended;

    var out: FragmentOutput;
    out.color = apply_pbr_lighting(pbr_input);
    out.color = main_pass_post_lighting_processing(pbr_input, out.color);
    return out;
}
