//! Overworld model for the spike (behaviors/world-generation.md subset).
//!
//! The full spec is an infinite seeded radial plane with 64×64 chunk streaming,
//! biomes, chokepoints and Gatekeeper arenas. This slice implements the part
//! that makes the loop feel like a *world*: a per-instance **seeded chain of
//! biome areas** marching east from the Center Hub. Each area (a "section") has
//! its own length (jittered, trending larger with depth), several creatures
//! placed along the corridor and scaled by their own distance, an extraction
//! portal near the chain's end, and — new — **terraced verticality**: raised
//! plateaus joined to the ground by connectors (ladders/ropes/slopes).
//!
//! **Per-section seeds & streaming** (VERTICALITY-PROPOSAL.md): each section `n`
//! is generated from its OWN derived seed `section_seed(run_seed, n)`, so sections
//! are independent (one section's RNG draws can't perturb another's) and any single
//! section reproduces exactly from `(run_seed, n)`. Sections are generated
//! **on demand** as the player advances ([`Arena::ensure_frontier`]) — the world is
//! endless, always fresh as you go deeper, and identical again on the same seed.
//! This is the deferred "chunk streaming" landing as the procedural core.
//!
//! **Verticality** (VERTICALITY-PROPOSAL.md): elevation is a small number of
//! integer levels, not a heightmap. Terraces are raised rectangles kept OUT of the
//! guaranteed clear-path tube, so the extraction route stays entirely on level 0
//! and is always feasible by construction. Cliffs are impassable walls; the only
//! way to change level is stepping onto a **connector** (slope/ladder/rope). There
//! is no free-form climbing.
//!
//! Still deferred (documented, not lost): true 2D radial chunk streaming,
//! Gatekeeper arenas, chokepoint geometry, and the infinite zone past d=5000.

use std::collections::HashMap;

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

/// A biome's combat-drop material — banked into the run backpack when a creature
/// is felled (feeds Forging/Alchemy crafting), distinct from harvestable resource
/// nodes. Forest keeps `forest_bloom_petal` (the crafting recipe + conformance
/// tests depend on that content id). Structural content; deeper bands repeat Mire.
pub fn combat_material_for_biome(d: i64) -> &'static str {
    match biome_for_distance(d) {
        "forest" => "forest_bloom_petal",
        "desert" => "sun_scarab_husk",
        "ashfall" => "ember_cinder",
        "tundra" => "frost_shard",
        _ => "bog_ichor",
    }
}

/// Red-chest gear rolled as creature loot (economy.md S1, gear-item-models.md).
#[derive(Debug, Clone, PartialEq)]
pub struct GearDrop {
    pub name: String,
    pub slot: String,
    pub tier: i32,
    pub atk_bonus: i32,
    pub max_durability: i32,
}

/// The loot a felled encounter yields to one participant.
#[derive(Debug, Clone, PartialEq)]
pub struct CreatureLoot {
    /// Chits found (banked on extraction, lost on death). Scales with depth.
    pub chits: i64,
    /// The biome's combat material (one unit).
    pub material: &'static str,
    /// Red-chest gear, only rolled at/after `red_chest_floor_distance`.
    pub gear: Option<GearDrop>,
}

/// Flavourful red-gear name for a biome + slot (deterministic given `rng`).
fn gear_name(d: i64, slot: &str, rng: &mut Rng) -> String {
    let adjectives: &[&str] = match biome_for_distance(d) {
        "forest" => &["Verdant", "Bloomforged", "Thornwood"],
        "desert" => &["Sunbaked", "Duneglass", "Scarab"],
        "ashfall" => &["Ashfall", "Cinderforged", "Emberwrought"],
        "tundra" => &["Rimebound", "Frostforged", "Glacial"],
        _ => &["Miremere", "Fungal", "Peatbound"],
    };
    let nouns: &[&str] = match slot {
        "weapon" => &["Greatblade", "Cleaver", "Warpick"],
        "armor" => &["Plate", "Aegis", "Carapace"],
        _ => &["Charm", "Sigil", "Band"],
    };
    let a = adjectives[rng.below(adjectives.len())];
    let n = nouns[rng.below(nouns.len())];
    format!("{a} {n}")
}

/// Roll the loot a felled encounter yields to one participant, deterministically
/// from `seed` (economy.md S1; balance `[loot]`). `distance` is the encounter's
/// floored distance (drives chit/gear scaling) and `monster_count` the number of
/// creatures in the group. Pure — the caller owns the seed (server rolls it from
/// the instance seed ⊕ player ⊕ clock, like the Town Portal drop).
pub fn roll_creature_loot(
    balance: &Balance,
    distance: i64,
    monster_count: i32,
    seed: u64,
) -> CreatureLoot {
    let mut rng = Rng(seed);
    let sc = Scaling::new(balance);
    let l = &balance.loot;
    // Chits scale with monster level × encounter size, with symmetric jitter.
    let jitter = 1.0 + rng.signed() * l.chits_jitter;
    let chits = (l.chits_per_mlevel
        * sc.mlevel(distance) as f64
        * monster_count.max(1) as f64
        * jitter)
        .round()
        .max(0.0) as i64;
    let material = combat_material_for_biome(distance);
    // Red-chest gear only generates at/after the red-chest floor (tier 3, d 300).
    let gear = if distance >= balance.world_scaling.red_chest_floor_distance
        && rng.unit() < l.gear_drop_chance
    {
        let tier = sc.tier(distance) as i32;
        let slot = ["weapon", "armor", "accessory"][rng.below(3)];
        let gjitter = 1.0 + rng.signed() * l.gear_atk_jitter;
        let atk_bonus = (l.gear_atk_per_tier * tier as f64 * gjitter).round().max(1.0) as i32;
        Some(GearDrop {
            name: gear_name(distance, slot, &mut rng),
            slot: slot.to_string(),
            tier,
            atk_bonus,
            max_durability: l.gear_base_durability,
        })
    } else {
        None
    };
    CreatureLoot {
        chits,
        material,
        gear,
    }
}

/// splitmix64 finalizer — the mix used both by [`Rng`] and by [`section_seed`].
fn splitmix64(mut z: u64) -> u64 {
    z = z.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// Derive an independent, reproducible seed for section `n` from the run seed
/// (VERTICALITY-PROPOSAL.md "per-section seeds"). Each section is generated from
/// its OWN seed stream, so crossing into a new section is like dropping into a
/// fresh seed — endless variety as you go, identical again on the same run seed.
pub fn section_seed(run_seed: u64, n: usize) -> u64 {
    splitmix64(run_seed ^ (n as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15))
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
    /// Uniform in `[lo, hi)`.
    fn range(&mut self, lo: f64, hi: f64) -> f64 {
        lo + self.unit() * (hi - lo)
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

// ---------------------------------------------------------------- verticality ---

/// The kind of connector joining two elevation levels. Cliffs are always
/// impassable walls; a connector is the *only* way to change level.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectorKind {
    /// Walkable incline — you just walk up/down and your height interpolates.
    Slope,
    /// Vertical; mount the base and climb to the top level.
    Ladder,
    /// Like a ladder, flavoured for dropping down a cliff.
    Rope,
}

impl ConnectorKind {
    pub fn as_str(self) -> &'static str {
        match self {
            ConnectorKind::Slope => "slope",
            ConnectorKind::Ladder => "ladder",
            ConnectorKind::Rope => "rope",
        }
    }
}

/// A placed connector joining levels `lo`↔`hi`. Stepping within `radius` of it
/// (while on one of its two levels) lets the avatar move to the other level.
#[derive(Debug, Clone)]
pub struct Connector {
    pub entity_id: Id,
    pub kind: ConnectorKind,
    pub position: Position,
    pub lo: u8,
    pub hi: u8,
    pub radius: f64,
}

impl Connector {
    /// Does this connector join levels `a` and `b` (in either order)?
    fn joins(&self, a: u8, b: u8) -> bool {
        (self.lo == a && self.hi == b) || (self.lo == b && self.hi == a)
    }
}

/// The elevation field for one section: a coarse grid of integer levels over the
/// section's `[start_x, start_x + cols*cell) × [y_min, y_min + rows*cell)` extent,
/// plus the connectors that join levels. Row-major: `level[gx*rows + gy]`.
#[derive(Debug, Clone, Default)]
pub struct Terrain {
    pub start_x: f64,
    pub y_min: f64,
    pub cell: f64,
    pub cols: usize,
    pub rows: usize,
    pub level: Vec<u8>,
    pub connectors: Vec<Connector>,
}

impl Terrain {
    fn empty(start_x: f64, end_x: f64, y_min: f64, cell: f64) -> Self {
        let cols = (((end_x - start_x) / cell).ceil() as usize).max(1);
        let rows = ((((-y_min) * 2.0) / cell).ceil() as usize).max(1);
        Terrain {
            start_x,
            y_min,
            cell,
            cols,
            rows,
            level: vec![0; cols * rows],
            connectors: Vec::new(),
        }
    }

    fn cell_of(&self, p: &Position) -> Option<(usize, usize)> {
        if self.cell <= 0.0 || self.cols == 0 || self.rows == 0 {
            return None;
        }
        let gx = ((p.x - self.start_x) / self.cell).floor();
        let gy = ((p.y - self.y_min) / self.cell).floor();
        if gx < 0.0 || gy < 0.0 {
            return None;
        }
        let (gx, gy) = (gx as usize, gy as usize);
        if gx >= self.cols || gy >= self.rows {
            return None;
        }
        Some((gx, gy))
    }

    /// The elevation level at world position `p` (0 outside the grid).
    pub fn level_at(&self, p: &Position) -> u8 {
        match self.cell_of(p) {
            Some((gx, gy)) => self.level[gx * self.rows + gy],
            None => 0,
        }
    }

    /// World-space centre of cell `(gx, gy)`.
    fn cell_center(&self, gx: usize, gy: usize) -> Position {
        Position::new(
            self.start_x + (gx as f64 + 0.5) * self.cell,
            self.y_min + (gy as f64 + 0.5) * self.cell,
        )
    }
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
    /// Elevation level (terrace) the creature stands on. Creatures spawn on the
    /// ground (0); a fight only triggers when the toucher shares its elevation.
    pub elevation: u8,
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
            elevation: 0,
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
    /// Elevation the node sits on. A terrace node is only harvestable once you've
    /// climbed to it (rewards exploring the verticality).
    pub elevation: u8,
    pub harvested: bool,
}

/// One generated area / **section**: a stretch of corridor `[start_x, end_x)` in
/// one biome, holding the indices of its creatures (into [`Arena::monsters`]), a
/// portal, and its elevation [`Terrain`].
#[derive(Debug, Clone)]
pub struct Area {
    pub index: usize,
    pub biome: &'static str,
    pub start_x: f64,
    pub end_x: f64,
    pub monster_idxs: Vec<usize>,
    pub portal: Position,
    /// The section's elevation field (terraces + connectors).
    pub terrain: Terrain,
}

/// A hand-placed treasure chest. Walk up and open it once for a loot roll — chits,
/// materials, and deep-enough red gear — into the backpack, the overworld half of
/// the loot economy (economy.md S2 world loot).
#[derive(Debug, Clone)]
pub struct Chest {
    pub entity_id: Id,
    pub position: Position,
    /// Loot tier band at this depth (`tier(d) = floor(d/100)`), for loot scaling.
    pub tier: i32,
    pub opened: bool,
}

/// A biome boundary the player funnels through: a wall of impassable geo across
/// the corridor with a single **gap** (aligned to the guaranteed clear path). The
/// server enforces the wall (movement can only cross `x` inside the gap); the
/// client draws cliffs/water with the opening. Makes "cross into the next region"
/// a real, legible moment instead of an invisible distance threshold.
#[derive(Debug, Clone)]
pub struct Seam {
    /// Corridor x where the biome changes.
    pub x: f64,
    /// Centre-y of the passable gap.
    pub gap_y: f64,
    /// Half-width of the gap (passable band is `[gap_y - h, gap_y + h]`).
    pub gap_half_width: f64,
    pub biome_from: &'static str,
    pub biome_to: &'static str,
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

/// The clear path's y where it crosses `x` (linear interp between the waypoints
/// that straddle `x`); clamps to the endpoints outside the path's x-range.
fn path_y_at(path: &[Position], x: f64) -> f64 {
    if path.is_empty() {
        return 0.0;
    }
    for w in path.windows(2) {
        let (a, b) = (w[0], w[1]);
        let (lo, hi) = if a.x <= b.x { (a, b) } else { (b, a) };
        if x >= lo.x && x <= hi.x {
            let span = (hi.x - lo.x).max(1e-6);
            let t = (x - lo.x) / span;
            return lo.y + (hi.y - lo.y) * t;
        }
    }
    // Outside the path's x-range: use the nearest endpoint's y.
    if x <= path[0].x {
        path[0].y
    } else {
        path[path.len() - 1].y
    }
}

/// A player avatar on the overworld.
#[derive(Debug, Clone)]
pub struct Avatar {
    pub player_id: Id,
    pub position: Position,
    /// `active` | `in_battle` | `channeling` | `sleeping`.
    pub state: String,
    /// Elevation level the avatar currently stands on (changes only via connectors).
    pub elevation: u8,
    pub last_input_seq: u32,
    pub max_speed_tiles_per_sec: f64,
}

/// The generated overworld for one MazeInstance (spike scope): a seeded chain of
/// biome areas along a walkable corridor, streamed section-by-section on demand.
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
    /// Hand-placed treasure chests scattered through the sections.
    pub chests: Vec<Chest>,
    /// Biome-boundary chokepoints (a walled seam with one gap you pass through).
    pub seams: Vec<Seam>,
    /// The guaranteed-clear route from the hub to the portal, as waypoints. A tube
    /// of `path_clear_radius` around it holds no obstacles AND no raised terrace, so
    /// the exit is always reachable on level 0; the client draws it as a faint trail.
    pub path: Vec<Position>,
    /// The single fixed extraction portal, deep at the end of the initial chain.
    /// Extraction is otherwise the Town Portal item (works anywhere).
    pub portal: Position,
    pub avatars: Vec<Avatar>,
    /// East edge of generated content; grows as sections stream in.
    cursor: f64,
    /// Walkable bounds: `x ∈ [x_min, x_max]`, `y ∈ [-lateral, lateral]`.
    x_min: f64,
    x_max: f64,
    lateral: f64,
    /// The avatar's collision radius against obstacles.
    player_radius: f64,
    touch_radius: f64,
    interaction_radius: f64,
    sim_dt: f64,
    // World-gen tunables (snapshot from balance) needed for streaming.
    seed_base: u64,
    terrain_cell: f64,
    terraces_per_area: f64,
    max_level: u8,
    terrace_min_size: f64,
    terrace_max_size: f64,
    connector_radius: f64,
    path_clear_radius: f64,
    world_margin: f64,
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
    /// creatures, terraces, and portals (world-generation.md determinism invariant).
    /// Eagerly builds the initial `area_count`-section chain (so the deep portal +
    /// clear path are known at run start); further sections stream on demand via
    /// [`Arena::ensure_frontier`].
    pub fn generate(balance: &Balance, seed: u64) -> Self {
        let wg = &balance.worldgen;
        let mut arena = Arena {
            seed,
            areas: Vec::new(),
            monsters: Vec::new(),
            resources: Vec::new(),
            ground_loot: Vec::new(),
            obstacles: Vec::new(),
            chests: Vec::new(),
            seams: Vec::new(),
            path: vec![Position::new(0.0, 0.0)],
            portal: Position::new(0.0, 0.0),
            avatars: Vec::new(),
            cursor: 0.0,
            x_min: -4.0, // a little slack behind the hub
            x_max: 0.0,
            lateral: wg.lateral_half_extent,
            player_radius: wg.player_radius,
            touch_radius: balance.world.touch_radius_tiles,
            interaction_radius: balance.world.interaction_radius_tiles,
            sim_dt: 1.0 / balance.world.overworld_sim_hz as f64,
            seed_base: seed,
            terrain_cell: wg.terrain_cell,
            terraces_per_area: wg.terraces_per_area,
            max_level: wg.max_level,
            terrace_min_size: wg.terrace_min_size,
            terrace_max_size: wg.terrace_max_size,
            connector_radius: wg.connector_radius,
            path_clear_radius: wg.path_clear_radius,
            world_margin: wg.world_margin,
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
        };

        let count = wg.area_count.max(1);
        for i in 0..count {
            arena.push_section(balance, i);
        }
        // A single fixed extraction portal, deep at the end of the initial chain.
        arena.portal = arena
            .areas
            .get(count - 1)
            .map(|a| a.portal)
            .unwrap_or_else(|| Position::new(arena.cursor, 0.0));
        arena.x_max = arena.cursor + wg.world_margin;
        arena
    }

    /// Generate one more section if the frontier is within `stream_lookahead` of
    /// `player_x`. Sections beyond the initial chain are endless and reproducible
    /// (each from `section_seed(seed, n)`). Returns the indices of any sections
    /// newly created this call (so the caller can stream their terrain to clients).
    pub fn ensure_frontier(&mut self, balance: &Balance, player_x: f64) -> Vec<usize> {
        let mut created = Vec::new();
        let lookahead = balance.worldgen.stream_lookahead;
        // Cap growth per call so a teleport can't explode work in one tick.
        let mut budget = 4;
        while self.cursor < player_x + lookahead && budget > 0 {
            let i = self.areas.len();
            self.push_section(balance, i);
            self.x_max = self.cursor + self.world_margin;
            created.push(i);
            budget -= 1;
        }
        created
    }

    /// Build section `i` from its OWN seed (`section_seed`) and append it to the
    /// flat entity vectors + the path. Self-contained per section: no shared RNG
    /// state threads between sections, which is exactly what makes streaming and
    /// reproducibility work (VERTICALITY-PROPOSAL.md per-section seeds).
    fn push_section(&mut self, balance: &Balance, i: usize) {
        let wg = &balance.worldgen;
        let mut rng = Rng(section_seed(self.seed_base, i));
        let start_x = self.cursor;
        let biome = biome_for_distance(start_x.floor() as i64);
        let kinds = creatures_for_biome(biome);
        let mut area_idxs = Vec::new();

        // Area 0 is a small, deterministic "tutorial" section near the Center Hub:
        // exactly one canonical creature on the centre line and a portal a short
        // walk past it. Predictable onboarding (a straight east walk always meets
        // one fightable target, then a portal) — and the e2e/conformance tests
        // depend on this determinism. Procedural variety (and terraces) begin at 1.
        if i == 0 {
            let pos = Position::new(wg.first_monster_x, 0.0);
            let idx = self.monsters.len();
            let mseed = rng.next_u64();
            self.monsters
                .push(MonsterSpawn::build(balance, format!("mob-{idx}"), kinds[0], pos, mseed));
            area_idxs.push(idx);
            let portal_x = wg.first_monster_x + wg.first_area_portal_gap;
            let end_x = portal_x + wg.portal_setback;
            self.monsters[idx].area_min_x = start_x;
            self.monsters[idx].area_max_x = end_x;
            // A guaranteed starter resource node just off the tutorial path, so
            // the first thing a new player can safely do is harvest (no fight).
            self.resources.push(ResourceNode {
                entity_id: format!("res-{}", self.resources.len()),
                kind: resources_for_biome(biome)[0].to_string(),
                position: Position::new(wg.first_monster_x * 0.5, 3.0),
                elevation: 0,
                harvested: false,
            });
            // A guaranteed starter treasure chest opposite the node, so a new
            // player sees the loot loop (open → chits/materials) in area 0.
            let starter_chest_x = wg.first_monster_x * 0.5;
            self.chests.push(Chest {
                entity_id: format!("chest-{}", self.chests.len()),
                position: Position::new(starter_chest_x, -3.0),
                tier: Scaling::new(balance).tier(starter_chest_x.floor() as i64) as i32,
                opened: false,
            });
            self.areas.push(Area {
                index: i,
                biome,
                start_x,
                end_x,
                monster_idxs: area_idxs,
                portal: Position::new(portal_x, 0.0),
                // The tutorial section is entirely flat (level 0).
                terrain: Terrain::empty(start_x, end_x, -self.lateral, self.terrain_cell),
            });
            // The tutorial path is a straight, obstacle-free line to y=0.
            self.path.push(Position::new(end_x, 0.0));
            self.cursor = end_x;
            return;
        }

        // Procedural section. Length trends larger with depth (growth·i) plus a
        // per-section jitter, so sections differ in size and later ones are bigger
        // on average.
        let nominal = wg.base_area_length + wg.area_length_growth * i as f64;
        let length = (nominal * (1.0 + wg.area_length_jitter * rng.signed())).max(8.0);
        let end_x = start_x + length;

        // Walk the corridor placing creatures at jittered gaps. Creatures scatter
        // across ±y so the map is populated in every direction and you explore to
        // find fights.
        let inner_end = end_x - wg.portal_setback - 1.0;
        let mut x = start_x + 2.0;
        while x < inner_end {
            let kind = kinds[rng.below(kinds.len())];
            let y = wg.creature_lateral_spread * rng.signed();
            let pos = Position::new(x, y);
            let idx = self.monsters.len();
            let mseed = rng.next_u64();
            self.monsters
                .push(MonsterSpawn::build(balance, format!("mob-{idx}"), kind, pos, mseed));
            self.monsters[idx].area_min_x = start_x;
            self.monsters[idx].area_max_x = end_x;
            area_idxs.push(idx);

            let gap = wg.monster_spacing * (1.0 + wg.monster_spacing_jitter * rng.signed());
            x += gap.max(2.0);
        }

        // Scatter harvestable resource nodes through the section (2D, biome kinds).
        let rkinds = resources_for_biome(biome);
        let n_nodes = wg.resources_per_area.max(0.0).round() as usize;
        let mut section_resources: Vec<usize> = Vec::new();
        for _ in 0..n_nodes {
            let rk = rkinds[rng.below(rkinds.len())];
            let rx = start_x + 2.0 + rng.unit() * (length - 4.0).max(1.0);
            let ry = wg.resource_lateral_spread * rng.signed();
            let nid = self.resources.len();
            self.resources.push(ResourceNode {
                entity_id: format!("res-{nid}"),
                kind: rk.to_string(),
                position: Position::new(rx, ry),
                elevation: 0,
                harvested: false,
            });
            section_resources.push(nid);
        }

        // The clear path meanders to a fresh ±y at this section's end. The initial
        // chain's last section aims its final waypoint at the portal; streamed
        // sections just meander onward (endless). This completes the path segment
        // spanning the section, letting obstacles + terraces avoid the whole tube.
        let is_chain_end = i + 1 == wg.area_count.max(1);
        let portal = if is_chain_end {
            let p = Position::new(end_x - wg.portal_setback, 0.0);
            self.path.push(p);
            p
        } else {
            self.path.push(Position::new(end_x, wg.path_meander * rng.signed()));
            Position::new(end_x - wg.portal_setback, 0.0)
        };

        // Climbing maze (#B): the terrain for this section is created up front so a
        // plateau can be raised over the INTERIOR of the clear-path segment — the
        // critical route itself climbs up a ramp and back down. Endpoints (the
        // section waypoints) stay on level 0, so seams/portal/streaming and the
        // "waypoints are grounded" guarantee are untouched; only the mid-segment
        // rises. `maybe_climb_path` uses its own rng stream so the creature/obstacle/
        // terrace/chest draws below stay byte-stable.
        let mut terrain = Terrain::empty(start_x, end_x, -self.lateral, self.terrain_cell);
        self.maybe_climb_path(&mut terrain, i, wg.path_climb_chance);

        // Scatter impassable biome terrain, rejecting anything that would block the
        // clear path tube or bury a creature/resource. Rejection-sampled so the
        // path (and the exit) is always feasible by construction.
        let okinds = obstacles_for_biome(biome);
        let n_obs = wg.obstacles_per_area.max(0.0).round() as usize;
        let (mut placed, mut attempts) = (0usize, 0usize);
        while placed < n_obs && attempts < n_obs * 10 {
            attempts += 1;
            let ox = start_x + rng.unit() * length;
            let oy = rng.signed() * (self.lateral - 1.0);
            let radius =
                wg.obstacle_min_radius + rng.unit() * (wg.obstacle_max_radius - wg.obstacle_min_radius);
            let pos = Position::new(ox, oy);
            if dist_to_path(&pos, &self.path) < self.path_clear_radius + radius {
                continue;
            }
            // Don't strand an obstacle on (or half-buried under) the raised path
            // plateau — keep them on the ground like the dense-forest pass does.
            if terrain.level_at(&pos) != 0 {
                continue;
            }
            let buries = self.monsters.iter().any(|m| m.position.distance_to(&pos) < radius + 1.5)
                || self.resources.iter().any(|r| r.position.distance_to(&pos) < radius + 1.5);
            if buries {
                continue;
            }
            self.obstacles.push(Obstacle {
                entity_id: format!("obs-{}", self.obstacles.len()),
                kind: okinds[rng.below(okinds.len())].to_string(),
                position: pos,
                radius,
            });
            placed += 1;
        }

        // Raise a few SIDE terraces off the clear-path tube (optional detours: grind
        // pockets + treasure). Each gets a connector so it's reachable; overlapped
        // creatures/resources are lifted onto it (a reward for climbing). These are
        // kept off the path — the path's own climb is the plateau raised above.
        let n_terraces = self.terraces_per_area.max(0.0).round() as usize;
        let (mut tplaced, mut tattempts) = (0usize, 0usize);
        while tplaced < n_terraces && tattempts < n_terraces * 12 {
            tattempts += 1;
            let level: u8 = 1 + rng.below(self.max_level.max(1) as usize) as u8;
            let w = rng.range(self.terrace_min_size, self.terrace_max_size);
            let h = rng.range(self.terrace_min_size, self.terrace_max_size);
            let cx = start_x + rng.range(2.0, (length - 2.0).max(2.0));
            let cy = rng.range(-self.lateral + 2.0, self.lateral - 2.0);
            let (x0, x1) = (cx - w * 0.5, cx + w * 0.5);
            let (y0, y1) = (cy - h * 0.5, cy + h * 0.5);
            // Reject if any part of the terrace (+ a margin so the cliff edge itself
            // stays clear) intrudes on the path tube — keeps extraction on level 0.
            if self.rect_intrudes_path(x0, y0, x1, y1) {
                continue;
            }
            // Reject overlap with an already-raised terrace (no ambiguous stacking).
            if terrain_rect_overlaps(&terrain, x0, y0, x1, y1) {
                continue;
            }
            // Reject burying an obstacle (a raised cliff under a tree reads wrong).
            if self.obstacles.iter().any(|o| {
                o.position.x >= x0 - o.radius
                    && o.position.x <= x1 + o.radius
                    && o.position.y >= y0 - o.radius
                    && o.position.y <= y1 + o.radius
            }) {
                continue;
            }
            raise_terrace(&mut terrain, x0, y0, x1, y1, level);
            // Place a connector on the middle of the terrace's south edge, nudged
            // outward toward the ground so it straddles the level boundary.
            let conn_pos = Position::new(cx, (y0 - terrain.cell * 0.5).max(-self.lateral));
            // Ramps sell better: weight the connector roll toward slopes (½ slope,
            // ¼ ladder, ¼ rope). One draw either way, so the main rng stays aligned.
            let kind = match rng.below(4) {
                0 => ConnectorKind::Ladder,
                1 => ConnectorKind::Rope,
                _ => ConnectorKind::Slope,
            };
            terrain.connectors.push(Connector {
                entity_id: format!("conn-{}-{}", i, tplaced),
                kind,
                position: conn_pos,
                lo: 0,
                hi: level,
                radius: self.connector_radius,
            });
            // Any creature/resource sitting on this terrace is lifted onto it, so it
            // isn't stranded under a cliff (and rewards the climb).
            for m in self.monsters.iter_mut() {
                if terrain.level_at(&m.position) == level {
                    m.elevation = level;
                }
            }
            for &nid in &section_resources {
                if terrain.level_at(&self.resources[nid].position) == level {
                    self.resources[nid].elevation = level;
                }
            }
            tplaced += 1;
        }

        // One treasure chest per section, rejection-sampled to sit off the clear
        // path and not on top of a creature/resource (reachable, but a small
        // detour off the main line — old-school "explore for treasure").
        for attempt in 0..24 {
            let cx = start_x + 2.0 + rng.unit() * (length - 4.0).max(1.0);
            let cy = (wg.creature_lateral_spread - 2.0) * rng.signed();
            let cpos = Position::new(cx, cy);
            let clear_of_path = dist_to_path(&cpos, &self.path) > wg.path_clear_radius;
            let clear_of_mobs = self.monsters.iter().all(|m| m.position.distance_to(&cpos) > 2.0)
                && self.resources.iter().all(|r| r.position.distance_to(&cpos) > 2.0);
            if (clear_of_path && clear_of_mobs) || attempt == 23 {
                self.chests.push(Chest {
                    entity_id: format!("chest-{}", self.chests.len()),
                    position: cpos,
                    tier: Scaling::new(balance).tier(cx.floor() as i64) as i32,
                    opened: false,
                });
                break;
            }
        }

        // Biome-boundary chokepoints: if this section's span crosses a biome
        // boundary, wall the corridor with a single gap centred on the clear path,
        // so the player funnels through a visible "pass" into the next region.
        for &bd in &[100.0_f64, 300.0, 500.0, 1000.0, 3000.0] {
            if bd <= start_x || bd > end_x {
                continue;
            }
            let from = biome_for_distance((bd - 1.0).floor() as i64);
            let to = biome_for_distance(bd.floor() as i64);
            if from == to {
                continue;
            }
            self.seams.push(Seam {
                x: bd,
                gap_y: path_y_at(&self.path, bd),
                gap_half_width: wg.path_clear_radius,
                biome_from: from,
                biome_to: to,
            });
        }

        // Every biome is a MAZE: pack the play area with extra impassable props so
        // only the winding clear path (plus the branch detours) stays open. Forest is
        // densest (forest_obstacle_mult); other biomes use maze_obstacle_mult. Uses a
        // SEPARATE rng stream (section_seed ⊕ a constant) so main's creature/terrace/
        // chest/seam draws stay byte-identical and every determinism test still holds.
        // Ground level only (nothing floating on a terrace/plateau), and never buries
        // the path/creatures/nodes/chests.
        let maze_mult = if biome == "forest" {
            wg.forest_obstacle_mult
        } else {
            wg.maze_obstacle_mult
        };
        if maze_mult > 0.0 {
            let mut frng = Rng(section_seed(self.seed_base, i) ^ 0x7EE5_7EE5_7EE5_7EE5);
            let extra = (maze_mult * wg.obstacles_per_area).round().max(0.0) as usize;
            let fill_kind = if biome == "forest" { "tree" } else { okinds[0] };
            let (mut fp, mut fa) = (0usize, 0usize);
            while fp < extra && fa < extra * 12 {
                fa += 1;
                let ox = start_x + frng.unit() * length;
                let oy = frng.signed() * (self.lateral - 1.0);
                let radius = wg.obstacle_min_radius
                    + frng.unit() * (wg.obstacle_max_radius - wg.obstacle_min_radius);
                let pos = Position::new(ox, oy);
                if dist_to_path(&pos, &self.path) < self.path_clear_radius + radius {
                    continue;
                }
                if terrain.level_at(&pos) != 0 {
                    continue;
                }
                let occupied = self
                    .monsters
                    .iter()
                    .any(|m| m.position.distance_to(&pos) < radius + 1.2)
                    || self.resources.iter().any(|r| r.position.distance_to(&pos) < radius + 1.2)
                    || self.chests.iter().any(|c| c.position.distance_to(&pos) < radius + 1.2)
                    || self.obstacles.iter().any(|o| o.position.distance_to(&pos) < radius + o.radius);
                if occupied {
                    continue;
                }
                self.obstacles.push(Obstacle {
                    entity_id: format!("obs-{}", self.obstacles.len()),
                    kind: fill_kind.to_string(),
                    position: pos,
                    radius,
                });
                fp += 1;
            }
        }

        self.areas.push(Area {
            index: i,
            biome,
            start_x,
            end_x,
            monster_idxs: area_idxs,
            portal,
            terrain,
        });
        self.cursor = end_x;
    }

    /// Climbing maze (#B): with probability `climb_chance`, raise a plateau over the
    /// INTERIOR of this section's clear-path segment and drop a guaranteed Slope ramp
    /// at each level boundary, so the critical route itself climbs up and back down.
    ///
    /// Feasibility is preserved by construction:
    /// - the plateau spans only the interior (30–70%) of the segment, so both section
    ///   waypoints stay on level 0 (seams/portal/streaming + the grounded-waypoint
    ///   invariant are untouched);
    /// - the plateau's y-extent covers the whole path tube across that span (no cliff
    ///   cuts through the route);
    /// - a Slope connector joining 0↔level sits exactly on the path at each boundary,
    ///   wide enough (≥ path_clear_radius) that any walker in the tube can climb it.
    ///
    /// Uses its own rng stream so the main creature/obstacle/terrace/chest draws stay
    /// byte-stable.
    fn maybe_climb_path(&self, terrain: &mut Terrain, i: usize, climb_chance: f64) {
        if self.max_level < 1 || self.path.len() < 2 {
            return;
        }
        let mut prng = Rng(section_seed(self.seed_base, i) ^ 0x9A5E_9A5E_9A5E_9A5E);
        if prng.unit() >= climb_chance {
            return;
        }
        let from = self.path[self.path.len() - 2];
        let to = self.path[self.path.len() - 1];
        let dx = to.x - from.x;
        // Need room for a level-0 approach, the plateau, and two ramps.
        if dx <= 14.0 {
            return;
        }
        let level: u8 = 1 + prng.below(self.max_level.max(1) as usize) as u8;
        let px0 = from.x + dx * 0.30;
        let px1 = from.x + dx * 0.70;
        let y_at = |x: f64| from.y + (to.y - from.y) * (x - from.x) / dx;
        let (y0, y1) = (y_at(px0), y_at(px1));
        let clear = self.path_clear_radius + self.terrain_cell;
        raise_terrace(terrain, px0, y0.min(y1) - clear, px1, y0.max(y1) + clear, level);
        // Ramps ON the path at each boundary — always slopes, and wide enough that a
        // walker anywhere in the path tube crosses within reach.
        let ramp_r = (self.path_clear_radius + 0.5).max(self.connector_radius);
        for (k, (rx, ry)) in [(px0, y0), (px1, y1)].into_iter().enumerate() {
            terrain.connectors.push(Connector {
                entity_id: format!("pramp-{i}-{k}"),
                kind: ConnectorKind::Slope,
                position: Position::new(rx, ry),
                lo: 0,
                hi: level,
                radius: ramp_r,
            });
        }
    }

    /// Does the axis-aligned terrace rectangle come within the clear-path tube?
    /// Samples the rect corners + centre + edge midpoints against the path.
    fn rect_intrudes_path(&self, x0: f64, y0: f64, x1: f64, y1: f64) -> bool {
        let margin = self.path_clear_radius + self.terrain_cell;
        let (cx, cy) = ((x0 + x1) * 0.5, (y0 + y1) * 0.5);
        let samples = [
            Position::new(x0, y0),
            Position::new(x1, y0),
            Position::new(x0, y1),
            Position::new(x1, y1),
            Position::new(cx, y0),
            Position::new(cx, y1),
            Position::new(x0, cy),
            Position::new(x1, cy),
            Position::new(cx, cy),
        ];
        samples.iter().any(|s| dist_to_path(s, &self.path) < margin)
    }

    /// The elevation level at world position `p` — samples whichever section's
    /// terrain contains `p.x` (0 outside any section, e.g. behind the hub).
    pub fn level_at(&self, p: &Position) -> u8 {
        area_level_at(&self.areas, p)
    }

    /// Is there a connector within reach of `p` that joins levels `a`↔`b`? (A move
    /// crossing a level boundary is allowed only on such a connector.)
    fn connector_between(&self, p: &Position, a: u8, b: u8) -> bool {
        if a == b {
            return false;
        }
        self.areas.iter().any(|area| {
            area.terrain
                .connectors
                .iter()
                .any(|c| c.joins(a, b) && p.distance_to(&c.position) <= c.radius)
        })
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
        // Spatial hash of live creatures (by index into `cs`) so the skirmish-target
        // search is ~O(nearby) instead of scanning every creature per creature
        // (was O(monsters²), which grew unbounded as the endless world streamed in).
        // Cell = skirmish_aggro so a creature's aggro circle always fits inside its
        // own cell's 3×3 neighbourhood. Determinism is preserved: candidates are
        // tie-broken by (distance, index j), which reproduces the old `min_by` over
        // index-ordered iteration exactly, regardless of bucket visit order.
        let cell = skirmish_aggro.max(1.0);
        let cell_of = |p: &Position| ((p.x / cell).floor() as i32, (p.y / cell).floor() as i32);
        let mut grid: HashMap<(i32, i32), Vec<usize>> = HashMap::new();
        for (j, (pos, _, alive, _)) in cs.iter().enumerate() {
            if *alive {
                grid.entry(cell_of(pos)).or_default().push(j);
            }
        }
        // Immutable borrow of a disjoint field — safe alongside `monsters.iter_mut()`.
        let areas = &self.areas;

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
            // A creature that can't chase (passive) has no player/creature target,
            // so skip both O(players)/O(monsters) scans entirely — otherwise every
            // passive creature still walks the whole monster list each tick just to
            // produce `None`. (Same result, less work; behaviour unchanged.)
            let (player_target, creature_target) = if aggro_range > 0.0 {
                // Nearest active player within aggro range.
                let player_target = players
                    .iter()
                    .copied()
                    .filter(|p| m.position.distance_to(p) <= aggro_range)
                    .min_by(|a, b| m.position.distance_to(a).total_cmp(&m.position.distance_to(b)));
                // Nearest hostile-faction creature within skirmish aggro (initiators
                // only), found via the spatial grid: scan just this creature's 3×3
                // cell neighbourhood. Tie-break by (distance, index) to match the old
                // full index-ordered scan bit-for-bit.
                let (cx, cy) = cell_of(&m.position);
                let mut best: Option<(f64, usize, Position)> = None;
                for dx in -1..=1 {
                    for dy in -1..=1 {
                        let Some(bucket) = grid.get(&(cx + dx, cy + dy)) else { continue };
                        for &j in bucket {
                            if j == i {
                                continue;
                            }
                            let (pos, fac, _, _) = &cs[j];
                            if !creatures_hostile(&m.faction, fac) {
                                continue;
                            }
                            let d = m.position.distance_to(pos);
                            if d > skirmish_aggro {
                                continue;
                            }
                            let better = match best {
                                None => true,
                                Some((bd, bj, _)) => d < bd || (d == bd && j < bj),
                            };
                            if better {
                                best = Some((d, j, *pos));
                            }
                        }
                    }
                }
                let creature_target = best.map(|(_, _, pos)| pos);
                (player_target, creature_target)
            } else {
                (None, None)
            };
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
                // Creatures don't walk through terrain either (slide per axis), and
                // they stay on their own elevation (never wander off a terrace edge).
                let cand = Position::new(nx, ny);
                if !Self::obstacle_blocks(&obstacles, &cand, 0.5) && area_level_at(areas, &cand) == m.elevation {
                    m.position = cand;
                } else if !Self::obstacle_blocks(&obstacles, &Position::new(nx, m.position.y), 0.5)
                    && area_level_at(areas, &Position::new(nx, m.position.y)) == m.elevation
                {
                    m.position.x = nx;
                } else if !Self::obstacle_blocks(&obstacles, &Position::new(m.position.x, ny), 0.5)
                    && area_level_at(areas, &Position::new(m.position.x, ny)) == m.elevation
                {
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

    /// The living creatures within `group_radius` of creature `idx` **on the same
    /// elevation** (including it). This is the encounter you pull when you touch one
    /// — nearby creatures pile in; their factions decide who fights whom once in
    /// battle. Creatures on a different terrace don't join.
    pub fn group_around(&self, idx: usize) -> Vec<usize> {
        let Some(origin) = self.monsters.get(idx) else {
            return vec![];
        };
        let center = origin.position;
        let elev = origin.elevation;
        let r = self.group_radius;
        self.monsters
            .iter()
            .enumerate()
            .filter(|(_, m)| {
                !m.defeated && m.elevation == elev && center.distance_to(&m.position) <= r
            })
            .map(|(i, _)| i)
            .collect()
    }

    /// Is `player` within interaction range of the single deep extraction portal
    /// (on the ground — the portal sits on level 0)?
    pub fn at_portal(&self, player_id: &str) -> bool {
        let Some(a) = self.avatar(player_id) else {
            return false;
        };
        a.elevation == 0 && a.position.distance_to(&self.portal) <= self.interaction_radius
    }

    /// Harvest the resource node `entity_id` if `player` is within interaction
    /// range **on the same elevation** and it isn't already spent. Marks it
    /// harvested and returns its content kind (the caller maps that to a material +
    /// skill XP via balance).
    pub fn harvest(&mut self, player_id: &str, entity_id: &str) -> Option<String> {
        let (ppos, pelev) = {
            let a = self.avatar(player_id)?;
            (a.position, a.elevation)
        };
        let radius = self.interaction_radius;
        let node = self
            .resources
            .iter_mut()
            .find(|n| n.entity_id == entity_id && !n.harvested)?;
        if node.elevation != pelev || ppos.distance_to(&node.position) > radius {
            return None;
        }
        node.harvested = true;
        Some(node.kind.clone())
    }

    /// Open the treasure chest `entity_id` if `player` is within interaction range
    /// and it isn't already open. Marks it opened and returns `(tier, distance)`
    /// so the caller can roll its loot via balance.
    pub fn open_chest(&mut self, player_id: &str, entity_id: &str) -> Option<(i32, i64)> {
        let ppos = self.avatar(player_id)?.position;
        let radius = self.interaction_radius;
        let chest = self
            .chests
            .iter_mut()
            .find(|c| c.entity_id == entity_id && !c.opened)?;
        if ppos.distance_to(&chest.position) > radius {
            return None;
        }
        chest.opened = true;
        Some((chest.tier, chest.position.distance_floor()))
    }

    /// Walkable bounds `(x_min, x_max, lateral)` — the client frames the map (edge
    /// cliffs/water + end walls) from these so it reads as contained, not endless.
    pub fn bounds(&self) -> (f64, f64, f64) {
        (self.x_min, self.x_max, self.lateral)
    }

    /// Spawn a player avatar near the Center Hub (staggered so parties don't
    /// stack). All start on the y=0 corridor (level 0) so they can walk east.
    pub fn add_avatar(&mut self, player_id: String, speed: f64) {
        let idx = self.avatars.len();
        self.avatars.push(Avatar {
            player_id,
            position: Position::new(-(idx as f64) * 0.6, 0.0),
            state: "active".to_string(),
            elevation: 0,
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

    /// Integrate one movement intent against authoritative position, clamped to the
    /// world bounds and max speed, blocked by biome obstacles, and gated by
    /// elevation (server owns movement — CANON.md §S, D11). A candidate step is
    /// accepted only if it clears obstacles AND either stays on the current level or
    /// crosses a boundary via a **connector** (cliffs are impassable walls — there
    /// is no free climbing). Collisions/cliffs **slide**: the axis-aligned
    /// components are tried so you glide along terrain rather than sticking. Returns
    /// the authoritative position after integration.
    pub fn apply_move(
        &mut self,
        player_id: &str,
        dir_x: f64,
        dir_y: f64,
        input_seq: u32,
    ) -> Option<Position> {
        // Read the avatar's current state first (immutable) so the elevation/obstacle
        // math below can borrow `&self`; write the result back at the end.
        let (cur, cur_elev, state, speed) = {
            let a = self.avatar(player_id)?;
            (a.position, a.elevation, a.state.clone(), a.max_speed_tiles_per_sec)
        };
        if state != "active" {
            return Some(cur); // can't move while in battle/channeling/sleeping
        }
        let dt = self.sim_dt;
        let (x_min, x_max, lateral) = (self.x_min, self.x_max, self.lateral);
        let pr = self.player_radius;
        let obstacles: Vec<(Position, f64)> =
            self.obstacles.iter().map(|o| (o.position, o.radius)).collect();
        // (seam_x, gap_y, gap_half_width) — you may only cross a seam inside its gap.
        let seams: Vec<(f64, f64, f64)> =
            self.seams.iter().map(|s| (s.x, s.gap_y, s.gap_half_width)).collect();

        // Clamp direction magnitude to ≤ 1 (movement-world.md).
        let mag = (dir_x * dir_x + dir_y * dir_y).sqrt();
        let (nx, ny) = if mag > 1.0 {
            (dir_x / mag, dir_y / mag)
        } else {
            (dir_x, dir_y)
        };
        let step = speed * dt;
        let clamp =
            |x: f64, y: f64| Position::new(x.max(x_min).min(x_max), y.max(-lateral).min(lateral));

        // A candidate is acceptable iff it clears obstacles AND is level-permitted:
        // same level, or a connector joins the current & destination levels.
        let accept = |cand: Position| -> Option<u8> {
            if Self::obstacle_blocks(&obstacles, &cand, pr) {
                return None;
            }
            // Crossing a biome seam is only permitted inside its gap: reject a
            // candidate that would step over the seam's x off-gap (you must funnel
            // through the pass). Mirrors an impassable wall, so the slide logic runs.
            if seams
                .iter()
                .any(|&(sx, gy, gh)| (cur.x < sx) != (cand.x < sx) && (cand.y - gy).abs() > gh)
            {
                return None;
            }
            let cl = self.level_at(&cand);
            if cl == cur_elev
                || self.connector_between(&cur, cur_elev, cl)
                || self.connector_between(&cand, cur_elev, cl)
            {
                Some(cl)
            } else {
                None
            }
        };

        let full = clamp(cur.x + nx * step, cur.y + ny * step);
        let (dest, new_elev) = if let Some(l) = accept(full) {
            (full, l)
        } else {
            // Slide: try moving along only x, then only y.
            let sx = clamp(cur.x + nx * step, cur.y);
            let sy = clamp(cur.x, cur.y + ny * step);
            if let Some(l) = accept(sx) {
                (sx, l)
            } else if let Some(l) = accept(sy) {
                (sy, l)
            } else {
                (cur, cur_elev) // fully blocked
            }
        };

        let a = self.avatar_mut(player_id)?;
        a.position = dest;
        a.elevation = new_elev;
        a.last_input_seq = input_seq;
        Some(a.position)
    }

    /// The first **living** monster within touch range of an **active** (not
    /// already battling) avatar **on the same elevation**, as `(player_id,
    /// monster_index)`. Battling avatars are `in_battle`, so a hit is always a fresh
    /// toucher — the caller starts a battle or raid-merges into one. A monster one
    /// terrace up (or down) is not touchable until you climb to it.
    pub fn check_touch(&self) -> Option<(Id, usize)> {
        for a in self.avatars.iter().filter(|a| a.state == "active") {
            for (idx, m) in self.monsters.iter().enumerate() {
                // Skip creatures already locked in someone else's fight (`in_battle`)
                // so concurrent battles never fight over the same creature, and
                // creatures on another terrace until you climb to them.
                if !m.defeated
                    && !m.in_battle
                    && m.elevation == a.elevation
                    && a.position.distance_to(&m.position) <= self.touch_radius
                {
                    return Some((a.player_id.clone(), idx));
                }
            }
        }
        None
    }
}

/// The elevation level at world position `p` over a section list (free function
/// so it can be called while another field of the arena is mutably borrowed).
fn area_level_at(areas: &[Area], p: &Position) -> u8 {
    for a in areas {
        if p.x >= a.start_x && p.x < a.end_x {
            return a.terrain.level_at(p);
        }
    }
    0
}

/// Do any cells of the axis-aligned rect `[x0,x1]×[y0,y1]` already hold a raised
/// (level > 0) terrace? Used to reject overlapping terraces.
fn terrain_rect_overlaps(t: &Terrain, x0: f64, y0: f64, x1: f64, y1: f64) -> bool {
    let gx0 = (((x0 - t.start_x) / t.cell).floor().max(0.0)) as usize;
    let gy0 = (((y0 - t.y_min) / t.cell).floor().max(0.0)) as usize;
    let gx1 = ((((x1 - t.start_x) / t.cell).ceil()) as usize).min(t.cols);
    let gy1 = ((((y1 - t.y_min) / t.cell).ceil()) as usize).min(t.rows);
    for gx in gx0..gx1 {
        for gy in gy0..gy1 {
            if gx < t.cols && gy < t.rows && t.level[gx * t.rows + gy] > 0 {
                return true;
            }
        }
    }
    false
}

/// Mark every cell whose centre falls inside `[x0,x1]×[y0,y1]` to `level`.
fn raise_terrace(t: &mut Terrain, x0: f64, y0: f64, x1: f64, y1: f64, level: u8) {
    for gx in 0..t.cols {
        for gy in 0..t.rows {
            let c = t.cell_center(gx, gy);
            if c.x >= x0 && c.x <= x1 && c.y >= y0 && c.y <= y1 {
                t.level[gx * t.rows + gy] = level;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loot_is_deterministic_and_scales_with_depth() {
        let b = Balance::load_default().unwrap();
        // Same seed ⇒ identical loot (pure function).
        assert_eq!(
            roll_creature_loot(&b, 50, 1, 12345),
            roll_creature_loot(&b, 50, 1, 12345)
        );
        // Forest keeps the crafting/conformance material id.
        assert_eq!(roll_creature_loot(&b, 10, 1, 1).material, "forest_bloom_petal");
        // Deeper fights pay more chits on average (sample a few seeds).
        let shallow: i64 = (0..16).map(|s| roll_creature_loot(&b, 40, 1, s).chits).sum();
        let deep: i64 = (0..16).map(|s| roll_creature_loot(&b, 800, 1, s).chits).sum();
        assert!(deep > shallow, "deeper creatures should drop more chits");
    }

    #[test]
    fn red_gear_never_drops_below_the_red_chest_floor() {
        let b = Balance::load_default().unwrap();
        let floor = b.world_scaling.red_chest_floor_distance;
        // Below the floor: no gear across many seeds.
        for s in 0..200 {
            assert!(roll_creature_loot(&b, floor - 1, 1, s).gear.is_none());
        }
        // At/after the floor: gear does appear for some seeds, at the right tier.
        let mut saw_gear = false;
        for s in 0..200 {
            if let Some(g) = roll_creature_loot(&b, floor, 1, s).gear {
                saw_gear = true;
                assert_eq!(g.tier, 3, "tier(300) = 3");
                assert!(g.atk_bonus >= 1 && g.max_durability > 0);
                assert!(["weapon", "armor", "accessory"].contains(&g.slot.as_str()));
            }
        }
        assert!(saw_gear, "red gear should drop at/after the floor for some seeds");
    }

    #[test]
    fn generates_chests_and_biome_seams() {
        let b = Balance::load_default().unwrap();
        let arena = Arena::generate(&b, 7);
        assert!(!arena.chests.is_empty(), "chests are placed");
        assert!(arena.chests.iter().all(|c| !c.opened));
        // The default world reaches the desert (d > 100), so at least a
        // forest→desert seam exists with a positive gap.
        assert!(!arena.seams.is_empty(), "biome seam(s) generated");
        assert!(arena
            .seams
            .iter()
            .any(|s| s.biome_from == "forest" && s.biome_to == "desert"));
        assert!(arena.seams.iter().all(|s| s.gap_half_width > 0.0));
    }

    #[test]
    fn seam_wall_blocks_crossing_outside_the_gap() {
        let b = Balance::load_default().unwrap();
        let mut arena = Arena::generate(&b, 7);
        let seam = arena.seams[0].clone();
        arena.add_avatar("p".into(), 100.0); // fast: one step would cross the seam
        // Far from the gap in y → the wall blocks the crossing.
        let off_y = (seam.gap_y + seam.gap_half_width + 6.0).min(arena.lateral - 1.0);
        arena.avatar_mut("p").unwrap().position = Position::new(seam.x - 0.5, off_y);
        let after = arena.apply_move("p", 1.0, 0.0, 1).unwrap();
        assert!(after.x < seam.x, "blocked away from the gap (x={})", after.x);
        // Lined up with the gap → the crossing is allowed.
        arena.avatar_mut("p").unwrap().position = Position::new(seam.x - 0.5, seam.gap_y);
        let after2 = arena.apply_move("p", 1.0, 0.0, 2).unwrap();
        assert!(after2.x >= seam.x, "can pass through the gap (x={})", after2.x);
    }

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
        // Park the second creature right next to the first (same elevation).
        arena.monsters[1].position = arena.monsters[0].position;
        arena.monsters[1].elevation = arena.monsters[0].elevation;
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
        // A different seed yields a different world (overwhelmingly likely). Compare
        // procedural content — monsters[0] is the fixed tutorial creature, identical
        // across seeds by design, so look past it (and at the terraces).
        let d = Arena::generate(&b, 999);
        let monsters_differ = a.monsters.len() != d.monsters.len()
            || a.monsters.iter().zip(d.monsters.iter()).any(|(m, n)| m.position != n.position);
        let terrain_differs = a
            .areas
            .iter()
            .zip(d.areas.iter())
            .any(|(x, y)| x.terrain.level != y.terrain.level);
        assert!(monsters_differ || terrain_differs, "different seeds → different worlds");
    }

    #[test]
    fn sections_are_independently_seeded_and_reproducible() {
        // Per-section seeds: section n depends only on (run_seed, n), so the SAME
        // section reproduces exactly and is independent of its neighbours.
        assert_eq!(section_seed(42, 3), section_seed(42, 3));
        assert_ne!(section_seed(42, 3), section_seed(42, 4));
        assert_ne!(section_seed(42, 3), section_seed(43, 3));
        // Two arenas from the same run seed produce identical terraces per section.
        let b = Balance::load_default().unwrap();
        let a = Arena::generate(&b, 77);
        let c = Arena::generate(&b, 77);
        for (x, y) in a.areas.iter().zip(c.areas.iter()) {
            assert_eq!(x.terrain.level, y.terrain.level);
            assert_eq!(x.terrain.connectors.len(), y.terrain.connectors.len());
        }
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
        // The single extraction portal is the last chain area's, deep from the hub.
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
        // Use the guaranteed level-0 starter node (area 0) so elevation doesn't gate.
        let node = arena.resources[0].clone();
        assert_eq!(node.elevation, 0);
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
        // Area 0 is entirely flat.
        assert!(arena.areas[0].terrain.level.iter().all(|&l| l == 0), "area 0 is flat");
        for o in &arena.obstacles {
            assert!(o.radius > 0.0);
        }
    }

    #[test]
    fn terraces_generate_with_reachable_connectors() {
        let b = Balance::load_default().unwrap();
        let arena = Arena::generate(&b, 7);
        // Some section beyond the tutorial has a raised terrace.
        let raised: usize = arena
            .areas
            .iter()
            .map(|a| a.terrain.level.iter().filter(|&&l| l > 0).count())
            .sum();
        assert!(raised > 0, "verticality: at least one terrace is raised");
        // Every raised level present in a section has a connector joining it to 0.
        for area in &arena.areas {
            let mut levels: Vec<u8> = area.terrain.level.iter().copied().filter(|&l| l > 0).collect();
            levels.sort_unstable();
            levels.dedup();
            for lvl in levels {
                assert!(
                    area.terrain.connectors.iter().any(|c| c.joins(0, lvl)),
                    "section {} level {lvl} has no connector to the ground",
                    area.index
                );
            }
        }
    }

    #[test]
    fn no_obstacle_or_terrace_intrudes_on_the_clear_path() {
        // The feasibility guarantee: the path tube holds no obstacle AND stays on
        // level 0, so extraction is always feasible without ever needing to climb.
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
            // Every path waypoint is on the ground.
            for wp in &arena.path {
                assert_eq!(arena.level_at(wp), 0, "seed {seed}: path waypoint left level 0");
            }
        }
    }

    #[test]
    fn the_clear_path_actually_reaches_the_portal() {
        // A walker that follows the waypoints reaches the portal without getting
        // stuck on terrain or a cliff — the route is feasible by construction.
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
        assert_eq!(arena.avatar("p").unwrap().elevation, 0, "walker stayed on the ground");
    }

    #[test]
    fn the_clear_path_climbs_a_plateau_and_still_reaches_the_portal() {
        // The #B guarantee: the critical route itself CLIMBS (up a ramp, across a
        // plateau, back down) yet is still always completable, ending grounded at the
        // portal. Pick a seed that actually generated a path-ramp, then walk it.
        let b = Balance::load_default().unwrap();
        let seed = [42u64, 1, 7, 999, 123456, 2, 3, 5, 11]
            .into_iter()
            .find(|&s| {
                Arena::generate(&b, s).areas.iter().any(|a| {
                    a.terrain.connectors.iter().any(|c| c.entity_id.starts_with("pramp-"))
                })
            })
            .expect("some seed produces a climbing clear path");
        let mut arena = Arena::generate(&b, seed);
        let waypoints = arena.path.clone();
        assert!(waypoints.len() >= 2);
        arena.add_avatar("p".into(), 8.0);
        let mut wp = 1usize;
        let mut reached = false;
        let mut max_elev = 0u8;
        for _ in 0..80_000 {
            let a = arena.avatar("p").unwrap();
            max_elev = max_elev.max(a.elevation);
            let pos = a.position;
            let target = waypoints[wp];
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
        assert!(reached, "seed {seed}: the climbing path still reaches the portal");
        assert!(max_elev > 0, "seed {seed}: the walker actually climbed a plateau en route");
        assert_eq!(
            arena.avatar("p").unwrap().elevation,
            0,
            "seed {seed}: walker ends grounded at the portal"
        );
    }

    #[test]
    fn a_cliff_blocks_but_a_connector_lets_you_climb() {
        // Find a raised terrace, prove you can't walk onto it across the cliff, then
        // prove stepping onto its connector carries you up.
        let b = Balance::load_default().unwrap();
        let mut arena = Arena::generate(&b, 7);
        let (conn, level) = arena
            .areas
            .iter()
            .flat_map(|a| a.terrain.connectors.iter().map(move |c| (c.clone(), c.hi)))
            .next()
            .expect("a connector exists");
        arena.add_avatar("p".into(), 6.0);

        // Approach the terrace from open ground, away from the connector: pick a
        // raised cell far from the connector and try to walk straight into it.
        let area = arena.areas.iter().find(|a| !a.terrain.connectors.is_empty()).unwrap();
        let mut target_cell = None;
        for gx in 0..area.terrain.cols {
            for gy in 0..area.terrain.rows {
                if area.terrain.level[gx * area.terrain.rows + gy] == level {
                    let c = area.terrain.cell_center(gx, gy);
                    if c.distance_to(&conn.position) > conn.radius + 3.0 {
                        target_cell = Some(c);
                    }
                }
            }
        }
        if let Some(cell) = target_cell {
            // Stand just off the terrace, not near the connector, and push into it.
            let start = Position::new(cell.x, area.terrain.y_min - 0.1); // below-grid ground
            let start = if arena.level_at(&start) == 0 { start } else { Position::new(cell.x, cell.y - 6.0) };
            arena.avatar_mut("p").unwrap().position = start;
            arena.avatar_mut("p").unwrap().elevation = 0;
            for _ in 0..80 {
                let p = arena.avatar("p").unwrap().position;
                arena.apply_move("p", cell.x - p.x, cell.y - p.y, 0);
            }
            assert_eq!(
                arena.avatar("p").unwrap().elevation,
                0,
                "a bare cliff must not let you climb"
            );
        }

        // Now use the connector: stand on it and step up onto the terrace.
        arena.avatar_mut("p").unwrap().position = conn.position;
        arena.avatar_mut("p").unwrap().elevation = 0;
        let up = Position::new(conn.position.x, conn.position.y + arena.connector_radius + 1.0);
        for _ in 0..40 {
            let p = arena.avatar("p").unwrap().position;
            arena.apply_move("p", up.x - p.x, up.y - p.y, 0);
            if arena.avatar("p").unwrap().elevation == level {
                break;
            }
        }
        assert_eq!(
            arena.avatar("p").unwrap().elevation,
            level,
            "stepping onto a connector should carry you up a level"
        );
    }

    #[test]
    fn streaming_extends_the_world_endlessly_and_reproducibly() {
        let b = Balance::load_default().unwrap();
        let mut a = Arena::generate(&b, 55);
        let chain = a.areas.len();
        // Walking the frontier east streams in fresh sections beyond the chain.
        let created = a.ensure_frontier(&b, a.areas.last().unwrap().end_x + 100.0);
        assert!(!created.is_empty(), "frontier advance streams new sections");
        assert!(a.areas.len() > chain, "world grew past the initial chain");
        // The deep portal does NOT move when streaming past it.
        assert_eq!(a.portal, a.areas[chain - 1].portal);
        // Reproducible: a second arena streamed the same way matches section-for-section.
        let mut c = Arena::generate(&b, 55);
        c.ensure_frontier(&b, c.areas.last().unwrap().end_x + 100.0);
        assert_eq!(a.areas.len(), c.areas.len());
        for (x, y) in a.areas.iter().zip(c.areas.iter()) {
            assert_eq!(x.start_x, y.start_x);
            assert_eq!(x.terrain.level, y.terrain.level);
        }
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
