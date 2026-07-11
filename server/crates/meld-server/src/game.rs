//! The authoritative game loop — the Rust descendant of the Go `GameHub`.
//!
//! One task owns all ephemeral state (sessions + the single MazeInstance of the
//! slice) and is fed [`ServerEvent`]s over an mpsc channel; it advances the ATB
//! battle on the 100 ms tick and fans authoritative `*.*` messages back to each
//! session's outbound channel. Because exactly one task touches the state, there
//! are no locks (CANON.md §S: server-authoritative throughout).
//!
//! Slice simplifications (documented, promoted in later slices): a single shared
//! MazeInstance; the party is formed from the connected players at the first
//! `run.enter_maze`; chunk streaming and Gatekeepers are deferred.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use meld_balance::Balance;
use meld_battle::{Battle, Event as BattleEvent, Reject};
use meld_proto::common::{ItemStack, Position};
use meld_proto::enums::*;
use meld_proto::realtime::{battle as wb, movement as wm, run as wr, session as ws, Message};
use meld_proto::RawEnvelope;
use meld_run::{build_battle, InstanceRun};
use meld_world::Arena;
use tokio::sync::mpsc;
use uuid::Uuid;

/// Events fed into the game loop from connection tasks.
pub enum ServerEvent {
    /// A socket completed the `session.authenticate` handshake.
    Connected {
        player_id: String,
        username: String,
        session_id: String,
        out: mpsc::UnboundedSender<String>,
    },
    /// A socket closed.
    Disconnected { player_id: String },
    /// A parsed C2S envelope arrived.
    Client { player_id: String, raw: RawEnvelope },
}

/// Handle used by the gateway to feed the loop.
#[derive(Clone)]
pub struct GameHandle {
    tx: mpsc::Sender<ServerEvent>,
}

impl GameHandle {
    pub async fn send(&self, ev: ServerEvent) {
        let _ = self.tx.send(ev).await;
    }
}

/// Spawn the game loop; returns a handle for the gateway.
pub fn spawn(balance: Arc<Balance>) -> GameHandle {
    let (tx, rx) = mpsc::channel(1024);
    tokio::spawn(async move {
        GameState::new(balance).run(rx).await;
    });
    GameHandle { tx }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}

struct Session {
    username: String,
    out: mpsc::UnboundedSender<String>,
    /// Logical session id — surfaced in `resume` blocks (resume slice, deferred).
    #[allow(dead_code)]
    session_id: String,
    seq_out: u32,
    last_client_seq: u32,
    in_instance: bool,
}

/// One outbound message queued for a player, before seq assignment.
struct Outgoing {
    player_id: String,
    msg_type: String,
    payload: serde_json::Value,
}

fn out_msg<M: Message>(player_id: &str, m: &M) -> Outgoing {
    Outgoing {
        player_id: player_id.to_string(),
        msg_type: M::TYPE.to_string(),
        payload: serde_json::to_value(m).expect("payload serializes"),
    }
}

/// The single active MazeInstance of the slice.
struct ActiveInstance {
    arena: Arena,
    run: InstanceRun,
    battle: Option<Battle>,
    battle_id: String,
    monster_combatant_id: String,
    /// combatant_id -> player_id (players only).
    combatant_player: HashMap<String, String>,
    /// player_id -> combatant_id.
    player_combatant: HashMap<String, String>,
}

struct GameState {
    balance: Arc<Balance>,
    sessions: HashMap<String, Session>,
    /// Connection order, for deterministic party formation.
    order: Vec<String>,
    instance: Option<ActiveInstance>,
}

impl GameState {
    fn new(balance: Arc<Balance>) -> Self {
        GameState {
            balance,
            sessions: HashMap::new(),
            order: Vec::new(),
            instance: None,
        }
    }

    async fn run(mut self, mut rx: mpsc::Receiver<ServerEvent>) {
        let tick_ms = self.balance.battle.tick_ms.max(10);
        let mut ticker = tokio::time::interval(Duration::from_millis(tick_ms));
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                maybe = rx.recv() => match maybe {
                    Some(ev) => {
                        let out = self.handle_event(ev);
                        self.dispatch(out);
                    }
                    None => break, // all senders dropped
                },
                _ = ticker.tick() => {
                    let out = self.tick();
                    self.dispatch(out);
                }
            }
        }
    }

    fn dispatch(&mut self, out: Vec<Outgoing>) {
        for o in out {
            if let Some(s) = self.sessions.get_mut(&o.player_id) {
                let env = serde_json::json!({
                    "type": o.msg_type,
                    "seq": s.seq_out,
                    "ts": now_ms(),
                    "payload": o.payload,
                });
                s.seq_out = s.seq_out.wrapping_add(1);
                let _ = s.out.send(env.to_string());
            }
        }
    }

    // --- event handling -----------------------------------------------------

    fn handle_event(&mut self, ev: ServerEvent) -> Vec<Outgoing> {
        match ev {
            ServerEvent::Connected {
                player_id,
                username,
                session_id,
                out,
            } => {
                // The gateway already sent `session.authenticated` (seq 1), so
                // the server-side counter continues at 2.
                self.sessions.insert(
                    player_id.clone(),
                    Session {
                        username,
                        out,
                        session_id,
                        seq_out: 2,
                        last_client_seq: 0,
                        in_instance: false,
                    },
                );
                self.order.push(player_id);
                Vec::new()
            }
            ServerEvent::Disconnected { player_id } => {
                self.sessions.remove(&player_id);
                self.order.retain(|p| p != &player_id);
                Vec::new()
            }
            ServerEvent::Client { player_id, raw } => self.handle_client(&player_id, raw),
        }
    }

    fn handle_client(&mut self, player_id: &str, raw: RawEnvelope) -> Vec<Outgoing> {
        // Per-session monotonic seq check (realtime-protocol.md §Sequencing).
        {
            let Some(s) = self.sessions.get_mut(player_id) else {
                return Vec::new();
            };
            if raw.seq <= s.last_client_seq {
                return vec![error(
                    player_id,
                    ErrorCode::SequenceError,
                    "seq must strictly increase",
                    Some(raw.seq),
                )];
            }
            s.last_client_seq = raw.seq;
        }

        match raw.msg_type.as_str() {
            ws::Heartbeat::TYPE => vec![out_msg(
                player_id,
                &ws::HeartbeatAck {
                    client_seq: raw.seq,
                    server_ts: now_ms(),
                },
            )],
            wr::EnterMaze::TYPE => self.handle_enter_maze(player_id, raw.seq),
            wm::MoveIntent::TYPE => self.handle_move(player_id, raw),
            wb::SubmitAction::TYPE => self.handle_submit(player_id, raw),
            other => vec![error(
                player_id,
                ErrorCode::ValidationError,
                format!("unknown message type: {other}"),
                Some(raw.seq),
            )],
        }
    }

    fn handle_enter_maze(&mut self, player_id: &str, client_seq: u32) -> Vec<Outgoing> {
        if self.instance.is_some() {
            return vec![error(
                player_id,
                ErrorCode::InvalidState,
                "A run is already active.",
                Some(client_seq),
            )];
        }
        // Form the party from up to PARTY_MAX connected players (slice model).
        let party_ids: Vec<String> = self
            .order
            .iter()
            .take(meld_proto::limits::PARTY_MAX)
            .cloned()
            .collect();
        if party_ids.is_empty() {
            return vec![error(
                player_id,
                ErrorCode::InvalidState,
                "No connected players.",
                Some(client_seq),
            )];
        }

        let departure_hub_distance = 0; // Center Hub
        let members: Vec<(String, String, CharacterClass, String)> = party_ids
            .iter()
            .map(|pid| {
                let username = self
                    .sessions
                    .get(pid)
                    .map(|s| s.username.clone())
                    .unwrap_or_default();
                (
                    pid.clone(),
                    username,
                    CharacterClass::Squire,
                    Uuid::now_v7().to_string(),
                )
            })
            .collect();

        let instance_id = Uuid::now_v7().to_string();
        let run = InstanceRun::new(
            instance_id.clone(),
            departure_hub_distance,
            members,
            &self.balance,
        );

        let speed = self.balance.world.avatar_speed_tiles_per_sec;
        let party_speeds: Vec<(String, f64)> =
            party_ids.iter().map(|p| (p.clone(), speed)).collect();
        let arena = Arena::new(&self.balance, &party_speeds, Uuid::now_v7().to_string());

        for pid in &party_ids {
            if let Some(s) = self.sessions.get_mut(pid) {
                s.in_instance = true;
            }
        }

        // Build the shared members list once (spawn positions from the arena).
        let member_views: Vec<wr::Member> = run
            .runs
            .iter()
            .map(|r| {
                let pos = arena
                    .avatar(&r.player_id)
                    .map(|a| a.position)
                    .unwrap_or(Position::new(0.0, 0.0));
                wr::Member {
                    player_id: r.player_id.clone(),
                    username: r.username.clone(),
                    character_class: r.character_class,
                    spawn_position: pos,
                }
            })
            .collect();

        let mut out = Vec::new();
        for pid in &party_ids {
            let msg = wr::Started {
                client_seq: if pid == player_id {
                    Some(client_seq)
                } else {
                    None
                },
                run_id: run
                    .runs
                    .iter()
                    .find(|r| &r.player_id == pid)
                    .map(|r| r.run_id.clone())
                    .unwrap_or_default(),
                instance_id: instance_id.clone(),
                departure_hub_distance,
                base_run_level: run.base_run_level,
                members: member_views.clone(),
                backpack: Vec::new(),
            };
            out.push(out_msg(pid, &msg));
        }

        self.instance = Some(ActiveInstance {
            arena,
            run,
            battle: None,
            battle_id: String::new(),
            monster_combatant_id: String::new(),
            combatant_player: HashMap::new(),
            player_combatant: HashMap::new(),
        });
        out
    }

    fn handle_move(&mut self, player_id: &str, raw: RawEnvelope) -> Vec<Outgoing> {
        let intent: wm::MoveIntent = match serde_json::from_value(raw.payload) {
            Ok(v) => v,
            Err(_) => {
                return vec![error(
                    player_id,
                    ErrorCode::ValidationError,
                    "bad move_intent",
                    Some(raw.seq),
                )]
            }
        };
        let Some(inst) = self.instance.as_mut() else {
            return vec![error(
                player_id,
                ErrorCode::InvalidState,
                "Not in a run.",
                Some(raw.seq),
            )];
        };
        // Movement is ignored while in battle (avatar not `active`).
        inst.arena.apply_move(
            player_id,
            intent.move_dir.x,
            intent.move_dir.y,
            intent.input_seq,
        );

        // Touch detection triggers the battle (behaviors/combat-atb.md).
        if inst.battle.is_none() {
            if let Some(toucher) = inst.arena.check_touch() {
                return self.start_battle(&toucher);
            }
        }
        Vec::new()
    }

    fn start_battle(&mut self, toucher: &str) -> Vec<Outgoing> {
        let seed = now_ms();
        let balance = self.balance.clone();
        let Some(inst) = self.instance.as_mut() else {
            return Vec::new();
        };
        inst.arena.monster.engaged = true;

        let battle_id = Uuid::now_v7().to_string();
        let monster_combatant_id = Uuid::now_v7().to_string();

        // Assign a combatant id per party member.
        let mut party: Vec<(String, String, CharacterClass)> = Vec::new();
        inst.combatant_player.clear();
        inst.player_combatant.clear();
        for r in &inst.run.runs {
            let cid = Uuid::now_v7().to_string();
            inst.combatant_player
                .insert(cid.clone(), r.player_id.clone());
            inst.player_combatant
                .insert(r.player_id.clone(), cid.clone());
            party.push((r.player_id.clone(), cid, r.character_class));
        }

        let battle = build_battle(
            battle_id.clone(),
            &party,
            &inst.arena.monster,
            monster_combatant_id.clone(),
            &inst.run,
            &balance,
            seed,
        );
        let (allies, enemies) = battle.wire_combatants();

        // Mark avatars in-battle.
        for r in &inst.run.runs {
            if let Some(a) = inst.arena.avatar_mut(&r.player_id) {
                a.state = "in_battle".to_string();
            }
        }

        let encounter_class = battle.encounter_class;
        inst.battle = Some(battle);
        inst.battle_id = battle_id.clone();
        inst.monster_combatant_id = monster_combatant_id;

        // Broadcast battle.started to every party member.
        let member_ids: Vec<String> = inst.run.runs.iter().map(|r| r.player_id.clone()).collect();
        let mut out = Vec::new();
        for pid in &member_ids {
            let your = inst.player_combatant.get(pid).cloned().unwrap_or_default();
            let msg = wb::Started {
                battle_id: battle_id.clone(),
                encounter_class,
                allies: allies.clone(),
                enemies: enemies.clone(),
                your_combatant_id: your,
                triggered_by: Some(toucher.to_string()),
            };
            out.push(out_msg(pid, &msg));
        }
        out
    }

    fn handle_submit(&mut self, player_id: &str, raw: RawEnvelope) -> Vec<Outgoing> {
        let submit: wb::SubmitAction = match serde_json::from_value(raw.payload) {
            Ok(v) => v,
            Err(_) => {
                return vec![error(
                    player_id,
                    ErrorCode::ValidationError,
                    "bad submit_action",
                    Some(raw.seq),
                )]
            }
        };
        let Some(inst) = self.instance.as_mut() else {
            return vec![error(
                player_id,
                ErrorCode::NotFound,
                "No battle.",
                Some(raw.seq),
            )];
        };
        if inst.battle.is_none() || submit.battle_id != inst.battle_id {
            return vec![error(
                player_id,
                ErrorCode::NotFound,
                "Unknown battle.",
                Some(raw.seq),
            )];
        }
        let Some(actor_cid) = inst.player_combatant.get(player_id).cloned() else {
            return vec![error(
                player_id,
                ErrorCode::NotFound,
                "Not a combatant.",
                Some(raw.seq),
            )];
        };

        let battle = inst.battle.as_mut().unwrap();
        let result = battle.submit(
            &actor_cid,
            submit.action_id.clone(),
            submit.action,
            submit.target_ids.clone(),
        );
        match result {
            Ok(events) => self.emit_battle_events(events),
            Err(reject) => {
                let (code, message) = reject_to_error(&reject);
                vec![error(player_id, code, message, Some(raw.seq))]
            }
        }
    }

    // --- tick ---------------------------------------------------------------

    fn tick(&mut self) -> Vec<Outgoing> {
        let has_battle = self
            .instance
            .as_ref()
            .map(|i| i.battle.is_some())
            .unwrap_or(false);
        if has_battle {
            let events = self
                .instance
                .as_mut()
                .unwrap()
                .battle
                .as_mut()
                .unwrap()
                .tick();
            let mut out = self.emit_battle_events(events);
            // Gauge keepalive (event-driven + periodic per battle.md).
            if let Some(inst) = self.instance.as_ref() {
                if let Some(b) = inst.battle.as_ref() {
                    out.extend(self.gauge_update_msgs(inst, b));
                }
            }
            out
        } else if self.instance.is_some() {
            self.snapshot_msgs()
        } else {
            Vec::new()
        }
    }

    fn gauge_update_msgs(&self, inst: &ActiveInstance, b: &Battle) -> Vec<Outgoing> {
        let combatants: Vec<wb::GaugeEntry> = b
            .gauge_state()
            .into_iter()
            .map(|(id, gauge, hp, statuses)| wb::GaugeEntry {
                combatant_id: id,
                gauge,
                hp,
                statuses,
            })
            .collect();
        let msg = wb::GaugeUpdate {
            battle_id: inst.battle_id.clone(),
            server_tick: b.tick_count() as i64,
            combatants,
        };
        inst.run
            .runs
            .iter()
            .map(|r| out_msg(&r.player_id, &msg))
            .collect()
    }

    fn snapshot_msgs(&self) -> Vec<Outgoing> {
        let Some(inst) = self.instance.as_ref() else {
            return Vec::new();
        };
        let entities: Vec<wm::SnapshotEntity> = inst
            .arena
            .avatars
            .iter()
            .map(|a| wm::SnapshotEntity {
                entity_id: a.player_id.clone(),
                position: a.position,
                velocity: wm::Velocity { x: 0.0, y: 0.0 },
                avatar_state: Some(a.state.clone()),
            })
            .collect();
        let msg = wm::Snapshot {
            server_tick: now_ms() as i64,
            entities,
        };
        inst.run
            .runs
            .iter()
            .map(|r| out_msg(&r.player_id, &msg))
            .collect()
    }

    /// Translate engine events into wire messages, handling terminal outcomes.
    fn emit_battle_events(&mut self, events: Vec<BattleEvent>) -> Vec<Outgoing> {
        let mut out = Vec::new();
        for ev in events {
            match ev {
                BattleEvent::TurnReady { combatant_id } => {
                    let is_player = self
                        .instance
                        .as_ref()
                        .map(|i| i.combatant_player.contains_key(&combatant_id))
                        .unwrap_or(false);
                    let timeout_at = if is_player {
                        Some(now_ms() + self.balance.battle.turn_timeout_ms)
                    } else {
                        None
                    };
                    let (battle_id, members) = self.battle_id_and_members();
                    for pid in &members {
                        out.push(out_msg(
                            pid,
                            &wb::TurnReady {
                                battle_id: battle_id.clone(),
                                combatant_id: combatant_id.clone(),
                                timeout_at,
                            },
                        ));
                    }
                }
                BattleEvent::Resolved(res) => {
                    let (battle_id, members) = self.battle_id_and_members();
                    let msg = wb::ActionResolved {
                        battle_id,
                        action_id: res.action_id.clone(),
                        actor_id: res.actor_id.clone(),
                        action: res.action,
                        auto: res.auto,
                        flee_success: res.flee_success,
                        effects: res
                            .effects
                            .iter()
                            .map(|e| wb::Effect {
                                target_id: e.target_id.clone(),
                                kind: e.kind,
                                amount: e.amount,
                                status: e.status.clone(),
                                hp_after: e.hp_after,
                            })
                            .collect(),
                    };
                    for pid in &members {
                        out.push(out_msg(pid, &msg));
                    }
                }
                BattleEvent::Ended { outcome } => {
                    out.extend(self.handle_battle_end(outcome));
                }
            }
        }
        out
    }

    fn battle_id_and_members(&self) -> (String, Vec<String>) {
        match &self.instance {
            Some(inst) => (
                inst.battle_id.clone(),
                inst.run.runs.iter().map(|r| r.player_id.clone()).collect(),
            ),
            None => (String::new(), Vec::new()),
        }
    }

    fn handle_battle_end(&mut self, outcome: BattleOutcome) -> Vec<Outgoing> {
        let mut out = Vec::new();
        let Some(inst) = self.instance.as_mut() else {
            return out;
        };
        let battle_id = inst.battle_id.clone();
        let xp_reward = inst.arena.monster.xp_reward;
        let members: Vec<String> = inst.run.runs.iter().map(|r| r.player_id.clone()).collect();

        match outcome {
            BattleOutcome::Victory => {
                inst.arena.monster.defeated = true;
                // Award XP + one loot item per surviving player; return avatars.
                for r in &mut inst.run.runs {
                    r.award_xp(xp_reward);
                }
                let owner_ids: Vec<String> =
                    inst.run.runs.iter().map(|r| r.player_id.clone()).collect();
                for pid in &owner_ids {
                    if let Some(a) = inst.arena.avatar_mut(pid) {
                        a.state = "active".to_string();
                    }
                }
                // Build per-member ended (own loot) + backpack update.
                let runs_snapshot: Vec<(String, i32, i64)> = inst
                    .run
                    .runs
                    .iter()
                    .map(|r| (r.player_id.clone(), r.run_level, r.xp))
                    .collect();
                for (pid, run_level, _xp) in &runs_snapshot {
                    let loot_item = ItemStack {
                        item_id: Uuid::now_v7().to_string(),
                        item_kind: "forest_bloom_petal".to_string(),
                        quantity: 1,
                        insurance: None,
                    };
                    let ended = wb::Ended {
                        battle_id: battle_id.clone(),
                        outcome: BattleOutcome::Victory,
                        xp_awards: vec![wb::XpAward {
                            player_id: pid.clone(),
                            xp: xp_reward,
                            run_level_after: *run_level,
                        }],
                        loot: vec![loot_item.clone()],
                        class_emblem_drops: vec![],
                        gatekeeper_cleared: false,
                    };
                    out.push(out_msg(pid, &ended));
                    out.push(out_msg(
                        pid,
                        &wr::BackpackUpdate {
                            changes: vec![wr::BackpackChange {
                                item: loot_item,
                                delta: "added".to_string(),
                                cause: "battle_loot".to_string(),
                            }],
                        },
                    ));
                }
                inst.battle = None;
            }
            BattleOutcome::Defeat => {
                for pid in &members {
                    out.push(out_msg(
                        pid,
                        &wb::Ended {
                            battle_id: battle_id.clone(),
                            outcome: BattleOutcome::Defeat,
                            xp_awards: vec![],
                            loot: vec![],
                            class_emblem_drops: vec![],
                            gatekeeper_cleared: false,
                        },
                    ));
                }
                // Each player's run → died (durability/vault handoff is DB-side,
                // deferred with the persistence slice).
                let run_ids: Vec<(String, String)> = inst
                    .run
                    .runs
                    .iter()
                    .map(|r| (r.player_id.clone(), r.run_id.clone()))
                    .collect();
                for r in &mut inst.run.runs {
                    r.result = Some(RunResult::Died);
                }
                for (pid, run_id) in &run_ids {
                    out.push(out_msg(
                        pid,
                        &wr::MemberResult {
                            run_id: run_id.clone(),
                            player_id: pid.clone(),
                            result: RunResult::Died,
                            max_distance_reached: 0,
                            banked: None,
                            lost: Some(vec![]),
                            durability_loss_applied: true,
                        },
                    ));
                }
                inst.battle = None;
            }
            BattleOutcome::Fled => {
                inst.battle = None;
            }
        }
        out
    }
}

fn error(
    player_id: &str,
    code: ErrorCode,
    message: impl Into<String>,
    client_seq: Option<u32>,
) -> Outgoing {
    out_msg(
        player_id,
        &ws::Error {
            code,
            message: message.into(),
            client_seq,
        },
    )
}

fn reject_to_error(reject: &Reject) -> (ErrorCode, &'static str) {
    match reject {
        Reject::NotFound => (ErrorCode::NotFound, "Target not found."),
        Reject::DuplicateAction => (ErrorCode::DuplicateAction, "Duplicate action_id."),
        Reject::InvalidState(m) => (ErrorCode::InvalidState, m),
        Reject::ValidationError(m) => (ErrorCode::ValidationError, m),
    }
}
