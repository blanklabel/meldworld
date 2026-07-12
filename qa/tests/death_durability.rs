//! Death durability sink (CANON.md D6): a passive solo bot walks into the
//! monster and never attacks, so it dies. Its equipped blue-chest gear loses
//! 10% max durability, persisted to Postgres — verified via `GET /v1/vault/gear`.
//!
//! Requires Postgres: set `MELD_DATABASE_URL`.

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
    let balance = Arc::new(meld_balance::Balance::load_default().unwrap());
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

async fn gear(http: &reqwest::Client, base: &str, token: &str) -> Vec<Value> {
    let v: Value = http
        .get(format!("{base}/v1/vault/gear"))
        .bearer_auth(token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    v["data"].as_array().cloned().unwrap_or_default()
}

#[tokio::test]
async fn death_degrades_equipped_gear_durability() {
    let addr = start_server().await;
    let http = reqwest::Client::new();
    let base = format!("http://{addr}");
    let username = format!("dd_{}", &uuid::Uuid::new_v4().simple().to_string()[..10]);
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

    // Starting gear is at full durability.
    let g0 = gear(&http, &base, &token).await;
    assert_eq!(g0.len(), 1, "one starting weapon");
    let base_max = g0[0]["base_max_durability"].as_i64().unwrap();
    assert_eq!(g0[0]["max_durability"], json!(base_max), "starts at full");

    let (mut ws, _) = connect_async(format!("ws://{addr}/v1/realtime")).await.unwrap();
    let mut seq = 1u32;
    let mut input_seq = 0u32;
    ws.send(Message::Text(
        json!({"type":"session.authenticate","seq":seq,"ts":0,"payload":{"ticket":ticket,"resume":null}}).to_string(),
    ))
    .await
    .unwrap();
    seq += 1;

    let mut in_battle = false;
    let mut died = false;
    let mut mover = tokio::time::interval(Duration::from_millis(80));
    mover.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);

    while !died {
        assert!(tokio::time::Instant::now() < deadline, "did not die in time");
        tokio::select! {
            _ = mover.tick(), if !in_battle => {
                input_seq += 1;
                ws.send(Message::Text(json!({
                    "type":"movement.move_intent","seq":seq,"ts":0,
                    "payload":{"input_seq":input_seq,"move_dir":{"x":1.0,"y":0.0},"client_pos":{"x":0.0,"y":0.0}}
                }).to_string())).await.unwrap();
                seq += 1;
            }
            msg = ws.next() => {
                let Some(Ok(Message::Text(t))) = msg else { panic!("ws closed") };
                let v: Value = serde_json::from_str(&t).unwrap();
                match v["type"].as_str().unwrap_or("") {
                    "session.authenticated" => { ws.send(Message::Text(json!({"type":"run.enter_maze","seq":seq,"ts":0,"payload":{}}).to_string())).await.unwrap(); seq += 1; }
                    "battle.started" => in_battle = true, // and never attack — auto-lose
                    "battle.ended" => assert_eq!(v["payload"]["outcome"], json!("defeat"), "passive bot should lose"),
                    "run.member_result" => { assert_eq!(v["payload"]["result"], json!("died")); died = true; }
                    _ => {}
                }
            }
        }
    }

    // The durability sink is applied asynchronously after death; poll for it.
    let mut degraded = None;
    for _ in 0..40 {
        let g = gear(&http, &base, &token).await;
        let md = g[0]["max_durability"].as_i64().unwrap();
        if md < base_max {
            degraded = Some(md);
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_eq!(
        degraded,
        Some((base_max * 9) / 10),
        "death should reduce max durability by 10% (floor)"
    );
}
