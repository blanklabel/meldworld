//! Overworld: movement + camera, snapshot→sprite reconciliation, terrain/walls,
//! chests, HUD/minimap, party-follower entourage, and the perk overlays (lamp,
//! nameplates). Extracted from `main.rs` during the module reorg.

use std::collections::HashSet;

use bevy::prelude::*;

use meld_client::glass;
use bevy::input::mouse::{MouseMotion, MouseWheel};
use bevy::gltf::GltfAssetLabel;

use meld_client::hd2d::{self, CharSprite};
use meld_client::net::{ClientCmd, EntityKind};

use super::*;

// -------------------------------------------------------------- overworld --

/// An overworld action reachable by a keyboard key OR an on-screen (touch) button.
#[derive(Clone, Copy, PartialEq)]
pub(crate) enum OverworldAct {
    /// Open the inventory/menu overlay (where distance, biome, the backpack and
    /// "Return to town" live). Keyboard equivalent: C / I, or tapping your own
    /// character.
    Menu,
}


/// The smithing bar's frame: a bar of RED. Hidden unless a heat is open. Spawned on
/// both the overworld and the city, since the anvil is in one and the stations in the
/// other, and a heat has to look the same at either.
#[derive(Component)]
pub(crate) struct HeatBar;
/// The YELLOW band on it — the hot part of this blow, positioned from the server's band.
#[derive(Component)]
pub(crate) struct HeatBarBand;
/// The hammer: where the marker is right now.
#[derive(Component)]
pub(crate) struct HeatBarMark;

/// Marks a tappable on-screen action button (touch-native via Bevy UI `Interaction`).
#[derive(Component)]
pub(crate) struct TouchActionButton(pub(crate) OverworldAct);

/// Spawn the smithing bar (red track, yellow band, marker) as real coloured nodes —
/// a text bar could only shade it, and "strike the yellow" has to BE yellow.
pub(crate) fn spawn_heat_bar(p: &mut ChildSpawnerCommands) {
    p.spawn((
        HeatBar,
        Node {
            display: Display::None,
            width: Val::Px(420.0),
            height: Val::Px(16.0),
            margin: UiRect::top(Val::Px(6.0)),
            border: UiRect::all(Val::Px(1.0)),
            position_type: PositionType::Relative,
            ..default()
        },
        BorderColor(Color::srgba(1.0, 0.85, 0.7, 0.5)),
        // The cold bar: red, and dark enough that the yellow reads off it at a glance.
        BackgroundColor(Color::srgb(0.42, 0.08, 0.06)),
    ))
    .with_children(|bar| {
        bar.spawn((
            HeatBarBand,
            Node {
                position_type: PositionType::Absolute,
                left: Val::Percent(0.0),
                width: Val::Percent(0.0),
                height: Val::Percent(100.0),
                ..default()
            },
            BackgroundColor(Color::srgb(1.0, 0.83, 0.25)),
        ));
        bar.spawn((
            HeatBarMark,
            Node {
                position_type: PositionType::Absolute,
                left: Val::Percent(0.0),
                width: Val::Px(3.0),
                height: Val::Percent(100.0),
                ..default()
            },
            BackgroundColor(Color::WHITE),
        ));
    });
}

/// Keep the bar in step with the open heat: show/hide it, put the yellow where this
/// blow's band is, and sweep the marker.
pub(crate) fn update_heat_bar(
    heat: Res<HeatUi>,
    time: Res<Time>,
    mut frame: Query<&mut Node, (With<HeatBar>, Without<HeatBarBand>, Without<HeatBarMark>)>,
    mut band: Query<&mut Node, (With<HeatBarBand>, Without<HeatBarMark>)>,
    mut mark: Query<&mut Node, With<HeatBarMark>>,
) {
    let open = heat.job_id.is_some().then(|| heat.band()).flatten();
    for mut n in &mut frame {
        n.display = if open.is_some() { Display::Flex } else { Display::None };
    }
    let Some((lo, hi)) = open else { return };
    for mut n in &mut band {
        n.left = Val::Percent((lo * 100.0) as f32);
        n.width = Val::Percent(((hi - lo) * 100.0) as f32);
    }
    let at = heat.marker(time.elapsed_secs_f64());
    for mut n in &mut mark {
        n.left = Val::Percent((at * 100.0) as f32);
    }
}

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
                Text::new(String::new()),
                TextFont {
                    font_size: 20.0,
                    ..default()
                },
                TextColor(Color::srgb(0.9, 0.92, 1.0)),
            ));
            spawn_heat_bar(p);
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
                // Just Menu, which is where going home lives. Interact and Boon used to sit
                // here too, duplicating prompts that now live over the player's head — and
                // each prompt there is its own tappable chip, so touch loses nothing.
                action_button(bar, OverworldAct::Menu, "\u{f0214} Menu"); // list icon
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
                BackgroundColor(glass::GLASS_THIN),
            ));
            // How deep you are, under the map that earned it. Distance is the whole
            // difficulty axis, so it belongs beside the reading of the ground rather
            // than in a corner of its own — and it shows only when the Explorer's map
            // does, because without one you are meant to be guessing.
            p.spawn((
                MinimapDistance,
                Text::new(String::new()),
                TextFont { font_size: 15.0, ..default() },
                TextColor(glass::TEXT),
                Node {
                    position_type: PositionType::Absolute,
                    right: Val::Px(14.0),
                    top: Val::Px(160.0),
                    display: Display::None,
                    ..default()
                },
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
            BackgroundColor(glass::GLASS),
        ))
        .with_children(|b| {
            b.spawn((
                Text::new(label.to_string()),
                TextFont { font_size: 16.0, ..default() },
                TextColor(Color::srgb(0.88, 0.92, 1.0)),
            ));
        });
}

/// The corner bar's one remaining button: Menu. Interact and Boon moved onto the plate over
/// the player's head, where the prompts are — a corner button that had to guess which action
/// you meant was a worse version of tapping the thing you are looking at.
pub(crate) fn touch_action_buttons(
    q: Query<(&Interaction, &TouchActionButton), Changed<Interaction>>,
    mut overlay: ResMut<Overlay>,
    mut tab: ResMut<OverlayTab>,
) {
    for (interaction, btn) in &q {
        if *interaction != Interaction::Pressed {
            continue;
        }
        match btn.0 {
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
    dungeon: Res<world_render::DungeonSceneRes>,
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
    // DG-6b: inside a dungeon, pull the camera TIGHT and steeper — a Dragon-Quest
    // dungeon rig where you see only the room around you and must move to explore,
    // instead of the pulled-back overworld survey that reveals the whole floor. Close
    // fog seals the view so a room reads as an enclosed space. (Look is cheap to clone
    // per-frame; overworld path is unchanged.)
    let mut look_eff = (*look).clone();
    if dungeon.active {
        look_eff.cam_dist = 13.0;
        look_eff.cam_pitch = 49.0;
        look_eff.focus = 13.0;
        look_eff.fog_start = 20.0;
        look_eff.fog_end = 64.0;
    }
    let look = &look_eff;
    if let Ok((mut t, mut proj, bloom, dof, fog)) = cam_q.single_mut() {
        let mut cam = hd2d::camera_transform(look, target, time.elapsed_secs());
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
            look,
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

/// The bodies a dungeon door within reach wants held on plates at once, when that is
/// more than one. `None` when there is no such door nearby.
pub(crate) fn coop_door_near(world: &Overworld, me: Option<(f32, f32)>) -> Option<u8> {
    let (mx, my) = me?;
    world
        .entities
        .values()
        .filter(|e| e.kind == EntityKind::Entrance && e.bodies_required > 1)
        .filter(|e| (e.x - mx).powi(2) + (e.y - my).powi(2) <= 2.25)
        .map(|e| e.bodies_required)
        .max()
}

/// The overworld HUD shows ONLY what you can do *right now*: the prompt for whatever
/// [E] would act on, or the fact that you are mid-channel. Nothing otherwise — a
/// permanent control list is noise a player stops reading on the second dive, so the
/// controls live in the menu's Guide column and the backpack in its own. Distance
/// reads under the Explorer's minimap ([`update_minimap_distance`]) and, for everyone,
/// on the menu's Map column (see [`update_run_stats`]).
/// (Passive-perk hints like "Regen"/"Bulwark" were dropped too: the party always has a
/// Resonant, so "Regen" was always on and read as a stuck status badge.)
pub(crate) fn update_overworld_hud(
    world: Res<Overworld>,
    session: Res<Session>,
    notice: Res<Notice>,
    time: Res<Time>,
    station: Res<StationUi>,
    craft: Res<CraftData>,
    inv: Res<InventoryData>,
    heat: Res<HeatUi>,
    mut q: Query<&mut Text, With<HudText>>,
) {
    let Ok(mut t) = q.single_mut() else { return };
    // A heat in progress owns the HUD: the player is mid-blow and everything else can
    // wait until the metal is worked.
    if let Some(bar) = heat_line(&heat, time.elapsed_secs_f64()) {
        if **t != bar {
            **t = bar;
        }
        return;
    }
    // An open field bench IS the HUD while it is open — it is a panel the player is
    // standing in, not a hint about what they could press.
    if let Some(bench) = station_line(&station, &craft, &inv) {
        let line = match notice.live(time.elapsed_secs_f64()) {
            Some(why) => format!("{bench}\n\u{f0026} {why}"),
            None => bench,
        };
        if **t != line {
            **t = line;
        }
        return;
    }
    // A refusal the player just earned outranks the prompt: they pressed a key and are
    // owed the reason before being told what they could press next.
    if let Some(why) = notice.live(time.elapsed_secs_f64()) {
        let line = format!("\u{f0026} {why}");
        if **t != line {
            **t = line;
        }
        return;
    }
    // The interact prompt, the boon prompt and the channel bar are NOT here any more: they
    // are over the player's head (`update_action_hud`), which is where you are looking while
    // you gather. A prompt in the corner is a prompt you read once and then stop seeing.
    let mut line = String::new();
    // A door that wants more bodies than one says so BEFORE you are inside it (#190):
    // there is no Town Portal in a dungeon, so a party that finds out at the gate has
    // walked the whole way for nothing.
    let me_pos = world.entities.get(&session.player_id).map(|e| (e.x, e.y));
    if let Some(n) = coop_door_near(&world, me_pos) {
        let warn = format!(
            "{} needs {n} heroes on its plates at once",
            '\u{f06cc}'
        );
        line = if line.is_empty() { warn } else { format!("{line}  -  {warn}") };
    }
    if **t != line {
        **t = line;
    }
}

/// How full the channel bar is, as a percentage, `phase` seconds into a channel whose
/// payout lands every `fill_ms`. Wraps, because a gather pays repeatedly — each fill is
/// one unit, so the bar emptying IS the unit landing.
pub(crate) fn channel_fill_pct(phase: f32, fill_ms: u64) -> f32 {
    let fill_secs = (fill_ms as f32 / 1000.0).max(0.05);
    ((phase % fill_secs) / fill_secs * 100.0).clamp(0.0, 100.0)
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

/// Overworld *actions*. **[E] is the one interact key** — it does whatever the world
/// is offering at your feet (gather, open, descend, extract at the deep portal, join a
/// fight), and stops a channel if one is running. There is **no hotkey for going home**:
/// a Town Portal is an item, so spending one is an explicit choice in the menu's Map
/// column ([`crate::menu::return_to_town_click`]) rather than a key you have to be told
/// about. Movement is device-agnostic in [`gather_steer`] + [`emit_move`]; the touch bar
/// mirrors the interact key.
#[allow(clippy::too_many_arguments)]
pub(crate) fn overworld_input(
    keys: Res<ButtonInput<KeyCode>>,
    net: NonSend<NetRes>,
    autoplay: Res<Autoplay>,
    world: Res<Overworld>,
    session: Res<Session>,
    overlay: Res<Overlay>,
    time: Res<Time>,
    inv: Res<InventoryData>,
    mut station: ResMut<StationUi>,
    mut auto_cooldown: Local<f32>,
) {
    // While a field bench is open its own keys own the keyboard (the bench's [E] is
    // "leave", handled by `station_input`), so [E] must not also re-open it.
    if station.open.is_some() {
        return;
    }
    if overlay.kind.is_some() {
        return;
    }

    // [E] while channeling puts the tool down (keeping every unit already banked) —
    // the keyboard twin of clicking away.
    if session.channeling {
        if keys.just_pressed(KeyCode::KeyE) {
            net.0.send(ClientCmd::CancelHarvest);
        }
        return;
    }

    // [N] asks the bench in reach for its temporary boon. A one-press favour, so it is
    // its own key and its own prompt rather than a row inside a screen.
    if keys.just_pressed(KeyCode::KeyN) {
        ask_for_boon(&net.0, &world, &session, &inv);
        return;
    }

    // Everything you do TO the world is [E]. Autoplay takes whatever is offered so
    // demos still gather, extract and descend on their own.
    let target = interact_target(&world, &session);
    let Some(target) = target else { return };
    let pressed = keys.just_pressed(KeyCode::KeyE);
    if !pressed {
        // A co-op door is a deliberate answer to the prompt, never an unattended one
        // (#190): a dungeon takes no Town Portal, and autoplay cannot muster the bodies
        // its plates want.
        if let Interact::EnterDungeon { entity_id } = &target {
            if world.entities.get(entity_id).is_some_and(|e| e.bodies_required > 1) {
                return;
            }
        }
        // Autoplay acts on its own, but throttled: firing every frame would flood the
        // server while the channel it just opened is still starting. A throttle rather
        // than a once-per-target latch, so a send the server refused is retried instead
        // of wedging the demo in front of something it never managed to touch.
        *auto_cooldown -= time.delta_secs();
        if !autoplay.0 || *auto_cooldown > 0.0 {
            return;
        }
        *auto_cooldown = AUTOPLAY_INTERACT_THROTTLE;
    }
    match target {
        Interact::JoinFight => net.0.send(ClientCmd::JoinBattle),
        Interact::Harvest { entity_id, .. } => net.0.send(ClientCmd::Harvest { entity_id }),
        Interact::OpenChest { entity_id } => net.0.send(ClientCmd::OpenChest { entity_id }),
        Interact::EnterDungeon { entity_id } => net.0.send(ClientCmd::EnterDungeon { entity_id }),
        // A station is a bench, not a one-shot: [E] opens it and the keys work from
        // there, the same way the city anvil does.
        Interact::UseStation { entity_id, kind, jobs } => {
            // A still needs the recipe book; in the field the client may never have
            // opened the city's Alembic, so ask for it on the way in.
            if kind == "alembic" {
                net.0.fetch_recipes();
            }
            station.open = Some(entity_id);
            station.kind = kind;
            station.jobs = jobs;
        }
        Interact::Extract => net.0.send(ClientCmd::Extract),
    }
}

/// What pressing **[E]** would do at the avatar's current position. One interact key
/// for the whole overworld: the world tells you what it offers, rather than the player
/// memorising a key per object. `None` = nothing in reach, and the HUD stays silent.
///
/// Priority is urgency first, then proximity: a teammate's fight is transient and
/// closes, so it outranks scenery that will still be there in ten seconds.
#[derive(Clone, PartialEq)]
pub(crate) enum Interact {
    JoinFight,
    Harvest { entity_id: String, label: String },
    OpenChest { entity_id: String },
    EnterDungeon { entity_id: String },
    /// Work at a field station someone raised. `jobs` is what it has left, so the
    /// prompt can say whether it is worth walking over to.
    UseStation { entity_id: String, kind: String, jobs: u8 },
    Extract,
}

impl Interact {
    /// What this action IS, in the player's words. Shared by the keyboard prompt and
    /// the touch button so the two can never describe the same key differently.
    pub(crate) fn verb(&self) -> String {
        match self {
            Interact::JoinFight => "Join the fight".into(),
            Interact::Harvest { label, .. } => format!("Gather {label}"),
            Interact::OpenChest { .. } => "Open the chest".into(),
            Interact::EnterDungeon { .. } => "Descend".into(),
            Interact::UseStation { kind, jobs, .. } => {
                let bench = if kind == "alembic" { "still" } else { "forge" };
                format!("Use the {bench} ({jobs} left)")
            }
            Interact::Extract => "Extract".into(),
        }
    }

    /// The prompt shown while this is in reach. Every line names the SAME key, which
    /// is the point of collapsing the controls onto one.
    /// Which world entity this action is aimed at, if any. `JoinFight` and `Extract` are
    /// aimed at a place rather than a thing, so they have none.
    pub(crate) fn entity_id(&self) -> Option<&str> {
        match self {
            Interact::Harvest { entity_id, .. }
            | Interact::OpenChest { entity_id }
            | Interact::EnterDungeon { entity_id }
            | Interact::UseStation { entity_id, .. } => Some(entity_id),
            Interact::JoinFight | Interact::Extract => None,
        }
    }

    pub(crate) fn prompt(&self) -> String {
        match self {
            Interact::JoinFight => format!("\u{f0817} [E] {}", self.verb()),
            _ => format!("[E] {}", self.verb()),
        }
    }
}

/// The temporary boon on offer at the bench in reach, as `(station id, kind, label)`.
/// A forge sharpens something you are WEARING; a still pours for the whole party. Both
/// last the dive and no longer, which the label says so nobody expects to carry it home.
pub(crate) fn boon_offer(
    world: &Overworld,
    session: &Session,
) -> Option<(String, String, String)> {
    let me = world.entities.get(&session.player_id)?;
    let (id, kind) = world
        .entities
        .iter()
        .filter(|(_, e)| e.kind == EntityKind::Station && e.level == me.level)
        .filter(|(_, e)| ((e.x - me.x).powi(2) + (e.y - me.y).powi(2)).sqrt() <= INTERACT_REACH)
        .map(|(id, e)| (id.clone(), e.name.clone().unwrap_or_default()))
        .next()?;
    let label = if kind == "alembic" {
        "Ask for a tonic (party, this dive)"
    } else {
        "Ask for an edge (this dive)"
    };
    Some((id, kind, label.to_string()))
}

/// The piece the given hero is wearing in the hand — what a smith's edge goes on.
/// `None` when that hero has nothing equipped there, which is the one case where an
/// edge has nothing to bite.
pub(crate) fn worn_piece(inv: &InventoryData, slot: usize) -> Option<String> {
    inv.gear
        .iter()
        .find(|g| g.equipped_hero_slot == Some(slot) && g.slot == "main_hand")
        .or_else(|| inv.gear.iter().find(|g| g.equipped_hero_slot == Some(slot)))
        .map(|g| g.gear_id.clone())
}

/// Seconds autoplay waits between unattended interactions.
const AUTOPLAY_INTERACT_THROTTLE: f32 = 1.0;

/// Reach for interacting with world scenery, in tiles. Generous on purpose — nobody
/// should have to pixel-hunt a doorway.
const INTERACT_REACH: f32 = 2.0;

/// Resolve the best [`Interact`] for the avatar's position.
pub(crate) fn interact_target(world: &Overworld, session: &Session) -> Option<Interact> {
    let me = world.entities.get(&session.player_id)?;
    let me_pos = Some((me.x, me.y));
    if near_fight(world, me_pos) {
        return Some(Interact::JoinFight);
    }
    // Nearest thing in reach on the player's own level wins.
    let mut best: Option<(f32, Interact)> = None;
    for (id, e) in &world.entities {
        let d = ((e.x - me.x).powi(2) + (e.y - me.y).powi(2)).sqrt();
        if d > INTERACT_REACH || e.level != me.level {
            continue;
        }
        let what = match e.kind {
            EntityKind::Resource => Some(Interact::Harvest {
                entity_id: id.clone(),
                label: node_label(e.name.as_deref().unwrap_or("")),
            }),
            EntityKind::Chest if !e.opened => Some(Interact::OpenChest { entity_id: id.clone() }),
            EntityKind::Entrance => Some(Interact::EnterDungeon { entity_id: id.clone() }),
            EntityKind::Station => Some(Interact::UseStation {
                entity_id: id.clone(),
                kind: e.name.clone().unwrap_or_default(),
                jobs: e.bodies_required,
            }),
            EntityKind::Portal => Some(Interact::Extract),
            _ => None,
        };
        if let Some(w) = what {
            if best.as_ref().map(|(bd, _)| d < *bd).unwrap_or(true) {
                best = Some((d, w));
            }
        }
    }
    best.map(|(_, w)| w)
}

/// A resource node's content id as a player-facing word (`bloom_herb` → "Bloom Herb").
/// An unnamed node still prompts — just generically, rather than showing a blank.
fn node_label(kind: &str) -> String {
    if kind.is_empty() {
        return "it".to_string();
    }
    let base = kind.split(':').next_back().unwrap_or(kind);
    base.split('_')
        .map(title_case)
        .collect::<Vec<_>>()
        .join(" ")
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
            let cur = InterpSample { x: e.x, y: e.y, t: now };
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
                // A boss with authored 8-direction animation frames (`bosses/<key>/`,
                // e.g. a dungeon boss `mob:hollowbishop:hostile`) renders as an
                // ANIMATED, camera-facing `CharSprite` — idle breathing + turning as
                // the camera orbits — just like a hero, instead of a single frozen
                // billboard. Regular creatures (single-PNG art) keep the billboard.
                let kind = creature_kind(e.name.as_deref().unwrap_or(""));
                if let Some(frames) = wa.boss_frames(&kind) {
                    let scale = match e.encounter_class.as_deref() {
                        Some("gatekeeper") => 2.6,
                        _ => 2.0,
                    };
                    let tint = if e.battling {
                        Color::srgb(1.5, 0.6, 0.5) // fighting → hot
                    } else {
                        Color::srgb(1.25, 1.12, 1.06) // looming, faintly warm
                    };
                    spawn_boss_char(&mut commands, &mut mats, &wa, &look, id, e, frames, scale, tint);
                    continue;
                }
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
                let root = spawn_billboard_entity(
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
                add_ground_ring(&mut commands, &wa, root);
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
            EntityKind::Station => {
                // A field forge someone raised. Warm-lit with the portal's ground ring,
                // because it has to read as "a thing that is HERE now" against terrain
                // the player has already walked past.
                let root = spawn_billboard_entity(
                    &mut commands,
                    &mut mats,
                    &wa,
                    id,
                    e,
                    wa.portal_sprite.clone(),
                    1.8,
                    Color::srgb(1.5, 1.0, 0.55),
                    0.25,
                );
                add_ground_ring(&mut commands, &wa, root);
            }
            EntityKind::Stair => {
                // The way down, and it has to out-read the walls around it: dungeon
                // wall blocks stand 3.2 units, so a 1.6 marker was hidden behind the
                // nearest corridor from every angle a player actually looks from. Same
                // arch as the exit but cool-lit, with the exit's ground ring so it
                // glows through a dim floor — a floor whose way down you cannot see is
                // a floor you wander until something kills you.
                let root = spawn_billboard_entity(
                    &mut commands,
                    &mut mats,
                    &wa,
                    id,
                    e,
                    wa.portal_sprite.clone(),
                    2.8,
                    Color::srgb(0.75, 1.15, 1.5),
                    0.3,
                );
                add_ground_ring(&mut commands, &wa, root);
            }
            EntityKind::Trap => {
                // A trap the party's Shifter has read. Drawn low and hot-red so it
                // reads as "do not stand here" without hiding the floor — the server
                // only ever sends the armed ones inside the Runner's sense.
                spawn_billboard_entity(
                    &mut commands,
                    &mut mats,
                    &wa,
                    id,
                    e,
                    wa.prop_sprites
                        .get("marker_target_marker")
                        .cloned()
                        .unwrap_or_default(),
                    0.9,
                    Color::srgb(1.4, 0.35, 0.3),
                    0.2,
                );
            }
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
    //
    // Only for UNTEXTURED materials — the 3D harvest models. Emissive is added flat across a
    // surface, ignoring its texture, so on a textured billboard ANY meaningful value paints
    // the whole quad one colour: the pulse did not glow the sprite, it erased it, and every
    // node was the same white blob whatever the art said. Turning the number down (my first
    // attempt) could not fix that, because the mechanism was wrong rather than the magnitude.
    //
    // The billboards therefore do not breathe. They do not need to: the node art is
    // distinctive, the Explorer's minimap dots point them out, and being able to tell bog
    // myrrh from peat iron matters more than a glow.
    const GLOW_FLOOR: f32 = 0.125;
    const GLOW_SWING: f32 = 0.55;
    let phase = (time.elapsed_secs() * std::f32::consts::TAU * 0.4).sin() * 0.5 + 0.5;
    let strength = GLOW_FLOOR + GLOW_SWING * phase;
    for root in &roots {
        for e in std::iter::once(root).chain(child_q.iter_descendants::<Children>(root)) {
            let Ok(mm) = mat_of.get(e) else { continue };
            let Some(m) = mats.get_mut(&mm.0) else {
                continue;
            };
            if m.base_color_texture.is_some() {
                // A sprite shows itself. Make sure nothing is washing it out.
                if m.emissive != LinearRgba::BLACK {
                    m.emissive = LinearRgba::BLACK;
                }
                continue;
            }
            let c = m.base_color.to_linear();
            m.emissive = LinearRgba::rgb(c.red * strength, c.green * strength, c.blue * strength);
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

/// Spawn a boss as an animated, camera-facing [`CharSprite`] (its authored
/// 8-direction idle/walk frames, driven by [`hd2d::animate_chars`]) instead of a
/// single static billboard — so a dungeon boss breathes and turns like a hero. A
/// bigger `scale` makes it loom; no lamp/glow (that's the local player's).
#[allow(clippy::too_many_arguments)]
pub(crate) fn spawn_boss_char(
    commands: &mut Commands,
    mats: &mut Assets<StandardMaterial>,
    wa: &WorldAssets,
    look: &hd2d::Look,
    id: &str,
    e: &OwEntity,
    frames: &hd2d::CharacterFrames,
    scale: f32,
    tint: Color,
) {
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
            p.spawn((
                Mesh3d(wa.sprite_quad.clone()),
                MeshMaterial3d(mat),
                Transform::from_xyz(0.0, look.sprite_y * scale, 0.0).with_scale(Vec3::splat(scale)),
                hd2d::Billboard,
            ));
            p.spawn((
                Mesh3d(wa.shadow_mesh.clone()),
                MeshMaterial3d(wa.shadow_mat.clone()),
                Transform::from_xyz(0.0, 0.02, 0.0)
                    .with_rotation(Quat::from_rotation_x(-std::f32::consts::FRAC_PI_2))
                    .with_scale(Vec3::new(scale, scale * 0.55, scale)),
            ));
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

/// The depth readout under the minimap.
#[derive(Component)]
pub(crate) struct MinimapDistance;

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
    let intel = perks.0.hunter_intel;
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
                            BackgroundColor(glass::SCRIM),
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

/// Rebuild the corner minimap. It is the EXPLORER's — `compute_perks` grants
/// `explorer_map` only with one in the party (the order whose vision is "a world
/// known" carries the map); the Shifter contributes just the dungeon-door dots.
/// The panel shows/hides by the map tier; dots plot entities within
/// `explorer_map_radius` of the player — mobs + portal (tier ≥1), chests (≥2),
/// harvestables (≥3), self at centre.
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
    let tier = perks.0.explorer_map;
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
    let radius = perks.0.explorer_map_radius.max(1.0);
    let scale = R / radius;
    let shifter_sense = perks.0.shifter_dungeon_radius;
    commands.entity(root).with_children(|p| {
        // The player, dead centre.
        spawn_dot(p, HALF, HALF, 6.0, Color::srgb(1.0, 1.0, 1.0));
        for e in world.entities.values() {
            let (col, size) = match e.kind {
                EntityKind::Monster => (Color::srgb(1.0, 0.4, 0.35), 5.0),
                EntityKind::Portal => (Color::srgb(0.4, 0.85, 1.0), 6.0),
                EntityKind::Chest if tier >= 2 => (Color::srgb(1.0, 0.82, 0.3), 5.0),
                EntityKind::Resource if tier >= 3 => (Color::srgb(0.5, 0.95, 0.5), 4.0),
                // A dungeon door is the SHIFTER's contribution to the map, not the
                // Explorer's: Shift-sense reads the instability a doorway leaks, so
                // entrances plot only while a Runner is in the party, and only inside
                // that Runner's sense radius.
                EntityKind::Entrance if shifter_sense > 0.0 => {
                    (Color::srgb(0.85, 0.55, 1.0), 7.0)
                }
                _ => continue,
            };
            // An entrance is limited by the Runner's sense, everything else by the
            // map's reach.
            if e.kind == EntityKind::Entrance {
                let d = ((e.x - me.x).powi(2) + (e.y - me.y).powi(2)).sqrt();
                if d > shifter_sense {
                    continue;
                }
            }
            let (dx, dy) = ((e.x - me.x) * scale, (e.y - me.y) * scale);
            if dx.abs() > R || dy.abs() > R {
                continue; // outside the minimap's world radius
            }
            spawn_dot(p, HALF + dx, HALF + dy, size, col);
        }
    });
}

/// The depth readout under the minimap: distance, its tier, and the biome it is in.
/// Rides the Explorer's map perk, so it appears and vanishes with the panel above it.
pub(crate) fn update_minimap_distance(
    perks: Res<PerksRes>,
    stats: Res<RunStats>,
    mut q: Query<(&mut Text, &mut Node), With<MinimapDistance>>,
) {
    let Ok((mut text, mut node)) = q.single_mut() else { return };
    if perks.0.explorer_map == 0 {
        node.display = Display::None;
        return;
    }
    node.display = Display::Flex;
    let line = format!("{} m  \u{b7}  T{}  \u{b7}  {}", stats.distance, stats.tier, stats.biome);
    if **text != line {
        **text = line;
    }
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
) -> Entity {
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
        })
        .id()
}

/// Lay a flat, faintly emissive disc on the ground under an already-spawned world
/// entity, so a way out reads as a glowing marker from across the floor rather than
/// only at arm's length.
///
/// It goes on as a CHILD deliberately. Spawned as a second root carrying the same
/// [`WorldEntity`] id, `sync_entities`' one-sprite-per-id guard would despawn one of
/// the pair on the very next frame — arbitrarily the arch or the ring — so a portal
/// rendered as half of itself, and a portal reduced to its flat ground disc is a
/// portal a player walks straight past.
fn add_ground_ring(commands: &mut Commands, wa: &WorldAssets, root: Entity) {
    commands.entity(root).with_children(|p| {
        p.spawn((
            Mesh3d(wa.portal_mesh.clone()),
            MeshMaterial3d(wa.portal_mat.clone()),
            Transform::from_xyz(0.0, 0.08, 0.0)
                .with_rotation(Quat::from_rotation_x(std::f32::consts::FRAC_PI_2)),
        ));
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
    // DG-6b: dungeon interior maze walls. Rendered as a SOLID, TILE-FILLING wall block
    // (a full unit cube, slightly over-sized so adjacent wall cells merge into one
    // continuous wall) so a floor reads as enclosed rooms + corridors you explore — not
    // scattered rocks. A door cell is a shorter, browner block (a legible opening). The
    // tight dungeon camera (see `hd2d_follow`) keeps the hero visible over the near
    // walls. Themed: mossy/basalt/ice/sand stone per biome; a forest dungeon uses a
    // deep-green mossy stone so its walls still read as walls under the canopy.
    if name == "dungeon_wall" || name == "dungeon_door" {
        let is_door = name == "dungeon_door";
        // Wear the tiling cobblestone masonry texture (so walls read as fitted stone,
        // not flat blocks), multiplied by a per-biome tint. The texture repeats up the
        // wall face so the stones stay ~square rather than stretching.
        let tint = if is_door {
            Color::srgb(0.60, 0.42, 0.26) // timber-brown door
        } else {
            match dungeon_theme {
                "forest" => Color::srgb(0.56, 0.68, 0.50), // mossy stone
                "desert" => Color::srgb(0.86, 0.75, 0.54), // sandstone
                "ashfall" => Color::srgb(0.52, 0.44, 0.46), // basalt
                "tundra" => Color::srgb(0.82, 0.88, 0.96), // ice-rimed stone
                "mire" => Color::srgb(0.56, 0.66, 0.56),   // wet mossy stone
                _ => Color::srgb(0.74, 0.74, 0.80),        // grey dungeon stone
            }
        };
        let height: f32 = if is_door { 2.2 } else { 3.2 };
        let mat = mats.add(StandardMaterial {
            base_color: tint,
            base_color_texture: Some(wa.wall_tex.clone()),
            // Repeat the cobblestone ~once per world-unit up the wall.
            uv_transform: bevy::math::Affine2::from_scale_angle_translation(
                Vec2::new(1.0, height.round()),
                0.0,
                Vec2::ZERO,
            ),
            perceptual_roughness: 1.0,
            ..default()
        });
        commands.spawn((
            WorldEntity(id.to_string()),
            Mesh3d(wa.wall_mesh.clone()),
            MeshMaterial3d(mat),
            // Base on the ground (cube is centre-origin, so lift by half height); width
            // 1.04 so neighbours overlap into a seamless wall.
            Transform::from_translation(world_pos(e.x, e.y, height * 0.5))
                .with_scale(Vec3::new(1.04, height, 1.04)),
        ));
        let _ = r;
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

/// Hide the field's decorative scatter when a battle opens.
///
/// The grass blades ([`crate::ambient::GrassBlade`]) and ground props
/// ([`crate::world_render::GroundDetail`]) are a persistent pool that follows the
/// player and is repositioned by systems gated to `Screen::Overworld`. They are not
/// snapshot entities, so `clear_overworld_sprites` never touched them — on entering a
/// battle they simply froze where they stood and kept drawing, which put grass and
/// mushrooms **in front of** the combatants. Their own systems make them visible again
/// on the way back out, so hiding is all this needs to do.
pub(crate) fn hide_field_decor(
    mut grass: Query<&mut Visibility, With<crate::ambient::GrassBlade>>,
    mut props: Query<&mut Visibility, (With<crate::world_render::GroundDetail>, Without<crate::ambient::GrassBlade>)>,
) {
    for mut v in &mut grass {
        *v = Visibility::Hidden;
    }
    for mut v in &mut props {
        *v = Visibility::Hidden;
    }
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

    /// The halo has to know WHICH thing it belongs on. `JoinFight` and `Extract` aim at a
    /// place rather than a thing, so they light nothing — and every other action names its
    /// entity, or the glow would have nowhere to go.
    #[test]
    fn every_thing_shaped_action_names_its_entity() {
        let cases = [
            (Interact::Harvest { entity_id: "n1".into(), label: "Bog Myrrh".into() }, Some("n1")),
            (Interact::OpenChest { entity_id: "c1".into() }, Some("c1")),
            (Interact::EnterDungeon { entity_id: "d1".into() }, Some("d1")),
            (
                Interact::UseStation { entity_id: "s1".into(), kind: "smith".into(), jobs: 2 },
                Some("s1"),
            ),
            (Interact::JoinFight, None),
            (Interact::Extract, None),
        ];
        for (action, want) in cases {
            assert_eq!(
                action.entity_id(),
                want,
                "{} should {} an entity to glow",
                action.verb(),
                if want.is_some() { "name" } else { "name no" }
            );
        }
    }

    use super::*;
    use super::creature_kind;

    fn ent(kind: EntityKind, x: f32, y: f32) -> OwEntity {
        OwEntity {
            x,
            y,
            kind,
            name: Some("bloom_herb".into()),
            faction: None,
            radius: 0.0,
            battling: false,
            level: 0,
            opened: false,
            mob_level: None,
            hp: None,
            max_hp: None,
            encounter_class: None,
            aggression: None,
            bodies_required: 1,
        }
    }

    // [E] is the one interact key, so what it does has to be unambiguous at any spot:
    // nothing in reach → no prompt at all (the HUD stays clean), otherwise the NEAREST
    // thing wins — except a teammate's fight, which is transient and outranks scenery.
    #[test]
    fn one_key_resolves_to_the_nearest_thing_worth_touching() {
        let mut world = Overworld::default();
        let session = Session { player_id: "me".into(), ..Default::default() };
        world.entities.insert("me".into(), ent(EntityKind::Player, 0.0, 0.0));

        // Empty hands → no prompt. A permanent control list is what this replaces.
        assert!(interact_target(&world, &session).is_none());

        // Out of reach stays silent; in reach prompts, and names the material.
        world.entities.insert("res-1".into(), ent(EntityKind::Resource, 40.0, 0.0));
        assert!(interact_target(&world, &session).is_none(), "40 tiles away is not in reach");
        world.entities.insert("res-1".into(), ent(EntityKind::Resource, 1.0, 0.0));
        let t = interact_target(&world, &session).expect("node in reach");
        assert!(matches!(&t, Interact::Harvest { entity_id, .. } if entity_id == "res-1"));
        assert_eq!(t.prompt(), "[E] Gather Bloom Herb");

        // A chest in reach is offered — including a DUNGEON chest, which arrives as the
        // same `chest:<tier>:<open>` snapshot tag as an overworld one.
        world.entities.remove("res-1");
        world.entities.insert("dchest-vault".into(), ent(EntityKind::Chest, 0.5, 0.0));
        assert!(
            matches!(interact_target(&world, &session), Some(Interact::OpenChest { entity_id }) if entity_id == "dchest-vault"),
            "a chest in reach should be offered"
        );
        // An already-opened chest is not offered again.
        let mut done = ent(EntityKind::Chest, 0.5, 0.0);
        done.opened = true;
        world.entities.insert("dchest-vault".into(), done);
        assert!(interact_target(&world, &session).is_none(), "an opened chest is done");
        world.entities.remove("dchest-vault");
        world.entities.insert("res-1".into(), ent(EntityKind::Resource, 1.0, 0.0));

        // A closer portal wins over the node.
        world.entities.insert("portal".into(), ent(EntityKind::Portal, 0.2, 0.0));
        assert!(matches!(interact_target(&world, &session), Some(Interact::Extract)));

        // …but a fight in progress outranks both, because it closes.
        let mut fighter = ent(EntityKind::Player, 1.0, 1.0);
        fighter.battling = true;
        world.entities.insert("ally".into(), fighter);
        assert!(matches!(interact_target(&world, &session), Some(Interact::JoinFight)));

        // Only things on your own level are reachable — a terrace node needs the climb.
        world.entities.remove("ally");
        world.entities.remove("portal");
        let mut up = ent(EntityKind::Resource, 1.0, 0.0);
        up.level = 2;
        world.entities.insert("res-1".into(), up);
        assert!(interact_target(&world, &session).is_none(), "a node a terrace up is not in reach");
    }

    // The bar has to READ as progress: empty at the start of a payout, near-full just
    // before it lands, and wrapping rather than sticking at 100% — because a gather pays
    // repeatedly and each emptying is a unit hitting the backpack.
    #[test]
    fn the_channel_bar_fills_then_wraps_for_the_next_unit() {
        assert_eq!(channel_fill_pct(0.0, 900), 0.0);
        let nearly = channel_fill_pct(0.89, 900);
        assert!(nearly > 95.0, "just before the unit lands it should look full: {nearly}");
        // One full tick later it has wrapped, not stuck.
        assert!(channel_fill_pct(0.90, 900) < 5.0);
        // Halfway through a 2.5s extraction reads as half.
        let half = channel_fill_pct(1.25, 2500);
        assert!((half - 50.0).abs() < 1.0, "{half}");
        // A zero/absent fill length can never divide by zero or overflow the bar.
        assert!((0.0..=100.0).contains(&channel_fill_pct(3.0, 0)));
    }

    // The corner channel bar this used to drive is gone: the bar moved over the player's
    // head (`update_action_hud`), where you are actually looking while you gather. Its fill
    // maths is still covered by `channel_fill_pct` above, and the payout floaters by
    // `harvest_pop_tests`.

    // A button that does nothing reads as broken. When the server refuses ("The vault
    // is sealed — defeat the boss first."), the reason has to reach the screen and then
    // get out of the way.
    // The field's decorative pool is persistent and follows the player, so a battle
    // used to open with grass and mushrooms still drawing — in FRONT of the
    // combatants, because they are billboards nearer the camera than the arena.
    #[test]
    fn a_battle_hides_the_fields_decoration() {
        let mut app = App::new();
        app.add_systems(Update, hide_field_decor);
        let blade = app
            .world_mut()
            .spawn((crate::ambient::GrassBlade::for_test(), Visibility::Visible))
            .id();
        let prop = app
            .world_mut()
            .spawn((crate::world_render::GroundDetail::for_test(), Visibility::Visible))
            .id();
        app.update();
        assert_eq!(
            *app.world().get::<Visibility>(blade).unwrap(),
            Visibility::Hidden,
            "grass must not draw over the combatants"
        );
        assert_eq!(
            *app.world().get::<Visibility>(prop).unwrap(),
            Visibility::Hidden,
            "ground props must not draw over the combatants"
        );
    }

    #[test]
    fn a_refusal_is_shown_and_then_expires() {
        let mut n = Notice::default();
        assert_eq!(n.live(0.0), None, "nothing to say at rest");
        n.say("The vault is sealed - defeat the boss first.", 100.0);
        assert_eq!(n.live(100.0), Some("The vault is sealed - defeat the boss first."));
        assert!(n.live(100.0 + NOTICE_SECS - 0.1).is_some(), "still on screen");
        assert_eq!(n.live(100.0 + NOTICE_SECS + 0.1), None, "and then it gets out of the way");
    }

    #[test]
    fn an_unnamed_node_still_prompts() {
        assert_eq!(node_label("bloom_herb"), "Bloom Herb");
        assert_eq!(node_label("resource:dune_iron"), "Dune Iron");
        assert_eq!(node_label(""), "it");
    }

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

// ------------------------------------------------------------ explored map --

/// World units per map cell. The map is a memory of where a party has BEEN, so its
/// grain is a stride rather than a step: fine enough that a corridor reads as a
/// corridor, coarse enough that a whole dive fits one panel.
pub(crate) const MAP_CELL: f32 = 6.0;

/// What a remembered cell holds. A cell keeps only its most notable landmark —
/// the portal you are trying to reach outranks a bush you walked past.
#[derive(Clone, Copy, PartialEq, Eq, Debug, PartialOrd, Ord)]
pub(crate) enum Landmark {
    Resource,
    Chest,
    Entrance,
    Portal,
}

/// This run's map: the cells the party has walked and the landmarks it has seen.
/// Client-side by design — it is a record of what THIS player witnessed, not world
/// state, so the server has nothing to say about it (CANON §S is about authority
/// over the world, and a memory of a walk is not the world).
#[derive(Resource, Default)]
pub(crate) struct ExploredMap {
    pub visited: std::collections::HashSet<(i32, i32)>,
    pub seen: HashMap<(i32, i32), Landmark>,
    /// Where the avatar is right now, in world units.
    pub here: (f32, f32),
    pub walked: bool,
}

impl ExploredMap {
    pub(crate) fn forget(&mut self) {
        self.visited.clear();
        self.seen.clear();
        self.here = (0.0, 0.0);
        self.walked = false;
    }
}

pub(crate) fn map_cell(x: f32, y: f32) -> (i32, i32) {
    ((x / MAP_CELL).floor() as i32, (y / MAP_CELL).floor() as i32)
}

/// Record where the party has been and what it has seen. Gated by the Explorer's
/// map perk on BOTH counts: without one in the party nothing is drawn, so nothing
/// needs remembering, and a landmark is only remembered from inside the map's own
/// reach — otherwise the map would know the whole instance the moment it loaded,
/// which is the opposite of exploring.
pub(crate) fn remember_explored(
    perks: Res<PerksRes>,
    world: Res<Overworld>,
    session: Res<Session>,
    mut map: ResMut<ExploredMap>,
) {
    let tier = perks.0.explorer_map;
    if tier == 0 {
        return;
    }
    let Some(me) = world.entities.get(&session.player_id) else {
        return;
    };
    map.here = (me.x, me.y);
    map.walked = true;
    map.visited.insert(map_cell(me.x, me.y));
    let reach = perks.0.explorer_map_radius.max(1.0);
    let shifter_sense = perks.0.shifter_dungeon_radius;
    for e in world.entities.values() {
        // Creatures roam, so a remembered mob dot would be a lie the moment it moved.
        // Only what stays put earns a place on the map.
        let what = match e.kind {
            EntityKind::Portal => Landmark::Portal,
            EntityKind::Chest if tier >= 2 => Landmark::Chest,
            EntityKind::Resource if tier >= 3 => Landmark::Resource,
            EntityKind::Entrance if shifter_sense > 0.0 => Landmark::Entrance,
            _ => continue,
        };
        let limit = if what == Landmark::Entrance { shifter_sense } else { reach };
        if ((e.x - me.x).powi(2) + (e.y - me.y).powi(2)).sqrt() > limit {
            continue;
        }
        let cell = map_cell(e.x, e.y);
        let slot = map.seen.entry(cell).or_insert(what);
        *slot = (*slot).max(what);
    }
}

/// The rectangle the map has to cover, in world units, always including where the
/// avatar stands so "you" never falls off the edge of your own map.
pub(crate) fn map_bounds(map: &ExploredMap) -> (f32, f32, f32, f32) {
    let mut min = (map.here.0, map.here.1);
    let mut max = min;
    for (cx, cy) in map.visited.iter().chain(map.seen.keys()) {
        let (x, y) = (*cx as f32 * MAP_CELL, *cy as f32 * MAP_CELL);
        min.0 = min.0.min(x);
        min.1 = min.1.min(y);
        max.0 = max.0.max(x + MAP_CELL);
        max.1 = max.1.max(y + MAP_CELL);
    }
    (min.0, min.1, max.0, max.1)
}

/// Project a world point into panel pixels, fitting the walked rectangle inside
/// `(w, h)` on ONE scale for both axes — a map that stretched each axis to fill
/// the panel would report a straight march as a diagonal.
pub(crate) fn map_to_px(
    x: f32,
    y: f32,
    bounds: (f32, f32, f32, f32),
    w: f32,
    h: f32,
) -> (f32, f32) {
    let (span_x, span_y) = ((bounds.2 - bounds.0).max(MAP_CELL), (bounds.3 - bounds.1).max(MAP_CELL));
    let scale = (w / span_x).min(h / span_y);
    let (draw_w, draw_h) = (span_x * scale, span_y * scale);
    (
        (w - draw_w) / 2.0 + (x - bounds.0) * scale,
        (h - draw_h) / 2.0 + (y - bounds.1) * scale,
    )
}

/// The colour a landmark plots in, matching the corner minimap's palette so the
/// two readings of the same world agree.
pub(crate) fn landmark_color(what: Landmark) -> Color {
    match what {
        Landmark::Portal => Color::srgb(0.4, 0.85, 1.0),
        Landmark::Entrance => Color::srgb(0.85, 0.55, 1.0),
        Landmark::Chest => Color::srgb(1.0, 0.82, 0.3),
        Landmark::Resource => Color::srgb(0.5, 0.95, 0.5),
    }
}

#[cfg(test)]
mod explored_map_tests {
    use super::*;

    fn at(kind: EntityKind, x: f32, y: f32) -> OwEntity {
        OwEntity {
            x,
            y,
            kind,
            name: None,
            faction: None,
            radius: 0.0,
            battling: false,
            level: 0,
            opened: false,
            mob_level: None,
            hp: None,
            max_hp: None,
            encounter_class: None,
            aggression: None,
            bodies_required: 1,
        }
    }

    fn app_with(perks: meld_client::net::PerksLine) -> App {
        let mut app = App::new();
        app.insert_resource(PerksRes(perks));
        app.insert_resource(Session { player_id: "me".into(), ..Default::default() });
        app.init_resource::<Overworld>();
        app.init_resource::<ExploredMap>();
        app.add_systems(Update, remember_explored);
        app
    }

    fn walk_to(app: &mut App, x: f32, y: f32) {
        app.world_mut()
            .resource_mut::<Overworld>()
            .entities
            .insert("me".into(), at(EntityKind::Player, x, y));
        app.update();
    }

    // The map is the Explorer's. Without one there is nothing to draw, so there is
    // nothing to record either — and a recorded walk from a party that cannot read it
    // would light up the moment an Explorer joined a LATER run.
    #[test]
    fn without_an_explorer_nothing_is_remembered() {
        let mut app = app_with(meld_client::net::PerksLine::default());
        walk_to(&mut app, 10.0, 4.0);
        let map = app.world().resource::<ExploredMap>();
        assert!(!map.walked);
        assert!(map.visited.is_empty());
    }

    #[test]
    fn walking_fills_cells_and_landmarks_are_only_learned_within_reach() {
        let perks = meld_client::net::PerksLine {
            explorer_map: 3,
            explorer_map_radius: 30.0,
            ..Default::default()
        };
        let mut app = app_with(perks);
        // A portal far out of reach is not on the map just because the instance holds it.
        app.world_mut()
            .resource_mut::<Overworld>()
            .entities
            .insert("portal".into(), at(EntityKind::Portal, 400.0, 0.0));
        walk_to(&mut app, 0.0, 0.0);
        assert!(app.world().resource::<ExploredMap>().seen.is_empty(), "no clairvoyance");

        // Walking a line lays down one cell per stride, and every step is kept.
        for step in 1..=6 {
            walk_to(&mut app, step as f32 * MAP_CELL, 0.0);
        }
        let cells = app.world().resource::<ExploredMap>().visited.len();
        assert_eq!(cells, 7, "one cell per stride walked, including where we started");

        // Come within the map's reach and the portal is learned — and stays learned
        // after walking away, which is the whole point of a map.
        walk_to(&mut app, 380.0, 0.0);
        assert_eq!(
            app.world().resource::<ExploredMap>().seen.values().copied().collect::<Vec<_>>(),
            vec![Landmark::Portal]
        );
        walk_to(&mut app, 0.0, 0.0);
        assert_eq!(app.world().resource::<ExploredMap>().seen.len(), 1, "a map remembers");

        // A new dive is a new world: the map is blanked, not merged.
        app.world_mut().resource_mut::<ExploredMap>().forget();
        let map = app.world().resource::<ExploredMap>();
        assert!(map.visited.is_empty() && map.seen.is_empty() && !map.walked);
    }

    // Tiers gate what may be plotted, exactly as the corner minimap's do: a chest at
    // tier 1 is not on the map, and a node needs tier 3.
    #[test]
    fn the_map_plots_only_what_the_party_s_perks_allow() {
        for (tier, want) in [(1u8, 0), (2, 1), (3, 2)] {
            let perks = meld_client::net::PerksLine {
                explorer_map: tier,
                explorer_map_radius: 30.0,
                ..Default::default()
            };
            let mut app = app_with(perks);
            {
                let mut world = app.world_mut().resource_mut::<Overworld>();
                world.entities.insert("chest".into(), at(EntityKind::Chest, 4.0, 0.0));
                world.entities.insert("node".into(), at(EntityKind::Resource, 8.0, 0.0));
            }
            walk_to(&mut app, 0.0, 0.0);
            assert_eq!(
                app.world().resource::<ExploredMap>().seen.len(),
                want,
                "map tier {tier} plotted the wrong set"
            );
        }
    }

    // A cell keeps its most notable landmark: two things in one cell must not make the
    // portal you are marching towards disappear behind a bush.
    #[test]
    fn a_crowded_cell_keeps_the_landmark_that_matters() {
        let perks = meld_client::net::PerksLine {
            explorer_map: 3,
            explorer_map_radius: 30.0,
            ..Default::default()
        };
        let mut app = app_with(perks);
        {
            let mut world = app.world_mut().resource_mut::<Overworld>();
            world.entities.insert("node".into(), at(EntityKind::Resource, 1.0, 1.0));
            world.entities.insert("portal".into(), at(EntityKind::Portal, 2.0, 2.0));
        }
        walk_to(&mut app, 0.0, 0.0);
        let map = app.world().resource::<ExploredMap>();
        assert_eq!(map.seen.get(&map_cell(1.0, 1.0)), Some(&Landmark::Portal));
    }

    // One scale for both axes: a straight march has to read as a straight line, and
    // whatever the walk's shape, everything drawn lands inside the panel.
    #[test]
    fn the_projection_keeps_its_aspect_and_stays_in_the_panel() {
        let mut map = ExploredMap { walked: true, here: (0.0, 0.0), ..Default::default() };
        for step in 0..20 {
            map.visited.insert((step, 0));
        }
        let bounds = map_bounds(&map);
        let (w, h) = (400.0, 200.0);

        let a = map_to_px(0.0, 0.0, bounds, w, h);
        let b = map_to_px(20.0 * MAP_CELL, 0.0, bounds, w, h);
        assert!((a.1 - b.1).abs() < 0.001, "a walk along y=0 must not tilt: {a:?} {b:?}");
        assert!(b.0 > a.0, "and it must run left to right");
        for (x, y) in [(0.0, 0.0), (20.0 * MAP_CELL, 0.0), (10.0 * MAP_CELL, 0.0)] {
            let (px, py) = map_to_px(x, y, bounds, w, h);
            assert!((0.0..=w).contains(&px) && (0.0..=h).contains(&py), "{px},{py} escaped");
        }

        // A single cell walked is the degenerate case — it must not divide by a zero
        // span and fling the dot off the panel.
        let mut one = ExploredMap { walked: true, ..Default::default() };
        one.visited.insert((3, 3));
        one.here = (18.0, 18.0);
        let b1 = map_bounds(&one);
        let (px, py) = map_to_px(18.0, 18.0, b1, w, h);
        assert!(px.is_finite() && py.is_finite());
        assert!((0.0..=w).contains(&px) && (0.0..=h).contains(&py), "{px},{py}");
    }
}

// ---------------------------------------------------------- field stations --

/// The field bench, while it is open. A station is a PLACE, so this holds the station
/// being worked at rather than a screen: walk away and the world closes it.
#[derive(Resource, Default)]
pub(crate) struct StationUi {
    pub open: Option<String>,
    /// Which bench it is: `smith` or `alembic`. They offer different work.
    pub kind: String,
    pub jobs: u8,
}

/// The field bench's one line: which piece is on it, and the two keys. Deliberately
/// the same shape as the city anvil's bench, because it is the same errand — the only
/// difference is whose skill is doing it.
pub(crate) fn station_line(
    station: &StationUi,
    craft: &CraftData,
    inv: &InventoryData,
) -> Option<String> {
    station.open.as_ref()?;
    // A Keeper's still is a pot with a recipe on it, not a bench with a piece: the
    // recipe book the city already fetched is the list, and up/down walks it.
    if station.kind == "alembic" {
        let head = format!("FIELD STILL  ({} brew(s) left)", station.jobs);
        let Some(r) = craft.recipes.get(craft.cursor.min(craft.recipes.len().saturating_sub(1)))
        else {
            return Some(format!("{head}   no recipes known   [E] leave"));
        };
        let inputs: Vec<String> = r
            .inputs
            .iter()
            .map(|(kind, need)| {
                let have = inv.materials.iter().find(|(k, _)| k == kind).map_or(0, |(_, q)| *q);
                format!("{have}/{need} {kind}")
            })
            .collect();
        let gate = if r.craftable {
            String::new()
        } else {
            format!("  (needs {} {})", r.skill, r.required_level)
        };
        return Some(format!(
            "{head}   up/down {} x{}  <- {}{gate}   [B] brew   [X] pack up   [E] leave",
            r.name,
            r.output_quantity,
            inputs.join(" + ")
        ));
    }
    let head = format!("FIELD FORGE  ({} job(s) left)", station.jobs);
    let Some(g) = crate::city::bench_gear(craft, inv) else {
        return Some(format!("{head}   nothing in your Vault to work on   [E] leave"));
    };
    let ins = meld_proto::enums::Insurance::from_wire(&g.insurance);
    let mut keys = Vec::new();
    if ins != Some(meld_proto::enums::Insurance::Ephemeral) {
        keys.push(format!("[R] reroll ({} stock)", g.reroll_cost));
    }
    if ins == Some(meld_proto::enums::Insurance::Insured) {
        keys.push("[P] repair".to_string());
    }
    if keys.is_empty() {
        keys.push("nothing a smith can do with this".to_string());
    }
    Some(format!(
        "{head}   <-/-> {} T{} {}   {}   [X] pack up   [E] leave",
        g.name,
        g.tier,
        ins.map(|i| i.label()).unwrap_or("?"),
        keys.join("   ")
    ))
}

/// Keys for the field bench. Only live while a station is open, and the station closes
/// the moment you step out of its reach — a bench you are not standing at is not yours
/// to use, and the server would refuse anyway.
pub(crate) fn station_input(
    keys: Res<ButtonInput<KeyCode>>,
    net: NonSend<NetRes>,
    world: Res<Overworld>,
    session: Res<Session>,
    inv: Res<InventoryData>,
    mut craft: ResMut<CraftData>,
    mut station: ResMut<StationUi>,
) {
    let Some(id) = station.open.clone() else { return };
    // Out of reach (or the station is spent and gone from the snapshot) → closed.
    let still_here = match (world.entities.get(&session.player_id), world.entities.get(&id)) {
        (Some(me), Some(st)) => {
            ((st.x - me.x).powi(2) + (st.y - me.y).powi(2)).sqrt() <= INTERACT_REACH * 2.0
        }
        _ => false,
    };
    if !still_here || keys.just_pressed(KeyCode::Escape) {
        station.open = None;
        return;
    }
    let n = inv.gear.len();
    if n > 0 && keys.just_pressed(KeyCode::ArrowRight) {
        craft.bench = (craft.bench + 1) % n;
        return;
    }
    if n > 0 && keys.just_pressed(KeyCode::ArrowLeft) {
        craft.bench = (craft.bench + n - 1) % n;
        return;
    }
    // [X] packs the bench up: its own channel, and only its owner may do it — the server
    // says so in its own words rather than the client guessing at ownership.
    if keys.just_pressed(KeyCode::KeyX) {
        net.0.send(ClientCmd::TeardownStation { entity_id: id });
        station.open = None;
        return;
    }
    // A still brews: up/down walk the recipe book (fetched over HTTP like the city's),
    // [B] puts the pot on. Its own keys, because a forge's make no sense at one.
    if station.kind == "alembic" {
        let n = craft.recipes.len();
        if n > 0 && keys.just_pressed(KeyCode::ArrowDown) {
            craft.cursor = (craft.cursor + 1) % n;
            return;
        }
        if n > 0 && keys.just_pressed(KeyCode::ArrowUp) {
            craft.cursor = (craft.cursor + n - 1) % n;
            return;
        }
        if keys.just_pressed(KeyCode::KeyB) {
            if let Some(r) = craft.recipes.get(craft.cursor) {
                net.0.send(ClientCmd::SmithRequest {
                    entity_id: id,
                    gear_id: String::new(),
                    service: "brew".into(),
                    material: String::new(),
                    recipe: r.recipe.clone(),
                });
            }
        }
        return;
    }
    let repair = keys.just_pressed(KeyCode::KeyP);
    let reroll = keys.just_pressed(KeyCode::KeyR);
    if !(repair || reroll) {
        return;
    }
    let Some(g) = crate::city::bench_gear(&craft, &inv) else { return };
    let material = if reroll {
        // Same rule as the city anvil: spend the deepest refined stock rather than
        // making anyone name a material at a bench in a maze.
        crate::city::best_stock(&inv, meld_proto::materials::MaterialClass::Refined).unwrap_or_default()
    } else {
        String::new()
    };
    let service = if repair { "repair" } else { "reroll" };
    net.0.send(ClientCmd::SmithRequest {
        entity_id: id,
        gear_id: g.gear_id.clone(),
        service: service.into(),
        material,
        recipe: String::new(),
    });
}

#[cfg(test)]
mod station_tests {
    use super::*;

    fn piece(insurance: &str, tier: i32) -> meld_client::net::GearLine {
        meld_client::net::GearLine {
            gear_id: "g1".into(),
            name: "Wearing Blade".into(),
            slot: "main_hand".into(),
            class_key: String::new(),
            insurance: insurance.into(),
            family: String::new(),
            armor_weight: String::new(),
            affixes: Vec::new(),
            unique_key: String::new(),
            set_key: String::new(),
            tier,
            equipped_hero_slot: None,
            max_durability: 8,
            base_max_durability: 12,
            atk_bonus: 4,
            def_bonus: 0,
            spd_bonus: 0,
            reroll_cost: 3 + 2 * tier,
        }
    }

    // A closed bench says nothing at all — the HUD is silent unless something is
    // actually in front of you.
    #[test]
    fn a_closed_bench_prints_nothing() {
        let station = StationUi::default();
        assert!(station_line(&station, &CraftData::default(), &InventoryData::default()).is_none());
    }

    // Open, the field bench reads like the city anvil's: the piece, its tier, and only
    // the services that tier can take — the same rules, wherever the anvil is.
    #[test]
    fn the_field_bench_offers_what_the_city_bench_would() {
        let station = StationUi {
            open: Some("station-smith-0".into()),
            kind: "smith".into(),
            jobs: 3,
        };
        let craft = CraftData::default();
        let mut inv = InventoryData::default();

        // An empty Vault is an answer, not a blank panel.
        let empty = station_line(&station, &craft, &inv).expect("open");
        assert!(empty.contains("3 job(s) left"), "{empty}");
        assert!(empty.contains("nothing in your Vault"), "{empty}");
        assert!(empty.contains("[E] leave"), "{empty}");

        inv.gear = vec![piece("insured", 2)];
        let insured = station_line(&station, &craft, &inv).expect("open");
        assert!(insured.contains("[R] reroll (7 stock)"), "{insured}");
        assert!(insured.contains("[P] repair"), "{insured}");

        inv.gear = vec![piece("standard", 1)];
        let standard = station_line(&station, &craft, &inv).expect("open");
        assert!(standard.contains("[R] reroll (5 stock)"), "{standard}");
        assert!(!standard.contains("[P] repair"), "{standard}");

        inv.gear = vec![piece("ephemeral", 3)];
        let ephemeral = station_line(&station, &craft, &inv).expect("open");
        assert!(!ephemeral.contains("[R] reroll"), "{ephemeral}");
        assert!(ephemeral.contains("nothing a smith can do"), "{ephemeral}");
    }

    // [E] on a station opens the bench rather than firing a one-shot action: the two
    // services are keys AT the bench, so the world's one interact key has to hand off.
    #[test]
    fn e_on_a_station_offers_the_forge_and_counts_its_jobs() {
        let mut world = Overworld::default();
        let session = Session { player_id: "me".into(), ..Default::default() };
        let mut me = OwEntity {
            x: 0.0,
            y: 0.0,
            kind: EntityKind::Player,
            name: None,
            faction: None,
            radius: 0.0,
            battling: false,
            level: 0,
            opened: false,
            mob_level: None,
            hp: None,
            max_hp: None,
            encounter_class: None,
            aggression: None,
            bodies_required: 1,
        };
        world.entities.insert("me".into(), me.clone());
        me.kind = EntityKind::Station;
        me.name = Some("smith".into());
        me.bodies_required = 2;
        me.x = 1.0;
        world.entities.insert("station-smith-0".into(), me);

        let target = interact_target(&world, &session).expect("a forge in reach");
        assert!(
            matches!(&target, Interact::UseStation { entity_id, kind, jobs }
                if entity_id == "station-smith-0" && kind == "smith" && *jobs == 2)
        );
        // The prompt says how many jobs are left, so nobody walks over for nothing.
        assert_eq!(target.prompt(), "[E] Use the forge (2 left)");
    }
}

// ------------------------------------------------------- the smithing heat --

/// An open heat (MS-1's smithing tempo game). The bar is **red** and the marker sweeps
/// it; each blow has one **yellow** band, and hitting it is what quality is. The server
/// laid the bands out and grades every blow — this is only where the bar is drawn and
/// when the player pressed.
#[derive(Resource, Default)]
pub(crate) struct HeatUi {
    pub job_id: Option<String>,
    pub service: String,
    pub strikes: i32,
    pub sweep_ms: i64,
    pub bands: Vec<(f64, f64)>,
    /// Blows landed so far, so the bar knows which band is live.
    pub struck: i32,
    /// Client seconds when the heat opened, for the marker's position.
    pub opened_at: f64,
}

impl HeatUi {
    /// Where the marker is right now, as a fraction of the bar. The marker sweeps left
    /// to right and wraps, one pass per `sweep_ms`.
    pub(crate) fn marker(&self, now: f64) -> f64 {
        let sweep = (self.sweep_ms.max(1) as f64) / 1000.0;
        (((now - self.opened_at).max(0.0) / sweep) % 1.0).clamp(0.0, 1.0)
    }

    pub(crate) fn band(&self) -> Option<(f64, f64)> {
        self.bands.get(self.struck.max(0) as usize).copied()
    }
}

/// The heat's label: which blow this is, and whether the marker is on the yellow right
/// now. The bar itself is real coloured nodes ([`spawn_heat_bar`]) — this is the line
/// under it, in the same panel every other prompt uses.
pub(crate) fn heat_line(heat: &HeatUi, now: f64) -> Option<String> {
    heat.job_id.as_ref()?;
    let (lo, hi) = heat.band()?;
    let m = heat.marker(now);
    Some(format!(
        "  {} {}  blow {}/{}   [SPACE] strike",
        heat.service.to_uppercase(),
        if (lo..=hi).contains(&m) { "NOW" } else { "   " },
        heat.struck + 1,
        heat.strikes
    ))
}

/// [SPACE] strikes the open heat. Nothing else here decides anything: the position is
/// reported, the server owns the bands and the grade.
pub(crate) fn heat_input(
    keys: Res<ButtonInput<KeyCode>>,
    net: NonSend<NetRes>,
    time: Res<Time>,
    mut heat: ResMut<HeatUi>,
) {
    let Some(job_id) = heat.job_id.clone() else { return };
    if !keys.just_pressed(KeyCode::Space) {
        return;
    }
    let at = heat.marker(time.elapsed_secs_f64());
    net.0.send(ClientCmd::Strike { job_id, at });
    heat.struck += 1;
    // The last blow closes the bar: the answer arrives as a smith result.
    if heat.struck >= heat.strikes {
        heat.job_id = None;
    }
}

#[cfg(test)]
mod heat_tests {
    use super::*;

    // The Keeper's still is a pot with a RECIPE on it, not a bench with a piece: it
    // reads out what the brew wants, what you carry, and the key that puts it on.
    #[test]
    fn a_field_still_reads_out_the_brew_rather_than_a_piece() {
        let station = StationUi {
            open: Some("station-alembic-0".into()),
            kind: "alembic".into(),
            jobs: 2,
        };
        let mut craft = CraftData { loaded: true, ..Default::default() };
        let mut inv = InventoryData::default();

        // No book yet is an answer, not a blank pot.
        let bare = station_line(&station, &craft, &inv).expect("open");
        assert!(bare.contains("FIELD STILL"), "{bare}");
        assert!(bare.contains("2 brew(s) left"), "{bare}");
        assert!(bare.contains("no recipes known"), "{bare}");

        craft.recipes = vec![meld_client::net::RecipeLine {
            recipe: "bloom_salve".into(),
            name: "Bloom Salve".into(),
            skill: "alchemy".into(),
            required_level: 1,
            skill_level: 1,
            craftable: true,
            output_quantity: 1,
            inputs: vec![("bloom_herb".to_string(), 2)],
        }];
        inv.materials = vec![("bloom_herb".to_string(), 1)];
        let line = station_line(&station, &craft, &inv).expect("open");
        // have/need per input is the whole answer to "why can't I brew this".
        assert!(line.contains("1/2 bloom_herb"), "{line}");
        assert!(line.contains("[B] brew"), "{line}");
        // A forge's keys have no meaning at a pot.
        assert!(!line.contains("[R] reroll"), "{line}");
        assert!(!line.contains("[P] repair"), "{line}");
    }

    fn open(bands: &[(f64, f64)]) -> HeatUi {
        HeatUi {
            job_id: Some("heat-1".into()),
            service: "reroll".into(),
            strikes: bands.len() as i32,
            sweep_ms: 1000,
            bands: bands.to_vec(),
            struck: 0,
            opened_at: 0.0,
        }
    }

    // No heat, no bar: the panel is silent unless there is metal on the anvil.
    #[test]
    fn a_closed_heat_draws_nothing() {
        assert!(heat_line(&HeatUi::default(), 0.0).is_none());
    }

    // The marker sweeps the bar once per `sweep_ms` and wraps — a rhythm, not a
    // one-way timer, so a missed blow comes round again.
    // The bar is REAL coloured nodes, not shaded text: hidden with no heat, and with one
    // open the yellow sits exactly where the server put the band while the marker sweeps
    // across it. A percentage-positioned band is what makes "strike the yellow" honest —
    // what the player aims at is what the server graded.
    #[test]
    fn the_bar_puts_the_yellow_where_the_server_said() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.init_resource::<HeatUi>();
        app.add_systems(Update, update_heat_bar);
        let bar = app
            .world_mut()
            .spawn((HeatBar, Node { display: Display::Flex, ..default() }))
            .id();
        let band = app
            .world_mut()
            .spawn((HeatBarBand, Node::default()))
            .id();
        let mark = app.world_mut().spawn((HeatBarMark, Node::default())).id();

        // No heat → the bar is not on screen at all.
        app.update();
        assert_eq!(app.world().get::<Node>(bar).unwrap().display, Display::None);

        *app.world_mut().resource_mut::<HeatUi>() = open(&[(0.25, 0.45)]);
        app.update();
        assert_eq!(app.world().get::<Node>(bar).unwrap().display, Display::Flex);
        let b = app.world().get::<Node>(band).unwrap();
        assert_eq!(b.left, Val::Percent(25.0));
        assert_eq!(b.width, Val::Percent(20.0), "the band is the server's, to the point");

        // The marker moves with the clock, and stays on the bar.
        let first = app.world().get::<Node>(mark).unwrap().left;
        std::thread::sleep(std::time::Duration::from_millis(120));
        app.update();
        let later = app.world().get::<Node>(mark).unwrap().left;
        assert_ne!(first, later, "the marker should sweep");
        if let Val::Percent(p) = later {
            assert!((0.0..=100.0).contains(&p), "{p}");
        } else {
            panic!("the marker should be positioned as a percentage: {later:?}");
        }
    }

    #[test]
    fn the_marker_sweeps_and_wraps() {
        let heat = open(&[(0.4, 0.6)]);
        assert!(heat.marker(0.0) < 0.01);
        assert!((heat.marker(0.5) - 0.5).abs() < 0.01);
        assert!(heat.marker(1.0) < 0.01, "one second in, it has wrapped");
        assert!((heat.marker(1.25) - 0.25).abs() < 0.01);
    }

    // The bar reads as red with the live blow's yellow on it, the hammer wherever the
    // marker is, and it says which blow this is out of how many.
    #[test]
    fn the_bar_shows_red_yellow_and_the_hammer() {
        let mut heat = open(&[(0.0, 0.1), (0.45, 0.55)]);
        let line = heat_line(&heat, 0.0).expect("open");
        assert!(line.contains("blow 1/2"), "{line}");
        // The marker is inside the first band at t=0, so the line calls it.
        assert!(line.contains("NOW"), "{line}");

        // The next blow has its OWN band, so a smith cannot learn one spot.
        heat.struck = 1;
        let second = heat_line(&heat, 0.0).expect("open");
        assert!(second.contains("blow 2/2"), "{second}");
        assert!(!second.contains("NOW"), "the second band is not where the first was");
        assert_eq!(heat.band(), Some((0.45, 0.55)));

        // Past the last blow there is nothing to draw.
        heat.struck = 2;
        assert!(heat_line(&heat, 0.0).is_none());
    }
}

/// One frame's worth of the over-the-head action panel (rebuilt each frame).
#[derive(Component)]
pub(crate) struct ActionHud;

/// Ask the bench in reach for its temporary boon — the ONE dispatch, shared by [N] and the
/// plate's chip.
pub(crate) fn ask_for_boon(
    net: &crate::net::Net,
    world: &Overworld,
    session: &Session,
    inv: &InventoryData,
) {
    let Some((entity_id, kind, _)) = boon_offer(world, session) else { return };
    let (service, class) = if kind == "alembic" {
        ("tonic", meld_proto::materials::MaterialClass::Reagent)
    } else {
        ("enhance", meld_proto::materials::MaterialClass::Refined)
    };
    net.send(ClientCmd::SmithRequest {
        entity_id,
        gear_id: worn_piece(inv, 0).unwrap_or_default(),
        service: service.into(),
        material: crate::city::best_stock(inv, class).unwrap_or_default(),
        recipe: String::new(),
    });
}

/// Do whatever is in reach — the ONE dispatch, shared by [E], the over-head plate and the
/// old touch button. It used to be inline in the touch handler, which meant a second copy
/// every time another surface wanted the same action.
pub(crate) fn do_interact(
    net: &crate::net::Net,
    world: &Overworld,
    session: &Session,
    station: &mut StationUi,
) {
    if session.channeling {
        net.send(ClientCmd::CancelHarvest);
        return;
    }
    match interact_target(world, session) {
        Some(Interact::JoinFight) => net.send(ClientCmd::JoinBattle),
        Some(Interact::Harvest { entity_id, .. }) => net.send(ClientCmd::Harvest { entity_id }),
        Some(Interact::OpenChest { entity_id }) => net.send(ClientCmd::OpenChest { entity_id }),
        Some(Interact::EnterDungeon { entity_id }) => {
            net.send(ClientCmd::EnterDungeon { entity_id })
        }
        Some(Interact::UseStation { entity_id, kind, jobs }) => {
            if kind == "alembic" {
                net.fetch_recipes();
            }
            station.open = Some(entity_id);
            station.kind = kind;
            station.jobs = jobs;
        }
        Some(Interact::Extract) => net.send(ClientCmd::Extract),
        None => {}
    }
}

/// The [E] chip on the plate — the touch twin of the key, in the place you are looking.
#[derive(Component)]
pub(crate) struct ActionHudTap;

/// The [N] chip: ask the bench for its boon.
#[derive(Component)]
pub(crate) struct ActionHudBoonTap;

/// Tapping the [N] chip asks for the bench's temporary boon — the same thing the key does.
pub(crate) fn action_hud_boon_tap(
    q: Query<&Interaction, (Changed<Interaction>, With<ActionHudBoonTap>)>,
    net: NonSend<NetRes>,
    world: Res<Overworld>,
    session: Res<Session>,
    inv: Res<InventoryData>,
) {
    for interaction in &q {
        if *interaction == Interaction::Pressed {
            ask_for_boon(&net.0, &world, &session, &inv);
        }
    }
}

/// Tapping the plate over your head does whatever [E] would, including stopping a channel.
pub(crate) fn action_hud_tap(
    q: Query<&Interaction, (Changed<Interaction>, With<ActionHudTap>)>,
    net: NonSend<NetRes>,
    world: Res<Overworld>,
    session: Res<Session>,
    mut station: ResMut<StationUi>,
) {
    for interaction in &q {
        if *interaction == Interaction::Pressed {
            do_interact(&net.0, &world, &session, &mut station);
        }
    }
}

/// The prompt, the channel bar and the "+1 <material>" pops, in frosted glass over the
/// player's own head.
///
/// They used to live in the top-left HUD line, which is the wrong place for all three: what
/// you are looking at while you gather is your character, and a bar in the corner is a bar
/// you do not watch. Paying per tick only reads as paying if you can see the payout land.
pub(crate) fn update_action_hud(
    mut commands: Commands,
    world: Res<Overworld>,
    session: Res<Session>,
    time: Res<Time>,
    mut pops: ResMut<HarvestPops>,
    // The same phase clock the old corner bar kept: seconds since this channel began,
    // wrapped per payout by `channel_fill_pct`.
    mut phase: Local<f32>,
    mut was_channeling: Local<bool>,
    cam_q: Query<(&Camera, &GlobalTransform), With<Camera3d>>,
    root_q: Query<Entity, With<NameplateRoot>>,
    players: Query<(&WorldEntity, &GlobalTransform)>,
    old: Query<Entity, With<ActionHud>>,
) {
    for e in &old {
        commands.entity(e).despawn();
    }
    // Age the floaters and drop the ones that have had their moment.
    let dt = time.delta_secs();
    for p in pops.items.iter_mut() {
        p.age += dt;
    }
    pops.items.retain(|p| p.age < HARVEST_POP_TTL);

    let running = session.channeling && session.channel_fill_ms > 0;
    if running && !*was_channeling {
        *phase = 0.0;
    }
    *was_channeling = running;
    if running {
        *phase += dt;
    }


    let target = interact_target(&world, &session);
    let boon = boon_offer(&world, &session);
    if target.is_none() && boon.is_none() && !session.channeling && pops.items.is_empty() {
        return; // nothing to say, so nothing on screen (the [E]-only rule)
    }
    let Some((cam, cam_tf)) = cam_q.iter().next() else { return };
    let Ok(root) = root_q.single() else { return };
    let Some(me) = players.iter().find(|(we, _)| we.0 == session.player_id).map(|(_, tf)| tf) else {
        return;
    };
    let head = me.translation() + Vec3::Y * 2.35;
    let Ok(at) = cam.world_to_viewport(cam_tf, head) else { return };

    const W: f32 = 230.0;
    commands.entity(root).with_children(|p| {
        p.spawn((
            ActionHud,
            Node {
                position_type: PositionType::Absolute,
                left: Val::Px(at.x - W / 2.0),
                // Sit above the head, and leave room for however many pops are in the air.
                top: Val::Px(at.y - 34.0 - 18.0 * pops.items.len() as f32),
                width: Val::Px(W),
                flex_direction: FlexDirection::Column,
                align_items: AlignItems::Center,
                row_gap: Val::Px(3.0),
                ..default()
            },
        ))
        .with_children(|col| {
            // The payouts, newest nearest the head, fading as they rise.
            for pop in pops.items.iter().rev() {
                let a = (1.0 - pop.age / HARVEST_POP_TTL).clamp(0.0, 1.0);
                col.spawn((
                    Text::new(pop.label.clone()),
                    TextFont { font_size: 17.0, ..default() },
                    TextColor(Color::srgba(0.62, 0.98, 0.7, a)),
                ));
            }
            let line = if session.channeling {
                Some("[E] stop".to_string())
            } else {
                target.as_ref().map(|t| t.prompt())
            };
            let boon_line = boon.as_ref().map(|(_, _, what)| format!("[N] {what}"));
            if line.is_none() && boon_line.is_none() && !session.channeling {
                return;
            }
            // One frosted plate holding the prompt and the bar, mostly see-through so it
            // never hides the character it belongs to.
            col.spawn((
                Node {
                    flex_direction: FlexDirection::Column,
                    align_items: AlignItems::Center,
                    row_gap: Val::Px(4.0),
                    padding: UiRect::axes(Val::Px(10.0), Val::Px(5.0)),
                    border: UiRect::all(Val::Px(1.0)),
                    ..default()
                },
                BackgroundColor(glass::GLASS_THIN),
                BorderColor(glass::EDGE_SOFT),
                BorderRadius::all(Val::Px(7.0)),
            ))
            .with_children(|plate| {
                // Each prompt is its own chip, so touch has a target per action instead of
                // one button in the corner that had to guess which you meant.
                if let Some(text) = line {
                    plate
                        .spawn((Button, ActionHudTap, glass::chip(false)))
                        .with_children(|b| {
                            b.spawn(glass::text(text, 15.0, glass::TEXT));
                        });
                }
                if let Some(text) = boon_line {
                    plate
                        .spawn((Button, ActionHudBoonTap, glass::chip(false)))
                        .with_children(|b| {
                            b.spawn(glass::text(text, 15.0, glass::WARN));
                        });
                }
                if session.channeling {
                    let pct = channel_fill_pct(*phase, session.channel_fill_ms);
                    plate
                        .spawn((
                            Node {
                                width: Val::Px(150.0),
                                height: Val::Px(7.0),
                                border: UiRect::all(Val::Px(1.0)),
                                ..default()
                            },
                            BorderColor(glass::EDGE_SOFT),
                            BackgroundColor(Color::srgba(0.02, 0.03, 0.06, 0.7)),
                        ))
                        .with_children(|bar| {
                            bar.spawn((
                                Node {
                                    width: Val::Percent(pct),
                                    height: Val::Percent(100.0),
                                    ..default()
                                },
                                BackgroundColor(glass::TITLE),
                            ));
                        });
                }
            });
        });
    });
}

/// The halo quad behind whatever is currently in reach.
#[derive(Component)]
pub(crate) struct ReachHalo;

/// Make the EDGE of the thing you could interact with glow, on a slow infrequent pulse.
///
/// Two jobs in one affordance. It says "this one is in reach" — nothing used to distinguish
/// the node you can actually gather from one three steps behind it — and it draws the eye
/// without erasing the art, which is where the old whole-sprite emissive pulse went wrong:
/// emissive is added flat across a textured quad, so it painted the sprite out.
///
/// The glow is a copy of the thing's OWN sprite, a little larger and drawn behind it, tinted
/// warm. A silhouette a few pixels wider than the sprite reads as a rim, which is the cheap
/// 2D outline trick and needs no shader. It breathes slowly and spends most of its cycle
/// nearly out, so it is noticeable when you look and quiet when you do not.
pub(crate) fn update_reach_halo(
    mut commands: Commands,
    world: Res<Overworld>,
    session: Res<Session>,
    time: Res<Time>,
    mut mats: ResMut<Assets<StandardMaterial>>,
    wa: Option<Res<WorldAssets>>,
    targets: Query<(Entity, &WorldEntity, &Children)>,
    sprite_of: Query<&MeshMaterial3d<StandardMaterial>, Without<ReachHalo>>,
    halos: Query<(Entity, &MeshMaterial3d<StandardMaterial>), With<ReachHalo>>,
) {
    let want = interact_target(&world, &session)
        .as_ref()
        .and_then(|t| t.entity_id().map(String::from));

    // A slow breathe that sits near zero most of the cycle: ~4s period, and the visible part
    // is the top of the curve. Obvious when you look for it, easy to ignore when you are not.
    let phase = (time.elapsed_secs() * std::f32::consts::TAU / 4.0).sin().max(0.0);
    let alpha = 0.12 + 0.55 * phase * phase;

    let Some(id) = want else {
        // Nothing in reach: clear any halo still standing.
        for (e, _) in &halos {
            commands.entity(e).despawn();
        }
        return;
    };
    let Some(wa) = wa else { return };

    // Already lit? Just breathe it.
    let mut found = false;
    for (e, mm) in &halos {
        let still_right = targets
            .iter()
            .any(|(_, we, kids)| we.0 == id && kids.iter().any(|k| k == e));
        if still_right {
            found = true;
            if let Some(m) = mats.get_mut(&mm.0) {
                m.base_color = m.base_color.with_alpha(alpha);
            }
        } else {
            commands.entity(e).despawn();
        }
    }
    if found {
        return;
    }
    // Otherwise build one from the target's own sprite texture.
    let Some((root, _, kids)) = targets.iter().find(|(_, we, _)| we.0 == id) else { return };
    let tex = kids
        .iter()
        .filter_map(|k| sprite_of.get(k).ok())
        .filter_map(|mm| mats.get(&mm.0).and_then(|m| m.base_color_texture.clone()))
        .next();
    let Some(tex) = tex else { return };
    let halo = mats.add(StandardMaterial {
        base_color: Color::srgba(1.0, 0.86, 0.45, alpha),
        base_color_texture: Some(tex),
        // Unlit and alpha-blended: this is a light, not a surface. Depth write OFF so it
        // never punches a hole in the sprite it sits behind.
        unlit: true,
        alpha_mode: AlphaMode::Blend,
        depth_bias: -1.0,
        double_sided: true,
        cull_mode: None,
        ..default()
    });
    commands.entity(root).with_children(|p| {
        p.spawn((
            ReachHalo,
            Mesh3d(wa.sprite_quad.clone()),
            MeshMaterial3d(halo),
            // A touch larger and a hair behind, so what shows is a rim around the sprite.
            Transform::from_xyz(0.0, 0.85, -0.02).with_scale(Vec3::splat(1.7 / 2.2 * 1.13)),
            hd2d::Billboard,
        ));
    });
}
