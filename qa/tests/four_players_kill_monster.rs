//! End-to-end conformance (BUILD-PLAN T6): four headless bot clients register +
//! log in over HTTP, authenticate the realtime socket, enter one MazeInstance,
//! walk into the monster, and cooperatively kill it via `battle.submit_action` —
//! no client-side combat math. Asserts a `battle.ended` with `outcome: victory`.
//!
//! Requires Postgres: set `MELD_DATABASE_URL`. `qa/scripts/local_pg.sh` boots a
//! throwaway instance and runs this for you.

use std::sync::Arc;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio::net::TcpListener;
use tokio::sync::Barrier;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;

const PARTY: usize = 4;

/// Boot the real server on an ephemeral port; return `host:port`.
async fn start_server() -> String {
    let db_url = std::env::var("MELD_DATABASE_URL")
        .expect("set MELD_DATABASE_URL (see qa/scripts/local_pg.sh)");
    // One hero per player: four separate players each start solo and OPT INTO the
    // same fight (no auto-pull), so 4 × 1 = 4 combatants — within the merge cap
    // (PARTY_MAX × merge_cap = 8). Each bot commands its own hero.
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

/// Register + login over HTTP; returns (realtime_ticket, player_id).
async fn http_login(addr: &str, username: &str) -> (String, String) {
    let client = reqwest::Client::new();
    let base = format!("http://{addr}");
    let body = json!({ "username": username, "password": "correct-horse-battery" });

    let reg = client
        .post(format!("{base}/v1/auth/register"))
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(reg.status(), 201, "register should 201");

    let login = client
        .post(format!("{base}/v1/auth/login"))
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(login.status(), 200, "login should 200");
    let v: Value = login.json().await.unwrap();
    (
        v["realtime_ticket"].as_str().unwrap().to_string(),
        v["player"]["player_id"].as_str().unwrap().to_string(),
    )
}

type Ws =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

/// A scripted bot session: greedily walks east into the monster, then attacks on
/// every turn-ready until the party wins.
struct Bot {
    ws: Ws,
    seq: u32,
    input_seq: u32,
    my_combatant: Option<String>,
    monster_combatant: Option<String>,
    battle_id: String,
    in_battle: bool,
    members_at_start: usize,
}

impl Bot {
    async fn connect(addr: &str, ticket: &str) -> Self {
        let (ws, _) = connect_async(format!("ws://{addr}/v1/realtime"))
            .await
            .unwrap();
        let mut bot = Bot {
            ws,
            seq: 1,
            input_seq: 0,
            my_combatant: None,
            monster_combatant: None,
            battle_id: String::new(),
            in_battle: false,
            members_at_start: 0,
        };
        // seq 1: authenticate.
        bot.send(
            "session.authenticate",
            json!({ "ticket": ticket, "resume": null }),
        )
        .await;
        let authed = bot.recv_type("session.authenticated").await;
        assert_eq!(authed["payload"]["resumed"], json!(false));
        bot
    }

    async fn send(&mut self, msg_type: &str, payload: Value) {
        let env = json!({ "type": msg_type, "seq": self.seq, "ts": 0u64, "payload": payload });
        self.seq += 1;
        self.ws.send(Message::Text(env.to_string())).await.unwrap();
    }

    /// Read frames until one of `msg_type` arrives; returns the full envelope.
    async fn recv_type(&mut self, msg_type: &str) -> Value {
        loop {
            let msg = tokio::time::timeout(Duration::from_secs(10), self.ws.next())
                .await
                .expect("timed out waiting for message")
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

    async fn enter_maze(&mut self) {
        self.send("run.enter_maze", json!({})).await;
    }

    async fn move_east(&mut self) {
        self.input_seq += 1;
        self.send(
            "movement.move_intent",
            json!({
                "input_seq": self.input_seq,
                "move_dir": { "x": 1.0, "y": 0.0 },
                "client_pos": { "x": 0.0, "y": 0.0 }
            }),
        )
        .await;
    }

    /// Opt into the nearby ongoing fight. Harmless (server-rejected) when there's
    /// no battle yet or we're too far / already in it.
    async fn try_join(&mut self) {
        self.send("run.join_battle", json!({})).await;
    }

    async fn attack_monster(&mut self) {
        let battle_id = self.battle_id.clone();
        let target = self.monster_combatant.clone().unwrap();
        self.send(
            "battle.submit_action",
            json!({
                "battle_id": battle_id,
                "action_id": uuid::Uuid::now_v7().to_string(),
                "action": "attack",
                "skill_kind": null,
                "item_id": null,
                "target_ids": [target]
            }),
        )
        .await;
    }

    /// Drive to victory. Only `leader` sends `enter_maze`.
    async fn play(mut self, leader: bool, barrier: Arc<Barrier>) -> Outcome {
        // Wait for everyone to be authenticated, then the leader starts the run.
        barrier.wait().await;
        if leader {
            // Give the loop a beat to register all Connected events so the party
            // forms with the full roster.
            tokio::time::sleep(Duration::from_millis(400)).await;
            self.enter_maze().await;
        }

        let mut move_timer = tokio::time::interval(Duration::from_millis(80));
        let deadline = tokio::time::Instant::now() + Duration::from_secs(60);

        loop {
            if tokio::time::Instant::now() > deadline {
                return Outcome::Timeout;
            }
            tokio::select! {
                _ = move_timer.tick(), if !self.in_battle => {
                    // Walk east toward the monster; the first to touch it starts the
                    // fight, and everyone else opts in (players are no longer
                    // auto-pulled into each other's battles).
                    self.move_east().await;
                    self.try_join().await;
                }
                msg = self.ws.next() => {
                    let Some(Ok(Message::Text(t))) = msg else { return Outcome::Closed };
                    let v: Value = serde_json::from_str(&t).unwrap();
                    match v["type"].as_str().unwrap_or("") {
                        "run.started" => {
                            self.members_at_start = v["payload"]["members"].as_array().map(|a| a.len()).unwrap_or(0);
                        }
                        "battle.started" => {
                            self.in_battle = true;
                            self.my_combatant = v["payload"]["your_combatant_id"].as_str().map(String::from);
                            self.battle_id = v["payload"]["battle_id"].as_str().unwrap_or("").to_string();
                            self.monster_combatant = v["payload"]["enemies"][0]["combatant_id"].as_str().map(String::from);
                        }
                        "battle.turn_ready"
                            if v["payload"]["combatant_id"].as_str().map(String::from) == self.my_combatant => {
                                self.attack_monster().await;
                            }
                        "battle.ended" => {
                            return match v["payload"]["outcome"].as_str() {
                                Some("victory") => Outcome::Victory { members: self.members_at_start },
                                Some("defeat") => Outcome::Defeat,
                                _ => Outcome::Fled,
                            };
                        }
                        _ => {}
                    }
                }
            }
        }
    }
}

#[derive(Debug, PartialEq)]
enum Outcome {
    Victory { members: usize },
    Defeat,
    Fled,
    Timeout,
    Closed,
}

#[tokio::test]
async fn four_players_join_and_kill_the_monster() {
    let addr = start_server().await;

    // Register + login + authenticate all four before anyone enters the maze.
    let barrier = Arc::new(Barrier::new(PARTY));
    let mut handles = Vec::new();
    for i in 0..PARTY {
        let addr = addr.clone();
        let barrier = barrier.clone();
        handles.push(tokio::spawn(async move {
            let username = format!("bot{i}_{}", &uuid::Uuid::new_v4().simple().to_string()[..8]);
            let (ticket, _pid) = http_login(&addr, &username).await;
            let bot = Bot::connect(&addr, &ticket).await;
            bot.play(i == 0, barrier).await
        }));
    }

    let mut victories = 0;
    let mut roster = 0;
    for h in handles {
        match h.await.unwrap() {
            Outcome::Victory { members } => {
                victories += 1;
                roster = roster.max(members);
            }
            other => panic!("a bot did not win: {other:?}"),
        }
    }
    assert_eq!(victories, PARTY, "all four bots should observe victory");
    assert_eq!(
        roster, PARTY,
        "the party should have formed with all four members"
    );
}
