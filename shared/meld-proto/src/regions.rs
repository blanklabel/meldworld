//! **REGIONS — the world's cell decomposition.**
//!
//! One partition of the WG-4 fan into cells, with adjacency. Everything that needs to
//! ask "which part of the world is this, and what is next to it" asks here: the biome a
//! patch of ground wears, the Shift's blast radius, an anchor's hold, and the routes a
//! maze is carved along. One decomposition rather than four, so a boundary drawn for one
//! of them is the boundary all of them agree on.
//!
//! **Why polar cells rather than a Voronoi.** The world streams outward without bound
//! ([`crate::coast`] has the same constraint), so a global diagram that has to be built
//! before it can be queried is not available: a cell has to be derivable from a position
//! alone, in constant time, at any radius, with no state. A ring index and a sector index
//! are exactly that — and their boundaries are analytic, which is what lets a range or a
//! ravine be DRAWN along one.
//!
//! **Why this is not the ring world it replaces.** A ring is one biome across the whole
//! 300° arc; a cell is one biome across a few hundred units of it, and the number of cells
//! around the arc grows with radius so cell AREA stays roughly constant. At r=1000 that is
//! ~21 independent draws where a ring gave one. Neighbouring cells that draw the same
//! biome merge on sight, so what the player reads is a patchwork of organic blobs rather
//! than a grid — and the ring boundary itself wobbles with bearing ([`Grid::warp_at`]) so
//! it does not read as an arc.
//!
//! **u32 and f32 only, on purpose.** WGSL has no 64-bit integer, and the ground shader has
//! to reach the same answer as the server or it paints a world the server does not collide
//! with. Every hash here is 32-bit wrapping arithmetic that mirrors line for line.

/// The biome label set, and its ORDER — which is a wire contract, not a detail: a cell's
/// biome is an index into this list on both sides of the wire and inside the ground
/// shader. `meld_world::BIOMES` re-exports it rather than declaring its own.
pub const BIOMES: [&str; 11] = [
    "field",
    "forest",
    "desert",
    "ashfall",
    "tundra",
    "mire",
    "amber_wood",
    "seized_engine",
    "nestiphian_cradle",
    "hearth_plains",
    "seraphic_oubliette",
];

/// Biomes that, once open, are the ONLY thing that draws — see [`Grid::biome_of`].
///
/// The world's deepest band is one biome, not a patchwork, because it is the END of the
/// world: the arena the whole walk out is pointed at. Everything else about this
/// decomposition exists to make the map read as varied blobs, and the one place that must
/// NOT be varied is the last one.
pub const EXCLUSIVE: &[&str] = &["seraphic_oubliette"];

/// Index of `name` in [`BIOMES`], or `None`.
pub fn biome_index(name: &str) -> Option<usize> {
    BIOMES.iter().position(|b| *b == name)
}

/// Sector indices are packed into 7 bits alongside the ring, so a cell is one `u32` key
/// that a Shift log or a persisted anchor can store. Past the radius where the arc wants
/// more than this many cells they simply grow wider, which is well beyond the structural
/// end of the world (~d3350).
pub const MAX_SECTORS: u32 = 128;

/// Bit width `MAX_SECTORS` occupies in a [`Cell::key`].
const SECTOR_BITS: u32 = 7;

/// 32-bit integer hash (`lowbias32`). Chosen over the splitmix64 the rest of the
/// generator uses because this one has to run in WGSL too.
pub fn hash32(mut h: u32) -> u32 {
    h ^= h >> 16;
    h = h.wrapping_mul(0x7feb_352d);
    h ^= h >> 15;
    h = h.wrapping_mul(0x846c_a68b);
    h ^ (h >> 16)
}

/// A cell: which radial ring, and which sector around the arc within that ring.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash, PartialOrd, Ord)]
pub struct Cell {
    pub ring: u32,
    pub sector: u32,
}

impl Cell {
    pub fn new(ring: u32, sector: u32) -> Self {
        Cell { ring, sector }
    }

    /// One stable integer per cell, for a Shift log entry or an anchor's pin.
    pub fn key(self) -> u32 {
        (self.ring << SECTOR_BITS) | (self.sector & (MAX_SECTORS - 1))
    }

    pub fn from_key(k: u32) -> Self {
        Cell { ring: k >> SECTOR_BITS, sector: k & (MAX_SECTORS - 1) }
    }
}

/// A cell's extent: a radius band and a bearing wedge. The radii are NOMINAL — the real
/// boundary wobbles by up to `warp` with bearing, which is what stops it reading as an arc.
#[derive(Clone, Copy, Debug)]
pub struct Span {
    pub inner: f32,
    pub outer: f32,
    pub bear_lo: f32,
    pub bear_hi: f32,
}

/// The decomposition's parameters. A bundle for the same reason [`crate::coast::Shore`] is
/// one: every function here needs all of it, and the argument list only ever grows.
#[derive(Clone, Copy, Debug, serde::Serialize, serde::Deserialize, PartialEq)]
pub struct Grid {
    /// Half the fan's arc, in radians (`radial_arc_degrees / 2`).
    pub arc_half: f32,
    /// Radial thickness of one ring.
    pub ring_step: f32,
    /// Target arc width of one cell. Together with `ring_step` this is the cell's size,
    /// and it is what keeps cell area roughly constant as the fan widens outward.
    pub cell_width: f32,
    /// How far a ring boundary wanders with bearing.
    pub warp: f32,
    /// Low 32 bits of the world seed.
    pub seed: u32,
}

impl Grid {
    /// How many cells `ring` is divided into: its own arc length over the target width.
    pub fn sectors(&self, ring: u32) -> u32 {
        let r_mid = (ring as f32 + 0.5) * self.ring_step;
        let arc = 2.0 * self.arc_half * r_mid;
        let n = (arc / self.cell_width.max(1.0)).round();
        (n.max(1.0) as u32).min(MAX_SECTORS)
    }

    /// A ring's angular phase, as a fraction of one sector. Rings draw independently, so
    /// their sector boundaries do not line up into continuous spokes.
    pub fn ring_offset(&self, ring: u32) -> f32 {
        let h = hash32(self.seed ^ ring.wrapping_mul(0x9E37_79B9));
        (h & 0xffff) as f32 / 65536.0
    }

    /// How far the ring boundary at this `bearing` is displaced. Two harmonics rather than
    /// one, so the boundary is a wandering line rather than a lobed flower. Depends on
    /// bearing alone, which is what keeps the partition well-defined: the ring index stays
    /// monotone in radius along every ray.
    pub fn warp_at(&self, bearing: f32) -> f32 {
        let phase = (hash32(self.seed ^ 0x5F35_6495) & 0xffff) as f32 / 65536.0
            * std::f32::consts::TAU;
        self.warp
            * (0.62 * (bearing * 3.0 + phase).sin() + 0.38 * (bearing * 7.0 - phase * 2.0).cos())
    }

    /// Which ring a radius falls in at this bearing.
    pub fn ring_at(&self, r: f32, bearing: f32) -> u32 {
        ((r + self.warp_at(bearing)).max(0.0) / self.ring_step.max(1.0)).floor() as u32
    }

    /// Where a bearing sits across the fan, as `0..=1` from one rim to the other.
    fn fan_t(&self, bearing: f32) -> f32 {
        ((bearing + self.arc_half) / (2.0 * self.arc_half)).clamp(0.0, 1.0)
    }

    /// The cell containing world `(x, z)`. Constant time, no state, exact at any radius.
    ///
    /// The two rim sectors are up to twice as wide as the interior ones, because
    /// `ring_offset` shifts the boundaries and the fan does not wrap — the shift has to
    /// fold into the edges somewhere. It folds into the rims, which are the sea and the
    /// city gap, rather than into the ground the player walks.
    pub fn cell_at(&self, x: f32, z: f32) -> Cell {
        let r = (x * x + z * z).sqrt();
        let bearing = z.atan2(x);
        let ring = self.ring_at(r, bearing);
        let n = self.sectors(ring);
        let idx = (self.fan_t(bearing) * n as f32 + self.ring_offset(ring)).floor();
        let sector = (idx.max(0.0) as u32).min(n - 1);
        Cell { ring, sector }
    }

    /// A sector's extent in fan-`t` terms — the inverse of [`Grid::cell_at`], and the ONE
    /// place that inversion is written. Both [`Grid::span`] and [`Grid::neighbours`] read it,
    /// because an inversion copied twice is an adjacency that disagrees with a boundary.
    ///
    /// The rims absorb `ring_offset`'s shift: `cell_at` clamps, so the first sector really
    /// does begin at the rim and the last really does end at it.
    fn sector_bounds(&self, ring: u32, sector: u32) -> (f32, f32) {
        let n = self.sectors(ring).max(1);
        let off = self.ring_offset(ring);
        let lo = if sector == 0 { 0.0 } else { (sector as f32 - off) / n as f32 };
        let hi = if sector + 1 >= n { 1.0 } else { (sector as f32 + 1.0 - off) / n as f32 };
        (lo.clamp(0.0, 1.0), hi.clamp(0.0, 1.0))
    }

    /// The nominal extent of a cell.
    pub fn span(&self, c: Cell) -> Span {
        let (lo, hi) = self.sector_bounds(c.ring, c.sector);
        Span {
            inner: c.ring as f32 * self.ring_step,
            outer: (c.ring + 1) as f32 * self.ring_step,
            bear_lo: (lo * 2.0 - 1.0) * self.arc_half,
            bear_hi: (hi * 2.0 - 1.0) * self.arc_half,
        }
    }

    /// A cell's centre, in world coordinates. What a Shift's tell and a range's endpoint
    /// anchor to.
    pub fn centroid(&self, c: Cell) -> (f32, f32) {
        let s = self.span(c);
        let r = 0.5 * (s.inner + s.outer);
        let b = 0.5 * (s.bear_lo + s.bear_hi);
        (r * b.cos(), r * b.sin())
    }

    /// Every cell sharing a boundary with `c`: its two neighbours around the arc, plus the
    /// cells inward and outward whose wedges overlap its own. Rings carry different sector
    /// counts, so the radial side is a range rather than a single cell — which is why
    /// adjacency is derived here instead of assumed anywhere else.
    ///
    /// A cell on a rim has no neighbour past it: the fan's edge is open sea, not a wrap.
    pub fn neighbours(&self, c: Cell) -> Vec<Cell> {
        let mut out = Vec::with_capacity(8);
        let n = self.sectors(c.ring).max(1);
        if c.sector > 0 {
            out.push(Cell::new(c.ring, c.sector - 1));
        }
        if c.sector + 1 < n {
            out.push(Cell::new(c.ring, c.sector + 1));
        }
        // This cell's wedge, inset so a shared boundary does not count the cell beyond it.
        let (b0, b1) = self.sector_bounds(c.ring, c.sector);
        let t0 = (b0 + 1e-4).min(b1);
        let t1 = (b1 - 1e-4).max(t0);
        for ring in [c.ring.checked_sub(1), Some(c.ring + 1)].into_iter().flatten() {
            let m = self.sectors(ring).max(1);
            let o = self.ring_offset(ring);
            let lo = ((t0 * m as f32 + o).floor().max(0.0) as u32).min(m - 1);
            let hi = ((t1 * m as f32 + o).floor().max(0.0) as u32).min(m - 1);
            for s in lo..=hi {
                out.push(Cell::new(ring, s));
            }
        }
        out
    }

    /// Distance from `(x, z)` to the nearest cell boundary, in WORLD units, and the cell on
    /// the other side of it. `None` when the nearest boundary is the fan's own rim, which
    /// is not a boundary between cells.
    ///
    /// This is what a cross-fade needs: it is symmetric, so both sides of a boundary blend
    /// to the same colour on it and there is no seam.
    pub fn edge_distance(&self, x: f32, z: f32) -> (f32, Option<Cell>) {
        let r = (x * x + z * z).sqrt();
        let bearing = z.atan2(x);
        let here = self.cell_at(x, z);
        let n = self.sectors(here.ring).max(1);
        let off = self.ring_offset(here.ring);
        let r_eff = (r + self.warp_at(bearing)).max(0.0);

        let mut best = f32::MAX;
        let mut across = None;
        // Inward and outward: already radial distances.
        if here.ring > 0 {
            let d = r_eff - here.ring as f32 * self.ring_step;
            if d < best {
                best = d;
                across = Some(Cell::new(here.ring - 1, self.sector_in(here.ring - 1, bearing)));
            }
        }
        let d = (here.ring + 1) as f32 * self.ring_step - r_eff;
        if d < best {
            best = d;
            across = Some(Cell::new(here.ring + 1, self.sector_in(here.ring + 1, bearing)));
        }
        // Around the arc: an angular gap costs `r` world units per radian, so a wedge that
        // is narrow near the hub is a long walk at the frontier.
        let t = self.fan_t(bearing);
        let to_bearing = 2.0 * self.arc_half / n as f32;
        let frac = t * n as f32 + off;
        for (gap, step) in [(frac - frac.floor(), -1i32), (1.0 - (frac - frac.floor()), 1i32)] {
            let target = here.sector as i32 + step;
            if target < 0 || target >= n as i32 {
                continue;
            }
            let d = gap * to_bearing * r;
            if d < best {
                best = d;
                across = Some(Cell::new(here.ring, target as u32));
            }
        }
        (best.max(0.0), across)
    }

    /// Which sector of `ring` a bearing falls in, without asking for the ring again.
    fn sector_in(&self, ring: u32, bearing: f32) -> u32 {
        let n = self.sectors(ring).max(1);
        let idx = (self.fan_t(bearing) * n as f32 + self.ring_offset(ring)).floor();
        (idx.max(0.0) as u32).min(n - 1)
    }

    /// The biome a cell wears: an index into [`BIOMES`].
    ///
    /// `gate` is `[biome_gate]` in `BIOMES` order — the min distance each theme is held
    /// back to, so the shallow ring stays an on-ramp rather than a coin toss. It is checked
    /// against the cell's INNER radius, so a cell straddling a gate is held until it is
    /// wholly past.
    ///
    /// There is deliberately no "not the same as the neighbour" rule. Two adjacent cells
    /// drawing the same biome is how a region larger than one cell exists at all, and any
    /// such rule would need an ordering — which would make this a traversal instead of a
    /// pure function of position.
    pub fn biome_of(&self, c: Cell, gate: &[f32; BIOMES.len()]) -> usize {
        let inner = c.ring as f32 * self.ring_step;
        // ⚠️ AN EXCLUSIVE BIOME TAKES THE WHOLE BAND. Past its gate the roll is skipped
        // entirely rather than weighted, because "mostly the end of the world, with the
        // occasional meadow" is not an ending. The DEEPEST open one wins, so exclusives
        // can be layered later without this rule changing.
        let mut capstone: Option<(usize, f32)> = None;
        for name in EXCLUSIVE {
            if let Some(i) = biome_index(name) {
                if gate[i] <= inner && capstone.is_none_or(|(_, g)| gate[i] >= g) {
                    capstone = Some((i, gate[i]));
                }
            }
        }
        if let Some((i, _)) = capstone {
            return i;
        }
        let mut open = [0usize; BIOMES.len()];
        let mut count = 0usize;
        for (i, g) in gate.iter().enumerate() {
            if *g <= inner {
                open[count] = i;
                count += 1;
            }
        }
        if count == 0 {
            return 0;
        }
        let h = hash32(self.seed ^ hash32(c.key() ^ 0x2545_F491));
        open[(h % count as u32) as usize]
    }

    /// The biome at a world position.
    pub fn biome_at(&self, x: f32, z: f32, gate: &[f32; BIOMES.len()]) -> usize {
        self.biome_of(self.cell_at(x, z), gate)
    }
}

impl Regions {
    /// The biome index at a world position — the harness override if one is set, otherwise the
    /// decomposition's own answer. ONE resolver, because "forced or derived" answered in two
    /// places is how the server and the client came to disagree about what biome the ground is.
    pub fn biome_at(&self, x: f32, z: f32) -> usize {
        if self.force >= 0 {
            return (self.force as usize).min(BIOMES.len() - 1);
        }
        let mut gate = [0.0f32; BIOMES.len()];
        for (i, g) in gate.iter_mut().enumerate() {
            *g = self.gate.get(i).copied().unwrap_or(0.0);
        }
        self.grid.biome_at(x, z, &gate)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The shipped shape: a 300° fan, cells ~250 units on a side.
    fn grid(seed: u32) -> Grid {
        Grid {
            arc_half: 150.0_f32.to_radians(),
            ring_step: 250.0,
            cell_width: 250.0,
            warp: 88.0,
            seed,
        }
    }

    /// Walk the fan and confirm every point lands in exactly one cell, and that the cell it
    /// lands in is the cell whose extent contains it. A decomposition that is not a
    /// partition is not one thing everything else can agree on.
    #[test]
    fn every_point_lands_in_exactly_one_cell_and_that_cell_contains_it() {
        let g = grid(424242);
        let mut checked = 0;
        for ri in 0..14u32 {
            for si in 0..40u32 {
                let r = (ri as f32 + 0.37) * g.ring_step;
                let b = (si as f32 / 40.0 * 2.0 - 1.0) * g.arc_half * 0.98;
                let (x, z) = (r * b.cos(), r * b.sin());
                let c = g.cell_at(x, z);
                let s = g.span(c);
                // The radial side is nominal ± the warp, so test it through `ring_at`,
                // which is the definition. The angular side is exact.
                assert_eq!(g.ring_at(r, b), c.ring, "ring disagrees with its own definition");
                assert!(
                    b >= s.bear_lo - 1e-3 && b <= s.bear_hi + 1e-3,
                    "bearing {b} outside cell {c:?} wedge [{}, {}]",
                    s.bear_lo,
                    s.bear_hi
                );
                assert!(s.inner < s.outer);
                checked += 1;
            }
        }
        assert!(checked > 500, "only {checked} samples");
    }

    /// A cell key is the identity a Shift log and an anchor's pin are stored under, so it
    /// has to survive the round trip for every cell the world can hold.
    #[test]
    fn a_cell_key_round_trips() {
        for ring in [0u32, 1, 7, 63, 1000, 100_000] {
            for sector in [0u32, 1, 5, MAX_SECTORS - 1] {
                let c = Cell::new(ring, sector);
                assert_eq!(Cell::from_key(c.key()), c, "{c:?} did not survive its key");
            }
        }
    }

    /// **THE POINT OF THE WHOLE MODULE.** A ring world shows ONE biome all the way around
    /// the arc at a given radius — sweep a circle in the world this replaces and you cross
    /// exactly one theme and zero boundaries. Sweep it here and count.
    ///
    /// The floors are depth-aware because the design is: the fan is genuinely narrower near
    /// the hub (8 cells at r=400 against 65 at r=3000) and `[biome_gate]` deliberately holds
    /// half the roster out of the on-ramp, so shallow variety is capped by the gate rather
    /// than by the decomposition. What is asserted at every depth is that a circle is NOT
    /// one biome, which is the claim that was false before.
    #[test]
    fn a_circle_around_the_world_crosses_many_biomes() {
        let gate = [
            0.0f32, 0.0, 400.0, 250.0, 550.0, 0.0, 0.0, 1200.0, 1800.0, 2400.0, 3000.0,
        ];
        for seed in [1u32, 7, 424242, 99, 1_000_003, 5, 31, 777] {
            let g = grid(seed);
            for r in [400.0f32, 900.0, 1800.0, 3000.0] {
                let mut seen = [false; BIOMES.len()];
                let mut runs = 0;
                let mut prev = usize::MAX;
                for k in 0..1440 {
                    let b = (k as f32 / 1440.0 * 2.0 - 1.0) * g.arc_half * 0.995;
                    let bi = g.biome_at(r * b.cos(), r * b.sin(), &gate);
                    seen[bi] = true;
                    if bi != prev {
                        runs += 1;
                        prev = bi;
                    }
                }
                let distinct = seen.iter().filter(|s| **s).count();
                let (min_distinct, min_runs) = if r >= 900.0 { (5, 10) } else { (2, 3) };
                assert!(
                    distinct >= min_distinct,
                    "seed {seed} at r={r}: {distinct} biomes around the whole arc (want \
                     {min_distinct}) — that is a ring world"
                );
                assert!(
                    runs >= min_runs,
                    "seed {seed} at r={r}: {runs} biome changes around the arc (want {min_runs})"
                );
            }
        }
    }

    /// Cells must not become thin slivers or vast provinces as the fan widens, or "a region"
    /// means something different at the hub than at the frontier.
    #[test]
    fn a_cell_stays_about_the_same_size_at_every_depth() {
        let g = grid(424242);
        let mut widths = Vec::new();
        for ring in 1..14u32 {
            let n = g.sectors(ring);
            let r_mid = (ring as f32 + 0.5) * g.ring_step;
            widths.push(2.0 * g.arc_half * r_mid / n as f32);
        }
        let (lo, hi) = widths.iter().fold((f32::MAX, 0.0f32), |(l, h), w| (l.min(*w), h.max(*w)));
        assert!(
            hi / lo < 1.6,
            "cell arc width ranges {lo:.0}..{hi:.0} across the world — {:.2}x",
            hi / lo
        );
        // And it is genuinely growing the sector COUNT rather than the cell.
        assert!(g.sectors(12) > g.sectors(1) * 6, "the arc is not being subdivided outward");
    }

    /// Adjacency has to be mutual, or a Shift that spreads to a neighbour cannot be
    /// replayed from the other side and an anchor's hold has a direction.
    #[test]
    fn adjacency_is_symmetric() {
        let g = grid(424242);
        for ring in 0..10u32 {
            for sector in 0..g.sectors(ring) {
                let c = Cell::new(ring, sector);
                for n in g.neighbours(c) {
                    assert!(
                        g.neighbours(n).contains(&c),
                        "{c:?} claims {n:?} as a neighbour and {n:?} does not agree"
                    );
                }
            }
        }
    }

    /// A cell's neighbours must be the cells actually TOUCHING it. Sampling just outside
    /// each boundary is the only check that does not simply restate the arithmetic.
    #[test]
    fn a_cell_across_a_boundary_is_one_of_that_cells_neighbours() {
        let g = grid(7);
        for ring in 1..9u32 {
            for sector in 0..g.sectors(ring) {
                let c = Cell::new(ring, sector);
                let (cx, cz) = g.centroid(c);
                let (_, across) = g.edge_distance(cx, cz);
                if let Some(a) = across {
                    assert!(
                        g.neighbours(c).contains(&a),
                        "{c:?}'s nearest boundary leads to {a:?}, which it does not call a \
                         neighbour"
                    );
                }
            }
        }
    }

    /// The distance to a boundary must fall to zero AT the boundary and rise on both sides,
    /// or a cross-fade drawn from it seams.
    #[test]
    fn the_edge_distance_vanishes_on_the_boundary() {
        let g = grid(99);
        let c = Cell::new(4, 3);
        let s = g.span(c);
        // Step across the cell's outward boundary along its own mid-bearing.
        let b = 0.5 * (s.bear_lo + s.bear_hi);
        let edge = s.outer - g.warp_at(b);
        for d in [-30.0f32, -8.0, -0.5, 0.5, 8.0, 30.0] {
            let r = edge + d;
            let (dist, _) = g.edge_distance(r * b.cos(), r * b.sin());
            assert!(
                dist <= d.abs() + 1e-2,
                "at {d} from the boundary the nearest edge reads {dist}"
            );
        }
        let (on_edge, _) = g.edge_distance(edge * b.cos(), edge * b.sin());
        assert!(on_edge < 1.0, "on the boundary the distance reads {on_edge}");
    }

    /// The gate is what keeps desert and tundra bruisers out of the on-ramp. A cell may only
    /// wear a theme its own inner radius has earned.
    #[test]
    fn no_cell_wears_a_biome_its_depth_has_not_earned() {
        let gate = [
            0.0f32, 0.0, 400.0, 250.0, 550.0, 0.0, 0.0, 1200.0, 1800.0, 2400.0, 3000.0,
        ];
        for seed in [1u32, 7, 424242, 99] {
            let g = grid(seed);
            for ring in 0..16u32 {
                for sector in 0..g.sectors(ring) {
                    let c = Cell::new(ring, sector);
                    let bi = g.biome_of(c, &gate);
                    let inner = ring as f32 * g.ring_step;
                    assert!(
                        gate[bi] <= inner,
                        "seed {seed}: {c:?} at inner radius {inner} wears {} (gated at {})",
                        BIOMES[bi],
                        gate[bi]
                    );
                }
            }
        }
    }

    /// The whole persistence story (CANON §W5) is that a world is its seed. A cell's biome
    /// has to be a pure function of that seed, and two seeds have to disagree.
    #[test]
    fn the_decomposition_is_the_seed_and_nothing_else() {
        // ⚠️ NOT an all-zero gate. An EXCLUSIVE biome open at radius 0 is open EVERYWHERE,
        // so every cell answers with it and the seed stops mattering at all — which is
        // exactly what this test then reports, as "the seed is inert". Gate the capstone
        // out past this fixture's rings so the roll below is a roll.
        let mut gate = [0.0f32; BIOMES.len()];
        for name in EXCLUSIVE {
            if let Some(i) = biome_index(name) {
                gate[i] = 100_000.0;
            }
        }
        let a = grid(424242);
        let b = grid(424242);
        let c = grid(424243);
        let mut differ = 0;
        for ring in 0..12u32 {
            for sector in 0..a.sectors(ring) {
                let cell = Cell::new(ring, sector);
                assert_eq!(
                    a.biome_of(cell, &gate),
                    b.biome_of(cell, &gate),
                    "the same seed gave {cell:?} two biomes"
                );
                if a.biome_of(cell, &gate) != c.biome_of(cell, &gate) {
                    differ += 1;
                }
            }
        }
        assert!(differ > 40, "two seeds only disagree about {differ} cells — the seed is inert");
    }

    /// ⚠️ **`MELD_BIOME` HAS TO REACH WHOEVER DERIVES A BIOME, NOT JUST WHOEVER SPAWNS ONE.**
    ///
    /// The server honours the override when it picks creature rosters and scatters props; the
    /// client and the ground shader DERIVE a cell's biome from the decomposition. When the
    /// override did not cross the wire the two disagreed, and the result was ashfall lava rocks
    /// strewn across green ground the HUD labelled Mire — the same mismatch the per-section
    /// biome LUT was built to fix, one layer up.
    ///
    /// So the override lives on `Regions` and `Regions::biome_at` is the ONE resolver. This
    /// holds it: forced means forced, at every position, whatever the grid would have said.
    #[test]
    fn a_forced_biome_answers_everywhere_the_grid_would_have() {
        let g = grid(424242);
        // Sized from BIOMES rather than written out, or this test silently stops covering
        // whatever the roster grew (it went from 6 to 11 in one merge).
        let mut gate = vec![0.0f32; BIOMES.len()];
        for (i, g) in [(2usize, 400.0f32), (3, 250.0), (4, 550.0)] {
            gate[i] = g;
        }
        let derived = Regions { grid: g, gate: gate.clone(), blend: 26.0, force: -1 };
        // …and the override must be doing real work overall. Asked in AGGREGATE rather than
        // per biome: with eleven themes one of them will legitimately be what the grid would
        // have picked anyway across a sample, and failing on that coincidence tests nothing.
        let mut total_differed = 0usize;
        for (want, name) in BIOMES.iter().enumerate() {
            let forced =
                Regions { grid: g, gate: gate.clone(), blend: 26.0, force: want as i32 };
            let mut differed = 0;
            for k in 0..400 {
                let r = 300.0 + k as f32 * 6.0;
                let b = (k as f32 / 400.0 * 2.0 - 1.0) * g.arc_half * 0.9;
                let (x, z) = (r * b.cos(), r * b.sin());
                assert_eq!(
                    forced.biome_at(x, z),
                    want,
                    "forced to {name} and answered {} at ({x:.0}, {z:.0})",
                    BIOMES[forced.biome_at(x, z)]
                );
                if derived.biome_at(x, z) != want {
                    differed += 1;
                }
            }
            total_differed += differed;
        }
        assert!(
            total_differed > 1_000,
            "forcing a biome matched what the grid would have said almost everywhere \
             ({total_differed} differences across {} biomes x 400 points) — this test would \
             pass even with the override ignored",
            BIOMES.len()
        );
    }

    /// Absent on an older server, `force` must default to "no override" rather than to biome 0
    /// — a `#[serde(default)]` of zero would silently force every world to `field`.
    #[test]
    fn a_missing_override_is_no_override_and_not_biome_zero() {
        let r: Regions = serde_json::from_str(
            r#"{"grid":{"arc_half":2.6,"ring_step":250.0,"cell_width":250.0,"warp":88.0,"seed":1},
                "gate":[0,0,0,0,0,0],"blend":26.0}"#,
        )
        .expect("Regions without `force` still deserializes");
        assert_eq!(r.force, -1, "a missing override must mean none, not `field`");
    }

    /// Every biome must be reachable somewhere, or a theme is authored content nothing draws.
    #[test]
    fn every_biome_is_somewhere_in_the_world() {
        let gate = [
            0.0f32, 0.0, 400.0, 250.0, 550.0, 0.0, 0.0, 1200.0, 1800.0, 2400.0, 3000.0,
        ];
        let g = grid(424242);
        let mut seen = [false; BIOMES.len()];
        for ring in 0..16u32 {
            for sector in 0..g.sectors(ring) {
                seen[g.biome_of(Cell::new(ring, sector), &gate)] = true;
            }
        }
        for (i, s) in seen.iter().enumerate() {
            assert!(*s, "{} never appears anywhere in the world", BIOMES[i]);
        }
    }
}

/// What a client needs to draw the decomposition: the grid, the gate that decides which
/// themes a cell may draw, and the width the ground fades a boundary over.
///
/// The gate rides here for the same reason the coast constants ride in the ground uniform:
/// the shader computes a cell's biome itself, so a shader that has not been told the gate
/// paints a theme the server does not spawn. It is `[biome_gate]`, which is balance, and the
/// client has no `balance.toml`.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, PartialEq)]
pub struct Regions {
    pub grid: Grid,
    /// `[biome_gate]` in [`BIOMES`] order.
    pub gate: Vec<f32>,
    /// World units the ground cross-fades across a cell boundary.
    pub blend: f32,
    /// **DEV/QA: the biome every cell is forced to** (`MELD_BIOME`), as an index into
    /// [`BIOMES`], or `-1` in normal play.
    ///
    /// ⚠️ It has to cross the wire because the client DERIVES a cell's biome now rather than
    /// being told it. The server honours the override when it spawns creatures and scatters
    /// props; a client that has not been told paints the decomposition's own answer, and the
    /// result was ashfall lava rocks strewn across green ground the HUD called Mire. Same
    /// class of mismatch the per-section biome LUT was originally built to fix, one layer up.
    #[serde(default = "no_force")]
    pub force: i32,
}

fn no_force() -> i32 {
    -1
}

impl Default for Regions {
    /// A decomposition with no ring step is the "there is no world here" state the menus and
    /// the city render against, and the ground shader tests `ring_step <= 0` for exactly it.
    fn default() -> Self {
        Regions {
            grid: Grid { arc_half: 0.0, ring_step: 0.0, cell_width: 250.0, warp: 0.0, seed: 0 },
            gate: vec![0.0; BIOMES.len()],
            blend: 26.0,
            force: -1,
        }
    }
}
