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
#[test]
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
