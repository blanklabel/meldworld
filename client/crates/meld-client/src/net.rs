//! Network layer: a background tokio thread that speaks the real MELDWORLD wire
//! protocol (HTTP auth + realtime WebSocket), bridged to Bevy's synchronous ECS
//! via channels. Bevy sends [`ClientCmd`]s; the thread emits [`ServerMsg`]s that
//! a Bevy system drains each frame. Message sequence mirrors the proven bot
//! harness (`qa/tests/four_players_kill_monster.rs`).

use crossbeam_channel::{Receiver, Sender};
use futures_util::{SinkExt, StreamExt};
use meld_proto::common::Combatant;
use meld_proto::realtime::{battle as wb, movement as wm, run as wr, session as ws, Message as _};
use meld_proto::RawEnvelope;
use serde_json::json;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;

/// Commands sent from Bevy into the network thread.
pub enum ClientCmd {
    /// Register (idempotent) + login + connect + authenticate as `username`.
    Connect { username: String },
    /// Ask the server to start the run (forms the party, spawns the arena).
    EnterMaze,
    /// A movement input sample (already-normalized direction).
    Move { dx: f64, dy: f64 },
    /// Attack a battle target.
    Attack { battle_id: String, target: String },
}

/// A render-ready combatant view for the battle screen.
#[derive(Clone)]
pub struct CombatantView {
    pub id: String,
    pub name: String,
    pub hp: i32,
    pub max_hp: i32,
    pub gauge: f64,
    pub is_player: bool,
}

impl CombatantView {
    fn from_wire(c: &Combatant) -> Self {
        let name = match (&c.player_id, &c.monster_kind) {
            (Some(_), _) => "Hero".to_string(),
            (_, Some(k)) => k.replace('_', " "),
            _ => "?".to_string(),
        };
        CombatantView {
            id: c.combatant_id.clone(),
            name,
            hp: c.hp,
            max_hp: c.max_hp,
            gauge: c.gauge,
            is_player: c.player_id.is_some(),
        }
    }
}

/// A dynamic overworld entity (player avatar or monster).
#[derive(Clone)]
pub struct EntityView {
    pub id: String,
    pub x: f64,
    pub y: f64,
    /// `true` for player avatars (snapshot `avatar_state` present), else monster.
    pub is_player: bool,
}

/// Events emitted from the network thread up to Bevy.
pub enum ServerMsg {
    Connected { player_id: String },
    Error { message: String },
    RunStarted,
    Snapshot { entities: Vec<EntityView> },
    BattleStarted {
        battle_id: String,
        your_combatant_id: String,
        combatants: Vec<CombatantView>,
        monster_combatant: Option<String>,
    },
    TurnReady { combatant_id: String },
    Gauge { updates: Vec<(String, f64, i32)> },
    BattleEnded { outcome: String },
    Disconnected,
}

/// Bevy-side handle to the network thread.
pub struct Net {
    pub cmd: UnboundedSender<ClientCmd>,
    pub evt: Receiver<ServerMsg>,
}

impl Net {
    pub fn send(&self, cmd: ClientCmd) {
        let _ = self.cmd.send(cmd);
    }
}

/// Spawn the network thread. `base` is the HTTP base, e.g. `http://127.0.0.1:8080`.
pub fn start(base: String) -> Net {
    let (cmd_tx, cmd_rx) = tokio::sync::mpsc::unbounded_channel();
    let (evt_tx, evt_rx) = crossbeam_channel::unbounded();
    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("tokio runtime");
        rt.block_on(net_main(base, cmd_rx, evt_tx));
    });
    Net {
        cmd: cmd_tx,
        evt: evt_rx,
    }
}

async fn send_env<S>(sink: &mut S, ty: &str, seq: u32, payload: serde_json::Value)
where
    S: SinkExt<Message> + Unpin,
{
    let env = json!({ "type": ty, "seq": seq, "ts": 0u64, "payload": payload });
    let _ = sink.send(Message::Text(env.to_string())).await;
}

async fn net_main(
    base: String,
    mut cmd_rx: UnboundedReceiver<ClientCmd>,
    evt_tx: Sender<ServerMsg>,
) {
    // 1. Wait for the Connect command carrying the chosen username.
    let username = loop {
        match cmd_rx.recv().await {
            Some(ClientCmd::Connect { username }) => break username,
            Some(_) => continue, // ignore anything before Connect
            None => return,
        }
    };

    // 2. HTTP: register (idempotent) then login → realtime ticket + player id.
    let password = "meld-guest-password";
    let http = reqwest::Client::new();
    let body = json!({ "username": username, "password": password });
    let _ = http
        .post(format!("{base}/v1/auth/register"))
        .json(&body)
        .send()
        .await; // conflict is fine (returning guest)
    let login = match http
        .post(format!("{base}/v1/auth/login"))
        .json(&body)
        .send()
        .await
    {
        Ok(r) => r,
        Err(e) => return fail(&evt_tx, format!("login request failed: {e}")),
    };
    if !login.status().is_success() {
        return fail(&evt_tx, "login rejected".into());
    }
    let lv: serde_json::Value = match login.json().await {
        Ok(v) => v,
        Err(e) => return fail(&evt_tx, format!("login body: {e}")),
    };
    let ticket = lv["realtime_ticket"].as_str().unwrap_or_default().to_string();
    let player_id = lv["player"]["player_id"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    // 3. WebSocket connect + authenticate handshake.
    let ws_url = format!("{}/v1/realtime", base.replacen("http", "ws", 1));
    let (stream, _) = match connect_async(&ws_url).await {
        Ok(x) => x,
        Err(e) => return fail(&evt_tx, format!("ws connect: {e}")),
    };
    let (mut sink, mut source) = stream.split();
    let mut seq = 1u32;
    send_env(
        &mut sink,
        ws::Authenticate::TYPE,
        seq,
        json!({ "ticket": ticket, "resume": null }),
    )
    .await;
    seq += 1;

    // 4. Steady state: pump commands out, server messages in.
    let mut input_seq = 0u32;
    loop {
        tokio::select! {
            cmd = cmd_rx.recv() => match cmd {
                Some(ClientCmd::EnterMaze) => {
                    send_env(&mut sink, wr::EnterMaze::TYPE, seq, json!({})).await;
                    seq += 1;
                }
                Some(ClientCmd::Move { dx, dy }) => {
                    input_seq += 1;
                    send_env(&mut sink, wm::MoveIntent::TYPE, seq, json!({
                        "input_seq": input_seq,
                        "move_dir": { "x": dx, "y": dy },
                        "client_pos": { "x": 0.0, "y": 0.0 }
                    })).await;
                    seq += 1;
                }
                Some(ClientCmd::Attack { battle_id, target }) => {
                    send_env(&mut sink, wb::SubmitAction::TYPE, seq, json!({
                        "battle_id": battle_id,
                        "action_id": uuid::Uuid::now_v7().to_string(),
                        "action": "attack",
                        "skill_kind": null,
                        "item_id": null,
                        "target_ids": [target]
                    })).await;
                    seq += 1;
                }
                Some(ClientCmd::Connect { .. }) => {} // already connected
                None => return, // Bevy dropped the sender
            },
            frame = source.next() => match frame {
                Some(Ok(Message::Text(t))) => handle_incoming(&t, &evt_tx, &player_id),
                Some(Ok(_)) => {}
                _ => { let _ = evt_tx.send(ServerMsg::Disconnected); return; }
            }
        }
    }
}

fn fail(evt_tx: &Sender<ServerMsg>, message: String) {
    let _ = evt_tx.send(ServerMsg::Error { message });
}

/// Parse one S2C frame and translate it into a [`ServerMsg`] for Bevy.
fn handle_incoming(text: &str, evt_tx: &Sender<ServerMsg>, player_id: &str) {
    let raw: RawEnvelope = match serde_json::from_str(text) {
        Ok(r) => r,
        Err(_) => return,
    };
    let send = |m: ServerMsg| {
        let _ = evt_tx.send(m);
    };
    match raw.msg_type.as_str() {
        "session.authenticated" => send(ServerMsg::Connected {
            player_id: player_id.to_string(),
        }),
        "session.error" => {
            if let Ok(e) = serde_json::from_value::<ws::Error>(raw.payload) {
                send(ServerMsg::Error { message: e.message });
            }
        }
        "run.started" => send(ServerMsg::RunStarted),
        "world.snapshot" => {
            if let Ok(s) = serde_json::from_value::<wm::Snapshot>(raw.payload) {
                let entities = s
                    .entities
                    .into_iter()
                    .map(|e| EntityView {
                        id: e.entity_id,
                        x: e.position.x,
                        y: e.position.y,
                        is_player: e.avatar_state.is_some(),
                    })
                    .collect();
                send(ServerMsg::Snapshot { entities });
            }
        }
        "battle.started" => {
            if let Ok(b) = serde_json::from_value::<wb::Started>(raw.payload) {
                let mut combatants: Vec<CombatantView> =
                    b.allies.iter().map(CombatantView::from_wire).collect();
                combatants.extend(b.enemies.iter().map(CombatantView::from_wire));
                let monster_combatant = b.enemies.first().map(|c| c.combatant_id.clone());
                send(ServerMsg::BattleStarted {
                    battle_id: b.battle_id,
                    your_combatant_id: b.your_combatant_id,
                    combatants,
                    monster_combatant,
                });
            }
        }
        "battle.turn_ready" => {
            if let Ok(t) = serde_json::from_value::<wb::TurnReady>(raw.payload) {
                send(ServerMsg::TurnReady {
                    combatant_id: t.combatant_id,
                });
            }
        }
        "battle.gauge_update" => {
            if let Ok(g) = serde_json::from_value::<wb::GaugeUpdate>(raw.payload) {
                let updates = g
                    .combatants
                    .into_iter()
                    .map(|c| (c.combatant_id, c.gauge, c.hp))
                    .collect();
                send(ServerMsg::Gauge { updates });
            }
        }
        "battle.ended" => {
            if let Ok(e) = serde_json::from_value::<wb::Ended>(raw.payload) {
                let outcome = serde_json::to_value(e.outcome)
                    .ok()
                    .and_then(|v| v.as_str().map(String::from))
                    .unwrap_or_else(|| "over".to_string());
                send(ServerMsg::BattleEnded { outcome });
            }
        }
        _ => {}
    }
}
