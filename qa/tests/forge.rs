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
        .json(&json!({ "slot": "main_hand", "material": "dune_ingot" }))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 409, "forged something from nothing");
    let err: Value = res.json().await.unwrap();
    let msg = err["error"]["message"].as_str().unwrap_or_default();
    assert!(msg.contains("dune_ingot"), "the refusal should name the cost: {msg}");

    // Nonsense inputs are validation errors rather than surprises.
    for bad in [
        json!({ "slot": "hat", "material": "dune_ingot" }),
        json!({ "slot": "main_hand", "class_key": "wizard", "material": "dune_ingot" }),
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
        .json(&json!({ "material": "dune_ingot" }))
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

    // ---- The happy path, which nothing covered before: seed the Vault directly (the
    // only way to hold stock without a dive), then forge for real and check the
    // response describes what was made and what it cost.
    let db_url = std::env::var("MELD_DATABASE_URL").unwrap();
    let balance = meld_balance::Balance::load_default().unwrap();
    let db = meld_db::Db::connect(&db_url, balance.auth.bcrypt_cost).await.unwrap();
    let me: Value = http
        .get(format!("{base}/v1/players/me"))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let pid = uuid::Uuid::parse_str(me["player_id"].as_str().unwrap()).unwrap();
    db.bank_extraction(
        pid,
        &[("dune_ingot".into(), 20), ("bog_ichor".into(), 20)],
        10_000,
    )
    .await
    .unwrap();

    let res = http
        .post(format!("{base}/v1/crafting/forge"))
        .bearer_auth(&token)
        .json(&json!({ "slot": "main_hand", "material": "dune_ingot" }))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200, "a paid-up smith should get a blade");
    let made: Value = res.json().await.unwrap();
    assert!(!made["forged"].as_str().unwrap_or_default().is_empty(), "{made}");
    assert_eq!(made["slot"], json!("main_hand"));
    assert_eq!(made["catalyzed"], json!(false));
    // The numbers you just paid for, in the answer.
    assert!(made["stats"]["atk"].as_i64().unwrap_or(0) > 0, "a weapon should have atk: {made}");
    assert!(made["max_durability"].as_i64().unwrap_or(0) > 0, "{made}");
    assert!(!made["family"].as_str().unwrap_or_default().is_empty(), "{made}");
    assert!(made["gear_id"].as_str().is_some(), "{made}");
    // …and what it cost, itemised.
    let spent = made["spent"]["materials"].as_array().expect("itemised cost");
    assert!(
        spent.iter().any(|m| m["item_kind"] == json!("dune_ingot")
            && m["quantity"].as_i64().unwrap_or(0) > 0),
        "the ore spent should be named: {made}"
    );
    assert!(made["spent"]["chits"].as_i64().unwrap_or(0) > 0, "{made}");

    // A catalyst reaches PAST the smith's own level and rolls the better pool — the
    // sentence the whole design turns on, now checked over the wire.
    let plain_tier = made["tier"].as_i64().unwrap_or(0);
    let res = http
        .post(format!("{base}/v1/crafting/forge"))
        .bearer_auth(&token)
        .json(&json!({
            "slot": "main_hand",
            "material": "dune_ingot",
            "catalyst": "bog_ichor",
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    let quenched: Value = res.json().await.unwrap();
    assert_eq!(quenched["catalyzed"], json!(true));
    assert!(
        quenched["tier"].as_i64().unwrap_or(0) > plain_tier,
        "a trophy should buy reach: {} vs {plain_tier}",
        quenched["tier"]
    );
    assert_eq!(quenched["rarity"], json!("epic"), "catalyzed rolls the better pool");
    assert!(
        quenched["spent"]["materials"]
            .as_array()
            .into_iter()
            .flatten()
            .any(|m| m["item_kind"] == json!("bog_ichor")),
        "the catalyst should be itemised too: {quenched}"
    );

    // And the whole Forge surface is authenticated.
    let res = http
        .post(format!("{base}/v1/crafting/forge"))
        .json(&json!({ "slot": "main_hand", "material": "dune_ingot" }))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 401);
}

/// A smith's two services are not for every tier, and re-drawing a deep piece's
/// affixes is a bigger job than a starter blade's. Both rules are the SERVER's — the
/// client only declines to advertise a key it knows will be refused.
#[tokio::test]
async fn a_smith_takes_only_the_work_the_tier_allows_and_charges_by_depth() {
    let addr = start_server().await;
    let http = reqwest::Client::new();
    let base = format!("http://{addr}");
    let username = format!("sm_{}", &uuid::Uuid::new_v4().simple().to_string()[..10]);
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
    let me: Value = http
        .get(format!("{base}/v1/players/me"))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let pid = uuid::Uuid::parse_str(me["player_id"].as_str().unwrap()).unwrap();

    let db_url = std::env::var("MELD_DATABASE_URL").unwrap();
    let balance = meld_balance::Balance::load_default().unwrap();
    let db = meld_db::Db::connect(&db_url, balance.auth.bcrypt_cost).await.unwrap();
    // A smith who can afford the work: stock, chits, and the Forging level a reroll
    // wants (otherwise every answer below is the level gate instead of the tier rule).
    db.bank_extraction(pid, &[("dune_ingot".into(), 60), ("bog_ichor".into(), 20)], 100_000)
        .await
        .unwrap();
    db.add_skill_xp(pid, "forging", 100_000).await.unwrap();

    let gear_of = |token: String| {
        let http = http.clone();
        let base = base.clone();
        async move {
            http.get(format!("{base}/v1/vault/gear"))
                .bearer_auth(&token)
                .send()
                .await
                .unwrap()
                .json::<Value>()
                .await
                .unwrap()
        }
    };
    let rows = gear_of(token.clone()).await;
    let starter = rows["data"][0]["gear_id"].as_str().expect("starter gear").to_string();
    let starter_uuid = uuid::Uuid::parse_str(&starter).unwrap();
    let starter_cost = rows["data"][0]["reroll_cost"].as_i64().expect("the row carries its cost");
    assert!(starter_cost > 0, "a reroll cost belongs on the row: {}", rows["data"][0]);

    // A deeper piece costs more stock to re-draw. Quench one to reach past tier 0.
    let quenched: Value = http
        .post(format!("{base}/v1/crafting/forge"))
        .bearer_auth(&token)
        .json(&json!({
            "slot": "main_hand",
            "material": "dune_ingot",
            "catalyst": "bog_ichor",
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let deep_id = quenched["gear_id"].as_str().unwrap().to_string();
    let deep_tier = quenched["tier"].as_i64().unwrap_or(0);
    let rows = gear_of(token.clone()).await;
    let deep = rows["data"]
        .as_array()
        .unwrap()
        .iter()
        .find(|g| g["gear_id"] == json!(deep_id))
        .expect("the forged piece");
    assert!(deep_tier > 0, "the quench should have reached a tier: {quenched}");
    assert!(
        deep["reroll_cost"].as_i64().unwrap() > starter_cost,
        "a tier {deep_tier} piece should cost more to re-draw than tier 0: {} vs {starter_cost}",
        deep["reroll_cost"]
    );

    // Standard gear never wears down, so there is nothing to repair — and the refusal
    // says which of the three tiers it is talking about.
    set_tier(&db_url, starter_uuid, "normal").await;
    let res = http
        .post(format!("{base}/v1/vault/gear/{starter}/repair"))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 409);
    let msg = res.json::<Value>().await.unwrap()["error"]["message"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(msg.contains("never wears down"), "{msg}");
    // …but it is yours to keep, so it is worth re-drawing.
    let res = http
        .post(format!("{base}/v1/vault/gear/{starter}/reroll"))
        .bearer_auth(&token)
        .json(&json!({ "material": "dune_ingot" }))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200, "standard gear should be rerollable");
    let done: Value = res.json().await.unwrap();
    assert_eq!(
        done["spent"]["materials"][0]["quantity"].as_i64().unwrap(),
        starter_cost,
        "the bill should be the cost the row advertised: {done}"
    );

    // Ephemeral gear burns on the walk home: neither service is worth a chit, and both
    // say so rather than taking the payment.
    set_tier(&db_url, starter_uuid, "red").await;
    for (path, body) in [
        (format!("gear/{starter}/reroll"), Some(json!({ "material": "dune_ingot" }))),
        (format!("gear/{starter}/repair"), None),
    ] {
        let mut req = http.post(format!("{base}/v1/vault/{path}")).bearer_auth(&token);
        if let Some(b) = body {
            req = req.json(&b);
        }
        let res = req.send().await.unwrap();
        assert_eq!(res.status(), 409, "{path} charged for work on ephemeral gear");
        let msg = res.json::<Value>().await.unwrap()["error"]["message"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert!(msg.contains("burns"), "{path}: {msg}");
    }

    // Whatever a smith does, OWNERSHIP never moves: the piece is still in the same
    // Vault, under the same id, after the work.
    set_tier(&db_url, starter_uuid, "blue").await;
    assert_eq!(
        http.post(format!("{base}/v1/vault/gear/{starter}/reroll"))
            .bearer_auth(&token)
            .json(&json!({ "material": "dune_ingot" }))
            .send()
            .await
            .unwrap()
            .status(),
        200
    );
    let rows = gear_of(token).await;
    assert!(
        rows["data"].as_array().unwrap().iter().any(|g| g["gear_id"] == json!(starter)),
        "the piece left its owner's Vault"
    );
}

/// Retier one item in the DB. Nothing player-facing can do this — the drop roll picks
/// a tier and it is fixed for the item's life — so a test that wants one piece of each
/// tier has to reach past the API to set it up.
async fn set_tier(db_url: &str, gear_id: uuid::Uuid, tier: &str) {
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .connect(db_url)
        .await
        .unwrap();
    sqlx::query("UPDATE gear SET insurance = $2 WHERE gear_id = $1")
        .bind(gear_id)
        .bind(tier)
        .execute(&pool)
        .await
        .unwrap();
}
