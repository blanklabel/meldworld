//! Continuous overworld terrain height — the SINGLE SOURCE OF TRUTH shared by the
//! server (movement collision, path routing, placement) and the client (entity/camera
//! Y), and mirrored exactly in `client/.../assets/shaders/ground_biome.wgsl` (the
//! ground vertex displacement). Keep all three in lock-step: change a coefficient here
//! ⇒ change it in the shader.
//!
//! Design: a smooth rolling BASE (gentle, walkable) plus isolated steep MESAS/BUTTES
//! that read as CLIFFS. The mesas are zero off their footprint, so the walkable base
//! stays a single connected region — you walk AROUND cliffs, never up them — which is
//! what keeps the world feasible under slope-based collision + A* path routing.
//!
//! PER-RUN VARIETY: the height field is a fixed function of world position, so without
//! help every run would grow the SAME hills/mesas at the hub (and the clear path would
//! bend around them identically — "the same corridor every time"). Every sampler takes a
//! world `(ox, oz)` OFFSET derived from the run seed ([`seed_offset`]); shifting the
//! sample point walks a different region of the infinite field, so each run's terrain —
//! and the route through it — is different. The server computes the offset from the seed
//! and sends it to the client, so both sample the identical field.

/// GLSL-style smoothstep (matches WGSL `smoothstep`).
fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// The world-space terrain offset for a run `seed`: shifts the whole height field so
/// each run's hills/mesas (and the routes that bend around them) differ. A splitmix64
/// hash spread over a wide range — big enough to walk many wavelengths into the field
/// for real variety, bounded so the sine arguments stay precise enough that the Rust and
/// WGSL evaluations agree (entities sit flush on the rendered ground). MUST be applied
/// identically on server + client; the server sends the resulting `(ox, oz)` to the
/// client on `run.started` so neither recomputes from the (client-hidden) seed.
pub fn seed_offset(seed: u64) -> (f32, f32) {
    fn mix(mut z: u64) -> u64 {
        z = z.wrapping_add(0x9E37_79B9_7F4A_7C15);
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    // ±16000 world units: ~16 periods of the longest base wave (period ≈ 997) in each
    // axis, so the hub lands somewhere genuinely different every run.
    const RANGE: f32 = 32_000.0;
    let ox = (mix(seed) % 32_000) as f32 - RANGE * 0.5;
    let oz = (mix(seed ^ 0xD1B5_4A32_D192_ED03) % 32_000) as f32 - RANGE * 0.5;
    (ox, oz)
}

/// Continuous ground height at world `(x, z)`, sampled through the run's `(ox, oz)`
/// offset (see module docs / [`seed_offset`]). Pass `(0.0, 0.0)` for the un-shifted field.
pub fn height(x: f32, z: f32, ox: f32, oz: f32) -> f32 {
    let (x, z) = (x + ox, z + oz);
    // Gentle rolling base — the WALKABLE terrain (long-wavelength hills + detail).
    let base = 9.0 * (x * 0.0063 + 0.4).sin() * (z * 0.0071 - 0.3).cos()
        + 4.5 * (x * 0.015 - 0.8).sin() * (z * 0.013 + 0.5).cos()
        + 2.2 * (x * 0.033 + 1.7).sin() * (z * 0.037 - 0.9).cos()
        + 0.9 * ((x + z) * 0.061 + 2.3).sin();
    // Small, SPARSE steep buttes = the CLIFFS. The mesa mask spikes only where `m`
    // crosses the [0.80, 0.92] smoothstep band, so buttes are isolated bumps that leave
    // the walkable base a single connected region. Slope collision walls their faces and
    // the guaranteed route (`Arena::astar_route`) bends AROUND them through walkable
    // cells, so the world stays feasible. `CLIFF_HEIGHT` MUST match the WGSL mirror.
    let m = (x * 0.03 + 1.1).sin() * (z * 0.028 - 0.6).cos()
        + 0.5 * (x * 0.051 - 2.0).sin() * (z * 0.047 + 1.4).cos();
    const CLIFF_HEIGHT: f32 = 11.0;
    base + CLIFF_HEIGHT * smoothstep(0.80, 0.92, m)
}

/// Terrain slope (gradient magnitude) at `(x, z)` under offset `(ox, oz)`, by central
/// finite differences — works for any `height` with no analytic derivative. Shallow on
/// the rolling base, large on a cliff face.
pub fn slope(x: f32, z: f32, ox: f32, oz: f32) -> f32 {
    const E: f32 = 1.5;
    let dx = (height(x + E, z, ox, oz) - height(x - E, z, ox, oz)) / (2.0 * E);
    let dz = (height(x, z + E, ox, oz) - height(x, z - E, ox, oz)) / (2.0 * E);
    (dx * dx + dz * dz).sqrt()
}

/// Slope at/above which terrain is an impassable CLIFF (movement blocked, A* avoids).
/// Below it is a walkable slope. Tuned to the mesa-edge steepness (~1.85) vs the base
/// roll (~0.25), so gentle hills stay walkable and only mesa faces wall you off.
pub const WALKABLE_SLOPE: f32 = 0.75;

/// Is `(x, z)` walkable ground (not a cliff face) under offset `(ox, oz)`? Movement collision.
pub fn walkable(x: f32, z: f32, ox: f32, oz: f32) -> bool {
    slope(x, z, ox, oz) < WALKABLE_SLOPE
}

/// Stricter threshold for PATH ROUTING (A*): the guaranteed route stays well clear of
/// the collision threshold, so a continuously-moving walker never clips a
/// just-over-`WALKABLE_SLOPE` spot beside the route and stalls on the boundary.
pub const ROUTE_SLOPE: f32 = 0.32;

/// Is `(x, z)` safe to route the guaranteed path through (with margin below the cliff)?
pub fn routable(x: f32, z: f32, ox: f32, oz: f32) -> bool {
    slope(x, z, ox, oz) < ROUTE_SLOPE
}
