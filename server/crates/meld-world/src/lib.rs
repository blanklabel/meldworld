//! Overworld model for the spike (behaviors/world-generation.md subset).
//!
//! The full spec is an infinite seeded radial plane with 64×64 chunk streaming,
//! biomes, chokepoints and Gatekeeper arenas. The today-slice needs only enough
//! overworld to get a party to *touch one monster*: a bounded Forest arena
//! around the Center Hub, server-authoritative movement integration, and touch
//! detection. Chunk streaming, biomes past Forest, and Gatekeepers are deferred
//! (next slices) — the scaling formulas that combat depends on are implemented
//! now so difficulty is correct where the monster stands.

use meld_balance::Balance;
use meld_proto::common::Position;
use meld_proto::Id;

/// Distance → difficulty formulas (world-generation.md). Structure in code;
/// coefficients from balance.
pub struct Scaling<'a> {
    b: &'a Balance,
}

impl<'a> Scaling<'a> {
    pub fn new(b: &'a Balance) -> Self {
        Self { b }
    }

    /// `tier(d) = floor(d / 100)`.
    pub fn tier(&self, d: i64) -> i64 {
        (d as f64 / self.b.world_scaling.tier_divisor).floor() as i64
    }

    /// `mlevel(d) = max(1, round(d / 12.5))`.
    pub fn mlevel(&self, d: i64) -> i32 {
        let m = (d as f64 / self.b.world_scaling.mlevel_divisor).round() as i32;
        m.max(1)
    }

    /// `stat_mult(d) = (1 + d/500)^1.25` for `d ≤ 5000` (exponential past that;
    /// the endgame branch lands with the infinite-zone slice).
    pub fn stat_mult(&self, d: i64) -> f64 {
        let base = 1.0 + d as f64 / self.b.world_scaling.stat_mult_base_divisor;
        base.powf(self.b.world_scaling.stat_mult_exp)
    }
}

/// A monster placed in the overworld arena.
#[derive(Debug, Clone)]
pub struct MonsterSpawn {
    pub entity_id: Id,
    pub monster_kind: String,
    pub position: Position,
    pub level: i32,
    pub encounter_class: String,
    /// World-scaled combat stats (stat_mult applied at spawn — no rescale later).
    pub hp: i32,
    pub atk: i32,
    pub def: i32,
    pub speed_stat: i32,
    pub xp_reward: i64,
    /// Set once this monster's battle has started, so we don't re-trigger.
    pub engaged: bool,
    pub defeated: bool,
}

/// A player avatar on the overworld.
#[derive(Debug, Clone)]
pub struct Avatar {
    pub player_id: Id,
    pub position: Position,
    /// `active` | `in_battle` | `channeling` | `sleeping`.
    pub state: String,
    pub last_input_seq: u32,
    pub max_speed_tiles_per_sec: f64,
}

/// The bounded arena for one MazeInstance (spike scope).
pub struct Arena {
    pub half_extent: f64,
    pub avatars: Vec<Avatar>,
    pub monster: MonsterSpawn,
    /// The extraction portal (deterministic at every hub — CANON.md D15).
    pub portal: Position,
    touch_radius: f64,
    interaction_radius: f64,
    sim_dt: f64,
}

impl Arena {
    /// Build a Forest arena with the party spawned near the Center Hub and one
    /// monster placed a short walk away (still Forest band, d < 100).
    pub fn new(balance: &Balance, party: &[(Id, f64)], monster_id: Id) -> Self {
        // Spawn the monster ~10 tiles out (Forest, d≈10).
        let monster_pos = Position::new(10.0, 0.0);
        let d = monster_pos.distance_floor();
        let scaling = Scaling::new(balance);
        let stats = balance
            .creature
            .get("forest_bloom_stalker")
            .expect("forest_bloom_stalker in balance.toml");
        let mult = scaling.stat_mult(d);

        let monster = MonsterSpawn {
            entity_id: monster_id,
            monster_kind: "forest_bloom_stalker".to_string(),
            position: monster_pos,
            level: scaling.mlevel(d),
            encounter_class: stats.encounter_class.clone(),
            hp: ((stats.base_hp as f64) * mult).round() as i32,
            atk: ((stats.base_atk as f64) * mult).round() as i32,
            def: stats.base_def,
            speed_stat: stats.speed_stat,
            xp_reward: stats.xp_reward,
            engaged: false,
            defeated: false,
        };

        // Party spawns in a small arc near the origin.
        let avatars = party
            .iter()
            .enumerate()
            .map(|(i, (pid, speed))| Avatar {
                player_id: pid.clone(),
                position: Position::new(0.0, (i as f64 - party.len() as f64 / 2.0) * 1.5),
                state: "active".to_string(),
                last_input_seq: 0,
                max_speed_tiles_per_sec: *speed,
            })
            .collect();

        Arena {
            half_extent: 64.0,
            avatars,
            monster,
            // A short walk east past where the monster stands (d≈14, Forest).
            portal: Position::new(14.0, 0.0),
            touch_radius: balance.world.touch_radius_tiles,
            interaction_radius: balance.world.interaction_radius_tiles,
            sim_dt: 1.0 / balance.world.overworld_sim_hz as f64,
        }
    }

    /// Is `player` within interaction range of the extraction portal?
    pub fn at_portal(&self, player_id: &str) -> bool {
        self.avatar(player_id)
            .map(|a| a.position.distance_to(&self.portal) <= self.interaction_radius)
            .unwrap_or(false)
    }

    pub fn avatar(&self, player_id: &str) -> Option<&Avatar> {
        self.avatars.iter().find(|a| a.player_id == player_id)
    }

    pub fn avatar_mut(&mut self, player_id: &str) -> Option<&mut Avatar> {
        self.avatars.iter_mut().find(|a| a.player_id == player_id)
    }

    /// Integrate one movement intent against authoritative position, clamped to
    /// arena bounds and max speed (server owns movement — CANON.md §S, D11).
    /// Returns the authoritative position after integration.
    pub fn apply_move(
        &mut self,
        player_id: &str,
        dir_x: f64,
        dir_y: f64,
        input_seq: u32,
    ) -> Option<Position> {
        let dt = self.sim_dt;
        let half_extent = self.half_extent;
        let a = self.avatar_mut(player_id)?;
        if a.state != "active" {
            return Some(a.position); // can't move while in battle/channeling/sleeping
        }
        // Clamp direction magnitude to ≤ 1 (movement-world.md).
        let mag = (dir_x * dir_x + dir_y * dir_y).sqrt();
        let (nx, ny) = if mag > 1.0 {
            (dir_x / mag, dir_y / mag)
        } else {
            (dir_x, dir_y)
        };
        let step = a.max_speed_tiles_per_sec * dt;
        let clamp = |v: f64, h: f64| v.max(-h).min(h);
        a.position.x = clamp(a.position.x + nx * step, half_extent);
        a.position.y = clamp(a.position.y + ny * step, half_extent);
        a.last_input_seq = input_seq;
        Some(a.position)
    }

    /// Any active avatar within touch range of the (undefeated, unengaged)
    /// monster? Returns the touching player's id — the battle trigger.
    pub fn check_touch(&self) -> Option<Id> {
        if self.monster.engaged || self.monster.defeated {
            return None;
        }
        self.avatars
            .iter()
            .find(|a| {
                a.state == "active"
                    && a.position.distance_to(&self.monster.position) <= self.touch_radius
            })
            .map(|a| a.player_id.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scaling_matches_canon_examples() {
        let b = Balance::load_default().unwrap();
        let s = Scaling::new(&b);
        assert_eq!(s.tier(99), 0);
        assert_eq!(s.tier(100), 1);
        assert_eq!(s.mlevel(500), 40);
        assert_eq!(s.mlevel(0), 1);
        assert!((s.stat_mult(0) - 1.0).abs() < 1e-9);
    }

    #[test]
    fn walking_toward_monster_eventually_touches() {
        let b = Balance::load_default().unwrap();
        let mut arena = Arena::new(&b, &[("p1".into(), 6.0)], "m1".into());
        assert!(arena.check_touch().is_none());
        // Walk east toward the monster at x=10 for up to 5 s of sim ticks.
        let mut touched = None;
        for i in 0..(20 * 5) {
            arena.apply_move("p1", 1.0, 0.0, i + 1);
            if let Some(p) = arena.check_touch() {
                touched = Some(p);
                break;
            }
        }
        assert_eq!(touched, Some("p1".to_string()));
    }
}
