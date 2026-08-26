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
pub const CITY_HALF_WIDTH: f32 = 66.0;

/// How far west of the hub the spit runs before it ends in open sea.
pub const PENINSULA_LENGTH: f32 = 260.0;

/// How far west of the hub Last City itself stands, on the widest part of the spit.
///
/// **Last City is a separate scene laid out in its own coordinates**, so it cannot sample
/// `is_ocean` directly — its ground is a hand-placed plaza, not the world's terrain. It
/// therefore takes its shoreline from the two constants below, and
/// `the_city_actually_fits_on_its_own_spit` holds them against the real geometry. That
/// assertion is what keeps the two scenes agreeing; without it the city's coast is just a
/// second hand-placed shoreline, which is the exact drift this module exists to prevent.
pub const CITY_CENTER_REACH: f32 = 190.0;

/// The city has to stand ON the spit — not back on the neck, and not out past the tip.
/// Compile-time, like the neck/border relationship: it is a fact about constants.
const _: () = assert!(CITY_CENTER_REACH > NECK_REACH);
const _: () = assert!(CITY_CENTER_REACH < PENINSULA_LENGTH);

/// Half-width of the dry ground Last City is built on, in its own scene. Water starts
/// beyond this on both flanks.
pub const CITY_SHORE_HALF_WIDTH: f32 = 52.0;

/// How far past the city's centre the spit's tip lies, in the city's own scene — beyond
/// this, open sea ahead as well as to the sides.
pub const CITY_TIP_REACH: f32 = 68.0;

/// How far BEHIND the plaza the spit meets the mainland, in the city's own scene. Past
/// this the flanks are dry land again, because a spit joins a coast somewhere.
///
/// ⚠️ WITHOUT THIS TERM THE CITY HAD A RIBBON OF GRASS RUNNING TO INFINITY. The first
/// version of [`city_sea_depth`] was `max(|x| - shore, z - tip)`, which makes land the
/// strip `|x| <= shore` for EVERY z — including z going to minus infinity behind the
/// city. Don saw it immediately: "there is a weird stretch of land behind it… it goes off
/// forever as a small straight grass line." A spit needs a back edge as much as a tip.
pub const CITY_MAINLAND_BACK: f32 = 68.0;

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
    // Land is the spit OR the mainland behind it, so the sea is however far you are from
    // the nearer of the two — `min`, exactly as the world's [`sea_depth`] takes the min of
    // its fan, spit and neck. A `min` of signed distances is also what keeps this
    // CONTINUOUS: an `if z < back { return land }` would jump across that line, and every
    // smoothstep over the field would collapse into a step there — the same cliff-instead-
    // of-beach bug this module already shipped once.
    let past_spit = (x.abs() - CITY_SHORE_HALF_WIDTH).max(z - CITY_TIP_REACH);
    let past_mainland = z + CITY_MAINLAND_BACK;
    past_spit.min(past_mainland)
}

/// Is `(x, z)` walkable ground as far as the *coast* is concerned? The inverse of
/// [`is_ocean`], named for the call sites that read as "can I stand here".
pub fn is_land(x: f32, z: f32, arc_half_rad: f32) -> bool {
    !is_ocean(x, z, arc_half_rad)
}

// ---------------------------------------------------------------------------------------
// CONTINENTS
// ---------------------------------------------------------------------------------------
//
// **The fan was ONE landmass, and the line that made it one was `is_ocean`'s first
// branch: inside the arc, land, always.** So the world had a coastline and no
// continents — every bearing was solid ground from the hub to the frontier, and the only
// water a diver could ever reach was the gap behind Last City (a *frame*: it tells you
// which way is out, and there is nothing on the far side of it to walk toward).
//
// A continent here is the land BETWEEN straits. A [`Strait`] is an inland sea filling an
// annular sector — a radius band across a span of bearing — pierced by **isthmuses**. The
// landmass on either side of one is hundreds to thousands of units across, which is what
// you actually experience of a continent: not the ocean's width, but its coast and the
// crossing.
//
// ```text
//                        . . . . . . . . . .
//                  .                         .        ← continent (outer)
//              .  ~~~~~~~~~~~~~~~~~~~~~~~       .
//            .  ~~~~~~~~~[ISTHMUS]~~~~~~~~~~      .   ← a STRAIT: radius band × bearing span
//           .   ~~~~~~~~~~~~~~~~~~~~~~~~~~~~~     .
//          .                                       .  ← continent (inner)
//         .              (hub)                      .
// ```
//
// # Why this is nearly free
//
// It is a term in [`sea_depth`], and four systems already read this module rather than
// keeping their own copy of the shoreline: `astar_route` land-checks **every bent edge**
// (so the guaranteed backbone routes around a strait honestly, and to an isthmus, with no
// new pathfinding), `apply_move` collides against the same predicate, and both ground
// shaders already ramp a beach and tint a depth over the signed field. Carve the strait
// here and routing, collision and rendering come along. It also stays **analytic** — no
// colliders, so `BlockField`'s cell (sized from the largest radius in the world) is
// untouched, where one r=150 water disc measured **+63%** on the creature tick.
//
// # Two things are guaranteed by CONSTRUCTION, and both are load-bearing
//
// 1. **A strait never spans the whole fan.** Both angular ends stay
//    [`STRAIT_FAN_MARGIN`] inside the arc, so you can always walk *around* either end.
// 2. **Every strait carries isthmuses**, each at least [`MIN_BRIDGE_HALF_WIDTH`] of arc
//    to a side — comfortably wider than the clear path's tube plus a player.
//
// Together those are why this is not the retired `Seam`. A seam was a full-width wall
// with **one** door, and it was removed for good reason: it "funnelled you through a
// single pass" and the world read as a corridor. Three or more ways across every barrier
// — two isthmuses and either end — is the difference between a funnel and a *decision*:
// you meet a coast, and you choose whether the crossing you can see is worth less walking
// than the one you cannot. That lateral choice is the thing the radial world has never
// had; two players on different bearings finally see different worlds.

/// One STRAIT, flat so it rides the wire and a shader uniform as two `vec4`s (the
/// [`crate::terrain::peak_height`] precedent — an explicit table, not noise, because a
/// barrier has to be *structured*: an isotropic threshold over a sum of sines cannot make
/// a long connected channel with a pass in it at any amplitude).
///
/// `[r_center, r_half, theta_center, theta_half, b0_theta, b0_half, b1_theta, b1_half]`
///
/// Radii and the two bridge half-widths are **world units**; the two `theta`s and
/// `theta_half` are **radians**. That split is deliberate and it is the same lesson
/// [`sea_depth`] already carries: a bridge measured as an ANGLE would be a few units wide
/// near the hub and hundreds at the frontier, so the one number that has to stay walkable
/// is stored as an arc length. A bridge with `half <= 0` is absent.
pub type Strait = [f32; 8];

/// Max straits a ground shader blends at once — windowed around the player's radius, the
/// way the biome rings are. The run holds as many as it has streamed.
pub const MAX_STRAITS: usize = 8;

/// The narrowest an isthmus may ever be, as half its arc width. It has to clear the
/// guaranteed route's tube (`[worldgen] path_clear_radius`, 1.9) plus a player, with room
/// to walk rather than thread — a "crossing" you can only find by pixel-hunting is a
/// funnel with extra steps.
pub const MIN_BRIDGE_HALF_WIDTH: f32 = 11.0;

/// How far inside the fan's edge a strait's angular span must stop, as an ARC LENGTH.
/// This is what keeps "walk around its end" available at every depth: measured as an angle
/// it would vanish near the hub and be kilometres out at the frontier.
pub const STRAIT_FAN_MARGIN: f32 = 40.0;

/// A strait is generated no shallower than this, so the on-ramp is untouched water-free
/// ground. CANON §B is not negotiable — distance is difficulty — and a diver's first
/// minutes should not be a coastline.
pub const STRAIT_MIN_REACH: f32 = 180.0;

/// Signed difference between two bearings, wrapped to `[-π, π]`, so a strait centred near
/// due west is not silently a strait spanning the entire world the other way round.
fn ang_diff(a: f32, b: f32) -> f32 {
    let mut d = a - b;
    while d > std::f32::consts::PI {
        d -= std::f32::consts::TAU;
    }
    while d < -std::f32::consts::PI {
        d += std::f32::consts::TAU;
    }
    d
}

/// **How far INSIDE this strait `(x, z)` is, in world units** — positive in the water,
/// negative on the land around it, zero on its waterline. The twin of [`sea_depth`] for
/// one inland sea.
///
/// Every term is a world-unit margin, and that is what makes it composable: the angular
/// span is multiplied by `r` into an ARC so it can be `min`'d against the radial one, and
/// the result is a continuous field the ground shader can ramp a beach over. Returning an
/// angle for one term and a length for another would give the coast a beach on its curved
/// edges and a cliff on its flat ones.
///
/// Land is the union of everything, so water is the INTERSECTION of "in the band", "in the
/// span" and "not on an isthmus" — a `min` of the three. An isthmus subtracts: off the
/// bridge its term is positive (still water), on it negative (land bridge).
pub fn strait_depth(x: f32, z: f32, s: &Strait) -> f32 {
    let (r_c, r_half, th_c, th_half) = (s[0], s[1], s[2], s[3]);
    if r_half <= 0.0 || th_half <= 0.0 {
        return -1000.0; // an empty slot is not a sea
    }
    let r = x.hypot(z);
    let theta = z.atan2(x);
    // Inside the radius band…
    let in_band = r_half - (r - r_c).abs();
    // …inside the bearing span, as an arc length so it composes with the rest…
    let in_span = (th_half - ang_diff(theta, th_c).abs()) * r;
    // …and not standing on one of its isthmuses.
    let mut off_bridge = f32::MAX;
    for b in [(s[4], s[5]), (s[6], s[7])] {
        if b.1 > 0.0 {
            off_bridge = off_bridge.min(ang_diff(theta, b.0).abs() * r - b.1);
        }
    }
    in_band.min(in_span).min(off_bridge)
}

/// [`sea_depth`] plus this world's inland seas — the full shoreline of a world that has
/// **continents** rather than one unbroken fan.
///
/// The sea is the union of the ocean and every strait, and a signed depth's union is a
/// `max`: past the ocean's land OR inside a strait. On open ground far from either the
/// ocean's own (negative) distance survives, so the beach at the fan's rim is unchanged.
pub fn sea_depth_with(x: f32, z: f32, arc_half_rad: f32, straits: &[Strait]) -> f32 {
    let mut d = sea_depth(x, z, arc_half_rad);
    if arc_half_rad <= 0.0 {
        return d; // corridor mode: no fan, no sea, and so no continents either
    }
    for s in straits {
        d = d.max(strait_depth(x, z, s));
    }
    d
}

/// [`is_ocean`], including the inland seas that separate the continents. Its sign is
/// [`sea_depth_with`] exactly — the water you SEE and the water you COLLIDE with are one
/// shoreline, which is the entire reason this module exists.
pub fn is_ocean_with(x: f32, z: f32, arc_half_rad: f32, straits: &[Strait]) -> bool {
    if is_ocean(x, z, arc_half_rad) {
        return true;
    }
    if arc_half_rad <= 0.0 {
        return false;
    }
    straits.iter().any(|s| strait_depth(x, z, s) > 0.0)
}

/// Is `(x, z)` walkable ground, continents included? The inverse of [`is_ocean_with`].
pub fn is_land_with(x: f32, z: f32, arc_half_rad: f32, straits: &[Strait]) -> bool {
    !is_ocean_with(x, z, arc_half_rad, straits)
}

/// Does `s` honour the two construction guarantees — an isthmus wide enough to walk, and
/// both angular ends stopping inside the fan so you can round either one?
///
/// Exposed (rather than left to the generator) because it is the *contract*, and this repo
/// has learned twice that a guarantee enforced only where a thing is built is a guarantee
/// the next builder does not know about. The generator asserts it; so do the tests.
pub fn strait_is_crossable(s: &Strait, arc_half_rad: f32) -> bool {
    let (r_c, r_half, th_c, th_half) = (s[0], s[1], s[2], s[3]);
    if r_half <= 0.0 || th_half <= 0.0 || r_c <= r_half {
        return false;
    }
    // At least one isthmus, wide enough to walk, and inside the span it pierces.
    let bridged = [(s[4], s[5]), (s[6], s[7])].iter().any(|&(bt, bh)| {
        bh >= MIN_BRIDGE_HALF_WIDTH && ang_diff(bt, th_c).abs() <= th_half
    });
    // Both ends stop inside the fan, so rounding either one is always an option.
    let r_out = r_c + r_half;
    let margin = STRAIT_FAN_MARGIN / r_out.max(1.0);
    let ends_inside = th_c.abs() + th_half + margin <= arc_half_rad;
    bridged && ends_inside
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
                // The rule stated independently of the depth field: off the spit AND
                // not yet onto the mainland behind it.
                let off_spit = x.abs() > CITY_SHORE_HALF_WIDTH || z > CITY_TIP_REACH;
                let sea = off_spit && z > -CITY_MAINLAND_BACK;
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
        // …and the spit has a BACK. Land far behind the city is the mainland it joins, not
        // a ribbon of grass running to the horizon.
        assert!(
            city_sea_depth(CITY_SHORE_HALF_WIDTH + 200.0, -CITY_MAINLAND_BACK - 200.0) < 0.0,
            "well behind the city is mainland, on the flanks as much as the centre"
        );
        assert!(
            city_sea_depth(CITY_SHORE_HALF_WIDTH + 10.0, 0.0) > 0.0,
            "beside the plaza is still sea"
        );
    }

    #[test]
    fn the_fan_is_all_land() {
        // Every angle inside the fan, at every depth, is walkable ground — the OCEAN only
        // ever occupies the western gap. This is a statement about the ocean's shape, not
        // about the world having one landmass: the inland seas that separate the continents
        // are a separate term, and they come in through `is_ocean_with`.
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

    // -----------------------------------------------------------------------------------
    // CONTINENTS
    // -----------------------------------------------------------------------------------

    /// A strait for the tests below: a band 40 units thick at r=600, spanning 50° of
    /// bearing centred on the axis, with two isthmuses in it.
    fn test_strait() -> Strait {
        let th_half = 50.0_f32.to_radians() * 0.5;
        [600.0, 20.0, 0.0, th_half, -th_half * 0.55, 14.0, th_half * 0.5, 14.0]
    }

    /// The whole point of the feature: a strait is water, and there are **three or more**
    /// ways past it — each isthmus, and around either end. That is what separates this from
    /// the retired `Seam`, a full-width wall with one door that made the world read as a
    /// corridor.
    #[test]
    fn a_strait_is_water_you_can_cross_and_water_you_can_round() {
        let s = test_strait();
        let at = |theta: f32, r: f32| {
            let (x, z) = (r * theta.cos(), r * theta.sin());
            is_ocean_with(x, z, ARC_HALF, &[s])
        };
        let th_half = s[3];

        // Open water: mid-band, mid-span, clear of both isthmuses.
        assert!(at(th_half * 0.9, 600.0), "the middle of the strait is open water");

        // Both isthmuses are dry land bridges, right through the middle of the band.
        assert!(!at(s[4], 600.0), "the first isthmus is a land bridge");
        assert!(!at(s[6], 600.0), "the second isthmus is a land bridge");

        // Either end of the span is land, so you can always walk around it…
        assert!(!at(th_half * 1.3, 600.0), "past the span's far end is land");
        assert!(!at(-th_half * 1.3, 600.0), "past the span's near end is land");
        // …and so is the ground on both shores.
        assert!(!at(th_half * 0.9, 540.0), "the near shore is land");
        assert!(!at(th_half * 0.9, 660.0), "the far shore is land");
    }

    /// Sweep the strait's own band and count how many separate stretches of land cross it.
    /// Three is the floor (two isthmuses + one end); the far end is outside the swept span
    /// on purpose, so this asserts the crossings that are *inside* the barrier.
    #[test]
    fn a_strait_never_severs_the_continent_it_borders() {
        let s = test_strait();
        let th_half = s[3];
        let mut crossings = 0;
        let mut was_land = false;
        for i in 0..=4000 {
            let th = -th_half * 1.2 + (i as f32 / 4000.0) * th_half * 2.4;
            let (x, z) = (600.0 * th.cos(), 600.0 * th.sin());
            let land = !is_ocean_with(x, z, ARC_HALF, &[s]);
            if land && !was_land {
                crossings += 1;
            }
            was_land = land;
        }
        assert!(
            crossings >= 3,
            "a strait must offer at least three ways past it — two isthmuses and its ends \
             — or it is the retired `Seam`: one door, and a world that reads as a corridor. \
             Found {crossings}"
        );
    }

    /// The contract, checked as a contract: an isthmus wide enough to walk, and both ends
    /// stopping inside the fan. A strait that fails this can sever the world.
    #[test]
    fn the_crossable_contract_catches_a_severing_strait() {
        let arc = ARC_HALF;
        assert!(strait_is_crossable(&test_strait(), arc));

        let mut narrow = test_strait();
        narrow[5] = MIN_BRIDGE_HALF_WIDTH - 0.5;
        narrow[7] = MIN_BRIDGE_HALF_WIDTH - 0.5;
        assert!(!strait_is_crossable(&narrow, arc), "a thread is not an isthmus");

        let mut unbridged = test_strait();
        unbridged[5] = 0.0;
        unbridged[7] = 0.0;
        assert!(!strait_is_crossable(&unbridged, arc), "a strait with no isthmus is a wall");

        // A span that reaches the fan's rim closes the "walk around its end" option.
        let mut rim = test_strait();
        rim[3] = arc;
        assert!(!strait_is_crossable(&rim, arc), "a span that reaches the rim has no end to round");

        // An isthmus outside the span it is meant to pierce pierces nothing.
        let mut adrift = test_strait();
        adrift[4] = arc * 0.9;
        adrift[6] = arc * 0.9;
        assert!(!strait_is_crossable(&adrift, arc), "an isthmus must sit inside its own strait");
    }

    /// The continental shoreline is ONE shoreline, exactly as the ocean's is
    /// (`the_depth_field_agrees_with_the_predicate`). The server collides against
    /// `is_ocean_with` and the ground shader ramps its beach over `sea_depth_with`; a
    /// disagreement is water you can walk on, or a beach drawn over sea.
    #[test]
    fn the_continental_depth_field_agrees_with_the_predicate() {
        let straits = [
            test_strait(),
            [300.0, 18.0, 0.6, 0.35, 0.45, 13.0, 0.72, 13.0],
            [900.0, 30.0, -0.9, 0.5, -1.1, 16.0, -0.7, 16.0],
        ];
        let mut checked = 0u32;
        for xi in -70..=70 {
            for zi in -70..=70 {
                let (x, z) = (xi as f32 * 13.7, zi as f32 * 13.7);
                if x.hypot(z) < 1.0 {
                    continue; // the origin has no angle
                }
                let depth = sea_depth_with(x, z, ARC_HALF, &straits);
                let ocean = is_ocean_with(x, z, ARC_HALF, &straits);
                if depth.abs() > 1e-3 {
                    assert_eq!(
                        depth > 0.0,
                        ocean,
                        "({x}, {z}): sea_depth_with says {depth} but is_ocean_with says \
                         {ocean} — the shoreline the player SEES and the one they COLLIDE \
                         with have drifted"
                    );
                    checked += 1;
                }
            }
        }
        assert!(checked > 10_000, "the sweep covered almost nothing ({checked} points)");
    }

    /// A strait's field has to be CONTINUOUS, because the ground shader smoothsteps a beach
    /// over it — the same lesson this module already shipped twice (the flat `-1000` inside
    /// the fan, and the city's `if z < back` early return). A jump means a wall of water
    /// with no beach possible in between.
    #[test]
    fn a_straits_shoreline_has_a_beach_rather_than_a_cliff() {
        let s = test_strait();
        // March across the strait's near shore in 0.5-unit steps; the field may never jump
        // by more than the step it took to get there (times slack for the arc's curvature).
        let th = s[3] * 0.9;
        let mut prev: Option<f32> = None;
        let mut r = 540.0_f32;
        while r <= 660.0 {
            let (x, z) = (r * th.cos(), r * th.sin());
            let d = sea_depth_with(x, z, ARC_HALF, &[s]);
            if let Some(p) = prev {
                assert!(
                    (d - p).abs() < 2.0,
                    "the sea depth jumped {:.1} units over a 0.5-unit step at r={r} — that \
                     is a cliff of water, and every smoothstep over it collapses to a step",
                    (d - p).abs()
                );
            }
            prev = Some(d);
            r += 0.5;
        }
    }

    /// Corridor mode (the tutorial, and every unit test that runs flat) has no fan, so it
    /// has no sea — and therefore no continents either, however many straits are handed in.
    #[test]
    fn corridor_mode_has_no_continents() {
        let s = test_strait();
        assert!(!is_ocean_with(600.0, 0.0, 0.0, &[s]));
        assert!(!is_ocean_with(600.0, 30.0, 0.0, &[s]));
        assert!(sea_depth_with(600.0, 0.0, 0.0, &[s]) < 0.0);
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

