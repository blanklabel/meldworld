//! Potion consumption conformance (roadmap `GR-4`): a battle Item action spends one
//! potion from the run backpack, the client is told so, and the fourth attempt on a
//! three-potion stock is refused. Drives the real wire protocol.
//!
//! Requires Postgres: set `MELD_DATABASE_URL` (see qa/scripts/local_pg.sh).

use std::sync::Arc;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio::net::TcpListener;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;

async fn start_server() -> (String, i32) {
    let db_url = std::env::var("MELD_DATABASE_URL")
        .expect("set MELD_DATABASE_URL (see qa/scripts/local_pg.sh)");
    let mut balance = meld_balance::Balance::load_default().unwrap();
    balance.battle.party_size_per_player = 1;
    let salves = balance.runs.starting_salves;
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
    (format!("{addr}"), salves)
}

#[tokio::test]
async fn drinking_a_potion_spends_it_and_running_out_is_refused() {
    let (addr, salves) = start_server().await;
    let http = reqwest::Client::new();
    let base = format!("http://{addr}");
    let username = format!("po_{}", &uuid::Uuid::new_v4().simple().to_string()[..10]);
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
        Walking,
        Drinking,
        Done,
    }
    let mut phase = Phase::Init;
    let (mut my_c, mut bid) = (String::new(), String::new());
    let (mut my_x, mut my_y) = (0.0f64, 0.0f64);
    let mut target: Option<(f64, f64)> = None;
    let mut carried = 0i32;
    let mut spent = 0i32;
    let mut refusals = 0i32;
    let mut drinks_sent = 0i32;

    let mut mover = tokio::time::interval(Duration::from_millis(80));
    mover.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(90);

    while phase != Phase::Done {
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out in {phase:?} (carried {carried}, spent {spent}, refusals {refusals})"
        );
        tokio::select! {
            _ = mover.tick(), if phase == Phase::Walking => {
                // Steer at the nearest creature rather than walking blindly east: with
                // the encounter ramp the shallow band is sparse, and a bot that walks
                // in a straight line can miss every fight.
                let (dx, dy) = match target {
                    Some((tx, ty)) => (tx - my_x, ty - my_y),
                    None => (1.0, 0.0),
                };
                input_seq += 1;
                ws.send(Message::Text(json!({
                    "type":"movement.move_intent","seq":seq,"ts":0,
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
                    "run.started" => {
                        // The dive's OPENING stock rides on run.started; later changes
                        // arrive as run.backpack_update deltas.
                        for it in v["payload"]["backpack"].as_array().into_iter().flatten() {
                            if it["item_kind"].as_str() == Some("bloom_salve") {
                                carried += it["quantity"].as_i64().unwrap_or(0) as i32;
                            }
                        }
                        phase = Phase::Walking;
                    }
                    "world.snapshot" => {
                        let ents = v["payload"]["entities"].as_array().cloned().unwrap_or_default();
                        for e in &ents {
                            if e["avatar_state"].as_str().map(|s| s.starts_with("mob:")).unwrap_or(false) {
                                continue;
                            }
                            if e["position"]["x"].is_number() && e["avatar_state"].as_str() == Some("active") {
                                my_x = e["position"]["x"].as_f64().unwrap_or(my_x);
                                my_y = e["position"]["y"].as_f64().unwrap_or(my_y);
                            }
                        }
                        let nearest = ents
                            .iter()
                            .filter(|e| {
                                e["avatar_state"]
                                    .as_str()
                                    .map(|s| s.starts_with("mob:"))
                                    .unwrap_or(false)
                            })
                            .map(|e| {
                                let x = e["position"]["x"].as_f64().unwrap_or(0.0);
                                let y = e["position"]["y"].as_f64().unwrap_or(0.0);
                                (x, y, (x - my_x).powi(2) + (y - my_y).powi(2))
                            })
                            .min_by(|a, b| a.2.total_cmp(&b.2));
                        if let Some((x, y, _)) = nearest {
                            target = Some((x, y));
                        }
                    }
                    "run.backpack_update" => {
                        for ch in v["payload"]["changes"].as_array().into_iter().flatten() {
                            let kind = ch["item"]["item_kind"].as_str().unwrap_or("");
                            let qty = ch["item"]["quantity"].as_i64().unwrap_or(0) as i32;
                            if kind != "bloom_salve" {
                                continue;
                            }
                            match ch["delta"].as_str().unwrap_or("") {
                                "added" => carried += qty,
                                "removed" => {
                                    spent += qty;
                                    assert_eq!(
                                        ch["cause"].as_str(),
                                        Some("battle_item"),
                                        "a potion left the pack for the wrong reason"
                                    );
                                }
                                _ => {}
                            }
                        }
                    }
                    "battle.started" => {
                        my_c = v["payload"]["your_combatant_id"].as_str().unwrap().to_string();
                        bid = v["payload"]["battle_id"].as_str().unwrap().to_string();
                        phase = Phase::Drinking;
                    }
                    "battle.turn_ready" if v["payload"]["combatant_id"].as_str() == Some(my_c.as_str()) => {
                        if phase != Phase::Drinking {
                            continue;
                        }
                        if drinks_sent > salves {
                            phase = Phase::Done;
                            continue;
                        }
                        drinks_sent += 1;
                        ws.send(Message::Text(json!({
                            "type":"battle.submit_action","seq":seq,"ts":0,
                            "payload":{
                                "battle_id":bid,
                                "action_id":uuid::Uuid::new_v4().to_string(),
                                "action":"item",
                                "skill_kind":null,
                                "item_id":"bloom_salve",
                                "target_ids":[my_c]
                            }
                        }).to_string())).await.unwrap();
                        seq += 1;
                    }
                    "session.error" | "error" => {
                        let msg = v["payload"]["message"].as_str().unwrap_or_default();
                        if msg.to_lowercase().contains("out of") {
                            refusals += 1;
                            phase = Phase::Done;
                        }
                    }
                    "battle.ended" => phase = Phase::Done,
                    _ => {}
                }
            }
        }
    }

    assert_eq!(carried, salves, "the dive seeded {carried} salves, expected {salves}");
    assert!(spent > 0, "drinking a potion did not spend one");
    assert_eq!(spent, drinks_sent.min(salves), "spent {spent} for {drinks_sent} drinks");
}
