//! **CONTINENTS** (WG-7): the fan is no longer one landmass.
//!
//! It was, and the line that made it one was a single early return in `coast::is_ocean` —
//! inside the arc, land, always. So the world had a *coastline* and no continents: every
//! bearing was solid ground from the hub to the frontier, and the only water a diver could
//! reach was the gap behind Last City, which tells you which way is out and has nothing on
//! its far side to walk toward.
//!
//! A `Strait` fills an annular sector with sea and pierces it with isthmuses; the CONTINENT
//! is the land between two straits. These tests hold the things that must be true by
//! CONSTRUCTION rather than by luck — because the failure mode is not a visual glitch, it is
//! a world with no route to its own portal:
//!
//! 1. straits actually get cut (the generator bails out on several conditions, every one a
//!    silent `return`, so the feature could ship inert and every other test here would still
//!    pass on an empty list),
//! 2. every one of them is crossable, and
//! 3. nothing — path, portal, creature, node, chest or prop — ends up in the water.

use meld_balance::Balance;
use meld_proto::coast;
use meld_world::Arena;

fn balance() -> Balance {
    Balance::load_default().expect("the shipped balance parses")
}

/// A world streamed far enough out to hold several continents.
fn deep_world(seed: u64) -> Arena {
    let b = balance();
    let mut a = Arena::generate(&b, seed, false);
    let mut reach = 0.0;
    while reach < 2400.0 {
        reach += 120.0;
        a.ensure_frontier(&b, reach);
    }
    a
}

/// **The feature is on, and every strait honours its contract.** Without the first half the
/// whole thing could be inert; without the second, "the route exists" is a hope rather than
/// a construction guarantee.
#[test]
fn a_strait_is_cut_and_it_is_always_crossable() {
    let mut worlds_with_straits = 0;
    let mut total = 0;
    for seed in [1u64, 7, 42, 99, 424242, 987654] {
        let a = deep_world(seed);
        let arc = a.radial_half() as f32;
        if !a.straits.is_empty() {
            worlds_with_straits += 1;
        }
        for s in &a.straits {
            total += 1;
            assert!(
                coast::strait_is_crossable(s, arc),
                "seed {seed}: a strait was cut that cannot be crossed — {s:?}. That is not a \
                 hard fight, it is a world with no route to its own portal."
            );
            assert!(
                s[0] - s[1] >= coast::STRAIT_MIN_REACH,
                "seed {seed}: a strait reaches into the on-ramp (inner edge {}), where CANON \
                 §B owns the difficulty and a diver's first minutes should not be a coast",
                s[0] - s[1]
            );
        }
    }
    assert_eq!(
        worlds_with_straits, 6,
        "every seeded world should reach its first strait by d2400"
    );
    assert!(
        total >= 6,
        "only {total} straits across six deep worlds — the world is still one landmass"
    );
}

/// **The world is more than one landmass now.** The direct refutation of the old `is_ocean`
/// early return: sweep the bearings of the fan at many radii and find water *inside* it.
#[test]
fn the_fan_holds_open_water_inside_itself() {
    let a = deep_world(424242);
    let arc = a.radial_half() as f32;
    assert!(!a.straits.is_empty(), "no straits — nothing to test");

    let (mut wet, mut dry) = (0u32, 0u32);
    for ri in 0..600 {
        let r = 200.0 + ri as f32 * 4.0;
        for ti in 0..=240 {
            let th = -arc + (ti as f32 / 240.0) * 2.0 * arc;
            let (x, z) = (r * th.cos(), r * th.sin());
            if coast::is_ocean_with(x, z, arc, &a.straits) {
                wet += 1;
            } else {
                dry += 1;
            }
        }
    }
    assert!(wet > 0, "the fan is still one unbroken landmass — {dry} land samples, no water");
    // And in the other direction: continents, not an archipelago.
    let share = wet as f64 / (wet + dry) as f64;
    assert!(
        share < 0.30,
        "{:.0}% of the fan is water — that is an archipelago, not continents",
        share * 100.0
    );
}

/// **A strait's shores are both inside its own section**, which is what lets `astar_route`
/// route a crossing at all: the section's clear path enters on dry ground and leaves on dry
/// ground, so A* always has two land endpoints to find an isthmus between. A strait
/// straddling a section boundary would put both of a section's path endpoints in the water.
#[test]
fn a_strait_leaves_dry_ground_on_both_of_its_shores() {
    for seed in [1u64, 42, 424242] {
        let a = deep_world(seed);
        for s in &a.straits {
            let (r_c, r_half) = (s[0] as f64, s[1] as f64);
            let area = a
                .areas
                .iter()
                .find(|ar| r_c >= ar.start_x && r_c < ar.end_x)
                .unwrap_or_else(|| panic!("seed {seed}: a strait at r={r_c} sits in no section"));
            assert!(
                r_c - r_half > area.start_x,
                "seed {seed}: a strait's inner shore ({}) is at or before its section's start \
                 ({}) — the section's path would begin in the water",
                r_c - r_half,
                area.start_x
            );
            assert!(
                r_c + r_half < area.end_x,
                "seed {seed}: a strait's outer shore ({}) is at or past its section's end ({})",
                r_c + r_half,
                area.end_x
            );
        }
    }
}

/// **The guaranteed route never swims.** The whole point of putting the strait in `coast` —
/// and of cutting it BEFORE the path is routed — is that `astar_route` land-checks every bent
/// edge and bends to an isthmus by itself. If this fails, the world has a barrier with no
/// pass and the deep portal is unreachable on foot.
#[test]
fn the_clear_path_crosses_at_an_isthmus_and_never_swims() {
    let b = Balance::load_default().unwrap();
    for seed in [1u64, 7, 42, 99, 424242, 987654] {
        let a = deep_world(seed);
        assert!(!a.straits.is_empty(), "seed {seed}: no straits to cross");
        for (i, w) in a.path.iter().enumerate() {
            assert!(
                a.on_land(w.x, w.y),
                "seed {seed}: clear-path waypoint {i} at ({:.1}, {:.1}) is in the water — the \
                 route out of the world is not feasible",
                w.x,
                w.y
            );
        }
        // ⚠️ **AND A DECK HAS TO CARRY THE TRAIL, NOT MERELY CROSS THE SEA NEAR IT.**
        //
        // `bridge_span` collapses a run of drowned trail into ONE straight capsule from its
        // first vertex to its last, so where the trail crosses at an angle — or bows around a
        // range, a peak, a lake — the deck cuts the chord and misses the middle of its own
        // run. Seed 424242: one section found 89 drowned vertices, laid ONE span, and left 26
        // of them in open water at r=2283.
        //
        // Nothing downstream could catch it: A* had already drawn that trail and was never
        // asked again, and `backbone_feasible` samples the route before the deeper section
        // that cuts the strait exists. So the bridging pass has to check its own work.
        //
        // Asserted with the ROUTE'S OWN CLEARANCE rather than the point test above, because
        // that is what `astar_route` guarantees and therefore what a deck has to preserve — a
        // trail that clears water by a hair is one the party wades. Folded in here rather than
        // standing alone: it needs exactly these worlds, and generating six more to d3000 is
        // minutes of a gate that already runs close to its CI timeout.
        let pad = (b.worldgen.path_clear_radius + b.worldgen.player_radius) as f32;
        let shore = a.shore();
        let mut worst = (f32::MIN, 0.0f64, 0.0f64);
        for w in a.path.iter() {
            let d = shore.water(w.x as f32, w.y as f32);
            if d > worst.0 {
                worst = (d, w.x, w.y);
            }
        }
        assert!(
            worst.0 < -pad,
            "seed {seed}: the trail comes within {:.2} of water at ({:.0}, {:.0}) — the route \
             keeps {pad:.2} from it, so this is a wade the pathfinder never agreed to",
            -worst.0,
            worst.1,
            worst.2
        );
        // The segments between them too: a chord between two dry waypoints can cut a strait.
        for pair in a.path.windows(2) {
            let (p, q) = (pair[0], pair[1]);
            let steps = (p.distance_to(&q).ceil() as i32).max(2);
            for s in 0..=steps {
                let t = s as f64 / steps as f64;
                let (x, y) = (p.x + (q.x - p.x) * t, p.y + (q.y - p.y) * t);
                assert!(
                    a.on_land(x, y),
                    "seed {seed}: the trail cuts across water between ({:.1}, {:.1}) and \
                     ({:.1}, {:.1}) — a dry waypoint either side is not enough",
                    p.x,
                    p.y,
                    q.x,
                    q.y
                );
            }
        }
        assert!(a.on_land(a.portal.x, a.portal.y), "seed {seed}: the deep portal is at sea");
    }
}

/// **Nothing is placed in the sea.** Placement never asked the shoreline — harmless while the
/// ocean sat outside the fan and nothing was ever placed out there, and a visible bug the
/// moment a section holds open water in the middle of it. A creature at sea is a fight nobody
/// can reach; a node or chest at sea is the unreachable reward `nudge_to_walkable` exists to
/// prevent; a tree at sea reads as a bug in the water.
#[test]
fn nothing_stands_in_the_water() {
    for seed in [1u64, 42, 424242] {
        let a = deep_world(seed);
        for m in &a.monsters {
            assert!(
                a.on_land(m.position.x, m.position.y),
                "seed {seed}: creature {} stands in the sea at ({:.1}, {:.1})",
                m.entity_id,
                m.position.x,
                m.position.y
            );
            assert!(
                a.on_land(m.home.x, m.home.y),
                "seed {seed}: creature {}'s wander anchor is at sea, so it spends the run \
                 walking at the water",
                m.entity_id
            );
        }
        for r in &a.resources {
            assert!(
                a.on_land(r.position.x, r.position.y),
                "seed {seed}: harvest node {} is at sea",
                r.entity_id
            );
        }
        for c in &a.chests {
            assert!(
                a.on_land(c.position.x, c.position.y),
                "seed {seed}: chest {} is at sea",
                c.entity_id
            );
        }
        for o in &a.obstacles {
            assert!(
                a.on_land(o.position.x, o.position.y),
                "seed {seed}: a `{}` prop stands in the sea at ({:.1}, {:.1})",
                o.kind,
                o.position.x,
                o.position.y
            );
        }
    }
}

/// The tutorial and every flat/corridor world stay exactly as they were: no fan, no sea, no
/// continents. The guided first dive is a fixed walk east, and a strait across it would be a
/// new player's first experience of the world being impassable.
#[test]
fn the_tutorial_has_no_continents() {
    let b = balance();
    let a = Arena::generate(&b, 424242, true);
    assert!(a.straits.is_empty(), "the tutorial dive must not be cut by a strait");
}

// ---------------------------------------------------------------------------------------
// INLAND WATER — the laws
// ---------------------------------------------------------------------------------------

/// **A river runs downhill, and ends at the sea or in a lake.** Both halves hold by
/// CONSTRUCTION (gradient descent on `terrain::height`, terminating at the sea or pooling in
/// a hollow it cannot climb out of), so this is really a check that the construction is what
/// actually ran — a descent that silently stopped stepping would still produce a chain.
#[test]
fn a_river_runs_downhill_and_ends_at_the_sea_or_in_a_lake() {
    let mut rivers_seen = 0;
    for seed in [1u64, 7, 42, 424242] {
        let a = deep_world(seed);
        let (ox, oz) = a.terrain_offset();
        let h = |n: &[f32; 4]| meld_proto::terrain::height(n[0], n[1], ox, oz);
        // Split the flat node list back into chains.
        let mut chains: Vec<Vec<[f32; 4]>> = Vec::new();
        for n in &a.rivers {
            if n[3] >= 0.5 || chains.is_empty() {
                chains.push(Vec::new());
            }
            chains.last_mut().unwrap().push(*n);
        }
        for chain in &chains {
            rivers_seen += 1;
            // Downhill, node to node, within a tolerance for the finite-difference step.
            for w in chain.windows(2) {
                assert!(
                    h(&w[1]) <= h(&w[0]) + 0.5,
                    "seed {seed}: a river climbs — {:.2} to {:.2}. Water does not do that, \
                     and it cannot happen unless something other than gradient descent \
                     placed these nodes",
                    h(&w[0]),
                    h(&w[1])
                );
            }
        }
        // Every river ends somewhere real: at the sea, or in one of this world's basins.
        for chain in &chains {
            let Some(last) = chain.last() else { continue };
            let at_sea = a.shore().sea(last[0], last[1]) > -30.0;
            let in_basin = a.basins.iter().any(|b| {
                (last[0] - b[0]).hypot(last[1] - b[1]) <= b[2] + 40.0
            });
            // A chain that ends because a FORD follows it is mid-river, not a terminus.
            let mid_river = chain.len() < a.rivers.len();
            assert!(
                at_sea || in_basin || mid_river,
                "seed {seed}: a river ends at ({:.0}, {:.0}) — not at the sea, not in a \
                 lake, and not at a ford. Water has to go somewhere.",
                last[0],
                last[1]
            );
        }
    }
    assert!(rivers_seen > 0, "no rivers in four deep worlds — inland water never generated");
}

/// **Every river is crossable**, because connectedness is what a river IS and a connected
/// impassable line is exactly what disconnects a world. Fords are placed on a cadence, so a
/// river long enough to need one has one.
#[test]
fn a_river_long_enough_to_block_you_has_a_ford() {
    let b = balance();
    let ford_every = b.worldgen.river_ford_every;
    for seed in [1u64, 7, 42, 424242] {
        let a = deep_world(seed);
        if a.rivers.is_empty() {
            continue;
        }
        let chains = a.rivers.iter().filter(|n| n[3] >= 0.5).count();
        let longest = {
            let mut best = 0usize;
            let mut cur = 0usize;
            for n in &a.rivers {
                if n[3] >= 0.5 {
                    cur = 0;
                }
                cur += 1;
                best = best.max(cur);
            }
            best
        };
        assert!(
            longest <= ford_every + 1,
            "seed {seed}: an unbroken river chain of {longest} nodes, with fords every \
             {ford_every} — a stretch that long is a wall"
        );
        assert!(chains >= 1, "seed {seed}: river nodes with no chain start");
    }
}

/// Inland water generates, in QUANTITY and at SIZE — and the floors are what matter here,
/// not the existence checks.
///
/// ⚠️ **A `> 0` bar passed while the feature was gutted.** An earlier attempt bounded every
/// body to its own section's radius band, which made the whole suite green and, measured, cut
/// a world out to d2400 down to 3-5 lakes of mean radius 44 and river chains of 2-3 nodes —
/// a 26-unit "river". Every test still passed, because every test only asked whether water
/// existed. Measured floors are the only thing that catches that class of regression, so
/// these are set from a real census (10-15 basins of mean radius 104-124, and 31-37 river
/// nodes per world) with generous headroom for seed variance.
#[test]
fn a_world_holds_lakes_and_rivers_worth_the_name() {
    let seeds = [1u64, 7, 42, 99, 424242, 987654];
    let (mut basins, mut nodes, mut radius_sum) = (0usize, 0usize, 0.0f32);
    for seed in seeds {
        let a = deep_world(seed);
        basins += a.basins.len();
        nodes += a.rivers.len();
        radius_sum += a.basins.iter().map(|b| b[2]).sum::<f32>();
    }
    assert!(
        basins >= 30,
        "only {basins} standing bodies across {} deep worlds — inland water is generating, \
         but nowhere near enough of it to meet",
        seeds.len()
    );
    assert!(nodes >= 60, "only {nodes} river nodes across {} deep worlds", seeds.len());
    let mean = radius_sum / basins.max(1) as f32;
    assert!(
        mean >= 60.0,
        "mean lake radius is {mean:.0} — that is a pond. Something is clamping the fill, \
         which is exactly the regression this test exists for (see the note above)."
    );
}

/// A lake is water you COLLIDE with and not ground the shader digs — the distinction that
/// keeps `Shore::sea` (which drives ground displacement toward a globally-zero sea level)
/// separate from `Shore::water`. Asserted on a real generated world, not a fixture.
#[test]
fn a_generated_lake_is_not_in_the_sea_field() {
    for seed in [1u64, 42, 424242] {
        let a = deep_world(seed);
        for b in &a.basins {
            let shore = a.shore();
            assert!(shore.inland(b[0], b[1]) > 0.0, "seed {seed}: a basin's centre is dry");
            assert!(!a.on_land(b[0] as f64, b[1] as f64), "seed {seed}: you can stand in a lake");
            assert!(
                shore.sea(b[0], b[1]) < 0.0,
                "seed {seed}: a lake leaked into the SEA field, so the ground shader will \
                 dig it a second time below its own bed"
            );
        }
    }
}

// ---------------------------------------------------------------------------------------
// ONE WATER SYSTEM
// ---------------------------------------------------------------------------------------

/// **No biome scatters water as a PROP any more**, and the payoff is measurable rather than
/// aesthetic: `BlockField::new` sizes its spatial-hash cell from the largest radius in the
/// world, water props ran to radius 10 while every other obstacle caps at 2.8, so water
/// alone forced `cell` to 20 instead of 8 — 2.5x wider cells holding ~6x the props, on the
/// creature tick, for the ENTIRE world and not only the biomes that had water.
///
/// Two mechanisms for one substance is the duplication this repo keeps paying for
/// (`is_water_kind`'s own doc lists three copies of that rule). Water is `coast::Basin` now.
#[test]
fn no_biome_scatters_water_as_a_prop() {
    for seed in [1u64, 7, 42, 424242] {
        let a = deep_world(seed);
        let wet: Vec<&str> = a
            .obstacles
            .iter()
            .map(|o| o.kind.as_str())
            .filter(|k| meld_proto::coast::is_water_kind(k))
            .collect();
        assert!(
            wet.is_empty(),
            "seed {seed}: {} water PROPS still scattered (e.g. `{}`). Water is analytic now — \
             a prop version puts colliders back in `BlockField` and coarsens its cell for \
             every prop in the game.",
            wet.len(),
            wet[0]
        );
        // …and nothing else is big enough to coarsen the grid.
        let widest = a.obstacles.iter().map(|o| o.radius).fold(0.0f64, f64::max);
        assert!(
            widest <= 4.0,
            "seed {seed}: an obstacle of radius {widest:.1} is back — anything past ~4 sets \
             `BlockField`'s cell from `(max_radius * 2).max(8)` and undoes the win"
        );
    }
}

/// **The Mire is still the wettest biome in the game**, which is the thing that had to
/// survive retiring its fill. Its water used to BE its maze (`fill_kind_for_biome` returned
/// `bog_pool`); now the fill is roots and the flooding comes from `biome_water_mult`, so this
/// asserts the swamp is a swamp rather than a drained one.
#[test]
fn the_mire_is_wetter_than_the_desert() {
    let b = balance();
    let area = |a: &Arena| -> f32 {
        a.basins.iter().map(|x| x[2] * x[2] * std::f32::consts::PI).sum()
    };
    let mut mire_total = 0.0f32;
    let mut desert_total = 0.0f32;
    for seed in [1u64, 42, 424242] {
        for (biome, acc) in [("mire", &mut mire_total), ("desert", &mut desert_total)] {
            let mut a = Arena::generate_with(&b, seed, false, Some(biome));
            let mut reach = 0.0;
            while reach < 1200.0 {
                reach += 120.0;
                a.ensure_frontier(&b, reach);
            }
            *acc += area(&a);
        }
    }
    assert!(
        mire_total > desert_total * 2.0,
        "the mire holds {mire_total:.0} sq units of standing water and the desert \
         {desert_total:.0} — a swamp has to be visibly wetter than a desert, and retiring \
         `bog_pool` as the mire's fill is exactly what could have quietly drained it"
    );
}
