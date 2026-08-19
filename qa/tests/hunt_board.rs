//! Hunt Board conformance (roadmap `AD-4`): the board is posted for a brand-new
//! account, it refuses a reward nobody has earned, and a creature felled in a real
//! fight moves the hunt that counts it — the whole server-authoritative path
//! (victory → session board → persistence → `GET /v1/hunts`), with no
//! client-submitted progress anywhere in it (CANON §S).
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
    let balance = Arc::new(meld_balance::Balance::load_default().unwrap());
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

/// Register + log in; returns (session token, realtime ticket, player id).
async fn fresh_player(base: &str, http: &reqwest::Client) -> (String, String, String) {
    let username = format!("hunt_{}", &uuid::Uuid::new_v4().simple().to_string()[..10]);
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
    (
        login["session_token"].as_str().unwrap().to_string(),
        login["realtime_ticket"].as_str().unwrap().to_string(),
        login["player"]["player_id"].as_str().unwrap().to_string(),
    )
}

async fn board(base: &str, http: &reqwest::Client, token: &str) -> Vec<Value> {
    let v: Value = http
        .get(format!("{base}/v1/hunts"))
        .bearer_auth(token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    v["data"].as_array().cloned().unwrap_or_default()
}

/// One test rather than two: both halves want a freshly-migrated server, and two of
/// them booting at the same instant race each other's `CREATE TABLE` (Postgres answers
/// with a duplicate-key on `pg_type`).
#[tokio::test]
async fn the_board_posts_refuses_what_nobody_earned_and_counts_a_real_kill() {
    let addr = start_server().await;
    let http = reqwest::Client::new();
    let base = format!("http://{addr}");
    let (token, ticket, _player_id) = fresh_player(&base, &http).await;

    let rows = board(&base, &http, &token).await;
    assert_eq!(
        rows.len(),
        meld_proto::hunts::HUNTS.len(),
        "every posted hunt is on the board, touched or not"
    );
    for r in &rows {
        assert_eq!(r["progress"], json!(0), "a new account has started nothing: {r}");
        assert_eq!(r["claimable"], json!(false));
        assert_eq!(r["claimed"], json!(false));
        assert!(r["target"].as_i64().unwrap() >= 1, "an unreachable hunt: {r}");
        // A row that advertises nothing is a row nobody walks over to read.
        assert!(r["reward_chits"].as_i64().unwrap() > 0, "{r} pays nothing");
        let objective = r["objective"].as_str().unwrap();
        assert!(
            objective.chars().any(|c| c.is_ascii_digit()),
            "objective states no number: {objective}"
        );
    }

    // A hunt nobody has finished pays nothing, and says how far off it is.
    let res = http
        .post(format!("{base}/v1/hunts/cull_the_bloom/claim"))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 409);
    let err: Value = res.json().await.unwrap();
    let msg = err["error"]["message"].as_str().unwrap();
    assert!(msg.contains("0/"), "a refusal should name the progress: {msg}");

    let vault: Value = http
        .get(format!("{base}/v1/vault"))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(vault["chits"].as_i64().unwrap_or(0), 0, "a refusal costs the board nothing");

    // An unknown hunt is a 404, not a silently-ignored payout.
    let res = http
        .post(format!("{base}/v1/hunts/no_such_hunt/claim"))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 404);

    // The board is the caller's own: reading it needs a session.
    let res = http.get(format!("{base}/v1/hunts")).send().await.unwrap();
    assert_eq!(res.status(), 401);

    // TAKE the hunt. Progress is only credited to an accepted hunt — a posted hunt used to
    // count itself from the moment the account existed, which made the board eight jobs
    // nobody had agreed to.
    let res = http
        .post(format!("{base}/v1/hunts/cull_the_bloom/accept"))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200, "taking a posted hunt should be allowed");
    // Idempotent: a second press is not an error.
    let res = http
        .post(format!("{base}/v1/hunts/cull_the_bloom/accept"))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    // A hunt that does not exist cannot be taken.
    let res = http
        .post(format!("{base}/v1/hunts/no_such_hunt/accept"))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 404);

    // The TUTORIAL dive is the deterministic one: area 0 holds exactly one creature,
    // `forest_bloom_stalker`, on the centre line — the quarry `cull_the_bloom` counts.
    // In any other world, which species dies is a roll.
    let (mut ws, _) = connect_async(format!("ws://{addr}/v1/realtime")).await.unwrap();
    let mut seq = 1u32;
    let mut input_seq = 0u32;
    ws.send(Message::Text(
        json!({"type":"session.authenticate","seq":seq,"ts":0,
               "payload":{"ticket":ticket,"resume":null}})
        .to_string(),
    ))
    .await
    .unwrap();
    seq += 1;

    let mut started = false;
    let mut won = false;
    let mut progressed = false;
    let mut marked = false;
    let (mut my_c, mut mon_c, mut bid) = (String::new(), String::new(), String::new());

    let mut mover = tokio::time::interval(Duration::from_millis(80));
    mover.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(75);

    while !won {
        assert!(tokio::time::Instant::now() < deadline, "never won a fight");
        tokio::select! {
            // Straight east, no prey-steering: area 0 of a tutorial run guarantees that
            // walk meets its one creature, and steering at the nearest mob can pick up
            // something from the procedural area past it instead.
            _ = mover.tick(), if started && bid.is_empty() => {
                input_seq += 1;
                ws.send(Message::Text(json!({
                    "type":"movement.move_intent","seq":seq,"ts":0,
                    "payload":{"input_seq":input_seq,"move_dir":{"x":1.0,"y":0.0},
                               "client_pos":{"x":0.0,"y":0.0}}
                }).to_string())).await.unwrap();
                seq += 1;
            }
            msg = ws.next() => {
                let Some(Ok(Message::Text(t))) = msg else { panic!("ws closed") };
                let v: Value = serde_json::from_str(&t).unwrap();
                match v["type"].as_str().unwrap_or("") {
                    "session.authenticated" => {
                        ws.send(Message::Text(json!({"type":"run.enter_maze","seq":seq,"ts":0,
                            "payload":{"tutorial":true}}).to_string())).await.unwrap();
                        seq += 1;
                    }
                    "run.started" => started = true,
                    // AD-4: the quarry of a hunt you are working is force-included in
                    // YOUR snapshot and tagged, which is what makes it trackable rather
                    // than something you stumble into.
                    "world.snapshot" => {
                        for e in v["payload"]["entities"].as_array().into_iter().flatten() {
                            let Some(state) = e["avatar_state"].as_str() else { continue };
                            if !state.ends_with(":quarry") {
                                continue;
                            }
                            // A fresh account is working every hunt, so the mark can land
                            // on any of their quarries — but only on one of THOSE. The
                            // registry is the list; a hand-written one here would drift.
                            let kind = state.strip_prefix("mob:").and_then(|r| r.split(':').next());
                            let class = e["encounter_class"].as_str().unwrap_or("");
                            let wanted = meld_proto::hunts::HUNTS.iter().any(|h| match h.goal {
                                meld_proto::hunts::HuntGoal::Fell { creature, .. } => {
                                    kind == Some(creature)
                                }
                                meld_proto::hunts::HuntGoal::FellClass { class: c, .. } => c == class,
                                _ => false,
                            });
                            assert!(wanted, "something no hunt asks for was marked: {e}");
                            marked = true;
                        }
                    }
                    "battle.started" => {
                        assert_eq!(
                            v["payload"]["enemies"][0]["monster_kind"],
                            json!("forest_bloom_stalker"),
                            "the tutorial's one creature is the quarry this hunt counts"
                        );
                        my_c = v["payload"]["your_combatant_id"].as_str().unwrap().to_string();
                        bid = v["payload"]["battle_id"].as_str().unwrap().to_string();
                        mon_c = v["payload"]["enemies"][0]["combatant_id"].as_str().unwrap().to_string();
                    }
                    "battle.turn_ready"
                        if v["payload"]["combatant_id"].as_str() == Some(my_c.as_str()) =>
                    {
                        ws.send(Message::Text(json!({"type":"battle.submit_action","seq":seq,"ts":0,
                            "payload":{"battle_id":bid,
                                       "action_id":uuid::Uuid::new_v4().to_string(),
                                       "action":"attack","skill_kind":null,"item_id":null,
                                       "target_ids":[mon_c]}}).to_string())).await.unwrap();
                        seq += 1;
                    }
                    "run.hunt_progress" if v["payload"]["key"] == json!("cull_the_bloom") => {
                        assert_eq!(v["payload"]["progress"], json!(1));
                        assert_eq!(v["payload"]["target"], json!(8));
                        assert_eq!(v["payload"]["complete"], json!(false));
                        progressed = true;
                    }
                    "battle.ended" => {
                        if v["payload"]["outcome"] == json!("victory") {
                            won = true;
                        } else {
                            panic!("the tutorial creature won: {v}");
                        }
                    }
                    _ => {}
                }
            }
        }
    }
    // `battle.ended` and the hunt announcement leave the loop from different arms, so
    // either can land first; keep reading for a beat rather than assuming an order.
    let drain = tokio::time::Instant::now() + Duration::from_secs(3);
    while !progressed && tokio::time::Instant::now() < drain {
        let Ok(Some(Ok(Message::Text(t)))) =
            tokio::time::timeout(Duration::from_millis(400), ws.next()).await
        else {
            continue;
        };
        let v: Value = serde_json::from_str(&t).unwrap();
        if v["type"] == json!("run.hunt_progress") && v["payload"]["key"] == json!("cull_the_bloom")
        {
            assert_eq!(v["payload"]["progress"], json!(1));
            progressed = true;
        }
    }
    assert!(progressed, "the kill never announced itself to the board");
    assert!(marked, "the hunt's quarry was never marked in the snapshot");

    // …and it is on the board over HTTP, which is the half that survives a relog.
    // The credit is fire-and-forget off the game loop, so give the DB task a beat.
    let mut rows = Vec::new();
    for _ in 0..25 {
        tokio::time::sleep(Duration::from_millis(200)).await;
        rows = board(&base, &http, &token).await;
        if rows
            .iter()
            .any(|r| r["key"] == json!("cull_the_bloom") && r["progress"].as_i64() == Some(1))
        {
            break;
        }
    }
    let bloom = rows.iter().find(|r| r["key"] == json!("cull_the_bloom")).unwrap();
    assert_eq!(bloom["progress"], json!(1), "the felled stalker never reached the board");
    assert_eq!(bloom["claimable"], json!(false), "one of eight is not a finished hunt");

    // A hunt the kill has nothing to do with stays where it was: the board counts the
    // creature's own kind, not "something died".
    let mire = rows.iter().find(|r| r["key"] == json!("drain_the_mire")).unwrap();
    assert_eq!(mire["progress"], json!(0));
}
