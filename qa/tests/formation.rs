//! Persistent party formation: a bot sets a hero to the back row (realtime), the
//! roster reflects it immediately, and — after a reconnect + fresh dive — the row
//! loads back from the account. Requires Postgres: set `MELD_DATABASE_URL`.

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
    balance.battle.party_size_per_player = 1; // one hero → slot 0 leads (a Hunter)
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

/// Connect with `ticket`, enter a dive, and return the first `run.party`'s slot-0
/// `back_row` flag. If `set_back` is Some, send `run.set_formation` after the first
/// roster and return the *updated* roster's flag instead.
async fn dive_and_read_back_row(addr: &str, ticket: &str, set_back: Option<bool>) -> bool {
    let (mut ws, _) = connect_async(format!("ws://{addr}/v1/realtime")).await.unwrap();
    let mut seq = 1u32;
    ws.send(Message::Text(
        json!({"type":"session.authenticate","seq":seq,"ts":0,"payload":{"ticket":ticket,"resume":null}}).to_string(),
    ))
    .await
    .unwrap();
    seq += 1;

    let mut sent = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        assert!(tokio::time::Instant::now() < deadline, "formation test timed out");
        let msg = ws.next().await;
        let Some(Ok(Message::Text(t))) = msg else { panic!("ws closed") };
        let v: Value = serde_json::from_str(&t).unwrap();
        match v["type"].as_str().unwrap_or("") {
            "session.authenticated" => {
                ws.send(Message::Text(json!({"type":"run.enter_maze","seq":seq,"ts":0,"payload":{}}).to_string())).await.unwrap();
                seq += 1;
            }
            "run.party" => {
                let back = v["payload"]["heroes"][0]["back_row"].as_bool().unwrap_or(false);
                match set_back {
                    Some(b) if !sent => {
                        sent = true;
                        ws.send(Message::Text(json!({"type":"run.set_formation","seq":seq,"ts":0,"payload":{"slot":0,"back_row":b}}).to_string())).await.unwrap();
                        seq += 1;
                    }
                    // The roster after the set (or, with no set, the first roster) is our answer.
                    _ => return back,
                }
            }
            _ => {}
        }
    }
}

async fn login(addr: &str, http: &reqwest::Client, username: &str) -> (String, String) {
    let base = format!("http://{addr}");
    let body = json!({ "username": username, "password": "correct-horse-battery" });
    let login: Value = http
        .post(format!("{base}/v1/auth/login"))
        .json(&body)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    (
        login["realtime_ticket"].as_str().unwrap().to_string(),
        login["session_token"].as_str().unwrap().to_string(),
    )
}

#[tokio::test]
async fn formation_persists_and_reloads() {
    let addr = start_server().await;
    let http = reqwest::Client::new();
    let base = format!("http://{addr}");
    let username = format!("fm_{}", &uuid::Uuid::new_v4().simple().to_string()[..10]);
    let body = json!({ "username": username, "password": "correct-horse-battery" });
    assert_eq!(
        http.post(format!("{base}/v1/auth/register")).json(&body).send().await.unwrap().status(),
        201
    );

    // Dive 1: a Hunter defaults to the FRONT row, then move it to the BACK — the
    // re-sent roster reflects the change immediately.
    let (ticket, _token) = login(&addr, &http, &username).await;
    let after_set = dive_and_read_back_row(&addr, &ticket, Some(true)).await;
    assert!(after_set, "hero moved to the back row on the live roster");

    // Wait for the async DB persist to land.
    tokio::time::sleep(Duration::from_millis(600)).await;

    // Dive 2 (fresh connection + ticket): the back row loads back from the account.
    let (ticket2, _t2) = login(&addr, &http, &username).await;
    let reloaded = dive_and_read_back_row(&addr, &ticket2, None).await;
    assert!(reloaded, "back-row formation persisted and reloaded on the next dive");
}
