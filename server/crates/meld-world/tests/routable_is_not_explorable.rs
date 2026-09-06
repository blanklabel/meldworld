use meld_balance::Balance;
use meld_world::Arena;

/// **ROUTABLE IS NOT EXPLORABLE** (`WG-11`).
///
/// The guaranteed trail is feasible by construction — a wall segment crossing it is dropped,
/// and dropping it IS a door. That says nothing about whether a player can leave the trail and
/// come back a different way, which is the property `open_sealed_ground` used to repair and
/// which stage 7 deleted along with it, on the argument that a decided maze cannot seal a
/// world. That argument is sound about the CELL GRAPH — a spanning tree is connected — and it
/// is not a proof about walkable GROUND, because ranges, water and the shoreline all cut ground
/// the graph knows nothing about. Nothing has held this since. This does.
///
/// ⚠️ **IT COUNTS DELIBERATE BARRIERS, NEVER SCATTER, AND THAT IS THE WHOLE INSTRUMENT.**
/// At any affordable lattice step the grid cannot resolve the gaps between trunks, so counting
/// trees makes a wood read as solid: measured that way this same flood reported **0.8-1.4%** of
/// three worlds reachable and looked exactly like a catastrophic seal. Ranges, water and the
/// coast are what actually divide a world.
///
/// It DOES count the maze's own deliberate walls — prop walls, furnished pass parts and the
/// minimaze interiors — because those are placed to block. That is what makes this one guard
/// cover the minimaze too: an interior maze that sealed its cell would show up here as ground
/// the hub cannot reach.
#[test]
fn most_of_the_world_can_be_reached_on_foot() {
    let b = Balance::load_default().unwrap();
    for seed in [1u64, 42, 424242] {
        let mut a = Arena::generate(&b, seed, false);
        let mut r = 0.0f64;
        while r < 600.0 { r += 60.0; a.ensure_frontier(&b, r); }
        let pad = a.player_radius_for_tests();
        // Deliberate walls: the maze's prop walls, its furnished pass parts, and the minimaze
        // interiors. Ordinary scatter is excluded — see the note above.
        let walls: Vec<(f64, f64, f64)> = a
            .obstacles
            .iter()
            .filter(|o| {
                let k = &o.entity_id;
                k.starts_with("obs-wall-") || k.starts_with("obs-mini-") || k.starts_with("obs-pass-")
            })
            .map(|o| (o.position.x, o.position.y, o.radius))
            .collect();
        let reach = 600.0f64;
        let step = 4.0f64;
        let n = ((2.0 * reach / step).ceil() as usize) + 1;
        let idx = |cx: usize, cy: usize| cy * n + cx;
        let mut open = vec![false; n * n];
        let mut total = 0usize;
        for cy in 0..n { for cx in 0..n {
            let x = -reach + cx as f64 * step;
            let y = -reach + cy as f64 * step;
            if x.hypot(y) > reach { continue }
            if !a.on_land(x, y) || a.range_blocks_for_tests(x, y, pad) { continue }
            if walls.iter().any(|(px, py, r)| (x - px).hypot(y - py) < r + pad) { continue }
            open[idx(cx, cy)] = true; total += 1;
        }}
        // Flood from the hub.
        let c0 = (reach / step) as usize;
        let mut start = None;
        'find: for dr in 0..40usize { for dy in 0..=dr { for sx in [-1i64,1] { for sy in [-1i64,1] {
            let cx = (c0 as i64 + sx * dr as i64) as usize;
            let cy = (c0 as i64 + sy * dy as i64) as usize;
            if cx < n && cy < n && open[idx(cx, cy)] { start = Some(idx(cx, cy)); break 'find }
        }}}}
        let Some(s0) = start else { panic!("seed {seed}: no open ground near the hub") };
        let mut seen = vec![false; n * n];
        seen[s0] = true;
        let mut stack = vec![s0];
        let mut got = 1usize;
        while let Some(cur) = stack.pop() {
            let (cy, cx) = (cur / n, cur % n);
            for (dx, dy) in [(1i64,0i64),(-1,0),(0,1),(0,-1)] {
                let (nx, ny) = (cx as i64 + dx, cy as i64 + dy);
                if nx < 0 || ny < 0 || nx >= n as i64 || ny >= n as i64 { continue }
                let k = idx(nx as usize, ny as usize);
                if !open[k] || seen[k] { continue }
                seen[k] = true; got += 1; stack.push(k);
            }
        }
        let share = 100.0 * got as f64 / total.max(1) as f64;
        println!("seed {seed}: reached {got} of {total} walkable cells = {share:.1}%");
        assert!(
            share >= 90.0,
            "seed {seed}: only {share:.1}% of the world's walkable ground is reachable from the \
             hub — a world whose interior is islands is the failure this whole arc exists to \
             remove, and the trail being feasible does not cover it"
        );
    }
}
