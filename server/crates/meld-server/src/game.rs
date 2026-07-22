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
use meld_proto::common::{ItemStack, LootGear, Position};
use meld_proto::enums::*;
use meld_proto::realtime::{
    battle as wb, lobby as wl, movement as wm, run as wr, session as ws, world as ww, Message,
};
use meld_proto::RawEnvelope;
use meld_run::{build_battle, InstanceRun};
use meld_world::{Arena, Area};
use tokio::sync::mpsc;
use uuid::Uuid;

/// Events fed into the game loop from connection tasks.
pub enum ServerEvent {
    /// A socket completed the `session.authenticate` handshake.
    Connected {
        player_id: String,
        username: String,
        session_id: String,
        out: mpsc::Sender<String>,
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

/// A fire-and-forget persistence job. These writes never feed back into the game
/// state, so they run on a dedicated DB task and NEVER block the single
/// state-owning game loop on a Postgres round-trip — that inline blocking was the
/// main source of tick stalls / jitter under load (harvest XP fired on every
/// harvest, deaths, renames). Loads that *do* feed state back stay on the loop.
enum DbWrite {
    /// Apply the death durability sink for a player whose run just ended.
    Death(String),
    /// Credit harvested Meld-skill XP: (player, skill, xp).
    SkillXp(String, String, i64),
    /// Persist a hero rename: (player, slot, name).
    HeroRename(String, i16, String),
    /// Persist a hero's formation rank: (player, slot, back_row).
    HeroFormation(String, i16, bool),
    /// Mark that a player has begun their first dive (ends the tutorial world).
    Dived(String),
    /// Clear a player's pending-backpack queue: its contents were just drained
    /// into a freshly-formed run's live Backpack.
    ClearPendingBackpack(String),
}

/// Drain the DB-write queue on its own task, serializing writes off the hot path.
async fn run_db_writer(db: Db, mut rx: mpsc::UnboundedReceiver<DbWrite>) {
    while let Some(job) = rx.recv().await {
        match job {
            DbWrite::Death(pid) => {
                if let Ok(uid) = Uuid::parse_str(&pid) {
                    if let Err(e) = db.apply_death_durability(uid).await {
                        tracing::error!("death durability failed for {pid}: {e}");
                    }
                }
            }
            DbWrite::SkillXp(pid, skill, xp) => {
                if xp > 0 {
                    if let Ok(uid) = Uuid::parse_str(&pid) {
                        if let Err(e) = db.add_skill_xp(uid, &skill, xp).await {
                            tracing::error!("harvest skill xp failed for {pid}: {e}");
                        }
                    }
                }
            }
            DbWrite::HeroRename(pid, slot, name) => {
                if let Ok(uid) = Uuid::parse_str(&pid) {
                    if let Err(e) = db.set_hero_name(uid, slot, &name).await {
                        tracing::error!("hero rename persist failed for {pid}: {e}");
                    }
                }
            }
            DbWrite::HeroFormation(pid, slot, back_row) => {
                if let Ok(uid) = Uuid::parse_str(&pid) {
                    if let Err(e) = db.set_hero_row(uid, slot, back_row).await {
                        tracing::error!("hero formation persist failed for {pid}: {e}");
                    }
                }
            }
            DbWrite::Dived(pid) => {
                if let Ok(uid) = Uuid::parse_str(&pid) {
                    if let Err(e) = db.set_has_dived(uid).await {
                        tracing::error!("mark-dived persist failed for {pid}: {e}");
                    }
                }
            }
            DbWrite::ClearPendingBackpack(pid) => {
                if let Ok(uid) = Uuid::parse_str(&pid) {
                    if let Err(e) = db.clear_pending_backpack(uid).await {
                        tracing::error!("clear pending backpack failed for {pid}: {e}");
                    }
                }
            }
        }
    }
}

/// Spawn the game loop; returns a handle for the gateway.
pub fn spawn(balance: Arc<Balance>, db: Db) -> GameHandle {
    let (tx, rx) = mpsc::channel(1024);
    let (db_tx, db_rx) = mpsc::unbounded_channel::<DbWrite>();
    tokio::spawn(run_db_writer(db.clone(), db_rx));
    tokio::spawn(async move {
        GameState::new(balance, db, db_tx).run(rx).await;
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

/// Per-session outbound buffer. Bounded so a slow/stuck client can't make the
/// queue (and server memory) grow without limit while its snapshots pile up. At
/// the 10 Hz tick this is ~100 s of frames; a client that falls this far behind
/// is treated as dead and dropped (see [`GameState::dispatch`]) rather than
/// stalling the loop or leaking memory. The game loop only ever `try_send`s, so
/// a slow client never back-pressures the single state-owning task.
pub(crate) const OUT_CHANNEL_CAP: usize = 1024;

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

/// Combine a hero's Vault-equipped bonus (this run's baseline, loaded from the
/// account's persistent loadout at dive time) with any run-loot gear they've
/// equipped *this run* (`run.equip_loot`) — worn loot overrides the vault
/// baseline for its own category (weapon→atk, armor→def, accessory→spd)
/// rather than stacking, mirroring the one-item-per-category rule the Vault
/// already enforces. `hero_slot` is the party slot index (0-based).
fn effective_gear_bonus(
    vault: meld_db::GearBonus,
    looted: &[LootGear],
    hero_slot: i32,
) -> meld_run::GearBonus {
    let mut bonus = meld_run::GearBonus { atk: vault.atk, def: vault.def, spd: vault.spd };
    for g in looted {
        if g.equipped_hero_slot != Some(hero_slot) {
            continue;
        }
        match g.slot.as_str() {
            "weapon" => bonus.atk = g.atk_bonus,
            "armor" => bonus.def = g.def_bonus,
            "accessory" => bonus.spd = g.spd_bonus,
            _ => {}
        }
    }
    bonus
}

/// The per-hero class composition of a player's party of `size`. The picked class
/// leads; the rest are a fixed spread so a single party mixes classes that play
/// very differently (Hunter bruiser + Psyker channeler + Resonant healer).
fn party_composition(chosen: CharacterClass, size: usize) -> Vec<CharacterClass> {
    let base = [
        chosen,
        CharacterClass::Psyker,
        CharacterClass::Resonant,
        CharacterClass::Hunter,
    ];
    (0..size.max(1)).map(|i| base[i % base.len()]).collect()
}

/// A class's starting/max HP from balance (falls back to hunter).
fn class_base_hp(class: CharacterClass, balance: &Balance) -> i32 {
    balance
        .player
        .get(meld_run::class_key(class))
        .or_else(|| balance.player.get("hunter"))
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
    out: mpsc::Sender<String>,
    /// Logical session id — surfaced in `resume` blocks (resume slice, deferred).
    #[allow(dead_code)]
    session_id: String,
    seq_out: u32,
    last_client_seq: u32,
    in_instance: bool,
    /// Per-hero-slot combat bonuses from equipped gear, loaded from the DB
    /// after connect (each hero can wear different gear).
    gear_bonuses: Vec<meld_db::GearBonus>,
    /// Class chosen at the player's most recent `run.enter_maze` (default Hunter).
    /// This is the party *lead* (slot 0).
    character_class: CharacterClass,
    /// Explicit per-hero party composition from the builder, if the client sent
    /// one; otherwise `None` and the server builds a default mixed party.
    party_comp: Option<Vec<CharacterClass>>,
    /// Per-slot persistent hero names from the builder (also stored via `/v1/heroes`).
    hero_names: Option<Vec<String>>,
    /// Per-slot persistent formation flags (`true` = back row), loaded from the DB.
    hero_rows: Option<Vec<bool>>,
    /// Has this account ever dived? Loaded from the DB on connect. The very first
    /// dive is the gentle Forest-first tutorial (fixed biome order + centred area 0;
    /// roadmap WG-2); every dive after gets a randomized biome order + start.
    /// Defaults `false` until loaded; the load lands before the first `enter_maze`.
    has_dived: bool,
    /// Materials withdrawn from the Vault (storage chest), refreshed right
    /// before `run.enter_maze` is handled so `form_run` can drain them
    /// synchronously into the fresh run's Backpack (see `flush_pending_materials`).
    pending_materials: Vec<(String, i32)>,
}

/// One outbound message queued for a player, before seq assignment.
///
/// `payload` is *pre-serialized once* to raw JSON and shared via `Arc`. This is
/// the hot path: a snapshot/gauge broadcast serializes its (potentially large)
/// body a single time, then every recipient clones the cheap `Arc` and
/// [`dispatch`] embeds the same bytes verbatim into each session's envelope —
/// instead of the old path, which serialized the whole body once per recipient
/// *and* again when stringifying the envelope (2×N full serializations/tick).
struct Outgoing {
    player_id: String,
    msg_type: &'static str,
    payload: Arc<serde_json::value::RawValue>,
}

/// Serialize a message body to shared raw JSON exactly once.
fn serialize_payload<M: Message>(m: &M) -> Arc<serde_json::value::RawValue> {
    Arc::from(serde_json::value::to_raw_value(m).expect("payload serializes"))
}

fn out_msg<M: Message>(player_id: &str, m: &M) -> Outgoing {
    Outgoing {
        player_id: player_id.to_string(),
        msg_type: M::TYPE,
        payload: serialize_payload(m),
    }
}

/// Fan one message out to many recipients, serializing the body a single time.
/// Use this for every broadcast (snapshots, gauge updates, battle events shared
/// by a whole party) so per-tick cost is O(body) + O(recipients), not O(body ×
/// recipients).
fn broadcast<'a, M: Message>(
    player_ids: impl IntoIterator<Item = &'a str>,
    m: &M,
) -> Vec<Outgoing> {
    let payload = serialize_payload(m);
    let msg_type = M::TYPE;
    player_ids
        .into_iter()
        .map(|pid| Outgoing {
            player_id: pid.to_string(),
            msg_type,
            payload: payload.clone(),
        })
        .collect()
}

/// Like [`broadcast`] but for a serialize-only body (e.g. a borrowing struct that
/// can't be `DeserializeOwned`, so it isn't a [`Message`]). The wire `type` is
/// passed explicitly. Used by the per-tick gauge_update, whose body borrows each
/// fighter's cached wire-status list to avoid allocating per tick.
fn broadcast_ser<'a>(
    player_ids: impl IntoIterator<Item = &'a str>,
    msg_type: &'static str,
    m: &impl serde::Serialize,
) -> Vec<Outgoing> {
    let payload: Arc<serde_json::value::RawValue> =
        Arc::from(serde_json::value::to_raw_value(m).expect("payload serializes"));
    player_ids
        .into_iter()
        .map(|pid| Outgoing {
            player_id: pid.to_string(),
            msg_type,
            payload: payload.clone(),
        })
        .collect()
}

/// Convert a generated [`Area`] into a `world.terrain_section` wire message. The
/// client builds one stepped ground+cliff mesh from `levels` and spawns the
/// connector props. `path` carries the section's trail contribution — non-empty for
/// streamed sections (they extend the trail); the initial chain's path already
/// rides `run.started.path`, so those pass an empty vec.
fn terrain_section_msg(area: &Area, path: Vec<Position>) -> ww::TerrainSection {
    let t = &area.terrain;
    ww::TerrainSection {
        index: area.index as u32,
        start_x: t.start_x,
        end_x: area.end_x,
        y_min: t.y_min,
        cell: t.cell,
        cols: t.cols as u32,
        rows: t.rows as u32,
        levels: t.level.clone(),
        connectors: t
            .connectors
            .iter()
            .map(|c| ww::ConnectorDto {
                kind: c.kind.as_str().to_string(),
                position: c.position,
                lo: c.lo,
                hi: c.hi,
                radius: c.radius,
            })
            .collect(),
        path,
    }
}

/// Tag each ally combatant with its hero's persistent name (`name:<name>`) so the
/// client shows names in battle. Slot = the combatant's index among its player's
/// combatants; the name comes from `ActiveInstance::hero_names`.
fn inject_hero_names(
    player_combatants: &HashMap<String, Vec<String>>,
    hero_names: &HashMap<String, Vec<String>>,
    allies: &mut [meld_proto::common::Combatant],
) {
    for c in allies.iter_mut() {
        let Some(pid) = &c.player_id else { continue };
        if let (Some(cids), Some(names)) =
            (player_combatants.get(pid), hero_names.get(pid))
        {
            if let Some(slot) = cids.iter().position(|x| x == &c.combatant_id) {
                if let Some(n) = names.get(slot) {
                    c.statuses.push(format!("name:{n}"));
                }
            }
        }
    }
}

/// An in-progress extraction channel (interruptible; completes → bank).
struct Extraction {
    completes_at: u64,
    /// `"portal"` or `"town_portal"` — a town-portal channel consumes one Town
    /// Portal item on completion.
    method: String,
}

/// One in-progress battle within the instance. Several run **concurrently** — a
/// party that touches a free creature starts its own; a nearby party can merge
/// into an existing one via `run.join_battle`. All the state a fight owns lives
/// here, so ending a battle is just dropping its slot (CANON §S: one task, no
/// locks — this is a plain `Vec`, not shared).
struct BattleSlot {
    battle: Battle,
    battle_id: String,
    /// Stable `entity_id`s of every creature in this encounter (the touched
    /// creature plus its nearby group), so victory marks them all defeated and
    /// awards their combined XP. Ids, not vec indices, so `Arena::prune_defeated`
    /// can compact `arena.monsters` between ticks without corrupting this battle.
    monster_ids: Vec<String>,
    /// combatant_id -> player_id, for the players in THIS battle only.
    combatant_player: HashMap<String, String>,
    /// player_id -> the combatant ids they control in THIS battle.
    player_combatants: HashMap<String, Vec<String>>,
    /// Party ids merged into this battle (raid merge).
    parties: std::collections::HashSet<u32>,
    /// Overworld position of this fight (the touched creature's spot), so a nearby
    /// teammate can opt in via `run.join_battle`.
    pos: Position,
}

/// The single active MazeInstance of the slice.
struct ActiveInstance {
    arena: Arena,
    run: InstanceRun,
    /// Every battle currently running in the instance. Independent parties fight
    /// separate encounters at the same time; each is one [`BattleSlot`].
    battles: Vec<BattleSlot>,
    /// player_id -> per-hero current HP (length = party_size_per_player), carried
    /// across the run's battles so wounds persist (no free heal between fights).
    /// Reset to full only when a player (re)enters the maze — a fresh dive.
    hero_hp: HashMap<String, Vec<i32>>,
    /// player_id -> per-hero class (the mixed party composition), parallel to
    /// `hero_hp`. Each slot's class drives its stats/kit for the whole run.
    party_classes: HashMap<String, Vec<CharacterClass>>,
    /// player_id -> per-hero display name (parallel to `party_classes`).
    hero_names: HashMap<String, Vec<String>>,
    /// player_id -> per-hero formation flag (`true` = back row), parallel to
    /// `party_classes`. Empty = fall back to each class's default row.
    hero_rows: HashMap<String, Vec<bool>>,
    /// player_id -> active extraction channel.
    extraction: HashMap<String, Extraction>,
    /// player_id -> fractional HP carried over between ticks for the Resonant
    /// "Overworld Regen" perk (regen is HP/sec but `hero_hp` is integer, so we
    /// bank the sub-1 remainder here and apply whole HP as it accrues).
    regen_accum: HashMap<String, f32>,
}

impl ActiveInstance {
    /// The party id a player belongs to (their run's `party_id`).
    fn party_id_of(&self, player_id: &str) -> Option<u32> {
        self.run
            .runs
            .iter()
            .find(|r| r.player_id == player_id)
            .map(|r| r.party_id)
    }
    /// The battle a party is currently in, if any.
    fn battle_of_party(&self, party_id: u32) -> Option<&BattleSlot> {
        self.battles.iter().find(|b| b.parties.contains(&party_id))
    }
    /// The battle a player is currently in, if any.
    fn battle_of_player(&self, player_id: &str) -> Option<&BattleSlot> {
        let pid = self.party_id_of(player_id)?;
        self.battle_of_party(pid)
    }
    fn battle_by_id(&self, battle_id: &str) -> Option<&BattleSlot> {
        self.battles.iter().find(|b| b.battle_id == battle_id)
    }
    fn battle_by_id_mut(&mut self, battle_id: &str) -> Option<&mut BattleSlot> {
        self.battles.iter_mut().find(|b| b.battle_id == battle_id)
    }
    /// Every party id currently in some battle (union across all slots). Used to
    /// scope overworld snapshots to the players who are still roaming.
    fn parties_in_battle(&self) -> std::collections::HashSet<u32> {
        self.battles.iter().flat_map(|b| b.parties.iter().copied()).collect()
    }
    /// The players (across every merged party) in a given battle.
    fn members_of(&self, slot: &BattleSlot) -> Vec<String> {
        self.run
            .runs
            .iter()
            .filter(|r| slot.parties.contains(&r.party_id))
            .map(|r| r.player_id.clone())
            .collect()
    }
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
    /// Loads feed session state back, so they stay on the loop (they only await
    /// Postgres when a player actually connects — infrequent, not per-tick).
    pending_gear_load: Vec<String>,
    /// Players whose persistent hero names should be loaded from Postgres.
    pending_hero_load: Vec<String>,
    /// Fire-and-forget persistence sink, drained by [`run_db_writer`] off the loop.
    db_writes: mpsc::UnboundedSender<DbWrite>,
}

impl GameState {
    fn new(balance: Arc<Balance>, db: Db, db_writes: mpsc::UnboundedSender<DbWrite>) -> Self {
        GameState {
            balance,
            db,
            sessions: HashMap::new(),
            order: Vec::new(),
            instance: None,
            lobbies: HashMap::new(),
            player_lobby: HashMap::new(),
            pending_gear_load: Vec::new(),
            pending_hero_load: Vec::new(),
            db_writes,
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
                        let out = self.handle_event(ev).await;
                        self.dispatch(out);
                    }
                    None => break, // all senders dropped
                },
                _ = ticker.tick() => {
                    let out = self.tick();
                    self.dispatch(out);
                }
            }
            // Async DB side-effects that feed session/run state back run after
            // either arm (they only await Postgres when there is pending work —
            // a fresh connect or a completed extraction, not every tick). Deaths,
            // harvest XP and renames are fire-and-forget and go to `run_db_writer`
            // off this task, so the tick never blocks on those round-trips.
            self.flush_gear_loads().await;
            self.flush_hero_loads().await;
            let banked = self.complete_extractions().await;
            self.dispatch(banked);
        }
    }

    fn dispatch(&mut self, out: Vec<Outgoing>) {
        if out.is_empty() {
            return;
        }
        let ts = now_ms();
        let mut slow: Vec<String> = Vec::new();
        for o in out {
            if let Some(s) = self.sessions.get_mut(&o.player_id) {
                // Build the envelope by embedding the already-serialized payload
                // bytes verbatim — `msg_type` is a static wire literal (no escaping
                // needed) and `RawValue::get()` is the payload's raw JSON, so the
                // (large) body is never walked again here.
                let env = format!(
                    "{{\"type\":\"{}\",\"seq\":{},\"ts\":{},\"payload\":{}}}",
                    o.msg_type,
                    s.seq_out,
                    ts,
                    o.payload.get(),
                );
                s.seq_out = s.seq_out.wrapping_add(1);
                // Non-blocking: the loop must never park on a slow socket. A full
                // buffer means the client is too far behind to catch up — drop it.
                if let Err(mpsc::error::TrySendError::Full(_)) = s.out.try_send(env) {
                    slow.push(o.player_id.clone());
                }
            }
        }
        // Force-disconnect over-buffered clients: removing the session drops its
        // `out` sender, ending the gateway writer, which triggers the normal
        // `Disconnected` cleanup path. Better a reconnect than an unbounded queue.
        for pid in slow {
            tracing::warn!("dropping slow client {pid}: outbound buffer full");
            self.sessions.remove(&pid);
        }
    }

    // --- event handling -----------------------------------------------------

    async fn handle_event(&mut self, ev: ServerEvent) -> Vec<Outgoing> {
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
                        gear_bonuses: Vec::new(),
                        character_class: CharacterClass::Hunter,
                        party_comp: None,
                        hero_names: None,
                        hero_rows: None,
                        has_dived: false,
                        pending_materials: Vec::new(),
                    },
                );
                self.order.push(player_id.clone());
                self.pending_gear_load.push(player_id.clone());
                self.pending_hero_load.push(player_id);
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
                self.pending_hero_load.retain(|p| p != &player_id);
                self.remove_from_instance(&player_id);
                out
            }
            ServerEvent::Client { player_id, raw } => {
                // A dive is about to (re)seed the run's Backpack from whatever
                // materials are pending withdrawal — make sure that's fresh
                // *before* `form_run` runs synchronously inside `handle_client`
                // (see `flush_pending_materials`'s doc comment for why this can't
                // just be queued like the gear-bonus reload).
                if raw.msg_type == wr::EnterMaze::TYPE {
                    self.flush_pending_materials(&player_id).await;
                }
                self.handle_client(&player_id, raw)
            }
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
        // Drop the player's combatant bookkeeping from whichever battle held them.
        for slot in inst.battles.iter_mut() {
            if let Some(cids) = slot.player_combatants.remove(player_id) {
                for cid in cids {
                    slot.combatant_player.remove(&cid);
                }
            }
        }
        inst.hero_hp.remove(player_id);
        inst.party_classes.remove(player_id);
        inst.hero_names.remove(player_id);
        inst.hero_rows.remove(player_id);
        inst.extraction.remove(player_id);
        if inst.run.runs.is_empty() {
            self.instance = None;
        }
    }

    /// A player's run has ended (extracted or died): release them so they can
    /// dive again from the hub. Clears the session's in-instance flag and drops
    /// their run/avatar/bookkeeping from the shared instance (tearing the
    /// instance down if they were the last one). Without this, `in_instance`
    /// stays `true` after a run ends and the next `enter_maze` is rejected with
    /// "A run is already active for you." — the extract-or-die loop can't close.
    fn release_from_run(&mut self, player_id: &str) {
        if let Some(s) = self.sessions.get_mut(player_id) {
            s.in_instance = false;
        }
        self.remove_from_instance(player_id);
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
            wr::OpenChest::TYPE => self.handle_open_chest(player_id, raw),
            wr::JoinBattle::TYPE => self.handle_join_battle(player_id, raw),
            wr::RenameHero::TYPE => self.handle_rename_hero(player_id, raw),
            wr::SetFormation::TYPE => self.handle_set_formation(player_id, raw),
            wr::EquipLoot::TYPE => self.handle_equip_loot(player_id, raw),
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
            .unwrap_or(CharacterClass::Hunter);
        let names = req
            .as_ref()
            .and_then(|e| e.names.clone())
            .filter(|n| !n.is_empty());
        if let Some(s) = self.sessions.get_mut(player_id) {
            s.character_class = chosen;
            s.party_comp = party_comp;
            // Only override the DB-loaded names if the client explicitly sent some.
            if names.is_some() {
                s.hero_names = names;
            }
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
    /// The caller's hero roster (name/class/level/attributes) for the party panel.
    /// Reuses `party_fighters` so the stats match combat exactly.
    fn party_views(&self, pid: &str) -> Vec<wr::HeroView> {
        let Some(inst) = self.instance.as_ref() else {
            return Vec::new();
        };
        let Some(comp) = inst.party_classes.get(pid).cloned() else {
            return Vec::new();
        };
        let names = inst.hero_names.get(pid).cloned().unwrap_or_default();
        let rows = inst.hero_rows.get(pid).cloned().unwrap_or_default();
        // Reflect each hero's own equipped gear (Vault baseline + any run-loot
        // worn this run) so the party panel matches combat.
        let hero_bonuses = self.sessions.get(pid).map(|s| &s.gear_bonuses);
        let looted = inst
            .run
            .runs
            .iter()
            .find(|r| r.player_id == pid)
            .map(|r| r.looted_gear.as_slice())
            .unwrap_or(&[]);
        let party: Vec<meld_run::PartyMember> = comp
            .iter()
            .enumerate()
            .map(|(slot, c)| {
                let b = hero_bonuses.and_then(|v| v.get(slot)).copied().unwrap_or_default();
                (pid.to_string(), String::new(), *c, effective_gear_bonus(b, looted, slot as i32))
            })
            .collect();
        let row_overrides: Vec<Option<bool>> = rows.iter().map(|r| Some(*r)).collect();
        let fighters = meld_run::party_fighters(&party, &inst.run, &self.balance, &row_overrides);
        fighters
            .iter()
            .enumerate()
            .map(|(slot, f)| wr::HeroView {
                slot: slot as i32,
                name: names
                    .get(slot)
                    .cloned()
                    .unwrap_or_else(|| format!("Hero {}", slot + 1)),
                class_key: f.class_key.clone(),
                level: f.level,
                str_: f.str_,
                mnd: f.mnd,
                dex: f.dex,
                wll: f.wll,
                max_hp: f.max_hp,
                back_row: f.back_row,
            })
            .collect()
    }

    /// A player's earned overworld class perks (Overworld Class Perks / "party
    /// sense"): class *presence* in the party gates each perk, the shared
    /// `run_level` scales its tier. Defaults (no perks) if the player isn't in
    /// the instance. See [`Self::compute_perks`] and the `[perks]` balance block.
    fn perks_for(&self, pid: &str) -> wr::Perks {
        let Some(inst) = self.instance.as_ref() else {
            return wr::Perks::default();
        };
        let Some(classes) = inst.party_classes.get(pid) else {
            return wr::Perks::default();
        };
        let run_level = inst
            .run
            .runs
            .iter()
            .find(|r| r.player_id == pid)
            .map(|r| r.run_level)
            .unwrap_or(1);
        self.compute_perks(classes, run_level)
    }

    /// Pure mapping from (party classes × run level) → earned perks against the
    /// `[perks]` balance thresholds. A perk stays neutral unless its class is in
    /// the party. Kept deterministic + side-effect-free so it can be unit-tested.
    fn compute_perks(&self, classes: &[CharacterClass], run_level: i32) -> wr::Perks {
        let p = &self.balance.perks;
        let has = |c: CharacterClass| classes.contains(&c);
        let lvl = run_level.max(1);
        let above = |floor: i32| (lvl - floor).max(0) as f32;
        let mut out = wr::Perks::default();
        // Hunter — night glow + "predator's eye" monster intel.
        if has(CharacterClass::Hunter) {
            out.hunter_glow = p.hunter_glow_base + p.hunter_glow_per_level * (lvl - 1) as f32;
            out.hunter_intel = if lvl >= p.hunter_intel_atb_at {
                3
            } else if lvl >= p.hunter_intel_hp_at {
                2
            } else if lvl >= p.hunter_intel_level_at {
                1
            } else {
                0
            };
        }
        // Shifter — corner minimap.
        if has(CharacterClass::Shifter) && lvl >= p.shifter_map_at {
            out.shifter_map = if lvl >= p.shifter_map_harvest_at {
                3
            } else if lvl >= p.shifter_map_chests_at {
                2
            } else {
                1
            };
            out.shifter_map_radius =
                p.shifter_map_radius_base + p.shifter_map_radius_per_level * above(p.shifter_map_at);
        }
        // Psyker — threat sense.
        if has(CharacterClass::Psyker) && lvl >= p.psyker_threat_elites_at {
            out.psyker_threat = if lvl >= p.psyker_threat_aggro_at { 2 } else { 1 };
            out.psyker_reveal_radius = (p.psyker_reveal_base
                + p.psyker_reveal_per_level * above(p.psyker_threat_elites_at) as f64)
                as f32;
        }
        // Resonant — overworld regen (HP/sec).
        if has(CharacterClass::Resonant) {
            out.resonant_regen = p.resonant_regen_per_level * lvl as f32;
        }
        // Iron Hull — bulwark (shrinks how close creatures chase this party).
        if has(CharacterClass::IronHull) {
            let mult = 1.0 - p.ironhull_aggro_reduction_per_level * lvl as f64;
            out.ironhull_aggro_mult = mult.max(p.ironhull_aggro_mult_floor) as f32;
        }
        out
    }

    /// Resonant "Overworld Regen": restore carried hero HP over time while a party
    /// roams (not in battle). Regen is HP/sec but `hero_hp` is integer, so the
    /// sub-1 remainder is banked in `regen_accum` and whole HP is applied as it
    /// accrues, most-wounded living hero first (downed heroes at 0 HP are not
    /// revived — that needs a real fight). Purely server state.
    fn apply_overworld_regen(&mut self, dt: f64) {
        // Plan first (shared borrow), then mutate hero_hp (exclusive borrow).
        let plans: Vec<(String, f32, Vec<i32>)> = {
            let Some(inst) = self.instance.as_ref() else {
                return;
            };
            let in_battle = inst.parties_in_battle();
            let mut v = Vec::new();
            for r in &inst.run.runs {
                if in_battle.contains(&r.party_id) {
                    continue;
                }
                let regen = self.perks_for(&r.player_id).resonant_regen;
                if regen <= 0.0 {
                    continue;
                }
                let caps: Vec<i32> = self
                    .party_views(&r.player_id)
                    .iter()
                    .map(|h| h.max_hp)
                    .collect();
                v.push((r.player_id.clone(), regen, caps));
            }
            v
        };
        if plans.is_empty() {
            return;
        }
        let Some(inst) = self.instance.as_mut() else {
            return;
        };
        for (pid, regen, caps) in plans {
            let acc = inst.regen_accum.entry(pid.clone()).or_insert(0.0);
            *acc += regen * dt as f32;
            let whole = acc.floor();
            if whole < 1.0 {
                continue;
            }
            *acc -= whole;
            let mut budget = whole as i32;
            let Some(hps) = inst.hero_hp.get_mut(&pid) else {
                continue;
            };
            while budget > 0 {
                // Most-wounded living hero still below its cap.
                let mut best: Option<usize> = None;
                let mut best_deficit = 0;
                for (i, h) in hps.iter().enumerate() {
                    let cap = caps.get(i).copied().unwrap_or(*h);
                    let deficit = cap - *h;
                    if *h > 0 && deficit > best_deficit {
                        best_deficit = deficit;
                        best = Some(i);
                    }
                }
                let Some(i) = best else { break };
                hps[i] += 1;
                budget -= 1;
            }
        }
    }

    /// Per-hero stat gains for a party level-up (old_level → new_level), for the
    /// classic JRPG "LEVEL UP!" screen. Mirrors the `party_fighters` derivation
    /// (max HP from Wll; the four attributes from `attributes_at`) so the numbers
    /// exactly match the party panel.
    fn hero_level_ups(&self, pid: &str, old_level: i32, new_level: i32) -> Vec<wr::HeroLevelUp> {
        let Some(inst) = self.instance.as_ref() else {
            return Vec::new();
        };
        let Some(comp) = inst.party_classes.get(pid).cloned() else {
            return Vec::new();
        };
        let names = inst.hero_names.get(pid).cloned().unwrap_or_default();
        let b = &self.balance;
        let a = &b.attributes;
        // (max_hp, str, mnd, dex, wll) for a class at a level — same formula as
        // meld_run::party_fighters (attributes_at + Wll→HP growth).
        let statline = |class: meld_proto::enums::CharacterClass, level: i32| {
            let key = meld_run::class_key(class);
            let s = b
                .player
                .get(key)
                .unwrap_or_else(|| b.player.get("hunter").expect("hunter stats"));
            let (str_, mnd, dex, wll) = s.attributes_at(level);
            let grow = |attr: i32, base: i32, coef: f64| ((attr - base) as f64 * coef).round() as i32;
            let max_hp = s.base_hp + grow(wll, s.wll, a.wll_to_hp);
            (max_hp, str_, mnd, dex, wll)
        };
        comp.iter()
            .enumerate()
            .map(|(slot, class)| {
                let (hp0, st0, mn0, dx0, wl0) = statline(*class, old_level);
                let (hp1, st1, mn1, dx1, wl1) = statline(*class, new_level);
                wr::HeroLevelUp {
                    slot: slot as i32,
                    name: names
                        .get(slot)
                        .cloned()
                        .unwrap_or_else(|| format!("Hero {}", slot + 1)),
                    class_key: meld_run::class_key(*class).to_string(),
                    level: new_level,
                    max_hp_before: hp0,
                    max_hp_after: hp1,
                    str_before: st0,
                    str_after: st1,
                    mnd_before: mn0,
                    mnd_after: mn1,
                    dex_before: dx0,
                    dex_after: dx1,
                    wll_before: wl0,
                    wll_after: wl1,
                }
            })
            .collect()
    }

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
            // The instance initiator's account decides the tutorial (roadmap WG-2):
            // an account's very first dive walks the gentle Forest-first biome order
            // with the centred area-0 onboarding; every dive after gets a randomized
            // biome order + start (WG-3). The world seed is always server-random —
            // the tutorial shapes the *structure*, not a fixed world.
            let tutorial = !self
                .sessions
                .get(initiator)
                .map(|s| s.has_dived)
                .unwrap_or(false);
            // Server-generated world seed (CANON: the client never supplies or
            // computes seeds).
            let seed = world_seed();
            self.instance = Some(ActiveInstance {
                arena: Arena::generate(&self.balance, seed, tutorial),
                run: InstanceRun::new(instance_id, departure_hub_distance, &self.balance),
                battles: Vec::new(),
                hero_hp: HashMap::new(),
                party_classes: HashMap::new(),
                hero_names: HashMap::new(),
                hero_rows: HashMap::new(),
                extraction: HashMap::new(),
                regen_accum: HashMap::new(),
            });
        }
        // Every diver's first dive ends their tutorial state, so their *next* run is
        // a fresh random world. Idempotent: only the not-yet-dived are persisted.
        for pid in &party_ids {
            if let Some(s) = self.sessions.get_mut(pid) {
                if !s.has_dived {
                    s.has_dived = true;
                    let _ = self.db_writes.send(DbWrite::Dived(pid.clone()));
                }
            }
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
                    .unwrap_or((String::new(), CharacterClass::Hunter));
                (pid.clone(), username, class, Uuid::now_v7().to_string())
            })
            .collect();
        // Each player is their OWN battle-party, so touching a creature pulls only
        // that player's heroes — teammates are never auto-dragged into a fight
        // (they opt in via `run.join_battle`). They still share the instance/arena
        // and dive together.
        for member in members {
            inst.run.add_party(vec![member]);
        }
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
        // Materials withdrawn from the Vault (storage chest) since the last dive
        // ride along into this fresh Backpack — `flush_pending_materials` (called
        // just before this handler, see `handle_event`) guarantees the session
        // field is current. Persisted clearing is fire-and-forget; it doesn't
        // block forming the run.
        for pid in &party_ids {
            let materials = self
                .sessions
                .get_mut(pid)
                .map(|s| std::mem::take(&mut s.pending_materials))
                .unwrap_or_default();
            if materials.is_empty() {
                continue;
            }
            if let Some(r) = inst.run.run_mut(pid) {
                for (item_kind, quantity) in materials {
                    r.backpack.push(ItemStack {
                        item_id: Uuid::now_v7().to_string(),
                        item_kind,
                        quantity,
                        insurance: None,
                    });
                }
            }
            let _ = self.db_writes.send(DbWrite::ClearPendingBackpack(pid.clone()));
        }
        for pid in &party_ids {
            inst.arena.add_avatar(pid.clone(), speed);
        }
        // (Re)enter = a fresh dive: build each player's mixed party composition and
        // start every hero at its class's full HP. Within the run this HP persists
        // across battles (see hero_hp write-back).
        let party_size = self.balance.battle.party_size_per_player.max(1);
        for pid in &party_ids {
            let (chosen, explicit, names, rows) = self
                .sessions
                .get(pid)
                .map(|s| (s.character_class, s.party_comp.clone(), s.hero_names.clone(), s.hero_rows.clone()))
                .unwrap_or((CharacterClass::Hunter, None, None, None));
            // The builder's explicit composition wins (normalized to party size,
            // padded with Hunter); otherwise build a default mixed party around
            // the lead.
            let comp = match explicit {
                Some(mut p) => {
                    p.truncate(party_size);
                    while p.len() < party_size {
                        p.push(CharacterClass::Hunter);
                    }
                    p
                }
                None => party_composition(chosen, party_size),
            };
            // Hero names by slot: the builder's, normalized to party size and
            // defaulted to "Hero N" for any unnamed slot.
            let mut names = names.unwrap_or_default();
            names.truncate(party_size);
            while names.len() < party_size {
                names.push(format!("Hero {}", names.len() + 1));
            }
            let hp: Vec<i32> = comp.iter().map(|c| class_base_hp(*c, &self.balance)).collect();
            // Saved formation by slot, normalized to party size (missing = false).
            let mut rows = rows.unwrap_or_default();
            rows.truncate(party_size);
            while rows.len() < party_size {
                rows.push(false);
            }
            inst.party_classes.insert(pid.clone(), comp);
            inst.hero_hp.insert(pid.clone(), hp);
            inst.hero_names.insert(pid.clone(), names);
            inst.hero_rows.insert(pid.clone(), rows);
        }
        for pid in &party_ids {
            if let Some(s) = self.sessions.get_mut(pid) {
                s.in_instance = true;
            }
        }
        // Roster views per player (built before the shared instance borrow below).
        let rosters: HashMap<String, Vec<wr::HeroView>> = party_ids
            .iter()
            .map(|pid| (pid.clone(), self.party_views(pid)))
            .collect();
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

        // Shared world framing (same for every party member): walkable bounds +
        // biome-seam chokepoints, so the client can build edge/end walls and gates.
        let (bx_min, bx_max, blat) = inst.arena.bounds();
        let world_bounds = wr::WorldBounds {
            x_min: bx_min,
            x_max: bx_max,
            lateral: blat,
        };
        let seam_views: Vec<wr::SeamView> = inst
            .arena
            .seams
            .iter()
            .map(|s| wr::SeamView {
                x: s.x,
                gap_y: s.gap_y,
                gap_half_width: s.gap_half_width,
                biome_from: s.biome_from.to_string(),
                biome_to: s.biome_to.to_string(),
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
            let backpack = inst
                .run
                .runs
                .iter()
                .find(|r| &r.player_id == pid)
                .map(|r| r.backpack.clone())
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
                    backpack,
                    // A dive begins with no chits and no red loot — both are found
                    // in the maze and banked on extraction (economy.md S1).
                    chits: 0,
                    backpack_gear: Vec::new(),
                    path: inst.arena.path.clone(),
                    bounds: Some(world_bounds.clone()),
                    seams: seam_views.clone(),
                },
            ));
            out.push(out_msg(
                pid,
                &wr::Party {
                    heroes: rosters.get(pid).cloned().unwrap_or_default(),
                },
            ));
            // The caller's earned overworld class perks ("party sense").
            out.push(out_msg(pid, &self.perks_for(pid)));
            // Stream the initial chain's terrain (elevation grid + connectors) so
            // the client can build the stepped relief. Path rides run.started, so
            // these carry no path segment.
            for area in &inst.arena.areas {
                out.push(out_msg(pid, &terrain_section_msg(area, Vec::new())));
            }
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
                    p.push(CharacterClass::Hunter);
                }
                p
            }
            _ => party_composition(CharacterClass::Hunter, size),
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
                s.character_class = party.first().copied().unwrap_or(CharacterClass::Hunter);
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

        // WG-4: crossing the western border behind the hub returns you to Last City
        // (an instant extraction home — you keep your backpack). `complete_extractions`
        // banks it this same tick and sends the result, so no touch/battle is resolved.
        if self.west_return(player_id) {
            return Vec::new();
        }

        // Contact starts a battle. Checked here for an instant response to the
        // player's own move, and again every tick (see `tick`) so a creature that
        // walks into a *stationary* player also triggers the fight — otherwise
        // standing still made you immune to an aggressive creature closing on you.
        self.resolve_touches()
    }

    /// WG-4: if the player has walked WEST of the return border (behind the hub),
    /// send them home to Last City — the safe anchor is always just to the west,
    /// "behind a giant wall you can always step back through." This *abandons* the
    /// run (backpack forfeited, no death penalty): near spawn there's nothing to
    /// lose, and from deep the long walk back west is impractical, so it is never a
    /// free extraction. The client routes the `abandoned` result to the City screen.
    fn west_return(&mut self, player_id: &str) -> bool {
        let border = self.balance.worldgen.west_return_border;
        let Some(inst) = self.instance.as_mut() else {
            return false;
        };
        let west = inst
            .arena
            .avatar(player_id)
            .map(|a| a.position.x < border)
            .unwrap_or(false);
        // Already heading home? don't re-enqueue.
        if !west || inst.extraction.contains_key(player_id) {
            return false;
        }
        // The city is right there to the west — step back in and KEEP your backpack.
        // This is an INSTANT free extraction home (no channel, no death penalty, no
        // item cost): near spawn it's just "I changed my mind" and you shouldn't be
        // punished for it; from deep, walking all the way back west is its own
        // gauntlet, so it's a fair "fight your way home" route. `complete_extractions`
        // banks the backpack next tick (method != "town_portal", so nothing is spent).
        inst.extraction.insert(
            player_id.to_string(),
            Extraction {
                completes_at: now_ms(),
                method: "west_return".to_string(),
            },
        );
        true
    }

    /// Start a fresh battle for every active avatar currently in contact with a free
    /// creature. Loops because several players can be touched in the same tick;
    /// `start_battle` flips the toucher's avatar and its creature to `in_battle`, so
    /// each pass resolves a distinct contact and the loop drains in ≤ (avatars)
    /// passes. Independent battles run concurrently — one party's fight never blocks
    /// another's — and teammates still opt into an *ongoing* fight via `join_battle`.
    fn resolve_touches(&mut self) -> Vec<Outgoing> {
        let mut out = Vec::new();
        let max_passes = self
            .instance
            .as_ref()
            .map(|i| i.arena.avatars.len())
            .unwrap_or(0);
        for _ in 0..max_passes {
            let Some(inst) = self.instance.as_ref() else { break };
            let decision = inst.arena.check_touch().and_then(|(toucher, monster_idx)| {
                inst.run
                    .runs
                    .iter()
                    .find(|r| r.player_id == toucher)
                    .map(|r| (toucher, r.party_id, monster_idx))
            });
            match decision {
                Some((toucher, pid, monster_idx)) => {
                    out.extend(self.start_battle(&toucher, pid, monster_idx))
                }
                None => break,
            }
        }
        out
    }

    fn start_battle(&mut self, toucher: &str, party_id: u32, monster_idx: usize) -> Vec<Outgoing> {
        let seed = now_ms();
        let balance = self.balance.clone();
        // Snapshot gear bonuses before borrowing the instance mutably.
        let bonuses: HashMap<String, Vec<meld_db::GearBonus>> = self
            .sessions
            .iter()
            .map(|(k, s)| (k.clone(), s.gear_bonuses.clone()))
            .collect();
        let Some(inst) = self.instance.as_mut() else {
            return Vec::new();
        };

        let battle_id = Uuid::now_v7().to_string();
        let monster_combatant_id = Uuid::now_v7().to_string();

        // Assign combatant ids for the *touching* party only. This battle owns its
        // own combatant maps (a fresh slot), so concurrent battles never collide.
        let mut party: Vec<meld_run::PartyMember> = Vec::new();
        let mut combatant_player: HashMap<String, String> = HashMap::new();
        let mut player_combatants: HashMap<String, Vec<String>> = HashMap::new();
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
        let mut row_overrides: Vec<Option<bool>> = Vec::new();
        for r in inst.run.runs.iter().filter(|r| r.party_id == party_id) {
            let hero_bonuses = bonuses.get(&r.player_id);
            let hp_vec = inst.hero_hp.get(&r.player_id).cloned().unwrap_or_default();
            let row_vec = inst.hero_rows.get(&r.player_id).cloned().unwrap_or_default();
            let comp = inst
                .party_classes
                .get(&r.player_id)
                .cloned()
                .unwrap_or_else(|| party_composition(r.character_class, hp_vec.len().max(1)));
            let mut cids = Vec::new();
            for (slot, cls) in comp.iter().enumerate() {
                let cid = Uuid::now_v7().to_string();
                combatant_player.insert(cid.clone(), r.player_id.clone());
                // Each hero wears their own gear (per-character equip slots).
                let vault_bonus = hero_bonuses.and_then(|v| v.get(slot)).copied().unwrap_or_default();
                let bonus = effective_gear_bonus(vault_bonus, &r.looted_gear, slot as i32);
                party.push((r.player_id.clone(), cid.clone(), *cls, bonus));
                hp_overrides.push(hp_vec.get(slot).copied());
                // Some(row) forces the saved rank; None falls back to the class default.
                row_overrides.push(row_vec.get(slot).copied());
                cids.push(cid);
            }
            player_combatants.insert(r.player_id.clone(), cids);
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
            &row_overrides,
        );
        // Store the group's stable ids (indices are only valid until the next prune).
        let monster_ids: Vec<String> = group_idxs
            .iter()
            .filter_map(|&gi| inst.arena.monsters.get(gi).map(|m| m.entity_id.clone()))
            .collect();
        let slot = BattleSlot {
            battle,
            battle_id: battle_id.clone(),
            monster_ids,
            combatant_player,
            player_combatants,
            parties: std::iter::once(party_id).collect(),
            pos: inst
                .arena
                .monsters
                .get(monster_idx)
                .map(|m| m.position)
                .unwrap_or_else(|| Position::new(0.0, 0.0)),
        };
        let (mut allies, enemies) = slot.battle.wire_combatants();
        inject_hero_names(&slot.player_combatants, &inst.hero_names, &mut allies);

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

        let encounter_class = slot.battle.encounter_class;
        tracing::info!(
            battle_id = %battle_id,
            party = party_players.len(),
            enemies = group_idxs.len(),
            triggered_by = %toucher,
            active_battles = inst.battles.len() + 1,
            "battle started"
        );

        let mut out = Vec::new();
        for pid in &party_players {
            let yours = slot.player_combatants.get(pid).cloned().unwrap_or_default();
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
        inst.battles.push(slot);
        out
    }

    /// Opt into the nearby ongoing fight (`run.join_battle`). Validates that a
    /// battle is in progress, the caller isn't already in it, and their avatar is
    /// within `join_radius` of the fight — then merges their party in.
    fn handle_join_battle(&mut self, player_id: &str, raw: RawEnvelope) -> Vec<Outgoing> {
        let join_radius = self.balance.ai.join_radius;
        let (party_id, battle_id) = {
            let Some(inst) = self.instance.as_ref() else {
                return vec![error(player_id, ErrorCode::InvalidState, "Not in a run.", Some(raw.seq))];
            };
            if inst.battles.is_empty() {
                return vec![error(player_id, ErrorCode::InvalidState, "No fight in progress.", Some(raw.seq))];
            }
            let Some(pid) = inst.party_id_of(player_id) else {
                return vec![error(player_id, ErrorCode::NotFound, "No run for you.", Some(raw.seq))];
            };
            if inst.battle_of_party(pid).is_some() {
                return vec![error(player_id, ErrorCode::InvalidState, "You're already in a fight.", Some(raw.seq))];
            }
            let Some(pos) = inst.arena.avatar(player_id).map(|a| a.position) else {
                return vec![error(player_id, ErrorCode::NotFound, "No run for you.", Some(raw.seq))];
            };
            // Join the NEAREST battle within join_radius (concurrent battles: there
            // may be several going on around the map).
            let target = inst
                .battles
                .iter()
                .map(|b| (b.battle_id.clone(), pos.distance_to(&b.pos)))
                .filter(|(_, d)| *d <= join_radius)
                .min_by(|a, b| a.1.total_cmp(&b.1))
                .map(|(id, _)| id);
            let Some(battle_id) = target else {
                return vec![error(player_id, ErrorCode::OutOfRange, "Too far from any fight to join.", Some(raw.seq))];
            };
            (pid, battle_id)
        };
        self.join_battle(player_id, party_id, &battle_id)
    }

    /// Rename one of the caller's heroes: update the active run's names + the
    /// session cache (for the next dive), persist to Postgres, and re-send the
    /// roster so the party panel updates at once.
    fn handle_rename_hero(&mut self, player_id: &str, raw: RawEnvelope) -> Vec<Outgoing> {
        let req: wr::RenameHero = match serde_json::from_value(raw.payload) {
            Ok(v) => v,
            Err(_) => {
                return vec![error(player_id, ErrorCode::ValidationError, "bad rename_hero", Some(raw.seq))]
            }
        };
        let party_size = self.balance.battle.party_size_per_player.max(1) as i32;
        let name: String = req.name.trim().chars().take(24).collect();
        if name.is_empty() || req.slot < 0 || req.slot >= party_size {
            return vec![error(player_id, ErrorCode::ValidationError, "Invalid hero name or slot.", Some(raw.seq))];
        }
        let slot = req.slot as usize;
        // Active run's names (so battle + panel reflect it now).
        if let Some(inst) = self.instance.as_mut() {
            if let Some(names) = inst.hero_names.get_mut(player_id) {
                if let Some(n) = names.get_mut(slot) {
                    *n = name.clone();
                }
            }
        }
        // Session cache (used to form the next dive).
        if let Some(s) = self.sessions.get_mut(player_id) {
            let mut v = s.hero_names.clone().unwrap_or_default();
            while v.len() <= slot {
                v.push(format!("Hero {}", v.len() + 1));
            }
            v[slot] = name.clone();
            s.hero_names = Some(v);
        }
        let _ = self
            .db_writes
            .send(DbWrite::HeroRename(player_id.to_string(), slot as i16, name));
        vec![out_msg(
            player_id,
            &wr::Party {
                heroes: self.party_views(player_id),
            },
        )]
    }

    /// Set one of the caller's heroes to the front or back row: update the active
    /// run's formation + the session cache (for the next dive), persist to Postgres,
    /// and re-send the roster so the party panel updates at once. Applies to the
    /// next battle assembled (an in-progress battle's Fighters are already built).
    fn handle_set_formation(&mut self, player_id: &str, raw: RawEnvelope) -> Vec<Outgoing> {
        let req: wr::SetFormation = match serde_json::from_value(raw.payload) {
            Ok(v) => v,
            Err(_) => {
                return vec![error(player_id, ErrorCode::ValidationError, "bad set_formation", Some(raw.seq))]
            }
        };
        let party_size = self.balance.battle.party_size_per_player.max(1) as i32;
        if req.slot < 0 || req.slot >= party_size {
            return vec![error(player_id, ErrorCode::ValidationError, "Invalid hero slot.", Some(raw.seq))];
        }
        let slot = req.slot as usize;
        let back = req.back_row;
        // Active run's formation (so the panel + next battle reflect it).
        if let Some(inst) = self.instance.as_mut() {
            let rows = inst.hero_rows.entry(player_id.to_string()).or_default();
            while rows.len() <= slot {
                rows.push(false);
            }
            rows[slot] = back;
        }
        // Session cache (used to form the next dive).
        if let Some(s) = self.sessions.get_mut(player_id) {
            let mut v = s.hero_rows.clone().unwrap_or_default();
            while v.len() <= slot {
                v.push(false);
            }
            v[slot] = back;
            s.hero_rows = Some(v);
        }
        let _ = self
            .db_writes
            .send(DbWrite::HeroFormation(player_id.to_string(), slot as i16, back));
        vec![out_msg(
            player_id,
            &wr::Party {
                heroes: self.party_views(player_id),
            },
        )]
    }

    /// Equip (or unequip) a piece of this run's not-yet-banked loot gear onto a
    /// hero slot. Unlike Vault equip (HTTP, persists to Postgres, effective
    /// from the next dive), this only touches the in-memory run — no DB write,
    /// since red gear isn't owned until extraction anyway — and takes effect
    /// on the caller's very next battle via `effective_gear_bonus`.
    fn handle_equip_loot(&mut self, player_id: &str, raw: RawEnvelope) -> Vec<Outgoing> {
        let req: wr::EquipLoot = match serde_json::from_value(raw.payload) {
            Ok(v) => v,
            Err(_) => {
                return vec![error(player_id, ErrorCode::ValidationError, "bad equip_loot", Some(raw.seq))]
            }
        };
        if let Some(slot) = req.hero_slot {
            let party_size = self.balance.battle.party_size_per_player.max(1) as i32;
            if slot < 0 || slot >= party_size {
                return vec![error(player_id, ErrorCode::ValidationError, "Invalid hero slot.", Some(raw.seq))];
            }
        }
        let Some(inst) = self.instance.as_mut() else {
            return vec![error(player_id, ErrorCode::InvalidState, "Not in a run.", Some(raw.seq))];
        };
        let Some(r) = inst.run.run_mut(player_id) else {
            return vec![error(player_id, ErrorCode::InvalidState, "Not in a run.", Some(raw.seq))];
        };
        let Some(idx) = r.looted_gear.iter().position(|g| g.gear_id == req.gear_id) else {
            return vec![error(player_id, ErrorCode::NotFound, "No such loot gear.", Some(raw.seq))];
        };
        if let Some(slot) = req.hero_slot {
            // One item per hero+category: unequip anything else this hero has
            // worn this run in the same category before wearing the new one.
            let category = r.looted_gear[idx].slot.clone();
            for g in r.looted_gear.iter_mut() {
                if g.slot == category && g.equipped_hero_slot == Some(slot) {
                    g.equipped_hero_slot = None;
                }
            }
            r.looted_gear[idx].equipped_hero_slot = Some(slot);
        } else {
            r.looted_gear[idx].equipped_hero_slot = None;
        }
        vec![out_msg(player_id, &wr::RunGear { gear: r.looted_gear.clone() })]
    }

    /// Merge a party into the in-progress battle (the toucher opted in via
    /// `run.join_battle`). The joiner brings their full hero composition, exactly
    /// as if they'd started the fight.
    fn join_battle(&mut self, toucher: &str, party_id: u32, battle_id: &str) -> Vec<Outgoing> {
        let balance = self.balance.clone();
        let cap =
            meld_proto::limits::PARTY_MAX * self.balance.battle.merge_cap_normal_instances.max(1) as usize;
        let bonuses: HashMap<String, Vec<meld_db::GearBonus>> = self
            .sessions
            .iter()
            .map(|(k, s)| (k.clone(), s.gear_bonuses.clone()))
            .collect();
        let Some(inst) = self.instance.as_mut() else {
            return Vec::new();
        };
        if inst.battle_by_id(battle_id).is_none() {
            return Vec::new();
        }

        // Build the joining party's combatants — the joiner's full hero
        // composition (parallel to `hero_hp`), just like starting a fight. These go
        // into the target battle's own combatant maps.
        let joiners: Vec<String> = inst
            .run
            .runs
            .iter()
            .filter(|r| r.party_id == party_id)
            .map(|r| r.player_id.clone())
            .collect();
        let mut party: Vec<meld_run::PartyMember> = Vec::new();
        let mut hp_overrides: Vec<Option<i32>> = Vec::new();
        let mut row_overrides: Vec<Option<bool>> = Vec::new();
        let mut add_combatant_player: HashMap<String, String> = HashMap::new();
        let mut add_player_combatants: HashMap<String, Vec<String>> = HashMap::new();
        for pid in &joiners {
            let r_ref = inst.run.runs.iter().find(|r| &r.player_id == pid);
            let lead = r_ref.map(|r| r.character_class).unwrap_or(CharacterClass::Hunter);
            let looted = r_ref.map(|r| r.looted_gear.as_slice()).unwrap_or(&[]);
            let comp = inst
                .party_classes
                .get(pid)
                .cloned()
                .unwrap_or_else(|| vec![lead]);
            let hp_vec = inst.hero_hp.get(pid).cloned().unwrap_or_default();
            let row_vec = inst.hero_rows.get(pid).cloned().unwrap_or_default();
            let hero_bonuses = bonuses.get(pid);
            let mut cids = Vec::new();
            for (slot, cls) in comp.iter().enumerate() {
                let cid = Uuid::now_v7().to_string();
                add_combatant_player.insert(cid.clone(), pid.clone());
                // Each hero wears their own gear (per-character equip slots).
                let vault_bonus = hero_bonuses.and_then(|v| v.get(slot)).copied().unwrap_or_default();
                let bonus = effective_gear_bonus(vault_bonus, looted, slot as i32);
                party.push((pid.clone(), cid.clone(), *cls, bonus));
                hp_overrides.push(hp_vec.get(slot).copied());
                row_overrides.push(row_vec.get(slot).copied());
                cids.push(cid);
            }
            add_player_combatants.insert(pid.clone(), cids);
        }
        if party.is_empty() {
            return Vec::new();
        }
        // Merge cap: a touch that would exceed it does not merge (combat-atb.md).
        let current = inst.battle_by_id(battle_id).unwrap().battle.player_count();
        if current + party.len() > cap {
            return Vec::new();
        }

        let mut fighters = meld_run::party_fighters(&party, &inst.run, &balance, &row_overrides);
        // Carry each joining hero's persisted HP into the merged battle.
        for (f, hp) in fighters.iter_mut().zip(hp_overrides.iter()) {
            if let Some(h) = hp {
                f.hp = (*h).clamp(0, f.max_hp);
            }
        }

        // Apply to the target battle slot, then extract what messaging needs.
        let (encounter_class, mut allies, enemies, joined_pc) = {
            let slot = inst.battle_by_id_mut(battle_id).unwrap();
            slot.battle.join(fighters);
            slot.parties.insert(party_id);
            for (k, v) in add_combatant_player {
                slot.combatant_player.insert(k, v);
            }
            for (k, v) in add_player_combatants {
                slot.player_combatants.insert(k, v);
            }
            let (allies, enemies) = slot.battle.wire_combatants();
            (slot.battle.encounter_class, allies, enemies, slot.player_combatants.clone())
        };
        inject_hero_names(&joined_pc, &inst.hero_names, &mut allies);
        for pid in &joiners {
            if let Some(a) = inst.arena.avatar_mut(pid) {
                a.state = "in_battle".to_string();
            }
        }

        let battle_id = battle_id.to_string();
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
            let yours = joined_pc.get(pid).cloned().unwrap_or_default();
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
        let members = inst
            .battle_by_id(&battle_id)
            .map(|s| inst.members_of(s))
            .unwrap_or_default();
        let existing: Vec<String> = members
            .into_iter()
            .filter(|pid| !joiners.contains(pid))
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
        // Route to the battle named in the request, and only if the sender is
        // actually in it (with concurrent battles, the id disambiguates which one).
        let owned = match inst.battle_by_id(&submit.battle_id) {
            Some(slot) => slot.player_combatants.get(player_id).cloned().unwrap_or_default(),
            None => {
                return vec![error(
                    player_id,
                    ErrorCode::NotFound,
                    "Unknown battle.",
                    Some(raw.seq),
                )]
            }
        };
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

        let battle = &mut inst.battle_by_id_mut(&submit.battle_id).unwrap().battle;
        let result = battle.submit(
            &actor_cid,
            submit.action_id.clone(),
            submit.action,
            submit.target_ids.clone(),
            submit.skill_kind.clone(),
            submit.item_id.clone(),
        );
        match result {
            Ok(events) => self.emit_battle_events(&submit.battle_id, events),
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
        if inst.battle_of_player(player_id).is_some() {
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

    /// Load equipped-gear bonuses (per hero slot) for freshly-connected players.
    async fn flush_gear_loads(&mut self) {
        let loads: Vec<String> = std::mem::take(&mut self.pending_gear_load);
        let party_size = self.balance.battle.party_size_per_player as i32;
        for pid in loads {
            if let Ok(uid) = Uuid::parse_str(&pid) {
                if let Ok(bonuses) = self.db.equipped_gear_bonuses(uid, party_size).await {
                    if let Some(s) = self.sessions.get_mut(&pid) {
                        s.gear_bonuses = bonuses;
                    }
                }
            }
        }
    }

    /// Refresh one player's pending-backpack materials (withdrawn from the Vault
    /// storage chest) right before `run.enter_maze` is handled, so `form_run` can
    /// drain them synchronously into the fresh run's Backpack in the same call.
    /// Unlike `flush_gear_loads`, this can't be a queue-drained-next-tick load:
    /// the value is needed *this* call, not later at battle time, so it's fetched
    /// on demand for the one player about to dive.
    async fn flush_pending_materials(&mut self, pid: &str) {
        if let Ok(uid) = Uuid::parse_str(pid) {
            if let Ok(items) = self.db.get_pending_backpack(uid).await {
                if let Some(s) = self.sessions.get_mut(pid) {
                    s.pending_materials = items;
                }
            }
        }
    }

    /// Load persistent hero names + formation from Postgres for freshly-connected
    /// players.
    async fn flush_hero_loads(&mut self) {
        let loads: Vec<String> = std::mem::take(&mut self.pending_hero_load);
        for pid in loads {
            if let Ok(uid) = Uuid::parse_str(&pid) {
                if let Ok(names) = self.db.get_hero_names(uid).await {
                    if !names.is_empty() {
                        if let Some(s) = self.sessions.get_mut(&pid) {
                            s.hero_names = Some(names);
                        }
                    }
                }
                if let Ok(rows) = self.db.get_hero_rows(uid).await {
                    if !rows.is_empty() {
                        if let Some(s) = self.sessions.get_mut(&pid) {
                            s.hero_rows = Some(rows);
                        }
                    }
                }
                if let Ok(dived) = self.db.get_has_dived(uid).await {
                    if let Some(s) = self.sessions.get_mut(&pid) {
                        s.has_dived = dived;
                    }
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
            if inst.battle_of_player(player_id).is_some() {
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
        let _ = self
            .db_writes
            .send(DbWrite::SkillXp(player_id.to_string(), skill, xp));
        vec![out_msg(
            player_id,
            &wr::BackpackUpdate {
                changes: vec![wr::BackpackChange {
                    item,
                    delta: "added".to_string(),
                    cause: format!("harvest:{kind}"),
                }],
                chits_delta: 0,
                gear_added: Vec::new(),
            },
        )]
    }

    /// Open the treasure chest the avatar is standing next to: roll its loot
    /// (a richer chit payout than a kill, a biome material, and deep-enough red
    /// gear) into the backpack. The chest shows opened on the next snapshot.
    fn handle_open_chest(&mut self, player_id: &str, raw: RawEnvelope) -> Vec<Outgoing> {
        // A chest pays out like several kills' worth of chits (economy.md S2).
        const CHEST_RICHNESS: i32 = 4;
        let req: wr::OpenChest = match serde_json::from_value(raw.payload) {
            Ok(v) => v,
            Err(_) => {
                return vec![error(player_id, ErrorCode::ValidationError, "bad open_chest", Some(raw.seq))]
            }
        };
        let balance = self.balance.clone();
        let Some(inst) = self.instance.as_mut() else {
            return vec![error(player_id, ErrorCode::InvalidState, "Not in a run.", Some(raw.seq))];
        };
        if inst.battle_of_player(player_id).is_some() {
            return vec![error(player_id, ErrorCode::InvalidState, "Resolve the battle first.", Some(raw.seq))];
        }
        let Some((_tier, distance)) = inst.arena.open_chest(player_id, &req.entity_id) else {
            return vec![error(player_id, ErrorCode::OutOfRange, "No chest in reach.", Some(raw.seq))];
        };
        // Deterministic per (chest, player); the chest can only be opened once.
        let seed = inst.arena.seed ^ hash_str(&req.entity_id) ^ hash_str(player_id);
        let loot = meld_world::roll_creature_loot(&balance, distance, CHEST_RICHNESS, 1.0, seed);
        let loot_item = ItemStack {
            item_id: Uuid::now_v7().to_string(),
            item_kind: loot.material.to_string(),
            quantity: 1,
            insurance: None,
        };
        let gear: Vec<LootGear> = loot
            .gear
            .iter()
            .map(|g| LootGear {
                gear_id: Uuid::now_v7().to_string(),
                name: g.name.clone(),
                rarity: g.rarity.clone(),
                slot: g.slot.clone(),
                insurance: Insurance::Red,
                tier: g.tier,
                atk_bonus: g.atk_bonus,
                def_bonus: g.def_bonus,
                spd_bonus: g.spd_bonus,
                base_max_durability: g.max_durability,
                max_durability: g.max_durability,
                equipped_hero_slot: None,
            })
            .collect();
        let mut run_gear_snapshot = None;
        if let Some(r) = inst.run.run_mut(player_id) {
            r.backpack.push(loot_item.clone());
            r.chits += loot.chits;
            r.looted_gear.extend(gear.iter().cloned());
            if !gear.is_empty() {
                run_gear_snapshot = Some(r.looted_gear.clone());
            }
        }
        let mut out = vec![out_msg(
            player_id,
            &wr::BackpackUpdate {
                changes: vec![wr::BackpackChange {
                    item: loot_item,
                    delta: "added".to_string(),
                    cause: "chest".to_string(),
                }],
                chits_delta: loot.chits,
                gear_added: gear,
            },
        )];
        if let Some(gear) = run_gear_snapshot {
            out.push(out_msg(player_id, &wr::RunGear { gear }));
        }
        out
    }

    /// Complete any extraction channels whose timer elapsed: bank the backpack
    /// into the Vault (Postgres) and finalize the run as `extracted`.
    async fn complete_extractions(&mut self) -> Vec<Outgoing> {
        let now = now_ms();
        struct Banked {
            player_id: String,
            run_id: String,
            items: Vec<ItemStack>,
            chits: i64,
            gear: Vec<LootGear>,
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
                    let gear = std::mem::take(&mut r.looted_gear);
                    let chits = std::mem::replace(&mut r.chits, 0);
                    r.result = Some(RunResult::Extracted);
                    banks.push(Banked {
                        player_id: pid.clone(),
                        run_id: r.run_id.clone(),
                        items,
                        chits,
                        gear,
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
        // Players who extracted this pass — released from the instance after
        // banking so they can dive again from the hub (see `release_from_run`).
        let banked_pids: Vec<String> = banks.iter().map(|b| b.player_id.clone()).collect();
        for b in banks {
            let items_kv: Vec<(String, i32)> = b
                .items
                .iter()
                .map(|i| (i.item_kind.clone(), i.quantity))
                .collect();
            if let Ok(uid) = Uuid::parse_str(&b.player_id) {
                // Bank materials + chits atomically (economy.md S1 mint-on-extract).
                if let Err(e) = db.bank_extraction(uid, &items_kv, b.chits).await {
                    tracing::error!("bank_extraction failed for {}: {e}", b.player_id);
                }
                // Convert red-chest loot to owned Vault gear (stays `red`).
                let looted: Vec<meld_db::LootedGear> = b
                    .gear
                    .iter()
                    .filter_map(|g| {
                        Some(meld_db::LootedGear {
                            gear_id: Uuid::parse_str(&g.gear_id).ok()?,
                            name: g.name.clone(),
                            slot: g.slot.clone(),
                            tier: g.tier,
                            atk_bonus: g.atk_bonus,
                            def_bonus: g.def_bonus,
                            spd_bonus: g.spd_bonus,
                            base_max_durability: g.base_max_durability,
                            max_durability: g.max_durability,
                        })
                    })
                    .collect();
                if let Err(e) = db.insert_looted_gear(uid, &looted).await {
                    tracing::error!("insert_looted_gear failed for {}: {e}", b.player_id);
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
                let own = pid == &b.player_id;
                out.push(out_msg(
                    pid,
                    &wr::MemberResult {
                        run_id: b.run_id.clone(),
                        player_id: b.player_id.clone(),
                        result: RunResult::Extracted,
                        max_distance_reached: 0,
                        banked: own.then(|| b.items.clone()),
                        lost: None,
                        chits: if own { b.chits } else { 0 },
                        gear_banked: if own { b.gear.clone() } else { vec![] },
                        durability_loss_applied: false,
                    },
                ));
            }
            if !b.items.is_empty() || b.chits != 0 {
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
                        chits_delta: -b.chits,
                        gear_added: Vec::new(),
                    },
                ));
            }
        }
        for pid in &banked_pids {
            self.release_from_run(pid);
        }
        out
    }

    // --- tick ---------------------------------------------------------------

    fn tick(&mut self) -> Vec<Outgoing> {
        if self.instance.is_none() {
            return Vec::new();
        }
        let dt = (self.balance.battle.tick_ms.max(1) as f64) / 1000.0;
        let mut out = Vec::new();

        // 1) The overworld always advances — even while some party is in a battle.
        // Roaming creatures move and skirmish with rival factions; creatures pulled
        // into a battle are `in_battle` and hold still. Doing this every tick (not
        // only in the no-battle branch) is what keeps players who *aren't* fighting
        // live: without it, one player's fight froze the whole instance and starved
        // everyone else of snapshots until their sockets dropped (the co-op crash).
        // Iron Hull "Bulwark": per-player creature-aggro multipliers (≤1 shrinks how
        // close a creature will chase/skirmish-pull that party). Built before the
        // mut borrow below (perks_for needs a shared borrow of the instance).
        let aggro_mult: HashMap<String, f64> = {
            let ids: Vec<String> = self
                .instance
                .as_ref()
                .map(|inst| inst.run.runs.iter().map(|r| r.player_id.clone()).collect())
                .unwrap_or_default();
            ids.into_iter()
                .map(|pid| {
                    let m = self.perks_for(&pid).ironhull_aggro_mult as f64;
                    (pid, m)
                })
                .collect()
        };
        let mut created_sections: Vec<usize> = Vec::new();
        {
            // `balance` and `instance` are disjoint fields → both borrowable.
            let balance = &self.balance;
            if let Some(inst) = self.instance.as_mut() {
                inst.arena.step_creatures_with_aggro(dt, &aggro_mult);
                // Stream in new sections as the frontier player advances (endless world).
                let max_x = inst
                    .arena
                    .avatars
                    .iter()
                    .map(|a| a.position.x)
                    .fold(f64::NEG_INFINITY, f64::max);
                if max_x.is_finite() {
                    created_sections = inst.arena.ensure_frontier(balance, max_x);
                }
            }
        }
        // Stream the freshly-generated sections' terrain (+ trail segment) so the
        // client extends its relief and path — the endless-world payoff.
        if !created_sections.is_empty() {
            if let Some(inst) = self.instance.as_ref() {
                for &i in &created_sections {
                    let Some(area) = inst.arena.areas.get(i) else { continue };
                    let seg = if i + 1 < inst.arena.path.len() {
                        vec![inst.arena.path[i], inst.arena.path[i + 1]]
                    } else {
                        Vec::new()
                    };
                    let msg = terrain_section_msg(area, seg);
                    for r in &inst.run.runs {
                        out.push(out_msg(&r.player_id, &msg));
                    }
                }
            }
        }
        // Resonant "Overworld Regen": top up carried hero HP while walking (feeds
        // the next fight's starting HP). Server-authoritative; emits no messages.
        self.apply_overworld_regen(dt);

        // Ground loot dropped by creature-vs-creature kills, auto-collected by any
        // roaming player who walks over it.
        out.extend(self.collect_ground_loot());

        // 1b) Creatures moved this tick (step_creatures), so a creature may have
        // closed onto a stationary player. Start any contact battles now — otherwise
        // an aggressive creature could reach you and just sit there until you moved.
        out.extend(self.resolve_touches());

        // 2) Advance every active battle independently, for the parties fighting it.
        // Concurrent battles: separate groups fight different encounters at once, so
        // we tick each slot and emit its events scoped to its own members. A slot
        // that ends is removed inside `emit_battle_events`.
        let battle_ids: Vec<String> = self
            .instance
            .as_ref()
            .map(|i| i.battles.iter().map(|b| b.battle_id.clone()).collect())
            .unwrap_or_default();
        for id in battle_ids {
            let events = match self.instance.as_mut().and_then(|i| i.battle_by_id_mut(&id)) {
                Some(slot) => slot.battle.tick(),
                None => continue,
            };
            out.extend(self.emit_battle_events(&id, events));
            // Gauge keepalive (event-driven + periodic per battle.md) — only if the
            // battle is still running (didn't end on this tick).
            if let Some(inst) = self.instance.as_ref() {
                if let Some(slot) = inst.battle_by_id(&id) {
                    out.extend(self.gauge_update_msgs(inst, slot));
                }
            }
        }

        // 3) Snapshot the overworld to everyone NOT currently in a battle. This
        // runs every tick regardless of whether any battle is active, so roaming
        // teammates keep receiving world state while others fight.
        out.extend(self.snapshot_msgs());

        // 4) Reclaim slain creatures so `arena.monsters` stays bounded over a long
        // dive instead of accumulating a corpse per kill forever. Safe here: this is
        // after all battle-end processing (which refers to creatures by stable id,
        // not index) and after the snapshot (which already omits defeated creatures).
        if let Some(inst) = self.instance.as_mut() {
            inst.arena.prune_defeated();
        }
        out
    }

    /// Auto-collect ground loot (creature-skirmish drops) for every active player
    /// standing on it, banking each into the run backpack and reporting the change.
    fn collect_ground_loot(&mut self) -> Vec<Outgoing> {
        let mut out = Vec::new();
        let Some(inst) = self.instance.as_mut() else {
            return out;
        };
        let players: Vec<String> = inst.run.runs.iter().map(|r| r.player_id.clone()).collect();
        for pid in players {
            let drops = inst.arena.collect_loot(&pid);
            if drops.is_empty() {
                continue;
            }
            let mut changes = Vec::new();
            for d in drops {
                let item = ItemStack {
                    item_id: Uuid::now_v7().to_string(),
                    item_kind: d.kind.clone(),
                    quantity: 1,
                    insurance: None,
                };
                if let Some(r) = inst.run.run_mut(&pid) {
                    r.backpack.push(item.clone());
                }
                changes.push(wr::BackpackChange {
                    item,
                    delta: "added".to_string(),
                    cause: format!("pickup:{}", d.kind),
                });
            }
            out.push(out_msg(
                &pid,
                &wr::BackpackUpdate { changes, chits_delta: 0, gear_added: Vec::new() },
            ));
        }
        out
    }

    fn gauge_update_msgs(&self, inst: &ActiveInstance, slot: &BattleSlot) -> Vec<Outgoing> {
        // Borrow each fighter's cached wire-status list rather than cloning it, so
        // this per-tick, per-battle broadcast allocates nothing for statuses. These
        // borrowing structs serialize byte-identically to `wb::GaugeEntry` /
        // `wb::GaugeUpdate` (same field names + snake_case), so the wire is unchanged.
        #[derive(serde::Serialize)]
        struct GaugeEntryRef<'a> {
            combatant_id: &'a str,
            gauge: f64,
            hp: i32,
            statuses: &'a [String],
        }
        #[derive(serde::Serialize)]
        struct GaugeUpdateRef<'a> {
            battle_id: &'a str,
            server_tick: i64,
            combatants: Vec<GaugeEntryRef<'a>>,
        }
        let combatants: Vec<GaugeEntryRef> = slot
            .battle
            .gauge_views()
            .map(|(combatant_id, gauge, hp, statuses)| GaugeEntryRef {
                combatant_id,
                gauge,
                hp,
                statuses,
            })
            .collect();
        let msg = GaugeUpdateRef {
            battle_id: &slot.battle_id,
            server_tick: slot.battle.tick_count() as i64,
            combatants,
        };
        broadcast_ser(
            inst.run
                .runs
                .iter()
                .filter(|r| slot.parties.contains(&r.party_id))
                .map(|r| r.player_id.as_str()),
            wb::GaugeUpdate::TYPE,
            &msg,
        )
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
                level: Some(a.elevation),
                ..Default::default()
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
                level: Some(m.elevation),
                // Overworld mob intel (client shows each field only when the
                // viewer's Hunter/Psyker perk unlocks it — see `run.perks`).
                mob_level: Some(m.level),
                hp: Some(m.hp),
                max_hp: Some(m.max_hp),
                encounter_class: Some(m.encounter_class.clone()),
                aggression: Some(m.aggression.clone()),
            });
        }
        // The single deep extraction portal (extraction is otherwise the Town
        // Portal item). Tagged `portal` so the client renders it specially.
        entities.push(wm::SnapshotEntity {
            entity_id: "portal".to_string(),
            position: inst.arena.portal,
            velocity: wm::Velocity { x: 0.0, y: 0.0 },
            avatar_state: Some("portal".to_string()),
            level: Some(0),
            ..Default::default()
        });
        // Treasure chests, tagged `chest:<tier>:<open>` (`open` = 0/1) so the client
        // draws unopened vs opened. Opened chests stay in the world (as opened).
        for c in &inst.arena.chests {
            entities.push(wm::SnapshotEntity {
                entity_id: c.entity_id.clone(),
                position: c.position,
                velocity: wm::Velocity { x: 0.0, y: 0.0 },
                avatar_state: Some(format!("chest:{}:{}", c.tier, c.opened as u8)),
                level: Some(c.elevation),
                ..Default::default()
            });
        }
        // Un-harvested resource nodes, tagged `resource:<kind>` for the client.
        for n in inst.arena.resources.iter().filter(|n| !n.harvested) {
            entities.push(wm::SnapshotEntity {
                entity_id: n.entity_id.clone(),
                position: n.position,
                velocity: wm::Velocity { x: 0.0, y: 0.0 },
                avatar_state: Some(format!("resource:{}", n.kind)),
                level: Some(n.elevation),
                ..Default::default()
            });
        }
        // Ground loot dropped by creature-vs-creature skirmishes, tagged
        // `loot:<kind>` — walk over it to auto-collect (see `collect_ground_loot`).
        for l in &inst.arena.ground_loot {
            entities.push(wm::SnapshotEntity {
                entity_id: l.entity_id.clone(),
                position: l.position,
                velocity: wm::Velocity { x: 0.0, y: 0.0 },
                avatar_state: Some(format!("loot:{}", l.kind)),
                level: None,
                ..Default::default()
            });
        }
        // Impassable biome terrain, tagged `obstacle:<kind>:<radius>` so the client
        // renders each feature at its true size (static, but sent with the snapshot
        // like the other world entities — pragmatic for the slice).
        for o in &inst.arena.obstacles {
            entities.push(wm::SnapshotEntity {
                entity_id: o.entity_id.clone(),
                position: o.position,
                velocity: wm::Velocity { x: 0.0, y: 0.0 },
                avatar_state: Some(format!("obstacle:{}:{:.2}", o.kind, o.radius)),
                level: None,
                ..Default::default()
            });
        }
        let server_tick = now_ms() as i64;
        // Interest management (CANON §B networking): a player only receives entities
        // within the interest radius (`interest_radius_chunks × chunk_size` tiles) of
        // their own avatar — instead of the whole world every tick, which grew
        // unbounded as the endless world streamed in. This bounds each snapshot (and
        // its per-recipient serialization) to a rolling window around the player.
        // Purely a bandwidth/CPU cull: the server stays authoritative, so nothing
        // gameplay-affecting depends on what a client is sent. The recipient's own
        // avatar and the deep portal (a navigation landmark) are always included.
        let radius = (self.balance.world.interest_radius_chunks.max(0)
            * self.balance.world.chunk_size.max(1)) as f64;
        let radius2 = radius * radius;
        // Overworld snapshots go to players NOT in any battle. A fighting party is
        // on the battle screen and driven by battle messages instead; when no battle
        // is running, `in_battle` is empty so this sends to everyone.
        let in_battle = inst.parties_in_battle();
        let mut out = Vec::new();
        for r in inst
            .run
            .runs
            .iter()
            .filter(|r| !in_battle.contains(&r.party_id))
        {
            let me_pos = inst
                .arena
                .avatars
                .iter()
                .find(|a| a.player_id == r.player_id)
                .map(|a| a.position);
            // Psyker "Threat Sense": reveal mobs beyond the normal interest radius
            // (dangerous foes sensed at range). Non-mob entities keep the base radius.
            let mob_radius = (self.perks_for(&r.player_id).psyker_reveal_radius as f64).max(radius);
            let mob_radius2 = mob_radius * mob_radius;
            let culled: Vec<wm::SnapshotEntity> = match me_pos {
                Some(p) => entities
                    .iter()
                    .filter(|e| {
                        // Always keep the recipient's own avatar (the client centres
                        // its camera on it) and the portal landmark.
                        e.entity_id == r.player_id
                            || e.entity_id == "portal"
                            || {
                                let (dx, dy) = (e.position.x - p.x, e.position.y - p.y);
                                let d2 = dx * dx + dy * dy;
                                let is_mob = e
                                    .avatar_state
                                    .as_deref()
                                    .is_some_and(|s| s.starts_with("mob:"));
                                d2 <= if is_mob { mob_radius2 } else { radius2 }
                            }
                    })
                    .cloned()
                    .collect(),
                // Defensive: a roaming run should always have an avatar; if not, don't
                // cull (send the full set) rather than send an empty world.
                None => entities.clone(),
            };
            out.push(out_msg(
                &r.player_id,
                &wm::Snapshot {
                    server_tick,
                    entities: culled,
                },
            ));
        }
        out
    }

    /// Translate one battle's engine events into wire messages, handling its
    /// terminal outcome. `battle_id` scopes every message + member lookup to that
    /// battle (concurrent battles each get their own event stream).
    fn emit_battle_events(&mut self, battle_id: &str, events: Vec<BattleEvent>) -> Vec<Outgoing> {
        let mut out = Vec::new();
        for ev in events {
            match ev {
                BattleEvent::TurnReady { combatant_id } => {
                    let is_player = self
                        .instance
                        .as_ref()
                        .and_then(|i| i.battle_by_id(battle_id))
                        .map(|s| s.combatant_player.contains_key(&combatant_id))
                        .unwrap_or(false);
                    let timeout_at = if is_player {
                        Some(now_ms() + self.balance.battle.turn_timeout_ms)
                    } else {
                        None
                    };
                    let members = self.members_of_battle(battle_id);
                    out.extend(broadcast(
                        members.iter().map(String::as_str),
                        &wb::TurnReady {
                            battle_id: battle_id.to_string(),
                            combatant_id: combatant_id.clone(),
                            timeout_at,
                        },
                    ));
                }
                BattleEvent::Resolved(res) => {
                    let members = self.members_of_battle(battle_id);
                    let msg = wb::ActionResolved {
                        battle_id: battle_id.to_string(),
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
                    out.extend(broadcast(members.iter().map(String::as_str), &msg));
                }
                BattleEvent::Ended { outcome } => {
                    out.extend(self.handle_battle_end(battle_id, outcome));
                }
            }
        }
        out
    }

    /// The players (across every merged party) currently in a given battle.
    fn members_of_battle(&self, battle_id: &str) -> Vec<String> {
        match &self.instance {
            Some(inst) => match inst.battle_by_id(battle_id) {
                Some(slot) => inst.members_of(slot),
                None => Vec::new(),
            },
            None => Vec::new(),
        }
    }

    fn handle_battle_end(&mut self, battle_id: &str, outcome: BattleOutcome) -> Vec<Outgoing> {
        let mut out = Vec::new();
        let mut leveled: Vec<String> = Vec::new();
        // (player_id, old_run_level, new_run_level) for anyone who leveled up this
        // victory — drives the classic per-hero stat-gain screen.
        let mut level_ups: Vec<(String, i32, i32)> = Vec::new();
        let balance = self.balance.clone();
        let Some(inst) = self.instance.as_mut() else {
            return out;
        };
        let Some(bidx) = inst.battles.iter().position(|b| b.battle_id == battle_id) else {
            return out;
        };
        let monster_ids = inst.battles[bidx].monster_ids.clone();
        let battle_pos = inst.battles[bidx].pos;
        // Combined XP for the whole encounter (touched creature + its group).
        let xp_reward: i64 = monster_ids
            .iter()
            .filter_map(|id| inst.arena.monster_by_id(id))
            .map(|m| m.xp_reward)
            .sum();
        tracing::info!(battle_id = %battle_id, ?outcome, "battle ended");
        // The outcome applies to every party merged into THIS battle (raid).
        let bp = inst.battles[bidx].parties.clone();
        let members: Vec<String> = inst
            .run
            .runs
            .iter()
            .filter(|r| bp.contains(&r.party_id))
            .map(|r| r.player_id.clone())
            .collect();

        // Persist each participant's per-hero HP so wounds carry to the next
        // encounter (no free heal between fights). Read from the battle before its
        // slot is dropped below. (Disjoint field borrows: `battles` vs `hero_hp`.)
        for pid in &members {
            if let (Some(cids), Some(hps)) =
                (inst.battles[bidx].player_combatants.get(pid), inst.hero_hp.get_mut(pid))
            {
                for (slot, cid) in cids.iter().enumerate() {
                    if let (Some(hp), Some(slot_hp)) =
                        (inst.battles[bidx].battle.combatant_hp(cid), hps.get_mut(slot))
                    {
                        *slot_hp = hp;
                    }
                }
            }
        }

        let mut dead: Vec<String> = Vec::new();

        match outcome {
            BattleOutcome::Victory => {
                // The whole encounter is cleared from the overworld (prune_defeated
                // then reclaims these corpses at the end of the tick).
                for id in &monster_ids {
                    if let Some(m) = inst.arena.monster_by_id_mut(id) {
                        m.defeated = true;
                        m.in_battle = false;
                    }
                }
                // Award XP to every participant; note who leveled so we can refresh
                // their party panel (stats change on level-up).
                for r in inst.run.runs.iter_mut().filter(|r| bp.contains(&r.party_id)) {
                    let old_level = r.run_level;
                    if r.award_xp(xp_reward, &balance) > 0 {
                        leveled.push(r.player_id.clone());
                        level_ups.push((r.player_id.clone(), old_level, r.run_level));
                        // A level-up heals every one of the player's heroes to their
                        // new max HP (mid-run wounds otherwise persist between
                        // battles — see `hero_hp`'s doc comment — but a level gain
                        // always tops them up).
                        if let (Some(classes), Some(hps)) = (
                            inst.party_classes.get(&r.player_id),
                            inst.hero_hp.get_mut(&r.player_id),
                        ) {
                            for (class, hp) in classes.iter().zip(hps.iter_mut()) {
                                *hp = meld_run::max_hp_at_level(*class, r.run_level, &balance);
                            }
                        }
                    }
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
                // Loot each participant: the biome's combat material (banked to
                // craft), depth-scaled chits, and — deep enough — red-chest gear
                // (economy.md S1; meld_world::roll_creature_loot). Seeded per kill
                // like the Town Portal roll (instance ⊕ player ⊕ clock).
                let loot_distance = battle_pos.distance_floor();
                let monster_count = monster_ids.len() as i32;
                // FS-4: the reward spike — the fattest encounter class among the felled
                // creatures drives the loot multiplier (gatekeeper > elite > standard).
                let loot_mult = monster_ids
                    .iter()
                    .filter_map(|id| inst.arena.monster_by_id(id))
                    .map(|m| match m.encounter_class.as_str() {
                        "gatekeeper" => balance.encounters.gatekeeper_loot_mult,
                        "elite" => balance.encounters.elite_loot_mult,
                        _ => 1.0,
                    })
                    .fold(1.0_f64, f64::max);
                for (pid, run_level, _xp) in &runs_snapshot {
                    let loot = meld_world::roll_creature_loot(
                        &balance,
                        loot_distance,
                        monster_count,
                        loot_mult,
                        inst.arena.seed ^ hash_str(pid) ^ now_ms(),
                    );
                    let loot_item = ItemStack {
                        item_id: Uuid::now_v7().to_string(),
                        item_kind: loot.material.to_string(),
                        quantity: 1,
                        insurance: None,
                    };
                    // Any gear drop becomes a wire LootGear with a fresh server id
                    // (base == max durability at creation).
                    let gear_drops: Vec<LootGear> = loot
                        .gear
                        .iter()
                        .map(|g| LootGear {
                            gear_id: Uuid::now_v7().to_string(),
                            name: g.name.clone(),
                            rarity: g.rarity.clone(),
                            slot: g.slot.clone(),
                            insurance: Insurance::Red,
                            tier: g.tier,
                            atk_bonus: g.atk_bonus,
                            def_bonus: g.def_bonus,
                            spd_bonus: g.spd_bonus,
                            base_max_durability: g.max_durability,
                            max_durability: g.max_durability,
                            equipped_hero_slot: None,
                        })
                        .collect();
                    // Record loot in the run so extraction can bank it.
                    let mut run_gear_snapshot = None;
                    if let Some(r) = inst.run.runs.iter_mut().find(|r| &r.player_id == pid) {
                        r.backpack.push(loot_item.clone());
                        r.chits += loot.chits;
                        r.looted_gear.extend(gear_drops.iter().cloned());
                        if !gear_drops.is_empty() {
                            run_gear_snapshot = Some(r.looted_gear.clone());
                        }
                    }
                    let ended = wb::Ended {
                        battle_id: battle_id.to_string(),
                        outcome: BattleOutcome::Victory,
                        xp_awards: vec![wb::XpAward {
                            player_id: pid.clone(),
                            xp: xp_reward,
                            run_level_after: *run_level,
                        }],
                        loot: vec![loot_item.clone()],
                        chits_found: loot.chits,
                        gear_drops: gear_drops.clone(),
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
                            chits_delta: loot.chits,
                            gear_added: gear_drops,
                        },
                    ));
                    if let Some(gear) = run_gear_snapshot {
                        out.push(out_msg(pid, &wr::RunGear { gear }));
                    }
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
                                chits_delta: 0,
                                gear_added: Vec::new(),
                            },
                        ));
                    }
                }
            }
            BattleOutcome::Defeat => {
                for pid in &members {
                    out.push(out_msg(
                        pid,
                        &wb::Ended {
                            battle_id: battle_id.to_string(),
                            outcome: BattleOutcome::Defeat,
                            xp_awards: vec![],
                            loot: vec![],
                            chits_found: 0,
                            gear_drops: vec![],
                            class_emblem_drops: vec![],
                            gatekeeper_cleared: false,
                        },
                    ));
                }
                // Each participating player's run → died. The Backpack is deleted
                // with the run: its items, red-chest gear, and chits are all lost
                // (economy.md S1 — un-extracted chits never entered circulation).
                // Report the forfeited haul so the client can show what was lost.
                // (Durability sink runs off-loop via `run_db_writer` in Postgres.)
                let lost_hauls: Vec<(String, String, Vec<ItemStack>, i64)> = inst
                    .run
                    .runs
                    .iter()
                    .filter(|r| bp.contains(&r.party_id))
                    .map(|r| {
                        (
                            r.player_id.clone(),
                            r.run_id.clone(),
                            r.backpack.clone(),
                            r.chits,
                        )
                    })
                    .collect();
                for r in inst.run.runs.iter_mut().filter(|r| bp.contains(&r.party_id)) {
                    r.result = Some(RunResult::Died);
                    r.backpack.clear();
                    r.looted_gear.clear();
                    r.chits = 0;
                }
                for (pid, run_id, lost, lost_chits) in &lost_hauls {
                    out.push(out_msg(
                        pid,
                        &wr::MemberResult {
                            run_id: run_id.clone(),
                            player_id: pid.clone(),
                            result: RunResult::Died,
                            max_distance_reached: 0,
                            banked: None,
                            lost: Some(lost.clone()),
                            chits: *lost_chits,
                            gear_banked: vec![],
                            durability_loss_applied: true,
                        },
                    ));
                }
                dead = members.clone();
            }
            BattleOutcome::Fled => {}
        }
        // Battle over: any surviving grouped creatures (e.g. after a flee) resume
        // roaming, then drop the battle slot entirely (its combatant bookkeeping
        // goes with it). Other concurrent battles are untouched.
        for id in &monster_ids {
            if let Some(m) = inst.arena.monster_by_id_mut(id) {
                if !m.defeated {
                    m.in_battle = false;
                }
            }
        }
        inst.battles.retain(|b| b.battle_id != battle_id);
        for pid in dead {
            let _ = self.db_writes.send(DbWrite::Death(pid.clone()));
            // The run is over: release the player from the instance so they can
            // dive again from the hub (see `release_from_run`).
            self.release_from_run(&pid);
        }
        // Announce level-ups (classic stat-gain screen) then refresh the party
        // panel for anyone who leveled up (stats changed).
        for (pid, old_level, new_level) in &level_ups {
            let heroes = self.hero_level_ups(pid, *old_level, *new_level);
            out.push(out_msg(
                pid,
                &wr::LevelUp {
                    new_run_level: *new_level,
                    levels_gained: new_level - old_level,
                    heroes,
                },
            ));
        }
        for pid in &leveled {
            let heroes = self.party_views(pid);
            out.push(out_msg(pid, &wr::Party { heroes }));
            // Perk tiers scale with run level, so refresh them on level-up too.
            out.push(out_msg(pid, &self.perks_for(pid)));
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
