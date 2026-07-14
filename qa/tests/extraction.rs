//! Extract-or-die conformance: a solo bot enters the maze, kills the monster
//! (loot into the backpack), walks to the extraction portal, channels an
//! extraction, and the loot is banked into the persistent Vault — verified over
//! the HTTP `GET /v1/vault` endpoint (Postgres-backed).
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
async fn extraction_banks_loot_into_the_vault() {
    let addr = start_server().await;
    let http = reqwest::Client::new();
    let base = format!("http://{addr}");
    let username = format!("ex_{}", &uuid::Uuid::new_v4().simple().to_string()[..10]);
    let body = json!({ "username": username, "password": "correct-horse-battery" });

    // Register + login → session token + realtime ticket + player id.
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

    // Vault starts empty.
    let v0: Value = http
        .get(format!("{base}/v1/vault"))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(v0["materials"].as_array().unwrap().len(), 0, "vault starts empty");

    // Connect + authenticate.
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
        ToPortal,
        Channeling,
        Done,
    }
    let mut phase = Phase::Init;
    let mut my_combatant = String::new();
    let mut monster_combatant = String::new();
    let mut battle_id = String::new();
    let mut my_x = 0.0f64;
    let mut portal_x = 14.0f64;
    let mut banked_petal = false;

    let mut mover = tokio::time::interval(Duration::from_millis(80));
    // Don't burst missed ticks after the (mover-gated) battle — that would fling
    // the avatar past the portal in one go.
    mover.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(75);

    while phase != Phase::Done {
        assert!(tokio::time::Instant::now() < deadline, "extraction timed out (phase {phase:?})");
        tokio::select! {
            _ = mover.tick(), if matches!(phase, Phase::ToMonster | Phase::ToPortal) => {
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
                    "session.authenticated" => { ws.send(Message::Text(json!({"type":"run.enter_maze","seq":seq,"ts":0,"payload":{}}).to_string())).await.unwrap(); seq += 1; }
                    "run.started" => phase = Phase::ToMonster,
                    "world.snapshot" => {
                        for e in v["payload"]["entities"].as_array().unwrap() {
                            match e["entity_id"].as_str() {
                                Some(id) if id == player_id => my_x = e["position"]["x"].as_f64().unwrap(),
                                Some("portal") => portal_x = e["position"]["x"].as_f64().unwrap(),
                                _ => {}
                            }
                        }
                        // Near the portal — stop and start channeling (the wider
                        // interaction radius tolerates a little snapshot lag).
                        if phase == Phase::ToPortal && my_x >= portal_x - 1.5 {
                            phase = Phase::Channeling;
                            ws.send(Message::Text(json!({
                                "type":"run.begin_extraction","seq":seq,"ts":0,
                                "payload":{"method":"portal","portal_entity_id":"portal","item_id":null}
                            }).to_string())).await.unwrap();
                            seq += 1;
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
                        phase = Phase::ToPortal; // now walk east to the portal
                    }
                    // A rejected channel or an interruption: walk up and retry.
                    "session.error" | "run.channel_interrupted" if phase == Phase::Channeling => phase = Phase::ToPortal,
                    "run.member_result" => {
                        assert_eq!(v["payload"]["result"], json!("extracted"));
                        let banked = v["payload"]["banked"].as_array().cloned().unwrap_or_default();
                        banked_petal = banked.iter().any(|i| i["item_kind"] == json!("forest_bloom_petal"));
                        phase = Phase::Done;
                    }
                    _ => {}
                }
            }
        }
    }

    assert!(banked_petal, "the extracted run should have banked the loot petal");

    // The Vault now persists the banked loot.
    let v1: Value = http
        .get(format!("{base}/v1/vault"))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let mats = v1["materials"].as_array().unwrap();
    let petal = mats
        .iter()
        .find(|m| m["item_kind"] == json!("forest_bloom_petal"))
        .expect("vault should contain the banked petal");
    assert_eq!(petal["quantity"], json!(1), "one petal banked");
}
