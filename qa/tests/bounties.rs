//! Bounty conformance (roadmap `AD-4`): the Den's board opens only to a Hunter, rolls
//! contracts against the caller's hunter rank, and refuses to pay for a mark that is still
//! standing — over the real HTTP surface.
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
    };
    let built = meld_server::build(&config).await.expect("server builds");
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, built.router).await.unwrap();
    });
    format!("{addr}")
}

async fn fresh_player(base: &str, http: &reqwest::Client) -> (String, String, String) {
    let username = format!("den_{}", &uuid::Uuid::new_v4().simple().to_string()[..10]);
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

#[tokio::test]
async fn the_den_opens_to_a_hunter_rolls_its_own_contracts_and_pays_for_nothing_standing() {
    let addr = start_server().await;
    let http = reqwest::Client::new();
    let base = format!("http://{addr}");
    let (token, _ticket, player_id) = fresh_player(&base, &http).await;

    // Before the Hunter is earned there is no board at all — the surface is the Den's, so
    // it is a 403 rather than an empty list a client would render as "nothing posted".
    let res = http.get(format!("{base}/v1/bounties")).bearer_auth(&token).send().await.unwrap();
    assert_eq!(res.status(), 403, "the Den handed its board to a stranger");

    // The Hunter waits on the second party slot (a hero at level 10) as well as coming
    // home, which is a grind no conformance test should walk. Grant it the way the other
    // unlock-gated tests do and get on with the board.
    let db = meld_db::Db::connect(&std::env::var("MELD_DATABASE_URL").unwrap(), 4)
        .await
        .unwrap();
    let keys: Vec<String> = ["party_slot_2", "class_hunter"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    db.grant_unlocks(uuid::Uuid::parse_str(&player_id).unwrap(), &keys)
        .await
        .unwrap();

    // The board is open now, and it posted a full slate without anyone scheduling it.
    let board: Value = http
        .get(format!("{base}/v1/bounties"))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(board["rank"], json!(0), "a first-time hunter is unblooded");
    assert_eq!(board["rank_title"], json!("Unblooded"));
    let active = board["active"].as_array().cloned().unwrap_or_default();
    assert!(!active.is_empty(), "the Den posted nothing: {board}");
    assert!(board["history"].as_array().is_some_and(|h| h.is_empty()));

    for b in &active {
        assert_eq!(b["state"], json!("active"));
        // Every contract is a named mark somewhere reachable, worth something.
        assert!(!b["mark_name"].as_str().unwrap_or_default().is_empty());
        assert!(!b["boss_kind"].as_str().unwrap_or_default().is_empty());
        assert!(b["distance"].as_i64().unwrap() > 0);
        assert!(b["power"].as_f64().unwrap() > 1.0, "a mark no worse than a creature: {b}");
        assert!(b["reward_chits"].as_i64().unwrap() > 0);
        assert!(b["reward_rank_xp"].as_i64().unwrap() > 0, "a contract that raises no rank");
        assert!(b["expires_in_secs"].as_i64().unwrap() > 0, "a contract already withdrawn");
        let venue = b["venue"].as_str().unwrap();
        assert!(matches!(venue, "overworld" | "dungeon"), "odd venue {venue}");
        let where_to = b["where_to_look"].as_str().unwrap();
        assert!(
            where_to.contains(&b["distance"].as_i64().unwrap().to_string()),
            "the sighting does not name its own depth: {where_to}"
        );
    }

    // Reading the board again is idempotent: the slate is topped up, never stacked.
    let again: Value = http
        .get(format!("{base}/v1/bounties"))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        again["active"].as_array().map(|a| a.len()),
        Some(active.len()),
        "the Den re-posted the board instead of keeping it"
    );

    // A mark still standing is not a payout, and the refusal says so by name.
    let id = active[0]["bounty_id"].as_str().unwrap();
    let res = http
        .post(format!("{base}/v1/bounties/{id}/claim"))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 409);
    let err: Value = res.json().await.unwrap();
    let msg = err["error"]["message"].as_str().unwrap();
    assert!(msg.contains("standing"), "unhelpful refusal: {msg}");

    // The rank cannot move on a refusal.
    let after: Value = http
        .get(format!("{base}/v1/bounties"))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(after["rank"], json!(0), "a refused claim moved the rank");

    // An unknown contract is a 404, and someone else's is not yours to claim.
    let res = http
        .post(format!("{base}/v1/bounties/{}/claim", uuid::Uuid::now_v7()))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 404);
    let (other, _, _) = fresh_player(&base, &http).await;
    let res = http
        .post(format!("{base}/v1/bounties/{id}/claim"))
        .bearer_auth(&other)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 404, "another account reached into this one's contracts");
}
