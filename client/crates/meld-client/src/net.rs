//! Cross-platform network layer (native desktop AND browser/wasm).
//!
//! Poll-based, single-threaded: [`Net`] holds an internal state machine advanced
//! by [`Net::poll`] once per frame. Auth HTTP goes through `ehttp`, the realtime
//! socket through `ewebsock` — neither needs tokio or OS threads, so the exact
//! same code runs on the desktop and compiled to wasm in the browser.
//!
//! Bevy holds `Net` as a NonSend resource; commands go in via [`Net::send`],
//! server events come out via [`Net::poll`] + [`Net::try_recv`]. Message
//! sequence mirrors the proven bot harness.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;
use std::sync::mpsc;

use ewebsock::{WsEvent, WsMessage, WsReceiver, WsSender};
use meld_proto::common::Combatant;
use meld_proto::realtime::{battle as wb, movement as wm, run as wr, session as ws, Message as _};
use meld_proto::RawEnvelope;
use serde_json::json;

const GUEST_PASSWORD: &str = "meld-guest-password";

/// Commands sent from Bevy into the network layer.
pub enum ClientCmd {
    Connect { username: String },
    EnterMaze,
    Move { dx: f64, dy: f64 },
    Attack { battle_id: String, target: String },
    /// Begin a portal extraction channel at the current position.
    Extract,
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

/// What an overworld entity is (decides how the client draws it).
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum EntityKind {
    Player,
    Monster,
    Portal,
}

/// A dynamic overworld entity.
#[derive(Clone)]
pub struct EntityView {
    pub id: String,
    pub x: f64,
    pub y: f64,
    pub kind: EntityKind,
}

/// Events emitted from the network layer up to Bevy.
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
    /// An extraction channel began / broke.
    ChannelStarted { completes_at: u64 },
    ChannelInterrupted,
    /// This player's run ended (extracted / died / abandoned), with the count of
    /// items banked on extraction.
    RunEnded { result: String, banked: usize },
    Disconnected,
}

#[derive(PartialEq)]
enum Phase {
    Idle,
    Http,
    WsConnecting,
    Ready,
    Dead,
}

/// The (ticket, player_id) login result, or an error string.
type LoginResult = Result<(String, String), String>;

struct Inner {
    base: String,
    phase: Phase,
    ws_tx: Option<WsSender>,
    ws_rx: Option<WsReceiver>,
    http_rx: Option<mpsc::Receiver<LoginResult>>,
    ticket: String,
    player_id: String,
    seq: u32,
    input_seq: u32,
    cmds: VecDeque<ClientCmd>,
    out: VecDeque<ServerMsg>,
}

/// Bevy-side handle. Cloneable (shared `Rc`), single-threaded (NonSend resource).
#[derive(Clone)]
pub struct Net(Rc<RefCell<Inner>>);

/// Create the network layer. No I/O happens until the first `Connect` command.
pub fn start(base: String) -> Net {
    Net(Rc::new(RefCell::new(Inner {
        base,
        phase: Phase::Idle,
        ws_tx: None,
        ws_rx: None,
        http_rx: None,
        ticket: String::new(),
        player_id: String::new(),
        seq: 1,
        input_seq: 0,
        cmds: VecDeque::new(),
        out: VecDeque::new(),
    })))
}

impl Net {
    /// Queue a command (processed on the next `poll`).
    pub fn send(&self, cmd: ClientCmd) {
        self.0.borrow_mut().cmds.push_back(cmd);
    }

    /// Advance the state machine: fire queued commands, pump HTTP + WS.
    pub fn poll(&self) {
        self.0.borrow_mut().step();
    }

    /// Pop the next server event, if any.
    pub fn try_recv(&self) -> Option<ServerMsg> {
        self.0.borrow_mut().out.pop_front()
    }
}

impl Inner {
    fn step(&mut self) {
        // 1. Drain queued commands.
        let cmds: Vec<ClientCmd> = self.cmds.drain(..).collect();
        for cmd in cmds {
            match cmd {
                ClientCmd::Connect { username } if self.phase == Phase::Idle => {
                    self.http_rx = Some(spawn_login(&self.base, &username));
                    self.phase = Phase::Http;
                }
                ClientCmd::Connect { .. } => {} // already connecting/connected
                other if self.phase == Phase::Ready => self.send_cmd(other),
                _ => { /* not connected yet — drop movement/attack */ }
            }
        }

        // 2. HTTP login result → open the socket.
        if self.phase == Phase::Http {
            if let Some(rx) = &self.http_rx {
                match rx.try_recv() {
                    Ok(Ok((ticket, player_id))) => {
                        self.http_rx = None;
                        self.open_socket(ticket, player_id);
                    }
                    Ok(Err(e)) => {
                        self.http_rx = None;
                        self.out.push_back(ServerMsg::Error { message: e });
                        self.phase = Phase::Dead;
                    }
                    Err(mpsc::TryRecvError::Empty) => {}
                    Err(mpsc::TryRecvError::Disconnected) => {
                        self.http_rx = None;
                        self.out.push_back(ServerMsg::Error {
                            message: "login task dropped".into(),
                        });
                        self.phase = Phase::Dead;
                    }
                }
            }
        }

        // 3. Drain socket events.
        let mut events = Vec::new();
        if let Some(rx) = self.ws_rx.as_mut() {
            while let Some(ev) = rx.try_recv() {
                events.push(ev);
            }
        }
        for ev in events {
            self.on_ws_event(ev);
        }
    }

    fn open_socket(&mut self, ticket: String, player_id: String) {
        let ws_url = format!("{}/v1/realtime", self.base.replacen("http", "ws", 1));
        match ewebsock::connect(&ws_url, ewebsock::Options::default()) {
            Ok((tx, rx)) => {
                self.ws_tx = Some(tx);
                self.ws_rx = Some(rx);
                self.ticket = ticket;
                self.player_id = player_id;
                self.seq = 1;
                self.phase = Phase::WsConnecting;
            }
            Err(e) => {
                self.out.push_back(ServerMsg::Error {
                    message: format!("ws connect: {e}"),
                });
                self.phase = Phase::Dead;
            }
        }
    }

    fn on_ws_event(&mut self, ev: WsEvent) {
        match ev {
            WsEvent::Opened => {
                // First frame must be session.authenticate (seq 1).
                self.send_env(
                    ws::Authenticate::TYPE,
                    json!({ "ticket": self.ticket, "resume": null }),
                );
            }
            WsEvent::Message(WsMessage::Text(t)) => self.handle_text(&t),
            WsEvent::Message(_) => {}
            WsEvent::Error(e) => {
                self.out.push_back(ServerMsg::Error { message: e });
                self.phase = Phase::Dead;
            }
            WsEvent::Closed => {
                self.out.push_back(ServerMsg::Disconnected);
                self.phase = Phase::Dead;
            }
        }
    }

    fn send_cmd(&mut self, cmd: ClientCmd) {
        match cmd {
            ClientCmd::EnterMaze => self.send_env(wr::EnterMaze::TYPE, json!({})),
            ClientCmd::Move { dx, dy } => {
                self.input_seq += 1;
                self.send_env(
                    wm::MoveIntent::TYPE,
                    json!({
                        "input_seq": self.input_seq,
                        "move_dir": { "x": dx, "y": dy },
                        "client_pos": { "x": 0.0, "y": 0.0 }
                    }),
                );
            }
            ClientCmd::Attack { battle_id, target } => self.send_env(
                wb::SubmitAction::TYPE,
                json!({
                    "battle_id": battle_id,
                    // v4 (random) not v7 — v7 needs a system clock, which panics
                    // on wasm. Uniqueness is all the server needs here.
                    "action_id": uuid::Uuid::new_v4().to_string(),
                    "action": "attack",
                    "skill_kind": null,
                    "item_id": null,
                    "target_ids": [target]
                }),
            ),
            ClientCmd::Extract => self.send_env(
                wr::BeginExtraction::TYPE,
                json!({ "method": "portal", "portal_entity_id": "portal", "item_id": null }),
            ),
            ClientCmd::Connect { .. } => {}
        }
    }

    fn send_env(&mut self, ty: &str, payload: serde_json::Value) {
        if let Some(tx) = self.ws_tx.as_mut() {
            let env = json!({ "type": ty, "seq": self.seq, "ts": 0u64, "payload": payload });
            tx.send(WsMessage::Text(env.to_string()));
            self.seq += 1;
        }
    }

    fn handle_text(&mut self, text: &str) {
        let raw: RawEnvelope = match serde_json::from_str(text) {
            Ok(r) => r,
            Err(_) => return,
        };
        match raw.msg_type.as_str() {
            "session.authenticated" => {
                self.phase = Phase::Ready;
                self.out.push_back(ServerMsg::Connected {
                    player_id: self.player_id.clone(),
                });
            }
            "session.error" => {
                if let Ok(e) = serde_json::from_value::<ws::Error>(raw.payload) {
                    self.out.push_back(ServerMsg::Error { message: e.message });
                }
            }
            "run.started" => self.out.push_back(ServerMsg::RunStarted),
            "world.snapshot" => {
                if let Ok(s) = serde_json::from_value::<wm::Snapshot>(raw.payload) {
                    let entities = s
                        .entities
                        .into_iter()
                        .map(|e| {
                            let kind = match e.avatar_state.as_deref() {
                                None => EntityKind::Monster,
                                Some("portal") => EntityKind::Portal,
                                Some(_) => EntityKind::Player,
                            };
                            EntityView {
                                id: e.entity_id,
                                x: e.position.x,
                                y: e.position.y,
                                kind,
                            }
                        })
                        .collect();
                    self.out.push_back(ServerMsg::Snapshot { entities });
                }
            }
            "battle.started" => {
                if let Ok(b) = serde_json::from_value::<wb::Started>(raw.payload) {
                    let mut combatants: Vec<CombatantView> =
                        b.allies.iter().map(CombatantView::from_wire).collect();
                    combatants.extend(b.enemies.iter().map(CombatantView::from_wire));
                    let monster_combatant = b.enemies.first().map(|c| c.combatant_id.clone());
                    self.out.push_back(ServerMsg::BattleStarted {
                        battle_id: b.battle_id,
                        your_combatant_id: b.your_combatant_id,
                        combatants,
                        monster_combatant,
                    });
                }
            }
            "battle.turn_ready" => {
                if let Ok(t) = serde_json::from_value::<wb::TurnReady>(raw.payload) {
                    self.out.push_back(ServerMsg::TurnReady {
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
                    self.out.push_back(ServerMsg::Gauge { updates });
                }
            }
            "battle.ended" => {
                if let Ok(e) = serde_json::from_value::<wb::Ended>(raw.payload) {
                    let outcome = serde_json::to_value(e.outcome)
                        .ok()
                        .and_then(|v| v.as_str().map(String::from))
                        .unwrap_or_else(|| "over".to_string());
                    self.out.push_back(ServerMsg::BattleEnded { outcome });
                }
            }
            "run.channel_started" => {
                if let Ok(c) = serde_json::from_value::<wr::ChannelStarted>(raw.payload) {
                    self.out.push_back(ServerMsg::ChannelStarted {
                        completes_at: c.completes_at,
                    });
                }
            }
            "run.channel_interrupted" => self.out.push_back(ServerMsg::ChannelInterrupted),
            "run.member_result" => {
                if let Ok(m) = serde_json::from_value::<wr::MemberResult>(raw.payload) {
                    // Only our own copy carries `banked`; others are notifications.
                    if m.player_id == self.player_id {
                        let result = serde_json::to_value(m.result)
                            .ok()
                            .and_then(|v| v.as_str().map(String::from))
                            .unwrap_or_default();
                        let banked = m.banked.map(|b| b.len()).unwrap_or(0);
                        self.out.push_back(ServerMsg::RunEnded { result, banked });
                    }
                }
            }
            _ => {}
        }
    }
}

/// Kick off register (idempotent) + login via `ehttp`; the result arrives on the
/// returned channel. Works on native (background thread) and wasm (fetch).
fn spawn_login(base: &str, username: &str) -> mpsc::Receiver<LoginResult> {
    let (tx, rx) = mpsc::channel();
    let body = serde_json::to_vec(&json!({ "username": username, "password": GUEST_PASSWORD }))
        .unwrap_or_default();

    let mut reg = ehttp::Request::post(format!("{base}/v1/auth/register"), body.clone());
    reg.headers.insert("Content-Type", "application/json");

    let login_url = format!("{base}/v1/auth/login");
    ehttp::fetch(reg, move |_reg| {
        // Conflict (already registered) is fine — proceed to login regardless.
        let mut login = ehttp::Request::post(&login_url, body);
        login.headers.insert("Content-Type", "application/json");
        ehttp::fetch(login, move |res| {
            let result: LoginResult = match res {
                Ok(resp) if resp.ok => match resp.text() {
                    Some(t) => match serde_json::from_str::<serde_json::Value>(t) {
                        Ok(v) => Ok((
                            v["realtime_ticket"].as_str().unwrap_or_default().to_string(),
                            v["player"]["player_id"]
                                .as_str()
                                .unwrap_or_default()
                                .to_string(),
                        )),
                        Err(e) => Err(format!("login parse: {e}")),
                    },
                    None => Err("login: empty body".into()),
                },
                Ok(resp) => Err(format!("login status {}", resp.status)),
                Err(e) => Err(format!("login request: {e}")),
            };
            let _ = tx.send(result);
        });
    });
    rx
}
