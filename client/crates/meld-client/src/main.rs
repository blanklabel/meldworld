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

// Two clippy lints fire on essentially every Bevy system in this crate and neither is
// telling us anything: a system's parameters ARE its dependency list (`too_many_arguments`
// counts them), and a `Query` with filters is a type by construction (`type_complexity`).
// Allowed crate-wide and named here rather than sprinkled over ~30 individual items, so
// the rest of clippy's output stays worth reading.
#![allow(clippy::too_many_arguments)]
#![allow(clippy::type_complexity)]

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
mod builder;
mod battle; // ATB command panel, party HUD, 3D arena + camera, per-class kits
mod city; // The Last City hub: districts, plaza, HUD
mod feel; // battle-feel timings/magnitudes, in one runtime-tunable place
mod flags; // launch-time `MELD_*` / `?query` toggles
mod icons; // one icon per item kind: its own sprite if we drew it, else a type glyph
mod menu; // the three-column cascading main menu (nav -> section -> pane)
mod minimap; // the corner map's GROUND: a bevy_ecs_tilemap grid rendered to a texture
mod mocks; // offline screenshot/demo seeds
mod music; // one looping background track per screen (assets/music/*.mp3)
mod netglue; // server messages → state, demo driver, despawn + font install
mod overlays; // inventory/equip/status, gear tooltip, loot report, level-up
mod overworld; // movement/camera, sprite reconciler, terrain, followers, minimap
mod screens; // Join, co-op Lobby, Ended summary
mod tutorial; // onboarding: the town welcome tour + the first-dive briefing
mod tutorial_predive; // the [T] guided dive's own pre-dive welcome + 4-class picker
mod world_render; // asset load + scene setup, biome ground, sky/weather/water
pub(crate) use battle::*;
pub(crate) use city::*;
pub(crate) use feel::*;
pub(crate) use flags::*;
pub(crate) use mocks::*;
pub(crate) use netglue::*;
pub(crate) use menu::*;
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
/// high.
fn raise_open_file_limit() {
    let _ = rlimit::increase_nofile_limit(u64::MAX);
}

/// The window mode at launch: borderless-fullscreen, which is big and readable.
fn default_window_mode() -> bevy::window::WindowMode {
    bevy::window::WindowMode::BorderlessFullscreen(bevy::window::MonitorSelection::Current)
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
                        // Open BIG: borderless-fullscreen so the world + sprites are
                        // readable; the resolution is the windowed fallback.
                        resolution: (1280u32, 800u32).into(),
                        mode: default_window_mode(),
                        ..default()
                    }),
                    ..default()
                }),
        )
        .init_state::<Screen>()
        // The biome-blending ground material (see `GroundBiome`).
        .add_plugins(MaterialPlugin::<GroundMat>::default())
        // Standing water that is a mesh: the maze's pools and Last City's sea. The open
        // ocean is painted by the ground shader instead — out there the depth is analytic.
        .add_plugins(MaterialPlugin::<world_render::WaterMat>::default())
        // The sky: a camera-anchored gradient dome with a sun in it, replacing a single
        // flat `ClearColor` that nothing could meaningfully reflect.
        .add_plugins(MaterialPlugin::<world_render::SkyDome>::default())
        // The corner map's ground. A map is a GRID, and it was being drawn as one
        // absolutely-positioned UI node per cell, respawned every frame.
        .add_plugins(bevy_ecs_tilemap::TilemapPlugin)
        // Daytime sky blue behind the diorama (the fog fades the ground into it).
        .insert_resource(ClearColor(Color::srgb(0.53, 0.72, 0.93)))
        .init_resource::<builder::BuildMode>()
        .init_resource::<hd2d::Look>()
        .init_resource::<hd2d::LookWatch>()
        .init_resource::<overworld::CamLift>()
        .init_resource::<minimap::MapView>()
        .insert_non_send(NetRes(net::start(base)))
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
        .insert_resource(Sky::opening(&crate::feel::WorldFeel::from_flags()))
        .init_resource::<Ashfall>()
        .init_resource::<DungeonSceneRes>()
        .init_resource::<MoveClock>()
        .init_resource::<LoginFocus>()
        .init_resource::<LoginBg>()
        .init_resource::<BattleMenu>()
        .init_resource::<BattleCam>()
        .init_resource::<PartyView>()
        .insert_resource(BattleFeel::from_flags())
        .insert_resource(crate::feel::WorldFeel::from_flags())
        .init_resource::<HitFx>()
        .init_resource::<AtbFlash>()
        .init_resource::<AllyPanel>()
        .init_resource::<Overlay>()
        .init_resource::<OwInterp>()
        .init_resource::<OverlayTab>()
        .init_resource::<MainMenu>()
        .init_resource::<EquipSelection>()
        .init_resource::<EquipPicker>()
        .init_resource::<OverlayCursor>()
        .init_resource::<InventoryData>()
        .init_resource::<RunGearData>()
        .init_resource::<ProgressData>()
        .init_resource::<AccountHeroNames>()
        .init_resource::<VanguardBoardData>()
        .init_resource::<HuntBoardData>()
        .init_resource::<BountyData>()
        .init_resource::<ShopData>()
        .init_resource::<Notice>()
        .init_resource::<CraftData>()
        .init_resource::<ShopSelling>()
        .init_resource::<CounterPick>()
        .init_resource::<overworld::HurtWash>()
        .init_resource::<ExploredMap>()
        .init_resource::<StationUi>()
        .init_resource::<HeatUi>()
        .init_resource::<Overworld>()
        .init_resource::<RunBackpack>()
        .init_resource::<RunStats>()
        .init_resource::<ShiftTell>()
        .init_resource::<WorldPath>()
        .init_resource::<WorldWeb>()
        .init_resource::<Terrain>()
        .init_resource::<PartyRoster>()
        .init_resource::<PerksRes>()
        .init_resource::<LevelUpQueue>()
        .init_resource::<UnlocksRes>()
        .init_resource::<Tutorial>()
        .init_resource::<TutorialRun>()
        .init_resource::<tutorial_predive::TutorialPreDive>()
        .init_resource::<LoadoutData>()
        .init_resource::<WorldFrame>()
        .init_resource::<HeroRename>()
        .init_resource::<HarvestPops>()
        .init_resource::<Steer>()
        .init_resource::<TapTarget>()
        .init_resource::<Joystick>()
        .init_resource::<BattleData>()
        .init_resource::<BattleTarget>()
        .init_resource::<EndInfo>()
        .init_resource::<CityUi>()
        .init_resource::<LobbyData>()
        .init_resource::<LootReport>()
        .init_resource::<GearHold>()
        .add_systems(
            Startup,
            (setup, load_ui_font, apply_class_flag, mock_battle_setup, mock_overlay_setup, ambient::setup_ambient, music::setup_music, minimap::setup),
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
                (world_render::anchor_sky_dome, world_render::drive_snow),
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
            ),
        )
        // Join
        .add_systems(OnEnter(Screen::Join), (join_ui, fetch_join_board))
        .add_systems(OnExit(Screen::Join), (despawn::<JoinRoot>, login_bg_unload))
        .add_systems(
            Update,
            (
                join_field_click,
                join_input,
                join_login_refresh,
                join_board_refresh,
                login_bg_play,
                login_bg_fit,
            )
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
            (
                despawn::<CityRoot>,
                despawn::<CityScene>,
                despawn::<tutorial_predive::TutorialPreDiveRoot>,
            ),
        )
        .add_systems(
            Update,
            (
                city_move,
                city_interact,
                city_camera,
                city::seed_party_from_account,
                city::prompt_party_if_unset,
                city::party_panel,
                city::party_panel_buttons,
                city::loadout_buttons,
                city::loadout_name_input,
                city::yard_rename_input,
                city::party_panel_refresh,
                (
                    city::render_travel_column,
                    city::travel_click,
                    city::travel_keys,
                    city::render_counter_panel,
                    city::counter_click,
                    city::render_district_nameplates,
                ),
                city_input,
                // The anvil's heat is struck in town as well as in the field.
                (heat_input, update_heat_bar),
                city_action_buttons,
                render_city,
                pulse_magitech,
                hd2d::animate_chars,
                hd2d::place_billboards,
                hd2d::billboard.in_set(hd2d::BillboardSet),
            )
                .run_if(in_state(Screen::City)),
        )
        // Onboarding: the town welcome tour. A separate call rather than folded into
        // the City tuple above, which is already at Bevy's practical flat-tuple ceiling.
        .add_systems(
            Update,
            (
                tutorial::render_town_tour,
                tutorial::tour_buttons,
                tutorial::tour_keyboard,
                tutorial::tour_checkbox_click,
                // Styles the real tap-action bar in place; lives in city.rs since it
                // touches that module's own `TapActionBar` marker.
                city::highlight_tap_action_bar,
            )
                .run_if(in_state(Screen::City)),
        )
        // Onboarding: the [T]-dive's own pre-dive welcome + 4-class picker.
        .add_systems(
            Update,
            (tutorial_predive::render_tutorial_predive, tutorial_predive::tutorial_predive_buttons)
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
                overworld::spawn_blind_mask,
                overworld::spawn_hurt_flash,
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
                // The guided-dive walkthrough's own UI is only ever managed by
                // systems gated to Overworld — despawn explicitly on the way out
                // (to Battle, or back to the City/Ended screens) rather than
                // leaving an orphaned card no longer-running system will clear.
                despawn::<tutorial::TutorialCaptionRoot>,
                despawn::<tutorial::TutorialExplainRoot>,
                despawn::<tutorial::ChestExplainRoot>,
                despawn::<tutorial::ExitTutorialRoot>,
            ),
        )
        .add_systems(
            Update,
            (
                overlay_input,
                overworld_input,
                (overworld_click_menu, psyker_hold_click),
                overworld_camera_control,
                gather_steer,
                emit_move,
                joystick_visual,
                touch_action_buttons,
                (action_hud_tap, action_hud_boon_tap, action_hud_watch_tap),
                sync_overworld_sprites,
                // Dotted trail overlays retired — the terrain itself will convey routes
                // once the continuous heightmap lands (natural valleys/ridges, DQ3-style).
                // (draw_path_trail, draw_web_trail)
                build_terrain_sections,
                world_render::manage_dungeon_scene,
                hd2d::animate_chars,
                hd2d_follow,
                hd2d::place_billboards,
                hd2d::billboard.in_set(hd2d::BillboardSet),
                // Both wind leans compose onto the yaw `hd2d::billboard` writes, so both
                // have to read a yaw that is already there.
                animate_sway.after(hd2d::BillboardSet),
                ambient::update_ambient_scatter.after(hd2d::BillboardSet),
                (update_overworld_hud, update_run_stats, update_action_hud),
                render_overlay,
            )
                .run_if(in_state(Screen::Overworld)),
        )
        // Onboarding: the first-dive briefing.
        .add_systems(
            Update,
            tutorial::first_run_popup.run_if(in_state(Screen::Overworld)),
        )
        // Onboarding: the [T]-dive step-by-step walkthrough. The caption card is
        // Overworld-only — once a fight actually starts, the highlighted Attack
        // tile in the command menu is the coaching, and the caption would only
        // be noise sitting over the battle UI.
        .add_systems(
            Update,
            tutorial::render_tutorial_caption.run_if(in_state(Screen::Overworld)),
        )
        .add_systems(
            Update,
            (
                tutorial::dungeon_explain_card,
                tutorial::chest_explain_card,
                tutorial::render_exit_tutorial_button,
                tutorial::exit_tutorial_click,
            )
                .run_if(in_state(Screen::Overworld)),
        )
        .add_systems(
            Update,
            (
                gear_click,
                formation_click,
                main_menu_input,
                withdraw_click,
                render_loot_report,
                mocks::mock_tally_setup,
                menu::render_main_menu,
                main_menu_click,
                use_item_click,
                // The Map column's two explicit choices, grouped so this tuple stays
                // inside Bevy's per-call system-tuple arity limit.
                (
                    return_to_town_click,
                    build_station_click,
                    menu::build_structure_click,
                    // BD-9 builder mode: the armed tool's own input and its ghost. Beside
                    // the row that arms it, so "click the row, then aim" is one place.
                    builder::builder_input,
                    builder::draw_ghosts,
                    menu::equip_best_click,
                ),
                build_world_walls,
                sync_chests,
                // Grouped: Bevy's system tuples cap at 20 elements.
                (pulse_collectibles, update_reach_halo),
                // Overworld class perks ("party sense").
                update_explorer_lamp,
                update_mob_nameplates,
                (minimap::track_map_view, minimap::repaint, update_minimap, update_minimap_distance),
                (remember_explored, station_input, heat_input, update_heat_bar),
                overworld::update_blind_mask,
                overworld::update_hurt_flash,
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
                // Before the render, so a hold that crosses the threshold this frame opens
                // the panel on THIS frame rather than the next one.
                track_gear_hold,
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
        //
        // `render_main_menu` and its input HAVE to be here too. `render_overlay`'s Inventory
        // arm is empty — the three-column cascade moved into `menu.rs` — so with only
        // `render_overlay` registered, pressing [V] at the Vault-Deep set `overlay.kind` and
        // drew absolutely nothing. The vault looked like it would not open.
        .add_systems(
            Update,
            (
                overlay_input,
                render_overlay,
                menu::render_main_menu,
                main_menu_input,
                main_menu_click,
                menu::equip_best_click,
                gear_click,
                formation_click,
                withdraw_click,
                category_button_click,
                picker_unequip_click,
                picker_back_click,
                // Before the render, so a hold that crosses the threshold this frame opens
                // the panel on THIS frame rather than the next one.
                track_gear_hold,
                render_gear_tooltip,
            )
                .run_if(in_state(Screen::City)),
        )
        // Battle
        .add_systems(
            OnEnter(Screen::Battle),
            (clear_overworld_sprites, hide_field_decor, despawn::<PartyFollower>, enter_battle),
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
                despawn::<BattleIntroRoot>,
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
                    hd2d::billboard.in_set(hd2d::BillboardSet),
                    render_status_icons,
                    update_condition_rims,
                    // The fight's own results screen — drawn here, over the battle
                    // it belongs to, and it is what returns you to the overworld.
                    render_loot_report,
                    render_watch_banner,
                    watch_keyboard,
                    mocks::mock_tally_setup,
                ),
            )
                .run_if(in_state(Screen::Battle)),
        )
        // Onboarding: the guided [T]-dive's first-fight command-menu walkthrough.
        // A separate call rather than folded into the Battle tuple above, which is
        // already nested once to stay under Bevy's arity cap.
        .add_systems(
            Update,
            (battle_intro_card, battle_intro_buttons, battle_intro_keyboard)
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
        .add_plugins(announce_plugin)
        .run();
}

// ------------------------------------------------------------- announces ---

/// The level-up stat screen and the CL-1 unlock banner, and the states they may
/// be seen on.
///
/// Registered only under `Screen::Overworld`, the node they spawn outlives the
/// state change (nothing despawns it on exit) while the system that reads
/// [Space] stops running. So the banner you earn by DYING — the Resonant, the
/// first unlock most accounts ever see — rode Ended into The Last City and sat
/// over the plaza forever, deaf to every key. A screen that must be dismissed
/// runs wherever it can be seen, and is torn down on entering a state that does
/// not draw it. `current` survives that teardown, so a banner interrupted by a
/// fight is re-shown afterwards rather than swallowed.
fn announce_plugin(app: &mut App) {
    app.add_systems(
        Update,
        (level_up_screen, unlock_banner).run_if(
            in_state(Screen::Overworld)
                .or_else(in_state(Screen::City))
                .or_else(in_state(Screen::Ended)),
        ),
    )
    .add_systems(
        OnEnter(Screen::Battle),
        (despawn::<LevelUpRoot>, despawn::<UnlockBannerRoot>),
    )
    .add_systems(
        OnEnter(Screen::Join),
        (despawn::<LevelUpRoot>, despawn::<UnlockBannerRoot>),
    );
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

/// Non-send: the socket handle isn't `Send`, so Bevy runs the systems that touch it
/// on the main thread.
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
    /// Milliseconds one fill of the channel bar takes (from `run.channel_started`).
    /// 0 = nothing to draw.
    channel_fill_ms: u64,
    status: String,
    /// The party — one class key per hero slot (wire form: "explorer" / "psyker" /
    /// "resonant"). Sent on enter_maze. Chosen in TOWN, not at login.
    party: Vec<String>,
    /// PG-2: the departure hub to dive from — a `meld_proto::hubs` key, or `None` for the
    /// deepest one this account has reached. The server clamps it against the account's own
    /// record either way, so this is a preference and never an authority.
    hub: Option<String>,
    /// True once this party came from somewhere real — the account's persisted
    /// composition or the player's own pick — rather than the newcomer default. Town
    /// prompts only when it is false, so a returning player is never re-asked.
    party_chosen: bool,
    /// `?party=` / `MELD_PARTY` pinned the composition, so nothing may overwrite it —
    /// the screenshot and autoplay harnesses depend on getting exactly what they asked
    /// for.
    party_from_flags: bool,
    /// Which party slot the builder cursor is on.
    party_cursor: usize,
    /// True if the player chose Co-op at Join (go to the lobby after connecting
    /// instead of diving solo).
    coop: bool,
}

impl Default for Session {
    fn default() -> Self {
        Session {
            hub: None,
            player_id: String::new(),
            username: String::new(),
            password: String::new(),
            connecting: false,
            entered: false,
            channeling: false,
            channel_fill_ms: 0,
            status: String::new(),
            // A diverse default so newcomers see a spread of classes at once.
            party: vec![
                "explorer".into(),
                "psyker".into(),
                "resonant".into(),
                "explorer".into(),
            ],
            party_cursor: 0,
            party_chosen: false,
            party_from_flags: false,
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
    /// True for a creature trading blows with another right now (`CR-2`). Drives the
    /// same ⚔ marker a fighting player wears — a fight is a fight — plus an HP bar
    /// that is NOT perk-gated: you can watch a brawl in front of you without a Hunter.
    clashing: bool,
    /// Elevation level (terraced verticality); render height rises by `level*STEP_HEIGHT`.
    level: u8,
    /// For chests: whether it's been opened.
    opened: bool,
    /// For chests: the treasure tier, which decides how good the loot is and therefore
    /// what the chest should look like. It rode the wire from the day chests existed and
    /// was thrown away here, so every chest drew as the common brown box.
    chest_tier: i32,
    /// Overworld mob intel (monsters only; `None` otherwise). Rendered as a
    /// nameplate only when the viewer's Explorer/Psyker perk unlocks each field.
    mob_level: Option<i32>,
    hp: Option<i32>,
    max_hp: Option<i32>,
    encounter_class: Option<String>,
    aggression: Option<String>,
    /// The quarry of a hunt this player is working (AD-4) — server-decided, per-viewer.
    quarry: bool,
    /// Pinned by a Psyker right now (CL-2): it cannot move, and engaging it opens the
    /// fight with the whole party's gauges full.
    held: bool,
    /// FS-4: the named boss this creature IS (`ironmaw`, `choirmother`, …), or `None` for
    /// ordinary fauna. A boss overlays a host creature, so `name` is the wildlife it rode
    /// in on — this is what it fights as, renders as, and is titled as.
    boss: Option<String>,
    /// Dungeon entrances: heroes the doors inside want on plates at once (1 = solo).
    bodies_required: u8,
    /// `FS-4`: how many PARTIES this fight is sized for, when more than one. 0 on anything
    /// ordinary, so the plate is only ever drawn where it is a warning.
    expects_parties: u8,
}

impl OwEntity {
    fn player(x: f32, y: f32) -> Self {
        Self { x, y, kind: EntityKind::Player, name: None, faction: None, radius: 0.0, battling: false, clashing: false, level: 0, opened: false, chest_tier: 0, mob_level: None, hp: None, max_hp: None, encounter_class: None, aggression: None, quarry: false, held: false, boss: None, bodies_required: 1, expects_parties: 0 }
    }
    fn monster(x: f32, y: f32, name: &str, faction: &str) -> Self {
        Self {
            x,
            y,
            expects_parties: 0,
            kind: EntityKind::Monster,
            name: Some(name.to_string()),
            faction: Some(faction.to_string()),
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
        }
    }
    fn portal(x: f32, y: f32) -> Self {
        Self { x, y, kind: EntityKind::Portal, name: None, faction: None, radius: 0.0, battling: false, clashing: false, level: 0, opened: false, chest_tier: 0, mob_level: None, hp: None, max_hp: None, encounter_class: None, aggression: None, quarry: false, held: false, boss: None, bodies_required: 1, expects_parties: 0 }
    }
}

#[derive(Resource, Default)]
struct Overworld {
    /// entity id -> its render state
    entities: HashMap<String, OwEntity>,
    /// An authoritative teleport the local avatar must SNAP to rather than chase. The
    /// local player is deliberately exempt from interpolation (the camera rides its
    /// transform), so without this a Shift that walks you back to the region entry
    /// renders as a one-second slide through everything in between.
    snap: Option<(f32, f32)>,
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
    /// Each hero's pouch by slot — what that hero can reach in a fight. Separate from
    /// `items`, which is the Party Inventory and out of reach mid-battle.
    pouches: Vec<Vec<(String, i32)>>,
    /// Slots one pouch holds, so a row can read `3/10`.
    pouch_capacity: i32,
}

impl RunBackpack {
    fn count(&self, kind: &str) -> i32 {
        self.items.iter().find(|(k, _)| k == kind).map_or(0, |(_, q)| *q)
    }

    /// What hero `slot` is carrying, in registry order.
    fn pouch(&self, slot: usize) -> &[(String, i32)] {
        self.pouches.get(slot).map(|p| p.as_slice()).unwrap_or(&[])
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

/// The Shift's tell (CANON D20/§W2), as the client needs it: which radius ring is
/// doomed, when it goes, and what it is about to become.
///
/// One resource drives all three surfaces — the burning annulus on the ground shader,
/// the HUD countdown, and the flash when it lands — so they cannot disagree about which
/// ring is in danger. Radii are in world units from the hub, which is the frame the
/// ground is already painted in.
#[derive(Resource, Default)]
struct ShiftTell {
    inner: f32,
    outer: f32,
    /// Client clock (seconds) the region swaps. Past it, the ring is history.
    lands_at: f64,
    /// What it becomes, for the countdown line.
    biome: String,
    /// Whether the local party is standing in it — the HUD shouts rather than mentions.
    caught: bool,
    /// Client clock until which the landing flash is still burning down.
    flash_until: f64,
    armed: bool,
}

impl ShiftTell {
    /// How hard the ground burns right now: a throb that quickens as the deadline
    /// closes, then a flash on the swap that decays to nothing. 0 = resting.
    fn intensity(&self, now: f64) -> f32 {
        if now < self.flash_until {
            let left = ((self.flash_until - now) / SHIFT_FLASH_SECS).clamp(0.0, 1.0);
            return left as f32;
        }
        if !self.armed || now >= self.lands_at {
            return 0.0;
        }
        let left = (self.lands_at - now).max(0.0);
        // Urgency rides how little time is left, not how long the window was: a 10 s tell
        // and a 30 s one should both be frantic in their last second.
        let urgency = (1.0 - (left / 10.0).clamp(0.0, 1.0)) as f32;
        let throb = ((now * (3.0 + 9.0 * urgency as f64)).sin() as f32) * 0.5 + 0.5;
        0.18 + 0.62 * urgency * (0.45 + 0.55 * throb)
    }
}

/// How long the landing flash burns down over.
const SHIFT_FLASH_SECS: f64 = 1.1;

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
    /// Ability key → the one-line magnitudes the server resolved from balance. The
    /// registry's prose says what KIND of thing an ability is; a `[TUNABLE]` lives on
    /// the server, so without this a row could name Adrenaline and never its cost.
    ability_effects: HashMap<String, String>,
    /// Ability key → Adrenaline cost, Hunter skills only. What the battle menu gates
    /// a row's `enabled` on — see `skill_entries`.
    ability_costs: HashMap<String, i32>,
}

impl PartyRoster {
    /// The magnitudes for `key`, or empty until the server has sent a roster.
    fn effect(&self, key: &str) -> &str {
        self.ability_effects.get(key).map(String::as_str).unwrap_or("")
    }

    /// `key`'s Adrenaline cost, if it has one (Hunter skills only).
    fn adrenaline_cost(&self, key: &str) -> Option<i32> {
        self.ability_costs.get(key).copied()
    }
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
    /// PG-2: the account's all-time deepest distance — the Vanguard Wall's own number.
    /// It no longer gates a departure point: the authored deep hubs are retired and a
    /// `BD-5` forward town is its own proof you stood there (`meld_proto::hubs`).
    deepest_ever: i32,
    pending: std::collections::VecDeque<meld_client::net::UnlockLine>,
    current: Option<meld_client::net::UnlockLine>,
    elapsed: f32,
    /// Offline demo/screenshot: hold the banner until [Space] instead of letting it
    /// time out, the same way the level-up screen does.
    hold: bool,
}

/// Ask for the season's board as the login screen opens — it is public, so this
/// works before anyone has authenticated.
fn fetch_join_board(net: NonSend<NetRes>) {
    net.0.fetch_vanguard();
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
    tutorial: ResMut<'w, Tutorial>,
    tutorial_run: ResMut<'w, TutorialRun>,
}

/// Per-account onboarding popups: the town welcome tour and the first-dive
/// briefing. Both gate on `loaded` — until the post-connect server sync
/// (`onboarding.status`) lands, neither popup may render, or a RETURNING
/// player would flash the first-timer UI during the race window before their
/// real flags arrive.
#[derive(Resource, Default)]
struct Tutorial {
    /// True once `onboarding.status` has been received at least once this session.
    loaded: bool,
    town_seen: bool,
    run_seen: bool,
    /// Town tour step index while it's actively showing (0..=3); `None` = not
    /// currently running (either finished/skipped, or not yet started/loaded).
    town_step: Option<u8>,
    /// The "don't show this tutorial again" checkbox state, live while step 0
    /// is showing.
    skip_checked: bool,
    /// One-shot arm flag for the first-dive briefing: set by the `RunStarted`
    /// handler the moment a fresh run is confirmed, cleared once the briefing
    /// has been shown/dismissed so re-entering Overworld later doesn't re-trigger it.
    show_run_popup: bool,
}

/// The `[T]` guided practice dive's step-by-step walkthrough. Unlike `Tutorial`
/// above, this is never persisted — a `[T]` dive is a deliberately repeatable
/// practice action, so the walkthrough re-arms fresh every time one begins and
/// lives purely in client memory.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum TutorialStep {
    /// Covers both harvesting the starter node and opening the starter chest —
    /// see `TutorialRun::harvested`/`chest_opened` for which of the two is owed.
    Harvest,
    Fight,
    Dungeon,
    /// The "What is a Dungeon?" card is up — expanded to also BE the
    /// walkthrough's completion moment (Keep going / Go back to town), so
    /// there is no separate "Done" step stacked after it.
    Explain,
}

/// The first tutorial battle's paced, one-at-a-time Attack/Defend/Skill/Flee
/// explainer — a sub-state of `TutorialStep::Fight`. Order matches the d-pad
/// cross's own layout (`rebuild_command_menu`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum BattleIntroStep {
    Attack,
    Defend,
    Skill,
    Flee,
}

#[derive(Resource, Default)]
struct TutorialRun {
    /// `None` = no guided walkthrough active (a normal dive, or not yet armed).
    ///
    /// Armed from `run.started.tutorial` — the WORLD's own answer — and never from the
    /// `[T]` keypress. A `pending_arm` intent flag used to live here and was cleared only
    /// when a dive actually started, so a refused `[T]` dive stayed armed and put a
    /// walkthrough over the player's next randomized run.
    step: Option<TutorialStep>,
    harvested: bool,
    chest_opened: bool,
    /// `Some` while the first fight's paced command-menu explainer is showing
    /// (and swallowing normal battle input); `None` once it's finished, skipped,
    /// or this isn't the tutorial's first fight.
    battle_intro: Option<BattleIntroStep>,
    /// True while the "What's in a Chest?" explainer is actually showing.
    chest_explain: bool,
    /// True once that explainer has been shown at all — never re-armed, so a
    /// LATER chest (e.g. the dungeon's own loot chest) doesn't show it again.
    chest_explained: bool,
}

impl TutorialRun {
    /// Arm the Fight step and its battle-intro walkthrough together — the two
    /// always begin in lockstep, so every call site sets them as one unit.
    fn arm_fight(&mut self) {
        self.step = Some(TutorialStep::Fight);
        self.battle_intro = Some(BattleIntroStep::Attack);
    }
}

/// Marker for spawned path-trail dots (despawned when the path changes).
#[derive(Component)]
struct PathTrail;

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
    /// This fight is being WATCHED, not fought (`SOC-3`). No command menu, no target
    /// picker, no loot report: every one of those is an answer to "what do I do", and a
    /// watcher's answer is "nothing, that is the point".
    spectating: bool,
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
        // Ask the REGISTRY who an ability is aimed at. This used to be a list of keys
        // here, and it had gone stale without anyone noticing: it still named the Iron
        // Hull's `root` and `toll_of_the_deep`, so the Phoenix Guard's self-cast Rite of
        // Rest and its all-enemy Purging Light both fell through the default arm and
        // asked the player to aim a stance at one creature. Every party-wide row added
        // since had the same problem.
        QueuedKind::Skill(k) => match meld_proto::skills::target_of(k) {
            meld_proto::skills::Target::Enemy => Some(Side::Enemy),
            meld_proto::skills::Target::Ally => Some(Side::Ally),
            _ => None,
        },
        QueuedKind::Item(_) => Some(Side::Ally),
        QueuedKind::Defend => None,
        // Psyker Foci: Kinetic Aegis wards the caster (self); the rest are aimed at an
        // enemy. Revoke/Hold need no target.
        QueuedKind::Focus("cast", f) | QueuedKind::Focus("reinforce", f) => {
            match meld_proto::skills::target_of(f) {
                meld_proto::skills::Target::Enemy => Some(Side::Enemy),
                meld_proto::skills::Target::Ally => Some(Side::Ally),
                _ => None,
            }
        }
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
    /// Party slot of the active hero. `your_ids` is in party order and the server builds
    /// its combatant list the same way, so this index is the hero's pouch index too.
    fn active_slot(&self) -> usize {
        self.active
            .as_ref()
            .and_then(|a| self.your_ids.iter().position(|id| id == a))
            .unwrap_or(0)
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
#[derive(Clone, Copy, PartialEq, Debug)]
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

/// The Psyker's manifestations, read from the shared registry rather than listed
/// here: a hand-kept copy silently stops offering whatever the server learned to
/// resolve, which is exactly what happened when the ladder grew past four.
fn manifests() -> Vec<&'static meld_proto::skills::SkillDef> {
    meld_proto::skills::skills_for_class("psyker")
}

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

/// Hunter kit catalog for AUTOPLAY: (wire kind, Adrenaline cost), a mirror of balance
/// `[battle] hunter_*_cost`. The unlock level is NOT written here — it comes from
/// `meld_proto::skills::unlock_level`, the same registry the server gates on.
///
/// ⚠️ It used to carry its own levels, and they had gone stale against the round `RUNGS`:
/// `second_wind` 2 (really 5), `snare` 2 (really 10), `frenzy` 3 (really 20). Autoplay
/// does not go through `menu_entries`, so nothing greyed those rows — it simply submitted
/// a skill the hero had not learned, the server refused it, and (before the turn-back fix
/// in `handle_submit`) the hero was uncommandable until the 15 s auto-defend. A Hunter on
/// autoplay spent most of its fight locked out by its own kit. A hand-written level beside
/// a registry that owns levels is a copy that will go stale; this is the repo's "never a
/// list of ability keys" rule wearing a tuple.
const HUNTER_SKILLS: [(&str, i32); 4] = [
    ("power_strike", 40),
    ("second_wind", 35),
    ("snare", 30),
    ("frenzy", 80),
];

/// The Adrenaline cost and registry unlock level of a Hunter autoplay row.
fn hunter_skill(kind: &str) -> (i32, i32) {
    let cost = HUNTER_SKILLS
        .iter()
        .find(|(k, _)| *k == kind)
        .map(|(_, c)| *c)
        .unwrap_or(i32::MAX);
    (meld_proto::skills::unlock_level(kind), cost)
}

/// Autoplay heuristic for a Explorer hero: build Adrenaline with basic attacks, then
/// release. Heal with Second Wind when badly hurt and it can afford it; otherwise if
/// it has leveled into Frenzy it banks toward that big hit, else cashes in Power
/// Strike as soon as it can afford it.
fn explorer_autoplay_op(view: &CombatantView) -> QueuedKind {
    let adr = status_num(&view.statuses, "adrenaline:");
    let (sw_lv, sw_cost) = hunter_skill("second_wind");
    let hurt = (view.hp as f32) < 0.4 * view.max_hp.max(1) as f32;
    if hurt && view.level >= sw_lv && adr >= sw_cost {
        return QueuedKind::Skill("second_wind");
    }
    let (frenzy_lv, frenzy_cost) = hunter_skill("frenzy");
    if view.level >= frenzy_lv {
        // Save up, then unleash Frenzy.
        return if adr >= frenzy_cost { QueuedKind::Skill("frenzy") } else { QueuedKind::Attack };
    }
    let (ps_lv, ps_cost) = hunter_skill("power_strike");
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
        for def in manifests() {
            if view.level >= def.unlock && !has(def.key) {
                return QueuedKind::Focus("cast", def.key);
            }
        }
    }
    for (kind, stacks) in &foci {
        if *stacks < 2 {
            if let Some(def) = manifests().into_iter().find(|d| d.key == kind.as_str()) {
                return QueuedKind::Focus("reinforce", def.key);
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

/// The display name for hero slot `i`, from whichever source `hero_count` used.
fn hero_name_at(roster: &PartyRoster, names: &AccountHeroNames, i: usize) -> Option<String> {
    if !roster.heroes.is_empty() {
        roster.heroes.get(i).map(|h| h.name.clone())
    } else {
        names.names.get(i).cloned()
    }
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
                && (g.class_key.is_empty() || hero_class.is_none_or(|c| c == g.class_key))
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
    /// [`BattleFeel::lunge_ttl`]); [`animate_battle_actors`] lunges that sprite.
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
    /// How many live numbers already shared this target when it landed. An all-enemy
    /// ability resolves its whole sweep in one message, so without this every number
    /// would draw at the identical anchor and overstrike into noise.
    stack: u8,
}
/// A monster's ability shout bubble (see `HitFx::callouts`).
struct Callout {
    /// Who shouted. One bubble per speaker, and the row it draws on comes from its
    /// place in the list.
    combatant_id: String,
    text: String,
    age: f32,
    ttl: f32,
    /// Telegraphs flash (channeling); instant callouts just fade.
    flashing: bool,
}
/// Tracks the "your turn!" pop: when a hero newly enters `BattleData::ready`, its
/// id gets a fading flash (see [`advance_atb_flash`]) that [`party_cell`] renders as
/// a quick brighten of the ATB bar + cell border.
#[derive(Resource, Default)]
struct AtbFlash {
    /// Heroes that were ready last frame, to detect the rising edge.
    prev: HashSet<String>,
    /// Active flashes: hero id → seconds elapsed (dropped past [`BattleFeel::atb_flash_ttl`]).
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
    /// True while the Forge & Alembic's recipe book is open (MS-1).
    craft_open: bool,
    /// True while the Vanguard Wall is lit — the board replaces the notice line
    /// until the player walks away or presses [E] again.
    board_open: bool,
    /// True while the Bounty Board's hunts are open (AD-4).
    hunts_open: bool,
    /// Which side of the Bounty Board is facing you: the posted hunts, or the Den's own
    /// contracts. One district, two boards — the same flip the counter uses for buy/sell.
    bounty_tab: bool,
    /// The name being typed for the next loadout save. On `CityUi` rather than the
    /// panel so it survives the panel being rebuilt when the saved list changes.
    loadout_name: String,
    /// True while the Drill Yard's party picker is open (PT: choose the team you
    /// take down). Opens by itself the first time an account reaches town without a
    /// party of its own, so nobody dives with the newcomer default by accident.
    party_open: bool,
    /// Which class the Drill Yard's detail panel is describing — the last one
    /// hovered or clicked. Separate from the party itself, so you can read a class
    /// before deciding to field it.
    yard_focus: String,
}

impl CityUi {
    /// Shut every counter. ONE place, because "close whatever is open" was written out
    /// longhand at each exit and the claims board was missing from one of them — which
    /// left it with no way out at all once travel stopped you having to walk away.
    /// A new panel adds its flag here and every exit learns about it at once.
    pub(crate) fn close_counters(&mut self) {
        self.shop_open = false;
        self.craft_open = false;
        self.board_open = false;
        self.hunts_open = false;
        self.bounty_tab = false;
        self.notice.clear();
    }

    /// Whether any counter currently owns the screen.
    pub(crate) fn any_counter_open(&self) -> bool {
        self.shop_open || self.craft_open || self.board_open || self.hunts_open
    }
}

/// Which way the counter is facing: `false` = what it sells, `true` = what it buys.
/// UI state, so it lives apart from [`ShopData`], which holds only the server's answer.
#[derive(Resource, Default)]
pub(crate) struct ShopSelling(pub bool);

/// The recipe book and the Forge's own selection, for the Forge & Alembic (MS-1).
#[derive(Resource, Default)]
pub(crate) struct CraftData {
    pub recipes: Vec<meld_client::net::RecipeLine>,
    pub loaded: bool,
    /// Which recipe row the cursor sits on.
    pub cursor: usize,
    /// Which equipment slot the Forge half would make.
    pub slot: usize,
    /// Whether the next forge quenches the piece in a trophy.
    pub catalyze: bool,
    /// Which Vault piece is on the bench for the smith's two services (reroll, repair).
    pub bench: usize,
    /// The last thing the workshop said — a made item, or why it refused.
    pub last: String,
}

/// The slots the Forge half cycles through, in loadout order.
pub(crate) const FORGE_SLOTS: [&str; 6] =
    ["main_hand", "off_hand", "head", "chest", "legs", "accessory"];

/// A short-lived line of feedback for something the player just tried and the server
/// refused. With walk-into interactions a refusal could be silent — you simply kept
/// walking — but **[E] is a button**, and a button that does nothing reads as broken.
/// The server already writes good refusals ("The vault is sealed — defeat the boss
/// first."); this is where they get seen.
#[derive(Resource, Default)]
pub(crate) struct Notice {
    pub(crate) text: String,
    /// Client-clock seconds after which it fades.
    pub(crate) until: f64,
}

impl Notice {
    pub(crate) fn say(&mut self, text: impl Into<String>, now: f64) {
        self.text = text.into();
        self.until = now + NOTICE_SECS;
    }
    pub(crate) fn live(&self, now: f64) -> Option<&str> {
        (now < self.until && !self.text.is_empty()).then_some(self.text.as_str())
    }
}

/// How long a refusal stays on screen.
pub(crate) const NOTICE_SECS: f64 = 3.5;

/// The Apothecary's shelf as last read from `GET /v1/vendors/apothecary` (EC-2).
#[derive(Resource, Default)]
pub(crate) struct ShopData {
    pub vendor: String,
    pub items: Vec<meld_client::net::ShopLine>,
    /// The Requisition's plain-gear stock, shown in the same panel: one shop button,
    /// both halves of "spend chits to make the next dive easier" (EC-2).
    pub gear: Vec<meld_client::net::GearShopLine>,
    /// What the Broker pays per material — the SELL side of the same counter (MS-1).
    pub quotes: Vec<meld_client::net::BrokerQuote>,
    pub loaded: bool,
}

/// The live Vanguard Board as last read from `GET /v1/leaderboards/vanguard`
/// (P1-1) — what the Vanguard Wall in Last City displays.
/// PT-2: the account's saved party loadouts, as last read from the server.
#[derive(Resource, Default)]
pub(crate) struct LoadoutData {
    pub list: Vec<meld_client::net::LoadoutLine>,
    pub loaded: bool,
}

/// The Den's bounty board as last read from `GET /v1/bounties` (AD-4) — what the menu's
/// Quests column shows.
#[derive(Resource, Default)]
pub(crate) struct BountyData {
    pub rank: i32,
    pub rank_title: String,
    pub rank_xp_to_next: i64,
    pub active: Vec<meld_client::net::BountyLine>,
    pub history: Vec<meld_client::net::BountyLine>,
    pub loaded: bool,
}

#[derive(Resource, Default)]
pub(crate) struct HuntBoardData {
    pub hunts: Vec<meld_client::net::HuntLine>,
    pub loaded: bool,
    /// Which row the detail column is describing.
    pub cursor: usize,
}

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
    /// What the last fight cost in gear durability, `(hero name, points, ephemeral pieces
    /// burned)` per hero that fell (GR-2). Shown on the DEATH screen above all: a wipe is
    /// where the bill is largest, so it is where the player most needs to see it.
    worn: Vec<(String, i32, Vec<String>)>,
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
    /// `(name, insurance)` per piece. The WORD travels with the name because this card is
    /// where a player decides what to risk next, and an Ephemeral piece that reads as an
    /// ordinary drop is an item they will lose without ever having been told it was
    /// temporary (`GR-6`).
    gear: Vec<(String, meld_proto::enums::Insurance)>,
    /// The COST half of the card: `(hero name, durability points, ephemeral pieces burned)`
    /// per hero that fell. A report that lists only what you gained is a report that hides
    /// the price — and the ephemeral half of that price can be a whole build.
    worn: Vec<(String, i32, Vec<String>)>,
    elapsed: f32,
    /// This report is the end of a FIGHT, so it is shown on the battle screen and
    /// the walk back to the overworld waits for it to be dismissed. The tally for a
    /// fight belongs on the screen the fight happened on, not on top of a world you
    /// are already walking around in.
    gate_return: bool,
}

impl LootReport {
    /// Raise the card, setting EVERY field from scratch.
    ///
    /// The resource outlives the report it last showed, so an arm that sets only the
    /// fields it cares about inherits the rest: a chest opened after a costly fight
    /// showed that fight's durability line over its treasure. A constructor cannot
    /// forget a field the way three call sites can.
    pub(crate) fn raise(
        &mut self,
        title: &str,
        xp: Option<i64>,
        chits: i64,
        items: Vec<(String, i32)>,
        gear: Vec<(String, meld_proto::enums::Insurance)>,
    ) {
        *self = LootReport {
            active: true,
            title: title.to_string(),
            xp,
            chits,
            items,
            gear,
            worn: Vec::new(),
            elapsed: 0.0,
            gate_return: false,
        };
    }
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
/// available.
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

impl EntryAction {
    /// The registry ability this row commands, if it commands one. A Psyker's
    /// Manifest rows are abilities too — they are the whole class.
    fn skill_key(&self) -> Option<&'static str> {
        match self {
            EntryAction::Skill(k) | EntryAction::Manifest(k) => Some(k),
            _ => None,
        }
    }
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
    /// False when picking this row would only get the hero refused (out of
    /// Adrenaline, or a once-per-battle call already spent) — greyed out and
    /// inert rather than submitted and stalled for the rest of the fight, since
    /// the server's refusal never resolves the hero's turn (see `select_entry`).
    enabled: bool,
    /// Hunter skills only: what this row costs in banked Adrenaline, shown as a
    /// right-aligned "N AP" badge so the cost to build toward reads at a glance.
    adrenaline_cost: Option<i32>,
}

/// The class's kit as menu rows, keeping only what the hero has leveled into. Read
/// straight from the shared registry: the name, the order, the unlock level and the
/// tooltip are all one definition (`meld_proto::skills`). `adrenaline` is the active
/// hero's current banked amount, checked against `roster`'s per-skill Hunter costs.
fn skill_entries(
    class: &str,
    hero_level: i32,
    spent: &[String],
    roster: &PartyRoster,
    adrenaline: i32,
) -> Vec<MenuEntry> {
    meld_proto::skills::skills_for_class_at(class, hero_level)
        .into_iter()
        .map(|d| {
            // A once-per-battle call that has been made says so on the row. The server
            // refuses it either way; this is so the player is not left guessing why.
            let gone = meld_proto::skills::is_once_per_battle(d.key)
                && spent.iter().any(|s| s == d.key);
            let cost = roster.adrenaline_cost(d.key);
            let affordable = cost.map(|c| c <= adrenaline).unwrap_or(true);
            MenuEntry {
                label: if gone {
                    format!("{} (spent)", d.name)
                } else {
                    d.name.to_string()
                },
                action: EntryAction::Skill(d.key),
                tooltip: d.description.to_string(),
                enabled: !gone && affordable,
                adrenaline_cost: cost,
            }
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
    // The acting hero's `spent:<skill>` tokens, so a once-per-battle call that is gone
    // says so on its row instead of only being refused when pressed.
    spent: &[String],
    // The Foci that hero is currently holding, which is what makes an ASPECT castable.
    foci: &[String],
    // Hunter Adrenaline costs + the active hero's current banked amount, so a skill
    // it can't afford greys out instead of stalling the hero's turn (`skill_entries`).
    roster: &PartyRoster,
    adrenaline: i32,
) -> Vec<MenuEntry> {
    let e = |label: &str, action| MenuEntry {
        label: label.to_string(),
        action,
        tooltip: String::new(),
        enabled: true,
        adrenaline_cost: None,
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
            let mut v = skill_entries(class, hero_level, spent, roster, adrenaline);
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
                        enabled: true,
                        adrenaline_cost: None,
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
            // Manifestations carry their tooltip like every other ability now. An ASPECT
            // is listed under the Focus it deepens and ONLY while that Focus is held — the
            // engine refuses it otherwise, and a row that costs a turn to be told no is
            // the thing this menu exists to prevent. It also teaches the chain: hold
            // Gravity Well and Gravity appears beneath it.
            fn row(d: &'static meld_proto::skills::SkillDef, depth: usize) -> MenuEntry {
                MenuEntry {
                    label: format!("{}{}", "  ".repeat(depth), d.name),
                    action: EntryAction::Manifest(d.key),
                    tooltip: d.description.to_string(),
                    enabled: true,
                    adrenaline_cost: None,
                }
            }
            fn push_aspects(
                v: &mut Vec<MenuEntry>,
                parent: &str,
                depth: usize,
                hero_level: i32,
                foci: &[String],
            ) {
                if !foci.iter().any(|k| k == parent) {
                    return;
                }
                for a in meld_proto::skills::aspects_of(parent) {
                    if hero_level < a.unlock {
                        continue;
                    }
                    v.push(row(a, depth));
                    push_aspects(v, a.key, depth + 1, hero_level, foci);
                }
            }
            let mut v: Vec<MenuEntry> = Vec::new();
            for d in meld_proto::skills::skills_for_class_at("psyker", hero_level) {
                v.push(row(d, 0));
                push_aspects(&mut v, d.key, 1, hero_level, foci);
            }
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
#[derive(Resource)]
struct PartyView {
    show: bool,
}

impl Default for PartyView {
    fn default() -> Self {
        // ON by default. You command a party, and a lone avatar walking a maze that four
        // heroes are fighting in is the wrong read of what the game is; [P] is there to
        // turn the entourage OFF when it is in the way, not to opt into it.
        PartyView { show: true }
    }
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
        // These four were all answered "pick an enemy" while the targeting list here
        // still named the Iron Hull's keys: a self-cast stance, an all-enemy sweep, a
        // party Barrier and a party haste.
        assert_eq!(order_side(QueuedKind::Skill("rite_of_rest")), None);
        assert_eq!(order_side(QueuedKind::Skill("purging_light")), None);
        assert_eq!(order_side(QueuedKind::Skill("unbroken_vigil")), None);
        assert_eq!(order_side(QueuedKind::Skill("a_world_known")), None);
        // The deep rungs land on the same rule without anyone adding them here.
        assert_eq!(order_side(QueuedKind::Skill("apex_predator")), None);
        assert_eq!(order_side(QueuedKind::Skill("world_tree")), None);
        assert_eq!(order_side(QueuedKind::Skill("assassinate")), Some(Side::Enemy));
        assert_eq!(order_side(QueuedKind::Skill("tempering_blow")), Some(Side::Ally));
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
        let empty =
            menu_entries(MenuLevel::Items, "explorer", 5, &[], &[], &[], &PartyRoster::default(), 0);
        assert_eq!(empty.len(), 2, "{:?}", empty.iter().map(|e| &e.label).collect::<Vec<_>>());
        assert!(empty[0].label.contains("no potions"));

        // Held potions appear with counts, in registry order.
        let rows = menu_entries(
            MenuLevel::Items,
            "explorer",
            5,
            &held(&[("bloom_salve", 3), ("bulwark_tonic", 1)]),
            &[],
            &[],
            &PartyRoster::default(),
            0,
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
            &[],
            &[],
            &PartyRoster::default(),
            0,
        );
        assert!(rows[0].label.contains("no potions"), "{:?}", rows[0].label);

        // A zero stack is not an offer.
        let rows = menu_entries(
            MenuLevel::Items,
            "explorer",
            5,
            &held(&[("elixir", 0)]),
            &[],
            &[],
            &PartyRoster::default(),
            0,
        );
        assert!(rows[0].label.contains("no potions"));
    }

    /// The battle Items page must read the ACTING hero's pouch and nothing else. The
    /// server checks the same pouch, so sourcing this from the Party Inventory would
    /// build a menu out of rows that are guaranteed to be refused — and it would say the
    /// opposite of the rule the two containers exist to express.
    #[test]
    fn the_battle_items_page_reads_the_acting_heros_pouch_not_the_party_inventory() {
        let bp = RunBackpack {
            items: held(&[("elixir", 9), ("bloom_salve", 9)]),
            pouches: vec![
                held(&[("bloom_salve", 2)]),
                vec![],
                held(&[("ghostdust", 1)]),
            ],
            pouch_capacity: 10,
            ..Default::default()
        };
        assert_eq!(crate::battle::held_potions(&bp, 0), held(&[("bloom_salve", 2)]));
        // An empty pouch offers nothing, however full the shared inventory is — this is
        // the case that makes running dry on the wrong hero a real outcome.
        assert!(crate::battle::held_potions(&bp, 1).is_empty());
        assert_eq!(crate::battle::held_potions(&bp, 2), held(&[("ghostdust", 1)]));
        // A hero slot with no pouch at all must read as empty rather than panic.
        assert!(crate::battle::held_potions(&bp, 7).is_empty());
        // The 9 elixirs in the Party Inventory are reachable from NO pouch.
        for slot in 0..4 {
            assert!(
                !crate::battle::held_potions(&bp, slot).iter().any(|(k, _)| k == "elixir"),
                "slot {slot} could reach the shared inventory"
            );
        }
    }

    /// A once-per-battle call that has been spent says so on its own row. The server
    /// refuses it either way, but a row that looks available and then refuses is the same
    /// "the rule exists and the screen never says so" problem as an invisible status.
    #[test]
    fn a_spent_once_per_battle_row_says_so() {
        let lvl = meld_proto::skills::unlock_level("now");
        let fresh = menu_entries(
            MenuLevel::Skills,
            "explorer",
            lvl,
            &[],
            &[],
            &[],
            &PartyRoster::default(),
            0,
        );
        let now = fresh.iter().find(|e| matches!(e.action, EntryAction::Skill("now")));
        assert!(now.is_some(), "a Globemaster should be offered Now");
        assert_eq!(now.unwrap().label, "Now");
        assert!(now.unwrap().enabled, "an unspent, affordable row must stay enabled");

        let after = menu_entries(
            MenuLevel::Skills,
            "explorer",
            lvl,
            &[],
            &["now".to_string()],
            &[],
            &PartyRoster::default(),
            0,
        );
        let now = after
            .iter()
            .find(|e| matches!(e.action, EntryAction::Skill("now")))
            .expect("the row stays visible");
        assert!(now.label.contains("spent"), "a used call should read as used: {}", now.label);
        assert!(!now.enabled, "a spent once-per-battle row must be inert, not just re-labelled");

        // An ability that is not once-per-battle is untouched by the same token.
        let tb = after
            .iter()
            .find(|e| matches!(e.action, EntryAction::Skill("trailblaze")))
            .expect("Trailblaze is still there");
        assert_eq!(tb.label, "Trailblaze");
    }
}

/// One "+1 Bog Myrrh" that just landed, floating up over the player's head.
///
/// A harvest channel pays out per tick, and the payout was invisible: the bar filled, the
/// stock arrived in a panel you were not looking at, and nothing on screen said a unit had
/// been banked. The whole point of paying per tick is that you can SEE it paying.
pub(crate) struct HarvestPop {
    /// The item kind, kept beside the words so the floater can wear the material's own
    /// sprite: "+1 Bog Myrrh" beside a picture of the bush you just pulled it off is the
    /// same read as the node in the world, and the label alone cannot recover the kind.
    pub kind: String,
    pub qty: i32,
    pub age: f32,
}

impl HarvestPop {
    /// What the floater says. The icon goes in front of this, never instead of it.
    pub fn label(&self) -> String {
        format!("+{} {}", self.qty, crate::icons::display_name(&self.kind))
    }
}

/// The floaters currently in the air, oldest first.
#[derive(Resource, Default)]
pub(crate) struct HarvestPops {
    pub items: Vec<HarvestPop>,
}

/// How long a `+1 <material>` floater stays up.
pub(crate) const HARVEST_POP_TTL: f32 = 1.4;

impl HarvestPops {
    /// Note a unit banked. Same material twice in a row stacks into one floater rather than
    /// printing a column of identical lines.
    pub fn banked(&mut self, kind: &str, qty: i32) {
        if let Some(last) = self.items.last_mut() {
            // Merging on the KIND rather than by parsing the count back out of the label:
            // the label is for the player, and reading state out of a string you formatted
            // is how a rename quietly breaks the tally.
            if last.age < 0.35 && last.kind == kind {
                last.qty += qty.max(1);
                last.age = 0.0;
                return;
            }
        }
        self.items.push(HarvestPop { kind: kind.to_string(), qty: qty.max(1), age: 0.0 });
    }
}

#[cfg(test)]
mod harvest_pop_tests {
    use super::*;

    /// A harvest channel pays per tick, and the payout used to be invisible: the bar filled,
    /// the stock landed in a panel nobody was looking at, and nothing said what you got.
    #[test]
    fn banked_units_stack_into_one_floater() {
        let mut pops = HarvestPops::default();
        pops.banked("bog_myrrh", 1);
        assert_eq!(pops.items.len(), 1);
        assert_eq!(pops.items[0].label(), "+1 Bog Myrrh", "the key is shown as words");

        // A second unit of the same thing, straight away, becomes "+2" rather than a
        // second identical line — a gather is many ticks and that would be a column.
        pops.banked("bog_myrrh", 1);
        assert_eq!(pops.items.len(), 1);
        assert_eq!(pops.items[0].label(), "+2 Bog Myrrh");

        // A different material is its own floater.
        pops.banked("peat_iron", 1);
        assert_eq!(pops.items.len(), 2);
        assert_eq!(pops.items[1].label(), "+1 Peat Iron");
    }

    /// Once a floater has aged out it is dropped, so the stack over the head cannot grow
    /// without bound over a long dig.
    #[test]
    fn floaters_expire() {
        let mut pops = HarvestPops::default();
        pops.banked("bog_myrrh", 1);
        pops.items[0].age = HARVEST_POP_TTL + 0.01;
        pops.items.retain(|p| p.age < HARVEST_POP_TTL);
        assert!(pops.items.is_empty(), "an old floater should be gone");
    }

    /// A Hunter skill the active hero can't currently afford greys out (`enabled ==
    /// false`) instead of staying clickable and stalling the hero's turn when the
    /// server refuses it without ever resolving that turn.
    #[test]
    fn a_hunter_skill_disables_when_adrenaline_is_short() {
        let mut roster = PartyRoster::default();
        roster.ability_costs.insert("power_strike".into(), 25);
        roster.ability_costs.insert("second_wind".into(), 15);

        // Not enough banked: greyed out.
        let short = skill_entries("hunter", 4, &[], &roster, 10);
        let ps = short.iter().find(|e| e.action.skill_key() == Some("power_strike")).unwrap();
        assert!(!ps.enabled, "10 banked must not afford a 25-cost skill");

        // Exactly enough: affordable.
        let exact = skill_entries("hunter", 4, &[], &roster, 25);
        let ps = exact.iter().find(|e| e.action.skill_key() == Some("power_strike")).unwrap();
        assert!(ps.enabled, "banking exactly the cost must afford it");

        // A skill with no registered cost (or a class with none at all) is never
        // gated by Adrenaline it doesn't spend.
        let no_cost = skill_entries("hunter", 4, &[], &PartyRoster::default(), 0);
        let ps = no_cost.iter().find(|e| e.action.skill_key() == Some("power_strike")).unwrap();
        assert!(ps.enabled, "no cost data yet must not read as unaffordable");
    }

    /// A skill row's tooltip is the registry's prose plus the magnitudes the server
    /// resolved. Only the server has `balance.toml`, so before the roster arrives the
    /// row still reads — it just has no numbers yet.
    #[test]
    fn a_skill_row_carries_its_numbers_once_the_roster_lands() {
        let mut roster = PartyRoster::default();
        let rows = skill_entries("hunter", 4, &[], &roster, 0);
        let ps = rows.iter().find(|e| e.action.skill_key() == Some("power_strike")).unwrap();
        assert!(!ps.tooltip.is_empty(), "the prose is always there");

        assert_eq!(roster.effect("power_strike"), "", "no numbers before the roster");
        roster.ability_effects.insert(
            "power_strike".into(),
            "1.75× damage · 40 of 100 Adrenaline (25 per Attack)".into(),
        );
        assert!(roster.effect("power_strike").contains("Adrenaline"));
        // A Psyker's Manifest rows are abilities too, so they resolve the same way.
        let foci = menu_entries(
            MenuLevel::Manifest,
            "psyker",
            16,
            &[],
            &[],
            &[],
            &PartyRoster::default(),
            0,
        );
        assert!(foci.iter().any(|e| e.action.skill_key() == Some("gravity_well")));
        // Rows that are not abilities have no key to look up, and must not panic.
        assert_eq!(EntryAction::Attack.skill_key(), None);
        assert_eq!(EntryAction::Back.skill_key(), None);
    }

    /// An ASPECT is offered only under a Focus that is actually held. The engine refuses
    /// it otherwise, and a row you can press to be told no is exactly what the registry-
    /// driven menu exists to prevent. Holding the parent is also how the chain teaches
    /// itself — Gravity appears the moment Gravity Well is up.
    #[test]
    fn an_aspect_row_appears_only_under_a_held_focus() {
        let key = |rows: &[MenuEntry], k: &str| rows.iter().any(|e| e.action.skill_key() == Some(k));

        let roster = PartyRoster::default();
        let cold = menu_entries(MenuLevel::Manifest, "psyker", 255, &[], &[], &[], &roster, 0);
        assert!(key(&cold, "gravity_well"), "the manifestation is always offered");
        assert!(!key(&cold, "gravity"), "Gravity offered with nothing to deepen");
        assert!(!key(&cold, "anchor"), "Anchor offered with nothing to deepen");

        let held = |ks: &[&str]| ks.iter().map(|k| k.to_string()).collect::<Vec<_>>();
        let one = menu_entries(
            MenuLevel::Manifest,
            "psyker",
            255,
            &[],
            &[],
            &held(&["gravity_well"]),
            &roster,
            0,
        );
        assert!(key(&one, "gravity"), "Gravity hidden while its parent is held");
        assert!(!key(&one, "anchor"), "Anchor skipped Gravity");

        let two = menu_entries(
            MenuLevel::Manifest,
            "psyker",
            255,
            &[],
            &[],
            &held(&["gravity_well", "gravity"]),
            &roster,
            0,
        );
        assert!(key(&two, "anchor"), "Anchor hidden with the whole chain held");

        // …and a hero too junior for the aspect never sees it, parent or no parent.
        let junior = menu_entries(
            MenuLevel::Manifest,
            "psyker",
            1,
            &[],
            &[],
            &held(&["gravity_well"]),
            &roster,
            0,
        );
        assert!(!key(&junior, "gravity"), "a level-1 Psyker was offered a level-5 aspect");
    }

    fn announce_app(screen: Screen) -> App {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, bevy::state::app::StatesPlugin))
            .init_state::<Screen>()
            .insert_resource(ButtonInput::<KeyCode>::default())
            .insert_resource(UnlocksRes::default())
            .insert_resource(LevelUpQueue::default())
            .add_plugins(announce_plugin);
        app.world_mut().resource_mut::<NextState<Screen>>().set(screen);
        app.update();
        app.world_mut().resource_mut::<UnlocksRes>().pending.push_back(net::UnlockLine {
            key: "class_resonant".into(),
            name: "Resonant".into(),
            kind: "class".into(),
            class_key: None,
            slot: None,
            trigger_text: "Lose a run to a wipe.".into(),
            banner: "That fight wanted a healer.".into(),
        });
        app
    }

    fn banners(app: &mut App) -> usize {
        app.world_mut().query::<&UnlockBannerRoot>().iter(app.world()).count()
    }

    /// The banner a wipe hands you lands while the client is on its way to The Last
    /// City, so town is where it has to be readable AND dismissable. It used to draw
    /// there and then ignore every key, because only the overworld ran its system.
    #[test]
    fn an_unlock_banner_is_dismissible_in_town() {
        for screen in [Screen::City, Screen::Ended, Screen::Overworld] {
            let mut app = announce_app(screen.clone());
            app.update();
            assert_eq!(banners(&mut app), 1, "no banner on {screen:?}");
            app.world_mut().resource_mut::<ButtonInput<KeyCode>>().press(KeyCode::Space);
            app.update();
            assert_eq!(banners(&mut app), 0, "[Space] did not dismiss on {screen:?}");
        }
    }

    /// A state that does not DRAW the banner must not inherit its node, or the thing
    /// the player cannot dismiss simply moves to the fight instead of the plaza.
    #[test]
    fn a_banner_does_not_follow_you_into_a_fight() {
        let mut app = announce_app(Screen::Overworld);
        app.update();
        assert_eq!(banners(&mut app), 1);
        app.world_mut().resource_mut::<NextState<Screen>>().set(Screen::Battle);
        app.update();
        assert_eq!(banners(&mut app), 0, "the banner rode into the battle");
        // Still owed, though — it is re-shown when the fight is over, not swallowed.
        app.world_mut().resource_mut::<NextState<Screen>>().set(Screen::Overworld);
        app.update();
        app.update();
        assert_eq!(banners(&mut app), 1, "the unlock was swallowed by the fight");
    }
}

#[cfg(test)]
mod autoplay_tests {
    use super::*;

    /// **AUTOPLAY MUST NOT PRESS A SKILL THE HERO HAS NOT LEARNED.** It bypasses
    /// `menu_entries` entirely, so nothing greys its rows — it just submits, and the server
    /// refuses. `HUNTER_SKILLS` used to carry its own unlock levels and they had gone stale
    /// against the round `RUNGS`: Second Wind at 2 (really 5), Snare at 2 (really 10),
    /// Frenzy at 3 (really 20). So an autoplaying Hunter spent most of its fight submitting
    /// locked abilities. The levels come from the registry now; this holds every row it
    /// names to actually being that class's, and to being priced.
    #[test]
    fn the_autoplay_hunter_only_presses_rows_the_registry_agrees_it_has() {
        for (key, cost) in HUNTER_SKILLS {
            assert_eq!(
                meld_proto::skills::skill_owner(key),
                Some("hunter"),
                "{key} is not a Hunter row"
            );
            let (unlock, c) = hunter_skill(key);
            assert_eq!(unlock, meld_proto::skills::unlock_level(key), "{key} unlock is a copy");
            assert_eq!(c, cost, "{key} price is not the table's");
            assert!(unlock >= 1 && c > 0, "{key}: unlock {unlock}, cost {c}");
        }
        // The specific regression: a level-4 Hunter is not offered Second Wind however hurt
        // it is and however much Adrenaline it is sitting on.
        let hurt_hunter = |level: i32| CombatantView {
            id: "h".into(),
            name: "H".into(),
            hp: 1,
            max_hp: 100,
            gauge: 1.0,
            is_player: true,
            player_id: Some("me".into()),
            level,
            statuses: vec!["class:hunter".into(), "adrenaline:100".into(), "adrenaline_max:100".into()],
        };
        for level in 1..meld_proto::skills::unlock_level("second_wind") {
            assert_ne!(
                explorer_autoplay_op(&hurt_hunter(level)),
                QueuedKind::Skill("second_wind"),
                "autoplay pressed Second Wind at level {level}, before the hero learns it"
            );
        }
        assert_eq!(
            explorer_autoplay_op(&hurt_hunter(meld_proto::skills::unlock_level("second_wind"))),
            QueuedKind::Skill("second_wind"),
            "a hurt Hunter that HAS learned Second Wind should use it"
        );
    }
}
