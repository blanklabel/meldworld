//! Tutorial round 2 server-side regressions:
//! - a guided `[T]` dive fields the real 4 classes the player picked, bypassing
//!   the normal party-slot/unlock clamp (both gates: `clamp_tutorial_party` in
//!   `handle_enter_maze`, and the tutorial branch of `party_size` in `form_run`)
//!   — a normal dive's clamp is untouched.
//! - a guided `[T]` dive always starts with one Town Portal item, so the
//!   walkthrough's own "Go back to town"/"Exit Tutorial" buttons actually work
//!   for a real (fresh) account — previously a real bug: `starting_town_portals`
//!   defaults to 0, so those buttons failed with "No Town Portal item."
//!
//! Requires Postgres (`MELD_DATABASE_URL`; `qa/scripts/local_pg.sh` provides it).

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
    // Deliberately NOT forcing `party_size_per_player` down to 1 here (unlike
    // most other QA harnesses) — these tests are specifically about a tutorial
    // dive fielding up to 4 heroes, so the cap must stay at its real default.
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

async fn http_login(addr: &str, username: &str) -> String {
    let client = reqwest::Client::new();
    let base = format!("http://{addr}");
    let body = json!({ "username": username, "password": "correct-horse-battery" });
    let reg = client.post(format!("{base}/v1/auth/register")).json(&body).send().await.unwrap();
    assert_eq!(reg.status(), 201);
    let login = client.post(format!("{base}/v1/auth/login")).json(&body).send().await.unwrap();
    assert_eq!(login.status(), 200);
    let v: Value = login.json().await.unwrap();
    v["realtime_ticket"].as_str().unwrap().to_string()
}

type Ws =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

struct Bot {
    ws: Ws,
    seq: u32,
}

impl Bot {
    async fn connect(addr: &str, ticket: &str) -> Self {
        let (ws, _) = connect_async(format!("ws://{addr}/v1/realtime")).await.unwrap();
        let mut bot = Bot { ws, seq: 1 };
        bot.send("session.authenticate", json!({ "ticket": ticket, "resume": null })).await;
        bot.recv_type("session.authenticated").await;
        bot
    }

    async fn send(&mut self, msg_type: &str, payload: Value) {
        let env = json!({ "type": msg_type, "seq": self.seq, "ts": 0u64, "payload": payload });
        self.seq += 1;
        self.ws.send(Message::Text(env.to_string())).await.unwrap();
    }

    /// Waits for `msg_type`, up to `secs` seconds. Returns `None` on timeout
    /// (used to prove a message never arrives, not just to read one that does).
    async fn try_recv_type(&mut self, msg_type: &str, secs: u64) -> Option<Value> {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(secs);
        loop {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                return None;
            }
            let Ok(Some(Ok(Message::Text(txt)))) =
                tokio::time::timeout(remaining, self.ws.next()).await
            else {
                return None;
            };
            let v: Value = serde_json::from_str(&txt).unwrap();
            if v["type"] == json!(msg_type) {
                return Some(v);
            }
        }
    }

    async fn recv_type(&mut self, msg_type: &str) -> Value {
        self.try_recv_type(msg_type, 10)
            .await
            .unwrap_or_else(|| panic!("timed out waiting for {msg_type}"))
    }
}

#[tokio::test]
async fn a_tutorial_dive_fields_the_chosen_four_classes() {
    let addr = start_server().await;
    let ticket =
        http_login(&addr, &format!("pbot_{}", &uuid::Uuid::new_v4().simple().to_string()[..8]))
            .await;
    let mut bot = Bot::connect(&addr, &ticket).await;
    bot.send(
        "run.enter_maze",
        json!({ "tutorial": true, "party": ["hunter", "psyker", "resonant", "shifter"] }),
    )
    .await;
    bot.recv_type("run.started").await;
    let party = bot.recv_type("run.party").await;
    let heroes = party["payload"]["heroes"].as_array().expect("heroes array");
    assert_eq!(
        heroes.len(),
        4,
        "a tutorial dive's 4-class pick must not be clamped down to 1 (saw {heroes:?})"
    );
    let classes: Vec<&str> =
        heroes.iter().map(|h| h["class_key"].as_str().unwrap_or("")).collect();
    assert_eq!(classes, vec!["hunter", "psyker", "resonant", "shifter"]);
}

#[tokio::test]
async fn a_tutorial_dive_clamps_a_non_live_class_to_explorer() {
    let addr = start_server().await;
    let ticket =
        http_login(&addr, &format!("pbot_{}", &uuid::Uuid::new_v4().simple().to_string()[..8]))
            .await;
    let mut bot = Bot::connect(&addr, &ticket).await;
    // `dragoon` is a real `CharacterClass` variant but has no kit/perk/unlock
    // wired anywhere (a reserved, non-functional placeholder) — the tutorial's
    // own clamp must still turn it into Explorer, exactly like an unowned class
    // would on a normal dive.
    bot.send(
        "run.enter_maze",
        json!({ "tutorial": true, "party": ["hunter", "dragoon", "resonant", "shifter"] }),
    )
    .await;
    bot.recv_type("run.started").await;
    let party = bot.recv_type("run.party").await;
    let heroes = party["payload"]["heroes"].as_array().expect("heroes array");
    let classes: Vec<&str> =
        heroes.iter().map(|h| h["class_key"].as_str().unwrap_or("")).collect();
    assert_eq!(classes, vec!["hunter", "explorer", "resonant", "shifter"]);
}

#[tokio::test]
async fn a_tutorial_dive_starts_with_a_town_portal() {
    let addr = start_server().await;
    let ticket =
        http_login(&addr, &format!("tpbot_{}", &uuid::Uuid::new_v4().simple().to_string()[..8]))
            .await;
    let mut bot = Bot::connect(&addr, &ticket).await;
    bot.send("run.enter_maze", json!({ "tutorial": true })).await;
    bot.recv_type("run.started").await;
    bot.send("run.begin_extraction", json!({ "method": "town_portal" })).await;
    let started = bot.try_recv_type("run.channel_started", 10).await;
    assert!(
        started.is_some(),
        "a tutorial dive must start with a Town Portal so 'Go back to town'/'Exit \
         Tutorial' actually work — got no run.channel_started"
    );
}

#[tokio::test]
async fn a_normal_dive_still_starts_with_zero_town_portals() {
    let addr = start_server().await;
    let ticket =
        http_login(&addr, &format!("npbot_{}", &uuid::Uuid::new_v4().simple().to_string()[..8]))
            .await;
    let mut bot = Bot::connect(&addr, &ticket).await;
    bot.send("run.enter_maze", json!({ "tutorial": false })).await;
    bot.recv_type("run.started").await;
    bot.send("run.begin_extraction", json!({ "method": "town_portal" })).await;
    let err = bot.recv_type("session.error").await;
    assert!(
        err["payload"]["message"].as_str().unwrap_or("").contains("Town Portal"),
        "a normal dive must still start with zero Town Portals (the tutorial's own \
         grant must not leak into normal play), got {err:?}"
    );
}
