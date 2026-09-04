//! **THE DECIDED MAZE IS CONNECTED, AND IT IS NOT A BARE TREE** (`WG-11` stage 8).
//!
//! The independent per-boundary roll it replaces left an ashfall region as 37 islands
//! (`the_maze_graph`, `--ignored`). These are the two properties that must hold instead, and
//! they pull against each other: a spanning tree is connected but is a PERFECT maze with one
//! path between any two cells, while braiding buys loops at the price of dead ends — which is
//! where `WG-11` puts its reward.
use meld_balance::Balance;
use meld_proto::regions::{Cell, Grid};
use meld_world::{maze, Arena};
use std::collections::HashSet;

/// The SAME predicate the generator uses — never a second copy of it, which is how the two
/// answers to "is this cell land" drift apart.
fn land_of(g: &Grid, arc_half: f32) -> impl Fn(Cell) -> bool + '_ {
    move |c: Cell| maze::cell_holds_land(g, arc_half, c)
}

fn components(g: &Grid, m: &maze::Maze, cells: &HashSet<u32>) -> (usize, usize) {
    let keys: Vec<u32> = {
        let mut v: Vec<u32> = cells.iter().copied().collect();
        v.sort_unstable();
        v
    };
    let idx: std::collections::HashMap<u32, usize> =
        keys.iter().enumerate().map(|(i, k)| (*k, i)).collect();
    let mut parent: Vec<usize> = (0..keys.len()).collect();
    for &k in &keys {
        for n in g.neighbours(Cell::from_key(k)) {
            if !cells.contains(&n.key()) || !m.is_open(Cell::from_key(k), n) {
                continue;
            }
            let (mut x, mut y) = (idx[&k], idx[&n.key()]);
            while parent[x] != x { x = parent[x]; }
            while parent[y] != y { y = parent[y]; }
            if x != y { parent[x] = y; }
        }
    }
    let mut sizes: std::collections::HashMap<usize, usize> = Default::default();
    for i in 0..keys.len() {
        let mut r = i;
        while parent[r] != r { r = parent[r]; }
        *sizes.entry(r).or_default() += 1;
    }
    (sizes.len(), sizes.values().copied().max().unwrap_or(0))
}

fn world(seed: u64) -> (Balance, Arena) {
    let b = Balance::load_default().unwrap();
    let a = Arena::generate(&b, seed, false);
    (b, a)
}

#[test]
fn the_decided_maze_is_whole_at_every_braid_rate() {
    for seed in [1u64, 7, 42, 99, 424242] {
        let (_b, a) = world(seed);
        let g = a.regions();
        let arc_half = a.radial_half() as f32;
        let land = land_of(&g, arc_half);
        for braid in [0.0f64, 0.05, 0.10, 0.20] {
            let m = maze::build(&g, seed, 3200.0, braid, &land);
            let (n, _open, _ends) = m.counts();
            let cells: HashSet<u32> = {
                let mut s = HashSet::new();
                let rings = (3200.0 / g.ring_step as f64).ceil() as u32 + 1;
                for ring in 0..rings {
                    for sector in 0..g.sectors(ring) {
                        let c = Cell::new(ring, sector);
                        if land(c) { s.insert(c.key()); }
                    }
                }
                s
            };
            let (comps, largest) = components(&g, &m, &cells);
            assert!(n > 50, "seed {seed}: the maze saw only {n} cells");
            assert_eq!(
                comps, 1,
                "seed {seed} braid {braid}: the maze is {comps} components, largest holding \
                 {largest} of {n} cells — a DECIDED topology must be connected by construction, \
                 which is the entire reason it replaced the per-boundary roll"
            );
        }
    }
}

#[test]
fn braiding_buys_loops_and_spends_dead_ends() {
    let (_b, a) = world(424242);
    let g = a.regions();
    let arc_half = a.radial_half() as f32;
    let land = land_of(&g, arc_half);
    let mut rows = Vec::new();
    for braid in [0.0f64, 0.05, 0.10, 0.20] {
        let m = maze::build(&g, 424242, 3200.0, braid, &land);
        let (n, open, ends) = m.counts();
        // A tree over n cells uses n-1 edges; anything above that is an independent loop.
        let loops = open.saturating_sub(n.saturating_sub(1));
        println!("braid {braid:.2}: {n} cells, {open} open, {loops} loops, {ends} dead ends");
        rows.push((braid, loops, ends));
    }
    let tree = rows[0];
    assert_eq!(tree.1, 0, "a spanning tree must have no loops, got {}", tree.1);
    assert!(tree.2 > 0, "a spanning tree over this graph must have dead ends");
    // Loops must RISE with the braid rate and dead ends must FALL. The magnitudes are
    // [TUNABLE]; the ordering is the design.
    for w in rows.windows(2) {
        assert!(
            w[1].1 >= w[0].1,
            "braid {:.2} gave {} loops against {:.2}'s {} — braiding must buy loops",
            w[1].0, w[1].1, w[0].0, w[0].1
        );
        assert!(
            w[1].2 <= w[0].2,
            "braid {:.2} left {} dead ends against {:.2}'s {} — a braid SPENDS a dead end, \
             which is what makes it a trade against dungeon sites",
            w[1].0, w[1].2, w[0].0, w[0].2
        );
    }
}

/// **A RELIEF MASS SITS IN ITS OWN CELL, AND MEETS THE ONES IT SHOULD** (`WG-11` stage 9).
///
/// The two properties that make `maze::cell_mass` the right primitive rather than merely a
/// function: a mass is INSIDE the cell that grew it (so nothing is positioned relative to a
/// boundary, which is what made stage 8's walls straight lines), and two masses across a
/// boundary the maze WALLS reach each other (so the wall exists as terrain rather than as a
/// line drawn on the edge).
/// ⚠️ **`#[ignore]`d: STAGE 9 WORK IN FLIGHT.** The primitive exists; the emission is not
/// wired to it. Two constraints do not hold together yet — correcting for the grid warp so a
/// mass lands in its OWN cell (`cell_at` derives the ring from `r + warp_at(bearing)` while
/// `centroid` is nominal, and the warp is comparable to HALF A RING) moves the masses far
/// enough that walled neighbours stop meeting. Run with `--ignored` to see both numbers.
#[test]
#[ignore = "WG-11 stage 9 in flight: warp correction and meeting are not both satisfied yet"]
fn a_relief_mass_stays_home_and_meets_its_walled_neighbours() {
    let b = Balance::load_default().unwrap();
    for seed in [1u64, 42, 424242] {
        let a = Arena::generate(&b, seed, false);
        let g = a.regions();
        let arc_half = a.radial_half() as f32;
        let land = land_of(&g, arc_half);
        let m = maze::build(&g, seed, 3200.0, 0.10, &land);
        let mut checked = 0usize;
        let mut met = 0usize;
        let mut pairs = 0usize;
        for ring in 1..12u32 {
            for sector in 0..g.sectors(ring) {
                let c = Cell::new(ring, sector);
                if !land(c) {
                    continue;
                }
                let Some((mx, my, reach)) = maze::cell_mass(&g, &m, c, seed) else { continue };
                checked += 1;
                // ⚠️ **INSIDE ITS OWN CELL.** Not on the boundary, not in the neighbour.
                let here = g.cell_at(mx as f32, my as f32);
                assert_eq!(
                    here.key(),
                    c.key(),
                    "seed {seed}: the mass for ring {ring} sector {sector} landed in ring {} \
                     sector {} — a mass belongs to the cell that grew it, or it is being \
                     positioned relative to a boundary again",
                    here.ring,
                    here.sector
                );
                assert!(reach > 0.0, "a mass with no reach walls nothing");
                // …and it reaches its walled neighbours' masses.
                for n in g.neighbours(c) {
                    if !land(n) || m.is_open(c, n) {
                        continue;
                    }
                    let Some((nx, ny, nreach)) = maze::cell_mass(&g, &m, n, seed) else { continue };
                    pairs += 1;
                    if (mx - nx).hypot(my - ny) <= reach + nreach + 1e-6 {
                        met += 1;
                    }
                }
            }
        }
        assert!(checked > 20, "seed {seed}: only {checked} cells grew a mass");
        assert!(pairs > 20, "seed {seed}: only {pairs} walled pairs to check");
        // ⚠️ The RATIO, not every pair: `reach` is half the way to the NEAREST walled
        // neighbour, so a cell walled on several sides under-reaches the further ones. That is
        // the spur work this primitive is the foundation for, and it is honest to say the
        // shortfall is bounded rather than to claim it does not exist.
        assert!(
            met * 4 >= pairs * 3,
            "seed {seed}: only {met} of {pairs} walled pairs have masses that meet — a wall \
             the ground does not express is a gate with nothing in it"
        );
    }
}

/// **A RANGE CROSSES ITS BOUNDARY, IT DOES NOT TRACE IT** (`WG-11` stage 9).
///
/// The measurable form of *"straight mountain lines"*. A range laid down a shared edge is
/// PARALLEL to that edge, so the cosine between its spine and the boundary's local direction
/// sits near 1 — and because a cell boundary is an exact arc or an exact radial ray, parallel
/// to one means straight. Grown between the two cells' masses instead, the spine leans
/// wherever their relief sits and crosses the boundary rather than following it.
///
/// Reported rather than merely asserted: the bound is loose on purpose, because the quantity
/// that matters is the DISTRIBUTION and the numbers here are what should move if stage 9's
/// later pieces (spurs, coalescence) land.
#[test]
fn a_range_crosses_its_boundary_rather_than_tracing_it() {
    let b = Balance::load_default().unwrap();
    let mut aligned = 0usize;
    let mut total = 0usize;
    let mut sum = 0.0f64;
    for seed in [1u64, 42, 424242] {
        let mut a = Arena::generate(&b, seed, false);
        let mut r = 0.0;
        while r < 900.0 {
            r += 50.0;
            a.ensure_frontier(&b, r);
        }
        let g = a.regions();
        let arc_half = a.radial_half() as f32;
        for rg in a.ridges.iter().filter(|r| r[4] > 0.0) {
            let (ax, ay) = (rg[0] as f64, rg[1] as f64);
            let (bx, by) = (rg[2] as f64, rg[3] as f64);
            let (sx, sy) = (bx - ax, by - ay);
            let slen = sx.hypot(sy);
            if slen < 1.0 {
                continue;
            }
            let (mx, my) = (0.5 * (ax + bx), 0.5 * (ay + by));
            // The boundary nearest the spine's MIDPOINT, and its local direction there.
            let here = g.cell_at(mx as f32, my as f32);
            let mut best: Option<(f64, (f64, f64))> = None;
            for n in g.neighbours(here) {
                if !maze::cell_holds_land(&g, arc_half, n) {
                    continue;
                }
                let Some(((r0, b0), (r1, b1))) = maze::shared_boundary(&g, here, n) else {
                    continue;
                };
                for k in 0..8 {
                    let (t0, t1) = (k as f64 / 8.0, (k + 1) as f64 / 8.0);
                    let p = |t: f64| {
                        let (rr, bb) = (r0 + (r1 - r0) * t, b0 + (b1 - b0) * t);
                        (rr * bb.cos(), rr * bb.sin())
                    };
                    let (q0, q1) = (p(t0), p(t1));
                    let d = (mx - 0.5 * (q0.0 + q1.0)).hypot(my - 0.5 * (q0.1 + q1.1));
                    if best.is_none_or(|(bd, _)| d < bd) {
                        best = Some((d, (q1.0 - q0.0, q1.1 - q0.1)));
                    }
                }
            }
            let Some((_, (dx, dy))) = best else { continue };
            let dlen = dx.hypot(dy);
            if dlen < 1e-6 {
                continue;
            }
            let cos = ((sx * dx + sy * dy) / (slen * dlen)).abs();
            total += 1;
            sum += cos;
            if cos > 0.9 {
                aligned += 1;
            }
        }
    }
    assert!(total > 20, "only {total} ranges to measure");
    let mean = sum / total as f64;
    println!(
        "range/boundary alignment over {total} ranges: mean |cos| {mean:.3}, \
         {aligned} ({:.0}%) are near-parallel (|cos| > 0.9)",
        100.0 * aligned as f64 / total as f64
    );
    // A range laid ALONG its boundary would put this near 1.0 and nearly every range in the
    // near-parallel bucket. Loose bound: the point is the report, and the design claim is only
    // that a range no longer FOLLOWS the grid.
    assert!(
        mean < 0.95,
        "mean |cos| {mean:.3} — ranges are still tracing their boundaries, so the grid is \
         still visible in the mountains"
    );
}

/// **A RANGE MUST NOT BLOCK A PASS.** Owner's priority, stated exactly: *"I don't care how big
/// ranges are as long as they don't block the maze."*
///
/// So this does not measure a range's size at all. It measures the one thing that would make a
/// big one unacceptable: whether the maze's OPEN boundaries — the ways through, the only
/// reason the world is connected — still have a gap a party fits through.
///
/// ⚠️ **`WG-11` stage 9 makes this the live risk.** A range used to be a capsule laid down the
/// boundary it walls, so it could only ever block THAT boundary — and that boundary was walled
/// by definition. Grown between the two cells' MASSES it spans two cell interiors, so its
/// flank can bulge across a neighbouring OPEN boundary and seal a pass the maze meant to
/// leave. Connectivity would then be guaranteed on paper by a topology the ground contradicts,
/// which is the exact failure this whole arc exists to remove.
#[test]
fn a_range_never_blocks_a_pass() {
    let b = Balance::load_default().unwrap();
    let mut open_seen = 0usize;
    let mut blocked = 0usize;
    for seed in [1u64, 42, 424242] {
        let mut a = Arena::generate(&b, seed, false);
        let mut r = 0.0;
        while r < 900.0 {
            r += 50.0;
            a.ensure_frontier(&b, r);
        }
        let g = a.regions();
        let arc_half = a.radial_half() as f32;
        // A party's width: the same clearance `astar_route` keeps.
        let pad = a.path_clear_radius_for_tests() + a.player_radius_for_tests();
        for ring in 1..8u32 {
            for sector in 0..g.sectors(ring) {
                let c = Cell::new(ring, sector);
                if !maze::cell_holds_land(&g, arc_half, c) {
                    continue;
                }
                for n in g.neighbours(c) {
                    if n.key() <= c.key()
                        || !maze::cell_holds_land(&g, arc_half, n)
                        || !a.maze.is_open(c, n)
                    {
                        continue;
                    }
                    let Some(((r0, b0), (r1, b1))) = maze::shared_boundary(&g, c, n) else {
                        continue;
                    };
                    open_seen += 1;
                    // Is ANY point along this pass clear of every range by a party's width?
                    // One is enough — a pass is a gap, not a highway.
                    let clear = (0..=16).any(|k| {
                        let t = k as f64 / 16.0;
                        let (rr, bb) = (r0 + (r1 - r0) * t, b0 + (b1 - b0) * t);
                        !a.range_blocks_for_tests(rr * bb.cos(), rr * bb.sin(), pad)
                    });
                    if !clear {
                        blocked += 1;
                    }
                }
            }
        }
    }
    assert!(open_seen > 50, "only {open_seen} open boundaries to check");
    println!(
        "{blocked} of {open_seen} passes are blocked by a range ({:.1}%)",
        100.0 * blocked as f64 / open_seen as f64
    );
    assert_eq!(
        blocked, 0,
        "{blocked} of {open_seen} of the maze's PASSES are sealed by a range — the topology \
         says the world is connected and the ground says otherwise, which is worse than \
         either being wrong alone"
    );
}
