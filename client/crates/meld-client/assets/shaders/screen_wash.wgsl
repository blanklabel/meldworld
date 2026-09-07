// THE SCREEN ANSWERING A BIG BLOW — the Dragon Quest colour wash.
//
// Drawn on one quad parented to the battle camera, a fixed distance in front of it, so it
// covers the view without touching the render graph. That is a deliberate choice over a
// real post-process node: a `ViewNode` is a lot of machinery to own, and everything wanted
// here (a colour wash, a vignette that closes, a shockwave ring, a wobble) is achievable
// on a camera-locked quad that despawns with the fight. Cleanup is then the same
// `despawn::<BattleFxRoot>` every other battle entity uses, rather than a graph edge
// somebody has to remember to remove.
//
// ⚠️ ADDITIVE, like the impacts. A wash that could go opaque would be a screen-wipe bug
// waiting for a tuning mistake; additive means the worst case is "too bright", never
// "the fight disappeared".

#import bevy_pbr::forward_io::VertexOutput

struct WashParams {
    // x = kind (matches ability_fx's KIND_*), y = age 0..1, z = seed, w = strength 0..1.
    params: vec4<f32>,
    tint: vec4<f32>,
};

@group(#{MATERIAL_BIND_GROUP}) @binding(100) var<uniform> wash: WashParams;

fn hash21(p: vec2<f32>) -> f32 {
    var q = fract(p * vec2<f32>(123.34, 456.21));
    q += dot(q, q + 45.32);
    return fract(q.x * q.y);
}

@fragment
fn fragment(in: VertexOutput) -> @location(0) vec4<f32> {
    let t = clamp(wash.params.y, 0.0, 1.0);
    let strength = wash.params.w;
    let seed = wash.params.z;
    let uv = (in.uv - vec2<f32>(0.5, 0.5)) * 2.0;
    let r = length(uv);

    // ⚠️ THE TINT IS NORMALISED BEFORE IT IS USED, and that is not cosmetic. The element
    // colours are authored past 1.0 so a small ADDITIVE impact blooms; multiplied across
    // the whole frame that is a 2-3x screen-wide add, which paints the arena out — the
    // first cut of this shader washed the entire battle flat orange and the sprites went
    // with it. Normalising keeps the element's HUE and hands the INTENSITY to the budget
    // below, so picking a hotter colour can never make the wash stronger.
    let hue = wash.tint.rgb / max(max(max(wash.tint.r, wash.tint.g), wash.tint.b), 0.001);

    // THE FLASH. Front-loaded hard and gone fast — a wash that fades in is a filter, a
    // wash that fades OUT is an impact. This is the half a player actually registers.
    let flash = pow(1.0 - t, 3.0);

    // A shockwave leaving the centre of the screen, so a party-wide blast reads as coming
    // from the field rather than from the UI. The loudest of the three, because it is the
    // only one that carries a DIRECTION.
    let ring = smoothstep(0.22, 0.0, abs(r - t * 1.6)) * (1.0 - t);

    // The edges close in while it holds — the "you are being hit" vignette. Kept to the
    // rim (0.75 in) so it frames the fight instead of tinting it: the middle of the screen
    // is where the fight is, and this must never be the reason you cannot read it.
    let vignette = smoothstep(0.75, 1.6, r) * (1.0 - t);

    // A little grain so a full-screen colour does not band on a gradient sky.
    let grain = (hash21(in.uv * 512.0 + seed) - 0.5) * 0.02;

    // ⚠️ THE BUDGET, and it is deliberately tiny. `WASH_PEAK` is the most of the frame
    // this may ever add — at 0.22 an apocalypse is a clear flash you cannot miss and the
    // arena stays fully readable underneath it. The centre term is the smallest of the
    // three on purpose; the ring and the rim do the work, since both leave the middle of
    // the screen alone.
    const WASH_PEAK: f32 = 0.22;
    let shape = flash * 0.35 + ring * 0.75 + vignette * 0.55 + grain;
    let a = clamp(shape * strength * wash.tint.a, 0.0, 1.0) * WASH_PEAK;
    return vec4<f32>(hue * a, a);
}
