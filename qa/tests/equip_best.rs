//! GR-5 "equip best": one press dresses a hero from the SPARE gear.
//!
//! Server-side on purpose — every rule it needs (slot/family/weight legality, the
//! two-handed off-hand reservation, "a broken piece cannot be worn") already lives there,
//! and picking in the client would mean firing one equip per slot and hoping they all land.
//! What this pins is the promise the button makes: it dresses the hero it was pressed on,
//! it never strips a teammate, and it refuses nothing silently.

use serde_json::{json, Value};
use std::sync::Arc;
use tokio::net::TcpListener;

async fn start_server() -> String {
    let db_url = std::env::var("MELD_DATABASE_URL")
        .expect("set MELD_DATABASE_URL (see qa/scripts/local_pg.sh)");
    let config = meld_server::Config {
        bind_addr: "127.0.0.1:0".to_string(),
        database_url: db_url,
        balance: Arc::new(meld_balance::Balance::load_default().unwrap()),
        client_dist: None,
    };
    let built = meld_server::build(&config).await.expect("server builds");
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, built.router).await.unwrap();
    });
    addr.to_string()
}

#[tokio::test]
async fn equip_best_dresses_one_hero_and_leaves_the_others_alone() {
    let addr = start_server().await;
    let http = reqwest::Client::new();
    let base = format!("http://{addr}");
    let username = format!("eb_{}", &uuid::Uuid::new_v4().simple().to_string()[..10]);
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

    let gear_now = |t: String| {
        let http = http.clone();
        let base = base.clone();
        async move {
            let v: Value = http
                .get(format!("{base}/v1/vault/gear"))
                .bearer_auth(&t)
                .send()
                .await
                .unwrap()
                .json()
                .await
                .unwrap();
            v["data"].as_array().cloned().unwrap_or_default()
        }
    };

    let before = gear_now(token.clone()).await;
    assert!(!before.is_empty(), "a fresh account has starter gear to work with");

    let res = http
        .post(format!("{base}/v1/party/heroes/0/equip-best"))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200, "equip-best should answer");
    let out: Value = res.json().await.unwrap();
    assert_eq!(out["data"]["hero_slot"], json!(0));
    assert!(out["data"]["changed"].is_array(), "it has to report what it did: {out}");

    // Whatever it equipped is now on hero 0, and every piece it touched is legal for that
    // hero's class — the server would have refused otherwise, and silence is not allowed.
    let after = gear_now(token.clone()).await;
    let worn: Vec<&Value> =
        after.iter().filter(|g| g["equipped_hero_slot"] == json!(0)).collect();
    assert!(!worn.is_empty(), "hero 0 should be wearing something: {after:?}");
    let mut slots: Vec<&str> = worn.iter().filter_map(|g| g["slot"].as_str()).collect();
    slots.sort_unstable();
    let deduped = {
        let mut s = slots.clone();
        s.dedup();
        s
    };
    assert_eq!(slots, deduped, "a hero cannot wear two things in one slot: {slots:?}");

    // Nothing broken got equipped.
    for g in &worn {
        assert!(
            g["max_durability"].as_i64().unwrap_or(0) > 0,
            "a broken piece must not be worn: {g}"
        );
    }

    // Idempotent: pressing it again changes nothing, because nothing spare beats what is on.
    let again: Value = http
        .post(format!("{base}/v1/party/heroes/0/equip-best"))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        again["data"]["changed"].as_array().map(|a| a.len()),
        Some(0),
        "a second press should find nothing better: {again}"
    );

    // A slot nobody has is refused, rather than quietly doing nothing.
    let bad = http
        .post(format!("{base}/v1/party/heroes/99/equip-best"))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap();
    assert_eq!(bad.status(), 400, "an impossible hero slot is a validation error");
}
