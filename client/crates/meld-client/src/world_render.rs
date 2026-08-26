//! World rendering + scene setup: asset loading, the biome ground shader,
//! sky/day-night, weather (rain/ashfall), clouds, ground detail, water.
//! Extracted from `main.rs` during the module reorg.

use std::collections::HashMap;

use bevy::gltf::GltfAssetLabel;
use bevy::light::NotShadowCaster;
use bevy::pbr::{ExtendedMaterial, MaterialExtension};
use bevy::render::render_resource::AsBindGroup;
use bevy::shader::ShaderRef;

/// How far the sea bed falls away from the shoreline, in world units. Presentation only —
/// the server's coastline is a flat predicate, and nothing swims — so this lives here
/// rather than in `meld_proto::coast`: it decides how water LOOKS, not where it is.
pub(crate) const SEA_DEPTH: f32 = 7.0;

/// How wide a disc of snow follows the player, and how high it starts. Wider and lower
/// than the rain's: snow is legible further out because it falls slowly, and starting it
/// too high wastes flakes above the camera where nobody sees them.
const SNOW_RADIUS: f32 = 34.0;
const SNOW_FALL_TOP: f32 = 16.0;

use meld_client::hd2d::{self, CharacterFrames};

use super::*;

/// Max radial biome rings the ground shader blends across (near the player). A
/// section = one concentric ring, so this bounds how many nearby sections colour the
/// visible ground; deeper/closer sections beyond the window clamp to the ends.
pub(crate) const MAX_BIOME_RINGS: usize = 32;

/// The sliding ground plane's size + tessellation. The plane follows the player so
/// there's always ground underfoot, and its vertices are displaced into hills by
/// `terrain_height`. Bevy's `Plane3d` emits `subdivisions + 2` vertices per side, so
/// the vertex spacing is `size / (subdivisions + 1)`.
pub(crate) const GROUND_SIZE: f32 = 2000.0;
pub(crate) const GROUND_SUBDIVISIONS: u32 = 400;
/// World distance between adjacent ground vertices — the lattice the follow snaps to
/// (see [`follow_world_ground`]) so the tessellation stops swimming under the hills.
pub(crate) const GROUND_CELL: f32 = GROUND_SIZE / (GROUND_SUBDIVISIONS as f32 + 1.0);

/// Uniform for [`GroundBiome`] — the ACTUAL per-section biome rings, so the ground
/// matches each section's real biome (radius ring) instead of fixed distance bands.
/// `rings[i] = (outer_radius, biome_index, _, _)`, sorted by radius; `count` entries
/// are live. `update_ground_biome_rings` rebuilds it from the streamed sections.
// `dead_code` here is about the `check` fn the ShaderType derive generates per field,
// not about the fields: every one of them is read by the WGSL side of this uniform.
/// Shader uniform peak slots (must equal `meld_proto::terrain::MAX_PEAKS`).
const PEAK_SLOTS: usize = 24;

/// `vec4` slots the ground shader reserves for STRAITS (WG-7 continents) — **two per
/// strait**, so this is `2 * coast::MAX_STRAITS`. Windowed around the player's radius the
/// way the biome rings are, because the world streams outward without bound and the only
/// straits that can be on screen are the ones near you.
const STRAIT_SLOTS: usize = meld_proto::coast::MAX_STRAITS * 2;

/// `vec4` slots for the coast's LOBES — bays and isles, one `vec4` each
/// (`[cx, cz, radius, kind]`), windowed around the player like the straits.
const LOBE_SLOTS: usize = meld_proto::coast::MAX_LOBES;

/// `vec4` slots for standing inland water (`[cx, cz, radius, level]`) and for river-chain
/// nodes (`[x, z, half_width, chain_start]`), both windowed around the player.
const BASIN_SLOTS: usize = meld_proto::coast::MAX_BASINS;
const RIVER_SLOTS: usize = meld_proto::coast::MAX_RIVER_NODES;

/// `dead_code` is allowed for exactly this one item: the `ShaderType` derive generates a
/// per-field `check` fn that nothing ever calls, and there is no way to annotate code a
/// macro emits. Scoped to a submodule so the rest of this file still reports its own dead
/// code honestly — every field here IS read, by the WGSL side of the uniform.
mod biome_params {
    #![allow(dead_code)]
    use super::{
        BASIN_SLOTS, LOBE_SLOTS, MAX_BIOME_RINGS, PEAK_SLOTS, RIVER_SLOTS, STRAIT_SLOTS,
    };
    use bevy::prelude::*;
    use bevy::render::render_resource::ShaderType;

    #[derive(Clone, Copy, ShaderType, Debug)]
    pub(crate) struct BiomeParams {
        pub(crate) rings: [Vec4; MAX_BIOME_RINGS],
        pub(crate) count: u32,
        pub(crate) uv_scale: f32,
        pub(crate) blend_half: f32,
        /// Heightmap displacement amplitude: 1.0 in the Overworld, 0.0 elsewhere (City +
        /// menus stay flat — see `set_ground_terrain_amp`). Also the struct's tail pad.
        pub(crate) terrain_amp: f32,
        /// This run's terrain offset (mirrors `world_render::terrain_offset` / the server's
        /// `run.started.terrain_offset`), so the displaced ground matches every entity's Y and
        /// the world looks different every run.
        pub(crate) terrain_off: Vec2,
        /// Explicit pad so `peaks` (a vec4 array, 16-aligned) starts on a 16-byte boundary in
        /// BOTH the Rust (encase) and WGSL uniform layouts — no implicit padding to mismatch.
        pub(crate) _pad_peaks: Vec2,
        /// Authored CLIMBABLE peaks (`[cx, cz, radius, height]`), summed onto the displaced
        /// ground so each mountain renders (mirrors `terrain::peak_height`). Windowed to
        /// `MAX_PEAKS`; `peak_count` live entries.
        pub(crate) peaks: [Vec4; PEAK_SLOTS],
        pub(crate) peak_count: u32,
        // Three SCALAR pads (not a `[u32; 3]` — a u32 array needs a 16-byte stride in a
        // uniform, which fails validation) to round the struct out to a 16-byte multiple.
        pub(crate) _pad_pc0: u32,
        pub(crate) _pad_pc1: u32,
        pub(crate) _pad_pc2: u32,
        /// The COASTLINE, straight from [`meld_proto::coast`]:
        /// `(arc_half_rad, neck_reach, peninsula_length, channel_land_share)`. Carried in
        /// the uniform rather than baked into the shader so **the sea the player sees is
        /// the sea the server collides with** — the shoreline is authored in two scenes
        /// that cannot see each other (the arena and `Screen::City`), and two hand-placed
        /// shorelines drift the way every other duplicated rule in this repo has.
        pub(crate) coast: Vec4,
        /// Peninsula widths, also from `coast`:
        /// `(neck_half_width, city_half_width, tip_taper, sea_depth)`.
        pub(crate) coast_w: Vec4,
        /// **CONTINENTS (WG-7): this world's STRAITS**, the inland seas that separate one
        /// landmass from the next. Two `vec4`s each, carrying the same eight numbers as
        /// [`meld_proto::coast::Strait`]: slot `2k` is
        /// `(r_center, r_half, theta_center, theta_half)` and `2k+1` is
        /// `(bridge0_theta, bridge0_half, bridge1_theta, bridge1_half)`.
        ///
        /// In the uniform for the same reason `coast` is: the server collides against
        /// `coast::is_ocean_with` and this shader ramps its beach over
        /// `coast::sea_depth_with`, and a shoreline the shader has not been told about is
        /// walkable ground drawn over open water.
        pub(crate) straits: [Vec4; STRAIT_SLOTS],
        pub(crate) strait_count: u32,
        // Three SCALAR pads, as with `peak_count` — a `[u32; 3]` needs a 16-byte stride in a
        // uniform and fails validation.
        pub(crate) _pad_sc0: u32,
        pub(crate) _pad_sc1: u32,
        pub(crate) _pad_sc2: u32,
        /// The coast's own shape: bays (water) and isles (land), `[cx, cz, radius, kind]`
        /// each. One array for both, because they are one primitive.
        pub(crate) lobes: [Vec4; LOBE_SLOTS],
        pub(crate) lobe_count: u32,
        pub(crate) _pad_lc0: u32,
        pub(crate) _pad_lc1: u32,
        pub(crate) _pad_lc2: u32,
        /// Standing inland water, `[cx, cz, radius, level]` each — where `level` is the
        /// water SURFACE elevation in the same units as `terrain::height`. That fourth
        /// number is what makes inland water a different thing from the sea, whose level is
        /// globally zero.
        pub(crate) basins: [Vec4; BASIN_SLOTS],
        /// River-chain nodes, `[x, z, half_width, chain_start]` each. A node with
        /// `chain_start >= 0.5` begins a new chain, and the gap before it is a FORD.
        pub(crate) rivers: [Vec4; RIVER_SLOTS],
        pub(crate) basin_count: u32,
        pub(crate) river_count: u32,
        pub(crate) _pad_wc0: u32,
        pub(crate) _pad_wc1: u32,
        /// The Shift's tell: `(inner_radius, outer_radius, intensity, 0)`. A region is a
        /// radius ring in the WG-4 fan and this ground is already painted in rings, so
        /// the doomed annulus needs no second coordinate system. `intensity == 0` is the
        /// resting state and costs the shader one compare.
        pub(crate) shift: Vec4,
        /// Open-water animation: `(seconds, 0, 0, 0)`. The sea needs a clock and this
        /// shader had none — the ocean was a static tile while every pond prop drifted its
        /// own material UVs from [`animate_water`]. A `Vec4` rather than a bare `f32` so it
        /// lands 16-byte aligned after `shift` and adds no padding to either mirror.
        pub(crate) sea_anim: Vec4,
    }

    impl Default for BiomeParams {
        fn default() -> Self {
            BiomeParams {
                rings: [Vec4::ZERO; MAX_BIOME_RINGS],
                count: 0,
                uv_scale: 1.0 / 3.0,
                blend_half: 18.0,
                // Default flat: menus/join/city render level ground. The Overworld flips it
                // to 1.0 on entry (`set_ground_terrain_amp`).
                terrain_amp: 0.0,
                terrain_off: Vec2::ZERO,
                _pad_peaks: Vec2::ZERO,
                coast: Vec4::ZERO,
                coast_w: Vec4::new(
                    meld_proto::coast::NECK_HALF_WIDTH,
                    meld_proto::coast::CITY_HALF_WIDTH,
                    meld_proto::coast::TIP_TAPER,
                    super::SEA_DEPTH,
                ),
                straits: [Vec4::ZERO; STRAIT_SLOTS],
                strait_count: 0,
                _pad_sc0: 0,
                _pad_sc1: 0,
                _pad_sc2: 0,
                lobes: [Vec4::ZERO; LOBE_SLOTS],
                lobe_count: 0,
                _pad_lc0: 0,
                _pad_lc1: 0,
                _pad_lc2: 0,
                basins: [Vec4::ZERO; BASIN_SLOTS],
                rivers: [Vec4::ZERO; RIVER_SLOTS],
                basin_count: 0,
                river_count: 0,
                _pad_wc0: 0,
                _pad_wc1: 0,
                shift: Vec4::ZERO,
                sea_anim: Vec4::ZERO,
                peaks: [Vec4::ZERO; PEAK_SLOTS],
                peak_count: 0,
                _pad_pc0: 0,
                _pad_pc1: 0,
                _pad_pc2: 0,
            }
        }
    }
}
pub(crate) use biome_params::BiomeParams;


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
    /// The SEA's own tiles, so the coast is drawn with the same art the city's water and
    /// every pond/bog-pool/frozen-pond already uses. It was a pair of hardcoded RGB
    /// constants in the shader at first — which meant the arena and Last City rendered the
    /// same sea two different ways, in exactly the two scenes `meld_proto::coast` exists to
    /// keep from disagreeing. Three, keyed off the fragment's biome, because a tundra shore
    /// is ice and a mire shore is bog: the same mapping `WorldAssets::water_mats` uses.
    #[texture(107)]
    water_clear: Handle<Image>,
    #[texture(108)]
    water_bog: Handle<Image>,
    #[texture(109)]
    water_ice: Handle<Image>,
    #[uniform(106)]
    params: BiomeParams,
}

impl MaterialExtension for GroundBiome {
    fn fragment_shader() -> ShaderRef {
        "shaders/ground_biome.wgsl".into()
    }
    /// Custom vertex shader displaces the ground into rolling hills (`terrain_height`).
    fn vertex_shader() -> ShaderRef {
        "shaders/ground_biome.wgsl".into()
    }
    /// ⚠️ THE SHADOW AND DEPTH PASSES TAKE THEIR VERTEX STAGE FROM HERE, not from
    /// `vertex_shader`. Leave it unimplemented and the ground is rasterized FLAT into the
    /// shadow map while the visible ground rolls into hills — so the terrain shadows itself
    /// with a sheet that is never drawn, measured at 7x darker across the whole world.
    fn prepass_vertex_shader() -> ShaderRef {
        "shaders/ground_prepass.wgsl".into()
    }
}

/// The SKY: a camera-anchored gradient dome with a sun in it.
///
/// See `sky_dome.wgsl`. The sky used to be a single `ClearColor`, which is fine behind a
/// diorama and wrong for anything that reflects it — water most of all.
#[derive(Asset, AsBindGroup, Reflect, Debug, Clone)]
pub(crate) struct SkyDome {
    #[uniform(100)]
    pub(crate) horizon: Vec4,
    #[uniform(100)]
    pub(crate) zenith: Vec4,
    /// `xyz` direction TO the sun, `w` the daylight factor.
    #[uniform(100)]
    pub(crate) sun_dir: Vec4,
    /// `rgb` the sun's colour, `a` how far its glow bleeds.
    #[uniform(100)]
    pub(crate) sun_col: Vec4,
}

impl Material for SkyDome {
    fn fragment_shader() -> ShaderRef {
        "shaders/sky_dome.wgsl".into()
    }
    /// A backdrop, not geometry: never occludes, never lit, never shadows.
    fn alpha_mode(&self) -> AlphaMode {
        AlphaMode::Opaque
    }
    /// ⚠️ CULLING OFF, or the dome does not draw AT ALL. We stand INSIDE this sphere, so
    /// every face we look at is a back face — and a custom `Material` defaults to back-face
    /// culling, unlike `StandardMaterial` where `cull_mode` is a field you can see. The
    /// first version compiled, booted, threw no errors and rendered absolutely nothing;
    /// only a garish diagnostic colour proved the geometry was being thrown away rather
    /// than the gradient being too subtle.
    fn specialize(
        _pipeline: &bevy::pbr::MaterialPipeline,
        descriptor: &mut bevy::render::render_resource::RenderPipelineDescriptor,
        _layout: &bevy::mesh::MeshVertexBufferLayoutRef,
        _key: bevy::pbr::MaterialPipelineKey<Self>,
    ) -> Result<(), bevy::render::render_resource::SpecializedMeshPipelineError> {
        descriptor.primitive.cull_mode = None;
        Ok(())
    }
}

/// Marks the sky dome so [`anchor_sky_dome`] can keep it centred on the camera.
#[derive(Component)]
pub(crate) struct SkyDomeMesh;

/// Keep the dome centred on the camera. A sky that moves relative to the viewer reads as a
/// ball you could walk to; one that never moves reads as infinitely far away.
pub(crate) fn anchor_sky_dome(
    cam_q: Query<&Transform, With<Camera3d>>,
    mut q: Query<&mut Transform, (With<SkyDomeMesh>, Without<Camera3d>)>,
) {
    let Ok(cam) = cam_q.single() else { return };
    for mut tf in &mut q {
        tf.translation = cam.translation;
    }
}

/// Standing water that is a MESH: the maze's pools and Last City's sea.
///
/// See `water_surface.wgsl`. Depth comes from the SHAPE (a basin is deepest in the middle)
/// rather than from a depth buffer, because our water is centimetres deep and every
/// measured approach resolves it to nothing.
#[derive(Asset, AsBindGroup, Reflect, Debug, Clone)]
pub(crate) struct WaterSurface {
    /// `(seconds, wave_scale, steepness, mode)`; mode 0 = basin, 1 = open plane.
    #[uniform(100)]
    pub(crate) params: Vec4,
    #[uniform(100)]
    pub(crate) deep: Vec4,
    #[uniform(100)]
    pub(crate) shallow: Vec4,
    #[uniform(100)]
    pub(crate) edge: Vec4,
}

impl MaterialExtension for WaterSurface {
    fn fragment_shader() -> ShaderRef {
        "shaders/water_surface.wgsl".into()
    }
}

/// Water-surface material type (StandardMaterial lighting + the wave extension).
pub(crate) type WaterMat = ExtendedMaterial<StandardMaterial, WaterSurface>;

/// The blended-biome ground material type (StandardMaterial lighting + our extension).
pub(crate) type GroundMat = ExtendedMaterial<StandardMaterial, GroundBiome>;

/// The ten boss/elite encounters (gothic / magitech-golem / nightmare), each with a
/// PixelLab sprite set under `assets/bosses/<key>/`. Tiers: elite (gloamhound,
/// rustfang), miniboss (choirmother, pyrewarden), dungeon (sepulcher, hollowbishop),
/// region (ironmaw, weepingcolossus), biome (miredrowned, ashenleviathan).
///
/// Read off [`meld_proto::bosses`], the registry the server names them from and the
/// client titles them from — a hand-copied list here is a list that goes stale against
/// the `boss:<key>` tags actually arriving on the wire.
pub(crate) fn boss_keys() -> impl Iterator<Item = &'static str> {
    meld_proto::bosses::keys()
}

/// Creature species whose animated sprite set is INSTALLED under `assets/creatures/`.
/// A species listed here stops being a single frozen 32px billboard and starts turning,
/// walking and swinging like everything else in the world.
///
/// **`<key>_pack_leader` is its own entry, and that is the point.** A pack's leader and
/// its rank and file are the same species at 1.7x and 0.45x HP; drawing them from one
/// sprite made a 3.8x health gap read as a rendering bug, and scaling one sprite only
/// ever made a bigger or smaller copy of the same animal.
///
/// The BASE key is the ordinary creature — a lone spawn, or a pack's minions — because
/// that is the common case; the LEADER is the variant that has to earn its own art.
///
/// Held against what is actually on disk by `every_installed_creature_set_is_loaded`, so
/// art that lands unlisted — art nobody would ever see — fails rather than sitting unused.
pub(crate) const CREATURE_CHARS: &[&str] = &[
    "bog_ooze",
    "bog_ooze_baby",
    "bog_ooze_belcher",
    "bog_ooze_grump",
    "bog_ooze_pack_leader",
    "bog_serpent",
    "bog_serpent_female",
    "bog_serpent_pack_leader",
    "bog_serpent_slither",
    "bog_serpent_twin_tail",
    "bog_stinger",
    "bog_stinger_buzz",
    "bog_stinger_licker",
    "bog_stinger_pack_leader",
    "bog_stinger_piercer",
    "briarling_pack_leader",
    "briarling_piper",
    "briarling_thistleback",
    "cinder_imp",
    "cinder_imp_pack_leader",
    "cinder_imp_wolf",
    "dune_colossus",
    "dune_colossus_pack_leader",
    "dune_wyrm",
    "dune_wyrm_pack_leader",
    "ember_wisp",
    "ember_wisp_pack_leader",
    "forest_bloom_stalker",
    "forest_bloom_stalker_adult",
    "forest_bloom_stalker_baby",
    "forest_bloom_stalker_pack_leader",
    "frost_lurker",
    "frost_lurker_pack_leader",
    "glacier_maw",
    "glacier_maw_pack_leader",
    "ice_revenant",
    "ice_revenant_pack_leader",
    "magma_golem",
    "magma_golem_pack_leader",
    "myconid_brute",
    "myconid_brute_boss",
    "myconid_brute_mage",
    "myconid_brute_minion",
    "myconid_brute_pack_leader",
    "sand_shade",
    "sand_shade_pack_leader",
    "sporeling",
    "sporeling_baby",
    "sporeling_healer",
    "sporeling_pack_leader",
    "sporeling_sprout",
    "thornback_boar",
    "thornback_boar_beta",
    "thornback_boar_charger",
    "thornback_boar_goarer",
    "thornback_boar_pack_leader",
    "verdant_ooze",
    "verdant_ooze_blob",
    "verdant_ooze_blopper",
    "verdant_ooze_healer",
    "verdant_ooze_pack_leader",
];

/// Which installed set a creature draws from: a pack leader's own art when it has some,
/// the ordinary creature's otherwise, and nothing at all if the species has no art yet.
///
/// The FALLBACK is the point — art lands in batches, so a species can have its ordinary
/// form drawn and its leader not, and a leader rendering as nothing is far worse than a
/// leader that merely looks like a big one of its own kind. Split out from
/// [`WorldAssets::creature_frames`] so the rule can be tested without standing up the
/// whole asset resource.
pub(crate) fn creature_art_key(
    kind: &str,
    leader: bool,
    installed: impl Fn(&str) -> bool,
) -> Option<String> {
    if leader {
        let boss_of_the_pack = format!("{kind}_pack_leader");
        if installed(&boss_of_the_pack) {
            return Some(boss_of_the_pack);
        }
    }
    installed(kind).then(|| kind.to_string())
}

/// Shared meshes/materials + the psyker sprite set, built once at startup so the
/// overworld sync can spawn 3D entities without rebuilding assets each frame.
#[derive(Resource)]
pub(crate) struct WorldAssets {
    /// Per-class hero sprite sets (bespoke PixelLab art, one folder per class under
    /// `characters/<class>/`), keyed by `CharacterClass` wire key ("explorer", "psyker",
    /// "resonant", "shifter", "phoenix_guard"). Look up via [`Self::class_frames`], which
    /// falls back to the Explorer for any unknown key.
    pub(crate) class_chars: HashMap<String, CharacterFrames>,
    /// Boss/elite encounter sprites (PixelLab, `bosses/<key>/`), keyed by boss id
    /// (`gloamhound`, `ironmaw`, …). Each has `walk` + `attack` + its ability clips
    /// (see [`boss_keys`]). Look up via [`Self::boss_frames`]. Used by scripted
    /// encounters (gameplay wiring lands separately) + the `MELD_BOSS` preview.
    pub(crate) boss_chars: HashMap<String, CharacterFrames>,
    /// Bespoke HD-2D pixel-art billboards (PixelLab) for world props, keyed by full
    /// prop key: `obstacle_<kind>`, `resource_<kind>`, `connector_<kind>`,
    /// `item_<name>`, `marker_<name>`. Preferred over the 3D `prop_scenes`/primitives
    /// where present, so the world matches the hand-drawn sprite style.
    pub(crate) prop_sprites: HashMap<String, Handle<Image>>,
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
    /// Per-species animated creature sets (`assets/creatures/<key>/`), keyed by the
    /// [`CREATURE_CHARS`] entry — `<kind>` for an ordinary creature or a pack's minions,
    /// `<kind>_pack_leader` for the one leading it. Absent for a species whose art has
    /// not landed, which keeps it
    /// on the old single-png billboard rather than on missing-asset errors.
    pub(crate) creature_chars: HashMap<String, CharacterFrames>,
    pub(crate) monster_pool: Vec<Handle<Image>>,
    /// Real 3D prop models (Kenney Nature Kit, CC0) keyed by terrain-obstacle kind →
    /// several `(scene, baked_scale)` variants (picked per-entity by id hash), so the
    /// world is built from actual geometry instead of flat billboards.
    pub(crate) prop_scenes: HashMap<String, Vec<(Handle<WorldAsset>, f32)>>,
    /// 3D harvest-node models keyed by resource content id → `(scene, baked_scale)`.
    pub(crate) resource_scenes: HashMap<String, (Handle<WorldAsset>, f32)>,
    pub(crate) portal_sprite: Handle<Image>,
    pub(crate) portal_mesh: Handle<Mesh>,
    pub(crate) portal_mat: Handle<StandardMaterial>,
    pub(crate) rock_mesh: Handle<Mesh>,
    /// Unit cube (origin at its base) for solid dungeon walls — adjacent wall tiles
    /// stamped with this merge into a continuous masonry wall (DG-6b), unlike the
    /// rounded `rock_mesh` which reads as scattered boulders.
    pub(crate) wall_mesh: Handle<Mesh>,
    /// Tiling cobblestone/masonry texture for dungeon walls (repeat-sampled), tinted
    /// per biome — so walls read as fitted stone, not flat blocks.
    pub(crate) wall_tex: Handle<Image>,
    pub(crate) water_mesh: Handle<Mesh>,
    /// A lily pad / floating leaf: the SAME organic lobed outline the pools use, just
    /// small. Bog water has almost no value contrast against the mire's ground, so a
    /// merged mere still read as a dark patch of mud — and the fix is not to tint swamp
    /// water blue, it is to put on its surface the things that make real still water
    /// legible as water. No new art: `blob_mesh` already draws a leaf if you shrink it.
    pub(crate) pad_mesh: Handle<Mesh>,
    /// A few leaf greens, so a pool's pads are not all the same colour.
    pub(crate) pad_mats: Vec<Handle<StandardMaterial>>,
    /// Per-water-kind materials (`pond`/`bog_pool`/`frozen_pond`), each wearing a
    /// bespoke pixel-art water tile and drifting via [`animate_water`]. Keyed by the
    /// `SnapshotEntity` obstacle name; fall back to `pond` via [`Self::water_mat`].
    pub(crate) water_mats: HashMap<String, Handle<WaterMat>>,
    pub(crate) ground_tex: Vec<Handle<Image>>, // per-biome textures; also dress terrace tops/cliffs
}

impl WorldAssets {
    /// The hero sprite set for a class wire key, falling back to the Explorer for any
    /// key without bespoke art (keeps rendering robust if a new class ships before
    /// its art does).
    /// The water material for an obstacle kind (`pond`/`bog_pool`/`frozen_pond`),
    /// falling back to the clear `pond` water for any unmapped kind.
    pub(crate) fn water_mat(&self, kind: &str) -> Handle<WaterMat> {
        self.water_mats
            .get(kind)
            .or_else(|| self.water_mats.get("pond"))
            .expect("pond water material always loaded")
            .clone()
    }

    /// The sprite set for a boss id (see [`boss_keys`]), or `None` if unknown.
    /// The palette a boss wears at a given depth band (`boss_band:<n>` on the wire,
    /// server-assigned from the level it is met at). Band 0 is the sprite's own
    /// colours; deeper bands push it hotter and darker, so meeting the Choirmother
    /// at distance 2000 reads as a worse Choirmother before it takes a turn.
    /// Applied as a material tint rather than new art — one boss, four moods.
    pub(crate) fn boss_frames(&self, key: &str) -> Option<&CharacterFrames> {
        self.boss_chars.get(key)
    }

    /// The animated set for a creature, or `None` if this species is still on the old
    /// static billboard. `minion` picks the runt's own art and FALLS BACK to the
    /// species' — a species may get its leader art before its minion art, and half a pack
    /// rendering as nothing at all is worse than half a pack sharing one sprite.
    pub(crate) fn creature_frames(&self, kind: &str, leader: bool) -> Option<&CharacterFrames> {
        let key = creature_art_key(kind, leader, |k| self.creature_chars.contains_key(k))?;
        self.creature_chars.get(&key)
    }

    pub(crate) fn class_frames(&self, class: &str) -> &CharacterFrames {
        self.class_chars
            .get(class)
            .or_else(|| self.class_chars.get("explorer"))
            .expect("explorer class sprite always loaded")
    }
}

/// Load an image with a Repeat sampler so it tiles across the big ground plane.
pub(crate) fn load_tiled(assets: &AssetServer, path: &str) -> Handle<Image> {
    assets
        .load_builder()
        .with_settings(|s: &mut ImageLoaderSettings| {
            s.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
                address_mode_u: ImageAddressMode::Repeat,
                address_mode_v: ImageAddressMode::Repeat,
                ..ImageSamplerDescriptor::nearest()
            });
        })
        .load(path.to_string())
}

/// Build the HD-2D world: camera + post stack, sun, the lit ground, and the shared
/// asset handles. Replaces the old flat Camera2d overworld (CANON D16 all-Bevy).
/// Keeps `water_wave.wgsl` alive.
///
/// A shader library only registers its `#define_import_path` once the asset is LOADED, and
/// nothing else references this file — no material names it as a `ShaderRef`, because it is
/// imported rather than run. Without a handle held somewhere it is never loaded, and every
/// pipeline that imports it fails to build at run time while compiling perfectly.
#[derive(Resource)]
pub(crate) struct WaveLib(#[allow(dead_code)] Handle<Shader>);



pub(crate) fn setup(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut mats: ResMut<Assets<StandardMaterial>>,
    mut ground_mats: ResMut<Assets<GroundMat>>,
    mut water_mats: ResMut<Assets<WaterMat>>,
    mut sky_mats: ResMut<Assets<SkyDome>>,
    mut images: ResMut<Assets<Image>>,
    assets: Res<AssetServer>,
    look: Res<hd2d::Look>,
) {
    hd2d::seed_look_file(&look);
    // Load the shared wave library and hold it (see `WaveLib`).
    commands.insert_resource(WaveLib(assets.load("shaders/water_wave.wgsl")));

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
    // Detailed per-biome ground tiles extracted from the PixelLab Wang tilesets
    // (seamless "full lower" terrain tile per biome); the `GroundBiome` shader still
    // applies each biome's colour tint on top.
    let ground_tex: Vec<Handle<Image>> = [
        "ground/tile_forest.png",  // Forest
        "ground/tile_desert.png",  // Desert
        "ground/tile_ashfall.png", // Ashfall
        "ground/tile_tundra.png",  // Tundra
        "ground/tile_mire.png",    // Mire
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
            // The same three water tiles the pond/bog-pool/frozen-pond props use, so the
            // arena's coast is drawn with the art the rest of the game's water already is.
            water_clear: load_tiled(&assets, "ground/water_clear.png"),
            water_bog: load_tiled(&assets, "ground/water_bog.png"),
            water_ice: load_tiled(&assets, "ground/water_ice.png"),
            // Rings start empty; `update_ground_biome_rings` fills them from the
            // streamed sections each frame (count 0 ⇒ shader falls back to forest).
            params: BiomeParams::default(),
        },
    });
    commands.spawn((
        WorldGround,
        // The ground CASTS, and correctly. It was briefly `NotShadowCaster` because the
        // shadow pass rasterized this plane FLAT while the visible ground rolled into hills,
        // so the terrain shadowed itself with a sheet that is never drawn. That was a missing
        // `prepass_vertex_shader` (see `GroundBiome`) rather than a reason to stop casting:
        // with the displacement applied in the shadow pass too, hills shade each other again,
        // which is where the scene's contrast comes from.
        // Square (was 2000×600, a corridor) so the WG-4 radial fan has ground in
        // every direction the player roams. SUBDIVIDED into a fine grid so the ground
        // shader's vertex displacement (`terrain_height`) reads as smooth rolling hills
        // rather than a tilted quad — ~5-unit cells over the hill wavelength (~350u).
        Mesh3d(meshes.add(
            Plane3d::default()
                .mesh()
                .size(GROUND_SIZE, GROUND_SIZE)
                .subdivisions(GROUND_SUBDIVISIONS),
        )),
        MeshMaterial3d(ground_mat.clone()),
        Transform::default(),
    ));

    // Shared assets. HD-2D split: 2D pixel sprites for the actors (heroes + monster
    // billboards, from DCSS/RLTiles — public domain), real 3D models for the world
    // (obstacles + harvest nodes, from Kenney Nature Kit — CC0). See assets/ATTRIBUTIONS.md.
    let ld = |p: &str| assets.load::<Image>(p.to_string());
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
        // The oozes, until their animated sets land (`CREATURE_CHARS`).
        ("verdant_ooze", "monsters/jelly.png"),
        ("bog_ooze", "monsters/acid_blob.png"),
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
    let sc = |p: &str, s: f32| -> (Handle<WorldAsset>, f32) {
        (
            assets.load(GltfAssetLabel::Scene(0).from_asset(format!("models/nature/{p}.glb"))),
            s,
        )
    };
    // Terrain-obstacle kind → real 3D model variants (picked per entity by id hash),
    // so every biome's cover is actual geometry that lights and casts shadow. Water
    // kinds (pond/lava/…) stay flat pools; hard fallbacks use the boulder mesh.
    let prop_scenes: HashMap<String, Vec<(Handle<WorldAsset>, f32)>> = [
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
    let resource_scenes: HashMap<String, (Handle<WorldAsset>, f32)> = [
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

    // Per-class hero sprites (PixelLab v3, 8-directional): idle rotations + a `walk`
    // clip plus battle `attack` and one clip per special ability, loaded into
    // `CharacterFrames::clips` and played in battle via `CharSprite::action`.
    fn class_clips(class: &str) -> &'static [(&'static str, usize)] {
        match class {
            "shifter" => &[
                ("walk", 8), ("attack", 8), ("backstab", 8), ("flicker", 8), ("ransack", 8),
            ],
            // The Explorer's own kit has no bespoke ABILITY art yet, so its abilities
            // fall back to the attack clip; the martial animations moved with the kit to
            // the Hunter.
            "explorer" => &[("walk", 8), ("attack", 8)],
            "hunter" => &[
                ("walk", 8), ("attack", 8), ("power_strike", 8), ("second_wind", 8),
                ("snare", 8), ("frenzy", 8),
            ],
            "psyker" => &[
                ("walk", 8), ("attack", 8), ("gravity_well", 8), ("kinetic_aegis", 8),
                ("mind_spike", 8), ("temporal_anchor", 8),
            ],
            "resonant" => &[
                ("walk", 8), ("attack", 8), ("transfuse", 8), ("regen_boon", 8), ("ward", 8),
            ],
            "phoenix_guard" => &[
                ("walk", 8), ("attack", 8), ("silvered_strike", 8), ("rite_of_rest", 8),
                ("holy_censure", 8), ("purging_light", 8),
            ],
            // The four newest orders have idle rotations, a walk cycle and a battle
            // attack, and nothing else yet — their abilities fall through to the attack.
            // Declare only what is ON DISK: a clip named here with no frames beside it is
            // 64 asset-loader errors a launch, which is exactly what the Explorer shipped
            // the moment it stopped being a copy of the Hunter's folder and lost the five
            // martial clips it had been borrowing.
            "smithwright" | "keeper" | "iron_hull" | "rift_knight" => {
                &[("walk", 8), ("attack", 8)]
            }
            _ => &[("walk", 8)],
        }
    }
    // Every class the client can muster, off the client's own roster — a hand-written
    // list here is a class whose art silently never loads, and the Smithwright and the
    // Keeper spent a release wearing the Explorer's coat because of exactly that.
    let class_chars: HashMap<String, CharacterFrames> = crate::screens::CLASS_INFO
        .iter()
        .map(|c| c.key)
        .map(|class| {
            (
                class.to_string(),
                hd2d::load_character_clips(&assets, &format!("characters/{class}"), class_clips(class)),
            )
        })
        .collect();

    // Boss/elite encounter clip sets (PixelLab, `bosses/<key>/`). Frame counts are
    // per-clip: template `walk` cycles are 6f for humanoids / 8f for quadrupeds; the
    // v3 attack + ability clips are 8f. miredrowned's two abilities + ashenleviathan's
    // `eruption` weren't generated (PixelLab credit cap) — they're simply absent.
    fn boss_clips(key: &str) -> &'static [(&'static str, usize)] {
        match key {
            "gloamhound" => &[("walk", 8), ("attack", 8), ("howl", 8), ("pounce", 8)],
            "rustfang" => &[("walk", 8), ("attack", 8), ("slam", 8), ("overcharge", 8)],
            "choirmother" => &[("walk", 6), ("attack", 8), ("wail", 8), ("grasp", 8)],
            "pyrewarden" => &[("walk", 6), ("attack", 8), ("furnace_slam", 8), ("ember_burst", 8)],
            "sepulcher" => &[("walk", 8), ("attack", 8), ("rend", 8), ("phantom", 8)],
            "hollowbishop" => &[("walk", 6), ("attack", 8), ("soulfire", 8), ("bone_nova", 8)],
            "ironmaw" => &[("walk", 8), ("attack", 8), ("devour", 8), ("reactor_roar", 8)],
            "weepingcolossus" => &[("walk", 6), ("attack", 8), ("chain_sweep", 8), ("sorrow_quake", 8)],
            "miredrowned" => &[("walk", 6), ("attack", 8)],
            "ashenleviathan" => &[("walk", 8), ("attack", 8), ("cinder_charge", 8)],
            // The barrow's fae court: walk + attack, no ability art yet, so its kit
            // falls through to the attack clip.
            "briarlord" => &[("walk", 8), ("attack", 8)],
            _ => &[("walk", 8), ("attack", 8)],
        }
    }
    let boss_chars: HashMap<String, CharacterFrames> = boss_keys()
        .map(|key| {
            (
                key.to_string(),
                hd2d::load_character_clips(&assets, &format!("bosses/{key}"), boss_clips(key)),
            )
        })
        .collect();

    // Creature sets. Walk + attack only — a creature has no ability art, so its clips
    // fall through to the attack the way a newly-drawn class's do. The WALK is drawn for
    // all eight facings because the overworld shows a creature from every angle; the
    // ATTACK is south-only because it is only ever seen in the arena, which faces the
    // party. See `hd2d::load_creature_clips`.
    let creature_chars: HashMap<String, CharacterFrames> = CREATURE_CHARS
        .iter()
        .map(|&key| {
            (
                key.to_string(),
                hd2d::load_creature_clips(
                    &assets,
                    &format!("creatures/{key}"),
                    &[("walk", 8, true), ("attack", 8, false)],
                ),
            )
        })
        .collect();

    let prop_sprites: HashMap<String, Handle<Image>> = PROP_KEYS
        .iter()
        .map(|&k| (k.to_string(), assets.load(format!("props/{k}.png"))))
        .collect();

    commands.insert_resource(WorldAssets {
        class_chars,
        creature_chars,
        boss_chars,
        prop_sprites,
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
        rock_mesh: meshes.add(Cuboid::new(1.0, 0.7, 1.0)),
        wall_mesh: meshes.add(Cuboid::new(1.0, 1.0, 1.0)), // unit cube for solid dungeon walls
        wall_tex: load_tiled(&assets, "ground/tile_street.png"), // cobblestone masonry for walls
        // A BASIN, not a disc: the rim stays proud of the terrain (no z-fighting) while the
        // surface sits below it, so water reads as sunk into the ground rather than floating
        // on it. Worst in the Mire, whose entire maze fill is water.
        water_mesh: meshes.add(hd2d::blob_basin_mesh(28, hd2d::WATER_BASIN_DEPTH, 0.74)),
        pad_mesh: meshes.add(hd2d::blob_mesh(14)),
        pad_mats: [
            LinearRgba::rgb(0.16, 0.34, 0.15),
            LinearRgba::rgb(0.22, 0.42, 0.18),
            LinearRgba::rgb(0.13, 0.27, 0.14),
        ]
        .into_iter()
        .map(|c| {
            mats.add(StandardMaterial {
                base_color: Color::from(c),
                perceptual_roughness: 0.85,
                ..default()
            })
        })
        .collect(),
        // Bespoke pixel-art water tiles (PixelLab), one per water kind — the BED, seen
        // through the wave surface `water_surface.wgsl` puts on top of them. Each kind
        // carries its own water: a bog is opaque and sour, a frozen pond is bright and
        // hard-rimmed, a clear pond is somewhere between.
        water_mats: [
            (
                "pond",
                "ground/water_clear.png",
                Vec4::new(0.10, 0.34, 0.48, 1.0),  // deep
                Vec4::new(0.42, 0.72, 0.76, 1.0),  // shallow
                Vec4::new(0.82, 0.92, 0.98, 0.45), // rim (a = how strongly it reads)
            ),
            (
                "bog_pool",
                "ground/water_bog.png",
                // A bog barely transmits light, and its shallows are only a little less
                // sour than its depths — so there is no beach-blue gradient in a swamp.
                // ⚠️ BOG WATER IS MOST OF THE MIRE'S SURFACE, so these two are what that
                // biome's brightness actually IS — not the ground tile under them. The
                // mire's fill kind is `bog_pool` at a 7.5x multiplier ("MOSTLY water: the
                // swamp is a flooded maze, land is the trail"), so lifting the terrain tint
                // barely moved it and lifting these moved it a lot.
                // ⚠️ BOG WATER MUST NOT BE THE COLOUR OF BOG. The first pass gave it
                // (0.23, 0.37, 0.19) — and the mire's ground tile averages (0.33, 0.32, 0.20).
                // The water was painted the same olive as the land it sits in, so a pool read
                // as a slightly different patch of ground with a rim around it: "you see the
                // edge of them and nothing else."
                //
                // Real standing bog water is DARK and reflective — a near-black mirror in a
                // green field, which is the contrast that says "this is not ground". Deep goes
                // almost to black, the shallows keep just enough peat to look foul, and the
                // rim reads harder so the bank stays legible.
                Vec4::new(0.03, 0.05, 0.04, 1.0),
                Vec4::new(0.13, 0.17, 0.11, 1.0),
                Vec4::new(0.42, 0.48, 0.30, 0.55),
            ),
            (
                "frozen_pond",
                "ground/water_ice.png",
                Vec4::new(0.28, 0.48, 0.60, 1.0),
                Vec4::new(0.74, 0.89, 0.95, 1.0),
                Vec4::new(0.97, 0.99, 1.0, 0.60),
            ),
        ]
        .iter()
        .map(|(kind, tex, deep, shallow, edge)| {
            (
                kind.to_string(),
                water_mats.add(WaterMat {
                    base: StandardMaterial {
                        base_color: Color::srgb(0.9, 0.94, 1.0),
                        base_color_texture: Some(load_tiled(&assets, tex)),
                        perceptual_roughness: 0.12,
                        metallic: 0.1,
                        // ⚠️ OPAQUE, NOT BLEND, AND IT IS WORTH 3.5x THE MIRE'S BRIGHTNESS.
                        // The mire is "MOSTLY water: the swamp is a flooded maze" — 740
                        // pools inside the interest radius — and blended water multiplies
                        // whatever is behind it, so hundreds of overlapping surfaces
                        // compounded into a void. Measured at pinned noon in fair weather:
                        // mean luminance 18.5 blended against 64.7 opaque, in a biome where
                        // the desert reads 95.
                        //
                        // It is also the correct model, not just the bright one: this shader
                        // composites the bed into the water itself, so alpha-blending the
                        // result over the bed AGAIN counts it twice.
                        alpha_mode: AlphaMode::Opaque,
                        ..default()
                    },
                    extension: WaterSurface {
                        // `(time, wave_scale, steepness, mode 0 = basin)`. Steepness 0 means
                        // FROZEN — see `water_surface.wgsl`; the frozen pond sets it below.
                        params: Vec4::new(0.0, 0.55, if *kind == "frozen_pond" { 0.0 } else { 0.7 }, 0.0),
                        deep: *deep,
                        shallow: *shallow,
                        edge: *edge,
                    },
                }),
            )
        })
        .collect(),
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
            Cloud { world: off, y },
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
            Cloud { world: off, y: 0.28 },
            CloudShadow,
            Mesh3d(puff.clone()),
            MeshMaterial3d(cloud_shadow_mat.clone()),
            Transform::from_translation(Vec3::new(off.x, 0.28, off.y))
                .with_rotation(flat)
                .with_scale(Vec3::new(sz, sz * 0.72, 1.0)),
        ));
    }
    commands.insert_resource(SkyMats { cloud: cloud_mat });

    // FAR distant mountain skyline: a few BIG rock models way out past the play area,
    // anchored around the camera (see `anchor_backdrop`) for a hint of horizon depth.
    // Pushed WAY out (was a ring at 165-220 that loomed as a wall on every side and read
    // as a corridor); now beyond ~300u it's a faint fogged silhouette on the horizon, not
    // an arena wall. Fewer of them, too, so gaps of open sky show between.
    let backdrop: Vec<Handle<WorldAsset>> = ["cliff_large_rock", "rock_largeA", "cliff_cornerLarge_rock"]
        .into_iter()
        .map(|p| assets.load(GltfAssetLabel::Scene(0).from_asset(format!("models/nature/{p}.glb"))))
        .collect();
    for i in 0..8 {
        let ang = i as f32 / 8.0 * std::f32::consts::TAU + (rnd() - 0.5) * 0.5;
        let rad = 320.0 + rnd() * 140.0;
        let off = Vec2::new(ang.cos() * rad, ang.sin() * rad);
        let size = 14.0 + rnd() * 12.0;
        commands.spawn((
            Backdrop { off },
            // ⚠️ NEVER A SHADOW CASTER. These are horizon SILHOUETTES — rock models scaled
            // 14-26x sitting 320-460 units out — and they are anchored to the CAMERA, so
            // they follow the player. Left casting, eight objects that size threw shade
            // across the whole play area and the shade travelled with you: the world read
            // as permanent overcast in every biome, worst in the mire whose ground is
            // already the second-darkest in the set. Several bugs got misdiagnosed off
            // captures that were really just standing in this.
            //
            // `no_billboard_shadows` does not catch them: that only marks `Billboard`
            // entities, and these are glTF meshes. Anything spawned huge and far out needs
            // this by hand.
            NotShadowCaster,
            WorldAssetRoot(backdrop[i % backdrop.len()].clone()),
            Transform::from_translation(Vec3::new(off.x, -0.5, off.y))
                .with_scale(Vec3::splat(size))
                .with_rotation(Quat::from_rotation_y(rnd() * std::f32::consts::TAU)),
        ));
    }

    // THE SKY DOME: a big inside-out sphere centred on the camera, carrying the gradient +
    // sun. Radius sits inside the far plane but outside everything else the world draws, and
    // `cull_mode: Front` so we see its inside. `NotShadowCaster` because a sky that shadows
    // the world is the same class of bug as the ground shadowing itself.
    let sky_dome_mat = sky_mats.add(SkyDome {
        horizon: Vec4::new(0.62, 0.76, 0.92, 1.0),
        zenith: Vec4::new(0.24, 0.46, 0.82, 1.0),
        sun_dir: Vec4::new(0.0, 1.0, 0.0, 1.0),
        sun_col: Vec4::new(1.0, 0.96, 0.86, 0.6),
    });
    commands.spawn((
        SkyDomeMesh,
        NotShadowCaster,
        Mesh3d(meshes.add(Sphere::new(900.0).mesh().ico(4).unwrap())),
        MeshMaterial3d(sky_dome_mat),
        Transform::default(),
    ));

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
    // Faint, translucent streaks — NOT glowing white slabs. The old drops were bright
    // (high emissive) + numerous (900), so a shower read as sheets of white. Fewer,
    // thinner, dimmer, and barely-emissive → a subtle drizzle you can see through.
    let drop_mesh = meshes.add(Cuboid::new(0.028, 1.1, 0.028));
    let drop_mat = mats.add(StandardMaterial {
        base_color: Color::srgba(0.72, 0.80, 0.94, 0.32),
        emissive: LinearRgba::rgb(0.04, 0.05, 0.07),
        unlit: true,
        alpha_mode: AlphaMode::Blend,
        ..default()
    });
    for _ in 0..460 {
        // Uniform over the disk: sqrt(u) keeps it from clustering at the centre. A super
        // storm scales these offsets up (see `drive_rain`) to soak the whole area.
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

    // ── Snow (tundra) ───────────────────────────────────────────────────────
    // Soft, slow flakes anchored on the player, shown only in the tundra. They reuse the
    // cloud puff texture rather than adding art: at flake scale it is just a soft dot,
    // which is exactly what a snowflake needs to be at this resolution.
    // ⚠️ WHITE SNOW ON WHITE GROUND IS INVISIBLE. The tundra's tile is the brightest in the
    // game (mean luminance 243), so a white flake at 85% alpha simply vanishes into it —
    // which is exactly how the first pass rendered: animating correctly, and unseeable.
    // A flake reads by being BRIGHTER than even that, so it is pushed emissive and lifted
    // into bloom's range rather than tinted darker, which would read as ash.
    let flake_mat = mats.add(StandardMaterial {
        base_color: Color::srgba(1.0, 1.0, 1.0, 0.95),
        base_color_texture: Some(images.add(hd2d::cloud_texture(48))),
        emissive: LinearRgba::rgb(1.9, 2.0, 2.3), // brighter than the snowfield it falls on
        unlit: true,
        alpha_mode: AlphaMode::Blend,
        double_sided: true,
        cull_mode: None,
        ..default()
    });
    let flake_mesh = meshes.add(Rectangle::new(1.0, 1.0));
    for _ in 0..420 {
        let ang = rnd() * std::f32::consts::TAU;
        let r = rnd() * rnd() * SNOW_RADIUS; // biased inward: distant flakes are invisible anyway
        let off = Vec3::new(ang.cos() * r, rnd() * SNOW_FALL_TOP, ang.sin() * r);
        // ⚠️ FLAKES ARE BIGGER THAN THEY SOUND. At 0.16-0.36 units these were physically
        // right and visually nothing: a few pixels of white against the brightest ground in
        // the game. Snow reads at this camera distance by being generous — closer to a
        // drifting mote than a crystal.
        let sz = 0.34 + rnd() * 0.40;
        commands.spawn((
            Snowflake { off, phase: rnd() * std::f32::consts::TAU },
            Mesh3d(flake_mesh.clone()),
            MeshMaterial3d(flake_mat.clone()),
            Transform::from_translation(off).with_scale(Vec3::splat(sz)),
            hd2d::Billboard,
            NotShadowCaster,
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
    let detail_scenes: Vec<(Handle<WorldAsset>, f32)> = [
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
                WorldAssetRoot(placeholder.clone()),
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
        base_color: Color::srgba(1.0, 0.95, 0.8, 0.6),
        base_color_texture: Some(mote_tex),
        emissive: LinearRgba::rgb(1.2, 1.0, 0.5), // a warm glow that reads clearly as a firefly
        unlit: true,
        alpha_mode: AlphaMode::Blend,
        double_sided: true,
        cull_mode: None,
        ..default()
    });
    // Fireflies pinned to fixed world spots, scattered over a disk around the origin;
    // `drift_motes` shimmers them in place and re-scatters any that fall far behind.
    // Every 4th also carries a soft warm point light (kept few + short-range so the
    // light clusters don't overflow → no flicker), so they cast a gentle glow.
    for i in 0..88 {
        let ang = rnd() * std::f32::consts::TAU;
        let r = rnd().sqrt() * 52.0;
        let pos = Vec2::new(ang.cos() * r, ang.sin() * r);
        let mut ent = commands.spawn((
            Mote {
                pos,
                base_y: 0.6 + rnd() * 3.4,
                phase: rnd() * std::f32::consts::TAU,
                amp: 0.25 + rnd() * 0.5,
                speed: 0.2 + rnd() * 0.5,
                seed: (i as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ 0xF17E,
            },
            Mesh3d(mote_mesh.clone()),
            MeshMaterial3d(mote_mat.clone()),
            Transform::from_translation(Vec3::new(pos.x, 1.0, pos.y))
                .with_scale(Vec3::splat(0.5 + rnd() * 1.1)),
            hd2d::Billboard,
        ));
        if i % 4 == 0 {
            ent.insert(PointLight {
                color: Color::srgb(1.0, 0.85, 0.5),
                intensity: 14_000.0,
                range: 4.5,
                radius: 0.1,
                shadow_maps_enabled: false,
                ..default()
            });
        }
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
    scenes: Vec<(Handle<WorldAsset>, f32)>,
}

/// One recyclable cosmetic ground-detail prop. `slot` is its fixed offset (in cells)
/// from the player's current cell; `last` is the world cell it currently shows, so a
/// prop only re-derives (and swaps scene) when it actually moves to a new cell.
#[derive(Component)]
pub(crate) struct GroundDetail {
    slot: IVec2,
    last: IVec2,
}

impl GroundDetail {
    /// A prop with no cell assigned — enough for tests that only care that the pool
    /// can be hidden.
    #[cfg(test)]
    pub(crate) fn for_test() -> Self {
        Self { slot: IVec2::ZERO, last: IVec2::ZERO }
    }
}

/// A firefly: a soft glowing dot pinned to a FIXED world spot (`pos`) that shimmers in
/// place — you walk past it, it does not follow. When it falls far behind the player it
/// re-scatters to a fresh random spot around them (via `seed`), keeping a lively
/// density nearby without any mote trailing you. Some fireflies also emit a soft light.
#[derive(Component)]
pub(crate) struct Mote {
    pos: Vec2, // fixed world xz until recycled
    base_y: f32,
    phase: f32,
    amp: f32,
    speed: f32,
    seed: u64,
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
    // ⚠️ THESE WERE A THIRD OF WHAT YOU CAN SEE. `animate_sway` multiplies them by
    // `0.06 + wind * 2.4`, which is 0.42 in fair weather — so a tree at 0.05 leaned ONE
    // POINT TWO DEGREES, and 4.6 even in a full storm. Giving Fair a real breeze fixed the
    // wind and changed nothing on screen, because the amplitude it drives was never large
    // enough to read. Two bugs stacked: a wind that never blew, and a lean nobody could see
    // if it had.
    //
    // Sized so fair weather is a visible stir (~4 degrees on a tree) and a storm is a real
    // toss (~15). Stylised rather than physical — this is a diorama seen from a distance,
    // and a botanically-correct two-degree sway is indistinguishable from a still frame.
    match kind {
        "tree" => Some(0.17),
        "fungal_wall" => Some(0.15),
        // A cactus is a water tank on a stalk. It moves, barely, and that contrast is worth
        // keeping — a desert where nothing stirs reads as a painting.
        "cactus" => Some(0.05),
        _ => None,
    }
}

/// Lean every [`Sway`] prop on the wind — top-heavy (pivots at the grounded base),
/// phase-offset per prop, and gustier while it's raining. Overworld only.
pub(crate) fn animate_sway(time: Res<Time>, sky: Option<Res<Sky>>, mut q: Query<(&Sway, &mut Transform)>) {
    let t = time.elapsed_secs();
    // Trees toss ONLY with the wind: dead calm in fair weather, building as the gust
    // rises before a storm and hardest in the downpour. A hair of idle sway keeps a
    // calm forest from looking frozen.
    let wind = sky.map(|s| s.wind).unwrap_or(0.0);
    let gust = 0.06 + wind * 2.4;
    for (s, mut tf) in &mut q {
        // Faster, choppier motion the harder it blows.
        let a = (t * s.speed * (1.0 + wind) + s.phase).sin() * s.amp * gust;
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

/// A drifting sky cloud. `world` is its ABSOLUTE world xz — wind is the only thing that
/// moves it — and `y` its altitude.
///
/// ⚠️ IT USED TO BE AN OFFSET FROM THE CAMERA, AND THAT WAS TWO BUGS IN ONE FIELD.
/// `drift_clouds` placed each cloud at `cam.xz + off`, which rigidly welds the entire sky
/// to the camera transform:
///
///   * **the clouds follow you.** Walk a hundred units and the same puffs are overhead in
///     the same arrangement, so the sky is wallpaper and the world stops feeling travelled.
///   * **and it wrecks the shadows when you SPIN.** Eleven of these are `CloudShadow`
///     quads — dark discs laid flat ON THE GROUND — so orbiting the camera drags eleven
///     big shadow patches across the terrain. Nothing was wrong with the real shadow map;
///     what was sweeping around was scenery pretending to be shade.
///
/// Anchoring is toroidal now: the cloud lives at a world position, and only which COPY of
/// it you see is chosen relative to the view. And the anchor is [`ground_focus`], not the
/// camera — the camera orbits AROUND that point, so it is the one thing in the rig that
/// does not move when you spin. Every future "keep it around the player" system wants that
/// same anchor for the same reason.
#[derive(Component)]
pub(crate) struct Cloud {
    world: Vec2,
    y: f32,
}

/// Wrap `v` into `[-r, r)` — the toroidal fold that lets a world-fixed object be recycled
/// around a moving viewer without ever sliding relative to the ground.
fn wrap_around(v: f32, r: f32) -> f32 {
    (v + r).rem_euclid(2.0 * r) - r
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
    let Ok(cam) = cam_q.single() else { return };
    let focus = ground_focus(cam);
    const R: f32 = 420.0;
    for (mut c, mut tf) in &mut q {
        // Wind is the ONLY thing that moves a cloud. Everything else here is choosing
        // which copy of a world-fixed cloud is the one in front of you.
        c.world.x += CLOUD_WIND * time.delta_secs();
        let rel = Vec2::new(
            wrap_around(c.world.x - focus.x, R),
            wrap_around(c.world.y - focus.z, R),
        );
        tf.translation = Vec3::new(focus.x + rel.x, c.y, focus.z + rel.y);
    }
}

/// Recycle the cosmetic ground-detail pool onto a grid centred on the player. Each
/// prop maps its fixed `slot` to the world cell `player_cell + slot`; position, type,
/// scale, yaw and visibility all derive deterministically from that cell, so a given
/// spot always looks identical and props never appear to slide or flicker — they only
/// re-derive (at the grid's edge, off-screen) as new cells scroll in.
#[allow(clippy::type_complexity)]
/// Is this world point OPEN SEA, and therefore no place for scenery?
///
/// **Both scatter systems place by world position and neither asked.** So mushrooms, grass
/// tufts and bushes were strewn across the ocean — a whole meadow floating on open water,
/// which is the first thing anyone notices about the coast.
///
/// One predicate, both call sites ([`tile_ground_detail`] and
/// [`crate::ambient::update_ambient_scatter`]), because two copies of "where is the water"
/// is the exact drift that has bitten this repo repeatedly — the wall-collision line that
/// went into one mover and not the other, the maze density written twice, and `is_water_kind`
/// living in three places at once.
///
/// It asks [`meld_proto::coast`], the same shoreline the ground shader paints and the server
/// collides against, so scenery stops exactly where the water starts rather than at some
/// second hand-placed line.
///
/// Only meaningful on the Overworld: Last City is a separate scene in its own coordinates
/// (its `coast` uniform is zeroed for exactly that reason), and a zero arc means corridor
/// mode, which has no sea at all.
/// How far inland Last City's beach ramp reaches — must match the land-side half of the
/// ground shader's `smoothstep(-14.0, 0.0, sea)` blend band.
const CITY_BEACH: f32 = 14.0;

pub(crate) fn on_open_water(frame: &crate::WorldFrame, screen: &Screen, wx: f32, wz: f32) -> bool {
    match screen {
        // The maze: ask the shoreline itself.
        Screen::Overworld => {
            // …the STRAITS and the BAYS too (WG-7). Scenery is scattered client-side
            // without asking the shoreline, and the ocean sits outside the fan where nothing
            // is scattered — so this cull only started mattering once a section could hold
            // open water in the middle of it.
            frame.have && {
                let arc = frame.radial_arc_degrees.to_radians() * 0.5;
                // EVERY kind of water here, salt and fresh: a tree standing in a lake reads
                // as a bug in the lake.
                shore_data().shore(arc).is_ocean(wx, wz)
            }
        }
        // Last City is a SEPARATE SCENE in its own coordinates — its `coast` uniform is
        // deliberately zeroed, so `is_ocean` cannot answer here. Its sea is authored from
        // the same constants instead (`city_scene`): water on both flanks past the shore,
        // and ahead past the tip. Reading the constants rather than repeating the numbers
        // is what keeps this from becoming a third hand-placed shoreline.
        Screen::City => {
            // ⚠️ AND A MARGIN INLAND, WHICH THE OVERWORLD DOES NOT NEED. The city's ground
            // now DIPS into its bay (see `sea_depth_at`), but city scenery is still placed
            // at flat y=0 — `tile_ground_detail` rides the heightmap only where
            // `terrain_amp` is 1. So a bush standing anywhere on the beach ramp would hang
            // in the air over the slope. Culling the ramp as well as the water costs a few
            // units of shoreline planting and avoids floating scenery entirely.
            meld_proto::coast::city_sea_depth(wx, wz) > -CITY_BEACH
        }
        _ => false,
    }
}

pub(crate) fn tile_ground_detail(
    cam_q: Query<&Transform, With<Camera3d>>,
    kit: Option<Res<DetailKit>>,
    state: Res<State<Screen>>,
    frame: Res<crate::WorldFrame>,
    mut q: Query<
        (&mut GroundDetail, &mut Transform, &mut Visibility, &mut WorldAssetRoot),
        Without<Camera3d>,
    >,
) {
    let (Ok(cam), Some(kit)) = (cam_q.single(), kit) else { return };
    let focus = ground_focus(cam);
    // Height comes from `terrain_height`, which applies the `terrain_amp` flatten AND the
    // sea dip itself — a prop on a beach has to ride the ramp down, and a prop on the
    // City's flat plaza has to stay level. Multiplying by an amp out here (which this used
    // to do) cannot express both.
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
        let (wx, wz) = ((cell.x as f32 + jx) * DETAIL_CELL, (cell.y as f32 + jz) * DETAIL_CELL);
        // Nothing grows on the sea.
        if on_open_water(&frame, state.get(), wx, wz) {
            *vis = Visibility::Hidden;
            continue;
        }
        tf.translation = Vec3::new(wx, terrain_height(wx, wz), wz);
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
    // SNAP the plane's translation to the vertex lattice. The hills + ground texture are
    // world-locked (the shader displaces + samples by world xz), but the tessellation
    // vertices sit at `local + translation`; sliding `translation` continuously drags
    // that finite sample grid across the fixed heightfield, so the polygonal hills — and
    // especially the ~2u-thick cliff faces — shimmer and pop as vertices cross them.
    // Snapping to a whole number of `GROUND_CELL`s pins the lattice to the same world
    // positions every frame, so the surface holds still while the player glides over it.
    for mut tf in &mut ground_q {
        tf.translation.x = (focus.x / GROUND_CELL).round() * GROUND_CELL;
        tf.translation.z = (focus.z / GROUND_CELL).round() * GROUND_CELL;
    }
}

/// Continuous overworld terrain height at world `(x, z)` — smooth rolling hills, the
/// DQ3/FF natural-elevation base. A sum of low-frequency sines so it's cheap and,
/// crucially, TRIVIAL to mirror EXACTLY in `ground_biome.wgsl` (the ground shader
/// displaces its vertices by this; Rust places every entity/camera on it). Keep the two
/// in lock-step: if you change a coefficient here, change it in the shader.
/// This run's terrain offset (from `run.started.terrain_offset`), held globally so the
/// free-function `terrain_height` — called from every entity/camera placement — applies
/// it without threading a resource through all of them. Two `f32` bit-patterns.
static TERRAIN_OFF_X: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
static TERRAIN_OFF_Z: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

/// Store this run's terrain offset (call on `run.started`); the ground shader reads the
/// same value via [`terrain_offset`] so its displaced ground matches every entity's Y.
pub(crate) fn set_terrain_offset(ox: f32, oz: f32) {
    use std::sync::atomic::Ordering::Relaxed;
    TERRAIN_OFF_X.store(ox.to_bits(), Relaxed);
    TERRAIN_OFF_Z.store(oz.to_bits(), Relaxed);
}
/// The current run's terrain offset.
pub(crate) fn terrain_offset() -> (f32, f32) {
    use std::sync::atomic::Ordering::Relaxed;
    (f32::from_bits(TERRAIN_OFF_X.load(Relaxed)), f32::from_bits(TERRAIN_OFF_Z.load(Relaxed)))
}

/// This run's authored CLIMBABLE peaks (mountains), world-space `[cx, cz, radius,
/// height]`, summed onto the ground so each renders + you climb it. Set on `run.started`
/// and appended per streamed section. A `RwLock` read is cheap + uncontended (only the
/// main thread touches it), and `terrain_height` is called on the render thread.
static PEAKS: std::sync::RwLock<Vec<[f32; 4]>> = std::sync::RwLock::new(Vec::new());

/// The peaks that came with `run.started` — the initial chain's, which no section message
/// ever re-sends. Held apart from the streamed ones so rebuilding the set after a retile
/// does not drop them.
static BASE_PEAKS: std::sync::RwLock<Vec<[f32; 4]>> = std::sync::RwLock::new(Vec::new());
/// Streamed sections' peaks, keyed by section index so a re-sent section REPLACES its own
/// contribution instead of adding a second mountain beside the first.
static SECTION_PEAKS: std::sync::RwLock<std::collections::BTreeMap<u32, Vec<[f32; 4]>>> =
    std::sync::RwLock::new(std::collections::BTreeMap::new());

/// Replace this run's peak set (call on `run.started`).
pub(crate) fn set_peaks(peaks: Vec<[f32; 4]>) {
    if let Ok(mut by_section) = SECTION_PEAKS.write() {
        by_section.clear();
    }
    if let Ok(mut b) = BASE_PEAKS.write() {
        *b = peaks.clone();
    }
    if let Ok(mut p) = PEAKS.write() {
        *p = peaks;
    }
}
/// Set one SECTION's peaks (call on `world.terrain_section`), replacing whatever that
/// section contributed before.
///
/// Keyed by section rather than appended, because a section is now re-sent — that is how
/// a Shift retiles the ground — and appending would have grown a second mountain on top
/// of the first every time one landed. `set_peaks` (a fresh run) clears the keying with
/// it, since `run.started` carries the whole initial chain in one go.
pub(crate) fn set_section_peaks(index: u32, peaks: &[[f32; 4]]) {
    let Ok(mut by_section) = SECTION_PEAKS.write() else { return };
    if peaks.is_empty() && !by_section.contains_key(&index) {
        return;
    }
    by_section.insert(index, peaks.to_vec());
    let streamed: Vec<[f32; 4]> = by_section.values().flatten().copied().collect();
    if let (Ok(mut p), Ok(base)) = (PEAKS.write(), BASE_PEAKS.read()) {
        *p = base.iter().copied().chain(streamed).collect();
    }
}
/// A snapshot of the current peaks (for the ground shader uniform).
pub(crate) fn peaks_snapshot() -> Vec<[f32; 4]> {
    PEAKS.read().map(|p| p.clone()).unwrap_or_default()
}

/// **CONTINENTS (WG-7): this world's STRAITS** — the inland seas that separate one landmass
/// from the next ([`meld_proto::coast::Strait`]). Kept exactly as `PEAKS` is, including the
/// base/per-section split, and for the same reason: a section is re-sent when a Shift
/// retiles the ground, so a section's contribution has to be REPLACEABLE rather than
/// appended.
///
/// ⚠️ The coastline is the one thing a Shift does not re-cut — a continent does not wander —
/// so the server re-sends each retiled section's straits UNCHANGED. If it ever stops, this
/// store drops a sea the server is still colliding against, and the ring redraws as walkable
/// ground over open water.
static STRAITS: std::sync::RwLock<Vec<meld_proto::coast::Strait>> =
    std::sync::RwLock::new(Vec::new());
static BASE_STRAITS: std::sync::RwLock<Vec<meld_proto::coast::Strait>> =
    std::sync::RwLock::new(Vec::new());
static SECTION_STRAITS: std::sync::RwLock<
    std::collections::BTreeMap<u32, Vec<meld_proto::coast::Strait>>,
> = std::sync::RwLock::new(std::collections::BTreeMap::new());

/// Replace this run's straits (call on `run.started`).
pub(crate) fn set_straits(straits: Vec<meld_proto::coast::Strait>) {
    if let Ok(mut by_section) = SECTION_STRAITS.write() {
        by_section.clear();
    }
    if let Ok(mut b) = BASE_STRAITS.write() {
        *b = straits.clone();
    }
    if let Ok(mut s) = STRAITS.write() {
        *s = straits;
    }
}

/// Set one SECTION's straits (call on `world.terrain_section`), replacing whatever that
/// section contributed before.
pub(crate) fn set_section_straits(index: u32, straits: &[meld_proto::coast::Strait]) {
    let Ok(mut by_section) = SECTION_STRAITS.write() else { return };
    if straits.is_empty() && !by_section.contains_key(&index) {
        return;
    }
    by_section.insert(index, straits.to_vec());
    let streamed: Vec<meld_proto::coast::Strait> = by_section.values().flatten().copied().collect();
    if let (Ok(mut s), Ok(base)) = (STRAITS.write(), BASE_STRAITS.read()) {
        *s = base.iter().copied().chain(streamed).collect();
    }
}

/// A snapshot of the current straits — for the ground shader uniform, for
/// [`terrain_height`], and for the prop cull that keeps scenery out of the water.
pub(crate) fn straits_snapshot() -> Vec<meld_proto::coast::Strait> {
    STRAITS.read().map(|s| s.clone()).unwrap_or_default()
}

/// The coast's own shape: **bays** bitten into the fan's rim and **isles** standing off it
/// ([`meld_proto::coast::Lobe`] — one list for both, since they are one primitive differing
/// only in which side of the waterline the disc adds to). Kept exactly as `STRAITS` is,
/// base/per-section split included, so a retile replaces a section's own.
static LOBES: std::sync::RwLock<Vec<meld_proto::coast::Lobe>> =
    std::sync::RwLock::new(Vec::new());
static BASE_LOBES: std::sync::RwLock<Vec<meld_proto::coast::Lobe>> =
    std::sync::RwLock::new(Vec::new());
static SECTION_LOBES: std::sync::RwLock<
    std::collections::BTreeMap<u32, Vec<meld_proto::coast::Lobe>>,
> = std::sync::RwLock::new(std::collections::BTreeMap::new());

/// Replace this run's lobes (call on `run.started`).
pub(crate) fn set_lobes(lobes: Vec<meld_proto::coast::Lobe>) {
    if let Ok(mut by_section) = SECTION_LOBES.write() {
        by_section.clear();
    }
    if let Ok(mut b) = BASE_LOBES.write() {
        *b = lobes.clone();
    }
    if let Ok(mut l) = LOBES.write() {
        *l = lobes;
    }
}

/// Set one SECTION's lobes (call on `world.terrain_section`).
pub(crate) fn set_section_lobes(index: u32, lobes: &[meld_proto::coast::Lobe]) {
    let Ok(mut by_section) = SECTION_LOBES.write() else { return };
    if lobes.is_empty() && !by_section.contains_key(&index) {
        return;
    }
    by_section.insert(index, lobes.to_vec());
    let streamed: Vec<meld_proto::coast::Lobe> = by_section.values().flatten().copied().collect();
    if let (Ok(mut l), Ok(base)) = (LOBES.write(), BASE_LOBES.read()) {
        *l = base.iter().copied().chain(streamed).collect();
    }
}

/// Inland water: standing bodies and the chains of flowing ones. Same base/per-section
/// split as everything else the coastline is made of.
static BASINS: std::sync::RwLock<Vec<meld_proto::coast::Basin>> =
    std::sync::RwLock::new(Vec::new());
static RIVERS: std::sync::RwLock<Vec<meld_proto::coast::RiverNode>> =
    std::sync::RwLock::new(Vec::new());
static BASE_WATER: std::sync::RwLock<(Vec<meld_proto::coast::Basin>, Vec<meld_proto::coast::RiverNode>)> =
    std::sync::RwLock::new((Vec::new(), Vec::new()));
#[allow(clippy::type_complexity)]
static SECTION_WATER: std::sync::RwLock<
    std::collections::BTreeMap<u32, (Vec<meld_proto::coast::Basin>, Vec<meld_proto::coast::RiverNode>)>,
> = std::sync::RwLock::new(std::collections::BTreeMap::new());

/// Replace this run's inland water (call on `run.started`).
pub(crate) fn set_water(
    basins: Vec<meld_proto::coast::Basin>,
    rivers: Vec<meld_proto::coast::RiverNode>,
) {
    if let Ok(mut by_section) = SECTION_WATER.write() {
        by_section.clear();
    }
    if let Ok(mut b) = BASE_WATER.write() {
        *b = (basins.clone(), rivers.clone());
    }
    if let Ok(mut x) = BASINS.write() {
        *x = basins;
    }
    if let Ok(mut x) = RIVERS.write() {
        *x = rivers;
    }
}

/// Set one SECTION's inland water (call on `world.terrain_section`).
///
/// ⚠️ A river CROSSES section boundaries, so each section carries only the nodes in its own
/// band and the chain is re-assembled from all of them. The per-section map is a `BTreeMap`,
/// so it re-assembles in section order — which is the order the nodes were generated in, and
/// therefore the order the chain runs. Iterating it out of order would connect a river's
/// head to a downstream node and draw a channel across open country.
pub(crate) fn set_section_water(
    index: u32,
    basins: &[meld_proto::coast::Basin],
    rivers: &[meld_proto::coast::RiverNode],
) {
    let Ok(mut by_section) = SECTION_WATER.write() else { return };
    if basins.is_empty() && rivers.is_empty() && !by_section.contains_key(&index) {
        return;
    }
    by_section.insert(index, (basins.to_vec(), rivers.to_vec()));
    let mut sb: Vec<meld_proto::coast::Basin> = Vec::new();
    let mut sr: Vec<meld_proto::coast::RiverNode> = Vec::new();
    for (b, r) in by_section.values() {
        sb.extend_from_slice(b);
        sr.extend_from_slice(r);
    }
    if let Ok(base) = BASE_WATER.read() {
        if let Ok(mut x) = BASINS.write() {
            *x = base.0.iter().copied().chain(sb).collect();
        }
        if let Ok(mut x) = RIVERS.write() {
            *x = base.1.iter().copied().chain(sr).collect();
        }
    }
}

/// **Every piece of this world's shoreline, owned** — so a caller can build a borrowed
/// [`meld_proto::coast::Shore`] from it.
///
/// One bundle rather than a getter per list, because asking for some of the shoreline and
/// not the rest is how a surface ends up drawing half a coast — which the minimap did: it
/// knew the ocean and not the straits, so inland seas were invisible on the one screen that
/// exists to answer "where am I".
pub(crate) struct ShoreData {
    pub(crate) terrain_off: (f32, f32),
    pub(crate) straits: Vec<meld_proto::coast::Strait>,
    pub(crate) lobes: Vec<meld_proto::coast::Lobe>,
    pub(crate) basins: Vec<meld_proto::coast::Basin>,
    pub(crate) rivers: Vec<meld_proto::coast::RiverNode>,
}

impl ShoreData {
    /// Borrow it as a [`meld_proto::coast::Shore`] for the given fan.
    pub(crate) fn shore(&self, arc_half: f32) -> meld_proto::coast::Shore<'_> {
        meld_proto::coast::Shore {
            arc_half,
            terrain_off: self.terrain_off,
            straits: &self.straits,
            lobes: &self.lobes,
            basins: &self.basins,
            rivers: &self.rivers,
        }
    }
}

/// A snapshot of the whole shoreline.
pub(crate) fn shore_data() -> ShoreData {
    ShoreData {
        terrain_off: terrain_offset(),
        straits: STRAITS.read().map(|s| s.clone()).unwrap_or_default(),
        lobes: LOBES.read().map(|l| l.clone()).unwrap_or_default(),
        basins: BASINS.read().map(|b| b.clone()).unwrap_or_default(),
        rivers: RIVERS.read().map(|r| r.clone()).unwrap_or_default(),
    }
}

/// **The seed of the world we are in — its public NAME** (CANON D19: the target overworld
/// is a *player-seeded* World, and §W5 stores this number rather than a map because the
/// baseline is a pure function of it).
///
/// Held here beside the terrain offset and the straits because it is the same kind of
/// thing: a per-world fact the server hands down once and several surfaces read. Never
/// derived from what the client asked for — see `RunStarted::world_seed`.
static WORLD_SEED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Record this world's seed (call on `run.started`).
pub(crate) fn set_world_seed(seed: u64) {
    WORLD_SEED.store(seed, std::sync::atomic::Ordering::Relaxed);
}

/// This world's seed, or `0` before a run has started.
pub(crate) fn world_seed() -> u64 {
    WORLD_SEED.load(std::sync::atomic::Ordering::Relaxed)
}

/// The coast + flatten state the ground shader is currently drawing with, so
/// [`terrain_height`] can answer for the SAME surface. Three atomics beside
/// `TERRAIN_OFF_*`, written by the one system that fills the shader uniform — because the
/// bug this fixes was precisely the placement side and the drawing side disagreeing about
/// where the ground is, and a second source of truth would reintroduce it.
static COAST_ARC: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
static COAST_CITY: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
static GROUND_AMP: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

/// Publish what the ground shader is about to draw. Called from the uniform update, so the
/// two cannot drift.
pub(crate) fn set_ground_coast(arc_half: f32, city: bool, amp: f32) {
    use std::sync::atomic::Ordering::Relaxed;
    COAST_ARC.store(arc_half.to_bits(), Relaxed);
    COAST_CITY.store(u32::from(city), Relaxed);
    GROUND_AMP.store(amp.to_bits(), Relaxed);
}

fn ground_coast() -> (f32, bool, f32) {
    use std::sync::atomic::Ordering::Relaxed;
    (
        f32::from_bits(COAST_ARC.load(Relaxed)),
        COAST_CITY.load(Relaxed) == 1,
        f32::from_bits(GROUND_AMP.load(Relaxed)),
    )
}

/// **The ground surface at `(x, z)`** — what everything in the world stands on.
///
/// ⚠️ IT USED TO RETURN THE LAND HEIGHT AND IGNORE THE SEA, SO EVERYTHING FLOATED. The
/// shader has dipped the ground toward sea level at every coast for a while; this function
/// — which places every prop, tree, building, creature and the player's own feet — knew
/// nothing about water. At a shoreline the ground fell away and the world stayed up where
/// the land used to be. It showed up the instant Last City got a coast, but it was true of
/// every pond, lake and ocean edge in the maze the whole time.
///
/// It also applies `terrain_amp` ITSELF now. Callers used to multiply, which is fine for a
/// pure land height and wrong the moment a sea is involved (a flat scene must still dip
/// into its bay) — so the scaling belongs on the inside, with the rule.
pub(crate) fn terrain_height(x: f32, z: f32) -> f32 {
    let (ox, oz) = terrain_offset();
    let base = meld_proto::terrain::height(x, z, ox, oz);
    let peaks = PEAKS.read();
    let peak = peaks
        .as_ref()
        .map(|p| meld_proto::terrain::peak_height(x, z, p))
        .unwrap_or(0.0);
    let land = base + peak;
    let (arc_half, city, amp) = ground_coast();
    let sea = if city {
        meld_proto::coast::city_sea_depth(x, z)
    } else if arc_half > 0.0 {
        // …including the STRAITS (WG-7). This function places every prop, tree, building,
        // creature and the player's own feet; a strait it did not know about would put the
        // whole world back up where the land used to be, over open water — the same bug this
        // function already shipped once for the ocean.
        {
            // ⚠️ `sea`, NOT `water`. This is the field the ground is DIPPED toward the sea
            // floor over, and sea level is globally zero — an inland basin sits at its own
            // elevation and its hollow is already in the heightmap. Folding inland water in
            // here would excavate every lake a second time, below its own bed.
            shore_data().shore(arc_half).sea(x, z)
        }
    } else {
        // Corridor mode (tests, the tutorial): no fan, so no sea anywhere.
        return amp * land;
    };
    meld_proto::terrain::with_sea(land, sea, amp, -SEA_DEPTH)
}

/// Capitalize the first letter for display ("ashfall" → "Ashfall").
pub(crate) fn title_case(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

/// Bespoke HD-2D prop billboards (PixelLab), one PNG per key under `assets/props/`.
///
/// A const rather than a literal inside `setup`, because the menus reach for the same
/// sprites (`icons`) and a name that only exists at the call site cannot be checked — a
/// menu asking for art nobody loads draws a hole where the icon should be.
pub(crate) const PROP_KEYS: [&str; 39] = [
    "obstacle_tree", "obstacle_tree_pine", "obstacle_tree_birch", "obstacle_tree_dead",
    "obstacle_tree_willow", "obstacle_tree_bushy",
    "obstacle_boulder", "obstacle_pond", "obstacle_dune",
    "obstacle_rock_spire", "obstacle_cactus", "obstacle_cliff", "obstacle_lava",
    "obstacle_cinder_rock", "obstacle_ice_spire", "obstacle_frozen_pond",
    "obstacle_snow_drift", "obstacle_bog_pool", "obstacle_mire_root", "obstacle_fungal_wall",
    "resource_bloom_herb", "resource_heartoak_bark", "resource_sun_salts",
    "resource_dune_iron", "resource_ember_ash", "resource_cinder_ore",
    "resource_frost_lichen", "resource_rime_ore", "resource_bog_myrrh", "resource_peat_iron",
    "connector_ladder", "connector_rope", "connector_ramp",
    "item_chest_common", "item_chest_rare", "item_chest_open", "item_gold_pile", "item_loot_gem",
    "marker_target_marker",
];

/// Biome theme name → ground-texture / ring index (matches `BIOMES` order in
/// meld-world and the texture bindings in `ground_biome.wgsl`).
pub(crate) fn biome_ring_index(name: &str) -> usize {
    match name {
        "desert" => 1,
        "ashfall" => 2,
        "tundra" => 3,
        "mire" => 4,
        // "field" shares the forest's grass: a meadow and a wood stand on the same ground,
        // and the only thing that separates them is how many trees are in the way.
        _ => 0, // field / forest / unknown
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
    state: Res<State<Screen>>,
    tell: Res<crate::ShiftTell>,
    clock: Res<Time>,
    frame: Res<crate::WorldFrame>,
    ground_q: Query<&MeshMaterial3d<GroundMat>, With<WorldGround>>,
    mut mats: ResMut<Assets<GroundMat>>,
) {
    let Ok(handle) = ground_q.single() else { return };
    let Some(mut mat) = mats.get_mut(&handle.0) else { return };
    // THE COASTLINE, from the server's own arc. Zeroed off the Overworld: Last City is a
    // separate scene laid out in its own coordinates, so painting the world's sea into it
    // would put water through the plaza. Giving the city its own shore is follow-up work —
    // the neck and the channel are walked HERE, in the arena.
    mat.extension.params.coast = if *state.get() == Screen::Overworld && frame.have {
        Vec4::new(
            (frame.radial_arc_degrees.to_radians() * 0.5).max(0.0),
            meld_proto::coast::NECK_REACH,
            meld_proto::coast::PENINSULA_LENGTH,
            meld_proto::coast::CHANNEL_LAND_SHARE,
        )
    } else {
        Vec4::ZERO
    };
    // The Shift's tell rides the same uniform as the biome rings because it IS a ring —
    // see `BiomeParams::shift`. Zero intensity is the resting state, so a world with no
    // Shift pending pays one compare per fragment.
    let now = clock.elapsed_secs_f64();
    let k = tell.intensity(now);
    mat.extension.params.shift = Vec4::new(tell.inner, tell.outer, k, 0.0);
    // The sea's clock. Wrapped rather than raw elapsed seconds: f32 loses sub-frame
    // precision in the thousands, and a session left running overnight would see the swell
    // quantise and then stop moving. 3600 is long enough that the wrap never lines up with
    // anything a player can perceive.
    // `yz` is LAST CITY's own spit (shore half-width, tip reach), zero everywhere else —
    // see `sea_depth_at`. The city is a separate scene in its own coordinates, so the
    // world's shoreline cannot be reused there; handing the city's own down this uniform is
    // what lets ONE shader draw both seas, instead of the city keeping three hand-placed
    // water planes that quietly missed every fix the world's sea received.
    let (city_shore, city_tip) = if *state.get() == Screen::City {
        (
            meld_proto::coast::CITY_SHORE_HALF_WIDTH,
            meld_proto::coast::CITY_TIP_REACH,
        )
    } else {
        (0.0, 0.0)
    };
    mat.extension.params.sea_anim =
        Vec4::new((now % 3600.0) as f32, city_shore, city_tip, 0.0);
    // Roll the ground into hills+cliffs ONLY in the Overworld. The City + menus are
    // hand-placed for FLAT ground (a level plaza), so displacing it there tilts every
    // prop and shades the troughs into blue "corridor" ribbons — flatten it (amp 0).
    mat.extension.params.terrain_amp =
        if *state.get() == Screen::Overworld { 1.0 } else { 0.0 };
    // Publish the SAME coast + flatten state to the placement side, right here, so
    // `terrain_height` answers for the surface this uniform is about to draw. Everything
    // in the world floated over every shoreline for as long as these were two answers.
    set_ground_coast(
        mat.extension.params.coast.x,
        *state.get() == Screen::City,
        mat.extension.params.terrain_amp,
    );
    let (ox, oz) = terrain_offset();
    mat.extension.params.terrain_off = Vec2::new(ox, oz);
    // Feed the authored peaks to the shader (windowed to the slot count) so each mountain
    // dome renders on the ground, matching `terrain_height`.
    let peaks = peaks_snapshot();
    let n = peaks.len().min(PEAK_SLOTS);
    for (i, slot) in mat.extension.params.peaks.iter_mut().enumerate() {
        *slot = if i < n {
            Vec4::new(peaks[i][0], peaks[i][1], peaks[i][2], peaks[i][3])
        } else {
            Vec4::ZERO
        };
    }
    mat.extension.params.peak_count = n as u32;
    // The player's own ring — the centre of both windows below (straits and biome rings).
    let pr = world
        .entities
        .get(&session.player_id)
        .map(|e| e.x.hypot(e.y))
        .unwrap_or(0.0);
    // …and the STRAITS (WG-7 continents), two vec4s each. WINDOWED by radius, unlike the
    // peaks above: the world streams outward without bound, so the only straits that can be
    // on screen are the ones near the player's own ring, and a flat truncation would drop
    // the coast you are standing on in favour of one back at the hub.
    let mut near: Vec<meld_proto::coast::Strait> = straits_snapshot();
    near.sort_by(|a, b| (a[0] - pr).abs().total_cmp(&(b[0] - pr).abs()));
    near.truncate(meld_proto::coast::MAX_STRAITS);
    for (i, slot) in mat.extension.params.straits.iter_mut().enumerate() {
        let (k, half) = (i / 2, i % 2);
        *slot = match near.get(k) {
            Some(s) if half == 0 => Vec4::new(s[0], s[1], s[2], s[3]),
            Some(s) => Vec4::new(s[4], s[5], s[6], s[7]),
            None => Vec4::ZERO,
        };
    }
    mat.extension.params.strait_count = near.len() as u32;
    // …and the coast's lobes, windowed the same way and for the same reason.
    let mut near_lobes: Vec<meld_proto::coast::Lobe> =
        LOBES.read().map(|l| l.clone()).unwrap_or_default();
    near_lobes.sort_by(|a, b| {
        let da = (a[0].hypot(a[1]) - pr).abs();
        let db = (b[0].hypot(b[1]) - pr).abs();
        da.total_cmp(&db)
    });
    near_lobes.truncate(LOBE_SLOTS);
    for (i, slot) in mat.extension.params.lobes.iter_mut().enumerate() {
        *slot = match near_lobes.get(i) {
            Some(l) => Vec4::new(l[0], l[1], l[2], l[3]),
            None => Vec4::ZERO,
        };
    }
    mat.extension.params.lobe_count = near_lobes.len() as u32;
    // …and inland water. Basins window by radius like the rest. River nodes do NOT get
    // sorted: a chain's order IS the channel, so re-ordering them would connect a river's
    // head to a downstream node and draw water across open country. They are taken as a
    // contiguous run around the player's own ring instead.
    let water = shore_data();
    let mut near_basins = water.basins.clone();
    near_basins.sort_by(|a, b| {
        let da = (a[0].hypot(a[1]) - pr).abs();
        let db = (b[0].hypot(b[1]) - pr).abs();
        da.total_cmp(&db)
    });
    near_basins.truncate(BASIN_SLOTS);
    for (i, slot) in mat.extension.params.basins.iter_mut().enumerate() {
        *slot = match near_basins.get(i) {
            Some(b) => Vec4::new(b[0], b[1], b[2], b[3]),
            None => Vec4::ZERO,
        };
    }
    mat.extension.params.basin_count = near_basins.len() as u32;

    let nodes = &water.rivers;
    let start = if nodes.len() <= RIVER_SLOTS {
        0
    } else {
        // Centre the window on the node nearest the player, then pull it back inside.
        let mid = nodes
            .iter()
            .enumerate()
            .min_by(|a, b| {
                let da = (a.1[0].hypot(a.1[1]) - pr).abs();
                let db = (b.1[0].hypot(b.1[1]) - pr).abs();
                da.total_cmp(&db)
            })
            .map(|(i, _)| i)
            .unwrap_or(0);
        mid.saturating_sub(RIVER_SLOTS / 2).min(nodes.len() - RIVER_SLOTS)
    };
    let window = &nodes[start..(start + RIVER_SLOTS).min(nodes.len())];
    for (i, slot) in mat.extension.params.rivers.iter_mut().enumerate() {
        *slot = match window.get(i) {
            // The first node of a window always starts a chain: the segment joining it to
            // whatever fell outside the window does not exist as far as this frame knows,
            // and drawing it would invent a channel across whatever lies between.
            Some(n) if i == 0 => Vec4::new(n[0], n[1], n[2], 1.0),
            Some(n) => Vec4::new(n[0], n[1], n[2], n[3]),
            None => Vec4::ZERO,
        };
    }
    mat.extension.params.river_count = window.len() as u32;
    // (outer_radius, biome_index) per section, sorted by radius (= corridor end_x).
    let mut rings: Vec<(f32, f32)> = terrain
        .sections
        .values()
        .map(|s| (s.end_x as f32, biome_ring_index(&s.biome) as f32))
        .collect();
    rings.sort_by(|a, b| a.0.total_cmp(&b.0));

    // Window the rings around the player's radius (`pr`, above) when there are more than fit.
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

/// Bob the atmosphere motes and keep them anchored around the PLAYER (in the
/// overworld) so the near air around you is always alive as you travel. Anchoring to
/// the player — not the camera's ground-aim point — keeps the fireflies centred on
/// you at any camera pitch/zoom (the aim point drifts up-screen as the camera tilts
/// down, which used to bunch every mote into the mid-distance). Off the overworld
/// (city/battle) there's no player, so fall back to the camera's ground focus.
pub(crate) fn drift_motes(
    time: Res<Time>,
    state: Res<State<Screen>>,
    world: Res<Overworld>,
    session: Res<Session>,
    mut q: Query<(&mut Mote, &mut Transform)>,
) {
    // Fireflies are pinned to the WORLD, not the player — you walk past them. Only when
    // one falls far behind (so you'd never see it again) does it re-scatter to a fresh
    // spot around you, keeping density nearby without any mote following you.
    let player = (*state.get() == Screen::Overworld)
        .then(|| world.entities.get(&session.player_id))
        .flatten()
        .map(|e| Vec2::new(e.x, e.y));
    let t = time.elapsed_secs();
    for (mut m, mut tf) in &mut q {
        if let Some(p) = player {
            if m.pos.distance(p) > 62.0 {
                // splitmix64 step → a fresh angle + radius on a ring around the player.
                let mut z = m.seed.wrapping_add(0x9E37_79B9_7F4A_7C15);
                z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
                z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
                z ^= z >> 31;
                m.seed = z;
                let ang = (z % 62831) as f32 / 10_000.0;
                let rad = 42.0 + ((z >> 20) % 18_000) as f32 / 1_000.0; // 42..60
                m.pos = p + Vec2::new(ang.cos() * rad, ang.sin() * rad);
                m.base_y = 0.6 + ((z >> 40) % 3400) as f32 / 1_000.0;
            }
        }
        // A gentle in-place shimmer around the fixed spot.
        tf.translation.x = m.pos.x + (t * m.speed + m.phase).sin() * m.amp * 0.4;
        tf.translation.z = m.pos.y + (t * m.speed * 0.7 + m.phase).cos() * m.amp * 0.4;
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

/// Time of day (`t`: 0 = midnight, 0.5 = noon) + weather (`0` clear .. `1` rain),
/// which together drive the sun, ambient, sky/fog colour, stars, and rain.
#[derive(Resource)]
pub(crate) struct Sky {
    pub(crate) t: f32,
    /// Rain intensity, 0 clear .. 1 downpour (smoothed toward the phase target).
    pub(crate) weather: f32,
    /// Wind intensity, 0 calm .. 1 gale (smoothed). Drives tree sway and precedes rain.
    pub(crate) wind: f32,
    /// Weather phase: 0 Fair, 1 Gust (windy precursor), 2 Storm (rain), 3 Clearing.
    pub(crate) phase: u8,
    pub(crate) phase_timer: f32,
    /// This storm covers the WHOLE area (rain everywhere, not just under the cloud).
    pub(crate) super_storm: bool,
    /// Counts storms, so the super-storm roll varies each time.
    pub(crate) cycle: u32,
    /// Daylight factor (0 = night, 1 = day), recomputed each frame by [`apply_sky`]
    /// so other systems (e.g. the Explorer avatar lamp) can read the darkness without
    /// duplicating the sun-angle math.
    pub(crate) day: f32,
}
impl Sky {
    /// The sky a session opens with, at the time of day `MELD_WORLD_FEEL="sky_t=…"`
    /// asks for (default mid-morning). One place, so `Default` and the flag can never
    /// disagree about when the world starts.
    pub(crate) fn opening(feel: &crate::feel::WorldFeel) -> Self {
        Sky { t: feel.sky_t, ..default() }
    }
}
impl Default for Sky {
    fn default() -> Self {
        Sky {
            t: 0.36,
            weather: 0.0,
            wind: 0.0,
            phase: 0,
            // The OPENING dry spell, deliberately shorter than `fair_secs`: rain is rare
            // now, and a player who never sees weather in their first session does not
            // know the sky does anything.
            phase_timer: 90.0,
            super_storm: false,
            cycle: 0,
            day: 1.0,
        }
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

/// A snowflake, camera-anchored like the rain. `off` is its position relative to the
/// player; `phase` gives each flake its own sideways wander so they do not fall as a sheet.
///
/// ⚠️ Snow is NOT rain with a different colour, and the difference is the whole effect.
/// Rain falls fast and straight and only during a storm. Snow is slow, it drifts sideways,
/// and in the tundra it falls in FAIR weather too — a cold biome is snowing most of the
/// time, and gating it on the storm phase would leave the ice fields looking like a
/// summer meadow between weather cycles.
#[derive(Component)]
pub(crate) struct Snowflake {
    off: Vec3,
    phase: f32,
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
pub(crate) fn advance_sky(
    time: Res<Time>,
    feel: Res<crate::feel::WorldFeel>,
    mut sky: ResMut<Sky>,
) {
    let dt = time.delta_secs();
    sky.t = (sky.t + dt / feel.day_len.max(1.0)).fract();
    // Weather phase machine: Fair → Gust (wind rises, a storm is coming) → Storm (rain)
    // → Clearing → Fair. Wind LEADS the rain, so the trees start tossing before the
    // downpour arrives. Occasionally the storm is a "super storm" that soaks the whole
    // area instead of just the patch under the cloud.
    sky.phase_timer -= dt;
    if sky.phase_timer <= 0.0 {
        sky.phase = (sky.phase + 1) % 4;
        sky.phase_timer = feel.phase_secs(sky.phase);
        if sky.phase == 2 {
            // Entering a storm: roll for a super storm (~1 in 5). splitmix64 of the
            // cycle so it varies without a global RNG.
            sky.cycle = sky.cycle.wrapping_add(1);
            let mut z = (sky.cycle as u64).wrapping_add(0x9E37_79B9_7F4A_7C15);
            z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
            z ^= z >> 31;
            sky.super_storm = z.is_multiple_of(5);
        } else if sky.phase == 0 {
            sky.super_storm = false;
        }
    }
    // Per-phase wind + rain targets (a super storm blows harder).
    let (wind_target, rain_target) = match sky.phase {
        1 => (0.7, 0.0),
        2 => (if sky.super_storm { 1.0 } else { 0.65 }, 1.0),
        3 => (0.3, 0.0),
        // ⚠️ FAIR IS A BREEZE, NOT DEAD CALM — AND THIS IS WHY NOTHING EVER SWAYED.
        //
        // `Sway` and the grass lean both scale off `sky.wind`, and Fair used to target 0.0.
        // Fair also runs for 600 seconds against a 16-second gust, 22 of storm and 14 of
        // clearing — so the world spent 92% OF ITS WALL-CLOCK perfectly motionless, and the
        // one window where anything moved was under a minute in every eleven. Both the trees
        // and the grass were working correctly and essentially nobody would ever see it.
        //
        // A calm day still moves leaves. 0.15 is a breeze: the canopy stirs, the grass
        // ripples, and the gust before a storm is still four times stronger, so the weather
        // keeps its shape.
        _ => (0.15, 0.0),
    };
    let wr = 0.35 * dt;
    sky.wind += (wind_target - sky.wind).clamp(-wr, wr);
    let rr = 0.25 * dt;
    sky.weather += (rain_target - sky.weather).clamp(-rr, rr);
}

// -------------------------------------------------------------- dungeon scene ---

/// DG-6b: the client-only "inside a dungeon" state, driven by the
/// `world.dungeon_scene` cue. When `active`, the environment is re-skinned as a
/// secluded biome enclosure (a forest wall for a `forest` dungeon, dim themed sky)
/// so no overworld shows through; `dirty` tells [`manage_dungeon_scene`] to
/// (re)build or tear down the decor on a change. Presentation only — the playable
/// floor is still the server's dungeon `Snapshot` walls.
#[derive(Resource, Default)]
pub(crate) struct DungeonSceneRes {
    pub(crate) active: bool,
    pub(crate) theme: String,
    pub(crate) floor: u32,
    pub(crate) width: u32,
    pub(crate) height: u32,
    /// Set by the net pump when a field changes; consumed by the builder.
    pub(crate) dirty: bool,
}

/// Marks the client-only dungeon enclosure props (the tree/rock ring). Kept OFF
/// `WorldEntity`/`WorldWall` so the snapshot reconciler never touches them; torn
/// down explicitly by [`manage_dungeon_scene`] and by the `OnExit(Overworld)` sweep.
#[derive(Component)]
pub(crate) struct DungeonDecor;

/// DG-6b: build or tear down the dungeon enclosure when the scene changes. On
/// descent it rings the play area `[0,width] × [0,height]` with a DEEP, DENSE belt
/// of biome props — for `forest`, a wall of the SAME PixelLab tree sprites the
/// overworld uses, tall and several rows deep so the angled camera can't see past
/// it to the open world — and hides stray overworld terraces; on exit it despawns
/// the belt and restores them. Collision-free: the authoritative blocking line is
/// the server's dungeon perimeter walls (§ WG-1/DG-6b, `docs/behaviors/dungeons.md`).
pub(crate) fn manage_dungeon_scene(
    mut commands: Commands,
    mut scene: ResMut<DungeonSceneRes>,
    wa: Option<Res<WorldAssets>>,
    mut mats: ResMut<Assets<StandardMaterial>>,
    decor: Query<Entity, With<DungeonDecor>>,
    // Overworld framing (terraces + biome-edge cliff/treeline walls) is hidden
    // underground so none of it leaks past the dungeon forest, restored on exit.
    mut overworld_framing: Query<&mut Visibility, Or<(With<TerrainMesh>, With<WorldWall>)>>,
) {
    if !scene.dirty {
        return;
    }
    // Assets not loaded yet — leave `dirty` set and retry on a later run.
    let Some(wa) = wa else { return };
    scene.dirty = false;
    // Tear down any prior enclosure (floor change rebuilds it for the new bounds).
    for e in &decor {
        commands.entity(e).despawn();
    }
    // Overworld terraces/cliffs streamed before descent would poke through the forest
    // — hide them underground, restore them on exit.
    let tvis = if scene.active { Visibility::Hidden } else { Visibility::Inherited };
    for mut v in &mut overworld_framing {
        *v = tvis;
    }
    if !scene.active {
        return;
    }

    let w = scene.width.max(1) as f32;
    let h = scene.height.max(1) as f32;
    let bi = biome_ring_index(&scene.theme);
    // A deep forest BOWL around the play box, not a thin ring: `margin` keeps the
    // walkable floor clear, and the belt runs `depth` units out — deep + tall enough
    // that when you zoom out the frame is forest to the horizon (no overworld shows).
    // Prop HEIGHT ramps with distance from the clearing: a low shrub rim right by the
    // play area (so the ~1.6-tall hero is always visible over it from any camera
    // angle) rising to towering trees far out. Angle-independent — a natural clearing.
    let margin = 1.3_f32;
    let depth = 30.0_f32;
    let step = 1.7_f32;
    let (x0, x1) = (-margin - depth, w + margin + depth);
    let (y0, y1) = (-margin - depth, h + margin + depth);
    let mut i = 0usize;
    let mut fx = x0;
    while fx <= x1 {
        let mut fy = y0;
        while fy <= y1 {
            // Plant only OUTSIDE the play box (+ inner margin) so the room stays clear.
            let inside = fx > -margin && fx < w + margin && fy > -margin && fy < h + margin;
            if !inside {
                let jx = (hash_pick(&format!("dx{i}"), 100) as f32 / 100.0 - 0.5) * 1.0;
                let jy = (hash_pick(&format!("dy{i}"), 100) as f32 / 100.0 - 0.5) * 1.0;
                // Chebyshev-ish distance OUTSIDE the box → 0 at the rim, 1 at the edge.
                let dx = (-fx).max(fx - w).max(0.0);
                let dy = (-fy).max(fy - h).max(0.0);
                let d = dx.hypot(dy);
                let t = ((d - margin) / depth).clamp(0.0, 1.0);
                // Thin the FAR field (its tall canopies overlap, so gaps don't show) to
                // keep the billboard count sane while still filling the frame: full
                // density at the rim ramping to ~45% at the edge.
                let dens = 1.0 - t * 0.62;
                if (hash_pick(&format!("dk{i}"), 100) as f32) < dens * 100.0 {
                    spawn_enclosure_prop(&mut commands, &wa, &mut mats, bi, fx + jx, fy + jy, i, t);
                }
            }
            fy += step;
            i += 1;
        }
        fx += step;
    }
}

/// One dungeon-enclosure prop, tagged [`DungeonDecor`]. `t` is the normalised
/// distance from the clearing rim (0) to the far edge (1); prop height ramps with it
/// — a low shrub rim you can see the hero over, rising to towering forest far out.
/// Forest (biome 0) uses the world's tree sprites; other biomes use tinted boulders.
fn spawn_enclosure_prop(
    commands: &mut Commands,
    wa: &WorldAssets,
    mats: &mut Assets<StandardMaterial>,
    bi: usize,
    x: f32,
    y: f32,
    idx: usize,
    t: f32,
) {
    let id = format!("denc-{idx}");
    // Height ramps rim→far: ~1.6 (waist-high shrub) up to ~9 (canopy), with per-prop
    // jitter so the treeline is layered, not a smooth wall.
    let vf = 0.85 + (hash_pick(&id, 100) as f32 / 100.0) * 0.3;
    let height = ((1.6 + t * 7.4) * vf).clamp(1.4, 9.5);
    if bi == 0 {
        // Near the rim, prefer the bushier sprites (reads as undergrowth); farther out,
        // the full tree pool (a tall canopy).
        const TREES: [&str; 5] = [
            // No `obstacle_tree` — the rune tree is reserved as a deliberate landmark
            // rather than one-in-six of a backdrop wood. See the overworld pool.
            "obstacle_tree_pine", "obstacle_tree_birch",
            "obstacle_tree_dead", "obstacle_tree_willow", "obstacle_tree_bushy",
        ];
        const SHRUBS: [&str; 2] = ["obstacle_tree_bushy", "obstacle_tree_willow"];
        let keys: &[&str] = if t < 0.18 { &SHRUBS } else { &TREES };
        let pool: Vec<Handle<Image>> = keys
            .iter()
            .filter_map(|k| wa.prop_sprites.get(*k).cloned())
            .collect();
        if !pool.is_empty() {
            let tex = pool[hash_pick(&id, pool.len())].clone();
            let mat = mats.add(hd2d::sprite_material(Color::WHITE, tex));
            commands
                .spawn((
                    DungeonDecor,
                    Transform::from_translation(crate::overworld::world_pos(x, y, 0.0)),
                    Visibility::default(),
                ))
                .with_children(|p| {
                    p.spawn((
                        Mesh3d(wa.sprite_quad.clone()),
                        MeshMaterial3d(mat),
                        Transform::from_xyz(0.0, height * 0.5, 0.0)
                            .with_scale(Vec3::splat(height / 2.2)),
                        hd2d::Billboard,
                    ));
                });
            return;
        }
    }
    // Non-forest (or tree sprites missing) → a rugged biome-tinted boulder ridge, also
    // ramping taller with distance.
    let s = (1.4 + t * 3.2) + (hash_pick(&id, 24) as f32) * 0.04;
    let col = match bi {
        1 => Color::srgb(0.74, 0.60, 0.38), // desert sandstone
        2 => Color::srgb(0.30, 0.26, 0.28), // ashfall basalt
        3 => Color::srgb(0.80, 0.85, 0.92), // tundra ice-rock
        4 => Color::srgb(0.28, 0.34, 0.28), // mire mossy stone
        _ => Color::srgb(0.48, 0.48, 0.54),
    };
    let mat = mats.add(StandardMaterial { base_color: col, perceptual_roughness: 1.0, ..default() });
    commands.spawn((
        DungeonDecor,
        Mesh3d(wa.rock_mesh.clone()),
        MeshMaterial3d(mat),
        Transform::from_translation(crate::overworld::world_pos(x, y, 0.24 * s))
            .with_scale(Vec3::splat(s * 0.9)),
    ));
}

pub(crate) fn apply_sky(
    mut sky: ResMut<Sky>,
    skymats: Option<Res<SkyMats>>,
    ashfall: Res<Ashfall>,
    dungeon: Res<DungeonSceneRes>,
    mut clear: ResMut<ClearColor>,
    mut ambient_q: Query<&mut AmbientLight>,
    mut mats: ResMut<Assets<StandardMaterial>>,
    mut sun_q: Query<(&mut Transform, &mut DirectionalLight)>,
    mut fog_q: Query<&mut bevy::pbr::DistanceFog, With<Camera3d>>,
    mut stars: Query<&mut Visibility, With<Star>>,
    mut sky_doms: ResMut<Assets<SkyDome>>,
) {
    use std::f32::consts::TAU;
    let Ok(mut ambient) = ambient_q.single_mut() else { return };
    let sun_h = ((sky.t - 0.25) * TAU).sin(); // +1 at noon, -1 at midnight
    // Slower transition = a longer golden hour at dawn/dusk.
    let day = ((sun_h + 0.14) / 0.36).clamp(0.0, 1.0); // 0 night → 1 day
    let dusk = ((0.30 - sun_h.abs()).max(0.0) / 0.30).powf(1.2); // horizon glow
    let rain = sky.weather;
    // Publish the daylight factor so other systems (Explorer lamp) can read darkness.
    sky.day = day;

    let night_sky = Color::srgb(0.03, 0.05, 0.10);
    let day_sky = Color::srgb(0.50, 0.72, 0.93);
    let dusk_sky = Color::srgb(0.66, 0.42, 0.30);
    let rain_sky = Color::srgb(0.36, 0.40, 0.44);
    let mut sky_col = mix_col(night_sky, day_sky, day);
    sky_col = mix_col(sky_col, dusk_sky, dusk * 0.6);
    sky_col = mix_col(sky_col, rain_sky, rain * 0.7 * (0.35 + day * 0.65));
    // Ashfall haze: a thick, choking volcanic smoke that drops visibility and casts the
    // whole scene volcanic. Layered on top of the day/weather sky by intensity. The smoke
    // is DAYLIGHT-SCALED: by day it's a bright, glowing amber haze (a grim but unmistakably
    // *daytime* sky — never a fake starless night), and only at true night does it go dark
    // (so "sky blue by day, stars by night" holds — ashfall just swaps blue for ember).
    let ash = ashfall.intensity.clamp(0.0, 1.0);
    let ash_smoke = mix_col(
        Color::srgb(0.10, 0.06, 0.07), // night: dark ash
        Color::srgb(0.66, 0.42, 0.33), // day: bright ember haze
        day,
    );
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
        light.illuminance = (day * 21000.0 + (1.0 - day) * 550.0) * (1.0 - rain * 0.55);
    }

    // Moonlit-blue at night (not black), warm-white by day.
    ambient.color = mix_col(Color::srgb(0.34, 0.42, 0.68), Color::srgb(0.6, 0.7, 0.85), day);
    ambient.brightness = (95.0 + day * 165.0) * (1.0 - rain * 0.35);
    // Ashfall dims + warms the ambient — an oppressive, smoke-choked half-light.
    if ash > 0.0 {
        ambient.color = mix_col(ambient.color, Color::srgb(0.9, 0.45, 0.32), ash * 0.6);
        ambient.brightness *= 1.0 - ash * 0.4;
    }

    // THE SKY DOME, from the same numbers the clear colour comes from — one source for
    // "what colour is the sky", so the dome, the fog and the water's reflection cannot
    // drift apart the way three hand-tuned palettes would.
    for (_, m) in sky_doms.iter_mut() {
        // Horizon keeps the sky colour we already compute; the zenith goes deeper. Real
        // skies are darkest overhead, and it is the CONTRAST between the two that makes a
        // gradient read as air rather than as a wash.
        m.horizon = Vec4::new(
            sky_col.to_linear().red,
            sky_col.to_linear().green,
            sky_col.to_linear().blue,
            1.0,
        );
        let deep = mix_col(sky_col, Color::srgb(0.10, 0.22, 0.52), 0.55 * day);
        m.zenith = Vec4::new(
            deep.to_linear().red,
            deep.to_linear().green,
            deep.to_linear().blue,
            1.0,
        );
        // The sun's direction, from the same pitch/yaw the light uses — so the glow in the
        // sky sits where the shadows say it should.
        let pitch = (sun_h.abs() * 66.0).max(12.0).to_radians();
        let yaw = (40.0 + (sky.t - 0.5) * 55.0).to_radians();
        let dir = Vec3::new(
            yaw.sin() * pitch.cos(),
            pitch.sin() * sun_h.signum().max(0.0).max(0.08),
            yaw.cos() * pitch.cos(),
        )
        .normalize();
        m.sun_dir = Vec4::new(dir.x, dir.y, dir.z, day);
        let sc = mix_col(Color::srgb(1.0, 0.62, 0.40), Color::srgb(1.0, 0.97, 0.90), day);
        m.sun_col = Vec4::new(
            sc.to_linear().red,
            sc.to_linear().green,
            sc.to_linear().blue,
            0.35 + dusk * 0.45,
        );
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
        if let Some(mut m) = mats.get_mut(&sm.cloud) {
            let g = (0.14 + day * 0.86) * (1.0 - rain * 0.25);
            m.emissive = LinearRgba::rgb(0.72 * g, 0.75 * g, 0.82 * g);
            m.base_color = Color::srgba(1.0, 1.0, 1.0, (0.72 + day * 0.28) * (1.0 - rain * 0.2));
        }
    }

    // DG-6b: inside a dungeon, override the open-sky look with a dim, theme-tinted
    // enclosure so a forest dungeon reads as a shadowed clearing, not the bright
    // overworld. Layered LAST so it wins over the day/weather/ashfall sky, and it
    // uses FIXED values (no day/night cycle underground — the space is enclosed).
    if dungeon.active {
        // (fog/clear, ambient colour, ambient brightness, sun colour, sun lux)
        let (fogc, ambc, ambb, sunc, sunlux) = match dungeon.theme.as_str() {
            "field" | "forest" => (Color::srgb(0.05, 0.11, 0.07), Color::srgb(0.34, 0.50, 0.36), 165.0, Color::srgb(0.72, 0.86, 0.66), 3400.0),
            "desert" => (Color::srgb(0.12, 0.09, 0.06), Color::srgb(0.56, 0.44, 0.30), 180.0, Color::srgb(0.95, 0.82, 0.58), 3600.0),
            "ashfall" => (Color::srgb(0.10, 0.06, 0.06), Color::srgb(0.50, 0.30, 0.26), 150.0, Color::srgb(0.90, 0.50, 0.40), 3000.0),
            "tundra" => (Color::srgb(0.07, 0.09, 0.12), Color::srgb(0.42, 0.50, 0.62), 175.0, Color::srgb(0.72, 0.82, 0.98), 3400.0),
            "mire" => (Color::srgb(0.05, 0.09, 0.07), Color::srgb(0.32, 0.44, 0.36), 150.0, Color::srgb(0.66, 0.82, 0.62), 3000.0),
            _ => (Color::srgb(0.06, 0.06, 0.08), Color::srgb(0.42, 0.44, 0.52), 160.0, Color::srgb(0.80, 0.82, 0.90), 3300.0),
        };
        clear.0 = fogc;
        if let Ok(mut fog) = fog_q.single_mut() {
            fog.color = fogc;
        }
        ambient.color = ambc;
        ambient.brightness = ambb;
        if let Ok((_, mut light)) = sun_q.single_mut() {
            light.color = sunc;
            light.illuminance = sunlux;
        }
        for mut v in &mut stars {
            *v = Visibility::Hidden;
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
/// Fall, drift and wrap the snow — and show it only where it belongs.
///
/// Snow is anchored on the player and wraps within a disc, the same trick the rain uses: a
/// bounded pool of flakes that never runs out because it recycles. What differs is the
/// MOTION, and that is the whole read: rain falls fast and straight, snow falls slowly and
/// wanders sideways, each flake on its own phase so they never descend as a sheet.
///
/// ⚠️ It falls in FAIR weather too. Gating snow on the storm phase — which is what rain
/// does — would leave the ice fields looking like a summer meadow between weather cycles,
/// and a tundra that is only occasionally cold is not a tundra. Weather scales how HARD it
/// snows, never whether it snows at all.
pub(crate) fn drive_snow(
    cam_q: Query<&Transform, With<Camera3d>>,
    time: Res<Time>,
    sky: Res<Sky>,
    stats: Res<crate::RunStats>,
    state: Res<State<Screen>>,
    mut flakes: Query<(&mut Snowflake, &mut Transform, &mut Visibility), Without<Camera3d>>,
) {
    let cam = cam_q.single().map(|t| t.translation).unwrap_or(Vec3::ZERO);
    // Tundra only, and only out in the world: Last City is a separate scene with its own
    // weather-free framing, and snow over the plaza would be a permanent blizzard indoors.
    // ⚠️ CASE-INSENSITIVE, because `RunStats.biome` is TITLE-CASED for the HUD readout
    // ("336 m · T3 · Tundra") while every biome key in the codebase is lowercase. Comparing
    // against `"tundra"` matches nothing, and the failure is silent: the snow animates
    // perfectly and simply never becomes visible. It cost two wrong hypotheses here — first
    // contrast, then falling through displaced terrain — before a garish diagnostic proved
    // the flakes were not being drawn at all.
    let snowing =
        *state.get() == Screen::Overworld && stats.biome.eq_ignore_ascii_case("tundra");
    let dt = time.delta_secs();
    let t = time.elapsed_secs();
    // A storm drives it harder and slants it further; fair weather is a gentle fall.
    let hard = 0.45 + sky.weather * 0.55;
    let slant = (0.6 + sky.wind * 2.4) * hard;
    for (mut f, mut tf, mut v) in &mut flakes {
        *v = if snowing { Visibility::Inherited } else { Visibility::Hidden };
        if !snowing {
            continue;
        }
        f.off.y -= (2.6 + 3.4 * hard) * dt;
        // Downwind travel, so a storm visibly blows the fall sideways.
        f.off.x += slant * dt;
        if f.off.y < 0.0 {
            f.off.y = SNOW_FALL_TOP;
        }
        // Wrap in x so the downwind drift never empties the upwind side.
        if f.off.x > SNOW_RADIUS {
            f.off.x -= 2.0 * SNOW_RADIUS;
        }
        // The wander: each flake on its own phase, so the fall reads as air moving rather
        // than as a curtain sliding.
        let wob = ((t * 0.8 + f.phase).sin() * 0.55 + (t * 1.9 + f.phase * 1.7).sin() * 0.22)
            * (0.4 + hard);
        // ⚠️ HEIGHT IS RELATIVE TO THE CAMERA, NOT ABSOLUTE. The rain gets away with an
        // absolute `y` because it falls from a cloud at a fixed altitude; snow anchored the
        // same way falls through terrain that `total_height` displaces by up to ±15 units,
        // so on any raised ground the whole snowfall is UNDERGROUND. It animated perfectly
        // and could not be seen — including against a cloud shadow, which is what proved it
        // was not a contrast problem.
        tf.translation = Vec3::new(
            cam.x + f.off.x + wob,
            cam.y + f.off.y - SNOW_FALL_TOP * 0.62,
            cam.z + f.off.z + wob * 0.6,
        );
    }
}

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
    // A super storm soaks the WHOLE area: anchor the drops on the player and spread
    // them wide, instead of the tight patch under the drifting cloud.
    let (anchor, spread) = if sky.super_storm {
        (Vec2::new(cam.x, cam.z), 2.4)
    } else {
        (ground, 1.0)
    };
    for (mut d, mut t, mut v) in &mut rain_q {
        *v = vis;
        if raining {
            d.off.y -= 55.0 * dt; // fall
            if d.off.y < 0.0 {
                d.off.y += RAIN_FALL_TOP; // wrap to the top of the column
            }
            t.translation =
                Vec3::new(anchor.x + d.off.x * spread, d.off.y, anchor.y + d.off.z * spread);
        }
    }
}

/// Drive EVERY water surface's clock: the maze's pools and Last City's sea alike.
///
/// The bed tile still drifts (it is what made a still pond read as water before there were
/// waves at all) and the wave field now advances with it. Wrapped rather than raw elapsed
/// seconds, for the same reason the ocean's clock is: f32 loses sub-frame precision in the
/// thousands, so a session left running overnight would see the swell quantise and stop.
pub(crate) fn animate_water(
    time: Res<Time>,
    mut mats: ResMut<Assets<WaterMat>>,
) {
    let t = time.elapsed_secs() % 3600.0;
    let xf = bevy::math::Affine2::from_scale_angle_translation(
        Vec2::splat(2.2),
        0.0,
        Vec2::new(t * 0.035, t * 0.055),
    );
    // Every water material, not just the ones `WorldAssets` knows about — Last City builds
    // its own sea, and a per-kind list is a list the city gets left off.
    for (_, m) in mats.iter_mut() {
        // Frozen water (steepness 0) neither ripples NOR drifts: scrolling the bed tile under
        // a still surface is the same lie as a wave on ice, just quieter.
        if m.extension.params.z > 0.0 {
            m.base.uv_transform = xf;
            m.extension.params.x = t;
        }
    }
}

/// Tint for a boss's depth band (`boss_band:<n>`; 0 = the sprite's own colours).
/// Deeper bands run hotter and darker — the same named boss in a worse mood —
/// so escalation is visible without four sets of art per boss.
pub(crate) fn boss_band_tint(band: u8) -> Color {
    match band {
        0 => Color::srgb(1.2, 1.15, 1.1),
        1 => Color::srgb(1.25, 1.0, 0.85),
        2 => Color::srgb(1.05, 0.82, 1.2),
        _ => Color::srgb(1.35, 0.72, 0.72),
    }
}

#[cfg(test)]
mod ground_uniform_tests {
    use super::*;
    use bevy::render::render_resource::ShaderType;

    /// The Rust `BiomeParams` and the WGSL one are two hand-written declarations of the
    /// same buffer, and nothing checks them against each other at build time — a
    /// mismatch surfaces as a wgpu validation failure at material-load, i.e. a black
    /// world in the running game. So hold the size here and read the field list out of
    /// the shader source: adding a field to one side and not the other fails a test
    /// instead of a screenshot.
    /// The ground is rasterized TWICE — once to draw it, once to record its depth for the
    /// shadow map — and the two stages live in different FILES because Bevy refuses two
    /// `@vertex` entry points in one module, while moving the field into an imported library
    /// fails at pipeline creation ("Bindings for [32] conflict with other resource": a
    /// material uniform declared inside an imported module collides rather than resolving).
    ///
    /// So the height field is duplicated, and duplication in this repo is only ever
    /// acceptable when it is CHECKED. If the two copies drift, the ground casts a shadow
    /// shaped like a world that is not the one being drawn — which is the exact bug this
    /// stage was added to fix, and it cost 7x the brightness of every biome.
    #[test]
    fn the_two_ground_shaders_share_one_height_field() {
        let main = include_str!("../assets/shaders/ground_biome.wgsl");
        let prepass = include_str!("../assets/shaders/ground_prepass.wgsl");

        /// One top-level item, from its `fn name(` (or `struct name {`) to the closing brace
        /// in column 0, whitespace-normalised so reformatting is not a false alarm.
        fn item(src: &str, head: &str) -> String {
            let a = src.find(head).unwrap_or_else(|| panic!("missing `{head}`"));
            let b = src[a..].find("\n}").unwrap_or_else(|| panic!("`{head}` never closes")) + a + 2;
            src[a..b].split_whitespace().collect::<Vec<_>>().join(" ")
        }

        for head in [
            "struct BiomeParams {",
            "fn terrain_height_wgsl(",
            "fn spit_half_width(",
            "fn sea_depth_at(",
            "fn peak_dome(",
            "fn total_height(",
            "fn terrain_normal(",
        ] {
            assert_eq!(
                item(main, head),
                item(prepass, head),
                "`{head}` differs between the ground's DRAW pass and its SHADOW pass — the \
                 terrain will cast a shadow shaped like a world it does not render"
            );
        }

        // Both must also carry the uniform at the same binding, or one pass reads nothing.
        let binding = "@binding(106) var<uniform> params: BiomeParams;";
        assert!(main.contains(binding) && prepass.contains(binding), "the uniform moved");
    }

    /// `coast::BASIN_SHORE_SLOPE` is written as a literal in both shaders, because a WGSL
    /// module cannot import a Rust const. Same situation as `terrain::BEACH_BLEND`, and the
    /// same answer: read the shader source and hold the two together, so a retune fails a
    /// test instead of quietly giving inland water a different shore than the one the server
    /// collides against.
    /// **Every coast helper the ground shader defines must actually be CALLED.**
    ///
    /// ⚠️ This test exists because the whole inland-water feature shipped invisible.
    /// `inland_depth_at` was written, mirrored into both shaders, carried through the
    /// uniform, filled by the client and fed by the server — and never called from a
    /// fragment. Lakes and rivers existed in the world model and blocked movement, and drew
    /// nothing at all. WGSL does not complain about an unused function, and the mirror test
    /// beside this one compares the two shaders to EACH OTHER, so it was perfectly happy
    /// with both being equally unwired.
    ///
    /// A definition with no call site is the shader equivalent of the `pack:` and `boss_kind`
    /// bugs this repo already carries warnings about: a thing that exists everywhere except
    /// where the player could see it.
    #[test]
    fn every_coast_helper_is_actually_called() {
        let wgsl = include_str!("../assets/shaders/ground_biome.wgsl");
        for f in ["sea_depth_at", "inland_water_at", "strait_depth_at", "spit_half_width"] {
            let defined = wgsl.contains(&format!("fn {f}("));
            let calls = wgsl.matches(&format!("{f}(")).count();
            assert!(defined, "ground_biome.wgsl should define `{f}`");
            assert!(
                calls >= 2,
                "`{f}` is DEFINED in ground_biome.wgsl and never called ({calls} occurrence). \
                 A shader helper with no call site renders nothing, and nothing else in this \
                 suite notices — which is exactly how inland water shipped invisible."
            );
        }
    }

    #[test]
    fn the_basin_shore_slope_matches_the_shader() {
        let want = format!("/ {:?};", meld_proto::coast::BASIN_SHORE_SLOPE);
        let biome = include_str!("../assets/shaders/ground_biome.wgsl");
        assert!(
            biome.contains(&want),
            "ground_biome.wgsl must divide a basin's vertical margin by \
             `coast::BASIN_SHORE_SLOPE` ({want:?}) — a mismatch gives every lake a different \
             shore in the renderer than the server collides with"
        );
        // ⚠️ The PREPASS is exempt, and that is the point rather than an oversight: it is the
        // ground's depth/shadow pass and carries no basin math at all, because inland water
        // must never displace the ground (its hollow is already in the heightmap). The two
        // files agree on the uniform's LAYOUT, not on what each reads from it.
        let prepass = include_str!("../assets/shaders/ground_prepass.wgsl");
        assert!(
            !prepass.contains("fn inland_water_at("),
            "the prepass must not carry inland-water math — displacing a basin excavates \
             every lake below its own bed"
        );
    }

    #[test]
    fn the_ground_uniform_matches_the_shader_that_reads_it() {
        let size = <biome_params::BiomeParams as ShaderType>::min_size().get();
        assert_eq!(size % 16, 0, "a uniform struct must round to 16 bytes, got {size}");

        let wgsl = include_str!("../assets/shaders/ground_biome.wgsl");
        let body = wgsl
            .split_once("struct BiomeParams {")
            .expect("the shader declares BiomeParams")
            .1
            .split_once("\n}")
            .expect("…and closes it")
            .0;
        for field in [
            "rings", "count", "uv_scale", "blend_half", "terrain_amp", "terrain_off",
            "_pad_peaks", "peaks", "peak_count", "straits", "strait_count", "lobes",
            "lobe_count", "basins", "rivers", "basin_count", "river_count", "shift",
            "sea_anim",
        ] {
            assert!(body.contains(&format!("{field}:")), "the shader is missing `{field}`");
        }
        // Declaration ORDER is the layout, so check the two agree on it rather than only
        // on membership — a reordered pair keeps every name and still corrupts the buffer.
        let order: Vec<&str> = body
            .lines()
            .filter_map(|l| l.trim().split_once(':').map(|(n, _)| n.trim()))
            .filter(|n| !n.starts_with("//"))
            .collect();
        assert_eq!(
            order.first().copied(),
            Some("rings"),
            "the shader's first field moved: {order:?}"
        );
        assert_eq!(
            order.last().copied(),
            Some("sea_anim"),
            "the last field is what the 16-byte tail rounds to: {order:?}"
        );

        // ⚠️ AND THE ARRAY LENGTHS, which nothing checked. `min_size()` is computed from
        // the RUST struct alone, so it cannot notice the shader declaring a shorter array —
        // the names would all match, the size assertion would pass, and the uniform would
        // be silently truncated at whatever the shader believes. That was already true of
        // `peaks` before straits existed; it is checked for both now, because an array
        // length is a duplicated number and this repo only tolerates duplication that is
        // held by a test.
        for (field, slots) in [
            ("peaks", PEAK_SLOTS),
            ("straits", STRAIT_SLOTS),
            ("lobes", LOBE_SLOTS),
            ("basins", BASIN_SLOTS),
            ("rivers", RIVER_SLOTS),
        ] {
            let decl = format!("{field}: array<vec4<f32>, {slots}>");
            assert!(
                body.contains(&decl),
                "the shader must declare `{decl}` — Rust reserves {slots} vec4 slots for \
                 `{field}`, and a shorter array there truncates the uniform silently"
            );
        }
    }
}

#[cfg(test)]
mod sky_tests {
    use super::*;

    /// Two literals for one fact — `Sky::default().t` and `WorldFeel::default().sky_t` —
    /// and a session would open at a different hour than the knob's own default claims.
    #[test]
    fn the_default_sky_opens_when_the_knob_says_it_does() {
        let feel = crate::feel::WorldFeel::default();
        assert_eq!(Sky::default().t, feel.sky_t);
        assert_eq!(Sky::opening(&feel).t, feel.sky_t);
    }
}

#[cfg(test)]
mod creature_sprite_tests {
    use super::*;

    /// ART THAT LANDS UNLISTED IS ART NOBODY SEES — and art listed before it is
    /// finished is a wall of missing-asset errors every launch. Both directions are
    /// checked against the filesystem, because a second hand-written list would drift
    /// from the first the way every other pair in this repo has.
    ///
    /// COMPLETENESS is the pivot, and it is per-facing rather than a count. A creature
    /// that ended up with two animation groups both named `walk` exported seven facings
    /// with `south` silently missing, because the two collided on the same folder; a
    /// count-based check called that finished. `load_creature_clips` asks for all eight
    /// by name, so this does too.
    ///
    /// A set that is on disk but INCOMPLETE is work in progress, not a bug — a
    /// generation batch is hours long and lands species by species — so the rule is:
    /// complete sets must be listed, incomplete ones must not be.
    #[test]
    fn every_finished_creature_set_is_loaded_and_no_unfinished_one_is() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/creatures");
        let Ok(entries) = std::fs::read_dir(&dir) else {
            assert!(CREATURE_CHARS.is_empty(), "no assets/creatures dir, but keys are listed");
            return;
        };
        // Exactly what `load_creature_clips` will ask for: eight walk facings by name, a
        // south attack, and the idle rotations.
        let complete = |key: &str| -> bool {
            let d = dir.join(key);
            d.join("rotations/south.png").is_file()
                && d.join("animations/attack/south/frame_000.png").is_file()
                && hd2d::DIRS.iter().all(|f| {
                    d.join("animations/walk").join(f).join("frame_000.png").is_file()
                })
        };
        for e in entries.filter_map(|e| e.ok()).filter(|e| e.path().is_dir()) {
            let key = e.file_name().to_string_lossy().into_owned();
            let listed = CREATURE_CHARS.contains(&key.as_str());
            if complete(&key) {
                assert!(
                    listed,
                    "assets/creatures/{key} is finished but not in CREATURE_CHARS, so it \
                     is never loaded and the species still draws as a static billboard"
                );
            } else {
                assert!(
                    !listed,
                    "CREATURE_CHARS lists {key}, which is not finished - every missing \
                     facing is a batch of asset errors on every launch. Run \
                     client/scripts/sync_creature_chars.py once its art lands."
                );
            }
        }
        for key in CREATURE_CHARS {
            assert!(
                dir.join(key).is_dir(),
                "CREATURE_CHARS lists {key} but assets/creatures/{key} does not exist"
            );
        }
    }

    /// A leader falls back to the ordinary creature's art, never to nothing — art lands
    /// in batches, so a species can have its ordinary form drawn and its leader not.
    #[test]
    fn a_pack_leader_without_its_own_art_borrows_the_ordinary_creature() {
        let installed =
            |k: &str| matches!(k, "thornback_boar" | "bog_serpent" | "bog_serpent_pack_leader");
        // A leader's own art wins when it exists.
        assert_eq!(
            creature_art_key("bog_serpent", true, installed).as_deref(),
            Some("bog_serpent_pack_leader")
        );
        // …and falls back to the ordinary creature when it does not.
        assert_eq!(
            creature_art_key("thornback_boar", true, installed).as_deref(),
            Some("thornback_boar"),
            "a leader fell back to no art at all"
        );
        // An ordinary creature never reaches for the leader's art, which is what keeps a
        // pack's rank and file from all drawing as their own leader.
        assert_eq!(
            creature_art_key("bog_serpent", false, installed).as_deref(),
            Some("bog_serpent")
        );
        // A species with no art stays a static billboard rather than drawing nothing.
        assert_eq!(creature_art_key("sporeling", true, installed), None);
        assert_eq!(creature_art_key("sporeling", false, installed), None);
        // Leader art alone is not enough: the ordinary form is the common case, so a
        // species with only a leader set stays on billboards for everything else.
        let only_leader = |k: &str| k == "glacier_maw_pack_leader";
        assert_eq!(creature_art_key("glacier_maw", false, only_leader), None);
        assert_eq!(
            creature_art_key("glacier_maw", true, only_leader).as_deref(),
            Some("glacier_maw_pack_leader")
        );
    }
}
