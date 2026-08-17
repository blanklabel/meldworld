//! THE END FIGHT, played rather than modelled (`EW-0`).
//!
//! This encounter had five tuning passes before anything ever ran it, and every one of them
//! was arithmetic against a stat model. That is exactly how it shipped **impossible** (442
//! rounds, a 0.3-round party wipe — the multipliers were sized against a creature 14x
//! smaller than the real one) and then, once fixed, how it shipped **trivial** for one build
//! (four Psykers cleared it in 6 rounds taking no hits, because Foci ignore defence and ride
//! Mnd rather than loot).
//!
//! Neither of those is visible in a spreadsheet. Both are obvious the moment a bot fights it.
//!
//! **What this measures:** rounds to clear, hits taken, and the outcome — for a MARTIAL party
//! and for the all-caster stack that broke it, then the RATIO between them.
//!
//! **Why the boss is scaled down here.** A QA bot dives from the Center Hub at level 1, and
//! the real encounter expects ~level 100 in tier-32 gear. Rather than fake a geared party,
//! the bosses are scaled to what a level-1 bot can actually trade blows with, and the
//! assertion is on the **ratio between builds** — which is scale-invariant and is what the
//! degenerate case actually was. The absolute numbers here are a fixture, not a target;
//! `the_end_fight_is_a_gear_check` in `meld-run` owns the real magnitudes.
//!
//! Requires Postgres: set `MELD_DATABASE_URL` (see qa/scripts/local_pg.sh).

use std::sync::Arc;
use std::time::{Duration, Instant};

use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio::net::TcpListener;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;

/// What one attempt on the end fight looked like.
#[derive(Debug)]
struct Attempt {
    party: &'static str,
    /// Enemies in the encounter the bot actually reached — 3 means it found the set piece.
    enemies: usize,
    /// Turns this bot's own hero took inside the fight. A stand-in for "rounds": the bot
    /// only ever commands hero 0, so its own turn count is the honest unit here.
    my_turns: usize,
    /// HP the party lost across the fight — the danger half, which the 0.3-boss-turns hole
    /// made invisible.
    hp_lost: i32,
    won: bool,
    reached: bool,
}

/// Boot a server with the end fight next to the hub and scaled to a level-1 bot.
async fn start_server(hp: i32, atk: i32) -> String {
    let db_url = std::env::var("MELD_DATABASE_URL")
        .expect("set MELD_DATABASE_URL (see qa/scripts/local_pg.sh)");
    // Pin the world: whether a bot FINDS the encounter is decided by the roll, and an
    // unseeded run is a coin flip on whether this test measures anything at all.
    std::env::set_var("MELD_SEED", "1");
    let mut balance = meld_balance::Balance::load_default().unwrap();
    // The whole point of authoring the bosses with absolute stats (`set_piece`) is that
    // moving the encounter does not change the fight. So: bring it to the hub…
    // Just outside `[ai] hub_safe_radius` (13), so the set piece is the FIRST thing the bot
    // can touch. At 30 it was not: the party met an ordinary creature on the way and was
    // WIPED by it — a level-1 four-hero party loses its first non-tutorial fight, which is
    // its own finding and is why this cannot simply walk further.
    balance.encounters.end_fight_min_distance = 14.0;
    // …and scale it to what a level-1 bot can trade with. See the module note.
    balance.encounters.end_fight_boss_hp = hp;
    balance.encounters.end_fight_boss_atk = atk;
    let config = meld_server::Config {
        bind_addr: "127.0.0.1:0".to_string(),
        database_url: db_url,
        balance: Arc::new(balance),
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

/// Walk `party` into the end fight and report what happened.
async fn attempt(party: &'static str, comp: &[&str], budget: Duration) -> Attempt {
    // Scaled to a LEVEL-1 party (40 HP, 12 atk, 3 def each): ~16 rounds to clear the three,
    // ~13 hits before a hero drops. The real magnitudes live in `meld-run`; these exist so
    // the ratio between builds can be measured at all.
    let addr = start_server(60, 6).await;
    let http = reqwest::Client::new();
    let base = format!("http://{addr}");
    let username = format!("end{}_{}", comp.len(), &uuid::Uuid::new_v4().simple().to_string()[..8]);
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
    let ticket = login["realtime_ticket"].as_str().unwrap().to_string();
    let player_id = login["player"]["player_id"].as_str().unwrap().to_string();

    // Party size is the slots an account has EARNED; `run.enter_maze` clamps a requested
    // composition to what it owns, so without these the whole comp collapses to one hero.
    let db = meld_db::Db::connect(&std::env::var("MELD_DATABASE_URL").unwrap(), 4).await.unwrap();
    let mut keys: Vec<String> = (2..=comp.len()).map(|n| format!("party_slot_{n}")).collect();
    for c in comp {
        keys.push(format!("class_{c}"));
    }
    db.grant_unlocks(uuid::Uuid::parse_str(&player_id).unwrap(), &keys).await.unwrap();

    let (mut ws, _) = connect_async(format!("ws://{addr}/v1/realtime")).await.unwrap();
    let mut seq = 1u32;
    let mut input_seq = 0u32;
    ws.send(Message::Text(
        json!({"type":"session.authenticate","seq":seq,"ts":0,"payload":{"ticket":ticket,"resume":null}})
            .to_string(),
    ))
    .await
    .unwrap();
    seq += 1;

    let mut nav = meld_qa::Nav::default();
    let mut in_battle = false;
    // EVERY hero, not just hero 0. Commanding one and letting the other three fall to the
    // 15s auto-act window makes a four-hero fight roughly four times slower than it should
    // be — the first run of this test entered exactly ONE ordinary fight in 75s and never
    // finished it, which reads as "the encounter is never placed".
    let mut mine: Vec<String> = Vec::new();
    let mut bid = String::new();
    let (mut turns_seen, mut acted) = (0usize, 0usize);
    let mut hp_seen: std::collections::HashMap<String, i32> = std::collections::HashMap::new();
    let mut out = Attempt {
        party,
        enemies: 0,
        my_turns: 0,
        hp_lost: 0,
        won: false,
        reached: false,
    };

    let mut mover = tokio::time::interval(Duration::from_millis(80));
    mover.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let deadline = Instant::now() + budget;
    let comp_json: Vec<Value> = comp.iter().map(|c| json!(c)).collect();

    while Instant::now() < deadline {
        tokio::select! {
            _ = mover.tick(), if !in_battle => {
                let (dx, dy) = nav.heading(0);
                input_seq += 1;
                ws.send(Message::Text(json!({"type":"movement.move_intent","seq":seq,"ts":0,
                    "payload":{"input_seq":input_seq,"move_dir":{"x":dx,"y":dy},"client_pos":{"x":0.0,"y":0.0}}
                }).to_string())).await.unwrap();
                seq += 1;
            }
            msg = ws.next() => {
                let Some(Ok(Message::Text(t))) = msg else { break };
                let v: Value = serde_json::from_str(&t).unwrap();
                match v["type"].as_str().unwrap_or("") {
                    "session.authenticated" => {
                        // Never the tutorial: it forces the on-ramp world, which is
                        // explicitly forbidden from holding the end fight.
                        ws.send(Message::Text(json!({"type":"run.enter_maze","seq":seq,"ts":0,
                            "payload":{"tutorial":false,"party":comp_json}}).to_string())).await.unwrap();
                        seq += 1;
                    }
                    "world.snapshot" => nav.observe(&v["payload"], &player_id),
                    "run.ended" => {
                        eprintln!(
                            "[end_fight]   {party} RUN ENDED result={:?} — cannot reach the set piece",
                            v["payload"]["result"].as_str()
                        );
                        break;
                    }
                    "battle.started" => {
                        in_battle = true;
                        mine = v["payload"]["your_combatant_ids"]
                            .as_array()
                            .into_iter()
                            .flatten()
                            .filter_map(|c| c.as_str().map(str::to_string))
                            .collect();
                        bid = v["payload"]["battle_id"].as_str().unwrap_or_default().to_string();
                        let enemies =
                            v["payload"]["enemies"].as_array().map(|a| a.len()).unwrap_or(0);
                        let class = v["payload"]["encounter_class"].as_str().unwrap_or("");
                        // `Combatant` carries no `name` — the display name rides `statuses`
                        // — so the set piece is recognised by its CLASS and its size. Near
                        // the hub that is unambiguous: a real Gatekeeper cannot spawn inside
                        // `gatekeeper_min_distance` (300), and nothing else reports
                        // Gatekeeper-class three-strong. Two earlier guesses (a space in
                        // every name, then the boss roster by name) both silently never
                        // fired, and a detector that never fires reads exactly like an
                        // encounter that is never placed.
                        let set_piece = enemies == 3 && class.eq_ignore_ascii_case("gatekeeper");
                        let allies =
                            v["payload"]["allies"].as_array().map(|a| a.len()).unwrap_or(0);
                        let lv = v["payload"]["enemies"][0]["level"].as_i64().unwrap_or(0);
                        let ehp = v["payload"]["enemies"][0]["max_hp"].as_i64().unwrap_or(0);
                        eprintln!(
                            "[end_fight]   {party} entered: {enemies} enemies (lv{lv} {ehp}hp),                              {allies} allies, class={class}, set_piece={set_piece}"
                        );
                        if set_piece {
                            out.reached = true;
                            out.enemies = enemies;
                            out.my_turns = 0;
                            out.hp_lost = 0;
                            hp_seen.clear();
                        }
                    }
                    "battle.turn_ready" => {
                        let who = v["payload"]["combatant_id"].as_str().unwrap_or("");
                        turns_seen += 1;
                        if !mine.iter().any(|m| m == who) {
                            eprintln!(
                                "[end_fight]   {party} turn_ready for {who} NOT in mine={mine:?}"
                            );
                            continue;
                        }
                        acted += 1;
                        if out.reached {
                            out.my_turns += 1;
                        }
                        let target = v["payload"]["valid_targets"]
                            .as_array()
                            .and_then(|a| a.first())
                            .and_then(|t| t.as_str())
                            .unwrap_or("")
                            .to_string();
                        // An empty `target_ids` is REJECTED and the hero keeps its turn
                        // until the 15s auto-act window burns — which reads as "the fight
                        // is unwinnable" rather than as a bot bug.
                        ws.send(Message::Text(json!({"type":"battle.submit_action","seq":seq,"ts":0,
                            "payload":{"battle_id":bid,"action_id":uuid::Uuid::new_v4().to_string(),
                                       "action":"attack","skill_kind":null,"item_id":null,"target_ids":[target]}
                        }).to_string())).await.unwrap();
                        seq += 1;
                    }
                    // Track the party's HP downward: the danger half of this encounter is
                    // exactly what the "bosses act 0.3 times" hole made invisible.
                    "battle.gauge_update" if out.reached => {
                        for c in v["payload"]["combatants"].as_array().into_iter().flatten() {
                            let Some(id) = c["combatant_id"].as_str() else { continue };
                            if !c["is_player"].as_bool().unwrap_or(false) {
                                continue;
                            }
                            let hp = c["hp"].as_i64().unwrap_or(0) as i32;
                            let prev = *hp_seen.get(id).unwrap_or(&hp);
                            if hp < prev {
                                out.hp_lost += prev - hp;
                            }
                            hp_seen.insert(id.to_string(), hp);
                        }
                    }
                    "session.error" if v["payload"]["code"] == json!("validation_error") => {
                        panic!("server refused a bot action: {}", v["payload"]);
                    }
                    "battle.ended" => {
                        in_battle = false;
                        eprintln!(
                            "[end_fight]   {party} battle.ended outcome={:?} reached={} turns_seen={turns_seen} acted={acted}",
                            v["payload"]["outcome"].as_str(), out.reached
                        );
                        if out.reached {
                            out.won = v["payload"]["outcome"].as_str() == Some("victory");
                            break;
                        }
                    }
                    _ => {}
                }
            }
        }
    }
    out
}

/// The end fight, fought by the party it is designed for and by the stack that broke it.
///
/// The assertion is the **ratio**, not the absolute rounds: an all-caster stack should be
/// good at this — it is a real build and the Psyker earned its kit — but it must not make
/// the encounter a formality next to the party the fight is tuned for. Before the wards it
/// was 4x faster and took no damage at all.
///
/// **`#[ignore]` — this does not measure the end fight yet, and says why rather than
/// passing on nothing.** Two things stop it, both found BY it:
///
/// 1. **A fresh party cannot survive the walk.** A level-1 four-hero party, acting on every
///    single turn it is given (24 of 24), *loses* to one level-2 216 HP creature at d14 in a
///    non-tutorial world. It is close — ~240 damage dealt into 216 HP before dodges — but it
///    loses, so the bot never reaches the set piece however near the hub it is placed. The
///    tutorial's on-ramp is doing more work than anyone had measured.
/// 2. **The bot cannot drive a Psyker.** Sending `action: "attack"` resolves through
///    `resolve_psyker` with no op, which is `hold` — a Psyker party does literally nothing
///    (13 turns, no damage). Measuring the caster stack that broke this encounter needs the
///    bot to cast Foci, which is real work and is the next piece.
///
/// What IS built here: a party that walks, finds encounters, commands every hero, and
/// reports turns / HP lost / outcome. Un-ignore it once the bot can cast and the party can
/// be handed a level.
#[ignore = "cannot reach the end fight yet: a level-1 party loses its first non-tutorial             fight, and the bot cannot cast Foci — see the doc comment"]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_end_fight_is_not_trivialised_by_one_build() {
    // Generous on purpose: the bot has to walk out, clear whatever ordinary creatures sit
    // between the hub and d30, and then fight three bosses. `pacing_arc` measures ~50s for a
    // single four-hero fight, so anything tight here fails on the stopwatch rather than on
    // the balance it is meant to be measuring.
    let budget = Duration::from_secs(240);
    let martial = attempt("hunter x4", &["hunter", "hunter", "hunter", "hunter"], budget).await;
    let casters = attempt("psyker x4", &["psyker", "psyker", "psyker", "psyker"], budget).await;

    for a in [&martial, &casters] {
        eprintln!(
            "[end_fight] {:9} reached={} enemies={} my_turns={} hp_lost={} won={}",
            a.party, a.reached, a.enemies, a.my_turns, a.hp_lost, a.won
        );
    }

    // Finding it at all is the floor. If this fails the encounter is not being placed or
    // the bot cannot walk to it — either way the measurements below mean nothing, so say
    // which it is rather than asserting on zeroes.
    assert!(
        martial.reached || casters.reached,
        "neither party reached the end fight — placement or navigation, not balance"
    );
    for a in [&martial, &casters] {
        if a.reached {
            assert_eq!(a.enemies, 3, "{} met {} enemies, not three", a.party, a.enemies);
        }
    }

    // The danger has to be real: an encounter nobody takes damage from is the hole the
    // slow floor was added to close.
    if casters.reached {
        assert!(
            casters.hp_lost > 0,
            "the caster stack took NO damage — control removed the fight again"
        );
    }

    // …and if both reached it, neither build may make it a formality relative to the other.
    if martial.reached && casters.reached && martial.my_turns > 0 && casters.my_turns > 0 {
        let ratio = martial.my_turns as f64 / casters.my_turns as f64;
        eprintln!("[end_fight] martial/caster turn ratio: {ratio:.2}");
        assert!(
            ratio < 3.0,
            "the caster stack cleared it {ratio:.1}x faster than the party the fight is \
             tuned for — one build trivialises the apex"
        );
    }
}
