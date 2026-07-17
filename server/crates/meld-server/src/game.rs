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
use meld_proto::realtime::{
    battle as wb, lobby as wl, movement as wm, run as wr, session as ws, Message,
};
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

/// Item kind of the Town Portal consumable — the primary extraction method.
const TOWN_PORTAL: &str = "town_portal";

/// A cheap uniform `[0,1)` roll from arbitrary material (splitmix64). Used for
/// non-authoritative rolls like loot drops (game-loop side may use wall-clock;
/// only meld-battle/meld-world must stay pure).
fn roll_unit(material: u64) -> f64 {
    let mut z = material.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^= z >> 31;
    (z >> 11) as f64 / (1u64 << 53) as f64
}

/// FNV-1a hash of a string (folds an id into the roll material).
fn hash_str(s: &str) -> u64 {
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    for b in s.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
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
/// A short, human-typeable lobby join code (server-side; not the pure engine).
fn new_lobby_code() -> String {
    Uuid::now_v7().simple().to_string()[..4].to_uppercase()
}

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
    /// `"portal"` or `"town_portal"` — a town-portal channel consumes one Town
    /// Portal item on completion.
    method: String,
}

/// The single active MazeInstance of the slice.
struct ActiveInstance {
    arena: Arena,
    run: InstanceRun,
    battle: Option<Battle>,
    battle_id: String,
    monster_combatant_id: String,
    /// Indices (into `arena.monsters`) of every creature in the active encounter
    /// (the touched creature plus its nearby group), so victory can mark them all
    /// defeated and award their combined XP.
    battle_monster_idxs: Vec<usize>,
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

/// One member of a pre-maze co-op lobby.
struct LobbyMember {
    player_id: String,
    party: Vec<CharacterClass>,
    ready: bool,
}

/// A pre-maze co-op lobby: a group forming up before diving together.
struct Lobby {
    code: String,
    host: String,
    members: Vec<LobbyMember>,
}

struct GameState {
    balance: Arc<Balance>,
    db: Db,
    sessions: HashMap<String, Session>,
    /// Connection order, for deterministic party formation.
    order: Vec<String>,
    instance: Option<ActiveInstance>,
    /// Open co-op lobbies, keyed by join code.
    lobbies: HashMap<String, Lobby>,
    /// player_id -> the lobby code they're in.
    player_lobby: HashMap<String, String>,
    /// Players whose gear bonus needs (re)loading from the DB (post-connect).
    pending_gear_load: Vec<String>,
    /// Meld-skill XP earned by harvesting, flushed to Postgres: (player, skill, xp).
    pending_skill_xp: Vec<(String, String, i64)>,
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
            lobbies: HashMap::new(),
            player_lobby: HashMap::new(),
            pending_gear_load: Vec::new(),
            pending_skill_xp: Vec::new(),
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
            self.flush_skill_xp().await;
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
                // Drop the player from any lobby first (notifying the rest), then
                // from the session/instance. The leaver's own `lobby.closed` is
                // discarded since their socket is gone.
                let out = self.leave_lobby(&player_id);
                self.sessions.remove(&player_id);
                self.order.retain(|p| p != &player_id);
                self.pending_gear_load.retain(|p| p != &player_id);
                self.remove_from_instance(&player_id);
                out
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
            wl::Create::TYPE => self.handle_lobby_create(player_id, raw),
            wl::Join::TYPE => self.handle_lobby_join(player_id, raw),
            wl::Ready::TYPE => self.handle_lobby_ready(player_id, raw),
            wl::Leave::TYPE => self.handle_lobby_leave(player_id, raw.seq),
            wl::Start::TYPE => self.handle_lobby_start(player_id, raw.seq),
            wr::BeginExtraction::TYPE => self.handle_begin_extraction(player_id, raw),
            wr::Harvest::TYPE => self.handle_harvest(player_id, raw),
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
        // Record the caller's party choice. The party builder sends an explicit
        // `party`; otherwise `character_class` is the lead and the server builds a
        // default mixed party around it.
        let req = serde_json::from_value::<wr::EnterMaze>(raw.payload).ok();
        let solo = req.as_ref().map(|e| e.solo).unwrap_or(false);
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
        // Co-op is the lobby flow — you can't solo/quick-enter while in a lobby.
        if self.player_lobby.contains_key(player_id) {
            return vec![error(
                player_id,
                ErrorCode::InvalidState,
                "You're in a lobby — start the dive from there.",
                Some(client_seq),
            )];
        }
        // Solo = a private instance for just the caller. Otherwise (legacy path,
        // used by the headless bot tests) group all waiting players up to the cap.
        let party_ids: Vec<String> = if solo {
            vec![player_id.to_string()]
        } else {
            self.order
                .iter()
                .filter(|p| {
                    self.sessions
                        .get(*p)
                        .map(|s| !s.in_instance && !self.player_lobby.contains_key(*p))
                        .unwrap_or(false)
                })
                .take(meld_proto::limits::PARTY_MAX)
                .cloned()
                .collect()
        };
        if party_ids.is_empty() {
            return vec![error(
                player_id,
                ErrorCode::InvalidState,
                "No eligible players.",
                Some(client_seq),
            )];
        }
        self.form_run(party_ids, player_id, Some(client_seq))
    }

    /// Enroll `party_ids` into a shared MazeInstance and emit `run.started` to
    /// each. The initiator's `run.started` echoes `client_seq`. Every enrolled
    /// player's session must already carry its `character_class` / `party_comp`.
    fn form_run(
        &mut self,
        party_ids: Vec<String>,
        initiator: &str,
        client_seq: Option<u32>,
    ) -> Vec<Outgoing> {
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
                battle_monster_idxs: Vec::new(),
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
        // Each dive starts with a stock of Town Portal items — the primary way
        // home now that there's a single, deep fixed portal.
        let starting_tp = self.balance.runs.starting_town_portals;
        if starting_tp > 0 {
            for pid in &party_ids {
                if let Some(r) = inst.run.run_mut(pid) {
                    r.backpack.push(ItemStack {
                        item_id: Uuid::now_v7().to_string(),
                        item_kind: TOWN_PORTAL.to_string(),
                        quantity: starting_tp,
                        insurance: None,
                    });
                }
            }
        }
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
                    client_seq: if pid == initiator { client_seq } else { None },
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

    // --- co-op lobby --------------------------------------------------------

    /// Broadcast a lobby's authoritative state to all its members.
    fn broadcast_lobby(&self, code: &str) -> Vec<Outgoing> {
        let Some(lobby) = self.lobbies.get(code) else {
            return Vec::new();
        };
        let members: Vec<wl::MemberView> = lobby
            .members
            .iter()
            .map(|m| wl::MemberView {
                player_id: m.player_id.clone(),
                username: self
                    .sessions
                    .get(&m.player_id)
                    .map(|s| s.username.clone())
                    .unwrap_or_default(),
                party: m.party.clone(),
                ready: m.ready,
            })
            .collect();
        let msg = wl::State {
            code: lobby.code.clone(),
            host_player_id: lobby.host.clone(),
            members,
        };
        lobby
            .members
            .iter()
            .map(|m| out_msg(&m.player_id, &msg))
            .collect()
    }

    /// A member's party choice, normalized to party size (or the default mix).
    fn lobby_party(&self, party: Option<Vec<CharacterClass>>) -> Vec<CharacterClass> {
        let size = self.balance.battle.party_size_per_player.max(1);
        match party {
            Some(mut p) if !p.is_empty() => {
                p.truncate(size);
                while p.len() < size {
                    p.push(CharacterClass::Squire);
                }
                p
            }
            _ => party_composition(CharacterClass::Squire, size),
        }
    }

    fn handle_lobby_create(&mut self, player_id: &str, raw: RawEnvelope) -> Vec<Outgoing> {
        if self.player_lobby.contains_key(player_id)
            || self.sessions.get(player_id).map(|s| s.in_instance).unwrap_or(false)
        {
            return vec![error(
                player_id,
                ErrorCode::InvalidState,
                "Already in a lobby or a run.",
                Some(raw.seq),
            )];
        }
        let party = serde_json::from_value::<wl::Create>(raw.payload)
            .ok()
            .and_then(|c| c.party);
        let party = self.lobby_party(party);
        // A short, unique join code.
        let mut code = new_lobby_code();
        while self.lobbies.contains_key(&code) {
            code = new_lobby_code();
        }
        self.lobbies.insert(
            code.clone(),
            Lobby {
                code: code.clone(),
                host: player_id.to_string(),
                members: vec![LobbyMember {
                    player_id: player_id.to_string(),
                    party,
                    ready: false,
                }],
            },
        );
        self.player_lobby.insert(player_id.to_string(), code.clone());
        self.broadcast_lobby(&code)
    }

    fn handle_lobby_join(&mut self, player_id: &str, raw: RawEnvelope) -> Vec<Outgoing> {
        if self.player_lobby.contains_key(player_id)
            || self.sessions.get(player_id).map(|s| s.in_instance).unwrap_or(false)
        {
            return vec![error(
                player_id,
                ErrorCode::InvalidState,
                "Already in a lobby or a run.",
                Some(raw.seq),
            )];
        }
        let seq = raw.seq;
        let req: wl::Join = match serde_json::from_value(raw.payload) {
            Ok(v) => v,
            Err(_) => {
                return vec![error(player_id, ErrorCode::ValidationError, "bad join", Some(seq))]
            }
        };
        let code = req.code.trim().to_uppercase();
        let party = self.lobby_party(req.party);
        let Some(lobby) = self.lobbies.get_mut(&code) else {
            return vec![error(player_id, ErrorCode::NotFound, "No such lobby.", Some(seq))];
        };
        if lobby.members.len() >= meld_proto::limits::PARTY_MAX {
            return vec![error(player_id, ErrorCode::InvalidState, "Lobby is full.", Some(seq))];
        }
        lobby.members.push(LobbyMember {
            player_id: player_id.to_string(),
            party,
            ready: false,
        });
        self.player_lobby.insert(player_id.to_string(), code.clone());
        self.broadcast_lobby(&code)
    }

    fn handle_lobby_ready(&mut self, player_id: &str, raw: RawEnvelope) -> Vec<Outgoing> {
        let Some(code) = self.player_lobby.get(player_id).cloned() else {
            return vec![error(player_id, ErrorCode::InvalidState, "Not in a lobby.", Some(raw.seq))];
        };
        let ready = serde_json::from_value::<wl::Ready>(raw.payload)
            .map(|r| r.ready)
            .unwrap_or(true);
        if let Some(lobby) = self.lobbies.get_mut(&code) {
            if let Some(m) = lobby.members.iter_mut().find(|m| m.player_id == player_id) {
                m.ready = ready;
            }
        }
        self.broadcast_lobby(&code)
    }

    fn handle_lobby_leave(&mut self, player_id: &str, _seq: u32) -> Vec<Outgoing> {
        self.leave_lobby(player_id)
    }

    /// Remove a player from whatever lobby they're in; dissolve it if empty,
    /// promote a new host if the host left, and broadcast the result.
    fn leave_lobby(&mut self, player_id: &str) -> Vec<Outgoing> {
        let Some(code) = self.player_lobby.remove(player_id) else {
            return Vec::new();
        };
        let mut out = Vec::new();
        if let Some(lobby) = self.lobbies.get_mut(&code) {
            lobby.members.retain(|m| m.player_id != player_id);
            if lobby.members.is_empty() {
                self.lobbies.remove(&code);
            } else {
                if lobby.host == player_id {
                    lobby.host = lobby.members[0].player_id.clone();
                }
                out = self.broadcast_lobby(&code);
            }
        }
        // Tell the leaver their lobby view is gone.
        out.push(out_msg(player_id, &wl::Closed {}));
        out
    }

    fn handle_lobby_start(&mut self, player_id: &str, seq: u32) -> Vec<Outgoing> {
        let Some(code) = self.player_lobby.get(player_id).cloned() else {
            return vec![error(player_id, ErrorCode::InvalidState, "Not in a lobby.", Some(seq))];
        };
        let Some(lobby) = self.lobbies.get(&code) else {
            return vec![error(player_id, ErrorCode::NotFound, "No such lobby.", Some(seq))];
        };
        if lobby.host != player_id {
            return vec![error(
                player_id,
                ErrorCode::InvalidState,
                "Only the host can start.",
                Some(seq),
            )];
        }
        if !lobby.members.iter().all(|m| m.ready) {
            return vec![error(
                player_id,
                ErrorCode::InvalidState,
                "Not everyone is ready.",
                Some(seq),
            )];
        }
        // Push each member's chosen party onto their session, then dissolve the
        // lobby and form one shared run.
        let members: Vec<(String, Vec<CharacterClass>)> = lobby
            .members
            .iter()
            .map(|m| (m.player_id.clone(), m.party.clone()))
            .collect();
        for (pid, party) in &members {
            if let Some(s) = self.sessions.get_mut(pid) {
                s.character_class = party.first().copied().unwrap_or(CharacterClass::Squire);
                s.party_comp = Some(party.clone());
            }
            self.player_lobby.remove(pid);
        }
        self.lobbies.remove(&code);
        let ids: Vec<String> = members.into_iter().map(|(pid, _)| pid).collect();
        self.form_run(ids, player_id, Some(seq))
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

        // The encounter is the touched creature plus every creature grouped
        // around it — they all pile in (their factions sort out who fights whom).
        let group_idxs = inst.arena.group_around(monster_idx);
        // Give each grouped creature a combatant id; the touched one leads (its id
        // is the client's default target).
        let mut enemy_members: Vec<(meld_world::MonsterSpawn, String)> = Vec::new();
        for &gi in &group_idxs {
            let cid = if gi == monster_idx {
                monster_combatant_id.clone()
            } else {
                Uuid::now_v7().to_string()
            };
            enemy_members.push((inst.arena.monsters[gi].clone(), cid));
        }
        // Put the touched creature first so `monster_combatant_id` = enemies[0].
        enemy_members.sort_by_key(|(_, cid)| *cid != monster_combatant_id);
        let enemies_ref: Vec<_> = enemy_members
            .iter()
            .map(|(m, cid)| (m, cid.clone()))
            .collect();
        let battle = build_battle(
            battle_id.clone(),
            &party,
            &enemies_ref,
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
        // Lock the grouped creatures out of roaming while the fight is on.
        for &gi in &group_idxs {
            if let Some(m) = inst.arena.monsters.get_mut(gi) {
                m.in_battle = true;
            }
        }

        let encounter_class = battle.encounter_class;
        tracing::info!(
            battle_id = %battle_id,
            party = party_players.len(),
            enemies = group_idxs.len(),
            triggered_by = %toucher,
            "battle started"
        );
        inst.battle = Some(battle);
        inst.battle_id = battle_id.clone();
        inst.monster_combatant_id = monster_combatant_id;
        inst.battle_monster_idxs = group_idxs;
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
        // "portal" requires standing at the single deep portal; "town_portal"
        // works anywhere but requires a Town Portal item (consumed on completion).
        match req.method.as_str() {
            "portal" => {
                if !inst.arena.at_portal(player_id) {
                    return vec![error(
                        player_id,
                        ErrorCode::OutOfRange,
                        "Not at the extraction portal.",
                        Some(raw.seq),
                    )];
                }
            }
            "town_portal" => {
                let has = inst
                    .run
                    .run_mut(player_id)
                    .is_some_and(|r| r.backpack.iter().any(|i| i.item_kind == TOWN_PORTAL));
                if !has {
                    return vec![error(
                        player_id,
                        ErrorCode::InvalidState,
                        "No Town Portal item.",
                        Some(raw.seq),
                    )];
                }
            }
            _ => {
                return vec![error(
                    player_id,
                    ErrorCode::ValidationError,
                    "unknown extraction method",
                    Some(raw.seq),
                )]
            }
        }
        let completes_at = now + channel_ms;
        inst.extraction.insert(
            player_id.to_string(),
            Extraction {
                completes_at,
                method: req.method.clone(),
            },
        );
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

    /// Credit Meld-skill XP earned by harvesting to Postgres (Forging/Alchemy).
    async fn flush_skill_xp(&mut self) {
        let jobs: Vec<(String, String, i64)> = std::mem::take(&mut self.pending_skill_xp);
        for (pid, skill, xp) in jobs {
            if xp <= 0 {
                continue;
            }
            if let Ok(uid) = Uuid::parse_str(&pid) {
                if let Err(e) = self.db.add_skill_xp(uid, &skill, xp).await {
                    tracing::error!("harvest skill xp failed for {pid}: {e}");
                }
            }
        }
    }

    /// Harvest the named resource node the avatar is standing next to: bank its
    /// material into the backpack and queue its Meld-skill XP. The node vanishes
    /// from the next snapshot (server-authoritative — client just renders).
    fn handle_harvest(&mut self, player_id: &str, raw: RawEnvelope) -> Vec<Outgoing> {
        let req: wr::Harvest = match serde_json::from_value(raw.payload) {
            Ok(v) => v,
            Err(_) => {
                return vec![error(
                    player_id,
                    ErrorCode::ValidationError,
                    "bad harvest",
                    Some(raw.seq),
                )]
            }
        };
        let balance = self.balance.clone();
        let (item, skill, xp, kind) = {
            let Some(inst) = self.instance.as_mut() else {
                return vec![error(player_id, ErrorCode::InvalidState, "Not in a run.", Some(raw.seq))];
            };
            if inst.battle.is_some() {
                return vec![error(
                    player_id,
                    ErrorCode::InvalidState,
                    "Resolve the battle first.",
                    Some(raw.seq),
                )];
            }
            let Some(kind) = inst.arena.harvest(player_id, &req.entity_id) else {
                return vec![error(
                    player_id,
                    ErrorCode::OutOfRange,
                    "Nothing to harvest here.",
                    Some(raw.seq),
                )];
            };
            let Some(res) = balance.resource.get(&kind) else {
                return vec![error(player_id, ErrorCode::ValidationError, "unknown resource", Some(raw.seq))];
            };
            let item = ItemStack {
                item_id: Uuid::now_v7().to_string(),
                item_kind: res.material.clone(),
                quantity: 1,
                insurance: None,
            };
            if let Some(r) = inst.run.run_mut(player_id) {
                r.backpack.push(item.clone());
            }
            (item, res.skill.clone(), res.xp, kind)
        };
        self.pending_skill_xp.push((player_id.to_string(), skill, xp));
        vec![out_msg(
            player_id,
            &wr::BackpackUpdate {
                changes: vec![wr::BackpackChange {
                    item,
                    delta: "added".to_string(),
                    cause: format!("harvest:{kind}"),
                }],
            },
        )]
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
            let done: Vec<(String, String)> = inst
                .extraction
                .iter()
                .filter(|(_, e)| e.completes_at <= now)
                .map(|(p, e)| (p.clone(), e.method.clone()))
                .collect();
            if done.is_empty() {
                return Vec::new();
            }
            let mut banks = Vec::new();
            for (pid, method) in &done {
                inst.extraction.remove(pid);
                if let Some(a) = inst.arena.avatar_mut(pid) {
                    a.state = "active".to_string();
                }
                if let Some(r) = inst.run.runs.iter_mut().find(|r| &r.player_id == pid) {
                    if r.result.is_some() {
                        continue;
                    }
                    // A town-portal extraction spends one Town Portal item; it is
                    // consumed, not banked.
                    if method == "town_portal" {
                        if let Some(slot) =
                            r.backpack.iter_mut().find(|i| i.item_kind == TOWN_PORTAL)
                        {
                            slot.quantity -= 1;
                        }
                        r.backpack.retain(|i| i.quantity > 0);
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
            // No battle: roam the creatures, then snapshot the overworld.
            let dt = (self.balance.battle.tick_ms.max(1) as f64) / 1000.0;
            if let Some(inst) = self.instance.as_mut() {
                inst.arena.step_creatures(dt);
            }
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
        // as `mob:<kind>:<faction>` so the client can colour/label it by faction;
        // that's distinct from the player states and the `portal` tag below. Slain
        // creatures are dropped from the snapshot.
        for m in inst.arena.monsters.iter().filter(|m| !m.defeated) {
            entities.push(wm::SnapshotEntity {
                entity_id: m.entity_id.clone(),
                position: m.position,
                velocity: wm::Velocity { x: 0.0, y: 0.0 },
                avatar_state: Some(format!("mob:{}:{}", m.monster_kind, m.faction)),
            });
        }
        // The single deep extraction portal (extraction is otherwise the Town
        // Portal item). Tagged `portal` so the client renders it specially.
        entities.push(wm::SnapshotEntity {
            entity_id: "portal".to_string(),
            position: inst.arena.portal,
            velocity: wm::Velocity { x: 0.0, y: 0.0 },
            avatar_state: Some("portal".to_string()),
        });
        // Un-harvested resource nodes, tagged `resource:<kind>` for the client.
        for n in inst.arena.resources.iter().filter(|n| !n.harvested) {
            entities.push(wm::SnapshotEntity {
                entity_id: n.entity_id.clone(),
                position: n.position,
                velocity: wm::Velocity { x: 0.0, y: 0.0 },
                avatar_state: Some(format!("resource:{}", n.kind)),
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
        let balance = self.balance.clone();
        let Some(inst) = self.instance.as_mut() else {
            return out;
        };
        let battle_id = inst.battle_id.clone();
        let monster_idxs = inst.battle_monster_idxs.clone();
        // Combined XP for the whole encounter (touched creature + its group).
        let xp_reward: i64 = monster_idxs
            .iter()
            .filter_map(|&i| inst.arena.monsters.get(i))
            .map(|m| m.xp_reward)
            .sum();
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
                // The whole encounter is cleared from the overworld.
                for &i in &monster_idxs {
                    if let Some(m) = inst.arena.monsters.get_mut(i) {
                        m.defeated = true;
                        m.in_battle = false;
                    }
                }
                // Award XP to every participant; return their avatars to active.
                for r in inst.run.runs.iter_mut().filter(|r| bp.contains(&r.party_id)) {
                    r.award_xp(xp_reward, &balance);
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
                    // A felled creature may drop a Town Portal, topping up the
                    // player's ability to extract (start with one, find more).
                    let roll = roll_unit(inst.arena.seed ^ hash_str(pid) ^ now_ms());
                    if roll < balance.runs.town_portal_drop_chance {
                        let tp = ItemStack {
                            item_id: Uuid::now_v7().to_string(),
                            item_kind: TOWN_PORTAL.to_string(),
                            quantity: 1,
                            insurance: None,
                        };
                        if let Some(r) = inst.run.runs.iter_mut().find(|r| &r.player_id == pid) {
                            r.backpack.push(tp.clone());
                        }
                        out.push(out_msg(
                            pid,
                            &wr::BackpackUpdate {
                                changes: vec![wr::BackpackChange {
                                    item: tp,
                                    delta: "added".to_string(),
                                    cause: "town_portal_drop".to_string(),
                                }],
                            },
                        ));
                    }
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
        // Battle over: any surviving grouped creatures (e.g. after a flee) resume
        // roaming; reset merge + combatant bookkeeping.
        for &i in &monster_idxs {
            if let Some(m) = inst.arena.monsters.get_mut(i) {
                if !m.defeated {
                    m.in_battle = false;
                }
            }
        }
        inst.battle_parties.clear();
        inst.battle_monster_idxs.clear();
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
