//! Shared HD-2D render pipeline — the look-dev diorama (`bin/hd2d.rs`) and the
//! real game (`main.rs`) build the **same** lit-3D + post stack from these pieces:
//! HDR + Bloom + tilt-shift Depth-of-Field + distance Fog + a shadow-casting sun +
//! tonemapping, plus pixel-sprite **billboards** (nearest-sampled, alpha-masked,
//! camera-facing) with 8-direction facing and frame animation.
//!
//! The pipeline is tuned by eye on a native display and driven hands-free through a
//! file channel (`/tmp/meld-look.json` + a screenshot request file), so the look
//! can be iterated without recompiling. Native only — the post stack needs a real
//! GPU (WebGL2 can't do DoF/shadows).

use std::time::{Duration, SystemTime};

use bevy::core_pipeline::bloom::Bloom;
use bevy::core_pipeline::dof::{DepthOfField, DepthOfFieldMode};
use bevy::core_pipeline::tonemapping::Tonemapping;
use bevy::pbr::{DistanceFog, FogFalloff};
use bevy::prelude::*;
use bevy::render::view::window::screenshot::{save_to_disk, Screenshot};
use serde::{Deserialize, Serialize};

// ---- file control channel (so the look can be iterated hands-free) ----------
/// Live params: written here, hot-reloaded on mtime change (see [`reload_look`]).
/// Separate from the look-dev's `/tmp/meld-look.json` so the game keeps its own
/// gameplay framing (follow-cam angle, sprite size) independent of look-dev experiments.
pub const LOOK_FILE: &str = "/tmp/meld-game-look.json";
/// Touch this file to request a frame capture to [`AUTO_SHOT`]. Distinct from the
/// look-dev's request file so a running look-dev window doesn't answer the game's
/// captures (and vice-versa) when both watch the disk at once.
pub const SHOT_REQ: &str = "/tmp/meld-game-shot-request";
/// Where the requested capture lands — a plain PNG on disk.
pub const AUTO_SHOT: &str = "/tmp/meld-game-latest.png";

/// The diorama's night-blue base: clear colour + fill ambient (shared so the
/// look-dev and the game match).
pub const CLEAR: Color = Color::srgb(0.02, 0.03, 0.06);
pub fn ambient_light() -> AmbientLight {
    AmbientLight {
        color: Color::srgb(0.6, 0.7, 0.95),
        brightness: 220.0,
        ..default()
    }
}

/// Every knob the HD-2D look exposes. Tuned live from the keyboard (look-dev) or
/// via [`LOOK_FILE`]; the same file drives both binaries so tuning carries over.
#[derive(Resource, Serialize, Deserialize, Clone)]
#[serde(default)]
pub struct Look {
    pub cam_pitch: f32, // degrees the camera tilts down toward the scene
    pub cam_yaw: f32,   // degrees around the target
    pub cam_dist: f32,  // distance from the focus point
    pub focus: f32,     // DoF focal distance (what stays sharp)
    pub aperture: f32,  // DoF f-stops — LOWER = more (tilt-shift) blur
    pub bloom: f32,     // bloom intensity
    pub fog_start: f32,
    pub fog_end: f32,
    pub sun_pitch: f32,
    pub sun_yaw: f32,
    pub orbit: bool,
    pub dof_on: bool,
    pub bloom_on: bool,
    pub fog_on: bool,
    pub sprite_y: f32,     // world-y of a billboard's centre (grounds the padded sprite)
    pub sprite_scale: f32, // uniform scale of the sprite quads
    // A NARROW fov (telephoto) gives the HD-2D miniature compression AND makes DoF
    // shallow; a LARGER dof_sensor lengthens the virtual lens for stronger blur.
    pub fov: f32,
    pub dof_sensor: f32,
    pub anim_fps: f32, // walk-cycle playback speed
}

impl Default for Look {
    fn default() -> Self {
        // A first-guess HD-2D framing: a steep look-down, strong tilt-shift, warm
        // bloom. Tuned further via LOOK_FILE.
        Look {
            // A shallower pitch keeps the horizon (and the sky + clouds) in frame and
            // views the 2D sprites more head-on. Yaw 0 aligns the camera with the
            // world axes, so cardinal movement (WASD) lands squarely on the N/S/E/W
            // sprites instead of drifting onto the diagonals. Orbit from here for an
            // angled look (facing stays camera-relative, so it keeps up).
            cam_pitch: 21.0,
            cam_yaw: 0.0,
            cam_dist: 26.0,
            focus: 26.0,   // track cam_dist so the followed hero stays sharp
            aperture: 3.5, // subtle tilt-shift — blur the far field, keep play sharp
            bloom: 0.28,
            fog_start: 110.0,
            fog_end: 520.0, // extended draw distance (cheap: ground is one mesh)
            sun_pitch: 55.0,
            sun_yaw: 40.0,
            orbit: false,
            dof_on: true,
            bloom_on: true,
            fog_on: true,
            sprite_y: 0.72,     // grounds the padded sprite at sprite_scale ≈ 1.6
            sprite_scale: 1.6,  // hero reads prominently in the diorama
            fov: 36.0,
            dof_sensor: 0.05,
            anim_fps: 10.0,
        }
    }
}

/// Last-seen mtime of [`LOOK_FILE`], to detect external edits.
#[derive(Resource, Default)]
pub struct LookWatch(pub Option<SystemTime>);

/// Tag: rotate to face the camera (yaw only) each frame — see [`billboard`].
#[derive(Component)]
pub struct Billboard;

/// Tag: a character sprite billboard whose footing + size are driven live from the
/// `Look` (see [`place_billboards`]). Distinct from [`Billboard`] so trees/clouds
/// (which also billboard) keep their own spawn scale + height.
#[derive(Component)]
pub struct HeroBillboard;

// ---- post-stack component builders ------------------------------------------

pub fn bloom_component(look: &Look) -> Bloom {
    let mut b = Bloom::NATURAL;
    b.intensity = look.bloom;
    b
}

pub fn dof_component(look: &Look) -> DepthOfField {
    DepthOfField {
        mode: DepthOfFieldMode::Bokeh,
        focal_distance: look.focus,
        aperture_f_stops: look.aperture,
        sensor_height: look.dof_sensor,
        max_circle_of_confusion_diameter: 64.0,
        max_depth: f32::INFINITY,
    }
}

pub fn fog_component(look: &Look) -> DistanceFog {
    DistanceFog {
        // Daytime haze — the ground fades into the sky-blue at the horizon.
        color: Color::srgb(0.62, 0.76, 0.92),
        falloff: FogFalloff::Linear {
            start: look.fog_start,
            end: look.fog_end,
        },
        ..default()
    }
}

/// Spawn the HD-2D camera (HDR + tonemap + the full post stack) and return it.
pub fn spawn_camera(commands: &mut Commands, look: &Look, initial: Transform) -> Entity {
    commands
        .spawn((
            Camera3d::default(),
            Camera { hdr: true, ..default() },
            Tonemapping::TonyMcMapface,
            bloom_component(look),
            dof_component(look),
            fog_component(look),
            initial,
        ))
        .id()
}

/// Spawn the shadow-casting sun.
pub fn spawn_sun(commands: &mut Commands, look: &Look) {
    commands.spawn((
        DirectionalLight {
            illuminance: 9000.0,
            shadows_enabled: true,
            color: Color::srgb(1.0, 0.96, 0.85),
            ..default()
        },
        sun_transform(look),
    ));
}

/// Camera transform orbiting `target` per the `Look` (auto-orbits when enabled).
pub fn camera_transform(look: &Look, target: Vec3, elapsed: f32) -> Transform {
    let yaw = if look.orbit {
        look.cam_yaw + elapsed * 12.0
    } else {
        look.cam_yaw
    };
    let (yr, pr) = (yaw.to_radians(), look.cam_pitch.to_radians());
    let offset = Vec3::new(yr.sin() * pr.cos(), pr.sin(), yr.cos() * pr.cos()) * look.cam_dist;
    let mut t = Transform::from_translation(target + offset);
    t.look_at(target, Vec3::Y);
    t
}

pub fn sun_transform(look: &Look) -> Transform {
    Transform::from_rotation(Quat::from_euler(
        EulerRot::YXZ,
        look.sun_yaw.to_radians(),
        -look.sun_pitch.to_radians(),
        0.0,
    ))
}

/// Push the `Look`'s post params into the live camera components + projection fov.
pub fn apply_post(
    look: &Look,
    proj: &mut Projection,
    bloom: Option<&mut Bloom>,
    dof: Option<&mut DepthOfField>,
    fog: Option<&mut DistanceFog>,
) {
    if let Projection::Perspective(p) = proj {
        p.fov = look.fov.to_radians();
    }
    if let Some(b) = bloom {
        b.intensity = look.bloom;
    }
    if let Some(d) = dof {
        d.focal_distance = look.focus;
        d.aperture_f_stops = look.aperture;
        d.sensor_height = look.dof_sensor;
    }
    if let Some(f) = fog {
        f.falloff = FogFalloff::Linear {
            start: look.fog_start,
            end: look.fog_end,
        };
    }
}

/// Seed the [`LOOK_FILE`] template if absent, so tuning persists across restarts.
pub fn seed_look_file(look: &Look) {
    if std::fs::metadata(LOOK_FILE).is_err() {
        if let Ok(s) = serde_json::to_string_pretty(look) {
            let _ = std::fs::write(LOOK_FILE, s);
        }
    }
}

/// Reload `Look` from [`LOOK_FILE`] when it changes on disk; returns true if it did.
pub fn reload_look(look: &mut Look, watch: &mut LookWatch) -> bool {
    if let Ok(meta) = std::fs::metadata(LOOK_FILE) {
        let mtime = meta.modified().ok();
        if mtime != watch.0 {
            watch.0 = mtime;
            if let Ok(s) = std::fs::read_to_string(LOOK_FILE) {
                match serde_json::from_str::<Look>(&s) {
                    Ok(new) => {
                        *look = new;
                        return true;
                    }
                    Err(e) => warn!("bad {LOOK_FILE}: {e}"),
                }
            }
        }
    }
    false
}

/// Capture the window to [`AUTO_SHOT`] if a screenshot was requested via [`SHOT_REQ`].
pub fn maybe_screenshot(commands: &mut Commands) {
    if std::fs::metadata(SHOT_REQ).is_ok() {
        let _ = std::fs::remove_file(SHOT_REQ);
        commands
            .spawn(Screenshot::primary_window())
            .observe(save_to_disk(AUTO_SHOT));
    }
}

/// System: ground + scale every [`HeroBillboard`] from the live `Look` (so the hero
/// footing + size stay tunable by eye). World props/monsters use only [`Billboard`]
/// and keep their own spawn-baked scale/height. Sets local translation.y + scale
/// only; [`billboard`] sets rotation, so they don't fight.
pub fn place_billboards(look: Res<Look>, mut q: Query<&mut Transform, With<HeroBillboard>>) {
    for mut t in &mut q {
        t.translation.y = look.sprite_y;
        t.scale = Vec3::splat(look.sprite_scale);
    }
}

/// System: rotate every [`Billboard`] to face the camera (yaw only, stays upright).
pub fn billboard(
    cam_q: Query<&Transform, (With<Camera3d>, Without<Billboard>)>,
    mut q: Query<&mut Transform, With<Billboard>>,
) {
    let Ok(cam) = cam_q.single() else { return };
    for mut t in &mut q {
        let mut at = cam.translation;
        at.y = t.translation.y; // upright — yaw only
        // Face the quad's FRONT (+Z) toward the camera. Using the away-vector would
        // show the back face — mirroring the sprite so side facings point the wrong
        // way (a right-facing "east" sprite reads as walking left). Toward-camera
        // also keeps the cylindrical lighting normals bowed at the viewer.
        let dir = (at - t.translation).normalize_or_zero();
        if dir.length_squared() > 0.0 {
            t.rotation = Quat::from_rotation_arc(Vec3::Z, dir);
        }
    }
}

// ---- 8-direction pixel-sprite characters ------------------------------------

/// Sprite facings, clockwise from front. Index 0 (`south`) faces +Z — toward a
/// camera parked behind the subject — so a subject walking +Z shows its front.
pub const DIRS: [&str; 8] = [
    "south",
    "south-east",
    "east",
    "north-east",
    "north",
    "north-west",
    "west",
    "south-west",
];

/// Map a heading to the nearest of the 8 [`DIRS`]. Pass `(screen_right, toward_cam)`
/// for camera-relative facing: `+y` = toward the viewer → index 0 (`south`/front),
/// `+x` = screen-right → index 2 (`east`).
pub fn dir_index(h: Vec2) -> usize {
    if h.length_squared() < 1e-9 {
        return 0;
    }
    let mut a = h.x.atan2(h.y).to_degrees(); // +y = toward viewer → 0° = south (front)
    if a < 0.0 {
        a += 360.0;
    }
    ((a / 45.0).round() as usize) % 8
}

/// Loaded frame handles for one character: an idle rotation + a walk clip per dir.
/// Follows a character folder's on-disk layout: `<base>/rotations/<dir>.png` and
/// `<base>/animations/<anim>/<dir>/frame_NNN.png` (the same shape `metadata.json`
/// describes, though its `folder` prefix can go stale after a re-export, so we key
/// off the real directory instead).
#[derive(Clone)]
pub struct CharacterFrames {
    pub idle: [Handle<Image>; 8],
    pub walk: [Vec<Handle<Image>>; 8],
}

/// Load a character's sprites from its asset folder. `base` is the character dir
/// (e.g. `characters/PSYKER_Male/Psyker`), `anim` the walk clip, `frame_count` its
/// length.
pub fn load_character(
    assets: &AssetServer,
    base: &str,
    anim: &str,
    frame_count: usize,
) -> CharacterFrames {
    let idle = std::array::from_fn(|i| assets.load(format!("{base}/rotations/{}.png", DIRS[i])));
    let walk = std::array::from_fn(|i| {
        (0..frame_count)
            .map(|f| {
                assets.load(format!(
                    "{base}/animations/{anim}/{}/frame_{f:03}.png",
                    DIRS[i]
                ))
            })
            .collect()
    });
    CharacterFrames { idle, walk }
}

/// A movement-driven character billboard: it walks (cycles the clip) while its
/// entity moves and faces its heading; idles otherwise. Put it on the entity root
/// (which moves); it drives its billboard child's material `mat`.
///
/// `facing` is stored in **world** space, and the rendered sprite is whichever of
/// the 8 rotations best matches that heading *as seen from the current camera* — so
/// orbiting the camera around a standing character reveals its other sides (the 3D
/// illusion) without the character turning.
#[derive(Component)]
pub struct CharSprite {
    pub frames: CharacterFrames,
    pub mat: Handle<StandardMaterial>,
    pub timer: Timer,
    pub frame: usize,
    pub facing: Vec2, // world-space heading (xz) the character faces
    pub last: Vec3,
    pub still: f32, // seconds since last movement (grace against snapshot gaps)
}

impl CharSprite {
    pub fn new(frames: CharacterFrames, mat: Handle<StandardMaterial>, start: Vec3) -> Self {
        CharSprite {
            frames,
            mat,
            timer: Timer::from_seconds(0.1, TimerMode::Repeating),
            frame: 0,
            facing: Vec2::new(0.0, 1.0), // world south (+Z) — faces a yaw-0 camera
            last: start,
            still: 1.0, // start idle
        }
    }
}

/// System: advance each [`CharSprite`] from its root's movement and swap its
/// material's texture to the right frame/facing. Server snapshots arrive at a
/// lower rate than render frames, so a short `still` grace keeps the walk cycle
/// playing across the gaps rather than stuttering to idle every other frame.
pub fn animate_chars(
    time: Res<Time>,
    look: Res<Look>,
    cam_q: Query<&GlobalTransform, With<Camera3d>>,
    mut mats: ResMut<Assets<StandardMaterial>>,
    mut q: Query<(&Transform, &mut CharSprite)>,
) {
    let dt = time.delta_secs();
    let fps = look.anim_fps.max(1.0);
    // Facing is chosen **relative to the camera**, not the world: a character
    // walking toward the viewer shows its front, one walking into the screen its
    // back — regardless of how the camera is orbited. Project the camera's forward
    // and right onto the ground plane for the screen-space basis.
    let (fwd, right) = match cam_q.single() {
        Ok(cam) => {
            let f = Vec3::from(cam.forward());
            let r = Vec3::from(cam.right());
            (
                Vec2::new(f.x, f.z).normalize_or_zero(),
                Vec2::new(r.x, r.z).normalize_or_zero(),
            )
        }
        Err(_) => (Vec2::new(0.0, -1.0), Vec2::new(1.0, 0.0)),
    };
    for (tf, mut cs) in &mut q {
        let pos = tf.translation;
        let d = pos - cs.last;
        cs.last = pos;
        let horiz = Vec2::new(d.x, d.z);
        // Update the WORLD facing only while actually moving (a small threshold so
        // smoothed near-stop jitter doesn't spin it).
        if horiz.length() > 2e-3 {
            cs.facing = horiz.normalize();
            cs.still = 0.0;
        } else {
            cs.still += dt;
        }
        // Pick the sprite for the world facing *as seen from the camera* — this is
        // what makes the character look 3D when you orbit: same facing, new side.
        let toward_cam = -cs.facing.dot(fwd); // + = facing the viewer (front)
        let screen_right = cs.facing.dot(right);
        let dir = dir_index(Vec2::new(screen_right, toward_cam));

        cs.timer.set_duration(Duration::from_secs_f32(1.0 / fps));
        cs.timer.tick(time.delta());
        if cs.timer.just_finished() {
            cs.frame = cs.frame.wrapping_add(1);
        }
        let walking = cs.still < 0.2;
        let tex = if walking {
            let clip = &cs.frames.walk[dir];
            if clip.is_empty() {
                cs.frames.idle[dir].clone()
            } else {
                clip[cs.frame % clip.len()].clone()
            }
        } else {
            cs.frames.idle[dir].clone()
        };
        if let Some(m) = mats.get_mut(&cs.mat) {
            m.base_color_texture = Some(tex);
        }
    }
}

/// Build the **lit**, alpha-masked, double-sided material a sprite billboard uses.
/// `tint` multiplies the texture (a cheap palette swap); `tex` is the first frame.
/// Lit (not unlit) so the sun + ambient model the sprite — paired with the
/// cylindrical normals of [`cyl_billboard_mesh`], a flat sprite reads rounded
/// instead of like a paper cut-out (the HD-2D depth trick, no new art needed).
pub fn sprite_material(tint: Color, tex: Handle<Image>) -> StandardMaterial {
    StandardMaterial {
        base_color: tint,
        base_color_texture: Some(tex),
        perceptual_roughness: 0.95,
        metallic: 0.0,
        reflectance: 0.05, // matte — no specular hotspot sliding across the sprite
        double_sided: true,
        cull_mode: None,
        alpha_mode: AlphaMode::Mask(0.5),
        ..default()
    }
}

/// A flat sprite quad whose **normals fan outward** across its width like a
/// vertical half-cylinder (positions stay planar). Under directional light this
/// shades the sprite left-to-dark-to-right, giving a flat billboard volumetric
/// form — the cheap HD-2D "impostor" depth trick. `arc_deg` is the total bow
/// (≈50-70° reads rounded without wrapping too hard).
pub fn cyl_billboard_mesh(w: f32, h: f32, cols: usize, arc_deg: f32) -> Mesh {
    use bevy::render::mesh::{Indices, PrimitiveTopology};
    use bevy::render::render_asset::RenderAssetUsages;

    let cols = cols.max(1);
    let arc = arc_deg.to_radians();
    let mut positions = Vec::with_capacity((cols + 1) * 2);
    let mut normals = Vec::with_capacity((cols + 1) * 2);
    let mut uvs = Vec::with_capacity((cols + 1) * 2);
    let mut indices = Vec::with_capacity(cols * 6);
    for i in 0..=cols {
        let t = i as f32 / cols as f32; // 0..1 across the width
        let x = (t - 0.5) * w;
        let ang = (t - 0.5) * arc; // fan the normal from -arc/2 .. +arc/2
        let n = [ang.sin(), 0.0, ang.cos()];
        positions.push([x, h * 0.5, 0.0]);
        positions.push([x, -h * 0.5, 0.0]);
        normals.push(n);
        normals.push(n);
        uvs.push([t, 0.0]);
        uvs.push([t, 1.0]);
    }
    for i in 0..cols {
        let (a, b, c, d) = (
            (i * 2) as u32,
            (i * 2 + 1) as u32,
            (i * 2 + 2) as u32,
            (i * 2 + 3) as u32,
        );
        indices.extend_from_slice(&[a, b, c, c, b, d]);
    }
    let mut mesh = Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::default(),
    );
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
    mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, normals);
    mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, uvs);
    mesh.insert_indices(Indices::U32(indices));
    mesh
}

/// A soft round white sprite (radial alpha falloff, 1 at centre → 0 at the rim) —
/// a cheap cloud/glow puff needing no art. Use on an unlit alpha-blended billboard.
pub fn soft_disc_texture(size: u32) -> Image {
    use bevy::render::render_asset::RenderAssetUsages;
    use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
    let s = size.max(4);
    let c = s as f32 / 2.0;
    let mut data = vec![0u8; (s * s * 4) as usize];
    for y in 0..s {
        for x in 0..s {
            let dx = (x as f32 + 0.5 - c) / c;
            let dy = (y as f32 + 0.5 - c) / c;
            let r = (dx * dx + dy * dy).sqrt();
            let a = (1.0 - r).clamp(0.0, 1.0);
            let a = a * a; // soft feathered edge
            let i = ((y * s + x) * 4) as usize;
            data[i] = 255;
            data[i + 1] = 255;
            data[i + 2] = 255;
            data[i + 3] = (a * 255.0) as u8;
        }
    }
    Image::new(
        Extent3d { width: s, height: s, depth_or_array_layers: 1 },
        TextureDimension::D2,
        data,
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::default(),
    )
}

/// Cheap value-noise hash in [0,1).
fn hash2(x: u32, y: u32) -> f32 {
    let mut h = x.wrapping_mul(374761393).wrapping_add(y.wrapping_mul(668265263));
    h = (h ^ (h >> 13)).wrapping_mul(1274126177);
    ((h ^ (h >> 16)) & 0xffff) as f32 / 65535.0
}

fn repeat_sampler(linear: bool) -> bevy::image::ImageSampler {
    use bevy::image::{ImageAddressMode, ImageFilterMode, ImageSampler, ImageSamplerDescriptor};
    let f = if linear {
        ImageFilterMode::Linear
    } else {
        ImageFilterMode::Nearest
    };
    ImageSampler::Descriptor(ImageSamplerDescriptor {
        address_mode_u: ImageAddressMode::Repeat,
        address_mode_v: ImageAddressMode::Repeat,
        mag_filter: f,
        min_filter: f,
        ..default()
    })
}

fn make_tex(s: u32, data: Vec<u8>, linear: bool) -> Image {
    use bevy::render::render_asset::RenderAssetUsages;
    use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
    let mut img = Image::new(
        Extent3d { width: s, height: s, depth_or_array_layers: 1 },
        TextureDimension::D2,
        data,
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::default(),
    );
    img.sampler = repeat_sampler(linear);
    img
}

/// A **grayscale** tiling grass detail (luminance variation + blade streaks) — the
/// ground material's `base_color` (per biome) tints it. Repeat-sampled for tiling.
pub fn grass_texture(size: u32) -> Image {
    let s = size.max(8);
    let mut data = vec![0u8; (s * s * 4) as usize];
    for y in 0..s {
        for x in 0..s {
            // A couple of noise octaves + faint vertical blades.
            let n = hash2(x, y) * 0.45
                + hash2(x / 2, y / 3) * 0.35
                + hash2(x / 5, y / 6) * 0.20;
            let blade = ((x as f32 * 0.9).sin() * 0.5 + 0.5) * 0.12;
            let l = (0.62 + n * 0.42 + blade - 0.06).clamp(0.35, 1.0);
            // Slightly green-biased so shaded ground still reads leafy.
            let v = |m: f32| ((l * m) * 255.0) as u8;
            let i = ((y * s + x) * 4) as usize;
            data[i] = v(0.9);
            data[i + 1] = v(1.0);
            data[i + 2] = v(0.82);
            data[i + 3] = 255;
        }
    }
    make_tex(s, data, true)
}

/// A tiling water ripple (soft interfering waves), scrolled + tinted by the water
/// material. Repeat-sampled.
pub fn water_ripple_texture(size: u32) -> Image {
    use std::f32::consts::TAU;
    let s = size.max(8);
    let mut data = vec![0u8; (s * s * 4) as usize];
    for y in 0..s {
        for x in 0..s {
            let u = x as f32 / s as f32;
            let v = y as f32 / s as f32;
            // Sum of a few sine waves → seamless because frequencies are integers.
            let w = (((u * 3.0 + v * 2.0) * TAU).sin()
                + ((u * 2.0 - v * 4.0) * TAU).sin()
                + ((u * 5.0 + v * 5.0) * TAU).sin() * 0.5)
                / 2.5;
            let l = (0.6 + w * 0.4).clamp(0.2, 1.0);
            let hi = ((w * 0.5 + 0.5).powf(6.0) * 255.0) as u8; // sparkle in alpha-ish
            let i = ((y * s + x) * 4) as usize;
            data[i] = (l * 0.7 * 255.0) as u8;
            data[i + 1] = (l * 0.88 * 255.0) as u8;
            data[i + 2] = (l * 255.0) as u8;
            data[i + 3] = 210 + (hi / 6);
        }
    }
    make_tex(s, data, true)
}

/// A flat **irregular blob** (triangle fan whose radius wobbles with angle) so pools
/// don't read as perfect circles. Lies in the XY plane — rotate it flat like a
/// `Circle`. Spin each instance around Y for variety.
pub fn blob_mesh(sides: usize) -> Mesh {
    use bevy::render::mesh::{Indices, PrimitiveTopology};
    use bevy::render::render_asset::RenderAssetUsages;
    use std::f32::consts::TAU;
    let n = sides.max(8);
    let mut positions = vec![[0.0f32, 0.0, 0.0]];
    let mut normals = vec![[0.0f32, 0.0, 1.0]];
    let mut uvs = vec![[0.5f32, 0.5]];
    for i in 0..n {
        let a = i as f32 / n as f32 * TAU;
        // A couple of low-frequency lobes → a natural, lumpy shoreline.
        let r = 0.78 + 0.16 * (a * 2.0 + 0.7).sin() + 0.10 * (a * 3.0 + 2.1).sin()
            + 0.05 * (a * 5.0).sin();
        positions.push([a.cos() * r, a.sin() * r, 0.0]);
        normals.push([0.0, 0.0, 1.0]);
        uvs.push([a.cos() * 0.5 * r + 0.5, a.sin() * 0.5 * r + 0.5]);
    }
    let mut indices = Vec::with_capacity(n * 3);
    for i in 0..n {
        indices.extend_from_slice(&[0, 1 + i as u32, 1 + ((i + 1) % n) as u32]);
    }
    let mut mesh = Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::default());
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
    mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, normals);
    mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, uvs);
    mesh.insert_indices(Indices::U32(indices));
    mesh
}

/// A **puffy cloud** silhouette (union of several soft lobes) rather than one round
/// disc, so clouds + their shadows read organically. White with alpha in the shape.
pub fn cloud_texture(size: u32) -> Image {
    let s = size.max(16);
    let mut data = vec![0u8; (s * s * 4) as usize];
    // Overlapping soft circles (cx, cy, radius) forming a lumpy cloud.
    let lobes = [
        (0.50, 0.58, 0.30),
        (0.30, 0.62, 0.20),
        (0.70, 0.60, 0.21),
        (0.40, 0.48, 0.22),
        (0.62, 0.46, 0.20),
        (0.50, 0.40, 0.16),
    ];
    for y in 0..s {
        for x in 0..s {
            let u = x as f32 / s as f32;
            let v = y as f32 / s as f32;
            let mut a = 0.0f32;
            for (cx, cy, r) in lobes {
                let d = (((u - cx).powi(2) + (v - cy).powi(2)).sqrt() / r).min(1.0);
                a += 1.0 - d;
            }
            let a = (a * 0.85).clamp(0.0, 1.0);
            let a = a * a; // feathered edge
            let i = ((y * s + x) * 4) as usize;
            data[i] = 255;
            data[i + 1] = 255;
            data[i + 2] = 255;
            data[i + 3] = (a * 255.0) as u8;
        }
    }
    make_tex(s, data, true)
}

/// A soft round contact-shadow material (blended dark disc) to ground billboards,
/// which the sun's real shadows can't touch (they're unlit).
pub fn contact_shadow_material() -> StandardMaterial {
    StandardMaterial {
        base_color: Color::srgba(0.0, 0.0, 0.0, 0.35),
        unlit: true,
        alpha_mode: AlphaMode::Blend,
        ..default()
    }
}
