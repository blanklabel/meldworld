//! **A COMBATANT'S HEALTH, PAINTED ON THE GROUND IT IS STANDING ON.**
//!
//! A bar kept anywhere other than the body it belongs to makes "how is the Knight doing" a
//! question you answer by looking away from the Knight — and a party of four against a pack
//! of five puts nine readouts around the edges of a fight happening in the middle.
//!
//! A ring at the feet answers where the question is asked. It lies on the GROUND, so it reads
//! in perspective with the scene rather than floating in front of it, and it costs no screen
//! real estate at all: the bar is drawn on ground the body already occupies.
//!
//! ⚠️ **GREEN IS LIFE. THE SIDE IS A HAIRLINE.** Health is read as LIQUID — a green pool, the
//! red bed it drains back over, a dark vessel under both — because that is the one reading
//! nobody has to be taught. Which side a body is on rides the ring's outer rim only, and it
//! can afford to: the body itself is standing in the middle of the ring.
//!
//! ⚠️ **NO ATB GAUGE LIVES HERE, AND THAT IS DELIBERATE.** A per-body gauge answers "who is
//! next" nine times, once per fighter, and leaves the player to compare them; the turn-order
//! bar answers it once by putting every fighter on one track in order. Two answers to one
//! question is one answer too many, and this is the one that loses.

use bevy::asset::Asset;
use bevy::prelude::*;
use bevy::reflect::TypePath;
use bevy::render::render_resource::AsBindGroup;
use bevy::shader::ShaderRef;

use crate::battle::SpriteQuad;
use crate::{status_num, BattleData};
use meld_client::net::CombatantView;

/// How wide the ring is, in world units. Sized to sit just outside the contact shadow so the
/// two read as one footprint rather than as a disc with a hoop around it.
const RING_SCALE: f32 = 1.35;
/// How fast the red bed a hit opened closes back up, as a fraction of the ring per second.
const GHOST_CHASE: f32 = 0.28;
/// How long it stays open at full width first, in seconds.
///
/// ⚠️ **WITHOUT THE HOLD THERE IS NOTHING TO SEE.** A bed that starts closing on the frame it
/// opened shows a blow worth a fifth of a bar for about a third of a second, on a ring the
/// player is not yet looking at — measured, most captures missed it entirely, and so would an
/// eye that was on the creature when the blow landed. The hold is what makes the ring slide
/// from green to red and then drain, rather than flickering.
const GHOST_HOLD: f32 = 0.42;
/// How fast the heal/hit flashes fade, per second. Long enough to catch out of the corner of
/// an eye in a four-body fight, short enough to be gone before the next blow lands.
const PULSE_FADE: f32 = 2.4;
/// How fast the FLOW settles once the level stops moving. Slower than the flashes: the surface
/// keeps running for a moment after the number lands, which is what a liquid does and what a
/// bar that simply snapped to its new value does not.
const FLOW_FADE: f32 = 1.6;
/// How fast the shown level rolls toward the real one, as a fraction of the bar per second,
/// and the least it will ever move in one second.
///
/// ⚠️ **THE NUMBER ROLLS; IT DOES NOT SNAP** (EarthBound's meter). A bar and a number that
/// jump have already finished telling you what happened by the time you look at them — the
/// counting IS the readout, and it is what makes a big hit feel big and a scratch feel like a
/// scratch. The floor is what keeps a one-point tick from being instant.
const ROLL_RATE: f32 = 0.55;
const ROLL_FLOOR: f32 = 0.08;

/// The stroke's inner and outer radius as a fraction of the mesh — mirrored from
/// `feet_ring.wgsl`, which cuts the annulus, and held against it by test. The Rust side needs
/// them because the HP number is written ALONG the middle of that stroke.
pub(crate) const R_IN: f32 = 0.60;
pub(crate) const R_OUT: f32 = 0.96;
/// `WorldAssets::shadow_mesh` is a `Circle::new(0.7)`, and the ring borrows it rather than
/// carrying a second disc.
const MESH_RADIUS: f32 = 0.7;
/// World-space radius of the middle of the stroke: the line the digits sit on.
pub(crate) const TEXT_RADIUS: f32 = MESH_RADIUS * RING_SCALE * (R_IN + R_OUT) * 0.5;

/// The ring material. One per combatant, driven every frame it changes.
#[derive(Asset, AsBindGroup, TypePath, Debug, Clone)]
pub(crate) struct FeetRing {
    /// `(fill, ghost, seconds, active)` — see `feet_ring.wgsl`.
    #[uniform(100)]
    pub(crate) params: Vec4,
    /// The SIDE this body is on, worn as the outer rim. Health is never read from it: green is
    /// life and red is what life cost, for everybody, because that reading needs no key.
    #[uniform(100)]
    pub(crate) tint: Vec4,
    /// `(heal, hit, _, _)` — the two momentary flashes, each fading to nothing.
    #[uniform(100)]
    pub(crate) pulse: Vec4,
}

impl Material for FeetRing {
    fn fragment_shader() -> ShaderRef {
        "shaders/feet_ring.wgsl".into()
    }
    /// Alpha-blended, not additive: this is paint on the ground, not light cast onto it.
    fn alpha_mode(&self) -> AlphaMode {
        AlphaMode::Blend
    }
}

/// Marks the ring under one combatant.
#[derive(Component)]
pub(crate) struct CombatantRing {
    pub(crate) id: String,
    pub(crate) mat: Handle<FeetRing>,
    /// The fraction the red bed is currently draining from, chasing the real level.
    pub(crate) ghost: f32,
    /// Last frame's level, which is the only way to tell a heal from a hit: the wire carries
    /// HP, never the event that changed it.
    pub(crate) last: f32,
    /// The two flashes, 1 at the moment it landed and fading from there.
    pub(crate) heal: f32,
    pub(crate) hit: f32,
    /// What is left of the beat the red bed stays open for before it starts closing.
    pub(crate) hold: f32,
    /// Which way the level is going and how hard: **positive while draining**, negative while
    /// filling, decaying to nothing once it settles. It is what the surface waves ride.
    pub(crate) flow: f32,
    /// The level being SHOWN, rolling toward the real one. A second blow during the roll only
    /// moves the target, so the meter carries straight on from wherever it had got to rather
    /// than restarting — which is the whole reason the roll is a state and not an animation.
    pub(crate) shown: f32,
}

/// The ring's RIM colour for a combatant: the same side reading the turn-order bar uses, so a
/// body's ring and its icon on the bar are obviously the same fighter. It never touches the
/// level itself — see the module note.
pub(crate) fn ring_color(c: &CombatantView, mine: bool) -> Color {
    if !c.is_player {
        Color::srgb(0.95, 0.42, 0.36)
    } else if mine {
        Color::srgb(0.40, 0.82, 1.0)
    } else {
        Color::srgb(0.55, 0.95, 0.65)
    }
}

/// The Barrier a combatant is holding, as a fraction of its own max HP. It is TEMP HP — a
/// pool that absorbs before health does — so it belongs on the health readout rather than in
/// an icon somewhere else, and reading it against max HP is what makes "how much does this
/// buy me" answerable at a glance instead of as a bare number.
pub(crate) fn barrier_fill(c: &CombatantView) -> f32 {
    (status_num(&c.statuses, "barrier:") as f32 / c.max_hp.max(1) as f32).clamp(0.0, 1.0)
}

/// A combatant's health as a fraction, clamped — the one number the ring draws.
pub(crate) fn hp_fill(c: &CombatantView) -> f32 {
    (c.hp as f32 / c.max_hp.max(1) as f32).clamp(0.0, 1.0)
}

/// The HP a rolling ring is currently SHOWING, as a whole number. It is what the digits in the
/// stroke read, so the number and the bar can never disagree about how far through a roll they
/// are — and a body still standing never reads 0 mid-roll, since the last point is exactly the
/// one the player is watching.
pub(crate) fn shown_hp(shown: f32, c: &CombatantView) -> i32 {
    let v = (shown * c.max_hp as f32).round() as i32;
    if c.hp > 0 { v.max(1) } else { v.max(0) }
}

/// Close the red bed toward the pool standing in it, after holding it open. Returns the new
/// bed level and what is left of the hold.
///
/// It only ever closes DOWNWARD: a heal shows up at once (the pool simply grows over the bed),
/// because a level that eases upward reads as the heal still arriving when it has landed.
pub(crate) fn chase_ghost(ghost: f32, fill: f32, hold: f32, dt: f32) -> (f32, f32) {
    if ghost <= fill {
        return (fill, 0.0);
    }
    let hold = (hold - dt).max(0.0);
    if hold > 0.0 {
        return (ghost, hold);
    }
    ((ghost - GHOST_CHASE * dt).max(fill), 0.0)
}

/// Fade a flash toward nothing. Its own function so the rate is one number rather than one per
/// call site, and so a pulse is provably gone rather than asymptotically nearly gone.
pub(crate) fn fade(pulse: f32, dt: f32) -> f32 {
    (pulse - PULSE_FADE * dt).max(0.0)
}

/// How far round the arc to step when measuring its scale — small enough that the chord is the
/// arc to within a pixel, large enough that the projection's own precision does not dominate.
const ARC_EPS: f32 = 0.02;

/// How many screen pixels one radian of the ring's front arc is worth, given a projection from
/// an angle (radians from the front) to a screen point.
///
/// ⚠️ **MEASURED AT THE FRONT, WHERE THE TANGENT IS THE UNFORESHORTENED AXIS.** The ring lies
/// on the ground, so its projection is an ELLIPSE: the radial direction at the front is the
/// squashed one and the tangent is not. Sizing the digits off the squashed axis would shrink
/// them for no reason other than the camera's pitch.
pub(crate) fn arc_scale(project: impl Fn(f32) -> Option<Vec2>) -> Option<f32> {
    let a = project(0.0)?;
    let b = project(ARC_EPS)?;
    let px = a.distance(b) / ARC_EPS;
    (px > 1.0).then_some(px)
}

/// Lay `n` glyphs along the ring's front arc, each `advance` pixels from the last, centred on
/// the point nearest the camera. Returns a screen position and a clockwise rotation per glyph.
///
/// ⚠️ **THE ROTATION IS TAKEN FROM THE PROJECTION, NOT FROM THE ANGLE.** The ring is an ellipse
/// on screen, so the tangent at a given angle is not that angle — computing it by hand would
/// have the number lying flat on the ground at the front and leaning the wrong way at the
/// sides. Sampling either side of each glyph gets it right for any camera.
pub(crate) fn arc_text(
    n: usize,
    advance: f32,
    project: impl Fn(f32) -> Option<Vec2>,
) -> Vec<(Vec2, f32)> {
    let Some(px_per_rad) = arc_scale(&project) else { return Vec::new() };
    // Which way round the ring is LEFT TO RIGHT on screen. Getting this wrong writes the
    // number backwards, and it flips as the camera orbits past the body.
    let reading = match (project(0.0), project(ARC_EPS)) {
        (Some(a), Some(b)) if b.x < a.x => -1.0,
        _ => 1.0,
    };
    let step = advance / px_per_rad * reading;
    let mid = (n as f32 - 1.0) * 0.5;
    (0..n)
        .filter_map(|i| {
            let theta = (i as f32 - mid) * step;
            let at = project(theta)?;
            let back = project(theta - ARC_EPS * reading)?;
            let ahead = project(theta + ARC_EPS * reading)?;
            let d = ahead - back;
            Some((at, d.y.atan2(d.x)))
        })
        .collect()
}

/// Spawn the ring under a combatant, as a child of its actor — the two actor spawners already
/// place a contact shadow at exactly this spot.
pub(crate) fn spawn_ring(
    parent: &mut ChildSpawnerCommands,
    mesh: Handle<Mesh>,
    rings: &mut Assets<FeetRing>,
    id: &str,
    col: Color,
    fill: f32,
) {
    let c = col.to_linear();
    let mat = rings.add(FeetRing {
        params: Vec4::new(fill, fill, 0.0, 0.0),
        tint: Vec4::new(c.red, c.green, c.blue, 1.0),
        pulse: Vec4::ZERO,
    });
    parent.spawn((
        CombatantRing {
            id: id.to_string(),
            mat: mat.clone(),
            ghost: fill,
            last: fill,
            heal: 0.0,
            hit: 0.0,
            hold: 0.0,
            flow: 0.0,
            shown: fill,
        },
        Mesh3d(mesh),
        MeshMaterial3d(mat),
        // Flat on the ground, a hair above it so it does not z-fight the terrain and a hair
        // above the contact shadow so the two do not fight each other either.
        Transform::from_xyz(0.0, 0.03, 0.0)
            .with_rotation(Quat::from_rotation_x(-std::f32::consts::FRAC_PI_2))
            .with_scale(Vec3::splat(RING_SCALE)),
    ));
}

/// **A RING FOLLOWS THE BODY STANDING IN IT.**
///
/// The lunge, the recoil and the impact shake are written onto the sprite BILLBOARD, which is
/// the actor root's child — and the ring is that billboard's SIBLING, so a hero that steps
/// forward to swing steps out of its own health bar and stands back in it a moment later.
///
/// ⚠️ **IT COPIES THE SPRITE RATHER THAN RE-DERIVING THE MOTION.** Reproducing the lunge
/// arithmetic here would be a second copy of a rule that already moved once this session, and
/// the two would drift the first time either is retuned — the `blocking_field` argument, one
/// crate over. Reading the sprite's own offset means a motion added later is followed for
/// free.
///
/// ⚠️ **AND ONLY THE HORIZONTAL HALF.** The ring lies ON the ground: lifting it with a body
/// that swells or bobs would peel it off the floor it is painted on, which is the one thing
/// that makes it read in perspective with the scene.
pub(crate) fn ring_follows_body(
    sprites: Query<(&SpriteQuad, &Transform), Without<CombatantRing>>,
    mut rings: Query<(&CombatantRing, &mut Transform)>,
) {
    for (ring, mut tf) in &mut rings {
        let Some((_, body)) = sprites.iter().find(|(s, _)| s.id == ring.id) else { continue };
        let (x, z) = (body.translation.x, body.translation.z);
        // A `DerefMut` write is a re-propagation whether or not the value moved, and most
        // bodies are standing still on most frames.
        if tf.translation.x != x || tf.translation.z != z {
            tf.translation.x = x;
            tf.translation.z = z;
        }
    }
}

/// Drive every ring: its fill, the ghost trailing a hit, and whether this body is the one
/// being asked for an order.
pub(crate) fn drive_rings(
    time: Res<Time>,
    battle: Res<BattleData>,
    mut rings: ResMut<Assets<FeetRing>>,
    mut q: Query<(&mut CombatantRing, &mut Visibility)>,
    // Who was being asked last frame. The command wheel pushing this body's circle out is the
    // same event as the turn arriving on it, so the slosh is triggered from the turn rather
    // than plumbed across from the wheel — one fact, read where it already is.
    mut last_active: Local<Option<String>>,
) {
    let t = time.elapsed_secs();
    let dt = time.delta_secs();
    let shoved = (battle.active != *last_active).then(|| battle.active.clone()).flatten();
    if battle.active != *last_active {
        *last_active = battle.active.clone();
    }
    for (mut ring, mut vis) in &mut q {
        let Some(c) = battle.view(&ring.id) else { continue };
        // A body that is down has no health to report and no turn to take.
        let want = if c.hp > 0 { Visibility::Inherited } else { Visibility::Hidden };
        if *vis != want {
            *vis = want;
        }
        let target = hp_fill(c);
        // A HEAL AND A HIT ARE THE SAME WIRE FIELD MOVING IN OPPOSITE DIRECTIONS. Nothing on
        // `CombatantView` says which one happened, so the level's own direction is the event.
        if target > ring.last + 0.001 {
            ring.heal = 1.0;
        } else if target < ring.last - 0.001 {
            ring.hit = 1.0;
            ring.hold = GHOST_HOLD;
        }
        ring.last = target;
        // …and the SHOWN level rolls toward it. Everything below reads `fill`, so the bar, the
        // bed, the waves and the number are all one moving quantity rather than four things
        // that have to be kept in step.
        let step = (ROLL_RATE * (target - ring.shown).abs()).max(ROLL_FLOOR) * dt;
        ring.shown = if (target - ring.shown).abs() <= step {
            target
        } else {
            ring.shown + (target - ring.shown).signum() * step
        };
        let fill = ring.shown;
        ring.heal = fade(ring.heal, dt);
        ring.hit = fade(ring.hit, dt);
        // The surface runs while the level does. A shove from the command wheel opening counts
        // too: the ring is physically pushed then, and a pool that ignored that would be the
        // one moment the bar stopped behaving like liquid.
        // The surface runs while the SHOWN level is still moving — which is the whole roll,
        // not just the frame the wire changed on.
        if fill > target + 0.0005 || shoved.as_deref() == Some(ring.id.as_str()) {
            ring.flow = 1.0;
        } else if fill < target - 0.0005 {
            ring.flow = -1.0;
        }
        ring.flow -= ring.flow.signum() * FLOW_FADE * dt;
        if ring.flow.abs() < 0.02 {
            ring.flow = 0.0;
        }
        let (ghost, hold) = chase_ghost(ring.ghost, fill, ring.hold, dt);
        ring.ghost = ghost;
        ring.hold = hold;
        let active = battle.active.as_deref() == Some(ring.id.as_str());
        let Some(mut mat) = rings.get_mut(&ring.mat) else { continue };
        mat.params = Vec4::new(fill, ring.ghost, t, if active { 1.0 } else { 0.0 });
        mat.pulse = Vec4::new(ring.heal, ring.hit, barrier_fill(c), ring.flow);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cv(id: &str, ally: bool, hp: i32, max: i32) -> CombatantView {
        CombatantView {
            id: id.into(),
            name: id.into(),
            hp,
            max_hp: max,
            gauge: 0.0,
            is_player: ally,
            player_id: ally.then(|| id.into()),
            level: 1,
            statuses: Vec::new(),
        }
    }

    /// **THE RIM'S COLOUR IS A SIDE, NEVER A MOOD.** The level is read from the liquid — green
    /// for life, red for what it cost — so the one channel left says whose body this is, and
    /// it must not move with the health.
    #[test]
    fn a_ring_says_whose_it_is_not_how_hurt() {
        let healthy = ring_color(&cv("h1", true, 100, 100), true);
        let dying = ring_color(&cv("h1", true, 1, 100), true);
        assert_eq!(healthy, dying, "the colour moved with the health");
        assert_ne!(
            ring_color(&cv("m1", false, 50, 100), false),
            healthy,
            "a creature and a hero must not share a ring colour"
        );
        assert_ne!(
            ring_color(&cv("co", true, 50, 100), false),
            healthy,
            "somebody else's hero must not look like one of mine"
        );
    }

    /// The fill is the health, clamped — a bad number from the wire cannot draw a ring past
    /// full or behind empty.
    #[test]
    fn the_fill_is_the_health_and_nothing_else() {
        assert_eq!(hp_fill(&cv("a", true, 50, 100)), 0.5);
        assert_eq!(hp_fill(&cv("a", true, 0, 100)), 0.0);
        assert_eq!(hp_fill(&cv("a", true, 999, 100)), 1.0, "a ring cannot overfill");
        assert_eq!(hp_fill(&cv("a", true, 10, 0)), 1.0, "a zero max must not divide by zero");
    }

    /// **THE GHOST CHASES DOWN AND SNAPS UP.** A hit leaves a trail you can read; a heal
    /// should simply be there, because a bar that eases upward reads as the heal still
    /// arriving when it has already landed.
    #[test]
    fn the_ghost_trails_damage_but_never_lags_a_heal() {
        // It HOLDS first — a bed that starts closing on the frame it opened is a flicker.
        let (held, left) = chase_ghost(1.0, 0.4, GHOST_HOLD, 1.0 / 60.0);
        assert_eq!(held, 1.0, "the bed closed during its own hold");
        assert!(left > 0.0 && left < GHOST_HOLD, "the hold is not running down: {left}");
        // …then it closes.
        let (after, _) = chase_ghost(1.0, 0.4, 0.0, 1.0 / 60.0);
        assert!(after < 1.0 && after > 0.4, "the bed must close gradually: {after}");
        assert_eq!(chase_ghost(0.4, 0.9, 0.0, 1.0 / 60.0).0, 0.9, "a heal lands at once");
        // A heal also cancels a hold, or the bed hangs open over a pool that already covered it.
        assert_eq!(chase_ghost(0.4, 0.9, GHOST_HOLD, 1.0 / 60.0).1, 0.0, "the hold outlived the bed");
        // …and it always arrives rather than easing forever.
        let (mut g, mut h) = (1.0, GHOST_HOLD);
        for _ in 0..600 {
            (g, h) = chase_ghost(g, 0.0, h, 1.0 / 60.0);
        }
        assert_eq!(g, 0.0, "the bed never closed");
    }

    /// **THE DIGITS ARE WRITTEN ON THE STROKE THE SHADER CUTS.** The annulus is cut in WGSL
    /// and the number is laid along the middle of it from Rust, so the two copies of where
    /// that stroke IS have to agree — and `make check` never builds a pipeline, so a drift
    /// here ships green and lands the HP number on bare grass.
    #[test]
    fn the_stroke_is_where_the_shader_cuts_it() {
        let src = include_str!("../assets/shaders/feet_ring.wgsl");
        let read = |name: &str| -> f32 {
            let line = src
                .lines()
                .find(|l| l.trim_start().starts_with(&format!("const {name}: f32")))
                .unwrap_or_else(|| panic!("{name} is gone from the shader"));
            line.split('=').nth(1).unwrap().trim().trim_end_matches(';').parse().unwrap()
        };
        assert_eq!(read("R_IN"), R_IN, "the inner wall moved in the shader only");
        assert_eq!(read("R_OUT"), R_OUT, "the outer wall moved in the shader only");
        // …and the digits sit between the two walls rather than on one of them.
        let mid = TEXT_RADIUS / (MESH_RADIUS * RING_SCALE);
        assert!(mid > R_IN && mid < R_OUT, "the number is written off the stroke: {mid}");
    }

    /// **A HEAL AND A HIT ARE THE SAME FIELD MOVING TWO WAYS**, and each gets its own flash —
    /// which has to END, or every ring in a long fight is lit permanently.
    #[test]
    fn a_flash_lands_and_then_goes_out() {
        assert_eq!(fade(1.0, 0.0), 1.0, "a flash must survive the frame it landed on");
        assert!(fade(1.0, 1.0 / 60.0) < 1.0, "the flash never started fading");
        assert_eq!(fade(0.02, 1.0), 0.0, "a flash must reach zero, not approach it");
        assert_eq!(fade(0.0, 1.0), 0.0, "an unlit ring must stay unlit");
    }

    /// **THE NUMBER FOLLOWS THE RING.** Laid on a plain circle the glyphs come out evenly
    /// spaced along the arc, centred on the front, reading left to right, and each turned to
    /// the curve under it — which is the whole difference between a number written IN the ring
    /// and a number lying on top of one.
    #[test]
    fn the_digits_lie_along_the_arc() {
        // A circle of radius 100 in screen space, angle 0 at the bottom (the front), with y
        // growing downward the way a viewport does.
        let project = |t: f32| Some(Vec2::new(100.0 * t.sin(), 100.0 * t.cos()));
        assert_eq!(arc_scale(project).map(|p| p.round()), Some(100.0));

        let slots = arc_text(5, 8.0, project);
        assert_eq!(slots.len(), 5);
        // Evenly spaced, at about the advance asked for.
        for pair in slots.windows(2) {
            let gap = pair[0].0.distance(pair[1].0);
            assert!((gap - 8.0).abs() < 0.2, "glyphs are {gap} apart, not 8");
            assert!(pair[1].0.x > pair[0].0.x, "the number reads backwards");
        }
        // Centred on the front of the ring…
        assert!(slots[2].0.x.abs() < 0.001, "the label is not centred on the front");
        // …and each glyph is turned to the curve, so the ends lean and the middle does not.
        assert!(slots[2].1.abs() < 0.01, "the middle glyph is not level");
        assert!(slots[0].1 * slots[4].1 < 0.0, "the ends lean the same way — that is a line");
    }

    /// A projection that cannot answer — a body behind the camera — draws nothing rather than
    /// stacking every digit at the origin.
    #[test]
    fn a_body_the_camera_cannot_see_writes_nothing() {
        assert!(arc_text(5, 8.0, |_| None).is_empty());
        assert_eq!(arc_scale(|_| Some(Vec2::ZERO)), None, "a ring with no size is not a ring");
    }

    /// **THE METER ROLLS, AND A SECOND BLOW ONLY MOVES THE TARGET.** A bar that jumps has
    /// finished telling you what happened before you look at it; and a roll that RESTARTED on
    /// the next hit would lose the ground it had already counted, which is the one thing
    /// EarthBound's meter never does.
    #[test]
    fn the_meter_rolls_and_a_second_blow_carries_on_from_here() {
        let step = |shown: f32, target: f32, dt: f32| {
            let s = (ROLL_RATE * (target - shown).abs()).max(ROLL_FLOOR) * dt;
            if (target - shown).abs() <= s { target } else { shown + (target - shown).signum() * s }
        };
        // It does not arrive in one frame…
        let after = step(1.0, 0.4, 1.0 / 60.0);
        assert!(after < 1.0 && after > 0.4, "the meter snapped: {after}");
        // …it does arrive.
        let mut v = 1.0;
        for _ in 0..600 {
            v = step(v, 0.4, 1.0 / 60.0);
        }
        assert!((v - 0.4).abs() < 1e-5, "the roll never landed: {v}");
        // A second blow part-way through carries on from where it had got to.
        let mut v = 1.0;
        for _ in 0..12 {
            v = step(v, 0.6, 1.0 / 60.0);
        }
        let mid = v;
        assert!(mid < 1.0 && mid > 0.6);
        let next = step(mid, 0.2, 1.0 / 60.0);
        assert!(next < mid, "the roll went backwards on a second hit");
        assert!(next > 0.2, "the roll teleported to the new target");
        // …and it rolls up as readily as down.
        assert!(step(0.3, 0.9, 1.0 / 60.0) > 0.3, "a heal does not roll");
    }

    /// The digits read the SHOWN level, and a body still standing never shows zero mid-roll.
    #[test]
    fn the_number_is_whatever_the_bar_is_showing() {
        let c = cv("a", true, 40, 100);
        assert_eq!(shown_hp(0.4, &c), 40);
        assert_eq!(shown_hp(1.0, &c), 100);
        assert_eq!(shown_hp(0.001, &c), 1, "a standing body read as dead mid-roll");
        assert_eq!(shown_hp(0.0, &cv("a", true, 0, 100)), 0, "a fallen body must reach zero");
    }
}
