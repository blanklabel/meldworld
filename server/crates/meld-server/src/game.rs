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

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use meld_balance::Balance;
use meld_battle::{Battle, Event as BattleEvent, Reject};
use meld_db::Db;
use meld_proto::equipment::GearBonus;
use meld_proto::common::{Combatant, ItemStack, LootGear, Position};
use meld_proto::enums::*;
use meld_proto::realtime::{
    battle as wb, chat as wc, lobby as wl, movement as wm, onboarding as wo, run as wr,
    session as ws, world as ww, Message,
};
use meld_proto::RawEnvelope;
use meld_dungeon_content::{ObjectKind, Tile};
use meld_dungeon_run::{DungeonInstance, Location, TrapHit};
use meld_run::{build_battle, InstanceRun};
use meld_world::{Arena, Area};
use tokio::sync::mpsc;
use uuid::Uuid;

/// What one player forfeits when the party flees a fight: rolled per stack, so the
/// mutable borrow of the runs ends before the messages go out.
struct FleeLoss {
    pid: String,
    /// Everything spilled, both containers — the tally the `battle.ended` line shows.
    dropped: Vec<ItemStack>,
    /// Just the Party-Inventory half. The `run.backpack_update` removals must carry ONLY
    /// these: a pouch stack reported against the shared inventory would decrement a bag
    /// stack the client's mirror does hold, silently under-counting it.
    dropped_bag: Vec<ItemStack>,
    /// Whether any pouch spilled, so the pouches are re-sent only when they changed.
    pouches_changed: bool,
    lost_chits: i64,
    /// The gear still in the pack AFTER the drop roll (what the client should show).
    gear: Vec<LootGear>,
    /// Whether the roll actually took a piece, so we only persist when it did.
    gear_changed: bool,
}


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

/// A world-scoped tick/handler can't touch Router-owned state (sessions, world
/// teardown) directly — it emits these effects, which `GameState` applies. This is
/// what lets a world's logic move off `GameState` now and onto its own task later.
enum WorldEffect {
    /// A player left their run (death/extraction) — the Router flips the session's
    /// `in_instance` and tears the world down if it's now empty.
    ReleaseFromRun(String),
    /// A hero rename must also update the caller's session cache (used to form the
    /// NEXT dive) — Router-owned, so the world emits it.
    SetSessionHeroName {
        player_id: String,
        slot: usize,
        name: String,
    },
    /// CL-1: something happened that an unlock might be waiting on. The world
    /// reports the fact; the Router owns the session + DB and decides what it
    /// grants (the world has no business knowing what an account owns).
    Milestone {
        player_id: String,
        milestone: meld_proto::unlocks::Milestone,
    },
    /// MS-1: a smith job the world has accepted (who asked, where they stood, whose
    /// skill). The Router owns the heat and the Vault, so it takes it from here.
    SmithJob(Box<SmithJob>),
    /// AD-4: something happened that a posted hunt might be counting. Same split as
    /// `Milestone` — the world reports the fact, the Router owns the board.
    Hunt {
        player_id: String,
        fact: HuntFact,
    },
    /// AD-4: a bounty's mark is down. The world knows which creature it was; the Router
    /// owns the contract and the telling.
    BountyFelled {
        player_id: String,
        bounty_id: String,
        mark: String,
    },
    /// Same, for the front/back-row formation flag.
    SetSessionHeroRow {
        player_id: String,
        slot: usize,
        back: bool,
    },
}

/// What one unfinished hunt is looking for, as the snapshot asks it.
#[derive(Debug, Clone, PartialEq, Eq)]
enum QuarryTarget {
    Kind(String),
    Class(String),
}

impl QuarryTarget {
    fn matches(&self, kind: &str, class: &str) -> bool {
        match self {
            QuarryTarget::Kind(k) => k == kind,
            QuarryTarget::Class(c) => c == class,
        }
    }
}

/// Every quarry a player is still working, from the hunts their session holds.
///
/// The board lives on the Router and the snapshot is built by the world, so this is
/// pushed across the same way `skill_levels` is — the world never reads a session.
fn quarry_targets(board: &HashMap<String, (i32, bool)>) -> Vec<QuarryTarget> {
    let mut out = Vec::new();
    for def in meld_proto::hunts::HUNTS {
        let (progress, claimed) = board.get(def.key).copied().unwrap_or((0, false));
        // A finished hunt stops marking: you are done looking, and the thing left to do
        // is walk home and be paid.
        if claimed || progress >= def.goal.target() {
            continue;
        }
        match def.goal {
            meld_proto::hunts::HuntGoal::Fell { creature, .. } => {
                out.push(QuarryTarget::Kind(creature.to_string()))
            }
            meld_proto::hunts::HuntGoal::FellClass { class, .. } => {
                out.push(QuarryTarget::Class(class.to_string()))
            }
            _ => {}
        }
    }
    out
}

/// An owned [`meld_proto::hunts::HuntEvent`], for the trip from the world to the
/// Router.
#[derive(Debug, Clone)]
enum HuntFact {
    Felled { creature: String, class: String },
    Depth(i32),
    Extracted(i32),
    DungeonCleared,
}

impl HuntFact {
    fn as_event(&self) -> meld_proto::hunts::HuntEvent<'_> {
        use meld_proto::hunts::HuntEvent;
        match self {
            HuntFact::Felled { creature, class } => HuntEvent::Felled { creature, class },
            HuntFact::Depth(d) => HuntEvent::Depth { distance: *d },
            HuntFact::Extracted(d) => HuntEvent::Extracted { deepest: *d },
            HuntFact::DungeonCleared => HuntEvent::DungeonCleared,
        }
    }
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
    /// A player's run ended in a wipe: destroy the STANDARD gear they had equipped and
    /// burn their ephemeral. Deliberately NOT the durability sink any more — that is
    /// charged per hero death (`HeroFalls`, GR-2), and a wipe is already every hero
    /// falling. Doing both would bill the same deaths twice.
    Death(String),
    /// Charge the durability tax on one hero's own equipped insured gear:
    /// (player, hero slot, how many times that hero FELL). GR-2 / CANON D6.
    ///
    /// It rides per (hero, count) rather than as a whole-party vector because the two
    /// non-battle death sources — a dungeon trap and a Shift's Force blast — put
    /// individual heroes down without a battle to end.
    HeroFalls(String, i32, u32),
    /// Permanently delete a player's EQUIPPED red-insurance gear — the spec §5
    /// canon-gap resolution: Vault-owned red gear brought back into a run is
    /// at absolute risk, burned when the run ends `died` OR `abandoned`.
    /// The run ended and this player is back in the city — burn their ephemeral gear.
    /// Sent on EVERY way home, extraction included.
    BurnEphemeral(String),
    /// Credit harvested Meld-skill XP: (player, skill, xp).
    SkillXp(String, String, i64),
    /// Persist a hero rename: (player, slot, name).
    HeroRename(String, i16, String),
    /// Persist a hero's formation rank: (player, slot, back_row).
    HeroFormation(String, i16, bool),
    /// Record a class's best level ever reached: (player, class_key, level). XP is
    /// dive-scoped; this is the achievement that survives it.
    ClassBest(String, String, i32),
    /// Persist a hero slot's class (GR-7): (player, slot, class_key). Written when
    /// a party dives, so the roster a player takes down becomes their roster in
    /// town — which is what equip-time legality checks against.
    HeroClass(String, i16, String),
    /// Mark that a player has begun their first dive (ends the tutorial world).
    Dived(String),
    /// Mark that a player has dismissed the town welcome tour (finished or skipped).
    TutorialTownSeen(String),
    /// Mark that a player has dismissed the first-dive briefing.
    TutorialRunSeen(String),
    /// Credit progress toward one posted hunt: (player, hunt key, delta, target).
    /// The session already decided this is worth writing, so the DB call is a store
    /// rather than a second ruling.
    HuntProgress(String, String, i32, i32),
    /// A bounty's mark was felled: (player, bounty id). The reward is still taken at the
    /// board — this only records that the contract is finished.
    BountyFelled(String, String),
    /// Post a new deepest distance to the Vanguard Board: (player, distance).
    /// Sent only when the run's record actually grows, so the board write rate is
    /// bounded by *progress*, not by movement (P1-1).
    Vanguard(String, wr::VanguardStamp),
    /// THE END FIGHT is down: the same posting, plus the wood star and the clear time.
    WorldEnd(String, wr::VanguardStamp, i64),
    /// Clear a player's pending-backpack queue: its contents were just drained
    /// into a freshly-formed run's live Backpack.
    ClearPendingBackpack(String),
    /// Persist earned unlocks: (player, keys). Fire-and-forget — the session's
    /// in-memory set is what gates THIS dive, so a slow write costs nothing.
    Unlocks(String, Vec<String>),
    /// Hibernate a world's delta (CANON §W5). Goes down this channel like everything
    /// else: the 100 ms tick must never wait on Postgres, and a save that lands a second
    /// late costs at most a second of a world nobody is standing in.
    SaveWorld(Box<meld_db::WorldSave>),
}

/// Drain the DB-write queue on its own task, serializing writes off the hot path.
async fn run_db_writer(db: Db, balance: Arc<Balance>, mut rx: mpsc::UnboundedReceiver<DbWrite>) {
    let per_fall = balance.loot.durability_loss_per_fall;
    while let Some(job) = rx.recv().await {
        match job {
            DbWrite::HeroFalls(pid, slot, falls) => {
                if let Ok(uid) = Uuid::parse_str(&pid) {
                    if let Err(e) = db.apply_hero_fall_durability(uid, slot, falls, per_fall).await {
                        tracing::error!("fall durability failed for {pid} hero {slot}: {e}");
                    }
                }
            }
            DbWrite::Death(pid) => {
                if let Ok(uid) = Uuid::parse_str(&pid) {
                    // A wipe takes the two tiers a death can TAKE: standard outright,
                    // ephemeral like it would have burned on any other way home. What
                    // insured gear pays is durability, and it has already been charged
                    // per hero fall (`HeroFalls`) — including the falls that made this
                    // wipe a wipe.
                    if let Err(e) = db.destroy_equipped_standard_gear(uid).await {
                        tracing::error!("standard-gear loss failed for {pid}: {e}");
                    }
                    if let Err(e) = db.burn_ephemeral_gear(uid).await {
                        tracing::error!("ephemeral burn failed for {pid}: {e}");
                    }
                }
            }
            DbWrite::BurnEphemeral(pid) => {
                if let Ok(uid) = Uuid::parse_str(&pid) {
                    if let Err(e) = db.burn_ephemeral_gear(uid).await {
                        tracing::error!("ephemeral burn failed for {pid}: {e}");
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
            DbWrite::ClassBest(pid, class_key, level) => {
                if let Ok(uid) = Uuid::parse_str(&pid) {
                    if let Err(e) = db.record_class_best(uid, &class_key, level).await {
                        tracing::error!("class best persist failed for {pid}: {e}");
                    }
                }
            }
            DbWrite::HeroClass(pid, slot, class_key) => {
                if let Ok(uid) = Uuid::parse_str(&pid) {
                    if let Err(e) = db.set_hero_class(uid, slot, &class_key).await {
                        tracing::error!("hero class persist failed for {pid}: {e}");
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
            DbWrite::TutorialTownSeen(pid) => {
                if let Ok(uid) = Uuid::parse_str(&pid) {
                    if let Err(e) = db.set_tutorial_town_seen(uid).await {
                        tracing::error!("tutorial-town-seen persist failed for {pid}: {e}");
                    }
                }
            }
            DbWrite::TutorialRunSeen(pid) => {
                if let Ok(uid) = Uuid::parse_str(&pid) {
                    if let Err(e) = db.set_tutorial_run_seen(uid).await {
                        tracing::error!("tutorial-run-seen persist failed for {pid}: {e}");
                    }
                }
            }
            DbWrite::Vanguard(pid, stamp) => {
                if let Ok(uid) = Uuid::parse_str(&pid) {
                    let season = meld_db::current_season();
                    if let Err(e) = db
                        .record_vanguard_distance(
                            uid,
                            season,
                            stamp.distance,
                            stamp.level,
                            stamp.fights,
                            stamp.flees,
                        )
                        .await
                    {
                        tracing::error!("vanguard post failed for {pid}: {e}");
                    }
                }
            }
            DbWrite::WorldEnd(pid, stamp, clear_ms) => {
                if let Ok(uid) = Uuid::parse_str(&pid) {
                    let season = meld_db::current_season();
                    if let Err(e) = db.record_world_end(uid, season, &stamp, clear_ms).await {
                        tracing::error!("world-end post failed for {pid}: {e}");
                    }
                }
            }
            DbWrite::HuntProgress(pid, key, delta, target) => {
                if let Ok(uid) = Uuid::parse_str(&pid) {
                    if let Err(e) = db.credit_hunt(uid, &key, delta, target).await {
                        tracing::error!("hunt credit failed for {pid}: {e}");
                    }
                }
            }
            DbWrite::BountyFelled(pid, bounty_id) => {
                if let (Ok(uid), Ok(bid)) = (Uuid::parse_str(&pid), Uuid::parse_str(&bounty_id)) {
                    if let Err(e) = db.complete_bounty(uid, bid).await {
                        tracing::error!("bounty completion failed for {pid}: {e}");
                    }
                }
            }
            DbWrite::SaveWorld(w) => {
                if let Err(e) = db.save_world(&w).await {
                    tracing::error!("world save failed: {e}");
                }
            }
            DbWrite::Unlocks(pid, keys) => {
                if let Ok(uid) = Uuid::parse_str(&pid) {
                    if let Err(e) = db.grant_unlocks(uid, &keys).await {
                        tracing::error!("unlock grant failed for {pid}: {e}");
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
    tokio::spawn(run_db_writer(db.clone(), balance.clone(), db_rx));
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
/// baseline for its own stat lane (main_hand→atk, protective pieces→def,
/// accessory→spd) rather than stacking, mirroring the per-category capacity
/// the Vault already enforces. `hero_slot` is the party slot index (0-based).
/// A smith's temporary EDGE on a hero's kit (MS-1 `enhance`). Run-scoped: it lives in
/// the world and dies with the dive, which is what keeps a temporary buff from becoming
/// a way to launder power home.
#[derive(Debug, Clone, Copy, Default)]
struct Edge {
    atk: i32,
    def: i32,
    spd: i32,
    /// HP restored at the start of the holder's turn — the still's line, since a tonic
    /// is not an edge on a blade.
    regen: i32,
}

fn effective_gear_bonus(
    vault: GearBonus,
    looted: &[LootGear],
    hero_slot: i32,
    edge: Option<&Edge>,
) -> GearBonus {
    // One type, so this is a move and not a field-by-field copy. It WAS a copy, across two
    // identical declarations, and a field added to one side compiled fine on the other while
    // the copy silently dropped it — gear that rolled in the Vault reaching nothing in the
    // fight. Anything that needs adding now has one place to be added.
    let mut bonus = vault;
    for g in looted {
        if g.equipped_hero_slot != Some(hero_slot) {
            continue;
        }
        // Armour looted THIS run answers for damage types too — the piece is being worn,
        // so leaving its weight out would mean run-scoped plate protected nothing.
        if !g.armor_weight.is_empty() {
            bonus.armor_weights.push(g.armor_weight.clone());
        }
        match g.slot.as_str() {
            "main_hand" => bonus.atk = g.atk_bonus,
            "off_hand" | "head" | "chest" | "legs" => bonus.def = g.def_bonus,
            "accessory" => bonus.spd = g.spd_bonus,
            _ => {}
        }
    }
    // The edge goes on LAST, so it sharpens whatever the hero actually ended up
    // wearing rather than being overwritten by a piece of run loot.
    if let Some(e) = edge {
        bonus.atk += e.atk;
        bonus.def += e.def;
        bonus.spd += e.spd;
        bonus.regen += e.regen;
    }
    bonus
}

/// Serialize a drop's elemental entries into the gear table's JSON column
/// format ({"FIRE":0.75}); empty entries become "{}".
fn modifiers_json(entries: &[(String, f64)]) -> String {
    if entries.is_empty() {
        return "{}".to_string();
    }
    let map: serde_json::Map<String, serde_json::Value> = entries
        .iter()
        .filter_map(|(k, v)| serde_json::Number::from_f64(*v).map(|n| (k.clone(), n.into())))
        .collect();
    serde_json::Value::Object(map).to_string()
}

/// The per-hero class composition of a player's party of `size`. The picked class
/// leads; the rest are a fixed spread so a single party mixes classes that play
/// very differently (Explorer bruiser + Psyker channeler + Resonant healer).
fn party_composition(chosen: CharacterClass, size: usize) -> Vec<CharacterClass> {
    let base = [
        chosen,
        CharacterClass::Psyker,
        CharacterClass::Resonant,
        CharacterClass::Explorer,
    ];
    (0..size.max(1)).map(|i| base[i % base.len()]).collect()
}


/// Lift one FAMILY of afflictions from a hero's carried set, returning what went.
///
/// A free function so the borrow is just the one map entry: `handle_use_item` is holding
/// several disjoint pieces of the world by the time it gets here.
fn cure_carried(
    carried: Option<&mut Vec<Vec<String>>>,
    slot: usize,
    family: meld_proto::statuses::Family,
) -> Vec<String> {
    let Some(all) = carried else {
        return Vec::new();
    };
    let Some(mine) = all.get_mut(slot) else {
        return Vec::new();
    };
    let lifted: Vec<String> = mine
        .iter()
        .filter(|n| meld_proto::statuses::cures(family, n))
        .cloned()
        .collect();
    mine.retain(|n| !meld_proto::statuses::cures(family, n));
    lifted
}

/// `MELD_POTIONS` — DEV/QA: how many of each starting potion a dive is dealt, so the apex
/// can be measured against a party that stocked up rather than an empty pouch.
fn dev_potions() -> Option<i32> {
    std::env::var("MELD_POTIONS").ok().and_then(|v| v.trim().parse::<i32>().ok()).filter(|n| *n > 0)
}

/// `MELD_TOWN_PORTALS` — DEV/QA, the same idea for the way home. Kit is BOUGHT now
/// (`[runs] starting_town_portals` is 0), so a harness that wants to exercise the portal
/// path has to be handed one rather than walking the shop first — the same reason
/// `MELD_POTIONS` exists.
fn dev_town_portals() -> Option<i32> {
    std::env::var("MELD_TOWN_PORTALS")
        .ok()
        .and_then(|v| v.trim().parse::<i32>().ok())
        .filter(|n| *n > 0)
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
    gear_bonuses: Vec<GearBonus>,
    /// The caller's persistent Forging level, loaded with their gear. The city anvil's
    /// heat is laid out against it, so a master's bar in town is as wide as in the field.
    forging_level: Option<i32>,
    /// Class chosen at the player's most recent `run.enter_maze` (default Explorer).
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
    /// Whether this account has dismissed the town welcome tour (finished or
    /// skipped it) and the first-dive briefing. Loaded from the DB alongside
    /// `has_dived`; sent to the client as `onboarding.status` once loaded (never
    /// on the immediate `Connected` message — that fires before this load can
    /// possibly have landed).
    tutorial_town_seen: bool,
    tutorial_run_seen: bool,
    /// PG-2: the deepest distance this account has EVER reached, all-time. The bar every
    /// departure hub is gated on — loaded on connect from the `vanguard` record, which is
    /// written off validated movement and cannot be client-submitted.
    deepest_ever: i32,
    /// Account-permanent unlocks (roadmap CL-1) — which party slots and classes
    /// this account has earned. Loaded on connect; `None` until then, and a party
    /// built before the load lands is NOT gated (the load beats the first
    /// `enter_maze` in practice, and failing open beats locking a player out).
    unlocks: Option<Vec<String>>,
    /// Materials withdrawn from the Vault (storage chest), refreshed right
    /// before `run.enter_maze` is handled so `form_run` can drain them
    /// synchronously into the fresh run's Backpack (see `flush_pending_materials`).
    pending_materials: Vec<(String, i32)>,
    /// AD-4: progress and claimed-state per posted hunt, loaded on connect. `None`
    /// until it lands, and a hunt credited before then is simply not counted — the
    /// alternative is an in-memory zero racing the load and announcing "1/8" on a
    /// hunt the account had already finished.
    hunts: Option<HashMap<String, (i32, bool)>>,
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

/// Clamp a requested party to what an account owns: at most `party_slots` heroes,
/// and every class one the player has earned. An unowned class becomes an Explorer
/// rather than an error — the dive still happens.
fn clamp_party_to_unlocks(
    party: Vec<CharacterClass>,
    owned: &[String],
) -> Vec<CharacterClass> {
    let slots = meld_proto::unlocks::party_slots(owned) as usize;
    let fieldable = meld_proto::unlocks::owned_classes(owned);
    party
        .into_iter()
        .take(slots.max(1))
        .map(|c| {
            if fieldable.contains(&c) {
                c
            } else {
                CharacterClass::Explorer
            }
        })
        .collect()
}

/// The party-slot bars a run currently clears: for each distinct `HeroesAtLevel`
/// rule in the registry, the count of heroes simultaneously at-or-above it. Read
/// from the registry rather than hardcoded, so adding a fifth slot needs no server
/// change.
fn party_slot_bars(run: &meld_run::PlayerRun) -> Vec<(i32, i32)> {
    let mut bars = Vec::new();
    for u in meld_proto::unlocks::UNLOCKS {
        if let meld_proto::unlocks::Trigger::HeroesAtLevel { level, .. } = u.trigger {
            let heroes = run.heroes_at_level(level) as i32;
            if heroes > 0 && !bars.iter().any(|(h, l)| *h == heroes && *l == level) {
                bars.push((heroes, level));
            }
        }
    }
    bars
}

/// Build the `run.unlocked` message: `newly` is what to announce (empty for the
/// connect-time inventory), `owned` is everything the account has after the grant.
fn unlock_inventory(
    owned: &[String],
    newly: &[&'static meld_proto::unlocks::UnlockDef],
    banner: bool,
    deepest_ever: i32,
) -> wr::Unlocked {
    wr::Unlocked {
        unlocks: newly.iter().map(|d| meld_proto::unlocks::view(d, owned)).collect(),
        owned: owned.to_vec(),
        party_slots: meld_proto::unlocks::party_slots(owned),
        banner,
        deepest_ever,
    }
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

/// Chunk coordinate of a world position for the interest grid (SC-1). Cell size is
/// the balance `world.chunk_size` — the same bucketing `Arena::step_creatures` uses
/// for its skirmish spatial hash.
fn chunk_key(x: f64, y: f64, cell: f64) -> (i32, i32) {
    ((x / cell).floor() as i32, (y / cell).floor() as i32)
}

/// Build the per-tick interest grid over `entities`: chunk coord → indices into
/// `entities`. Built once per snapshot so each player's interest query touches only
/// the cells around them instead of re-scanning the whole (endless-world) entity
/// list — turning the per-player cull from O(entities) into O(visible).
fn build_interest_grid(entities: &[wm::SnapshotEntity], cell: f64) -> HashMap<(i32, i32), Vec<usize>> {
    let mut grid: HashMap<(i32, i32), Vec<usize>> = HashMap::new();
    for (i, e) in entities.iter().enumerate() {
        grid.entry(chunk_key(e.position.x, e.position.y, cell))
            .or_default()
            .push(i);
    }
    grid
}

/// Indices of the entities visible to a viewer at (`px`, `py`), via the interest
/// grid. Behaviour-identical to a full linear distance filter (proven in the tests
/// below) but O(cells-in-range) instead of O(entities): mobs use `mob_radius` (the
/// Psyker reveal radius, always ≥ `radius`), everything else uses the base `radius`;
/// the viewer's own avatar (`own_idx`) and the portal landmark (`portal_idx`) are
/// always included. Returned sorted + deduped so the emitted snapshot preserves the
/// entities' original push order.
///
/// Why the box query is exact: any entity that passes the precise per-type test lies
/// within `mob_radius` of the viewer (`mob_radius ≥ radius`), so its cell is inside
/// the queried box — the grid can never drop a visible entity.
#[allow(clippy::too_many_arguments)]
fn interest_visible_indices(
    entities: &[wm::SnapshotEntity],
    grid: &HashMap<(i32, i32), Vec<usize>>,
    cell: f64,
    px: f64,
    py: f64,
    radius2: f64,
    mob_radius: f64,
    mob_radius2: f64,
    own_idx: Option<usize>,
    portal_idx: Option<usize>,
) -> Vec<usize> {
    let (min_cx, min_cy) = chunk_key(px - mob_radius, py - mob_radius, cell);
    let (max_cx, max_cy) = chunk_key(px + mob_radius, py + mob_radius, cell);
    let mut idxs: Vec<usize> = Vec::new();
    for cx in min_cx..=max_cx {
        for cy in min_cy..=max_cy {
            let Some(bucket) = grid.get(&(cx, cy)) else {
                continue;
            };
            for &i in bucket {
                let e = &entities[i];
                let (dx, dy) = (e.position.x - px, e.position.y - py);
                let d2 = dx * dx + dy * dy;
                let is_mob = e
                    .avatar_state
                    .as_deref()
                    .is_some_and(|s| s.starts_with("mob:"));
                if d2 <= if is_mob { mob_radius2 } else { radius2 } {
                    idxs.push(i);
                }
            }
        }
    }
    if let Some(i) = own_idx {
        idxs.push(i);
    }
    if let Some(i) = portal_idx {
        idxs.push(i);
    }
    idxs.sort_unstable();
    idxs.dedup();
    idxs
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
fn terrain_section_msg(
    area: &Area,
    path: Vec<Position>,
    radial_half: f64,
    corridor_lateral: f64,
    peaks: Vec<[f32; 4]>,
) -> ww::TerrainSection {
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
        biome: area.biome.to_string(),
        radial_half,
        corridor_lateral,
        peaks,
    }
}

/// Tag each ally combatant with its hero's persistent name (`name:<name>`) so the
/// client shows names in battle. Slot = the combatant's index among its player's
/// combatants; the name comes from `WorldActor::hero_names`.
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

/// An in-progress **harvest** channel (MS-2). Unlike extraction this is *repeating*:
/// it hands over one unit every `tick_ms` until the node is empty or something stops
/// it. Interruption is strict — the tick in flight is lost — but every unit already
/// banked stays banked, so a broken gather costs one tick rather than the node. That
/// is what turns "do I dare start" into "how long do I dare stay".
struct Harvest {
    node_id: String,
    /// Wall-clock ms at which the next unit comes loose.
    next_at: u64,
    tick_ms: u64,
}

/// Raising or packing up a bench (MS-1). A station is a commitment you make in a
/// dangerous place, so both ends of its life are channels: interruptible, and visible to
/// the whole instance while you are crouched over it.
struct Building {
    completes_at: u64,
    /// `smith` / `alembic` — which bench is going up.
    kind: String,
    /// Set when packing UP: the station being dismantled.
    tearing_down: Option<String>,
    /// The stock already taken for it, which the raised bench then remembers so a
    /// teardown hands back the same material rather than something guessed at.
    stock: String,
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
    /// Overworld creature `entity_id` -> its combatant id in THIS battle (`CR-2`). The
    /// bridge the wound write-back needs: `monster_ids` says which creatures are in the
    /// fight and the engine reports HP by combatant, and without a mapping between them a
    /// creature that survived (a flee) walked away with every point of damage forgotten.
    /// Empty for a dungeon boss, which is built in the battle rather than standing in the
    /// arena, so there is nothing to write back to.
    monster_combatants: HashMap<String, String>,
    /// combatant_id -> player_id, for the players in THIS battle only.
    combatant_player: HashMap<String, String>,
    /// player_id -> the combatant ids they control in THIS battle.
    player_combatants: HashMap<String, Vec<String>>,
    /// Party ids merged into this battle (raid merge).
    parties: std::collections::HashSet<u32>,
    /// Players WATCHING this fight without being in it (`SOC-3`). They receive every
    /// message a participant does — that is the whole feature — but they own no
    /// combatant, so `handle_submit` refuses them and the engine never asks them for
    /// a turn. Kept on the slot rather than beside it so a battle that ends takes its
    /// watchers with it and cannot leak a feed into the next fight.
    spectators: std::collections::HashSet<String>,
    /// Overworld position of this fight (the touched creature's spot), so a nearby
    /// teammate can opt in via `run.join_battle`.
    pos: Position,
    /// `Some` for a DG-3b dungeon boss fight — the dungeon key + the boss object id.
    /// Drives the post-battle fixups (victory ⇒ `boss_dead`, defeat ⇒ dungeon
    /// cleanup) in `finish_dungeon_battle`. `None` for every overworld battle.
    dungeon: Option<DungeonBattle>,
    /// The `encounter_party_scale` these creatures were actually built with, so the
    /// XP can be paid against the health the party really had to chew through.
    ///
    /// Without it the two halves of the party rule disagreed: a four-hero party met
    /// creatures with 4.4x the HP and then SPLIT the unscaled XP four ways, so each
    /// hero earned at 0.057x the solo rate per point of health destroyed. The split
    /// alone is the intended cost (a lone hero absorbs the whole lesson); the 4.4x
    /// on top of it was not.
    ///
    /// Captured at BUILD time on purpose. A co-op joiner does not re-scale the
    /// creatures — the mob stays crushable — so joining must not inflate the payout
    /// either: more heroes splitting the same XP is exactly the pressure that sends
    /// a full co-op group looking for a much harder fight.
    party_scale: f64,
}

/// Dungeon context carried by a boss-fight [`BattleSlot`] (DG-3b).
#[derive(Clone)]
struct DungeonBattle {
    key: u64,
    boss_id: String,
    /// The bounty this door's boss IS, or empty (AD-4). A dungeon boss is built here
    /// rather than placed in the arena, so the contract rides the battle instead of a
    /// `MonsterSpawn`.
    bounty: String,
    /// Which named boss the mark fights as, for the telling.
    mark_boss: String,
}

/// What a watching session is pointed at (`SOC-3`). Two sources, one client feed:
/// both arrive as a `battle.started` the viewer controls nothing in.
#[derive(Clone, PartialEq)]
enum WatchFeed {
    /// Another player's battle, by id. Its `BattleSlot` holds the watcher too, so every
    /// message the fight already broadcasts reaches them through the one audience funnel
    /// — no per-message plumbing, and nothing to forget when a new event type lands.
    Battle(String),
    /// A creature-vs-creature clash (`CR-2`), anchored on ONE creature rather than on a
    /// clash identity. A clash gains and loses bodies as blows land, so an id for it
    /// would go stale under the watcher every few seconds; a body is either still
    /// swinging or the fight it was in is over, which is exactly the question the feed
    /// needs answered each tick.
    Clash {
        anchor: String,
        /// Who was in the feed when it was last sent, so the roster is only re-sent when
        /// it actually changed (a `battle.started` every tick would reset the client's
        /// battle screen 10 times a second).
        roster: Vec<String>,
    },
}

impl WatchFeed {
    /// The `battle_id` this feed rides on. A clash borrows its anchor's id under a
    /// `clash:` prefix so it can never collide with a real battle id — and so
    /// `handle_submit` refuses an action against it with "Unknown battle" rather than
    /// finding something to aim at.
    fn battle_id(&self) -> String {
        match self {
            WatchFeed::Battle(id) => id.clone(),
            WatchFeed::Clash { anchor, .. } => format!("clash:{anchor}"),
        }
    }
}

/// One world's authoritative state — the nucleus that SC-3 will own on its own
/// task (a `WorldActor`). Today the server runs exactly one, owned inline by
/// `GameState`; the tick and world-pure logic are being migrated onto it so the
/// eventual per-world task split is a change of transport, not a rewrite.
struct WorldActor {
    balance: Arc<Balance>,
    /// Fire-and-forget persistence sink (a clone of `GameState`'s), so world-owned
    /// lifecycle logic (deaths, etc.) can enqueue writes without touching the Router.
    db_writes: mpsc::UnboundedSender<DbWrite>,
    arena: Arena,
    run: InstanceRun,
    /// Every battle currently running in the instance. Independent parties fight
    /// separate encounters at the same time; each is one [`BattleSlot`].
    battles: Vec<BattleSlot>,
    /// player_id -> per-hero current HP (length = party_size_per_player), carried
    /// across the run's battles so wounds persist (no free heal between fights).
    /// Reset to full only when a player (re)enters the maze — a fresh dive.
    hero_hp: HashMap<String, Vec<i32>>,
    /// player_id -> per-hero AFFLICTIONS carried between encounters, parallel to `hero_hp`.
    ///
    /// Afflictions do not expire, so a poison has to outlive the fight that inflicted it — and
    /// a `Fighter` is rebuilt every battle, so the run is what remembers. This is what makes a
    /// condition something you carry down the road rather than something the next loading
    /// screen washes off.
    hero_afflictions: HashMap<String, Vec<Vec<String>>>,
    /// Players whose insured gear this run has actually charged (GR-2). The tax rides
    /// hero FALLS rather than the run's outcome, so "did this run cost durability" is
    /// no longer a synonym for "did it end in death" — a hero can go down and the party
    /// extract — and `run.member_result.durability_loss_applied` has to be told.
    durability_charged: HashSet<String>,
    /// Movement inputs since a poisoned party last took its bite — venom is charged by
    /// DISTANCE, not by time.
    venom_steps: HashMap<String, i32>,
    /// player_id -> per-hero class (the mixed party composition), parallel to
    /// `hero_hp`. Each slot's class drives its stats/kit for the whole run.
    party_classes: HashMap<String, Vec<CharacterClass>>,
    /// player_id -> per-hero equipped Vault gear bonuses — a world-local mirror of
    /// each member's `Session.gear_bonuses`, so world-owned combat/roster logic
    /// reads gear WITHOUT reaching into the Router's sessions. Seeded in `form_run`
    /// and kept in sync wherever `Session.gear_bonuses` is written (`flush_gear_loads`).
    /// Gear only changes at connect/form_run/flush and `flush_gear_loads` runs after
    /// `tick`, so within any tick this equals a live session read (behaviour-identical).
    gear_bonuses: HashMap<String, Vec<GearBonus>>,
    /// player_id -> per-hero display name (parallel to `party_classes`).
    hero_names: HashMap<String, Vec<String>>,
    /// player_id -> per-hero formation flag (`true` = back row), parallel to
    /// `party_classes`. Empty = fall back to each class's default row.
    hero_rows: HashMap<String, Vec<bool>>,
    /// player_id -> active extraction channel.
    extraction: HashMap<String, Extraction>,
    /// player -> in-progress harvest channel (MS-2).
    harvest: HashMap<String, Harvest>,
    /// player -> in-progress station setup/teardown channel (MS-1).
    building: HashMap<String, Building>,
    /// player_id -> fractional HP carried over between ticks for the Resonant
    /// "Overworld Regen" perk (regen is HP/sec but `hero_hp` is integer, so we
    /// bank the sub-1 remainder here and apply whole HP as it accrues).
    regen_accum: HashMap<String, RegenAccum>,
    /// When each player's Psyker last pinned a creature (ms), for the cooldown.
    hold_last_ms: HashMap<String, u64>,
    /// player_id -> the fight they are WATCHING (`SOC-3`). One entry per player: you
    /// cannot watch two fights at once, and a second `run.watch_battle` re-aims the
    /// same feed rather than opening another.
    watching: HashMap<String, WatchFeed>,
    /// DG-3: hand-designed dungeon entrances placed as the world streams (a chanced
    /// per-section draw from the biome's authored pool). Rendered in the overworld
    /// snapshot as `entrance:<dungeon>`; touch one to descend (`enter_dungeon`).
    entrances: Vec<DungeonEntrance>,
    /// Whether this world is the gentle first-dive tutorial — chanced entrances
    /// are suppressed there to keep onboarding clean, except for the one
    /// hand-placed entrance below (see `tutorial_entrance_placed`).
    tutorial: bool,
    /// DG-3-tutorial: place-once guard for the single forced, hand-placed
    /// dungeon entrance a tutorial run gets beyond area 0, so the guided
    /// walkthrough's "how to enter a dungeon" step has something to point at.
    /// Never set on a normal run.
    tutorial_entrance_placed: bool,
    /// DG-3b: each player's current space. Absent ⇒ overworld; `InDungeon` scopes
    /// their movement + snapshot to a live [`DungeonInstance`].
    location: HashMap<String, Location>,
    /// DG-3b: live dungeon subinstances, keyed by a minted [`DungeonKey`]. Per-entry
    /// fresh — created on entry, dropped when the last occupant leaves.
    dungeons: HashMap<u64, DungeonInstance<'static>>,
    /// Monotonic source of unique dungeon keys.
    next_dungeon_key: u64,
    /// High-water mark: sections `0..entrances_scanned` have already been rolled for
    /// a dungeon entrance (covers the initial chain on the first tick + streamed ones).
    entrances_scanned: usize,
    /// DG-6b: last `world.dungeon_scene` state sent to each player — `Some((active,
    /// floor))` — so the per-tick loop emits the re-skin cue only on a transition
    /// (descend / floor change / exit), not every tick. Purely presentational.
    dungeon_scene_sent: HashMap<String, (bool, usize)>,
    /// CL-1 milestones raised by handlers that cannot return a `WorldEffect`
    /// (a descent, for one). Drained on the next `tick`, so a milestone is never
    /// lost just because its call path returns only messages.
    pending_effects: Vec<WorldEffect>,
    /// player_id -> per-hero temporary edges a smith put on their kit this run (MS-1
    /// `enhance`). Kept apart from `gear_bonuses` on purpose: that mirror is rebuilt
    /// from Postgres whenever gear changes, and an edge must survive re-equipping.
    edges: HashMap<String, Vec<Edge>>,
    /// player_id -> persistent Meld skill levels, mirrored in at form_run. The world
    /// gates the professions' field verbs on these (raising a station, and whose skill
    /// a station's work is done at), so it must not have to ask Postgres mid-tick.
    skill_levels: HashMap<String, HashMap<String, i32>>,
    /// player_id -> wall-clock ms until which `resolve_touches` won't start them a new
    /// battle. Set on every way out of a fight (victory, defeat-with-survivors, flee) so
    /// a monster that was already adjacent can't yank the player straight back in while
    /// the previous result is still on screen, and so fleeing buys a real window to walk
    /// away instead of the fight restarting on the very next tick. `meld-world` must stay
    /// pure (no wall-clock), so this lives here and is passed into `check_touch` as an
    /// exclusion set rather than being computed inside the arena.
    battle_immune_until: HashMap<String, u64>,
    /// player_id -> the quarry of every hunt they are still working (AD-4), mirrored in
    /// from the Router's session board so the snapshot can force-include and mark it.
    quarry: HashMap<String, Vec<QuarryTarget>>,
    /// player_id -> their standing bounty contracts (AD-4), mirrored in from the DB. The
    /// world stands each mark up once the frontier reaches its sighted distance.
    bounties: HashMap<String, Vec<(String, meld_proto::bounties::BountySpec)>>,
    /// Contract ids whose mark has already been stood up in this world, so a mark is
    /// never two creatures and a felled one never comes back.
    marks_placed: std::collections::HashSet<String>,
    /// The world clock, in ticks since this world was seeded. Everything the Shifting
    /// Lands do is scheduled against it and NEVER against wall-clock (CANON §W2), which
    /// is what keeps the world deterministic and what makes `(seed, tick, generation)`
    /// enough to replay it after a restart.
    tick_count: u64,
    /// Which Shift the world is currently counting down to. Retired (incremented) the
    /// tick it lands, so the schedule is never re-derived from anything but this.
    shift_generation: u64,
    /// Whether the current generation's tell has already gone up, so the warning is
    /// announced once rather than every tick of the window.
    shift_warned: bool,
    /// Every landed Shift as `(generation, first, last)` — CANON §W5's event log, and
    /// the ONLY part of the Shift that is not re-derivable from the seed. `shift_region`
    /// picks the least-recently-disturbed span half the time, so which sections a Shift
    /// reached depends on how far the world had streamed when it landed; that is history,
    /// and history has to be written down.
    shift_log: Vec<(u64, usize, usize)>,
}

/// CANON §W5 — everything about a world that is NOT derivable from its seed.
///
/// The baseline map (terrain, biome layout, initial placement) is regenerated from the
/// seed and never stored, and the natural Shift schedule is a pure function of
/// `(seed, generation)` — so the only things written down are the landed Shifts' chosen
/// regions (the schedule says *when* and *how big*; which sections it actually reached
/// depends on how far the world had streamed at the time, which is history rather than
/// arithmetic) and what players did to the place.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
struct WorldDelta {
    /// `(generation, first_section, last_section)` per landed Shift, in order. Replaying
    /// these against `shift::roll` reproduces every retiled biome and every re-scattered
    /// prop exactly, because `apply_shift` is deterministic given the roll and the span.
    shifts: Vec<(u64, usize, usize)>,
    /// Nodes that are not at full stock: `(entity_id, remaining, spent_tick)`.
    nodes: Vec<(String, i32, u64)>,
    /// Chests somebody has opened: `(entity_id, opened_tick)`.
    chests: Vec<(String, u64)>,
    /// Ground a creature used to hold and has not yet regrown into.
    fallen: Vec<FallenDto>,
    /// Field stations still standing (MS-1).
    stations: Vec<StationDto>,
    /// Player-built structures (CANON D21/§W3). The most load-bearing entry in the whole
    /// delta: an anchor IS the ground a player holds, and a world that forgot one on
    /// restart would hand their region back to the Shift for free.
    #[serde(default)]
    structures: Vec<StructureDto>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct StructureDto {
    id: String,
    function: String,
    owner: String,
    x: f64,
    y: f64,
    elevation: u8,
    hp: i32,
    max_hp: i32,
    placed_tick: u64,
    build_ticks: u64,
    ore: String,
    ore_cost: i32,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct FallenDto {
    id: String,
    kind: String,
    x: f64,
    y: f64,
    min_x: f64,
    max_x: f64,
    felled_tick: u64,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct StationDto {
    id: String,
    kind: String,
    x: f64,
    y: f64,
    elevation: u8,
    owner: String,
    uses_left: i32,
    stock: String,
}

/// The one world key today. Multi-world (SC-3) is what varies it; until then a single
/// constant keeps the schema honest about being keyed rather than pretending there can
/// only ever be one row.
const WORLD_KEY: &str = "default";

impl WorldActor {
    /// Fold the live world down to what §W5 actually stores.
    fn world_save(&self) -> meld_db::WorldSave {
        let delta = WorldDelta {
            shifts: self.shift_log.clone(),
            nodes: self
                .arena
                .resources
                .iter()
                .filter(|n| n.spent_tick > 0 || n.depleted())
                .map(|n| (n.entity_id.clone(), n.remaining, n.spent_tick))
                .collect(),
            chests: self
                .arena
                .chests
                .iter()
                .filter(|c| c.opened)
                .map(|c| (c.entity_id.clone(), c.opened_tick))
                .collect(),
            fallen: self
                .arena
                .fallen
                .iter()
                .map(|f| FallenDto {
                    id: f.entity_id.clone(),
                    kind: f.monster_kind.clone(),
                    x: f.home.x,
                    y: f.home.y,
                    min_x: f.area_min_x,
                    max_x: f.area_max_x,
                    felled_tick: f.felled_tick,
                })
                .collect(),
            stations: self
                .arena
                .stations
                .iter()
                .map(|s| StationDto {
                    id: s.entity_id.clone(),
                    kind: s.kind.clone(),
                    x: s.position.x,
                    y: s.position.y,
                    elevation: s.elevation,
                    owner: s.owner_player_id.clone(),
                    uses_left: s.uses_left,
                    stock: s.stock.clone(),
                })
                .collect(),
            structures: self
                .arena
                .structures
                .iter()
                .map(|s| StructureDto {
                    id: s.entity_id.clone(),
                    function: s.function.clone(),
                    owner: s.owner_player_id.clone(),
                    x: s.position.x,
                    y: s.position.y,
                    elevation: s.elevation,
                    hp: s.hp,
                    max_hp: s.max_hp,
                    placed_tick: s.placed_tick,
                    build_ticks: s.build_ticks,
                    ore: s.ore.clone(),
                    ore_cost: s.ore_cost,
                })
                .collect(),
        };
        meld_db::WorldSave {
            world_key: WORLD_KEY.to_string(),
            seed: self.arena.seed as i64,
            tick_count: self.tick_count as i64,
            shift_generation: self.shift_generation as i64,
            sections: self.arena.areas.len() as i32,
            delta: serde_json::to_string(&delta).unwrap_or_else(|_| "{}".to_string()),
        }
    }
}

/// Rebuild a hibernated world: regenerate the baseline from the seed, stream the
/// frontier back out, replay every landed Shift, then re-apply what players changed.
///
/// The replay is why the log stores each Shift's *span* and not just its generation:
/// `shift_region` picks the least-recently-disturbed section half the time, and at
/// restore every section exists at once, so re-deriving the span would pick differently
/// than a world that grew into it did. The roll itself still comes from the seed.
fn restore_world(balance: &Balance, save: &meld_db::WorldSave) -> Arena {
    let mut arena = Arena::generate_with(balance, save.seed as u64, false, None);
    let want = save.sections.max(0) as usize;
    // `ensure_frontier` streams a bounded few per call (a teleport must not explode one
    // tick's work), so ask repeatedly rather than once with a huge reach.
    let mut guard = 0;
    while arena.areas.len() < want && guard < want * 4 + 64 {
        guard += 1;
        let reach = arena.areas.last().map(|a| a.end_x).unwrap_or(0.0);
        arena.ensure_frontier(balance, reach + 1.0);
    }
    let delta: WorldDelta = serde_json::from_str(&save.delta).unwrap_or_default();
    for &(generation, first, last) in &delta.shifts {
        if first >= arena.areas.len() {
            continue;
        }
        let roll = meld_world::shift::roll(balance, arena.seed, generation);
        let last = last.min(arena.areas.len() - 1);
        arena.apply_shift(balance, &roll, first, last);
    }
    for (id, remaining, spent) in &delta.nodes {
        if let Some(n) = arena.resources.iter_mut().find(|n| &n.entity_id == id) {
            n.remaining = *remaining;
            n.spent_tick = *spent;
        }
    }
    for (id, opened) in &delta.chests {
        if let Some(c) = arena.chests.iter_mut().find(|c| &c.entity_id == id) {
            c.opened = true;
            c.opened_tick = *opened;
        }
    }
    // A creature the world remembers as dead must not also be standing there.
    let dead: std::collections::HashSet<&String> = delta.fallen.iter().map(|f| &f.id).collect();
    arena.monsters.retain(|m| !dead.contains(&m.entity_id));
    arena.fallen = delta
        .fallen
        .iter()
        .map(|f| meld_world::Fallen {
            entity_id: f.id.clone(),
            monster_kind: f.kind.clone(),
            home: Position::new(f.x, f.y),
            area_min_x: f.min_x,
            area_max_x: f.max_x,
            felled_tick: f.felled_tick,
        })
        .collect();
    arena.structures = delta
        .structures
        .iter()
        .map(|s| meld_world::Structure {
            entity_id: s.id.clone(),
            function: s.function.clone(),
            owner_player_id: s.owner.clone(),
            position: Position::new(s.x, s.y),
            elevation: s.elevation,
            hp: s.hp,
            max_hp: s.max_hp,
            placed_tick: s.placed_tick,
            build_ticks: s.build_ticks,
            ore: s.ore.clone(),
            ore_cost: s.ore_cost,
        })
        .collect();
    arena.stations = delta
        .stations
        .iter()
        .map(|s| meld_world::Station {
            entity_id: s.id.clone(),
            kind: s.kind.clone(),
            position: Position::new(s.x, s.y),
            elevation: s.elevation,
            owner_player_id: s.owner.clone(),
            uses_left: s.uses_left,
            stock: s.stock.clone(),
        })
        .collect();
    arena
}


/// Spend `need` units of one material `class` out of a run's backpack, deepest tier first,
/// **across stacks**. Returns the material kind actually spent.
///
/// Across stacks is the whole point. A harvest channel banks one unit per tick as its OWN
/// `ItemStack` (`advance_harvests` pushes rather than merging), so ore you gathered in the
/// field is six stacks of one — never one stack of six. Both build paths used to look for a
/// SINGLE stack holding the whole cost, which meant a structure costing 6 and a field forge
/// costing 3 were **both unbuildable from ore you had just dug up**, while reporting only
/// "takes 6 ore" to a player who was carrying exactly six.
///
/// One KIND, though, not a mix: a structure records what it was built from so packing it
/// down hands back the same stock, and a refund cannot be split across materials the player
/// no longer has a record of. So the deepest-tier kind whose TOTAL covers the cost wins.
fn spend_material(
    run: &mut meld_run::PlayerRun,
    class: meld_proto::materials::MaterialClass,
    need: i32,
) -> Option<String> {
    let mut totals: HashMap<String, (i32, i32)> = HashMap::new();
    for item in run.backpack.iter().filter(|i| {
        i.quantity > 0 && meld_proto::materials::is_class(&i.item_kind, class)
    }) {
        let tier = meld_proto::materials::material(&item.item_kind).map(|m| m.tier).unwrap_or(0);
        let e = totals.entry(item.item_kind.clone()).or_insert((0, tier));
        e.0 += item.quantity;
    }
    let kind = totals
        .into_iter()
        .filter(|(_, (have, _))| *have >= need)
        .max_by_key(|(_, (_, tier))| *tier)
        .map(|(k, _)| k)?;
    let mut left = need;
    for item in run.backpack.iter_mut().filter(|i| i.item_kind == kind) {
        let take = left.min(item.quantity);
        item.quantity -= take;
        left -= take;
        if left == 0 {
            break;
        }
    }
    run.backpack.retain(|i| i.quantity > 0);
    Some(kind)
}


/// DEV/QA: dress a party as though it were wearing a full six-slot set of tier-`n` insured
/// epics (`MELD_GEAR_TIER`), on top of whatever the Vault actually holds.
///
/// **Applied wherever `gear_bonuses` is written, not just at `form_run`.** It used to be set
/// once when the run formed, and then `flush_gear_loads` — which mirrors the real Vault into
/// the world a tick later — overwrote it with the empty set. So the flag dressed the party
/// for exactly as long as it took the first gear load to land, and every measurement taken
/// through it was of an UNGEARED party wearing the word "geared".
///
/// Measured: at tier 3 a level-28 party's skills did -103 and -127 dressed, and -103 and
/// -127 undressed. Identical to the digit.
fn dress_for_dev(
    balance: &Balance,
    classes: &[CharacterClass],
    gear: Vec<meld_db::GearBonus>,
) -> Vec<meld_db::GearBonus> {
            // `MELD_GEAR_TIER=<n>` — DEV/QA: dress every hero as if it were wearing a full
    // set of tier-`n` insured epics, without a Vault full of them. The end fight is
    // tuned as a GEAR CHECK, and the difference between a geared and an ungeared
    // party there is 3.5x survivability — so without this the only observable case
    // is the ungeared one, and the number that matters cannot be looked at.
    //
    // Mirrors what `equipped_gear_bonuses` derives from a real six-slot set: one
    // weapon's worth of atk, four armour pieces' worth of def, each carrying two
    // epic stat affixes.
    let gear = match std::env::var("MELD_GEAR_TIER")
        .ok()
        .and_then(|v| v.trim().parse::<i32>().ok())
    {
        Some(tier) if tier > 0 => {
            let l = &balance.loot;
            let af = &balance.affix;
            let piece = l.gear_atk_per_tier * tier as f64 * l.insured_power_mult;
            let affix = af.magnitude_per_tier * tier as f64 * af.count_epic as f64;
            let atk = (piece + affix).round() as i32;
            let def = (4.0 * (piece + affix)).round() as i32;
            tracing::warn!(tier, atk, def, "MELD_GEAR_TIER: heroes dressed (DEV/QA)");
            // Four armour pieces of the weight the CLASS actually wears, so the
            // dressed party answers for damage types the way a real set would.
            // Without this the harness could see the atk/def half of gear and not
            // the resistance half — which is the half the apex is gated on.
            let comp = classes.to_vec();
            gear.iter()
                .enumerate()
                .map(|(i, _)| {
                    let weight = comp
                        .get(i)
                        .and_then(|c| {
                            meld_proto::equipment::armor_weights(*c).first().copied()
                        })
                        .map(|w| w.wire().to_string());
                    // `MELD_GEAR_WARD=<TYPE>` dresses the set with an elemental ward
                    // on every piece — what a player who KNEW what they were walking
                    // into would bring. Without it the harness can only show gear's
                    // physical half, and a fire fight is not answered by plate.
                    let ward = std::env::var("MELD_GEAR_WARD").ok().and_then(|t| {
                        let key = t.trim().to_uppercase();
                        meld_proto::enums::DamageType::from_wire(&key).map(|_| key)
                    });
                    // A tier-`n` set rolls Aegis (flat ward) and Furnace (element
                    // power) lines like any other epic, so the harness grants them
                    // too — otherwise a dressed party shows gear's physical half and
                    // none of its elemental one, which is the half the apex needs.
                    let ward_stat = (af.magnitude_per_tier * tier as f64
                        * af.count_epic as f64)
                        .round() as i32;
                    GearBonus {
                        atk,
                        def,
                        ward: ward_stat,
                        armor_weights: weight
                            .into_iter()
                            .flat_map(|w| std::iter::repeat_n(w, 4))
                            .collect(),
                        modifiers: ward
                            .into_iter()
                            .flat_map(|k| std::iter::repeat_n((k, 0.75), 4))
                            .collect(),
                        ..Default::default()
                    }
                })
                .collect()
        }
        _ => gear,
    };
    gear
}

/// One queued request at a field station: everything the DB half needs, decided
/// already by the world half. `owner`/`smith_level` are the STATION's smith — the
/// skill the job is done at — while `requester` is whose gear it is. They are separate
/// fields precisely because they are allowed to be different players, and because the
/// only Vault ever touched is the requester's.
struct SmithJob {
    requester: String,
    owner: String,
    /// Which bench this is: `smith` (a forge) or `alembic` (a still). Empty = the city.
    kind: String,
    smith_level: i32,
    /// Other smiths in the party lending a hand — they widen the yellow.
    crew: i32,
    station_id: String,
    gear_id: String,
    service: String,
    material: String,
    /// The recipe a brew cooks (alembic only).
    recipe: String,
    client_seq: u32,
    /// The heat's quality once it has been struck and graded.
    quality: f64,
}

/// A heat waiting on its blows: the bar the server laid out, and what has landed so far.
struct OpenHeat {
    job: SmithJob,
    heat: meld_world::tempo::Heat,
    strikes: Vec<f64>,
    opened_at: u64,
}

/// A class's name as a player reads it, for a refusal that names who is missing.
fn class_label(c: CharacterClass) -> &'static str {
    match c {
        CharacterClass::Smithwright => "Smithwright",
        CharacterClass::Keeper => "Keeper",
        other => meld_run::class_key(other),
    }
}

/// A placed dungeon entrance in the overworld (DG-3).
struct DungeonEntrance {
    entity_id: String,
    /// The authored dungeon this entrance leads to (`meld_dungeon_content` name).
    dungeon: &'static str,
    position: Position,
}

/// A static dungeon-floor prop as a snapshot entity (DG-3b crude render — walls,
/// doors, exits, etc. mapped onto existing client tags until DG-6b).
fn dungeon_prop(entity_id: String, position: Position, tag: &str) -> wm::SnapshotEntity {
    wm::SnapshotEntity {
        entity_id,
        position,
        velocity: wm::Velocity { x: 0.0, y: 0.0 },
        avatar_state: Some(tag.to_string()),
        level: Some(0),
        ..Default::default()
    }
}

impl WorldActor {
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
    /// The players FIGHTING this battle — the ones whose heroes are in it, who earn its
    /// XP, take its loot, and answer for its outcome. Never the audience: a watcher did
    /// not flee, did not clear the dungeon, and is owed nothing.
    fn fighters_of(&self, slot: &BattleSlot) -> Vec<String> {
        self.run
            .runs
            .iter()
            .filter(|r| slot.parties.contains(&r.party_id))
            .map(|r| r.player_id.clone())
            .collect()
    }

    /// Everyone this battle's messages go to: its fighters PLUS anyone watching it
    /// (`SOC-3`). This is the ONE audience funnel — every battle broadcast asks it, so a
    /// watcher gains a new message type the day it is added rather than the day someone
    /// remembers to add them to its call site. (The repo has been bitten twice by the
    /// same rule living in two places; a spectator feed missing one message reads as the
    /// fight freezing.)
    fn audience_of(&self, slot: &BattleSlot) -> Vec<String> {
        let mut who = self.fighters_of(slot);
        // A watcher who has since started their own fight is no longer audience for this
        // one — they are looking at their own screen, and the sweep will drop them this
        // tick. Filter here too so the two can never disagree mid-tick.
        who.extend(
            slot.spectators
                .iter()
                .filter(|pid| self.party_id_of(pid).is_none_or(|p| !slot.parties.contains(&p)))
                .cloned(),
        );
        who
    }

    /// A player's earned overworld class perks (Overworld Class Perks / "party
    /// sense"): class *presence* in the party gates each perk, the shared
    /// `run_level` scales its tier. Defaults (no perks) if the player isn't in
    /// the instance. See [`Self::compute_perks`] and the `[perks]` balance block.
    fn perks_for(&self, pid: &str) -> wr::Perks {
        let Some(classes) = self.party_classes.get(pid) else {
            return wr::Perks::default();
        };
        let run_level = self
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
        compute_perks(&self.balance.perks, classes, run_level)
    }
}

/// One player's overworld-regen tick: the two sources, the party's HP caps, and which
/// slots are Resonants (the only heroes the walking regen tends).
struct RegenPlan {
    player_id: String,
    own: f32,
    field: f32,
    caps: Vec<i32>,
    healers: Vec<usize>,
}

/// Sub-1 HP banked per source. They are kept apart because they reach different heroes,
/// so a shared remainder would let the field's overflow heal through the Resonant-only
/// rule (and vice versa).
#[derive(Default)]
pub(crate) struct RegenAccum {
    own: f32,
    field: f32,
}

/// Take the whole HP out of an accumulator, leaving the remainder banked.
fn take_whole(acc: &mut f32) -> i32 {
    let whole = acc.floor();
    if whole < 1.0 {
        return 0;
    }
    *acc -= whole;
    whole as i32
}

/// Spend `budget` HP into `hps`, most-wounded living hero first. `eligible` restricts it
/// to those slots (the Resonant-only source); `None` means the whole party.
fn pour_regen(hps: &mut [i32], caps: &[i32], eligible: Option<&[usize]>, mut budget: i32) {
    while budget > 0 {
        let mut best: Option<usize> = None;
        let mut best_deficit = 0;
        for (i, h) in hps.iter().enumerate() {
            if eligible.is_some_and(|e| !e.contains(&i)) {
                continue;
            }
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

/// Pure mapping from (party classes x run level) -> earned perks against the `[perks]`
/// balance thresholds. A perk stays neutral unless its class is in the party. A free
/// function rather than a method because it reads nothing but balance — which is what
/// makes it unit-testable without standing up an instance.
pub(crate) fn compute_perks(
    p: &meld_balance::Perks,
    classes: &[CharacterClass],
    run_level: i32,
) -> wr::Perks {
    {
        let has = |c: CharacterClass| classes.contains(&c);
        let lvl = run_level.max(1);
        let above = |floor: i32| (lvl - floor).max(0) as f32;
        let mut out = wr::Perks::default();
        // Explorer — the lantern, and the MAP. The order whose vision is "a world
        // known" is the one that carries the minimap (docs/lore/factions.md).
        if has(CharacterClass::Explorer) {
            out.explorer_glow = p.explorer_glow_base + p.explorer_glow_per_level * (lvl - 1) as f32;
            if lvl >= p.explorer_map_at {
                out.explorer_map = if lvl >= p.explorer_map_harvest_at {
                    3
                } else if lvl >= p.explorer_map_chests_at {
                    2
                } else {
                    1
                };
                out.explorer_map_radius = p.explorer_map_radius_base
                    + p.explorer_map_radius_per_level * above(p.explorer_map_at);
            }
        }
        // Hunter — the predator's eye. Sizing up prey before committing is the guild's
        // trade, so creature intel belongs to it rather than to the mapmakers.
        if has(CharacterClass::Hunter) {
            out.hunter_intel = if lvl >= p.hunter_intel_atb_at {
                3
            } else if lvl >= p.hunter_intel_hp_at {
                2
            } else if lvl >= p.hunter_intel_level_at {
                1
            } else {
                0
            };
            // Threat sense is the same eye at longer range: what is dangerous before it
            // is in reach. It used to be the Psyker's, where it duplicated this lane and
            // stopped growing at run level 3.
            if lvl >= p.hunter_threat_elites_at {
                out.hunter_threat = if lvl >= p.hunter_threat_aggro_at { 2 } else { 1 };
                out.hunter_reveal_radius = (p.hunter_reveal_base
                    + p.hunter_reveal_per_level * above(p.hunter_threat_elites_at) as f64)
                    as f32;
            }
        }
        // Shifter — Shift-sense. Not a map: a Runner reads the instability a door
        // leaks, and can tell what is worth carrying out before touching it.
        if has(CharacterClass::Shifter) {
            if lvl >= p.shifter_dungeon_at {
                out.shifter_dungeon_radius = p.shifter_dungeon_radius_base
                    + p.shifter_dungeon_radius_per_level * above(p.shifter_dungeon_at);
            }
            out.shifter_item_sense = lvl >= p.shifter_item_sense_at;
            if lvl >= p.shifter_trap_sense_at {
                out.shifter_trap_radius = p.shifter_trap_radius_base
                    + p.shifter_trap_radius_per_level * above(p.shifter_trap_sense_at);
            }
        }
        // Psyker — telekinesis. Seeing went to the Hunter and the map is the Explorer's,
        // so what is left for the order of manifestations is a VERB: it reaches out and
        // pins a creature where it stands. Duration and count grow; the cooldown shortens
        // to a floor, because a pin with no gap between uses walks past all content.
        if has(CharacterClass::Psyker) && lvl >= p.psyker_hold_at {
            let over = above(p.psyker_hold_at);
            out.psyker_hold_seconds = (p.psyker_hold_seconds_base
                + p.psyker_hold_seconds_per_level * over)
                .min(p.psyker_hold_seconds_cap);
            out.psyker_hold_cooldown = (p.psyker_hold_cooldown_base
                - p.psyker_hold_cooldown_per_level * over)
                .max(p.psyker_hold_cooldown_floor);
            out.psyker_hold_radius = p.psyker_hold_radius;
            out.psyker_hold_targets = if lvl >= p.psyker_hold_targets_at {
                let step = p.psyker_hold_targets_per_level.max(1);
                (2 + (lvl - p.psyker_hold_targets_at) / step).min(p.psyker_hold_targets_cap) as u8
            } else {
                1
            };
            out.psyker_mind_link = lvl >= p.psyker_mind_link_at;
        }
        // Resonant — overworld regen (HP/sec).
        if has(CharacterClass::Resonant) {
            out.resonant_regen = p.resonant_regen_per_level * lvl as f32;
        }
        // Smithwright — the Foundry's half of MS-1's second ladder. It reads rock at
        // range, raises benches quicker and cheaper than anyone, gets its whole stock
        // back when it packs one up, and its benches outlast other people's.
        if has(CharacterClass::Smithwright) {
            if lvl >= p.smithwright_ore_sense_at {
                out.smithwright_ore_radius = p.smithwright_ore_radius_base
                    + p.smithwright_ore_radius_per_level * above(p.smithwright_ore_sense_at);
            }
            if lvl >= p.smithwright_setup_at {
                out.smithwright_setup_mult = p.smithwright_setup_mult;
                out.smithwright_stock_discount = p.smithwright_stock_discount;
            }
            out.smithwright_pack_full = lvl >= p.smithwright_pack_full_at;
            if lvl >= p.smithwright_bench_uses_at {
                out.smithwright_bench_uses = p.smithwright_bench_uses_bonus;
            }
        }
        // Keeper — the Open Flower's half. It reads growing things at range, takes more
        // from a bed than anyone, and its still is a place the party can actually rest.
        if has(CharacterClass::Keeper) {
            if lvl >= p.keeper_reagent_sense_at {
                out.keeper_reagent_radius = p.keeper_reagent_radius_base
                    + p.keeper_reagent_radius_per_level * above(p.keeper_reagent_sense_at);
            }
            if lvl >= p.keeper_green_thumb_at {
                out.keeper_extra_unit_chance = p.keeper_green_thumb_chance as f32;
            }
            if lvl >= p.keeper_rooted_at {
                out.keeper_field_radius_mult = p.keeper_rooted_radius_mult;
                out.keeper_field_regen_mult = p.keeper_rooted_regen_mult;
            }
            if lvl >= p.keeper_whole_vein_at {
                out.keeper_free_unit_chance = p.keeper_whole_vein_chance as f32;
            }
        }
        // Phoenix Guard — bulwark (shrinks how close creatures chase this party).
        if has(CharacterClass::PhoenixGuard) {
            let mult = 1.0 - p.phoenix_guard_aggro_reduction_per_level * lvl as f64;
            out.phoenix_guard_aggro_mult = mult.max(p.phoenix_guard_aggro_mult_floor) as f32;
        }
        out
    }
}

impl WorldActor {
    fn snapshot_msgs(&mut self) -> Vec<Outgoing> {
        let mut entities: Vec<wm::SnapshotEntity> = self
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
        // AD-4: a hunt's quarry is force-included in its holder's own snapshot from
        // further out than anyone else sees it, so it can be tracked rather than
        // stumbled upon. Remembered here the way node-sense remembers nodes.
        let mut mob_index: Vec<(usize, Position, &str, &str)> = Vec::new();
        // A bounty MARK exists for one player (AD-4): its index is remembered here so
        // every other player's cull drops it. A contract with your name on it must not be
        // scenery in a stranger's world — or worse, a fight they can watch you lose.
        let mut mark_index: Vec<(usize, String)> = Vec::new();
        // CR-2: which creatures are actually trading blows right now. A clash is an
        // EVENT, not intel, so its marker rides the shared tag rather than a perk gate
        // — you can see a brawl in front of you without a Hunter in the party.
        let clashing: std::collections::HashSet<String> =
            self.arena.clashing().into_iter().map(String::from).collect();
        for m in self.arena.monsters.iter().filter(|m| !m.defeated) {
            mob_index.push((
                entities.len(),
                m.position,
                m.monster_kind.as_str(),
                m.encounter_class.as_str(),
            ));
            if !m.owner.is_empty() {
                mark_index.push((entities.len(), m.owner.clone()));
            }
            entities.push(wm::SnapshotEntity {
                entity_id: m.entity_id.clone(),
                position: m.position,
                velocity: wm::Velocity { x: 0.0, y: 0.0 },
                // A pinned creature says so, so the party can SEE the opening it paid a
                // cooldown for — an affordance you cannot read is one you will not use.
                avatar_state: Some({
                    // Markers are a SET, appended in order and read as a set by the
                    // client: a pinned creature can also be a quarry (which the
                    // per-viewer cull appends below), and a marker that only survives
                    // when it happens to sort first is a marker that vanishes at random.
                    let mut tag = format!("mob:{}:{}", m.monster_kind, m.faction);
                    if m.held_for > 0.0 {
                        tag.push_str(":held");
                    }
                    if clashing.contains(&m.entity_id) {
                        tag.push_str(":clash");
                    }
                    tag
                }),
                level: Some(m.elevation),
                // Overworld mob intel (client shows each field only when the
                // viewer's Explorer/Psyker perk unlocks it — see `run.perks`).
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
            position: self.arena.portal,
            velocity: wm::Velocity { x: 0.0, y: 0.0 },
            avatar_state: Some("portal".to_string()),
            level: Some(0),
            ..Default::default()
        });
        // Treasure chests, tagged `chest:<tier>:<open>` (`open` = 0/1) so the client
        // draws unopened vs opened. Opened chests stay in the world (as opened).
        for c in &self.arena.chests {
            entities.push(wm::SnapshotEntity {
                entity_id: c.entity_id.clone(),
                position: c.position,
                velocity: wm::Velocity { x: 0.0, y: 0.0 },
                avatar_state: Some(format!("chest:{}:{}", c.tier, c.opened as u8)),
                level: Some(c.elevation),
                ..Default::default()
            });
        }
        // Un-harvested resource nodes, tagged `resource:<kind>` for the client. Their
        // index and material CLASS are remembered so a crafter's node-sense can force
        // them into its own snapshot from further out than anyone else sees them.
        let mut node_index: Vec<(usize, Position, Option<meld_proto::materials::MaterialClass>)> =
            Vec::new();
        for n in self.arena.resources.iter().filter(|n| !n.depleted()) {
            let class = self
                .balance
                .resource
                .get(&n.kind)
                .and_then(|r| meld_proto::materials::material(&r.material))
                .map(|m| m.class);
            node_index.push((entities.len(), n.position, class));
            entities.push(wm::SnapshotEntity {
                entity_id: n.entity_id.clone(),
                position: n.position,
                velocity: wm::Velocity { x: 0.0, y: 0.0 },
                avatar_state: Some(format!("resource:{}", n.kind)),
                level: Some(n.elevation),
                ..Default::default()
            });
        }
        // Player-raised field stations, tagged `station:<kind>:<uses>` so the client can
        // draw the bench and count its remaining jobs in the prompt. A spent station is
        // gone from the snapshot, which is how it reads as used up.
        for st in self.arena.stations.iter().filter(|s| !s.spent()) {
            entities.push(wm::SnapshotEntity {
                entity_id: st.entity_id.clone(),
                position: st.position,
                velocity: wm::Velocity { x: 0.0, y: 0.0 },
                avatar_state: Some(format!("station:{}:{}", st.kind, st.uses_left)),
                level: Some(st.elevation),
                ..Default::default()
            });
        }
        // Ground loot dropped by creature-vs-creature skirmishes, tagged
        // `loot:<kind>` — walk over it to auto-collect (see `collect_ground_loot`).
        for l in &self.arena.ground_loot {
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
        for o in &self.arena.obstacles {
            entities.push(wm::SnapshotEntity {
                entity_id: o.entity_id.clone(),
                position: o.position,
                velocity: wm::Velocity { x: 0.0, y: 0.0 },
                avatar_state: Some(format!("obstacle:{}:{:.2}", o.kind, o.radius)),
                level: None,
                ..Default::default()
            });
        }
        // Player-built structures (CANON D21/§W3), tagged
        // `structure:<function>:<hp_pct>:<building>` — ONE tag for every function, so a
        // new function needs no new render path and cannot be forgotten by one.
        for st in &self.arena.structures {
            entities.push(wm::SnapshotEntity {
                entity_id: st.entity_id.clone(),
                position: st.position,
                velocity: wm::Velocity { x: 0.0, y: 0.0 },
                avatar_state: Some(format!(
                    "structure:{}:{}:{}",
                    st.function,
                    st.hp_pct(),
                    u8::from(st.building(self.tick_count))
                )),
                level: Some(st.elevation),
                ..Default::default()
            });
        }
        // DG-3: hand-designed dungeon entrances, tagged `entrance:<dungeon>` — walk
        // up to descend (the enter flow lands in the next increment). Pushed before
        // the interest grid so they cull by position like any other entity.
        for e in &self.entrances {
            entities.push(wm::SnapshotEntity {
                entity_id: e.entity_id.clone(),
                position: e.position,
                velocity: wm::Velocity { x: 0.0, y: 0.0 },
                // `entrance:<dungeon>:<bodies>` — how many heroes the doors inside
                // want held on plates at once. A dungeon takes no Town Portal, so a
                // party that learns this on the far side of the maze has wasted the
                // trip; the door says it up front.
                avatar_state: Some(format!(
                    "entrance:{}:{}",
                    e.dungeon,
                    meld_dungeon_content::by_name(e.dungeon)
                        .map(|d| d.bodies_required())
                        .unwrap_or(1)
                )),
                level: Some(0),
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
        //
        // SC-1: the cull runs off a per-tick chunk grid (built once here) so each
        // player's query touches only the cells in range instead of re-scanning the
        // whole entity list — O(sessions × visible) not O(sessions × entities).
        let cell = self.balance.world.chunk_size.max(1) as f64;
        let radius = self.balance.world.interest_radius_chunks.max(0) as f64 * cell;
        let radius2 = radius * radius;
        // AD-4: which mobs are each player's QUARRY, decided before anything takes `self`
        // mutably (and so before `mob_index`'s borrows end). A Hunter senses one from much
        // further out — the guild's whole trade — but anyone holding a posted hunt knows
        // what they are looking for.
        let quarry_marks: HashMap<String, std::collections::HashSet<usize>> = self
            .run
            .runs
            .iter()
            .filter_map(|r| {
                let targets = self.quarry.get(&r.player_id).filter(|q| !q.is_empty())?;
                let pos = self.arena.avatar(&r.player_id)?.position;
                let h = &self.balance.hunt;
                let reach = if self.perks_for(&r.player_id).hunter_intel > 0 {
                    h.quarry_sense_hunter_radius
                } else {
                    h.quarry_sense_radius
                }
                .max(radius);
                let hits: std::collections::HashSet<usize> = mob_index
                    .iter()
                    .filter(|(_, mpos, kind, class)| {
                        pos.distance_to(mpos) <= reach
                            && targets.iter().any(|t| t.matches(kind, class))
                    })
                    .map(|(i, _, _, _)| *i)
                    .collect();
                (!hits.is_empty()).then(|| (r.player_id.clone(), hits))
            })
            .collect();
        let grid = build_interest_grid(&entities, cell);
        let portal_idx = entities.iter().position(|e| e.entity_id == "portal");
        // Overworld snapshots go to players NOT in any battle. A fighting party is
        // on the battle screen and driven by battle messages instead; when no battle
        // is running, `in_battle` is empty so this sends to everyone.
        let in_battle = self.parties_in_battle();
        let mut out = Vec::new();
        // DG-6b: emit the client re-skin cue (`world.dungeon_scene`) on a *transition*
        // only — descend / floor-change / exit. Computed up front (a `&mut self` diff
        // against the last-sent scene) so the snapshot loop below stays an immutable
        // borrow of `self`. All scene deltas precede the snapshots, so a player is in
        // dungeon mode before that space's floor geometry arrives. Purely cosmetic.
        let scene_players: Vec<String> = self
            .run
            .runs
            .iter()
            .filter(|r| !in_battle.contains(&r.party_id))
            .map(|r| r.player_id.clone())
            .collect();
        for pid in &scene_players {
            if let Some(scene) = self.dungeon_scene_delta(pid) {
                out.push(scene);
            }
        }
        for r in self
            .run
            .runs
            .iter()
            .filter(|r| !in_battle.contains(&r.party_id))
        {
            // DG-3b: a player inside a dungeon gets THAT space's snapshot (its floor
            // geometry + occupants), not the overworld cull.
            if let Some((key, floor)) = self.dungeon_of(&r.player_id) {
                out.push(self.dungeon_snapshot(&r.player_id, key, floor, server_tick));
                continue;
            }
            // Avatars are pushed first, in arena order, so an avatar's index in
            // `entities` equals its index in `arena.avatars` — reuse it as `own_idx`.
            let me = self
                .arena
                .avatars
                .iter()
                .enumerate()
                .find(|(_, a)| a.player_id == r.player_id);
            let me_pos = me.map(|(_, a)| a.position);
            let own_idx = me.map(|(i, _)| i);
            // Psyker "Threat Sense": reveal mobs beyond the normal interest radius
            // (dangerous foes sensed at range). Non-mob entities keep the base radius.
            let mob_radius = (self.perks_for(&r.player_id).hunter_reveal_radius as f64).max(radius);
            let mob_radius2 = mob_radius * mob_radius;
            // A crafter reads the half of the world its own trade is built on, from
            // further out than the interest radius: the Foundry sees ORE, the Open Flower
            // sees REAGENTS. Force-included the way the portal is, rather than widening
            // the shared cull — a wider cull would show everyone everything.
            let perks = self.perks_for(&r.player_id);
            let sight = |class: &Option<meld_proto::materials::MaterialClass>| -> f64 {
                match class {
                    Some(meld_proto::materials::MaterialClass::Ore) => {
                        perks.smithwright_ore_radius as f64
                    }
                    Some(meld_proto::materials::MaterialClass::Reagent) => {
                        perks.keeper_reagent_radius as f64
                    }
                    _ => 0.0,
                }
            };
            let sensed: Vec<usize> = match me_pos {
                Some(p) if perks.smithwright_ore_radius > 0.0 || perks.keeper_reagent_radius > 0.0 => {
                    node_index
                        .iter()
                        .filter(|(_, pos, class)| {
                            let reach = sight(class);
                            reach > 0.0 && p.distance_to(pos) <= reach
                        })
                        .map(|(i, _, _)| *i)
                        .collect()
                }
                _ => Vec::new(),
            };
            let mut marked = quarry_marks.get(&r.player_id).cloned().unwrap_or_default();
            // Your own mark is always tracked and always yours; everyone else's is not in
            // your world at all.
            let mut hidden: std::collections::HashSet<usize> = std::collections::HashSet::new();
            for (idx, owner) in &mark_index {
                if owner == &r.player_id {
                    marked.insert(*idx);
                } else {
                    hidden.insert(*idx);
                }
            }
            // BLINDED, enforced at the source. A client-side blackout is a suggestion, and a
            // hacked client would simply not honour it — so a blinded party is not SENT the
            // creatures. It still walks into them: `check_touch` runs off server positions and
            // starts the fight regardless, which is the point. You cannot see what is out
            // there, and it can still find you.
            if self
                .hero_afflictions
                .get(&r.player_id)
                .is_some_and(|c| c.iter().flatten().any(|n| n == "blinded"))
            {
                for (i, e) in entities.iter().enumerate() {
                    if e.avatar_state.as_deref().is_some_and(|s| s.starts_with("mob:")) {
                        hidden.insert(i);
                    }
                }
            }
            let culled: Vec<wm::SnapshotEntity> = match me_pos {
                // Grid-indexed interest cull (SC-1): behaviour-identical to the old
                // full scan (own avatar + portal always; mobs at `mob_radius`, the
                // rest at `radius`) but O(visible) via the chunk grid.
                Some(p) => {
                    let mut idxs = interest_visible_indices(
                        &entities, &grid, cell, p.x, p.y, radius2, mob_radius, mob_radius2,
                        own_idx, portal_idx,
                    );
                    idxs.extend(sensed);
                    idxs.extend(marked.iter().copied());
                    // Mind Link (CL-2): a Psyker deep enough keeps its co-op teammates in
                    // the snapshot however far off they are. POSITIONS only — the map is
                    // the Explorer's, so this answers "where are they", never "what do
                    // they see". Force-included like the portal rather than by widening
                    // the shared cull, which would show everyone everybody.
                    if perks.psyker_mind_link {
                        idxs.extend(0..self.arena.avatars.len());
                    }
                    idxs.retain(|i| !hidden.contains(i));
                    idxs.sort_unstable();
                    idxs.dedup();
                    idxs.into_iter()
                        .map(|i| {
                            let mut e = entities[i].clone();
                            // The tag rides this player's OWN copy of the row: the same
                            // creature is not a quarry to the teammate beside them.
                            if marked.contains(&i) {
                                if let Some(st) = e.avatar_state.as_mut() {
                                    st.push_str(":quarry");
                                }
                            }
                            e
                        })
                        .collect()
                }
                // Defensive: a roaming run should always have an avatar; if not, don't
                // cull (send the full set) rather than send an empty world. Someone
                // else's mark is still not theirs to see.
                None => entities
                    .iter()
                    .enumerate()
                    .filter(|(i, _)| !hidden.contains(i))
                    .map(|(_, e)| e.clone())
                    .collect(),
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

    // --- DG-3b: dungeon subinstances (enter / move / exit + per-space snapshot) ---

    /// The `(key, floor)` of the dungeon a player is in, if any.
    fn dungeon_of(&self, pid: &str) -> Option<(u64, usize)> {
        match self.location.get(pid) {
            Some(Location::InDungeon { key, floor }) => Some((*key, *floor)),
            _ => None,
        }
    }

    /// Deliberate descent (`run.enter_dungeon`): `pid` must be an overworld,
    /// non-fighting player standing within reach of the named entrance. Returns an
    /// error message if not; otherwise enters. Entry is never automatic — you press
    /// to descend, so walking past an entrance never pulls you in.
    fn enter_dungeon_by_id(&mut self, pid: &str, entity_id: &str, seq: u32) -> Vec<Outgoing> {
        if self.dungeon_of(pid).is_some() {
            return vec![error(pid, ErrorCode::InvalidState, "Already in a dungeon.", Some(seq))];
        }
        if self.battle_of_player(pid).is_some() {
            return vec![error(pid, ErrorCode::InvalidState, "Resolve the battle first.", Some(seq))];
        }
        let Some(idx) = self.entrances.iter().position(|e| e.entity_id == entity_id) else {
            return vec![error(pid, ErrorCode::NotFound, "No such dungeon entrance.", Some(seq))];
        };
        let radius = self.balance.world.interaction_radius_tiles;
        let near = self
            .arena
            .avatar(pid)
            .map(|a| a.state == "active" && a.position.distance_to(&self.entrances[idx].position) <= radius)
            .unwrap_or(false);
        if !near {
            return vec![error(pid, ErrorCode::OutOfRange, "Not close enough to the entrance.", Some(seq))];
        }
        self.enter_dungeon(pid, idx)
    }

    /// Descend `pid` into the dungeon behind entrance `entrance_idx`: mint a key,
    /// instantiate the authored dungeon stamped at the entry's overworld distance,
    /// place the player at the entrance cell, and freeze their overworld avatar at
    /// the entry position (restored on exit — you return exactly where you came in).
    fn enter_dungeon(&mut self, pid: &str, entrance_idx: usize) -> Vec<Outgoing> {
        let Some((name, entrance_pos)) = self.entrances.get(entrance_idx).map(|e| (e.dungeon, e.position))
        else {
            return Vec::new();
        };
        // `MELD_DUNGEON=<name>` forces which authored dungeon a descent loads (dev/QA
        // screenshots of a specific dungeon), read only at the server boundary so
        // `meld-world`/`meld-dungeon` stay pure — same spirit as `MELD_BIOME`/`MELD_SEED`.
        let forced = std::env::var("MELD_DUNGEON")
            .ok()
            .and_then(|n| meld_dungeon_content::by_name(&n));
        let Some(def) = forced.or_else(|| meld_dungeon_content::by_name(name)) else {
            return Vec::new();
        };
        let level = self.arena.avatar(pid).map(|a| a.position.distance_floor()).unwrap_or(0);
        let depth_step = self.balance.worldgen.dungeon_depth_level_step;
        let key = self.next_dungeon_key;
        self.next_dungeon_key += 1;
        self.dungeons.insert(key, DungeonInstance::new(key, def, level, depth_step));
        // The initiator descends, plus any teammate gathered at the entrance — a co-op
        // group of up to 4 enters *together* into one fresh subinstance (design §3;
        // `[ai] join_radius`, same proximity rule as opting into a fight). A dungeon
        // already in progress is not joinable later.
        self.place_in_dungeon(pid, key);
        let join_radius = self.balance.ai.join_radius;
        let mates: Vec<String> = self
            .arena
            .avatars
            .iter()
            .filter(|a| a.player_id != pid && a.state == "active")
            .map(|a| (a.player_id.clone(), a.position))
            .collect::<Vec<_>>()
            .into_iter()
            .filter(|(id, pos)| {
                self.dungeon_of(id).is_none() && pos.distance_to(&entrance_pos) <= join_radius
            })
            .map(|(id, _)| id)
            .collect();
        for m in mates {
            self.place_in_dungeon(&m, key);
        }
        // CL-1: the Shifter is the class whose senses are ABOUT dungeons, so the
        // first descent is what earns it — for everyone who walked in, not just
        // whoever pressed the key.
        let descended: Vec<String> = self
            .arena
            .avatars
            .iter()
            .map(|a| a.player_id.clone())
            .filter(|id| self.dungeon_of(id).map(|(k, _)| k) == Some(key))
            .collect();
        for id in descended {
            self.pending_effects.push(WorldEffect::Milestone {
                player_id: id,
                milestone: meld_proto::unlocks::Milestone::DungeonEntered,
            });
        }
        Vec::new()
    }

    /// Put `pid` inside dungeon `key` at its entrance: add them as an occupant, set
    /// their `Location`, and freeze their overworld avatar at the entry position
    /// (restored on exit — you return exactly where you came in).
    fn place_in_dungeon(&mut self, pid: &str, key: u64) {
        if let Some(d) = self.dungeons.get_mut(&key) {
            d.enter(pid);
        }
        self.location.insert(pid.to_string(), Location::InDungeon { key, floor: 0 });
        if let Some(a) = self.arena.avatar_mut(pid) {
            a.state = "in_dungeon".to_string();
        }
    }

    /// Per-intent step for dungeon movement — `speed × sim_dt`, matching the
    /// overworld so the feel is consistent.
    fn dungeon_step(&self, pid: &str) -> f64 {
        let hz = self.balance.world.overworld_sim_hz.max(1) as f64;
        let speed = self.arena.avatar(pid).map(|a| a.max_speed_tiles_per_sec).unwrap_or(4.0);
        speed / hz
    }

    /// Apply a move intent for a player inside a dungeon: slide-move on their floor,
    /// auto-activate any emitter reached (opens doors/gates), take a stair on
    /// contact, and exit to the overworld on reaching the end-exit.
    fn dungeon_move(&mut self, pid: &str, intent: &wm::MoveIntent) -> Vec<Outgoing> {
        let Some((key, floor)) = self.dungeon_of(pid) else {
            return Vec::new();
        };
        let step = self.dungeon_step(pid);
        // Move + activate + stair within one scoped borrow; capture the outcome.
        let (final_floor, final_pos, exiting, cell_changed) = {
            let Some(dj) = self.dungeons.get_mut(&key) else {
                return Vec::new();
            };
            let pre = dj.occupant(pid).map(|o| o.pos);
            let Some(newpos) = dj.try_move(pid, intent.move_dir.x, intent.move_dir.y, step) else {
                return Vec::new();
            };
            dj.activate_at(floor, newpos); // reaching a lever/plate/key/boss opens gated doors
            // `_for` rather than by position: a stair delivers you onto its partner,
            // which is itself a stair, so the plain lookup re-fires it and bounces the
            // player between floors forever.
            let stair = dj.stair_dest_for(pid);
            if let Some((df, dp)) = stair {
                dj.take_stair(pid, df, dp);
            }
            let (ff, fp) = stair.unwrap_or((floor, newpos));
            // Fire an armed trap only on ENTERING a new cell (not while lingering).
            let changed = stair.is_some()
                || pre.is_none_or(|p| (p.x.floor() as i64, p.y.floor() as i64) != (fp.x.floor() as i64, fp.y.floor() as i64));
            // `_for` so the entrance you are standing on the instant you arrive does
            // not throw you straight back out.
            (ff, fp, dj.at_exit_for(pid), changed)
        };
        if final_floor != floor {
            self.location.insert(pid.to_string(), Location::InDungeon { key, floor: final_floor });
        }
        if exiting {
            return self.exit_dungeon(pid, key);
        }
        // DG-3b(3/n): entering the boss's cell starts the boss fight (once, until
        // it's dead — its gated chest unlocks on victory). Guarded to cell-entry.
        if cell_changed {
            let boss = self.dungeons.get(&key).and_then(|d| {
                let id = d.object_at(final_floor, final_pos)?.clone();
                match d.def().objects.get(&id) {
                    Some(ObjectKind::Boss { sprite, .. }) if !d.is_active(&id) => Some((id, sprite.clone())),
                    _ => None,
                }
            });
            if let Some((boss_id, sprite)) = boss {
                let (biome, eff) = {
                    let d = &self.dungeons[&key];
                    (d.def().biome.to_string(), d.effective_distance(final_floor))
                };
                return self.start_dungeon_battle(pid, key, &boss_id, &sprite, &biome, eff);
            }
        }
        // DG-3b(3/n): an armed trap on the newly-entered cell fires (DG-4a). Damage is
        // scaled to the dungeon's stamped distance and applied to the party; a wipe
        // ends the run in death (back to town, backpack lost).
        let trap_hit = if cell_changed {
            self.dungeons.get(&key).and_then(|d| d.spring_trap(final_floor, final_pos))
        } else {
            None
        };
        if let Some(hit) = trap_hit {
            return self.apply_trap_hit(pid, &hit);
        }
        Vec::new()
    }

    /// Convert a sprung [`TrapHit`] to HP damage on the stepping player's party and
    /// apply it; a full wipe kills the run (design §5). `severity` (the floor's
    /// effective distance) scales the base `[worldgen] dungeon_trap_damage`.
    fn apply_trap_hit(&mut self, pid: &str, hit: &TrapHit) -> Vec<Outgoing> {
        let base = self.balance.worldgen.dungeon_trap_damage as f64;
        let div = self.balance.world_scaling.stat_mult_base_divisor.max(1.0);
        let dmg = (base * (1.0 + hit.severity as f64 / div)).round() as i32;
        // Who the trap put DOWN, not merely who it hurt — each of those owes the
        // durability tax on its own kit (GR-2), whether or not the party survived.
        let mut fell: Vec<(String, i32)> = Vec::new();
        let wiped = match self.hero_hp.get_mut(pid) {
            Some(hp) => {
                for (slot, h) in hp.iter_mut().enumerate() {
                    if *h > 0 {
                        *h = (*h - dmg).max(0);
                        if *h == 0 {
                            fell.push((pid.to_string(), slot as i32));
                        }
                    }
                }
                hp.iter().all(|h| *h <= 0)
            }
            None => return Vec::new(),
        };
        self.charge_non_battle_falls(&fell);
        if wiped {
            self.world_death(pid)
        } else {
            Vec::new()
        }
    }

    /// End `pid`'s run in death with no battle to end — a sprung dungeon trap, or the
    /// Force blast of a Shift that landed on them (CANON §W2): mirror the
    /// battle-defeat arm for one player — drop them from the dungeon + overworld,
    /// clear the run's haul, report `MemberResult { died }`, and queue the death
    /// durability sink. The Router then releases the session (see `handle_move`).
    fn world_death(&mut self, pid: &str) -> Vec<Outgoing> {
        if let Some((key, _)) = self.dungeon_of(pid) {
            if let Some(d) = self.dungeons.get_mut(&key) {
                d.remove(pid);
                if d.is_empty() {
                    self.dungeons.remove(&key);
                }
            }
        }
        self.location.remove(pid);
        self.arena.avatars.retain(|a| a.player_id != pid);
        let mut out = Vec::new();
        let charged = self.durability_charged.contains(pid);
        if let Some(r) = self.run.runs.iter_mut().find(|r| r.player_id == pid) {
            let mut lost = r.backpack.clone();
            lost.extend(r.pouches.iter().flatten().cloned());
            let (run_id, lost_chits) = (r.run_id.clone(), r.chits);
            r.result = Some(RunResult::Died);
            r.backpack.clear();
            for pouch in r.pouches.iter_mut() {
                pouch.clear();
            }
            r.looted_gear.clear();
            r.chits = 0;
            out.push(out_msg(
                pid,
                &wr::MemberResult {
                    run_id,
                    player_id: pid.to_string(),
                    result: RunResult::Died,
                    max_distance_reached: 0,
                    banked: None,
                    lost: Some(lost),
                    chits: lost_chits,
                    gear_banked: vec![],
                    durability_loss_applied: charged,
                },
            ));
        }
        let _ = self.db_writes.send(DbWrite::Death(pid.to_string()));
        out
    }

    /// Charge the durability tax for heroes a NON-BATTLE blow just put down (GR-2):
    /// a dungeon trap, or the Force blast of a Shift. Those are the only two ways a
    /// hero dies with no battle to end, so there is no engine fall counter to read —
    /// but it is the same tax, and it goes through the same write for the same reason
    /// the engine counts falls in one place.
    fn charge_non_battle_falls(&mut self, fell: &[(String, i32)]) {
        for (pid, slot) in fell {
            let _ = self.db_writes.send(DbWrite::HeroFalls(pid.clone(), *slot, 1));
            self.durability_charged.insert(pid.clone());
        }
    }

    /// Whether `pid`'s run has ended (result recorded) — the Router uses this after
    /// a dungeon move to release a player who just died to a trap.
    fn run_ended(&self, pid: &str) -> bool {
        self.run.runs.iter().any(|r| r.player_id == pid && r.result.is_some())
    }

    /// Leave a dungeon: drop the occupant (despawning the instance if now empty —
    /// per-entry fresh), clear the location, and un-freeze the overworld avatar,
    /// which is still parked at the entry position.
    fn exit_dungeon(&mut self, pid: &str, key: u64) -> Vec<Outgoing> {
        if let Some(d) = self.dungeons.get_mut(&key) {
            d.remove(pid);
            if d.is_empty() {
                self.dungeons.remove(&key);
            }
        }
        self.location.remove(pid);
        if let Some(a) = self.arena.avatar_mut(pid) {
            a.state = "active".to_string();
        }
        Vec::new()
    }

    /// DG-6b: the `world.dungeon_scene` re-skin cue for `pid`, emitted only when it
    /// changed since we last told them — on descend (`active`, with the floor's biome
    /// theme + grid bounds), on a floor change, and on exit/death (`!active`). Returns
    /// `None` when unchanged (the common per-tick case). Purely presentational: it
    /// lets the client swap ground/sky and ring the play area with a biome enclosure
    /// so no overworld shows; the authoritative floor is still the `Snapshot` walls.
    fn dungeon_scene_delta(&mut self, pid: &str) -> Option<Outgoing> {
        let desired: (bool, usize) = match self.dungeon_of(pid) {
            Some((_, floor)) => (true, floor),
            None => (false, 0),
        };
        // First time we've seen this player AND they're in the overworld ⇒ the client
        // already looks like the overworld; say nothing.
        let last = self.dungeon_scene_sent.get(pid).copied();
        if last == Some(desired) || (last.is_none() && !desired.0) {
            return None;
        }
        self.dungeon_scene_sent.insert(pid.to_string(), desired);
        let (theme, width, height) = if desired.0 {
            self.dungeon_of(pid)
                .and_then(|(key, floor)| {
                    let def = self.dungeons.get(&key)?.def();
                    let g = def.grids.get(floor)?;
                    Some((def.biome.clone(), g.width as u32, g.height as u32))
                })
                .unwrap_or_default()
        } else {
            (String::new(), 0, 0)
        };
        Some(out_msg(
            pid,
            &ww::DungeonScene { active: desired.0, theme, floor: desired.1 as u32, width, height },
        ))
    }

    /// Build the snapshot for a player inside a dungeon floor. The floor is mapped
    /// onto existing entity tags so the client shows the space: occupants as avatars,
    /// walls + closed doors as obstacles, the end-exit as a portal, chests and the
    /// boss as their usual tags. The client re-skins the surround from the paired
    /// `world.dungeon_scene` cue (see `dungeon_scene_delta`). No interest cull (a
    /// floor is small).
    fn dungeon_snapshot(&self, pid: &str, key: u64, floor: usize, server_tick: i64) -> Outgoing {
        let mut entities: Vec<wm::SnapshotEntity> = Vec::new();
        // Shifter trap sense: armed traps within the Runner's radius are revealed.
        // Server-side, because whether a trap is armed is authoritative state and a
        // client that could see every trap by asking would be a client that cheats.
        let trap_radius = self.perks_for(pid).shifter_trap_radius as f64;
        let sensed_from = self
            .dungeons
            .get(&key)
            .and_then(|d| d.occupants().find(|(o, _)| *o == pid).map(|(_, occ)| occ.pos));
        if let Some(d) = self.dungeons.get(&key) {
            let def = d.def();
            for (opid, occ) in d.occupants() {
                if occ.floor != floor {
                    continue;
                }
                entities.push(wm::SnapshotEntity {
                    entity_id: opid.clone(),
                    position: occ.pos,
                    velocity: wm::Velocity { x: 0.0, y: 0.0 },
                    avatar_state: Some("active".to_string()),
                    level: Some(0),
                    ..Default::default()
                });
            }
            if let Some(grid) = def.grids.get(floor) {
                for y in 0..grid.height {
                    for x in 0..grid.width {
                        let cell = grid.at(x, y);
                        let pos = Position::new(x as f64 + 0.5, y as f64 + 0.5);
                        if cell.tile == Tile::Wall {
                            entities.push(dungeon_prop(format!("dwall-{floor}-{x}-{y}"), pos, "obstacle:dungeon_wall:0.5"));
                            continue;
                        }
                        let Some(id) = &cell.object else { continue };
                        match def.objects.get(id) {
                            Some(k) if k.is_barrier() && !d.is_open(id) => {
                                entities.push(dungeon_prop(format!("ddoor-{id}"), pos, "obstacle:dungeon_door:0.5"));
                            }
                            Some(ObjectKind::Chest { .. }) => {
                                entities.push(dungeon_prop(format!("dchest-{id}"), pos, "chest:1:0"));
                            }
                            Some(ObjectKind::Boss { sprite, .. }) => {
                                entities.push(dungeon_prop(format!("dboss-{id}"), pos, &format!("mob:{sprite}:hostile")));
                            }
                            // Stairs were never sent, so nothing downstream could see
                            // them: not the client, and not a player trying to find
                            // the way down. A floor's exit being invisible is a bug
                            // whether a human or a bot is looking for it.
                            Some(ObjectKind::Stair) => {
                                entities.push(dungeon_prop(
                                    format!("dstair-{floor}-{id}"),
                                    pos,
                                    "stair",
                                ));
                            }
                            // An ARMED trap the Runner can read from here. A disarmed
                            // one is furniture and stays unmarked.
                            Some(ObjectKind::Trap { kind, .. })
                                if trap_radius > 0.0
                                    && d.trap_state(id) == Some(meld_dungeon_run::TrapState::Armed)
                                    && sensed_from.is_some_and(|from| {
                                        from.distance_to(&pos) <= trap_radius
                                    }) =>
                            {
                                entities.push(dungeon_prop(
                                    format!("dtrap-{id}"),
                                    pos,
                                    &format!("trap:{kind}"),
                                ));
                            }
                            _ => {}
                        }
                    }
                }
                // Both KINDS of way out, because `DungeonInstance::at_exit` accepts
                // both: the authored far exit, and the door you walked in through.
                // The entrance was never drawn, so the one exit that is always
                // reachable — the whole point of it, since a dungeon refuses a Town
                // Portal — was invisible, and a player who lost the thread had no
                // marked way back to it.
                for (n, e) in def
                    .exits
                    .iter()
                    .chain(def.entrances.iter())
                                        .filter(|e| e.floor == floor)
                    .enumerate()
                {
                    let pos = Position::new(e.x as f64 + 0.5, e.y as f64 + 0.5);
                    entities.push(dungeon_prop(format!("dexit-{floor}-{n}"), pos, "portal"));
                }
            }
        }
        out_msg(pid, &wm::Snapshot { server_tick, entities })
    }

    /// The caller's hero roster (name/class/level/attributes) for the party panel.
    /// Reuses `party_fighters` so the stats match combat exactly.
    /// AD-2: the class-pair synergies this player's comp has active and the combos
    /// it can run, described for the party screen. Empty when the comp is unknown
    /// (no run in flight) — the client just shows nothing.
    fn party_depth(&self, pid: &str) -> (Vec<wr::SynergyView>, Vec<wr::ComboView>) {
        use meld_proto::synergies::{self as syn, SynergyEffect as E};
        let Some(comp) = self.party_classes.get(pid) else {
            return (Vec::new(), Vec::new());
        };
        let adv = &self.balance.adventure;
        let synergies = syn::active_synergies(comp)
            .into_iter()
            .map(|s| wr::SynergyView {
                key: s.key.to_string(),
                name: s.name.to_string(),
                description: s.description.to_string(),
                effect: match s.effect {
                    E::PartyBarrier => format!(
                        "every hero opens each fight with Barrier worth {:.0}% of its own HP",
                        adv.synergy_party_barrier_fraction * 100.0
                    ),
                    E::PartyRegen => format!(
                        "every hero regenerates {:.1}% of its own HP a turn",
                        adv.synergy_party_regen_fraction * 100.0
                    ),
                    E::BackRowEvasion => format!(
                        "back-row heroes gain {}% Evasion",
                        adv.synergy_back_row_evasion
                    ),
                },
            })
            .collect();
        let combos = syn::available_combos(comp)
            .into_iter()
            .map(|c| wr::ComboView {
                key: c.key.to_string(),
                name: c.name.to_string(),
                sequence: format!(
                    "{} ({}) then {} ({})",
                    meld_proto::skills::pretty_skill(c.setup),
                    syn::pretty_class_name(c.setup_class),
                    meld_proto::skills::pretty_skill(c.payoff),
                    syn::pretty_class_name(c.payoff_class),
                ),
                description: c.description.to_string(),
                bonus_pct: ((c.damage_mult - 1.0) * 100.0).round() as i32,
            })
            .collect();
        (synergies, combos)
    }

    /// Every ability this player's classes can hold, with its magnitudes resolved from
    /// balance. The registry's prose is shared with the client, but the NUMBERS are
    /// `[TUNABLE]`s the client cannot read — so a row could say "Spends Adrenaline" and
    /// never how much, which is the only question that decides between two rows. Sent
    /// with the roster so the battle menu and the abilities panel read the same line.
    fn party_ability_views(&self, pid: &str) -> Vec<wr::AbilityView> {
        let Some(comp) = self.party_classes.get(pid) else {
            return Vec::new();
        };
        let mut out: Vec<wr::AbilityView> = Vec::new();
        for class in comp {
            for def in meld_proto::skills::skills_for_class(meld_run::class_key(*class)) {
                if out.iter().any(|a| a.key == def.key) {
                    continue;
                }
                let effect = meld_run::ability_effects::effect_line(def.key, &self.balance);
                if !effect.is_empty() {
                    let adrenaline_cost =
                        meld_run::ability_effects::adrenaline_cost(def.key, &self.balance);
                    out.push(wr::AbilityView { key: def.key.to_string(), effect, adrenaline_cost });
                }
            }
        }
        out
    }

    fn party_views(&self, pid: &str) -> Vec<wr::HeroView> {
        let inst = self;
        let Some(comp) = inst.party_classes.get(pid).cloned() else {
            return Vec::new();
        };
        let names = inst.hero_names.get(pid).cloned().unwrap_or_default();
        let rows = inst.hero_rows.get(pid).cloned().unwrap_or_default();
        // Reflect each hero's own equipped gear (Vault baseline + any run-loot
        // worn this run) so the party panel matches combat. Sourced from the world's
        // own synced mirror of the session gear (see `WorldActor::gear_bonuses`).
        let hero_bonuses = self.gear_bonuses.get(pid);
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
                let b = hero_bonuses.and_then(|v| v.get(slot)).cloned().unwrap_or_default();
                (
                    pid.to_string(),
                    String::new(),
                    *c,
                    effective_gear_bonus(b, looted, slot as i32, self.edge_for(pid, slot)),
                )
            })
            .collect();
        let row_overrides: Vec<Option<bool>> = rows.iter().map(|r| Some(*r)).collect();
        let fighters = meld_run::party_fighters(&party, &inst.run, &self.balance, &row_overrides);
        // Current (possibly wounded) HP persists across battles within a run —
        // `hero_hp` is the live source; a missing slot (not yet in a battle
        // this run) reads as full.
        let hp_now = inst.hero_hp.get(pid).cloned().unwrap_or_default();
        // Each hero carries its OWN banked XP and its OWN next-level bar. These used to
        // be one shared pair off the run, so the encounter split was invisible: four
        // heroes sharing a pool reported the same number a lone hero did, and the whole
        // point of the split could not be seen from inside the game.
        let run = inst.run.runs.iter().find(|r| r.player_id == pid);
        let hero_xp = run.map(|r| r.hero_xp.clone()).unwrap_or_default();
        // What still has hold of each hero, so the client can show it on the road as well as
        // in the arena — afflictions do not expire, so out here is where most of them are felt.
        let carried = inst.hero_afflictions.get(pid).cloned().unwrap_or_default();
        fighters
            .iter()
            .enumerate()
            .map(|(slot, f)| wr::HeroView {
                afflictions: carried.get(slot).cloned().unwrap_or_default(),
                slot: slot as i32,
                name: names
                    .get(slot)
                    .cloned()
                    .unwrap_or_else(|| generated_hero_name(pid, slot)),
                class_key: f.class_key.clone(),
                level: f.level,
                str_: f.str_,
                mnd: f.mnd,
                dex: f.dex,
                wll: f.wll,
                max_hp: f.max_hp,
                xp: hero_xp.get(slot).copied().unwrap_or(0),
                xp_to_next: meld_run::xp_to_next(f.level, &self.balance),
                hp: hp_now.get(slot).copied().unwrap_or(f.max_hp).clamp(0, f.max_hp),
                back_row: f.back_row,
            })
            .collect()
    }

    /// Overworld regen: restore carried hero HP over time while a party roams (not in
    /// battle). Regen is HP/sec but `hero_hp` is integer, so the sub-1 remainder is
    /// banked in `regen_accum` and whole HP is applied as it accrues, most-wounded
    /// living hero first (downed heroes at 0 HP are not revived — that needs a real
    /// fight). Purely server state.
    ///
    /// **Two sources, and they reach different people.** A Resonant's walking regen
    /// tends only the Resonants themselves: poured over the whole party it mended every
    /// wound between fights, so a party that brought a healer never needed healing and
    /// the class's own kit — the thing it is best in the game at — went unspent. A
    /// Keeper's alembic field still reaches everyone standing in it, because a field is
    /// a PLACE you choose to stand rather than a passive you get for bringing someone.
    fn apply_overworld_regen(&mut self, dt: f64) {
        // Plan first (shared borrow), then mutate hero_hp (exclusive borrow).
        let plans: Vec<RegenPlan> = {
            let in_battle = self.parties_in_battle();
            let mut v = Vec::new();
            for r in &self.run.runs {
                if in_battle.contains(&r.party_id) {
                    continue;
                }
                let own = self.perks_for(&r.player_id).resonant_regen;
                let field = self.alembic_field_regen(&r.player_id);
                if own <= 0.0 && field <= 0.0 {
                    continue;
                }
                let party = self.party_views(&r.player_id);
                v.push(RegenPlan {
                    player_id: r.player_id.clone(),
                    own,
                    field,
                    caps: party.iter().map(|h| h.max_hp).collect(),
                    healers: party
                        .iter()
                        .enumerate()
                        .filter(|(_, h)| h.class_key == meld_run::class_key(CharacterClass::Resonant))
                        .map(|(i, _)| i)
                        .collect(),
                });
            }
            v
        };
        if plans.is_empty() {
            return;
        }
        for plan in plans {
            let acc = self.regen_accum.entry(plan.player_id.clone()).or_default();
            acc.own += plan.own * dt as f32;
            acc.field += plan.field * dt as f32;
            let own_budget = take_whole(&mut acc.own);
            let field_budget = take_whole(&mut acc.field);
            if own_budget == 0 && field_budget == 0 {
                continue;
            }
            let Some(hps) = self.hero_hp.get_mut(&plan.player_id) else {
                continue;
            };
            pour_regen(hps, &plan.caps, Some(&plan.healers), own_budget);
            pour_regen(hps, &plan.caps, None, field_budget);
        }
    }

    /// The regen a live alembic is pouring over this player right now. A field is a
    /// PLACE: it only reaches `alembic_field_radius`, only on its own level, and only
    /// while the still still has brews in it — a spent bench is cold.
    fn alembic_field_regen(&self, player_id: &str) -> f32 {
        let Some(a) = self.arena.avatar(player_id) else { return 0.0 };
        let f = &self.balance.forge;
        // A Keeper deep enough puts ROOTS under its still: the field reaches further and
        // heals harder, which is the only rest a party without a Resonant gets.
        let perks = self.perks_for(player_id);
        let reach = f.alembic_field_radius * perks.keeper_field_radius_mult as f64;
        let warm = self.arena.stations.iter().any(|s| {
            s.kind == "alembic"
                && !s.spent()
                && s.elevation == a.elevation
                && a.position.distance_to(&s.position) <= reach
        });
        if warm {
            f.alembic_regen_per_sec * perks.keeper_field_regen_mult
        } else {
            0.0
        }
    }

    /// Per-hero stat gains for a party level-up (old_level → new_level), for the
    /// classic JRPG "LEVEL UP!" screen. Mirrors the `party_fighters` derivation
    /// (max HP from Wll; the four attributes from `attributes_at`) so the numbers
    /// exactly match the party panel.
    fn hero_level_ups(&self, pid: &str, old_level: i32, new_level: i32) -> Vec<wr::HeroLevelUp> {
        let inst = self;
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
                .unwrap_or_else(|| b.player.get("explorer").expect("explorer stats"));
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
                        .unwrap_or_else(|| generated_hero_name(pid, slot)),
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

    /// Start a fresh battle for every active avatar currently in contact with a free
    /// creature. Loops because several players can be touched in the same tick;
    /// `start_battle` flips the toucher's avatar and its creature to `in_battle`, so
    /// each pass resolves a distinct contact and the loop drains in ≤ (avatars)
    /// passes. Independent battles run concurrently — one party's fight never blocks
    /// another's — and teammates still opt into an *ongoing* fight via `join_battle`.
    fn resolve_touches(&mut self) -> Vec<Outgoing> {
        let mut out = Vec::new();
        // Players who just walked out of a battle (win, loss, or flee) sit out a short
        // grace window before they can be touched into another one — see
        // `battle_immune_until`. Expired entries are dropped here so the map doesn't
        // grow forever.
        let now = now_ms();
        self.battle_immune_until.retain(|_, until| *until > now);
        let immune: std::collections::HashSet<String> =
            self.battle_immune_until.keys().cloned().collect();
        let max_passes = self.arena.avatars.len();
        for _ in 0..max_passes {
            let decision = self.arena.check_touch(&immune).and_then(|(toucher, monster_idx)| {
                self.run
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
        // A fight breaks whatever the toucher was channeling. Without this an
        // in-flight extraction completes *during* the battle and banks the backpack,
        // which is a free escape past the flee cost (`flee_chit_loss_fraction` /
        // `flee_item_drop_chance`) — and `handle_begin_extraction` already refuses to
        // START one mid-battle, so surviving into a battle was never intended. The
        // Town Portal is only consumed on completion, so an interrupted one is kept.
        let mut broke = self.end_harvest(toucher, "battle_started");
        broke.extend(self.end_building(toucher, "battle_started"));
        if self.extraction.remove(toucher).is_some() {
            if let Some(a) = self.arena.avatar_mut(toucher) {
                if a.state == "channeling" {
                    a.state = "active".to_string();
                }
            }
            let members: Vec<String> = self.run.runs.iter().map(|r| r.player_id.clone()).collect();
            broke.extend(members.iter().map(|pid| {
                out_msg(
                    pid,
                    &wr::ChannelInterrupted {
                        player_id: toucher.to_string(),
                        reason: "battle_started".to_string(),
                    },
                )
            }));
        }
        let seed = now_ms();
        let balance = self.balance.clone();
        // Snapshot the world's own synced gear mirror before the mutable reborrow —
        // behaviour-identical to the old per-tick session read (see `gear_bonuses`).
        let bonuses = self.gear_bonuses.clone();
        let edges = self.edges.clone();
        let inst = &mut *self;

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
                let vault_bonus = hero_bonuses.and_then(|v| v.get(slot)).cloned().unwrap_or_default();
                let bonus = effective_gear_bonus(
                    vault_bonus,
                    &r.looted_gear,
                    slot as i32,
                    edges.get(&r.player_id).and_then(|v| v.get(slot)),
                );
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
        // The Vanguard board reports HOW a run got deep, so an encounter is counted the
        // moment it is assembled — not on victory, because a fight you fled was still a
        // fight you took.
        for r in inst.run.runs.iter_mut() {
            if r.player_id == toucher {
                r.fights += 1;
            }
        }
        // A pinned creature is the whole point of the pin: the party chose the moment, so
        // it opens with every gauge full. Read off the creature that was actually TOUCHED,
        // not the group — pinning one of a pack does not surprise the pack.
        let surprise = inst.arena.monsters[monster_idx].held_for > 0.0;
        let mut battle = build_battle(
            battle_id.clone(),
            &party,
            &enemies_ref,
            &inst.run,
            &balance,
            seed,
            &hp_overrides,
            &row_overrides,
            surprise,
        );
        // Whatever still had hold of a hero when the last fight ended has hold of it now.
        // Afflictions do not expire, so walking away from the creature that poisoned you is
        // not a cure — a `Fighter` is rebuilt per battle, so the run is what remembers.
        for (pid, cids) in &player_combatants {
            if let Some(carried) = inst.hero_afflictions.get(pid) {
                for (slot, cid) in cids.iter().enumerate() {
                    for name in carried.get(slot).into_iter().flatten() {
                        battle.afflict(cid, name);
                    }
                }
            }
        }
        // Store the group's stable ids (indices are only valid until the next prune).
        let monster_ids: Vec<String> = group_idxs
            .iter()
            .filter_map(|&gi| inst.arena.monsters.get(gi).map(|m| m.entity_id.clone()))
            .collect();
        let slot = BattleSlot {
            battle,
            battle_id: battle_id.clone(),
            monster_ids,
            // Built from the SAME list the enemy fighters were, so the two cannot drift.
            monster_combatants: enemy_members
                .iter()
                .map(|(m, cid)| (m.entity_id.clone(), cid.clone()))
                .collect(),
            combatant_player,
            player_combatants,
            parties: std::iter::once(party_id).collect(),
            spectators: std::collections::HashSet::new(),
            pos: inst
                .arena
                .monsters
                .get(monster_idx)
                .map(|m| m.position)
                .unwrap_or_else(|| Position::new(0.0, 0.0)),
            dungeon: None,
            party_scale: meld_run::encounter_party_scale(party.len(), &balance),
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

        let mut out = broke;
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
                    spectating: false,
                },
            ));
        }
        inst.battles.push(slot);
        out
    }

    /// DG-3b(3/n): start a boss fight inside a dungeon. The triggering player's party
    /// faces the authored boss (scaled to the dungeon's stamped distance, FS-4 boss
    /// mechanics via `boss_kind`). Tagged with dungeon context so `finish_dungeon_battle`
    /// unlocks the boss-gated chest on victory / cleans up on defeat.
    fn start_dungeon_battle(
        &mut self,
        pid: &str,
        key: u64,
        boss_id: &str,
        boss_kind: &str,
        biome: &str,
        eff_dist: i64,
    ) -> Vec<Outgoing> {
        let seed = now_ms();
        let balance = self.balance.clone();
        let bonuses = self.gear_bonuses.clone();
        let edges = self.edges.clone();
        let Some(party_id) = self.party_id_of(pid) else {
            return Vec::new();
        };
        let inst = &mut *self;
        let battle_id = Uuid::now_v7().to_string();
        let boss_cid = Uuid::now_v7().to_string();

        let party_players: Vec<String> = inst
            .run
            .runs
            .iter()
            .filter(|r| r.party_id == party_id)
            .map(|r| r.player_id.clone())
            .collect();
        let mut party: Vec<meld_run::PartyMember> = Vec::new();
        let mut combatant_player: HashMap<String, String> = HashMap::new();
        let mut player_combatants: HashMap<String, Vec<String>> = HashMap::new();
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
                let vault_bonus = hero_bonuses.and_then(|v| v.get(slot)).cloned().unwrap_or_default();
                let bonus = effective_gear_bonus(
                    vault_bonus,
                    &r.looted_gear,
                    slot as i32,
                    edges.get(&r.player_id).and_then(|v| v.get(slot)),
                );
                party.push((r.player_id.clone(), cid.clone(), *cls, bonus));
                hp_overrides.push(hp_vec.get(slot).copied());
                row_overrides.push(row_vec.get(slot).copied());
                cids.push(cid);
            }
            player_combatants.insert(r.player_id.clone(), cids);
        }

        let boss_entity = format!("dboss-{key}-{boss_id}");
        // AD-4: a bounty whose venue is a DESCENT waits at the bottom of one. If this
        // player holds such a contract and the door is deep enough for it, the thing
        // keeping the door IS their mark — built from the contract, so the fight it names
        // is the fight they get.
        let mark = inst
            .bounties
            .get(pid)
            .into_iter()
            .flatten()
            .find(|(id, spec)| {
                spec.venue == meld_proto::bounties::Venue::Dungeon
                    && !inst.marks_placed.contains(id)
                    && eff_dist >= spec.distance as i64
            })
            .map(|(id, spec)| (id.clone(), spec.clone()));
        let boss = match &mark {
            Some((id, spec)) => meld_world::MonsterSpawn::bounty_mark_at(
                &balance,
                boss_entity,
                spec,
                id,
                pid,
                eff_dist,
                seed,
            ),
            None => meld_world::MonsterSpawn::dungeon_boss(
                &balance,
                boss_entity,
                biome,
                boss_kind,
                eff_dist,
                seed,
            ),
        };
        if let Some((id, _)) = &mark {
            inst.marks_placed.insert(id.clone());
        }
        let enemies_ref: Vec<(&meld_world::MonsterSpawn, String)> = vec![(&boss, boss_cid.clone())];
        let battle = build_battle(
            battle_id.clone(),
            &party,
            &enemies_ref,
            &inst.run,
            &balance,
            seed,
            &hp_overrides,
            &row_overrides,
            // Joining a fight already in progress is not a surprise: the moment was
            // chosen by whoever started it.
            false,
        );
        let slot = BattleSlot {
            battle,
            battle_id: battle_id.clone(),
            monster_ids: vec![],
            // A dungeon boss is built in the battle rather than standing in the arena, so
            // there is no overworld creature to carry its wound back to.
            monster_combatants: HashMap::new(),
            combatant_player,
            player_combatants,
            parties: std::iter::once(party_id).collect(),
            spectators: std::collections::HashSet::new(),
            pos: Position::new(0.0, 0.0),
            dungeon: Some(DungeonBattle {
                key,
                boss_id: boss_id.to_string(),
                bounty: mark.as_ref().map(|(id, _)| id.clone()).unwrap_or_default(),
                mark_boss: mark.as_ref().map(|(_, s)| s.boss_kind.clone()).unwrap_or_default(),
            }),
            party_scale: meld_run::encounter_party_scale(party.len(), &balance),
        };
        let (mut allies, enemies) = slot.battle.wire_combatants();
        inject_hero_names(&slot.player_combatants, &inst.hero_names, &mut allies);
        for p in &party_players {
            if let Some(a) = inst.arena.avatar_mut(p) {
                a.state = "in_battle".to_string();
            }
        }
        let encounter_class = slot.battle.encounter_class;
        let mut out = Vec::new();
        for p in &party_players {
            let yours = slot.player_combatants.get(p).cloned().unwrap_or_default();
            out.push(out_msg(
                p,
                &wb::Started {
                    battle_id: battle_id.clone(),
                    encounter_class,
                    allies: allies.clone(),
                    enemies: enemies.clone(),
                    your_combatant_id: yours.first().cloned().unwrap_or_default(),
                    your_combatant_ids: yours,
                    triggered_by: Some(pid.to_string()),
                    spectating: false,
                },
            ));
        }
        inst.battles.push(slot);
        out
    }

    /// DG-3b(3/n): fix up dungeon state after a boss battle ends. Victory marks the
    /// boss dead (unlocking its gated chest) and returns survivors to the dungeon;
    /// defeat (a wipe) — the run already ended in `handle_battle_end` — clears the
    /// dead player's dungeon occupancy; fleeing drops them back into the dungeon.
    fn finish_dungeon_battle(&mut self, members: &[String], outcome: BattleOutcome, d: DungeonBattle) -> Vec<Outgoing> {
        match outcome {
            BattleOutcome::Victory => {
                if let Some(dj) = self.dungeons.get_mut(&d.key) {
                    dj.activate(&d.boss_id); // boss_dead(<id>) → the vault unlocks
                }
                for pid in members {
                    if self.dungeon_of(pid).is_some() {
                        if let Some(a) = self.arena.avatar_mut(pid) {
                            a.state = "in_dungeon".to_string();
                        }
                    }
                }
            }
            BattleOutcome::Defeat => {
                for pid in members {
                    if let Some((key, _)) = self.dungeon_of(pid) {
                        if let Some(dj) = self.dungeons.get_mut(&key) {
                            dj.remove(pid);
                            if dj.is_empty() {
                                self.dungeons.remove(&key);
                            }
                        }
                    }
                    self.location.remove(pid);
                }
            }
            BattleOutcome::Fled => {
                for pid in members {
                    if self.dungeon_of(pid).is_some() {
                        if let Some(a) = self.arena.avatar_mut(pid) {
                            a.state = "in_dungeon".to_string();
                        }
                    }
                }
            }
        }
        Vec::new()
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
    world: Option<WorldActor>,
    /// A world read back from Postgres at boot and not yet stood up. `form_run` claims
    /// it the first time anybody dives: the world is only *built* in one place, and
    /// restoring is that build reading its seed and its delta from disk instead of from
    /// a fresh roll.
    restore: Option<meld_db::WorldSave>,
    /// The world's tick at the last hibernate, so the save cadence is measured in world
    /// time rather than in how long the process happens to have been up.
    last_world_save: u64,
    /// Open co-op lobbies, keyed by join code.
    lobbies: HashMap<String, Lobby>,
    /// player_id -> the lobby code they're in.
    player_lobby: HashMap<String, String>,
    /// Players whose gear bonus needs (re)loading from the DB (post-connect).
    /// Loads feed session state back, so they stay on the loop (they only await
    /// Postgres when a player actually connects — infrequent, not per-tick).
    pending_gear_load: Vec<String>,
    /// Players whose persistent Meld skill levels the world still needs (MS-1 field
    /// stations gate on them). Drained by `flush_skill_loads` after the tick.
    pending_skill_load: Vec<String>,
    /// Players whose standing bounty contracts (AD-4) still have to be read out of the DB
    /// and handed to the world, so their marks can be stood up.
    pending_bounty_load: Vec<String>,
    /// Open heats (MS-1's smithing tempo game), keyed by job id. A heat holds the bar
    /// the server laid out and the blows reported so far; it leaves here graded, either
    /// when the last blow lands or when its window runs out.
    open_heats: HashMap<String, OpenHeat>,
    /// Graded smith jobs waiting for their Postgres half. Drained after the tick by
    /// `flush_smith_jobs`, so the loop never parks on a round-trip.
    pending_smith: Vec<SmithJob>,
    /// Monotonic source of job ids.
    next_job: u64,
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
            world: None,
            restore: None,
            last_world_save: 0,
            lobbies: HashMap::new(),
            player_lobby: HashMap::new(),
            pending_gear_load: Vec::new(),
            pending_skill_load: Vec::new(),
            pending_bounty_load: Vec::new(),
            open_heats: HashMap::new(),
            pending_smith: Vec::new(),
            next_job: 0,
            pending_hero_load: Vec::new(),
            db_writes,
        }
    }

    async fn run(mut self, mut rx: mpsc::Receiver<ServerEvent>) {
        // CANON §W5: a hibernated world reloads on first joiner. Read at boot rather than
        // lazily, because this is the one moment the loop is allowed to await Postgres —
        // once the tick is running, every DB touch goes down `db_writes`.
        if self.balance.world_persist.enabled {
            match self.db.load_world(WORLD_KEY).await {
                Ok(Some(save)) => {
                    tracing::info!(
                        seed = save.seed,
                        tick = save.tick_count,
                        generation = save.shift_generation,
                        sections = save.sections,
                        "world.persist: a world was waiting"
                    );
                    self.restore = Some(save);
                }
                Ok(None) => {}
                Err(e) => tracing::error!("world.persist: load failed, seeding fresh: {e}"),
            }
        }
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
                    let (out, effects) = self.world.as_mut().map(|w| w.tick()).unwrap_or_default();
                    self.apply_world_effects(effects);
                    self.dispatch(out);
                }
            }
            // Async DB side-effects that feed session/run state back run after
            // either arm (they only await Postgres when there is pending work —
            // a fresh connect or a completed extraction, not every tick). Deaths,
            // harvest XP and renames are fire-and-forget and go to `run_db_writer`
            // off this task, so the tick never blocks on those round-trips.
            self.expire_heats();
            self.flush_gear_loads().await;
            self.flush_skill_loads().await;
            self.flush_smith_jobs().await;
            self.flush_hero_loads().await;
            self.flush_bounty_loads().await;
            let banked = self.complete_extractions().await;
            self.dispatch(banked);
            if let Some(w) = self.world.as_mut() {
                let harvested = w.advance_harvests();
                self.dispatch(harvested);
            }
            if let Some(w) = self.world.as_mut() {
                let built = w.advance_building();
                self.dispatch(built);
            }
            self.hibernate_world();
        }
    }

    /// Write the world's delta out on its own cadence (`[world_persist]
    /// save_every_ticks`). Measured in WORLD ticks, so a world nobody is standing in —
    /// which still shifts, still regrows, and is exactly the case persistence exists for
    /// — is saved at the same rate as a busy one.
    fn hibernate_world(&mut self) {
        if !self.balance.world_persist.enabled {
            return;
        }
        let every = self.balance.world_persist.save_every_ticks.max(1);
        let Some(w) = self.world.as_ref() else { return };
        if w.tutorial || w.tick_count < self.last_world_save + every {
            return;
        }
        self.last_world_save = w.tick_count;
        let _ = self.db_writes.send(DbWrite::SaveWorld(Box::new(w.world_save())));
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
                        forging_level: None,
                        character_class: CharacterClass::Explorer,
                        party_comp: None,
                        hero_names: None,
                        hero_rows: None,
                        has_dived: false,
                        tutorial_town_seen: false,
                        tutorial_run_seen: false,
                        deepest_ever: 0,
                        unlocks: None,
                        pending_materials: Vec::new(),
                        hunts: None,
                    },
                );
                self.order.push(player_id.clone());
                self.pending_gear_load.push(player_id.clone());
                // The city anvil lays its heat out against the caller's own Forging
                // level, so it is needed from the moment they connect, not just on a dive.
                self.pending_skill_load.push(player_id.clone());
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
                self.pending_skill_load.retain(|p| p != &player_id);
                self.pending_bounty_load.retain(|p| p != &player_id);
                self.pending_hero_load.retain(|p| p != &player_id);
                // A disconnect that drops a still-unresolved run ends it
                // `abandoned` — the other red-burning end (spec §5): any
                // equipped Vault-owned red gear is permanently deleted.
                let abandoned = self.world.as_ref().is_some_and(|inst| {
                    inst.run
                        .runs
                        .iter()
                        .any(|r| r.player_id == player_id && r.result.is_none())
                });
                if abandoned {
                    let _ = self.db_writes.send(DbWrite::BurnEphemeral(player_id.clone()));
                }
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
                    self.ensure_starter_gear_for(&player_id).await;
                }
                self.handle_client(&player_id, raw)
            }
        }
    }

    /// Drop a player's overworld/run state from the world (on disconnect, or when
    /// their run ends).
    ///
    /// **The world is NOT torn down when the last diver leaves** (CANON §W1): it is a
    /// place, and it is still there — same seed, same ground you cleared, same Shift
    /// schedule ticking — when you or anyone else comes back. What used to justify the
    /// teardown was that a felled creature never returned and the second dive found an
    /// empty map; `Arena::regrow` and the Shift are what answer that now, and they
    /// answer it as content rather than as amnesia.
    ///
    /// A TUTORIAL world is the one exception and still dies with its diver. The guided
    /// first dive is onboarding — a fixed biome order and a centred, obstacle-free area
    /// 0 — so persisting it would hand the next player a world that had already been
    /// walked, and hand the returning one a corridor they had already seen.
    fn remove_from_instance(&mut self, player_id: &str) {
        let Some(inst) = self.world.as_mut() else {
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
        if inst.run.runs.is_empty() && (inst.tutorial || !self.balance.world_persist.enabled) {
            self.world = None;
        }
    }

    /// A player's run has ended (extracted or died): release them so they can
    /// dive again from the hub. Clears the session's in-instance flag and drops
    /// their run/avatar/bookkeeping from the shared instance (tearing the
    /// instance down if they were the last one). Without this, `in_instance`
    /// stays `true` after a run ends and the next `enter_maze` is rejected with
    /// "A run is already active for you." — the extract-or-die loop can't close.
    /// CL-1: a milestone landed — work out what it unlocks for this account,
    /// persist it, and announce it. Everything is decided against the session's
    /// in-memory set, so a milestone reported every tick still only ever grants
    /// once, and the DB write can lag without letting it grant twice.
    fn grant_milestone(&mut self, player_id: &str, milestone: meld_proto::unlocks::Milestone) {
        let Some(owned) = self.sessions.get(player_id).and_then(|s| s.unlocks.clone()) else {
            return; // unlocks not loaded yet — nothing to compare against
        };
        let newly = meld_proto::unlocks::granted_by(milestone, &owned);
        if newly.is_empty() {
            return;
        }
        let keys: Vec<String> = newly.iter().map(|u| u.key.to_string()).collect();
        let mut owned = owned;
        owned.extend(keys.iter().cloned());
        owned.sort();
        if let Some(s) = self.sessions.get_mut(player_id) {
            s.unlocks = Some(owned.clone());
        }
        let _ = self
            .db_writes
            .send(DbWrite::Unlocks(player_id.to_string(), keys));
        let deepest = self.sessions.get(player_id).map(|s| s.deepest_ever).unwrap_or(0);
        let msg = unlock_inventory(&owned, &newly, true, deepest);
        self.dispatch(vec![out_msg(player_id, &msg)]);
    }

    /// AD-4: offer a fact to every posted hunt and credit the ones that want it.
    ///
    /// Decided against the session's in-memory board, like `grant_milestone`: the cap
    /// and the completion crossing are settled here, so several kills in one tick
    /// cannot each announce the same finish and the DB write can lag without paying
    /// twice. A claimed hunt is inert.
    fn credit_hunts(&mut self, player_id: &str, fact: &HuntFact) {
        let Some(board) = self.sessions.get(player_id).and_then(|s| s.hunts.clone()) else {
            return;
        };
        let ev = fact.as_event();
        let mut moved: Vec<(String, i32, i32, i32, bool)> = Vec::new();
        for def in meld_proto::hunts::HUNTS {
            let (progress, claimed) = board.get(def.key).copied().unwrap_or((0, false));
            let target = def.goal.target();
            if claimed || progress >= target {
                continue;
            }
            let delta = def.goal.credits(&ev);
            if delta <= 0 {
                continue;
            }
            let now = (progress + delta).min(target);
            moved.push((def.key.to_string(), delta, now, target, now >= target));
        }
        if moved.is_empty() {
            return;
        }
        if let Some(s) = self.sessions.get_mut(player_id) {
            if let Some(b) = s.hunts.as_mut() {
                for (key, _, now, _, _) in &moved {
                    let e = b.entry(key.clone()).or_insert((0, false));
                    e.0 = *now;
                }
            }
        }
        // A finished hunt stops being tracked, so the world's copy is refreshed from the
        // same board the credit just moved.
        let targets = self
            .sessions
            .get(player_id)
            .and_then(|s| s.hunts.as_ref())
            .map(quarry_targets);
        if let (Some(w), Some(t)) = (self.world.as_mut(), targets) {
            w.quarry.insert(player_id.to_string(), t);
        }
        let mut out = Vec::new();
        for (key, delta, now, target, complete) in moved {
            let _ = self.db_writes.send(DbWrite::HuntProgress(
                player_id.to_string(),
                key.clone(),
                delta,
                target,
            ));
            let name = meld_proto::hunts::hunt(&key).map_or(key.clone(), |d| d.name.to_string());
            out.push(out_msg(
                player_id,
                &wr::HuntProgress { key, name, progress: now, target, complete },
            ));
        }
        self.dispatch(out);
    }

    /// AD-4: a bounty's mark is down. Record it and say so; the Den pays at the board.
    fn finish_bounty(&mut self, player_id: &str, bounty_id: &str, mark: &str) {
        let _ = self.db_writes.send(DbWrite::BountyFelled(
            player_id.to_string(),
            bounty_id.to_string(),
        ));
        let name = meld_world::boss_display_name(mark);
        self.dispatch(vec![out_msg(
            player_id,
            &wr::HuntProgress {
                key: format!("bounty:{bounty_id}"),
                name: format!("{name} is down"),
                progress: 1,
                target: 1,
                complete: true,
            },
        )]);
    }

    fn release_from_run(&mut self, player_id: &str) {
        if let Some(s) = self.sessions.get_mut(player_id) {
            s.in_instance = false;
        }
        self.remove_from_instance(player_id);
    }

    /// Apply the world actor's emitted [`WorldEffect`]s. World-scoped tick/handler
    /// logic can't touch the Router's sessions or tear the world down mid-borrow,
    /// so it returns these and the Router applies them once the world borrow ends.
    fn apply_world_effects(&mut self, effects: Vec<WorldEffect>) {
        for e in effects {
            match e {
                WorldEffect::ReleaseFromRun(pid) => self.release_from_run(&pid),
                WorldEffect::Milestone {
                    player_id,
                    milestone,
                } => self.grant_milestone(&player_id, milestone),
                WorldEffect::SetSessionHeroName {
                    player_id,
                    slot,
                    name,
                } => {
                    if let Some(s) = self.sessions.get_mut(&player_id) {
                        let mut v = s.hero_names.clone().unwrap_or_default();
                        while v.len() <= slot {
                            v.push(generated_hero_name(&player_id, v.len()));
                        }
                        v[slot] = name;
                        s.hero_names = Some(v);
                    }
                }
                WorldEffect::SmithJob(job) => self.open_heat(*job),
                WorldEffect::Hunt { player_id, fact } => self.credit_hunts(&player_id, &fact),
                WorldEffect::BountyFelled {
                    player_id,
                    bounty_id,
                    mark,
                } => self.finish_bounty(&player_id, &bounty_id, &mark),
                WorldEffect::SetSessionHeroRow {
                    player_id,
                    slot,
                    back,
                } => {
                    if let Some(s) = self.sessions.get_mut(&player_id) {
                        let mut v = s.hero_rows.clone().unwrap_or_default();
                        while v.len() <= slot {
                            v.push(false);
                        }
                        v[slot] = back;
                        s.hero_rows = Some(v);
                    }
                }
            }
        }
    }

    /// Say something to the other people on the server.
    ///
    /// Router-level, not world-level, and that is the point: chat has to work in town, in
    /// a lobby and mid-dive alike, and only the Router can see all three. A world-scoped
    /// handler would have silently swallowed every line said by anyone not currently in a
    /// maze — which is most people, most of the time.
    ///
    /// The sender's name and the timestamp are stamped HERE. A client that supplied either
    /// is a client that can impersonate, and a chat line is the one message whose whole
    /// value is that you can trust who it came from.
    fn handle_say(&mut self, player_id: &str, raw: RawEnvelope) -> Vec<Outgoing> {
        let req: wc::Say = match serde_json::from_value(raw.payload) {
            Ok(v) => v,
            Err(_) => {
                return vec![error(
                    player_id,
                    ErrorCode::ValidationError,
                    "bad chat.say",
                    Some(raw.seq),
                )]
            }
        };
        let text: String = req.text.trim().chars().take(wc::TEXT_MAX).collect();
        if text.is_empty() {
            return vec![error(
                player_id,
                ErrorCode::ValidationError,
                "nothing to say",
                Some(raw.seq),
            )];
        }
        let Some(me) = self.sessions.get(player_id) else {
            return Vec::new();
        };
        let (username, mine) = (me.username.clone(), me.in_instance);
        // `Party` is "the people you are actually among": everyone in the maze if you are
        // in it, everyone still in town if you are not. There is one world, so this is the
        // honest scope today — LC-1's ward sharding is what narrows it to proximity later.
        let room: Vec<&str> = self
            .sessions
            .iter()
            .filter(|(_, s)| match req.channel {
                wc::Channel::World => true,
                wc::Channel::Party => s.in_instance == mine,
            })
            .map(|(pid, _)| pid.as_str())
            .collect();
        broadcast(
            room,
            &wc::Line {
                player_id: player_id.to_string(),
                username,
                text,
                channel: req.channel,
                ts: now_ms(),
            },
        )
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
            wo::TownSeen::TYPE => self.handle_onboarding_town_seen(player_id, raw.seq),
            wo::RunSeen::TYPE => self.handle_onboarding_run_seen(player_id, raw.seq),
            wr::BeginExtraction::TYPE => {
                let (out, eff) = match self.world.as_mut() {
                    Some(w) => w.handle_begin_extraction(player_id, raw),
                    None => (
                        vec![error(player_id, ErrorCode::InvalidState, "Not in a run.", Some(raw.seq))],
                        Vec::new(),
                    ),
                };
                self.apply_world_effects(eff);
                out
            }
            wr::BuildStation::TYPE => {
                let (out, eff) = match self.world.as_mut() {
                    Some(w) => w.handle_build_station(player_id, raw),
                    None => (
                        vec![error(player_id, ErrorCode::InvalidState, "Not in a run.", Some(raw.seq))],
                        Vec::new(),
                    ),
                };
                self.apply_world_effects(eff);
                out
            }
            wr::Strike::TYPE => self.handle_strike(player_id, raw),
            wr::TeardownStation::TYPE => {
                let (out, eff) = match self.world.as_mut() {
                    Some(w) => w.handle_teardown_station(player_id, raw),
                    None => (
                        vec![error(player_id, ErrorCode::InvalidState, "Not in a run.", Some(raw.seq))],
                        Vec::new(),
                    ),
                };
                self.apply_world_effects(eff);
                out
            }
            wr::SmithRequest::TYPE => {
                // In a run you work at a STATION someone raised; in town you work at the
                // city anvil, where the only smith is you. Same message, same heat, same
                // rules — the difference is only whose skill is doing it.
                let in_run = self
                    .world
                    .as_ref()
                    .is_some_and(|w| w.run.runs.iter().any(|r| r.player_id == player_id));
                let (out, eff) = if in_run {
                    self.world
                        .as_mut()
                        .map(|w| w.handle_smith_request(player_id, raw))
                        .unwrap_or_default()
                } else {
                    (self.handle_anvil_request(player_id, raw), Vec::new())
                };
                self.apply_world_effects(eff);
                out
            }
            wr::Harvest::TYPE => {
                let (out, eff) = match self.world.as_mut() {
                    Some(w) => w.handle_harvest(player_id, raw),
                    None => (
                        vec![error(player_id, ErrorCode::InvalidState, "Not in a run.", Some(raw.seq))],
                        Vec::new(),
                    ),
                };
                self.apply_world_effects(eff);
                out
            }
            wr::PsykerHold::TYPE => {
                let out = match self.world.as_mut() {
                    Some(w) => w.handle_psyker_hold(player_id, raw),
                    None => {
                        vec![error(player_id, ErrorCode::InvalidState, "Not in a run.", Some(raw.seq))]
                    }
                };
                out
            }
            wr::BuildStructure::TYPE => match self.world.as_mut() {
                Some(w) => w.handle_build_structure(player_id, raw),
                None => vec![error(player_id, ErrorCode::InvalidState, "Not in a run.", Some(raw.seq))],
            },
            wr::RepairStructure::TYPE => match self.world.as_mut() {
                Some(w) => w.handle_repair_structure(player_id, raw),
                None => vec![error(player_id, ErrorCode::InvalidState, "Not in a run.", Some(raw.seq))],
            },
            wr::DemolishStructure::TYPE => match self.world.as_mut() {
                Some(w) => w.handle_demolish_structure(player_id, raw),
                None => vec![error(player_id, ErrorCode::InvalidState, "Not in a run.", Some(raw.seq))],
            },
            wr::UseItem::TYPE => {
                let (out, eff) = match self.world.as_mut() {
                    Some(w) => w.handle_use_item(player_id, raw),
                    None => (
                        vec![error(player_id, ErrorCode::InvalidState, "Not in a run.", Some(raw.seq))],
                        Vec::new(),
                    ),
                };
                self.apply_world_effects(eff);
                out
            }
            wr::MoveItem::TYPE => {
                let (out, eff) = match self.world.as_mut() {
                    Some(w) => w.handle_move_item(player_id, raw),
                    None => (
                        vec![error(player_id, ErrorCode::InvalidState, "Not in a run.", Some(raw.seq))],
                        Vec::new(),
                    ),
                };
                self.apply_world_effects(eff);
                out
            }
            wc::Say::TYPE => self.handle_say(player_id, raw),
            wr::CancelHarvest::TYPE => {
                let (out, eff) = match self.world.as_mut() {
                    Some(w) => w.handle_cancel_harvest(player_id, raw),
                    None => (
                        vec![error(player_id, ErrorCode::InvalidState, "Not in a run.", Some(raw.seq))],
                        Vec::new(),
                    ),
                };
                self.apply_world_effects(eff);
                out
            }
            wr::OpenChest::TYPE => {
                let (out, eff) = match self.world.as_mut() {
                    Some(w) => w.handle_open_chest(player_id, raw),
                    None => (
                        vec![error(player_id, ErrorCode::InvalidState, "Not in a run.", Some(raw.seq))],
                        Vec::new(),
                    ),
                };
                self.apply_world_effects(eff);
                out
            }
            wr::WatchBattle::TYPE => {
                let (out, eff) = match self.world.as_mut() {
                    Some(w) => w.handle_watch_battle(player_id, raw),
                    None => (
                        vec![error(player_id, ErrorCode::InvalidState, "Not in a run.", Some(raw.seq))],
                        Vec::new(),
                    ),
                };
                self.apply_world_effects(eff);
                out
            }
            wr::StopWatching::TYPE => {
                let (out, eff) = match self.world.as_mut() {
                    // Nothing to stop without a world, and asking is not an error: the
                    // client fires this off the same key that opened the feed.
                    Some(w) => w.handle_stop_watching(player_id, raw),
                    None => (Vec::new(), Vec::new()),
                };
                self.apply_world_effects(eff);
                out
            }
            wr::JoinBattle::TYPE => {
                let (out, eff) = match self.world.as_mut() {
                    Some(w) => w.handle_join_battle(player_id, raw),
                    None => (
                        vec![error(player_id, ErrorCode::InvalidState, "Not in a run.", Some(raw.seq))],
                        Vec::new(),
                    ),
                };
                self.apply_world_effects(eff);
                out
            }
            wr::RenameHero::TYPE => {
                let (out, eff) = match self.world.as_mut() {
                    Some(w) => w.handle_rename_hero(player_id, raw),
                    // No active run (party-builder pre-dive path): reproduce the
                    // pre-move no-world behaviour — validate, update only the session
                    // cache (for the NEXT dive) via the effect + persist, and return
                    // an empty roster (no world → no party).
                    None => match serde_json::from_value::<wr::RenameHero>(raw.payload) {
                        Ok(req) => {
                            let party_size = self.balance.battle.party_size_per_player.max(1) as i32;
                            let name: String = req.name.trim().chars().take(24).collect();
                            if name.is_empty() || req.slot < 0 || req.slot >= party_size {
                                (
                                    vec![error(player_id, ErrorCode::ValidationError, "Invalid hero name or slot.", Some(raw.seq))],
                                    Vec::new(),
                                )
                            } else {
                                let slot = req.slot as usize;
                                let _ = self.db_writes.send(DbWrite::HeroRename(
                                    player_id.to_string(),
                                    slot as i16,
                                    name.clone(),
                                ));
                                (
                                    vec![out_msg(player_id, &wr::Party { heroes: Vec::new(), synergies: Vec::new(), combos: Vec::new(), abilities: Vec::new() })],
                                    vec![WorldEffect::SetSessionHeroName {
                                        player_id: player_id.to_string(),
                                        slot,
                                        name,
                                    }],
                                )
                            }
                        }
                        Err(_) => (
                            vec![error(player_id, ErrorCode::ValidationError, "bad rename_hero", Some(raw.seq))],
                            Vec::new(),
                        ),
                    },
                };
                self.apply_world_effects(eff);
                out
            }
            wr::SetFormation::TYPE => {
                let (out, eff) = match self.world.as_mut() {
                    Some(w) => w.handle_set_formation(player_id, raw),
                    // No active run (party-builder pre-dive path): reproduce the
                    // pre-move no-world behaviour — validate, update only the session
                    // cache (for the NEXT dive) via the effect + persist, and return
                    // an empty roster (no world → no party).
                    None => match serde_json::from_value::<wr::SetFormation>(raw.payload) {
                        Ok(req) => {
                            let party_size = self.balance.battle.party_size_per_player.max(1) as i32;
                            if req.slot < 0 || req.slot >= party_size {
                                (
                                    vec![error(player_id, ErrorCode::ValidationError, "Invalid hero slot.", Some(raw.seq))],
                                    Vec::new(),
                                )
                            } else {
                                let slot = req.slot as usize;
                                let back = req.back_row;
                                let _ = self.db_writes.send(DbWrite::HeroFormation(
                                    player_id.to_string(),
                                    slot as i16,
                                    back,
                                ));
                                (
                                    vec![out_msg(player_id, &wr::Party { heroes: Vec::new(), synergies: Vec::new(), combos: Vec::new(), abilities: Vec::new() })],
                                    vec![WorldEffect::SetSessionHeroRow {
                                        player_id: player_id.to_string(),
                                        slot,
                                        back,
                                    }],
                                )
                            }
                        }
                        Err(_) => (
                            vec![error(player_id, ErrorCode::ValidationError, "bad set_formation", Some(raw.seq))],
                            Vec::new(),
                        ),
                    },
                };
                self.apply_world_effects(eff);
                out
            }
            wr::EquipLoot::TYPE => {
                let (out, eff) = match self.world.as_mut() {
                    Some(w) => w.handle_equip_loot(player_id, raw),
                    None => (
                        vec![error(player_id, ErrorCode::InvalidState, "Not in a run.", Some(raw.seq))],
                        Vec::new(),
                    ),
                };
                self.apply_world_effects(eff);
                out
            }
            wr::EnterDungeon::TYPE => self.handle_enter_dungeon(player_id, raw),
            wm::MoveIntent::TYPE => {
                let (out, eff) = match self.world.as_mut() {
                    Some(w) => w.handle_move(player_id, raw),
                    None => (
                        vec![error(player_id, ErrorCode::InvalidState, "Not in a run.", Some(raw.seq))],
                        Vec::new(),
                    ),
                };
                self.apply_world_effects(eff);
                out
            }
            wb::SubmitAction::TYPE => {
                let (out, eff) = match self.world.as_mut() {
                    Some(w) => w.handle_submit(player_id, raw),
                    None => (
                        vec![error(player_id, ErrorCode::NotFound, "No battle.", Some(raw.seq))],
                        Vec::new(),
                    ),
                };
                self.apply_world_effects(eff);
                out
            }
            other => vec![error(
                player_id,
                ErrorCode::ValidationError,
                format!("unknown message type: {other}"),
                Some(raw.seq),
            )],
        }
    }

    /// DG-3b: deliberate descent into a hand-designed dungeon (`run.enter_dungeon`).
    fn handle_enter_dungeon(&mut self, player_id: &str, raw: RawEnvelope) -> Vec<Outgoing> {
        let seq = raw.seq;
        let req: wr::EnterDungeon = match serde_json::from_value(raw.payload) {
            Ok(v) => v,
            Err(_) => {
                return vec![error(player_id, ErrorCode::ValidationError, "bad enter_dungeon", Some(seq))]
            }
        };
        let Some(inst) = self.world.as_mut() else {
            return vec![error(player_id, ErrorCode::InvalidState, "Not in a run.", Some(seq))];
        };
        inst.enter_dungeon_by_id(player_id, &req.entity_id, seq)
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
            .unwrap_or(CharacterClass::Explorer);
        let names = req
            .as_ref()
            .and_then(|e| e.names.clone())
            .filter(|n| !n.is_empty());
        // CL-1: a party is clamped to what the account has EARNED — extra slots
        // dropped, unowned classes replaced by the Explorer. Clamped rather than
        // rejected, so a stale client (or a saved party from before a wipe) still
        // gets a dive instead of an error it can't act on.
        let owned = self.sessions.get(player_id).and_then(|s| s.unlocks.clone());
        let party_comp = match (&owned, party_comp) {
            (Some(owned), Some(p)) => Some(clamp_party_to_unlocks(p, owned)),
            (_, p) => p,
        };
        let chosen = match &owned {
            Some(owned) if !meld_proto::unlocks::owned_classes(owned).contains(&chosen) => {
                CharacterClass::Explorer
            }
            _ => chosen,
        };
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
        let wants_tutorial = req.as_ref().and_then(|e| e.tutorial).unwrap_or(false);
        let wants_hub = req.as_ref().and_then(|e| e.hub.clone());
        self.form_run(party_ids, player_id, Some(client_seq), wants_tutorial, wants_hub)
    }

    /// Enroll `party_ids` into a shared MazeInstance and emit `run.started` to
    /// each. The initiator's `run.started` echoes `client_seq`. Every enrolled
    /// player's session must already carry its `character_class` / `party_comp`.
    fn form_run(
        &mut self,
        party_ids: Vec<String>,
        initiator: &str,
        client_seq: Option<u32>,
        wants_tutorial: bool,
        // The departure hub the initiator asked for (PG-2), if any. Clamped below.
        wants_hub: Option<String>,
    ) -> Vec<Outgoing> {
        // PG-2 — where this dive departs from, and therefore what level its heroes start
        // at (`meld_run::base_run_level`). This was hard-coded to the Center Hub, which is
        // why every hero started every dive at level 1 and the ladder above roughly level
        // 16 was authored ahead of anything reachable.
        //
        // **THE AUTHORED DEEP HUBS ARE RETIRED, and `wants_hub` with them.** There were six
        // (d500 … d3250) gated on the account's all-time deepest distance from the `vanguard`
        // record. `BD-5`'s player-built forward towns replace them and the gate: you cannot
        // raise a town where you cannot stand, so the structure IS the proof you were there,
        // and a departure point that is HP-bearing and Shift-exposed makes the deep ladder a
        // loop you keep paying for rather than a list you tick off once.
        //
        // The authored ladder was also self-defeating — measured, the ground at d3200 demands
        // ~level 251 to survive four basic hits from a *standard* creature, and levels are
        // dive-scoped, so its top rung could only be unlocked by a party that had already
        // walked to d3250 at level 1. It required what it was meant to grant.
        //
        // What survives is the LOOKUP: a dive reads one distance and `base_run_level` turns
        // it into a starting level. When a town supplies that distance, this is where it
        // lands — the blockers are unchanged (spawn-at-distance, frontier generation around
        // it, and extraction still assuming d0 is the start).
        let _ = wants_hub;
        // `MELD_START_LEVEL=<n>` — DEV/QA, beside MELD_END_FIGHT / MELD_GEAR_TIER / MELD_POTIONS.
        //
        // It sets a DISTANCE, and the level follows from it, because level and depth are the
        // same fact in this game and decoupling them measures a party that cannot exist. The
        // two curves already agree: `base_run_level(d) = 1 + 0.078d` and creature level
        // `= d/12.5` both read 40 at d500, so a party started at its own level's distance meets
        // creatures of its own level — which is the whole point of asking for a level at all.
        //
        // Earlier this only overrode `base_run_level`, which handed out a level-40 party
        // standing in the level-1 ring. Every "middle game" number taken that way was really a
        // measurement of over-levelled trivia: d0→d185 cost such a party 5 HP across 10 fights.
        let dev_distance = std::env::var("MELD_START_LEVEL")
            .ok()
            .and_then(|v| v.trim().parse::<i32>().ok())
            .filter(|l| *l > 1)
            .map(|l| {
                let per = self.balance.runs.base_run_level_per_distance.max(1e-6);
                let capped = l.min(self.balance.runs.max_hero_level);
                (((capped - 1) as f64) / per).round() as i32
            });
        let departure_hub_distance = dev_distance.unwrap_or(0);
        let speed = self.balance.world.avatar_speed_tiles_per_sec;

        // Create the shared instance on the first entry.
        if self.world.is_none() {
            let instance_id = Uuid::now_v7().to_string();
            // The tutorial is OPT-IN, not auto-forced on a first dive: the hub OFFERS the
            // guided Forest-first onboarding (centred, obstacle-free area 0) but a normal
            // dive is a randomized run, so a returning player isn't dropped into the same
            // corridor every `make play`. `wants_tutorial` comes from the client's
            // `run.enter_maze` `tutorial` flag; `MELD_TUTORIAL=1` forces it on for
            // headless/QA.
            // DEV/QA harness (in-memory build): `MELD_BIOME=<forest|desert|ashfall|
            // tundra|mire>` pins every section to that biome so its maze can be loaded +
            // screenshotted on demand, and `MELD_SEED=<n>` fixes the layout for
            // reproducibility. `MELD_BIOME`/`MELD_NO_TUTORIAL` force the tutorial off.
            // All read only here at the server boundary — `meld-world` stays pure.
            let force_biome: Option<&'static str> = std::env::var("MELD_BIOME")
                .ok()
                .and_then(|v| meld_world::BIOMES.iter().copied().find(|b| *b == v.trim()));
            let tutorial = force_biome.is_none()
                && !std::env::var("MELD_NO_TUTORIAL").is_ok_and(|v| v != "0")
                && (wants_tutorial || std::env::var("MELD_TUTORIAL").is_ok_and(|v| v != "0"));
            // Server-generated world seed (CANON: the client never supplies or
            // computes seeds) — overridable by `MELD_SEED` for the QA harness.
            let seed = std::env::var("MELD_SEED")
                .ok()
                .and_then(|v| v.trim().parse::<u64>().ok())
                .unwrap_or_else(world_seed);
            // `MELD_END_FIGHT=1` — bring THE END FIGHT to the hub instead of walking an hour
            // out to it. It only moves where the encounter is placed: the three bosses carry
            // AUTHORED absolute stats (`set_piece`), so the fight you meet at d30 is
            // numerically the fight you would meet at d3200. That is the whole reason this
            // harness can be a one-line override rather than a fake.
            //
            // It does NOT make the fight winnable — a starting party dies in about a hit and
            // a half, which is the tuning working. Pair it with `MELD_GEAR_TIER` below to see
            // the other side of the gear gate.
            let balance = match std::env::var("MELD_END_FIGHT") {
                Ok(v) if v != "0" => {
                    let mut b = (*self.balance).clone();
                    b.encounters.end_fight_min_distance =
                        std::env::var("MELD_END_FIGHT_AT")
                            .ok()
                            .and_then(|v| v.trim().parse::<f64>().ok())
                            .unwrap_or(30.0);
                    tracing::warn!(
                        at = b.encounters.end_fight_min_distance,
                        "MELD_END_FIGHT: the end fight is placed near the hub (DEV/QA)"
                    );
                    Arc::new(b)
                }
                _ => self.balance.clone(),
            };
            // A world read back at boot is stood up here rather than in a second
            // construction site: restoring IS the normal build, reading its seed and its
            // delta off disk instead of off a fresh roll. A tutorial dive never claims
            // one — the guided corridor is onboarding, not a place.
            let restored = (!tutorial).then(|| self.restore.take()).flatten();
            if let Some(save) = &restored {
                tracing::info!(seed = save.seed, "world.persist: standing the saved world back up");
            }
            self.last_world_save = restored.as_ref().map(|s| s.tick_count as u64).unwrap_or(0);
            self.world = Some(WorldActor {
                balance: balance.clone(),
                db_writes: self.db_writes.clone(),
                arena: match &restored {
                    Some(save) => restore_world(&balance, save),
                    None => Arena::generate_with(&balance, seed, tutorial, force_biome),
                },
                run: InstanceRun::new(instance_id, departure_hub_distance, &balance, now_ms()),
                battles: Vec::new(),
                hero_hp: HashMap::new(),
                hero_afflictions: HashMap::new(),
                durability_charged: HashSet::new(),
                venom_steps: HashMap::new(),
                party_classes: HashMap::new(),
                gear_bonuses: HashMap::new(),
                hero_names: HashMap::new(),
                hero_rows: HashMap::new(),
                extraction: HashMap::new(),
                harvest: HashMap::new(),
                building: HashMap::new(),
                regen_accum: HashMap::new(),
                hold_last_ms: HashMap::new(),
                watching: HashMap::new(),
                entrances: Vec::new(),
                tutorial,
                tutorial_entrance_placed: false,
                location: HashMap::new(),
                dungeons: HashMap::new(),
                next_dungeon_key: 0,
                entrances_scanned: 0,
                dungeon_scene_sent: HashMap::new(),
                pending_effects: Vec::new(),
                skill_levels: HashMap::new(),
                quarry: HashMap::new(),
                bounties: HashMap::new(),
                marks_placed: std::collections::HashSet::new(),
                tick_count: restored.as_ref().map(|s| s.tick_count as u64).unwrap_or(0),
                shift_generation: restored
                    .as_ref()
                    .map(|s| s.shift_generation as u64)
                    .unwrap_or(0),
                shift_warned: false,
                shift_log: restored
                    .as_ref()
                    .and_then(|s| serde_json::from_str::<WorldDelta>(&s.delta).ok())
                    .map(|d| d.shifts)
                    .unwrap_or_default(),
                edges: HashMap::new(),
                battle_immune_until: HashMap::new(),
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

        let inst = self.world.as_mut().expect("instance exists");
        let instance_id = inst.run.instance_id.clone();
        let base_run_level = inst.run.base_run_level;

        let members: Vec<(String, String, CharacterClass, String)> = party_ids
            .iter()
            .map(|pid| {
                let (username, class) = self
                    .sessions
                    .get(pid)
                    .map(|s| (s.username.clone(), s.character_class))
                    .unwrap_or((String::new(), CharacterClass::Explorer));
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
        let starting_tp = dev_town_portals().unwrap_or(self.balance.runs.starting_town_portals);
        // Seed the starting consumables: Town Portals (extraction) + finite battle heal
        // items (Salve/Elixir), so the battle Item command is now inventory-backed.
        let starting_stock = [
            (TOWN_PORTAL, starting_tp),
            // `MELD_POTIONS=<n>` — DEV/QA, the family's fourth flag. A party that walked to
            // the end-world can shop before it dives, and a salve is 40% of a hero's max HP,
            // so the 3-salve starting kit measures an UNPREPARED party at the apex.
            ("bloom_salve", dev_potions().unwrap_or(self.balance.runs.starting_salves)),
            ("elixir", dev_potions().unwrap_or(self.balance.runs.starting_elixirs)),
        ];
        for (kind, qty) in starting_stock {
            if qty <= 0 {
                continue;
            }
            for pid in &party_ids {
                if let Some(r) = inst.run.run_mut(pid) {
                    r.backpack.push(ItemStack {
                        item_id: Uuid::now_v7().to_string(),
                        item_kind: kind.to_string(),
                        quantity: qty,
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
        // A DEV/QA deep start has to move the party as well as its level, and the world has to
        // exist out there first: `ensure_frontier` streams a few sections per call (it caps
        // growth so a teleport cannot explode the tick), so it is pumped until it stops
        // producing rings or a bound is hit. Then the avatars are placed **on the route** at
        // that depth.
        //
        // NOT at `(reach, 0)`, which is what this used to do. The fan (WG-4) bends corridor y
        // into an angle, so a distance is a RING and `(reach, 0)` is one arbitrary point on
        // it — while the world's clear path crosses that ring somewhere else entirely.
        // Measured across five seeds, the old spawn stood **600 to 1,811 units of arc** off
        // the route. Everything the world anchors to its route is therefore a quarter-turn
        // away from a party started deep: the end fight, the deep portal, the Gatekeeper in
        // the pass. At seed 424242 / d1269 the end-fight bosses sit at angle -87 degrees
        // while the party stood at 0, which is why that fight had never once been played.
        //
        // The rest of PG-2 is still not wired — extraction still assumes d0 is the start —
        // which is exactly why this is a TEST flag and not a departure hub.
        if departure_hub_distance > 0 {
            let reach = departure_hub_distance as f64;
            // `inst.balance`, NOT `self.balance`. The world was built from a CLONE carrying
            // the DEV overrides (`MELD_END_FIGHT_AT` rewrites `end_fight_min_distance` on it),
            // and every section this pump streams is generated from whatever balance it is
            // handed. Passing the un-overridden one meant `start_level` + `end_fight_at`
            // silently disagreed: the initial `generate` honoured the requested floor, then
            // every streamed section past it went back to the shipped d3200 — so the end
            // fight was placed at d3200, an hour's walk further out than the flag asked for,
            // and a party started deep never met it. Two overrides that do not compose is the
            // same one-rule-two-call-sites failure this file has been bitten by before.
            let inst_balance = inst.balance.clone();
            for _ in 0..256 {
                if inst.arena.ensure_frontier(&inst_balance, reach).is_empty() {
                    break;
                }
            }
            let landing = inst.arena.route_point_at(reach);
            for pid in &party_ids {
                if let Some(a) = inst.arena.avatar_mut(pid) {
                    a.position = landing;
                    a.elevation = 0;
                }
            }
            tracing::warn!(
                distance = departure_hub_distance,
                level = inst.run.base_run_level,
                "MELD_START_LEVEL: party started deep (DEV/QA)"
            );
        }
        // (Re)enter = a fresh dive: build each player's mixed party composition and
        // start every hero at its class's full HP. Within the run this HP persists
        // across battles (see hero_hp write-back).
        // The CAP, not the entitlement. How many heroes a player actually fields is
        // how many party slots their account has EARNED (CL-1) — clamping the chosen
        // composition down and then padding it back to the cap handed a one-slot
        // account four copies of the only class it owns.
        let party_cap = self.balance.battle.party_size_per_player.max(1);
        for pid in &party_ids {
            let (chosen, explicit, names, rows, gear, owned) = self
                .sessions
                .get(pid)
                .map(|s| {
                    (
                        s.character_class,
                        s.party_comp.clone(),
                        s.hero_names.clone(),
                        s.hero_rows.clone(),
                        s.gear_bonuses.clone(),
                        s.unlocks.clone(),
                    )
                })
                .unwrap_or((CharacterClass::Explorer, None, None, None, Vec::new(), None));
            // Unlocks not loaded yet (a dive racing the account read) falls back to
            // ONE hero rather than the cap: too few heroes is a worse dive, too many
            // is a party the account did not earn.
            let party_size = owned
                .as_deref()
                .map(|o| meld_proto::unlocks::party_slots(o) as usize)
                .unwrap_or(1)
                .clamp(1, party_cap);
            // The builder's explicit composition wins (normalized to party size,
            // padded with Explorer); otherwise build a default mixed party around
            // the lead.
            let comp = match explicit {
                Some(mut p) => {
                    p.truncate(party_size);
                    while p.len() < party_size {
                        p.push(CharacterClass::Explorer);
                    }
                    p
                }
                None => party_composition(chosen, party_size),
            };
            // GR-7: the party you take down is the roster you come home with — the
            // RESOLVED comp, so a default mixed party is recorded too. Equip-time
            // legality (GR-5) reads these rows while the player is in town.
            for (i, class) in comp.iter().enumerate() {
                let _ = self.db_writes.send(DbWrite::HeroClass(
                    pid.to_string(),
                    i as i16,
                    meld_run::class_key(*class).to_string(),
                ));
            }
            // Hero names by slot: the builder's, normalized to party size, with any
            // unnamed slot falling back to its generated name.
            let mut names = names.unwrap_or_default();
            names.truncate(party_size);
            while names.len() < party_size {
                names.push(generated_hero_name(pid, names.len()));
            }
            let hp = meld_run::starting_hp(&comp, inst.run.base_run_level, &self.balance);
            // Saved formation by slot, normalized to party size (missing = false).
            let mut rows = rows.unwrap_or_default();
            rows.truncate(party_size);
            while rows.len() < party_size {
                rows.push(false);
            }
            // Starter gear for any empty slot is handled earlier, before this
            // function runs, as a permanent Vault backfill (see
            // `ensure_starter_gear_for` / `Db::ensure_starter_gear`) — it's
            // class-unrestricted, so it already shows up in `comp`'s eventual
            // gear_bonuses load like any other equipped Vault item, in town
            // and on every dive alike, with no dive-time special-casing here.
            // A pouch per hero, from the composition that actually went in. Sized HERE
            // rather than lazily on the first XP award, because the starting kit is
            // dealt into the pouches before any fight happens.
            if let Some(r) = inst.run.run_mut(pid) {
                r.pouches = vec![Vec::new(); comp.len()];
            }
            inst.party_classes.insert(pid.clone(), comp);
            inst.hero_hp.insert(pid.clone(), hp);
            // A fresh dive owes nothing yet. Cleared when the run STARTS rather than when
            // the last one was released, because the end-of-run report is what reads it
            // and a redive in a persistent world would otherwise inherit the last dive's
            // answer.
            inst.durability_charged.remove(pid);
            let dressed_classes =
                inst.party_classes.get(pid).cloned().unwrap_or_default();
            let gear = dress_for_dev(&inst.balance, &dressed_classes, gear);
            inst.gear_bonuses.insert(pid.clone(), gear);
            inst.hero_names.insert(pid.clone(), names);
            inst.hero_rows.insert(pid.clone(), rows);
        }
        // Deal the starting POTIONS out of the bag and into the pouches, round-robin,
        // so hero 1 can drink in the first fight without a transfer ritual first. The
        // totals are balance's (`starting_salves`/`starting_elixirs`) and the round-robin
        // spends them rather than handing each hero a full set, which would multiply the
        // starting kit by the party size. Town Portals stay in the bag: extraction is a
        // menu action, not a battle one, so a pouch slot spent on one is a slot wasted.
        let balance = self.balance.clone();
        for pid in &party_ids {
            let Some(r) = inst.run.run_mut(pid) else { continue };
            let heroes = r.pouches.len();
            if heroes == 0 {
                continue;
            }
            let kinds: Vec<(String, i32)> = r
                .backpack
                .iter()
                .filter(|i| meld_proto::consumables::is_consumable(&i.item_kind))
                .map(|i| (i.item_kind.clone(), i.quantity))
                .collect();
            let mut next = 0usize;
            for (kind, qty) in kinds {
                for _ in 0..qty {
                    if r.move_item(next % heroes, &kind, 1, true, &balance) == 0 {
                        break;
                    }
                    next += 1;
                }
            }
        }
        for pid in &party_ids {
            if let Some(s) = self.sessions.get_mut(pid) {
                s.in_instance = true;
            }
        }
        // Roster views per player (built before the shared instance borrow below).
        let rosters: HashMap<String, Vec<wr::HeroView>> = party_ids
            .iter()
            .map(|pid| (pid.clone(), self.world.as_ref().unwrap().party_views(pid)))
            .collect();
        let inst = self.world.as_ref().expect("instance exists");

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
            west_return_border: self.balance.worldgen.west_return_border,
            radial_arc_degrees: self.balance.worldgen.radial_arc_degrees,
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
            let this_run = inst.run.runs.iter().find(|r| &r.player_id == pid);
            let backpack = this_run.map(|r| r.backpack.clone()).unwrap_or_default();
            // Starter gear (see above) already lives in `looted_gear` by this
            // point — everything else found this dive is chits/red-loot, which
            // starts empty (economy.md S1).
            let backpack_gear = this_run.map(|r| r.looted_gear.clone()).unwrap_or_default();
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
                    chits: 0,
                    backpack_gear: backpack_gear.clone(),
                    path: inst.arena.path.clone(),
                    web: inst.arena.web.clone(),
                    bounds: Some(world_bounds.clone()),
                    seams: seam_views.clone(),
                    terrain_offset: {
                        let (ox, oz) = inst.arena.terrain_offset();
                        [ox, oz]
                    },
                    peaks: inst.arena.peaks.clone(),
                },
            ));
            if !backpack_gear.is_empty() {
                out.push(out_msg(pid, &wr::RunGear { gear: backpack_gear }));
            }
            out.extend(inst.pouches_msg(pid));
            out.push(out_msg(
                pid,
                &{
                    let (synergies, combos) = inst.party_depth(pid);
                    wr::Party {
                        heroes: rosters.get(pid).cloned().unwrap_or_default(),
                        synergies,
                        combos,
                        abilities: inst.party_ability_views(pid),
                    }
                },
            ));
            // The caller's earned overworld class perks ("party sense").
            out.push(out_msg(pid, &inst.perks_for(pid)));
            // Stream the initial chain's terrain (elevation grid + connectors) so
            // the client can build the stepped relief. Path rides run.started, so
            // these carry no path segment.
            let (rh, cl) = (inst.arena.radial_half(), inst.arena.corridor_lateral());
            for area in &inst.arena.areas {
                // Initial-chain peaks ride `run.started.peaks`, so the per-section
                // messages carry none (avoids double-sending).
                out.push(out_msg(pid, &terrain_section_msg(area, Vec::new(), rh, cl, Vec::new())));
            }
        }
        self.pending_gear_load.extend(party_ids.iter().cloned());
        // The professions' field verbs gate on persistent Meld levels, so the world
        // needs them in hand before anyone tries to raise a station.
        self.pending_skill_load.extend(party_ids.iter().cloned());
        // AD-4: the diver's standing contracts, so their marks can stand up as the world
        // grows out to them.
        self.pending_bounty_load.extend(party_ids.iter().cloned());
        // AD-4: and the quarry of every hunt each diver is working, so the snapshot can
        // mark it from the first tick. The board was loaded on connect, when there was no
        // world to hold it — this is the handover.
        let quarry: Vec<(String, Vec<QuarryTarget>)> = party_ids
            .iter()
            .filter_map(|pid| {
                let board = self.sessions.get(pid).and_then(|s| s.hunts.as_ref())?;
                Some((pid.clone(), quarry_targets(board)))
            })
            .collect();
        if let Some(w) = self.world.as_mut() {
            for (pid, targets) in quarry {
                w.quarry.insert(pid, targets);
            }
        }
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
                    p.push(CharacterClass::Explorer);
                }
                p
            }
            _ => party_composition(CharacterClass::Explorer, size),
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

    /// The caller dismissed the town welcome tour (finished it or ticked "don't
    /// show again"). Idempotent — only the first ack for an account is persisted.
    fn handle_onboarding_town_seen(&mut self, player_id: &str, _seq: u32) -> Vec<Outgoing> {
        if let Some(s) = self.sessions.get_mut(player_id) {
            if !s.tutorial_town_seen {
                s.tutorial_town_seen = true;
                let _ = self.db_writes.send(DbWrite::TutorialTownSeen(player_id.to_string()));
            }
        }
        Vec::new()
    }

    /// The caller dismissed the first-dive briefing. Idempotent, same shape as
    /// `handle_onboarding_town_seen`.
    fn handle_onboarding_run_seen(&mut self, player_id: &str, _seq: u32) -> Vec<Outgoing> {
        if let Some(s) = self.sessions.get_mut(player_id) {
            if !s.tutorial_run_seen {
                s.tutorial_run_seen = true;
                let _ = self.db_writes.send(DbWrite::TutorialRunSeen(player_id.to_string()));
            }
        }
        Vec::new()
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
                s.character_class = party.first().copied().unwrap_or(CharacterClass::Explorer);
                s.party_comp = Some(party.clone());
            }
            self.player_lobby.remove(pid);
        }
        self.lobbies.remove(&code);
        let ids: Vec<String> = members.into_iter().map(|(pid, _)| pid).collect();
        // Co-op dives are always the normal randomized run (the tutorial is a solo,
        // first-load onboarding — never the shared lobby path).
        // A co-op dive departs from the INITIATOR's deepest hub — the lobby leader is the
        // one whose record the run is scoped to, and `form_run` clamps it anyway.
        self.form_run(ids, player_id, Some(seq), false, None)
    }

}

impl WorldActor {
    /// A LEVEL-UP CURES. Every affliction gripping the hero that just advanced is lifted —
    /// but not death: a fallen hero still needs a raise, and coming back up is the one thing
    /// levelling does not do for you.
    ///
    /// Afflictions stopped expiring on purpose (outlasting a debuff by standing still is not
    /// a decision), which left the road with only two answers to one: carry the right bottle,
    /// or find a mender. Levelling is the third, and it is the one the player earns by
    /// fighting through it — so a condition caught early in a dive is not a tax on the whole
    /// rest of the dive.
    fn cure_on_level_up(&mut self, player_id: &str, slot: usize) {
        if let Some(hero) = self.hero_afflictions.get_mut(player_id).and_then(|c| c.get_mut(slot)) {
            hero.clear();
        }
    }

    /// What the party's afflictions cost it on the road: how much of its speed it keeps, and
    /// whether venom bites on this step.
    ///
    /// Venom is counted per STEP rather than per second on purpose. There is no waiting a
    /// poison out any more — it needs a cure — so charging by time would only punish a player
    /// for existing, while charging by distance makes "march on with poison in you" the real
    /// decision it should be.
    fn affliction_toll(&mut self, pid: &str) -> (f64, bool) {
        use meld_proto::statuses::{family_of, Family};
        let carried = match self.hero_afflictions.get(pid) {
            Some(c) => c,
            None => return (1.0, false),
        };
        let mut bound = false;
        let mut venom = false;
        for slot in carried {
            for name in slot {
                match family_of(name) {
                    Some(Family::Bindings) => bound = true,
                    Some(Family::Venom) => venom = true,
                    _ => {}
                }
            }
        }
        let drag = if bound {
            self.balance.affliction.bindings_move_mult.clamp(0.05, 1.0)
        } else {
            1.0
        };
        if !venom {
            return (drag, false);
        }
        let every = self.balance.affliction.venom_steps_per_tick.max(1);
        let n = self.venom_steps.entry(pid.to_string()).or_insert(0);
        *n += 1;
        let bites = *n % every == 0;
        (drag, bites)
    }

    /// Venom takes its bite out of every hero carrying it, and can kill the run.
    /// Returns whether it actually bit, so the caller can tell the player. A bite used to be
    /// entirely silent: HP came off the roster server-side and nothing was sent, so the party
    /// arrived at the next fight nearly dead with no explanation on the way there.
    fn venom_bites(&mut self, pid: &str) -> bool {
        use meld_proto::statuses::{family_of, Family};
        let dmg = self.balance.affliction.venom_hp_per_step.max(1);
        let poisoned: Vec<usize> = match self.hero_afflictions.get(pid) {
            Some(c) => c
                .iter()
                .enumerate()
                .filter(|(_, names)| {
                    names.iter().any(|n| family_of(n) == Some(Family::Venom))
                })
                .map(|(i, _)| i)
                .collect(),
            None => return false,
        };
        if poisoned.is_empty() {
            return false;
        }
        // Ground to a knee, never finished. Ending a run needs a
        // `WorldEffect::ReleaseFromRun`, which this call site cannot emit — `handle_move`
        // returns messages, not effects — so rather than half-wire a death path, venom floors
        // at 1 HP. It still bites: you arrive at the next fight nearly dead, which is a real
        // cost and the reason to carry a cure. Making a poison able to finish a party is a
        // follow-up, and it should go through the same teardown a defeat uses.
        let mut bit = false;
        if let Some(hps) = self.hero_hp.get_mut(pid) {
            for i in poisoned {
                if let Some(h) = hps.get_mut(i) {
                    if *h > 1 {
                        *h = (*h - dmg).max(1);
                        bit = true;
                    }
                }
            }
        }
        bit
    }

    fn handle_move(
        &mut self,
        player_id: &str,
        raw: RawEnvelope,
    ) -> (Vec<Outgoing>, Vec<WorldEffect>) {
        let intent: wm::MoveIntent = match serde_json::from_value(raw.payload) {
            Ok(v) => v,
            Err(_) => {
                return (
                    vec![error(
                        player_id,
                        ErrorCode::ValidationError,
                        "bad move_intent",
                        Some(raw.seq),
                    )],
                    Vec::new(),
                )
            }
        };
        // DG-3b: a player inside a dungeon moves within that space (its own
        // walkability, stairs, exit) — never the overworld arena. The no-world
        // guard is hoisted to the router, so `self` is the live world here.
        if self.dungeon_of(player_id).is_some() {
            let out = self.dungeon_move(player_id, &intent);
            // A trap wipe inside the dungeon ends the run — the Router releases the
            // session via a WorldEffect (world state was already updated by
            // `world_death` inside `dungeon_move`).
            let eff = if self.run_ended(player_id) {
                vec![WorldEffect::ReleaseFromRun(player_id.to_string())]
            } else {
                Vec::new()
            };
            return (out, eff);
        }
        // Any movement interrupts an in-progress harvest channel, exactly as it does
        // an extraction (D15): the input that breaks a channel is spent breaking it,
        // and every unit already banked stays banked.
        let harvest_broke = self.end_harvest(player_id, "moved");
        if !harvest_broke.is_empty() {
            return (harvest_broke, Vec::new());
        }
        // Raising a bench is a channel like any other: step away and it comes to nothing
        // (the stock stays spent — the materials went into the ground).
        let build_broke = self.end_building(player_id, "moved");
        if !build_broke.is_empty() {
            return (build_broke, Vec::new());
        }
        // Any movement interrupts an in-progress extraction channel (D15).
        if self.extraction.remove(player_id).is_some() {
            if let Some(a) = self.arena.avatar_mut(player_id) {
                a.state = "active".to_string();
            }
            let members: Vec<String> = self.run.runs.iter().map(|r| r.player_id.clone()).collect();
            return (
                members
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
                    .collect(),
                Vec::new(),
            );
        }
        // What is gripping the party is felt out here, not only in the arena. Afflictions do
        // not expire, so one caught in a fight follows the party down the road — and if the
        // road does not feel it, "you are poisoned" is a word on a HUD.
        let (drag, bleed) = self.affliction_toll(player_id);
        // Movement is ignored while in battle (avatar not `active`). A sub-unit direction is
        // used AS GIVEN by `apply_move` (it only normalises magnitudes above 1), so scaling it
        // is how being webbed or chilled slows a march.
        self.arena.apply_move(
            player_id,
            intent.move_dir.x * drag,
            intent.move_dir.y * drag,
            intent.input_seq,
        );
        // A bite re-sends the roster. That message already carries every hero's CURRENT HP
        // and afflictions, so the party strip, the over-head condition line and the client's
        // own "somebody just took damage" flash all come off one thing the client already
        // knows how to read — no new wire type for a number that has always existed.
        let bitten = bleed && self.venom_bites(player_id);

        let deeper = self.post_vanguard(player_id);

        // WG-4: crossing the western border behind the hub returns you to Last City
        // (an instant extraction home — you keep your backpack). `complete_extractions`
        // banks it this same tick and sends the result, so no touch/battle is resolved.
        if self.west_return(player_id) {
            return (Vec::new(), deeper);
        }

        // Contact starts a battle. Checked here for an instant response to the
        // player's own move, and again every tick (see `tick`) so a creature that
        // walks into a *stationary* player also triggers the fight — otherwise
        // standing still made you immune to an aggressive creature closing on you.
        let mut out = self.resolve_touches();
        if bitten {
            let (synergies, combos) = self.party_depth(player_id);
            out.push(out_msg(
                player_id,
                &wr::Party {
                    heroes: self.party_views(player_id),
                    synergies,
                    combos,
                    abilities: self.party_ability_views(player_id),
                },
            ));
        }
        (out, deeper)
    }

    /// Post the player's current distance to the Vanguard Board when it beats
    /// their run record (roadmap P1-1, behaviors/endgame-seasons.md).
    ///
    /// The run's `max_distance_reached` is the local high-water mark, so the DB
    /// write fires once per *new* deepest tile rather than on every move — and the
    /// number never comes from the client: it is read off the server-owned avatar
    /// after movement was validated (CANON §S anti-forgery).
    fn post_vanguard(&mut self, player_id: &str) -> Vec<WorldEffect> {
        let Some(d) = self
            .arena
            .avatar(player_id)
            .map(|a| a.position.distance_floor().clamp(0, i32::MAX as i64) as i32)
        else {
            return Vec::new();
        };
        let Some(run) = self
            .run
            .runs
            .iter_mut()
            .find(|r| r.player_id == player_id && r.result.is_none())
        else {
            return Vec::new();
        };
        if d <= run.max_distance_reached {
            return Vec::new();
        }
        run.max_distance_reached = d;
        // The board records HOW you got here, not only how far. 500 fights and 0 fights are
        // the same tile and completely different runs, and going quietly is a real way to
        // travel (see `unlocks`' Pacifist) rather than an exploit to close.
        let stamp = wr::VanguardStamp {
            distance: d,
            level: run.run_level,
            fights: run.fights,
            flees: run.flees,
        };
        let _ = self
            .db_writes
            .send(DbWrite::Vanguard(player_id.to_string(), stamp));
        // A depth hunt rides the same high-water mark, so it is asked once per new
        // deepest tile rather than on every step of the walk out — and so does the
        // Pacifist, which is the same question asked of the same moment.
        let fights = run.fights;
        vec![
            WorldEffect::Hunt {
                player_id: player_id.to_string(),
                fact: HuntFact::Depth(d),
            },
            WorldEffect::Milestone {
                player_id: player_id.to_string(),
                milestone: meld_proto::unlocks::Milestone::ReachedUntouched {
                    distance: d,
                    fights,
                },
            },
        ]
    }

    /// THE END FIGHT is down (EW, first cut). The reward, the omen, and the way home.
    ///
    /// Three insured pieces go into `looted_gear` and then the player is enqueued for an
    /// already-due extraction — the same route `west_return` uses — so the tested banking
    /// path carries everything home rather than this growing a second one. Heroes come back
    /// at level 1 because levels were only ever dive-scoped; nothing has to reset them.
    fn finish_end_fight(&mut self, player_id: &str) -> Vec<Outgoing> {
        let enc = self.balance.encounters.clone();
        let balance = self.balance.clone();
        let started = self.run.started_ms;
        let Some(run) = self
            .run
            .runs
            .iter_mut()
            .find(|r| r.player_id == player_id && r.result.is_none())
        else {
            return Vec::new();
        };
        // Insured, so it survives the walk home and every death after it: this is the one
        // thing in the game handed over for felling the world's own resistance.
        for n in 0..enc.end_fight_reward_pieces.max(0) {
            let seed = hash_str(&format!("{}-end-{n}", run.run_id));
            let slots = meld_proto::equipment::SLOTS;
            let slot = slots[(n as usize) % slots.len()];
            let class_key = meld_run::class_key(run.character_class);
            let g = meld_world::rolled_gear(
                &balance,
                enc.end_fight_reward_tier,
                "epic",
                0.0,
                slot,
                class_key,
                "ashfall",
                seed,
            );
            run.looted_gear.push(LootGear {
                gear_id: Uuid::now_v7().to_string(),
                name: g.name.clone(),
                rarity: g.rarity.clone(),
                slot: g.slot.clone(),
                class_key: g.class_key.clone(),
                insurance: g.insurance,
                tier: g.tier,
                atk_bonus: g.atk_bonus,
                def_bonus: g.def_bonus,
                spd_bonus: g.spd_bonus,
                base_max_durability: g.max_durability,
                max_durability: g.max_durability,
                equipped_hero_slot: None,
                damage_modifiers: g.damage_modifiers.clone(),
                family: g.family.clone(),
                armor_weight: g.armor_weight.clone(),
                affixes: g.affixes.clone(),
                unique_key: g.unique_key.clone(),
                set_key: g.set_key.clone(),
            });
        }
        let clear_ms = (now_ms() as i64 - started as i64).max(0);
        let stamp = wr::VanguardStamp {
            distance: run.max_distance_reached,
            level: run.run_level,
            fights: run.fights,
            flees: run.flees,
        };
        let pieces = enc.end_fight_reward_pieces;
        let _ = self.db_writes.send(DbWrite::WorldEnd(
            player_id.to_string(),
            stamp,
            clear_ms,
        ));
        // Home the tested way: an already-due extraction, banked next tick.
        self.extraction.insert(
            player_id.to_string(),
            Extraction { completes_at: now_ms(), method: "world_end".to_string() },
        );
        vec![out_msg(
            player_id,
            &wr::WorldEndFelled {
                // Deliberately unexplained. Three of them stood together and it changed
                // nothing about the ground — EW-4 is what answers this.
                omen: "Three of them fell together. The land is not stabilized."
                    .to_string(),
                clear_ms,
                pieces,
            },
        )]
    }

    /// WG-4: if the player has walked WEST of the return border (behind the hub),
    /// send them home to Last City — the safe anchor is always just to the west,
    /// "behind a giant wall you can always step back through." This *abandons* the
    /// run (backpack forfeited, no death penalty): near spawn there's nothing to
    /// lose, and from deep the long walk back west is impractical, so it is never a
    /// free extraction. The client routes the `abandoned` result to the City screen.
    fn west_return(&mut self, player_id: &str) -> bool {
        let border = self.balance.worldgen.west_return_border;
        // Radial-aware: "west" is the empty city wedge due-west of the hub, NOT a
        // straight x < border line (which would slice through explorable western
        // content in the 340° fan and extract a player merely walking over to a fight).
        let west = self.arena.heading_into_city(player_id, border);
        // Already heading home? don't re-enqueue.
        if !west || self.extraction.contains_key(player_id) {
            return false;
        }
        // The city is right there to the west — step back in and KEEP your backpack.
        // This is an INSTANT free extraction home (no channel, no death penalty, no
        // item cost): near spawn it's just "I changed my mind" and you shouldn't be
        // punished for it; from deep, walking all the way back west is its own
        // gauntlet, so it's a fair "fight your way home" route. `complete_extractions`
        // banks the backpack next tick (method != "town_portal", so nothing is spent).
        self.extraction.insert(
            player_id.to_string(),
            Extraction {
                completes_at: now_ms(),
                method: "west_return".to_string(),
            },
        );
        true
    }

    /// Opt into the nearby ongoing fight (`run.join_battle`). Validates that a
    /// battle is in progress, the caller isn't already in it, and their avatar is
    /// within `join_radius` of the fight — then merges their party in.
    fn handle_join_battle(
        &mut self,
        player_id: &str,
        raw: RawEnvelope,
    ) -> (Vec<Outgoing>, Vec<WorldEffect>) {
        let join_radius = self.balance.ai.join_radius;
        let (party_id, battle_id) = {
            if self.battles.is_empty() {
                return (vec![error(player_id, ErrorCode::InvalidState, "No fight in progress.", Some(raw.seq))], Vec::new());
            }
            let Some(pid) = self.party_id_of(player_id) else {
                return (vec![error(player_id, ErrorCode::NotFound, "No run for you.", Some(raw.seq))], Vec::new());
            };
            if self.battle_of_party(pid).is_some() {
                return (vec![error(player_id, ErrorCode::InvalidState, "You're already in a fight.", Some(raw.seq))], Vec::new());
            }
            let Some(pos) = self.arena.avatar(player_id).map(|a| a.position) else {
                return (vec![error(player_id, ErrorCode::NotFound, "No run for you.", Some(raw.seq))], Vec::new());
            };
            // Join the NEAREST battle within join_radius (concurrent battles: there
            // may be several going on around the map).
            let target = self
                .battles
                .iter()
                .map(|b| (b.battle_id.clone(), pos.distance_to(&b.pos)))
                .filter(|(_, d)| *d <= join_radius)
                .min_by(|a, b| a.1.total_cmp(&b.1))
                .map(|(id, _)| id);
            let Some(battle_id) = target else {
                return (vec![error(player_id, ErrorCode::OutOfRange, "Too far from any fight to join.", Some(raw.seq))], Vec::new());
            };
            (pid, battle_id)
        };
        // Opting into a fight breaks whatever you were channeling, same as being
        // dragged into one does.
        let mut out = self.end_harvest(player_id, "battle_started");
        // Stepping in ends watching: you are in it now, and the feed you were reading
        // is the fight you are standing in.
        out.extend(self.stop_watching(player_id, "own_battle"));
        out.extend(self.join_battle(player_id, party_id, &battle_id));
        (out, Vec::new())
    }

    /// WATCH the nearest fight in reach (`run.watch_battle`, `SOC-3`). Two things can be
    /// watched and both arrive as the same feed: another player's battle, or a
    /// creature-vs-creature clash (`CR-2`).
    ///
    /// Watching costs nothing and commits nothing — which is the whole point, and why
    /// its radius is wider than `join_radius`. Refused only when the caller is in a
    /// fight of their own (you cannot watch and swing) or is inside a dungeon, which is
    /// a committed space with its own screen.
    fn handle_watch_battle(
        &mut self,
        player_id: &str,
        raw: RawEnvelope,
    ) -> (Vec<Outgoing>, Vec<WorldEffect>) {
        if self.location.contains_key(player_id) {
            return (
                vec![error(player_id, ErrorCode::InvalidState, "Not out here.", Some(raw.seq))],
                Vec::new(),
            );
        }
        let Some(my_party) = self.party_id_of(player_id) else {
            return (
                vec![error(player_id, ErrorCode::NotFound, "No run for you.", Some(raw.seq))],
                Vec::new(),
            );
        };
        if self.battle_of_party(my_party).is_some() {
            return (
                vec![error(player_id, ErrorCode::InvalidState, "You're in a fight of your own.", Some(raw.seq))],
                Vec::new(),
            );
        }
        let Some(pos) = self.arena.avatar(player_id).map(|a| a.position) else {
            return (
                vec![error(player_id, ErrorCode::NotFound, "No run for you.", Some(raw.seq))],
                Vec::new(),
            );
        };
        let reach = self.balance.ai.watch_radius;
        // Nearest of BOTH kinds, compared in one pass: whichever fight is closest is the
        // one you meant, and a player brawl standing beside a creature brawl should not
        // resolve by which arm of an `if` came first.
        let nearest_battle = self
            .battles
            .iter()
            .filter(|b| b.dungeon.is_none() && !b.parties.contains(&my_party))
            .map(|b| (pos.distance_to(&b.pos), WatchFeed::Battle(b.battle_id.clone())))
            .filter(|(d, _)| *d <= reach)
            .min_by(|a, b| a.0.total_cmp(&b.0));
        let nearest_clash = self
            .arena
            .clashes
            .iter()
            .filter_map(|c| {
                let anchor = c.anchor()?.clone();
                Some((pos.distance_to(&c.position), WatchFeed::Clash { anchor, roster: Vec::new() }))
            })
            .filter(|(d, _)| *d <= reach)
            .min_by(|a, b| a.0.total_cmp(&b.0));
        let feed = match (nearest_battle, nearest_clash) {
            (Some(a), Some(b)) => Some(if a.0 <= b.0 { a.1 } else { b.1 }),
            (Some(a), None) => Some(a.1),
            (None, Some(b)) => Some(b.1),
            (None, None) => None,
        };
        let Some(feed) = feed else {
            return (
                vec![error(player_id, ErrorCode::OutOfRange, "No fight close enough to watch.", Some(raw.seq))],
                Vec::new(),
            );
        };
        // Re-aiming at what you are already watching is a no-op, not a re-send: the
        // client would otherwise rebuild its battle screen on every keypress.
        if self.watching.get(player_id).is_some_and(|f| f.battle_id() == feed.battle_id()) {
            return (Vec::new(), Vec::new());
        }
        let mut out = self.stop_watching(player_id, "stopped");
        out.extend(self.open_watch(player_id, feed));
        (out, Vec::new())
    }

    /// `run.stop_watching`. Idempotent by design — the client fires it off the same key
    /// that opened the feed, so "watching nothing" is an answer, not an error.
    fn handle_stop_watching(
        &mut self,
        player_id: &str,
        _raw: RawEnvelope,
    ) -> (Vec<Outgoing>, Vec<WorldEffect>) {
        (self.stop_watching(player_id, "stopped"), Vec::new())
    }

    /// Point `player_id` at `feed` and send them its opening roster. For a player battle
    /// this also enrolls them in that slot's `spectators`, which is what puts them on the
    /// audience funnel — from here on they receive the fight's own messages and this side
    /// has nothing per-message to remember.
    fn open_watch(&mut self, player_id: &str, feed: WatchFeed) -> Vec<Outgoing> {
        let battle_id = feed.battle_id();
        let (allies, enemies) = match &feed {
            WatchFeed::Battle(id) => {
                let Some(slot) = self.battles.iter().find(|b| &b.battle_id == id) else {
                    return Vec::new();
                };
                let (mut allies, enemies) = slot.battle.wire_combatants();
                inject_hero_names(&slot.player_combatants, &self.hero_names, &mut allies);
                (allies, enemies)
            }
            // A clash has no "our side": both mobs are somebody else's problem, so every
            // body lands on the enemy side and the arena draws them as the knot they are.
            WatchFeed::Clash { anchor, .. } => (Vec::new(), self.clash_combatants(anchor)),
        };
        if enemies.is_empty() && allies.is_empty() {
            return Vec::new();
        }
        let feed = match feed {
            WatchFeed::Clash { anchor, .. } => WatchFeed::Clash {
                roster: enemies.iter().map(|c| c.combatant_id.clone()).collect(),
                anchor,
            },
            other => other,
        };
        if let WatchFeed::Battle(id) = &feed {
            if let Some(slot) = self.battles.iter_mut().find(|b| &b.battle_id == id) {
                slot.spectators.insert(player_id.to_string());
            }
        }
        self.watching.insert(player_id.to_string(), feed);
        vec![out_msg(
            player_id,
            &wb::Started {
                battle_id,
                encounter_class: EncounterClass::Standard,
                allies,
                enemies,
                your_combatant_id: String::new(),
                your_combatant_ids: Vec::new(),
                triggered_by: None,
                spectating: true,
            },
        )]
    }

    /// Close `player_id`'s feed, if they have one, and say why. Also un-enrolls them from
    /// the battle slot's `spectators` so a stale entry can never keep broadcasting to
    /// somebody who has walked away.
    fn stop_watching(&mut self, player_id: &str, reason: &str) -> Vec<Outgoing> {
        let Some(feed) = self.watching.remove(player_id) else {
            return Vec::new();
        };
        if let WatchFeed::Battle(id) = &feed {
            if let Some(slot) = self.battles.iter_mut().find(|b| &b.battle_id == id) {
                slot.spectators.remove(player_id);
            }
        }
        vec![out_msg(
            player_id,
            &wb::WatchEnded { battle_id: feed.battle_id(), reason: reason.to_string() },
        )]
    }

    /// Every creature swinging in the clash anchored on `anchor`, as combatants. Their
    /// gauge is the wind-up to their next blow (`skirmish_cd` against its interval), so a
    /// watched clash reads as a fight with timing rather than HP bars twitching at random.
    fn clash_combatants(&self, anchor: &str) -> Vec<Combatant> {
        let interval = self.balance.ai.skirmish_attack_interval.max(0.001);
        let Some(clash) = self.arena.clash_of(anchor) else {
            return Vec::new();
        };
        clash
            .members
            .iter()
            .filter_map(|id| self.arena.monsters.iter().find(|m| &m.entity_id == id))
            .map(|m| Combatant {
                combatant_id: m.entity_id.clone(),
                kind: CombatantKind::Monster,
                player_id: None,
                monster_kind: Some(m.monster_kind.clone()),
                level: m.level,
                hp: m.hp,
                max_hp: m.max_hp,
                gauge: (1.0 - (m.skirmish_cd / interval)).clamp(0.0, 1.0),
                // The faction rides the wire so the two sides of a brawl are legible as
                // sides. Same `key:value` convention every other per-combatant extra uses.
                statuses: vec![format!("faction:{}", m.faction)],
            })
            .collect()
    }

    /// Per-tick upkeep for every watcher (`SOC-3`): drop the feeds that are no longer
    /// watchable, and drive the ones this side has to drive itself.
    ///
    /// A player battle needs nothing here — the watcher is on its audience funnel, so it
    /// streams itself. A CLASH has no engine behind it, so its gauges and HP are sent
    /// from here; its roster is re-sent only when it actually changes, because a
    /// `battle.started` every tick would rebuild the client's battle screen ten times a
    /// second.
    fn sweep_watchers(&mut self) -> Vec<Outgoing> {
        let mut out = Vec::new();
        let reach = self.balance.ai.watch_radius;
        let watchers: Vec<(String, WatchFeed)> =
            self.watching.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
        for (pid, feed) in watchers {
            // Gone from the overworld entirely — the run ended, or they descended into a
            // dungeon. Nothing to watch from in there, and the reason is not "your own
            // fight": they simply stopped standing where the feed made sense.
            let Some(mine) = self.party_id_of(&pid) else {
                out.extend(self.stop_watching(&pid, "stopped"));
                continue;
            };
            if self.location.contains_key(&pid) {
                out.extend(self.stop_watching(&pid, "stopped"));
                continue;
            }
            // Pulled into (or opted into) a fight of their own. The client must be told
            // WHY, because it has just been sent its own `battle.started` and must not act
            // on this one — see the note on `battle.watch_ended`.
            if self.battle_of_party(mine).is_some() {
                out.extend(self.stop_watching(&pid, "own_battle"));
                continue;
            }
            let Some(me) = self.arena.avatar(&pid).map(|a| a.position) else {
                out.extend(self.stop_watching(&pid, "stopped"));
                continue;
            };
            match feed {
                WatchFeed::Battle(id) => {
                    let Some(at) = self.battles.iter().find(|b| b.battle_id == id).map(|b| b.pos)
                    else {
                        // The slot is gone, so the fight is over. `battle.ended` never
                        // reaches a watcher (it carries somebody else's XP and haul), so
                        // this is the message that closes their screen.
                        out.extend(self.stop_watching(&pid, "finished"));
                        continue;
                    };
                    if me.distance_to(&at) > reach {
                        out.extend(self.stop_watching(&pid, "out_of_range"));
                    }
                }
                WatchFeed::Clash { anchor, roster } => {
                    let Some(at) = self.arena.clash_of(&anchor).map(|c| c.position) else {
                        out.extend(self.stop_watching(&pid, "finished"));
                        continue;
                    };
                    if me.distance_to(&at) > reach {
                        out.extend(self.stop_watching(&pid, "out_of_range"));
                        continue;
                    }
                    let now = self.clash_combatants(&anchor);
                    let ids: Vec<String> = now.iter().map(|c| c.combatant_id.clone()).collect();
                    if ids != roster {
                        // Somebody joined the brawl or fell out of it: re-send the roster
                        // so the arena matches the fight, and remember it so the next tick
                        // is quiet again.
                        self.watching.insert(
                            pid.clone(),
                            WatchFeed::Clash { anchor: anchor.clone(), roster: ids },
                        );
                        out.push(out_msg(
                            &pid,
                            &wb::Started {
                                battle_id: format!("clash:{anchor}"),
                                encounter_class: EncounterClass::Standard,
                                allies: Vec::new(),
                                enemies: now,
                                your_combatant_id: String::new(),
                                your_combatant_ids: Vec::new(),
                                triggered_by: None,
                                spectating: true,
                            },
                        ));
                        continue;
                    }
                    out.push(out_msg(
                        &pid,
                        &wb::GaugeUpdate {
                            battle_id: format!("clash:{anchor}"),
                            server_tick: self.tick_count as i64,
                            combatants: now
                                .into_iter()
                                .map(|c| wb::GaugeEntry {
                                    combatant_id: c.combatant_id,
                                    gauge: c.gauge,
                                    hp: c.hp,
                                    statuses: c.statuses,
                                })
                                .collect(),
                        },
                    ));
                }
            }
        }
        out
    }

}

impl WorldActor {
    /// Rename one of the caller's heroes: update the active run's names + the
    /// session cache (for the next dive, via a `SetSessionHeroName` effect),
    /// persist to Postgres, and re-send the roster so the party panel updates at
    /// once.
    fn handle_rename_hero(
        &mut self,
        player_id: &str,
        raw: RawEnvelope,
    ) -> (Vec<Outgoing>, Vec<WorldEffect>) {
        let req: wr::RenameHero = match serde_json::from_value(raw.payload) {
            Ok(v) => v,
            Err(_) => {
                return (
                    vec![error(player_id, ErrorCode::ValidationError, "bad rename_hero", Some(raw.seq))],
                    Vec::new(),
                )
            }
        };
        let party_size = self.balance.battle.party_size_per_player.max(1) as i32;
        let name: String = req.name.trim().chars().take(24).collect();
        if name.is_empty() || req.slot < 0 || req.slot >= party_size {
            return (
                vec![error(player_id, ErrorCode::ValidationError, "Invalid hero name or slot.", Some(raw.seq))],
                Vec::new(),
            );
        }
        let slot = req.slot as usize;
        // Active run's names (so battle + panel reflect it now).
        if let Some(names) = self.hero_names.get_mut(player_id) {
            if let Some(n) = names.get_mut(slot) {
                *n = name.clone();
            }
        }
        // Session cache (used to form the next dive) — Router-owned, so emit an effect.
        let effects = vec![WorldEffect::SetSessionHeroName {
            player_id: player_id.to_string(),
            slot,
            name: name.clone(),
        }];
        let _ = self
            .db_writes
            .send(DbWrite::HeroRename(player_id.to_string(), slot as i16, name));
        (
            vec![out_msg(
                player_id,
                &{
                    let (synergies, combos) = self.party_depth(player_id);
                    wr::Party { heroes: self.party_views(player_id), synergies, combos, abilities: self.party_ability_views(player_id) }
                },
            )],
            effects,
        )
    }

    /// Set one of the caller's heroes to the front or back row: update the active
    /// run's formation + the session cache (for the next dive, via a
    /// `SetSessionHeroRow` effect), persist to Postgres, and re-send the roster so
    /// the party panel updates at once. Applies to the next battle assembled (an
    /// in-progress battle's Fighters are already built).
    fn handle_set_formation(
        &mut self,
        player_id: &str,
        raw: RawEnvelope,
    ) -> (Vec<Outgoing>, Vec<WorldEffect>) {
        let req: wr::SetFormation = match serde_json::from_value(raw.payload) {
            Ok(v) => v,
            Err(_) => {
                return (
                    vec![error(player_id, ErrorCode::ValidationError, "bad set_formation", Some(raw.seq))],
                    Vec::new(),
                )
            }
        };
        let party_size = self.balance.battle.party_size_per_player.max(1) as i32;
        if req.slot < 0 || req.slot >= party_size {
            return (
                vec![error(player_id, ErrorCode::ValidationError, "Invalid hero slot.", Some(raw.seq))],
                Vec::new(),
            );
        }
        let slot = req.slot as usize;
        let back = req.back_row;
        // Active run's formation (so the panel + next battle reflect it).
        let rows = self.hero_rows.entry(player_id.to_string()).or_default();
        while rows.len() <= slot {
            rows.push(false);
        }
        rows[slot] = back;
        // Session cache (used to form the next dive) — Router-owned, so emit an effect.
        let effects = vec![WorldEffect::SetSessionHeroRow {
            player_id: player_id.to_string(),
            slot,
            back,
        }];
        let _ = self
            .db_writes
            .send(DbWrite::HeroFormation(player_id.to_string(), slot as i16, back));
        (
            vec![out_msg(
                player_id,
                &{
                    let (synergies, combos) = self.party_depth(player_id);
                    wr::Party { heroes: self.party_views(player_id), synergies, combos, abilities: self.party_ability_views(player_id) }
                },
            )],
            effects,
        )
    }
}

impl WorldActor {
    /// Equip (or unequip) a piece of this run's not-yet-banked loot gear onto a
    /// hero slot. Unlike Vault equip (HTTP, persists to Postgres, effective
    /// from the next dive), this only touches the in-memory run — no DB write,
    /// since red gear isn't owned until extraction anyway — and takes effect
    /// on the caller's very next battle via `effective_gear_bonus`.
    fn handle_equip_loot(
        &mut self,
        player_id: &str,
        raw: RawEnvelope,
    ) -> (Vec<Outgoing>, Vec<WorldEffect>) {
        let req: wr::EquipLoot = match serde_json::from_value(raw.payload) {
            Ok(v) => v,
            Err(_) => {
                return (vec![error(player_id, ErrorCode::ValidationError, "bad equip_loot", Some(raw.seq))], Vec::new())
            }
        };
        if let Some(slot) = req.hero_slot {
            let party_size = self.balance.battle.party_size_per_player.max(1) as i32;
            if slot < 0 || slot >= party_size {
                return (vec![error(player_id, ErrorCode::ValidationError, "Invalid hero slot.", Some(raw.seq))], Vec::new());
            }
        }
        // This dive's class for the target slot (if equipping) — class isn't
        // persisted per hero, only chosen per dive, so `party_classes` (set at
        // `enter_maze`) is the only place this run's actual class lives.
        let hero_class: Option<String> = req.hero_slot.and_then(|slot| {
            self.party_classes
                .get(player_id)
                .and_then(|v| v.get(slot as usize))
                .map(|c| meld_run::class_key(*c).to_string())
        });
        let Some(r) = self.run.run_mut(player_id) else {
            return (vec![error(player_id, ErrorCode::InvalidState, "Not in a run.", Some(raw.seq))], Vec::new());
        };
        let Some(idx) = r.looted_gear.iter().position(|g| g.gear_id == req.gear_id) else {
            return (vec![error(player_id, ErrorCode::NotFound, "No such loot gear.", Some(raw.seq))], Vec::new());
        };
        if let Some(slot) = req.hero_slot {
            let item_class = &r.looted_gear[idx].class_key;
            if !item_class.is_empty() && hero_class.as_deref() != Some(item_class.as_str()) {
                return (vec![error(
                    player_id,
                    ErrorCode::ValidationError,
                    "Wrong class for this item.",
                    Some(raw.seq),
                )], Vec::new());
            }
            // One item per hero+category: unequip anything else this hero has
            // worn this run in the same category before wearing the new one.
            let category = r.looted_gear[idx].slot.clone();
            // Per-(hero, category) capacity mirrors the Vault rule: two
            // accessory slots (ACCESSORY_1/2), one of everything else. When
            // full, the oldest worn piece makes room for the new one.
            let cap = if category == "accessory" { 2 } else { 1 };
            loop {
                let worn: Vec<usize> = r
                    .looted_gear
                    .iter()
                    .enumerate()
                    .filter(|(i, g)| {
                        *i != idx && g.slot == category && g.equipped_hero_slot == Some(slot)
                    })
                    .map(|(i, _)| i)
                    .collect();
                if worn.len() < cap {
                    break;
                }
                r.looted_gear[worn[0]].equipped_hero_slot = None;
            }
            r.looted_gear[idx].equipped_hero_slot = Some(slot);
        } else {
            r.looted_gear[idx].equipped_hero_slot = None;
        }
        (vec![out_msg(player_id, &wr::RunGear { gear: r.looted_gear.clone() })], Vec::new())
    }

    /// Merge a party into the in-progress battle (the toucher opted in via
    /// `run.join_battle`). The joiner brings their full hero composition, exactly
    /// as if they'd started the fight.
    fn join_battle(&mut self, toucher: &str, party_id: u32, battle_id: &str) -> Vec<Outgoing> {
        let balance = self.balance.clone();
        let cap =
            meld_proto::limits::PARTY_MAX * self.balance.battle.merge_cap_normal_instances.max(1) as usize;
        // Read gear from the world's own synced mirror (same data a live session
        // read returned; see `WorldActor::gear_bonuses`).
        let bonuses: HashMap<String, Vec<GearBonus>> = self.gear_bonuses.clone();
        let edges = self.edges.clone();
        if self.battle_by_id(battle_id).is_none() {
            return Vec::new();
        }

        // Build the joining party's combatants — the joiner's full hero
        // composition (parallel to `hero_hp`), just like starting a fight. These go
        // into the target battle's own combatant maps.
        let joiners: Vec<String> = self
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
            let r_ref = self.run.runs.iter().find(|r| &r.player_id == pid);
            let lead = r_ref.map(|r| r.character_class).unwrap_or(CharacterClass::Explorer);
            let looted = r_ref.map(|r| r.looted_gear.as_slice()).unwrap_or(&[]);
            let comp = self
                .party_classes
                .get(pid)
                .cloned()
                .unwrap_or_else(|| vec![lead]);
            let hp_vec = self.hero_hp.get(pid).cloned().unwrap_or_default();
            let row_vec = self.hero_rows.get(pid).cloned().unwrap_or_default();
            let hero_bonuses = bonuses.get(pid);
            let mut cids = Vec::new();
            for (slot, cls) in comp.iter().enumerate() {
                let cid = Uuid::now_v7().to_string();
                add_combatant_player.insert(cid.clone(), pid.clone());
                // Each hero wears their own gear (per-character equip slots).
                let vault_bonus = hero_bonuses.and_then(|v| v.get(slot)).cloned().unwrap_or_default();
                let bonus = effective_gear_bonus(
                    vault_bonus,
                    looted,
                    slot as i32,
                    edges.get(pid).and_then(|v| v.get(slot)),
                );
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
        let current = self.battle_by_id(battle_id).unwrap().battle.player_count();
        if current + party.len() > cap {
            return Vec::new();
        }

        let mut fighters = meld_run::party_fighters(&party, &self.run, &balance, &row_overrides);
        // Carry each joining hero's persisted HP into the merged battle.
        for (f, hp) in fighters.iter_mut().zip(hp_overrides.iter()) {
            if let Some(h) = hp {
                f.hp = (*h).clamp(0, f.max_hp);
            }
        }

        // Apply to the target battle slot, then extract what messaging needs.
        let (encounter_class, mut allies, enemies, joined_pc) = {
            let slot = self.battle_by_id_mut(battle_id).unwrap();
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
        inject_hero_names(&joined_pc, &self.hero_names, &mut allies);
        for pid in &joiners {
            if let Some(a) = self.arena.avatar_mut(pid) {
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
                    spectating: false,
                },
            ));
        }
        // battle.party_joined (delta) to everyone already in the battle.
        let members = self
            .battle_by_id(&battle_id)
            .map(|s| self.audience_of(s))
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
                    joining_instance_id: self.run.instance_id.clone(),
                    joining_allies: joining_allies.clone(),
                },
            ));
        }
        out
    }

    fn handle_submit(
        &mut self,
        player_id: &str,
        raw: RawEnvelope,
    ) -> (Vec<Outgoing>, Vec<WorldEffect>) {
        let submit: wb::SubmitAction = match serde_json::from_value(raw.payload) {
            Ok(v) => v,
            Err(_) => {
                return (
                    vec![error(
                        player_id,
                        ErrorCode::ValidationError,
                        "bad submit_action",
                        Some(raw.seq),
                    )],
                    Vec::new(),
                )
            }
        };
        // Route to the battle named in the request, and only if the sender is
        // actually in it (with concurrent battles, the id disambiguates which one).
        let owned = match self.battle_by_id(&submit.battle_id) {
            Some(slot) => slot.player_combatants.get(player_id).cloned().unwrap_or_default(),
            None => {
                return (
                    vec![error(
                        player_id,
                        ErrorCode::NotFound,
                        "Unknown battle.",
                        Some(raw.seq),
                    )],
                    Vec::new(),
                )
            }
        };
        // The actor must be one of the sender's own combatants; default to their
        // first hero when the client doesn't name one (back-compat). This is also what
        // refuses a WATCHER (`SOC-3`): a spectator is on the battle's audience funnel but
        // owns nothing in it, so `owned` is empty and every action they could send lands
        // on "Not a combatant." — no separate spectator guard to keep in sync.
        let actor_cid = match &submit.actor_combatant_id {
            Some(cid) if owned.contains(cid) => cid.clone(),
            Some(_) => {
                return (
                    vec![error(
                        player_id,
                        ErrorCode::ValidationError,
                        "That combatant is not yours.",
                        Some(raw.seq),
                    )],
                    Vec::new(),
                )
            }
            None => match owned.first() {
                Some(cid) => cid.clone(),
                None => {
                    return (
                        vec![error(
                            player_id,
                            ErrorCode::NotFound,
                            "Not a combatant.",
                            Some(raw.seq),
                        )],
                        Vec::new(),
                    )
                }
            },
        };

        // Battle heal items are FINITE (inventory-backed): an Item action consumes one
        // of its item from the run backpack, and is rejected when you're out. Checked
        // BEFORE submit (reject), spent AFTER it's accepted.
        let consume_kind: Option<String> = if submit.action == BattleActionKind::Item {
            submit.item_id.clone()
        } else {
            None
        };
        // Which of the sender's heroes is acting. `owned` is built in party-slot order
        // (see `form_battle`), so its index IS the hero slot — and the hero slot is what
        // owns a pouch.
        let actor_slot = owned.iter().position(|c| c == &actor_cid).unwrap_or(0);
        if let Some(kind) = &consume_kind {
            // A hero may only drink what IT is carrying. The shared bag is out of reach
            // in a fight, so being out of heals on the hero whose turn it is is a real
            // outcome rather than a lookup that quietly succeeds from the party's stock.
            let have = self
                .run
                .run_mut(player_id)
                .map_or(0, |r| r.pouch_qty(actor_slot, kind));
            if have <= 0 {
                let name = kind.replace('_', " ");
                return (
                    vec![error(
                        player_id,
                        ErrorCode::ValidationError,
                        format!("This hero is not carrying {name}."),
                        Some(raw.seq),
                    )],
                    Vec::new(),
                );
            }
        }
        let result = {
            let battle = &mut self.battle_by_id_mut(&submit.battle_id).unwrap().battle;
            battle.submit(
                &actor_cid,
                submit.action_id.clone(),
                submit.action,
                submit.target_ids.clone(),
                submit.skill_kind.clone(),
                submit.item_id.clone(),
            )
        };
        match result {
            Ok(events) => {
                let mut out = Vec::new();
                // Accepted → spend one of the item and tell the client (so its count
                // ticks down and the menu can grey it out at zero).
                if let Some(kind) = consume_kind {
                    if let Some(r) = self.run.run_mut(player_id) {
                        r.spend_from_pouch(actor_slot, &kind);
                    }
                    // The pouch is what changed, not the bag — a `backpack_update` here
                    // would decrement a bag stack the client does not have and leave its
                    // mirror short by one.
                    out.extend(self.pouches_msg(player_id));
                }
                let (evout, eff) = self.emit_battle_events(&submit.battle_id, events);
                out.extend(evout);
                // The world can't apply Router-scoped effects (release-from-run
                // touches sessions); hand them back to the Router to apply.
                (out, eff)
            }
            Err(reject) => {
                let (code, message) = reject_to_error(&reject);
                (vec![error(player_id, code, message, Some(raw.seq))], Vec::new())
            }
        }
    }

}

impl WorldActor {
    fn handle_begin_extraction(
        &mut self,
        player_id: &str,
        raw: RawEnvelope,
    ) -> (Vec<Outgoing>, Vec<WorldEffect>) {
        let req: wr::BeginExtraction = match serde_json::from_value(raw.payload) {
            Ok(v) => v,
            Err(_) => {
                return (
                    vec![error(
                        player_id,
                        ErrorCode::ValidationError,
                        "bad begin_extraction",
                        Some(raw.seq),
                    )],
                    Vec::new(),
                )
            }
        };
        let now = now_ms();
        let channel_ms = self.balance.runs.extraction_channel_ms;
        if self.battle_of_player(player_id).is_some() {
            return (
                vec![error(
                    player_id,
                    ErrorCode::InvalidState,
                    "Resolve the battle first.",
                    Some(raw.seq),
                )],
                Vec::new(),
            );
        }
        // DG-3b: a dungeon is a committed space (design §4) — no Town Portal inside.
        // Exception: the guided [T]-dive walkthrough's completion screen offers
        // "Go back to town" from inside its one forced tutorial dungeon (DG-3-
        // tutorial) — a normal dive never reaches this branch, since it's gated
        // on the whole world being tutorial-flagged, not on which dungeon.
        if !self.tutorial && self.dungeon_of(player_id).is_some() {
            return (
                vec![error(
                    player_id,
                    ErrorCode::InvalidState,
                    "Can't extract from inside a dungeon — reach the exit.",
                    Some(raw.seq),
                )],
                Vec::new(),
            );
        }
        if self.extraction.contains_key(player_id) {
            return (
                vec![error(
                    player_id,
                    ErrorCode::InvalidState,
                    "Already channeling.",
                    Some(raw.seq),
                )],
                Vec::new(),
            );
        }
        // "portal" requires standing at the single deep portal; "town_portal"
        // works anywhere but requires a Town Portal item (consumed on completion).
        match req.method.as_str() {
            "portal" => {
                if !self.arena.at_portal(player_id) {
                    return (
                        vec![error(
                            player_id,
                            ErrorCode::OutOfRange,
                            "Not at the extraction portal.",
                            Some(raw.seq),
                        )],
                        Vec::new(),
                    );
                }
            }
            "town_portal" => {
                let has = self
                    .run
                    .run_mut(player_id)
                    .is_some_and(|r| r.backpack.iter().any(|i| i.item_kind == TOWN_PORTAL));
                if !has {
                    return (
                        vec![error(
                            player_id,
                            ErrorCode::InvalidState,
                            "No Town Portal item.",
                            Some(raw.seq),
                        )],
                        Vec::new(),
                    );
                }
            }
            _ => {
                return (
                    vec![error(
                        player_id,
                        ErrorCode::ValidationError,
                        "unknown extraction method",
                        Some(raw.seq),
                    )],
                    Vec::new(),
                )
            }
        }
        let completes_at = now + channel_ms;
        self.extraction.insert(
            player_id.to_string(),
            Extraction {
                completes_at,
                method: req.method.clone(),
            },
        );
        if let Some(a) = self.arena.avatar_mut(player_id) {
            a.state = "channeling".to_string();
        }
        let members: Vec<String> = self.run.runs.iter().map(|r| r.player_id.clone()).collect();
        let msgs: Vec<Outgoing> = members
            .iter()
            .map(|pid| {
                out_msg(
                    pid,
                    &wr::ChannelStarted {
                        client_seq: if pid == player_id { Some(raw.seq) } else { None },
                        player_id: player_id.to_string(),
                        method: req.method.clone(),
                        completes_at,
                        fill_ms: channel_ms,
                    },
                )
            })
            .collect();
        (msgs, Vec::new())
    }
}

impl GameState {
    /// Load equipped-gear bonuses (per hero slot) for freshly-connected players.
    /// Passes each slot's class *for this dive* (empty before any `enter_maze`)
    /// so class-restricted gear only contributes when it actually matches —
    /// see `Db::equipped_gear_bonuses`.
    async fn flush_gear_loads(&mut self) {
        let loads: Vec<String> = std::mem::take(&mut self.pending_gear_load);
        let party_size = self.balance.battle.party_size_per_player as i32;
        for pid in loads {
            if let Ok(uid) = Uuid::parse_str(&pid) {
                let hero_classes: Vec<String> = self
                    .world
                    .as_ref()
                    .and_then(|inst| inst.party_classes.get(&pid))
                    .map(|classes| classes.iter().map(|c| meld_run::class_key(*c).to_string()).collect())
                    .unwrap_or_default();
                if let Ok(bonuses) = self.db.equipped_gear_bonuses(uid, party_size, &hero_classes).await {
                    if let Some(s) = self.sessions.get_mut(&pid) {
                        s.gear_bonuses = bonuses.clone();
                    }
                    // Keep the world-local mirror in lock-step for any player who is
                    // currently a member of the running world. This is what makes the
                    // world copy behaviour-identical to a live session read: gear only
                    // changes here (and at form_run), and this flush runs AFTER `tick`
                    // in the loop, so a moved method never sees a stale copy mid-tick.
                    if let Some(w) = self.world.as_mut() {
                        if w.run.runs.iter().any(|r| r.player_id == pid) {
                            // Re-dress on the way in. THIS is where the dev flag used to
                            // die: `form_run` dressed the party, and this line — a tick
                            // later, with the real (often empty) Vault — put the undressed
                            // set straight back. The flag held for as long as it took the
                            // first gear load to land, and never longer.
                            let classes =
                                w.party_classes.get(&pid).cloned().unwrap_or_default();
                            let bonuses = dress_for_dev(&w.balance, &classes, bonuses);
                            w.gear_bonuses.insert(pid.clone(), bonuses);
                        }
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

    /// Backfill any of a player's hero slots missing a piece of gear in some
    /// category with a permanent, class-unrestricted starter piece
    /// (`Db::ensure_starter_gear`) right before a dive forms. The write only
    /// needs to land before `form_run` runs (moments later, in the same
    /// call) — `gear_bonuses` itself is refreshed by the existing
    /// queue-drained-next-tick reload `form_run` already triggers
    /// (`pending_gear_load`), same as any other Vault equip change.
    async fn ensure_starter_gear_for(&mut self, pid: &str) {
        if let Ok(uid) = Uuid::parse_str(pid) {
            let party_size = self.balance.battle.party_size_per_player.max(1) as i32;
            if let Err(e) = self.db.ensure_starter_gear(uid, party_size).await {
                tracing::error!("ensure_starter_gear failed for {pid}: {e}");
            }
        }
    }

    /// A smith job at the CITY anvil: no station, and the caller is their own smith.
    /// Their persistent Forging level is looked up on the flush like everything else,
    /// so the only gate here is that the request is well-formed.
    fn handle_anvil_request(&mut self, player_id: &str, raw: RawEnvelope) -> Vec<Outgoing> {
        let seq = raw.seq;
        let req: wr::SmithRequest = match serde_json::from_value(raw.payload) {
            Ok(v) => v,
            Err(_) => {
                return vec![error(player_id, ErrorCode::ValidationError, "bad smith_request", Some(seq))]
            }
        };
        if !matches!(req.service.as_str(), "reroll" | "repair" | "enhance") {
            return vec![error(player_id, ErrorCode::ValidationError, "No such service.", Some(seq))];
        }
        // An edge dies with the dive, so buying one in town would be buying nothing.
        if req.service == "enhance" {
            return vec![error(
                player_id,
                ErrorCode::InvalidState,
                "An edge only lasts a dive - ask a smith in the field.",
                Some(seq),
            )];
        }
        let level = self
            .sessions
            .get(player_id)
            .and_then(|s| s.forging_level)
            .unwrap_or(1);
        self.open_heat(SmithJob {
            requester: player_id.to_string(),
            owner: player_id.to_string(),
            kind: "smith".to_string(),
            smith_level: level,
            crew: 0,
            station_id: String::new(),
            gear_id: req.gear_id,
            service: req.service,
            material: req.material,
            recipe: String::new(),
            client_seq: seq,
            quality: 0.0,
        });
        Vec::new()
    }

    /// Open a heat for an accepted smith job: lay out the bar and hand it to the smith.
    /// Nothing is spent yet — a heat that is never struck costs nothing but the walk.
    fn open_heat(&mut self, job: SmithJob) {
        let tier = self
            .open_heat_tier(&job)
            .unwrap_or(0);
        self.next_job = self.next_job.wrapping_add(1);
        let job_id = format!("heat-{}", self.next_job);
        let heat = meld_world::tempo::schedule(
            &self.balance,
            tier,
            job.smith_level,
            job.crew,
            now_ms() ^ hash_str(&job_id),
        );
        let started = wr::TempoStarted {
            job_id: job_id.clone(),
            service: job.service.clone(),
            strikes: heat.strikes,
            sweep_ms: heat.sweep_ms,
            bands: heat
                .bands
                .iter()
                .map(|b| wr::TempoBand { lo: b.lo, hi: b.hi })
                .collect(),
        };
        let requester = job.requester.clone();
        self.open_heats.insert(
            job_id,
            OpenHeat {
                job,
                heat,
                strikes: Vec::new(),
                opened_at: now_ms(),
            },
        );
        self.dispatch(vec![out_msg(&requester, &started)]);
    }

    /// The tier the heat's difficulty rides. The gear row is in Postgres, so the world
    /// cannot know it mid-tick; the run's own depth band is the honest stand-in, and it
    /// says the same thing — deeper work is harder work.
    fn open_heat_tier(&self, job: &SmithJob) -> Option<i32> {
        // A brew's difficulty is the recipe's own level — the alembic's answer to a
        // piece's tier.
        if job.service == "brew" {
            return meld_proto::consumables::recipe(&job.recipe).map(|r| r.min_level);
        }
        let w = self.world.as_ref()?;
        let run = w.run.runs.iter().find(|r| r.player_id == job.requester)?;
        Some(
            meld_world::Scaling::new(&self.balance).tier(run.max_distance_reached.max(0) as i64)
                as i32,
        )
    }

    /// A blow. The client reports where the marker was; the server owns the bar, so a
    /// strike is only ever a claim about timing, never about whether it counted.
    fn handle_strike(&mut self, player_id: &str, raw: RawEnvelope) -> Vec<Outgoing> {
        let req: wr::Strike = match serde_json::from_value(raw.payload) {
            Ok(v) => v,
            Err(_) => {
                return vec![error(player_id, ErrorCode::ValidationError, "bad strike", Some(raw.seq))]
            }
        };
        let Some(open) = self.open_heats.get_mut(&req.job_id) else {
            return vec![error(player_id, ErrorCode::InvalidState, "No heat open.", Some(raw.seq))];
        };
        // Someone else's heat is not yours to strike.
        if open.job.requester != player_id {
            return vec![error(player_id, ErrorCode::InvalidState, "Not your heat.", Some(raw.seq))];
        }
        if open.strikes.len() < open.heat.strikes.max(0) as usize {
            open.strikes.push(req.at.clamp(0.0, 1.0));
        }
        if open.strikes.len() >= open.heat.strikes.max(0) as usize {
            self.grade_heat(&req.job_id);
        }
        Vec::new()
    }

    /// Grade a heat and queue its Postgres half.
    fn grade_heat(&mut self, job_id: &str) {
        let Some(open) = self.open_heats.remove(job_id) else { return };
        let mut job = open.job;
        job.quality = meld_world::tempo::grade(&open.heat, &open.strikes);
        self.pending_smith.push(job);
    }

    /// Grade any heat whose window has run out. A smith who walked away from the anvil
    /// gets what they actually struck — the job still happens, just badly.
    fn expire_heats(&mut self) {
        if self.open_heats.is_empty() {
            return;
        }
        let now = now_ms();
        let stale: Vec<String> = self
            .open_heats
            .iter()
            .filter(|(_, h)| now.saturating_sub(h.opened_at) as i64 > h.heat.window_ms(&self.balance))
            .map(|(id, _)| id.clone())
            .collect();
        for id in stale {
            self.grade_heat(&id);
        }
    }

    /// Mirror each fresh diver's persistent Meld skill levels into the world, so the
    /// field-station gates never have to ask Postgres mid-tick.
    async fn flush_skill_loads(&mut self) {
        let loads: Vec<String> = std::mem::take(&mut self.pending_skill_load);
        for pid in loads {
            let Ok(uid) = Uuid::parse_str(&pid) else { continue };
            let Ok(skills) = self.db.get_skills(uid).await else { continue };
            let per = self.balance.meld.xp_per_level;
            let levels: HashMap<String, i32> = skills
                .into_iter()
                .map(|(kind, xp)| (kind, meld_balance::meld_skill_level(xp, per)))
                .collect();
            if let Some(s) = self.sessions.get_mut(&pid) {
                s.forging_level = levels.get("forging").copied();
            }
            if let Some(w) = self.world.as_mut() {
                w.skill_levels.insert(pid, levels);
            }
        }
    }

    /// Read each pending player's standing bounty contracts and hand them to the world
    /// (AD-4). Only `active` rows travel: a felled or withdrawn contract has no mark.
    async fn flush_bounty_loads(&mut self) {
        let loads: Vec<String> = std::mem::take(&mut self.pending_bounty_load);
        for pid in loads {
            let Ok(uid) = Uuid::parse_str(&pid) else { continue };
            let Ok(rows) = self.db.list_bounties(uid).await else { continue };
            let specs: Vec<(String, meld_proto::bounties::BountySpec)> = rows
                .into_iter()
                .filter(|r| r.state == "active")
                .filter_map(|r| {
                    serde_json::from_str(&r.spec)
                        .ok()
                        .map(|spec| (r.bounty_id.to_string(), spec))
                })
                .collect();
            if let Some(w) = self.world.as_mut() {
                w.bounties.insert(pid, specs);
            }
        }
    }

    /// Do the Postgres half of the smith jobs the world queued this tick. The world
    /// already decided WHO may ask and WHERE they were standing; what is left is the
    /// same atomic Vault call the HTTP anvil makes — which is what keeps "ownership
    /// never moves" structural rather than a rule to remember: every call is scoped to
    /// the REQUESTER's own player id, so a station cannot touch anyone else's gear.
    async fn flush_smith_jobs(&mut self) {
        let jobs: Vec<SmithJob> = std::mem::take(&mut self.pending_smith);
        for job in jobs {
            let forge = self.balance.forge.clone();
            let Ok(requester) = Uuid::parse_str(&job.requester) else { continue };
            let gid = match Uuid::parse_str(&job.gear_id) {
                Ok(g) => g,
                // A brew names a recipe rather than a piece, so a missing gear id is only
                // an error for the smith's services.
                Err(_) if job.service == "brew" || job.service == "tonic" => Uuid::nil(),
                Err(_) => {
                    self.dispatch(vec![error(
                        &job.requester,
                        ErrorCode::ValidationError,
                        "Unknown gear.",
                        Some(job.client_seq),
                    )]);
                    continue;
                }
            };
            // A brew has no piece in it — a pot is not a gear row — so the cook is
            // resolved before anything reaches for the Vault's gear table.
            if job.service == "brew" {
                let outcome = self.cook(&job, requester).await;
                self.finish_job(job, outcome).await;
                continue;
            }
            // A tonic is poured for the whole party, so it names no piece either.
            if job.service == "tonic" {
                let outcome = self.pour_tonic(&job, requester).await;
                self.finish_job(job, outcome).await;
                continue;
            }
            let row = match self.db.get_gear_by_id(requester, gid).await {
                Ok(Some(r)) => r,
                // Not owned by the requester is the same answer as not existing: a
                // station is not a way to reach into someone else's Vault.
                _ => {
                    self.dispatch(vec![error(
                        &job.requester,
                        ErrorCode::NotFound,
                        "That is not yours to work on.",
                        Some(job.client_seq),
                    )]);
                    continue;
                }
            };
            let ins = meld_proto::enums::Insurance::from_wire(&row.insurance);
            let outcome: Result<String, String> = match job.service.as_str() {
                // A temporary EDGE. It is never a Vault write — the bonus lives in the
                // run and dies with the dive — so it cannot be a way to launder power
                // home, which is what makes it worth asking for on the way IN.
                "enhance" if job.smith_level < forge.enhance_min_forging_level => Err(format!(
                    "This smith is Forging {} - an edge wants {}.",
                    job.smith_level, forge.enhance_min_forging_level
                )),
                // An edge goes on what a hero is WEARING: there is nothing to sharpen
                // about a piece sitting in the Vault at home.
                "enhance" if row.equipped_hero_slot.is_none() => {
                    Err("Only a piece a hero is wearing can take an edge.".to_string())
                }
                "enhance" => {
                    let slot = row.equipped_hero_slot.unwrap_or(0);
                    let in_run = self
                        .world
                        .as_ref()
                        .is_some_and(|w| w.run.runs.iter().any(|r| r.player_id == job.requester));
                    if !in_run {
                        Err("An edge only lasts a dive - ask on the way in.".to_string())
                    } else {
                        let material = if job.material.is_empty() {
                            "dune_ingot".to_string()
                        } else {
                            job.material.clone()
                        };
                        let materials = [(material.clone(), forge.enhance_material_cost)];
                        match self
                            .db
                            .spend_for_service(requester, &materials, forge.enhance_chit_cost)
                            .await
                        {
                            Ok(true) => {
                                let amount = forge.enhance_bonus_base
                                    + (forge.enhance_bonus_per_quality as f64 * job.quality).floor()
                                        as i32;
                                let edge = match row.slot.as_str() {
                                    "main_hand" => Edge { atk: amount, ..Default::default() },
                                    "accessory" => Edge { spd: amount, ..Default::default() },
                                    _ => Edge { def: amount, ..Default::default() },
                                };
                                if let Some(w) = self.world.as_mut() {
                                    let v = w.edges.entry(job.requester.clone()).or_default();
                                    while v.len() <= slot as usize {
                                        v.push(Edge::default());
                                    }
                                    let cur = &mut v[slot as usize];
                                    cur.atk += edge.atk;
                                    cur.def += edge.def;
                                    cur.spd += edge.spd;
                                }
                                Ok(format!(
                                    "put a +{amount} edge on {} for the rest of the dive ({:.0}% heat)",
                                    row.name,
                                    job.quality * 100.0
                                ))
                            }
                            Ok(false) => Err(format!(
                                "An edge needs {} {material} and {} chits.",
                                forge.enhance_material_cost, forge.enhance_chit_cost
                            )),
                            Err(_) => Err("The forge went cold.".to_string()),
                        }
                    }
                }
                "reroll" if job.smith_level < forge.reroll_min_forging_level => Err(format!(
                    "This smith is Forging {} - a reroll wants {}.",
                    job.smith_level, forge.reroll_min_forging_level
                )),
                "reroll" if ins == Some(meld_proto::enums::Insurance::Ephemeral) => Err(
                    "Ephemeral gear burns when you reach the city - a reroll would burn with it."
                        .to_string(),
                ),
                "reroll" => {
                    let class_key = if row.class_key.is_empty() {
                        "explorer".to_string()
                    } else {
                        row.class_key.clone()
                    };
                    // The HEAT decides the pool: a flawless run reaches the epic
                    // affixes, the same reach a trophy catalyst buys — paid in skill
                    // instead of monster parts.
                    let rarity = self.balance.tempo.rarity_for(job.quality);
                    let rolled = meld_world::reroll_affixes_at(
                        &self.balance,
                        row.tier,
                        &class_key,
                        &row.slot,
                        "forest",
                        rarity,
                        now_ms() ^ hash_str(&job.gear_id),
                    );
                    let need = forge.reroll_materials(row.tier);
                    let materials = [(job.material.clone(), need)];
                    match self
                        .db
                        .reroll_gear_affixes(
                            requester,
                            gid,
                            &materials,
                            forge.reroll_chit_cost,
                            &meld_proto::affixes::to_json(&rolled),
                        )
                        .await
                    {
                        Ok(true) => Ok(format!(
                            "re-drew {} ({rarity}, {:.0}% heat) for {} {} and {}c",
                            row.name,
                            job.quality * 100.0,
                            need,
                            job.material,
                            forge.reroll_chit_cost
                        )),
                        Ok(false) => Err(format!(
                            "A reroll on a tier {} piece needs {} {} and {} chits.",
                            row.tier, need, job.material, forge.reroll_chit_cost
                        )),
                        Err(_) => Err("The forge went cold.".to_string()),
                    }
                }
                _ if ins != Some(meld_proto::enums::Insurance::Insured) => Err(
                    "Only insured gear wears down - there is nothing here to repair."
                        .to_string(),
                ),
                _ => {
                    // A missed heat still mends something (its floor) — a bad job, not a
                    // robbery — and a clean one gives the smith's full reach back.
                    let full = forge.repair_points(job.smith_level) as f64;
                    let points =
                        ((full * self.balance.tempo.repair_fraction(job.quality)).floor() as i32)
                            .max(1);
                    match self
                        .db
                        .repair_gear(requester, gid, points, forge.repair_chit_cost_per_point)
                        .await
                    {
                        Ok(0) => Err("Nothing to repair, or not enough chits.".to_string()),
                        Ok(restored) => Ok(format!(
                            "mended {} +{restored} ({:.0}% heat) for {}c",
                            row.name,
                            job.quality * 100.0,
                            restored as i64 * forge.repair_chit_cost_per_point
                        )),
                        Err(_) => Err("The forge went cold.".to_string()),
                    }
                }
            };
            self.finish_job(job, outcome).await;
        }
    }

    /// Bill the station and tell the requester. A station only pays for work that
    /// happened, and the XP goes to the OWNER whose bench it is — a field station is a
    /// service its owner provides, which is the whole reason to raise one for a party.
    async fn finish_job(&mut self, job: SmithJob, outcome: Result<String, String>) {
        let uses_left = match &outcome {
            Ok(_) => {
                // The city anvil has no station to wear out (`station_id` empty).
                let left = self
                    .world
                    .as_mut()
                    .filter(|_| !job.station_id.is_empty())
                    .and_then(|w| w.arena.spend_station_use(&job.station_id))
                    .unwrap_or(0);
                let skill = if job.kind == "alembic" { "alchemy" } else { "forging" };
                let _ = self.db_writes.send(DbWrite::SkillXp(
                    job.owner.clone(),
                    skill.to_string(),
                    self.balance.forge.forge_xp_per_craft,
                ));
                left
            }
            Err(_) => self
                .world
                .as_ref()
                .and_then(|w| {
                    w.arena
                        .stations
                        .iter()
                        .find(|s| s.entity_id == job.station_id)
                        .map(|s| s.uses_left)
                })
                .unwrap_or(0),
        };
        let (ok, message) = match outcome {
            Ok(m) => (true, m),
            Err(m) => (false, m),
        };
        let result = wr::SmithResult {
            player_id: job.requester.clone(),
            entity_id: job.station_id.clone(),
            gear_id: job.gear_id.clone(),
            service: job.service.clone(),
            ok,
            message,
            uses_left,
            quality: job.quality,
        };
        self.dispatch(vec![out_msg(&job.requester, &result)]);
    }

    /// A tonic at a Keeper's alembic: the still's answer to the forge's edge, poured
    /// across the requester's WHOLE party instead of onto one piece. Like the edge it is
    /// never a Vault write — it lasts the dive and dies with it.
    async fn pour_tonic(&mut self, job: &SmithJob, requester: Uuid) -> Result<String, String> {
        let f = self.balance.forge.clone();
        let in_run = self
            .world
            .as_ref()
            .is_some_and(|w| w.run.runs.iter().any(|r| r.player_id == job.requester));
        if !in_run {
            return Err("A tonic only lasts a dive - ask on the way in.".to_string());
        }
        let material = if job.material.is_empty() {
            "bloom_herb".to_string()
        } else {
            job.material.clone()
        };
        let materials = [(material.clone(), f.tonic_material_cost)];
        match self.db.spend_for_service(requester, &materials, f.tonic_chit_cost).await {
            Ok(true) => {
                let (atk, def, regen) = (
                    f.tonic_amount(f.tonic_atk, job.quality),
                    f.tonic_amount(f.tonic_def, job.quality),
                    f.tonic_amount(f.tonic_regen, job.quality),
                );
                let size = self.balance.battle.party_size_per_player;
                if let Some(w) = self.world.as_mut() {
                    let v = w.edges.entry(job.requester.clone()).or_default();
                    while v.len() < size {
                        v.push(Edge::default());
                    }
                    for e in v.iter_mut() {
                        e.atk += atk;
                        e.def += def;
                        e.regen += regen;
                    }
                }
                Ok(format!(
                    "poured a tonic for the party: +{atk} atk, +{def} def, +{regen} regen for the rest of the dive ({:.0}% cook)",
                    job.quality * 100.0
                ))
            }
            Ok(false) => Err(format!(
                "A tonic needs {} {material} and {} chits.",
                f.tonic_material_cost, f.tonic_chit_cost
            )),
            Err(_) => Err("The pot cracked.".to_string()),
        }
    }

    /// A brew at a Keeper's alembic: the same cook the Apothecary's recipes run over
    /// HTTP, except the COOK is graded — a good one feeds more people from the same
    /// reagents (`[tempo] cook_bonus_doses`). The reagents and the doses are the
    /// requester's; the Keeper's level is what gates the pot and takes the XP.
    async fn cook(&mut self, job: &SmithJob, requester: Uuid) -> Result<String, String> {
        let Some(r) = meld_proto::consumables::recipe(&job.recipe) else {
            return Err("No such recipe.".to_string());
        };
        if r.skill != "alchemy" {
            return Err(format!("{} is smith's work, not a brew.", r.name));
        }
        if job.smith_level < r.min_level {
            return Err(format!(
                "This Keeper is {} {} - {} wants {}.",
                r.skill, job.smith_level, r.name, r.min_level
            ));
        }
        let bonus = self.balance.tempo.bonus_doses(job.quality);
        let qty = r.output_qty + bonus;
        let inputs: Vec<(String, i32)> = r
            .inputs
            .iter()
            .map(|(k, q)| ((*k).to_string(), *q))
            .collect();
        // XP goes to the Keeper, not the pot's owner, so it is credited by `finish_job`
        // rather than here.
        match self.db.craft(requester, &inputs, (r.output, qty), r.skill, 0).await {
            Ok(true) => Ok(format!(
                "brewed {qty}x {} ({:.0}% cook{})",
                r.name,
                job.quality * 100.0,
                if bonus > 0 { format!(", +{bonus} dose") } else { String::new() }
            )),
            Ok(false) => Err(format!(
                "{} wants {}.",
                r.name,
                inputs
                    .iter()
                    .map(|(k, q)| format!("{q} {k}"))
                    .collect::<Vec<_>>()
                    .join(" + ")
            )),
            Err(_) => Err("The pot cracked.".to_string()),
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
                // Onboarding: has this account already dismissed the town welcome
                // tour / first-dive briefing? Loaded here (never on the immediate
                // `Connected` message, which fires before this async load could
                // possibly have landed) and sent back below alongside the unlocks
                // sync, so the client never has to guess whether the load is done.
                let mut onboarding_status = None;
                if let (Ok(town), Ok(run)) = (
                    self.db.get_tutorial_town_seen(uid).await,
                    self.db.get_tutorial_run_seen(uid).await,
                ) {
                    if let Some(s) = self.sessions.get_mut(&pid) {
                        s.tutorial_town_seen = town;
                        s.tutorial_run_seen = run;
                    }
                    onboarding_status = Some(wo::Status { town_seen: town, run_seen: run });
                }
                if let Some(status) = onboarding_status {
                    self.dispatch(vec![out_msg(&pid, &status)]);
                }
                // PG-2: which departure hubs this account has earned by standing on them.
                if let Ok(deepest) = self.db.deepest_distance_ever(uid).await {
                    if let Some(s) = self.sessions.get_mut(&pid) {
                        s.deepest_ever = deepest;
                    }
                }
                // CL-1: what this account owns. Sent straight back with
                // `banner: false` so the party builder can grey the rows it
                // cannot field without four banners firing at login.
                if let Ok(owned) = self.db.get_unlocks(uid).await {
                    // `deepest_ever` was loaded just above, so the inventory carries the
                    // hub bar in the same message the party builder already reads.
                    let deepest =
                        self.sessions.get(&pid).map(|s| s.deepest_ever).unwrap_or(0);
                    let inventory = unlock_inventory(&owned, &[], false, deepest);
                    if let Some(s) = self.sessions.get_mut(&pid) {
                        s.unlocks = Some(owned);
                    }
                    self.dispatch(vec![out_msg(&pid, &inventory)]);
                }
                // AD-4: the board's state for this account, so a kill can be credited
                // against it on the tick it happens rather than at a DB round-trip.
                if let Ok(rows) = self.db.get_hunts(uid).await {
                    let board: HashMap<String, (i32, bool)> = rows
                        .into_iter()
                        .map(|r| (r.hunt_key, (r.progress, r.claimed)))
                        .collect();
                    let targets = quarry_targets(&board);
                    if let Some(s) = self.sessions.get_mut(&pid) {
                        s.hunts = Some(board);
                    }
                    if let Some(w) = self.world.as_mut() {
                        w.quarry.insert(pid.clone(), targets);
                    }
                }
            }
        }
    }

}

impl WorldActor {
    /// Harvest the named resource node the avatar is standing next to: bank its
    /// material into the backpack and queue its Meld-skill XP. The node vanishes
    /// from the next snapshot (server-authoritative — client just renders).
    /// Begin working a resource node (MS-2). This opens a **channel** rather than
    /// completing a gather: `advance_harvests` hands over one unit per tick while the
    /// player stays put and the node holds out.
    /// The temporary edge a smith put on this hero's kit this run, if any.
    fn edge_for(&self, player_id: &str, slot: usize) -> Option<&Edge> {
        self.edges.get(player_id).and_then(|v| v.get(slot))
    }

    /// Raise a field station where the player stands (MS-1). The ore comes out of the
    /// RUN backpack, so a field smith has to have gathered for it, and the Forging gate
    /// is checked against the builder's persistent Meld level — the profession is the
    /// skill, not the class (see `proposals/crafting-and-professions.md`).
    fn handle_build_station(
        &mut self,
        player_id: &str,
        raw: RawEnvelope,
    ) -> (Vec<Outgoing>, Vec<WorldEffect>) {
        let seq = raw.seq;
        let reject = |code: ErrorCode, msg: &str| {
            (vec![error(player_id, code, msg, Some(seq))], Vec::new())
        };
        let req: wr::BuildStation = match serde_json::from_value(raw.payload) {
            Ok(v) => v,
            Err(_) => return reject(ErrorCode::ValidationError, "bad build_station"),
        };
        // Two benches, one idea: a smith's forge is built from ORE and gated on Forging,
        // a Keeper's alembic from REAGENTS and gated on Alchemy.
        let forge = self.balance.forge.clone();
        let (skill, class, what) = match req.kind.as_str() {
            "smith" => (
                "forging",
                meld_proto::materials::MaterialClass::Ore,
                ("A field forge", forge.station_min_forging_level, "ore"),
            ),
            "alembic" => (
                "alchemy",
                meld_proto::materials::MaterialClass::Reagent,
                ("A field still", forge.station_min_alchemy_level, "reagent"),
            ),
            _ => return reject(ErrorCode::ValidationError, "No such station."),
        };
        let (label, min_level, stock_word) = what;
        if self.battle_of_player(player_id).is_some() {
            return reject(ErrorCode::InvalidState, "Resolve the battle first.");
        }
        // And somebody in the party has to be able to BUILD it. A forge is a
        // Smithwright's bench and a still is a Keeper's; the skill gate below is about how
        // good the work is, not about who may set one up. Without this the menu offered
        // "Set up a smith station" to a party with no smith in it.
        let builder = match req.kind.as_str() {
            "smith" => CharacterClass::Smithwright,
            _ => CharacterClass::Keeper,
        };
        let has_builder = self
            .party_classes
            .get(player_id)
            .is_some_and(|comp| comp.contains(&builder));
        if !has_builder {
            return reject(
                ErrorCode::InvalidState,
                &format!("{label} needs a {} in the party.", class_label(builder)),
            );
        }
        let level = self.skill_levels.get(player_id).and_then(|m| m.get(skill)).copied();
        // No level loaded yet is not a pass: a station is a service, and the whole
        // point is that the smith's own skill is what the work is done at.
        if level.unwrap_or(0) < min_level {
            return reject(
                ErrorCode::InvalidState,
                &format!(
                    "{label} takes {} level {min_level}.",
                    meld_proto::affixes::pretty_class(skill)
                ),
            );
        }
        // Built from ore you are carrying — the deepest stack first, so a smith who
        // hauled good ore out does not have it spent last.
        // A Smithwright in the party raises benches cheaper and quicker (its overworld
        // perk); the discount can never take the cost below one unit of stock.
        let perks = self.perks_for(player_id);
        let need = (forge.station_ore_cost - perks.smithwright_stock_discount).max(1);
        // One duration, used for BOTH the completion time and the bar the client draws —
        // announcing the unperked length would put the progress bar out of step with the
        // bench actually arriving.
        let setup_ms = (((forge.station_setup_ms as f64) * perks.smithwright_setup_mult).round()
            as u64)
            .max(1);
        let Some(run) = self.run.runs.iter_mut().find(|r| r.player_id == player_id) else {
            return reject(ErrorCode::InvalidState, "Not in a run.");
        };
        // Summed ACROSS stacks, deepest tier first: a harvest banks one unit per tick as
        // its own stack, so a smith who gathered ore in the field never has one stack
        // holding the whole cost — this refused every bench built from freshly-dug ore and
        // said "takes 3 ore" to somebody carrying five.
        let mut have: HashMap<String, (i32, i32)> = HashMap::new();
        for item in run
            .backpack
            .iter()
            .filter(|i| i.quantity > 0 && meld_proto::materials::is_class(&i.item_kind, class))
        {
            let tier =
                meld_proto::materials::material(&item.item_kind).map(|m| m.tier).unwrap_or(0);
            have.entry(item.item_kind.clone()).or_insert((0, tier)).0 += item.quantity;
        }
        let Some(picked) = have
            .into_iter()
            .filter(|(_, (n, _))| *n >= need)
            .max_by_key(|(_, (_, tier))| *tier)
            .map(|(k, _)| k)
        else {
            return reject(
                ErrorCode::InvalidState,
                &format!("{label} takes {need} of one {stock_word} you are carrying."),
            );
        };
        let ore_kind = picked;
        let radius = forge.station_radius;
        // Refuse before charging anyone: a spot that is already taken, or a channel
        // already running, must not cost stock.
        if self.station_here(player_id, radius) {
            return reject(ErrorCode::InvalidState, "There is already a bench here.");
        }
        if self.extraction.contains_key(player_id)
            || self.harvest.contains_key(player_id)
            || self.building.contains_key(player_id)
        {
            return reject(ErrorCode::InvalidState, "Already channeling.");
        }
        let run = self
            .run
            .runs
            .iter_mut()
            .find(|r| r.player_id == player_id)
            .expect("checked above");
        spend_material(run, class, need);
        // The stock goes in NOW and the bench arrives when the channel completes — an
        // interrupted setup costs you the materials, which is what makes choosing the
        // moment (and the ground) a real decision.
        let now = now_ms();
        self.building.insert(
            player_id.to_string(),
            Building {
                completes_at: now + setup_ms,
                kind: req.kind.clone(),
                tearing_down: None,
                stock: ore_kind.clone(),
            },
        );
        if let Some(a) = self.arena.avatar_mut(player_id) {
            a.state = "channeling".to_string();
        }
        let update = wr::BackpackUpdate {
            changes: vec![wr::BackpackChange {
                item: ItemStack {
                    item_id: Uuid::now_v7().to_string(),
                    item_kind: ore_kind.clone(),
                    quantity: need,
                    insurance: None,
                },
                delta: "removed".to_string(),
                cause: "station".to_string(),
            }],
            chits_delta: 0,
            gear_added: Vec::new(),
        };
        let mut out = vec![out_msg(player_id, &update)];
        out.extend(self.announce_channel(
            player_id,
            &format!("build:{}", req.kind),
            now + setup_ms,
            setup_ms,
            Some(seq),
        ));
        (out, Vec::new())
    }

    /// Raise a `Structure` where the player stands (CANON D21/§W3, `BD-2`).
    ///
    /// One handler for every function, because there is one primitive: the `function` key
    /// picks the cost and the HP out of `[building]` and the rules out of the registry,
    /// and nothing else about the flow branches. That is the discipline D21 mandates — the
    /// moment this grows a `match` on function for anything but its numbers, it is broken.
    fn handle_build_structure(&mut self, player_id: &str, raw: RawEnvelope) -> Vec<Outgoing> {
        let seq = raw.seq;
        let reject = |code: ErrorCode, msg: &str| vec![error(player_id, code, msg, Some(seq))];
        let req: wr::BuildStructure = match serde_json::from_value(raw.payload) {
            Ok(v) => v,
            Err(_) => return reject(ErrorCode::ValidationError, "bad build_structure"),
        };
        let Some(def) = meld_proto::structures::structure(&req.function) else {
            return reject(ErrorCode::ValidationError, "No such structure.");
        };
        if self.battle_of_player(player_id).is_some() {
            return reject(ErrorCode::InvalidState, "Resolve the battle first.");
        }
        if self.location.contains_key(player_id) {
            return reject(ErrorCode::InvalidState, "Not down here.");
        }
        let balance = self.balance.clone();
        let Some((cost, _, _)) = balance.building.spec(&req.function) else {
            return reject(ErrorCode::ValidationError, "No such structure.");
        };
        // What ore we WOULD spend, deepest tier first and summed ACROSS stacks — a harvest
        // banks one unit per tick as its own stack, so ore you just dug up is never one
        // stack big enough to pay for anything. Chosen here but not spent until placement
        // has been validated.
        let Some(run) = self.run.runs.iter().find(|r| r.player_id == player_id) else {
            return reject(ErrorCode::InvalidState, "Not in a run.");
        };
        let mut have: HashMap<String, (i32, i32)> = HashMap::new();
        for item in run.backpack.iter().filter(|i| {
            i.quantity > 0
                && meld_proto::materials::is_class(
                    &i.item_kind,
                    meld_proto::materials::MaterialClass::Ore,
                )
        }) {
            let tier =
                meld_proto::materials::material(&item.item_kind).map(|m| m.tier).unwrap_or(0);
            have.entry(item.item_kind.clone()).or_insert((0, tier)).0 += item.quantity;
        }
        let Some(ore_kind) = have
            .into_iter()
            .filter(|(_, (n, _))| *n >= cost)
            .max_by_key(|(_, (_, tier))| *tier)
            .map(|(k, _)| k)
        else {
            return reject(ErrorCode::InvalidState, &format!("{} takes {cost} ore.", def.name));
        };
        // Validated BEFORE the stock is spent: a refusal that also charged you is the
        // worst kind, and the arena is the only thing that knows the ground.
        let tick = self.tick_count;
        if let Err(why) =
            self.arena.place_structure(&balance, player_id, &req.function, &ore_kind, tick)
        {
            return reject(ErrorCode::InvalidState, why.message());
        }
        let run = self
            .run
            .runs
            .iter_mut()
            .find(|r| r.player_id == player_id)
            .expect("checked above");
        spend_material(run, meld_proto::materials::MaterialClass::Ore, cost);
        vec![out_msg(
            player_id,
            &wr::BackpackUpdate {
                changes: vec![wr::BackpackChange {
                    item: ItemStack {
                        item_id: Uuid::now_v7().to_string(),
                        item_kind: ore_kind,
                        quantity: cost,
                        insurance: None,
                    },
                    delta: "removed".to_string(),
                    cause: "build".to_string(),
                }],
                chits_delta: 0,
                gear_added: Vec::new(),
            },
        )]
    }

    /// Spend one unit of ore mending a structure you are standing at.
    ///
    /// **Anyone may repair; only the owner may demolish.** Holding ground is a thing a
    /// party does together — a teammate hauling ore out to your anchor is the co-op verb
    /// this whole epic is for — while taking something down is a decision about somebody
    /// else's work.
    fn handle_repair_structure(&mut self, player_id: &str, raw: RawEnvelope) -> Vec<Outgoing> {
        let seq = raw.seq;
        let reject = |code: ErrorCode, msg: &str| vec![error(player_id, code, msg, Some(seq))];
        let req: wr::RepairStructure = match serde_json::from_value(raw.payload) {
            Ok(v) => v,
            Err(_) => return reject(ErrorCode::ValidationError, "bad repair_structure"),
        };
        if self.battle_of_player(player_id).is_some() {
            return reject(ErrorCode::InvalidState, "Resolve the battle first.");
        }
        let balance = self.balance.clone();
        let reach = balance.world.interaction_radius_tiles;
        let Some(target) = self.arena.structure_at(player_id, &req.entity_id, reach) else {
            return reject(ErrorCode::InvalidState, "Nothing in reach.");
        };
        if target.hp >= target.max_hp {
            let name = target.def().map(|d| d.name).unwrap_or("It");
            return reject(ErrorCode::InvalidState, &format!("The {name} is sound."));
        }
        let Some(run) = self.run.runs.iter_mut().find(|r| r.player_id == player_id) else {
            return reject(ErrorCode::InvalidState, "Not in a run.");
        };
        let Some(ore_kind) = spend_material(run, meld_proto::materials::MaterialClass::Ore, 1)
        else {
            return reject(ErrorCode::InvalidState, "No ore to mend it with.");
        };
        self.arena.repair_structure(&balance, &req.entity_id);
        vec![out_msg(
            player_id,
            &wr::BackpackUpdate {
                changes: vec![wr::BackpackChange {
                    item: ItemStack {
                        item_id: Uuid::now_v7().to_string(),
                        item_kind: ore_kind,
                        quantity: 1,
                        insurance: None,
                    },
                    delta: "removed".to_string(),
                    cause: "repair".to_string(),
                }],
                chits_delta: 0,
                gear_added: Vec::new(),
            },
        )]
    }

    /// Pack a structure you own back down, for part of its materials.
    fn handle_demolish_structure(&mut self, player_id: &str, raw: RawEnvelope) -> Vec<Outgoing> {
        let seq = raw.seq;
        let reject = |code: ErrorCode, msg: &str| vec![error(player_id, code, msg, Some(seq))];
        let req: wr::DemolishStructure = match serde_json::from_value(raw.payload) {
            Ok(v) => v,
            Err(_) => return reject(ErrorCode::ValidationError, "bad demolish_structure"),
        };
        if self.battle_of_player(player_id).is_some() {
            return reject(ErrorCode::InvalidState, "Resolve the battle first.");
        }
        let balance = self.balance.clone();
        let reach = balance.world.interaction_radius_tiles;
        let Some(target) = self.arena.structure_at(player_id, &req.entity_id, reach) else {
            return reject(ErrorCode::InvalidState, "Nothing in reach.");
        };
        if target.owner_player_id != player_id {
            return reject(ErrorCode::InvalidState, "That is not yours to take down.");
        }
        let Some((ore_kind, back)) = self.arena.demolish_structure(&balance, &req.entity_id) else {
            return reject(ErrorCode::InvalidState, "Nothing in reach.");
        };
        if back <= 0 {
            return Vec::new();
        }
        let item = ItemStack {
            item_id: Uuid::now_v7().to_string(),
            item_kind: ore_kind,
            quantity: back,
            insurance: None,
        };
        if let Some(run) = self.run.runs.iter_mut().find(|r| r.player_id == player_id) {
            run.backpack.push(item.clone());
        }
        vec![out_msg(
            player_id,
            &wr::BackpackUpdate {
                changes: vec![wr::BackpackChange {
                    item,
                    delta: "added".to_string(),
                    cause: "demolish".to_string(),
                }],
                chits_delta: 0,
                gear_added: Vec::new(),
            },
        )]
    }

    /// How many OTHER pairs of the right hands are in the party. A Smithwright helps at a
    /// forge and a Keeper at a still; anyone else is standing around holding a lamp. The
    /// requester's own party is what counts, since that is who is actually there.
    fn crew_for(&self, kind: &str) -> i32 {
        let want = if kind == "alembic" {
            CharacterClass::Keeper
        } else {
            CharacterClass::Smithwright
        };
        let hands = self
            .party_classes
            .values()
            .flatten()
            .filter(|c| **c == want)
            .count() as i32;
        // The first pair of hands IS the crafter — their own level is already the bar's
        // other input — so only the rest are a crew. Four Smithwrights is three extra
        // pairs of hands, which is exactly `extra_hands_max`.
        (hands - 1).max(0)
    }

    /// Is there a live bench within reach of where this player stands?
    fn station_here(&self, player_id: &str, radius: f64) -> bool {
        let Some(a) = self.arena.avatar(player_id) else { return false };
        self.arena.stations.iter().any(|s| {
            !s.spent() && s.elevation == a.elevation && a.position.distance_to(&s.position) <= radius
        })
    }

    /// Tell the instance a channel opened. Every channel in the game says the same thing
    /// the same way, so the client's one progress bar covers all of them.
    fn announce_channel(
        &self,
        player_id: &str,
        method: &str,
        completes_at: u64,
        fill_ms: u64,
        client_seq: Option<u32>,
    ) -> Vec<Outgoing> {
        self.run
            .runs
            .iter()
            .map(|r| r.player_id.clone())
            .collect::<Vec<_>>()
            .iter()
            .map(|pid| {
                out_msg(
                    pid,
                    &wr::ChannelStarted {
                        client_seq: if pid == player_id { client_seq } else { None },
                        player_id: player_id.to_string(),
                        method: method.to_string(),
                        completes_at,
                        fill_ms,
                    },
                )
            })
            .collect()
    }

    /// Pack up a bench you are standing at: its own channel, and it hands part of the
    /// stock back. Anyone may work at a station, but only its OWNER may take it down.
    fn handle_teardown_station(
        &mut self,
        player_id: &str,
        raw: RawEnvelope,
    ) -> (Vec<Outgoing>, Vec<WorldEffect>) {
        let seq = raw.seq;
        let reject = |code: ErrorCode, msg: &str| {
            (vec![error(player_id, code, msg, Some(seq))], Vec::new())
        };
        let req: wr::TeardownStation = match serde_json::from_value(raw.payload) {
            Ok(v) => v,
            Err(_) => return reject(ErrorCode::ValidationError, "bad teardown_station"),
        };
        if self.battle_of_player(player_id).is_some() {
            return reject(ErrorCode::InvalidState, "Resolve the battle first.");
        }
        if self.extraction.contains_key(player_id)
            || self.harvest.contains_key(player_id)
            || self.building.contains_key(player_id)
        {
            return reject(ErrorCode::InvalidState, "Already channeling.");
        }
        let radius = self.balance.forge.station_radius;
        let Some(station) = self.arena.station_at(player_id, &req.entity_id, radius) else {
            return reject(ErrorCode::OutOfRange, "No bench in reach.");
        };
        if station.owner_player_id != player_id {
            return reject(ErrorCode::InvalidState, "That bench is not yours to move.");
        }
        let kind = station.kind.clone();
        let now = now_ms();
        let ms = self.balance.forge.station_teardown_ms;
        self.building.insert(
            player_id.to_string(),
            Building {
                completes_at: now + ms,
                kind: kind.clone(),
                tearing_down: Some(req.entity_id.clone()),
                stock: String::new(),
            },
        );
        if let Some(a) = self.arena.avatar_mut(player_id) {
            a.state = "channeling".to_string();
        }
        (
            self.announce_channel(player_id, &format!("pack:{kind}"), now + ms, ms, Some(seq)),
            Vec::new(),
        )
    }

    /// Finish any setup/teardown channel whose time is up: the bench appears, or comes
    /// apart and hands back what `station_teardown_refund` says.
    fn advance_building(&mut self) -> Vec<Outgoing> {
        let now = now_ms();
        let due: Vec<String> = self
            .building
            .iter()
            .filter(|(_, b)| b.completes_at <= now)
            .map(|(pid, _)| pid.clone())
            .collect();
        let mut out = Vec::new();
        for pid in due {
            let Some(b) = self.building.remove(&pid) else { continue };
            if let Some(a) = self.arena.avatar_mut(&pid) {
                if a.state == "channeling" {
                    a.state = "active".to_string();
                }
            }
            let forge = self.balance.forge.clone();
            let perks = self.perks_for(&pid);
            match &b.tearing_down {
                None => {
                    if self
                        .arena
                        .place_station(
                            &pid,
                            &b.kind,
                            forge.station_uses + perks.smithwright_bench_uses,
                            forge.station_radius,
                            &b.stock,
                        )
                        .is_none()
                    {
                        // Someone else raised one here while this was going up. The stock
                        // is already gone, so say so rather than swallowing it.
                        out.push(error(
                            &pid,
                            ErrorCode::InvalidState,
                            "Someone raised a bench here first.",
                            None,
                        ));
                    }
                }
                Some(id) => {
                    // Only a bench with work left in it is worth salvaging, and it hands
                    // back the same stock it was built from.
                    let salvage = self
                        .arena
                        .remove_station(id)
                        .filter(|(left, _)| *left > 0)
                        // A Smithwright deep enough packs a bench up WHOLE: it gets back
                        // everything the bench was built from, not the salvage everyone
                        // else settles for.
                        .map(|(_, stock)| {
                            let refund = if perks.smithwright_pack_full {
                                (forge.station_ore_cost - perks.smithwright_stock_discount).max(1)
                            } else {
                                forge.station_teardown_refund.max(0)
                            };
                            (refund, stock)
                        });
                    if let Some((refund, kind)) = salvage.filter(|(r, _)| *r > 0) {
                        let kind = kind.as_str();
                        if let Some(run) = self.run.runs.iter_mut().find(|r| r.player_id == pid) {
                            match run.backpack.iter_mut().find(|i| i.item_kind == kind) {
                                Some(slot) => slot.quantity += refund,
                                None => run.backpack.push(ItemStack {
                                    item_id: Uuid::now_v7().to_string(),
                                    item_kind: kind.to_string(),
                                    quantity: refund,
                                    insurance: None,
                                }),
                            }
                        }
                        out.push(out_msg(
                            &pid,
                            &wr::BackpackUpdate {
                                changes: vec![wr::BackpackChange {
                                    item: ItemStack {
                                        item_id: Uuid::now_v7().to_string(),
                                        item_kind: kind.to_string(),
                                        quantity: refund,
                                        insurance: None,
                                    },
                                    delta: "added".to_string(),
                                    cause: "station".to_string(),
                                }],
                                chits_delta: 0,
                                gear_added: Vec::new(),
                            },
                        ));
                    }
                }
            }
        }
        out
    }

    /// End a setup/teardown channel early. An interrupted SETUP keeps the stock spent —
    /// the materials went into the ground — which is what makes where and when you build
    /// a real choice rather than a free action.
    fn end_building(&mut self, player_id: &str, reason: &str) -> Vec<Outgoing> {
        if self.building.remove(player_id).is_none() {
            return Vec::new();
        }
        if let Some(a) = self.arena.avatar_mut(player_id) {
            if a.state == "channeling" {
                a.state = "active".to_string();
            }
        }
        self.run
            .runs
            .iter()
            .map(|r| r.player_id.clone())
            .collect::<Vec<_>>()
            .iter()
            .map(|pid| {
                out_msg(
                    pid,
                    &wr::ChannelInterrupted {
                        player_id: player_id.to_string(),
                        reason: reason.to_string(),
                    },
                )
            })
            .collect()
    }

    /// Ask the smith whose station this is to work a piece of the REQUESTER's gear.
    /// Everything that decides whether it can happen is here (who is standing where,
    /// whether the station has jobs left); the DB half runs off the tick in
    /// `flush_smith_jobs`, because the loop must not park on Postgres.
    fn handle_smith_request(
        &mut self,
        player_id: &str,
        raw: RawEnvelope,
    ) -> (Vec<Outgoing>, Vec<WorldEffect>) {
        let seq = raw.seq;
        let reject = |code: ErrorCode, msg: &str| {
            (vec![error(player_id, code, msg, Some(seq))], Vec::new())
        };
        let req: wr::SmithRequest = match serde_json::from_value(raw.payload) {
            Ok(v) => v,
            Err(_) => return reject(ErrorCode::ValidationError, "bad smith_request"),
        };
        if !matches!(
            req.service.as_str(),
            "reroll" | "repair" | "enhance" | "brew" | "tonic"
        ) {
            return reject(ErrorCode::ValidationError, "No such service.");
        }
        if self.battle_of_player(player_id).is_some() {
            return reject(ErrorCode::InvalidState, "Resolve the battle first.");
        }
        let radius = self.balance.forge.station_radius;
        let Some(station) = self.arena.station_at(player_id, &req.entity_id, radius) else {
            return reject(ErrorCode::OutOfRange, "No bench in reach.");
        };
        let kind = station.kind.clone();
        let owner = station.owner_player_id.clone();
        // A forge cannot cook and a still cannot mend: the bench you are standing at is
        // what decides what may be asked of it.
        let (skill, allowed): (&str, &[&str]) = match kind.as_str() {
            "alembic" => ("alchemy", &["brew", "tonic"]),
            _ => ("forging", &["reroll", "repair", "enhance"]),
        };
        if !allowed.contains(&req.service.as_str()) {
            return reject(
                ErrorCode::ValidationError,
                &format!("A {kind} does not do that."),
            );
        }
        // The station OWNER's skill is the skill the job is done at — that is the whole
        // point of asking someone else's smith. An unloaded level counts as none.
        let smith_level =
            self.skill_levels.get(&owner).and_then(|m| m.get(skill)).copied().unwrap_or(0);
        // A crew makes the bar easier to hit, and a crew is a party of the RIGHT PEOPLE:
        // every other Smithwright at a forge, every other Keeper at a still. This is what
        // the profession classes buy — hands on the work, not a bigger number.
        let crew = self.crew_for(&kind);
        (
            Vec::new(),
            vec![WorldEffect::SmithJob(Box::new(SmithJob {
                requester: player_id.to_string(),
                owner,
                kind,
                smith_level,
                crew,
                station_id: req.entity_id.clone(),
                gear_id: req.gear_id.clone(),
                service: req.service.clone(),
                material: req.material.clone(),
                recipe: req.recipe.clone(),
                client_seq: seq,
                quality: 0.0,
            }))],
        )
    }

    /// CL-2 — a Psyker pins a creature where it stands. Every gate is server-side: the
    /// party must field a Psyker deep enough, the cooldown must have run out, the creature
    /// must be in reach and on the same terrace, and the party must not already be holding
    /// its limit. A refusal is silent (an empty reply): the client greys the affordance
    /// out from the same `run.perks` numbers, so a refusal here means a stale client
    /// rather than something a player needs told.
    fn handle_psyker_hold(&mut self, player_id: &str, raw: RawEnvelope) -> Vec<Outgoing> {
        let req: wr::PsykerHold = match serde_json::from_value(raw.payload) {
            Ok(v) => v,
            Err(_) => {
                return vec![error(player_id, ErrorCode::ValidationError, "bad hold", Some(raw.seq))]
            }
        };
        if self.battle_of_player(player_id).is_some() {
            return Vec::new();
        }
        let perks = self.perks_for(player_id);
        if perks.psyker_hold_targets == 0 || perks.psyker_hold_seconds <= 0.0 {
            return Vec::new();
        }
        let now = now_ms();
        let cooldown_ms = (perks.psyker_hold_cooldown * 1000.0) as u64;
        if let Some(last) = self.hold_last_ms.get(player_id) {
            if now.saturating_sub(*last) < cooldown_ms {
                return Vec::new();
            }
        }
        // Already at the limit? Count what this player is still holding.
        let live = self.arena.monsters.iter().filter(|m| m.held_for > 0.0).count();
        if live >= perks.psyker_hold_targets as usize {
            return Vec::new();
        }
        let Some(a) = self.arena.avatar(player_id) else { return Vec::new() };
        let (apos, alevel) = (a.position, a.elevation);
        let reach = perks.psyker_hold_radius as f64;
        let seconds = perks.psyker_hold_seconds as f64;
        let Some(m) = self.arena.monsters.iter_mut().find(|m| {
            m.entity_id == req.entity_id
                && !m.defeated
                && !m.in_battle
                && m.elevation == alevel
                && apos.distance_to(&m.position) <= reach
        }) else {
            return Vec::new();
        };
        m.held_for = seconds;
        self.hold_last_ms.insert(player_id.to_string(), now);
        Vec::new()
    }

    fn handle_harvest(
        &mut self,
        player_id: &str,
        raw: RawEnvelope,
    ) -> (Vec<Outgoing>, Vec<WorldEffect>) {
        let req: wr::Harvest = match serde_json::from_value(raw.payload) {
            Ok(v) => v,
            Err(_) => {
                return (
                    vec![error(
                        player_id,
                        ErrorCode::ValidationError,
                        "bad harvest",
                        Some(raw.seq),
                    )],
                    Vec::new(),
                )
            }
        };
        if self.battle_of_player(player_id).is_some() {
            return (
                vec![error(
                    player_id,
                    ErrorCode::InvalidState,
                    "Resolve the battle first.",
                    Some(raw.seq),
                )],
                Vec::new(),
            );
        }
        // One channel at a time: a player mid-extraction is not also mining.
        if self.extraction.contains_key(player_id) || self.harvest.contains_key(player_id) {
            return (
                vec![error(
                    player_id,
                    ErrorCode::InvalidState,
                    "Already channeling.",
                    Some(raw.seq),
                )],
                Vec::new(),
            );
        }
        let Some(kind) = self.arena.can_harvest(player_id, &req.entity_id) else {
            return (
                vec![error(
                    player_id,
                    ErrorCode::OutOfRange,
                    "Nothing to harvest here.",
                    Some(raw.seq),
                )],
                Vec::new(),
            );
        };
        let Some(tick_ms) = self.harvest_tick_ms(&kind) else {
            return (
                vec![error(
                    player_id,
                    ErrorCode::ValidationError,
                    "unknown resource",
                    Some(raw.seq),
                )],
                Vec::new(),
            );
        };
        let now = now_ms();
        // `completes_at` is when the node would run dry if nobody interrupted — the
        // horizon the client draws its bar against, not a promise.
        let remaining = self
            .arena
            .resources
            .iter()
            .find(|n| n.entity_id == req.entity_id)
            .map(|n| n.remaining.max(0) as u64)
            .unwrap_or(1);
        self.harvest.insert(
            player_id.to_string(),
            Harvest {
                node_id: req.entity_id.clone(),
                next_at: now + tick_ms,
                tick_ms,
            },
        );
        if let Some(a) = self.arena.avatar_mut(player_id) {
            a.state = "channeling".to_string();
        }
        let members: Vec<String> = self.run.runs.iter().map(|r| r.player_id.clone()).collect();
        let msgs: Vec<Outgoing> = members
            .iter()
            .map(|pid| {
                out_msg(
                    pid,
                    &wr::ChannelStarted {
                        client_seq: if pid == player_id { Some(raw.seq) } else { None },
                        player_id: player_id.to_string(),
                        method: format!("harvest:{kind}"),
                        completes_at: now + tick_ms * remaining,
                        fill_ms: tick_ms,
                    },
                )
            })
            .collect();
        (msgs, Vec::new())
    }

    /// Put the tool down on purpose (the "click away" gesture). Already-banked units
    /// stay banked; there is nothing to lose but the tick in flight.
    /// [E] while channeling stops whatever is running, so the cancel path covers a
    /// half-raised bench as well as a gather.
    fn handle_cancel_harvest(
        &mut self,
        player_id: &str,
        _raw: RawEnvelope,
    ) -> (Vec<Outgoing>, Vec<WorldEffect>) {
        let mut out = self.end_harvest(player_id, "cancelled");
        out.extend(self.end_building(player_id, "cancelled"));
        (out, Vec::new())
    }

    /// Drink a potion in the field (out of combat) — the overworld half of the
    /// battle Item command. Before this, a wounded party had to FIND a fight before
    /// it could heal, which is exactly backwards: you died on the walk there.
    ///
    /// Server-authoritative in the way that matters for a client someone has edited:
    /// the request names only an item kind and a hero slot, and every number — the
    /// dose, the cap, whether the effect even applies — is computed here from the
    /// registry and balance. A hacked client can ask to drink what it doesn't have,
    /// or heal a hero that doesn't exist; it gets an error, not an effect.
    fn handle_use_item(
        &mut self,
        player_id: &str,
        raw: RawEnvelope,
    ) -> (Vec<Outgoing>, Vec<WorldEffect>) {
        use meld_proto::consumables::{self as con, ConsumableEffect as E};
        let seq = raw.seq;
        let reject = |msg: &str| {
            (
                vec![error(player_id, ErrorCode::ValidationError, msg, Some(seq))],
                Vec::new(),
            )
        };
        let req: wr::UseItem = match serde_json::from_value(raw.payload) {
            Ok(v) => v,
            Err(_) => return reject("bad use_item"),
        };

        // In a fight the battle Item command is the one that works: it runs on the
        // actor's turn, spends the gauge, and is visible to everyone in the fight.
        let in_battle = self.parties_in_battle();
        let fighting = self
            .run
            .runs
            .iter()
            .any(|r| r.player_id == player_id && in_battle.contains(&r.party_id));
        if fighting {
            return reject("Use it on your turn.");
        }

        let Some(def) = con::consumable(&req.item_kind) else {
            return reject("Not a potion.");
        };
        let dose = self.balance.consumable.potency_mult(def.potency);
        // Whether a potion works out here is a property of the POTION, so it is
        // answered before anything about this particular pack. "Save that one for a
        // fight" is the useful reply either way; "out of bulwark tonic" would send a
        // player looking for more of something that was never going to help.
        if matches!(def.effect, E::Barrier | E::Regen | E::Evasion | E::Adrenaline) {
            return reject("Save that one for a fight.");
        }

        let heroes = self.party_views(player_id);
        let slot = req.hero_slot;
        if slot < 0 || slot as usize >= heroes.len() {
            return reject("No such hero.");
        }
        let slot = slot as usize;
        let max_hp = heroes[slot].max_hp;
        let hp_now = heroes[slot].hp;

        // In the FIELD either container is in reach — you are standing still, so
        // rummaging in the Party Inventory is fine. Only a battle restricts a hero to
        // its own pouch, so a potion already moved onto someone is not stranded here.
        let (in_bag, in_pouch) = self
            .run
            .run_mut(player_id)
            .map_or((0, 0), |r| {
                let bag = r
                    .backpack
                    .iter()
                    .find(|i| i.item_kind == req.item_kind)
                    .map_or(0, |i| i.quantity);
                (bag, r.pouch_qty(slot, &req.item_kind))
            });
        if in_bag + in_pouch <= 0 {
            return reject(&format!("Out of {}.", req.item_kind.replace('_', " ")));
        }

        // Work out the new HP (and any XP) BEFORE spending anything, so a bottle that
        // would do nothing stays corked. A no-op that still consumed the item would be
        // the cruellest possible reading of "you can use items in the field".
        let mut new_hp: Option<i32> = None;
        let mut grant_xp: i64 = 0;
        let mut cured: Vec<String> = Vec::new();
        match def.effect {
            // You throw these AT something, and out here there is nothing to throw them at.
            // Refused rather than spent: this function's own rule is that a bottle which
            // would do nothing stays corked, and a francisca lobbed across an empty field
            // is a francisca gone.
            E::ThrownAll => return reject("Save that for a fight."),
            E::Heal | E::FullHeal => {
                if hp_now <= 0 {
                    return reject("They're down — that needs a revive.");
                }
                if hp_now >= max_hp {
                    return reject("Already at full health.");
                }
                let raw_heal = match def.effect {
                    E::FullHeal => max_hp,
                    _ => ((max_hp as f64) * self.balance.battle.item_heal_fraction * dose).round()
                        as i32,
                };
                new_hp = Some((hp_now + raw_heal).min(max_hp));
            }
            // A cure works out here now that afflictions are run-scoped — this is where most
            // of them are actually felt (venom bites per step, bindings drag a march). Still
            // refused when it would lift NOTHING, which is this function's own rule: a bottle
            // that does nothing stays corked.
            E::Cleanse | E::Panacea => {
                let family = if def.effect == E::Panacea {
                    meld_proto::statuses::Family::All
                } else {
                    meld_proto::statuses::Family::Mind
                };
                let lifted = cure_carried(
                    self.hero_afflictions.get_mut(player_id),
                    slot as usize,
                    family,
                );
                if lifted.is_empty() {
                    return reject("Nothing that would answer to it has hold of them.");
                }
                cured = lifted;
            }
            E::Revive => {
                if hp_now > 0 {
                    return reject("They're still standing.");
                }
                let fraction = (self.balance.consumable.revive_hp_fraction * dose).min(1.0);
                new_hp = Some((((max_hp as f64) * fraction).round() as i32).clamp(1, max_hp));
            }
            E::Experience => {
                grant_xp = self.balance.consumable.insight_mote_xp;
                if grant_xp <= 0 {
                    return reject("Nothing to learn.");
                }
            }
            E::Barrier | E::Regen | E::Evasion | E::Adrenaline => {
                return reject("Save that one for a fight.")
            }
        }

        let mut out = Vec::new();
        // A cure changes what the roster says about a hero, and the roster is how the client
        // learns it — including out here, where a distracted hero's controls are reversed and a
        // blinded one cannot see. Re-sent rather than patched so there is one source of truth.
        if !cured.is_empty() {
            let (synergies, combos) = self.party_depth(player_id);
            out.push(out_msg(
                player_id,
                &wr::Party {
                    heroes: self.party_views(player_id),
                    synergies,
                    combos,
                    abilities: self.party_ability_views(player_id),
                },
            ));
        }
        if let Some(hp) = new_hp {
            let hps = self.hero_hp.entry(player_id.to_string()).or_default();
            if hps.len() < heroes.len() {
                hps.resize(heroes.len(), 0);
                for (i, h) in heroes.iter().enumerate() {
                    if hps[i] == 0 && h.hp > 0 {
                        hps[i] = h.hp;
                    }
                }
            }
            hps[slot] = hp;
        }
        if grant_xp > 0 {
            let balance = self.balance.clone();
            let size = heroes.len().max(1);
            let mut leveled: Option<(i32, i32)> = None;
            if let Some(r) = self.run.runs.iter_mut().find(|r| r.player_id == player_id) {
                let old = r.run_level;
                // A mote is drunk by ONE hero and pays out whole — it is not an
                // encounter pool, so it is a single share rather than a pre-multiply
                // that cancels a division.
                if r.award_hero_xp(slot, 1, size, grant_xp, &balance) > 0 {
                    leveled = Some((old, r.run_level));
                }
            }
            // Advancing lifts what is gripping that hero (never their death).
            if leveled.is_some() {
                self.cure_on_level_up(player_id, slot);
            }
            if let Some((old, new)) = leveled {
                let hero_ups = self.hero_level_ups(player_id, old, new);
                out.push(out_msg(
                    player_id,
                    &wr::LevelUp {
                        new_run_level: new,
                        levels_gained: new - old,
                        heroes: hero_ups,
                    },
                ));
            }
        }

        // Spend the HERO's own copy first. Draining the shared inventory while the
        // drinker had one in their pouch would quietly unstock the party to keep one
        // hero topped up — and the pouch is the copy that will matter in the next fight.
        let from_pouch = in_pouch > 0;
        if let Some(r) = self.run.run_mut(player_id) {
            if from_pouch {
                r.spend_from_pouch(slot, &req.item_kind);
            } else {
                if let Some(stack) = r.backpack.iter_mut().find(|i| i.item_kind == req.item_kind) {
                    stack.quantity -= 1;
                }
                r.backpack.retain(|i| i.quantity > 0);
            }
        }
        if from_pouch {
            out.extend(self.pouches_msg(player_id));
        } else {
            out.push(out_msg(
                player_id,
                &wr::BackpackUpdate {
                    changes: vec![wr::BackpackChange {
                        item: ItemStack {
                            item_id: String::new(),
                            item_kind: req.item_kind,
                            quantity: 1,
                            insurance: None,
                        },
                        delta: "removed".to_string(),
                        cause: "field_item".to_string(),
                    }],
                    chits_delta: 0,
                    gear_added: Vec::new(),
                },
            ));
        }
        // The roster is how the client learns the new HP (and level) — same message
        // the party panel already listens to.
        let refreshed = self.party_views(player_id);
        let (synergies, combos) = self.party_depth(player_id);
        let abilities = self.party_ability_views(player_id);
        out.push(out_msg(player_id, &wr::Party { heroes: refreshed, synergies, combos, abilities }));
        (out, Vec::new())
    }

    /// `run.pouches` for one player, or nothing when they have no run. A whole
    /// snapshot: a pouch is `hero_pouch_slots` deep at most, so re-sending it is
    /// cheaper than reconciling deltas the client might have missed.
    fn pouches_msg(&self, player_id: &str) -> Vec<Outgoing> {
        let cap = self.balance.runs.hero_pouch_slots;
        let Some(r) = self.run.runs.iter().find(|r| r.player_id == player_id) else {
            return Vec::new();
        };
        let pouches = r
            .pouches
            .iter()
            .enumerate()
            .map(|(i, items)| wr::PouchView {
                hero_slot: i as i32,
                items: items.clone(),
                capacity: cap,
            })
            .collect();
        vec![out_msg(player_id, &wr::Pouches { pouches })]
    }

    /// Move an item between the shared bag and one hero's pouch (`run.move_item`).
    ///
    /// Refused during a battle: the whole point of the two containers is that a fight
    /// is fought with what the heroes were already carrying, so allowing a mid-fight
    /// restock would make the pouch a formality.
    fn handle_move_item(
        &mut self,
        player_id: &str,
        raw: RawEnvelope,
    ) -> (Vec<Outgoing>, Vec<WorldEffect>) {
        let req: wr::MoveItem = match serde_json::from_value(raw.payload) {
            Ok(v) => v,
            Err(_) => {
                return (
                    vec![error(player_id, ErrorCode::ValidationError, "bad move_item", Some(raw.seq))],
                    Vec::new(),
                )
            }
        };
        if self.battle_of_player(player_id).is_some() {
            return (
                vec![error(
                    player_id,
                    ErrorCode::InvalidState,
                    "Not in a fight — a hero carries what they set out with.",
                    Some(raw.seq),
                )],
                Vec::new(),
            );
        }
        let balance = self.balance.clone();
        let qty = if req.quantity <= 0 { 1 } else { req.quantity };
        let slot = if req.hero_slot < 0 { usize::MAX } else { req.hero_slot as usize };
        let Some(r) = self.run.run_mut(player_id) else {
            return (
                vec![error(player_id, ErrorCode::InvalidState, "Not in a run.", Some(raw.seq))],
                Vec::new(),
            );
        };
        if slot >= r.pouches.len() {
            return (
                vec![error(player_id, ErrorCode::ValidationError, "No such hero.", Some(raw.seq))],
                Vec::new(),
            );
        }
        let moved = r.move_item(slot, &req.item_kind, qty, req.to_pouch, &balance);
        if moved == 0 {
            let why = if req.to_pouch {
                "That hero's pouch is full."
            } else {
                "The bag is full."
            };
            return (
                vec![error(player_id, ErrorCode::ValidationError, why, Some(raw.seq))],
                Vec::new(),
            );
        }
        // Both ends changed, so both are re-reported: the bag as a delta (the HUD counts
        // items) and the pouches whole.
        let item = ItemStack {
            item_id: String::new(),
            item_kind: req.item_kind.clone(),
            quantity: moved,
            insurance: None,
        };
        let mut out = vec![out_msg(
            player_id,
            &wr::BackpackUpdate {
                changes: vec![wr::BackpackChange {
                    item,
                    delta: if req.to_pouch { "removed" } else { "added" }.to_string(),
                    cause: "pouch_transfer".to_string(),
                }],
                chits_delta: 0,
                gear_added: Vec::new(),
            },
        )];
        out.extend(self.pouches_msg(player_id));
        (out, Vec::new())
    }

    /// The channel tick rate for a node kind, from its material class (`[harvest]`).
    fn harvest_tick_ms(&self, kind: &str) -> Option<u64> {
        let res = self.balance.resource.get(kind)?;
        let class = meld_proto::materials::material(&res.material)
            .map(|m| m.class.wire())
            .unwrap_or("");
        Some(self.balance.harvest.node_yield(class).1)
    }

    /// Drop `player_id`'s harvest channel and tell the instance why. Returns no
    /// messages when there was nothing running, so every interrupt site can call it
    /// unconditionally.
    fn end_harvest(&mut self, player_id: &str, reason: &str) -> Vec<Outgoing> {
        if self.harvest.remove(player_id).is_none() {
            return Vec::new();
        }
        // Only clear the *channeling* state: a channel broken by a battle must not
        // stamp `active` over the state the battle itself just set.
        if let Some(a) = self.arena.avatar_mut(player_id) {
            if a.state == "channeling" {
                a.state = "active".to_string();
            }
        }
        self.run
            .runs
            .iter()
            .map(|r| r.player_id.clone())
            .collect::<Vec<_>>()
            .iter()
            .map(|pid| {
                out_msg(
                    pid,
                    &wr::ChannelInterrupted {
                        player_id: player_id.to_string(),
                        reason: reason.to_string(),
                    },
                )
            })
            .collect()
    }

    /// Hand one unit to every harvest channel whose tick has elapsed (MS-2). Each unit
    /// is banked the instant it comes out, so an interrupt costs only the tick in
    /// flight. A channel ends here when the node runs dry (`exhausted`) or when the
    /// player is no longer beside it (`moved`) — walking away needs no special case,
    /// because `take_one` re-checks range and elevation every tick.
    fn advance_harvests(&mut self) -> Vec<Outgoing> {
        let now = now_ms();
        let due: Vec<(String, String)> = self
            .harvest
            .iter()
            .filter(|(_, h)| h.next_at <= now)
            .map(|(pid, h)| (pid.clone(), h.node_id.clone()))
            .collect();
        if due.is_empty() {
            return Vec::new();
        }
        let balance = self.balance.clone();
        let mut out = Vec::new();
        for (pid, node_id) in due {
            // A Keeper's two gathering perks, rolled off the node and the tick so the
            // outcome is reproducible rather than wall-clock: GREEN THUMB pays a second
            // unit into the pack, THE WHOLE VEIN takes that unit without charging the
            // node for it. Both are why the Open Flower is the class you bring to gather.
            let perks = self.perks_for(&pid);
            let material = hash_str(&node_id) ^ hash_str(&pid) ^ now;
            let free = roll_unit(material) < perks.keeper_free_unit_chance as f64;
            let extra = roll_unit(material ^ 0xA5A5_A5A5) < perks.keeper_extra_unit_chance as f64;
            let Some(kind) = self.arena.take_one(&pid, &node_id) else {
                out.extend(self.end_harvest(&pid, "moved"));
                continue;
            };
            if free {
                self.arena.refund_one(&node_id);
            }
            let Some(res) = balance.resource.get(&kind) else {
                out.extend(self.end_harvest(&pid, "cancelled"));
                continue;
            };
            let item = ItemStack {
                item_id: Uuid::now_v7().to_string(),
                item_kind: res.material.clone(),
                quantity: if extra { 2 } else { 1 },
                insurance: None,
            };
            if let Some(r) = self.run.run_mut(&pid) {
                r.backpack.push(item.clone());
            }
            let _ = self.db_writes.send(DbWrite::SkillXp(
                pid.clone(),
                res.skill.clone(),
                res.xp,
            ));
            out.push(out_msg(
                &pid,
                &wr::BackpackUpdate {
                    changes: vec![wr::BackpackChange {
                        item,
                        delta: "added".to_string(),
                        cause: format!("harvest:{kind}"),
                    }],
                    chits_delta: 0,
                    gear_added: Vec::new(),
                },
            ));
            let dry = self
                .arena
                .resources
                .iter()
                .find(|n| n.entity_id == node_id)
                .map(|n| n.depleted())
                .unwrap_or(true);
            if dry {
                // Taking the whole vein rather than skimming it is what the Open Flower
                // recruits on (CL-1's milestone shape: the world reports the fact, the
                // Router decides what it earns).
                self.pending_effects.push(WorldEffect::Milestone {
                    player_id: pid.clone(),
                    milestone: meld_proto::unlocks::Milestone::NodeExhausted,
                });
                out.extend(self.end_harvest(&pid, "exhausted"));
            } else if let Some(h) = self.harvest.get_mut(&pid) {
                h.next_at = now + h.tick_ms;
            }
        }
        out
    }

    /// Open the treasure chest the avatar is standing next to: roll its loot
    /// (a richer chit payout than a kill, a biome material, and deep-enough red
    /// gear) into the backpack. The chest shows opened on the next snapshot.
    fn handle_open_chest(
        &mut self,
        player_id: &str,
        raw: RawEnvelope,
    ) -> (Vec<Outgoing>, Vec<WorldEffect>) {
        // A chest pays out like several kills' worth of chits (economy.md S2).
        const CHEST_RICHNESS: i32 = 4;
        let req: wr::OpenChest = match serde_json::from_value(raw.payload) {
            Ok(v) => v,
            Err(_) => {
                return (vec![error(player_id, ErrorCode::ValidationError, "bad open_chest", Some(raw.seq))], Vec::new())
            }
        };
        let balance = self.balance.clone();
        if self.battle_of_player(player_id).is_some() {
            return (vec![error(player_id, ErrorCode::InvalidState, "Resolve the battle first.", Some(raw.seq))], Vec::new());
        }
        // DG-3b(3/n) C: a dungeon chest (`dchest-<id>`) loots via the DungeonInstance
        // (unlocked once its `when`, e.g. `boss_dead`, is satisfied) — not the arena.
        if self.dungeon_of(player_id).is_some() {
            return self.open_dungeon_chest(player_id, &req.entity_id, raw.seq);
        }
        let Some((_tier, distance)) = self.arena.open_chest(player_id, &req.entity_id) else {
            return (vec![error(player_id, ErrorCode::OutOfRange, "No chest in reach.", Some(raw.seq))], Vec::new());
        };
        // Deterministic per (chest, player); the chest can only be opened once.
        let seed = self.arena.seed ^ hash_str(&req.entity_id) ^ hash_str(player_id);
        let loot = meld_world::roll_creature_loot(&balance, distance, CHEST_RICHNESS, 1.0, seed);
        let loot_item = ItemStack {
            item_id: Uuid::now_v7().to_string(),
            item_kind: loot.material.to_string(),
            quantity: loot.material_qty,
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
                class_key: g.class_key.clone(),
                // The tier is decided by the ROLL, not the drop site.
                insurance: g.insurance,
                tier: g.tier,
                atk_bonus: g.atk_bonus,
                def_bonus: g.def_bonus,
                spd_bonus: g.spd_bonus,
                base_max_durability: g.max_durability,
                max_durability: g.max_durability,
                equipped_hero_slot: None,
                damage_modifiers: g.damage_modifiers.clone(),
                family: g.family.clone(),
                armor_weight: g.armor_weight.clone(),
                affixes: g.affixes.clone(),
                unique_key: g.unique_key.clone(),
                set_key: g.set_key.clone(),
            })
            .collect();
        let mut chest_items = vec![loot_item];
        if !loot.potion.is_empty() {
            chest_items.push(ItemStack {
                item_id: Uuid::now_v7().to_string(),
                item_kind: loot.potion.to_string(),
                quantity: 1,
                insurance: None,
            });
        }
        let mut run_gear_snapshot = None;
        if let Some(r) = self.run.run_mut(player_id) {
            // A full pack drops the loot on the floor of the world, so to speak: the
            // player is told, and the choice of what to carry stays theirs.
            for item in &chest_items {
                if !r.try_carry(item.clone(), &balance) {
                    tracing::debug!("pack full for {player_id}; {} not carried", item.item_kind);
                }
            }
            r.chits += loot.chits;
            r.looted_gear.extend(gear.iter().cloned());
            if !gear.is_empty() {
                run_gear_snapshot = Some(r.looted_gear.clone());
            }
        }
        let mut out = vec![out_msg(
            player_id,
            &wr::BackpackUpdate {
                changes: chest_items
                    .into_iter()
                    .map(|item| wr::BackpackChange {
                        item,
                        delta: "added".to_string(),
                        cause: "chest".to_string(),
                    })
                    .collect(),
                chits_delta: loot.chits,
                gear_added: gear,
            },
        )];
        if let Some(gear) = run_gear_snapshot {
            out.push(out_msg(player_id, &wr::RunGear { gear }));
        }
        (out, Vec::new())
    }

    /// DG-3b(3/n) C: loot a dungeon chest (`dchest-<id>`). Requires the player in the
    /// dungeon, standing by the chest, and its `when` satisfied (e.g. the boss dead).
    /// Rolled loot rides the dungeon's stamped distance (design §6); authored contents
    /// are granted verbatim. Reuses the run-backpack banking of `handle_open_chest`.
    fn open_dungeon_chest(&mut self, pid: &str, entity_id: &str, seq: u32) -> (Vec<Outgoing>, Vec<WorldEffect>) {
        // Dungeons out-reward open-world chests at equal distance (the risk premium).
        const DUNGEON_CHEST_RICHNESS: i32 = 6;
        let chest_id = entity_id.strip_prefix("dchest-").unwrap_or(entity_id).to_string();
        let balance = self.balance.clone();
        let radius = self.balance.world.interaction_radius_tiles;
        let Some((key, _floor)) = self.dungeon_of(pid) else {
            return (vec![error(pid, ErrorCode::InvalidState, "Not in a dungeon.", Some(seq))], Vec::new());
        };
        let reward = {
            let Some(d) = self.dungeons.get(&key) else {
                return (vec![error(pid, ErrorCode::InvalidState, "Not in a dungeon.", Some(seq))], Vec::new());
            };
            let placement = d.def().placements.iter().find(|p| p.id == chest_id);
            let near = match (placement, d.occupant(pid)) {
                (Some(p), Some(o)) => {
                    o.floor == p.floor
                        && o.pos.distance_to(&meld_dungeon_run::cell_center(p.x, p.y)) <= radius
                }
                _ => false,
            };
            if !near {
                return (vec![error(pid, ErrorCode::OutOfRange, "No chest in reach.", Some(seq))], Vec::new());
            }
            if !d.chest_openable(&chest_id) {
                return (vec![error(pid, ErrorCode::InvalidState, "The vault is sealed — defeat the boss first.", Some(seq))], Vec::new());
            }
            let seed = d.key ^ hash_str(&chest_id) ^ hash_str(pid);
            d.resolve_chest(&chest_id, &balance, DUNGEON_CHEST_RICHNESS, 1.0, seed)
        };
        let Some(reward) = reward else {
            return (vec![error(pid, ErrorCode::NotFound, "No such dungeon chest.", Some(seq))], Vec::new());
        };
        if let Some(d) = self.dungeons.get_mut(&key) {
            d.open_chest(&chest_id);
        }
        // Build the backpack additions: the rolled material/chits/gear + authored items.
        let mut changes: Vec<wr::BackpackChange> = Vec::new();
        let mut gear_added: Vec<LootGear> = Vec::new();
        let mut chits_delta = 0i64;
        if let Some(l) = &reward.rolled {
            chits_delta += l.chits;
            let item = ItemStack {
                item_id: Uuid::now_v7().to_string(),
                item_kind: l.material.to_string(),
                quantity: l.material_qty,
                insurance: None,
            };
            changes.push(wr::BackpackChange { item, delta: "added".to_string(), cause: "chest".to_string() });
            if !l.potion.is_empty() {
                let potion = ItemStack {
                    item_id: Uuid::now_v7().to_string(),
                    item_kind: l.potion.to_string(),
                    quantity: 1,
                    insurance: None,
                };
                changes.push(wr::BackpackChange { item: potion, delta: "added".to_string(), cause: "chest".to_string() });
            }
            if let Some(g) = &l.gear {
                gear_added.push(LootGear {
                    gear_id: Uuid::now_v7().to_string(),
                    name: g.name.clone(),
                    rarity: g.rarity.clone(),
                    slot: g.slot.clone(),
                    class_key: g.class_key.clone(),
                    // The tier is decided by the ROLL, not the drop site.
                insurance: g.insurance,
                    tier: g.tier,
                    atk_bonus: g.atk_bonus,
                    def_bonus: g.def_bonus,
                    spd_bonus: g.spd_bonus,
                    base_max_durability: g.max_durability,
                    max_durability: g.max_durability,
                    equipped_hero_slot: None,
                    damage_modifiers: g.damage_modifiers.clone(),
                    family: g.family.clone(),
                    armor_weight: g.armor_weight.clone(),
                    affixes: g.affixes.clone(),
                    unique_key: g.unique_key.clone(),
                    set_key: g.set_key.clone(),
                });
            }
        }
        // Authored contents: granted as backpack items (authored gear-as-real-gear is
        // a DG-5 refinement — it rides as a named item for now).
        for it in &reward.authored {
            let kind = it.gear.clone().or_else(|| it.item.clone()).unwrap_or_default();
            let item = ItemStack {
                item_id: Uuid::now_v7().to_string(),
                item_kind: kind,
                quantity: it.quantity.max(1) as i32,
                insurance: None,
            };
            changes.push(wr::BackpackChange { item, delta: "added".to_string(), cause: "chest".to_string() });
        }
        let mut run_gear_snapshot = None;
        if let Some(r) = self.run.run_mut(pid) {
            for c in &changes {
                r.backpack.push(c.item.clone());
            }
            r.chits += chits_delta;
            r.looted_gear.extend(gear_added.iter().cloned());
            if !gear_added.is_empty() {
                run_gear_snapshot = Some(r.looted_gear.clone());
            }
        }
        let mut out = vec![out_msg(pid, &wr::BackpackUpdate { changes, chits_delta, gear_added })];
        if let Some(gear) = run_gear_snapshot {
            out.push(out_msg(pid, &wr::RunGear { gear }));
        }
        (out, Vec::new())
    }

}

impl GameState {
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
            deepest: i32,
        }
        let (banks, members): (Vec<Banked>, Vec<String>) = {
            let Some(inst) = self.world.as_mut() else {
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
                    // A pouch comes home too: it is carried loot like anything in the
                    // bag, just held by a hero instead of the party.
                    let mut items = std::mem::take(&mut r.backpack);
                    for pouch in std::mem::take(&mut r.pouches) {
                        items.extend(pouch);
                    }
                    let gear = std::mem::take(&mut r.looted_gear);
                    let chits = std::mem::replace(&mut r.chits, 0);
                    r.result = Some(RunResult::Extracted);
                    banks.push(Banked {
                        player_id: pid.clone(),
                        run_id: r.run_id.clone(),
                        items,
                        chits,
                        gear,
                        deepest: r.max_distance_reached,
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
            // CL-1: you came back, and you brought the proof. That is what the
            // Hunters' hall recruits on ("hunts are only rewarded with evidence of
            // kills"), so a completed extraction is the Hunter's trigger.
            self.grant_milestone(&b.player_id, meld_proto::unlocks::Milestone::Extracted);
            // AD-4: and it is what a "come home from depth" hunt is waiting for.
            self.credit_hunts(&b.player_id, &HuntFact::Extracted(b.deepest));
            // Reaching the city burns ephemeral gear even when you WALKED in. It is
            // the strongest gear in the game precisely because it can never be banked
            // — surviving extraction would make it merely the best loot.
            let _ = self.db_writes.send(DbWrite::BurnEphemeral(b.player_id.clone()));
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
                // Convert looted gear to owned Vault gear, keeping whichever
                // insurance it was rolled with.
                let looted: Vec<meld_db::LootedGear> = b
                    .gear
                    .iter()
                    .filter_map(|g| {
                        Some(meld_db::LootedGear {
                            insurance: g.insurance,
                            gear_id: Uuid::parse_str(&g.gear_id).ok()?,
                            name: g.name.clone(),
                            slot: g.slot.clone(),
                            class_key: g.class_key.clone(),
                            tier: g.tier,
                            atk_bonus: g.atk_bonus,
                            def_bonus: g.def_bonus,
                            spd_bonus: g.spd_bonus,
                            base_max_durability: g.base_max_durability,
                            max_durability: g.max_durability,
                            damage_modifiers: modifiers_json(&g.damage_modifiers),
                            family: g.family.clone(),
                            armor_weight: g.armor_weight.clone(),
                            affixes: meld_proto::affixes::to_json(&g.affixes),
                            unique_key: g.unique_key.clone(),
                            set_key: g.set_key.clone(),
                        })
                    })
                    .collect();
                if let Err(e) = db.insert_looted_gear(uid, &looted).await {
                    tracing::error!("insert_looted_gear failed for {}: {e}", b.player_id);
                }
                // Extraction success credits Alchemy XP (GDD §4.1) — for the plants and
                // monster parts you brought back, NOT for the kit you dived in with.
                // Counting every stack pays out for walking in and straight back out
                // with the starting salves and elixirs still in the bag.
                let axp = items_kv
                    .iter()
                    .filter(|(kind, _)| {
                        !meld_proto::consumables::is_consumable(kind) && kind != "town_portal"
                    })
                    .count() as i64
                    * alchemy_per;
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
                        durability_loss_applied: self
                            .world
                            .as_ref()
                            .is_some_and(|w| w.durability_charged.contains(&b.player_id)),
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
}

impl WorldActor {
    // --- tick ---------------------------------------------------------------
    //
    // The authoritative per-tick advance and its battle subtree live on the world
    // actor (SC-3): `self` IS the world here. World-scoped logic can't touch the
    // Router's sessions or tear the world down, so it emits [`WorldEffect`]s that
    // `GameState::apply_world_effects` applies after the borrow ends.

    fn tick(&mut self) -> (Vec<Outgoing>, Vec<WorldEffect>) {
        let dt = (self.balance.battle.tick_ms.max(1) as f64) / 1000.0;
        self.tick_count += 1;
        let mut out = Vec::new();
        let mut effects: Vec<WorldEffect> = std::mem::take(&mut self.pending_effects);

        // 1) The overworld always advances — even while some party is in a battle.
        // Roaming creatures move and skirmish with rival factions; creatures pulled
        // into a battle are `in_battle` and hold still. Doing this every tick (not
        // only in the no-battle branch) is what keeps players who *aren't* fighting
        // live: without it, one player's fight froze the whole instance and starved
        // everyone else of snapshots until their sockets dropped (the co-op crash).
        // Phoenix Guard "Bulwark": per-player creature-aggro multipliers (≤1 shrinks how
        // close a creature will chase/skirmish-pull that party). Built before the
        // mut borrow below (perks_for needs a shared borrow of the instance).
        let aggro_mult: HashMap<String, f64> = {
            let ids: Vec<String> = self.run.runs.iter().map(|r| r.player_id.clone()).collect();
            ids.into_iter()
                .map(|pid| {
                    let m = self.perks_for(&pid).phoenix_guard_aggro_mult as f64;
                    (pid, m)
                })
                .collect()
        };
        let mut created_sections: Vec<usize> = Vec::new();
        {
            let balance = self.balance.clone();
            self.arena.step_creatures_with_aggro(dt, &aggro_mult);
            // Stream in new sections as the frontier player advances (endless world).
            // Difficulty is radial (distance = hypot from the hub), so in the radial
            // world the frontier is the player's RADIUS; in corridor mode it's x.
            let radial = balance.worldgen.radial_arc_degrees > 0.0;
            let reach = self
                .arena
                .avatars
                .iter()
                .map(|a| if radial { a.position.x.hypot(a.position.y) } else { a.position.x })
                .fold(f64::NEG_INFINITY, f64::max);
            if reach.is_finite() {
                created_sections = self.arena.ensure_frontier(&balance, reach);
            }
        }
        // AD-4: stand up any bounty mark the world has now grown out far enough to hold.
        // Cheap: only contracts not yet placed are considered, and each is tried once.
        if !self.bounties.is_empty() {
            let balance = self.balance.clone();
            let pending: Vec<(String, String, meld_proto::bounties::BountySpec)> = self
                .bounties
                .iter()
                .flat_map(|(pid, specs)| {
                    specs
                        .iter()
                        .filter(|(id, spec)| {
                            // A descent contract waits for a descent; standing it up in
                            // the open too would be the same mark twice.
                            spec.venue == meld_proto::bounties::Venue::Overworld
                                && !self.marks_placed.contains(id)
                        })
                        .map(move |(id, spec)| (pid.clone(), id.clone(), spec.clone()))
                })
                .collect();
            for (pid, id, spec) in pending {
                let seed = hash_str(&id);
                if self.arena.place_bounty_mark(&balance, &pid, &id, &spec, seed) {
                    self.marks_placed.insert(id);
                }
            }
        }
        // Stream the freshly-generated sections' terrain (+ trail segment) so the
        // client extends its relief and path — the endless-world payoff.
        if !created_sections.is_empty() {
            let (rh, cl) = (self.arena.radial_half(), self.arena.corridor_lateral());
            for &i in &created_sections {
                let Some(area) = self.arena.areas.get(i) else { continue };
                let seg = if i + 1 < self.arena.path.len() {
                    vec![self.arena.path[i], self.arena.path[i + 1]]
                } else {
                    Vec::new()
                };
                // This streamed section's peaks (centre radius in its band); the client
                // appends them so the streamed mountains render.
                let (s0, e0) = (area.start_x, area.end_x);
                let section_peaks: Vec<[f32; 4]> = self
                    .arena
                    .peaks
                    .iter()
                    .filter(|p| {
                        let r = (p[0] as f64).hypot(p[1] as f64);
                        r >= s0 && r < e0
                    })
                    .copied()
                    .collect();
                let msg = terrain_section_msg(area, seg, rh, cl, section_peaks);
                for r in &self.run.runs {
                    out.push(out_msg(&r.player_id, &msg));
                }
            }
        }
        // DG-3b: place a chanced hand-designed dungeon entrance for every section not
        // yet scanned — the initial chain (first tick) AND each streamed section —
        // drawn from the section's biome pool, on its clear-path segment. Suppressed
        // in the tutorial; area 0 (spawn) is skipped. Streams to clients as
        // `entrance:<dungeon>`; `run.enter_dungeon` descends.
        if !self.tutorial {
            let chance = self.balance.worldgen.dungeon_spawn_chance;
            while self.entrances_scanned < self.arena.areas.len() {
                let i = self.entrances_scanned;
                self.entrances_scanned += 1;
                if i == 0 {
                    continue;
                }
                let (biome, portal) = {
                    let Some(area) = self.arena.areas.get(i) else { continue };
                    (area.biome, area.portal)
                };
                let p0 = self.arena.path.get(i).copied().unwrap_or(portal);
                let p1 = self.arena.path.get(i + 1).copied().unwrap_or(p0);
                let seed = meld_world::section_seed(self.arena.seed, i);
                // Never on the doorstep: a dungeon takes no Town Portal, so one you
                // can see from the city gate is a committed space a new player has no
                // way to read as one.
                let too_close =
                    p0.distance_floor() < self.balance.worldgen.dungeon_min_distance as i64;
                if let Some(pl) = (!too_close)
                    .then(|| meld_dungeon_run::place_entrance(seed, biome, chance, p0, p1))
                    .flatten()
                {
                    self.entrances.push(DungeonEntrance {
                        entity_id: format!("dungeon-entrance-{i}"),
                        dungeon: pl.dungeon,
                        position: pl.position,
                    });
                }
            }
        } else if !self.tutorial_entrance_placed {
            // DG-3-tutorial: the guided [T]-dive walkthrough's "how to enter a
            // dungeon" step needs one to find. A deliberate, scoped exception to
            // `dungeon_min_distance` (the doorstep-dungeon protection above) —
            // this ONE hand-placed entrance is meant to be found early and on
            // foot, by design; it never affects normal-run placement.
            // `guardia_forest` is hand-picked rather than drawn from
            // `place_entrance`'s random biome pool: it's the one authored forest
            // dungeon that needs just a single body on its gate
            // (`bodies_required() == 1`), so a solo tutorial player is never
            // handed a co-op-only door at the one entrance the tutorial just
            // told them to walk through.
            if let Some(area0) = self.arena.areas.first() {
                self.tutorial_entrance_placed = true;
                self.entrances.push(DungeonEntrance {
                    entity_id: "dungeon-entrance-tutorial".to_string(),
                    dungeon: "guardia_forest",
                    // Just a few tiles past area 0's own portal — right after
                    // the tutorial's guaranteed fight, not a long walk further
                    // out, so "close to the start" the way the harvest node
                    // and chest already are. Off the clear path, mirroring the
                    // ±3.0 lateral convention those two already use.
                    position: Position::new(area0.portal.x + 3.0, 4.0),
                });
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
        let battle_ids: Vec<String> = self.battles.iter().map(|b| b.battle_id.clone()).collect();
        for id in battle_ids {
            let events = match self.battle_by_id_mut(&id) {
                Some(slot) => slot.battle.tick(),
                None => continue,
            };
            let (evout, eveffects) = self.emit_battle_events(&id, events);
            out.extend(evout);
            effects.extend(eveffects);
            // Gauge keepalive (event-driven + periodic per battle.md) — only if the
            // battle is still running (didn't end on this tick).
            if let Some(slot) = self.battle_by_id(&id) {
                out.extend(self.gauge_update_msgs(slot));
            }
        }

        // 2b) Every WATCHED feed (`SOC-3`): drop the ones no longer watchable, and drive
        // the creature clashes, which have no engine behind them. A watched player battle
        // needs nothing here — the watcher rides its audience funnel above.
        out.extend(self.sweep_watchers());

        // 3) Snapshot the overworld to everyone NOT currently in a battle. This
        // runs every tick regardless of whether any battle is active, so roaming
        // teammates keep receiving world state while others fight.
        out.extend(self.snapshot_msgs());

        // 4) The Shifting Lands, and the slow recovery between their Shifts. Both run
        // last so they see this tick's deaths and harvests, and both are driven off
        // `tick_count` rather than a clock (CANON §W2).
        self.arena.advance_builds(self.tick_count);
        // The general safety net (BD-2): anyone standing inside something impassable is
        // walked to open ground. The Shift has its own rescue because it knows which
        // region moved; this is the same mechanism with no event behind it, for every
        // other way a player can end up inside geometry. On a cadence — being stuck for a
        // tenth of a second is nothing, being stuck forever means closing the game.
        if self.tick_count.is_multiple_of(self.balance.building.stuck_check_ticks.max(1)) {
            for (pid, to) in self.arena.rescue_trapped() {
                let seq = self.arena.avatar(&pid).map(|a| a.last_input_seq).unwrap_or(0);
                out.push(out_msg(
                    &pid,
                    &wm::PositionCorrection { position: to, last_input_seq: seq },
                ));
            }
        }
        out.extend(self.advance_shift());
        {
            let balance = self.balance.clone();
            self.arena.regrow(&balance, self.tick_count);
        }

        // 5) Reclaim slain creatures so `arena.monsters` stays bounded over a long
        // dive instead of accumulating a corpse per kill forever. Safe here: this is
        // after all battle-end processing (which refers to creatures by stable id,
        // not index) and after the snapshot (which already omits defeated creatures).
        self.arena.prune_defeated();
        (out, effects)
    }

    /// Drive the Shifting Lands one tick (CANON D20/§W2): put the tell up when a
    /// generation's warning window opens, land it when its tick arrives, retire the
    /// generation either way.
    ///
    /// The schedule is read from `(seed, generation)` every tick rather than cached, so
    /// a world reloaded from Postgres resumes mid-warning exactly where it left off —
    /// which is the whole point of the scheduler being pure (§W5: two integers replay
    /// the history).
    fn advance_shift(&mut self) -> Vec<Outgoing> {
        let balance = self.balance.clone();
        let seed = self.arena.seed;
        let roll = meld_world::shift::roll(&balance, seed, self.shift_generation);
        let mut out = Vec::new();

        if self.tick_count >= roll.warn_tick && !self.shift_warned {
            self.shift_warned = true;
            if let Some((first, last)) = self.arena.shift_region(&balance, &roll) {
                let (inner, outer) = self.arena.shift_band(first, last);
                let becoming = self.arena.incoming_biome_for(&balance, &roll, first);
                let lands_in_ms =
                    roll.land_tick.saturating_sub(self.tick_count) * balance.battle.tick_ms;
                for r in &self.run.runs {
                    let caught = self
                        .arena
                        .avatar(&r.player_id)
                        .map(|a| {
                            let rad = self.arena.corridorize(&a.position).x;
                            rad >= inner && rad < outer
                        })
                        .unwrap_or(false);
                    out.push(out_msg(
                        &r.player_id,
                        &ww::ShiftWarning {
                            generation: roll.generation,
                            inner_radius: inner,
                            outer_radius: outer,
                            biome: becoming.to_string(),
                            lands_in_ms,
                            caught,
                        },
                    ));
                }
            }
        }

        if self.tick_count < roll.land_tick {
            return out;
        }
        self.shift_generation += 1;
        self.shift_warned = false;
        let Some((first, last)) = self.arena.shift_region(&balance, &roll) else {
            return out;
        };
        // CANON §W3: an anchor does not alter the natural schedule — that stays a pure
        // function of the seed (§W5) — it alters the OUTCOME, and the suppression is the
        // event. So the roll happens either way and the anchors are consulted here, which
        // is also why a held Shift never reaches `shift_log`: nothing about the world
        // changed except the anchors' HP, and that rides the delta already.
        if let Some(held) = self.arena.hold_shift(&balance, first, last) {
            // The engine stays pure (no wire types), so this one conversion has to exist —
            // but it is written to FAIL rather than drift. Destructuring the engine's record
            // means a field added to it stops compiling here; reading `a.field` into a struct
            // literal would have silently left the new field off the wire, which is exactly
            // how `max_hp` came to be sent and never read. If you must hand-write a bridge,
            // hand-write one the compiler checks.
            let anchors: Vec<ww::HeldAnchor> = held
                .anchors
                .iter()
                .map(|a| {
                    let meld_world::HeldAnchor { entity_id, damage, hp, max_hp, destroyed } = a;
                    ww::HeldAnchor {
                        entity_id: entity_id.clone(),
                        damage: *damage,
                        hp: *hp,
                        max_hp: *max_hp,
                        destroyed: *destroyed,
                    }
                })
                .collect();
            let msg = ww::ShiftHeld {
                generation: roll.generation,
                inner_radius: held.inner_radius,
                outer_radius: held.outer_radius,
                anchors,
            };
            for r in &self.run.runs {
                out.push(out_msg(&r.player_id, &msg));
            }
            return out;
        }
        let from = self.arena.areas.get(first).map(|a| a.biome).unwrap_or("").to_string();
        self.shift_log.push((roll.generation, first, last));
        let outcome = self.arena.apply_shift(&balance, &roll, first, last);

        // The Force blast, then the retile. Order matters for the client: the damage
        // numbers belong to the ground the player was standing on, not the ground that
        // replaced it.
        let caught: Vec<(String, f64)> = outcome.caught.clone();
        let mut dead: Vec<String> = Vec::new();
        let mut fell: Vec<(String, i32)> = Vec::new();
        let mut hits: HashMap<String, Vec<i32>> = HashMap::new();
        for (pid, fraction) in caught {
            let Some(classes) = self.party_classes.get(&pid).cloned() else { continue };
            let levels: Vec<i32> = self
                .run
                .runs
                .iter()
                .find(|r| r.player_id == pid)
                .map(|r| r.hero_levels.clone())
                .unwrap_or_default();
            let Some(hp) = self.hero_hp.get_mut(&pid) else { continue };
            let mut taken = vec![0; hp.len()];
            for (slot, cur) in hp.iter_mut().enumerate() {
                if *cur <= 0 {
                    continue;
                }
                let class = classes.get(slot).copied().unwrap_or(CharacterClass::Explorer);
                let level = levels.get(slot).copied().unwrap_or(1).max(1);
                let max = meld_run::max_hp_at_level(class, level, &balance);
                let dmg = ((max as f64) * fraction).round().max(1.0) as i32;
                taken[slot] = dmg.min(*cur);
                *cur = (*cur - dmg).max(0);
                if *cur == 0 {
                    // The weather is the other thing that kills a hero with no battle
                    // to end it (GR-2).
                    fell.push((pid.clone(), slot as i32));
                }
            }
            if hp.iter().all(|h| *h <= 0) {
                dead.push(pid.clone());
            }
            hits.insert(pid, taken);
        }
        self.charge_non_battle_falls(&fell);

        let members: Vec<String> = self.run.runs.iter().map(|r| r.player_id.clone()).collect();
        for pid in &members {
            out.push(out_msg(
                pid,
                &ww::Shifted {
                    generation: roll.generation,
                    inner_radius: outcome.inner_radius,
                    outer_radius: outcome.outer_radius,
                    biome: outcome.biome.clone(),
                    from_biome: from.clone(),
                    wiped: outcome.wiped.clone(),
                    damage: hits.get(pid).cloned().unwrap_or_default(),
                },
            ));
        }
        // Repaint the ground. The client keys biome ground + HUD label off per-section
        // radius rings, so re-sending each retiled section IS the retile — no new
        // rendering path, which is why the Shift is section-granular in the first place.
        let (rh, cl) = (self.arena.radial_half(), self.arena.corridor_lateral());
        for &i in &outcome.sections {
            let Some(area) = self.arena.areas.get(i) else { continue };
            // The section's NEW mountains ride with it. The client keys peaks by section,
            // so this replaces whatever that ring used to raise rather than adding to it —
            // which is what lets a Shift to Desert actually flatten a range.
            let peaks = outcome
                .peaks
                .iter()
                .find(|(n, _)| *n == i)
                .map(|(_, p)| p.clone())
                .unwrap_or_default();
            let msg = terrain_section_msg(area, Vec::new(), rh, cl, peaks);
            for pid in &members {
                out.push(out_msg(pid, &msg));
            }
        }
        // Anyone the new land was strewn on top of has already been walked to the
        // region's entry; correct their client so it snaps there instead of sliding
        // across the map chasing an authoritative position it never agreed to.
        for (pid, to) in &outcome.moved {
            let seq = self.arena.avatar(pid).map(|a| a.last_input_seq).unwrap_or(0);
            out.push(out_msg(
                pid,
                &wm::PositionCorrection { position: *to, last_input_seq: seq },
            ));
        }
        for pid in dead {
            out.extend(self.world_death(&pid));
        }
        out
    }

    /// Auto-collect ground loot (creature-skirmish drops) for every active player
    /// standing on it, banking each into the run backpack and reporting the change.
    fn collect_ground_loot(&mut self) -> Vec<Outgoing> {
        let mut out = Vec::new();
        let inst = &mut *self;
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

    fn gauge_update_msgs(&self, slot: &BattleSlot) -> Vec<Outgoing> {
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
        // The audience funnel, not a second copy of the party filter: this used to
        // re-derive "who is in this battle" inline, which is exactly how a watcher ends
        // up receiving every message EXCEPT the one that moves the gauges — a feed that
        // reads as the fight having frozen.
        let who = self.audience_of(slot);
        broadcast_ser(who.iter().map(String::as_str), wb::GaugeUpdate::TYPE, &msg)
    }

    /// Translate one battle's engine events into wire messages, handling its
    /// terminal outcome. `battle_id` scopes every message + member lookup to that
    /// battle (concurrent battles each get their own event stream).
    fn emit_battle_events(
        &mut self,
        battle_id: &str,
        events: Vec<BattleEvent>,
    ) -> (Vec<Outgoing>, Vec<WorldEffect>) {
        let mut out = Vec::new();
        let mut effects: Vec<WorldEffect> = Vec::new();
        for ev in events {
            match ev {
                BattleEvent::TurnReady { combatant_id } => {
                    let is_player = self
                        .battle_by_id(battle_id)
                        .map(|s| s.combatant_player.contains_key(&combatant_id))
                        .unwrap_or(false);
                    let timeout_at = if is_player {
                        Some(now_ms() + self.balance.battle.turn_timeout_ms)
                    } else {
                        None
                    };
                    let members = self.audience_of_battle(battle_id);
                    out.extend(broadcast(
                        members.iter().map(String::as_str),
                        &wb::TurnReady {
                            battle_id: battle_id.to_string(),
                            combatant_id: combatant_id.clone(),
                            timeout_at,
                        },
                    ));
                }
                BattleEvent::TelegraphStarted {
                    combatant_id,
                    callout_text,
                    executes_at_tick,
                } => {
                    let members = self.audience_of_battle(battle_id);
                    out.extend(broadcast(
                        members.iter().map(String::as_str),
                        &wb::TelegraphStarted {
                            battle_id: battle_id.to_string(),
                            combatant_id,
                            callout_text,
                            executes_at_tick: executes_at_tick as i64,
                        },
                    ));
                }
                BattleEvent::Stolen {
                    victim_player_id,
                    kind,
                } => {
                    self.apply_steal(&victim_player_id, kind);
                }
                // A Shifter picked a creature's pocket. The engine reported the theft;
                // deciding what a creature was carrying is this side's job (economy
                // and loot live here, and the engine stays pure).
                BattleEvent::Pilfered {
                    thief_player_id,
                    victim_combatant_id,
                } => {
                    out.extend(self.apply_pilfer(&thief_player_id, &victim_combatant_id));
                }
                BattleEvent::Resolved(res) => {
                    // An Insight Mote's XP is banked HERE, not in the engine: the
                    // battle has no notion of persistent progression, so it reports an
                    // `insight` status and this side pays it. Without this the mote
                    // was drunk, consumed, and did nothing at all.
                    out.extend(self.bank_insight(battle_id, &res));
                    // A successful flee is the other way past a creature, so the board
                    // counts it rather than hiding it — running is a tactic, not a failure.
                    if res.flee_success == Some(true) {
                        // Fighters, not the audience: watching someone else run away is
                        // not a flee of your own, and the board records what YOU did.
                        let who: Vec<String> = self.fighters_of_battle(battle_id);
                        for r in self.run.runs.iter_mut() {
                            if who.contains(&r.player_id) {
                                r.flees += 1;
                            }
                        }
                    }
                    let members = self.audience_of_battle(battle_id);
                    let msg = wb::ActionResolved {
                        battle_id: battle_id.to_string(),
                        action_id: res.action_id.clone(),
                        actor_id: res.actor_id.clone(),
                        action: res.action,
                        auto: res.auto,
                        flee_success: res.flee_success,
                        callout_text: res.callout_text.clone(),
                        effects: res
                            .effects
                            .iter()
                            .map(|e| wb::Effect {
                                target_id: e.target_id.clone(),
                                kind: e.kind,
                                amount: e.amount,
                                status: e.status.clone(),
                                hp_after: e.hp_after,
                                modifier_flag: e.modifier_flag,
                            })
                            .collect(),
                    };
                    out.extend(broadcast(members.iter().map(String::as_str), &msg));
                }
                BattleEvent::Ended { outcome } => {
                    // DG-3b(3/n): capture dungeon context + members BEFORE the slot is
                    // torn down, so we can fix up dungeon state after (guarded — a
                    // `None` dungeon tag leaves overworld battles byte-identical).
                    let dctx = self.battle_by_id(battle_id).and_then(|s| s.dungeon.clone());
                    // Fighters only: clearing a dungeon is credited to the party that
                    // put the boss down, never to whoever stood at the door watching.
                    let members = if dctx.is_some() {
                        self.fighters_of_battle(battle_id)
                    } else {
                        Vec::new()
                    };
                    let (bout, beff) = self.handle_battle_end(battle_id, outcome);
                    out.extend(bout);
                    effects.extend(beff);
                    if let Some(d) = dctx {
                        if outcome == BattleOutcome::Victory {
                            effects.extend(members.iter().map(|pid| WorldEffect::Hunt {
                                player_id: pid.clone(),
                                fact: HuntFact::DungeonCleared,
                            }));
                            // AD-4: and if the door was keeping someone's MARK, the
                            // contract is finished — for its owner, whoever else swung.
                            if !d.bounty.is_empty() {
                                if let Some(owner) = self
                                    .bounties
                                    .iter()
                                    .find(|(_, specs)| specs.iter().any(|(id, _)| *id == d.bounty))
                                    .map(|(pid, _)| pid.clone())
                                {
                                    effects.push(WorldEffect::BountyFelled {
                                        player_id: owner,
                                        bounty_id: d.bounty.clone(),
                                        mark: d.mark_boss.clone(),
                                    });
                                }
                            }
                        }
                        out.extend(self.finish_dungeon_battle(&members, outcome, d));
                    }
                }
            }
        }
        (out, effects)
    }

    /// Apply a monster's connected `steal` effect to the victim's run (spec §2):
    /// chits lose `steal_chits_fraction`; a consumable/material steal takes one
    /// unit of the first matching backpack stack. Silently a no-op when the
    /// pockets are empty — the shout still happened.
    /// Pay out an Insight Mote: the engine reports an `insight` status on the hero
    /// that drank it, and the XP is banked here, where persistent progression lives.
    ///
    /// The mote is the one consumable whose whole effect is progression, so without
    /// this it was drunk, consumed, and did nothing — `insight_mote_xp` was dead
    /// config and the status was a label with no payout behind it.
    fn bank_insight(&mut self, battle_id: &str, res: &meld_battle::Resolution) -> Vec<Outgoing> {
        let xp = self.balance.consumable.insight_mote_xp;
        if xp <= 0 {
            return Vec::new();
        }
        let drinkers: Vec<String> = res
            .effects
            .iter()
            .filter(|e| e.status.as_deref() == Some("insight"))
            .map(|e| e.target_id.clone())
            .collect();
        if drinkers.is_empty() {
            return Vec::new();
        }
        // Resolve who drank before touching the runs — the battle slot and the runs
        // live on the same actor, so the lookup has to finish first.
        let Some(slot) = self.battle_by_id(battle_id) else {
            return Vec::new();
        };
        let mut owed: Vec<(String, usize, usize)> = Vec::new();
        for cid in drinkers {
            let Some(pid) = slot.combatant_player.get(&cid) else {
                continue;
            };
            let Some(cids) = slot.player_combatants.get(pid) else {
                continue;
            };
            if let Some(hero_slot) = cids.iter().position(|c| *c == cid) {
                owed.push((pid.clone(), hero_slot, cids.len().max(1)));
            }
        }
        let balance = self.balance.clone();
        let mut level_ups: Vec<(String, i32, i32)> = Vec::new();
        let mut cured: Vec<(String, usize)> = Vec::new();
        for (pid, hero_slot, size) in owed {
            if let Some(r) = self.run.runs.iter_mut().find(|r| r.player_id == pid) {
                let old = r.run_level;
                // A mote is drunk by ONE hero and is not an encounter, so it pays out
                // WHOLE: one share, whatever the party size.
                if r.award_hero_xp(hero_slot, 1, size, xp, &balance) > 0 {
                    level_ups.push((pid.clone(), old, r.run_level));
                    cured.push((pid.clone(), hero_slot));
                }
            }
        }
        // Collected rather than cleared in the loop above: that loop holds `self.run`.
        for (pid, hero_slot) in cured {
            self.cure_on_level_up(&pid, hero_slot);
        }
        let mut out = Vec::new();
        for (pid, old, new) in level_ups {
            let heroes = self.hero_level_ups(&pid, old, new);
            out.push(out_msg(
                &pid,
                &wr::LevelUp { new_run_level: new, levels_gained: new - old, heroes },
            ));
            let party = self.party_views(&pid);
            let (synergies, combos) = self.party_depth(&pid);
            let abilities = self.party_ability_views(&pid);
            out.push(out_msg(&pid, &wr::Party { heroes: party, synergies, combos, abilities }));
        }
        out
    }

    /// The Shifter's side of a theft: chits scaled off where the creature was met,
    /// and a chance at whatever it was carrying. The engine reports that a pocket was
    /// picked; what was in it is decided here, next to the rest of the economy.
    fn apply_pilfer(&mut self, thief: &str, victim_combatant: &str) -> Vec<Outgoing> {
        let b = &self.balance;
        // Size the haul off the creature's own tier — a deep theft is worth the trip.
        let dist = self
            .arena
            .monsters
            .iter()
            .find(|m| m.entity_id == victim_combatant)
            .map(|m| m.position.distance_floor())
            .unwrap_or(0);
        let tier = meld_world::Scaling::new(b).tier(dist);
        let chits = b.battle.shifter_steal_chits_per_tier * (tier + 1);
        let roll = roll_unit(self.arena.seed ^ hash_str(thief) ^ hash_str(victim_combatant));
        let take_material = roll < b.battle.shifter_steal_material_chance;
        let material = take_material
            .then(|| meld_world::combat_material_for_biome(dist))
            .map(|m| m.to_string());
        let Some(r) = self.run.run_mut(thief) else {
            return Vec::new();
        };
        r.chits += chits;
        let mut added = Vec::new();
        if let Some(kind) = material {
            let item = ItemStack {
                item_id: Uuid::now_v7().to_string(),
                item_kind: kind,
                quantity: 1,
                insurance: None,
            };
            if r.try_carry(item.clone(), b) {
                added.push(wr::BackpackChange {
                    item,
                    delta: "added".to_string(),
                    cause: "pilfered".to_string(),
                });
            }
        }
        vec![out_msg(
            thief,
            &wr::BackpackUpdate {
                changes: added,
                chits_delta: chits,
                gear_added: Vec::new(),
            },
        )]
    }

    fn apply_steal(&mut self, victim: &str, kind: meld_proto::abilities::StealTargetKind) {
        use meld_proto::abilities::StealTargetKind as K;
        let frac = self.balance.battle.steal_chits_fraction;
        let Some(r) = self.run.run_mut(victim) else {
            return;
        };
        match kind {
            K::Chits => {
                let taken = (((r.chits as f64) * frac).ceil() as i64).clamp(0, r.chits);
                r.chits -= taken;
            }
            K::Consumable | K::Material => {
                // The registry decides what a consumable is, so a new potion is
                // never silently treated as a crafting material.
                let is_consumable = |k: &str| {
                    k == "town_portal" || meld_proto::consumables::is_consumable(k)
                };
                let want_consumable = matches!(kind, K::Consumable);
                // Lift a potion off a HERO before going through the bag: a pouch is what
                // is on their person, and after the bag/pouch split it is where the
                // party's potions actually live — a bag-only steal would mostly miss.
                if want_consumable {
                    for pouch in r.pouches.iter_mut() {
                        if let Some(stack) = pouch
                            .iter_mut()
                            .find(|s| s.quantity > 0 && is_consumable(&s.item_kind))
                        {
                            stack.quantity -= 1;
                            pouch.retain(|s| s.quantity > 0);
                            return;
                        }
                    }
                }
                if let Some(stack) = r
                    .backpack
                    .iter_mut()
                    .find(|s| s.quantity > 0 && is_consumable(&s.item_kind) == want_consumable)
                {
                    stack.quantity -= 1;
                }
                r.backpack.retain(|s| s.quantity > 0);
            }
        }
    }

    /// The players (across every merged party) currently in a given battle.
    /// [`Self::audience_of`] by battle id — the fan-out list for every battle message.
    fn audience_of_battle(&self, battle_id: &str) -> Vec<String> {
        match self.battle_by_id(battle_id) {
            Some(slot) => self.audience_of(slot),
            None => Vec::new(),
        }
    }

    /// [`Self::fighters_of`] by battle id — for the things only a fighter earns.
    fn fighters_of_battle(&self, battle_id: &str) -> Vec<String> {
        match self.battle_by_id(battle_id) {
            Some(slot) => self.fighters_of(slot),
            None => Vec::new(),
        }
    }

    fn handle_battle_end(
        &mut self,
        battle_id: &str,
        outcome: BattleOutcome,
    ) -> (Vec<Outgoing>, Vec<WorldEffect>) {
        let mut out = Vec::new();
        let mut effects: Vec<WorldEffect> = Vec::new();
        let mut leveled: Vec<String> = Vec::new();
        // (player_id, old_run_level, new_run_level) for anyone who leveled up this
        // victory — drives the classic per-hero stat-gain screen.
        let mut level_ups: Vec<(String, i32, i32)> = Vec::new();
        let balance = self.balance.clone();
        // `self` IS the world now; reborrow it for the world-state block below so
        // the Router-scoped tail (effects, level-up party refresh) can use `self`
        // once this borrow ends (as `collect_ground_loot` does).
        let inst = &mut *self;
        let Some(bidx) = inst.battles.iter().position(|b| b.battle_id == battle_id) else {
            return (out, effects);
        };
        let monster_ids = inst.battles[bidx].monster_ids.clone();
        let battle_pos = inst.battles[bidx].pos;
        // Combined XP for the whole encounter (touched creature + its group), paid
        // against the health it was actually built with: `BattleSlot::party_scale`
        // is the same multiplier its HP wears, so a fight that took four times the
        // chewing pays four times the lesson before the party splits it.
        let base_xp: i64 = monster_ids
            .iter()
            .filter_map(|id| inst.arena.monster_by_id(id))
            .map(|m| m.xp_reward)
            .sum();
        let xp_reward: i64 =
            ((base_xp as f64) * inst.battles[bidx].party_scale).round().max(0.0) as i64;
        // The toughest thing in the encounter is what a hero learns from, and what
        // `xp_after_level_gap` weighs its own level against.
        let encounter_level: i32 = monster_ids
            .iter()
            .filter_map(|id| inst.arena.monster_by_id(id))
            .map(|m| m.level)
            .max()
            .unwrap_or(1);
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
        //
        // `falls` is collected here and queued after the loop: the DB write needs
        // `self.db_writes` while `inst` still holds the instance borrow.
        let mut falls: Vec<(String, i32, u32)> = Vec::new();
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
                // …and what is still gripping each hero, for the same reason: no free
                // cleanse between fights any more than a free heal.
                let carried: Vec<Vec<String>> = cids
                    .iter()
                    .map(|cid| inst.battles[bidx].battle.combatant_afflictions(cid))
                    .collect();
                inst.hero_afflictions.insert(pid.clone(), carried);
                // Every hero that FELL in this fight owes the durability tax on its own
                // kit (GR-2). Counted by the engine per fall rather than read off the
                // end state, so a hero raised and killed again pays twice and a hero
                // who was already down pays nothing. Charged whatever the outcome:
                // falling is what costs you, not losing.
                falls.extend(cids.iter().enumerate().filter_map(|(slot, cid)| {
                    let n = inst.battles[bidx].battle.combatant_falls(cid);
                    (n > 0).then(|| (pid.clone(), slot as i32, n))
                }));
            }
        }

        // The same falls, shaped for the player rather than for the DB: what this fight
        // cost each of THEIR heroes, ready to ride out on `battle.ended`. Built here so
        // every outcome arm reports it — a hero that fell in a fight you won, fled from
        // or lost went down all the same.
        let per_fall = inst.balance.loot.durability_loss_per_fall;
        let mut worn: HashMap<String, Vec<wb::GearWorn>> = HashMap::new();
        for (pid, slot, n) in &falls {
            let hero_name = inst
                .hero_names
                .get(pid)
                .and_then(|names| names.get(*slot as usize))
                .cloned()
                .unwrap_or_else(|| generated_hero_name(pid, *slot as usize));
            worn.entry(pid.clone()).or_default().push(wb::GearWorn {
                hero_slot: *slot,
                hero_name,
                falls: *n,
                durability_lost: per_fall.saturating_mul(*n as i32),
            });
        }

        let mut dead: Vec<String> = Vec::new();
        // Filled by the victory arm; drained after `inst`'s borrow ends, because ending the
        // run needs `&mut self` and the arm still holds the instance.
        let mut end_fight_winners: Vec<String> = Vec::new();

        match outcome {
            BattleOutcome::Victory => {
                // Was this THE END FIGHT? Asked before the corpses are cleared, because
                // the encounter class lives on the creature and is gone a line later.
                let was_end_fight = monster_ids.iter().any(|id| {
                    inst.arena
                        .monster_by_id(id)
                        .is_some_and(|m| m.encounter_class == "world_end")
                });
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
                // Snapshot what the per-hero award needs before the runs are borrowed
                // mutably: who is still standing, and which class each slot is.
                let hero_hp_snapshot = inst.hero_hp.clone();
                let party_classes_snapshot = inst.party_classes.clone();
                // XP goes to each hero that is still STANDING, at its own ladder: a
                // hero that fell earns nothing from the fight it did not finish, and
                // the hero doing the killing is the one that gets stronger.
                let mut class_bests: Vec<(String, String, i32)> = Vec::new();
                // Who advanced, so their conditions can be lifted once the run borrow ends.
                let mut cured: Vec<(String, usize)> = Vec::new();
                let run_level_before: Vec<(String, i32)> = inst
                    .run
                    .runs
                    .iter()
                    .filter(|r| bp.contains(&r.party_id))
                    .map(|r| (r.player_id.clone(), r.run_level))
                    .collect();
                for r in inst.run.runs.iter_mut().filter(|r| bp.contains(&r.party_id)) {
                    let hps = hero_hp_snapshot.get(&r.player_id).cloned().unwrap_or_default();
                    let comp = party_classes_snapshot
                        .get(&r.player_id)
                        .cloned()
                        .unwrap_or_default();
                    let size = comp.len().max(hps.len());
                    // The encounter is a POOL divided among whoever is still STANDING
                    // when it ends. Three heroes down means the survivor banks the whole
                    // thing — a fight that nearly killed you should be worth what it
                    // cost. Dividing by the full party instead simply evaporated the
                    // fallen heroes' shares.
                    let standing = hps.iter().filter(|hp| **hp > 0).count().max(1);
                    for (slot, hp) in hps.iter().enumerate() {
                        if *hp <= 0 {
                            continue;
                        }
                        // Each hero weighs the encounter against ITS OWN level, so the
                        // one that has fallen behind still learns from ground the rest
                        // of the party has outgrown.
                        let paid = meld_run::xp_after_level_gap(
                            xp_reward,
                            encounter_level,
                            r.hero_level(slot),
                            &balance,
                        );
                        if r.award_hero_xp(slot, standing, size, paid, &balance) > 0 {
                            cured.push((r.player_id.clone(), slot));
                            if let Some(class) = comp.get(slot) {
                                class_bests.push((
                                    r.player_id.clone(),
                                    meld_run::class_key(*class).to_string(),
                                    r.hero_level(slot),
                                ));
                            }
                        }
                    }
                }
                for (pid, class, level) in class_bests {
                    let _ = inst.db_writes.send(DbWrite::ClassBest(pid, class, level));
                }
                // Going up a level CURES: the afflictions harvested off this battle a moment
                // ago are lifted from whichever heroes advanced on it, so a poison caught in
                // the fight you levelled on does not follow you down the road.
                for (pid, slot) in cured {
                    inst.cure_on_level_up(&pid, slot);
                }
                // CL-1 milestones from a won fight. Read off the creatures BEFORE
                // they leave the arena: what was in the encounter is what earns the
                // class, and a moment later there is nothing left to ask.
                let (mut felled_champion, mut felled_rite) = (false, false);
                // AD-4 reads the same carcasses: what a hunt counts is the creature's
                // OWN kind and class, taken before the encounter leaves the arena.
                let mut felled: Vec<(String, String)> = Vec::new();
                for id in &monster_ids {
                    match inst.arena.monster_by_id(id).map(|m| m.encounter_class.as_str()) {
                        Some("elite") | Some("gatekeeper") => felled_champion = true,
                        Some("undead_rite") => felled_rite = true,
                        _ => {}
                    }
                    if let Some(m) = inst.arena.monster_by_id(id) {
                        felled.push((m.monster_kind.clone(), m.encounter_class.clone()));
                        // AD-4: a felled MARK finishes its contract, and it finishes it
                        // for its owner only — whoever else was swinging, the contract has
                        // one name on it.
                        if !m.bounty.is_empty() && !m.owner.is_empty() {
                            effects.push(WorldEffect::BountyFelled {
                                player_id: m.owner.clone(),
                                bounty_id: m.bounty.clone(),
                                mark: m.boss_kind.clone(),
                            });
                        }
                    }
                }
                for r in inst.run.runs.iter().filter(|r| bp.contains(&r.party_id)) {
                    for (creature, class) in &felled {
                        effects.push(WorldEffect::Hunt {
                            player_id: r.player_id.clone(),
                            fact: HuntFact::Felled {
                                creature: creature.clone(),
                                class: class.clone(),
                            },
                        });
                    }
                    if felled_champion {
                        effects.push(WorldEffect::Milestone {
                            player_id: r.player_id.clone(),
                            milestone: meld_proto::unlocks::Milestone::EliteFelled,
                        });
                    }
                    if felled_rite {
                        effects.push(WorldEffect::Milestone {
                            player_id: r.player_id.clone(),
                            milestone: meld_proto::unlocks::Milestone::SurvivedUndeadRite,
                        });
                    }
                    // The party-slot bars: report the DEEPEST bar this party clears
                    // (most heroes simultaneously at a level), so one milestone can
                    // satisfy every rule it qualifies for.
                    for (heroes, level) in party_slot_bars(r) {
                        effects.push(WorldEffect::Milestone {
                            player_id: r.player_id.clone(),
                            milestone: meld_proto::unlocks::Milestone::HeroesReached {
                                heroes,
                                level,
                            },
                        });
                    }
                }
                // The headline level is `max(hero_levels)`, maintained by `award_hero_xp`
                // above — so a level-up is "did that push the best hero up", not a second
                // award. There used to be a `r.award_xp(xp_reward)` here as well: the run
                // kept its OWN xp pool and its own ladder, fed the FULL encounter XP while
                // each hero got the split share. So the banner announced level 3 off the run
                // pool while the party screen still read level 2 off the hero, and every
                // victory paid twice.
                for (pid, before) in run_level_before.iter() {
                    let Some(r) =
                        inst.run.runs.iter().find(|r| &r.player_id == pid && bp.contains(&r.party_id))
                    else {
                        continue;
                    };
                    if r.run_level <= *before {
                        continue;
                    }
                    let now = r.run_level;
                    leveled.push(pid.clone());
                    level_ups.push((pid.clone(), *before, now));
                    // A level-up tops up the LIVING and raises nobody: the dead
                    // come back on a Waking Salt, not on someone else's good
                    // fortune. (`hero_hp` at 0 is a fallen hero.)
                    if let (Some(classes), Some(hps)) =
                        (inst.party_classes.get(pid).cloned(), inst.hero_hp.get_mut(pid))
                    {
                        for (slot, (class, hp)) in classes.iter().zip(hps.iter_mut()).enumerate() {
                            if *hp > 0 {
                                // Each hero to ITS own level, not the party's best.
                                let lvl = inst
                                    .run
                                    .runs
                                    .iter()
                                    .find(|r| &r.player_id == pid)
                                    .map(|r| r.hero_level(slot))
                                    .unwrap_or(now);
                                *hp = meld_run::max_hp_at_level(*class, lvl, &balance);
                            }
                        }
                    }
                }
                for pid in &members {
                    if let Some(a) = inst.arena.avatar_mut(pid) {
                        a.state = "active".to_string();
                    }
                }
                // A nearby second monster shouldn't be able to pull the party straight
                // into another fight while the victory/loot summary is still on screen.
                let reentry_until = now_ms() + balance.world.battle_reentry_grace_ms;
                for pid in &members {
                    inst.battle_immune_until.insert(pid.clone(), reentry_until);
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
                        // The apex has to be the best SOURCE in the game, not just a
                        // guaranteed floor: `rolled_gear` (what the three insured pieces
                        // come from) deliberately cannot produce a unique or a set piece,
                        // so without a spike here the end fight was a worse source than a
                        // Gatekeeper standing in a pass at d300.
                        "world_end" => balance.encounters.end_fight_loot_mult,
                        "gatekeeper" => balance.encounters.gatekeeper_loot_mult,
                        "undead_rite" => balance.encounters.undead_rite_loot_mult,
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
                        quantity: loot.material_qty,
                        insurance: None,
                    };
                    let potion_item = (!loot.potion.is_empty()).then(|| ItemStack {
                        item_id: Uuid::now_v7().to_string(),
                        item_kind: loot.potion.to_string(),
                        quantity: 1,
                        insurance: None,
                    });
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
                            class_key: g.class_key.clone(),
                            // The tier is decided by the ROLL, not the drop site.
                insurance: g.insurance,
                            tier: g.tier,
                            atk_bonus: g.atk_bonus,
                            def_bonus: g.def_bonus,
                            spd_bonus: g.spd_bonus,
                            base_max_durability: g.max_durability,
                            max_durability: g.max_durability,
                            equipped_hero_slot: None,
                            damage_modifiers: g.damage_modifiers.clone(),
                            family: g.family.clone(),
                            armor_weight: g.armor_weight.clone(),
                            affixes: g.affixes.clone(),
                            unique_key: g.unique_key.clone(),
                            set_key: g.set_key.clone(),
                        })
                        .collect();
                    // Record loot in the run so extraction can bank it.
                    let mut run_gear_snapshot = None;
                    if let Some(r) = inst.run.runs.iter_mut().find(|r| &r.player_id == pid) {
                        r.backpack.push(loot_item.clone());
                        if let Some(p) = &potion_item {
                            r.backpack.push(p.clone());
                        }
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
                        loot: [Some(loot_item.clone()), potion_item.clone()]
                            .into_iter()
                            .flatten()
                            .collect(),
                        chits_found: loot.chits,
                        gear_drops: gear_drops.clone(),
                        class_emblem_drops: vec![],
                        gatekeeper_cleared: false,
                        gear_worn: worn.get(pid).cloned().unwrap_or_default(),
                    };
                    out.push(out_msg(pid, &ended));
                    out.push(out_msg(
                        pid,
                        &wr::BackpackUpdate {
                            changes: [Some((loot_item, "battle_loot")), potion_item.map(|p| (p, "potion_drop"))]
                                .into_iter()
                                .flatten()
                                .map(|(item, cause)| wr::BackpackChange {
                                    item,
                                    delta: "added".to_string(),
                                    cause: cause.to_string(),
                                })
                                .collect(),
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
                    // The world also sprinkles the two progression consumables: an
                    // Insight Mote (XP you choose who to spend on) and a Waking Salt
                    // (the only way a fallen hero stands back up, now that a level-up
                    // raises nobody). Separate rolls off separate seeds so one lucky
                    // number cannot hand out both.
                    for (kind, chance, salt) in [
                        ("insight_mote", balance.consumable.world_xp_item_chance, 0xA11u64),
                        ("waking_salt", balance.consumable.world_revive_item_chance, 0xB22u64),
                    ] {
                        let roll = roll_unit(inst.arena.seed ^ hash_str(pid) ^ now_ms() ^ salt);
                        if roll >= chance {
                            continue;
                        }
                        let item = ItemStack {
                            item_id: Uuid::now_v7().to_string(),
                            item_kind: kind.to_string(),
                            quantity: 1,
                            insurance: None,
                        };
                        if let Some(r) = inst.run.runs.iter_mut().find(|r| &r.player_id == pid) {
                            r.backpack.push(item.clone());
                        }
                        out.push(out_msg(
                            pid,
                            &wr::BackpackUpdate {
                                changes: vec![wr::BackpackChange {
                                    item,
                                    delta: "added".to_string(),
                                    cause: format!("{kind}_drop"),
                                }],
                                chits_delta: 0,
                                gear_added: Vec::new(),
                            },
                        ));
                    }
                }
                // THE END FIGHT ends the dive — and it does so LAST, after the XP, the
                // class records, the hunt credit and the ordinary drops have all landed.
                // Returning early here skipped every one of them, so felling the top of
                // the game paid less than felling a boar.
                //
                // Ending the run is the point: this is a roguelite, so the reward is
                // banked and the party comes home rather than carrying on deeper.
                if was_end_fight {
                    end_fight_winners = inst
                        .run
                        .runs
                        .iter()
                        .filter(|r| bp.contains(&r.party_id))
                        .map(|r| r.player_id.clone())
                        .collect();
                }
            }
            BattleOutcome::Defeat => {
                // CL-1: how MANY heroes were lost decides what the wipe teaches — the
                // Resonant on any wipe (a lone hero's death is the fight that explains
                // why you want a healer), the Psyker only on a real party's. Counted
                // off `party_classes`, the composition that actually went in, and
                // floored at one: a wipe means at least one hero fell, so a missing
                // composition must not silently swallow the grant.
                for pid in &members {
                    let heroes = inst
                        .party_classes
                        .get(pid)
                        .map(|c| c.len() as i32)
                        .unwrap_or(1)
                        .max(1);
                    effects.push(WorldEffect::Milestone {
                        player_id: pid.clone(),
                        milestone: meld_proto::unlocks::Milestone::PartyWiped { heroes },
                    });
                }
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
                            gear_worn: worn.get(pid).cloned().unwrap_or_default(),
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
                        let mut lost = r.backpack.clone();
                        lost.extend(r.pouches.iter().flatten().cloned());
                        (r.player_id.clone(), r.run_id.clone(), lost, r.chits)
                    })
                    .collect();
                for r in inst.run.runs.iter_mut().filter(|r| bp.contains(&r.party_id)) {
                    r.result = Some(RunResult::Died);
                    r.backpack.clear();
                    for pouch in r.pouches.iter_mut() {
                        pouch.clear();
                    }
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
                            durability_loss_applied: inst.durability_charged.contains(pid),
                        },
                    ));
                }
                dead = members.clone();
            }
            BattleOutcome::Fled => {
                // Fleeing saves your heroes but not your whole haul (combat-atb.md).
                // Unlike Defeat the run CONTINUES — you're back in the overworld — but
                // you bolt and spill some of what you were carrying: forfeit a fraction
                // of your un-banked chits, and roll each non-permanent item (backpack
                // material + red-chest looted gear) to drop. Insured (blue) equipped
                // gear is owned, not in the backpack, so it's never at risk. This is the
                // cost that makes fleeing a real decision rather than a free escape.
                let frac = balance.battle.flee_chit_loss_fraction.clamp(0.0, 1.0);
                let drop_chance = balance.battle.flee_item_drop_chance.clamp(0.0, 1.0);
                let seed = inst.arena.seed ^ now_ms();
                // Everyone who was in the fight is roaming again (they didn't die) —
                // and BOLTS clear of the creature. Setting the state back to `active`
                // without moving them left them standing on top of it, so the next
                // tick's `resolve_touches` pulled them straight back into the fight
                // they had just paid to escape. Fleeing was a death sentence.
                let flee_dist = balance.world.touch_radius_tiles * 4.0;
                for pid in &members {
                    if let Some(a) = inst.arena.avatar_mut(pid) {
                        a.state = "active".to_string();
                        let (dx, dy) = (a.position.x - battle_pos.x, a.position.y - battle_pos.y);
                        let len = (dx * dx + dy * dy).sqrt();
                        // Standing exactly on it means no direction to flee in; fall
                        // back to west, which is the way home.
                        let (ux, uy) = if len > 1e-6 { (dx / len, dy / len) } else { (-1.0, 0.0) };
                        a.position.x = battle_pos.x + ux * flee_dist;
                        a.position.y = battle_pos.y + uy * flee_dist;
                    }
                }
                // The teleport alone wasn't enough — an aggressive creature's chase
                // speed can close a 4-tile gap in under a second, so the fight restarted
                // before the player had reacted. A real grace window on top of the
                // teleport gives fleeing an actual chance to work.
                let reentry_until = now_ms() + balance.world.battle_reentry_grace_ms;
                for pid in &members {
                    inst.battle_immune_until.insert(pid.clone(), reentry_until);
                }
                let mut losses: Vec<FleeLoss> = Vec::new();
                for r in inst.run.runs.iter_mut().filter(|r| bp.contains(&r.party_id)) {
                    let base = seed ^ hash_str(&r.player_id);
                    let lost_chits = ((r.chits.max(0) as f64) * frac).floor() as i64;
                    r.chits -= lost_chits;
                    // Roll each stack independently (keyed by its stable id, so the
                    // outcome is deterministic and reproducible) — across the Party
                    // Inventory AND every pouch. A pouch that were exempt would make
                    // "stuff the potions onto the heroes" a way to flee for free, and a
                    // pouch is carried on the run like anything else.
                    let spill = |items: &mut Vec<ItemStack>| {
                        let mut out: Vec<ItemStack> = Vec::new();
                        items.retain(|it| {
                            if roll_unit(base ^ hash_str(&it.item_id)) < drop_chance {
                                out.push(it.clone());
                                false
                            } else {
                                true
                            }
                        });
                        out
                    };
                    let dropped_bag = spill(&mut r.backpack);
                    let mut dropped = dropped_bag.clone();
                    let mut pouches_changed = false;
                    for pouch in r.pouches.iter_mut() {
                        let lost = spill(pouch);
                        pouches_changed |= !lost.is_empty();
                        dropped.extend(lost);
                    }
                    // And each piece of not-yet-banked red-chest gear.
                    let before_gear = r.looted_gear.len();
                    r.looted_gear
                        .retain(|g| roll_unit(base ^ hash_str(&g.gear_id)) >= drop_chance);
                    let gear_changed = r.looted_gear.len() != before_gear;
                    losses.push(FleeLoss {
                        pid: r.player_id.clone(),
                        dropped,
                        dropped_bag,
                        pouches_changed,
                        lost_chits,
                        gear: r.looted_gear.clone(),
                        gear_changed,
                    });
                }
                for FleeLoss {
                    pid,
                    dropped,
                    dropped_bag,
                    pouches_changed,
                    lost_chits,
                    gear,
                    gear_changed,
                } in losses
                {
                    // `battle.ended`/Fled takes the client out of the battle screen and
                    // back to the overworld (see the client's `BattleEnded` handler).
                    // For the Fled outcome these fields report what was DROPPED (not
                    // gained), so the client can show a "fled — dropped N" line.
                    out.push(out_msg(
                        &pid,
                        &wb::Ended {
                            battle_id: battle_id.to_string(),
                            outcome: BattleOutcome::Fled,
                            xp_awards: vec![],
                            loot: dropped.clone(),
                            chits_found: lost_chits,
                            gear_drops: vec![],
                            class_emblem_drops: vec![],
                            gatekeeper_cleared: false,
                            gear_worn: worn.get(&pid).cloned().unwrap_or_default(),
                        },
                    ));
                    // Authoritatively mutate the client's mirrored backpack: the same
                    // message shape every other backpack change uses (the client just
                    // applies the removals + negative chit delta).
                    if !dropped_bag.is_empty() || lost_chits > 0 {
                        let changes = dropped_bag
                            .iter()
                            .map(|it| wr::BackpackChange {
                                item: it.clone(),
                                delta: "removed".to_string(),
                                cause: "fled".to_string(),
                            })
                            .collect();
                        out.push(out_msg(
                            &pid,
                            &wr::BackpackUpdate {
                                changes,
                                chits_delta: -lost_chits,
                                gear_added: vec![],
                            },
                        ));
                    }
                    if pouches_changed {
                        out.extend(inst.pouches_msg(&pid));
                    }
                    // Correct the run-loot (equip tab) with a fresh full snapshot when
                    // any red-chest gear was dropped.
                    if gear_changed {
                        out.push(out_msg(&pid, &wr::RunGear { gear }));
                    }
                }
            }
        }
        // Battle over: any surviving grouped creatures (e.g. after a flee) resume
        // roaming, then drop the battle slot entirely (its combatant bookkeeping
        // goes with it). Other concurrent battles are untouched.
        //
        // CR-2: they resume roaming WOUNDED. Every point the party landed used to be
        // forgotten the moment the slot dropped, so fleeing a fight you had nearly won
        // reset the creature to full and the whole encounter had to be paid for again —
        // and a party could never soften something up and come back.
        //
        // Written back as a FRACTION, never as the raw battle number: the fight scaled the
        // creature's pool by `party_scale`, so a four-hero party chewed through ~4.4x the
        // health this spawn actually has. Writing 3000-of-13200 onto a 3000 HP creature
        // would leave it untouched; writing the raw remainder onto it would kill it.
        let wounds: Vec<(String, f64)> = {
            let slot = inst.battles.get(bidx);
            monster_ids
                .iter()
                .filter_map(|id| {
                    let slot = slot?;
                    let cid = slot.monster_combatants.get(id)?;
                    let (hp, max) = slot.battle.combatant_health(cid)?;
                    (max > 0).then(|| (id.clone(), (hp.max(0) as f64) / (max as f64)))
                })
                .collect()
        };
        for (id, left) in wounds {
            if let Some(m) = inst.arena.monster_by_id_mut(&id) {
                if !m.defeated {
                    // At least 1: a creature the engine says is alive must not be written
                    // back dead by rounding, or it would be a corpse nothing killed.
                    m.hp = (((m.max_hp as f64) * left).round() as i32).clamp(1, m.max_hp);
                }
            }
        }
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
            // Drop the dead player's avatar + run from the world NOW — inline, this
            // tick, before the later overworld snapshot — so they don't linger a frame
            // (matching the pre-SC-3 behaviour where release ran inline here). The rest
            // of the teardown (session `in_instance` flip, per-player bookkeeping, and
            // dropping the world if it's now empty) can't be done from world logic, so
            // it rides the effect below; its `remove_from_instance` re-runs these two
            // retains idempotently.
            self.arena.avatars.retain(|a| a.player_id != pid);
            self.run.runs.retain(|r| r.player_id != pid);
            effects.push(WorldEffect::ReleaseFromRun(pid));
        }
        // Charge the durability tax for every hero that fell in this fight (GR-2). Queued
        // here rather than inside the loop that counted them, because the instance borrow
        // is still live up there. It is deliberately charged on a VICTORY too: a hero that
        // went down and was carried through the rest of the fight still went down, and a
        // tax only the losing run pays is a tax a careful player never meets.
        for (pid, slot, n) in falls {
            let _ = self.db_writes.send(DbWrite::HeroFalls(pid.clone(), slot, n));
            self.durability_charged.insert(pid);
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
        // Refresh the roster for EVERYONE who fought, not just whoever levelled. The
        // roster is what carries `xp`/`xp_to_next`, so sending it only on a level-up
        // left the party screen reading 0 XP for every fight in between — the progress
        // was real and completely invisible, which reads as "my heroes get no XP".
        for pid in &members {
            let heroes = self.party_views(pid);
            let (synergies, combos) = self.party_depth(pid);
            let abilities = self.party_ability_views(pid);
            out.push(out_msg(pid, &wr::Party { heroes, synergies, combos, abilities }));
        }
        // Perk tiers scale with run level, so they only change on a level-up.
        for pid in &leveled {
            out.push(out_msg(pid, &self.perks_for(pid)));
        }
        // THE END FIGHT ends the dive, and does it LAST — every reward above has landed by
        // now. This is the roguelite bargain: the top of the game pays out and sends you
        // home rather than letting you carry on deeper.
        for pid in &end_fight_winners {
            out.extend(self.finish_end_fight(pid));
        }
        (out, effects)
    }
}

/// The name a hero slot falls back to when nothing has been stored for it — the same
/// generated name registration would have seeded, so a slot never surfaces as
/// "Hero 3" just because its row is missing.
fn generated_hero_name(player_id: &str, slot: usize) -> String {
    meld_proto::names::hero_name(meld_proto::names::seed_of(player_id), slot).to_string()
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

#[cfg(test)]
mod interest_grid_tests {
    use super::*;

    fn ent(id: &str, x: f64, y: f64, state: Option<&str>) -> wm::SnapshotEntity {
        wm::SnapshotEntity {
            entity_id: id.to_string(),
            position: Position { x, y },
            avatar_state: state.map(str::to_string),
            ..Default::default()
        }
    }

    /// The original full linear scan the grid replaces — the equivalence oracle.
    fn naive_visible(
        entities: &[wm::SnapshotEntity],
        px: f64,
        py: f64,
        radius2: f64,
        mob_radius2: f64,
        own_id: &str,
    ) -> Vec<usize> {
        let mut v: Vec<usize> = entities
            .iter()
            .enumerate()
            .filter(|(_, e)| {
                e.entity_id == own_id
                    || e.entity_id == "portal"
                    || {
                        let (dx, dy) = (e.position.x - px, e.position.y - py);
                        let d2 = dx * dx + dy * dy;
                        let is_mob = e
                            .avatar_state
                            .as_deref()
                            .is_some_and(|s| s.starts_with("mob:"));
                        d2 <= if is_mob { mob_radius2 } else { radius2 }
                    }
            })
            .map(|(i, _)| i)
            .collect();
        v.sort_unstable();
        v.dedup();
        v
    }

    // The grid-indexed cull must return exactly what the old whole-list scan did,
    // for arbitrary worlds and (base, mob-reveal) radii — that equivalence is the
    // whole safety guarantee of SC-1.
    #[test]
    fn grid_cull_matches_naive_over_random_worlds() {
        let cell = 64.0;
        let radius = 128.0;
        let radius2 = radius * radius;
        // Deterministic xorshift — reproducible, and the engine bans nondeterminism.
        let mut s: u64 = 0x9E37_79B9_7F4A_7C15;
        let mut next = || {
            s ^= s << 13;
            s ^= s >> 7;
            s ^= s << 17;
            s
        };
        let coord = |r: u64| ((r % 16000) as f64) / 10.0 - 800.0; // [-800, 800)

        for trial in 0..300 {
            let mut entities = vec![ent("me", coord(next()), coord(next()), Some("idle"))];
            let (ax, ay) = (entities[0].position.x, entities[0].position.y);
            let mut placed_portal = false;
            let n = 4 + (next() % 70) as usize;
            for k in 0..n {
                let (x, y) = (coord(next()), coord(next()));
                let (id, state) = match next() % 4 {
                    0 => (format!("mob{k}"), Some("mob:wolf:hostile".to_string())),
                    1 => (format!("res{k}"), Some("resource:herb".to_string())),
                    2 if !placed_portal => {
                        placed_portal = true;
                        ("portal".to_string(), Some("portal".to_string()))
                    }
                    _ => (format!("obs{k}"), Some("obstacle:tree:1.00".to_string())),
                };
                entities.push(wm::SnapshotEntity {
                    entity_id: id,
                    position: Position { x, y },
                    avatar_state: state,
                    ..Default::default()
                });
            }
            // Psyker reveal radius is always ≥ base (as the server enforces).
            let mob_radius = radius + (next() % 500) as f64;
            let mob_radius2 = mob_radius * mob_radius;

            let grid = build_interest_grid(&entities, cell);
            let portal_idx = entities.iter().position(|e| e.entity_id == "portal");

            let got = interest_visible_indices(
                &entities, &grid, cell, ax, ay, radius2, mob_radius, mob_radius2, Some(0),
                portal_idx,
            );
            let want = naive_visible(&entities, ax, ay, radius2, mob_radius2, "me");
            assert_eq!(got, want, "trial {trial}: grid cull diverged from the naive scan");
        }
    }

    #[test]
    fn own_avatar_and_portal_included_even_when_out_of_range() {
        let cell = 64.0;
        let radius2 = 100.0; // radius 10
        let (mob_radius, mob_radius2) = (10.0, 100.0);
        let entities = vec![
            ent("me", 0.0, 0.0, Some("idle")),
            ent("portal", 5000.0, -5000.0, Some("portal")),
            ent("far_mob", 9000.0, 0.0, Some("mob:x:y")),
            ent("near_res", 3.0, 4.0, Some("resource:herb")), // d = 5 ≤ 10
        ];
        let grid = build_interest_grid(&entities, cell);
        let portal_idx = entities.iter().position(|e| e.entity_id == "portal");
        let got = interest_visible_indices(
            &entities, &grid, cell, 0.0, 0.0, radius2, mob_radius, mob_radius2, Some(0), portal_idx,
        );
        assert!(got.contains(&0), "own avatar always included");
        assert!(got.contains(&1), "portal always included even when far");
        assert!(!got.contains(&2), "far mob excluded");
        assert!(got.contains(&3), "near resource included");
    }
}

#[cfg(test)]
mod unlock_gate_tests {
    use super::*;

    fn owned(keys: &[&str]) -> Vec<String> {
        keys.iter().map(|k| k.to_string()).collect()
    }

    /// A run whose heroes sit at the given levels — the only thing the slot bars
    /// read.
    pub(super) fn run_at(levels: &[i32]) -> meld_run::PlayerRun {
        meld_run::PlayerRun {
            run_id: "r".into(),
            player_id: "p".into(),
            username: "u".into(),
            character_class: CharacterClass::Explorer,
            run_level: *levels.iter().max().unwrap_or(&1),
            xp: 0,
            backpack: vec![],
            pouches: vec![],
            chits: 0,
            looted_gear: vec![],
            max_distance_reached: 0,
                fights: 0,
                flees: 0,
            result: None,
            party_id: 0,
            hero_levels: levels.to_vec(),
            hero_xp: vec![0; levels.len()],
        }
    }

    #[test]
    fn a_party_is_only_as_big_as_the_slots_the_account_earned() {
        // The bug this pins: `clamp_party_to_unlocks` correctly cut a one-slot
        // account's party down to a single Explorer, and `form_run` then padded it
        // straight back up to `party_size_per_player` — so a new player was handed
        // FOUR copies of the only class they owned. The cap is a ceiling, not a
        // grant. This is the arithmetic `form_run` now does per player.
        let sized = |owned_keys: &[&str], cap: usize| -> usize {
            let o = owned(owned_keys);
            (meld_proto::unlocks::party_slots(&o) as usize).clamp(1, cap)
        };
        assert_eq!(sized(&["class_explorer"], 4), 1, "a fresh account fields ONE hero");
        assert_eq!(sized(&["class_explorer", "party_slot_2"], 4), 2);
        assert_eq!(sized(&["class_explorer", "party_slot_2", "party_slot_3"], 4), 3);
        assert_eq!(
            sized(&["class_explorer", "party_slot_2", "party_slot_3", "party_slot_4"], 4),
            4
        );
        // The balance cap still wins when it is the smaller of the two, so lowering
        // `party_size_per_player` for a test or a mode is still honoured.
        assert_eq!(
            sized(&["class_explorer", "party_slot_2", "party_slot_3", "party_slot_4"], 2),
            2
        );
    }

    #[test]
    fn a_party_is_clamped_to_what_the_account_owns() {
        use CharacterClass::*;
        let wanted = vec![Explorer, Psyker, Resonant, Shifter];

        // A brand-new account fields exactly one Explorer, whatever the client asked
        // for. Clamped rather than rejected: a stale saved party still gets a dive.
        let fresh = owned(&["class_explorer"]);
        assert_eq!(clamp_party_to_unlocks(wanted.clone(), &fresh), vec![Explorer]);

        // A second slot seats a second hero — but an unowned class in it becomes an
        // Explorer, not an error.
        let two = owned(&["class_explorer", "party_slot_2"]);
        assert_eq!(clamp_party_to_unlocks(wanted.clone(), &two), vec![Explorer, Explorer]);

        // Earn the Resonant and it can take the seat; the Psyker still can't.
        let two_res = owned(&["class_explorer", "party_slot_2", "class_resonant"]);
        assert_eq!(
            clamp_party_to_unlocks(vec![Resonant, Psyker], &two_res),
            vec![Resonant, Explorer]
        );

        // Everything owned: the party comes through untouched.
        let all: Vec<String> =
            meld_proto::unlocks::UNLOCKS.iter().map(|u| u.key.to_string()).collect();
        assert_eq!(clamp_party_to_unlocks(wanted.clone(), &all), wanted);
    }

    #[test]
    fn the_party_slot_bars_count_heroes_standing_together() {
        // Nobody at a bar yet: no milestone to report, rather than a zero-count one
        // that would grant slot 2 to a level-1 party.
        assert!(party_slot_bars(&run_at(&[1, 1, 1, 1])).is_empty());

        let run = run_at(&[10, 3, 1, 1]);
        let bars = party_slot_bars(&run);
        assert!(bars.contains(&(1, 10)), "{bars:?}");
        assert!(!bars.iter().any(|(_, l)| *l == 20), "{bars:?}");

        // Two at 20 clears the level-10 bar with two heroes AND the 20 bar with two.
        let run = run_at(&[22, 20, 9, 1]);
        let bars = party_slot_bars(&run);
        assert!(bars.contains(&(2, 20)), "{bars:?}");
        assert!(bars.contains(&(2, 10)), "{bars:?}");

        // Three at 30 is the deepest bar, and it clears every shallower one.
        let run = run_at(&[30, 31, 30, 4]);
        let bars = party_slot_bars(&run);
        for want in [(3, 30), (3, 20), (3, 10)] {
            assert!(bars.contains(&want), "missing {want:?} in {bars:?}");
        }
    }

    #[test]
    fn the_bars_a_party_clears_grant_exactly_the_slots_they_should() {
        // The registry and the reporting have to agree end-to-end: feed each bar
        // through `granted_by` the way the loop does and check the slot it opens.
        let mut have = owned(&["class_explorer"]);

        let run = run_at(&[10, 1, 1, 1]);
        let mut granted: Vec<&str> = Vec::new();
        for (heroes, level) in party_slot_bars(&run) {
            let m = meld_proto::unlocks::Milestone::HeroesReached { heroes, level };
            granted.extend(meld_proto::unlocks::granted_by(m, &have).iter().map(|u| u.key));
        }
        assert_eq!(granted, vec!["party_slot_2"]);
        have.push("party_slot_2".to_string());

        // One hero at 40 is still only ONE hero: no third slot.
        let run = run_at(&[40, 3, 1, 1]);
        for (heroes, level) in party_slot_bars(&run) {
            let m = meld_proto::unlocks::Milestone::HeroesReached { heroes, level };
            assert!(
                meld_proto::unlocks::granted_by(m, &have).is_empty(),
                "a single hero at 40 opened a slot it should not"
            );
        }
    }
}

/// The two PROFESSION classes had no overworld perk at all — every other class earns one
/// for walking around, and the pair whose whole identity is what they do between fights
/// earned nothing. These cover MS-1's second ladder.
#[cfg(test)]
mod quarry_tests {
    use super::*;

    fn board(entries: &[(&str, i32, bool)]) -> HashMap<String, (i32, bool)> {
        entries.iter().map(|(k, p, c)| (k.to_string(), (*p, *c))).collect()
    }

    /// A fresh account is working every hunt, so every quarry on the board is tracked.
    #[test]
    fn an_untouched_board_tracks_every_quarry_it_names() {
        let targets = quarry_targets(&board(&[]));
        assert!(targets.contains(&QuarryTarget::Kind("forest_bloom_stalker".into())));
        assert!(targets.contains(&QuarryTarget::Class("gatekeeper".into())));
        assert!(targets.contains(&QuarryTarget::Class("elite".into())));
        // A depth or an extraction has no quarry to mark — there is nothing to point at.
        assert_eq!(
            targets.len(),
            meld_proto::hunts::HUNTS
                .iter()
                .filter(|h| matches!(
                    h.goal,
                    meld_proto::hunts::HuntGoal::Fell { .. }
                        | meld_proto::hunts::HuntGoal::FellClass { .. }
                ))
                .count()
        );
    }

    /// Finished or paid, it stops being marked: the thing left to do is walk home, and a
    /// world that keeps shouting QUARRY at a hunt you closed is noise.
    #[test]
    fn a_finished_or_claimed_hunt_stops_being_tracked() {
        let done = quarry_targets(&board(&[("unseat_the_keeper", 1, false)]));
        assert!(!done.contains(&QuarryTarget::Class("gatekeeper".into())));
        let paid = quarry_targets(&board(&[("cull_the_bloom", 8, true)]));
        assert!(!paid.contains(&QuarryTarget::Kind("forest_bloom_stalker".into())));
        // Partial progress is still a hunt you are working.
        let partial = quarry_targets(&board(&[("cull_the_bloom", 7, false)]));
        assert!(partial.contains(&QuarryTarget::Kind("forest_bloom_stalker".into())));
    }

    #[test]
    fn a_target_matches_its_own_kind_or_class_and_nothing_else() {
        let kind = QuarryTarget::Kind("dune_wyrm".into());
        assert!(kind.matches("dune_wyrm", "standard"));
        assert!(!kind.matches("bog_serpent", "standard"));
        let class = QuarryTarget::Class("gatekeeper".into());
        assert!(class.matches("anything", "gatekeeper"));
        assert!(!class.matches("anything", "elite"));
    }
}

#[cfg(test)]
mod crafter_perk_tests {
    use super::*;
    use meld_proto::enums::CharacterClass::{Keeper, Smithwright};

    fn perks(classes: &[CharacterClass], lvl: i32) -> wr::Perks {
        let b = meld_balance::Balance::load_default().unwrap();
        compute_perks(&b.perks, classes, lvl)
    }

    /// Neutral means neutral: a party without the class gets the identity values, not
    /// zeroes. A `0.0` multiplier would silently make every bench instant and every
    /// alembic field vanish.
    #[test]
    fn a_party_without_a_crafter_is_left_exactly_as_it_was() {
        let p = perks(&[CharacterClass::Explorer], 50);
        assert_eq!(p.smithwright_ore_radius, 0.0);
        assert_eq!(p.smithwright_setup_mult, 1.0, "no smith must not speed benches up");
        assert_eq!(p.smithwright_stock_discount, 0);
        assert!(!p.smithwright_pack_full);
        assert_eq!(p.smithwright_bench_uses, 0);
        assert_eq!(p.keeper_reagent_radius, 0.0);
        assert_eq!(p.keeper_extra_unit_chance, 0.0);
        assert_eq!(p.keeper_field_radius_mult, 1.0, "no Keeper must not shrink the field");
        assert_eq!(p.keeper_field_regen_mult, 1.0);
        assert_eq!(p.keeper_free_unit_chance, 0.0);
    }

    /// Each rung arrives at its own level and not before — the same shape every other
    /// class's perks have.
    #[test]
    fn the_smithwrights_ladder_arrives_a_rung_at_a_time() {
        let b = meld_balance::Balance::load_default().unwrap();
        let at = |lvl| compute_perks(&b.perks, &[Smithwright], lvl);
        let p = &b.perks;

        assert!(at(p.smithwright_ore_sense_at).smithwright_ore_radius > 0.0, "no ore sense");
        // Deeper reads further.
        assert!(
            at(p.smithwright_ore_sense_at + 10).smithwright_ore_radius
                > at(p.smithwright_ore_sense_at).smithwright_ore_radius
        );

        assert_eq!(at(p.smithwright_setup_at - 1).smithwright_setup_mult, 1.0);
        assert!(at(p.smithwright_setup_at).smithwright_setup_mult < 1.0, "benches not quicker");
        assert!(at(p.smithwright_setup_at).smithwright_stock_discount > 0);

        assert!(!at(p.smithwright_pack_full_at - 1).smithwright_pack_full);
        assert!(at(p.smithwright_pack_full_at).smithwright_pack_full);

        assert_eq!(at(p.smithwright_bench_uses_at - 1).smithwright_bench_uses, 0);
        assert!(at(p.smithwright_bench_uses_at).smithwright_bench_uses > 0);
    }

    #[test]
    fn the_keepers_ladder_arrives_a_rung_at_a_time() {
        let b = meld_balance::Balance::load_default().unwrap();
        let at = |lvl| compute_perks(&b.perks, &[Keeper], lvl);
        let p = &b.perks;

        assert!(at(p.keeper_reagent_sense_at).keeper_reagent_radius > 0.0, "no reagent sense");
        assert!(
            at(p.keeper_reagent_sense_at + 10).keeper_reagent_radius
                > at(p.keeper_reagent_sense_at).keeper_reagent_radius
        );

        assert_eq!(at(p.keeper_green_thumb_at - 1).keeper_extra_unit_chance, 0.0);
        assert!(at(p.keeper_green_thumb_at).keeper_extra_unit_chance > 0.0);

        assert_eq!(at(p.keeper_rooted_at - 1).keeper_field_radius_mult, 1.0);
        assert!(at(p.keeper_rooted_at).keeper_field_radius_mult > 1.0, "field did not root");
        assert!(at(p.keeper_rooted_at).keeper_field_regen_mult > 1.0);

        assert_eq!(at(p.keeper_whole_vein_at - 1).keeper_free_unit_chance, 0.0);
        assert!(at(p.keeper_whole_vein_at).keeper_free_unit_chance > 0.0);
    }

    /// A crafter sees the half of the world its OWN trade is built on, and not the other
    /// half — the Foundry reads rock, the Open Flower reads growing things.
    #[test]
    fn each_crafter_reads_its_own_materials() {
        let smith = perks(&[Smithwright], 50);
        assert!(smith.smithwright_ore_radius > 0.0 && smith.keeper_reagent_radius == 0.0);
        let keeper = perks(&[Keeper], 50);
        assert!(keeper.keeper_reagent_radius > 0.0 && keeper.smithwright_ore_radius == 0.0);
        // Both in the party and both halves are lit.
        let both = perks(&[Smithwright, Keeper], 50);
        assert!(both.smithwright_ore_radius > 0.0 && both.keeper_reagent_radius > 0.0);
    }

    /// EVERY class earns something for walking around. Read off the class list rather
    /// than a hand-written one, because the two that were missing were missing for a
    /// whole release and nothing said so.
    #[test]
    fn no_class_walks_the_overworld_with_nothing() {
        let b = meld_balance::Balance::load_default().unwrap();
        let neutral = format!("{:?}", compute_perks(&b.perks, &[], 50));
        for key in meld_proto::skills::all_classes() {
            let Some(class) = meld_proto::equipment::class_from_key(&key) else { continue };
            let p = format!("{:?}", compute_perks(&b.perks, &[class], 50));
            assert!(p != neutral, "{key} earns no overworld perk at all");
        }
    }

    /// The Psyker's overworld perk is a VERB, which is what was left once seeing went to
    /// the Hunter and the map stayed the Explorer's. The pin grows in both directions a
    /// player can feel — longer, and eventually more than one — while the cooldown
    /// shortens to a floor rather than to nothing, because a pin with no gap between uses
    /// walks past every encounter in the game.
    #[test]
    fn the_psyker_reaches_out_and_holds_things() {
        let b = meld_balance::Balance::load_default().unwrap();
        let at = |lv| compute_perks(&b.perks, &[CharacterClass::Psyker], lv);

        let early = at(b.perks.psyker_hold_at);
        assert_eq!(early.psyker_hold_targets, 1, "the pin starts on one creature");
        assert!(early.psyker_hold_seconds > 0.0 && early.psyker_hold_radius > 0.0);
        assert!(!early.psyker_mind_link, "Mind Link is earned later than the pin");

        let deep = at(255);
        assert!(deep.psyker_hold_seconds > early.psyker_hold_seconds, "the pin never lengthens");
        assert!(deep.psyker_hold_targets > early.psyker_hold_targets, "it never widens");
        assert!(deep.psyker_hold_cooldown < early.psyker_hold_cooldown, "it never quickens");
        assert!(deep.psyker_mind_link, "Mind Link never arrives");

        // The floor and the caps hold — otherwise the deep game is a Psyker pinning the
        // whole world permanently.
        assert!(deep.psyker_hold_cooldown >= b.perks.psyker_hold_cooldown_floor);
        assert!(deep.psyker_hold_seconds <= b.perks.psyker_hold_seconds_cap);
        assert!(deep.psyker_hold_targets as i32 <= b.perks.psyker_hold_targets_cap);
        // The load-bearing one, at EVERY level rather than only at the caps: to sustain N
        // pins you must lay one every `seconds / N`, so the cooldown has to stay above
        // that line or a Psyker walks through content with the world held still. The
        // first tuning of these numbers passed at level 1 and failed at 255.
        for lv in 1..=255 {
            let p = at(lv);
            if p.psyker_hold_targets == 0 {
                continue;
            }
            let needed = p.psyker_hold_seconds / p.psyker_hold_targets as f32;
            assert!(
                p.psyker_hold_cooldown > needed,
                "level {lv}: a pin every {needed}s sustains all {} targets on a {}s cooldown",
                p.psyker_hold_targets,
                p.psyker_hold_cooldown
            );
        }

        // And nobody else reaches out at all.
        for other in [CharacterClass::Hunter, CharacterClass::Explorer, CharacterClass::Resonant] {
            let p = compute_perks(&b.perks, &[other], 255);
            assert_eq!(p.psyker_hold_targets, 0, "{other:?} can pin creatures");
            assert!(!p.psyker_mind_link, "{other:?} has Mind Link");
        }
    }

    /// Threat sense is the Hunter's: the long-range half of the same eye that reads a
    /// mob's level, HP and gauge. A party without a Hunter reads nothing.
    #[test]
    fn threat_sense_belongs_to_the_hunter() {
        let b = meld_balance::Balance::load_default().unwrap();
        let hunter = compute_perks(&b.perks, &[CharacterClass::Hunter], 50);
        assert!(hunter.hunter_threat >= 1, "a Hunter marks elites and gatekeepers");
        assert!(hunter.hunter_reveal_radius > 0.0, "and sees them from further off");
        for other in [CharacterClass::Psyker, CharacterClass::Resonant, CharacterClass::Explorer] {
            let p = compute_perks(&b.perks, &[other], 50);
            assert_eq!(p.hunter_threat, 0, "{other:?} should not read threat");
            assert_eq!(p.hunter_reveal_radius, 0.0, "{other:?} should not widen the cull");
        }
    }

    /// A Resonant's walking regen tends only Resonants. Poured over the party it mended
    /// every wound between fights, so the best healer in the game never had to heal.
    #[test]
    fn walking_regen_tends_only_the_healer() {
        let caps = [40, 40, 40];
        let healers = [2usize];
        let mut hps = [5, 6, 30];
        pour_regen(&mut hps, &caps, Some(&healers), 4);
        assert_eq!(hps, [5, 6, 34], "the wounded front line is not the healer's business");
    }

    /// The alembic's field is a PLACE, so it reaches whoever stands in it — most wounded
    /// first, and never a hero already down (standing back up takes a real fight).
    #[test]
    fn a_field_reaches_the_whole_party_but_never_the_fallen() {
        let caps = [40, 40, 40];
        let mut hps = [0, 10, 39];
        pour_regen(&mut hps, &caps, None, 3);
        assert_eq!(hps, [0, 13, 39], "the fallen stay down; the living mend worst-first");
    }

    /// A cap is a cap: a budget bigger than the party's total deficit stops, it does not
    /// spill past max HP or spin.
    #[test]
    fn regen_stops_at_full() {
        let caps = [10, 10];
        let mut hps = [9, 8];
        pour_regen(&mut hps, &caps, None, 99);
        assert_eq!(hps, [10, 10]);
    }
}

#[cfg(test)]
mod spending_tests {
    use super::*;

    fn stacks(kinds: &[(&str, i32)]) -> meld_run::PlayerRun {
        let mut r = super::unlock_gate_tests::run_at(&[1]);
        for (kind, qty) in kinds {
            r.backpack.push(ItemStack {
                item_id: Uuid::now_v7().to_string(),
                item_kind: (*kind).to_string(),
                quantity: *qty,
                insurance: None,
            });
        }
        r
    }

    /// The bug this exists for. A harvest channel banks one unit per tick as its OWN
    /// stack, so ore you just dug up is six stacks of one — never one stack of six. Both
    /// build paths looked for a single stack holding the whole cost, so a structure
    /// costing 6 and a field forge costing 3 were unbuildable from freshly-gathered ore,
    /// and the refusal said "takes 6 ore" to a player carrying exactly six.
    #[test]
    fn ore_gathered_one_unit_at_a_time_still_pays_for_things() {
        let mut run = stacks(&[("heartoak_bark", 1); 6]);
        assert_eq!(
            spend_material(&mut run, meld_proto::materials::MaterialClass::Ore, 6).as_deref(),
            Some("heartoak_bark"),
            "six stacks of one could not pay a cost of six"
        );
        assert!(run.backpack.is_empty(), "spent stacks were left behind at zero");
    }

    #[test]
    fn a_run_that_cannot_cover_the_cost_is_refused_and_charged_nothing() {
        let mut run = stacks(&[("heartoak_bark", 1); 5]);
        assert_eq!(spend_material(&mut run, meld_proto::materials::MaterialClass::Ore, 6), None);
        assert_eq!(
            run.backpack.iter().map(|i| i.quantity).sum::<i32>(),
            5,
            "a refused spend still took the ore"
        );
    }

    /// One KIND, not a mix: a structure records what it was built from so packing it down
    /// hands back the same stock, and a refund cannot be split across materials.
    #[test]
    fn a_spend_never_mixes_two_materials_to_reach_the_cost() {
        let mut run = stacks(&[("heartoak_bark", 3), ("peat_iron", 3)]);
        assert_eq!(spend_material(&mut run, meld_proto::materials::MaterialClass::Ore, 6), None);
        assert_eq!(run.backpack.len(), 2, "it mixed two ores and spent both");
    }

    /// A builder who hauled good ore out should not have it spent last.
    #[test]
    fn the_deepest_ore_that_can_cover_it_is_the_one_spent() {
        let deep = meld_proto::materials::material("peat_iron").map(|m| m.tier).unwrap_or(0);
        let shallow = meld_proto::materials::material("heartoak_bark").map(|m| m.tier).unwrap_or(0);
        assert!(deep > shallow, "the fixture no longer has two different tiers");
        let mut run = stacks(&[("heartoak_bark", 6), ("peat_iron", 6)]);
        assert_eq!(
            spend_material(&mut run, meld_proto::materials::MaterialClass::Ore, 6).as_deref(),
            Some("peat_iron")
        );
    }
}

#[cfg(test)]
mod level_up_cure_tests {

    /// Going up a level lifts what is gripping THAT hero, and nothing else's.
    #[test]
    fn advancing_lifts_the_conditions_off_the_hero_that_advanced() {
        let (mut w, _rx) = super::shifting_lands_tests::world(50, 10);
        w.hero_afflictions.insert(
            "p1".into(),
            vec![
                vec!["poison".to_string(), "web".to_string()],
                vec!["blinded".to_string()],
            ],
        );
        w.cure_on_level_up("p1", 0);
        let carried = &w.hero_afflictions["p1"];
        assert!(carried[0].is_empty(), "the hero that levelled is still poisoned");
        assert_eq!(
            carried[1],
            vec!["blinded".to_string()],
            "a hero who did not level should keep what it caught"
        );
    }

    /// Death is the exception: levelling cures conditions, it does not raise anybody.
    /// HP lives in `hero_hp` and this must not go near it.
    #[test]
    fn a_cure_does_not_raise_the_fallen() {
        let (mut w, _rx) = super::shifting_lands_tests::world(50, 10);
        w.hero_hp.insert("p1".into(), vec![0, 12]);
        w.hero_afflictions.insert("p1".into(), vec![vec!["dread".to_string()], vec![]]);
        w.cure_on_level_up("p1", 0);
        assert_eq!(w.hero_hp["p1"][0], 0, "levelling must not stand a fallen hero back up");
    }

    /// Nothing recorded for that player, or a slot past the end of the party: a cure is a
    /// no-op rather than a panic, because the XP paths call it before they know either.
    #[test]
    fn curing_an_unknown_hero_is_harmless() {
        let (mut w, _rx) = super::shifting_lands_tests::world(50, 10);
        w.cure_on_level_up("nobody", 0);
        w.hero_afflictions.insert("p1".into(), vec![vec!["poison".to_string()]]);
        w.cure_on_level_up("p1", 7);
        assert_eq!(w.hero_afflictions["p1"][0], vec!["poison".to_string()]);
    }
}

#[cfg(test)]
mod shifting_lands_tests {
    use super::*;

    /// A world with a Shift due almost immediately, so the driver can be watched
    /// end-to-end in a test instead of in five wall-clock minutes.
    pub(super) fn world(
        cadence: u64,
        warning: u64,
    ) -> (WorldActor, mpsc::UnboundedReceiver<DbWrite>) {
        let mut b = Balance::load_default().unwrap();
        b.shift.cadence_ticks = cadence;
        b.shift.cadence_jitter = 0.0;
        b.shift.warning_ticks = warning;
        let balance = Arc::new(b);
        let (tx, rx) = mpsc::unbounded_channel();
        let mut arena = Arena::generate(&balance, 909, false);
        for _ in 0..30 {
            arena.ensure_frontier(&balance, 900.0);
        }
        let w = WorldActor {
            balance: balance.clone(),
            db_writes: tx,
            arena,
            run: InstanceRun::new("w".into(), 0, &balance, 0),
            battles: Vec::new(),
            hero_hp: HashMap::new(),
            hero_afflictions: HashMap::new(),
            durability_charged: HashSet::new(),
            venom_steps: HashMap::new(),
            party_classes: HashMap::new(),
            gear_bonuses: HashMap::new(),
            hero_names: HashMap::new(),
            hero_rows: HashMap::new(),
            extraction: HashMap::new(),
            harvest: HashMap::new(),
            building: HashMap::new(),
            regen_accum: HashMap::new(),
            hold_last_ms: HashMap::new(),
            watching: HashMap::new(),
            entrances: Vec::new(),
            tutorial: false,
            tutorial_entrance_placed: false,
            location: HashMap::new(),
            dungeons: HashMap::new(),
            next_dungeon_key: 0,
            entrances_scanned: 0,
            dungeon_scene_sent: HashMap::new(),
            pending_effects: Vec::new(),
            edges: HashMap::new(),
            skill_levels: HashMap::new(),
            battle_immune_until: HashMap::new(),
            quarry: HashMap::new(),
            bounties: HashMap::new(),
            marks_placed: std::collections::HashSet::new(),
            tick_count: 0,
            shift_generation: 0,
            shift_warned: false,
            shift_log: Vec::new(),
        };
        (w, rx)
    }

    /// The whole loop through the server driver: the tell goes up once, the land swaps
    /// on schedule, and the retiled sections are re-sent — which is what repaints the
    /// ground, since the client keys its biome rings off `world.terrain_section`.
    #[test]
    fn the_tell_goes_up_once_then_the_land_swaps() {
        let (mut w, _rx) = world(20, 5);
        w.run.add_party(vec![("p1".into(), "p1".into(), CharacterClass::Explorer, "r1".into())]);
        let mut warnings = 0;
        let mut shifts = 0;
        let mut retiles = 0;
        for _ in 0..25 {
            for m in w.advance_shift() {
                match m.msg_type {
                    ww::ShiftWarning::TYPE => warnings += 1,
                    ww::Shifted::TYPE => shifts += 1,
                    ww::TerrainSection::TYPE => retiles += 1,
                    _ => {}
                }
            }
            w.tick_count += 1;
        }
        assert_eq!(warnings, 1, "the tell fired {warnings} times, not once");
        assert_eq!(shifts, 1, "the land swapped {shifts} times");
        assert!(retiles >= 1, "nothing was re-sent, so nothing repaints");
        assert_eq!(w.shift_generation, 1, "the generation was not retired");
    }


    /// §W5's claim, checked: seed + a small delta reconstructs the world exactly. If this
    /// ever fails, hibernation is silently handing players a different place than the one
    /// they left — which is worse than not persisting at all.
    #[test]
    fn a_hibernated_world_comes_back_the_same_place() {
        let (mut w, _rx) = world(20, 5);
        w.run.add_party(vec![("p1".into(), "p1".into(), CharacterClass::Explorer, "r1".into())]);
        // Live a while: several Shifts land, a node is dug out, a chest is opened, and a
        // creature is felled and pruned into the ground it used to hold.
        for _ in 0..200 {
            w.advance_shift();
            w.tick_count += 1;
        }
        w.arena.resources[0].remaining = 0;
        w.arena.chests[0].opened = true;
        w.arena.monsters[0].defeated = true;
        let felled = w.arena.monsters[0].entity_id.clone();
        w.arena.prune_defeated();
        w.arena.regrow(&w.balance.clone(), w.tick_count);
        assert!(!w.shift_log.is_empty(), "nothing shifted, so nothing is being tested");

        let save = w.world_save();
        let back = restore_world(&w.balance, &save);

        assert_eq!(back.areas.len(), w.arena.areas.len(), "the frontier did not come back");
        assert_eq!(
            back.areas.iter().map(|a| a.biome).collect::<Vec<_>>(),
            w.arena.areas.iter().map(|a| a.biome).collect::<Vec<_>>(),
            "the shifted biomes were not replayed"
        );
        assert_eq!(
            back.obstacles.len(),
            w.arena.obstacles.len(),
            "the re-scattered props were not reproduced"
        );
        assert_eq!(
            back.obstacles.iter().map(|o| o.position).collect::<Vec<_>>(),
            w.arena.obstacles.iter().map(|o| o.position).collect::<Vec<_>>(),
            "the props came back somewhere else"
        );
        assert!(back.resources[0].depleted(), "the dug-out node came back full");
        assert!(back.chests[0].opened, "the opened chest came back sealed");
        assert!(
            back.monsters.iter().all(|m| m.entity_id != felled),
            "a creature the world remembers as dead was standing there again"
        );
        assert_eq!(back.fallen.len(), w.arena.fallen.len());
    }

    /// An anchor IS the ground a player holds, so a world that forgot one on restart
    /// would hand their region back to the Shift for free. The most load-bearing entry in
    /// the whole delta.
    #[test]
    fn a_hibernated_world_still_holds_the_ground_you_anchored() {
        let (mut w, _rx) = world(20, 5);
        w.run.add_party(vec![("p1".into(), "p1".into(), CharacterClass::Explorer, "r1".into())]);
        w.arena.add_avatar("p1".into(), 5.0);
        let balance = w.balance.clone();
        let lat = w.arena.corridor_lateral();
        let half = w.arena.radial_half();
        let mut placed = None;
        for k in 0..400 {
            let frac = -0.9 + 1.8 * (k as f64 / 400.0);
            let (r, y) = (300.0_f64, lat * frac);
            let theta = (y / lat.max(1.0)).clamp(-1.0, 1.0) * half;
            let p = if half > 0.0 {
                Position::new(r * theta.cos(), r * theta.sin())
            } else {
                Position::new(r, y)
            };
            w.arena.avatar_mut("p1").unwrap().position = p;
            if let Ok(s) = w.arena.place_structure(&balance, "p1", "anchor", "dune_iron", 7) {
                placed = Some(s.entity_id.clone());
                break;
            }
        }
        let id = placed.expect("somewhere legal at d300");
        w.arena.advance_builds(7 + w.arena.structures[0].build_ticks);
        w.arena.structures[0].hp -= 40;
        let (hp, max_hp) = (w.arena.structures[0].hp, w.arena.structures[0].max_hp);

        let back = restore_world(&w.balance, &w.world_save());
        let s = back
            .structures
            .iter()
            .find(|s| s.entity_id == id)
            .expect("the anchor did not survive the restart");
        assert_eq!((s.hp, s.max_hp), (hp, max_hp), "it came back at a different HP");
        assert_eq!(s.owner_player_id, "p1");
        assert!(s.pins(), "it came back as something that no longer holds ground");
    }

    /// The delta is a *delta*: a world with hours on it must not cost megabytes to write.
    #[test]
    fn a_saved_world_stores_the_delta_and_not_the_map() {
        let (mut w, _rx) = world(20, 5);
        for _ in 0..400 {
            w.advance_shift();
            w.tick_count += 1;
        }
        let save = w.world_save();
        assert!(
            save.delta.len() < 64 * 1024,
            "the delta is {} bytes for {} sections and {} props — it is storing the map",
            save.delta.len(),
            w.arena.areas.len(),
            w.arena.obstacles.len()
        );
        assert!(w.arena.obstacles.len() > 200, "not enough world to make the claim");
    }

    /// Standing in it costs you; the schedule is otherwise identical, so the only
    /// variable is where the party was.
    #[test]
    fn the_force_blast_only_reaches_what_is_standing_in_it() {
        let (mut w, _rx) = world(20, 5);
        w.run.add_party(vec![("p1".into(), "p1".into(), CharacterClass::Explorer, "r1".into())]);
        w.party_classes.insert("p1".into(), vec![CharacterClass::Explorer; 4]);
        w.hero_hp.insert("p1".into(), vec![9999; 4]);
        w.arena.add_avatar("p1".into(), 5.0);

        let roll = meld_world::shift::roll(&w.balance, w.arena.seed, 0);
        let (first, last) = w.arena.shift_region(&w.balance, &roll).expect("a region");
        let (inner, outer) = w.arena.shift_band(first, last);
        let safe = w.hero_hp["p1"].clone();
        for _ in 0..25 {
            w.advance_shift();
            w.tick_count += 1;
        }
        assert_eq!(w.hero_hp["p1"], safe, "the hub took Force damage from a distant Shift");

        let (mut w, _rx) = world(20, 5);
        w.run.add_party(vec![("p1".into(), "p1".into(), CharacterClass::Explorer, "r1".into())]);
        w.party_classes.insert("p1".into(), vec![CharacterClass::Explorer; 4]);
        w.hero_hp.insert("p1".into(), vec![9999; 4]);
        w.arena.add_avatar("p1".into(), 5.0);
        let mid = (inner + outer) * 0.5;
        w.arena.avatar_mut("p1").unwrap().position = Position::new(mid, 0.0);
        for _ in 0..25 {
            w.advance_shift();
            w.tick_count += 1;
        }
        assert!(
            w.hero_hp["p1"].iter().all(|h| *h < 9999),
            "the party stood in the Shift and took nothing"
        );
    }
}

#[cfg(test)]
mod watching_tests {
    use super::*;

    fn env(msg_type: &str) -> RawEnvelope {
        RawEnvelope {
            msg_type: msg_type.to_string(),
            seq: 1,
            ts: 0,
            payload: serde_json::json!({}),
        }
    }

    /// Did `player_id` get told `msg_type`, and what did it say?
    pub(super) fn sent(out: &[Outgoing], pid: &str, msg_type: &str) -> Option<serde_json::Value> {
        out.iter()
            .find(|o| o.player_id == pid && o.msg_type == msg_type)
            .map(|o| serde_json::from_str(o.payload.get()).expect("payload is json"))
    }

    /// A world with a fight already going: p1's party is locked in with the nearest
    /// creature, p2 is standing right beside them doing nothing at all.
    fn a_fight_and_a_bystander() -> WorldActor {
        let (mut w, rx) = super::shifting_lands_tests::world(1_000_000, 1);
        // The DB sink must outlive the world or every enqueue is a send error.
        std::mem::forget(rx);
        let p1 = w
            .run
            .add_party(vec![("p1".into(), "p1".into(), CharacterClass::Explorer, "r1".into())]);
        w.run
            .add_party(vec![("p2".into(), "p2".into(), CharacterClass::Explorer, "r2".into())]);
        w.arena.add_avatar("p1".into(), 5.0);
        w.arena.add_avatar("p2".into(), 5.0);
        let at = w.arena.monsters[0].position;
        for pid in ["p1", "p2"] {
            if let Some(a) = w.arena.avatar_mut(pid) {
                a.position = at;
            }
        }
        w.start_battle("p1", p1, 0);
        assert_eq!(w.battles.len(), 1, "the fixture did not start a fight");
        w
    }

    /// The feature, in one test: a bystander gets the whole fight and none of the
    /// commitment. `spectating` is on the wire and `your_combatant_ids` is empty, which
    /// together are what the client reads as "look, do not touch".
    #[test]
    fn a_bystander_can_watch_the_fight_without_being_in_it() {
        let mut w = a_fight_and_a_bystander();
        let (out, _) = w.handle_watch_battle("p2", env(wr::WatchBattle::TYPE));
        let started = sent(&out, "p2", wb::Started::TYPE).expect("no battle.started for the watcher");
        assert_eq!(started["spectating"], serde_json::json!(true));
        assert_eq!(
            started["your_combatant_ids"].as_array().map(Vec::len),
            Some(0),
            "a watcher was handed combatants to command"
        );
        assert!(
            started["enemies"].as_array().is_some_and(|e| !e.is_empty()),
            "the watcher was sent an empty fight"
        );
        // p1's party is still the only party IN it: watching must not merge you.
        assert_eq!(w.battles[0].parties.len(), 1, "watching joined the fight");
    }

    /// The whole point of the audience funnel. Every battle broadcast asks ONE question
    /// ("who is watching this fight"), so a watcher receives each new event type the day
    /// it is added — rather than the day somebody remembers to add them to its call site.
    #[test]
    fn every_battle_broadcast_reaches_the_watcher_including_the_gauges() {
        let mut w = a_fight_and_a_bystander();
        w.handle_watch_battle("p2", env(wr::WatchBattle::TYPE));
        let audience = w.audience_of(&w.battles[0]);
        assert!(audience.contains(&"p1".to_string()), "the fighter fell off its own fight");
        assert!(audience.contains(&"p2".to_string()), "the watcher is not on the funnel");
        // The gauges used to re-derive the party filter inline, which is exactly how a
        // watcher ends up receiving everything EXCEPT the thing that moves — a feed that
        // reads as the fight having frozen.
        let gauges = w.gauge_update_msgs(&w.battles[0]);
        assert!(
            gauges.iter().any(|o| o.player_id == "p2"),
            "the watcher was left out of the gauge stream"
        );
    }

    /// A watcher earns nothing and answers for nothing, so they are not a FIGHTER — the
    /// two lists are deliberately different, and the things a fight pays out read the
    /// narrower one.
    #[test]
    fn a_watcher_is_audience_and_never_a_fighter() {
        let mut w = a_fight_and_a_bystander();
        w.handle_watch_battle("p2", env(wr::WatchBattle::TYPE));
        assert_eq!(w.fighters_of(&w.battles[0]), vec!["p1".to_string()]);
    }

    /// A watcher owns no combatant, so every action they could send lands on the same
    /// refusal an impostor's would. No separate spectator guard to keep in sync.
    #[test]
    fn a_watcher_cannot_act_in_the_fight_they_are_watching() {
        let mut w = a_fight_and_a_bystander();
        w.handle_watch_battle("p2", env(wr::WatchBattle::TYPE));
        let bid = w.battles[0].battle_id.clone();
        let mut raw = env(wb::SubmitAction::TYPE);
        raw.payload = serde_json::json!({ "battle_id": bid, "action": "attack" });
        let (out, _) = w.handle_submit("p2", raw);
        assert!(
            sent(&out, "p2", ws::Error::TYPE).is_some(),
            "a watcher was allowed to act in somebody else's fight"
        );
    }

    /// You cannot watch and swing. Otherwise a player already fighting could open a
    /// second battle screen over the top of their own.
    #[test]
    fn you_cannot_watch_while_you_are_fighting() {
        let mut w = a_fight_and_a_bystander();
        let (out, _) = w.handle_watch_battle("p1", env(wr::WatchBattle::TYPE));
        assert!(sent(&out, "p1", ws::Error::TYPE).is_some(), "a fighter opened a spectator feed");
        assert!(w.watching.is_empty());
    }

    /// Stepping in ends looking on. Without this the watcher would hold a feed on the
    /// fight they are now standing in, and the sweep would close it a tick later —
    /// yanking them out of their own battle screen.
    #[test]
    fn joining_the_fight_you_were_watching_ends_the_watch() {
        let mut w = a_fight_and_a_bystander();
        w.handle_watch_battle("p2", env(wr::WatchBattle::TYPE));
        assert!(w.watching.contains_key("p2"));
        let (out, _) = w.handle_join_battle("p2", env(wr::JoinBattle::TYPE));
        assert!(!w.watching.contains_key("p2"), "the feed survived joining");
        assert!(sent(&out, "p2", wb::WatchEnded::TYPE).is_some(), "nothing closed the feed");
        assert!(!w.battles[0].spectators.contains("p2"), "a fighter is still on the watch list");
    }

    /// Walk out of range and the feed closes on its own — the same tick sweep that keeps
    /// it honest when the fight ends.
    #[test]
    fn walking_out_of_range_closes_the_feed() {
        let mut w = a_fight_and_a_bystander();
        w.handle_watch_battle("p2", env(wr::WatchBattle::TYPE));
        let far = w.balance.ai.watch_radius * 4.0;
        if let Some(a) = w.arena.avatar_mut("p2") {
            a.position = Position::new(a.position.x + far, a.position.y);
        }
        let out = w.sweep_watchers();
        let closed = sent(&out, "p2", wb::WatchEnded::TYPE).expect("the feed stayed open");
        assert_eq!(closed["reason"], serde_json::json!("out_of_range"));
        assert!(w.watching.is_empty());
    }

    /// `battle.ended` never reaches a watcher — it carries somebody else's XP and haul —
    /// so this is the ONLY thing that takes their screen down when the fight resolves.
    #[test]
    fn the_feed_closes_when_the_fight_it_was_watching_does() {
        let mut w = a_fight_and_a_bystander();
        w.handle_watch_battle("p2", env(wr::WatchBattle::TYPE));
        w.battles.clear(); // the fight resolved; its slot is gone
        let out = w.sweep_watchers();
        let closed = sent(&out, "p2", wb::WatchEnded::TYPE).expect("the feed outlived the fight");
        assert_eq!(closed["reason"], serde_json::json!("finished"));
    }

    /// Asking twice for the same feed is a no-op, not a re-send: the client fires this off
    /// a key, and a fresh `battle.started` per press would rebuild its battle screen.
    #[test]
    fn re_asking_for_the_feed_you_already_have_says_nothing() {
        let mut w = a_fight_and_a_bystander();
        w.handle_watch_battle("p2", env(wr::WatchBattle::TYPE));
        let (again, _) = w.handle_watch_battle("p2", env(wr::WatchBattle::TYPE));
        assert!(again.is_empty(), "the same feed was re-sent and would reset the screen");
    }

    /// Stopping is idempotent, because the client toggles it off one key.
    #[test]
    fn stopping_something_you_were_not_watching_is_not_an_error() {
        let mut w = a_fight_and_a_bystander();
        let (out, _) = w.handle_stop_watching("p2", env(wr::StopWatching::TYPE));
        assert!(out.is_empty());
    }

    /// CR-2: two mobs tearing at each other is a fight too, and it arrives as the same
    /// feed — one client path for both sources. Its `battle_id` is namespaced so an action
    /// aimed at it can never resolve against a real battle.
    ///
    /// The clash is not staged: the world is stepped and one of the brawls it starts on
    /// its own is walked over to. Hostile factions are seeded into every biome roster
    /// precisely so this happens everywhere, and a test that hand-placed two creatures
    /// would prove nothing about whether the player ever meets one.
    #[test]
    fn a_creature_clash_is_watched_through_the_same_feed() {
        let mut w = a_fight_and_a_bystander();
        w.arena.step_creatures(0.2);
        let (at, members) = w
            .arena
            .clashes
            .iter()
            .map(|c| (c.position, c.members.clone()))
            .next()
            .expect("the world started no clashes of its own");
        if let Some(a) = w.arena.avatar_mut("p2") {
            a.position = at;
        }

        let (out, _) = w.handle_watch_battle("p2", env(wr::WatchBattle::TYPE));
        let started = sent(&out, "p2", wb::Started::TYPE).expect("no feed for the clash");
        assert_eq!(started["spectating"], serde_json::json!(true));
        assert!(
            started["battle_id"].as_str().is_some_and(|id| id.starts_with("clash:")),
            "a clash borrowed a real battle id"
        );
        assert_eq!(
            started["enemies"].as_array().map(Vec::len),
            Some(members.len()),
            "the feed does not hold every body in the brawl"
        );
        // And an action aimed at it is refused, because there is no battle behind it.
        let mut raw = env(wb::SubmitAction::TYPE);
        raw.payload = serde_json::json!({
            "battle_id": started["battle_id"].as_str().unwrap_or_default(),
            "action": "attack",
        });
        let (refused, _) = w.handle_submit("p2", raw);
        assert!(sent(&refused, "p2", ws::Error::TYPE).is_some(), "a clash accepted a player action");
    }

    /// A clash that stops being a clash closes its feed. Not a nicety: the creatures are
    /// gone from the arena the moment they are pruned, so a feed left open would stream
    /// an empty roster forever.
    #[test]
    fn a_clash_that_ends_closes_its_feed() {
        let mut w = a_fight_and_a_bystander();
        w.watching.insert(
            "p2".to_string(),
            WatchFeed::Clash { anchor: "nothing-is-fighting".to_string(), roster: Vec::new() },
        );
        let out = w.sweep_watchers();
        let closed = sent(&out, "p2", wb::WatchEnded::TYPE).expect("the feed outlived the clash");
        assert_eq!(closed["reason"], serde_json::json!("finished"));
    }

    /// The wire path end to end: a clashing creature rides the snapshot with its marker,
    /// so the client can draw the ⚔ and the HP bar without asking anything else.
    ///
    /// Worth pinning at this level because the tag is the ONLY thing the client has to go
    /// on — the clash itself lives entirely server-side, and a marker that never left the
    /// server would make the whole of `CR-2`'s visibility invisible while every
    /// world-level test still passed.
    #[test]
    fn a_clashing_creature_says_so_in_the_snapshot() {
        let mut w = a_fight_and_a_bystander();
        w.arena.step_creatures(0.2);
        let at = w
            .arena
            .clashes
            .first()
            .map(|c| c.position)
            .expect("the world started no clashes of its own");
        if let Some(a) = w.arena.avatar_mut("p2") {
            a.position = at;
        }
        let out = w.snapshot_msgs();
        let snap = sent(&out, "p2", wm::Snapshot::TYPE).expect("no snapshot for the bystander");
        let tagged: Vec<String> = snap["entities"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|e| e["avatar_state"].as_str())
            .filter(|s| s.starts_with("mob:") && s.ends_with(":clash"))
            .map(String::from)
            .collect();
        assert!(
            !tagged.is_empty(),
            "the brawl the player is standing in the middle of is unmarked: {:?}",
            snap["entities"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|e| e["avatar_state"].as_str())
                .filter(|s| s.starts_with("mob:"))
                .collect::<Vec<_>>()
        );
        // The faction still reads cleanly with a marker appended — reading it with a
        // `split_once` is what swallowed `hostile:quarry` whole once already.
        for tag in &tagged {
            let mut parts = tag.split(':');
            assert_eq!(parts.next(), Some("mob"));
            assert!(parts.next().is_some_and(|k| !k.is_empty()), "{tag}");
            assert!(parts.next().is_some_and(|f| !f.is_empty()), "{tag}");
        }
    }

    /// CR-2: a creature that survives a fight resumes roaming WOUNDED.
    ///
    /// The bug this pins: every point the party landed was forgotten the moment the battle
    /// slot dropped, so fleeing a fight you had nearly won reset the creature to full and
    /// the whole encounter had to be paid for again — and softening something up to come
    /// back for it later was impossible.
    ///
    /// The wound rides back as a FRACTION, never as the raw battle number: the fight scaled
    /// the creature's pool by `encounter_party_scale`, so a four-hero party chews through
    /// several times the health the spawn actually has. Writing the raw remainder onto it
    /// would kill it outright.
    #[test]
    fn a_creature_that_survives_a_fight_stays_wounded() {
        let mut w = a_fight_and_a_bystander();
        let bid = w.battles[0].battle_id.clone();
        let (creature, cid) = w.battles[0]
            .monster_combatants
            .iter()
            .map(|(e, c)| (e.clone(), c.clone()))
            .next()
            .expect("the fight knows no creature to carry a wound back to");
        let full = w.arena.monster_by_id(&creature).map(|m| m.max_hp).expect("no such creature");
        assert_eq!(
            w.arena.monster_by_id(&creature).map(|m| m.hp),
            Some(full),
            "the fixture's creature was already hurt"
        );

        // Actually fight it. The engine's own timeout auto-DEFENDS rather than attacking,
        // so a party that never submits never lands anything — swing for real instead.
        let heroes: Vec<String> =
            w.battles[0].player_combatants.get("p1").cloned().unwrap_or_default();
        let mut left = 1.0;
        let mut swing = 0u64;
        for _ in 0..600 {
            if w.battles.is_empty() {
                break;
            }
            let _ = w.battles[0].battle.tick();
            for h in &heroes {
                swing += 1;
                let _ = w.battles[0].battle.submit(
                    h,
                    format!("a{swing}"),
                    BattleActionKind::Attack,
                    Some(vec![cid.clone()]),
                    None,
                    None,
                );
            }
            if let Some((hp, max)) = w.battles[0].battle.combatant_health(&cid) {
                left = (hp.max(0) as f64) / (max as f64);
                if left < 0.9 {
                    break;
                }
            }
        }
        assert!(left < 0.9, "the party never landed a blow in 60s of fight");

        // Flee: the party bolts, the creature lives — and keeps the damage.
        w.handle_battle_end(&bid, BattleOutcome::Fled);
        let m = w.arena.monster_by_id(&creature).expect("the creature vanished");
        assert!(!m.in_battle, "a fled-from creature never resumed roaming");
        assert!(m.hp < full, "the wound was forgotten: {} of {full}", m.hp);
        assert!(m.hp >= 1, "a creature the engine says is alive was written back dead");
        assert_eq!(m.max_hp, full, "the wound shrank the creature instead of hurting it");
        // The fraction, not the raw number — the battle pool is `party_scale` times this.
        let carried = (m.hp as f64) / (full as f64);
        assert!(
            (carried - left).abs() < 0.02,
            "the fight left it at {left:.3} but the world says {carried:.3}"
        );
    }

    /// You can SEE further than you can reach, and watching commits nothing — so the
    /// radius that offers a look must be the wider of the two. Reversed, the only fight
    /// you could watch would be one you were already close enough to walk into.
    #[test]
    fn you_can_watch_from_further_than_you_can_join() {
        let b = Balance::load_default().unwrap();
        assert!(
            b.ai.watch_radius > b.ai.join_radius,
            "watch_radius {} is not wider than join_radius {}",
            b.ai.watch_radius,
            b.ai.join_radius
        );
    }
}

/// GR-2: the durability tax follows the hero who FELL. These hold the two halves the
/// old wipe-scoped sink could not express — one hero going down while the party lives,
/// and a wipe being nothing more than every hero going down at once.
#[cfg(test)]
mod hero_fall_tax_tests {
    use super::*;
    use meld_dungeon_run::TrapHit;

    fn a_party_in_the_field(hp: Vec<i32>) -> (WorldActor, mpsc::UnboundedReceiver<DbWrite>) {
        let (mut w, rx) = super::shifting_lands_tests::world(1_000_000, 1);
        w.run
            .add_party(vec![("p1".into(), "p1".into(), CharacterClass::Explorer, "r1".into())]);
        w.arena.add_avatar("p1".into(), 5.0);
        w.hero_hp.insert("p1".into(), hp);
        (w, rx)
    }

    fn drained(rx: &mut mpsc::UnboundedReceiver<DbWrite>) -> Vec<DbWrite> {
        let mut out = Vec::new();
        while let Ok(w) = rx.try_recv() {
            out.push(w);
        }
        out
    }

    fn falls_of(writes: &[DbWrite]) -> Vec<(String, i32, u32)> {
        writes
            .iter()
            .filter_map(|w| match w {
                DbWrite::HeroFalls(pid, slot, n) => Some((pid.clone(), *slot, *n)),
                _ => None,
            })
            .collect()
    }

    fn a_death_was_recorded(writes: &[DbWrite]) -> bool {
        writes.iter().any(|w| matches!(w, DbWrite::Death(_)))
    }

    /// The whole change, in one test: a trap kills ONE hero, the party walks on, and
    /// that hero's gear is billed while nobody else's is. Under the old rule this run
    /// paid nothing at all — the sink fired only when the run itself ended.
    #[test]
    fn one_hero_falling_bills_that_hero_and_the_run_continues() {
        let (mut w, mut rx) = a_party_in_the_field(vec![1, 9_999, 9_999, 9_999]);
        let out = w.apply_trap_hit("p1", &TrapHit { kind: "dart".into(), severity: 0 });
        let writes = drained(&mut rx);
        assert_eq!(
            falls_of(&writes),
            vec![("p1".to_string(), 0, 1)],
            "only the hero the trap put down owes anything"
        );
        assert!(!a_death_was_recorded(&writes), "the party survived; the run has not ended");
        assert!(out.is_empty(), "a survivable trap should not end the run");
    }

    /// A wipe is not its own rule any more — it is four falls, so it arrives as four
    /// charges. If this ever regresses to a single whole-party write, the tax stops
    /// being able to say WHICH hero fell.
    #[test]
    fn a_wipe_is_every_hero_falling_and_nothing_more() {
        let (mut w, mut rx) = a_party_in_the_field(vec![1, 1, 1, 1]);
        let _ = w.apply_trap_hit("p1", &TrapHit { kind: "pit".into(), severity: 0 });
        let writes = drained(&mut rx);
        let mut slots: Vec<i32> = falls_of(&writes).into_iter().map(|(_, s, _)| s).collect();
        slots.sort();
        assert_eq!(slots, vec![0, 1, 2, 3], "each hero pays for its own fall, once");
        assert!(
            a_death_was_recorded(&writes),
            "a wipe still ends the run — that is what takes the standard gear"
        );
    }

    /// The headline claim, end to end: a hero falls in a fight the party goes on to
    /// WIN, and the tax still lands. This is what the old rule could not do — the sink
    /// fired on the run's terminal transition, so a party that lost a hero and then
    /// won, extracted, and went home paid nothing for it, and durability was a death
    /// penalty rather than the repair sink GDD §7 specifies.
    #[test]
    fn a_hero_that_fell_pays_even_when_the_party_wins() {
        let (mut w, mut rx) = super::shifting_lands_tests::world(1_000_000, 1);
        let party = w
            .run
            .add_party(vec![("p1".into(), "p1".into(), CharacterClass::Explorer, "r1".into())]);
        w.arena.add_avatar("p1".into(), 5.0);
        let at = w.arena.monsters[0].position;
        if let Some(a) = w.arena.avatar_mut("p1") {
            a.position = at;
        }
        // The hero walks in on its last hit point, so the creature's first blow puts it
        // down. Carried HP is how a wound crosses between fights, so this is a state a
        // real party reaches by taking a bad encounter and pressing on.
        w.hero_hp.insert("p1".into(), vec![1]);
        w.start_battle("p1", party, 0);
        assert_eq!(w.battles.len(), 1, "the fixture did not start a fight");
        let bid = w.battles[0].battle_id.clone();
        let hero = w.battles[0]
            .player_combatants
            .get("p1")
            .and_then(|c| c.first().cloned())
            .expect("the hero has no combatant");
        let _ = drained(&mut rx);

        // Let the creature act. The hero never swings — the engine's timeout
        // auto-DEFENDS, and defending does not save a hero on 1 HP.
        for _ in 0..900 {
            if w.battles.is_empty() || w.battles[0].battle.combatant_falls(&hero) > 0 {
                break;
            }
            let _ = w.battles[0].battle.tick();
        }
        assert_eq!(
            w.battles[0].battle.combatant_falls(&hero),
            1,
            "the creature never put the hero down, so there is nothing to charge"
        );

        // The party finishes the fight anyway. Victory, not defeat — the point.
        let (out, _) = w.handle_battle_end(&bid, BattleOutcome::Victory);
        let writes = drained(&mut rx);

        // And the player is TOLD, on the same card that reports the XP and the loot. A
        // charge nobody is shown is a charge that reads as a bug the next time they open
        // the Vault.
        let ended = super::watching_tests::sent(&out, "p1", wb::Ended::TYPE)
            .expect("no battle.ended for the fighter");
        let charged = ended["gear_worn"].as_array().expect("gear_worn missing from the wire");
        assert_eq!(charged.len(), 1, "the fallen hero was left off the report: {ended}");
        assert_eq!(charged[0]["falls"], serde_json::json!(1));
        assert_eq!(
            charged[0]["durability_lost"],
            serde_json::json!(w.balance.loot.durability_loss_per_fall),
            "the card quoted a different number than the tax charges"
        );
        assert_eq!(
            falls_of(&writes),
            vec![("p1".to_string(), 0, 1)],
            "a hero fell and the win wrote it off"
        );
        assert!(
            !a_death_was_recorded(&writes),
            "nobody's run ended — a fallen hero is not a wipe"
        );
    }

    /// The other direction, and the one a per-fall tax could get wrong: heroes who
    /// were merely HURT owe nothing.
    #[test]
    fn a_hero_that_survives_the_blow_pays_nothing() {
        let (mut w, mut rx) = a_party_in_the_field(vec![9_999, 9_999, 9_999, 9_999]);
        let _ = w.apply_trap_hit("p1", &TrapHit { kind: "dart".into(), severity: 0 });
        let writes = drained(&mut rx);
        let charged = falls_of(&writes);
        assert!(charged.is_empty(), "nobody went down, yet gear was billed: {charged:?}");
    }
}
