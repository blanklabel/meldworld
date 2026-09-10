//! **SC-3 — two worlds are two places.** A world's identity is its SEED (CANON §W1), so
//! `run.enter_maze { seed }` names which one you dive into. This drives real bot clients
//! over the real wire and asserts the three things that make that true rather than
//! decorative:
//!
//! 1. **You land in the world you asked for** — `run.started.world_seed` echoes the
//!    request. It is the world's own fact, so a client that displayed what it *asked*
//!    for would look identical while being wrong; only the server's answer proves it.
//! 2. **Different seeds cannot see each other.** Every avatar rides the snapshot as an
//!    entity keyed by `player_id`, and both divers start at their own world's origin —
//!    so if the shard were a fiction they would be standing on top of each other and
//!    each other's ids would appear immediately. Nothing is a stronger tell.
//! 3. **The same seed IS the same place.** The isolation half passes trivially if
//!    `seed` simply did nothing and every dive got a private world, so the positive
//!    case has to be asserted beside it: two divers naming one seed must meet.
//!
//! Requires Postgres: set `MELD_DATABASE_URL`.

use std::collections::HashSet;
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
    tokio::spawn(async move { axum::serve(listener, built.router).await.unwrap() });
    format!("{addr}")
}

async fn login(addr: &str, username: &str) -> (String, String) {
    let http = reqwest::Client::new();
    let base = format!("http://{addr}");
    let body = json!({ "username": username, "password": "correct-horse-battery" });
    http.post(format!("{base}/v1/auth/register")).json(&body).send().await.unwrap();
    let v: Value = http
        .post(format!("{base}/v1/auth/login"))
        .json(&body)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    (
        v["realtime_ticket"].as_str().unwrap().to_string(),
        v["player"]["player_id"].as_str().unwrap().to_string(),
    )
}

struct Report {
    player_id: String,
    /// The seed the SERVER says this diver is standing in.
    landed_seed: u64,
    /// Every other player id this diver ever saw in a snapshot.
    saw: HashSet<String>,
}

/// Dive into `seed`, hold still, and record who shows up. Deliberately does NOT walk:
/// this measures whether two divers share a world, and both start at their world's
/// origin — moving would only let them wander apart and weaken the signal.
async fn run_bot(addr: String, username: String, seed: u64, watch_for: Duration) -> Report {
    let (ticket, player_id) = login(&addr, &username).await;
    let (mut ws, _) = connect_async(format!("ws://{addr}/v1/realtime")).await.unwrap();
    let mut seq = 1u32;
    ws.send(Message::Text(
        json!({"type":"session.authenticate","seq":seq,"ts":0,"payload":{"ticket":ticket,"resume":null}})
            .to_string(),
    ))
    .await
    .unwrap();
    seq += 1;

    let mut landed_seed = 0u64;
    let mut saw: HashSet<String> = HashSet::new();
    let mut started = false;
    // Watch for a fixed window AFTER the dive lands, so both bots are certainly in
    // their worlds at the same time — a race here would make isolation pass for the
    // wrong reason (the other diver simply had not arrived yet).
    let hard_deadline = tokio::time::Instant::now() + Duration::from_secs(60);
    let mut watch_until = None;

    loop {
        if let Some(until) = watch_until {
            if tokio::time::Instant::now() >= until {
                break;
            }
        }
        assert!(tokio::time::Instant::now() < hard_deadline, "{username}: timed out");
        let msg = tokio::time::timeout(Duration::from_secs(5), ws.next()).await;
        let Ok(Some(Ok(Message::Text(t)))) = msg else { continue };
        let v: Value = serde_json::from_str(&t).unwrap();
        match v["type"].as_str().unwrap_or("") {
            "session.authenticated" => {
                ws.send(Message::Text(
                    json!({"type":"run.enter_maze","seq":seq,"ts":0,
                           "payload":{"solo":true,"seed":seed}})
                    .to_string(),
                ))
                .await
                .unwrap();
                seq += 1;
            }
            "run.started" => {
                landed_seed = v["payload"]["world_seed"].as_u64().unwrap_or(0);
                started = true;
                watch_until = Some(tokio::time::Instant::now() + watch_for);
            }
            "world.snapshot" if started => {
                if let Some(entities) = v["payload"]["entities"].as_array() {
                    for e in entities {
                        let Some(id) = e["entity_id"].as_str() else { continue };
                        // Only other AVATARS matter: a creature's id is not a player's.
                        if id != player_id && e["avatar_state"].as_str() == Some("active") {
                            saw.insert(id.to_string());
                        }
                    }
                }
            }
            _ => {}
        }
    }
    Report { player_id, landed_seed, saw }
}

/// Two divers naming two seeds are in two worlds, and each is told which.
#[tokio::test]
async fn two_seeds_are_two_worlds() {
    let addr = start_server().await;
    let suffix = uuid::Uuid::new_v4().simple().to_string();
    let watch = Duration::from_secs(6);

    let a = tokio::spawn(run_bot(addr.clone(), format!("wa_{}", &suffix[..10]), 111_111, watch));
    let b = tokio::spawn(run_bot(addr.clone(), format!("wb_{}", &suffix[10..20]), 222_222, watch));
    let (ra, rb) = (a.await.unwrap(), b.await.unwrap());

    assert_eq!(ra.landed_seed, 111_111, "diver A should land in the world it asked for");
    assert_eq!(rb.landed_seed, 222_222, "diver B should land in the world it asked for");
    assert!(
        !ra.saw.contains(&rb.player_id),
        "A (seed 111111) saw B (seed 222222) — the worlds are not isolated"
    );
    assert!(
        !rb.saw.contains(&ra.player_id),
        "B (seed 222222) saw A (seed 111111) — the worlds are not isolated"
    );
}

/// …and the other half: one seed is ONE place. Without this, a `seed` field that was
/// silently ignored in favour of a private world per diver would pass the test above.
#[tokio::test]
async fn one_seed_is_one_world() {
    let addr = start_server().await;
    let suffix = uuid::Uuid::new_v4().simple().to_string();
    let watch = Duration::from_secs(8);
    let shared = 424_242;

    let a = tokio::spawn(run_bot(addr.clone(), format!("sa_{}", &suffix[..10]), shared, watch));
    let b = tokio::spawn(run_bot(addr.clone(), format!("sb_{}", &suffix[10..20]), shared, watch));
    let (ra, rb) = (a.await.unwrap(), b.await.unwrap());

    assert_eq!(ra.landed_seed, shared);
    assert_eq!(rb.landed_seed, shared);
    assert!(
        ra.saw.contains(&rb.player_id) || rb.saw.contains(&ra.player_id),
        "two divers naming seed {shared} never saw each other — the seed is not naming a place"
    );
}
