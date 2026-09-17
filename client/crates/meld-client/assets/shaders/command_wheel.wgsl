// THE ORDERS, AS A WHEEL AROUND THE HERO BEING ASKED FOR THEM.
//
// A ring of wedges lying on the ground at the acting hero's feet, one per verb, with the body
// standing in the middle of it. It is the same object its health ring is — paint on the ground
// the fighter occupies — so the question and the thing being asked are one shape rather than a
// panel somewhere else on the screen.
//
// ⚠️ **IT IS WEDGES, NOT CHIPS.** Floating pills on an arc are five separate objects that
// happen to be near a hero; a divided ring is ONE object that belongs to it, which is what
// makes the hero read as being inside its own menu. UI cannot draw a wedge — a `Node` is an
// alpha-blended rounded rectangle — so the wheel is a ground material and the labels are UI
// text projected onto each wedge, exactly as the health ring's digits are.
//
// ⚠️ **AND IT IS A DISC MESH.** The annulus, the wedges and the gaps between them are all cut
// in the fragment stage, so the count can change with the page without rebuilding geometry.

#import bevy_pbr::forward_io::VertexOutput

struct WheelParams {
    // x = how many wedges, y = which one the cursor is on (its index, -1 for none),
    // z = seconds, w = the bearing of wedge 0 in turns, so the wheel can be laid out from
    // whichever direction the camera is looking along.
    params: vec4<f32>,
    // rgb = the acting side's colour, a = the wheel's overall opacity.
    tint: vec4<f32>,
};

@group(#{MATERIAL_BIND_GROUP}) @binding(100) var<uniform> wheel: WheelParams;

const TAU: f32 = 6.2831853;
// The band the wedges occupy, as a fraction of the mesh radius. Thick, because a wedge has to
// hold an icon over a word — a thin one is a pie chart with writing on it.
// ⚠️ `battle_radial::W_IN`/`W_OUT` mirror these; a test reads them back out of this file.
const W_IN: f32 = 0.52;
const W_OUT: f32 = 0.97;
// Half the gap between two wedges, in turns. It is what makes them read as separate choices
// rather than as one ring with text on it.
const GAP: f32 = 0.006;

@fragment
fn fragment(in: VertexOutput) -> @location(0) vec4<f32> {
    let count = max(wheel.params.x, 1.0);
    let cursor = wheel.params.y;
    let t = wheel.params.z;
    let bearing = wheel.params.w;

    let p = (in.uv - vec2<f32>(0.5, 0.5)) * 2.0;
    let r = length(p);
    if (r < W_IN || r > W_OUT) {
        discard;
    }
    // Measured from the FRONT — the arc nearest the camera — so wedge 0 is the one the eye
    // lands on and the layout never depends on where the body happens to be standing.
    let a = fract(atan2(p.x, p.y) / TAU + 1.0 - bearing);

    // Which wedge this pixel is in, and how far across it.
    let slot = floor(a * count);
    let across = fract(a * count);
    // The gap, cut from both ends of every wedge.
    let edge = min(across, 1.0 - across);
    if (edge < GAP * count) {
        discard;
    }
    let cut = smoothstep(GAP * count, GAP * count + 0.02, edge);

    // Across the band: the wedge is darkest at its walls and open in the middle, so it reads
    // as a pressed plate rather than as a flat sector of a pie.
    let u = (r - W_IN) / (W_OUT - W_IN);
    let dish = 1.0 - smoothstep(0.35, 1.0, abs(u - 0.5) * 2.0);

    let chosen = abs(slot - cursor) < 0.5;
    // The one under the cursor lifts toward the acting side's own colour and breathes, so
    // "this is what Enter presses" needs no second marker.
    let pulse = 0.78 + 0.22 * sin(t * 3.4);
    var col = vec3<f32>(0.05, 0.06, 0.10);
    var alpha = 0.72 * cut * (0.55 + 0.45 * dish);
    if (chosen) {
        col = mix(col, wheel.tint.rgb, (0.55 + 0.35 * dish) * pulse);
        alpha = max(alpha, 0.88 * cut);
    }

    // A rim on both walls of the band, brighter on the chosen wedge — the same trick the glass
    // panels use to say "this is an object" rather than "this is a stain".
    let rim = 1.0 - smoothstep(0.0, 0.16, min(u, 1.0 - u));
    let rim_col = select(vec3<f32>(0.42, 0.48, 0.62), wheel.tint.rgb, chosen);
    col = mix(col, rim_col, rim * 0.8 * cut);
    alpha = max(alpha, rim * cut * select(0.55, 0.9, chosen));

    return vec4<f32>(col, alpha * wheel.tint.a);
}
