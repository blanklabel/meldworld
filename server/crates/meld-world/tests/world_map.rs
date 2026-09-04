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

const REACH: f64 = 1500.0;

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
    for seed in [1u64, 42, 424242] {
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
        svg.push_str("</g>\n</svg>\n");

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
