//! Loop-closes conformance: a solo bot dives, kills the monster, extracts with
//! its Town Portal — and then **dives again**. Before `release_from_run`, the
//! session's `in_instance` flag stayed `true` after a run ended, so the second
//! `run.enter_maze` was rejected with "A run is already active for you." and the
//! extract-or-die loop never closed. This test asserts the second dive reaches a
//! fresh `run.started` (see CITY-PROPOSAL.md M0).
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
    balance.battle.party_size_per_player = 1; // pin one hero so test timing stays stable
    balance.runs.town_portal_drop_chance = 0.0; // deterministic banked haul
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
async fn a_player_can_dive_again_after_extracting() {
    let addr = start_server().await;
    let http = reqwest::Client::new();
    let base = format!("http://{addr}");
    let username = format!("rd_{}", &uuid::Uuid::new_v4().simple().to_string()[..10]);
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
    enum Phase {
        Init,
        ToMonster,
        InBattle,
        Channeling,
        // The whole point: a *second* dive after the first run ended.
        SecondDive,
        Done,
    }
    let mut phase = Phase::Init;
    let mut my_combatant = String::new();
    let mut monster_combatant = String::new();
    let mut battle_id = String::new();

    let mut mover = tokio::time::interval(Duration::from_millis(80));
    mover.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(90);

    while phase != Phase::Done {
        assert!(tokio::time::Instant::now() < deadline, "re-dive timed out (phase {phase:?})");
        tokio::select! {
            _ = mover.tick(), if matches!(phase, Phase::ToMonster) => {
                input_seq += 1;
                ws.send(Message::Text(json!({
                    "type":"movement.move_intent","seq":seq,"ts":0,
                    "payload":{"input_seq":input_seq,"move_dir":{"x":1.0,"y":0.0},"client_pos":{"x":0.0,"y":0.0}}
                }).to_string())).await.unwrap();
                seq += 1;
            }
            msg = ws.next() => {
                let Some(Ok(Message::Text(t))) = msg else { panic!("ws closed unexpectedly") };
                let v: Value = serde_json::from_str(&t).unwrap();
                match v["type"].as_str().unwrap_or("") {
                    "session.authenticated" => {
                        ws.send(Message::Text(json!({"type":"run.enter_maze","seq":seq,"ts":0,"payload":{}}).to_string())).await.unwrap();
                        seq += 1;
                    }
                    "run.started" => {
                        // First dive → walk to the monster; second dive → we're done.
                        if phase == Phase::SecondDive {
                            phase = Phase::Done;
                        } else {
                            phase = Phase::ToMonster;
                        }
                    }
                    "battle.started" => {
                        phase = Phase::InBattle;
                        my_combatant = v["payload"]["your_combatant_id"].as_str().unwrap().to_string();
                        battle_id = v["payload"]["battle_id"].as_str().unwrap().to_string();
                        monster_combatant = v["payload"]["enemies"][0]["combatant_id"].as_str().unwrap().to_string();
                    }
                    "battle.turn_ready" if v["payload"]["combatant_id"].as_str() == Some(my_combatant.as_str()) => {
                        ws.send(Message::Text(json!({
                            "type":"battle.submit_action","seq":seq,"ts":0,
                            "payload":{"battle_id":battle_id,"action_id":uuid::Uuid::new_v4().to_string(),"action":"attack","skill_kind":null,"item_id":null,"target_ids":[monster_combatant]}
                        }).to_string())).await.unwrap();
                        seq += 1;
                    }
                    "battle.ended" => {
                        assert_eq!(v["payload"]["outcome"], json!("victory"), "solo should win");
                        phase = Phase::Channeling;
                        ws.send(Message::Text(json!({
                            "type":"run.begin_extraction","seq":seq,"ts":0,
                            "payload":{"method":"town_portal","portal_entity_id":null,"item_id":null}
                        }).to_string())).await.unwrap();
                        seq += 1;
                    }
                    "session.error" | "run.channel_interrupted" if phase == Phase::Channeling => {
                        ws.send(Message::Text(json!({
                            "type":"run.begin_extraction","seq":seq,"ts":0,
                            "payload":{"method":"town_portal","portal_entity_id":null,"item_id":null}
                        }).to_string())).await.unwrap();
                        seq += 1;
                    }
                    "run.member_result" => {
                        assert_eq!(v["payload"]["result"], json!("extracted"));
                        // The run is over. Dive AGAIN — this is what `release_from_run`
                        // makes possible. A regression re-sends the "already active"
                        // error instead of a fresh run.started.
                        phase = Phase::SecondDive;
                        ws.send(Message::Text(json!({"type":"run.enter_maze","seq":seq,"ts":0,"payload":{}}).to_string())).await.unwrap();
                        seq += 1;
                    }
                    "session.error" if phase == Phase::SecondDive => {
                        panic!(
                            "second dive rejected — loop did not close: {}",
                            v["payload"]["message"].as_str().unwrap_or("")
                        );
                    }
                    _ => {}
                }
            }
        }
    }
}
