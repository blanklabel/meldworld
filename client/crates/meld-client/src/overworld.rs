//! Overworld: movement + camera, snapshot→sprite reconciliation, terrain/walls,
//! chests, HUD/minimap, party-follower entourage, and the perk overlays (lamp,
//! nameplates). Extracted from `main.rs` during the module reorg.

use std::collections::HashSet;

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
        BorderColor::all(Color::srgba(1.0, 0.85, 0.7, 0.5)),
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


/// The red wash that says SOMETHING JUST HURT YOU.
///
/// Damage taken on the road — venom biting every few steps, the Force of a Shift — came off
/// the party's HP with nothing on screen: the number only appeared if you happened to open
/// the party panel, so bleeding out on a march was invisible right up to the fight you
/// arrived nearly dead at. This is the tell.
#[derive(Component)]
pub(crate) struct HurtFlash;

/// How much party HP we last saw, and how long the wash has left to run.
///
/// Driven off the ROSTER rather than a new message: `run.party` already carries every hero's
/// current HP, and the server re-sends it when a bite lands, so a drop in the total is the
/// signal. That also means anything else that quietly costs HP gets the same tell for free.
#[derive(Resource, Default)]
pub(crate) struct HurtWash {
    /// Total party HP at the last roster we saw. `None` until the first one arrives — a
    /// fresh roster must not read as damage.
    pub(crate) last_total: Option<i32>,
    /// Seconds of wash left.
    pub(crate) left: f32,
    /// How hard this wash goes, 0..1 — set from the share of the party's health the hit
    /// took, so a per-step venom nibble and a sprung trap do not look identical.
    pub(crate) peak: f32,
}

/// The wash is a QUICK one — long enough to catch the eye, short enough not to sit over the
/// world while you walk. A reminder, not a state.
const HURT_FLASH_SECS: f32 = 0.28;
/// How red it gets at its peak, for a blow that takes a QUARTER of the party. Well under
/// half, because it covers the whole screen and you are still steering through it.
///
/// Scaled by how much was actually lost (see [`update_hurt_flash`]), because the same wash
/// served a sprung trap and a venom nibble: venom bites once every few STEPS, so walking
/// poisoned washed the whole screen over and over at full strength. A reminder that fires
/// constantly at catastrophe volume stops reading as a warning and starts reading as a fault.
const HURT_FLASH_ALPHA: f32 = 0.34;
/// The share of the party's health a hit has to take to earn the full wash.
const HURT_FLASH_FULL_AT: f32 = 0.25;
/// The faintest a real hit is allowed to be — a one-HP nibble still has to register, or the
/// player learns nothing from the thing that is slowly killing them.
const HURT_FLASH_MIN: f32 = 0.06;

/// How hard a wash goes for losing `lost` of a `pool`-sized party: 0..1, full at
/// [`HURT_FLASH_FULL_AT`].
///
/// Pulled out as a function so the RULE can be tested — the bug it exists for was a venom
/// nibble and a sprung trap rendering identically, and "identically" is not something a
/// screenshot of one of them can catch.
pub(crate) fn wash_peak(lost: i32, pool: i32) -> f32 {
    if lost <= 0 || pool <= 0 {
        return 0.0;
    }
    ((lost as f32 / pool as f32) / HURT_FLASH_FULL_AT).clamp(0.0, 1.0)
}

/// Spawn the wash once, transparent.
///
/// Idempotent, because this runs on every `OnEnter(Overworld)` and the overworld is entered
/// again after every battle. A second panel would not be a second wash — the two alphas
/// multiply, so the flash would read darker each dive until it blacked the screen out.
pub(crate) fn spawn_hurt_flash(
    mut commands: Commands,
    mut wash: ResMut<HurtWash>,
    existing: Query<Entity, With<HurtFlash>>,
) {
    // RE-BASELINE on arrival, so the fight's own cost does not wash the screen on the way
    // out. The roster that lands when you return carries POST-fight HP, which is lower than
    // the pre-fight total this resource last saw — so the drop-detector fired after every
    // battle anybody was hurt in, replaying a whole fight's damage as one red flash over the
    // overworld. The damage was already shown hit by hit on the battle screen; this wash is
    // for being hurt OUT HERE. Dropping the baseline makes the next roster re-seed it
    // silently (a `None` cannot read as a drop), which is the same guard a first roster gets.
    wash.last_total = None;
    wash.left = 0.0;
    if !existing.is_empty() {
        return;
    }
    commands.spawn((
        HurtFlash,
        Node {
            position_type: PositionType::Absolute,
            left: Val::Percent(0.0),
            right: Val::Percent(0.0),
            top: Val::Percent(0.0),
            bottom: Val::Percent(0.0),
            ..default()
        },
        BackgroundColor(Color::srgba(0.7, 0.05, 0.06, 0.0)),
        // Under the blind mask, over the world.
        GlobalZIndex(38),
    ));
}

/// Watch the party's total HP; wash the screen red when it drops, then fade it out.
pub(crate) fn update_hurt_flash(
    time: Res<Time>,
    roster: Res<crate::PartyRoster>,
    mut wash: ResMut<HurtWash>,
    mut q: Query<&mut BackgroundColor, With<HurtFlash>>,
) {
    if !roster.heroes.is_empty() {
        let total: i32 = roster.heroes.iter().map(|h| h.hp.max(0)).sum();
        match wash.last_total {
            // Only a DROP. Healing, levelling and a hero joining all move this number, and
            // none of them should flash.
            Some(was) if total < was => {
                // How BADLY, as a share of what the party can hold. A venom bite is a
                // nibble and reads as one; a trap or a Shift's Force blast still fills the
                // screen. One mechanism, proportionate — rather than a second flash for
                // small damage, which is two rules for one fact.
                let pool: i32 = roster.heroes.iter().map(|h| h.max_hp.max(1)).sum();
                wash.peak = wash_peak(was - total, pool);
                wash.left = HURT_FLASH_SECS;
            }
            _ => {}
        }
        wash.last_total = Some(total);
    }
    if wash.left <= 0.0 {
        for mut c in &mut q {
            if c.0.alpha() != 0.0 {
                c.0.set_alpha(0.0);
            }
        }
        return;
    }
    wash.left = (wash.left - time.delta_secs()).max(0.0);
    // Fade out over the window, so the brightest instant is the moment it landed, and cap
    // the peak by how much the hit actually cost.
    let scale = wash.peak.max(HURT_FLASH_MIN / HURT_FLASH_ALPHA);
    let a = HURT_FLASH_ALPHA * scale * (wash.left / HURT_FLASH_SECS);
    for mut c in &mut q {
        c.0.set_alpha(a);
    }
}

/// The blackout a BLINDED party sees: a full-screen mask with a small clear circle around
/// the middle, so you can see your own feet and nothing else.
#[derive(Component)]
pub(crate) struct BlindMask;

/// Spawn the mask once, hidden. It is four opaque panels leaving a gap in the centre rather
/// than a texture, because the gap has to scale with the window and a bitmap would not.
///
/// Idempotent: this runs on every `OnEnter(Screen::Overworld)`, so a return from battle would
/// otherwise stack another four opaque panels on the first four, and `update_blind_mask` shows
/// ALL of them — a blinded party would get strictly darker with every fight it walked out of.
pub(crate) fn spawn_blind_mask(mut commands: Commands, existing: Query<Entity, With<BlindMask>>) {
    if !existing.is_empty() {
        return;
    }
    for (left, right, top, bottom) in [
        (0.0, 0.0, 0.0, 62.0),   // above the gap
        (0.0, 0.0, 62.0, 0.0),   // below it
        (0.0, 62.0, 38.0, 38.0), // left of it
        (62.0, 0.0, 38.0, 38.0), // right of it
    ] {
        commands.spawn((
            BlindMask,
            Node {
                position_type: PositionType::Absolute,
                left: Val::Percent(left),
                right: Val::Percent(right),
                top: Val::Percent(top),
                bottom: Val::Percent(bottom),
                ..default()
            },
            BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.96)),
            Visibility::Hidden,
            GlobalZIndex(40),
        ));
    }
}

/// Show it exactly while somebody in the party is blinded.
///
/// This is PRESENTATION only. The server already stops sending a blinded party the creatures
/// (see `snapshot_msgs`), because a client-side blackout is a suggestion and a hacked client
/// would simply ignore it — you still walk into what you cannot see, and the fight starts.
pub(crate) fn update_blind_mask(
    roster: Res<crate::PartyRoster>,
    mut mask: Query<&mut Visibility, With<BlindMask>>,
) {
    let blind = roster
        .heroes
        .iter()
        .any(|h| h.afflictions.iter().any(|a| a == "blinded"));
    for mut v in &mut mask {
        *v = if blind { Visibility::Visible } else { Visibility::Hidden };
    }
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

pub(crate) fn overworld_ui(
    mut commands: Commands,
    ground: Option<Res<crate::minimap::MinimapTiles>>,
) {
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
                    font_size: FontSize::Px(20.0),
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
                    border_radius: BorderRadius::all(Val::Percent(50.0)),
                    position_type: PositionType::Absolute,
                    width: Val::Px(120.0),
                    height: Val::Px(120.0),
                    border: UiRect::all(Val::Px(2.0)),
                    display: Display::None,
                    ..default()
                },
                BorderColor::all(Color::srgba(0.7, 0.8, 1.0, 0.5)),
                BackgroundColor(Color::srgba(0.3, 0.4, 0.7, 0.15)),
            ));
            p.spawn((
                JoystickKnob,
                Node {
                    border_radius: BorderRadius::all(Val::Percent(50.0)),
                    position_type: PositionType::Absolute,
                    width: Val::Px(56.0),
                    height: Val::Px(56.0),
                    display: Display::None,
                    ..default()
                },
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
            // The corner minimap, BACK — and now sharing the Map column's rendered ground
            // rather than being a panel of dots on glass. Both surfaces sample the same
            // texture; `minimap::track_map_view` frames it for whichever is being looked at
            // (player-centred out here, the walked rectangle when the Map column is open).
            //
            // It was removed when the ground arrived, on the argument that 140px cannot show
            // a 64px tile. That was true of the TILES and false of the panel: a glance at
            // where the monsters are is worth having without opening a menu, and the ground
            // under it reads as colour and coastline even when the texture detail does not.
            p.spawn((
                MinimapRoot,
                Node {
                    border_radius: BorderRadius::all(Val::Px(6.0)),
                    position_type: PositionType::Absolute,
                    right: Val::Px(14.0),
                    top: Val::Px(14.0),
                    width: Val::Px(140.0),
                    height: Val::Px(140.0),
                    border: UiRect::all(Val::Px(2.0)),
                    overflow: Overflow::clip(),
                    display: Display::None,
                    ..default()
                },
                BorderColor::all(Color::srgba(0.6, 0.8, 1.0, 0.5)),
                BackgroundColor(glass::GLASS_THIN),
            ))
            .with_children(|p| {
                if let Some(g) = &ground {
                    p.spawn((
                        ImageNode::new(g.image.clone()),
                        Node {
                            position_type: PositionType::Absolute,
                            left: Val::Px(0.0),
                            top: Val::Px(0.0),
                            width: Val::Percent(100.0),
                            height: Val::Percent(100.0),
                            ..default()
                        },
                    ));
                }
            });
            // How deep you are, under the map that earned it. Distance is the whole
            // difficulty axis, so it belongs beside the reading of the ground rather
            // than in a corner of its own — and it shows only when the Explorer's map
            // does, because without one you are meant to be guessing.
            p.spawn((
                MinimapDistance,
                Text::new(String::new()),
                TextFont { font_size: FontSize::Px(15.0), ..default() },
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
                border_radius: BorderRadius::all(Val::Px(8.0)),
                width: Val::Px(150.0),
                padding: UiRect::axes(Val::Px(14.0), Val::Px(11.0)),
                justify_content: JustifyContent::Center,
                border: UiRect::all(Val::Px(1.5)),
                ..default()
            },
            BorderColor::all(Color::srgb(0.4, 0.5, 0.8)),
            BackgroundColor(glass::GLASS),
        ))
        .with_children(|b| {
            b.spawn((
                Text::new(label.to_string()),
                TextFont { font_size: FontSize::Px(16.0), ..default() },
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
/// edge. This is a rendering concern only — the server still tracks and simulates every
/// creature regardless. The local player and the deep portal (a landmark beacon) are never
/// culled.
///
/// ⚠️ **THESE MUST REACH AT LEAST AS FAR AS THE FOG IS STILL CLEAR, AND FOR A LONG TIME
/// THEY DID NOT.** The note here used to read *"both radii sit BEYOND the fog wall
/// (`Look::fog_end` ~118), so culling is visually invisible"* — and that was TRUE when the
/// fog wall was 118. The fog was then pushed out to 700 and these never followed, so props,
/// trees and creatures stopped being drawn at **21%** of the distance the ground was visible
/// for. Reported from play: *"the forest and mire I just went through had almost no forest
/// or water"* — a wood reads as a field when four fifths of the visible radius is bare
/// terrain. Two numbers answering one question, drifted 4.7x apart, with the comment still
/// asserting the old relationship.
///
/// The rule is now `RENDER_UNLOAD_NEAR >= Look::fog_start` — populated at least as far as
/// the world is SHARP, held by `props_are_drawn_as_far_as_the_world_is_sharp`. Past
/// `fog_start` the fog is progressively hiding things, so culling there is a fair trade;
/// inside it, culling is a visible hole. Cost is bounded and small: at a forest's measured
/// route density (0.00321/u²) a 240-unit radius holds ~580 props against ~230 at 150 — a few
/// hundred extra billboards, not a different order of magnitude. **If the frame rate ever
/// drops, this pair is the first thing to pull back in** — and the fog must come in with it.
pub(crate) const RENDER_UNLOAD_NEAR: f32 = 200.0;
pub(crate) const RENDER_UNLOAD_FAR: f32 = 240.0;

/// The NEAR/FAR gap is the hysteresis that stops entities flickering at the boundary, so
/// FAR must exceed NEAR. Checked at COMPILE time rather than in a test: it is a relationship
/// between two constants, so a violation should refuse to build rather than wait for
/// `cargo test` — and clippy is right that asserting it at runtime is asserting a constant.
const _: () = assert!(RENDER_UNLOAD_FAR > RENDER_UNLOAD_NEAR);

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
            Option<&mut bevy::post_process::bloom::Bloom>,
            Option<&mut bevy::post_process::dof::DepthOfField>,
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

/// Roughly the server's `[ai] watch_radius` — wider than join range, because you can
/// SEE further than you can reach and watching commits nothing. The server does the
/// real check; this only decides whether to offer the prompt.
pub(crate) const WATCH_PROMPT_RADIUS: f32 = 16.0;

/// Is the player within join range of a teammate's ongoing fight?
pub(crate) fn near_fight(world: &Overworld, me: Option<(f32, f32)>) -> bool {
    let Some((mx, my)) = me else { return false };
    world
        .entities
        .values()
        .any(|e| e.battling && ((e.x - mx).powi(2) + (e.y - my).powi(2)).sqrt() <= JOIN_PROMPT_RADIUS)
}

/// What is in WATCHING range, as the word for it (`SOC-3`) — `None` when nothing is.
///
/// Two things can be watched and the nearest wins, exactly as the server resolves it:
/// another player's battle, or two creatures tearing at each other (`CR-2`). Reported as
/// a label rather than a bool because the two read completely differently to a player —
/// "watch the fight" is a decision about a teammate, "watch the clash" is a decision
/// about whether to wait and take what falls.
pub(crate) fn watchable(world: &Overworld, me: Option<(f32, f32)>) -> Option<&'static str> {
    let (mx, my) = me?;
    let near = |e: &OwEntity| ((e.x - mx).powi(2) + (e.y - my).powi(2)).sqrt();
    let fight = world
        .entities
        .values()
        .filter(|e| e.battling)
        .map(near)
        .filter(|d| *d <= WATCH_PROMPT_RADIUS)
        .min_by(f32::total_cmp);
    let clash = world
        .entities
        .values()
        .filter(|e| e.clashing)
        .map(near)
        .filter(|d| *d <= WATCH_PROMPT_RADIUS)
        .min_by(f32::total_cmp);
    match (fight, clash) {
        (Some(f), Some(c)) => Some(if f <= c { "Watch the fight" } else { "Watch the clash" }),
        (Some(_), None) => Some("Watch the fight"),
        (None, Some(_)) => Some("Watch the clash"),
        (None, None) => None,
    }
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
/// reads under the Explorer's depth line ([`update_minimap_distance`]) and, for everyone,
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
    mut stats: ResMut<RunStats>,
) {
    let Some(me) = world.entities.get(&session.player_id) else {
        return;
    };
    let d = (me.x * me.x + me.y * me.y).sqrt().floor() as i64;
    let tier = d / 100; // tier(d) = floor(d/100) — the CANON distance axis.
    // The label reads the CELL the player is standing in, through the same decomposition
    // the ground shader paints with — so it names the ground under your feet rather than a
    // whole ring's representative theme, which is now only a summary of many cells.
    let biome = title_case(crate::world_render::biome_at_world(me.x, me.y));
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

    // [V] WATCHES the fight in reach (`SOC-3`). Its own key rather than a rung on [E],
    // because [E] already JOINS and the two are opposite decisions: joining puts your
    // heroes in the queue and can kill them, watching costs nothing. Collapsing them onto
    // one key would mean a player who wanted to look walked into a fight instead.
    if keys.just_pressed(KeyCode::KeyV) {
        net.0.send(ClientCmd::WatchBattle);
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
        Interact::MendStructure { entity_id, .. } => {
            net.0.send(ClientCmd::RepairStructure { entity_id })
        }
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
    /// Spend a unit of ore mending a player-built structure. `hp_pct` is what it has
    /// left, so the prompt says whether it is worth the ore before you spend it.
    MendStructure { entity_id: String, name: String, hp_pct: u8 },
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
            Interact::MendStructure { name, hp_pct, .. } => {
                let what = meld_proto::structures::structure(name)
                    .map(|d| d.name)
                    .unwrap_or("structure");
                format!("Mend the {what} ({hp_pct}%)")
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
            | Interact::UseStation { entity_id, .. }
            | Interact::MendStructure { entity_id, .. } => Some(entity_id),
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
        "Ask for a tonic (party, this run)"
    } else {
        "Ask for an edge (this run)"
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
            // Mending is what holding ground actually costs, so it is on the one
            // interact key beside every other thing you do by standing near it.
            EntityKind::Structure => Some(Interact::MendStructure {
                entity_id: id.clone(),
                name: e.name.clone().unwrap_or_default(),
                hp_pct: e.bodies_required,
            }),
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
/// **THE CAMERA THE PLAYER IS ACTUALLY LOOKING THROUGH.**
///
/// ⚠️ THERE IS MORE THAN ONE CAMERA, AND THE OTHER ONE IS 512x288. The minimap renders the
/// corner map to a texture through its own `Camera2d` ([`crate::minimap`]), so a bare
/// `Query<(&Camera, &GlobalTransform)>` matches it too and `iter().next()` picks whichever
/// archetype order hands over first. Project a world point through THAT and every creature
/// in the world lands on the same pixel — measured, 42 creatures between 14 and 126 units
/// away all projecting to (256, 144), the exact centre of the minimap's viewport and the
/// top-left corner of the player's screen.
///
/// That is the whole of the recurring nameplate clump: *"a stack of quarry and creatures in
/// the top left"*, *"those creatures stayed in the top left of my screen the whole time"*.
/// It survived three fixes aimed at distance, at behind-the-camera projection and at
/// terrain occlusion — every one of which was reasonable, and every one of which was being
/// evaluated against the wrong camera, so all of them passed and none of them helped. A
/// guard is only as good as the frame you ask it in, which is the same lesson the corridor
/// frame keeps teaching the world generator.
///
/// So the filter is a NAMED TYPE rather than something each system remembers: it was the
/// only camera query in this file missing `With<Camera3d>`, which is exactly the kind of
/// omission a shared alias makes impossible instead of merely unlikely.
pub(crate) type WorldCamera<'w, 's> =
    Query<'w, 's, (&'static Camera, &'static GlobalTransform), With<Camera3d>>;

pub(crate) fn overworld_click_menu(
    mouse: Res<ButtonInput<MouseButton>>,
    touches: Res<Touches>,
    windows: Query<&Window>,
    cam_q: WorldCamera,
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
        net.0.fetch_bounties();
        inv.loaded = false;
        net.0.fetch_inventory();
    }
}

/// CL-2 — tap a creature to have the party's Psyker PIN it where it stands. The reach,
/// the cooldown and how many can be held at once are all the server's (`run.perks`); this
/// only decides *which* creature was pointed at and asks. A refusal is a no-op there, so
/// the affordance is gated here too — a button that does nothing teaches nothing.
///
/// Deliberately separate from [`overworld_click_menu`], which hit-tests the player's OWN
/// avatar to open the menu: the two never want the same sprite, and folding a second
/// target set into that system would make one click mean two things.
pub(crate) fn psyker_hold_click(
    mouse: Res<ButtonInput<MouseButton>>,
    touches: Res<Touches>,
    windows: Query<&Window>,
    cam_q: WorldCamera,
    world: Res<Overworld>,
    session: Res<Session>,
    perks: Res<PerksRes>,
    look: Res<hd2d::Look>,
    net: NonSend<NetRes>,
    overlay: Res<Overlay>,
    ui_hit: Query<&Interaction, With<Button>>,
    mut press: Local<Option<Vec2>>,
) {
    if overlay.kind.is_some() || session.channeling || perks.0.psyker_hold_targets == 0 {
        return;
    }
    let win = windows.iter().next();
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
        return;
    }
    let Some((cam, cam_tf)) = cam_q.iter().next() else { return };
    let Some(me) = world.entities.get(&session.player_id) else { return };

    // Nearest creature whose sprite the tap landed on, and which is actually in reach —
    // the server checks reach too, but asking for something it will refuse just spends
    // the press.
    let reach = perks.0.psyker_hold_radius;
    let mut best: Option<(f32, String)> = None;
    for (id, e) in world.entities.iter() {
        if !matches!(e.kind, EntityKind::Monster) || e.held {
            continue;
        }
        if Vec2::new(e.x - me.x, e.y - me.y).length() > reach {
            continue;
        }
        let base_y = e.level as f32 * STEP_HEIGHT + crate::world_render::terrain_height(e.x, e.y);
        let feet_w = Vec3::new(e.x, base_y, e.y);
        let head_w = feet_w + Vec3::Y * (look.sprite_y * 2.0);
        let (Ok(feet_s), Ok(head_s)) = (
            cam.world_to_viewport(cam_tf, feet_w),
            cam.world_to_viewport(cam_tf, head_w),
        ) else {
            continue;
        };
        let radius = ((head_s - feet_s).length() * 0.6).max(30.0);
        let d = seg_point_dist(p, feet_s, head_s);
        if d < radius && best.as_ref().is_none_or(|(bd, _)| d < *bd) {
            best = Some((d, id.clone()));
        }
    }
    if let Some((_, entity_id)) = best {
        net.0.send(ClientCmd::PsykerHold { entity_id });
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
    roster: Res<crate::PartyRoster>,
    keys: Res<ButtonInput<KeyCode>>,
    touches: Res<Touches>,
    autoplay: Res<Autoplay>,
    overlay: Res<Overlay>,
    session: Res<Session>,
    world: Res<Overworld>,
    windows: Query<&Window>,
    cam_q: WorldCamera,
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
    // DISTRACTED: the controls fight you. Applied to the KEYBOARD/stick heading only —
    // a tap-to-move destination is a place you pointed at, and reversing that would read as
    // the game ignoring the click rather than as a condition.
    if roster.heroes.iter().any(|h| h.afflictions.iter().any(|a| a == "distracted")) {
        mv = -mv;
    }
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
    mut world: ResMut<Overworld>,
    session: Res<Session>,
    look: Res<hd2d::Look>,
    time: Res<Time>,
    wa: Option<Res<WorldAssets>>,
    mut mats: ResMut<Assets<StandardMaterial>>,
    // One material per sprite rather than one per prop — see `SpriteMats`.
    mut sprite_mats: ResMut<SpriteMats>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut interp: ResMut<OwInterp>,
    dungeon: Res<world_render::DungeonSceneRes>,
    mut q: Query<(Entity, &WorldEntity, &mut Transform)>,
) {
    let Some(wa) = wa else { return };
    // Every water body in the snapshot, so a pool being spawned can tell whether its rim
    // is really a shore or just the middle of a larger mere (`blob_basin_mesh_merged`).
    // Gathered once per pass rather than per prop: the Mire's entire fill is water.
    let water_bodies: Vec<(f32, f32, f32)> = world
        .entities
        .values()
        .filter(|e| {
            matches!(e.name.as_deref(), Some("pond") | Some("frozen_pond") | Some("bog_pool"))
        })
        .map(|e| (e.x, e.y, e.radius.max(0.4)))
        .collect();
    // Consumed here rather than held: a correction applies to exactly one frame, and a
    // sticky one would pin the avatar in place the moment the player walked away.
    let snap = world.snap.take();
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
            match snap {
                // An authoritative teleport (a Shift walked us out of the props it just
                // strewed on our head). Chasing it would render as a second-long slide
                // through everything in between, with the camera along for the ride.
                Some((sx, sy)) => {
                    tf.translation.x = sx;
                    tf.translation.z = sy;
                }
                // Responsive: chase the latest snapshot directly.
                None => {
                    tf.translation.x += (e.x - tf.translation.x) * k;
                    tf.translation.z += (e.y - tf.translation.z) * k;
                }
            }
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
                // A boss with authored 8-direction animation frames (`bosses/<key>/`)
                // renders as an ANIMATED, camera-facing `CharSprite` — idle breathing +
                // turning as the camera orbits — just like a hero, instead of a single
                // frozen billboard. Regular creatures (single-PNG art) keep the billboard.
                //
                // The boss IDENTITY comes off the tag (`boss:<key>`, FS-4), not off the
                // creature kind. A boss overlays a host creature, so its kind is the
                // wildlife it rode in on: reading the kind alone worked only for the
                // dungeon props that carry a boss key in the kind slot, and drew every
                // Gatekeeper, every end-fight peer and every bounty mark as ordinary
                // fauna — the same fight, rendered as the thing it is standing in for.
                // Kind stays the fallback, so those dungeon props still resolve.
                let kind = creature_kind(e.name.as_deref().unwrap_or(""));
                let boss_key = e.boss.as_deref().unwrap_or(&kind);
                if let Some(frames) = wa.boss_frames(boss_key) {
                    let scale = match e.encounter_class.as_deref() {
                        Some("gatekeeper") => 2.6,
                        _ => 2.0,
                    };
                    let tint = if e.battling {
                        Color::srgb(1.5, 0.6, 0.5) // fighting → hot
                    } else {
                        Color::srgb(1.25, 1.12, 1.06) // looming, faintly warm
                    };
                    spawn_boss_char(&mut commands, &mut mats, &wa, id, e, frames, scale, tint);
                    continue;
                }
                // An ordinary creature with an installed sprite set renders the way a
                // boss does — animated, camera-facing, turning as it walks — instead of
                // as one frozen billboard. A PACK'S RUNT GETS ITS OWN ART:
                // `encounter_class` already rode the snapshot ("leader"/"minion"), the
                // client just never used it for anything but boss scale, so a leader at
                // 1.7x HP and its minion at 0.45x drew as the same animal out in the
                // world and only separated once you touched them. The BASE art is the
                // ordinary creature (a lone spawn, or a pack's minions); only the leader
                // reaches for a set of its own.
                let leader = e.encounter_class.as_deref() == Some("leader");
                if let Some(frames) = wa.creature_frames(&kind, leader).cloned() {
                    let scale = 2.0 * pack_scale_for(e.encounter_class.as_deref());
                    let tint = if e.battling {
                        Color::srgb(1.4, 0.75, 0.55)
                    } else {
                        Color::srgb(1.2, 1.15, 1.1)
                    };
                    spawn_boss_char(&mut commands, &mut mats, &wa, id, e, &frames, scale, tint);
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
                spawn_billboard_entity(&mut commands, &mut mats, &wa, id, e, tex, size, tint, 0.55, None, None);
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
                    None,
                    None,
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
                        WorldAssetRoot(scene.clone()),
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
                spawn_obstacle(
                    &mut commands, &mut mats, &mut sprite_mats, &mut meshes, &wa, id, e, theme,
                    &water_bodies,
                );
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
                    None,
                    None,
                );
                add_ground_ring(&mut commands, &wa, root);
            }
            EntityKind::Structure => {
                // A player-built structure, out of the same Kenney kit Last City is built
                // from: a timber palisade for a wall, a standing stone for an anchor.
                //
                // ⚠️ IT USED TO BE A TINTED PORTAL ARCH — the identical billboard a dungeon
                // exit uses. So the entire player-building pillar drew as "there is a portal
                // here", and a wall and an anchor were the same picture in two shades of
                // blue. A building is GEOMETRY rather than a billboard on purpose: you walk
                // around it and it has to occlude and cast shadow from any angle, and a
                // billboard's shading normals swing with the camera (fixed earlier, but a
                // flat quad would still read as paper when you orbit).
                let going_up = e.opened;
                let function = e.name.as_deref().unwrap_or("wall");
                let root = commands
                    .spawn((
                        WorldEntity(id.clone()),
                        Transform::from_translation(world_pos(e.x, e.y, 0.0)),
                        Visibility::default(),
                    ))
                    .id();
                match wa.structure_parts.get(function) {
                    Some(parts) => {
                        for (scene, off, yaw, scale) in parts {
                            // While it is still going up it stands PART WAY out of the
                            // ground, rather than being drawn dimmer: a half-built wall is
                            // legible as half-built from any distance, and sinking it avoids
                            // reaching into a GLB's materials to tint them.
                            let grow = if going_up { 0.45 } else { 1.0 };
                            let child = commands
                                .spawn((
                                    WorldAssetRoot(scene.clone()),
                                    Transform::from_translation(Vec3::new(
                                        off.x,
                                        off.y - (1.0 - grow) * 1.6,
                                        off.z,
                                    ))
                                    .with_scale(Vec3::splat(scale * grow))
                                    .with_rotation(Quat::from_rotation_y(yaw.to_radians())),
                                ))
                                .id();
                            commands.entity(root).add_child(child);
                        }
                    }
                    // A function with no art is a bug, not a case to design around — but it
                    // must still be visible enough to walk up to and demolish.
                    None => {
                        let child = commands
                            .spawn((
                                Mesh3d(wa.sprite_quad.clone()),
                                MeshMaterial3d(mats.add(hd2d::sprite_material(
                                    Color::srgb(1.0, 0.2, 0.8),
                                    wa.portal_sprite.clone(),
                                ))),
                                Transform::from_xyz(0.0, 1.2, 0.0),
                                hd2d::Billboard,
                            ))
                            .id();
                        commands.entity(root).add_child(child);
                    }
                }
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
                    None,
                    None,
                );
                add_ground_ring(&mut commands, &wa, root);
            }
            EntityKind::Trap => {
                // A trap the party's Shifter has read. Drawn low so it reads as "do not
                // stand here" without hiding the floor — the server only ever sends the
                // armed ones inside the Runner's sense.
                //
                // It draws its OWN KIND now. The kind has always been on the wire
                // (`trap:<kind>`) and every trap alike rendered as the target marker
                // tinted red, so the one thing the warning could have told you — what it
                // is you are about to stand on — was the one thing it did not. Anything
                // without art keeps the marker, tinted, rather than vanishing.
                // Four sprites per kind, picked by the trap's own id so a given trap
                // looks the same every time you walk past it while a corridor of them
                // does not read as one sprite stamped six times.
                let art = e.name.as_deref().and_then(|k| {
                    let n = hash_pick(id, TRAP_VARIANTS);
                    wa.prop_sprites.get(&format!("trap_{k}_{n}")).cloned()
                });
                let known = art.is_some();
                spawn_billboard_entity(
                    &mut commands,
                    &mut mats,
                    &wa,
                    id,
                    e,
                    art.or_else(|| wa.prop_sprites.get("marker_target_marker").cloned())
                        .unwrap_or_default(),
                    0.9,
                    // Bespoke art carries its own colour; the fallback marker still needs
                    // the red to mean anything at all.
                    if known {
                        Color::srgb(1.25, 1.0, 1.0)
                    } else {
                        Color::srgb(1.4, 0.35, 0.3)
                    },
                    0.2,
                    None,
                    None,
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
                    None,
                    None,
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
            let Some(mut m) = mats.get_mut(&mm.0) else {
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
                WorldAssetRoot(assets.load(GltfAssetLabel::Scene(0).from_asset(format!("models/{path}.glb")))),
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
        // ⚠️ AND BOUNDED BY THE GROUND IT STANDS ON, WHICH IT WAS NOT. The rampart ran a
        // flat ±14 while the crossing it straddles is `APPROACH_HALF_WIDTH` (9) — so its
        // outermost segment and both end towers stood IN THE SEA. The gate is a gate on a
        // bridge: there is room for a gatehouse across the deck and not for a curtain wall,
        // and saying so is better than building one out over the water. `- 1.5` keeps the
        // stonework inside the parapets rather than on them.
        let deck_half = if arc_deg > 0.0 {
            meld_proto::coast::APPROACH_HALF_WIDTH
        } else {
            f32::INFINITY
        };
        let wall_half: f32 =
            if arc_deg > 0.0 { 14.0_f32.min(deck_half - 1.5).max(0.0) } else { 44.0 };
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
        // Gatehouse dead-centre (the doorway) + two flanking towers with pennants. The
        // gatehouse always goes up — it IS the Threshold — but everything beside it is
        // conditional on there being dry ground under it.
        prop(&mut commands, "pirate/castle-gate", wx, 0.0, gate_yaw, WALL_SCALE);
        for tz in [-(GATE_HALF + 1.0), GATE_HALF + 1.0] {
            if tz.abs() <= wall_half {
                prop(&mut commands, "pirate/tower-complete-large", wx, tz, gate_yaw, 3.5);
                prop(&mut commands, "pirate/flag-high", wx, tz, gate_yaw, 3.5);
            }
        }
        // A tower capping each end of the rampart — only where there IS a rampart.
        if wall_half > GATE_HALF + 2.0 {
            for tz in [-wall_half + 2.0, wall_half - 2.0] {
                prop(&mut commands, "pirate/tower-complete-small", wx, tz, gate_yaw, 3.0);
            }
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
        // Bespoke HD-2D chest billboard (PixelLab): closed vs. overflowing-open art,
        // and CLOSED ART VARIES BY TIER. `chest:<tier>` has ridden the wire since chests
        // existed and every one of them drew as the common brown box — so the blue art
        // shipped in `PROP_KEYS` was never once rendered, and a deep chest looked exactly
        // like the one on the on-ramp. The tier is the whole promise of walking further.
        let key = if opened {
            "item_chest_open"
        } else {
            chest_art(e.chest_tier)
        };
        let tex = wa.prop_sprites.get(key).cloned().unwrap_or_default();
        // A red chest is the best loot in the game and is drawn to say so — its size is
        // the only thing that reads from across a room, before any tooltip or colour.
        let chest_h = if !opened && key == "item_chest_red" { 2.6 } else { 1.5 };
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
                    Transform::from_xyz(0.0, chest_h * 0.47, 0.0)
                        .with_scale(Vec3::splat(chest_h / 2.2)),
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
                hd2d::ContactShadow,
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
                        shadow_maps_enabled: false,
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
/// How big a pack member draws on the OVERWORLD. The battle arena has the same rule in
/// `battle::pack_scale`, but reads it off the combatant `statuses` (`pack:leader`); out
/// in the world the same fact rides `encounter_class`, which was already on the snapshot
/// and unused. Same numbers, so a creature does not change size when the fight starts.
pub(crate) fn pack_scale_for(encounter_class: Option<&str>) -> f32 {
    match encounter_class {
        Some("leader") => 1.3,
        Some("minion") => 0.75,
        _ => 1.0,
    }
}

pub(crate) fn spawn_boss_char(
    commands: &mut Commands,
    mats: &mut Assets<StandardMaterial>,
    wa: &WorldAssets,
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
                // Grounded through the one helper: `sprite_y` scales WITH the quad, not by
                // it. See `hd2d::grounded_sprite_y` — the flying-boar bug lived on this line.
                Transform::from_xyz(0.0, hd2d::grounded_sprite_y(scale), 0.0)
                    .with_scale(Vec3::splat(scale)),
                hd2d::Billboard,
            ));
            p.spawn((
                Mesh3d(wa.shadow_mesh.clone()),
                MeshMaterial3d(wa.shadow_mat.clone()),
                hd2d::ContactShadow,
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
                hd2d::ContactShadow,
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
    roster: Res<PartyRoster>,
    lead_q: Query<(&WorldEntity, &Transform, &CharSprite), Without<PartyFollower>>,
    mut followers: Query<(Entity, &PartyFollower, &mut Transform), With<PartyFollower>>,
) {
    // WHO is actually on this dive comes from the server's roster, not from
    // `session.party` — that is the composition the client *asked* for, it defaults to a
    // four-class spread for a newcomer, and the server clamps it to the slots the account
    // has earned. Reading the request meant a player with two heroes walked the overworld
    // trailed by four, two of whom were nobody.
    let classes: Vec<&str> = if roster.heroes.is_empty() {
        session.party.iter().map(String::as_str).collect()
    } else {
        roster.heroes.iter().map(|h| h.class_key.as_str()).collect()
    };
    // How many followers we want: every party member after the lead (cap 3).
    let want = if pv.show {
        classes.len().min(4).saturating_sub(1)
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
            let class = classes.get(slot).copied().unwrap_or("explorer");
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
/// Root UI node for the corner minimap.
#[derive(Component)]
pub(crate) struct MinimapRoot;
/// One minimap dot, rebuilt each frame.
#[derive(Component)]
pub(crate) struct MinimapDot;

/// Rebuild the corner minimap's blips over its ground. The Explorer's map perk gates it.
#[allow(clippy::type_complexity)]
pub(crate) fn update_minimap(
    mut commands: Commands,
    perks: Res<PerksRes>,
    world: Res<Overworld>,
    session: Res<Session>,
    view: Res<crate::minimap::MapView>,
    mut root_q: Query<(Entity, &mut Node), With<MinimapRoot>>,
    old: Query<Entity, With<MinimapDot>>,
) {
    for e in &old {
        commands.entity(e).despawn();
    }
    let Ok((root, mut node)) = root_q.single_mut() else { return };
    let tier = perks.0.explorer_map;
    node.display = if tier >= 1 { Display::Flex } else { Display::None };
    if tier == 0 {
        return;
    }
    let Some(me) = world.entities.get(&session.player_id) else { return };
    const HALF: f32 = 70.0;
    const R: f32 = 68.0;
    // The dots must use the SAME framing the ground was drawn at, or the blips float over a
    // map of somewhere else. `MapView` is that framing, so there is one answer to "what is
    // this panel showing" rather than two that drift.
    let units = view.units.max(0.0001);
    let scale = R / (units * crate::minimap::corner_tiles_half());
    let shifter_sense = perks.0.shifter_dungeon_radius;
    commands.entity(root).with_children(|p| {
        spawn_dot(p, HALF, HALF, 6.0, Color::srgb(1.0, 1.0, 1.0));
        for e in world.entities.values() {
            let (col, size) = match e.kind {
                EntityKind::Monster => (Color::srgb(1.0, 0.4, 0.35), 5.0),
                EntityKind::Portal => (Color::srgb(0.4, 0.85, 1.0), 6.0),
                EntityKind::Chest if tier >= 2 => (Color::srgb(1.0, 0.82, 0.3), 5.0),
                EntityKind::Resource if tier >= 3 => (Color::srgb(0.5, 0.95, 0.5), 4.0),
                EntityKind::Entrance if shifter_sense > 0.0 => {
                    (Color::srgb(0.85, 0.55, 1.0), 7.0)
                }
                _ => continue,
            };
            if e.kind == EntityKind::Entrance {
                let d = ((e.x - me.x).powi(2) + (e.y - me.y).powi(2)).sqrt();
                if d > shifter_sense {
                    continue;
                }
            }
            let (dx, dy) = ((e.x - me.x) * scale, (e.y - me.y) * scale);
            if dx.abs() > R || dy.abs() > R {
                continue;
            }
            spawn_dot(p, HALF + dx, HALF + dy, size, col);
        }
    });
}

/// Spawn one absolutely-positioned map dot centred at (`cx`,`cy`) px.
pub(crate) fn spawn_dot(p: &mut ChildSpawnerCommands, cx: f32, cy: f32, size: f32, col: Color) {
    p.spawn((
        MinimapDot,
        Node {
            border_radius: BorderRadius::all(Val::Percent(50.0)),
            position_type: PositionType::Absolute,
            left: Val::Px(cx - size / 2.0),
            top: Val::Px(cy - size / 2.0),
            width: Val::Px(size),
            height: Val::Px(size),
            ..default()
        },
        BackgroundColor(col),
    ));
}

/// The depth readout beneath the corner minimap. The map moved to the
/// menu's Map column ([`crate::minimap`]); this line stayed, because "how deep am I" is a
/// HUD fact you need while walking and not a thing you open a menu for.
#[derive(Component)]
pub(crate) struct MinimapDistance;

/// How much further every carried light reaches than it used to. One constant so the
/// overworld avatar's lamp and the battle party's lamps cannot drift apart.
pub(crate) const LAMP_REACH_MULT: f32 = 4.0 / 3.0;

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
        // Widened by a third, the same third the battle party's lamps got
        // (`battle::LAMP_REACH`) — this is the one a player actually walks around inside,
        // so the two had to move together or "the light reaches further" would be true in
        // a fight and false on the road.
        light.range = (12.0 + glow / 8000.0) * LAMP_REACH_MULT;
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
    // NOT a battle sprite: `animate_battle_actors` owns the emissive on those, folding
    // this same night glow in beside its flash and rage. Two unordered writers to one
    // field is a coin flip per frame, and it read as the party's lights flickering
    // through a whole fight.
    sprites: Query<
        &MeshMaterial3d<StandardMaterial>,
        (With<PlayerGlowSprite>, Without<crate::battle::SpriteQuad>),
    >,
    mut lamps: Query<(&mut PointLight, &BattlePartyLamp)>,
) {
    let night = (1.0 - sky.day).clamp(0.0, 1.0);
    // Self-illumination: warm glow keyed off each sprite's own texture colours.
    let ef = night * 1.15;
    for mh in &sprites {
        if let Some(mut m) = mats.get_mut(&mh.0) {
            // Only the COLOUR here. Which frame lights up is `animate_chars`' business,
            // set alongside the base texture so the two can never disagree — read from
            // here it was a frame stale on whichever frames the scheduler happened to run
            // this system first, and the hero juddered in the dark.
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
    session: Res<Session>,
    look: Res<hd2d::Look>,
    cam_q: WorldCamera,
    root_q: Query<Entity, With<NameplateRoot>>,
    mob_q: Query<(&WorldEntity, &GlobalTransform)>,
    old: Query<Entity, With<Nameplate>>,
) {
    // Clear last frame's plates.
    for e in &old {
        commands.entity(e).despawn();
    }
    let intel = perks.0.hunter_intel;
    let threat = perks.0.hunter_threat;
    if !nameplates_wanted(intel, threat, &world) {
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
            // ⚠️ **DO NOT LABEL WHAT THE FOG HIDES.** Being in front of the camera and
            // inside the viewport is not the same as being VISIBLE. A creature past the
            // hill crest still projects — to a point just above the terrain silhouette —
            // so its plate hangs in the empty sky with no creature under it, and because it
            // is far away it barely moves as the player walks. Reported exactly that way:
            // "those creatures stayed in the top left of my screen the whole time", with
            // several plates overlapping into one unreadable clump, because distant mobs
            // project to nearly the same pixel.
            //
            // This is the other half of the behind-the-camera bug fixed below — that guard
            // answered "is it behind me", and this one answers "can it be seen at all". The
            // cap is the FOG, which is the distance the renderer itself stops showing the
            // world at, so the plate and the creature appear and disappear together instead
            // of the label outliving the thing it names.
            if let Some(me) = world.entities.get(&session.player_id) {
                let away = Vec2::new(ent.x - me.x, ent.y - me.y).length();
                if !plate_is_close_enough_to_see(away, look.fog_on, look.fog_end) {
                    continue;
                }
            }
            // Project a point above the mob's head to the screen.
            let head = gtf.translation() + Vec3::Y * 2.6;
            // ⚠️ BEHIND THE CAMERA STILL PROJECTS. `world_to_viewport` returns `Ok` for a
            // point behind the viewer, with coordinates that land wherever the perspective
            // divide throws them — which is how a boss forty units behind the party ends up
            // as a nameplate pinned in the TOP-LEFT CORNER of the screen. It has been in
            // most of today's captures ("Rustfang", with a health bar, floating in the sky)
            // and reads as a UI glitch because it is one.
            //
            // So the projection is not enough on its own: the point has to be in FRONT of
            // the camera, and inside the viewport.
            if cam_tf.forward().dot(head - cam_tf.translation()) <= 0.0 {
                continue;
            }
            let Some(s) = cam.world_to_viewport(cam_tf, head).ok() else {
                continue;
            };
            // ...and on screen. A plate half a screen out is not information, it is clutter
            // clinging to an edge.
            if let Some(size) = cam.logical_viewport_size() {
                if s.x < 0.0 || s.y < 0.0 || s.x > size.x || s.y > size.y {
                    continue;
                }
            }
            // Threat marker (Hunter): elites/gatekeepers, then aggressive mobs.
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
                // FS-4: a named boss WEARS ITS NAME. A Gatekeeper stands in the pass on
                // every run and the end fight is what the whole walk out is pointed at,
                // and until the tag carried an identity both read as the wildlife they
                // overlay — you learned what you had touched by touching it. Ungated for
                // the same reason the ⚔ and the QUARRY plate are: this is the world
                // saying what that is, not intel a perk buys. Unknown keys are titleless
                // rather than titled a guess.
                if let Some(title) = ent.boss.as_deref().and_then(meld_proto::bosses::display_name) {
                    c.spawn((
                        Text::new(title),
                        TextFont { font_size: FontSize::Px(12.0), ..default() },
                        TextColor(Color::srgb(1.0, 0.6, 0.55)),
                    ));
                }
                // HOW BIG THE FIGHT IS, above everything else on the plate and in the
                // loudest colour, because it is the only line on it a player has to act on
                // BEFORE touching the creature. Every other marker describes what the thing
                // is; this one says whether you should be here at all. Measured: a solo
                // party ground a four-party-sized gatekeeper for 464 turns with nothing on
                // screen to warn them.
                let scale = meld_proto::warbands::warband(ent.expects_parties);
                if meld_proto::warbands::is_raid(scale.parties) {
                    c.spawn((
                        Text::new(scale.title.to_uppercase()),
                        TextFont { font_size: FontSize::Px(13.0), ..default() },
                        TextColor(Color::srgb(1.0, 0.45, 0.35)),
                    ));
                    c.spawn((
                        Text::new(format!("{} parties", scale.parties)),
                        TextFont { font_size: FontSize::Px(10.0), ..default() },
                        TextColor(Color::srgb(1.0, 0.7, 0.6)),
                    ));
                }
                // What you came out here for, over its head, in the board's own word.
                if ent.quarry {
                    c.spawn((
                        Text::new("QUARRY"),
                        TextFont { font_size: FontSize::Px(11.0), ..default() },
                        TextColor(Color::srgb(1.0, 0.85, 0.35)),
                    ));
                }
                // A pinned creature has to READ as pinned, or the cooldown was spent on
                // something invisible — and the opening it buys expires.
                if ent.held {
                    c.spawn((
                        Text::new("HELD"),
                        TextFont { font_size: FontSize::Px(11.0), ..default() },
                        TextColor(Color::srgb(0.62, 0.72, 1.0)),
                    ));
                }
                // The same crossed-swords a fighting player wears, because it is the same
                // fact: this thing is in a fight. Whether to wait it out and take what
                // falls is the decision, and it cannot be made from an unmarked creature.
                if ent.clashing {
                    c.spawn((
                        Text::new("\u{f0817}"),
                        TextFont { font_size: FontSize::Px(14.0), ..default() },
                        TextColor(Color::srgb(1.0, 0.55, 0.4)),
                    ));
                }
                if !marker.is_empty() {
                    c.spawn((
                        Text::new(marker),
                        TextFont { font_size: FontSize::Px(13.0), ..default() },
                        TextColor(marker_col),
                    ));
                }
                if intel >= 1 {
                    let lvl = ent.mob_level.unwrap_or(0);
                    c.spawn((
                        Text::new(format!("Lv {lvl}")),
                        TextFont { font_size: FontSize::Px(12.0), ..default() },
                        TextColor(Color::srgb(0.95, 0.95, 1.0)),
                    ));
                }
                // A clash's whole tension is who is losing it, and a WOUND is the reason
                // to hurry — so neither bar is perk-gated. The clash resolves in seconds
                // and a watcher deciding whether to step in has no other way to read it;
                // the wound closes in under a minute and is the difference between a fight
                // worth taking and one that isn't.
                if intel >= 2 || ent.clashing || wounded(ent) {
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
                            BorderColor::all(Color::srgba(0.0, 0.0, 0.0, 0.7)),
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

/// Is a creature near enough that a plate over it means anything?
///
/// ⚠️ **BEING ON SCREEN IS NOT BEING VISIBLE.** `update_mob_nameplates` already refuses a
/// point behind the camera and one outside the viewport, and that is still not enough: a
/// creature past the hill crest projects to just above the terrain silhouette, so its plate
/// hangs in empty sky with nothing under it, and being far away it barely moves as the
/// player walks. Reported as "those creatures stayed in the top left of my screen the whole
/// time" — several distant mobs projecting to nearly the same pixel and stacking into one
/// unreadable clump.
///
/// The cap is the FOG, because that is the distance the renderer itself stops showing the
/// world at: the label and the thing it names then appear and disappear together. With fog
/// off (look-dev) there is no horizon to speak of, so nothing is culled — that is a
/// deliberate escape hatch for inspecting the world, not an oversight.
pub(crate) fn plate_is_close_enough_to_see(away: f32, fog_on: bool, fog_end: f32) -> bool {
    // ⚠️ **THE FOG IS THE WRONG DISTANCE, AND GATING ON IT LET THE BUG BACK.** A plate may
    // only name a creature that is actually ON SCREEN, and a creature's sprite is despawned
    // at `RENDER_UNLOAD_FAR` (240) — well inside `fog_end` (500). So every mob in that
    // 260-unit band had its body culled and its label kept: bars hanging in empty sky, barely
    // moving because they are distant, piling into a screen corner. Exactly the clump this
    // function was written to fix, reported a second time.
    //
    // The rule is "as close as the creature is DRAWN", which is the nearer of the two — and
    // the render cull binds whether the fog is on or not, so look-dev's fog toggle cannot opt
    // out of it either.
    away <= RENDER_UNLOAD_FAR && (!fog_on || away <= fog_end)
}

/// Is there anything over a creature's head to draw right now?
///
/// Two of these are PERK readouts (level and the HP bar are the Hunter's intel; the `!!!`
/// threat marker is his eye at range) and four are FACTS the world is reporting: the
/// hunt you are holding (`quarry`), the pin you just spent (`held`), a fight actually
/// happening in front of you (`clash`, `CR-2`), and which named boss a creature IS
/// (`boss`, FS-4). Only the first pair may be gated on a perk. Getting that wrong is silent: an ungeared party would see a brawl go on beside
/// them with nothing on screen to say so, and never learn that waiting it out leaves loot.
pub(crate) fn nameplates_wanted(intel: u8, threat: u8, world: &Overworld) -> bool {
    intel > 0
        || threat > 0
        || world
            .entities
            .values()
            .any(|e| e.quarry || e.held || e.clashing || wounded(e) || named_boss(e))
}

/// Is this creature a NAMED boss (FS-4) the plate can title?
///
/// A fourth thing the world reports rather than a perk readout: a boss overlays a host
/// creature, so nothing about its billboard or its kind says which of the ten it is, and
/// a Gatekeeper in the pass is exactly the creature a player must be able to identify
/// *before* walking into it. Gated on the title resolving, so a dungeon's bespoke sprite
/// asks for no plate it has nothing to put on.
pub(crate) fn named_boss(e: &OwEntity) -> bool {
    e.boss.as_deref().and_then(meld_proto::bosses::display_name).is_some()
}

/// Is this creature carrying a wound (`CR-2`)?
///
/// Creature HP persists — a skirmish it survived, a fight a party fled — and it mends
/// only slowly, so a hurt creature is a **time-bound opportunity**. Which is worth exactly
/// nothing if you cannot see it: an unmarked creature at 20% looks like an unmarked
/// creature at 100%, and the whole mechanic becomes something the server knows and the
/// player does not. So the bar shows for anyone, like the ⚔ and the QUARRY plate — a wound
/// is an event the world is reporting, not intel a perk buys. The Hunter's `intel >= 2`
/// still reads the bar on UNTOUCHED creatures, which is the 95% case and where sizing one
/// up actually matters.
pub(crate) fn wounded(e: &OwEntity) -> bool {
    matches!((e.hp, e.max_hp), (Some(hp), Some(max)) if max > 0 && hp < max)
}

/// The depth readout under the minimap: distance, its tier, and the biome it is in.
/// Rides the Explorer's map perk, so it appears and vanishes with the panel above it.
pub(crate) fn update_minimap_distance(
    perks: Res<PerksRes>,
    stats: Res<RunStats>,
    tell: Res<crate::ShiftTell>,
    clock: Res<Time>,
    mut q: Query<(&mut Text, &mut Node), With<MinimapDistance>>,
) {
    let Ok((mut text, mut node)) = q.single_mut() else { return };
    if perks.0.explorer_map == 0 {
        node.display = Display::None;
        return;
    }
    node.display = Display::Flex;
    let mut line = format!("{} m  \u{b7}  T{}  \u{b7}  {}", stats.distance, stats.tier, stats.biome);
    // The countdown lives on the line that already answers "where am I", because the
    // question a tell raises is "am I in it" — and the ring's own burning edge on the
    // ground is what answers "where is it".
    let left = tell.lands_at - clock.elapsed_secs_f64();
    if tell.armed && left > 0.0 {
        let secs = left.ceil() as i64;
        line.push_str(&if tell.caught {
            format!("\n! SHIFTING TO {} IN {secs}s - GET OUT", tell.biome.to_uppercase())
        } else {
            format!("\n~ {} shifting in {secs}s", tell.biome)
        });
    }
    if **text != line {
        **text = line;
    }
}

/// Deterministically pick an index in `0..n` from an entity id (FNV-1a). Lets a
/// grove of identical-kind obstacles show varied art without any per-entity state.
/// How many sprites each trap kind has. Four, and the pick is by the trap's own id.
pub(crate) const TRAP_VARIANTS: usize = 4;

/// Which chest art a tier gets. `tier(d) = floor(d/100)`, and `red_chest_floor_distance`
/// (d=300, CANON §B: no gear drops shallower) is where the gear game starts — so the RED
/// chest marks the band that can hold real gear, and the blue one the shallow-but-not-
/// starter middle. Every chest drew as the common brown box before this, which meant the
/// blue art already in `PROP_KEYS` had never once been shown and depth looked identical
/// from the on-ramp to the frontier.
pub(crate) fn chest_art(tier: i32) -> &'static str {
    match tier {
        t if t >= 3 => "item_chest_red",
        t if t >= 1 => "item_chest_rare",
        _ => "item_chest_common",
    }
}

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







/// `sway` is the wind-lean amplitude from [`sway_amp`], or `None` for anything rigid. It
/// lands on the QUAD rather than the root because a billboard owns its own world rotation
/// (see [`animate_sway`]); the root stays translation-only, which is the invariant
/// `hd2d::billboard` depends on.
/// **ONE MATERIAL PER SPRITE, NOT ONE PER ENTITY.**
///
/// ⚠️ EVERY BILLBOARD IN THE WORLD ALLOCATED ITS OWN `StandardMaterial`. The quad MESH was
/// shared, so it looked batched — but two trees with byte-identical materials are two
/// different assets to Bevy, and a distinct material is a distinct draw call. Measured near
/// the hub: **2,139 obstacles, so 2,139 materials and 2,139 draw calls** for what is a couple
/// of dozen distinct sprites. That is the real ceiling on draw distance — the wire cost is
/// fixable by streaming, but 10,277 draw calls at a 400-unit radius is not.
///
/// Keyed on the texture AND the tint, because the tint is the material's other input; every
/// obstacle passes `Color::WHITE`, so in practice obstacles collapse onto one material per
/// sprite.
///
/// ⚠️ **ONLY FOR SPRITES NOTHING MUTATES PER INSTANCE.** A shared material is shared
/// state: `pulse_collectibles`, `illuminate_players` and `update_reach_halo` all reach into
/// a material and write it, and the moment two entities share one, lighting up the thing in
/// reach lights up every other one of its kind. Obstacles are the verified-safe set (nothing
/// writes an obstacle's own material — the reach glow is a separate halo entity with its own
/// material), which is why the cache is threaded to the obstacle path and nowhere else.
/// **HOW HIGH A SPRITE SITS IS A PROPERTY OF ITS ART, NOT OF ITS CATEGORY.**
///
/// A billboard maps the WHOLE png onto its quad, so grounding the picture's feet means
/// knowing how much empty canvas sits below the art — and that varies per FILE. Measured
/// across the shipped set: characters 25.5%, creatures 26.6%, npcs 25.5%, bosses 25.4% —
/// one tight family — but **props range from 0% to 26.8% with a median of 18.6%**, and
/// landscape from 1.6% to 22.1%. So there is no per-category constant that grounds props,
/// and the two attempts that assumed one each broke half the world: putting the quad's
/// bottom edge on the floor floated every padded tree, and applying the CHARACTER padding
/// to props buried the unpadded ones to their waists (*"trees are halfway underground"*).
///
/// So measure the picture. The alpha bounding box is computed once per texture, on demand,
/// and cached by [`AssetId`] — every billboard of that sprite then grounds off its own art
/// for free, and a new asset needs no constant, no category and no tuning.
#[derive(Resource, Default)]
pub(crate) struct SpritePads(std::collections::HashMap<bevy::asset::AssetId<Image>, f32>);

impl SpritePads {
    /// The fraction of `tex`'s canvas that is empty below the art, or `None` while the
    /// image has not finished loading (or is in a format we cannot read).
    fn pad_of(&mut self, imgs: &Assets<Image>, tex: &Handle<Image>) -> Option<f32> {
        let id = tex.id();
        if let Some(p) = self.0.get(&id) {
            return Some(*p);
        }
        let img = imgs.get(id)?;
        let pad = measure_bottom_pad(img)?;
        self.0.insert(id, pad);
        Some(pad)
    }
}

/// The fraction of an image's height that is fully transparent below the artwork.
///
/// `None` for a format whose alpha we cannot read byte-wise — better to fall back to the
/// family default than to ground everything off a misread buffer.
fn measure_bottom_pad(img: &Image) -> Option<f32> {
    use bevy::render::render_resource::TextureFormat;
    if !matches!(
        img.texture_descriptor.format,
        TextureFormat::Rgba8Unorm | TextureFormat::Rgba8UnormSrgb
    ) {
        return None;
    }
    let data = img.data.as_ref()?;
    let w = img.texture_descriptor.size.width as usize;
    let h = img.texture_descriptor.size.height as usize;
    if w == 0 || h == 0 || data.len() < w * h * 4 {
        return None;
    }
    // Walk up from the bottom row until a row holds an opaque-ish pixel. Anything at or
    // below `ALPHA_FLOOR` is the sprite's own soft edge rather than art standing on the
    // ground, and counting it would ground the sprite on its antialiasing.
    const ALPHA_FLOOR: u8 = 8;
    for row in (0..h).rev() {
        let base = row * w * 4;
        let (rows, _) = data[base..base + w * 4].as_chunks::<4>();
        if rows.iter().any(|px| px[3] > ALPHA_FLOOR) {
            return Some((h - 1 - row) as f32 / h as f32);
        }
    }
    None
}

/// A billboard still grounded on a FALLBACK, waiting for its texture to load so it can be
/// grounded on the art instead. Removed once it has been.
#[derive(Component)]
pub(crate) struct GroundOnArt {
    /// The quad's world height, which the centre is a fraction of.
    pub height: f32,
    pub tex: Handle<Image>,
}

/// Re-ground billboards whose texture has finished loading — see [`SpritePads`]. A sprite is
/// spawned on its family's default and corrected here, usually within a frame or two, so
/// nothing waits on the asset server to appear.
pub(crate) fn ground_billboards_on_their_art(
    mut commands: Commands,
    imgs: Res<Assets<Image>>,
    mut pads: ResMut<SpritePads>,
    mut q: Query<(Entity, &GroundOnArt, &mut Transform, Option<&mut crate::world_render::Sway>)>,
) {
    for (entity, g, mut t, sway) in &mut q {
        let Some(pad) = pads.pad_of(&imgs, &g.tex) else {
            continue;
        };
        t.translation.y = g.height * (0.5 - pad);
        // The sway pivots about the sprite, so it follows the grounding rather than
        // restating it — a pivot left behind rocks the tree about a point in the air.
        if let Some(mut sway) = sway {
            sway.pivot_y = t.translation.y;
        }
        commands.entity(entity).remove::<GroundOnArt>();
    }
}

#[derive(Resource, Default)]
pub(crate) struct SpriteMats(
    std::collections::HashMap<(bevy::asset::AssetId<Image>, u32), Handle<StandardMaterial>>,
);

impl SpriteMats {
    /// How many distinct sprite materials are shared right now.
    #[cfg(test)]
    fn len(&self) -> usize {
        self.0.len()
    }

    /// The shared material for this `(sprite, tint)`, minted on first use.
    pub(crate) fn get(
        &mut self,
        mats: &mut Assets<StandardMaterial>,
        tint: Color,
        tex: Handle<Image>,
    ) -> Handle<StandardMaterial> {
        // The tint as bits, so the key is exact rather than an epsilon compare.
        let c = tint.to_srgba();
        let key = (
            tex.id(),
            (((c.red * 255.0) as u32) << 16)
                | (((c.green * 255.0) as u32) << 8)
                | ((c.blue * 255.0) as u32),
        );
        self.0
            .entry(key)
            .or_insert_with(|| mats.add(hd2d::sprite_material(tint, tex)))
            .clone()
    }
}

#[allow(clippy::too_many_arguments)]
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
    sway: Option<f32>,
    // `Some` to take a SHARED material for this sprite — see `SpriteMats` for which
    // sprites may and which may not.
    shared: Option<&mut SpriteMats>,
) -> Entity {
    // The shared quad mesh is 2.2 world-units tall; scale to the wanted height and lift it
    // so the ART'S FEET — not the quad's bottom edge — sit on the ground plane. See
    // `hd2d::GROUNDED_CENTRE`: the difference between those two is a quarter of the
    // sprite's height, and it is what had the whole world hovering over its own shadows.
    let scale = height / hd2d::SPRITE_QUAD_HEIGHT;
    // A starting guess only: `ground_billboards_on_their_art` replaces it with the padding
    // measured from this very texture as soon as the image is available.
    let centre_y = hd2d::grounded_centre(height);
    let mat = match shared {
        // The shared path: one material per sprite (see `SpriteMats`).
        Some(cache) => cache.get(mats, tint, tex.clone()),
        // Per-instance, for the sprites something writes a material on.
        None => mats.add(hd2d::sprite_material(tint, tex.clone())),
    };
    commands
        .spawn((
            WorldEntity(id.to_string()),
            Transform::from_translation(world_pos(e.x, e.y, 0.0)),
            Visibility::default(),
        ))
        .with_children(|p| {
            let mut quad = p.spawn((
                Mesh3d(wa.sprite_quad.clone()),
                MeshMaterial3d(mat),
                Transform::from_xyz(0.0, centre_y, 0.0).with_scale(Vec3::splat(scale)),
                hd2d::Billboard,
                GroundOnArt { height, tex: tex.clone() },
            ));
            if let Some(amp) = sway {
                // Phase and speed off the id, so neighbouring trees never toss in lockstep.
                let h = hash_pick(id, 10000);
                quad.insert(Sway {
                    // The quad's own centre, whatever that is — the sway pivots about the
                    // sprite, so it has to follow the grounding rather than restate it.
                    pivot_y: centre_y,
                    phase: (h % 628) as f32 / 100.0,
                    amp,
                    speed: 0.7 + ((h / 628) % 60) as f32 / 100.0,
                });
            }
            if shadow > 0.0 {
                p.spawn((
                    Mesh3d(wa.shadow_mesh.clone()),
                    MeshMaterial3d(wa.shadow_mat.clone()),
                    hd2d::ContactShadow,
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

/// The art pool a WOODED obstacle kind draws from, or `None` if it is not one.
///
/// ⚠️ `obstacle_tree` IS DELIBERATELY NOT IN THE FOREST POOL. It is the RUNE tree —
/// carved, glowing — and while it sat in the ordinary rotation one in six trees in every
/// wood was a piece of standing magic that meant nothing. It stays loaded and reserved for
/// a tree dungeon's mouth.
pub(crate) fn tree_pool(kind: &str) -> Option<&'static [&'static str]> {
    Some(match kind {
        "tree" => &[
            "obstacle_tree_pine",
            "obstacle_tree_birch",
            "obstacle_tree_dead",
            "obstacle_tree_willow",
            "obstacle_tree_bushy",
        ],
        "amber_tree" => &[
            "obstacle_amber_tree_1",
            "obstacle_amber_tree_2",
            "obstacle_amber_tree_3",
            "obstacle_amber_tree_4",
        ],
        "mire_tree" => &[
            "obstacle_mire_tree_1",
            "obstacle_mire_tree_2",
            "obstacle_mire_tree_3",
            "obstacle_mire_tree_4",
        ],
        "snow_tree" => &[
            "obstacle_snow_tree_1",
            "obstacle_snow_tree_2",
            "obstacle_snow_tree_3",
            "obstacle_snow_tree_4",
        ],
        _ => return None,
    })
}

/// Spawn a terrain obstacle sized to its world radius. Vegetation and rock kinds are
/// **real 3D models** (Kenney Nature Kit, CC0) — one of several variants picked by id
/// hash and rotated for variety, so the world reads as dimensional HD-2D geometry
/// rather than flat cut-outs. Water kinds stay flat pools; anything unmapped falls
/// back to the lit boulder mesh.
pub(crate) fn spawn_obstacle(
    commands: &mut Commands,
    mats: &mut Assets<StandardMaterial>,
    // One material per sprite rather than one per tree — see `SpriteMats`. A forest is the
    // case that needs it: a desert can afford a draw call per prop, a wood cannot.
    sprite_mats: &mut SpriteMats,
    meshes: &mut Assets<Mesh>,
    wa: &WorldAssets,
    id: &str,
    e: &OwEntity,
    dungeon_theme: &str,
    // Every OTHER water body in the snapshot, as `(x, y, radius)`. A pool whose rim is
    // covered by one of these drops that rim to the waterline, so touching pools fuse into
    // a single body with one outer bank (see `hd2d::blob_basin_mesh_merged`).
    water_bodies: &[(f32, f32, f32)],
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
                "field" | "forest" => Color::srgb(0.56, 0.68, 0.50), // mossy stone
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
    let is_water = meld_proto::coast::is_water_kind(name);
    if !is_water {
        // Trees draw from a variety pool (oak/pine/birch/dead/willow/bushy) picked by
        // id-hash, with an extra per-id size factor on top of the radius so a forest
        // reads as a mix of shapes and heights rather than one stamped tree.
        // A WOOD IS THE BIOME IT GROWS IN. This used to be `if name == "tree"` with one
        // hardcoded pool, so a swamp, a tundra and an autumn wood all grew the same five
        // trees — the thing you walk through, which is most of what a biome looks like,
        // was the one part that never changed. Each wooded kind now has its own pool and
        // the server decides which kind a biome grows (`obstacles_for_biome`).
        if let Some(pool_keys) = tree_pool(name) {
            // ⚠️ `obstacle_tree` IS DELIBERATELY NOT IN THIS POOL. It is the RUNE tree —
            // carved, glowing — and while it sat in the ordinary rotation one in six trees
            // in every wood and meadow was a piece of standing magic that meant nothing.
            // A landmark that appears at random is scenery; the art is far too specific to
            // spend that way. It stays loaded (see `PROP_KEYS`) and reserved, to be placed
            // deliberately and much larger as the mouth of a tree dungeon — which is
            // `WG-?` and not built yet, so today it simply does not spawn.
            let pool: Vec<Handle<Image>> = pool_keys
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
                spawn_billboard_entity(
                    commands, mats, wa, id, e, tex, height, Color::WHITE, height * 0.28,
                    sway_amp(name),
                    Some(sprite_mats),
                );
                return;
            }
        }
        // The boulder has four rocks now; everything else still has one sprite.
        if name == "boulder" {
            let pool: Vec<Handle<Image>> = ["obstacle_boulder_1", "obstacle_boulder_2",
                "obstacle_boulder_3", "obstacle_boulder_4"]
                .iter()
                .filter_map(|k| wa.prop_sprites.get(*k).cloned())
                .collect();
            if !pool.is_empty() {
                let tex = pool[hash_pick(id, pool.len())].clone();
                let height = (1.8 + r * 0.8).clamp(1.8, 4.5);
                spawn_billboard_entity(
                    commands, mats, wa, id, e, tex, height, Color::WHITE, 0.55,
                    sway_amp(name),
                    Some(sprite_mats),
                );
                return;
            }
        }
        if let Some(tex) = wa.prop_sprites.get(&format!("obstacle_{name}")) {
            let height = (1.8 + r * 0.8).clamp(1.8, 4.5);
            spawn_billboard_entity(
                commands, mats, wa, id, e, tex.clone(), height, Color::WHITE, 0.55,
                sway_amp(name),
                Some(sprite_mats),
            );
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
            commands.spawn((
                WorldEntity(id.to_string()),
                WorldAssetRoot(scene.clone()),
                Transform::from_translation(world_pos(e.x, e.y, 0.0))
                    .with_scale(Vec3::splat(scale))
                    .with_rotation(Quat::from_rotation_y(yaw)),
            ));
            // ⚠️ NO SWAY ON THE 3D-MODEL PATH, AND THAT IS WHY TREES NEVER SWAYED. Every
            // kind `sway_amp` answers for — tree, cactus, fungal_wall — has a prop sprite in
            // `PROP_KEYS`, and the sprite branch above returns before reaching here. So this
            // was the ONLY place `Sway` was ever inserted, on a path nothing takes: the
            // feature had no live entities at all. It belongs on the billboard quad, where
            // `spawn_billboard_entity` now puts it. A swaying 3D model would need its own
            // path, since rotating a mesh root and rotating a billboard are different
            // problems.
            return;
        }
    }
    match name {
        "pond" | "frozen_pond" | "bog_pool" => {
            // Bespoke pixel-art water tile per kind (drifted by `animate_water`); spin
            // each organic blob a different way so pools don't look stamped from one shape.
            let spin = (hash_pick(id, 360) as f32).to_radians();
            // Neighbours in this pool's OWN local frame: undo the spin, then divide by the
            // prop's scale, so the mesh can ask "is this rim vertex inside another pool?"
            // in the units it is built in. `LOCAL_R` is the mean lobed outline, which is
            // close enough to decide coverage.
            // Drawn no wider than it blocks: `BLOB_MAX_RADIUS` is the outline's widest
            // lobe, so this puts that lobe exactly on the collision edge. Anything larger
            // leaves walkable ground inside visible water.
            let scale = r / hd2d::BLOB_MAX_RADIUS;
            let (cs, sn) = ((-spin).cos(), (-spin).sin());
            let near: Vec<(f32, f32, f32)> = water_bodies
                .iter()
                .filter(|(nx, ny, nr)| {
                    let d = (nx - e.x).hypot(ny - e.y);
                    d > 1e-4 && d < r + nr
                })
                .map(|(nx, ny, nr)| {
                    let (dx, dy) = ((nx - e.x) / scale, (ny - e.y) / scale);
                    (dx * cs - dy * sn, dx * sn + dy * cs, nr / scale)
                })
                .collect();
            let mesh = if near.is_empty() {
                wa.water_mesh.clone()
            } else {
                meshes.add(hd2d::blob_basin_mesh_merged(28, 0.16, 0.74, &near))
            };
            // Floating leaves. Still water reads as water because of what is ON it —
            // duckweed, pads, fallen leaves — and bog water has almost no value contrast
            // against the mire's own ground, so without them a merged mere is just a dark
            // patch of mud. Not on ice: nothing floats on a frozen pond.
            let pads: Vec<(f32, f32, f32, usize)> = if name == "frozen_pond" {
                Vec::new()
            } else {
                // Count rides the pool's size, so a mere is dressed and a puddle is not.
                let n = ((r * 1.5) as usize).clamp(1, 7);
                (0..n)
                    .map(|i| {
                        let key = format!("{id}-pad{i}");
                        let a = hash_pick(&key, 360) as f32;
                        // sqrt for a uniform draw over the AREA, or every pad crowds the rim.
                        let t = (hash_pick(&format!("{key}r"), 100) as f32 / 100.0).sqrt();
                        let rad = t * 0.58; // inside the flat surface, clear of the bank
                        let sz = 0.07 + hash_pick(&format!("{key}s"), 60) as f32 / 1000.0;
                        (
                            a.to_radians().cos() * rad,
                            a.to_radians().sin() * rad,
                            sz,
                            hash_pick(&format!("{key}c"), wa.pad_mats.len().max(1)),
                        )
                    })
                    .collect()
            };
            commands
                .spawn((
                    WorldEntity(id.to_string()),
                    Mesh3d(mesh),
                    MeshMaterial3d(wa.water_mat(name)),
                    Transform::from_translation(world_pos(e.x, e.y, 0.2))
                        .with_rotation(
                            Quat::from_rotation_y(spin)
                                * Quat::from_rotation_x(-std::f32::consts::FRAC_PI_2),
                        )
                        .with_scale(Vec3::splat(scale)),
                ))
                .with_children(|p| {
                    // Children live in the BASIN's own mesh space, so the parent's spin,
                    // flat-lay rotation and radius scale all apply for free — and the pad
                    // sits at the waterline (`-depth`) rather than the rim, a hair above
                    // the surface so it does not z-fight with it.
                    for (px, py, sz, ci) in pads {
                        if let Some(m) = wa.pad_mats.get(ci) {
                            p.spawn((
                                Mesh3d(wa.pad_mesh.clone()),
                                MeshMaterial3d(m.clone()),
                                Transform::from_xyz(px, py, -0.16 + 0.006)
                                    .with_scale(Vec3::splat(sz)),
                            ));
                        }
                    }
                });
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
    // Water asks the shared predicate rather than re-listing the kinds: this palette was
    // the third copy of that list, and the one most likely to be missed, since a new
    // water kind rendering in stone-grey looks like art rather than a bug.
    if meld_proto::coast::is_water_kind(kind) {
        return Color::srgb(0.22, 0.4, 0.6);
    }
    match kind {
        "tree" | "cactus" | "mire_root" | "fungal_wall" | "mire_tree" => {
            Color::srgb(0.18, 0.42, 0.22) // foliage
        }
        // The minimap dot follows the SEASON, not the species: an autumn wood reading
        // summer-green on the map would disagree with the ground it is drawn on.
        "amber_tree" => Color::srgb(0.55, 0.3, 0.12),
        "snow_tree" => Color::srgb(0.58, 0.68, 0.72),
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
    mut motion: MessageReader<MouseMotion>,
    mut wheel: MessageReader<MouseWheel>,
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
    /// A NIBBLE MUST NOT LOOK LIKE A CATASTROPHE. One wash served both a sprung trap and a
    /// venom bite — and venom bites once every few STEPS, so walking poisoned filled the
    /// screen red over and over at full strength. A warning that fires constantly at maximum
    /// volume stops being a warning.
    #[test]
    fn the_hurt_wash_is_proportionate_to_what_was_taken() {
        use super::wash_peak;
        let pool = 1000;
        let nibble = wash_peak(3, pool); // a venom step
        let blow = wash_peak(120, pool); // a trap
        let ruin = wash_peak(600, pool); // a Shift's Force blast
        assert!(nibble > 0.0, "a real hit must still register");
        assert!(nibble < blow, "a venom nibble washes as hard as a trap: {nibble} vs {blow}");
        assert!(blow < ruin || ruin >= 1.0, "a bigger blow must not wash softer");
        assert_eq!(ruin, 1.0, "past the threshold it is simply the full wash");
        // Healing, levelling and a joining hero all move the total; none is a hit.
        assert_eq!(wash_peak(0, pool), 0.0);
        assert_eq!(wash_peak(-50, pool), 0.0);
        // A pool that has not arrived yet cannot be divided by.
        assert_eq!(wash_peak(10, 0), 0.0);
    }


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

    /// **PROPS MUST BE DRAWN AS FAR AS THE WORLD IS SHARP.** The render-unload radii and the
    /// fog distance are one question answered twice, and they drifted: the radii were set
    /// when `fog_end` was ~118, the fog went out to 700, and nothing moved them — so a wood
    /// stopped having trees in it past 150 units while the ground stayed visible to 700.
    /// This is the relationship, so the next person to move the fog cannot leave the props
    /// behind.
    #[test]
    fn props_are_drawn_as_far_as_the_world_is_sharp() {
        let look = hd2d::Look::default();
        assert!(
            RENDER_UNLOAD_NEAR >= look.fog_start,
            "props stop at {RENDER_UNLOAD_NEAR} but the world is still sharp out to {} — that \
             band renders as bare terrain, which is what made a forest read as a field",
            look.fog_start
        );
        // And the fog must actually reach past where we stop drawing, or the cull is a hole
        // in plain sight rather than something the haze is covering.
        assert!(
            look.fog_end > RENDER_UNLOAD_FAR,
            "fog_end {} must be beyond the cull at {RENDER_UNLOAD_FAR}, or entities pop out \
             of existence against fully-visible ground",
            look.fog_end
        );
    }

    /// **A PLATE MUST NOT OUTLIVE THE CREATURE IT NAMES.** The behind-camera and
    /// inside-the-viewport guards both pass for a mob well past the hill crest, whose plate
    /// then hangs in empty sky and — being distant — barely moves, which is how a clump of
    /// them ended up parked in a screen corner for a whole session.
    #[test]
    fn a_plate_stops_where_the_creature_does() {
        // Close in: named.
        assert!(plate_is_close_enough_to_see(10.0, true, 320.0));
        // A near fog still wins where it is the tighter of the two.
        assert!(plate_is_close_enough_to_see(200.0, true, 200.0), "the boundary is inclusive");
        assert!(!plate_is_close_enough_to_see(200.1, true, 200.0));
        // ⚠️ AND THE RENDER CULL BINDS TOO, which is what this test missed the first time:
        // with `fog_end` out past `RENDER_UNLOAD_FAR`, gating on the fog alone kept a label
        // for a creature whose sprite had been despawned — a bar hanging in empty sky.
        assert!(
            !plate_is_close_enough_to_see(RENDER_UNLOAD_FAR + 0.1, true, 4_000.0),
            "a plate must not outlive the sprite it names, however far the fog reaches"
        );
        // …and look-dev's fog toggle cannot opt out of it: the body is still culled.
        assert!(!plate_is_close_enough_to_see(4_000.0, false, 320.0));
        assert!(plate_is_close_enough_to_see(RENDER_UNLOAD_FAR - 1.0, false, 320.0));
    }

    fn ent(kind: EntityKind, x: f32, y: f32) -> OwEntity {
        OwEntity {
            x,
            y,
            kind,
            name: Some("bloom_herb".into()),
            faction: None,
            radius: 0.0,
            battling: false,
            clashing: false,
            level: 0,
            opened: false,
            chest_tier: 0,
            mob_level: None,
            hp: None,
            max_hp: None,
            encounter_class: None,
            aggression: None,
            quarry: false,
            held: false,
            boss: None,
            bodies_required: 1,
            expects_parties: 0,
        }
    }

    /// A CLASH, a QUARRY and a PIN are events the world is reporting, not intel a perk
    /// buys — so an ungeared party still sees them. Gate any of the three on a perk and
    /// the failure is silent: a brawl goes on beside you with nothing on screen to say so,
    /// and you never learn that waiting it out leaves loot on the ground.
    #[test]
    fn an_event_over_a_creatures_head_is_never_gated_on_a_perk() {
        let mut world = Overworld::default();
        let plain = ent(EntityKind::Monster, 1.0, 0.0);
        world.entities.insert("boar".into(), plain);
        assert!(!nameplates_wanted(0, 0, &world), "an ordinary creature drew a plate");
        // The Hunter's own readouts still turn it on, as before.
        assert!(nameplates_wanted(1, 0, &world));
        assert!(nameplates_wanted(0, 1, &world));

        for mark in ["clash", "quarry", "held", "wound", "boss"] {
            let mut world = Overworld::default();
            let mut e = ent(EntityKind::Monster, 1.0, 0.0);
            match mark {
                "clash" => e.clashing = true,
                "quarry" => e.quarry = true,
                "wound" => {
                    e.hp = Some(30);
                    e.max_hp = Some(100);
                }
                // FS-4: a named boss is a fourth fact the world reports. A Gatekeeper
                // stands in every pass and overlays a host creature, so nothing about
                // its billboard says which of the ten it is — and it is exactly the
                // creature a player must identify BEFORE walking into it.
                "boss" => e.boss = Some("ironmaw".into()),
                _ => e.held = true,
            }
            world.entities.insert("boar".into(), e);
            assert!(
                nameplates_wanted(0, 0, &world),
                "`{mark}` was swallowed by the perk early-out"
            );
        }
    }

    /// A plate is asked for only when there is a TITLE to put on it. A dungeon's
    /// authored boss sprite need not be one of the ten (`twingolem` is bespoke art), and
    /// a plate reading a guess over ordinary scenery is worse than no plate — while a
    /// creature carrying a real boss key must never be silent, since that is the whole
    /// point of the token.
    #[test]
    fn only_a_boss_the_registry_can_name_asks_for_a_plate() {
        let mut e = ent(EntityKind::Monster, 1.0, 0.0);
        assert!(!named_boss(&e), "ordinary fauna claimed a boss title");
        e.boss = Some("twingolem".into());
        assert!(!named_boss(&e), "bespoke dungeon art was titled a named boss");
        for key in meld_proto::bosses::keys() {
            e.boss = Some(key.to_string());
            assert!(named_boss(&e), "{key} rides the wire with no title to draw");
        }
    }

    /// CR-2: a creature's HP persists and mends only slowly, so a hurt one is a
    /// time-bound opportunity — worth nothing if it looks identical to a healthy one.
    /// A full-health creature is NOT wounded, which is what keeps the Hunter's
    /// `intel >= 2` bar worth having on the 95% case.
    #[test]
    fn a_wound_is_read_from_the_health_the_snapshot_carries() {
        let mut e = ent(EntityKind::Monster, 1.0, 0.0);
        assert!(!wounded(&e), "a creature with no health on the wire read as hurt");
        e.hp = Some(100);
        e.max_hp = Some(100);
        assert!(!wounded(&e), "an untouched creature read as hurt");
        e.hp = Some(99);
        assert!(wounded(&e), "a single point of damage is still a wound");
        // Degenerate wire values must not invent a wound out of a division by zero.
        e.hp = Some(0);
        e.max_hp = Some(0);
        assert!(!wounded(&e));
    }

    /// SOC-3/CR-2: watching is offered for BOTH kinds of fight, from further away than
    /// joining, and the nearest one wins — which matters because the two read completely
    /// differently to a player. Nothing nearby offers nothing at all, so the plate stays
    /// clean (the [E]-only rule).
    #[test]
    fn watching_is_offered_for_either_kind_of_fight_nearest_first() {
        let mut world = Overworld::default();
        let me = Some((0.0, 0.0));
        world.entities.insert("me".into(), ent(EntityKind::Player, 0.0, 0.0));
        assert_eq!(watchable(&world, me), None, "an empty field offered a fight to watch");

        // A teammate's battle, well past join range but inside watching range.
        let mut fighter = ent(EntityKind::Player, JOIN_PROMPT_RADIUS + 3.0, 0.0);
        fighter.battling = true;
        world.entities.insert("ally".into(), fighter);
        assert_eq!(
            watchable(&world, me),
            Some("Watch the fight"),
            "you cannot watch a fight you can already see"
        );

        // A creature clash nearer than the battle takes the prompt — the same resolution
        // the server does, so the prompt never names a thing the server would not pick.
        let mut brawler = ent(EntityKind::Monster, 2.0, 0.0);
        brawler.clashing = true;
        world.entities.insert("boar".into(), brawler);
        assert_eq!(watchable(&world, me), Some("Watch the clash"));

        // Beyond watching range, neither is on offer.
        world.entities.clear();
        let mut far = ent(EntityKind::Monster, WATCH_PROMPT_RADIUS + 5.0, 0.0);
        far.clashing = true;
        world.entities.insert("boar".into(), far);
        assert_eq!(watchable(&world, me), None, "a clash across the map was offered");
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

    // The mask spawns on every entry to the overworld, and the panels are opaque. Walking
    // out of four battles used to leave twenty panels stacked, all of which
    // `update_blind_mask` shows — so a blinded party got strictly darker the longer the
    // session ran, and the entity count never came back down.
    #[test]
    fn returning_to_the_overworld_does_not_stack_a_second_blackout() {
        let mut app = App::new();
        app.add_systems(Update, spawn_blind_mask);
        for _ in 0..3 {
            app.update();
        }
        assert_eq!(
            app.world_mut().query::<&BlindMask>().iter(app.world()).count(),
            4,
            "one blackout, four panels - re-entering the overworld must not add more"
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
            clashing: false,
            level: 0,
            opened: false,
            chest_tier: 0,
            mob_level: None,
            hp: None,
            max_hp: None,
            encounter_class: None,
            aggression: None,
            quarry: false,
            held: false,
            boss: None,
            bodies_required: 1,
            expects_parties: 0,
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
            clashing: false,
            level: 0,
            opened: false,
            chest_tier: 0,
            mob_level: None,
            hp: None,
            max_hp: None,
            encounter_class: None,
            aggression: None,
            quarry: false,
            held: false,
            boss: None,
            bodies_required: 1,
            expects_parties: 0,
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
mod sprite_mat_tests {
    use super::*;

    /// **A FOREST HAS TO BATCH, AND A PER-INSTANCE MATERIAL IS WHAT STOPS IT.**
    ///
    /// The quad mesh was already shared, so obstacle billboards looked batched — but every
    /// one allocated its own `StandardMaterial`, and to Bevy two byte-identical materials are
    /// two assets, so each prop was its own draw call. Measured at the hub, seed 424242:
    /// **2,139 obstacles → 2,139 materials**, for what turns out to be **nine** distinct
    /// sprites. With the cache the same scene reports `standard_materials` 449 total and
    /// **9 shared sprite materials** — the forest collapses to nine batches.
    ///
    /// It is worth a test rather than a comment because the failure is invisible: the world
    /// looks identical either way and only the frame time knows. Anything that goes back to
    /// minting a material per prop fails here.
    #[test]
    fn one_material_per_sprite_not_one_per_prop() {
        let mut mats: Assets<StandardMaterial> = Assets::default();
        let mut imgs: Assets<Image> = Assets::default();
        let mut cache = SpriteMats::default();
        let blank = || Image::new_fill(
            bevy::render::render_resource::Extent3d { width: 1, height: 1, depth_or_array_layers: 1 },
            bevy::render::render_resource::TextureDimension::D2,
            &[255, 255, 255, 255],
            bevy::render::render_resource::TextureFormat::Rgba8UnormSrgb,
            bevy::asset::RenderAssetUsages::default(),
        );
        let oak = imgs.add(blank());
        let pine = imgs.add(blank());

        // A thousand oaks are ONE material…
        let first = cache.get(&mut mats, Color::WHITE, oak.clone());
        for _ in 0..1000 {
            assert_eq!(
                cache.get(&mut mats, Color::WHITE, oak.clone()),
                first,
                "every prop of one sprite must share a material, or the wood cannot batch"
            );
        }
        assert_eq!(cache.len(), 1, "a thousand oaks minted {} materials", cache.len());

        // …a different SPRITE is its own, or every tree would wear the same bark.
        assert_ne!(cache.get(&mut mats, Color::WHITE, pine.clone()), first);
        // …and so is a different TINT, since the tint is the material's other input.
        assert_ne!(cache.get(&mut mats, Color::srgb(1.0, 0.5, 0.5), oak), first);
        assert_eq!(cache.len(), 3, "sprite and tint both have to key the cache");
        assert_eq!(mats.len(), 3, "the cache minted more assets than it keys");
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
        Some(Interact::MendStructure { entity_id, .. }) => {
            net.send(ClientCmd::RepairStructure { entity_id })
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

/// The [V] chip: watch the fight in reach without joining it (`SOC-3`).
#[derive(Component)]
pub(crate) struct ActionHudWatchTap;

/// Tapping the [V] chip watches the nearest fight — the touch twin of the key.
pub(crate) fn action_hud_watch_tap(
    q: Query<&Interaction, (Changed<Interaction>, With<ActionHudWatchTap>)>,
    net: NonSend<NetRes>,
) {
    for interaction in &q {
        if *interaction == Interaction::Pressed {
            net.0.send(ClientCmd::WatchBattle);
        }
    }
}

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
    cam_q: WorldCamera,
    root_q: Query<Entity, With<NameplateRoot>>,
    players: Query<(&WorldEntity, &GlobalTransform)>,
    old: Query<Entity, With<ActionHud>>,
    wa: Option<Res<WorldAssets>>,
    tutorial_run: Res<TutorialRun>,
    roster: Res<PartyRoster>,
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
    // The guided [T]-dive walkthrough coaches by brightening this exact prompt
    // chip in place (no overlay box) when it matches the step still owed.
    let highlight = match (tutorial_run.step, &target) {
        (Some(TutorialStep::Harvest), Some(Interact::Harvest { .. })) => !tutorial_run.harvested,
        (Some(TutorialStep::Harvest), Some(Interact::OpenChest { .. })) => {
            tutorial_run.harvested && !tutorial_run.chest_opened
        }
        (Some(TutorialStep::Dungeon), Some(Interact::EnterDungeon { .. })) => true,
        _ => false,
    };
    let boon = boon_offer(&world, &session);
    let me_pos = world.entities.get(&session.player_id).map(|e| (e.x, e.y));
    let watch = watchable(&world, me_pos);
    // Who is hurt, and how. An affliction does NOT wear off (`meld_proto::statuses`), so a
    // hero who caught one in a fight carries it down the road — and out here nothing said
    // so: the venom biting per step and the bindings dragging the march were both invisible
    // until the next battle screen. Named per hero, because the party travels as one avatar
    // and "someone is poisoned" is not actionable.
    let conditions: Vec<String> = roster
        .heroes
        .iter()
        .filter_map(|h| {
            let state = h.condition_label();
            (!state.is_empty()).then(|| format!("{}: {state}", h.name))
        })
        .collect();
    if target.is_none()
        && boon.is_none()
        && watch.is_none()
        && !session.channeling
        && pops.items.is_empty()
        && conditions.is_empty()
    {
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
                // The thing itself, then how many of it: a shrunk copy of the node's own
                // sprite is the same picture as the bush you are standing at, so the payout
                // is recognisable before the word is read.
                col.spawn(Node {
                    flex_direction: FlexDirection::Row,
                    align_items: AlignItems::Center,
                    column_gap: Val::Px(5.0),
                    ..default()
                })
                .with_children(|row| {
                    crate::icons::spawn_icon(row, wa.as_deref(), &pop.kind, 20.0);
                    row.spawn((
                        Text::new(pop.label()),
                        TextFont { font_size: FontSize::Px(17.0), ..default() },
                        TextColor(Color::srgba(0.62, 0.98, 0.7, a)),
                    ));
                });
            }
            let line = if session.channeling {
                Some("[E] stop".to_string())
            } else {
                target.as_ref().map(|t| t.prompt())
            };
            let boon_line = boon.as_ref().map(|(_, _, what)| format!("[N] {what}"));
            // Watching stays on offer even mid-channel: reading the fight over there is
            // exactly what you might want to do while you finish gathering.
            let watch_line = watch.map(|what| format!("\u{f0817} [V] {what}"));
            if line.is_none()
                && boon_line.is_none()
                && watch_line.is_none()
                && !session.channeling
                && conditions.is_empty()
            {
                return;
            }
            // One frosted plate holding the prompt and the bar, mostly see-through so it
            // never hides the character it belongs to.
            col.spawn((
                Node {
                    border_radius: BorderRadius::all(Val::Px(7.0)),
                    flex_direction: FlexDirection::Column,
                    align_items: AlignItems::Center,
                    row_gap: Val::Px(4.0),
                    padding: UiRect::axes(Val::Px(10.0), Val::Px(5.0)),
                    border: UiRect::all(Val::Px(1.0)),
                    ..default()
                },
                BackgroundColor(glass::GLASS_THIN),
                BorderColor::all(glass::EDGE_SOFT),
            ))
            .with_children(|plate| {
                // Conditions first — above the prompts, because what is wrong with the party
                // outranks what there is to press. NOT a chip: there is nothing to tap, and a
                // chip is this UI's promise that something happens if you do.
                for text in &conditions {
                    plate.spawn(glass::text(text.clone(), 15.0, glass::WARN));
                }
                // Each prompt is its own chip, so touch has a target per action instead of
                // one button in the corner that had to guess which you meant.
                if let Some(text) = line {
                    plate
                        .spawn((Button, ActionHudTap, glass::chip(highlight)))
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
                if let Some(text) = watch_line {
                    plate
                        .spawn((Button, ActionHudWatchTap, glass::chip(false)))
                        .with_children(|b| {
                            b.spawn(glass::text(text, 15.0, glass::DIM));
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
                            BorderColor::all(glass::EDGE_SOFT),
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

/// Full-swell brightness of the light the reach rim throws (lumens, scaled by the breathe).
const REACH_LAMP_LUMENS: f32 = 26_000.0;

/// Make the thing you could interact with glow, on a slow pulse — whatever it is made of.
///
/// Two jobs in one affordance. It says "this one is in reach" — nothing used to distinguish
/// the node you can actually gather from one three steps behind it — and it draws the eye
/// without erasing the art, which is where the old whole-sprite emissive pulse went wrong:
/// emissive is added flat across a textured quad, so it painted the sprite out.
///
/// TWO parts, because the world is not all billboards. A **pool of light on the ground**
/// under the target works for anything: a 3D prop model has no sprite to copy, and a bare
/// mesh has no children at all, so a sprite-copy rim left every ore vein and boulder in the
/// game with no affordance whatsoever — which is exactly how it was reported ("these don't
/// glow at all"). On top of that, where there IS a sprite, a copy of it a little larger and
/// drawn behind reads as a rim, the cheap 2D outline trick that needs no shader. Both breathe
/// together, and both throw light so the ground answers.
pub(crate) fn update_reach_halo(
    mut commands: Commands,
    world: Res<Overworld>,
    session: Res<Session>,
    time: Res<Time>,
    mut mats: ResMut<Assets<StandardMaterial>>,
    wa: Option<Res<WorldAssets>>,
    targets: Query<(Entity, &WorldEntity, Option<&Children>)>,
    sprite_of: Query<&MeshMaterial3d<StandardMaterial>, Without<ReachHalo>>,
    mut halos: Query<
        (
            Entity,
            &ChildOf,
            &MeshMaterial3d<StandardMaterial>,
            Option<&mut PointLight>,
        ),
        With<ReachHalo>,
    >,
) {
    let want = interact_target(&world, &session)
        .as_ref()
        .and_then(|t| t.entity_id().map(String::from));

    // A slow breathe, ~3s, and it never fully lets go: the squared curve on a 0.12 floor
    // was subtle enough to miss entirely, so the rim now holds a third of its brightness
    // between breaths and the swell is linear rather than squared. Still slow, so it reads
    // as something the object is doing rather than a blinking UI element.
    let phase = (time.elapsed_secs() * std::f32::consts::TAU / 3.0).sin().max(0.0);
    let alpha = 0.34 + 0.66 * phase;

    let Some(id) = want else {
        // Nothing in reach: clear any halo still standing.
        for (e, _, _, _) in &halos {
            commands.entity(e).despawn();
        }
        return;
    };
    let Some(wa) = wa else { return };
    let Some((root, _, kids)) = targets.iter().find(|(_, we, _)| we.0 == id) else {
        for (e, _, _, _) in &halos {
            commands.entity(e).despawn();
        }
        return;
    };

    // Already lit? Just breathe it — every part's alpha and the light each throws.
    let mut found = false;
    for (e, parent, mm, light) in &mut halos {
        if parent.parent() == root {
            found = true;
            if let Some(mut m) = mats.get_mut(&mm.0) {
                m.base_color = m.base_color.with_alpha(alpha);
            }
            if let Some(mut light) = light {
                light.intensity = REACH_LAMP_LUMENS * alpha;
            }
        } else {
            commands.entity(e).despawn();
        }
    }
    if found {
        return;
    }

    let glow = |mats: &mut Assets<StandardMaterial>, tex: Option<Handle<Image>>| {
        mats.add(StandardMaterial {
            base_color: Color::srgba(1.0, 0.86, 0.45, alpha),
            base_color_texture: tex,
            // Unlit and alpha-blended: this is a light, not a surface. Depth write OFF so it
            // never punches a hole in the thing it sits behind.
            unlit: true,
            alpha_mode: AlphaMode::Blend,
            depth_bias: -1.0,
            double_sided: true,
            cull_mode: None,
            ..default()
        })
    };
    let pool = glow(&mut mats, None);
    let rim_tex = kids
        .into_iter()
        .flatten()
        .filter_map(|k| sprite_of.get(*k).ok())
        .filter_map(|mm| mats.get(&mm.0).and_then(|m| m.base_color_texture.clone()))
        .next();
    let rim = rim_tex.map(|tex| glow(&mut mats, Some(tex)));
    commands.entity(root).with_children(|p| {
        // The ground pool, and the light it stands for. Laid flat and just clear of the
        // ground so it is a glow ON the terrain rather than a disc floating over it.
        p.spawn((
            ReachHalo,
            Mesh3d(wa.shadow_mesh.clone()),
            MeshMaterial3d(pool),
            Transform::from_xyz(0.0, 0.06, 0.0)
                .with_rotation(Quat::from_rotation_x(-std::f32::consts::FRAC_PI_2))
                .with_scale(Vec3::new(1.5, 1.5, 1.5)),
            PointLight {
                color: Color::srgb(1.0, 0.86, 0.45),
                intensity: 0.0,
                range: 6.5,
                radius: 0.3,
                shadow_maps_enabled: false,
                ..default()
            },
        ));
        if let Some(rim) = rim {
            p.spawn((
                ReachHalo,
                Mesh3d(wa.sprite_quad.clone()),
                MeshMaterial3d(rim),
                // A touch larger and a hair behind, so what shows is a rim around the sprite.
                Transform::from_xyz(0.0, 0.85, -0.02).with_scale(Vec3::splat(1.7 / 2.2 * 1.20)),
                hd2d::Billboard,
            ));
        }
    });
}
