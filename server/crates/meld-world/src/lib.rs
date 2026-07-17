//! Overworld model for the spike (behaviors/world-generation.md subset).
//!
//! The full spec is an infinite seeded radial plane with 64×64 chunk streaming,
//! biomes, chokepoints and Gatekeeper arenas. This slice implements the part
//! that makes the loop feel like a *world*: a per-instance **seeded chain of
//! biome areas** marching east from the Center Hub. Each area has its own
//! length (jittered, trending larger with depth), several creatures placed
//! along the corridor and scaled by their own distance, and an extraction
//! portal near its end. Distance → difficulty uses the canon `tier/mlevel/
//! stat_mult` formulas so deeper creatures are correctly harder.
//!
//! Deferred to later slices (documented, not lost): true 2D chunk streaming,
//! Gatekeeper arenas, chokepoint geometry, and the infinite zone past d=5000.
//! The world here is a wide corridor (movement along +x is "distance"; a narrow
//! ±y band lets players stray) rather than an open plane — enough to march
//! through many procedurally-sized areas and fight a variety of creatures.

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

/// Fixed biome band for a floored distance (world-generation.md Biome Bands).
/// Structural order; bands past Mire repeat Mire content in this slice.
pub fn biome_for_distance(d: i64) -> &'static str {
    match d {
        0..=99 => "forest",
        100..=299 => "desert",
        300..=499 => "ashfall",
        500..=999 => "tundra",
        _ => "mire",
    }
}

/// Creature content ids that spawn in a biome. Structural (content-extensible);
/// stats for each key live in `balance.toml` under `[creature.<key>]`.
fn creatures_for_biome(biome: &str) -> &'static [&'static str] {
    match biome {
        "forest" => &["forest_bloom_stalker", "thornback_boar"],
        "desert" => &["dune_wyrm", "sand_shade"],
        "ashfall" => &["cinder_imp", "magma_golem"],
        "tundra" => &["frost_lurker", "ice_revenant"],
        _ => &["bog_serpent", "myconid_brute"],
    }
}

/// A tiny deterministic PRNG (splitmix64). Same seed ⇒ same world, always —
/// the determinism invariant (world-generation.md §Invariants). No external rng
/// dependency (keeps the crate lean and wasm-neutral).
struct Rng(u64);

impl Rng {
    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    /// Uniform in `[0, 1)`.
    fn unit(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }
    /// Uniform in `[-1, 1)` (for symmetric jitter).
    fn signed(&mut self) -> f64 {
        self.unit() * 2.0 - 1.0
    }
    /// Pick an index in `[0, n)`.
    fn below(&mut self, n: usize) -> usize {
        if n == 0 {
            0
        } else {
            (self.next_u64() % n as u64) as usize
        }
    }
}

/// Advance a raw `u64` PRNG state and return a uniform `[0, 1)` (for per-creature
/// wander, whose state lives on the `MonsterSpawn`).
fn next_unit(state: &mut u64) -> f64 {
    let mut r = Rng(*state);
    let u = r.unit();
    *state = r.0;
    u
}

/// A monster placed in the overworld. Creatures roam (see [`Arena::step_creatures`])
/// and belong to a faction (grouping + hostility).
#[derive(Debug, Clone)]
pub struct MonsterSpawn {
    pub entity_id: Id,
    pub monster_kind: String,
    pub position: Position,
    /// Where it spawned — passive/territorial creatures leash to it.
    pub home: Position,
    /// The x-bounds of this creature's area; it never roams outside them (keeps
    /// creatures in their biome and stops distant creatures from wandering into a
    /// safe/tutorial area).
    pub area_min_x: f64,
    pub area_max_x: f64,
    pub level: i32,
    pub encounter_class: String,
    pub faction: String,
    /// `passive` | `territorial` | `aggressive`.
    pub aggression: String,
    pub flees: bool,
    /// World-scaled combat stats (stat_mult applied at spawn — no rescale later).
    pub hp: i32,
    pub atk: i32,
    pub def: i32,
    pub speed_stat: i32,
    pub xp_reward: i64,
    pub defeated: bool,
    /// True while this creature is locked in a battle (so it stops roaming).
    pub in_battle: bool,
    /// Per-creature PRNG state for deterministic wander.
    rng: u64,
}

impl MonsterSpawn {
    /// Build a spawn for `kind` at `position`, scaling the creature's base stats
    /// by `stat_mult` at that position's floored distance. `seed` drives its wander.
    fn build(balance: &Balance, entity_id: Id, kind: &str, position: Position, seed: u64) -> Self {
        let d = position.distance_floor();
        let scaling = Scaling::new(balance);
        let stats = balance
            .creature
            .get(kind)
            .unwrap_or_else(|| panic!("creature `{kind}` in balance.toml"));
        let mult = scaling.stat_mult(d);
        MonsterSpawn {
            entity_id,
            monster_kind: kind.to_string(),
            position,
            home: position,
            area_min_x: f64::NEG_INFINITY,
            area_max_x: f64::INFINITY,
            level: scaling.mlevel(d),
            encounter_class: stats.encounter_class.clone(),
            faction: stats.faction.clone(),
            aggression: stats.aggression.clone(),
            flees: stats.flees,
            hp: ((stats.base_hp as f64) * mult).round() as i32,
            atk: ((stats.base_atk as f64) * mult).round() as i32,
            def: stats.base_def,
            speed_stat: stats.speed_stat,
            xp_reward: stats.xp_reward,
            defeated: false,
            in_battle: false,
            rng: seed | 1,
        }
    }
}

/// One generated area: a stretch of corridor `[start_x, end_x)` in one biome,
/// holding the indices of its creatures (into [`Arena::monsters`]) and a portal.
#[derive(Debug, Clone)]
pub struct Area {
    pub index: usize,
    pub biome: &'static str,
    pub start_x: f64,
    pub end_x: f64,
    pub monster_idxs: Vec<usize>,
    pub portal: Position,
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

/// The generated overworld for one MazeInstance (spike scope): a seeded chain of
/// biome areas along a walkable corridor.
pub struct Arena {
    /// The seed this world was generated from (determinism / debugging).
    pub seed: u64,
    pub areas: Vec<Area>,
    pub monsters: Vec<MonsterSpawn>,
    pub avatars: Vec<Avatar>,
    /// Walkable bounds: `x ∈ [x_min, x_max]`, `y ∈ [-lateral, lateral]`.
    x_min: f64,
    x_max: f64,
    lateral: f64,
    touch_radius: f64,
    interaction_radius: f64,
    sim_dt: f64,
    // Creature-AI tunables (snapshot from balance).
    wander_speed: f64,
    chase_speed: f64,
    aggro_radius: f64,
    territorial_aggro_radius: f64,
    leash_radius: f64,
    group_radius: f64,
}

impl Arena {
    /// Generate a fresh world from `seed`. Deterministic: same seed ⇒ same areas,
    /// creatures, and portals (world-generation.md determinism invariant).
    pub fn generate(balance: &Balance, seed: u64) -> Self {
        let wg = &balance.worldgen;
        let mut rng = Rng(seed);

        let mut areas: Vec<Area> = Vec::new();
        let mut monsters: Vec<MonsterSpawn> = Vec::new();
        let mut cursor = 0.0_f64; // current x as we lay areas end-to-end

        for i in 0..wg.area_count.max(1) {
            let start_x = cursor;
            let biome = biome_for_distance(start_x.floor() as i64);
            let kinds = creatures_for_biome(biome);
            let mut area_idxs = Vec::new();

            // Area 0 is a small, deterministic "tutorial" area near the Center Hub:
            // exactly one canonical creature on the centre line and a portal a short
            // walk past it. Predictable onboarding (a straight east walk always meets
            // one fightable target, then a portal) — and the e2e/conformance tests
            // depend on this determinism. Procedural variety begins at area 1.
            if i == 0 {
                let pos = Position::new(wg.first_monster_x, 0.0);
                let idx = monsters.len();
                let mseed = rng.next_u64();
                monsters.push(MonsterSpawn::build(balance, format!("mob-{idx}"), kinds[0], pos, mseed));
                area_idxs.push(idx);
                let portal_x = wg.first_monster_x + wg.first_area_portal_gap;
                let end_x = portal_x + wg.portal_setback;
                monsters[idx].area_min_x = start_x;
                monsters[idx].area_max_x = end_x;
                areas.push(Area {
                    index: i,
                    biome,
                    start_x,
                    end_x,
                    monster_idxs: area_idxs,
                    portal: Position::new(portal_x, 0.0),
                });
                cursor = end_x;
                continue;
            }

            // Procedural area. Length trends larger with depth (growth·i) plus a
            // per-area jitter, so areas differ in size and later ones are bigger
            // on average.
            let nominal = wg.base_area_length + wg.area_length_growth * i as f64;
            let length = (nominal * (1.0 + wg.area_length_jitter * rng.signed())).max(8.0);
            let end_x = start_x + length;

            // Walk the corridor placing creatures at jittered gaps.
            let inner_end = end_x - wg.portal_setback - 1.0;
            let mut x = start_x + 2.0;
            while x < inner_end {
                let kind = kinds[rng.below(kinds.len())];
                // Keep creatures within the corridor's touch band of the centre
                // line so an east walk collides with them (touch_radius ~1 tile).
                let y = wg.lateral_jitter * rng.signed();
                let pos = Position::new(x, y);
                let idx = monsters.len();
                let mseed = rng.next_u64();
                monsters.push(MonsterSpawn::build(balance, format!("mob-{idx}"), kind, pos, mseed));
                monsters[idx].area_min_x = start_x;
                monsters[idx].area_max_x = end_x;
                area_idxs.push(idx);

                let gap = wg.monster_spacing * (1.0 + wg.monster_spacing_jitter * rng.signed());
                x += gap.max(2.0);
            }

            areas.push(Area {
                index: i,
                biome,
                start_x,
                end_x,
                monster_idxs: area_idxs,
                portal: Position::new(end_x - wg.portal_setback, 0.0),
            });
            cursor = end_x;
        }

        let x_max = cursor + wg.world_margin;
        Arena {
            seed,
            areas,
            monsters,
            avatars: Vec::new(),
            x_min: -wg.lateral_half_extent, // a little slack behind the hub
            x_max,
            lateral: wg.lateral_half_extent,
            touch_radius: balance.world.touch_radius_tiles,
            interaction_radius: balance.world.interaction_radius_tiles,
            sim_dt: 1.0 / balance.world.overworld_sim_hz as f64,
            wander_speed: balance.ai.wander_speed,
            chase_speed: balance.ai.chase_speed,
            aggro_radius: balance.ai.aggro_radius,
            territorial_aggro_radius: balance.ai.territorial_aggro_radius,
            leash_radius: balance.ai.leash_radius,
            group_radius: balance.ai.group_radius,
        }
    }

    /// Advance every roaming creature one step of `dt` seconds. Aggressive creatures
    /// chase the nearest active player within their aggro radius; territorial ones
    /// only give chase when a player is close, else patrol near home; passive ones
    /// just drift near home. Battling/defeated creatures hold still. Deterministic
    /// given the per-creature seed (no wall-clock, no global RNG).
    pub fn step_creatures(&mut self, dt: f64) {
        // Snapshot active-avatar positions (immutable borrow) before moving creatures.
        let players: Vec<Position> = self
            .avatars
            .iter()
            .filter(|a| a.state == "active")
            .map(|a| a.position)
            .collect();
        let (x_max, x_min, lateral) = (self.x_max, self.x_min, self.lateral);
        let (wander, chase) = (self.wander_speed, self.chase_speed);
        let (aggro, terr_aggro, leash) = (self.aggro_radius, self.territorial_aggro_radius, self.leash_radius);

        for m in self.monsters.iter_mut() {
            if m.defeated || m.in_battle {
                continue;
            }
            // Nearest active player.
            let nearest = players
                .iter()
                .min_by(|a, b| {
                    m.position
                        .distance_to(a)
                        .total_cmp(&m.position.distance_to(b))
                })
                .copied();
            let aggro_range = match m.aggression.as_str() {
                "aggressive" => aggro,
                "territorial" => terr_aggro,
                _ => 0.0, // passive: never chases
            };
            let (mut dx, mut dy, speed) = match nearest {
                Some(p) if m.position.distance_to(&p) <= aggro_range && aggro_range > 0.0 => {
                    // Chase the player.
                    (p.x - m.position.x, p.y - m.position.y, chase)
                }
                _ => {
                    // Wander: drift toward a seeded random point within the leash.
                    let ang = next_unit(&mut m.rng) * std::f64::consts::TAU;
                    let target_x = m.home.x + ang.cos() * leash;
                    let target_y = m.home.y + ang.sin() * leash * 0.4; // corridor is wider in x
                    (target_x - m.position.x, target_y - m.position.y, wander)
                }
            };
            let mag = (dx * dx + dy * dy).sqrt();
            if mag > 1e-6 {
                dx /= mag;
                dy /= mag;
                let step = speed * dt;
                // Clamp to the world bounds AND the creature's own area (creatures
                // stay in their biome; distant ones never wander into a safe area).
                let lo_x = x_min.max(m.area_min_x);
                let hi_x = x_max.min(m.area_max_x);
                m.position.x = (m.position.x + dx * step).max(lo_x).min(hi_x);
                m.position.y = (m.position.y + dy * step).max(-lateral).min(lateral);
            }
        }
    }

    /// The living creatures within `group_radius` of creature `idx` (including it).
    /// This is the encounter you pull when you touch one — nearby creatures pile
    /// in; their factions decide who fights whom once in battle.
    pub fn group_around(&self, idx: usize) -> Vec<usize> {
        let Some(origin) = self.monsters.get(idx) else {
            return vec![];
        };
        let center = origin.position;
        let r = self.group_radius;
        self.monsters
            .iter()
            .enumerate()
            .filter(|(_, m)| !m.defeated && center.distance_to(&m.position) <= r)
            .map(|(i, _)| i)
            .collect()
    }

    /// Positions of every portal (one per area).
    pub fn portals(&self) -> impl Iterator<Item = Position> + '_ {
        self.areas.iter().map(|a| a.portal)
    }

    /// Is `player` within interaction range of any extraction portal?
    pub fn at_portal(&self, player_id: &str) -> bool {
        let Some(a) = self.avatar(player_id) else {
            return false;
        };
        self.portals()
            .any(|p| a.position.distance_to(&p) <= self.interaction_radius)
    }

    /// Spawn a player avatar near the Center Hub (staggered so parties don't
    /// stack). All start on the y=0 corridor so they can walk east into creatures.
    pub fn add_avatar(&mut self, player_id: String, speed: f64) {
        let idx = self.avatars.len();
        self.avatars.push(Avatar {
            player_id,
            position: Position::new(-(idx as f64) * 0.6, 0.0),
            state: "active".to_string(),
            last_input_seq: 0,
            max_speed_tiles_per_sec: speed,
        });
    }

    pub fn avatar(&self, player_id: &str) -> Option<&Avatar> {
        self.avatars.iter().find(|a| a.player_id == player_id)
    }

    pub fn avatar_mut(&mut self, player_id: &str) -> Option<&mut Avatar> {
        self.avatars.iter_mut().find(|a| a.player_id == player_id)
    }

    /// Integrate one movement intent against authoritative position, clamped to
    /// the corridor bounds and max speed (server owns movement — CANON.md §S,
    /// D11). Returns the authoritative position after integration.
    pub fn apply_move(
        &mut self,
        player_id: &str,
        dir_x: f64,
        dir_y: f64,
        input_seq: u32,
    ) -> Option<Position> {
        let dt = self.sim_dt;
        let (x_min, x_max, lateral) = (self.x_min, self.x_max, self.lateral);
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
        a.position.x = (a.position.x + nx * step).max(x_min).min(x_max);
        a.position.y = (a.position.y + ny * step).max(-lateral).min(lateral);
        a.last_input_seq = input_seq;
        Some(a.position)
    }

    /// The first **living** monster within touch range of an **active** (not
    /// already battling) avatar, as `(player_id, monster_index)`. Battling
    /// avatars are `in_battle`, so a hit is always a fresh toucher — the caller
    /// starts a battle or raid-merges into one.
    pub fn check_touch(&self) -> Option<(Id, usize)> {
        for a in self.avatars.iter().filter(|a| a.state == "active") {
            for (idx, m) in self.monsters.iter().enumerate() {
                if !m.defeated && a.position.distance_to(&m.position) <= self.touch_radius {
                    return Some((a.player_id.clone(), idx));
                }
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aggressive_creature_chases_a_nearby_player() {
        let b = Balance::load_default().unwrap();
        let mut arena = Arena::generate(&b, 5);
        arena.monsters[0].aggression = "aggressive".to_string();
        let m = arena.monsters[0].position;
        arena.add_avatar("p".into(), 6.0);
        arena.avatar_mut("p").unwrap().position = Position::new(m.x + 3.0, m.y);
        let before = arena.monsters[0].position.x;
        for _ in 0..10 {
            arena.step_creatures(0.1);
        }
        assert!(
            arena.monsters[0].position.x > before + 0.5,
            "aggressive creature should move toward the player"
        );
    }

    #[test]
    fn passive_creature_leashes_near_home() {
        let b = Balance::load_default().unwrap();
        let mut arena = Arena::generate(&b, 5);
        arena.monsters[0].aggression = "passive".to_string();
        let home = arena.monsters[0].home;
        // A player standing on it must NOT draw a passive creature.
        arena.add_avatar("p".into(), 6.0);
        arena.avatar_mut("p").unwrap().position = home;
        for _ in 0..40 {
            arena.step_creatures(0.1);
        }
        assert!(
            arena.monsters[0].position.distance_to(&home) <= arena.leash_radius + 1.0,
            "passive creature should stay leashed to home"
        );
    }

    #[test]
    fn group_around_pulls_in_close_creatures() {
        let b = Balance::load_default().unwrap();
        let mut arena = Arena::generate(&b, 5);
        assert!(arena.monsters.len() >= 2);
        // Park the second creature right next to the first.
        arena.monsters[1].position = arena.monsters[0].position;
        let g = arena.group_around(0);
        assert!(g.contains(&0) && g.contains(&1), "close creatures group up");
    }

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
    fn generation_is_deterministic() {
        let b = Balance::load_default().unwrap();
        let a = Arena::generate(&b, 12345);
        let c = Arena::generate(&b, 12345);
        assert_eq!(a.areas.len(), c.areas.len());
        assert_eq!(a.monsters.len(), c.monsters.len());
        for (m, n) in a.monsters.iter().zip(c.monsters.iter()) {
            assert_eq!(m.monster_kind, n.monster_kind);
            assert_eq!(m.position, n.position);
            assert_eq!(m.hp, n.hp);
        }
        // A different seed yields a different world (overwhelmingly likely).
        let d = Arena::generate(&b, 999);
        assert!(a.monsters.len() != d.monsters.len() || a.monsters[0].position != d.monsters[0].position);
    }

    #[test]
    fn areas_trend_larger_and_carry_creatures() {
        let b = Balance::load_default().unwrap();
        let arena = Arena::generate(&b, 7);
        assert_eq!(arena.areas.len(), b.worldgen.area_count);
        assert!(!arena.monsters.is_empty());
        // Every area has a portal past its creatures and at least one creature.
        for area in &arena.areas {
            assert!(area.portal.x <= area.end_x);
            assert!(!area.monster_idxs.is_empty());
        }
        // First vs last area length: last is larger on average (growth term).
        let first = arena.areas.first().unwrap();
        let last = arena.areas.last().unwrap();
        assert!(last.end_x - last.start_x > first.end_x - first.start_x);
        // Deeper creatures are stronger (monotone difficulty in d).
        let shallow = &arena.monsters[0];
        let deep = arena.monsters.last().unwrap();
        assert!(deep.position.x > shallow.position.x);
        assert!(deep.level >= shallow.level);
    }

    #[test]
    fn walking_east_touches_the_first_creature_then_it_is_slain() {
        let b = Balance::load_default().unwrap();
        let mut arena = Arena::generate(&b, 42);
        arena.add_avatar("p1".into(), 6.0);
        assert!(arena.check_touch().is_none());
        // Walk east along the corridor for up to ~8 s of sim ticks.
        let mut hit = None;
        for i in 0..(20 * 8) {
            arena.apply_move("p1", 1.0, 0.0, i + 1);
            if let Some((p, idx)) = arena.check_touch() {
                hit = Some((p, idx));
                break;
            }
        }
        let (player, idx) = hit.expect("east walk meets a creature");
        assert_eq!(player, "p1");
        // Slay it: a defeated monster is no longer touchable.
        arena.monsters[idx].defeated = true;
        // Standing on the slain monster, check_touch must not re-trigger it.
        arena.avatar_mut("p1").unwrap().position = arena.monsters[idx].position;
        let again = arena.check_touch();
        assert!(again.map(|(_, i)| i != idx).unwrap_or(true));
    }
}
