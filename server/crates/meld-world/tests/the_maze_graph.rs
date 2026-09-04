//! **IS THE CELL-GRAPH MAZE ACTUALLY CONNECTED?** (`WG-11`)
//!
//! ```sh
//! cargo test -p meld-world --test the_maze_graph -- --ignored --nocapture
//! ```
//!
//! `regions::pass_open` rolls each boundary INDEPENDENTLY at the biome's `porosity`, and its
//! own doc is explicit that *"connectivity is NOT this function's job"* — feasibility was
//! delegated to the routes carved through the world, which cut their own gaps. This measures
//! what that leaves for a player who steps OFF the route, and the answer is the reason
//! `WG-11` stage 8 exists.
//!
//! Two numbers matter. The **floor**, `(N-1)/E`, is the share of boundaries a perfect
//! spanning tree needs: a porosity below it cannot be connected by any arrangement. And the
//! **component count** is what the independent roll actually delivers, which is far worse
//! than the floor because a random graph needs mean-degree x p > ln(N) to be connected at
//! all. Measured at the shipped table: ashfall (0.30) is **37 islands** with the largest
//! holding 28% of the world's cells, forest (0.55) is 7, and only field (0.92) is whole.
//! Ashfall and mire sit BELOW the floor, so they are unconnectable by construction.
//!
//! `#[ignore]`d: it is a report, not an invariant, and stage 8 replaces the mechanism it
//! measures. Kept because it is the evidence for that stage, and because it compiles with
//! the gate (`clippy --all-targets`) so it cannot rot.

use meld_balance::Balance;
use meld_world::Arena;
use std::collections::HashSet;

/// ⚠️ **A CELL IS LAND IF ITS WEDGE INTERSECTS THE CORRIDOR, not if its MIDPOINT does.**
/// `span` returns bearings over the FULL fan while `sectors` counts against the TAPERED one,
/// so at depth a single-sector ring spans +/-2.618 rad while the land is 0.04 wide. Testing
/// the midpoint made ring 11 (5 sectors, each 1.05 rad) read as entirely ocean and split the
/// end of the world into its own component — an artifact, not a gap.
fn land_overlap(g: &meld_proto::regions::Grid, c: meld_proto::regions::Cell, arc_half: f32) -> bool {
    let sp = g.span(c);
    for k in 0..=4 {
        let r = sp.inner + (sp.outer - sp.inner) * (k as f32 / 4.0);
        let lim = meld_proto::coast::arc_half_at(r, arc_half);
        if sp.bear_lo <= lim && sp.bear_hi >= -lim {
            return true;
        }
    }
    false
}

#[test]
#[ignore = "dev report: run with --ignored"]
fn connectivity_floor() {
    let b = Balance::load_default().unwrap();
    let a = Arena::generate(&b, 1, false);
    let g = a.regions();
    let arc_half = a.radial_half() as f32;
    let rings = (3200.0 / g.ring_step as f64).ceil() as u32 + 1;

    // Land cells only: a cell whose midpoint is outside the tapered fan is ocean.
    let mut cells: HashSet<u32> = HashSet::new();
    for ring in 0..rings {
        for sector in 0..g.sectors(ring) {
            let c = meld_proto::regions::Cell::new(ring, sector);
            if land_overlap(&g, c, arc_half) {
                cells.insert(c.key());
            }
        }
    }
    // Undirected edges between land cells.
    let mut edges: HashSet<(u32, u32)> = HashSet::new();
    let mut degrees: Vec<usize> = Vec::new();
    for &k in &cells {
        let c = meld_proto::regions::Cell::from_key(k);
        let mut deg = 0;
        for n in g.neighbours(c) {
            if !cells.contains(&n.key()) { continue; }
            deg += 1;
            let (lo, hi) = if k <= n.key() { (k, n.key()) } else { (n.key(), k) };
            edges.insert((lo, hi));
        }
        degrees.push(deg);
    }
    degrees.sort_unstable();
    let n = cells.len();
    let e = edges.len();
    let floor = (n.saturating_sub(1)) as f64 / e.max(1) as f64;
    let mean_deg = degrees.iter().sum::<usize>() as f64 / n.max(1) as f64;
    println!("ring_step {:.0}, cell_width {:.0}", g.ring_step, g.cell_width);
    println!("land cells N={n}, edges E={e}, mean degree {mean_deg:.2}");
    println!("  degree min {} median {} max {}", degrees[0], degrees[n/2], degrees[n-1]);
    println!("CONNECTIVITY FLOOR (N-1)/E = {:.3}", floor);
    println!("  porosity table: ashfall 0.30, mire 0.35, tundra 0.38, forest 0.55, field 0.92");
    for (name, p) in [("ashfall",0.30),("mire",0.35),("tundra",0.38),("forest",0.55),("field",0.92)] {
        println!("    {name:<8} {p:.2} -> {}", if p < floor { "BELOW THE FLOOR: cannot connect" } else { "achievable by a tree" });
    }
    println!("The taper's last rings, in detail:");
    for ring in 9..15u32 {
        let n = g.sectors(ring);
        let mut land_n = 0;
        let mut detail = String::new();
        for sector in 0..n {
            let c = meld_proto::regions::Cell::new(ring, sector);
            let sp = g.span(c);
            let mid_r = 0.5 * (sp.inner + sp.outer);
            let mid_b = 0.5 * (sp.bear_lo + sp.bear_hi);
            let lim = meld_proto::coast::arc_half_at(mid_r, arc_half);
            let is_land = land_overlap(&g, c, arc_half);
            if is_land { land_n += 1; }
            if n <= 3 {
                detail.push_str(&format!(
                    " [s{sector}: mid {mid_b:+.3} vs lim {lim:.3}, span {:+.3}..{:+.3} {}]",
                    sp.bear_lo, sp.bear_hi, if is_land { "LAND" } else { "ocean" }));
            }
        }
        println!("  ring {ring:>2} d{:>6.0}: {n} sectors, {land_n} land{detail}",
            ring as f32 * g.ring_step);
    }
    println!("What the INDEPENDENT roll actually gives (uniform porosity, seed 1):");
    let keys: Vec<u32> = cells.iter().copied().collect();
    let idx: std::collections::HashMap<u32, usize> =
        keys.iter().enumerate().map(|(i, k)| (*k, i)).collect();
    for p in [0.30f32, 0.35, 0.38, 0.55, 0.70, 0.92, 1.0] {
        let open: Vec<(u32, u32)> = edges
            .iter()
            .copied()
            .filter(|(lo, hi)| {
                meld_proto::regions::pass_open(
                    g.seed,
                    meld_proto::regions::Cell::from_key(*lo),
                    meld_proto::regions::Cell::from_key(*hi),
                    p,
                )
            })
            .collect();
        let mut parent: Vec<usize> = (0..keys.len()).collect();
        for (lo, hi) in &open {
            let (mut x, mut y) = (idx[lo], idx[hi]);
            while parent[x] != x { x = parent[x]; }
            while parent[y] != y { y = parent[y]; }
            if x != y { parent[x] = y; }
        }
        let mut sizes: std::collections::HashMap<usize, usize> = Default::default();
        for i in 0..keys.len() {
            let mut r = i;
            while parent[r] != r { r = parent[r]; }
            *sizes.entry(r).or_default() += 1;
        }
        println!(
            "  p={p:.2}: {} of {} open, {} components, largest holds {:.0}% of cells",
            open.len(),
            edges.len(),
            sizes.len(),
            100.0 * sizes.values().copied().max().unwrap_or(0) as f64 / keys.len() as f64
        );
        if p >= 1.0 {
            let biggest = sizes.iter().max_by_key(|(_, v)| **v).map(|(k, _)| *k).unwrap();
            for (i, key) in keys.iter().enumerate() {
                let mut r = i;
                while parent[r] != r { r = parent[r]; }
                if r == biggest { continue; }
                let c = meld_proto::regions::Cell::from_key(*key);
                let sp = g.span(c);
                let land_nb = g.neighbours(c).iter().filter(|n| cells.contains(&n.key())).count();
                println!(
                    "    STRANDED ring {} sector {} d{:.0} bearing {:.2}: {} land nb of {} total",
                    c.ring, c.sector, 0.5 * (sp.inner + sp.outer),
                    0.5 * (sp.bear_lo + sp.bear_hi), land_nb, g.neighbours(c).len()
                );
            }
        }
    }
}
