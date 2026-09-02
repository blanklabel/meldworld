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
    // The western gap. Near the hub it is too narrow to hold a channel and the land closes
    // across it — the NECK.
    let d = x.hypot(z);
    if d <= NECK_REACH {
        return false;
    }
    // Inside the fan: land, always. This is the overwhelming majority of every query.
    //
    // ⚠️ **THE FAN'S OWN ANGLE AT THIS RADIUS, not the nominal one.** This fast path read
    // the constant `arc_half_rad` while [`sea_depth`] measures against [`arc_half_at`], so
    // the moment the world gained a taper and a wandering coast the two disagreed: this said
    // LAND everywhere inside the nominal 150° while the depth field said SEA wherever the
    // coast had bitten in. `the_depth_field_agrees_with_the_predicate` exists for exactly
    // that — the shoreline you SEE and the one you COLLIDE with must be one shoreline — and
    // it caught it. Eighth site of the same rule: everything that asks how wide the world is
    // here has to ask the same function.
    if z.atan2(x).abs() <= arc_half_at(d, arc_half_rad) {
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
/// How far the coastline wanders in or out, as a FRACTION of the fan's local half-angle.
/// A fraction rather than an angle, because a fixed angular wobble is metres at the hub and
/// kilometres at the frontier. Bounded well under 1.0 so a wander can never pinch the fan.
pub const COAST_WANDER: f32 = 0.06;
/// Radial distance over which the coast's first harmonic completes a cycle. Long enough that
/// a bay-sized wander takes a real walk to round rather than reading as noise on the edge.
pub const COAST_WANDER_WAVELENGTH: f32 = 520.0;

/// **WG-11: THE WORLD IS A TEARDROP.** Where the land starts closing back in.
pub const TAPER_START: f32 = 1200.0;
/// Where it has closed to its final width — the end of the world, and the prison's door.
pub const TAPER_END: f32 = 3200.0;
/// How wide the land is at [`TAPER_END`] and everywhere past it: the corridor you walk to
/// the prison. Narrow enough that no town can be founded in it, so the final approach is
/// always on foot — and that falls out of the geometry rather than needing a rule.
pub const END_WIDTH: f32 = 200.0;

/// The fan's half-angle at radius `d` — **the world's own shape, not a constant.**
///
/// A constant half-angle makes the world a wedge that widens forever: ~2,000 units of arc at
/// r=400 against ~15,700 at r=3000, so the deep world is mostly ground nobody will stand on,
/// and anything placed per unit of arc out there is paid for and never seen. Tapering makes
/// it a **teardrop**, and buys three things at once:
///
/// - **The ending becomes a PLACE.** `seraphic_oubliette` is `EXCLUSIVE` past its gate, so
///   *"only the prison draws out here"* and *"the world funnels to a point"* stop being two
///   statements and become one. No cell has to be chosen as the prison.
/// - **Deep rings get SMALLER instead of quadratically larger**, which is what makes the
///   cell-graph maze affordable: wall cost rides the boundary count, which rides the arc.
/// - **No town fits in a 200-unit corridor.**
///
/// Held FULL until `TAPER_START` and smoothstepped after, because the taper fights the
/// anti-east rule: a funnel makes the centre line geometrically shortest and the pull
/// strengthens toward the point. Gentle through the mid-world keeps *which bearing* a real
/// decision for as long as possible. Past `TAPER_END` the width is HELD rather than driven
/// to zero — the prison corridor runs on.
///
/// ⚠️ **THIS IS THE BEND'S ANGLE TOO, NOT JUST THE SEA'S.** The first attempt at the taper
/// changed `sea_depth` alone and the world stopped generating: `radial_tf` still mapped
/// corridor `y` across the CONSTANT fan, so a section at d2800 scattered its creatures,
/// props and route waypoints across ±150° while only ±17° of it was land — ~88% of
/// everything placed there landed in the sea and the feasibility re-rolls burned against
/// water. The coast is not what decides where content goes; the bend is. Both ask this.
pub fn arc_half_at(d: f32, arc_half_rad: f32) -> f32 {
    if arc_half_rad <= 0.0 {
        return arc_half_rad;
    }
    let tapered = if d <= TAPER_START {
        arc_half_rad
    } else {
        let t = ((d - TAPER_START) / (TAPER_END - TAPER_START).max(1.0)).clamp(0.0, 1.0);
        let s = t * t * (3.0 - 2.0 * t);
        let end_half = (END_WIDTH * 0.5 / d.max(1.0)).min(arc_half_rad);
        arc_half_rad + (end_half - arc_half_rad) * s
    };
    // ── **AND THE COASTLINE WANDERS.** Measured before this, the ocean's edge sat at a
    // constant bearing — 150.000° at every radius from d200 to d1200, not approximately
    // straight but EXACTLY straight, because the fan term is `(theta - arc_half) * d` and
    // `arc_half` was a constant. Bays and isles bit discs out of it, so the coast read as a
    // ruler edge with circular scallops.
    //
    // Two harmonics of RADIUS, which is the same trick `regions::Grid::warp_at` uses on
    // bearing "so the boundary is a wandering line rather than a lobed flower" — and it is
    // safe for the same reason: it depends on ONE variable, so the field stays single-valued
    // and every "how wide is the world here" question still has one answer. A walk outward can
    // now cross a cove and come back to land, which is what a coastline does.
    //
    // Applied as a FRACTION of the local half-angle, never as a fixed angle: a constant
    // angular wobble is metres at the hub and kilometres at the frontier — the lesson
    // `sea_depth` already carries about `STRAIT_FAN_MARGIN`. And bounded well under 1.0, so
    // the wander can never pinch the fan shut or push it past its own rim.
    // ⚠️ **THE WANDER ONLY EVER BITES IN.** A two-sided wobble pushed the rim PAST the fan's
    // nominal 150° — measured, to 158.9° at d800 — and the 60° behind the fan is not spare
    // ground: it is the gap the Center Hub's peninsula and the west-return border live in. So
    // the nominal arc is the world's MAXIMUM extent and the sea bites inward from it: coves
    // cut in, headlands reach the design edge, and nothing the taper or the city relies on
    // can be pushed over.
    let w = d / COAST_WANDER_WAVELENGTH;
    let harmonic = 0.63 * w.sin() + 0.37 * (w * 2.7 + 1.9).cos();
    let bite = COAST_WANDER * 0.5 * (1.0 + harmonic.clamp(-1.0, 1.0));
    tapered * (1.0 - bite)
}

pub fn sea_depth(x: f32, z: f32, arc_half_rad: f32) -> f32 {
    if arc_half_rad <= 0.0 {
        return -1000.0; // corridor mode: no gap, no sea
    }
    let d = x.hypot(z);
    let theta = z.atan2(x).abs();
    let past_fan = (theta - arc_half_at(d, arc_half_rad)) * d;
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
/// **A BRIDGE** — `[x0, z0, x1, z1, half_width]`, a capsule of forced LAND spanning water.
///
/// It is the same mechanism as a strait's isthmus, localised and made visible: the water is
/// simply not there along the span. That choice is the whole design. `is_land` stays a **pure
/// function of position**, so `astar_route`, `apply_move`, `backbone_feasible` and the ground
/// shader all understand a bridge with no code of their own.
///
/// ⚠️ **A "walkable over water" special case would be the first crack in that rule**, and four
/// systems depend on it. Do not add one. A bridge is land; what makes it read as a bridge is
/// the DECK sitting above the waterline with sea still drawn beneath its parapets, which is a
/// question for the heightfield and the shader, not for the shoreline.
pub type Bridge = [f32; 5];

/// Bridges the ground shader carries at once (two `vec4`s each in the uniform).
pub const MAX_BRIDGES: usize = 8;

/// How much of a bridge's own half-width is solid deck before the water resumes. Below 1.0 so
/// the span reads as a ribbon over water rather than a land isthmus with a road painted on it.
pub const BRIDGE_DECK_SHARE: f32 = 1.0;

/// Distance from `(x, z)` to a bridge's span, negative INSIDE it. The signed field the
/// shoreline subtracts, so a bridge is land by the same arithmetic an isthmus is.
pub fn bridge_clearance(x: f32, z: f32, bridges: &[Bridge]) -> f32 {
    let mut best = f32::MAX;
    for b in bridges {
        let hw = b[4] * BRIDGE_DECK_SHARE;
        if hw <= 0.0 {
            continue;
        }
        let d = dist_to_segment(x, z, b[0], b[1], b[2], b[3]);
        best = best.min(d - hw);
    }
    best
}

/// Distance from `(x, z)` to the nearest point of a segment. Mirrors
/// [`crate::terrain::dist_to_segment`]; kept here so `coast` does not depend on `terrain` for
/// one line of arithmetic.
pub fn dist_to_segment_pub(x: f32, z: f32, x0: f32, z0: f32, x1: f32, z1: f32) -> f32 {
    dist_to_segment(x, z, x0, z0, x1, z1)
}

fn dist_to_segment(x: f32, z: f32, x0: f32, z0: f32, x1: f32, z1: f32) -> f32 {
    let (dx, dz) = (x1 - x0, z1 - z0);
    let len2 = dx * dx + dz * dz;
    let t = if len2 <= 1e-6 {
        0.0
    } else {
        (((x - x0) * dx + (z - z0) * dz) / len2).clamp(0.0, 1.0)
    };
    let (px, pz) = (x0 + dx * t, z0 + dz * t);
    ((x - px) * (x - px) + (z - pz) * (z - pz)).sqrt()
}

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
pub fn ang_diff(a: f32, b: f32) -> f32 {
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

// ---------------------------------------------------------------------------------------
// BAYS AND ISLANDS
// ---------------------------------------------------------------------------------------
//
// The [`Strait`] above gave the world INTERIOR structure. These two give its edge a shape:
// a **bay** bites inward from the fan's rim, and an **isle** stands out in the open sea.
//
// **They are ONE primitive**, and that is not a shortcut — a disc that edits the
// shoreline is exactly what both of them are, and they differ only in which side of the
// waterline the disc adds to. Keeping them as one type means one wire field, one uniform
// array, and one fold; two near-identical lists is how the pond/bog-pool/frozen-pond rule
// ended up written three times and one edit from disagreeing (see [`is_water_kind`]).
//
// A bay is the cheaper half of `WG-7`'s "regions with shapes": it is **convex**, so routing
// around one costs `πr / 2r` = **π/2 ≈ 1.57x** the straight line — bounded, where a ridge or
// a river can force an arbitrarily long detour. And it cannot sever the world, because a
// bay's reach inward is bounded to a share of the local half-arc, so there is always land
// between it and the fan's centre line ([`BAY_LAND_SHARE`], the same discipline
// [`CHANNEL_LAND_SHARE`] uses to guarantee the channel beside the city's spit).
//
// An isle is honestly **scenery**: it stands in the western gap, which no one can walk to,
// so nothing is ever placed on one. It is here because an ocean with nothing in it reads as
// a backdrop, and because it costs one term in a function that is already being evaluated.

/// One LOBE of the coastline: a disc that edits where the water is.
/// `[cx, cz, radius, kind]` — world-space centre, world-unit radius, and
/// [`LOBE_BAY`] (water) or [`LOBE_ISLE`] (land).
///
/// The 4th component carries the kind because a disc leaves it free, and because the
/// alternative — two arrays, two counts, two lots of uniform padding — costs more on both
/// sides of the wire than one tagged list. A radius of zero is an empty slot.
///
/// Lobes apply **in order**, so an isle listed after a bay stands *in* that bay. That falls
/// out of the fold rather than needing a rule.
pub type Lobe = [f32; 4];

/// A [`Lobe`] that is WATER: a bay or gulf, biting inward from the fan's rim.
pub const LOBE_BAY: f32 = 0.0;

/// A [`Lobe`] that is LAND: an isle standing in the sea.
pub const LOBE_ISLE: f32 = 1.0;

/// Max lobes a ground shader blends at once, windowed around the player like the straits.
pub const MAX_LOBES: usize = 12;

/// The most of the local half-arc a bay may eat, leaving the rest as land. **This is what
/// makes a bay unable to sever the world** — there is always ground between it and the
/// fan's centre line, at every radius, whatever the arc is retuned to. Same guarantee, and
/// the same reasoning, as [`CHANNEL_LAND_SHARE`].
pub const BAY_LAND_SHARE: f32 = 0.42;

// ---------------------------------------------------------------------------------------
// INLAND WATER
// ---------------------------------------------------------------------------------------
//
// Lakes, ponds, bogs, marshes, lagoons, creeks, springs, oases, rivers. **Nine names, two
// mechanisms** — and collapsing them is the whole design, the same discipline CANON D21
// demands of `Structure` ("do not build towns, anchors, portals and camps as separate
// systems"). Nine systems is nine subtly different answers to "where is the water".
//
// | mechanism | what it is | the names that fall out of it |
// |---|---|---|
// | [`Basin`] | standing water filling a hollow to a LEVEL | pond, lake, bog, marsh, lagoon, oasis |
// | [`RiverNode`] | flowing water descending the terrain | spring, creek, river |
//
// **The names are emergent, not authored, and that is why there is no `kind` field.**
//
// * **size** separates a pond from a lake — one radius draw.
// * **SLOPE separates a bog from a lake**, for free, because a basin is filled to a
//   CONTOUR rather than drawn as a circle: the same `level` over near-flat ground floods a
//   wide ragged sheet and over rolling ground makes a small round pool. Marshes come out
//   looking like marshes because the terrain under them is flat, not because anything asked
//   for a marsh.
// * **biome** names a bog a bog, an oasis an oasis, and an ice tarn an ice tarn — and it
//   already does, because the client picks its water tile from the section's biome
//   (`water_tile`), exactly as it does for the sea.
// * **adjacency** makes a lagoon: a basin whose contour reaches the sea.
//
// # The one thing that is genuinely new: water has a LEVEL
//
// The ocean works as a single scalar field because sea level is globally zero, so
// [`sea_depth`] can drive both the ground's dip and the water's tint. **An inland body sits
// at its own elevation**, and that is the difference between this and the bays above.
//
// It also makes it CHEAPER, not dearer: a basin needs no ground displacement at all,
// because the hollow is already in the heightmap — that is what makes it a basin. The
// terrain is the lakebed, and the water is a tint whose depth is `level - height`. So
// inland water deliberately does **not** join [`Shore::sea`] (which the vertex stage dips
// toward the sea floor); it joins [`Shore::water`], which is what collision and tinting
// ask. Folding it into the displacement field would dig every lake a second time, below
// its own bed.
//
// # And the laws hold BY CONSTRUCTION
//
// `terrain::height` is a pure analytic function, so a river is gradient descent on it: it
// runs downhill because downhill is the only direction it is generated in. It ends at the
// sea, or in a hollow it cannot climb out of — and a hollow a river cannot leave **is** a
// lake, so the endorheic case builds its own terminus rather than needing a rule. "Rivers
// flow to the ocean" is therefore not a check that can fail; it is the generator's only
// move.

/// A BASIN: standing water filling a hollow up to a level.
/// `[cx, cz, radius, level]` — world-space centre, a world-unit bound on how far it may
/// spread, and the **water surface elevation** in the same units as `terrain::height`.
///
/// The radius is a *bound*, not the shape. The shape is the terrain's own contour at
/// `level`, which is what makes a lake look like a lake instead of a disc — and what makes
/// a bog spread wide over flat ground with no extra machinery.
pub type Basin = [f32; 4];

/// One node of a river's chain. `[x, z, half_width, chain_start]`.
///
/// A river is a polyline, so it is the one water body that cannot be four floats. Nodes are
/// stored consecutively and a node with `chain_start >= 0.5` **begins a new chain** — which
/// is how a **FORD** is expressed: the gap between two chains is dry ground, a stony
/// crossing. That matters structurally, because connectedness is what a river *is*, and a
/// connected impassable line is exactly what disconnects a world. Fords are therefore
/// placed by the generator at a fixed cadence — a guarantee, like a strait's isthmus, never
/// a repair afterwards.
pub type RiverNode = [f32; 4];

/// Max basins a ground shader blends at once, windowed around the player.
///
/// ⚠️ **A SWAMP NEEDS MORE OF THESE THAN A FIELD DOES, AND 10 WAS SIZED FOR A FIELD.**
/// Measured over five seeds streamed to d900 with the biome pinned: the mire generates
/// **65 basins** against a desert's 15 — it is the wettest biome in the game by design
/// (`biome_water_mult` 3.2), and its water is what does its mazing since its fill stopped
/// being pools. At 10 slots, at most 15% of a mire's water could be DRAWN while
/// `Shore::water` went on colliding against all of it: reported as "the mire I just went
/// through had almost no water", and it also means walking into an invisible pool.
///
/// **SIZED TO WHAT CAN BE ON SCREEN, MEASURED — not to what the world holds.** The shader
/// loops to the filled COUNT (`params.basin_count`) once per ground fragment, so a slot costs
/// 16 bytes of uniform (nothing, against a 64 KiB guaranteed binding) while a *filled* slot
/// costs a loop iteration per pixel. Over-sizing is therefore not free, and sizing to the
/// world's total (a mire generates 65) would be paying for basins nobody can see.
///
/// Measured over 120 viewpoints — five seeds x six biomes x four depths, counting basins
/// within `fog_end` of a point on the route — **the most ever in view at once is 11.** So 16
/// covers the worst case with headroom and no more. The old 10 was wrong by one.
pub const MAX_BASINS: usize = 16;

/// Max river nodes a ground shader holds at once, windowed around the player.
///
/// ⚠️ **THIS WAS THE ONE CAP ACTUALLY BELOW WHAT IS VISIBLE.** It read "a river of
/// `river_max_nodes` is a handful of these, so this is a few rivers' worth" — reasoning from
/// the size of ONE river rather than from how many can be in frame. Measured the same way as
/// [`MAX_BASINS`], over the same 120 viewpoints: **31 river nodes can be within `fog_end` at
/// once**, against 28 slots. So a river visibly stopped mid-flow while `Shore::water` went on
/// blocking the part that was no longer drawn — the same collide-with-the-invisible failure
/// the peaks had, on the one landform whose entire contract is that it is CONNECTED.
pub const MAX_RIVER_NODES: usize = 40;

/// Nominal shoreline slope, used to turn a basin's *vertical* margin (`level - height`)
/// into an approximate *horizontal* distance so it can share a `min` with the radius bound
/// and hand the ground shader a beach of sane width.
///
/// It is an approximation on purpose. The exact horizontal distance to the contour is
/// `(level - height) / |∇height|`, which blows up to infinity on flat ground — and flat
/// ground is precisely the bog case, where the shore really is enormously wide and a
/// renderer still needs a finite number.
pub const BASIN_SHORE_SLOPE: f32 = 0.12;

/// **How far INSIDE this basin `(x, z)` is** — positive in the water, negative on the land
/// around it. Needs the run's terrain offset, because a basin is defined against the
/// heightmap rather than against a shape.
///
/// Two terms, both brought into world units so the field stays continuous and the beach has
/// a gradient: inside the radius bound, and below the water level.
pub fn basin_depth(x: f32, z: f32, b: &Basin, ox: f32, oz: f32, peaks: &[[f32; 4]]) -> f32 {
    let (cx, cz, r, level) = (b[0], b[1], b[2], b[3]);
    if r <= 0.0 {
        return -1000.0; // an empty slot
    }
    let within = r - (x - cx).hypot(z - cz);
    // ⚠️ **THE GROUND HERE IS THE BASE FIELD *PLUS* THE PEAKS**, and leaving the domes out is
    // water flooding straight through a mountain. It did: an authored peak raises the ground
    // by `radius * PEAK_MAX_ASPECT * 0.9` ≈ 9.8 units while `basin_fill` is 3.5, so a summit
    // inside a lake's radius stands SIX UNITS ABOVE its surface — and was nonetheless reported
    // submerged, which displaced the summit chest and cost the peak the reward that is the only
    // reason to climb it (`authored_peaks_are_climbable_and_crowned`).
    //
    // With the domes counted, a hill standing in a lake is an ISLAND in the lake, which is what
    // it physically is. `terrain::peak_height` is the same sum the ground shader adds through
    // `peak_dome`, so both sides agree.
    let ground = crate::terrain::height(x, z, ox, oz) + crate::terrain::peak_height(x, z, peaks);
    let below = (level - ground) / BASIN_SHORE_SLOPE;
    within.min(below)
}

/// **How far INSIDE a river `(x, z)` is** — positive in the channel. The max over every
/// segment, where a segment joins two consecutive nodes and a node marked `chain_start`
/// begins a new chain instead (so the gap before it is a dry FORD).
pub fn river_depth(x: f32, z: f32, nodes: &[RiverNode]) -> f32 {
    let mut best = -1000.0f32;
    for w in nodes.windows(2) {
        let (a, b) = (w[0], w[1]);
        if b[3] >= 0.5 {
            continue; // a new chain starts here — the gap is the ford
        }
        let half = (a[2] + b[2]) * 0.5;
        if half <= 0.0 {
            continue;
        }
        // Distance from the point to the segment a→b.
        let (px, pz) = (x - a[0], z - a[1]);
        let (sx, sz) = (b[0] - a[0], b[1] - a[1]);
        let len2 = sx * sx + sz * sz;
        let t = if len2 > 1e-6 { ((px * sx + pz * sz) / len2).clamp(0.0, 1.0) } else { 0.0 };
        let d = (px - sx * t).hypot(pz - sz * t);
        best = best.max(half - d);
    }
    best
}

/// **The whole shoreline of one world**, gathered so it can be asked in one place.
///
/// It is a bundle rather than an ever-growing argument list: the call sites went from
/// `(x, z, arc_half)` to `+ straits` to `+ lobes` to `+ basins, rivers`, and every addition
/// meant revisiting all ~15 of them. They ask `shore.is_land(x, z)` now, so the next thing
/// the coastline learns costs one field here and nothing anywhere else.
#[derive(Clone, Copy, Default)]
pub struct Shore<'a> {
    /// Half the world's fan, in radians (`radial_arc_degrees.to_radians() * 0.5`). Zero is
    /// corridor mode: no fan, so no sea at all and every other field is inert.
    pub arc_half: f32,
    /// This run's terrain offset, because a [`Basin`] is defined against the heightmap
    /// rather than against a shape of its own.
    pub terrain_off: (f32, f32),
    /// The authored PEAKS, because they are part of the ground a basin fills against — a hill
    /// standing in a lake is an island, and leaving the domes out floods straight through a
    /// mountain (see [`basin_depth`]).
    pub peaks: &'a [[f32; 4]],
    /// The inland seas that separate the continents ([`Strait`]).
    pub straits: &'a [Strait],
    /// Bays cut into the rim and isles standing offshore ([`Lobe`]).
    pub lobes: &'a [Lobe],
    /// Standing inland water — lakes, ponds, bogs, marshes, lagoons, oases ([`Basin`]).
    pub basins: &'a [Basin],
    /// Flowing inland water — rivers and creeks, as chains of [`RiverNode`].
    pub rivers: &'a [RiverNode],
    /// **The BRIDGES** ([`Bridge`]) — spans of forced land over water.
    pub bridges: &'a [Bridge],
}

impl<'a> Shore<'a> {
    /// A shoreline with nothing but the ocean — the world as it was before continents.
    /// Handy for the City (whose own coast is [`city_sea_depth`]) and for corridor mode.
    pub fn bare(arc_half: f32) -> Self {
        Shore { arc_half, ..Default::default() }
    }

    /// **The SEA's signed depth** at `(x, z)` — the ocean, the straits that break the fan
    /// into continents, and the lobes that shape its edge. Negative on land, positive at
    /// sea, zero on the waterline.
    ///
    /// ⚠️ **Inland water is deliberately NOT in here**, and that is the load-bearing
    /// distinction of this module. This is the field the vertex stage dips the ground
    /// toward the sea floor over (`terrain::with_sea`), and sea level is globally zero. A
    /// [`Basin`] sits at its OWN elevation and needs no dip at all — the hollow is already
    /// in the heightmap, which is what makes it a basin. Folding it in here would excavate
    /// every lake a second time, below its own bed. Collision and tinting want
    /// [`Self::water`] instead.
    ///
    /// Order is the order the shapes were added to the world, and every term is a signed
    /// distance in world units, so the field stays CONTINUOUS and the beach ramp has a
    /// gradient to ramp over. That is the property this module keeps having to re-learn: a
    /// boolean wearing a float renders as a vertical wall of water with no beach in between.
    pub fn sea(&self, x: f32, z: f32) -> f32 {
        let mut d = sea_depth_with(x, z, self.arc_half, self.straits);
        if self.arc_half <= 0.0 {
            return d; // corridor mode: no fan, no sea, and so no coastline to shape
        }
        for l in self.lobes {
            let (cx, cz, r, kind) = (l[0], l[1], l[2], l[3]);
            if r <= 0.0 {
                continue; // an empty slot
            }
            // Positive inside the disc, negative outside — a circle's signed distance.
            let inside = r - (x - cx).hypot(z - cz);
            if kind < 0.5 {
                d = d.max(inside); // BAY: water, so it wins over whatever was land
            } else {
                d = d.min(-inside); // ISLE: land, so it wins over whatever was sea
            }
        }
        // A BRIDGE beats every water term, and it is applied last for exactly that reason: it
        // is the thing put there so a crossing exists, so nothing may drown it.
        let span = bridge_clearance(x, z, self.bridges);
        if span < 0.0 {
            d = d.min(span);
        }
        d
    }

    /// **Inland water's signed depth** at `(x, z)` — basins and rivers only, negative
    /// everywhere else. Positive in standing or flowing fresh water.
    pub fn inland(&self, x: f32, z: f32) -> f32 {
        if self.arc_half <= 0.0 {
            return -1000.0; // corridor mode has no water of any kind
        }
        let (ox, oz) = self.terrain_off;
        let mut d = river_depth(x, z, self.rivers);
        for b in self.basins {
            d = d.max(basin_depth(x, z, b, ox, oz, self.peaks));
        }
        d
    }

    /// **All water, salt and fresh** — the union of [`Self::sea`] and [`Self::inland`], and
    /// the field every "can I stand here" question is a sign test of.
    pub fn water(&self, x: f32, z: f32) -> f32 {
        self.sea(x, z).max(self.inland(x, z))
    }

    /// Is `(x, z)` water? The sign of [`Self::water`], so the water a player SEES and the
    /// water they COLLIDE with cannot disagree — the whole reason this module exists.
    pub fn is_ocean(&self, x: f32, z: f32) -> bool {
        self.water(x, z) > 0.0
    }

    /// Is `(x, z)` walkable ground as far as water is concerned? Named for the call sites,
    /// which read as "can I stand here".
    pub fn is_land(&self, x: f32, z: f32) -> bool {
        !self.is_ocean(x, z)
    }
}

/// Does this bay leave land between itself and the fan's centre line, at its own radius?
///
/// The contract, asked of the thing rather than assumed of the arithmetic that built it —
/// the same discipline as [`strait_is_crossable`]. A bay that fails this can pinch the fan
/// in two, and unlike a strait it carries no isthmus to cross at.
pub fn bay_leaves_a_shore(l: &Lobe, arc_half_rad: f32) -> bool {
    if l[3] >= 0.5 {
        return true; // an isle is land; it cannot cut anything off
    }
    let (r, radius) = (l[0].hypot(l[1]), l[2]);
    if radius <= 0.0 {
        return true;
    }
    // Half the arc available at this radius, and the share of it a bay may take.
    radius <= r * arc_half_rad * BAY_LAND_SHARE
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
        //
        // ⚠️ **"INSIDE THE FAN" MEANS INSIDE ITS OWN COAST AT THAT RADIUS**, not inside the
        // nominal arc. The world tapers and its coastline wanders (`arc_half_at`), so the
        // rim moves — sampling the full ±`ARC_HALF` and demanding land is demanding a ruler
        // edge, which is the defect the wander exists to remove.
        for i in 0..=100 {
            for r in [5.0_f32, 40.0, 200.0, 1200.0] {
                let local = arc_half_at(r, ARC_HALF);
                let th = -local + (i as f32 / 100.0) * 2.0 * local;
                let (x, z) = (r * th.cos(), r * th.sin());
                assert!(
                    !is_ocean(x, z, ARC_HALF),
                    "inside the fan's own coast must be land (r={r}, theta={:.1}°)",
                    th.to_degrees()
                );
            }
        }
        // …and not vacuous: past that coast, inside the NOMINAL arc, is sea. That is the
        // wander, and without it these two statements would describe the same set.
        let mut bitten = 0;
        for r in [200.0_f32, 600.0, 1200.0] {
            let local = arc_half_at(r, ARC_HALF);
            if local < ARC_HALF - 1e-4 {
                let th = (local + ARC_HALF) * 0.5;
                if is_ocean(r * th.cos(), r * th.sin(), ARC_HALF) {
                    bitten += 1;
                }
            }
        }
        assert!(bitten > 0, "the coast never bites inside the nominal arc — it is a ray again");
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

    // -----------------------------------------------------------------------------------
    // BAYS AND ISLANDS
    // -----------------------------------------------------------------------------------

    /// A bay sitting ON the fan's rim at r=800, biting inward.
    fn test_bay() -> Lobe {
        // ⚠️ **ON THE COAST, NOT ON THE NOMINAL RIM.** A bay is an edit to the SHORELINE, so
        // the fixture has to sit where the shoreline is — and the world's coast now wanders
        // inside `ARC_HALF` (`arc_half_at`). Placed on the nominal rim it sat offshore, and
        // its inward reach no longer touched land, so the test's own premise failed: exactly
        // the way generation places one.
        let r = 800.0_f32;
        let th = arc_half_at(r, ARC_HALF);
        [r * th.cos(), r * th.sin(), 90.0, LOBE_BAY]
    }

    /// **A bay eats into the fan and never through it.** Water at the rim where it sits,
    /// land still there between it and the centre line — which is the guarantee, because
    /// unlike a strait a bay carries no isthmus to cross at.
    #[test]
    fn a_bay_bites_into_the_fan_but_never_through_it() {
        let bay = test_bay();
        let lobes = [bay];
        let shore = Shore { arc_half: ARC_HALF, lobes: &lobes, ..Default::default() };

        // Its own centre, on the rim, is water.
        assert!(shore.is_ocean(bay[0], bay[1]), "a bay is water");
        // And it has genuinely bitten INWARD: a point just inside the rim, at the bay's
        // radius, is now sea where the bare fan called it land.
        // ⚠️ Measured from the coast's OWN angle at this radius, not the nominal rim: the
        // wander has already taken the ground between them, so a point 20 units inside
        // `ARC_HALF` is sea with or without the bay and would prove nothing.
        let inward_th = arc_half_at(800.0, ARC_HALF) - 20.0 / 800.0;
        let (ix, iz) = (800.0 * inward_th.cos(), 800.0 * inward_th.sin());
        assert!(shore.is_ocean(ix, iz), "the bay must reach inside the fan's rim");
        assert!(
            Shore::bare(ARC_HALF).is_land(ix, iz),
            "…and that point must be land without it, or this test proves nothing"
        );

        // But the centre line at the same radius is still dry, at every radius the bay
        // touches — there is always a way past it.
        for r in [720.0_f32, 800.0, 880.0] {
            assert!(
                shore.is_land(r, 0.0),
                "a bay must never reach the fan's centre line (r={r}) — that is a barrier \
                 with no way around and no isthmus to cross"
            );
        }
    }

    /// The contract, asked as a contract: a bay bounded to its share of the local half-arc
    /// leaves a shore; one that is not can pinch the fan in two.
    #[test]
    fn the_bay_contract_catches_one_that_would_pinch_the_fan() {
        assert!(bay_leaves_a_shore(&test_bay(), ARC_HALF));

        let mut greedy = test_bay();
        greedy[2] = 800.0 * ARC_HALF; // the whole half-arc
        assert!(
            !bay_leaves_a_shore(&greedy, ARC_HALF),
            "a bay spanning the entire half-arc cuts the fan in two"
        );

        // An isle is land and can never cut anything off, whatever its radius.
        let big_isle = [-400.0, 0.0, 9_000.0, LOBE_ISLE];
        assert!(bay_leaves_a_shore(&big_isle, ARC_HALF));
    }

    /// **An isle is land standing in water**, and the sea around it is still sea.
    #[test]
    fn an_isle_stands_in_the_open_sea() {
        // Out past the spit's tip and OFF the axis: open ocean in the bare world.
        //
        // Off-axis on purpose. Dead on the axis past the tip, `peninsula_half_width` is 0
        // and so `past_spit` is exactly 0 — the measure-zero waterline where a depth of 0.0
        // makes `is_ocean` false while the boolean `is_ocean` calls it sea. That line is
        // precisely what `the_depth_field_agrees_with_the_predicate` skips with its epsilon,
        // and putting a fixture on it tests the tie-break rather than the isle.
        let (cx, cz) = (-(PENINSULA_LENGTH + 120.0), 90.0);
        assert!(Shore::bare(ARC_HALF).is_ocean(cx, cz), "the fixture must start as sea");

        let lobes = [[cx, cz, 40.0, LOBE_ISLE]];
        let shore = Shore { arc_half: ARC_HALF, lobes: &lobes, ..Default::default() };
        assert!(shore.is_land(cx, cz), "an isle is dry land");
        assert!(shore.is_land(cx + 30.0, cz), "…across its whole width");
        assert!(shore.is_ocean(cx + 60.0, cz), "and the sea closes again past its shore");
    }

    /// Lobes apply IN ORDER, so an isle listed after a bay stands in that bay. This falls
    /// out of the `max`/`min` fold rather than needing a rule of its own.
    #[test]
    fn an_isle_listed_after_a_bay_stands_in_it() {
        let bay = test_bay();
        let isle = [bay[0], bay[1], 25.0, LOBE_ISLE];
        let lobes = [bay, isle];
        let shore = Shore { arc_half: ARC_HALF, lobes: &lobes, ..Default::default() };
        assert!(shore.is_land(bay[0], bay[1]), "the isle wins where it overlaps the bay");
        // …and the bay is still water just outside the isle.
        let out = 40.0;
        let (ox, oz) = (bay[0] + out * ARC_HALF.cos(), bay[1] + out * ARC_HALF.sin());
        assert!(shore.is_ocean(ox, oz) || shore.is_land(ox, oz), "field is defined either way");
        assert!(shore.is_ocean(bay[0] - 45.0, bay[1]), "the bay survives beside its isle");
    }

    /// The bundle's sign is its own depth, over a world carrying every kind of shape at
    /// once — the same invariant `the_depth_field_agrees_with_the_predicate` holds for the
    /// bare ocean. A disagreement here is water you can walk on.
    #[test]
    fn the_whole_shoreline_agrees_with_itself() {
        let straits = [[600.0, 20.0, 0.0, 0.44, -0.24, 14.0, 0.22, 14.0]];
        let lobes = [
            test_bay(),
            [-(PENINSULA_LENGTH + 120.0), 0.0, 40.0, LOBE_ISLE],
            [500.0 * (-ARC_HALF).cos(), 500.0 * (-ARC_HALF).sin(), 60.0, LOBE_BAY],
        ];
        let shore = Shore { arc_half: ARC_HALF, straits: &straits, lobes: &lobes, ..Default::default() };
        let mut checked = 0u32;
        for xi in -70..=70 {
            for zi in -70..=70 {
                let (x, z) = (xi as f32 * 13.7, zi as f32 * 13.7);
                if x.hypot(z) < 1.0 {
                    continue;
                }
                let d = shore.water(x, z);
                if d.abs() > 1e-3 {
                    assert_eq!(d > 0.0, shore.is_ocean(x, z), "({x}, {z}): depth {d}");
                    checked += 1;
                }
            }
        }
        assert!(checked > 10_000, "the sweep covered almost nothing ({checked})");
    }

    /// A bay's and an isle's shores both get a BEACH, not a cliff — the field has to stay
    /// continuous across them, because the ground shader smoothsteps over its magnitude.
    /// This module has shipped the flat-field bug twice; a disc is easy to get right and
    /// easy to get wrong the same way.
    #[test]
    fn a_lobes_shoreline_has_a_beach_rather_than_a_cliff() {
        let lobes = [test_bay(), [-(PENINSULA_LENGTH + 120.0), 0.0, 40.0, LOBE_ISLE]];
        let shore = Shore { arc_half: ARC_HALF, lobes: &lobes, ..Default::default() };
        for l in &lobes {
            // March out through the lobe's own shore in 0.5-unit steps.
            let mut prev: Option<f32> = None;
            let mut t = -1.5 * l[2];
            while t <= 1.5 * l[2] {
                let d = shore.water(l[0] + t, l[1]);
                if let Some(p) = prev {
                    assert!(
                        (d - p).abs() < 2.0,
                        "the depth jumped {:.1} over a 0.5-unit step at t={t} on lobe \
                         {l:?} — that is a cliff of water, and every smoothstep over it \
                         collapses into a step",
                        (d - p).abs()
                    );
                }
                prev = Some(d);
                t += 0.5;
            }
        }
    }

    // -----------------------------------------------------------------------------------
    // INLAND WATER
    // -----------------------------------------------------------------------------------

    /// Find a hollow in the real height field near `(x0, z0)` by walking downhill, and
    /// return its centre and floor height. The generator does exactly this, so the tests
    /// exercise basins where the world would actually put them.
    fn hollow_near(x0: f32, z0: f32) -> (f32, f32, f32) {
        let (mut x, mut z) = (x0, z0);
        let mut h = crate::terrain::height(x, z, 0.0, 0.0);
        for _ in 0..400 {
            let e = 6.0;
            let dx = crate::terrain::height(x + e, z, 0.0, 0.0)
                - crate::terrain::height(x - e, z, 0.0, 0.0);
            let dz = crate::terrain::height(x, z + e, 0.0, 0.0)
                - crate::terrain::height(x, z - e, 0.0, 0.0);
            let m = (dx * dx + dz * dz).sqrt();
            if m < 1e-5 {
                break;
            }
            let (nx, nz) = (x - 8.0 * dx / m, z - 8.0 * dz / m);
            let nh = crate::terrain::height(nx, nz, 0.0, 0.0);
            if nh >= h {
                break; // a local minimum: this is where a lake forms
            }
            (x, z, h) = (nx, nz, nh);
        }
        (x, z, h)
    }

    /// **A basin fills to a CONTOUR, not a circle**, and that is the single thing that makes
    /// inland water look like water: its shoreline is the land's own shape. Proven by
    /// sampling a ring at ONE distance from the centre and finding both wet and dry points
    /// on it — impossible for a disc.
    #[test]
    fn a_basin_fills_to_a_contour_rather_than_a_circle() {
        let (cx, cz, floor) = hollow_near(900.0, 400.0);
        let basin = [cx, cz, 400.0, floor + 3.0];
        let basins = [basin];
        let shore = Shore { arc_half: ARC_HALF, basins: &basins, ..Default::default() };

        let (mut wet, mut dry) = (0, 0);
        for k in 0..180 {
            let a = k as f32 * std::f32::consts::TAU / 180.0;
            let (x, z) = (cx + 60.0 * a.cos(), cz + 60.0 * a.sin());
            if shore.inland(x, z) > 0.0 { wet += 1 } else { dry += 1 }
        }
        assert!(
            wet > 0 && dry > 0,
            "a ring at one radius came out all-{} — the basin is a disc, not a contour, so \
             every lake in the game is a circle",
            if dry == 0 { "wet" } else { "dry" }
        );
        // …and its own floor is under water.
        assert!(shore.inland(cx, cz) > 0.0, "the hollow's floor must be flooded");
    }

    /// **Slope is what separates a bog from a lake, for free.** The same water level over
    /// flatter ground floods a wider sheet. Nothing authored a marsh; the terrain did.
    #[test]
    fn the_same_level_floods_wider_over_flatter_ground() {
        // Measure flooded area for a fixed level offset at two hollows, and correlate it
        // with the local slope around each.
        let mut samples: Vec<(f32, f32)> = Vec::new(); // (mean slope, flooded fraction)
        for (sx, sz) in [(900.0, 400.0), (-1500.0, 700.0), (2300.0, -1100.0), (400.0, 2600.0)] {
            let (cx, cz, floor) = hollow_near(sx, sz);
            let basins = [[cx, cz, 500.0, floor + 4.0]];
            let shore = Shore { arc_half: ARC_HALF, basins: &basins, ..Default::default() };
            let (mut wet, mut total, mut slope_sum) = (0.0f32, 0.0f32, 0.0f32);
            for i in -30..=30 {
                for j in -30..=30 {
                    let (x, z) = (cx + i as f32 * 6.0, cz + j as f32 * 6.0);
                    total += 1.0;
                    if shore.inland(x, z) > 0.0 {
                        wet += 1.0;
                    }
                    slope_sum += crate::terrain::slope(x, z, 0.0, 0.0);
                }
            }
            samples.push((slope_sum / total, wet / total));
        }
        samples.sort_by(|a, b| a.0.total_cmp(&b.0));
        let (flat, steep) = (samples[0], samples[samples.len() - 1]);
        assert!(
            flat.1 > steep.1,
            "the flattest hollow (slope {:.3}) flooded {:.1}% and the steepest (slope {:.3}) \
             flooded {:.1}% — a bog is supposed to spread where a lake does not, and that \
             only happens if the fill follows a contour",
            flat.0,
            flat.1 * 100.0,
            steep.0,
            steep.1 * 100.0
        );
    }

    /// A river is water along its chain and **dry at its ford** — which is the guarantee,
    /// because connectedness is what a river is and a connected impassable line is exactly
    /// what disconnects a world.
    #[test]
    fn a_river_runs_wet_and_its_ford_is_dry() {
        // Two chains along +x with a gap between them: the gap is the ford.
        let rivers: Vec<RiverNode> = vec![
            [0.0, 0.0, 8.0, 1.0],
            [100.0, 0.0, 8.0, 0.0],
            [160.0, 0.0, 8.0, 1.0], // new chain — the 100..160 gap is the ford
            [260.0, 0.0, 8.0, 0.0],
        ];
        let shore = Shore { arc_half: ARC_HALF, rivers: &rivers, ..Default::default() };
        assert!(shore.inland(50.0, 0.0) > 0.0, "mid-chain is water");
        assert!(shore.inland(210.0, 0.0) > 0.0, "the second chain is water too");
        assert!(shore.inland(130.0, 0.0) < 0.0, "the FORD must be dry ground");
        assert!(shore.inland(50.0, 40.0) < 0.0, "and the bank beside it is dry");
    }

    /// ⚠️ **The load-bearing distinction: inland water is NOT in the sea field.** `sea` is
    /// what the vertex stage dips the ground toward the sea floor over, and sea level is
    /// globally zero — a lake sits at its own elevation and its hollow is already in the
    /// heightmap. Folding it in would excavate every lake a second time, below its own bed.
    #[test]
    fn a_lake_is_water_you_collide_with_and_not_ground_the_shader_digs() {
        let (cx, cz, floor) = hollow_near(900.0, 400.0);
        let basins = [[cx, cz, 400.0, floor + 3.0]];
        let rivers: Vec<RiverNode> = vec![[cx, cz, 9.0, 1.0], [cx + 90.0, cz, 9.0, 0.0]];
        let shore =
            Shore { arc_half: ARC_HALF, basins: &basins, rivers: &rivers, ..Default::default() };

        assert!(shore.inland(cx, cz) > 0.0, "the lake is inland water");
        assert!(shore.water(cx, cz) > 0.0, "…so it is water you collide with");
        assert!(shore.is_ocean(cx, cz), "…and `is_land` refuses it");
        assert!(
            shore.sea(cx, cz) < 0.0,
            "but the SEA field must still call it land, or the ground shader digs the lake \
             a second time below its own bed"
        );
    }

    /// Both inland shapes hand the shader a continuous field, so both get a beach. Same
    /// check as the ocean's and the lobes' — this module has shipped the flat-field bug twice.
    #[test]
    fn inland_shorelines_have_beaches_rather_than_cliffs() {
        let (cx, cz, floor) = hollow_near(900.0, 400.0);
        let basins = [[cx, cz, 400.0, floor + 3.0]];
        let rivers: Vec<RiverNode> = vec![[cx, cz - 300.0, 9.0, 1.0], [cx + 200.0, cz - 300.0, 9.0, 0.0]];
        let shore =
            Shore { arc_half: ARC_HALF, basins: &basins, rivers: &rivers, ..Default::default() };
        for (sx, sz, span) in [(cx, cz, 260.0f32), (cx + 100.0, cz - 300.0, 40.0)] {
            let mut prev: Option<f32> = None;
            let mut t = -span;
            while t <= span {
                let d = shore.inland(sx + t, sz);
                if let Some(p) = prev {
                    assert!(
                        (d - p).abs() < 6.0,
                        "inland depth jumped {:.1} over a 0.5-unit step at t={t} — that is a \
                         cliff of water, and every smoothstep over it collapses to a step",
                        (d - p).abs()
                    );
                }
                prev = Some(d);
                t += 0.5;
            }
        }
    }

    /// Corridor mode has no water of any kind, however much is handed in — the tutorial and
    /// every flat unit test stay exactly as they were.
    #[test]
    fn corridor_mode_has_no_inland_water() {
        let basins = [[100.0, 0.0, 300.0, 999.0]]; // a level above every hill
        let rivers: Vec<RiverNode> = vec![[0.0, 0.0, 20.0, 1.0], [200.0, 0.0, 20.0, 0.0]];
        let shore = Shore { arc_half: 0.0, basins: &basins, rivers: &rivers, ..Default::default() };
        assert!(shore.is_land(100.0, 0.0));
        assert!(shore.inland(100.0, 0.0) < 0.0);
    }

    /// Corridor mode has no fan, so it has no sea — and therefore no bays and no isles,
    /// however many are handed in. The tutorial and every flat unit test stay untouched.
    #[test]
    fn corridor_mode_has_no_coastline_to_shape() {
        let lobes = [test_bay(), [-300.0, 0.0, 40.0, LOBE_ISLE]];
        let shore = Shore { arc_half: 0.0, lobes: &lobes, ..Default::default() };
        assert!(shore.is_land(600.0, 0.0));
        assert!(shore.is_land(-300.0, 0.0));
        assert!(shore.water(600.0, 30.0) < 0.0);
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
mod taper_tests {
    use super::*;

    #[test]
    fn the_world_closes_to_a_corridor() {
        // The teardrop: full arc through the on-ramp and mid-world, then closing to a
        // fixed-width corridor at the end. Asserted as SHAPE — full, then widening, then
        // closing, then constant — because every number in it is a constant somebody will
        // retune.
        //
        // ⚠️ **TOLERANCED FOR THE COAST'S WANDER, and compared over WIDE gaps.** The
        // shoreline now meanders by up to `COAST_WANDER` (see `the_shoreline_is_never
        // _straight`), so "unchanged" and "monotone between adjacent radii" are both false by
        // design: adjacent samples can move either way while the trend closes. Asserting them
        // strictly was asserting that the coast is a ruler edge, which is the defect the
        // wander exists to fix.
        let ah = 2.618_f32;
        let width = |d: f32| 2.0 * arc_half_at(d, ah) * d;
        let nominal = |d: f32| 2.0 * ah * d;
        let tol = COAST_WANDER * 1.05;
        // Through the on-ramp and the mid-world the fan is full, up to the wander.
        for d in [100.0_f32, 500.0, 1000.0, TAPER_START] {
            let got = width(d) / nominal(d);
            assert!(
                got > 1.0 - tol && got <= 1.0 + 1e-4,
                "the fan must be full at d{d} up to the coast's wander — got {got:.3} of nominal"
            );
        }
        // It WIDENS before it closes, or it is a cone rather than a teardrop.
        assert!(
            width(1600.0) > width(TAPER_START) * (1.0 - tol),
            "the world still opens out past the taper's start"
        );
        // …and then closes. Compared over WIDE gaps so the trend, not the wander, is measured.
        let mut last = width(1600.0);
        for d in [2200.0_f32, 2800.0, TAPER_END] {
            let w = width(d);
            assert!(w < last, "the world must keep closing: d{d} is {w:.0} against {last:.0}");
            last = w;
        }
        // The end of the world is the corridor's width, to within the wander.
        for d in [TAPER_END, 3400.0, 4000.0] {
            let got = width(d) / END_WIDTH;
            assert!(
                (got - 1.0).abs() < tol + 1e-3,
                "past the end the corridor holds its width: {:.0} at d{d}, wanted {END_WIDTH}",
                width(d)
            );
        }
    }

    #[test]
    fn the_shoreline_is_never_straight() {
        // Measured before the wander: the ocean's edge sat at **150.000° at every radius**
        // from d200 to d1200 — not approximately straight, exactly straight, because the fan
        // term is `(theta - arc_half) * d` and `arc_half` was a constant. Bays and isles bit
        // discs out of it, so the coast read as a ruler edge with circular scallops.
        //
        // Asserted as a VARIANCE, never as a shape: a coastline that is straight anywhere is
        // one somebody drew with a ruler, and "not straight" is the property while any
        // particular wiggle is a tunable.
        let ah = 2.618_f32;
        let bearings: Vec<f32> = (2..=28).map(|k| arc_half_at(k as f32 * 50.0, ah)).collect();
        let n = bearings.len() as f32;
        let mean = bearings.iter().sum::<f32>() / n;
        let var = bearings.iter().map(|b| (b - mean) * (b - mean)).sum::<f32>() / n;
        assert!(
            var.sqrt() > 0.01,
            "the coast barely moves (sd {:.4} rad) — that is a ruler edge",
            var.sqrt()
        );
        // …and it must move by a distance a player would NOTICE, not by noise on the edge.
        let swing = bearings.iter().fold((f32::MAX, 0.0f32), |(l, h), b| (l.min(*b), h.max(*b)));
        let at_800 = (swing.1 - swing.0) * 800.0;
        assert!(
            at_800 > 40.0,
            "the coast wanders only {at_800:.0} world units at d800 — a cove has to be worth \
             rounding"
        );
        // ⚠️ **AND IT NEVER PUSHES PAST THE NOMINAL ARC.** The 60° behind the fan is not spare
        // ground — the hub's peninsula and the west-return border live there — so the design
        // arc is the world's MAXIMUM extent and the sea only ever bites in.
        for k in 1..=68 {
            let d = k as f32 * 50.0;
            assert!(
                arc_half_at(d, ah) <= ah + 1e-6,
                "at d{d} the coast reaches {:.3}° past the fan's own {:.3}°",
                arc_half_at(d, ah).to_degrees(),
                ah.to_degrees()
            );
        }
    }

    #[test]
    fn the_taper_matches_the_shader() {
        // The ground shader mirrors `arc_half_at` so the coastline it paints is the one the
        // server collides with. This repo has already shipped a water feature invisible
        // behind an unused WGSL function, and a bridge nobody rendered that parties walked
        // across — so the constants are READ OUT of the shader rather than trusted.
        let src = include_str!(
            "../../../client/crates/meld-client/assets/shaders/ground_biome.wgsl"
        );
        let grab = |name: &str| -> f32 {
            let at = src.find(name).unwrap_or_else(|| panic!("{name} missing from the shader"));
            let tail = &src[at + name.len()..];
            let lo = tail.find('=').expect("an assignment") + 1;
            let hi = tail.find(';').expect("a statement end");
            tail[lo..hi].trim().parse().unwrap_or_else(|e| panic!("{name}: {e}"))
        };
        assert_eq!(grab("let taper_start"), TAPER_START, "taper start drifted from the shader");
        assert_eq!(grab("let taper_end"), TAPER_END, "taper end drifted from the shader");
        assert_eq!(grab("let end_width"), END_WIDTH, "the corridor width drifted");
        assert_eq!(grab("let coast_wander"), COAST_WANDER, "the coast's wander drifted");
        assert_eq!(
            grab("let coast_wander_wavelength"),
            COAST_WANDER_WAVELENGTH,
            "the coast's wavelength drifted"
        );
    }
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

#[cfg(test)]
mod visible_cap_tests {
    /// **WHAT ARE THE SLOT COUNTS BASED ON?** Until this test, nothing — they were round
    /// numbers, and one of them (`MAX_RIVER_NODES`) was below what a player can actually see.
    ///
    /// The basis is: a slot count must cover **the most of that landform that can be within
    /// `fog_end` at once**, because past the fog nothing is drawn, and anything visible that
    /// does not fit a slot is terrain the client collides with and stands entities on while
    /// drawing flat ground over it. Sizing to the world's TOTAL is the other error: a mire
    /// generates 65 basins, and the shader loops to the filled count once per ground
    /// fragment, so uploading basins nobody can see is paid for per pixel.
    ///
    /// Measured with a world-gen harness over 120 viewpoints (5 seeds x 6 biomes x 4 depths,
    /// counting each landform within `fog_end` of a point on the route). The figures are
    /// recorded here rather than re-measured, because `meld-proto` must not depend on the
    /// generator — so this is a *ratchet*: if generation gets denser, re-run the sweep and
    /// raise both the figure and the cap together.
    #[test]
    fn every_slot_count_covers_what_can_be_on_screen() {
        // (landform, measured worst-in-view, slots)
        let measured: &[(&str, usize, usize)] = &[
            ("basins", 11, super::MAX_BASINS),
            ("river nodes", 31, super::MAX_RIVER_NODES),
            ("ridges", 13, crate::terrain::MAX_RIDGES),
            ("peaks", 8, crate::terrain::MAX_PEAKS),
        ];
        for (name, worst, slots) in measured {
            assert!(
                slots >= worst,
                "{name}: {worst} can be in view at once but only {slots} slots exist — the                  overflow is terrain the client collides with and never draws"
            );
            // …and not wildly over, since a filled slot costs a loop iteration per pixel.
            assert!(
                *slots <= worst * 3 + 4,
                "{name}: {slots} slots against a measured worst-in-view of {worst} is paying                  per-pixel for landforms nobody can see"
            );
        }
    }
}
