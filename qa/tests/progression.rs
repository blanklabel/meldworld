//! Meld-skill progression + crafting: a bot extracts (banking a loot petal),
//! which credits Alchemy XP; then crafts the petal into a bloom_salve, crediting
//! Forging XP and mutating the Vault. All persisted to Postgres, read over HTTP.
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

fn skill_xp(skills: &Value, kind: &str) -> i64 {
    skills["data"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["skill_kind"] == json!(kind))
        .map(|s| s["xp"].as_i64().unwrap())
        .unwrap_or(-1)
}

#[tokio::test]
async fn extraction_and_crafting_grow_meld_skills() {
    let addr = start_server().await;
    let http = reqwest::Client::new();
    let base = format!("http://{addr}");
    let username = format!("pg_{}", &uuid::Uuid::new_v4().simple().to_string()[..10]);
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
    // Fresh skills are all 0.
    assert_eq!(login["player"]["meld_skills"].as_array().unwrap().len(), 3);

    // --- run: win, walk to portal, extract (reused from the extraction flow) ---
    let (mut ws, _) = connect_async(format!("ws://{addr}/v1/realtime")).await.unwrap();
    let mut seq = 1u32;
    let mut input_seq = 0u32;
    ws.send(Message::Text(
        json!({"type":"session.authenticate","seq":seq,"ts":0,"payload":{"ticket":ticket,"resume":null}}).to_string(),
    ))
    .await
    .unwrap();
    seq += 1;

    #[derive(PartialEq)]
    enum Phase { Init, ToMonster, InBattle, ToPortal, Channeling, Done }
    let mut phase = Phase::Init;
    let (mut my_c, mut mon_c, mut bid) = (String::new(), String::new(), String::new());
    let (mut my_x, mut portal_x) = (0.0f64, 14.0f64);
    let mut mover = tokio::time::interval(Duration::from_millis(80));
    mover.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);

    while phase != Phase::Done {
        assert!(tokio::time::Instant::now() < deadline, "run timed out");
        tokio::select! {
            _ = mover.tick(), if matches!(phase, Phase::ToMonster | Phase::ToPortal) => {
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
                    "world.snapshot" => {
                        for e in v["payload"]["entities"].as_array().unwrap() {
                            match e["entity_id"].as_str() {
                                Some(id) if id == player_id => my_x = e["position"]["x"].as_f64().unwrap(),
                                Some("portal") => portal_x = e["position"]["x"].as_f64().unwrap(),
                                _ => {}
                            }
                        }
                        if phase == Phase::ToPortal && my_x >= portal_x - 1.5 {
                            phase = Phase::Channeling;
                            ws.send(Message::Text(json!({"type":"run.begin_extraction","seq":seq,"ts":0,"payload":{"method":"portal","portal_entity_id":"portal","item_id":null}}).to_string())).await.unwrap();
                            seq += 1;
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
                    "battle.ended" => { assert_eq!(v["payload"]["outcome"], json!("victory")); phase = Phase::ToPortal; }
                    "session.error" | "run.channel_interrupted" if phase == Phase::Channeling => phase = Phase::ToPortal,
                    "run.member_result" => { assert_eq!(v["payload"]["result"], json!("extracted")); phase = Phase::Done; }
                    _ => {}
                }
            }
        }
    }

    // Extraction credited Alchemy XP.
    let get_skills = || async {
        http.get(format!("{base}/v1/meld-skills")).bearer_auth(&token).send().await.unwrap().json::<Value>().await.unwrap()
    };
    let mut alchemy = 0;
    for _ in 0..40 {
        let s = get_skills().await;
        if skill_xp(&s, "alchemy") > 0 { alchemy = skill_xp(&s, "alchemy"); break; }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_eq!(alchemy, 15, "extraction should credit 15 alchemy xp (1 stack)");

    // Craft the banked petal into a bloom_salve → Forging XP + Vault mutation.
    let craft = http.post(format!("{base}/v1/crafting/craft")).bearer_auth(&token).send().await.unwrap();
    assert_eq!(craft.status(), 200, "craft should succeed with the banked petal");

    let skills = get_skills().await;
    assert_eq!(skill_xp(&skills, "forging"), 25, "craft credits 25 forging xp");

    let vault: Value = http.get(format!("{base}/v1/vault")).bearer_auth(&token).send().await.unwrap().json().await.unwrap();
    let mats = vault["materials"].as_array().unwrap();
    assert!(mats.iter().any(|m| m["item_kind"] == json!("bloom_salve")), "crafted item in vault");
    assert!(!mats.iter().any(|m| m["item_kind"] == json!("forest_bloom_petal")), "petal was consumed");
}
