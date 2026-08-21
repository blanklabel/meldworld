//! The two profession classes (roadmap `MS-1`): a Smithwright and a Keeper are real
//! heroes you dive with, not a label. Over the real wire:
//!
//! 1. They are **earned** — a fresh account cannot field one, and the unlock says how.
//! 2. Once unlocked, a party of them dives and the roster reports their classes, their
//!    ranks and their own stat spread (a Keeper leads on Mnd, a Smithwright on Str).
//! 3. Their kits are on the battle menu at the right levels, from the one registry.
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
    };
    let built = meld_server::build(&config).await.expect("server builds");
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, built.router).await.unwrap();
    });
    format!("{addr}")
}

#[tokio::test]
async fn a_smithwright_and_a_keeper_are_earned_and_then_fieldable() {
    // Earned first: the unlock rows exist, say how to get them, and wait on a seat.
    for (key, class) in [
        ("class_smithwright", "Smithwright"),
        ("class_keeper", "Keeper"),
    ] {
        let def = meld_proto::unlocks::unlock(key).expect("the unlock exists");
        assert_eq!(def.name, class);
        assert!(!def.trigger_text.is_empty(), "{key} must say how to earn it");
        assert!(def.requires.is_some(), "a class you cannot seat is not a reward");
    }
    // A fresh account owns neither, so neither is fieldable yet.
    let start = meld_proto::unlocks::starting_unlocks()
        .iter()
        .map(|s| s.to_string())
        .collect::<Vec<_>>();
    let owned = meld_proto::unlocks::owned_classes(&start);
    assert!(!owned.contains(&meld_proto::enums::CharacterClass::Smithwright));
    assert!(!owned.contains(&meld_proto::enums::CharacterClass::Keeper));

    let addr = start_server().await;
    let http = reqwest::Client::new();
    let base = format!("http://{addr}");
    let username = format!("pc_{}", &uuid::Uuid::new_v4().simple().to_string()[..10]);
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
    let player_id = login["player"]["player_id"].as_str().unwrap().to_string();

    // Earn them (and the seats they wait on), the way the game would.
    let db = meld_db::Db::connect(&std::env::var("MELD_DATABASE_URL").unwrap(), 4)
        .await
        .unwrap();
    let keys: Vec<String> = [
        "party_slot_2",
        "party_slot_3",
        "class_smithwright",
        "class_keeper",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    db.grant_unlocks(uuid::Uuid::parse_str(&player_id).unwrap(), &keys)
        .await
        .unwrap();

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

    let mut roster: Option<Value> = None;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(45);
    while roster.is_none() {
        assert!(tokio::time::Instant::now() < deadline, "no roster arrived");
        let Some(Ok(Message::Text(t))) = ws.next().await else { panic!("ws closed") };
        let v: Value = serde_json::from_str(&t).unwrap();
        match v["type"].as_str().unwrap_or("") {
            "session.authenticated" => send!("run.enter_maze", {
                "party": ["smithwright", "keeper", "smithwright"],
                "tutorial": true
            }),
            "session.error" => panic!("refused a party it had earned: {v}"),
            "run.party"
                if v["payload"]["heroes"].as_array().is_some_and(|h| !h.is_empty()) =>
            {
                roster = Some(v["payload"].clone());
            }
            _ => {}
        }
    }

    let heroes = roster.unwrap()["heroes"].as_array().cloned().unwrap_or_default();
    let classes: Vec<&str> = heroes.iter().filter_map(|h| h["class_key"].as_str()).collect();
    assert!(classes.contains(&"smithwright"), "{classes:?}");
    assert!(classes.contains(&"keeper"), "{classes:?}");

    // Each dives with its OWN stat spread, from `[player.<key>]` — a Keeper leads on
    // Mnd, a Smithwright on Str. Interchangeable numbers would make the classes a skin.
    let of = |key: &str| -> Value {
        heroes
            .iter()
            .find(|h| h["class_key"] == json!(key))
            .cloned()
            .expect("hero present")
    };
    let (smith, keeper) = (of("smithwright"), of("keeper"));
    assert!(
        smith["str_"].as_i64().unwrap_or(0) > smith["mnd"].as_i64().unwrap_or(0),
        "a Smithwright lifts iron: {smith}"
    );
    assert!(
        keeper["mnd"].as_i64().unwrap_or(0) > keeper["str_"].as_i64().unwrap_or(0),
        "a Keeper's work is medicine: {keeper}"
    );
    assert!(
        smith["max_hp"].as_i64().unwrap_or(0) > keeper["max_hp"].as_i64().unwrap_or(0),
        "the smith is the one standing in front"
    );

    // The kits come from the one registry, gated by the same ladder as everyone else.
    for (class, first, deepest) in [
        ("smithwright", "Hammer Fall", "The Great Work"),
        ("keeper", "Thornlash", "World Tree"),
    ] {
        let at_one = meld_proto::skills::skills_for_class_at(class, 1);
        assert_eq!(at_one.len(), 1, "{class} opens with one rung");
        assert_eq!(at_one[0].name, first);
        // The ladder runs to the top like everyone else's, inside its archetype's width.
        let full = meld_proto::skills::skills_for_class_at(class, 255);
        assert!(full.iter().any(|s| s.name == deepest), "{class} stops short of the top");
        assert!(
            full.len() <= meld_proto::skills::menu_width(meld_proto::skills::archetype(class)),
            "{class} fields {} rows",
            full.len()
        );
        // And a rank to wear while doing it.
        assert!(meld_proto::skills::rank_title(class, 1).is_some(), "{class} has a rank");
    }
}

/// A bench is a CLASS's bench: a Smithwright raises the forge, a Keeper the still. The Map
/// column used to offer "Set up a smith station" to any party carrying ore, and the server
/// took it — the skill gate is about how good the work is, not about who may set one up.
#[tokio::test]
async fn only_the_right_class_may_raise_a_bench() {
    let addr = start_server().await;
    let http = reqwest::Client::new();
    let base = format!("http://{addr}");
    let username = format!("cls_{}", &uuid::Uuid::new_v4().simple().to_string()[..10]);
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
        json!({"type":"session.authenticate","seq":seq,"ts":0,
               "payload":{"ticket":ticket,"resume":null}})
        .to_string(),
    ))
    .await
    .unwrap();
    seq += 1;

    // A fresh account fields ONE Explorer, so neither bench is its to raise.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(45);
    let mut refusals = 0;
    loop {
        assert!(tokio::time::Instant::now() < deadline, "no answer to the builds");
        let Some(Ok(Message::Text(t))) = ws.next().await else { panic!("ws closed") };
        let v: Value = serde_json::from_str(&t).unwrap();
        match v["type"].as_str().unwrap_or("") {
            "session.authenticated" => send!("run.enter_maze", {"tutorial": true}),
            "run.started" => {
                send!("run.build_station", {"kind": "smith"});
                send!("run.build_station", {"kind": "alembic"});
            }
            "session.error" => {
                let msg = v["payload"]["message"].as_str().unwrap_or_default().to_string();
                assert!(
                    !msg.is_empty(),
                    "a refusal with no words is the bug this is here to stop"
                );
                refusals += 1;
                if refusals == 2 {
                    break;
                }
            }
            "run.station_built" => panic!("an Explorer-only party raised a bench: {v}"),
            _ => {}
        }
    }
}
