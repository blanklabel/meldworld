//! A new account dives with ONE hero, not four — on a NORMAL dive.
//!
//! `clamp_party_to_unlocks` cut an unearned party down correctly and `form_run` then
//! padded it straight back up to `party_size_per_player`, so a player who had unlocked
//! only the Explorer was handed four copies of it. The cap is a ceiling, not a grant —
//! and this is the end-to-end check, because the two halves were individually right.
//!
//! Deliberately `"tutorial": false`: the guided `[T]` dive is a later, INTENTIONAL
//! exception to this exact clamp (it lets a brand-new account try up to 4 chosen
//! classes for that one dive — see `clamp_tutorial_party`/`qa/tests/
//! tutorial_party_and_portal.rs`), so asserting the clamp under `tutorial: true` would
//! now fail this test for the very reason it exists to be lenient.
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
    let balance = meld_balance::Balance::load_default().unwrap();
    assert!(
        balance.battle.party_size_per_player >= 2,
        "this test is meaningless unless the CAP is bigger than one hero"
    );
    let config = meld_server::Config {
        bind_addr: "127.0.0.1:0".to_string(),
        database_url: db_url,
        balance: Arc::new(balance),
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
async fn a_fresh_account_fields_one_hero_however_many_it_asks_for() {
    let addr = start_server().await;
    let http = reqwest::Client::new();
    let base = format!("http://{addr}");
    let username = format!("ps_{}", &uuid::Uuid::new_v4().simple().to_string()[..10]);
    let body = json!({ "username": username, "password": "correct-horse-battery" });
    http.post(format!("{base}/v1/auth/register")).json(&body).send().await.unwrap();
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

    let (mut ws, _) = connect_async(format!("ws://{addr}/v1/realtime")).await.unwrap();
    let mut seq = 1u32;
    ws.send(Message::Text(
        json!({"type":"session.authenticate","seq":seq,"ts":0,"payload":{"ticket":ticket,"resume":null}})
            .to_string(),
    ))
    .await
    .unwrap();
    seq += 1;

    let deadline = tokio::time::Instant::now() + Duration::from_secs(45);
    let mut heroes: Option<usize> = None;
    let mut slots = -1i64;
    while heroes.is_none() {
        assert!(tokio::time::Instant::now() < deadline, "no roster arrived");
        let Some(Ok(Message::Text(t))) = ws.next().await else { panic!("ws closed") };
        let v: Value = serde_json::from_str(&t).unwrap();
        match v["type"].as_str().unwrap_or("") {
            "session.authenticated" => {
                // Ask for a FULL four-hero party. A fresh account has not earned it.
                ws.send(Message::Text(
                    json!({"type":"run.enter_maze","seq":seq,"ts":0,"payload":{
                        "party":["explorer","explorer","explorer","explorer"],
                        "tutorial":false
                    }})
                    .to_string(),
                ))
                .await
                .unwrap();
                seq += 1;
            }
            "run.unlocked" => {
                slots = v["payload"]["party_slots"].as_i64().unwrap_or(-1);
            }
            "run.party" => {
                heroes = v["payload"]["heroes"].as_array().map(|a| a.len());
            }
            _ => {}
        }
    }
    assert_eq!(slots, 1, "a fresh account has exactly one party slot");
    assert_eq!(
        heroes,
        Some(1),
        "asked for four heroes on a one-slot account and got {heroes:?} \u{2014} the \
         balance cap is a ceiling, not an entitlement"
    );
}
