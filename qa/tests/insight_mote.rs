//! The Insight Mote actually pays out (`ConsumableEffect::Experience`).
//!
//! The mote is the one consumable whose entire effect is progression, and the payout
//! crosses a layer boundary: `meld-battle` is pure and has no notion of persistent
//! levels, so drinking one only reports an `insight` status and the server is what
//! turns that into XP. Nothing was reading the status, so the mote was drunk,
//! consumed, and did nothing — `insight_mote_xp` was dead config. A unit test on
//! either side alone would have missed that, because each side was individually fine.
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
    // Every win drops a mote, and one mote is worth an unmistakable number of levels
    // — the assertion is "the XP arrived", not "the XP was exactly 250".
    balance.consumable.world_xp_item_chance = 1.0;
    balance.consumable.insight_mote_xp = 100_000;
    let config = meld_server::Config {
        bind_addr: "127.0.0.1:0".to_string(),
        database_url: db_url,
        balance: Arc::new(balance),
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
async fn drinking_an_insight_mote_actually_grants_xp() {
    let addr = start_server().await;
    let http = reqwest::Client::new();
    let base = format!("http://{addr}");
    let username = format!("im_{}", &uuid::Uuid::new_v4().simple().to_string()[..10]);
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
    let player_id = login["player"]["player_id"].as_str().unwrap().to_string();

    let (mut ws, _) = connect_async(format!("ws://{addr}/v1/realtime")).await.unwrap();
    let mut seq = 1u32;
    let mut input_seq = 0u32;
    ws.send(Message::Text(
        json!({"type":"session.authenticate","seq":seq,"ts":0,"payload":{"ticket":ticket,"resume":null}})
            .to_string(),
    ))
    .await
    .unwrap();
    seq += 1;

    let mut nav = meld_qa::Nav::default();
    let mut in_battle = false;
    let mut have_mote = false;
    let mut drank = false;
    let mut level = 1i64;
    let (mut my_c, mut mon_c, mut bid) = (String::new(), String::new(), String::new());

    let mut mover = tokio::time::interval(Duration::from_millis(80));
    mover.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(120);

    // Win a fight to be handed a mote, then find a second fight and drink it there —
    // an item is a battle action, so the mote cannot be used on the walk between.
    while !(drank && level > 1) {
        assert!(
            tokio::time::Instant::now() < deadline,
            "mote never paid out (have_mote={have_mote} drank={drank} level={level})"
        );
        tokio::select! {
            _ = mover.tick(), if !in_battle => {
                let (dx, dy) = nav.heading(0);
                input_seq += 1;
                ws.send(Message::Text(json!({"type":"movement.move_intent","seq":seq,"ts":0,
                    "payload":{"input_seq":input_seq,"move_dir":{"x":dx,"y":dy},"client_pos":{"x":0.0,"y":0.0}}
                }).to_string())).await.unwrap();
                seq += 1;
            }
            msg = ws.next() => {
                let Some(Ok(Message::Text(t))) = msg else { panic!("ws closed") };
                let v: Value = serde_json::from_str(&t).unwrap();
                match v["type"].as_str().unwrap_or("") {
                    "session.authenticated" => {
                        ws.send(Message::Text(json!({"type":"run.enter_maze","seq":seq,"ts":0,"payload":{"tutorial":true}}).to_string())).await.unwrap();
                        seq += 1;
                    }
                    "world.snapshot" => nav.observe(&v["payload"], &player_id),
                    "run.backpack_update" => {
                        for c in v["payload"]["changes"].as_array().into_iter().flatten() {
                            if c["item"]["item_kind"] == json!("insight_mote") && c["delta"] == json!("added") {
                                have_mote = true;
                            }
                        }
                    }
                    "run.party" => {
                        for h in v["payload"]["heroes"].as_array().into_iter().flatten() {
                            level = level.max(h["level"].as_i64().unwrap_or(1));
                        }
                    }
                    "run.level_up" => {
                        level = level.max(v["payload"]["new_run_level"].as_i64().unwrap_or(1));
                    }
                    "battle.started" => {
                        in_battle = true;
                        my_c = v["payload"]["your_combatant_id"].as_str().unwrap().to_string();
                        bid = v["payload"]["battle_id"].as_str().unwrap().to_string();
                        mon_c = v["payload"]["enemies"][0]["combatant_id"].as_str().unwrap().to_string();
                    }
                    "battle.turn_ready" if v["payload"]["combatant_id"].as_str() == Some(my_c.as_str()) => {
                        // Drink the mote the moment there is one; otherwise fight on.
                        let (action, item) = if have_mote && !drank {
                            drank = true;
                            ("item", json!("insight_mote"))
                        } else {
                            ("attack", Value::Null)
                        };
                        ws.send(Message::Text(json!({"type":"battle.submit_action","seq":seq,"ts":0,
                            "payload":{"battle_id":bid,"action_id":uuid::Uuid::new_v4().to_string(),
                                       "action":action,"skill_kind":null,"item_id":item,
                                       "target_ids":[if action == "item" { my_c.clone() } else { mon_c.clone() }]}
                        }).to_string())).await.unwrap();
                        seq += 1;
                    }
                    "battle.ended" => in_battle = false,
                    _ => {}
                }
            }
        }
    }

    assert!(have_mote, "a victory should have dropped a mote at chance 1.0");
    assert!(
        level > 1,
        "the mote was drunk and consumed but granted no XP — the `insight` status is \
         reported by the engine and has to be banked by the server"
    );
}
