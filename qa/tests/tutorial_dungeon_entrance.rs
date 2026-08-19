//! DG-3-tutorial: a `[T]` guided practice dive gets exactly one forced,
//! hand-placed dungeon entrance (`dungeon-entrance-tutorial`, leading to
//! `guardia_forest`) so the step-by-step walkthrough's "how to enter a
//! dungeon" step has something to find — even though tutorial world-gen
//! otherwise excludes dungeons entirely. A normal (non-tutorial) dive never
//! gets this particular entrance.
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
    let mut balance = meld_balance::Balance::load_default().unwrap();
    balance.battle.party_size_per_player = 1;
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

async fn http_login(addr: &str, username: &str) -> (String, String) {
    let client = reqwest::Client::new();
    let base = format!("http://{addr}");
    let body = json!({ "username": username, "password": "correct-horse-battery" });
    let reg = client.post(format!("{base}/v1/auth/register")).json(&body).send().await.unwrap();
    assert_eq!(reg.status(), 201);
    let login = client.post(format!("{base}/v1/auth/login")).json(&body).send().await.unwrap();
    assert_eq!(login.status(), 200);
    let v: Value = login.json().await.unwrap();
    (
        v["realtime_ticket"].as_str().unwrap().to_string(),
        v["player"]["player_id"].as_str().unwrap().to_string(),
    )
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

    async fn recv_type(&mut self, msg_type: &str) -> Value {
        loop {
            let msg = tokio::time::timeout(Duration::from_secs(10), self.ws.next())
                .await
                .expect("timed out")
                .expect("closed")
                .expect("ws error");
            if let Message::Text(t) = msg {
                let v: Value = serde_json::from_str(&t).unwrap();
                if v["type"] == json!(msg_type) {
                    return v;
                }
            }
        }
    }
}

/// Collects every distinct `entrance:<dungeon>:<bodies>` tag seen across
/// `world.snapshot` messages for up to `secs` seconds, keyed by entity id.
async fn collect_entrances(bot: &mut Bot, secs: u64) -> std::collections::HashMap<String, String> {
    let mut entrances = std::collections::HashMap::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(secs);
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            return entrances;
        }
        let Ok(Some(Ok(Message::Text(txt)))) =
            tokio::time::timeout(remaining, bot.ws.next()).await
        else {
            return entrances;
        };
        let v: Value = serde_json::from_str(&txt).unwrap();
        if v["type"] == json!("world.snapshot") {
            let empty = vec![];
            for e in v["payload"]["entities"].as_array().unwrap_or(&empty) {
                let state = e["avatar_state"].as_str().unwrap_or("");
                if state.starts_with("entrance:") {
                    entrances.insert(
                        e["entity_id"].as_str().unwrap_or_default().to_string(),
                        state.to_string(),
                    );
                }
            }
        }
    }
}

#[tokio::test]
async fn a_tutorial_dive_gets_the_one_forced_entrance() {
    let addr = start_server().await;
    let (ticket, pid) =
        http_login(&addr, &format!("tbot_{}", &uuid::Uuid::new_v4().simple().to_string()[..8]))
            .await;
    let mut bot = Bot::connect(&addr, &ticket).await;
    bot.send("run.enter_maze", json!({ "tutorial": true })).await;
    bot.recv_type("run.started").await;

    let entrances = collect_entrances(&mut bot, 10).await;
    let tutorial: Vec<_> =
        entrances.iter().filter(|(id, _)| id.as_str() == "dungeon-entrance-tutorial").collect();
    assert_eq!(
        tutorial.len(),
        1,
        "a tutorial dive must get exactly one forced entrance (saw {entrances:?}, player {pid})"
    );
    let (_, state) = tutorial[0];
    assert!(
        state.starts_with("entrance:guardia_forest:"),
        "the forced tutorial entrance must lead to guardia_forest (solo-safe), got {state}"
    );
}

#[tokio::test]
async fn a_normal_dive_never_gets_the_tutorial_entrance() {
    let addr = start_server().await;
    let (ticket, _pid) = http_login(
        &addr,
        &format!("nbot_{}", &uuid::Uuid::new_v4().simple().to_string()[..8]),
    )
    .await;
    let mut bot = Bot::connect(&addr, &ticket).await;
    bot.send("run.enter_maze", json!({ "tutorial": false })).await;
    bot.recv_type("run.started").await;

    let entrances = collect_entrances(&mut bot, 5).await;
    assert!(
        !entrances.contains_key("dungeon-entrance-tutorial"),
        "a normal dive must never get the hand-placed tutorial entrance (saw {entrances:?})"
    );
}
