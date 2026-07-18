//! HD-2D **look-dev** scene — a standalone diorama for tuning the render pipeline
//! by eye before wiring it into the game. Primitive stand-ins (lit ground, trees,
//! rocks, a glowing portal, billboarded "sprite" characters) under the full HD-2D
//! post stack: HDR + Bloom + tilt-shift Depth-of-Field + distance Fog + a
//! shadow-casting sun + tonemapping.
//!
//! Everything is **live-tunable from the keyboard** with an on-screen readout, so
//! you can dial in the look on a real display and tell me the numbers to bake in.
//! Native only (the HD-2D post needs a real GPU): `cargo run -p meld-client --bin hd2d`.

use bevy::core_pipeline::bloom::Bloom;
use bevy::core_pipeline::dof::{DepthOfField, DepthOfFieldMode};
use bevy::core_pipeline::tonemapping::Tonemapping;
use bevy::pbr::{DistanceFog, FogFalloff};
use bevy::prelude::*;

/// Every knob the scene exposes. Tweak live; the readout prints current values.
#[derive(Resource)]
struct Look {
    cam_pitch: f32, // degrees the camera tilts down toward the diorama
    cam_yaw: f32,   // degrees around the target
    cam_dist: f32,  // distance from the focus point
    focus: f32,     // DoF focal distance (what stays sharp)
    aperture: f32,  // DoF f-stops — LOWER = more (tilt-shift) blur
    bloom: f32,     // bloom intensity
    fog_start: f32,
    fog_end: f32,
    sun_pitch: f32,
    sun_yaw: f32,
    orbit: bool,
    dof_on: bool,
    bloom_on: bool,
    fog_on: bool,
}

impl Default for Look {
    fn default() -> Self {
        // A first-guess HD-2D framing: a steep look-down, strong tilt-shift, warm
        // bloom. You'll tune from here.
        Look {
            cam_pitch: 42.0,
            cam_yaw: 0.0,
            cam_dist: 22.0,
            focus: 20.0,
            aperture: 1.2,
            bloom: 0.28,
            fog_start: 34.0,
            fog_end: 90.0,
            sun_pitch: 55.0,
            sun_yaw: 40.0,
            orbit: false,
            dof_on: true,
            bloom_on: true,
            fog_on: true,
        }
    }
}

#[derive(Resource)]
struct CamEntity(Entity);

#[derive(Component)]
struct Billboard;

#[derive(Component)]
struct HudText;

fn main() {
    App::new()
        .add_plugins(
            DefaultPlugins
                .set(ImagePlugin::default_nearest())
                .set(WindowPlugin {
                    primary_window: Some(Window {
                        title: "MELDWORLD — HD-2D look-dev".into(),
                        ..default()
                    }),
                    ..default()
                }),
        )
        .insert_resource(ClearColor(Color::srgb(0.02, 0.03, 0.06)))
        .insert_resource(AmbientLight {
            color: Color::srgb(0.6, 0.7, 0.95),
            brightness: 220.0,
            ..default()
        })
        .init_resource::<Look>()
        .add_systems(Startup, setup)
        .add_systems(Update, (control, apply, billboard, hud).chain())
        .run();
}

fn bloom_component(look: &Look) -> Bloom {
    let mut b = Bloom::NATURAL;
    b.intensity = look.bloom;
    b
}

fn dof_component(look: &Look) -> DepthOfField {
    DepthOfField {
        mode: DepthOfFieldMode::Bokeh,
        focal_distance: look.focus,
        aperture_f_stops: look.aperture,
        sensor_height: 0.01866,
        max_circle_of_confusion_diameter: 64.0,
        max_depth: f32::INFINITY,
    }
}

fn fog_component(look: &Look) -> DistanceFog {
    DistanceFog {
        color: Color::srgb(0.35, 0.42, 0.6),
        falloff: FogFalloff::Linear {
            start: look.fog_start,
            end: look.fog_end,
        },
        ..default()
    }
}

fn setup(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut mats: ResMut<Assets<StandardMaterial>>,
    look: Res<Look>,
) {
    // Camera with the full HD-2D post stack.
    let cam = commands
        .spawn((
            Camera3d::default(),
            Camera { hdr: true, ..default() },
            Tonemapping::TonyMcMapface,
            bloom_component(&look),
            dof_component(&look),
            fog_component(&look),
            Transform::default(),
        ))
        .id();
    commands.insert_resource(CamEntity(cam));

    // Sun (shadow-casting directional light).
    commands.spawn((
        DirectionalLight {
            illuminance: 9000.0,
            shadows_enabled: true,
            color: Color::srgb(1.0, 0.96, 0.85),
            ..default()
        },
        Transform::default(),
    ));

    // Ground.
    commands.spawn((
        Mesh3d(meshes.add(Plane3d::default().mesh().size(400.0, 400.0))),
        MeshMaterial3d(mats.add(StandardMaterial {
            base_color: Color::srgb(0.18, 0.32, 0.16),
            perceptual_roughness: 0.95,
            ..default()
        })),
        Transform::default(),
    ));

    // Scattered trees + rocks (deterministic splitmix so the scene is stable).
    let trunk = meshes.add(Cylinder::new(0.18, 1.4));
    let canopy = meshes.add(Sphere::new(1.1));
    let rock = meshes.add(Cuboid::new(1.0, 0.7, 1.0));
    let bark = mats.add(StandardMaterial {
        base_color: Color::srgb(0.35, 0.22, 0.12),
        perceptual_roughness: 1.0,
        ..default()
    });
    let leaf = mats.add(StandardMaterial {
        base_color: Color::srgb(0.2, 0.5, 0.24),
        perceptual_roughness: 0.9,
        ..default()
    });
    let stone = mats.add(StandardMaterial {
        base_color: Color::srgb(0.4, 0.4, 0.42),
        perceptual_roughness: 1.0,
        ..default()
    });
    let mut s: u64 = 0x1234_5678;
    let mut rnd = || {
        s = s
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        ((s >> 33) as f32) / (u32::MAX as f32)
    };
    for _ in 0..44 {
        let x = (rnd() - 0.5) * 60.0;
        let z = (rnd() - 0.5) * 60.0;
        if x.abs() < 4.0 && z.abs() < 4.0 {
            continue; // keep the centre stage clear
        }
        if rnd() < 0.6 {
            commands.spawn((
                Mesh3d(trunk.clone()),
                MeshMaterial3d(bark.clone()),
                Transform::from_xyz(x, 0.7, z),
            ));
            commands.spawn((
                Mesh3d(canopy.clone()),
                MeshMaterial3d(leaf.clone()),
                Transform::from_xyz(x, 1.9, z).with_scale(Vec3::splat(0.8 + rnd() * 0.6)),
            ));
        } else {
            commands.spawn((
                Mesh3d(rock.clone()),
                MeshMaterial3d(stone.clone()),
                Transform::from_xyz(x, 0.35, z).with_scale(Vec3::splat(0.6 + rnd() * 1.2)),
            ));
        }
    }

    // Glowing portal (HDR emissive torus → bloom catches it).
    commands.spawn((
        Mesh3d(meshes.add(Torus::new(0.35, 1.5))),
        MeshMaterial3d(mats.add(StandardMaterial {
            base_color: Color::srgb(0.1, 0.4, 0.5),
            emissive: LinearRgba::rgb(0.4, 5.0, 6.0),
            ..default()
        })),
        Transform::from_xyz(11.0, 2.2, -6.0).with_rotation(Quat::from_rotation_x(std::f32::consts::FRAC_PI_2)),
    ));

    // Resource "gems" (emissive).
    let gem = meshes.add(Sphere::new(0.35));
    for (gx, gz) in [(-6.0, 5.0), (4.0, 7.0), (-9.0, -4.0)] {
        commands.spawn((
            Mesh3d(gem.clone()),
            MeshMaterial3d(mats.add(StandardMaterial {
                base_color: Color::srgb(0.8, 0.7, 0.2),
                emissive: LinearRgba::rgb(4.0, 3.0, 0.5),
                ..default()
            })),
            Transform::from_xyz(gx, 0.5, gz),
        ));
    }

    // Billboarded "sprite" stand-ins (flat unlit quads = how real sprites read).
    let quad = meshes.add(Rectangle::new(1.2, 1.8));
    let sprite = |c: Color| StandardMaterial {
        base_color: c,
        unlit: true,
        double_sided: true,
        cull_mode: None,
        alpha_mode: AlphaMode::Blend,
        ..default()
    };
    for (hx, hz, c) in [
        (0.0, 0.0, Color::srgb(0.45, 0.95, 0.55)),
        (-2.2, 1.6, Color::srgb(0.55, 0.75, 1.0)),
        (2.2, 1.3, Color::srgb(0.95, 0.65, 1.0)),
    ] {
        commands.spawn((
            Mesh3d(quad.clone()),
            MeshMaterial3d(mats.add(sprite(c))),
            Transform::from_xyz(hx, 0.9, hz),
            Billboard,
        ));
    }
    commands.spawn((
        Mesh3d(quad.clone()),
        MeshMaterial3d(mats.add(sprite(Color::srgb(0.95, 0.35, 0.35)))),
        Transform::from_xyz(6.0, 1.05, 2.0).with_scale(Vec3::splat(1.3)),
        Billboard,
    ));

    // On-screen readout.
    commands.spawn((
        Text::new(""),
        TextFont { font_size: 15.0, ..default() },
        TextColor(Color::srgb(0.9, 0.95, 1.0)),
        Node {
            position_type: PositionType::Absolute,
            top: Val::Px(8.0),
            left: Val::Px(10.0),
            ..default()
        },
        HudText,
    ));
}

#[allow(clippy::too_many_arguments)]
fn control(
    keys: Res<ButtonInput<KeyCode>>,
    time: Res<Time>,
    mut look: ResMut<Look>,
    mut commands: Commands,
    cam: Res<CamEntity>,
) {
    let dt = time.delta_secs();
    let held = |a: KeyCode, b: KeyCode| -> f32 {
        (keys.pressed(a) as i32 - keys.pressed(b) as i32) as f32
    };
    look.cam_pitch = (look.cam_pitch + held(KeyCode::ArrowUp, KeyCode::ArrowDown) * 25.0 * dt).clamp(5.0, 89.0);
    look.cam_yaw += held(KeyCode::ArrowRight, KeyCode::ArrowLeft) * 40.0 * dt;
    look.cam_dist = (look.cam_dist + held(KeyCode::KeyS, KeyCode::KeyW) * 12.0 * dt).clamp(6.0, 60.0);
    look.focus = (look.focus + held(KeyCode::KeyE, KeyCode::KeyQ) * 12.0 * dt).clamp(2.0, 80.0);
    look.aperture = (look.aperture + held(KeyCode::KeyD, KeyCode::KeyA) * 2.0 * dt).clamp(0.2, 16.0);
    look.bloom = (look.bloom + held(KeyCode::KeyX, KeyCode::KeyZ) * 0.4 * dt).clamp(0.0, 1.0);
    look.fog_start = (look.fog_start + held(KeyCode::KeyF, KeyCode::KeyR) * 25.0 * dt).clamp(0.0, 200.0);
    look.fog_end = (look.fog_end + held(KeyCode::KeyG, KeyCode::KeyT) * 25.0 * dt).clamp(5.0, 400.0);
    look.sun_pitch = (look.sun_pitch + held(KeyCode::KeyI, KeyCode::KeyK) * 30.0 * dt).clamp(5.0, 89.0);
    look.sun_yaw += held(KeyCode::KeyL, KeyCode::KeyJ) * 30.0 * dt;

    if keys.just_pressed(KeyCode::Digit0) {
        look.orbit = !look.orbit;
    }
    if keys.just_pressed(KeyCode::Digit1) {
        look.dof_on = !look.dof_on;
        if look.dof_on {
            commands.entity(cam.0).insert(dof_component(&look));
        } else {
            commands.entity(cam.0).remove::<DepthOfField>();
        }
    }
    if keys.just_pressed(KeyCode::Digit2) {
        look.bloom_on = !look.bloom_on;
        if look.bloom_on {
            commands.entity(cam.0).insert(bloom_component(&look));
        } else {
            commands.entity(cam.0).remove::<Bloom>();
        }
    }
    if keys.just_pressed(KeyCode::Digit3) {
        look.fog_on = !look.fog_on;
        if look.fog_on {
            commands.entity(cam.0).insert(fog_component(&look));
        } else {
            commands.entity(cam.0).remove::<DistanceFog>();
        }
    }
}

#[allow(clippy::type_complexity)]
fn apply(
    look: Res<Look>,
    time: Res<Time>,
    mut cam_q: Query<
        (
            &mut Transform,
            Option<&mut Bloom>,
            Option<&mut DepthOfField>,
            Option<&mut DistanceFog>,
        ),
        With<Camera3d>,
    >,
    mut sun_q: Query<&mut Transform, (With<DirectionalLight>, Without<Camera3d>)>,
) {
    let yaw = if look.orbit {
        look.cam_yaw + time.elapsed_secs() * 12.0
    } else {
        look.cam_yaw
    };
    let (yr, pr) = (yaw.to_radians(), look.cam_pitch.to_radians());
    let target = Vec3::new(0.0, 1.0, 0.0);
    let offset = Vec3::new(yr.sin() * pr.cos(), pr.sin(), yr.cos() * pr.cos()) * look.cam_dist;
    if let Ok((mut t, bloom, dof, fog)) = cam_q.single_mut() {
        t.translation = target + offset;
        t.look_at(target, Vec3::Y);
        if let Some(mut b) = bloom {
            b.intensity = look.bloom;
        }
        if let Some(mut d) = dof {
            d.focal_distance = look.focus;
            d.aperture_f_stops = look.aperture;
        }
        if let Some(mut f) = fog {
            f.falloff = FogFalloff::Linear {
                start: look.fog_start,
                end: look.fog_end,
            };
        }
    }
    if let Ok(mut t) = sun_q.single_mut() {
        *t = Transform::from_rotation(Quat::from_euler(
            EulerRot::YXZ,
            look.sun_yaw.to_radians(),
            -look.sun_pitch.to_radians(),
            0.0,
        ));
    }
}

fn billboard(
    cam_q: Query<&Transform, (With<Camera3d>, Without<Billboard>)>,
    mut q: Query<&mut Transform, With<Billboard>>,
) {
    let Ok(cam) = cam_q.single() else { return };
    for mut t in &mut q {
        let mut look_at = cam.translation;
        look_at.y = t.translation.y; // upright — yaw only
        let dir = (t.translation - look_at).normalize_or_zero();
        if dir.length_squared() > 0.0 {
            t.rotation = Quat::from_rotation_arc(Vec3::Z, dir);
        }
    }
}

fn hud(look: Res<Look>, mut q: Query<&mut Text, With<HudText>>) {
    let Ok(mut t) = q.single_mut() else { return };
    let onoff = |b: bool| if b { "on" } else { "off" };
    **t = format!(
        "HD-2D look-dev\n\
         camera  pitch {:.0}°  yaw {:.0}°  dist {:.0}   (↑↓ ←→ pitch/yaw, W/S dist)\n\
         DoF [{}]  focus {:.0}  aperture f/{:.1}   (Q/E focus, A/D blur, 1 toggle)\n\
         bloom [{}]  {:.2}   (Z/X, 2 toggle)\n\
         fog [{}]  start {:.0}  end {:.0}   (R/F start, T/G end, 3 toggle)\n\
         sun  pitch {:.0}°  yaw {:.0}°   (I/K pitch, J/L yaw)\n\
         orbit [{}]  (0 toggle)",
        look.cam_pitch, look.cam_yaw, look.cam_dist,
        onoff(look.dof_on), look.focus, look.aperture,
        onoff(look.bloom_on), look.bloom,
        onoff(look.fog_on), look.fog_start, look.fog_end,
        look.sun_pitch, look.sun_yaw,
        onoff(look.orbit),
    );
}
