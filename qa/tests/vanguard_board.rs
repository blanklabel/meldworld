//! Vanguard Board conformance (roadmap P1-1): a bot enters the maze, walks
//! outward, and its deepest distance shows up on the live seasonal board over
//! HTTP — proving the server-authoritative path (validated movement → run record
//! → persistence → `GET /v1/leaderboards/vanguard`) with no client-submitted
//! score anywhere in it (CANON §S anti-forgery).
//!
//! Requires Postgres: set `MELD_DATABASE_URL` (see qa/scripts/local_pg.sh).

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

#[tokio::test]
async fn walking_outward_posts_the_run_to_the_vanguard_board() {
    // Pin the world. This bot walks outward through a live map, and whether a roaming
    // creature intercepts it decides between a 4s depth check and a 75s timeout —
    // reproducibly per seed (1 and 2 pass, 3 fails, every time). The subject is that a
    // server-validated depth reaches the board, not that a walk goes uninterrupted.
    std::env::set_var("MELD_SEED", "1");
    let addr = start_server().await;
    let http = reqwest::Client::new();
    let base = format!("http://{addr}");
    let username = format!("vg_{}", &uuid::Uuid::new_v4().simple().to_string()[..10]);
    let body = json!({ "username": username, "password": "correct-horse-battery" });

    assert_eq!(
        http.post(format!("{base}/v1/auth/register")).json(&body).send().await.unwrap().status(),
        201
    );
    let login: Value = http
        .post(format!("{base}/v1/auth/login"))
        .json(&body)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let ticket = login["realtime_ticket"].as_str().unwrap().to_string();
    let token = login["session_token"].as_str().unwrap().to_string();
    let player_id = login["player"]["player_id"].as_str().unwrap().to_string();

    // Before the dive the caller has no placement — a 200 with a null entry, not
    // a 404 (the season exists; the placement doesn't).
    let me: Value = http
        .get(format!("{base}/v1/leaderboards/vanguard/me"))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(me["entry"].is_null(), "unranked caller should have a null entry: {me}");

    let (mut ws, _) = connect_async(format!("ws://{addr}/v1/realtime")).await.unwrap();
    let mut seq = 1u32;
    let mut input_seq = 0u32;
    // No prey-steering here, unlike the fight tests: this one wants DEPTH, and the
    // nearest creature is as often behind the bot as ahead of it (area 0 scatters
    // creatures to negative x), so hunting walks it back toward the hub.
    ws.send(Message::Text(
        json!({"type":"session.authenticate","seq":seq,"ts":0,"payload":{"ticket":ticket,"resume":null}}).to_string(),
    ))
    .await
    .unwrap();
    seq += 1;

    // Walk out-and-UP until the server has seen us at least this deep. Distance is
    // `hypot(x, y)`, so climbing counts the same as heading east — and it lifts the
    // bot off the centre line where the shallow creatures sit, which is what turns a
    // 4s depth check into a 75s timeout when it gets dragged into a fight on the way.
    // East also keeps it away from the western return border (`west_return`).
    const TARGET: f64 = 12.0;
    const DIAG: f64 = std::f64::consts::FRAC_1_SQRT_2;
    let mut started = false;
    let (mut my_x, mut my_y) = (0.0f64, 0.0f64);
    let mut deep_enough = false;
    let (mut my_c, mut mon_c, mut bid) = (String::new(), String::new(), String::new());

    let mut mover = tokio::time::interval(Duration::from_millis(80));
    mover.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(75);

    while !deep_enough {
        assert!(tokio::time::Instant::now() < deadline, "never reached d={TARGET}");
        tokio::select! {
            _ = mover.tick(), if started => {
                input_seq += 1;
                ws.send(Message::Text(json!({
                    "type":"movement.move_intent","seq":seq,"ts":0,
                    "payload":{"input_seq":input_seq,"move_dir":{"x":DIAG,"y":DIAG},"client_pos":{"x":0.0,"y":0.0}}
                }).to_string())).await.unwrap();
                seq += 1;
            }
            msg = ws.next() => {
                let Some(Ok(Message::Text(t))) = msg else { panic!("ws closed") };
                let v: Value = serde_json::from_str(&t).unwrap();
                match v["type"].as_str().unwrap_or("") {
                    "session.authenticated" => {
                        ws.send(Message::Text(json!({"type":"run.enter_maze","seq":seq,"ts":0,"payload":{}}).to_string())).await.unwrap();
                        seq += 1;
                    }
                    "run.started" => started = true,
                    "world.snapshot" => {
                        for e in v["payload"]["entities"].as_array().into_iter().flatten() {
                            if e["entity_id"].as_str() == Some(player_id.as_str()) {
                                my_x = e["position"]["x"].as_f64().unwrap();
                                my_y = e["position"]["y"].as_f64().unwrap();
                            }
                        }
                        if my_x.hypot(my_y) >= TARGET {
                            deep_enough = true;
                        }
                    }
                    // Anything we bump into on the way out gets punched until it drops.
                    "battle.started" => {
                        my_c = v["payload"]["your_combatant_id"].as_str().unwrap().to_string();
                        bid = v["payload"]["battle_id"].as_str().unwrap().to_string();
                        mon_c = v["payload"]["enemies"][0]["combatant_id"].as_str().unwrap().to_string();
                    }
                    "battle.turn_ready" if v["payload"]["combatant_id"].as_str() == Some(my_c.as_str()) => {
                        ws.send(Message::Text(json!({"type":"battle.submit_action","seq":seq,"ts":0,
                            "payload":{"battle_id":bid,"action_id":uuid::Uuid::new_v4().to_string(),"action":"attack","skill_kind":null,"item_id":null,"target_ids":[mon_c]}}).to_string())).await.unwrap();
                        seq += 1;
                    }
                    _ => {}
                }
            }
        }
    }

    // The board write is fire-and-forget off the game loop, so give the DB task a
    // moment to drain before reading the placement back.
    //
    // Read the CALLER'S OWN placement, not the top-N list: the QA Postgres is shared
    // across every agent and every past run, so a twelve-metre walk does not make the
    // leaderboard's first page and scanning `data` for our id finds nothing.
    let mut me = Value::Null;
    for _ in 0..25 {
        tokio::time::sleep(Duration::from_millis(200)).await;
        let res: Value = http
            .get(format!("{base}/v1/leaderboards/vanguard/me"))
            .bearer_auth(&token)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        if !res["entry"].is_null() {
            me = res;
            break;
        }
    }
    assert!(!me["entry"].is_null(), "the run never reached the Vanguard Board");
    let entry = me["entry"].clone();
    assert!(entry["rank"].as_i64().unwrap() >= 1, "ranks are 1-based: {entry}");
    assert_eq!(entry["username"].as_str().unwrap(), username);
    let posted = entry["max_distance"].as_i64().unwrap();
    assert!(
        posted >= TARGET as i64 - 1,
        "board distance {posted} should match the depth walked (~{TARGET})"
    );

    // …and the open season's board itself is live and unarchived.
    let board: Value = http
        .get(format!("{base}/v1/leaderboards/vanguard"))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(board["archived"], json!(false), "the open season is never archived");

    // A season that hasn't happened yet is a 404, not an empty board.
    let future_season = me["season"].as_i64().unwrap() + 1;
    let res = http
        .get(format!("{base}/v1/leaderboards/vanguard/{future_season}"))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 404);
}
