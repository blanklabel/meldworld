//! Canonical enums (CANON.md §G glossary). Wire form is snake_case (CANON.md §I).

use serde::{Deserialize, Serialize};

/// Character classes (CANON.md §G `CharacterClass`, D9). `hunter` is the default —
/// the martial baseline that builds Adrenaline with basic attacks and spends it on
/// its skills (see `Battle::resolve_skill`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CharacterClass {
    /// Martial baseline / default. Basic attacks bank Adrenaline; skills (Power
    /// Strike, Second Wind, Snare, Frenzy) spend it. The
    /// disposal-of-dangerous-creatures guild.
    Hunter,
    Dragoon,
    Sage,
    Ranger,
    AlchemistKnight,
    Bard,
    /// Psychic controller: armour-ignoring psychic strikes + projected wards.
    Psyker,
    /// Healer: spends its own HP to mend allies, grants Regen + Barrier.
    Resonant,
    /// Rogue / fortune-hunter ("Runner"): fast, fragile, evasive. Armour-piercing
    /// Backstab, a Flicker evasion blink, and Ransack (damage + ATB-gauge steal).
    Shifter,
    /// Order of the Iron Hull monk: a dense, slow front-line tank. Blunt kinetic
    /// strikes that stagger (drain the enemy's ATB gauge), a Root stance that
    /// grants Barrier, and Toll of the Deep — an all-enemy shockwave.
    IronHull,
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
