use meld_balance::Balance;
use meld_world::Arena;

/// **PROP DENSITY BLENDS ACROSS A CELL EDGE** (`WG-11` stage 9).
///
/// The ground already cross-fades at a boundary, so the COLOUR wanders across it — while what
/// GROWS there changed at a line. The thinning read one cell's multiplier and its own comment
/// said so: *"no transition width, no neighbour lookup, no direction."* A wood that stops dead
/// against a desert is the square surviving in the thing you walk through rather than in the
/// thing you look at.
///
/// ⚠️ **APPEARANCE BLENDS; DECISIONS STAY DISCRETE.** Only HOW MANY blends, never WHICH: the
/// kind of prop, the creature roster, a node's yield and the maze's own wall material each
/// still resolve to one answer, or you get half-desert half-forest wildlife and a wall made of
/// two materials at once.
///
/// Measured as: how far the density in a narrow band either side of a boundary sits from the
/// density well inside the cells, counting ONLY boundaries where the two cells carry different
/// biomes — the only place a blend can show. With the transition off, the band is just more
/// cell interior. With it on, the band is pulled toward the neighbour, and that gap is the
/// blend doing its work.
#[test]
fn prop_density_blends_across_a_cell_edge() {
    let on = Balance::load_default().unwrap();
    let mut off = on.clone();
    off.worldgen.biome_transition_width = 0.0;
    for seed in [1u64, 424242] {
        let (n_off, f_off) = profile(&off, seed);
        let (n_on, f_on) = profile(&on, seed);
        let gap_off = (n_off - f_off).abs();
        let gap_on = (n_on - f_on).abs();
        println!(
            "seed {seed}: off edge {n_off:.1} interior {f_off:.1} (gap {gap_off:.2}) | \
             on edge {n_on:.1} interior {f_on:.1} (gap {gap_on:.2})"
        );
        assert!(
            gap_on > gap_off,
            "seed {seed}: the band at a differing-biome boundary is no more distinct from the \
             cell interiors with the transition on ({gap_on:.2}) than with it off \
             ({gap_off:.2}) — density is still stepping at a line"
        );
    }
}

/// Props per 1000 u² in the band (<10 units from a boundary) and well inside a cell (>40),
/// counting only ground whose nearest boundary separates two DIFFERENT biomes.
fn profile(b: &Balance, seed: u64) -> (f64, f64) {
    let mut a = Arena::generate(b, seed, false);
    let mut r = 0.0f64;
    while r < 600.0 {
        r += 60.0;
        a.ensure_frontier(b, r);
    }
    let g = a.regions();
    let (mut area, mut cnt) = ([0.0f64; 2], [0.0f64; 2]);
    let step = 4.0f64;
    let bucket = |d: f32| -> Option<usize> {
        if d < 10.0 {
            Some(0)
        } else if d > 40.0 {
            Some(1)
        } else {
            None
        }
    };
    let differing = |x: f64, z: f64| -> Option<usize> {
        let (d, across) = g.edge_distance(x as f32, z as f32);
        let nb = across?;
        let bi = bucket(d)?;
        (a.biome_of_cell(g.cell_at(x as f32, z as f32)) != a.biome_of_cell(nb)).then_some(bi)
    };
    let mut x = -600.0f64;
    while x < 600.0 {
        let mut z = -600.0f64;
        while z < 600.0 {
            if (140.0..560.0).contains(&x.hypot(z)) && a.on_land(x, z) {
                if let Some(bi) = differing(x, z) {
                    area[bi] += step * step;
                }
            }
            z += step;
        }
        x += step;
    }
    for o in &a.obstacles {
        let (ox, oz) = (o.position.x, o.position.y);
        if !(140.0..560.0).contains(&ox.hypot(oz)) {
            continue;
        }
        if let Some(bi) = differing(ox, oz) {
            cnt[bi] += 1.0;
        }
    }
    let per = |i: usize| if area[i] > 0.0 { 1000.0 * cnt[i] / area[i] } else { 0.0 };
    (per(0), per(1))
}
