//! **THE MAZE IS A DECIDED TOPOLOGY, NOT A ROLL PER BOUNDARY** (`WG-11` stage 8).
//!
//! [`meld_proto::regions::pass_open`] rolls each cell boundary independently at the biome's
//! `porosity`, and its own doc is explicit that *"connectivity is NOT this function's job"* —
//! feasibility was delegated to the routes carved through the world, which cut their own gaps.
//! Measured (`tests/the_maze_graph.rs`), that leaves a player who steps OFF the route in a
//! shattered world: at ashfall's 0.30 porosity the teardrop is **37 islands** with the largest
//! holding 28% of its cells, and both ashfall and mire sit BELOW the `(N-1)/E` floor, so they
//! cannot be connected by any arrangement at all.
//!
//! So the topology is **decided** instead: a spanning tree over the land cells, which is
//! connected by construction, then braided so it is not a bare tree. Everything else —
//! which material a wall is made of, whether an open biome erases it, what a Shift does to it
//! — is EXPRESSION on top of a skeleton that never changes.
//!
//! ## Why this is still "derived from the seed"
//!
//! ⚠️ Per-boundary independence was not a preference, it was forced: a cell had to be
//! answerable from a position alone, in constant time, at any radius, because the world
//! streamed outward without bound (§W5 keeps the baseline a pure function of the seed). The
//! **teardrop retires that constraint**. `coast::arc_half_at` closes the world to a ~200-unit
//! corridor by `TAPER_END`, so the interesting cell set is finite — measured at **172 land
//! cells / 475 boundaries** — and past the taper each ring holds one or two cells in a linear
//! chain, which needs no maze because a corridor is not a choice. A global topology over a few
//! hundred cells is a few hundred bytes, and it is *recomputed* from the seed rather than
//! stored, so §W5 survives untouched. "Derived from the seed" and "computed globally" only
//! conflict when the world is infinite.
//!
//! ## Houston's algorithm
//!
//! Robin Houston's hybrid: run **Aldous-Broder** while the grid is mostly unvisited (a random
//! walk that carves whenever it enters a new cell — fast at first, glacial on the tail), then
//! switch to **Wilson's** (loop-erased random walks from unvisited cells until they strike the
//! tree — glacial at first, fast once a third of the grid is standing). Each covers the
//! other's bad half.
//!
//! ⚠️ At this size the performance argument is **moot** — a few hundred nodes finishes in
//! microseconds whichever way you do it — and that is worth saying out loud so nobody defends
//! the complexity on speed grounds later. It is chosen for what both halves share: they sample
//! a **uniform** spanning tree, so the maze carries no directional bias. A recursive
//! backtracker would give long snaking corridors, a binary tree a visible diagonal grain;
//! uniform means the maze's character comes from the WORLD (which cells, which biome, which
//! material) rather than from an artifact of the carving order.
//!
//! ## Braiding is not optional
//!
//! A spanning tree is a **perfect** maze: exactly one path between any two cells. That is the
//! opposite of what this world wants — *"deliberately not a tree, with terminal branches"* —
//! and of the *"multiple explorable paths"* the design asks for directly. So a share of the
//! **dead ends** get a second way out. Dead ends rather than walls is the unit on purpose: a
//! dead end is where `WG-11` hangs its reward, so braiding one **spends a dungeon site**, and
//! the trade is legible instead of an abstract wall budget.

use std::collections::{BTreeSet, HashMap, HashSet};

use meld_proto::regions::{Cell, Grid};

/// A boundary between two cells, as an unordered pair of [`Cell::key`]s.
///
/// ⚠️ **ORDERLESS, for the same reason `pass_open` is**: a boundary is one thing seen from two
/// sides, so a cell's own neighbour list has to agree with its neighbour's. Sorted rather than
/// combined commutatively, because `a ^ b` and `a + b` both collide far too readily on a
/// packed `(ring, sector)` key.
pub type Edge = (u32, u32);

/// Sort a pair into an [`Edge`].
pub fn edge(a: Cell, b: Cell) -> Edge {
    let (x, y) = (a.key(), b.key());
    if x <= y { (x, y) } else { (y, x) }
}

/// Does this cell hold any land?
///
/// ⚠️ **A CELL IS LAND IF ITS WEDGE INTERSECTS THE CORRIDOR, NEVER IF ITS MIDPOINT DOES.**
/// [`Grid::span`] returns bearings over the FULL fan while [`Grid::sectors`] counts against the
/// TAPERED one, so at depth a five-sector ring's wedges are ~1.05 rad each against a ~0.05 rad
/// corridor and not one midpoint lands on ground. Asking the midpoint reports the end of the
/// world as a severed island — it did, and the tell was that the graph came apart even with
/// every single boundary open.
///
/// A free function so the generator, the tests and any report ask the same question.
pub fn cell_holds_land(grid: &Grid, arc_half: f32, c: Cell) -> bool {
    if arc_half <= 0.0 {
        return true;
    }
    let sp = grid.span(c);
    (0..=4).any(|k| {
        let r = sp.inner + (sp.outer - sp.inner) * (k as f32 / 4.0);
        let lim = meld_proto::coast::arc_half_at(r, arc_half);
        sp.bear_lo <= lim && sp.bear_hi >= -lim
    })
}

/// The decided topology of one world: which cell boundaries are ways through.
#[derive(Debug, Clone, Default)]
pub struct Maze {
    /// Every boundary the maze leaves open. A boundary absent from this set is a WALL — but
    /// whether a wall is actually built there, and of what, is expression and lives elsewhere.
    open: BTreeSet<Edge>,
    /// The cells the topology was built over, so a caller can tell "outside the maze" (past
    /// the taper, where the world is a corridor) from "inside it and walled".
    cells: HashSet<u32>,
    /// Cells left with exactly one way out after braiding — where the reward goes.
    dead_ends: Vec<u32>,
}

impl Maze {
    /// Is this boundary a way through?
    ///
    /// ⚠️ **A boundary outside the maze's own cell set is OPEN.** Past `TAPER_END` the world is
    /// a ~200-unit corridor holding one or two cells a ring, and a corridor is not a choice —
    /// walling it would only be a gate with no alternative behind it. Defaulting to open also
    /// means a cell the topology never saw can never become a sealed pocket, which is the
    /// failure mode this whole stage exists to remove.
    pub fn is_open(&self, a: Cell, b: Cell) -> bool {
        let e = edge(a, b);
        if !self.cells.contains(&e.0) || !self.cells.contains(&e.1) {
            return true;
        }
        self.open.contains(&e)
    }

    /// The cells with exactly one way out — `WG-11`'s content sites.
    pub fn dead_ends(&self) -> &[u32] {
        &self.dead_ends
    }

    /// How many boundaries the maze holds open, and how many it saw. For reports and tests.
    pub fn counts(&self) -> (usize, usize, usize) {
        (self.cells.len(), self.open.len(), self.dead_ends.len())
    }
}

/// A small deterministic PRNG, so a maze is a pure function of the world seed.
///
/// Its own stream rather than `meld_world::Rng`: the maze is computed once, up front, over the
/// whole teardrop, while that generator is threaded through per-section placement. Sharing a
/// stream would make every seeded world re-roll the moment this algorithm changed how many
/// numbers it draws — which is the `RETUNING CREATURE DENSITY MOVES EVERY SEEDED WORLD` trap,
/// and it would fire on every tweak to the carve order.
struct Mrng(u64);

impl Mrng {
    fn new(seed: u64) -> Self {
        Mrng(seed ^ 0x4D41_5A45_5F57_4731)
    }
    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    fn below(&mut self, n: usize) -> usize {
        if n == 0 { 0 } else { (self.next_u64() % n as u64) as usize }
    }
}

/// Build the maze over every land cell out to `horizon`.
///
/// `is_land` decides membership. ⚠️ **It must ask whether a cell's WEDGE holds land, never
/// whether its midpoint does**: `Grid::span` returns bearings over the full fan while
/// `Grid::sectors` counts against the TAPERED one, so at depth a five-sector ring's wedges are
/// each ~1.05 rad against a ~0.05 rad corridor and not one midpoint lands on ground. Getting
/// that wrong reports the end of the world as a severed island — it did, and the tell was that
/// the graph came apart even with every boundary open.
pub fn build(
    grid: &Grid,
    seed: u64,
    horizon: f64,
    braid: f64,
    is_land: &dyn Fn(Cell) -> bool,
) -> Maze {
    // ── The graph. Land cells out to the horizon, and the boundaries between them.
    let mut cells: HashSet<u32> = HashSet::new();
    let rings = (horizon / grid.ring_step.max(1.0) as f64).ceil() as u32 + 1;
    for ring in 0..rings {
        for sector in 0..grid.sectors(ring) {
            let c = Cell::new(ring, sector);
            if is_land(c) {
                cells.insert(c.key());
            }
        }
    }
    let mut adj: HashMap<u32, Vec<u32>> = HashMap::new();
    for &k in &cells {
        let c = Cell::from_key(k);
        let ns: Vec<u32> = grid
            .neighbours(c)
            .into_iter()
            .filter(|n| cells.contains(&n.key()))
            .map(|n| n.key())
            .collect();
        adj.insert(k, ns);
    }
    // A stable order, because a HashSet's iteration order is not the seed's business.
    let mut order: Vec<u32> = cells.iter().copied().collect();
    order.sort_unstable();
    if order.is_empty() {
        return Maze::default();
    }
    for ns in adj.values_mut() {
        ns.sort_unstable();
    }

    let mut rng = Mrng::new(seed);
    let mut open: BTreeSet<Edge> = BTreeSet::new();
    let mut in_tree: HashSet<u32> = HashSet::new();

    // ── PHASE 1: Aldous-Broder. A random walk that carves whenever it steps somewhere new.
    // Fast while most of the grid is unvisited, which is exactly the half it is asked for.
    let switch_at = (order.len() as f64 / 3.0).ceil() as usize;
    let mut at = order[rng.below(order.len())];
    in_tree.insert(at);
    // ⚠️ Bounded. A random walk has no guaranteed finish, and this runs inside world
    // generation — the phase switch is the design, but a pathological walk must not be able
    // to hang a world. Wilson's below finishes whatever this leaves.
    let walk_cap = order.len().saturating_mul(200).max(1000);
    let mut steps = 0;
    while in_tree.len() < switch_at && steps < walk_cap {
        steps += 1;
        let ns = &adj[&at];
        if ns.is_empty() {
            at = order[rng.below(order.len())];
            continue;
        }
        let next = ns[rng.below(ns.len())];
        if in_tree.insert(next) {
            open.insert(if at <= next { (at, next) } else { (next, at) });
        }
        at = next;
    }

    // ── PHASE 2: Wilson's. Loop-erased random walks from unvisited cells until they strike
    // the tree. Glacial on a blank grid and fast now that a third of it is standing.
    for &start in &order {
        if in_tree.contains(&start) {
            continue;
        }
        // Walk, recording the direction taken OUT of each cell. Revisiting a cell overwrites
        // its exit, which is what erases the loop — the path is read back from the map, so a
        // loop simply stops being on it.
        let mut exit: HashMap<u32, u32> = HashMap::new();
        let mut cur = start;
        let mut hops = 0;
        while !in_tree.contains(&cur) && hops < walk_cap {
            hops += 1;
            let ns = &adj[&cur];
            if ns.is_empty() {
                break;
            }
            let next = ns[rng.below(ns.len())];
            exit.insert(cur, next);
            cur = next;
        }
        if !in_tree.contains(&cur) {
            // The walk could not reach the tree (an isolated cell, or the cap). Attach it
            // directly if it has any neighbour at all, so no cell is ever left sealed.
            if let Some(&n) = adj[&start].first() {
                in_tree.insert(start);
                open.insert(if start <= n { (start, n) } else { (n, start) });
            } else {
                in_tree.insert(start);
            }
            continue;
        }
        // Read the loop-erased path back and carve it.
        let mut w = start;
        while let Some(&next) = exit.get(&w) {
            in_tree.insert(w);
            open.insert(if w <= next { (w, next) } else { (next, w) });
            if in_tree.contains(&next) && next == cur {
                break;
            }
            w = next;
        }
        in_tree.insert(cur);
    }

    // ── BRAIDING. A spanning tree is a perfect maze; this world wants loops.
    let degree = |open: &BTreeSet<Edge>, k: u32| -> usize {
        adj[&k]
            .iter()
            .filter(|n| open.contains(&if k <= **n { (k, **n) } else { (**n, k) }))
            .count()
    };
    let mut ends: Vec<u32> = order.iter().copied().filter(|k| degree(&open, *k) == 1).collect();
    ends.sort_unstable();
    let want = (ends.len() as f64 * braid.clamp(0.0, 1.0)).round() as usize;
    for _ in 0..want {
        if ends.is_empty() {
            break;
        }
        let pick = rng.below(ends.len());
        let k = ends.swap_remove(pick);
        // Give it a SECOND way out: any neighbour it is not already joined to.
        let mut options: Vec<u32> = adj[&k]
            .iter()
            .copied()
            .filter(|n| !open.contains(&if k <= *n { (k, *n) } else { (*n, k) }))
            .collect();
        if options.is_empty() {
            continue;
        }
        options.sort_unstable();
        let n = options[rng.below(options.len())];
        open.insert(if k <= n { (k, n) } else { (n, k) });
    }

    let dead_ends: Vec<u32> = order.iter().copied().filter(|k| degree(&open, *k) == 1).collect();
    Maze { open, cells, dead_ends }
}

/// **THE SHARED BOUNDARY OF TWO CELLS**, as `((r0, bearing0), (r1, bearing1))` in polar world
/// terms — a radial segment when the two share a bearing, an arc when they share a radius.
/// `None` when they are not adjacent.
///
/// ⚠️ **A CELL'S OUTWARD ARC IS NOT ONE BOUNDARY.** Sector counts ride the radius, so a cell
/// has **two or more** outward neighbours — measured over one world, the fan-out is 2 for 585
/// of 663 cells, 3 for 46 and 4 for one. Walling the whole arc on ONE neighbour's verdict was
/// wrong for **341 of 632** multi-neighbour arcs: the arc came out walled where the maze had
/// left an edge open (sealing a cell the topology meant to reach) or open where it had walled
/// one (leaking a wall the maze wanted). Splitting per neighbour is what makes the wall that
/// gets BUILT the edge the maze actually decided.
///
/// ⚠️ And the neighbour cannot be guessed from the sector index or found by sampling a point.
/// `Cell::new(ring + 1, sector)` is a different cell entirely once the counts differ, and
/// `cell_at` on the shared arc lands back in the INNER ring for 203 of 663 cells, because
/// `Grid::warp_at` wobbles the ring boundary with bearing — so the sector it returns is
/// indexed against the wrong ring. Ask [`Grid::neighbours`], which is the one place adjacency
/// is defined.
pub fn shared_boundary(grid: &Grid, a: Cell, b: Cell) -> Option<((f64, f64), (f64, f64))> {
    let step = grid.ring_step.max(1.0) as f64;
    let (sa, sb) = (grid.span(a), grid.span(b));
    if a.ring == b.ring {
        // A SPOKE: the radial line where two sectors of one ring meet, over the ring's band.
        let bearing = if a.sector + 1 == b.sector {
            sa.bear_hi as f64
        } else if b.sector + 1 == a.sector {
            sb.bear_hi as f64
        } else {
            return None;
        };
        let lo = a.ring as f64 * step;
        Some(((lo, bearing), (lo + step, bearing)))
    } else {
        // An ARC: at the radius the two rings meet, over the BEARING OVERLAP of their spans —
        // the sub-arc these two cells actually share, not the whole of either's frontage.
        let (inner, outer) = if a.ring + 1 == b.ring {
            (a, b)
        } else if b.ring + 1 == a.ring {
            (b, a)
        } else {
            return None;
        };
        let (si, so) = (grid.span(inner), grid.span(outer));
        let lo = si.bear_lo.max(so.bear_lo) as f64;
        let hi = si.bear_hi.min(so.bear_hi) as f64;
        if hi <= lo {
            return None;
        }
        let r = outer.ring as f64 * step;
        Some(((r, lo), (r, hi)))
    }
}

/// **A CELL'S RELIEF MASS** — where its ground stands higher, and how far that reaches.
///
/// `WG-11` stage 9's primitive. Returns `None` for a cell the maze walls on no side: nothing
/// to express, so it stays open ground.
///
/// ⚠️ **THE MASS SITS IN THE CELL'S BODY, NEVER ON A BOUNDARY.** Stage 8 laid a wall as a
/// capsule down the shared edge, which is why the survey map reads as straight mountain lines
/// — a boundary is a statement about connectivity, not a place to put things. So the mass is
/// positioned inside the cell and only its REACH is the maze's business: walled means two
/// cells' masses meet, open means they stay apart, and the gap between two masses IS the pass.
///
/// The centre is the cell's own centroid pulled toward the boundaries it walls, so a cell
/// walled on one side grows a spur and a cell walled on three grows a lobed mass — and the
/// union of neighbouring masses wanders, because it is a union of blobs rather than a line.
/// Jittered from `(seed, cell)` so it is deterministic and does not sit on the centroid.
pub fn cell_mass(grid: &Grid, m: &Maze, c: Cell, seed: u64) -> Option<(f64, f64, f64)> {
    let walled: Vec<Cell> = grid
        .neighbours(c)
        .into_iter()
        .filter(|n| !m.is_open(c, *n))
        .collect();
    if walled.is_empty() {
        return None;
    }
    let (cx, cy) = grid.centroid(c);
    let (cx, cy) = (cx as f64, cy as f64);
    // Pull toward every walled neighbour. The SUM rather than the nearest, so three walls
    // give a lobed mass leaning into all three instead of a blob hugging one.
    let (mut px, mut py) = (0.0f64, 0.0f64);
    for n in &walled {
        let (nx, ny) = grid.centroid(*n);
        let (dx, dy) = (nx as f64 - cx, ny as f64 - cy);
        let d = dx.hypot(dy).max(1e-6);
        px += dx / d;
        py += dy / d;
    }
    let pull = px.hypot(py).max(1e-6);
    // Deterministic jitter, so no mass sits exactly on its centroid and a run of them does
    // not read as a row of beads.
    let mut rng = Mrng::new(seed ^ (c.key() as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15));
    let step = grid.ring_step.max(1.0) as f64;
    let lean = step * (0.18 + 0.12 * (rng.below(1000) as f64 / 1000.0));
    let jx = step * 0.10 * (rng.below(2001) as f64 / 1000.0 - 1.0);
    let jy = step * 0.10 * (rng.below(2001) as f64 / 1000.0 - 1.0);
    let (mx, my) = (cx + px / pull * lean + jx, cy + py / pull * lean + jy);
    // ⚠️ **CORRECT FOR THE WARP, because `centroid` is NOMINAL and `cell_at` is not.**
    // `Grid::cell_at` computes the ring from `r + warp_at(bearing)`, while `centroid` and
    // `span` are a cell's nominal extent — and the warp (~40 units) is comparable to half a
    // ring (62.5), so **a cell's own centroid is not reliably inside it** as `cell_at` sees it.
    // Measured: the mass for ring 3 sector 4 on seed 1 landed in ring 2 sector 3, and pulling
    // back toward the centroid could not fix it, because the centroid was outside too.
    //
    // The warp is a pure radius offset at a given bearing, so it inverts exactly: subtract it.
    // Then verify with the function that decides, pulling toward a warp-corrected centroid as
    // the floor.
    let warped = |x: f64, y: f64| -> (f64, f64) {
        let b = y.atan2(x);
        let r = x.hypot(y) - grid.warp_at(b as f32) as f64;
        (r * b.cos(), r * b.sin())
    };
    let (tx, ty) = warped(cx, cy);
    let (mut mx, mut my) = warped(mx, my);
    for _ in 0..8 {
        if grid.cell_at(mx as f32, my as f32).key() == c.key() {
            break;
        }
        mx = 0.5 * (mx + tx);
        my = 0.5 * (my + ty);
    }
    let (mx, my) = (mx, my);

    // Reach: far enough to MEET the nearest walled neighbour's mass, which is half the way to
    // its centre. Two masses across a walled boundary therefore touch by construction rather
    // than by a radius anyone tuned.
    let reach = walled
        .iter()
        .map(|n| {
            let (nx, ny) = grid.centroid(*n);
            0.5 * (mx - nx as f64).hypot(my - ny as f64)
        })
        .fold(f64::MAX, f64::min);
    Some((mx, my, reach))
}
