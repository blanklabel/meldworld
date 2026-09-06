//! The pacing arc, played rather than calculated (`PT-4`).
//!
//! ⚠️ **THE ARC THIS FILE ASSERTS WAS INVERTED.** It used to hold that a full party's
//! fights take LONGER than a lone hero's, because creature health scaled superlinearly
//! with party size (`encounter_party_scale`, [1.0, 1.9, 3.0, 4.4] on HP). That is
//! retired: a creature's HP, attack, defence, speed and XP are fixed by its LEVEL, and
//! nothing looks at how many heroes are standing in front of it.
//!
//! So the design's claim is now the other one, and it is a better claim because it is
//! about a thing the player feels:
//!
//! - a party of any size can win fights (the floor — a wipe at any size is a bug),
//! - a full party's fights are SHORTER, because four heroes bring four times the damage
//!   to the same creature; that shorter fight is what mustering buys,
//! - and XP per SECOND is roughly FLAT across party size, because a fixed pool divided
//!   among the survivors of a fight that took a quarter as long is the same rate.
//!
//! That last one is the whole reason the two halves fit together. A lone hero banks the
//! entire pool over four times the turns; each of four banks a quarter of it over one.
//! What changes with party size is not how fast you level, it is how much of the world
//! you can survive — which is the axis the game actually has.
//!
//! Each size still gets a budget scaled to its party, unchanged: it is now generous
//! rather than necessary, and a generous budget is what keeps a shared, loaded box from
//! failing the run for being slow.
//!
//! ⚠️ **Every measured figure that used to be quoted here is void** — 12.5 / 35.1 / 37.5
//! / 51.6 seconds per fight and 4.83 / 1.85 / 1.63 / 1.30 XP per second were all taken
//! against scaled encounters. Read the printout, not this comment, and re-record it here
//! once it has been run on a quiet box.
//!
//! The bounds below are deliberately LOOSE. This test drives real bots through real-time
//! loops on a box shared with up to twenty other agents, so a tight rate band is a test
//! that goes red on its own schedule — and a gate that does that trains everyone to
//! ignore it. It asserts the SHAPE (ordering, and a wide band on the rate), never a value.
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
    /// TOTAL XP banked by the best hero — the levels it bought plus the bar it is on.
    /// The raw `xp` field is only the remainder after a level-up spends its cost, so
    /// comparing those across party sizes rewards whoever levelled least.
    xp: i64,
    /// Mean wall-clock seconds from `battle.started` to `battle.ended`.
    secs_per_fight: f64,
    wiped: bool,
}

async fn start_server(heroes: usize) -> String {
    let db_url = std::env::var("MELD_DATABASE_URL")
        .expect("set MELD_DATABASE_URL (see qa/scripts/local_pg.sh)");
    // Pin the world: these bots have to FIND fights, and whether they do is decided by
    // the roll — unseeded, all four sizes won nothing in 50s.
    std::env::set_var("MELD_SEED", "1");
    let mut balance = meld_balance::Balance::load_default().unwrap();
    balance.battle.party_size_per_player = heroes;
    let config = meld_server::Config {
        bind_addr: "127.0.0.1:0".to_string(),
        database_url: db_url,
        balance: Arc::new(balance),
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

    // Grant the party SLOTS this dive is meant to test. Party size is the slots an
    // account has EARNED, capped by `party_size_per_player` — raising the cap alone
    // leaves a fresh account fielding one hero, which is the whole arc collapsed.
    if heroes > 1 {
        let db = meld_db::Db::connect(&std::env::var("MELD_DATABASE_URL").unwrap(), 4)
            .await
            .unwrap();
        let keys: Vec<String> = (2..=heroes).map(|n| format!("party_slot_{n}")).collect();
        db.grant_unlocks(uuid::Uuid::parse_str(&player_id).unwrap(), &keys)
            .await
            .unwrap();
    }

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

    let balance_ref = meld_balance::Balance::load_default().unwrap();
    let mut nav = meld_qa::Nav::default();
    let mut in_battle = false;
    let mut my_c = String::new();
    let mut bid = String::new();
    let mut out = Dive {
        heroes,
        fights_won: 0,
        level: 1,
        xp: 0,
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
                        // TOTAL earned, not the bar. `xp` is the REMAINDER after a
                        // level-up subtracts what the level cost, so a hero that just
                        // levelled reads as having earned almost nothing — a solo dive
                        // that reached level 2 reported 61 where it had banked 185.
                        for h in v["payload"]["heroes"].as_array().into_iter().flatten() {
                            let lv = h["level"].as_i64().unwrap_or(1) as i32;
                            let banked = meld_run::xp_total_to_level(lv, &balance_ref)
                                + h["xp"].as_i64().unwrap_or(0);
                            out.level = out.level.max(lv);
                            out.xp = out.xp.max(banked);
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
                        // Name the target. An attack with an empty `target_ids` is
                        // REJECTED, and a rejected order is not a fast no-op: the hero
                        // keeps its turn until the 15s auto-act window expires, so a
                        // bot that never named anyone spent the whole budget waiting
                        // and reported "0 fights won" as if the game were unwinnable.
                        let target = v["payload"]["valid_targets"]
                            .as_array()
                            .and_then(|a| a.first())
                            .and_then(|t| t.as_str())
                            .unwrap_or("")
                            .to_string();
                        ws.send(Message::Text(json!({"type":"battle.submit_action","seq":seq,"ts":0,
                            "payload":{"battle_id":bid,"action_id":uuid::Uuid::new_v4().to_string(),
                                       "action":"attack","skill_kind":null,"item_id":null,"target_ids":[target]}
                        }).to_string())).await.unwrap();
                        seq += 1;
                    }
                    // A `validation_error` means this bot is speaking the protocol
                    // wrong; failing here names the cause instead of letting the dive
                    // score zero and look like a balance regression. (`invalid_state`
                    // is expected: the mover sends intents before the run starts.)
                    "session.error" if v["payload"]["code"] == json!("validation_error") => {
                        panic!("server refused a bot action: {}", v["payload"]);
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
    // The budget still SCALES with party size. It no longer has to — a full party's
    // fights are the short ones now — but a bigger party spends real time on more
    // client handshakes and more per-hero turns, and this runs on a box shared with up
    // to twenty other agents. A budget that is merely generous costs nothing; one that
    // is exactly enough is a test that fails on someone else's build.
    let budget = |heroes: usize| Duration::from_secs(45 + 35 * (heroes as u64 - 1));
    let mut runs = Vec::new();
    for heroes in [1usize, 2, 3, 4] {
        runs.push(dive(heroes, budget(heroes)).await);
    }
    for d in &runs {
        println!(
            "  {} hero(es): {} fights won, level {} ({} xp), {:.1}s per fight, wiped={}",
            d.heroes, d.fights_won, d.level, d.xp, d.secs_per_fight, d.wiped
        );
    }

    // The floor: every party size can actually play the game. A size that cannot win a
    // fight inside its budget is a balance bug — this is the check that caught creature
    // attack being multiplied by party size, and it is the one that would catch a solo
    // hero being left unable to kill anything once the world stopped shrinking for them.
    for d in &runs {
        assert!(
            d.fights_won > 0,
            "a party of {} won nothing in {:?} (wiped={})",
            d.heroes,
            budget(d.heroes),
            d.wiped
        );
        assert!(!d.wiped, "a party of {} was wiped out", d.heroes);
    }

    let solo = &runs[0];
    let full = &runs[3];

    // WHAT MUSTERING BUYS: a shorter fight. The creature is the same creature, so four
    // heroes bring four times the damage to the same pool of health. If a full party's
    // fights are not shorter, something is still sizing the world to the party.
    assert!(
        full.secs_per_fight <= solo.secs_per_fight,
        "a full party's fights were no shorter than a lone hero's ({:.1}s vs {:.1}s) — \
         something is still scaling the encounter to the party",
        full.secs_per_fight,
        solo.secs_per_fight
    );

    // AND WHAT IT DOES NOT BUY: a faster ladder. A fixed pool split among the survivors
    // of a proportionally shorter fight is the SAME XP per second at every party size,
    // which is what makes the split fair rather than a tax on bringing friends.
    //
    // A wide band on purpose (see the header). These are real-time bot dives on a shared
    // box; anything tighter than 3x either way is measuring the machine's load.
    let per_sec = |d: &Dive| d.xp as f64 / (d.secs_per_fight * d.fights_won.max(1) as f64).max(0.1);
    let (solo_rate, full_rate) = (per_sec(solo), per_sec(full));
    let ratio = solo_rate / full_rate.max(1e-6);
    assert!(
        (0.33..=3.0).contains(&ratio),
        "XP per second is not flat across party size: {solo_rate:.2}/s solo vs \
         {full_rate:.2}/s at four ({ratio:.2}x). A fixed creature split among the \
         survivors of a shorter fight should pay every size about the same rate"
    );
}
