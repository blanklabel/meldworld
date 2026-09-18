// ONE GLYPH OF A COMBATANT'S HP, PROJECTED ONTO ITS OWN GLASS TUBE.
//
// A quad hovering just off the health ring, painting one character onto whatever surface is
// behind it. The projection is ORTHOGRAPHIC ALONG THE QUAD'S OWN NORMAL: for each fragment we
// read the depth prepass, rebuild the world point under that pixel, drop it into the quad's
// local frame, and use the two in-plane coordinates as the glyph's UV. Anything landing outside
// the quad's box is not part of this glyph and is discarded.
//
// ⚠️ **THIS REPLACES `bevy_pbr`'s OWN `ForwardDecal`, AND THE REASON IS ONE LINE OF ITS
// SHADER.** `get_forward_decal_info` recovers the quad's scale as
// `(world_from_local * vec4(1,1,1,0)).xyz` — the SUM OF THE MATRIX'S THREE BASIS COLUMNS, which
// equals `(sx, sy, sz)` only when the rotation is identity. It then divides the tangent by that
// to build a TBN and parallax-corrects the UV in tangent space. For an axis-aligned decal
// stamped on flat ground that is fine, and it is the case the engine ships for.
//
// Every quad here is heavily rotated, and each one differently: they follow the ring's arc AND
// turn to face the camera. So that divisor is an arbitrary vector — measured on a failing glyph
// it came out `(0.141, -0.065, 0.280)`, negative in one axis and an order of magnitude apart
// between them — the TBN is skewed by an amount that depends purely on orientation, and the UV
// correction with it. What that looked like: the glyphs at one end of every number came out
// smeared, then missing, antisymmetrically along the arc, getting worse the further the quad
// stood off its surface and not caring at all about the quad's size. Four rounds of tuning
// constants moved which digits were lost and never fixed it, because it was never a constant.
//
// Projecting ourselves is both correct and SIMPLER: there is no tangent space involved, so
// there is nothing for a rotation to corrupt.

#import bevy_pbr::forward_io::VertexOutput
#import bevy_pbr::mesh_functions::get_world_from_local
#import bevy_pbr::prepass_utils::prepass_depth
#import bevy_pbr::view_transformations::{frag_coord_to_ndc, position_ndc_to_world}

@group(#{MATERIAL_BIND_GROUP}) @binding(0) var glyph: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(1) var glyph_sampler: sampler;

@fragment
fn fragment(in: VertexOutput) -> @location(0) vec4<f32> {
#ifdef DEPTH_PREPASS
    // THE SURFACE UNDER THIS PIXEL. Everything this shader does rests on the depth prepass;
    // without one there is nothing to project onto, which is why the camera grows a
    // `DepthPrepass` for this (see `hd2d::spawn_camera`).
    let depth = prepass_depth(in.position, 0u);
    // Reversed-Z: 0 is the far plane, i.e. nothing was drawn here. A glyph has no business
    // painting the sky.
    if (depth <= 0.0) {
        discard;
    }
    let ndc = vec3<f32>(frag_coord_to_ndc(in.position).xy, depth);
    let world = position_ndc_to_world(ndc);

    // …DROPPED INTO THIS QUAD'S OWN FRAME. The transform is a rotation with a uniform scale, so
    // its inverse is a dot against each column over that column's squared length — no matrix
    // inversion, and no assumption about what the rotation is.
    let m = get_world_from_local(in.instance_index);
    let d = world - m[3].xyz;
    let cx = m[0].xyz;
    let cy = m[1].xyz;
    let u = dot(d, cx) / dot(cx, cx);
    let v = dot(d, cy) / dot(cy, cy);
    // Outside this glyph's own box is some other glyph's business, or none. ⚠️ This is what
    // makes the quads safe to overlap and safe to hang past the tube: a fragment that misses is
    // REJECTED rather than smeared, which is the whole failure mode of the shader this replaces.
    if (abs(u) > 0.5 || abs(v) > 0.5) {
        discard;
    }
    // `v` grows UP the quad and a texture's grows DOWN from its top, so the vertical flips.
    let c = textureSample(glyph, glyph_sampler, vec2<f32>(u + 0.5, 0.5 - v));
    if (c.a < 0.004) {
        discard;
    }
    return c;
#else
    // No prepass on this camera: there is no surface to print on, so print nothing. Silent
    // rather than fatal, because a second camera (a minimap, a render target) must not take the
    // whole material down with it.
    discard;
#endif
}
