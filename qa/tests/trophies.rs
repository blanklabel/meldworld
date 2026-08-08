//! Trophy conformance (roadmap `MS-1`): the two sinks a combat drop now has — the
//! **trophy recipe line** (monster parts brewed into stronger potions, gated behind a
//! permanent Alchemy level), the **Forge catalyst**, and the **Broker** that buys any
//! material for chits and pays Mercantile XP for it.
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

async fn fresh_player(base: &str, http: &reqwest::Client, prefix: &str) -> String {
    let username = format!("{prefix}{}", &uuid::Uuid::new_v4().simple().to_string()[..10]);
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
    login["session_token"].as_str().unwrap().to_string()
}

#[tokio::test]
async fn every_combat_drop_has_a_recipe_a_forge_use_and_a_price() {
    let addr = start_server().await;
    let http = reqwest::Client::new();
    let base = format!("http://{addr}");
    let token = fresh_player(&base, &http, "tr_").await;

    // The Broker quotes every material — including all five combat drops. This is
    // the floor that makes carrying something home never worthless.
    let broker: Value = http
        .get(format!("{base}/v1/vendors/broker"))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let quotes = broker["data"].as_array().unwrap();
    for trophy in [
        "forest_bloom_petal",
        "sun_scarab_husk",
        "ember_cinder",
        "frost_shard",
        "bog_ichor",
    ] {
        let q = quotes
            .iter()
            .find(|q| q["item_kind"] == trophy)
            .unwrap_or_else(|| panic!("the Broker does not quote {trophy}"));
        assert_eq!(q["material_class"], "trophy", "{trophy} is not classed as a trophy");
        assert!(q["price_chits"].as_i64().unwrap() > 0, "{trophy} is priced at nothing");
    }
    // A deep trophy is worth more than a shallow herb, which is what makes the deep
    // bands worth the walk back.
    let price = |kind: &str| -> i64 {
        quotes
            .iter()
            .find(|q| q["item_kind"] == kind)
            .and_then(|q| q["price_chits"].as_i64())
            .unwrap()
    };
    assert!(price("bog_ichor") > price("bloom_herb"));
    assert!(price("forest_bloom_petal") > price("bloom_herb"), "a trophy beats a plant");

    // Every trophy is named by a real recipe, listed with the level it needs.
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
    for r in listed {
        assert!(r["required_level"].as_i64().unwrap() >= 1, "unlevelled recipe: {r}");
        assert!(r["skill_level"].as_i64().unwrap() >= 1);
    }
    let trophy_inputs: Vec<&str> = listed
        .iter()
        .flat_map(|r| r["inputs"].as_array().unwrap())
        .filter(|i| i["material_class"] == "trophy")
        .map(|i| i["item_kind"].as_str().unwrap())
        .collect();
    for trophy in ["forest_bloom_petal", "sun_scarab_husk", "ember_cinder", "frost_shard", "bog_ichor"] {
        assert!(trophy_inputs.contains(&trophy), "no recipe consumes {trophy}");
    }

    // A fresh alchemist is level 1: the basics are craftable (they only lack
    // materials, a 409) but the deeper trophy recipes are LOCKED (a 403). That
    // distinction is the whole point of a permanent crafting level.
    let craft = |recipe: &'static str| {
        let http = http.clone();
        let base = base.clone();
        let token = token.clone();
        async move {
            http.post(format!("{base}/v1/crafting/craft"))
                .bearer_auth(&token)
                .json(&json!({ "recipe": recipe }))
                .send()
                .await
                .unwrap()
        }
    };
    assert_eq!(craft("bloom_salve").await.status(), 409, "level 1 basics should be open");
    let locked = craft("quintessence").await;
    assert_eq!(locked.status(), 403, "the capstone was craftable at Alchemy 1");
    let err: Value = locked.json().await.unwrap();
    let msg = err["error"]["message"].as_str().unwrap_or_default();
    assert!(
        msg.contains("alchemy") && msg.contains("level"),
        "a locked recipe should say which level is missing: {msg}"
    );
    assert_eq!(craft("ichor_salve").await.status(), 403);

    // The Forge takes an ORE for the body and a TROPHY as the catalyst; anything
    // else is a validation error rather than a silently-accepted absurdity.
    let forge = |body: Value| {
        let http = http.clone();
        let base = base.clone();
        let token = token.clone();
        async move {
            http.post(format!("{base}/v1/crafting/forge"))
                .bearer_auth(&token)
                .json(&body)
                .send()
                .await
                .unwrap()
        }
    };
    assert_eq!(
        forge(json!({ "slot": "main_hand", "material": "bloom_herb" })).await.status(),
        400,
        "forged a blade out of herbs"
    );
    // Raw ore is refused too — a Smelter stands between the ground and the anvil — and
    // the refusal names the smelt so the player is not left guessing.
    let raw = forge(json!({ "slot": "main_hand", "material": "dune_iron" })).await;
    assert_eq!(raw.status(), 400, "forged a blade out of unsmelted ore");
    let err: Value = raw.json().await.unwrap();
    let msg = err["error"]["message"].as_str().unwrap_or_default();
    assert!(
        msg.contains("dune_ingot"),
        "the refusal should name the smelt to run: {msg}"
    );
    assert_eq!(
        forge(json!({ "slot": "main_hand", "material": "bog_ichor" })).await.status(),
        400,
        "forged a blade out of a trophy with no ore"
    );
    assert_eq!(
        forge(json!({
            "slot": "main_hand",
            "material": "dune_ingot",
            "catalyst": "dune_ingot",
        }))
        .await
        .status(),
        400,
        "catalyzed a forge with an ore"
    );
    // With legal inputs the only thing standing in the way is the bill, and the
    // refusal names both halves of it.
    let res = forge(json!({
        "slot": "main_hand",
        "material": "dune_ingot",
        "catalyst": "bog_ichor",
    }))
    .await;
    assert_eq!(res.status(), 409);
    let err: Value = res.json().await.unwrap();
    let msg = err["error"]["message"].as_str().unwrap_or_default();
    assert!(
        msg.contains("dune_ingot") && msg.contains("bog_ichor"),
        "the refusal should name the catalyst too: {msg}"
    );

    // The SMELT line: Forging's own craft ladder. Every ore has a smelt, the deep ones
    // are gated behind a better smith, and the Forge builds from what they produce — so
    // a Smithwright's pipeline is harvest ore -> smelt -> forge rather than a single tap.
    let forging_recipes: Vec<&Value> =
        listed.iter().filter(|r| r["skill"] == "forging").collect();
    assert!(
        forging_recipes.len() > 1,
        "Forging had one recipe (the Town Portal) and needs a craft line: {:?}",
        forging_recipes.iter().map(|r| &r["recipe"]).collect::<Vec<_>>()
    );
    for (ore, refined) in [
        ("heartoak_bark", "heartoak_stave"),
        ("dune_iron", "dune_ingot"),
        ("cinder_ore", "cinder_ingot"),
        ("rime_ore", "rime_ingot"),
        ("peat_iron", "peat_ingot"),
    ] {
        let r = listed
            .iter()
            .find(|r| r["output"] == json!(refined))
            .unwrap_or_else(|| panic!("no recipe makes {refined}"));
        assert_eq!(r["skill"], "forging", "{refined} should credit Forging");
        let takes_raw = r["inputs"].as_array().into_iter().flatten().any(|i| {
            i["item_kind"] == json!(ore)
                && i["material_class"] == json!("ore")
                && i["quantity"].as_i64().unwrap_or(0) > 1
        });
        assert!(takes_raw, "{refined} should cost several raw {ore}: {r}");
        // Refined stock is worth more at the Broker than the ore it came from — a
        // Smelter's labour is in it.
        assert!(
            price(refined) > price(ore),
            "{refined} ({}) should out-price {ore} ({})",
            price(refined),
            price(ore)
        );
    }
    // A fresh smith can run the shallow smelt (only materials are missing → 409) but
    // the deep bands are locked (403). That gate is the reason to bank ore you cannot
    // yet work.
    assert_eq!(craft("heartoak_stave").await.status(), 409, "the first smelt should be open");
    assert_eq!(craft("peat_ingot").await.status(), 403, "deep ore needs a better smith");

    // A craft says what it MADE and what it COST, so a caller never has to hold the
    // recipe table or re-read the Vault to report a result.
    let smelt_refusal = craft("heartoak_stave").await;
    assert_eq!(smelt_refusal.status(), 409);
    let err: Value = smelt_refusal.json().await.unwrap();
    assert!(
        err["error"]["message"].as_str().unwrap_or_default().contains("heartoak_bark"),
        "a refusal should name what is missing: {err}"
    );

    // The Broker refuses what it does not deal in, refuses a sale the Vault cannot
    // cover, and never gifts chits for either.
    let sell = |body: Value| {
        let http = http.clone();
        let base = base.clone();
        let token = token.clone();
        async move {
            http.post(format!("{base}/v1/vendors/broker/sell"))
                .bearer_auth(&token)
                .json(&body)
                .send()
                .await
                .unwrap()
        }
    };
    assert_eq!(
        sell(json!({ "item_kind": "bloom_salve", "quantity": 1 })).await.status(),
        400,
        "the Broker bought a potion"
    );
    assert_eq!(
        sell(json!({ "item_kind": "not_a_thing", "quantity": 1 })).await.status(),
        400
    );
    assert_eq!(sell(json!({ "item_kind": "bog_ichor", "quantity": 0 })).await.status(), 400);
    assert_eq!(
        sell(json!({ "item_kind": "bog_ichor", "quantity": 3 })).await.status(),
        409,
        "sold trophies it never had"
    );
    let vault: Value = http
        .get(format!("{base}/v1/vault"))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(vault["chits"].as_i64().unwrap(), 0, "a failed sale minted chits");

    // THE REQUISITION (EC-2): chits buy the plainest gear in the game, so a player who
    // died with nothing can walk back out equipped.
    let stock: Value = http
        .get(format!("{base}/v1/vendors/requisition"))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let rows = stock["data"].as_array().expect("the counter has stock");
    assert!(!rows.is_empty(), "a fresh account should be able to buy something: {stock}");
    for row in rows {
        assert!(row["price_chits"].as_i64().unwrap_or(0) > 0, "unpriced stock: {row}");
        // Shop gear is the FLOOR: tier 0, common, and never insured against a wipe the
        // way found or forged gear is. Chits must not buy a way past the loot chase.
        assert_eq!(row["tier"], json!(0), "shop gear should be the baseline: {row}");
        assert_eq!(row["rarity"], json!("common"), "{row}");
        assert_eq!(row["insurance"], json!("standard"), "{row}");
    }

    // A penniless player is refused and told the price.
    let res = http
        .post(format!("{base}/v1/vendors/requisition/buy"))
        .bearer_auth(&token)
        .json(&json!({ "slot": "main_hand" }))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 409, "bought a blade with no chits");
    let err: Value = res.json().await.unwrap();
    assert!(
        err["error"]["message"].as_str().unwrap_or_default().contains("chits"),
        "the refusal should name the price: {err}"
    );
    // Nonsense slots are validation errors rather than surprises.
    for bad in [json!({ "slot": "hat" }), json!({ "slot": "main_hand", "class_key": "wizard" })] {
        let res = http
            .post(format!("{base}/v1/vendors/requisition/buy"))
            .bearer_auth(&token)
            .json(&bad)
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), 400, "accepted {bad}");
    }

    // And the whole surface is authenticated.
    for path in ["/v1/vendors/broker", "/v1/vendors/requisition"] {
        assert_eq!(
            http.get(format!("{base}{path}")).send().await.unwrap().status(),
            401,
            "{path} served an unauthenticated caller"
        );
    }
    assert_eq!(
        http.post(format!("{base}/v1/vendors/broker/sell"))
            .json(&json!({ "item_kind": "bog_ichor", "quantity": 1 }))
            .send()
            .await
            .unwrap()
            .status(),
        401
    );
}
