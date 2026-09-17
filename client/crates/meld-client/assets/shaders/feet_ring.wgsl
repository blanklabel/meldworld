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
    // body is holding as a fraction of its own max HP, w = how hard the pool is SLOSHING.
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
    // THE POOL ROCKS WHEN THE WHEEL PUSHES THE CIRCLE OUT. It is liquid, and the one moment
    // the ring is physically shoved is the moment it should behave like liquid — the slosh is
    // what sells the command wheel growing OUT OF the bar rather than appearing over it.
    let slosh = clamp(ring.pulse.w, 0.0, 1.0);
    // DANGER is a state, not a second reading of the level: the pool boils and embers when
    // there is nearly none of it left. It never recolours the liquid wholesale — green still
    // means life at one hit point — it agitates it, which is what the eye catches in a fight
    // it is not looking directly at.
    let danger = 1.0 - smoothstep(0.12, 0.34, fill);

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
    // The surface swings, and it swings by POSITION round the ring, so one end piles up while
    // the other draws down — a level that merely pulsed up and down together would read as the
    // bar changing value rather than as the liquid moving.
    let wob = slosh * 0.055 * sin(d * 7.5 - t * 12.0);
    let fill_s = clamp(fill + wob, 0.0, 1.0);

    var green = 1.0 - smoothstep(fill_s - BLEND, fill_s + BLEND, d);
    green = max(green, step(0.999, fill));
    let has_bed = smoothstep(0.0, 0.01, ghost - fill);
    var red = smoothstep(fill_s - BLEND, fill_s + BLEND, d)
        * (1.0 - smoothstep(ghost - BLEND, ghost + BLEND, d));
    red = max(red, step(0.999, ghost) * smoothstep(fill_s - BLEND, fill_s + BLEND, d)) * has_bed;

    // LIQUID, NOT PAINT: light gathers against the inner wall, the surface deepens toward the
    // outer one, and a slow swell travels round the ring so the level looks held rather than
    // drawn. It is the same shading in both sections, which is what makes them one fluid.
    // The swell QUICKENS as the pool runs out — the same trick a channeling body uses to say
    // "soon" with its own pulse instead of a second widget saying it.
    let swell = 0.5 + 0.5 * sin(a * TAU * (2.0 + 2.0 * slosh) - t * (0.9 + 4.5 * danger + 7.0 * slosh));
    let across = 1.0 - smoothstep(0.12, 1.0, u);
    // ⚠️ **THE FLOOR IS HIGH ON PURPOSE.** This ring is alpha-blended onto bright ground, so a
    // shading term that dips far below 1 does not read as depth in the liquid, it reads as the
    // grass coming through and the whole ring going pale.
    let shade = 0.82 + 0.10 * across + 0.10 * swell * across;

    var col = EMPTY;
    var alpha = 0.72;
    // The drained bed keeps its own dark shading, so an empty ring is a vessel with something
    // missing from it rather than a hole in the picture.
    col = mix(col, LOSS * shade, red);
    alpha = mix(alpha, 0.98, red);
    let pool = mix(LIFE, vec3<f32>(1.0, 0.62, 0.18), danger * (0.35 + 0.25 * swell));
    col = mix(col, pool * shade, green);
    alpha = mix(alpha, 1.0, green);

    // THE WATERLINE: a bright meniscus where the pool ends. It is what makes a change of level
    // legible at a glance — the eye tracks the line, not the area.
    let edge = 1.0 - smoothstep(0.0, BLEND * 0.8, abs(d - fill_s));
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
