//! Harvest conformance (`MS-3` + `MS-2`): a solo bot enters the maze, walks to a
//! scattered resource node, and **works it as a channel** — one unit per tick while it
//! stands still. The test carries the design claims that make gathering a decision:
//!
//! 1. Harvesting **takes time**: the first unit does not arrive on the same tick as the
//!    request.
//! 2. It is **incremental**: a node hands over several units, one at a time.
//! 3. An interrupt costs **only the tick in flight** — walk away mid-channel and every
//!    unit already banked is still in the backpack.
//! 4. A node **runs dry** (`exhausted`) rather than giving forever.
//! 5. The whole haul banks to the persistent Vault on extraction, and harvesting credits
//!    a Meld skill.
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
async fn harvesting_is_a_channel_that_pays_as_it_goes() {
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

    #[derive(PartialEq, Debug, Clone, Copy)]
    enum Phase {
        Init,
        ToNode,
        /// Standing still, taking units off the node.
        Gathering,
        /// Deliberately walked off mid-channel; expecting the interrupt.
        ProvingInterrupt,
        /// Back on the node, draining it to nothing.
        Draining,
        Extracting,
        Done,
    }
    let mut phase = Phase::Init;
    let (mut my_c, mut mon_c, mut bid) = (String::new(), String::new(), String::new());
    let (mut my_x, mut my_y) = (0.0f64, 0.0f64);
    let mut node: Option<(String, f64, f64)> = None;
    let mut harvested_kind: Option<String> = None;
    let mut units = 0usize;
    let mut units_at_interrupt = 0usize;
    let mut saw_exhausted = false;
    let mut requested_at: Option<tokio::time::Instant> = None;
    let mut first_unit_took: Option<Duration> = None;
    let mut in_battle = false;

    let mut mover = tokio::time::interval(Duration::from_millis(80));
    mover.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(90);

    macro_rules! send {
        ($ws:expr, $t:expr, $p:tt) => {{
            $ws.send(Message::Text(
                json!({"type":$t,"seq":seq,"ts":0,"payload":$p}).to_string(),
            ))
            .await
            .unwrap();
            seq += 1;
        }};
    }

    while phase != Phase::Done {
        assert!(
            tokio::time::Instant::now() < deadline,
            "harvest timed out (phase {phase:?}, units {units})"
        );
        tokio::select! {
            // Walking is the ONLY thing this bot does on a timer — while gathering it
            // must stand still, because movement is what breaks a channel.
            _ = mover.tick(), if phase == Phase::ToNode || phase == Phase::ProvingInterrupt => {
                if let Some((_, nx, ny)) = &node {
                    // Toward the node normally; away from it when proving the interrupt.
                    let (mut dx, mut dy) = (nx - my_x, ny - my_y);
                    if phase == Phase::ProvingInterrupt {
                        dx = -dx;
                        dy = -dy;
                    }
                    input_seq += 1;
                    send!(ws, "movement.move_intent", {
                        "input_seq": input_seq,
                        "move_dir": {"x": dx, "y": dy},
                        "client_pos": {"x": 0.0, "y": 0.0}
                    });
                }
            }
            msg = ws.next() => {
                let Some(Ok(Message::Text(t))) = msg else { panic!("ws closed") };
                let v: Value = serde_json::from_str(&t).unwrap();
                match v["type"].as_str().unwrap_or("") {
                    "session.authenticated" => {
                        send!(ws, "run.enter_maze", {"tutorial": true});
                    }
                    "run.started" => phase = Phase::ToNode,
                    "world.snapshot" => {
                        let ents = v["payload"]["entities"].as_array().unwrap();
                        for e in ents {
                            if e["entity_id"].as_str() == Some(player_id.as_str()) {
                                my_x = e["position"]["x"].as_f64().unwrap();
                                my_y = e["position"]["y"].as_f64().unwrap();
                            }
                        }
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
                        let Some((id, nx, ny)) = node.clone() else { continue };
                        let close = ((nx - my_x).powi(2) + (ny - my_y).powi(2)).sqrt() <= 1.2;
                        match phase {
                            // In reach and standing on it → open the channel and stop moving.
                            Phase::ToNode if close && !in_battle => {
                                requested_at = Some(tokio::time::Instant::now());
                                send!(ws, "run.harvest", {"entity_id": id});
                                phase = Phase::Gathering;
                            }
                            Phase::Draining if close => {
                                send!(ws, "run.harvest", {"entity_id": id});
                            }
                            _ => {}
                        }
                    }
                    "battle.started" => {
                        in_battle = true;
                        my_c = v["payload"]["your_combatant_id"].as_str().unwrap().to_string();
                        bid = v["payload"]["battle_id"].as_str().unwrap().to_string();
                        mon_c = v["payload"]["enemies"][0]["combatant_id"].as_str().unwrap().to_string();
                    }
                    "battle.turn_ready" if v["payload"]["combatant_id"].as_str() == Some(my_c.as_str()) => {
                        send!(ws, "battle.submit_action", {
                            "battle_id": bid,
                            "action_id": uuid::Uuid::new_v4().to_string(),
                            "action": "attack",
                            "skill_kind": null,
                            "item_id": null,
                            "target_ids": [mon_c]
                        });
                    }
                    "battle.ended" => {
                        assert_eq!(v["payload"]["outcome"], json!("victory"));
                        in_battle = false;
                        // A fight breaks the channel, so go back and start again.
                        if phase == Phase::Gathering || phase == Phase::Draining {
                            phase = Phase::ToNode;
                        }
                    }
                    "run.backpack_update" => {
                        for ch in v["payload"]["changes"].as_array().into_iter().flatten() {
                            if ch["cause"].as_str().map(|c| c.starts_with("harvest")).unwrap_or(false) {
                                units += 1;
                                harvested_kind = ch["item"]["item_kind"].as_str().map(|s| s.to_string());
                                if first_unit_took.is_none() {
                                    first_unit_took = requested_at.map(|t| t.elapsed());
                                }
                            }
                        }
                        // Two units in hand proves the channel is incremental. Now walk
                        // off mid-channel and prove what an interrupt actually costs.
                        if phase == Phase::Gathering && units >= 2 {
                            units_at_interrupt = units;
                            phase = Phase::ProvingInterrupt;
                        }
                    }
                    "run.channel_interrupted" => {
                        let reason = v["payload"]["reason"].as_str().unwrap_or("").to_string();
                        match phase {
                            Phase::ProvingInterrupt => {
                                assert_eq!(
                                    reason, "moved",
                                    "walking away should break the channel as `moved`"
                                );
                                // The claim: only the tick in flight is lost.
                                assert_eq!(
                                    units, units_at_interrupt,
                                    "an interrupted channel must not claw back banked units"
                                );
                                phase = Phase::Draining;
                            }
                            Phase::Draining if reason == "exhausted" => {
                                saw_exhausted = true;
                                phase = Phase::Extracting;
                                send!(ws, "run.begin_extraction", {
                                    "method": "town_portal", "portal_entity_id": null, "item_id": null
                                });
                            }
                            // Any other break (a fight, a stray move) → walk back and resume.
                            Phase::Draining => phase = Phase::ToNode,
                            Phase::Extracting => {
                                send!(ws, "run.begin_extraction", {
                                    "method": "town_portal", "portal_entity_id": null, "item_id": null
                                });
                            }
                            _ => {}
                        }
                    }
                    "run.member_result" if phase == Phase::Extracting => {
                        assert_eq!(v["payload"]["result"], json!("extracted"));
                        phase = Phase::Done;
                    }
                    _ => {}
                }
            }
        }
    }

    let material = harvested_kind.expect("a resource node was worked");
    let took = first_unit_took.expect("timed the first unit");

    // 1. Harvesting takes time — the first unit is not free on the same tick.
    assert!(
        took >= Duration::from_millis(300),
        "the first unit arrived in {took:?} — harvesting should be a channel, not a tap"
    );
    // 2 + 4. It paid out repeatedly, and the node ran dry rather than giving forever.
    assert!(units >= 3, "expected several units off one node, got {units}");
    assert!(saw_exhausted, "the node should end the channel by running out");

    // 5. Harvesting credited a Meld skill, and the whole haul banked to the Vault.
    let skills: Value = http
        .get(format!("{base}/v1/meld-skills"))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(total_skill_xp(&skills) > 0, "harvesting should credit Meld-skill XP");

    let vault: Value = http
        .get(format!("{base}/v1/vault"))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let banked = vault["materials"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["item_kind"] == json!(material))
        .and_then(|m| m["quantity"].as_i64())
        .unwrap_or(0);
    assert!(
        banked >= 2,
        "the haul of `{material}` should bank to the Vault, got {banked} (harvested {units})"
    );
}
