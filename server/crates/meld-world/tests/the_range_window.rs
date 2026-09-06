use meld_balance::Balance;
use meld_world::Arena;

/// **THE RANGE WINDOW HOLDS WHAT A PLAYER CAN SEE** (`WG-11` stage 9).
///
/// `MAX_RIDGES` sizes a WGSL uniform array, so it is a compile-time bound that can be raised
/// but not removed — and raising it is not free, because `ridge_height` loops EVERY slot for
/// every fragment of ground. The window is nearest-first, so the question is not how many
/// ranges a world holds (143 after stage 9 grew walls mass-to-mass, up from 35) but **how many
/// stand within one interest radius of a player**. While that fits, the GPU cost is never paid.
///
/// ⚠️ It is a CORRECTNESS bound, not fidelity. A range past the window renders FLAT while
/// `ridge_blocks` still refuses it — an invisible wall a player walks into.
#[test]
fn the_range_window_holds_what_a_player_can_see() {
    let b = Balance::load_default().unwrap();
    // `interest_radius_chunks` x `chunk_size` — what the snapshot sends a player.
    let radius = 128.0f64;
    let mut worst = 0usize;
    let mut worst_at = (0u64, 0.0f64);
    for seed in [1u64, 42, 424242] {
        let mut a = Arena::generate(&b, seed, false);
        let mut reach = 0.0f64;
        while reach < 1200.0 {
            reach += 60.0;
            a.ensure_frontier(&b, reach);
        }
        // Sample where a player actually walks: along the guaranteed route.
        let mut d = 60.0f64;
        while d < reach - 120.0 {
            let p = a.route_point_at(d);
            let near = a
                .ridges
                .iter()
                .filter(|r| {
                    let (ax, az, bx, bz) = (r[0] as f64, r[1] as f64, r[2] as f64, r[3] as f64);
                    let (dx, dz) = (bx - ax, bz - az);
                    let l2 = dx * dx + dz * dz;
                    let t = if l2 > 1e-6 {
                        (((p.x - ax) * dx + (p.y - az) * dz) / l2).clamp(0.0, 1.0)
                    } else {
                        0.0
                    };
                    (p.x - (ax + dx * t)).hypot(p.y - (az + dz * t)) < radius + r[4] as f64
                })
                .count();
            if near > worst {
                worst = near;
                worst_at = (seed, d);
            }
            d += 40.0;
        }
    }
    println!(
        "worst: {worst} ranges within {radius} units of the route (seed {}, d{:.0}), window {}",
        worst_at.0,
        worst_at.1,
        meld_proto::terrain::MAX_RIDGES
    );
    assert!(
        worst <= meld_proto::terrain::MAX_RIDGES,
        "{worst} ranges stand within {radius} units of a point on the route (seed {}, d{:.0}), \
         against a window of {} — the ones past it render FLAT while `ridge_blocks` still \
         refuses them, which is a wall a player cannot see and cannot walk through",
        worst_at.0,
        worst_at.1,
        meld_proto::terrain::MAX_RIDGES
    );
}
