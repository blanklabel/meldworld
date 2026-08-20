//! The per-tick creature step must stay ~LINEAR in the creature count.
//!
//! The world streams outward without bound, so every creature ever generated stays in the
//! arena: a shallow dive holds ~1,300 and d1269 holds ~10,650. Anything quadratic in that
//! number therefore does not degrade, it *detonates* — and twice now it has. The movement
//! pass was fixed with a spatial grid; the **damage pass twenty lines below it was left
//! scanning every creature for every creature**, which at d1269 was ~113 million pair tests
//! per 100 ms tick, each one a string faction compare. Measured **1.7 seconds a tick in a
//! release build**, against a 100 ms budget, on a single-task game loop — so a deep dive
//! never sent `run.started` at all and the world simply could not be entered.
//!
//! This asserts the RATIO rather than a duration, because a duration bound is either flaky on
//! a loaded machine or too loose to catch anything. 8x the creatures may cost noticeably more
//! than 8x the time, but it must not cost 59x.

#[test]
fn the_creature_step_stays_linear_in_the_creature_count() {
    let b = meld_balance::Balance::load_default().unwrap();
    let mut shallow = meld_world::Arena::generate(&b, 42, false);
    let mut deep = meld_world::Arena::generate(&b, 42, false);
    for _ in 0..4096 {
        if shallow.ensure_frontier(&b, 308.0).is_empty() {
            break;
        }
    }
    for _ in 0..4096 {
        if deep.ensure_frontier(&b, 1269.0).is_empty() {
            break;
        }
    }
    let (ns, nd) = (shallow.monsters.len(), deep.monsters.len());
    assert!(nd > ns * 4, "the deep world is not meaningfully bigger ({ns} vs {nd})");

    // Enough ticks that the timing is stable rather than a single noisy sample.
    let time = |a: &mut meld_world::Arena| {
        let t = std::time::Instant::now();
        for _ in 0..40 {
            a.step_creatures(0.1);
        }
        t.elapsed().as_secs_f64()
    };
    let _ = time(&mut shallow); // warm up, so neither side pays first-touch costs
    let _ = time(&mut deep);
    let ts = time(&mut shallow).max(1e-6);
    let td = time(&mut deep);

    let creature_ratio = nd as f64 / ns as f64;
    let cost_ratio = td / ts;
    assert!(
        cost_ratio < creature_ratio * 2.5,
        "the creature step went superlinear: {nd}/{ns} = {creature_ratio:.1}x the creatures \
         cost {cost_ratio:.1}x the time ({ts:.3}s -> {td:.3}s). A quadratic pass is back."
    );
}
