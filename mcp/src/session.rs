//! One live game: a server booted in-process, a real WebSocket, and the state a tool reads.
//!
//! Nothing here is a shortcut into the engine. The session registers an account over the
//! HTTP API, opens `/v1/realtime`, and speaks the same `meld-proto` envelopes the Bevy
//! client does — so a tool call is a **player input**, and everything a tool reports came
//! back from the authoritative loop. A "play the game" harness that reached into
//! `MazeInstance` directly would measure the model rather than the game, which is the exact
//! mistake the end fight already shipped twice.
//!
//! The database is `memory://`, so a game costs nothing to start, needs no Postgres, and
//! cannot collide with another agent's server (the port is ephemeral). Every `new_game`
//! boots a fresh one and drops the last.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio::net::TcpListener;
use tokio::sync::{mpsc, Mutex, Notify};
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;

/// How a game is set up. Everything optional is a DEV/QA override — the defaults are the
/// game as a player meets it.
#[derive(Debug, Clone)]
pub struct Boot {
    pub party: Vec<String>,
    pub seed: Option<u64>,
    pub tutorial: bool,
    /// Start every hero at this level instead of 1 (`MELD_START_LEVEL`). The deep content
    /// is authored for level ~100, and walking there takes an hour.
    pub start_level: Option<i32>,
    /// Dress every hero in a full six-slot set of tier-`n` insured epics (`MELD_GEAR_TIER`).
    pub gear_tier: Option<i32>,
    /// Place the end fight at this distance instead of d3200 (`MELD_END_FIGHT`).
    pub end_fight_at: Option<f64>,
    pub biome: Option<String>,
    pub dungeon: Option<String>,
    /// Salves and elixirs each hero's pouch is dealt at the start (`MELD_POTIONS`). A party
    /// that walked to the end-world has been looting for an hour and can shop before it
    /// dives, so measuring the apex against the 3-salve starting kit measures an
    /// unprepared party — and a salve is 40% of a hero's max HP.
    pub potions: Option<i32>,
    /// Dress the harness set with an elemental ward (`MELD_GEAR_WARD`) — what a player who
    /// knew what they were walking into would bring. Armour weight only answers blades,
    /// points and hammers, so without this a fire fight cannot show gear mattering at all.
    pub ward: Option<String>,
}

impl Default for Boot {
    fn default() -> Self {
        Boot {
            party: ["explorer", "hunter", "psyker", "resonant"]
                .iter()
                .map(|s| s.to_string())
                .collect(),
            seed: None,
            tutorial: false,
            start_level: None,
            gear_tier: None,
            end_fight_at: None,
            biome: None,
            dungeon: None,
            potions: None,
            ward: None,
        }
    }
}

/// One thing standing in the world, as the snapshot describes it.
#[derive(Debug, Clone)]
pub struct Ent {
    pub id: String,
    /// `avatar_state`: `mob:<kind>:<faction>`, `resource:<kind>`, `portal`, `chest`, …
    pub state: String,
    pub x: f64,
    pub y: f64,
    pub level: i64,
    pub encounter_class: String,
    pub elevation: i64,
}

impl Ent {
    /// `mob:dune_wyrm:beast` → `dune_wyrm`.
    pub fn kind(&self) -> &str {
        self.state.split(':').nth(1).unwrap_or(&self.state)
    }
    pub fn is_mob(&self) -> bool {
        self.state.starts_with("mob:")
    }
    pub fn dist_to(&self, p: (f64, f64)) -> f64 {
        ((self.x - p.0).powi(2) + (self.y - p.1).powi(2)).sqrt()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Ally,
    Enemy,
}

/// One fighter in the arena.
#[derive(Debug, Clone)]
pub struct Comb {
    pub id: String,
    pub side: Side,
    pub name: String,
    pub class: String,
    pub level: i32,
    pub hp: i32,
    pub max_hp: i32,
    pub gauge: f64,
    pub statuses: Vec<String>,
    /// Whether this is one of MINE — the heroes this session commands.
    pub mine: bool,
}

impl Comb {
    fn from_json(v: &Value, side: Side, mine: bool) -> Comb {
        let statuses: Vec<String> = v["statuses"]
            .as_array()
            .map(|a| a.iter().filter_map(|s| s.as_str().map(String::from)).collect())
            .unwrap_or_default();
        let tok = |p: &str| {
            statuses
                .iter()
                .find_map(|s| s.strip_prefix(p).map(String::from))
                .unwrap_or_default()
        };
        let name = {
            let n = tok("name:");
            if !n.is_empty() {
                n
            } else {
                v["monster_kind"].as_str().unwrap_or("creature").to_string()
            }
        };
        Comb {
            id: v["combatant_id"].as_str().unwrap_or_default().to_string(),
            side,
            name,
            class: tok("class:"),
            level: v["level"].as_i64().unwrap_or(0) as i32,
            hp: v["hp"].as_i64().unwrap_or(0) as i32,
            max_hp: v["max_hp"].as_i64().unwrap_or(0) as i32,
            gauge: v["gauge"].as_f64().unwrap_or(0.0),
            statuses,
            mine,
        }
    }
    pub fn alive(&self) -> bool {
        self.hp > 0
    }
}

/// A fight in progress, or the one that just finished.
#[derive(Debug, Clone, Default)]
pub struct Battle {
    pub id: String,
    pub encounter_class: String,
    pub combatants: Vec<Comb>,
    /// Combatants of mine whose gauge has filled and whose 15s window is open.
    pub ready: Vec<String>,
    pub ended: Option<String>,
    /// Turns MY heroes have been offered — the honest unit for "how long did this take".
    pub my_turns: usize,
    /// Party HP at the opening bell, so damage taken is a subtraction rather than a guess.
    pub opening_hp: i32,
    pub loot: Vec<String>,
}

impl Battle {
    /// Drop the fallen from the waiting list.
    ///
    /// A hero's gauge fills, then it is killed before it acts — and nothing takes its
    /// window back, because a window is only closed by a resolution and a corpse never
    /// resolves one. Commanding it gets `Reject::NotFound`, forever, which reads as the
    /// server rejecting a legal order rather than as the hero being dead.
    fn prune_ready(&mut self) {
        let fallen: Vec<String> = self
            .combatants
            .iter()
            .filter(|c| !c.alive())
            .map(|c| c.id.clone())
            .collect();
        self.ready.retain(|id| !fallen.contains(id));
    }

    pub fn get(&self, id: &str) -> Option<&Comb> {
        self.combatants.iter().find(|c| c.id == id)
    }
    pub fn mine(&self) -> Vec<&Comb> {
        self.combatants.iter().filter(|c| c.mine).collect()
    }
    pub fn enemies(&self) -> Vec<&Comb> {
        self.combatants.iter().filter(|c| c.side == Side::Enemy).collect()
    }
    pub fn party_hp(&self) -> i32 {
        self.combatants.iter().filter(|c| c.side == Side::Ally).map(|c| c.hp).sum()
    }
}

/// Everything a tool can look at. Rebuilt from authoritative messages only.
#[derive(Default)]
pub struct State {
    pub player_id: String,
    pub pos: (f64, f64),
    pub entities: Vec<Ent>,
    /// `run.party` heroes, verbatim — name/class/level/attributes/abilities.
    pub heroes: Vec<Value>,
    pub perks: Vec<String>,
    pub backpack: HashMap<String, i64>,
    pub base_level: i32,
    pub run_active: bool,
    pub run_result: Option<String>,
    pub battle: Option<Battle>,
    pub last_battle: Option<Battle>,
    /// Everything worth reading since the last time a tool drained it.
    pub log: Vec<String>,
    /// The chat transcript, kept whole rather than drained — you can scroll back on what
    /// somebody said to you, and that is the difference between chat and a notification.
    pub chat: Vec<String>,
    /// Per-hero-slot pouch contents. A hero in a fight may only drink what IT carries —
    /// the shared bag is out of reach — so a policy that reads the backpack would offer
    /// potions no hero can actually reach.
    pub pouches: Vec<Vec<(String, i64)>>,
    /// Waypoints of the world's GUARANTEED clear path, hub to deep portal — obstacles are
    /// rejection-sampled out of its tube by construction, so following it is the one route
    /// that always exists. Marching due east instead wedges against a cliff: measured, a
    /// party aiming at d500 stalled at d222 doing exactly that.
    pub path: Vec<(f64, f64)>,
    /// Set the moment the end fight is felled — the one event this whole harness exists for.
    pub world_end: Option<String>,
    pub steering: String,
}

impl State {
    fn note(&mut self, line: impl Into<String>) {
        self.log.push(line.into());
        if self.log.len() > 600 {
            self.log.drain(..200);
        }
    }
    pub fn drain_log(&mut self) -> Vec<String> {
        std::mem::take(&mut self.log)
    }
    pub fn in_battle(&self) -> bool {
        self.battle.as_ref().is_some_and(|b| b.ended.is_none())
    }
    /// The distance band the world scales everything off — floored, as every threshold is.
    pub fn distance(&self) -> i64 {
        (self.pos.0.powi(2) + self.pos.1.powi(2)).sqrt().floor() as i64
    }
    pub fn ent(&self, id: &str) -> Option<&Ent> {
        self.entities.iter().find(|e| e.id == id)
    }
}

/// Where the avatar is being walked.
#[derive(Debug, Clone)]
pub enum Steer {
    Stop,
    Dir(f64, f64),
    Toward(String),
}

enum Cmd {
    Send(Value),
    Steer(Steer),
}

/// A live game. Dropping it drops the socket and the server with it.
pub struct Session {
    pub state: Arc<Mutex<State>>,
    /// Bumped on every authoritative message, so a tool can wait for one rather than poll.
    pub bump: Arc<Notify>,
    tx: mpsc::UnboundedSender<Cmd>,
    pub boot: Boot,
    pub addr: String,
    _shutdown: mpsc::Sender<()>,
}

impl Session {
    /// Boot a server, register an account, dive, and return once the run has started.
    pub async fn start(boot: Boot) -> Result<Session, String> {
        // The world-shaping overrides are read at the server boundary (`run.enter_maze`),
        // so they must be in the environment before the dive — not before the build.
        set_or_clear("MELD_SEED", boot.seed.map(|s| s.to_string()));
        set_or_clear("MELD_BIOME", boot.biome.clone());
        set_or_clear("MELD_DUNGEON", boot.dungeon.clone());
        set_or_clear("MELD_GEAR_TIER", boot.gear_tier.map(|t| t.to_string()));
        set_or_clear("MELD_START_LEVEL", boot.start_level.map(|l| l.to_string()));
        set_or_clear("MELD_POTIONS", boot.potions.map(|n| n.to_string()));
        set_or_clear("MELD_GEAR_WARD", boot.ward.clone());
        match boot.end_fight_at {
            Some(d) => {
                std::env::set_var("MELD_END_FIGHT", "1");
                std::env::set_var("MELD_END_FIGHT_AT", d.to_string());
            }
            None => {
                std::env::remove_var("MELD_END_FIGHT");
                std::env::remove_var("MELD_END_FIGHT_AT");
            }
        }

        let balance = meld_balance::Balance::load_default().map_err(|e| format!("balance: {e}"))?;
        let config = meld_server::Config {
            bind_addr: "127.0.0.1:0".to_string(),
            // Ephemeral and dependency-free: a game costs nothing to start and nothing to
            // throw away, which is what makes "boot a fresh world per question" reasonable.
            database_url: "memory://meld-mcp".to_string(),
            balance: Arc::new(balance),
            client_dist: None,
        };
        let built = meld_server::build(&config).await?;
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .map_err(|e| format!("bind: {e}"))?;
        let addr = listener.local_addr().map_err(|e| e.to_string())?.to_string();
        let (shutdown, mut shutdown_rx) = mpsc::channel::<()>(1);
        tokio::spawn(async move {
            let _ = axum::serve(listener, built.router)
                .with_graceful_shutdown(async move {
                    let _ = shutdown_rx.recv().await;
                })
                .await;
        });

        let http = reqwest::Client::new();
        let base = format!("http://{addr}");
        let username = format!("mcp_{}", &uuid::Uuid::new_v4().simple().to_string()[..10]);
        let creds = json!({ "username": username, "password": "correct-horse-battery" });
        http.post(format!("{base}/v1/auth/register"))
            .json(&creds)
            .send()
            .await
            .map_err(|e| format!("register: {e}"))?;
        let login: Value = http
            .post(format!("{base}/v1/auth/login"))
            .json(&creds)
            .send()
            .await
            .map_err(|e| format!("login: {e}"))?
            .json()
            .await
            .map_err(|e| format!("login body: {e}"))?;
        let ticket = login["realtime_ticket"]
            .as_str()
            .ok_or_else(|| format!("no ticket in {login}"))?
            .to_string();
        let player_id = login["player"]["player_id"].as_str().unwrap_or_default().to_string();

        // Every class and every party slot. `run.enter_maze` CLAMPS a party to what the
        // account owns, so without this a four-hero request silently collapses to one
        // Explorer — the composition would look honoured and simply not be.
        let keys: Vec<String> = meld_proto::unlocks::UNLOCKS
            .iter()
            .map(|u| u.key.to_string())
            .collect();
        if let Ok(pid) = uuid::Uuid::parse_str(&player_id) {
            let _ = built.db.grant_unlocks(pid, &keys).await;
        }

        let (mut ws, _) = connect_async(format!("ws://{addr}/v1/realtime"))
            .await
            .map_err(|e| format!("realtime connect: {e}"))?;

        let state = Arc::new(Mutex::new(State {
            player_id: player_id.clone(),
            ..Default::default()
        }));
        let bump = Arc::new(Notify::new());
        let (tx, mut rx) = mpsc::unbounded_channel::<Cmd>();

        let party: Vec<Value> = boot.party.iter().map(|c| json!(c)).collect();
        let tutorial = boot.tutorial;
        let st = state.clone();
        let nudge = bump.clone();
        tokio::spawn(async move {
            let mut seq = 1u32;
            let mut input_seq = 0u32;
            let mut steer = Steer::Stop;
            let _ = send_env(
                &mut ws,
                &mut seq,
                json!({"type":"session.authenticate",
                       "payload":{"ticket":ticket,"resume":null}}),
            )
            .await;

            let mut mover = tokio::time::interval(Duration::from_millis(80));
            mover.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                tokio::select! {
                    cmd = rx.recv() => {
                        match cmd {
                            None => break,
                            Some(Cmd::Steer(s)) => {
                                let mut g = st.lock().await;
                                g.steering = match &s {
                                    Steer::Stop => String::new(),
                                    Steer::Dir(x, y) => format!("heading ({x:.2}, {y:.2})"),
                                    Steer::Toward(id) => format!("walking at {id}"),
                                };
                                steer = s;
                            }
                            Some(Cmd::Send(v)) => { let _ = send_env(&mut ws, &mut seq, v).await; }
                        }
                    }
                    _ = mover.tick() => {
                        let heading = {
                            let g = st.lock().await;
                            if g.in_battle() || !g.run_active { None } else { heading_for(&g, &steer) }
                        };
                        if let Some((dx, dy)) = heading {
                            input_seq += 1;
                            let _ = send_env(&mut ws, &mut seq, json!({"type":"movement.move_intent",
                                "payload":{"input_seq":input_seq,"move_dir":{"x":dx,"y":dy},
                                           "client_pos":{"x":0.0,"y":0.0}}})).await;
                        }
                    }
                    msg = ws.next() => {
                        let Some(Ok(Message::Text(t))) = msg else { break };
                        let Ok(v) = serde_json::from_str::<Value>(&t) else { continue };
                        let mut g = st.lock().await;
                        let dive = apply(&mut g, &v);
                        drop(g);
                        if dive {
                            let _ = send_env(&mut ws, &mut seq, json!({"type":"run.enter_maze",
                                "payload":{"tutorial": tutorial, "solo": true, "party": party}}))
                                .await;
                        }
                        nudge.notify_waiters();
                    }
                }
            }
        });

        let session = Session {
            state,
            bump,
            tx,
            boot,
            addr,
            _shutdown: shutdown,
        };
        // A dive that never starts is the difference between "the game is hard" and "the
        // harness is broken", so surface it here rather than in whatever tool asks next.
        session
            .wait_until(Duration::from_secs(15), |s| s.run_active || s.run_result.is_some())
            .await;
        if !session.state.lock().await.run_active {
            return Err("the run never started (no run.started within 15s)".into());
        }
        // `run.started` precedes the first snapshot by a tick, so returning on it alone
        // hands back an opening view of an empty world — which reads exactly like a world
        // that failed to generate.
        session
            .wait_until(Duration::from_secs(5), |s| {
                !s.entities.is_empty() && !s.heroes.is_empty()
            })
            .await;
        Ok(session)
    }

    pub fn send(&self, v: Value) {
        let _ = self.tx.send(Cmd::Send(v));
    }

    pub fn steer(&self, s: Steer) {
        let _ = self.tx.send(Cmd::Steer(s));
    }

    /// Block until `cond` holds or `budget` runs out. Returns whether it held.
    ///
    /// Registers for the wake-up BEFORE testing the condition, so a message that lands
    /// between the test and the wait is not missed — the bug that makes this kind of
    /// helper hang forever roughly one time in fifty.
    pub async fn wait_until<F>(&self, budget: Duration, mut cond: F) -> bool
    where
        F: FnMut(&State) -> bool,
    {
        let deadline = tokio::time::Instant::now() + budget;
        loop {
            let waiter = self.bump.notified();
            let held = self.state.lock().await;
            let ok = cond(&held);
            drop(held);
            if ok {
                return true;
            }
            let left = deadline.saturating_duration_since(tokio::time::Instant::now());
            if left.is_zero() {
                return false;
            }
            let _ = tokio::time::timeout(left, waiter).await;
        }
    }
}

type Ws = tokio_tungstenite::WebSocketStream<
    tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
>;

async fn send_env(ws: &mut Ws, seq: &mut u32, v: Value) -> Result<(), ()> {
    let env = json!({"type": v["type"], "seq": *seq, "ts": 0, "payload": v["payload"]});
    *seq += 1;
    ws.send(Message::Text(env.to_string())).await.map_err(|_| ())
}

fn set_or_clear(key: &str, val: Option<String>) {
    match val {
        Some(v) => std::env::set_var(key, v),
        None => std::env::remove_var(key),
    }
}

/// The direction to walk this tick, or `None` to stand still.
fn heading_for(s: &State, steer: &Steer) -> Option<(f64, f64)> {
    match steer {
        Steer::Stop => None,
        Steer::Dir(x, y) => Some((*x, *y)),
        Steer::Toward(id) => {
            // A target that has left the snapshot is a target that has been fought, felled
            // or culled — keep walking at its last known spot and the avatar marches into
            // the fog, so stop instead.
            let e = s.ent(id)?;
            let (dx, dy) = (e.x - s.pos.0, e.y - s.pos.1);
            let d = (dx * dx + dy * dy).sqrt();
            if d < 1e-6 {
                return None;
            }
            Some((dx / d, dy / d))
        }
    }
}

/// Fold one authoritative message into the state. Returns true when it is time to dive.
fn apply(s: &mut State, v: &Value) -> bool {
    let p = &v["payload"];
    let empty: Vec<Value> = Vec::new();
    match v["type"].as_str().unwrap_or("") {
        "session.authenticated" => return true,
        "session.error" => {
            s.note(format!("! server refused: {}", p["message"].as_str().unwrap_or("?")))
        }
        "world.snapshot" => {
            let mut ents = Vec::new();
            for e in p["entities"].as_array().unwrap_or(&empty) {
                let id = e["entity_id"].as_str().unwrap_or_default().to_string();
                let (x, y) = (
                    e["position"]["x"].as_f64().unwrap_or(0.0),
                    e["position"]["y"].as_f64().unwrap_or(0.0),
                );
                if id == s.player_id {
                    s.pos = (x, y);
                }
                ents.push(Ent {
                    id,
                    state: e["avatar_state"].as_str().unwrap_or_default().to_string(),
                    x,
                    y,
                    level: e["mob_level"].as_i64().unwrap_or(0),
                    encounter_class: e["encounter_class"].as_str().unwrap_or("standard").to_string(),
                    elevation: e["level"].as_i64().unwrap_or(0),
                });
            }
            s.entities = ents;
        }
        "run.started" => {
            s.run_active = true;
            s.path = p["path"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(|q| Some((q["x"].as_f64()?, q["y"].as_f64()?)))
                        .collect()
                })
                .unwrap_or_default();
            s.base_level = p["base_run_level"].as_i64().unwrap_or(1) as i32;
            s.note(format!("dive started — heroes at level {}", s.base_level));
        }
        "run.party" => {
            if let Some(h) = p["heroes"].as_array() {
                s.heroes = h.clone();
            }
        }
        "run.pouches" => {
            let mut out: Vec<Vec<(String, i64)>> = Vec::new();
            for pv in p["pouches"].as_array().unwrap_or(&empty) {
                let slot = pv["hero_slot"].as_i64().unwrap_or(0).max(0) as usize;
                if out.len() <= slot {
                    out.resize(slot + 1, Vec::new());
                }
                out[slot] = pv["items"]
                    .as_array()
                    .map(|a| {
                        a.iter()
                            .map(|i| {
                                (
                                    i["item_kind"].as_str().unwrap_or("?").to_string(),
                                    i["quantity"].as_i64().unwrap_or(0),
                                )
                            })
                            .collect()
                    })
                    .unwrap_or_default();
            }
            s.pouches = out;
        }
        // A streamed section extends the trail; without this the path runs out at the end of
        // the initial chain and travel goes blind again exactly when it gets interesting.
        "world.terrain_section" => {
            for q in p["path"].as_array().unwrap_or(&empty) {
                if let (Some(x), Some(y)) = (q["x"].as_f64(), q["y"].as_f64()) {
                    s.path.push((x, y));
                }
            }
        }
        "run.perks" => {
            s.perks = p["perks"]
                .as_array()
                .map(|a| a.iter().map(|x| x.to_string()).collect())
                .unwrap_or_default();
        }
        "run.level_up" => s.note(format!(
            "level up: hero {} is now {}",
            p["hero_slot"].as_i64().unwrap_or(0),
            p["level"].as_i64().unwrap_or(0)
        )),
        // The wire shape is `{changes: [{item, delta, cause}], chits_delta, gear_added}`
        // — NOT `{added, removed}` with a `count`, which is what this read for as long as
        // it has existed. Both lookups missed silently (`unwrap_or(&empty)`), so the
        // harness reported an empty bag no matter what you gathered, and every
        // measurement of gathering, loot or spending taken through it was blind.
        // Mirrors `meld-client`'s parser, which is the known-good reader of this message.
        "run.backpack_update" => {
            for ch in p["changes"].as_array().unwrap_or(&empty) {
                let k = ch["item"]["item_kind"].as_str().unwrap_or("").to_string();
                let n = ch["item"]["quantity"].as_i64().unwrap_or(0);
                if k.is_empty() || n == 0 {
                    continue;
                }
                if ch["delta"].as_str() == Some("removed") {
                    *s.backpack.entry(k.clone()).or_insert(0) -= n;
                    s.note(format!("-{n} {k}"));
                } else {
                    *s.backpack.entry(k.clone()).or_insert(0) += n;
                    s.note(format!("+{n} {k}"));
                }
            }
            s.backpack.retain(|_, n| *n > 0);
            match p["chits_delta"].as_i64().unwrap_or(0) {
                0 => {}
                c => s.note(format!("{c:+} chits")),
            }
        }
        // `name`/`progress`/`target`, not `hunt_key`/`goal`. Both wrong lookups failed
        // silently, so every hunt credit printed "hunt ?: 1/0" — the same shape as the
        // backpack bug: a field name nobody checked against the wire.
        "run.hunt_progress" => s.note(format!(
            "hunt {}: {}/{}{}",
            p["name"].as_str().unwrap_or("?"),
            p["progress"].as_i64().unwrap_or(0),
            p["target"].as_i64().unwrap_or(0),
            if p["complete"].as_bool().unwrap_or(false) { " COMPLETE" } else { "" }
        )),
        "run.channel_started" => {
            s.note(format!("channel: {}", p["kind"].as_str().unwrap_or("working")))
        }
        "run.channel_interrupted" => s.note("channel interrupted"),
        "chat.line" => {
            let line = format!(
                "<{}> {}",
                p["username"].as_str().unwrap_or("?"),
                p["text"].as_str().unwrap_or("")
            );
            // Kept apart from the event log as well as in it: everything else in that log
            // is the world reporting itself, and a person talking to you must not scroll
            // past inside a wall of damage numbers.
            s.chat.push(line.clone());
            s.note(line);
        }
        "run.world_end_felled" => {
            let omen = p["omen"].as_str().unwrap_or("").to_string();
            let ms = p["clear_ms"].as_i64().unwrap_or(0);
            let pieces = p["pieces"].as_i64().unwrap_or(0);
            s.note(format!(
                "*** THE END FIGHT IS DOWN *** {pieces} insured pieces, cleared in {:.1}s",
                ms as f64 / 1000.0
            ));
            s.note(format!("omen: {omen}"));
            s.world_end = Some(omen);
        }
        // The terminal message for ONE member. `run.ended` closes the whole instance and
        // does not arrive for a solo extraction at all, so watching only for that leaves a
        // successful dive looking like a channel that silently did nothing.
        "run.member_result" => {
            let r = p["result"].as_str().unwrap_or("?").to_string();
            let banked: Vec<String> = p["banked"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .map(|i| {
                            format!(
                                "{}x {}",
                                i["count"].as_i64().unwrap_or(1),
                                i["item_kind"].as_str().unwrap_or("?")
                            )
                        })
                        .collect()
                })
                .unwrap_or_default();
            s.note(format!(
                "run {r} at d{}{}",
                p["max_distance_reached"].as_i64().unwrap_or(0),
                if banked.is_empty() {
                    String::new()
                } else {
                    format!(" — banked {}", banked.join(", "))
                }
            ));
            s.run_active = false;
            s.run_result = Some(r);
        }
        "run.ended" => {
            s.run_active = false;
            let r = p["result"].as_str().unwrap_or("?").to_string();
            s.note(format!("run ended: {r}"));
            s.run_result = Some(r);
        }
        "battle.started" => {
            let mine: Vec<String> = p["your_combatant_ids"]
                .as_array()
                .map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect())
                .unwrap_or_default();
            let mut combatants = Vec::new();
            for a in p["allies"].as_array().unwrap_or(&empty) {
                let id = a["combatant_id"].as_str().unwrap_or_default();
                combatants.push(Comb::from_json(a, Side::Ally, mine.iter().any(|m| m == id)));
            }
            for e in p["enemies"].as_array().unwrap_or(&empty) {
                combatants.push(Comb::from_json(e, Side::Enemy, false));
            }
            let b = Battle {
                id: p["battle_id"].as_str().unwrap_or_default().to_string(),
                encounter_class: p["encounter_class"].as_str().unwrap_or("standard").to_string(),
                opening_hp: combatants
                    .iter()
                    .filter(|c| c.side == Side::Ally)
                    .map(|c| c.hp)
                    .sum(),
                combatants,
                ..Default::default()
            };
            s.note(format!(
                "battle: {} {} vs {} of yours",
                b.enemies().len(),
                b.encounter_class,
                b.mine().len()
            ));
            s.battle = Some(b);
        }
        "battle.turn_ready" => {
            let id = p["combatant_id"].as_str().unwrap_or_default().to_string();
            if let Some(b) = s.battle.as_mut() {
                if b.get(&id).is_some_and(|c| c.mine) && !b.ready.contains(&id) {
                    b.ready.push(id);
                    b.my_turns += 1;
                }
            }
        }
        "battle.gauge_update" => {
            if let Some(b) = s.battle.as_mut() {
                for e in p["combatants"].as_array().unwrap_or(&empty) {
                    let id = e["combatant_id"].as_str().unwrap_or_default();
                    if let Some(c) = b.combatants.iter_mut().find(|c| c.id == id) {
                        c.hp = e["hp"].as_i64().unwrap_or(c.hp as i64) as i32;
                        c.gauge = e["gauge"].as_f64().unwrap_or(c.gauge);
                        if let Some(st) = e["statuses"].as_array() {
                            c.statuses =
                                st.iter().filter_map(|x| x.as_str().map(String::from)).collect();
                        }
                    }
                }
                b.prune_ready();
            }
        }
        "battle.telegraph_started" => s.note(format!(
            "\"{}\" — something is charging",
            p["callout_text"].as_str().unwrap_or("")
        )),
        "battle.action_resolved" => {
            let actor = p["actor_id"].as_str().unwrap_or_default().to_string();
            let auto = p["auto"].as_bool().unwrap_or(false);
            if let Some(b) = s.battle.as_mut() {
                b.ready.retain(|r| *r != actor);
                for e in p["effects"].as_array().unwrap_or(&empty) {
                    let tid = e["target_id"].as_str().unwrap_or_default();
                    if let Some(c) = b.combatants.iter_mut().find(|c| c.id == tid) {
                        c.hp = e["hp_after"].as_i64().unwrap_or(c.hp as i64) as i32;
                    }
                }
                b.prune_ready();
            }
            let line = describe_action(s, p, &actor, auto);
            s.note(line);
        }
        "battle.ended" => {
            if let Some(mut b) = s.battle.take() {
                b.ended = Some(p["outcome"].as_str().unwrap_or("?").to_string());
                b.loot = p["loot"]
                    .as_array()
                    .map(|a| {
                        a.iter()
                            .map(|i| {
                                format!(
                                    "{}x {}",
                                    i["count"].as_i64().unwrap_or(1),
                                    i["item_kind"].as_str().unwrap_or("?")
                                )
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                s.note(format!(
                    "battle {} after {} of your turns{}",
                    b.ended.clone().unwrap_or_default(),
                    b.my_turns,
                    if b.loot.is_empty() {
                        String::new()
                    } else {
                        format!(" — loot: {}", b.loot.join(", "))
                    }
                ));
                s.last_battle = Some(b);
            }
        }
        _ => {}
    }
    false
}

/// One resolved action as a line a reader can follow: who did what to whom, for how much.
fn describe_action(s: &State, p: &Value, actor: &str, auto: bool) -> String {
    // Two creatures of the same kind share a name, so an unqualified label makes a pack
    // fight unreadable: "thornback_boar -251" could be either boar, and a log you cannot
    // attribute is a log you cannot measure from. Duplicated names get their position among
    // their own kind appended — the leader and its minion become #1 and #2.
    let name = |id: &str| -> String {
        let Some(b) = s.battle.as_ref() else {
            return id.chars().take(6).collect();
        };
        let Some(c) = b.get(id) else {
            return id.chars().take(6).collect();
        };
        let same: Vec<&Comb> = b.combatants.iter().filter(|o| o.name == c.name).collect();
        if same.len() < 2 {
            return c.name.clone();
        }
        let nth = same.iter().position(|o| o.id == c.id).map(|i| i + 1).unwrap_or(0);
        format!("{}#{nth}", c.name)
    };
    let what = match p["action"].as_str().unwrap_or("") {
        "skill" => p["skill_kind"].as_str().unwrap_or("skill").to_string(),
        other => other.to_string(),
    };
    let mut parts: Vec<String> = Vec::new();
    for e in p["effects"].as_array().unwrap_or(&Vec::new()) {
        let tid = e["target_id"].as_str().unwrap_or_default();
        let kind = e["kind"].as_str().unwrap_or("");
        let amt = e["amount"].as_i64().unwrap_or(0);
        let flag = e["modifier_flag"].as_str().map(|f| format!(" [{f}]")).unwrap_or_default();
        parts.push(match kind {
            "damage" => format!(
                "{} -{amt}{flag} ({} left)",
                name(tid),
                e["hp_after"].as_i64().unwrap_or(0)
            ),
            "heal" => format!("{} +{amt}", name(tid)),
            "status" => format!("{} {}", name(tid), e["status"].as_str().unwrap_or("affected")),
            k => format!("{} {k}", name(tid)),
        });
    }
    let callout = p["callout_text"]
        .as_str()
        .map(|c| format!("\"{c}\" "))
        .unwrap_or_default();
    format!(
        "{callout}{}{} {what}{}",
        name(actor),
        if auto { " (auto)" } else { "" },
        if parts.is_empty() {
            String::new()
        } else {
            format!(" → {}", parts.join(", "))
        }
    )
}
