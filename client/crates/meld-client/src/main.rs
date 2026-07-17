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

use meld_client::net;
use net::{ClientCmd, CombatantView, EntityKind, GearLine, HitEffect, Net, ServerMsg, SkillLine};

/// World tiles → screen pixels.
const TILE_PX: f32 = 22.0;

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
        .add_plugins(DefaultPlugins.set(WindowPlugin {
            primary_window: Some(Window {
                title: "MELDWORLD".to_string(),
                resolution: (960.0_f32, 640.0_f32).into(),
                // Browser (wasm): bind to <canvas id="bevy"> and fill its parent.
                canvas: Some("#bevy".to_string()),
                fit_canvas_to_parent: true,
                ..default()
            }),
            ..default()
        }))
        .init_state::<Screen>()
        .insert_resource(ClearColor(Color::srgb(0.05, 0.06, 0.09)))
        .insert_non_send_resource(NetRes(net::start(base)))
        // Demo and autoplay are mutually exclusive; demo skips networking.
        .insert_resource(Autoplay(autoplay_flag() && !demo_flag()))
        .insert_resource(Demo {
            on: demo_flag(),
            t: 0.0,
            started: false,
        })
        .init_resource::<Session>()
        .init_resource::<MoveClock>()
        .init_resource::<BattleMenu>()
        .init_resource::<HitFx>()
        .init_resource::<Overlay>()
        .init_resource::<InventoryData>()
        .init_resource::<ProgressData>()
        .init_resource::<Overworld>()
        .init_resource::<BattleData>()
        .init_resource::<EndInfo>()
        .init_resource::<LobbyData>()
        .add_systems(
            Startup,
            (setup, apply_class_flag, mock_battle_setup, mock_overlay_setup),
        )
        .add_systems(Update, (pump_net, demo_driver)) // run in every state
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
        .add_systems(
            OnEnter(Screen::Overworld),
            (overworld_ui, spawn_overworld_scenery),
        )
        .add_systems(
            OnExit(Screen::Overworld),
            (
                despawn::<OverworldRoot>,
                despawn::<OverlayRoot>,
                despawn::<WorldScenery>,
                despawn::<WorldEntity>,
            ),
        )
        .add_systems(
            Update,
            (
                overlay_input,
                overworld_input,
                sync_overworld_sprites,
                follow_camera,
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
                despawn::<CommandWindow>,
                despawn::<HitFxRoot>,
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
                advance_hit_fx,
                render_hit_fx,
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
    /// Creature content id (monsters only) — drives the label.
    name: Option<String>,
    /// Creature faction (monsters only) — drives the colour.
    faction: Option<String>,
}

impl OwEntity {
    fn player(x: f32, y: f32) -> Self {
        Self { x, y, kind: EntityKind::Player, name: None, faction: None }
    }
    fn monster(x: f32, y: f32, name: &str, faction: &str) -> Self {
        Self {
            x,
            y,
            kind: EntityKind::Monster,
            name: Some(name.to_string()),
            faction: Some(faction.to_string()),
        }
    }
    fn portal(x: f32, y: f32) -> Self {
        Self { x, y, kind: EntityKind::Portal, name: None, faction: None }
    }
}

#[derive(Resource, Default)]
struct Overworld {
    /// entity id -> its render state
    entities: HashMap<String, OwEntity>,
}

#[derive(Resource, Default)]
struct BattleData {
    battle_id: String,
    /// Combatant ids this player controls, in party order (Hero 1..N).
    your_ids: Vec<String>,
    monster_combatant: Option<String>,
    combatants: Vec<CombatantView>,
    /// Heroes whose ATB gauge is full (server said TurnReady).
    ready: HashSet<String>,
    /// Per-hero queued order; auto-fires the instant that hero is ready.
    queued: HashMap<String, QueuedKind>,
    /// The hero the command window is giving orders to.
    active: Option<String>,
}

impl BattleData {
    /// Party-order label for a hero combatant ("Hero 1"), or its raw name.
    fn hero_label(&self, id: &str) -> String {
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
/// commands, a Skill / Item sub-list, or (Psyker) the Manifestation picker.
#[derive(Clone, Copy, PartialEq, Eq, Default)]
enum MenuLevel {
    #[default]
    Root,
    Skills,
    Items,
    /// Psyker: pick which Manifestation the current op (cast/reinforce/revoke)
    /// applies to.
    Manifest,
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
    /// Psyker: open the Manifestation picker for this op verb (cast/reinforce/revoke).
    PsykerOp(&'static str),
    /// Psyker: apply the current op to this manifestation kind.
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

/// The rows shown at a given menu level. For a Psyker the root is the Focus ops
/// (Cast/Reinforce/Revoke/Hold) and the Manifest page lists the manifestations
/// unlocked at `hero_level`.
fn menu_entries(level: MenuLevel, class: &str, hero_level: i32) -> Vec<MenuEntry> {
    let e = |label, action| MenuEntry { label, action };
    match level {
        MenuLevel::Root if class == "psyker" => vec![
            e("Cast", EntryAction::PsykerOp("cast")),
            e("Reinforce", EntryAction::PsykerOp("reinforce")),
            e("Revoke", EntryAction::PsykerOp("revoke")),
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
    /// Psyker: the op verb chosen at Root, applied on the Manifest page.
    psyker_verb: &'static str,
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

/// Static overworld scenery (biome ground bands + reference grid).
#[derive(Component)]
struct WorldScenery;

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

fn setup(mut commands: Commands) {
    commands.spawn(Camera2d);
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
        level: 1,
        statuses: vec![],
    };
    battle.battle_id = "mock".to_string();
    battle.your_ids = vec!["h1".into(), "h2".into(), "h3".into(), "h4".into()];
    battle.monster_combatant = Some("grendel".to_string());
    battle.active = Some("h1".to_string());
    battle.ready.insert("h1".to_string());
    battle.ready.insert("h3".to_string());
    battle.queued.insert("h2".to_string(), QueuedKind::Attack);
    battle.queued.insert("h4".to_string(), QueuedKind::Skill("power_strike"));
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
    state: Res<State<Screen>>,
    mut next: ResMut<NextState<Screen>>,
) {
    net.0.poll();
    while let Some(msg) = net.0.try_recv() {
        match msg {
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
            CombatantView { id: "me".into(), name: "Hero".into(), hp: 40, max_hp: 40, gauge: 0.0, is_player: true, level: 1, statuses: vec![] },
            CombatantView { id: "g".into(), name: "forest bloom stalker".into(), hp: 60, max_hp: 60, gauge: 0.0, is_player: false, level: 1, statuses: vec![] },
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
                    "WASD/arrows: move east through the areas  -  E: extract at a portal  -  I: inventory  -  L: level up",
                ),
                TextFont {
                    font_size: 15.0,
                    ..default()
                },
                TextColor(Color::srgb(0.6, 0.65, 0.8)),
            ));
        });
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

/// Ground tint for a biome band (display-only scenery).
fn biome_ground_color(biome: &str) -> Color {
    match biome {
        "forest" => Color::srgb(0.09, 0.19, 0.12),
        "desert" => Color::srgb(0.26, 0.22, 0.11),
        "ashfall" => Color::srgb(0.20, 0.10, 0.10),
        "tundra" => Color::srgb(0.12, 0.19, 0.25),
        "mire" => Color::srgb(0.11, 0.17, 0.13),
        _ => Color::srgb(0.10, 0.11, 0.16), // hub pad
    }
}

/// Spawn biome ground bands and a faint reference grid the player scrolls across.
/// Bands are laid at the canon distance boundaries (distance ≈ x on the corridor);
/// this is display-only scenery, generously sized to cover any generated run.
fn spawn_overworld_scenery(mut commands: Commands) {
    let bands: [(f32, f32, &str); 6] = [
        (-20.0, 0.0, "hub"),
        (0.0, 100.0, "forest"),
        (100.0, 300.0, "desert"),
        (300.0, 500.0, "ashfall"),
        (500.0, 1000.0, "tundra"),
        (1000.0, 1400.0, "mire"),
    ];
    let band_h = 44.0 * TILE_PX;
    for (start, end, biome) in bands {
        let w = (end - start) * TILE_PX;
        let cx = (start + end) * 0.5 * TILE_PX;
        commands.spawn((
            WorldScenery,
            Sprite::from_color(biome_ground_color(biome), Vec2::new(w, band_h)),
            Transform::from_xyz(cx, 0.0, -3.0),
        ));
    }
    // Faint grid — a motion reference while scrolling through otherwise-flat bands.
    let grid = Color::srgba(1.0, 1.0, 1.0, 0.045);
    let (x0, x1) = (-20.0_f32, 600.0_f32);
    let y_span = band_h;
    let x_span = (x1 - x0) * TILE_PX;
    let mut x = x0;
    while x <= x1 {
        commands.spawn((
            WorldScenery,
            Sprite::from_color(grid, Vec2::new(1.5, y_span)),
            Transform::from_xyz(x * TILE_PX, 0.0, -2.0),
        ));
        x += 6.0;
    }
    let mut y = -18.0_f32;
    while y <= 18.0 {
        commands.spawn((
            WorldScenery,
            Sprite::from_color(grid, Vec2::new(x_span, 1.5)),
            Transform::from_xyz((x0 + x1) * 0.5 * TILE_PX, y * TILE_PX, -2.0),
        ));
        y += 6.0;
    }
}

/// Keep the camera centred on the player so walking east scrolls the world into
/// view (rather than the avatar sliding off into the void).
fn follow_camera(
    world: Res<Overworld>,
    session: Res<Session>,
    mut cam: Query<&mut Transform, With<Camera2d>>,
) {
    let Some(me) = world.entities.get(&session.player_id) else {
        return;
    };
    if let Ok(mut tf) = cam.single_mut() {
        tf.translation.x = me.x * TILE_PX;
        tf.translation.y = -me.y * TILE_PX;
    }
}

/// Update the distance/biome HUD from the player's authoritative position.
fn update_overworld_hud(
    world: Res<Overworld>,
    session: Res<Session>,
    mut q: Query<&mut Text, With<HudText>>,
) {
    let Some(me) = world.entities.get(&session.player_id) else {
        return;
    };
    let d = (me.x * me.x + me.y * me.y).sqrt().floor() as i64;
    if let Ok(mut t) = q.single_mut() {
        **t = format!("distance {d}  -  {}", biome_display(d));
    }
}

#[allow(clippy::too_many_arguments)]
fn overworld_input(
    keys: Res<ButtonInput<KeyCode>>,
    net: NonSend<NetRes>,
    autoplay: Res<Autoplay>,
    world: Res<Overworld>,
    session: Res<Session>,
    overlay: Res<Overlay>,
    time: Res<Time>,
    mut clock: ResMut<MoveClock>,
) {
    // No walking while a screen is open or while channeling an extraction.
    if overlay.kind.is_some() || session.channeling {
        clock.acc = 0.0;
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

    // Extract at the portal (E key, or autopilot once it arrives).
    if keys.just_pressed(KeyCode::KeyE) || (autoplay.0 && near_portal) {
        net.0.send(ClientCmd::Extract);
        return;
    }

    let mut dx = 0.0;
    let mut dy = 0.0;
    if autoplay.0 && !near_portal {
        dx += 1.0; // walk east: into Grendel, then on to the portal
    }
    if keys.pressed(KeyCode::KeyW) || keys.pressed(KeyCode::ArrowUp) {
        dy += 1.0;
    }
    if keys.pressed(KeyCode::KeyS) || keys.pressed(KeyCode::ArrowDown) {
        dy -= 1.0;
    }
    if keys.pressed(KeyCode::KeyA) || keys.pressed(KeyCode::ArrowLeft) {
        dx -= 1.0;
    }
    if keys.pressed(KeyCode::KeyD) || keys.pressed(KeyCode::ArrowRight) {
        dx += 1.0;
    }
    if dx == 0.0 && dy == 0.0 {
        clock.acc = 0.0; // standing still — don't bank up steps to burst later
        return;
    }

    // Emit MoveIntents at a fixed cadence (see MOVE_INTENT_HZ) so walk speed is
    // frame-rate-independent. Bank elapsed time and drain it in fixed steps;
    // cap the backlog so a throttled/backgrounded tab can't accumulate a big
    // teleport when it resumes.
    let step = 1.0 / MOVE_INTENT_HZ;
    clock.acc = (clock.acc + time.delta_secs()).min(0.25);
    while clock.acc >= step {
        clock.acc -= step;
        // Server Y grows south; screen Y grows north - flip for the intent.
        net.0.send(ClientCmd::Move { dx, dy: -dy });
    }
}

// -------------------------------------------------- overworld overlays -----

/// Open/close the inventory (I) and level-up (L) screens; fetch fresh data on
/// open. ESC closes whichever is up.
fn overlay_input(
    keys: Res<ButtonInput<KeyCode>>,
    net: NonSend<NetRes>,
    mut overlay: ResMut<Overlay>,
    mut inv: ResMut<InventoryData>,
    mut prog: ResMut<ProgressData>,
) {
    if keys.just_pressed(KeyCode::Escape) {
        overlay.kind = None;
    }
    if keys.just_pressed(KeyCode::KeyI) {
        if overlay.kind == Some(OverlayKind::Inventory) {
            overlay.kind = None;
        } else {
            overlay.kind = Some(OverlayKind::Inventory);
            inv.loaded = false;
            net.0.fetch_inventory();
        }
    }
    if keys.just_pressed(KeyCode::KeyL) {
        if overlay.kind == Some(OverlayKind::LevelUp) {
            overlay.kind = None;
        } else {
            overlay.kind = Some(OverlayKind::LevelUp);
            prog.loaded = false;
            net.0.fetch_progress();
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
                    label(p, "[ESC] close   [L] level up".into(), 13.0, dim);
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
fn sync_overworld_sprites(
    mut commands: Commands,
    world: Res<Overworld>,
    session: Res<Session>,
    mut q: Query<(Entity, &WorldEntity, &mut Transform, &mut Sprite)>,
) {
    let mut seen = HashMap::new();
    for (entity, we, mut tf, mut sprite) in &mut q {
        if let Some(e) = world.entities.get(&we.0) {
            tf.translation.x = e.x * TILE_PX;
            tf.translation.y = -e.y * TILE_PX; // server Y south → screen Y north
            sprite.color = entity_color(&we.0, e, &session.player_id);
            seen.insert(we.0.clone(), true);
        } else {
            commands.entity(entity).despawn();
        }
    }
    for (id, e) in &world.entities {
        if seen.contains_key(id) {
            continue;
        }
        let size = match e.kind {
            EntityKind::Player => 18.0,
            EntityKind::Monster => 28.0,
            EntityKind::Portal => 26.0,
        };
        let mut ent = commands.spawn((
            WorldEntity(id.clone()),
            Sprite::from_color(entity_color(id, e, &session.player_id), Vec2::splat(size)),
            Transform::from_xyz(e.x * TILE_PX, -e.y * TILE_PX, 1.0),
        ));
        // A small floating label above creatures and portals so a march through
        // the areas reads clearly (which creature, where the exit is).
        // Label creatures with their kind + faction (so you can read who sides
        // with whom); the sprite colour also encodes the faction.
        let label = match e.kind {
            EntityKind::Monster => e.name.as_deref().map(|k| match &e.faction {
                Some(f) => format!("{} · {}", nice_name(k), f),
                None => nice_name(k),
            }),
            EntityKind::Portal => Some("portal".to_string()),
            EntityKind::Player => None,
        };
        if let Some(text) = label {
            ent.with_children(|p| {
                p.spawn((
                    Text2d::new(text),
                    TextFont { font_size: 11.0, ..default() },
                    TextColor(Color::srgb(0.85, 0.9, 1.0)),
                    Transform::from_xyz(0.0, size * 0.5 + 9.0, 2.0),
                ));
            });
        }
    }
}

/// Turn a creature content id into a display name (`dune_wyrm` → `dune wyrm`).
fn nice_name(kind: &str) -> String {
    kind.replace('_', " ")
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

fn entity_color(id: &str, e: &OwEntity, me: &str) -> Color {
    match e.kind {
        EntityKind::Monster => match &e.faction {
            Some(f) => faction_color(f),
            None => Color::srgb(0.9, 0.3, 0.3),
        },
        EntityKind::Portal => Color::srgb(0.35, 0.85, 0.95), // cyan
        EntityKind::Player if id == me => Color::srgb(0.4, 0.9, 0.5), // you
        EntityKind::Player => Color::srgb(0.5, 0.7, 1.0),            // ally
    }
}

fn clear_overworld_sprites(mut commands: Commands, q: Query<Entity, With<WorldEntity>>) {
    for e in &q {
        commands.entity(e).despawn();
    }
}

// ---------------------------------------------------------------- battle ---

/// Reset the command window to its root page.
fn reset_menu(menu: &mut BattleMenu) {
    menu.level = MenuLevel::Root;
    menu.cursor = 0;
    menu.dirty = true;
}

/// On entering a battle, open the command window on the root page.
fn enter_battle(mut menu: ResMut<BattleMenu>) {
    reset_menu(&mut menu);
}

/// Queue an order for the active hero and advance to the next hero that still
/// needs orders. The order fires automatically once that hero's ATB fills
/// ([`auto_fire_queued`]).
fn queue_order(battle: &mut BattleData, hero: &str, kind: QueuedKind, menu: &mut BattleMenu) {
    battle.queued.insert(hero.to_string(), kind);
    battle.active = next_needing_order(battle, hero).or_else(|| Some(hero.to_string()));
    reset_menu(menu);
}

/// First alive hero after `current` (wrapping) that has no queued order yet.
fn next_needing_order(battle: &BattleData, current: &str) -> Option<String> {
    let ids = &battle.your_ids;
    let n = ids.len();
    if n == 0 {
        return None;
    }
    let start = ids.iter().position(|h| h == current).unwrap_or(0);
    (1..=n).find_map(|off| {
        let h = &ids[(start + off) % n];
        (battle.alive(h) && !battle.queued.contains_key(h)).then(|| h.clone())
    })
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

/// Send a hero's order to the server. Attack/Skill need the monster as target.
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
        QueuedKind::Item(it) => Some(ClientCmd::Item {
            battle_id: battle_id.to_string(),
            actor: actor.to_string(),
            item_id: it.to_string(),
        }),
        // Psyker Focus ops ride the Skill action with a `verb:kind` skill_kind
        // (the server ignores the target for these; send the monster if known).
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

/// Keep `active` pointing at a live, controllable hero.
fn validate_active(mut battle: ResMut<BattleData>) {
    let ok = battle
        .active
        .as_ref()
        .map(|a| battle.your_ids.contains(a) && battle.alive(a))
        .unwrap_or(false);
    if !ok {
        battle.active = pick_active(&battle);
    }
}

/// Fire every hero whose gauge is full and who has a queued order.
fn auto_fire_queued(net: NonSend<NetRes>, mut battle: ResMut<BattleData>) {
    let battle_id = battle.battle_id.clone();
    let target = battle.monster_combatant.clone();
    let ready_orders: Vec<(String, QueuedKind)> = battle
        .your_ids
        .iter()
        .filter(|h| battle.ready.contains(*h))
        .filter_map(|h| battle.queued.get(h).map(|k| (h.clone(), *k)))
        .collect();
    for (hero, kind) in ready_orders {
        fire_order(&net.0, &battle_id, &hero, kind, target.as_deref());
        battle.ready.remove(&hero);
        battle.queued.remove(&hero);
    }
}

/// Act on the command row at `index`: root Attack/Defend queue an order for the
/// active hero; Item/Skill open a sub-page; a Skill/Item row queues it; Back
/// returns to root.
fn select_entry(index: usize, menu: &mut BattleMenu, battle: &mut BattleData, class: &str) {
    let active = match battle.active.clone() {
        Some(a) => a,
        None => return,
    };
    let hero_level = battle.view(&active).map(|c| c.level).unwrap_or(1);
    let entries = menu_entries(menu.level, class, hero_level);
    let Some(entry) = entries.get(index) else {
        return;
    };
    match entry.action {
        EntryAction::Attack => queue_order(battle, &active, QueuedKind::Attack, menu),
        EntryAction::Defend => queue_order(battle, &active, QueuedKind::Defend, menu),
        EntryAction::OpenSkills => open_page(menu, MenuLevel::Skills),
        EntryAction::OpenItems => open_page(menu, MenuLevel::Items),
        EntryAction::Skill(kind) => queue_order(battle, &active, QueuedKind::Skill(kind), menu),
        EntryAction::Item(id) => queue_order(battle, &active, QueuedKind::Item(id), menu),
        // Psyker: pick the op verb, then choose a manifestation on the next page.
        EntryAction::PsykerOp(verb) => {
            menu.psyker_verb = verb;
            open_page(menu, MenuLevel::Manifest);
        }
        EntryAction::Manifest(kind) => {
            queue_order(battle, &active, QueuedKind::Focus(menu.psyker_verb, kind), menu)
        }
        EntryAction::Hold => queue_order(battle, &active, QueuedKind::Hold, menu),
        EntryAction::Back => reset_menu(menu),
    }
}

/// Switch the command window to a sub-page.
fn open_page(menu: &mut BattleMenu, level: MenuLevel) {
    menu.level = level;
    menu.cursor = 0;
    menu.dirty = true;
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
            // mends the party, everyone else swings.
            let hc = battle.view(&h).map(hero_class).unwrap_or_else(|| "squire".into());
            let order = match hc.as_str() {
                "psyker" => battle.view(&h).map(psyker_autoplay_op).unwrap_or(QueuedKind::Hold),
                "resonant" => resonant_autoplay_op(&battle),
                _ => QueuedKind::Attack,
            };
            battle.queued.insert(h, order);
        }
        return;
    }

    let hero_level = battle.active_level();
    let n = menu_entries(menu.level, &class, hero_level).len().max(1);
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
    // Psyker uses the list renderer at every level (Cast/Reinforce/Revoke/Hold at
    // root); the martial classes keep the Lufia cross at root.
    let verb_upper = menu.psyker_verb.to_uppercase();
    commands
        .spawn((
            CommandWindow,
            Node {
                position_type: PositionType::Absolute,
                left: Val::Px(12.0),
                right: Val::Px(12.0),
                bottom: Val::Px(96.0),
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
                    MenuLevel::Root => "FOCUS",
                    MenuLevel::Skills => "SKILL",
                    MenuLevel::Items => "ITEM",
                    MenuLevel::Manifest => &verb_upper,
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
                    for (i, entry) in menu_entries(level, &class, hero_level)
                        .into_iter()
                        .enumerate()
                    {
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
                                Text::new(entry.label),
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

/// Immediate-mode enemy panel (top): each enemy as a block + name + HP bar,
/// flashing white when struck.
fn render_enemy_panel(
    mut commands: Commands,
    battle: Res<BattleData>,
    hitfx: Res<HitFx>,
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
                    row.spawn(Node {
                        width: Val::Px(190.0),
                        flex_direction: FlexDirection::Column,
                        align_items: AlignItems::Center,
                        row_gap: Val::Px(6.0),
                        ..default()
                    })
                    .with_children(|e| {
                        e.spawn((
                            Node {
                                width: Val::Px(76.0),
                                height: Val::Px(76.0),
                                ..default()
                            },
                            BackgroundColor(block),
                            BorderRadius::all(Val::Px(8.0)),
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
        });
}

/// Immediate-mode party window (bottom-left): one row per hero with HP bar, ATB
/// gauge, the active-hero highlight, a ready flag, and the queued-order icon.
/// Distinct portrait colours per party slot.
const HERO_COLORS: [Color; 4] = [
    Color::srgb(0.45, 0.9, 0.55),
    Color::srgb(0.5, 0.7, 1.0),
    Color::srgb(0.9, 0.6, 0.95),
    Color::srgb(0.95, 0.8, 0.45),
];

/// One Lufia-style party window (name + Lv, HP + ATB bars, portrait, order icon).
fn party_cell(parent: &mut ChildSpawnerCommands, battle: &BattleData, hitfx: &HitFx, id: &str, idx: usize) {
    let Some(c) = battle.view(id) else { return };
    let active = battle.active.as_deref() == Some(id);
    let ready = battle.ready.contains(id);
    let queued = battle.queued.get(id).copied();
    let hp_frac = c.hp as f32 / c.max_hp.max(1) as f32;
    let gauge = c.gauge.clamp(0.0, 1.0) as f32;
    let name = battle.hero_label(id);
    let hurt = flashing(hitfx, id);
    parent
        .spawn((
            Node {
                width: Val::Percent(46.0),
                flex_direction: FlexDirection::Row,
                column_gap: Val::Px(8.0),
                padding: UiRect::all(Val::Px(8.0)),
                border: UiRect::all(Val::Px(2.0)),
                ..default()
            },
            BorderColor(if active {
                Color::srgb(0.95, 0.85, 0.4)
            } else {
                Color::srgb(0.4, 0.5, 0.8)
            }),
            BackgroundColor(if hurt {
                Color::srgb(0.28, 0.1, 0.12)
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
                // Attribute line (Str/Mnd/Dex/Wll) — heroes carry these, monsters
                // don't, so it only shows for the party.
                let a_str = status_num(&c.statuses, "str:");
                let a_mnd = status_num(&c.statuses, "mnd:");
                let a_dex = status_num(&c.statuses, "dex:");
                let a_wll = status_num(&c.statuses, "wll:");
                if a_str != 0 || a_mnd != 0 || a_dex != 0 || a_wll != 0 {
                    col.spawn((
                        Text::new(format!("STR {a_str}  MND {a_mnd}  DEX {a_dex}  WLL {a_wll}")),
                        TextFont { font_size: 11.0, ..default() },
                        TextColor(Color::srgb(0.62, 0.64, 0.74)),
                    ));
                }
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
            // Right: portrait tile.
            cell.spawn((
                Node {
                    width: Val::Px(46.0),
                    height: Val::Px(46.0),
                    align_self: AlignSelf::Center,
                    ..default()
                },
                BackgroundColor(if c.hp == 0 {
                    Color::srgb(0.25, 0.25, 0.28)
                } else {
                    HERO_COLORS[idx % 4]
                }),
                BorderRadius::all(Val::Px(4.0)),
            ));
        });
}

/// Immediate-mode party grid: a 2×2 of Lufia-style windows across the bottom,
/// with the command cross floating in the centre gap.
fn render_party_window(
    mut commands: Commands,
    battle: Res<BattleData>,
    hitfx: Res<HitFx>,
    existing: Query<Entity, With<PartyWindow>>,
) {
    for e in &existing {
        commands.entity(e).despawn();
    }
    let ids = battle.your_ids.clone();
    commands
        .spawn((
            PartyWindow,
            Node {
                position_type: PositionType::Absolute,
                left: Val::Px(12.0),
                right: Val::Px(12.0),
                bottom: Val::Px(12.0),
                height: Val::Px(288.0),
                flex_direction: FlexDirection::Column,
                justify_content: JustifyContent::SpaceBetween,
                row_gap: Val::Px(10.0),
                ..default()
            },
        ))
        .with_children(|grid| {
            for row_start in [0usize, 2] {
                grid.spawn(Node {
                    flex_direction: FlexDirection::Row,
                    justify_content: JustifyContent::SpaceBetween,
                    flex_grow: 1.0,
                    ..default()
                })
                .with_children(|row| {
                    for i in row_start..row_start + 2 {
                        match ids.get(i) {
                            Some(id) => party_cell(row, &battle, &hitfx, id, i),
                            None => {
                                row.spawn(Node { width: Val::Percent(46.0), ..default() });
                            }
                        }
                    }
                });
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
                    // Heroes sit in a 2×2 grid across the bottom; float the number
                    // over that hero's quadrant.
                    let idx = battle
                        .your_ids
                        .iter()
                        .position(|id| id == &hit.target)
                        .unwrap_or(0);
                    let x = if idx % 2 == 0 { w * 0.27 } else { w * 0.73 };
                    let y = if idx / 2 == 0 { h - 250.0 } else { h - 110.0 };
                    (x, y)
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
