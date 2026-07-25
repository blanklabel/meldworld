//! World rendering + scene setup: asset loading, the biome ground shader,
//! sky/day-night, weather (rain/ashfall), clouds, ground detail, water.
//! Extracted from `main.rs` during the module reorg.

use std::collections::HashMap;

use bevy::prelude::*;
use bevy::gltf::GltfAssetLabel;
use bevy::pbr::{ExtendedMaterial, MaterialExtension};
use bevy::render::render_resource::{AsBindGroup, ShaderRef, ShaderType};

use meld_client::hd2d::{self, CharacterFrames};

use super::*;

/// Max radial biome rings the ground shader blends across (near the player). A
/// section = one concentric ring, so this bounds how many nearby sections colour the
/// visible ground; deeper/closer sections beyond the window clamp to the ends.
pub(crate) const MAX_BIOME_RINGS: usize = 32;

/// Uniform for [`GroundBiome`] — the ACTUAL per-section biome rings, so the ground
/// matches each section's real biome (radius ring) instead of fixed distance bands.
/// `rings[i] = (outer_radius, biome_index, _, _)`, sorted by radius; `count` entries
/// are live. `update_ground_biome_rings` rebuilds it from the streamed sections.
#[derive(Clone, Copy, ShaderType, Debug)]
pub(crate) struct BiomeParams {
    rings: [Vec4; MAX_BIOME_RINGS],
    count: u32,
    uv_scale: f32,
    blend_half: f32,
    _pad0: f32,
}

impl Default for BiomeParams {
    fn default() -> Self {
        BiomeParams {
            rings: [Vec4::ZERO; MAX_BIOME_RINGS],
            count: 0,
            uv_scale: 1.0 / 3.0,
            blend_half: 18.0,
            _pad0: 0.0,
        }
    }
}

/// Ground material extension: blends the five biome ground textures by the fragment's
/// world position so biome transitions fade in ahead of the player (see
/// `assets/shaders/ground_biome.wgsl`). Replaces the old single-plane texture swap.
#[derive(Asset, TypePath, AsBindGroup, Clone)]
pub(crate) struct GroundBiome {
    #[texture(100)]
    #[sampler(105)]
    forest: Handle<Image>,
    #[texture(101)]
    desert: Handle<Image>,
    #[texture(102)]
    ashfall: Handle<Image>,
    #[texture(103)]
    tundra: Handle<Image>,
    #[texture(104)]
    mire: Handle<Image>,
    #[uniform(106)]
    params: BiomeParams,
}

impl MaterialExtension for GroundBiome {
    fn fragment_shader() -> ShaderRef {
        "shaders/ground_biome.wgsl".into()
    }
}

/// The blended-biome ground material type (StandardMaterial lighting + our extension).
pub(crate) type GroundMat = ExtendedMaterial<StandardMaterial, GroundBiome>;

/// Shared meshes/materials + the psyker sprite set, built once at startup so the
/// overworld sync can spawn 3D entities without rebuilding assets each frame.
#[derive(Resource)]
pub(crate) struct WorldAssets {
    pub(crate) psyker: CharacterFrames,
    /// The martial sprite (used by the Hunter and any class without its own art).
    /// The on-disk art folder is still named `Squire`.
    pub(crate) hunter: CharacterFrames,
    pub(crate) sprite_quad: Handle<Mesh>,
    /// Cropped billboard showing only head→torso — the back-row "bust" (see
    /// [`hd2d::bust_billboard_mesh`]).
    pub(crate) bust_quad: Handle<Mesh>,
    pub(crate) shadow_mesh: Handle<Mesh>,
    pub(crate) shadow_mat: Handle<StandardMaterial>,
    /// CC0 pixel-art creature billboards keyed by creature content id (see
    /// `meld-world::creatures_for_biome`); unknown kinds fall back to [`Self::monster_pool`].
    /// Creatures stay 2D sprites — the HD-2D convention (2D actors, 3D world).
    pub(crate) monster_sprites: HashMap<String, Handle<Image>>,
    pub(crate) monster_pool: Vec<Handle<Image>>,
    /// Real 3D prop models (Kenney Nature Kit, CC0) keyed by terrain-obstacle kind →
    /// several `(scene, baked_scale)` variants (picked per-entity by id hash), so the
    /// world is built from actual geometry instead of flat billboards.
    pub(crate) prop_scenes: HashMap<String, Vec<(Handle<Scene>, f32)>>,
    /// 3D harvest-node models keyed by resource content id → `(scene, baked_scale)`.
    pub(crate) resource_scenes: HashMap<String, (Handle<Scene>, f32)>,
    pub(crate) portal_sprite: Handle<Image>,
    pub(crate) portal_mesh: Handle<Mesh>,
    pub(crate) portal_mat: Handle<StandardMaterial>,
    /// Floating "target" marker: a small faceted diamond that hovers, bounces, and
    /// slowly spins over the currently-targeted enemy (see [`highlight_target`]).
    /// Shared across enemies; per-enemy `Visibility` gates which one shows.
    pub(crate) target_diamond_mesh: Handle<Mesh>,
    pub(crate) target_diamond_mat: Handle<StandardMaterial>,
    // Capsule stand-in for enemies in the HD-2D battle diorama (PR #21); the
    // overworld uses creature billboards from `monster_sprites` instead.
    pub(crate) monster_mesh: Handle<Mesh>,
    pub(crate) rock_mesh: Handle<Mesh>,
    pub(crate) water_mesh: Handle<Mesh>,
    pub(crate) water_mat: Handle<StandardMaterial>, // shared, animated (see animate_water)
    pub(crate) ground_tex: Vec<Handle<Image>>, // per-biome textures; also dress terrace tops/cliffs
}

/// Biome → index into `WorldAssets::ground_tex` (Forest/Desert/Ashfall/Tundra/Mire).
pub(crate) fn biome_index(d: i64) -> usize {
    match biome_display(d) {
        "Forest" => 0,
        "Desert" => 1,
        "Ashfall" => 2,
        "Tundra" => 3,
        _ => 4,
    }
}

/// Load an image with a Repeat sampler so it tiles across the big ground plane.
pub(crate) fn load_tiled(assets: &AssetServer, path: &str) -> Handle<Image> {
    assets.load_with_settings(path, |s: &mut ImageLoaderSettings| {
        s.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
            address_mode_u: ImageAddressMode::Repeat,
            address_mode_v: ImageAddressMode::Repeat,
            ..ImageSamplerDescriptor::nearest()
        });
    })
}

/// Build the HD-2D world: camera + post stack, sun, the lit ground, and the shared
/// asset handles. Replaces the old flat Camera2d overworld (CANON D16 all-Bevy).
pub(crate) fn setup(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut mats: ResMut<Assets<StandardMaterial>>,
    mut ground_mats: ResMut<Assets<GroundMat>>,
    mut images: ResMut<Assets<Image>>,
    assets: Res<AssetServer>,
    look: Res<hd2d::Look>,
) {
    hd2d::seed_look_file(&look);

    // Camera parked at a nice diorama angle for the menu screens; `hd2d_follow`
    // re-aims it at the player once in the overworld.
    let cam_tf = hd2d::camera_transform(&look, Vec3::new(0.0, 1.0, 0.0), 0.0);
    hd2d::spawn_camera(&mut commands, &look, cam_tf);
    hd2d::spawn_sun(&mut commands, &look);

    // The lit ground: one big plane wearing a tiled CC0 grass texture, tinted per
    // biome by `hd2d_ground_color`. The grass PNG must repeat (default sampler
    // clamps), so load it with a Repeat address mode; `uv_transform` scales the
    // plane's 0..1 UVs up so each tile is ~3 world units (nearest-sampled → crisp).
    // Per-biome ground textures (green grass in the forest, sand in the desert, …);
    // `hd2d_follow` swaps the material's texture as you cross biomes.
    let ground_tex: Vec<Handle<Image>> = [
        "ground/grass0.png",     // Forest
        "ground/sand.png",       // Desert
        "ground/dirt_full.png",  // Ashfall
        "ground/grass_dark0.png", // Tundra (tinted frost-blue)
        "ground/moss.png",       // Mire
    ]
    .iter()
    .map(|p| load_tiled(&assets, p))
    .collect();
    // The ground is ONE plane wearing a biome-blending shader (`GroundBiome`): it
    // picks the biome from each fragment's world position and cross-fades between
    // adjacent biome textures across a band around every boundary, so the next biome
    // fades in ahead of you as you approach it (corridor transitions) instead of the
    // whole floor snapping when you cross the line. Boundaries mirror the server's
    // `biome_for_distance` (radial distance); the ~36-unit band gives a gradual fade.
    let ground_mat = ground_mats.add(GroundMat {
        base: StandardMaterial {
            // The shader multiplies the blended biome texture by this — keep it white
            // so the textures read true; lighting/shadow come from StandardMaterial.
            base_color: Color::WHITE,
            perceptual_roughness: 0.95,
            ..default()
        },
        extension: GroundBiome {
            forest: ground_tex[0].clone(),
            desert: ground_tex[1].clone(),
            ashfall: ground_tex[2].clone(),
            tundra: ground_tex[3].clone(),
            mire: ground_tex[4].clone(),
            // Rings start empty; `update_ground_biome_rings` fills them from the
            // streamed sections each frame (count 0 ⇒ shader falls back to forest).
            params: BiomeParams::default(),
        },
    });
    commands.spawn((
        WorldGround,
        // Square (was 2000×600, a corridor) so the WG-4 radial fan has ground in
        // every direction the player roams.
        Mesh3d(meshes.add(Plane3d::default().mesh().size(2000.0, 2000.0))),
        MeshMaterial3d(ground_mat.clone()),
        Transform::default(),
    ));

    // Shared assets. HD-2D split: 2D pixel sprites for the actors (heroes + monster
    // billboards, from DCSS/RLTiles — public domain), real 3D models for the world
    // (obstacles + harvest nodes, from Kenney Nature Kit — CC0). See assets/ATTRIBUTIONS.md.
    let ld = |p: &str| assets.load::<Image>(p);
    // Creature content id → billboard (biome-appropriate). Kinds come from
    // `meld-world::creatures_for_biome`.
    let monster_sprites: HashMap<String, Handle<Image>> = [
        ("forest_bloom_stalker", "monsters/wolf_spider.png"),
        ("thornback_boar", "monsters/hog.png"),
        ("dune_wyrm", "monsters/wyvern.png"),
        ("sand_shade", "monsters/wraith.png"),
        ("cinder_imp", "monsters/salamander.png"),
        ("magma_golem", "monsters/ogre.png"),
        ("frost_lurker", "monsters/wolf.png"),
        ("ice_revenant", "monsters/skeletal_warrior.png"),
        ("bog_serpent", "monsters/adder.png"),
        ("myconid_brute", "monsters/troll.png"),
    ]
    .into_iter()
    .map(|(k, p)| (k.to_string(), ld(p)))
    .collect();
    // Fallback pool for any creature id not mapped above (deeper/added content).
    let monster_pool: Vec<Handle<Image>> = [
        "monsters/goblin.png",
        "monsters/gnoll.png",
        "monsters/kobold.png",
        "monsters/jelly.png",
        "monsters/scorpion.png",
        "monsters/bat.png",
        "monsters/jackal.png",
        "monsters/hydra1.png",
        "monsters/fire_dragon.png",
        "monsters/vampire.png",
    ]
    .into_iter()
    .map(ld)
    .collect();
    // Load a Kenney Nature Kit GLB as a spawnable 3D scene, paired with a baked scale
    // that brings its native size to a sensible world height (computed from each
    // model's bounding box; see assets/ATTRIBUTIONS.md).
    let sc = |p: &str, s: f32| -> (Handle<Scene>, f32) {
        (
            assets.load(GltfAssetLabel::Scene(0).from_asset(format!("models/nature/{p}.glb"))),
            s,
        )
    };
    // Terrain-obstacle kind → real 3D model variants (picked per entity by id hash),
    // so every biome's cover is actual geometry that lights and casts shadow. Water
    // kinds (pond/lava/…) stay flat pools; hard fallbacks use the boulder mesh.
    let prop_scenes: HashMap<String, Vec<(Handle<Scene>, f32)>> = [
        (
            "tree",
            vec![
                sc("tree_default", 3.045),
                sc("tree_oak", 3.751),
                sc("tree_detailed", 3.452),
                sc("tree_fat", 3.651),
                sc("tree_tall", 3.081),
                sc("tree_thin", 3.221),
                sc("tree_pineRoundC", 3.672),
            ],
        ),
        (
            "boulder",
            vec![
                sc("rock_largeA", 7.699),
                sc("rock_largeC", 6.851),
                sc("rock_largeD", 4.575),
                sc("rock_largeE", 8.212),
                sc("rock_largeF", 5.428),
                sc("stone_largeA", 7.699),
            ],
        ),
        (
            "dune",
            vec![
                sc("stone_smallFlatA", 9.239),
                sc("stone_smallFlatB", 9.239),
                sc("rock_largeA", 7.699),
            ],
        ),
        (
            "rock_spire",
            vec![
                sc("rock_tallB", 3.621),
                sc("rock_tallF", 4.532),
                sc("rock_tallH", 4.784),
                sc("rock_tallJ", 5.806),
                sc("stone_tallC", 3.832),
            ],
        ),
        ("cactus", vec![sc("cactus_tall", 3.467), sc("cactus_short", 3.189)]),
        (
            "cliff",
            vec![
                sc("cliff_large_rock", 4.2),
                sc("cliff_cornerLarge_rock", 4.2),
                sc("cliff_top_rock", 3.4),
                sc("cliff_diagonal_rock", 3.4),
                sc("cliff_waterfall_rock", 4.0),
                sc("cliff_rock", 2.6),
                sc("cliff_block_rock", 2.6),
            ],
        ),
        (
            "cinder_rock",
            vec![
                sc("rock_smallA", 5.229),
                sc("rock_smallB", 5.656),
                sc("stone_smallC", 7.337),
            ],
        ),
        (
            "ice_spire",
            vec![
                sc("rock_tallD", 3.885),
                sc("rock_tallH", 4.784),
                sc("rock_tallJ", 5.806),
                sc("stone_tallC", 3.832),
                sc("rock_tallB", 3.621),
            ],
        ),
        (
            "snow_drift",
            vec![
                sc("stone_smallFlatA", 9.239),
                sc("stone_smallFlatB", 9.239),
                sc("rock_smallA", 5.229),
            ],
        ),
        (
            "mire_root",
            vec![
                sc("stump_old", 3.752),
                sc("stump_squareDetailed", 4.5),
                sc("log_stack", 3.175),
                sc("log", 4.041),
            ],
        ),
        (
            "fungal_wall",
            vec![
                sc("mushroom_redGroup", 4.791),
                sc("mushroom_redTall", 5.988),
                sc("mushroom_tanGroup", 4.791),
                sc("plant_bushLarge", 5.351),
            ],
        ),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v))
    .collect();
    // Resource content id → 3D harvest-node model (reagents read as plants/fungi,
    // ores as rocks/stones). Kinds from `meld-world::resources_for_biome`.
    let resource_scenes: HashMap<String, (Handle<Scene>, f32)> = [
        ("bloom_herb", sc("flower_purpleA", 3.299)),
        ("heartoak_bark", sc("log", 4.041)),
        ("sun_salts", sc("stone_smallC", 7.337)),
        ("dune_iron", sc("rock_smallB", 5.656)),
        ("ember_ash", sc("flower_redA", 2.735)),
        ("cinder_ore", sc("rock_smallA", 5.229)),
        ("frost_lichen", sc("plant_bushSmall", 4.824)),
        ("rime_ore", sc("stone_smallC", 7.337)),
        ("bog_myrrh", sc("mushroom_redGroup", 4.791)),
        ("peat_iron", sc("rock_smallB", 5.656)),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v))
    .collect();

    commands.insert_resource(WorldAssets {
        psyker: hd2d::load_character(
            &assets,
            "characters/PSYKER_Male/Psyker",
            "Scary_Walking",
            8,
        ),
        hunter: hd2d::load_character(
            &assets,
            "characters/PSYKER_Male/Squire", // martial art folder (kept as-is)
            "Walking",
            8,
        ),
        // Cylindrical normals so the sun models the flat sprite (HD-2D depth).
        sprite_quad: meshes.add(hd2d::cyl_billboard_mesh(2.2, 2.2, 12, 60.0)),
        // Head→torso crop (top 55%) for stacked back-row busts.
        bust_quad: meshes.add(hd2d::bust_billboard_mesh(2.2, 2.2, 12, 60.0, 0.55)),
        shadow_mesh: meshes.add(Circle::new(0.7)),
        shadow_mat: mats.add(hd2d::contact_shadow_material()),
        monster_sprites,
        monster_pool,
        prop_scenes,
        resource_scenes,
        portal_sprite: ld("fx/portal_arch.png"),
        // A faint emissive ground-ring keeps the portal glowing under the billboard.
        portal_mesh: meshes.add(Torus::new(0.18, 1.15)),
        portal_mat: mats.add(StandardMaterial {
            base_color: Color::srgb(0.1, 0.4, 0.5),
            emissive: LinearRgba::rgb(0.4, 5.0, 6.0),
            ..default()
        }),
        // Target marker: a small gold diamond gem that floats over the picked foe.
        // Lit + emissive so its facets glint and bloom as it slowly spins; drawn
        // double-sided so a facet never disappears.
        target_diamond_mesh: meshes.add(hd2d::diamond_mesh(0.32, 0.5)),
        target_diamond_mat: mats.add(StandardMaterial {
            base_color: Color::srgb(1.0, 0.88, 0.4),
            emissive: LinearRgba::rgb(2.2, 1.6, 0.5),
            perceptual_roughness: 0.35,
            metallic: 0.1,
            cull_mode: None,
            ..default()
        }),
        monster_mesh: meshes.add(Capsule3d::new(0.38, 0.6)),
        rock_mesh: meshes.add(Cuboid::new(1.0, 0.7, 1.0)),
        water_mesh: meshes.add(hd2d::blob_mesh(28)), // organic pool outline, not a circle
        water_mat: mats.add(StandardMaterial {
            base_color: Color::srgb(0.22, 0.45, 0.62),
            base_color_texture: Some(images.add(hd2d::water_ripple_texture(96))),
            emissive: LinearRgba::rgb(0.02, 0.06, 0.1), // faint sky sheen
            perceptual_roughness: 0.12,                 // reflective
            metallic: 0.1,
            alpha_mode: AlphaMode::Blend,
            ..default()
        }),
        ground_tex,
    });

    // Drifting clouds: soft white billboard puffs high overhead, anchored around the
    // camera + drifting on the wind (see `drift_clouds`). Deterministic scatter.
    let puff = meshes.add(Rectangle::new(1.0, 1.0));
    let cloud_tex = images.add(hd2d::cloud_texture(160)); // puffy silhouette, not a disc
    let cloud_mat = mats.add(StandardMaterial {
        base_color: Color::srgba(1.0, 1.0, 1.0, 1.0),
        base_color_texture: Some(cloud_tex.clone()),
        // Mild emissive so the clouds stay bright through the distance fog instead of
        // fading into the sky (they sit near the horizon, where fog is strong).
        emissive: LinearRgba::rgb(0.7, 0.75, 0.82),
        unlit: true,
        alpha_mode: AlphaMode::Blend,
        double_sided: true,
        cull_mode: None,
        ..default()
    });
    // Cloud-shadow material — the same soft disc, dark + transparent, laid flat on
    // the ground and drifting so shadows sweep across as clouds pass overhead.
    let cloud_shadow_mat = mats.add(StandardMaterial {
        base_color: Color::srgba(0.0, 0.0, 0.0, 0.24),
        base_color_texture: Some(cloud_tex),
        unlit: true,
        alpha_mode: AlphaMode::Blend,
        double_sided: true,
        cull_mode: None,
        ..default()
    });
    let flat = Quat::from_rotation_x(-std::f32::consts::FRAC_PI_2);
    let mut s: u64 = 0x9E37_79B9;
    let mut rnd = || {
        s = s.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        ((s >> 33) as f32) / (u32::MAX as f32)
    };
    for _ in 0..22 {
        // Far AHEAD (north, -z) near the horizon, never close/overhead, so they read
        // as clouds in the sky band and never blob over the player. Drift sideways.
        let off = Vec2::new((rnd() - 0.5) * 760.0, -(95.0 + rnd() * 150.0));
        let y = 11.0 + rnd() * 16.0;
        let w = 58.0 + rnd() * 66.0;
        let h = w * (0.28 + rnd() * 0.12);
        commands.spawn((
            Cloud { off, y },
            Mesh3d(puff.clone()),
            MeshMaterial3d(cloud_mat.clone()),
            Transform::from_xyz(off.x, y, off.y).with_scale(Vec3::new(w, h, 1.0)),
            hd2d::Billboard,
        ));
    }
    // Cloud shadows sweeping the ground *around the player* (independent of the
    // horizon clouds), so you see shade pass over you as the wind blows.
    for _ in 0..11 {
        let off = Vec2::new((rnd() - 0.5) * 300.0, (rnd() - 0.5) * 300.0);
        let sz = 34.0 + rnd() * 46.0;
        commands.spawn((
            Cloud { off, y: 0.28 },
            CloudShadow,
            Mesh3d(puff.clone()),
            MeshMaterial3d(cloud_shadow_mat.clone()),
            Transform::from_translation(Vec3::new(off.x, 0.28, off.y))
                .with_rotation(flat)
                .with_scale(Vec3::new(sz, sz * 0.72, 1.0)),
        ));
    }
    commands.insert_resource(SkyMats { cloud: cloud_mat });

    // Distant cliff/mountain backdrop: a sparse ring of BIG rock models far out on the
    // horizon, anchored around the camera (see `anchor_backdrop`) so the diorama always
    // has depth behind the play area. Sparse + far, so it reads as a scattered skyline
    // rather than a wall, and the distance fog softens it into the sky.
    let backdrop: Vec<Handle<Scene>> = ["cliff_large_rock", "rock_largeA", "cliff_cornerLarge_rock"]
        .into_iter()
        .map(|p| assets.load(GltfAssetLabel::Scene(0).from_asset(format!("models/nature/{p}.glb"))))
        .collect();
    for i in 0..14 {
        let ang = i as f32 / 14.0 * std::f32::consts::TAU + (rnd() - 0.5) * 0.35;
        let rad = 165.0 + rnd() * 55.0;
        let off = Vec2::new(ang.cos() * rad, ang.sin() * rad);
        let size = 10.0 + rnd() * 10.0;
        commands.spawn((
            Backdrop { off },
            SceneRoot(backdrop[i % backdrop.len()].clone()),
            Transform::from_translation(Vec3::new(off.x, -0.5, off.y))
                .with_scale(Vec3::splat(size))
                .with_rotation(Quat::from_rotation_y(rnd() * std::f32::consts::TAU)),
        ));
    }

    // Stars — tiny emissive points on a camera-anchored dome, shown only at night.
    let star_mesh = meshes.add(Sphere::new(0.12));
    let star_mat = mats.add(StandardMaterial {
        base_color: Color::WHITE,
        emissive: LinearRgba::rgb(6.0, 6.0, 7.0),
        unlit: true,
        ..default()
    });
    for _ in 0..200 {
        // Far + low so they sit in the thin sky band near the horizon (the only sky
        // a low-pitch diorama camera actually shows).
        let ang = rnd() * std::f32::consts::TAU;
        let r = 200.0 + rnd() * 260.0;
        let off = Vec3::new(ang.cos() * r, 10.0 + rnd() * 55.0, ang.sin() * r);
        commands.spawn((
            Star { off },
            Mesh3d(star_mesh.clone()),
            MeshMaterial3d(star_mat.clone()),
            Transform::from_translation(off).with_scale(Vec3::splat(0.6 + rnd() * 1.4)),
            Visibility::Hidden,
        ));
    }

    // The rain cloud: a single dark, low storm cloud that drifts over the play area
    // and CARRIES the rain — rain falls only in the disk beneath it (see `drive_rain`),
    // not as a screen-wide slab. Darker than the fair-weather clouds so it reads as a
    // storm cloud; shown only while it rains.
    let rain_cloud_mat = mats.add(StandardMaterial {
        base_color: Color::srgb(0.34, 0.36, 0.40),
        emissive: LinearRgba::rgb(0.02, 0.02, 0.03),
        unlit: false,
        perceptual_roughness: 1.0,
        alpha_mode: AlphaMode::Blend,
        ..default()
    });
    commands.spawn((
        RainCloud { off: Vec2::new(-22.0, -6.0) },
        Mesh3d(puff.clone()),
        MeshMaterial3d(rain_cloud_mat),
        Transform::from_xyz(0.0, RAIN_CLOUD_Y, 0.0).with_scale(Vec3::new(78.0, 34.0, 1.0)),
        hd2d::Billboard,
        Visibility::Hidden,
    ));

    // Rain — thin streaks confined to a DISK under the rain cloud (radius
    // `RAIN_RADIUS`), so the shower tracks the cloud rather than filling the screen.
    // `off.xz` is the drop's position within that disk; `off.y` is its fall height.
    let drop_mesh = meshes.add(Cuboid::new(0.035, 1.3, 0.035));
    let drop_mat = mats.add(StandardMaterial {
        base_color: Color::srgba(0.78, 0.85, 0.97, 0.6),
        emissive: LinearRgba::rgb(0.32, 0.38, 0.48),
        unlit: true,
        alpha_mode: AlphaMode::Blend,
        ..default()
    });
    for _ in 0..900 {
        // Uniform over the disk: sqrt(u) keeps it from clustering at the centre.
        let ang = rnd() * std::f32::consts::TAU;
        let r = rnd().sqrt() * RAIN_RADIUS;
        let off = Vec3::new(ang.cos() * r, rnd() * RAIN_FALL_TOP, ang.sin() * r);
        commands.spawn((
            RainDrop { off },
            Mesh3d(drop_mesh.clone()),
            MeshMaterial3d(drop_mat.clone()),
            Transform::from_translation(off),
            Visibility::Hidden,
        ));
    }

    // ── Cosmetic ground detail (client-only) ────────────────────────────────
    // Small Kenney nature props (flowers/bushes/mushrooms/pebbles) scattered to
    // give the ground life the tiled texture can't. Server-authoritative props
    // are untouched — this is pure decoration, spawned as a fixed pool of entities
    // that `tile_ground_detail` recycles onto a player-anchored grid, so coverage
    // is endless with a bounded entity count. Position + type + visibility all
    // derive from the world cell, so a spot always looks the same (no popping).
    let detail_scenes: Vec<(Handle<Scene>, f32)> = [
        ("flower_purpleA", 2.6),
        ("flower_redA", 2.6),
        ("plant_bushSmall", 2.2),
        ("plant_bushLarge", 1.8),
        ("mushroom_redGroup", 2.4),
        ("mushroom_tanGroup", 2.4),
        ("rock_smallA", 2.0),
        ("rock_smallB", 2.0),
        ("stone_smallFlatA", 2.2),
        ("stone_smallC", 1.8),
    ]
    .into_iter()
    .map(|(p, sc)| {
        (
            assets.load(GltfAssetLabel::Scene(0).from_asset(format!("models/nature/{p}.glb"))),
            sc,
        )
    })
    .collect();
    let placeholder = detail_scenes[0].0.clone();
    commands.insert_resource(DetailKit { scenes: detail_scenes });
    for gz in -DETAIL_K..=DETAIL_K {
        for gx in -DETAIL_K..=DETAIL_K {
            commands.spawn((
                GroundDetail { slot: IVec2::new(gx, gz), last: IVec2::splat(i32::MIN) },
                SceneRoot(placeholder.clone()),
                Transform::default(),
                Visibility::Hidden,
            ));
        }
    }

    // ── Atmosphere motes (client-only) ──────────────────────────────────────
    // Drifting dust/pollen: soft billboarded discs anchored around the camera so
    // the near air always reads as alive. `drift_motes` bobs them; `billboard`
    // faces them at the camera.
    let mote_tex = images.add(hd2d::soft_disc_texture(64));
    let mote_mesh = meshes.add(Rectangle::new(0.16, 0.16));
    let mote_mat = mats.add(StandardMaterial {
        base_color: Color::srgba(1.0, 0.95, 0.8, 0.5),
        base_color_texture: Some(mote_tex),
        emissive: LinearRgba::rgb(0.7, 0.62, 0.35),
        unlit: true,
        alpha_mode: AlphaMode::Blend,
        double_sided: true,
        cull_mode: None,
        ..default()
    });
    for _ in 0..90 {
        let off = Vec2::new((rnd() - 0.5) * 60.0, (rnd() - 0.5) * 44.0);
        commands.spawn((
            Mote {
                off,
                base_y: 0.6 + rnd() * 4.2,
                phase: rnd() * std::f32::consts::TAU,
                amp: 0.3 + rnd() * 0.9,
                speed: 0.2 + rnd() * 0.5,
            },
            Mesh3d(mote_mesh.clone()),
            MeshMaterial3d(mote_mat.clone()),
            Transform::from_scale(Vec3::splat(0.5 + rnd() * 1.2)),
            hd2d::Billboard,
        ));
    }

    // ── Falling ash (Ashfall biome only) ────────────────────────────────────
    // A column of drifting grey ash flecks anchored around the camera, hidden
    // everywhere but the Ashfall band, where `drive_ashfall` fades them in as they
    // sift down (with the reddened haze + charred ground) so the biome reads as a
    // volcanic wasteland, not "the forest again".
    let ash_tex = images.add(hd2d::soft_disc_texture(64));
    let ash_mesh = meshes.add(Rectangle::new(0.2, 0.2));
    let ash_mat = mats.add(StandardMaterial {
        base_color: Color::srgba(0.82, 0.78, 0.74, 0.9), // pale drifting ash
        base_color_texture: Some(ash_tex),
        emissive: LinearRgba::rgb(0.35, 0.30, 0.27),
        unlit: true,
        alpha_mode: AlphaMode::Blend,
        double_sided: true,
        cull_mode: None,
        ..default()
    });
    for _ in 0..ASH_COUNT {
        let off = Vec3::new((rnd() - 0.5) * 64.0, rnd() * ASH_FALL_TOP, (rnd() - 0.5) * 48.0);
        commands.spawn((
            AshFleck { off, sway: rnd() * std::f32::consts::TAU, fall: 3.2 + rnd() * 3.6 },
            Mesh3d(ash_mesh.clone()),
            MeshMaterial3d(ash_mat.clone()),
            Transform::from_translation(off).with_scale(Vec3::splat(0.6 + rnd() * 1.1)),
            hd2d::Billboard,
            Visibility::Hidden,
        ));
    }

    // ── Giant volcanoes (Ashfall biome only) ────────────────────────────────
    // A ring of huge dark cones with glowing lava craters, looming on the horizon
    // around the player through the ash haze — "giant volcanoes everywhere". Like
    // the backdrop mountains, they anchor to the player (a distant skyline, not
    // approachable geometry); `drive_ashfall` places + fades them by intensity.
    let cone_mesh = meshes.add(Cone { radius: 26.0, height: 46.0 });
    let crater_mesh = meshes.add(Cone { radius: 9.0, height: 8.0 });
    let cone_mat = mats.add(StandardMaterial {
        base_color: Color::srgb(0.16, 0.09, 0.09), // charred basalt
        perceptual_roughness: 1.0,
        ..default()
    });
    let crater_mat = mats.add(StandardMaterial {
        base_color: Color::srgb(0.9, 0.3, 0.12),
        emissive: LinearRgba::rgb(3.2, 0.9, 0.2), // molten glow
        unlit: true,
        ..default()
    });
    for i in 0..VOLCANO_COUNT {
        let angle = (i as f32 / VOLCANO_COUNT as f32) * std::f32::consts::TAU;
        let dist = 130.0 + rnd() * 70.0;
        commands
            .spawn((
                VolcanoProp { angle, dist },
                Mesh3d(cone_mesh.clone()),
                MeshMaterial3d(cone_mat.clone()),
                Transform::from_scale(Vec3::new(1.0, 0.7 + rnd() * 0.7, 1.0)),
                Visibility::Hidden,
            ))
            .with_children(|p| {
                // The lava crater sits at the cone's apex (Cone is centred on origin,
                // apex at +height/2).
                p.spawn((
                    Mesh3d(crater_mesh.clone()),
                    MeshMaterial3d(crater_mat.clone()),
                    Transform::from_xyz(0.0, 23.0, 0.0),
                ));
            });
    }
}

/// How many giant volcanoes ring the Ashfall horizon.
pub(crate) const VOLCANO_COUNT: usize = 7;

/// A distant volcano on the Ashfall skyline: `angle` around the player and `dist`
/// out. Anchored to the player (a skyline, not walkable), shown only in Ashfall.
#[derive(Component)]
pub(crate) struct VolcanoProp {
    angle: f32,
    dist: f32,
}

/// Number of ash flecks in the recycled Ashfall pool, and the top of their fall
/// column (they wrap to the top when they sift below the ground).
pub(crate) const ASH_COUNT: usize = 150;
pub(crate) const ASH_FALL_TOP: f32 = 26.0;

/// A single drifting ash fleck: `off` is its position relative to the camera focus
/// (so the ash travels with the player), `sway` a per-fleck phase for the lateral
/// drift, `fall` its descent speed.
#[derive(Component)]
pub(crate) struct AshFleck {
    off: Vec3,
    sway: f32,
    fall: f32,
}

/// How strongly the Ashfall biome atmosphere is showing (0 = off, 1 = full): a
/// smoothed factor `drive_ashfall` ramps up while the player is in the Ashfall band
/// and `apply_sky` reads to redden the haze + dim the light (reduced visibility).
#[derive(Resource, Default)]
pub(crate) struct Ashfall {
    intensity: f32,
}

/// Grid cell size (world units) for the cosmetic ground-detail field.
pub(crate) const DETAIL_CELL: f32 = 4.0;
/// Half-extent of the detail grid around the focus point: `(2K+1)²` entities.
pub(crate) const DETAIL_K: i32 = 8;

/// The ground point the camera is aimed at (where its view ray meets y=0) — the
/// centre of the visible play area. Both the detail grid and the motes anchor here
/// rather than to the camera itself, which sits well behind/above the play area.
pub(crate) fn ground_focus(cam: &Transform) -> Vec3 {
    let fwd = Vec3::from(cam.forward());
    if fwd.y.abs() > 1e-3 {
        let t = (-cam.translation.y / fwd.y).max(0.0);
        cam.translation + fwd * t
    } else {
        cam.translation
    }
}

/// Loaded small nature props for the cosmetic ground-detail field: `(scene, base_scale)`.
#[derive(Resource)]
pub(crate) struct DetailKit {
    scenes: Vec<(Handle<Scene>, f32)>,
}

/// One recyclable cosmetic ground-detail prop. `slot` is its fixed offset (in cells)
/// from the player's current cell; `last` is the world cell it currently shows, so a
/// prop only re-derives (and swaps scene) when it actually moves to a new cell.
#[derive(Component)]
pub(crate) struct GroundDetail {
    slot: IVec2,
    last: IVec2,
}

/// Drifting atmosphere mote: `off` is its xz offset from the camera (anchored so it
/// travels with the player), plus a bob phase/amplitude/speed.
#[derive(Component)]
pub(crate) struct Mote {
    off: Vec2,
    base_y: f32,
    phase: f32,
    amp: f32,
    speed: f32,
}

/// Deterministic hash of a world cell → 64 bits of stable per-cell randomness.
pub(crate) fn detail_hash(c: IVec2) -> u64 {
    let mut x = (c.x as u64)
        .wrapping_mul(0x9E37_79B9_7F4A_7C15)
        ^ (c.y as u64).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x ^= x >> 29;
    x = x.wrapping_mul(0x94D0_49BB_1331_11EB);
    x ^= x >> 32;
    x
}

/// Height of the drifting rain cloud, and the footprint the rain falls within.
pub(crate) const RAIN_CLOUD_Y: f32 = 32.0;
pub(crate) const RAIN_RADIUS: f32 = 18.0;
pub(crate) const RAIN_FALL_TOP: f32 = 30.0;

/// Marks a cloud's ground shadow (flat, dark) vs a sky cloud puff — both drift via
/// [`drift_clouds`], but shadows stay flat on the ground (no billboarding).
#[derive(Component)]
pub(crate) struct CloudShadow;

/// A far-off cliff/mountain on the horizon, anchored around the camera (like the
/// clouds) so the diorama always has depth behind it. `off` is its xz offset from the
/// camera; see [`anchor_backdrop`]. Fogged into the sky at that distance.
#[derive(Component)]
pub(crate) struct Backdrop {
    off: Vec2,
}

/// Wind sway for foliage: the prop leans back and forth around its base (which sits
/// on the ground) so the top travels most — reading as leaves moving in the wind.
/// `base_yaw` preserves the spawn-time variety rotation the sway composes onto; the
/// sway strengthens in rain (see [`animate_sway`]). Applied to trees/mushrooms/cacti.
#[derive(Component)]
pub(crate) struct Sway {
    pub(crate) base_yaw: f32,
    pub(crate) phase: f32,
    pub(crate) amp: f32,
    pub(crate) speed: f32,
}

/// Per-obstacle-kind wind-sway amplitude (radians of lean); `None` = rigid (rock/etc).
pub(crate) fn sway_amp(kind: &str) -> Option<f32> {
    match kind {
        "tree" => Some(0.05),
        "fungal_wall" => Some(0.045),
        "cactus" => Some(0.02),
        _ => None,
    }
}

/// Lean every [`Sway`] prop on the wind — top-heavy (pivots at the grounded base),
/// phase-offset per prop, and gustier while it's raining. Overworld only.
pub(crate) fn animate_sway(time: Res<Time>, sky: Option<Res<Sky>>, mut q: Query<(&Sway, &mut Transform)>) {
    let t = time.elapsed_secs();
    let gust = 1.0 + sky.map(|s| s.weather).unwrap_or(0.0) * 1.6;
    for (s, mut tf) in &mut q {
        let a = (t * s.speed + s.phase).sin() * s.amp * gust;
        tf.rotation = Quat::from_rotation_y(s.base_yaw)
            * Quat::from_rotation_z(a)
            * Quat::from_rotation_x(a * 0.35);
    }
}

/// Keep the [`Backdrop`] cliffs parked around the camera (they never get closer, like
/// a parallax skyline) so the horizon always frames the scene with depth.
pub(crate) fn anchor_backdrop(
    cam_q: Query<&Transform, With<Camera3d>>,
    mut q: Query<(&Backdrop, &mut Transform), Without<Camera3d>>,
) {
    let Ok(cam) = cam_q.single() else { return };
    for (b, mut tf) in &mut q {
        tf.translation.x = cam.translation.x + b.off.x;
        tf.translation.z = cam.translation.z + b.off.y;
    }
}

/// A drifting sky cloud: `off` is its position **relative to the camera** on the xz
/// plane (so clouds stay overhead as you travel), `y` its altitude.
#[derive(Component)]
pub(crate) struct Cloud {
    off: Vec2,
    y: f32,
}

/// Wind speed (world units/sec) the clouds drift east.
pub(crate) const CLOUD_WIND: f32 = 2.5;

/// Drift the clouds on the wind and keep them anchored around the camera (wrapping
/// so the sky never empties as the player travels).
pub(crate) fn drift_clouds(
    time: Res<Time>,
    cam_q: Query<&Transform, With<Camera3d>>,
    mut q: Query<(&mut Cloud, &mut Transform), Without<Camera3d>>,
) {
    let cam = cam_q.single().map(|t| t.translation).unwrap_or(Vec3::ZERO);
    const R: f32 = 420.0;
    for (mut c, mut tf) in &mut q {
        c.off.x += CLOUD_WIND * time.delta_secs();
        if c.off.x > R {
            c.off.x -= 2.0 * R;
        }
        tf.translation.x = cam.x + c.off.x;
        tf.translation.z = cam.z + c.off.y;
        tf.translation.y = c.y;
    }
}

/// Recycle the cosmetic ground-detail pool onto a grid centred on the player. Each
/// prop maps its fixed `slot` to the world cell `player_cell + slot`; position, type,
/// scale, yaw and visibility all derive deterministically from that cell, so a given
/// spot always looks identical and props never appear to slide or flicker — they only
/// re-derive (at the grid's edge, off-screen) as new cells scroll in.
#[allow(clippy::type_complexity)]
pub(crate) fn tile_ground_detail(
    cam_q: Query<&Transform, With<Camera3d>>,
    kit: Option<Res<DetailKit>>,
    mut q: Query<
        (&mut GroundDetail, &mut Transform, &mut Visibility, &mut SceneRoot),
        Without<Camera3d>,
    >,
) {
    let (Ok(cam), Some(kit)) = (cam_q.single(), kit) else { return };
    let focus = ground_focus(cam);
    let cc = IVec2::new(
        (focus.x / DETAIL_CELL).floor() as i32,
        (focus.z / DETAIL_CELL).floor() as i32,
    );
    for (mut d, mut tf, mut vis, mut root) in &mut q {
        let cell = cc + d.slot;
        if cell == d.last {
            continue; // still the same world cell — nothing to re-derive
        }
        d.last = cell;
        let h = detail_hash(cell);
        // Density gate: only ~45% of cells carry detail, so it scatters instead of
        // reading as a rigid grid.
        if (h & 0xff) as f32 / 255.0 > 0.45 {
            *vis = Visibility::Hidden;
            continue;
        }
        let (scene, base) = &kit.scenes[((h >> 8) as usize) % kit.scenes.len()];
        root.0 = scene.clone();
        let jx = ((h >> 16) & 0xffff) as f32 / 65535.0;
        let jz = ((h >> 32) & 0xffff) as f32 / 65535.0;
        let yaw = ((h >> 24) & 0xff) as f32 / 255.0 * std::f32::consts::TAU;
        let sc = base * (0.7 + ((h >> 48) & 0xff) as f32 / 255.0 * 0.7);
        tf.translation = Vec3::new(
            (cell.x as f32 + jx) * DETAIL_CELL,
            0.0,
            (cell.y as f32 + jz) * DETAIL_CELL,
        );
        tf.rotation = Quat::from_rotation_y(yaw);
        tf.scale = Vec3::splat(sc);
        *vis = Visibility::Inherited;
    }
}

/// Keep the base ground plane centred under the player so the endless radial world
/// (WG-4 streaming) always has ground underfoot — the plane is huge but still finite,
/// and difficulty is unbounded, so far-out dives would otherwise walk off its edge.
/// Safe because `ground_biome.wgsl` keys BOTH its texture UV and its biome/distance
/// off `world_position.xz`, so sliding the mesh never swims the texture or the biome.
pub(crate) fn follow_world_ground(
    cam_q: Query<&Transform, With<Camera3d>>,
    mut ground_q: Query<&mut Transform, (With<WorldGround>, Without<Camera3d>)>,
) {
    let Ok(cam) = cam_q.single() else { return };
    let focus = ground_focus(cam);
    for mut tf in &mut ground_q {
        tf.translation.x = focus.x;
        tf.translation.z = focus.z;
    }
}

/// Capitalize the first letter for display ("ashfall" → "Ashfall").
pub(crate) fn title_case(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

/// Biome theme name → ground-texture / ring index (matches `BIOMES` order in
/// meld-world and the texture bindings in `ground_biome.wgsl`).
pub(crate) fn biome_ring_index(name: &str) -> usize {
    match name {
        "desert" => 1,
        "ashfall" => 2,
        "tundra" => 3,
        "mire" => 4,
        _ => 0, // forest / unknown
    }
}

/// Rebuild the ground shader's radial biome LUT from the streamed sections, so the
/// floor is coloured by each section's ACTUAL biome (its concentric radius ring)
/// instead of fixed distance bands — the fix for the ground/creature biome mismatch.
/// A window of `MAX_BIOME_RINGS` rings centred on the player covers the visible fan;
/// deeper dives clamp the far/near ends (out in the haze / behind you anyway).
pub(crate) fn update_ground_biome_rings(
    terrain: Res<Terrain>,
    world: Res<Overworld>,
    session: Res<Session>,
    ground_q: Query<&MeshMaterial3d<GroundMat>, With<WorldGround>>,
    mut mats: ResMut<Assets<GroundMat>>,
) {
    let Ok(handle) = ground_q.single() else { return };
    let Some(mat) = mats.get_mut(&handle.0) else { return };
    // (outer_radius, biome_index) per section, sorted by radius (= corridor end_x).
    let mut rings: Vec<(f32, f32)> = terrain
        .sections
        .values()
        .map(|s| (s.end_x as f32, biome_ring_index(&s.biome) as f32))
        .collect();
    rings.sort_by(|a, b| a.0.total_cmp(&b.0));

    // Window the rings around the player's radius when there are more than fit.
    let pr = world
        .entities
        .get(&session.player_id)
        .map(|e| (e.x.hypot(e.y)) as f32)
        .unwrap_or(0.0);
    let start = if rings.len() <= MAX_BIOME_RINGS {
        0
    } else {
        let center = rings.iter().position(|(r, _)| *r >= pr).unwrap_or(rings.len() - 1);
        center
            .saturating_sub(MAX_BIOME_RINGS / 2)
            .min(rings.len() - MAX_BIOME_RINGS)
    };
    let window = &rings[start..(start + MAX_BIOME_RINGS).min(rings.len())];

    let p = &mut mat.extension.params;
    p.count = window.len() as u32;
    for (i, (radius, biome)) in window.iter().enumerate() {
        p.rings[i] = Vec4::new(*radius, *biome, 0.0, 0.0);
    }
}

/// Bob the atmosphere motes and keep them anchored around the camera (like the
/// clouds) so the near air is always alive as the player travels.
pub(crate) fn drift_motes(
    time: Res<Time>,
    cam_q: Query<&Transform, With<Camera3d>>,
    mut q: Query<(&Mote, &mut Transform), Without<Camera3d>>,
) {
    let focus = cam_q.single().map(ground_focus).unwrap_or(Vec3::ZERO);
    let t = time.elapsed_secs();
    for (m, mut tf) in &mut q {
        tf.translation.x = focus.x + m.off.x + (t * m.speed + m.phase).sin() * m.amp;
        tf.translation.z = focus.z + m.off.y + (t * m.speed * 0.7 + m.phase).cos() * m.amp;
        tf.translation.y = m.base_y + (t * 0.6 + m.phase).sin() * m.amp * 0.5;
    }
}

/// Ramp the Ashfall atmosphere up while the local player is in the Ashfall band and
/// down everywhere else (and outside the overworld), then sift the ash flecks down
/// around the camera. The reddened haze + dimmed light is layered in by `apply_sky`
/// from the shared [`Ashfall`] intensity this writes.
#[allow(clippy::type_complexity, clippy::too_many_arguments)]
pub(crate) fn drive_ashfall(
    time: Res<Time>,
    state: Res<State<Screen>>,
    world: Res<Overworld>,
    session: Res<Session>,
    terrain: Res<Terrain>,
    mut ash: ResMut<Ashfall>,
    cam_q: Query<&Transform, With<Camera3d>>,
    mut flecks: Query<(&mut AshFleck, &mut Transform, &mut Visibility), (Without<Camera3d>, Without<VolcanoProp>)>,
    mut volcanoes: Query<(&VolcanoProp, &mut Transform, &mut Visibility), (Without<Camera3d>, Without<AshFleck>)>,
) {
    // The Ashfall atmosphere follows the ACTUAL section the player is standing in
    // (its radius ring), so it fires exactly where the charred ground + ashfall
    // creatures are — not on a fixed distance band.
    let in_ashfall = *state.get() == Screen::Overworld
        && world.entities.get(&session.player_id).is_some_and(|e| {
            let r = (e.x * e.x + e.y * e.y).sqrt() as f64;
            terrain
                .sections
                .values()
                .find(|s| r >= s.start_x && r < s.end_x)
                .map(|s| s.biome == "ashfall")
                .unwrap_or(false)
        });
    let target = if in_ashfall { 1.0 } else { 0.0 };
    let dt = time.delta_secs();
    ash.intensity += (target - ash.intensity).clamp(-dt * 0.8, dt * 0.8);

    let focus = cam_q.single().map(ground_focus).unwrap_or(Vec3::ZERO);
    let t = time.elapsed_secs();
    let show = ash.intensity > 0.02;
    for (mut f, mut tf, mut v) in &mut flecks {
        *v = if show { Visibility::Inherited } else { Visibility::Hidden };
        if !show {
            continue;
        }
        f.off.y -= f.fall * dt;
        if f.off.y < 0.0 {
            f.off.y += ASH_FALL_TOP; // wrap to the top of the column
        }
        // Sift down with a gentle lateral sway, anchored around the play area.
        let sway = (t * 0.6 + f.sway).sin() * 1.3;
        tf.translation = Vec3::new(focus.x + f.off.x + sway, f.off.y, focus.z + f.off.z);
    }
    // Loom the volcanoes on the horizon ring around the player (a skyline that
    // travels with you), shown once the haze is well established.
    let volcano_show = ash.intensity > 0.3;
    for (vp, mut tf, mut v) in &mut volcanoes {
        *v = if volcano_show { Visibility::Inherited } else { Visibility::Hidden };
        if volcano_show {
            tf.translation.x = focus.x + vp.angle.cos() * vp.dist;
            tf.translation.z = focus.z + vp.angle.sin() * vp.dist;
            // Base sits on the ground: Cone is centred on origin, so lift by half its
            // (scaled) height — the y-scale is baked at spawn, read it back.
            tf.translation.y = 46.0 * tf.scale.y * 0.5 - 2.0;
        }
    }
}

// ============================ time of day + weather ========================

/// Seconds for one full day → night → day cycle.
pub(crate) const DAY_LEN: f32 = 210.0;

/// Time of day (`t`: 0 = midnight, 0.5 = noon) + weather (`0` clear .. `1` rain),
/// which together drive the sun, ambient, sky/fog colour, stars, and rain.
#[derive(Resource)]
pub(crate) struct Sky {
    pub(crate) t: f32,
    pub(crate) weather: f32,
    pub(crate) weather_target: f32,
    pub(crate) weather_timer: f32,
    /// Daylight factor (0 = night, 1 = day), recomputed each frame by [`apply_sky`]
    /// so other systems (e.g. the Hunter avatar lamp) can read the darkness without
    /// duplicating the sun-angle math.
    pub(crate) day: f32,
}
impl Default for Sky {
    fn default() -> Self {
        Sky { t: 0.36, weather: 0.0, weather_target: 0.0, weather_timer: 55.0, day: 1.0 }
    }
}

/// Material handles `apply_sky` modulates over the day (cloud glow).
#[derive(Resource, Default)]
pub(crate) struct SkyMats {
    cloud: Handle<StandardMaterial>,
}

/// A background star, camera-anchored (`off`) and shown only at night.
#[derive(Component)]
pub(crate) struct Star {
    off: Vec3,
}

/// A rain streak. `off.xz` is its position within the rain cloud's footprint disk;
/// `off.y` is its fall height. Positioned under the drifting rain cloud (`drive_rain`).
#[derive(Component)]
pub(crate) struct RainDrop {
    off: Vec3,
}

/// The single storm cloud that carries the rain. `off` is its xz offset from the
/// camera; it drifts on the wind and the rain falls in the disk beneath it.
#[derive(Component)]
pub(crate) struct RainCloud {
    off: Vec2,
}

/// Lerp two colours in sRGB space.
pub(crate) fn mix_col(a: Color, b: Color, t: f32) -> Color {
    let (a, b) = (Srgba::from(a), Srgba::from(b));
    Srgba::new(
        a.red + (b.red - a.red) * t,
        a.green + (b.green - a.green) * t,
        a.blue + (b.blue - a.blue) * t,
        1.0,
    )
    .into()
}

/// Advance the day clock and roll the weather (longer clear spells than rain).
pub(crate) fn advance_sky(time: Res<Time>, mut sky: ResMut<Sky>) {
    let dt = time.delta_secs();
    sky.t = (sky.t + dt / DAY_LEN).fract();
    sky.weather_timer -= dt;
    if sky.weather_timer <= 0.0 {
        sky.weather_target = if sky.weather_target > 0.5 { 0.0 } else { 1.0 };
        sky.weather_timer = if sky.weather_target > 0.5 { 32.0 } else { 75.0 };
    }
    let rate = 0.2 * dt;
    sky.weather += (sky.weather_target - sky.weather).clamp(-rate, rate);
}

/// Drive the sun (angle/colour/brightness), ambient, sky + fog colour, star
/// visibility, and cloud glow from the time of day + weather. Owns the sun light
/// (so `hd2d_follow`/`battle_camera` no longer touch it).
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
pub(crate) fn apply_sky(
    mut sky: ResMut<Sky>,
    skymats: Option<Res<SkyMats>>,
    ashfall: Res<Ashfall>,
    mut clear: ResMut<ClearColor>,
    mut ambient: ResMut<AmbientLight>,
    mut mats: ResMut<Assets<StandardMaterial>>,
    mut sun_q: Query<(&mut Transform, &mut DirectionalLight)>,
    mut fog_q: Query<&mut bevy::pbr::DistanceFog, With<Camera3d>>,
    mut stars: Query<&mut Visibility, With<Star>>,
) {
    use std::f32::consts::TAU;
    let sun_h = ((sky.t - 0.25) * TAU).sin(); // +1 at noon, -1 at midnight
    // Slower transition = a longer golden hour at dawn/dusk.
    let day = ((sun_h + 0.14) / 0.36).clamp(0.0, 1.0); // 0 night → 1 day
    let dusk = ((0.30 - sun_h.abs()).max(0.0) / 0.30).powf(1.2); // horizon glow
    let rain = sky.weather;
    // Publish the daylight factor so other systems (Hunter lamp) can read darkness.
    sky.day = day;

    let night_sky = Color::srgb(0.03, 0.05, 0.10);
    let day_sky = Color::srgb(0.50, 0.72, 0.93);
    let dusk_sky = Color::srgb(0.66, 0.42, 0.30);
    let rain_sky = Color::srgb(0.36, 0.40, 0.44);
    let mut sky_col = mix_col(night_sky, day_sky, day);
    sky_col = mix_col(sky_col, dusk_sky, dusk * 0.6);
    sky_col = mix_col(sky_col, rain_sky, rain * 0.7 * (0.35 + day * 0.65));
    // Ashfall haze: a thick, sooty red-grey smoke drops visibility and casts the
    // whole scene volcanic. Layered on top of the day/weather sky by intensity.
    let ash = ashfall.intensity.clamp(0.0, 1.0);
    let ash_smoke = Color::srgb(0.30, 0.16, 0.13);
    if ash > 0.0 {
        clear.0 = mix_col(sky_col, ash_smoke, ash * 0.8);
    } else {
        clear.0 = sky_col;
    }
    if let Ok(mut fog) = fog_q.single_mut() {
        let base_fog = mix_col(sky_col, Color::WHITE, 0.04);
        // Pull the fog heavily toward the smoke so distance dissolves into red haze —
        // reduced visibility without fighting the camera's per-frame falloff sync.
        fog.color = mix_col(base_fog, ash_smoke, ash * 0.85);
    }

    if let Ok((mut t, mut light)) = sun_q.single_mut() {
        // Keep a shallow angle even at night so the "moon" casts soft directional light.
        let pitch = (sun_h.abs() * 66.0).max(12.0);
        let yaw = 40.0 + (sky.t - 0.5) * 55.0; // arc east → west across the day
        *t = Transform::from_rotation(Quat::from_euler(
            EulerRot::YXZ,
            yaw.to_radians(),
            -pitch.to_radians(),
            0.0,
        ));
        let noon = Color::srgb(1.0, 0.97, 0.9);
        let warm = Color::srgb(1.0, 0.6, 0.38);
        let moon = Color::srgb(0.55, 0.65, 0.95);
        light.color = mix_col(moon, mix_col(warm, noon, day), day);
        // Full sun by day; a dim cool moon fill at night.
        light.illuminance = (day * 9200.0 + (1.0 - day) * 550.0) * (1.0 - rain * 0.55);
    }

    // Moonlit-blue at night (not black), warm-white by day.
    ambient.color = mix_col(Color::srgb(0.34, 0.42, 0.68), Color::srgb(0.6, 0.7, 0.85), day);
    ambient.brightness = (95.0 + day * 165.0) * (1.0 - rain * 0.35);
    // Ashfall dims + warms the ambient — an oppressive, smoke-choked half-light.
    if ash > 0.0 {
        ambient.color = mix_col(ambient.color, Color::srgb(0.9, 0.45, 0.32), ash * 0.6);
        ambient.brightness *= 1.0 - ash * 0.4;
    }

    let star_vis = if day < 0.22 && rain < 0.45 {
        Visibility::Inherited
    } else {
        Visibility::Hidden
    };
    for mut v in &mut stars {
        *v = star_vis;
    }

    if let Some(sm) = skymats {
        if let Some(m) = mats.get_mut(&sm.cloud) {
            let g = (0.14 + day * 0.86) * (1.0 - rain * 0.25);
            m.emissive = LinearRgba::rgb(0.72 * g, 0.75 * g, 0.82 * g);
            m.base_color = Color::srgba(1.0, 1.0, 1.0, (0.72 + day * 0.28) * (1.0 - rain * 0.2));
        }
    }
}

/// Keep the stars anchored around the camera (they'd otherwise be left behind).
pub(crate) fn anchor_sky_fx(
    cam_q: Query<&Transform, With<Camera3d>>,
    mut stars: Query<(&Star, &mut Transform), (Without<Camera3d>, Without<RainDrop>)>,
) {
    let cam = cam_q.single().map(|t| t.translation).unwrap_or(Vec3::ZERO);
    for (s, mut t) in &mut stars {
        t.translation = cam + s.off;
    }
}

/// Drift the rain cloud over the play area and rain ONLY in the disk beneath it, so
/// the shower reads as "that cloud is raining" rather than a screen-wide slab. The
/// cloud + drops are shown only while it's raining.
pub(crate) fn drive_rain(
    cam_q: Query<&Transform, With<Camera3d>>,
    time: Res<Time>,
    sky: Res<Sky>,
    mut cloud_q: Query<
        (&mut RainCloud, &mut Transform, &mut Visibility),
        (Without<Camera3d>, Without<RainDrop>),
    >,
    mut rain_q: Query<
        (&mut RainDrop, &mut Transform, &mut Visibility),
        (Without<Camera3d>, Without<RainCloud>),
    >,
) {
    let cam = cam_q.single().map(|t| t.translation).unwrap_or(Vec3::ZERO);
    let raining = sky.weather > 0.05;
    let vis = if raining { Visibility::Inherited } else { Visibility::Hidden };
    let dt = time.delta_secs();
    // Drift the rain cloud on the wind, wrapping in a tight band so it keeps passing
    // over the play area. Capture its ground position for the drops below.
    let mut ground = Vec2::new(cam.x, cam.z);
    for (mut rc, mut t, mut v) in &mut cloud_q {
        rc.off.x += CLOUD_WIND * dt;
        // Keep the cloud in a tight band over the play area so its shower passes over
        // the player as it drifts (rather than wandering off to the horizon).
        const BAND: f32 = 30.0;
        if rc.off.x > BAND {
            rc.off.x -= 2.0 * BAND;
        }
        t.translation = Vec3::new(cam.x + rc.off.x, RAIN_CLOUD_Y, cam.z + rc.off.y);
        ground = Vec2::new(t.translation.x, t.translation.z);
        *v = vis;
    }
    for (mut d, mut t, mut v) in &mut rain_q {
        *v = vis;
        if raining {
            d.off.y -= 55.0 * dt; // fall
            if d.off.y < 0.0 {
                d.off.y += RAIN_FALL_TOP; // wrap to the top of the column
            }
            // Fall straight down under the cloud's current ground footprint.
            t.translation = Vec3::new(ground.x + d.off.x, d.off.y, ground.y + d.off.z);
        }
    }
}

/// Scroll the shared water ripple so pools shimmer + drift (all water at once).
pub(crate) fn animate_water(
    time: Res<Time>,
    wa: Option<Res<WorldAssets>>,
    mut mats: ResMut<Assets<StandardMaterial>>,
) {
    let Some(wa) = wa else { return };
    if let Some(m) = mats.get_mut(&wa.water_mat) {
        let t = time.elapsed_secs();
        m.uv_transform = bevy::math::Affine2::from_scale_angle_translation(
            Vec2::splat(2.2),
            0.0,
            Vec2::new(t * 0.035, t * 0.055),
        );
    }
}
