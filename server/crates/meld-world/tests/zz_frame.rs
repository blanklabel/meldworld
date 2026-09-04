use meld_balance::Balance;
use meld_world::Arena;

#[test]
fn are_wall_props_where_their_boundary_is() {
    let b = Balance::load_default().unwrap();
    let mut a = Arena::generate(&b, 424242, false);
    let mut r = 0.0;
    while r < 700.0 { r += 40.0; a.ensure_frontier(&b, r); }
    let g = a.regions();
    let arc_half = a.radial_half() as f32;
    // For each wall prop, how far is the nearest WALLED cell boundary? If prop walls are
    // double-bent, they will be nowhere near one.
    let mut dists: Vec<f64> = Vec::new();
    let walls: Vec<_> = a.obstacles.iter()
        .filter(|o| o.entity_id.starts_with("obs-wall-")).take(400).collect();
    for o in &walls {
        let mut best = f64::MAX;
        for ring in 0..(700.0 / g.ring_step as f64).ceil() as u32 {
            for sector in 0..g.sectors(ring) {
                let c = meld_proto::regions::Cell::new(ring, sector);
                if !meld_world::maze::cell_holds_land(&g, arc_half, c) { continue }
                for other in g.neighbours(c) {
                    if other.key() <= c.key() { continue }
                    if a.maze.is_open(c, other) { continue }
                    if let Some(((r0,b0),(r1,b1))) = meld_world::maze::shared_boundary(&g, c, other) {
                        for k in 0..=6 {
                            let t = k as f64 / 6.0;
                            let (rr, bb) = (r0 + (r1-r0)*t, b0 + (b1-b0)*t);
                            let d = (o.position.x - rr*bb.cos()).hypot(o.position.y - rr*bb.sin());
                            if d < best { best = d }
                        }
                    }
                }
            }
        }
        dists.push(best);
    }
    dists.sort_by(f64::total_cmp);
    let n = dists.len();
    println!("{n} wall props sampled | nearest walled boundary: median {:.1}u, p90 {:.1}u, max {:.1}u",
        dists[n/2], dists[n*9/10], dists[n-1]);
    println!("(a few units = correct frame; hundreds = double-bent)");
}
