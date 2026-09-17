//! **THE WORLD'S BIGGEST MOMENT, IN THE LANGUAGE THE ARENA ALREADY SPEAKS** (`UX-24`).
//!
//! A Shift rearranges the ground a player is standing on: props are re-scattered, mountains
//! are re-cut, and everyone inside takes Force damage. The client answered all of that with
//! a line of text and a glow on the ground — so the one event in the game where the WORLD
//! acts looked like a notification, while a single sword blow throws sparks.
//!
//! The debris is [`bevy_hanabi`], the same way a felled body comes apart (`UX-22`), and it
//! is tinted by the INCOMING biome from [`meld_proto::regions::biome_rgb`] — the land you
//! are about to be standing in is what gets thrown up out of the land you are standing on.
//!
//! ⚠️ **A REGION IS ENORMOUS AND A SCREEN IS NOT.** The doomed patch is an annulus wedge
//! that can run thousands of units around the fan, so seeding it evenly would put almost
//! every emitter outside the interest radius and pay for all of them. The emitters are
//! placed on the part of the region NEAREST THE PLAYER, which is the same rule the ground
//! shader's ranges and bridges learned the hard way — truncation is not a window.

use bevy::prelude::*;

/// How long a landing's emitters are kept before they are swept. Comfortably past the
/// longest particle lifetime below, since despawning the emitter kills its live motes.
const SHIFT_DUST_TTL: f32 = 2.6;

/// How many emitters one landing seeds — ~23,000 motes across the view, which is what it
/// takes for the ground to read as coming apart rather than as a scatter of pops. A Shift
/// is the loudest thing the world does, so this is deliberately far more than a death burst — but it is a fixed cost per landing
/// rather than one that rides the region's size, because the region can be a whole ring.
const SHIFT_EMITTERS: usize = 288;

/// How far up-screen the seeded ground reaches, in world units ahead of the look point.
/// About the interest radius, which is as far as the world is furnished.
const SHIFT_FAR: f32 = 195.0;

/// How far BEHIND the look point it reaches — the strip of ground along the bottom edge,
/// which is nearer the camera than whatever it is aimed at.
const SHIFT_BEHIND: f32 = 46.0;

/// Half-width of the ground footprint at the look point, and how fast it opens with
/// distance. A camera sees a WEDGE, so a patch of constant width covers the near ground and
/// misses the far ground entirely.
const SHIFT_WIDE_NEAR: f32 = 38.0;
const SHIFT_WIDE_SLOPE: f32 = 0.95;

/// Still used as the "is this landing near enough to draw at all" bound.
const SHIFT_REACH: f32 = 46.0;

/// The dimmest a landing's colour may be, as its brightest channel. Below this an additive
/// effect at night is indistinguishable from nothing at all.
const SHIFT_TINT_FLOOR: f32 = 0.72;

/// The Shift's debris. ONE `EffectAsset` whose colour is a property, for the reason the
/// death burst's is: an asset per biome is a pipeline per biome, compiled the first time
/// each theme happens to arrive.
#[derive(Resource)]
pub(crate) struct ShiftDust {
    pub(crate) effect: Handle<bevy_hanabi::EffectAsset>,
    pub(crate) dot: Handle<Image>,
}

/// A landing waiting to be drawn, queued by the wire handler and drained by
/// [`spawn_shift_dust`]. Queued rather than spawned in place because the handler has no
/// access to the terrain height the emitters have to stand on.
pub(crate) struct ShiftBurst {
    pub(crate) inner: f32,
    pub(crate) outer: f32,
    pub(crate) arc_center: f32,
    pub(crate) arc_half: f32,
    /// The colour of the biome being thrown OFF — what goes up.
    pub(crate) rgb_from: Vec3,
    /// The colour of the biome arriving — what settles back down.
    pub(crate) rgb_to: Vec3,
}

/// The world's own effect queue, the overworld's counterpart to `BattleFx`.
#[derive(Resource, Default)]
pub(crate) struct WorldFx {
    pub(crate) shifts: Vec<ShiftBurst>,
}

/// Marks a spawned emitter so the sweep can find it.
#[derive(Component)]
pub(crate) struct ShiftDustTtl(pub(crate) f32);

/// A soft round mote, built rather than authored.
///
/// ⚠️ **A PARTICLE WITH NO TEXTURE IS A SQUARE, AND IT READS AS ONE.** Hanabi draws an
/// untextured particle as a flat quad, so the first cut of the debris was a scatter of hard
/// white CARDS lying in the forest — confetti rather than earth being thrown. The texture is
/// generated here instead of shipped as a png because it is three lines of falloff and an
/// asset nobody can open in an editor is an asset that goes stale; `ModulateOpacityFromR`
/// then takes only the red channel, so the particle's own `COLOR` still carries the biome.
pub(crate) fn soft_dot(images: &mut Assets<Image>) -> Handle<Image> {
    use bevy::image::Image;
    use bevy::asset::RenderAssetUsages;
    use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};

    const N: u32 = 32;
    let mut data = Vec::with_capacity((N * N * 4) as usize);
    for y in 0..N {
        for x in 0..N {
            // Distance from the centre in units of the radius, eased so the edge is soft
            // rather than a circle with a hard rim — which is the same square problem
            // wearing a rounder shape.
            let dx = (x as f32 + 0.5) / N as f32 * 2.0 - 1.0;
            let dy = (y as f32 + 0.5) / N as f32 * 2.0 - 1.0;
            let d = (dx * dx + dy * dy).sqrt().min(1.0);
            let a = ((1.0 - d) * (1.0 - d) * 255.0) as u8;
            data.extend_from_slice(&[a, a, a, a]);
        }
    }
    images.add(Image::new(
        Extent3d { width: N, height: N, depth_or_array_layers: 1 },
        TextureDimension::D2,
        data,
        TextureFormat::Rgba8Unorm,
        RenderAssetUsages::RENDER_WORLD,
    ))
}

/// Build the one debris asset.
pub(crate) fn init_shift_dust(
    mut commands: Commands,
    mut effects: ResMut<Assets<bevy_hanabi::EffectAsset>>,
    mut images: ResMut<Assets<Image>>,
) {
    use bevy_hanabi::*;

    let dot = soft_dot(&mut images);
    let writer = ExprWriter::new();
    let texture_slot = writer.lit(0u32).expr();

    // A patch of ground rather than a point: each emitter throws up a column a few units
    // across, so the seeded points read as a disturbance running through the land instead
    // of as two dozen identical puffs.
    let init_pos = SetPositionSphereModifier {
        center: writer.lit(Vec3::ZERO).expr(),
        radius: writer.lit(9.0).expr(),
        dimension: ShapeDimension::Volume,
    };
    // Up and out, hard. This is the ground itself being thrown, which is the one effect in
    // the game allowed to look like an explosion.
    let init_vel = SetVelocitySphereModifier {
        center: writer.lit(Vec3::Y * 5.0).expr(),
        speed: writer.lit(5.0).uniform(writer.lit(13.0)).expr(),
    };
    let lifetime = writer.lit(1.6).uniform(writer.lit(3.0)).expr();
    let init_lifetime = SetAttributeModifier::new(Attribute::LIFETIME, lifetime);
    let init_age = SetAttributeModifier::new(Attribute::AGE, writer.lit(0.).expr());
    // Gravity, so it comes back down — debris that drifts away is smoke, and smoke does
    // not say the ground moved.
    // **A BOOM, THEN A FLOAT.** Heavy drag against a light gravity is what makes the two
    // halves read as different events: the throw is spent almost at once, and what is left
    // drifts back down slowly enough to watch it change colour. A big gravity with light
    // drag is a fountain, which is the same motion in both directions.
    let accel = AccelModifier::new(writer.lit(Vec3::Y * -3.2).expr());
    let drag = LinearDragModifier::new(writer.lit(2.8).expr());

    // **IT GOES UP AS WHAT WAS THERE AND COMES DOWN AS WHAT ARRIVES.** The Shift is a
    // SUBSTITUTION, and the debris is the only place a player can watch that happen: the
    // throw carries the outgoing biome's colour and each mote crosses to the incoming one
    // as it falls, so a mire becoming desert is green thrown up and sand drifting back.
    //
    // ⚠️ It is written in the UPDATE pass rather than set at birth, because a colour that
    // changes over a particle's own life is not something a birth-time attribute can say —
    // and it deliberately replaces `ColorOverLifetimeModifier` rather than sitting beside
    // it, since that modifier OVERWRITES the colour this computes.
    let up = writer.add_property("tint_up", Value::Vector(Vec3::ONE.into()));
    let down = writer.add_property("tint_down", Value::Vector(Vec3::ONE.into()));
    let up = writer.prop(up);
    let down = writer.prop(down);
    let init_colour = SetAttributeModifier::new(
        Attribute::COLOR,
        up.clone().vec4_xyz_w(writer.lit(1.)).pack4x8unorm().expr(),
    );

    // How far through its own life this mote is, 0 at the throw and 1 as it lands.
    let life_t = (writer.attr(Attribute::AGE) / writer.attr(Attribute::LIFETIME)).clamp(
        writer.lit(0.),
        writer.lit(1.),
    );
    // The crossing happens EARLY — most of the fall is already the new colour, so the eye
    // reads "it came back different" rather than watching a slow tween all the way down.
    let mix = (life_t.clone() * writer.lit(1.8)).clamp(writer.lit(0.), writer.lit(1.));
    let rgb = up.clone() + (down - up) * mix;
    // Full through the throw, then out as it settles. Squared so the tail is quick and the
    // ground is not left carpeted in motes.
    let alpha = writer.lit(1.) - life_t.clone() * life_t;
    let fade_colour = SetAttributeModifier::new(
        Attribute::COLOR,
        rgb.vec4_xyz_w(alpha).pack4x8unorm().expr(),
    );

    // Debris is chunky at the throw and dust by the time it lands.
    let mut size = Gradient::new();
    size.add_key(0.0, Vec3::splat(0.55));
    size.add_key(0.55, Vec3::splat(0.34));
    size.add_key(1.0, Vec3::splat(0.05));

    let mut module = writer.finish();
    module.add_texture_slot("dot");
    let effect = effects.add(
        EffectAsset::new(768, SpawnerSettings::once(80.0.into()), module)
            .with_name("shift_dust")
            // ⚠️ **ADDITIVE, the fifth time this trap has been closed.** A blended quad is
            // an opaque card painted over the art — measured, the first cut drew as sheets
            // of white paper standing in the forest, occluding the props behind them.
            // Additive LIGHTS the ground instead, which is what debris catching the sun is.
            .with_alpha_mode(bevy_hanabi::AlphaMode::Add)
            .init(init_pos)
            .init(init_vel)
            .init(init_lifetime)
            .init(init_age)
            .init(init_colour)
            .update(accel)
            .update(drag)
            .update(fade_colour)
            .render(ParticleTextureModifier {
                texture_slot,
                sample_mapping: ImageSampleMapping::ModulateOpacityFromR,
            })
            .render(SizeOverLifetimeModifier { gradient: size, screen_space_size: false }),
    );
    commands.insert_resource(ShiftDust { effect, dot });
}

/// Where a landing's emitters stand: the part of the doomed patch nearest `at`.
///
/// A free function so the windowing rule is testable without a world — the trap this repo
/// keeps recording is a feature that generates correctly and is consumed nowhere, and a
/// seeding rule that quietly puts every emitter over the horizon looks exactly like a
/// shader that does not work.
///
/// Bearings are taken the way the ground shader takes them (`atan2(z, x)`), and the anchor
/// is the player's own position CLAMPED into the region — so a party standing in the Shift
/// gets it under their feet, and a party watching from outside gets it along the near edge
/// rather than on the far side of the ring.
pub(crate) fn dust_points(b: &ShiftBurst, at: Vec2, fwd: Vec2, n: usize) -> Vec<Vec2> {
    let (inner, outer) = (b.inner.min(b.outer), b.outer.max(b.inner));
    let r_at = at.length();
    let anchor_r = r_at.clamp(inner, outer);
    let bearing = at.y.atan2(at.x);
    let anchor_b = if b.arc_half <= 0.0 {
        bearing
    } else {
        let d = wrapped(bearing - b.arc_center);
        b.arc_center + d.clamp(-b.arc_half, b.arc_half)
    };

    // **A SHIFT ON THE FAR SIDE OF THE WORLD COSTS NOTHING.** A region is placed by the
    // world, not by where the party happens to be, so most landings a session hears about
    // are nowhere near it — and debris a thousand units past the interest radius is a
    // draw call for ground that is not even furnished. If the nearest corner of the patch
    // is out of sight, there is nothing to throw.
    let anchor = Vec2::new(anchor_r * anchor_b.cos(), anchor_r * anchor_b.sin());
    if anchor.distance(at) > SHIFT_REACH * 2.0 {
        return Vec::new();
    }

    // **THE FOOTPRINT IS THE CAMERA'S, NOT A DISC.** A disc around the look point covers the
    // bottom of the frame and stops well short of the horizon, because perspective puts the
    // top half of the screen on ground a hundred units further out — reported from play as
    // the effect only covering the bottom half. This walks the view's own ground wedge:
    // forward from behind the look point out to the furnished edge, opening sideways as it
    // goes, which is the shape of what a player can actually see.
    let fwd = if fwd.length_squared() > 1e-6 { fwd.normalize() } else { Vec2::Y };
    let right = Vec2::new(-fwd.y, fwd.x);

    let mut out = Vec::with_capacity(n);
    // Candidates are generated deterministically — a Shift is a world event, and two
    // clients watching the same one should not disagree about where the ground broke — and
    // OVERSAMPLED, because a landing whose region covers only part of the view has the rest
    // of its candidates dropped rather than shoved onto the boundary.
    for i in 0..(n * 4) {
        if out.len() == n {
            break;
        }
        let t = i as f32;
        let u = (t * 0.7548777).fract();
        let v = (t * 0.5698403).fract();
        // Biased toward the far half: the near ground is a sliver of the frame and the far
        // ground is most of it, so an even walk along the axis crowds the bottom edge.
        let d = -SHIFT_BEHIND + u.sqrt() * (SHIFT_FAR + SHIFT_BEHIND);
        let half = SHIFT_WIDE_NEAR + d.max(0.0) * SHIFT_WIDE_SLOPE;
        let p = at + fwd * d + right * ((v - 0.5) * 2.0 * half);

        // The region is the truth about what is Shifting, so a candidate outside it is
        // DROPPED. Clamping it back in was the first cut and it stacked every outside
        // candidate onto the boundary, drawing the landing as a blob sitting on a circle.
        let pr = p.length();
        if pr < inner || pr > outer {
            continue;
        }
        if b.arc_half > 0.0 && wrapped(p.y.atan2(p.x) - b.arc_center).abs() > b.arc_half {
            continue;
        }
        out.push(p);
    }
    out
}

/// An angle folded into `-PI..=PI`, so "how far round is this bearing from that one" is the
/// short way round rather than the long one.
fn wrapped(a: f32) -> f32 {
    let tau = std::f32::consts::TAU;
    let mut d = a % tau;
    if d > std::f32::consts::PI {
        d -= tau;
    } else if d < -std::f32::consts::PI {
        d += tau;
    }
    d
}

/// Drain queued landings into emitters standing on the ground they threw up.
pub(crate) fn spawn_shift_dust(
    mut commands: Commands,
    mut fx: ResMut<WorldFx>,
    dust: Option<Res<ShiftDust>>,
    cam: Query<&Transform, With<Camera3d>>,
) {
    if fx.shifts.is_empty() {
        return;
    }
    let Some(dust) = dust else {
        fx.shifts.clear();
        return;
    };
    // ⚠️ **WHERE THE CAMERA LOOKS, NOT WHERE IT STANDS.** The overworld camera sits behind
    // and above the party, so a patch centred on its own position spends half its emitters
    // behind the viewer. Projecting its forward ray onto the ground puts the centre of the
    // heave at the centre of the frame.
    let (at, fwd) = cam
        .iter()
        .next()
        .map(|t| (ground_ahead(t), Vec2::new(t.forward().x, t.forward().z)))
        .unwrap_or((Vec2::ZERO, Vec2::Y));

    for b in std::mem::take(&mut fx.shifts) {
        // ⚠️ **LIFTED TO A FLOOR, NOT FLATTENED TO A HUE.** Ashfall's authored grey-brown
        // added to a night forest is nothing at all — measured, the landing was invisible in
        // every frame — but normalising to the brightest channel was the overcorrection: it
        // washes every desaturated biome to near-white, and an ashfall Shift came out
        // looking like snow. Scaling the whole colour up until its brightest channel clears
        // the floor keeps the RATIOS, so a tundra stays pale blue and an ashfall stays
        // ashen, and only the too-dark end is lifted at all.
        let lift = |c: Vec3| c * (SHIFT_TINT_FLOOR / c.max_element().max(0.05)).max(1.0);
        let mut props = bevy_hanabi::EffectProperties::default();
        props.set("tint_up", bevy_hanabi::Value::Vector(lift(b.rgb_from).into()));
        props.set("tint_down", bevy_hanabi::Value::Vector(lift(b.rgb_to).into()));
        for p in dust_points(&b, at, fwd, SHIFT_EMITTERS) {
            let y = crate::world_render::terrain_height(p.x, p.y);
            commands.spawn((
                ShiftDustTtl(SHIFT_DUST_TTL),
                bevy_hanabi::ParticleEffect::new(dust.effect.clone()),
                bevy_hanabi::EffectMaterial { images: vec![dust.dot.clone()] },
                props.clone(),
                Transform::from_xyz(p.x, y, p.y),
            ));
        }
    }
}

/// Where a camera's view ray meets the ground, in world xz — the middle of the frame.
///
/// Falls back to the camera's own footprint when it is looking at the horizon, since a ray
/// that never descends has no ground point and a division by it runs off to infinity.
fn ground_ahead(t: &Transform) -> Vec2 {
    let f = t.forward();
    let here = Vec2::new(t.translation.x, t.translation.z);
    if f.y >= -0.05 {
        return here;
    }
    let d = (t.translation.y / -f.y).clamp(0.0, 200.0);
    here + Vec2::new(f.x, f.z) * d
}

/// Sweep spent emitters.
pub(crate) fn advance_shift_dust(
    mut commands: Commands,
    time: Res<Time>,
    mut q: Query<(Entity, &mut ShiftDustTtl)>,
) {
    let dt = time.delta_secs();
    for (e, mut ttl) in &mut q {
        ttl.0 -= dt;
        if ttl.0 <= 0.0 {
            commands.entity(e).despawn();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn burst(inner: f32, outer: f32, c: f32, half: f32) -> ShiftBurst {
        ShiftBurst {
            inner,
            outer,
            arc_center: c,
            arc_half: half,
            rgb_from: Vec3::ONE,
            rgb_to: Vec3::ONE,
        }
    }

    /// Every emitter lands inside the patch that is actually Shifting. Ground outside it
    /// is not changing, and debris thrown off it says the wrong region is going.
    #[test]
    fn every_emitter_stands_inside_the_doomed_patch() {
        let b = burst(300.0, 560.0, 0.8, 0.25);
        for p in dust_points(&b, Vec2::new(320.0, 300.0), Vec2::Y, 40) {
            let r = p.length();
            assert!(r >= b.inner - 0.01 && r <= b.outer + 0.01, "r {r} outside band");
            let d = wrapped(p.y.atan2(p.x) - b.arc_center).abs();
            assert!(d <= b.arc_half + 0.01, "bearing {d} outside wedge");
        }
    }

    /// The seeding follows the PLAYER, not the region's midpoint. A region can run
    /// thousands of units around the fan, so two parties at opposite ends of one landing
    /// must each get it under their own feet rather than sharing one patch between them.
    #[test]
    fn the_debris_is_seeded_where_somebody_is_standing() {
        let b = burst(200.0, 3000.0, 0.0, 2.0);
        // Both stand INSIDE the wedge (bearing within `arc_half` of its centre) and far
        // apart along it — a point just outside is correctly given nothing at all, which is
        // what the horizon rule above is for.
        let (west, east) = (Vec2::new(0.0, 1800.0), Vec2::new(2400.0, 300.0));
        let mine = |at: Vec2, from: Vec2| {
            let pts = dust_points(&b, at, Vec2::Y, 24);
            assert!(!pts.is_empty(), "nothing seeded for a party standing in the region");
            pts.iter().map(|p| p.distance(from)).sum::<f32>() / pts.len() as f32
        };
        // Each party's own patch is under its feet and the other party's is not — which is
        // the whole rule, and is not something a fixed patch on the region could satisfy.
        assert!(mine(west, west) < SHIFT_FAR);
        assert!(mine(east, east) < SHIFT_FAR);
        assert!(mine(west, east) > SHIFT_FAR * 2.0, "both parties got the same patch");
    }

    /// A landing nobody is near throws nothing. Most Shifts a session hears about are
    /// somewhere else in the world, and debris past the interest radius is a draw call for
    /// ground that has not even been furnished.
    #[test]
    fn a_shift_over_the_horizon_costs_nothing() {
        let b = burst(200.0, 3000.0, 0.0, 2.0);
        // Outside the wedge by more than a screen: the party can see where it landed on
        // the map and cannot see the land itself.
        assert!(dust_points(&b, Vec2::new(-1500.0, 900.0), Vec2::Y, 24).is_empty());
        // And outside the BAND, which is the other way to be nowhere near one: a party in
        // the hub ring hears that the deep world rearranged itself.
        let deep = burst(2000.0, 3000.0, 0.0, 2.0);
        assert!(dust_points(&deep, Vec2::new(12.0, 3.0), Vec2::Y, 24).is_empty());
        // The near edge of a band you are just outside still throws, though — a Shift you
        // ran out of a moment ago is one you should be able to watch land.
        assert!(!dust_points(&b, Vec2::new(150.0, 40.0), Vec2::Y, 24).is_empty());
    }

    /// A whole-ring landing (`arc_half <= 0`, what an older server sends) still seeds, and
    /// still seeds near the player rather than collapsing onto one bearing.
    #[test]
    fn a_ring_with_no_wedge_still_breaks_ground_underfoot() {
        let b = burst(100.0, 400.0, 0.0, 0.0);
        let at = Vec2::new(-250.0, -80.0);
        let pts = dust_points(&b, at, Vec2::Y, 24);
        assert_eq!(pts.len(), 24);
        // The bound is the view's own footprint: forward to the furnished edge, and
        // opening sideways as it goes.
        let far = SHIFT_FAR + SHIFT_WIDE_NEAR + SHIFT_FAR * SHIFT_WIDE_SLOPE;
        assert!(pts.iter().all(|p| p.distance(at) < far));
    }
}
