//! Shared payload objects referenced across realtime domains
//! (realtime-protocol.md §Common Payload Objects).

use serde::{Deserialize, Serialize};

use crate::enums::{CombatantKind, Insurance};
use crate::Id;

/// A tile-space coordinate from the world origin (Center Hub). Y grows south.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Position {
    pub x: f64,
    pub y: f64,
}

impl Position {
    pub fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }

    /// Euclidean distance to another position, in tile units.
    pub fn distance_to(&self, other: &Position) -> f64 {
        let dx = self.x - other.x;
        let dy = self.y - other.y;
        (dx * dx + dy * dy).sqrt()
    }

    /// Floored Euclidean distance from the world origin — the value all
    /// threshold checks use (CANON.md §G "Distance").
    pub fn distance_floor(&self) -> i64 {
        (self.x * self.x + self.y * self.y).sqrt().floor() as i64
    }
}

/// A backpack/loot item stack (realtime-protocol.md ItemStack).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ItemStack {
    pub item_id: Id,
    pub item_kind: String,
    pub quantity: i32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub insurance: Option<Insurance>,
}

/// A battle actor's public state (realtime-protocol.md Combatant).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Combatant {
    pub combatant_id: Id,
    pub kind: CombatantKind,
    pub player_id: Option<Id>,
    pub monster_kind: Option<String>,
    pub level: i32,
    pub hp: i32,
    pub max_hp: i32,
    pub gauge: f64,
    pub statuses: Vec<String>,
}
