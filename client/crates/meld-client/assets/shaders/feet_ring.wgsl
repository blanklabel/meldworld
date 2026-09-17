// THE LIQUID INSIDE A COMBATANT'S HEALTH RING.
//
// The ring is a glass TUBE lying on the ground at a fighter's feet, and this is what is in it:
// a green pool filling it round from the arc nearest the camera, the red bed a hit opens
// behind the pool, and nothing at all past that — where the tube is empty you see empty glass,
// because the shell (a plain `StandardMaterial` with specular transmission) is always there.
//
// ⚠️ **IT IS A TORUS, NOT AN ANNULUS CUT FROM A DISC.** The flat version was shaded to look
// round with three hand-written terms — a specular, a far wall, a fresnel — and still came back
// from play as not reading as a tube. It never could: the light was a guess about a surface
// that was not there. A torus carries real normals and real depth, so the scene's own light
// does the work, and its UVs are already the two coordinates this bar needs.
//
// ⚠️ **AND THE LEVEL IS MEASURED FROM THE CAMERA, NOT FROM THE MESH.** `u` starts wherever the
// torus generator began winding; the pool has to start at the arc NEAREST THE VIEWER, so the
// Rust side hands in that bearing (`front`). Reading the mesh's own seam instead is what put
// the pool behind the body for a whole build.

#import bevy_pbr::forward_io::VertexOutput

struct RingParams {
    // x = filled fraction 0..1, y = the fraction the red bed reaches, z = seconds,
    // w = 1 while this body owns the turn.
    params: vec4<f32>,
    // The SIDE this body is on, worn as a tint on the liquid's rim, opacity in `a`.
    tint: vec4<f32>,
    // x = a heal just landed (1 → 0), y = a hit just landed (1 → 0), z = the Barrier held as a
    // fraction of max HP, w = the level's FLOW (positive draining, negative filling).
    pulse: vec4<f32>,
    // x = the bearing of the camera-facing arc, in turns round `u`. y/z/w spare.
    view: vec4<f32>,
    // The surface: 48 heights round the ring, four to a vec4, simulated in Rust as a damped
    // wave equation (`battle_rings::RingWave`).
    //
    // ⚠️ std140 gives a bare `f32` array a 16-byte stride, so the packing is four-to-a-`vec4`
    // on both sides — and ⚠️ the FIELD ORDER of this struct is the ABI: `AsBindGroup` packs one
    // `#[uniform(100)]` block in declaration order, so a member added in a different place on
    // one side makes the shader read another's bytes, silently and with no error.
    wave: array<vec4<f32>, 12>,
};

@group(#{MATERIAL_BIND_GROUP}) @binding(100) var<uniform> ring: RingParams;

const TAU: f32 = 6.2831853;
// Life, and what life cost. Green is ALWAYS life — the level is read from how much there is,
// never from what colour it is.
const LIFE: vec3<f32> = vec3<f32>(0.16, 0.86, 0.36);
const LOSS: vec3<f32> = vec3<f32>(0.93, 0.20, 0.22);
// How wide the gradient between the two is, in fractions of the half-ring.
const BLEND: f32 = 0.024;

fn wave_at(i: i32) -> f32 {
    let q = ring.wave[i / 4];
    let k = i % 4;
    if (k == 0) { return q.x; }
    if (k == 1) { return q.y; }
    if (k == 2) { return q.z; }
    return q.w;
}

@fragment
fn fragment(in: VertexOutput) -> @location(0) vec4<f32> {
    let fill = clamp(ring.params.x, 0.0, 1.0);
    let ghost = clamp(max(ring.params.y, ring.params.x), 0.0, 1.0);
    let t = ring.params.z;
    // ⚠️ Not `active`: a reserved word in WGSL, and naming a local after it fails the shader at
    // PIPELINE BUILD — which `make check` never reaches.
    let is_turn = ring.params.w;
    let heal = clamp(ring.pulse.x, 0.0, 1.0);
    let hit = clamp(ring.pulse.y, 0.0, 1.0);

    // How far round from the camera-facing arc, as a fraction of the half-ring. The pool is
    // symmetric about that arc, so both ends of it are the same number — which keeps the half
    // the player is looking at full until the fighter is nearly gone.
    let rel = fract(in.uv.x - ring.view.x + 1.0);
    let signed = rel - select(0.0, 1.0, rel > 0.5);
    let d = abs(signed) * 2.0;

    // Past the bed there is no liquid at all: what shows is the empty glass of the shell.
    if (d > ghost + BLEND) {
        discard;
    }

    let green = 1.0 - smoothstep(fill - BLEND, fill + BLEND, d);
    let red = clamp(1.0 - green, 0.0, 1.0) * (1.0 - smoothstep(ghost - BLEND, ghost + BLEND, d));

    // The simulated surface, sampled round the ring and interpolated so it reads as one sheet
    // rather than as 48 stripes. HIGH PARTS ARE LIGHTER.
    let wpos = fract(in.uv.x) * 48.0;
    let w0 = i32(floor(wpos)) % 48;
    let wf = fract(wpos);
    let h = mix(wave_at(w0), wave_at((w0 + 1) % 48), wf);
    let crest = clamp(h * 9.0, -1.0, 1.0);

    // ── real light on a real surface ──────────────────────────────────────────────────────
    // The torus gives us its own normal, so this is shading rather than an impression of it.
    let n = normalize(in.world_normal);
    let key = normalize(vec3<f32>(-0.35, 0.85, 0.40));
    let lambert = clamp(dot(n, key), 0.0, 1.0);
    // The tube's upper half catches the light; `v` runs round the cross-section, so this is
    // the meniscus — liquid climbing the glass — sitting where it physically would.
    let climb = pow(clamp(sin(in.uv.y * TAU), 0.0, 1.0), 2.0);

    // ⚠️ **THE LIQUID CARRIES ITS OWN LIGHT.** It is seen THROUGH a shell, and anything that
    // only reflects the scene comes out the far side as grey — the bar went pale the moment the
    // glass became visible enough to read. A pool that glows survives its own container, which
    // is also what a health bar should do: be the brightest thing on the body.
    var col = mix(LOSS, LIFE, green);
    col = col * (1.05 + 0.45 * lambert + 0.20 * crest);
    // A bright meniscus where the liquid meets the glass, lifted by the wave passing under it.
    col = mix(col, mix(col, vec3<f32>(1.0), 0.55), climb * (0.30 + 0.25 * crest));

    // THE WATERLINE, and the two things that happen at it. ⚠️ Neither ever MOVES it: the
    // boundary is the health number, and a boundary that wobbles reads as the number changing
    // — which came back from play as people thinking they were being healed.
    let edge = 1.0 - smoothstep(0.0, BLEND * 0.9, abs(d - fill));
    let has_line = (1.0 - step(0.999, fill)) * step(0.001, fill);
    col = mix(col, mix(LIFE, vec3<f32>(1.0), 0.5), edge * has_line * 0.45);
    col = mix(col, vec3<f32>(0.72, 1.0, 0.80), edge * heal * 0.8);
    col = mix(col, vec3<f32>(1.0, 0.72, 0.62), red * hit * 0.4);

    // WHOSE BODY THIS IS, on the liquid's own rim — one hairline, because the body standing
    // inside the ring has already answered that.
    let rim = pow(clamp(1.0 - abs(sin(in.uv.y * TAU)), 0.0, 1.0), 3.0);
    col = mix(col, ring.tint.rgb, rim * 0.30);

    // WHOSE TURN IT IS: a light running round the tube. The turn-order bar says who is NEXT;
    // this says who is being asked right now, on the body about to be given an order.
    if (is_turn > 0.5) {
        let chase = fract(in.uv.x - t * 0.18);
        let spark = 1.0 - smoothstep(0.0, 0.07, chase);
        col = mix(col, mix(col, vec3<f32>(1.0, 0.97, 0.86), 0.85), spark * 0.7);
    }

    return vec4<f32>(col, ring.tint.a);
}
