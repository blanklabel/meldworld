//! Field-station conformance (roadmap `MS-1`): a smith who carries ore can raise a
//! forge out in the maze, and then work a piece at it — over the real wire, with the
//! real game loop deciding all of it. The claims this carries:
//!
//! 1. Raising one is **refused** without the ore, in words that say what it wants.
//! 2. Ore **carried out of town** (a Vault withdrawal riding into the run) pays for it,
//!    and the backpack is debited.
//! 3. The station appears in the world as `station:smith:<jobs>` for the whole instance.
//! 4. Work done at it lands on the requester's OWN gear — the reply names what changed —
//!    and the station's jobs count down.
//! 5. **Ownership never moves**: the piece is still in the same Vault afterwards.
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

/// Register + log in a fresh account, returning `(token, ticket, player_id)`.
async fn account(http: &reqwest::Client, base: &str, prefix: &str) -> (String, String, String) {
    let username = format!("{prefix}{}", &uuid::Uuid::new_v4().simple().to_string()[..10]);
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
    (
        login["session_token"].as_str().unwrap().to_string(),
        login["realtime_ticket"].as_str().unwrap().to_string(),
        login["player"]["player_id"].as_str().unwrap().to_string(),
    )
}

/// A forge is built from ore you are CARRYING, so a smith who walked out empty-handed
/// is refused — in words that say what it wants, not a silent no-op.
#[tokio::test]
async fn a_field_forge_needs_ore_in_hand() {
    let addr = start_server().await;
    let http = reqwest::Client::new();
    let base = format!("http://{addr}");
    let (_token, ticket, _pid) = account(&http, &base, "sr_").await;

    let (mut ws, _) = connect_async(format!("ws://{addr}/v1/realtime")).await.unwrap();
    let mut seq = 1u32;
    macro_rules! send {
        ($t:expr, $p:tt) => {{
            ws.send(Message::Text(
                json!({"type":$t,"seq":seq,"ts":0,"payload":$p}).to_string(),
            ))
            .await
            .unwrap();
            seq += 1;
        }};
    }
    ws.send(Message::Text(
        json!({"type":"session.authenticate","seq":seq,"ts":0,"payload":{"ticket":ticket,"resume":null}}).to_string(),
    ))
    .await
    .unwrap();
    seq += 1;

    let deadline = tokio::time::Instant::now() + Duration::from_secs(45);
    loop {
        assert!(tokio::time::Instant::now() < deadline, "no answer to a bare build");
        let Some(Ok(Message::Text(t))) = ws.next().await else { panic!("ws closed") };
        let v: Value = serde_json::from_str(&t).unwrap();
        match v["type"].as_str().unwrap_or("") {
            "session.authenticated" => send!("run.enter_maze", {"tutorial": true}),
            "run.started" => send!("run.build_station", {"kind": "smith"}),
            "session.error" => {
                let msg = v["payload"]["message"].as_str().unwrap_or_default();
                // Either gate is a legitimate answer for a fresh account (no Forging
                // level, no ore) — what matters is that it NAMES what is missing.
                assert!(
                    msg.contains("ore") || msg.contains("Forging"),
                    "a refusal has to say what a forge wants: {msg}"
                );
                assert!(!msg.is_empty());
                return;
            }
            "run.smith_result" => panic!("built a forge out of nothing: {v}"),
            _ => {}
        }
    }
}

/// The whole errand: carry ore out of town, raise a forge with it, and have a piece
/// worked at it — over the real wire, with the game loop deciding all of it.
#[tokio::test]
async fn a_smith_raises_a_forge_in_the_field_and_works_a_piece_at_it() {
    let addr = start_server().await;
    let http = reqwest::Client::new();
    let base = format!("http://{addr}");
    let (token, ticket, player_id) = account(&http, &base, "st_").await;
    let pid = uuid::Uuid::parse_str(&player_id).unwrap();

    // A smith with the skill, the stock and the chits — and ore withdrawn to carry out,
    // which is exactly how a field smith supplies themselves.
    let db_url = std::env::var("MELD_DATABASE_URL").unwrap();
    let balance = meld_balance::Balance::load_default().unwrap();
    let db = meld_db::Db::connect(&db_url, balance.auth.bcrypt_cost).await.unwrap();
    db.bank_extraction(pid, &[("dune_iron".into(), 20), ("dune_ingot".into(), 20)], 100_000)
        .await
        .unwrap();
    db.add_skill_xp(pid, "forging", 100_000).await.unwrap();
    assert_eq!(
        http.post(format!("{base}/v1/vault/materials/dune_iron/withdraw"))
            .bearer_auth(&token)
            .json(&json!({ "quantity": 10 }))
            .send()
            .await
            .unwrap()
            .status(),
        200
    );

    let gear: Value = http
        .get(format!("{base}/v1/vault/gear"))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let my_gear = gear["data"][0]["gear_id"].as_str().expect("starter gear").to_string();

    let (mut ws, _) = connect_async(format!("ws://{addr}/v1/realtime")).await.unwrap();
    let mut seq = 1u32;
    macro_rules! send {
        ($t:expr, $p:tt) => {{
            ws.send(Message::Text(
                json!({"type":$t,"seq":seq,"ts":0,"payload":$p}).to_string(),
            ))
            .await
            .unwrap();
            seq += 1;
        }};
    }
    ws.send(Message::Text(
        json!({"type":"session.authenticate","seq":seq,"ts":0,"payload":{"ticket":ticket,"resume":null}}).to_string(),
    ))
    .await
    .unwrap();
    seq += 1;

    let mut ore_removed = false;
    let mut asked = false;
    let mut station_id: Option<String> = None;
    let mut station_jobs_seen: Option<i64> = None;
    let mut result: Option<Value> = None;

    let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
    while result.is_none() {
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out (ore_removed={ore_removed}, station={station_id:?})"
        );
        let Some(Ok(Message::Text(t))) = ws.next().await else { panic!("ws closed") };
        let v: Value = serde_json::from_str(&t).unwrap();
        match v["type"].as_str().unwrap_or("") {
            "session.authenticated" => send!("run.enter_maze", {"tutorial": true}),
            "run.started" => send!("run.build_station", {"kind": "smith"}),
            "session.error" => panic!("refused: {v}"),
            "run.backpack_update" => {
                // The ore it was built from leaves the backpack, named and itemised.
                let changes = v["payload"]["changes"].as_array().cloned().unwrap_or_default();
                if changes.iter().any(|c| {
                    c["delta"] == json!("removed")
                        && c["cause"] == json!("station")
                        && c["item"]["item_kind"] == json!("dune_iron")
                }) {
                    ore_removed = true;
                }
            }
            "world.snapshot" if ore_removed && !asked => {
                let ents = v["payload"]["entities"].as_array().cloned().unwrap_or_default();
                if let Some(st) = ents.iter().find(|e| {
                    e["avatar_state"]
                        .as_str()
                        .is_some_and(|s| s.starts_with("station:smith:"))
                }) {
                    let tag = st["avatar_state"].as_str().unwrap().to_string();
                    station_jobs_seen = tag.rsplit(':').next().and_then(|n| n.parse::<i64>().ok());
                    let id = st["entity_id"].as_str().unwrap().to_string();
                    station_id = Some(id.clone());
                    asked = true;
                    send!("run.smith_request", {
                        "entity_id": id,
                        "gear_id": my_gear,
                        "service": "reroll",
                        "material": "dune_ingot"
                    });
                }
            }
            "run.smith_result" => result = Some(v["payload"].clone()),
            _ => {}
        }
    }

    assert!(ore_removed, "the station should have been paid for out of the backpack");
    let station_id = station_id.expect("the station is in the world");
    assert!(station_id.starts_with("station-smith"), "{station_id}");
    assert_eq!(
        station_jobs_seen,
        Some(balance.forge.station_uses as i64),
        "a fresh station advertises its full run of jobs"
    );

    let result = result.expect("the smith answered");
    assert_eq!(result["ok"], json!(true), "the work should have happened: {result}");
    assert_eq!(result["gear_id"], json!(my_gear), "it worked the piece we asked about");
    assert!(
        result["message"].as_str().unwrap_or_default().contains("re-drew"),
        "the reply should say what changed: {result}"
    );
    assert_eq!(
        result["uses_left"].as_i64(),
        Some(balance.forge.station_uses as i64 - 1),
        "a job done is a job spent: {result}"
    );

    // Ownership never moves: the piece is still in the same Vault, same id.
    let after: Value = http
        .get(format!("{base}/v1/vault/gear"))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(
        after["data"].as_array().unwrap().iter().any(|g| g["gear_id"] == json!(my_gear)),
        "the piece left its owner's Vault"
    );
}
