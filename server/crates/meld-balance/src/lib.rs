//! Typed loader for `balance/balance.toml` — every `[TUNABLE]` constant
//! (CANON.md §B; working agreement #2: no gameplay literal lives in code).
//!
//! The server loads this once at boot and threads `&Balance` into the world,
//! run, and battle systems. Changing a tunable is a one-line config edit + a
//! reboot, never a code change.

use serde::Deserialize;

#[derive(Debug, thiserror::Error)]
pub enum BalanceError {
    #[error("reading balance file {0}: {1}")]
    Io(String, std::io::Error),
    #[error("parsing balance toml: {0}")]
    Parse(#[from] toml::de::Error),
}

#[derive(Debug, Clone, Deserialize)]
pub struct Balance {
    pub session: Session,
    pub auth: Auth,
    pub world: World,
    pub runs: Runs,
    pub battle: Battle,
    pub meld: Meld,
    pub combat_math: CombatMath,
    pub world_scaling: WorldScaling,
    pub creature: Creatures,
    pub player: Players,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Session {
    pub heartbeat_interval_ms: i32,
    pub grace_window_ms: i32,
    pub auth_timeout_ms: i32,
    pub realtime_ticket_ttl_ms: i64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Auth {
    pub bcrypt_cost: u32,
    pub session_token_ttl_secs: i32,
}

#[derive(Debug, Clone, Deserialize)]
pub struct World {
    pub chunk_size: i32,
    pub interest_radius_chunks: i32,
    pub overworld_sim_hz: u64,
    pub snapshot_hz: u64,
    pub touch_radius_tiles: f64,
    pub interaction_radius_tiles: f64,
    pub avatar_speed_tiles_per_sec: f64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Runs {
    pub base_run_level_per_distance: f64,
    pub backpack_slots: i32,
    pub extraction_channel_ms: u64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Battle {
    pub tick_ms: u64,
    pub gauge_fill_divisor: f64,
    pub turn_timeout_ms: u64,
    pub flee_base: f64,
    pub flee_penalty_per_tier: f64,
    pub flee_floor: f64,
    pub merge_cap_normal_instances: i32,
    pub merge_cap_gatekeeper_instances: i32,
    pub defend_damage_reduction: f64,
    pub skill_power_mult: f64,
    pub skill_heal_fraction: f64,
    pub item_heal_fraction: f64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Meld {
    pub xp_per_level: i64,
    pub alchemy_xp_per_extracted_stack: i64,
    pub forging_xp_per_craft: i64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CombatMath {
    pub min_damage: i32,
}

#[derive(Debug, Clone, Deserialize)]
pub struct WorldScaling {
    pub tier_divisor: f64,
    pub mlevel_divisor: f64,
    pub stat_mult_base_divisor: f64,
    pub stat_mult_exp: f64,
    pub red_chest_floor_distance: i64,
}

/// Content-ish stat blocks. Keyed by content id (e.g. `forest_bloom_stalker`).
pub type Creatures = std::collections::HashMap<String, CreatureStats>;
pub type Players = std::collections::HashMap<String, PlayerStats>;

#[derive(Debug, Clone, Deserialize)]
pub struct CreatureStats {
    pub base_hp: i32,
    pub base_atk: i32,
    pub base_def: i32,
    pub speed_stat: i32,
    pub xp_reward: i64,
    pub encounter_class: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PlayerStats {
    pub base_hp: i32,
    pub base_atk: i32,
    pub base_def: i32,
    pub speed_stat: i32,
}

impl Balance {
    /// Parse a balance TOML string.
    pub fn from_toml_str(s: &str) -> Result<Self, BalanceError> {
        Ok(toml::from_str(s)?)
    }

    /// Load from a path on disk.
    pub fn load(path: &str) -> Result<Self, BalanceError> {
        let text =
            std::fs::read_to_string(path).map_err(|e| BalanceError::Io(path.to_string(), e))?;
        Self::from_toml_str(&text)
    }

    /// Load from the checked-in default location, resolved relative to the
    /// workspace root (`balance/balance.toml`). Falls back to `MELD_BALANCE`.
    pub fn load_default() -> Result<Self, BalanceError> {
        if let Ok(p) = std::env::var("MELD_BALANCE") {
            return Self::load(&p);
        }
        // CARGO_MANIFEST_DIR of this crate is server/crates/meld-balance.
        let root = concat!(env!("CARGO_MANIFEST_DIR"), "/../../../balance/balance.toml");
        Self::load(root)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_balance_parses_and_has_creature() {
        let b = Balance::load_default().expect("balance.toml parses");
        assert_eq!(b.battle.tick_ms, 100);
        assert_eq!(b.auth.bcrypt_cost, 12);
        assert!(b.creature.contains_key("forest_bloom_stalker"));
        assert!(b.player.contains_key("squire"));
    }
}
