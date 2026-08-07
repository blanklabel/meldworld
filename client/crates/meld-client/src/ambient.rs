//! Client-side ambient ground life — purely decorative, no server entities, no
//! collision. A world-snapped scatter of grass/shrub/flower billboards grid-locked to
//! the ground around the player (so they read as fixed to the ground, not sliding
//! with the camera), gated to grassy biomes and thinned + size-varied by a per-cell
//! hash. (Drifting atmosphere motes/fireflies are handled separately by
//! `world_render::drift_motes`.)
//!
//! Grid trick: blade `idx` covers cell `base_cell + offset(idx)`; its world position /
//! variant are keyed on the *cell*, not the entity — so as the player moves, every
//! visible cell stays filled at its fixed spot and nothing visibly slides. A blade
//! only recomputes when the cell it covers changes (cheap most frames).

use bevy::prelude::*;

use crate::hd2d;
use crate::overworld::biome_display;
use crate::{Session, Terrain, WorldEntity};

const SPACING: f32 = 3.0;
const GRID: i32 = 15;
const N_BLADES: usize = (GRID * GRID) as usize;

#[derive(Component)]
pub(crate) struct GrassBlade {
    idx: usize,
    mat: Handle<StandardMaterial>,
    last: Option<(i32, i32)>,
}

impl GrassBlade {
    /// A blade with no cell assigned — enough for tests that only care that the pool
    /// can be hidden.
    #[cfg(test)]
    pub(crate) fn for_test() -> Self {
        Self { idx: 0, mat: Handle::default(), last: None }
    }
}

/// Grass variant sprites, shared with the scatter update so it can re-point a blade's
/// material at its cell's chosen variant.
#[derive(Resource)]
pub(crate) struct AmbientGrass(Vec<Handle<Image>>);

fn hash(mut x: u64) -> u32 {
    x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    x = (x ^ (x >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    ((x ^ (x >> 31)) & 0xFFFF_FFFF) as u32
}
fn cell_hash(cx: i32, cz: i32, salt: u32) -> u32 {
    hash(((cx as u32 as u64) << 32) ^ (cz as u32 as u64) ^ ((salt as u64) << 20))
}

fn biome_at(terrain: &Terrain, x: f32, z: f32) -> String {
    let r = ((x * x + z * z).sqrt()) as f64;
    terrain
        .sections
        .values()
        .find(|s| r >= s.start_x && r < s.end_x)
        .map(|s| s.biome.clone())
        .unwrap_or_else(|| biome_display(r.floor() as i64).to_string())
}
fn grassy(biome: &str) -> bool {
    matches!(biome, "forest" | "mire")
}

pub(crate) fn setup_ambient(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut mats: ResMut<Assets<StandardMaterial>>,
    assets: Res<AssetServer>,
) {
    let quad = meshes.add(hd2d::cyl_billboard_mesh(2.2, 2.2, 10, 55.0));
    let grass: Vec<Handle<Image>> = ["decor_grass_tuft", "decor_grass_shrub", "decor_grass_flower"]
        .iter()
        .map(|k| assets.load(format!("props/{k}.png")))
        .collect();
    for i in 0..N_BLADES {
        let mat = mats.add(hd2d::sprite_material(Color::WHITE, grass[i % grass.len()].clone()));
        commands.spawn((
            GrassBlade { idx: i, mat: mat.clone(), last: None },
            Mesh3d(quad.clone()),
            MeshMaterial3d(mat),
            Transform::default(),
            Visibility::Hidden,
            hd2d::Billboard,
        ));
    }
    commands.insert_resource(AmbientGrass(grass));
}

fn player_pos(
    session: &Session,
    q: &Query<(&WorldEntity, &Transform), Without<GrassBlade>>,
) -> Option<Vec3> {
    q.iter().find(|(we, _)| we.0 == session.player_id).map(|(_, t)| t.translation)
}

pub(crate) fn update_ambient_scatter(
    session: Res<Session>,
    terrain: Res<Terrain>,
    grass: Res<AmbientGrass>,
    mut mats: ResMut<Assets<StandardMaterial>>,
    players: Query<(&WorldEntity, &Transform), Without<GrassBlade>>,
    mut blades: Query<(&mut GrassBlade, &mut Transform, &mut Visibility)>,
) {
    let Some(p) = player_pos(&session, &players) else {
        for (_, _, mut v) in &mut blades {
            if !matches!(*v, Visibility::Hidden) {
                *v = Visibility::Hidden;
            }
        }
        return;
    };
    let bx = (p.x / SPACING).round() as i32;
    let bz = (p.z / SPACING).round() as i32;
    for (mut blade, mut tf, mut vis) in &mut blades {
        let dx = (blade.idx as i32 % GRID) - GRID / 2;
        let dz = (blade.idx as i32 / GRID) - GRID / 2;
        let cell = (bx + dx, bz + dz);
        if blade.last == Some(cell) {
            continue; // still covering the same cell — nothing to recompute
        }
        blade.last = Some(cell);
        let h = cell_hash(cell.0, cell.1, 1);
        let wx = cell.0 as f32 * SPACING + ((h & 0xFF) as f32 / 255.0 - 0.5) * SPACING * 0.9;
        let wz = cell.1 as f32 * SPACING + (((h >> 8) & 0xFF) as f32 / 255.0 - 0.5) * SPACING * 0.9;
        if (h % 100) < 55 || !grassy(&biome_at(&terrain, wx, wz)) {
            *vis = Visibility::Hidden;
            continue;
        }
        let variant = ((h >> 16) % grass.0.len() as u32) as usize;
        let scale = 0.85 + ((h >> 18) & 0x7F) as f32 / 127.0 * 0.8; // 0.85..1.65
        tf.translation = Vec3::new(wx, 0.02 + crate::world_render::terrain_height(wx, wz), wz);
        tf.scale = Vec3::splat(scale * 1.2 / 2.2);
        if let Some(m) = mats.get_mut(&blade.mat) {
            m.base_color_texture = Some(grass.0[variant].clone());
        }
        *vis = Visibility::Visible;
    }
}
