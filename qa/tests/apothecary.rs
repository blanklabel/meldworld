//! Apothecary + potion crafting conformance (roadmap `GR-4` / `MS-1` / `EC-2`):
//! one NPC sells the lowest-tier basics for chits, and recipes turn harvested
//! materials into potions crediting the right Meld skill — all over the real HTTP
//! surface.
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
async fn the_apothecary_sells_the_basics_and_recipes_brew_potions() {
    let addr = start_server().await;
    let http = reqwest::Client::new();
    let base = format!("http://{addr}");
    let username = format!("ap_{}", &uuid::Uuid::new_v4().simple().to_string()[..10]);
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

    // The shelf: a heal, a Barrier, a Regen, and a way home — every one priced.
    let shop: Value = http
        .get(format!("{base}/v1/vendors/apothecary"))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let stock: Vec<String> = shop["data"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["item_kind"].as_str().unwrap().to_string())
        .collect();
    for want in ["bloom_salve", "bulwark_tonic", "mending_draught", "town_portal"] {
        assert!(stock.contains(&want.to_string()), "shelf missing {want}: {stock:?}");
    }
    for s in shop["data"].as_array().unwrap() {
        assert!(s["price_chits"].as_i64().unwrap() > 0, "unpriced stock: {s}");
        assert!(!s["name"].as_str().unwrap().is_empty());
    }

    // A fresh account has no chits, so the shop must refuse rather than gift.
    let broke = http
        .post(format!("{base}/v1/vendors/apothecary/buy"))
        .bearer_auth(&token)
        .json(&json!({ "item_kind": "bloom_salve", "quantity": 1 }))
        .send()
        .await
        .unwrap();
    assert_eq!(broke.status(), 409, "a penniless player bought a potion");

    // Things the Apothecary does not stock are a 404, not a silent success.
    let nope = http
        .post(format!("{base}/v1/vendors/apothecary/buy"))
        .bearer_auth(&token)
        .json(&json!({ "item_kind": "elixir", "quantity": 1 }))
        .send()
        .await
        .unwrap();
    assert_eq!(nope.status(), 404, "bought something off-menu");

    // Every recipe is listed with its inputs and the skill it credits.
    let recipes: Value = http
        .get(format!("{base}/v1/crafting/recipes"))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let listed = recipes["data"].as_array().unwrap();
    assert!(listed.len() >= 6, "only {} recipes listed", listed.len());
    let salve = listed
        .iter()
        .find(|r| r["recipe"] == "bloom_salve")
        .expect("the salve recipe");
    assert_eq!(salve["skill"], "alchemy", "a potion must credit Alchemy");
    assert!(!salve["inputs"].as_array().unwrap().is_empty());

    // Crafting without the materials is a 409 that names what is missing.
    let res = http
        .post(format!("{base}/v1/crafting/craft"))
        .bearer_auth(&token)
        .json(&json!({ "recipe": "bulwark_tonic" }))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 409);
    let err: Value = res.json().await.unwrap();
    let msg = err["error"]["message"].as_str().unwrap_or_default();
    assert!(msg.contains("heartoak_bark"), "the refusal should name the material: {msg}");

    // An unknown recipe is a 404 rather than a crafted mystery.
    let res = http
        .post(format!("{base}/v1/crafting/craft"))
        .bearer_auth(&token)
        .json(&json!({ "recipe": "philosophers_stone" }))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 404);

    // And the whole vendor surface is authenticated.
    for path in ["/v1/vendors/apothecary", "/v1/crafting/recipes"] {
        let res = http.get(format!("{base}{path}")).send().await.unwrap();
        assert_eq!(res.status(), 401, "{path} served an unauthenticated caller");
    }
}
