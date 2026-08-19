//! Meld-skill progression + crafting: a bot extracts (banking a combat drop), which
//! credits Alchemy XP; then crafts a bloom_salve from harvest reagents, crediting the
//! recipe's own skill and mutating the Vault. All persisted to Postgres, read over HTTP.
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
    let mut balance = meld_balance::Balance::load_default().unwrap();
    balance.battle.party_size_per_player = 1; // pin one hero so test timing stays stable
    balance.runs.town_portal_drop_chance = 0.0; // deterministic: no bonus Town Portal in the banked haul
    // Kit is BOUGHT now (`[runs] starting_town_portals` is 0), and this test extracts to
    // measure the skills extraction grows — it is not measuring the shop.
    balance.runs.starting_town_portals = 1;
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
    // Steer at prey: a straight line east walks past the sparse shallow ring.
    let mut nav = meld_qa::Nav::default();
    ws.send(Message::Text(
        json!({"type":"session.authenticate","seq":seq,"ts":0,"payload":{"ticket":ticket,"resume":null}}).to_string(),
    ))
    .await
    .unwrap();
    seq += 1;

    #[derive(PartialEq)]
    enum Phase { Init, ToMonster, InBattle, Channeling, Done }
    let mut phase = Phase::Init;
    let (mut my_c, mut mon_c, mut bid) = (String::new(), String::new(), String::new());
    let _ = &player_id;
    let mut mover = tokio::time::interval(Duration::from_millis(80));
    mover.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(75);

    while phase != Phase::Done {
        assert!(tokio::time::Instant::now() < deadline, "run timed out");
        tokio::select! {
            _ = mover.tick(), if matches!(phase, Phase::ToMonster) => {
                input_seq += 1;
                ws.send(Message::Text(json!({"type":"movement.move_intent","seq":seq,"ts":0,
                    "payload":{"input_seq":input_seq,"move_dir":{"x":nav.heading(0).0,"y":nav.heading(0).1},"client_pos":{"x":0.0,"y":0.0}}}).to_string())).await.unwrap();
                seq += 1;
            }
            msg = ws.next() => {
                let Some(Ok(Message::Text(t))) = msg else { panic!("ws closed") };
                let v: Value = serde_json::from_str(&t).unwrap();
                match v["type"].as_str().unwrap_or("") {
                    // Every snapshot re-aims the walk at the nearest creature.
                    "world.snapshot" => nav.observe(&v["payload"], &player_id),
                    "session.authenticated" => { ws.send(Message::Text(json!({"type":"run.enter_maze","seq":seq,"ts":0,"payload":{"tutorial":true}}).to_string())).await.unwrap(); seq += 1; }
                    "run.started" => phase = Phase::ToMonster,
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
                    "battle.ended" => {
                        assert_eq!(v["payload"]["outcome"], json!("victory"));
                        // Extract in place with the starting Town Portal item.
                        phase = Phase::Channeling;
                        ws.send(Message::Text(json!({"type":"run.begin_extraction","seq":seq,"ts":0,"payload":{"method":"town_portal","portal_entity_id":null,"item_id":null}}).to_string())).await.unwrap();
                        seq += 1;
                    }
                    "session.error" | "run.channel_interrupted" if phase == Phase::Channeling => {
                        ws.send(Message::Text(json!({"type":"run.begin_extraction","seq":seq,"ts":0,"payload":{"method":"town_portal","portal_entity_id":null,"item_id":null}}).to_string())).await.unwrap();
                        seq += 1;
                    }
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
    // One stack, not three: the run also carries the starting salves and elixirs home,
    // and diving in and straight back out with your own kit is not alchemy.
    assert_eq!(alchemy, 15, "extraction should credit 15 alchemy xp (1 stack)");

    // The banked combat drop is NOT a crafting input — every recipe takes harvest-node
    // reagents — so the craft is refused until the reagents are actually there.
    let refused = http.post(format!("{base}/v1/crafting/craft")).bearer_auth(&token).send().await.unwrap();
    assert_eq!(refused.status(), 409, "craft should be refused without the reagents");

    // Bank the two bloom_herb the recipe wants. Harvesting them for real is its own
    // test (`harvest`); doing it here would make this one hunt blind for a node and
    // pass or fail on the world seed.
    let db = meld_db::Db::connect(&std::env::var("MELD_DATABASE_URL").unwrap(), 4).await.unwrap();
    db.bank_extraction(uuid::Uuid::parse_str(&player_id).unwrap(), &[("bloom_herb".into(), 2)], 0)
        .await
        .unwrap();

    // Craft the herbs into a bloom_salve → Meld XP + Vault mutation.
    let craft = http.post(format!("{base}/v1/crafting/craft")).bearer_auth(&token).send().await.unwrap();
    assert_eq!(craft.status(), 200, "craft should succeed with the banked reagents");

    // A craft credits its OWN recipe's skill: bloom_salve is a potion, so Alchemy —
    // on top of the 15 the extraction paid. Forging stays untouched.
    let skills = get_skills().await;
    assert_eq!(skill_xp(&skills, "alchemy"), 40, "craft credits 25 alchemy xp on top of the 15");
    assert_eq!(skill_xp(&skills, "forging"), 0, "a potion craft is not forging");

    let vault: Value = http.get(format!("{base}/v1/vault")).bearer_auth(&token).send().await.unwrap().json().await.unwrap();
    let mats = vault["materials"].as_array().unwrap();
    assert!(mats.iter().any(|m| m["item_kind"] == json!("bloom_salve")), "crafted item in vault");
    assert!(
        !mats.iter().any(|m| m["item_kind"] == json!("bloom_herb") && m["quantity"].as_i64() != Some(0)),
        "the herbs were consumed"
    );
}
