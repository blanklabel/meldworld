//! `meld-mcp` — MELDWORLD as a set of tools, so an agent can PLAY it rather than model it.
//!
//! Five tuning passes on the end fight were arithmetic against a stat model, and the two
//! things wrong with it — impossible, then trivial for one build — were invisible in every
//! one of them and obvious in the first fight. A `qa/` test can measure a question you
//! already know to ask; this measures the ones you don't, because it lets whoever is
//! holding the controller look around, walk somewhere, and try something.
//!
//! It speaks MCP over stdio (JSON-RPC 2.0, one object per line). The framing is hand-rolled
//! against `serde_json` rather than pulled from a crate: it is about sixty lines, and the
//! alternative is a dependency in a workspace whose whole build story is "no network".
//!
//! **stdout is the protocol.** Anything printed there that is not a JSON-RPC response
//! corrupts the stream, which is why the tracing subscriber is pinned to stderr below.

mod session;
mod view;

use std::time::Duration;

use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

use session::{Boot, Session, Steer};

const PROTOCOL: &str = "2025-06-18";

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "warn".into()),
        )
        .init();

    let mut lines = BufReader::new(tokio::io::stdin()).lines();
    let mut stdout = tokio::io::stdout();
    let mut game: Option<Session> = None;

    while let Ok(Some(line)) = lines.next_line().await {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(req) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let id = req.get("id").cloned();
        let method = req["method"].as_str().unwrap_or("").to_string();
        let params = req.get("params").cloned().unwrap_or(json!({}));

        // A notification has no id and MUST NOT be answered — replying to
        // `notifications/initialized` is the classic way to wedge a handshake.
        if id.is_none() {
            continue;
        }

        let result = match method.as_str() {
            "initialize" => Ok(json!({
                "protocolVersion": params["protocolVersion"].as_str().unwrap_or(PROTOCOL),
                "capabilities": { "tools": {} },
                "serverInfo": { "name": "meldworld", "version": env!("CARGO_PKG_VERSION") },
                "instructions": "Boot a world with new_game, then look/walk/battle/act. \
                                 Everything runs against a real in-process server over the \
                                 real wire protocol."
            })),
            "ping" => Ok(json!({})),
            "tools/list" => Ok(json!({ "tools": tool_schemas() })),
            "resources/list" => Ok(json!({ "resources": [] })),
            "prompts/list" => Ok(json!({ "prompts": [] })),
            "tools/call" => {
                let name = params["name"].as_str().unwrap_or("").to_string();
                let args = params.get("arguments").cloned().unwrap_or(json!({}));
                match call(&mut game, &name, &args).await {
                    Ok(text) => Ok(json!({
                        "content": [{ "type": "text", "text": text }],
                        "isError": false
                    })),
                    Err(text) => Ok(json!({
                        "content": [{ "type": "text", "text": text }],
                        "isError": true
                    })),
                }
            }
            other => Err(format!("unknown method {other}")),
        };

        let resp = match result {
            Ok(r) => json!({ "jsonrpc": "2.0", "id": id, "result": r }),
            Err(e) => json!({ "jsonrpc": "2.0", "id": id,
                              "error": { "code": -32601, "message": e } }),
        };
        let _ = stdout.write_all(format!("{resp}\n").as_bytes()).await;
        let _ = stdout.flush().await;
    }
}

fn tool_schemas() -> Value {
    json!([
        {
            "name": "new_game",
            "description": "Boot a fresh world and dive into it. Drops any game in progress. \
                            Returns what you can see from the hub.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "party": { "type": "array", "items": { "type": "string" },
                               "description": "Up to 4 class keys: explorer, hunter, psyker, resonant, shifter, phoenix_guard, smithwright, keeper. Default explorer/hunter/psyker/resonant." },
                    "seed": { "type": "integer", "description": "Pin the world layout (MELD_SEED) so a run is repeatable." },
                    "tutorial": { "type": "boolean", "description": "Dive the guided on-ramp world instead of a normal randomized one." },
                    "start_level": { "type": "integer", "description": "DEV: start every hero at this level. The deep content is authored for ~100 and walking there takes an hour." },
                    "gear_tier": { "type": "integer", "description": "DEV: dress every hero in a full six-slot set of tier-n insured epics." },
                    "end_fight_at": { "type": "number", "description": "DEV: place the END FIGHT at this distance instead of d3200. Must exceed the hub safe radius (13)." },
                    "biome": { "type": "string", "description": "DEV: pin every section to one biome (forest, desert, ashfall, tundra, mire)." },
                    "ward": { "type": "string", "description": "DEV: dress the gear set with an elemental ward (FIRE, ICE, MIND, …) — what a prepared party brings. Armour WEIGHT only answers slash/blunt/pierce." },
                    "potions": { "type": "integer", "description": "DEV: deal this many salves AND elixirs into the pouches instead of the 3/1 starting kit — a party that shopped before diving. A salve heals 40% of max HP." }
                }
            }
        },
        {
            "name": "look",
            "description": "Where you are, what stands near you (with the entity ids the other tools take), your party, and your backpack.",
            "inputSchema": { "type": "object", "properties": {} }
        },
        {
            "name": "walk",
            "description": "Walk, and stop when something happens: a battle starts, the target is reached, the run ends, or the budget runs out. Returns what you can see when you stop.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "toward": { "type": "string", "description": "An entity id from look. Steering re-aims each tick, so a roaming creature is still caught." },
                    "dir": { "type": "string", "description": "A compass heading (e, ne, n, nw, w, sw, s, se) when you just want to explore. East is outward." },
                    "max_ms": { "type": "integer", "description": "Walking budget in ms (default 8000, max 120000)." }
                }
            }
        },
        {
            "name": "battle",
            "description": "The arena: both sides with hp, gauges and conditions, and whose 15-second window is open.",
            "inputSchema": { "type": "object", "properties": {} }
        },
        {
            "name": "abilities",
            "description": "What each hero can press right now — the registry's kit at that hero's level, aspects included.",
            "inputSchema": { "type": "object", "properties": {} }
        },
        {
            "name": "act",
            "description": "Command one hero. Waits for that hero's gauge to fill (up to wait_ms), submits, and returns what happened plus the new arena state.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "hero": { "description": "Party slot (0-based) or hero name. Default: whichever of yours is waiting." },
                    "action": { "type": "string", "enum": ["attack", "skill", "defend", "flee", "item"],
                                "description": "Default attack, or skill when a skill is named." },
                    "skill": { "type": "string", "description": "A skill/manifestation key from abilities." },
                    "item": { "type": "string", "description": "A consumable key, for action=item." },
                    "target": { "description": "Enemy index (0-based, from battle) or a name. Left out, the server's own default aim is used: the weakest enemy or the most wounded ally." },
                    "wait_ms": { "type": "integer", "description": "How long to wait for the turn (default 20000)." }
                }
            }
        },
        {
            "name": "auto_battle",
            "description": "Play the current fight out to the end on a simple policy and report rounds, damage taken and outcome. For measuring rather than deciding.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "policy": { "type": "string", "enum": ["attack", "kit"],
                                "description": "attack = basic attacks only. kit = each hero's best unlocked skill, falling back to attack. Default kit." },
                    "max_ms": { "type": "integer", "description": "Give up after this long (default 120000)." }
                }
            }
        },
        {
            "name": "travel",
            "description": "March outward toward a distance, FIGHTING what gets in the way, and report the arc: depth reached, fights won/lost, hp and potions spent, levels gained. This is how you measure the middle game — walk stops at every fight and auto_battle plays only one.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "to_distance": { "type": "integer", "description": "Stop on reaching this distance from the origin (default 500). Creature level is roughly distance/12.5, so d500 is level-40 country." },
                    "policy": { "type": "string", "enum": ["attack", "kit"], "description": "How to fight what it meets. Default kit." },
                    "max_ms": { "type": "integer", "description": "Give up after this long (default 300000, max 3600000)." }
                }
            }
        },
        {
            "name": "interact",
            "description": "The world verbs [E] covers, named explicitly: harvest a node, open a chest, descend an entrance, extract, pin a creature (Psyker), join a teammate's fight, drink a potion.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "verb": { "type": "string",
                              "enum": ["harvest", "chest", "descend", "extract", "town_portal", "hold", "join", "use_item", "cancel"] },
                    "entity_id": { "type": "string", "description": "The thing being acted on, for the verbs that take one." },
                    "item": { "type": "string", "description": "Consumable key, for use_item." },
                    "hero": { "type": "integer", "description": "Party slot that drinks it, for use_item." }
                },
                "required": ["verb"]
            }
        },
        {
            "name": "say",
            "description": "Say something to the other people on the server. They see it in their chat; whatever they say back arrives in your next tool result.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "text": { "type": "string", "description": "What to say (up to 400 characters)." },
                    "channel": { "type": "string", "enum": ["party", "world"],
                                 "description": "party = the people you are among (in the maze with you, or in town with you). world = everyone connected. Default party." }
                },
                "required": ["text"]
            }
        },
        {
            "name": "chat",
            "description": "The chat transcript so far — kept whole, so you can scroll back on what somebody said rather than hunting for it in the event log.",
            "inputSchema": { "type": "object", "properties": {} }
        },
        {
            "name": "wait",
            "description": "Let the world run for a while and report everything that happened. Creatures roam and close on you while you stand still.",
            "inputSchema": {
                "type": "object",
                "properties": { "ms": { "type": "integer", "description": "Default 3000, max 60000." } }
            }
        }
    ])
}

async fn call(game: &mut Option<Session>, name: &str, args: &Value) -> Result<String, String> {
    if name == "new_game" {
        let mut boot = Boot::default();
        if let Some(p) = args["party"].as_array() {
            let picked: Vec<String> = p
                .iter()
                .filter_map(|c| c.as_str().map(|s| s.to_lowercase()))
                .collect();
            if !picked.is_empty() {
                boot.party = picked;
            }
        }
        boot.seed = args["seed"].as_u64();
        boot.tutorial = args["tutorial"].as_bool().unwrap_or(false);
        boot.start_level = args["start_level"].as_i64().map(|v| v as i32);
        boot.gear_tier = args["gear_tier"].as_i64().map(|v| v as i32);
        boot.end_fight_at = args["end_fight_at"].as_f64();
        boot.biome = args["biome"].as_str().map(String::from);
        boot.potions = args["potions"].as_i64().map(|v| v as i32);
        boot.ward = args["ward"].as_str().map(|w| w.to_uppercase());
        boot.dungeon = args["dungeon"].as_str().map(String::from);
        // Drop the old game FIRST: its server is holding a port and a game loop, and two
        // live sessions would both be reading the process-global MELD_* overrides.
        *game = None;
        let s = Session::start(boot).await?;
        let text = {
            let mut st = s.state.lock().await;
            let log = st.drain_log();
            let b = &s.boot;
            let mut how = vec![format!("party {}", b.party.join("/"))];
            if let Some(v) = b.seed {
                how.push(format!("seed {v}"));
            }
            if b.tutorial {
                how.push("tutorial".into());
            }
            for (label, v) in [
                ("start_level", b.start_level),
                ("gear_tier", b.gear_tier),
                ("potions", b.potions),
            ] {
                if let Some(v) = v {
                    how.push(format!("{label} {v}"));
                }
            }
            if let Some(v) = b.end_fight_at {
                how.push(format!("end_fight at d{v}"));
            }
            if let Some(v) = &b.biome {
                how.push(format!("biome {v}"));
            }
            if let Some(v) = &b.ward {
                how.push(format!("ward {v}"));
            }
            format!(
                "new game on {} — {}\n{}\n{}",
                s.addr,
                how.join(", "),
                log.join("\n"),
                view::look(&st)
            )
        };
        *game = Some(s);
        return Ok(text);
    }

    let s = game.as_ref().ok_or("no game — call new_game first")?;

    match name {
        "look" => {
            let mut st = s.state.lock().await;
            let log = st.drain_log();
            Ok(format!("{}{}", view::look(&st), tail(&log)))
        }
        "abilities" => {
            let st = s.state.lock().await;
            Ok(view::abilities(&st))
        }
        "battle" => {
            let mut st = s.state.lock().await;
            let log = st.drain_log();
            let b = st.battle.clone().or_else(|| st.last_battle.clone());
            match b {
                Some(b) => Ok(format!("{}{}", view::battle(&b), tail(&log))),
                None => Ok(format!("not in a fight.\n{}", tail(&log))),
            }
        }
        "wait" => {
            let ms = args["ms"].as_u64().unwrap_or(3000).min(60_000);
            s.wait_until(Duration::from_millis(ms), |_| false).await;
            let mut st = s.state.lock().await;
            let log = st.drain_log();
            Ok(format!("{}{}", view::look(&st), tail(&log)))
        }
        "say" => {
            let text = args["text"].as_str().ok_or("say needs text")?;
            let channel = args["channel"].as_str().unwrap_or("party");
            s.send(json!({"type":"chat.say","payload":{"text": text, "channel": channel}}));
            // Wait for the server's ECHO rather than reporting the send: a line that was
            // refused (empty, or a session the server has since dropped) otherwise reads
            // as delivered, which is the one thing a chat box must never do.
            let said = s
                .wait_until(Duration::from_millis(2000), |st| {
                    st.chat.iter().any(|l| l.contains(text))
                })
                .await;
            let mut st = s.state.lock().await;
            let log = st.drain_log();
            Ok(format!(
                "{}\n{}{}",
                if said { "said." } else { "the server did not echo that back — it was not delivered." },
                st.chat.join("\n"),
                tail(&log)
            ))
        }
        "chat" => {
            let st = s.state.lock().await;
            Ok(if st.chat.is_empty() {
                "nothing said yet.".to_string()
            } else {
                st.chat.join("\n")
            })
        }
        "walk" => walk(s, args).await,
        "act" => act(s, args).await,
        "auto_battle" => auto_battle(s, args).await,
        "travel" => travel(s, args).await,
        "interact" => interact(s, args).await,
        other => Err(format!("unknown tool {other}")),
    }
}

fn tail(log: &[String]) -> String {
    if log.is_empty() {
        String::new()
    } else {
        format!("\nsince last look:\n  {}\n", log.join("\n  "))
    }
}

async fn walk(s: &Session, args: &Value) -> Result<String, String> {
    let budget = Duration::from_millis(args["max_ms"].as_u64().unwrap_or(8000).min(120_000));
    let toward = args["toward"].as_str().map(String::from);
    let steer = match (&toward, args["dir"].as_str()) {
        (Some(id), _) => Steer::Toward(id.clone()),
        (None, Some(d)) => {
            let (x, y) = compass(d).ok_or_else(|| format!("'{d}' is not a compass heading"))?;
            Steer::Dir(x, y)
        }
        _ => return Err("walk needs `toward` (an entity id) or `dir` (a heading)".into()),
    };
    s.steer(steer);
    let target = toward.clone();
    let stopped = s
        .wait_until(budget, |st| {
            if st.in_battle() || !st.run_active {
                return true;
            }
            // Tighter than it looks like it should be, because `touch_radius_tiles` is
            // 1.0: stopping at a comfortable-looking 4 units from a creature stops you
            // just short of the contact that starts the fight, and "walked at it and
            // nothing happened" is indistinguishable from a broken world.
            match &target {
                Some(id) => st.ent(id).is_some_and(|e| e.dist_to(st.pos) < 1.2),
                None => false,
            }
        })
        .await;
    s.steer(Steer::Stop);

    let mut st = s.state.lock().await;
    let log = st.drain_log();
    let why = if !st.run_active {
        "the run ended"
    } else if st.in_battle() {
        "a fight started"
    } else if stopped {
        "you are there"
    } else {
        "the walking budget ran out"
    };
    let body = match &st.battle {
        Some(b) => view::battle(b),
        None => view::look(&st),
    };
    Ok(format!("stopped: {why}\n{body}{}", tail(&log)))
}

fn compass(d: &str) -> Option<(f64, f64)> {
    let r = std::f64::consts::FRAC_1_SQRT_2;
    Some(match d.to_lowercase().as_str() {
        "e" | "east" => (1.0, 0.0),
        "w" | "west" => (-1.0, 0.0),
        "n" | "north" => (0.0, 1.0),
        "s" | "south" => (0.0, -1.0),
        "ne" => (r, r),
        "nw" => (-r, r),
        "se" => (r, -r),
        "sw" => (-r, -r),
        _ => return None,
    })
}

/// Which of my heroes this command is for, and which combatant it aims at.
async fn act(s: &Session, args: &Value) -> Result<String, String> {
    let wait = Duration::from_millis(args["wait_ms"].as_u64().unwrap_or(20_000).min(120_000));
    let skill = args["skill"].as_str().map(String::from);
    let action = args["action"]
        .as_str()
        .map(String::from)
        .unwrap_or_else(|| if skill.is_some() { "skill".into() } else { "attack".into() });
    let hero_pick = args.get("hero").cloned().unwrap_or(Value::Null);

    // Wait for the WINDOW, not for the fight: a hero whose gauge is not full cannot be
    // commanded, and submitting anyway is silently dropped by the engine. When a hero is
    // NAMED, wait for that one — waiting for "anybody" and then insisting on the named
    // one turns "your Psyker is still filling" into a hard error.
    let ready = s
        .wait_until(wait, |st| {
            let Some(b) = st.battle.as_ref() else { return true };
            match pick_hero(b, &hero_pick) {
                Some(_) => true,
                None => matches!(hero_pick, Value::Null) && !b.ready.is_empty(),
            }
        })
        .await;
    if !ready {
        let st = s.state.lock().await;
        return Ok(format!(
            "no hero's gauge filled within the wait.\n{}",
            st.battle.as_ref().map(view::battle).unwrap_or_default()
        ));
    }

    let (battle_id, actor, target, kind) = {
        let st = s.state.lock().await;
        let b = st
            .battle
            .as_ref()
            .ok_or_else(|| "not in a fight (it ended while waiting)".to_string())?;
        let actor = pick_hero(b, &hero_pick).ok_or("no such hero, or that hero is not waiting")?;
        let kind = b.get(&actor).map(|c| c.class.clone()).unwrap_or_default();
        let target = pick_target(b, args.get("target"), skill.as_deref());
        (b.id.clone(), actor, target, kind)
    };
    // A Psyker's `skill_kind` is an OP, not an ability key: `resolve_psyker` splits it on
    // `:` and anything it does not recognise falls through to `hold` — so sending the bare
    // key spends the turn doing nothing, silently and successfully. That is exactly what a
    // Psyker party looked like from the outside for two rounds of measurement.
    let skill = match (kind.as_str(), skill) {
        ("psyker", Some(k)) if !k.contains(':') => {
            let st = s.state.lock().await;
            let held = st
                .battle
                .as_ref()
                .and_then(|b| b.get(&actor))
                .map(|c| c.statuses.iter().any(|s| s.starts_with(&format!("focus:{k}:"))))
                .unwrap_or(false);
            Some(format!("{}:{k}", if held { "reinforce" } else { "cast" }))
        }
        (_, other) => other,
    };

    let payload = json!({
        "battle_id": battle_id,
        "action_id": uuid::Uuid::new_v4().to_string(),
        "action": action,
        "actor_combatant_id": actor,
        "skill_kind": skill,
        "item_id": args["item"].as_str(),
        "target_ids": target.clone().map(|t| vec![t]),
    });
    s.send(json!({ "type": "battle.submit_action", "payload": payload }));

    // The submission is acknowledged by a resolution, not by the send — and a REFUSED one
    // (a locked skill, an unbanked Adrenaline cost) produces no resolution at all, which
    // is exactly the case worth reporting rather than hiding behind a timeout.
    let resolved = s
        .wait_until(Duration::from_millis(4000), |st| {
            !st.in_battle() || st.battle.as_ref().is_some_and(|b| !b.ready.contains(&actor))
        })
        .await;

    let mut st = s.state.lock().await;
    let log = st.drain_log();
    let body = match &st.battle {
        Some(b) => view::battle(b),
        None => st.last_battle.as_ref().map(view::battle).unwrap_or_default(),
    };
    let head = if resolved {
        String::new()
    } else {
        format!(
            "the server did not resolve that — a {kind} may not have {} available \
             (locked, or its cost is not banked).\n",
            skill.unwrap_or_else(|| action.clone())
        )
    };
    Ok(format!("{head}{body}{}", tail(&log)))
}

/// The hero a command is for: an explicit slot or name, else whoever is waiting.
fn pick_hero(b: &session::Battle, pick: &Value) -> Option<String> {
    let mine: Vec<&session::Comb> = b.mine().into_iter().filter(|c| c.alive()).collect();
    let chosen = match pick {
        Value::Number(n) => mine.get(n.as_u64().unwrap_or(0) as usize).map(|c| c.id.clone()),
        Value::String(s) => mine
            .iter()
            .find(|c| c.id == *s || c.name.eq_ignore_ascii_case(s))
            .map(|c| c.id.clone()),
        _ => None,
    };
    match chosen {
        // A named hero still has to be the one whose window is open; falling through to
        // "somebody else then" would silently command the wrong hero.
        Some(id) if b.ready.contains(&id) => Some(id),
        Some(_) => None,
        None => b.ready.first().cloned(),
    }
}

/// What the action aims at. `None` means "let the server aim", which is what every
/// resolver already does well (weakest enemy, most wounded ally).
fn pick_target(b: &session::Battle, pick: Option<&Value>, skill: Option<&str>) -> Option<String> {
    let enemies: Vec<&session::Comb> = b.enemies().into_iter().filter(|c| c.alive()).collect();
    match pick {
        Some(Value::Number(n)) => {
            enemies.get(n.as_u64().unwrap_or(0) as usize).map(|c| c.id.clone())
        }
        Some(Value::String(s)) => b
            .combatants
            .iter()
            .find(|c| c.id == *s || c.name.eq_ignore_ascii_case(s))
            .map(|c| c.id.clone()),
        _ => {
            use meld_proto::skills::Target;
            match skill.map(meld_proto::skills::target_of) {
                Some(Target::Ally) => b
                    .mine()
                    .into_iter()
                    .filter(|c| c.alive())
                    .min_by_key(|c| c.hp)
                    .map(|c| c.id.clone()),
                Some(Target::Caster) | Some(Target::AllEnemies) | Some(Target::Party) => None,
                _ => enemies.iter().min_by_key(|c| c.hp).map(|c| c.id.clone()),
            }
        }
    }
}

/// Play the fight in progress to its end, on the policy. Shared by `auto_battle` (measure one
/// encounter) and `travel` (measure a journey), because a journey is mostly fights and a
/// second copy of this loop is a second place for the policy to drift.
async fn play_out_battle(s: &Session, use_kit: bool, deadline: tokio::time::Instant) {
    loop {
        let now = tokio::time::Instant::now();
        if now >= deadline {
            break;
        }
        let left = (deadline - now).min(Duration::from_secs(20));
        let acted = s
            .wait_until(left, |st| {
                !st.in_battle() || st.battle.as_ref().is_some_and(|b| !b.ready.is_empty())
            })
            .await;
        let order = {
            let st = s.state.lock().await;
            let pouches = st.pouches.clone();
            let Some(b) = st.battle.as_ref() else { break };
            match b.ready.first().cloned() {
                None if !acted => break,
                None => continue,
                Some(actor) => {
                    let me = b.get(&actor).cloned();
                    let party = b.mine();
                    // The pouch belongs to the hero's party SLOT, and `mine()` is in slot
                    // order — so the acting hero's index in it is the index into pouches.
                    let slot = party.iter().position(|c| c.id == actor).unwrap_or(0);
                    let pouch = pouches.get(slot).cloned().unwrap_or_default();
                    let ord = me
                        .as_ref()
                        .filter(|_| use_kit)
                        .map(|c| choose_order(c, &party, &pouch))
                        .unwrap_or(Order::Attack);
                    let mut poured: Option<String> = None;
                    let (action, skill, item) = match ord {
                        Order::Attack => ("attack", None, None),
                        Order::Drink { item, target } => {
                            poured = Some(target);
                            ("item", None, Some(item))
                        }
                        Order::Skill(k) => {
                            let held = me
                                .as_ref()
                                .map(|c| {
                                    c.statuses.iter().any(|s| s.starts_with(&format!("focus:{k}:")))
                                })
                                .unwrap_or(false);
                            let wrapped = match me.as_ref().map(|c| c.class.as_str()) {
                                Some("psyker") if held => format!("reinforce:{k}"),
                                Some("psyker") => format!("cast:{k}"),
                                _ => k,
                            };
                            ("skill", Some(wrapped), None)
                        }
                    };
                    // The bare key is what aiming reads; `cast:`/`reinforce:` is a Psyker
                    // op wrapper and `target_of` knows nothing about it.
                    let aim = skill.as_deref().map(|x| x.rsplit(':').next().unwrap_or(x));
                    let target = poured.or_else(|| pick_target(b, None, aim));
                    (b.id.clone(), actor, action, skill, item, target)
                }
            }
        };
        let (bid, actor, action, skill, item, target) = order;
        s.send(json!({ "type": "battle.submit_action", "payload": {
            "battle_id": bid,
            "action_id": uuid::Uuid::new_v4().to_string(),
            "action": action,
            "actor_combatant_id": actor,
            "skill_kind": skill,
            "item_id": item,
            "target_ids": target.map(|t| vec![t]),
        }}));
        s.wait_until(Duration::from_millis(3000), |st| {
            !st.in_battle() || st.battle.as_ref().is_some_and(|b| !b.ready.contains(&actor))
        })
        .await;
    }

}

async fn auto_battle(s: &Session, args: &Value) -> Result<String, String> {
    let budget = Duration::from_millis(args["max_ms"].as_u64().unwrap_or(120_000).min(600_000));
    let use_kit = args["policy"].as_str().unwrap_or("kit") == "kit";
    let deadline = tokio::time::Instant::now() + budget;

    let opening = {
        let st = s.state.lock().await;
        st.battle.as_ref().map(|b| b.opening_hp).unwrap_or(0)
    };
    play_out_battle(s, use_kit, deadline).await;

    let mut st = s.state.lock().await;
    let log = st.drain_log();
    let b = st.battle.clone().or_else(|| st.last_battle.clone());
    let Some(b) = b else {
        return Ok(format!("no fight to play out.\n{}", tail(&log)));
    };
    Ok(format!(
        "{}\nturns taken: {}   hp lost: {}   outcome: {}\n{}",
        view::battle(&b),
        b.my_turns,
        opening.max(b.opening_hp) - b.party_hp(),
        b.ended.clone().unwrap_or_else(|| "still going".into()),
        tail(&log)
    ))
}

/// What this hero should do with its turn.
enum Order {
    Attack,
    Skill(String),
    /// A bottle, and WHO it is poured into — which need not be the hero holding it. The
    /// engine has always honoured an ally target (`ally_target(target_id)`) and the client's
    /// Item row opens the ally picker, so triage is a real player option; a policy that only
    /// ever drinks on the acting hero leaves heroes dying with full pouches, and makes
    /// stocking up worth nothing (measured: 8 potions each performed the same as 3).
    Drink { item: String, target: String },
}

/// What this hero should do. Not a strategy — but it has to be competent enough that a
/// measurement taken with it is a measurement of the FIGHT.
///
/// Four rules, each of which a naive "cast the deepest thing you own" got wrong badly
/// enough to move the numbers by more than the tuning being measured:
///
/// - A hero about to die **drinks**. A Bloom Salve is `item_heal_fraction` (40%) of max HP
///   and an Elixir is a full one, dealt into the pouches at dive start — so a policy that
///   never drinks is throwing away roughly a third of the party's effective HP and calling
///   the result the fight's difficulty.
/// - A **once-a-fight** call is gone once spent (`spent:<key>` rides the wire). Re-pressing
///   it burns the turn on `already used this battle`.
/// - A **Hunter** pays in Adrenaline it earns by ATTACKING. Opening with its capstone means
///   every turn is refused and the class contributes nothing at all.
/// - A party that is dying wants its **healer** to heal.
fn choose_order(me: &session::Comb, party: &[&session::Comb], pouch: &[(String, i64)]) -> Order {
    use meld_proto::skills::Target;
    let tok = |p: &str| me.statuses.iter().find_map(|s| s.strip_prefix(p));
    let spent = |k: &str| me.statuses.iter().any(|s| s == &format!("spent:{k}"));
    let owned = meld_proto::skills::skills_for_class_at(&me.class, me.level);
    let carrying = |k: &str| pouch.iter().any(|(kind, n)| kind == k && *n > 0);

    // A potion heals a FRACTION of the drinker's own max HP, so the same bottle is worth
    // 417 on the 1042 HP Phoenix Guard and 113 on the 282 HP Psyker. Pouring it into
    // whoever is proportionally worst off therefore spends it where it buys the least —
    // measured, that policy turned a victory with a hero standing into a defeat at 67%.
    // So: among the heroes actually in danger, pour it where the most HP comes back, and
    // never waste the overflow on someone barely scratched.
    const DANGER: f64 = 0.6;
    let restored = |c: &session::Comb| {
        (((c.max_hp as f64) * 0.4).round() as i32).min(c.max_hp - c.hp)
    };
    let best = party
        .iter()
        .filter(|c| c.alive() && c.max_hp > 0)
        .filter(|c| (c.hp as f64) < DANGER * c.max_hp as f64)
        .max_by_key(|c| restored(c));
    if let Some(w) = best {
        for k in ["bloom_salve", "elixir"] {
            if carrying(k) {
                return Order::Drink { item: k.to_string(), target: w.id.clone() };
            }
        }
    }

    let hurt = party
        .iter()
        .filter(|c| c.alive() && c.max_hp > 0)
        .any(|c| (c.hp as f64) < 0.6 * c.max_hp as f64);
    if hurt {
        if let Some(sk) = owned
            .iter()
            .filter(|s| matches!(s.target, Target::Ally | Target::Party) && !spent(s.key))
            .max_by_key(|s| s.unlock)
        {
            return Order::Skill(sk.key.to_string());
        }
    }

    if me.class == "hunter" {
        let banked: i32 = tok("adrenaline:").and_then(|v| v.parse().ok()).unwrap_or(0);
        // Below a full-price skill's cost there is nothing to spend, and attacking is
        // what refills it. The exact cost is a `[TUNABLE]` the client cannot read, so
        // this is a floor rather than a lookup.
        if banked < 40 {
            return Order::Attack;
        }
    }

    owned
        .into_iter()
        .filter(|s| matches!(s.target, Target::Enemy | Target::AllEnemies) && !spent(s.key))
        .max_by_key(|s| s.unlock)
        .map(|s| Order::Skill(s.key.to_string()))
        .unwrap_or(Order::Attack)
}

/// The direction of the next clear-path waypoint AHEAD of us, as a unit vector.
///
/// "Ahead" is by radius rather than by list order: the party may be dropped anywhere along the
/// trail, and steering at a waypoint already behind it walks the journey backwards.
fn next_waypoint(st: &session::State) -> Option<(f64, f64)> {
    let here = st.pos;
    let r_here = (here.0 * here.0 + here.1 * here.1).sqrt();
    let (wx, wy) = *st
        .path
        .iter()
        // Meaningfully further out, not merely ahead: the trail meanders, so a waypoint
        // 2 units out can be almost entirely SIDEWAYS, and steering at it makes no radial
        // progress at all.
        .filter(|(x, y)| (x * x + y * y).sqrt() > r_here + 10.0)
        .min_by(|a, b| {
            let d = |p: &(f64, f64)| (p.0 - here.0).powi(2) + (p.1 - here.1).powi(2);
            d(a).total_cmp(&d(b))
        })?;
    let (dx, dy) = (wx - here.0, wy - here.1);
    let len = (dx * dx + dy * dy).sqrt();
    (len > 1e-6).then(|| (dx / len, dy / len))
}

/// March outward, fighting what gets in the way, and report the ARC.
///
/// This is the tool the end-fight work needed and did not have. `walk` stops the moment a
/// fight starts and `auto_battle` plays one encounter — so measuring the JOURNEY meant a human
/// alternating them by hand, and the fights in between resolved themselves on the 15-second
/// auto-act while the harness idled. A level-40 party aiming at d500 got **d27 in two
/// minutes** that way.
///
/// What it reports is the shape of the middle game: how deep you got, how many fights that
/// cost, what you spent to win them, and where you died if you did. Those are the numbers
/// "can a party actually reach the end-world" is made of, and nothing in the repo could
/// produce them before.
async fn travel(s: &Session, args: &Value) -> Result<String, String> {
    let target = args["to_distance"].as_i64().unwrap_or(500);
    let budget = Duration::from_millis(args["max_ms"].as_u64().unwrap_or(300_000).min(3_600_000));
    let use_kit = args["policy"].as_str().unwrap_or("kit") == "kit";
    let deadline = tokio::time::Instant::now() + budget;

    let (start_d, start_level) = {
        let st = s.state.lock().await;
        (st.distance(), st.base_level)
    };
    let (mut fights, mut wins, mut losses, mut fled) = (0u32, 0u32, 0u32, 0u32);
    let mut hp_spent: i64 = 0;
    let mut deepest = start_d;
    // Following a meandering trail moves you sideways for stretches, so "no progress" has to
    // mean no progress over a WHILE. Two slices of equal floored radius is just a bend.
    let mut last_progress_d = start_d;
    let mut quiet_slices = 0u32;
    let mut potions_start: i64 = -1;

    loop {
        let now = tokio::time::Instant::now();
        if now >= deadline {
            break;
        }
        {
            let st = s.state.lock().await;
            if potions_start < 0 {
                potions_start = pouch_total(&st);
            }
            deepest = deepest.max(st.distance());
            if !st.run_active || st.distance() >= target {
                break;
            }
        }
        // Follow the world's guaranteed clear path rather than marching at the horizon.
        // Obstacles are kept out of its tube by construction, so it is the only heading that
        // cannot wedge — due east stalls against the first cliff it meets.
        let heading = {
            let st = s.state.lock().await;
            next_waypoint(&st).unwrap_or((1.0, 0.0))
        };
        s.steer(Steer::Dir(heading.0, heading.1));
        // Walk until something interrupts, then deal with it. A short slice keeps the loop
        // responsive to a fight starting without spinning.
        let interrupted = s
            .wait_until(Duration::from_millis(4000), |st| {
                st.in_battle() || !st.run_active || st.distance() >= target
            })
            .await;
        if !interrupted {
            let here = s.state.lock().await.distance();
            if here >= last_progress_d + 2 {
                last_progress_d = here;
                quiet_slices = 0;
            } else {
                quiet_slices += 1;
                // ~40s of walking without gaining ground is wedged, not winding.
                if quiet_slices >= 10 {
                    break;
                }
            }
            continue;
        }
        quiet_slices = 0;

        let in_fight = s.state.lock().await.in_battle();
        if in_fight {
            s.steer(Steer::Stop);
            let before = s.state.lock().await.battle.as_ref().map(|b| b.opening_hp).unwrap_or(0);
            fights += 1;
            play_out_battle(s, use_kit, deadline).await;
            let st = s.state.lock().await;
            if let Some(b) = st.last_battle.as_ref() {
                hp_spent += (before - b.party_hp()).max(0) as i64;
                match b.ended.as_deref() {
                    Some("victory") => wins += 1,
                    Some("fled") => fled += 1,
                    Some(_) => losses += 1,
                    None => {}
                }
            }
        }
    }
    s.steer(Steer::Stop);

    let mut st = s.state.lock().await;
    let log = st.drain_log();
    let potions_left = pouch_total(&st);
    let levels: Vec<String> = st
        .heroes
        .iter()
        .map(|h| {
            format!(
                "{} {}",
                h["class_key"].as_str().unwrap_or("?"),
                h["level"].as_i64().unwrap_or(0)
            )
        })
        .collect();
    let reached = st.distance();
    let ended = st.run_result.clone();
    Ok(format!(
        "TRAVEL d{start_d} -> d{reached} (deepest d{deepest} of a target d{target})\n         fights {fights}: {wins} won, {losses} lost, {fled} fled\n         hp spent {hp_spent}   potions used {}\n         levels: started {start_level}, now {}\n         {}{}",
        (potions_start - potions_left).max(0),
        levels.join(", "),
        match &ended {
            Some(r) => format!("RUN OVER: {r}\n"),
            None if reached >= target => "arrived.\n".to_string(),
            None => "still going (budget or a wall).\n".to_string(),
        },
        tail(&log)
    ))
}

/// Everything in every hero's pouch, so "what did the journey cost" can include the bottles.
fn pouch_total(st: &session::State) -> i64 {
    st.pouches.iter().flat_map(|p| p.iter()).map(|(_, n)| *n).sum()
}

async fn interact(s: &Session, args: &Value) -> Result<String, String> {
    let verb = args["verb"].as_str().unwrap_or("");
    let ent = args["entity_id"].as_str().map(String::from);
    let need_ent = || -> Result<String, String> {
        ent.clone()
            .ok_or_else(|| format!("{verb} needs an entity_id from look"))
    };
    let msg = match verb {
        "harvest" => json!({"type":"run.harvest","payload":{"entity_id": need_ent()?}}),
        "chest" => json!({"type":"run.open_chest","payload":{"entity_id": need_ent()?}}),
        "descend" => json!({"type":"run.enter_dungeon","payload":{"entity_id": need_ent()?}}),
        "hold" => json!({"type":"run.psyker_hold","payload":{"entity_id": need_ent()?}}),
        "join" => json!({"type":"run.join_battle","payload":{}}),
        "cancel" => json!({"type":"run.cancel_harvest","payload":{}}),
        "extract" => json!({"type":"run.begin_extraction",
            "payload":{"method":"portal","portal_entity_id": ent, "item_id": Value::Null}}),
        "town_portal" => json!({"type":"run.begin_extraction",
            "payload":{"method":"town_portal","portal_entity_id": Value::Null, "item_id": Value::Null}}),
        "use_item" => json!({"type":"run.use_item","payload":{
            "item_kind": args["item"].as_str().ok_or("use_item needs an item")?,
            "hero_slot": args["hero"].as_i64().unwrap_or(0)}}),
        other => return Err(format!("unknown verb {other}")),
    };
    s.send(msg);
    // Long enough for a channel to pay out at least once, so "did it take" is answered by
    // the log rather than by the absence of an error the server never sends.
    s.wait_until(Duration::from_millis(2500), |_| false).await;
    let mut st = s.state.lock().await;
    let log = st.drain_log();
    let body = match &st.battle {
        Some(b) => view::battle(b),
        None => view::look(&st),
    };
    Ok(format!("{body}{}", tail(&log)))
}
