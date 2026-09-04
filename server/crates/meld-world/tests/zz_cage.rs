use meld_balance::Balance;
use meld_proto::common::Position;
use meld_world::Arena;

#[test]
fn what_is_near_the_cage() {
    let b = Balance::load_default().unwrap();
    let mut a = Arena::generate(&b, 909, false);
    for _ in 0..20 { a.ensure_frontier(&b, 700.0); }
    let centre = Position::new(420.0, 0.0);
    let mut near = 0;
    for r in &a.ridges {
        let (ax, az) = (r[0] as f64, r[1] as f64);
        let (bx, bz) = (r[2] as f64, r[3] as f64);
        let (dx, dz) = (bx - ax, bz - az);
        let len2 = dx * dx + dz * dz;
        let t = if len2 > 1e-6 { (((centre.x - ax) * dx + (centre.y - az) * dz) / len2).clamp(0.0, 1.0) } else { 0.0 };
        let d = (centre.x - (ax + dx * t)).hypot(centre.y - (az + dz * t));
        let hw = r[4] as f64;
        if d - hw < 60.0 {
            near += 1;
            println!("  RANGE {:.0} units away (half-width {:.0}, height {:.0})", d, r[4], r[5]);
        }
    }
    let wall_props = a.obstacles.iter().filter(|o| {
        o.position.distance_to(&centre) < 60.0
            && (o.entity_id.starts_with("obs-wall-") || o.entity_id.starts_with("obs-pass-"))
    }).count();
    let sh = a.shore();
    println!("ranges within 60u of the cage: {near} | wall props within 60u: {wall_props} | water at centre: {:.1}",
        sh.water(centre.x as f32, centre.y as f32));
}
