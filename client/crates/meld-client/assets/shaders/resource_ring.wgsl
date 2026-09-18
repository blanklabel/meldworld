// WHAT THIS BODY HAS BANKED, AND WHAT IT CAN SPEND.
//
// A second, thinner hoop lying INSIDE the health ring at a fighter's feet. Health says what
// it costs to keep standing; this says what the class has stored up to do something about it
// — a Hunter's Adrenaline, a Psyker's Focus slots — and it is the one number a player of
// those classes watches climb between turns.
//
// ⚠️ **INSIDE, BECAUSE OUTSIDE IS TAKEN.** The health tube reaches 0.915 and the command
// wheel's inner wall stands at 0.941 with 0.026 between them; there is no room out there for
// anything, and the resource matters MOST on the hero being asked for an order, which is
// exactly when the wheel is open. The hole in the middle of the health ring is empty ground
// the body is already standing on.
//
// ⚠️ **IT IS NOT LIQUID, AND THAT IS THE POINT.** `feet_ring.wgsl` next door is a pool with a
// surface, a bed and waves — health is a level that drains. A resource is CHARGE: it has no
// surface, it flickers harder the more of it there is, and at capacity it runs a light round
// itself. Drawing them in the same language is what would make two concentric tori read as
// "two rings stacked on one body", which is the exact reading the health ring's own glass
// shell was deleted for.
//
// ⚠️ **AND IT IS THE FRONT ARC ONLY, BECAUSE THE REST OF THE CIRCLE IS BEHIND THE BODY.** The
// hoop is 0.98 across and a hero is standing in the middle of it, so — measured by RENDERING
// it — roughly the front HALF is ever on screen and a robed class hides the rest outright. Run
// a 0..1 level round the whole circle and half its range lands where nothing can be seen: a
// Psyker holding two of four slots drew exactly like one holding none, and a Hunter at 75
// exactly like one at 100, which is the difference between affording Frenzy and not. Both
// directions were tried and both failed the same way; the direction was never the problem.
//
// So the tube's rear is DISCARDED and the whole range is laid across the arc that is actually
// visible. It costs nothing — that geometry was never on screen — and it buys a readout whose
// every value can be told apart. The body standing inside completes the circle for the eye.
// ⚠️ The health ring keeps its full sweep and does NOT want this: its interesting end is
// already at the front, and a pool needs a vessel that goes all the way round to drain into.

#import bevy_pbr::forward_io::VertexOutput

struct ResourceParams {
    // x = filled fraction 0..1, y = how many WHOLE UNITS the resource is counted in
    // (0 = a smooth quantity), z = seconds, w = the bearing of the camera-facing arc in turns.
    params: vec4<f32>,
    // rgb = this resource's own colour, a = the hoop's opacity.
    tint: vec4<f32>,
    // x = something was just banked (1 → 0), y = something was just spent (1 → 0). z/w spare.
    //
    // ⚠️ **THE ORDER OF THESE FIELDS IS THE ABI.** `AsBindGroup` packs one `#[uniform(100)]`
    // struct in DECLARATION order, so a member added here in a different place than in the
    // Rust makes the shader read one field's bytes as another's — silently, with no error.
    pulse: vec4<f32>,
};

@group(#{MATERIAL_BIND_GROUP}) @binding(100) var<uniform> res: ResourceParams;

const TAU: f32 = 6.2831853;
// How wide the edge of the charge is, in fractions of the half-ring. Tighter than the health
// ring's: a resource has no meniscus to soften, and a crisp end is what makes "it went up by
// that much" legible on a hoop this thin.
const EDGE: f32 = 0.014;
// How far round from the camera-facing arc the hoop is drawn at all, as a fraction of the
// half-ring — so 0.5 is exactly the front half of the circle, ending at the two points where
// the projected ellipse is widest. ⚠️ Picked as the WORST case rather than the average: the
// Psyker's robe is the widest sprite in the party and its hem sits inside these two points, so
// an arc that stops here is fully visible for every class instead of for most of them.
const SWEEP: f32 = 0.5;
// Half-width of a unit notch, same units. Wide enough to survive the hoop being ~8 screen
// pixels, narrow enough that four of them do not eat the charge they are dividing.
const NOTCH: f32 = 0.018;

@fragment
fn fragment(in: VertexOutput) -> @location(0) vec4<f32> {
    let fill = clamp(res.params.x, 0.0, 1.0);
    let units = res.params.y;
    let t = res.params.z;
    let front = res.params.w;
    let gain = clamp(res.pulse.x, 0.0, 1.0);
    let spend = clamp(res.pulse.y, 0.0, 1.0);

    // How far round from the camera-facing arc, as a fraction of the half-ring — the same
    // measure `feet_ring.wgsl` takes, so the two hoops agree about where the front is.
    let rel = fract(in.uv.x - front + 1.0);
    let signed = rel - select(0.0, 1.0, rel > 0.5);
    let d = abs(signed) * 2.0;

    // Real light on a real surface: the torus carries its own normal, so the tube is shaded
    // rather than shaded-to-look-like.
    let n = normalize(in.world_normal);
    let key = normalize(vec3<f32>(-0.35, 0.85, 0.40));
    let lambert = clamp(dot(n, key), 0.0, 1.0);
    // `v` runs round the cross-section: the crown of the tube, which is the part facing the
    // player, and the grazing silhouette that says "cylinder" rather than "painted stripe".
    let crown = pow(clamp(sin(in.uv.y * TAU), 0.0, 1.0), 2.0);
    let sheen = pow(clamp(1.0 - abs(sin(in.uv.y * TAU)), 0.0, 1.0), 3.0);

    // Everything past the arc is behind the body. Cutting it here is what lets the level below
    // use the whole width of what can actually be seen.
    if (d > SWEEP) {
        discard;
    }
    // How far along the VISIBLE arc this pixel is, 0 at the near point and 1 at either end. The
    // level is measured in this, never in `d`, so full really does mean the arc is full.
    let span = d / SWEEP;
    let charged = 1.0 - smoothstep(fill - EDGE, fill + EDGE, span);

    // THE EMPTY CHANNEL. Dark and cold, and darker than the health ring's empty glass on
    // purpose: an unfilled resource must not compete with a health bar for attention, and the
    // channel's only job is to say how much room is left.
    let hollow = vec3<f32>(0.06, 0.07, 0.10) * (0.5 + 0.8 * lambert);

    // THE CHARGE. ⚠️ **IT FLICKERS HARDER THE MORE OF IT THERE IS.** A resource that sat
    // perfectly still would be a coloured arc; the whole read of banked fury is that it is
    // straining to be spent, and tying the movement to `fill` means a body with nothing banked
    // is completely calm — so the motion itself is the readout, before the length is.
    let hue = res.tint.rgb;
    let flick = 1.0 + 0.16 * fill * sin(t * 11.0 + in.uv.x * TAU * 3.0);
    var lit = hue * (0.85 + 0.55 * lambert) * flick;
    // A bright crown where the eye actually meets the tube.
    lit = mix(lit, mix(lit, vec3<f32>(1.0), 0.55), crown * 0.45);

    var col = mix(hollow, lit, charged);

    // THE UNITS, for a resource counted in whole ones. A Psyker's Focus slots are three or
    // four discrete places, not a smooth quantity, and a bar with no divisions on it cannot
    // say which. With the gap at the front the notches divide the EMPTY arc, so what you count
    // is how many slots are FREE — which is the number a Psyker is actually deciding on, since
    // the question at its turn is whether there is room to manifest another one. ⚠️ Adrenaline deliberately carries NONE: it banks 25 a swing against costs of
    // 30/35/40/80, so notches at any granularity would advertise a grid the costs do not sit
    // on — which is worse than no grid at all.
    if (units > 0.5) {
        let e = abs(fract(span * units + 0.5) - 0.5) / units;
        // ⚠️ Faded out near the near point: the divisions are symmetric about the
        // camera-facing arc, so the first boundary lands exactly on it — a notch there would
        // split the charge down the middle of the one part everybody reads.
        let notch = (1.0 - smoothstep(0.0, NOTCH, e)) * smoothstep(0.0, 0.10, span);
        // Cut hard rather than shaded: a notch is only ever read across the LIT part (count the
        // cells that are charged and you have counted the slots held), and a gentle darkening
        // on a hoop eight screen pixels wide is a notch nobody can see — which is what the
        // first cut of this was, measured in a capture.
        col = mix(col, col * 0.10, notch);
    }

    // THE WATERLINE — where the charge currently ends. Only while there is an end to see: at
    // empty and at full there is none, and drawing one anyway puts a bright mark on the front
    // arc of every body that has nothing banked.
    let line = 1.0 - smoothstep(0.0, EDGE * 1.4, abs(span - fill));
    let has_line = (1.0 - step(0.999, fill)) * step(0.001, fill);
    col = mix(col, mix(hue, vec3<f32>(1.0), 0.6), line * has_line * 0.55);
    // …and what just happened at it. A gain lifts the new end toward white; a spend burns the
    // ground it gave up, which is the half of the story an emptying bar cannot tell on its own.
    col = mix(col, vec3<f32>(1.0), line * gain * 0.7);
    col = mix(col, mix(hue, vec3<f32>(1.0, 0.85, 0.7), 0.5), (1.0 - charged) * spend * 0.45);

    // **AT CAPACITY IT RUNS A LIGHT ROUND ITSELF.** "Full" is the one state of this readout a
    // player acts on — it is the moment the expensive thing is affordable — so it gets a
    // motion of its own rather than merely being the length the bar stops at.
    if (fill > 0.999) {
        let chase = fract(in.uv.x - t * 0.24);
        let spark = 1.0 - smoothstep(0.0, 0.11, chase);
        col = mix(col, mix(col, vec3<f32>(1.0), 0.9), spark * 0.75);
    }

    // The tube's own silhouette, so the hoop reads as an object at any level — including at
    // zero, where there is otherwise nothing lit on it at all.
    col = mix(col, mix(col, vec3<f32>(0.70, 0.76, 0.88), 0.7), sheen * 0.35);

    return vec4<f32>(col, res.tint.a);
}
