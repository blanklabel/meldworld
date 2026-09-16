// WHAT IS BEING DONE TO A FIGHTER'S PLACE IN THE TURN ORDER.
//
// Two states share one shader, picked by `kind` — the same argument `ability_fx.wgsl` makes
// for putting sixteen damage types in one file: a second shader is a second thing to keep in
// step, and the day a third state is added the two that exist should already say how.
//
//   WALL  — this fighter is PINNED, so a little translucent wall stands across its lane and
//           its icon is pressed up against it. A barrier reads as *impeded* in a way a ring
//           around the body does not: the body is trying to go somewhere and cannot.
//   PRESS — this fighter is STAGGERED, so neon is raining down on it, pushing it into the
//           floor of its own lane. Light in the middle of each line and dark at its edges,
//           more solid than rain.
//   PUNCH — this fighter was RECOILED: a grey ball comes in from the right, stops dead on
//           the body for a beat, and carries on through and out the other side. The beat is
//           the point — hitstop is what a blow landing FEELS like, and its timing lives in
//           Rust (`punch_travel`) where it can be tested rather than eyeballed.
//
// ⚠️ **BOTH ARE SHADERS FOR THE REASON THE FLAME IS.** A Bevy UI node is an alpha-blended
// rounded rectangle: it cannot ramp a colour or carry a pattern, and four `Node`-built
// attempts at the flame failed on exactly that before a `UiMaterial` fixed it in one go.
// The ring these replaced was the best a rectangle can do, and it looked like a rectangle.

#import bevy_ui::ui_vertex_output::UiVertexOutput

struct StateParams {
    // y is always the kind. The rest is read per kind, because a punch and a standing wall
    // do not want the same four numbers:
    //   WALL / PRESS — x = seconds, z = per-fighter seed, w = unused.
    //   PUNCH        — x = the icon's height as a fraction of this quad's, z = the ball's
    //                   position in uv.x, w = impact flash 0..1. Its opacity rides `tint.a`.
    params: vec4<f32>,
    // The state's colour, with an overall opacity in `a`.
    tint: vec4<f32>,
    // PUNCH only: x = the ball's position in uv.y, y = how far its tail stretches,
    // zw = the direction it is travelling (unit, in uv).
    extra: vec4<f32>,
};

@group(1) @binding(0) var<uniform> st: StateParams;

const KIND_WALL: f32 = 0.0;
const KIND_PRESS: f32 = 1.0;
const KIND_PUNCH: f32 = 2.0;

fn hash1(n: f32) -> f32 {
    return fract(sin(n * 127.1) * 43758.5453);
}

// THE LITTLE WALL: courses of translucent blocks standing across the lane, lit along the
// edge the icon is pressed against.
fn wall(uv: vec2<f32>, t: f32, seed: f32) -> vec4<f32> {
    // Stacked courses, each offset half a block from the one below — the pattern that says
    // "wall" rather than "pane of glass" at any size.
    let courses = 5.0;
    let row = floor(uv.y * courses);
    let shift = select(0.0, 0.5, fract(row * 0.5) > 0.25);
    let bx = fract(uv.x * 1.6 + shift);
    let by = fract(uv.y * courses);

    // Mortar: a dark gap between blocks, and the block faces between them.
    let joint = min(smoothstep(0.0, 0.10, bx), smoothstep(0.0, 0.10, by))
        * min(smoothstep(0.0, 0.10, 1.0 - bx), smoothstep(0.0, 0.10, 1.0 - by));
    // Each block sits at its own brightness, so the wall has some depth to it.
    let face = 0.62 + 0.38 * hash1(row * 3.7 + floor(uv.x * 1.6 + shift) * 11.3 + seed);

    // THE IMPACT EDGE. The icon is pressed against the left face, so that edge is lit and
    // pulses — the wall is holding something back, and it should look like it is taking the
    // weight.
    let press = 1.0 - smoothstep(0.0, 0.42, uv.x);
    let pulse = 0.72 + 0.28 * sin(t * 9.0 + seed * 6.0);

    // Fade out at the top and bottom so the wall reads as a standing slab rather than as a
    // rectangle clipped by its own node.
    let ends = smoothstep(0.0, 0.14, uv.y) * smoothstep(0.0, 0.14, 1.0 - uv.y);

    let body = joint * face * ends;
    var a = body * 0.5 + press * ends * 0.55 * pulse;
    // A hard bright line right where contact happens.
    a = a + (1.0 - smoothstep(0.0, 0.10, uv.x)) * ends * 0.5 * pulse;

    // ⚠️ **KEEP THE COLOUR.** Additive over a bright background already pulls everything
    // toward white, so a generous white mix on top of that left the wall reading as plain
    // grey stone — the tint is what says which state this is, and it has to survive.
    var rgb = st.tint.rgb * (0.5 + 0.5 * face);
    rgb = mix(rgb, vec3<f32>(1.0), press * 0.38 * pulse);
    return vec4<f32>(rgb, clamp(a, 0.0, 1.0));
}

// THE PRESS: neon coming down on a staggered body, light-cored and dark-edged, heavier the
// further down it gets — it is pushing the fighter into the floor of its lane.
fn press(uv: vec2<f32>, t: f32, seed: f32) -> vec4<f32> {
    let lines = 7.0;
    let col = floor(uv.x * lines);
    let across = fract(uv.x * lines);
    // LIGHT IN THE MIDDLE, DARK AT THE EDGES — across each line, not along it.
    let core = 1.0 - smoothstep(0.18, 0.5, abs(across - 0.5));

    // Each line falls at its own speed and phase, or the whole thing strobes as one bar.
    let speed = 1.1 + 0.9 * hash1(col + seed);
    let phase = hash1(col * 5.1 + seed * 2.3);
    let p = fract(uv.y * 0.85 - t * speed + phase);
    // A solid segment rather than a thin streak: this is heavier than rain.
    let seg = smoothstep(0.0, 0.10, p) * (1.0 - smoothstep(0.42, 0.72, p));

    // It bears DOWN: nothing at the top of the lane, weight at the bottom.
    let weight = 0.35 + 0.65 * uv.y;
    // …and stops short of the very edges so the quad's own bounds never show.
    let ends = smoothstep(0.0, 0.08, uv.x) * smoothstep(0.0, 0.08, 1.0 - uv.x);

    let a = seg * core * weight * ends;
    // DARK purple at the edge of each line, LIGHT purple at its core — light, not white:
    // additive compositing washes the core out on its own, and a line that reaches white has
    // stopped being purple at exactly the point the eye looks at.
    let rgb = mix(st.tint.rgb * 0.22, mix(st.tint.rgb, vec3<f32>(1.0), 0.18), core);
    return vec4<f32>(rgb, clamp(a, 0.0, 1.0));
}

// THE PUNCH: a grey ball arcing up from the bottom right, peaking ON the body — where it
// lands and everything stops for a beat — then falling away to the bottom left off the lane.
//
// ⚠️ **EVERYTHING HERE IS MEASURED IN QUAD HEIGHTS, NOT IN UV.** The quad is several times
// wider than it is tall, so a radius in raw uv draws an ellipse the width of the lane: the
// first cut put out a forty-pixel white blob with an eighty-pixel streak that swallowed the
// icon and half the rail, which reads as an explosion rather than as a hit. `size` comes free
// on `UiVertexOutput`, so the aspect never has to be passed in.
//
// ⚠️ **The travel direction is handed IN**, because the ball follows an arc: a tail that
// assumed horizontal motion would hang off the side of the curve rather than behind the ball.
fn punch(uv: vec2<f32>, size: vec2<f32>, icon_h: f32, stretch: f32, ball: vec2<f32>, dir: vec2<f32>, flash: f32) -> vec4<f32> {
    let aspect = max(size.x / max(size.y, 1.0), 1.0);
    // Into height units, so a circle is a circle.
    let d = vec2<f32>((uv.x - ball.x) * aspect, uv.y - ball.y);
    let r = length(d);

    // ⚠️ **SIZED AGAINST THE ICON, NOT THE QUAD.** The quad's height changes with the arc's
    // drop, so a radius in quad-heights silently doubled the ball the moment the arc was
    // made taller — a forty-pixel blown-out blob over a thirty-pixel body. `icon_h` is the
    // icon's height as a fraction of this quad's, so the ball stays the same size on screen
    // however the arc is retuned.
    let core = 1.0 - smoothstep(0.08 * icon_h, 0.30 * icon_h, r);

    // The streak it left BEHIND, measured back along the direction of travel.
    //
    // ⚠️ `dir` is ALREADY in height units — it is a unit vector built from a pixel delta, and
    // dividing both components by the same height leaves it unchanged. Scaling its x by the
    // aspect a second time skewed the tail toward horizontal, so on an arc it hung off the
    // side of the ball instead of behind it.
    let along = -dot(d, dir);
    let side = length(d + along * dir);
    // ⚠️ **AND IT ONLY EXISTS BEHIND THE BALL.** Clamping the distance with `max(along, 0)`
    // gives every pixel IN FRONT of the ball a distance of zero — which is full tail
    // brightness — so the streak drew ahead of the ball instead of behind it, pointing the
    // punch backwards on every frame.
    let behind = select(0.0, 1.0, along > 0.0);
    let tail = behind
        * (1.0 - smoothstep(0.0, 1.3 * icon_h * stretch, max(along, 0.0)))
        * (1.0 - smoothstep(0.04 * icon_h, 0.22 * icon_h, side));
    // The impact: a ring thrown out from the body at the moment everything stops.
    let ring = (1.0 - smoothstep(0.15 * icon_h, 0.55 * icon_h, r)) * flash;

    let a = clamp(core * 0.9 + tail * 0.3 + ring * 0.55, 0.0, 1.0);
    // GREY: this is something done TO the fighter, and every side colour on this bar is
    // already spoken for. Only the very core of the ball and the impact reach white.
    var rgb = vec3<f32>(0.60, 0.63, 0.70);
    rgb = mix(rgb, vec3<f32>(1.0), clamp(core * 0.85 + ring * 0.7, 0.0, 1.0));
    return vec4<f32>(rgb, a);
}

@fragment
fn fragment(in: UiVertexOutput) -> @location(0) vec4<f32> {
    let t = st.params.x;
    let kind = st.params.y;
    let seed = st.params.z;

    var c: vec4<f32>;
    if (kind > (KIND_PRESS + KIND_PUNCH) * 0.5) {
        c = punch(
            in.uv,
            in.size,
            t,
            st.extra.y,
            vec2<f32>(st.params.z, st.extra.x),
            vec2<f32>(st.extra.z, st.extra.w),
            st.params.w,
        );
    } else if (kind > (KIND_WALL + KIND_PRESS) * 0.5) {
        c = press(in.uv, t, seed);
    } else {
        c = wall(in.uv, t, seed);
    }

    let a = c.a * st.tint.a;
    if (a <= 0.004) {
        discard;
    }
    // ADDITIVE, like every other effect on this bar: the icon underneath is never occluded.
    return vec4<f32>(c.rgb * a, a);
}
