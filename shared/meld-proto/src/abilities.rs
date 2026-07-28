//! Monster ability schema (Creature AI/Combat/Gear spec §1).
//!
//! A `MonsterDefinition` carries a pool of [`MonsterAbility`] options; the
//! server filters the pool at runtime (level gate, cooldown, HP threshold),
//! rolls a weighted choice, and resolves each [`AbilityEffect`] in order.
//! Lives in `meld-proto` so `meld-world` (content) and `meld-battle`
//! (execution) share one schema without depending on each other.

use serde::{Deserialize, Serialize};

use crate::enums::DamageType;

/// What an [`AbilityEffect`] mechanically does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AbilityEffectKind {
    Damage,
    Heal,
    AtbManipulation,
    Status,
    Steal,
}

/// Which runtime stat a damage/heal/ATB effect scales from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScalingBase {
    Attack,
    Magic,
    Level,
    MaxHp,
}

/// Who an effect lands on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AbilityTarget {
    SingleEnemy,
    AllEnemies,
    MonsterGroup,
    #[serde(rename = "self")]
    SelfCast,
    AllAllies,
}

/// What a `steal` effect takes from the victim's run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StealTargetKind {
    Chits,
    Consumable,
    Material,
}

/// One mechanical consequence of a [`MonsterAbility`], resolved in order.
/// All numbers scale dynamically off the monster's *runtime* (world-scaled)
/// stats — `stats[scaling_base] × coefficient`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AbilityEffect {
    pub effect_kind: AbilityEffectKind,
    /// Required for damage/heal/ATB math; `None` for status/steal.
    #[serde(default)]
    pub scaling_base: Option<ScalingBase>,
    /// Multiplier applied to `scaling_base` (1.5 = 150%).
    #[serde(default)]
    pub coefficient: Option<f64>,
    /// The [`DamageType`] of the effect. Required if `effect_kind` is damage.
    #[serde(default)]
    pub damage_type: Option<DamageType>,
    pub target: AbilityTarget,
    /// Required for status (e.g. `poison`).
    #[serde(default)]
    pub status_name: Option<String>,
    /// Required for status — duration in 100 ms ATB ticks.
    #[serde(default)]
    pub duration_ticks: Option<i32>,
    /// Required for steal.
    #[serde(default)]
    pub steal_target_kind: Option<StealTargetKind>,
}

/// One option in a monster's ability pool, attached to its definition.
/// Pools are permanent per creature kind but expand with level via
/// `min_level` (a L1 spider knows `web`; the L99 one also has `poisonado`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MonsterAbility {
    /// Content identifier (e.g. `fire_breath`, `web_bind`).
    pub ability_kind: String,
    /// Text for the client UI shout bubble (e.g. "Flame Burst!").
    pub callout_text: String,
    /// Relative probability weight for AI selection.
    pub weight: i32,
    /// 100 ms ATB ticks before reuse.
    pub cooldown_ticks: i32,
    /// If > 0, the monster shouts but delays execution (channeling).
    pub telegraph_ticks: i32,
    /// If set, ability is only in the pool while HP ≤ this fraction (0.0–1.0).
    #[serde(default)]
    pub hp_threshold_pct: Option<f64>,
    /// Level gate — the ability joins the pool once the monster's level
    /// reaches this (the "pool grows with level" mechanic).
    #[serde(default)]
    pub min_level: i32,
    /// Ordered list of mechanical consequences.
    pub effects: Vec<AbilityEffect>,
}
