//! **Builder mode** (BD-9): aim, rotate, and drag out a run of pieces.
//!
//! The Map column's build row used to place a structure at your feet the instant you
//! clicked it — no preview, no aim, no way to lay a second piece against the first. It now
//! ARMS this tool instead: you point where it goes, `R` turns it, and dragging lays a line.
//!
//! ⚠️ **The server half had to come first, and it was a rules change rather than UX.** Every
//! piece of a dragged wall after the first was refused as `TooClose` — `min_spacing` (6.0)
//! forbade abutment outright — so drag-to-stretch was impossible, not unimplemented. Two
//! blocking structures now answer to `abut_spacing`, and because that retires the geometric
//! promise that a ring of walls always has a gap, placement asks `would_enclose_someone`
//! directly. See `Arena::place_structure_at`.
//!
//! **This module deliberately predicts almost nothing.** The server owns the rules — reach,
//! the clear path, spacing, enclosure, cost — and a client that re-derived them would be a
//! second copy that drifts, which is the failure this repo keeps meeting. So the ghost shows
//! WHERE pieces will go and how many, tinted only by the one thing the client legitimately
//! knows (whether the bag holds the right material), and a refusal arrives from the server
//! like any other.

use bevy::prelude::*;

use crate::net::ClientCmd;
use crate::{NetRes, RunBackpack};

/// Ground pitch between pieces of a dragged run, in world units.
///
/// ⚠️ **This must stay at or above `[building] abut_spacing`**, which is the server's
/// closest-legal distance between two blocking structures. Below it, every second piece of
/// a run comes back refused as `TooClose` and the wall has holes in it. The client cannot
/// read `balance.toml` at runtime (it ships without one), so the relationship is held by
/// `the_piece_pitch_is_at_least_the_servers_abut_spacing`, which parses the file in a test.
///
/// It is derived from the ART rather than the rule: a `wall-wood` panel is drawn at scale
/// 2.2, so pieces laid ~2 units apart read as a continuous fence.
pub(crate) const PIECE_PITCH: f32 = 2.0;

/// The most pieces one drag may lay. A bound rather than a taste: each piece is its own
/// build intent, its own cost check and its own structure, and a drag across the screen
/// would otherwise fire a hundred of them at the game loop in one frame.
pub(crate) const MAX_RUN: usize = 12;

#[derive(Default)]
pub(crate) struct Armed {
    /// Which structure the tool is holding.
    pub(crate) function: String,
    /// Facing in degrees; `R` turns it.
    pub(crate) yaw: f32,
    /// Where a drag began, in world xz. `None` while merely aiming.
    pub(crate) drag_from: Option<Vec2>,
}

/// Builder mode's whole state: armed with something, or not.
#[derive(Resource, Default)]
pub(crate) struct BuildMode {
    pub(crate) armed: Option<Armed>,
    /// Last aim point, so the ghost and the commit agree about where "here" is.
    pub(crate) aim: Option<Vec2>,
}

impl BuildMode {
    pub(crate) fn arm(&mut self, function: &str) {
        self.armed = Some(Armed { function: function.to_string(), yaw: 0.0, drag_from: None });
    }
    pub(crate) fn disarm(&mut self) {
        self.armed = None;
        self.aim = None;
    }
}

/// One translucent footprint marker.
#[derive(Component)]
pub(crate) struct GhostPiece;

/// Where the cursor meets the ground, in world xz.
///
/// The same ray-to-`y=0` intersection the overworld already uses to decide whether a click
/// landed near your feet — kept as a plane rather than the heightmap because the ghost is a
/// footprint, and a footprint that climbed hills would disagree with the server, which
/// places on xz.
fn cursor_ground(
    windows: &Query<&Window>,
    cams: &Query<(&Camera, &GlobalTransform)>,
) -> Option<Vec2> {
    let p = windows.iter().next()?.cursor_position()?;
    let (cam, tf) = cams.iter().next()?;
    let ray = cam.viewport_to_world(tf, p).ok()?;
    let dv = ray.direction.y;
    if dv.abs() < 1e-6 {
        return None;
    }
    let dist = -ray.origin.y / dv;
    if dist <= 0.0 {
        return None;
    }
    let hit = ray.get_point(dist);
    Some(Vec2::new(hit.x, hit.z))
}

/// The snapped run a drag from `from` to `to` describes: piece positions, nose to tail.
///
/// Snapping is what makes a run a WALL rather than a scatter — pieces land on a fixed pitch
/// along the drag, so they abut instead of overlapping or gapping. A drag shorter than one
/// pitch is a single piece, because arming the tool and clicking once must still build one
/// thing.
pub(crate) fn run_pieces(from: Vec2, to: Vec2) -> Vec<Vec2> {
    let delta = to - from;
    let len = delta.length();
    if len < PIECE_PITCH * 0.5 {
        return vec![from];
    }
    let dir = delta / len;
    let n = ((len / PIECE_PITCH).floor() as usize + 1).min(MAX_RUN);
    (0..n).map(|i| from + dir * (PIECE_PITCH * i as f32)).collect()
}

/// The yaw, in degrees, that lays a piece ALONG a drag — so a dragged wall faces the way it
/// runs rather than all pointing north.
pub(crate) fn run_yaw(from: Vec2, to: Vec2, fallback: f32) -> f32 {
    let d = to - from;
    if d.length() < PIECE_PITCH * 0.5 {
        return fallback;
    }
    d.x.atan2(d.y).to_degrees()
}

/// Keys and mouse for the armed tool: `R` turns, `Esc`/right-click puts it away, and
/// left-press-drag-release lays a run.
#[allow(clippy::too_many_arguments)]
pub(crate) fn builder_input(
    keys: Res<ButtonInput<KeyCode>>,
    buttons: Res<ButtonInput<MouseButton>>,
    windows: Query<&Window>,
    cams: Query<(&Camera, &GlobalTransform)>,
    mut mode: ResMut<BuildMode>,
    net: NonSend<NetRes>,
) {
    if mode.armed.is_none() {
        return;
    }
    if keys.just_pressed(KeyCode::Escape) || buttons.just_pressed(MouseButton::Right) {
        mode.disarm();
        return;
    }
    let aim = cursor_ground(&windows, &cams);
    mode.aim = aim;
    if keys.just_pressed(KeyCode::KeyR) {
        if let Some(a) = mode.armed.as_mut() {
            a.yaw = (a.yaw + 45.0) % 360.0;
        }
    }
    let Some(aim) = aim else { return };
    if buttons.just_pressed(MouseButton::Left) {
        if let Some(a) = mode.armed.as_mut() {
            a.drag_from = Some(aim);
        }
        return;
    }
    if buttons.just_released(MouseButton::Left) {
        // Commit: one intent per piece. The server prices, bounds and refuses each — this
        // is a request for a run, not a decision that one is legal.
        let (function, from, yaw) = {
            let Some(a) = mode.armed.as_ref() else { return };
            let from = a.drag_from.unwrap_or(aim);
            (a.function.clone(), from, run_yaw(from, aim, a.yaw))
        };
        for p in run_pieces(from, aim) {
            net.0.send(ClientCmd::BuildStructureAt {
                function: function.clone(),
                at: (p.x as f64, p.y as f64),
                yaw: yaw as f64,
            });
        }
        if let Some(a) = mode.armed.as_mut() {
            a.drag_from = None;
        }
    }
}

/// Redraw the ghost every frame the tool is armed: one translucent marker per piece.
///
/// Rebuilt rather than moved because the COUNT changes as you drag, and a pool that
/// sometimes has the wrong number of pieces in it is the kind of bug that shows up as a
/// phantom wall segment left behind.
pub(crate) fn draw_ghosts(
    mut commands: Commands,
    mode: Res<BuildMode>,
    backpack: Res<RunBackpack>,
    existing: Query<Entity, With<GhostPiece>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut mats: ResMut<Assets<StandardMaterial>>,
) {
    for e in &existing {
        commands.entity(e).despawn();
    }
    let (Some(a), Some(aim)) = (mode.armed.as_ref(), mode.aim) else { return };
    // The ONE thing the client legitimately knows about legality: is the right stock in the
    // bag. Everything else — reach, the trail, spacing, enclosure — is the server's, and
    // guessing at it here would be a second copy of the rules.
    let affordable = meld_proto::structures::structure(&a.function)
        .map(|def| crate::menu::carried_of_class(&backpack, def.material).is_some())
        .unwrap_or(false);
    let tint = if affordable {
        Color::srgba(0.45, 0.95, 0.55, 0.45)
    } else {
        Color::srgba(0.95, 0.35, 0.35, 0.40)
    };
    let mat = mats.add(StandardMaterial {
        base_color: tint,
        alpha_mode: AlphaMode::Blend,
        unlit: true,
        ..default()
    });
    let mesh = meshes.add(Cuboid::new(PIECE_PITCH * 0.9, 0.12, PIECE_PITCH * 0.35));
    let from = a.drag_from.unwrap_or(aim);
    let yaw = run_yaw(from, aim, a.yaw);
    for p in run_pieces(from, aim) {
        commands.spawn((
            GhostPiece,
            Mesh3d(mesh.clone()),
            MeshMaterial3d(mat.clone()),
            Transform::from_xyz(p.x, 0.08, p.y)
                .with_rotation(Quat::from_rotation_y(yaw.to_radians())),
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **The client's pitch must not be tighter than the server's closest-legal spacing**,
    /// or every second piece of a run comes back `TooClose` and the wall has holes. The
    /// client ships without `balance.toml`, so the relationship is checked here by reading
    /// the file rather than at runtime.
    #[test]
    fn the_piece_pitch_is_at_least_the_servers_abut_spacing() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../balance/balance.toml");
        let text = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));
        let abut: f32 = text
            .lines()
            .find_map(|l| l.strip_prefix("abut_spacing")?.split('=').nth(1))
            .and_then(|v| v.split('#').next())
            .and_then(|v| v.trim().parse().ok())
            .expect("balance.toml should declare abut_spacing");
        assert!(
            PIECE_PITCH >= abut,
            "pieces are laid {PIECE_PITCH} apart but the server's closest legal is {abut} — \
             every second piece of a run would be refused"
        );
    }

    #[test]
    fn a_click_without_a_drag_still_builds_exactly_one() {
        let p = Vec2::new(10.0, 4.0);
        assert_eq!(run_pieces(p, p), vec![p]);
        // A twitch of the mouse is still a click, not a two-piece run.
        assert_eq!(run_pieces(p, p + Vec2::new(0.2, 0.0)).len(), 1);
    }

    #[test]
    fn a_drag_lays_pieces_on_the_pitch_and_is_bounded() {
        let a = Vec2::ZERO;
        let b = Vec2::new(PIECE_PITCH * 5.0, 0.0);
        let run = run_pieces(a, b);
        assert_eq!(run.len(), 6, "five pitches should be six pieces: {run:?}");
        for w in run.windows(2) {
            assert!((w[1].distance(w[0]) - PIECE_PITCH).abs() < 1e-3, "off pitch: {run:?}");
        }
        // And a drag across the world does not fire a hundred intents at the game loop.
        let far = run_pieces(a, Vec2::new(PIECE_PITCH * 500.0, 0.0));
        assert_eq!(far.len(), MAX_RUN);
    }

    /// A dragged wall faces the way it runs — otherwise a run laid east still points north
    /// and reads as a row of gates rather than a fence.
    #[test]
    fn a_run_faces_along_its_drag() {
        let east = run_yaw(Vec2::ZERO, Vec2::new(10.0, 0.0), 0.0);
        let north = run_yaw(Vec2::ZERO, Vec2::new(0.0, 10.0), 0.0);
        assert!((east - 90.0).abs() < 1e-3, "east should be 90°, got {east}");
        assert!(north.abs() < 1e-3, "north should be 0°, got {north}");
        // Too short to infer a direction: keep whatever R last chose.
        assert_eq!(run_yaw(Vec2::ZERO, Vec2::new(0.1, 0.0), 33.0), 33.0);
    }
}
