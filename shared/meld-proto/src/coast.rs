//! The coastline — where the world's land ends and the sea begins.
//!
//! **This exists because the shoreline is authored in two places that cannot see each
//! other.** The arena is server truth (movement, path routing, placement); Last City is a
//! *separate Bevy scene* (`Screen::City`, its own scene/move/camera, reached by a screen
//! transition rather than by walking). Nothing makes their geometry agree — and this
//! repo's whole catalogue of self-inflicted bugs is one rule living in two places: the
//! wall-collision line that went into one mover and not the other, the maze density
//! written once in `push_section` and again in `reroll_props`, and the `terrain.rs` ↔ WGSL
//! mirror that has to be hand-kept in lock-step. A peninsula whose two halves disagree is
//! that bug wearing scenery, so the shoreline — **and the neck** — live here, once.
//!
//! # The shape
//!
//! The world fans out east over `radial_arc_degrees`, leaving a **gap** to the west. That
//! gap is the sea, except for a **spit of land running west from the hub**, which is where
//! Last City stands:
//!
//! ```text
//!                    . . . . . . .          |theta| <= arc_half  ->  LAND (the fan)
//!            .                     .
//!        ~~~~~~~~                   .
//!      ~~~ sea ~~~                   .
//!   [CITY]======NECK====(hub)         .     the gap  ->  SEA, except the spit
//!      ~~~ sea ~~~                   .
//!        ~~~~~~~~                   .
//!            .                     .
//!                    . . . . . . .
//! ```
//!
//! **The neck is not authored as a width — it falls out of the geometry.** Near the hub
//! the gap is a narrow wedge (its half-width is `r · tan(gap_half)`), too tight to hold a
//! channel, so the land closes across it. That land bridge *is* the neck, and it is the
//! only way in or out of the city on foot: the Threshold stops being a UI affordance and
//! becomes a geographic fact. It is also why the city is defensible, and why a siege
//! (`BD-4`/`BD-8`) has exactly one axis of approach.
//!
//! # Why this is free at runtime
//!
//! The sea has an **analytic boundary**, so it never becomes colliders. Every query is
//! this predicate — O(1), no props, and no effect on `BlockField`'s spatial hash (whose
//! cell is sized from the largest radius in the world, so a single big collider coarsens
//! the grid for everything; measured, one r=150 disc cost **+63%** on the creature tick).
//! An ocean made of geometry would have been the most expensive object in the game. As a
//! function it costs nothing.
//!
//! ⚠️ **The gap has to be wide enough to hold water.** At the original 340° arc the gap
//! was 20°, so its half-width at r=30 is `30 · tan(10°)` ≈ **5.3 units** — the fan came
//! around to within a few units of due west and there was no room for a sea beside the
//! neck at all. A peninsula is not expressible at that arc; `radial_arc_degrees` is the
//! knob, and `the_sea_is_wide_enough_to_see` holds the relationship rather than the value.

/// GLSL-style smoothstep, matching the one in [`crate::terrain`] and the WGSL mirror.
fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// How far west of the hub the land bridge reaches before the sea opens on both sides.
/// **This is the neck.**
///
/// ⚠️ It has to sit INSIDE `[worldgen] west_return_border`, not outside it, and getting
/// that backwards is what made the peninsula invisible on the day it shipped: the border
/// was -20 and this was 34, so a player walking west was returned to Last City **fourteen
/// units before the sea opened**. The coastline was real in the world model, correct in
/// every test, and no player could ever reach it. The screenshots that "verified" it were
/// taken with `MELD_IDLE` and a pulled-back survey camera — looking over a boundary the
/// player cannot cross. A geometry test is not a reachability test.
///
/// The invariant now runs the other way and is asserted at compile time: the neck ends,
/// the sea opens on both flanks, and only THEN does the border hand you to the city — so
/// the walk home crosses the shore rather than stopping short of it.
pub const NECK_REACH: f32 = 14.0;

/// Where `[worldgen] west_return_border` sits, mirrored here so the relationship below can
/// be checked at all. Balance owns the real value; this is the contract it must honour.
pub const RETURN_BORDER_REACH: f32 = 46.0;

/// The player must cross the shore before the city takes them: the neck has to END well
/// inside the return border, leaving open water on both flanks for the last stretch of the
/// walk — and it has to end near enough the hub that the sea is IN FRAME from spawn rather
/// than a rumour you would have to go looking for. Checked at COMPILE time rather than in a
/// test, because it is a structural relationship between constants and a build that
/// violates it ships an ocean nobody can see.
const _: () = assert!(NECK_REACH > 8.0);
const _: () = assert!(NECK_REACH * 2.0 < RETURN_BORDER_REACH);

/// Half-width of the spit at its landward end, where it leaves the neck.
pub const NECK_HALF_WIDTH: f32 = 12.0;

/// Half-width of the spit at its widest — the shelf Last City is built on.
pub const CITY_HALF_WIDTH: f32 = 34.0;

/// How far west of the hub the spit runs before it ends in open sea.
pub const PENINSULA_LENGTH: f32 = 150.0;

/// How far west of the hub Last City itself stands, on the widest part of the spit.
///
/// **Last City is a separate scene laid out in its own coordinates**, so it cannot sample
/// `is_ocean` directly — its ground is a hand-placed plaza, not the world's terrain. It
/// therefore takes its shoreline from the two constants below, and
/// `the_city_actually_fits_on_its_own_spit` holds them against the real geometry. That
/// assertion is what keeps the two scenes agreeing; without it the city's coast is just a
/// second hand-placed shoreline, which is the exact drift this module exists to prevent.
pub const CITY_CENTER_REACH: f32 = 110.0;

/// The city has to stand ON the spit — not back on the neck, and not out past the tip.
/// Compile-time, like the neck/border relationship: it is a fact about constants.
const _: () = assert!(CITY_CENTER_REACH > NECK_REACH);
const _: () = assert!(CITY_CENTER_REACH < PENINSULA_LENGTH);

/// Half-width of the dry ground Last City is built on, in its own scene. Water starts
/// beyond this on both flanks.
pub const CITY_SHORE_HALF_WIDTH: f32 = 26.0;

/// How far past the city's centre the spit's tip lies, in the city's own scene — beyond
/// this, open sea ahead as well as to the sides.
pub const CITY_TIP_REACH: f32 = 34.0;

/// Fraction of the spit's run over which it tapers to its tip, so the peninsula ends in a
/// point rather than a cliff-edged rectangle.
pub const TIP_TAPER: f32 = 0.22;

/// The most of the western gap's width the spit is ever allowed to take, leaving the rest
/// as open water. **This is what makes the channel a guarantee rather than a tuning
/// accident**: the gap narrows toward the hub, so a spit authored at a fixed width would
/// silently swallow the sea near the neck (it did — the first draft left 7.6 units of
/// water at d=50 and the test caught it). Bounding the land as a SHARE of the gap means
/// there is sea on both flanks at every depth the spit exists, by construction, whatever
/// the arc is retuned to. Same discipline as the clear path's guaranteed route and the
/// `Seam`'s guaranteed door.
pub const CHANNEL_LAND_SHARE: f32 = 0.5;

/// Half-width of the land at `d` world units west of the hub, measured across the spit.
/// Zero past the tip. Only meaningful inside the western gap — the fan itself is land at
/// any width (see [`is_ocean`]).
pub fn peninsula_half_width(d: f32, arc_half_rad: f32) -> f32 {
    if d <= NECK_REACH {
        return NECK_HALF_WIDTH;
    }
    if d >= PENINSULA_LENGTH {
        return 0.0;
    }
    let t = (d - NECK_REACH) / (PENINSULA_LENGTH - NECK_REACH);
    // Swell from the neck out to the city's shelf and back — a spit, not a corridor.
    let swell = (std::f32::consts::PI * t).sin();
    let w = NECK_HALF_WIDTH + (CITY_HALF_WIDTH - NECK_HALF_WIDTH) * swell;
    // …then close to a point over the last stretch.
    let w = w * smoothstep(1.0, 1.0 - TIP_TAPER, t);
    // Never take more than its share of the gap, so the sea beside it cannot vanish.
    let gap_half = (std::f32::consts::PI - arc_half_rad).max(0.0);
    w.min(d * gap_half.tan() * CHANNEL_LAND_SHARE)
}

/// Is world position `(x, z)` open sea? `arc_half_rad` is half the world's fan
/// (`radial_arc_degrees.to_radians() * 0.5`) — passed in rather than baked, so the server
/// and the client cannot disagree about it the way two hand-placed shorelines would.
///
/// Land is: anywhere inside the fan, the neck's land bridge, and the spit. Everything else
/// in the western gap is sea.
pub fn is_ocean(x: f32, z: f32, arc_half_rad: f32) -> bool {
    // A degenerate arc means corridor mode (no fan) — there is no gap, so no sea.
    if arc_half_rad <= 0.0 {
        return false;
    }
    // Inside the fan: land, always. This is the overwhelming majority of every query.
    if z.atan2(x).abs() <= arc_half_rad {
        return false;
    }
    // The western gap. Near the hub it is too narrow to hold a channel and the land closes
    // across it — the NECK.
    let d = x.hypot(z);
    if d <= NECK_REACH {
        return false;
    }
    // Otherwise: sea, except on the spit. A zero width is past the tip — open water even
    // dead on the axis, or the peninsula would run west forever as a one-point-wide line.
    let w = peninsula_half_width(d, arc_half_rad);
    w <= 0.0 || z.abs() > w
}

/// **How far past the shoreline `(x, z)` is, in world units** — negative on land, positive
/// at sea, zero exactly on the waterline.
///
/// [`is_ocean`] answers a yes/no, which is all movement needs. The RENDERER needs the
/// magnitude: the beach ramp, the depth colour and the swell all smoothstep over this, and
/// a predicate that only knows "land" cannot tell them how much beach to make. The ground
/// shader mirrors this function, and it used to mirror a version that returned a flat
/// `-1000` for everything inside the fan — so the field jumped from `-1000` to about `+26`
/// across the fan's edge, every smoothstep over it collapsed to a step, and the coast there
/// rendered as a vertical wall of water with no beach possible in between.
///
/// Land is three shapes, so the sea is however far you are from the nearest of them:
/// the FAN (its edge is a ray, so the distance past it is an ARC LENGTH — a fixed angular
/// margin would be metres at the hub and kilometres at the frontier), the SPIT across its
/// width, and the NECK that closes the gap near the hub.
///
/// Its SIGN is `is_ocean` exactly — held by `the_depth_field_agrees_with_the_predicate`,
/// because the thing you can see and the thing you collide with must be one shoreline.
pub fn sea_depth(x: f32, z: f32, arc_half_rad: f32) -> f32 {
    if arc_half_rad <= 0.0 {
        return -1000.0; // corridor mode: no gap, no sea
    }
    let d = x.hypot(z);
    let theta = z.atan2(x).abs();
    let past_fan = (theta - arc_half_rad) * d;
    let past_spit = z.abs() - peninsula_half_width(d, arc_half_rad);
    let past_neck = d - NECK_REACH;
    past_fan.min(past_spit).min(past_neck)
}

/// How far past LAST CITY's own shoreline `(x, z)` is, **in city-scene coordinates** —
/// negative on the spit, positive at sea. The city's twin of [`sea_depth`].
///
/// The city is a separate scene laid out in its own coordinates, so [`is_ocean`] cannot
/// answer there (the world's shoreline, expressed in city space, runs through the plaza).
/// Its spit is the simple one the constants describe: land within `CITY_SHORE_HALF_WIDTH`
/// of the centreline and short of `CITY_TIP_REACH`, sea beyond either.
///
/// It is a DEPTH rather than a predicate for the same reason [`sea_depth`] is: the ground
/// shader smoothsteps over it to make the plaza dip into a real bay, and a boolean has no
/// magnitude to ramp. Both the shader and the scenery cull read this one function, so the
/// city cannot grow a second hand-placed shoreline — which is exactly what it had, three
/// water planes laid a hair above the lawn, quietly missing every fix the world's sea got.
pub fn city_sea_depth(x: f32, z: f32) -> f32 {
    (x.abs() - CITY_SHORE_HALF_WIDTH).max(z - CITY_TIP_REACH)
}

/// Is `(x, z)` walkable ground as far as the *coast* is concerned? The inverse of
/// [`is_ocean`], named for the call sites that read as "can I stand here".
pub fn is_land(x: f32, z: f32, arc_half_rad: f32) -> bool {
    !is_ocean(x, z, arc_half_rad)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The arc the world actually ships with, for the geometry tests below. Balance owns
    /// the real value; this only has to be the same *kind* of world.
    const ARC_HALF: f32 = 150.0_f32 * std::f32::consts::PI / 180.0; // 300° fan

    /// The signed depth field and the boolean predicate are ONE shoreline.
    ///
    /// They are consulted by different halves of the game — the server collides against
    /// `is_ocean`, the ground shader ramps and colours over `sea_depth` — and this repo's
    /// standing failure is one rule living in two places. A disagreement here is water you
    /// can walk on, or a beach that renders over ground the server calls sea.
    #[test]
    fn the_depth_field_agrees_with_the_predicate() {
        let mut checked = 0u32;
        for xi in -60..=60 {
            for zi in -60..=60 {
                let (x, z) = (xi as f32 * 7.3, zi as f32 * 7.3);
                if x.hypot(z) < 1.0 {
                    continue; // the origin has no angle
                }
                let depth = sea_depth(x, z, ARC_HALF);
                let ocean = is_ocean(x, z, ARC_HALF);
                // Exactly on the waterline either answer is defensible; everywhere else the
                // sign is the predicate.
                if depth.abs() > 1e-3 {
                    assert_eq!(
                        depth > 0.0,
                        ocean,
                        "({x}, {z}): sea_depth says {depth} but is_ocean says {ocean} — the \
                         shoreline the player SEES and the one they COLLIDE with have drifted"
                    );
                    checked += 1;
                }
            }
        }
        assert!(checked > 10_000, "the sweep covered almost nothing ({checked} points)");
    }

    /// The city's depth field is the city's shoreline — the one the ground shader dips on
    /// and the one scenery is culled against. Same discipline as
    /// `the_depth_field_agrees_with_the_predicate` for the world: a magnitude and a
    /// boolean describing one coast must never disagree about where it is.
    #[test]
    fn the_citys_depth_field_is_its_shoreline() {
        for xi in -40..=40 {
            for zi in -40..=40 {
                let (x, z) = (xi as f32 * 2.7, zi as f32 * 2.7);
                let depth = city_sea_depth(x, z);
                let sea = x.abs() > CITY_SHORE_HALF_WIDTH || z > CITY_TIP_REACH;
                if depth.abs() > 1e-3 {
                    assert_eq!(depth > 0.0, sea, "({x}, {z}) disagrees about the city's coast");
                }
            }
        }
        // And the city actually HAS a bay to dip into: the plaza at the origin is dry, and
        // both flanks and the tip are wet. A shoreline that put water through the fountain
        // would satisfy the agreement above and still be wrong.
        assert!(city_sea_depth(0.0, 0.0) < 0.0, "the plaza must be dry ground");
        assert!(city_sea_depth(CITY_SHORE_HALF_WIDTH + 5.0, 0.0) > 0.0, "left flank is sea");
        assert!(city_sea_depth(0.0, CITY_TIP_REACH + 5.0) > 0.0, "past the tip is sea");
    }

    #[test]
    fn the_fan_is_all_land() {
        // Every angle inside the fan, at every depth, is walkable ground — the sea only
        // ever occupies the western gap.
        for i in 0..=100 {
            let th = -ARC_HALF + (i as f32 / 100.0) * 2.0 * ARC_HALF;
            for r in [5.0_f32, 40.0, 200.0, 1200.0] {
                let (x, z) = (r * th.cos(), r * th.sin());
                assert!(
                    !is_ocean(x, z, ARC_HALF),
                    "inside the fan must be land (r={r}, theta={:.1}°)",
                    th.to_degrees()
                );
            }
        }
    }

    #[test]
    fn the_neck_is_a_land_bridge_and_the_city_is_reachable_on_foot() {
        // Walk due west from the hub to the city's shelf: every step is land, so the only
        // route out of Last City is over the neck and there is always one.
        let mut d = 0.0_f32;
        while d <= PENINSULA_LENGTH * 0.6 {
            assert!(
                !is_ocean(-d, 0.0, ARC_HALF),
                "the walk west along the spit must stay on land (d={d})"
            );
            d += 0.5;
        }
    }

    #[test]
    fn the_sea_is_wide_enough_to_see() {
        // The point of a peninsula is water you can SEE from the shore. The gap has to be
        // wide enough to hold a channel beside the spit — at the original 340° arc it was
        // not (5.3 units of half-width at r=30), which is why the arc had to widen. This
        // holds the RELATIONSHIP, not the arc value: wherever the spit runs, there must be
        // real open water between it and the fan's coastline.
        let gap_half = std::f32::consts::PI - ARC_HALF;
        for d in [50.0_f32, 80.0, 120.0] {
            let wedge = d * gap_half.tan();
            let land = peninsula_half_width(d, ARC_HALF);
            let channel = wedge - land;
            assert!(
                channel > 8.0,
                "there must be open sea beside the spit at d={d} \
                 (gap half-width {wedge:.1}, spit {land:.1}, channel {channel:.1})"
            );
        }
    }

    #[test]
    fn past_the_tip_is_open_sea() {
        // The spit ends. Otherwise "peninsula" is just a corridor running west forever.
        assert!(
            is_ocean(-(PENINSULA_LENGTH + 20.0), 0.0, ARC_HALF),
            "due west past the tip must be open water"
        );
        assert_eq!(peninsula_half_width(PENINSULA_LENGTH + 1.0, ARC_HALF), 0.0);
    }

    #[test]
    fn the_spit_has_water_on_both_sides() {
        // Water to the north AND south of the city's shelf — that is what makes it a
        // peninsula rather than a headland.
        let d = (NECK_REACH + PENINSULA_LENGTH) * 0.5;
        let w = peninsula_half_width(d, ARC_HALF);
        for side in [1.0_f32, -1.0] {
            assert!(
                is_ocean(-d, side * (w + 6.0), ARC_HALF),
                "the sea must reach both flanks of the spit (side {side})"
            );
        }
    }

    #[test]
    fn the_city_actually_fits_on_its_own_spit() {
        // Last City is a separate scene and draws its shore from `CITY_SHORE_HALF_WIDTH`
        // rather than by sampling `is_ocean`. That is only safe while the constant agrees
        // with the real spit — otherwise the city's water sits where the world's is land
        // (or worse, the reverse) and the two scenes tell different stories about the same
        // place. This is the assertion that keeps them honest.
        let w = peninsula_half_width(CITY_CENTER_REACH, ARC_HALF);
        assert!(
            w >= CITY_SHORE_HALF_WIDTH,
            "the spit at the city's reach ({CITY_CENTER_REACH}) is {w:.1} half-wide, but \
             the city scene draws {CITY_SHORE_HALF_WIDTH} of dry ground — the city would \
             be standing in its own sea"
        );
    }

    #[test]
    fn corridor_mode_has_no_sea() {
        // A zero arc is the flat-corridor world the tests and the tutorial still use.
        assert!(!is_ocean(-500.0, 400.0, 0.0));
    }
}

/// Is this obstacle kind **water**? The module already owns where the *sea* is
/// ([`is_ocean`]); this is the other half of the same question — the pools scattered
/// inland, which are water by KIND rather than by geometry.
///
/// **It lives here because it was written three times and was one edit from
/// disagreeing.** `meld_world::is_water_kind` decides which fills may POOL (two bog
/// pools touching make a bigger mere, where two touching boulders are a clipping bug);
/// the client's prop spawner decides which get a basin, a water tile and a drifting
/// surface; the client's palette decides which read blue. Three lists of the same three
/// strings, in two workspaces — so a new water kind (a lava pool, a tarn, a flooded
/// crater) lands in the world, pools correctly, and renders as a *boulder*, because the
/// spawner's copy never heard about it. That is the `GearBonus` bug and the
/// wall-collision bug and the `push_section`/`reroll_props` bug, again.
///
/// Adding a kind here gives it pooling, a basin, a tile and a colour at once.
pub fn is_water_kind(kind: &str) -> bool {
    matches!(kind, "pond" | "bog_pool" | "frozen_pond")
}

#[cfg(test)]
mod water_kind_tests {
    use super::*;

    #[test]
    fn the_three_water_kinds_are_water_and_a_boulder_is_not() {
        for k in ["pond", "bog_pool", "frozen_pond"] {
            assert!(is_water_kind(k), "{k} should be water");
        }
        for k in ["boulder", "tree", "cliff", "lava_vent", ""] {
            assert!(!is_water_kind(k), "{k} should not be water");
        }
    }
}
