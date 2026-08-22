//! A DISTANCE IS A RING, NOT A PLACE (WG-4).
//!
//! The fan bends corridor y into an angle, so "d1269" is a whole ring and the world's
//! clear path crosses it at exactly one arbitrary point. Anything that puts a player at a
//! depth has to ask the route where it is at that depth — otherwise it lands them on an
//! arc the path never visits, and every piece of content the world anchors to its route
//! (the end fight, the deep portal, the Gatekeeper in the pass) is a quarter-turn away.
//!
//! This is not hypothetical: the DEV/QA deep start used `(reach, 0)`, and at seed 424242 /
//! d1269 the end-fight bosses stand at angle -87 degrees while the party stood at 0 — which
//! is why that fight had never been played, by the harness or by anyone.

use meld_balance::Balance;
use meld_world::Arena;

/// Stream far enough out to hold the target ring. `ensure_frontier` caps growth per call
/// on purpose (a teleport must not explode a tick), so it is pumped the way the game loop
/// pumps it.
fn arena_out_to(b: &Balance, seed: u64, reach: f64) -> Arena {
    let mut a = Arena::generate(b, seed, false);
    for _ in 0..1024 {
        if a.ensure_frontier(b, reach).is_empty() {
            break;
        }
    }
    a
}

#[test]
fn the_route_point_at_a_depth_is_on_the_route_and_at_that_depth() {
    let b = Balance::load_default().unwrap();
    for seed in [424242u64, 1, 7, 99, 12345] {
        for reach in [200.0f64, 500.0, 1269.0] {
            let a = arena_out_to(&b, seed, reach);
            let p = a.route_point_at(reach);

            // It is one of the route's own waypoints — not a point near it, and not a
            // synthesised one that could land inside a tree.
            assert!(
                a.path.iter().any(|w| w.x == p.x && w.y == p.y),
                "seed {seed} d{reach}: the landing is not a clear-path waypoint"
            );

            // And it is at the depth that was asked for. Compared in the CORRIDOR frame,
            // because that is the frame `reach` is stated in — comparing raw world x is
            // the exact mistake the fan punishes.
            let cx = a.corridorize(&p).x;
            assert!(
                (cx - reach).abs() < 60.0,
                "seed {seed}: asked for d{reach}, the route's nearest point is d{cx:.0}"
            );
        }
    }
}

/// **The measurement that motivated the change**, kept as an assertion: the route is
/// nowhere near `(reach, 0)`, so landing there is landing off the map's own path. If this
/// ever fails because the fan flattened, the fix above stops being necessary — and that is
/// worth being told about rather than silently keeping.
#[test]
fn a_distance_is_a_ring_so_the_naive_landing_misses_the_route() {
    let b = Balance::load_default().unwrap();
    let reach = 1269.0;
    let mut gaps: Vec<(u64, f64)> = Vec::new();
    for seed in [424242u64, 1, 7, 99, 12345, 2, 3, 4, 5, 8, 11, 77, 555, 9001, 31337] {
        let a = arena_out_to(&b, seed, reach);
        let p = a.route_point_at(reach);
        gaps.push((seed, (p.x - reach).hypot(p.y)));
    }
    // A DISTRIBUTION, NOT A PER-SEED RULE — and the difference is the whole point.
    //
    // This asserted `> 100.0` for every seed and was correct for the five it happened to
    // list. Retuning creature density shifted the seeded worlds (creature placement draws
    // from a section's main RNG stream, so changing how many are placed moves everything
    // drawn after) and seed 99's clear path came to cross the centre line at exactly this
    // ring: corridor `y = 0` maps to angle 0, so its waypoint bends to `(reach, 0)` and the
    // gap is EXACTLY zero. That is not the fan flattening — it is a meandering path doing
    // the one thing it is supposed to do, and a path that never crossed its own centre line
    // would be the real bug.
    //
    // Measured over 15 seeds after the change: 14 are 178-1892u off and one is 0.
    // What proves the fix is necessary is that the naive landing is usually catastrophic
    // and can be enormous — not that it is never accidentally right.
    gaps.sort_by(|a, b| a.1.total_cmp(&b.1));
    let far = gaps.iter().filter(|(_, g)| *g > 100.0).count();
    assert!(
        far * 4 >= gaps.len() * 3,
        "only {far} of {} seeds land off-route; if the naive landing is usually FINE then \
         the fan is no longer bending and this fix is moot: {gaps:?}",
        gaps.len()
    );
    let worst = gaps.last().map(|(_, g)| *g).unwrap_or(0.0);
    assert!(
        worst > 500.0,
        "the worst seed is only {worst:.0}u off-route; the measured spread was 178-1892u"
    );
}
