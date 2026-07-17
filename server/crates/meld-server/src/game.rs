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
use meld_db::Db;
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
pub fn spawn(balance: Arc<Balance>, db: Db) -> GameHandle {
    let (tx, rx) = mpsc::channel(1024);
    tokio::spawn(async move {
        GameState::new(balance, db).run(rx).await;
    });
    GameHandle { tx }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}

/// The per-hero class composition of a player's party of `size`. The picked class
/// leads; the rest are a fixed spread so a single party mixes classes that play
/// very differently (Squire bruiser + Psyker channeler + Resonant healer).
fn party_composition(chosen: CharacterClass, size: usize) -> Vec<CharacterClass> {
    let base = [
        chosen,
        CharacterClass::Psyker,
        CharacterClass::Resonant,
        CharacterClass::Squire,
    ];
    (0..size.max(1)).map(|i| base[i % base.len()]).collect()
}

/// A class's starting/max HP from balance (falls back to squire).
fn class_base_hp(class: CharacterClass, balance: &Balance) -> i32 {
    balance
        .player
        .get(meld_run::class_key(class))
        .or_else(|| balance.player.get("squire"))
        .map(|p| p.base_hp)
        .unwrap_or(40)
}

/// A server-generated world seed. Folds a fresh v7 UUID's 16 bytes into a u64 so
/// each MazeInstance gets a distinct, unpredictable layout (CANON: seeds are
/// server-side; the client never supplies one).
fn world_seed() -> u64 {
    let bytes = Uuid::now_v7().into_bytes();
    let mut seed = 0u64;
    for chunk in bytes.chunks(8) {
        let mut buf = [0u8; 8];
        buf[..chunk.len()].copy_from_slice(chunk);
        seed ^= u64::from_le_bytes(buf);
    }
    seed
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
    /// Attack bonus from equipped gear, loaded from the DB after connect.
    gear_atk_bonus: i32,
    /// Class chosen at the player's most recent `run.enter_maze` (default Squire).
    /// This is the party *lead* (slot 0).
    character_class: CharacterClass,
    /// Explicit per-hero party composition from the builder, if the client sent
    /// one; otherwise `None` and the server builds a default mixed party.
    party_comp: Option<Vec<CharacterClass>>,
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

/// An in-progress extraction channel (interruptible; completes → bank).
struct Extraction {
    completes_at: u64,
}

/// The single active MazeInstance of the slice.
struct ActiveInstance {
    arena: Arena,
    run: InstanceRun,
    battle: Option<Battle>,
    battle_id: String,
    monster_combatant_id: String,
    /// Index (into `arena.monsters`) of the creature the active battle is
    /// against, so victory can mark that specific creature defeated.
    battle_monster_idx: Option<usize>,
    /// combatant_id -> player_id (players only).
    combatant_player: HashMap<String, String>,
    /// player_id -> the combatant ids they control (a solo player fields a party
    /// of four; in co-op each player controls one).
    player_combatants: HashMap<String, Vec<String>>,
    /// player_id -> per-hero current HP (length = party_size_per_player), carried
    /// across the run's battles so wounds persist (no free heal between fights).
    /// Reset to full only when a player (re)enters the maze — a fresh dive.
    hero_hp: HashMap<String, Vec<i32>>,
    /// player_id -> per-hero class (the mixed party composition), parallel to
    /// `hero_hp`. Each slot's class drives its stats/kit for the whole run.
    party_classes: HashMap<String, Vec<CharacterClass>>,
    /// player_id -> active extraction channel.
    extraction: HashMap<String, Extraction>,
    /// Party ids currently merged into the active battle (raid merge).
    battle_parties: std::collections::HashSet<u32>,
}

struct GameState {
    balance: Arc<Balance>,
    db: Db,
    sessions: HashMap<String, Session>,
    /// Connection order, for deterministic party formation.
    order: Vec<String>,
    instance: Option<ActiveInstance>,
    /// Players whose gear bonus needs (re)loading from the DB (post-connect).
    pending_gear_load: Vec<String>,
    /// Players whose run just ended in death; durability sink applied async.
    pending_deaths: Vec<String>,
}

impl GameState {
    fn new(balance: Arc<Balance>, db: Db) -> Self {
        GameState {
            balance,
            db,
            sessions: HashMap::new(),
            order: Vec::new(),
            instance: None,
            pending_gear_load: Vec::new(),
            pending_deaths: Vec::new(),
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
            // Async DB side-effects run after either arm (the sync paths queue
            // work; here we await Postgres): gear loads, extraction banking, and
            // the death durability sink.
            self.flush_gear_loads().await;
            let banked = self.complete_extractions().await;
            self.dispatch(banked);
            self.flush_deaths().await;
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
                        gear_atk_bonus: 0,
                        character_class: CharacterClass::Squire,
                        party_comp: None,
                    },
                );
                self.order.push(player_id.clone());
                self.pending_gear_load.push(player_id);
                Vec::new()
            }
            ServerEvent::Disconnected { player_id } => {
                self.sessions.remove(&player_id);
                self.order.retain(|p| p != &player_id);
                self.pending_gear_load.retain(|p| p != &player_id);
                self.remove_from_instance(&player_id);
                Vec::new()
            }
            ServerEvent::Client { player_id, raw } => self.handle_client(&player_id, raw),
        }
    }

    /// Drop a player's overworld/run state from the shared instance (on
    /// disconnect). When nobody is left, tear the instance down entirely so the
    /// next `enter_maze` rebuilds a clean arena with a live monster — otherwise
    /// dead avatars pile up and the slain monster never returns.
    fn remove_from_instance(&mut self, player_id: &str) {
        let Some(inst) = self.instance.as_mut() else {
            return;
        };
        inst.arena.avatars.retain(|a| a.player_id != player_id);
        inst.run.runs.retain(|r| r.player_id != player_id);
        if let Some(cids) = inst.player_combatants.remove(player_id) {
            for cid in cids {
                inst.combatant_player.remove(&cid);
            }
        }
        inst.hero_hp.remove(player_id);
        inst.party_classes.remove(player_id);
        inst.extraction.remove(player_id);
        if inst.run.runs.is_empty() {
            self.instance = None;
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
            wr::EnterMaze::TYPE => self.handle_enter_maze(player_id, raw),
            wr::BeginExtraction::TYPE => self.handle_begin_extraction(player_id, raw),
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

    fn handle_enter_maze(&mut self, player_id: &str, raw: RawEnvelope) -> Vec<Outgoing> {
        let client_seq = raw.seq;
        // Record the caller's party choice before forming the party. The party
        // builder sends an explicit `party`; otherwise `character_class` is the
        // lead and the server builds a default mixed party around it.
        let req = serde_json::from_value::<wr::EnterMaze>(raw.payload).ok();
        let party_comp = req.as_ref().and_then(|e| e.party.clone()).filter(|p| !p.is_empty());
        let chosen = req
            .as_ref()
            .and_then(|e| e.character_class)
            .or_else(|| party_comp.as_ref().and_then(|p| p.first().copied()))
            .unwrap_or(CharacterClass::Squire);
        if let Some(s) = self.sessions.get_mut(player_id) {
            s.character_class = chosen;
            s.party_comp = party_comp;
        }
        // The caller can't already be in a run.
        if self
            .sessions
            .get(player_id)
            .map(|s| s.in_instance)
            .unwrap_or(false)
        {
            return vec![error(
                player_id,
                ErrorCode::InvalidState,
                "A run is already active for you.",
                Some(client_seq),
            )];
        }
        // Party = connected players not already in a run, up to the cap. So the
        // first enter_maze with everyone waiting forms one big party; a player
        // who enters later forms a fresh party that can raid-merge in.
        let party_ids: Vec<String> = self
            .order
            .iter()
            .filter(|p| {
                self.sessions
                    .get(*p)
                    .map(|s| !s.in_instance)
                    .unwrap_or(false)
            })
            .take(meld_proto::limits::PARTY_MAX)
            .cloned()
            .collect();
        if party_ids.is_empty() {
            return vec![error(
                player_id,
                ErrorCode::InvalidState,
                "No eligible players.",
                Some(client_seq),
            )];
        }

        let departure_hub_distance = 0; // Center Hub
        let speed = self.balance.world.avatar_speed_tiles_per_sec;

        // Create the shared instance on the first entry.
        if self.instance.is_none() {
            let instance_id = Uuid::now_v7().to_string();
            // Server-generated world seed (CANON: the client never supplies or
            // computes seeds). Derived from a fresh v7 UUID's bytes.
            let seed = world_seed();
            self.instance = Some(ActiveInstance {
                arena: Arena::generate(&self.balance, seed),
                run: InstanceRun::new(instance_id, departure_hub_distance, &self.balance),
                battle: None,
                battle_id: String::new(),
                monster_combatant_id: String::new(),
                battle_monster_idx: None,
                combatant_player: HashMap::new(),
                player_combatants: HashMap::new(),
                hero_hp: HashMap::new(),
                party_classes: HashMap::new(),
                extraction: HashMap::new(),
                battle_parties: std::collections::HashSet::new(),
            });
        }
        let inst = self.instance.as_mut().expect("instance exists");
        let instance_id = inst.run.instance_id.clone();
        let base_run_level = inst.run.base_run_level;

        let members: Vec<(String, String, CharacterClass, String)> = party_ids
            .iter()
            .map(|pid| {
                let (username, class) = self
                    .sessions
                    .get(pid)
                    .map(|s| (s.username.clone(), s.character_class))
                    .unwrap_or((String::new(), CharacterClass::Squire));
                (pid.clone(), username, class, Uuid::now_v7().to_string())
            })
            .collect();
        inst.run.add_party(members);
        for pid in &party_ids {
            inst.arena.add_avatar(pid.clone(), speed);
        }
        // (Re)enter = a fresh dive: build each player's mixed party composition and
        // start every hero at its class's full HP. Within the run this HP persists
        // across battles (see hero_hp write-back).
        let party_size = self.balance.battle.party_size_per_player.max(1);
        for pid in &party_ids {
            let (chosen, explicit) = self
                .sessions
                .get(pid)
                .map(|s| (s.character_class, s.party_comp.clone()))
                .unwrap_or((CharacterClass::Squire, None));
            // The builder's explicit composition wins (normalized to party size,
            // padded with Squire); otherwise build a default mixed party around
            // the lead.
            let comp = match explicit {
                Some(mut p) => {
                    p.truncate(party_size);
                    while p.len() < party_size {
                        p.push(CharacterClass::Squire);
                    }
                    p
                }
                None => party_composition(chosen, party_size),
            };
            let hp: Vec<i32> = comp.iter().map(|c| class_base_hp(*c, &self.balance)).collect();
            inst.party_classes.insert(pid.clone(), comp);
            inst.hero_hp.insert(pid.clone(), hp);
        }
        for pid in &party_ids {
            if let Some(s) = self.sessions.get_mut(pid) {
                s.in_instance = true;
            }
        }
        let inst = self.instance.as_ref().expect("instance exists");

        // run.started to this party's members (spawn positions from the arena).
        let member_views: Vec<wr::Member> = party_ids
            .iter()
            .filter_map(|pid| inst.run.runs.iter().find(|r| &r.player_id == pid))
            .map(|r| wr::Member {
                player_id: r.player_id.clone(),
                username: r.username.clone(),
                character_class: r.character_class,
                spawn_position: inst
                    .arena
                    .avatar(&r.player_id)
                    .map(|a| a.position)
                    .unwrap_or(Position::new(0.0, 0.0)),
            })
            .collect();

        let mut out = Vec::new();
        for pid in &party_ids {
            let run_id = inst
                .run
                .runs
                .iter()
                .find(|r| &r.player_id == pid)
                .map(|r| r.run_id.clone())
                .unwrap_or_default();
            out.push(out_msg(
                pid,
                &wr::Started {
                    client_seq: if pid == player_id { Some(client_seq) } else { None },
                    run_id,
                    instance_id: instance_id.clone(),
                    departure_hub_distance,
                    base_run_level,
                    members: member_views.clone(),
                    backpack: Vec::new(),
                },
            ));
        }
        self.pending_gear_load.extend(party_ids.iter().cloned());
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
        // Any movement interrupts an in-progress extraction channel (D15).
        if inst.extraction.remove(player_id).is_some() {
            if let Some(a) = inst.arena.avatar_mut(player_id) {
                a.state = "active".to_string();
            }
            let members: Vec<String> = inst.run.runs.iter().map(|r| r.player_id.clone()).collect();
            return members
                .iter()
                .map(|pid| {
                    out_msg(
                        pid,
                        &wr::ChannelInterrupted {
                            player_id: player_id.to_string(),
                            reason: "moved".to_string(),
                        },
                    )
                })
                .collect();
        }
        // Movement is ignored while in battle (avatar not `active`).
        inst.arena.apply_move(
            player_id,
            intent.move_dir.x,
            intent.move_dir.y,
            intent.input_seq,
        );

        // Touching the monster starts a battle, or raid-merges the toucher's
        // party into the one already in progress (combat-atb.md).
        let decision = {
            match inst.arena.check_touch() {
                Some((toucher, monster_idx)) => {
                    let party = inst
                        .run
                        .runs
                        .iter()
                        .find(|r| r.player_id == toucher)
                        .map(|r| r.party_id);
                    match party {
                        Some(pid) if inst.battle.is_none() => {
                            Some((toucher, pid, Some(monster_idx)))
                        }
                        Some(pid) if !inst.battle_parties.contains(&pid) => {
                            Some((toucher, pid, None))
                        }
                        _ => None,
                    }
                }
                None => None,
            }
        };
        match decision {
            Some((toucher, pid, Some(monster_idx))) => {
                self.start_battle(&toucher, pid, monster_idx)
            }
            Some((toucher, pid, None)) => self.join_battle(&toucher, pid),
            None => Vec::new(),
        }
    }

    fn start_battle(&mut self, toucher: &str, party_id: u32, monster_idx: usize) -> Vec<Outgoing> {
        let seed = now_ms();
        let balance = self.balance.clone();
        // Snapshot gear bonuses before borrowing the instance mutably.
        let bonuses: HashMap<String, i32> = self
            .sessions
            .iter()
            .map(|(k, s)| (k.clone(), s.gear_atk_bonus))
            .collect();
        let Some(inst) = self.instance.as_mut() else {
            return Vec::new();
        };

        let battle_id = Uuid::now_v7().to_string();
        let monster_combatant_id = Uuid::now_v7().to_string();

        // Assign combatant ids for the *touching* party only.
        let mut party: Vec<meld_run::PartyMember> = Vec::new();
        inst.combatant_player.clear();
        inst.player_combatants.clear();
        let party_players: Vec<String> = inst
            .run
            .runs
            .iter()
            .filter(|r| r.party_id == party_id)
            .map(|r| r.player_id.clone())
            .collect();
        // Every player fields a mixed party of up to `party_size_per_player`
        // heroes (GDD: per-player party), each slot its own class from the party
        // composition. Up to PARTY_MAX players share the instance, so a full co-op
        // battle is (players × party size) combatants. Per-hero starting HP is
        // aligned with `party` (carried across the run so wounds persist).
        let mut hp_overrides: Vec<Option<i32>> = Vec::new();
        for r in inst.run.runs.iter().filter(|r| r.party_id == party_id) {
            let bonus = bonuses.get(&r.player_id).copied().unwrap_or(0);
            let hp_vec = inst.hero_hp.get(&r.player_id).cloned().unwrap_or_default();
            let comp = inst
                .party_classes
                .get(&r.player_id)
                .cloned()
                .unwrap_or_else(|| party_composition(r.character_class, hp_vec.len().max(1)));
            let mut cids = Vec::new();
            for (slot, cls) in comp.iter().enumerate() {
                let cid = Uuid::now_v7().to_string();
                inst.combatant_player
                    .insert(cid.clone(), r.player_id.clone());
                party.push((r.player_id.clone(), cid.clone(), *cls, bonus));
                hp_overrides.push(hp_vec.get(slot).copied());
                cids.push(cid);
            }
            inst.player_combatants.insert(r.player_id.clone(), cids);
        }

        let monster = inst.arena.monsters[monster_idx].clone();
        let battle = build_battle(
            battle_id.clone(),
            &party,
            &monster,
            monster_combatant_id.clone(),
            &inst.run,
            &balance,
            seed,
            &hp_overrides,
        );
        let (allies, enemies) = battle.wire_combatants();

        for pid in &party_players {
            if let Some(a) = inst.arena.avatar_mut(pid) {
                a.state = "in_battle".to_string();
            }
        }

        let encounter_class = battle.encounter_class;
        tracing::info!(
            battle_id = %battle_id,
            party = party_players.len(),
            triggered_by = %toucher,
            "battle started"
        );
        inst.battle = Some(battle);
        inst.battle_id = battle_id.clone();
        inst.monster_combatant_id = monster_combatant_id;
        inst.battle_monster_idx = Some(monster_idx);
        inst.battle_parties.clear();
        inst.battle_parties.insert(party_id);

        let mut out = Vec::new();
        for pid in &party_players {
            let yours = inst.player_combatants.get(pid).cloned().unwrap_or_default();
            out.push(out_msg(
                pid,
                &wb::Started {
                    battle_id: battle_id.clone(),
                    encounter_class,
                    allies: allies.clone(),
                    enemies: enemies.clone(),
                    your_combatant_id: yours.first().cloned().unwrap_or_default(),
                    your_combatant_ids: yours,
                    triggered_by: Some(toucher.to_string()),
                },
            ));
        }
        out
    }

    /// Raid merge: the toucher's party joins the in-progress battle.
    fn join_battle(&mut self, toucher: &str, party_id: u32) -> Vec<Outgoing> {
        let balance = self.balance.clone();
        let cap =
            meld_proto::limits::PARTY_MAX * self.balance.battle.merge_cap_normal_instances.max(1) as usize;
        let bonuses: HashMap<String, i32> = self
            .sessions
            .iter()
            .map(|(k, s)| (k.clone(), s.gear_atk_bonus))
            .collect();
        let Some(inst) = self.instance.as_mut() else {
            return Vec::new();
        };
        if inst.battle.is_none() {
            return Vec::new();
        }

        // Build the joining party's combatants.
        let mut party: Vec<meld_run::PartyMember> = Vec::new();
        let mut joiners: Vec<String> = Vec::new();
        for r in inst.run.runs.iter().filter(|r| r.party_id == party_id) {
            let cid = Uuid::now_v7().to_string();
            let bonus = bonuses.get(&r.player_id).copied().unwrap_or(0);
            party.push((r.player_id.clone(), cid.clone(), r.character_class, bonus));
            joiners.push(r.player_id.clone());
            inst.combatant_player.insert(cid.clone(), r.player_id.clone());
            inst.player_combatants
                .insert(r.player_id.clone(), vec![cid]);
        }
        if party.is_empty() {
            return Vec::new();
        }
        // Merge cap: a touch that would exceed it does not merge (combat-atb.md).
        if inst.battle.as_ref().unwrap().player_count() + party.len() > cap {
            for cid_pid in &party {
                inst.combatant_player.remove(&cid_pid.1);
                inst.player_combatants.remove(&cid_pid.0);
            }
            return Vec::new();
        }

        let mut fighters = meld_run::party_fighters(&party, &inst.run, &balance);
        // Carry each joiner's persisted HP (slot 0) into the merged battle.
        for (f, pm) in fighters.iter_mut().zip(party.iter()) {
            if let Some(h) = inst.hero_hp.get(&pm.0).and_then(|v| v.first().copied()) {
                f.hp = h.clamp(0, f.max_hp);
            }
        }
        inst.battle.as_mut().unwrap().join(fighters);
        inst.battle_parties.insert(party_id);
        for pid in &joiners {
            if let Some(a) = inst.arena.avatar_mut(pid) {
                a.state = "in_battle".to_string();
            }
        }

        let battle = inst.battle.as_ref().unwrap();
        let battle_id = inst.battle_id.clone();
        let encounter_class = battle.encounter_class;
        let (allies, enemies) = battle.wire_combatants();
        // Joining combatants (for party_joined to the existing side).
        let joining_allies: Vec<meld_proto::common::Combatant> = allies
            .iter()
            .filter(|c| {
                c.player_id
                    .as_ref()
                    .map(|p| joiners.contains(p))
                    .unwrap_or(false)
            })
            .cloned()
            .collect();

        let mut out = Vec::new();
        // battle.started (full state) to the joiners.
        for pid in &joiners {
            let yours = inst.player_combatants.get(pid).cloned().unwrap_or_default();
            out.push(out_msg(
                pid,
                &wb::Started {
                    battle_id: battle_id.clone(),
                    encounter_class,
                    allies: allies.clone(),
                    enemies: enemies.clone(),
                    your_combatant_id: yours.first().cloned().unwrap_or_default(),
                    your_combatant_ids: yours,
                    triggered_by: Some(toucher.to_string()),
                },
            ));
        }
        // battle.party_joined (delta) to everyone already in the battle.
        let existing: Vec<String> = inst
            .run
            .runs
            .iter()
            .filter(|r| inst.battle_parties.contains(&r.party_id) && !joiners.contains(&r.player_id))
            .map(|r| r.player_id.clone())
            .collect();
        for pid in &existing {
            out.push(out_msg(
                pid,
                &wb::PartyJoined {
                    battle_id: battle_id.clone(),
                    joining_instance_id: inst.run.instance_id.clone(),
                    joining_allies: joining_allies.clone(),
                },
            ));
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
        let owned = inst.player_combatants.get(player_id).cloned().unwrap_or_default();
        // The actor must be one of the sender's own combatants; default to their
        // first hero when the client doesn't name one (back-compat).
        let actor_cid = match &submit.actor_combatant_id {
            Some(cid) if owned.contains(cid) => cid.clone(),
            Some(_) => {
                return vec![error(
                    player_id,
                    ErrorCode::ValidationError,
                    "That combatant is not yours.",
                    Some(raw.seq),
                )]
            }
            None => match owned.first() {
                Some(cid) => cid.clone(),
                None => {
                    return vec![error(
                        player_id,
                        ErrorCode::NotFound,
                        "Not a combatant.",
                        Some(raw.seq),
                    )]
                }
            },
        };

        let battle = inst.battle.as_mut().unwrap();
        let result = battle.submit(
            &actor_cid,
            submit.action_id.clone(),
            submit.action,
            submit.target_ids.clone(),
            submit.skill_kind.clone(),
            submit.item_id.clone(),
        );
        match result {
            Ok(events) => self.emit_battle_events(events),
            Err(reject) => {
                let (code, message) = reject_to_error(&reject);
                vec![error(player_id, code, message, Some(raw.seq))]
            }
        }
    }

    fn handle_begin_extraction(&mut self, player_id: &str, raw: RawEnvelope) -> Vec<Outgoing> {
        let req: wr::BeginExtraction = match serde_json::from_value(raw.payload) {
            Ok(v) => v,
            Err(_) => {
                return vec![error(
                    player_id,
                    ErrorCode::ValidationError,
                    "bad begin_extraction",
                    Some(raw.seq),
                )]
            }
        };
        let now = now_ms();
        let channel_ms = self.balance.runs.extraction_channel_ms;
        let Some(inst) = self.instance.as_mut() else {
            return vec![error(
                player_id,
                ErrorCode::InvalidState,
                "Not in a run.",
                Some(raw.seq),
            )];
        };
        if inst.battle.is_some() {
            return vec![error(
                player_id,
                ErrorCode::InvalidState,
                "Resolve the battle first.",
                Some(raw.seq),
            )];
        }
        if inst.extraction.contains_key(player_id) {
            return vec![error(
                player_id,
                ErrorCode::InvalidState,
                "Already channeling.",
                Some(raw.seq),
            )];
        }
        // Portal extraction requires standing at the portal; escape items work
        // from anywhere (their consumption is deferred with the item slice).
        if req.method == "portal" && !inst.arena.at_portal(player_id) {
            return vec![error(
                player_id,
                ErrorCode::OutOfRange,
                "Not at an extraction portal.",
                Some(raw.seq),
            )];
        }
        let completes_at = now + channel_ms;
        inst.extraction
            .insert(player_id.to_string(), Extraction { completes_at });
        if let Some(a) = inst.arena.avatar_mut(player_id) {
            a.state = "channeling".to_string();
        }
        let members: Vec<String> = inst.run.runs.iter().map(|r| r.player_id.clone()).collect();
        members
            .iter()
            .map(|pid| {
                out_msg(
                    pid,
                    &wr::ChannelStarted {
                        client_seq: if pid == player_id { Some(raw.seq) } else { None },
                        player_id: player_id.to_string(),
                        method: req.method.clone(),
                        completes_at,
                    },
                )
            })
            .collect()
    }

    /// Load equipped-gear attack bonuses for freshly-connected players.
    async fn flush_gear_loads(&mut self) {
        let loads: Vec<String> = std::mem::take(&mut self.pending_gear_load);
        for pid in loads {
            if let Ok(uid) = Uuid::parse_str(&pid) {
                if let Ok(bonus) = self.db.equipped_atk_bonus(uid).await {
                    if let Some(s) = self.sessions.get_mut(&pid) {
                        s.gear_atk_bonus = bonus;
                    }
                }
            }
        }
    }

    /// Apply the death durability sink (Postgres) for players who just died.
    async fn flush_deaths(&mut self) {
        let deaths: Vec<String> = std::mem::take(&mut self.pending_deaths);
        for pid in deaths {
            if let Ok(uid) = Uuid::parse_str(&pid) {
                if let Err(e) = self.db.apply_death_durability(uid).await {
                    tracing::error!("death durability failed for {pid}: {e}");
                }
            }
        }
    }

    /// Complete any extraction channels whose timer elapsed: bank the backpack
    /// into the Vault (Postgres) and finalize the run as `extracted`.
    async fn complete_extractions(&mut self) -> Vec<Outgoing> {
        let now = now_ms();
        struct Banked {
            player_id: String,
            run_id: String,
            items: Vec<ItemStack>,
        }
        let (banks, members): (Vec<Banked>, Vec<String>) = {
            let Some(inst) = self.instance.as_mut() else {
                return Vec::new();
            };
            let done: Vec<String> = inst
                .extraction
                .iter()
                .filter(|(_, e)| e.completes_at <= now)
                .map(|(p, _)| p.clone())
                .collect();
            if done.is_empty() {
                return Vec::new();
            }
            let mut banks = Vec::new();
            for pid in &done {
                inst.extraction.remove(pid);
                if let Some(a) = inst.arena.avatar_mut(pid) {
                    a.state = "active".to_string();
                }
                if let Some(r) = inst.run.runs.iter_mut().find(|r| &r.player_id == pid) {
                    if r.result.is_some() {
                        continue;
                    }
                    let items = std::mem::take(&mut r.backpack);
                    r.result = Some(RunResult::Extracted);
                    banks.push(Banked {
                        player_id: pid.clone(),
                        run_id: r.run_id.clone(),
                        items,
                    });
                }
            }
            let members: Vec<String> =
                inst.run.runs.iter().map(|r| r.player_id.clone()).collect();
            (banks, members)
        };

        let db = self.db.clone();
        let alchemy_per = self.balance.meld.alchemy_xp_per_extracted_stack;
        let mut out = Vec::new();
        for b in banks {
            let items_kv: Vec<(String, i32)> = b
                .items
                .iter()
                .map(|i| (i.item_kind.clone(), i.quantity))
                .collect();
            if let Ok(uid) = Uuid::parse_str(&b.player_id) {
                if let Err(e) = db.bank_extraction(uid, &items_kv, 0).await {
                    tracing::error!("bank_extraction failed for {}: {e}", b.player_id);
                }
                // Extraction success credits Alchemy XP (GDD §4.1).
                let axp = items_kv.len() as i64 * alchemy_per;
                if axp > 0 {
                    if let Err(e) = db.add_skill_xp(uid, "alchemy", axp).await {
                        tracing::error!("alchemy xp failed for {}: {e}", b.player_id);
                    }
                }
            }
            for pid in &members {
                let banked = if pid == &b.player_id {
                    Some(b.items.clone())
                } else {
                    None
                };
                out.push(out_msg(
                    pid,
                    &wr::MemberResult {
                        run_id: b.run_id.clone(),
                        player_id: b.player_id.clone(),
                        result: RunResult::Extracted,
                        max_distance_reached: 0,
                        banked,
                        lost: None,
                        durability_loss_applied: false,
                    },
                ));
            }
            if !b.items.is_empty() {
                out.push(out_msg(
                    &b.player_id,
                    &wr::BackpackUpdate {
                        changes: b
                            .items
                            .iter()
                            .map(|i| wr::BackpackChange {
                                item: i.clone(),
                                delta: "removed".to_string(),
                                cause: "banked".to_string(),
                            })
                            .collect(),
                    },
                ));
            }
        }
        out
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
            .filter(|r| inst.battle_parties.contains(&r.party_id))
            .map(|r| out_msg(&r.player_id, &msg))
            .collect()
    }

    fn snapshot_msgs(&self) -> Vec<Outgoing> {
        let Some(inst) = self.instance.as_ref() else {
            return Vec::new();
        };
        let mut entities: Vec<wm::SnapshotEntity> = inst
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
        // Every living creature is a dynamic entity too (movement-world.md:
        // snapshots carry players and monsters). We tag a monster's `avatar_state`
        // with its creature kind (`mob:<kind>`) so the client can colour/label it;
        // that's distinct from the player states and the `portal` tag below. Slain
        // creatures are dropped from the snapshot.
        for m in inst.arena.monsters.iter().filter(|m| !m.defeated) {
            entities.push(wm::SnapshotEntity {
                entity_id: m.entity_id.clone(),
                position: m.position,
                velocity: wm::Velocity { x: 0.0, y: 0.0 },
                avatar_state: Some(format!("mob:{}", m.monster_kind)),
            });
        }
        // One extraction portal per area, each tagged with a distinct avatar_state
        // the client renders specially (a pragmatic stand-in for world.entity_spawn).
        for (i, portal) in inst.arena.portals().enumerate() {
            entities.push(wm::SnapshotEntity {
                entity_id: format!("portal-{i}"),
                position: portal,
                velocity: wm::Velocity { x: 0.0, y: 0.0 },
                avatar_state: Some("portal".to_string()),
            });
        }
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

    /// Battle id + the players currently in it (all merged parties).
    fn battle_id_and_members(&self) -> (String, Vec<String>) {
        match &self.instance {
            Some(inst) => (
                inst.battle_id.clone(),
                inst.run
                    .runs
                    .iter()
                    .filter(|r| inst.battle_parties.contains(&r.party_id))
                    .map(|r| r.player_id.clone())
                    .collect(),
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
        let monster_idx = inst.battle_monster_idx;
        let xp_reward = monster_idx
            .and_then(|i| inst.arena.monsters.get(i))
            .map(|m| m.xp_reward)
            .unwrap_or(0);
        tracing::info!(battle_id = %battle_id, ?outcome, "battle ended");
        // The outcome applies to every party merged into the battle (raid).
        let bp = inst.battle_parties.clone();
        let members: Vec<String> = inst
            .run
            .runs
            .iter()
            .filter(|r| bp.contains(&r.party_id))
            .map(|r| r.player_id.clone())
            .collect();

        // Persist each participant's per-hero HP so wounds carry to the next
        // encounter (no free heal between fights). Read from the battle before it
        // is torn down below.
        if let Some(b) = &inst.battle {
            for pid in &members {
                if let (Some(cids), Some(hps)) =
                    (inst.player_combatants.get(pid), inst.hero_hp.get_mut(pid))
                {
                    for (slot, cid) in cids.iter().enumerate() {
                        if let (Some(hp), Some(slot_hp)) = (b.combatant_hp(cid), hps.get_mut(slot)) {
                            *slot_hp = hp;
                        }
                    }
                }
            }
        }

        let mut dead: Vec<String> = Vec::new();

        match outcome {
            BattleOutcome::Victory => {
                if let Some(i) = monster_idx {
                    if let Some(m) = inst.arena.monsters.get_mut(i) {
                        m.defeated = true;
                    }
                }
                // Award XP to every participant; return their avatars to active.
                for r in inst.run.runs.iter_mut().filter(|r| bp.contains(&r.party_id)) {
                    r.award_xp(xp_reward);
                }
                for pid in &members {
                    if let Some(a) = inst.arena.avatar_mut(pid) {
                        a.state = "active".to_string();
                    }
                }
                // Build per-member ended (own loot) + backpack update.
                let runs_snapshot: Vec<(String, i32, i64)> = inst
                    .run
                    .runs
                    .iter()
                    .filter(|r| bp.contains(&r.party_id))
                    .map(|r| (r.player_id.clone(), r.run_level, r.xp))
                    .collect();
                for (pid, run_level, _xp) in &runs_snapshot {
                    let loot_item = ItemStack {
                        item_id: Uuid::now_v7().to_string(),
                        item_kind: "forest_bloom_petal".to_string(),
                        quantity: 1,
                        insurance: None,
                    };
                    // Record loot in the run's backpack so extraction can bank it.
                    if let Some(r) = inst.run.runs.iter_mut().find(|r| &r.player_id == pid) {
                        r.backpack.push(loot_item.clone());
                    }
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
                // Each participating player's run → died. (Durability sink runs
                // in flush_deaths against Postgres.)
                let run_ids: Vec<(String, String)> = inst
                    .run
                    .runs
                    .iter()
                    .filter(|r| bp.contains(&r.party_id))
                    .map(|r| (r.player_id.clone(), r.run_id.clone()))
                    .collect();
                for r in inst.run.runs.iter_mut().filter(|r| bp.contains(&r.party_id)) {
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
                dead = members.clone();
            }
            BattleOutcome::Fled => {
                inst.battle = None;
            }
        }
        // Battle over: reset merge + combatant bookkeeping.
        inst.battle_parties.clear();
        inst.battle_monster_idx = None;
        inst.combatant_player.clear();
        inst.player_combatants.clear();
        self.pending_deaths.append(&mut dead);
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
