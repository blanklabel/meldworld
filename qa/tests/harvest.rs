//! Harvest conformance: a solo bot enters the maze, walks to a scattered
//! resource node, harvests it (material → backpack, XP → a Meld skill), then
//! Town-Portals out and the material lands in the persistent Vault. Exercises the
//! whole gather loop over the real wire + HTTP (Postgres-backed).
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
    balance.battle.party_size_per_player = 1; // one hero → stable timing
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

fn total_skill_xp(skills: &Value) -> i64 {
    skills["data"]
        .as_array()
        .map(|a| a.iter().map(|s| s["xp"].as_i64().unwrap_or(0)).sum())
        .unwrap_or(0)
}

#[tokio::test]
async fn harvest_banks_material_and_grants_skill_xp() {
    let addr = start_server().await;
    let http = reqwest::Client::new();
    let base = format!("http://{addr}");
    let username = format!("hv_{}", &uuid::Uuid::new_v4().simple().to_string()[..10]);
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
    enum Phase { Init, ToNode, InBattle, Channeling, Done }
    let mut phase = Phase::Init;
    let (mut my_c, mut mon_c, mut bid) = (String::new(), String::new(), String::new());
    let (mut my_x, mut my_y) = (0.0f64, 0.0f64);
    // The resource node we're walking to: (entity_id, x, y). Locked once chosen.
    let mut node: Option<(String, f64, f64)> = None;
    let mut harvested_kind: Option<String> = None;

    let mut mover = tokio::time::interval(Duration::from_millis(80));
    mover.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(75);

    while phase != Phase::Done {
        assert!(tokio::time::Instant::now() < deadline, "harvest timed out (phase {phase:?})");
        tokio::select! {
            _ = mover.tick(), if phase == Phase::ToNode => {
                if let Some((_, nx, ny)) = &node {
                    let (dx, dy) = (nx - my_x, ny - my_y);
                    input_seq += 1;
                    ws.send(Message::Text(json!({
                        "type":"movement.move_intent","seq":seq,"ts":0,
                        "payload":{"input_seq":input_seq,"move_dir":{"x":dx,"y":dy},"client_pos":{"x":0.0,"y":0.0}}
                    }).to_string())).await.unwrap();
                    seq += 1;
                }
            }
            msg = ws.next() => {
                let Some(Ok(Message::Text(t))) = msg else { panic!("ws closed") };
                let v: Value = serde_json::from_str(&t).unwrap();
                match v["type"].as_str().unwrap_or("") {
                    "session.authenticated" => { ws.send(Message::Text(json!({"type":"run.enter_maze","seq":seq,"ts":0,"payload":{}}).to_string())).await.unwrap(); seq += 1; }
                    "run.started" => phase = Phase::ToNode,
                    "world.snapshot" => {
                        let ents = v["payload"]["entities"].as_array().unwrap();
                        for e in ents {
                            if e["entity_id"].as_str() == Some(player_id.as_str()) {
                                my_x = e["position"]["x"].as_f64().unwrap();
                                my_y = e["position"]["y"].as_f64().unwrap();
                            }
                        }
                        // Choose the nearest resource node once, then keep its position fresh.
                        if node.is_none() {
                            let nearest = ents.iter()
                                .filter(|e| e["avatar_state"].as_str().map(|s| s.starts_with("resource:")).unwrap_or(false))
                                .map(|e| {
                                    let (x, y) = (e["position"]["x"].as_f64().unwrap(), e["position"]["y"].as_f64().unwrap());
                                    (e["entity_id"].as_str().unwrap().to_string(), x, y, (x - my_x).powi(2) + (y - my_y).powi(2))
                                })
                                .min_by(|a, b| a.3.total_cmp(&b.3));
                            if let Some((id, x, y, _)) = nearest { node = Some((id, x, y)); }
                        }
                        // Close enough → harvest (server does the authoritative range check).
                        if phase == Phase::ToNode {
                            if let Some((id, nx, ny)) = &node {
                                if ((nx - my_x).powi(2) + (ny - my_y).powi(2)).sqrt() <= 1.2 {
                                    ws.send(Message::Text(json!({"type":"run.harvest","seq":seq,"ts":0,"payload":{"entity_id":id}}).to_string())).await.unwrap();
                                    seq += 1;
                                }
                            }
                        }
                    }
                    "battle.started" => {
                        phase = Phase::InBattle;
                        my_c = v["payload"]["your_combatant_id"].as_str().unwrap().to_string();
                        bid = v["payload"]["battle_id"].as_str().unwrap().to_string();
                        mon_c = v["payload"]["enemies"][0]["combatant_id"].as_str().unwrap().to_string();
                    }
                    "battle.turn_ready" if v["payload"]["combatant_id"].as_str() == Some(my_c.as_str()) => {
                        ws.send(Message::Text(json!({"type":"battle.submit_action","seq":seq,"ts":0,
                            "payload":{"battle_id":bid,"action_id":uuid::Uuid::new_v4().to_string(),"action":"attack","skill_kind":null,"item_id":null,"target_ids":[mon_c]}}).to_string())).await.unwrap();
                        seq += 1;
                    }
                    "battle.ended" => { assert_eq!(v["payload"]["outcome"], json!("victory")); phase = Phase::ToNode; }
                    "run.backpack_update" => {
                        // The harvest change carries cause `harvest:<node>`.
                        let is_harvest = v["payload"]["changes"].as_array().into_iter().flatten().any(|ch| {
                            ch["cause"].as_str().map(|c| c.starts_with("harvest")).unwrap_or(false)
                        });
                        if is_harvest && phase != Phase::Channeling {
                            for ch in v["payload"]["changes"].as_array().into_iter().flatten() {
                                if ch["cause"].as_str().map(|c| c.starts_with("harvest")).unwrap_or(false) {
                                    harvested_kind = ch["item"]["item_kind"].as_str().map(|s| s.to_string());
                                }
                            }
                            // Prove harvest credited a Meld skill *before* extracting
                            // (extraction also grants Alchemy, so check it in isolation).
                            let mut xp = 0;
                            for _ in 0..30 {
                                let s: Value = http.get(format!("{base}/v1/meld-skills")).bearer_auth(&token).send().await.unwrap().json().await.unwrap();
                                xp = total_skill_xp(&s);
                                if xp > 0 { break; }
                                tokio::time::sleep(Duration::from_millis(100)).await;
                            }
                            assert!(xp > 0, "harvesting should credit Meld-skill XP");
                            // Now Town-Portal out to bank the material.
                            phase = Phase::Channeling;
                            ws.send(Message::Text(json!({"type":"run.begin_extraction","seq":seq,"ts":0,"payload":{"method":"town_portal","portal_entity_id":null,"item_id":null}}).to_string())).await.unwrap();
                            seq += 1;
                        }
                    }
                    "session.error" | "run.channel_interrupted" if phase == Phase::Channeling => {
                        ws.send(Message::Text(json!({"type":"run.begin_extraction","seq":seq,"ts":0,"payload":{"method":"town_portal","portal_entity_id":null,"item_id":null}}).to_string())).await.unwrap();
                        seq += 1;
                    }
                    "run.member_result" if phase == Phase::Channeling => { assert_eq!(v["payload"]["result"], json!("extracted")); phase = Phase::Done; }
                    _ => {}
                }
            }
        }
    }

    let material = harvested_kind.expect("a resource node was harvested");

    // The harvested material banked into the Vault (skill XP was already asserted
    // in isolation, before extraction, above).
    let vault: Value = http.get(format!("{base}/v1/vault")).bearer_auth(&token).send().await.unwrap().json().await.unwrap();
    let mats = vault["materials"].as_array().unwrap();
    assert!(
        mats.iter().any(|m| m["item_kind"] == json!(material)),
        "the harvested material `{material}` should be banked in the vault"
    );
}
