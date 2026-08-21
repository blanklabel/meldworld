use meld_balance::Balance;
use meld_world::{Arena, Obstacle};
use meld_proto::common::Position;
use std::time::Instant;

fn world(b: &Balance) -> Arena {
    let mut a = Arena::generate(b, 424242, false);
    let mut reach = 0.0f64;
    while reach < 1300.0 { reach += 40.0; a.ensure_frontier(b, reach); }
    a
}
fn tick_ms(a: &mut Arena) -> f64 {
    for _ in 0..3 { a.step_creatures(0.1); }
    let mut best = f64::INFINITY;
    for _ in 0..5 {
        let t = Instant::now();
        for _ in 0..10 { a.step_creatures(0.1); }
        best = best.min(t.elapsed().as_secs_f64() * 1000.0 / 10.0);
    }
    best
}

#[test]
fn scratch_a_lake_as_edges_beats_a_lake_as_a_disc() {
    let b = Balance::load_default().unwrap();
    let mut base = world(&b);
    println!("world: {} props, {} creatures", base.obstacles.len(), base.monsters.len());
    println!("  baseline                                  {:.2} ms", tick_ms(&mut base));

    // (a) One filled disc — a lake as a single big-radius collider.
    let mut disc = world(&b);
    disc.obstacles.push(Obstacle {
        entity_id: "lake-disc".into(), kind: "pond".into(),
        position: Position::new(9_000.0, 9_000.0), radius: 150.0,
    });
    println!("  ONE r=150 disc                            {:.2} ms  (+{} props)",
        tick_ms(&mut disc), 1);

    // (b) The SAME lake as its boundary only: small colliders around the rim,
    //     spaced so the ring is continuous (gap < 2r_collider).
    let mut edge = world(&b);
    let (r_lake, r_col) = (150.0f64, 2.8f64);
    let step = r_col * 1.6;                       // overlap, so nothing walks through
    let n = ((2.0 * std::f64::consts::PI * r_lake) / step).ceil() as usize;
    for i in 0..n {
        let th = i as f64 / n as f64 * std::f64::consts::TAU;
        edge.obstacles.push(Obstacle {
            entity_id: format!("lake-edge-{i}"), kind: "pond".into(),
            position: Position::new(9_000.0 + r_lake * th.cos(), 9_000.0 + r_lake * th.sin()),
            radius: r_col,
        });
    }
    println!("  SAME lake as {n} rim colliders (r={r_col})   {:.2} ms  (+{n} props)",
        tick_ms(&mut edge));

    // (c) Ten such lakes as edges — is it still cheap at scale?
    let mut many = world(&b);
    let mut total = 0usize;
    for k in 0..10 {
        let (cx, cy) = (9_000.0 + (k as f64) * 500.0, 9_000.0);
        for i in 0..n {
            let th = i as f64 / n as f64 * std::f64::consts::TAU;
            many.obstacles.push(Obstacle {
                entity_id: format!("lake{k}-edge-{i}"), kind: "pond".into(),
                position: Position::new(cx + r_lake * th.cos(), cy + r_lake * th.sin()),
                radius: r_col,
            });
            total += 1;
        }
    }
    println!("  TEN lakes as edges                        {:.2} ms  (+{total} props)",
        tick_ms(&mut many));
}
