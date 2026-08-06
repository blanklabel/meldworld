//! meld-client - the Bevy client for MELDWORLD's core gameplay loop
//! (BUILD-PLAN T4 overworld + T5 UI; CANON D16 all-Bevy). Server-authoritative:
//! the client sends intents and renders whatever the server reports (CANON §S).
//!
//! Loop: Join → City (The Last City hub) → Overworld (walk into the monster) →
//! Battle (ATB) → Ended → back to City. The city is the persistent home the
//! extract-or-die loop returns to (see docs/proposals/last-city.md).
//!
//! Config: `MELD_SERVER` (default `http://127.0.0.1:8080`) and `MELD_NAME`
//! (default a random guest name).

use std::collections::{HashMap, HashSet};

use bevy::prelude::*;

use bevy::image::{ImageAddressMode, ImageLoaderSettings, ImageSampler, ImageSamplerDescriptor};
// `Affine2` (was: ground uv_transform) is now referenced fully-qualified in apply_sky.

use meld_client::hd2d::{self};
use meld_client::net;
use net::{ClientCmd, CombatantView, EntityKind, GearLine, Net, SkillLine};

// Feature modules. This root file owns the App wiring, the shared domain model
// (screen state, session, world/battle/overlay data — kept here so every module
// can reach it), and a handful of cross-cutting helpers; each module below owns
// one slice of behavior. They re-export into the crate root so a system in one
// module can call a sibling's via `use super::*`.
mod ambient; // client-side decorative life: world-snapped grass scatter + biome motes
mod battle; // ATB command panel, party HUD, 3D arena + camera, per-class kits
mod city; // The Last City hub: districts, plaza, HUD
mod flags; // launch-time `MELD_*` / `?query` toggles
mod mocks; // offline screenshot/demo seeds
mod music; // one looping background track per screen (assets/music/*.mp3)
mod netglue; // server messages → state, demo driver, despawn + font install
mod overlays; // inventory/equip/status, gear tooltip, loot report, level-up
mod overworld; // movement/camera, sprite reconciler, terrain, followers, minimap
mod screens; // Join, co-op Lobby, Ended summary
mod world_render; // asset load + scene setup, biome ground, sky/weather/water
pub(crate) use battle::*;
pub(crate) use city::*;
pub(crate) use flags::*;
pub(crate) use mocks::*;
pub(crate) use netglue::*;
pub(crate) use overlays::*;
pub(crate) use overworld::*;
pub(crate) use screens::*;
pub(crate) use world_render::*;

/// MoveIntents are emitted at this fixed rate (Hz). The server advances the
/// avatar by `avatar_speed / overworld_sim_hz` tiles per intent, so pacing
/// intents at `overworld_sim_hz` yields the configured tiles/sec at ANY render
/// frame rate. Sending one intent per frame instead would make walk speed scale
/// with FPS (crawl in a throttled tab, rocket at 120fps). Keep in sync with
/// `[world] overworld_sim_hz` in balance.toml.
const MOVE_INTENT_HZ: f32 = 20.0;


/// Raise the process's open-file limit as high as the OS allows. Bevy's asset
/// server opens many files at once loading the ~84 MB of art; a process launched
/// with a low soft `RLIMIT_NOFILE` (256 on a GUI/launchd-started macOS app) runs
/// out of descriptors and loads fail with "Too many open files (os error 24)" —
/// which silently drops sprite atlases and GLB models (missing creatures, a
/// wrecked/absent castle, ground decals). Harmless no-op when the limit is already
/// high. No-op on wasm (no such limit in the browser).
#[cfg(not(target_arch = "wasm32"))]
fn raise_open_file_limit() {
    let _ = rlimit::increase_nofile_limit(u64::MAX);
}
#[cfg(target_arch = "wasm32")]
fn raise_open_file_limit() {}

/// The window mode at launch: borderless-fullscreen on the native desktop (big +
/// readable), plain windowed in the browser (the canvas fills its parent instead).
#[cfg(not(target_arch = "wasm32"))]
fn default_window_mode() -> bevy::window::WindowMode {
    bevy::window::WindowMode::BorderlessFullscreen(bevy::window::MonitorSelection::Current)
}
#[cfg(target_arch = "wasm32")]
fn default_window_mode() -> bevy::window::WindowMode {
    bevy::window::WindowMode::Windowed
}

fn main() {
    raise_open_file_limit();
    // Self-contained build: boot the server in-process (in-memory DB, embedded
    // balance) and set MELD_SERVER before we read it below. No-op in normal builds.
    #[cfg(feature = "embedded-server")]
    meld_client::embedded::boot();

    let base = server_base();
    let mut app = App::new();
    // Serve every game asset from inside the binary (no `assets/` folder beside it,
    // and no file-descriptor storm from loading thousands of loose files). Gated on
    // `embedded-assets` — on for `make play`/`play-solo`/`dist`, OFF for `make
    // play-dev` (which hot-reloads loose files). Must precede DefaultPlugins.
    #[cfg(feature = "embedded-assets")]
    app.add_plugins(bevy_embedded_assets::EmbeddedAssetPlugin {
        mode: bevy_embedded_assets::PluginMode::ReplaceDefault,
    });
    app
        .add_plugins(
            DefaultPlugins
                .set(ImagePlugin::default_nearest()) // crisp pixel sprites
                .set(WindowPlugin {
                    primary_window: Some(Window {
                        title: "MELDWORLD".to_string(),
                        // Open BIG: borderless-fullscreen on the native desktop so the
                        // world + sprites are readable; the resolution is the windowed
                        // fallback (and what the wasm canvas uses).
                        resolution: (1280.0_f32, 800.0_f32).into(),
                        mode: default_window_mode(),
                        // Browser (wasm): bind to <canvas id="bevy"> and fill its parent.
                        canvas: Some("#bevy".to_string()),
                        fit_canvas_to_parent: true,
                        ..default()
                    }),
                    ..default()
                }),
        )
        .init_state::<Screen>()
        // The biome-blending ground material (see `GroundBiome`).
        .add_plugins(MaterialPlugin::<GroundMat>::default())
        // Daytime sky blue behind the diorama (the fog fades the ground into it).
        .insert_resource(ClearColor(Color::srgb(0.53, 0.72, 0.93)))
        .insert_resource(hd2d::ambient_light())
        .init_resource::<hd2d::Look>()
        .init_resource::<hd2d::LookWatch>()
        .init_resource::<overworld::CamLift>()
        .insert_non_send_resource(NetRes(net::start(base)))
        // Demo and autoplay are mutually exclusive; demo skips networking.
        // `?city` connects via the autoplay path but parks in the hub (see CityIdle).
        .insert_resource(Autoplay((autoplay_flag() || city_idle_flag()) && !demo_flag()))
        .init_resource::<Tactics>()
        .insert_resource(CityIdle(city_idle_flag()))
        .insert_resource(Demo {
            on: demo_flag(),
            t: 0.0,
            started: false,
        })
        .init_resource::<Session>()
        .init_resource::<Sky>()
        .init_resource::<Ashfall>()
        .init_resource::<DungeonSceneRes>()
        .init_resource::<MoveClock>()
        .init_resource::<JoinFocus>()
        .init_resource::<LoginFocus>()
        .init_resource::<BattleMenu>()
        .init_resource::<BattleCam>()
        .init_resource::<PartyView>()
        .init_resource::<HitFx>()
        .init_resource::<AtbFlash>()
        .init_resource::<AllyPanel>()
        .init_resource::<Overlay>()
        .init_resource::<OwInterp>()
        .init_resource::<OverlayTab>()
        .init_resource::<EquipSelection>()
        .init_resource::<EquipPicker>()
        .init_resource::<OverlayCursor>()
        .init_resource::<InventoryData>()
        .init_resource::<RunGearData>()
        .init_resource::<ProgressData>()
        .init_resource::<AccountHeroNames>()
        .init_resource::<VanguardBoardData>()
        .init_resource::<ShopData>()
        .init_resource::<Overworld>()
        .init_resource::<RunBackpack>()
        .init_resource::<RunStats>()
        .init_resource::<WorldPath>()
        .init_resource::<WorldWeb>()
        .init_resource::<Terrain>()
        .init_resource::<PartyRoster>()
        .init_resource::<PerksRes>()
        .init_resource::<LevelUpQueue>()
        .init_resource::<UnlocksRes>()
        .init_resource::<WorldFrame>()
        .init_resource::<HeroRename>()
        .init_resource::<Steer>()
        .init_resource::<TapTarget>()
        .init_resource::<Joystick>()
        .init_resource::<BattleData>()
        .init_resource::<BattleTarget>()
        .init_resource::<EndInfo>()
        .init_resource::<CityUi>()
        .init_resource::<LobbyData>()
        .init_resource::<LootReport>()
        .add_systems(
            Startup,
            (setup, load_ui_font, apply_class_flag, mock_battle_setup, mock_overlay_setup, ambient::setup_ambient, music::setup_music),
        )
        // run in every state: net pump, demo autopilot, the HD-2D file channel
        // (hot-reload look params + honour screenshot requests), cloud drift, and
        // the day/night + weather sky.
        .add_systems(
            Update,
            (
                pump_net,
                demo_driver,
                hd2d_remote,
                hd2d::no_billboard_shadows,
                drift_clouds,
                tile_ground_detail,
                follow_world_ground,
                update_ground_biome_rings,
                drift_motes,
                drive_ashfall,
                anchor_backdrop,
                advance_sky,
                apply_sky,
                anchor_sky_fx,
                drive_rain,
                animate_water,
                // Background music: swap the looping track when the screen changes.
                music::update_music,
                // Player characters carry their own light at night (overworld +
                // battle) so the game stays readable in the dark.
                illuminate_players,
                // Route every UI text through the bundled symbol-capable font.
                apply_ui_font,
            ),
        )
        // Join
        .add_systems(OnEnter(Screen::Join), join_ui)
        .add_systems(OnExit(Screen::Join), despawn::<JoinRoot>)
        .add_systems(
            Update,
            (join_input, join_interact, join_refresh, join_login_refresh)
                .run_if(in_state(Screen::Join)),
        )
        // City — The Last City (persistent hub): a walkable HD-2D plaza built from Kenney
        // CC0 kits, reusing the overworld camera/avatar/animation machinery.
        // Each state ENTRY purges the actor kinds that don't belong to it, so a sprite
        // can never stick across a transition: `WorldEntity` lives only in Overworld,
        // `BattleActor` only in Battle, `CityScene` only in City. (OnExit handlers do
        // this too, but a deferred spawn on the transition frame can slip past them —
        // enforcing it on entry as well makes a stuck/double sprite impossible.)
        .add_systems(
            OnEnter(Screen::City),
            (
                city_hud,
                city_scene,
                despawn::<BattleActor>,
                despawn::<WorldEntity>,
                despawn::<PartyFollower>,
            ),
        )
        .add_systems(
            OnExit(Screen::City),
            (despawn::<CityRoot>, despawn::<CityScene>),
        )
        .add_systems(
            Update,
            (
                city_move,
                city_interact,
                city_camera,
                city_input,
                city_action_buttons,
                render_city,
                pulse_magitech,
                hd2d::animate_chars,
                hd2d::place_billboards,
                hd2d::billboard,
            )
                .run_if(in_state(Screen::City)),
        )
        // Lobby (co-op)
        .add_systems(OnEnter(Screen::Lobby), lobby_ui)
        .add_systems(OnExit(Screen::Lobby), despawn::<LobbyRoot>)
        .add_systems(
            Update,
            (lobby_input, lobby_buttons, render_lobby).run_if(in_state(Screen::Lobby)),
        )
        // Overworld
        //
        // Start every overworld visit from a clean slate: purge `BattleActor`,
        // `CityScene`, AND `WorldEntity` on entry, then let `sync_overworld_sprites`
        // rebuild the avatars from the live snapshot on the same frame. This is the
        // definitive fix for the "double sprite" — a second avatar for the local hero
        // that stops receiving position/facing updates and stands frozen facing south
        // (a fresh `CharSprite`'s default). It appeared after a battle/zone change when
        // a stale avatar (or a `sync_battle_actors` spawn racing past `OnExit(Battle)`)
        // survived the transition and overlapped the live one. Rebuilding on entry
        // guarantees exactly one avatar per entity regardless of any transition race.
        .add_systems(
            OnEnter(Screen::Overworld),
            (
                overworld_ui,
                despawn::<BattleActor>,
                despawn::<CityScene>,
                despawn::<WorldEntity>,
                despawn::<PartyFollower>,
                // Re-entering the overworld (e.g. after a dungeon boss battle) wipes
                // decor via OnExit; if we're still inside a dungeon the server won't
                // resend the (unchanged) scene, so force the enclosure to rebuild.
                |mut s: ResMut<world_render::DungeonSceneRes>| s.dirty = true,
            ),
        )
        .add_systems(
            OnExit(Screen::Overworld),
            (
                despawn::<OverworldRoot>,
                despawn::<OverlayRoot>,
                despawn::<WorldEntity>,
                despawn::<PartyFollower>,
                despawn::<PathTrail>,
                despawn::<TerrainMesh>,
                despawn::<WorldWall>,
                despawn::<world_render::DungeonDecor>,
                despawn::<ChestEntity>,
                despawn::<LootReportRoot>,
            ),
        )
        .add_systems(
            Update,
            (
                overlay_input,
                overworld_input,
                auto_harvest,
                overworld_click_menu,
                overworld_camera_control,
                gather_steer,
                emit_move,
                joystick_visual,
                touch_action_buttons,
                sync_overworld_sprites,
                // Dotted trail overlays retired — the terrain itself will convey routes
                // once the continuous heightmap lands (natural valleys/ridges, DQ3-style).
                // (draw_path_trail, draw_web_trail)
                build_terrain_sections,
                world_render::manage_dungeon_scene,
                hd2d::animate_chars,
                hd2d_follow,
                hd2d::place_billboards,
                hd2d::billboard,
                animate_sway,
                ambient::update_ambient_scatter,
                (update_overworld_hud, update_run_stats),
                render_overlay,
            )
                .run_if(in_state(Screen::Overworld)),
        )
        .add_systems(
            Update,
            (
                gear_click,
                formation_click,
                overlay_tab_click,
                equip_hero_switch_click,
                overlay_nav_input,
                withdraw_click,
                render_loot_report,
                level_up_screen,
                unlock_banner,
                build_world_walls,
                sync_chests,
                auto_open_chest,
                pulse_collectibles,
                // Overworld class perks ("party sense").
                update_explorer_lamp,
                update_mob_nameplates,
                update_minimap,
            )
                .run_if(in_state(Screen::Overworld)),
        )
        // Equip tab picker screen — a third group (rather than growing the
        // tuple above further) to stay within Bevy's per-call system-tuple
        // arity limit.
        .add_systems(
            Update,
            (
                category_button_click,
                picker_unequip_click,
                picker_back_click,
                render_gear_tooltip,
            )
                .run_if(in_state(Screen::Overworld)),
        )
        // Optional "show the whole party" entourage that trails the lead avatar.
        .add_systems(
            Update,
            (toggle_party_view, sync_party_followers, cull_stray_avatars)
                .run_if(in_state(Screen::Overworld)),
        )
        // The storage chest (Vault-Deep) reuses the same tabbed inventory
        // overlay as the Overworld — City and Overworld are mutually exclusive
        // states, so registering these systems again here is safe (never both
        // active at once).
        .add_systems(
            Update,
            (
                overlay_input,
                render_overlay,
                gear_click,
                overlay_tab_click,
                equip_hero_switch_click,
                overlay_nav_input,
                withdraw_click,
                category_button_click,
                picker_unequip_click,
                picker_back_click,
                render_gear_tooltip,
            )
                .run_if(in_state(Screen::City)),
        )
        // Battle
        .add_systems(
            OnEnter(Screen::Battle),
            (clear_overworld_sprites, despawn::<PartyFollower>, enter_battle),
        )
        .add_systems(
            OnExit(Screen::Battle),
            (
                despawn::<BattleScene>,
                despawn::<PartyWindow>,
                despawn::<AllyPartyStrips>,
                despawn::<CommandWindow>,
                despawn::<HitFxRoot>,
                despawn::<BattleActor>,
                // Floating status badges (regen/barrier/…) are rebuilt each frame by
                // `render_status_icons`, which only runs in Battle. Without this, the
                // last frame's badges (e.g. a lingering Regen heart) orphan onto the
                // overworld and never clear. Tear them down on battle exit.
                despawn::<StatusIconLayer>,
            ),
        )
        .add_systems(
            Update,
            (
                validate_active,
                auto_fire_queued,
                tactics_toggle,
                tactics_click,
                menu_keyboard,
                menu_click,
                party_select_click,
                rebuild_command_menu,
                style_command_menu,
                render_enemy_panel,
                render_party_window,
                render_ally_parties,
                ally_collapse_click,
                advance_hit_fx,
                advance_atb_flash,
                render_hit_fx,
                // HD-2D arena: 3D combatant sprites + battle camera, framed by the UI.
                // Nested so the battle system tuple stays under Bevy's 20-arg cap.
                (
                    sync_battle_actors,
                    battle_click_target,
                    highlight_target,
                    drive_battle_action_clips,
                    drive_battle_facing,
                    animate_battle_actors,
                    battle_zoom_input,
                    battle_camera,
                    hd2d::animate_chars,
                    hd2d::place_billboards,
                    hd2d::billboard,
                    render_status_icons,
                ),
            )
                .run_if(in_state(Screen::Battle)),
        )
        // Ended — the extract/death summary. Clean any lingering world/battle actors
        // off it on entry, and (crucially) despawn the summary UI on EXIT: without
        // this the `EndedRoot` text was never removed, so it stayed on screen after
        // returning to The Last City and a second extraction stacked a duplicate on top.
        .add_systems(
            OnEnter(Screen::Ended),
            (
                ended_ui,
                despawn::<BattleActor>,
                despawn::<WorldEntity>,
                despawn::<CityScene>,
                despawn::<PartyFollower>,
            ),
        )
        .add_systems(OnExit(Screen::Ended), despawn::<EndedRoot>)
        .add_systems(Update, (ended_input, ended_buttons).run_if(in_state(Screen::Ended)))
        .run();
}

// ---------------------------------------------------------------- states ---

#[derive(States, Debug, Clone, PartialEq, Eq, Hash, Default)]
enum Screen {
    #[default]
    Join,
    /// The Last City — the persistent hub city. Post-auth home and the return target
    /// after every run: spend chits, read the Vault, and step through The
    /// Threshold to dive again. Closes the extract-or-die loop (see docs/proposals/last-city.md).
    City,
    /// Co-op lobby: create/join by code, ready up, host starts the shared dive.
    Lobby,
    Overworld,
    Battle,
    Ended,
}

/// Co-op lobby state, mirrored from the server's `lobby.state`.
#[derive(Resource, Default)]
struct LobbyData {
    in_lobby: bool,
    code: String,
    host: String,
    /// (player_id, username, ready)
    members: Vec<(String, String, bool)>,
    /// The code being typed on the join line (before joining).
    code_input: String,
    my_ready: bool,
}

// ------------------------------------------------------------- resources ---

/// Non-send: the browser socket handle isn't `Send`, so Bevy runs the systems
/// that touch it on the main thread.
struct NetRes(Net);

#[derive(Resource)]
struct Session {
    player_id: String,
    /// Account credentials typed on the Join screen (real, persistent accounts —
    /// register-on-first-use then login). Empty until the player types them.
    username: String,
    password: String,
    connecting: bool,
    entered: bool,
    channeling: bool,
    status: String,
    /// The party the player built on the Join screen — one class key per hero
    /// slot (wire form: "explorer" / "psyker" / "resonant"). Sent on enter_maze.
    party: Vec<String>,
    /// Which party slot the builder cursor is on.
    party_cursor: usize,
    /// True if the player chose Co-op at Join (go to the lobby after connecting
    /// instead of diving solo).
    coop: bool,
}

impl Default for Session {
    fn default() -> Self {
        Session {
            player_id: String::new(),
            username: String::new(),
            password: String::new(),
            connecting: false,
            entered: false,
            channeling: false,
            status: String::new(),
            // A diverse default so newcomers see a spread of classes at once.
            party: vec![
                "explorer".into(),
                "psyker".into(),
                "resonant".into(),
                "explorer".into(),
            ],
            party_cursor: 0,
            coop: false,
        }
    }
}

/// One overworld entity as the client knows it (from the latest snapshot).
#[derive(Clone)]
struct OwEntity {
    x: f32,
    y: f32,
    kind: EntityKind,
    /// Creature content id (monsters) or terrain kind (obstacles) — drives label/render.
    name: Option<String>,
    /// Creature faction (monsters only) — drives the colour.
    faction: Option<String>,
    /// World-unit radius for obstacles; 0 otherwise.
    radius: f32,
    /// True for a player currently in a fight (drives the ⚔ marker + Join prompt).
    battling: bool,
    /// Elevation level (terraced verticality); render height rises by `level*STEP_HEIGHT`.
    level: u8,
    /// For chests: whether it's been opened.
    opened: bool,
    /// Overworld mob intel (monsters only; `None` otherwise). Rendered as a
    /// nameplate only when the viewer's Explorer/Psyker perk unlocks each field.
    mob_level: Option<i32>,
    hp: Option<i32>,
    max_hp: Option<i32>,
    encounter_class: Option<String>,
    aggression: Option<String>,
}

impl OwEntity {
    fn player(x: f32, y: f32) -> Self {
        Self { x, y, kind: EntityKind::Player, name: None, faction: None, radius: 0.0, battling: false, level: 0, opened: false, mob_level: None, hp: None, max_hp: None, encounter_class: None, aggression: None }
    }
    fn monster(x: f32, y: f32, name: &str, faction: &str) -> Self {
        Self {
            x,
            y,
            kind: EntityKind::Monster,
            name: Some(name.to_string()),
            faction: Some(faction.to_string()),
            radius: 0.0,
            battling: false,
            level: 0,
            opened: false,
            mob_level: None,
            hp: None,
            max_hp: None,
            encounter_class: None,
            aggression: None,
        }
    }
    fn portal(x: f32, y: f32) -> Self {
        Self { x, y, kind: EntityKind::Portal, name: None, faction: None, radius: 0.0, battling: false, level: 0, opened: false, mob_level: None, hp: None, max_hp: None, encounter_class: None, aggression: None }
    }
}

#[derive(Resource, Default)]
struct Overworld {
    /// entity id -> its render state
    entities: HashMap<String, OwEntity>,
    /// Bumped on every snapshot so the render-side interpolation buffer
    /// ([`OwInterp`]) can tell when a fresh snapshot arrived.
    seq: u64,
}

/// One captured position sample, stamped with the client-clock time (seconds) it
/// was received.
#[derive(Clone, Copy, Default)]
struct InterpSample {
    x: f32,
    y: f32,
    level: f32,
    t: f32,
}

/// Per-entity overworld interpolation buffer: the two most recent snapshot samples
/// per entity. Remote sprites render by lerping between them (a little behind the
/// latest), instead of exponentially chasing the newest position — which lagged
/// and rubber-banded when snapshots arrived slightly irregularly. Purely
/// client-side: derived from the positions the server already sends, so no wire or
/// server change. The local player is exempt (kept responsive; see
/// `sync_overworld_sprites`).
#[derive(Resource, Default)]
struct OwInterp {
    seen_seq: u64,
    /// entity id -> (previous sample, current sample)
    states: HashMap<String, (InterpSample, InterpSample)>,
}

/// Render remote entities this many seconds behind the latest snapshot, so we
/// always interpolate between two *received* samples rather than extrapolating
/// past the newest one. One 100 ms server tick plus a little slack.
const OW_INTERP_DELAY: f32 = 0.11;

/// The current run's backpack (Town Portals + gathered materials), mirrored from
/// the server for the overworld HUD.
#[derive(Resource, Default)]
struct RunBackpack {
    items: Vec<(String, i32)>,
    /// Chits found this run (banked on extraction, lost on death).
    chits: i64,
    /// Looted red-chest gear this run as (name, atk_bonus).
    gear: Vec<(String, i32)>,
}

impl RunBackpack {
    fn count(&self, kind: &str) -> i32 {
        self.items.iter().find(|(k, _)| k == kind).map_or(0, |(_, q)| *q)
    }
}

/// Live exploration readouts (distance / biome / tier) that used to sit in the
/// always-on overworld HUD but now live only in the menu (Status tab). Kept as a
/// coarse resource — `update_run_stats` writes a field ONLY when its displayed
/// value actually changes — so the immediate-mode overlay doesn't rebuild every
/// frame (it must not: gear rows are real buttons that persist for click detection).
#[derive(Resource, Default)]
struct RunStats {
    distance: i64,
    tier: i64,
    biome: String,
}

/// The guaranteed clear path (world-unit waypoints), drawn as a faint trail so the
/// feasible route through the terrain is legible. `drawn` gates one-time spawning.
#[derive(Resource, Default)]
struct WorldPath {
    points: Vec<(f32, f32)>,
    drawn: bool,
}

/// The web of extra trails (disjoint edges), drawn as fainter dot-trails than the
/// backbone so the overworld reads as an interconnected maze of routes. `drawn` gates
/// one-time spawning (rebuilt after a battle, like [`WorldPath`]).
#[derive(Resource, Default)]
struct WorldWeb {
    edges: Vec<((f32, f32), (f32, f32))>,
    drawn: bool,
}

/// One elevation level of a terrace lifts the ground (and anything standing on it)
/// by this many world units — roughly one Kenney cliff-block tall, so a terrace
/// edge is dressed with a single row of `cliff_rock` models (see `spawn_terrace_cliffs`).
const STEP_HEIGHT: f32 = 2.0;

// (CLIFF_EDGE_SCALE/CLIFF_YAW_OFFSET removed — terrace edges are now HD-2D cliff
// sprite billboards; see overworld::spawn_terrace_cliffs.)
// (SLOPE_SCALE/SLOPE_YAW removed — slope connectors are now HD-2D billboards; see
// overworld::spawn_connector.)

/// Streamed terraced terrain: the elevation grid + connectors for every section the
/// server has sent. `build_terrain_sections` turns each into a stepped ground+cliff
/// mesh (rebuilding on return from battle, like the path trail).
#[derive(Resource, Default)]
struct Terrain {
    sections: HashMap<u32, meld_client::net::TerrainSectionView>,
}

/// Marks a spawned terrain-mesh / connector-prop entity, tagged by section index so
/// they can be despawned wholesale and rebuilt.
#[derive(Component)]
struct TerrainMesh(u32);

/// Walkable bounds + biome seams for the instance. The client streams framing
/// walls (edge treeline/ridge/water + west end-cap + gated biome seams) from this
/// plus the per-section terrain, extending them as the endless world grows.
#[derive(Resource, Default)]
struct WorldFrame {
    have: bool,
    x_min: f32,
    x_max: f32,
    lateral: f32,
    /// Crossing west of this world-x returns to Last City — the client marks it with
    /// a castle wall + gate so the boundary is visible before you cross it.
    west_return_border: f32,
    /// WG-4 radial fan arc (degrees; 0 = flat corridor). Content fans across this arc,
    /// leaving the western `360 - arc` wedge for Last City; the wall/gate is drawn as
    /// an arc clipped to that wedge (see the castle block in `build_terrain_sections`).
    radial_arc_degrees: f32,
    seams: Vec<meld_client::net::SeamLine>,
}

/// Marker for spawned framing-wall geometry (despawned on run change / screen exit).
#[derive(Component)]
struct WorldWall;

/// A spawned chest visual, tracked by id + opened state so it can be re-rendered
/// when it opens.
#[derive(Component)]
struct ChestEntity {
    id: String,
    opened: bool,
}

/// The caller's hero roster (name/class/level/stats), shown on the inventory party
/// screen — this is where stats live, not the battle HUD.
#[derive(Resource, Default)]
struct PartyRoster {
    heroes: Vec<meld_client::net::HeroLine>,
    /// AD-2: the class-pair synergies this comp has ACTIVE and the sequenced combos
    /// it can run, as described by the server. Shown on the party screen — a build
    /// system the player cannot see is a build system they will never plan around.
    synergies: Vec<meld_client::net::DepthLine>,
    combos: Vec<meld_client::net::DepthLine>,
    /// CL-1: the roster this account is working TOWARD — `(name, how to earn it)`
    /// per still-locked unlock, and how many party slots are open. Lives on the
    /// roster rather than its own overlay param because the party screen is the
    /// only place that shows it, and `render_overlay` is at Bevy's param ceiling.
    locked: Vec<(String, String)>,
    party_slots: i32,
}

/// The caller's earned overworld class perks ("party sense"), from `run.perks`.
/// Gates the Explorer avatar glow + monster intel, the Shifter minimap, the Psyker
/// threat markers, and the battle ATB reveal. Default = no perks.
#[derive(Resource, Default)]
struct PerksRes(meld_client::net::PerksLine);

/// In-progress hero rename on the party screen: the slot being edited + its buffer.
#[derive(Resource, Default)]
struct HeroRename {
    slot: Option<usize>,
    buffer: String,
}

/// Queue of pending "LEVEL UP!" stat screens (one per leveled hero), played
/// one-at-a-time old-school style. `elapsed` drives the line-by-line reveal +
/// auto-advance; `run_level` is the party's new run level for the banner.
#[derive(Resource, Default)]
struct LevelUpQueue {
    pending: std::collections::VecDeque<meld_client::net::HeroLevelUpLine>,
    current: Option<meld_client::net::HeroLevelUpLine>,
    run_level: i32,
    elapsed: f32,
    /// When set (offline demo/screenshot), the current hero is held on screen
    /// until [Space] instead of auto-advancing. Off in normal play.
    hold: bool,
}

/// Marker for the immediate-mode level-up screen root.
#[derive(Component)]
struct LevelUpRoot;

/// CL-1: what the account owns, plus a queue of unlocks still to announce.
/// `owned` is the server's full set on every message, so the client never has to
/// accumulate deltas to know what it can field.
#[derive(Resource, Default)]
struct UnlocksRes {
    owned: Vec<String>,
    party_slots: i32,
    pending: std::collections::VecDeque<meld_client::net::UnlockLine>,
    current: Option<meld_client::net::UnlockLine>,
    elapsed: f32,
    /// Offline demo/screenshot: hold the banner until [Space] instead of letting it
    /// time out, the same way the level-up screen does.
    hold: bool,
}

/// Marker for the immediate-mode unlock-banner root.
#[derive(Component)]
struct UnlockBannerRoot;

/// The two "announce this to the player" queues, bundled: `pump_net` is already at
/// Bevy's system-param ceiling, and these two always travel together anyway.
#[derive(bevy::ecs::system::SystemParam)]
struct Announce<'w> {
    levelup: ResMut<'w, LevelUpQueue>,
    unlocks: ResMut<'w, UnlocksRes>,
}

/// Marker for spawned path-trail dots (despawned when the path changes).
#[derive(Component)]
struct PathTrail;

/// Marker for spawned WEB-trail dots (the branch/loop/spur trails off the backbone).
#[derive(Component)]
struct WebTrail;

#[derive(Resource, Default)]
struct BattleData {
    battle_id: String,
    /// Combatant ids this player controls, in party order (Hero 1..N).
    your_ids: Vec<String>,
    monster_combatant: Option<String>,
    combatants: Vec<CombatantView>,
    /// Heroes whose ATB gauge is full (server said TurnReady).
    ready: HashSet<String>,
    /// Per-hero queued order (action + chosen target); auto-fires the instant that
    /// hero is ready.
    queued: HashMap<String, Order>,
    /// The hero the command window is giving orders to.
    active: Option<String>,
    /// The skill kind each hero most recently fired. `battle.action_resolved` only
    /// carries the coarse Attack/Skill kind, so this lets the sprite layer pick the
    /// exact special-ability clip (backstab vs frenzy, …) to play.
    last_skill: HashMap<String, String>,
}

/// A queued order: what the hero will do and (for aimed actions) which combatant it
/// hits. `target` is `None` for self-cast actions (Defend, Second Wind, Hold).
#[derive(Clone)]
struct Order {
    kind: QueuedKind,
    target: Option<String>,
}

/// Which side an order picks a target from. `None` from [`order_side`] means the
/// action is self-cast and needs no target picker.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Side {
    Enemy,
    Ally,
}

/// The side an order targets, or `None` if it is self-cast. Attacks and offensive
/// manifestations hit an enemy; heals/wards/items land on an ally (any living player
/// combatant, including co-op heroes who joined the battle).
fn order_side(kind: QueuedKind) -> Option<Side> {
    match kind {
        QueuedKind::Attack => Some(Side::Enemy),
        QueuedKind::Skill("power_strike") => Some(Side::Enemy),
        QueuedKind::Skill("transfuse") | QueuedKind::Skill("regen_boon") | QueuedKind::Skill("ward") => {
            Some(Side::Ally)
        }
        QueuedKind::Skill("second_wind") => None,
        // Shifter Flicker is a self-cast evasion blink (Backstab/Ransack hit an enemy
        // via the default arm below).
        QueuedKind::Skill("flicker") => None,
        // Phoenix Guard: Root is a self-cast stance; Toll of the Deep is an all-enemy
        // shockwave that needs no single target (Swell Strike / Kinetic Shock hit an
        // enemy via the default arm below).
        QueuedKind::Skill("root") | QueuedKind::Skill("toll_of_the_deep") => None,
        // Any other/unknown skill defaults to an offensive (enemy) target.
        QueuedKind::Skill(_) => Some(Side::Enemy),
        QueuedKind::Item(_) => Some(Side::Ally),
        QueuedKind::Defend => None,
        // Psyker Foci: Kinetic Aegis wards the caster (self); the rest are aimed at an
        // enemy. Revoke/Hold need no target.
        QueuedKind::Focus("cast", "kinetic_aegis") | QueuedKind::Focus("reinforce", "kinetic_aegis") => None,
        QueuedKind::Focus("cast", _) | QueuedKind::Focus("reinforce", _) => Some(Side::Enemy),
        QueuedKind::Focus(_, _) => None,
        QueuedKind::Hold => None,
        // Flee bails the whole party — no target to pick.
        QueuedKind::Flee => None,
    }
}

impl BattleData {
    /// The hero's (persistent) name, falling back to its party-order label.
    fn hero_label(&self, id: &str) -> String {
        if let Some(c) = self.view(id) {
            if !c.name.is_empty() && c.name != "Hero" {
                return c.name.clone();
            }
        }
        match self.your_ids.iter().position(|h| h == id) {
            Some(i) => format!("Hero {}", i + 1),
            None => id.to_string(),
        }
    }
    fn view(&self, id: &str) -> Option<&CombatantView> {
        self.combatants.iter().find(|c| c.id == id)
    }
    fn alive(&self, id: &str) -> bool {
        self.view(id).map(|c| c.hp > 0).unwrap_or(false)
    }
    /// Class of the hero the command window is currently giving orders to.
    fn active_class(&self) -> String {
        self.active
            .as_ref()
            .and_then(|a| self.view(a))
            .map(hero_class)
            .unwrap_or_else(|| "explorer".to_string())
    }
    /// Level of the active hero (for level-gated menus), default 1.
    fn active_level(&self) -> i32 {
        self.active
            .as_ref()
            .and_then(|a| self.view(a))
            .map(|c| c.level)
            .unwrap_or(1)
    }
}

/// A queued battle order for one hero. Attack/Skill hit the monster; Defend/Item
/// are self-cast. `Focus`/`Hold` are Psyker channels (verb, manifestation kind).
/// The `&'static str`s are the skill_kind / item_id / manifestation kind.
#[derive(Clone, Copy, PartialEq)]
enum QueuedKind {
    Attack,
    Defend,
    Skill(&'static str),
    Item(&'static str),
    /// Psyker: (verb, manifestation kind) — verb is "cast"/"reinforce"/"revoke".
    Focus(&'static str, &'static str),
    /// Psyker: let the active Foci tick, no new op.
    Hold,
    /// Flee the battle on this hero's turn — ends the whole encounter (a toll is
    /// charged server-side). Self-cast; needs no target.
    Flee,
}

impl QueuedKind {
    /// Short tag shown as the queued-order icon next to a hero.
    fn tag(self) -> &'static str {
        match self {
            QueuedKind::Attack => "ATK",
            QueuedKind::Defend => "DEF",
            QueuedKind::Skill(_) => "SKL",
            QueuedKind::Item(_) => "ITM",
            QueuedKind::Focus("cast", _) => "CST",
            QueuedKind::Focus("reinforce", _) => "RNF",
            QueuedKind::Focus("revoke", _) => "RVK",
            QueuedKind::Focus(_, _) => "FOC",
            QueuedKind::Hold => "...",
            QueuedKind::Flee => "FLEE",
        }
    }
    fn color(self) -> Color {
        match self {
            QueuedKind::Attack => Color::srgb(0.95, 0.55, 0.5),
            QueuedKind::Defend => Color::srgb(0.55, 0.7, 1.0),
            QueuedKind::Skill(_) => Color::srgb(0.8, 0.6, 1.0),
            QueuedKind::Item(_) => Color::srgb(0.5, 0.9, 0.6),
            QueuedKind::Focus(_, _) => Color::srgb(0.8, 0.6, 1.0),
            QueuedKind::Hold => Color::srgb(0.6, 0.65, 0.8),
            QueuedKind::Flee => Color::srgb(1.0, 0.5, 0.45),
        }
    }
}

/// Psyker manifestation catalog: (wire kind, display name, unlock level). Mirrors
/// the server's `manifest_unlock_level` for menu gating (display only).
const MANIFESTS: [(&str, &str, i32); 4] = [
    ("gravity_well", "Gravity Well", 1),
    ("kinetic_aegis", "Kinetic Aegis", 1),
    ("mind_spike", "Mind Spike", 3),
    ("temporal_anchor", "Temporal Anchor", 5),
];

/// Short two-letter tag for a manifestation kind (focus-bar display).
fn manifest_abbrev(kind: &str) -> String {
    kind.split('_')
        .filter_map(|w| w.chars().next())
        .map(|c| c.to_ascii_uppercase())
        .collect()
}

/// Parse a Psyker's Focus state out of its wire statuses:
/// `(focus_slots, [(kind, stacks), …])`.
fn parse_foci(statuses: &[String]) -> (usize, Vec<(String, u8)>) {
    let mut max = 0usize;
    let mut foci = Vec::new();
    for s in statuses {
        if let Some(n) = s.strip_prefix("focus_slots:") {
            max = n.parse().unwrap_or(0);
        } else if let Some(rest) = s.strip_prefix("focus:") {
            let mut it = rest.rsplitn(2, ':');
            let stacks = it.next().and_then(|x| x.parse().ok()).unwrap_or(1);
            if let Some(kind) = it.next() {
                foci.push((kind.to_string(), stacks));
            }
        }
    }
    (max, foci)
}

/// A hero's class key parsed from its wire statuses (`class:<key>`), default explorer.
fn hero_class(view: &CombatantView) -> String {
    view.statuses
        .iter()
        .find_map(|s| s.strip_prefix("class:"))
        .unwrap_or("explorer")
        .to_string()
}

/// A numeric status value (`prefix<n>`) parsed from a combatant's statuses.
fn status_num(statuses: &[String], prefix: &str) -> i32 {
    statuses
        .iter()
        .find_map(|s| s.strip_prefix(prefix).and_then(|n| n.parse().ok()))
        .unwrap_or(0)
}

/// Autoplay heuristic for a Resonant hero: mend the party (Transfuse) whenever any
/// ally is meaningfully hurt, otherwise chip at the enemy.
fn resonant_autoplay_op(battle: &BattleData) -> QueuedKind {
    let wounded = battle.combatants.iter().any(|c| {
        c.is_player && c.hp > 0 && (c.hp as f32 / c.max_hp.max(1) as f32) < 0.7
    });
    if wounded {
        QueuedKind::Skill("transfuse")
    } else {
        QueuedKind::Attack
    }
}

/// Explorer kit catalog: (wire kind, unlock level, Adrenaline cost). A display/autoplay
/// mirror of balance `[battle] explorer_*_cost` + `meld_proto::skills` unlock levels —
/// the server stays authoritative; this only steers the menu/autoplay.
const HUNTER_SKILLS: [(&str, i32, i32); 4] = [
    ("power_strike", 1, 40),
    ("second_wind", 2, 35),
    ("snare", 2, 30),
    ("frenzy", 3, 80),
];

/// Autoplay heuristic for a Explorer hero: build Adrenaline with basic attacks, then
/// release. Heal with Second Wind when badly hurt and it can afford it; otherwise if
/// it has leveled into Frenzy it banks toward that big hit, else cashes in Power
/// Strike as soon as it can afford it.
fn explorer_autoplay_op(view: &CombatantView) -> QueuedKind {
    let adr = status_num(&view.statuses, "adrenaline:");
    let skill = |kind: &str| HUNTER_SKILLS.iter().find(|(k, _, _)| *k == kind).unwrap();
    let (_, sw_lv, sw_cost) = *skill("second_wind");
    let hurt = (view.hp as f32) < 0.4 * view.max_hp.max(1) as f32;
    if hurt && view.level >= sw_lv && adr >= sw_cost {
        return QueuedKind::Skill("second_wind");
    }
    let (_, frenzy_lv, frenzy_cost) = *skill("frenzy");
    if view.level >= frenzy_lv {
        // Save up, then unleash Frenzy.
        return if adr >= frenzy_cost { QueuedKind::Skill("frenzy") } else { QueuedKind::Attack };
    }
    let (_, ps_lv, ps_cost) = *skill("power_strike");
    if view.level >= ps_lv && adr >= ps_cost {
        return QueuedKind::Skill("power_strike");
    }
    QueuedKind::Attack // build Adrenaline
}

/// Autoplay heuristic for a Shifter hero: blink (Flicker) to stay slippery when it
/// has none active and has leveled into it, otherwise stab with the armour-piercing
/// Backstab (falling back to a plain Attack before the skill unlocks at L1).
fn shifter_autoplay_op(view: &CombatantView) -> QueuedKind {
    let has_evasion = status_num(&view.statuses, "evasion:") > 0;
    if !has_evasion && view.level >= meld_proto::skills::unlock_level("flicker") {
        return QueuedKind::Skill("flicker");
    }
    QueuedKind::Skill("backstab")
}

/// Autoplay heuristic for an Phoenix Guard hero: smash with the heaviest kinetic strike
/// it has unlocked — Kinetic Shock once available (L3), otherwise Swell Strike (L1).
fn phoenix_guard_autoplay_op(view: &CombatantView) -> QueuedKind {
    if view.level >= meld_proto::skills::unlock_level("kinetic_shock") {
        QueuedKind::Skill("kinetic_shock")
    } else {
        QueuedKind::Skill("swell_strike")
    }
}

/// Autoplay heuristic for a Psyker hero: fill free slots with unlocked
/// manifestations (offense first, then the ward), then reinforce, else hold.
fn psyker_autoplay_op(view: &CombatantView) -> QueuedKind {
    let (max, foci) = parse_foci(&view.statuses);
    let has = |k: &str| foci.iter().any(|(kind, _)| kind == k);
    if foci.len() < max {
        for (kind, _name, lv) in MANIFESTS {
            if view.level >= lv && !has(kind) {
                return QueuedKind::Focus("cast", kind);
            }
        }
    }
    for (kind, stacks) in &foci {
        if *stacks < 2 {
            if let Some((k, _, _)) = MANIFESTS.iter().find(|(mk, _, _)| *mk == kind.as_str()) {
                return QueuedKind::Focus("reinforce", k);
            }
        }
    }
    QueuedKind::Hold
}

/// Which overworld overlay screen is open (none/inventory/level-up).
#[derive(Clone, Copy, PartialEq)]
enum OverlayKind {
    Inventory,
    LevelUp,
}
#[derive(Resource, Default)]
struct Overlay {
    kind: Option<OverlayKind>,
}

/// Which of the `OverlayKind::Inventory` panel's vertical tabs is showing.
#[derive(Resource, Clone, Copy, PartialEq, Default)]
enum OverlayTab {
    #[default]
    Items,
    Equip,
    Status,
}

/// Which hero the Equip tab is showing/acting on (party slot index).
#[derive(Resource, Default)]
struct EquipSelection {
    hero_slot: usize,
}

/// Keyboard cursor position within the active overlay tab's navigable row
/// list (see `equip_tab_rows`) — Up/Down move it, Left/Right switch tabs
/// (and reset it), Space activates whatever it's on.
#[derive(Resource, Default)]
struct OverlayCursor {
    index: usize,
}

/// Where a piece of gear in the Equip tab comes from — the caller's permanent
/// Vault (equip persists to the account, HTTP, next-dive-only) or this run's
/// not-yet-banked loot (equip is run-scoped, WS, effective immediately). Each
/// routes an equip click/keypress to a different backend call.
#[derive(Clone, Copy, PartialEq)]
enum GearSource {
    Vault,
    RunLoot,
}

/// One keyboard/click-navigable row in the Equip tab's main (per-hero)
/// screen: a hero-switcher button, or one equipment category — activating a
/// category opens the picker screen for it (`EquipPicker`).
#[derive(Clone)]
enum EquipRow {
    Hero(usize),
    Category(&'static str),
}

/// One keyboard/click-navigable row in the Equip tab's picker screen (opened
/// by activating a category on the main screen): an explicit unequip, or one
/// candidate piece of gear. Selecting a `Gear` row is the only thing that
/// equips it — nothing equips just by being found or browsed past.
#[derive(Clone)]
enum PickerRow {
    Unequip,
    Gear { gear_id: String, source: GearSource },
}

/// The Equip tab's optional second screen. `None` shows the per-hero summary
/// (current item per category); `Some(category)` shows every candidate for
/// that category — the picker is what a category row opens when activated.
#[derive(Resource, Default)]
struct EquipPicker {
    category: Option<&'static str>,
}

/// This run's not-yet-banked loot gear (mirrors `run.gear` snapshots from the
/// server) — separate from `InventoryData.gear`, which is the permanent
/// Vault. Both feed the Equip tab; see `GearSource`.
#[derive(Resource, Default)]
struct RunGearData {
    gear: Vec<GearLine>,
}

/// How many heroes the Equip/Status tabs have to show: the active run's
/// `PartyRoster` when there is one, else the caller's persistent account names
/// (e.g. opening the storage chest from the City, with no run in progress).
fn hero_count(roster: &PartyRoster, names: &AccountHeroNames) -> usize {
    if !roster.heroes.is_empty() {
        roster.heroes.len()
    } else {
        names.names.len()
    }
}

/// The display name for hero slot `i`, from whichever source `hero_count` used.
fn hero_name_at(roster: &PartyRoster, names: &AccountHeroNames, i: usize) -> Option<String> {
    if !roster.heroes.is_empty() {
        roster.heroes.get(i).map(|h| h.name.clone())
    } else {
        names.names.get(i).cloned()
    }
}

/// Hero slot `i`'s class this dive, if known — only the roster (populated
/// during an active run) carries class; browsing gear from the City with no
/// dive in progress has no class to filter by (`category_gear` shows
/// everything in that case rather than guessing).
/// The class of hero `i`: the live run's roster when there is one, else the
/// account's persisted classes (GR-7) so the Equip tab knows the rules in town
/// too. `None` only when neither source has ever recorded that slot.
fn hero_class_at<'a>(
    roster: &'a PartyRoster,
    names: &'a AccountHeroNames,
    i: usize,
) -> Option<&'a str> {
    roster
        .heroes
        .get(i)
        .map(|h| h.class_key.as_str())
        .or_else(|| names.classes.get(i).map(|c| c.as_str()))
        .filter(|c| !c.is_empty())
}

/// Why this hero may not wear this item, in the words the player sees. `None`
/// when the item is legal (or the rules cannot apply — unknown class). The same
/// `meld_proto::equipment` table the server enforces, so the UI can never claim
/// something the server would allow, or vice versa (GR-5).
pub(crate) fn gear_block_reason(item: &GearLine, hero_class: Option<&str>) -> Option<String> {
    use meld_proto::equipment::{self as eq, Legality};
    let class = eq::class_from_key(hero_class?)?;
    let verdict = eq::check_equip(
        class,
        &item.class_key,
        &item.slot,
        eq::ItemFamily::from_wire(&item.family),
        eq::ArmorWeight::from_wire(&item.armor_weight),
    );
    match verdict {
        Legality::Ok => None,
        Legality::ClassFamily => Some("cannot wield".into()),
        Legality::ClassWeight => Some("too heavy".into()),
        Legality::ClassExclusive => Some("another class".into()),
        Legality::SlotMismatch => Some("wrong slot".into()),
    }
}

/// The off-hand item this hero would have to take off to hold `item` — `Some`
/// only when `item` is two-handed and something is in the way. Equipping then
/// clears it for the player instead of handing them a 409 (GR-5).
pub(crate) fn off_hand_in_the_way(
    gear: &[GearLine],
    item: &GearLine,
    hero_slot: usize,
) -> Option<String> {
    let two_handed = meld_proto::equipment::ItemFamily::from_wire(&item.family)
        .map(|f| f.reserves_off_hand())
        .unwrap_or(false);
    if !two_handed {
        return None;
    }
    gear.iter()
        .find(|g| g.slot == "off_hand" && g.equipped_hero_slot == Some(hero_slot))
        .map(|g| g.gear_id.clone())
}

/// Gear from one source, filtered to one category and `selected` — unequipped
/// or already worn by `selected`. A hero's row never shows gear another hero
/// currently has on, so there's nothing to accidentally snipe. Also hides gear
/// restricted to a different class whenever `hero_class` is known (mid-run,
/// via the roster) — unrestricted gear and, when the class isn't known (e.g.
/// browsing from the City with no active run), everything still shows. Shared
/// by `equip_tab_rows` and the Equip tab's render body so the two can't drift.
fn category_gear<'a>(
    gear: &'a [GearLine],
    category: &str,
    selected: usize,
    hero_class: Option<&str>,
) -> Vec<&'a GearLine> {
    let mut items: Vec<&GearLine> = gear
        .iter()
        .filter(|g| {
            g.slot == category
                && (g.equipped_hero_slot.is_none() || g.equipped_hero_slot == Some(selected))
                && (g.class_key.is_empty() || hero_class.map_or(true, |c| c == g.class_key))
        })
        .collect();
    items.sort_by(|a, b| a.name.cmp(&b.name));
    items
}

/// The six item categories of the 7-slot loadout (Epic GR spec §5), in
/// display order. Two accessory *equip* slots share the one accessory
/// category (the server enforces the ×2 capacity).
pub(crate) const GEAR_CATEGORIES: [&str; 6] =
    ["main_hand", "off_hand", "head", "chest", "legs", "accessory"];

/// The Equip tab main screen's navigable row list: hero switcher buttons,
/// then the six equipment categories (each opens the picker screen).
fn equip_main_rows(hero_count: usize) -> Vec<EquipRow> {
    let mut rows: Vec<EquipRow> = (0..hero_count).map(EquipRow::Hero).collect();
    for c in GEAR_CATEGORIES {
        rows.push(EquipRow::Category(c));
    }
    rows
}

/// The Equip tab picker screen's navigable row list for `category` +
/// `selected`: an explicit unequip, then every candidate from the Vault and
/// this run's not-yet-banked loot (`GearSource`) — Vault rows first.
fn equip_picker_rows(
    inv: &InventoryData,
    run_gear: &RunGearData,
    category: &str,
    selected: usize,
    hero_class: Option<&str>,
) -> Vec<PickerRow> {
    let mut rows = vec![PickerRow::Unequip];
    rows.extend(category_gear(&inv.gear, category, selected, hero_class).into_iter().map(|g| {
        PickerRow::Gear { gear_id: g.gear_id.clone(), source: GearSource::Vault }
    }));
    rows.extend(category_gear(&run_gear.gear, category, selected, hero_class).into_iter().map(|g| {
        PickerRow::Gear { gear_id: g.gear_id.clone(), source: GearSource::RunLoot }
    }));
    rows
}

/// Whichever item is actually contributing to combat for `selected` in
/// `category` right now: a run-loot equip in that category overrides the
/// Vault baseline (mirrors the server's `effective_gear_bonus`), so this is
/// what the main screen displays as "currently worn" and what the picker's
/// arrows compare against.
fn effective_worn_item<'a>(
    inv: &'a InventoryData,
    run_gear: &'a RunGearData,
    category: &str,
    selected: usize,
) -> Option<&'a GearLine> {
    run_gear
        .gear
        .iter()
        .find(|g| g.slot == category && g.equipped_hero_slot == Some(selected))
        .or_else(|| inv.gear.iter().find(|g| g.slot == category && g.equipped_hero_slot == Some(selected)))
}

fn effective_baseline_stat(
    inv: &InventoryData,
    run_gear: &RunGearData,
    category: &str,
    selected: usize,
) -> i32 {
    effective_worn_item(inv, run_gear, category, selected).map(gear_slot_stat).unwrap_or(0)
}

/// Clear `selected`'s equipped item(s) in `category` — both the Vault's (if
/// any) and this run's loot equip (if any), so the slot is genuinely empty
/// afterward and every candidate becomes available to hand to another hero.
fn unequip_category(net: &Net, inv: &InventoryData, run_gear: &RunGearData, category: &str, selected: usize) {
    if let Some(g) = inv.gear.iter().find(|g| g.slot == category && g.equipped_hero_slot == Some(selected)) {
        net.equip_gear(g.gear_id.clone(), None);
    }
    if let Some(g) = run_gear.gear.iter().find(|g| g.slot == category && g.equipped_hero_slot == Some(selected)) {
        net.send(ClientCmd::EquipLoot { gear_id: g.gear_id.clone(), hero_slot: None });
    }
}

/// Vault + gear for the inventory overlay (fetched over HTTP on open).
#[derive(Resource, Default)]
struct InventoryData {
    loaded: bool,
    chits: i64,
    materials: Vec<(String, i32)>,
    gear: Vec<GearLine>,
    /// Materials withdrawn from the Vault (storage chest), staged to seed the
    /// next dive's Backpack.
    pending: Vec<(String, i32)>,
}

/// Meld skills + class unlocks for the level-up overlay.
#[derive(Resource, Default)]
struct ProgressData {
    loaded: bool,
    skills: Vec<SkillLine>,
    classes: Vec<String>,
}

/// Persistent per-account hero names (`GET /v1/heroes`) — the Equip/Status tabs'
/// hero-name fallback when there's no active run's `PartyRoster` to source
/// names from (e.g. opening the storage chest from the City).
#[derive(Resource, Default)]
struct AccountHeroNames {
    loaded: bool,
    names: Vec<String>,
    /// Each slot's persisted class key (GR-7) — what the server will actually
    /// enforce an equip against; empty for a slot that has never dived.
    classes: Vec<String>,
}

/// Floating hit-feedback numbers (damage/heal) with a short lifetime, plus the
/// attacker-lunge timers ([`Self::acts`]) that drive the "step in to strike" motion.
#[derive(Resource, Default)]
struct HitFx {
    items: Vec<Hit>,
    /// Attacker id → seconds since it landed a damaging action (dropped past
    /// [`ATTACK_LUNGE_TTL`]); [`animate_battle_actors`] lunges that sprite.
    acts: HashMap<String, f32>,
    /// Actor id → the animation clip to play once for a just-resolved action (its
    /// `attack` or a specific special). Consumed by `drive_battle_action_clips`,
    /// which hands it to the actor's `hd2d::CharSprite`.
    act_clip: HashMap<String, String>,
    /// Monster ability shout bubbles (spec §6): a telegraph flashes for its
    /// channel window; an instant ability's callout pops briefly.
    callouts: Vec<Callout>,
}
struct Hit {
    target: String,
    text: String,
    color: Color,
    age: f32,
    /// Font-size multiplier — WEAK! hits pop bigger (screen-shaking flourish).
    scale: f32,
}
/// A monster's ability shout bubble (see `HitFx::callouts`).
struct Callout {
    combatant_id: String,
    text: String,
    age: f32,
    ttl: f32,
    /// Telegraphs flash (channeling); instant callouts just fade.
    flashing: bool,
}
/// Seconds a floating number lives.
const HIT_TTL: f32 = 1.0;
/// Seconds a target stays "flashed" after being hit.
const FLASH_TTL: f32 = 0.18;
/// Seconds a struck sprite is knocked back before easing home.
const HIT_RECOIL_TTL: f32 = 0.3;
/// Seconds a struck sprite flashes white (a subset of the recoil).
const HIT_WHITE_TTL: f32 = 0.12;
/// Seconds an attacker's lunge (step in + back) lasts.
const ATTACK_LUNGE_TTL: f32 = 0.34;
/// Seconds a hero's cell/ATB bar flashes when its gauge fills (turn ready).
const ATB_FLASH_TTL: f32 = 0.55;

/// Tracks the "your turn!" pop: when a hero newly enters `BattleData::ready`, its
/// id gets a fading flash (see [`advance_atb_flash`]) that [`party_cell`] renders as
/// a quick brighten of the ATB bar + cell border.
#[derive(Resource, Default)]
struct AtbFlash {
    /// Heroes that were ready last frame, to detect the rising edge.
    prev: HashSet<String>,
    /// Active flashes: hero id → seconds elapsed (dropped past [`ATB_FLASH_TTL`]).
    age: HashMap<String, f32>,
}

/// UI state for the City (The Last City) hub screen.
#[derive(Resource, Default)]
struct CityUi {
    /// A transient district notice (e.g. a not-yet-built district's status).
    notice: String,
    /// Index into [`CITY_DISTRICTS`] the avatar is currently standing in (for the
    /// contextual interact prompt), or `None` when out in the open plaza.
    near: Option<usize>,
    /// True while the Apothecary's shelf is open (EC-2).
    shop_open: bool,
    /// True while the Vanguard Wall is lit — the board replaces the notice line
    /// until the player walks away or presses [E] again.
    board_open: bool,
}

/// The Apothecary's shelf as last read from `GET /v1/vendors/apothecary` (EC-2).
#[derive(Resource, Default)]
pub(crate) struct ShopData {
    pub vendor: String,
    pub items: Vec<meld_client::net::ShopLine>,
    pub loaded: bool,
}

/// The live Vanguard Board as last read from `GET /v1/leaderboards/vanguard`
/// (P1-1) — what the Vanguard Wall in Last City displays.
#[derive(Resource, Default)]
pub(crate) struct VanguardBoardData {
    pub season: i32,
    pub entries: Vec<meld_client::net::VanguardLine>,
    /// The caller's own rank this season, when they are on the page.
    pub you: Option<i32>,
    pub loaded: bool,
}

#[derive(Resource, Default)]
struct EndInfo {
    outcome: String,
    banked: usize,
    /// Chits banked (extracted) or forfeited (died) with this run.
    chits: i64,
    /// Count of red-chest gear banked to the Vault on extraction.
    gear: usize,
}

/// A loot report banner shown for a few seconds after a battle victory or a
/// treasure chest opening (XP/chits/items/gear gained). `xp` is `None` for a
/// chest (no XP line shown) and `Some` for a battle.
#[derive(Resource, Default)]
struct LootReport {
    active: bool,
    title: String,
    xp: Option<i64>,
    chits: i64,
    items: Vec<(String, i32)>,
    gear: Vec<String>,
    elapsed: f32,
}

/// Paces MoveIntents at a fixed cadence (see [`MOVE_INTENT_HZ`]) so walk speed
/// is independent of render frame rate. `acc` banks elapsed time between sends.
#[derive(Resource, Default)]
struct MoveClock {
    acc: f32,
}

/// When true, the client self-drives the loop against the real server.
#[derive(Resource)]
struct Autoplay(bool);

/// The Tactics auto-battle toggle (spec §6): available while an Phoenix Guard is
/// in the battle; when enabled, ready heroes auto-queue their class default
/// (same per-class heuristics as `?autoplay`) with no human reaction delay.
/// Toggled with T on the battle screen.
#[derive(Resource, Default)]
struct Tactics(bool);

/// When true (`?city` / `MELD_CITY`), the client connects but parks in The Last City
/// (the hub) instead of auto-diving — for screenshotting / iterating on the city.
#[derive(Resource)]
struct CityIdle(bool);

/// Offline render demo: scripts canned data through the real rendering systems
/// (no server). Used to show the Overworld/Battle screens where a live WS isn't
/// available (e.g. a headless preview browser).
#[derive(Resource)]
struct Demo {
    on: bool,
    t: f32,
    started: bool,
}

// -------------------------------------------------------- battle command ---

/// Which page of the battle command window is showing. FF/Lufia-style: the root
/// commands, a Skill / Item sub-list, the Psyker Manifestation list, and the dynamic
/// Target / Revoke pickers (whose rows come from live battle state, not [`menu_entries`]).
#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
enum MenuLevel {
    #[default]
    Root,
    Skills,
    Items,
    /// Psyker: the Manifestation list (shaped like the Skill list). Selecting one
    /// casts it, or reinforces it if already active.
    Manifest,
    /// Psyker: pick which active Manifestation to end.
    Revoke,
    /// Pick which combatant the pending action hits (enemy or ally).
    Target,
}

/// What selecting a menu row does.
#[derive(Clone, Copy)]
enum EntryAction {
    Attack,
    Defend,
    OpenSkills,
    OpenItems,
    Skill(&'static str), // skill_kind
    Item(&'static str),  // item_id
    /// Psyker: open the Manifestation list.
    OpenManifest,
    /// Psyker: open the Revoke picker (active Foci).
    OpenRevoke,
    /// Psyker: cast or reinforce this manifestation kind (verb inferred from whether
    /// it is already active).
    Manifest(&'static str),
    /// Psyker: hold — let the active Foci tick.
    Hold,
    /// Flee the encounter (self-cast on the active hero).
    Flee,
    Back,
}

/// One selectable row in the command window.
struct MenuEntry {
    /// Owned, because an item row carries its count ("Bloom Salve x2").
    label: String,
    action: EntryAction,
    /// What this row DOES, shown under the menu while it is selected. Comes from
    /// the shared registry, so the tooltip cannot drift from what the server
    /// resolves. Empty for rows that need no explanation (Back, Flee).
    tooltip: String,
}

/// The class's kit as menu rows, keeping only what the hero has leveled into. Read
/// straight from the shared registry: the name, the order, the unlock level and the
/// tooltip are all one definition (`meld_proto::skills`).
fn skill_entries(class: &str, hero_level: i32) -> Vec<MenuEntry> {
    meld_proto::skills::skills_for_class(class)
        .into_iter()
        .filter(|d| hero_level >= d.unlock)
        .map(|d| MenuEntry {
            label: d.name.to_string(),
            action: EntryAction::Skill(d.key),
            tooltip: d.description.to_string(),
        })
        .collect()
}

/// The rows shown at a given menu level. For a Psyker the root is `Focus / Revoke /
/// Hold` and the Manifest page lists the manifestations unlocked at `hero_level`. The
/// dynamic pages (Target, Revoke) draw their rows from live battle state instead
/// ([`BattleMenu::rows`]), so they return empty here.
/// Build the command menu for one page. `held` is the run backpack as
/// `(item_kind, quantity)` — the Items page is built from what the party ACTUALLY
/// carries, so it can never offer a potion the server will refuse.
fn menu_entries(
    level: MenuLevel,
    class: &str,
    hero_level: i32,
    held: &[(String, i32)],
) -> Vec<MenuEntry> {
    let e = |label: &str, action| MenuEntry {
        label: label.to_string(),
        action,
        tooltip: String::new(),
    };
    match level {
        MenuLevel::Root if class == "psyker" => vec![
            e("Focus", EntryAction::OpenManifest),
            e("Revoke", EntryAction::OpenRevoke),
            e("Hold", EntryAction::Hold),
            e("Flee", EntryAction::Flee),
        ],
        // The d-pad cross keys off these indices: 0 Attack (centre), 1 Defend
        // (right), 2 Item (left), 3 Skill (up), 4 Flee (down). Keep this order in
        // sync with `rebuild_command_menu`'s cross and `menu_keyboard`'s arrows.
        MenuLevel::Root => vec![
            e("Attack", EntryAction::Attack),
            e("Defend", EntryAction::Defend),
            e("Item", EntryAction::OpenItems),
            e("Skill", EntryAction::OpenSkills),
            e("Flee", EntryAction::Flee),
        ],
        // Skills unlock as the hero levels; a locked one is simply hidden (the
        // server would reject it anyway). The rows — names, order, unlock levels and
        // tooltips — all come from `meld_proto::skills`, so a class's kit is defined
        // in exactly one place instead of once per surface.
        MenuLevel::Skills => {
            let mut v = skill_entries(class, hero_level);
            v.push(e("Back", EntryAction::Back));
            v
        }
        // GR-4: only the potions the party is carrying, with counts. The old page
        // offered a fixed "Salve"/"Elixir" pair — and "salve" is not even a real item
        // kind, so that row could only ever come back "Out of salve".
        MenuLevel::Items => {
            let mut v: Vec<MenuEntry> = meld_proto::consumables::CONSUMABLES
                .iter()
                .filter_map(|def| {
                    let qty = held
                        .iter()
                        .find(|(kind, _)| kind == def.key)
                        .map(|(_, q)| *q)
                        .unwrap_or(0);
                    (qty > 0).then(|| MenuEntry {
                        label: format!("{} x{qty}", def.name),
                        action: EntryAction::Item(def.key),
                        tooltip: def.description.to_string(),
                    })
                })
                .collect();
            if v.is_empty() {
                v.push(e("(no potions)", EntryAction::Back));
            }
            v.push(e("Back", EntryAction::Back));
            v
        }
        MenuLevel::Manifest => {
            let mut v: Vec<MenuEntry> = MANIFESTS
                .iter()
                .filter(|(_, _, lv)| hero_level >= *lv)
                .map(|(kind, name, _)| e(*name, EntryAction::Manifest(kind)))
                .collect();
            v.push(e("Back", EntryAction::Back));
            v
        }
        // Rows come from `BattleMenu::rows`; rendered/selected specially.
        MenuLevel::Target | MenuLevel::Revoke => Vec::new(),
    }
}

/// Battle command-window state: which page, and the highlighted row. `dirty`
/// asks [`rebuild_command_menu`] to respawn the rows (only on a page change, so
/// button entities persist within a page and clicks/taps register).
#[derive(Resource, Default)]
struct BattleMenu {
    level: MenuLevel,
    cursor: usize,
    dirty: bool,
    /// The action waiting for a target: `(actor id, order kind)`. Set when a command
    /// that needs a target is chosen; consumed when a Target row is picked.
    pending: Option<(String, QueuedKind)>,
    /// Dynamic rows for the Target/Revoke pages: `(display label, value)`. The value
    /// is a combatant id (Target) or a manifestation kind (Revoke).
    rows: Vec<(String, String)>,
    /// Signature of the last-rendered command panel (shown?, active hero, page).
    /// `rebuild_command_menu` respawns only when this changes, so button
    /// `Interaction` survives across frames within one stable state.
    sig: String,
}

// ------------------------------------------------------------- marker(s) ---

#[derive(Component)]
struct JoinRoot;
#[derive(Component)]
struct OverworldRoot;
/// Immediate-mode enemy panel + battle banner (top of the screen).
#[derive(Component)]
struct BattleScene;
/// Immediate-mode party status window (bottom-left).
#[derive(Component)]
struct PartyWindow;
/// Immediate-mode edge strips showing joined allies' parties (north/west/east).
#[derive(Component)]
struct AllyPartyStrips;
/// Persistent command window (bottom-right); rebuilt only on page change.
#[derive(Component)]
struct CommandWindow;
/// One clickable row in the command window, tagged with its index.
#[derive(Component)]
struct MenuRow {
    index: usize,
}
/// The tappable Phoenix Guard Tactics-stance toggle in the command window (keyboard: T).
#[derive(Component)]
struct TacticsButton;
/// A clickable party HUD cell: tapping it makes that hero the one the command panel
/// is giving orders to (if it's alive and hasn't locked an action yet). The
/// touch-friendly way to pick WHICH ready hero to command.
#[derive(Component)]
struct PartyCellButton {
    id: String,
}
/// Immediate-mode overlay holding floating hit numbers.
#[derive(Component)]
struct HitFxRoot;
/// Immediate-mode root for an overworld overlay (inventory / level-up).
#[derive(Component)]
struct OverlayRoot;
/// A clickable gear row in the Equip tab's picker screen: clicking equips it
/// to the hero the picker was opened for — over HTTP for a Vault item, over
/// WS for run-loot (see `GearSource`) — and closes back to the main screen.
/// `worn` only drives the row's highlight, not the click behavior.
#[derive(Component)]
struct GearButton {
    gear_id: String,
    source: GearSource,
    /// Hero slot this row equips to when clicked (whichever hero the picker
    /// was opened for).
    target_hero_slot: usize,
    /// True if this hero already has this exact item equipped (highlight only).
    worn: bool,
    /// Set when this hero's class may not wear the item (GR-5): the row renders
    /// dim with the reason and a press does nothing, so the player is told the
    /// rule instead of being handed a server refusal.
    blocked: bool,
    /// The off-hand item that has to come off first for this (two-handed) item.
    free_first: Option<String>,
}
/// A category row on the Equip tab's main screen ("Weapon", "Armor",
/// "Accessory"). Clicking opens the picker screen for that category.
#[derive(Component)]
struct CategoryButton {
    category: &'static str,
}
/// The picker screen's explicit "remove" row: clicking clears whatever the
/// hero currently has equipped in `category` (both Vault and run-loot) and
/// closes back to the main screen.
#[derive(Component)]
struct PickerUnequipButton {
    category: &'static str,
}
/// The picker screen's "back" control: clicking closes it without changing
/// anything, returning to the main Equip screen (mirrors pressing Escape).
#[derive(Component)]
struct PickerBackButton;
/// A per-hero front/back-row toggle on the party screen. Clicking flips the row and
/// sends [`ClientCmd::SetFormation`]; `back_row` is the hero's current rank.
#[derive(Component)]
struct FormationButton {
    slot: i32,
    back_row: bool,
}
/// One vertical tab button on the inventory overlay (Items / Equip / Status).
#[derive(Component)]
struct TabButton(OverlayTab);
/// One hero-switcher button on the Equip tab.
#[derive(Component)]
struct HeroSwitchButton(usize);
/// A "Withdraw" button on a material row in the Items tab: takes 1 unit out of
/// the Vault into the pending-backpack queue for the next dive.
#[derive(Component)]
struct WithdrawButton {
    item_kind: String,
}
#[derive(Component)]
struct EndedRoot;
/// Root of the City (The Last City) HUD (2D overlay).
#[derive(Component)]
struct CityRoot;
/// Any 3D entity of the walkable city scene (ground, buildings, props, avatar).
#[derive(Component)]
struct CityScene;
/// The walkable player avatar in the city (moved locally, no server).
#[derive(Component)]
struct CityPlayer;
/// The City's live Vault summary line (chits + banked-count), refreshed from
/// [`InventoryData`] as it loads.
#[derive(Component)]
struct CityVaultText;
/// The City's status/prompt line (dive prompts + district notices).
#[derive(Component)]
struct CityStatusText;
#[derive(Component)]
struct StatusText;

/// A sprite representing an overworld entity, tagged by its server id.
#[derive(Component)]
struct WorldEntity(String);

/// Toggle for showing the whole party (the lead + its heroes) trailing you in the
/// overworld, instead of just the lead avatar. Flipped from the party/inventory
/// screen or with `P` (see [`toggle_party_view`] / [`sync_party_followers`]).
#[derive(Resource, Default)]
struct PartyView {
    show: bool,
}

/// One of the lead's heroes, drawn trailing them in the overworld when [`PartyView`]
/// is on. Purely cosmetic (client-side): the server still tracks one avatar per
/// player. `slot` is its party index (1..) and its trailing position in formation.
#[derive(Component)]
struct PartyFollower {
    slot: usize,
}

/// A gatherable/pickup (harvest node or ground loot) whose own material slowly
/// pulses an emissive glow to draw the eye — instead of a flat disc on the ground
/// that z-fights the terrain. `pulse_collectibles` animates the emissive.
#[derive(Component)]
struct Collectible;

/// The overworld HUD line that reports distance + current biome.
#[derive(Component)]
struct HudText;

/// Root of the co-op lobby screen.
#[derive(Component)]
struct LobbyRoot;
/// The lobby screen's dynamic body (member list / join prompt).
#[derive(Component)]
struct LobbyText;

// ---------------------------------------------------------------- setup ----

/// The single lit ground plane (recoloured to the current biome as you travel).
#[derive(Component)]
struct WorldGround;

/// The HD-2D file channel, run in every screen: hot-reload the look params from
/// `/tmp/meld-look.json` when they change, and honour a screenshot request. Lets
/// the look be tuned + captured hands-free on a live native window.
fn hd2d_remote(
    mut commands: Commands,
    mut look: ResMut<hd2d::Look>,
    mut watch: ResMut<hd2d::LookWatch>,
) {
    hd2d::reload_look(&mut look, &mut watch);
    hd2d::maybe_screenshot(&mut commands);
}












#[cfg(test)]
mod tests {
    use super::*;

    fn cv(id: &str, is_player: bool, hp: i32, max_hp: i32, statuses: &[&str]) -> CombatantView {
        CombatantView {
            id: id.into(),
            name: id.into(),
            hp,
            max_hp,
            gauge: 0.0,
            is_player,
            player_id: is_player.then(|| id.into()),
            level: 5,
            statuses: statuses.iter().map(|s| s.to_string()).collect(),
        }
    }

    /// A battle with two heroes we control, a co-op ally who joined (not in `your_ids`),
    /// and two enemies of differing health.
    fn battle() -> BattleData {
        BattleData {
            your_ids: vec!["h1".into(), "h2".into()],
            combatants: vec![
                cv("h1", true, 40, 40, &["class:explorer"]),
                cv("h2", true, 12, 40, &["class:resonant"]), // most wounded ally
                cv("ally", true, 30, 40, &["class:explorer"]), // joined co-op hero
                cv("m1", false, 100, 100, &["faction:beast"]),
                cv("m2", false, 40, 100, &["faction:beast"]),
            ],
            ..Default::default()
        }
    }

    #[test]
    fn order_side_routes_targets_by_action() {
        assert_eq!(order_side(QueuedKind::Attack), Some(Side::Enemy));
        assert_eq!(order_side(QueuedKind::Skill("power_strike")), Some(Side::Enemy));
        assert_eq!(order_side(QueuedKind::Skill("transfuse")), Some(Side::Ally));
        assert_eq!(order_side(QueuedKind::Item("salve")), Some(Side::Ally));
        assert_eq!(order_side(QueuedKind::Defend), None);
        assert_eq!(order_side(QueuedKind::Skill("second_wind")), None);
        // Kinetic Aegis wards the caster; other Foci are aimed at an enemy.
        assert_eq!(order_side(QueuedKind::Focus("cast", "kinetic_aegis")), None);
        assert_eq!(order_side(QueuedKind::Focus("cast", "gravity_well")), Some(Side::Enemy));
        assert_eq!(order_side(QueuedKind::Focus("reinforce", "mind_spike")), Some(Side::Enemy));
        assert_eq!(order_side(QueuedKind::Focus("revoke", "gravity_well")), None);
        assert_eq!(order_side(QueuedKind::Hold), None);
    }

    #[test]
    fn valid_targets_split_by_side_and_include_joined_allies() {
        let b = battle();
        let enemies: Vec<String> = valid_targets(&b, Side::Enemy).into_iter().map(|(_, id)| id).collect();
        assert_eq!(enemies, vec!["m1", "m2"], "enemies only");
        let allies: Vec<String> = valid_targets(&b, Side::Ally).into_iter().map(|(_, id)| id).collect();
        // The joined co-op hero "ally" (absent from your_ids) is still targetable.
        assert_eq!(allies, vec!["h1", "h2", "ally"], "all living player combatants");
    }

    #[test]
    fn default_target_picks_first_enemy_or_most_wounded_ally() {
        let b = battle();
        assert_eq!(default_target(&b, QueuedKind::Attack).as_deref(), Some("m1"));
        // Transfuse auto-aims at the lowest-HP-fraction ally (h2 at 12/40).
        assert_eq!(default_target(&b, QueuedKind::Skill("transfuse")).as_deref(), Some("h2"));
        assert_eq!(default_target(&b, QueuedKind::Defend), None);
    }

    #[test]
    fn manifest_verb_reinforces_an_active_focus_else_casts() {
        let mut b = battle();
        // Give h1 an active gravity_well focus.
        b.combatants[0].statuses = vec!["class:psyker".into(), "focus:gravity_well:1".into()];
        assert_eq!(manifest_verb(&b, "h1", "gravity_well"), "reinforce");
        assert_eq!(manifest_verb(&b, "h1", "mind_spike"), "cast");
    }
}

#[cfg(test)]
mod potion_menu_tests {
    use super::*;

    fn held(pairs: &[(&str, i32)]) -> Vec<(String, i32)> {
        pairs.iter().map(|(k, q)| ((*k).to_string(), *q)).collect()
    }

    #[test]
    fn the_items_page_offers_only_potions_the_party_carries() {
        // Nothing held: one dead row plus Back, so the page cannot offer a potion
        // the server would refuse with "Out of …".
        let empty = menu_entries(MenuLevel::Items, "explorer", 5, &[]);
        assert_eq!(empty.len(), 2, "{:?}", empty.iter().map(|e| &e.label).collect::<Vec<_>>());
        assert!(empty[0].label.contains("no potions"));

        // Held potions appear with counts, in registry order.
        let rows = menu_entries(
            MenuLevel::Items,
            "explorer",
            5,
            &held(&[("bloom_salve", 3), ("bulwark_tonic", 1)]),
        );
        let labels: Vec<&str> = rows.iter().map(|e| e.label.as_str()).collect();
        assert_eq!(labels, vec!["Bloom Salve x3", "Bulwark Tonic x1", "Back"]);
        // The row carries the REGISTRY key, so the server recognises what it is sent.
        assert!(matches!(rows[0].action, EntryAction::Item("bloom_salve")));

        // Materials and the Town Portal are not drinkable, so they never appear.
        let rows = menu_entries(
            MenuLevel::Items,
            "explorer",
            5,
            &held(&[("bloom_herb", 9), ("town_portal", 2)]),
        );
        assert!(rows[0].label.contains("no potions"), "{:?}", rows[0].label);

        // A zero stack is not an offer.
        let rows = menu_entries(MenuLevel::Items, "explorer", 5, &held(&[("elixir", 0)]));
        assert!(rows[0].label.contains("no potions"));
    }
}
