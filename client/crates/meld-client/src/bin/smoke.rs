//! Headless end-to-end smoke: drives the whole gameplay loop through the
//! client's own (cross-platform) network layer against a real running server.
//! Exits 0 on victory, 1 on defeat, non-zero on timeout/error.
//!
//! The runnable proof of the client thread where a native Bevy window can't be
//! observed. Solo run: one client forms a party of one and fights the monster.
//!
//! Usage: `MELD_SERVER=http://127.0.0.1:8080 cargo run -p meld-client --bin smoke`

use std::time::{Duration, Instant};

use meld_client::net::{self, ClientCmd, ServerMsg};

fn main() {
    let base = std::env::var("MELD_SERVER").unwrap_or_else(|_| "http://127.0.0.1:8080".to_string());
    eprintln!("[smoke] connecting to {base}");
    let net = net::start(base);

    let name = format!("smoke{}", &uuid::Uuid::new_v4().simple().to_string()[..8]);
    net.send(ClientCmd::Connect { username: name });

    // Optional class (MELD_CLASS=psyker exercises the Psyker Focus system end to
    // end: focus slots + manifestations ticking + a win).
    let class = std::env::var("MELD_CLASS").unwrap_or_else(|_| "squire".to_string());
    let mut psyker_turn = 0u32;
    let mut entered = false;
    let mut in_battle = false;
    let mut my_combatant = String::new();
    let mut battle_id = String::new();
    let mut monster: Option<String> = None;
    let mut my_turn = false;
    let mut last_move = Instant::now();
    let deadline = Instant::now() + Duration::from_secs(40);

    loop {
        if Instant::now() > deadline {
            eprintln!("[smoke] TIMEOUT");
            std::process::exit(2);
        }

        net.poll();
        while let Some(msg) = net.try_recv() {
            match msg {
                ServerMsg::Connected { player_id } => {
                    eprintln!("[smoke] authenticated as {player_id}; entering maze");
                    net.send(ClientCmd::EnterMaze {
                        character_class: class.clone(),
                    });
                }
                ServerMsg::RunStarted => {
                    entered = true;
                    eprintln!("[smoke] run started; walking to the monster");
                }
                ServerMsg::BattleStarted {
                    battle_id: b,
                    your_combatant_id,
                    your_combatant_ids: _,
                    monster_combatant,
                    combatants,
                } => {
                    in_battle = true;
                    battle_id = b;
                    my_combatant = your_combatant_id.clone();
                    monster = monster_combatant;
                    let my_max = combatants
                        .iter()
                        .find(|c| c.id == your_combatant_id)
                        .map(|c| c.max_hp)
                        .unwrap_or(0);
                    eprintln!(
                        "[smoke] battle started ({} combatants); {class} hero max HP = {my_max}",
                        combatants.len()
                    );
                }
                ServerMsg::TurnReady { combatant_id } => {
                    if combatant_id == my_combatant {
                        my_turn = true;
                    }
                }
                ServerMsg::BattleEnded { outcome } => {
                    eprintln!("[smoke] battle ended: {outcome}");
                    match outcome.as_str() {
                        "victory" => std::process::exit(0),
                        "defeat" => std::process::exit(1),
                        _ => std::process::exit(4),
                    }
                }
                ServerMsg::Error { message } => eprintln!("[smoke] server error: {message}"),
                ServerMsg::Disconnected => {
                    eprintln!("[smoke] disconnected");
                    std::process::exit(3);
                }
                ServerMsg::Gauge { .. }
                | ServerMsg::Snapshot { .. }
                | ServerMsg::CombatantsJoined { .. }
                | ServerMsg::ActionResolved { .. }
                | ServerMsg::ChannelStarted { .. }
                | ServerMsg::ChannelInterrupted
                | ServerMsg::InventoryData { .. }
                | ServerMsg::ProgressData { .. }
                | ServerMsg::RunEnded { .. } => {}
            }
        }

        // Walk east toward the monster until pulled into battle.
        if entered && !in_battle && last_move.elapsed() > Duration::from_millis(80) {
            net.send(ClientCmd::Move { dx: 1.0, dy: 0.0 });
            last_move = Instant::now();
        }
        // Act as soon as it's our turn. A Psyker channels Foci: cast Gravity Well,
        // then Kinetic Aegis, then keep reinforcing — its ticks kill the monster.
        // Everyone else swings.
        if in_battle && my_turn {
            if let Some(t) = monster.clone() {
                if class == "psyker" {
                    let op = match psyker_turn {
                        0 => "cast:gravity_well",
                        1 => "cast:kinetic_aegis",
                        _ => "reinforce:gravity_well",
                    };
                    psyker_turn += 1;
                    net.send(ClientCmd::Skill {
                        battle_id: battle_id.clone(),
                        actor: my_combatant.clone(),
                        target: t,
                        skill_kind: op.to_string(),
                    });
                } else {
                    net.send(ClientCmd::Attack {
                        battle_id: battle_id.clone(),
                        actor: my_combatant.clone(),
                        target: t,
                    });
                }
                my_turn = false;
            }
        }

        std::thread::sleep(Duration::from_millis(10));
    }
}
