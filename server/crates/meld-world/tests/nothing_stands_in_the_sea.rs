//! **NOTHING THE WORLD RAISES MAY STAND IN OPEN WATER.**
//!
//! Every prop the world scatters is culled off water by `radialize`'s `is_land` retain, and
//! `push_prop_walls` asks `on_land` for each prop it lays. A **range** was neither: it is a
//! capsule in the HEIGHT FIELD, so no obstacle cull touches it, and neither emitter checked.
//!
//! Spotted on the survey map (`world_map`, `--ignored`) as pale spines lying in open sea, and
//! measured at **18 of 107 ranges on seed 424242, the deepest 2,611 units offshore**. Fixing
//! `push_boundary_walls` took it to 8; the other 8 came from `push_ridges`, because a rule that
//! lives in one of two places is half a rule — the same shape as the wall-collision line that
//! went into one mover and not the other.
//!
//! ⚠️ Sampled ALONG the spine, not at its ends: a range that begins and finishes ashore can
//! still wade through a bay in the middle, which is what the endpoint-only check left behind.
use meld_balance::Balance;
use meld_world::Arena;

#[test]
fn no_range_stands_in_open_water() {
    let b = Balance::load_default().unwrap();
    for seed in [1u64, 7, 42, 424242] {
        let mut a = Arena::generate(&b, seed, false);
        let mut r = 0.0;
        while r < 2000.0 {
            r += 100.0;
            a.ensure_frontier(&b, r);
        }
        let sh = a.shore();
        let mut wet = 0usize;
        let mut deepest = 0.0f32;
        for rg in &a.ridges {
            for k in 0..=6 {
                let t = k as f32 / 6.0;
                let (x, z) = (rg[0] + (rg[2] - rg[0]) * t, rg[1] + (rg[3] - rg[1]) * t);
                let d = sh.sea(x, z);
                if d > 0.0 {
                    wet += 1;
                    deepest = deepest.max(d);
                    break;
                }
            }
        }
        assert!(!a.ridges.is_empty(), "seed {seed} raised no ranges at all to check");
        assert_eq!(
            wet, 0,
            "seed {seed}: {wet} of {} ranges wade through open water (deepest {deepest:.0} \
             units offshore) — a range is a capsule in the height field, so no obstacle cull \
             will save it and both emitters have to ask",
            a.ridges.len()
        );
    }
}
