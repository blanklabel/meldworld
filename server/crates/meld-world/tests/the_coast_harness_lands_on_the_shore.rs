//! `MELD_COAST` must put the party ON LAND with open water in front of them.
//!
//! The flag exists because the sea was unphotographable — autoplay walks east into the
//! maze and the survey camera frames whatever the party stands on, so four separate water
//! diagnoses were tuned against frames containing no sea at all. A harness that lands in
//! the wrong place would recreate exactly that problem while looking like it worked, so
//! the landing rule is pinned here rather than trusted.
//!
//! This mirrors the placement in `meld-server`'s `MELD_COAST` branch: the fan spans
//! `|theta| <= arc_half` and everything past it is sea, so the shoreline is that edge at
//! any radius.

use meld_proto::coast::is_ocean;

/// The same landing the server computes, kept as one expression so a change to either
/// side is visible as a difference here.
fn landing(reach: f64, arc_half: f64) -> (f64, f64) {
    let theta = arc_half * 0.97;
    (reach * theta.cos(), reach * theta.sin())
}

#[test]
fn the_party_lands_on_land_and_the_sea_is_just_past_it() {
    let arc_half = 300.0f64.to_radians() * 0.5; // `[worldgen] radial_arc_degrees`
    for reach in [40.0f64, 90.0, 160.0, 400.0, 1200.0] {
        let (x, z) = landing(reach, arc_half);

        // You cannot stand in the sea.
        assert!(
            !is_ocean(x as f32, z as f32, arc_half as f32),
            "d{reach}: the landing is in the water at ({x:.1}, {z:.1})"
        );

        // ...and the water has to be RIGHT THERE, or the harness frames a field. Step
        // outward in angle from the landing and the sea must begin within a few paces.
        let mut found = None;
        for step in 1..=40 {
            let theta = arc_half * 0.97 + (step as f64) * 0.002;
            let (sx, sz) = (reach * theta.cos(), reach * theta.sin());
            if is_ocean(sx as f32, sz as f32, arc_half as f32) {
                let gap = ((sx - x).powi(2) + (sz - z).powi(2)).sqrt();
                found = Some(gap);
                break;
            }
        }
        let gap = found.unwrap_or_else(|| panic!("d{reach}: never reached the sea at all"));
        assert!(
            gap < reach * 0.12,
            "d{reach}: the shore is {gap:.1} units away — too far to frame"
        );
    }
}

/// A degenerate arc is corridor mode: no fan, no gap, and therefore no sea to look at.
/// The harness should not silently "work" there.
#[test]
fn a_corridor_world_has_no_coast_to_stand_on() {
    assert!(!is_ocean(50.0, 0.0, 0.0), "corridor mode has no sea");
    assert!(!is_ocean(50.0, 900.0, 0.0), "corridor mode has no sea anywhere");
}
