//! **A 10,000-FOOT VIEW OF A GENERATED WORLD**, as an SVG you can open and argue about.
//!
//! `#[ignore]`d on purpose: it is a dev instrument rather than an invariant, so it compiles
//! with the gate (`clippy --all-targets`) and cannot rot, but costs nothing on every run.
//!
//! ```sh
//! cargo test -p meld-world --test world_map -- --ignored --nocapture
//! # → /tmp/meld-map-<seed>.svg
//! ```
//!
//! Why not a screenshot: the client's snapshot interest cull is 128 units, so the game can
//! only ever show you a neighbourhood. This draws straight from the generator, so it shows
//! the things the renderer *cannot* — which cell boundaries `regions::pass_open` closed, what
//! material each closed one was walled with, and where the guaranteed route actually runs.
//!
//! Layers are `<g id=…>` so a viewer can toggle them.

use meld_balance::Balance;
use meld_world::Arena;

/// ⚠️ **FAR ENOUGH TO SEE THE TEARDROP.** This was 1500 — and `coast::TAPER_START` is 1200
/// with `TAPER_END` at 3200, so the map stopped 300 units after the taper BEGAN and showed
/// only the fat end of the fan. Reported as "this doesn't look like a teardrop", and it did
/// not, because the shape lives entirely outside what the picture covered.
const REACH: f64 = 3400.0;

fn biome_colour(b: &str) -> &'static str {
    match b {
        "field" => "#7d9b58",
        "forest" => "#3f6b3a",
        "desert" => "#c9b070",
        "ashfall" => "#5a4a4a",
        "tundra" => "#b9c6cc",
        "mire" => "#4a5f4a",
        "amber_wood" => "#a8763f",
        "seized_engine" => "#6a6a72",
        "nestiphian_cradle" => "#6b4a5f",
        "hearth_plains" => "#a09055",
        "seraphic_oubliette" => "#8a7fa8",
        _ => "#666666",
    }
}

#[test]
#[ignore = "dev instrument: writes an SVG map; run with --ignored"]
fn dump_world_map() {
    let b = Balance::load_default().unwrap();
    for seed in [424242u64] {
        let mut a = Arena::generate(&b, seed, false);
        for _ in 0..40 {
            a.ensure_frontier(&b, REACH);
        }
        let g = a.regions();
        let mut gate = [0.0f32; meld_proto::regions::BIOMES.len()];
        for (i, v) in meld_world::biome_gate_slice(&b).iter().enumerate() {
            if i < gate.len() {
                gate[i] = *v;
            }
        }
        let rep = a.repaints().clone();

        let pad = 40.0;
        let span = REACH + pad;
        let size = 1400.0;
        let sx = |x: f64| (x + span) / (2.0 * span) * size;
        let sy = |z: f64| (z + span) / (2.0 * span) * size;

        let mut svg = String::new();
        svg.push_str(&format!(
            "<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 {size} {size}' \
             width='{size}' height='{size}'>\n<rect width='100%' height='100%' fill='#10131a'/>\n"
        ));

        // ── CELLS, filled by biome. A biome is a property of a CELL, so this is the honest
        // unit for a survey: ~150 polygons instead of half a million samples.
        svg.push_str("<g id='cells'>\n");
        let mut cells = 0;
        for ring in 0..(REACH / g.ring_step as f64).ceil() as u32 + 1 {
            for sector in 0..g.sectors(ring) {
                let c = meld_proto::regions::Cell::new(ring, sector);
                let sp = g.span(c);
                if sp.inner as f64 > REACH {
                    continue;
                }
                let bio = meld_proto::regions::BIOMES[g.biome_of(c, &gate, &rep)];
                let mut d = String::new();
                let steps = 10;
                for k in 0..=steps {
                    let t = k as f32 / steps as f32;
                    let th = sp.bear_lo + (sp.bear_hi - sp.bear_lo) * t;
                    let (x, z) = (sp.inner * th.cos(), sp.inner * th.sin());
                    d.push_str(&format!(
                        "{}{:.1},{:.1}",
                        if k == 0 { "M" } else { "L" },
                        sx(x as f64),
                        sy(z as f64)
                    ));
                    d.push(' ');
                }
                for k in (0..=steps).rev() {
                    let t = k as f32 / steps as f32;
                    let th = sp.bear_lo + (sp.bear_hi - sp.bear_lo) * t;
                    let (x, z) = (sp.outer * th.cos(), sp.outer * th.sin());
                    d.push_str(&format!("L{:.1},{:.1} ", sx(x as f64), sy(z as f64)));
                }
                svg.push_str(&format!(
                    "<path d='{d}Z' fill='{}' fill-opacity='0.5' stroke='#0b0d12' \
                     stroke-width='0.5'/>\n",
                    biome_colour(bio)
                ));
                cells += 1;
            }
        }
        svg.push_str("</g>\n");

        // ── THE MAZE: which cell boundaries the decided topology WALLED, and where the
        // dead ends are. This is the layer the whole of `WG-11` stage 8 is about — the cells
        // layer shows what biome a region is, and this shows whether you can walk out of it.
        //
        // Drawn from the same `maze::shared_boundary` the wall placement uses, so the picture
        // cannot disagree with the world about where a wall stands.
        {
            let arc_half = a.radial_half() as f32;
            svg.push_str("<g id='maze'>\n");
            let mut walled = 0usize;
            let mut opens = 0usize;
            // A boundary the maze walls with NOTHING standing on it.
            let mut bare = 0usize;
            let rings = (REACH / g.ring_step as f64).ceil() as u32 + 1;
            for ring in 0..rings {
                for sector in 0..g.sectors(ring) {
                    let c = meld_proto::regions::Cell::new(ring, sector);
                    if !meld_world::maze::cell_holds_land(&g, arc_half, c) {
                        continue;
                    }
                    for other in g.neighbours(c) {
                        if other.key() <= c.key()
                            || !meld_world::maze::cell_holds_land(&g, arc_half, other)
                        {
                            continue;
                        }
                        let Some(((r0, b0), (r1, b1))) =
                            meld_world::maze::shared_boundary(&g, c, other)
                        else {
                            continue;
                        };
                        if r0 > REACH {
                            continue;
                        }
                        let open = a.maze.is_open(c, other);
                        // ⚠️ **IS THE WALL ACTUALLY THERE?** The maze says which boundaries
                        // should be walls; whether the GROUND says so is a separate fact, and
                        // the map had no way to show the difference. That gap is the whole
                        // subject of stage 9 — a boundary the maze walls and the ground
                        // ignores is a gate with nothing in it — so measure it here and draw
                        // the two differently.
                        let mut expressed = false;
                        if !open {
                            let sh = a.shore();
                            for k in 0..=6 {
                                let t = k as f64 / 6.0;
                                let (r, bb) = (r0 + (r1 - r0) * t, b0 + (b1 - b0) * t);
                                let (px, pz) = (r * bb.cos(), r * bb.sin());
                                let at = meld_proto::common::Position::new(px, pz);
                                let ranged = a.ridges.iter().any(|rg| {
                                    let hw = rg[4] as f64;
                                    if hw <= 0.0 { return false }
                                    let (ax, az) = (rg[0] as f64, rg[1] as f64);
                                    let (bx, bz) = (rg[2] as f64, rg[3] as f64);
                                    let (dx, dz) = (bx - ax, bz - az);
                                    let l2 = dx * dx + dz * dz;
                                    let tt = if l2 > 1e-6 {
                                        (((px - ax) * dx + (pz - az) * dz) / l2).clamp(0.0, 1.0)
                                    } else { 0.0 };
                                    (px - (ax + dx * tt)).hypot(pz - (az + dz * tt)) < hw + 4.0
                                });
                                let propped = a.obstacles.iter().any(|o| {
                                    (o.entity_id.starts_with("obs-wall-")
                                        || o.entity_id.starts_with("obs-pass-"))
                                        && o.position.distance_to(&at) < 8.0
                                });
                                let wet = sh.water(px as f32, pz as f32) > -2.0;
                                if ranged || propped || wet {
                                    expressed = true;
                                    break;
                                }
                            }
                        }
                        if open {
                            opens += 1;
                        } else if expressed {
                            walled += 1;
                        } else {
                            bare += 1;
                        }
                        // An arc is drawn as a polyline so it curves; a spoke is a line.
                        let mut d = String::new();
                        let steps = if (r0 - r1).abs() < 1e-6 { 8 } else { 1 };
                        for k in 0..=steps {
                            let t = k as f64 / steps as f64;
                            let (r, bb) = (r0 + (r1 - r0) * t, b0 + (b1 - b0) * t);
                            d.push_str(&format!(
                                "{}{:.1},{:.1} ",
                                if k == 0 { "M" } else { "L" },
                                sx(r * bb.cos()),
                                sy(r * bb.sin())
                            ));
                        }
                        // A WALL is solid and hot; a PASS is a faint dash. The eye should read
                        // the walls as the structure and the passes as the gaps between them.
                        if open {
                            svg.push_str(&format!(
                                "<path d='{d}' fill='none' stroke='#4de0a0' stroke-width='0.8' \
                                 stroke-opacity='0.30' stroke-dasharray='2 3'/>\n"
                            ));
                        } else if !expressed {
                            svg.push_str(&format!(
                                "<path d='{d}' fill='none' stroke='#8a4a52' stroke-width='1.2' \
                                 stroke-opacity='0.55' stroke-dasharray='1 4'/>\n"
                            ));
                        } else {
                            svg.push_str(&format!(
                                "<path d='{d}' fill='none' stroke='#ff5c4d' stroke-width='2.2' \
                                 stroke-opacity='0.85'/>\n"
                            ));
                        }
                    }
                }
            }
            // Dead ends: where WG-11 hangs its reward.
            for &k in a.maze.dead_ends() {
                let c = meld_proto::regions::Cell::from_key(k);
                let sp = g.span(c);
                let (r, bb) = (
                    0.5 * (sp.inner + sp.outer) as f64,
                    0.5 * (sp.bear_lo + sp.bear_hi) as f64,
                );
                if r > REACH {
                    continue;
                }
                svg.push_str(&format!(
                    "<circle cx='{:.1}' cy='{:.1}' r='3.5' fill='none' stroke='#ffd166' \
                     stroke-width='1.6' stroke-opacity='0.9'/>\n",
                    sx(r * bb.cos()),
                    sy(r * bb.sin())
                ));
            }
            svg.push_str("</g>\n");
            println!(
                "  maze: {walled} walled AND built, {bare} walled but BARE ({:.0}% of walls \
                 have nothing standing on them), {opens} passes, {} dead ends in view",
                100.0 * bare as f64 / (walled + bare).max(1) as f64,
                a.maze.dead_ends().len()
            );
        }

        // ── SEA: the actual signed shoreline field, sampled and run-length encoded.
        //
        // ⚠️ This layer exists because the map had EIGHT layers and not one of them drew a
        // coastline, a strait, a bridge or the city — so everything west of the hub, which is
        // ocean with a single span across it to Last City, was simply absent from a survey
        // whose whole job is showing the shape of the world. Reported as the map "forgetting
        // the land behind the last city"; the same class of omission as the scatter layer
        // that once hid 97% of the props.
        //
        // Sampled rather than reconstructed from `straits`/`lobes`/`bridges` on purpose: a
        // BRIDGE is forced land inside `Shore::sea`, so sampling the field draws the deck
        // for free and cannot disagree with what the server collides against. Drawn AFTER
        // the cells so a strait cuts visibly through the biome fill it crosses.
        {
            let sh = a.shore();
            let step = 6.0_f64;
            let n = ((span * 2.0) / step).ceil() as i64;
            let px = (step / (2.0 * span) * size).max(0.6);
            svg.push_str("<g id='sea'>\n");
            let mut runs = 0;
            for j in 0..=n {
                let z = -span + j as f64 * step;
                let mut run: Option<f64> = None;
                for i in 0..=n + 1 {
                    let x = -span + i as f64 * step;
                    let wet = i <= n && sh.sea(x as f32, z as f32) > 0.0;
                    match (wet, run) {
                        (true, None) => run = Some(x),
                        (false, Some(x0)) => {
                            svg.push_str(&format!(
                                "<rect x='{:.1}' y='{:.1}' width='{:.1}' height='{:.1}' \
                                 fill='#1d4f7a'/>\n",
                                sx(x0),
                                sy(z),
                                (sx(x) - sx(x0)).max(px),
                                px
                            ));
                            runs += 1;
                            run = None;
                        }
                        _ => {}
                    }
                }
            }
            svg.push_str("</g>\n");
            println!("  sea: {runs} runs");
        }

        // ── WATER: the ocean's rim, the straits, and every inland body.
        svg.push_str("<g id='water'>\n");
        for bs in &a.basins {
            svg.push_str(&format!(
                "<circle cx='{:.1}' cy='{:.1}' r='{:.1}' fill='#2f6ea8' fill-opacity='0.75'/>\n",
                sx(bs[0] as f64),
                sy(bs[1] as f64),
                bs[2] as f64 / (2.0 * span) * size
            ));
        }
        for w in a.rivers.windows(2) {
            if w[1][3] >= 0.5 {
                continue; // a new chain starts here — the gap before it is a FORD
            }
            svg.push_str(&format!(
                "<line x1='{:.1}' y1='{:.1}' x2='{:.1}' y2='{:.1}' stroke='#3f8ec9' \
                 stroke-width='2' stroke-opacity='0.9'/>\n",
                sx(w[0][0] as f64),
                sy(w[0][1] as f64),
                sx(w[1][0] as f64),
                sy(w[1][1] as f64)
            ));
        }
        svg.push_str("</g>\n");

        // ── SCATTER: every other obstacle in the world.
        //
        // ⚠️ **THE FIRST CUT OF THIS MAP OMITTED THIS LAYER ENTIRELY**, drawing only the walls
        // and the pass throats — about 1,000 props out of 35,000. Read from above, the world
        // looked all but empty, and the survey's first conclusion off the back of it was "we
        // barely have any tree props". The instrument was wrong, not the world. A survey that
        // leaves out the bulk of what it surveys is worse than no survey at all.
        //
        // Emitted as ONE path of degenerate dashes rather than 35,000 `<circle>` elements:
        // same information, a fifth of the bytes, and one DOM node instead of a browser-
        // wrecking 35,000. `stroke-linecap:round` is what turns each zero-length segment into
        // a dot.
        svg.push_str("<g id='scatter'><path fill='none' stroke='#9db98a' stroke-width='1.4' \
                      stroke-linecap='round' stroke-opacity='0.5' d='");
        for o in a.obstacles.iter().filter(|o| {
            !o.entity_id.starts_with("obs-wall-") && !o.entity_id.starts_with("obs-pass-")
        }) {
            svg.push_str(&format!("M{:.1} {:.1}h.01", sx(o.position.x), sy(o.position.y)));
        }
        svg.push_str("'/></g>\n");

        // ── RANGES: the walls made of mountain.
        svg.push_str("<g id='ranges'>\n");
        for r in &a.ridges {
            svg.push_str(&format!(
                "<line x1='{:.1}' y1='{:.1}' x2='{:.1}' y2='{:.1}' stroke='#e8dcc8' \
                 stroke-width='{:.1}' stroke-opacity='0.95' stroke-linecap='round'/>\n",
                sx(r[0] as f64),
                sy(r[1] as f64),
                sx(r[2] as f64),
                sy(r[3] as f64),
                (r[4] as f64 * 2.0 / (2.0 * span) * size).max(1.5)
            ));
        }
        svg.push_str("</g>\n");

        // ── PROP WALLS: the walls made of the biome's own trees. Drawn as their own layer
        // because "is a closed boundary actually closed" is the whole question.
        svg.push_str("<g id='prop-walls'>\n");
        for o in a.obstacles.iter().filter(|o| o.entity_id.starts_with("obs-wall-")) {
            svg.push_str(&format!(
                "<circle cx='{:.1}' cy='{:.1}' r='1.6' fill='#ffcf6b'/>\n",
                sx(o.position.x),
                sy(o.position.y)
            ));
        }
        svg.push_str("</g>\n");

        // ── PASS PARTS: what stands inside a pass (the micro maze).
        svg.push_str("<g id='pass-parts'>\n");
        for o in a.obstacles.iter().filter(|o| o.entity_id.starts_with("obs-pass-")) {
            svg.push_str(&format!(
                "<circle cx='{:.1}' cy='{:.1}' r='2.2' fill='#ff7b4a'/>\n",
                sx(o.position.x),
                sy(o.position.y)
            ));
        }
        svg.push_str("</g>\n");

        // ── THE GUARANTEED ROUTE.
        svg.push_str("<g id='route'>\n<polyline fill='none' stroke='#ff4fa3' stroke-width='2' points='");
        for p in &a.path {
            svg.push_str(&format!("{:.1},{:.1} ", sx(p.x), sy(p.y)));
        }
        svg.push_str("'/>\n</g>\n");

        // ── PEAKS.
        svg.push_str("<g id='peaks'>\n");
        for k in &a.peaks {
            svg.push_str(&format!(
                "<circle cx='{:.1}' cy='{:.1}' r='{:.1}' fill='none' stroke='#d8c8a0' \
                 stroke-width='1' stroke-opacity='0.8'/>\n",
                sx(k[0] as f64),
                sy(k[1] as f64),
                k[2] as f64 / (2.0 * span) * size
            ));
        }
        svg.push_str("</g>\n");

        // ── LEGEND. A survey nobody can read is a picture rather than an instrument — and
        // every layer here is a toggleable `<g id=…>`, so the key names the layer too.
        {
            svg.push_str("<g id='legend'>\n");
            let (lx, ly, lw, row) = (14.0f64, 14.0f64, 268.0f64, 15.0f64);
            let rows: &[(&str, &str, &str)] = &[
                ("head", "", "THE MAZE  (id=maze)"),
                ("line-thick", "#ff5c4d", "walled AND built - no way through"),
                ("line-bare", "#8a4a52", "walled but BARE - nothing standing on it"),
                ("line-dash", "#4de0a0", "pass - the maze's way through"),
                ("ring", "#ffd166", "dead end - where the reward goes"),
                ("head", "", "GROUND"),
                ("capsule", "#e8dcc8", "mountain range - blocks by slope"),
                ("dot", "#d8c8a0", "peak - climbable, crowned (id=peaks)"),
                ("dot", "#9db98a", "scatter prop (id=scatter)"),
                ("dot", "#ffcf6b", "prop wall - boundary walled with trees"),
                ("dot", "#ff7b4a", "pass part - micro maze inside a mouth"),
                ("head", "", "WATER"),
                ("swatch", "#1d4f7a", "ocean, straits, bays (id=sea)"),
                ("disc", "#2f6ea8", "lake / basin - fills a contour"),
                ("line", "#3f8ec9", "river / water wall - gaps are FORDS"),
                ("head", "", "ROUTE"),
                ("line", "#ff4fa3", "guaranteed trail plus web (id=route)"),
            ];
            let lh = row * (rows.len() as f64) + 30.0;
            svg.push_str(&format!(
                "<rect x='{lx}' y='{ly}' width='{lw}' height='{lh:.0}' rx='5' fill='#0b0d12' \
                 fill-opacity='0.85' stroke='#3a4152'/>\n"
            ));
            svg.push_str(&format!(
                "<text x='{:.0}' y='{:.0}' fill='#e8edf6' font-family='monospace' \
                 font-size='11' font-weight='bold'>seed {seed} - d0..{:.0} - {} cells</text>\n",
                lx + 10.0,
                ly + 18.0,
                REACH,
                cells
            ));
            let mut y = ly + 36.0;
            for (kind, colour, label) in rows {
                let (sx0, tx) = (lx + 12.0, lx + 44.0);
                let cy = y - 3.0;
                match *kind {
                    "head" => {
                        svg.push_str(&format!(
                            "<text x='{:.0}' y='{y:.0}' fill='#8f9bb3' font-family='monospace' \
                             font-size='9' letter-spacing='1'>{label}</text>\n",
                            lx + 10.0
                        ));
                        y += row;
                        continue;
                    }
                    "line-thick" => svg.push_str(&format!(
                        "<path d='M{sx0:.0},{cy:.0} L{:.0},{cy:.0}' stroke='{colour}' \
                         stroke-width='2.2'/>\n",
                        sx0 + 22.0
                    )),
                    "line-bare" => svg.push_str(&format!(
                        "<path d='M{sx0:.0},{cy:.0} L{:.0},{cy:.0}' stroke='{colour}' \
                         stroke-width='1.2' stroke-dasharray='1 4'/>\n",
                        sx0 + 22.0
                    )),
                    "line-dash" => svg.push_str(&format!(
                        "<path d='M{sx0:.0},{cy:.0} L{:.0},{cy:.0}' stroke='{colour}' \
                         stroke-width='1' stroke-dasharray='2 3'/>\n",
                        sx0 + 22.0
                    )),
                    "line" => svg.push_str(&format!(
                        "<path d='M{sx0:.0},{cy:.0} L{:.0},{cy:.0}' stroke='{colour}' \
                         stroke-width='1.6'/>\n",
                        sx0 + 22.0
                    )),
                    "capsule" => svg.push_str(&format!(
                        "<path d='M{sx0:.0},{cy:.0} L{:.0},{cy:.0}' stroke='{colour}' \
                         stroke-width='7' stroke-linecap='round'/>\n",
                        sx0 + 22.0
                    )),
                    "ring" => svg.push_str(&format!(
                        "<circle cx='{:.0}' cy='{cy:.0}' r='3.5' fill='none' stroke='{colour}' \
                         stroke-width='1.6'/>\n",
                        sx0 + 11.0
                    )),
                    "disc" => svg.push_str(&format!(
                        "<circle cx='{:.0}' cy='{cy:.0}' r='5' fill='{colour}' \
                         fill-opacity='0.75'/>\n",
                        sx0 + 11.0
                    )),
                    "swatch" => svg.push_str(&format!(
                        "<rect x='{sx0:.0}' y='{:.0}' width='22' height='8' fill='{colour}'/>\n",
                        cy - 4.0
                    )),
                    _ => svg.push_str(&format!(
                        "<circle cx='{:.0}' cy='{cy:.0}' r='2.4' fill='{colour}'/>\n",
                        sx0 + 11.0
                    )),
                }
                svg.push_str(&format!(
                    "<text x='{tx:.0}' y='{y:.0}' fill='#cdd6e5' font-family='monospace' \
                     font-size='10'>{label}</text>\n"
                ));
                y += row;
            }
            svg.push_str("</g>\n");

            // ── BIOME KEY: what a cell's fill means.
            svg.push_str("<g id='biome-key'>\n");
            let biomes = [
                "field", "forest", "amber_wood", "mire", "tundra", "desert", "ashfall",
                "hearth_plains", "seized_engine", "seraphic_oubliette", "nestiphian_cradle",
            ];
            let bh = row * biomes.len() as f64 + 26.0;
            let by = size - bh - 14.0;
            svg.push_str(&format!(
                "<rect x='14' y='{by:.0}' width='200' height='{bh:.0}' rx='5' fill='#0b0d12' \
                 fill-opacity='0.85' stroke='#3a4152'/>\n"
            ));
            svg.push_str(&format!(
                "<text x='24' y='{:.0}' fill='#8f9bb3' font-family='monospace' font-size='9' \
                 letter-spacing='1'>BIOME  (cell fill, id=cells)</text>\n",
                by + 17.0
            ));
            let mut y = by + 34.0;
            for b in biomes {
                svg.push_str(&format!(
                    "<rect x='26' y='{:.0}' width='12' height='9' fill='{}' fill-opacity='0.5' \
                     stroke='#0b0d12' stroke-width='0.5'/>\n",
                    y - 8.0,
                    biome_colour(b)
                ));
                svg.push_str(&format!(
                    "<text x='48' y='{y:.0}' fill='#cdd6e5' font-family='monospace' \
                     font-size='10'>{b}</text>\n"
                ));
                y += row;
            }
            svg.push_str("</g>\n");
        }

        svg.push_str("</svg>\n");

        let path = format!("/tmp/meld-map-{seed}.svg");
        std::fs::write(&path, &svg).unwrap();
        let walls = a.obstacles.iter().filter(|o| o.entity_id.starts_with("obs-wall-")).count();
        let scatter = a.obstacles.len()
            - walls
            - a.obstacles.iter().filter(|o| o.entity_id.starts_with("obs-pass-")).count();
        let parts = a.obstacles.iter().filter(|o| o.entity_id.starts_with("obs-pass-")).count();
        println!(
            "{path}: {cells} cells | scatter {scatter} | ranges {} | wall props {walls} | \
             pass props {parts} | basins {} | river nodes {} | path {} | dungeons {}",
            a.ridges.len(),
            a.basins.len(),
            a.rivers.len(),
            a.path.len(),
            a.areas.iter().filter(|s| s.dungeon).count()
        );
    }
}
