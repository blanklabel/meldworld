//! Persistent hero names: a bot renames a hero (HTTP + realtime), the name is
//! saved to the account, loaded into the next dive, shown in battle, and reflected
//! in the party roster. Requires Postgres: set `MELD_DATABASE_URL`.

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
    balance.battle.party_size_per_player = 1; // one hero → slot 0 leads, stable timing
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
async fn hero_names_persist_load_and_show_in_battle() {
    let addr = start_server().await;
    let http = reqwest::Client::new();
    let base = format!("http://{addr}");
    let username = format!("hn_{}", &uuid::Uuid::new_v4().simple().to_string()[..10]);
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

    // Fresh accounts seed default hero names.
    let h0: Value = http.get(format!("{base}/v1/heroes")).bearer_auth(&token).send().await.unwrap().json().await.unwrap();
    assert_eq!(h0["names"][0], json!("Hero 1"), "default hero name");

    // Rename slot 0 over HTTP → persisted.
    let r = http
        .put(format!("{base}/v1/heroes/0"))
        .bearer_auth(&token)
        .json(&json!({ "name": "Aria" }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let h1: Value = http.get(format!("{base}/v1/heroes")).bearer_auth(&token).send().await.unwrap().json().await.unwrap();
    assert_eq!(h1["names"][0], json!("Aria"), "rename persisted");

    // Enter a dive: the name loads from the account and rides the battle wire.
    let (mut ws, _) = connect_async(format!("ws://{addr}/v1/realtime")).await.unwrap();
    let mut seq = 1u32;
    let mut input_seq = 0u32;
    ws.send(Message::Text(
        json!({"type":"session.authenticate","seq":seq,"ts":0,"payload":{"ticket":ticket,"resume":null}}).to_string(),
    ))
    .await
    .unwrap();
    seq += 1;

    #[derive(PartialEq, Debug)]
    enum Phase { Init, ToMonster, Done }
    let mut phase = Phase::Init;
    let mut roster_name = String::new();
    let mut battle_name = String::new();
    let mut renamed = false;
    let mut mover = tokio::time::interval(Duration::from_millis(80));
    mover.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(75);

    while phase != Phase::Done {
        assert!(tokio::time::Instant::now() < deadline, "hero_names timed out (phase {phase:?})");
        tokio::select! {
            _ = mover.tick(), if phase == Phase::ToMonster => {
                input_seq += 1;
                ws.send(Message::Text(json!({"type":"movement.move_intent","seq":seq,"ts":0,
                    "payload":{"input_seq":input_seq,"move_dir":{"x":1.0,"y":0.0},"client_pos":{"x":0.0,"y":0.0}}}).to_string())).await.unwrap();
                seq += 1;
            }
            msg = ws.next() => {
                let Some(Ok(Message::Text(t))) = msg else { panic!("ws closed") };
                let v: Value = serde_json::from_str(&t).unwrap();
                match v["type"].as_str().unwrap_or("") {
                    "session.authenticated" => { ws.send(Message::Text(json!({"type":"run.enter_maze","seq":seq,"ts":0,"payload":{}}).to_string())).await.unwrap(); seq += 1; }
                    "run.started" => phase = Phase::ToMonster,
                    "run.party" => {
                        roster_name = v["payload"]["heroes"][0]["name"].as_str().unwrap_or("").to_string();
                        // After the DB-loaded name shows, do a realtime rename once.
                        if !renamed {
                            renamed = true;
                            assert_eq!(roster_name, "Aria", "roster shows the account name");
                            ws.send(Message::Text(json!({"type":"run.rename_hero","seq":seq,"ts":0,"payload":{"slot":0,"name":"Zed"}}).to_string())).await.unwrap();
                            seq += 1;
                        }
                    }
                    "battle.started" => {
                        let statuses = &v["payload"]["allies"][0]["statuses"];
                        battle_name = statuses.as_array().unwrap().iter()
                            .find_map(|s| s.as_str().and_then(|s| s.strip_prefix("name:")))
                            .unwrap_or("").to_string();
                        phase = Phase::Done;
                    }
                    _ => {}
                }
            }
        }
    }

    // The realtime rename took effect on the roster + persisted to the account.
    assert_eq!(roster_name, "Zed", "realtime rename updated the roster");
    // The battle wire carried the hero's name (the rename lands before the fight).
    assert_eq!(battle_name, "Zed", "battle combatant carried the hero name");
    // Wait for the async persist, then confirm the account saved the new name.
    let mut saved = String::new();
    for _ in 0..40 {
        let h: Value = http.get(format!("{base}/v1/heroes")).bearer_auth(&token).send().await.unwrap().json().await.unwrap();
        saved = h["names"][0].as_str().unwrap_or("").to_string();
        if saved == "Zed" { break; }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_eq!(saved, "Zed", "realtime rename persisted to the account");
}
