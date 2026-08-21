//! Field-station conformance (roadmap `MS-1`): a smith who carries ore can raise a
//! forge out in the maze, and then work a piece at it — over the real wire, with the
//! real game loop deciding all of it. The claims this carries:
//!
//! 1. Raising one is **refused** without the ore, in words that say what it wants.
//! 2. Ore **carried out of town** (a Vault withdrawal riding into the run) pays for it,
//!    and the backpack is debited.
//! 3. The station appears in the world as `station:smith:<jobs>` for the whole instance.
//! 4. Work done at it lands on the requester's OWN gear — the reply names what changed —
//!    and the station's jobs count down.
//! 5. **Ownership never moves**: the piece is still in the same Vault afterwards.
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
    let balance = Arc::new(balance);
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

/// Register + log in a fresh account, returning `(token, ticket, player_id)`.
async fn account(http: &reqwest::Client, base: &str, prefix: &str) -> (String, String, String) {
    let username = format!("{prefix}{}", &uuid::Uuid::new_v4().simple().to_string()[..10]);
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

/// A forge is built from ore you are CARRYING, so a smith who walked out empty-handed
/// is refused — in words that say what it wants, not a silent no-op.
#[tokio::test]
async fn a_field_forge_needs_ore_in_hand() {
    let addr = start_server().await;
    let http = reqwest::Client::new();
    let base = format!("http://{addr}");
    let (_token, ticket, _pid) = account(&http, &base, "sr_").await;

    let (mut ws, _) = connect_async(format!("ws://{addr}/v1/realtime")).await.unwrap();
    let mut seq = 1u32;
    macro_rules! send {
        ($t:expr, $p:tt) => {{
            ws.send(Message::Text(
                json!({"type":$t,"seq":seq,"ts":0,"payload":$p}).to_string(),
            ))
            .await
            .unwrap();
            seq += 1;
        }};
    }
    ws.send(Message::Text(
        json!({"type":"session.authenticate","seq":seq,"ts":0,"payload":{"ticket":ticket,"resume":null}}).to_string(),
    ))
    .await
    .unwrap();
    seq += 1;

    let deadline = tokio::time::Instant::now() + Duration::from_secs(45);
    loop {
        assert!(tokio::time::Instant::now() < deadline, "no answer to a bare build");
        let Some(Ok(Message::Text(t))) = ws.next().await else { panic!("ws closed") };
        let v: Value = serde_json::from_str(&t).unwrap();
        match v["type"].as_str().unwrap_or("") {
            "session.authenticated" => send!("run.enter_maze", {"tutorial": true}),
            "run.started" => send!("run.build_station", {"kind": "smith"}),
            "session.error" => {
                let msg = v["payload"]["message"].as_str().unwrap_or_default();
                // Any of the three gates is a legitimate answer for a fresh account: no
                // Smithwright in the party, no Forging level, no ore. What matters is that
                // the refusal NAMES what is missing. The party gate is the one it hits
                // first now — the menu used to offer "Set up a smith station" to a party
                // with no smith in it, and the server took it.
                assert!(
                    msg.contains("ore")
                        || msg.contains("Forging")
                        || msg.contains("Smithwright"),
                    "a refusal has to say what a forge wants: {msg}"
                );
                assert!(!msg.is_empty());
                return;
            }
            "run.smith_result" => panic!("built a forge out of nothing: {v}"),
            _ => {}
        }
    }
}

/// The whole errand: carry ore out of town, raise a forge with it, and have a piece
/// worked at it — over the real wire, with the game loop deciding all of it.
#[tokio::test]
async fn a_smith_raises_a_forge_in_the_field_and_works_a_piece_at_it() {
    let addr = start_server().await;
    let http = reqwest::Client::new();
    let base = format!("http://{addr}");
    let (token, ticket, player_id) = account(&http, &base, "st_").await;
    let pid = uuid::Uuid::parse_str(&player_id).unwrap();

    // A smith with the skill, the stock and the chits — and ore withdrawn to carry out,
    // which is exactly how a field smith supplies themselves.
    let db_url = std::env::var("MELD_DATABASE_URL").unwrap();
    let balance = meld_balance::Balance::load_default().unwrap();
    let db = meld_db::Db::connect(&db_url, balance.auth.bcrypt_cost).await.unwrap();
    db.bank_extraction(pid, &[("dune_iron".into(), 20), ("dune_ingot".into(), 20)], 100_000)
        .await
        .unwrap();
    db.add_skill_xp(pid, "forging", 100_000).await.unwrap();
    // A forge is a Smithwright's bench, so the account has to be able to FIELD one — the
    // skill gate says how good the work is, the class gate says whose bench it is.
    db.grant_unlocks(pid, &["class_smithwright".to_string()]).await.unwrap();
    assert_eq!(
        http.post(format!("{base}/v1/vault/materials/dune_iron/withdraw"))
            .bearer_auth(&token)
            .json(&json!({ "quantity": 10 }))
            .send()
            .await
            .unwrap()
            .status(),
        200
    );

    let gear: Value = http
        .get(format!("{base}/v1/vault/gear"))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let my_gear = gear["data"][0]["gear_id"].as_str().expect("starter gear").to_string();

    let (mut ws, _) = connect_async(format!("ws://{addr}/v1/realtime")).await.unwrap();
    let mut seq = 1u32;
    macro_rules! send {
        ($t:expr, $p:tt) => {{
            ws.send(Message::Text(
                json!({"type":$t,"seq":seq,"ts":0,"payload":$p}).to_string(),
            ))
            .await
            .unwrap();
            seq += 1;
        }};
    }
    ws.send(Message::Text(
        json!({"type":"session.authenticate","seq":seq,"ts":0,"payload":{"ticket":ticket,"resume":null}}).to_string(),
    ))
    .await
    .unwrap();
    seq += 1;

    let mut ore_removed = false;
    let mut asked = false;
    let mut station_id: Option<String> = None;
    let mut station_jobs_seen: Option<i64> = None;
    let mut result: Option<Value> = None;
    let mut heat_strikes = 0i64;
    let mut saw_build_channel = false;

    let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
    while result.is_none() {
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out (ore_removed={ore_removed}, station={station_id:?})"
        );
        let Some(Ok(Message::Text(t))) = ws.next().await else { panic!("ws closed") };
        let v: Value = serde_json::from_str(&t).unwrap();
        match v["type"].as_str().unwrap_or("") {
            // A forge is a Smithwright's bench, so field one.
            "session.authenticated" => {
                send!("run.enter_maze", {"tutorial": true, "party": ["smithwright"]})
            }
            "run.started" => send!("run.build_station", {"kind": "smith"}),
            "session.error" => panic!("refused: {v}"),
            // Raising a bench TAKES TIME: the station must not exist until the channel
            // completes, so the build announces itself like every other channel.
            "run.channel_started" => {
                let m = v["payload"]["method"].as_str().unwrap_or_default().to_string();
                if m.starts_with("build:") {
                    saw_build_channel = true;
                    assert!(
                        v["payload"]["fill_ms"].as_u64().unwrap_or(0) > 0,
                        "a channel the client draws a bar for needs a fill: {v}"
                    );
                }
            }
            "run.backpack_update" => {
                // The ore it was built from leaves the backpack, named and itemised.
                let changes = v["payload"]["changes"].as_array().cloned().unwrap_or_default();
                if changes.iter().any(|c| {
                    c["delta"] == json!("removed")
                        && c["cause"] == json!("station")
                        && c["item"]["item_kind"] == json!("dune_iron")
                }) {
                    ore_removed = true;
                }
            }
            "world.snapshot" if ore_removed && !asked => {
                let ents = v["payload"]["entities"].as_array().cloned().unwrap_or_default();
                if let Some(st) = ents.iter().find(|e| {
                    e["avatar_state"]
                        .as_str()
                        .is_some_and(|s| s.starts_with("station:smith:"))
                }) {
                    let tag = st["avatar_state"].as_str().unwrap().to_string();
                    station_jobs_seen = tag.rsplit(':').next().and_then(|n| n.parse::<i64>().ok());
                    let id = st["entity_id"].as_str().unwrap().to_string();
                    station_id = Some(id.clone());
                    asked = true;
                    send!("run.smith_request", {
                        "entity_id": id,
                        "gear_id": my_gear,
                        "service": "reroll",
                        "material": "dune_ingot"
                    });
                }
            }
            // Smithing is a HEAT now: the server hands over a bar of red with a yellow
            // band per blow, and the smith strikes. A bot can hit the middle of every
            // band, which is what a player with a good ear does — so this run should
            // grade as flawless.
            "run.tempo_started" => {
                let p = &v["payload"];
                let job_id = p["job_id"].as_str().unwrap().to_string();
                heat_strikes = p["strikes"].as_i64().unwrap_or(0);
                let bands = p["bands"].as_array().cloned().unwrap_or_default();
                assert!(heat_strikes > 0, "a heat with no blows is not a heat: {p}");
                assert_eq!(bands.len() as i64, heat_strikes, "one band per blow: {p}");
                assert!(p["sweep_ms"].as_i64().unwrap_or(0) > 0, "{p}");
                for b in &bands {
                    let (lo, hi) = (b["lo"].as_f64().unwrap(), b["hi"].as_f64().unwrap());
                    assert!(lo >= 0.0 && hi <= 1.0 && hi > lo, "band off the bar: {b}");
                    let at = (lo + hi) / 2.0;
                    send!("run.strike", {"job_id": job_id, "at": at});
                }
            }
            "run.smith_result" => result = Some(v["payload"].clone()),
            _ => {}
        }
    }

    assert!(ore_removed, "the station should have been paid for out of the backpack");
    assert!(saw_build_channel, "raising a bench should open a channel, not happen instantly");
    let station_id = station_id.expect("the station is in the world");
    assert!(station_id.starts_with("station-smith"), "{station_id}");
    assert_eq!(
        station_jobs_seen,
        Some(balance.forge.station_uses as i64),
        "a fresh station advertises its full run of jobs"
    );

    let result = result.expect("the smith answered");
    assert_eq!(result["ok"], json!(true), "the work should have happened: {result}");
    assert_eq!(result["gear_id"], json!(my_gear), "it worked the piece we asked about");
    assert!(
        result["message"].as_str().unwrap_or_default().contains("re-drew"),
        "the reply should say what changed: {result}"
    );
    // Every blow landed on yellow, so the heat is flawless — and a flawless heat is what
    // buys the epic affix pool, the same reach a trophy catalyst buys.
    assert!(heat_strikes > 0, "no heat was ever opened");
    assert_eq!(result["quality"].as_f64(), Some(1.0), "a clean run should grade 1.0: {result}");
    assert!(
        result["message"].as_str().unwrap_or_default().contains("epic"),
        "a flawless heat should reach the epic pool: {result}"
    );
    assert_eq!(
        result["uses_left"].as_i64(),
        Some(balance.forge.station_uses as i64 - 1),
        "a job done is a job spent: {result}"
    );

    // Ownership never moves: the piece is still in the same Vault, same id.
    let after: Value = http
        .get(format!("{base}/v1/vault/gear"))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(
        after["data"].as_array().unwrap().iter().any(|g| g["gear_id"] == json!(my_gear)),
        "the piece left its owner's Vault"
    );
}

/// The Keeper's half of the same idea: a still raised from reagents you carry, and a
/// brew that is a COOK — graded like a smith's heat, and a good cook feeds more people
/// from the same reagents.
#[tokio::test]
async fn a_keeper_raises_a_still_and_a_good_cook_yields_more() {
    let addr = start_server().await;
    let http = reqwest::Client::new();
    let base = format!("http://{addr}");
    let (token, ticket, player_id) = account(&http, &base, "kp_").await;
    let pid = uuid::Uuid::parse_str(&player_id).unwrap();

    let db_url = std::env::var("MELD_DATABASE_URL").unwrap();
    let balance = meld_balance::Balance::load_default().unwrap();
    let db = meld_db::Db::connect(&db_url, balance.auth.bcrypt_cost).await.unwrap();
    // Reagents to build the still with AND to brew from, plus the Alchemy to do both.
    db.bank_extraction(pid, &[("bloom_herb".into(), 40)], 10_000).await.unwrap();
    db.add_skill_xp(pid, "alchemy", 100_000).await.unwrap();
    // Likewise the still is a Keeper's.
    db.grant_unlocks(pid, &["class_keeper".to_string()]).await.unwrap();
    assert_eq!(
        http.post(format!("{base}/v1/vault/materials/bloom_herb/withdraw"))
            .bearer_auth(&token)
            .json(&json!({ "quantity": 20 }))
            .send()
            .await
            .unwrap()
            .status(),
        200
    );

    let (mut ws, _) = connect_async(format!("ws://{addr}/v1/realtime")).await.unwrap();
    let mut seq = 1u32;
    macro_rules! send {
        ($t:expr, $p:tt) => {{
            ws.send(Message::Text(
                json!({"type":$t,"seq":seq,"ts":0,"payload":$p}).to_string(),
            ))
            .await
            .unwrap();
            seq += 1;
        }};
    }
    ws.send(Message::Text(
        json!({"type":"session.authenticate","seq":seq,"ts":0,"payload":{"ticket":ticket,"resume":null}}).to_string(),
    ))
    .await
    .unwrap();
    seq += 1;

    let mut built = false;
    let mut asked = false;
    let mut jobs_seen: Option<i64> = None;
    let mut result: Option<Value> = None;

    let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
    while result.is_none() {
        assert!(tokio::time::Instant::now() < deadline, "timed out (built={built})");
        let Some(Ok(Message::Text(t))) = ws.next().await else { panic!("ws closed") };
        let v: Value = serde_json::from_str(&t).unwrap();
        match v["type"].as_str().unwrap_or("") {
            // A still is a Keeper's bench, so field one.
            "session.authenticated" => {
                send!("run.enter_maze", {"tutorial": true, "party": ["keeper"]})
            }
            "run.started" => send!("run.build_station", {"kind": "alembic"}),
            "session.error" => panic!("refused: {v}"),
            "run.backpack_update" => {
                let changes = v["payload"]["changes"].as_array().cloned().unwrap_or_default();
                if changes.iter().any(|c| {
                    c["delta"] == json!("removed") && c["cause"] == json!("station")
                }) {
                    built = true;
                }
            }
            "world.snapshot" if built && !asked => {
                let ents = v["payload"]["entities"].as_array().cloned().unwrap_or_default();
                if let Some(st) = ents.iter().find(|e| {
                    e["avatar_state"]
                        .as_str()
                        .is_some_and(|s| s.starts_with("station:alembic:"))
                }) {
                    let tag = st["avatar_state"].as_str().unwrap();
                    jobs_seen = tag.rsplit(':').next().and_then(|n| n.parse::<i64>().ok());
                    asked = true;
                    send!("run.smith_request", {
                        "entity_id": st["entity_id"].as_str().unwrap(),
                        "gear_id": "",
                        "service": "brew",
                        "material": "",
                        "recipe": "bloom_salve"
                    });
                }
            }
            // A brew is a cook: the same bar, at the RECIPE's difficulty. Hit every band.
            "run.tempo_started" => {
                let p = &v["payload"];
                assert_eq!(p["service"], json!("brew"), "{p}");
                let job_id = p["job_id"].as_str().unwrap().to_string();
                for b in p["bands"].as_array().cloned().unwrap_or_default() {
                    let at = (b["lo"].as_f64().unwrap() + b["hi"].as_f64().unwrap()) / 2.0;
                    send!("run.strike", {"job_id": job_id, "at": at});
                }
            }
            "run.smith_result" => result = Some(v["payload"].clone()),
            _ => {}
        }
    }

    assert!(built, "the still should have been paid for out of the backpack");
    assert_eq!(
        jobs_seen,
        Some(balance.forge.station_uses as i64),
        "a fresh still advertises its full run of brews"
    );
    let result = result.expect("the Keeper answered");
    assert_eq!(result["ok"], json!(true), "{result}");
    assert_eq!(result["quality"].as_f64(), Some(1.0), "a clean cook grades 1.0: {result}");
    let msg = result["message"].as_str().unwrap_or_default();
    assert!(msg.contains("brewed"), "{msg}");
    // A flawless cook yields the recipe's doses PLUS the bonus — the whole point of
    // grading a cook rather than just charging for one.
    let bonus = balance.tempo.bonus_doses(1.0);
    assert!(bonus > 0, "a flawless cook should be worth something");
    assert!(msg.contains(&format!("+{bonus} dose")), "{msg}");

    // And the doses are in the Vault, where the requester's stock always was.
    let vault: Value = http
        .get(format!("{base}/v1/vault"))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let salves = vault["materials"]
        .as_array()
        .into_iter()
        .flatten()
        .chain(vault["pending"].as_array().into_iter().flatten())
        .find(|m| m["item_kind"] == json!("bloom_salve"))
        .and_then(|m| m["quantity"].as_i64())
        .unwrap_or(0);
    assert!(salves > bonus as i64, "the doses should be banked: {vault}");
}
