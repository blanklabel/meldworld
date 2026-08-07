//! The pacing arc, played rather than calculated (`PT-4`).
//!
//! The design says a new player's first hours are quick and easy — one hero, short
//! fights, fast levels — and that by the time all four party slots are unlocked a run
//! is a long haul. Both halves of that come from balance numbers that were tuned
//! against arithmetic: encounter XP is split across the party, and creature health
//! scales superlinearly with party size.
//!
//! Arithmetic cannot tell you whether the resulting game is playable. This drives a
//! real bot through a real dive at one, two, three and four heroes and reports what
//! actually happened, then asserts the shape the design claims:
//!
//! - a party of any size can win fights (the floor — a wipe at any size is a bug),
//! - a lone hero levels FASTER per fight than a full party (the XP split),
//! - a full party's fights take LONGER than a lone hero's (the encounter ramp).
//!
//! Requires Postgres: set `MELD_DATABASE_URL` (see qa/scripts/local_pg.sh).

use std::sync::Arc;
use std::time::{Duration, Instant};

use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio::net::TcpListener;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;

/// What one dive looked like.
#[derive(Debug)]
struct Dive {
    heroes: usize,
    fights_won: usize,
    /// Highest level any hero reached inside the dive.
    level: i32,
    /// Mean wall-clock seconds from `battle.started` to `battle.ended`.
    secs_per_fight: f64,
    wiped: bool,
}

async fn start_server(heroes: usize) -> String {
    let db_url = std::env::var("MELD_DATABASE_URL")
        .expect("set MELD_DATABASE_URL (see qa/scripts/local_pg.sh)");
    let mut balance = meld_balance::Balance::load_default().unwrap();
    balance.battle.party_size_per_player = heroes;
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

/// Play one dive with `heroes` heroes for at most `budget`, fighting whatever it can
/// reach, and report what happened.
async fn dive(heroes: usize, budget: Duration) -> Dive {
    let addr = start_server(heroes).await;
    let http = reqwest::Client::new();
    let base = format!("http://{addr}");
    let username = format!("pace{heroes}_{}", &uuid::Uuid::new_v4().simple().to_string()[..8]);
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
    let mut my_c = String::new();
    let mut bid = String::new();
    let mut out = Dive {
        heroes,
        fights_won: 0,
        level: 1,
        secs_per_fight: 0.0,
        wiped: false,
    };
    let mut fight_started: Option<Instant> = None;
    let mut total_fight = Duration::ZERO;

    let mut mover = tokio::time::interval(Duration::from_millis(80));
    mover.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let deadline = Instant::now() + budget;

    while Instant::now() < deadline && !out.wiped {
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
                let Some(Ok(Message::Text(t))) = msg else { break };
                let v: Value = serde_json::from_str(&t).unwrap();
                match v["type"].as_str().unwrap_or("") {
                    "session.authenticated" => {
                        ws.send(Message::Text(json!({"type":"run.enter_maze","seq":seq,"ts":0,"payload":{"tutorial":true}}).to_string())).await.unwrap();
                        seq += 1;
                    }
                    "world.snapshot" => nav.observe(&v["payload"], &player_id),
                    "run.party" => {
                        for h in v["payload"]["heroes"].as_array().into_iter().flatten() {
                            out.level = out.level.max(h["level"].as_i64().unwrap_or(1) as i32);
                        }
                    }
                    "battle.started" => {
                        in_battle = true;
                        fight_started = Some(Instant::now());
                        my_c = v["payload"]["your_combatant_id"].as_str().unwrap_or_default().to_string();
                        bid = v["payload"]["battle_id"].as_str().unwrap_or_default().to_string();
                    }
                    "battle.turn_ready"
                        if v["payload"]["combatant_id"].as_str() == Some(my_c.as_str()) =>
                    {
                        ws.send(Message::Text(json!({"type":"battle.submit_action","seq":seq,"ts":0,
                            "payload":{"battle_id":bid,"action_id":uuid::Uuid::new_v4().to_string(),
                                       "action":"attack","skill_kind":null,"item_id":null,"target_ids":[]}
                        }).to_string())).await.unwrap();
                        seq += 1;
                    }
                    "battle.ended" => {
                        in_battle = false;
                        if let Some(t0) = fight_started.take() {
                            total_fight += t0.elapsed();
                        }
                        match v["payload"]["outcome"].as_str() {
                            Some("victory") => out.fights_won += 1,
                            Some("defeat") => out.wiped = true,
                            _ => {}
                        }
                    }
                    _ => {}
                }
            }
        }
    }
    if out.fights_won > 0 {
        out.secs_per_fight = total_fight.as_secs_f64() / out.fights_won as f64;
    }
    out
}

#[tokio::test]
async fn the_pacing_arc_holds_from_one_hero_to_four() {
    let budget = Duration::from_secs(50);
    let mut runs = Vec::new();
    for heroes in [1usize, 2, 3, 4] {
        runs.push(dive(heroes, budget).await);
    }
    for d in &runs {
        println!(
            "  {} hero(es): {} fights won, level {}, {:.1}s per fight, wiped={}",
            d.heroes, d.fights_won, d.level, d.secs_per_fight, d.wiped
        );
    }

    // The floor: every party size can actually play the game. A size that cannot win
    // a fight in fifty seconds is a balance bug, and this is exactly the check that
    // caught creature attack being multiplied by party size.
    for d in &runs {
        assert!(
            d.fights_won > 0,
            "a party of {} won nothing in {budget:?} (wiped={})",
            d.heroes,
            d.wiped
        );
        assert!(!d.wiped, "a party of {} was wiped out", d.heroes);
    }

    // The XP split: a lone hero absorbs a whole encounter, four share it, so the solo
    // dive should climb at least as fast per fight as the full party's.
    let solo = &runs[0];
    let full = &runs[3];
    let solo_rate = solo.level as f64 / solo.fights_won.max(1) as f64;
    let full_rate = full.level as f64 / full.fights_won.max(1) as f64;
    assert!(
        solo_rate >= full_rate,
        "a lone hero levelled slower per fight than a full party: {solo_rate:.2} vs {full_rate:.2}"
    );

    // The encounter ramp: creature health scales superlinearly with party size, so a
    // full party's fights are longer than a lone hero's rather than four times faster.
    assert!(
        full.secs_per_fight >= solo.secs_per_fight,
        "a full party's fights were SHORTER than a lone hero's ({:.1}s vs {:.1}s) — the \
         party ramp is not doing its job",
        full.secs_per_fight,
        solo.secs_per_fight
    );
}
