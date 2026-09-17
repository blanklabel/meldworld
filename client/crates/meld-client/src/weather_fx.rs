//! **THE WEATHER COVERS WHAT YOU CAN SEE** (`UX-25`).
//!
//! Rain, snow, ashfall and the drifting motes were each a few hundred `Cuboid` and quad
//! entities whose `Transform` was rewritten every frame — and, far worse, each was confined
//! to a volume sized for a much tighter camera than this game uses:
//!
//! | effect | old volume radius | ground the camera sees |
//! |---|---|---|
//! | rain | **18** | ~200 |
//! | snow / ash | **34** | ~200 |
//! | motes | **62** | ~200 |
//!
//! Reported from play as the weather only covering a small part of the screen, which is
//! exactly what it was doing. Rain's own comment called the disc deliberate — "so the shower
//! tracks the cloud rather than filling the screen" — and that reads as a bug at this camera.
//!
//! ⚠️ **AND THE FIX IS NOT A BIGGER NUMBER OF NODES.** Holding the old density over the
//! ground a player can see needs ~31,000 drops, against the 460 the CPU path could afford:
//! that is the same wall the turn bar's fire hit at forty ember nodes per fighter. These are
//! [`bevy_hanabi`] emitters, simulated on the GPU, spawning on a disc ABOVE the camera and
//! falling through the view — so the cost is one emitter per kind rather than one entity per
//! flake, and the volume is sized to the frame.
//!
//! ⚠️ **The emitters FOLLOW the camera and the particles do NOT.** `SimulationSpace::Global`
//! (hanabi's default) detaches a particle from its emitter the moment it spawns, so the
//! spawn volume tracks the view while the snow already falling stays where it was. Simulated
//! locally instead, a turn of the camera would drag the whole snowfall around with it.

use bevy::prelude::*;

/// The disc each kind spawns on, in world units — sized to the ground the camera sees
/// rather than to the old CPU budget. Rain reaches furthest because it is the one a player
/// reads as "the whole sky is doing this".
const RAIN_RADIUS: f32 = 175.0;
const SNOW_RADIUS: f32 = 150.0;
const ASH_RADIUS: f32 = 150.0;
const MOTE_RADIUS: f32 = 95.0;

/// How far above the camera each kind is born. Rain falls fastest and so needs the least
/// head-room to be at full speed by the time it crosses the frame.
const RAIN_TOP: f32 = 48.0;
const SNOW_TOP: f32 = 40.0;
const ASH_TOP: f32 = 42.0;

/// How many real lights the fireflies carry between them.
///
/// ⚠️ **A GPU PARTICLE CANNOT LIGHT ANYTHING**, and a firefly that does not throw light is
/// a yellow dot. The motes are particles and the GLOW is a handful of `PointLight`s drifting
/// among them — deliberately few and short-range, because Last City already proved this
/// scene can overflow the GPU cluster index list, and a shadowed point light is six scene
/// passes a frame (none of these cast).
const FIREFLY_LAMPS: usize = 7;

/// A drifting firefly lamp: the light half of the motes.
#[derive(Component)]
pub(crate) struct FireflyLamp {
    /// Its own offset from the player, in world xz, and a phase so no two drift alike.
    off: Vec2,
    phase: f32,
}

/// The storm's lightning. `flash` is how hard the sky is lit right now, 0 at rest.
#[derive(Resource, Default)]
pub(crate) struct Lightning {
    pub(crate) flash: f32,
    next_in: f32,
    /// A strike is two beats — the stroke and its echo — because one clean fade reads as a
    /// screen fading to white rather than as lightning.
    beat: u8,
}

/// The full-screen white the flash paints.
#[derive(Component)]
pub(crate) struct LightningPane;

/// Which weather an emitter draws. One component rather than four marker types, because the
/// driver's job is identical for all of them — follow the camera, and decide whether this
/// kind is falling here.
#[derive(Component, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WeatherKind {
    Rain,
    Snow,
    Ash,
    Motes,
}

/// The four built effects, plus the soft dot they all draw with.
#[derive(Resource)]
pub(crate) struct WeatherFx {
    pub(crate) rain: Handle<bevy_hanabi::EffectAsset>,
    pub(crate) snow: Handle<bevy_hanabi::EffectAsset>,
    pub(crate) ash: Handle<bevy_hanabi::EffectAsset>,
    pub(crate) motes: Handle<bevy_hanabi::EffectAsset>,
    pub(crate) dot: Handle<Image>,
    pub(crate) spawned: bool,
}

/// One falling kind, built from the handful of things that actually separate them.
///
/// Rain, snow and ash differ in speed, size, colour and how much they wander — not in
/// structure — so they are one builder with four call sites rather than four near-copies,
/// which is the same argument `ability_fx` makes for carrying sixteen damage types in one
/// shader.
struct Falling {
    name: &'static str,
    radius: f32,
    top: f32,
    /// Downward speed, and how much each particle varies from it.
    fall: (f32, f32),
    /// Sideways wander, which is what separates a drifting flake from a falling drop.
    drift: f32,
    /// Quad size: x is the width and y the length, so rain can be a streak and snow a dot.
    size: Vec2,
    colour: Vec3,
    alpha: f32,
    /// Particles born per second across the whole disc.
    rate: f32,
}

fn falling(effects: &mut Assets<bevy_hanabi::EffectAsset>, f: Falling) -> Handle<bevy_hanabi::EffectAsset> {
    use bevy_hanabi::*;

    let writer = ExprWriter::new();
    let texture_slot = writer.lit(0u32).expr();

    // Born on a horizontal disc over the camera. A disc rather than a sphere because weather
    // arrives from ABOVE: a sphere spends most of its particles beside the viewer, where a
    // falling thing has already missed the frame.
    let init_pos = SetPositionCircleModifier {
        center: writer.lit(Vec3::Y * f.top).expr(),
        axis: writer.lit(Vec3::Y).expr(),
        radius: writer.lit(f.radius).expr(),
        dimension: ShapeDimension::Volume,
    };

    let vel = writer
        .lit(Vec3::new(0.0, -f.fall.0, 0.0))
        + writer.lit(Vec3::new(f.drift, -f.fall.1, f.drift * 0.6))
            * (writer.rand(ValueType::Vector(VectorType::VEC3F)) - writer.lit(Vec3::splat(0.5)))
            * writer.lit(2.0);
    let init_vel = SetAttributeModifier::new(Attribute::VELOCITY, vel.expr());

    // Long enough to cross the whole drop from the spawn disc to well below the camera, so
    // nothing winks out in mid-air where the eye can see it happen.
    let life = (f.top + 30.0) / f.fall.0.max(0.1);
    let init_life =
        SetAttributeModifier::new(Attribute::LIFETIME, writer.lit(life).uniform(writer.lit(life * 1.25)).expr());
    let init_age = SetAttributeModifier::new(Attribute::AGE, writer.lit(0.).uniform(writer.lit(life)).expr());

    let init_colour = SetAttributeModifier::new(
        Attribute::COLOR,
        writer
            .lit(f.colour)
            .vec4_xyz_w(writer.lit(f.alpha))
            .pack4x8unorm()
            .expr(),
    );

    let mut size = Gradient::new();
    size.add_key(0.0, Vec3::new(f.size.x, f.size.y, f.size.x));
    size.add_key(1.0, Vec3::new(f.size.x, f.size.y, f.size.x));

    let mut module = writer.finish();
    module.add_texture_slot("dot");

    // Capacity is rate x lifetime: what can be alive at once. Under-sizing it silently caps
    // the fall partway up the disc, which reads as the weather stopping at a ceiling.
    let capacity = ((f.rate * life * 1.3) as u32).clamp(256, 60_000);

    effects.add(
        EffectAsset::new(capacity, SpawnerSettings::rate(f.rate.into()), module)
            .with_name(f.name)
            .with_alpha_mode(bevy_hanabi::AlphaMode::Blend)
            .init(init_pos)
            .init(init_vel)
            .init(init_life)
            // ⚠️ **AGE IS RANDOMISED AT BIRTH.** Without it every particle alive at startup
            // is the same age, so the first fall arrives as one solid SHEET crossing the
            // frame and then a gap — the weather pulses instead of falling.
            .init(init_age)
            .init(init_colour)
            .render(ParticleTextureModifier {
                texture_slot,
                sample_mapping: ImageSampleMapping::ModulateOpacityFromR,
            })
            .render(SizeOverLifetimeModifier { gradient: size, screen_space_size: false }),
    )
}

/// Build all four. Motes are their own shape — they hover and glow rather than fall.
pub(crate) fn init_weather_fx(
    mut commands: Commands,
    mut effects: ResMut<Assets<bevy_hanabi::EffectAsset>>,
    mut images: ResMut<Assets<Image>>,
) {
    use bevy_hanabi::*;

    let dot = crate::world_fx::soft_dot(&mut images);

    let rain = falling(
        &mut effects,
        Falling {
            name: "rain",
            radius: RAIN_RADIUS,
            top: RAIN_TOP,
            fall: (30.0, 6.0),
            drift: 1.6,
            // A streak, not a dot: length is what makes rain read as rain, and it is the one
            // kind here whose quad is deliberately not square.
            size: Vec2::new(0.035, 0.85),
            colour: Vec3::new(0.72, 0.80, 0.94),
            alpha: 0.34,
            rate: 5200.0,
        },
    );
    let snow = falling(
        &mut effects,
        Falling {
            name: "snow",
            radius: SNOW_RADIUS,
            top: SNOW_TOP,
            // Slow, and wandering far more than it falls — snow is not rain with a different
            // colour, and the difference is the whole effect.
            fall: (2.6, 0.9),
            drift: 2.2,
            size: Vec2::new(0.16, 0.16),
            colour: Vec3::new(0.95, 0.97, 1.0),
            alpha: 0.75,
            rate: 620.0,
        },
    );
    let ash = falling(
        &mut effects,
        Falling {
            name: "ashfall",
            radius: ASH_RADIUS,
            top: ASH_TOP,
            fall: (2.2, 0.8),
            drift: 1.9,
            size: Vec2::new(0.13, 0.13),
            colour: Vec3::new(0.52, 0.36, 0.30),
            alpha: 0.62,
            rate: 520.0,
        },
    );

    // The motes are the one kind that does not fall, so they are built here rather than
    // through `falling`: they hang in the near air, glow, and fade in and out where they
    // stand, which is what reads as a firefly.
    let writer = ExprWriter::new();
    let texture_slot = writer.lit(0u32).expr();
    let init_pos = SetPositionCircleModifier {
        center: writer.lit(Vec3::Y * 1.4).expr(),
        axis: writer.lit(Vec3::Y).expr(),
        radius: writer.lit(MOTE_RADIUS).expr(),
        dimension: ShapeDimension::Volume,
    };
    let init_vel = SetAttributeModifier::new(
        Attribute::VELOCITY,
        (writer.rand(ValueType::Vector(VectorType::VEC3F)) - writer.lit(Vec3::splat(0.5)))
            .mul(writer.lit(Vec3::new(0.5, 0.28, 0.5)))
            .expr(),
    );
    let mote_life = 7.0f32;
    let init_life = SetAttributeModifier::new(
        Attribute::LIFETIME,
        writer.lit(mote_life).uniform(writer.lit(mote_life * 1.6)).expr(),
    );
    let init_age =
        SetAttributeModifier::new(Attribute::AGE, writer.lit(0.).uniform(writer.lit(mote_life)).expr());
    let init_colour = SetAttributeModifier::new(
        Attribute::COLOR,
        writer
            .lit(Vec3::new(1.0, 0.92, 0.55))
            .vec4_xyz_w(writer.lit(1.0))
            .pack4x8unorm()
            .expr(),
    );
    // Up and out, which is what a blinking firefly is: the size carries the blink, since a
    // mote that merely appears and vanishes reads as a rendering fault.
    let mut mote_size = Gradient::new();
    mote_size.add_key(0.0, Vec3::splat(0.0));
    mote_size.add_key(0.25, Vec3::splat(0.075));
    mote_size.add_key(0.75, Vec3::splat(0.075));
    mote_size.add_key(1.0, Vec3::splat(0.0));
    let mut mote_col = Gradient::new();
    mote_col.add_key(0.0, Vec4::new(1.0, 1.0, 1.0, 0.0));
    mote_col.add_key(0.3, Vec4::new(1.0, 1.0, 1.0, 1.0));
    mote_col.add_key(0.7, Vec4::new(1.0, 1.0, 1.0, 1.0));
    mote_col.add_key(1.0, Vec4::new(1.0, 1.0, 1.0, 0.0));
    let mut mote_module = writer.finish();
    mote_module.add_texture_slot("dot");
    let motes = effects.add(
        EffectAsset::new(2048, SpawnerSettings::rate(150.0.into()), mote_module)
            .with_name("motes")
            // Additive: a firefly is LIGHT, and this is the one weather kind that should
            // brighten the ground rather than sit in front of it.
            .with_alpha_mode(bevy_hanabi::AlphaMode::Add)
            .init(init_pos)
            .init(init_vel)
            .init(init_life)
            .init(init_age)
            .init(init_colour)
            .render(ParticleTextureModifier {
                texture_slot,
                sample_mapping: ImageSampleMapping::ModulateOpacityFromR,
            })
            .render(ColorOverLifetimeModifier { gradient: mote_col, ..default() })
            .render(SizeOverLifetimeModifier { gradient: mote_size, screen_space_size: false }),
    );

    commands.insert_resource(WeatherFx { rain, snow, ash, motes, dot, spawned: false });
}

/// Stand the four emitters up once, and thereafter keep them over the camera and tell each
/// whether its weather is falling here.
///
/// ⚠️ **The emitter is moved, never respawned.** Tearing one down and rebuilding it when the
/// weather turns would drop every flake already in the air, so a shower would end by
/// vanishing rather than by passing.
#[allow(clippy::too_many_arguments)]
pub(crate) fn drive_weather_fx(
    mut commands: Commands,
    mut fx: ResMut<WeatherFx>,
    cam_q: Query<&Transform, With<Camera3d>>,
    state: Res<State<crate::Screen>>,
    sky: Res<crate::world_render::Sky>,
    stats: Res<crate::RunStats>,
    ashfall: Res<crate::world_render::Ashfall>,
    mut q: Query<(&WeatherKind, &mut Transform, &mut bevy_hanabi::EffectSpawner), Without<Camera3d>>,
) {
    if !fx.spawned {
        for (kind, effect) in [
            (WeatherKind::Rain, fx.rain.clone()),
            (WeatherKind::Snow, fx.snow.clone()),
            (WeatherKind::Ash, fx.ash.clone()),
            (WeatherKind::Motes, fx.motes.clone()),
        ] {
            commands.spawn((
                kind,
                bevy_hanabi::ParticleEffect::new(effect),
                bevy_hanabi::EffectMaterial { images: vec![fx.dot.clone()] },
                Transform::default(),
            ));
        }
        // The light half of the fireflies, and the pane the lightning paints.
        for i in 0..FIREFLY_LAMPS {
            let a = i as f32 / FIREFLY_LAMPS as f32 * std::f32::consts::TAU;
            commands.spawn((
                FireflyLamp {
                    off: Vec2::new(a.cos(), a.sin()) * (9.0 + i as f32 * 2.3),
                    phase: a * 1.7,
                },
                PointLight {
                    color: Color::srgb(1.0, 0.86, 0.45),
                    intensity: 26_000.0,
                    range: 7.5,
                    // Never a caster: one shadowed point light is six scene passes a frame.
                    shadow_maps_enabled: false,
                    ..default()
                },
                Transform::default(),
                Visibility::Hidden,
            ));
        }
        commands.spawn((
            LightningPane,
            Node {
                position_type: PositionType::Absolute,
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                ..default()
            },
            BackgroundColor(Color::NONE),
            // Under the menus, over the world: a flash washes the scene, not the UI a
            // player might be reading mid-storm.
            GlobalZIndex(20),
            Pickable::IGNORE,
        ));
        fx.spawned = true;
        return;
    }

    let Some(cam) = cam_q.iter().next() else {
        return;
    };
    let outdoors = *state.get() == crate::Screen::Overworld;
    // The same predicates the old CPU drivers asked, so the weather cannot start disagreeing
    // with itself about whether it is raining.
    //
    // ⚠️ The biome comparison is CASE-INSENSITIVE: `RunStats.biome` is title-cased for the
    // HUD readout while every biome key in the codebase is lowercase, and comparing against
    // `"tundra"` matches nothing — silently, with the snow animating perfectly and never
    // appearing. That cost two wrong hypotheses the first time.
    let mut snowing = outdoors && stats.biome.eq_ignore_ascii_case("tundra");
    let mut raining = outdoors && sky.weather > 0.05 && !snowing;
    let mut ashing = outdoors && ashfall.intensity > 0.02;
    if let Some(forced) = crate::flags::weather_mock_flag() {
        raining = outdoors && (forced == "rain" || forced == "storm");
        snowing = outdoors && forced == "snow";
        ashing = outdoors && forced == "ash";
    }

    for (kind, mut tf, mut spawner) in &mut q {
        // ⚠️ **FALLING WEATHER HANGS OFF THE CAMERA; THE MOTES STAND ON THE GROUND.** Rain,
        // snow and ash are born above the camera's own y and fall past it, which is what
        // keeps them out of the terrain on raised ground — an absolute height put the whole
        // snowfall UNDERGROUND wherever the heightmap lifted the land. Fireflies are the
        // opposite case: they belong in the near air just over the grass, and hanging them
        // off the camera's y put them fourteen units up, hovering above the treeline.
        let y = match kind {
            WeatherKind::Motes => crate::world_render::terrain_height(
                cam.translation.x,
                cam.translation.z,
            ),
            _ => cam.translation.y,
        };
        let want = Vec3::new(cam.translation.x, y, cam.translation.z);
        if tf.translation != want {
            tf.translation = want;
        }
        let on = match kind {
            WeatherKind::Rain => raining,
            WeatherKind::Snow => snowing,
            WeatherKind::Ash => ashing,
            WeatherKind::Motes => outdoors,
        };
        if spawner.active != on {
            spawner.active = on;
        }
    }
}

/// Drift the firefly lamps around the player and light them only where the motes are.
///
/// They are lit by NIGHT rather than by the clock alone: a firefly glowing at noon is a
/// bright dot on grass, and the whole reason to carry real lights is the dark.
pub(crate) fn drive_firefly_lamps(
    time: Res<Time>,
    state: Res<State<crate::Screen>>,
    sky: Res<crate::world_render::Sky>,
    cam_q: Query<&Transform, (With<Camera3d>, Without<FireflyLamp>)>,
    mut q: Query<(&FireflyLamp, &mut Transform, &mut Visibility), Without<Camera3d>>,
) {
    let Some(cam) = cam_q.iter().next() else {
        return;
    };
    // `t` runs 0 at midnight to 0.5 at noon, so the dark hours are the two ends.
    let night = sky.t < 0.22 || sky.t > 0.78;
    let on = *state.get() == crate::Screen::Overworld && night;
    let want = if on { Visibility::Inherited } else { Visibility::Hidden };
    let t = time.elapsed_secs();
    for (lamp, mut tf, mut vis) in &mut q {
        if *vis != want {
            *vis = want;
        }
        if !on {
            continue;
        }
        // A slow wander, each on its own phase — a ring of lamps holding formation around
        // the player reads as a lantern rig rather than as insects.
        let wob = Vec2::new(
            (t * 0.23 + lamp.phase).sin() * 4.5,
            (t * 0.19 + lamp.phase * 1.3).cos() * 4.5,
        );
        tf.translation = Vec3::new(
            cam.translation.x + lamp.off.x + wob.x,
            crate::world_render::terrain_height(
                cam.translation.x + lamp.off.x + wob.x,
                cam.translation.z + lamp.off.y + wob.y,
            ) + 1.3,
            cam.translation.z + lamp.off.y + wob.y,
        );
    }
}

/// **A NIGHT STORM THROWS LIGHTNING.** Only a SUPER storm — the one that covers the whole
/// area rather than a shower passing under a cloud — and only after dark, because a flash
/// that does not light anything up is a white rectangle.
///
/// ⚠️ The strike is two beats, the stroke and its echo. One clean fade reads as the screen
/// fading to white; the double blink is what makes it lightning.
pub(crate) fn drive_lightning(
    time: Res<Time>,
    state: Res<State<crate::Screen>>,
    sky: Res<crate::world_render::Sky>,
    mut storm: ResMut<Lightning>,
    mut pane: Query<&mut BackgroundColor, With<LightningPane>>,
) {
    let dt = time.delta_secs();
    let night = sky.t < 0.22 || sky.t > 0.78;
    let forced_storm =
        crate::flags::weather_mock_flag().is_some_and(|f| f == "storm");
    let storming = *state.get() == crate::Screen::Overworld
        && night
        && (forced_storm || (sky.super_storm && sky.phase == 2 && sky.weather > 0.5));

    if storming {
        storm.next_in -= dt;
        if storm.next_in <= 0.0 {
            storm.flash = if storm.beat == 0 { 1.0 } else { 0.55 };
            // The echo follows close; the next strike is a long wait, so a storm is punctuated
            // rather than strobing.
            storm.next_in = if storm.beat == 0 { 0.12 } else { 5.0 + (sky.t * 37.0).fract() * 7.0 };
            storm.beat = if storm.beat == 0 { 1 } else { 0 };
        }
    } else {
        storm.next_in = 2.0;
    }
    // Fast decay: a stroke is over before you can look at it, which is what makes the
    // afterimage of the scene the thing you remember rather than the white.
    storm.flash = (storm.flash - dt * 4.5).max(0.0);

    let a = storm.flash * 0.5;
    for mut bg in &mut pane {
        let want = if a > 0.001 { Color::srgba(0.86, 0.90, 1.0, a) } else { Color::NONE };
        if bg.0 != want {
            bg.0 = want;
        }
    }
}
