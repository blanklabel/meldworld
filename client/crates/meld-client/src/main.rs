//! meld-client - the Bevy client for MELDWORLD's core gameplay loop
//! (BUILD-PLAN T4 overworld + T5 UI; CANON D16 all-Bevy). Server-authoritative:
//! the client sends intents and renders whatever the server reports (CANON §S).
//!
//! Loop: Join → Overworld (walk into the monster) → Battle (ATB) → Ended.
//!
//! Config: `MELD_SERVER` (default `http://127.0.0.1:8080`) and `MELD_NAME`
//! (default a random guest name).

use std::collections::HashMap;

use bevy::prelude::*;

use meld_client::net;
use net::{ClientCmd, CombatantView, EntityKind, Net, ServerMsg};

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
        .init_resource::<Overworld>()
        .init_resource::<BattleData>()
        .init_resource::<EndInfo>()
        .add_systems(Startup, (setup, mock_battle_setup))
        .add_systems(Update, (pump_net, demo_driver)) // run in every state
        // Join
        .add_systems(OnEnter(Screen::Join), join_ui)
        .add_systems(OnExit(Screen::Join), despawn::<JoinRoot>)
        .add_systems(Update, join_input.run_if(in_state(Screen::Join)))
        // Overworld
        .add_systems(OnEnter(Screen::Overworld), overworld_ui)
        .add_systems(OnExit(Screen::Overworld), despawn::<OverworldRoot>)
        .add_systems(
            Update,
            (overworld_input, sync_overworld_sprites).run_if(in_state(Screen::Overworld)),
        )
        // Battle
        .add_systems(OnEnter(Screen::Battle), (clear_overworld_sprites, enter_battle))
        .add_systems(
            OnExit(Screen::Battle),
            (
                despawn::<BattleScene>,
                despawn::<PartyWindow>,
                despawn::<CommandWindow>,
            ),
        )
        .add_systems(
            Update,
            (
                render_enemy_panel,
                render_party_window,
                rebuild_command_menu,
                style_command_menu,
                menu_keyboard,
                menu_click,
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
    Overworld,
    Battle,
    Ended,
}

// ------------------------------------------------------------- resources ---

/// Non-send: the browser socket handle isn't `Send`, so Bevy runs the systems
/// that touch it on the main thread.
struct NetRes(Net);

#[derive(Resource, Default)]
struct Session {
    player_id: String,
    connecting: bool,
    entered: bool,
    channeling: bool,
    status: String,
}

#[derive(Resource, Default)]
struct Overworld {
    /// entity id -> (x, y, kind)
    entities: HashMap<String, (f32, f32, EntityKind)>,
}

#[derive(Resource, Default)]
struct BattleData {
    battle_id: String,
    your_combatant_id: String,
    monster_combatant: Option<String>,
    combatants: Vec<CombatantView>,
    my_turn: bool,
}

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
/// four commands, or a Skill / Item sub-list.
#[derive(Clone, Copy, PartialEq, Eq, Default)]
enum MenuLevel {
    #[default]
    Root,
    Skills,
    Items,
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
    Back,
}

/// One selectable row in the command window.
struct MenuEntry {
    label: &'static str,
    action: EntryAction,
}

/// The rows shown at a given menu level (slice content). Skill/Item pages carry
/// a Back row so keyboard-only players can return to the root commands.
fn menu_entries(level: MenuLevel) -> Vec<MenuEntry> {
    let e = |label, action| MenuEntry { label, action };
    match level {
        MenuLevel::Root => vec![
            e("Attack", EntryAction::Attack),
            e("Defend", EntryAction::Defend),
            e("Item", EntryAction::OpenItems),
            e("Skill", EntryAction::OpenSkills),
        ],
        MenuLevel::Skills => vec![
            e("Power Strike", EntryAction::Skill("power_strike")),
            e("Second Wind", EntryAction::Skill("second_wind")),
            e("Back", EntryAction::Back),
        ],
        MenuLevel::Items => vec![
            e("Salve", EntryAction::Item("salve")),
            e("Elixir", EntryAction::Item("elixir")),
            e("Back", EntryAction::Back),
        ],
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
#[derive(Component)]
struct EndedRoot;
#[derive(Component)]
struct StatusText;

/// A sprite representing an overworld entity, tagged by its server id.
#[derive(Component)]
struct WorldEntity(String);

// ---------------------------------------------------------------- setup ----

fn setup(mut commands: Commands) {
    commands.spawn(Camera2d);
}

/// If the battle-mockup flag is set, seed canned combatants and jump straight to
/// the Battle screen (no networking) so the battle subscreen is viewable on its
/// own. Runs once at startup; a no-op otherwise.
fn mock_battle_setup(mut battle: ResMut<BattleData>, mut next: ResMut<NextState<Screen>>) {
    if !battle_mockup_flag() {
        return;
    }
    battle.battle_id = "mock".to_string();
    battle.your_combatant_id = "hero".to_string();
    battle.monster_combatant = Some("grendel".to_string());
    battle.my_turn = true;
    battle.combatants = vec![
        CombatantView {
            id: "hero".into(),
            name: "Hero".into(),
            hp: 32,
            max_hp: 40,
            gauge: 1.0,
            is_player: true,
        },
        CombatantView {
            id: "ally".into(),
            name: "Ally".into(),
            hp: 27,
            max_hp: 38,
            gauge: 0.5,
            is_player: true,
        },
        CombatantView {
            id: "grendel".into(),
            name: "Grendel".into(),
            hp: 44,
            max_hp: 60,
            gauge: 0.65,
            is_player: false,
        },
    ];
    next.set(Screen::Battle);
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
    state: Res<State<Screen>>,
    mut next: ResMut<NextState<Screen>>,
) {
    net.0.poll();
    while let Some(msg) = net.0.try_recv() {
        match msg {
            ServerMsg::Connected { player_id } => {
                session.player_id = player_id;
                session.status = "connected - entering maze...".to_string();
                if !session.entered {
                    session.entered = true;
                    net.0.send(ClientCmd::EnterMaze);
                }
            }
            ServerMsg::RunStarted => {
                if *state.get() == Screen::Join {
                    next.set(Screen::Overworld);
                }
            }
            ServerMsg::Snapshot { entities } => {
                world.entities.clear();
                for e in entities {
                    world
                        .entities
                        .insert(e.id, (e.x as f32, e.y as f32, e.kind));
                }
            }
            ServerMsg::BattleStarted {
                battle_id,
                your_combatant_id,
                combatants,
                monster_combatant,
            } => {
                battle.battle_id = battle_id;
                battle.your_combatant_id = your_combatant_id;
                battle.monster_combatant = monster_combatant;
                battle.combatants = combatants;
                battle.my_turn = false;
                if *state.get() != Screen::Battle {
                    next.set(Screen::Battle);
                }
            }
            ServerMsg::TurnReady { combatant_id } => {
                if combatant_id == battle.your_combatant_id {
                    battle.my_turn = true;
                    reset_menu(&mut menu); // open on the root commands each turn
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
                for (id, gauge, hp) in updates {
                    if let Some(c) = battle.combatants.iter_mut().find(|c| c.id == id) {
                        c.gauge = gauge;
                        c.hp = hp;
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
        world.entities.insert("me".to_string(), (x, 0.0, EntityKind::Player));
        world.entities.insert("grendel".to_string(), (10.0, 0.0, EntityKind::Monster));
        world.entities.insert("portal".to_string(), (14.0, 0.0, EntityKind::Portal));
        return;
    }

    // 3s+: battle. Grendel's HP falls to 0 over ~5s; gauges animate.
    if *state.get() == Screen::Overworld {
        battle.your_combatant_id = "me".to_string();
        battle.monster_combatant = Some("g".to_string());
        battle.combatants = vec![
            CombatantView { id: "me".into(), name: "Hero".into(), hp: 40, max_hp: 40, gauge: 0.0, is_player: true },
            CombatantView { id: "g".into(), name: "forest bloom stalker".into(), hp: 60, max_hp: 60, gauge: 0.0, is_player: false },
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
    battle.my_turn = (phase * 1.5) as i32 % 2 == 0;
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
                Text::new("Press ENTER to enter the maze and fight Grendel"),
                TextFont {
                    font_size: 20.0,
                    ..default()
                },
                TextColor(Color::srgb(0.6, 0.65, 0.8)),
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

fn join_input(
    keys: Res<ButtonInput<KeyCode>>,
    net: NonSend<NetRes>,
    autoplay: Res<Autoplay>,
    mut session: ResMut<Session>,
    mut status_q: Query<&mut Text, With<StatusText>>,
) {
    if (keys.just_pressed(KeyCode::Enter) || autoplay.0) && !session.connecting {
        session.connecting = true;
        let name = std::env::var("MELD_NAME").unwrap_or_else(|_| {
            format!("guest{}", &uuid::Uuid::new_v4().simple().to_string()[..8])
        });
        session.status = "connecting...".to_string();
        net.0.send(ClientCmd::Connect { username: name });
    }
    if let Ok(mut t) = status_q.single_mut() {
        **t = session.status.clone();
    }
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
                Text::new(
                    "WASD/arrows: move - walk into Grendel (red) to fight - press E at the cyan portal to extract",
                ),
                TextFont {
                    font_size: 16.0,
                    ..default()
                },
                TextColor(Color::srgb(0.6, 0.65, 0.8)),
            ));
        });
}

fn overworld_input(
    keys: Res<ButtonInput<KeyCode>>,
    net: NonSend<NetRes>,
    autoplay: Res<Autoplay>,
    world: Res<Overworld>,
    session: Res<Session>,
    time: Res<Time>,
    mut clock: ResMut<MoveClock>,
) {
    // Movement is locked while channeling an extraction (and would interrupt it).
    if session.channeling {
        return;
    }

    let me = world.entities.get(&session.player_id).map(|&(x, y, _)| (x, y));
    let portal = world
        .entities
        .values()
        .find(|&&(_, _, k)| k == EntityKind::Portal)
        .map(|&(x, y, _)| (x, y));
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
        if let Some(&(x, y, kind)) = world.entities.get(&we.0) {
            tf.translation.x = x * TILE_PX;
            tf.translation.y = -y * TILE_PX; // server Y south → screen Y north
            sprite.color = entity_color(&we.0, kind, &session.player_id);
            seen.insert(we.0.clone(), true);
        } else {
            commands.entity(entity).despawn();
        }
    }
    for (id, &(x, y, kind)) in &world.entities {
        if seen.contains_key(id) {
            continue;
        }
        let size = match kind {
            EntityKind::Player => 18.0,
            EntityKind::Monster => 30.0,
            EntityKind::Portal => 26.0,
        };
        commands.spawn((
            WorldEntity(id.clone()),
            Sprite::from_color(entity_color(id, kind, &session.player_id), Vec2::splat(size)),
            Transform::from_xyz(x * TILE_PX, -y * TILE_PX, 1.0),
        ));
    }
}

fn entity_color(id: &str, kind: EntityKind, me: &str) -> Color {
    match kind {
        EntityKind::Monster => Color::srgb(0.9, 0.3, 0.3),
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

/// Send a command, end the local turn, and return the menu to root.
fn fire(net: &Net, cmd: ClientCmd, battle: &mut BattleData, menu: &mut BattleMenu) {
    net.send(cmd);
    battle.my_turn = false; // wait for the next turn_ready
    reset_menu(menu);
}

/// Act on the menu row at `index` for the current page: root commands fire or
/// open a sub-page; Skill/Item rows fire that skill/item; Back returns to root.
/// Attack/Skill target Grendel (the sole enemy in the slice).
fn select_entry(index: usize, menu: &mut BattleMenu, battle: &mut BattleData, net: &Net) {
    let entries = menu_entries(menu.level);
    let Some(entry) = entries.get(index) else {
        return;
    };
    let battle_id = battle.battle_id.clone();
    match entry.action {
        EntryAction::Attack => {
            if let Some(target) = battle.monster_combatant.clone() {
                fire(net, ClientCmd::Attack { battle_id, target }, battle, menu);
            }
        }
        EntryAction::Defend => fire(net, ClientCmd::Defend { battle_id }, battle, menu),
        EntryAction::OpenSkills => {
            menu.level = MenuLevel::Skills;
            menu.cursor = 0;
            menu.dirty = true;
        }
        EntryAction::OpenItems => {
            menu.level = MenuLevel::Items;
            menu.cursor = 0;
            menu.dirty = true;
        }
        EntryAction::Skill(kind) => {
            if let Some(target) = battle.monster_combatant.clone() {
                fire(
                    net,
                    ClientCmd::Skill {
                        battle_id,
                        target,
                        skill_kind: kind.to_string(),
                    },
                    battle,
                    menu,
                );
            }
        }
        EntryAction::Item(id) => fire(
            net,
            ClientCmd::Item {
                battle_id,
                item_id: id.to_string(),
            },
            battle,
            menu,
        ),
        EntryAction::Back => reset_menu(menu),
    }
}

/// Keyboard control: ↑/↓ move the cursor, ENTER/SPACE (or a number key) selects,
/// ESC/BACKSPACE backs out of a sub-page. At the root page A/D/I/S jump to the
/// matching command. Autoplay just attacks.
fn menu_keyboard(
    keys: Res<ButtonInput<KeyCode>>,
    net: NonSend<NetRes>,
    autoplay: Res<Autoplay>,
    mut menu: ResMut<BattleMenu>,
    mut battle: ResMut<BattleData>,
) {
    if !battle.my_turn {
        return;
    }
    if autoplay.0 {
        select_entry(0, &mut menu, &mut battle, &net.0); // root row 0 = Attack
        return;
    }

    let n = menu_entries(menu.level).len().max(1);
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

    // Number keys pick the Nth row directly.
    let digits = [
        KeyCode::Digit1,
        KeyCode::Digit2,
        KeyCode::Digit3,
        KeyCode::Digit4,
    ];
    for (i, key) in digits.iter().enumerate() {
        if i < n && keys.just_pressed(*key) {
            menu.cursor = i;
            select_entry(i, &mut menu, &mut battle, &net.0);
            return;
        }
    }

    // Root-page initials (A/D/I/S).
    if menu.level == MenuLevel::Root {
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
            select_entry(i, &mut menu, &mut battle, &net.0);
            return;
        }
    }

    if keys.just_pressed(KeyCode::Enter) || keys.just_pressed(KeyCode::Space) {
        select_entry(menu.cursor, &mut menu, &mut battle, &net.0);
    }
}

/// Mouse/touch control: pressing a row selects it (bevy_ui `Interaction` covers
/// pointer and touch alike).
fn menu_click(
    net: NonSend<NetRes>,
    mut menu: ResMut<BattleMenu>,
    mut battle: ResMut<BattleData>,
    rows: Query<(&Interaction, &MenuRow), Changed<Interaction>>,
) {
    if !battle.my_turn {
        return;
    }
    let mut pressed = None;
    for (interaction, row) in &rows {
        if *interaction == Interaction::Pressed {
            pressed = Some(row.index);
        }
    }
    if let Some(index) = pressed {
        menu.cursor = index;
        select_entry(index, &mut menu, &mut battle, &net.0);
    }
}

/// Respawn the command window's rows when the page changes. Kept stable between
/// changes so `Interaction` on the row buttons works — an immediate-mode rebuild
/// every frame would reset click/hover state.
fn rebuild_command_menu(
    mut commands: Commands,
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
    let header = match menu.level {
        MenuLevel::Root => "COMMAND",
        MenuLevel::Skills => "SKILL",
        MenuLevel::Items => "ITEM",
    };
    commands
        .spawn((
            CommandWindow,
            Node {
                position_type: PositionType::Absolute,
                right: Val::Px(24.0),
                bottom: Val::Px(24.0),
                width: Val::Px(300.0),
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
        .with_children(|w| {
            w.spawn((
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
            for (i, entry) in menu_entries(menu.level).into_iter().enumerate() {
                w.spawn((
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

/// Highlight the cursor row (and hover); blank the highlights when it isn't the
/// player's turn.
fn style_command_menu(
    menu: Res<BattleMenu>,
    battle: Res<BattleData>,
    mut rows: Query<(&MenuRow, &Interaction, &mut BackgroundColor)>,
) {
    for (row, interaction, mut bg) in &mut rows {
        let selected = row.index == menu.cursor;
        *bg = BackgroundColor(if !battle.my_turn {
            Color::NONE
        } else if *interaction == Interaction::Pressed || selected {
            Color::srgb(0.2, 0.28, 0.52)
        } else if *interaction == Interaction::Hovered {
            Color::srgb(0.14, 0.17, 0.3)
        } else {
            Color::NONE
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

/// Immediate-mode enemy panel (top): each enemy as a block + name + HP bar.
fn render_enemy_panel(
    mut commands: Commands,
    battle: Res<BattleData>,
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
                            BackgroundColor(Color::srgb(0.85, 0.28, 0.28)),
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

/// Immediate-mode party status window (bottom-left): name, HP bar, ATB gauge.
fn render_party_window(
    mut commands: Commands,
    battle: Res<BattleData>,
    existing: Query<Entity, With<PartyWindow>>,
) {
    for e in &existing {
        commands.entity(e).despawn();
    }
    commands
        .spawn((
            PartyWindow,
            Node {
                position_type: PositionType::Absolute,
                left: Val::Px(24.0),
                bottom: Val::Px(24.0),
                width: Val::Px(380.0),
                flex_direction: FlexDirection::Column,
                row_gap: Val::Px(8.0),
                padding: UiRect::all(Val::Px(10.0)),
                border: UiRect::all(Val::Px(2.0)),
                ..default()
            },
            BorderColor(Color::srgb(0.45, 0.55, 0.85)),
            BackgroundColor(Color::srgb(0.055, 0.075, 0.17)),
            BorderRadius::all(Val::Px(6.0)),
        ))
        .with_children(|p| {
            p.spawn((
                Text::new("PARTY"),
                TextFont {
                    font_size: 15.0,
                    ..default()
                },
                TextColor(Color::srgb(0.95, 0.85, 0.5)),
            ));
            for c in battle.combatants.iter().filter(|c| c.is_player) {
                let mine = c.id == battle.your_combatant_id;
                let hp_frac = c.hp as f32 / c.max_hp.max(1) as f32;
                let gauge = c.gauge.clamp(0.0, 1.0) as f32;
                p.spawn(Node {
                    flex_direction: FlexDirection::Column,
                    row_gap: Val::Px(3.0),
                    ..default()
                })
                .with_children(|r| {
                    r.spawn((
                        Text::new(format!(
                            "{}{}   HP {}/{}",
                            if mine { "> " } else { "  " },
                            c.name,
                            c.hp,
                            c.max_hp
                        )),
                        TextFont {
                            font_size: 15.0,
                            ..default()
                        },
                        TextColor(if mine {
                            Color::srgb(0.6, 0.95, 0.7)
                        } else {
                            Color::srgb(0.7, 0.8, 1.0)
                        }),
                    ));
                    meter(r, hp_frac, 10.0, Color::srgb(0.4, 0.85, 0.5));
                    meter(r, gauge, 6.0, Color::srgb(0.55, 0.7, 1.0));
                });
            }
        });
}

// ----------------------------------------------------------------- ended ---

fn ended_ui(mut commands: Commands, end: Res<EndInfo>) {
    let (title, color): (String, Color) = match end.outcome.as_str() {
        "victory" => ("VICTORY - Grendel is slain!".into(), Color::srgb(0.5, 0.95, 0.6)),
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
