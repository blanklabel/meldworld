#define_import_path meld::water_wave

// The wave field and its normal, shared by every water surface in the game.
//
// ⚠️ THIS IS A LIBRARY BECAUSE THERE ARE THREE WATERS AND THEY MUST NOT DRIFT. The open
// sea is painted by the ground shader (`ground_biome.wgsl`), the maze's ponds and bogs are
// prop meshes, and Last City's sea is three planes in a scene with its own coordinates.
// The first version of this shipped with the wave code living only in the ground shader,
// so the ocean moved and every other body of water in the game stayed a flat slate slab —
// which is exactly the "one rule, N call sites" failure this repo keeps re-learning, with
// the difference visible from the shore.
//
// Detail is per-fragment on purpose. A pond disc is 28 segments and the city's sea is two
// triangles; anything that put the ripples in the VERTICES would give the ocean waves and
// the pond none.

// Summed directional waves. Each octave is rotated off the last so the crests never line up
// into a visible grid, which is the tell that gives away cheap procedural water.
fn wave_height(p_in: vec2<f32>, t: f32) -> f32 {
    var q = p_in;
    var h = 0.0;
    var amp = 1.0;
    var freq = 0.075;
    for (var i = 0; i < 4; i = i + 1) {
        let a = sin(q.x * freq + t * 1.05) * cos(q.y * freq * 0.87 - t * 0.71);
        let b = sin((q.x + q.y) * freq * 1.31 - t * 0.93);
        h = h + (a + b * 0.6) * amp;
        amp = amp * 0.5;
        freq = freq * 1.93;
        // ~16 degrees per octave.
        q = vec2<f32>(q.x * 0.961 - q.y * 0.276, q.x * 0.276 + q.y * 0.961);
    }
    return h;
}

// The surface normal, by finite-differencing the same field. `steep` scales how much the
// slope tilts the normal — the field is unitless, so this is where it becomes a look.
fn water_normal(p: vec2<f32>, t: f32, steep: f32) -> vec3<f32> {
    let e = 0.75;
    let h = wave_height(p, t);
    let hx = wave_height(p + vec2<f32>(e, 0.0), t);
    let hz = wave_height(p + vec2<f32>(0.0, e), t);
    return normalize(vec3<f32>((h - hx) * steep, e, (h - hz) * steep));
}
