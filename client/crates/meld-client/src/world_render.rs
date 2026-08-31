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
/// Two `vec4`s per range (endpoints, then half-width + height), so the shader array is twice
/// [`meld_proto::terrain::MAX_RIDGES`] — the same packing the straits use.
const RIDGE_SLOTS: usize = meld_proto::terrain::MAX_RIDGES * 2;
/// Two `vec4`s per bridge (endpoints, then half-width).
const BRIDGE_SLOTS: usize = meld_proto::coast::MAX_BRIDGES * 2;

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
        BASIN_SLOTS, BRIDGE_SLOTS, LOBE_SLOTS, PEAK_SLOTS, RIDGE_SLOTS, RIVER_SLOTS,
        STRAIT_SLOTS,
    };
    use bevy::prelude::*;
    use bevy::render::render_resource::ShaderType;

    #[derive(Clone, Copy, ShaderType, Debug)]
    pub(crate) struct BiomeParams {
        /// **THE REGION DECOMPOSITION** ([`meld_proto::regions`]):
        /// `(arc_half, ring_step, cell_width, boundary_warp)`. A biome is a property of a
        /// CELL, so the shader derives the cell a fragment stands in rather than reading a
        /// radius band out of a LUT. `ring_step <= 0` is the "no world here" state the menus
        /// and the city render against, which is what the shader tests.
        pub(crate) region: Vec4,
        /// `[biome_gate]` in `BIOMES` order — `gate.xyzw` = field, forest, desert, ashfall
        /// and `gate_hi.xy` = tundra, mire. In the uniform for the same reason the coast
        /// constants are: the shader picks a cell's biome itself, and a shader that has not
        /// been told the gate paints a theme the server does not spawn.
        pub(crate) gate: Vec4,
        pub(crate) gate_hi: Vec4,
        pub(crate) gate_hi2: Vec4,
        /// World units the ground cross-fades across a cell boundary. A distance from the
        /// nearest edge, because a boundary is 2D now rather than a radial band.
        pub(crate) region_blend: f32,
        pub(crate) region_seed: u32,
        /// DEV/QA `MELD_BIOME`: the biome index every cell is forced to, or `-1` in play. In
        /// the uniform because the shader picks a cell's biome itself — without it the ground
        /// paints the decomposition's answer while the server spawns the forced one, which is
        /// how ashfall lava rocks ended up strewn across green ground.
        pub(crate) region_force: i32,
        pub(crate) uv_scale: f32,
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
        /// 1 while the player is inside a DUNGEON, so the ground draws flagstones instead
        /// of the biome's outdoor tile.
        ///
        /// Underground used to be the overworld ground dimmed — a stopgap from when there
        /// was no dungeon floor art, which `docs/asset-pipeline.md` wrote up as if it were
        /// a design ("a desert dungeon already stands on sand with no extra work"). There
        /// is art now, so a built dungeon stands on a floor. It takes one of the three
        /// pads that were already here, so the uniform layout is byte-for-byte unchanged.
        pub(crate) dungeon: u32,
        pub(crate) _pad_pc1: u32,
        pub(crate) _pad_pc2: u32,
        /// **THE RANGES** ([`meld_proto::terrain::Ridge`]) — two `vec4`s each: slot `2k` is
        /// `(x0, z0, x1, z1)` and `2k+1` is `(half_width, height, 0, 0)`.
        ///
        /// In the uniform for the same reason the coast is: a range is a WALL the server
        /// collides against, and a barrier the ground has not been told about is an invisible
        /// one — which is strictly worse than no barrier at all.
        /// **THE BRIDGES** ([`meld_proto::coast::Bridge`]) — two `vec4`s each: slot `2k` is
        /// `(x0, z0, x1, z1)` and `2k+1` is `(half_width, 0, 0, 0)`. The ground both RAISES the
        /// deck here and paints it, because a bridge that is only forced land renders as the
        /// sea not being there.
        pub(crate) bridges: [Vec4; BRIDGE_SLOTS],
        pub(crate) bridge_count: u32,
        pub(crate) _pad_bc0: u32,
        pub(crate) _pad_bc1: u32,
        pub(crate) _pad_bc2: u32,
        pub(crate) ridges: [Vec4; RIDGE_SLOTS],
        pub(crate) ridge_count: u32,
        pub(crate) _pad_rc0: u32,
        pub(crate) _pad_rc1: u32,
        pub(crate) _pad_rc2: u32,
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
                region: Vec4::ZERO,
                gate: Vec4::ZERO,
                gate_hi: Vec4::ZERO,
                gate_hi2: Vec4::ZERO,
                region_blend: 26.0,
                region_seed: 0,
                region_force: -1,
                uv_scale: 1.0 / 3.0,
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
                dungeon: 0,
                _pad_pc1: 0,
                _pad_pc2: 0,
                bridges: [Vec4::ZERO; BRIDGE_SLOTS],
                bridge_count: 0,
                _pad_bc0: 0,
                _pad_bc1: 0,
                _pad_bc2: 0,
                ridges: [Vec4::ZERO; RIDGE_SLOTS],
                ridge_count: 0,
                _pad_rc0: 0,
                _pad_rc1: 0,
                _pad_rc2: 0,
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
    /// SIDE-VIEW rock, one per biome, for the steep parts of the terrain.
    ///
    /// The overworld is a single displaced plane, so a cliff is not a separate mesh — it
    /// is a steep patch of the same ground. And because the ground's uv is the fragment's
    /// world XZ, a near-vertical face used to smear its top-down grass down its whole
    /// length. These are sampled by the vertical projection instead (see the shader's
    /// triplanar blend), so a cliff face is textured along its own axis at its own scale.
    #[texture(110)]
    cliff_forest: Handle<Image>,
    #[texture(111)]
    cliff_desert: Handle<Image>,
    #[texture(112)]
    cliff_ashfall: Handle<Image>,
    #[texture(113)]
    cliff_tundra: Handle<Image>,
    #[texture(114)]
    cliff_mire: Handle<Image>,
    /// The floor underground. One tile, tinted by the theme lighting a dungeon already
    /// applies, rather than five near-identical flagstones.
    #[texture(115)]
    dungeon_floor: Handle<Image>,
    /// The deep world (`meld_proto::regions::BIOMES` 6..), each one a world boss's arena.
    #[texture(116)]
    amber_wood: Handle<Image>,
    #[texture(117)]
    seized_engine: Handle<Image>,
    #[texture(118)]
    nestiphian_cradle: Handle<Image>,
    #[texture(119)]
    hearth_plains: Handle<Image>,
    #[texture(120)]
    seraphic_oubliette: Handle<Image>,
    #[texture(121)]
    cliff_amber_wood: Handle<Image>,
    #[texture(122)]
    cliff_seized_engine: Handle<Image>,
    #[texture(123)]
    cliff_nestiphian_cradle: Handle<Image>,
    #[texture(124)]
    cliff_hearth_plains: Handle<Image>,
    #[texture(125)]
    cliff_seraphic_oubliette: Handle<Image>,
    /// A bridge's DECK — worn flagstone, and its PARAPETS — a rampart wall. Both are ground
    /// textures the tiling work already shipped; a bridge needs no art of its own.
    #[texture(126)]
    bridge_deck: Handle<Image>,
    #[texture(127)]
    bridge_parapet: Handle<Image>,
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
        .chain(meld_proto::bosses::WORLD_BOSSES.iter().map(|(k, _)| *k))
        .chain(meld_proto::bosses::LIEUTENANTS.iter().map(|(k, _, _)| *k))
        .chain(DUNGEON_SPRITES.iter().copied())
}

/// Bespoke dungeon sprites that get an animated set but are NOT named bosses.
///
/// A dungeon can name any `sprite` for its `[boss.B1]`, and that is not the same thing
/// as being one of the FS-4 named bosses: `meld_proto::bosses::display_name` returns
/// `None` for these, so they draw no name plate — deliberately, since a plate reading
/// `Unknown Horror` over a set piece is worse than no plate.
///
/// They still need loading, though, and the two lists are separate for exactly that
/// reason. `twingolem` guarded the Ocean Palace for a long time with no art anywhere,
/// which meant `creature_sprite` HASHED its kind into the fallback pool and it drew as
/// a random 32px billboard — a boss rendering as a bat, and nothing anywhere saying so.
pub(crate) const DUNGEON_SPRITES: &[&str] = &["twingolem"];

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
/// Each entry carries its walk's FRAME COUNT, because that is a property of the art and
/// not a constant: the stock `walking` template yields six frames and a custom v3 clip
/// eight, and demanding one number made perfectly good sets read as unfinished forever.
///
/// Held against what is actually on disk by `every_finished_creature_set_is_loaded_and_no_unfinished_one_is`,
/// so art that lands unlisted — art nobody would ever see — fails rather than sitting unused.
pub(crate) const CREATURE_CHARS: &[(&str, usize)] = &[
    ("amber_stag", 8),
    ("amber_stag_pack_leader", 8),
    ("arc_phantom", 8),
    ("arc_phantom_pack_leader", 8),
    ("bloat_carrier", 8),
    ("bloat_carrier_pack_leader", 8),
    ("bog_ooze", 8),
    ("bog_ooze_baby", 8),
    ("bog_ooze_belcher", 8),
    ("bog_ooze_grump", 8),
    ("bog_ooze_pack_leader", 8),
    ("bog_serpent", 8),
    ("bog_serpent_female", 8),
    ("bog_serpent_pack_leader", 8),
    ("bog_serpent_slither", 8),
    ("bog_serpent_twin_tail", 8),
    ("bog_stinger", 8),
    ("bog_stinger_buzz", 8),
    ("bog_stinger_licker", 8),
    ("bog_stinger_pack_leader", 8),
    ("bog_stinger_piercer", 8),
    ("bog_stinger_wasp", 8),
    ("briarling", 6),
    ("briarling_pack_leader", 6),
    ("briarling_piper", 6),
    ("briarling_thistleback", 6),
    ("cinder_imp", 8),
    ("cinder_imp_dog", 8),
    ("cinder_imp_fire_mage", 8),
    ("cinder_imp_pack_leader", 8),
    ("cinder_imp_wolf", 8),
    ("cog_sentry", 8),
    ("cog_sentry_pack_leader", 8),
    ("dune_colossus", 8),
    ("dune_colossus_pack_leader", 8),
    ("dune_colossus_shardling", 8),
    ("dune_colossus_sunmarked", 8),
    ("dune_wyrm", 8),
    ("dune_wyrm_glassback", 8),
    ("dune_wyrm_hatchling", 8),
    ("dune_wyrm_pack_leader", 8),
    ("ember_wisp", 8),
    ("ember_wisp_cinderveil", 8),
    ("ember_wisp_mote", 6),
    ("ember_wisp_pack_leader", 8),
    ("forest_bloom_stalker", 8),
    ("forest_bloom_stalker_adult", 8),
    ("forest_bloom_stalker_baby", 8),
    ("forest_bloom_stalker_pack_leader", 8),
    ("frog_tribesman", 8),
    ("frog_tribesman_elder", 8),
    ("frog_tribesman_pack_leader", 8),
    ("frog_tribesman_spearfisher", 8),
    ("frost_lurker", 8),
    ("frost_lurker_pack_leader", 8),
    ("frost_lurker_pup", 8),
    ("frost_lurker_rimefang", 8),
    ("gilded_hound", 8),
    ("gilded_hound_pack_leader", 8),
    ("glacier_maw", 8),
    ("glacier_maw_cub", 8),
    ("glacier_maw_frostjaw", 8),
    ("glacier_maw_pack_leader", 8),
    ("ice_revenant", 8),
    ("ice_revenant_pack_leader", 8),
    ("ice_revenant_shieldbound", 8),
    ("ice_revenant_thrall", 8),
    ("leaf_rook", 8),
    ("leaf_rook_pack_leader", 8),
    ("magma_golem", 8),
    ("magma_golem_cinderling", 6),
    ("magma_golem_pack_leader", 8),
    ("magma_golem_slagfist", 6),
    ("myconid_brute_boss", 8),
    ("myconid_brute_pack_leader", 8),
    ("myconid_mage", 8),
    ("myconid_minion", 8),
    ("myconid_warrior", 6),
    ("rail_hound", 8),
    ("rail_hound_pack_leader", 8),
    ("rot_grub", 8),
    ("rot_grub_pack_leader", 8),
    ("sand_shade", 8),
    ("sand_shade_gravebound", 8),
    ("sand_shade_pack_leader", 8),
    ("sand_shade_wisp", 8),
    ("spore_midwife", 8),
    ("spore_midwife_pack_leader", 8),
    ("sporeling", 8),
    ("sporeling_baby", 8),
    ("sporeling_healer", 8),
    ("sporeling_pack_leader", 8),
    ("sporeling_sprout", 8),
    ("thorn_paramour", 8),
    ("thorn_paramour_pack_leader", 8),
    ("thornback_boar", 8),
    ("thornback_boar_beta", 8),
    ("thornback_boar_charger", 8),
    ("thornback_boar_goarer", 8),
    ("thornback_boar_pack_leader", 8),
    ("tinder_wolf", 8),
    ("tinder_wolf_pack_leader", 8),
    ("twingolem", 8),
    ("velvet_lure", 8),
    ("velvet_lure_pack_leader", 8),
    ("verdant_ooze", 8),
    ("verdant_ooze_blob", 8),
    ("verdant_ooze_blopper", 8),
    ("verdant_ooze_healer", 8),
    ("verdant_ooze_pack_leader", 8),
];

/// The Last City's townsfolk (`assets/npcs/<key>/`). Loaded exactly like a creature —
/// eight walk facings and, for the armed ones, a south attack — and looked up by key
/// rather than pooled, because an NPC is one person rather than one of a species.
///
/// They replace three Kenney GRAVEYARD models (a keeper, a ghost and a skeleton) that
/// stood in the friendly hub as "a hint of the crowd to come".
pub(crate) const NPC_CHARS: &[&str] = &[
    "npc_alembic_keeper",
    "npc_apothecary_keeper",
    "npc_beggar",
    "npc_bounty_clerk",
    "npc_broker",
    "npc_cartographer",
    "npc_child",
    "npc_innkeeper",
    "npc_master_smith",
    "npc_phoenix_guard_sentry",
    "npc_quartermaster",
    "npc_soldier_bow",
    "npc_soldier_captain",
    "npc_soldier_spear",
    "npc_soldier_sword",
    "npc_townsfolk_dwarf",
    "npc_townsfolk_elf",
    "npc_townsfolk_gnome",
    "npc_townsfolk_halfling",
    "npc_townsfolk_harefolk",
    "npc_townsfolk_hobgoblin",
    "npc_townsfolk_human",
    "npc_vault_clerk",
];

/// Which installed set a creature draws from, out of its species' whole POOL.
///
/// A species is a set of variants sharing a name prefix — `myconid_brute`,
/// `myconid_mage`, `myconid_warrior`, `myconid_pack_leader` — and NONE of them need be
/// named exactly after the species. That is not a detail: renaming the species key from
/// `myconid_brute` to `myconid` (because the brute is one myconid among several, not the
/// species) instantly left the species with no art at all under a lookup that only knew
/// `<kind>` and `<kind>_pack_leader`.
///
/// So: take the pool, prefer the half that matches what this spawn IS — a pack leader
/// draws from `_pack_leader` variants, everything else from the rest — and fall back to
/// the other half rather than to nothing, because a leader rendering as an ordinary one
/// of its kind is far better than a leader rendering as nothing.
///
/// The exact species name wins inside its half when it exists, so a species that does
/// have a canonical ordinary form keeps using it.
pub(crate) fn creature_art_key(kind: &str, leader: bool, installed: &[&str]) -> Option<String> {
    let mine = |k: &str| k == kind || k.strip_prefix(kind).is_some_and(|r| r.starts_with('_'));
    let is_leader = |k: &str| k.ends_with("_pack_leader");
    let (want, rest): (Vec<&str>, Vec<&str>) = installed
        .iter()
        .copied()
        .filter(|k| mine(k))
        .partition(|k| is_leader(k) == leader);
    let pick = |v: &[&str]| -> Option<String> {
        if v.contains(&kind) {
            return Some(kind.to_string());
        }
        // Sorted rather than first-seen: the list's order is whatever the sync wrote, and
        // a creature must not change appearance because a file landed in a new order.
        v.iter().min().map(|k| k.to_string())
    };
    pick(&want).or_else(|| pick(&rest))
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
    /// Townsfolk sprite sets (`assets/npcs/<key>/`), keyed by [`NPC_CHARS`] entry.
    pub(crate) npc_chars: HashMap<String, CharacterFrames>,
    pub(crate) monster_pool: Vec<Handle<Image>>,
    /// Real 3D prop models (Kenney Nature Kit, CC0) keyed by terrain-obstacle kind →
    /// several `(scene, baked_scale)` variants (picked per-entity by id hash), so the
    /// world is built from actual geometry instead of flat billboards.
    pub(crate) prop_scenes: HashMap<String, Vec<(Handle<WorldAsset>, f32)>>,
    /// 3D harvest-node models keyed by resource content id → `(scene, baked_scale)`.
    pub(crate) resource_scenes: HashMap<String, (Handle<WorldAsset>, f32)>,
    /// **What a player-built structure is MADE OF, as kit pieces.** Keyed by structure
    /// function; each entry is `(scene, local offset, yaw°, scale)`, composed the way
    /// `CITY_PROPS` composes a crypt out of a body and a roof.
    ///
    /// ⚠️ Before this, a wall and an anchor both drew as a tinted copy of
    /// `fx/portal_arch.png` — the same blue arch as a dungeon exit. The whole
    /// player-building pillar had no art at all, so "I built a wall" and "there is a
    /// portal here" were the same picture.
    pub(crate) structure_parts: HashMap<&'static str, Vec<(Handle<WorldAsset>, Vec3, f32, f32)>>,
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
        let installed: Vec<&str> = CREATURE_CHARS.iter().map(|(k, _)| *k).collect();
        let key = creature_art_key(kind, leader, &installed)?;
        self.creature_chars.get(&key)
    }

    /// A townsfolk's sprite set. Keyed by name, not pooled by prefix like a creature:
    /// an NPC is one PERSON, and the innkeeper standing at the inn has to be the
    /// innkeeper every time rather than whichever townsfolk the hash landed on.
    pub(crate) fn npc_frames(&self, key: &str) -> Option<&CharacterFrames> {
        self.npc_chars.get(key)
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
        "ground/atlas/forest.png",  // Forest
        "ground/atlas/desert.png",  // Desert
        "ground/atlas/ashfall.png", // Ashfall
        "ground/atlas/tundra.png",  // Tundra
        "ground/atlas/mire.png",    // Mire
    ]
    .iter()
    .map(|p| load_tiled(&assets, p))
    .collect();
    // A bridge's deck and parapets. Existing ground art, tiled the same way — the span needed
    // no assets of its own, which is why it could ship with the rest of the feature.
    let bridge_tex = (
        load_tiled(&assets, "ground/tile_path.png"),
        load_tiled(&assets, "ground/wall_rampart.png"),
    );
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
            bridge_deck: bridge_tex.0.clone(),
            bridge_parapet: bridge_tex.1.clone(),
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
            // Side-view rock per biome, tiled like everything else here.
            cliff_forest: load_tiled(&assets, "ground/cliff_forest.png"),
            cliff_desert: load_tiled(&assets, "ground/cliff_desert.png"),
            cliff_ashfall: load_tiled(&assets, "ground/cliff_ashfall.png"),
            cliff_tundra: load_tiled(&assets, "ground/cliff_tundra.png"),
            cliff_mire: load_tiled(&assets, "ground/cliff_mire.png"),
            amber_wood: load_tiled(&assets, "ground/atlas/amber_wood.png"),
            seized_engine: load_tiled(&assets, "ground/atlas/seized_engine.png"),
            nestiphian_cradle: load_tiled(&assets, "ground/atlas/nestiphian_cradle.png"),
            hearth_plains: load_tiled(&assets, "ground/atlas/hearth_plains.png"),
            seraphic_oubliette: load_tiled(&assets, "ground/atlas/seraphic_oubliette.png"),
            cliff_amber_wood: load_tiled(&assets, "ground/cliff_amber_wood.png"),
            cliff_seized_engine: load_tiled(&assets, "ground/cliff_seized_engine.png"),
            cliff_nestiphian_cradle: load_tiled(&assets, "ground/cliff_nestiphian_cradle.png"),
            cliff_hearth_plains: load_tiled(&assets, "ground/cliff_hearth_plains.png"),
            cliff_seraphic_oubliette: load_tiled(&assets, "ground/cliff_seraphic_oubliette.png"),
            dungeon_floor: load_tiled(&assets, "ground/atlas/dungeon.png"),
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
        ("myconid", "monsters/troll.png"),
        // The oozes, until their animated sets land (`CREATURE_CHARS`).
        ("verdant_ooze", "monsters/jelly.png"),
        ("bog_ooze", "monsters/acid_blob.png"),
        // The frog tribes, until their animated set lands. Without an entry here a kind
        // HASHES to an arbitrary sprite, so a brand-new species renders as whatever the
        // hash lands on — which reads as a bug rather than as missing art.
        ("frog_tribesman", "monsters/kobold.png"),
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
        // ⚠️ BD-1's STRUCTURAL NODES, AND THEY MUST BE HERE OR THEY ARE INVISIBLE.
        //
        // The node spawn tries `resource_<kind>.png` first and falls through to this map —
        // and if BOTH miss, it spawns NOTHING AT ALL. So shipping seven new materials
        // without a row here put seven kinds of gatherable stock into the world that no
        // player could see: the server knew they were there, `[E]` would even harvest one
        // you happened to stand on, and the ground looked empty. A material you cannot see
        // is a material that does not exist, the same way a token nothing renders does not.
        //
        // Timber reads as a stack of cut logs (deadfall you can carry off, not a standing
        // tree — that is CR's `Flora`); masonry as loose stone, sized and shaped to its
        // band. Bespoke billboards should replace these the moment the art exists, which is
        // what the `resource_<kind>.png` branch above is for.
        ("heartoak_log", sc("log_stack", 3.4)),
        ("bog_root_timber", sc("log", 4.041)),
        ("river_granite", sc("stone_smallFlatA", 6.2)),
        ("sun_sandstone", sc("stone_smallC", 7.337)),
        ("basalt_slab", sc("stone_smallFlatB", 6.2)),
        ("rime_stone", sc("stone_tallC", 5.4)),
        ("peat_shale", sc("stone_largeA", 4.6)),
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
    // `(clip, frames, drawn-for-all-eight-facings)`. The flag exists because a dungeon
    // sprite may be drawn only from the south — declaring such a clip as directional asks
    // the loader for seven folders nobody made, which is 56 missing-asset errors a launch.
    fn boss_clips(key: &str) -> &'static [(&'static str, usize, bool)] {
        match key {
            "gloamhound" => &[("walk", 8, true), ("attack", 8, true), ("howl", 8, true), ("pounce", 8, true)],
            "rustfang" => &[("walk", 8, true), ("attack", 8, true), ("slam", 8, true), ("overcharge", 8, true)],
            "choirmother" => &[("walk", 6, true), ("attack", 8, true), ("wail", 8, true), ("grasp", 8, true)],
            "pyrewarden" => &[("walk", 6, true), ("attack", 8, true), ("furnace_slam", 8, true), ("ember_burst", 8, true)],
            "sepulcher" => &[("walk", 8, true), ("attack", 8, true), ("rend", 8, true), ("phantom", 8, true)],
            "hollowbishop" => &[("walk", 6, true), ("attack", 8, true), ("soulfire", 8, true), ("bone_nova", 8, true)],
            "ironmaw" => &[("walk", 8, true), ("attack", 8, true), ("devour", 8, true), ("reactor_roar", 8, true)],
            "weepingcolossus" => &[("walk", 6, true), ("attack", 8, true), ("chain_sweep", 8, true), ("sorrow_quake", 8, true)],
            "miredrowned" => &[("walk", 6, true), ("attack", 8, true)],
            "ashenleviathan" => &[("walk", 8, true), ("attack", 8, true), ("cinder_charge", 8, true)],
            // The barrow's fae court: walk + attack, no ability art yet, so its kit
            // falls through to the attack clip.
            "briarlord" => &[("walk", 8, true), ("attack", 8, true)],
            // The Ocean Palace's guardian. A dungeon `sprite`, deliberately NOT one of
            // the named bosses (`meld_proto::bosses`) — it draws no name plate. Its walk
            // is still to be made, so it declares the attack only: naming a clip with no
            // frames behind it is asset errors on every launch.
            // Its attack is drawn SOUTH-ONLY; its walk is not drawn at all yet.
            "twingolem" => &[("attack", 8, false)],
            // ⚠️ THE WORLD BOSSES DO NOT WALK, so they declare an IDLE and nothing else.
            // The fallback below asks for `walk` and `attack` across all eight facings —
            // sixteen folders nobody drew, which is a wall of missing-asset errors every
            // launch. Their idle is drawn SOUTH-ONLY (they are met head-on, in an arena,
            // and never seen from behind), hence the `false`.
            "termina" | "nestiph" | "slake" | "ometus" | "velvetmaw" | "cogwright"
            | "vatmother" => &[("idle", 8, false)],
            // The All-Father is an OBJECT rather than a character: eight rotations and no
            // clips at all, because a mountain has no animation to give.
            "allfather" => &[],
            _ => &[("walk", 8, true), ("attack", 8, true)],
        }
    }
    let boss_chars: HashMap<String, CharacterFrames> = boss_keys()
        .map(|key| {
            (
                key.to_string(),
                hd2d::load_creature_clips(&assets, &format!("bosses/{key}"), boss_clips(key)),
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
        .map(|&(key, walk_frames)| {
            (
                key.to_string(),
                hd2d::load_creature_clips(
                    &assets,
                    &format!("creatures/{key}"),
                    // The walk's length comes from the SET, not a constant — six frames
                    // from the stock template, eight from a custom clip.
                    &[("walk", walk_frames, true), ("attack", 8, false)],
                ),
            )
        })
        .collect();

    // Townsfolk. Eighteen of the twenty-three never fight, so only the armed ones have
    // an attack — `load_creature_clips` is told which clips exist rather than assuming.
    let npc_chars: HashMap<String, CharacterFrames> = NPC_CHARS
        .iter()
        .map(|&key| {
            let armed = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("assets/npcs")
                .join(key)
                .join("animations/attack")
                .is_dir();
            let clips: &[(&str, usize, bool)] = if armed {
                &[("walk", 8, true), ("attack", 8, false)]
            } else {
                &[("walk", 8, true)]
            };
            (
                key.to_string(),
                hd2d::load_creature_clips(&assets, &format!("npcs/{key}"), clips),
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
        npc_chars,
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
        structure_parts: {
            // Kenney CC0 `fantasy-town`, already the kit Last City is built from — and its
            // matched wood/stone lines map onto BD-1's timber/masonry split exactly: a
            // palisade is `wall-wood`, an anchor is a standing stone.
            let kit = |p: &str| -> Handle<WorldAsset> {
                assets.load(GltfAssetLabel::Scene(0).from_asset(format!("models/fantasy-town/{p}.glb")))
            };
            let mut m: HashMap<&'static str, Vec<(Handle<WorldAsset>, Vec3, f32, f32)>> =
                HashMap::new();
            // A palisade: three timber panels in a short run, so it reads as a LENGTH of
            // wall rather than one lonely panel — a wall you cannot tell the facing of is a
            // wall you cannot line up with the next one.
            m.insert(
                "wall",
                vec![
                    (kit("wall-wood"), Vec3::new(0.0, 0.0, 0.0), 0.0, 2.2),
                    (kit("wall-wood"), Vec3::new(2.0, 0.0, 0.0), 0.0, 2.2),
                    (kit("wall-wood"), Vec3::new(-2.0, 0.0, 0.0), 0.0, 2.2),
                ],
            );
            // An anchor: a standing stone on a plinth. It has to read as PERMANENT from a
            // distance, because that is the entire claim it makes about the ground.
            m.insert(
                "anchor",
                vec![
                    (kit("pillar-stone"), Vec3::new(0.0, 0.0, 0.0), 0.0, 2.6),
                    (kit("wall-block-half"), Vec3::new(0.0, 0.0, 0.0), 0.0, 2.0),
                ],
            );
            m
        },
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
        // A dungeon wall is a vertical face, so it wants a SIDE-VIEW texture. It wore
        // `tile_street.png` — a top-down cobblestone — as the stopgap
        // docs/asset-pipeline.md has admitted to since it was written.
        wall_tex: load_tiled(&assets, "ground/wall_dungeon.png"),
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
                GroundDetail {
                    slot: IVec2::new(gx, gz),
                    last: IVec2::splat(i32::MIN),
                    epoch: u64::MAX,
                },
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

/// **THE TERRAIN EPOCH** — bumped whenever anything [`terrain_height`] reads changes.
///
/// ⚠️ **GROUND DETAIL IS GROUNDED ONCE PER CELL, AND TERRAIN ARRIVES LATER THAN THE DETAIL
/// STANDING ON IT.** `tile_ground_detail` re-derives a slot only when its world CELL
/// changes, and the height is computed inside that branch — so a mushroom placed before its
/// section's peaks, ranges, bridges or coastline arrived keeps the height it was given, for
/// as long as the player stays in the same cell. A Shift is the same story from the other
/// end: it re-cuts a region's peaks under detail that is already standing.
///
/// The result is scenery that does not follow the ground it is drawn on — reported as "no
/// sprites are following the heightmap", with mushrooms sitting on open water. Note it also
/// stales the WATER cull in the same branch, so a prop hidden for standing on the sea stays
/// hidden after a bridge makes that spot land.
///
/// Streaming made this reachable and it has been latent since detail existed: before ranges
/// and bridges were sent per section, far less of the height field arrived after the fact.
static TERRAIN_EPOCH: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Mark the height field changed. Every setter feeding [`terrain_height`] must call this —
/// held by `every_terrain_setter_invalidates_the_ground_detail`, because a landform added
/// later and not wired here is scenery floating over it with nothing saying so.
fn bump_terrain_epoch() {
    TERRAIN_EPOCH.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
}

/// The current terrain epoch, stamped onto each detail slot when it is grounded.
pub(crate) fn terrain_epoch() -> u64 {
    TERRAIN_EPOCH.load(std::sync::atomic::Ordering::Relaxed)
}

/// One recyclable cosmetic ground-detail prop. `slot` is its fixed offset (in cells)
/// from the player's current cell; `last` is the world cell it currently shows, so a
/// prop only re-derives (and swaps scene) when it actually moves to a new cell.
#[derive(Component)]
pub(crate) struct GroundDetail {
    slot: IVec2,
    last: IVec2,
    /// The epoch this slot's height was computed against. Differing from [`terrain_epoch`]
    /// means the ground moved under it and it must be re-derived even in the same cell.
    epoch: u64,
}

impl GroundDetail {
    /// A prop with no cell assigned — enough for tests that only care that the pool
    /// can be hidden.
    #[cfg(test)]
    pub(crate) fn for_test() -> Self {
        Self { slot: IVec2::ZERO, last: IVec2::ZERO, epoch: u64::MAX }
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

/// Wind sway for foliage: the sprite leans back and forth around its BASE, so the canopy
/// travels and the trunk stays planted. Lives on the billboard QUAD (not its root), because
/// a billboard owns its own world rotation — see [`animate_sway`] for why that decides
/// everything about how this is applied.
///
/// `pivot_y` is the quad's local height above the ground it stands on; the lean is a
/// rotation about that point rather than about the quad's centre, which is the difference
/// between a tree bending and a tree see-sawing at its waist.
#[derive(Component)]
pub(crate) struct Sway {
    pub(crate) pivot_y: f32,
    pub(crate) phase: f32,
    pub(crate) amp: f32,
    pub(crate) speed: f32,
}

/// Per-obstacle-kind wind-sway amplitude (radians of lean at full storm); `None` = rigid
/// (rock/cliff/etc). Read together with [`gust_response`], which is shaped so these numbers
/// land on the degrees the comments below claim.
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
        // A bare autumn canopy has less to catch the wind than a full one; a snow-laden
        // conifer is weighed down and barely moves. Same wind, different mass.
        "amber_tree" => Some(0.14),
        "mire_tree" => Some(0.10),
        "snow_tree" => Some(0.05),
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
    let gust = gust_response(wind);
    for (s, mut tf) in &mut q {
        // Faster, choppier motion the harder it blows.
        let a = (t * s.speed * (1.0 + wind) + s.phase).sin() * s.amp * gust;
        // ⚠️ COMPOSE ONTO THE BILLBOARD'S YAW, NEVER REPLACE IT — the same rule the grass
        // lean follows, for the same reason: `hd2d::billboard` wrote this rotation, and
        // assigning over it drops the sprite's facing and turns it edge-on. Ordered
        // `.after(hd2d::billboard)` at the registration, so the yaw is already here.
        //
        // Leaning about Z in the billboard's OWN frame is what makes the lean read as a lean
        // from every camera bearing. Rotating the ROOT instead would pivot at the base for
        // free, but the root is a billboard's ancestor, and a lean applied there tips the
        // sprite toward the camera rather than across the screen whenever the camera looks
        // down the world Z axis.
        let q = tf.rotation * Quat::from_rotation_z(a);
        tf.rotation = q;
        // Which leaves the pivot to rebuild by hand: a quad's own origin is its CENTRE, so
        // rotating there swings the trunk out as far as the canopy. Carrying the quad's
        // offset through the same rotation pivots the whole sprite about the ground instead.
        tf.translation = q * Vec3::new(0.0, s.pivot_y, 0.0);
    }
}

/// How hard the wind leans things, per unit of `sky.wind`.
///
/// Shaped rather than arbitrary: [`sway_amp`] is multiplied by this, so the two together
/// decide the actual angle, and an amplitude table nobody can read in degrees is what let a
/// tree ship leaning one and a half degrees. These coefficients put a tree (`0.17`) at **4°
/// in fair weather and 15° in a super storm** — the design the table's own comments state.
pub(crate) fn gust_response(wind: f32) -> f32 {
    0.213 + wind * 1.327
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
    // The height field's version, so a slot re-derives when the ground under it changes as
    // well as when the player walks it into a new cell.
    let epoch = terrain_epoch();
    for (mut d, mut tf, mut vis, mut root) in &mut q {
        let cell = cc + d.slot;
        if cell == d.last && d.epoch == epoch {
            continue; // same world cell AND the same ground — nothing to re-derive
        }
        d.last = cell;
        d.epoch = epoch;
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
    // The height field moved: re-ground the scenery standing on it.
    bump_terrain_epoch();
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
    // The height field moved: re-ground the scenery standing on it.
    bump_terrain_epoch();
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
    // The height field moved: re-ground the scenery standing on it.
    bump_terrain_epoch();
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
    // The height field moved: re-ground the scenery standing on it.
    bump_terrain_epoch();
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
    // The height field moved: re-ground the scenery standing on it.
    bump_terrain_epoch();
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
    // The height field moved: re-ground the scenery standing on it.
    bump_terrain_epoch();
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
    // The height field moved: re-ground the scenery standing on it.
    bump_terrain_epoch();
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
    // The height field moved: re-ground the scenery standing on it.
    bump_terrain_epoch();
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
    // The height field moved: re-ground the scenery standing on it.
    bump_terrain_epoch();
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
    pub(crate) peaks: Vec<[f32; 4]>,
    pub(crate) straits: Vec<meld_proto::coast::Strait>,
    pub(crate) lobes: Vec<meld_proto::coast::Lobe>,
    pub(crate) basins: Vec<meld_proto::coast::Basin>,
    pub(crate) rivers: Vec<meld_proto::coast::RiverNode>,
    pub(crate) bridges: Vec<meld_proto::coast::Bridge>,
}

impl ShoreData {
    /// Borrow it as a [`meld_proto::coast::Shore`] for the given fan.
    ///
    /// The PEAKS are part of the shoreline here because a basin fills against
    /// `height + peak_height` — a hill standing in a lake is an island, and leaving the domes
    /// out floods straight through a mountain.
    pub(crate) fn shore(&self, arc_half: f32) -> meld_proto::coast::Shore<'_> {
        meld_proto::coast::Shore {
            arc_half,
            terrain_off: self.terrain_off,
            peaks: &self.peaks,
            bridges: &self.bridges,
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
        peaks: peaks_snapshot(),
        straits: STRAITS.read().map(|s| s.clone()).unwrap_or_default(),
        lobes: LOBES.read().map(|l| l.clone()).unwrap_or_default(),
        basins: BASINS.read().map(|b| b.clone()).unwrap_or_default(),
        rivers: RIVERS.read().map(|r| r.clone()).unwrap_or_default(),
        bridges: bridges(),
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
    // ⚠️ **ON CHANGE ONLY — THIS ONE RUNS EVERY FRAME.** `terrain_height` reads all three,
    // so a change here does move the ground (entering the City flattens it, `amp` 0). But
    // this is called from the per-frame uniform update rather than on a message, so bumping
    // unconditionally would invalidate every detail slot every frame and turn the cache
    // into a full re-derivation — the opposite of what the epoch is for.
    // Each `swap` must run — separate bindings rather than one short-circuiting
    // expression, or a later store is skipped once an earlier one reports a change.
    let arc_moved = COAST_ARC.swap(arc_half.to_bits(), Relaxed) != arc_half.to_bits();
    let city_moved = COAST_CITY.swap(u32::from(city), Relaxed) != u32::from(city);
    let amp_moved = GROUND_AMP.swap(amp.to_bits(), Relaxed) != amp.to_bits();
    if arc_moved || city_moved || amp_moved {
        bump_terrain_epoch();
    }
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
    // …and the RANGES. This function places every prop, creature and the player's own feet,
    // so a range it did not know about would leave everything standing at the old ground level
    // with a mountain drawn through it — the same bug this function already shipped once for
    // the ocean and once for the straits.
    let land = base + peak + meld_proto::terrain::ridge_height(x, z, &ridges());
    // On a span, the ground IS the deck — a flat surface at its own level over the water, so
    // anything standing there stands on the bridge rather than in the sea beneath it.
    if let Some((rise, _)) = meld_proto::terrain::bridge_surface(x, z, &bridges()) {
        return -SEA_DEPTH + rise;
    }
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
pub(crate) const PROP_KEYS: [&str; 93] = [
    "obstacle_tree", "obstacle_tree_pine", "obstacle_tree_birch", "obstacle_tree_dead",
    "obstacle_tree_willow", "obstacle_tree_bushy",
    "obstacle_boulder", "obstacle_pond", "obstacle_dune",
    "obstacle_rock_spire", "obstacle_cactus", "obstacle_cliff", "obstacle_lava",
    "obstacle_cinder_rock", "obstacle_ice_spire", "obstacle_frozen_pond",
    "obstacle_snow_drift", "obstacle_bog_pool", "obstacle_mire_root", "obstacle_fungal_wall",
    // A WOOD IS THE BIOME IT GROWS IN. One `tree` kind drawn from one pool put the same
    // five trees in a swamp, a tundra and an autumn wood — so the thing you walk through,
    // which is most of what a biome LOOKS like, was the one part that never changed.
    // Each of these is its own obstacle kind with its own pool of four.
    "obstacle_amber_tree_1", "obstacle_amber_tree_2", "obstacle_amber_tree_3",
    "obstacle_amber_tree_4",
    "obstacle_mire_tree_1", "obstacle_mire_tree_2", "obstacle_mire_tree_3",
    "obstacle_mire_tree_4",
    "obstacle_snow_tree_1", "obstacle_snow_tree_2", "obstacle_snow_tree_3",
    "obstacle_snow_tree_4",
    // The boulder had exactly one rock. Four now, so a scree slope stops being one stone
    // stamped forty times.
    "obstacle_boulder_1", "obstacle_boulder_2", "obstacle_boulder_3", "obstacle_boulder_4",
    "resource_bloom_herb", "resource_heartoak_bark", "resource_sun_salts",
    "resource_dune_iron", "resource_ember_ash", "resource_cinder_ore",
    "resource_frost_lichen", "resource_rime_ore", "resource_bog_myrrh", "resource_peat_iron",
    // The deep world's nodes, and the STRUCTURAL stock every band already had but had no
    // model for — a gatherable with no art spawns nothing and is invisible in the world.
    "resource_coolant_bloom", "resource_brass_scrap", "resource_pale_shoot",
    "resource_bone_iron", "resource_rose_attar", "resource_gilt_sand",
    "resource_basalt_slab", "resource_bog_root_timber", "resource_heartoak_log",
    "resource_peat_shale", "resource_rime_stone", "resource_river_granite",
    "resource_sun_sandstone",
    "connector_ladder", "connector_rope", "connector_ramp",
    "item_chest_common", "item_chest_rare", "item_chest_open", "item_gold_pile", "item_loot_gem",
    "marker_target_marker",
    // The best chest in the game. `chest:<tier>` has always been on the wire and the
    // client drew every chest as the common one, so the blue art already in this list
    // was never once shown and the tier meant nothing to look at.
    "item_chest_red",
    // Dungeon traps, FOUR SPRITES PER KIND. A trap rides the wire as `trap:<kind>` and
    // used to draw as the target marker tinted red for every kind alike — so the one
    // thing the warning could have told you, which is what you are about to step on, was
    // the one thing it did not. Variants for the same reason creatures have them: a
    // corridor of identical thorn traps reads as copy-paste, and the pick is by entity
    // id so a given trap looks the same every time you walk past it.
    "trap_thorns_0", "trap_thorns_1", "trap_thorns_2", "trap_thorns_3",
    "trap_dart_0", "trap_dart_1", "trap_dart_2", "trap_dart_3",
    "trap_snare_0", "trap_snare_1", "trap_snare_2", "trap_snare_3",
    "trap_rune_0", "trap_rune_1", "trap_rune_2", "trap_rune_3",
    "trap_acid_0", "trap_acid_1", "trap_acid_2", "trap_acid_3",
    "trap_pit_0", "trap_pit_1", "trap_pit_2", "trap_pit_3",
];

/// Biome theme name → ground-texture / ring index (matches `BIOMES` order in
/// meld-world and the texture bindings in `ground_biome.wgsl`).
pub(crate) fn biome_ring_index(name: &str) -> usize {
    match name {
        "desert" => 1,
        "ashfall" => 2,
        "tundra" => 3,
        "mire" => 4,
        "amber_wood" => 5,
        "seized_engine" => 6,
        "nestiphian_cradle" => 7,
        "hearth_plains" => 8,
        "seraphic_oubliette" => 9,
        // "field" shares the forest's grass: a meadow and a wood stand on the same ground,
        // and the only thing that separates them is how many trees are in the way.
        _ => 0, // field / forest / unknown
    }
}

/// Feed the ground shader this world's coast, water, mountains and REGION DECOMPOSITION.
///
/// The biome half needs no window and no LUT any more: the shader derives a fragment's own
/// cell from five numbers, so there is nothing to centre on the player and nothing to run
/// out of at depth. The coast and water tables are still windowed, because those are lists.
pub(crate) fn update_ground_biome_rings(
    world: Res<Overworld>,
    session: Res<Session>,
    state: Res<State<Screen>>,
    tell: Res<crate::ShiftTell>,
    clock: Res<Time>,
    frame: Res<crate::WorldFrame>,
    dungeon: Res<DungeonSceneRes>,
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
    // Underground stands on a floor. Set beside `terrain_amp` because they answer the
    // same question — what KIND of place is this — and splitting them is how one of the
    // two ends up stale in a scene the other already knows about.
    mat.extension.params.dungeon = u32::from(dungeon.active);
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
    // The player's own position — the centre of every landform window below. Read BEFORE
    // the ranges rather than after, because they need it too.
    let (px, pz) = world
        .entities
        .get(&session.player_id)
        .map(|e| (e.x, e.y))
        .unwrap_or((0.0, 0.0));

    // THE AUTHORED PEAKS — **nearest-first**, for exactly the reason the ranges below are.
    //
    // ⚠️ **THIS WAS THE LAST FLAT TRUNCATION, AND IT MADE EVERYTHING FLY.** `PEAKS`
    // accumulates the base chain plus every streamed section, so taking the first
    // `PEAK_SLOTS` kept the SHALLOWEST domes in the world forever: walk past twenty-four of
    // them and the peaks around you stopped being uploaded, while `terrain_height` — which
    // reads the FULL list — went on lifting every entity onto them. A walkable dome is up to
    // `radius * PEAK_MAX_ASPECT` tall, so a radius-60 peak stands a creature ~25 units up in
    // the air over ground the shader draws flat. Reported as "everyone is flying".
    //
    // Every other landform here was already sorted (straits, lobes, basins) or was fixed
    // when the same bug was found in the ranges; the peaks were missed **because they were
    // written before the pattern existed**, and left alone when the ranges were corrected.
    // `every_windowed_landform_is_sorted_nearest_first` is why they cannot be missed again.
    let mut peaks = peaks_snapshot();
    peaks.sort_by(|a, b| {
        let d = |q: &[f32; 4]| (q[0] - px).hypot(q[1] - pz) - q[2];
        d(a).total_cmp(&d(b))
    });
    let n = peaks.len().min(PEAK_SLOTS);
    for (i, slot) in mat.extension.params.peaks.iter_mut().enumerate() {
        *slot = if i < n {
            Vec4::new(peaks[i][0], peaks[i][1], peaks[i][2], peaks[i][3])
        } else {
            Vec4::ZERO
        };
    }
    mat.extension.params.peak_count = n as u32;
    // THE RANGES, two vec4s each.
    //
    // ⚠️ **TRUNCATION IS NOT A WINDOW, AND THIS SAID "windowed" WHILE IT TRUNCATED.**
    // `SECTION_RIDGES` is a `BTreeMap` keyed by section and flattened in section ORDER, so
    // taking the first `RIDGE_SLOTS / 2` kept the SHALLOWEST ranges in the world forever.
    // Walk out past sixteen of them and every range after that stopped being drawn while
    // `landform_slope` — server-side, and client-side in `terrain_height` — went on
    // colliding against it: an invisible wall that refuses you, and raises you as you slide
    // along a mountainside the ground renders as flat. The straits twenty lines below got
    // this right; these two did not. Nearest-first, by real distance to the span.
    let near_first = |s: &[f32; 6]| {
        meld_proto::coast::dist_to_segment_pub(px, pz, s[0], s[1], s[2], s[3])
    };
    let mut rg = ridges();
    rg.sort_by(|a, b| near_first(a).total_cmp(&near_first(b)));
    let rn = rg.len().min(RIDGE_SLOTS / 2);
    for (i, slot) in mat.extension.params.ridges.iter_mut().enumerate() {
        let (seg, half) = (i / 2, i % 2);
        *slot = match rg.get(seg) {
            Some(r) if seg < rn && half == 0 => Vec4::new(r[0], r[1], r[2], r[3]),
            Some(r) if seg < rn => Vec4::new(r[4], r[5], 0.0, 0.0),
            _ => Vec4::ZERO,
        };
    }
    mat.extension.params.ridge_count = rn as u32;
    // THE BRIDGES, two vec4s each, same packing as the ranges — and nearest-first for the
    // same reason. A bridge is the one landform you are guaranteed to be standing ON when it
    // matters, so dropping the near one for a shallower one is the worst possible trade.
    let span_near_first = |s: &[f32; 5]| {
        meld_proto::coast::dist_to_segment_pub(px, pz, s[0], s[1], s[2], s[3])
    };
    let mut bg = bridges();
    bg.sort_by(|a, b| span_near_first(a).total_cmp(&span_near_first(b)));
    let bn = bg.len().min(BRIDGE_SLOTS / 2);
    for (i, slot) in mat.extension.params.bridges.iter_mut().enumerate() {
        let (seg, half) = (i / 2, i % 2);
        *slot = match bg.get(seg) {
            Some(b) if seg < bn && half == 0 => Vec4::new(b[0], b[1], b[2], b[3]),
            Some(b) if seg < bn => Vec4::new(b[4], 0.0, 0.0, 0.0),
            _ => Vec4::ZERO,
        };
    }
    mat.extension.params.bridge_count = bn as u32;
    // The player's own ring — the centre of both windows below (straits and biome rings).
    let pr = px.hypot(pz);
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

    // THE REGION DECOMPOSITION. Not a window and not a LUT: the shader derives a fragment's
    // own cell from these five numbers, so there is nothing to centre on the player and
    // nothing to run out of at depth — which is what the ring window had to keep managing.
    let rg = regions();
    let p = &mut mat.extension.params;
    p.region = Vec4::new(rg.grid.arc_half, rg.grid.ring_step, rg.grid.cell_width, rg.grid.warp);
    p.region_blend = rg.blend;
    p.region_seed = rg.grid.seed;
    p.region_force = rg.force;
    // `BIOMES` order, four then two — the split is only that a uniform wants `vec4`s.
    let g = |i: usize| rg.gate.get(i).copied().unwrap_or(0.0);
    p.gate = Vec4::new(g(0), g(1), g(2), g(3));
    p.gate_hi = Vec4::new(g(4), g(5), g(6), g(7));
    p.gate_hi2 = Vec4::new(g(8), g(9), g(10), 0.0);
}

/// **THIS WORLD'S REGION DECOMPOSITION**, as the server sent it on `run.started`.
///
/// Held beside the terrain offset, the straits and the world seed because it is the same
/// kind of thing: a per-world fact handed down once that several surfaces read. Never
/// derived client-side — the gate and the cell size are balance, and a client that guessed
/// them would paint a world the server does not hold.
static REGIONS: std::sync::RwLock<Option<meld_proto::regions::Regions>> =
    std::sync::RwLock::new(None);

/// **THIS WORLD'S BRIDGES** — spans of forced land carrying the trail across a strait. Held
/// beside the ranges for the same reason: a per-world landform table the ground shader draws
/// and [`terrain_height`] stands entities on.
static BRIDGES: std::sync::RwLock<Vec<meld_proto::coast::Bridge>> =
    std::sync::RwLock::new(Vec::new());

/// The bridges that came with `run.started` — the initial chain's, which no section message
/// re-sends. Held apart from the streamed ones exactly as `BASE_RIDGES` is.
static BASE_BRIDGES: std::sync::RwLock<Vec<meld_proto::coast::Bridge>> =
    std::sync::RwLock::new(Vec::new());

/// Streamed bridges, keyed by the section that sent them.
static SECTION_BRIDGES: std::sync::RwLock<
    std::collections::BTreeMap<u32, Vec<meld_proto::coast::Bridge>>,
> = std::sync::RwLock::new(std::collections::BTreeMap::new());

/// Replace this world's bridges (call on `run.started`).
pub(crate) fn set_bridges(b: Vec<meld_proto::coast::Bridge>) {
    if let Ok(mut base) = BASE_BRIDGES.write() {
        *base = b.clone();
    }
    if let Ok(mut by_section) = SECTION_BRIDGES.write() {
        by_section.clear();
    }
    if let Ok(mut all) = BRIDGES.write() {
        *all = b;
    }
    // The height field moved: re-ground the scenery standing on it.
    bump_terrain_epoch();
}

/// Replace one section's bridges (call on `world.terrain_section`).
///
/// ⚠️ **THIS WAS MISSING, AND IT MADE EVERY DEEP BRIDGE INVISIBLE.** `run.started` carries
/// only the initial chain (`area_count` 8), while `strait_min_section` is 6 — so two of the
/// eight sections a run starts with can hold a strait, and *every other strait in the world*
/// is in a STREAMED section. `world.terrain_section` carried `bridges` all the way to the
/// client, and nothing here consumed them: past the initial chain the shader drew open water,
/// `terrain_height` stood nobody on a deck, and the server's `is_land` walked the party
/// straight across it. A span you cross without seeing it is the isthmus failure in its worst
/// form — the sea simply not being there, with no bridge even drawn.
///
/// REPLACE, never append — a Shift re-sends a section, and two copies of one span read as a
/// wider deck nothing in the world model believes is there.
pub(crate) fn set_section_bridges(index: u32, bridges: &[meld_proto::coast::Bridge]) {
    let Ok(mut by_section) = SECTION_BRIDGES.write() else { return };
    if bridges.is_empty() && !by_section.contains_key(&index) {
        return;
    }
    by_section.insert(index, bridges.to_vec());
    let streamed: Vec<meld_proto::coast::Bridge> =
        by_section.values().flatten().copied().collect();
    if let (Ok(mut b), Ok(base)) = (BRIDGES.write(), BASE_BRIDGES.read()) {
        *b = base.iter().copied().chain(streamed).collect();
    }
    // The height field moved: re-ground the scenery standing on it.
    bump_terrain_epoch();
}

/// This world's bridges.
pub(crate) fn bridges() -> Vec<meld_proto::coast::Bridge> {
    BRIDGES.read().map(|b| b.clone()).unwrap_or_default()
}

/// **THIS WORLD'S RANGES.** Held beside the peaks because it is the same kind of thing: a
/// per-world landform table the server hands down and both the ground shader and
/// [`terrain_height`] read, so an entity stands on the mountain rather than inside it.
static RIDGES: std::sync::RwLock<Vec<meld_proto::terrain::Ridge>> =
    std::sync::RwLock::new(Vec::new());

/// Replace this world's ranges (call on `run.started`).
pub(crate) fn set_ridges(r: Vec<meld_proto::terrain::Ridge>) {
    if let Ok(mut base) = BASE_RIDGES.write() {
        *base = r.clone();
    }
    if let Ok(mut by_section) = SECTION_RIDGES.write() {
        by_section.clear();
    }
    if let Ok(mut all) = RIDGES.write() {
        *all = r;
    }
    // The height field moved: re-ground the scenery standing on it.
    bump_terrain_epoch();
}

/// The ranges that came with `run.started` — the initial chain's, which no section message
/// re-sends. Held apart from the streamed ones so rebuilding after a retile cannot drop them.
static BASE_RIDGES: std::sync::RwLock<Vec<meld_proto::terrain::Ridge>> =
    std::sync::RwLock::new(Vec::new());

/// Streamed ranges, keyed by the section that sent them.
static SECTION_RIDGES: std::sync::RwLock<
    std::collections::BTreeMap<u32, Vec<meld_proto::terrain::Ridge>>,
> = std::sync::RwLock::new(std::collections::BTreeMap::new());

/// Replace one section's ranges (call on `world.terrain_section`).
///
/// ⚠️ **REPLACE, never append.** A Shift re-cuts a region's topography and re-sends the
/// section, so accumulating would leave the old range standing beside the new one — and
/// because ranges combine with `max`, two overlapping walls read as one taller wall that
/// nothing in the world model believes is there.
pub(crate) fn set_section_ridges(index: u32, ridges: &[meld_proto::terrain::Ridge]) {
    let Ok(mut by_section) = SECTION_RIDGES.write() else { return };
    if ridges.is_empty() && !by_section.contains_key(&index) {
        return;
    }
    by_section.insert(index, ridges.to_vec());
    let streamed: Vec<meld_proto::terrain::Ridge> =
        by_section.values().flatten().copied().collect();
    if let (Ok(mut r), Ok(base)) = (RIDGES.write(), BASE_RIDGES.read()) {
        *r = base.iter().copied().chain(streamed).collect();
    }
    // The height field moved: re-ground the scenery standing on it.
    bump_terrain_epoch();
}

/// This world's ranges.
pub(crate) fn ridges() -> Vec<meld_proto::terrain::Ridge> {
    RIDGES.read().map(|r| r.clone()).unwrap_or_default()
}

/// Record this world's decomposition (call on `run.started`).
pub(crate) fn set_regions(r: meld_proto::regions::Regions) {
    *REGIONS.write().unwrap() = Some(r);
}

/// This world's decomposition, or the empty one (`ring_step == 0`, which every reader
/// treats as "no world here") before a run has started.
pub(crate) fn regions() -> meld_proto::regions::Regions {
    REGIONS.read().unwrap().clone().unwrap_or_default()
}

/// The biome at a world position, by the same decomposition the ground shader paints with.
/// The one place a client-side coordinate becomes a theme, so grass placement, the minimap
/// and the HUD label cannot disagree with the floor they are drawn on.
pub(crate) fn biome_at_world(x: f32, z: f32) -> &'static str {
    let rg = regions();
    // No world yet (menus, city): the decomposition is inert and everything reads forest.
    // A FORCED biome still answers, so `MELD_BIOME` colours those screens too.
    if rg.grid.ring_step <= 0.0 && rg.force < 0 {
        return "forest";
    }
    meld_proto::regions::BIOMES[rg.biome_at(x, z)]
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
    let (wind_target, rain_target) = wind_and_rain_targets(sky.phase, sky.super_storm);
    let wr = 0.35 * dt;
    sky.wind += (wind_target - sky.wind).clamp(-wr, wr);
    let rr = 0.25 * dt;
    sky.weather += (rain_target - sky.weather).clamp(-rr, rr);
}

/// The weather's shape, as a pure function so the ordering it promises can be tested:
/// Fair → Gust → Storm → Clearing, with the wind LEADING the rain and peaking in the
/// downpour.
pub(crate) fn wind_and_rain_targets(phase: u8, super_storm: bool) -> (f32, f32) {
    match phase {
        1 => (0.7, 0.0),
        // ⚠️ A STORM MUST OUTBLOW ITS OWN PRECURSOR. The gust phase targets 0.7, so an
        // ordinary storm at 0.65 blew SOFTER than the wind that announced it — the weather
        // peaked and then eased off exactly as the rain arrived, which is backwards from
        // the shape the phase machine is built to tell.
        2 => (if super_storm { 1.0 } else { 0.82 }, 1.0),
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
    }
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

    /// **EVERY WINDOWED LANDFORM MUST BE SORTED NEAREST-FIRST, NOT TRUNCATED.**
    ///
    /// Each landform rides a fixed-size uniform array while the world streams outward without
    /// bound, so something must be dropped. WHICH ones is the whole question: sorted by
    /// distance you lose what you cannot see; flat-truncated you keep whatever is first in the
    /// list — and for every store here that is the SHALLOWEST in the world, forever.
    ///
    /// The failure is silent and severe, because the CPU does not truncate: `Shore` and
    /// `terrain_height` read the FULL list. A dropped landform is one the client collides with
    /// and stands entities on while drawing flat ground over it. It has happened twice — the
    /// ranges (an invisible wall), then the peaks (**"everyone is flying"**: an entity lifted
    /// ~25 units onto a dome that was never uploaded).
    ///
    /// Read off the SOURCE, because a human read this function twice and missed the peaks.
    #[test]
    fn every_windowed_landform_is_sorted_nearest_first() {
        // ⚠️ **SCAN PRODUCTION CODE ONLY.** This test's own body contains `.truncate(` as a
        // string literal, so scanning the whole file made the guard match ITSELF and then
        // fail for having no `sort_by` above it. A source-reading test has to exclude the
        // source that does the reading.
        let full = include_str!("world_render.rs");
        // Anchored on THIS module by name, not on `#[cfg(test)]` — that attribute also sits
        // on `GroundDetail::for_test` a thousand lines earlier, and cutting there silently
        // hid every real cut from the scan. The count assertion below is what caught that.
        let src = &full[..full.find("mod ground_uniform_tests").unwrap_or(full.len())];
        let lines: Vec<&str> = src.lines().collect();
        let is_cut = |l: &str| {
            (l.contains(".min(PEAK_SLOTS")
                || l.contains(".min(RIDGE_SLOTS")
                || l.contains(".min(BRIDGE_SLOTS")
                || l.contains(".truncate("))
                && !l.trim_start().starts_with("//")
                && !l.trim_start().starts_with("///")
        };
        let cuts: Vec<usize> = lines
            .iter()
            .enumerate()
            .filter(|(_, l)| is_cut(l))
            .map(|(i, _)| i)
            .collect();
        assert!(
            cuts.len() >= 6,
            "found only {} windowed landforms — the scan is not guarding anything. (This \
             assertion has already earned its keep once: the first version of this test \
             found 2 of 6 and said so instead of passing.)",
            cuts.len()
        );
        for cut in cuts {
            // A sort belonging to this cut sits just above it — same statement group.
            let lo = cut.saturating_sub(8);
            let sorted = lines[lo..cut].iter().any(|l| l.contains("sort_by"));
            assert!(
                sorted,
                "line {} cuts a landform to its slot count with no `sort_by` within 8 lines \
                 above it. That is a flat truncation: it keeps the shallowest landforms in \
                 the world and makes the client collide with, and stand entities on, terrain \
                 it never draws — the \"everyone is flying\" bug.\n    {}",
                cut + 1,
                lines[cut].trim()
            );
        }
    }


    /// **EVERY TERRAIN SETTER MUST INVALIDATE THE SCENERY STANDING ON IT.**
    ///
    /// `tile_ground_detail` grounds a slot once per world CELL, so anything that moves the
    /// height field after the fact leaves scenery at the old height — mushrooms on open
    /// water, props sunk into a range. The epoch is what re-derives them, and it only works
    /// if every setter feeding `terrain_height` bumps it.
    ///
    /// So this reads the SOURCE rather than trusting a list: a new landform setter added
    /// here either bumps the epoch or names itself in `EXEMPT`, with a reason. A
    /// hand-written roster is the thing this repo has been bitten by repeatedly — the whole
    /// point is that forgetting is what fails, not remembering.
    #[test]
    fn every_terrain_setter_invalidates_the_ground_detail() {
    // Setters that genuinely do not move the ground or the waterline.
    const EXEMPT: &[(&str, &str)] = &[
        ("set_world_seed", "a name, not geometry"),
        ("set_regions", "paints the biome; height is the same field either way"),
        ("set_ground_coast", "bumps on CHANGE — it runs every frame (see its comment)"),
    ];
    let src = include_str!("world_render.rs");
    let mut checked = 0;
    for (idx, _) in src.match_indices("pub(crate) fn set_") {
        let name: String = src[idx + "pub(crate) fn ".len()..]
            .chars()
            .take_while(|c| c.is_alphanumeric() || *c == '_')
            .collect();
        if EXEMPT.iter().any(|(e, _)| *e == name) {
            continue;
        }
        // The function body, by brace matching from its signature.
        let open = idx + src[idx..].find('{').expect("a body");
        let (mut depth, mut end) = (0usize, open);
        for (o, c) in src[open..].char_indices() {
            match c {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        end = open + o;
                        break;
                    }
                }
                _ => {}
            }
        }
        assert!(
            src[open..end].contains("bump_terrain_epoch"),
            "`{name}` sets terrain state without bumping the epoch, so ground detail \
             placed before it keeps the height it was given — scenery floating over the \
             world. Bump it, or add it to EXEMPT with a reason."
        );
        checked += 1;
    }
    assert!(checked >= 13, "only {checked} setters checked — the scan is not finding them");
    }


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
            // The landforms WG-7 added. Both displace geometry, so both must be in the
            // shadow pass too — a range that casts no shadow reads as painted-on, and a
            // deck that casts none is a plank floating over its own water.
            "fn rg_seg_dist(",
            "fn ridge_wedge(",
            "fn bridge_at(",
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
        for f in [
            "sea_depth_at",
            "inland_water_at",
            "strait_depth_at",
            "spit_half_width",
            // …and the two WG-7 landforms, for exactly the reason in the doc comment above:
            // a helper nothing calls renders nothing, and no other test notices.
            "ridge_wedge",
            "bridge_at",
        ] {
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

    /// **THE RANGES ARE WRITTEN THREE TIMES** — `terrain::ridge_height` in Rust and
    /// `ridge_wedge` in each of the two ground shaders. A range is a WALL the server collides
    /// against, so a renderer that computes it differently draws a mountain somewhere the
    /// world does not have one, and the player walks into open air.
    ///
    /// Both halves of the shape are held here, because both are load-bearing: **linear**
    /// falloff is what makes slope exactly `height / half_width` at every point (a cosine
    /// would be gentler off the crest and impassability would stop being an identity), and
    /// **`max`** is what stops overlapping segments of one range stacking into a wall twice
    /// its authored height at every joint.
    #[test]
    fn the_ranges_are_drawn_the_way_the_server_collides_with_them() {
        for (name, src) in [
            ("ground_biome.wgsl", include_str!("../assets/shaders/ground_biome.wgsl")),
            ("ground_prepass.wgsl", include_str!("../assets/shaders/ground_prepass.wgsl")),
        ] {
            assert!(src.contains("fn ridge_wedge("), "{name} must define `ridge_wedge`");
            assert!(
                src.matches("ridge_wedge(").count() >= 2,
                "{name} defines `ridge_wedge` and never calls it — a barrier the ground does \
                 not draw is an invisible wall, which is worse than no wall"
            );
            // Linear, not a dome: `(1.0 - d / hw)`.
            assert!(
                src.contains("(1.0 - d / hw)"),
                "{name}'s range must fall off LINEARLY — that is what makes its slope exactly \
                 height/half_width at every point on the flank"
            );
            // `max`, not `+`.
            assert!(
                src.contains("h = max(h, r1.y * (1.0 - d / hw))"),
                "{name} must combine ranges with `max` — summing stacks overlapping segments \
                 of one range into a wall twice its authored height"
            );
            // A range DISPLACES the ground, so both passes must raise it — unlike inland
            // water, which the prepass deliberately omits.
            assert!(
                src.contains("+ ridge_wedge(wxz)"),
                "{name} must add `ridge_wedge` to the land term, or the ground it draws (or \
                 shadows) is a world with no mountains in it"
            );
        }
    }

    /// The uniform reserves two `vec4`s per range, and a shorter array in the shader would
    /// truncate the table silently — `min_size()` is computed from the Rust struct alone and
    /// cannot notice.
    #[test]
    fn the_shader_reserves_room_for_every_range() {
        let want = meld_proto::terrain::MAX_RIDGES * 2;
        assert_eq!(RIDGE_SLOTS, want, "two vec4s per range");
        for (name, src) in [
            ("ground_biome.wgsl", include_str!("../assets/shaders/ground_biome.wgsl")),
            ("ground_prepass.wgsl", include_str!("../assets/shaders/ground_prepass.wgsl")),
        ] {
            assert!(
                src.contains(&format!("ridges: array<vec4<f32>, {want}>")),
                "{name} must declare `ridges: array<vec4<f32>, {want}>`"
            );
        }
    }

    /// ⚠️ **THE REGION DECOMPOSITION IS WRITTEN TWICE**, in `meld_proto::regions` and again in
    /// WGSL, because a shader cannot import Rust. The whole module is 32-bit integer and f32
    /// arithmetic FOR this reason — WGSL has no 64-bit integer — so the two can mirror line
    /// for line. What they cannot do is notice each other drifting: a changed hash constant
    /// or sector cap on the Rust side would leave the server spawning one world and the
    /// ground painting another, with nothing red anywhere.
    ///
    /// So derive the numbers from the Rust constants and read them out of the shader.
    #[test]
    fn the_region_decomposition_matches_the_shader() {
        use meld_proto::regions::MAX_SECTORS;
        let wgsl = include_str!("../assets/shaders/ground_biome.wgsl");
        let bits = MAX_SECTORS.trailing_zeros();

        // The cap on sectors per ring, and the key packing that depends on it.
        assert!(
            wgsl.contains(&format!("{MAX_SECTORS}u)")),
            "the shader must cap sectors at MAX_SECTORS ({MAX_SECTORS})"
        );
        assert!(
            wgsl.contains(&format!("(ring << {bits}u) | (sector & {}u)", MAX_SECTORS - 1)),
            "the shader must pack a cell key the same way `Cell::key` does \
             (<< {bits}, mask {})",
            MAX_SECTORS - 1
        );
        // The biome list length is the gate's length and the loop bound.
        assert!(
            wgsl.contains(&format!("i < {}u", meld_proto::regions::BIOMES.len())),
            "the shader must walk all {} biomes when filtering the gate",
            meld_proto::regions::BIOMES.len()
        );
        // Every salt. A changed one silently repartitions the world on one side only.
        for salt in ["0x7feb352du", "0x846ca68bu", "0x9E3779B9u", "0x5F356495u", "0x2545F491u"] {
            assert!(wgsl.contains(salt), "the shader is missing the salt `{salt}`");
        }
        // The warp's two harmonics — its whole job is to stop a ring boundary reading as an
        // arc, and a shader that wobbles it differently draws a boundary the server does not
        // have.
        for term in ["0.62 * sin(bearing * 3.0 + phase)", "0.38 * cos(bearing * 7.0 - phase * 2.0)"] {
            assert!(wgsl.contains(term), "the shader's ring warp is missing `{term}`");
        }
    }

    /// **Every region helper the ground shader defines must actually be CALLED** — the same
    /// rule as the coast helpers below, for the same reason: inland water shipped completely
    /// invisible because `inland_depth_at` was defined, mirrored, carried through the uniform
    /// and never called from a fragment, and WGSL does not complain about an unused function.
    #[test]
    fn every_region_helper_is_actually_called() {
        let wgsl = include_str!("../assets/shaders/ground_biome.wgsl");
        for f in [
            "rg_hash32", "rg_sectors", "rg_ring_offset", "rg_warp_at", "rg_ring_at",
            "rg_fan_t", "rg_sector_in", "rg_biome_of", "rg_biome_at", "rg_edge", "rg_tex_of",
        ] {
            assert!(wgsl.contains(&format!("fn {f}(")), "ground_biome.wgsl should define `{f}`");
            assert!(
                wgsl.matches(&format!("{f}(")).count() >= 2,
                "`{f}` is defined in ground_biome.wgsl and never called — a shader helper \
                 with no call site renders nothing, and nothing else in this suite notices"
            );
        }
    }

    /// **A DECK'S HEIGHT IS WRITTEN TWICE, SO HOLD THE TWO TOGETHER.** `bridge_at` mirrors
    /// `terrain::bridge_surface` into both ground shaders as bare literals, because WGSL
    /// cannot import a Rust const — the same situation as `BASIN_SHORE_SLOPE` below, and the
    /// same answer. Drift here does not fail anything on its own: the CPU stands entities on
    /// one deck height while the GPU draws another, so the party walks a span at the wrong
    /// level — sunk into it, or hovering over it — and every test stays green.
    #[test]
    fn a_bridge_deck_is_the_same_height_in_both_worlds() {
        let wgsl = include_str!("../assets/shaders/ground_biome.wgsl");
        for (name, v) in [
            ("BRIDGE_DECK_RISE", meld_proto::terrain::BRIDGE_DECK_RISE),
            ("BRIDGE_PARAPET_RISE", meld_proto::terrain::BRIDGE_PARAPET_RISE),
            ("BRIDGE_PARAPET_SHARE", meld_proto::terrain::BRIDGE_PARAPET_SHARE),
        ] {
            assert!(
                wgsl.contains(&format!("{v:?}")),
                "`terrain::{name}` is {v:?} and `bridge_at` in ground_biome.wgsl does not \
                 carry that literal — the deck the client DRAWS is not the deck \
                 `terrain_height` stands the party on"
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
            "bridges", "bridge_count",
            "region", "gate", "gate_hi", "gate_hi2", "region_blend", "region_seed",
            "region_force", "uv_scale",
            "terrain_amp", "terrain_off",
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
            Some("region"),
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
        // Must agree with `sync_creature_chars.py`'s definition, because the two decide
        // the same thing from opposite sides — and when they disagreed, the looser one
        // (this) called a set finished that the stricter one refused to list, so the test
        // failed on art that was genuinely broken and pointed at the wrong culprit.
        //
        // EVERY FACING MUST AGREE ON ITS FRAME COUNT. `dune_colossus_sunmarked` came back
        // with seven frames facing north and eight everywhere else — one job of eight
        // quietly short — which a "does frame_000 exist" check waves straight through.
        let complete = |key: &str| -> bool {
            let d = dir.join(key);
            if !d.join("rotations/south.png").is_file()
                || !d.join("animations/attack/south/frame_000.png").is_file()
            {
                return false;
            }
            let counts: Vec<usize> = hd2d::DIRS
                .iter()
                .map(|f| {
                    std::fs::read_dir(d.join("animations/walk").join(f))
                        .map(|r| r.count())
                        .unwrap_or(0)
                })
                .collect();
            counts[0] >= 4 && counts.iter().all(|n| *n == counts[0])
        };
        for e in entries.filter_map(|e| e.ok()).filter(|e| e.path().is_dir()) {
            let key = e.file_name().to_string_lossy().into_owned();
            let listed = CREATURE_CHARS.iter().any(|(k, _)| *k == key);
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
        for (key, frames) in CREATURE_CHARS {
            assert!(
                dir.join(key).is_dir(),
                "CREATURE_CHARS lists {key} but assets/creatures/{key} does not exist"
            );
            // THE DECLARED LENGTH MUST MATCH THE FILES. The loader asks for exactly this
            // many frames per facing, so a number that drifted from the art is a missing
            // -asset error per frame per direction, on every launch.
            let d = dir.join(key).join("animations/walk/south");
            let on_disk = std::fs::read_dir(&d).map(|r| r.count()).unwrap_or(0);
            assert_eq!(
                on_disk, *frames,
                "creatures/{key} declares a {frames}-frame walk but has {on_disk}"
            );
        }
    }

    /// A species draws from its POOL, and no variant need carry the species' own name.
    #[test]
    fn a_species_draws_from_its_whole_pool() {
        // The case that forced this: six myconids, not one of them called `myconid`.
        const MYCONID: &[&str] = &["myconid_brute", "myconid_mage", "myconid_minion",
                                   "myconid_pack_leader", "myconid_warrior"];
        assert_eq!(
            creature_art_key("myconid", true, MYCONID).as_deref(),
            Some("myconid_pack_leader"),
            "a leader must find the leader art"
        );
        let ordinary = creature_art_key("myconid", false, MYCONID);
        assert!(
            ordinary.as_deref().is_some_and(|k| !k.ends_with("_pack_leader")),
            "an ordinary myconid drew its own leader's art"
        );
        // Stable across list order: a creature must not change appearance because the
        // sync happened to write the folders in a different sequence.
        let shuffled: Vec<&str> = MYCONID.iter().rev().copied().collect();
        assert_eq!(ordinary, creature_art_key("myconid", false, &shuffled));

        // The species' own name still wins when a species does have one.
        const BOAR: &[&str] = &["thornback_boar", "thornback_boar_beta",
                                "thornback_boar_pack_leader"];
        assert_eq!(
            creature_art_key("thornback_boar", false, BOAR).as_deref(),
            Some("thornback_boar")
        );

        // A leader with no leader art borrows an ordinary one rather than drawing nothing.
        const NO_LEADER: &[&str] = &["sporeling_baby"];
        assert_eq!(
            creature_art_key("sporeling", true, NO_LEADER).as_deref(),
            Some("sporeling_baby")
        );
        // And a species with no art at all stays on its billboard.
        assert_eq!(creature_art_key("glacier_maw", false, NO_LEADER), None);
        // A prefix must not bleed across species: `bog_ooze` is not a `bog` variant.
        const BOG: &[&str] = &["bog_ooze_baby"];
        assert_eq!(creature_art_key("bog_serpent", false, BOG), None);
    }
}

#[cfg(test)]
mod node_art_tests {
    /// **Every gatherable material must have something to draw.** The node spawn tries a
    /// bespoke `resource_<kind>.png` billboard, falls back to a 3D scene, and if BOTH miss
    /// it spawns nothing at all — so a material with neither is invisible stock. BD-1
    /// shipped seven of them that way for one commit.
    ///
    /// Every obstacle kind the world may place has art on this side.
    ///
    /// Held against `meld_proto::obstacles::KINDS` rather than a list here, because the two
    /// sides cannot see each other: the server picks the kinds, this draws them, and the
    /// registry is the only place both read. A wooded kind needs a whole POOL — one missing
    /// variant is a tree that silently never appears in the rotation.
    #[test]
    fn every_obstacle_kind_has_art() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/props");
        for kind in meld_proto::obstacles::KINDS {
            if let Some(pool) = crate::overworld::tree_pool(kind) {
                assert!(!pool.is_empty(), "{kind} is wooded but its pool is empty");
                for k in pool {
                    assert!(dir.join(format!("{k}.png")).is_file(), "{kind}: {k}.png missing");
                }
            } else {
                assert!(
                    dir.join(format!("obstacle_{kind}.png")).is_file(),
                    "{kind} has no obstacle_{kind}.png — it would spawn nothing"
                );
            }
        }
        for kind in meld_proto::obstacles::WOODED {
            assert!(crate::overworld::tree_pool(kind).is_some(), "{kind} is wooded but has no pool");
        }
    }

    /// Checked against the material REGISTRY rather than a list, because the failure mode is
    /// adding a material and forgetting the art, and a hand-written list is a list the new
    /// material gets left off.
    #[test]
    fn every_gatherable_material_has_something_to_render() {
        // The scene map is built inside `setup` against a live `AssetServer`, so mirror just
        // its KEYS here — the same discipline as the shader-mirror tests.
        const SCENE_KEYS: &[&str] = &[
            "bloom_herb", "heartoak_bark", "sun_salts", "dune_iron", "ember_ash", "cinder_ore",
            "frost_lichen", "rime_ore", "bog_myrrh", "peat_iron", "heartoak_log",
            "bog_root_timber", "river_granite", "sun_sandstone", "basalt_slab", "rime_stone",
            "peat_shale",
        ];
        for m in meld_proto::materials::MATERIALS {
            // Only the ones the world actually scatters as nodes: refined stock is smelted
            // and a trophy comes off a carcass, so neither is ever a thing standing in a
            // field waiting to be harvested.
            if !matches!(
                m.class,
                meld_proto::materials::MaterialClass::Reagent
                    | meld_proto::materials::MaterialClass::Ore
                    | meld_proto::materials::MaterialClass::Wood
                    | meld_proto::materials::MaterialClass::Stone
            ) {
                continue;
            }
            let art = std::path::Path::new("assets/props")
                .join(format!("resource_{}.png", m.key))
                .exists();
            assert!(
                art || SCENE_KEYS.contains(&m.key),
                "`{}` ({:?}) has neither a resource_{}.png billboard nor a scene — it would \
                 spawn NOTHING and be invisible in the world",
                m.key,
                m.class,
                m.key
            );
        }
    }
}

#[cfg(test)]
mod structure_art_tests {
    /// **Every buildable thing must have its own art.** The bug this closes had a wall and
    /// an anchor drawing as the same tinted `portal_arch` billboard a dungeon exit uses — so
    /// the whole player-building pillar rendered as "there is a portal here", and the two
    /// functions were indistinguishable.
    ///
    /// Read off the registry rather than a list, because the failure is adding a structure
    /// function and forgetting the art — and it fails SILENTLY, since the fallback still
    /// draws something you can walk up to.
    #[test]
    fn every_structure_function_has_its_own_kit_parts() {
        // Mirror of the keys built in `setup` (which needs a live AssetServer), plus the
        // model files they name — so a renamed .glb fails here rather than at runtime.
        const PARTS: &[(&str, &[&str])] = &[
            ("wall", &["wall-wood"]),
            ("anchor", &["pillar-stone", "wall-block-half"]),
        ];
        for def in meld_proto::structures::STRUCTURES {
            let entry = PARTS.iter().find(|(k, _)| *k == def.key);
            let Some((_, pieces)) = entry else {
                panic!(
                    "`{}` has no kit parts — it would fall back to the placeholder and be \
                     indistinguishable from every other structure",
                    def.key
                );
            };
            for piece in *pieces {
                let path = std::path::Path::new("assets/models/fantasy-town")
                    .join(format!("{piece}.glb"));
                assert!(path.exists(), "{} names `{piece}`, which is not in the kit", def.key);
            }
        }
    }

    /// A palisade is timber and an anchor is masonry (BD-1), and the ART has to agree — a
    /// stone-looking wall you paid wood for is a lie about the cost.
    #[test]
    fn the_art_matches_the_material_it_is_built_from() {
        use meld_proto::materials::MaterialClass;
        for def in meld_proto::structures::STRUCTURES {
            let looks_wooden = match def.key {
                "wall" => true,
                "anchor" => false,
                other => panic!("`{other}` has no art claim in this test"),
            };
            let is_wooden = def.material == MaterialClass::Wood;
            assert_eq!(
                looks_wooden, is_wooden,
                "`{}` is built from {:?} but its kit pieces read the other way",
                def.key, def.material
            );
        }
    }
}

#[cfg(test)]
mod weather_tests {
    use super::*;

    /// The phase machine tells a story — a breeze, then a gust announcing a storm, then the
    /// storm itself, then it eases off. That story is only true if the numbers rise and fall
    /// in that order, and one of them did not: the gust outblew the storm it was announcing.
    #[test]
    fn the_wind_builds_toward_the_downpour_and_eases_after_it() {
        let fair = wind_and_rain_targets(0, false).0;
        let gust = wind_and_rain_targets(1, false).0;
        let storm = wind_and_rain_targets(2, false).0;
        let super_storm = wind_and_rain_targets(2, true).0;
        let clearing = wind_and_rain_targets(3, false).0;

        assert!(fair > 0.0, "fair weather still moves leaves — a dead calm world reads as a still frame");
        assert!(gust > fair, "the gust must rise above the breeze ({gust} vs {fair})");
        assert!(
            storm > gust,
            "an ordinary storm must outblow its own precursor ({storm} vs gust {gust}) — \
             otherwise the weather peaks before the rain and eases as it lands"
        );
        assert!(super_storm >= storm, "a super storm blows at least as hard ({super_storm} vs {storm})");
        assert!(clearing < storm, "clearing eases off ({clearing} vs {storm})");
    }

    /// Only the storm phases bring rain, and the wind arrives BEFORE the water.
    #[test]
    fn the_wind_leads_the_rain() {
        assert_eq!(wind_and_rain_targets(1, false).1, 0.0, "the gust is dry — it only announces");
        assert_eq!(wind_and_rain_targets(2, false).1, 1.0, "the storm rains");
        assert_eq!(wind_and_rain_targets(3, false).1, 0.0, "clearing stops raining");
    }

    /// The amplitude table is multiplied by [`gust_response`], so neither number is the angle
    /// on its own — which is exactly how a tree shipped leaning one and a half degrees while
    /// its table entry looked reasonable. Assert the DEGREES, so what the comments promise is
    /// what the wind does.
    #[test]
    fn a_tree_leans_the_degrees_the_table_claims() {
        let tree = sway_amp("tree").expect("a tree sways");
        let fair = (tree * gust_response(wind_and_rain_targets(0, false).0)).to_degrees();
        let storm = (tree * gust_response(wind_and_rain_targets(2, true).0)).to_degrees();
        assert!(
            (3.5..4.5).contains(&fair),
            "fair weather should stir a tree about 4 degrees, got {fair:.1}"
        );
        assert!(
            (14.0..16.0).contains(&storm),
            "a super storm should toss a tree about 15 degrees, got {storm:.1}"
        );
        // And a cactus is a water tank on a stalk: it moves, barely.
        let cactus = sway_amp("cactus").expect("a cactus sways");
        assert!(cactus * 3.0 < tree, "a cactus should stay far stiffer than a tree");
        assert!(sway_amp("boulder").is_none(), "rock is rigid");
    }

    /// ⚠️ **A SPRITE'S OWN ORIGIN IS ITS CENTRE, SO A LEAN ABOUT IT SEE-SAWS.** The trunk
    /// swings out as far as the canopy and the tree stops touching the ground. `animate_sway`
    /// carries the quad's offset through the same rotation to pivot about the base instead;
    /// this is that arithmetic, checked rather than asserted in a comment.
    #[test]
    fn a_leaning_sprite_pivots_at_its_base_not_its_middle() {
        let pivot_y = 3.5_f32; // a 7-unit tree
        for lean in [0.07_f32, 0.26] {
            // `hd2d::billboard` has already put the camera-facing yaw here; take it as
            // identity (camera dead ahead) so the lean is the only thing moving anything.
            let q = Quat::IDENTITY * Quat::from_rotation_z(lean);
            let centre = q * Vec3::new(0.0, pivot_y, 0.0);
            // The quad's bottom edge sits `pivot_y` below its centre in its own frame.
            let base = centre + q * Vec3::new(0.0, -pivot_y, 0.0);
            assert!(
                base.length() < 1e-5,
                "the trunk left the ground at lean {lean}: {base:?}"
            );
        }
        // And the canopy genuinely travels, or the whole exercise renders as a still frame.
        let q = Quat::from_rotation_z(0.26);
        let travel = (q * Vec3::new(0.0, pivot_y, 0.0)).x.abs();
        assert!(travel > 0.8, "a full storm moved the canopy {travel:.2} units");
    }

    /// ⚠️ **A SWAYING KIND WITH NO SPRITE SILENTLY LOSES ITS SWAY.** `Sway` now lives on the
    /// billboard quad, so a kind that falls through to the 3D-model path gets none — which is
    /// the bug this replaces, where every swaying kind took the sprite path and the only
    /// `Sway` insertion sat on the model path nothing reached.
    #[test]
    fn every_swaying_kind_draws_as_a_sprite() {
        for (kind, sprite) in [
            ("tree", "obstacle_tree_pine"),
            ("cactus", "obstacle_cactus"),
            ("fungal_wall", "obstacle_fungal_wall"),
        ] {
            assert!(sway_amp(kind).is_some(), "{kind} should sway");
            assert!(
                PROP_KEYS.contains(&sprite),
                "`{kind}` sways, so it must draw as a sprite — `{sprite}` is missing from \
                 PROP_KEYS, which drops it onto the 3D-model path where nothing sways"
            );
        }
    }

    /// Both wind leans compose onto the yaw rather than assigning over it, and both are
    /// ordered so there is a yaw to compose onto. One rule, two call sites, held in one place.
    #[test]
    fn both_wind_leans_compose_after_the_billboard() {
        let here = include_str!("world_render.rs");
        assert!(
            here.contains("let q = tf.rotation * Quat::from_rotation_z(a);"),
            "the prop lean must post-multiply onto the billboard yaw"
        );
        assert!(
            here.contains("tf.translation = q * Vec3::new(0.0, s.pivot_y, 0.0);"),
            "the prop lean must carry the quad's offset through the rotation, or it see-saws"
        );
        let main = include_str!("main.rs");
        for sys in ["animate_sway.after(hd2d::BillboardSet)",
                    "ambient::update_ambient_scatter.after(hd2d::BillboardSet)"] {
            assert!(main.contains(sys), "`{sys}` must be ordered after the billboard pass");
        }
    }

    /// ⚠️ **A GRASS BLADE IS A BILLBOARD, AND TWO SYSTEMS WANT ITS ROTATION.**
    ///
    /// ⚠️ ORDER AGAINST `hd2d::BillboardSet`, NEVER AGAINST `hd2d::billboard` ITSELF.
    ///
    /// `billboard` is registered once per screen (City, Overworld, Battle), and Bevy's
    /// implicit `SystemTypeSet` for a system added more than once in one schedule is
    /// AMBIGUOUS — ordering against it panics at schedule init and the app never reaches
    /// its first frame. It shipped exactly that way and nothing caught it, because
    /// `cargo test` never builds the real `App`: the two assertions below were both green
    /// while `make play` died on boot. They check that the ordering EXISTS; they cannot
    /// check that it RESOLVES. This one checks the shape that made it resolvable.
    #[test]
    fn nothing_orders_against_the_billboard_system_itself() {
        let main = include_str!("main.rs");
        assert!(
            !main.contains(".after(hd2d::billboard)") && !main.contains(".before(hd2d::billboard)"),
            "order against `hd2d::BillboardSet`, not `hd2d::billboard` — the system is added \
             once per screen, so its implicit set is ambiguous and Bevy panics at schedule init"
        );
        // And every registration must actually be IN the set, or the ordering is vacuous:
        // it resolves against an empty set and silently orders against nothing.
        for line in main.lines() {
            let l = line.trim();
            if !l.contains("hd2d::billboard") || l.starts_with("//") || l.contains("after(") {
                continue;
            }
            assert!(
                l.contains("hd2d::billboard.in_set(hd2d::BillboardSet)"),
                "every `hd2d::billboard` registration must be `.in_set(hd2d::BillboardSet)`, \
                 or the ordering resolves against an empty set: {l}"
            );
        }
    }

    /// `hd2d::billboard` writes the camera-facing yaw; the grass lean in
    /// `ambient::update_ambient_scatter` writes the bend. Assigning in the second dropped the
    /// first, and with no ordering between them the winner changed frame to frame — grass
    /// snapping between flat-on and edge-on across the whole ground plane. The rule is
    /// COMPOSE, and it is only a comment at the call site, so hold it here.
    #[test]
    fn the_grass_lean_composes_onto_the_billboard_yaw() {
        let src = include_str!("ambient.rs");
        assert!(
            src.contains("tf.rotation *= Quat::from_rotation_z(lean)"),
            "the grass lean must post-multiply onto the yaw `hd2d::billboard` already wrote — \
             assigning `tf.rotation` there destroys the blade's facing"
        );
        assert!(
            !src.contains("tf.rotation = Quat::from_rotation_z(lean)"),
            "the grass lean is ASSIGNING its rotation again, which drops the billboard yaw"
        );
        // And the compose is only meaningful if the yaw is there to compose onto.
        let main = include_str!("main.rs");
        assert!(
            main.contains("ambient::update_ambient_scatter.after(hd2d::BillboardSet)"),
            "the grass scatter must be ordered after `hd2d::billboard`, or it composes onto \
             whatever last frame left behind"
        );
    }
}
