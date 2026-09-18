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
//!
//! ⚠️ **A CLASS RESOURCE DOES LIVE HERE, AND THAT IS NOT THE SAME ARGUMENT.** A second, thinner
//! hoop inside the first draws what a Hunter has banked or a Psyker is holding. The gauge loses
//! its place here because it is a COMPARISON — "who goes next" is only answerable by putting
//! everybody on one track — while nobody ever compares one fighter's Adrenaline against
//! another's. A resource is a fact about one body and nothing else, so the body is where it
//! belongs, and there is no second surface already answering it to be in competition with.

use bevy::asset::Asset;
use bevy::prelude::*;
use bevy::reflect::TypePath;
use bevy::render::render_resource::AsBindGroup;
use bevy::shader::ShaderRef;

use crate::battle::SpriteQuad;
use crate::{status_num, BattleData};
use meld_client::net::CombatantView;

/// How high the tube's centre floats above the ground. Its own minor radius, so the tube sits
/// ON the ground rather than half sunk into it.
pub(crate) const RING_LIFT: f32 = 0.115;
/// The torus's major radius — the line the tube runs along, and where the HP digits sit.
/// ⚠️ Mirrored from `world_render`'s `ring_liquid_mesh`; a test holds the two together.
pub(crate) const RING_MAJOR: f32 = 0.78;

/// Half the tube's thickness — the torus's minor radius, mirrored from `world_render`'s mesh
/// (a test holds the two together).
///
/// ⚠️ **IT IS WHAT THE DIGITS RIDE ON.** The number sits at `RING_MAJOR`, which is the tube's
/// CENTRE-LINE — and a centre-line projects onto the tube's upper EDGE, not its middle, because
/// a camera looking down at a fat tube sees its crown. Written at the centre-line height the
/// digits straddled the rim: half on the green, half over the hole, which is what "the text
/// isn't on the tube" looks like. They ride the crown now, which is the surface actually facing
/// the player.
pub(crate) const RING_MINOR: f32 = 0.135;
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
/// How many points the wave is simulated at, around the whole ring.
///
/// ⚠️ **PACKED AS `vec4`s BECAUSE std140 GIVES A BARE `f32` ARRAY A 16-BYTE STRIDE.** An array
/// of floats in a uniform block costs four times its own size and reads back wrong if the two
/// sides disagree about the padding; four-to-a-`vec4` is the packing both sides can state.
pub(crate) const WAVE_N: usize = 48;
pub(crate) const WAVE_VEC4S: usize = WAVE_N / 4;
/// How stiff the surface is — this is the wave SPEED, and it is the number to reason about
/// rather than guess.
///
/// ⚠️ The discrete wave equation carries a disturbance at `sqrt(STIFF)` samples per second, so
/// at 260 a splash crossed four of forty-eight samples in a quarter second and died where it
/// landed — a dent, not a wave. 6400 is ~80 samples/s, about two-thirds of a lap per second,
/// which is what reads as water moving. ⚠️ It is bounded by stability: `STIFF * dt²` must stay
/// under 1 or the integrator diverges, and at 6400 with a 1/120 step it is 0.44.
const WAVE_STIFF: f32 = 6400.0;
/// The restoring pull toward flat. This is what makes it WATER rather than sound: without it a
/// disturbance propagates forever and never settles into a level.
const WAVE_SPRING: f32 = 7.0;
/// Velocity damping, so a ring that was hit eventually goes still.
const WAVE_DAMP: f32 = 3.6;
/// A slow bleed on the height itself, as the reference does (`pressure *= 0.999`). Velocity
/// damping alone leaves a damped oscillator whose envelope is set by the spring, which is a
/// long tail on a bar that has to be still between blows.
const WAVE_BLEED: f32 = 0.9965;
/// The fixed step the surface is integrated at. A wave equation solved on a variable frame
/// time changes its own speed with the frame rate and blows up on a long one.
const WAVE_DT: f32 = 1.0 / 120.0;
/// How broad a splash is, in samples squared.
///
/// ⚠️ **A NARROW PULSE DOES NOT TRAVEL, IT SITS THERE AND DISPERSES.** On a discrete Laplacian
/// the group velocity falls to zero as the wavelength approaches two samples, so a tight
/// Gaussian is almost entirely made of components that go nowhere — measured, a 1.7-sample
/// splash put 5e-11 of itself on the far side of the ring after a quarter second. Four and a
/// half samples wide is low-frequency enough to actually propagate.
const WAVE_SPLASH_WIDTH: f32 = 40.0;

/// The surface of one ring's liquid: height and velocity at [`WAVE_N`] points around it.
///
/// ⚠️ **THE WAVE NEVER MOVES THE WATERLINE.** The boundary between green and red IS the health
/// number — displace it and the bar reads as the value changing, which came back from play as
/// *people think they're getting slight heals*. The surface rides INSIDE the liquid as light
/// and shade, and the level it sits on is only ever the real one.
pub(crate) struct RingWave {
    p: [f32; WAVE_N],
    v: [f32; WAVE_N],
    /// Left-over time, so the step stays fixed whatever the frame did.
    acc: f32,
}

// ⚠️ Hand-written: `Default` is only derived for arrays up to 32, and the surface is 48 points.
impl Default for RingWave {
    fn default() -> Self {
        Self { p: [0.0; WAVE_N], v: [0.0; WAVE_N], acc: 0.0 }
    }
}

impl RingWave {
    /// Integrate the surface forward. Periodic: it is a ring, so a wave that leaves one end
    /// arrives at the other rather than reflecting off a wall that is not there.
    pub(crate) fn step(&mut self, dt: f32) {
        self.acc = (self.acc + dt).min(WAVE_DT * 8.0);
        while self.acc >= WAVE_DT {
            self.acc -= WAVE_DT;
            let old = self.p;
            for i in 0..WAVE_N {
                let left = old[(i + WAVE_N - 1) % WAVE_N];
                let right = old[(i + 1) % WAVE_N];
                let accel = (left + right - 2.0 * old[i]) * WAVE_STIFF - WAVE_SPRING * old[i];
                self.v[i] = (self.v[i] + accel * WAVE_DT) * (1.0 - WAVE_DAMP * WAVE_DT);
            }
            for i in 0..WAVE_N {
                self.p[i] = (self.p[i] + self.v[i] * WAVE_DT) * WAVE_BLEED;
            }
        }
    }

    /// Drop something in at `at` (0..1 around the ring), with `amp` as its size.
    pub(crate) fn splash(&mut self, at: f32, amp: f32) {
        let centre = at.rem_euclid(1.0) * WAVE_N as f32;
        for i in 0..WAVE_N {
            // Distance the short way round, since the two ends of the array are neighbours.
            let d = ((i as f32 - centre).abs()).min(WAVE_N as f32 - (i as f32 - centre).abs());
            self.v[i] += amp * (-d * d / WAVE_SPLASH_WIDTH).exp();
        }
    }

    /// The surface, packed for the shader.
    pub(crate) fn packed(&self) -> [Vec4; WAVE_VEC4S] {
        let mut out = [Vec4::ZERO; WAVE_VEC4S];
        for (i, q) in out.iter_mut().enumerate() {
            *q = Vec4::new(self.p[i * 4], self.p[i * 4 + 1], self.p[i * 4 + 2], self.p[i * 4 + 3]);
        }
        out
    }
}

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

/// Step a SHOWN value toward the real one. Both rings at a fighter's feet roll rather than
/// snap, and they roll at the same rate on purpose — a health bar and a resource bar that
/// counted at different speeds would read as two unrelated widgets stuck to one body.
///
/// ⚠️ **ONE COPY, because the second one is where it drifts.** This lived inline in
/// `drive_rings` with the test re-implementing it beside, which is two statements of a rule
/// and no way for either to notice the other moving.
pub(crate) fn roll_toward(shown: f32, target: f32, dt: f32) -> f32 {
    let step = (ROLL_RATE * (target - shown).abs()).max(ROLL_FLOOR) * dt;
    if (target - shown).abs() <= step {
        target
    } else {
        shown + (target - shown).signum() * step
    }
}

/// World-space radius of the middle of the stroke: the line the digits sit on.
pub(crate) const TEXT_RADIUS: f32 = RING_MAJOR;

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
    /// `(heal, hit, barrier, flow)`.
    #[uniform(100)]
    pub(crate) pulse: Vec4,
    /// `(front bearing in turns, _, _, _)` — where the camera-facing arc is on the torus's own
    /// `u`. ⚠️ Handed in rather than read off the mesh: `u` starts wherever the generator began
    /// winding, and reading that seam instead is what put the pool behind the body once already.
    #[uniform(100)]
    pub(crate) view: Vec4,
    /// The liquid's surface, four samples to a `vec4`, [`WAVE_N`] round the ring.
    #[uniform(100)]
    pub(crate) wave: [Vec4; WAVE_VEC4S],
}

impl Material for FeetRing {
    fn fragment_shader() -> ShaderRef {
        "shaders/feet_ring.wgsl".into()
    }
    /// ⚠️ **OPAQUE, because it is now a solid thing inside a transparent one.** Blending the
    /// liquid made sense while it was paint on the ground; inside a glass shell it has to be
    /// what the shell REFRACTS, and a transmissive material cannot pick up something that is
    /// itself drawn in the transparent pass.
    fn alpha_mode(&self) -> AlphaMode {
        AlphaMode::Opaque
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
    /// This ring's own liquid surface.
    pub(crate) wave: RingWave,
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

/// **ONE MESH, ONE MATERIAL — the glass is a term in the shader, not a second torus.**
///
/// ⚠️ This spawned TWO tori for a while: the liquid, and a slightly fatter
/// `StandardMaterial` shell around it with `specular_transmission` on, so the vessel would
/// be real glass with real refraction. It reads correctly at the size that reference art is
/// drawn at and it does not read at all at the size a feet ring occupies on screen — a
/// twenty-pixel stroke seen nearly edge-on. Reported from play as **"you have these double
/// stacked on enemies"**, and that is the honest reading of the picture: two concentric
/// rings, one grey and one green, not liquid inside a vessel.
///
/// ⚠️ **And the shell ATE the readout it was decorating.** Alpha-blended over an opaque
/// liquid it greyed the pool down, so a creature at FULL health — the case with the most
/// green to show — drew as a plain grey ring with nothing in it. The two failures its own
/// retired comment recorded (transmit everything and the empty half vanishes; tint it enough
/// to see and the full half goes grey) were not a band to be threaded. They were the shape
/// being wrong.
///
/// So the tube is ONE torus and `feet_ring.wgsl` draws its empty part as empty glass instead
/// of discarding it. The vessel is then the same object as its contents by construction, and
/// nothing can stack on anything.
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
        view: Vec4::ZERO,
        wave: [Vec4::ZERO; WAVE_VEC4S],
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
            wave: RingWave::default(),
        },
        Mesh3d(mesh),
        MeshMaterial3d(mat),
        // ⚠️ **NO ROTATION.** A torus is already generated in the XZ plane, so the -90° turn the
        // flat disc needed would stand this one on its edge.
        Transform::from_xyz(0.0, RING_LIFT, 0.0),
    ));
}

/// Anything lying on the ground at one fighter's feet, and whose body it belongs to.
///
/// ⚠️ **IT EXISTS SO THE FOLLOW CANNOT BE FORGOTTEN.** This file already records what happens
/// when a system drives one entity and a second one is spawned beside it: the health ring's
/// glass shell carried no `CombatantRing`, so `drive_rings` never hid it and every corpse kept
/// an empty bar at its feet for the rest of the fight. `ring_follows_body` is written ONCE
/// against this trait and registered per ring, so a hoop added at these feet tomorrow follows
/// the body the day it is written rather than the day somebody remembers.
pub(crate) trait AtTheFeetOf {
    fn body(&self) -> &str;
}

impl AtTheFeetOf for CombatantRing {
    fn body(&self) -> &str {
        &self.id
    }
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
pub(crate) fn ring_follows_body<R: Component + AtTheFeetOf>(
    sprites: Query<(&SpriteQuad, &Transform), Without<R>>,
    mut rings: Query<(&R, &mut Transform)>,
) {
    for (ring, mut tf) in &mut rings {
        let Some((_, body)) = sprites.iter().find(|(s, _)| s.id == ring.body()) else { continue };
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
    cam: Query<&GlobalTransform, With<Camera3d>>,
    mut q: Query<(&mut CombatantRing, &mut GlobalTransform, &mut Visibility), Without<Camera3d>>,
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
    // Where the camera is, so each ring can put its pool on the arc facing the viewer. ⚠️ This
    // is per-ring, not global: two bodies at opposite ends of a wide formation are seen from
    // measurably different bearings, and a single number would swing one of their pools off
    // the front.
    let eye = cam.iter().next().map(|t| t.translation());
    for (mut ring, gt, mut vis) in &mut q {
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
        ring.shown = roll_toward(ring.shown, target, dt);
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

        // ── the surface ───────────────────────────────────────────────────────────────────
        // A blow drops something in AT THE WATERLINE, which is where it physically lands: the
        // level is symmetric about the near arc, so that is two points, one each side.
        // ⚠️ The disturbance is a VELOCITY, not a displacement — pushing the height directly
        // gives a square-edged bump that reads as a graphics error rather than as a splash.
        if ring.hit >= 0.999 || ring.heal >= 0.999 {
            let amp = if ring.hit >= 0.999 { 3.4 } else { 2.0 };
            ring.wave.splash(fill * 0.5, amp);
            ring.wave.splash(1.0 - fill * 0.5, amp);
        }
        // …and the wheel shoving the ring open rocks the whole thing from the front.
        if shoved.as_deref() == Some(ring.id.as_str()) {
            ring.wave.splash(0.0, 2.6);
        }
        ring.wave.step(dt);
        let (ghost, hold) = chase_ghost(ring.ghost, fill, ring.hold, dt);
        ring.ghost = ghost;
        ring.hold = hold;
        let active = battle.active.as_deref() == Some(ring.id.as_str());
        let Some(mut mat) = rings.get_mut(&ring.mat) else { continue };
        mat.params = Vec4::new(fill, ring.ghost, t, if active { 1.0 } else { 0.0 });
        mat.pulse = Vec4::new(ring.heal, ring.hit, barrier_fill(c), ring.flow);
        mat.wave = ring.wave.packed();
        // ⚠️ **THE TORUS'S `u = 0` IS WHEREVER ITS GENERATOR STARTED WINDING**, which has
        // nothing to do with the camera. Bevy winds it from +x, so the bearing of the viewer in
        // the ring's own frame is `atan2(dz, dx)` turned into turns.
        if let Some(eye) = eye {
            let to_eye = eye - gt.translation();
            let turns = to_eye.z.atan2(to_eye.x) / std::f32::consts::TAU;
            mat.view.x = turns.rem_euclid(1.0);
        }
    }
}

// ── THE SECOND RING: WHAT THIS BODY HAS BANKED ────────────────────────────────────────────

/// The resource hoop's major radius — the line its tube runs along.
///
/// ⚠️ **INSIDE THE HEALTH RING, BECAUSE OUTSIDE IS FULL.** The health tube reaches
/// `RING_MAJOR + RING_MINOR` = 0.915 and the command wheel's inner wall stands at 0.941, with
/// 0.026 between them — the tightest number in `battle_radial`. The hole in the middle
/// (everything under 0.645) is ground the body is already standing on and nothing else uses at
/// rest, so that is where the second readout goes. ⚠️ Unlike the health ring above, the MESH is
/// built from this constant (`world_render`'s `ring_resource_mesh`) rather than repeating it,
/// so the two cannot disagree in the first place.
pub(crate) const RES_MAJOR: f32 = 0.49;
/// Half the resource tube's thickness — a little over half the health tube's, because the two
/// hoops have to be told apart at a glance and THICKNESS is the cheapest way to say which one
/// is the bar you keep standing on.
pub(crate) const RES_MINOR: f32 = 0.075;
/// How high its centre floats: its own minor radius, so the tube sits ON the ground rather
/// than half sunk into it — the same rule `RING_LIFT` states for the health ring.
pub(crate) const RES_LIFT: f32 = RES_MINOR;

// **THE TWO HOOPS MUST NOT TOUCH, AND THEY MUST NOT WEIGH THE SAME.** Asserted at COMPILE TIME
// rather than in a test: every term is a constant in this file, so there is nothing to run and
// nothing to remember to run. (`glass.rs` holds its column fractions the same way.)
//
// ⚠️ Touching walls is precisely what made the health ring's own glass shell read as *"two rings
// stacked on one body"* — the reading that got it deleted. The gap has to be wider than the
// hoop itself, or the pair reads as one banded ring; and the hoop has to be visibly the thinner
// of the two, because thickness is most of what says which one you keep standing on.
const _: () = assert!(RES_MAJOR + RES_MINOR < RING_MAJOR - RING_MINOR);
const _: () = assert!(RING_MAJOR - RING_MINOR - (RES_MAJOR + RES_MINOR) > RES_MINOR);
const _: () = assert!(RES_MINOR < RING_MINOR * 0.7);
// It sits ON the ground rather than half sunk into it — `RING_LIFT`'s rule, one hoop in.
const _: () = assert!(RES_LIFT >= RES_MINOR * 0.8);

/// **BANKED FURY.** Amber rather than the health ring's red: a resource climbing must never be
/// mistaken for a body bleeding, and those two hoops are eighty thousandths of a world unit
/// apart. It is the same heat the sprite's own rage tint wears (`animate_battle_actors` reads
/// the identical `adrenaline:` fraction), so the ring says the number the body is already
/// shouting.
const FURY: Color = Color::srgb(1.0, 0.52, 0.10);
/// **HELD MANIFESTATIONS.** Cold, against the Hunter's hot — which is the whole rule for this
/// hoop's palette: fury is amber and mind is azure, so two resources can never be mistaken for
/// each other or for the health ring's green-and-red.
///
/// ⚠️ **IT WAS VIOLET FIRST, AND VIOLET IS THE ONE COLOUR IT CANNOT BE.** The Psyker's own
/// sprite is a purple robe, and the hoop lies at its feet — rendered, the ring simply
/// disappeared into the hem. A readout drawn on a body has to be picked against THAT BODY'S
/// art, not only against the rest of the palette.
const PSYCHE: Color = Color::srgb(0.32, 0.66, 1.0);

/// What a fighter has banked, as the ring draws it.
pub(crate) struct Resource {
    /// How full, 0..1.
    pub(crate) fill: f32,
    /// How many WHOLE units it is counted in, or 0 for a smooth quantity. It decides whether
    /// the hoop carries division notches.
    pub(crate) units: f32,
    pub(crate) hue: Color,
}

/// **DOES THIS BODY BANK ANYTHING, AND HOW FULL IS IT?** The one place that answers.
///
/// ⚠️ **IT ASKS THE WIRE, NEVER A LIST OF CLASSES.** The obvious shape is
/// `match class { "hunter" => …, "psyker" => … }`, and this repo has deleted that shape twice
/// already — the engine's per-class skill dispatch and the client's per-ability targeting list,
/// both of which went stale and both of which failed silently. `adrenaline_max:` and
/// `focus_slots:` are only ever sent by a fighter that HAS the thing, so a class that gains a
/// resource on the wire gets a ring the day the server sends it, and one that loses its
/// resource loses the ring without anything here being edited.
///
/// ⚠️ **AND A CREATURE IS NOT EXCLUDED ON PURPOSE.** Nothing in the engine gives a creature
/// either status today, so this returns `None` for every one of them — but excluding them HERE
/// would be the list again, one level down.
pub(crate) fn resource_of(c: &CombatantView) -> Option<Resource> {
    // A Hunter's **Adrenaline**: banked by basic attacks, spent by every skill it owns.
    let max = status_num(&c.statuses, "adrenaline_max:");
    if max > 0 {
        return Some(Resource {
            fill: (status_num(&c.statuses, "adrenaline:") as f32 / max as f32).clamp(0.0, 1.0),
            // ⚠️ **SMOOTH, DELIBERATELY.** It banks 25 a swing and the costs are 30 / 35 / 40 /
            // 80, so notches at the swing's own granularity would draw a grid the prices do not
            // sit on — three notches filled reads as "affordable" and buys nothing.
            units: 0.0,
            hue: FURY,
        });
    }
    // A Psyker's **Focus**: N slots, each holding a manifestation that fires every Psyker turn.
    // Full is what you are working toward, exactly as with Adrenaline, so the two read the same
    // way round.
    let slots = status_num(&c.statuses, "focus_slots:");
    if slots > 0 {
        // ⚠️ `focus_slots:` does not match `focus:` — the separator differs — so the count
        // cannot accidentally include its own capacity.
        let held = c.statuses.iter().filter(|s| s.starts_with("focus:")).count();
        return Some(Resource {
            fill: (held as f32 / slots as f32).clamp(0.0, 1.0),
            // Slots ARE whole things, so the hoop is divided into them: "two of four held" is a
            // countable fact and a smooth bar cannot state it.
            units: slots as f32,
            hue: PSYCHE,
        });
    }
    None
}

/// **WHETHER THIS HOOP SHOULD BE DRAWN AT ALL**, in one expression: there has to be a body,
/// it has to be standing, and it has to bank something.
///
/// ⚠️ **ITS OWN FUNCTION SO IT CAN BE TESTED.** The failure it exists to prevent has already
/// shipped in this file — a ring whose owner was down kept drawing, because the hide lived in
/// a system and nothing could reach it. A predicate can be asserted; a system body cannot.
pub(crate) fn resource_shown(c: Option<&CombatantView>) -> Option<Resource> {
    c.filter(|c| c.hp > 0).and_then(resource_of)
}

/// The resource hoop's material. One per body that banks something.
#[derive(Asset, AsBindGroup, TypePath, Debug, Clone)]
pub(crate) struct ResourceRing {
    /// `(fill, units, seconds, front bearing in turns)` — see `resource_ring.wgsl`.
    #[uniform(100)]
    pub(crate) params: Vec4,
    /// This resource's own colour, opacity in `a`.
    #[uniform(100)]
    pub(crate) tint: Vec4,
    /// `(gain, spend, _, _)`.
    #[uniform(100)]
    pub(crate) pulse: Vec4,
}

impl Material for ResourceRing {
    fn fragment_shader() -> ShaderRef {
        "shaders/resource_ring.wgsl".into()
    }
    fn alpha_mode(&self) -> AlphaMode {
        AlphaMode::Opaque
    }
}

/// Marks the resource hoop under one combatant.
#[derive(Component)]
pub(crate) struct ResourceRingOf {
    pub(crate) id: String,
    pub(crate) mat: Handle<ResourceRing>,
    /// Last frame's level — the only way to tell a bank from a spend, since the wire carries
    /// the amount and never the event that moved it. Same trick the health ring plays on HP.
    pub(crate) last: f32,
    /// The two flashes, 1 at the moment it landed and fading from there.
    pub(crate) gain: f32,
    pub(crate) spend: f32,
    /// The level being SHOWN, rolling toward the real one (see [`roll_toward`]).
    pub(crate) shown: f32,
}

impl AtTheFeetOf for ResourceRingOf {
    fn body(&self) -> &str {
        &self.id
    }
}

/// Put a resource hoop at one body's feet. Only ever called where [`resource_of`] said yes —
/// most heroes and every creature bank nothing, and an entity per body for a readout with no
/// number behind it is an entity per body drawing a permanently empty ring.
pub(crate) fn spawn_resource_ring(
    parent: &mut ChildSpawnerCommands,
    mesh: Handle<Mesh>,
    mats: &mut Assets<ResourceRing>,
    id: &str,
    res: &Resource,
) {
    let c = res.hue.to_linear();
    let mat = mats.add(ResourceRing {
        params: Vec4::new(res.fill, res.units, 0.0, 0.0),
        tint: Vec4::new(c.red, c.green, c.blue, 1.0),
        pulse: Vec4::ZERO,
    });
    parent.spawn((
        ResourceRingOf {
            id: id.to_string(),
            mat: mat.clone(),
            last: res.fill,
            gain: 0.0,
            spend: 0.0,
            shown: res.fill,
        },
        Mesh3d(mesh),
        MeshMaterial3d(mat),
        // ⚠️ **NO ROTATION** — a torus is generated in the XZ plane already, exactly as the
        // health ring's note says.
        Transform::from_xyz(0.0, RES_LIFT, 0.0),
    ));
}

/// Drive every resource hoop: what is banked, what just moved it, and where the camera is.
pub(crate) fn drive_resource_rings(
    time: Res<Time>,
    battle: Res<BattleData>,
    mut mats: ResMut<Assets<ResourceRing>>,
    cam: Query<&GlobalTransform, With<Camera3d>>,
    mut q: Query<(&mut ResourceRingOf, &GlobalTransform, &mut Visibility)>,
) {
    let t = time.elapsed_secs();
    let dt = time.delta_secs();
    let eye = cam.iter().next().map(|t| t.translation());
    for (mut ring, gt, mut vis) in &mut q {
        // ⚠️ **ONE EXPRESSION COVERS BOTH WAYS THIS HOOP SHOULD NOT BE DRAWN** — the body is
        // down, or it no longer banks anything. Written as two separate guards they are two
        // things to remember, and forgetting the first is exactly the bug that left an empty
        // health ring on every corpse for a whole build.
        let Some(res) = resource_shown(battle.view(&ring.id)) else {
            if *vis != Visibility::Hidden {
                *vis = Visibility::Hidden;
            }
            continue;
        };
        if *vis != Visibility::Inherited {
            *vis = Visibility::Inherited;
        }
        // A BANK AND A SPEND ARE THE SAME FIELD MOVING TWO WAYS.
        if res.fill > ring.last + 0.001 {
            ring.gain = 1.0;
        } else if res.fill < ring.last - 0.001 {
            ring.spend = 1.0;
        }
        ring.last = res.fill;
        ring.shown = roll_toward(ring.shown, res.fill, dt);
        ring.gain = fade(ring.gain, dt);
        ring.spend = fade(ring.spend, dt);

        let shown = ring.shown;
        let (gain, spend) = (ring.gain, ring.spend);
        let Some(now) = mats.get(&ring.mat) else { continue };
        // ⚠️ **THE CLOCK IS ONLY SPENT WHEN SOMETHING IS MOVING.** `Assets::get_mut` flags the
        // asset modified and the render world rebuilds its bind group, so a material rewritten
        // every frame costs a frame whether or not the value moved — and an EMPTY hoop has
        // nothing animating on it at all: the flicker is masked out by the charge that is not
        // there, the waterline needs a level to sit at, and the capacity chase needs capacity.
        // So an empty ring holds its last `t` and stops writing entirely.
        let animating = shown > 0.001 || gain > 0.0 || spend > 0.0;
        let front = eye
            .map(|eye| {
                let to_eye = eye - gt.translation();
                (to_eye.z.atan2(to_eye.x) / std::f32::consts::TAU).rem_euclid(1.0)
            })
            .unwrap_or(now.params.w);
        // The hue can move mid-fight — a body that took on a different resource is a different
        // colour on the same hoop — so it is written rather than fixed at spawn.
        let c = res.hue.to_linear();
        let params = Vec4::new(shown, res.units, if animating { t } else { now.params.z }, front);
        let pulse = Vec4::new(gain, spend, 0.0, 0.0);
        let tint = Vec4::new(c.red, c.green, c.blue, 1.0);
        if now.params == params && now.pulse == pulse && now.tint == tint {
            continue;
        }
        let Some(mut m) = mats.get_mut(&ring.mat) else { continue };
        m.params = params;
        m.pulse = pulse;
        m.tint = tint;
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

    /// **THE RING'S GEOMETRY IS DECLARED IN ONE PLACE AND USED IN ANOTHER.** The two tori are
    /// built in `world_render` and everything here — where the digits sit, how high the tube
    /// floats — is stated against their radii. `make check` never builds a mesh, so a radius
    /// changed on one side and not the other ships green and puts the number in mid-air.
    #[test]
    fn the_tube_is_the_size_the_meshes_are() {
        let src = include_str!("world_render.rs");
        let read = |mesh: &str, field: &str| -> f32 {
            let line = src
                .lines()
                .find(|l| l.contains(mesh) && l.contains("Torus"))
                .unwrap_or_else(|| panic!("{mesh} is gone"));
            let at = line.find(field).unwrap_or_else(|| panic!("{mesh} has no {field}"));
            line[at + field.len()..]
                .trim_start_matches(|c: char| c == ':' || c.is_whitespace())
                .split([',', ' ', '}'])
                .next()
                .unwrap()
                .parse()
                .unwrap()
        };
        let major = read("ring_liquid_mesh", "major_radius");
        let minor = read("ring_liquid_mesh", "minor_radius");
        assert_eq!(major, RING_MAJOR, "the digits sit off the tube");
        // The digits are lifted onto the tube's crown by `RING_MINOR`, so a mesh whose tube got
        // fatter without that constant moving would write them back inside the hole.
        assert_eq!(minor, RING_MINOR, "the digits ride a tube of a different thickness");
        // ⚠️ **AND THERE IS EXACTLY ONE OF THEM.** A second concentric torus is what
        // `spawn_ring`'s note calls the double-stacked reading; re-adding one is re-adding
        // that bug, so the absence is asserted rather than left to memory.
        assert!(
            !src.contains("ring_glass_mesh"),
            "the ring is one torus: a second shell reads as two rings, not as liquid in glass",
        );
        // The tube sits ON the ground rather than half sunk into it.
        assert!(RING_LIFT >= minor * 0.8, "the tube is buried: {RING_LIFT}");
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
        // ⚠️ The REAL function, not a copy of it beside the thing it is testing — which is
        // what this was, and is why the rule could have moved under the test unnoticed. Both
        // hoops at a body's feet roll through it.
        let step = roll_toward;
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


    fn banking(id: &str, hp: i32, toks: &[&str]) -> CombatantView {
        let mut c = cv(id, true, hp, 100);
        c.statuses = toks.iter().map(|s| s.to_string()).collect();
        c
    }

    /// **A RESOURCE IS READ OFF THE WIRE, NEVER OFF A CLASS NAME.** This repo has deleted a
    /// hand-written list of ability keys twice; a hand-written list of classes-with-resources
    /// is the same thing wearing a different hat, and it fails the same way — silently, on the
    /// class somebody forgot.
    #[test]
    fn a_resource_is_whatever_the_body_says_it_has() {
        // A Hunter banks Adrenaline, and it is a SMOOTH quantity.
        let h = resource_of(&banking("h", 40, &["class:hunter", "adrenaline:50", "adrenaline_max:100"]))
            .expect("a Hunter banks Adrenaline");
        assert!((h.fill - 0.5).abs() < 1e-6, "half-banked read as {}", h.fill);
        assert_eq!(h.units, 0.0, "Adrenaline must carry no notches: its costs do not sit on a grid");

        // A Psyker holds Focus, counted in whole slots.
        let p = resource_of(&banking(
            "p",
            40,
            &["class:psyker", "focus_slots:4", "focus:gravity_well:2", "focus:kinetic_aegis:1"],
        ))
        .expect("a Psyker holds Focus");
        assert!((p.fill - 0.5).abs() < 1e-6, "two of four read as {}", p.fill);
        assert_eq!(p.units, 4.0, "the slots are not divisions on the hoop");
        // ⚠️ The capacity token must not be counted as one of the things held.
        assert_ne!(p.hue, h.hue, "two resources cannot share a colour");

        // Everybody else banks nothing — including a body that merely CLAIMS to be a Hunter.
        assert!(resource_of(&banking("e", 40, &["class:explorer"])).is_none());
        assert!(
            resource_of(&banking("liar", 40, &["class:hunter"])).is_none(),
            "the ring came off a class name rather than off what the body actually carries"
        );
        // …and a creature, which carries neither status.
        assert!(resource_of(&cv("boar", false, 30, 60)).is_none());
    }

    /// A slot count that is somehow zero, or more held than held-able, must not divide by zero
    /// or overfill the hoop.
    #[test]
    fn a_nonsense_resource_draws_nothing_rather_than_a_broken_ring() {
        assert!(resource_of(&banking("z", 40, &["focus_slots:0"])).is_none());
        assert!(resource_of(&banking("z", 40, &["adrenaline_max:0", "adrenaline:9"])).is_none());
        let over = resource_of(&banking(
            "o",
            40,
            &["focus_slots:1", "focus:a:1", "focus:b:1", "focus:c:1"],
        ))
        .unwrap();
        assert_eq!(over.fill, 1.0, "a hoop cannot overfill");
        let over = resource_of(&banking("o", 40, &["adrenaline_max:10", "adrenaline:999"])).unwrap();
        assert_eq!(over.fill, 1.0, "a hoop cannot overfill");
    }

    /// **A FALLEN BODY BANKS NOTHING.** The health ring's own history is a corpse that kept
    /// wearing an empty bar for the rest of the fight, because the hide lived somewhere the
    /// second entity never reached. One predicate answers for the hoop, and here it is.
    #[test]
    fn a_downed_body_wears_no_resource_hoop() {
        let alive = banking("h", 40, &["adrenaline:50", "adrenaline_max:100"]);
        let dead = banking("h", 0, &["adrenaline:50", "adrenaline_max:100"]);
        assert!(resource_shown(Some(&alive)).is_some());
        assert!(resource_shown(Some(&dead)).is_none(), "a corpse kept its resource ring");
        assert!(resource_shown(None).is_none(), "a body the fight has forgotten kept its ring");
    }

    /// **THE SHADER READS THE FIELDS IN THE ORDER THE RUST WRITES THEM.** `AsBindGroup` packs
    /// one `#[uniform(100)]` block in DECLARATION order, so a member added on one side in a
    /// different place makes the shader read another's bytes — which shows up as a hoop in
    /// completely the wrong colour, or a fill that is really a unit count, and no error
    /// anywhere. `make check` never builds a pipeline, so nothing else can catch it.
    #[test]
    fn the_resource_shader_and_its_uniform_agree() {
        let wgsl = include_str!("../assets/shaders/resource_ring.wgsl");
        let body = wgsl
            .split_once("struct ResourceParams {")
            .expect("the shader's uniform block is gone")
            .1
            .split_once("};")
            .unwrap()
            .0;
        let fields: Vec<&str> = body
            .lines()
            .filter_map(|l| l.trim().split_once(':').map(|(n, _)| n.trim()))
            .filter(|n| !n.starts_with("//") && !n.is_empty())
            .collect();
        assert_eq!(
            fields,
            vec!["params", "tint", "pulse"],
            "the shader's uniform block no longer matches `ResourceRing`'s field order"
        );
    }

    /// **THE SURFACE IS A WAVE, AND IT SETTLES.** A disturbance has to travel (a splash at one
    /// point must reach a point away from it) and it has to DIE (a ring that rang forever
    /// would be a bar that never holds still, which is the complaint this replaced).
    #[test]
    fn the_surface_carries_a_wave_and_then_goes_flat() {
        let mut w = RingWave::default();
        w.splash(0.0, 3.0);
        // It moves at all…
        w.step(0.05);
        let near = w.packed()[0].x.abs();
        assert!(near > 1e-4, "the splash did nothing: {near}");
        // …it travels away from where it landed…
        w.step(0.25);
        let far: f32 = w.packed()[WAVE_VEC4S / 2].to_array().iter().map(|v| v.abs()).sum();
        assert!(far > 1e-4, "the wave never reached the far side: {far}");
        // …and it ends flat rather than ringing forever. Measured as the DEEPEST remaining
        // sample, which is the thing a player could still see — a sum over 48 points answers
        // a question nobody is asking and fails on a ring that is visibly still.
        for _ in 0..300 {
            w.step(1.0 / 60.0);
        }
        let left = w.packed().iter().flat_map(|q| q.to_array()).fold(0.0_f32, |m, v| m.max(v.abs()));
        assert!(left < 0.02, "the surface never settled: {left}");
    }

    /// **A SPLASH IS PERIODIC.** The ring has no ends, so a disturbance near the seam has to
    /// reach round it — landing at 0.99 must stir the samples just past 0.0.
    #[test]
    fn a_splash_reaches_round_the_seam() {
        let mut w = RingWave::default();
        w.splash(0.995, 3.0);
        w.step(1.0 / 120.0);
        let first = w.packed()[0].x.abs();
        assert!(first > 1e-5, "the splash stopped at the seam: {first}");
    }
}
