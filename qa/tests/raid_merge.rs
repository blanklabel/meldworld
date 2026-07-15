//! Raid merge (the Expandable Party): an "anchor" party engages the monster,
//! then a second party touches the same monster and merges into the ongoing
//! battle (`battle.party_joined`). Both parties fight and win together.
//!
//! The anchor defends until the joiner arrives (so it survives), then both
//! attack. Requires Postgres: set `MELD_DATABASE_URL`.

use std::sync::Arc;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio::net::TcpListener;
use tokio::sync::Notify;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;

async fn start_server() -> String {
    let db_url = std::env::var("MELD_DATABASE_URL")
        .expect("set MELD_DATABASE_URL (see qa/scripts/local_pg.sh)");
    let mut balance = meld_balance::Balance::load_default().unwrap();
    balance.battle.party_size_per_player = 1; // pin one hero so test timing stays stable
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
    tokio::spawn(async move { axum::serve(listener, built.router).await.unwrap() });
    format!("{addr}")
}

async fn login(addr: &str, username: &str) -> String {
    let http = reqwest::Client::new();
    let base = format!("http://{addr}");
    let body = json!({ "username": username, "password": "correct-horse-battery" });
    http.post(format!("{base}/v1/auth/register")).json(&body).send().await.unwrap();
    let v: Value = http
        .post(format!("{base}/v1/auth/login"))
        .json(&body)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    v["realtime_ticket"].as_str().unwrap().to_string()
}

struct Report {
    outcome: String,
    saw_party_joined: bool,
}

/// Drive one bot. `anchor` engages first and defends until the joiner merges;
/// otherwise the bot waits for `ready`, then enters and attacks.
async fn run_bot(addr: String, username: String, anchor: bool, ready: Arc<Notify>) -> Report {
    if !anchor {
        ready.notified().await; // wait until the anchor's battle is live
    }
    let ticket = login(&addr, &username).await;
    let (mut ws, _) = connect_async(format!("ws://{addr}/v1/realtime")).await.unwrap();
    let mut seq = 1u32;
    let mut input_seq = 0u32;
    ws.send(Message::Text(
        json!({"type":"session.authenticate","seq":seq,"ts":0,"payload":{"ticket":ticket,"resume":null}}).to_string(),
    ))
    .await
    .unwrap();
    seq += 1;

    let mut walking = false;
    let mut in_battle = false;
    let mut my_c = String::new();
    let mut mon_c = String::new();
    let mut bid = String::new();
    let mut saw_party_joined = false;
    let mut outcome = String::new();

    let mut mover = tokio::time::interval(Duration::from_millis(80));
    mover.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(75);

    while outcome.is_empty() {
        assert!(tokio::time::Instant::now() < deadline, "{username}: timed out");
        tokio::select! {
            _ = mover.tick(), if walking && !in_battle => {
                input_seq += 1;
                ws.send(Message::Text(json!({"type":"movement.move_intent","seq":seq,"ts":0,
                    "payload":{"input_seq":input_seq,"move_dir":{"x":1.0,"y":0.0},"client_pos":{"x":0.0,"y":0.0}}}).to_string())).await.unwrap();
                seq += 1;
            }
            msg = ws.next() => {
                let Some(Ok(Message::Text(t))) = msg else { panic!("{username}: ws closed") };
                let v: Value = serde_json::from_str(&t).unwrap();
                match v["type"].as_str().unwrap_or("") {
                    "session.authenticated" => { ws.send(Message::Text(json!({"type":"run.enter_maze","seq":seq,"ts":0,"payload":{}}).to_string())).await.unwrap(); seq += 1; }
                    "run.started" => walking = true,
                    "battle.started" => {
                        in_battle = true;
                        my_c = v["payload"]["your_combatant_id"].as_str().unwrap().to_string();
                        bid = v["payload"]["battle_id"].as_str().unwrap().to_string();
                        mon_c = v["payload"]["enemies"][0]["combatant_id"].as_str().unwrap().to_string();
                        if anchor { ready.notify_one(); } // tell the joiner the battle is live
                    }
                    "battle.party_joined" => saw_party_joined = true,
                    "battle.turn_ready" if v["payload"]["combatant_id"].as_str() == Some(my_c.as_str()) => {
                        // Anchor defends until the joiner merges, then everyone attacks.
                        let action = if anchor && !saw_party_joined { "defend" } else { "attack" };
                        let targets = if action == "attack" { json!([mon_c]) } else { json!(null) };
                        ws.send(Message::Text(json!({"type":"battle.submit_action","seq":seq,"ts":0,
                            "payload":{"battle_id":bid,"action_id":uuid::Uuid::new_v4().to_string(),"action":action,"skill_kind":null,"item_id":null,"target_ids":targets}}).to_string())).await.unwrap();
                        seq += 1;
                    }
                    "battle.ended" => outcome = v["payload"]["outcome"].as_str().unwrap_or("").to_string(),
                    _ => {}
                }
            }
        }
    }
    Report { outcome, saw_party_joined }
}

#[tokio::test]
async fn second_party_merges_into_the_battle() {
    let addr = start_server().await;
    let ready = Arc::new(Notify::new());
    let suffix = uuid::Uuid::new_v4().simple().to_string();

    let a = tokio::spawn(run_bot(addr.clone(), format!("anchor_{}", &suffix[..8]), true, ready.clone()));
    let b = tokio::spawn(run_bot(addr.clone(), format!("joiner_{}", &suffix[8..16]), false, ready.clone()));

    let ra = a.await.unwrap();
    let rb = b.await.unwrap();

    assert_eq!(ra.outcome, "victory", "anchor should win");
    assert_eq!(rb.outcome, "victory", "joiner should win");
    assert!(ra.saw_party_joined, "anchor should have seen the second party merge in");
}
