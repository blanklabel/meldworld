//! PT-2: named party loadouts — save, list, load, overwrite, delete.
//!
//! The validation is the interesting half. A loadout is stored as a composition and
//! replayed later, and `clamp_party_to_unlocks` rewrites any party the account cannot
//! field at dive time — so a loadout saved with unearned heroes would silently become
//! a different party when loaded. It is refused at save time instead, which is the
//! only point where the player can be told.
//!
//! Requires Postgres: set `MELD_DATABASE_URL` (see qa/scripts/local_pg.sh).

use std::sync::Arc;

use serde_json::{json, Value};
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
    format!("{addr}")
}

#[tokio::test]
async fn loadouts_save_list_and_refuse_what_the_account_cannot_field() {
    let addr = start_server().await;
    let http = reqwest::Client::new();
    let base = format!("http://{addr}");
    let username = format!("lo_{}", &uuid::Uuid::new_v4().simple().to_string()[..10]);
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
    let token = login["session_token"].as_str().unwrap().to_string();
    let get = |t: String, b: String| async move {
        reqwest::Client::new()
            .get(format!("{b}/v1/party/loadouts"))
            .bearer_auth(t)
            .send()
            .await
            .unwrap()
            .json::<Value>()
            .await
            .unwrap()
    };

    // A fresh account has none.
    let l0 = get(token.clone(), base.clone()).await;
    assert_eq!(l0["data"].as_array().unwrap().len(), 0);

    // A fresh account owns ONE slot and only the Explorer, so that is all it may save.
    let ok = http
        .post(format!("{base}/v1/party/loadouts"))
        .bearer_auth(&token)
        .json(&json!({ "name": "Scout", "classes": ["explorer"] }))
        .send()
        .await
        .unwrap();
    assert_eq!(ok.status(), 200, "a legal one-hero loadout should save");

    // Too many heroes for the slots earned — refused, not silently truncated.
    let too_big = http
        .post(format!("{base}/v1/party/loadouts"))
        .bearer_auth(&token)
        .json(&json!({ "name": "Full", "classes": ["explorer", "explorer"] }))
        .send()
        .await
        .unwrap();
    assert_eq!(too_big.status(), 400, "two heroes on a one-slot account must be refused");

    // A class the account has not earned — refused for the same reason.
    let unowned = http
        .post(format!("{base}/v1/party/loadouts"))
        .bearer_auth(&token)
        .json(&json!({ "name": "Healer", "classes": ["resonant"] }))
        .send()
        .await
        .unwrap();
    assert_eq!(unowned.status(), 400, "an unearned class must be refused");

    // Nonsense class → refused rather than stored.
    let bogus = http
        .post(format!("{base}/v1/party/loadouts"))
        .bearer_auth(&token)
        .json(&json!({ "name": "Bogus", "classes": ["wizard"] }))
        .send()
        .await
        .unwrap();
    assert_eq!(bogus.status(), 400);

    // An empty name is not a name.
    let unnamed = http
        .post(format!("{base}/v1/party/loadouts"))
        .bearer_auth(&token)
        .json(&json!({ "name": "   ", "classes": ["explorer"] }))
        .send()
        .await
        .unwrap();
    assert_eq!(unnamed.status(), 400);

    // Only the one legal save landed.
    let l1 = get(token.clone(), base.clone()).await;
    let rows = l1["data"].as_array().unwrap();
    assert_eq!(rows.len(), 1, "only the legal loadout should exist: {l1}");
    assert_eq!(rows[0]["name"], json!("Scout"));
    assert_eq!(rows[0]["classes"], json!(["explorer"]));

    // Saving over a name UPDATES it rather than making a second row.
    http.post(format!("{base}/v1/party/loadouts"))
        .bearer_auth(&token)
        .json(&json!({ "name": "Scout", "classes": ["explorer"] }))
        .send()
        .await
        .unwrap();
    let l2 = get(token.clone(), base.clone()).await;
    assert_eq!(l2["data"].as_array().unwrap().len(), 1, "overwrite, not duplicate");

    // Delete it.
    let del = http
        .delete(format!("{base}/v1/party/loadouts/Scout"))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap();
    assert_eq!(del.status(), 200);
    let l3 = get(token.clone(), base.clone()).await;
    assert_eq!(l3["data"].as_array().unwrap().len(), 0);

    // Deleting something that is not there is not an error — it is already gone.
    let again = http
        .delete(format!("{base}/v1/party/loadouts/Scout"))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap();
    assert_eq!(again.status(), 200);

    // Loadouts are per-account: another player cannot see this one's.
    let other = format!("lo2_{}", &uuid::Uuid::new_v4().simple().to_string()[..10]);
    let ob = json!({ "username": other, "password": "correct-horse-battery" });
    http.post(format!("{base}/v1/auth/register")).json(&ob).send().await.unwrap();
    let ol: Value = http
        .post(format!("{base}/v1/auth/login"))
        .json(&ob)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let otoken = ol["session_token"].as_str().unwrap().to_string();
    http.post(format!("{base}/v1/party/loadouts"))
        .bearer_auth(&otoken)
        .json(&json!({ "name": "Theirs", "classes": ["explorer"] }))
        .send()
        .await
        .unwrap();
    let mine = get(token.clone(), base.clone()).await;
    assert_eq!(mine["data"].as_array().unwrap().len(), 0, "another account leaked in");

    // --- the guards ---
    //
    // A loadout is applied by NAME. The client never says which gear to equip, so
    // there is no request in which it could name gear it does not own. The server
    // reads the snapshot it took itself and re-validates every piece.

    // Applying a loadout that is not yours is a 404, not someone else's party.
    let steal = http
        .post(format!("{base}/v1/party/loadouts/Theirs/apply"))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap();
    assert_eq!(steal.status(), 404, "another account's loadout must not be applicable");

    // Anonymous callers cannot apply at all.
    let anon_apply = reqwest::Client::new()
        .post(format!("{base}/v1/party/loadouts/Theirs/apply"))
        .send()
        .await
        .unwrap();
    assert_eq!(anon_apply.status(), 401);

    // Save a loadout for THIS account, with whatever the starter kit equipped.
    http.post(format!("{base}/v1/party/loadouts"))
        .bearer_auth(&token)
        .json(&json!({ "name": "Kit", "classes": ["explorer"] }))
        .send()
        .await
        .unwrap();
    let saved = get(token.clone(), base.clone()).await;
    let kit = saved["data"]
        .as_array()
        .unwrap()
        .iter()
        .find(|l| l["name"] == json!("Kit"))
        .expect("Kit saved");
    let captured = kit["gear_count"].as_i64().unwrap();
    assert!(captured > 0, "the starter kit is equipped, so the snapshot should hold gear");

    // Applying it restores that gear — it is all still owned and unbroken.
    let applied: Value = http
        .post(format!("{base}/v1/party/loadouts/Kit/apply"))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        applied["gear_restored"].as_i64().unwrap(),
        captured,
        "everything still owned should go back on: {applied}"
    );
    assert_eq!(applied["gear_missing"].as_i64().unwrap(), 0);
    assert_eq!(applied["classes"], json!(["explorer"]), "the composition is re-clamped");

    // Unauthenticated callers get nothing.
    let anon = reqwest::Client::new()
        .get(format!("{base}/v1/party/loadouts"))
        .send()
        .await
        .unwrap();
    assert_eq!(anon.status(), 401);
}
