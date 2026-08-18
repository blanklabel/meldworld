//! Drinking a potion OUT of combat (`run.use_item`). The bug this covers: a wounded
//! party had to find a fight before it could heal, so the walk to the next monster
//! was where you died.
//!
//! Three things have to hold, and the last two matter more than the first:
//!
//! 1. A wounded hero drinks a salve on the overworld and comes back up.
//! 2. A potion that would do NOTHING is refused rather than swallowed — at full HP
//!    the bottle stays in the pack. A no-op that still spent the item would be the
//!    cruellest reading of "you can use items in the field".
//! 3. A combat-only potion (Barrier/Regen/Evasion/Adrenaline) is refused out here,
//!    because the state it grants would be gone before the next encounter.
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
    let mut balance = meld_balance::Balance::load_default().unwrap();
    balance.battle.party_size_per_player = 1;
    // FORCE the precondition instead of waiting for the world to supply it. This test is
    // about the overworld Item path — that a wounded hero may drink and a battle-only bottle
    // may not — and it was reaching that state by wandering until something happened to hurt
    // somebody. That is a coin flip on the difficulty curve, and it failed roughly half the
    // time regardless of what the curve was doing (confirmed by zeroing the resistance work
    // and watching it fail anyway). `insight_mote` already does exactly this for its own
    // probabilistic input, setting `world_xp_item_chance = 1.0`.
    //
    // The lever is the HERO, not the creature. Cranking creature attack (tried: x4) wounds
    // nobody — it KILLS them, and the run ends before a potion is ever drunk. A hero with a
    // great deal of HP is chipped by the first exchange and cannot die to it, so "wounded but
    // alive" is reached every run instead of on a lucky one.
    for p in balance.player.values_mut() {
        p.base_hp *= 20;
    }
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

#[tokio::test]
async fn a_wounded_hero_can_drink_on_the_overworld_and_a_useless_potion_stays_corked() {
    // Pinned: the bot has to actually MEET something to get wounded, and which
    // creatures the walk finds is the world roll's call, not this test's subject.
    std::env::set_var("MELD_SEED", "1");
    let addr = start_server().await;
    let http = reqwest::Client::new();
    let base = format!("http://{addr}");
    let username = format!("fi_{}", &uuid::Uuid::new_v4().simple().to_string()[..10]);
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

    let (mut ws, _) = connect_async(format!("ws://{addr}/v1/realtime")).await.unwrap();
    let mut seq = 1u32;
    let mut input_seq = 0u32;
    let mut nav = meld_qa::Nav::default();
    ws.send(Message::Text(
        json!({"type":"session.authenticate","seq":seq,"ts":0,"payload":{"ticket":ticket,"resume":null}}).to_string(),
    ))
    .await
    .unwrap();
    seq += 1;

    // The two refusals are checked the moment the run starts (everyone is at full
    // HP then, which is exactly the condition for refusal #2); the heal is checked
    // after a fight leaves someone short.
    let mut salves = 0i32;
    let mut refusals: Vec<String> = Vec::new();
    let mut probed_full_hp = false;
    let mut probed_combat_only = false;
    let mut in_battle = false;
    let mut my_c = String::new();
    let mut bid = String::new();
    let mut wounded_hp: Option<(i32, i32)> = None;
    let mut drink_sent = false;
    let mut healed_to: Option<i32> = None;
    let mut spent_in_field = 0i32;
    // A field drink is paid by whichever container held it — the drinker's own pouch
    // first, else the Party Inventory — so the test watches BOTH and asserts exactly one
    // salve left the run. Watching only one container makes the assertion depend on
    // where the starting kit happened to be dealt.
    let mut pouch_salves = 0i32;
    let mut pouch_salves_first: Option<i32> = None;

    let mut mover = tokio::time::interval(Duration::from_millis(80));
    mover.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(180);

    while healed_to.is_none() {
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out (salves {salves}, wounded {wounded_hp:?}, refusals {refusals:?})"
        );
        tokio::select! {
            _ = mover.tick(), if !in_battle => {
                input_seq += 1;
                let (dx, dy) = nav.heading_any(0);
                ws.send(Message::Text(json!({
                    "type":"movement.move_intent","seq":seq,"ts":0,
                    "payload":{"input_seq":input_seq,"move_dir":{"x":dx,"y":dy},"client_pos":{"x":0.0,"y":0.0}}
                }).to_string())).await.unwrap();
                seq += 1;
            }
            msg = ws.next() => {
                let Some(Ok(Message::Text(t))) = msg else { panic!("ws closed") };
                let v: Value = serde_json::from_str(&t).unwrap();
                match v["type"].as_str().unwrap_or("") {
                    "session.authenticated" => {
                        ws.send(Message::Text(json!({"type":"run.enter_maze","seq":seq,"ts":0,"payload":{}}).to_string())).await.unwrap();
                        seq += 1;
                    }
                    "run.started" => {
                        for it in v["payload"]["backpack"].as_array().into_iter().flatten() {
                            if it["item_kind"].as_str() == Some("bloom_salve") {
                                salves += it["quantity"].as_i64().unwrap_or(0) as i32;
                            }
                        }
                        // Refusal #2: nobody is hurt yet.
                        ws.send(Message::Text(json!({
                            "type":"run.use_item","seq":seq,"ts":0,
                            "payload":{"item_kind":"bloom_salve","hero_slot":0}
                        }).to_string())).await.unwrap();
                        seq += 1;
                        probed_full_hp = true;
                        // Refusal #3: a Barrier potion out of combat.
                        ws.send(Message::Text(json!({
                            "type":"run.use_item","seq":seq,"ts":0,
                            "payload":{"item_kind":"bulwark_tonic","hero_slot":0}
                        }).to_string())).await.unwrap();
                        seq += 1;
                        probed_combat_only = true;
                    }
                    // The starting salves are dealt into the heroes' POUCHES, not the
                    // Party Inventory, so this is where the kit shows up. Counting only
                    // the inventory found none and made the field-drink untestable.
                    "run.pouches" => {
                        // A snapshot message, so ASSIGN rather than accumulate: it is
                        // re-sent whole after every change.
                        pouch_salves = 0;
                        for p in v["payload"]["pouches"].as_array().into_iter().flatten() {
                            for it in p["items"].as_array().into_iter().flatten() {
                                if it["item_kind"].as_str() == Some("bloom_salve") {
                                    pouch_salves += it["quantity"].as_i64().unwrap_or(0) as i32;
                                }
                            }
                        }
                        if pouch_salves_first.is_none() {
                            pouch_salves_first = Some(pouch_salves);
                            salves += pouch_salves;
                            assert!(
                                salves > 0,
                                "the starting kit should include salves to drink"
                            );
                        }
                    }
                    "world.snapshot" => nav.observe(&v["payload"], &player_id),
                    "run.party" => {
                        let heroes = v["payload"]["heroes"].as_array().cloned().unwrap_or_default();
                        let Some(h) = heroes.first() else { continue };
                        let hp = h["hp"].as_i64().unwrap_or(0) as i32;
                        let max = h["max_hp"].as_i64().unwrap_or(0) as i32;
                        if drink_sent && wounded_hp.is_some_and(|(was, _)| hp > was) {
                            healed_to = Some(hp);
                            continue;
                        }
                        // Wounded, alive, and out of the fight: the exact moment the
                        // old build made you go find a monster before you could heal.
                        if !drink_sent && !in_battle && hp > 0 && hp < max {
                            wounded_hp = Some((hp, max));
                            drink_sent = true;
                            ws.send(Message::Text(json!({
                                "type":"run.use_item","seq":seq,"ts":0,
                                "payload":{"item_kind":"bloom_salve","hero_slot":0}
                            }).to_string())).await.unwrap();
                            seq += 1;
                        }
                    }
                    "run.backpack_update" => {
                        for ch in v["payload"]["changes"].as_array().into_iter().flatten() {
                            if ch["item"]["item_kind"].as_str() != Some("bloom_salve") {
                                continue;
                            }
                            if ch["delta"].as_str() == Some("removed")
                                && ch["cause"].as_str() == Some("field_item")
                            {
                                spent_in_field += ch["item"]["quantity"].as_i64().unwrap_or(0) as i32;
                            }
                        }
                    }
                    "session.error" | "error" => {
                        refusals.push(v["payload"]["message"].as_str().unwrap_or_default().to_string());
                    }
                    "battle.started" => {
                        in_battle = true;
                        my_c = v["payload"]["your_combatant_id"].as_str().unwrap_or_default().to_string();
                        bid = v["payload"]["battle_id"].as_str().unwrap_or_default().to_string();
                    }
                    "battle.ended" => in_battle = false,
                    "battle.turn_ready" if v["payload"]["combatant_id"].as_str() == Some(my_c.as_str()) => {
                        // Attack, so the fight ends and leaves a survivor to heal.
                        let target = v["payload"]["valid_targets"]
                            .as_array()
                            .and_then(|a| a.first())
                            .and_then(|t| t.as_str())
                            .unwrap_or("")
                            .to_string();
                        ws.send(Message::Text(json!({
                            "type":"battle.submit_action","seq":seq,"ts":0,
                            "payload":{
                                "battle_id":bid,
                                "action_id":uuid::Uuid::new_v4().to_string(),
                                "action":"attack",
                                "skill_kind":null,
                                "item_id":null,
                                "target_ids":[target]
                            }
                        }).to_string())).await.unwrap();
                        seq += 1;
                    }
                    _ => {}
                }
            }
        }
    }

    assert!(probed_full_hp && probed_combat_only, "both refusals were probed");
    let (was, max) = wounded_hp.expect("the bot took a wound");
    let now = healed_to.unwrap();
    assert!(now > was, "the salve should have healed: {was} -> {now} (max {max})");
    assert!(now <= max, "a heal must not overshoot max HP: {now} > {max}");
    let from_pouch = pouch_salves_first.expect("pouches were reported") - pouch_salves;
    assert_eq!(
        spent_in_field + from_pouch,
        1,
        "exactly one salve should leave the run \
         (inventory {spent_in_field}, pouches {from_pouch})"
    );

    assert!(
        refusals.iter().any(|m| m.contains("full health")),
        "drinking at full HP should be refused, not swallowed: {refusals:?}"
    );
    assert!(
        refusals.iter().any(|m| m.contains("for a fight")),
        "a Barrier potion should be refused out of combat: {refusals:?}"
    );
}
