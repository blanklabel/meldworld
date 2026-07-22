//! Overworld model for the spike (docs/behaviors/world-generation.md subset).
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
//! **Per-section seeds & streaming** (docs/proposals/verticality.md): each section `n`
//! is generated from its OWN derived seed `section_seed(run_seed, n)`, so sections
//! are independent (one section's RNG draws can't perturb another's) and any single
//! section reproduces exactly from `(run_seed, n)`. Sections are generated
//! **on demand** as the player advances ([`Arena::ensure_frontier`]) — the world is
//! endless, always fresh as you go deeper, and identical again on the same seed.
//! This is the deferred "chunk streaming" landing as the procedural core.
//!
//! **Verticality** (docs/proposals/verticality.md): elevation is a small number of
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
/// The *tutorial* run walks these in order for a gentle, known onboarding; every
/// other run draws its biomes per-section (see [`section_biome`]).
pub fn biome_for_distance(d: i64) -> &'static str {
    match d {
        0..=99 => "forest",
        100..=299 => "desert",
        300..=499 => "ashfall",
        500..=999 => "tundra",
        _ => "mire",
    }
}

/// The base biome set. Difficulty is carried entirely by `distance` (creature
/// stats scale via `stat_mult` at spawn), so a biome is a difficulty-neutral
/// **skin** — which is exactly what lets us vary the theme order per run without
/// touching fairness. This is the Hades / Risk-of-Rain-2 model: fixed difficulty
/// axis, shuffled theme. See docs/proposals/worldgen-wg.md and roadmap WG-2/WG-3.
pub const BIOMES: [&str; 5] = ["forest", "desert", "ashfall", "tundra", "mire"];

/// Independent per-section biome stream, salted off the section seed so the theme
/// choice is stable even if unrelated placement draws change.
fn biome_pick_seed(run_seed: u64, i: usize) -> u64 {
    section_seed(run_seed ^ 0x1D8E_4E27_C47D_124F, i).wrapping_add(0xB105_F00D)
}

/// The biome THEME for section `i` at `distance`.
/// - **Tutorial run** → the classic distance-ordered bands ([`biome_for_distance`]),
///   so a new player's first dive is the hand-tuned Forest→Desert→… progression.
/// - **Any other run** → a per-section draw (WG-3: the order varies every run;
///   WG-2: the *first* section is randomized too, so you don't always start in
///   Forest), excluding the previous section's biome so two identical themes never
///   sit back-to-back. Uniform for this first pass; per-band weighting can layer on
///   later without changing callers.
fn section_biome(run_seed: u64, i: usize, distance: i64, prev: Option<&str>, tutorial: bool) -> &'static str {
    if tutorial {
        return biome_for_distance(distance);
    }
    let cands: Vec<&'static str> = BIOMES.iter().copied().filter(|b| Some(*b) != prev).collect();
    let mut rng = Rng(biome_pick_seed(run_seed, i));
    cands[rng.below(cands.len())]
}

/// Creature content ids that spawn in a biome. Structural (content-extensible);
/// stats for each key live in `balance.toml` under `[creature.<key>]`.
fn creatures_for_biome(biome: &str) -> &'static [&'static str] {
    // Each biome's 3rd creature is a distinct archetype — a fast aggressive SWARMER
    // or a slow tanky BRUISER — so the combat rhythm varies as you explore. Appended
    // (index 0 stays the tutorial creature). Stats live under `[creature.<key>]`.
    match biome {
        "forest" => &["forest_bloom_stalker", "thornback_boar", "sporeling"],
        "desert" => &["dune_wyrm", "sand_shade", "dune_colossus"],
        "ashfall" => &["cinder_imp", "magma_golem", "ember_wisp"],
        "tundra" => &["frost_lurker", "ice_revenant", "glacier_maw"],
        _ => &["bog_serpent", "myconid_brute", "bog_stinger"],
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
/// Each slot carries exactly one relevant stat — weapon → atk, armor → def,
/// accessory → spd — the other two stay 0 (no secondary stats/sockets yet).
#[derive(Debug, Clone, PartialEq)]
pub struct GearDrop {
    pub name: String,
    /// Rarity tier: common/rare/epic/legendary (scales the stat + flavours the name).
    pub rarity: String,
    pub slot: String,
    pub tier: i32,
    pub atk_bonus: i32,
    pub def_bonus: i32,
    pub spd_bonus: i32,
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

/// Named gear catalog — 20 curated items per slot, ordered weakest → strongest
/// (economy.md S1 content pass). A drop's name is picked by its tier, indexed
/// from the red-chest floor tier and clamped into the catalog range, so the
/// *name* rides the same distance-driven power curve `roll_creature_loot`
/// already uses for the numeric stat — a shallow kill can't hand out a name
/// that reads as endgame gear, and a deep one won't hand out a name that reads
/// as starter junk.
const WEAPON_NAMES: [&str; 20] = [
    "Ashfall Shortsword",
    "Cinderforged Cleaver",
    "Emberwrought Warpick",
    "Scarab Fang Blade",
    "Duneglass Broadsword",
    "Sunbaked Warhammer",
    "Rimebound Longsword",
    "Frostforged Battleaxe",
    "Glacial Warpick",
    "Verdant Greatblade",
    "Bloomforged Cleaver",
    "Thornwood Reaver",
    "Miremere Scythe",
    "Fungal Ripper",
    "Peatbound Warblade",
    "Ashen Doomblade",
    "Stormcaller's Edge",
    "Voidforged Greatblade",
    "Ancient Worldbreaker",
    "Eternal Starfall Edge",
];

const ARMOR_NAMES: [&str; 20] = [
    "Ashfall Cuirass",
    "Cinderforged Plate",
    "Emberwrought Carapace",
    "Scarab Shell Armor",
    "Duneglass Aegis",
    "Sunbaked Bulwark",
    "Rimebound Plate",
    "Frostforged Aegis",
    "Glacial Carapace",
    "Verdant Aegis",
    "Bloomforged Plate",
    "Thornwood Carapace",
    "Miremere Plate",
    "Fungal Husk Armor",
    "Peatbound Aegis",
    "Ashen Bulwark",
    "Stormguard Mantle",
    "Voidforged Plate",
    "Ancient Warplate",
    "Eternal Aegis of the Deep",
];

const ACCESSORY_NAMES: [&str; 20] = [
    "Ashfall Charm",
    "Cinderforged Sigil",
    "Emberwrought Band",
    "Scarab Talisman",
    "Duneglass Amulet",
    "Sunbaked Ring",
    "Rimebound Sigil",
    "Frostforged Band",
    "Glacial Talisman",
    "Verdant Charm",
    "Bloomforged Sigil",
    "Thornwood Band",
    "Miremere Talisman",
    "Fungal Charm",
    "Peatbound Sigil",
    "Ashen Relic",
    "Stormcaller's Pendant",
    "Voidforged Sigil",
    "Ancient Relic",
    "Eternal Starfall Amulet",
];

/// Look up a drop's flavor name: index into the slot's 20-item catalog by how
/// many tiers past `floor_tier` (the red-chest floor's tier — the earliest a
/// drop can ever roll) this drop's tier is, clamped to the catalog's range.
fn gear_catalog_name(slot: &str, tier: i32, floor_tier: i32) -> &'static str {
    let names: &[&str; 20] = match slot {
        "weapon" => &WEAPON_NAMES,
        "armor" => &ARMOR_NAMES,
        _ => &ACCESSORY_NAMES,
    };
    let idx = (tier - floor_tier).clamp(0, names.len() as i32 - 1) as usize;
    names[idx]
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
    loot_mult: f64,
    seed: u64,
) -> CreatureLoot {
    let mut rng = Rng(seed);
    let sc = Scaling::new(balance);
    let l = &balance.loot;
    // Chits scale with monster level × encounter size, with symmetric jitter. The
    // `loot_mult` is the encounter reward spike (1.0 standard; > 1 for elites /
    // gatekeepers — it fattens the chit haul and the gear-drop chance, FS-4).
    let jitter = 1.0 + rng.signed() * l.chits_jitter;
    let chits = (l.chits_per_mlevel
        * sc.mlevel(distance) as f64
        * monster_count.max(1) as f64
        * jitter
        * loot_mult.max(0.0))
        .round()
        .max(0.0) as i64;
    let material = combat_material_for_biome(distance);
    // Red-chest gear only generates at/after the red-chest floor
    // (`world_scaling.red_chest_floor_distance`; content-tunable, currently 0
    // — gear can drop from the very first kill); a reward spike (loot_mult)
    // boosts (and can guarantee) the drop.
    let gear = if distance >= balance.world_scaling.red_chest_floor_distance
        && rng.unit() < (l.gear_drop_chance * loot_mult.max(0.0)).min(1.0)
    {
        let tier = sc.tier(distance) as i32;
        let floor_tier = sc.tier(balance.world_scaling.red_chest_floor_distance) as i32;
        let slot = ["weapon", "armor", "accessory"][rng.below(3)];
        let gjitter = 1.0 + rng.signed() * l.gear_atk_jitter;
        // Rarity: the encounter's loot spike multiplies the rare/epic/legendary
        // odds (so elites/gatekeepers drop the shiny stuff), capped so Common is
        // always possible. Rarity then scales the stat bonus + flavours the name.
        let gr = &balance.gear_rarity;
        let boost = loot_mult.max(1.0);
        let (mut w_rare, mut w_epic, mut w_leg) =
            (gr.rare_weight * boost, gr.epic_weight * boost, gr.legendary_weight * boost);
        let noncommon = w_rare + w_epic + w_leg;
        if noncommon > 0.95 {
            let k = 0.95 / noncommon;
            w_rare *= k;
            w_epic *= k;
            w_leg *= k;
        }
        let u = rng.unit();
        let (rarity, rarity_mult) = if u < w_leg {
            ("legendary", gr.legendary_mult)
        } else if u < w_leg + w_epic {
            ("epic", gr.epic_mult)
        } else if u < w_leg + w_epic + w_rare {
            ("rare", gr.rare_mult)
        } else {
            ("common", 1.0)
        };
        // One roll, routed into whichever stat this slot cares about: weapon
        // hits harder, armor shrugs off more, an accessory moves faster.
        let stat =
            (l.gear_atk_per_tier * tier as f64 * gjitter * rarity_mult).round().max(1.0) as i32;
        let (atk_bonus, def_bonus, spd_bonus) = match slot {
            "weapon" => (stat, 0, 0),
            "armor" => (0, stat, 0),
            _ => (0, 0, stat),
        };
        // The catalog name already rides the depth curve (see `gear_catalog_name`);
        // rarity prefixes it on top ("Legendary Ashfall Shortsword") rather than
        // picking a separate biome-adjective name, so depth and rarity both read
        // in the same name instead of fighting each other.
        let base_name = gear_catalog_name(slot, tier, floor_tier).to_string();
        let name = if rarity == "common" {
            base_name
        } else {
            // Title-case the rarity for the name ("Legendary Frostforged Greatblade").
            let mut c = rarity.chars();
            let cap = c.next().unwrap().to_uppercase().collect::<String>() + c.as_str();
            format!("{cap} {base_name}")
        };
        Some(GearDrop {
            name,
            rarity: rarity.to_string(),
            slot: slot.to_string(),
            tier,
            atk_bonus,
            def_bonus,
            spd_bonus,
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
/// (docs/proposals/verticality.md "per-section seeds"). Each section is generated from
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
    /// FS-4: an Elite's affix (Swift/Brutal/Armored/Giant/Vicious), empty otherwise.
    /// Shown as a prefix on the battle name so the champion reads distinctly.
    pub affix: String,
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
    /// Full HP at spawn (= `hp` before any damage). Overworld mobs lose `hp` to
    /// hostile-faction skirmishes, so `hp/max_hp` is a meaningful pre-fight bar
    /// (surfaced to the client for the Hunter's HP-intel perk).
    pub max_hp: i32,
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
            affix: String::new(),
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
            max_hp: ((stats.base_hp as f64) * mult).round() as i32,
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

    /// Promote a fresh standard spawn to an Elite champion or a Gatekeeper boss
    /// (FS-4): scale its HP/atk/XP and tag the encounter class — which drives the
    /// loot multiplier on the kill, the battle merge cap, and the client's size +
    /// tint. Call once, on a standard spawn.
    fn promote(&mut self, hp_mult: f64, atk_mult: f64, xp_mult: f64, class: &str) {
        self.max_hp = ((self.max_hp as f64) * hp_mult).round().max(1.0) as i32;
        self.hp = self.max_hp;
        self.atk = ((self.atk as f64) * atk_mult).round().max(1.0) as i32;
        self.xp_reward = ((self.xp_reward as f64) * xp_mult).round().max(0.0) as i64;
        self.encounter_class = class.to_string();
    }

    /// Roll and apply one champion AFFIX (FS-4) — a stat-twist that makes every
    /// elite/gatekeeper fight feel different: a Swift pack acts far more often, an
    /// Armored one shrugs off blows, a Giant is a sponge, a Brutal/Vicious one hits
    /// like a truck. Pure stat mods that carry straight into the battle Fighter, plus
    /// a name prefix the client shows.
    fn apply_affix(&mut self, seed: u64) {
        // (name, hp_mult, atk_mult, def_add, speed_mult)
        let affixes: [(&str, f64, f64, i32, f64); 5] = [
            ("Swift", 1.0, 1.0, 0, 1.6),
            ("Brutal", 1.0, 1.4, 0, 1.0),
            ("Armored", 1.15, 1.0, 8, 1.0),
            ("Giant", 1.5, 1.0, 0, 0.85),
            ("Vicious", 1.0, 1.25, 0, 1.25),
        ];
        let (name, hp_m, atk_m, def_add, spd_m) = affixes[(seed % affixes.len() as u64) as usize];
        self.max_hp = ((self.max_hp as f64) * hp_m).round().max(1.0) as i32;
        self.hp = self.max_hp;
        self.atk = ((self.atk as f64) * atk_m).round().max(1.0) as i32;
        self.def += def_add;
        self.speed_stat = ((self.speed_stat as f64) * spd_m).round().max(1.0) as i32;
        self.affix = name.to_string();
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
    pub portal: Position,
    /// The section's elevation field (terraces + connectors).
    pub terrain: Terrain,
    /// WG-1: this section is a dungeon (rooms divided by walls with a door on the
    /// clear path, denser creatures, a guaranteed loot chest). Flat (no terraces).
    pub dungeon: bool,
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
    /// Elevation level the chest sits at (0 = ground). A chest atop a terrace can
    /// only be opened from that level — the reward for climbing the detour.
    pub elevation: u8,
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
    /// Frontier of generated content in **corridor** space (= max radius in radial
    /// mode). Grows as sections stream in.
    cursor: f64,
    /// Walkable bounds: `x ∈ [x_min, x_max]`, `y ∈ [-lateral, lateral]`. In radial
    /// mode these are the fan's bounding box, NOT the corridor extent.
    x_min: f64,
    x_max: f64,
    lateral: f64,
    /// WG-4 radial world: half the arc in radians (0.0 ⇒ corridor mode, no bend).
    /// Set once at generation; drives both the initial bend and outward streaming.
    radial_half: f64,
    /// The corridor y half-extent used for the arc angle mapping and for placing a
    /// section's content. Preserved even after `radialize` widens `lateral` to the
    /// fan's bounding box, so streamed sections bend with the SAME mapping.
    corridor_lateral: f64,
    /// The clear path in **unbent corridor** space. `path` is the public (bent, in
    /// radial mode) copy sent to clients; `corridor_path` is what section generation
    /// rejects obstacles/terraces against, so streaming stays in the corridor frame.
    corridor_path: Vec<Position>,
    /// The avatar's collision radius against obstacles.
    player_radius: f64,
    touch_radius: f64,
    interaction_radius: f64,
    sim_dt: f64,
    // World-gen tunables (snapshot from balance) needed for streaming.
    seed_base: u64,
    /// Tutorial run (the account's first dive): classic distance-ordered biomes +
    /// the centred area-0 onboarding. Otherwise biomes are drawn per-section
    /// (roadmap WG-2/WG-3) and area 0 is a normal procedural section.
    tutorial: bool,
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
    pub fn generate(balance: &Balance, seed: u64, tutorial: bool) -> Self {
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
            radial_half: if wg.radial_arc_degrees > 0.0 {
                wg.radial_arc_degrees.to_radians() * 0.5
            } else {
                0.0
            },
            corridor_lateral: wg.lateral_half_extent,
            corridor_path: vec![Position::new(0.0, 0.0)],
            player_radius: wg.player_radius,
            touch_radius: balance.world.touch_radius_tiles,
            interaction_radius: balance.world.interaction_radius_tiles,
            sim_dt: 1.0 / balance.world.overworld_sim_hz as f64,
            seed_base: seed,
            tutorial,
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
        // Snapshot the unbent corridor path BEFORE the bend — outward streaming
        // regenerates in this corridor frame, then bends each new section's tail.
        arena.corridor_path = arena.path.clone();
        // WG-4: bend the whole (flat) corridor into a radial arc around the hub, so
        // the world fans out in every direction but the western city sliver.
        arena.radialize(wg.radial_arc_degrees);
        arena
    }

    /// WG-4: bend the generated corridor into a radial arc around the Center Hub.
    /// A point's corridor `x` becomes its **radius** (so distance — and therefore
    /// difficulty — is unchanged), and its lateral `y` becomes an **angle** across
    /// the arc. The eastward tube spirals outward into a ~350° fan, leaving the
    /// western sliver for Last City. Purely a placement remap of already-generated
    /// content, so biomes/dungeons/gatekeepers/loot/the-clear-path all come along;
    /// the world is flat (terraces are off), so it renders on the client's base
    /// ground plane with no per-section relief mesh. Bounds widen to a square box
    /// that contains the fan; the western return-to-city border is unchanged.
    fn radialize(&mut self, arc_degrees: f64) {
        if arc_degrees <= 0.0 {
            return; // corridor mode — no bend.
        }
        let half = arc_degrees.to_radians() * 0.5;
        // Bend against the corridor half-extent (self.lateral still equals it here,
        // but corridor_lateral is what streaming reuses after lateral widens).
        let lat = self.corridor_lateral.max(1.0);
        let tf = |p: Position| -> Position {
            let r = p.x.max(0.0);
            let theta = (p.y / lat).clamp(-1.0, 1.0) * half;
            Position::new(r * theta.cos(), r * theta.sin())
        };
        for m in &mut self.monsters {
            m.position = tf(m.position);
            m.home = tf(m.home);
            // Corridor x-bounds no longer map to world x; let creatures roam near home.
            m.area_min_x = f64::NEG_INFINITY;
            m.area_max_x = f64::INFINITY;
        }
        for r in &mut self.resources {
            r.position = tf(r.position);
        }
        for o in &mut self.obstacles {
            o.position = tf(o.position);
        }
        for c in &mut self.chests {
            c.position = tf(c.position);
        }
        for p in &mut self.path {
            *p = tf(*p);
        }
        self.portal = tf(self.portal);
        // The non-linear bend distorts the carefully-carved clear-path tube, so an
        // obstacle can end up on it. Re-clear the tube (in the bent coords) so a
        // feasible route outward is preserved by construction, as in the corridor.
        let clear_r = self.path_clear_radius;
        let path = self.path.clone();
        self.obstacles
            .retain(|o| dist_to_path(&o.position, &path) > clear_r + o.radius);
        // Straight-wall biome seams don't survive the bend — drop them.
        self.seams.clear();
        // A square box that contains the whole fan (radius up to the frontier).
        let rmax = self.cursor + 4.0;
        self.x_min = -rmax;
        self.x_max = rmax;
        self.lateral = rmax;
    }

    /// Generate one more section if the frontier is within `stream_lookahead` of
    /// `player_x`. Sections beyond the initial chain are endless and reproducible
    /// (each from `section_seed(seed, n)`). Returns the indices of any sections
    /// newly created this call (so the caller can stream their terrain to clients).
    pub fn ensure_frontier(&mut self, balance: &Balance, reach: f64) -> Vec<usize> {
        let lookahead = balance.worldgen.stream_lookahead;
        // Cap growth per call so a teleport can't explode work in one tick.
        let mut budget = 4;
        // WG-4 radial world: stream new content **rings** outward. The frontier lives
        // in corridor space (`cursor` = the ring's radius, since `radialize` maps
        // corridor x → radius), and `reach` is the player's RADIUS (`hypot(pos−hub)`).
        // Each new section is generated in the pristine corridor frame (so obstacle/
        // terrace rejection stays correct against the unbent path and corridor extent),
        // then its freshly-added tail is bent into the arc and appended — the same
        // remap the initial disk got, applied incrementally. Difficulty rides
        // `distance` as always, so the world is endless AND monotonically harder outward.
        if self.radial_half > 0.0 {
            let mut created = Vec::new();
            while self.cursor < reach + lookahead && budget > 0 {
                let i = self.areas.len();
                created.push(self.stream_radial_section(balance, i));
                budget -= 1;
            }
            return created;
        }
        let mut created = Vec::new();
        while self.cursor < reach + lookahead && budget > 0 {
            let i = self.areas.len();
            self.push_section(balance, i);
            self.x_max = self.cursor + self.world_margin;
            created.push(i);
            budget -= 1;
        }
        created
    }

    /// Generate section `i` in the corridor frame, then bend its new content into the
    /// radial arc and append it — the streaming counterpart to the one-shot bend in
    /// [`Arena::radialize`]. Returns `i`. Only called in radial mode.
    fn stream_radial_section(&mut self, balance: &Balance, i: usize) -> usize {
        // Enter the pristine corridor frame: `push_section` reads `self.lateral` (the
        // placement extent) and `self.path` (the rejection polyline), both of which
        // `radialize` repurposed for the bent world — so swap the corridor values in
        // for the duration of the call, then swap the bent world back.
        let saved_lateral = self.lateral;
        let saved_path = std::mem::replace(&mut self.path, std::mem::take(&mut self.corridor_path));
        self.lateral = self.corridor_lateral;
        // Snapshot the tails so we can bend exactly what this section appends.
        let (m0, r0, o0, c0, s0) = (
            self.monsters.len(),
            self.resources.len(),
            self.obstacles.len(),
            self.chests.len(),
            self.seams.len(),
        );
        let p0 = self.path.len();

        self.push_section(balance, i); // corridor-space append; advances `cursor`.

        // Leave the corridor frame: the (now-extended) corridor path goes back to
        // `corridor_path`; restore the bent public `path` + the fan's bounds `lateral`.
        self.corridor_path = std::mem::replace(&mut self.path, saved_path);
        self.lateral = saved_lateral;

        // Bend this section's freshly-added tail into the arc (same map as radialize).
        let half = self.radial_half;
        let lat = self.corridor_lateral.max(1.0);
        let tf = |p: Position| -> Position {
            let r = p.x.max(0.0);
            let theta = (p.y / lat).clamp(-1.0, 1.0) * half;
            Position::new(r * theta.cos(), r * theta.sin())
        };
        for m in &mut self.monsters[m0..] {
            m.position = tf(m.position);
            m.home = tf(m.home);
            m.area_min_x = f64::NEG_INFINITY; // roam near home, not a corridor x-band.
            m.area_max_x = f64::INFINITY;
        }
        for r in &mut self.resources[r0..] {
            r.position = tf(r.position);
        }
        for o in &mut self.obstacles[o0..] {
            o.position = tf(o.position);
        }
        for c in &mut self.chests[c0..] {
            c.position = tf(c.position);
        }
        // Append this section's new corridor waypoint(s) to the bent public path.
        for k in p0..self.corridor_path.len() {
            self.path.push(tf(self.corridor_path[k]));
        }
        // Straight-wall biome seams don't survive the bend — drop the ones just added.
        self.seams.truncate(s0);
        // The bend distorts the clear-path tube AND appending this section's waypoint
        // adds a new path segment near the previous frontier — either can pull an
        // already-placed obstacle into the tube. Re-clear ALL obstacles against the
        // full bent path, exactly as the one-shot `radialize` does, so a feasible route
        // outward stays guaranteed by construction across the whole streamed world.
        let clear_r = self.path_clear_radius;
        let path = self.path.clone();
        self.obstacles
            .retain(|o| dist_to_path(&o.position, &path) > clear_r + o.radius);
        // Grow the fan's bounding box to contain the new outer ring.
        let rmax = self.cursor + 4.0;
        self.x_min = -rmax;
        self.x_max = rmax;
        self.lateral = rmax;
        i
    }

    /// Build section `i` from its OWN seed (`section_seed`) and append it to the
    /// flat entity vectors + the path. Self-contained per section: no shared RNG
    /// state threads between sections, which is exactly what makes streaming and
    /// reproducibility work (docs/proposals/verticality.md per-section seeds).
    fn push_section(&mut self, balance: &Balance, i: usize) {
        let wg = &balance.worldgen;
        let mut rng = Rng(section_seed(self.seed_base, i));
        let start_x = self.cursor;
        // Theme rides the run (WG-2/WG-3) but difficulty rides `distance` as always.
        let prev_biome = self.areas.last().map(|a| a.biome);
        let biome = section_biome(
            self.seed_base,
            i,
            start_x.floor() as i64,
            prev_biome,
            self.tutorial,
        );
        let kinds = creatures_for_biome(biome);

        // Area 0 of the TUTORIAL run is a small, deterministic onboarding section
        // near the Center Hub: exactly one canonical creature on the centre line and
        // a portal a short walk past it. Predictable onboarding (a straight east walk
        // always meets one fightable target, then a portal) — and the e2e/conformance
        // tests depend on this determinism. On non-tutorial runs area 0 is a normal
        // procedural section (random biome, scattered creatures, terraces).
        if i == 0 && self.tutorial {
            let pos = Position::new(wg.first_monster_x, 0.0);
            let idx = self.monsters.len();
            let mseed = rng.next_u64();
            self.monsters
                .push(MonsterSpawn::build(balance, format!("mob-{idx}"), kinds[0], pos, mseed));
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
                elevation: 0,
            });
            self.areas.push(Area {
                index: i,
                biome,
                start_x,
                end_x,
                portal: Position::new(portal_x, 0.0),
                // The tutorial section is entirely flat (level 0).
                terrain: Terrain::empty(start_x, end_x, -self.lateral, self.terrain_cell),
                dungeon: false,
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

        // WG-1: every Nth procedural section is a DUNGEON — rooms divided by walls
        // with a door on the clear path (connectivity guaranteed like a biome seam),
        // packed denser with creatures and ending in a guaranteed loot chest. Never
        // the tutorial run or the spawn section (i == 0). Dungeons stay flat.
        let is_dungeon =
            !self.tutorial && i > 0 && wg.dungeon_every > 0 && i.is_multiple_of(wg.dungeon_every);

        // Walk the corridor placing creatures at jittered gaps. Creatures scatter
        // across ±y so the map is populated in every direction and you explore to
        // find fights. A dungeon packs them denser (tighter spacing).
        let creature_spacing = if is_dungeon {
            wg.monster_spacing / wg.dungeon_creature_mult.max(1.0)
        } else {
            wg.monster_spacing
        };
        let inner_end = end_x - wg.portal_setback - 1.0;
        let mut x = start_x + 2.0;
        // FS-4: a fraction of creatures roll ELITE (champions). A SEPARATE rng stream
        // so the main placement draws stay byte-identical (determinism tests hold).
        // Never in the spawn section (i == 0), which stays gentle onboarding.
        let enc = &balance.encounters;
        let mut erng = Rng(section_seed(self.seed_base, i) ^ 0xE117_E117_E117_E117);
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
            if i > 0 && !self.tutorial && erng.unit() < enc.elite_chance {
                self.monsters[idx].promote(
                    enc.elite_hp_mult,
                    enc.elite_atk_mult,
                    enc.elite_xp_mult,
                    "elite",
                );
                self.monsters[idx].apply_affix(erng.next_u64());
            }

            let gap = creature_spacing * (1.0 + wg.monster_spacing_jitter * rng.signed());
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

        // One treasure chest per section. With `chest_terrace_chance` it sits ON TOP
        // of a raised terrace at that terrace's elevation — the payoff for climbing a
        // detour (open_chest gates on matching elevation, so you must be up there).
        // Otherwise it's rejection-sampled onto the ground off the clear path (a small
        // detour off the main line — old-school "explore for treasure").
        let mut chest_placed = false;
        if rng.unit() < wg.chest_terrace_chance {
            let raised: Vec<(f64, f64, u8)> = (0..terrain.cols)
                .flat_map(|gx| (0..terrain.rows).map(move |gy| (gx, gy)))
                .filter_map(|(gx, gy)| {
                    let lvl = terrain.level[gx * terrain.rows + gy];
                    (lvl > 0).then(|| {
                        let c = terrain.cell_center(gx, gy);
                        (c.x, c.y, lvl)
                    })
                })
                .collect();
            if !raised.is_empty() {
                let (tx, ty, lvl) = raised[rng.below(raised.len())];
                self.chests.push(Chest {
                    entity_id: format!("chest-{}", self.chests.len()),
                    position: Position::new(tx, ty),
                    tier: Scaling::new(balance).tier(tx.floor() as i64) as i32,
                    opened: false,
                    elevation: lvl,
                });
                chest_placed = true;
            }
        }
        if !chest_placed {
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
                        elevation: 0,
                    });
                    break;
                }
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
            // FS-4: a GATEKEEPER boss stands in the door — a big, unavoidable fight
            // guarding the pass into the next region, with a fat guaranteed reward.
            // Not on the tutorial run (a new player's first dive stays gentle).
            if self.tutorial {
                continue;
            }
            let gk_pos = Position::new(bd, path_y_at(&self.path, bd));
            let gidx = self.monsters.len();
            let gseed = section_seed(self.seed_base, i) ^ (0x6A7E_0000_0000_0000 | bd as u64);
            self.monsters
                .push(MonsterSpawn::build(balance, format!("mob-{gidx}"), kinds[0], gk_pos, gseed));
            self.monsters[gidx].area_min_x = start_x;
            self.monsters[gidx].area_max_x = end_x;
            self.monsters[gidx].promote(
                enc.gatekeeper_hp_mult,
                enc.gatekeeper_atk_mult,
                enc.gatekeeper_xp_mult,
                "gatekeeper",
            );
            self.monsters[gidx].apply_affix(gseed ^ 0xAFF1);
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
        // A dungeon lays out rooms-and-corridors instead of the scattered maze fill.
        if maze_mult > 0.0 && !is_dungeon {
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

        // WG-1 dungeon layout: `dungeon_rooms − 1` divider walls span the corridor,
        // each leaving a single door gap centred on the clear path — so the section
        // reads as a chain of rooms, and connectivity is guaranteed by construction
        // (every door sits on the already-carved, obstacle-free clear path). The
        // final room holds a guaranteed loot chest. Walls skip terraced cells and
        // never bury a creature/resource. Rendered by the normal obstacle path.
        if is_dungeon {
            let r = wg.dungeon_wall_radius.max(0.4);
            let rooms = wg.dungeon_rooms.max(2);
            for w in 1..rooms {
                let wall_x = start_x + length * (w as f64) / (rooms as f64);
                let door_y = path_y_at(&self.path, wall_x);
                let mut y = -self.lateral + 1.0;
                while y <= self.lateral - 1.0 {
                    let pos = Position::new(wall_x, y);
                    let in_door = (y - door_y).abs() < wg.dungeon_door_half;
                    let occupied = self.monsters.iter().any(|m| m.position.distance_to(&pos) < r + 1.0)
                        || self.resources.iter().any(|rn| rn.position.distance_to(&pos) < r + 1.0)
                        || self.chests.iter().any(|c| c.position.distance_to(&pos) < r + 1.0);
                    if !in_door && terrain.level_at(&pos) == 0 && !occupied {
                        self.obstacles.push(Obstacle {
                            entity_id: format!("obs-{}", self.obstacles.len()),
                            kind: okinds[0].to_string(),
                            position: pos,
                            radius: r,
                        });
                    }
                    y += r * 1.8;
                }
            }
            // Guaranteed loot chest in the final room, just inside the exit.
            let chest_x = end_x - wg.portal_setback - 2.0;
            let cy = path_y_at(&self.path, chest_x) + 2.0;
            let cpos = Position::new(chest_x, cy);
            let elevation = terrain.level_at(&cpos);
            self.chests.push(Chest {
                entity_id: format!("chest-{}", self.chests.len()),
                position: cpos,
                tier: Scaling::new(balance).tier(chest_x.floor() as i64) as i32,
                opened: false,
                elevation,
            });
        }

        self.areas.push(Area {
            index: i,
            biome,
            start_x,
            end_x,
            portal,
            terrain,
            dungeon: is_dungeon,
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
        self.step_creatures_with_aggro(dt, &HashMap::new());
    }

    /// Like [`Arena::step_creatures`], but scales each player's effective aggro
    /// radius by `aggro_mult[player_id]` (default 1.0) — the Iron Hull "Bulwark"
    /// perk shrinks how close a creature will chase/skirmish-pull that party.
    /// Deterministic given the per-creature seed and the multiplier map.
    pub fn step_creatures_with_aggro(&mut self, dt: f64, aggro_mult: &HashMap<Id, f64>) {
        // Snapshot active-avatar positions + their aggro multiplier (immutable
        // borrow) before moving creatures.
        let players: Vec<(Position, f64)> = self
            .avatars
            .iter()
            .filter(|a| a.state == "active")
            .map(|a| {
                (
                    a.position,
                    aggro_mult.get(&a.player_id).copied().unwrap_or(1.0),
                )
            })
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
                // Nearest active player within aggro range — each player's range is
                // scaled by their Bulwark multiplier (Iron Hull parties are chased
                // from closer).
                let player_target = players
                    .iter()
                    .filter(|(p, mult)| m.position.distance_to(p) <= aggro_range * mult)
                    .map(|(p, _)| *p)
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
        let (ppos, pelev) = {
            let a = self.avatar(player_id)?;
            (a.position, a.elevation)
        };
        let radius = self.interaction_radius;
        let chest = self
            .chests
            .iter_mut()
            .find(|c| c.entity_id == entity_id && !c.opened)?;
        // Must share the chest's elevation (a terrace-top chest needs you up there),
        // and be within reach — mirrors `harvest`.
        if chest.elevation != pelev || ppos.distance_to(&chest.position) > radius {
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
    /// Look up a monster by its stable `entity_id`. Battles reference their
    /// creatures by id (not vec index) so [`Self::prune_defeated`] can compact the
    /// list without corrupting in-flight battles.
    pub fn monster_by_id(&self, entity_id: &str) -> Option<&MonsterSpawn> {
        self.monsters.iter().find(|m| m.entity_id == entity_id)
    }

    pub fn monster_by_id_mut(&mut self, entity_id: &str) -> Option<&mut MonsterSpawn> {
        self.monsters.iter_mut().find(|m| m.entity_id == entity_id)
    }

    /// Drop slain creatures from the world so `monsters` doesn't grow without bound
    /// over a long dive (every kill used to leave a corpse in the vec forever, and
    /// `step_creatures`/snapshot iterate the whole list each tick). A creature still
    /// locked in a fight (`in_battle`) is kept even if flagged defeated — its battle
    /// slot still refers to it by id. Safe because ids, not indices, are the durable
    /// reference; call it only outside battle-assembly (e.g. end of the game tick).
    pub fn prune_defeated(&mut self) {
        self.monsters.retain(|m| !m.defeated || m.in_battle);
    }

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

    /// The default balance now generates the WG-4 **radial** world (flat, no
    /// terraces/seams/streaming). Tests that specifically exercise those corridor
    /// features build this corridor-mode balance instead (radial bend off).
    fn corridor_balance() -> Balance {
        let mut b = Balance::load_default().unwrap();
        b.worldgen.radial_arc_degrees = 0.0;
        b.worldgen.terraces_per_area = 3.0;
        b.worldgen.max_level = 2;
        b
    }

    #[test]
    fn loot_is_deterministic_and_scales_with_depth() {
        let b = Balance::load_default().unwrap();
        // Same seed ⇒ identical loot (pure function).
        assert_eq!(
            roll_creature_loot(&b, 50, 1, 1.0, 12345),
            roll_creature_loot(&b, 50, 1, 1.0, 12345)
        );
        // Forest keeps the crafting/conformance material id.
        assert_eq!(roll_creature_loot(&b, 10, 1, 1.0, 1).material, "forest_bloom_petal");
        // Deeper fights pay more chits on average (sample a few seeds).
        let shallow: i64 = (0..16).map(|s| roll_creature_loot(&b, 40, 1, 1.0, s).chits).sum();
        let deep: i64 = (0..16).map(|s| roll_creature_loot(&b, 800, 1, 1.0, s).chits).sum();
        assert!(deep > shallow, "deeper creatures should drop more chits");
    }

    #[test]
    fn red_gear_never_drops_below_the_red_chest_floor() {
        let b = Balance::load_default().unwrap();
        let floor = b.world_scaling.red_chest_floor_distance;
        // Below the floor: no gear across many seeds.
        for s in 0..200 {
            assert!(roll_creature_loot(&b, floor - 1, 1, 1.0, s).gear.is_none());
        }
        // At/after the floor: gear does appear for some seeds, at the right tier.
        let mut saw_gear = false;
        for s in 0..200 {
            if let Some(g) = roll_creature_loot(&b, floor, 1, 1.0, s).gear {
                saw_gear = true;
                assert_eq!(g.tier, Scaling::new(&b).tier(floor) as i32);
                assert!(g.max_durability > 0);
                // Exactly one stat is rolled, matching the drop's own slot.
                let stat = match g.slot.as_str() {
                    "weapon" => g.atk_bonus,
                    "armor" => g.def_bonus,
                    "accessory" => g.spd_bonus,
                    other => panic!("unexpected gear slot {other}"),
                };
                assert!(stat >= 1, "the {} drop should roll a nonzero stat", g.slot);
                let others = g.atk_bonus + g.def_bonus + g.spd_bonus - stat;
                assert_eq!(others, 0, "only the {} slot's stat should be nonzero", g.slot);
            }
        }
        assert!(saw_gear, "red gear should drop at/after the floor for some seeds");
    }

    #[test]
    fn generates_chests_and_biome_seams() {
        let b = corridor_balance();
        let arena = Arena::generate(&b, 7, true);
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
        let b = corridor_balance();
        let mut arena = Arena::generate(&b, 7, true);
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
        let mut arena = Arena::generate(&b, 5, true);
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
    fn bulwark_multiplier_shrinks_a_creatures_effective_aggro() {
        // Same seed + same player position: a full-aggro party is chased; a Bulwark
        // party (low multiplier) that falls outside the scaled range is not.
        let b = Balance::load_default().unwrap();
        let build = || {
            let mut arena = Arena::generate(&b, 5, true);
            arena.monsters[0].aggression = "aggressive".to_string();
            let m = arena.monsters[0].position;
            arena.add_avatar("p".into(), 6.0);
            // Inside the base aggro radius (11) but outside 0.5× of it.
            arena.avatar_mut("p").unwrap().position = Position::new(m.x + 8.0, m.y);
            arena
        };
        let mut normal = build();
        let mut bulwark = build();
        let start = normal.monsters[0].position.x;
        let mut mult = HashMap::new();
        mult.insert("p".to_string(), 0.5);
        for _ in 0..10 {
            normal.step_creatures(0.1);
            bulwark.step_creatures_with_aggro(0.1, &mult);
        }
        assert!(
            normal.monsters[0].position.x - start > 2.0,
            "full-aggro creature chases the player"
        );
        assert!(
            bulwark.monsters[0].position.x - start < 2.0,
            "Bulwark shrinks the aggro radius so the creature doesn't chase"
        );
    }

    #[test]
    fn passive_creature_leashes_near_home() {
        let b = Balance::load_default().unwrap();
        let mut arena = Arena::generate(&b, 5, true);
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
        let mut arena = Arena::generate(&b, 5, true);
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
        let mut arena = Arena::generate(&b, 5, true);
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
        let mut arena = Arena::generate(&b, 5, true);
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
        let mut arena = Arena::generate(&b, 5, true);
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
        let a = Arena::generate(&b, 12345, true);
        let c = Arena::generate(&b, 12345, true);
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
        let d = Arena::generate(&b, 999, true);
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
        let a = Arena::generate(&b, 77, true);
        let c = Arena::generate(&b, 77, true);
        for (x, y) in a.areas.iter().zip(c.areas.iter()) {
            assert_eq!(x.terrain.level, y.terrain.level);
            assert_eq!(x.terrain.connectors.len(), y.terrain.connectors.len());
        }
    }

    #[test]
    fn areas_trend_larger_and_carry_creatures() {
        let b = Balance::load_default().unwrap();
        let arena = Arena::generate(&b, 7, true);
        assert_eq!(arena.areas.len(), b.worldgen.area_count);
        assert!(!arena.monsters.is_empty());
        // Every area has a portal past its creatures and at least one creature.
        for area in &arena.areas {
            assert!(area.portal.x <= area.end_x);
            assert!(
                arena
                    .monsters
                    .iter()
                    .any(|m| m.position.x >= area.start_x && m.position.x < area.end_x),
                "area {} has no creature",
                area.index
            );
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
        let arena = Arena::generate(&b, 7, true);
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
        let arena = Arena::generate(&b, 7, true);
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
        let mut arena = Arena::generate(&b, 7, true);
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
        let b = corridor_balance();
        let arena = Arena::generate(&b, 7, true);
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
        let b = corridor_balance();
        let arena = Arena::generate(&b, 7, true);
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
            let arena = Arena::generate(&b, seed, true);
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
        let mut arena = Arena::generate(&b, 42, true);
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
        let b = corridor_balance();
        let seed = [42u64, 1, 7, 999, 123456, 2, 3, 5, 11]
            .into_iter()
            .find(|&s| {
                Arena::generate(&b, s, true).areas.iter().any(|a| {
                    a.terrain.connectors.iter().any(|c| c.entity_id.starts_with("pramp-"))
                })
            })
            .expect("some seed produces a climbing clear path");
        let mut arena = Arena::generate(&b, seed, true);
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
    fn a_terrace_chest_only_opens_from_its_elevation() {
        // Treasure atop a climb: a chest sitting on a terrace can't be opened from the
        // ground below it — you must be up on the terrace (matching elevation).
        let b = corridor_balance();
        let seed = (1u64..300)
            .find(|&s| Arena::generate(&b, s, true).chests.iter().any(|c| c.elevation > 0))
            .expect("some seed puts a chest on a terrace");
        let mut arena = Arena::generate(&b, seed, true);
        let chest = arena.chests.iter().find(|c| c.elevation > 0).unwrap().clone();
        arena.add_avatar("p".into(), 8.0);
        // Standing at the chest's (x,y) but on the GROUND: blocked.
        {
            let a = arena.avatar_mut("p").unwrap();
            a.position = chest.position;
            a.elevation = 0;
        }
        assert!(
            arena.open_chest("p", &chest.entity_id).is_none(),
            "seed {seed}: a ground-level player can't open a terrace-top chest"
        );
        // Up on the terrace (matching elevation): it opens.
        arena.avatar_mut("p").unwrap().elevation = chest.elevation;
        assert!(
            arena.open_chest("p", &chest.entity_id).is_some(),
            "seed {seed}: at the chest's elevation it opens"
        );
    }

    #[test]
    fn a_cliff_blocks_but_a_connector_lets_you_climb() {
        // Find a raised terrace, prove you can't walk onto it across the cliff, then
        // prove stepping onto its connector carries you up.
        let b = corridor_balance();
        let mut arena = Arena::generate(&b, 7, true);
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
        let b = corridor_balance();
        let mut a = Arena::generate(&b, 55, true);
        let chain = a.areas.len();
        // Walking the frontier east streams in fresh sections beyond the chain.
        let created = a.ensure_frontier(&b, a.areas.last().unwrap().end_x + 100.0);
        assert!(!created.is_empty(), "frontier advance streams new sections");
        assert!(a.areas.len() > chain, "world grew past the initial chain");
        // The deep portal does NOT move when streaming past it.
        assert_eq!(a.portal, a.areas[chain - 1].portal);
        // Reproducible: a second arena streamed the same way matches section-for-section.
        let mut c = Arena::generate(&b, 55, true);
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
        let mut arena = Arena::generate(&b, 7, true);
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
        let mut arena = Arena::generate(&b, 42, true);
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

    #[test]
    fn prune_defeated_reclaims_corpses_but_keeps_in_battle_and_ids_resolve() {
        let b = Balance::load_default().unwrap();
        let mut arena = Arena::generate(&b, 11, true);
        let before = arena.monsters.len();
        assert!(before >= 3, "need a few monsters for the test");

        // A stable id resolves to the right monster regardless of vec position.
        let victim = arena.monsters[0].entity_id.clone();
        let survivor = arena.monsters[1].entity_id.clone();
        let fighting = arena.monsters[2].entity_id.clone();
        assert_eq!(arena.monster_by_id(&victim).unwrap().entity_id, victim);

        // One slain, one slain-but-still-locked-in-a-fight, one untouched.
        arena.monster_by_id_mut(&victim).unwrap().defeated = true;
        {
            let f = arena.monster_by_id_mut(&fighting).unwrap();
            f.defeated = true;
            f.in_battle = true; // its battle slot still refers to it by id
        }

        arena.prune_defeated();

        assert_eq!(arena.monsters.len(), before - 1, "only the free corpse is dropped");
        assert!(arena.monster_by_id(&victim).is_none(), "slain free creature reclaimed");
        assert!(arena.monster_by_id(&survivor).is_some(), "living creature kept");
        assert!(
            arena.monster_by_id(&fighting).is_some(),
            "creature still in a battle is kept even if flagged defeated",
        );
    }

    // ---- WG-2 / WG-3: seeded biome randomization + tutorial carve-out ----

    #[test]
    fn tutorial_run_always_starts_in_forest() {
        // The account's first dive is the hand-tuned onboarding, whatever the seed.
        let b = Balance::load_default().unwrap();
        for seed in [1u64, 42, 9999, 123_456] {
            assert_eq!(Arena::generate(&b, seed, true).areas[0].biome, "forest");
        }
    }

    #[test]
    fn non_tutorial_start_biome_varies_and_is_not_pinned_to_forest() {
        // WG-2: later runs start in a random biome, not always Forest.
        let b = Balance::load_default().unwrap();
        let starts: std::collections::HashSet<&str> = (0u64..40)
            .map(|s| Arena::generate(&b, s, false).areas[0].biome)
            .collect();
        assert!(starts.len() > 1, "start biome should vary across runs: {starts:?}");
        assert!(starts.iter().any(|&x| x != "forest"), "some runs start off-Forest");
    }

    #[test]
    fn biome_order_is_deterministic_per_seed_and_varies_across_seeds() {
        // WG-3: reproducible per seed (determinism is load-bearing), different per run.
        let b = Balance::load_default().unwrap();
        let order = |seed: u64| -> Vec<&'static str> {
            let mut a = Arena::generate(&b, seed, false);
            a.ensure_frontier(&b, 500.0);
            a.areas.iter().map(|x| x.biome).collect()
        };
        assert_eq!(order(77), order(77), "same seed reproduces the same biome order");
        assert_ne!(order(1), order(2), "different seeds vary the biome order");
    }

    #[test]
    fn no_two_adjacent_sections_share_a_biome() {
        // The no-adjacent-repeat rule: you never walk from one theme into the same one.
        let b = Balance::load_default().unwrap();
        let mut a = Arena::generate(&b, 31_337, false);
        a.ensure_frontier(&b, 800.0);
        assert!(a.areas.len() >= 3, "need a few sections to check adjacency");
        for w in a.areas.windows(2) {
            assert_ne!(w[0].biome, w[1].biome, "adjacent sections must differ in biome");
        }
    }

    // ---- WG-1: dungeons (BSP-ish rooms via divider walls + guaranteed loot) ----

    #[test]
    fn dungeons_appear_with_walls_and_a_guaranteed_loot_chest() {
        let b = Balance::load_default().unwrap();
        // dungeon_every=4, area_count=8 → section 4 is a dungeon in the initial chain.
        let arena = Arena::generate(&b, 7, false);
        let dungeon = arena
            .areas
            .iter()
            .find(|a| a.dungeon)
            .expect("a dungeon section exists in the chain");
        let (s, e) = (dungeon.start_x, dungeon.end_x);
        let walls = arena.obstacles.iter().filter(|o| o.position.x >= s && o.position.x <= e).count();
        assert!(walls > 0, "dungeon carries divider-wall obstacles");
        assert!(
            arena.chests.iter().any(|c| c.position.x >= s && c.position.x <= e),
            "dungeon has a guaranteed loot chest",
        );
    }

    #[test]
    fn tutorial_and_spawn_are_never_dungeons() {
        let b = Balance::load_default().unwrap();
        // The whole tutorial run is dungeon-free (gentle onboarding).
        assert!(Arena::generate(&b, 3, true).areas.iter().all(|a| !a.dungeon));
        // Non-tutorial: the spawn section (index 0) is never a dungeon.
        assert!(!Arena::generate(&b, 3, false).areas[0].dungeon);
    }

    #[test]
    fn the_clear_path_reaches_the_portal_through_dungeons() {
        // Feasibility survives the divider walls: a walker following the waypoints
        // still reaches the deep portal (every door sits on the clear path).
        let b = Balance::load_default().unwrap();
        let mut arena = Arena::generate(&b, 9, false);
        assert!(arena.areas.iter().any(|a| a.dungeon), "chain contains a dungeon");
        let waypoints = arena.path.clone();
        arena.add_avatar("p".into(), 2.0);
        let mut wp = 1usize;
        let mut reached = false;
        for _ in 0..100_000 {
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
        assert!(reached, "the path stays feasible through the dungeon doors");
    }

    #[test]
    fn dungeon_layout_is_deterministic() {
        let b = Balance::load_default().unwrap();
        let sig = |seed: u64| -> (Vec<bool>, usize, usize) {
            let a = Arena::generate(&b, seed, false);
            (a.areas.iter().map(|x| x.dungeon).collect(), a.obstacles.len(), a.chests.len())
        };
        assert_eq!(sig(55), sig(55), "same seed reproduces the same dungeons + walls");
    }

    // ---- FS-4: Elite champions + Gatekeeper bosses ----

    #[test]
    fn promote_scales_stats_and_tags_the_encounter_class() {
        let b = Balance::load_default().unwrap();
        let base = MonsterSpawn::build(&b, "m".into(), "forest_bloom_stalker", Position::new(50.0, 0.0), 1);
        let mut elite = base.clone();
        elite.promote(2.0, 1.5, 3.0, "elite");
        assert_eq!(elite.encounter_class, "elite");
        assert_eq!(elite.max_hp, base.max_hp * 2);
        assert_eq!(elite.hp, elite.max_hp, "promoted spawn is at full HP");
        assert!(elite.atk > base.atk && elite.xp_reward > base.xp_reward);
    }

    #[test]
    fn gatekeepers_guard_biome_borders_and_are_a_wall_of_hp() {
        let b = Balance::load_default().unwrap();
        let mut a = Arena::generate(&b, 7, false);
        a.ensure_frontier(&b, 400.0); // cross the 100 + 300 borders
        let gks: Vec<_> = a.monsters.iter().filter(|m| m.encounter_class == "gatekeeper").cloned().collect();
        assert!(!gks.is_empty(), "a gatekeeper guards each crossed border");
        for gk in &gks {
            // Compare to a standard creature of the same kind at the same spot.
            let std_hp = MonsterSpawn::build(&b, "s".into(), &gk.monster_kind, gk.position, 1).max_hp;
            assert!(gk.max_hp > std_hp * 3, "gatekeeper is a real fight: {} vs {}", gk.max_hp, std_hp);
        }
    }

    #[test]
    fn elites_appear_among_mostly_standard_creatures() {
        let b = Balance::load_default().unwrap();
        let mut a = Arena::generate(&b, 3, false);
        a.ensure_frontier(&b, 500.0);
        let elites = a.monsters.iter().filter(|m| m.encounter_class == "elite").count();
        let standard = a.monsters.iter().filter(|m| m.encounter_class == "standard").count();
        assert!(elites > 0, "some creatures are elite champions");
        assert!(standard > elites, "but most creatures are still standard");
    }

    #[test]
    fn the_tutorial_run_has_no_elites_or_gatekeepers() {
        let b = Balance::load_default().unwrap();
        let mut a = Arena::generate(&b, 9, true);
        a.ensure_frontier(&b, 400.0);
        assert!(a.monsters.iter().all(|m| m.encounter_class == "standard"),
            "a new player's first dive stays gentle");
    }

    #[test]
    fn champions_roll_a_known_affix_and_standards_have_none() {
        let b = Balance::load_default().unwrap();
        let mut a = Arena::generate(&b, 3, false);
        a.ensure_frontier(&b, 500.0);
        let known = ["Swift", "Brutal", "Armored", "Giant", "Vicious"];
        let mut champions = 0;
        for m in &a.monsters {
            if m.encounter_class == "standard" {
                assert!(m.affix.is_empty(), "standard creatures carry no affix");
            } else {
                assert!(known.contains(&m.affix.as_str()), "champion affix is known: {:?}", m.affix);
                champions += 1;
            }
        }
        assert!(champions > 0, "some champions exist to carry affixes");
    }

    #[test]
    fn a_reward_spike_fattens_the_loot() {
        // Same seed: a gatekeeper's loot_mult yields far more chits + a surer gear drop.
        let b = Balance::load_default().unwrap();
        let d = 600; // past the red-chest floor
        let standard: i64 = (0..24).map(|s| roll_creature_loot(&b, d, 1, 1.0, s).chits).sum();
        let boss: i64 = (0..24)
            .map(|s| roll_creature_loot(&b, d, 1, b.encounters.gatekeeper_loot_mult, s).chits)
            .sum();
        assert!(boss > standard * 4, "a gatekeeper pays out far more: {boss} vs {standard}");
        let boss_gear = (0..24)
            .filter(|&s| roll_creature_loot(&b, d, 1, b.encounters.gatekeeper_loot_mult, s).gear.is_some())
            .count();
        assert!(boss_gear >= 20, "a gatekeeper almost always drops gear: {boss_gear}/24");
    }

    #[test]
    fn gear_rolls_rarities_and_bosses_favour_the_shiny() {
        let b = Balance::load_default().unwrap();
        let d = 600; // past the red-chest floor
        // Standard drops span multiple rarities; the rarity word rides the name.
        let mut kinds = std::collections::HashSet::new();
        for s in 0..400u64 {
            if let Some(g) = roll_creature_loot(&b, d, 1, 1.0, s).gear {
                kinds.insert(g.rarity.clone());
                if g.rarity != "common" {
                    let cap = format!("{}{}", g.rarity[..1].to_uppercase(), &g.rarity[1..]);
                    assert!(g.name.starts_with(&cap), "rarity rides the name: {} / {}", g.rarity, g.name);
                }
            }
        }
        assert!(kinds.contains("common"), "commons exist: {kinds:?}");
        assert!(kinds.len() >= 2, "multiple rarities appear: {kinds:?}");
        // A gatekeeper's loot spike shifts hard toward non-common gear.
        let (mut drops, mut shiny) = (0, 0);
        for s in 0..200u64 {
            if let Some(g) = roll_creature_loot(&b, d, 1, b.encounters.gatekeeper_loot_mult, s).gear {
                drops += 1;
                if g.rarity != "common" {
                    shiny += 1;
                }
            }
        }
        assert!(shiny * 4 > drops * 3, "bosses mostly drop non-common: {shiny}/{drops}");
    }

    #[test]
    fn each_biome_gains_a_distinct_archetype_creature() {
        let b = Balance::load_default().unwrap();
        let p = Position::new(50.0, 0.0);
        // Every new creature is defined in balance (build panics if a key is missing).
        for k in ["sporeling", "dune_colossus", "ember_wisp", "glacier_maw", "bog_stinger"] {
            let _ = MonsterSpawn::build(&b, "m".into(), k, p, 1);
        }
        // A SWARMER is fast + fragile; a BRUISER is slow + tanky — the rhythm differs.
        let swarmer = MonsterSpawn::build(&b, "s".into(), "sporeling", p, 1);
        let bruiser = MonsterSpawn::build(&b, "br".into(), "dune_colossus", p, 1);
        assert!(swarmer.speed_stat > bruiser.speed_stat, "swarmer acts faster");
        assert!(bruiser.max_hp > swarmer.max_hp * 3, "bruiser is a tank vs the swarmer");
        // Each biome's creature pool grew to 3 (the tutorial creature, index 0, is kept).
        assert_eq!(creatures_for_biome("forest").len(), 3);
        assert_eq!(creatures_for_biome("forest")[0], "forest_bloom_stalker");
    }

    #[test]
    fn wg4_radial_world_fans_content_around_the_hub() {
        // The default balance bends the world into a radial arc: content spreads in
        // every direction around the hub, leaving the western sliver for Last City.
        let b = Balance::load_default().unwrap();
        let arena = Arena::generate(&b, 7, false);
        let angles: Vec<f64> = arena
            .monsters
            .iter()
            .filter(|m| (m.position.x.powi(2) + m.position.y.powi(2)).sqrt() > 5.0)
            .map(|m| m.position.y.atan2(m.position.x).to_degrees())
            .collect();
        assert!(angles.len() >= 5, "enough placed content to judge the spread");
        let max_a = angles.iter().cloned().fold(f64::MIN, f64::max);
        let min_a = angles.iter().cloned().fold(f64::MAX, f64::min);
        assert!(max_a - min_a > 120.0, "content fans across a wide arc: {min_a:.0}..{max_a:.0}");
        // No content in the western sliver (kept for the city + its wall).
        assert!(angles.iter().all(|a| a.abs() < 176.0), "western sliver stays clear");
        // Difficulty is still radial distance — a deep creature is far from the hub.
        let max_r = arena
            .monsters
            .iter()
            .map(|m| (m.position.x.powi(2) + m.position.y.powi(2)).sqrt())
            .fold(0.0_f64, f64::max);
        assert!(max_r > 50.0, "the world extends outward, not just a ring");
    }

    #[test]
    fn wg4_radial_world_streams_endlessly_outward() {
        // The radial world is INFINITE: as a player's radius grows, new content rings
        // stream outward — bent into the arc, harder with distance, route stays feasible.
        let b = Balance::load_default().unwrap();
        let mut arena = Arena::generate(&b, 11, false);
        let r_of = |p: &Position| p.x.hypot(p.y);

        let initial_frontier = arena.cursor; // corridor frontier = outer ring radius
        let initial_sections = arena.areas.len();
        let initial_max_r = arena.monsters.iter().map(|m| r_of(&m.position)).fold(0.0_f64, f64::max);

        // Walk the frontier out to a much larger RADIUS; the world must generate to meet it.
        let target_radius = initial_frontier + 400.0;
        let mut created_total = 0usize;
        for _ in 0..200 {
            let created = arena.ensure_frontier(&b, target_radius);
            created_total += created.len();
            if arena.cursor >= target_radius {
                break;
            }
        }
        assert!(created_total > 0, "streaming created new sections outward");
        assert!(arena.areas.len() > initial_sections, "the section chain grew");
        assert!(
            arena.cursor >= target_radius,
            "frontier ({:.0}) reached the far radius ({:.0})",
            arena.cursor,
            target_radius
        );

        // New creatures live out past the old frontier — the world is genuinely endless,
        // and difficulty (radial distance) keeps climbing.
        let new_max_r = arena.monsters.iter().map(|m| r_of(&m.position)).fold(0.0_f64, f64::max);
        assert!(
            new_max_r > initial_max_r + 200.0,
            "content now reaches much farther out ({initial_max_r:.0} → {new_max_r:.0})"
        );

        // The streamed content is BENT into the arc, not a straight +x corridor tail:
        // some far creature sits well outside the corridor's lateral half-extent in |y|.
        let lat = b.worldgen.lateral_half_extent;
        assert!(
            arena
                .monsters
                .iter()
                .filter(|m| r_of(&m.position) > initial_frontier)
                .any(|m| m.position.y.abs() > lat + 5.0),
            "streamed content fans around the arc (|y| exceeds the corridor width)"
        );

        // A feasible route outward is preserved by construction: no obstacle sits inside
        // the bent clear-path tube (checked across the whole streamed world).
        let clear_r = arena.path_clear_radius;
        for o in &arena.obstacles {
            assert!(
                dist_to_path(&o.position, &arena.path) > clear_r + o.radius - 1e-6,
                "obstacle at ({:.1},{:.1}) blocks the clear path",
                o.position.x,
                o.position.y
            );
        }

        // Determinism: same seed + same reach ⇒ identical streamed world.
        let mut twin = Arena::generate(&b, 11, false);
        for _ in 0..200 {
            twin.ensure_frontier(&b, target_radius);
            if twin.cursor >= target_radius {
                break;
            }
        }
        assert_eq!(twin.monsters.len(), arena.monsters.len(), "streaming is deterministic");
        assert_eq!(twin.areas.len(), arena.areas.len());
    }
}
