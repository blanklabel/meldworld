//! **BRIDGES**: where the clear trail crosses a strait, the crossing is SPANNED rather than
//! the strait being refused — so a continent can still be divided where the route runs.
use meld_balance::Balance;
use meld_world::Arena;

const SEEDS: [u64; 8] = [1, 7, 16, 42, 99, 424242, 1_000_003, 31];

fn worlds(b: &Balance) -> Vec<Arena> {
    SEEDS.iter().map(|s| Arena::generate(b, *s, false)).collect()
}

/// **THE POINT OF THE FEATURE.** The trail must never run through water — that is the
/// guarantee bridges exist to keep while still letting a strait be cut across the route.
#[test]
fn the_trail_is_never_in_the_water() {
    let b = Balance::load_default().unwrap();
    for (i, a) in worlds(&b).into_iter().enumerate() {
        let shore = a.shore();
        let wet = a
            .path
            .iter()
            .filter(|w| !shore.is_land(w.x as f32, w.y as f32))
            .count();
        assert_eq!(wet, 0, "seed {}: {wet} of {} trail vertices in water", SEEDS[i], a.path.len());
    }
}

/// A bridge is only worth having if it SPANS something. One standing on dry ground is a
/// flagstone path, which is scenery pretending to be a mechanism.
#[test]
fn every_bridge_spans_water_that_would_otherwise_stop_you() {
    let b = Balance::load_default().unwrap();
    let mut spans = 0;
    for (i, a) in worlds(&b).into_iter().enumerate() {
        for br in &a.bridges {
            spans += 1;
            // Without its own span the midpoint would be sea: rebuild the shoreline with this
            // bridge removed and ask.
            let others: Vec<meld_proto::coast::Bridge> =
                a.bridges.iter().copied().filter(|o| o != br).collect();
            let bare = meld_proto::coast::Shore {
                arc_half: a.radial_half() as f32,
                terrain_off: a.terrain_offset(),
                peaks: &a.peaks,
                straits: &a.straits,
                lobes: &a.lobes,
                basins: &a.basins,
                rivers: &a.rivers,
                bridges: &others,
            };
            let mid = (
                (br[0] + br[2]) * 0.5,
                (br[1] + br[3]) * 0.5,
            );
            assert!(
                bare.sea(mid.0, mid.1) > 0.0,
                "seed {}: a bridge spans dry ground — that is a path, not a bridge",
                SEEDS[i]
            );
        }
    }
    // Not asserted as a floor: whether any seed's trail crosses a strait at all is the
    // world's business. What matters is that the ones that exist are real.
    println!("{spans} bridges across {} seeds", SEEDS.len());
}

/// A bridge must be WALKABLE end to end, or it is a wall with a road painted on it. Asked
/// through `is_land`, which is what the pathfinder and every mover consult.
#[test]
fn a_bridge_is_walkable_from_end_to_end() {
    let b = Balance::load_default().unwrap();
    for (i, a) in worlds(&b).into_iter().enumerate() {
        let shore = a.shore();
        for br in &a.bridges {
            for k in 0..=40 {
                let t = k as f32 / 40.0;
                let (x, z) = (br[0] + (br[2] - br[0]) * t, br[1] + (br[3] - br[1]) * t);
                assert!(
                    shore.is_land(x, z),
                    "seed {}: a bridge is under water {:.0}% along its own span",
                    SEEDS[i],
                    t * 100.0
                );
            }
        }
    }
}

/// The deck stands ABOVE the waterline, which is the whole difference between a bridge and an
/// isthmus — an isthmus is the sea not being there, a bridge is a span over it.
#[test]
fn the_deck_stands_above_the_water_and_wears_a_parapet() {
    let b = Balance::load_default().unwrap();
    let mut checked = 0;
    for a in worlds(&b) {
        for br in &a.bridges {
            let mid = ((br[0] + br[2]) * 0.5, (br[1] + br[3]) * 0.5);
            let (h, on_parapet) = meld_proto::terrain::bridge_surface(mid.0, mid.1, &a.bridges)
                .expect("a bridge's own midpoint is on its span");
            assert!(h > 0.0, "the deck must stand above sea level");
            assert_eq!(on_parapet, 0.0, "the middle of a span is deck, not parapet");
            // …and its edge is a parapet, raised further.
            let (dx, dz) = (br[2] - br[0], br[3] - br[1]);
            let n = (dx * dx + dz * dz).sqrt().max(1e-6);
            let (px, pz) = (-dz / n, dx / n);
            let edge = (mid.0 + px * br[4] * 0.92, mid.1 + pz * br[4] * 0.92);
            if let Some((eh, ep)) = meld_proto::terrain::bridge_surface(edge.0, edge.1, &a.bridges) {
                assert_eq!(ep, 1.0, "a span's outer band is its parapet");
                assert!(eh > h, "a parapet stands above its own deck");
            }
            checked += 1;
        }
    }
    println!("{checked} decks checked");
}

/// ⚠️ **A FEATURE WITH NO INSTANCES PASSES EVERY TEST IT HAS.** Before the direct line was
/// bridged, straits existed in every world and the trail crossed none of them — so all four
/// invariants above passed over an empty set and reported nothing wrong. Hold the population
/// itself, or the suite goes green on a feature that never fires.
#[test]
fn bridges_actually_get_built() {
    let b = Balance::load_default().unwrap();
    let total: usize = worlds(&b).iter().map(|a| a.bridges.len()).sum();
    assert!(
        total >= 3,
        "only {total} bridges across {} seeds — the crossing is not being chosen, so every \
         other bridge invariant is passing over an empty set",
        SEEDS.len()
    );
}

#[test]
fn census_do_straits_even_meet_the_trail() {
    let b = Balance::load_default().unwrap();
    for (i, a) in worlds(&b).into_iter().enumerate() {
        let mut crossings = 0;
        for st in &a.straits {
            for w in &a.path {
                if meld_proto::coast::strait_depth(w.x as f32, w.y as f32, st) > 0.0 {
                    crossings += 1;
                }
            }
        }
        println!(
            "seed {:>7}: {} straits, {} bridges, {} trail vertices inside a strait",
            SEEDS[i], a.straits.len(), a.bridges.len(), crossings
        );
    }
}
