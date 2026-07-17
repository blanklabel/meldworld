//! Realtime C2S/S2C message payloads, grouped by domain
//! (interfaces/realtime-protocol.md and its detail files).
//!
//! Each payload struct binds to its wire `type` string via [`Message::TYPE`],
//! so the gateway can peek a [`crate::RawEnvelope`], match the string, and
//! decode into the right struct. Only the subset the today-slice uses is
//! modelled; the rest land as their systems do.

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::common::{Combatant, ItemStack, Position};
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
    /// a default mixed party around it), and to Squire if both are absent.
    #[derive(Debug, Clone, Default, Serialize, Deserialize)]
    pub struct EnterMaze {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub character_class: Option<crate::enums::CharacterClass>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub party: Option<Vec<crate::enums::CharacterClass>>,
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

    /// C2S — start an extraction channel (portal or escape item).
    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct BeginExtraction {
        pub method: String, // "portal" | "escape_item"
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
        pub durability_loss_applied: bool,
    }
    impl Message for MemberResult {
        const TYPE: &'static str = "run.member_result";
    }

    /// S2C — authoritative delta to the recipient's own backpack.
    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct BackpackUpdate {
        pub changes: Vec<BackpackChange>,
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
    fn submit_action_round_trips_against_spec_example() {
        // battle.md example, wrapped in the envelope.
        let json = r#"{"type":"battle.submit_action","seq":310,"ts":1783728115000,"payload":{"battle_id":"b","action_id":"a","action":"attack","skill_kind":null,"item_id":null,"target_ids":["t"]}}"#;
        let env: Envelope<battle::SubmitAction> = serde_json::from_str(json).unwrap();
        assert_eq!(env.payload.action, BattleActionKind::Attack);
        assert_eq!(env.payload.target_ids.as_ref().unwrap()[0], "t");
    }
}
