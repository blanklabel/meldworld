use meld_balance::Balance;
use meld_world::Arena;

/// **A FRONTIER GROWN OFF THE TICK KEEPS THE WORLD PLAYERS ARE STANDING IN** (`WG-11` stage 9).
///
/// `WorldActor::tick` runs in the same `select!` arm as every other event, so generating a
/// section there stalls battles, movement and messaging alike — measured in release, the worst
/// section is **971 ms against a 100 ms tick**. The work is handed to a blocking thread against
/// a CLONE, and `adopt_grown` takes the grown world whole and puts the live-only state back on
/// top of it.
///
/// ⚠️ That direction is the safety property being tested here. An `Arena` has 27 public fields;
/// folding the generated tail of each into the live world is the shape of bug this crate keeps
/// paying for. Taking the grown world whole means a field missing from the live list REVERTS
/// something a player can see — which is loud — instead of leaving geometry quietly wrong.
#[test]
fn adopting_a_grown_frontier_keeps_the_live_world() {
    let b = Balance::load_default().unwrap();
    let mut live = Arena::generate(&b, 424242, false);
    live.ensure_frontier(&b, 200.0);

    // Something only the live world knows: a player, and a partly-harvested node.
    live.add_avatar("p1".into(), 5.0);
    let (node_id, left) = {
        let n = live.resources.first_mut().expect("a world has nodes");
        n.remaining = 3;
        (n.entity_id.clone(), n.remaining)
    };
    let areas_before = live.areas.len();

    // What the blocking thread does: grow a clone.
    let mut grown = live.clone();
    grown.ensure_frontier(&b, 800.0);
    assert!(grown.areas.len() > areas_before, "the clone should have grown");

    assert!(live.adopt_grown(grown), "an unshifted world should adopt its own frontier");
    assert!(live.areas.len() > areas_before, "the frontier did not arrive");
    assert!(
        live.avatars.iter().any(|a| a.player_id == "p1"),
        "the player was dropped by the merge — live-only state has to survive it"
    );
    let n = live
        .resources
        .iter()
        .find(|n| n.entity_id == node_id)
        .expect("the node still exists");
    assert_eq!(
        n.remaining, left,
        "the node's harvested stock reverted — a merge that loses this hands a player back \
         what they already dug up"
    );
    // And nothing the world placed is standing in water afterwards.
    let drowned = live
        .monsters
        .iter()
        .filter(|m| !live.on_land(m.position.x, m.position.y))
        .count();
    assert_eq!(drowned, 0, "{drowned} creatures stand in water after the merge");
}

/// **AND IT REFUSES WHEN THE WORLD MOVED UNDERNEATH IT.** A Shift repaints biomes and
/// re-scatters props, and a frontier grown before it does not have it. Merging then would
/// quietly undo the Shift for everyone standing in it, so the job is discarded and the caller
/// asks again.
#[test]
fn a_shift_during_generation_discards_the_job() {
    let b = Balance::load_default().unwrap();
    let mut live = Arena::generate(&b, 424242, false);
    live.ensure_frontier(&b, 300.0);
    let mut grown = live.clone();
    grown.ensure_frontier(&b, 700.0);

    // The Shift lands while the thread is still working.
    let roll = meld_world::shift::roll(&b, live.seed, 1);
    live.apply_shift(&b, &roll, 0, 1.min(live.areas.len().saturating_sub(1)));

    assert!(
        !live.adopt_grown(grown),
        "a world that shifted while the frontier was growing must discard the job — adopting \
         it would silently undo the Shift for everyone standing in it"
    );
}
