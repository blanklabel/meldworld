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

/// A piece of gear carried as run loot (gear-item-models.md `GearItem`). Rides
/// the backpack/loot wire so the client can show looted red-chest gear before it
/// is banked (extraction converts it to owned Vault gear). Only the fields the
/// slice's HUD/inventory needs; sockets/gems land with their systems.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LootGear {
    pub gear_id: Id,
    pub name: String,
    /// Equipment slot key (content-defined opaque string, e.g. `weapon`).
    pub slot: String,
    pub insurance: Insurance,
    /// Loot tier band at generation: `tier(d) = floor(d / 100)`.
    pub tier: i32,
    /// Flat physical-attack bonus granted while equipped (weapon slot).
    pub atk_bonus: i32,
    /// Flat defence bonus granted while equipped (armor slot).
    #[serde(default)]
    pub def_bonus: i32,
    /// Flat ATB-speed bonus granted while equipped (accessory slot).
    #[serde(default)]
    pub spd_bonus: i32,
    pub base_max_durability: i32,
    pub max_durability: i32,
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
