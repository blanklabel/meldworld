//! Overworld: movement + camera, snapshot→sprite reconciliation, terrain/walls,
//! chests, HUD/minimap, party-follower entourage, and the perk overlays (lamp,
//! nameplates). Extracted from `main.rs` during the module reorg.

use std::collections::HashSet;

use bevy::prelude::*;
use bevy::input::mouse::{MouseMotion, MouseWheel};
use bevy::gltf::GltfAssetLabel;

use meld_client::hd2d::{self, CharSprite};
use meld_client::net::{ClientCmd, EntityKind};

use super::*;

// -------------------------------------------------------------- overworld --

/// An overworld action reachable by a keyboard key OR an on-screen (touch) button.
#[derive(Clone, Copy, PartialEq)]
pub(crate) enum OverworldAct {
    Extract,
    TownPortal,
    Join,
    /// Open the inventory/menu overlay (where distance, biome and the backpack now
    /// live). Keyboard equivalent: C / I, or tapping your own character.
    Menu,
}

/// Marks a tappable on-screen action button (touch-native via Bevy UI `Interaction`).
#[derive(Component)]
pub(crate) struct TouchActionButton(pub(crate) OverworldAct);

pub(crate) fn overworld_ui(mut commands: Commands) {
    commands
        .spawn((
            OverworldRoot,
            Node {
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                flex_direction: FlexDirection::Column,
                padding: UiRect::all(Val::Px(12.0)),
                ..default()
            },
        ))
        .with_children(|p| {
            p.spawn((
                HudText,
                Text::new("distance 0  -  Forest"),
                TextFont {
                    font_size: 20.0,
                    ..default()
                },
                TextColor(Color::srgb(0.9, 0.92, 1.0)),
            ));
            p.spawn((
                Text::new(
                    "WASD/arrows or drag = move | tap = go there | Menu (or tap yourself / C) = inventory, distance & stats | walk into nodes to harvest | T town portal | J join | E portal",
                ),
                TextFont {
                    font_size: 14.0,
                    ..default()
                },
                TextColor(Color::srgb(0.6, 0.65, 0.8)),
            ));
            // Touch action bar (bottom-right). Also clickable with the mouse.
            p.spawn((
                Node {
                    position_type: PositionType::Absolute,
                    right: Val::Px(14.0),
                    bottom: Val::Px(14.0),
                    flex_direction: FlexDirection::Column,
                    row_gap: Val::Px(8.0),
                    align_items: AlignItems::FlexEnd,
                    ..default()
                },
            ))
            .with_children(|bar| {
                // Only the Menu button shows on the overworld now — it opens the
                // inventory/stats overlay (also: tap yourself, or C/I). The situational
                // actions (Join / deep Portal / Town Portal) live INSIDE that menu (and
                // still have keyboard shortcuts J/E/T), so the field view stays clean.
                for (act, label) in [
                    (OverworldAct::Menu, "\u{f0214} Menu"), // list icon
                ] {
                    action_button(bar, act, label);
                }
            });
            // Virtual thumbstick (base ring + knob), shown only while dragging.
            p.spawn((
                JoystickBase,
                Node {
                    position_type: PositionType::Absolute,
                    width: Val::Px(120.0),
                    height: Val::Px(120.0),
                    border: UiRect::all(Val::Px(2.0)),
                    display: Display::None,
                    ..default()
                },
                BorderColor(Color::srgba(0.7, 0.8, 1.0, 0.5)),
                BorderRadius::all(Val::Percent(50.0)),
                BackgroundColor(Color::srgba(0.3, 0.4, 0.7, 0.15)),
            ));
            p.spawn((
                JoystickKnob,
                Node {
                    position_type: PositionType::Absolute,
                    width: Val::Px(56.0),
                    height: Val::Px(56.0),
                    display: Display::None,
                    ..default()
                },
                BorderRadius::all(Val::Percent(50.0)),
                BackgroundColor(Color::srgba(0.8, 0.88, 1.0, 0.55)),
            ));
            // Full-screen overlay that holds per-mob nameplates (Explorer/Psyker
            // intel), positioned in screen space by `update_mob_nameplates`.
            p.spawn((
                NameplateRoot,
                Node {
                    position_type: PositionType::Absolute,
                    width: Val::Percent(100.0),
                    height: Val::Percent(100.0),
                    ..default()
                },
            ));
            // Shifter corner minimap (top-right). Hidden until the perk unlocks it;
            // populated with dots by `update_minimap`.
            p.spawn((
                MinimapRoot,
                Node {
                    position_type: PositionType::Absolute,
                    right: Val::Px(14.0),
                    top: Val::Px(14.0),
                    width: Val::Px(140.0),
                    height: Val::Px(140.0),
                    border: UiRect::all(Val::Px(2.0)),
                    display: Display::None,
                    ..default()
                },
                BorderColor(Color::srgba(0.6, 0.8, 1.0, 0.5)),
                BorderRadius::all(Val::Px(6.0)),
                BackgroundColor(Color::srgba(0.05, 0.08, 0.14, 0.65)),
            ));
        });
}

/// Position + show/hide the thumbstick from the [`Joystick`] state (touch UI).
#[allow(clippy::type_complexity)]
pub(crate) fn joystick_visual(
    stick: Res<Joystick>,
    mut base: Query<&mut Node, (With<JoystickBase>, Without<JoystickKnob>)>,
    mut knob: Query<&mut Node, (With<JoystickKnob>, Without<JoystickBase>)>,
) {
    let active = stick.touch.is_some();
    if let Ok(mut b) = base.single_mut() {
        b.display = if active { Display::Flex } else { Display::None };
        if active {
            b.left = Val::Px(stick.origin.x - 60.0);
            b.top = Val::Px(stick.origin.y - 60.0);
        }
    }
    if let Ok(mut k) = knob.single_mut() {
        k.display = if active { Display::Flex } else { Display::None };
        if active {
            let off = (stick.cur - stick.origin).clamp_length_max(60.0);
            k.left = Val::Px(stick.origin.x + off.x - 28.0);
            k.top = Val::Px(stick.origin.y + off.y - 28.0);
        }
    }
}

/// Spawn one action button into the touch bar.
pub(crate) fn action_button(parent: &mut ChildSpawnerCommands, act: OverworldAct, label: &str) {
    parent
        .spawn((
            Button,
            TouchActionButton(act),
            Node {
                width: Val::Px(150.0),
                padding: UiRect::axes(Val::Px(14.0), Val::Px(11.0)),
                justify_content: JustifyContent::Center,
                border: UiRect::all(Val::Px(1.5)),
                ..default()
            },
            BorderColor(Color::srgb(0.4, 0.5, 0.8)),
            BorderRadius::all(Val::Px(8.0)),
            BackgroundColor(Color::srgba(0.08, 0.11, 0.22, 0.9)),
        ))
        .with_children(|b| {
            b.spawn((
                Text::new(label.to_string()),
                TextFont { font_size: 16.0, ..default() },
                TextColor(Color::srgb(0.88, 0.92, 1.0)),
            ));
        });
}

/// Handle taps/clicks on the overworld action buttons — same effects as the
/// keyboard shortcuts, so touch and keyboard are fully interchangeable.
#[allow(clippy::too_many_arguments)]
pub(crate) fn touch_action_buttons(
    q: Query<(&Interaction, &TouchActionButton), Changed<Interaction>>,
    net: NonSend<NetRes>,
    world: Res<Overworld>,
    session: Res<Session>,
    backpack: Res<RunBackpack>,
    mut overlay: ResMut<Overlay>,
    mut tab: ResMut<OverlayTab>,
) {
    for (interaction, btn) in &q {
        if *interaction != Interaction::Pressed {
            continue;
        }
        let me = world.entities.get(&session.player_id).map(|e| (e.x, e.y));
        match btn.0 {
            OverworldAct::Extract => net.0.send(ClientCmd::Extract),
            OverworldAct::TownPortal => {
                if backpack.count("town_portal") > 0 {
                    net.0.send(ClientCmd::TownPortal);
                }
            }
            OverworldAct::Join => {
                if near_fight(&world, me) {
                    net.0.send(ClientCmd::JoinBattle);
                }
            }
            OverworldAct::Menu => {
                // Toggle the overlay open to the Status tab, where distance/biome and
                // the run backpack now live (moved off the always-on HUD).
                if overlay.kind.is_some() {
                    overlay.kind = None;
                } else {
                    overlay.kind = Some(OverlayKind::Inventory);
                    *tab = OverlayTab::Status;
                }
            }
        }
    }
}

/// Display name of the biome band at a floored distance (client-side mirror of
/// the server's structural biome table — display only; the server stays
/// authoritative for what actually spawns).
pub(crate) fn biome_display(d: i64) -> &'static str {
    match d {
        0..=99 => "Forest",
        100..=299 => "Desert",
        300..=499 => "Ashfall",
        500..=999 => "Tundra",
        _ => "Mire",
    }
}

/// Server (x, y) → HD-2D world space: x east, **z = server y** (south, +Z toward
/// the camera parked behind the player). Y is up: `height` above the rolling ground,
/// which sits at `terrain_height(x, z)` — so everything placed through here rides the
/// continuous heightmap the ground shader displaces to (Phase A). Moving entities
/// (avatar/mobs) re-apply it per frame in the interp/camera systems.
pub(crate) fn world_pos(x: f32, y: f32, height: f32) -> Vec3 {
    Vec3::new(x, height + crate::world_render::terrain_height(x, y), y)
}

/// Exponential rate the rendered overworld positions chase the 20 Hz server
/// snapshots (higher = snappier + less smoothing). Kills the pixel-sprite jitter.
pub(crate) const OW_SMOOTH_RATE: f32 = 16.0;

/// Client render-unload radii (world units from the player). The server sends EVERY
/// entity in the instance each snapshot; as you dive deep that set grows without
/// bound. So the client only *renders* what's near: an entity beyond `_FAR` is
/// despawned (freeing its sprite atlas / model), and one is (re)spawned from the
/// snapshot once within `_NEAR` — the gap is hysteresis so nothing flickers at the
/// edge. Both radii sit BEYOND the fog wall (`Look::fog_end` ~118), so culling is
/// visually invisible; it purely bounds render + memory. This is a rendering concern
/// only — the server still tracks and simulates every creature regardless. The local
/// player and the deep portal (a landmark beacon) are never culled.
pub(crate) const RENDER_UNLOAD_NEAR: f32 = 120.0;
pub(crate) const RENDER_UNLOAD_FAR: f32 = 150.0;

/// Drive the HD-2D camera each frame: orbit-follow the player, push the live
/// `Look` post params into the camera, aim the sun, and recolour the ground to the
/// player's current biome. Replaces the old flat 2D `follow_camera`.
#[allow(clippy::type_complexity, clippy::too_many_arguments)]
/// Smoothed extra camera-eye height (above the orbit base) that keeps the player clear
/// of intervening hills/cliffs — see the terrain-aware lift in [`hd2d_follow`].
#[derive(Resource, Default)]
pub(crate) struct CamLift(pub f32);

pub(crate) fn hd2d_follow(
    session: Res<Session>,
    look: Res<hd2d::Look>,
    time: Res<Time>,
    mut cam_lift: ResMut<CamLift>,
    // Follow the player's *smoothed* transform (not the raw 20 Hz snapshot), so the
    // camera and the sprite move together — no relative jitter. Exclude the camera
    // and sun so this `&Transform` read is disjoint from their `&mut Transform`.
    players: Query<(&WorldEntity, &Transform), (Without<Camera3d>, Without<DirectionalLight>)>,
    mut cam_q: Query<
        (
            &mut Transform,
            &mut Projection,
            Option<&mut bevy::core_pipeline::bloom::Bloom>,
            Option<&mut bevy::core_pipeline::dof::DepthOfField>,
            Option<&mut bevy::pbr::DistanceFog>,
        ),
        With<Camera3d>,
    >,
) {
    let Some(pos) = players
        .iter()
        .find(|(we, _)| we.0 == session.player_id)
        .map(|(_, tf)| tf.translation)
    else {
        return;
    };
    // Rise with the player's terrace (pos.y already carries the smoothed elevation).
    let target = Vec3::new(pos.x, 1.0 + pos.y, pos.z);
    if let Ok((mut t, mut proj, bloom, dof, fog)) = cam_q.single_mut() {
        let mut cam = hd2d::camera_transform(&look, target, time.elapsed_secs());
        // TERRAIN-AWARE eye lift: the low HD-2D pitch means a hill or cliff-mesa BETWEEN
        // the camera and the player easily hides them (and on a slope the eye can sink
        // into the ground). Sample the heightmap along the horizontal eye→target line and
        // raise the eye just enough that the whole sightline clears every intervening
        // ridge — so the player is never buried. The clearance TAPERS to zero at the
        // player (whose feet are on the ground, so no lift is demanded there), and the
        // total lift is capped at ~50° pitch (sprites go edge-on past that) + smoothed
        // (rise fast so a wall never clips in, settle slowly so open ground doesn't bob).
        let eye = cam.translation;
        let base_y = eye.y;
        let mut want = base_y;
        const CLEAR: f32 = 2.5;
        const STEPS: i32 = 18;
        for k in 1..STEPS {
            let f = k as f32 / STEPS as f32;
            let px = eye.x + (target.x - eye.x) * f;
            let pz = eye.z + (target.z - eye.z) * f;
            let ground = crate::world_render::terrain_height(px, pz) + CLEAR * (1.0 - f);
            // eye.y solving `eye.y*(1-f) + f*target.y >= ground` for the ray to clear here.
            let need = (ground - f * target.y) / (1.0 - f);
            want = want.max(need);
        }
        let max_pitch = 50f32.to_radians().sin() - look.cam_pitch.to_radians().sin();
        let want_lift = (want - base_y).clamp(0.0, (look.cam_dist * max_pitch).max(0.0));
        let rate = if want_lift > cam_lift.0 { 8.0 } else { 2.5 };
        cam_lift.0 += (want_lift - cam_lift.0) * (rate * time.delta_secs()).min(1.0);
        cam.translation.y = base_y + cam_lift.0;
        cam.look_at(target, Vec3::Y);
        *t = cam;
        hd2d::apply_post(
            &look,
            &mut proj,
            bloom.map(|b| b.into_inner()),
            dof.map(|d| d.into_inner()),
            fog.map(|f| f.into_inner()),
        );
    }
    // The ground's biome is now painted by the `GroundBiome` shader from world
    // position (blended across boundaries), so there's no per-frame texture swap here.
}

/// Despawn any stray character sprite that has slipped into the overworld without a
/// valid owner — a `CharSprite` that is neither a snapshot-driven `WorldEntity` avatar
/// nor a [`PartyFollower`]. In the overworld those are the ONLY legitimate character
/// sprites, so anything else is a leftover from another screen (a battle hero or a
/// city avatar that raced past its `OnExit`/`OnEnter` cleanup on the transition
/// frame). Such a leftover never receives movement, so it stands frozen facing the
/// camera and reads as a "second sprite overlaying" the real one. The per-id
/// reconciler dedup can't catch it (it only dedups WorldEntity avatars by id); this
/// guard runs every overworld frame and removes it regardless of how it arrived.
pub(crate) fn cull_stray_avatars(
    mut commands: Commands,
    strays: Query<Entity, (With<CharSprite>, Without<WorldEntity>, Without<PartyFollower>)>,
) {
    for e in &strays {
        commands.entity(e).despawn();
    }
}

/// Roughly the server's `join_radius` — the client only shows the Join prompt /
/// accepts J within this of a fighting teammate; the server does the real check.
pub(crate) const JOIN_PROMPT_RADIUS: f32 = 9.0;

/// Is the player within join range of a teammate's ongoing fight?
pub(crate) fn near_fight(world: &Overworld, me: Option<(f32, f32)>) -> bool {
    let Some((mx, my)) = me else { return false };
    world
        .entities
        .values()
        .any(|e| e.battling && ((e.x - mx).powi(2) + (e.y - my).powi(2)).sqrt() <= JOIN_PROMPT_RADIUS)
}

/// The backbone route is NO LONGER drawn as a glowing "walkway" — that highlighted trail
/// (plus the old wide clear tube) read as a tutorial corridor. A traversable route still
/// exists, but you have to FIND it through the maze; the client keeps any legacy trail
/// discs cleared so none linger. (`draw_web_trails` still hints the branch network.)
pub(crate) fn draw_path_trail(
    mut commands: Commands,
    mut world_path: ResMut<WorldPath>,
    existing: Query<Entity, With<PathTrail>>,
) {
    for e in &existing {
        commands.entity(e).despawn();
    }
    world_path.drawn = true;
}

/// Draw the WEB of trails (disjoint branch/loop/spur edges) as fainter dotted trails
/// than the backbone, so the overworld reads as an interconnected maze of routes. Same
/// redraw-when-absent idea as [`draw_path_trail`]; each edge is dotted independently.
pub(crate) fn draw_web_trail(
    mut commands: Commands,
    mut world_web: ResMut<WorldWeb>,
    existing: Query<Entity, With<WebTrail>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut mats: ResMut<Assets<StandardMaterial>>,
) {
    if world_web.edges.is_empty() {
        return;
    }
    if world_web.drawn && !existing.is_empty() {
        return;
    }
    for e in &existing {
        commands.entity(e).despawn();
    }
    let disc = meshes.add(Circle::new(0.28));
    let mat = mats.add(StandardMaterial {
        base_color: Color::srgba(0.9, 0.86, 0.55, 0.14),
        emissive: LinearRgba::rgb(0.34, 0.3, 0.11),
        unlit: true,
        alpha_mode: AlphaMode::Blend,
        ..default()
    });
    let flat = Quat::from_rotation_x(-std::f32::consts::FRAC_PI_2);
    let step = 2.5_f32;
    for ((ax, ay), (bx, by)) in &world_web.edges {
        let seg = ((bx - ax).powi(2) + (by - ay).powi(2)).sqrt();
        let n = (seg / step).ceil().max(1.0) as i32;
        for i in 0..=n {
            let t = i as f32 / n as f32;
            let x = ax + (bx - ax) * t;
            let y = ay + (by - ay) * t;
            commands.spawn((
                WebTrail,
                Mesh3d(disc.clone()),
                MeshMaterial3d(mat.clone()),
                Transform::from_translation(world_pos(x, y, 0.14)).with_rotation(flat),
            ));
        }
    }
    world_web.drawn = true;
}

/// The always-on overworld HUD now shows ONLY contextual prompts. Distance, biome
/// and the run backpack moved off the HUD into the menu (Status tab — see
/// [`update_run_stats`] + the overlay); the view stays uncluttered. Kept here: only
/// the "join the fight" prompt. (Passive-perk hints like "Regen"/"Bulwark" were
/// dropped — the party always has a Resonant, so "Regen" was always on and read as a
/// stuck status badge cluttering the world map.)
pub(crate) fn update_overworld_hud(
    world: Res<Overworld>,
    session: Res<Session>,
    mut q: Query<&mut Text, With<HudText>>,
) {
    let Some(me) = world.entities.get(&session.player_id) else {
        return;
    };
    let me_pos = Some((me.x, me.y));
    if let Ok(mut t) = q.single_mut() {
        let mut parts: Vec<String> = Vec::new();
        if near_fight(&world, me_pos) {
            parts.push("\u{f0817} Press [J] to join the fight".into()); // crossed-swords marker
        }
        **t = parts.join("  -  ");
    }
}

/// Recompute the live exploration readouts (distance / tier / biome) into the
/// [`RunStats`] resource. Writes a field ONLY when its displayed value actually
/// changes, so the immediate-mode menu overlay's change-gate doesn't fire every
/// frame (movement is frozen while the overlay is open, so this is quiescent then).
pub(crate) fn update_run_stats(
    world: Res<Overworld>,
    session: Res<Session>,
    terrain: Res<Terrain>,
    mut stats: ResMut<RunStats>,
) {
    let Some(me) = world.entities.get(&session.player_id) else {
        return;
    };
    let d = (me.x * me.x + me.y * me.y).sqrt().floor() as i64;
    let tier = d / 100; // tier(d) = floor(d/100) — the CANON distance axis.
    // The biome label reads the ACTUAL section the player stands in (its radius ring),
    // so it agrees with the ground + the creatures — not the fixed distance bands.
    let r = d as f64;
    let biome = terrain
        .sections
        .values()
        .find(|s| r >= s.start_x && r < s.end_x)
        .map(|s| title_case(&s.biome))
        .unwrap_or_else(|| biome_display(d).to_string());
    if stats.distance != d {
        stats.distance = d;
    }
    if stats.tier != tier {
        stats.tier = tier;
    }
    if stats.biome != biome {
        stats.biome = biome;
    }
}

/// Keyboard-only overworld *actions* (E/T/H/J). Movement is device-agnostic in
/// [`gather_steer`] + [`emit_move`]; the touch bar mirrors these actions.
#[allow(clippy::too_many_arguments)]
pub(crate) fn overworld_input(
    keys: Res<ButtonInput<KeyCode>>,
    net: NonSend<NetRes>,
    autoplay: Res<Autoplay>,
    world: Res<Overworld>,
    session: Res<Session>,
    overlay: Res<Overlay>,
    backpack: Res<RunBackpack>,
    mut entered: Local<HashSet<String>>,
) {
    // No actions while a screen is open or while channeling an extraction.
    if overlay.kind.is_some() || session.channeling {
        return;
    }

    let me = world.entities.get(&session.player_id).map(|e| (e.x, e.y));
    // Nearest portal to the player (there is one per area now).
    let portal = match me {
        Some((mx, my)) => world
            .entities
            .values()
            .filter(|e| e.kind == EntityKind::Portal)
            .min_by(|a, b| {
                let da = (a.x - mx).powi(2) + (a.y - my).powi(2);
                let db = (b.x - mx).powi(2) + (b.y - my).powi(2);
                da.total_cmp(&db)
            })
            .map(|e| (e.x, e.y)),
        None => None,
    };
    let near_portal = match (me, portal) {
        (Some((mx, my)), Some((px, py))) => ((mx - px).powi(2) + (my - py).powi(2)).sqrt() <= 2.0,
        _ => false,
    };

    // Extract at the deep portal (E key, or autopilot once it arrives).
    if keys.just_pressed(KeyCode::KeyE) || (autoplay.0 && near_portal) {
        net.0.send(ClientCmd::Extract);
        return;
    }
    // Town Portal (T): the primary way out — spend a Town Portal item to extract
    // from anywhere.
    if keys.just_pressed(KeyCode::KeyT) && backpack.count("town_portal") > 0 {
        net.0.send(ClientCmd::TownPortal);
        return;
    }
    // Harvesting is automatic now (walk into a node → `auto_harvest`); no key.
    // Join a nearby fight (J): opt into a teammate's ongoing battle (never pulled
    // in automatically). The server re-checks range.
    if keys.just_pressed(KeyCode::KeyJ) && near_fight(&world, me) {
        net.0.send(ClientCmd::JoinBattle);
    }
    // Descend into a hand-designed dungeon by **walking into its entrance** — like
    // harvesting a node, entry is collision-based (WG-1/DG-6b). Touching the doorway
    // sends `run.enter_dungeon`; the server still pulls in teammates gathered at the
    // entrance for a co-op descent. `F` remains as an explicit fallback. `entered`
    // dedupes so we send once per entrance, not every frame while standing on it
    // (the reach is generous so you don't have to pixel-hunt the doorway).
    if let Some((mx, my)) = me {
        let touch = world
            .entities
            .iter()
            .filter(|(_, e)| e.kind == EntityKind::Entrance)
            .map(|(id, e)| (id.clone(), (e.x - mx).powi(2) + (e.y - my).powi(2)))
            .filter(|(_, d2)| *d2 <= 2.25) // ~1.5 tiles — collision reach
            .min_by(|a, b| a.1.total_cmp(&b.1));
        match touch {
            // `insert` is true the first frame we touch this doorway → send once.
            Some((eid, _)) if entered.insert(eid.clone()) || keys.just_pressed(KeyCode::KeyF) => {
                net.0.send(ClientCmd::EnterDungeon { entity_id: eid });
            }
            Some(_) => {} // still standing on an already-triggered doorway
            None => entered.clear(), // walked clear of every entrance → re-arm
        }
    }
}

/// Harvest resource nodes automatically the moment you walk within reach — so
/// "touching" a node picks it up (and tapping/clicking a distant node just walks
/// you there via tap-to-move, then this fires on arrival). `sent` dedupes so a node
/// isn't requested twice before the server removes it.
pub(crate) fn auto_harvest(
    net: NonSend<NetRes>,
    world: Res<Overworld>,
    session: Res<Session>,
    overlay: Res<Overlay>,
    mut sent: Local<HashSet<String>>,
) {
    if overlay.kind.is_some() || session.channeling {
        return;
    }
    let Some(me) = world.entities.get(&session.player_id) else {
        return;
    };
    for (id, e) in &world.entities {
        if e.kind == EntityKind::Resource
            && ((e.x - me.x).powi(2) + (e.y - me.y).powi(2)).sqrt() <= 2.0
            && !sent.contains(id)
        {
            net.0.send(ClientCmd::Harvest { entity_id: id.clone() });
            sent.insert(id.clone());
        }
    }
    sent.retain(|id| world.entities.contains_key(id)); // forget harvested/gone nodes
}

/// Distance in screen pixels from point `p` to the segment `a`–`b`.
pub(crate) fn seg_point_dist(p: Vec2, a: Vec2, b: Vec2) -> f32 {
    let ab = b - a;
    let len2 = ab.length_squared();
    let t = if len2 > 1e-6 {
        ((p - a).dot(ab) / len2).clamp(0.0, 1.0)
    } else {
        0.0
    };
    p.distance(a + ab * t)
}

/// Open the party + inventory menu (the old-school RPG screen) by **clicking or
/// tapping your own character**. Replaces the inventory key/button. A click is a
/// mouse press+release without a drag (drags orbit the camera); a tap is a touch on
/// the character. The hit-test is in SCREEN space against the sprite's projected
/// extent — the avatar is an upright billboard and the camera is tilted, so a flat
/// ground-plane hit lands well *behind* the sprite you actually clicked.
#[allow(clippy::too_many_arguments)]
pub(crate) fn overworld_click_menu(
    mouse: Res<ButtonInput<MouseButton>>,
    touches: Res<Touches>,
    windows: Query<&Window>,
    cam_q: Query<(&Camera, &GlobalTransform), With<Camera3d>>,
    world: Res<Overworld>,
    session: Res<Session>,
    look: Res<hd2d::Look>,
    net: NonSend<NetRes>,
    ui_hit: Query<&Interaction, With<Button>>,
    mut overlay: ResMut<Overlay>,
    mut inv: ResMut<InventoryData>,
    mut press: Local<Option<Vec2>>,
) {
    if overlay.kind.is_some() || session.channeling {
        return;
    }
    let win = windows.iter().next();
    // Gather a click point: a no-drag mouse click, or a touch tap.
    let mut point = None;
    if let Some(w) = win {
        if mouse.just_pressed(MouseButton::Left) {
            *press = w.cursor_position();
        }
        if mouse.just_released(MouseButton::Left) {
            if let (Some(p0), Some(p1)) = (*press, w.cursor_position()) {
                if p0.distance(p1) < 6.0 {
                    point = Some(p1);
                }
            }
            *press = None;
        }
    }
    for t in touches.iter_just_pressed() {
        point = Some(t.position());
    }
    let Some(p) = point else { return };
    if ui_hit.iter().any(|i| *i != Interaction::None) {
        return; // clicked a UI button, not the world
    }
    let Some((cam, cam_tf)) = cam_q.iter().next() else { return };
    let Some(me) = world.entities.get(&session.player_id) else { return };

    // Primary hit-test: project the sprite's vertical extent (feet→head) to the
    // screen and measure the click's pixel distance to that line. This matches the
    // billboard the player sees, regardless of camera tilt or zoom.
    let base_y = me.level as f32 * STEP_HEIGHT + crate::world_render::terrain_height(me.x, me.y);
    let feet_w = Vec3::new(me.x, base_y, me.y);
    let head_w = feet_w + Vec3::Y * (look.sprite_y * 2.0);
    let on_sprite = match (
        cam.world_to_viewport(cam_tf, feet_w).ok(),
        cam.world_to_viewport(cam_tf, head_w).ok(),
    ) {
        (Some(feet_s), Some(head_s)) => {
            // Tolerance scales with the sprite's on-screen height (bigger when
            // zoomed in) with a floor so a small distant sprite is still clickable.
            let radius = ((head_s - feet_s).length() * 0.6).max(40.0);
            seg_point_dist(p, feet_s, head_s) < radius
        }
        _ => false,
    };

    // Fallback: a click that raycasts to the ground right at the avatar's feet
    // still counts (covers extreme camera angles where projection is degenerate).
    let mut near_feet = false;
    if let Ok(ray) = cam.viewport_to_world(cam_tf, p) {
        let dv = ray.direction.y;
        if dv.abs() >= 1e-6 {
            let dist = -ray.origin.y / dv;
            if dist > 0.0 {
                let hit = ray.get_point(dist);
                near_feet = Vec2::new(hit.x, hit.z).distance(Vec2::new(me.x, me.y)) < 1.8;
            }
        }
    }

    if on_sprite || near_feet {
        overlay.kind = Some(OverlayKind::Inventory);
        inv.loaded = false;
        net.0.fetch_inventory();
    }
}

/// Server-frame (x = east, y = south) steering vector for this frame, filled by
/// keyboard, the virtual joystick, or tap-to-move — whichever is active. Consumed
/// by [`emit_move`]. Unifying here is what makes keyboard + touch interchangeable.
#[derive(Resource, Default)]
pub(crate) struct Steer(Vec2);

/// A tap-to-move destination in *server* coords; cleared on arrival or when the
/// player takes direct control (keyboard/joystick).
#[derive(Resource, Default)]
pub(crate) struct TapTarget(Option<Vec2>);

/// The active virtual-joystick touch: its id + on-screen origin + current point.
#[derive(Resource, Default)]
pub(crate) struct Joystick {
    touch: Option<u64>,
    origin: Vec2,
    cur: Vec2,
}

/// Markers for the on-screen thumbstick (base ring + knob).
#[derive(Component)]
pub(crate) struct JoystickBase;
#[derive(Component)]
pub(crate) struct JoystickKnob;

/// Collect this frame's movement from keyboard OR the virtual joystick OR a
/// tap-to-move target, into [`Steer`] (server frame). Priority: direct input
/// (keyboard/joystick) overrides and cancels any tap-to-move.
#[allow(clippy::too_many_arguments)]
pub(crate) fn gather_steer(
    keys: Res<ButtonInput<KeyCode>>,
    touches: Res<Touches>,
    autoplay: Res<Autoplay>,
    overlay: Res<Overlay>,
    session: Res<Session>,
    world: Res<Overworld>,
    windows: Query<&Window>,
    cam_q: Query<(&Camera, &GlobalTransform), With<Camera3d>>,
    ui_hit: Query<&Interaction, With<Button>>,
    mut steer: ResMut<Steer>,
    mut tap: ResMut<TapTarget>,
    mut stick: ResMut<Joystick>,
) {
    steer.0 = Vec2::ZERO;
    if overlay.kind.is_some() || session.channeling {
        stick.touch = None;
        tap.0 = None;
        return;
    }
    let win = windows.iter().next();
    let joy_zone = win.map(|w| Vec2::new(w.width() * 0.38, w.height())); // left ~third

    // Camera ground basis (server frame: x east, y south), so movement is
    // **camera-relative** — "up" walks the way the camera faces, not a fixed world
    // axis. Keeps the camera and movement married as you orbit.
    let (fwd, right) = cam_q
        .iter()
        .next()
        .map(|(_, tf)| {
            let f = Vec3::from(tf.forward());
            let r = Vec3::from(tf.right());
            (
                Vec2::new(f.x, f.z).normalize_or_zero(),
                Vec2::new(r.x, r.z).normalize_or_zero(),
            )
        })
        .unwrap_or((Vec2::new(0.0, -1.0), Vec2::new(1.0, 0.0)));

    // 1) Keyboard — forward/right in the camera's frame.
    let mut fwd_amt = 0.0;
    let mut right_amt = 0.0;
    if keys.pressed(KeyCode::KeyW) || keys.pressed(KeyCode::ArrowUp) { fwd_amt += 1.0; }
    if keys.pressed(KeyCode::KeyS) || keys.pressed(KeyCode::ArrowDown) { fwd_amt -= 1.0; }
    if keys.pressed(KeyCode::KeyA) || keys.pressed(KeyCode::ArrowLeft) { right_amt -= 1.0; }
    if keys.pressed(KeyCode::KeyD) || keys.pressed(KeyCode::ArrowRight) { right_amt += 1.0; }
    let mut mv = fwd * fwd_amt + right * right_amt;
    if autoplay.0 && !world_idle_flag() {
        mv += Vec2::new(1.0, 0.0); // demo walks world-east, camera-independent
    }
    if mv != Vec2::ZERO {
        steer.0 = mv;
        tap.0 = None;
        stick.touch = None;
        return;
    }

    // 2) Virtual joystick — a touch that began in the left zone. Window coords are
    // y-down, which is exactly the server frame (south positive), so no flip.
    if let Some(id) = stick.touch {
        match touches.get_pressed(id) {
            Some(t) => {
                stick.cur = t.position();
                let v = stick.cur - stick.origin; // screen px, y-down
                if v.length() > 4.0 {
                    // Camera-relative: up-drag walks the way the camera faces.
                    let m = (right * v.x + fwd * -v.y) / 60.0; // full tilt ≈ 60px
                    steer.0 = m.clamp_length_max(1.0);
                }
                tap.0 = None;
                return;
            }
            None => stick.touch = None, // released
        }
    }
    if let Some(zone) = joy_zone {
        for t in touches.iter_just_pressed() {
            let p = t.position();
            if p.x <= zone.x && p.y >= zone.y * 0.35 {
                stick.touch = Some(t.id());
                stick.origin = p;
                stick.cur = p;
                tap.0 = None;
                return;
            }
        }
    }

    // 3) Tap-to-move — a fresh tap on the world (not the joystick zone, not a UI
    // button) sets a destination; we steer toward it until we arrive.
    let ui_busy = ui_hit.iter().any(|i| *i != Interaction::None);
    if !ui_busy {
        if let (Some((cam, cam_tf)), Some(zone)) = (cam_q.iter().next(), joy_zone) {
            for t in touches.iter_just_pressed() {
                let p = t.position();
                if p.x > zone.x {
                    // Cast the tap through the 3D camera onto the ground plane
                    // (y=0); the hit's (x, z) are the server (x, y) coords.
                    if let Ok(ray) = cam.viewport_to_world(cam_tf, p) {
                        let dy = ray.direction.y;
                        if dy.abs() > 1e-6 {
                            let dist = -ray.origin.y / dy;
                            if dist > 0.0 {
                                let hit = ray.get_point(dist);
                                tap.0 = Some(Vec2::new(hit.x, hit.z));
                            }
                        }
                    }
                }
            }
        }
    }
    if let (Some(target), Some(me)) = (tap.0, world.entities.get(&session.player_id)) {
        let dir = target - Vec2::new(me.x, me.y);
        if dir.length() < 0.6 {
            tap.0 = None;
        } else {
            steer.0 = dir.normalize_or_zero();
        }
    }
}

/// Send `movement.move_intent` from [`Steer`] at a fixed cadence so walk speed is
/// frame-rate-independent (device-agnostic — keyboard and touch feed the same
/// path).
pub(crate) fn emit_move(
    steer: Res<Steer>,
    net: NonSend<NetRes>,
    time: Res<Time>,
    mut clock: ResMut<MoveClock>,
) {
    if steer.0 == Vec2::ZERO {
        clock.acc = 0.0;
        return;
    }
    let step = 1.0 / MOVE_INTENT_HZ;
    clock.acc = (clock.acc + time.delta_secs()).min(0.25);
    while clock.acc >= step {
        clock.acc -= step;
        net.0.send(ClientCmd::Move { dx: steer.0.x as f64, dy: steer.0.y as f64 });
    }
}


/// Reconcile sprites to the authoritative snapshot: spawn new entities, move
/// known ones, despawn the gone.
/// Reconcile the 3D overworld scene with the latest server snapshot: move entities
/// that persist, spawn newcomers as HD-2D visuals (billboard sprites for players,
/// lit primitives for monsters/portals/resources/terrain), and despawn the gone.
pub(crate) fn sync_overworld_sprites(
    mut commands: Commands,
    world: Res<Overworld>,
    session: Res<Session>,
    look: Res<hd2d::Look>,
    time: Res<Time>,
    wa: Option<Res<WorldAssets>>,
    mut mats: ResMut<Assets<StandardMaterial>>,
    mut interp: ResMut<OwInterp>,
    dungeon: Res<world_render::DungeonSceneRes>,
    mut q: Query<(Entity, &WorldEntity, &mut Transform)>,
) {
    let Some(wa) = wa else { return };
    let now = time.elapsed_secs();

    // Server snapshots arrive on the authoritative 100 ms tick (~10 Hz). When a
    // fresh one arrives, roll it into the interpolation buffer (shift current →
    // previous), so remote sprites can lerp between the two most recent samples.
    if interp.seen_seq != world.seq {
        interp.seen_seq = world.seq;
        for (id, e) in &world.entities {
            let cur = InterpSample { x: e.x, y: e.y, level: e.level as f32, t: now };
            interp
                .states
                .entry(id.clone())
                .and_modify(|(prev, c)| {
                    *prev = *c;
                    *c = cur;
                })
                .or_insert((cur, cur));
        }
        interp.states.retain(|id, _| world.entities.contains_key(id));
    }

    // The LOCAL player stays on responsive frame-rate-independent exponential
    // smoothing — the camera follows its transform (see `hd2d_follow`), so it must
    // not render a snapshot behind. Every OTHER entity renders ~one tick behind,
    // linearly interpolated between its two most recent snapshots: constant-velocity
    // smooth motion with no rubber-banding and no extrapolation overshoot.
    let k = 1.0 - (-time.delta_secs() * OW_SMOOTH_RATE).exp();
    let render_t = now - OW_INTERP_DELAY;
    // Render-unload bookkeeping (see RENDER_UNLOAD_*): cull entities far from the
    // player. The local player and the deep portal landmark are exempt. Positions come
    // from the snapshot (not the smoothed transform) — good enough within a tick.
    let me_pos = world.entities.get(&session.player_id).map(|e| (e.x, e.y));
    let my_id = session.player_id.clone();
    let exempt = |id: &str| id == my_id.as_str() || id == "portal";
    let dist_from_me = |x: f32, y: f32| me_pos.map(|(mx, my)| (x - mx).hypot(y - my));
    let mut seen = HashSet::new();
    for (entity, we, mut tf) in &mut q {
        let Some(e) = world.entities.get(&we.0) else {
            commands.entity(entity).despawn();
            continue;
        };
        // Render-unload: drop entities that have fallen far behind (past the fog wall)
        // so render + memory stay bounded as you dive deep. The server keeps tracking
        // and simulating them — this is purely what the client chooses to draw.
        if !exempt(&we.0) && dist_from_me(e.x, e.y).is_some_and(|d| d > RENDER_UNLOAD_FAR) {
            commands.entity(entity).despawn();
            continue;
        }
        // Idempotency guard: keep exactly one avatar per id. A rapid
        // Battle→Overworld round-trip could otherwise leave a second sprite for
        // the same id — it stops getting position updates and stands "frozen,
        // facing the camera" while the live one moves. Despawn any such extra.
        if !seen.insert(we.0.clone()) {
            commands.entity(entity).despawn();
            continue;
        }
        // Horizontal (xz) is smoothed/interpolated for fluid motion; the VERTICAL
        // (elevation) is always SNAPPED to the discrete terrace level. You only ever
        // change level by stepping onto a connector, so there is no in-between height
        // to smooth toward — and smoothing it (STEP_HEIGHT = 2.0 per level, ramped
        // over ~10 frames) briefly left the avatar root *below* a terrace it had just
        // stepped onto, so the raised ground clipped the billboard's lower half and
        // the hero looked buried to the thigh until y caught up. (Y is set after xz
        // below, so it can read the rolling-ground height under the updated position.)
        if we.0 == session.player_id {
            // Responsive: chase the latest snapshot directly.
            tf.translation.x += (e.x - tf.translation.x) * k;
            tf.translation.z += (e.y - tf.translation.z) * k;
        } else if let Some((prev, cur)) = interp.states.get(&we.0) {
            // Interpolate between the two most recent samples at the delayed clock.
            let denom = cur.t - prev.t;
            let f = if denom > 1e-4 {
                ((render_t - prev.t) / denom).clamp(0.0, 1.0)
            } else {
                1.0
            };
            tf.translation.x = prev.x + (cur.x - prev.x) * f;
            tf.translation.z = prev.y + (cur.y - prev.y) * f;
        } else {
            // Just appeared (no buffer yet): snap to its latest position.
            tf.translation.x = e.x;
            tf.translation.z = e.y;
        }
        // Ride the rolling ground: discrete terrace level + the continuous heightmap
        // under the just-updated xz. Matches `world_pos` so spawn and per-frame agree.
        tf.translation.y = e.level as f32 * STEP_HEIGHT
            + crate::world_render::terrain_height(tf.translation.x, tf.translation.z);
    }
    for (id, e) in &world.entities {
        if seen.contains(id) {
            continue;
        }
        // Render-unload: hold off spawning far entities until they're within NEAR (the
        // NEAR/FAR gap is hysteresis, so nothing flickers at the boundary).
        if !exempt(id) && dist_from_me(e.x, e.y).is_some_and(|d| d > RENDER_UNLOAD_NEAR) {
            continue;
        }
        match e.kind {
            EntityKind::Player => {
                // We only know the local player's lead class (from their party);
                // remote avatars fall back to the Explorer.
                let lead = session.party.first().map(|s| s.as_str()).unwrap_or("explorer");
                spawn_player_avatar(
                    &mut commands,
                    &mut mats,
                    &wa,
                    &look,
                    id,
                    e,
                    &session.player_id,
                    lead,
                );
            }
            EntityKind::Monster => {
                // Pick the creature's billboard by normalized kind (shared with the
                // battle arena so the same creature looks the same in both). Tinted
                // faintly warm (like heroes) to stay vibrant under the cool ambient;
                // a fighting creature glows hot.
                let tex = creature_sprite(&wa, e.name.as_deref().unwrap_or(""));
                let base = if e.battling {
                    Color::srgb(1.4, 0.75, 0.55)
                } else {
                    Color::srgb(1.2, 1.15, 1.1)
                };
                // Nudge the (bright) tint faintly toward the faction hue so a clan of
                // creatures still reads as belonging together, as the old colours did.
                let tint = match (&e.faction, e.battling) {
                    (Some(f), false) => {
                        let (b, fc) = (base.to_srgba(), faction_color(f).to_srgba());
                        let k = 0.2;
                        Color::srgb(
                            b.red * (1.0 - k) + fc.red * 1.5 * k,
                            b.green * (1.0 - k) + fc.green * 1.5 * k,
                            b.blue * (1.0 - k) + fc.blue * 1.5 * k,
                        )
                    }
                    _ => base,
                };
                // FS-4: elites and gatekeepers read at a glance — bigger and menacingly
                // tinted (a gatekeeper towers; an elite is a hot-glowing champion).
                let (size, tint) = match e.encounter_class.as_deref() {
                    Some("gatekeeper") => (1.6 * 2.2, Color::srgb(1.7, 0.45, 0.5)),
                    Some("elite") => (1.6 * 1.4, Color::srgb(1.5, 0.8, 0.55)),
                    _ => (1.6, tint),
                };
                spawn_billboard_entity(&mut commands, &mut mats, &wa, id, e, tex, size, tint, 0.55);
            }
            EntityKind::Portal => {
                // The stone-gateway billboard, plus a faint emissive ground ring so
                // it still reads as a glowing exit at a distance.
                spawn_billboard_entity(
                    &mut commands,
                    &mut mats,
                    &wa,
                    id,
                    e,
                    wa.portal_sprite.clone(),
                    3.0,
                    Color::srgb(1.2, 1.2, 1.3),
                    0.0,
                );
                commands.spawn((
                    WorldEntity(id.clone()),
                    Mesh3d(wa.portal_mesh.clone()),
                    MeshMaterial3d(wa.portal_mat.clone()),
                    Transform::from_translation(world_pos(e.x, e.y, 0.08))
                        .with_rotation(Quat::from_rotation_x(std::f32::consts::FRAC_PI_2)),
                ));
            }
            EntityKind::Resource => {
                // A real 3D harvest-node model that draws the eye by slowly pulsing
                // its own emissive glow (`pulse_collectibles`) — no ground disc.
                let kind = e.name.as_deref().unwrap_or("");
                // Prefer the bespoke HD-2D harvest-node billboard (PixelLab); fall back
                // to the 3D model for any unmapped kind.
                if let Some(tex) = wa.prop_sprites.get(&format!("resource_{kind}")) {
                    let mat = mats.add(hd2d::sprite_material(Color::WHITE, tex.clone()));
                    commands
                        .spawn((
                            WorldEntity(id.clone()),
                            Collectible,
                            Transform::from_translation(world_pos(e.x, e.y, 0.0)),
                            Visibility::default(),
                        ))
                        .with_children(|p| {
                            p.spawn((
                                Mesh3d(wa.sprite_quad.clone()),
                                MeshMaterial3d(mat),
                                Transform::from_xyz(0.0, 0.85, 0.0).with_scale(Vec3::splat(1.7 / 2.2)),
                                hd2d::Billboard,
                            ));
                        });
                } else if let Some((scene, scale)) = wa.resource_scenes.get(kind) {
                    let yaw = (hash_pick(id, 360) as f32).to_radians();
                    commands.spawn((
                        WorldEntity(id.clone()),
                        Collectible,
                        SceneRoot(scene.clone()),
                        Transform::from_translation(world_pos(e.x, e.y, 0.0))
                            .with_scale(Vec3::splat(*scale))
                            .with_rotation(Quat::from_rotation_y(yaw)),
                    ));
                }
            }
            EntityKind::Loot => {
                // A dropped skirmish trophy — an HD-2D gold-pile pickup billboard
                // (PixelLab) until a player walks over it.
                let tex = wa
                    .prop_sprites
                    .get("item_gold_pile")
                    .cloned()
                    .unwrap_or_default();
                let mat = mats.add(hd2d::sprite_material(Color::WHITE, tex));
                commands
                    .spawn((
                        WorldEntity(id.clone()),
                        Collectible,
                        Transform::from_translation(world_pos(e.x, e.y, 0.0)),
                        Visibility::default(),
                    ))
                    .with_children(|p| {
                        p.spawn((
                            Mesh3d(wa.sprite_quad.clone()),
                            MeshMaterial3d(mat),
                            Transform::from_xyz(0.0, 0.5, 0.0).with_scale(Vec3::splat(1.0 / 2.2)),
                            hd2d::Billboard,
                        ));
                    });
            }
            EntityKind::Obstacle => {
                let theme = if dungeon.active { dungeon.theme.as_str() } else { "" };
                spawn_obstacle(&mut commands, &mut mats, &wa, id, e, theme);
            }
            // Chests are static and change look when opened — a dedicated
            // reconciler (`sync_chests`) owns them, not the generic sprite path.
            EntityKind::Chest => {}
            EntityKind::Entrance => {
                // A hand-designed dungeon entrance (WG-1/DG-6b): the stone gateway,
                // but tinted a glowing violet (vs the exit portal's cool blue) so it
                // reads as an ominous "descend here" doorway, distinct from a player.
                spawn_billboard_entity(
                    &mut commands,
                    &mut mats,
                    &wa,
                    id,
                    e,
                    wa.portal_sprite.clone(),
                    3.2,
                    Color::srgb(0.85, 0.45, 1.25),
                    0.45,
                );
            }
        }
    }
}

/// Slowly pulse the emissive glow of every [`Collectible`] (harvest node + ground
/// loot) so pickups draw the eye without a flat disc on the ground that z-fights the
/// grass. Drives the item's own material(s): a GLB harvest node keeps its meshes in a
/// child scene (walk descendants), while a loot nub carries the material directly.
pub(crate) fn pulse_collectibles(
    time: Res<Time>,
    roots: Query<Entity, With<Collectible>>,
    child_q: Query<&Children>,
    mat_of: Query<&MeshMaterial3d<StandardMaterial>>,
    mut mats: ResMut<Assets<StandardMaterial>>,
) {
    // ~2.5 s breathe; `strength` scales each material's own colour into its emissive,
    // so a blue gem glows blue and a gold trophy glows gold.
    let phase = (time.elapsed_secs() * std::f32::consts::TAU * 0.4).sin() * 0.5 + 0.5;
    let strength = 0.5 + 2.2 * phase;
    for root in &roots {
        for e in std::iter::once(root).chain(child_q.iter_descendants::<Children>(root)) {
            let Ok(mm) = mat_of.get(e) else { continue };
            let Some(m) = mats.get_mut(&mm.0) else {
                continue;
            };
            let c = m.base_color.to_linear();
            m.emissive = LinearRgba::rgb(c.red * strength, c.green * strength, c.blue * strength);
        }
    }
}

/// Biome cliff/rock tone for the boulder-ridge walls (indexed by biome).
pub(crate) fn biome_rock_color(bi: usize) -> Color {
    match bi {
        1 => Color::srgb(0.66, 0.53, 0.33), // Desert — sandstone
        2 => Color::srgb(0.24, 0.18, 0.17), // Ashfall — dark basalt
        3 => Color::srgb(0.74, 0.82, 0.92), // Tundra — pale ice/snow rock
        4 => Color::srgb(0.30, 0.36, 0.30), // Mire — mossy stone (rare; mire uses water)
        _ => Color::srgb(0.44, 0.48, 0.42), // Forest — grey-green cliff (also fallback)
    }
}

/// Spawn one biome-appropriate boundary prop at world (x, y), tagged [`WorldWall`]
/// (so the snapshot sync leaves it alone). Reuses the world's own art: a painterly
/// treeline in the forest, a rugged boulder ridge elsewhere, water in the mire —
/// so the border looks like natural geography, not a slab.
#[allow(clippy::too_many_arguments)]
pub(crate) fn spawn_wall_prop(
    commands: &mut Commands,
    wa: &WorldAssets,
    mats: &mut Assets<StandardMaterial>,
    rock_mats: &[Handle<StandardMaterial>],
    bi: usize,
    x: f32,
    y: f32,
    idx: usize,
) {
    let id = format!("wall-{idx}");
    match bi {
        0 => {
            // Forest → a dense treeline of HD-2D tree billboards — the SAME PixelLab
            // sprites the playfield trees use — so the border reads as pixel-art like
            // the rest of the world instead of clashing smooth 3D Kenney models.
            // Variant + height vary per id so the canopy line is layered, not stamped.
            const TREE_VARIANTS: [&str; 6] = [
                "obstacle_tree", "obstacle_tree_pine", "obstacle_tree_birch",
                "obstacle_tree_dead", "obstacle_tree_willow", "obstacle_tree_bushy",
            ];
            let pool: Vec<Handle<Image>> = TREE_VARIANTS
                .iter()
                .filter_map(|k| wa.prop_sprites.get(*k).cloned())
                .collect();
            if !pool.is_empty() {
                let tex = pool[hash_pick(&id, pool.len())].clone();
                // Per-id height 4.0..7.5 → a varied, layered wall of trees.
                let vf = 0.85 + (hash_pick(&id, 100) as f32 / 100.0) * 0.9;
                let height = (5.2 * vf).clamp(4.0, 7.5);
                let mat = mats.add(hd2d::sprite_material(Color::WHITE, tex));
                commands
                    .spawn((
                        WorldWall,
                        Transform::from_translation(world_pos(x, y, 0.0)),
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
                        p.spawn((
                            Mesh3d(wa.shadow_mesh.clone()),
                            MeshMaterial3d(wa.shadow_mat.clone()),
                            Transform::from_xyz(0.0, 0.02, 0.0)
                                .with_rotation(Quat::from_rotation_x(-std::f32::consts::FRAC_PI_2))
                                .with_scale(Vec3::new(height * 0.28, height * 0.28 * 0.55, height * 0.28)),
                        ));
                    });
            } else {
                // Fallback: a rugged rock if the tree sprites failed to load.
                let mat = rock_mats.first().cloned().unwrap_or_default();
                let s = 3.2 + (hash_pick(&id, 24) as f32) * 0.08;
                commands.spawn((
                    WorldWall,
                    Mesh3d(wa.rock_mesh.clone()),
                    MeshMaterial3d(mat),
                    Transform::from_translation(world_pos(x, y, 0.24 * s))
                        .with_scale(Vec3::splat(s * 0.9)),
                ));
            }
        }
        4 => {
            // Mire → a border of animated bog water blobs.
            let spin = (hash_pick(&id, 360) as f32).to_radians();
            commands.spawn((
                WorldWall,
                Mesh3d(wa.water_mesh.clone()),
                MeshMaterial3d(wa.water_mat("bog_pool")),
                Transform::from_translation(world_pos(x, y, 0.2))
                    .with_rotation(
                        Quat::from_rotation_y(spin)
                            * Quat::from_rotation_x(-std::f32::consts::FRAC_PI_2),
                    )
                    .with_scale(Vec3::splat(3.4)),
            ));
        }
        _ => {
            // Desert / Ashfall / Tundra → a rugged boulder-cliff ridge.
            let mat = rock_mats.get(bi).cloned().unwrap_or_default();
            let s = 3.2 + (hash_pick(&id, 24) as f32) * 0.08; // 3.2–5.1, varied
            commands.spawn((
                WorldWall,
                Mesh3d(wa.rock_mesh.clone()),
                MeshMaterial3d(mat),
                Transform::from_translation(world_pos(x, y, 0.24 * s))
                    .with_scale(Vec3::splat(s * 0.9)),
            ));
        }
    }
}

/// Build the map's framing, STREAMING with the endless world (#29): as each
/// terrain section arrives it hugs that section's ±lateral edges with the biome
/// border (treeline / boulder ridge / water), so "you can't leave the corridor"
/// holds for the whole run — not just the starting chunk. The west end-cap
/// (behind the hub) and the initial biome-seam gates are built once when the
/// run's bounds arrive. No east cap — the world streams on forever.
pub(crate) fn build_world_walls(
    mut commands: Commands,
    frame: Res<WorldFrame>,
    terrain: Res<Terrain>,
    wa: Option<Res<WorldAssets>>,
    assets: Res<AssetServer>,
    existing: Query<Entity, With<WorldWall>>,
    _mats: ResMut<Assets<StandardMaterial>>,
    mut walled: Local<std::collections::HashSet<u32>>,
) {
    let Some(_wa) = wa else { return };
    if !frame.have {
        return;
    }
    // A fresh run (new bounds) wipes the old framing + per-section tracking.
    let new_run = frame.is_changed();
    // WorldWall entities are despawned on OnExit(Overworld) — e.g. every time you
    // enter a battle. On return `new_run` is false and `walled` still marks every
    // section done, so WITHOUT this nothing would rebuild — including the Last City
    // gate, leaving an INVISIBLE return border you extract through by accident. So
    // also rebuild when the walls are gone but terrain exists (mirrors the path trail).
    let wiped = existing.is_empty() && !terrain.sections.is_empty();
    let rebuild = new_run || wiped;
    if rebuild {
        for e in &existing {
            commands.entity(e).despawn();
        }
        walled.clear();
    }
    // Sections still needing edge walls (grows as the world streams east).
    let mut todo: Vec<u32> = terrain
        .sections
        .keys()
        .copied()
        .filter(|i| !walled.contains(i))
        .collect();
    if !rebuild && todo.is_empty() {
        return;
    }
    todo.sort_unstable();

    // Biome-seam gates (a ridge across the corridor with one gap you funnel through,
    // flanked by standing-stone posts) + the Last City gate. Rebuilt whenever the
    // framing is (re)built — including on return from a battle — so the city is always
    // visible before its return border (never an invisible extraction trap).
    if rebuild {
        // Biome boundaries no longer WALL the world with a line of props + one gap — that
        // full-width barrier (a line of trees in a forest, funnelling you through a single
        // slit) was the "corridor". Biomes now just cross-fade on the ground (the shader)
        // and a Gatekeeper still stands near the boundary as a milestone fight you can
        // choose to round. `frame.seams` is left in the wire for that boss + the biome
        // marker; nothing walls it.

        // Last City's WALL + GATE + skyline, built from real Kenney castle models
        // (Pirate Kit, CC0) rather than scaled boxes. A stone rampart runs across the
        // western return border with a central gatehouse; behind it, towers + rooftops
        // read as the city itself — mostly glimpsed THROUGH the open gate as you
        // approach. Crossing west of `west_return_border` returns you; the gate marks
        // the line. The seeded terrain is no longer flat at y=0, so the whole city is set
        // on ONE terrain height (at the gate) — grounded so it never floats in the sky,
        // and rigid (a single base so the wall/towers don't step across the rolling ground).
        let wx = frame.west_return_border;
        let arc_deg = frame.radial_arc_degrees;
        let city_base_y = crate::world_render::terrain_height(wx, 0.0);
        let prop = |commands: &mut Commands, path: &str, x: f32, z: f32, yaw: f32, scale: f32| {
            commands.spawn((
                WorldWall,
                SceneRoot(assets.load(GltfAssetLabel::Scene(0).from_asset(format!("models/{path}.glb")))),
                Transform::from_xyz(x, city_base_y, z)
                    .with_rotation(Quat::from_rotation_y(yaw.to_radians()))
                    .with_scale(Vec3::splat(scale)),
            ));
        };
        // castle-wall renders ~2 units wide at scale 1 → ~7 wide at scale 3.5.
        const WALL_SCALE: f32 = 3.5;
        const SEG_W: f32 = 6.5;
        // A STRAIGHT rampart facing the hub with a central gateway you walk THROUGH —
        // so the doorway is legible head-on. `wall_half` bounds how far ±z the wall
        // runs: MODERATE in the radial fan (a clear gate that neither spans the whole
        // field like the old ±44 wall nor wraps around the approach like the wedge-arc
        // did — and stays off the fan's creatures, which top out well outside it), and
        // full in a flat corridor. The gate sits due-west on the return line; the
        // skyline is set straight back so it's glimpsed through the open door.
        let wall_half: f32 = if arc_deg > 0.0 { 14.0 } else { 44.0 };
        let behind = if wx < 0.0 { -1.0_f32 } else { 1.0 };
        let wall_yaw = 90.0_f32; // segments run north–south along world z
        let gate_yaw = if wx < 0.0 { 90.0_f32 } else { 270.0 }; // gatehouse faces the player
        const GATE_HALF: f32 = 7.0; // half-width of the central gateway
        let mut z = -wall_half;
        while z <= wall_half {
            if z.abs() > GATE_HALF {
                prop(&mut commands, "pirate/castle-wall", wx, z, wall_yaw, WALL_SCALE);
            }
            z += SEG_W;
        }
        // Gatehouse dead-centre (the doorway) + two flanking towers with pennants.
        prop(&mut commands, "pirate/castle-gate", wx, 0.0, gate_yaw, WALL_SCALE);
        for tz in [-(GATE_HALF + 1.0), GATE_HALF + 1.0] {
            prop(&mut commands, "pirate/tower-complete-large", wx, tz, gate_yaw, 3.5);
            prop(&mut commands, "pirate/flag-high", wx, tz, gate_yaw, 3.5);
        }
        // A tower capping each end of the rampart.
        for tz in [-wall_half + 2.0, wall_half - 2.0] {
            prop(&mut commands, "pirate/tower-complete-small", wx, tz, gate_yaw, 3.0);
        }
        // City skyline set straight back behind the gate — seen through the doorway.
        let city: &[(&str, f32, f32, f32, f32)] = &[
            ("pirate/tower-complete-large", 10.0, 0.0, 0.0, 4.0),
            ("pirate/tower-complete-small", 9.0, -6.0, 0.0, 3.0),
            ("pirate/tower-complete-small", 9.0, 6.0, 0.0, 3.0),
            ("pirate/tower-watch", 16.0, -5.0, 0.0, 3.5),
            ("pirate/tower-watch", 16.0, 5.0, 0.0, 3.5),
            ("pirate/tower-complete-large", 21.0, 1.0, 0.0, 4.5),
        ];
        for (path, back, cz, yaw, scale) in city {
            prop(&mut commands, path, wx + behind * back, *cz, *yaw, *scale);
        }
    }

    // Per-section EDGE WALLS removed. These ran a 5-7 rank thick band of props down both
    // lateral sides of every section ("thick enough to fully occlude the distance") — near
    // the hub, where the radial fan is narrow, those bands crowded right up to the player
    // and enclosed the start in a walled channel: THE corridor. The play area is bounded by
    // a soft clamp at ±lateral; it doesn't need a prop wall drawing that boundary. The world
    // now opens to the horizon (distant backdrop skyline + fog give the depth the wall used
    // to fake). `spawn_wall_prop` stays for the (retired) seam gates / any future use.
    for sidx in todo {
        walled.insert(sidx);
    }
}

/// Reconcile treasure-chest visuals from the snapshot: spawn a chest when it
/// first appears, re-spawn it (opened look) when it's opened, and despawn it if
/// it leaves the world. Chests are few and static, so this owns them directly
/// rather than the smoothed sprite path.
pub(crate) fn sync_chests(
    mut commands: Commands,
    world: Res<Overworld>,
    existing: Query<(Entity, &ChestEntity)>,
    _meshes: ResMut<Assets<Mesh>>,
    mut mats: ResMut<Assets<StandardMaterial>>,
    wa: Res<WorldAssets>,
) {
    use std::collections::HashSet;
    let mut present: HashSet<String> = HashSet::new();
    for (entity, ce) in &existing {
        match world.entities.get(&ce.id) {
            Some(e) if e.kind == EntityKind::Chest && e.opened == ce.opened => {
                present.insert(ce.id.clone()); // up to date, keep
            }
            _ => commands.entity(entity).despawn(), // gone or opened-state changed → rebuild
        }
    }
    for (id, e) in &world.entities {
        if e.kind != EntityKind::Chest || present.contains(id) {
            continue;
        }
        // A little wooden chest built from a few parts: body + banded domed lid +
        // gold trim + latch. Closed → gold trim glows to catch the eye; opened →
        // the lid is thrown back and the glow dies.
        let opened = e.opened;
        // Bespoke HD-2D chest billboard (PixelLab): closed vs. overflowing-open art.
        let key = if opened { "item_chest_open" } else { "item_chest_common" };
        let tex = wa.prop_sprites.get(key).cloned().unwrap_or_default();
        let mat = mats.add(hd2d::sprite_material(Color::WHITE, tex));
        commands
            .spawn((
                ChestEntity { id: id.clone(), opened },
                // Lift onto its terrace so a treasure-atop-a-climb chest sits on the
                // plateau, not buried in the cliff below it.
                Transform::from_translation(world_pos(e.x, e.y, e.level as f32 * STEP_HEIGHT)),
                Visibility::default(),
            ))
            .with_children(|p| {
                p.spawn((
                    Mesh3d(wa.sprite_quad.clone()),
                    MeshMaterial3d(mat),
                    Transform::from_xyz(0.0, 0.7, 0.0).with_scale(Vec3::splat(1.5 / 2.2)),
                    hd2d::Billboard,
                ));
            });
    }
}

/// Walk-into-to-open: when the avatar is within reach of an unopened chest, ask
/// the server to open it (mirrors [`auto_harvest`]). The server rolls the loot.
pub(crate) fn auto_open_chest(
    net: NonSend<NetRes>,
    world: Res<Overworld>,
    session: Res<Session>,
    overlay: Res<Overlay>,
    mut sent: Local<std::collections::HashSet<String>>,
) {
    if overlay.kind.is_some() || session.channeling {
        return;
    }
    let Some(me) = world.entities.get(&session.player_id) else {
        return;
    };
    for (id, e) in &world.entities {
        if e.kind == EntityKind::Chest
            && !e.opened
            && e.level == me.level // must be on the chest's level (a terrace-top chest)
            && ((e.x - me.x).powi(2) + (e.y - me.y).powi(2)).sqrt() <= 2.0
            && !sent.contains(id)
        {
            net.0.send(ClientCmd::OpenChest { entity_id: id.clone() });
            sent.insert(id.clone());
        }
    }
    sent.retain(|id| world.entities.contains_key(id));
}

/// Spawn a player's overworld avatar: a ground-anchored, walk-animated psyker
/// billboard (the placeholder for every class until per-class sprites land) with a
/// soft contact shadow. Tinted so you (white) read apart from allies/fighters.
#[allow(clippy::too_many_arguments)]
pub(crate) fn spawn_player_avatar(
    commands: &mut Commands,
    mats: &mut Assets<StandardMaterial>,
    wa: &WorldAssets,
    look: &hd2d::Look,
    id: &str,
    e: &OwEntity,
    me: &str,
    class: &str,
) {
    // Tints run slightly hot to counter the cool ambient dimming the now-lit
    // sprite, keeping the pixel art vibrant while it still catches the sun.
    let tint = if id == me {
        Color::srgb(1.25, 1.22, 1.12) // you — bright, faintly warm
    } else if e.battling {
        Color::srgb(1.3, 0.7, 0.5) // a fighting ally glows warm — go join
    } else {
        Color::srgb(0.85, 1.0, 1.3) // ally
    };
    // The overworld shows one avatar per player; pick its sprite from the lead
    // class (only the Psyker has bespoke art → everyone else uses the martial sprite).
    let frames = wa.class_frames(class);
    let mat = mats.add(hd2d::sprite_material(tint, frames.idle[0].clone()));
    let root = world_pos(e.x, e.y, 0.0);
    commands
        .spawn((
            WorldEntity(id.to_string()),
            Transform::from_translation(root),
            Visibility::default(),
            CharSprite::new(frames.clone(), mat.clone(), root),
        ))
        .with_children(|p| {
            let mut bb = p.spawn((
                Mesh3d(wa.sprite_quad.clone()),
                MeshMaterial3d(mat),
                Transform::from_xyz(0.0, look.sprite_y, 0.0),
                hd2d::Billboard,
                hd2d::HeroBillboard,
            ));
            // The local hero's own sprite self-illuminates at night (a point light
            // alone can't light the billboard it sits inside).
            if id == me {
                bb.insert(PlayerGlowSprite);
            }
            p.spawn((
                Mesh3d(wa.shadow_mesh.clone()),
                MeshMaterial3d(wa.shadow_mat.clone()),
                Transform::from_xyz(0.0, 0.02, 0.0)
                    .with_rotation(Quat::from_rotation_x(-std::f32::consts::FRAC_PI_2))
                    .with_scale(Vec3::new(1.0, 0.55, 1.0)),
            ));
            // Explorer "Predator's Eye" lamp: a real point light on the local avatar,
            // brightening at night as the perk levels (see `update_explorer_lamp`). Only
            // the local player carries it; intensity 0 until the perk is earned.
            if id == me {
                p.spawn((
                    ExplorerLamp,
                    PointLight {
                        color: Color::srgb(1.0, 0.86, 0.6),
                        intensity: 0.0,
                        range: 14.0,
                        radius: 0.4,
                        shadows_enabled: false,
                        ..default()
                    },
                    Transform::from_xyz(0.0, 2.2, 0.0),
                ));
            }
        });
}

/// Spawn one trailing party-member avatar (cosmetic; see [`sync_party_followers`]).
/// Lighter than the lead: a movement-animated billboard + shadow, no lamp/glow.
pub(crate) fn spawn_follower(
    commands: &mut Commands,
    mats: &mut Assets<StandardMaterial>,
    wa: &WorldAssets,
    look: &hd2d::Look,
    class: &str,
    slot: usize,
    pos: Vec3,
) {
    let frames = wa.class_frames(class);
    let tint = Color::srgb(1.1, 1.12, 1.18); // party members read a touch cooler than the lead
    let mat = mats.add(hd2d::sprite_material(tint, frames.idle[0].clone()));
    commands
        .spawn((
            PartyFollower { slot },
            Transform::from_translation(pos),
            Visibility::default(),
            CharSprite::new(frames.clone(), mat.clone(), pos),
        ))
        .with_children(|p| {
            p.spawn((
                Mesh3d(wa.sprite_quad.clone()),
                MeshMaterial3d(mat),
                Transform::from_xyz(0.0, look.sprite_y, 0.0),
                hd2d::Billboard,
                hd2d::HeroBillboard,
                PlayerGlowSprite, // stay visible at night like the lead
            ));
            p.spawn((
                Mesh3d(wa.shadow_mesh.clone()),
                MeshMaterial3d(wa.shadow_mat.clone()),
                Transform::from_xyz(0.0, 0.02, 0.0)
                    .with_rotation(Quat::from_rotation_x(-std::f32::consts::FRAC_PI_2))
                    .with_scale(Vec3::new(1.0, 0.55, 1.0)),
            ));
        });
}

/// `P` toggles showing the whole party trailing you in the overworld (the menu's
/// party screen offers the same toggle — see the inventory Status tab hint).
pub(crate) fn toggle_party_view(keys: Res<ButtonInput<KeyCode>>, overlay: Res<Overlay>, mut pv: ResMut<PartyView>) {
    // Ignore while a full-screen overlay owns the keyboard.
    if overlay.kind.is_none() && keys.just_pressed(KeyCode::KeyP) {
        pv.show = !pv.show;
    }
}

/// Keep the cosmetic party-follower avatars in step with [`PartyView`]: spawn one per
/// non-lead hero when on (despawn them when off), and trail them behind the lead in a
/// loose V so the whole party appears to travel together.
#[allow(clippy::too_many_arguments)]
pub(crate) fn sync_party_followers(
    mut commands: Commands,
    mut mats: ResMut<Assets<StandardMaterial>>,
    time: Res<Time>,
    wa: Res<WorldAssets>,
    look: Res<hd2d::Look>,
    pv: Res<PartyView>,
    session: Res<Session>,
    lead_q: Query<(&WorldEntity, &Transform, &CharSprite), Without<PartyFollower>>,
    mut followers: Query<(Entity, &PartyFollower, &mut Transform), With<PartyFollower>>,
) {
    // How many followers we want: every party member after the lead (cap 3).
    let want = if pv.show {
        session.party.len().min(4).saturating_sub(1)
    } else {
        0
    };
    // Drop any follower that's no longer wanted (toggle off, or party shrank).
    for (e, f, _) in &followers {
        if f.slot > want {
            commands.entity(e).despawn();
        }
    }
    if want == 0 {
        return;
    }
    // Find the lead avatar (the local player's) for the formation anchor.
    let Some((lead_pos, facing)) = lead_q
        .iter()
        .find(|(we, _, _)| we.0 == session.player_id)
        .map(|(_, t, cs)| (t.translation, cs.facing))
    else {
        return;
    };
    let f = facing.normalize_or_zero();
    let fwd = Vec3::new(f.x, 0.0, f.y);
    let right = Vec3::new(-f.y, 0.0, f.x);
    // Per-slot trailing offset (behind the lead, fanned into a V).
    let slot_offset = |slot: usize| -> Vec3 {
        let (back, side) = match slot {
            1 => (2.0, -1.3),
            2 => (2.9, 0.0),
            3 => (2.0, 1.3),
            _ => (3.6, 0.0),
        };
        -fwd * back + right * side
    };
    // Ensure a follower exists for each wanted slot; ones present just get steered.
    let mut present: Vec<usize> = followers.iter().map(|(_, f, _)| f.slot).collect();
    for slot in 1..=want {
        if !present.contains(&slot) {
            let class = session.party.get(slot).map(String::as_str).unwrap_or("explorer");
            spawn_follower(&mut commands, &mut mats, &wa, &look, class, slot, lead_pos + slot_offset(slot));
            present.push(slot);
        }
    }
    // Steer existing followers toward their formation slot (smooth, frame-rate independent).
    let k = 1.0 - (-time.delta_secs() * 6.0).exp();
    for (_, f, mut t) in &mut followers {
        let target = lead_pos + slot_offset(f.slot);
        t.translation.x += (target.x - t.translation.x) * k;
        t.translation.z += (target.z - t.translation.z) * k;
        t.translation.y += (lead_pos.y - t.translation.y) * k;
    }
}

// ---------------------------------------------------------- perks (party sense) ---

/// Marks the point light carried by the local avatar (Explorer "Predator's Eye").
#[derive(Component)]
pub(crate) struct ExplorerLamp;
/// Marks a PLAYER-CHARACTER sprite billboard (overworld local hero + every battle
/// hero) so its material self-illuminates at night — a co-located point light
/// can't light the billboard it sits inside, so player sprites would otherwise go
/// black in the dark. See [`illuminate_players`].
#[derive(Component)]
pub(crate) struct PlayerGlowSprite;
/// A warm point light carried by each battle hero at night. The **Explorer** carries a
/// big, bright lamp — its "Predator's Eye" class feature — with enough reach to light
/// the enemy row across the arena; every other class carries only a soft, short-range
/// glow so it stays visible without washing the scene or overflowing the renderer's
/// light clusters (which read as flicker). `strength` is the full-dark intensity that
/// [`illuminate_players`] scales by nightfall; the range/radius are baked at spawn.
#[derive(Component)]
pub(crate) struct BattlePartyLamp {
    pub(crate) strength: f32,
}
/// Root UI node that holds the per-mob nameplates (Explorer/Psyker intel).
#[derive(Component)]
pub(crate) struct NameplateRoot;
/// One mob nameplate (rebuilt each frame).
#[derive(Component)]
pub(crate) struct Nameplate;
/// Root UI node for the Shifter corner minimap.
#[derive(Component)]
pub(crate) struct MinimapRoot;
/// One minimap dot (rebuilt each frame).
#[derive(Component)]
pub(crate) struct MinimapDot;

/// Explorer "Predator's Eye": drive the avatar lamp — brighter at night, wider as the
/// perk levels, dark by day and absent without a Explorer (intensity from `run.perks`).
/// Explorer "Predator's Eye": the avatar's point light illuminates the surrounding
/// overworld at night, brighter + wider as the perk levels (from `run.perks`).
pub(crate) fn update_explorer_lamp(
    perks: Res<PerksRes>,
    sky: Res<Sky>,
    mut q: Query<&mut PointLight, With<ExplorerLamp>>,
) {
    let glow = perks.0.explorer_glow;
    let night = (1.0 - sky.day).clamp(0.0, 1.0);
    for mut light in &mut q {
        light.intensity = glow * night;
        light.range = 12.0 + glow / 8000.0;
    }
}

/// Player characters carry their own light at night so the game stays readable in
/// the dark — overworld AND battle. Two parts, both scaled by darkness (nothing by
/// day): (1) every [`PlayerGlowSprite`] self-illuminates by emitting its own
/// texture, so the hero never goes black; (2) each [`BattlePartyLamp`] point light
/// throws warm light off the party onto the enemy creature in the arena.
pub(crate) fn illuminate_players(
    sky: Res<Sky>,
    mut mats: ResMut<Assets<StandardMaterial>>,
    sprites: Query<&MeshMaterial3d<StandardMaterial>, With<PlayerGlowSprite>>,
    mut lamps: Query<(&mut PointLight, &BattlePartyLamp)>,
) {
    let night = (1.0 - sky.day).clamp(0.0, 1.0);
    // Self-illumination: warm glow keyed off each sprite's own texture colours.
    let ef = night * 1.15;
    for mh in &sprites {
        if let Some(m) = mats.get_mut(&mh.0) {
            // Track the CURRENT frame every tick, not once. `animate_chars` swaps
            // `base_color_texture` to the right facing/walk frame each frame; if the
            // emissive layer latched onto the first (idle-south) frame it would paint a
            // frozen south sprite over the animating base — at night, where emissive
            // dominates, the hero looked permanently stuck facing south while its walk
            // cycle still played. Mirroring it here keeps the lit glow in sync.
            m.emissive_texture = m.base_color_texture.clone();
            m.emissive = LinearRgba::rgb(ef, ef * 0.9, ef * 0.7);
        }
    }
    // Each hero's lamp, scaled by nightfall and its own strength (the Explorer's is far
    // brighter — its class feature — while the rest stay a soft fill).
    for (mut light, lamp) in &mut lamps {
        light.intensity = night * lamp.strength;
    }
}

/// Explorer/Psyker intel: float a nameplate over each overworld mob — its level
/// (Explorer tier ≥1), an HP bar (tier ≥2), and a Psyker threat marker for
/// elites/gatekeepers (≥1) and aggressive mobs (≥2). Rebuilt each frame from the
/// mobs' rendered positions, projected to screen.
#[allow(clippy::type_complexity)]
pub(crate) fn update_mob_nameplates(
    mut commands: Commands,
    perks: Res<PerksRes>,
    world: Res<Overworld>,
    cam_q: Query<(&Camera, &GlobalTransform)>,
    root_q: Query<Entity, With<NameplateRoot>>,
    mob_q: Query<(&WorldEntity, &GlobalTransform)>,
    old: Query<Entity, With<Nameplate>>,
) {
    // Clear last frame's plates.
    for e in &old {
        commands.entity(e).despawn();
    }
    let intel = perks.0.explorer_intel;
    let threat = perks.0.psyker_threat;
    if intel == 0 && threat == 0 {
        return;
    }
    let Some((cam, cam_tf)) = cam_q.iter().next() else {
        return;
    };
    let Ok(root) = root_q.single() else {
        return;
    };
    commands.entity(root).with_children(|p| {
        for (we, gtf) in &mob_q {
            let Some(ent) = world.entities.get(&we.0) else {
                continue;
            };
            if !matches!(ent.kind, EntityKind::Monster) {
                continue;
            }
            // Project a point above the mob's head to the screen.
            let head = gtf.translation() + Vec3::Y * 2.6;
            let Some(s) = cam.world_to_viewport(cam_tf, head).ok() else {
                continue;
            };
            // Threat marker (Psyker): elites/gatekeepers, then aggressive mobs.
            let ec = ent.encounter_class.as_deref().unwrap_or("standard");
            let aggr = ent.aggression.as_deref().unwrap_or("passive");
            let (marker, marker_col) = if threat >= 1 && ec == "gatekeeper" {
                ("!!!", Color::srgb(1.0, 0.3, 0.3))
            } else if threat >= 1 && ec == "elite" {
                ("!!", Color::srgb(1.0, 0.55, 0.2))
            } else if threat >= 2 && aggr == "aggressive" {
                ("!", Color::srgb(1.0, 0.75, 0.3))
            } else {
                ("", Color::NONE)
            };
            p.spawn((
                Nameplate,
                Node {
                    position_type: PositionType::Absolute,
                    left: Val::Px(s.x - 24.0),
                    top: Val::Px(s.y - 14.0),
                    width: Val::Px(48.0),
                    flex_direction: FlexDirection::Column,
                    align_items: AlignItems::Center,
                    row_gap: Val::Px(1.0),
                    ..default()
                },
            ))
            .with_children(|c| {
                if !marker.is_empty() {
                    c.spawn((
                        Text::new(marker),
                        TextFont { font_size: 13.0, ..default() },
                        TextColor(marker_col),
                    ));
                }
                if intel >= 1 {
                    let lvl = ent.mob_level.unwrap_or(0);
                    c.spawn((
                        Text::new(format!("Lv {lvl}")),
                        TextFont { font_size: 12.0, ..default() },
                        TextColor(Color::srgb(0.95, 0.95, 1.0)),
                    ));
                }
                if intel >= 2 {
                    if let (Some(hp), Some(max)) = (ent.hp, ent.max_hp) {
                        let frac = if max > 0 { (hp as f32 / max as f32).clamp(0.0, 1.0) } else { 0.0 };
                        // Green → red as HP falls.
                        let fill = Color::srgb(1.0 - frac * 0.8, 0.2 + frac * 0.7, 0.2);
                        c.spawn((
                            Node {
                                width: Val::Px(40.0),
                                height: Val::Px(5.0),
                                border: UiRect::all(Val::Px(1.0)),
                                ..default()
                            },
                            BorderColor(Color::srgba(0.0, 0.0, 0.0, 0.7)),
                            BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.5)),
                        ))
                        .with_children(|bar| {
                            bar.spawn((
                                Node {
                                    width: Val::Percent(frac * 100.0),
                                    height: Val::Percent(100.0),
                                    ..default()
                                },
                                BackgroundColor(fill),
                            ));
                        });
                    }
                }
            });
        }
    });
}

/// Shifter "Scout's Instinct": rebuild the corner minimap. The panel shows/hides by
/// the map tier; dots plot entities within `shifter_map_radius` of the player —
/// mobs + portal (tier ≥1), chests (≥2), harvestables (≥3), self at centre.
#[allow(clippy::type_complexity)]
pub(crate) fn update_minimap(
    mut commands: Commands,
    perks: Res<PerksRes>,
    world: Res<Overworld>,
    session: Res<Session>,
    mut root_q: Query<(Entity, &mut Node), With<MinimapRoot>>,
    old: Query<Entity, With<MinimapDot>>,
) {
    for e in &old {
        commands.entity(e).despawn();
    }
    let Ok((root, mut node)) = root_q.single_mut() else {
        return;
    };
    let tier = perks.0.shifter_map;
    node.display = if tier >= 1 { Display::Flex } else { Display::None };
    if tier == 0 {
        return;
    }
    let Some(me) = world.entities.get(&session.player_id) else {
        return;
    };
    // Panel is 140px; keep dots inside a 64px radius from its centre.
    const HALF: f32 = 70.0;
    const R: f32 = 64.0;
    let radius = perks.0.shifter_map_radius.max(1.0);
    let scale = R / radius;
    commands.entity(root).with_children(|p| {
        // The player, dead centre.
        spawn_dot(p, HALF, HALF, 6.0, Color::srgb(1.0, 1.0, 1.0));
        for e in world.entities.values() {
            let (col, size) = match e.kind {
                EntityKind::Monster => (Color::srgb(1.0, 0.4, 0.35), 5.0),
                EntityKind::Portal => (Color::srgb(0.4, 0.85, 1.0), 6.0),
                EntityKind::Chest if tier >= 2 => (Color::srgb(1.0, 0.82, 0.3), 5.0),
                EntityKind::Resource if tier >= 3 => (Color::srgb(0.5, 0.95, 0.5), 4.0),
                _ => continue,
            };
            let (dx, dy) = ((e.x - me.x) * scale, (e.y - me.y) * scale);
            if dx.abs() > R || dy.abs() > R {
                continue; // outside the minimap's world radius
            }
            spawn_dot(p, HALF + dx, HALF + dy, size, col);
        }
    });
}

/// Spawn one absolutely-positioned minimap dot centred at (`cx`,`cy`) px.
pub(crate) fn spawn_dot(p: &mut ChildSpawnerCommands, cx: f32, cy: f32, size: f32, col: Color) {
    p.spawn((
        MinimapDot,
        Node {
            position_type: PositionType::Absolute,
            left: Val::Px(cx - size / 2.0),
            top: Val::Px(cy - size / 2.0),
            width: Val::Px(size),
            height: Val::Px(size),
            ..default()
        },
        BorderRadius::all(Val::Percent(50.0)),
        BackgroundColor(col),
    ));
}

/// Deterministically pick an index in `0..n` from an entity id (FNV-1a). Lets a
/// grove of identical-kind obstacles show varied art without any per-entity state.
pub(crate) fn hash_pick(id: &str, n: usize) -> usize {
    if n == 0 {
        return 0;
    }
    let mut h: u32 = 2166136261;
    for b in id.bytes() {
        h = (h ^ b as u32).wrapping_mul(16777619);
    }
    (h as usize) % n
}

/// Normalize a creature's wire name to its bare content-id **kind**. The overworld
/// tags a mob `mob:<kind>:<faction>` (client parses out `<kind>`, e.g. `dune_wyrm`),
/// but the battle sends a champion's affix prepended (`"Swift dune_wyrm"`, affixes
/// Swift/Brutal/Armored/Giant/Vicious — see meld-world `apply_affix`). A content-id
/// kind is a single underscored token with no spaces, so the kind is the last
/// whitespace-delimited word; lowercased for good measure.
pub(crate) fn creature_kind(name: &str) -> String {
    // Canonicalize any form of a creature's name to its underscored content id so the
    // SAME creature resolves to the SAME sprite everywhere. The overworld sends the
    // underscored kind ("forest_bloom_stalker"); the battle sends a spaced DISPLAY name
    // ("Forest Bloom Stalker"), optionally with a champion affix prefix ("Swift
    // dune_wyrm"). So: lowercase, drop a leading known affix, then join the remaining
    // words with '_' (which turns "forest bloom stalker" back into
    // "forest_bloom_stalker" and leaves an already-underscored kind untouched).
    const AFFIXES: [&str; 5] = ["swift", "brutal", "armored", "giant", "vicious"];
    let lower = name.trim().to_ascii_lowercase();
    let mut words: Vec<&str> = lower.split_whitespace().collect();
    if words.len() > 1 && AFFIXES.contains(&words[0]) {
        words.remove(0);
    }
    words.join("_")
}

/// Resolve the billboard sprite for a creature by its normalized [`creature_kind`],
/// so the SAME creature always renders the SAME sprite in the overworld and in
/// battle. Mapped kinds hit `monster_sprites`; anything else hashes the *kind*
/// (not the per-entity id) into the fallback pool, so every instance of an unmapped
/// kind — and its later battle combatant — still agree on one sprite. (Fixes the
/// overworld↔battle sprite mismatch, which was worst for affixed champions.)
pub(crate) fn creature_sprite(wa: &WorldAssets, name: &str) -> Handle<Image> {
    let kind = creature_kind(name);
    wa.monster_sprites.get(&kind).cloned().unwrap_or_else(|| {
        let pool = &wa.monster_pool;
        pool[hash_pick(&kind, pool.len().max(1))].clone()
    })
}

/// Spawn a camera-facing pixel-sprite billboard for a world entity (monster, prop,
/// harvest node, portal): a lit, alpha-masked, ground-anchored quad plus (optionally)
/// a soft contact shadow. `height` is the sprite's world height; `tint` recolours it;
/// `shadow` is the shadow disc radius (0 = none). Tagged only [`hd2d::Billboard`]
/// (not `HeroBillboard`), so it keeps this spawn-baked scale/height and just yaws to
/// face the camera — hero sprites alone follow the live-tuned `Look` size.
#[allow(clippy::too_many_arguments)]
/// Build the stepped ground+cliff relief for every streamed section that isn't
/// rendered yet, and spawn its connector props. Rebuilds sections whose meshes are
/// gone (e.g. after returning from a battle) — the same redraw-when-absent idea as
/// the path trail. Terraces sit ON TOP of the existing flat ground plane; only
/// raised cells get a top surface + cliff faces, so level 0 is the plain ground.
pub(crate) fn build_terrain_sections(
    mut commands: Commands,
    terrain: Res<Terrain>,
    wa: Option<Res<WorldAssets>>,
    assets: Res<AssetServer>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut mats: ResMut<Assets<StandardMaterial>>,
    existing: Query<&TerrainMesh>,
) {
    let Some(wa) = wa else { return };
    let built: HashSet<u32> = existing.iter().map(|t| t.0).collect();
    for (idx, sec) in &terrain.sections {
        if built.contains(idx) {
            continue;
        }
        if sec.levels.iter().any(|&l| l > 0) {
            let (top, cliff) = terrace_meshes(sec);
            // Plateau top wears the SECTION's own biome ground tile (a desert mesa,
            // ashfall shelf or tundra plateau no longer shows grass) — biome-aware
            // instead of always grass. Neutral white base so the tile's colour shows.
            let bi = crate::world_render::biome_ring_index(&sec.biome)
                .min(wa.ground_tex.len().saturating_sub(1));
            let top_mat = mats.add(StandardMaterial {
                base_color: Color::WHITE,
                base_color_texture: wa.ground_tex.get(bi).cloned(),
                perceptual_roughness: 0.95,
                cull_mode: None,
                ..default()
            });
            // Earthy tiled rock wall behind the cliff-edge dressing (loaded directly so
            // it's independent of the repurposed biome ground tiles above).
            let cliff_mat = mats.add(StandardMaterial {
                base_color: Color::srgb(0.62, 0.5, 0.38),
                base_color_texture: Some(crate::world_render::load_tiled(
                    &assets,
                    "ground/dirt_full.png",
                )),
                perceptual_roughness: 1.0,
                cull_mode: None,
                ..default()
            });
            commands.spawn((
                TerrainMesh(*idx),
                Mesh3d(meshes.add(top)),
                MeshMaterial3d(top_mat),
                Transform::default(),
            ));
            commands.spawn((
                TerrainMesh(*idx),
                Mesh3d(meshes.add(cliff)),
                MeshMaterial3d(cliff_mat),
                Transform::default(),
            ));
            // Dress the terrace edges with our HD-2D cliff sprite billboards.
            spawn_terrace_cliffs(&mut commands, &mut meshes, &mut mats, &assets, sec, *idx);
        } else {
            // Flat section (e.g. the tutorial): record it as built so we don't
            // rescan it every frame, but draw nothing.
            commands.spawn((TerrainMesh(*idx), Transform::default(), Visibility::Hidden));
        }
        // The ladders / ropes / slopes that make each terrace reachable.
        let (half, lat) = radial_params(sec);
        for c in &sec.connectors {
            spawn_connector(&mut commands, &mut meshes, &mut mats, &assets, *idx, c, half, lat);
        }
    }
}

/// Dress a section's terrace edges with our HD-2D **cliff rock** sprite billboards:
/// one per boundary cell (a raised cell with a lower neighbour), spanning the rise, so
/// the terraces read as rocky cliffs rather than flat brown walls — no Kenney models.
/// The grass-top mesh covers the surface; the backing cliff mesh fills any gaps.
pub(crate) fn spawn_terrace_cliffs(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    mats: &mut Assets<StandardMaterial>,
    assets: &AssetServer,
    sec: &meld_client::net::TerrainSectionView,
    idx: u32,
) {
    // One shared sprite billboard (rocky outcrop) reused for every edge cell.
    let cliff_mat = mats.add(hd2d::sprite_material(
        Color::srgb(0.9, 0.9, 0.92),
        assets.load("props/obstacle_cliff.png"),
    ));
    let cliff_quad = meshes.add(hd2d::cyl_billboard_mesh(2.2, 2.2, 10, 55.0));
    let cols = sec.cols as usize;
    let rows = sec.rows as usize;
    let cell = sec.cell as f32;
    let sx = sec.start_x as f32;
    let zmin = sec.y_min as f32;
    let (half, lat) = radial_params(sec); // bend cliff billboards into the fan
    let lvl = |gx: i64, gy: i64| -> u8 {
        if gx < 0 || gy < 0 || gx >= cols as i64 || gy >= rows as i64 {
            0
        } else {
            sec.levels[gx as usize * rows + gy as usize]
        }
    };
    let mut placed = 0u32;
    for gx in 0..cols {
        for gy in 0..rows {
            let l = sec.levels[gx * rows + gy];
            if l == 0 {
                continue;
            }
            // Outward direction = sum of the lower-neighbour directions; the lowest
            // neighbour sets how far the rock face drops.
            let mut dir = Vec2::ZERO;
            let mut lowest = l;
            for (ddx, ddz) in [(0i64, -1i64), (0, 1), (-1, 0), (1, 0)] {
                let nl = lvl(gx as i64 + ddx, gy as i64 + ddz);
                if nl < l {
                    dir += Vec2::new(ddx as f32, ddz as f32);
                    lowest = lowest.min(nl);
                }
            }
            if dir == Vec2::ZERO {
                continue; // interior cell — the grass top mesh covers it
            }
            let _ = dir; // billboards face the camera, so no outward yaw needed
            let cx = sx + (gx as f32 + 0.5) * cell;
            let cz = zmin + (gy as f32 + 0.5) * cell;
            let (bx, bz) = radial_bend(cx, cz, half, lat); // fan the cliff into the arc
            let by = lowest as f32 * STEP_HEIGHT;
            // Span the rise from the lower neighbour up to this cell's top, a touch
            // taller so the rocky face overlaps the lip.
            let face = ((l - lowest) as f32).max(1.0) * STEP_HEIGHT * 1.15 + 0.6;
            commands
                .spawn((
                    TerrainMesh(idx),
                    Transform::from_xyz(bx, by, bz),
                    Visibility::default(),
                ))
                .with_children(|p| {
                    p.spawn((
                        Mesh3d(cliff_quad.clone()),
                        MeshMaterial3d(cliff_mat.clone()),
                        Transform::from_xyz(0.0, face * 0.5, 0.0)
                            .with_scale(Vec3::splat(face / 2.2)),
                        hd2d::Billboard,
                    ));
                });
            placed += 1;
            if placed > 400 {
                return; // safety cap on a pathological section
            }
        }
    }
}

/// Half-arc (radians) + corridor lateral for a section's WG-4 radial bend. `(0, 0)`
/// ⇒ flat corridor (no bend). Falls back to `-y_min` for the lateral if the wire
/// didn't carry it (older stream), since the grid spans `y ∈ [y_min, -y_min]`.
pub(crate) fn radial_params(sec: &meld_client::net::TerrainSectionView) -> (f32, f32) {
    let lat = if sec.corridor_lateral > 0.0 {
        sec.corridor_lateral as f32
    } else {
        (-sec.y_min) as f32
    };
    (sec.radial_half as f32, lat)
}

/// Bend a corridor point (`x` = radius axis, `z` = lateral axis) into the WG-4 fan,
/// matching the server's `radial_tf` exactly so raised terrain lines up with the
/// server-bent positions the avatar walks on. Identity when `half`/`lat` ≤ 0.
pub(crate) fn radial_bend(x: f32, z: f32, half: f32, lat: f32) -> (f32, f32) {
    if half <= 0.0 || lat <= 0.0 {
        return (x, z);
    }
    let r = x.max(0.0);
    let theta = (z / lat).clamp(-1.0, 1.0) * half;
    (r * theta.cos(), r * theta.sin())
}

/// Append a quad (two triangles) with a flat `normal` and per-corner `uv`. Winding
/// is fixed; the terrace materials render double-sided so face direction never
/// hides a surface.
#[allow(clippy::too_many_arguments)]
pub(crate) fn push_quad(
    p: &mut Vec<[f32; 3]>,
    n: &mut Vec<[f32; 3]>,
    u: &mut Vec<[f32; 2]>,
    idx: &mut Vec<u32>,
    a: [f32; 3],
    b: [f32; 3],
    c: [f32; 3],
    d: [f32; 3],
    normal: [f32; 3],
    uv: [[f32; 2]; 4],
) {
    let base = p.len() as u32;
    p.extend_from_slice(&[a, b, c, d]);
    n.extend_from_slice(&[normal; 4]);
    u.extend_from_slice(&uv);
    idx.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
}

/// Turn a section's elevation grid into two meshes: the terrace **tops** (grass,
/// biome-tinted) and the **cliff faces** (dirt/rock) dropping to each lower
/// neighbour. Vertices are in world space; overworld `y` maps to world Z.
pub(crate) fn terrace_meshes(sec: &meld_client::net::TerrainSectionView) -> (Mesh, Mesh) {
    use bevy::render::mesh::{Indices, PrimitiveTopology};
    use bevy::render::render_asset::RenderAssetUsages;
    let cols = sec.cols as usize;
    let rows = sec.rows as usize;
    let cell = sec.cell as f32;
    let sx = sec.start_x as f32;
    let zmin = sec.y_min as f32;
    let tile = 0.22f32; // texture repeats per world unit
    // WG-4 radial fan: the grid is in corridor coords; bend every vertex by the same
    // arc the server used to fan entity positions, so the terrace lines up with where
    // the (server-bent) avatar walks. Identity when the world is a flat corridor.
    let (half, lat) = radial_params(sec);
    let bend = |x: f32, z: f32| -> (f32, f32) { radial_bend(x, z, half, lat) };
    // Rotate a horizontal (cliff) normal by the local bearing so lighting still reads.
    let bend_n = |n: [f32; 3], z: f32| -> [f32; 3] {
        if half <= 0.0 || lat <= 0.0 {
            return n;
        }
        let th = (z / lat).clamp(-1.0, 1.0) * half;
        let (s, c) = th.sin_cos();
        [n[0] * c - n[2] * s, n[1], n[0] * s + n[2] * c]
    };
    let lvl = |gx: i64, gy: i64| -> u8 {
        if gx < 0 || gy < 0 || gx >= cols as i64 || gy >= rows as i64 {
            0
        } else {
            sec.levels[gx as usize * rows + gy as usize]
        }
    };
    let (mut tp, mut tn, mut tu, mut ti) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    let (mut cp, mut cn, mut cu, mut ci) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    for gx in 0..cols {
        for gy in 0..rows {
            let l = sec.levels[gx * rows + gy];
            if l == 0 {
                continue;
            }
            let topy = l as f32 * STEP_HEIGHT;
            let x0 = sx + gx as f32 * cell;
            let x1 = x0 + cell;
            let z0 = zmin + gy as f32 * cell;
            let z1 = z0 + cell;
            // Bent world XZ of each corner (radial fan; identity in corridor mode).
            let (a0, a1) = (bend(x0, z0), bend(x1, z0));
            let (a2, a3) = (bend(x1, z1), bend(x0, z1));
            // Terrace top.
            push_quad(
                &mut tp, &mut tn, &mut tu, &mut ti,
                [a0.0, topy, a0.1], [a1.0, topy, a1.1], [a2.0, topy, a2.1], [a3.0, topy, a3.1],
                [0.0, 1.0, 0.0],
                [[x0 * tile, z0 * tile], [x1 * tile, z0 * tile], [x1 * tile, z1 * tile], [x0 * tile, z1 * tile]],
            );
            // Cliff faces toward any lower neighbour (outside grid counts as level 0).
            // `quad` gives the two TOP corners as bent world XZ; the two bottom corners
            // reuse the same XZ at the neighbour's floor height.
            let mut face = |gx2: i64, gy2: i64, top_a: (f32, f32), top_b: (f32, f32), zc: f32, normal: [f32; 3]| {
                let nl = lvl(gx2, gy2);
                if (nl as f32) < l as f32 {
                    let by = nl as f32 * STEP_HEIGHT;
                    let hh = (topy - by) * tile;
                    push_quad(
                        &mut cp, &mut cn, &mut cu, &mut ci,
                        [top_a.0, by, top_a.1], [top_b.0, by, top_b.1],
                        [top_b.0, topy, top_b.1], [top_a.0, topy, top_a.1],
                        bend_n(normal, zc),
                        [[0.0, 0.0], [cell * tile, 0.0], [cell * tile, hh], [0.0, hh]],
                    );
                }
            };
            // -Z, +Z, -X, +X. Each face's two top corners in bent world XZ.
            face(gx as i64, gy as i64 - 1, a1, a0, z0, [0.0, 0.0, -1.0]);
            face(gx as i64, gy as i64 + 1, a3, a2, z1, [0.0, 0.0, 1.0]);
            face(gx as i64 - 1, gy as i64, a0, a3, z0, [-1.0, 0.0, 0.0]);
            face(gx as i64 + 1, gy as i64, a2, a1, z1, [1.0, 0.0, 0.0]);
        }
    }
    let build = |p: Vec<[f32; 3]>, n: Vec<[f32; 3]>, u: Vec<[f32; 2]>, i: Vec<u32>| {
        let mut m = Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::default());
        m.insert_attribute(Mesh::ATTRIBUTE_POSITION, p);
        m.insert_attribute(Mesh::ATTRIBUTE_NORMAL, n);
        m.insert_attribute(Mesh::ATTRIBUTE_UV_0, u);
        m.insert_indices(Indices::U32(i));
        m
    };
    (build(tp, tn, tu, ti), build(cp, cn, cu, ci))
}

/// Spawn the visible prop for one connector so the route up a cliff is legible: a
/// **slope** as a tilted ramp board, a **ladder** as an upright rung post, a **rope**
/// as a thin dangling line — each faintly emissive so it's findable in shade.
#[allow(clippy::too_many_arguments)]
pub(crate) fn spawn_connector(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    mats: &mut Assets<StandardMaterial>,
    assets: &AssetServer,
    idx: u32,
    c: &meld_client::net::ConnectorView,
    half: f32,
    lat: f32,
) {
    let lo_y = c.lo as f32 * STEP_HEIGHT;
    let hi_y = c.hi as f32 * STEP_HEIGHT;
    let h = (hi_y - lo_y).max(0.2);
    // Bend into the fan, then stand the prop a touch proud of the cliff base (nudged
    // toward the hub) so it reads as a distinct affordance, not swallowed by the face.
    let (mut x, mut z) = radial_bend(c.x as f32, c.y as f32, half, lat);
    let d = (x * x + z * z).sqrt();
    if d > 1.0 {
        x -= x / d * 0.5;
        z -= z / d * 0.5;
    } else {
        z -= 0.5;
    }

    // HD-2D pixel billboard for the connector (PixelLab art): a plank ramp for a
    // slope, a rung ladder, or a dangling rope — standing the whole rise so the route
    // up/down a cliff reads clearly. Billboards face the camera (`hd2d::Billboard`), so
    // they stay legible as the view orbits (no more stretched rock or glowing cuboids).
    let key = match c.kind.as_str() {
        "slope" => "connector_ramp",
        "rope" => "connector_rope",
        _ => "connector_ladder",
    };
    let tex: Handle<Image> = assets.load(format!("props/{key}.png"));
    let mat = mats.add(hd2d::sprite_material(Color::WHITE, tex));
    // Span the rise plus a little overlap so the ends tuck onto both levels.
    let span = (h + STEP_HEIGHT * 0.5).max(1.6);
    let quad = meshes.add(hd2d::cyl_billboard_mesh(2.2, 2.2, 12, 60.0));
    commands
        .spawn((
            TerrainMesh(idx),
            Transform::from_xyz(x, lo_y + 0.1, z),
            Visibility::default(),
        ))
        .with_children(|p| {
            p.spawn((
                Mesh3d(quad),
                MeshMaterial3d(mat),
                Transform::from_xyz(0.0, span * 0.5, 0.0).with_scale(Vec3::splat(span / 2.2)),
                hd2d::Billboard,
            ));
        });
}

pub(crate) fn spawn_billboard_entity(
    commands: &mut Commands,
    mats: &mut Assets<StandardMaterial>,
    wa: &WorldAssets,
    id: &str,
    e: &OwEntity,
    tex: Handle<Image>,
    height: f32,
    tint: Color,
    shadow: f32,
) {
    // The shared quad mesh is 2.2 world-units tall; scale to the wanted height and
    // lift it so the sprite's feet sit on the ground plane.
    let scale = height / 2.2;
    let mat = mats.add(hd2d::sprite_material(tint, tex));
    commands
        .spawn((
            WorldEntity(id.to_string()),
            Transform::from_translation(world_pos(e.x, e.y, 0.0)),
            Visibility::default(),
        ))
        .with_children(|p| {
            p.spawn((
                Mesh3d(wa.sprite_quad.clone()),
                MeshMaterial3d(mat),
                Transform::from_xyz(0.0, height * 0.5, 0.0).with_scale(Vec3::splat(scale)),
                hd2d::Billboard,
            ));
            if shadow > 0.0 {
                p.spawn((
                    Mesh3d(wa.shadow_mesh.clone()),
                    MeshMaterial3d(wa.shadow_mat.clone()),
                    Transform::from_xyz(0.0, 0.02, 0.0)
                        .with_rotation(Quat::from_rotation_x(-std::f32::consts::FRAC_PI_2))
                        .with_scale(Vec3::new(shadow, shadow * 0.55, shadow)),
                ));
            }
        });
}

/// Spawn a terrain obstacle sized to its world radius. Vegetation and rock kinds are
/// **real 3D models** (Kenney Nature Kit, CC0) — one of several variants picked by id
/// hash and rotated for variety, so the world reads as dimensional HD-2D geometry
/// rather than flat cut-outs. Water kinds stay flat pools; anything unmapped falls
/// back to the lit boulder mesh.
pub(crate) fn spawn_obstacle(
    commands: &mut Commands,
    mats: &mut Assets<StandardMaterial>,
    wa: &WorldAssets,
    id: &str,
    e: &OwEntity,
    dungeon_theme: &str,
) {
    let name = e.name.as_deref().unwrap_or("");
    let r = e.radius.max(0.4);
    let col = obstacle_color(name);
    // DG-6b: dungeon interior maze walls. A forest dungeon reads as a forest — so a
    // wall/closed-door cell is planted with LOW foliage (a squat bush, kept shorter
    // than the ~1.6-tall hero so you always see your character to steer), not a stone
    // block that would look out of place under the canopy. Non-forest themes (ruins in
    // desert/ashfall/tundra/mire) keep tinted stone/timber masonry, which suits them.
    if name == "dungeon_wall" || name == "dungeon_door" {
        let is_door = name == "dungeon_door";
        if dungeon_theme == "forest" {
            // Squat bush billboard — the SAME PixelLab foliage the world uses, scaled
            // low so it reads as undergrowth and never hides the hero. A door cell gets
            // a paler, slightly taller sprig so an opening is still legible.
            const BUSH: [&str; 3] = ["obstacle_tree_bushy", "obstacle_tree_willow", "obstacle_tree"];
            let pool: Vec<Handle<Image>> = BUSH
                .iter()
                .filter_map(|k| wa.prop_sprites.get(*k).cloned())
                .collect();
            if !pool.is_empty() {
                let tex = pool[hash_pick(id, pool.len())].clone();
                let height = if is_door { 1.9 } else { 1.5 + (hash_pick(id, 40) as f32) * 0.01 };
                let tint = if is_door { Color::srgb(0.75, 0.95, 0.7) } else { Color::WHITE };
                let mat = mats.add(hd2d::sprite_material(tint, tex));
                commands
                    .spawn((
                        WorldEntity(id.to_string()),
                        Transform::from_translation(world_pos(e.x, e.y, 0.0)),
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
        let (base_color, height) = if is_door {
            (Color::srgb(0.42, 0.26, 0.15), 2.3) // banded timber door
        } else {
            (Color::srgb(0.33, 0.31, 0.36), 2.9) // grey dungeon stone
        };
        let mat = mats.add(StandardMaterial { base_color, perceptual_roughness: 1.0, ..default() });
        commands.spawn((
            WorldEntity(id.to_string()),
            Mesh3d(wa.rock_mesh.clone()),
            MeshMaterial3d(mat),
            Transform::from_translation(world_pos(e.x, e.y, 0.0))
                .with_scale(Vec3::new((r * 1.7).max(0.9), height, (r * 1.7).max(0.9))),
        ));
        return;
    }
    // Prefer the bespoke HD-2D pixel billboard for this obstacle (PixelLab art),
    // scaled by the collision radius. Water pools keep their animated shader (below),
    // where the moving surface reads better than a flat sprite.
    let is_water = matches!(name, "pond" | "frozen_pond" | "bog_pool");
    if !is_water {
        // Trees draw from a variety pool (oak/pine/birch/dead/willow/bushy) picked by
        // id-hash, with an extra per-id size factor on top of the radius so a forest
        // reads as a mix of shapes and heights rather than one stamped tree.
        if name == "tree" {
            const TREE_VARIANTS: [&str; 6] = [
                "obstacle_tree", "obstacle_tree_pine", "obstacle_tree_birch",
                "obstacle_tree_dead", "obstacle_tree_willow", "obstacle_tree_bushy",
            ];
            let pool: Vec<Handle<Image>> = TREE_VARIANTS
                .iter()
                .filter_map(|k| wa.prop_sprites.get(*k).cloned())
                .collect();
            if !pool.is_empty() {
                let tex = pool[hash_pick(id, pool.len())].clone();
                // Per-id size factor 0.75..1.6 → a varied canopy line.
                // Trees tower over the ~2.2-unit heroes; wide per-id spread so the
                // canopy line varies. (Bumped up — they read too small/low before.)
                let vf = 0.85 + (hash_pick(id, 100) as f32 / 100.0) * 0.9; // 0.85..1.75
                let height = ((3.6 + r * 1.4) * vf).clamp(3.4, 9.5);
                spawn_billboard_entity(commands, mats, wa, id, e, tex, height, Color::WHITE, height * 0.28);
                return;
            }
        }
        if let Some(tex) = wa.prop_sprites.get(&format!("obstacle_{name}")) {
            let height = (1.8 + r * 0.8).clamp(1.8, 4.5);
            spawn_billboard_entity(commands, mats, wa, id, e, tex.clone(), height, Color::WHITE, 0.55);
            return;
        }
    }
    // 3D prop model (tree/rock/cliff/cactus/mushroom/…), variant + yaw from the id.
    if let Some(variants) = wa.prop_scenes.get(name) {
        if !variants.is_empty() {
            let (scene, base) = &variants[hash_pick(id, variants.len())];
            // Gently modulate the baked scale by the collision radius so bigger
            // obstacles read bigger, without drifting far from the tuned size.
            let scale = base * (0.85 + r * 0.15).clamp(0.85, 1.5);
            let yaw = (hash_pick(id, 360) as f32).to_radians();
            let mut ent = commands.spawn((
                WorldEntity(id.to_string()),
                SceneRoot(scene.clone()),
                Transform::from_translation(world_pos(e.x, e.y, 0.0))
                    .with_scale(Vec3::splat(scale))
                    .with_rotation(Quat::from_rotation_y(yaw)),
            ));
            // Foliage sways in the wind (see `animate_sway`); rock/cliff stays rigid.
            if let Some(amp) = sway_amp(name) {
                let h = hash_pick(id, 10000);
                ent.insert(Sway {
                    base_yaw: yaw,
                    phase: (h % 628) as f32 / 100.0,
                    amp,
                    speed: 0.7 + ((h / 628) % 60) as f32 / 100.0,
                });
            }
            return;
        }
    }
    match name {
        "pond" | "frozen_pond" | "bog_pool" => {
            // Bespoke pixel-art water tile per kind (drifted by `animate_water`); spin
            // each organic blob a different way so pools don't look stamped from one shape.
            let spin = (hash_pick(id, 360) as f32).to_radians();
            commands.spawn((
                WorldEntity(id.to_string()),
                Mesh3d(wa.water_mesh.clone()),
                MeshMaterial3d(wa.water_mat(name)),
                Transform::from_translation(world_pos(e.x, e.y, 0.2))
                    .with_rotation(
                        Quat::from_rotation_y(spin) * Quat::from_rotation_x(-std::f32::consts::FRAC_PI_2),
                    )
                    .with_scale(Vec3::splat(r * 2.0)),
            ));
        }
        _ => {
            let mat = mats.add(StandardMaterial {
                base_color: col,
                perceptual_roughness: 1.0,
                ..default()
            });
            commands.spawn((
                WorldEntity(id.to_string()),
                Mesh3d(wa.rock_mesh.clone()),
                MeshMaterial3d(mat),
                Transform::from_translation(world_pos(e.x, e.y, 0.24 * r))
                    .with_scale(Vec3::splat(r * 0.7)),
            ));
        }
    }
}

/// Turn a creature content id into a display name (`dune_wyrm` → `dune wyrm`).
pub(crate) fn nice_name(kind: &str) -> String {
    kind.replace('_', " ")
}

/// Title-case a class key for display (`alchemist_knight` → `Alchemist Knight`).
pub(crate) fn class_display(key: &str) -> String {
    key.split('_')
        .map(|w| {
            let mut cs = w.chars();
            match cs.next() {
                Some(f) => f.to_uppercase().collect::<String>() + cs.as_str(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Colour for a terrain obstacle kind — greenery, stone, water and lava read
/// distinctly so the map's geography is legible.
pub(crate) fn obstacle_color(kind: &str) -> Color {
    match kind {
        "tree" | "cactus" | "mire_root" | "fungal_wall" => Color::srgb(0.18, 0.42, 0.22), // foliage
        "pond" | "frozen_pond" | "bog_pool" => Color::srgb(0.22, 0.4, 0.6), // water
        "lava" => Color::srgb(0.75, 0.32, 0.12), // molten
        "ice_spire" | "snow_drift" => Color::srgb(0.72, 0.82, 0.9), // ice
        // cliffs, boulders, dunes, spires, cinder rock — stone tones
        _ => Color::srgb(0.42, 0.4, 0.38),
    }
}

/// A distinct, deterministic colour per creature **faction** (FNV-1a hash → hue),
/// so you can read who belongs together (and who doesn't) at a glance.
pub(crate) fn faction_color(faction: &str) -> Color {
    let mut h: u32 = 2166136261;
    for b in faction.bytes() {
        h = (h ^ b as u32).wrapping_mul(16777619);
    }
    Color::hsl((h % 360) as f32, 0.62, 0.56)
}

pub(crate) fn clear_overworld_sprites(mut commands: Commands, q: Query<Entity, With<WorldEntity>>) {
    for e in &q {
        commands.entity(e).despawn();
    }
}

/// Move + pivot the overworld camera: **mouse** left/right-drag orbits, wheel
/// zooms; **touch** two-finger drag orbits, pinch zooms. Both nudge the live
/// `Look` (yaw/pitch/dist), which `hd2d_follow` then applies while keeping the
/// player centred. Camera-relative facing keeps the hero oriented as you orbit.
#[allow(clippy::too_many_arguments)]
pub(crate) fn overworld_camera_control(
    mut look: ResMut<hd2d::Look>,
    overlay: Res<Overlay>,
    buttons: Res<ButtonInput<MouseButton>>,
    mut motion: EventReader<MouseMotion>,
    mut wheel: EventReader<MouseWheel>,
    touches: Res<Touches>,
    mut pinch: Local<Option<f32>>,
    mut two_mid: Local<Option<Vec2>>,
) {
    // Don't pivot while a full-screen overlay (inventory / level-up) is up.
    if overlay.kind.is_some() {
        motion.clear();
        wheel.clear();
        return;
    }
    let orbit = |look: &mut hd2d::Look, dx: f32, dy: f32| {
        look.cam_yaw -= dx * 0.4;
        // Cap the tilt well below overhead. The sprites are upright billboards that
        // only yaw to face the camera, so their apparent height falls off as ~cos(pitch)
        // as you tilt down — at 60° they're half-height (a sliver) and near overhead they
        // vanish edge-on. Capping at 50° keeps them at ~64% height and clearly readable
        // at every allowed angle (the HD-2D convention: never let the camera go overhead).
        look.cam_pitch = (look.cam_pitch + dy * 0.4).clamp(10.0, 50.0);
    };
    let zoom = |look: &mut hd2d::Look, d: f32| {
        look.cam_dist = (look.cam_dist + d).clamp(8.0, 60.0);
    };

    // Mouse: drag (either button) to orbit, wheel to zoom.
    if buttons.pressed(MouseButton::Left) || buttons.pressed(MouseButton::Right) {
        let mut d = Vec2::ZERO;
        for e in motion.read() {
            d += e.delta;
        }
        if d != Vec2::ZERO {
            orbit(&mut look, d.x, d.y);
        }
    } else {
        motion.clear();
    }
    for e in wheel.read() {
        zoom(&mut look, -e.y * 2.0);
    }

    // Touch: two-finger pinch to zoom + two-finger drag to orbit.
    let pts: Vec<Vec2> = touches.iter().map(|t| t.position()).collect();
    if pts.len() == 2 {
        let dist = pts[0].distance(pts[1]);
        let mid = (pts[0] + pts[1]) * 0.5;
        if let Some(prev) = *pinch {
            zoom(&mut look, -(dist - prev) * 0.05);
        }
        if let Some(pm) = *two_mid {
            let dm = mid - pm;
            orbit(&mut look, dm.x, dm.y);
        }
        *pinch = Some(dist);
        *two_mid = Some(mid);
    } else {
        *pinch = None;
        *two_mid = None;
    }
}

#[cfg(test)]
mod tests {
    use super::creature_kind;

    // A creature must resolve to the SAME kind (hence the same sprite) whether it
    // arrives as the overworld's underscored id, the battle's spaced display name, or
    // a champion with an affix prefix — otherwise the field/battle sprites diverge.
    #[test]
    fn creature_kind_canonicalizes_every_form() {
        assert_eq!(creature_kind("forest_bloom_stalker"), "forest_bloom_stalker");
        assert_eq!(creature_kind("Forest Bloom Stalker"), "forest_bloom_stalker");
        assert_eq!(creature_kind("Swift dune_wyrm"), "dune_wyrm");
        assert_eq!(creature_kind("Giant Forest Bloom Stalker"), "forest_bloom_stalker");
        assert_eq!(creature_kind("dune_wyrm"), "dune_wyrm");
        assert_eq!(creature_kind("Sporeling"), "sporeling");
    }
}
