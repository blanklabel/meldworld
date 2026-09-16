// WHAT A FIGHTER CHARGING FASTER THAN IT SHOULD BE LOOKS LIKE.
//
// The turn-order bar's fast rail draws this behind the body: a tongue of flame with a
// white-hot head over the sprite, licking backwards down that fighter's own charge line.
//
// ⚠️ **THIS IS A SHADER BECAUSE THE BAR IS UI, AND UI IS ROUNDED RECTANGLES.** Three cuts
// were built out of `Node`s first and every one of them failed for the same reason: a UI
// node is an alpha-blended box with rounded corners, so a bolt became a torn ribbon, then a
// square wave, and an aura became a stack of lozenges that read as a smudge. None of them
// could do the two things fire actually needs — colour that RAMPS through a gradient, and
// light that ADDS where it overlaps. A `UiMaterial` can do both: this ramps white → the
// line's own colour → nothing, and the Rust side sets an additive blend in `specialize`.
//
// ⚠️ **AND IT IS ONE NODE.** The particle version was forty embers plus seven aura layers
// per fast fighter, all moved by the CPU every frame. This is a single quad; the motion is
// the noise field scrolling inside it.

#import bevy_ui::ui_vertex_output::UiVertexOutput

struct FireParams {
    // x = seconds, y = how far back the tail reaches in uv, z = per-fighter seed,
    // w = where the BODY is along the quad, in uv.x.
    //
    // ⚠️ `w` used to be the aspect ratio, which the quad already knows: `UiVertexOutput`
    // carries the node's pixel `size`. Spending the slot on the body's position instead is
    // what lets the quad extend PAST the icon, so the hot core can fade out inside its own
    // node rather than being sliced off by the right-hand edge.
    params: vec4<f32>,
    // The charge line's own colour, and an overall opacity in `a`. The flame ramps to WHITE
    // at its core rather than to a fixed fire orange, so an ally burns blue-white and a
    // creature burns red-white — the colour still says whose it is.
    tint: vec4<f32>,
};

@group(1) @binding(0) var<uniform> fire: FireParams;

fn hash(p: vec2<f32>) -> f32 {
    let h = dot(p, vec2<f32>(127.1, 311.7));
    return fract(sin(h) * 43758.5453);
}

fn noise(p: vec2<f32>) -> f32 {
    let i = floor(p);
    let f = fract(p);
    // Smoothstep the cell so the field has no grid edges in it.
    let u = f * f * (3.0 - 2.0 * f);
    let a = hash(i);
    let b = hash(i + vec2<f32>(1.0, 0.0));
    let c = hash(i + vec2<f32>(0.0, 1.0));
    let d = hash(i + vec2<f32>(1.0, 1.0));
    return mix(mix(a, b, u.x), mix(c, d, u.x), u.y);
}

// Four octaves is enough for a tongue of flame at this size; more is invisible detail paid
// for on every pixel of every fast fighter's line.
fn fbm(p: vec2<f32>) -> f32 {
    var v = 0.0;
    var amp = 0.5;
    var q = p;
    for (var i = 0; i < 4; i = i + 1) {
        v = v + amp * noise(q);
        q = q * 2.02 + vec2<f32>(1.7, 9.2);
        amp = amp * 0.5;
    }
    return v;
}

@fragment
fn fragment(in: UiVertexOutput) -> @location(0) vec4<f32> {
    let t = fire.params.x;
    let fill = max(fire.params.y, 0.02);
    let seed = fire.params.z;
    let body = fire.params.w;
    // The node knows its own pixel size, so the noise can be sampled in a space that is
    // square on screen without the CPU telling it the aspect.
    let aspect = max(in.size.x / max(in.size.y, 1.0), 1.0);

    // How far BEHIND the body this pixel is; negative in front of it.
    let back = body - in.uv.x;
    // Distance from the rail the fighter is travelling along, 1.0 at the quad's edge.
    let off = abs(in.uv.y - 0.5) * 2.0;

    // ⚠️ **SAMPLE FINE ENOUGH TO SEE.** The first cut sampled at `aspect * 0.55`, which put
    // about four noise cells across a four-hundred-pixel quad — so the "fire" was a smooth
    // airbrushed cone with no structure in it at all. Tongues need cells you can count.
    var p = vec2<f32>(in.uv.x * aspect * 2.4, in.uv.y * 5.0);
    // Scroll BACKWARDS: the fire is being left behind by something moving right.
    p.x = p.x + t * 3.0 + seed * 13.0;
    // …and warp the sample by a slower second field, which is what turns bands of noise
    // into licking tongues instead of a scrolling texture.
    let warp = fbm(p * 0.5 + vec2<f32>(0.0, t * 0.8));
    let n = fbm(p + vec2<f32>(0.0, warp * 1.4));

    // The envelope: full at the body, gone `fill` behind it, and fading quickly in FRONT of
    // it so the head is a rounded nose rather than a cut edge.
    // ⚠️ **THE NOSE AND THE CORE ARE SIZED IN PIXELS, NOT IN UV.** This quad's width is the
    // fighter's own charge line, so it grows from ~30px at the start of the track to ~600 at
    // the GO end — a fixed uv radius would make the head glow twenty times bigger by the
    // time a fighter is about to act, which reads as it swelling as it charges.
    let px = 1.0 / max(in.size.x, 1.0);
    let tail = smoothstep(fill, 0.0, back);
    let nose = 1.0 - smoothstep(0.0, 22.0 * px, -back);
    let along = tail * nose;
    // It narrows as it goes back — a tongue, not a stripe — and reaches zero well inside the
    // quad, or the node's own top and bottom edges slice the flame off in straight lines.
    let width = mix(0.16, 0.6, along);
    let across = 1.0 - smoothstep(0.0, width, off);

    // Fire is the noise field CUT at a threshold, so its edge breaks into flickering tongues
    // instead of fading out as a gradient. Bias and gain are set so the field still SHOWS:
    // swamp it and this is an airbrush again.
    var heat = (n * 1.8 - 0.42) * along * across;
    heat = clamp(heat * 1.9, 0.0, 1.0);

    // The core: the aura on the sprite itself, drawn by the same field so the body and its
    // trail are one thing rather than two effects sharing a position.
    let core = (1.0 - smoothstep(0.0, 30.0 * px, abs(back))) * (1.0 - smoothstep(0.0, 0.5, off));
    heat = max(heat, core * 0.8);

    if (heat <= 0.004) {
        discard;
    }

    // WHITE AT THE CORE, the line's colour through the body, dark at the dying edge — the
    // same ramp a real flame has, expressed in whatever colour this fighter's line is.
    let col = fire.tint.rgb;
    var rgb = mix(col * 0.5, col, smoothstep(0.0, 0.5, heat));
    rgb = mix(rgb, vec3<f32>(1.0, 1.0, 1.0), smoothstep(0.66, 1.0, heat));
    // Over-bright at the very core so it blooms where the renderer has bloom to give, and
    // simply clamps to white where it does not.
    rgb = rgb * (1.0 + 1.3 * smoothstep(0.82, 1.0, heat));

    // ADDITIVE (see `specialize`): alpha is the weight this light is added with, so the
    // sprite underneath is never occluded — the trap `sprite_material` records three times.
    let a = heat * fire.tint.a;
    return vec4<f32>(rgb * a, a);
}
