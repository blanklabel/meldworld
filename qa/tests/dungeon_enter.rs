//! DG-3b end-to-end: a headless bot dives, finds a hand-designed dungeon ENTRANCE
//! streamed into the overworld, sends `run.enter_dungeon`, and confirms it is now
//! inside the dungeon (its snapshot carries the dungeon floor — walls + the exit
//! portal) rather than the overworld. Drives the real server over the real wire.
//!
//! Deterministic + fast: every section is pinned to Forest (`MELD_BIOME`) and the
//! dungeon spawn chance is forced to 1.0, so the first streamed section hosts an
//! entrance the bot can walk to.
//!
//! Requires Postgres (`MELD_DATABASE_URL`; `qa/scripts/local_pg.sh` provides it).

use std::sync::Arc;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio::net::TcpListener;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;

async fn start_server() -> String {
    let db_url = std::env::var("MELD_DATABASE_URL")
        .expect("set MELD_DATABASE_URL (see qa/scripts/local_pg.sh)");
    let mut balance = meld_balance::Balance::load_default().unwrap();
    balance.battle.party_size_per_player = 1;
    // Force an entrance on every streamed section so the test is fast + deterministic.
    balance.worldgen.dungeon_spawn_chance = 1.0;
    // This test is about what happens INSIDE a dungeon, not where doors are allowed
    // to be, so it opts out of the hub exclusion that keeps them off the doorstep.
    balance.worldgen.dungeon_min_distance = 0.0;
    let balance = Arc::new(balance);
    let config = meld_server::Config {
        bind_addr: "127.0.0.1:0".to_string(),
        database_url: db_url,
        balance,
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
}

impl Bot {
    async fn connect(addr: &str, ticket: &str) -> Self {
        let (ws, _) = connect_async(format!("ws://{addr}/v1/realtime")).await.unwrap();
        let mut bot = Bot { ws, seq: 1, input_seq: 0 };
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
                .await
                .expect("timed out")
                .expect("closed")
                .expect("ws error");
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
        self.send(
            "movement.move_intent",
            json!({ "input_seq": self.input_seq, "move_dir": { "x": x, "y": y }, "client_pos": { "x": 0.0, "y": 0.0 } }),
        )
        .await;
    }
}

#[tokio::test]
async fn a_bot_enters_a_hand_designed_dungeon() {
    // Pin every section to Forest (guaranteed dungeon pool) and force the tutorial
    // off. Process-global, but this is the test binary's only world.
    std::env::set_var("MELD_BIOME", "forest");

    let addr = start_server().await;
    let (ticket, pid) = http_login(&addr, &format!("dbot_{}", &uuid::Uuid::new_v4().simple().to_string()[..8])).await;
    let mut bot = Bot::connect(&addr, &ticket).await;
    bot.send("run.enter_maze", json!({})).await;

    let mut my = (0.0f64, 0.0f64);
    let mut path: Vec<(f64, f64)> = Vec::new();
    let mut k = 1usize; // next path waypoint (skip 0 = hub)
    // Every entrance we've seen, by id → position.
    let mut entrances: std::collections::HashMap<String, (f64, f64)> = std::collections::HashMap::new();
    let mut entered = false;
    let mut min_dist = f64::INFINITY;

    let mut ticker = tokio::time::interval(Duration::from_millis(100));
    let deadline = tokio::time::Instant::now() + Duration::from_secs(90);

    loop {
        assert!(
            tokio::time::Instant::now() < deadline,
            "never entered the dungeon (entrances_seen={}, min_dist={min_dist:.2}, my={my:?}, waypoint={k}/{})",
            entrances.len(), path.len()
        );
        tokio::select! {
            _ = ticker.tick() => {
                if entered {
                    bot.move_dir(0.0, 1.0).await; // keep the dungeon snapshot flowing
                    continue;
                }
                // Enter the moment we're beside any entrance we've seen (walking the
                // path takes us right through them — they sit on the clear path).
                for (id, (ex, ey)) in &entrances {
                    let d = ((ex - my.0).powi(2) + (ey - my.1).powi(2)).sqrt();
                    min_dist = min_dist.min(d);
                    if d < 1.8 {
                        bot.send("run.enter_dungeon", json!({ "entity_id": id })).await;
                    }
                }
                // Follow the guaranteed-clear path outward (obstacle-free by
                // construction), advancing to the next waypoint as we reach each.
                if k < path.len() {
                    let (wx, wy) = path[k];
                    let (dx, dy) = (wx - my.0, wy - my.1);
                    let d = (dx * dx + dy * dy).sqrt();
                    if d < 1.5 {
                        k += 1;
                    } else {
                        bot.move_dir(dx / d.max(1e-6), dy / d.max(1e-6)).await;
                    }
                } else {
                    bot.move_dir(1.0, 0.0).await; // ran out of path — keep pressing outward
                }
            }
            msg = bot.ws.next() => {
                let Some(Ok(Message::Text(txt))) = msg else { panic!("socket closed") };
                let v: Value = serde_json::from_str(&txt).unwrap();
                match v["type"].as_str().unwrap_or("") {
                    "run.started" => {
                        if let Some(pts) = v["payload"]["path"].as_array() {
                            path = pts.iter().map(|p| (p["x"].as_f64().unwrap_or(0.0), p["y"].as_f64().unwrap_or(0.0))).collect();
                        }
                    }
                    "world.snapshot" => {
                        let empty = vec![];
                        let mut ways_out = 0usize;
                        for e in v["payload"]["entities"].as_array().unwrap_or(&empty) {
                            let state = e["avatar_state"].as_str().unwrap_or("");
                            let (ex, ey) = (
                                e["position"]["x"].as_f64().unwrap_or(0.0),
                                e["position"]["y"].as_f64().unwrap_or(0.0),
                            );
                            if e["entity_id"].as_str() == Some(pid.as_str()) {
                                my = (ex, ey);
                            }
                            if let Some(name) = state.strip_prefix("entrance:") {
                                let _ = name;
                                entrances.insert(e["entity_id"].as_str().unwrap().to_string(), (ex, ey));
                            }
                            // Proof we're inside: the dungeon floor's walls only reach a
                            // player whose snapshot is scoped to the dungeon space.
                            if state.starts_with("obstacle:dungeon_wall") {
                                entered = true;
                            }
                            if state == "portal" {
                                ways_out += 1;
                            }
                        }
                        if entered {
                            // A dungeon refuses a Town Portal, so the marked way out is
                            // the ONLY way out that isn't dying. `at_exit` has always
                            // accepted the door you came in by as one — but the snapshot
                            // drew only the authored far exit, so on a floor whose exit
                            // you hadn't found yet there was nothing on screen to walk
                            // back to. Floor 0 always holds the entrance, so a floor-0
                            // snapshot with no portal at all is that bug.
                            assert!(
                                ways_out > 0,
                                "a dungeon floor must show at least one way out"
                            );
                            return;
                        }
                    }
                    _ => {}
                }
            }
        }
    }
}
