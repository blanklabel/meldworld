//! meld-client — the Bevy client for MELDWORLD's core gameplay loop
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
use net::{ClientCmd, CombatantView, Net, ServerMsg};

/// World tiles → screen pixels.
const TILE_PX: f32 = 22.0;

fn main() {
    App::new()
        .add_plugins(DefaultPlugins.set(WindowPlugin {
            primary_window: Some(Window {
                title: "MELDWORLD".to_string(),
                resolution: (960.0, 640.0).into(),
                ..default()
            }),
            ..default()
        }))
        .init_state::<Screen>()
        .insert_resource(ClearColor(Color::srgb(0.05, 0.06, 0.09)))
        .init_resource::<Session>()
        .init_resource::<Overworld>()
        .init_resource::<BattleData>()
        .init_resource::<EndInfo>()
        .add_systems(Startup, setup)
        .add_systems(Update, pump_net) // runs in every state
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
        .add_systems(OnEnter(Screen::Battle), clear_overworld_sprites)
        .add_systems(OnExit(Screen::Battle), despawn::<BattleRoot>)
        .add_systems(
            Update,
            (battle_input, render_battle).run_if(in_state(Screen::Battle)),
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

#[derive(Resource)]
struct NetRes(Net);

#[derive(Resource, Default)]
struct Session {
    player_id: String,
    connecting: bool,
    entered: bool,
    status: String,
}

#[derive(Resource, Default)]
struct Overworld {
    /// entity id -> (x, y, is_player)
    entities: HashMap<String, (f32, f32, bool)>,
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
}

// ------------------------------------------------------------- marker(s) ---

#[derive(Component)]
struct JoinRoot;
#[derive(Component)]
struct OverworldRoot;
#[derive(Component)]
struct BattleRoot;
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
    let base = std::env::var("MELD_SERVER").unwrap_or_else(|_| "http://127.0.0.1:8080".to_string());
    commands.insert_resource(NetRes(net::start(base)));
}

fn despawn<T: Component>(mut commands: Commands, q: Query<Entity, With<T>>) {
    for e in &q {
        commands.entity(e).despawn();
    }
}

// --------------------------------------------------------------- net pump --

/// Drain server messages every frame, update resources, drive transitions.
fn pump_net(
    net: Res<NetRes>,
    mut session: ResMut<Session>,
    mut world: ResMut<Overworld>,
    mut battle: ResMut<BattleData>,
    mut end: ResMut<EndInfo>,
    state: Res<State<Screen>>,
    mut next: ResMut<NextState<Screen>>,
) {
    while let Ok(msg) = net.0.evt.try_recv() {
        match msg {
            ServerMsg::Connected { player_id } => {
                session.player_id = player_id;
                session.status = "connected — entering maze…".to_string();
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
                        .insert(e.id, (e.x as f32, e.y as f32, e.is_player));
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
                end.outcome = outcome;
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
                Text::new("⚔  MELDWORLD"),
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
    net: Res<NetRes>,
    mut session: ResMut<Session>,
    mut status_q: Query<&mut Text, With<StatusText>>,
) {
    if keys.just_pressed(KeyCode::Enter) && !session.connecting {
        session.connecting = true;
        let name = std::env::var("MELD_NAME").unwrap_or_else(|_| {
            format!("guest{}", &uuid::Uuid::new_v4().simple().to_string()[..8])
        });
        session.status = "connecting…".to_string();
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
                Text::new("WASD / arrows to move — walk into Grendel (red) to fight"),
                TextFont {
                    font_size: 16.0,
                    ..default()
                },
                TextColor(Color::srgb(0.6, 0.65, 0.8)),
            ));
        });
}

fn overworld_input(keys: Res<ButtonInput<KeyCode>>, net: Res<NetRes>) {
    let mut dx = 0.0;
    let mut dy = 0.0;
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
    if dx != 0.0 || dy != 0.0 {
        // Server Y grows south; screen Y grows north — flip for the intent.
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
        if let Some(&(x, y, is_player)) = world.entities.get(&we.0) {
            tf.translation.x = x * TILE_PX;
            tf.translation.y = -y * TILE_PX; // server Y south → screen Y north
            sprite.color = entity_color(&we.0, is_player, &session.player_id);
            seen.insert(we.0.clone(), true);
        } else {
            commands.entity(entity).despawn();
        }
    }
    for (id, &(x, y, is_player)) in &world.entities {
        if seen.contains_key(id) {
            continue;
        }
        let size = if is_player { 18.0 } else { 30.0 };
        commands.spawn((
            WorldEntity(id.clone()),
            Sprite::from_color(entity_color(id, is_player, &session.player_id), Vec2::splat(size)),
            Transform::from_xyz(x * TILE_PX, -y * TILE_PX, 1.0),
        ));
    }
}

fn entity_color(id: &str, is_player: bool, me: &str) -> Color {
    if !is_player {
        Color::srgb(0.9, 0.3, 0.3) // monster
    } else if id == me {
        Color::srgb(0.4, 0.9, 0.5) // you
    } else {
        Color::srgb(0.5, 0.7, 1.0) // ally
    }
}

fn clear_overworld_sprites(mut commands: Commands, q: Query<Entity, With<WorldEntity>>) {
    for e in &q {
        commands.entity(e).despawn();
    }
}

// ---------------------------------------------------------------- battle ---

fn battle_input(keys: Res<ButtonInput<KeyCode>>, net: Res<NetRes>, mut battle: ResMut<BattleData>) {
    if battle.my_turn && (keys.just_pressed(KeyCode::Space) || keys.just_pressed(KeyCode::Enter)) {
        if let Some(target) = battle.monster_combatant.clone() {
            net.0.send(ClientCmd::Attack {
                battle_id: battle.battle_id.clone(),
                target,
            });
            battle.my_turn = false; // wait for the next turn_ready
        }
    }
}

/// Rebuild the battle HUD each frame from `BattleData` (server-authoritative —
/// the client never computes HP or damage).
fn render_battle(
    mut commands: Commands,
    battle: Res<BattleData>,
    roots: Query<Entity, With<BattleRoot>>,
) {
    // Cheap immediate-mode: clear and rebuild the panel each frame.
    for e in &roots {
        commands.entity(e).despawn();
    }
    commands
        .spawn((
            BattleRoot,
            Node {
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                flex_direction: FlexDirection::Column,
                align_items: AlignItems::Center,
                justify_content: JustifyContent::Center,
                row_gap: Val::Px(10.0),
                ..default()
            },
        ))
        .with_children(|p| {
            p.spawn((
                Text::new("⚔  BATTLE"),
                TextFont {
                    font_size: 34.0,
                    ..default()
                },
                TextColor(Color::srgb(0.9, 0.85, 0.6)),
            ));
            for c in &battle.combatants {
                let filled = (c.gauge.clamp(0.0, 1.0) * 10.0).round() as usize;
                let gauge_bar = format!("{}{}", "█".repeat(filled), "░".repeat(10 - filled));
                let mine = c.id == battle.your_combatant_id;
                let line = format!(
                    "{}{:<16} HP {:>4}/{:<4}  [{}]",
                    if mine { "▶ " } else { "  " },
                    c.name,
                    c.hp,
                    c.max_hp,
                    gauge_bar
                );
                let color = if !c.is_player {
                    Color::srgb(0.95, 0.5, 0.5)
                } else if mine {
                    Color::srgb(0.5, 0.95, 0.6)
                } else {
                    Color::srgb(0.7, 0.8, 1.0)
                };
                p.spawn((
                    Text::new(line),
                    TextFont {
                        font_size: 20.0,
                        ..default()
                    },
                    TextColor(color),
                ));
            }
            let prompt = if battle.my_turn {
                "Your turn — press SPACE to attack Grendel"
            } else {
                "…gauges filling…"
            };
            p.spawn((
                Text::new(prompt),
                TextFont {
                    font_size: 18.0,
                    ..default()
                },
                TextColor(if battle.my_turn {
                    Color::srgb(0.95, 0.95, 0.5)
                } else {
                    Color::srgb(0.5, 0.55, 0.7)
                }),
            ));
        });
}

// ----------------------------------------------------------------- ended ---

fn ended_ui(mut commands: Commands, end: Res<EndInfo>) {
    let (title, color) = match end.outcome.as_str() {
        "victory" => ("VICTORY — Grendel is slain!", Color::srgb(0.5, 0.95, 0.6)),
        "defeat" => ("DEFEAT — your hero has fallen.", Color::srgb(0.95, 0.4, 0.4)),
        _ => ("The battle is over.", Color::srgb(0.8, 0.8, 0.8)),
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
