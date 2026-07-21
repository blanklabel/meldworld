//! Realtime C2S/S2C message payloads, grouped by domain
//! (interfaces/realtime-protocol.md and its detail files).
//!
//! Each payload struct binds to its wire `type` string via [`Message::TYPE`],
//! so the gateway can peek a [`crate::RawEnvelope`], match the string, and
//! decode into the right struct. Only the subset the today-slice uses is
//! modelled; the rest land as their systems do.

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::common::{Combatant, ItemStack, LootGear, Position};
use crate::enums::*;
use crate::Id;

/// Binds a payload struct to its canonical `<domain>.<verb>` wire type.
pub trait Message: Serialize + DeserializeOwned {
    const TYPE: &'static str;
}

// ---------------------------------------------------------------- session ---

pub mod session {
    use super::*;

    /// C2S — first frame on a socket; presents a realtime ticket (session.md).
    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct Authenticate {
        pub ticket: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub resume: Option<Resume>,
    }
    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct Resume {
        pub session_id: Id,
        pub last_server_seq: u32,
    }
    impl Message for Authenticate {
        const TYPE: &'static str = "session.authenticate";
    }

    /// S2C — handshake success + session parameters.
    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct Authenticated {
        pub client_seq: u32,
        pub session_id: Id,
        pub player_id: Id,
        pub resumed: bool,
        pub heartbeat_interval_ms: i32,
        pub grace_window_ms: i32,
        pub server_ts: u64,
        pub last_client_seq: u32,
    }
    impl Message for Authenticated {
        const TYPE: &'static str = "session.authenticated";
    }

    /// C2S — keepalive ping (empty payload).
    #[derive(Debug, Clone, Default, Serialize, Deserialize)]
    pub struct Heartbeat {}
    impl Message for Heartbeat {
        const TYPE: &'static str = "session.heartbeat";
    }

    /// S2C — keepalive pong.
    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct HeartbeatAck {
        pub client_seq: u32,
        pub server_ts: u64,
    }
    impl Message for HeartbeatAck {
        const TYPE: &'static str = "session.heartbeat_ack";
    }

    /// S2C — the single rejection message for any failed C2S intent.
    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct Error {
        pub code: ErrorCode,
        pub message: String,
        pub client_seq: Option<u32>,
    }
    impl Message for Error {
        const TYPE: &'static str = "session.error";
    }

    /// S2C — server-initiated close notice.
    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct Terminated {
        pub reason: TerminateReason,
        pub resumable: bool,
    }
    impl Message for Terminated {
        const TYPE: &'static str = "session.terminated";
    }
}

// --------------------------------------------------------------- movement ---

pub mod movement {
    use super::*;

    /// C2S — a movement input sample (movement-world.md).
    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct MoveIntent {
        pub input_seq: u32,
        pub move_dir: MoveDir,
        pub client_pos: Position,
    }
    #[derive(Debug, Clone, Copy, Serialize, Deserialize)]
    pub struct MoveDir {
        pub x: f64,
        pub y: f64,
    }
    impl Message for MoveIntent {
        const TYPE: &'static str = "movement.move_intent";
    }

    /// S2C — authoritative position override.
    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct PositionCorrection {
        pub position: Position,
        pub last_input_seq: u32,
    }
    impl Message for PositionCorrection {
        const TYPE: &'static str = "movement.position_correction";
    }

    /// S2C — periodic dynamic-entity snapshot in interest radius.
    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct Snapshot {
        pub server_tick: i64,
        pub entities: Vec<SnapshotEntity>,
    }
    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct SnapshotEntity {
        pub entity_id: Id,
        pub position: Position,
        pub velocity: Velocity,
        pub avatar_state: Option<String>,
        /// Elevation level this entity stands on (terraced verticality). Absent →
        /// ground level 0; old clients ignore it. The client raises the entity's
        /// render height by `level × step_height`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub level: Option<u8>,
    }
    #[derive(Debug, Clone, Copy, Serialize, Deserialize)]
    pub struct Velocity {
        pub x: f64,
        pub y: f64,
    }
    impl Message for Snapshot {
        const TYPE: &'static str = "world.snapshot";
    }
}

// ------------------------------------------------------------------ world ---

/// Static section geometry for terraced verticality (VERTICALITY-PROPOSAL.md).
/// The overworld streams in as a sequence of **sections**; each carries a coarse
/// elevation grid + the connectors (ladders/ropes/slopes) that join levels. The
/// client builds one stepped ground+cliff mesh per section and spawns the
/// connector props. Sent per initial section at run start, and again for each new
/// section the server streams in as the player advances (endless world).
pub mod world {
    use super::*;

    /// One connector joining two elevation levels.
    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct ConnectorDto {
        pub kind: String, // "slope" | "ladder" | "rope"
        pub position: Position,
        pub lo: u8,
        pub hi: u8,
        pub radius: f64,
    }

    /// S2C — one section's elevation field + connectors (+ its trail contribution
    /// for streamed sections). `levels` is row-major `levels[gx*rows + gy]`.
    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct TerrainSection {
        pub index: u32,
        pub start_x: f64,
        pub end_x: f64,
        pub y_min: f64,
        pub cell: f64,
        pub cols: u32,
        pub rows: u32,
        pub levels: Vec<u8>,
        pub connectors: Vec<ConnectorDto>,
        /// This section's clear-path waypoints, so a streamed section extends the
        /// trail. Empty for initial-chain sections (already in `run.started.path`).
        #[serde(default)]
        pub path: Vec<Position>,
    }
    impl Message for TerrainSection {
        const TYPE: &'static str = "world.terrain_section";
    }
}

// ----------------------------------------------------------------- battle ---

pub mod battle {
    use super::*;

    /// S2C — a battle subscreen opened (battle.md).
    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct Started {
        pub battle_id: Id,
        pub encounter_class: EncounterClass,
        pub allies: Vec<Combatant>,
        pub enemies: Vec<Combatant>,
        /// The first combatant this player controls (back-compat single-hero id).
        pub your_combatant_id: Id,
        /// Every combatant this player controls (a solo player fields a party of
        /// four; in co-op each player controls their one hero).
        #[serde(default)]
        pub your_combatant_ids: Vec<Id>,
        pub triggered_by: Option<Id>,
    }
    impl Message for Started {
        const TYPE: &'static str = "battle.started";
    }

    /// S2C — a second party merged into an active battle (raid merge).
    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct PartyJoined {
        pub battle_id: Id,
        pub joining_instance_id: Id,
        pub joining_allies: Vec<Combatant>,
    }
    impl Message for PartyJoined {
        const TYPE: &'static str = "battle.party_joined";
    }

    /// S2C — a combatant's gauge filled; a player's 15s window opens.
    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct TurnReady {
        pub battle_id: Id,
        pub combatant_id: Id,
        pub timeout_at: Option<u64>,
    }
    impl Message for TurnReady {
        const TYPE: &'static str = "battle.turn_ready";
    }

    /// S2C — authoritative gauge/HP sync (event-driven + 1 Hz keepalive).
    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct GaugeUpdate {
        pub battle_id: Id,
        pub server_tick: i64,
        pub combatants: Vec<GaugeEntry>,
    }
    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct GaugeEntry {
        pub combatant_id: Id,
        pub gauge: f64,
        pub hp: i32,
        pub statuses: Vec<String>,
    }
    impl Message for GaugeUpdate {
        const TYPE: &'static str = "battle.gauge_update";
    }

    /// C2S — submit the acting player's chosen action.
    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct SubmitAction {
        pub battle_id: Id,
        pub action_id: Id,
        pub action: BattleActionKind,
        /// Which of the sender's combatants is acting. Optional for back-compat
        /// (absent → the player's first/only hero).
        #[serde(default)]
        pub actor_combatant_id: Option<Id>,
        #[serde(default)]
        pub skill_kind: Option<String>,
        #[serde(default)]
        pub item_id: Option<Id>,
        #[serde(default)]
        pub target_ids: Option<Vec<Id>>,
    }
    impl Message for SubmitAction {
        const TYPE: &'static str = "battle.submit_action";
    }

    /// S2C — authoritative outcome of one resolved action.
    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct ActionResolved {
        pub battle_id: Id,
        pub action_id: Option<Id>,
        pub actor_id: Id,
        pub action: BattleActionKind,
        pub auto: bool,
        pub flee_success: Option<bool>,
        pub effects: Vec<Effect>,
    }
    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct Effect {
        pub target_id: Id,
        pub kind: EffectKind,
        pub amount: Option<i32>,
        pub status: Option<String>,
        pub hp_after: i32,
    }
    impl Message for ActionResolved {
        const TYPE: &'static str = "battle.action_resolved";
    }

    /// S2C — one party's combatants left an ongoing battle.
    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct ParticipantLeft {
        pub battle_id: Id,
        pub combatant_ids: Vec<Id>,
        pub reason: String, // "fled" | "forced_flee"
    }
    impl Message for ParticipantLeft {
        const TYPE: &'static str = "battle.participant_left";
    }

    /// S2C — terminal battle resolution for the recipient's party.
    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct Ended {
        pub battle_id: Id,
        pub outcome: BattleOutcome,
        pub xp_awards: Vec<XpAward>,
        pub loot: Vec<ItemStack>,
        /// Chits found by the recipient this encounter (economy.md S1). Banked on
        /// extraction, lost on death (it never entered circulation).
        #[serde(default)]
        pub chits_found: i64,
        /// Red-chest gear dropped to the recipient this encounter (deep fights only).
        #[serde(default)]
        pub gear_drops: Vec<LootGear>,
        pub class_emblem_drops: Vec<EmblemDrop>,
        pub gatekeeper_cleared: bool,
    }
    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct XpAward {
        pub player_id: Id,
        pub xp: i64,
        pub run_level_after: i32,
    }
    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct EmblemDrop {
        pub player_id: Id,
        pub emblem_kind: String,
    }
    impl Message for Ended {
        const TYPE: &'static str = "battle.ended";
    }
}

// -------------------------------------------------------------------- run ---

pub mod run {
    use super::*;

    /// C2S — start the party's run. Class selection is optional and back-compatible:
    /// `party` is the explicit per-hero composition from the party builder; if it is
    /// absent the server falls back to `character_class` as the party lead (building
    /// a default mixed party around it), and to Hunter if both are absent.
    #[derive(Debug, Clone, Default, Serialize, Deserialize)]
    pub struct EnterMaze {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub character_class: Option<crate::enums::CharacterClass>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub party: Option<Vec<crate::enums::CharacterClass>>,
        /// Per-slot hero names (persistent, per-account). Mirrors the player's saved
        /// roster; the server also reads/writes them via the `/v1/heroes` HTTP API.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub names: Option<Vec<String>>,
        /// Solo dive: a private instance for just the caller (no other humans).
        /// When absent/false, legacy behavior groups all waiting players (used by
        /// the headless bot tests); the co-op path is the `lobby.*` flow.
        #[serde(default)]
        pub solo: bool,
    }
    impl Message for EnterMaze {
        const TYPE: &'static str = "run.enter_maze";
    }

    /// S2C — authoritative run/instance state at entry.
    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct Started {
        pub client_seq: Option<u32>,
        pub run_id: Id,
        pub instance_id: Id,
        pub departure_hub_distance: i32,
        pub base_run_level: i32,
        pub members: Vec<Member>,
        pub backpack: Vec<ItemStack>,
        /// Chits carried in the run backpack at entry (always 0 — chits is found
        /// in the maze and banked on extraction, economy.md S1).
        #[serde(default)]
        pub chits: i64,
        /// Red-chest gear carried in the run backpack at entry (always empty at
        /// entry; grows as deep creatures drop loot).
        #[serde(default)]
        pub backpack_gear: Vec<LootGear>,
        /// Waypoints of the guaranteed clear path from the hub to the deep portal.
        /// The client draws this as a faint trail so the feasible route is legible.
        #[serde(default)]
        pub path: Vec<Position>,
        /// Walkable bounds — the client frames the map (edge cliffs/water + end
        /// walls) from these so it reads as a contained map, not an endless plain.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub bounds: Option<WorldBounds>,
        /// Biome-boundary chokepoints (a walled seam with one gap you pass through).
        #[serde(default)]
        pub seams: Vec<SeamView>,
    }
    /// Walkable extent of the instance (world-generation.md corridor bounds).
    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct WorldBounds {
        pub x_min: f64,
        pub x_max: f64,
        /// Half-height of the corridor: walkable `y ∈ [-lateral, lateral]`.
        pub lateral: f64,
    }
    /// One biome seam for the client to wall + gate.
    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct SeamView {
        pub x: f64,
        pub gap_y: f64,
        pub gap_half_width: f64,
        pub biome_from: String,
        pub biome_to: String,
    }
    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct Member {
        pub player_id: Id,
        pub username: String,
        pub character_class: CharacterClass,
        pub spawn_position: Position,
    }
    impl Message for Started {
        const TYPE: &'static str = "run.started";
    }

    /// One of the caller's heroes, for the party/roster panel: persistent name,
    /// class, level, and the four attributes at that level. Stats live here (the
    /// inventory party screen) rather than cluttering the battle HUD.
    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct HeroView {
        pub slot: i32,
        pub name: String,
        pub class_key: String,
        pub level: i32,
        pub str_: i32,
        pub mnd: i32,
        pub dex: i32,
        pub wll: i32,
        pub max_hp: i32,
        /// Formation rank: `true` = back row (halved damage, targeted less). The
        /// player sets this per hero on the party screen; defaults to the class
        /// default (casters back) until overridden. See [`SetFormation`].
        #[serde(default)]
        pub back_row: bool,
    }

    /// S2C — the caller's current party roster (sent at run start and refreshed on
    /// level-up), for the inventory party panel.
    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct Party {
        pub heroes: Vec<HeroView>,
    }
    impl Message for Party {
        const TYPE: &'static str = "run.party";
    }

    /// S2C — the party gained one or more levels this victory. Carries the
    /// before/after stats per hero so the client can play the classic JRPG
    /// "LEVEL UP!" stat-gain sequence. Sent alongside the refreshed `run.party`.
    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct LevelUp {
        pub new_run_level: i32,
        pub levels_gained: i32,
        pub heroes: Vec<HeroLevelUp>,
    }
    /// One hero's stat gains across a level-up (before → after).
    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct HeroLevelUp {
        pub slot: i32,
        pub name: String,
        pub class_key: String,
        pub level: i32,
        pub max_hp_before: i32,
        pub max_hp_after: i32,
        pub str_before: i32,
        pub str_after: i32,
        pub mnd_before: i32,
        pub mnd_after: i32,
        pub dex_before: i32,
        pub dex_after: i32,
        pub wll_before: i32,
        pub wll_after: i32,
    }
    impl Message for LevelUp {
        const TYPE: &'static str = "run.level_up";
    }

    /// C2S — harvest a resource node the avatar is standing next to. The node's
    /// `material` banks into the backpack and its `skill` gains XP (world-gen.md).
    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct Harvest {
        pub entity_id: Id,
    }
    impl Message for Harvest {
        const TYPE: &'static str = "run.harvest";
    }

    /// C2S — open a treasure chest the avatar is standing next to. Rolls loot
    /// (chits + materials + deep-enough red gear) into the backpack (economy.md S2).
    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct OpenChest {
        pub entity_id: Id,
    }
    impl Message for OpenChest {
        const TYPE: &'static str = "run.open_chest";
    }

    /// C2S — opt into the fight already in progress nearby (the avatar must be
    /// within join range of the battle). The whole of the caller's party joins the
    /// existing side; teammates are never auto-pulled in.
    #[derive(Debug, Clone, Default, Serialize, Deserialize)]
    pub struct JoinBattle {}
    impl Message for JoinBattle {
        const TYPE: &'static str = "run.join_battle";
    }

    /// C2S — rename one of the caller's heroes (persistent, per-account). Takes
    /// effect immediately (the roster is re-sent) and is saved to the account.
    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct RenameHero {
        pub slot: i32,
        pub name: String,
    }
    impl Message for RenameHero {
        const TYPE: &'static str = "run.rename_hero";
    }

    /// C2S — set one of the caller's heroes to the front (`back_row=false`) or back
    /// (`back_row=true`) row. Persistent per-account, like [`RenameHero`]: takes
    /// effect immediately (the roster is re-sent) and applies to the next/active
    /// battle's Fighter, overriding the class default.
    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct SetFormation {
        pub slot: i32,
        pub back_row: bool,
    }
    impl Message for SetFormation {
        const TYPE: &'static str = "run.set_formation";
    }

    /// C2S — start an extraction channel. `method` is `"portal"` (stand at the
    /// single deep portal) or `"town_portal"` (consume a Town Portal item, works
    /// anywhere — the primary way out).
    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct BeginExtraction {
        pub method: String, // "portal" | "town_portal"
        #[serde(default)]
        pub portal_entity_id: Option<Id>,
        #[serde(default)]
        pub item_id: Option<Id>,
    }
    impl Message for BeginExtraction {
        const TYPE: &'static str = "run.begin_extraction";
    }

    /// S2C — an extraction channel began (interruptible; visible to the instance).
    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct ChannelStarted {
        pub client_seq: Option<u32>,
        pub player_id: Id,
        pub method: String,
        pub completes_at: u64,
    }
    impl Message for ChannelStarted {
        const TYPE: &'static str = "run.channel_started";
    }

    /// S2C — an extraction channel broke before completing.
    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct ChannelInterrupted {
        pub player_id: Id,
        pub reason: String, // damage_taken | battle_started | moved | cancelled | disconnected
    }
    impl Message for ChannelInterrupted {
        const TYPE: &'static str = "run.channel_interrupted";
    }

    /// S2C — a member's run reached a terminal state.
    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct MemberResult {
        pub run_id: Id,
        pub player_id: Id,
        pub result: RunResult,
        pub max_distance_reached: i32,
        pub banked: Option<Vec<ItemStack>>,
        pub lost: Option<Vec<ItemStack>>,
        /// Chits banked (on `extracted`) or forfeited (on `died`/`abandoned`) with
        /// this run. Minted into the persistent economy only on extraction.
        #[serde(default)]
        pub chits: i64,
        /// Red-chest gear banked into the Vault on extraction (empty on death).
        #[serde(default)]
        pub gear_banked: Vec<LootGear>,
        pub durability_loss_applied: bool,
    }
    impl Message for MemberResult {
        const TYPE: &'static str = "run.member_result";
    }

    /// S2C — authoritative delta to the recipient's own backpack.
    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct BackpackUpdate {
        pub changes: Vec<BackpackChange>,
        /// Signed change to the run's chits total (economy.md S1). Positive on a
        /// loot drop, negative when chits leaves the backpack (banked/dropped).
        #[serde(default)]
        pub chits_delta: i64,
        /// Red-chest gear added to the backpack by this update (loot drops).
        #[serde(default)]
        pub gear_added: Vec<LootGear>,
    }
    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct BackpackChange {
        pub item: ItemStack,
        pub delta: String, // "added" | "removed"
        pub cause: String,
    }
    impl Message for BackpackUpdate {
        const TYPE: &'static str = "run.backpack_update";
    }
}

// -------------------------------------------------------------------- lobby ---

/// Pre-maze co-op lobby: create/join a party by code, ready up, and the host
/// starts a shared dive (everyone lands in one instance). Solo play skips this
/// entirely via `run.enter_maze { solo: true }`.
pub mod lobby {
    use super::*;

    /// C2S — create a new lobby (caller becomes host + first member).
    #[derive(Debug, Clone, Default, Serialize, Deserialize)]
    pub struct Create {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub party: Option<Vec<CharacterClass>>,
    }
    impl Message for Create {
        const TYPE: &'static str = "lobby.create";
    }

    /// C2S — join an existing lobby by its code.
    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct Join {
        pub code: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub party: Option<Vec<CharacterClass>>,
    }
    impl Message for Join {
        const TYPE: &'static str = "lobby.join";
    }

    /// C2S — toggle the caller's ready flag.
    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct Ready {
        pub ready: bool,
    }
    impl Message for Ready {
        const TYPE: &'static str = "lobby.ready";
    }

    /// C2S — leave the current lobby.
    #[derive(Debug, Clone, Default, Serialize, Deserialize)]
    pub struct Leave {}
    impl Message for Leave {
        const TYPE: &'static str = "lobby.leave";
    }

    /// C2S — host only: launch the dive with all (ready) members.
    #[derive(Debug, Clone, Default, Serialize, Deserialize)]
    pub struct Start {}
    impl Message for Start {
        const TYPE: &'static str = "lobby.start";
    }

    /// One member in a lobby (S2C view).
    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct MemberView {
        pub player_id: Id,
        pub username: String,
        pub party: Vec<CharacterClass>,
        pub ready: bool,
    }

    /// S2C — authoritative lobby state, broadcast to all members on any change.
    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct State {
        pub code: String,
        pub host_player_id: Id,
        pub members: Vec<MemberView>,
    }
    impl Message for State {
        const TYPE: &'static str = "lobby.state";
    }

    /// S2C — the lobby was disbanded (host left / everyone gone).
    #[derive(Debug, Clone, Default, Serialize, Deserialize)]
    pub struct Closed {}
    impl Message for Closed {
        const TYPE: &'static str = "lobby.closed";
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::envelope::Envelope;

    #[test]
    fn authenticate_type_string_matches_canon() {
        assert_eq!(session::Authenticate::TYPE, "session.authenticate");
        assert_eq!(battle::SubmitAction::TYPE, "battle.submit_action");
        assert_eq!(run::EnterMaze::TYPE, "run.enter_maze");
    }

    #[test]
    fn terrain_section_round_trips() {
        let json = r#"{"type":"world.terrain_section","seq":5,"ts":1,"payload":{"index":2,"start_x":40.0,"end_x":72.0,"y_min":-28.0,"cell":2.0,"cols":16,"rows":28,"levels":[0,1,1],"connectors":[{"kind":"ladder","position":{"x":50.0,"y":-6.0},"lo":0,"hi":1,"radius":2.2}],"path":[{"x":40.0,"y":0.0},{"x":72.0,"y":3.0}]}}"#;
        let env: Envelope<world::TerrainSection> = serde_json::from_str(json).unwrap();
        assert_eq!(env.payload.index, 2);
        assert_eq!(env.payload.connectors[0].kind, "ladder");
        assert_eq!(env.payload.levels, vec![0, 1, 1]);
        // Round-trips back out.
        let s = serde_json::to_string(&env.payload).unwrap();
        let back: world::TerrainSection = serde_json::from_str(&s).unwrap();
        assert_eq!(back.cols, 16);
        assert_eq!(back.connectors.len(), 1);
    }

    #[test]
    fn snapshot_entity_level_is_optional_and_defaults() {
        // Old wire (no `level`) still decodes; absent → None.
        let json = r#"{"entity_id":"m","position":{"x":1.0,"y":2.0},"velocity":{"x":0.0,"y":0.0},"avatar_state":"active"}"#;
        let e: movement::SnapshotEntity = serde_json::from_str(json).unwrap();
        assert_eq!(e.level, None);
    }

    #[test]
    fn submit_action_round_trips_against_spec_example() {
        // battle.md example, wrapped in the envelope.
        let json = r#"{"type":"battle.submit_action","seq":310,"ts":1783728115000,"payload":{"battle_id":"b","action_id":"a","action":"attack","skill_kind":null,"item_id":null,"target_ids":["t"]}}"#;
        let env: Envelope<battle::SubmitAction> = serde_json::from_str(json).unwrap();
        assert_eq!(env.payload.action, BattleActionKind::Attack);
        assert_eq!(env.payload.target_ids.as_ref().unwrap()[0], "t");
    }
}
