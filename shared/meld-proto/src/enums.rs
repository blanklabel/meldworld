//! Canonical enums (CANON.md §G glossary). Wire form is snake_case (CANON.md §I).

use serde::{Deserialize, Serialize};

/// Character classes (CANON.md §G `CharacterClass`, D9). `squire` is the default.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CharacterClass {
    Squire,
    Dragoon,
    Sage,
    Ranger,
    AlchemistKnight,
    Bard,
}

/// A combatant's category, deciding friend-vs-foe and disconnect rules.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CombatantKind {
    Player,
    Monster,
    GatekeeperBoss,
}

/// Encounter classification (realtime battle.md). Drives flee + disconnect rules.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EncounterClass {
    Standard,
    Elite,
    Gatekeeper,
}

/// Gear insurance tier (CANON.md §G).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Insurance {
    Blue,
    Red,
}

/// A battle action a player may submit (realtime battle.md).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BattleActionKind {
    Attack,
    Skill,
    Item,
    Defend,
    Flee,
}

/// Terminal result of one combatant's battle (realtime battle.md).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BattleOutcome {
    Victory,
    Defeat,
    Fled,
}

/// The kind of a per-target effect inside `battle.action_resolved`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EffectKind {
    Damage,
    Heal,
    StatusApplied,
    StatusRemoved,
    Ko,
    Revive,
}

/// Terminal state of a `Run` (CANON.md §G).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunResult {
    Extracted,
    Died,
    Abandoned,
}

/// Realtime rejection codes (realtime-protocol.md common rejection table).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    ValidationError,
    Unauthorized,
    Forbidden,
    NotFound,
    InvalidState,
    OutOfRange,
    DuplicateAction,
    SequenceError,
    ResumeFailed,
    RateLimitExceeded,
    Internal,
}

/// Reason a server-initiated `session.terminated` closes a socket.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TerminateReason {
    ReplacedByNewConnection,
    AuthTimeout,
    IdleTimeout,
    ServerShutdown,
    ProtocolViolation,
}
