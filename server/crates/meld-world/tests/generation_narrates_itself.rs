//! **THE DESCENT SCREEN'S ONLY SOURCE OF TRUTH** (`WG-12`).
//!
//! Generating a world is one blocking call that takes **3.4-4.2 s in release** for the
//! initial eight-section chain — and several times that when the guaranteed route does not
//! hold and the whole thing is drawn again. Until [`Arena::generate_reporting`] the client
//! was told nothing until it was finished, so the wait could only be covered by an elapsed
//! clock, which reads as a hang rather than as work.
//!
//! Two properties, and the first is the one that makes the second safe to have at all.
use meld_balance::Balance;
use meld_world::{Arena, GenStage};

/// A digest of the world that is sensitive to every draw the generator makes: the counts move
/// if a single extra roll is taken, and the trail's ends move if any of them lands elsewhere.
fn digest(a: &Arena) -> String {
    format!(
        "{}|{}|{}|{}|{:?}|{:?}|{:?}",
        a.areas.len(),
        a.monsters.len(),
        a.obstacles.len(),
        a.path.len(),
        a.portal,
        a.path.first(),
        a.path.last(),
    )
}

/// **THE OBSERVER CANNOT TOUCH THE WORLD IT WATCHES.**
///
/// `meld-world` must stay pure (no clock, no global RNG, no I/O), and a progress callback is
/// the one thing that looks like an exception to that. It is not: `on` is the *caller's* I/O
/// and cannot influence a single draw, so the same seed has to produce the byte-same world
/// whether anyone is listening or not. Break this and world generation stops being replayable
/// from its seed, which is the whole basis of §W5 persistence.
#[test]
fn narrating_generation_does_not_change_the_world() {
    let b = Balance::load_default().unwrap();
    for seed in [424242u64, 1, 7] {
        let quiet = Arena::generate(&b, seed, false);
        let mut heard = 0usize;
        let loud = Arena::generate_reporting(&b, seed, false, None, &mut |_| heard += 1);
        assert_eq!(digest(&quiet), digest(&loud), "seed {seed} changed under narration");
        assert!(heard > 0, "seed {seed} generated a world and said nothing");
    }
}

/// **THE READOUT IS THE REAL PASSES, IN THE ORDER THEY RUN.** The maze is decided before any
/// ground is placed, then every section of the chain exactly once, then the bend, then the
/// route is walked. A screen built from a hand-written list of flavour lines beside the
/// generator would drift from it; this is what keeps the two the same thing.
#[test]
fn every_generation_pass_is_reported_in_the_order_it_runs() {
    let b = Balance::load_default().unwrap();
    let mut steps: Vec<String> = Vec::new();
    let mut sections: Vec<(usize, usize, String)> = Vec::new();
    let _ = Arena::generate_reporting(&b, 424242, false, None, &mut |s| match s {
        GenStage::Maze { .. } => steps.push("maze".into()),
        GenStage::Section { index, total, biome, .. } => {
            steps.push("section".into());
            sections.push((index, total, biome.to_string()));
        }
        GenStage::Bend { .. } => steps.push("bend".into()),
        GenStage::Route { .. } => steps.push("route".into()),
        GenStage::Restart { .. } => steps.push("restart".into()),
    });
    // One attempt's worth of passes, in order — a re-draw repeats the whole shape after a
    // `restart`, so read only up to the first route walk.
    let first: Vec<&str> = steps
        .iter()
        .map(String::as_str)
        .take_while(|s| *s != "restart")
        .collect();
    assert_eq!(first.first(), Some(&"maze"), "the maze is decided first: {first:?}");
    assert_eq!(first.last(), Some(&"route"), "the route walk is last: {first:?}");
    assert_eq!(
        first.iter().rev().nth(1),
        Some(&"bend"),
        "the bend comes between the last section and the route walk: {first:?}"
    );

    // Every section of the chain, once, counted 1..=total, and each naming real ground.
    let total = sections.first().map(|s| s.1).unwrap_or(0);
    assert!(total > 1, "a chain of {total} is not a chain");
    let run: Vec<&(usize, usize, String)> = sections.iter().take(total).collect();
    for (i, s) in run.iter().enumerate() {
        assert_eq!(s.0, i + 1, "sections are counted 1-based and in order: {sections:?}");
        assert_eq!(s.1, total, "the total must not move mid-chain: {sections:?}");
        assert!(
            meld_world::BIOMES.contains(&s.2.as_str()),
            "section {} named a theme the world does not have: {:?}",
            s.0,
            s.2
        );
    }
}
