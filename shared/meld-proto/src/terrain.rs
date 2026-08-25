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
    // Impassable steep cliff-mesas are OFF (amplitude 0): even sparse, they rendered as
    // stair-stepped blocky WALLS (the coarse ground grid can't smooth an 11u vertical
    // face) that the player kept reading as a corridor. The world is now open, gentle,
    // fully-walkable rolling hills. The cliff mask + slope-collision + A* routing all
    // still work if we bring dramatic terrain back later — but as WALKABLE tall hills
    // (raise this gently, e.g. 4-6, and widen the band), not impassable blocky faces.
    // `CLIFF_HEIGHT` MUST match the WGSL mirror.
    let m = (x * 0.03 + 1.1).sin() * (z * 0.028 - 0.6).cos()
        + 0.5 * (x * 0.051 - 2.0).sin() * (z * 0.047 + 1.4).cos();
    const CLIFF_HEIGHT: f32 = 0.0;
    base + CLIFF_HEIGHT * smoothstep(1.15, 1.30, m)
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

/// Max authored landmark peaks the ground shader blends at once (windowed around the
/// player like the biome rings). The run may hold more across all sections.
pub const MAX_PEAKS: usize = 24;

/// Keep an authored dome WALKABLE (climbable): its steepest slope (raised-cosine, at
/// d = radius/2) is `height·π/(2·radius)`, so `height ≤ radius · PEAK_MAX_ASPECT` stays
/// under `WALKABLE_SLOPE` with margin — you can climb the mountain from any side.
pub const PEAK_MAX_ASPECT: f32 = 0.42;

/// Extra height from authored CLIMBABLE landmark peaks at world `(x, z)` — smooth
/// raised-cosine DOMES (never a cliff), each `[cx, cz, radius, height]`. This is summed
/// ONTO [`height`] on the server, the client, and the WGSL mirror, so a mountain renders,
/// the ground rises under you as you climb, and the summit reward sits on top. Domes are
/// gentle by construction (`height ≤ radius · PEAK_MAX_ASPECT`), so they add no collision
/// or path-routing cost — hence they live outside `slope`/`walkable`/`routable`. Peaks are
/// placed at true world positions (NOT through the seed offset). MUST match the WGSL mirror.
pub fn peak_height(x: f32, z: f32, peaks: &[[f32; 4]]) -> f32 {
    let mut h = 0.0;
    for p in peaks {
        let (cx, cz, r, ph) = (p[0], p[1], p[2], p[3]);
        if r <= 0.0 {
            continue;
        }
        let d = ((x - cx) * (x - cx) + (z - cz) * (z - cz)).sqrt();
        if d < r {
            h += ph * 0.5 * (1.0 + (std::f32::consts::PI * d / r).cos());
        }
    }
    h
}

/// Land-side width of the shore blend — the BEACH. Ground ramps from the land's own height
/// down to sea level over this many units INSIDE the shoreline, so a coast has a beach
/// rather than a cliff.
///
/// ⚠️ The ground shader's `total_height` mirrors this as a literal (`smoothstep(-14.0,
/// 0.0, sea)`); `the_beach_blend_matches_the_shader` reads the .wgsl and holds the two
/// together, because this is the number that decides where the ground SURFACE is and a
/// disagreement puts everything the game places at a different height than it draws.
pub const BEACH_BLEND: f32 = 14.0;

/// **Where the ground surface actually is** — the one rule, folding the sea into a land
/// height. `sea_depth` is signed (negative inland, positive at sea, from
/// `coast::sea_depth` or `coast::city_sea_depth`), `amp` flattens the LAND for
/// hand-placed scenes, and `sea_level` is the water's own y.
///
/// ⚠️ THIS EXISTS BECAUSE EVERYTHING FLOATED. The ground shader has dipped its vertices
/// toward sea level at every coast for a while, and [`height`] — the function that places
/// every tree, prop, building, creature and the PLAYER — knew nothing about the sea at
/// all. So at any shoreline the ground fell away and the whole world stayed up at the
/// land's height, standing on nothing. Don caught it the moment Last City got a coast:
/// "trees, the castle, and yes… even the player now floats."
///
/// It is deliberately given the same shape as the shader's `mix(amp * land, level, t)`,
/// and it takes NO swell term: the swell only ramps in past the waterline, where nothing
/// stands, and a prop that bobbed with the waves would be worse than one that did not.
pub fn with_sea(land: f32, sea_depth: f32, amp: f32, sea_level: f32) -> f32 {
    let t = smoothstep(-BEACH_BLEND, 0.0, sea_depth);
    (amp * land) * (1.0 - t) + sea_level * t
}

#[cfg(test)]
mod sea_fold_tests {
    use super::*;

    #[test]
    fn the_fold_is_land_inland_and_sea_level_offshore() {
        // Well inland the sea is not consulted at all…
        assert_eq!(with_sea(9.0, -500.0, 1.0, -7.0), 9.0);
        // …past the waterline the surface IS the water's level, flat, from the very first
        // fragment. (The beach is the band INSIDE the shore, never past it — that inversion
        // is what made every coast render as the bank of a pit.)
        assert_eq!(with_sea(9.0, 0.0, 1.0, -7.0), -7.0);
        assert_eq!(with_sea(9.0, 40.0, 1.0, -7.0), -7.0);
    }

    #[test]
    fn a_flat_scene_still_has_a_sea_to_dip_into() {
        // `amp` 0 is the City and the menus: hand-placed level ground. It must flatten the
        // LAND without flattening the water, or the city's plaza cannot dip into its bay —
        // which is exactly the bug that kept Last City's sea sitting on top of its lawn.
        assert_eq!(with_sea(9.0, -500.0, 0.0, -7.0), 0.0);
        assert_eq!(with_sea(9.0, 10.0, 0.0, -7.0), -7.0);
    }

    #[test]
    fn the_beach_is_monotonic_downhill_to_the_water() {
        // A beach has to fall the whole way in. A non-monotonic ramp reads as a lip or a
        // dune at the waterline and, worse, would place props above or below their ground.
        let mut prev = f32::INFINITY;
        for i in 0..=140 {
            let sea = -BEACH_BLEND + (i as f32 / 140.0) * BEACH_BLEND;
            let h = with_sea(9.0, sea, 1.0, -7.0);
            assert!(h <= prev + 1e-4, "the beach rises at sea depth {sea}");
            prev = h;
        }
    }
}
