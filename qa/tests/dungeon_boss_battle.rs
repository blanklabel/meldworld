//! DG-3b (3/n) dungeon combat: a bot descends, walks to the boss room (doors open
//! as it reaches levers/keys, stairs auto-transition), and a BOSS FIGHT starts — it
//! wins, ending the run's dungeon boss battle in victory. The boss is weakened here
//! so a lone hero wins deterministically. Requires Postgres.

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
    // This test is about what happens INSIDE a dungeon, not where doors are allowed
    // to be, so it opts out of the hub exclusion that keeps them off the doorstep.
    balance.worldgen.dungeon_min_distance = 0.0;
    balance.worldgen.dungeon_trap_damage = 1; // don't let the corridor trap matter
    // Pin WHICH dungeon loads. The point of this test is that a bot can fight and beat
    // a dungeon boss — not that it can solve an authored maze.
    //
    // `verdant_barrow` is a ONE-CELL-WIDE corridor on both floors, so walking east is
    // a complete solution: floor 0 runs entrance -> trap -> lever (opens D1) -> key ->
    // stairs, and floor 1 runs up-stair -> keyed door -> three plates -> gate -> boss.
    // Note it clears G1, a `all[P1,P2,P3]` CO-OP gate, only because the runtime latches
    // plates permanently and ignores their `momentary` flag; if that is ever made
    // faithful, a lone bot can no longer open this gate and this test needs a dungeon
    // without one.
    //
    // It was pinned to `world_of_ruin` on the grounds that a single floor meant no
    // stairs in the way. That is the most complex dungeon in the game — nine mandatory
    // bosses, six dragons each holding a switch, all six levers needed to bridge to
    // Kefka's Tower — so no amount of pathfinding could have solved it, and it is a
    // DESERT dungeon pinned under `MELD_BIOME=forest` besides.
    std::env::set_var("MELD_DUNGEON", "verdant_barrow");
    // A pushover boss so a single hero wins fast + deterministically.
    balance.encounters.gatekeeper_hp_mult = 0.02;
    balance.encounters.gatekeeper_atk_mult = 0.0;
    let balance = std::sync::Arc::new(balance);
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
async fn a_bot_fights_and_kills_the_dungeon_boss() {
    std::env::set_var("MELD_BIOME", "forest");
    let addr = start_server().await;
    let (ticket, pid) =
        http_login(&addr, &format!("bbot_{}", &uuid::Uuid::new_v4().simple().to_string()[..8])).await;
    let (mut ws, _) = connect_async(format!("ws://{addr}/v1/realtime")).await.unwrap();

    let mut seq = 1u32;
    let mut input_seq = 0u32;
    send(&mut ws, "session.authenticate", json!({ "ticket": ticket, "resume": null }), &mut seq).await;
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
    let mut boss_at: Option<(f64, f64)> = None;
    let mut stair_at: Option<(f64, f64)> = None;
    let mut in_battle = false;
    let mut my_cid: Option<String> = None;
    let mut boss_cid: Option<String> = None;
    let mut battle_id = String::new();

    let mut ticker = tokio::time::interval(Duration::from_millis(100));
    let deadline = tokio::time::Instant::now() + Duration::from_secs(120);

    loop {
        assert!(tokio::time::Instant::now() < deadline, "never beat the boss (entered={entered}, in_battle={in_battle})");
        tokio::select! {
            _ = ticker.tick(), if !in_battle => {
                input_seq += 1;
                let (dx, dy) = if entered {
                    // The boss if we can see it; otherwise the stairs down, which are
                    // how you reach the floor it is on. Marching east and hoping only
                    // worked when the layout happened to cooperate — which dungeon
                    // rolled decided whether this test passed.
                    // The boss if visible; otherwise the stairs down, which are how you
                    // reach the floor it is on. Straight-line — a BFS over the walls
                    // in the snapshot performed WORSE in practice (0/3 vs 1/3), so the
                    // harness keeps the simple thing until someone can show better.
                    match boss_at.or(stair_at) {
                        Some((tx, ty)) => {
                            let (dx, dy) = (tx - my.0, ty - my.1);
                            let d = (dx * dx + dy * dy).sqrt();
                            (dx / d.max(1e-6), dy / d.max(1e-6))
                        }
                        None => (1.0, 0.0),
                    }
                } else if let Some((id, ex, ey)) = &entrance {
                    let (dx, dy) = (ex - my.0, ey - my.1);
                    let d = (dx*dx+dy*dy).sqrt();
                    if d < 1.8 { send(&mut ws, "run.enter_dungeon", json!({"entity_id": id}), &mut seq).await; }
                    (dx/d.max(1e-6), dy/d.max(1e-6))
                } else if k < path.len() {
                    let (wx, wy) = path[k];
                    let (dx, dy) = (wx - my.0, wy - my.1);
                    let d = (dx*dx+dy*dy).sqrt();
                    if d < 1.5 { k += 1; }
                    (dx/d.max(1e-6), dy/d.max(1e-6))
                } else { (1.0, 0.0) };
                send(&mut ws, "movement.move_intent", json!({"input_seq": input_seq, "move_dir": {"x": dx, "y": dy}, "client_pos": {"x": 0.0, "y": 0.0}}), &mut seq).await;
            }
            msg = ws.next() => {
                let Some(Ok(Message::Text(txt))) = msg else { panic!("socket closed") };
                let v: Value = serde_json::from_str(&txt).unwrap();
                match v["type"].as_str().unwrap_or("") {
                    "run.started" => {
                        if let Some(pts) = v["payload"]["path"].as_array() {
                            path = pts.iter().map(|p| (p["x"].as_f64().unwrap_or(0.0), p["y"].as_f64().unwrap_or(0.0))).collect();
                        }
                    }
                    "world.snapshot" => {
                        // Each snapshot re-describes the floor we are on, so forget
                        // last floor's landmarks before reading this one.
                        boss_at = None;
                        stair_at = None;
                        let empty = vec![];
                        for e in v["payload"]["entities"].as_array().unwrap_or(&empty) {
                            let state = e["avatar_state"].as_str().unwrap_or("");
                            let (ex, ey) = (e["position"]["x"].as_f64().unwrap_or(0.0), e["position"]["y"].as_f64().unwrap_or(0.0));
                            if e["entity_id"].as_str() == Some(pid.as_str()) { my = (ex, ey); }
                            if state.starts_with("entrance:") { entrance = Some((e["entity_id"].as_str().unwrap().to_string(), ex, ey)); }
                            if state.starts_with("obstacle:dungeon_wall") { entered = true; }
                            // The boss is a `mob:` prop on the floor. Walking east and
                            // hoping only works when it happens to be due east with
                            // nothing in the way; steer at it once it is on screen.
                            if state.starts_with("mob:") { boss_at = Some((ex, ey)); }
                            if state == "stair" { stair_at = Some((ex, ey)); }
                        }
                    }
                    "battle.started" => {
                        in_battle = true;
                        my_cid = v["payload"]["your_combatant_id"].as_str().map(String::from);
                        boss_cid = v["payload"]["enemies"][0]["combatant_id"].as_str().map(String::from);
                        battle_id = v["payload"]["battle_id"].as_str().unwrap_or("").to_string();
                    }
                    "battle.turn_ready" if v["payload"]["combatant_id"].as_str().map(String::from) == my_cid => {
                        send(&mut ws, "battle.submit_action", json!({
                            "battle_id": battle_id,
                            "action_id": uuid::Uuid::now_v7().to_string(),
                            "action": "attack",
                            "skill_kind": null,
                            "item_id": null,
                            "target_ids": [boss_cid.clone().unwrap()]
                        }), &mut seq).await;
                    }
                    "battle.ended" => {
                        assert_eq!(v["payload"]["outcome"], json!("victory"), "should beat the dungeon boss");
                        return;
                    }
                    _ => {}
                }
            }
        }
    }
}
