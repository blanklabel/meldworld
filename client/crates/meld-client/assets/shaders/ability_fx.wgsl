// THE IMPACT A BLOW LEAVES — one shader for every element in the game.
//
// A camera-facing quad is spawned over whoever was struck and lives for its own TTL;
// this paints what lands on it. ONE shader with a `kind` uniform rather than one per
// element, for the reason a registry beats a list everywhere else in this repo: sixteen
// shaders is sixteen things to keep in step, and the day someone adds a damage type the
// fifteen that already exist tell you nothing about what the new one should look like.
//
// Everything here is procedural. A pixel-art game could sprite these instead, and the
// trade is deliberate: a shader costs no art pipeline, scales to any resolution, and —
// the part that matters for HD-2D — composites as LIGHT over the billboards rather than
// as a cut-out rectangle in front of them.
//
// ⚠️ ADDITIVE, AND THAT IS WHY IT NEVER EATS THE SPRITE. The whole trap this repo has hit
// three times (`sprite_material`'s note) is a full-quad effect painting over the art. An
// impact writes `rgb * a` with `a` falling to zero everywhere it is not drawing, so the
// hero under it survives — see `AlphaMode::Add` on the Rust side.

#import bevy_pbr::forward_io::VertexOutput

struct FxParams {
    // x = kind (see KIND_* below), y = age 0..1, z = per-cast seed, w = strength 0..1.
    params: vec4<f32>,
    // The element's colour. `a` is an overall opacity the CPU fades at the tail.
    tint: vec4<f32>,
};

@group(#{MATERIAL_BIND_GROUP}) @binding(100) var<uniform> fx: FxParams;

const KIND_PHYSICAL: f32 = 0.0;
const KIND_FIRE:     f32 = 1.0;
const KIND_ICE:      f32 = 2.0;
const KIND_LIGHTNING:f32 = 3.0;
const KIND_WATER:    f32 = 4.0;
const KIND_WIND:     f32 = 5.0;
const KIND_EARTH:    f32 = 6.0;
const KIND_MIND:     f32 = 7.0;
const KIND_POISON:   f32 = 8.0;
const KIND_HOLY:     f32 = 9.0;
const KIND_SHADOW:   f32 = 10.0;

fn hash21(p: vec2<f32>) -> f32 {
    var q = fract(p * vec2<f32>(123.34, 456.21));
    q += dot(q, q + 45.32);
    return fract(q.x * q.y);
}

fn noise(p: vec2<f32>) -> f32 {
    let i = floor(p);
    let f = fract(p);
    let u = f * f * (3.0 - 2.0 * f);
    let a = hash21(i);
    let b = hash21(i + vec2<f32>(1.0, 0.0));
    let c = hash21(i + vec2<f32>(0.0, 1.0));
    let d = hash21(i + vec2<f32>(1.0, 1.0));
    return mix(mix(a, b, u.x), mix(c, d, u.x), u.y);
}

/// Four octaves is enough for flame at this size and cheap enough to run on every
/// impact in a five-enemy sweep.
fn fbm(p: vec2<f32>) -> f32 {
    var v = 0.0;
    var amp = 0.5;
    var q = p;
    for (var i = 0; i < 4; i = i + 1) {
        v += amp * noise(q);
        q *= 2.02;
        amp *= 0.5;
    }
    return v;
}

/// A ring that starts at the centre and expands, thinning as it goes. The shape most of
/// these are built from — an impact reads as "something happened HERE and spread".
fn shock_ring(r: f32, t: f32, width: f32) -> f32 {
    let edge = t;
    let d = abs(r - edge);
    return smoothstep(width, 0.0, d) * (1.0 - t);
}

@fragment
fn fragment(in: VertexOutput) -> @location(0) vec4<f32> {
    let kind = fx.params.x;
    let t = clamp(fx.params.y, 0.0, 1.0);
    let seed = fx.params.z;
    let strength = fx.params.w;

    // Centred, y up. The quad is a unit square, so this is -1..1 across the impact.
    let uv = (in.uv - vec2<f32>(0.5, 0.5)) * vec2<f32>(2.0, -2.0);
    let r = length(uv);
    let ang = atan2(uv.y, uv.x);

    var a = 0.0;
    var col = fx.tint.rgb;

    if (kind == KIND_FIRE) {
        // A plume: noise scrolling UPWARD, narrowing with height, hottest at the base.
        //
        // ⚠️ THE FREQUENCY IS THE WHOLE EFFECT. The first cut sampled at 3.0 and the four
        // fbm octaves resolved to two soft blobs the size of the quad — an orange cloud
        // that hid the sprite it was meant to be burning. Fire needs FILAMENTS, so the
        // scale is high and the result is gamma-sharpened rather than used raw.
        let rise = t * 1.6;
        let n = fbm(vec2<f32>(uv.x * 9.0 + seed, uv.y * 6.0 - rise * 7.0 + seed));
        // Sharpen: `pow` pushes the mid-tones down and leaves the peaks, which turns a
        // smooth cloud into tongues. Without it there is no visible flame structure at all.
        let tongues = pow(clamp(n * 1.35, 0.0, 1.0), 2.6);
        // The body of the flame, wide at the feet and pinched at the top.
        let waist = 1.0 - clamp((uv.y + 0.6) * 0.55, 0.0, 1.0) * 0.6;
        let body = smoothstep(0.8, 0.1, abs(uv.x) / max(waist, 0.12) + (n - 0.5) * 0.9);
        let height = smoothstep(1.15, -0.85, uv.y - rise * 0.5);
        a = body * height * tongues * 3.2;
        // Hot core → cooler edge. White at the base is what sells heat — but only a
        // LITTLE of it: mixed all the way to white the flame stops being orange, and an
        // additive white over a sprite is the silhouette-erasing failure again.
        col = mix(fx.tint.rgb, vec3<f32>(1.0, 0.95, 0.72), clamp(a * 0.4, 0.0, 0.55));
        a += shock_ring(r, t, 0.16) * 0.55;
    } else if (kind == KIND_ICE) {
        // Crystal: hard angular spokes that STOP growing and then shatter outward.
        let spokes = 7.0;
        let facet = abs(cos(ang * spokes + seed * 6.28));
        let grow = smoothstep(0.0, 0.35, t);
        let burst = smoothstep(0.55, 1.0, t);
        let reach = mix(0.35, 1.0, grow) + burst * 0.5;
        let shard = smoothstep(reach, reach - 0.5, r) * pow(facet, 3.0);
        a = shard * (1.0 - burst * 0.8) * 2.0;
        a += shock_ring(r, t, 0.1) * 0.7;
        col = mix(fx.tint.rgb, vec3<f32>(0.92, 0.99, 1.0), pow(facet, 6.0));
    } else if (kind == KIND_LIGHTNING) {
        // A bolt down the middle, re-struck a few times over the life of the effect.
        // The flicker is what makes it read as electricity rather than a white bar.
        let strike = fract(t * 3.0);
        let jitter = (fbm(vec2<f32>(uv.y * 6.0 - seed, floor(t * 3.0) * 7.0)) - 0.5) * 0.85;
        let bolt = smoothstep(0.16, 0.0, abs(uv.x - jitter));
        let flick = 1.0 - smoothstep(0.0, 0.7, strike);
        a = bolt * flick * 2.4;
        // A branch, half the width and offset, so it is not one clean stroke.
        let j2 = (fbm(vec2<f32>(uv.y * 9.0 + seed * 3.0, 11.0)) - 0.5) * 1.3;
        a += smoothstep(0.07, 0.0, abs(uv.x - j2)) * flick * 1.1;
        a += shock_ring(r, t, 0.16) * 0.6;
        col = mix(fx.tint.rgb, vec3<f32>(1.0, 1.0, 1.0), 0.55);
    } else if (kind == KIND_WATER) {
        // Two rings out of phase: a splash is never one wave.
        a = shock_ring(r, t, 0.2) * 1.4 + shock_ring(r, clamp(t * 1.5, 0.0, 1.0), 0.12) * 0.8;
        let ripple = sin(r * 22.0 - t * 14.0) * 0.5 + 0.5;
        a *= 0.45 + ripple * 0.75;
    } else if (kind == KIND_WIND) {
        // A swirl: the angle sheared by the radius, so it reads as rotation.
        let swirl = sin(ang * 3.0 + r * 9.0 - t * 12.0 + seed) * 0.5 + 0.5;
        a = swirl * smoothstep(1.0, 0.15, r) * smoothstep(0.0, 0.2, t) * (1.0 - t) * 1.6;
    } else if (kind == KIND_EARTH) {
        // A dust puff with grit in it: low, wide, and it SETTLES rather than rising.
        let n = fbm(vec2<f32>(uv.x * 4.0 + seed, uv.y * 4.0 - t));
        let low = smoothstep(0.9, -0.4, uv.y + t * 0.4);
        a = n * low * smoothstep(1.1, 0.2, r) * (1.0 - t) * 2.2;
        col = mix(fx.tint.rgb, vec3<f32>(0.45, 0.35, 0.26), n);
    } else if (kind == KIND_MIND) {
        // Concentric rings warped by noise — a pressure wave you feel rather than see.
        let warp = fbm(vec2<f32>(uv * 3.0 + seed)) * 0.3;
        let rings = sin((r + warp) * 18.0 - t * 16.0) * 0.5 + 0.5;
        a = pow(rings, 2.5) * smoothstep(1.0, 0.1, r) * (1.0 - t) * 1.8;
    } else if (kind == KIND_POISON) {
        // Bubbles rising out of a low pool: blobby, slow, and it lingers.
        let n = fbm(vec2<f32>(uv.x * 5.0 + seed, uv.y * 5.0 - t * 2.5));
        a = smoothstep(0.55, 0.85, n) * smoothstep(1.05, 0.2, r) * (1.0 - t * 0.7) * 2.0;
    } else if (kind == KIND_HOLY) {
        // A column of light with a hard flare at the moment it lands.
        let beam = smoothstep(0.42, 0.0, abs(uv.x)) * smoothstep(1.4, -0.6, uv.y);
        let flare = (1.0 - smoothstep(0.0, 0.25, t)) * smoothstep(0.9, 0.0, r);
        a = (beam * (1.0 - t) + flare) * 2.0;
        col = mix(fx.tint.rgb, vec3<f32>(1.0, 1.0, 0.92), 0.5);
    } else if (kind == KIND_SHADOW) {
        // Tendrils reaching IN rather than a burst going out — the one effect here that
        // converges, because that is what makes it read as wrong.
        let tend = abs(sin(ang * 5.0 + fbm(vec2<f32>(uv * 2.5 + seed)) * 4.0));
        let close = mix(1.15, 0.15, t);
        a = smoothstep(close, close - 0.55, r) * pow(tend, 2.0) * 1.9;
        col = mix(fx.tint.rgb, vec3<f32>(0.05, 0.0, 0.09), 0.35);
    } else {
        // PHYSICAL: a short bright slash across the impact plus a tight spark ring. No
        // element, so it must not look like a spell — it is over in a few frames.
        let slash = smoothstep(0.17, 0.0, abs(uv.y * 0.72 + uv.x * 0.69))
            * smoothstep(1.05, 0.25, r);
        a = slash * (1.0 - t) * 2.2 + shock_ring(r, t, 0.13) * 1.1;
        // A handful of sparks thrown along the cut.
        let sp = hash21(floor(uv * 7.0) + seed);
        a += step(0.93, sp) * (1.0 - t) * smoothstep(1.0, 0.2, r) * 1.6;
        col = mix(fx.tint.rgb, vec3<f32>(1.0, 1.0, 0.9), 0.4);
    }

    // Every kind fades to nothing at its own edge, so the QUAD is never visible — only
    // what is drawn inside it. Without this the effect is a bright square.
    a *= smoothstep(1.15, 0.55, r);
    a *= fx.tint.a * strength;
    // ⚠️ CAPPED. An impact sits directly over a sprite, and an additive layer over 1.0 is
    // a white-out of the art underneath — the exact failure `sprite_material`'s note
    // records for emissive on a textured quad, arriving by a different route. Every branch
    // above is free to author whatever intensity reads best; the cap is what keeps the
    // creature you are hitting visible while you hit it.
    a = clamp(a, 0.0, 0.85);
    return vec4<f32>(col * a, a);
}
