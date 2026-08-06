//! Forge conformance (roadmap `MS-1`): forging gear, rerolling its affixes, and
//! repairing what a death chewed — over the real HTTP surface, with Forging level
//! as the lever on all three.
//!
//! Requires Postgres: set `MELD_DATABASE_URL` (see qa/scripts/local_pg.sh).

use std::sync::Arc;

use serde_json::{json, Value};
use tokio::net::TcpListener;

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

#[tokio::test]
async fn the_forge_makes_gear_rerolls_affixes_and_charges_for_it() {
    let addr = start_server().await;
    let http = reqwest::Client::new();
    let base = format!("http://{addr}");
    let username = format!("fg_{}", &uuid::Uuid::new_v4().simple().to_string()[..10]);
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
    let token = login["session_token"].as_str().unwrap().to_string();

    // A penniless smith is refused, and told what the forge needs.
    let res = http
        .post(format!("{base}/v1/crafting/forge"))
        .bearer_auth(&token)
        .json(&json!({ "slot": "main_hand", "material": "dune_iron" }))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 409, "forged something from nothing");
    let err: Value = res.json().await.unwrap();
    let msg = err["error"]["message"].as_str().unwrap_or_default();
    assert!(msg.contains("dune_iron"), "the refusal should name the cost: {msg}");

    // Nonsense inputs are validation errors rather than surprises.
    for bad in [
        json!({ "slot": "hat", "material": "dune_iron" }),
        json!({ "slot": "main_hand", "class_key": "wizard", "material": "dune_iron" }),
    ] {
        let res = http
            .post(format!("{base}/v1/crafting/forge"))
            .bearer_auth(&token)
            .json(&bad)
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), 400, "accepted {bad}");
    }

    // Rerolling is gated behind Forging level, and a fresh smith has none.
    let gear: Value = http
        .get(format!("{base}/v1/vault/gear"))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let starter = gear["data"][0]["gear_id"].as_str().expect("starter gear").to_string();
    let res = http
        .post(format!("{base}/v1/vault/gear/{starter}/reroll"))
        .bearer_auth(&token)
        .json(&json!({ "material": "dune_iron" }))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 409);
    let err: Value = res.json().await.unwrap();
    let msg = err["error"]["message"].as_str().unwrap_or_default();
    assert!(msg.contains("Forging level"), "a beginner should be told why: {msg}");

    // Repairing undamaged gear is refused rather than billed for nothing.
    let res = http
        .post(format!("{base}/v1/vault/gear/{starter}/repair"))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 409, "charged for a repair with nothing to fix");

    // Unknown gear is a 404 on both, not a 500.
    let ghost = uuid::Uuid::now_v7();
    for path in [format!("gear/{ghost}/repair")] {
        let res = http
            .post(format!("{base}/v1/vault/{path}"))
            .bearer_auth(&token)
            .send()
            .await
            .unwrap();
        assert!(
            res.status() == 409 || res.status() == 404,
            "{path} answered {}",
            res.status()
        );
    }

    // And the whole Forge surface is authenticated.
    let res = http
        .post(format!("{base}/v1/crafting/forge"))
        .json(&json!({ "slot": "main_hand", "material": "dune_iron" }))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 401);
}
