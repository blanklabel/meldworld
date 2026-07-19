//! meld-client - the Bevy client for MELDWORLD's core gameplay loop
//! (BUILD-PLAN T4 overworld + T5 UI; CANON D16 all-Bevy). Server-authoritative:
//! the client sends intents and renders whatever the server reports (CANON §S).
//!
//! Loop: Join → Overworld (walk into the monster) → Battle (ATB) → Ended.
//!
//! Config: `MELD_SERVER` (default `http://127.0.0.1:8080`) and `MELD_NAME`
//! (default a random guest name).

use std::collections::{HashMap, HashSet};

use bevy::prelude::*;

use bevy::input::mouse::{MouseMotion, MouseWheel};
use bevy::image::{ImageAddressMode, ImageLoaderSettings, ImageSampler, ImageSamplerDescriptor};
use bevy::gltf::GltfAssetLabel;
use bevy::math::Affine2;

use meld_client::hd2d::{self, CharSprite, CharacterFrames};
use meld_client::net;
use net::{ClientCmd, CombatantView, EntityKind, GearLine, HitEffect, Net, ServerMsg, SkillLine};

/// MoveIntents are emitted at this fixed rate (Hz). The server advances the
/// avatar by `avatar_speed / overworld_sim_hz` tiles per intent, so pacing
/// intents at `overworld_sim_hz` yields the configured tiles/sec at ANY render
/// frame rate. Sending one intent per frame instead would make walk speed scale
/// with FPS (crawl in a throttled tab, rocket at 120fps). Keep in sync with
/// `[world] overworld_sim_hz` in balance.toml.
const MOVE_INTENT_HZ: f32 = 20.0;

/// Where the API + realtime socket live. Native: `MELD_SERVER` env (default
/// localhost). Browser: the page origin (trunk proxies `/v1` to the server).
#[cfg(not(target_arch = "wasm32"))]
fn server_base() -> String {
    std::env::var("MELD_SERVER").unwrap_or_else(|_| "http://127.0.0.1:8080".to_string())
}
#[cfg(target_arch = "wasm32")]
fn server_base() -> String {
    let win = web_sys::window();
    let search = win
        .as_ref()
        .and_then(|w| w.location().search().ok())
        .unwrap_or_default();
    // `?server=<url>` override (for the local demo); else the page origin.
    if let Ok(params) = web_sys::UrlSearchParams::new_with_str(&search) {
        if let Some(s) = params.get("server") {
            if !s.is_empty() {
                return s;
            }
        }
    }
    win.and_then(|w| w.location().origin().ok())
        .unwrap_or_else(|| "http://127.0.0.1:8080".to_string())
}

/// Autopilot self-drives the loop (connect → walk → attack) for demos and
/// headless screenshots. Native: `MELD_AUTOPLAY` env. Browser: `?autoplay` in
/// the URL. Real players use the keyboard as normal.
#[cfg(not(target_arch = "wasm32"))]
fn autoplay_flag() -> bool {
    std::env::var("MELD_AUTOPLAY").is_ok()
}
#[cfg(target_arch = "wasm32")]
fn autoplay_flag() -> bool {
    query_has("autoplay")
}

/// Offline render demo: no networking; scripted canned data drives the real
/// rendering so the Overworld/Battle screens can be shown without a server.
/// Native: `MELD_DEMO` env. Browser: `?demo`.
#[cfg(not(target_arch = "wasm32"))]
fn demo_flag() -> bool {
    std::env::var("MELD_DEMO").is_ok()
}
#[cfg(target_arch = "wasm32")]
fn demo_flag() -> bool {
    query_has("demo")
}
#[cfg(target_arch = "wasm32")]
fn query_has(key: &str) -> bool {
    web_sys::window()
        .and_then(|w| w.location().search().ok())
        .map(|s| s.contains(key))
        .unwrap_or(false)
}

/// Pre-select a class without the Join screen (handy for demos/headless runs and
/// with `?autoplay`). Native: `MELD_CLASS` env. Browser: `?class=psyker`.
#[cfg(not(target_arch = "wasm32"))]
fn class_flag() -> Option<String> {
    std::env::var("MELD_CLASS").ok().filter(|s| !s.is_empty())
}
#[cfg(target_arch = "wasm32")]
fn class_flag() -> Option<String> {
    let search = web_sys::window()?.location().search().ok()?;
    let params = web_sys::UrlSearchParams::new_with_str(&search).ok()?;
    params.get("class").filter(|s| !s.is_empty())
}

/// Pre-build the whole party (comma-separated class keys) without the builder.
/// Native: `MELD_PARTY=squire,psyker,resonant,squire`. Browser: `?party=…`.
#[cfg(not(target_arch = "wasm32"))]
fn party_flag() -> Option<String> {
    std::env::var("MELD_PARTY").ok().filter(|s| !s.is_empty())
}
#[cfg(target_arch = "wasm32")]
fn party_flag() -> Option<String> {
    let search = web_sys::window()?.location().search().ok()?;
    let params = web_sys::UrlSearchParams::new_with_str(&search).ok()?;
    params.get("party").filter(|s| !s.is_empty())
}

/// Offline battle-screen mockup: jump straight into the Battle screen with canned
/// combatants and the command window open, so the subscreen can be inspected
/// without a server or walking there. Native: `MELD_BATTLE` env. Browser: `?battle`.
#[cfg(not(target_arch = "wasm32"))]
fn battle_mockup_flag() -> bool {
    std::env::var("MELD_BATTLE").is_ok()
}
#[cfg(target_arch = "wasm32")]
fn battle_mockup_flag() -> bool {
    query_has("battle")
}

/// Offline mockups for the overworld overlays (`?inventory` / `?levelup`, or
/// `MELD_INVENTORY` / `MELD_LEVELUP`).
#[cfg(not(target_arch = "wasm32"))]
fn inventory_mockup_flag() -> bool {
    std::env::var("MELD_INVENTORY").is_ok()
}
#[cfg(target_arch = "wasm32")]
fn inventory_mockup_flag() -> bool {
    query_has("inventory")
}
#[cfg(not(target_arch = "wasm32"))]
fn levelup_mockup_flag() -> bool {
    std::env::var("MELD_LEVELUP").is_ok()
}
#[cfg(target_arch = "wasm32")]
fn levelup_mockup_flag() -> bool {
    query_has("levelup")
}

fn main() {
    let base = server_base();
    App::new()
        .add_plugins(
            DefaultPlugins
                .set(ImagePlugin::default_nearest()) // crisp pixel sprites
                .set(WindowPlugin {
                    primary_window: Some(Window {
                        title: "MELDWORLD".to_string(),
                        resolution: (960.0_f32, 640.0_f32).into(),
                        // Browser (wasm): bind to <canvas id="bevy"> and fill its parent.
                        canvas: Some("#bevy".to_string()),
                        fit_canvas_to_parent: true,
                        ..default()
                    }),
                    ..default()
                }),
        )
        .init_state::<Screen>()
        // Daytime sky blue behind the diorama (the fog fades the ground into it).
        .insert_resource(ClearColor(Color::srgb(0.53, 0.72, 0.93)))
        .insert_resource(hd2d::ambient_light())
        .init_resource::<hd2d::Look>()
        .init_resource::<hd2d::LookWatch>()
        .insert_non_send_resource(NetRes(net::start(base)))
        // Demo and autoplay are mutually exclusive; demo skips networking.
        .insert_resource(Autoplay(autoplay_flag() && !demo_flag()))
        .insert_resource(Demo {
            on: demo_flag(),
            t: 0.0,
            started: false,
        })
        .init_resource::<Session>()
        .init_resource::<Sky>()
        .init_resource::<MoveClock>()
        .init_resource::<BattleMenu>()
        .init_resource::<HitFx>()
        .init_resource::<Overlay>()
        .init_resource::<InventoryData>()
        .init_resource::<ProgressData>()
        .init_resource::<Overworld>()
        .init_resource::<RunBackpack>()
        .init_resource::<WorldPath>()
        .init_resource::<Terrain>()
        .init_resource::<PartyRoster>()
        .init_resource::<HeroRename>()
        .init_resource::<Steer>()
        .init_resource::<TapTarget>()
        .init_resource::<Joystick>()
        .init_resource::<BattleData>()
        .init_resource::<EndInfo>()
        .init_resource::<LobbyData>()
        .add_systems(
            Startup,
            (setup, apply_class_flag, mock_battle_setup, mock_overlay_setup),
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
                drift_clouds,
                anchor_backdrop,
                advance_sky,
                apply_sky,
                anchor_sky_fx,
                drive_rain,
                animate_water,
            ),
        )
        // Join
        .add_systems(OnEnter(Screen::Join), join_ui)
        .add_systems(OnExit(Screen::Join), despawn::<JoinRoot>)
        .add_systems(Update, join_input.run_if(in_state(Screen::Join)))
        // Lobby (co-op)
        .add_systems(OnEnter(Screen::Lobby), lobby_ui)
        .add_systems(OnExit(Screen::Lobby), despawn::<LobbyRoot>)
        .add_systems(
            Update,
            (lobby_input, render_lobby).run_if(in_state(Screen::Lobby)),
        )
        // Overworld
        .add_systems(OnEnter(Screen::Overworld), overworld_ui)
        .add_systems(
            OnExit(Screen::Overworld),
            (
                despawn::<OverworldRoot>,
                despawn::<OverlayRoot>,
                despawn::<WorldEntity>,
                despawn::<PathTrail>,
                despawn::<TerrainMesh>,
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
                draw_path_trail,
                build_terrain_sections,
                hd2d::animate_chars,
                hd2d_follow,
                hd2d::place_billboards,
                hd2d::billboard,
                animate_sway,
                update_overworld_hud,
                render_overlay,
            )
                .run_if(in_state(Screen::Overworld)),
        )
        // Battle
        .add_systems(OnEnter(Screen::Battle), (clear_overworld_sprites, enter_battle))
        .add_systems(
            OnExit(Screen::Battle),
            (
                despawn::<BattleScene>,
                despawn::<PartyWindow>,
                despawn::<AllyPartyStrips>,
                despawn::<CommandWindow>,
                despawn::<HitFxRoot>,
                despawn::<BattleActor>,
            ),
        )
        .add_systems(
            Update,
            (
                validate_active,
                auto_fire_queued,
                menu_keyboard,
                menu_click,
                rebuild_command_menu,
                style_command_menu,
                render_enemy_panel,
                render_party_window,
                render_ally_parties,
                advance_hit_fx,
                render_hit_fx,
                // HD-2D arena: 3D combatant sprites + battle camera, framed by the UI.
                sync_battle_actors,
                battle_camera,
                hd2d::animate_chars,
                hd2d::place_billboards,
                hd2d::billboard,
            )
                .run_if(in_state(Screen::Battle)),
        )
        // Ended
        .add_systems(OnEnter(Screen::Ended), ended_ui)
        .add_systems(Update, ended_input.run_if(in_state(Screen::Ended)))
        .run();
}

// ---------------------------------------------------------------- states ---

#[derive(States, Debug, Clone, PartialEq, Eq, Hash, Default)]
enum Screen {
    #[default]
    Join,
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
    connecting: bool,
    entered: bool,
    channeling: bool,
    status: String,
    /// The party the player built on the Join screen — one class key per hero
    /// slot (wire form: "squire" / "psyker" / "resonant"). Sent on enter_maze.
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
            connecting: false,
            entered: false,
            channeling: false,
            status: String::new(),
            // A diverse default so newcomers see all three classes at once.
            party: vec![
                "squire".into(),
                "psyker".into(),
                "resonant".into(),
                "squire".into(),
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
}

impl OwEntity {
    fn player(x: f32, y: f32) -> Self {
        Self { x, y, kind: EntityKind::Player, name: None, faction: None, radius: 0.0, battling: false, level: 0 }
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
        }
    }
    fn portal(x: f32, y: f32) -> Self {
        Self { x, y, kind: EntityKind::Portal, name: None, faction: None, radius: 0.0, battling: false, level: 0 }
    }
}

#[derive(Resource, Default)]
struct Overworld {
    /// entity id -> its render state
    entities: HashMap<String, OwEntity>,
}

/// The current run's backpack (Town Portals + gathered materials), mirrored from
/// the server for the overworld HUD.
#[derive(Resource, Default)]
struct RunBackpack {
    items: Vec<(String, i32)>,
}

impl RunBackpack {
    fn count(&self, kind: &str) -> i32 {
        self.items.iter().find(|(k, _)| k == kind).map_or(0, |(_, q)| *q)
    }
}

/// The guaranteed clear path (world-unit waypoints), drawn as a faint trail so the
/// feasible route through the terrain is legible. `drawn` gates one-time spawning.
#[derive(Resource, Default)]
struct WorldPath {
    points: Vec<(f32, f32)>,
    drawn: bool,
}

/// One elevation level of a terrace lifts the ground (and anything standing on it)
/// by this many world units — roughly one Kenney cliff-block tall, so a terrace
/// edge is dressed with a single row of `cliff_rock` models (see `spawn_terrace_cliffs`).
const STEP_HEIGHT: f32 = 2.0;

/// Uniform scale + facing tuning for the cliff models lining terrace edges (sized so
/// a one-level block rises ~STEP_HEIGHT to meet the grass top).
const CLIFF_EDGE_SCALE: f32 = 1.9;
const CLIFF_YAW_OFFSET: f32 = 0.0;

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

/// The caller's hero roster (name/class/level/stats), shown on the inventory party
/// screen — this is where stats live, not the battle HUD.
#[derive(Resource, Default)]
struct PartyRoster {
    heroes: Vec<meld_client::net::HeroLine>,
}

/// In-progress hero rename on the party screen: the slot being edited + its buffer.
#[derive(Resource, Default)]
struct HeroRename {
    slot: Option<usize>,
    buffer: String,
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
            .unwrap_or_else(|| "squire".to_string())
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
            QueuedKind::Hold => "···",
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

/// A hero's class key parsed from its wire statuses (`class:<key>`), default squire.
fn hero_class(view: &CombatantView) -> String {
    view.statuses
        .iter()
        .find_map(|s| s.strip_prefix("class:"))
        .unwrap_or("squire")
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

/// Vault + gear for the inventory overlay (fetched over HTTP on open).
#[derive(Resource, Default)]
struct InventoryData {
    loaded: bool,
    chits: i64,
    materials: Vec<(String, i32)>,
    gear: Vec<GearLine>,
}

/// Meld skills + class unlocks for the level-up overlay.
#[derive(Resource, Default)]
struct ProgressData {
    loaded: bool,
    skills: Vec<SkillLine>,
    classes: Vec<String>,
}

/// Floating hit-feedback numbers (damage/heal) with a short lifetime.
#[derive(Resource, Default)]
struct HitFx {
    items: Vec<Hit>,
}
struct Hit {
    target: String,
    text: String,
    color: Color,
    age: f32,
}
/// Seconds a floating number lives.
const HIT_TTL: f32 = 1.0;
/// Seconds a target stays "flashed" after being hit.
const FLASH_TTL: f32 = 0.18;

#[derive(Resource, Default)]
struct EndInfo {
    outcome: String,
    banked: usize,
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
#[derive(Clone, Copy, PartialEq, Eq, Default)]
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
    Back,
}

/// One selectable row in the command window.
struct MenuEntry {
    label: &'static str,
    action: EntryAction,
}

/// Build skill menu rows from `(label, skill_kind)` pairs, keeping only those the
/// hero has leveled into (per `meld_proto::skills`).
fn skill_entries(skills: &[(&'static str, &'static str)], hero_level: i32) -> Vec<MenuEntry> {
    skills
        .iter()
        .filter(|(_, kind)| meld_proto::skills::is_unlocked(kind, hero_level))
        .map(|(label, kind)| MenuEntry {
            label,
            action: EntryAction::Skill(kind),
        })
        .collect()
}

/// The rows shown at a given menu level. For a Psyker the root is `Focus / Revoke /
/// Hold` and the Manifest page lists the manifestations unlocked at `hero_level`. The
/// dynamic pages (Target, Revoke) draw their rows from live battle state instead
/// ([`BattleMenu::rows`]), so they return empty here.
fn menu_entries(level: MenuLevel, class: &str, hero_level: i32) -> Vec<MenuEntry> {
    let e = |label, action| MenuEntry { label, action };
    match level {
        MenuLevel::Root if class == "psyker" => vec![
            e("Focus", EntryAction::OpenManifest),
            e("Revoke", EntryAction::OpenRevoke),
            e("Hold", EntryAction::Hold),
        ],
        MenuLevel::Root => vec![
            e("Attack", EntryAction::Attack),
            e("Defend", EntryAction::Defend),
            e("Item", EntryAction::OpenItems),
            e("Skill", EntryAction::OpenSkills),
        ],
        // Skills unlock as the hero levels; a locked one is simply hidden (the
        // server would reject it anyway). Levels come from `meld_proto::skills`.
        MenuLevel::Skills if class == "resonant" => {
            let mut v = skill_entries(
                &[
                    ("Transfuse", "transfuse"),
                    ("Regen Boon", "regen_boon"),
                    ("Ward", "ward"),
                ],
                hero_level,
            );
            v.push(e("Back", EntryAction::Back));
            v
        }
        MenuLevel::Skills => {
            let mut v = skill_entries(
                &[("Power Strike", "power_strike"), ("Second Wind", "second_wind")],
                hero_level,
            );
            v.push(e("Back", EntryAction::Back));
            v
        }
        MenuLevel::Items => vec![
            e("Salve", EntryAction::Item("salve")),
            e("Elixir", EntryAction::Item("elixir")),
            e("Back", EntryAction::Back),
        ],
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
/// Immediate-mode overlay holding floating hit numbers.
#[derive(Component)]
struct HitFxRoot;
/// Immediate-mode root for an overworld overlay (inventory / level-up).
#[derive(Component)]
struct OverlayRoot;
#[derive(Component)]
struct EndedRoot;
#[derive(Component)]
struct StatusText;
/// Join-screen line showing the currently-selected class.
#[derive(Component)]
struct ClassText;

/// A sprite representing an overworld entity, tagged by its server id.
#[derive(Component)]
struct WorldEntity(String);

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

/// Move + pivot the overworld camera: **mouse** left/right-drag orbits, wheel
/// zooms; **touch** two-finger drag orbits, pinch zooms. Both nudge the live
/// `Look` (yaw/pitch/dist), which `hd2d_follow` then applies while keeping the
/// player centred. Camera-relative facing keeps the hero oriented as you orbit.
#[allow(clippy::too_many_arguments)]
fn overworld_camera_control(
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
        look.cam_pitch = (look.cam_pitch + dy * 0.4).clamp(10.0, 85.0);
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

/// Shared meshes/materials + the psyker sprite set, built once at startup so the
/// overworld sync can spawn 3D entities without rebuilding assets each frame.
#[derive(Resource)]
struct WorldAssets {
    psyker: CharacterFrames,
    squire: CharacterFrames,
    sprite_quad: Handle<Mesh>,
    shadow_mesh: Handle<Mesh>,
    shadow_mat: Handle<StandardMaterial>,
    /// CC0 pixel-art creature billboards keyed by creature content id (see
    /// `meld-world::creatures_for_biome`); unknown kinds fall back to [`Self::monster_pool`].
    /// Creatures stay 2D sprites — the HD-2D convention (2D actors, 3D world).
    monster_sprites: HashMap<String, Handle<Image>>,
    monster_pool: Vec<Handle<Image>>,
    /// Real 3D prop models (Kenney Nature Kit, CC0) keyed by terrain-obstacle kind →
    /// several `(scene, baked_scale)` variants (picked per-entity by id hash), so the
    /// world is built from actual geometry instead of flat billboards.
    prop_scenes: HashMap<String, Vec<(Handle<Scene>, f32)>>,
    /// 3D harvest-node models keyed by resource content id → `(scene, baked_scale)`.
    resource_scenes: HashMap<String, (Handle<Scene>, f32)>,
    /// Emissive disc laid under a harvest node so it still reads as gatherable.
    glow_disc: Handle<Mesh>,
    resource_glow: Handle<StandardMaterial>,
    portal_sprite: Handle<Image>,
    portal_mesh: Handle<Mesh>,
    portal_mat: Handle<StandardMaterial>,
    // Capsule stand-in for enemies in the HD-2D battle diorama (PR #21); the
    // overworld uses creature billboards from `monster_sprites` instead.
    monster_mesh: Handle<Mesh>,
    rock_mesh: Handle<Mesh>,
    water_mesh: Handle<Mesh>,
    water_mat: Handle<StandardMaterial>, // shared, animated (see animate_water)
    ground_mat: Handle<StandardMaterial>,
    ground_tex: Vec<Handle<Image>>, // per-biome ground textures (see biome_index)
}

/// Biome tint for the ground, distance-keyed. This now *multiplies* the tiled CC0
/// grass texture (HDR pipeline, so values >1 brighten), recolouring one grass tile
/// into forest green / desert sand / ashen / frosty / murky per biome — richer than
/// the old flat colour band while keeping the geography legible.
/// Subtle per-biome tint — each biome now has its OWN ground texture (see
/// `biome_index` / `WorldAssets::ground_tex`), so the tint only nudges the mood
/// rather than recolouring a single shared texture.
fn hd2d_ground_color(d: i64) -> Color {
    match biome_display(d) {
        "Forest" => Color::srgb(0.92, 1.05, 0.85), // fresh green
        "Desert" => Color::srgb(1.12, 1.02, 0.82), // warm sand
        "Ashfall" => Color::srgb(1.05, 0.72, 0.66), // scorched
        "Tundra" => Color::srgb(0.85, 0.95, 1.18), // frosted blue
        _ => Color::srgb(0.82, 1.0, 0.86),          // Mire — murky green
    }
}

/// Biome → index into `WorldAssets::ground_tex` (Forest/Desert/Ashfall/Tundra/Mire).
fn biome_index(d: i64) -> usize {
    match biome_display(d) {
        "Forest" => 0,
        "Desert" => 1,
        "Ashfall" => 2,
        "Tundra" => 3,
        _ => 4,
    }
}

/// Load an image with a Repeat sampler so it tiles across the big ground plane.
fn load_tiled(assets: &AssetServer, path: &str) -> Handle<Image> {
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
fn setup(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut mats: ResMut<Assets<StandardMaterial>>,
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
    let ground_mat = mats.add(StandardMaterial {
        base_color: hd2d_ground_color(0),
        base_color_texture: Some(ground_tex[0].clone()),
        uv_transform: Affine2::from_scale(Vec2::new(2000.0 / 3.0, 600.0 / 3.0)),
        perceptual_roughness: 0.95,
        ..default()
    });
    commands.spawn((
        WorldGround,
        Mesh3d(meshes.add(Plane3d::default().mesh().size(2000.0, 600.0))),
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
        squire: hd2d::load_character(
            &assets,
            "characters/PSYKER_Male/Squire",
            "Walking",
            8,
        ),
        // Cylindrical normals so the sun models the flat sprite (HD-2D depth).
        sprite_quad: meshes.add(hd2d::cyl_billboard_mesh(2.2, 2.2, 12, 60.0)),
        shadow_mesh: meshes.add(Circle::new(0.7)),
        shadow_mat: mats.add(hd2d::contact_shadow_material()),
        monster_sprites,
        monster_pool,
        prop_scenes,
        resource_scenes,
        glow_disc: meshes.add(Circle::new(0.6)),
        resource_glow: mats.add(StandardMaterial {
            base_color: Color::srgba(1.0, 0.85, 0.35, 0.55),
            emissive: LinearRgba::rgb(2.2, 1.6, 0.4),
            unlit: true,
            alpha_mode: AlphaMode::Blend,
            ..default()
        }),
        portal_sprite: ld("fx/portal_arch.png"),
        // A faint emissive ground-ring keeps the portal glowing under the billboard.
        portal_mesh: meshes.add(Torus::new(0.18, 1.15)),
        portal_mat: mats.add(StandardMaterial {
            base_color: Color::srgb(0.1, 0.4, 0.5),
            emissive: LinearRgba::rgb(0.4, 5.0, 6.0),
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
        ground_mat,
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
            Cloud { off, y: 0.1 },
            CloudShadow,
            Mesh3d(puff.clone()),
            MeshMaterial3d(cloud_shadow_mat.clone()),
            Transform::from_translation(Vec3::new(off.x, 0.1, off.y))
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
}

/// Height of the drifting rain cloud, and the footprint the rain falls within.
const RAIN_CLOUD_Y: f32 = 32.0;
const RAIN_RADIUS: f32 = 18.0;
const RAIN_FALL_TOP: f32 = 30.0;

/// Marks a cloud's ground shadow (flat, dark) vs a sky cloud puff — both drift via
/// [`drift_clouds`], but shadows stay flat on the ground (no billboarding).
#[derive(Component)]
struct CloudShadow;

/// A far-off cliff/mountain on the horizon, anchored around the camera (like the
/// clouds) so the diorama always has depth behind it. `off` is its xz offset from the
/// camera; see [`anchor_backdrop`]. Fogged into the sky at that distance.
#[derive(Component)]
struct Backdrop {
    off: Vec2,
}

/// Wind sway for foliage: the prop leans back and forth around its base (which sits
/// on the ground) so the top travels most — reading as leaves moving in the wind.
/// `base_yaw` preserves the spawn-time variety rotation the sway composes onto; the
/// sway strengthens in rain (see [`animate_sway`]). Applied to trees/mushrooms/cacti.
#[derive(Component)]
struct Sway {
    base_yaw: f32,
    phase: f32,
    amp: f32,
    speed: f32,
}

/// Per-obstacle-kind wind-sway amplitude (radians of lean); `None` = rigid (rock/etc).
fn sway_amp(kind: &str) -> Option<f32> {
    match kind {
        "tree" => Some(0.05),
        "fungal_wall" => Some(0.045),
        "cactus" => Some(0.02),
        _ => None,
    }
}

/// Lean every [`Sway`] prop on the wind — top-heavy (pivots at the grounded base),
/// phase-offset per prop, and gustier while it's raining. Overworld only.
fn animate_sway(time: Res<Time>, sky: Option<Res<Sky>>, mut q: Query<(&Sway, &mut Transform)>) {
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
fn anchor_backdrop(
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
struct Cloud {
    off: Vec2,
    y: f32,
}

/// Wind speed (world units/sec) the clouds drift east.
const CLOUD_WIND: f32 = 2.5;

/// Drift the clouds on the wind and keep them anchored around the camera (wrapping
/// so the sky never empties as the player travels).
fn drift_clouds(
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

// ============================ time of day + weather ========================

/// Seconds for one full day → night → day cycle.
const DAY_LEN: f32 = 210.0;

/// Time of day (`t`: 0 = midnight, 0.5 = noon) + weather (`0` clear .. `1` rain),
/// which together drive the sun, ambient, sky/fog colour, stars, and rain.
#[derive(Resource)]
struct Sky {
    t: f32,
    weather: f32,
    weather_target: f32,
    weather_timer: f32,
}
impl Default for Sky {
    fn default() -> Self {
        Sky { t: 0.36, weather: 0.0, weather_target: 0.0, weather_timer: 55.0 }
    }
}

/// Material handles `apply_sky` modulates over the day (cloud glow).
#[derive(Resource, Default)]
struct SkyMats {
    cloud: Handle<StandardMaterial>,
}

/// A background star, camera-anchored (`off`) and shown only at night.
#[derive(Component)]
struct Star {
    off: Vec3,
}

/// A rain streak. `off.xz` is its position within the rain cloud's footprint disk;
/// `off.y` is its fall height. Positioned under the drifting rain cloud (`drive_rain`).
#[derive(Component)]
struct RainDrop {
    off: Vec3,
}

/// The single storm cloud that carries the rain. `off` is its xz offset from the
/// camera; it drifts on the wind and the rain falls in the disk beneath it.
#[derive(Component)]
struct RainCloud {
    off: Vec2,
}

/// Lerp two colours in sRGB space.
fn mix_col(a: Color, b: Color, t: f32) -> Color {
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
fn advance_sky(time: Res<Time>, mut sky: ResMut<Sky>) {
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
fn apply_sky(
    sky: Res<Sky>,
    skymats: Option<Res<SkyMats>>,
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

    let night_sky = Color::srgb(0.03, 0.05, 0.10);
    let day_sky = Color::srgb(0.50, 0.72, 0.93);
    let dusk_sky = Color::srgb(0.66, 0.42, 0.30);
    let rain_sky = Color::srgb(0.36, 0.40, 0.44);
    let mut sky_col = mix_col(night_sky, day_sky, day);
    sky_col = mix_col(sky_col, dusk_sky, dusk * 0.6);
    sky_col = mix_col(sky_col, rain_sky, rain * 0.7 * (0.35 + day * 0.65));
    clear.0 = sky_col;
    if let Ok(mut fog) = fog_q.single_mut() {
        fog.color = mix_col(sky_col, Color::WHITE, 0.04);
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
fn anchor_sky_fx(
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
fn drive_rain(
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
fn animate_water(
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

/// The classes the party builder cycles through.
const PARTY_CLASSES: [&str; 3] = ["squire", "psyker", "resonant"];

/// Pre-fill the party builder from flags: `?party=` (whole party) wins, else
/// `?class=` sets the lead (slot 0). Both default to the diverse starting party.
fn apply_class_flag(mut session: ResMut<Session>) {
    if let Some(p) = party_flag() {
        let party: Vec<String> = p
            .split(',')
            .map(|s| s.trim().to_lowercase())
            .filter(|s| PARTY_CLASSES.contains(&s.as_str()))
            .collect();
        if !party.is_empty() {
            session.party = party;
        }
    } else if let Some(c) = class_flag() {
        if let Some(slot0) = session.party.first_mut() {
            *slot0 = c;
        }
    }
}

/// If the battle-mockup flag is set, seed canned combatants and jump straight to
/// the Battle screen (no networking) so the battle subscreen is viewable on its
/// own. Runs once at startup; a no-op otherwise.
fn mock_battle_setup(
    mut battle: ResMut<BattleData>,
    mut hitfx: ResMut<HitFx>,
    mut next: ResMut<NextState<Screen>>,
) {
    if !battle_mockup_flag() {
        return;
    }
    // Canned hit feedback so the floating numbers + flash are visible statically.
    hitfx.items.push(Hit {
        target: "grendel".into(),
        text: "-17".into(),
        color: Color::srgb(1.0, 0.5, 0.4),
        age: 0.0,
    });
    hitfx.items.push(Hit {
        target: "h3".into(),
        text: "+12".into(),
        color: Color::srgb(0.5, 1.0, 0.6),
        age: 0.0,
    });
    let hero = |id: &str, hp, gauge| CombatantView {
        id: id.into(),
        name: "Hero".into(),
        hp,
        max_hp: 40,
        gauge,
        is_player: true,
        player_id: Some("me".into()),
        level: 1,
        statuses: vec![],
    };
    battle.battle_id = "mock".to_string();
    battle.your_ids = vec!["h1".into(), "h2".into(), "h3".into(), "h4".into()];
    battle.monster_combatant = Some("grendel".to_string());
    battle.active = Some("h1".to_string());
    battle.ready.insert("h1".to_string());
    battle.ready.insert("h3".to_string());
    battle.queued.insert(
        "h2".to_string(),
        Order { kind: QueuedKind::Attack, target: Some("grendel".into()) },
    );
    battle.queued.insert(
        "h4".to_string(),
        Order { kind: QueuedKind::Skill("power_strike"), target: Some("grendel".into()) },
    );
    battle.combatants = vec![
        hero("h1", 32, 1.0),
        hero("h2", 40, 0.4),
        hero("h3", 21, 1.0),
        hero("h4", 36, 0.75),
        CombatantView {
            id: "grendel".into(),
            name: "Grendel".into(),
            hp: 44,
            max_hp: 60,
            gauge: 0.65,
            is_player: false,
            player_id: None,
            level: 1,
            statuses: vec![],
        },
    ];
    next.set(Screen::Battle);
}

/// If an overlay-mockup flag is set, seed canned inventory/progress data and jump
/// to the overworld with that screen open — so the overlays are viewable on their
/// own without a server.
fn mock_overlay_setup(
    mut overlay: ResMut<Overlay>,
    mut inv: ResMut<InventoryData>,
    mut prog: ResMut<ProgressData>,
    mut world: ResMut<Overworld>,
    mut next: ResMut<NextState<Screen>>,
) {
    if inventory_mockup_flag() {
        inv.loaded = true;
        inv.chits = 1240;
        inv.materials = vec![
            ("forest_bloom_petal".into(), 7),
            ("stalker_hide".into(), 3),
            ("bloom_salve".into(), 2),
        ];
        inv.gear = vec![
            GearLine {
                name: "Chipped Blade".into(),
                equipped: true,
                max_durability: 90,
                base_max_durability: 100,
                atk_bonus: 3,
            },
            GearLine {
                name: "Bloom Ward".into(),
                equipped: false,
                max_durability: 60,
                base_max_durability: 60,
                atk_bonus: 1,
            },
        ];
        overlay.kind = Some(OverlayKind::Inventory);
    } else if levelup_mockup_flag() {
        prog.loaded = true;
        prog.skills = vec![
            SkillLine { kind: "alchemy".into(), level: 3, xp: 245 },
            SkillLine { kind: "forging".into(), level: 2, xp: 130 },
            SkillLine { kind: "gatekeeping".into(), level: 1, xp: 20 },
        ];
        prog.classes = vec!["squire".into(), "dragoon".into()];
        overlay.kind = Some(OverlayKind::LevelUp);
    } else {
        return;
    }
    // A minimal overworld behind the overlay.
    world.entities.insert("me".into(), OwEntity::player(0.0, 0.0));
    world.entities.insert("grendel".into(), OwEntity::monster(10.0, 0.0, "forest_bloom_stalker", "beast"));
    world.entities.insert("portal".into(), OwEntity::portal(14.0, 0.0));
    next.set(Screen::Overworld);
}

fn despawn<T: Component>(mut commands: Commands, q: Query<Entity, With<T>>) {
    for e in &q {
        commands.entity(e).despawn();
    }
}

// --------------------------------------------------------------- net pump --

/// Drain server messages every frame, update resources, drive transitions.
#[allow(clippy::too_many_arguments)]
fn pump_net(
    net: NonSend<NetRes>,
    mut session: ResMut<Session>,
    mut world: ResMut<Overworld>,
    mut battle: ResMut<BattleData>,
    mut end: ResMut<EndInfo>,
    mut menu: ResMut<BattleMenu>,
    mut hitfx: ResMut<HitFx>,
    mut inv: ResMut<InventoryData>,
    mut prog: ResMut<ProgressData>,
    mut lobby: ResMut<LobbyData>,
    mut backpack: ResMut<RunBackpack>,
    mut world_path: ResMut<WorldPath>,
    mut terrain: ResMut<Terrain>,
    mut roster: ResMut<PartyRoster>,
    state: Res<State<Screen>>,
    mut next: ResMut<NextState<Screen>>,
) {
    net.0.poll();
    while let Some(msg) = net.0.try_recv() {
        match msg {
            ServerMsg::Backpack { items } => backpack.items = items,
            ServerMsg::Party { heroes } => roster.heroes = heroes,
            ServerMsg::WorldPath { points } => {
                world_path.points = points.iter().map(|(x, y)| (*x as f32, *y as f32)).collect();
                world_path.drawn = false;
            }
            ServerMsg::TerrainSection { section } => {
                // A streamed section extends the clear-path trail (initial-chain
                // sections carry no path — that already rode run.started).
                if !section.path.is_empty() {
                    for (x, y) in &section.path {
                        world_path.points.push((*x as f32, *y as f32));
                    }
                    world_path.drawn = false;
                }
                terrain.sections.insert(section.index, section);
            }
            ServerMsg::Connected { player_id } => {
                session.player_id = player_id;
                if session.coop {
                    // Co-op: head to the lobby to create/join a party.
                    session.status = "connected - open the lobby".to_string();
                    if *state.get() == Screen::Join {
                        next.set(Screen::Lobby);
                    }
                } else if !session.entered {
                    // Solo: dive straight in with the built party.
                    session.status = "connected - entering maze...".to_string();
                    session.entered = true;
                    net.0.send(ClientCmd::EnterMaze {
                        party: session.party.clone(),
                    });
                }
            }
            ServerMsg::RunStarted => {
                // Fresh dive: drop any terrain from the previous run before the new
                // section stream arrives (server sends them right after this).
                terrain.sections.clear();
                // The dive can start from Join (solo) or Lobby (co-op).
                lobby.in_lobby = false;
                if matches!(*state.get(), Screen::Join | Screen::Lobby) {
                    next.set(Screen::Overworld);
                }
            }
            ServerMsg::LobbyState { code, host, members } => {
                lobby.in_lobby = true;
                lobby.code = code;
                lobby.my_ready = members
                    .iter()
                    .find(|(id, _, _)| id == &session.player_id)
                    .map(|(_, _, r)| *r)
                    .unwrap_or(false);
                lobby.host = host;
                lobby.members = members;
                if *state.get() == Screen::Join {
                    next.set(Screen::Lobby);
                }
            }
            ServerMsg::LobbyClosed => {
                lobby.in_lobby = false;
                lobby.members.clear();
                lobby.code.clear();
            }
            ServerMsg::Snapshot { entities } => {
                world.entities.clear();
                for e in entities {
                    world.entities.insert(
                        e.id,
                        OwEntity {
                            x: e.x as f32,
                            y: e.y as f32,
                            kind: e.kind,
                            name: e.monster_kind,
                            faction: e.faction,
                            radius: e.radius as f32,
                            battling: e.battling,
                            level: e.level,
                        },
                    );
                }
            }
            ServerMsg::BattleStarted {
                battle_id,
                your_combatant_id: _,
                your_combatant_ids,
                combatants,
                monster_combatant,
            } => {
                battle.battle_id = battle_id;
                battle.your_ids = your_combatant_ids;
                battle.monster_combatant = monster_combatant;
                battle.combatants = combatants;
                battle.ready.clear();
                battle.queued.clear();
                battle.active = battle.your_ids.first().cloned();
                reset_menu(&mut menu);
                if *state.get() != Screen::Battle {
                    next.set(Screen::Battle);
                }
            }
            ServerMsg::TurnReady { combatant_id } => {
                // A hero's gauge filled; it can now act (its queued order fires).
                battle.ready.insert(combatant_id);
            }
            ServerMsg::ActionResolved {
                actor: _,
                action: _,
                effects,
            } => {
                for e in effects {
                    // Reflect the authoritative HP immediately + spawn feedback.
                    if let Some(c) = battle.combatants.iter_mut().find(|c| c.id == e.target) {
                        c.hp = e.hp_after;
                    }
                    push_hit_fx(&mut hitfx, &e);
                }
            }
            ServerMsg::CombatantsJoined { combatants } => {
                for c in combatants {
                    if !battle.combatants.iter().any(|x| x.id == c.id) {
                        battle.combatants.push(c);
                    }
                }
            }
            ServerMsg::Gauge { updates } => {
                for (id, gauge, hp, statuses) in updates {
                    if let Some(c) = battle.combatants.iter_mut().find(|c| c.id == id) {
                        c.gauge = gauge;
                        c.hp = hp;
                        c.statuses = statuses;
                    }
                }
            }
            ServerMsg::BattleEnded { outcome } => {
                // Victory returns to the overworld (go extract!); defeat ends.
                if outcome == "victory" {
                    if *state.get() == Screen::Battle {
                        next.set(Screen::Overworld);
                    }
                } else {
                    end.outcome = outcome;
                    end.banked = 0;
                    next.set(Screen::Ended);
                }
            }
            ServerMsg::ChannelStarted { .. } => {
                session.channeling = true;
                session.status = "extracting…".to_string();
            }
            ServerMsg::ChannelInterrupted => {
                session.channeling = false;
                session.status = "extraction interrupted".to_string();
            }
            ServerMsg::RunEnded { result, banked } => {
                session.channeling = false;
                end.outcome = result;
                end.banked = banked;
                next.set(Screen::Ended);
            }
            ServerMsg::InventoryData {
                chits,
                materials,
                gear,
            } => {
                inv.chits = chits;
                inv.materials = materials;
                inv.gear = gear;
                inv.loaded = true;
            }
            ServerMsg::ProgressData { skills, classes } => {
                prog.skills = skills;
                prog.classes = classes;
                prog.loaded = true;
            }
            ServerMsg::Error { message } => {
                session.status = format!("error: {message}");
            }
            ServerMsg::Disconnected => {
                session.status = "disconnected".to_string();
            }
        }
    }
}

/// Offline demo timeline (no networking): walk the overworld, then fight and win.
#[allow(clippy::too_many_arguments)]
fn demo_driver(
    time: Res<Time>,
    mut demo: ResMut<Demo>,
    mut world: ResMut<Overworld>,
    mut battle: ResMut<BattleData>,
    mut end: ResMut<EndInfo>,
    mut session: ResMut<Session>,
    state: Res<State<Screen>>,
    mut next: ResMut<NextState<Screen>>,
) {
    if !demo.on {
        return;
    }
    demo.t += time.delta_secs();
    let t = demo.t;
    session.player_id = "me".to_string();

    // 0–3s: overworld, hero walking east toward Grendel.
    if t < 3.0 {
        if !demo.started {
            demo.started = true;
            next.set(Screen::Overworld);
        }
        let x = t / 3.0 * 9.0;
        world.entities.clear();
        world.entities.insert("me".to_string(), OwEntity::player(x, 0.0));
        world.entities.insert("grendel".to_string(), OwEntity::monster(10.0, 0.0, "forest_bloom_stalker", "beast"));
        world.entities.insert("portal".to_string(), OwEntity::portal(14.0, 0.0));
        return;
    }

    // 3s+: battle. Grendel's HP falls to 0 over ~5s; gauges animate.
    if *state.get() == Screen::Overworld {
        battle.your_ids = vec!["me".to_string()];
        battle.active = Some("me".to_string());
        battle.monster_combatant = Some("g".to_string());
        battle.combatants = vec![
            CombatantView { id: "me".into(), name: "Hero".into(), hp: 40, max_hp: 40, gauge: 0.0, is_player: true, player_id: Some("me".into()), level: 1, statuses: vec![] },
            CombatantView { id: "g".into(), name: "forest bloom stalker".into(), hp: 60, max_hp: 60, gauge: 0.0, is_player: false, player_id: None, level: 1, statuses: vec![] },
        ];
        next.set(Screen::Battle);
    }
    let phase = t - 3.0;
    let hp = (60.0 * (1.0 - phase / 5.0)).max(0.0) as i32;
    for c in battle.combatants.iter_mut() {
        let p = phase as f64;
        c.gauge = if c.is_player { (p * 0.9) % 1.0 } else { (p * 0.6) % 1.0 };
        if c.id == "g" {
            c.hp = hp;
        }
    }
    if hp <= 0 && *state.get() == Screen::Battle {
        end.outcome = "victory".to_string();
        next.set(Screen::Ended);
    }
}

// ---------------------------------------------------------------- join -----

fn join_ui(mut commands: Commands) {
    commands
        .spawn((
            JoinRoot,
            Node {
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                flex_direction: FlexDirection::Column,
                align_items: AlignItems::Center,
                justify_content: JustifyContent::Center,
                row_gap: Val::Px(16.0),
                ..default()
            },
        ))
        .with_children(|p| {
            p.spawn((
                Text::new("MELDWORLD"),
                TextFont {
                    font_size: 52.0,
                    ..default()
                },
                TextColor(Color::srgb(0.85, 0.9, 1.0)),
            ));
            p.spawn((
                Text::new("Build your party of 4 (keys 1-4 cycle a slot).  ENTER: solo dive   C: co-op with others"),
                TextFont {
                    font_size: 18.0,
                    ..default()
                },
                TextColor(Color::srgb(0.6, 0.65, 0.8)),
            ));
            p.spawn((
                ClassText,
                Text::new(""),
                TextFont {
                    font_size: 22.0,
                    ..default()
                },
                TextColor(Color::srgb(0.75, 0.85, 1.0)),
            ));
            p.spawn((
                StatusText,
                Text::new(""),
                TextFont {
                    font_size: 16.0,
                    ..default()
                },
                TextColor(Color::srgb(0.9, 0.6, 0.6)),
            ));
        });
}

#[allow(clippy::too_many_arguments)]
fn join_input(
    keys: Res<ButtonInput<KeyCode>>,
    net: NonSend<NetRes>,
    autoplay: Res<Autoplay>,
    mut session: ResMut<Session>,
    mut status_q: Query<&mut Text, With<StatusText>>,
    mut class_q: Query<&mut Text, (With<ClassText>, Without<StatusText>)>,
) {
    // Build the party before connecting: keys 1-4 cycle each slot's class through
    // Squire → Psyker → Resonant. Locked in once we start connecting.
    if !session.connecting {
        let slots = [KeyCode::Digit1, KeyCode::Digit2, KeyCode::Digit3, KeyCode::Digit4];
        for (slot, key) in slots.iter().enumerate() {
            if keys.just_pressed(*key) {
                if let Some(cur) = session.party.get(slot).cloned() {
                    let next = PARTY_CLASSES
                        .iter()
                        .position(|c| *c == cur)
                        .map(|i| (i + 1) % PARTY_CLASSES.len())
                        .unwrap_or(0);
                    session.party[slot] = PARTY_CLASSES[next].to_string();
                }
            }
        }
    }

    // ENTER (or autoplay) = solo dive. C = co-op → the lobby after connecting.
    let solo = keys.just_pressed(KeyCode::Enter) || autoplay.0;
    let coop = keys.just_pressed(KeyCode::KeyC);
    if (solo || coop) && !session.connecting {
        session.connecting = true;
        session.coop = coop;
        let name = std::env::var("MELD_NAME").unwrap_or_else(|_| {
            format!("guest{}", &uuid::Uuid::new_v4().simple().to_string()[..8])
        });
        session.status = "connecting...".to_string();
        net.0.send(ClientCmd::Connect { username: name });
    }

    if let Ok(mut t) = class_q.single_mut() {
        let slots: Vec<String> = session
            .party
            .iter()
            .enumerate()
            .map(|(i, c)| format!("[{}] {}", i + 1, nice_class(c)))
            .collect();
        **t = slots.join("   ");
    }
    if let Ok(mut t) = status_q.single_mut() {
        **t = session.status.clone();
    }
}

/// Display name for a class key.
fn nice_class(key: &str) -> &'static str {
    match key {
        "psyker" => "Psyker",
        "resonant" => "Resonant",
        _ => "Squire",
    }
}

// ---------------------------------------------------------------- lobby ----

fn lobby_ui(mut commands: Commands) {
    commands
        .spawn((
            LobbyRoot,
            Node {
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                flex_direction: FlexDirection::Column,
                align_items: AlignItems::Center,
                justify_content: JustifyContent::Center,
                row_gap: Val::Px(14.0),
                ..default()
            },
        ))
        .with_children(|p| {
            p.spawn((
                Text::new("CO-OP LOBBY"),
                TextFont { font_size: 40.0, ..default() },
                TextColor(Color::srgb(0.85, 0.9, 1.0)),
            ));
            p.spawn((
                LobbyText,
                Text::new(""),
                TextFont { font_size: 20.0, ..default() },
                TextColor(Color::srgb(0.8, 0.88, 1.0)),
            ));
        });
}

/// Map a just-pressed key to a lobby-code character (A–Z, 0–9).
fn key_to_code_char(key: KeyCode) -> Option<char> {
    use KeyCode::*;
    let c = match key {
        KeyA => 'A', KeyB => 'B', KeyC => 'C', KeyD => 'D', KeyE => 'E', KeyF => 'F',
        KeyG => 'G', KeyH => 'H', KeyI => 'I', KeyJ => 'J', KeyK => 'K', KeyL => 'L',
        KeyM => 'M', KeyN => 'N', KeyO => 'O', KeyP => 'P', KeyQ => 'Q', KeyR => 'R',
        KeyS => 'S', KeyT => 'T', KeyU => 'U', KeyV => 'V', KeyW => 'W', KeyX => 'X',
        KeyY => 'Y', KeyZ => 'Z',
        Digit0 => '0', Digit1 => '1', Digit2 => '2', Digit3 => '3', Digit4 => '4',
        Digit5 => '5', Digit6 => '6', Digit7 => '7', Digit8 => '8', Digit9 => '9',
        _ => return None,
    };
    Some(c)
}

#[allow(clippy::too_many_arguments)]
fn lobby_input(
    keys: Res<ButtonInput<KeyCode>>,
    net: NonSend<NetRes>,
    session: Res<Session>,
    mut lobby: ResMut<LobbyData>,
    mut next: ResMut<NextState<Screen>>,
) {
    if !lobby.in_lobby {
        // Not in a lobby yet: create one, or type a code and join.
        if keys.just_pressed(KeyCode::Enter) {
            // ENTER with no code = create; with a code = join.
            if lobby.code_input.is_empty() {
                net.0.send(ClientCmd::LobbyCreate { party: session.party.clone() });
            } else {
                net.0.send(ClientCmd::LobbyJoin {
                    code: lobby.code_input.clone(),
                    party: session.party.clone(),
                });
            }
            return;
        }
        if keys.just_pressed(KeyCode::Backspace) {
            lobby.code_input.pop();
        }
        for key in keys.get_just_pressed() {
            if lobby.code_input.len() < 6 {
                if let Some(c) = key_to_code_char(*key) {
                    lobby.code_input.push(c);
                }
            }
        }
        return;
    }

    // In a lobby: ready up, start (host), or leave.
    if keys.just_pressed(KeyCode::KeyR) {
        let want = !lobby.my_ready;
        lobby.my_ready = want;
        net.0.send(ClientCmd::LobbyReady { ready: want });
    }
    if keys.just_pressed(KeyCode::Enter) && lobby.host == session.player_id {
        net.0.send(ClientCmd::LobbyStart);
    }
    if keys.just_pressed(KeyCode::Escape) {
        net.0.send(ClientCmd::LobbyLeave);
        lobby.in_lobby = false;
        lobby.code_input.clear();
        next.set(Screen::Join);
    }
}

fn render_lobby(
    lobby: Res<LobbyData>,
    session: Res<Session>,
    mut q: Query<&mut Text, With<LobbyText>>,
) {
    let Ok(mut t) = q.single_mut() else { return };
    if !lobby.in_lobby {
        **t = format!(
            "Join code: {}_\n\ntype a code + ENTER to join,\nor ENTER (empty) to create a new lobby",
            lobby.code_input
        );
        return;
    }
    let host_is_me = lobby.host == session.player_id;
    let mut lines = vec![format!("Code: {}", lobby.code), String::new()];
    for (id, username, ready) in &lobby.members {
        let you = if id == &session.player_id { " (you)" } else { "" };
        let host = if id == &lobby.host { " [host]" } else { "" };
        let tag = if *ready { "READY" } else { "…" };
        lines.push(format!("  {username}{you}{host}  —  {tag}"));
    }
    lines.push(String::new());
    let all_ready = !lobby.members.is_empty() && lobby.members.iter().all(|(_, _, r)| *r);
    let start = if host_is_me {
        if all_ready { "ENTER: start the dive" } else { "ENTER: start (need everyone READY)" }
    } else {
        "waiting for the host to start…"
    };
    lines.push(format!("R: toggle ready    {start}    ESC: leave"));
    **t = lines.join("\n");
}

// -------------------------------------------------------------- overworld --

/// An overworld action reachable by a keyboard key OR an on-screen (touch) button.
#[derive(Clone, Copy, PartialEq)]
enum OverworldAct {
    Extract,
    TownPortal,
    Join,
}

/// Marks a tappable on-screen action button (touch-native via Bevy UI `Interaction`).
#[derive(Component)]
struct TouchActionButton(OverworldAct);

fn overworld_ui(mut commands: Commands) {
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
                    "WASD/arrows or drag = move · tap = go there · tap yourself = party/inventory · walk into nodes to harvest · T town portal · J join · E portal",
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
                // Harvest is automatic (walk into a node); inventory/party opens by
                // tapping your character — so the bar is just the situational actions.
                for (act, label) in [
                    (OverworldAct::Join, "Join"),
                    (OverworldAct::Extract, "Portal"),
                    (OverworldAct::TownPortal, "Town Portal"),
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
        });
}

/// Position + show/hide the thumbstick from the [`Joystick`] state (touch UI).
#[allow(clippy::type_complexity)]
fn joystick_visual(
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
fn action_button(parent: &mut ChildSpawnerCommands, act: OverworldAct, label: &str) {
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
fn touch_action_buttons(
    q: Query<(&Interaction, &TouchActionButton), Changed<Interaction>>,
    net: NonSend<NetRes>,
    world: Res<Overworld>,
    session: Res<Session>,
    backpack: Res<RunBackpack>,
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
        }
    }
}

/// Display name of the biome band at a floored distance (client-side mirror of
/// the server's structural biome table — display only; the server stays
/// authoritative for what actually spawns).
fn biome_display(d: i64) -> &'static str {
    match d {
        0..=99 => "Forest",
        100..=299 => "Desert",
        300..=499 => "Ashfall",
        500..=999 => "Tundra",
        _ => "Mire",
    }
}

/// Server (x, y) → HD-2D world space: x east, **z = server y** (south, +Z toward
/// the camera parked behind the player). Y is up (height above the ground plane).
fn world_pos(x: f32, y: f32, height: f32) -> Vec3 {
    Vec3::new(x, height, y)
}

/// Exponential rate the rendered overworld positions chase the 20 Hz server
/// snapshots (higher = snappier + less smoothing). Kills the pixel-sprite jitter.
const OW_SMOOTH_RATE: f32 = 16.0;

/// Drive the HD-2D camera each frame: orbit-follow the player, push the live
/// `Look` post params into the camera, aim the sun, and recolour the ground to the
/// player's current biome. Replaces the old flat 2D `follow_camera`.
#[allow(clippy::type_complexity, clippy::too_many_arguments)]
fn hd2d_follow(
    session: Res<Session>,
    look: Res<hd2d::Look>,
    time: Res<Time>,
    assets: Option<Res<WorldAssets>>,
    mut mats: ResMut<Assets<StandardMaterial>>,
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
        *t = hd2d::camera_transform(&look, target, time.elapsed_secs());
        hd2d::apply_post(
            &look,
            &mut proj,
            bloom.map(|b| b.into_inner()),
            dof.map(|d| d.into_inner()),
            fog.map(|f| f.into_inner()),
        );
    }
    // The sun is owned by `apply_sky` (day/night). Swap the ground texture + tint to
    // the player's biome (grass in the forest, sand in the desert, …).
    if let Some(assets) = assets {
        if let Some(m) = mats.get_mut(&assets.ground_mat) {
            let d = (pos.x * pos.x + pos.z * pos.z).sqrt().floor() as i64;
            m.base_color = hd2d_ground_color(d);
            if let Some(tex) = assets.ground_tex.get(biome_index(d)) {
                if m.base_color_texture.as_ref() != Some(tex) {
                    m.base_color_texture = Some(tex.clone());
                }
            }
        }
    }
}

/// Roughly the server's `join_radius` — the client only shows the Join prompt /
/// accepts J within this of a fighting teammate; the server does the real check.
const JOIN_PROMPT_RADIUS: f32 = 9.0;

/// Is the player within join range of a teammate's ongoing fight?
fn near_fight(world: &Overworld, me: Option<(f32, f32)>) -> bool {
    let Some((mx, my)) = me else { return false };
    world
        .entities
        .values()
        .any(|e| e.battling && ((e.x - mx).powi(2) + (e.y - my).powi(2)).sqrt() <= JOIN_PROMPT_RADIUS)
}

/// Draw the guaranteed clear path as a faint glowing trail of ground discs so the
/// feasible route through the terrain reads at a glance. Redraws whenever the trail
/// is gone (e.g. after returning from a battle, where the overworld is despawned).
fn draw_path_trail(
    mut commands: Commands,
    mut world_path: ResMut<WorldPath>,
    existing: Query<Entity, With<PathTrail>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut mats: ResMut<Assets<StandardMaterial>>,
) {
    if world_path.points.len() < 2 {
        return;
    }
    if world_path.drawn && !existing.is_empty() {
        return;
    }
    for e in &existing {
        commands.entity(e).despawn();
    }
    let disc = meshes.add(Circle::new(0.35));
    let mat = mats.add(StandardMaterial {
        base_color: Color::srgba(0.95, 0.9, 0.5, 0.2),
        emissive: LinearRgba::rgb(0.5, 0.45, 0.15),
        unlit: true,
        alpha_mode: AlphaMode::Blend,
        ..default()
    });
    let flat = Quat::from_rotation_x(-std::f32::consts::FRAC_PI_2);
    let step = 2.5_f32; // world units between dots
    for w in world_path.points.windows(2) {
        let (ax, ay) = w[0];
        let (bx, by) = w[1];
        let seg = ((bx - ax).powi(2) + (by - ay).powi(2)).sqrt();
        let n = (seg / step).ceil().max(1.0) as i32;
        for i in 0..=n {
            let t = i as f32 / n as f32;
            let x = ax + (bx - ax) * t;
            let y = ay + (by - ay) * t;
            commands.spawn((
                PathTrail,
                Mesh3d(disc.clone()),
                MeshMaterial3d(mat.clone()),
                Transform::from_translation(world_pos(x, y, 0.05)).with_rotation(flat),
            ));
        }
    }
    world_path.drawn = true;
}

/// Update the distance/biome HUD from the player's authoritative position.
fn update_overworld_hud(
    world: Res<Overworld>,
    session: Res<Session>,
    backpack: Res<RunBackpack>,
    mut q: Query<&mut Text, With<HudText>>,
) {
    let Some(me) = world.entities.get(&session.player_id) else {
        return;
    };
    let d = (me.x * me.x + me.y * me.y).sqrt().floor() as i64;
    // Town Portals first (your way home), then a compact tally of gathered materials.
    let tp = backpack.count("town_portal");
    let mats: String = backpack
        .items
        .iter()
        .filter(|(k, _)| k != "town_portal")
        .map(|(k, q)| format!("{} {}", nice_name(k), q))
        .collect::<Vec<_>>()
        .join(", ");
    let me_pos = Some((me.x, me.y));
    if let Ok(mut t) = q.single_mut() {
        let mut line = format!("distance {d}  -  {}  -  ⌂×{tp}", biome_display(d));
        if !mats.is_empty() {
            line.push_str(&format!("  -  {mats}"));
        }
        if near_fight(&world, me_pos) {
            line.push_str("  -  ⚔ Press [J] to join the fight");
        }
        **t = line;
    }
}

/// Keyboard-only overworld *actions* (E/T/H/J). Movement is device-agnostic in
/// [`gather_steer`] + [`emit_move`]; the touch bar mirrors these actions.
#[allow(clippy::too_many_arguments)]
fn overworld_input(
    keys: Res<ButtonInput<KeyCode>>,
    net: NonSend<NetRes>,
    autoplay: Res<Autoplay>,
    world: Res<Overworld>,
    session: Res<Session>,
    overlay: Res<Overlay>,
    backpack: Res<RunBackpack>,
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
}

/// Harvest resource nodes automatically the moment you walk within reach — so
/// "touching" a node picks it up (and tapping/clicking a distant node just walks
/// you there via tap-to-move, then this fires on arrival). `sent` dedupes so a node
/// isn't requested twice before the server removes it.
fn auto_harvest(
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

/// Open the party + inventory menu (the old-school RPG screen) by **clicking or
/// tapping your own character**. Replaces the inventory key/button. A click is a
/// mouse press+release without a drag (drags orbit the camera); a tap is a touch on
/// the character. Both raycast to the ground and check proximity to the player.
#[allow(clippy::too_many_arguments)]
fn overworld_click_menu(
    mouse: Res<ButtonInput<MouseButton>>,
    touches: Res<Touches>,
    windows: Query<&Window>,
    cam_q: Query<(&Camera, &GlobalTransform), With<Camera3d>>,
    world: Res<Overworld>,
    session: Res<Session>,
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
    let Ok(ray) = cam.viewport_to_world(cam_tf, p) else { return };
    let dv = ray.direction.y;
    if dv.abs() < 1e-6 {
        return;
    }
    let dist = -ray.origin.y / dv;
    if dist <= 0.0 {
        return;
    }
    let hit = ray.get_point(dist);
    if let Some(me) = world.entities.get(&session.player_id) {
        // Tapped on (near) your own avatar → open the party/inventory screen.
        if Vec2::new(hit.x, hit.z).distance(Vec2::new(me.x, me.y)) < 1.8 {
            overlay.kind = Some(OverlayKind::Inventory);
            inv.loaded = false;
            net.0.fetch_inventory();
        }
    }
}

/// Server-frame (x = east, y = south) steering vector for this frame, filled by
/// keyboard, the virtual joystick, or tap-to-move — whichever is active. Consumed
/// by [`emit_move`]. Unifying here is what makes keyboard + touch interchangeable.
#[derive(Resource, Default)]
struct Steer(Vec2);

/// A tap-to-move destination in *server* coords; cleared on arrival or when the
/// player takes direct control (keyboard/joystick).
#[derive(Resource, Default)]
struct TapTarget(Option<Vec2>);

/// The active virtual-joystick touch: its id + on-screen origin + current point.
#[derive(Resource, Default)]
struct Joystick {
    touch: Option<u64>,
    origin: Vec2,
    cur: Vec2,
}

/// Markers for the on-screen thumbstick (base ring + knob).
#[derive(Component)]
struct JoystickBase;
#[derive(Component)]
struct JoystickKnob;

/// Collect this frame's movement from keyboard OR the virtual joystick OR a
/// tap-to-move target, into [`Steer`] (server frame). Priority: direct input
/// (keyboard/joystick) overrides and cancels any tap-to-move.
#[allow(clippy::too_many_arguments)]
fn gather_steer(
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
    if autoplay.0 {
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
fn emit_move(
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

// -------------------------------------------------- overworld overlays -----

/// Open/close the inventory (I) and level-up (L) screens; fetch fresh data on
/// open. ESC closes whichever is up.
#[allow(clippy::too_many_arguments)]
fn overlay_input(
    keys: Res<ButtonInput<KeyCode>>,
    net: NonSend<NetRes>,
    mut overlay: ResMut<Overlay>,
    roster: Res<PartyRoster>,
    mut rename: ResMut<HeroRename>,
) {
    // While renaming a hero on the party screen, capture text and swallow the
    // other overlay hotkeys (so typing a name doesn't toggle screens).
    if let Some(slot) = rename.slot {
        if keys.just_pressed(KeyCode::Escape) {
            rename.slot = None;
            rename.buffer.clear();
            return;
        }
        if keys.just_pressed(KeyCode::Enter) {
            let name = rename.buffer.trim().to_string();
            if !name.is_empty() {
                net.0.send(ClientCmd::RenameHero { slot: slot as i32, name });
            }
            rename.slot = None;
            rename.buffer.clear();
            return;
        }
        if keys.just_pressed(KeyCode::Backspace) {
            rename.buffer.pop();
            return;
        }
        if keys.just_pressed(KeyCode::Space) && rename.buffer.len() < 24 {
            rename.buffer.push(' ');
        }
        for key in keys.get_just_pressed() {
            if let Some(c) = key_to_code_char(*key) {
                if rename.buffer.len() < 24 {
                    rename.buffer.push(c);
                }
            }
        }
        return;
    }

    // ESC closes the menu (which you open by tapping your character — see
    // `overworld_click_menu`; the inventory/level-up keybinds are gone).
    if keys.just_pressed(KeyCode::Escape) {
        overlay.kind = None;
    }
    // On the party screen, digits 1-4 start renaming that hero slot.
    if overlay.kind == Some(OverlayKind::Inventory) {
        let slots = [KeyCode::Digit1, KeyCode::Digit2, KeyCode::Digit3, KeyCode::Digit4];
        for (i, k) in slots.iter().enumerate() {
            if keys.just_pressed(*k) {
                if let Some(h) = roster.heroes.get(i) {
                    rename.slot = Some(i);
                    rename.buffer = h.name.clone();
                }
            }
        }
    }
}

/// Immediate-mode: draw the open overlay (inventory or level-up) as a centered
/// window over a dimmed overworld.
fn render_overlay(
    mut commands: Commands,
    overlay: Res<Overlay>,
    inv: Res<InventoryData>,
    prog: Res<ProgressData>,
    roster: Res<PartyRoster>,
    rename: Res<HeroRename>,
    existing: Query<Entity, With<OverlayRoot>>,
) {
    for e in &existing {
        commands.entity(e).despawn();
    }
    let Some(kind) = overlay.kind else {
        return;
    };
    let label = |p: &mut ChildSpawnerCommands, text: String, size: f32, color: Color| {
        p.spawn((
            Text::new(text),
            TextFont { font_size: size, ..default() },
            TextColor(color),
        ));
    };
    let gold = Color::srgb(0.95, 0.85, 0.5);
    let dim = Color::srgb(0.72, 0.78, 0.9);
    commands
        .spawn((
            OverlayRoot,
            Node {
                position_type: PositionType::Absolute,
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                align_items: AlignItems::Center,
                justify_content: JustifyContent::Center,
                ..default()
            },
            BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.55)),
        ))
        .with_children(|root| {
            root.spawn((
                Node {
                    width: Val::Px(520.0),
                    flex_direction: FlexDirection::Column,
                    row_gap: Val::Px(8.0),
                    padding: UiRect::all(Val::Px(18.0)),
                    border: UiRect::all(Val::Px(2.0)),
                    ..default()
                },
                BorderColor(Color::srgb(0.45, 0.55, 0.85)),
                BackgroundColor(Color::srgb(0.055, 0.075, 0.17)),
                BorderRadius::all(Val::Px(8.0)),
            ))
            .with_children(|p| match kind {
                OverlayKind::Inventory => {
                    label(p, "INVENTORY".into(), 24.0, gold);
                    // Party screen: every hero's name, class, level and stats — this
                    // is where attributes live (not the battle HUD).
                    label(p, "- Party -".into(), 15.0, gold);
                    if roster.heroes.is_empty() {
                        label(p, "  (enter a dive to see your heroes)".into(), 13.0, dim);
                    }
                    for (i, h) in roster.heroes.iter().enumerate() {
                        let name_line = if rename.slot == Some(i) {
                            format!("  [{}] {}_   (typing…)", i + 1, rename.buffer)
                        } else {
                            format!("  [{}] {}   {} · Lv {}", i + 1, h.name, class_display(&h.class_key), h.level)
                        };
                        let name_col = if rename.slot == Some(i) {
                            Color::srgb(0.98, 0.85, 0.4)
                        } else {
                            Color::srgb(0.85, 0.92, 1.0)
                        };
                        label(p, name_line, 16.0, name_col);
                        label(
                            p,
                            format!(
                                "       STR {}  MND {}  DEX {}  WLL {}   HP {}",
                                h.str_, h.mnd, h.dex, h.wll, h.max_hp
                            ),
                            13.0,
                            dim,
                        );
                    }
                    if !roster.heroes.is_empty() {
                        label(p, "  [1-4] rename hero · [Enter] save · [Esc] cancel".into(), 12.0, dim);
                    }
                    if !inv.loaded {
                        label(p, "loading…".into(), 16.0, dim);
                        label(p, "[ESC] close".into(), 13.0, dim);
                        return;
                    }
                    label(p, format!("Chits: {}", inv.chits), 18.0, Color::srgb(0.95, 0.85, 0.5));
                    label(p, "- Materials -".into(), 15.0, gold);
                    if inv.materials.is_empty() {
                        label(p, "  (none)".into(), 14.0, dim);
                    }
                    for (kind, qty) in &inv.materials {
                        label(p, format!("  {} x{}", kind.replace('_', " "), qty), 15.0, dim);
                    }
                    label(p, "- Gear -".into(), 15.0, gold);
                    for g in &inv.gear {
                        let tag = if g.equipped { "[equipped]" } else { "" };
                        label(
                            p,
                            format!(
                                "  {}  atk+{}  dur {}/{} {}",
                                g.name, g.atk_bonus, g.max_durability, g.base_max_durability, tag
                            ),
                            15.0,
                            if g.equipped { Color::srgb(0.6, 0.95, 0.7) } else { dim },
                        );
                    }
                    label(p, "[ESC] close   [1-4] rename hero".into(), 13.0, dim);
                }
                OverlayKind::LevelUp => {
                    label(p, "LEVEL UP".into(), 24.0, gold);
                    if !prog.loaded {
                        label(p, "loading…".into(), 16.0, dim);
                        label(p, "[ESC] close".into(), 13.0, dim);
                        return;
                    }
                    label(p, "- Meld Skills -".into(), 15.0, gold);
                    for s in &prog.skills {
                        label(
                            p,
                            format!("  {:<12} Lv {}   ({} XP)", s.kind.replace('_', " "), s.level, s.xp),
                            16.0,
                            Color::srgb(0.7, 0.85, 1.0),
                        );
                    }
                    label(p, "- Classes Unlocked -".into(), 15.0, gold);
                    let classes = if prog.classes.is_empty() {
                        "  (none)".to_string()
                    } else {
                        format!("  {}", prog.classes.join(", "))
                    };
                    label(p, classes, 15.0, dim);
                    label(p, "[ESC] close   [I] inventory".into(), 13.0, dim);
                }
            });
        });
}

/// Reconcile sprites to the authoritative snapshot: spawn new entities, move
/// known ones, despawn the gone.
/// Reconcile the 3D overworld scene with the latest server snapshot: move entities
/// that persist, spawn newcomers as HD-2D visuals (billboard sprites for players,
/// lit primitives for monsters/portals/resources/terrain), and despawn the gone.
fn sync_overworld_sprites(
    mut commands: Commands,
    world: Res<Overworld>,
    session: Res<Session>,
    look: Res<hd2d::Look>,
    time: Res<Time>,
    wa: Option<Res<WorldAssets>>,
    mut mats: ResMut<Assets<StandardMaterial>>,
    mut q: Query<(Entity, &WorldEntity, &mut Transform)>,
) {
    let Some(wa) = wa else { return };
    // Server snapshots arrive at ~20 Hz; snapping the transform to them each render
    // frame is what made the pixel sprite jitter. Smooth (exponential) toward the
    // target on the ground plane instead — the camera follows the *smoothed* player
    // (see `hd2d_follow`), so sprite and world stay locked together.
    let k = 1.0 - (-time.delta_secs() * OW_SMOOTH_RATE).exp();
    let mut seen = HashSet::new();
    for (entity, we, mut tf) in &mut q {
        if let Some(e) = world.entities.get(&we.0) {
            // Ground-plane position + elevation update; per-kind height/scale/facing
            // stay as spawned (facing is driven by CharSprite/billboard). Raising the
            // parent by the entity's terrace height lifts the whole sprite onto it.
            tf.translation.x += (e.x - tf.translation.x) * k;
            tf.translation.z += (e.y - tf.translation.z) * k;
            let target_y = e.level as f32 * STEP_HEIGHT;
            tf.translation.y += (target_y - tf.translation.y) * k;
            seen.insert(we.0.clone());
        } else {
            commands.entity(entity).despawn();
        }
    }
    for (id, e) in &world.entities {
        if seen.contains(id) {
            continue;
        }
        match e.kind {
            EntityKind::Player => {
                // We only know the local player's lead class (from their party);
                // remote avatars fall back to the Squire.
                let lead = session.party.first().map(|s| s.as_str()).unwrap_or("squire");
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
                // Pick the creature's billboard by content id, else hash into the
                // fallback pool. Tinted faintly warm (like heroes) to stay vibrant
                // under the cool ambient; a fighting creature glows hot.
                let kind = e.name.as_deref().unwrap_or("");
                let tex = wa.monster_sprites.get(kind).cloned().unwrap_or_else(|| {
                    // Unmapped creature → deterministic pick from the fallback pool
                    // (always non-empty, so this never panics).
                    let pool = &wa.monster_pool;
                    pool[hash_pick(id, pool.len().max(1))].clone()
                });
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
                spawn_billboard_entity(&mut commands, &mut mats, &wa, id, e, tex, 1.6, tint, 0.55);
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
                // A real 3D harvest-node model with a glowing ground disc under it so
                // it still reads as gatherable out of the grass.
                let kind = e.name.as_deref().unwrap_or("");
                if let Some((scene, scale)) = wa.resource_scenes.get(kind) {
                    let yaw = (hash_pick(id, 360) as f32).to_radians();
                    commands.spawn((
                        WorldEntity(id.clone()),
                        SceneRoot(scene.clone()),
                        Transform::from_translation(world_pos(e.x, e.y, 0.0))
                            .with_scale(Vec3::splat(*scale))
                            .with_rotation(Quat::from_rotation_y(yaw)),
                    ));
                    commands.spawn((
                        WorldEntity(id.clone()),
                        Mesh3d(wa.glow_disc.clone()),
                        MeshMaterial3d(wa.resource_glow.clone()),
                        Transform::from_translation(world_pos(e.x, e.y, 0.05))
                            .with_rotation(Quat::from_rotation_x(-std::f32::consts::FRAC_PI_2)),
                    ));
                }
            }
            EntityKind::Loot => {
                // A dropped skirmish trophy — a small glowing golden pickup on the
                // grass until a player walks over it. Rendered from the 3D-era assets
                // (glow disc + an emissive gold nub); there's no dedicated loot sprite
                // in the current asset set.
                commands.spawn((
                    WorldEntity(id.clone()),
                    Mesh3d(wa.glow_disc.clone()),
                    MeshMaterial3d(wa.resource_glow.clone()),
                    Transform::from_translation(world_pos(e.x, e.y, 0.05))
                        .with_rotation(Quat::from_rotation_x(-std::f32::consts::FRAC_PI_2)),
                ));
                let nub = mats.add(StandardMaterial {
                    base_color: Color::srgb(1.0, 0.85, 0.35),
                    emissive: LinearRgba::rgb(1.6, 1.2, 0.4),
                    unlit: true,
                    ..default()
                });
                commands.spawn((
                    WorldEntity(id.clone()),
                    Mesh3d(wa.rock_mesh.clone()),
                    MeshMaterial3d(nub),
                    Transform::from_translation(world_pos(e.x, e.y, 0.35))
                        .with_scale(Vec3::splat(0.32)),
                ));
            }
            EntityKind::Obstacle => {
                spawn_obstacle(&mut commands, &mut mats, &wa, id, e);
            }
        }
    }
}

/// Spawn a player's overworld avatar: a ground-anchored, walk-animated psyker
/// billboard (the placeholder for every class until per-class sprites land) with a
/// soft contact shadow. Tinted so you (white) read apart from allies/fighters.
#[allow(clippy::too_many_arguments)]
fn spawn_player_avatar(
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
    // class (Resonant has no sprite yet → the Squire stands in).
    let frames = match class {
        "psyker" => &wa.psyker,
        _ => &wa.squire,
    };
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
                Transform::from_xyz(0.0, look.sprite_y, 0.0),
                hd2d::Billboard,
                hd2d::HeroBillboard,
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

/// Deterministically pick an index in `0..n` from an entity id (FNV-1a). Lets a
/// grove of identical-kind obstacles show varied art without any per-entity state.
fn hash_pick(id: &str, n: usize) -> usize {
    if n == 0 {
        return 0;
    }
    let mut h: u32 = 2166136261;
    for b in id.bytes() {
        h = (h ^ b as u32).wrapping_mul(16777619);
    }
    (h as usize) % n
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
fn build_terrain_sections(
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
            // Grassy plateau top: the grass texture over a SATURATED green base, so it
            // reads as grass in every light (a pale base washes teal under the cool
            // rain/dusk ambient). Terraces wear grass in all biomes — a mesa's top.
            let grass_tex = wa.ground_tex.first().cloned(); // grass0.png
            let top_mat = mats.add(StandardMaterial {
                base_color: Color::srgb(0.36, 0.6, 0.26),
                base_color_texture: grass_tex,
                perceptual_roughness: 0.95,
                cull_mode: None,
                ..default()
            });
            // A dirt-textured backing mesh so the cliff reads as an earthy wall and
            // never shows a gap behind the cliff models that dress the edges.
            let dirt_tex = wa.ground_tex.get(2).cloned(); // dirt_full.png
            let cliff_mat = mats.add(StandardMaterial {
                base_color: Color::srgb(0.62, 0.5, 0.38),
                base_color_texture: dirt_tex,
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
            // Dress the terrace edges with real Kenney cliff_rock models.
            spawn_terrace_cliffs(&mut commands, &assets, sec, *idx);
        } else {
            // Flat section (e.g. the tutorial): record it as built so we don't
            // rescan it every frame, but draw nothing.
            commands.spawn((TerrainMesh(*idx), Transform::default(), Visibility::Hidden));
        }
        // The ladders / ropes / slopes that make each terrace reachable.
        for c in &sec.connectors {
            spawn_connector(&mut commands, &mut meshes, &mut mats, *idx, c);
        }
    }
}

/// Dress a section's terrace edges with real Kenney **cliff_rock** models: one per
/// boundary cell (a raised cell with a lower neighbour), facing outward, so the
/// terraces read as rocky cliffs rather than flat brown walls. The grass-top mesh
/// covers the surface; these give the rocky face; the backing cliff mesh fills any
/// gaps behind them.
fn spawn_terrace_cliffs(
    commands: &mut Commands,
    assets: &AssetServer,
    sec: &meld_client::net::TerrainSectionView,
    idx: u32,
) {
    // Blockier cliff pieces (flat rock faces) read as a clean terrace wall; the
    // rounded cliff_rock is kept as an occasional accent.
    let cliffs: [Handle<Scene>; 3] = [
        assets.load(GltfAssetLabel::Scene(0).from_asset("models/nature/cliff_block_rock.glb")),
        assets.load(GltfAssetLabel::Scene(0).from_asset("models/nature/cliff_block_rock.glb")),
        assets.load(GltfAssetLabel::Scene(0).from_asset("models/nature/cliff_rock.glb")),
    ];
    let cols = sec.cols as usize;
    let rows = sec.rows as usize;
    let cell = sec.cell as f32;
    let sx = sec.start_x as f32;
    let zmin = sec.y_min as f32;
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
            let dir = dir.normalize_or_zero();
            let cx = sx + (gx as f32 + 0.5) * cell;
            let cz = zmin + (gy as f32 + 0.5) * cell;
            let by = lowest as f32 * STEP_HEIGHT;
            let yaw = dir.x.atan2(dir.y) + CLIFF_YAW_OFFSET;
            let scene = cliffs[(gx + gy) % cliffs.len()].clone();
            commands.spawn((
                TerrainMesh(idx),
                SceneRoot(scene),
                Transform::from_xyz(cx, by, cz)
                    .with_scale(Vec3::splat(CLIFF_EDGE_SCALE))
                    .with_rotation(Quat::from_rotation_y(yaw)),
            ));
            placed += 1;
            if placed > 400 {
                return; // safety cap on a pathological section
            }
        }
    }
}

/// Append a quad (two triangles) with a flat `normal` and per-corner `uv`. Winding
/// is fixed; the terrace materials render double-sided so face direction never
/// hides a surface.
#[allow(clippy::too_many_arguments)]
fn push_quad(
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
fn terrace_meshes(sec: &meld_client::net::TerrainSectionView) -> (Mesh, Mesh) {
    use bevy::render::mesh::{Indices, PrimitiveTopology};
    use bevy::render::render_asset::RenderAssetUsages;
    let cols = sec.cols as usize;
    let rows = sec.rows as usize;
    let cell = sec.cell as f32;
    let sx = sec.start_x as f32;
    let zmin = sec.y_min as f32;
    let tile = 0.22f32; // texture repeats per world unit
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
            // Terrace top.
            push_quad(
                &mut tp, &mut tn, &mut tu, &mut ti,
                [x0, topy, z0], [x1, topy, z0], [x1, topy, z1], [x0, topy, z1],
                [0.0, 1.0, 0.0],
                [[x0 * tile, z0 * tile], [x1 * tile, z0 * tile], [x1 * tile, z1 * tile], [x0 * tile, z1 * tile]],
            );
            // Cliff faces toward any lower neighbour (outside grid counts as level 0).
            let mut face = |gx2: i64, gy2: i64, quad: [[f32; 3]; 4], normal: [f32; 3]| {
                let nl = lvl(gx2, gy2);
                if (nl as f32) < l as f32 {
                    let by = nl as f32 * STEP_HEIGHT;
                    let hh = (topy - by) * tile;
                    let mut q = quad;
                    q[0][1] = by;
                    q[1][1] = by;
                    push_quad(
                        &mut cp, &mut cn, &mut cu, &mut ci, q[0], q[1], q[2], q[3], normal,
                        [[0.0, 0.0], [cell * tile, 0.0], [cell * tile, hh], [0.0, hh]],
                    );
                }
            };
            // -Z, +Z, -X, +X. Bottom two verts' Y is overwritten inside `face`.
            face(gx as i64, gy as i64 - 1, [[x1, 0.0, z0], [x0, 0.0, z0], [x0, topy, z0], [x1, topy, z0]], [0.0, 0.0, -1.0]);
            face(gx as i64, gy as i64 + 1, [[x0, 0.0, z1], [x1, 0.0, z1], [x1, topy, z1], [x0, topy, z1]], [0.0, 0.0, 1.0]);
            face(gx as i64 - 1, gy as i64, [[x0, 0.0, z0], [x0, 0.0, z1], [x0, topy, z1], [x0, topy, z0]], [-1.0, 0.0, 0.0]);
            face(gx as i64 + 1, gy as i64, [[x1, 0.0, z1], [x1, 0.0, z0], [x1, topy, z0], [x1, topy, z1]], [1.0, 0.0, 0.0]);
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
fn spawn_connector(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    mats: &mut Assets<StandardMaterial>,
    idx: u32,
    c: &meld_client::net::ConnectorView,
) {
    let lo_y = c.lo as f32 * STEP_HEIGHT;
    let hi_y = c.hi as f32 * STEP_HEIGHT;
    let h = (hi_y - lo_y).max(0.2);
    let x = c.x as f32;
    // Stand the prop a touch proud of the cliff base (toward the camera / −Z) so it
    // reads as a distinct affordance and isn't swallowed by the cliff face.
    let z = c.y as f32 - 0.5; // overworld y → world Z
    let mid_y = (lo_y + hi_y) * 0.5;
    // Bold, warm, emissive so the route up a cliff is unmistakable (and findable in
    // shade) — the same "legible route" spirit as the glowing path trail.
    let (mesh, color, emissive, transform) = match c.kind.as_str() {
        "slope" => {
            // A ramp board rising from the ground (−Z) up to the terrace lip (+Z).
            let run = h * 1.8;
            let len = (run * run + h * h).sqrt();
            let angle = h.atan2(run);
            (
                meshes.add(Cuboid::new(2.6, 0.22, len)),
                Color::srgb(0.72, 0.62, 0.5),
                LinearRgba::new(0.28, 0.22, 0.12, 1.0),
                Transform::from_xyz(x, mid_y, z + run * 0.5)
                    .with_rotation(Quat::from_rotation_x(-angle)),
            )
        }
        "rope" => (
            meshes.add(Cuboid::new(0.22, h * 1.05, 0.22)),
            Color::srgb(0.95, 0.8, 0.42),
            LinearRgba::new(0.5, 0.36, 0.12, 1.0),
            Transform::from_xyz(x, mid_y, z),
        ),
        _ => (
            // ladder: an upright post, bright wood, glowing rungs implied by emissive.
            meshes.add(Cuboid::new(1.0, h * 1.05, 0.22)),
            Color::srgb(0.9, 0.62, 0.28),
            LinearRgba::new(0.55, 0.34, 0.1, 1.0),
            Transform::from_xyz(x, mid_y, z),
        ),
    };
    let mat = mats.add(StandardMaterial {
        base_color: color,
        emissive,
        perceptual_roughness: 0.85,
        ..default()
    });
    commands.spawn((
        TerrainMesh(idx),
        Mesh3d(mesh),
        MeshMaterial3d(mat),
        transform,
    ));
}

fn spawn_billboard_entity(
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
fn spawn_obstacle(
    commands: &mut Commands,
    mats: &mut Assets<StandardMaterial>,
    wa: &WorldAssets,
    id: &str,
    e: &OwEntity,
) {
    let name = e.name.as_deref().unwrap_or("");
    let r = e.radius.max(0.4);
    let col = obstacle_color(name);
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
            // Shared animated water material (scrolled by `animate_water`); spin each
            // organic blob a different way so pools don't look stamped from one shape.
            let spin = (hash_pick(id, 360) as f32).to_radians();
            commands.spawn((
                WorldEntity(id.to_string()),
                Mesh3d(wa.water_mesh.clone()),
                MeshMaterial3d(wa.water_mat.clone()),
                Transform::from_translation(world_pos(e.x, e.y, 0.04))
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
fn nice_name(kind: &str) -> String {
    kind.replace('_', " ")
}

/// Title-case a class key for display (`alchemist_knight` → `Alchemist Knight`).
fn class_display(key: &str) -> String {
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
fn obstacle_color(kind: &str) -> Color {
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
fn faction_color(faction: &str) -> Color {
    let mut h: u32 = 2166136261;
    for b in faction.bytes() {
        h = (h ^ b as u32).wrapping_mul(16777619);
    }
    Color::hsl((h % 360) as f32, 0.62, 0.56)
}

fn clear_overworld_sprites(mut commands: Commands, q: Query<Entity, With<WorldEntity>>) {
    for e in &q {
        commands.entity(e).despawn();
    }
}

// ---------------------------------------------------------------- battle ---

/// Reset the command window to its root page, clearing any pending target choice.
fn reset_menu(menu: &mut BattleMenu) {
    menu.level = MenuLevel::Root;
    menu.cursor = 0;
    menu.dirty = true;
    menu.pending = None;
    menu.rows.clear();
}

/// On entering a battle, open the command window on the root page.
fn enter_battle(mut menu: ResMut<BattleMenu>) {
    reset_menu(&mut menu);
}

/// A 3D combatant in the HD-2D battle arena, keyed by its combatant id.
#[derive(Component)]
struct BattleActor {
    id: String,
}

/// Reconcile the 3D battle arena with `BattleData`: your party as character
/// billboards on the near side (facing the foe, Octopath-style backs), enemies as
/// lit capsule stand-ins on the far side. The HP/ATB/command UI frames it.
fn sync_battle_actors(
    mut commands: Commands,
    battle: Res<BattleData>,
    wa: Option<Res<WorldAssets>>,
    mut mats: ResMut<Assets<StandardMaterial>>,
    q: Query<(Entity, &BattleActor)>,
) {
    let Some(wa) = wa else { return };
    let mut seen = HashSet::new();
    for (ent, a) in &q {
        if battle.combatants.iter().any(|c| c.id == a.id) {
            seen.insert(a.id.clone());
        } else {
            commands.entity(ent).despawn();
        }
    }
    let party: Vec<&CombatantView> = battle.combatants.iter().filter(|c| c.is_player).collect();
    let enemies: Vec<&CombatantView> = battle.combatants.iter().filter(|c| !c.is_player).collect();
    let spread = |i: usize, n: usize, gap: f32| (i as f32 - (n.max(1) as f32 - 1.0) * 0.5) * gap;

    for (i, c) in party.iter().enumerate() {
        if seen.contains(&c.id) {
            continue;
        }
        let class = c
            .statuses
            .iter()
            .find_map(|s| s.strip_prefix("class:"))
            .unwrap_or("squire");
        let frames = match class {
            "psyker" => &wa.psyker,
            _ => &wa.squire,
        };
        let root = Vec3::new(spread(i, party.len(), 2.7), 0.0, 1.2);
        let mat = mats.add(hd2d::sprite_material(
            Color::srgb(1.2, 1.18, 1.08),
            frames.idle[0].clone(),
        ));
        let mut cs = CharSprite::new(frames.clone(), mat.clone(), root);
        cs.facing = Vec2::new(0.0, -1.0); // face the enemies (north)
        commands
            .spawn((
                BattleActor { id: c.id.clone() },
                Transform::from_translation(root),
                Visibility::default(),
                cs,
            ))
            .with_children(|p| {
                p.spawn((
                    Mesh3d(wa.sprite_quad.clone()),
                    MeshMaterial3d(mat),
                    Transform::from_xyz(0.0, 0.72, 0.0),
                    hd2d::Billboard,
                    hd2d::HeroBillboard,
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
    for (i, c) in enemies.iter().enumerate() {
        if seen.contains(&c.id) {
            continue;
        }
        // Same creature billboard the overworld uses: look up by content id (the
        // combatant name with underscores), else hash into the fallback pool.
        let kind = c.name.replace(' ', "_");
        let tex = wa.monster_sprites.get(&kind).cloned().unwrap_or_else(|| {
            // Unmapped creature → deterministic pick from the fallback pool
            // (always non-empty, so this never panics).
            let pool = &wa.monster_pool;
            pool[hash_pick(&c.id, pool.len().max(1))].clone()
        });
        let h = 3.4;
        let root = Vec3::new(spread(i, enemies.len(), 3.6), 0.0, -4.5);
        let mat = mats.add(hd2d::sprite_material(Color::srgb(1.2, 1.15, 1.1), tex));
        commands
            .spawn((
                BattleActor { id: c.id.clone() },
                Transform::from_translation(root),
                Visibility::default(),
            ))
            .with_children(|p| {
                p.spawn((
                    Mesh3d(wa.sprite_quad.clone()),
                    MeshMaterial3d(mat),
                    Transform::from_xyz(0.0, h * 0.5, 0.0).with_scale(Vec3::splat(h / 2.2)),
                    hd2d::Billboard,
                ));
                p.spawn((
                    Mesh3d(wa.shadow_mesh.clone()),
                    MeshMaterial3d(wa.shadow_mat.clone()),
                    Transform::from_xyz(0.0, 0.02, 0.0)
                        .with_rotation(Quat::from_rotation_x(-std::f32::consts::FRAC_PI_2))
                        .with_scale(Vec3::new(h * 0.42, h * 0.23, 1.0)),
                ));
            });
    }
}

/// Frame the HD-2D battle arena: a fixed 3/4 view of the two rows, with the live
/// `Look` post stack. (Overworld `hd2d_follow` doesn't run here.)
#[allow(clippy::type_complexity)]
fn battle_camera(
    look: Res<hd2d::Look>,
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
    if let Ok((mut t, mut proj, bloom, dof, fog)) = cam_q.single_mut() {
        *t = Transform::from_translation(Vec3::new(0.0, 9.5, 12.5))
            .looking_at(Vec3::new(0.0, 0.9, -1.6), Vec3::Y);
        hd2d::apply_post(
            &look,
            &mut proj,
            bloom.map(|b| b.into_inner()),
            dof.map(|d| d.into_inner()),
            fog.map(|f| f.into_inner()),
        );
    }
    // Sun owned by `apply_sky` (day/night cycle).
}

/// Queue an order (with its chosen target) for `hero`, then hand focus to the next
/// hero that still needs one — preferring a hero whose ATB is already full
/// ([`pick_active`]). The order fires the instant that hero is ready
/// ([`auto_fire_queued`]).
fn queue_order(
    battle: &mut BattleData,
    hero: &str,
    kind: QueuedKind,
    target: Option<String>,
    menu: &mut BattleMenu,
) {
    battle.queued.insert(hero.to_string(), Order { kind, target });
    battle.active = pick_active(battle).or_else(|| Some(hero.to_string()));
    reset_menu(menu);
}

/// Begin an order for `hero`: self-cast orders queue immediately; aimed orders open the
/// Target picker (auto-picking when only one valid target exists).
fn begin_order(battle: &mut BattleData, menu: &mut BattleMenu, hero: &str, kind: QueuedKind) {
    match order_side(kind) {
        None => queue_order(battle, hero, kind, None, menu),
        Some(side) => {
            let targets = valid_targets(battle, side);
            match targets.len() {
                0 => reset_menu(menu), // nothing valid to hit — abandon the choice
                1 => queue_order(battle, hero, kind, Some(targets[0].1.clone()), menu),
                _ => {
                    menu.pending = Some((hero.to_string(), kind));
                    menu.rows = targets;
                    open_page(menu, MenuLevel::Target);
                }
            }
        }
    }
}

/// Living combatants on `side`, as `(label, id)` rows for the Target picker. Allies are
/// every living player combatant — including co-op heroes who joined the battle (they
/// live in `combatants`, not `your_ids`).
fn valid_targets(battle: &BattleData, side: Side) -> Vec<(String, String)> {
    battle
        .combatants
        .iter()
        .filter(|c| c.hp > 0 && (side == Side::Ally) == c.is_player)
        .map(|c| {
            let name = if c.is_player { battle.hero_label(&c.id) } else { c.name.clone() };
            (format!("{}  {}/{}", name, c.hp, c.max_hp), c.id.clone())
        })
        .collect()
}

/// A default target for an order lacking an explicit pick (autoplay, or a queued target
/// that died before firing): first living enemy for offensive orders, most-wounded
/// living ally for supportive ones. `None` when nothing valid remains.
fn default_target(battle: &BattleData, kind: QueuedKind) -> Option<String> {
    match order_side(kind) {
        Some(Side::Enemy) => battle
            .combatants
            .iter()
            .find(|c| !c.is_player && c.hp > 0)
            .map(|c| c.id.clone()),
        Some(Side::Ally) => battle
            .combatants
            .iter()
            .filter(|c| c.is_player && c.hp > 0)
            .min_by(|a, b| {
                let fa = a.hp as f32 / a.max_hp.max(1) as f32;
                let fb = b.hp as f32 / b.max_hp.max(1) as f32;
                fa.total_cmp(&fb)
            })
            .map(|c| c.id.clone()),
        None => None,
    }
}

/// A sensible active hero: prefer one that's ready and unordered, then any
/// unordered hero, then any live hero.
fn pick_active(battle: &BattleData) -> Option<String> {
    let alive: Vec<&String> = battle.your_ids.iter().filter(|h| battle.alive(h)).collect();
    alive
        .iter()
        .find(|h| battle.ready.contains(**h) && !battle.queued.contains_key(**h))
        .or_else(|| alive.iter().find(|h| !battle.queued.contains_key(**h)))
        .or_else(|| alive.first())
        .map(|h| (*h).clone())
}

/// Send a hero's order to the server, aimed at `target` (the combatant the player
/// chose; already validated/retargeted by [`auto_fire_queued`]).
fn fire_order(net: &Net, battle_id: &str, actor: &str, kind: QueuedKind, target: Option<&str>) {
    let cmd = match kind {
        QueuedKind::Attack => target.map(|t| ClientCmd::Attack {
            battle_id: battle_id.to_string(),
            actor: actor.to_string(),
            target: t.to_string(),
        }),
        QueuedKind::Skill(sk) => target.map(|t| ClientCmd::Skill {
            battle_id: battle_id.to_string(),
            actor: actor.to_string(),
            target: t.to_string(),
            skill_kind: sk.to_string(),
        }),
        QueuedKind::Defend => Some(ClientCmd::Defend {
            battle_id: battle_id.to_string(),
            actor: actor.to_string(),
        }),
        // Items heal the chosen ally (server falls back to the actor for an empty id).
        QueuedKind::Item(it) => Some(ClientCmd::Item {
            battle_id: battle_id.to_string(),
            actor: actor.to_string(),
            item_id: it.to_string(),
            target: target.unwrap_or(actor).to_string(),
        }),
        // Psyker Focus ops ride the Skill action with a `verb:kind` skill_kind; the
        // aimed enemy (for offensive Foci) travels as the target.
        QueuedKind::Focus(verb, kind) => Some(ClientCmd::Skill {
            battle_id: battle_id.to_string(),
            actor: actor.to_string(),
            target: target.unwrap_or("").to_string(),
            skill_kind: format!("{verb}:{kind}"),
        }),
        QueuedKind::Hold => Some(ClientCmd::Skill {
            battle_id: battle_id.to_string(),
            actor: actor.to_string(),
            target: target.unwrap_or("").to_string(),
            skill_kind: "hold".to_string(),
        }),
    };
    if let Some(cmd) = cmd {
        net.send(cmd);
    }
}

/// Keep `active` on a live, controllable hero and auto-focus the ready one: re-pick
/// whenever the active hero is gone or already has a queued order, so focus follows
/// the ATB. Frozen while the Target picker is open (the pending actor owns the turn).
fn validate_active(mut battle: ResMut<BattleData>, menu: Res<BattleMenu>) {
    if menu.level == MenuLevel::Target {
        return;
    }
    let needs_repick = match &battle.active {
        Some(a) => {
            !(battle.your_ids.contains(a) && battle.alive(a)) || battle.queued.contains_key(a)
        }
        None => true,
    };
    if needs_repick {
        battle.active = pick_active(&battle);
    }
}

/// Fire every hero whose gauge is full and who has a queued order, at its chosen
/// target — retargeting to a sensible default if that target died while the gauge filled.
fn auto_fire_queued(net: NonSend<NetRes>, mut battle: ResMut<BattleData>) {
    let battle_id = battle.battle_id.clone();
    let ready_orders: Vec<(String, Order)> = battle
        .your_ids
        .iter()
        .filter(|h| battle.ready.contains(*h))
        .filter_map(|h| battle.queued.get(h).map(|o| (h.clone(), o.clone())))
        .collect();
    for (hero, order) in ready_orders {
        let target = order
            .target
            .filter(|t| battle.alive(t))
            .or_else(|| default_target(&battle, order.kind));
        fire_order(&net.0, &battle_id, &hero, order.kind, target.as_deref());
        battle.ready.remove(&hero);
        battle.queued.remove(&hero);
    }
}

/// The `&'static str` manifestation kind matching a dynamic `kind` string (from a
/// combatant's parsed foci), or `None` if it isn't a known manifestation.
fn manifest_static(kind: &str) -> Option<&'static str> {
    MANIFESTS.iter().find(|(k, _, _)| *k == kind).map(|(k, _, _)| *k)
}

/// Cast vs reinforce for a Psyker picking `kind`: reinforce if that manifestation is
/// already active on the hero, else cast. Mirrors the server's slot logic so the
/// unified menu "just reinforces" a live Focus.
fn manifest_verb(battle: &BattleData, hero: &str, kind: &str) -> &'static str {
    let active = battle
        .view(hero)
        .map(|v| parse_foci(&v.statuses).1)
        .unwrap_or_default();
    if active.iter().any(|(k, _)| k == kind) {
        "reinforce"
    } else {
        "cast"
    }
}

/// Act on the command row at `index`. Root/list rows come from [`menu_entries`]; the
/// dynamic Target/Revoke pages index into [`BattleMenu::rows`] (with a trailing Back).
/// Order-producing rows route through [`begin_order`], which opens the Target picker
/// when the action needs aiming.
fn select_entry(index: usize, menu: &mut BattleMenu, battle: &mut BattleData, class: &str) {
    let active = match battle.active.clone() {
        Some(a) => a,
        None => return,
    };

    // Dynamic pages: `menu.rows` then a trailing Back row.
    if matches!(menu.level, MenuLevel::Target | MenuLevel::Revoke) {
        let Some((_, value)) = menu.rows.get(index).cloned() else {
            reset_menu(menu); // the Back row (or out of range)
            return;
        };
        match menu.level {
            MenuLevel::Target => match menu.pending.clone() {
                Some((actor, kind)) => queue_order(battle, &actor, kind, Some(value), menu),
                None => reset_menu(menu),
            },
            MenuLevel::Revoke => match manifest_static(&value) {
                Some(kind) => queue_order(battle, &active, QueuedKind::Focus("revoke", kind), None, menu),
                None => reset_menu(menu),
            },
            _ => unreachable!(),
        }
        return;
    }

    let hero_level = battle.view(&active).map(|c| c.level).unwrap_or(1);
    let entries = menu_entries(menu.level, class, hero_level);
    let Some(entry) = entries.get(index) else {
        return;
    };
    match entry.action {
        EntryAction::Attack => begin_order(battle, menu, &active, QueuedKind::Attack),
        EntryAction::Defend => begin_order(battle, menu, &active, QueuedKind::Defend),
        EntryAction::OpenSkills => open_page(menu, MenuLevel::Skills),
        EntryAction::OpenItems => open_page(menu, MenuLevel::Items),
        EntryAction::Skill(kind) => begin_order(battle, menu, &active, QueuedKind::Skill(kind)),
        EntryAction::Item(id) => begin_order(battle, menu, &active, QueuedKind::Item(id)),
        // Psyker: Focus opens the manifestation list; Revoke lists the live Foci.
        EntryAction::OpenManifest => open_page(menu, MenuLevel::Manifest),
        EntryAction::OpenRevoke => open_revoke_page(menu, battle, &active),
        // Cast, or reinforce if already active; begin_order aims offensive ones.
        EntryAction::Manifest(kind) => {
            let verb = manifest_verb(battle, &active, kind);
            begin_order(battle, menu, &active, QueuedKind::Focus(verb, kind));
        }
        EntryAction::Hold => begin_order(battle, menu, &active, QueuedKind::Hold),
        EntryAction::Back => reset_menu(menu),
    }
}

/// Build the Revoke page rows from the hero's live Foci and open it (staying at root
/// if there is nothing to revoke).
fn open_revoke_page(menu: &mut BattleMenu, battle: &BattleData, hero: &str) {
    let foci = battle
        .view(hero)
        .map(|v| parse_foci(&v.statuses).1)
        .unwrap_or_default();
    menu.rows = foci
        .iter()
        .filter_map(|(kind, stacks)| {
            MANIFESTS
                .iter()
                .find(|(k, _, _)| *k == kind.as_str())
                .map(|(k, name, _)| (format!("{name}  x{stacks}"), (*k).to_string()))
        })
        .collect();
    if menu.rows.is_empty() {
        reset_menu(menu);
    } else {
        open_page(menu, MenuLevel::Revoke);
    }
}

/// Switch the command window to a sub-page.
fn open_page(menu: &mut BattleMenu, level: MenuLevel) {
    menu.level = level;
    menu.cursor = 0;
    menu.dirty = true;
}

/// Number of selectable rows on the current page. Static pages come from
/// [`menu_entries`]; the dynamic Target/Revoke pages are `rows` plus a Back row.
fn page_len(menu: &BattleMenu, class: &str, hero_level: i32) -> usize {
    match menu.level {
        MenuLevel::Target | MenuLevel::Revoke => menu.rows.len() + 1,
        level => menu_entries(level, class, hero_level).len(),
    }
}

/// Keyboard control. Orders are *queued* for the active hero and fire when its
/// ATB fills. At root: 1-4 pick which hero to command; A/D/I/S choose a command;
/// ↑/↓ + ENTER also work. In a sub-page: 1-N / ↑↓+ENTER pick, ESC backs out.
/// Autoplay queues Attack for every hero.
fn menu_keyboard(
    keys: Res<ButtonInput<KeyCode>>,
    autoplay: Res<Autoplay>,
    mut menu: ResMut<BattleMenu>,
    mut battle: ResMut<BattleData>,
) {
    // The command menu keys off the *active hero's* class — a mixed party is
    // commanded hero by hero.
    let class = battle.active_class();
    if autoplay.0 {
        let idle: Vec<String> = battle
            .your_ids
            .iter()
            .filter(|h| battle.alive(h) && !battle.queued.contains_key(*h))
            .cloned()
            .collect();
        for h in idle {
            // Each hero autoplays by its own class: Psyker channels Foci, Resonant
            // mends the party, everyone else swings — each at a sensible default target.
            let hc = battle.view(&h).map(hero_class).unwrap_or_else(|| "squire".into());
            let kind = match hc.as_str() {
                "psyker" => battle.view(&h).map(psyker_autoplay_op).unwrap_or(QueuedKind::Hold),
                "resonant" => resonant_autoplay_op(&battle),
                _ => QueuedKind::Attack,
            };
            let target = default_target(&battle, kind);
            battle.queued.insert(h, Order { kind, target });
        }
        return;
    }

    let hero_level = battle.active_level();
    let n = page_len(&menu, &class, hero_level).max(1);
    if keys.just_pressed(KeyCode::ArrowDown) {
        menu.cursor = (menu.cursor + 1) % n;
    }
    if keys.just_pressed(KeyCode::ArrowUp) {
        menu.cursor = (menu.cursor + n - 1) % n;
    }
    if (keys.just_pressed(KeyCode::Escape) || keys.just_pressed(KeyCode::Backspace))
        && menu.level != MenuLevel::Root
    {
        reset_menu(&mut menu);
        return;
    }

    let digits = [
        KeyCode::Digit1,
        KeyCode::Digit2,
        KeyCode::Digit3,
        KeyCode::Digit4,
    ];
    // At root, number keys pick the hero to command; in a sub-page, the row.
    if menu.level == MenuLevel::Root {
        for (i, key) in digits.iter().enumerate() {
            if i < battle.your_ids.len() && keys.just_pressed(*key) {
                battle.active = Some(battle.your_ids[i].clone());
                return;
            }
        }
        let hotkey = if keys.just_pressed(KeyCode::KeyA) {
            Some(0)
        } else if keys.just_pressed(KeyCode::KeyD) {
            Some(1)
        } else if keys.just_pressed(KeyCode::KeyI) {
            Some(2)
        } else if keys.just_pressed(KeyCode::KeyS) {
            Some(3)
        } else {
            None
        };
        if let Some(i) = hotkey {
            menu.cursor = i;
            select_entry(i, &mut menu, &mut battle, &class);
            return;
        }
    } else {
        for (i, key) in digits.iter().enumerate() {
            if i < n && keys.just_pressed(*key) {
                menu.cursor = i;
                select_entry(i, &mut menu, &mut battle, &class);
                return;
            }
        }
    }

    if keys.just_pressed(KeyCode::Enter) || keys.just_pressed(KeyCode::Space) {
        select_entry(menu.cursor, &mut menu, &mut battle, &class);
    }
}

/// Mouse/touch: pressing a command row queues it for the active hero.
fn menu_click(
    mut menu: ResMut<BattleMenu>,
    mut battle: ResMut<BattleData>,
    rows: Query<(&Interaction, &MenuRow), Changed<Interaction>>,
) {
    let mut pressed = None;
    for (interaction, row) in &rows {
        if *interaction == Interaction::Pressed {
            pressed = Some(row.index);
        }
    }
    if let Some(index) = pressed {
        menu.cursor = index;
        let class = battle.active_class();
        select_entry(index, &mut menu, &mut battle, &class);
    }
}

/// One command tile in the cross, tagged with its menu-entry index.
fn cmd_tile(parent: &mut ChildSpawnerCommands, index: usize, label: &str) {
    parent
        .spawn((
            Button,
            MenuRow { index },
            Node {
                width: Val::Px(58.0),
                height: Val::Px(48.0),
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                border: UiRect::all(Val::Px(2.0)),
                ..default()
            },
            BorderColor(Color::srgb(0.5, 0.55, 0.7)),
            BackgroundColor(Color::srgb(0.1, 0.12, 0.22)),
            BorderRadius::all(Val::Px(5.0)),
        ))
        .with_children(|t| {
            t.spawn((
                Text::new(label.to_string()),
                TextFont {
                    font_size: 15.0,
                    ..default()
                },
                TextColor(Color::srgb(0.92, 0.94, 1.0)),
            ));
        });
}

/// Rebuild the command UI when the page changes. Root shows a Lufia-style cross
/// of command tiles (Attack centre, Skill up, Item left, Defend right), centred
/// over the party grid; Skill/Item pages show a compact list. Rebuilt only on a
/// page change so button `Interaction` survives.
fn rebuild_command_menu(
    mut commands: Commands,
    battle: Res<BattleData>,
    mut menu: ResMut<BattleMenu>,
    existing: Query<Entity, With<CommandWindow>>,
) {
    if !menu.dirty {
        return;
    }
    menu.dirty = false;
    for e in &existing {
        commands.entity(e).despawn();
    }
    let level = menu.level;
    let class = battle.active_class();
    let is_psyker = class == "psyker";
    let hero_level = battle.active_level();
    // Row labels: the dynamic Target/Revoke pages draw from `menu.rows` (+ a Back row);
    // every other page comes from `menu_entries`. The martial classes keep the Lufia
    // cross at root; everything else uses the list renderer below.
    let labels: Vec<String> = match level {
        MenuLevel::Target | MenuLevel::Revoke => menu
            .rows
            .iter()
            .map(|(l, _)| l.clone())
            .chain(std::iter::once("Back".to_string()))
            .collect(),
        _ => menu_entries(level, &class, hero_level)
            .into_iter()
            .map(|e| e.label.to_string())
            .collect(),
    };
    commands
        .spawn((
            CommandWindow,
            Node {
                position_type: PositionType::Absolute,
                left: Val::Px(12.0),
                right: Val::Px(12.0),
                bottom: Val::Px(142.0), // just above the compact party HUD row
                flex_direction: FlexDirection::Row,
                justify_content: JustifyContent::Center,
                ..default()
            },
        ))
        .with_children(|w| {
            if level == MenuLevel::Root && !is_psyker {
                // Cross: Skill on top; Item / Attack / Defend across the middle.
                w.spawn(Node {
                    flex_direction: FlexDirection::Column,
                    align_items: AlignItems::Center,
                    row_gap: Val::Px(6.0),
                    ..default()
                })
                .with_children(|cross| {
                    cross
                        .spawn(Node {
                            flex_direction: FlexDirection::Row,
                            ..default()
                        })
                        .with_children(|r| cmd_tile(r, 3, "SKL"));
                    cross
                        .spawn(Node {
                            flex_direction: FlexDirection::Row,
                            column_gap: Val::Px(6.0),
                            ..default()
                        })
                        .with_children(|r| {
                            cmd_tile(r, 2, "ITM");
                            cmd_tile(r, 0, "ATK");
                            cmd_tile(r, 1, "DEF");
                        });
                });
            } else {
                let header: &str = match level {
                    MenuLevel::Root => "FOCUS", // Psyker root list
                    MenuLevel::Skills => "SKILL",
                    MenuLevel::Items => "ITEM",
                    MenuLevel::Manifest => "FOCUS",
                    MenuLevel::Revoke => "REVOKE",
                    MenuLevel::Target => "TARGET",
                };
                w.spawn((
                    Node {
                        width: Val::Px(230.0),
                        flex_direction: FlexDirection::Column,
                        row_gap: Val::Px(2.0),
                        padding: UiRect::all(Val::Px(10.0)),
                        border: UiRect::all(Val::Px(2.0)),
                        ..default()
                    },
                    BorderColor(Color::srgb(0.45, 0.55, 0.85)),
                    BackgroundColor(Color::srgb(0.055, 0.075, 0.17)),
                    BorderRadius::all(Val::Px(6.0)),
                ))
                .with_children(|list| {
                    list.spawn((
                        Text::new(header),
                        TextFont {
                            font_size: 15.0,
                            ..default()
                        },
                        TextColor(Color::srgb(0.95, 0.85, 0.5)),
                        Node {
                            margin: UiRect::bottom(Val::Px(4.0)),
                            ..default()
                        },
                    ));
                    for (i, label) in labels.iter().enumerate() {
                        list.spawn((
                            Button,
                            MenuRow { index: i },
                            Node {
                                width: Val::Percent(100.0),
                                padding: UiRect::axes(Val::Px(8.0), Val::Px(4.0)),
                                ..default()
                            },
                            BackgroundColor(Color::NONE),
                            BorderRadius::all(Val::Px(3.0)),
                        ))
                        .with_children(|r| {
                            r.spawn((
                                Text::new(label.clone()),
                                TextFont {
                                    font_size: 19.0,
                                    ..default()
                                },
                                TextColor(Color::srgb(0.9, 0.93, 1.0)),
                            ));
                        });
                    }
                });
            }
        });
}

/// Highlight the cursor tile/row (and hover). Cross tiles keep a dark base so
/// they read as buttons; list rows fall back to transparent.
fn style_command_menu(
    menu: Res<BattleMenu>,
    mut rows: Query<(&MenuRow, &Interaction, &mut BackgroundColor)>,
) {
    let base = if menu.level == MenuLevel::Root {
        Color::srgb(0.1, 0.12, 0.22)
    } else {
        Color::NONE
    };
    for (row, interaction, mut bg) in &mut rows {
        let selected = row.index == menu.cursor;
        *bg = BackgroundColor(if *interaction == Interaction::Pressed || selected {
            Color::srgb(0.4, 0.34, 0.12) // Lufia-gold selection
        } else if *interaction == Interaction::Hovered {
            Color::srgb(0.18, 0.2, 0.32)
        } else {
            base
        });
    }
}

/// A labelled meter (HP or gauge): a bordered track with a proportional fill.
fn meter(parent: &mut ChildSpawnerCommands, frac: f32, height: f32, fill: Color) {
    parent
        .spawn((
            Node {
                width: Val::Percent(100.0),
                height: Val::Px(height),
                border: UiRect::all(Val::Px(1.0)),
                ..default()
            },
            BorderColor(Color::srgb(0.4, 0.45, 0.6)),
            BackgroundColor(Color::srgb(0.1, 0.11, 0.16)),
        ))
        .with_children(|t| {
            t.spawn((
                Node {
                    width: Val::Percent((frac * 100.0).clamp(0.0, 100.0)),
                    height: Val::Percent(100.0),
                    ..default()
                },
                BackgroundColor(fill),
            ));
        });
}

/// True if a combatant was hit within the last [`FLASH_TTL`] seconds.
fn flashing(hitfx: &HitFx, id: &str) -> bool {
    hitfx.items.iter().any(|h| h.target == id && h.age < FLASH_TTL)
}

/// During the Target picker, classify a combatant: `(is a candidate, is the
/// highlighted pick)`. Off the Target page both are false, so panels render normally.
fn target_state(menu: &BattleMenu, id: &str) -> (bool, bool) {
    if menu.level != MenuLevel::Target {
        return (false, false);
    }
    let candidate = menu.rows.iter().any(|(_, v)| v == id);
    let cursor = menu.rows.get(menu.cursor).map(|(_, v)| v.as_str()) == Some(id);
    (candidate, cursor)
}

/// Immediate-mode enemy panel (top): each enemy as a block + name + HP bar,
/// flashing white when struck.
fn render_enemy_panel(
    mut commands: Commands,
    battle: Res<BattleData>,
    hitfx: Res<HitFx>,
    menu: Res<BattleMenu>,
    existing: Query<Entity, With<BattleScene>>,
) {
    for e in &existing {
        commands.entity(e).despawn();
    }
    commands
        .spawn((
            BattleScene,
            Node {
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                flex_direction: FlexDirection::Column,
                align_items: AlignItems::Center,
                justify_content: JustifyContent::FlexStart,
                padding: UiRect::top(Val::Px(56.0)),
                row_gap: Val::Px(22.0),
                ..default()
            },
        ))
        .with_children(|p| {
            p.spawn((
                Text::new("== BATTLE =="),
                TextFont {
                    font_size: 22.0,
                    ..default()
                },
                TextColor(Color::srgb(0.9, 0.85, 0.6)),
            ));
            p.spawn(Node {
                flex_direction: FlexDirection::Row,
                column_gap: Val::Px(48.0),
                align_items: AlignItems::FlexEnd,
                ..default()
            })
            .with_children(|row| {
                for c in battle.combatants.iter().filter(|c| !c.is_player) {
                    let frac = c.hp as f32 / c.max_hp.max(1) as f32;
                    // Colour the enemy by faction so a mixed brawl is readable.
                    let faction = c
                        .statuses
                        .iter()
                        .find_map(|s| s.strip_prefix("faction:"));
                    let block = if flashing(&hitfx, &c.id) {
                        Color::srgb(1.0, 0.95, 0.95)
                    } else {
                        faction.map(faction_color).unwrap_or(Color::srgb(0.85, 0.28, 0.28))
                    };
                    // While aiming an enemy-targeted action, ring the candidates and
                    // brighten the currently-highlighted one.
                    let (is_cand, is_cursor) = target_state(&menu, &c.id);
                    let ring = if is_cursor {
                        Color::srgb(1.0, 0.95, 0.4)
                    } else if is_cand {
                        Color::srgba(1.0, 0.9, 0.4, 0.5)
                    } else {
                        Color::NONE
                    };
                    row.spawn(Node {
                        width: Val::Px(190.0),
                        flex_direction: FlexDirection::Column,
                        align_items: AlignItems::Center,
                        row_gap: Val::Px(6.0),
                        ..default()
                    })
                    .with_children(|e| {
                        // A small status chip (targeting ring + hit flash); the enemy
                        // itself is the 3D sprite in the arena.
                        e.spawn((
                            Node {
                                width: Val::Px(40.0),
                                height: Val::Px(40.0),
                                border: UiRect::all(Val::Px(if is_cand { 3.0 } else { 0.0 })),
                                ..default()
                            },
                            BackgroundColor(block),
                            BorderColor(ring),
                            BorderRadius::all(Val::Px(6.0)),
                        ));
                        e.spawn((
                            Text::new(format!("{}   {}/{}", c.name, c.hp, c.max_hp)),
                            TextFont {
                                font_size: 16.0,
                                ..default()
                            },
                            TextColor(Color::srgb(0.95, 0.65, 0.65)),
                        ));
                        meter(e, frac, 12.0, Color::srgb(0.85, 0.3, 0.3));
                    });
                }
            });
            // Joined allies are drawn as their own per-party lineups on the screen
            // edges (`render_ally_parties`), not as a flat strip here.
        });
}

/// One joined ally party, pinned to a screen edge (north/west/east). Your own
/// party is the bottom grid; up to three other players' full lineups line the
/// other three edges so a co-op fight reads as several parties fighting together.
#[derive(Clone, Copy)]
enum AllyEdge {
    North,
    West,
    East,
}

/// Immediate-mode ally-party strips: group every joined hero (a player-combatant
/// that isn't one of yours) by its owning `player_id`, then lay each party out as
/// a compact lineup on the north, west, or east edge. Rebuilt each frame from
/// [`BattleData`] like the other battle HUD panels.
fn render_ally_parties(
    mut commands: Commands,
    battle: Res<BattleData>,
    hitfx: Res<HitFx>,
    existing: Query<Entity, With<AllyPartyStrips>>,
) {
    for e in &existing {
        commands.entity(e).despawn();
    }
    // Group joined heroes by owner, preserving first-seen order for a stable layout.
    let mut order: Vec<String> = Vec::new();
    let mut parties: HashMap<String, Vec<&CombatantView>> = HashMap::new();
    for c in battle.combatants.iter() {
        if !c.is_player || battle.your_ids.contains(&c.id) {
            continue;
        }
        let owner = c.player_id.clone().unwrap_or_else(|| c.id.clone());
        if !parties.contains_key(&owner) {
            order.push(owner.clone());
        }
        parties.entry(owner).or_default().push(c);
    }
    if order.is_empty() {
        return;
    }
    let edges = [AllyEdge::North, AllyEdge::West, AllyEdge::East];
    for (gi, owner) in order.iter().enumerate() {
        // Only three edges are free (your party owns the bottom); extra parties
        // stack onto the north edge rather than vanish.
        let edge = edges[gi.min(edges.len() - 1)];
        let heroes = &parties[owner];
        spawn_ally_party(&mut commands, &hitfx, edge, gi, heroes);
    }
}

/// Spawn one edge-pinned ally party: a labelled container holding a slim cell per
/// hero (name, Lv, HP + gauge bars), flashing when struck. North lays the heroes
/// in a row; west/east stack them in a column.
fn spawn_ally_party(
    commands: &mut Commands,
    hitfx: &HitFx,
    edge: AllyEdge,
    group_index: usize,
    heroes: &[&CombatantView],
) {
    // Edge anchoring. North parties beyond the first are nudged down so they stack.
    let mut node = Node {
        position_type: PositionType::Absolute,
        flex_direction: FlexDirection::Column,
        row_gap: Val::Px(5.0),
        padding: UiRect::all(Val::Px(8.0)),
        border: UiRect::all(Val::Px(2.0)),
        ..default()
    };
    match edge {
        AllyEdge::North => {
            node.top = Val::Px(92.0 + group_index.saturating_sub(1) as f32 * 116.0);
            node.left = Val::Px(0.0);
            node.right = Val::Px(0.0);
            node.align_items = AlignItems::Center;
        }
        AllyEdge::West => {
            node.left = Val::Px(10.0);
            node.top = Val::Percent(30.0);
            node.width = Val::Px(180.0);
        }
        AllyEdge::East => {
            node.right = Val::Px(10.0);
            node.top = Val::Percent(30.0);
            node.width = Val::Px(180.0);
        }
    }
    let label = heroes
        .iter()
        .find_map(|c| (!c.name.is_empty() && c.name != "Hero").then(|| c.name.clone()))
        .map(|n| format!("{n}'s party"))
        .unwrap_or_else(|| "Allied party".to_string());
    commands
        .spawn((
            AllyPartyStrips,
            node,
            BorderColor(Color::srgba(0.4, 0.6, 0.95, 0.7)),
            BackgroundColor(Color::srgba(0.05, 0.08, 0.16, 0.82)),
            BorderRadius::all(Val::Px(8.0)),
        ))
        .with_children(|panel| {
            panel.spawn((
                Text::new(label),
                TextFont { font_size: 13.0, ..default() },
                TextColor(Color::srgb(0.7, 0.85, 1.0)),
            ));
            // The heroes: a row on the north edge, a column on the sides.
            let inner_dir = match edge {
                AllyEdge::North => FlexDirection::Row,
                _ => FlexDirection::Column,
            };
            panel
                .spawn(Node {
                    flex_direction: inner_dir,
                    column_gap: Val::Px(8.0),
                    row_gap: Val::Px(5.0),
                    ..default()
                })
                .with_children(|row| {
                    for c in heroes {
                        ally_cell(row, hitfx, c);
                    }
                });
        });
}

/// A slim status cell for one joined ally hero: name + Lv, HP text (with Barrier/
/// Regen suffixes), and HP + ATB meters. No command affordances — it's read-only.
fn ally_cell(parent: &mut ChildSpawnerCommands, hitfx: &HitFx, c: &CombatantView) {
    let hp_frac = c.hp as f32 / c.max_hp.max(1) as f32;
    let gauge = c.gauge.clamp(0.0, 1.0) as f32;
    let hurt = flashing(hitfx, &c.id);
    let name = if !c.name.is_empty() && c.name != "Hero" {
        c.name.clone()
    } else {
        "Hero".to_string()
    };
    parent
        .spawn((
            Node {
                width: Val::Px(158.0),
                flex_direction: FlexDirection::Column,
                row_gap: Val::Px(2.0),
                padding: UiRect::all(Val::Px(5.0)),
                border: UiRect::all(Val::Px(1.0)),
                ..default()
            },
            BorderColor(Color::srgba(0.4, 0.5, 0.8, 0.8)),
            BackgroundColor(if hurt {
                Color::srgb(0.28, 0.1, 0.12)
            } else {
                Color::srgba(0.08, 0.11, 0.22, 0.9)
            }),
            BorderRadius::all(Val::Px(5.0)),
        ))
        .with_children(|cell| {
            cell.spawn(Node {
                flex_direction: FlexDirection::Row,
                justify_content: JustifyContent::SpaceBetween,
                align_items: AlignItems::Center,
                ..default()
            })
            .with_children(|line| {
                line.spawn((
                    Text::new(name),
                    TextFont { font_size: 14.0, ..default() },
                    TextColor(if c.hp == 0 {
                        Color::srgb(0.55, 0.55, 0.6)
                    } else {
                        Color::srgb(0.8, 0.88, 1.0)
                    }),
                ));
                line.spawn((
                    Text::new(format!("Lv {}", c.level)),
                    TextFont { font_size: 11.0, ..default() },
                    TextColor(Color::srgb(0.85, 0.8, 0.45)),
                ));
            });
            let barrier = status_num(&c.statuses, "barrier:");
            let regen = status_num(&c.statuses, "regen:");
            let mut hp_line = format!("Hp {}/{}", c.hp, c.max_hp);
            if barrier > 0 {
                hp_line.push_str(&format!("  +{barrier}\u{25c6}"));
            }
            if regen > 0 {
                hp_line.push_str(&format!("  +{regen}/t"));
            }
            cell.spawn((
                Text::new(hp_line),
                TextFont { font_size: 11.0, ..default() },
                TextColor(Color::srgb(0.6, 0.75, 0.95)),
            ));
            meter(cell, hp_frac, 7.0, Color::srgb(0.35, 0.6, 0.95));
            meter(cell, gauge, 5.0, Color::srgb(0.4, 0.85, 0.5));
        });
}

/// Immediate-mode party window (bottom-left): one row per hero with HP bar, ATB
/// gauge, the active-hero highlight, a ready flag, and the queued-order icon.
/// One Lufia-style party window (name + Lv, HP + ATB bars, portrait, order icon).
fn party_cell(
    parent: &mut ChildSpawnerCommands,
    battle: &BattleData,
    hitfx: &HitFx,
    menu: &BattleMenu,
    id: &str,
    _idx: usize,
) {
    let Some(c) = battle.view(id) else { return };
    let active = battle.active.as_deref() == Some(id);
    let ready = battle.ready.contains(id);
    let queued = battle.queued.get(id).map(|o| o.kind);
    // While aiming an ally-targeted action, this cell is a candidate; the cursor one
    // gets the bright ring (reusing the active-hero highlight colour).
    let (_is_cand, is_target_cursor) = target_state(menu, id);
    let hp_frac = c.hp as f32 / c.max_hp.max(1) as f32;
    let gauge = c.gauge.clamp(0.0, 1.0) as f32;
    let name = battle.hero_label(id);
    let hurt = flashing(hitfx, id);
    parent
        .spawn((
            Node {
                flex_grow: 1.0,
                flex_basis: Val::Px(0.0),
                flex_direction: FlexDirection::Row,
                column_gap: Val::Px(8.0),
                padding: UiRect::all(Val::Px(7.0)),
                border: UiRect::all(Val::Px(2.0)),
                ..default()
            },
            BorderColor(if is_target_cursor {
                Color::srgb(1.0, 0.95, 0.4)
            } else if active {
                Color::srgb(0.95, 0.85, 0.4)
            } else {
                Color::srgb(0.4, 0.5, 0.8)
            }),
            BackgroundColor(if hurt {
                Color::srgb(0.28, 0.1, 0.12)
            } else if is_target_cursor {
                Color::srgb(0.16, 0.2, 0.1)
            } else if active {
                Color::srgb(0.1, 0.14, 0.3)
            } else {
                Color::srgb(0.05, 0.07, 0.16)
            }),
            BorderRadius::all(Val::Px(6.0)),
        ))
        .with_children(|cell| {
            // Left: name + Lv, HP, bars.
            cell.spawn(Node {
                flex_grow: 1.0,
                flex_direction: FlexDirection::Column,
                row_gap: Val::Px(3.0),
                ..default()
            })
            .with_children(|col| {
                col.spawn(Node {
                    flex_direction: FlexDirection::Row,
                    justify_content: JustifyContent::SpaceBetween,
                    align_items: AlignItems::Center,
                    ..default()
                })
                .with_children(|line| {
                    line.spawn((
                        Text::new(name),
                        TextFont { font_size: 18.0, ..default() },
                        TextColor(if c.hp == 0 {
                            Color::srgb(0.55, 0.55, 0.6)
                        } else {
                            Color::srgb(0.85, 0.92, 1.0)
                        }),
                    ));
                    line.spawn((
                        Text::new(format!("Lv {}", c.level)),
                        TextFont { font_size: 14.0, ..default() },
                        TextColor(Color::srgb(0.95, 0.85, 0.4)),
                    ));
                });
                col.spawn(Node {
                    flex_direction: FlexDirection::Row,
                    justify_content: JustifyContent::SpaceBetween,
                    ..default()
                })
                .with_children(|line| {
                    // HP, with a Barrier (temp HP) suffix and a Regen tick when present.
                    let barrier = status_num(&c.statuses, "barrier:");
                    let regen = status_num(&c.statuses, "regen:");
                    let mut hp_line = format!("Hp {}/{}", c.hp, c.max_hp);
                    if barrier > 0 {
                        hp_line.push_str(&format!("  +{barrier}\u{25c6}")); // ◆ = Barrier
                    }
                    if regen > 0 {
                        hp_line.push_str(&format!("  +{regen}/t")); // Regen per turn
                    }
                    line.spawn((
                        Text::new(hp_line),
                        TextFont { font_size: 13.0, ..default() },
                        TextColor(if barrier > 0 {
                            Color::srgb(0.7, 0.8, 1.0)
                        } else {
                            Color::srgb(0.6, 0.75, 0.95)
                        }),
                    ));
                    let (tag, tag_color) = match queued {
                        Some(k) => (k.tag().to_string(), k.color()),
                        None if ready => ("!".to_string(), Color::srgb(0.98, 0.8, 0.3)),
                        None => (String::new(), Color::NONE),
                    };
                    line.spawn((
                        Text::new(tag),
                        TextFont { font_size: 14.0, ..default() },
                        TextColor(tag_color),
                    ));
                });
                // Attributes are intentionally NOT shown in battle — they live on
                // the party screen in the inventory (keeps the combat HUD clean).
                meter(col, hp_frac, 9.0, Color::srgb(0.35, 0.6, 0.95));
                meter(col, gauge, 7.0, Color::srgb(0.4, 0.85, 0.5));
                // Psyker: a row of Focus slots — filled slots show the manifestation
                // abbreviation (+stacks), empty slots a dot.
                let (fmax, foci) = parse_foci(&c.statuses);
                if fmax > 0 {
                    col.spawn(Node {
                        flex_direction: FlexDirection::Row,
                        column_gap: Val::Px(5.0),
                        margin: UiRect::top(Val::Px(3.0)),
                        ..default()
                    })
                    .with_children(|row| {
                        for slot in 0..fmax {
                            let (label, filled) = match foci.get(slot) {
                                Some((k, s)) => {
                                    let tag = if *s > 1 {
                                        format!("{}{}", manifest_abbrev(k), s)
                                    } else {
                                        manifest_abbrev(k)
                                    };
                                    (tag, true)
                                }
                                None => ("·".to_string(), false),
                            };
                            row.spawn((
                                Text::new(label),
                                TextFont { font_size: 13.0, ..default() },
                                TextColor(if filled {
                                    Color::srgb(0.8, 0.6, 1.0)
                                } else {
                                    Color::srgb(0.4, 0.45, 0.6)
                                }),
                            ));
                        }
                    });
                }
            });
            // The hero's avatar is now the 3D battle sprite; the cell is just status.
        });
}

/// Immediate-mode party grid: a 2×2 of Lufia-style windows across the bottom,
/// with the command cross floating in the centre gap.
fn render_party_window(
    mut commands: Commands,
    battle: Res<BattleData>,
    hitfx: Res<HitFx>,
    menu: Res<BattleMenu>,
    existing: Query<Entity, With<PartyWindow>>,
) {
    for e in &existing {
        commands.entity(e).despawn();
    }
    let ids = battle.your_ids.clone();
    // Compact HD-2D HUD: a single row of slim hero status cells across the very
    // bottom, leaving the arena above open for the 3D combatant sprites.
    commands
        .spawn((
            PartyWindow,
            Node {
                position_type: PositionType::Absolute,
                left: Val::Px(10.0),
                right: Val::Px(10.0),
                bottom: Val::Px(10.0),
                height: Val::Px(118.0),
                flex_direction: FlexDirection::Row,
                column_gap: Val::Px(8.0),
                ..default()
            },
        ))
        .with_children(|row| {
            for i in 0..4 {
                match ids.get(i) {
                    Some(id) => party_cell(row, &battle, &hitfx, &menu, id, i),
                    None => {
                        row.spawn(Node {
                            flex_grow: 1.0,
                            flex_basis: Val::Px(0.0),
                            ..default()
                        });
                    }
                }
            }
        });
}

/// Age floating hit numbers; drop the expired. Frozen in the static mockup so
/// the seeded feedback stays on screen.
fn advance_hit_fx(time: Res<Time>, mut hitfx: ResMut<HitFx>) {
    if battle_mockup_flag() {
        return;
    }
    let dt = time.delta_secs();
    for h in &mut hitfx.items {
        h.age += dt;
    }
    hitfx.items.retain(|h| h.age < HIT_TTL);
}

/// Immediate-mode overlay: draw each floating number, rising and fading, anchored
/// over the monster (top-centre) or the striking hero's slot (bottom-left).
fn render_hit_fx(
    mut commands: Commands,
    hitfx: Res<HitFx>,
    battle: Res<BattleData>,
    windows: Query<&Window>,
    existing: Query<Entity, With<HitFxRoot>>,
) {
    for e in &existing {
        commands.entity(e).despawn();
    }
    let Some(win) = windows.iter().next() else {
        return;
    };
    let (w, h) = (win.width(), win.height());
    commands
        .spawn((
            HitFxRoot,
            Node {
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                position_type: PositionType::Absolute,
                ..default()
            },
        ))
        .with_children(|p| {
            for hit in &hitfx.items {
                let (x, y0) = if Some(hit.target.as_str()) == battle.monster_combatant.as_deref() {
                    (w * 0.5 - 16.0, h * 0.22)
                } else {
                    // Heroes sit in a single compact row across the bottom; float the
                    // number over that hero's cell.
                    let idx = battle
                        .your_ids
                        .iter()
                        .position(|id| id == &hit.target)
                        .unwrap_or(0);
                    ((idx as f32 + 0.5) / 4.0 * w, h - 150.0)
                };
                let rise = hit.age * 46.0;
                let alpha = (1.0 - hit.age / HIT_TTL).clamp(0.0, 1.0);
                p.spawn((
                    Node {
                        position_type: PositionType::Absolute,
                        left: Val::Px(x),
                        top: Val::Px(y0 - rise),
                        ..default()
                    },
                    Text::new(hit.text.clone()),
                    TextFont {
                        font_size: 26.0,
                        ..default()
                    },
                    TextColor(hit.color.with_alpha(alpha)),
                ));
            }
        });
}

/// Turn a resolved effect into a floating number (skips zero/no-op effects).
fn push_hit_fx(hitfx: &mut HitFx, e: &HitEffect) {
    let (text, color) = match e.kind.to_lowercase().as_str() {
        "damage" => {
            let n = e.amount.unwrap_or(0);
            if n == 0 {
                return;
            }
            (format!("-{n}"), Color::srgb(1.0, 0.5, 0.4))
        }
        "heal" => {
            let n = e.amount.unwrap_or(0);
            if n == 0 {
                return;
            }
            (format!("+{n}"), Color::srgb(0.5, 1.0, 0.6))
        }
        "ko" => ("KO!".to_string(), Color::srgb(1.0, 0.35, 0.35)),
        _ => return,
    };
    hitfx.items.push(Hit {
        target: e.target.clone(),
        text,
        color,
        age: 0.0,
    });
}

// ----------------------------------------------------------------- ended ---

fn ended_ui(mut commands: Commands, end: Res<EndInfo>) {
    let (title, color): (String, Color) = match end.outcome.as_str() {
        "victory" => ("VICTORY - the creature is slain!".into(), Color::srgb(0.5, 0.95, 0.6)),
        "extracted" => (
            format!("EXTRACTED - banked {} item(s) to your Vault", end.banked),
            Color::srgb(0.4, 0.9, 0.95),
        ),
        "defeat" | "died" => ("DEFEAT - your hero has fallen.".into(), Color::srgb(0.95, 0.4, 0.4)),
        _ => ("The run is over.".into(), Color::srgb(0.8, 0.8, 0.8)),
    };
    commands
        .spawn((
            EndedRoot,
            Node {
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                flex_direction: FlexDirection::Column,
                align_items: AlignItems::Center,
                justify_content: JustifyContent::Center,
                row_gap: Val::Px(14.0),
                ..default()
            },
        ))
        .with_children(|p| {
            p.spawn((
                Text::new(title),
                TextFont {
                    font_size: 40.0,
                    ..default()
                },
                TextColor(color),
            ));
            p.spawn((
                Text::new("Press ESC to exit"),
                TextFont {
                    font_size: 18.0,
                    ..default()
                },
                TextColor(Color::srgb(0.6, 0.65, 0.8)),
            ));
        });
}

fn ended_input(keys: Res<ButtonInput<KeyCode>>, mut exit: EventWriter<AppExit>) {
    if keys.just_pressed(KeyCode::Escape) {
        exit.write(AppExit::Success);
    }
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
                cv("h1", true, 40, 40, &["class:squire"]),
                cv("h2", true, 12, 40, &["class:resonant"]), // most wounded ally
                cv("ally", true, 30, 40, &["class:squire"]), // joined co-op hero
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
