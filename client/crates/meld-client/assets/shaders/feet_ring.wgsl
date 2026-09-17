// THE RING AT A COMBATANT'S FEET: its health, painted on the ground it is standing on.
//
// A bar kept anywhere other than the body it belongs to makes "how is the Knight doing" a
// question you answer by looking away from the Knight. A ring around the feet puts the answer
// where the body already is, and because it lies on the GROUND it reads in perspective with
// the scene instead of floating in front of it.
//
// ⚠️ **IT IS A DISC MESH, NOT A TORUS.** The annulus, the pool and the drained bed are all cut
// in the fragment shader, which is what lets the level be an arbitrary fraction that changes
// every time something takes damage — a torus would need its geometry rebuilt for every HP
// change, on every body, every tick.
//
// ⚠️ **GREEN IS LIFE AND RED IS WHAT IT COST, AND NEITHER IS A SIDE.** The ring is read as
// LIQUID: a green pool, the red bed it drains back over, and a dark vessel under both. Which
// side a body is on rides the outer rim instead — one hairline, because "whose is this" is
// answered by the body standing inside the ring long before the ring says anything.
//
// ⚠️ **THE POOL IS CENTRED ON THE FRONT AND EMPTIES TOWARD THE BACK.** A clock-style bar
// starting at the front would put the empty half exactly where the number is written and
// where the player is looking; draining symmetrically keeps the readable arc — the one
// nearest the camera and never behind the body — full until the fighter is nearly gone.

#import bevy_pbr::forward_io::VertexOutput

struct RingParams {
    // x = filled fraction 0..1, y = the fraction the pool is draining FROM (the red bed a hit
    // leaves behind), z = seconds, w = flags: 1 = this body owns the turn.
    params: vec4<f32>,
    // The SIDE this body is on, worn as the outer rim only, with an overall opacity in `a`.
    tint: vec4<f32>,
    // x = a heal just landed (1 → 0), y = a hit just landed (1 → 0), z = the Barrier this
    // body is holding as a fraction of its own max HP, w = the level's FLOW: positive while
    // it is draining, negative while it is filling, and zero the moment it settles.
    pulse: vec4<f32>,
};

@group(#{MATERIAL_BIND_GROUP}) @binding(100) var<uniform> ring: RingParams;

const TAU: f32 = 6.2831853;
// Where the ring's stroke sits, as a fraction of the mesh's radius. The gap between them is
// the stroke's thickness, and it is wide because the HP number is written INSIDE it.
// ⚠️ `battle_rings::R_IN`/`R_OUT` mirror these, because the Rust side has to know where the
// middle of the stroke is to lay the digits along it. A test reads them back out of this file.
const R_IN: f32 = 0.60;
const R_OUT: f32 = 0.96;

// Life, what life cost, and the vessel both sit in.
const LIFE: vec3<f32> = vec3<f32>(0.16, 0.86, 0.36);
const LOSS: vec3<f32> = vec3<f32>(0.93, 0.20, 0.22);
const EMPTY: vec3<f32> = vec3<f32>(0.05, 0.06, 0.09);
// Temp HP is not health: it is a shell standing in front of it, so it gets the steel blue the
// rest of the game already spends on Barrier rather than a shade of the liquid.
const WARD: vec3<f32> = vec3<f32>(0.55, 0.78, 1.0);
// How wide a gradient between two sections is, in fractions of the half-ring. Wide enough to
// read as one liquid shading into another rather than as two painted arcs meeting.
//
// ⚠️ **IT IS WIDEST ON SCREEN EXACTLY WHERE A HEALTHY RING PUTS IT.** The ring is an ellipse,
// so a fixed angular band covers far more pixels at the left and right extremes than at the
// front or back — and those extremes are where the waterline sits at around four-fifths of a
// bar. A blend that reads right in the middle of the ring reads as a pale WEDGE there.
const BLEND: f32 = 0.024;

// ⚠️ **THE POOL IS CONTINUOUS. DO NOT TICK IT INTO SEGMENTS.** The reference art draws its
// bars as blocks and a segmented version was built and thrown away: at this radius, on ground
// seen in perspective, twenty-six gaps read as a row of dashes rather than as a quantity —
// "ugly", and the liquid it spends is the whole reading. A block bar earns its gaps on a flat
// HUD bar with room for them; this one is a curve around a body.

@fragment
fn fragment(in: VertexOutput) -> @location(0) vec4<f32> {
    let fill = clamp(ring.params.x, 0.0, 1.0);
    // The bed can never be shallower than the pool standing in it.
    let ghost = clamp(max(ring.params.y, ring.params.x), 0.0, 1.0);
    let t = ring.params.z;
    // ⚠️ Not `active`: that is a reserved word in WGSL, and naming a local after it fails the
    // shader at PIPELINE BUILD — which `make check` never reaches.
    let is_turn = ring.params.w;
    let heal = clamp(ring.pulse.x, 0.0, 1.0);
    let hit = clamp(ring.pulse.y, 0.0, 1.0);
    let ward = clamp(ring.pulse.z, 0.0, 1.0);

    // The mesh is a unit `Circle`, so its uv is the bounding square: recentre to get a local
    // position, and work in polar from there.
    let p = (in.uv - vec2<f32>(0.5, 0.5)) * 2.0;
    let r = length(p);
    if (r < R_IN || r > R_OUT) {
        discard;
    }

    // Distance from the FRONT of the ring — the arc nearest the camera — as a fraction of the
    // half-ring, so both ends of the pool are the same number and the level is symmetric.
    // ⚠️ **MEASURED FROM +y, WHICH THE -90° TURN ABOUT X LAYS TOWARD THE CAMERA.** Negating it
    // centres the pool BEHIND the body and drains it across the one arc the number is written
    // on — verified by rendering it, because which way a flattened mesh faces is not something
    // to reason out from the transform.
    let d = abs(atan2(p.x, p.y)) / (TAU * 0.5);
    // Across the stroke: 0 at the inner wall, 1 at the outer.
    let u = (r - R_IN) / (R_OUT - R_IN);
    let a = fract((atan2(p.x, p.y) + TAU) / TAU);

    // ── the two surfaces ──────────────────────────────────────────────────────────────────
    // Each is a gradient, never a cut: a step between green and red reads as two stacked bars,
    // and the whole point of the ring is that it is ONE body of liquid at a level.
    // ⚠️ A full ring has its edge at d = 1, which is the seam at the back — `step` pins it
    // open, or a body at full health wears a dark notch behind it.
    // ⚠️ **WAVES RUN THE WAY THE LEVEL IS GOING, AND ONLY WHILE IT IS GOING.** An ambient
    // swell was built first and did not land: a sine travelling round a stroke twenty pixels
    // thick, on textured ground, seen in perspective, is not liquid — it is a bar that will
    // not hold still. What reads as liquid is the surface breaking up WHEN IT MOVES, so the
    // ripple's amplitude is the flow itself and a settled bar is perfectly flat.
    //
    // The phase runs in `d` — distance from the near arc, which is the axis the pool actually
    // drains along — so a crest travels toward the empty end while it is draining and back
    // toward full while it is filling, mirrored on both halves the way the pool itself is.
    let flow = clamp(ring.pulse.w, -1.0, 1.0);
    let surge = sign(flow) * min(abs(flow), 1.0);
    let lvl = fill + 0.017 * abs(surge) * sin(d * 34.0 - t * 9.0 * surge);

    var green = 1.0 - smoothstep(lvl - BLEND, lvl + BLEND, d);
    green = max(green, step(0.999, fill));
    let has_bed = smoothstep(0.0, 0.01, ghost - fill);
    var red = smoothstep(lvl - BLEND, lvl + BLEND, d)
        * (1.0 - smoothstep(ghost - BLEND, ghost + BLEND, d));
    red = max(red, step(0.999, ghost) * smoothstep(lvl - BLEND, lvl + BLEND, d)) * has_bed;

    // Light gathers along the inner wall. This does NOT move — depth is a property of the
    // vessel, and the only thing that animates is the surface, and only when it is going
    // somewhere.
    let across = 1.0 - smoothstep(0.12, 1.0, u);
    let shade = 0.88 + 0.12 * across;

    var col = EMPTY;
    var alpha = 0.72;
    // The drained bed keeps its own dark shading, so an empty ring is a vessel with something
    // missing from it rather than a hole in the picture.
    col = mix(col, LOSS * shade, red);
    alpha = mix(alpha, 0.98, red);
    col = mix(col, LIFE * shade, green);
    alpha = mix(alpha, 1.0, green);

    // THE WATERLINE: a bright meniscus where the pool ends. It is what makes a change of level
    // legible at a glance — the eye tracks the line, not the area.
    let edge = 1.0 - smoothstep(0.0, BLEND * 0.8, abs(d - lvl));
    let has_line = (1.0 - step(0.999, fill)) * step(0.001, fill);
    col = mix(col, mix(LIFE, vec3<f32>(1.0), 0.5), edge * has_line * 0.45);
    alpha = max(alpha, edge * has_line * 0.7);

    // A heal FLOWS IN over the red: the waterline runs white-green as it rises.
    col = mix(col, vec3<f32>(0.72, 1.0, 0.80), edge * heal * 0.8);
    alpha = max(alpha, edge * heal * 0.9);
    // A hit lights the bed it just opened, so the cost is visible for the beat it takes the
    // pool to settle — the same argument hitstop makes one screen over.
    // ⚠️ **KEEP THE RED.** A generous wash toward white over a bed that is already bright
    // leaves it reading as PINK — the flash is meant to say "this just happened", not to spend
    // the one colour that says what happened.
    col = mix(col, vec3<f32>(1.0, 0.72, 0.62), red * hit * 0.4);

    // THE SHELL IN FRONT OF THE HEALTH: a Barrier lining the inner wall, on its own arc from
    // the same front, so "how much does this buy me" is read against the pool it is standing
    // in front of rather than off a number somewhere else. Inside the stroke, because it is
    // literally what a blow meets first.
    if (ward > 0.001) {
        let lining = 1.0 - smoothstep(0.20, 0.30, u);
        let held = 1.0 - smoothstep(ward - BLEND, ward + BLEND, d);
        let w = lining * max(held, step(0.999, ward));
        // It BREATHES, so a shell that is holding reads as something doing work.
        let lit = 0.72 + 0.28 * sin(t * 2.2 + a * TAU);
        col = mix(col, WARD * lit, w * 0.92);
        alpha = max(alpha, w * 0.92);
    }

    // THE BLOW ITSELF, as a wave crossing the stroke from the inner wall outward — the ring's
    // own hitstop. It rides the SAME fading pulse the bed's flash does, so one hit is one
    // event drawn twice rather than two effects that can disagree about whether it happened.
    if (hit > 0.004) {
        let front = 1.0 - hit;
        // ⚠️ **NARROW AND THIN.** A wide bright wave washes the whole stroke, which spends
        // the pool's own colour — and the pool is the reading. It is a line crossing the
        // liquid, not a flash over it.
        let wave = 1.0 - smoothstep(0.0, 0.14, abs(u - front));
        col = mix(col, vec3<f32>(1.0, 0.92, 0.86), wave * hit * 0.40);
        alpha = max(alpha, wave * hit * 0.55);
    }

    // WHOSE BODY THIS IS, as a hairline on the outer wall — and a dark inner wall so the ring
    // reads as a drawn object on grass rather than as a smear of colour.
    let outer = smoothstep(0.84, 1.0, u);
    let inner = 1.0 - smoothstep(0.0, 0.13, u);
    col = mix(col, ring.tint.rgb, outer * 0.85);
    alpha = max(alpha, outer * 0.8);
    col = mix(col, vec3<f32>(0.02, 0.02, 0.04), inner * 0.7);
    alpha = max(alpha, inner * 0.75);

    // WHOSE TURN IT IS, on the ring itself: a light that runs round the stroke. The turn-order
    // bar says who is NEXT; this says who is being asked right now, on the body the player is
    // about to give an order to.
    //
    // ⚠️ **IT RIDES THE LIQUID'S COLOUR, IT DOES NOT REPLACE IT.** A wide cream spark painted
    // over the pool reads as a GAP in it — a sixth of the ring going pale looks exactly like a
    // section of bar that is neither full nor empty, on the one readout whose whole job is to
    // say which. Narrow, and brightening what is already there.
    if (is_turn > 0.5) {
        let chase = fract(a - t * 0.55);
        let spark = 1.0 - smoothstep(0.0, 0.07, chase);
        col = mix(col, mix(col, vec3<f32>(1.0, 0.97, 0.86), 0.8), spark * 0.6);
        alpha = max(alpha, spark * 0.7);
    }

    return vec4<f32>(col, alpha * ring.tint.a);
}
