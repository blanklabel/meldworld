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
    pub loot: Loot,
    pub meld: Meld,
    pub combat_math: CombatMath,
    pub world_scaling: WorldScaling,
    pub worldgen: WorldGen,
    pub ai: Ai,
    pub attributes: Attributes,
    pub creature: Creatures,
    pub player: Players,
    pub resource: Resources,
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
    /// Level curve: `xp_to_next(L) = xp_base * xp_growth_factor^(L-1)`.
    pub xp_base: i64,
    pub xp_growth_factor: f64,
    /// Town Portal item economy (extraction is mostly this item now).
    pub starting_town_portals: i32,
    pub town_portal_drop_chance: f64,
}

/// How the four attributes (Str/Mnd/Dex/Wll) map to combat stats. See the
/// `[attributes]` block in balance.toml for the meaning of each coefficient.
#[derive(Debug, Clone, Deserialize)]
pub struct Attributes {
    pub str_to_atk: f64,
    pub mnd_to_power: f64,
    pub dex_to_speed: f64,
    pub wll_to_hp: f64,
    pub wll_to_def: f64,
    pub dodge_dex_floor: i32,
    pub dodge_per_dex: f64,
    pub dodge_cap: f64,
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
    pub party_size_per_player: usize,
    pub skill_power_mult: f64,
    pub skill_heal_fraction: f64,
    pub item_heal_fraction: f64,
    pub psyker_focus_base: usize,
    pub psyker_focus_per_level: i32,
    pub psyker_focus_cap: usize,
    pub psyker_gravity_tick_mult: f64,
    pub psyker_spike_tick_mult: f64,
    pub psyker_aegis_tick_fraction: f64,
    pub psyker_anchor_gauge_drain: f64,
    pub barrier_decay_per_turn: i32,
    pub resonant_regen_per_turn: i32,
    pub resonant_transfuse_heal_fraction: f64,
    pub resonant_transfuse_cost_fraction: f64,
    pub resonant_boon_regen: i32,
    pub resonant_ward_barrier_fraction: f64,
}

/// Creature loot tunables (economy.md sources S1). See the `[loot]` block in
/// balance.toml. Chits + biome material + red-chest gear on a felled encounter.
#[derive(Debug, Clone, Deserialize)]
pub struct Loot {
    pub chits_per_mlevel: f64,
    pub chits_jitter: f64,
    pub gear_drop_chance: f64,
    pub gear_atk_per_tier: f64,
    pub gear_atk_jitter: f64,
    pub gear_base_durability: i32,
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

/// Procedural area-generation tunables (world-generation.md subset).
#[derive(Debug, Clone, Deserialize)]
pub struct WorldGen {
    pub area_count: usize,
    pub base_area_length: f64,
    pub area_length_growth: f64,
    pub area_length_jitter: f64,
    pub monster_spacing: f64,
    pub monster_spacing_jitter: f64,
    pub lateral_jitter: f64,
    pub first_monster_x: f64,
    pub first_area_portal_gap: f64,
    pub portal_setback: f64,
    pub world_margin: f64,
    pub lateral_half_extent: f64,
    pub creature_lateral_spread: f64,
    pub resources_per_area: f64,
    pub resource_lateral_spread: f64,
    pub obstacles_per_area: f64,
    /// Forest sections pack this multiple of `obstacles_per_area` extra trees into
    /// the play area (dense maze); other biomes keep the base density.
    pub forest_obstacle_mult: f64,
    /// Non-forest biomes pack this multiple of `obstacles_per_area` extra props for
    /// their maze fill (every biome is a maze; forest uses `forest_obstacle_mult`).
    pub maze_obstacle_mult: f64,
    pub obstacle_min_radius: f64,
    pub obstacle_max_radius: f64,
    pub path_clear_radius: f64,
    pub path_meander: f64,
    pub player_radius: f64,
    // --- Verticality (terraces + connectors), VERTICALITY-PROPOSAL.md. ---
    /// Avg raised terraces per procedural section (area 0 stays flat).
    pub terraces_per_area: f64,
    /// Highest elevation level a terrace can reach (0 = ground).
    pub max_level: u8,
    /// Smallest terrace footprint side (tiles).
    pub terrace_min_size: f64,
    /// Largest terrace footprint side (tiles).
    pub terrace_max_size: f64,
    /// Grid resolution of the elevation field (tiles/cell).
    pub terrain_cell: f64,
    /// Reach around a connector (ladder/rope/slope) that permits a level change.
    pub connector_radius: f64,
    /// How far ahead of the frontier player the world streams new sections in.
    pub stream_lookahead: f64,
    /// Probability a procedural section's CLEAR PATH climbs onto a mid-segment
    /// plateau (up a ramp, across, back down) — the "path itself is a maze" knob.
    /// Endpoints stay on level 0, so feasibility is preserved.
    pub path_climb_chance: f64,
    /// Probability a section's treasure chest sits ON TOP of a raised terrace (at
    /// that elevation) instead of on the ground — treasure that rewards a climb.
    pub chest_terrace_chance: f64,
}

/// Creature AI tunables (overworld movement + encounter grouping).
#[derive(Debug, Clone, Deserialize)]
pub struct Ai {
    pub wander_speed: f64,
    pub chase_speed: f64,
    pub aggro_radius: f64,
    pub territorial_aggro_radius: f64,
    pub leash_radius: f64,
    pub group_radius: f64,
    pub flee_hp_fraction: f64,
    pub join_radius: f64,
    /// Overworld creature-vs-creature skirmish: hostile-faction creatures hunt
    /// each other within this range, trade blows once `skirmish_attack_range`
    /// close, on a `skirmish_attack_interval`-second cadence.
    pub skirmish_aggro_radius: f64,
    pub skirmish_attack_range: f64,
    pub skirmish_attack_interval: f64,
    /// A player auto-collects a ground-loot drop within this range.
    pub loot_pickup_radius: f64,
}

/// Content-ish stat blocks. Keyed by content id (e.g. `forest_bloom_stalker`).
pub type Creatures = std::collections::HashMap<String, CreatureStats>;
pub type Players = std::collections::HashMap<String, PlayerStats>;
pub type Resources = std::collections::HashMap<String, ResourceStats>;

/// A harvestable resource node's content, keyed by node id (e.g. `bloom_herb`).
#[derive(Debug, Clone, Deserialize)]
pub struct ResourceStats {
    /// Item kind banked into the backpack when harvested (feeds crafting).
    pub material: String,
    /// Meld skill credited on harvest (`forging` | `alchemy`).
    pub skill: String,
    /// Skill XP granted per harvest.
    pub xp: i64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CreatureStats {
    pub base_hp: i32,
    pub base_atk: i32,
    pub base_def: i32,
    pub speed_stat: i32,
    pub xp_reward: i64,
    pub encounter_class: String,
    /// Faction (for grouping + hostility). See meld-proto::factions.
    pub faction: String,
    /// `passive` | `territorial` | `aggressive` — overworld movement style.
    pub aggression: String,
    /// Whether this creature flees a losing battle.
    #[serde(default)]
    pub flees: bool,
    /// Item kind dropped as ground loot when this creature is felled by an
    /// overworld skirmish (players walk over it to collect).
    #[serde(default = "default_loot_kind")]
    pub loot_kind: String,
}

fn default_loot_kind() -> String {
    "monster_trophy".to_string()
}

#[derive(Debug, Clone, Deserialize)]
pub struct PlayerStats {
    pub base_hp: i32,
    pub base_atk: i32,
    pub base_def: i32,
    pub speed_stat: i32,
    /// Level-1 attribute baseline.
    pub str: i32,
    pub mnd: i32,
    pub dex: i32,
    pub wll: i32,
    /// Attribute points auto-gained per level (the class's growth focus).
    pub str_per_level: i32,
    pub mnd_per_level: i32,
    pub dex_per_level: i32,
    pub wll_per_level: i32,
}

impl PlayerStats {
    /// The four attributes at `level` = baseline + per-level gain × (level-1).
    /// Returns `(str, mnd, dex, wll)`.
    pub fn attributes_at(&self, level: i32) -> (i32, i32, i32, i32) {
        let steps = (level - 1).max(0);
        (
            self.str + self.str_per_level * steps,
            self.mnd + self.mnd_per_level * steps,
            self.dex + self.dex_per_level * steps,
            self.wll + self.wll_per_level * steps,
        )
    }
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

    /// The default `balance.toml`, embedded at compile time. This is what makes a
    /// shipped binary (e.g. the self-contained QA build) self-sufficient: it needs
    /// no `balance/balance.toml` on disk beside it. `load_default` still prefers a
    /// live file when one is present, so in-repo runs pick up local tweaks.
    pub const EMBEDDED_DEFAULT: &'static str =
        include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../../balance/balance.toml"));

    /// Load the balance table, in priority order:
    /// 1. `MELD_BALANCE` env → that file (explicit override).
    /// 2. The checked-in `balance/balance.toml`, if it exists on disk (in-repo runs).
    /// 3. Otherwise the [`Self::EMBEDDED_DEFAULT`] baked into the binary, so a
    ///    standalone binary works with no config file present.
    pub fn load_default() -> Result<Self, BalanceError> {
        if let Ok(p) = std::env::var("MELD_BALANCE") {
            return Self::load(&p);
        }
        // CARGO_MANIFEST_DIR of this crate is server/crates/meld-balance.
        let root = concat!(env!("CARGO_MANIFEST_DIR"), "/../../../balance/balance.toml");
        if std::path::Path::new(root).exists() {
            return Self::load(root);
        }
        Self::from_toml_str(Self::EMBEDDED_DEFAULT)
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
