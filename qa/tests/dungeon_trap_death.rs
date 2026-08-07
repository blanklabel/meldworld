//! DG-3b (3/n) trap damage + death-in-dungeon: a bot descends, walks onto an armed
//! trap, and DIES — the run ends exactly like an overworld death (`run.member_result`
//! `result: "died"`, backpack forfeited). Trap damage is forced lethal here
//! (`dungeon_trap_damage` cranked) so one step kills, making death deterministic.
//! Requires Postgres.

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
    balance.worldgen.dungeon_spawn_chance = 1.0;
    balance.worldgen.dungeon_trap_damage = 99999; // one step is lethal
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

async fn send(ws: &mut Ws, t: &str, p: Value, seq: &mut u32) {
    let env = json!({ "type": t, "seq": *seq, "ts": 0u64, "payload": p });
    *seq += 1;
    ws.send(Message::Text(env.to_string())).await.unwrap();
}

#[tokio::test]
async fn stepping_on_a_dungeon_trap_kills_the_run() {
    std::env::set_var("MELD_BIOME", "forest");
    // Pin the world too, not just the biome. This bot walks into the dungeon and
    // marches east hoping to cross a trap, and whether that line of travel meets one
    // is decided by the rolled layout: measured across seeds 1-6 it passed on 1 and 4
    // and missed on the rest. The subject here is that a sprung trap kills the run,
    // not that a blind march finds a trap, so the layout is held still.
    std::env::set_var("MELD_SEED", "1");
    let addr = start_server().await;
    let (ticket, pid) =
        http_login(&addr, &format!("tbot_{}", &uuid::Uuid::new_v4().simple().to_string()[..8])).await;
    let (mut ws, _) = connect_async(format!("ws://{addr}/v1/realtime")).await.unwrap();

    let mut seq = 1u32;
    let mut input_seq = 0u32;
    send(&mut ws, "session.authenticate", json!({ "ticket": ticket, "resume": null }), &mut seq).await;
    // wait for authed
    loop {
        if let Some(Ok(Message::Text(t))) = ws.next().await {
            if serde_json::from_str::<Value>(&t).unwrap()["type"] == json!("session.authenticated") {
                break;
            }
        }
    }
    send(&mut ws, "run.enter_maze", json!({}), &mut seq).await;

    let mut my = (0.0f64, 0.0f64);
    let mut path: Vec<(f64, f64)> = Vec::new();
    let mut k = 1usize;
    let mut entrance: Option<(String, f64, f64)> = None;
    let mut entered = false;
    let mut ticker = tokio::time::interval(Duration::from_millis(100));
    let deadline = tokio::time::Instant::now() + Duration::from_secs(90);

    loop {
        assert!(tokio::time::Instant::now() < deadline, "never died to the trap (entered={entered})");
        tokio::select! {
            _ = ticker.tick() => {
                input_seq += 1;
                let (dx, dy) = if entered {
                    (1.0, 0.0) // once inside, march east across the trap corridor
                } else if let Some((id, ex, ey)) = &entrance {
                    let (dx, dy) = (ex - my.0, ey - my.1);
                    let d = (dx*dx + dy*dy).sqrt();
                    if d < 1.8 { send(&mut ws, "run.enter_dungeon", json!({"entity_id": id}), &mut seq).await; }
                    (dx / d.max(1e-6), dy / d.max(1e-6))
                } else if k < path.len() {
                    let (wx, wy) = path[k];
                    let (dx, dy) = (wx - my.0, wy - my.1);
                    let d = (dx*dx + dy*dy).sqrt();
                    if d < 1.5 { k += 1; }
                    (dx / d.max(1e-6), dy / d.max(1e-6))
                } else { (1.0, 0.0) };
                send(&mut ws, "movement.move_intent", json!({"input_seq": input_seq, "move_dir": {"x": dx, "y": dy}, "client_pos": {"x": 0.0, "y": 0.0}}), &mut seq).await;
            }
            msg = ws.next() => {
                let Some(Ok(Message::Text(txt))) = msg else { panic!("socket closed") };
                let v: Value = serde_json::from_str(&txt).unwrap();
                match v["type"].as_str().unwrap_or("") {
                    "run.member_result" => {
                        assert_eq!(v["payload"]["result"], json!("died"), "trap should kill the run");
                        return; // died as expected
                    }
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
                            if e["entity_id"].as_str() == Some(pid.as_str()) { my = (ex, ey); }
                            if state.starts_with("entrance:") { entrance = Some((e["entity_id"].as_str().unwrap().to_string(), ex, ey)); }
                            if state.starts_with("obstacle:dungeon_wall") { entered = true; }
                        }
                    }
                    _ => {}
                }
            }
        }
    }
}
