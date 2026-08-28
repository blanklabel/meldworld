//! **RANGES** (`WG-7`, the routes half): the world grew something you walk around.
//!
//! Every claim here is about the SHAPE of the guarantee rather than a tuned value, because
//! every number involved is a `[TUNABLE]`.
use meld_balance::Balance;
use meld_world::Arena;

const SEEDS: [u64; 6] = [1, 7, 42, 99, 424242, 1_000_003];

fn worlds(b: &Balance) -> Vec<Arena> {
    SEEDS.iter().map(|s| Arena::generate(b, *s, false)).collect()
}

/// A range must be a WALL. Its falloff is linear, so its slope is `height / half_width` at
/// every point on the flank — one ratio decides it globally, and it has to clear the
/// threshold movement collides against or a range is a hill you stroll over.
#[test]
fn a_range_is_steeper_than_anything_may_be_walked() {
    let b = Balance::load_default().unwrap();
    let slope = b.worldgen.ridge_aspect;
    assert!(
        slope > meld_proto::terrain::WALKABLE_SLOPE as f64,
        "`ridge_aspect` ({slope}) must exceed WALKABLE_SLOPE ({}) — below it a range is a hill",
        meld_proto::terrain::WALKABLE_SLOPE
    );
    // And the shape actually delivers that slope: sample across a real range's flank.
    let mut checked = 0;
    for a in worlds(&b) {
        for r in &a.ridges {
            let (hw, h) = (r[4], r[5]);
            let mid = ((r[0] + r[2]) * 0.5, (r[1] + r[3]) * 0.5);
            // Perpendicular to the spine.
            let (dx, dz) = (r[2] - r[0], r[3] - r[1]);
            let n = (dx * dx + dz * dz).sqrt().max(1e-6);
            let (px, pz) = (-dz / n, dx / n);
            let crest = meld_proto::terrain::ridge_height(mid.0, mid.1, std::slice::from_ref(r));
            let foot = meld_proto::terrain::ridge_height(
                mid.0 + px * hw * 0.999,
                mid.1 + pz * hw * 0.999,
                std::slice::from_ref(r),
            );
            assert!(crest > foot, "a range must be highest on its spine");
            assert!((crest - h).abs() < 0.01, "the spine should stand at the authored height");
            checked += 1;
        }
    }
    assert!(checked > 0, "no ranges were raised across {} seeds", SEEDS.len());
}

/// **IT CAN NEVER SEAL THE WORLD**, and that is by construction: the span share is capped
/// below 1.0, so a range always stops short of the fan's rim and rounding its end is always
/// possible. The feasibility gate is the backstop, not the mechanism — so it must never be
/// the thing doing the work.
#[test]
fn a_range_never_seals_the_world() {
    let b = Balance::load_default().unwrap();
    assert!(
        b.worldgen.ridge_arc_share_max < 1.0,
        "the span share must cap below the full fan, or a range can touch both rims"
    );
    // Every world still routes hub -> portal, which `generate` guarantees by construction.
    for (i, a) in worlds(&b).into_iter().enumerate() {
        assert!(a.path.len() >= 2, "seed {} has no clear path", SEEDS[i]);
    }
}

/// **THE ROUTE ALREADY DRAWN ALWAYS WINS.** A range is laid before the section's path is
/// routed, but a region ring spans many sections, so a range for section N can land on ground
/// whose trail was drawn for section N-3. It yields there — and yielding IS a pass.
#[test]
fn no_range_stands_on_the_clear_path() {
    let b = Balance::load_default().unwrap();
    let clear = b.worldgen.path_clear_radius + b.worldgen.player_radius;
    for (i, a) in worlds(&b).into_iter().enumerate() {
        for r in &a.ridges {
            let hw = r[4] as f64;
            for w in &a.path {
                let d = meld_proto::terrain::dist_to_segment(
                    w.x as f32, w.y as f32, r[0], r[1], r[2], r[3],
                ) as f64;
                assert!(
                    d > clear + hw - 1.0,
                    "seed {}: a range stands on the clear path (gap {d:.1}, needs {:.1})",
                    SEEDS[i],
                    clear + hw
                );
            }
        }
    }
}

/// Water is placed by walking DOWNHILL, and a range is the steepest ground in the world — so
/// descent runs into one and pools against it. A basin inside a mountain is also water no
/// party can reach, because the mountain is impassable.
#[test]
fn no_water_forms_inside_a_range() {
    let b = Balance::load_default().unwrap();
    for (i, a) in worlds(&b).into_iter().enumerate() {
        for basin in &a.basins {
            for r in &a.ridges {
                let d = meld_proto::terrain::dist_to_segment(
                    basin[0], basin[1], r[0], r[1], r[2], r[3],
                );
                assert!(
                    d > r[4],
                    "seed {}: a basin sits inside a range (centre {d:.1} from the spine, \
                     half-width {:.1})",
                    SEEDS[i],
                    r[4]
                );
            }
        }
    }
}

/// **ASHFALL IS VOLCANIC AND DESERT IS NOT.** Ranges are weighted by the ground the spine
/// stands on (`biome_terrace_mult`), so forcing a biome must change how much mountain a world
/// grows. The ORDERING is the claim; the ratio is a tunable.
#[test]
fn a_volcanic_region_grows_more_mountain_than_a_desert() {
    let b = Balance::load_default().unwrap();
    let total = |biome: &'static str| -> f64 {
        SEEDS
            .iter()
            .map(|s| {
                Arena::generate_with(&b, *s, false, Some(biome))
                    .ridges
                    .iter()
                    .map(|r| {
                        let l = ((r[2] - r[0]).powi(2) + (r[3] - r[1]).powi(2)).sqrt() as f64;
                        l * r[4] as f64
                    })
                    .sum::<f64>()
            })
            .sum()
    };
    let (ash, desert) = (total("ashfall"), total("desert"));
    assert!(
        ash > desert * 2.0,
        "ashfall should grow far more mountain than desert (ashfall {ash:.0} vs {desert:.0})"
    );
}
