// The SKY: a gradient dome with a sun in it.
//
// ⚠️ THE SKY WAS ONE FLAT COLOUR — `ClearColor`, remixed per frame for time of day and
// weather. That is fine behind a diorama and wrong for anything that has to REFLECT it.
// Water is the obvious casualty: fresnel mixes in "the sky", and a single value means the
// sea reflects the same grey whichever way it faces, which is a large part of why open
// water read as wet concrete no matter how the wave shading was tuned.
//
// It is also just what a sky looks like. A real one is darkest overhead and pale at the
// horizon, with a bright band around the sun. None of that needs a cubemap or an
// atmosphere integral — a dome and a dot product get most of the way there, at pixel-art
// resolution where nobody is counting photons.
//
// Unlit and depth-write-off: this is a backdrop, not geometry. It is anchored to the camera
// (`anchor_sky_dome`) so it never moves relative to the viewer, which is what makes a dome
// read as infinitely far away rather than as a ball you could walk to.

#import bevy_pbr::forward_io::VertexOutput

struct SkyDome {
    /// Colour at the horizon.
    horizon: vec4<f32>,
    /// Colour straight overhead.
    zenith: vec4<f32>,
    /// `xyz` = direction TO the sun (normalised); `w` = daylight factor 0..1.
    sun_dir: vec4<f32>,
    /// `rgb` = the sun's own colour; `a` = how strongly its glow bleeds into the sky.
    sun_col: vec4<f32>,
}

@group(#{MATERIAL_BIND_GROUP}) @binding(100) var<uniform> sky: SkyDome;

@fragment
fn fragment(in: VertexOutput) -> @location(0) vec4<f32> {
    // The dome is a sphere centred on the camera, so its outward NORMAL at this fragment is
    // exactly the direction the viewer is looking. That saves reaching for the view uniform
    // (and hand-declaring a binding Bevy already owns) for a value the geometry hands over.
    let dir = normalize(in.world_normal);
    // Height in the sky, 0 at the horizon .. 1 overhead. `pow` biases the gradient toward
    // the horizon, where the interesting half of a sky lives (and where a low diorama
    // camera is actually looking).
    let up = clamp(dir.y, 0.0, 1.0);
    var col = mix(sky.horizon.rgb, sky.zenith.rgb, pow(up, 0.55));

    // The sun's glow: broad and soft, so it reads as light in the air rather than a decal.
    // Scaled by daylight so it does not hang in a midnight sky.
    let toward = clamp(dot(dir, sky.sun_dir.xyz), 0.0, 1.0);
    let day = sky.sun_dir.w;
    let halo = pow(toward, 7.0) * 0.55 + pow(toward, 90.0) * 0.9;
    col = col + sky.sun_col.rgb * halo * sky.sun_col.a * day;

    // The disc itself — small, and only by day.
    let disc = smoothstep(0.9975, 0.9990, toward);
    col = mix(col, sky.sun_col.rgb * 1.6, disc * day);

    return vec4<f32>(col, 1.0);
}
