//! Concurrent battles: two independent parties fight **separate** encounters at
//! the same time. One party engages the tutorial creature and stalls (defends);
//! while that fight is still live, a second party walks past it and touches a
//! *different* creature, getting its OWN `battle.started` with a different
//! `battle_id`. Under the old single-battle model the second touch was a no-op
//! (the instance was globally "in battle"), so this would hang. Both then win.
//!
//! Requires Postgres: set `MELD_DATABASE_URL` (see qa/scripts/local_pg.sh).

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
    balance.battle.party_size_per_player = 1; // one hero → stable timing
    // Put every creature on the centre line so a straight east walk reliably
    // meets the next one (area ≥ 1 normally scatters across ±y).
    balance.worldgen.creature_lateral_spread = 0.0;
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
    battle_id: String,
}

/// Drive one bot. The `anchor` engages the tutorial creature first and defends
/// until the `second` bot's separate battle is live (proving concurrency), then
/// attacks. The `second` bot waits for the anchor's fight to start, walks past it
/// to a different creature, and fights its own battle.
async fn run_bot(
    addr: String,
    username: String,
    anchor: bool,
    a_live: Arc<Notify>,
    b_live: Arc<Notify>,
) -> Report {
    if !anchor {
        // Let the anchor claim the tutorial creature before we set out.
        a_live.notified().await;
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
    let mut second_started = false; // anchor: has the second party's battle begun?
    let mut my_c = String::new();
    let mut mon_c = String::new();
    let mut bid = String::new();
    let mut outcome = String::new();

    let mut mover = tokio::time::interval(Duration::from_millis(80));
    mover.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(75);

    while outcome.is_empty() {
        assert!(tokio::time::Instant::now() < deadline, "{username}: timed out");
        tokio::select! {
            // The anchor learns the second battle started (so it can stop stalling).
            _ = b_live.notified(), if anchor && !second_started => {
                second_started = true;
            }
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
                        if anchor {
                            a_live.notify_one(); // tell the second bot the anchor fight is live
                        } else {
                            b_live.notify_one(); // tell the anchor our separate fight is live
                        }
                    }
                    "battle.turn_ready" if v["payload"]["combatant_id"].as_str() == Some(my_c.as_str()) => {
                        // Anchor stalls (defends) until the second battle is live, so the
                        // two fights genuinely overlap; then everyone attacks to win.
                        let action = if anchor && !second_started { "defend" } else { "attack" };
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
    Report { outcome, battle_id: bid }
}

#[tokio::test]
async fn two_parties_fight_separate_battles_at_once() {
    let addr = start_server().await;
    let a_live = Arc::new(Notify::new());
    let b_live = Arc::new(Notify::new());
    let suffix = uuid::Uuid::new_v4().simple().to_string();

    let a = tokio::spawn(run_bot(
        addr.clone(),
        format!("anchorC_{}", &suffix[..8]),
        true,
        a_live.clone(),
        b_live.clone(),
    ));
    let b = tokio::spawn(run_bot(
        addr.clone(),
        format!("secondC_{}", &suffix[8..16]),
        false,
        a_live.clone(),
        b_live.clone(),
    ));

    let ra = a.await.unwrap();
    let rb = b.await.unwrap();

    assert_eq!(ra.outcome, "victory", "anchor party should win its own fight");
    assert_eq!(rb.outcome, "victory", "second party should win its own fight");
    assert_ne!(
        ra.battle_id, rb.battle_id,
        "the two parties must be in SEPARATE concurrent battles"
    );
}
