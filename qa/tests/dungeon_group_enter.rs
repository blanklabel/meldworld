//! DG-3b (3/n) group entry: two bots gather at a dungeon entrance; ONE sends
//! `run.enter_dungeon`, and BOTH end up inside the same subinstance (teammates
//! within `[ai] join_radius` descend together — a co-op group). Proven end-to-end:
//! both bots' snapshots become the dungeon floor (walls) after the single enter.
//!
//! Deterministic: every section is Forest (`MELD_BIOME`) with dungeon spawn chance
//! 1.0, so an entrance sits on the clear path both bots walk. Requires Postgres.

use std::sync::Arc;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio::net::TcpListener;
use tokio::sync::Barrier;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;

async fn start_server() -> String {
    let db_url = std::env::var("MELD_DATABASE_URL")
        .expect("set MELD_DATABASE_URL (see qa/scripts/local_pg.sh)");
    let mut balance = meld_balance::Balance::load_default().unwrap();
    balance.battle.party_size_per_player = 1;
    balance.worldgen.dungeon_spawn_chance = 1.0;
    let balance = Arc::new(balance);
    let config = meld_server::Config {
        bind_addr: "127.0.0.1:0".to_string(),
        database_url: db_url,
        balance,
        client_dist: None,
    };
    let built = meld_server::build(&config).await.expect("server builds");
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, built.router).await.unwrap();
    });
    format!("{addr}")
}

async fn http_login(addr: &str, username: &str) -> (String, String) {
    let client = reqwest::Client::new();
    let base = format!("http://{addr}");
    let body = json!({ "username": username, "password": "correct-horse-battery" });
    let reg = client.post(format!("{base}/v1/auth/register")).json(&body).send().await.unwrap();
    assert_eq!(reg.status(), 201);
    let login = client.post(format!("{base}/v1/auth/login")).json(&body).send().await.unwrap();
    assert_eq!(login.status(), 200);
    let v: Value = login.json().await.unwrap();
    (
        v["realtime_ticket"].as_str().unwrap().to_string(),
        v["player"]["player_id"].as_str().unwrap().to_string(),
    )
}

type Ws =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

struct Bot {
    ws: Ws,
    seq: u32,
    input_seq: u32,
    pid: String,
}

impl Bot {
    async fn connect(addr: &str, ticket: &str, pid: &str) -> Self {
        let (ws, _) = connect_async(format!("ws://{addr}/v1/realtime")).await.unwrap();
        let mut bot = Bot { ws, seq: 1, input_seq: 0, pid: pid.to_string() };
        bot.send("session.authenticate", json!({ "ticket": ticket, "resume": null })).await;
        bot.recv_type("session.authenticated").await;
        bot
    }
    async fn send(&mut self, msg_type: &str, payload: Value) {
        let env = json!({ "type": msg_type, "seq": self.seq, "ts": 0u64, "payload": payload });
        self.seq += 1;
        self.ws.send(Message::Text(env.to_string())).await.unwrap();
    }
    async fn recv_type(&mut self, msg_type: &str) -> Value {
        loop {
            let msg = tokio::time::timeout(Duration::from_secs(10), self.ws.next())
                .await.expect("timeout").expect("closed").expect("ws error");
            if let Message::Text(t) = msg {
                let v: Value = serde_json::from_str(&t).unwrap();
                if v["type"] == json!(msg_type) {
                    return v;
                }
            }
        }
    }
    async fn move_dir(&mut self, x: f64, y: f64) {
        self.input_seq += 1;
        self.send("movement.move_intent", json!({ "input_seq": self.input_seq, "move_dir": { "x": x, "y": y }, "client_pos": { "x": 0.0, "y": 0.0 } })).await;
    }

    /// Path-follow toward the streamed entrance. The `leader` sends `run.enter_dungeon`
    /// once it has lingered by the entrance (so the follower catches up); the follower
    /// sends nothing. Returns `true` once THIS bot's snapshot becomes the dungeon floor.
    async fn play(mut self, leader: bool, start: Arc<Barrier>) -> bool {
        start.wait().await;
        if leader {
            tokio::time::sleep(Duration::from_millis(400)).await;
            self.send("run.enter_maze", json!({})).await;
        }
        let mut my = (0.0f64, 0.0f64);
        let mut path: Vec<(f64, f64)> = Vec::new();
        let mut k = 1usize;
        let mut entrance: Option<(String, f64, f64)> = None;
        let mut near_ticks = 0u32;
        let mut ticker = tokio::time::interval(Duration::from_millis(100));
        let deadline = tokio::time::Instant::now() + Duration::from_secs(90);
        loop {
            if tokio::time::Instant::now() >= deadline {
                return false;
            }
            tokio::select! {
                _ = ticker.tick() => {
                    // Steer to the entrance if we've seen one, else follow the path out.
                    let (tx, ty) = match &entrance {
                        Some((_, ex, ey)) => (*ex, *ey),
                        None if k < path.len() => path[k],
                        None => (my.0 + 5.0, my.1),
                    };
                    let (dx, dy) = (tx - my.0, ty - my.1);
                    let d = (dx * dx + dy * dy).sqrt();
                    if entrance.is_none() && k < path.len() && d < 1.5 { k += 1; }
                    if d > 0.05 { self.move_dir(dx / d.max(1e-6), dy / d.max(1e-6)).await; }
                    if let Some((id, _, _)) = &entrance {
                        if d < 3.0 { near_ticks += 1; } else { near_ticks = 0; }
                        // Leader lingers ~1.5s so the follower is within join_radius, then descends.
                        if leader && near_ticks == 15 {
                            self.send("run.enter_dungeon", json!({ "entity_id": id })).await;
                        }
                    }
                }
                msg = self.ws.next() => {
                    let Some(Ok(Message::Text(txt))) = msg else { return false };
                    let v: Value = serde_json::from_str(&txt).unwrap();
                    match v["type"].as_str().unwrap_or("") {
                        "run.started" => {
                            if let Some(pts) = v["payload"]["path"].as_array() {
                                path = pts.iter().map(|p| (p["x"].as_f64().unwrap_or(0.0), p["y"].as_f64().unwrap_or(0.0))).collect();
                            }
                        }
                        "world.snapshot" => {
                            let empty = vec![];
                            for e in v["payload"]["entities"].as_array().unwrap_or(&empty) {
                                let state = e["avatar_state"].as_str().unwrap_or("");
                                let (ex, ey) = (e["position"]["x"].as_f64().unwrap_or(0.0), e["position"]["y"].as_f64().unwrap_or(0.0));
                                if e["entity_id"].as_str() == Some(self.pid.as_str()) { my = (ex, ey); }
                                if state.starts_with("entrance:") { entrance = Some((e["entity_id"].as_str().unwrap().to_string(), ex, ey)); }
                                if state.starts_with("obstacle:dungeon_wall") { return true; }
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
    }
}

#[tokio::test]
async fn two_teammates_enter_a_dungeon_together() {
    std::env::set_var("MELD_BIOME", "forest");
    let addr = start_server().await;
    let start = Arc::new(Barrier::new(2));
    let mut handles = Vec::new();
    for i in 0..2 {
        let addr = addr.clone();
        let start = start.clone();
        handles.push(tokio::spawn(async move {
            let user = format!("gbot{i}_{}", &uuid::Uuid::new_v4().simple().to_string()[..8]);
            let (ticket, pid) = http_login(&addr, &user).await;
            let bot = Bot::connect(&addr, &ticket, &pid).await;
            bot.play(i == 0, start).await
        }));
    }
    let mut inside = 0;
    for h in handles {
        if h.await.unwrap() {
            inside += 1;
        }
    }
    assert_eq!(inside, 2, "both teammates should be pulled into the dungeon by one enter");
}
