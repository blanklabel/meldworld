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
