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
use meld_proto::factions::creatures_hostile;
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

/// Harvestable resource node ids that spawn in a biome (one alchemy reagent + one
/// forging ore/wood per biome). Structural; stats live under `[resource.<key>]`.
fn resources_for_biome(biome: &str) -> &'static [&'static str] {
    match biome {
        "forest" => &["bloom_herb", "heartoak_bark"],
        "desert" => &["sun_salts", "dune_iron"],
        "ashfall" => &["ember_ash", "cinder_ore"],
        "tundra" => &["frost_lichen", "rime_ore"],
        _ => &["bog_myrrh", "peat_iron"],
    }
}

/// Impassable terrain feature kinds per biome (drives client rendering; all block
/// movement identically). Structural content.
fn obstacles_for_biome(biome: &str) -> &'static [&'static str] {
    match biome {
        "forest" => &["tree", "boulder", "pond"],
        "desert" => &["dune", "rock_spire", "cactus"],
        "ashfall" => &["cliff", "lava", "cinder_rock"],
        "tundra" => &["ice_spire", "frozen_pond", "snow_drift"],
        _ => &["bog_pool", "mire_root", "fungal_wall"],
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
    /// Item kind dropped as ground loot when felled by an overworld skirmish.
    pub loot_kind: String,
    pub defeated: bool,
    /// True while this creature is locked in a battle (so it stops roaming).
    pub in_battle: bool,
    /// Seconds until this creature can land its next overworld skirmish blow.
    skirmish_cd: f64,
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
            loot_kind: stats.loot_kind.clone(),
            defeated: false,
            in_battle: false,
            skirmish_cd: 0.0,
            rng: seed | 1,
        }
    }
}

/// An item dropped on the ground when a creature is felled by an overworld
/// skirmish. Players auto-collect it by walking within `loot_pickup_radius`.
#[derive(Debug, Clone)]
pub struct GroundLoot {
    pub entity_id: Id,
    /// Item kind banked into the backpack on pickup.
    pub kind: String,
    pub position: Position,
}

/// A harvestable resource node in the overworld. Walk up and harvest it once for
/// its material (into the backpack) + Meld-skill XP; then it's spent.
#[derive(Debug, Clone)]
pub struct ResourceNode {
    pub entity_id: Id,
    /// Content id (`bloom_herb`, `dune_iron`, …) — keys `[resource.<kind>]`.
    pub kind: String,
    pub position: Position,
    pub harvested: bool,
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

/// An impassable terrain feature (tree, cliff, pond, …). Circular for the spike;
/// the player and roaming creatures cannot enter its radius.
#[derive(Debug, Clone)]
pub struct Obstacle {
    pub entity_id: Id,
    /// Content kind (`tree`/`cliff`/`lava`/…) — drives client rendering.
    pub kind: String,
    pub position: Position,
    pub radius: f64,
}

/// Shortest distance from point `p` to the polyline `path` (min over segments).
fn dist_to_path(p: &Position, path: &[Position]) -> f64 {
    if path.is_empty() {
        return f64::INFINITY;
    }
    if path.len() == 1 {
        return p.distance_to(&path[0]);
    }
    let mut best = f64::INFINITY;
    for w in path.windows(2) {
        best = best.min(dist_point_segment(p, &w[0], &w[1]));
    }
    best
}

/// Distance from point `p` to segment `a`–`b`.
fn dist_point_segment(p: &Position, a: &Position, b: &Position) -> f64 {
    let (abx, aby) = (b.x - a.x, b.y - a.y);
    let (apx, apy) = (p.x - a.x, p.y - a.y);
    let len2 = abx * abx + aby * aby;
    let t = if len2 <= 1e-9 {
        0.0
    } else {
        ((apx * abx + apy * aby) / len2).clamp(0.0, 1.0)
    };
    let (cx, cy) = (a.x + t * abx, a.y + t * aby);
    ((p.x - cx).powi(2) + (p.y - cy).powi(2)).sqrt()
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
    pub resources: Vec<ResourceNode>,
    /// Loot dropped by creatures felled in overworld skirmishes, awaiting pickup.
    pub ground_loot: Vec<GroundLoot>,
    /// Impassable biome terrain (trees/cliffs/water/…). Never intrudes on `path`.
    pub obstacles: Vec<Obstacle>,
    /// The guaranteed-clear route from the hub to the portal, as waypoints. A tube
    /// of `path_clear_radius` around it holds no obstacles, so the exit is always
    /// reachable; the client draws it as a faint trail.
    pub path: Vec<Position>,
    /// The single fixed extraction portal, deep at the end of the last area.
    /// Extraction is otherwise the Town Portal item (works anywhere).
    pub portal: Position,
    pub avatars: Vec<Avatar>,
    /// Walkable bounds: `x ∈ [x_min, x_max]`, `y ∈ [-lateral, lateral]`.
    x_min: f64,
    x_max: f64,
    lateral: f64,
    /// The avatar's collision radius against obstacles.
    player_radius: f64,
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
    skirmish_aggro: f64,
    skirmish_range: f64,
    skirmish_interval: f64,
    loot_pickup_radius: f64,
}

impl Arena {
    /// Generate a fresh world from `seed`. Deterministic: same seed ⇒ same areas,
    /// creatures, and portals (world-generation.md determinism invariant).
    pub fn generate(balance: &Balance, seed: u64) -> Self {
        let wg = &balance.worldgen;
        let mut rng = Rng(seed);

        let mut areas: Vec<Area> = Vec::new();
        let mut monsters: Vec<MonsterSpawn> = Vec::new();
        let mut resources: Vec<ResourceNode> = Vec::new();
        let mut obstacles: Vec<Obstacle> = Vec::new();
        // The guaranteed clear path, waypoint by waypoint (starts at the hub).
        let mut path: Vec<Position> = vec![Position::new(0.0, 0.0)];
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
                // A guaranteed starter resource node just off the tutorial path, so
                // the first thing a new player can safely do is harvest (no fight).
                resources.push(ResourceNode {
                    entity_id: format!("res-{}", resources.len()),
                    kind: resources_for_biome(biome)[0].to_string(),
                    position: Position::new(wg.first_monster_x * 0.5, 3.0),
                    harvested: false,
                });
                areas.push(Area {
                    index: i,
                    biome,
                    start_x,
                    end_x,
                    monster_idxs: area_idxs,
                    portal: Position::new(portal_x, 0.0),
                });
                // The tutorial path is a straight, obstacle-free line to y=0.
                path.push(Position::new(end_x, 0.0));
                cursor = end_x;
                continue;
            }

            // Procedural area. Length trends larger with depth (growth·i) plus a
            // per-area jitter, so areas differ in size and later ones are bigger
            // on average.
            let nominal = wg.base_area_length + wg.area_length_growth * i as f64;
            let length = (nominal * (1.0 + wg.area_length_jitter * rng.signed())).max(8.0);
            let end_x = start_x + length;

            // Walk the corridor placing creatures at jittered gaps. Creatures no
            // longer hug the centre line — they scatter across ±y so the map is
            // populated in every direction and you explore to find fights (area 0
            // stays on the line for the deterministic tutorial).
            let inner_end = end_x - wg.portal_setback - 1.0;
            let mut x = start_x + 2.0;
            while x < inner_end {
                let kind = kinds[rng.below(kinds.len())];
                let y = wg.creature_lateral_spread * rng.signed();
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

            // Scatter harvestable resource nodes through the area (2D, biome kinds).
            let rkinds = resources_for_biome(biome);
            let n_nodes = wg.resources_per_area.max(0.0).round() as usize;
            for _ in 0..n_nodes {
                let rk = rkinds[rng.below(rkinds.len())];
                let rx = start_x + 2.0 + rng.unit() * (length - 4.0).max(1.0);
                let ry = wg.resource_lateral_spread * rng.signed();
                let nid = resources.len();
                resources.push(ResourceNode {
                    entity_id: format!("res-{nid}"),
                    kind: rk.to_string(),
                    position: Position::new(rx, ry),
                    harvested: false,
                });
            }

            areas.push(Area {
                index: i,
                biome,
                start_x,
                end_x,
                monster_idxs: area_idxs,
                portal: Position::new(end_x - wg.portal_setback, 0.0),
            });

            // The clear path meanders to a fresh ±y at this area's end. The last
            // area's waypoint IS the portal, so the obstacle pass below keeps the
            // final approach clear too. This completes the path segment spanning
            // the area, letting obstacles avoid the whole tube by construction.
            let is_last = i + 1 == wg.area_count.max(1);
            if is_last {
                path.push(Position::new(end_x - wg.portal_setback, 0.0));
            } else {
                path.push(Position::new(end_x, wg.path_meander * rng.signed()));
            }

            // Scatter impassable biome terrain, rejecting anything that would block
            // the clear path tube or bury a creature/resource. Rejection-sampled so
            // the path (and the exit) is always feasible by construction.
            let okinds = obstacles_for_biome(biome);
            let n_obs = wg.obstacles_per_area.max(0.0).round() as usize;
            let (mut placed, mut attempts) = (0usize, 0usize);
            while placed < n_obs && attempts < n_obs * 10 {
                attempts += 1;
                let ox = start_x + rng.unit() * length;
                let oy = rng.signed() * (wg.lateral_half_extent - 1.0);
                let radius = wg.obstacle_min_radius
                    + rng.unit() * (wg.obstacle_max_radius - wg.obstacle_min_radius);
                let pos = Position::new(ox, oy);
                // Keep the guaranteed path tube clear.
                if dist_to_path(&pos, &path) < wg.path_clear_radius + radius {
                    continue;
                }
                // Don't bury a creature or resource node under terrain.
                let buries = monsters.iter().any(|m| m.position.distance_to(&pos) < radius + 1.5)
                    || resources.iter().any(|r| r.position.distance_to(&pos) < radius + 1.5);
                if buries {
                    continue;
                }
                obstacles.push(Obstacle {
                    entity_id: format!("obs-{}", obstacles.len()),
                    kind: okinds[rng.below(okinds.len())].to_string(),
                    position: pos,
                    radius,
                });
                placed += 1;
            }
            cursor = end_x;
        }

        let x_max = cursor + wg.world_margin;
        // A single fixed extraction portal, deep at the end of the last area.
        let portal = areas
            .last()
            .map(|a| a.portal)
            .unwrap_or_else(|| Position::new(x_max, 0.0));
        Arena {
            seed,
            areas,
            monsters,
            resources,
            ground_loot: Vec::new(),
            obstacles,
            path,
            portal,
            avatars: Vec::new(),
            x_min: -4.0, // a little slack behind the hub
            x_max,
            lateral: wg.lateral_half_extent,
            player_radius: wg.player_radius,
            touch_radius: balance.world.touch_radius_tiles,
            interaction_radius: balance.world.interaction_radius_tiles,
            sim_dt: 1.0 / balance.world.overworld_sim_hz as f64,
            wander_speed: balance.ai.wander_speed,
            chase_speed: balance.ai.chase_speed,
            aggro_radius: balance.ai.aggro_radius,
            territorial_aggro_radius: balance.ai.territorial_aggro_radius,
            leash_radius: balance.ai.leash_radius,
            group_radius: balance.ai.group_radius,
            skirmish_aggro: balance.ai.skirmish_aggro_radius,
            skirmish_range: balance.ai.skirmish_attack_range,
            skirmish_interval: balance.ai.skirmish_attack_interval,
            loot_pickup_radius: balance.ai.loot_pickup_radius,
        }
    }

    /// Advance every roaming creature one step of `dt` seconds. Creatures chase the
    /// nearest target within their aggro radius — either an active player OR a
    /// hostile-faction creature (overworld skirmishing); aggressive creatures hunt
    /// on sight, territorial ones only when close, passive ones just drift near
    /// home. Adjacent hostile creatures hold and trade blows (the damage pass); a
    /// creature felled by a skirmish drops [`GroundLoot`] where it fell. Battling/
    /// defeated creatures hold still. Deterministic given the per-creature seed.
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
        let (skirmish_aggro, skirmish_range) = (self.skirmish_aggro, self.skirmish_range);
        let interval = self.skirmish_interval;
        let obstacles: Vec<(Position, f64)> =
            self.obstacles.iter().map(|o| (o.position, o.radius)).collect();
        // Combat state of every creature, snapshotted so a creature can target
        // another without aliasing the `&mut` iteration below. (pos, faction, alive, def).
        let cs: Vec<(Position, String, bool, i32)> = self
            .monsters
            .iter()
            .map(|m| (m.position, m.faction.clone(), !m.defeated && !m.in_battle, m.def))
            .collect();

        // --- Movement pass: pick a target and close on it (or wander) ----------
        for (i, m) in self.monsters.iter_mut().enumerate() {
            if m.defeated || m.in_battle {
                continue;
            }
            let aggro_range = match m.aggression.as_str() {
                "aggressive" => aggro,
                "territorial" => terr_aggro,
                _ => 0.0, // passive: never chases (but still retaliates below)
            };
            // Nearest active player within aggro range.
            let player_target = players
                .iter()
                .copied()
                .filter(|p| aggro_range > 0.0 && m.position.distance_to(p) <= aggro_range)
                .min_by(|a, b| m.position.distance_to(a).total_cmp(&m.position.distance_to(b)));
            // Nearest hostile-faction creature within skirmish aggro (initiators only).
            let creature_target = cs
                .iter()
                .enumerate()
                .filter(|(j, (_, fac, alive, _))| {
                    *j != i && *alive && aggro_range > 0.0 && creatures_hostile(&m.faction, fac)
                })
                .map(|(_, (pos, _, _, _))| *pos)
                .filter(|pos| m.position.distance_to(pos) <= skirmish_aggro)
                .min_by(|a, b| m.position.distance_to(a).total_cmp(&m.position.distance_to(b)));
            // Prefer whichever target is closer; a creature target lets us stop short
            // and brawl, a player target we must actually touch to trigger a battle.
            let (target, is_creature) = match (player_target, creature_target) {
                (Some(p), Some(c)) => {
                    if m.position.distance_to(&p) <= m.position.distance_to(&c) {
                        (Some(p), false)
                    } else {
                        (Some(c), true)
                    }
                }
                (Some(p), None) => (Some(p), false),
                (None, Some(c)) => (Some(c), true),
                (None, None) => (None, false),
            };
            let (mut dx, mut dy, speed) = match target {
                Some(p) => {
                    // Hold position once adjacent to a creature rival (trade blows in
                    // the damage pass) instead of jittering through it.
                    if is_creature && m.position.distance_to(&p) <= skirmish_range {
                        (0.0, 0.0, chase)
                    } else {
                        (p.x - m.position.x, p.y - m.position.y, chase)
                    }
                }
                None => {
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
                let nx = (m.position.x + dx * step).max(lo_x).min(hi_x);
                let ny = (m.position.y + dy * step).max(-lateral).min(lateral);
                // Creatures don't walk through terrain either (slide per axis).
                let cand = Position::new(nx, ny);
                if !Self::obstacle_blocks(&obstacles, &cand, 0.5) {
                    m.position = cand;
                } else if !Self::obstacle_blocks(&obstacles, &Position::new(nx, m.position.y), 0.5) {
                    m.position.x = nx;
                } else if !Self::obstacle_blocks(&obstacles, &Position::new(m.position.x, ny), 0.5) {
                    m.position.y = ny;
                }
            }
        }

        // --- Damage pass: adjacent hostile creatures trade blows ---------------
        // Any two living hostile-faction creatures within attack range hit each
        // other on their own cooldown — passive creatures fight back too, they just
        // never gave chase. Uses post-movement positions.
        let now: Vec<(Position, String, bool, i32)> = self
            .monsters
            .iter()
            .map(|m| (m.position, m.faction.clone(), !m.defeated && !m.in_battle, m.def))
            .collect();
        let mut hits: Vec<(usize, i32)> = Vec::new();
        for (i, m) in self.monsters.iter_mut().enumerate() {
            if m.defeated || m.in_battle {
                m.skirmish_cd = 0.0;
                continue;
            }
            m.skirmish_cd = (m.skirmish_cd - dt).max(0.0);
            if m.skirmish_cd > 0.0 {
                continue;
            }
            let victim = now
                .iter()
                .enumerate()
                .filter(|(j, (_, fac, alive, _))| {
                    *j != i && *alive && creatures_hostile(&m.faction, fac)
                })
                .filter(|(_, (pos, _, _, _))| m.position.distance_to(pos) <= skirmish_range)
                .min_by(|(_, (a, _, _, _)), (_, (b, _, _, _))| {
                    m.position.distance_to(a).total_cmp(&m.position.distance_to(b))
                });
            if let Some((j, (_, _, _, victim_def))) = victim {
                let dmg = (m.atk - victim_def).max(1);
                hits.push((j, dmg));
                m.skirmish_cd = interval;
            }
        }
        for (j, dmg) in hits {
            self.monsters[j].hp -= dmg;
        }

        // --- Deaths → ground loot ---------------------------------------------
        let mut drops: Vec<(Id, String, Position)> = Vec::new();
        for m in self.monsters.iter_mut() {
            if !m.defeated && !m.in_battle && m.hp <= 0 {
                m.defeated = true;
                drops.push((m.entity_id.clone(), m.loot_kind.clone(), m.position));
            }
        }
        for (eid, kind, position) in drops {
            let n = self.ground_loot.len();
            self.ground_loot.push(GroundLoot {
                entity_id: format!("loot-{eid}-{n}"),
                kind,
                position,
            });
        }
    }

    /// Collect (remove and return) every ground-loot drop within pickup range of
    /// `player_id`. The caller banks each into the player's backpack.
    pub fn collect_loot(&mut self, player_id: &str) -> Vec<GroundLoot> {
        let Some(pos) = self.avatar(player_id).map(|a| a.position) else {
            return Vec::new();
        };
        let radius = self.loot_pickup_radius;
        let mut taken = Vec::new();
        let mut i = 0;
        while i < self.ground_loot.len() {
            if pos.distance_to(&self.ground_loot[i].position) <= radius {
                taken.push(self.ground_loot.remove(i));
            } else {
                i += 1;
            }
        }
        taken
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

    /// Is `player` within interaction range of the single deep extraction portal?
    pub fn at_portal(&self, player_id: &str) -> bool {
        let Some(a) = self.avatar(player_id) else {
            return false;
        };
        a.position.distance_to(&self.portal) <= self.interaction_radius
    }

    /// Harvest the resource node `entity_id` if `player` is within interaction
    /// range and it isn't already spent. Marks it harvested and returns its
    /// content kind (the caller maps that to a material + skill XP via balance).
    pub fn harvest(&mut self, player_id: &str, entity_id: &str) -> Option<String> {
        let ppos = self.avatar(player_id)?.position;
        let radius = self.interaction_radius;
        let node = self
            .resources
            .iter_mut()
            .find(|n| n.entity_id == entity_id && !n.harvested)?;
        if ppos.distance_to(&node.position) > radius {
            return None;
        }
        node.harvested = true;
        Some(node.kind.clone())
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

    /// Is `p` (a body of `radius`) inside any impassable obstacle?
    fn obstacle_blocks(obstacles: &[(Position, f64)], p: &Position, radius: f64) -> bool {
        obstacles.iter().any(|(c, r)| p.distance_to(c) < r + radius)
    }

    /// Integrate one movement intent against authoritative position, clamped to
    /// the world bounds and max speed, and blocked by biome obstacles (server owns
    /// movement — CANON.md §S, D11). Collisions **slide**: if the full step hits an
    /// obstacle, the axis-aligned components are tried so you glide along terrain
    /// rather than sticking. Returns the authoritative position after integration.
    pub fn apply_move(
        &mut self,
        player_id: &str,
        dir_x: f64,
        dir_y: f64,
        input_seq: u32,
    ) -> Option<Position> {
        let dt = self.sim_dt;
        let (x_min, x_max, lateral) = (self.x_min, self.x_max, self.lateral);
        let pr = self.player_radius;
        let obstacles: Vec<(Position, f64)> =
            self.obstacles.iter().map(|o| (o.position, o.radius)).collect();
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
        let cur = a.position;
        let clamp = |x: f64, y: f64| {
            Position::new(x.max(x_min).min(x_max), y.max(-lateral).min(lateral))
        };
        let full = clamp(cur.x + nx * step, cur.y + ny * step);
        let dest = if !Self::obstacle_blocks(&obstacles, &full, pr) {
            full
        } else {
            // Slide: try moving along only x, then only y.
            let sx = clamp(cur.x + nx * step, cur.y);
            let sy = clamp(cur.x, cur.y + ny * step);
            if !Self::obstacle_blocks(&obstacles, &sx, pr) {
                sx
            } else if !Self::obstacle_blocks(&obstacles, &sy, pr) {
                sy
            } else {
                cur // fully blocked
            }
        };
        a.position = dest;
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
    fn hostile_creatures_skirmish_and_drop_loot() {
        let b = Balance::load_default().unwrap();
        let mut arena = Arena::generate(&b, 5);
        assert!(arena.monsters.len() >= 2);
        // Isolate the encounter to a single hostile pair (other creatures across
        // the arena would skirmish too and add their own drops).
        arena.monsters.truncate(2);
        // Force a hostile pair adjacent: an aggressive attacker vs a weak rival.
        // Widen both area bounds so neither is snapped back to its home area.
        for k in 0..2 {
            arena.monsters[k].area_min_x = f64::NEG_INFINITY;
            arena.monsters[k].area_max_x = f64::INFINITY;
        }
        arena.monsters[0].faction = "beast".to_string();
        arena.monsters[0].aggression = "aggressive".to_string();
        arena.monsters[0].atk = 50;
        arena.monsters[0].hp = 500;
        let pos = arena.monsters[0].position;
        arena.monsters[1].faction = "fiend".to_string(); // beast vs fiend = hostile
        arena.monsters[1].aggression = "passive".to_string();
        arena.monsters[1].hp = 20;
        arena.monsters[1].def = 0;
        arena.monsters[1].home = Position::new(pos.x + 1.0, pos.y);
        arena.monsters[1].position = Position::new(pos.x + 1.0, pos.y);
        // No players present, so the only thing that can happen is a skirmish.
        for _ in 0..60 {
            arena.step_creatures(0.1);
        }
        assert!(
            arena.monsters[1].defeated,
            "the weaker rival should be felled by the skirmish"
        );
        assert_eq!(arena.ground_loot.len(), 1, "a felled creature drops loot");
        assert_eq!(arena.ground_loot[0].kind, arena.monsters[1].loot_kind);
    }

    #[test]
    fn player_collects_nearby_ground_loot() {
        let b = Balance::load_default().unwrap();
        let mut arena = Arena::generate(&b, 5);
        arena.ground_loot.push(GroundLoot {
            entity_id: "loot-x".into(),
            kind: "boar_tusk".into(),
            position: Position::new(20.0, 0.0),
        });
        arena.add_avatar("p".into(), 6.0);
        // Too far to pick up.
        arena.avatar_mut("p").unwrap().position = Position::new(30.0, 0.0);
        assert!(arena.collect_loot("p").is_empty());
        // Walk onto it.
        arena.avatar_mut("p").unwrap().position = Position::new(20.0, 0.0);
        let got = arena.collect_loot("p");
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].kind, "boar_tusk");
        assert!(arena.ground_loot.is_empty(), "loot removed once collected");
    }

    #[test]
    fn same_faction_creatures_do_not_skirmish() {
        let b = Balance::load_default().unwrap();
        let mut arena = Arena::generate(&b, 5);
        arena.monsters[0].faction = "beast".to_string();
        arena.monsters[0].aggression = "aggressive".to_string();
        arena.monsters[1].faction = "beast".to_string();
        let pos = arena.monsters[0].position;
        arena.monsters[1].position = Position::new(pos.x + 1.0, pos.y);
        let hp0 = arena.monsters[0].hp;
        let hp1 = arena.monsters[1].hp;
        for _ in 0..30 {
            arena.step_creatures(0.1);
        }
        assert_eq!(arena.monsters[0].hp, hp0, "allies never damage each other");
        assert_eq!(arena.monsters[1].hp, hp1);
        assert!(arena.ground_loot.is_empty());
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
    fn one_deep_portal_only() {
        let b = Balance::load_default().unwrap();
        let arena = Arena::generate(&b, 7);
        // The single extraction portal is the last area's, deep from the hub.
        assert_eq!(arena.portal, arena.areas.last().unwrap().portal);
        let first_area_end = arena.areas.first().unwrap().end_x;
        assert!(
            arena.portal.x > first_area_end,
            "the portal is deep, well past area 0"
        );
    }

    #[test]
    fn creatures_scatter_off_the_centre_line() {
        let b = Balance::load_default().unwrap();
        let arena = Arena::generate(&b, 7);
        // Area 0's tutorial creature stays on the line; deeper ones scatter in y.
        assert_eq!(arena.monsters[0].position.y, 0.0);
        let spread = arena
            .monsters
            .iter()
            .any(|m| m.position.y.abs() > b.worldgen.lateral_jitter + 1.0);
        assert!(spread, "creatures should scatter across ±y, not hug the line");
    }

    #[test]
    fn resource_nodes_generate_and_harvest_once_within_range() {
        let b = Balance::load_default().unwrap();
        let mut arena = Arena::generate(&b, 7);
        assert!(!arena.resources.is_empty(), "resource nodes are scattered in");
        let node = arena.resources[0].clone();
        arena.add_avatar("p".into(), 6.0);
        // Too far → no harvest.
        arena.avatar_mut("p").unwrap().position = Position::new(node.position.x + 50.0, node.position.y);
        assert!(arena.harvest("p", &node.entity_id).is_none(), "out of range");
        // Standing on it → harvest yields its kind, once.
        arena.avatar_mut("p").unwrap().position = node.position;
        assert_eq!(arena.harvest("p", &node.entity_id).as_deref(), Some(node.kind.as_str()));
        assert!(arena.harvest("p", &node.entity_id).is_none(), "already harvested");
        // Every node kind maps to balance content.
        for n in &arena.resources {
            assert!(b.resource.contains_key(&n.kind), "resource {} in balance", n.kind);
        }
    }

    #[test]
    fn terrain_generated_but_area0_stays_clear() {
        let b = Balance::load_default().unwrap();
        let arena = Arena::generate(&b, 7);
        assert!(!arena.obstacles.is_empty(), "biome terrain is generated");
        let area0_end = arena.areas[0].end_x;
        // The tutorial area is obstacle-free (deterministic onboarding).
        assert!(
            arena.obstacles.iter().all(|o| o.position.x > area0_end),
            "no obstacles in area 0"
        );
        // Kinds map to a biome table (no stray content).
        for o in &arena.obstacles {
            assert!(o.radius > 0.0);
        }
    }

    #[test]
    fn no_obstacle_intrudes_on_the_clear_path() {
        // The feasibility guarantee: every obstacle sits outside the path tube.
        for seed in [1u64, 7, 42, 999, 123456] {
            let b = Balance::load_default().unwrap();
            let arena = Arena::generate(&b, seed);
            for o in &arena.obstacles {
                let d = dist_to_path(&o.position, &arena.path);
                assert!(
                    d >= b.worldgen.path_clear_radius - 1e-6,
                    "seed {seed}: obstacle {} intrudes on the clear path (d={d:.2})",
                    o.entity_id
                );
            }
        }
    }

    #[test]
    fn the_clear_path_actually_reaches_the_portal() {
        // A walker that follows the waypoints reaches the portal without getting
        // stuck on terrain — the route is feasible by construction, end to end.
        let b = Balance::load_default().unwrap();
        let mut arena = Arena::generate(&b, 42);
        let waypoints = arena.path.clone();
        assert!(waypoints.len() >= 2);
        arena.add_avatar("p".into(), 8.0);
        let mut wp = 1usize;
        let mut reached = false;
        for _ in 0..50_000 {
            let target = waypoints[wp];
            let pos = arena.avatar("p").unwrap().position;
            if pos.distance_to(&target) < 0.6 {
                if wp + 1 >= waypoints.len() {
                    reached = true;
                    break;
                }
                wp += 1;
                continue;
            }
            arena.apply_move("p", target.x - pos.x, target.y - pos.y, 0);
        }
        assert!(reached, "following the path should reach the portal");
        let end = arena.avatar("p").unwrap().position;
        assert!(end.distance_to(&arena.portal) < 1.5, "walker ended at the portal");
    }

    #[test]
    fn obstacles_block_movement() {
        let b = Balance::load_default().unwrap();
        let mut arena = Arena::generate(&b, 7);
        let obs = arena.obstacles[0].clone();
        arena.add_avatar("p".into(), 6.0);
        // Stand just outside the obstacle and push straight into it.
        let start = Position::new(obs.position.x - obs.radius - 1.0, obs.position.y);
        arena.avatar_mut("p").unwrap().position = start;
        for _ in 0..60 {
            let p = arena.avatar("p").unwrap().position;
            arena.apply_move("p", obs.position.x - p.x, obs.position.y - p.y, 0);
        }
        let p = arena.avatar("p").unwrap().position;
        assert!(
            p.distance_to(&obs.position) >= obs.radius - 1e-6,
            "the avatar never enters the obstacle"
        );
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
