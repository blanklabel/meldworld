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

// ⚠️ WATER IS NOT A SUM OF SINES. This was four rotated `sin*cos` octaves, and it read as
// exactly that: smooth, regular, evenly rounded — a rippled bedsheet rather than a sea. Real
// water has SHARP crests and BROAD flat troughs, and the waves pile up against each other
// instead of sliding through one another.
//
// Two changes get both, and neither is expensive:
//
// 1. `exp(sin(x) - 1.0)` instead of `sin(x)`. Same period, but the exponential squashes the
//    trough toward zero and keeps the peak, which is the crest shape open water actually has.
//    It also lands in 0..1 rather than -1..1, so the sum is re-centred at the end.
//
// 2. POSITION DRAGGING. Each octave nudges the sample point along its own direction by its
//    own derivative before the next octave reads it (`DRAG`). That is what makes waves
//    interfere and heap instead of passing through each other, and it is the single thing
//    that separates procedural water that looks like water from procedural water that looks
//    like noise. Getting the derivative for free is why `wave_dx` returns both.
//
// The technique is standard (it is how most procedural oceans are built); the implementation
// is ours, which also keeps us clear of the non-commercial licences the reference shaders
// carry.

/// One directional wave: `x` is its height in 0..1, `y` its derivative along `dir`.
fn wave_dx(pos: vec2<f32>, dir: vec2<f32>, freq: f32, timeshift: f32) -> vec2<f32> {
    let x = dot(dir, pos) * freq + timeshift;
    let wave = exp(sin(x) - 1.0);
    return vec2<f32>(wave, -wave * cos(x));
}

/// Summed directional waves, dragged. Directions come off `sin`/`cos` of a marching angle so
/// no two octaves share a heading and the crests never line up into a visible grid.
fn wave_height(p_in: vec2<f32>, t: f32) -> f32 {
    // How hard each octave shoves the next one's sample point. Small: this is a nudge, and
    // past about 0.1 the field folds over itself into froth.
    let drag = 0.048;
    var pos = p_in;
    var iter = 0.0;
    var freq = 0.085;
    var speed = 1.35;
    var weight = 1.0;
    var sum = 0.0;
    var total = 0.0;
    for (var i = 0; i < 5; i = i + 1) {
        let dir = vec2<f32>(sin(iter), cos(iter));
        let res = wave_dx(pos, dir, freq, t * speed);
        pos = pos + dir * res.y * weight * drag;
        sum = sum + res.x * weight;
        total = total + weight;
        // Later octaves are finer, faster and count for less — the usual spectrum, except
        // the weight falls off by a MIX rather than a halving, so the small chop keeps
        // contributing instead of vanishing after two steps.
        weight = mix(weight, 0.0, 0.2);
        freq = freq * 1.18;
        speed = speed * 1.07;
        iter = iter + 12.0;
    }
    // Back to roughly -1..1, so callers' amplitude and steepness keep their meaning.
    return (sum / max(total, 0.0001)) * 2.0 - 1.0;
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

/// THE LONG SWELL — the half of a sea that has to be GEOMETRY.
///
/// ⚠️ EVERYTHING ABOVE THIS IS A NORMAL, AND A NORMAL CANNOT MAKE A SILHOUETTE. Our water
/// was a wave normal painted on a flat plane, and no amount of tuning gets a crest to break
/// the horizon, occlude what is behind it, or catch the light on one face and not the other.
/// Reference oceans (Seascape and its kin) raymarch a displaced heightfield, which is why
/// they have wave *shapes* and ours read as a flat sheet with texture on it — a difference
/// of geometry, not of colour, which is why three passes of colour tuning never closed it.
///
/// We do not need a raymarcher to get it: the open sea is part of the sliding GROUND MESH,
/// which is already vertex-displaced by `total_height`. So the swell is free — it is the
/// same displacement the hills use, and `terrain_normal` differentiates it into correct
/// lighting with no extra code.
///
/// ⚠️ WAVELENGTH IS BOUNDED BY THE VERTEX GRID (`GROUND_CELL`, ~5 world units). These three
/// components are ~49, ~32 and ~25 units long, so the coarsest has ten vertices to its
/// crest and the finest five. Anything shorter aliases into a shimmering mess as the plane
/// slides, and shorter is what the per-fragment chop above is FOR: the split is swell in
/// geometry, chop in the normal.
fn sea_swell(wxz: vec2<f32>, t: f32) -> f32 {
    let a = sin(dot(wxz, vec2<f32>(0.118, 0.052)) + t * 0.55);
    let b = sin(dot(wxz, vec2<f32>(-0.061, 0.186)) + t * 0.41);
    let c = sin(dot(wxz, vec2<f32>(0.203, 0.149)) + t * 0.83);
    // Crests sharper than troughs, the same asymmetry `wave_dx` gives the chop.
    let h = a * 0.62 + b * 0.42 + c * 0.20;
    return (h + abs(h) * 0.35) * 0.85;
}
