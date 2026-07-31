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

/// GLSL-style smoothstep (matches WGSL `smoothstep`).
fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Continuous ground height at world `(x, z)`.
pub fn height(x: f32, z: f32) -> f32 {
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

/// Terrain slope (gradient magnitude) at `(x, z)`, by central finite differences —
/// works for any `height` with no analytic derivative. Shallow on the rolling base,
/// large on a cliff face.
pub fn slope(x: f32, z: f32) -> f32 {
    const E: f32 = 1.5;
    let dx = (height(x + E, z) - height(x - E, z)) / (2.0 * E);
    let dz = (height(x, z + E) - height(x, z - E)) / (2.0 * E);
    (dx * dx + dz * dz).sqrt()
}

/// Slope at/above which terrain is an impassable CLIFF (movement blocked, A* avoids).
/// Below it is a walkable slope. Tuned to the mesa-edge steepness (~1.85) vs the base
/// roll (~0.25), so gentle hills stay walkable and only mesa faces wall you off.
pub const WALKABLE_SLOPE: f32 = 0.75;

/// Is `(x, z)` walkable ground (not a cliff face)? Used by movement collision.
pub fn walkable(x: f32, z: f32) -> bool {
    slope(x, z) < WALKABLE_SLOPE
}

/// Stricter threshold for PATH ROUTING (A*): the guaranteed route stays well clear of
/// the collision threshold, so a continuously-moving walker never clips a
/// just-over-`WALKABLE_SLOPE` spot beside the route and stalls on the boundary.
pub const ROUTE_SLOPE: f32 = 0.32;

/// Is `(x, z)` safe to route the guaranteed path through (with margin below the cliff)?
pub fn routable(x: f32, z: f32) -> bool {
    slope(x, z) < ROUTE_SLOPE
}
