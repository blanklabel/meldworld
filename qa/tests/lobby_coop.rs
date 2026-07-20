//! Co-op lobby end-to-end: two bots form a pre-maze lobby (create + join by code),
//! ready up, the host starts, and both land in ONE shared MazeInstance with a
//! two-member party. Proves the `lobby.*` flow and that `lobby.start` reuses the
//! same run-formation as `run.enter_maze`.
//!
//! Requires Postgres: set `MELD_DATABASE_URL` (see qa/scripts/local_pg.sh).

use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio::net::TcpListener;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;

async fn start_server() -> String {
    let db_url = std::env::var("MELD_DATABASE_URL")
        .expect("set MELD_DATABASE_URL (see qa/scripts/local_pg.sh)");
    let balance = std::sync::Arc::new(meld_balance::Balance::load_default().unwrap());
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

async fn http_login(addr: &str, username: &str) -> String {
    let client = reqwest::Client::new();
    let base = format!("http://{addr}");
    let body = json!({ "username": username, "password": "correct-horse-battery" });
    let reg = client.post(format!("{base}/v1/auth/register")).json(&body).send().await.unwrap();
    assert_eq!(reg.status(), 201);
    let login = client.post(format!("{base}/v1/auth/login")).json(&body).send().await.unwrap();
    let v: Value = login.json().await.unwrap();
    v["realtime_ticket"].as_str().unwrap().to_string()
}

type Ws = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

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
                .expect("stream closed")
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

#[tokio::test]
async fn two_players_form_a_lobby_and_dive_together() {
    let addr = start_server().await;
    let ta = http_login(&addr, &format!("host_{}", &uuid::Uuid::new_v4().simple().to_string()[..8])).await;
    let tb = http_login(&addr, &format!("guest_{}", &uuid::Uuid::new_v4().simple().to_string()[..8])).await;
    let mut host = Bot::connect(&addr, &ta).await;
    let mut guest = Bot::connect(&addr, &tb).await;

    // Host creates a lobby with a Resonant lead; reads back the join code.
    host.send("lobby.create", json!({ "party": ["resonant", "psyker", "hunter", "hunter"] })).await;
    let state = host.recv_type("lobby.state").await;
    let code = state["payload"]["code"].as_str().unwrap().to_string();
    assert_eq!(state["payload"]["members"].as_array().unwrap().len(), 1);

    // Guest joins by code with a Psyker lead; both now see a 2-member lobby.
    guest.send("lobby.join", json!({ "code": code, "party": ["psyker", "psyker", "resonant", "hunter"] })).await;
    let gstate = guest.recv_type("lobby.state").await;
    assert_eq!(gstate["payload"]["members"].as_array().unwrap().len(), 2);

    // Everyone readies up.
    host.send("lobby.ready", json!({ "ready": true })).await;
    guest.send("lobby.ready", json!({ "ready": true })).await;
    // Drain to the state where both are ready (host sees guest's ready broadcast).
    let mut both_ready = false;
    for _ in 0..8 {
        let s = host.recv_type("lobby.state").await;
        let members = s["payload"]["members"].as_array().unwrap();
        if members.len() == 2 && members.iter().all(|m| m["ready"] == json!(true)) {
            both_ready = true;
            break;
        }
    }
    assert!(both_ready, "both members should be ready");

    // Host starts → both dive into ONE instance with a 2-member party.
    host.send("lobby.start", json!({})).await;
    let hs = host.recv_type("run.started").await;
    let gs = guest.recv_type("run.started").await;
    let hi = hs["payload"]["instance_id"].as_str().unwrap();
    let gi = gs["payload"]["instance_id"].as_str().unwrap();
    assert_eq!(hi, gi, "both players share one instance");
    assert_eq!(
        hs["payload"]["members"].as_array().unwrap().len(),
        2,
        "the run has both party members"
    );
}
