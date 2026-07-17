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
use meld_proto::realtime::{
    battle as wb, lobby as wl, movement as wm, run as wr, session as ws, Message as _,
};
use meld_proto::RawEnvelope;
use serde_json::{json, Value};

const GUEST_PASSWORD: &str = "meld-guest-password";

/// Commands sent from Bevy into the network layer.
pub enum ClientCmd {
    Connect { username: String },
    /// Enter the maze with the built party (one class key per hero slot).
    EnterMaze { party: Vec<String> },
    Move { dx: f64, dy: f64 },
    /// Battle commands. `actor` is which of the player's heroes acts; `target` is the
    /// chosen combatant (an enemy for Attack/offensive Skill, an ally for a
    /// heal/support Skill or Item). Defend is self-cast (no target).
    Attack { battle_id: String, actor: String, target: String },
    Defend { battle_id: String, actor: String },
    Skill { battle_id: String, actor: String, target: String, skill_kind: String },
    Item { battle_id: String, actor: String, item_id: String, target: String },
    /// Begin an extraction channel at the single deep fixed portal.
    Extract,
    /// Consume a Town Portal item to extract from anywhere (the primary way out).
    TownPortal,
    /// Harvest a resource node the avatar is standing next to.
    Harvest { entity_id: String },
    /// Co-op lobby.
    LobbyCreate { party: Vec<String> },
    LobbyJoin { code: String, party: Vec<String> },
    LobbyReady { ready: bool },
    LobbyStart,
    LobbyLeave,
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
    pub level: i32,
    /// Wire statuses — for a Psyker these carry Focus state (`focus_slots:N`,
    /// `focus:<kind>:<stacks>`) that drives the focus UI.
    pub statuses: Vec<String>,
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
            level: c.level,
            statuses: c.statuses.clone(),
        }
    }
}

/// What an overworld entity is (decides how the client draws it).
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum EntityKind {
    Player,
    Monster,
    Portal,
    /// A harvestable resource node (`monster_kind` carries its content id/label).
    Resource,
    /// An impassable terrain feature (`monster_kind` carries its kind, `radius` its size).
    Obstacle,
}

/// A dynamic overworld entity.
#[derive(Clone)]
pub struct EntityView {
    pub id: String,
    pub x: f64,
    pub y: f64,
    pub kind: EntityKind,
    /// Creature content id for monsters, or the terrain kind for obstacles.
    pub monster_kind: Option<String>,
    /// Creature faction for monsters (drives colour); `None` otherwise.
    pub faction: Option<String>,
    /// World-unit radius for obstacles; `0.0` otherwise.
    pub radius: f64,
}

/// One resolved effect for hit feedback (a damage or heal on a combatant).
pub struct HitEffect {
    pub target: String,
    pub kind: String,
    pub amount: Option<i32>,
    pub hp_after: i32,
}

/// A gear row for the inventory screen.
pub struct GearLine {
    pub name: String,
    pub equipped: bool,
    pub max_durability: i32,
    pub base_max_durability: i32,
    pub atk_bonus: i32,
}

/// A meld-skill row for the level-up screen.
pub struct SkillLine {
    pub kind: String,
    pub level: i32,
    pub xp: i64,
}

type InvPayload = (i64, Vec<(String, i32)>, Vec<GearLine>);
type ProgPayload = (Vec<SkillLine>, Vec<String>);

/// Events emitted from the network layer up to Bevy.
pub enum ServerMsg {
    Connected { player_id: String },
    Error { message: String },
    RunStarted,
    /// Waypoints of the guaranteed clear path (world units) — drawn as a trail.
    WorldPath { points: Vec<(f64, f64)> },
    /// Current run backpack (item_kind, quantity), sorted — drives the HUD.
    Backpack { items: Vec<(String, i32)> },
    Snapshot { entities: Vec<EntityView> },
    BattleStarted {
        battle_id: String,
        your_combatant_id: String,
        your_combatant_ids: Vec<String>,
        combatants: Vec<CombatantView>,
        monster_combatant: Option<String>,
    },
    TurnReady { combatant_id: String },
    /// An action resolved — drives hit feedback (floating numbers + flash).
    ActionResolved {
        actor: String,
        action: String,
        effects: Vec<HitEffect>,
    },
    /// A second party merged into the battle (raid merge) — add their combatants.
    CombatantsJoined { combatants: Vec<CombatantView> },
    Gauge { updates: Vec<(String, f64, i32, Vec<String>)> },
    BattleEnded { outcome: String },
    /// An extraction channel began / broke.
    ChannelStarted { completes_at: u64 },
    ChannelInterrupted,
    /// This player's run ended (extracted / died / abandoned), with the count of
    /// items banked on extraction.
    RunEnded { result: String, banked: usize },
    /// Vault + gear, for the overworld inventory screen.
    InventoryData {
        chits: i64,
        materials: Vec<(String, i32)>,
        gear: Vec<GearLine>,
    },
    /// Meld skills + class unlocks, for the overworld level-up screen.
    ProgressData {
        skills: Vec<SkillLine>,
        classes: Vec<String>,
    },
    /// Co-op lobby state — members are (player_id, username, ready).
    LobbyState {
        code: String,
        host: String,
        members: Vec<(String, String, bool)>,
    },
    /// The lobby was disbanded / this player left it.
    LobbyClosed,
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

/// The (ticket, player_id, session_token) login result, or an error string.
type LoginResult = Result<(String, String, String), String>;

struct Inner {
    base: String,
    phase: Phase,
    ws_tx: Option<WsSender>,
    ws_rx: Option<WsReceiver>,
    http_rx: Option<mpsc::Receiver<LoginResult>>,
    inv_rx: Option<mpsc::Receiver<InvPayload>>,
    prog_rx: Option<mpsc::Receiver<ProgPayload>>,
    ticket: String,
    player_id: String,
    /// Bearer token for authenticated HTTP (vault/gear/players).
    session_token: String,
    seq: u32,
    input_seq: u32,
    cmds: VecDeque<ClientCmd>,
    out: VecDeque<ServerMsg>,
    /// Current run backpack counts (item_kind -> quantity), maintained from
    /// `run.started` + `run.backpack_update` so the overworld HUD can show your
    /// Town Portals + gathered materials.
    backpack: std::collections::HashMap<String, i32>,
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
        inv_rx: None,
        prog_rx: None,
        ticket: String::new(),
        player_id: String::new(),
        session_token: String::new(),
        seq: 1,
        input_seq: 0,
        cmds: VecDeque::new(),
        out: VecDeque::new(),
        backpack: std::collections::HashMap::new(),
    })))
}

impl Net {
    /// Queue a command (processed on the next `poll`).
    pub fn send(&self, cmd: ClientCmd) {
        self.0.borrow_mut().cmds.push_back(cmd);
    }

    /// Kick off an authenticated GET of vault + gear (→ `InventoryData`).
    pub fn fetch_inventory(&self) {
        self.0.borrow_mut().fetch_inventory();
    }

    /// Kick off an authenticated GET of the player profile (→ `ProgressData`).
    pub fn fetch_progress(&self) {
        self.0.borrow_mut().fetch_progress();
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
                    Ok(Ok((ticket, player_id, session_token))) => {
                        self.http_rx = None;
                        self.session_token = session_token;
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

        // 2b. Drain any HTTP data fetches (inventory / progress screens).
        if let Some(rx) = &self.inv_rx {
            if let Ok((chits, materials, gear)) = rx.try_recv() {
                self.inv_rx = None;
                self.out.push_back(ServerMsg::InventoryData {
                    chits,
                    materials,
                    gear,
                });
            }
        }
        if let Some(rx) = &self.prog_rx {
            if let Ok((skills, classes)) = rx.try_recv() {
                self.prog_rx = None;
                self.out.push_back(ServerMsg::ProgressData { skills, classes });
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

    /// GET `/v1/vault` then `/v1/vault/gear` (Bearer auth); deliver combined.
    fn fetch_inventory(&mut self) {
        if self.session_token.is_empty() {
            return;
        }
        let (tx, rx) = mpsc::channel();
        self.inv_rx = Some(rx);
        let base = self.base.clone();
        let token = self.session_token.clone();
        let gear_url = format!("{base}/v1/vault/gear");
        let mut req = ehttp::Request::get(format!("{base}/v1/vault"));
        req.headers.insert("Authorization", format!("Bearer {token}"));
        ehttp::fetch(req, move |vault_res| {
            let mut chits = 0i64;
            let mut materials = Vec::new();
            if let Ok(resp) = &vault_res {
                if let Some(v) = resp.text().and_then(|t| serde_json::from_str::<Value>(t).ok()) {
                    chits = v["chits"].as_i64().unwrap_or(0);
                    if let Some(arr) = v["materials"].as_array() {
                        materials = arr
                            .iter()
                            .map(|m| {
                                (
                                    m["item_kind"].as_str().unwrap_or("?").to_string(),
                                    m["quantity"].as_i64().unwrap_or(0) as i32,
                                )
                            })
                            .collect();
                    }
                }
            }
            let mut greq = ehttp::Request::get(&gear_url);
            greq.headers.insert("Authorization", format!("Bearer {token}"));
            ehttp::fetch(greq, move |gear_res| {
                let mut gear = Vec::new();
                if let Ok(resp) = &gear_res {
                    if let Some(v) = resp.text().and_then(|t| serde_json::from_str::<Value>(t).ok())
                    {
                        if let Some(arr) = v["data"].as_array() {
                            gear = arr
                                .iter()
                                .map(|g| GearLine {
                                    name: g["name"].as_str().unwrap_or("?").to_string(),
                                    equipped: g["equipped"].as_bool().unwrap_or(false),
                                    max_durability: g["max_durability"].as_i64().unwrap_or(0) as i32,
                                    base_max_durability: g["base_max_durability"].as_i64().unwrap_or(0)
                                        as i32,
                                    atk_bonus: g["atk_bonus"].as_i64().unwrap_or(0) as i32,
                                })
                                .collect();
                        }
                    }
                }
                let _ = tx.send((chits, materials, gear));
            });
        });
    }

    /// GET `/v1/players/me` (Bearer auth) for meld skills + class unlocks.
    fn fetch_progress(&mut self) {
        if self.session_token.is_empty() {
            return;
        }
        let (tx, rx) = mpsc::channel();
        self.prog_rx = Some(rx);
        let token = self.session_token.clone();
        let mut req = ehttp::Request::get(format!("{}/v1/players/me", self.base));
        req.headers.insert("Authorization", format!("Bearer {token}"));
        ehttp::fetch(req, move |res| {
            let mut skills = Vec::new();
            let mut classes = Vec::new();
            if let Ok(resp) = &res {
                if let Some(v) = resp.text().and_then(|t| serde_json::from_str::<Value>(t).ok()) {
                    if let Some(arr) = v["meld_skills"].as_array() {
                        skills = arr
                            .iter()
                            .map(|s| SkillLine {
                                kind: s["skill_kind"].as_str().unwrap_or("?").to_string(),
                                level: s["level"].as_i64().unwrap_or(1) as i32,
                                xp: s["xp"].as_i64().unwrap_or(0),
                            })
                            .collect();
                    }
                    if let Some(arr) = v["class_unlocks"].as_array() {
                        classes = arr
                            .iter()
                            .filter_map(|c| c.as_str().map(String::from))
                            .collect();
                    }
                }
            }
            let _ = tx.send((skills, classes));
        });
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
            // The client's direct enter is always a solo (private) dive; co-op
            // goes through the lobby. (Bot tests that want grouping send raw JSON
            // without `solo`.)
            ClientCmd::EnterMaze { party } => {
                self.send_env(wr::EnterMaze::TYPE, json!({ "party": party, "solo": true }))
            }
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
            // v4 (random) not v7 for action_id — v7 needs a system clock, which
            // panics on wasm. Uniqueness is all the server needs here.
            ClientCmd::Attack {
                battle_id,
                actor,
                target,
            } => self.send_env(
                wb::SubmitAction::TYPE,
                json!({
                    "battle_id": battle_id,
                    "action_id": uuid::Uuid::new_v4().to_string(),
                    "actor_combatant_id": actor,
                    "action": "attack",
                    "skill_kind": null,
                    "item_id": null,
                    "target_ids": [target]
                }),
            ),
            ClientCmd::Defend { battle_id, actor } => self.send_env(
                wb::SubmitAction::TYPE,
                json!({
                    "battle_id": battle_id,
                    "action_id": uuid::Uuid::new_v4().to_string(),
                    "actor_combatant_id": actor,
                    "action": "defend",
                    "skill_kind": null,
                    "item_id": null,
                    "target_ids": null
                }),
            ),
            ClientCmd::Skill {
                battle_id,
                actor,
                target,
                skill_kind,
            } => self.send_env(
                wb::SubmitAction::TYPE,
                json!({
                    "battle_id": battle_id,
                    "action_id": uuid::Uuid::new_v4().to_string(),
                    "actor_combatant_id": actor,
                    "action": "skill",
                    "skill_kind": skill_kind,
                    "item_id": null,
                    "target_ids": [target]
                }),
            ),
            ClientCmd::Item {
                battle_id,
                actor,
                item_id,
                target,
            } => self.send_env(
                wb::SubmitAction::TYPE,
                json!({
                    "battle_id": battle_id,
                    "action_id": uuid::Uuid::new_v4().to_string(),
                    "actor_combatant_id": actor,
                    "action": "item",
                    "skill_kind": null,
                    "item_id": item_id,
                    "target_ids": [target]
                }),
            ),
            ClientCmd::Extract => self.send_env(
                wr::BeginExtraction::TYPE,
                json!({ "method": "portal", "portal_entity_id": "portal", "item_id": null }),
            ),
            ClientCmd::TownPortal => self.send_env(
                wr::BeginExtraction::TYPE,
                json!({ "method": "town_portal", "portal_entity_id": null, "item_id": null }),
            ),
            ClientCmd::Harvest { entity_id } => {
                self.send_env(wr::Harvest::TYPE, json!({ "entity_id": entity_id }))
            }
            ClientCmd::LobbyCreate { party } => {
                self.send_env(wl::Create::TYPE, json!({ "party": party }))
            }
            ClientCmd::LobbyJoin { code, party } => {
                self.send_env(wl::Join::TYPE, json!({ "code": code, "party": party }))
            }
            ClientCmd::LobbyReady { ready } => {
                self.send_env(wl::Ready::TYPE, json!({ "ready": ready }))
            }
            ClientCmd::LobbyStart => self.send_env(wl::Start::TYPE, json!({})),
            ClientCmd::LobbyLeave => self.send_env(wl::Leave::TYPE, json!({})),
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

    /// Emit the current backpack as a sorted (item_kind, qty) list for the HUD.
    fn emit_backpack(&mut self) {
        let mut items: Vec<(String, i32)> =
            self.backpack.iter().map(|(k, v)| (k.clone(), *v)).collect();
        items.sort_by(|a, b| a.0.cmp(&b.0));
        self.out.push_back(ServerMsg::Backpack { items });
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
            "run.started" => {
                self.backpack.clear();
                if let Some(items) = raw.payload["backpack"].as_array() {
                    for it in items {
                        let kind = it["item_kind"].as_str().unwrap_or("").to_string();
                        let qty = it["quantity"].as_i64().unwrap_or(0) as i32;
                        if !kind.is_empty() {
                            *self.backpack.entry(kind).or_insert(0) += qty;
                        }
                    }
                }
                self.out.push_back(ServerMsg::RunStarted);
                self.emit_backpack();
                if let Some(pts) = raw.payload["path"].as_array() {
                    let points: Vec<(f64, f64)> = pts
                        .iter()
                        .filter_map(|p| Some((p["x"].as_f64()?, p["y"].as_f64()?)))
                        .collect();
                    if !points.is_empty() {
                        self.out.push_back(ServerMsg::WorldPath { points });
                    }
                }
            }
            "run.backpack_update" => {
                for ch in raw.payload["changes"].as_array().into_iter().flatten() {
                    let kind = ch["item"]["item_kind"].as_str().unwrap_or("").to_string();
                    let qty = ch["item"]["quantity"].as_i64().unwrap_or(0) as i32;
                    if kind.is_empty() {
                        continue;
                    }
                    let signed = if ch["delta"].as_str() == Some("removed") { -qty } else { qty };
                    let e = self.backpack.entry(kind).or_insert(0);
                    *e += signed;
                    if *e <= 0 {
                        let k = ch["item"]["item_kind"].as_str().unwrap_or("").to_string();
                        self.backpack.remove(&k);
                    }
                }
                self.emit_backpack();
            }
            "lobby.state" => {
                let members = raw.payload["members"]
                    .as_array()
                    .map(|arr| {
                        arr.iter()
                            .map(|m| {
                                (
                                    m["player_id"].as_str().unwrap_or("").to_string(),
                                    m["username"].as_str().unwrap_or("").to_string(),
                                    m["ready"].as_bool().unwrap_or(false),
                                )
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                self.out.push_back(ServerMsg::LobbyState {
                    code: raw.payload["code"].as_str().unwrap_or("").to_string(),
                    host: raw.payload["host_player_id"].as_str().unwrap_or("").to_string(),
                    members,
                });
            }
            "lobby.closed" => self.out.push_back(ServerMsg::LobbyClosed),
            "world.snapshot" => {
                if let Ok(s) = serde_json::from_value::<wm::Snapshot>(raw.payload) {
                    let entities = s
                        .entities
                        .into_iter()
                        .map(|e| {
                            // Server tags monsters `mob:<kind>:<faction>`, the portal
                            // `portal`, and players with their avatar state (`active`, …).
                            let mut radius = 0.0;
                            let (kind, monster_kind, faction) = match e.avatar_state.as_deref() {
                                Some("portal") => (EntityKind::Portal, None, None),
                                Some(s) if s.starts_with("mob:") => {
                                    let rest = &s["mob:".len()..];
                                    let (k, f) = rest.split_once(':').unwrap_or((rest, ""));
                                    (
                                        EntityKind::Monster,
                                        Some(k.to_string()),
                                        (!f.is_empty()).then(|| f.to_string()),
                                    )
                                }
                                Some(s) if s.starts_with("resource:") => {
                                    (EntityKind::Resource, Some(s["resource:".len()..].to_string()), None)
                                }
                                Some(s) if s.starts_with("obstacle:") => {
                                    // obstacle:<kind>:<radius>
                                    let rest = &s["obstacle:".len()..];
                                    let (k, r) = rest.rsplit_once(':').unwrap_or((rest, "1"));
                                    radius = r.parse().unwrap_or(1.0);
                                    (EntityKind::Obstacle, Some(k.to_string()), None)
                                }
                                _ => (EntityKind::Player, None, None),
                            };
                            EntityView {
                                id: e.entity_id,
                                x: e.position.x,
                                y: e.position.y,
                                kind,
                                monster_kind,
                                faction,
                                radius,
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
                    let your_combatant_ids = if b.your_combatant_ids.is_empty() {
                        vec![b.your_combatant_id.clone()]
                    } else {
                        b.your_combatant_ids.clone()
                    };
                    self.out.push_back(ServerMsg::BattleStarted {
                        battle_id: b.battle_id,
                        your_combatant_id: b.your_combatant_id,
                        your_combatant_ids,
                        combatants,
                        monster_combatant,
                    });
                }
            }
            "battle.action_resolved" => {
                if let Ok(r) = serde_json::from_value::<wb::ActionResolved>(raw.payload) {
                    let action = serde_json::to_value(r.action)
                        .ok()
                        .and_then(|v| v.as_str().map(String::from))
                        .unwrap_or_default();
                    let effects = r
                        .effects
                        .into_iter()
                        .map(|e| {
                            let kind = serde_json::to_value(e.kind)
                                .ok()
                                .and_then(|v| v.as_str().map(String::from))
                                .unwrap_or_default();
                            HitEffect {
                                target: e.target_id,
                                kind,
                                amount: e.amount,
                                hp_after: e.hp_after,
                            }
                        })
                        .collect();
                    self.out.push_back(ServerMsg::ActionResolved {
                        actor: r.actor_id,
                        action,
                        effects,
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
            "battle.party_joined" => {
                if let Ok(p) = serde_json::from_value::<wb::PartyJoined>(raw.payload) {
                    let combatants = p.joining_allies.iter().map(CombatantView::from_wire).collect();
                    self.out.push_back(ServerMsg::CombatantsJoined { combatants });
                }
            }
            "battle.gauge_update" => {
                if let Ok(g) = serde_json::from_value::<wb::GaugeUpdate>(raw.payload) {
                    let updates = g
                        .combatants
                        .into_iter()
                        .map(|c| (c.combatant_id, c.gauge, c.hp, c.statuses))
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
                            v["session_token"].as_str().unwrap_or_default().to_string(),
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
