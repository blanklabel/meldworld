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

pub mod abilities;
pub mod shift;

use std::collections::HashMap;

use meld_balance::Balance;
use meld_proto::common::Position;
use meld_proto::enums::Insurance;
use meld_proto::affixes as aff;
use meld_proto::uniques as uq;
use meld_proto::equipment as eq;
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

    /// Creature HEALTH: `1 + hp_per_tier * tier(d)` — linear in the same `tier(d)`
    /// that gear power is linear in.
    ///
    /// Every creature stat is scaled against the hero stat that opposes it, and they
    /// do not share a curve because they are not opposed by the same thing:
    ///
    /// - **HP** is opposed by the party's DAMAGE, which is dominated by gear
    ///   (`gear_atk_per_tier` x 7 slots ~= 21 x tier — linear in tier). So HP is linear
    ///   in tier too, and the rounds-per-fight ratio holds at every depth by
    ///   construction rather than by tuning.
    /// - **Attack** is opposed by hero HP and defence, which grow with LEVEL. That is
    ///   [`Self::stat_mult`], still exponential in distance.
    /// - **Armour** is opposed by hero attack, gently — [`Self::def_mult`].
    ///
    /// Before this split, HP rode `stat_mult` while gear rode tier: different shapes,
    /// so no exponent could make them track, and a geared hero one-shot ordinary
    /// creatures at every distance while an ungeared one did fine.
    ///
    /// Linear in `d`, NOT in the integer `tier(d)` — that made it a staircase, and at
    /// `hp_per_tier = 5.4` the step is 6.4x. Measured: a forest stalker at d=99 died in
    /// 2 swings and the same creature at d=100 took 10, so one unit of walking turned an
    /// 8-second fight into a 40-second one. Nothing threatened the party either side of
    /// the line, so the only thing the step changed was how long they held the button
    /// down. The `- 0.5` puts the line through each band's CENTRE, so the depths this
    /// was tuned at (d=150, 250, …) keep the exact multiplier they had and only the
    /// cliff between them goes.
    pub fn hp_mult(&self, d: i64) -> f64 {
        let ws = &self.b.world_scaling;
        let bands = d as f64 / ws.tier_divisor.max(1.0) - 0.5;
        (1.0 + ws.hp_per_tier * bands).max(1.0)
    }

    /// Armour's own curve: `(1 + d/500)^def_mult_exp`, deliberately gentler than
    /// [`Self::stat_mult`]. Defence SUBTRACTS from damage instead of scaling it, so
    /// armour that grew as fast as HP would floor every physical hit at `min_damage`
    /// out deep; armour that does not grow at all (as it did not) leaves nothing for
    /// an armour-piercing ability to pierce.
    pub fn def_mult(&self, d: i64) -> f64 {
        let base = 1.0 + d as f64 / self.b.world_scaling.stat_mult_base_divisor;
        base.powf(self.b.world_scaling.def_mult_exp)
    }

    /// The ON-RAMP: creature power ramps linearly from `onboarding_floor` at the hub
    /// to full strength at `onboarding_distance`, and is 1.0 everywhere past it.
    ///
    /// Every other curve here multiplies a creature's *base* stats, and at the shallow
    /// ring those multipliers are all ~1.0 — so the first creature a new account meets
    /// is simply whatever `[creature.<kind>]` says, tuned for no particular opponent.
    /// Measured, that was a level-1 solo losing better than half its opening fights.
    /// This is the one lever that makes "distance is the difficulty axis" true at the
    /// shallow end too, instead of only past the first tier.
    pub fn onboarding_mult(&self, d: i64) -> f64 {
        let ws = &self.b.world_scaling;
        if d >= ws.onboarding_distance || ws.onboarding_distance <= 0 {
            return 1.0;
        }
        let t = d as f64 / ws.onboarding_distance as f64;
        ws.onboarding_floor + (1.0 - ws.onboarding_floor) * t
    }

    /// XP curve (spec §4): `(1 + d/500)^1.5` — steeper than `stat_mult`, so a
    /// deep kill out-rewards a shallow one of the same creature by more than
    /// the fight got harder. The final award is `floor(base_xp × this)`.
    pub fn xp_mult(&self, d: i64) -> f64 {
        let base = 1.0 + d as f64 / self.b.world_scaling.stat_mult_base_divisor;
        base.powf(self.b.world_scaling.xp_distance_exp)
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
pub const BIOMES: [&str; 6] = ["field", "forest", "desert", "ashfall", "tundra", "mire"];

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
fn section_biome(
    balance: &Balance,
    run_seed: u64,
    i: usize,
    distance: i64,
    prev: Option<&str>,
    tutorial: bool,
) -> &'static str {
    if tutorial {
        return biome_for_distance(distance);
    }
    // A biome's creature ROSTER is not difficulty-neutral even though its scaling is:
    // desert and tundra lead with armoured bruisers a level-1 party cannot chew
    // through. `[biome_gate]` holds each theme back until the party has had room to
    // grow, so the shallow ring is an on-ramp rather than a coin toss.
    let unlocked =
        |b: &'static str| balance.biome_gate.get(b).copied().unwrap_or(0) <= distance;
    let cands: Vec<&'static str> = BIOMES
        .iter()
        .copied()
        .filter(|b| Some(*b) != prev && unlocked(b))
        .collect();
    // Never strand the generator: if the gates and the no-repeat rule between them
    // leave nothing, the no-repeat rule is the one that yields.
    let cands = if cands.is_empty() {
        let open: Vec<&'static str> =
            BIOMES.iter().copied().filter(|b| unlocked(b)).collect();
        if open.is_empty() {
            return BIOMES[0];
        }
        open
    } else {
        cands
    };
    let mut rng = Rng(biome_pick_seed(run_seed, i));
    cands[rng.below(cands.len())]
}

/// Creature content ids that spawn in a biome. Structural (content-extensible);
/// stats for each key live in `balance.toml` under `[creature.<key>]`.
/// Every ordinary creature kind the world can place, biome order. Derived from
/// [`creatures_for_biome`] rather than listed again, so a rule about creatures is held
/// against all of them and a new kind cannot be quietly left out of one.
pub fn all_creature_kinds() -> Vec<&'static str> {
    let mut out: Vec<&'static str> = Vec::new();
    for biome in BIOMES {
        for k in creatures_for_biome(biome) {
            if !out.contains(k) {
                out.push(k);
            }
        }
    }
    out
}

fn creatures_for_biome(biome: &str) -> &'static [&'static str] {
    // Each biome's 3rd creature is a distinct archetype — a fast aggressive SWARMER
    // or a slow tanky BRUISER — so the combat rhythm varies as you explore. Appended
    // (index 0 stays the tutorial creature). Stats live under `[creature.<key>]`.
    match biome {
        // The field is the forest's open ground: the same fauna, room to see it coming.
        "field" | "forest" => &["forest_bloom_stalker", "thornback_boar", "sporeling"],
        "desert" => &["dune_wyrm", "sand_shade", "dune_colossus"],
        "ashfall" => &["cinder_imp", "magma_golem", "ember_wisp"],
        "tundra" => &["frost_lurker", "ice_revenant", "glacier_maw"],
        _ => &["bog_serpent", "myconid_brute", "bog_stinger"],
    }
}

/// How many units a node of this content kind holds, from its **material class**
/// (`[harvest]`): a reagent patch is several quick gathers, an ore vein is a longer
/// dig. Keying the rhythm on the class rather than the node id is what makes the two
/// gathering professions play differently instead of merely banking different ids.
fn node_stock(balance: &Balance, kind: &str) -> i32 {
    let class = balance
        .resource
        .get(kind)
        .and_then(|r| meld_proto::materials::material(&r.material))
        .map(|m| m.class.wire())
        .unwrap_or("");
    balance.harvest.node_yield(class).0
}

/// Which biomes spawn a creature kind, in `BIOMES` order.
///
/// The inverse of [`creatures_for_biome`], so anything that has to tell a player where a
/// species lives reads the same table the generator places it from rather than a second
/// list that can quietly disagree.
pub fn biomes_of_creature(kind: &str) -> Vec<&'static str> {
    BIOMES
        .iter()
        .copied()
        .filter(|b| creatures_for_biome(b).contains(&kind))
        .collect()
}

/// Harvestable resource node ids that spawn in a biome (one alchemy reagent + one
/// forging ore/wood per biome). Structural; stats live under `[resource.<key>]`.
fn resources_for_biome(biome: &str) -> &'static [&'static str] {
    match biome {
        "field" | "forest" => &["bloom_herb", "heartoak_bark"],
        "desert" => &["sun_salts", "dune_iron"],
        "ashfall" => &["ember_ash", "cinder_ore"],
        "tundra" => &["frost_lichen", "rime_ore"],
        _ => &["bog_myrrh", "peat_iron"],
    }
}

/// Impassable terrain feature kinds per biome (drives client rendering; all block
/// movement identically). Structural content.
/// How much a section's maze-fill count must be multiplied to hold its DESIGNED density
/// (`obstacles_per_area` over one base-length corridor) at this section's radius and size.
///
/// **This rule has two call sites — `push_section` when a section is generated and
/// `reroll_props` when a Shift retiles one — and it lived twice.** That is the exact drift
/// this repo has been bitten by three times now (the wall-collision line that went into one
/// mover and not the other, the creature damage pass that kept the O(n²) scan the movement
/// pass above it had already lost, and this). A Shift re-scattering at a stale density is
/// invisible until someone measures the ring it landed on.
///
/// Density thins along TWO axes and only the first was ever compensated:
/// - the WG-4 fan bends a fixed corridor width into an arc that grows with radius, and
/// - `obstacles_per_area` is a count per SECTION, but sections grow with depth
///   (`area_length_growth`) — 13 units thick near the hub against 184 by d1560 — so the
///   same count also spreads over ever more radial extent.
///
/// Their product is exactly the section's world area over the base area's. Capped by
/// `maze_radial_scale_cap`, because the true stretch grows without bound in a world that
/// streams outward forever and every prop is rebuilt into the blocking field every tick.
/// Holding the designed density at every depth is NOT affordable under eager per-section
/// fill — see the note on that cap in `balance.toml`.
fn maze_fill_scale(
    wg: &meld_balance::WorldGen,
    radial_half: f64,
    lateral: f64,
    start_x: f64,
    end_x: f64,
) -> f64 {
    let length = (end_x - start_x).max(1.0);
    let r_mid = (start_x + end_x) * 0.5;
    let arc_stretch = if radial_half > 0.0 {
        (r_mid * radial_half / lateral.max(1.0)).max(1.0)
    } else {
        1.0
    };
    let thickness_scale = (length / wg.base_area_length.max(1.0)).max(1.0);
    (arc_stretch * thickness_scale).min(wg.maze_radial_scale_cap.max(1.0))
}

fn obstacles_for_biome(biome: &str) -> &'static [&'static str] {
    match biome {
        "field" | "forest" => &["tree", "boulder", "pond"],
        "desert" => &["dune", "rock_spire", "cactus"],
        "ashfall" => &["cliff", "lava", "cinder_rock"],
        "tundra" => &["ice_spire", "frozen_pond", "snow_drift"],
        _ => &["bog_pool", "mire_root", "fungal_wall"],
    }
}

/// The SIGNATURE prop a biome's dense maze fill is made of — the thing you're walking
/// through (a wood of trees, a scatter of cacti, a field of volcanic rock, …). Usually
/// solid geometry so the fill reads as cover — EXCEPT the Mire, which is MOSTLY water:
/// its fill is impassable `bog_pool` water, so the swamp floods into a maze of pools
/// with the trail as the only reliable land.
fn fill_kind_for_biome(biome: &str) -> &'static str {
    match biome {
        "field" | "forest" => "tree",
        "desert" => "cactus",
        "ashfall" => "cinder_rock",
        "tundra" => "ice_spire",
        _ => "bog_pool", // mire: flooded — water is the fill, land is the trail
    }
}

/// A biome's VERTICALITY weight — scales its terrace count AND path-climb chance so
/// each biome climbs differently: Ashfall is a mountain-pass MAZE (tall terraces block
/// most routes, the path almost always climbs); Forest has some rises but does its
/// mazing with trees; Tundra rolls gently; the Mire is flooded (little to climb, it's
/// mostly water); the Desert is the OPEN breather — nearly flat. Keeps the desert open
/// (#8) while the forest/ashfall become real mazes.
fn biome_terrace_mult(biome: &str) -> f64 {
    match biome {
        "ashfall" => 1.6, // a maze of mountain terraces — the path climbs constantly
        "forest" => 0.8,  // trees do the mazing; a few rises
        "field" => 0.3,   // open meadow — you can see across it
        "tundra" => 0.7,  // rolling
        "mire" => 0.35,   // flooded, not mountainous
        _ => 0.15,        // desert: the open breather — nearly flat
    }
}

/// A biome's maze-fill density multiplier (× `obstacles_per_area`). Each biome has its
/// own so it FEELS distinct; unlisted biomes fall back to `maze_obstacle_mult`.
fn biome_obstacle_mult(wg: &meld_balance::WorldGen, biome: &str) -> f64 {
    match biome {
        "field" => wg.field_obstacle_mult,
        "forest" => wg.forest_obstacle_mult,
        "desert" => wg.desert_obstacle_mult,
        "ashfall" => wg.ashfall_obstacle_mult,
        "tundra" => wg.tundra_obstacle_mult,
        "mire" => wg.mire_obstacle_mult,
        _ => wg.maze_obstacle_mult,
    }
}

/// A biome's combat-drop material — banked into the run backpack when a creature is
/// felled, distinct from harvestable resource nodes. These are the **trophies**
/// (`meld_proto::materials::MaterialClass::Trophy`): they feed the trophy potion line,
/// they are the Forge's catalyst ([`forge_gear`]), and the Broker buys them. The
/// registry is the contract — a key here that isn't in `materials::MATERIALS` is loot
/// nothing can spend. Structural content; deeper bands repeat Mire.
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
    /// Which class this item belongs to (`CLASS_KEYS`) — every drop is
    /// class-specific; there is no class-agnostic gear.
    pub class_key: String,
    pub tier: i32,
    pub atk_bonus: i32,
    pub def_bonus: i32,
    pub spd_bonus: i32,
    pub max_durability: i32,
    /// Elemental profile the piece grants its wearer (spec §5): DamageType wire
    /// key → multiplier (`0.75` = resists a quarter of that element). Empty for
    /// common/rare drops; epic+/signature pieces carry their biome's element.
    pub damage_modifiers: Vec<(String, f64)>,
    /// GR-5 weapon family wire word for hand slots (`sword`, `staff`, `globe`, …);
    /// empty for armor and accessories.
    pub family: String,
    /// GR-5 armor weight wire word for head/chest/legs (`heavy`, `robe`, …);
    /// empty for weapons and accessories.
    pub armor_weight: String,
    /// Which of the three tiers this is, which decides both how it is lost and how
    /// strong it rolled. Ephemeral is the most powerful gear in the game and cannot be
    /// banked; insured sits above ordinary kit and erodes; standard is the baseline.
    pub insurance: Insurance,
    /// AD-1 rolled affixes — the qualities that make this drop a build rather
    /// than a bigger number.
    pub affixes: Vec<meld_proto::affixes::Affix>,
    /// AD-1 unique key (`meld_proto::uniques::UNIQUES`); empty for ordinary loot.
    pub unique_key: String,
    /// AD-1 set key (`meld_proto::uniques::SETS`); empty when not part of a set.
    pub set_key: String,
}

/// The loot a felled encounter yields to one participant.
#[derive(Debug, Clone, PartialEq)]
pub struct CreatureLoot {
    /// Chits found (banked on extraction, lost on death). Scales with depth.
    pub chits: i64,
    /// The biome's combat material — the **trophy** the kill yields.
    pub material: &'static str,
    /// How many units of it. Scales with the size of the pack you felled (a
    /// carcass each) and, gently, with depth — so trophy *supply* tracks the
    /// difficulty of getting it, the way the chit haul already does. Deliberately
    /// draws no RNG: a crafter can plan a hunt.
    pub material_qty: i32,
    /// Red-chest gear. Ramps in with depth (see [`gear_drop_chance_at`]).
    pub gear: Option<GearDrop>,
    /// A `meld_proto::consumables` key, or empty for no potion (GR-8). Drawn from a
    /// **separate** RNG sub-stream, so it cannot shift any other roll in this function.
    pub potion: &'static str,
}

/// The playable classes' content keys, matching `meld_run::class_key`'s exact
/// spelling. Kept as a plain literal list here (rather than depending on meld-run's
/// `CharacterClass` enum) since this crate only ever needs the strings, to pick which
/// class a gear drop belongs to.
/// The classes a gear drop may be rolled FOR — the eight a player can actually field,
/// held against `meld_proto::unlocks` by test.
///
/// It used to be twelve, and the twelve were wrong in both directions: it carried five
/// classes nobody can field (`dragoon`, `sage`, `ranger`, `alchemist_knight`, `bard` — enum
/// variants with no kit, no stats and no unlock) and it **omitted the Hunter**, which is a
/// shipped, unlockable class. Every drop is class-locked (`can_wear` refuses another
/// class's piece), so five in twelve drops were unwearable by anybody alive and a Hunter
/// hero could never find a single piece of gear in the entire world. That is loot's own
/// version of the hand-written-list bug: the pool is derived from the unlock registry's
/// intent now, and a class added there but not here fails
/// `every_fieldable_class_can_find_gear`.
pub const CLASS_KEYS: [&str; 8] = [
    "explorer",
    "hunter",
    "psyker",
    "resonant",
    "shifter",
    "phoenix_guard",
    "smithwright",
    "keeper",
];

/// The universal 20-step power ladder (weakest → strongest), shared by every
/// class's catalog so a name's *rank* always reads the same way regardless of
/// class — only the noun (`class_slot_noun`) carries the class's flavor. A
/// drop's adjective is picked by its tier, indexed from the red-chest floor
/// tier and clamped into this range, so the *name* rides the same
/// distance-driven power curve `roll_creature_loot` already uses for the
/// numeric stat — a shallow kill can't hand out a name that reads as endgame
/// gear, and a deep one won't hand out a name that reads as starter junk.
const POWER_ADJECTIVES: [&str; 20] = [
    "Ashfall",
    "Cinderforged",
    "Emberwrought",
    "Scarab",
    "Duneglass",
    "Sunbaked",
    "Rimebound",
    "Frostforged",
    "Glacial",
    "Verdant",
    "Bloomforged",
    "Thornwood",
    "Miremere",
    "Fungal",
    "Peatbound",
    "Ashen",
    "Stormcaller's",
    "Voidforged",
    "Ancient",
    "Eternal",
];

/// Every class's signature noun per slot — every gear drop is class-specific
/// (economy.md S1 content pass): this is the word a `POWER_ADJECTIVES` entry
/// prefixes to build that class's 20-item catalog name for one slot.
pub fn class_slot_noun(class_key: &str, slot: &str) -> &'static str {
    match (class_key, slot) {
        ("explorer", "main_hand") => "Warblade",
        ("explorer", "chest") => "Battleplate",
        ("explorer", "accessory") => "Bloodcuff",
        ("dragoon", "main_hand") => "Lance",
        ("dragoon", "chest") => "Greaves",
        ("dragoon", "accessory") => "Windclasp",
        ("sage", "main_hand") => "Tome",
        ("sage", "chest") => "Vestments",
        ("sage", "accessory") => "Runestone",
        ("ranger", "main_hand") => "Longbow",
        ("ranger", "chest") => "Cloak",
        ("ranger", "accessory") => "Quiver Charm",
        ("alchemist_knight", "main_hand") => "Vialblade",
        ("alchemist_knight", "chest") => "Alchemal Plate",
        ("alchemist_knight", "accessory") => "Elixir Charm",
        ("bard", "main_hand") => "Songblade",
        ("bard", "chest") => "Minstrel's Coat",
        ("bard", "accessory") => "Lyre Pendant",
        ("psyker", "main_hand") => "Psi-Orb",
        ("psyker", "chest") => "Psi-Ward",
        ("psyker", "accessory") => "Mindshard",
        ("resonant", "main_hand") => "Ward Stave",
        ("resonant", "chest") => "Resonant Vestments",
        ("resonant", "accessory") => "Harmony Bell",
        ("shifter", "main_hand") => "Glitchblade",
        ("shifter", "chest") => "Runner's Wrap",
        ("shifter", "accessory") => "Flicker Charm",
        // The Hunter: the guild's trade is disposal, and its kit reads like a kill-tool.
        ("hunter", "main_hand") => "Culling Blade",
        ("hunter", "off_hand") => "Trapper's Buckler",
        ("hunter", "head") => "Stalker's Hood",
        ("hunter", "chest") => "Quarry Harness",
        ("hunter", "legs") => "Tracker's Greaves",
        ("hunter", "accessory") => "Adrenal Cuff",
        ("phoenix_guard", "main_hand") => "Kinetic Gauntlet",
        ("phoenix_guard", "chest") => "Bulwark Plate",
        ("phoenix_guard", "accessory") => "Aggro Band",
        // The two profession classes (MS-1): a Smithwright's kit is the trade's tools
        // worn as armour, a Keeper's is the garden carried on your back.
        ("smithwright", "main_hand") => "Forge Hammer",
        ("smithwright", "off_hand") => "Anvil Shield",
        ("smithwright", "head") => "Smelter's Mask",
        ("smithwright", "chest") => "Foundry Apron",
        ("smithwright", "legs") => "Slag Boots",
        ("smithwright", "accessory") => "Quench Ring",
        ("keeper", "main_hand") => "Grafting Stave",
        ("keeper", "off_hand") => "Seed Satchel",
        ("keeper", "head") => "Sunhood",
        ("keeper", "chest") => "Bloomweave",
        ("keeper", "legs") => "Roothose",
        ("keeper", "accessory") => "Terra Locket",
        // 7-slot expansion (Epic GR spec §5): off-hand / head / legs nouns.
        ("explorer", "off_hand") => "Targe",
        ("explorer", "head") => "Warhelm",
        ("explorer", "legs") => "Striders",
        ("dragoon", "off_hand") => "Wing Shield",
        ("dragoon", "head") => "Drakehelm",
        ("dragoon", "legs") => "Skygreaves",
        ("sage", "off_hand") => "Censer",
        ("sage", "head") => "Circlet",
        ("sage", "legs") => "Pilgrim Sandals",
        ("ranger", "off_hand") => "Bracer",
        ("ranger", "head") => "Hood",
        ("ranger", "legs") => "Trailboots",
        ("alchemist_knight", "off_hand") => "Alembic Shield",
        ("alchemist_knight", "head") => "Visored Helm",
        ("alchemist_knight", "legs") => "Plated Boots",
        ("bard", "off_hand") => "Chorus Buckler",
        ("bard", "head") => "Plumed Hat",
        ("bard", "legs") => "Dancer's Boots",
        ("psyker", "off_hand") => "Null Buckler",
        ("psyker", "head") => "Psi-Crown",
        ("psyker", "legs") => "Drift Boots",
        ("resonant", "off_hand") => "Chime Shield",
        ("resonant", "head") => "Halo Band",
        ("resonant", "legs") => "Grace Boots",
        ("shifter", "off_hand") => "Parry Dagger",
        ("shifter", "head") => "Runner's Cowl",
        ("shifter", "legs") => "Phase Boots",
        ("phoenix_guard", "off_hand") => "Tower Shield",
        ("phoenix_guard", "head") => "Great Helm",
        ("phoenix_guard", "legs") => "Anchor Boots",
        _ => "Trinket",
    }
}

/// One bespoke, unique flagship item per class+slot (30 total) — much
/// stronger and much rarer than the 20-item tiered catalog (see
/// `roll_creature_loot`'s `class_signature_*` roll).
fn class_signature_name(class_key: &str, slot: &str) -> &'static str {
    match (class_key, slot) {
        ("explorer", "main_hand") => "Bloodfang, the Frenzied Cleaver",
        ("explorer", "chest") => "Aegis of the Unbroken Line",
        ("explorer", "accessory") => "The Last Adrenaline",
        ("dragoon", "main_hand") => "Skyreaver, Lance of the Falling Star",
        ("dragoon", "chest") => "Stormstep Greaves",
        ("dragoon", "accessory") => "The Windbound Clasp",
        ("sage", "main_hand") => "The Unbound Codex",
        ("sage", "chest") => "Robes of the Still Mind",
        ("sage", "accessory") => "Runestone of First Light",
        ("ranger", "main_hand") => "Farsight, the Wind-Bent Bow",
        ("ranger", "chest") => "Cloak of the Silent Trail",
        ("ranger", "accessory") => "The Explorer's Mark",
        ("alchemist_knight", "main_hand") => "Mercurial Edge",
        ("alchemist_knight", "chest") => "Platemail of the Transmuted Heart",
        ("alchemist_knight", "accessory") => "The Philosopher's Vial",
        ("bard", "main_hand") => "The Last Refrain",
        ("bard", "chest") => "Coat of a Thousand Verses",
        ("bard", "accessory") => "The Siren's Pendant",
        ("hunter", "main_hand") => "Gravemaker, the Last Contract",
        ("hunter", "chest") => "Harness of the Long Hunt",
        ("hunter", "accessory") => "The Second Wind",
        ("psyker", "main_hand") => "The Fractured Lens",
        ("psyker", "chest") => "Ward of the Silent Mind",
        ("psyker", "accessory") => "Shard of the Second Sight",
        ("resonant", "main_hand") => "Scepter of the Unbroken Chord",
        ("resonant", "chest") => "Vestments of Everlasting Grace",
        ("resonant", "accessory") => "The Undying Bell",
        ("shifter", "main_hand") => "Paradox, the Glitched Kris",
        ("shifter", "chest") => "Wrap of a Thousand Steps",
        ("shifter", "accessory") => "The Flicker Between Moments",
        ("phoenix_guard", "main_hand") => "Worldender",
        ("phoenix_guard", "chest") => "The Immovable Bulwark",
        ("phoenix_guard", "accessory") => "Band of the Undying Wall",
        _ => "Unnamed Relic",
    }
}

/// Look up a drop's flavor name: the `POWER_ADJECTIVES` entry for how many
/// tiers past `floor_tier` (the red-chest floor's tier — the earliest a drop
/// can ever roll) this drop's tier is, clamped to the ladder's range, prefixed
/// onto the class+slot's noun (`class_slot_noun`).
fn gear_catalog_name(class_key: &str, slot: &str, tier: i32, floor_tier: i32) -> String {
    let idx = (tier - floor_tier).clamp(0, POWER_ADJECTIVES.len() as i32 - 1) as usize;
    format!("{} {}", POWER_ADJECTIVES[idx], class_slot_noun(class_key, slot))
}

/// P(a felled encounter drops gear) at `distance`, before any reward spike (GR-8).
///
/// Zero below `[loot] gear_ramp_start_distance`, then linear from
/// `gear_ramp_start_mult` of `gear_drop_chance` up to all of it at
/// `red_chest_floor_distance` — the depth CANON §B names as the gear game's home.
/// A hard cutoff at that floor is not usable as the only rule: the chain's deep
/// portal sits barely past it, so a cutoff means a whole dive with the chase
/// switched off. Every distance at or past the floor gets exactly
/// `gear_drop_chance`, so nothing about deep loot moves.
pub fn gear_drop_chance_at(balance: &Balance, distance: i64) -> f64 {
    let l = &balance.loot;
    let floor = balance.world_scaling.red_chest_floor_distance;
    let start = l.gear_ramp_start_distance;
    if distance < start {
        return 0.0;
    }
    // A floor at or below the ramp start collapses the ramp rather than dividing by
    // zero: a tuner who drops the floor to 0 means "full rate everywhere".
    let t = if floor <= start {
        1.0
    } else {
        ((distance - start) as f64 / (floor - start) as f64).clamp(0.0, 1.0)
    };
    let m = l.gear_ramp_start_mult.clamp(0.0, 1.0);
    l.gear_drop_chance * (m + (1.0 - m) * t)
}

/// The potions a kill at `distance` may drop (GR-8), deepest-appropriate last.
///
/// Band-capped: a potion is eligible once `tier(distance)` reaches its own
/// `ConsumableDef::tier`, which is what holds the trophy line behind the depth it was
/// authored for. `Revive`/`Experience` are excluded because those two already have
/// their own dedicated faucets (`[consumable] world_revive_item_chance` /
/// `world_xp_item_chance`) — including them here would quietly double their rate.
/// Derived from the registry rather than a key list, so a new potion joins the pool
/// by existing.
pub fn potion_drop_pool(balance: &Balance, distance: i64) -> Vec<&'static str> {
    use meld_proto::consumables::{ConsumableEffect as E, CONSUMABLES};
    let band = Scaling::new(balance).tier(distance) as i32;
    CONSUMABLES
        .iter()
        .filter(|c| !matches!(c.effect, E::Revive | E::Experience))
        .filter(|c| c.tier <= band)
        .map(|c| c.key)
        .collect()
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
    // Trophies per felled creature, plus a band bonus, times the reward spike. No
    // RNG draw here on purpose: it would shift every subsequent roll in this
    // stream (the gear check below), and a predictable trophy yield is what lets a
    // crafter decide "four more of those and I can quench a blade".
    let material_qty = ((l.material_per_creature
        * monster_count.max(1) as f64
        * (1.0 + sc.tier(distance) as f64 * l.material_qty_per_tier)
        * loot_mult.max(0.0))
    .round() as i32)
        .max(1);
    // Red-chest gear ramps in with depth (`gear_drop_chance_at`) and a reward spike
    // (loot_mult) boosts — and can guarantee — the drop. Still exactly ONE `rng.unit()`
    // draw whatever the chance works out to, so the stream past here is unmoved.
    let gear = if rng.unit() < (gear_drop_chance_at(balance, distance) * loot_mult.max(0.0)).min(1.0)
    {
        let tier = sc.tier(distance) as i32;
        let a_cfg = &balance.affix;
        let floor_tier = sc.tier(balance.world_scaling.red_chest_floor_distance) as i32;
        // The six item categories of the 7-slot loadout (Epic GR spec §5):
        // ACCESSORY_1/2 are two *equip* slots sharing the one accessory category.
        let mut slot =
            ["main_hand", "off_hand", "head", "chest", "legs", "accessory"][rng.below(6)];
        // Every drop belongs to one of the FIELDABLE classes (no class-agnostic gear) —
        // picked independent of the party's actual composition, like any other loot roll;
        // a hero can only wear/benefit from gear that matches their own class (enforced
        // server-side at equip/battle time). Which is exactly why the pool may only ever
        // hold classes somebody can play: a drop for a class with no kit is loot nobody in
        // the game can use.
        let class_key = CLASS_KEYS[rng.below(CLASS_KEYS.len())];
        // GR-5: the drop's family/weight come from the class it belongs to, so a
        // Resonant drop is a stave and an Phoenix Guard drop is plate. A two-handed
        // class has no off-hand to fill, so an off_hand roll becomes its main
        // hand rather than an unwearable (dead) drop.
        let drop_class = eq::class_from_key(class_key);
        if slot == "off_hand" && drop_class.map(|c| !eq::has_off_hand(c)).unwrap_or(false) {
            slot = "main_hand";
        }
        let (family, armor_weight) = match (drop_class, slot) {
            (Some(c), "main_hand" | "off_hand") => {
                let legal = eq::families_for_slot(c, slot);
                let f = legal
                    .get(if legal.len() > 1 { rng.below(legal.len()) } else { 0 })
                    .copied();
                (f.map(|f| f.wire().to_string()).unwrap_or_default(), String::new())
            }
            (Some(c), "head" | "chest" | "legs") => {
                (String::new(), eq::drop_weight(c).wire().to_string())
            }
            _ => (String::new(), String::new()),
        };
        let gjitter = 1.0 + rng.signed() * l.gear_atk_jitter;
        // Rarity: the encounter's loot spike multiplies the rare/epic/legendary
        // odds (so elites/gatekeepers drop the shiny stuff), capped so Common is
        // always possible. Rarity then scales the stat bonus + flavours the name.
        let gr = &balance.gear_rarity;
        let boost = loot_mult.max(1.0);
        // Distance-shifted weights (spec §4): every 2 tiers the non-common
        // weights grow 10% (and Common, being the remainder, shrinks to match)
        // — the deep world drops progressively shinier loot.
        let depth_shift = 1.0 + gr.rarity_shift_per_2_tiers * (tier / 2).max(0) as f64;
        let (mut w_rare, mut w_epic, mut w_leg) = (
            gr.rare_weight * boost * depth_shift,
            gr.epic_weight * boost * depth_shift,
            gr.legendary_weight * boost * depth_shift,
        );
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
        // Class-signature: a much rarer, much stronger unique named piece for
        // this class+slot (`class_signature_name`) — independent of rarity (a
        // signature item can itself still separately roll Legendary), gated
        // behind a minimum tier so it can't show up on a shallow kill.
        // Signatures exist only for the original three categories (the 30-relic
        // catalog); the RNG still advances uniformly so rolls stay reproducible.
        let has_signature = matches!(slot, "main_hand" | "chest" | "accessory");
        let is_signature = tier >= gr.class_signature_min_tier
            && rng.unit() < gr.class_signature_chance
            && has_signature;
        let signature_mult = if is_signature { gr.class_signature_mult } else { 1.0 };
        // The tier is rolled BEFORE the stat because it scales it. Only a reward-spike
        // encounter (`loot_mult > 1` — a champion, gatekeeper, rite or chest, and
        // nothing else) can yield the two special tiers; trash always drops standard.
        let spike = loot_mult > 1.0;
        let roll = rng.unit();
        let insurance = if spike && roll < l.ephemeral_gear_chance {
            Insurance::Ephemeral
        } else if spike && roll < l.ephemeral_gear_chance + l.permanent_gear_chance {
            Insurance::Insured
        } else {
            Insurance::Standard
        };
        let tier_mult = match insurance {
            Insurance::Ephemeral => l.ephemeral_power_mult,
            Insurance::Insured => l.insured_power_mult,
            Insurance::Standard => 1.0,
        };
        // AN EPHEMERAL PIECE IS NEVER COMMON. Rarity and insurance are two independent
        // rolls, so the combination was reachable — and a common carries `count_common`
        // affixes, which is ZERO. That made the strongest tier in the game capable of
        // dropping a piece with no build on it at all: it burns when you reach the city,
        // burns when its wearer falls, and in exchange offers one inflated stat number.
        // Strictly worse than the standard drop beside it, for the tier that is supposed to
        // be the reason you push deeper. The floor is structural rather than a tunable: the
        // rule is "the tier that defines a run always carries a build", not a coefficient.
        let (rarity, rarity_mult) = if insurance == Insurance::Ephemeral && rarity == "common" {
            ("rare", gr.rare_mult)
        } else {
            (rarity, rarity_mult)
        };
        // One roll, routed into whichever stat this slot cares about: weapon
        // hits harder, armor shrugs off more, an accessory moves faster.
        let stat = (l.gear_atk_per_tier * tier as f64 * gjitter * rarity_mult * signature_mult * tier_mult)
            .round()
            .max(1.0) as i32;
        // Stat routing: the main hand hits harder, an accessory moves faster,
        // and every protective piece (off-hand/head/chest/legs) shrugs off more.
        let (atk_bonus, def_bonus, spd_bonus) = match slot {
            "main_hand" => (stat, 0, 0),
            "accessory" => (0, 0, stat),
            _ => (0, stat, 0),
        };
        // The catalog name already rides the depth curve (see `gear_catalog_name`);
        // rarity prefixes it on top ("Legendary Ashfall Warblade") rather than
        // picking a separate biome-adjective name, so depth and rarity both read
        // in the same name instead of fighting each other.
        let base_name = if is_signature {
            class_signature_name(class_key, slot).to_string()
        } else {
            gear_catalog_name(class_key, slot, tier, floor_tier)
        };
        // AD-1 chase tiers. A unique replaces the drop wholesale — its slot, name,
        // affixes and drawback are authored, not rolled — and only a reward spike
        // (elite / Gatekeeper / boss, `loot_mult > 1`) can produce one.
        let spiked = loot_mult > 1.0;
        let unique_def = (tier >= a_cfg.unique_min_tier
            && (spiked || !a_cfg.unique_requires_spike)
            && rng.unit() < a_cfg.unique_chance)
            .then(|| {
                let pool: Vec<&uq::UniqueDef> = uq::UNIQUES
                    .iter()
                    .filter(|u| match u.only_class {
                        Some(c) => eq::class_key(c) == class_key,
                        None => true,
                    })
                    .collect();
                (!pool.is_empty()).then(|| pool[rng.below(pool.len())])
            })
            .flatten();
        // Set membership is independent of rarity: a plain-looking piece can be the
        // third Warden's March you needed.
        let set_key = if tier >= a_cfg.set_min_tier && rng.unit() < a_cfg.set_chance {
            uq::SETS[rng.below(uq::SETS.len())].key.to_string()
        } else {
            String::new()
        };
        // AD-1: roll this drop's affixes before naming it, so the name can carry the
        // suffix of whichever affix defines the piece.
        let affixes = roll_affixes(
            balance,
            &mut rng,
            tier,
            rarity,
            is_signature,
            insurance == Insurance::Ephemeral,
            class_key,
            slot,
            biome_for_distance(distance),
        );
        let (slot, affixes, base_name) = match unique_def {
            Some(u) => (u.slot, u.rolled(), u.name.to_string()),
            None => {
                let named = match aff::name_suffix(&affixes) {
                    Some(suffix) => format!("{base_name} {suffix}"),
                    None => base_name,
                };
                (slot, affixes, named)
            }
        };
        // A unique keeps its own name — no rarity prefix, no suffix.
        let name = if unique_def.is_some() || rarity == "common" {
            base_name
        } else {
            // Title-case the rarity for the name ("Legendary Frostforged Greatblade").
            let mut c = rarity.chars();
            let cap = c.next().unwrap().to_uppercase().collect::<String>() + c.as_str();
            format!("{cap} {base_name}")
        };
        // Epic+/signature pieces carry an elemental ward themed to the biome
        // they dropped in (spec §5 gear damage_modifiers) — a quarter-resist
        // to the local element, aggregated (and clamped) server-side at battle
        // assembly with every other equipped piece.
        let damage_modifiers = if is_signature || rarity == "epic" || rarity == "legendary" {
            let elem = match biome_for_distance(distance) {
                "forest" => "POISON",
                "desert" => "WIND",
                "ashfall" => "FIRE",
                "tundra" => "ICE",
                _ => "POISON",
            };
            vec![(elem.to_string(), 0.75)]
        } else {
            Vec::new()
        };
        Some(GearDrop {
            name,
            rarity: rarity.to_string(),
            slot: slot.to_string(),
            class_key: class_key.to_string(),
            tier,
            atk_bonus,
            def_bonus,
            spd_bonus,
            max_durability: roll_durability(balance, &mut rng, &affixes),
            damage_modifiers,
            family,
            armor_weight,
            affixes,
            unique_key: unique_def.map(|u| u.key.to_string()).unwrap_or_default(),
            set_key,
            insurance,
        })
    } else {
        None
    };
    // Its OWN sub-stream, not `rng`: a draw taken from the shared stream here would
    // shift every gear/affix/rarity roll above it, which is the trap the trophy-qty
    // calculation is written to avoid too.
    let mut prng = Rng(seed ^ 0x9017_1047_9017_1047);
    let pool = potion_drop_pool(balance, distance);
    let potion = if !pool.is_empty()
        && prng.unit() < (l.potion_drop_chance * loot_mult.max(0.0)).min(1.0)
    {
        pool[prng.below(pool.len())]
    } else {
        ""
    };
    CreatureLoot {
        chits,
        material,
        material_qty,
        gear,
        potion,
    }
}

/// Roll a drop's affixes (AD-1, docs/proposals/gear-identity.md §3).
///
/// Deterministic from the caller's seeded `rng` like every other loot roll, and
/// **tier-gated per affix class** (`[affix]` floors) so a shallow drop is still a
/// plain stat stick a new player can read (P1-3) and depth is where builds appear.
/// A Keyword affix only rolls for the class the item belongs to — its whole point
/// is twisting *that* class's mechanic. A Synergy affix names a *different* class,
/// so it can only pay off in a mixed party.
#[allow(clippy::too_many_arguments)]
pub(crate) fn roll_affixes(
    balance: &Balance,
    rng: &mut Rng,
    tier: i32,
    rarity: &str,
    is_signature: bool,
    // An EPHEMERAL piece rolls `count_ephemeral_bonus` extra lines. Ephemeral is the tier
    // that cannot be banked and burns when its wearer falls, so what it buys is not a
    // bigger number (that is `ephemeral_power_mult`) but a wider BUILD — the strongest
    // synergies in the game being ones you can only hold together for a single dive.
    is_ephemeral: bool,
    class_key: &str,
    slot: &str,
    biome: &str,
) -> Vec<aff::Affix> {
    let a = &balance.affix;
    let count = a.count_for(rarity, is_signature, is_ephemeral);
    if count == 0 {
        return Vec::new();
    }
    // The pool: every affix whose class has unlocked at this tier, minus keyword
    // affixes belonging to another class.
    let hand = slot == "main_hand" || slot == "off_hand";
    let pool: Vec<&aff::AffixDef> = aff::AFFIXES
        .iter()
        .filter(|d| tier >= a.tier_floor(affix_class_word(d.class)))
        .filter(|d| match d.only_class {
            Some(c) => eq::class_key(c) == class_key,
            None => true,
        })
        // A brand decides what your attacks ARE, so only a weapon may carry one.
        .filter(|d| d.key != "brand" || hand)
        .collect();
    if pool.is_empty() {
        return Vec::new();
    }
    // WEIGHTED, and WITHOUT REPLACEMENT. Two things were wrong with a uniform draw that
    // skipped duplicates:
    //
    // * Flat odds meant nothing was a prize. Every reachable key landed on 32-34% of a deep
    //   legendary, so `brand` — which decides what damage type your attacks ARE — was
    //   exactly as common as `masterwork`, extra durability. A wider roll was more random
    //   lines rather than a better-targeted build, and there was nothing to chase.
    // * `continue` on a duplicate silently ATE the line, so the counts overstated what
    //   landed and the gap grew with the count: a nominal 5 delivered 4.29, a nominal 6
    //   delivered 4.97. Removing the drawn entry instead is exact — `min(count, pool)` lines
    //   every time, and a piece still never rolls the same key twice.
    let mut pool: Vec<(&aff::AffixDef, f64)> = pool
        .into_iter()
        .map(|d| (d, a.weight(affix_class_word(d.class))))
        .collect();
    let mut out: Vec<aff::Affix> = Vec::new();
    for _ in 0..count {
        if pool.is_empty() {
            break;
        }
        let total: f64 = pool.iter().map(|(_, w)| *w).sum();
        let mut pick = rng.unit() * total;
        let mut idx = pool.len() - 1;
        for (i, (_, w)) in pool.iter().enumerate() {
            if pick < *w {
                idx = i;
                break;
            }
            pick -= *w;
        }
        // `swap_remove` rather than `remove`: the ORDER of the remaining pool never reaches
        // the output (the weighted walk above re-derives its own total every draw), so the
        // cheap removal is safe and still fully deterministic for a given seed.
        let (d, _) = pool.swap_remove(idx);
        let jitter = 1.0 + rng.signed() * a.magnitude_jitter;
        let magnitude = match d.class {
            // A brand has no magnitude; a resist does.
            _ if d.key == "brand" => 1,
            // A percent of extra max durability, jittered like any other roll so two
            // masterwork pieces are not the same piece.
            aff::AffixClass::Quality => {
                ((a.masterwork_durability_pct as f64 * jitter).round() as i32).max(1)
            }
            // Both element affixes read as a PERCENTAGE — resisted, or dealt extra.
            aff::AffixClass::Element => (a.resist_pct_per_tier * tier).clamp(1, a.resist_pct_cap),
            _ => ((a.magnitude_per_tier * tier.max(1) as f64 * d.scale * jitter).round() as i32)
                .max(1),
        };
        out.push(aff::Affix {
            key: d.key.to_string(),
            magnitude,
            element: matches!(d.class, aff::AffixClass::Element)
                .then(|| biome_element(biome).to_string()),
            ally_class: matches!(d.class, aff::AffixClass::Synergy).then(|| {
                // Name a class that is NOT this item's own, so the affix always
                // asks the player to build a mixed party.
                let others: Vec<&&str> =
                    CLASS_KEYS.iter().filter(|k| **k != class_key).collect();
                others[rng.below(others.len())].to_string()
            }),
        });
    }
    out
}

/// The `[affix]` tunable key for an affix class.
fn affix_class_word(class: aff::AffixClass) -> &'static str {
    match class {
        aff::AffixClass::Stat => "stat",
        aff::AffixClass::Element => "element",
        aff::AffixClass::Ward => "ward",
        aff::AffixClass::Keyword => "keyword",
        aff::AffixClass::Synergy => "synergy",
        aff::AffixClass::Quality => "quality",
    }
}

/// How much punishment this particular piece can take, in points of max durability.
///
/// Rolled per drop rather than read off one constant, because the loss per hero death
/// is FLAT points (`durability_loss_per_fall`) — so durability is the number of deaths
/// the piece survives, and two swords that differ here differ in something a player
/// can feel. An `of Masterwork` roll lifts it clear of the band entirely.
///
/// Takes its draw LAST, after the affixes it reads, so adding it did not shift any
/// roll above it in the same stream (the trap the trophy-quantity draw is written to
/// avoid too).
fn roll_durability(balance: &Balance, rng: &mut Rng, affixes: &[aff::Affix]) -> i32 {
    let l = &balance.loot;
    let jitter = 1.0 + rng.signed() * l.gear_durability_jitter;
    let mut points = (l.gear_base_durability as f64) * jitter;
    if let Some(m) = affixes.iter().find(|a| a.key == "masterwork") {
        points *= 1.0 + (m.magnitude as f64) / 100.0;
    }
    (points.round() as i32).max(1)
}

/// The element a biome's gear wards against.
fn biome_element(biome: &str) -> &'static str {
    match biome {
        "desert" => "WIND",
        "ashfall" => "FIRE",
        "tundra" => "ICE",
        _ => "POISON",
    }
}

/// Forge one piece of gear (MS-1). The crafted counterpart to
/// [`roll_creature_loot`]'s drop: the smith chooses the slot and the class, and
/// **Forging level** decides both how deep a tier they can work at and how tightly
/// the stat rolls (`variance_at` — a master smith is consistent, an apprentice is
/// not). Affixes come from the same tier-gated roller as loot, so a forged piece is
/// the same KIND of object as a found one; the difference is that you chose its slot.
///
/// `catalyzed` is a **trophy** quenched into the piece: it buys `catalyst_tier_bonus`
/// tiers past the smith's own reach and rolls the affix pool at epic instead of rare.
/// Levelling raises the floor; monster parts raise the ceiling.
pub fn forge_gear(
    balance: &Balance,
    forging_level: i32,
    slot: &str,
    class_key: &str,
    biome: &str,
    catalyzed: bool,
    seed: u64,
) -> GearDrop {
    let fg = &balance.forge;
    let tier = if catalyzed {
        fg.catalyzed_tier(forging_level)
    } else {
        fg.forgeable_tier(forging_level)
    }
    .max(0);
    rolled_gear(
        balance,
        tier,
        if catalyzed { "epic" } else { "rare" },
        fg.variance_at(forging_level),
        slot,
        class_key,
        biome,
        seed,
    )
}

/// An **insured** piece rolled at a stated tier and rarity: the one roll path behind
/// anything the persistent world hands over as a made-or-awarded item (the Forge, a Hunt
/// Board payout). A second copy of this body is a second game's worth of drift.
#[allow(clippy::too_many_arguments)]
pub fn rolled_gear(
    balance: &Balance,
    tier: i32,
    rarity: &str,
    variance: f64,
    slot: &str,
    class_key: &str,
    biome: &str,
    seed: u64,
) -> GearDrop {
    let l = &balance.loot;
    let mut rng = Rng(seed);
    let tier = tier.max(0);
    let jitter = 1.0 + rng.signed() * variance;
    let stat = ((l.gear_atk_per_tier * tier.max(1) as f64 * jitter).round() as i32).max(1);
    let (atk_bonus, def_bonus, spd_bonus) = match slot {
        "main_hand" => (stat, 0, 0),
        "accessory" => (0, 0, stat),
        _ => (0, stat, 0),
    };
    let family = match eq::class_from_key(class_key) {
        Some(c) => {
            let legal = eq::families_for_slot(c, slot);
            legal
                .get(if legal.len() > 1 { rng.below(legal.len()) } else { 0 })
                .map(|f| f.wire().to_string())
                .unwrap_or_default()
        }
        None => String::new(),
    };
    let armor_weight = match (eq::class_from_key(class_key), eq::is_armor_slot(slot)) {
        (Some(c), true) => eq::drop_weight(c).wire().to_string(),
        _ => String::new(),
    };
    let affixes = roll_affixes(balance, &mut rng, tier, rarity, false, false, class_key, slot, biome);
    let base = format!(
        "{} {}",
        POWER_ADJECTIVES[rng.below(POWER_ADJECTIVES.len())],
        class_slot_noun(class_key, slot)
    );
    let name = match aff::name_suffix(&affixes) {
        Some(suffix) => format!("{base} {suffix}"),
        None => base,
    };
    GearDrop {
        name,
        rarity: rarity.to_string(),
        slot: slot.to_string(),
        class_key: class_key.to_string(),
        tier,
        atk_bonus,
        def_bonus,
        spd_bonus,
        max_durability: roll_durability(balance, &mut rng, &affixes),
        damage_modifiers: Vec::new(),
        family,
        armor_weight,
        affixes,
        unique_key: String::new(),
        set_key: String::new(),
        // Earned in town — made at a forge, or paid out for work finished — so it does
        // not evaporate the first time you die.
        insurance: Insurance::Insured,
    }
}

/// Shop-counter gear (`EC-2`): the plain, honest baseline a city vendor sells for
/// chits — tier 0, common, **no affixes**, `Standard` insurance.
///
/// Deliberately the dullest gear in the game. A shop exists so a player who died with
/// nothing can walk back out equipped, not so chits can buy their way past the loot
/// chase: what a Requisition counter stocks must always be worse than what a smith
/// forges or a creature drops. No RNG at all, so a price can be a fixed number and the
/// player knows exactly what they are buying.
pub fn shop_gear(balance: &Balance, slot: &str, class_key: &str) -> GearDrop {
    let l = &balance.loot;
    let stat = (l.gear_atk_per_tier.round() as i32).max(1);
    let (atk_bonus, def_bonus, spd_bonus) = match slot {
        "main_hand" => (stat, 0, 0),
        "accessory" => (0, 0, stat),
        _ => (0, stat, 0),
    };
    let family = match eq::class_from_key(class_key) {
        Some(c) => eq::families_for_slot(c, slot)
            .first()
            .map(|f| f.wire().to_string())
            .unwrap_or_default(),
        None => String::new(),
    };
    let armor_weight = match (eq::class_from_key(class_key), eq::is_armor_slot(slot)) {
        (Some(c), true) => eq::drop_weight(c).wire().to_string(),
        _ => String::new(),
    };
    GearDrop {
        name: format!("Issued {}", class_slot_noun(class_key, slot)),
        rarity: "common".to_string(),
        slot: slot.to_string(),
        class_key: class_key.to_string(),
        tier: 0,
        atk_bonus,
        def_bonus,
        spd_bonus,
        max_durability: l.gear_base_durability,
        damage_modifiers: Vec::new(),
        family,
        armor_weight,
        insurance: meld_proto::Insurance::Standard,
        affixes: Vec::new(),
        unique_key: String::new(),
        set_key: String::new(),
    }
}

/// Reroll a piece's affixes (MS-1, closing AD-1's last thread): same tier, same
/// slot, a fresh draw. What a smith sells is another chance at the roll, not a
/// better piece — the stats are untouched.
pub fn reroll_affixes(
    balance: &Balance,
    tier: i32,
    class_key: &str,
    slot: &str,
    biome: &str,
    seed: u64,
) -> Vec<aff::Affix> {
    reroll_affixes_at(balance, tier, class_key, slot, biome, "rare", seed)
}

/// Same, at a chosen rarity pool. The smithing tempo game (MS-1) earns the pool with the
/// heat's quality, so the rarity is an input rather than a constant.
#[allow(clippy::too_many_arguments)]
pub fn reroll_affixes_at(
    balance: &Balance,
    tier: i32,
    class_key: &str,
    slot: &str,
    biome: &str,
    rarity: &str,
    seed: u64,
) -> Vec<aff::Affix> {
    let mut rng = Rng(seed);
    roll_affixes(balance, &mut rng, tier, rarity, false, false, class_key, slot, biome)
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
    /// How many PARTIES this encounter is sized for (`meld_proto::warbands`). 1 for
    /// everything ordinary.
    ///
    /// A gatekeeper's HP wall was always sized for a merge and never declared it, so a solo
    /// party met a four-party boss with nothing on screen to say so. The count is the single
    /// source of both the scaling and the name — derive one from the other and they cannot
    /// disagree about how big the fight is.
    pub expects_parties: u8,
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
    /// Which of the 10 named bosses (FS-4 "unique boss mechanics") this Elite/
    /// Gatekeeper fights as — empty for a standard spawn. Drives a bespoke
    /// ability pool (`meld_world::abilities`), the in-battle display name
    /// (`boss_display_name`), and the client's boss sprite/animation. Set by
    /// `pick_elite_boss_kind` / `pick_gatekeeper_boss_kind` at promotion time.
    pub boss_kind: String,
    pub faction: String,
    /// `passive` | `territorial` | `aggressive`.
    pub aggression: String,
    /// THE END FIGHT: which damage family this one of the three shrugs off — `"mind"`,
    /// `"physical"` or `"elemental"`, empty for everything else.
    ///
    /// Each of the three carries a DIFFERENT ward on purpose, so **no single damage source
    /// clears the encounter**. A stack of four Psykers deletes anything that only has
    /// armour to hide behind (Foci ignore defence entirely and ride Mnd, which comes from
    /// levelling rather than loot) — measured at 6 rounds against the intended 25, taking
    /// no hits at all. A ward the Psyker cannot burn through is what makes bringing a mixed
    /// party the answer, without touching the class that earned its kit.
    pub set_piece_ward: String,
    /// Which RANK this creature stands in. The back rank halves the physical damage it
    /// takes AND the physical damage it deals (`back_row_damage_mult` /
    /// `back_row_attack_mult`) — the same trade a hero's back row makes, in the same
    /// engine code. So a pack in formation is not just more creatures: its rear is
    /// protected from a sword and answered by a spell, an elemental brand, or reach.
    ///
    /// A formation is also what makes a BIG pack readable — ten in a line runs off the
    /// screen, two ranks of five do not, which is why every game that fields packs this
    /// size stacks them.
    pub back_row: bool,
    /// Seconds this creature remains PINNED by a Psyker (CL-2), counted down by
    /// [`Arena::step_creatures_with_aggro`]. A pinned creature does not move, chase or
    /// skirmish — but it is still touchable and still fights when reached, because the
    /// pin is an opening the party chooses to take, not a way to delete an encounter.
    pub held_for: f64,
    /// The one player this creature exists for, or empty for everyone's world (AD-4
    /// bounty marks). A mark is left out of every other player's snapshot and cannot be
    /// touched by them, so a contract with your name on it is *yours* — including in a
    /// co-op instance, where the party can fight it beside you but never trigger it.
    pub owner: String,
    /// The bounty this creature IS, or empty. Felling it completes that contract, which
    /// is decided by identity rather than by matching a species after the fact.
    pub bounty: String,
    pub flees: bool,
    /// World-scaled combat stats (stat_mult applied at spawn — no rescale later).
    pub hp: i32,
    /// Full HP at spawn (= `hp` before any damage). Overworld mobs lose `hp` to
    /// hostile-faction skirmishes, so `hp/max_hp` is a meaningful pre-fight bar
    /// (surfaced to the client for the Explorer's HP-intel perk).
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
    /// Seconds until this creature can land its next overworld skirmish blow. Public
    /// because a clash feed (`CR-2`) shows it as the creature's gauge — watching a brawl
    /// means watching the wind-up to each blow, not just the HP falling afterwards.
    pub skirmish_cd: f64,
    /// Sub-1 HP banked from the slow roaming regen (`CR-2`). `hp` is an integer and the
    /// regen is a small fraction of `max_hp` per second, so without a remainder the whole
    /// mechanic rounds to nothing on any creature whose max HP is under a few hundred —
    /// which is every creature in the on-ramp. Same reason the Resonant's overworld regen
    /// banks its own remainder.
    regen_accum: f64,
    /// Where this creature is currently walking while it has nothing to chase, or
    /// `None` before it has picked its first destination.
    ///
    /// A WANDER DESTINATION HAS TO OUTLIVE THE TICK THAT PICKED IT. This used to be
    /// re-rolled inside [`Arena::step_creatures_with_aggro`] on every pass — a fresh
    /// angle ten times a second at the 100 ms authoritative tick — so every creature
    /// in the overworld was chasing a point that teleported around its leash faster
    /// than it could walk. Measured over 30 s of wander: 47.8 tiles of path walked
    /// (full speed the whole time) for **0.87 tiles** of net displacement, never more
    /// than 1.93 tiles from where it started. 98% of the motion cancelled, so the
    /// whole world read as vibrating in place — and because the client picks its 8-way
    /// facing off frame-to-frame movement (`hd2d::animate_chars`), the sprites spun on
    /// the spot as well.
    wander_to: Option<Position>,
    /// Seconds left to pursue [`Self::wander_to`] before giving up and picking another.
    /// A creature can be walked into a rock by its own destination (the mover slides
    /// per-axis and then stops), so a leg is time-bounded as well as arrival-bounded —
    /// without it, a blocked creature grinds against the same tree for the whole dive.
    wander_left: f64,
    /// Seconds left standing still before the next leg. A creature that walks without
    /// ever stopping reads as machinery; the pause is what makes it read as grazing.
    wander_wait: f64,
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
        let ramp = scaling.onboarding_mult(d);
        let mult = scaling.stat_mult(d) * ramp;
        MonsterSpawn {
            entity_id,
            monster_kind: kind.to_string(),
            affix: String::new(),
            expects_parties: 1,
            position,
            home: position,
            area_min_x: f64::NEG_INFINITY,
            area_max_x: f64::INFINITY,
            level: scaling.mlevel(d),
            elevation: 0,
            encounter_class: stats.encounter_class.clone(),
            boss_kind: String::new(),
            faction: stats.faction.clone(),
            aggression: stats.aggression.clone(),
            set_piece_ward: String::new(),
            back_row: false,
            held_for: 0.0,
            owner: String::new(),
            bounty: String::new(),
            flees: stats.flees,
            hp: ((stats.base_hp as f64) * scaling.hp_mult(d) * ramp).round().max(1.0) as i32,
            max_hp: ((stats.base_hp as f64) * scaling.hp_mult(d) * ramp).round().max(1.0) as i32,
            atk: ((stats.base_atk as f64) * mult).round().max(1.0) as i32,
            def: ((stats.base_def as f64) * scaling.def_mult(d) * ramp).round() as i32,
            speed_stat: stats.speed_stat,
            // XP rides its own curve (spec §4): floor(base_xp × (1 + d/500)^1.5) —
            // steeper than the stat curve, so difficulty AND reward both ride
            // `distance` and the deep kill out-earns the shallow grind. The biome
            // stays a difficulty-neutral skin (base stats are the d=0 budget).
            xp_reward: ((stats.xp_reward as f64) * scaling.xp_mult(d)).floor().max(0.0) as i64,
            loot_kind: stats.loot_kind.clone(),
            defeated: false,
            in_battle: false,
            skirmish_cd: 0.0,
            regen_accum: 0.0,
            wander_to: None,
            wander_left: 0.0,
            wander_wait: 0.0,
            rng: seed | 1,
        }
    }

    /// Build a **dungeon boss** (WG-1/DG-3b) scaled to a dungeon's stamped
    /// `effective_distance` (not a world position): a biome creature's stats scaled
    /// by `stat_mult(effective_distance)`, promoted to Gatekeeper tier, and tagged
    /// with the authored `boss_kind` (one of the FS-4 named bosses — drives the
    /// bespoke ability pool + display name + client sprite). `seed` drives its AI.
    pub fn dungeon_boss(
        balance: &Balance,
        entity_id: Id,
        biome: &str,
        boss_kind: &str,
        effective_distance: i64,
        seed: u64,
    ) -> Self {
        // Stat base = the biome's toughest listed creature (last), scaled at the
        // synthetic distance so the boss rides the dungeon's difficulty stamp.
        let base_kind = creatures_for_biome(biome).last().copied().unwrap_or("forest_bloom_stalker");
        let pos = Position::new(effective_distance.max(0) as f64, 0.0);
        let mut m = Self::build(balance, entity_id, base_kind, pos, seed);
        let e = &balance.encounters;
        m.promote(e.gatekeeper_hp_mult, e.gatekeeper_atk_mult, e.gatekeeper_xp_mult, "gatekeeper");
        m.become_boss(boss_kind);
        m
    }

    /// Build a **bounty mark** (`AD-4`): the named boss a generated contract names,
    /// standing at `position` for `owner` alone.
    ///
    /// Promoted by the contract's own `power` rather than the Gatekeeper constants, so a
    /// deep-rank mark is worse than the door it walked past, and always affixed — a mark
    /// is the most specific fight in the game, not a bigger version of a common one.
    pub fn bounty_mark(
        balance: &Balance,
        entity_id: Id,
        spec: &meld_proto::bounties::BountySpec,
        bounty_id: &str,
        owner: &str,
        position: Position,
        seed: u64,
    ) -> Self {
        let mut m = Self::build(balance, entity_id, &spec.creature, position, seed);
        let xp = balance.encounters.gatekeeper_xp_mult;
        m.promote(spec.power, spec.power, xp, "gatekeeper");
        m.become_boss(&spec.boss_kind);
        m.apply_affix(seed ^ 0xB0_1177);
        m.owner = owner.to_string();
        m.bounty = bounty_id.to_string();
        // A mark waits where it was sighted rather than roaming off it: a contract that
        // wanders out of its own reported position cannot be tracked to one.
        m.aggression = "territorial".to_string();
        m
    }

    /// A bounty mark built for a **stamped distance** rather than a world position — the
    /// dungeon case, where the boss is assembled for the fight instead of standing in the
    /// arena. Same promotion as [`Self::bounty_mark`], so both venues fight the same thing.
    pub fn bounty_mark_at(
        balance: &Balance,
        entity_id: Id,
        spec: &meld_proto::bounties::BountySpec,
        bounty_id: &str,
        owner: &str,
        effective_distance: i64,
        seed: u64,
    ) -> Self {
        let pos = Position::new(effective_distance.max(0) as f64, 0.0);
        Self::bounty_mark(balance, entity_id, spec, bounty_id, owner, pos, seed)
    }

    /// Promote a fresh standard spawn to an Elite champion or a Gatekeeper boss
    /// (FS-4): scale its HP/atk/XP and tag the encounter class — which drives the
    /// loot multiplier on the kill, the battle merge cap, and the client's size +
    /// tint. Call once, on a standard spawn.
    /// Give this spawn AUTHORED stats rather than a multiple of whatever it rode in on.
    ///
    /// A set piece is not a promoted local spawn. At d3200 an ordinary creature is already
    /// ~10k HP and two-shots a hero, because the deep curve is tuned for a party that
    /// departed from a deep hub — and departure hubs are held off (`PG-2`). Expressing the
    /// end fight as `x4 of local` therefore made it ~14x harder than anything a party can
    /// actually bring, so it is authored instead: absolute numbers, tuned against the party
    /// that can really arrive, and raised when hubs land.
    fn set_piece(&mut self, hp: i32, atk: i32, xp: i64, class: &str) {
        self.max_hp = hp.max(1);
        self.hp = self.max_hp;
        self.atk = atk.max(1);
        self.xp_reward = xp.max(0);
        self.encounter_class = class.to_string();
    }

    fn promote(&mut self, hp_mult: f64, atk_mult: f64, xp_mult: f64, class: &str) {
        self.max_hp = ((self.max_hp as f64) * hp_mult).round().max(1.0) as i32;
        self.hp = self.max_hp;
        self.atk = ((self.atk as f64) * atk_mult).round().max(1.0) as i32;
        self.xp_reward = ((self.xp_reward as f64) * xp_mult).round().max(0.0) as i64;
        self.encounter_class = class.to_string();
    }

    /// Size this encounter for `parties` full parties, and let it say so.
    ///
    /// HP and XP ride the count while ATK does NOT: a raid boss is a longer fight for more
    /// people, not one that hits each of them harder — scaling attack too would make the
    /// same blow that a full raid shrugs off a one-shot for anybody who arrives early.
    fn scale_to_warband(&mut self, parties: u8) {
        let w = meld_proto::warbands::warband(parties);
        self.expects_parties = w.parties;
        if !meld_proto::warbands::is_raid(w.parties) {
            return;
        }
        let n = w.parties as f64;
        self.max_hp = ((self.max_hp as f64) * n).round().max(1.0) as i32;
        self.hp = self.max_hp;
        self.xp_reward = ((self.xp_reward as f64) * n).round().max(0.0) as i64;
    }

    /// Give this spawn a named boss identity, and with it the boss's OWN lineage
    /// (FS-4 + PG-2). A boss used to inherit whatever creature it rode in on, so a
    /// Choirmother fought as a beast; the dead are undead and the made are constructs
    /// regardless of host.
    fn become_boss(&mut self, boss_kind: &str) {
        self.boss_kind = boss_kind.to_string();
        if let Some(faction) = abilities::boss_faction(boss_kind) {
            self.faction = faction.to_string();
        }
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

/// How many parties a gatekeeper is sized for.
///
/// Weighted toward the shallow end of the ladder rather than uniform: two parties is a
/// plausible thing to gather, four is the cap the merge rule allows, and a flat roll would
/// make the hardest tier as common as the easiest.
fn roll_warband(enc: &meld_balance::Encounters, seed: u64) -> u8 {
    let mut rng = Rng(seed);
    if rng.unit() >= enc.gatekeeper_raid_chance {
        return 1;
    }
    let cap = enc.gatekeeper_raid_max_parties.clamp(2, meld_proto::warbands::max_parties());
    // Halving weights: 2 parties twice as likely as 3, which is twice as likely as 4.
    let mut parties = 2u8;
    while parties < cap && rng.unit() < 0.5 {
        parties += 1;
    }
    parties
}

/// Roll one bounty contract for a hunter of `rank` (`AD-4`).
///
/// Pure: balance, rank and a seed in, a spec out — so the Den's offers are unit-testable
/// and a stored contract is never re-derived (a retune changes the *next* one).
///
/// Everything about the mark is drawn from the band it is sighted in, which is what keeps
/// a rank-0 contract a shallow-forest fight and a rank-20 one an apocalypse in the mire.
pub fn roll_bounty(
    balance: &Balance,
    rank: i32,
    seed: u64,
) -> meld_proto::bounties::BountySpec {
    use meld_proto::bounties::{BountySpec, Venue, EPITHETS};
    let b = &balance.bounty;
    let mut rng = Rng(seed | 1);
    let jitter = 1.0 + rng.signed() * b.sighting_jitter;
    let distance = ((b.sighting(rank) as f64) * jitter).round().max(1.0) as i32;
    let biome = biome_for_distance(distance as i64);
    let pool = creatures_for_biome(biome);
    let creature = pool[rng.below(pool.len())].to_string();
    let venue = if rng.unit() < b.dungeon_chance {
        Venue::Dungeon
    } else {
        Venue::Overworld
    };
    BountySpec {
        boss_kind: pick_gatekeeper_boss_kind(distance as i64, rng.next_u64()).to_string(),
        epithet: EPITHETS[rng.below(EPITHETS.len())].to_string(),
        creature,
        biome: biome.to_string(),
        distance,
        venue,
        rank,
        power: b.power(rank),
        reward_chits: b.reward_chits(rank),
        reward_material: combat_material_for_biome(distance as i64).to_string(),
        reward_material_qty: b.reward_qty(rank),
        reward_gear: rank >= b.reward_gear_from_rank,
        reward_rank_xp: b.rank_xp(rank),
    }
}

/// Which of the 10 named bosses (FS-4) an Elite champion fights as — the
/// "elite" tier of `BOSS_KEYS` (`client::world_render`), reserved for the
/// far-more-common Elite roll rather than the rarer Gatekeeper.
fn pick_elite_boss_kind(seed: u64) -> &'static str {
    let mut rng = Rng(seed);
    ["gloamhound", "rustfang"][rng.below(2)]
}

/// Which of the 10 named bosses (FS-4) a Gatekeeper fights as, tiered by the
/// distance of the seam/summit it guards — the same escalation the world
/// already uses for difficulty, so early gates feel like a "miniboss" and the
/// deep ones read as apocalyptic. Buckets align with the seam thresholds
/// `push_section` already walls (100/300/500/1000/3000).
fn pick_gatekeeper_boss_kind(distance: i64, seed: u64) -> &'static str {
    let mut rng = Rng(seed);
    let tier: [&str; 2] = if distance < 300 {
        ["choirmother", "pyrewarden"] // miniboss
    } else if distance < 500 {
        ["sepulcher", "hollowbishop"] // dungeon
    } else if distance < 1000 {
        ["ironmaw", "weepingcolossus"] // region
    } else {
        ["miredrowned", "ashenleviathan"] // biome
    };
    tier[rng.below(2)]
}

/// The in-battle title for a boss id (FS-4), shown in place of the plain
/// biome-creature name when a fighter carries a `boss_kind`.
///
/// The names live in [`meld_proto::bosses`] rather than here, because the CLIENT needs
/// them too — a boss on the overworld wears its title on a name plate (`FS-4`), and a
/// second copy of ten names on the far side of the wire is a copy that goes stale.
pub fn boss_display_name(key: &str) -> &'static str {
    meld_proto::bosses::display_name(key).unwrap_or("Unknown Horror")
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

/// A live creature-vs-creature **clash** (`CR-2`): the creatures currently tearing
/// at each other in one place. Derived from the blows that actually landed rather
/// than from proximity — two hostiles standing near each other are not fighting, and
/// a marker that said otherwise would cry wolf on every crowded ring.
///
/// It is a fact about the ENCOUNTER, not about a creature, which is why it lives here
/// and not on `MonsterSpawn`: the same creature belongs to a different clash depending
/// on who it ended up swinging at, and it belongs to none the moment the blows stop.
/// (The same reasoning that derives a battle GROUP at assembly rather than carrying it
/// through the world.)
#[derive(Debug, Clone)]
pub struct Clash {
    /// Every creature swinging in it, both sides. Entity ids, not indices, so
    /// `prune_defeated` can compact `monsters` without corrupting a clash.
    pub members: Vec<Id>,
    /// Where it is happening (the mean of its members' positions), for the range
    /// checks a watcher's feed does.
    pub position: Position,
    /// Seconds since the last blow landed in it. A clash is a CADENCE of blows, so
    /// this is what keeps it alive between swings instead of strobing once a second.
    pub quiet: f64,
}

impl Clash {
    /// The creature whose id sorts first — a stable anchor for a watcher's
    /// subscription, so a clash that gains or loses a member does not silently
    /// become a different clash and drop the feed.
    pub fn anchor(&self) -> Option<&Id> {
        self.members.iter().min()
    }
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
    /// Units still in the node (MS-2). A harvest CHANNEL takes these one at a time,
    /// so a node is a quantity you can partly work and come back to rather than a
    /// one-tap flag — which is what lets an interrupted gather cost only the tick in
    /// flight. Stock and pace come from the material's class (`[harvest]`).
    pub remaining: i32,
    /// World tick the node was emptied, or 0 while it still has stock. A persistent
    /// world re-stocks it (`[world_persist] node_regrow_ticks`).
    pub spent_tick: u64,
}

impl ResourceNode {
    /// Nothing left to take. Kept as a method so the old `harvested` reads still
    /// make sense at call sites (the snapshot hides an empty node).
    pub fn depleted(&self) -> bool {
        self.remaining <= 0
    }
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
    /// The Shift generation that last retiled this section, or 0 for ground the
    /// Shifting Lands have not yet come for (CANON D20/§W2). Half of every Shift
    /// picks the least-recently-disturbed region, so a section that has never gone
    /// is the one most likely to go next — otherwise the churn pools in one place
    /// and the rest of the world is a museum.
    pub shifted_at: u64,
}

/// A creature's vacated ground: everything [`Arena::regrow`] needs to stand a fresh
/// one of the same species back up where the last one died.
#[derive(Debug, Clone)]
pub struct Fallen {
    pub entity_id: Id,
    pub monster_kind: String,
    pub home: Position,
    pub area_min_x: f64,
    pub area_max_x: f64,
    /// World tick it fell, stamped by `regrow` on the first pass that sees it (the
    /// arena is pure and never asks what time it is).
    pub felled_tick: u64,
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
    /// World tick it was opened, or 0 while it is still sealed. Treasure is the one
    /// thing farming must not print, so its regrowth is by far the slowest of the
    /// three (`[world_persist] chest_regrow_ticks`).
    pub opened_tick: u64,
}

/// A field workstation a player has raised in the maze (MS-1). A smith who carries
/// ore can put one down and then ANYONE standing at it can have a piece worked — the
/// station's owner is the skill doing the job, which is what makes a stacked
/// profession party worth forming. Finite `uses_left`, so the city anvil stays the
/// cheaper place to work in bulk.
#[derive(Debug, Clone)]
pub struct Station {
    pub entity_id: Id,
    /// What kind of bench it is (`smith` today; the Alembic's field twin is next).
    pub kind: String,
    pub position: Position,
    pub elevation: u8,
    /// Who raised it. Their Meld skill is the one the work is done at, and they take
    /// the XP for it — a station is a service its owner provides, not a free anvil.
    pub owner_player_id: Id,
    pub uses_left: i32,
    /// The material it was built from, so packing it up hands back the same stock rather
    /// than something the world had to guess at.
    pub stock: String,
}

impl Station {
    pub fn spent(&self) -> bool {
        self.uses_left <= 0
    }
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

/// Distance from `p` to the nearest edge of the trail web (∞ if the web is empty).
fn dist_to_web(p: &Position, web: &[(Position, Position)]) -> f64 {
    web.iter()
        .map(|(a, b)| dist_point_segment(p, a, b))
        .fold(f64::INFINITY, f64::min)
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
    /// Creature-vs-creature fights happening right now (`CR-2`). Rebuilt from the
    /// blows each `step_creatures` pass actually lands, so it is a report of what
    /// happened rather than a guess from positions.
    pub clashes: Vec<Clash>,
    /// Impassable biome terrain (trees/cliffs/water/…). Never intrudes on `path`.
    pub obstacles: Vec<Obstacle>,
    /// Hand-placed treasure chests scattered through the sections.
    pub chests: Vec<Chest>,
    /// Player-raised field workstations (MS-1). Empty until someone builds one. Predates
    /// the `Structure` primitive below and still runs its own lifecycle; folding it in is
    /// `BD-6`, the roadmap item that owns field crafting.
    pub stations: Vec<Station>,
    /// Player-built structures — the ONE primitive (CANON D21/§W3), whatever their
    /// `function`. Anchors in here are what hold ground against the Shift.
    pub structures: Vec<Structure>,
    /// Monotonic source of structure ids, so an id is never reused after a demolish (a
    /// reused id is a client rendering the new thing with the old one's HP bar).
    next_structure: u64,
    /// Ground a creature used to hold. `prune_defeated` moves a slain creature here
    /// instead of deleting it, so `regrow` can put something back where it stood —
    /// a few fields per kill rather than a corpse the snapshot and the AI both walk
    /// every tick. This is the half of persistence that keeps a world from being
    /// strip-minable: without it, "the world remembers" only ever means "the world
    /// is emptier than you left it".
    pub fallen: Vec<Fallen>,
    /// Biome-boundary chokepoints (a walled seam with one gap you pass through).
    pub seams: Vec<Seam>,
    /// The guaranteed-clear route from the hub to the portal, as waypoints. A tube
    /// of `path_clear_radius` around it holds no obstacles AND no raised terrace, so
    /// the exit is always reachable on level 0; the client draws it as a faint trail.
    pub path: Vec<Position>,
    /// The WEB of extra trails woven through each section — cross-links, loops and
    /// dead-end spurs (with loot) branching off the backbone, each an edge `(a, b)` in
    /// the SAME frame as `path` (bent in radial mode). The clear tube is carved around
    /// these too, so the overworld reads as an interconnected maze of trails with real
    /// junctions and choices, not one lane. Feasibility still rides `path`; the web is
    /// extra. Client draws them as trail dots like the backbone.
    pub web: Vec<(Position, Position)>,
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
    /// The web of trails in **unbent corridor** space (mirrors `corridor_path`).
    /// Generation clears obstacles/terraces around these; `web` is the bent public copy.
    corridor_web: Vec<(Position, Position)>,
    /// The avatar's collision radius against obstacles.
    player_radius: f64,
    /// How much room a blocking structure takes up. Structure rather than balance: it is
    /// the footprint the client draws, not a knob anyone tunes for feel.
    structure_radius: f64,
    touch_radius: f64,
    interaction_radius: f64,
    sim_dt: f64,
    // World-gen tunables (snapshot from balance) needed for streaming.
    seed_base: u64,
    /// Tutorial run (the account's first dive): classic distance-ordered biomes +
    /// the centred area-0 onboarding. Otherwise biomes are drawn per-section
    /// (roadmap WG-2/WG-3) and area 0 is a normal procedural section.
    tutorial: bool,
    /// THE END FIGHT is placed once per instance and never rolled again — it is the thing
    /// the walk out is pointed at, not a spawn type that can recur.
    end_fight_placed: bool,
    /// DEV/QA harness override: when set to a `BIOMES` name, EVERY section is forced to
    /// that biome so a specific biome's maze can be loaded + inspected on demand instead
    /// of waiting for a random draw to surface it. `None` in normal play. Set at the
    /// server boundary from `MELD_BIOME` (see `generate_with`); the engine stays pure.
    force_biome: Option<&'static str>,
    /// Per-run world-space offset into the shared height field (`terrain::seed_offset`),
    /// so each seed grows DIFFERENT hills/mesas at the hub and the clear path bends a
    /// different way — no more "same corridor every run". Every terrain sample in this
    /// arena passes it; the client gets it on `run.started` and samples the same field.
    terrain_off: (f32, f32),
    /// Authored CLIMBABLE landmark peaks (mountains), WORLD-space `[cx, cz, radius,
    /// height]` — smooth walkable domes summed onto the terrain (`terrain::peak_height`)
    /// with a boss/treasure on the summit. Bent into the fan by `radialize` like the rest
    /// of the content; sent to the client so each mountain renders + is climbable.
    pub peaks: Vec<[f32; 4]>,
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
    wander_leg_seconds: f64,
    wander_arrive_radius: f64,
    wander_pause_chance: f64,
    wander_pause_seconds: f64,
    chase_speed: f64,
    aggro_radius: f64,
    territorial_aggro_radius: f64,
    leash_radius: f64,
    group_radius: f64,
    skirmish_aggro: f64,
    skirmish_range: f64,
    skirmish_interval: f64,
    clash_linger: f64,
    creature_regen: f64,
    loot_pickup_radius: f64,
    /// Every placed creature's position **in the bent (world) frame**, bucketed into a
    /// coarse grid so placement can refuse to drop a standard spawn inside another
    /// creature's pull radius in O(1) rather than scanning thousands of monsters (the
    /// world streams outward without bound, so a linear scan is quadratic in dive depth).
    /// Its own store rather than a read of `monsters`, because `push_section` places in
    /// the corridor frame and cannot tell whether the monsters already in the arena were
    /// bent by `radialize` (one-shot generate) or not yet (streaming) — so it records the
    /// bent position itself, at placement, and the frame is never in question. Spans
    /// sections, so a spawn at a seam is separated from the one in the section next door.
    creature_spots: SpotGrid,
}

/// How many lateral positions a creature station tries before it is abandoned. Structure,
/// not balance: it trades a little generation work for how completely the placement fills.
const CREATURE_PLACEMENT_TRIES: u32 = 6;

/// Spacing index for one section's maze fill: *does anything already stand within our
/// combined radii of here?* Answers in the **stretched** metric, because corridor y is an
/// angle and a tangential gap is worth `stretch` times what it measures. Seeded only from
/// content inside the section's own x-range, since everything outside it may already have
/// been bent into the fan and its coordinates would mean something else entirely.
#[derive(Debug)]
struct BlockGrid {
    cell: f64,
    stretch: f64,
    x_lo: f64,
    x_hi: f64,
    items: HashMap<(i32, i32), Vec<(Position, f64)>>,
}

impl BlockGrid {
    fn new(cell: f64, stretch: f64, start_x: f64, end_x: f64) -> Self {
        Self {
            cell: cell.max(0.001),
            stretch: stretch.max(1.0),
            x_lo: start_x - cell,
            x_hi: end_x + cell,
            items: HashMap::new(),
        }
    }

    fn key(&self, p: &Position) -> (i32, i32) {
        ((p.x / self.cell).floor() as i32, (p.y * self.stretch / self.cell).floor() as i32)
    }

    fn seed(
        &mut self,
        monsters: &[MonsterSpawn],
        resources: &[ResourceNode],
        chests: &[Chest],
        obstacles: &[Obstacle],
    ) {
        for p in monsters.iter().map(|m| m.position) {
            self.insert(p, 1.2);
        }
        for p in resources.iter().map(|r| r.position) {
            self.insert(p, 1.2);
        }
        for p in chests.iter().map(|c| c.position) {
            self.insert(p, 1.2);
        }
        for o in obstacles {
            self.insert(o.position, o.radius);
        }
    }

    fn blocked(&self, p: &Position, r: f64) -> bool {
        let (kx, ky) = self.key(p);
        (-1..=1).any(|dx| {
            (-1..=1).any(|dy| {
                self.items.get(&(kx + dx, ky + dy)).is_some_and(|v| {
                    v.iter().any(|(q, qr)| {
                        (q.x - p.x).hypot((q.y - p.y) * self.stretch) < r + *qr
                    })
                })
            })
        })
    }

    fn insert(&mut self, p: Position, r: f64) {
        if p.x < self.x_lo || p.x > self.x_hi {
            return;
        }
        self.items.entry(self.key(&p)).or_default().push((p, r));
    }
}

/// A coarse uniform grid of world positions with one question: *is anything within `r` of
/// here?* Cells are `r` on a side, so the answer is always inside the 3×3 block around the
/// query — no scan of the whole world.
#[derive(Debug, Clone)]
struct SpotGrid {
    radius: f64,
    spots: HashMap<(i32, i32), Vec<Position>>,
}

impl SpotGrid {
    fn new(radius: f64) -> Self {
        Self { radius: radius.max(0.001), spots: HashMap::new() }
    }

    fn key(&self, p: &Position) -> (i32, i32) {
        ((p.x / self.radius).floor() as i32, (p.y / self.radius).floor() as i32)
    }

    fn crowded(&self, p: &Position) -> bool {
        let (kx, ky) = self.key(p);
        (-1..=1).any(|dx| {
            (-1..=1).any(|dy| {
                self.spots
                    .get(&(kx + dx, ky + dy))
                    .is_some_and(|v| v.iter().any(|q| q.distance_to(p) <= self.radius))
            })
        })
    }

    fn insert(&mut self, p: Position) {
        self.spots.entry(self.key(&p)).or_default().push(p);
    }
}

impl Arena {
    /// Generate a fresh world from `seed`. Deterministic: same seed ⇒ same areas,
    /// creatures, terraces, and portals (world-generation.md determinism invariant).
    /// Eagerly builds the initial `area_count`-section chain (so the deep portal +
    /// clear path are known at run start); further sections stream on demand via
    /// [`Arena::ensure_frontier`].
    pub fn generate(balance: &Balance, seed: u64, tutorial: bool) -> Self {
        Self::generate_with(balance, seed, tutorial, None)
    }

    /// This run's terrain offset into the shared height field (sent to the client on
    /// `run.started` so it renders the identical hills/mesas). See `terrain::seed_offset`.
    pub fn terrain_offset(&self) -> (f32, f32) {
        self.terrain_off
    }

    /// Total terrain height (base field + authored peak domes) at world `(x, z)`. The prod
    /// server never needs total height (peaks are visual + the client grounds the summit
    /// reward on them; the 2D sim ignores them) — this is the reference the peak test
    /// checks against, hence `#[cfg(test)]`.
    #[cfg(test)]
    fn t_height(&self, x: f64, z: f64) -> f64 {
        let base = meld_proto::terrain::height(x as f32, z as f32, self.terrain_off.0, self.terrain_off.1);
        (base + meld_proto::terrain::peak_height(x as f32, z as f32, &self.peaks)) as f64
    }
    fn t_walkable(&self, x: f64, z: f64) -> bool {
        meld_proto::terrain::walkable(x as f32, z as f32, self.terrain_off.0, self.terrain_off.1)
    }

    /// Like [`Self::generate`], but with a DEV/QA `force_biome` override (from the
    /// server's `MELD_BIOME` env) that pins every section to one biome so its maze can be
    /// loaded + screenshotted directly. `None` reproduces normal generation exactly.
    ///
    /// Seeds the per-run terrain (so each run's hills differ) and RE-ROLLS the offset if
    /// the resulting initial-chain backbone isn't cleanly walkable end-to-end — a mesa can
    /// occasionally land on a seam gap / dungeon door and pinch the route. Re-rolling
    /// (rather than patching every placement) keeps the "every run is feasible by
    /// construction" guarantee while the terrain still varies; the last resort is the
    /// hand-tuned un-shifted field.
    pub fn generate_with(
        balance: &Balance,
        seed: u64,
        tutorial: bool,
        force_biome: Option<&'static str>,
    ) -> Self {
        let mut off = hub_terrain_offset(seed);
        for attempt in 0..12u64 {
            let mut arena = Self::build_with(balance, seed, tutorial, force_biome, off);
            if arena.backbone_feasible() {
                return arena;
            }
            off = hub_terrain_offset(seed ^ (attempt + 1).wrapping_mul(0x2545_F491_4F6C_DD1D));
        }
        // Nothing clean found (extremely rare): the un-shifted hand-tuned field is known
        // feasible, so fall back to it rather than ship a pinched world.
        Self::build_with(balance, seed, tutorial, force_biome, (0.0, 0.0))
    }

    /// Can a walker actually follow the initial-chain clear path from the hub to the deep
    /// portal? Simulates it exactly like the conformance walker test — so it catches a
    /// seeded mesa pinching a seam gap / dungeon door / bend that the A* backbone (which
    /// only guarantees ITS own edges) can't see. The re-roll gate that keeps every run
    /// feasible by construction while the terrain varies. Uses + removes a throwaway
    /// probe avatar; `apply_move` is pure movement (no touch/battle side effects), so the
    /// arena is left pristine.
    fn backbone_feasible(&mut self) -> bool {
        if self.path.len() < 2 {
            return false;
        }
        let waypoints = self.path.clone();
        let portal = self.portal;
        let probe = "__feasibility_probe__".to_string();
        self.add_avatar(probe.clone(), 6.0);
        let (mut wp, mut reached) = (1usize, false);
        for _ in 0..100_000 {
            let pos = match self.avatar(&probe) {
                Some(a) => a.position,
                None => break,
            };
            let target = waypoints[wp];
            if pos.distance_to(&target) < 0.6 {
                if wp + 1 >= waypoints.len() {
                    reached = pos.distance_to(&portal) < 2.0;
                    break;
                }
                wp += 1;
                continue;
            }
            self.apply_move(&probe, target.x - pos.x, target.y - pos.y, 0);
        }
        self.avatars.retain(|a| a.player_id != probe);
        reached
    }

    fn build_with(
        balance: &Balance,
        seed: u64,
        tutorial: bool,
        force_biome: Option<&'static str>,
        terrain_off: (f32, f32),
    ) -> Self {
        let wg = &balance.worldgen;
        let mut arena = Arena {
            fallen: Vec::new(),
            structures: Vec::new(),
            next_structure: 0,
            end_fight_placed: false,
            seed,
            areas: Vec::new(),
            monsters: Vec::new(),
            resources: Vec::new(),
            ground_loot: Vec::new(),
            clashes: Vec::new(),
            obstacles: Vec::new(),
            chests: Vec::new(),
            stations: Vec::new(),
            seams: Vec::new(),
            path: vec![Position::new(0.0, 0.0)],
            web: Vec::new(),
            corridor_web: Vec::new(),
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
            structure_radius: 2.2,
            touch_radius: balance.world.touch_radius_tiles,
            interaction_radius: balance.world.interaction_radius_tiles,
            sim_dt: 1.0 / balance.world.overworld_sim_hz as f64,
            seed_base: seed,
            tutorial,
            force_biome,
            terrain_off,
            peaks: Vec::new(),
            terrain_cell: wg.terrain_cell,
            terraces_per_area: wg.terraces_per_area,
            max_level: wg.max_level,
            terrace_min_size: wg.terrace_min_size,
            terrace_max_size: wg.terrace_max_size,
            connector_radius: wg.connector_radius,
            path_clear_radius: wg.path_clear_radius,
            world_margin: wg.world_margin,
            wander_speed: balance.ai.wander_speed,
            wander_leg_seconds: balance.ai.wander_leg_seconds,
            wander_arrive_radius: balance.ai.wander_arrive_radius,
            wander_pause_chance: balance.ai.wander_pause_chance,
            wander_pause_seconds: balance.ai.wander_pause_seconds,
            chase_speed: balance.ai.chase_speed,
            aggro_radius: balance.ai.aggro_radius,
            territorial_aggro_radius: balance.ai.territorial_aggro_radius,
            leash_radius: balance.ai.leash_radius,
            group_radius: balance.ai.group_radius,
            skirmish_aggro: balance.ai.skirmish_aggro_radius,
            skirmish_range: balance.ai.skirmish_attack_range,
            skirmish_interval: balance.ai.skirmish_attack_interval,
            clash_linger: balance.ai.clash_linger_seconds,
            creature_regen: balance.ai.creature_regen_fraction_per_sec,
            loot_pickup_radius: balance.ai.loot_pickup_radius,
            creature_spots: SpotGrid::new(balance.ai.group_radius + balance.encounters.pack_spread),
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
        let toff = self.terrain_off;
        let tf = |p: Position| -> Position {
            let r = p.x.max(0.0);
            let theta = (p.y / lat).clamp(-1.0, 1.0) * half;
            Position::new(r * theta.cos(), r * theta.sin())
        };
        for m in &mut self.monsters {
            m.position = tf(m.position);
            m.home = tf(m.home);
            // Keep the corridor [start_x, end_x] as a RADIUS band: after the bend a
            // creature's hub-distance is its corridor x, so `step_creatures` clamps its
            // radius to this band and it never wanders out of its biome ring (#10).
        }
        // Chests + harvest nodes are scattered without a walkability check, so the bend
        // can drop one onto a heightmap CLIFF — an unreachable reward. Nudge each onto
        // the nearest walkable ground so everything the player is meant to collect stays
        // reachable. (Summit chests already sit on the walkable route, so it's a no-op
        // for them.) Obstacles are left on cliffs — impassable scenery is fine there.
        for r in &mut self.resources {
            r.position = nudge_to_walkable(tf(r.position), toff);
        }
        for o in &mut self.obstacles {
            o.position = tf(o.position);
        }
        for c in &mut self.chests {
            c.position = nudge_to_walkable(tf(c.position), toff);
        }
        // Bend each authored peak's CENTRE into the fan (radius/height are world-space
        // scalars — the dome is a world circle at the bent centre, matching its summit
        // reward, which is bent by the same `tf`). A summit chest sits on the gentle base
        // at the centre, so `nudge_to_walkable` above is a no-op and keeps it on the peak.
        for p in &mut self.peaks {
            let c = tf(Position::new(p[0] as f64, p[1] as f64));
            p[0] = c.x as f32;
            p[1] = c.y as f32;
        }
        // Bend the path with DENSIFICATION: the meander swings the corridor path across
        // the fan, so a straight world chord between two far-apart-in-bearing waypoints
        // would cut deep across the arc — off the cleared tube and into off-path
        // terraces (breaking a path-follower). Inserting collinear intermediates makes
        // the bent trail hug the arc so a follower stays inside the corridor tube.
        let corridor_path = std::mem::take(&mut self.path);
        if let Some(&first) = corridor_path.first() {
            self.path.push(radial_tf(first, half, lat));
            for w in corridor_path.windows(2) {
                push_bent_segment(&mut self.path, w[0], w[1], half, lat);
            }
        }
        self.portal = tf(self.portal);
        // Bend the web edges into the fan too (endpoints only — each edge is short
        // enough that its chord hugs the arc). This is the public `web` sent to clients.
        self.web = self
            .corridor_web
            .iter()
            .map(|(a, b)| (radial_tf(*a, half, lat), radial_tf(*b, half, lat)))
            .collect();
        // Heightmap cliffs: the backbone path is already routed around buttes by A*
        // (`astar_route`, in the corridor frame), so it needs no repair here. Just nudge
        // the web trails + portal onto walkable ground (the web isn't A*-routed), and
        // re-anchor the portal to the routed path's walkable end.
        for (a, b) in self.web.iter_mut() {
            *a = nudge_to_walkable(*a, toff);
            *b = nudge_to_walkable(*b, toff);
        }
        self.portal = nudge_to_walkable(self.portal, toff);
        if let Some(last) = self.path.last_mut() {
            *last = self.portal;
        }
        // The non-linear bend distorts the carefully-carved clear tube, so an obstacle
        // can end up on the backbone OR a web trail. Re-clear both (in bent coords) so
        // every route stays feasible by construction, as in the corridor.
        let clear_r = self.path_clear_radius;
        let web_r = self.web_clear();
        let path = self.path.clone();
        let web = self.web.clone();
        self.obstacles.retain(|o| {
            dist_to_path(&o.position, &path) > clear_r + o.radius
                && dist_to_web(&o.position, &web) > web_r + o.radius
        });
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
        let toff = self.terrain_off;
        // Snapshot the tails so we can bend exactly what this section appends.
        let (m0, r0, o0, c0, s0) = (
            self.monsters.len(),
            self.resources.len(),
            self.obstacles.len(),
            self.chests.len(),
            self.seams.len(),
        );
        let p0 = self.path.len();
        let w0 = self.corridor_web.len();
        let pk0 = self.peaks.len();

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
            // Keep [start_x, end_x] as a radius band (see `radialize`) so streamed
            // creatures also stay inside their own biome ring (#10).
        }
        // Keep streamed chests + harvest nodes off cliffs too (see `radialize`).
        for r in &mut self.resources[r0..] {
            r.position = nudge_to_walkable(tf(r.position), toff);
        }
        for o in &mut self.obstacles[o0..] {
            o.position = tf(o.position);
        }
        for c in &mut self.chests[c0..] {
            c.position = nudge_to_walkable(tf(c.position), toff);
        }
        // Bend this streamed section's new peak centres into the fan (see `radialize`).
        for k in pk0..self.peaks.len() {
            let c = tf(Position::new(self.peaks[k][0] as f64, self.peaks[k][1] as f64));
            self.peaks[k][0] = c.x as f32;
            self.peaks[k][1] = c.y as f32;
        }
        // Append this section's new corridor waypoint(s) to the bent public path,
        // densified (see `radialize`) so the streamed trail hugs the arc and stays
        // walkable across the terraced fan.
        let repair_from = self.path.len();
        let mut prev = self.corridor_path[p0.saturating_sub(1)];
        for k in p0..self.corridor_path.len() {
            push_bent_segment(&mut self.path, prev, self.corridor_path[k], half, lat);
            prev = self.corridor_path[k];
        }
        // (The streamed tail is already A*-routed around cliffs in `push_section`.)
        let _ = repair_from;
        // Bend this section's new web edges and append to the public `web`.
        for e in w0..self.corridor_web.len() {
            let (a, b) = self.corridor_web[e];
            self.web.push((nudge_to_walkable(tf(a), toff), nudge_to_walkable(tf(b), toff)));
        }
        // Straight-wall biome seams don't survive the bend — drop the ones just added.
        self.seams.truncate(s0);
        // The bend distorts the clear-path tube AND appending this section's waypoint
        // adds a new path segment near the previous frontier — either can pull an
        // already-placed obstacle into the tube. Re-clear ALL obstacles against the
        // full bent path, exactly as the one-shot `radialize` does, so a feasible route
        // outward stays guaranteed by construction across the whole streamed world.
        let clear_r = self.path_clear_radius;
        let web_r = self.web_clear();
        let path = self.path.clone();
        let web = self.web.clone();
        self.obstacles.retain(|o| {
            dist_to_path(&o.position, &path) > clear_r + o.radius
                && dist_to_web(&o.position, &web) > web_r + o.radius
        });
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
        let biome = self.force_biome.unwrap_or_else(|| {
            section_biome(balance, self.seed_base, i, start_x.floor() as i64, prev_biome, self.tutorial)
        });
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
            let starter_kind = resources_for_biome(biome)[0].to_string();
            self.resources.push(ResourceNode {
                spent_tick: 0,
                entity_id: format!("res-{}", self.resources.len()),
                remaining: node_stock(balance, &starter_kind),
                kind: starter_kind,
                position: Position::new(wg.first_monster_x * 0.5, 3.0),
                elevation: 0,
            });
            // A guaranteed starter treasure chest opposite the node, so a new
            // player sees the loot loop (open → chits/materials) in area 0.
            let starter_chest_x = wg.first_monster_x * 0.5;
            self.chests.push(Chest {
                opened_tick: 0,
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
                shifted_at: 0,
            });
            // The tutorial path routes to y=0, around any cliffs (A*, like the procedural
            // sections) so the very first stretch is walkable too.
            let entry = *self.path.last().unwrap_or(&Position::new(0.0, 0.0));
            for p in self.astar_route(entry, Position::new(end_x, 0.0)) {
                self.path.push(p);
            }
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
        // FS-4: a fraction of creatures roll ELITE (champions). A SEPARATE rng stream
        // so the main placement draws stay byte-identical (determinism tests hold).
        // Never in the spawn section (i == 0), which stays gentle onboarding.
        let enc = &balance.encounters;
        let mut erng = Rng(section_seed(self.seed_base, i) ^ 0xE117_E117_E117_E117);
        // Radial density compensation for CREATURES. `monster_spacing` lays one creature
        // per gap across ONE corridor's worth of width — but the WG-4 fan bends that
        // fixed width into an arc that grows with radius, so a deep section is an annular
        // sector hundreds of units of arc across holding the same handful of creatures.
        // You can cross it and meet nothing. (The maze fill compensates for exactly this;
        // creature placement did not.) So walk the corridor once per corridor-width of
        // arc: density-per-tile holds instead of thinning outward. Capped, because the
        // multiplier passes 30× in a world that streams forever. Lane 0 draws from the
        // section's main stream in the original order, so every placement/determinism
        // test still holds byte-identically; each extra lane draws from its OWN stream.
        let r_mid = (start_x + end_x) * 0.5;
        let arc_stretch = if self.radial_half > 0.0 {
            (r_mid * self.radial_half / self.lateral.max(1.0)).max(1.0)
        } else {
            1.0
        };
        let lanes = arc_stretch
            .min(wg.creature_radial_lane_cap.max(1.0))
            .round()
            .max(1.0) as u64;
        // `group_around` pulls every creature within `[ai] group_radius` into whatever
        // fight you start, so at the designed density an unrelated neighbour silently
        // joins — the first-150-tiles band promised duels and started handing out 1.4
        // creatures a fight, sometimes five. A PACK is the only thing that may make a
        // group, so no standard spawn goes down within pull range of another. The margin
        // is `pack_spread` on top: a pack's satellites scatter that far from their leader,
        // and it was one of those, not a leader, that reached into the set-piece next door.
        // Measured in the BENT frame, which is the frame `group_around` measures in: a
        // corridor-space check would over-separate by the whole arc stretch and undo the
        // very density it is protecting.
        let (bend_half, bend_lat) = (self.radial_half, self.corridor_lateral.max(1.0));
        let bend = move |p: Position| -> Position {
            if bend_half <= 0.0 {
                return p;
            }
            let theta = (p.y / bend_lat).clamp(-1.0, 1.0) * bend_half;
            Position::new(p.x.max(0.0) * theta.cos(), p.x.max(0.0) * theta.sin())
        };
        let mut taken = std::mem::replace(&mut self.creature_spots, SpotGrid::new(1.0));
        for lane in 0..lanes {
            let mut lane_rng =
                (lane > 0).then(|| Rng(section_seed(self.seed_base, i) ^ 0x1A4E_5EED ^ (lane << 32)));
            let rng = lane_rng.as_mut().unwrap_or(&mut rng);
            // The SPAWN section (i == 0) keeps a creature-free safe ring around the Center
            // Hub: a stationary player at spawn is otherwise inside `[ai] aggro_radius` of
            // the first creature, so something closes and yanks them into a battle before
            // they can react. `hub_safe_radius` exceeds aggro_radius, and in the radial
            // world a creature's hub-distance is its corridor-x, so starting placement
            // there guarantees a calm spawn. Deeper sections start at their western edge.
            let mut x = start_x + if i == 0 { wg.hub_safe_radius.max(2.0) } else { 2.0 };
            while x < inner_end {
                let kind = kinds[rng.below(kinds.len())];
                // A few tries at a fresh lateral before giving the station up: skipping on
                // the first clash loses the spawn outright, and at this density a clash is
                // common enough that the world would thin back out through the side door.
                let Some((pos, world)) = (0..CREATURE_PLACEMENT_TRIES).find_map(|_| {
                    let p = Position::new(x, wg.creature_lateral_spread * rng.signed());
                    let w = bend(p);
                    (!taken.crowded(&w)).then_some((p, w))
                }) else {
                    let gap = creature_spacing * (1.0 + wg.monster_spacing_jitter * rng.signed());
                    x += gap.max(2.0);
                    continue;
                };
                taken.insert(world);
                let idx = self.monsters.len();
                let mseed = rng.next_u64();
                self.monsters
                    .push(MonsterSpawn::build(balance, format!("mob-{idx}"), kind, pos, mseed));
                self.monsters[idx].area_min_x = start_x;
                self.monsters[idx].area_max_x = end_x;
                // Never shallow: an Elite is a named boss carrying `elite_hp_mult`, so one
                // in the first ring is a wipe rather than an encounter. Gate on the RADIUS
                // (corridor x), which is what the spawn's hub distance becomes once the fan
                // bends it — `distance_floor()` here is corridor hypot(x, y), which reads up
                // to `creature_lateral_spread` too far out and let elites past the on-ramp.
                if i > 0
                    && !self.tutorial
                    && (pos.x.max(0.0).floor() as i64) >= enc.elite_min_distance
                    && erng.unit() < enc.elite_chance
                {
                    self.monsters[idx].promote(
                        enc.elite_hp_mult,
                        enc.elite_atk_mult,
                        enc.elite_xp_mult,
                        "elite",
                    );
                    let bseed = erng.next_u64();
                    self.monsters[idx].apply_affix(bseed);
                    // FS-4: unique boss mechanics — an Elite fights as one of the
                    // "elite" tier's two named bosses instead of a plain reskin.
                    self.monsters[idx].become_boss(pick_elite_boss_kind(bseed ^ 0xB055));
                }

                // THE UNDEAD RITE (PG-2): rarely, and never shallow, a spawn becomes a
                // named UNDEAD boss with a retinue of undead minions — the set-piece that
                // teaches a party it wants a wall. Uses CR-7's pack machinery (leader +
                // minions) so it fights as a unit, and is checked BEFORE the ordinary pack
                // roll so a rite is never demoted into one.
                let leader_idx = idx;
                let mut became_rite = false;
                // THE END FIGHT (EW, first cut): past `end_fight_min_distance` one encounter
                // becomes THREE named bosses standing together — peers, not a boss with a
                // retinue. Guaranteed rather than rolled, and only once per instance,
                // because it is the thing the whole walk out is pointed at; checked before
                // the rite and the pack roll so nothing can demote it.
                if !self.tutorial
                    && !self.end_fight_placed
                    && pos.x >= enc.end_fight_min_distance
                    && self.monsters[leader_idx].encounter_class == "standard"
                {
                    self.end_fight_placed = true;
                    became_rite = true;
                    let all = abilities::all_bosses();
                    for n in 0..enc.end_fight_bosses.max(1) {
                        let bidx = if n == 0 {
                            leader_idx
                        } else {
                            let angle = erng.unit() * std::f64::consts::TAU;
                            let dist = enc.pack_spread * (0.5 + 0.5 * erng.unit());
                            let bpos = corridor_offset(
                                pos,
                                dist * angle.cos(),
                                dist * angle.sin(),
                                self.radial_half,
                                self.corridor_lateral.max(1.0),
                            );
                            let j = self.monsters.len();
                            let bseed = erng.next_u64();
                            self.monsters.push(MonsterSpawn::build(
                                balance,
                                format!("mob-{j}"),
                                kind,
                                bpos,
                                bseed,
                            ));
                            self.monsters[j].area_min_x = start_x;
                            j
                        };
                        self.monsters[bidx].set_piece(
                            enc.end_fight_boss_hp,
                            enc.end_fight_boss_atk,
                            enc.end_fight_boss_xp,
                            "world_end",
                        );
                        // Three DIFFERENT bosses: the same name three times reads as a bug.
                        let boss = all[(erng.below(all.len().max(1)) + n) % all.len().max(1)];
                        self.monsters[bidx].become_boss(boss);
                        // …and three DIFFERENT wards, so no single damage source clears the
                        // encounter. Rotated rather than rolled: the encounter must always
                        // cover all three families, or a seed could hand out a free run.
                        self.monsters[bidx].set_piece_ward =
                            ["mind", "physical", "elemental"][n % 3].to_string();
                    }
                }
                if i > 0
                    && !self.tutorial
                    && tier_at_distance(balance, pos.x) >= enc.undead_rite_min_tier
                    && self.monsters[leader_idx].encounter_class == "standard"
                    && erng.unit() < enc.undead_rite_chance
                {
                    became_rite = true;
                    let undead = abilities::bosses_of_faction("undead");
                    let boss = undead[erng.below(undead.len().max(1)).min(undead.len() - 1)];
                    self.monsters[leader_idx].promote(
                        enc.undead_rite_boss_hp_mult,
                        enc.undead_rite_boss_atk_mult,
                        enc.undead_rite_boss_xp_mult,
                        "undead_rite",
                    );
                    self.monsters[leader_idx].become_boss(boss);
                    self.monsters[leader_idx].apply_affix(erng.next_u64());
                    for _ in 0..enc.undead_rite_minions {
                        let angle = erng.unit() * std::f64::consts::TAU;
                        let dist = enc.pack_spread * (0.4 + 0.6 * erng.unit());
                        let mpos = corridor_offset(
                            pos,
                            dist * angle.cos(),
                            dist * angle.sin(),
                            self.radial_half,
                            self.corridor_lateral.max(1.0),
                        );
                        let midx = self.monsters.len();
                        let mseed = erng.next_u64();
                        self.monsters.push(MonsterSpawn::build(
                            balance,
                            format!("mob-{midx}"),
                            kind,
                            mpos,
                            mseed,
                        ));
                        self.monsters[midx].area_min_x = start_x;
                        self.monsters[midx].area_max_x = end_x;
                        taken.insert(bend(mpos));
                        self.monsters[midx].promote(
                            enc.undead_rite_minion_hp_mult,
                            enc.undead_rite_minion_atk_mult,
                            enc.minion_xp_mult,
                            "minion",
                        );
                        // A rite's retinue is its own dead, whatever the local wildlife is.
                        self.monsters[midx].faction = "undead".to_string();
                    }
                    // Keep the rite a rite: nothing else groups into it.
                    x += balance.ai.group_radius;
                }
                // PACKS, on a distance RAMP (`[[encounters.group_ramp]]`): duels while a
                // player learns the ATB, then duos, then mixed triples, then quads. The
                // band is chosen by the spawn's hub distance — which in corridor space is
                // simply its x — so the ramp is a readable curve rather than a dice roll.
                // Rolled on the elite stream so the main placement draws stay
                // byte-identical (determinism tests). Never in the spawn section or the
                // tutorial: onboarding stays calm regardless of the table.
                let band = enc.group_band_at(pos.x.max(0.0));
                if let Some(band) = band {
                    if i > 0
                        && !became_rite
                        && !self.tutorial
                        && band.size > 1
                        && self.monsters[leader_idx].encounter_class == "standard"
                        && erng.unit() < band.chance
                    {
                        self.monsters[leader_idx].promote(
                            enc.leader_hp_mult,
                            enc.leader_atk_mult,
                            enc.leader_xp_mult,
                            "leader",
                        );
                        // FORMATION. A pack of three or more forms two ranks: the leader
                        // and roughly the front half hold the line, the rest stand behind
                        // it. Below three there is no formation to speak of — two
                        // creatures abreast is not a front and a back — and a solo spawn
                        // is always front, or a single creature would be half-immune to
                        // every sword in the game for free.
                        let ranked = band.size >= 3;
                        let front = band.size.div_ceil(2);
                        for k in 0..band.size - 1 {
                            // Mixed groups: past the duo band, some of the littles are a
                            // different species than what they follow.
                            let mkind = if erng.unit() < band.mixed_chance {
                                kinds[erng.below(kinds.len())]
                            } else {
                                kind
                            };
                            let angle = erng.unit() * std::f64::consts::TAU;
                            let dist = enc.pack_spread * (0.35 + 0.65 * erng.unit());
                            let mpos = corridor_offset(
                                pos,
                                dist * angle.cos(),
                                dist * angle.sin(),
                                self.radial_half,
                                self.corridor_lateral.max(1.0),
                            );
                            let midx = self.monsters.len();
                            let mseed = erng.next_u64();
                            self.monsters.push(MonsterSpawn::build(
                                balance,
                                format!("mob-{midx}"),
                                mkind,
                                mpos,
                                mseed,
                            ));
                            self.monsters[midx].area_min_x = start_x;
                            self.monsters[midx].area_max_x = end_x;
                            taken.insert(bend(mpos));
                            self.monsters[midx].promote(
                                enc.minion_hp_mult,
                                enc.minion_atk_mult,
                                enc.minion_xp_mult,
                                "minion",
                            );
                            // Minion `k` is the (k+1)th body in the pack — the leader is
                            // the first and always holds the front.
                            self.monsters[midx].back_row = ranked && k + 1 >= front;
                        }
                        // Clear the grouping radius before the next spawn, or two packs
                        // placed a normal gap apart merge into one oversized fight — the
                        // ramp promises a quad, not an accidental eight.
                        x += balance.ai.group_radius;
                    }
                }

                let gap = creature_spacing * (1.0 + wg.monster_spacing_jitter * rng.signed());
                x += gap.max(2.0);
            }
        }
        self.creature_spots = taken;

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
                spent_tick: 0,
                entity_id: format!("res-{nid}"),
                kind: rk.to_string(),
                position: Position::new(rx, ry),
                elevation: 0,
                remaining: node_stock(balance, rk),
            });
            section_resources.push(nid);
        }

        // The clear path meanders to a fresh ±y at this section's end. The initial
        // chain's last section aims its final waypoint at the portal; streamed
        // sections just meander onward (endless). This completes the path segment
        // spanning the section, letting obstacles + terraces avoid the whole tube.
        let is_chain_end = i + 1 == wg.area_count.max(1);
        // Where this section's path segment aims (the deep portal on the last section, a
        // fresh meander ±y otherwise). A* ROUTES the segment there through walkable
        // terrain, bending around cliffs; the portal is the routed segment's walkable end.
        let exit_target = if is_chain_end {
            Position::new(end_x - wg.portal_setback, 0.0)
        } else {
            Position::new(end_x, wg.path_meander * rng.signed())
        };
        let entry = *self.path.last().unwrap_or(&Position::new(0.0, 0.0));
        let route = self.astar_route(entry, exit_target);
        for p in &route {
            self.path.push(*p);
        }
        let portal = if is_chain_end {
            *self.path.last().unwrap()
        } else {
            Position::new(end_x - wg.portal_setback, 0.0)
        };

        // WEB of trails: weave extra routes through this section so the overworld is an
        // interconnected maze with real junctions + choices, not one lane. Built in
        // CORRIDOR space (bent + streamed like `path`) from the section's entry/exit
        // backbone points; own rng stream so the main creature/obstacle/terrace draws
        // stay byte-stable. The clear tube is carved around these too (below).
        {
            let entry = self.path[self.path.len() - 2];
            let exit = *self.path.last().unwrap();
            // Dungeons keep their own rooms-and-corridors layout — no woven web through
            // them. Scale the count with section size so a small early section keeps its
            // dense walls (a few trails) while a big deep one webs richly.
            let n = if is_dungeon {
                0
            } else {
                (wg.web_trails_per_area * (length / 40.0).clamp(0.35, 1.5)).round() as usize
            };
            if n > 0 {
                let mut wrng = Rng(section_seed(self.seed_base, i) ^ 0x3EB0_57A1_3EB0_57A1);
                let lat = (self.lateral - 2.0).max(2.0);
                // A parallel LOOP: a chain of side-offset nodes from entry to exit (a
                // second route alongside the backbone — take either fork).
                let mut prev = entry;
                let mut nodes: Vec<Position> = Vec::new();
                for k in 0..n {
                    let t = (k as f64 + 1.0) / (n as f64 + 1.0);
                    let bx = entry.x + (exit.x - entry.x) * t;
                    let by = entry.y + (exit.y - entry.y) * t;
                    let side = if k % 2 == 0 { 1.0 } else { -1.0 };
                    let off = wrng.range(6.0, lat) * side;
                    let nd = Position::new(bx, (by + off).clamp(-lat, lat));
                    self.corridor_web.push((prev, nd));
                    prev = nd;
                    nodes.push(nd);
                }
                self.corridor_web.push((prev, exit)); // close the loop back to the backbone
                // CROSS-LINKS: tie every other loop node straight to the backbone
                // midpoint — real junctions where routes cross.
                let mid = Position::new((entry.x + exit.x) * 0.5, (entry.y + exit.y) * 0.5);
                for &nd in nodes.iter().step_by(2) {
                    self.corridor_web.push((nd, mid));
                }
                // A DEAD-END SPUR off the last node — an explore-for-it pocket.
                if let Some(&last) = nodes.last() {
                    let spur = Position::new(
                        (last.x + wrng.range(-6.0, 6.0)).clamp(start_x + 2.0, end_x - 2.0),
                        (last.y + wrng.range(-8.0, 8.0)).clamp(-lat, lat),
                    );
                    self.corridor_web.push((last, spur));
                }
            }
        }

        // Climbing maze (#B): the terrain for this section is created up front so a
        // plateau can be raised over the INTERIOR of the clear-path segment — the
        // critical route itself climbs up a ramp and back down. Endpoints (the
        // section waypoints) stay on level 0, so seams/portal/streaming and the
        // "waypoints are grounded" guarantee are untouched; only the mid-segment
        // rises. `maybe_climb_path` uses its own rng stream so the creature/obstacle/
        // terrace/chest draws below stay byte-stable.
        let mut terrain = Terrain::empty(start_x, end_x, -self.lateral, self.terrain_cell);
        // Crown a walkable SUMMIT with a payoff (#3): where the A*-routed clear path
        // climbs over a genuine crest of the rolling heightmap, a gate-boss guards the
        // top on a `peak_boss_chance` roll, otherwise a guaranteed treasure chest rewards
        // the climb — so scaling a hill is always worth it. No discrete terrace now: the
        // crest IS the heightmap, so the reward sits at elevation 0 on high ground (the
        // client grounds every entity on `terrain_height`), and it's guaranteed reachable
        // because it lands ON the cleared route. Boss is held off the tutorial (a first
        // dive tops out in loot, not a wall of HP) and the creature-free hub ring; every
        // other qualifying summit still gets one or the other. Own rng stream keeps the
        // main creature/obstacle/chest draws byte-stable.
        // Authored CLIMBABLE landmark MOUNTAIN (#3): on a `path_climb_chance` roll (biome
        // weighted), raise a walkable dome beside the route and crown its SUMMIT with a
        // gate-boss (`peak_boss_chance`) or a guaranteed treasure chest — so scaling a
        // mountain is always worth it. The dome is summed into the terrain
        // (`terrain::peak_height`), so it renders and the reward's Y (client
        // `terrain_height`) puts it on the peak. Placed beside an interior route waypoint
        // (a landmark near the trail), deep enough that the big dome has room, and off the
        // tutorial. Own rng stream keeps the main creature/obstacle/chest draws byte-stable.
        let terr_mult = biome_terrace_mult(biome);
        let mut prng = Rng(section_seed(self.seed_base, i) ^ 0x5EED_9EA1_B055_0BEE);
        let mid = route.get(route.len() / 2).copied();
        if let Some(base_wp) = mid {
            if !self.tutorial
                && base_wp.x >= wg.peak_min_distance
                && prng.unit() < wg.path_climb_chance * terr_mult
            {
                // A walkable dome: height ≤ radius·PEAK_MAX_ASPECT keeps its slope climbable.
                let radius = wg.peak_radius;
                let height = radius * meld_proto::terrain::PEAK_MAX_ASPECT as f64 * 0.9;
                // Nudge the centre off the path so the climb is a side-trip landmark, kept
                // inside the lateral bounds.
                let side = if prng.unit() < 0.5 { 1.0 } else { -1.0 };
                let cy = (base_wp.y + side * radius * 0.55)
                    .clamp(-(self.lateral - 2.0), self.lateral - 2.0);
                let summit = Position::new(base_wp.x, cy);
                self.peaks
                    .push([summit.x as f32, summit.y as f32, radius as f32, height as f32]);
                // `hub_safe_radius` alone put a 10x-HP Gatekeeper 14 units from the
                // hub — a new party's first contact, and the end of the dive.
                if summit.x > wg.hub_safe_radius
                    && summit.distance_floor() >= enc.gatekeeper_min_distance
                    && prng.unit() < wg.peak_boss_chance
                {
                    let gidx = self.monsters.len();
                    let gseed = section_seed(self.seed_base, i) ^ 0x9EA1_B055_0000_0000;
                    self.monsters
                        .push(MonsterSpawn::build(balance, format!("mob-{gidx}"), kinds[0], summit, gseed));
                    self.monsters[gidx].area_min_x = start_x;
                    self.monsters[gidx].area_max_x = end_x;
                    self.monsters[gidx].promote(
                        enc.gatekeeper_hp_mult,
                        enc.gatekeeper_atk_mult,
                        enc.gatekeeper_xp_mult,
                        "gatekeeper",
                    );
                    self.monsters[gidx].apply_affix(gseed ^ 0xAFF1);
                    // Through `become_boss`, not by writing `boss_kind`: the identity and
                    // the LINEAGE that comes with it are one act. Assigning the field
                    // directly is what left every summit Gatekeeper fighting as whatever
                    // wildlife it rode in on — a Choirmother tagged `beast`, which is the
                    // exact bug `become_boss` was written to end.
                    let boss = pick_gatekeeper_boss_kind(summit.x.floor() as i64, gseed ^ 0xB055);
                    self.monsters[gidx].become_boss(boss);
                } else {
                    self.chests.push(Chest {
                        opened_tick: 0,
                        entity_id: format!("chest-{}", self.chests.len()),
                        position: summit,
                        tier: Scaling::new(balance).tier(summit.x.floor() as i64) as i32,
                        opened: false,
                        elevation: 0,
                    });
                }
            }
        }

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
            if dist_to_path(&pos, &self.path) < self.path_clear_radius + radius
                || dist_to_web(&pos, &self.corridor_web) < self.web_clear() + radius
            {
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
        // Biome-weighted: ashfall is mountainous, desert nearly flat (see biome_terrace_mult).
        let n_terraces = (self.terraces_per_area * biome_terrace_mult(biome)).max(0.0).round() as usize;
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
                    opened_tick: 0,
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
                        opened_tick: 0,
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
            // The seam itself still forms — the biomes do change here — but nobody
            // mounts the door this shallow. The first seam sits at d=100, and a
            // `gatekeeper_hp_mult` (10x) wall there is a level-1 party's whole dive.
            let mount_gatekeeper = bd.floor() as i64 >= enc.gatekeeper_min_distance;
            self.seams.push(Seam {
                x: bd,
                gap_y: path_y_at(&self.path, bd),
                gap_half_width: wg.path_clear_radius,
                biome_from: from,
                biome_to: to,
            });
            // FS-4: a GATEKEEPER boss stands in the door — a big, unavoidable fight
            // guarding the pass into the next region, with a fat guaranteed reward.
            // Gatekeepers spawn on EVERY run, including the tutorial: a milestone
            // gate-boss is a legible "you made it to the next biome" moment, and the
            // in-memory demo build is perpetually a first (tutorial) dive — gating them
            // off it meant bosses effectively never appeared. Scattered Elites stay off
            // the tutorial (see the elite roll above); only the gate-bosses run here.
            if !mount_gatekeeper {
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
            // Some doors are RAIDS. Rolled on its own sub-stream so adding this cannot shift
            // any existing draw, and declared on the spawn so both the scaling and the name
            // come from the one number — the wall used to be four-party-sized for everyone
            // and said nothing.
            self.monsters[gidx].scale_to_warband(roll_warband(enc, gseed ^ 0x5CA1_E000));
            // FS-4: unique boss mechanics — this gate boss fights as one of
            // the named bosses, tiered by which biome-seam threshold it guards.
            // `become_boss` rather than a bare field write, so it also takes the boss's
            // own lineage (see the summit gate above).
            let boss = pick_gatekeeper_boss_kind(bd as i64, gseed ^ 0xB055);
            self.monsters[gidx].become_boss(boss);
        }

        // Every biome is a MAZE: pack the play area with extra impassable props so
        // only the winding clear path (plus the branch detours) stays open. Forest is
        // densest (forest_obstacle_mult); other biomes use maze_obstacle_mult. Uses a
        // SEPARATE rng stream (section_seed ⊕ a constant) so main's creature/terrace/
        // chest/seam draws stay byte-identical and every determinism test still holds.
        // Ground level only (nothing floating on a terrace/plateau), and never buries
        // the path/creatures/nodes/chests.
        let maze_mult = biome_obstacle_mult(wg, biome);
        // A DUNGEON SECTION IS STILL A BIOME SECTION. Its divider walls used to be laid
        // INSTEAD of the maze fill ("rooms-and-corridors instead of the scattered fill"),
        // which was true when a section was a 20-tile corridor with three rooms in it.
        // After WG-4 a section is an annular band spanning the whole 340° arc, so the two
        // walls are a rounding error across it and `dungeon_every = 4` meant EVERY FOURTH
        // RING OF THE WORLD was a featureless plain. Measured at seed 424242 out to d1700:
        // dungeon sections averaged 0.167 obstacles per 1000 u² against 4.92 for ordinary
        // ones — a 30x gap — and section 16 (forest, `forest_obstacle_mult = 7.0`, "a
        // THICK wood") held 29 obstacles across 900,893 u², a mean prop spacing of 88
        // tiles. The walls are laid OVER the biome now; they still leave their door gaps
        // on the clear path, so connectivity is untouched.
        if maze_mult > 0.0 {
            let mut frng = Rng(section_seed(self.seed_base, i) ^ 0x7EE5_7EE5_7EE5_7EE5);
            // Radial density compensation: the fan bends the fixed-width corridor into an
            // arc that widens with radius, so a per-area count spreads ever thinner at
            // depth (a deep area is a huge annular sector). Scale the count by how much
            // this area's arc stretches vs the corridor width — capped so the deepest
            // areas stay renderable — so the maze holds its density instead of thinning
            // into an open field. Obstacles are still placed across the corridor lateral
            // and bent, so the extra count fills the widened arc.
            let radial_scale =
                maze_fill_scale(wg, self.radial_half, self.lateral, start_x, end_x);
            let extra = (maze_mult * wg.obstacles_per_area * radial_scale).round().max(0.0) as usize;
            let fill_kind = fill_kind_for_biome(biome);
            // Density taper: near each edge, blend toward the NEIGHBOUR section's
            // density so a dense biome visibly THINS as it gives way to a sparser one
            // (trees scarce into desert) and thickens into a denser one (rock into
            // Ashfall) — matching the ground cross-fade. Only ever thins (a section
            // never exceeds its own count), and the neighbour ramps up from its side.
            let tw = wg.biome_transition_width.max(0.0);
            let next_biome = self.force_biome.unwrap_or_else(|| {
                section_biome(balance, self.seed_base, i + 1, end_x.floor() as i64, Some(biome), self.tutorial)
            });
            let prev_ratio = (biome_obstacle_mult(wg, prev_biome.unwrap_or(biome)) / maze_mult).min(1.0);
            let next_ratio = (biome_obstacle_mult(wg, next_biome) / maze_mult).min(1.0);
            let keep_prob = |ox: f64| -> f64 {
                let mut p = 1.0_f64;
                if tw > 0.0 {
                    if ox < start_x + tw {
                        let t = ((ox - start_x) / tw).clamp(0.0, 1.0);
                        p = p.min(prev_ratio + (1.0 - prev_ratio) * t);
                    }
                    if ox > end_x - tw {
                        let t = ((end_x - ox) / tw).clamp(0.0, 1.0);
                        p = p.min(next_ratio + (1.0 - next_ratio) * t);
                    }
                }
                p
            };
            // How close is too close, asked in the frame the player stands in. Corridor y
            // is an ANGLE: at r=355 a tangential gap is worth 37× what it measures here.
            // Comparing raw corridor distance therefore threw out trees that would end up
            // 190 world units apart, which is why the forest asked for 392 props, placed
            // 90, and read as a field with a few trees in it — the count was compensated
            // for the fan, the SPACING never was. Indexed, because the check now succeeds
            // often enough that a linear scan over every tree in the world would dominate
            // world generation.
            let mut near =
                BlockGrid::new(2.0 * wg.obstacle_max_radius + 1.2, radial_scale, start_x, end_x);
            near.seed(&self.monsters, &self.resources, &self.chests, &self.obstacles);
            let (mut fp, mut fa) = (0usize, 0usize);
            while fp < extra && fa < extra * 12 {
                fa += 1;
                let ox = start_x + frng.unit() * length;
                let oy = frng.signed() * (self.lateral - 1.0);
                // Taper toward the neighbouring biome near the section edges.
                if frng.unit() > keep_prob(ox) {
                    continue;
                }
                let radius = wg.obstacle_min_radius
                    + frng.unit() * (wg.obstacle_max_radius - wg.obstacle_min_radius);
                let pos = Position::new(ox, oy);
                if dist_to_path(&pos, &self.path) < self.path_clear_radius + radius
                    || dist_to_web(&pos, &self.corridor_web) < self.web_clear() + radius
                {
                    continue;
                }
                if terrain.level_at(&pos) != 0 {
                    continue;
                }
                if near.blocked(&pos, radius) {
                    continue;
                }
                near.insert(pos, radius);
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
                let mut y = -self.lateral + 1.0;
                while y <= self.lateral - 1.0 {
                    let pos = Position::new(wall_x, y);
                    // A door gap wherever the (A*-routed, possibly winding) clear path
                    // crosses this wall — so every path crossing has an opening, not just
                    // one, and connectivity survives the cliff detours.
                    let in_door = dist_to_path(&pos, &self.path) < wg.dungeon_door_half + r;
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
                opened_tick: 0,
                entity_id: format!("chest-{}", self.chests.len()),
                position: cpos,
                tier: Scaling::new(balance).tier(chest_x.floor() as i64) as i32,
                opened: false,
                elevation,
            });
        }

        // Keep every connector's reach CLEAR: the dense biome fill (now much denser)
        // could otherwise drop a tree/rock right on a ladder or ramp and strand a
        // terrace. Prune any obstacle overlapping a connector's reach (scatter fill runs
        // before connectors exist, so a placement-time check alone can't catch it).
        if !terrain.connectors.is_empty() {
            let conns: Vec<(Position, f64)> =
                terrain.connectors.iter().map(|c| (c.position, c.radius)).collect();
            self.obstacles.retain(|o| {
                !conns.iter().any(|(cp, cr)| o.position.distance_to(cp) < cr + o.radius + 0.5)
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
            shifted_at: 0,
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
    /// Route the section's clear-path segment `entry → exit_target` through WALKABLE
    /// terrain with grid A*, so the guaranteed route bends AROUND heightmap cliffs
    /// instead of through them (feasibility under slope collision). Works in CORRIDOR
    /// space but costs WORLD slope (each cell bent through the radial arc), so it stays
    /// aligned with the rest of section generation (which is corridor-space, bent later).
    /// Returns the corridor waypoints AFTER `entry` (last one ≈ a walkable `exit_target`).
    /// Falls back to a straight `[exit_target]` if no route is found (the connected base
    /// makes that essentially never happen).
    fn astar_route(&self, entry: Position, exit_target: Position) -> Vec<Position> {
        use std::cmp::Reverse;
        use std::collections::{BinaryHeap, HashMap};
        // Fine grid: the cliff RING (steep transition) is only ~2u thick, so a coarse
        // grid would step over it (base cell → mesa-top cell) and strand a walker on the
        // skipped ring. 1.5u reliably samples the ring so A* routes AROUND the whole mesa.
        const CELL: f64 = 1.5;
        let half = self.radial_half;
        let lat = self.corridor_lateral.max(1.0);
        let (ox, oz) = self.terrain_off;
        // A corridor cell is passable iff its BENT (world) position is walkable ground.
        let walk = |c: (i64, i64)| -> bool {
            let w = radial_tf(Position::new(c.0 as f64 * CELL, c.1 as f64 * CELL), half, lat);
            meld_proto::terrain::routable(w.x as f32, w.y as f32, ox, oz)
        };
        // The radial bend makes a 1-cell corridor step span many WORLD units tangentially
        // at large radius, so checking only cell centres would leap over buttes. Check the
        // whole EDGE by sampling its bent arc (corridor-interpolated then bent, matching
        // densification) at ~2-world-unit intervals — this is what makes A* honest.
        let edge_walk = |a: (i64, i64), b: (i64, i64)| -> bool {
            let (ca, cb) = (
                Position::new(a.0 as f64 * CELL, a.1 as f64 * CELL),
                Position::new(b.0 as f64 * CELL, b.1 as f64 * CELL),
            );
            let (wa, wb) = (radial_tf(ca, half, lat), radial_tf(cb, half, lat));
            // Sample at ≤1 world unit (min 2 steps ⇒ always incl. the midpoint), so the
            // ~2u-thick cliff ring is never skipped even on short near-hub edges.
            let steps = (wa.distance_to(&wb)).ceil().max(2.0) as i32;
            for s in 0..=steps {
                let t = s as f64 / steps as f64;
                let c = Position::new(ca.x + (cb.x - ca.x) * t, ca.y + (cb.y - ca.y) * t);
                let w = radial_tf(c, half, lat);
                if !meld_proto::terrain::routable(w.x as f32, w.y as f32, ox, oz) {
                    return false;
                }
            }
            true
        };
        let cell_of = |p: Position| ((p.x / CELL).round() as i64, (p.y / CELL).round() as i64);
        let pos_of = |c: (i64, i64)| Position::new(c.0 as f64 * CELL, c.1 as f64 * CELL);
        let start = cell_of(entry);
        let ylim = (lat / CELL).ceil() as i64 + 4;
        // Snap a target to the nearest WALKABLE cell (so no waypoint lands on a cliff).
        let nudge = |mut goal: (i64, i64)| -> (i64, i64) {
            if !walk(goal) {
                'outer: for r in 1..40i64 {
                    for dx in -r..=r {
                        for dy in -r..=r {
                            if dx.abs() != r && dy.abs() != r {
                                continue;
                            }
                            let c = (goal.0 + dx, goal.1 + dy);
                            if walk(c) {
                                goal = c;
                                break 'outer;
                            }
                        }
                    }
                }
            }
            goal
        };
        let goal = nudge(cell_of(exit_target));
        // Grid A* from `start` toward `goal`, costing WORLD walkability along each bent
        // edge. Generous x-slack so a detour around a butte can swing well past the
        // straight entry→goal span without the search box clipping the only way around.
        let (xlo, xhi) = (start.0.min(goal.0) - 80, start.0.max(goal.0) + 80);
        let exit_w = radial_tf(exit_target, half, lat);
        let h = |c: (i64, i64)| (((c.0 - goal.0).pow(2) + (c.1 - goal.1).pow(2)) as f64).sqrt();
        let mut open: BinaryHeap<Reverse<(i64, (i64, i64))>> = BinaryHeap::new();
        let mut g: HashMap<(i64, i64), f64> = HashMap::new();
        let mut came: HashMap<(i64, i64), (i64, i64)> = HashMap::new();
        g.insert(start, 0.0);
        open.push(Reverse(((h(start) * 64.0) as i64, start)));
        let mut iters = 0u32;
        // The meander exit can land on a mesa TOP — walkable, but ring-enclosed by the
        // cliff face and so unreachable from the connected base. If A* can't connect to
        // it, we head instead for the closest cell we DID reach (nearest the exit in
        // WORLD space), reconstructed from the very same exploration. That route is
        // feasible BY CONSTRUCTION (every edge was edge_walk-checked) and never a
        // straight cliff-crosser — so a walled-off exit just shortens the section instead
        // of stranding the walker. `best` tracks that closest reachable cell.
        let mut best = start;
        let mut best_d = radial_tf(pos_of(start), half, lat).distance_to(&exit_w);
        while let Some(Reverse((_, cur))) = open.pop() {
            iters += 1;
            if iters > 300_000 {
                break;
            }
            if cur == goal {
                best = goal;
                break;
            }
            let d = radial_tf(pos_of(cur), half, lat).distance_to(&exit_w);
            if d < best_d {
                best_d = d;
                best = cur;
            }
            let cg = *g.get(&cur).unwrap_or(&f64::INFINITY);
            for dx in -1..=1i64 {
                for dy in -1..=1i64 {
                    if dx == 0 && dy == 0 {
                        continue;
                    }
                    let n = (cur.0 + dx, cur.1 + dy);
                    if n.0 < xlo || n.0 > xhi || n.1 < -ylim || n.1 > ylim {
                        continue;
                    }
                    // The full edge (bent arc) must be walkable, not just the endpoint —
                    // catches buttes on the long tangential leaps.
                    if !edge_walk(cur, n) {
                        continue;
                    }
                    let step = if dx != 0 && dy != 0 { std::f64::consts::SQRT_2 } else { 1.0 };
                    let ng = cg + step;
                    if ng < *g.get(&n).unwrap_or(&f64::INFINITY) {
                        g.insert(n, ng);
                        came.insert(n, cur);
                        open.push(Reverse((((ng + h(n)) * 64.0) as i64, n)));
                    }
                }
            }
        }
        // Reconstruct start→best (== goal when A* connected). `best == start` only if the
        // entry itself is boxed in with no walkable neighbour — return empty (the path
        // just stays put this section) rather than a cliff-crossing straight line.
        let mut cells = vec![best];
        let mut c = best;
        while c != start {
            match came.get(&c) {
                Some(&p) => {
                    cells.push(p);
                    c = p;
                }
                None => break,
            }
        }
        cells.reverse();
        // Skip `start` (== entry, already on the path); return the routed waypoints.
        cells.iter().skip(1).map(|&c| pos_of(c)).collect()
    }

    /// Half-width of a WEB trail's cleared slit — narrower than the backbone tube so the
    /// web threads tight paths THROUGH the dense terrain rather than clearing it away
    /// (the maze must keep its walls). Still wide enough to walk.
    fn web_clear(&self) -> f64 {
        (self.path_clear_radius * 0.5).max(1.8)
    }

    /// Does the axis-aligned terrace rectangle come within the clear-path tube OR a web
    /// trail? Samples the rect corners + centre + edge midpoints. Keeps raised cliffs off
    /// both the backbone and the woven trails, so every route stays walkable on level 0.
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
        let web_margin = self.web_clear() + self.terrain_cell;
        samples.iter().any(|s| {
            dist_to_path(s, &self.path) < margin || dist_to_web(s, &self.corridor_web) < web_margin
        })
    }

    /// WG-4: half the fan arc in radians (0 ⇒ flat corridor). Exposed so the wire can
    /// carry it to the client, which bends terrace/cliff/connector geometry by the same
    /// arc the server used to fan entity positions.
    pub fn radial_half(&self) -> f64 {
        self.radial_half
    }

    /// The corridor half-extent the arc maps against (pairs with [`Self::radial_half`]).
    pub fn corridor_lateral(&self) -> f64 {
        self.corridor_lateral
    }

    /// Un-bend a WORLD position back into the corridor frame the terrain grids live
    /// in. Identity in flat-corridor mode (`radial_half == 0`). Inverse of the
    /// `radialize` map `world = (r·cosθ, r·sinθ)` with `r = corridor_x`,
    /// `θ = (corridor_y / lat)·half` — so terrace elevation, connectors and touch all
    /// sample the right cell even though the world fans around the hub.
    pub fn corridorize(&self, p: &Position) -> Position {
        if self.radial_half <= 0.0 {
            return *p;
        }
        let r = p.x.hypot(p.y);
        let theta = p.y.atan2(p.x);
        let lat = self.corridor_lateral.max(1.0);
        Position::new(r, (theta / self.radial_half) * lat)
    }

    /// The elevation level at world position `p` — samples whichever section's
    /// terrain contains `p` (0 outside any section, e.g. behind the hub). Un-bends
    /// into corridor coords first so it works in the radial world.
    pub fn level_at(&self, p: &Position) -> u8 {
        area_level_at(&self.areas, &self.corridorize(p))
    }

    /// Is there a connector within reach of `p` that joins levels `a`↔`b`? (A move
    /// crossing a level boundary is allowed only on such a connector.)
    fn connector_between(&self, p: &Position, a: u8, b: u8) -> bool {
        if a == b {
            return false;
        }
        // Connectors live in un-bent corridor coords (radialize leaves the terrain
        // grids alone), so compare the reach in that frame.
        let cp = self.corridorize(p);
        self.areas.iter().any(|area| {
            area.terrain
                .connectors
                .iter()
                .any(|c| c.joins(a, b) && cp.distance_to(&c.position) <= c.radius)
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
    /// radius by `aggro_mult[player_id]` (default 1.0) — the Phoenix Guard "Bulwark"
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
        // Arc params for un-bending world → corridor when sampling terrace elevation
        // (creatures stay on their own level even though the world fans around the hub).
        let (radial_half, corridor_lateral) = (self.radial_half, self.corridor_lateral);
        let corridorize = |p: &Position| -> Position {
            if radial_half <= 0.0 {
                *p
            } else {
                let r = p.x.hypot(p.y);
                let theta = p.y.atan2(p.x);
                Position::new(r, (theta / radial_half) * corridor_lateral.max(1.0))
            }
        };
        let (wander, chase) = (self.wander_speed, self.chase_speed);
        let (wander_leg, wander_arrive) = (self.wander_leg_seconds, self.wander_arrive_radius);
        let (wander_pause_chance, wander_pause) =
            (self.wander_pause_chance, self.wander_pause_seconds);
        let (aggro, terr_aggro, leash) = (self.aggro_radius, self.territorial_aggro_radius, self.leash_radius);
        let (skirmish_aggro, skirmish_range) = (self.skirmish_aggro, self.skirmish_range);
        let interval = self.skirmish_interval;
        let obstacles = self.blocking_field();
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
            // A Psyker's pin: it does not move, chase or skirmish while it holds. Counted
            // down HERE rather than against a clock, so the world stays a pure function of
            // its own ticks and a paused instance does not leak the hold away.
            if m.held_for > 0.0 {
                m.held_for = (m.held_for - dt).max(0.0);
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
                // scaled by their Bulwark multiplier (Phoenix Guard parties are chased
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
                    // Wander: walk to a destination that OUTLIVES THIS TICK. Standing
                    // still counts down first — a creature that never stops reads as
                    // machinery rather than as something grazing.
                    if m.wander_wait > 0.0 {
                        m.wander_wait -= dt;
                        (0.0, 0.0, wander)
                    } else {
                        m.wander_left -= dt;
                        let arrived = m
                            .wander_to
                            .is_some_and(|t| m.position.distance_to(&t) <= wander_arrive);
                        // Repick on arrival OR on the leg timing out: a destination can
                        // walk a creature into a rock, where the per-axis slide leaves it
                        // grinding against the same tree for the rest of the dive.
                        if m.wander_to.is_none() || arrived || m.wander_left <= 0.0 {
                            // A point in the leash DISC, not on its rim — sqrt for a
                            // uniform draw over the area, so a creature crosses its
                            // territory instead of only ever visiting the edge. No axis
                            // squash: `home` is world-space, and in the radial fan a
                            // squashed y biased every creature into walking radially
                            // (in/out) when tangential movement is what reads as roaming.
                            let ang = next_unit(&mut m.rng) * std::f64::consts::TAU;
                            let rad = leash * next_unit(&mut m.rng).sqrt();
                            m.wander_to = Some(Position::new(
                                m.home.x + ang.cos() * rad,
                                m.home.y + ang.sin() * rad,
                            ));
                            m.wander_left = wander_leg;
                            if next_unit(&mut m.rng) < wander_pause_chance {
                                m.wander_wait =
                                    wander_pause * (0.5 + next_unit(&mut m.rng));
                            }
                        }
                        match m.wander_to {
                            Some(t) => {
                                (t.x - m.position.x, t.y - m.position.y, wander)
                            }
                            None => (0.0, 0.0, wander),
                        }
                    }
                }
            };
            let mag = (dx * dx + dy * dy).sqrt();
            if mag > 1e-6 {
                dx /= mag;
                dy /= mag;
                let step = speed * dt;
                // Clamp to the world bounds AND the creature's own area so it stays in
                // its biome. In the radial fan the area is a RADIUS band ([start_x,
                // end_x] = a biome ring), so clamp the candidate's radius; in flat
                // corridor mode it's an x-band as before. Keeps biome creatures inside
                // their biome instead of wandering across rings (#10).
                let (mut nx, mut ny) = (m.position.x + dx * step, m.position.y + dy * step);
                if radial_half > 0.0 {
                    let r = (nx * nx + ny * ny).sqrt();
                    let (r_lo, r_hi) = (m.area_min_x.max(0.0), m.area_max_x);
                    if r > 1e-6 && (r < r_lo || r > r_hi) {
                        let rc = r.clamp(r_lo, r_hi);
                        nx *= rc / r;
                        ny *= rc / r;
                    }
                }
                let nx = if radial_half > 0.0 { nx } else { nx.max(x_min.max(m.area_min_x)).min(x_max.min(m.area_max_x)) };
                let ny = ny.max(-lateral).min(lateral);
                // Creatures don't walk through terrain either (slide per axis), and
                // they stay on their own elevation (never wander off a terrace edge).
                let cand = Position::new(nx, ny);
                if !obstacles.blocks(&cand, 0.5) && area_level_at(areas, &corridorize(&cand)) == m.elevation {
                    m.position = cand;
                } else if !obstacles.blocks(&Position::new(nx, m.position.y), 0.5)
                    && area_level_at(areas, &corridorize(&Position::new(nx, m.position.y))) == m.elevation
                {
                    m.position.x = nx;
                } else if !obstacles.blocks(&Position::new(m.position.x, ny), 0.5)
                    && area_level_at(areas, &corridorize(&Position::new(m.position.x, ny))) == m.elevation
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
        // Spatial hash of the POST-MOVEMENT positions, for the same reason the movement
        // pass above has one — and it is the same bug: that pass was fixed and this one,
        // twenty lines below it, was left scanning every creature for every creature.
        // Measured at d1269 (10,650 creatures) that was ~113 million pair tests per 100 ms
        // tick, each one a STRING faction compare, and it cost **1.7 seconds a tick** in a
        // release build. The game loop is a single task, so a deep dive never caught up:
        // `run.started` was never sent and the world read as unbootable.
        //
        // Cell = `skirmish_range`, so a creature's whole strike circle fits inside its own
        // cell's 3x3 neighbourhood. Determinism is preserved by tie-breaking on
        // (distance, index j), which reproduces the old `min_by` over index-ordered
        // iteration exactly, whatever order the buckets are visited in (CANON §S).
        let dcell = skirmish_range.max(1.0);
        let dcell_of = |p: &Position| ((p.x / dcell).floor() as i32, (p.y / dcell).floor() as i32);
        let mut dgrid: HashMap<(i32, i32), Vec<usize>> = HashMap::new();
        for (j, (pos, _, alive, _)) in now.iter().enumerate() {
            if *alive {
                dgrid.entry(dcell_of(pos)).or_default().push(j);
            }
        }
        // (attacker, victim, damage) — the attacker is carried so the pass can report
        // WHO is fighting whom (`CR-2`), not only who lost HP.
        let mut hits: Vec<(usize, usize, i32)> = Vec::new();
        for (i, m) in self.monsters.iter_mut().enumerate() {
            if m.defeated || m.in_battle {
                m.skirmish_cd = 0.0;
                continue;
            }
            m.skirmish_cd = (m.skirmish_cd - dt).max(0.0);
            if m.skirmish_cd > 0.0 {
                continue;
            }
            let (cx, cy) = dcell_of(&m.position);
            let mut best: Option<(f64, usize, i32)> = None;
            for dx in -1..=1 {
                for dy in -1..=1 {
                    let Some(bucket) = dgrid.get(&(cx + dx, cy + dy)) else {
                        continue;
                    };
                    for &j in bucket {
                        if j == i {
                            continue;
                        }
                        let (pos, fac, alive, def) = &now[j];
                        if !*alive || !creatures_hostile(&m.faction, fac) {
                            continue;
                        }
                        let d = m.position.distance_to(pos);
                        if d > skirmish_range {
                            continue;
                        }
                        if best.is_none_or(|(bd, bj, _)| d < bd || (d == bd && j < bj)) {
                            best = Some((d, j, *def));
                        }
                    }
                }
            }
            if let Some((_, j, victim_def)) = best {
                let dmg = (m.atk - victim_def).max(1);
                hits.push((i, j, dmg));
                m.skirmish_cd = interval;
            }
        }
        for &(_, j, dmg) in &hits {
            self.monsters[j].hp -= dmg;
        }
        self.record_clashes(&hits, dt);
        // A wound closes, slowly, and only once nobody is hitting it any more.
        self.mend_creatures(dt);

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

    /// Fold this tick's landed skirmish blows into [`Arena::clashes`] (`CR-2`).
    ///
    /// A clash is derived from BLOWS, never from proximity: on a crowded ring plenty
    /// of hostiles stand within reach of each other without ever swinging, and a
    /// marker over every one of those would be noise the player learns to ignore.
    /// It lingers `[ai] clash_linger_seconds` past its last blow because creatures
    /// trade on a cadence — without the grace period the marker would strobe between
    /// swings and a watcher would be dropped every `skirmish_attack_interval`.
    ///
    /// Membership is by ENTITY ID so `prune_defeated` may compact `monsters` freely,
    /// and a member that has died, been pulled into a player's battle, or streamed
    /// away is dropped here — a clash of one is not a fight.
    fn record_clashes(&mut self, hits: &[(usize, usize, i32)], dt: f64) {
        for c in self.clashes.iter_mut() {
            c.quiet += dt;
        }
        for &(i, j, _) in hits {
            let (Some(a), Some(b)) = (
                self.monsters.get(i).map(|m| m.entity_id.clone()),
                self.monsters.get(j).map(|m| m.entity_id.clone()),
            ) else {
                continue;
            };
            // Both fighters join ONE clash. Two blows that share a creature are one
            // fight, so an existing clash holding either end absorbs the pair rather
            // than a second clash growing beside the first over the same bodies.
            match self
                .clashes
                .iter_mut()
                .position(|c| c.members.contains(&a) || c.members.contains(&b))
            {
                Some(k) => {
                    let c = &mut self.clashes[k];
                    for id in [a, b] {
                        if !c.members.contains(&id) {
                            c.members.push(id);
                        }
                    }
                    c.quiet = 0.0;
                }
                None => self.clashes.push(Clash {
                    members: vec![a, b],
                    position: Position::new(0.0, 0.0),
                    quiet: 0.0,
                }),
            }
        }
        let linger = self.clash_linger;
        // ONE index pass over the monsters, not a `find` per member: this runs every tick,
        // the world streams outward without bound, and a scan per clash member is
        // quadratic in dive depth — the same trap the placement grid exists for.
        let live: std::collections::HashMap<&str, Position> = self
            .monsters
            .iter()
            .filter(|m| !m.defeated && !m.in_battle && m.hp > 0)
            .map(|m| (m.entity_id.as_str(), m.position))
            .collect();
        self.clashes.retain_mut(|c| {
            c.members.retain(|id| live.contains_key(id.as_str()));
            if c.members.len() < 2 || c.quiet > linger {
                return false;
            }
            // Re-centre on its own bodies, so a watcher's range check follows the fight as
            // it drifts rather than the spot it started at.
            let (mut sx, mut sy) = (0.0, 0.0);
            for id in &c.members {
                let p = live[id.as_str()];
                sx += p.x;
                sy += p.y;
            }
            let n = c.members.len() as f64;
            c.position = Position::new(sx / n, sy / n);
            true
        });
    }

    /// Mend every roaming creature a little (`CR-2`).
    ///
    /// A creature's HP persists — a skirmish it survived, and a fight a party fled from,
    /// both leave it wounded — and that is the whole point: a hurt creature is an
    /// opportunity. But a wound that never closed would make the world strip-minable by
    /// attrition (walk a ring, chip everything, come home to a map of half-dead things),
    /// which is the opposite of a living world. So it heals, at
    /// `[ai] creature_regen_fraction_per_sec` of its own max, making the opportunity
    /// **time-bound** rather than permanent.
    ///
    /// Suspended while it is clashing or locked in a battle: nothing mends while it is
    /// still being hit, and the clash's own `clash_linger_seconds` is exactly what covers
    /// the gap between one blow and the next — without it a creature would tick health
    /// back between every pair of swings and a skirmish could never resolve.
    fn mend_creatures(&mut self, dt: f64) {
        let rate = self.creature_regen;
        if rate <= 0.0 {
            return;
        }
        let busy = self.clashing().into_iter().map(String::from).collect::<std::collections::HashSet<_>>();
        for m in self.monsters.iter_mut() {
            if m.defeated || m.in_battle || m.hp >= m.max_hp || busy.contains(&m.entity_id) {
                // A creature at full carries no debt forward, so a long healthy stretch
                // cannot bank a burst of healing for the moment it finally gets hit.
                m.regen_accum = 0.0;
                continue;
            }
            m.regen_accum += (m.max_hp as f64) * rate * dt;
            let whole = m.regen_accum.floor();
            if whole >= 1.0 {
                m.regen_accum -= whole;
                m.hp = (m.hp + whole as i32).min(m.max_hp);
            }
        }
    }

    /// The live clash a given creature is swinging in, if any (`CR-2`). One lookup for
    /// the snapshot's ⚔ marker and for a watcher's feed, so the two can never disagree
    /// about whether that creature is in a fight.
    pub fn clash_of(&self, entity_id: &str) -> Option<&Clash> {
        self.clashes.iter().find(|c| c.members.iter().any(|m| m == entity_id))
    }

    /// Every creature currently swinging in a clash, by entity id.
    pub fn clashing(&self) -> std::collections::HashSet<&str> {
        self.clashes.iter().flat_map(|c| c.members.iter().map(String::as_str)).collect()
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

    /// WG-4: is `player` stepping back into Last City (the western free return)?
    ///
    /// In the **radial** world the city is the angular wedge due-west that the content
    /// fan does NOT cover (`|bearing| > radial_half`); the content fans across the rest.
    /// A naive `position.x < border` test would fire across *explorable* western content
    /// — a creature legitimately placed at, say, bearing ~130° / radius 30 sits at a
    /// world-x well past `border`, so walking over to fight it would silently extract
    /// you. So return only when the player is genuinely out past the wall ring AND
    /// inside that empty western wedge. In **corridor** mode (no bend) the city is
    /// straight west of the hub, so the original `x < border` line is exactly right.
    pub fn heading_into_city(&self, player_id: &str, border: f64) -> bool {
        let Some(a) = self.avatar(player_id) else {
            return false;
        };
        let p = a.position;
        if self.radial_half > 0.0 {
            // Out to the gate ring (|border| from the hub) and inside the fan's western
            // angular gap. `bearing`: 0 = due east, ±π = due west.
            p.x.hypot(p.y) >= border.abs() && p.y.atan2(p.x).abs() > self.radial_half
        } else {
            p.x < border
        }
    }

    /// Whether `player` may work the node `entity_id` right now: it exists, has
    /// stock left, sits on the player's own elevation, and is within interaction
    /// range. Returns its content kind. Pure — this is the check a harvest channel
    /// re-runs every tick, so walking off (or emptying the node) ends the channel
    /// without the caller needing its own range logic.
    pub fn can_harvest(&self, player_id: &str, entity_id: &str) -> Option<String> {
        let a = self.avatar(player_id)?;
        let node = self
            .resources
            .iter()
            .find(|n| n.entity_id == entity_id && !n.depleted())?;
        if node.elevation != a.elevation
            || a.position.distance_to(&node.position) > self.interaction_radius
        {
            return None;
        }
        Some(node.kind.clone())
    }

    /// Take **one unit** out of the node (MS-2's channel tick) and return its content
    /// kind, or `None` if it is out of reach or already empty. The unit is banked the
    /// moment it comes out, which is what bounds an interrupted gather to the tick in
    /// flight rather than the whole node.
    /// Put one unit of stock BACK into a node — the Keeper's "the whole vein" perk,
    /// where a unit taken sometimes costs the bed nothing. Only ever called immediately
    /// after a [`Self::take_one`] that succeeded, so this restores rather than creates.
    pub fn refund_one(&mut self, entity_id: &str) {
        if let Some(n) = self.resources.iter_mut().find(|n| n.entity_id == entity_id) {
            n.remaining += 1;
        }
    }

    pub fn take_one(&mut self, player_id: &str, entity_id: &str) -> Option<String> {
        let kind = self.can_harvest(player_id, entity_id)?;
        let node = self.resources.iter_mut().find(|n| n.entity_id == entity_id)?;
        node.remaining -= 1;
        Some(kind)
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

    /// Raise a field station where this player stands. Pure: the caller has already
    /// checked the builder's skill and taken the ore — this only owns WHERE it lands
    /// and refuses to stack two on the same spot (which would let one smith cover a
    /// tile in benches and never walk again).
    #[allow(clippy::too_many_arguments)]
    /// Stand a bounty mark at its sighted distance, for its owner alone (`AD-4`).
    ///
    /// Placed on the guaranteed clear path, like a Gatekeeper, so a contract that reports
    /// a distance can actually be walked to. Refuses when the world has not yet grown out
    /// that far (the caller retries as sections stream in) and when the mark is already
    /// standing — a contract must never be two creatures.
    pub fn place_bounty_mark(
        &mut self,
        balance: &Balance,
        owner: &str,
        bounty_id: &str,
        spec: &meld_proto::bounties::BountySpec,
        seed: u64,
    ) -> bool {
        if self.monsters.iter().any(|m| m.bounty == bounty_id) {
            return false;
        }
        let x = spec.distance as f64;
        if x > self.cursor {
            return false;
        }
        let pos = Position::new(x, path_y_at(&self.path, x));
        let mark = MonsterSpawn::bounty_mark(
            balance,
            format!("mark-{bounty_id}"),
            spec,
            bounty_id,
            owner,
            pos,
            seed,
        );
        self.monsters.push(mark);
        true
    }

    /// Every bounty mark standing in this world, as `(owner, bounty id)`.
    pub fn standing_marks(&self) -> Vec<(&str, &str)> {
        self.monsters
            .iter()
            .filter(|m| !m.defeated && !m.bounty.is_empty())
            .map(|m| (m.owner.as_str(), m.bounty.as_str()))
            .collect()
    }

    pub fn place_station(
        &mut self,
        player_id: &str,
        kind: &str,
        uses: i32,
        radius: f64,
        stock: &str,
    ) -> Option<&Station> {
        let (position, elevation) = {
            let a = self.avatar(player_id)?;
            (a.position, a.elevation)
        };
        if self.stations.iter().any(|s| {
            !s.spent() && s.elevation == elevation && s.position.distance_to(&position) <= radius
        }) {
            return None;
        }
        let entity_id = format!("station-{}-{}", kind, self.stations.len());
        self.stations.push(Station {
            entity_id,
            kind: kind.to_string(),
            position,
            elevation,
            owner_player_id: player_id.to_string(),
            uses_left: uses,
            stock: stock.to_string(),
        });
        self.stations.last()
    }

    /// The station this player is standing at, if any — same elevation, within reach.
    /// A station is a place, so working at one is a question about where you are.
    pub fn station_at(&self, player_id: &str, entity_id: &str, radius: f64) -> Option<&Station> {
        let (ppos, pelev) = {
            let a = self.avatar(player_id)?;
            (a.position, a.elevation)
        };
        self.stations.iter().find(|s| {
            s.entity_id == entity_id
                && !s.spent()
                && s.elevation == pelev
                && ppos.distance_to(&s.position) <= radius
        })
    }

    /// Take a bench out of the world. Returns `(jobs left, the stock it was built from)`,
    /// so the caller can decide whether packing it up was worth anything back.
    pub fn remove_station(&mut self, entity_id: &str) -> Option<(i32, String)> {
        let i = self.stations.iter().position(|s| s.entity_id == entity_id)?;
        let s = self.stations.remove(i);
        Some((s.uses_left, s.stock))
    }

    /// Spend one of a station's jobs. Returns what is left, or None if it is gone or
    /// already spent — the caller reports the refusal.
    pub fn spend_station_use(&mut self, entity_id: &str) -> Option<i32> {
        let s = self
            .stations
            .iter_mut()
            .find(|s| s.entity_id == entity_id && !s.spent())?;
        s.uses_left -= 1;
        Some(s.uses_left)
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
        let obstacles = self.blocking_field();
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
            if obstacles.blocks(&cand, pr) {
                return None;
            }
            // Biome seams NO LONGER wall the world — that full-width barrier-with-a-gap
            // funnelled you through a single pass (the "corridor"). You cross biome
            // boundaries freely now; the boundary is just a cross-fade + a Gatekeeper you
            // can round. (`seams` kept for the Gatekeeper/biome data — see `Seam`.)
            // Heightmap CLIFFS: a steep terrain face is an impassable wall — you walk
            // AROUND it, not up it (the slide logic below routes along the edge). Gentle
            // rolling ground stays walkable. World-space, matching `cand`.
            if !self.t_walkable(cand.x, cand.y) {
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
        for m in self.monsters.iter().filter(|m| m.defeated && !m.in_battle) {
            // A bounty mark is one creature that existed for one contract; standing a
            // second one up would make a contract farmable and a felled mark a lie.
            if m.bounty.is_empty() && m.owner.is_empty() {
                self.fallen.push(Fallen {
                    entity_id: m.entity_id.clone(),
                    monster_kind: m.monster_kind.clone(),
                    home: m.home,
                    area_min_x: m.area_min_x,
                    area_max_x: m.area_max_x,
                    felled_tick: 0,
                });
            }
        }
        self.monsters.retain(|m| !m.defeated || m.in_battle);
    }

    /// Resolve a scheduled [`shift::ShiftRoll`] against the sections that actually
    /// exist: `[first, last]` inclusive, or `None` if this world has no region far
    /// enough out to be shiftable yet.
    ///
    /// The tell and the land both call this, so the region a player was warned about
    /// is provably the region that goes. Sections stream in between the two, which is
    /// why it must be resolved from the roll rather than remembered as an index.
    pub fn shift_region(&self, balance: &Balance, roll: &shift::ShiftRoll) -> Option<(usize, usize)> {
        let safe = balance.shift.safe_radius;
        let candidates: Vec<usize> = self
            .areas
            .iter()
            .enumerate()
            .filter(|(_, a)| a.start_x >= safe)
            .map(|(i, _)| i)
            .collect();
        let first = if roll.uniform_pick {
            *candidates.get((roll.locate * candidates.len() as f64) as usize % candidates.len().max(1))?
        } else {
            // Least-recently-disturbed, `locate` breaking ties so two worlds on the
            // same generation don't march up the map in the same order.
            let offset = (roll.locate * candidates.len() as f64) as usize;
            (0..candidates.len())
                .map(|k| candidates[(k + offset) % candidates.len()])
                .min_by_key(|i| self.areas[*i].shifted_at)?
        };
        let last = (first + roll.sections - 1).min(self.areas.len().saturating_sub(1));
        Some((first, last))
    }

    /// The world-space radius band a section span occupies. Corridor x IS radius in the
    /// WG-4 fan, so a section is a ring and the client can draw the tell from two numbers.
    pub fn shift_band(&self, first: usize, last: usize) -> (f64, f64) {
        let inner = self.areas.get(first).map(|a| a.start_x).unwrap_or(0.0);
        let outer = self.areas.get(last).map(|a| a.end_x).unwrap_or(inner);
        (inner, outer)
    }

    /// Land a Shift on `[first, last]` (CANON D20/§W2): the region swaps biome, every
    /// creature and collectable in it is wiped, and what grows back belongs to the new
    /// land. Returns what happened, for the wire and the persistence log.
    ///
    /// **The props are re-scattered, not reskinned** ([`Self::reroll_props`]): the new
    /// biome strews its own count at its own density in its own places, so a wood
    /// becoming desert genuinely thins out instead of turning into differently-coloured
    /// trees in the same spots. Placement rejects the clear-path tube exactly as
    /// generation does, so the route out stays feasible by construction — but a prop can
    /// still land on a player standing off-trail, and [`Self::rescue_stranded`] walks
    /// them back to the region's entry rather than constraining the world to avoid it.
    ///
    /// **Terrain elevation is NOT re-rolled.** CANON §W2 retiles a region's *biome*; the
    /// topography is the ground's bones, and re-cutting terraces under a live player
    /// drops them through a cliff face rather than merely boxing them in.
    ///
    /// Bounty marks, chests and player-raised stations survive. A contract with your
    /// name on it must not evaporate because the weather turned, and a structure is
    /// what CANON §W3's anchors will contest — that is BD-3's fight, not this one's.
    pub fn apply_shift(
        &mut self,
        balance: &Balance,
        roll: &shift::ShiftRoll,
        first: usize,
        last: usize,
    ) -> shift::ShiftOutcome {
        let (inner, outer) = self.shift_band(first, last);
        let from = self.areas.get(first).map(|a| a.biome).unwrap_or("forest");
        let to = self.incoming_biome(balance, roll, from, inner);
        let mut rng = Rng(roll.biome_pick ^ (first as u64).wrapping_mul(0x9E37_79B9));
        let in_band = |arena: &Self, p: &Position| {
            let r = arena.corridorize(p).x;
            r >= inner && r < outer
        };

        let mut wiped = Vec::new();
        // What lived here dies with the land. A creature already locked in a battle is
        // left alone: deleting a combatant mid-fight breaks the encounter its party is
        // standing in, and the fight is over in seconds either way.
        let doomed: Vec<usize> = (0..self.monsters.len())
            .filter(|&i| {
                let m = &self.monsters[i];
                !m.in_battle && m.bounty.is_empty() && in_band(self, &m.position)
            })
            .collect();
        let kinds = creatures_for_biome(to);
        for i in doomed {
            wiped.push(self.monsters[i].entity_id.clone());
            let (id, pos, amin, amax) = {
                let m = &self.monsters[i];
                (m.entity_id.clone(), m.position, m.area_min_x, m.area_max_x)
            };
            let kind = kinds[rng.below(kinds.len())];
            let seed = rng.next_u64();
            // Re-seeded IN PLACE rather than re-scattered: placement already proved
            // this spot legal (spacing, obstacles, the clear-path tube), and the Shift
            // is a change of tenant, not of geometry.
            let mut fresh = MonsterSpawn::build(balance, id, kind, pos, seed);
            fresh.area_min_x = amin;
            fresh.area_max_x = amax;
            self.monsters[i] = fresh;
        }

        // Collectables go with it, and come back as the new biome's. A depleted node
        // restocking is the point: the Shift is how a persistent world recovers from
        // being farmed, which is the whole reason it can afford to be persistent.
        let new_nodes = resources_for_biome(to);
        let old_nodes = resources_for_biome(from);
        for i in 0..self.resources.len() {
            if !in_band(self, &self.resources[i].position) {
                continue;
            }
            let slot = old_nodes.iter().position(|k| *k == self.resources[i].kind).unwrap_or(0);
            let kind = new_nodes[slot.min(new_nodes.len() - 1)].to_string();
            self.resources[i].remaining = node_stock(balance, &kind);
            self.resources[i].kind = kind;
        }


        let keep: Vec<bool> =
            self.ground_loot.iter().map(|g| !in_band(self, &g.position)).collect();
        let mut it = keep.into_iter();
        self.ground_loot.retain(|_| it.next().unwrap_or(true));

        let caught: Vec<(String, f64)> = self
            .avatars
            .iter()
            .filter(|a| {
                let r = self.corridorize(&a.position).x;
                r >= inner && r < outer
            })
            .map(|a| (a.player_id.clone(), roll.damage_fraction))
            .collect();

        for i in first..=last.min(self.areas.len().saturating_sub(1)) {
            self.areas[i].biome = to;
            self.areas[i].shifted_at = roll.generation + 1;
        }

        // Topography first, then props: both go last, so the scatter sees the creatures
        // and nodes the new land just grew and refuses to bury them.
        let peaks = self.reroll_peaks(balance, first, last, to, roll.biome_pick);
        wiped.extend(self.reroll_props(balance, first, last, to, roll.biome_pick));
        let moved = self.rescue_stranded(first, last);

        shift::ShiftOutcome {
            sections: (first..=last).collect(),
            biome: to.to_string(),
            inner_radius: inner,
            outer_radius: outer,
            wiped,
            caught,
            moved,
            peaks,
        }
    }

    /// What section `first`'s region is about to become — the tell has to name the same
    /// biome the land will actually be, and it is asked one warning window earlier.
    pub fn incoming_biome_for(
        &self,
        balance: &Balance,
        roll: &shift::ShiftRoll,
        first: usize,
    ) -> &'static str {
        let (from, radius) = match self.areas.get(first) {
            Some(a) => (a.biome, a.start_x),
            None => return "forest",
        };
        self.incoming_biome(balance, roll, from, radius)
    }

    /// Re-cut a shifted region's TOPOGRAPHY: its **peaks**, the authored climbable
    /// mountains that are what elevation actually is in this world (discrete terraces are
    /// retired — `[worldgen] terraces_per_area = 0` — and the rest of the relief is a
    /// continuous heightmap keyed off one per-world offset).
    ///
    /// This is the half of "the land changed shape" that renders. A peak is biome-weighted
    /// through the same `biome_terrace_mult` generation uses, so the Shift inherits the
    /// contrast for free: shifting to Ashfall raises mountains where a plain was, shifting
    /// to Desert flattens them.
    ///
    /// A peak is a **smooth walkable dome** summed onto the height field, which is exactly
    /// why it is safe to re-roll mid-run where a discontinuous height offset would not be:
    /// it cannot produce an uncrossable wall at the region's edge, and it cannot strand
    /// anyone standing on it — the slope stays climbable by construction
    /// (`height <= radius * PEAK_MAX_ASPECT`).
    ///
    /// Returns the region's new peaks per section, so the retile message can carry them.
    fn reroll_peaks(
        &mut self,
        balance: &Balance,
        first: usize,
        last: usize,
        biome: &'static str,
        seed: u64,
    ) -> Vec<(usize, Vec<[f32; 4]>)> {
        let wg = &balance.worldgen;
        let (inner, outer) = self.shift_band(first, last);
        let keep: Vec<bool> = self
            .peaks
            .iter()
            .map(|p| {
                let r = self.corridorize(&Position::new(p[0] as f64, p[1] as f64)).x;
                r < inner || r >= outer
            })
            .collect();
        let mut it = keep.into_iter();
        self.peaks.retain(|_| it.next().unwrap_or(true));
        let mult = biome_terrace_mult(biome);
        let mut out = Vec::new();
        for i in first..=last.min(self.areas.len().saturating_sub(1)) {
            let (start_x, end_x) = (self.areas[i].start_x, self.areas[i].end_x);
            let mut rng = Rng(seed ^ 0x9EA1_B055_0BEE_0001 ^ (i as u64).wrapping_mul(0x9E37_79B9));
            let mut mine = Vec::new();
            // The route's midpoint through this ring, in the frame the peaks live in — a
            // landmark is only a landmark if it is beside the road you are on.
            let anchor = self
                .path
                .iter()
                .copied()
                .find(|p| {
                    let r = self.corridorize(p).x;
                    r >= start_x && r < end_x
                })
                .filter(|_| !self.tutorial);
            if let Some(base) = anchor {
                if start_x >= wg.peak_min_distance && rng.unit() < wg.path_climb_chance * mult {
                    let radius = wg.peak_radius;
                    let height = radius * meld_proto::terrain::PEAK_MAX_ASPECT as f64 * 0.9;
                    // Off the road, so the climb is a side-trip rather than a toll gate.
                    let side = if rng.unit() < 0.5 { 1.0 } else { -1.0 };
                    let away = radius * 0.55;
                    let len = base.x.hypot(base.y).max(1.0);
                    let (nx, ny) = (-base.y / len, base.x / len);
                    let summit =
                        Position::new(base.x + nx * side * away, base.y + ny * side * away);
                    let peak =
                        [summit.x as f32, summit.y as f32, radius as f32, height as f32];
                    self.peaks.push(peak);
                    mine.push(peak);
                }
            }
            out.push((i, mine));
        }
        out
    }

    /// Re-scatter a shifted span's impassable props: the old biome's are gone and the
    /// new land's are strewn where IT would have put them — a different count, different
    /// radii, different positions. This is what makes a Shift read as the ground
    /// rearranging rather than as a recolour: a wood becoming desert genuinely thins out,
    /// because the maze-fill density is per-biome (`biome_obstacle_mult`) and is re-drawn
    /// from scratch here rather than reskinned in place.
    ///
    /// Placement happens in the CORRIDOR frame the generator works in — rejecting the
    /// clear-path tube and the trail web exactly as `push_section` does, so the route out
    /// is still feasible by construction — and each accepted prop is bent into the fan on
    /// the way into `obstacles`, which is the frame a streamed section's props already
    /// live in.
    ///
    /// Terrain elevation is deliberately NOT re-rolled. CANON §W2 retiles a region's
    /// *biome*; the topography is the ground's bones, and re-cutting terraces mid-run
    /// would drop a player through a cliff face rather than merely box them in. Props can
    /// still land on someone — [`Self::rescue_stranded`] is the answer to that.
    fn reroll_props(
        &mut self,
        balance: &Balance,
        first: usize,
        last: usize,
        biome: &'static str,
        seed: u64,
    ) -> Vec<Id> {
        let wg = &balance.worldgen;
        let (inner, outer) = self.shift_band(first, last);
        let removed: Vec<Id> = self
            .obstacles
            .iter()
            .filter(|o| {
                let r = self.corridorize(&o.position).x;
                r >= inner && r < outer
            })
            .map(|o| o.entity_id.clone())
            .collect();
        let doomed: std::collections::HashSet<&Id> = removed.iter().collect();
        self.obstacles.retain(|o| !doomed.contains(&o.entity_id));

        let (bend_half, bend_lat) = (self.radial_half, self.corridor_lateral.max(1.0));
        let bend = move |p: Position| -> Position {
            if bend_half <= 0.0 {
                return p;
            }
            let theta = (p.y / bend_lat).clamp(-1.0, 1.0) * bend_half;
            Position::new(p.x.max(0.0) * theta.cos(), p.x.max(0.0) * theta.sin())
        };
        let lat = self.corridor_lateral.max(2.0);
        let mut rng = Rng(seed ^ 0x0B57_AC1E_0000_0001);
        let scatter = obstacles_for_biome(biome);
        let fill = fill_kind_for_biome(biome);
        let mut next_id = self.obstacles.len();

        for i in first..=last.min(self.areas.len().saturating_sub(1)) {
            let (start_x, end_x) = (self.areas[i].start_x, self.areas[i].end_x);
            let length = (end_x - start_x).max(1.0);
            // The SAME rule `push_section` uses, from the one place it lives — a Shift
            // that re-scatters at a stale density is invisible until someone measures
            // the ring it landed on.
            let radial_scale =
                maze_fill_scale(wg, self.radial_half, self.lateral, start_x, end_x);
            let sparse = wg.obstacles_per_area.max(0.0).round() as usize;
            let dense = (biome_obstacle_mult(wg, biome) * wg.obstacles_per_area * radial_scale)
                .round()
                .max(0.0) as usize;

            let mut near = BlockGrid::new(2.0 * wg.obstacle_max_radius + 1.2, radial_scale, start_x, end_x);
            near.seed(&self.monsters, &self.resources, &self.chests, &self.obstacles);
            let want = sparse + dense;
            let (mut placed, mut tries) = (0usize, 0usize);
            while placed < want && tries < want * 12 {
                tries += 1;
                let ox = start_x + rng.unit() * length;
                let oy = rng.signed() * (lat - 1.0);
                let radius = wg.obstacle_min_radius
                    + rng.unit() * (wg.obstacle_max_radius - wg.obstacle_min_radius);
                let pos = Position::new(ox, oy);
                if dist_to_path(&pos, &self.corridor_path) < self.path_clear_radius + radius
                    || dist_to_web(&pos, &self.corridor_web) < self.web_clear() + radius
                {
                    continue;
                }
                if area_level_at(&self.areas, &pos) != 0 {
                    continue;
                }
                if near.blocked(&pos, radius) {
                    continue;
                }
                near.insert(pos, radius);
                let world = bend(pos);
                if self.monsters.iter().any(|m| m.position.distance_to(&world) < radius + 1.5)
                    || self.resources.iter().any(|r| r.position.distance_to(&world) < radius + 1.5)
                    || self.stations.iter().any(|s| s.position.distance_to(&world) < radius + 2.0)
                {
                    continue;
                }
                self.obstacles.push(Obstacle {
                    entity_id: format!("obs-shift-{}-{next_id}", self.areas[i].shifted_at),
                    kind: if placed < sparse {
                        scatter[rng.below(scatter.len())].to_string()
                    } else {
                        fill.to_string()
                    },
                    position: world,
                    radius,
                });
                next_id += 1;
                placed += 1;
            }
        }
        removed
    }

    /// The rescue. The new land was cut and strewn without asking who was standing there,
    /// so anyone it left somewhere they cannot be is walked back to **the start of the
    /// region** — the clear-path waypoint at its inner edge, on level 0, which is by
    /// construction open ground and by construction connected to the way home.
    ///
    /// This is the trade the re-roll buys: the ground may become whatever the new biome
    /// wants, including a cliff where you were standing, and the answer is to move the
    /// player rather than to constrain the world.
    ///
    /// Two ways to be stranded, and both are checked: a fresh prop landed on you, or the
    /// ground under you rose. The second matters more than it looks — a terrace is only
    /// leavable by a connector, and the one that would have served the plateau you are
    /// suddenly standing on was placed for a plateau that no longer exists.
    ///
    /// **The trail is safe from this**, and that is the emergent rule worth knowing: the
    /// clear-path tube holds no prop and no raised terrace by construction, so a player
    /// who stayed on the route is never moved and the Shift costs them only HP. Walking
    /// off-trail is what puts you somewhere the world can rearrange out from under you.
    ///
    /// Returns everyone moved, so the server can correct their client rather than let it
    /// fight the authoritative position for a second.
    fn rescue_stranded(&mut self, first: usize, last: usize) -> Vec<(Id, Position)> {
        let (inner, outer) = self.shift_band(first, last);
        let entry = self.region_entry(first);
        let field = self.blocking_field();
        let blocked: Vec<(Id, Position)> = self
            .avatars
            .iter()
            .filter(|a| {
                let c = self.corridorize(&a.position);
                c.x >= inner && c.x < outer && self.trapped_at(&field, &a.position)
            })
            .map(|a| (a.player_id.clone(), entry))
            .collect();
        for (pid, to) in &blocked {
            if let Some(a) = self.avatars.iter_mut().find(|a| &a.player_id == pid) {
                a.position = *to;
                a.elevation = 0;
            }
        }
        blocked
    }

    /// Is this a spot nobody can stand in — inside something impassable, or on ground
    /// that has risen since they got there?
    ///
    /// The one predicate behind both rescues, and it reads `blocking_field` so it can
    /// never disagree with what movement itself considers solid. That is the same lesson
    /// the wall bug taught: two copies of "what blocks" drift, and the drift is invisible
    /// until a player is standing in the difference.
    fn trapped_at(&self, field: &BlockField, p: &Position) -> bool {
        if area_level_at(&self.areas, &self.corridorize(p)) != 0 {
            return true;
        }
        field.blocks(p, self.player_radius)
    }

    /// The nearest place a player can definitely stand: the closest waypoint on the
    /// guaranteed clear path. Its tube holds no prop and no raised ground by
    /// construction, so this is open, level 0, and on the route home — which is what
    /// makes it a safe answer without having to search for one.
    pub fn nearest_open_ground(&self, p: &Position) -> Position {
        self.path
            .iter()
            .copied()
            .min_by(|a, b| p.distance_to(a).total_cmp(&p.distance_to(b)))
            .unwrap_or(Position::new(0.0, 0.0))
    }

    /// The general safety net: anyone standing somewhere they cannot be is walked to the
    /// nearest open ground.
    ///
    /// The Shift's own rescue is the special case — it knows which region moved and sends
    /// you to *its* entry. This is the same mechanism with no event behind it, for every
    /// other way a player can end up inside geometry: a spawn, a correction, a future
    /// buildable, or a bug nobody has found yet. Cheap enough to run on a cadence, and
    /// the alternative to running it is a player who has to close the game.
    ///
    /// Only ACTIVE avatars: someone in a battle or mid-channel is not walking anywhere,
    /// and teleporting them out of a fight would be a worse bug than the one being fixed.
    pub fn rescue_trapped(&mut self) -> Vec<(Id, Position)> {
        let field = self.blocking_field();
        let moves: Vec<(Id, Position)> = self
            .avatars
            .iter()
            .filter(|a| a.state == "active" && self.trapped_at(&field, &a.position))
            .map(|a| (a.player_id.clone(), self.nearest_open_ground(&a.position)))
            .collect();
        for (pid, to) in &moves {
            if let Some(a) = self.avatars.iter_mut().find(|a| &a.player_id == pid) {
                a.position = *to;
                a.elevation = 0;
            }
        }
        moves
    }

    /// Where "the start of the region" is: the clear-path waypoint nearest the span's
    /// inner edge, in world coords. The tube around the path holds no prop and no raised
    /// terrace by construction, so this is always open, always level 0, and always on the
    /// route the player was following anyway.
    pub fn region_entry(&self, first: usize) -> Position {
        let inner = self.areas.get(first).map(|a| a.start_x).unwrap_or(0.0);
        self.route_point_at(inner)
    }

    /// Where the ROUTE is at a given corridor distance — the clear-path waypoint closest
    /// to `corridor_x`, in world space.
    ///
    /// The fan (WG-4) bends corridor y into an ANGLE, so "distance d" is a whole ring and
    /// the route crosses it at exactly one arbitrary point on it. Anything that wants to
    /// put a player *at a depth* therefore has to ask where the route is at that depth,
    /// or it lands them somewhere the world's own path never goes — 600 to 1,800 units of
    /// arc away, measured across seeds. Both callers need the same answer (the Shift's
    /// rescue and the DEV/QA deep start), which is why it is one function.
    pub fn route_point_at(&self, corridor_x: f64) -> Position {
        self.path
            .iter()
            .copied()
            .min_by(|a, b| {
                let da = (self.corridorize(a).x - corridor_x).abs();
                let db = (self.corridorize(b).x - corridor_x).abs();
                da.total_cmp(&db)
            })
            .unwrap_or(Position::new(corridor_x, 0.0))
    }

    /// Which biome the region becomes: never the one it already is, and never one the
    /// `[biome_gate]` holds deeper than this ring. A Shift that can drop the tundra's
    /// armoured bruisers onto the d80 on-ramp is a Shift that kills new players for
    /// standing still, and the gate exists precisely to stop that on the way out.
    fn incoming_biome(
        &self,
        balance: &Balance,
        roll: &shift::ShiftRoll,
        from: &str,
        radius: f64,
    ) -> &'static str {
        let d = radius.floor() as i64;
        let allowed: Vec<&'static str> = BIOMES
            .iter()
            .copied()
            .filter(|b| *b != from)
            .filter(|b| balance.biome_gate.get(*b).copied().unwrap_or(0) <= d)
            .collect();
        let pool = if allowed.is_empty() {
            BIOMES.iter().copied().filter(|b| *b != from).collect::<Vec<_>>()
        } else {
            allowed
        };
        pool[(roll.biome_pick as usize) % pool.len()]
    }

    /// Regrowth: a persistent world's slow recovery between Shifts (`[world_persist]`).
    /// Creatures stand back up where they fell, spent nodes re-stock, opened chests
    /// re-seal — each on its own timer, none of them fast.
    ///
    /// The Shift is the headline churn, but it is a *region* on a cadence, and a world
    /// whose first three sections happen not to shift for an hour is one a player can
    /// strip permanently. That is the failure mode persistence introduces, so the fix
    /// ships with it rather than after it.
    ///
    /// Also where the world clock is stamped onto whatever was spent since the last
    /// pass: this crate stays pure and is handed `tick` rather than reading a clock.
    pub fn regrow(&mut self, balance: &Balance, tick: u64) -> usize {
        let wp = &balance.world_persist;
        let now = tick.max(1);
        let mut back = 0;
        for f in self.fallen.iter_mut().filter(|f| f.felled_tick == 0) {
            f.felled_tick = now;
        }
        for r in self.resources.iter_mut().filter(|r| r.depleted() && r.spent_tick == 0) {
            r.spent_tick = now;
        }
        for c in self.chests.iter_mut().filter(|c| c.opened && c.opened_tick == 0) {
            c.opened_tick = now;
        }
        let due: Vec<Fallen> = {
            let (ready, waiting) = self
                .fallen
                .drain(..)
                .partition(|f| now.saturating_sub(f.felled_tick) >= wp.creature_regrow_ticks);
            self.fallen = waiting;
            ready
        };
        for f in due {
            // Re-seeded off the world clock so the replacement is a different creature
            // than the one that died there, not the same wander pattern rerun.
            let seed = Rng(now).next_u64() ^ (f.home.x.to_bits() ^ f.home.y.to_bits());
            let mut fresh =
                MonsterSpawn::build(balance, f.entity_id, &f.monster_kind, f.home, seed);
            fresh.area_min_x = f.area_min_x;
            fresh.area_max_x = f.area_max_x;
            self.monsters.push(fresh);
            back += 1;
        }
        for r in self.resources.iter_mut() {
            if r.depleted() && now.saturating_sub(r.spent_tick) >= wp.node_regrow_ticks {
                let kind = r.kind.clone();
                r.remaining = node_stock(balance, &kind);
                r.spent_tick = 0;
                back += 1;
            }
        }
        for c in self.chests.iter_mut() {
            if c.opened && now.saturating_sub(c.opened_tick) >= wp.chest_regrow_ticks {
                c.opened = false;
                c.opened_tick = 0;
                back += 1;
            }
        }
        back
    }

    /// `immune` excludes players a caller has decided (via wall-clock state it holds,
    /// since this crate must stay pure) should not be pulled into a new battle right
    /// now — e.g. they just won, lost, or fled one and haven't had a moment to react.
    pub fn check_touch(&self, immune: &std::collections::HashSet<Id>) -> Option<(Id, usize)> {
        for a in self.avatars.iter().filter(|a| a.state == "active" && !immune.contains(&a.player_id)) {
            for (idx, m) in self.monsters.iter().enumerate() {
                // Skip creatures already locked in someone else's fight (`in_battle`)
                // so concurrent battles never fight over the same creature, and
                // creatures on another terrace until you climb to them.
                // A bounty mark belongs to one player: walking into someone else's
                // contract must not start their fight for them (AD-4).
                if !m.owner.is_empty() && m.owner != a.player_id {
                    continue;
                }
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

/// A per-run terrain offset that VARIES the world each seed (so no two runs grow the
/// same hills at the hub) but keeps the HUB itself on gentle, walkable ground — so spawn
/// is never buried under a cliff and area-0 generation stays feasible (a cliff at the
/// origin would box the clear path in and strand generation). Re-hashes the seed until a
/// small hub ring is routable; the base is mostly gentle so this succeeds within a few
/// tries, and falls back to the hand-tuned un-shifted field if nothing qualifies.
fn hub_terrain_offset(seed: u64) -> (f32, f32) {
    for k in 0..128u64 {
        let off = meld_proto::terrain::seed_offset(seed ^ k.wrapping_mul(0x9E37_79B9_7F4A_7C15));
        let hub_ok = meld_proto::terrain::routable(0.0, 0.0, off.0, off.1)
            && (0..8).all(|i| {
                let a = i as f32 * std::f32::consts::TAU / 8.0;
                meld_proto::terrain::routable(12.0 * a.cos(), 12.0 * a.sin(), off.0, off.1)
            });
        if hub_ok {
            return off;
        }
    }
    (0.0, 0.0)
}

/// Nudge `p` off any heightmap CLIFF to the nearest walkable ground, spiralling out.
/// Buttes are small + convex and sit in a connected walkable base, so a short search
/// always finds walkable terrain around them — this routes the clear path AROUND a
/// cliff instead of through it, keeping the world feasible under slope collision.
fn nudge_to_walkable(p: Position, off: (f32, f32)) -> Position {
    if meld_proto::terrain::walkable(p.x as f32, p.y as f32, off.0, off.1) {
        return p;
    }
    for step in 1..48 {
        let r = step as f64 * 2.0;
        for k in 0..12 {
            let a = k as f64 * std::f64::consts::TAU / 12.0;
            let q = Position::new(p.x + r * a.cos(), p.y + r * a.sin());
            if meld_proto::terrain::walkable(q.x as f32, q.y as f32, off.0, off.1) {
                return q;
            }
        }
    }
    p
}

/// Bend a corridor point (`x` = radius axis, `y` = lateral axis) into the WG-4 fan:
/// `x → radius`, `y → bearing`. The one true forward map, shared by `radialize` and
/// streaming so every bend site agrees (and `Arena::corridorize` inverts it).
/// The difficulty tier at a corridor x (its hub distance).
fn tier_at_distance(balance: &Balance, x: f64) -> i32 {
    Scaling::new(balance).tier(x.max(0.0) as i64) as i32
}

/// Place a companion `want` world-units from `anchor` in **corridor** space, so it
/// still lands that far away *after* [`radial_tf`] bends the corridor into the fan.
///
/// The bend turns corridor `y` into an ANGLE: a lateral offset of `dy` becomes an
/// arc of `r · (dy/lat) · half`, which grows with depth — a flat 3-unit offset is
/// ~50 world units out at r=500, far outside `[ai] group_radius`. Inverting that is
/// what keeps a pack a pack at any depth. `radial_half == 0` is corridor mode,
/// where world and corridor space agree.
fn corridor_offset(anchor: Position, radial: f64, tangential: f64, half: f64, lat: f64) -> Position {
    let dy = if half > 0.0 {
        let r = anchor.x.max(1.0);
        tangential * lat / (half * r)
    } else {
        tangential
    };
    Position::new(anchor.x + radial, anchor.y + dy)
}

fn radial_tf(p: Position, half: f64, lat: f64) -> Position {
    let r = p.x.max(0.0);
    let theta = (p.y / lat).clamp(-1.0, 1.0) * half;
    Position::new(r * theta.cos(), r * theta.sin())
}

/// Append the bent images of the straight corridor segment `prev → next`, inserting
/// collinear intermediate points so the bent polyline hugs the arc. The path meander
/// swings across the fan, so without this a straight world chord between two
/// far-apart-in-bearing waypoints cuts deep across the arc — off the cleared corridor
/// tube and into legitimately-placed side terraces/obstacles, which strands a
/// path-follower (and a real player steering toward the trail). Emits the intermediates
/// AND `next`; never `prev` (the caller already placed it).
fn push_bent_segment(out: &mut Vec<Position>, prev: Position, next: Position, half: f64, lat: f64) {
    // Subdivide by real ARC LENGTH (~one piece per `PIECE` world units): the tangential
    // arc r·Δθ plus the radial span dr. This scales naturally — few pieces near the hub
    // (small radius), more for a wide deep swing — and bounds the chord-to-arc sag to
    // well under the path clear radius at every depth, without exploding the point count
    // the way a fixed angular budget does across the fan.
    const PIECE: f64 = 6.0;
    let dbear = (((next.y - prev.y) / lat.max(1.0)) * half).abs();
    let r_avg = (prev.x.max(0.0) + next.x.max(0.0)) * 0.5;
    let dr = (next.x - prev.x).abs();
    let seg_len = ((r_avg * dbear).powi(2) + dr * dr).sqrt();
    let n = (seg_len / PIECE).ceil().max(1.0) as usize;
    for k in 1..=n {
        let t = k as f64 / n as f64;
        let mid = Position::new(prev.x + (next.x - prev.x) * t, prev.y + (next.y - prev.y) * t);
        out.push(radial_tf(mid, half, lat));
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



    /// **You cannot wall someone in.** A `wall` blocks movement, only its owner may take
    /// it down, and one builder may raise twelve — so "ring a player and leave them there"
    /// is the obvious grief, and the thing standing between the game and it is one
    /// inequality: `min_spacing > 2 * (structure_radius + player_radius)`.
    ///
    /// That inequality currently holds by 0.6 world units, entirely by accident. These
    /// tests make it a rule: the first proves the gap is real by *walking a player out
    /// through it* (arithmetic does not answer whether the slide actually funnels you
    /// through a 0.6-unit slot), and the second states the inequality directly so tuning
    /// `min_spacing` down — or a prop's footprint up — fails here rather than in someone's
    /// session.
    mod walling_in {
        use super::*;

        /// Ring a player with `n` walls whose adjacent centres sit exactly `chord` apart
        /// — the tightest packing placement will ever allow — and report whether they can
        /// walk out of it.
        fn can_escape(b: &Balance, chord: f64, n: usize) -> bool {
            let mut a = Arena::generate(b, 909, false);
            for _ in 0..20 {
                a.ensure_frontier(b, 700.0);
            }
            a.add_avatar("victim".into(), 5.0);
            // Open ground well away from props, so the only thing penning them in is the
            // cage — a tree closing the last gap would make this test lie.
            let centre = Position::new(420.0, 0.0);
            a.obstacles.retain(|o| o.position.distance_to(&centre) > 40.0);
            let r = chord / (2.0 * (std::f64::consts::PI / n as f64).sin());
            for k in 0..n {
                let th = std::f64::consts::TAU * (k as f64) / (n as f64);
                a.structures.push(Structure {
                    entity_id: format!("cage-{k}"),
                    function: "wall".into(),
                    owner_player_id: "griefer".into(),
                    position: Position::new(centre.x + r * th.cos(), centre.y + r * th.sin()),
                    elevation: 0,
                    hp: 100,
                    max_hp: 100,
                    placed_tick: 0,
                    build_ticks: 1,
                    ore: "dune_iron".into(),
                    ore_cost: 1,
                });
            }
            // Push out along every bearing; a cage is only a cage if EVERY one fails.
            for k in 0..180 {
                a.avatar_mut("victim").unwrap().position = centre;
                let th = std::f64::consts::TAU * (k as f64) / 180.0;
                let (dx, dy) = (th.cos(), th.sin());
                for _ in 0..400 {
                    a.apply_move("victim", dx, dy, 1);
                }
                if a.avatar("victim").unwrap().position.distance_to(&centre) > r + 8.0 {
                    return true;
                }
            }
            false
        }

        /// The regression, named. A `wall` is defined by `blocks: true` and shipped
        /// stopping CREATURES only — the collision line went into `step_creatures_with_aggro`
        /// and never into `apply_move`, so players walked through walls for a release.
        #[test]
        fn a_wall_stops_a_player_walking_into_it() {
            let b = Balance::load_default().unwrap();
            let mut a = Arena::generate(&b, 909, false);
            for _ in 0..20 {
                a.ensure_frontier(&b, 700.0);
            }
            a.add_avatar("p1".into(), 5.0);
            let here = Position::new(420.0, 0.0);
            a.obstacles.retain(|o| o.position.distance_to(&here) > 40.0);
            a.avatar_mut("p1").unwrap().position = here;
            a.structures.push(Structure {
                entity_id: "wall-0".into(),
                function: "wall".into(),
                owner_player_id: "someone".into(),
                position: Position::new(here.x + 10.0, here.y),
                elevation: 0,
                hp: 100,
                max_hp: 100,
                placed_tick: 0,
                build_ticks: 1,
                ore: "dune_iron".into(),
                ore_cost: 1,
            });
            for _ in 0..200 {
                a.apply_move("p1", 1.0, 0.0, 1);
            }
            let stopped = a.avatar("p1").unwrap().position;
            assert!(
                stopped.distance_to(&Position::new(here.x + 10.0, here.y))
                    >= a.structure_footprint(),
                "the player walked through a wall and ended up at {stopped:?}"
            );
            assert!(stopped.x > here.x, "the player never set off");
        }

        #[test]
        fn a_ring_of_walls_always_leaves_a_way_out() {
            let b = Balance::load_default().unwrap();
            assert!(
                can_escape(&b, b.building.min_spacing, b.building.max_per_player),
                "a player was sealed in by {} walls at the shipped spacing — and nobody can \
                 take them down but the griefer who put them there",
                b.building.max_per_player
            );
        }

        /// The test above is only worth anything if it can fail. Close the gap and the
        /// cage becomes real — which is exactly what a tuning pass that lowered
        /// `min_spacing` would ship.
        #[test]
        fn the_cage_is_real_once_the_gap_closes() {
            let b = Balance::load_default().unwrap();
            let a = Arena::generate(&b, 1, false);
            let too_tight = 2.0 * (a.structure_footprint() + b.worldgen.player_radius) - 0.2;
            assert!(
                !can_escape(&b, too_tight, 12),
                "walls {too_tight} apart still let a player out, so the escape test proves \
                 nothing about the spacing"
            );
        }

        /// The inequality itself, so a tuning pass cannot quietly close the gap. Stated
        /// rather than measured: the walking test above proves the slide gets you through
        /// today's slot, but this is the reason a slot exists at all.
        #[test]
        fn wall_spacing_leaves_a_gap_wider_than_a_player() {
            let b = Balance::load_default().unwrap();
            let a = Arena::generate(&b, 1, false);
            let blocked_at = a.structure_footprint() + b.worldgen.player_radius;
            assert!(
                b.building.min_spacing > 2.0 * blocked_at,
                "two walls at min_spacing {} leave no gap for a {}-radius player: they are \
                 impassable from {blocked_at} out, so a ring of them is a cage",
                b.building.min_spacing,
                b.worldgen.player_radius,
            );
        }
    }

    /// One primitive, many functions (CANON D21/§W3): the lifecycle is shared, and only
    /// the numbers and the two flags differ. These hold the *loop* — plant, hold, pay,
    /// lose — rather than today's magnitudes.
    mod structures {
        use super::*;

        fn built() -> (Balance, Arena) {
            let b = Balance::load_default().unwrap();
            let mut a = Arena::generate(&b, 4242, false);
            for _ in 0..30 {
                a.ensure_frontier(&b, 900.0);
            }
            a.add_avatar("p1".into(), 5.0);
            (b, a)
        }

        /// Stand somewhere a structure can legally go, at roughly `radius` out. The world
        /// is a maze — most spots are on the trail, in a tree or inside another build — so
        /// a test that guesses one is a test that fails for the wrong reason.
        fn stand_somewhere_legal(b: &Balance, a: &mut Arena, radius: f64) -> bool {
            let lat = a.corridor_lateral();
            for k in 0..400 {
                let frac = -0.9 + 1.8 * (k as f64 / 400.0);
                let p = Position::new(radius, lat * frac);
                a.avatar_mut("p1").unwrap().position = bend_for_test(a, p);
                if a.place_structure(b, "p1", "wall", "probe", 0).is_ok() {
                    let id = a.structures.last().unwrap().entity_id.clone();
                    a.demolish_structure(b, &id);
                    return true;
                }
            }
            false
        }

        /// Corridor → world, so a corridor-space probe lands where the arena expects it.
        fn bend_for_test(a: &Arena, p: Position) -> Position {
            let half = a.radial_half();
            if half <= 0.0 {
                return p;
            }
            let theta = (p.y / a.corridor_lateral().max(1.0)).clamp(-1.0, 1.0) * half;
            Position::new(p.x.max(0.0) * theta.cos(), p.x.max(0.0) * theta.sin())
        }

        #[test]
        fn a_structure_goes_up_weak_and_ramps_to_full() {
            let (b, mut a) = built();
            assert!(stand_somewhere_legal(&b, &mut a, 200.0), "no legal ground at d200");
            let id = a
                .place_structure(&b, "p1", "anchor", "dune_iron", 100)
                .map(|s| s.entity_id.clone())
                .expect("placeable off the trail");
            let (hp0, max_hp, build) = {
                let s = a.structures.iter().find(|s| s.entity_id == id).unwrap();
                (s.hp, s.max_hp, s.build_ticks)
            };
            assert!(hp0 < max_hp, "it went up already finished");
            assert!(a.structures[0].building(100), "not under construction on the tick it was placed");

            a.advance_builds(100 + build / 2);
            let mid = a.structures.iter().find(|s| s.entity_id == id).unwrap().hp;
            assert!(mid > hp0 && mid < max_hp, "the ramp did not ramp: {hp0} -> {mid}");

            a.advance_builds(100 + build);
            let done = a.structures.iter().find(|s| s.entity_id == id).unwrap();
            assert_eq!(done.hp, max_hp);
            assert!(!done.building(100 + build), "still building past its own build time");
        }

        /// A besieged wall must not mend itself just because it is still going up.
        #[test]
        fn construction_never_heals_damage_it_took_mid_build() {
            let (b, mut a) = built();
            assert!(stand_somewhere_legal(&b, &mut a, 200.0), "no legal ground at d200");
            a.place_structure(&b, "p1", "wall", "dune_iron", 100).unwrap();
            let build = a.structures[0].build_ticks;
            a.advance_builds(100 + build);
            a.structures[0].hp = 5;
            a.advance_builds(100 + build + 1);
            assert_eq!(a.structures[0].hp, 5, "the build ramp healed a damaged structure");
        }

        /// The route out is feasible by construction everywhere else in this world; a
        /// player-built wall across it would be the one thing that could seal the exit,
        /// and it would do it on purpose.
        #[test]
        fn nothing_may_be_built_on_the_clear_path() {
            let (b, mut a) = built();
            let on_trail = *a.path.get(4).expect("a waypoint out there");
            a.avatar_mut("p1").unwrap().position = on_trail;
            assert_eq!(
                a.place_structure(&b, "p1", "wall", "dune_iron", 100).err(),
                Some(PlaceRefusal::OnTheTrail)
            );
        }

        #[test]
        fn a_builder_is_capped_and_cannot_stack_structures() {
            let (b, mut a) = built();
            assert!(stand_somewhere_legal(&b, &mut a, 200.0), "no legal ground at d200");
            a.place_structure(&b, "p1", "wall", "dune_iron", 100).unwrap();
            assert_eq!(
                a.place_structure(&b, "p1", "wall", "dune_iron", 100).err(),
                Some(PlaceRefusal::TooClose),
                "two structures went up on the same spot"
            );
            let lat = a.corridor_lateral();
            for k in 0..b.building.max_per_player + 4 {
                a.avatar_mut("p1").unwrap().position =
                    Position::new(200.0 + (k as f64) * b.building.min_spacing * 2.0, lat * 0.5);
                let _ = a.place_structure(&b, "p1", "wall", "dune_iron", 100);
            }
            let held = a.structures.iter().filter(|s| s.owner_player_id == "p1").count();
            assert!(
                held <= b.building.max_per_player,
                "{held} structures past a cap of {}",
                b.building.max_per_player
            );
            // And the cap is what stops it, not the terrain running out of room.
            while a.structures.iter().filter(|s| s.owner_player_id == "p1").count()
                < b.building.max_per_player
            {
                let n = a.structures.len();
                a.structures.push(Structure {
                    entity_id: format!("filler-{n}"),
                    function: "wall".into(),
                    owner_player_id: "p1".into(),
                    position: Position::new(-9000.0 - n as f64 * 50.0, 0.0),
                    elevation: 0,
                    hp: 1,
                    max_hp: 1,
                    placed_tick: 0,
                    build_ticks: 1,
                    ore: "dune_iron".into(),
                    ore_cost: 1,
                });
            }
            let lat2 = a.corridor_lateral();
            a.avatar_mut("p1").unwrap().position =
                bend_for_test(&a, Position::new(640.0, lat2 * 0.4));
            assert_eq!(
                a.place_structure(&b, "p1", "wall", "dune_iron", 100).err(),
                Some(PlaceRefusal::AtYourLimit)
            );
        }

        #[test]
        fn packing_one_down_hands_back_part_of_what_it_cost() {
            let (b, mut a) = built();
            assert!(stand_somewhere_legal(&b, &mut a, 200.0), "no legal ground at d200");
            let id = a
                .place_structure(&b, "p1", "anchor", "dune_iron", 100)
                .map(|s| s.entity_id.clone())
                .unwrap();
            let (ore, back) = a.demolish_structure(&b, &id).expect("it was standing");
            assert_eq!(ore, "dune_iron", "it handed back something it was not built from");
            assert!(back > 0 && back < b.building.anchor_ore_cost, "a full refund makes moving free");
            assert!(a.structures.is_empty());
        }

        #[test]
        fn repair_stops_at_full_and_costs_nothing_extra() {
            let (b, mut a) = built();
            assert!(stand_somewhere_legal(&b, &mut a, 200.0), "no legal ground at d200");
            a.place_structure(&b, "p1", "wall", "dune_iron", 100).unwrap();
            let id = a.structures[0].entity_id.clone();
            a.structures[0].hp = 1;
            assert!(a.repair_structure(&b, &id).unwrap() > 0);
            a.structures[0].hp = a.structures[0].max_hp;
            assert_eq!(a.repair_structure(&b, &id), None, "a sound structure charged for a mend");
        }


        /// The spacing invariant is a promise about GEOMETRY — a ring of walls has a gap.
        /// It says nothing about penning somebody in one block at a time while they stand
        /// still, and a player cannot demolish what someone else built around them.
        #[test]
        fn you_cannot_drop_a_wall_on_another_player() {
            let (b, mut a) = built();
            assert!(stand_somewhere_legal(&b, &mut a, 200.0), "no legal ground at d200");
            let spot = a.avatar("p1").unwrap().position;
            a.add_avatar("victim".into(), 5.0);
            a.avatar_mut("victim").unwrap().position = spot;
            assert_eq!(
                a.place_structure(&b, "p1", "wall", "dune_iron", 100).err(),
                Some(PlaceRefusal::SomeoneStanding)
            );
        }

        /// An anchor does not block, so it cannot pen anyone — gating it would only stop
        /// a party holding ground together, which is the verb the epic is for.
        #[test]
        fn an_anchor_may_go_up_beside_a_teammate() {
            let (b, mut a) = built();
            assert!(stand_somewhere_legal(&b, &mut a, 200.0), "no legal ground at d200");
            let spot = a.avatar("p1").unwrap().position;
            a.add_avatar("mate".into(), 5.0);
            a.avatar_mut("mate").unwrap().position = spot;
            assert!(
                a.place_structure(&b, "p1", "anchor", "dune_iron", 100).is_ok(),
                "a teammate standing there stopped an anchor, which cannot trap anyone"
            );
        }

        /// The net under everything else. However a player ends up inside geometry — a
        /// spawn, a correction, a buildable nobody has written yet — the world walks them
        /// out instead of leaving them to close the game.
        #[test]
        fn anyone_standing_inside_something_is_walked_to_open_ground() {
            let (b, mut a) = built();
            let stuck = a
                .obstacles
                .iter()
                .find(|o| o.position.x.hypot(o.position.y) > 120.0)
                .map(|o| o.position)
                .expect("a prop out there");
            a.avatar_mut("p1").unwrap().position = stuck;

            let moved = a.rescue_trapped();
            assert_eq!(moved.len(), 1, "the player inside a tree was left there");
            let now = a.avatar("p1").unwrap().position;
            assert_ne!(now, stuck);
            assert_eq!(a.avatar("p1").unwrap().elevation, 0);
            // And where they landed is somewhere they can actually stand.
            let field = a.blocking_field_for_test();
            assert!(
                !field.blocks(&now, b.worldgen.player_radius),
                "the rescue put them inside something else"
            );
        }

        /// A rescue that fires on someone standing in the open would teleport players at
        /// random, which is a worse bug than the one it is for.
        #[test]
        fn nobody_standing_in_the_open_is_moved() {
            let (b, mut a) = built();
            assert!(stand_somewhere_legal(&b, &mut a, 200.0), "no legal ground at d200");
            let spot = a.avatar("p1").unwrap().position;
            assert!(a.rescue_trapped().is_empty(), "a player in the open was teleported");
            assert_eq!(a.avatar("p1").unwrap().position, spot);
        }

        /// Nobody gets yanked out of a fight by the safety net.
        #[test]
        fn a_player_in_a_battle_is_left_alone() {
            let (_b, mut a) = built();
            let stuck = a
                .obstacles
                .iter()
                .find(|o| o.position.x.hypot(o.position.y) > 120.0)
                .map(|o| o.position)
                .expect("a prop out there");
            {
                let av = a.avatar_mut("p1").unwrap();
                av.position = stuck;
                av.state = "in_battle".into();
            }
            assert!(a.rescue_trapped().is_empty(), "the net pulled someone out of a battle");
            assert_eq!(a.avatar("p1").unwrap().position, stuck);
        }

        // ---- BD-3: the anchor holds, and holding costs ----

        /// The headline loop. An anchor stops the Shift its region was due, and the land
        /// takes the difference out of the anchor.
        #[test]
        fn an_anchor_holds_its_region_and_pays_for_it() {
            let (b, mut a) = built();
            let roll = shift::roll(&b, a.seed, 0);
            let (first, last) = a.shift_region(&b, &roll).expect("a shiftable region");
            let (inner, outer) = a.shift_band(first, last);
            assert!(
                stand_somewhere_legal(&b, &mut a, (inner + outer) * 0.5),
                "no legal ground inside the doomed ring"
            );
            a.place_structure(&b, "p1", "anchor", "dune_iron", 0).unwrap();
            a.advance_builds(a.structures[0].build_ticks);
            let full = a.structures[0].hp;
            assert!(a.section_pinned(&b, first), "the anchor does not hold its own section");

            let biomes: Vec<&str> = a.areas.iter().map(|s| s.biome).collect();
            let held = a.hold_shift(&b, first, last).expect("the anchor should have held it");
            assert_eq!(held.anchors.len(), 1);
            assert_eq!(
                biomes,
                a.areas.iter().map(|s| s.biome).collect::<Vec<_>>(),
                "a held Shift retiled anyway"
            );
            assert!(a.structures[0].hp < full, "holding cost the anchor nothing");
        }

        /// An anchor nobody maintains falls on its own, and the ground goes back to the
        /// Shift. Without this, `BD-3` would be "plant one, never think about this region
        /// again" — the opposite of the loop it is named for.
        #[test]
        fn an_unmaintained_anchor_eventually_falls_and_gives_the_ground_back() {
            let (b, mut a) = built();
            let roll = shift::roll(&b, a.seed, 0);
            let (first, last) = a.shift_region(&b, &roll).expect("a shiftable region");
            let (inner, outer) = a.shift_band(first, last);
            assert!(
                stand_somewhere_legal(&b, &mut a, (inner + outer) * 0.5),
                "no legal ground inside the doomed ring"
            );
            a.place_structure(&b, "p1", "anchor", "dune_iron", 0).unwrap();
            a.advance_builds(a.structures[0].build_ticks);

            let mut holds = 0;
            while a.hold_shift(&b, first, last).is_some() {
                holds += 1;
                assert!(holds < 100, "the anchor is holding forever");
            }
            assert!(holds >= 2, "an anchor that folds on the first Shift is not worth planting");
            assert!(a.structures.is_empty(), "a 0-HP anchor is still standing");
            assert!(!a.section_pinned(&b, first), "the ground is still held by a dead anchor");
        }

        /// A wall is a wall. If `pins` did not distinguish the functions, the registry's
        /// one claim would be untested.
        #[test]
        fn a_wall_does_not_hold_ground() {
            let (b, mut a) = built();
            let roll = shift::roll(&b, a.seed, 0);
            let (first, last) = a.shift_region(&b, &roll).expect("a shiftable region");
            let (inner, outer) = a.shift_band(first, last);
            assert!(
                stand_somewhere_legal(&b, &mut a, (inner + outer) * 0.5),
                "no legal ground inside the doomed ring"
            );
            a.place_structure(&b, "p1", "wall", "dune_iron", 0).unwrap();
            a.advance_builds(a.structures[0].build_ticks);
            assert!(!a.section_pinned(&b, first), "a wall held ground");
            assert!(a.hold_shift(&b, first, last).is_none(), "a wall stopped the Shift");
        }
    }

    /// A pack in FORMATION (CR/encounter composition). The back rank is the same trade a
    /// hero's back row is — half the physical damage taken, half the physical damage dealt
    /// — so a pack's rear shrugs off a sword and is answered by a spell, a brand or reach.
    mod formations {
        use super::*;

        fn packs(seed: u64) -> (Balance, Arena) {
            let b = Balance::load_default().unwrap();
            let mut a = Arena::generate(&b, seed, false);
            for _ in 0..40 {
                a.ensure_frontier(&b, 1400.0);
            }
            (b, a)
        }

        /// Every pack that is big enough to HAVE a formation has one, and the leader is
        /// never hiding behind its own minions.
        #[test]
        fn a_pack_forms_two_ranks_and_its_leader_holds_the_front() {
            let (b, a) = packs(4242);
            let mut ranked = 0;
            for lead in a.monsters.iter().filter(|m| m.encounter_class == "leader") {
                let pack: Vec<&MonsterSpawn> = a
                    .monsters
                    .iter()
                    .filter(|m| {
                        m.position.distance_to(&lead.position) <= b.ai.group_radius
                            && m.encounter_class != "leader"
                    })
                    .collect();
                assert!(!lead.back_row, "a leader was hiding in its own back rank");
                if pack.len() + 1 >= 3 {
                    ranked += 1;
                    assert!(
                        pack.iter().any(|m| m.back_row),
                        "a pack of {} has no back rank",
                        pack.len() + 1
                    );
                    assert!(
                        pack.iter().any(|m| !m.back_row) || pack.len() == 1,
                        "a pack of {} has no front rank but its leader",
                        pack.len() + 1
                    );
                }
            }
            assert!(ranked > 0, "no pack in this world was big enough to test");
        }

        /// A lone creature must never stand in a back rank: it would be half-immune to
        /// every sword in the game, for free, with no formation to justify it.
        #[test]
        fn a_creature_fighting_alone_is_always_in_front() {
            let (b, a) = packs(909);
            for m in a.monsters.iter() {
                let company = a
                    .monsters
                    .iter()
                    .filter(|o| {
                        o.entity_id != m.entity_id
                            && o.position.distance_to(&m.position) <= b.ai.group_radius
                    })
                    .count();
                if company == 0 {
                    assert!(!m.back_row, "{} stands alone in a back rank", m.entity_id);
                }
            }
        }

        /// The rank has to be worth something: the engine's own trade is what gives the
        /// rear its meaning, and a build where either half is 1.0 has no formation at all.
        #[test]
        fn the_back_rank_is_a_trade_in_both_directions() {
            let b = Balance::load_default().unwrap();
            assert!(
                b.battle.back_row_damage_mult < 1.0,
                "a back rank that takes full physical damage is not a back rank"
            );
            assert!(
                b.battle.back_row_attack_mult < 1.0,
                "a back rank that deals full physical damage is a free hiding place"
            );
        }
    }

    /// A world that shifts is a world that stays worth walking through — the whole
    /// reason it can afford to be persistent. Every one of these holds a property of
    /// the mechanic rather than of today's numbers.
    mod shifting_lands {
        use super::*;

        fn world() -> (Balance, Arena) {
            let b = Balance::load_default().unwrap();
            let mut a = Arena::generate(&b, 4242, false);
            for _ in 0..30 {
                a.ensure_frontier(&b, 900.0);
            }
            (b, a)
        }

        /// A Shift that can drop the tundra's armoured bruisers onto the on-ramp kills
        /// new players for standing still. The `[biome_gate]` holds the harsh themes
        /// outward on the way OUT; it has to hold them there on the way SIDEWAYS too.
        #[test]
        fn a_shift_never_lands_a_biome_the_gate_holds_deeper() {
            let (b, mut a) = world();
            for g in 0..60u64 {
                let roll = shift::roll(&b, a.seed, g);
                let Some((first, last)) = a.shift_region(&b, &roll) else { continue };
                let radius = a.areas[first].start_x.floor() as i64;
                let out = a.apply_shift(&b, &roll, first, last);
                let gate = b.biome_gate.get(&out.biome).copied().unwrap_or(0);
                assert!(
                    gate <= radius,
                    "gen {g} put {} at d{radius}, gated to d{gate}",
                    out.biome
                );
            }
        }

        #[test]
        fn a_shift_never_lands_on_the_doorstep() {
            let (b, a) = world();
            for g in 0..80u64 {
                let roll = shift::roll(&b, a.seed, g);
                let Some((first, _)) = a.shift_region(&b, &roll) else { continue };
                assert!(
                    a.areas[first].start_x >= b.shift.safe_radius,
                    "gen {g} shifted the hub ring at {}",
                    a.areas[first].start_x
                );
            }
        }

        /// The land REARRANGES — the whole point of re-scattering rather than reskinning.
        /// Terrain and the clear path stay put (topography is the ground's bones, and the
        /// route out must remain feasible by construction), but the props are somewhere
        /// else entirely.
        #[test]
        fn a_shift_rearranges_the_ground_it_does_not_recolour_it() {
            let (b, mut a) = world();
            let roll = shift::roll(&b, a.seed, 0);
            let (first, last) = a.shift_region(&b, &roll).expect("a shiftable region");
            let (inner, outer) = a.shift_band(first, last);
            let in_band = |arena: &Arena, p: &Position| {
                let r = arena.corridorize(p).x;
                r >= inner && r < outer
            };
            let before: Vec<(Position, f64)> = a
                .obstacles
                .iter()
                .filter(|o| in_band(&a, &o.position))
                .map(|o| (o.position, o.radius))
                .collect();
            let levels: Vec<Vec<u8>> = a.areas.iter().map(|s| s.terrain.level.clone()).collect();
            let path = a.path.clone();

            a.apply_shift(&b, &roll, first, last);

            let after: Vec<(Position, f64)> = a
                .obstacles
                .iter()
                .filter(|o| in_band(&a, &o.position))
                .map(|o| (o.position, o.radius))
                .collect();
            assert!(!before.is_empty() && !after.is_empty());
            assert_ne!(before, after, "the props were reskinned, not re-scattered");
            assert_eq!(levels, a.areas.iter().map(|s| s.terrain.level.clone()).collect::<Vec<_>>());
            assert_eq!(path, a.path, "the Shift re-routed the clear path");
        }

        /// Elevation in this world is the continuous heightmap plus **peaks** — discrete
        /// terraces are retired — so peaks are what "the land changed shape" has to mean.
        /// A biome carries its own mountain weighting, so a Shift inherits the contrast:
        /// Ashfall raises ranges where Desert would flatten them.
        #[test]
        fn a_shift_re_cuts_the_regions_mountains() {
            let (b, mut a) = world();
            let mut changed = 0;
            for g in 0..30u64 {
                let roll = shift::roll(&b, a.seed, g);
                let Some((first, last)) = a.shift_region(&b, &roll) else { continue };
                let (inner, outer) = a.shift_band(first, last);
                let before: Vec<[f32; 4]> = a
                    .peaks
                    .iter()
                    .filter(|p| {
                        let r = a.corridorize(&Position::new(p[0] as f64, p[1] as f64)).x;
                        r >= inner && r < outer
                    })
                    .copied()
                    .collect();
                let out = a.apply_shift(&b, &roll, first, last);
                let after: Vec<[f32; 4]> = a
                    .peaks
                    .iter()
                    .filter(|p| {
                        let r = a.corridorize(&Position::new(p[0] as f64, p[1] as f64)).x;
                        r >= inner && r < outer
                    })
                    .copied()
                    .collect();
                assert_eq!(
                    after.len(),
                    out.peaks.iter().map(|(_, p)| p.len()).sum::<usize>(),
                    "the reported peaks are not the peaks that stand there"
                );
                if before != after {
                    changed += 1;
                }
            }
            assert!(changed > 0, "30 Shifts never re-cut a single mountain");
        }

        /// A dome must stay climbable, or a Shift can raise a wall nobody gets over — the
        /// exact soft-lock that rules out re-rolling the height field itself.
        #[test]
        fn a_re_cut_mountain_is_still_walkable() {
            let (b, mut a) = world();
            for g in 0..30u64 {
                let roll = shift::roll(&b, a.seed, g);
                let Some((first, last)) = a.shift_region(&b, &roll) else { continue };
                a.apply_shift(&b, &roll, first, last);
            }
            for p in &a.peaks {
                let (radius, height) = (p[2], p[3]);
                assert!(radius > 0.0, "a peak with no footprint is a spike");
                assert!(
                    height <= radius * meld_proto::terrain::PEAK_MAX_ASPECT,
                    "a {height}-high peak over a {radius} radius is a cliff, not a climb"
                );
            }
        }

        /// Feasibility is the invariant a re-roll could break, and it is kept the same way
        /// generation keeps it: nothing is ever placed inside the clear-path tube.
        #[test]
        fn the_way_out_survives_every_shift() {
            let (b, mut a) = world();
            for g in 0..25u64 {
                let roll = shift::roll(&b, a.seed, g);
                let Some((first, last)) = a.shift_region(&b, &roll) else { continue };
                a.apply_shift(&b, &roll, first, last);
            }
            for o in &a.obstacles {
                let c = a.corridorize(&o.position);
                assert!(
                    dist_to_path(&c, &a.corridor_path) >= a.path_clear_radius,
                    "a shifted prop landed in the clear-path tube at {c:?}"
                );
            }
        }

        /// The trade the re-roll buys: props land where the new biome wants them, and a
        /// player they land on is walked back to the region's entry rather than the world
        /// being constrained to avoid them.
        #[test]
        fn a_player_the_new_land_lands_on_is_walked_to_the_regions_entry() {
            let (b, mut a) = world();
            let roll = shift::roll(&b, a.seed, 0);
            let (first, last) = a.shift_region(&b, &roll).expect("a shiftable region");
            let (inner, outer) = a.shift_band(first, last);
            a.add_avatar("p1".into(), 5.0);
            // Park them well off-trail, mid-region, where the scatter is free to build.
            let mid = (inner + outer) * 0.5;
            a.avatar_mut("p1").unwrap().position =
                Position::new(mid, a.corridor_lateral() * 0.6);
            let entry = a.region_entry(first);

            let out = a.apply_shift(&b, &roll, first, last);
            let here = a.avatar("p1").unwrap().position;
            if out.moved.iter().any(|(p, _)| p == "p1") {
                assert_eq!(here, entry, "a rescued player did not land at the entry");
            }
            // Whatever happened, they are standing somewhere they can stand.
            assert!(
                !a.obstacles
                    .iter()
                    .any(|o| here.distance_to(&o.position) < o.radius),
                "a player was left inside a prop"
            );
        }

        #[test]
        fn what_grows_back_belongs_to_the_new_biome() {
            let (b, mut a) = world();
            let roll = shift::roll(&b, a.seed, 0);
            let (first, last) = a.shift_region(&b, &roll).expect("a shiftable region");
            let (inner, outer) = a.shift_band(first, last);
            let out = a.apply_shift(&b, &roll, first, last);
            let native = creatures_for_biome(&out.biome);
            let nodes = resources_for_biome(&out.biome);
            let in_band = |p: &Position| {
                let r = a.corridorize(p).x;
                r >= inner && r < outer
            };
            for m in a.monsters.iter().filter(|m| in_band(&m.position) && m.bounty.is_empty()) {
                assert!(
                    native.contains(&m.monster_kind.as_str()),
                    "{} is not a {} creature",
                    m.monster_kind,
                    out.biome
                );
            }
            for n in a.resources.iter().filter(|n| in_band(&n.position)) {
                assert!(nodes.contains(&n.kind.as_str()), "{} is not {} stock", n.kind, out.biome);
                assert!(!n.depleted(), "the new land grew an already-empty node");
            }
            assert!(!out.wiped.is_empty(), "a Shift that wiped nothing is not a Shift");
        }

        /// A contract with your name on it must not evaporate because the weather turned.
        #[test]
        fn a_shift_does_not_take_a_bounty_mark() {
            let (b, mut a) = world();
            let roll = shift::roll(&b, a.seed, 0);
            let (first, last) = a.shift_region(&b, &roll).expect("a shiftable region");
            let (inner, _) = a.shift_band(first, last);
            let victim = a
                .monsters
                .iter_mut()
                .find(|m| m.position.x.hypot(m.position.y) >= inner)
                .expect("a creature out there");
            victim.bounty = "contract-1".into();
            let kept = victim.entity_id.clone();
            let out = a.apply_shift(&b, &roll, first, last);
            assert!(!out.wiped.contains(&kept), "the Shift took a standing contract");
            assert!(a.monsters.iter().any(|m| m.entity_id == kept && m.bounty == "contract-1"));
        }

        /// The failure mode persistence introduces: a world nobody ever refreshes is a
        /// world a player strips permanently. Regrowth is the floor under that.
        #[test]
        fn a_persistent_world_grows_back_what_was_taken() {
            let (b, mut a) = world();
            let doomed: Vec<String> =
                a.monsters.iter().take(5).map(|m| m.entity_id.clone()).collect();
            for m in a.monsters.iter_mut().take(5) {
                m.defeated = true;
            }
            for n in a.resources.iter_mut().take(3) {
                n.remaining = 0;
            }
            a.prune_defeated();
            assert!(a.monsters.iter().all(|m| !doomed.contains(&m.entity_id)));

            a.regrow(&b, 1);
            assert!(
                a.monsters.iter().all(|m| !doomed.contains(&m.entity_id)),
                "the ground was back on the very next tick"
            );
            a.regrow(&b, 1 + b.world_persist.creature_regrow_ticks);
            for id in &doomed {
                assert!(a.monsters.iter().any(|m| &m.entity_id == id), "{id} never came back");
            }
            a.regrow(&b, 1 + b.world_persist.node_regrow_ticks);
            assert!(a.resources.iter().all(|n| !n.depleted()), "a node never re-stocked");
        }

        /// Half the picks are least-recently-disturbed, so the churn cannot pool in one
        /// ring while the rest of the world becomes a museum.
        #[test]
        fn the_churn_spreads_rather_than_pooling() {
            let (b, mut a) = world();
            let mut touched = std::collections::HashSet::new();
            for g in 0..40u64 {
                let roll = shift::roll(&b, a.seed, g);
                let Some((first, last)) = a.shift_region(&b, &roll) else { continue };
                let out = a.apply_shift(&b, &roll, first, last);
                touched.extend(out.sections);
            }
            let shiftable =
                a.areas.iter().filter(|s| s.start_x >= b.shift.safe_radius).count();
            assert!(
                touched.len() * 2 >= shiftable,
                "40 Shifts reached {} of {shiftable} shiftable sections",
                touched.len()
            );
        }
    }

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

    /// THE END FIGHT (EW, first cut): past `end_fight_min_distance` one encounter becomes
    /// three named bosses standing together — and exactly one, once, because it is the thing
    /// the walk out is pointed at rather than a spawn type that recurs.
    #[test]
    fn the_end_fight_is_three_bosses_placed_once() {
        let b = Balance::load_default().unwrap();
        let floor = b.encounters.end_fight_min_distance;
        // Stream far enough out that the section past the floor exists.
        let mut found = None;
        for seed in 0..6u64 {
            let mut arena = Arena::generate(&b, seed, false);
            for _ in 0..80 {
                arena.ensure_frontier(&b, floor + 900.0);
            }
            let enders: Vec<&MonsterSpawn> =
                arena.monsters.iter().filter(|m| m.encounter_class == "world_end").collect();
            if !enders.is_empty() {
                assert_eq!(
                    enders.len(),
                    b.encounters.end_fight_bosses,
                    "seed {seed}: the end fight placed {} bosses",
                    enders.len()
                );
                // Measured as DISTANCE FROM THE ORIGIN, not `position.x`: the placement gate
                // is in corridor space (where x is the radius) while a stored position is
                // world space after the radial bend, so `position.x` is `r * cos(theta)` and
                // is legitimately smaller. Comparing the two frames is the WG-4 trap.
                //
                // The encounter is past the floor; its three peers then scatter around the
                // leader by `pack_spread` like any group. What matters is that they stand
                // together — touching one pulls all three (`group_around`).
                let dist = |m: &&MonsterSpawn| m.position.distance_floor() as f64;
                let deepest = enders.iter().map(dist).fold(f64::MIN, f64::max);
                let shallowest = enders.iter().map(dist).fold(f64::MAX, f64::min);
                assert!(deepest >= floor, "the end fight was placed at {deepest}, short of {floor}");
                assert!(
                    deepest - shallowest <= b.encounters.pack_spread * 2.0,
                    "the three bosses are {} apart — that is not one encounter",
                    deepest - shallowest
                );
                for m in &enders {
                    assert!(!m.boss_kind.is_empty(), "an end-fight boss has no identity");
                }
                // Three DIFFERENT names: the same boss three times reads as a bug.
                let names: std::collections::HashSet<&str> =
                    enders.iter().map(|m| m.boss_kind.as_str()).collect();
                assert!(names.len() > 1, "the end fight is the same boss repeated: {names:?}");
                found = Some(enders.len());
                break;
            }
        }
        assert!(found.is_some(), "no seed placed the end fight past its floor at all");
    }

    /// The `MELD_END_FIGHT` harness moves the fight to the hub by lowering its floor, so a
    /// tuning pass can be WATCHED instead of modelled. This pins that the override actually
    /// places it — and close enough to spawn to walk to, which is the whole point.
    #[test]
    fn lowering_the_floor_brings_the_end_fight_to_the_hub() {
        let mut b = Balance::load_default().unwrap();
        b.encounters.end_fight_min_distance = 30.0;
        let mut placed = 0;
        for seed in 0..8u64 {
            let mut arena = Arena::generate(&b, seed, false);
            arena.ensure_frontier(&b, 200.0);
            let enders: Vec<&MonsterSpawn> =
                arena.monsters.iter().filter(|m| m.encounter_class == "world_end").collect();
            if enders.len() == b.encounters.end_fight_bosses {
                let deepest = enders
                    .iter()
                    .map(|m| m.position.distance_floor())
                    .max()
                    .unwrap_or(0);
                assert!(
                    (30..250).contains(&deepest),
                    "seed {seed}: the harness put the end fight at {deepest} — not a walk"
                );
                placed += 1;
            }
        }
        assert!(placed > 0, "lowering the floor never placed the end fight at all");
    }

    /// The tutorial dive is an on-ramp and must never contain it, however far a first-time
    /// player somehow walks.
    #[test]
    fn the_tutorial_never_holds_the_end_fight() {
        let b = Balance::load_default().unwrap();
        let mut arena = Arena::generate(&b, 3, true);
        for _ in 0..80 {
            arena.ensure_frontier(&b, b.encounters.end_fight_min_distance + 900.0);
        }
        assert!(
            !arena.monsters.iter().any(|m| m.encounter_class == "world_end"),
            "the tutorial placed the end fight"
        );
    }

    /// A Psyker's pin stops a creature moving — and NOTHING else. It is still touchable
    /// and still fights when reached: the pin buys the party the first move (the surprise
    /// round in `build_battle`), it does not delete an encounter. The hold counts down
    /// against the world's own `dt` rather than a clock, so the world stays pure.
    #[test]
    fn a_pinned_creature_stops_moving_but_is_still_there_to_fight() {
        let b = Balance::load_default().unwrap();
        let build = || {
            let mut arena = Arena::generate(&b, 5, true);
            arena.monsters[0].aggression = "aggressive".to_string();
            let m = arena.monsters[0].position;
            arena.add_avatar("p".into(), 6.0);
            arena.avatar_mut("p").unwrap().position = Position::new(m.x + 8.0, m.y);
            arena
        };
        let mut free = build();
        let mut pinned = build();
        pinned.monsters[0].held_for = 2.0;
        let start = free.monsters[0].position.x;
        for _ in 0..10 {
            free.step_creatures(0.1);
            pinned.step_creatures(0.1);
        }
        assert!(free.monsters[0].position.x - start > 2.0, "the control never chased");
        assert_eq!(
            pinned.monsters[0].position, Position::new(start, pinned.monsters[0].position.y),
            "a pinned creature moved"
        );

        // Still touchable while pinned — that is the whole point of spending one.
        let immune = std::collections::HashSet::new();
        pinned.avatar_mut("p").unwrap().position = pinned.monsters[0].position;
        assert!(
            pinned.check_touch(&immune).is_some(),
            "a pinned creature could not be engaged — the pin deleted the encounter"
        );

        // And it lapses on its own: 1s of hold left, 2s of ticks, then it chases again.
        let mut lapsing = build();
        lapsing.monsters[0].held_for = 0.5;
        let from = lapsing.monsters[0].position.x;
        for _ in 0..20 {
            lapsing.step_creatures(0.1);
        }
        assert_eq!(lapsing.monsters[0].held_for, 0.0, "the hold never expired");
        assert!(lapsing.monsters[0].position.x - from > 0.0, "it never resumed chasing");
    }

    // AD-4: the Den's roll. Pure, so the same seed is the same contract — and the ladder
    // has to bite: a higher rank must send you further out against something worse, or the
    // hunter rank is a number that means nothing.
    #[test]
    fn a_rolled_contract_is_deterministic_and_scales_with_rank() {
        let b = Balance::load_default().unwrap();
        let a = roll_bounty(&b, 0, 42);
        let again = roll_bounty(&b, 0, 42);
        assert_eq!(a, again, "the same seed rolled two different contracts");

        let deep = roll_bounty(&b, 12, 42);
        assert!(deep.distance > a.distance, "rank did not push the sighting outward");
        assert!(deep.power > a.power, "rank did not make the mark worse");
        assert!(deep.reward_chits > a.reward_chits, "a harder contract pays no better");
        assert!(deep.reward_rank_xp > a.reward_rank_xp);
        assert_eq!(deep.rank, 12);
    }

    // Everything a contract names has to be real: a species the world spawns, in the band
    // it was sighted in, with stats in balance — a mark that cannot be built is a contract
    // that crashes a dive.
    #[test]
    fn every_rolled_contract_names_a_creature_the_world_can_actually_build() {
        let b = Balance::load_default().unwrap();
        let mut venues = std::collections::HashSet::new();
        for seed in 0..200u64 {
            for rank in [0, 3, 9, 25] {
                let spec = roll_bounty(&b, rank, seed.wrapping_mul(0x9E37_79B9));
                assert!(spec.distance > 0);
                assert_eq!(
                    spec.biome,
                    biome_for_distance(spec.distance as i64),
                    "a mark sighted outside its own band"
                );
                assert!(
                    creatures_for_biome(&spec.biome).contains(&spec.creature.as_str()),
                    "{} does not spawn in {}",
                    spec.creature,
                    spec.biome
                );
                assert!(b.creature.contains_key(&spec.creature));
                assert!(!boss_display_name(&spec.boss_kind).is_empty());
                assert_ne!(boss_display_name(&spec.boss_kind), "Unknown Horror");
                assert!(!spec.epithet.is_empty());
                venues.insert(spec.venue);
            }
        }
        // Both venues have to actually come up, or "or in a dungeon" is a promise the
        // roller never keeps.
        assert_eq!(venues.len(), 2, "the roller only ever picks one venue");
    }

    // A contract is ONE player's. Another diver walking over it must not trigger it — in a
    // co-op instance they can fight beside you, but the mark answers to its owner.
    #[test]
    fn a_mark_is_only_touchable_by_its_owner() {
        let b = Balance::load_default().unwrap();
        let mut arena = Arena::generate(&b, 9, true);
        arena.add_avatar("owner".to_string(), 10.0);
        arena.add_avatar("stranger".to_string(), 10.0);
        let spec = roll_bounty(&b, 0, 7);
        // Stand the mark where the world already reaches, then put both players on it.
        let spec = meld_proto::bounties::BountySpec { distance: 20, ..spec };
        assert!(arena.place_bounty_mark(&b, "owner", "b-1", &spec, 5));
        assert!(!arena.place_bounty_mark(&b, "owner", "b-1", &spec, 5), "placed twice");
        let mark_pos = arena
            .monsters
            .iter()
            .find(|m| m.bounty == "b-1")
            .expect("the mark stands")
            .position;
        for pid in ["owner", "stranger"] {
            if let Some(a) = arena.avatar_mut(pid) {
                a.position = mark_pos;
                a.state = "active".to_string();
            }
        }
        // Every other creature is out of the way, so whatever touches is the mark.
        let none = std::collections::HashSet::new();
        let touched: Vec<String> = std::iter::from_fn(|| arena.check_touch(&none))
            .take(1)
            .map(|(pid, _)| pid)
            .collect();
        assert_eq!(touched, vec!["owner".to_string()], "a stranger triggered someone's contract");
    }

    // A board that tells a player where to hunt something is only as honest as this
    // inverse: if it disagrees with the placement table, the advice sends them to the
    // wrong biome.
    #[test]
    fn biomes_of_creature_is_the_inverse_of_the_placement_table() {
        for b in BIOMES {
            for kind in creatures_for_biome(b) {
                assert!(
                    biomes_of_creature(kind).contains(&b),
                    "{kind} spawns in {b} but the inverse does not say so"
                );
            }
        }
        assert!(biomes_of_creature("no_such_creature").is_empty());
        // Field and forest are the same fauna on different tree counts, so a forest
        // creature answers with both — the advice has to name both or it is wrong half
        // the time.
        assert_eq!(biomes_of_creature("forest_bloom_stalker"), vec!["field", "forest"]);
    }

    // AD-4: a hunt that names a creature nothing spawns is a contract that can never
    // be filled, and the board would advertise it forever. The registry lives in
    // `meld-proto` and the roster lives here, so this is the only place the two can be
    // held against each other.
    #[test]
    fn every_posted_hunt_names_a_creature_the_world_actually_spawns() {
        let balance = Balance::load_default().unwrap();
        let spawnable: std::collections::HashSet<&str> = BIOMES
            .iter()
            .flat_map(|b| creatures_for_biome(b).iter().copied())
            .collect();
        for hunt in meld_proto::hunts::HUNTS {
            if let meld_proto::hunts::HuntGoal::Fell { creature, .. } = hunt.goal {
                assert!(
                    spawnable.contains(creature),
                    "{} hunts {creature}, which no biome spawns",
                    hunt.key
                );
                assert!(
                    balance.creature.contains_key(creature),
                    "{creature} has no [creature.{creature}] stats"
                );
            }
        }
    }

    // A field station is a PLACE: it lands where its builder stands, only one to a
    // spot, and it can only be worked from that spot and that elevation. Everything
    // about who may build one lives in the server; this is the world's half.
    #[test]
    fn a_station_lands_where_you_stand_and_is_worked_from_there() {
        let b = Balance::load_default().unwrap();
        let mut arena = Arena::generate(&b, 4, false);
        arena.add_avatar("smith".into(), 6.0);
        arena.add_avatar("client".into(), 6.0);
        let radius = 3.0;

        let id = arena.place_station("smith", "smith", 2, radius, "dune_iron").expect("raised").entity_id.clone();
        let here = arena.avatar("smith").unwrap().position;
        assert_eq!(arena.stations.len(), 1);
        assert_eq!(arena.stations[0].owner_player_id, "smith");

        // Anyone standing at it may work at it — the station is the permission, not
        // the person. Its owner is only whose SKILL the job is done at.
        assert!(arena.station_at("client", &id, radius).is_some());

        // Not from across the maze, though.
        if let Some(a) = arena.avatar_mut("client") {
            a.position = Position { x: here.x + 50.0, y: here.y };
        }
        assert!(arena.station_at("client", &id, radius).is_none(), "reach is reach");

        // Nor from a terrace above it: a bench is on the ground you built it on.
        if let Some(a) = arena.avatar_mut("client") {
            a.position = here;
            a.elevation = 1;
        }
        assert!(arena.station_at("client", &id, radius).is_none(), "elevation counts");

        // One to a spot: a smith cannot carpet a tile in benches.
        assert!(arena.place_station("smith", "smith", 2, radius, "dune_iron").is_none());

        // Its jobs run out, and a spent station is no longer a station.
        assert_eq!(arena.spend_station_use(&id), Some(1));
        assert_eq!(arena.spend_station_use(&id), Some(0));
        assert_eq!(arena.spend_station_use(&id), None, "spent is spent");
        assert!(arena.station_at("smith", &id, radius).is_none());
        // …which frees the ground for the next one.
        assert!(arena.place_station("smith", "smith", 2, radius, "dune_iron").is_some());
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
    fn red_gear_never_drops_below_the_ramp_start() {
        let b = Balance::load_default().unwrap();
        let floor = b.world_scaling.red_chest_floor_distance;
        let start = b.loot.gear_ramp_start_distance;
        for s in 0..200 {
            assert!(roll_creature_loot(&b, start - 1, 1, 1.0, s).gear.is_none());
        }
        // At/after the floor: gear does appear for some seeds, at the right tier.
        let mut saw_gear = false;
        for s in 0..200 {
            if let Some(g) = roll_creature_loot(&b, floor, 1, 1.0, s).gear {
                saw_gear = true;
                assert_eq!(g.tier, Scaling::new(&b).tier(floor) as i32);
                assert!(g.max_durability > 0);
                // Exactly one stat is rolled, matching the drop's own category
                // (7-slot loadout: main hand hits, accessory speeds, the four
                // protective pieces all defend).
                let stat = match g.slot.as_str() {
                    "main_hand" => g.atk_bonus,
                    "off_hand" | "head" | "chest" | "legs" => g.def_bonus,
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

    /// The bug GR-8 fixes is arithmetic between two tunables, so the test is
    /// arithmetic too: there must be no reachable depth of an ordinary dive at which
    /// the gear chase is switched off. `area_count` and the area-length curve decide
    /// how far a dive goes, so this reads the world's OWN extent rather than a
    /// written-down number that would not follow a retune.
    #[test]
    fn gear_can_drop_everywhere_a_dive_actually_reaches() {
        let b = Balance::load_default().unwrap();
        let deepest = (0..12)
            .map(|s| Arena::generate(&b, s, false).portal.x.floor() as i64)
            .min()
            .unwrap();
        assert!(
            deepest > b.loot.gear_ramp_start_distance,
            "a dive's own portal ({deepest}) must sit past the ramp start"
        );
        // Sample the whole walk out, not just the ends: a gap anywhere in here is a
        // stretch of the dive where killing things cannot pay in gear.
        for d in (b.loot.gear_ramp_start_distance..=deepest).step_by(20) {
            let hits = (0..600).filter(|s| roll_creature_loot(&b, d, 1, 1.0, *s).gear.is_some()).count();
            assert!(hits > 0, "d={d} is inside a normal dive and drops no gear at all");
        }
    }

    #[test]
    fn the_gear_ramp_climbs_and_lands_exactly_on_the_floor_rate() {
        let b = Balance::load_default().unwrap();
        let floor = b.world_scaling.red_chest_floor_distance;
        let start = b.loot.gear_ramp_start_distance;
        assert_eq!(gear_drop_chance_at(&b, start - 1), 0.0);
        // Deep rates are the untouchable half of this change: everything at or past
        // the floor must still be exactly `gear_drop_chance`, so no existing deep
        // tuning moves and the ramp is provably additive.
        for d in [floor, floor + 1, floor + 700, 40_000] {
            assert!(
                (gear_drop_chance_at(&b, d) - b.loot.gear_drop_chance).abs() < 1e-9,
                "d={d} should sit at exactly the full rate"
            );
        }
        let mid = start + (floor - start) / 2;
        for (lo, hi) in [(start, mid), (mid, floor)] {
            assert!(
                gear_drop_chance_at(&b, hi) > gear_drop_chance_at(&b, lo),
                "the ramp should climb from d={lo} to d={hi}"
            );
        }
        assert!(
            gear_drop_chance_at(&b, start) > 0.0
                && gear_drop_chance_at(&b, start) < b.loot.gear_drop_chance,
            "the shallow end should be a trickle, not the full faucet and not nothing"
        );
    }

    #[test]
    fn a_kill_can_drop_a_potion_and_the_band_gates_which_one() {
        let b = Balance::load_default().unwrap();
        let drops = |d: i64| -> Vec<&'static str> {
            (0..400).filter_map(|s| {
                let p = roll_creature_loot(&b, d, 1, 1.0, s).potion;
                (!p.is_empty()).then_some(p)
            }).collect()
        };
        let shallow = drops(30);
        assert!(!shallow.is_empty(), "a shallow kill should sometimes drop a potion");
        // The trophy line is authored for depth and the Apothecary's basics are not,
        // so a hub-ring kill must never hand over a deep-band brew.
        for key in &shallow {
            let def = meld_proto::consumables::consumable(key).expect("a real potion");
            assert_eq!(def.tier, 0, "{key} is above the shallow band");
        }
        let deep: std::collections::BTreeSet<_> = drops(4000).into_iter().collect();
        assert!(
            deep.len() > shallow.iter().collect::<std::collections::BTreeSet<_>>().len(),
            "the deep pool should be wider than the shallow one"
        );
        // The two progression consumables have their own faucets
        // (`world_xp_item_chance` / `world_revive_item_chance`); appearing here too
        // would silently double their rate.
        for key in &deep {
            let def = meld_proto::consumables::consumable(key).unwrap();
            assert!(
                !matches!(
                    def.effect,
                    meld_proto::consumables::ConsumableEffect::Revive
                        | meld_proto::consumables::ConsumableEffect::Experience
                ),
                "{key} has a dedicated faucet and must stay out of the shared pool"
            );
        }
    }

    /// The potion roll is deliberately on its own RNG sub-stream, because a draw taken
    /// from the shared one would have shifted every gear/rarity/affix roll above it.
    #[test]
    fn the_potion_roll_does_not_disturb_the_rest_of_the_loot() {
        let mut b = Balance::load_default().unwrap();
        let deep = 4000;
        let with: Vec<_> = (0..200)
            .map(|s| roll_creature_loot(&b, deep, 2, 1.0, s))
            .map(|l| (l.chits, l.material, l.material_qty, l.gear))
            .collect();
        b.loot.potion_drop_chance = 0.0;
        let without: Vec<_> = (0..200)
            .map(|s| roll_creature_loot(&b, deep, 2, 1.0, s))
            .map(|l| (l.chits, l.material, l.material_qty, l.gear))
            .collect();
        assert_eq!(with, without, "turning potions off must change nothing else");
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

    // (Removed `seam_wall_blocks_crossing_outside_the_gap`: biome seams no longer WALL the
    // world with a gap-only crossing — that full-width barrier was the "corridor". You
    // cross boundaries freely now; seams remain only as the Gatekeeper/biome-transition
    // marker, tested by `generates_chests_and_biome_seams`.)

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
    fn a_wandering_creature_actually_goes_somewhere() {
        // Reported from play: "creature movement in the overworld makes no sense
        // whatsoever." The wander DESTINATION was re-rolled inside the movement pass on
        // every tick — a fresh angle ten times a second — so a creature was chasing a
        // point that teleported around its leash faster than it could walk. It walked at
        // full speed and stayed put: 47.8 tiles of path for 0.87 tiles of net
        // displacement over 30 s, never more than 1.93 tiles from where it started, with
        // 98% of the motion cancelling itself out. The client picks its 8-way facing off
        // frame-to-frame movement, so the sprites spun on the spot too.
        //
        // This pins the OUTCOME rather than the mechanism: a creature with nothing to
        // chase must cover a real share of its own leash. Anything that reintroduces
        // per-tick destination churn fails here however it is written.
        let b = Balance::load_default().unwrap();
        let mut arena = Arena::generate(&b, 424242, false);
        // No avatars: nothing to chase, so every creature is on the wander path.
        arena.avatars.clear();
        let sample: Vec<usize> = (0..arena.monsters.len().min(200)).collect();
        let start: Vec<Position> = sample.iter().map(|&i| arena.monsters[i].position).collect();
        let mut reach: Vec<f64> = vec![0.0; sample.len()];
        // 30 s at the 100 ms authoritative tick.
        for _ in 0..300 {
            arena.step_creatures(0.1);
            for (k, &i) in sample.iter().enumerate() {
                reach[k] = reach[k].max(arena.monsters[i].position.distance_to(&start[k]));
            }
        }
        let mean = reach.iter().sum::<f64>() / reach.len() as f64;
        // Half its own leash is a low bar deliberately: the destination is drawn inside
        // the leash disc, so the EXPECTED excursion is well under the full radius, and
        // terrain legitimately blocks some legs. The bug sat at 1.93.
        assert!(
            mean > arena.leash_radius * 0.5,
            "a wandering creature should cover a real share of its leash \
             (mean furthest excursion {mean:.2} of leash {:.1})",
            arena.leash_radius
        );
        // ...and it must still be a LEASH, not a migration: the fix must not let
        // anything walk off into the next biome.
        for (k, &i) in sample.iter().enumerate() {
            assert!(
                reach[k] <= arena.leash_radius * 2.5,
                "creature {i} wandered {:.1} from home — the leash still has to hold",
                reach[k]
            );
        }
        // ...and the measurement above is not vacuous: put the destination back on a
        // per-tick re-roll (leg length 0, no pauses) and the world really does go back
        // to vibrating in place, which is the bug as it shipped.
        let mut churn = b.clone();
        churn.ai.wander_leg_seconds = 0.0;
        churn.ai.wander_pause_chance = 0.0;
        let mut arena = Arena::generate(&churn, 424242, false);
        arena.avatars.clear();
        let start: Vec<Position> = sample.iter().map(|&i| arena.monsters[i].position).collect();
        let mut reach: Vec<f64> = vec![0.0; sample.len()];
        for _ in 0..300 {
            arena.step_creatures(0.1);
            for (k, &i) in sample.iter().enumerate() {
                reach[k] = reach[k].max(arena.monsters[i].position.distance_to(&start[k]));
            }
        }
        let churned = reach.iter().sum::<f64>() / reach.len() as f64;
        assert!(
            churned < arena.leash_radius * 0.35,
            "with a per-tick re-roll the creature should barely leave its own tile \
             (mean furthest excursion {churned:.2})"
        );
    }

    #[test]
    fn a_wandering_creature_stands_still_sometimes() {
        // A creature that walks every single tick reads as machinery. The pause is the
        // other half of what makes the overworld look inhabited, and it is cheap to lose
        // in a refactor because nothing else observes it.
        let b = Balance::load_default().unwrap();
        let mut arena = Arena::generate(&b, 424242, false);
        arena.avatars.clear();
        let sample: Vec<usize> = (0..arena.monsters.len().min(200)).collect();
        let mut still = 0usize;
        let mut total = 0usize;
        let mut prev: Vec<Position> = sample.iter().map(|&i| arena.monsters[i].position).collect();
        for _ in 0..300 {
            arena.step_creatures(0.1);
            for (k, &i) in sample.iter().enumerate() {
                let now = arena.monsters[i].position;
                if now.distance_to(&prev[k]) < 1e-9 {
                    still += 1;
                }
                total += 1;
                prev[k] = now;
            }
        }
        let share = still as f64 / total as f64;
        assert!(
            share > 0.10 && share < 0.80,
            "wandering creatures should pause sometimes and walk sometimes \
             (standing still {:.0}% of ticks)",
            share * 100.0
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

    /// The fixture the clash tests share: exactly one hostile pair, nose to nose, with
    /// nothing else in the world to brawl and nobody to chase.
    fn a_lone_hostile_pair(b: &Balance) -> Arena {
        let mut arena = Arena::generate(b, 5, true);
        assert!(arena.monsters.len() >= 2);
        arena.monsters.truncate(2);
        for k in 0..2 {
            arena.monsters[k].area_min_x = f64::NEG_INFINITY;
            arena.monsters[k].area_max_x = f64::INFINITY;
        }
        arena.monsters[0].faction = "beast".to_string();
        arena.monsters[0].aggression = "passive".to_string();
        arena.monsters[0].atk = 1;
        arena.monsters[0].hp = 5000;
        arena.monsters[0].def = 0;
        let pos = arena.monsters[0].position;
        arena.monsters[1].faction = "fiend".to_string(); // beast vs fiend = hostile
        arena.monsters[1].aggression = "passive".to_string();
        arena.monsters[1].atk = 1;
        arena.monsters[1].hp = 5000;
        arena.monsters[1].def = 0;
        arena.monsters[1].home = Position::new(pos.x + 1.0, pos.y);
        arena.monsters[1].position = Position::new(pos.x + 1.0, pos.y);
        arena.monsters[1].elevation = arena.monsters[0].elevation;
        arena
    }

    /// CR-2: a clash is derived from BLOWS, never from proximity. On a crowded ring
    /// plenty of hostiles stand inside each other's reach without ever swinging, and a
    /// ⚔ over every one of those is noise the player learns to ignore in one dive.
    #[test]
    fn a_clash_is_the_blows_that_landed_not_the_creatures_standing_near_each_other() {
        let b = Balance::load_default().unwrap();
        let mut arena = a_lone_hostile_pair(&b);
        // Out of reach of each other: hostile, but not fighting.
        let pos = arena.monsters[0].position;
        arena.monsters[1].home = Position::new(pos.x + 400.0, pos.y);
        arena.monsters[1].position = Position::new(pos.x + 400.0, pos.y);
        arena.step_creatures(0.1);
        assert!(arena.clashes.is_empty(), "two creatures merely existing counted as a fight");

        // Nose to nose: now blows land, and both ends of each blow are in the clash.
        let mut arena = a_lone_hostile_pair(&b);
        arena.step_creatures(0.1);
        assert_eq!(arena.clashes.len(), 1, "adjacent hostiles did not start a clash");
        assert_eq!(arena.clashes[0].members.len(), 2, "only one side of the brawl is in it");
        for m in &arena.monsters {
            assert!(
                arena.clash_of(&m.entity_id).is_some(),
                "{} is swinging but is not marked as clashing",
                m.entity_id
            );
        }
    }

    /// A clash is a CADENCE of blows, so it has to outlive the gap between them —
    /// otherwise the ⚔ marker strobes once per `skirmish_attack_interval` and a watcher
    /// is dropped every time a creature reloads its swing. It does eventually end, or a
    /// brawl that finished would stay marked forever.
    #[test]
    fn a_clash_outlives_the_gap_between_blows_but_not_the_quiet_after_it() {
        let b = Balance::load_default().unwrap();
        let mut arena = a_lone_hostile_pair(&b);
        arena.step_creatures(0.1);
        assert_eq!(arena.clashes.len(), 1);

        // Pull them apart so no further blow can land, then let a little time pass —
        // less than the linger. Still a clash: the last blow is recent.
        let pos = arena.monsters[0].position;
        arena.monsters[1].home = Position::new(pos.x + 400.0, pos.y);
        arena.monsters[1].position = Position::new(pos.x + 400.0, pos.y);
        arena.step_creatures(b.ai.clash_linger_seconds * 0.5);
        assert_eq!(arena.clashes.len(), 1, "the clash blinked out between swings");

        arena.step_creatures(b.ai.clash_linger_seconds);
        assert!(arena.clashes.is_empty(), "a brawl that stopped is still marked as one");
    }

    /// A clash of one is not a fight. A body that dies, is pulled into a player's battle,
    /// or is streamed away leaves the clash — and the last one standing leaves nothing.
    #[test]
    fn a_creature_that_stops_swinging_leaves_the_clash() {
        let b = Balance::load_default().unwrap();
        let mut arena = a_lone_hostile_pair(&b);
        arena.step_creatures(0.1);
        assert_eq!(arena.clashes.len(), 1);
        // Membership is by ENTITY ID exactly so `prune_defeated` may compact `monsters`
        // underneath a live clash without corrupting it.
        arena.monsters[1].in_battle = true;
        arena.step_creatures(0.1);
        assert!(
            arena.clashes.is_empty(),
            "one creature was left clashing with a partner that had walked into a battle"
        );
        assert!(arena.clashing().is_empty());
    }

    /// CR-2: a wound CLOSES, slowly. Without it the world is strip-minable by attrition
    /// — walk a ring, chip everything, come home to a map of half-dead things — and with
    /// it a hurt creature is a time-bound opportunity instead of a permanent discount.
    #[test]
    fn a_wounded_creature_mends_as_it_roams() {
        let b = Balance::load_default().unwrap();
        let mut arena = a_lone_hostile_pair(&b);
        // One creature, alone and hurt, nothing to fight.
        arena.monsters.truncate(1);
        let max = arena.monsters[0].max_hp;
        arena.monsters[0].hp = max / 2;
        let before = arena.monsters[0].hp;

        // Long enough for the fraction to clear a whole HP at this creature's size.
        for _ in 0..40 {
            arena.step_creatures(0.25);
        }
        let after = arena.monsters[0].hp;
        assert!(after > before, "a wound never closed: {before} -> {after} of {max}");
        assert!(after <= max, "it healed past full: {after} of {max}");

        // And it stops at full rather than running away with it.
        for _ in 0..4000 {
            arena.step_creatures(0.25);
        }
        assert_eq!(arena.monsters[0].hp, max, "it did not reach full, or overshot it");
    }

    /// Nothing mends while it is still being hit. The clash's own linger is what covers
    /// the gap between one blow and the next — without that gate a creature would tick
    /// health back between every pair of swings and a skirmish could never resolve.
    #[test]
    fn a_creature_in_a_clash_does_not_mend() {
        let b = Balance::load_default().unwrap();
        let mut arena = a_lone_hostile_pair(&b);
        // Big pools and 1-damage blows, so the only thing that could move the HP up is
        // the regen and the only thing that could move it down is the skirmish.
        for k in 0..2 {
            arena.monsters[k].hp = arena.monsters[k].max_hp / 2;
        }
        let before: i32 = arena.monsters.iter().map(|m| m.hp).sum();
        for _ in 0..40 {
            arena.step_creatures(0.25);
        }
        assert!(!arena.clashes.is_empty(), "the fixture stopped clashing");
        let after: i32 = arena.monsters.iter().map(|m| m.hp).sum();
        assert!(after < before, "creatures healed mid-brawl: {before} -> {after}");
    }

    /// A creature at full carries no healing DEBT forward, so a long healthy stretch
    /// cannot bank a burst that lands the instant something finally hits it.
    #[test]
    fn a_healthy_creature_banks_no_healing_for_later() {
        let b = Balance::load_default().unwrap();
        let mut arena = a_lone_hostile_pair(&b);
        arena.monsters.truncate(1);
        let max = arena.monsters[0].max_hp;
        for _ in 0..200 {
            arena.step_creatures(0.25);
        }
        arena.monsters[0].hp = max - 1;
        arena.step_creatures(0.001);
        assert_eq!(
            arena.monsters[0].hp,
            max - 1,
            "a healthy creature had banked healing and spent it the moment it was hurt"
        );
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

    /// Spec §4: `xp = floor(base_xp × (1 + d/500)^1.5)`, and the elite/boss
    /// multiplier ×3 rides on top via `promote` (encounters.elite_xp_mult).
    #[test]
    fn xp_follows_the_spec_distance_curve() {
        let b = Balance::load_default().unwrap();
        let s = Scaling::new(&b);
        assert!((s.xp_mult(0) - 1.0).abs() < 1e-9);
        // d=500 → (1 + 1)^1.5 = 2.828…
        assert!((s.xp_mult(500) - 2.0_f64.powf(1.5)).abs() < 1e-9);
        assert_eq!(b.encounters.elite_xp_mult, 3.0);
        assert_eq!(b.encounters.gatekeeper_xp_mult, 3.0);
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
        // Un-seeded terrain: asserts area SIZING + the portal-in-bounds invariant, which a
        // per-run mesa nudging the portal shouldn't perturb (terrain variety is tested by
        // the walker sweep). Deterministic structure only.
        let arena = Arena::build_with(&b, 7, true, None, (0.0, 0.0));
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
    fn a_node_holds_stock_and_gives_it_up_one_unit_at_a_time() {
        let b = Balance::load_default().unwrap();
        let mut arena = Arena::generate(&b, 7, true);
        assert!(!arena.resources.is_empty(), "resource nodes are scattered in");
        // Use the guaranteed level-0 starter node (area 0) so elevation doesn't gate.
        let node = arena.resources[0].clone();
        assert_eq!(node.elevation, 0);
        assert!(node.remaining > 1, "MS-2: a node is a quantity, not a one-tap flag");
        assert_eq!(node.remaining, node_stock(&b, &node.kind));

        // Every node spawns stocked, and every kind maps to balance content AND to the
        // material registry — so a node can never yield an item nothing can spend.
        // Checked before anything is drained below.
        for n in &arena.resources {
            let res = b.resource.get(&n.kind).unwrap_or_else(|| panic!("{} in balance", n.kind));
            assert!(
                meld_proto::materials::material(&res.material).is_some(),
                "node {} yields unregistered material {}",
                n.kind,
                res.material
            );
            assert!(n.remaining > 0, "node {} spawned empty", n.kind);
        }
        arena.add_avatar("p".into(), 6.0);

        // Too far → nothing, and the check is the same one the channel re-runs.
        arena.avatar_mut("p").unwrap().position =
            Position::new(node.position.x + 50.0, node.position.y);
        assert!(arena.can_harvest("p", &node.entity_id).is_none(), "out of range");
        assert!(arena.take_one("p", &node.entity_id).is_none(), "out of range");
        assert_eq!(arena.resources[0].remaining, node.remaining, "a failed take costs nothing");

        // Standing on it → one unit per call, until the node runs dry.
        arena.avatar_mut("p").unwrap().position = node.position;
        for left in (0..node.remaining).rev() {
            assert_eq!(
                arena.take_one("p", &node.entity_id).as_deref(),
                Some(node.kind.as_str())
            );
            assert_eq!(arena.resources[0].remaining, left);
        }
        assert!(arena.resources[0].depleted());
        assert!(arena.take_one("p", &node.entity_id).is_none(), "an empty node gives nothing");
        assert!(arena.can_harvest("p", &node.entity_id).is_none());

        // An ore vein is a longer commitment than a reagent patch — that rhythm split
        // is what makes the two gathering professions play differently.
        let (r_stock, r_tick) = b.harvest.node_yield("reagent");
        let (o_stock, o_tick) = b.harvest.node_yield("ore");
        assert!(o_stock > r_stock, "an ore vein should hold more");
        assert!(o_tick > r_tick, "…and give it up more slowly");

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
    fn no_obstacle_intrudes_the_clear_path_and_the_ends_stay_grounded() {
        // The feasibility guarantee: the path tube holds no blocking obstacle, and the
        // route both STARTS and ENDS on the ground (spawn + portal on level 0) so a
        // dive always opens and extraction always closes without a climb. The middle of
        // the path MAY climb a ramp-connected plateau (the `path_climb` feature) — that
        // it stays traversable is proven by `the_clear_path_actually_reaches_the_portal`.
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
            if let (Some(first), Some(last)) = (arena.path.first(), arena.path.last()) {
                assert_eq!(arena.level_at(first), 0, "seed {seed}: the route starts off the ground");
                assert_eq!(arena.level_at(last), 0, "seed {seed}: the portal end is off the ground");
            }
        }
    }

    #[test]
    fn creatures_stay_inside_their_biome_ring() {
        // #10: a creature belongs to its biome, which in the radial fan is a RADIUS
        // ring ([start_x, end_x]). After a long roam (chasing a player around) no
        // creature has wandered out of its ring into a neighbouring biome.
        let b = Balance::load_default().unwrap();
        let mut arena = Arena::generate(&b, 7, false);
        // A player roaming the fan gives aggressive creatures something to chase.
        arena.add_avatar("p".into(), 40.0);
        let bands: Vec<(usize, f64, f64)> = arena
            .monsters
            .iter()
            .enumerate()
            .filter(|(_, m)| m.area_min_x.is_finite() && m.area_max_x.is_finite())
            .map(|(i, m)| (i, m.area_min_x, m.area_max_x))
            .collect();
        assert!(!bands.is_empty(), "creatures carry a radius band");
        for tick in 0..2000 {
            // Sweep the player around so aggressive creatures try to chase across rings.
            let ang = (tick as f64) * 0.05;
            arena.apply_move("p", ang.cos(), ang.sin(), tick as u32);
            arena.step_creatures(0.1);
        }
        for (i, lo, hi) in bands {
            let m = &arena.monsters[i];
            let r = m.position.x.hypot(m.position.y);
            assert!(
                r >= lo - 3.0 && r <= hi + 3.0,
                "creature {} left its biome ring: r={r:.1} band=[{lo:.1},{hi:.1}]",
                m.entity_id
            );
        }
    }

    /// Trees per unit of GROUND, ring by ring, for a world pinned to one biome. Per unit
    /// of corridor length (what `forest_is_a_dense_maze_and_desert_is_open` measures) a
    /// section can look thick while the ground under the player is nearly bare, because
    /// the fan stretches each section over an ever-larger sector.
    fn fill_per_area_by_ring(b: &Balance, biome: &'static str) -> Vec<f64> {
        let a = Arena::generate_with(b, 42, false, Some(biome));
        let half = a.radial_half();
        [(30.0, 90.0), (90.0, 170.0), (170.0, 270.0)]
            .iter()
            .map(|&(lo, hi)| {
                let n = a
                    .obstacles
                    .iter()
                    .filter(|o| {
                        let r = o.position.x.hypot(o.position.y);
                        r >= lo && r < hi
                    })
                    .count() as f64;
                n / ((hi * hi - lo * lo) * half)
            })
            .collect()
    }

    #[test]
    fn the_wood_is_thick_where_you_are_standing() {
        // Reported from play: "the forest biome feels more like a field with trees in it."
        // It asked for 392 props a section and placed 90. Spacing was checked in corridor
        // coordinates where y is an ANGLE, so two trees that end up 190 world units apart
        // at depth read as overlapping and one of them was thrown away — the count was
        // compensated for the fan, the spacing never was. Density on the GROUND is the
        // only measure that catches that, so that is what this pins.
        let b = Balance::load_default().unwrap();
        let forest = fill_per_area_by_ring(&b, "forest");
        for (n, d) in forest.iter().enumerate() {
            assert!(*d > 0.006, "forest ring {n} is a field, not a wood: {d:.4}/u²");
        }
        // And the FIELD is the same ground deliberately left open — the contrast between
        // them is the content, so it has to be a big one, not a tuning nudge.
        let field = fill_per_area_by_ring(&b, "field");
        assert!(
            forest[1] > field[1] * 4.0,
            "a wood should be several times a meadow: forest {:.4}/u² vs field {:.4}/u²",
            forest[1],
            field[1]
        );
        assert!(field[1] > 0.0, "a field still has trees in the distance");
    }

    #[test]
    fn forest_is_a_dense_maze_and_desert_is_open() {
        // #8: a forest section packs far more blocking fill than the open desert, so
        // the wood reads as a maze with only the trail open. Compare per-biome counts.
        let b = Balance::load_default().unwrap();
        // Aggregate obstacle DENSITY (obstacles per unit corridor-length) for a biome
        // across many sections/seeds — robust to per-section noise from the web + terraces.
        let density_for = |biome: &str| -> f64 {
            let (mut obs, mut span) = (0.0f64, 0.0f64);
            for seed in 0u64..80 {
                let a = Arena::generate(&b, seed, false);
                for sec in a.areas.iter().filter(|s| s.index >= 1 && s.biome == biome && !s.dungeon) {
                    let (lo, hi) = (sec.start_x, sec.end_x);
                    obs += a
                        .obstacles
                        .iter()
                        .filter(|o| {
                            let r = o.position.x.hypot(o.position.y);
                            r >= lo && r < hi
                        })
                        .count() as f64;
                    span += hi - lo;
                }
            }
            if span > 0.0 { obs / span } else { 0.0 }
        };
        let forest = density_for("forest");
        let desert = density_for("desert");
        // Forest is a thick maze; desert is the open breather biome. Both carry the
        // biome-independent base scatter (dilutes the ratio), so require a clear margin.
        assert!(forest > desert * 1.5, "forest density ({forest:.2}/u) should far exceed desert ({desert:.2}/u)");
    }

    #[test]
    fn the_overworld_weaves_a_web_of_trails_not_one_lane() {
        // Ditch the corridor: each non-tutorial run weaves extra trails (branches,
        // loops, spurs) beyond the backbone, and the dense fill is carved clear of them
        // so they're actually walkable — an interconnected maze, not a single route.
        let b = Balance::load_default().unwrap();
        let mut a = Arena::generate(&b, 7, false);
        a.ensure_frontier(&b, 400.0);
        assert!(a.web.len() > a.areas.len(), "several web trails woven per section");
        // No obstacle sits on a web trail (its slit is cleared), so every trail walks.
        let web_r = a.web_clear();
        for o in &a.obstacles {
            let d = dist_to_web(&o.position, &a.web);
            assert!(
                d >= web_r - 1e-6,
                "obstacle {} blocks a web trail (d={d:.2} < {web_r:.2})",
                o.entity_id
            );
        }
    }

    #[test]
    fn the_mire_floods_with_water() {
        // #9: the Mire's dense fill is impassable water (`bog_pool`) — a flooded maze,
        // not a field of solid props.
        assert_eq!(fill_kind_for_biome("mire"), "bog_pool");
    }

    #[test]
    fn authored_peaks_are_climbable_and_crowned() {
        // #3 (verticality): authored landmark MOUNTAINS — smooth walkable domes summed
        // into the terrain, each crowned with a boss or a treasure chest on its SUMMIT.
        // Assert peaks appear across seeds, stay CLIMBABLE (dome height within the
        // walkable-aspect cap), carry a summit reward at the centre, and genuinely RAISE
        // the ground there (base + dome, via `t_height`).
        let b = Balance::load_default().unwrap();
        let mut total = 0usize;
        for seed in 0u64..48 {
            let a = Arena::generate(&b, seed, false);
            for p in &a.peaks {
                total += 1;
                let (cx, cy, r, h) = (p[0] as f64, p[1] as f64, p[2] as f64, p[3] as f64);
                assert!(
                    h <= r * meld_proto::terrain::PEAK_MAX_ASPECT as f64 + 1e-3,
                    "seed {seed}: peak height {h:.1} / radius {r:.1} must stay climbable"
                );
                let c = Position::new(cx, cy);
                let crowned = a.chests.iter().any(|ch| ch.position.distance_to(&c) < 2.0)
                    || a.monsters.iter().any(|m| m.position.distance_to(&c) < 2.0);
                assert!(crowned, "seed {seed}: peak at ({cx:.0},{cy:.0}) has a summit reward");
                let base = meld_proto::terrain::height(cx as f32, cy as f32, a.terrain_off.0, a.terrain_off.1) as f64;
                assert!(
                    a.t_height(cx, cy) > base + h * 0.8,
                    "seed {seed}: the dome raises the summit above the base"
                );
            }
        }
        assert!(total > 0, "authored climbable peaks appear across 48 procedural seeds");
    }

    #[test]
    fn the_clear_path_actually_reaches_the_portal() {
        // A walker that follows the A*-routed waypoints reaches the portal without ever
        // stalling on terrain or a heightmap cliff — the route is feasible by construction
        // (the A* backbone bends AROUND every mesa). Swept across many seeds AND both
        // tutorial/non-tutorial worlds, since cliffs strand only on specific seeds where a
        // butte sits on a naive straight segment: this is the guarantee cliffs never trap.
        let b = Balance::load_default().unwrap();
        for seed in 0u64..24 {
            for tutorial in [true, false] {
                let mut arena = Arena::generate(&b, seed, tutorial);
                let waypoints = arena.path.clone();
                assert!(waypoints.len() >= 2, "seed {seed} t={tutorial}: path has waypoints");
                arena.add_avatar("p".into(), if tutorial { 8.0 } else { 2.0 });
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
                assert!(reached, "seed {seed} t={tutorial}: following the path reaches the portal");
                let end = arena.avatar("p").unwrap().position;
                assert!(
                    end.distance_to(&arena.portal) < 1.5,
                    "seed {seed} t={tutorial}: walker ended at the portal"
                );
                assert_eq!(
                    arena.avatar("p").unwrap().elevation,
                    0,
                    "seed {seed} t={tutorial}: walker stayed on the ground"
                );
            }
        }
    }

    // (Removed `the_clear_path_climbs_a_plateau_and_still_reaches_the_portal`: the
    // discrete path-CLIMB — `maybe_climb_path`'s `pramp-` ramp raised across a section-
    // spanning meander — is superseded by the continuous heightmap + A* path routing.
    // A* fragments the path into short walkable waypoints, so no section-spanning
    // segment exists to raise a plateau over; elevation now comes from the heightmap
    // and the path routes AROUND cliffs rather than climbing them.)

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
        let none: std::collections::HashSet<String> = std::collections::HashSet::new();
        assert!(arena.check_touch(&none).is_none());
        // Walk east along the corridor for up to ~8 s of sim ticks.
        let mut hit = None;
        for i in 0..(20 * 8) {
            arena.apply_move("p1", 1.0, 0.0, i + 1);
            if let Some((p, idx)) = arena.check_touch(&none) {
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
        let again = arena.check_touch(&none);
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
        // The section is a RADIUS band in the bent world, so test by radius (hypot), not
        // world-x — after the radial bend `position.x` is `r·cosθ`, not the radius.
        let in_dungeon = |p: &Position| {
            let r = p.x.hypot(p.y);
            r >= s && r <= e
        };
        let walls = arena.obstacles.iter().filter(|o| in_dungeon(&o.position)).count();
        assert!(walls > 0, "dungeon carries divider-wall obstacles");
        assert!(
            arena.chests.iter().any(|c| in_dungeon(&c.position)),
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

    // ---- The shallow-ring on-ramp ----

    #[test]
    fn the_on_ramp_softens_the_hub_and_vanishes_past_its_distance() {
        let b = Balance::load_default().unwrap();
        let s = Scaling::new(&b);
        let ws = &b.world_scaling;
        assert!((s.onboarding_mult(0) - ws.onboarding_floor).abs() < 1e-9);
        assert_eq!(s.onboarding_mult(ws.onboarding_distance), 1.0);
        // Past the on-ramp it is EXACTLY 1.0, so the deep game is untouched by it.
        for d in [
            ws.onboarding_distance,
            ws.onboarding_distance + 1,
            1_000,
            10_000,
        ] {
            assert_eq!(s.onboarding_mult(d), 1.0, "ramp leaked out to d={d}");
        }
        // Monotonic across the ramp — no step a player can feel as a difficulty wall.
        let mut prev = 0.0;
        for d in (0..=ws.onboarding_distance).step_by(10) {
            let m = s.onboarding_mult(d);
            assert!(m >= prev, "ramp dipped at d={d}");
            prev = m;
        }
    }

    #[test]
    fn a_hub_creature_is_gentler_than_the_same_kind_past_the_on_ramp() {
        let b = Balance::load_default().unwrap();
        let past = b.world_scaling.onboarding_distance as f64;
        let near = MonsterSpawn::build(&b, "m".into(), "bog_serpent", Position::new(10.0, 0.0), 1);
        let far = MonsterSpawn::build(&b, "m".into(), "bog_serpent", Position::new(past, 0.0), 1);
        assert!(near.atk < far.atk, "{} vs {}", near.atk, far.atk);
        assert!(near.max_hp < far.max_hp, "{} vs {}", near.max_hp, far.max_hp);
        // Never scaled out of existence: a creature the party cannot damage or that
        // cannot act is a softlock, not an easy fight.
        assert!(near.max_hp >= 1 && near.atk >= 1);
    }

    #[test]
    fn the_harsh_biomes_stay_out_of_the_shallow_ring() {
        let b = Balance::load_default().unwrap();
        // Measured: desert and tundra lead with armoured bruisers that a level-1 solo
        // beat 0 times in 15, so they are gated outward rather than flattened.
        for (biome, gate) in &b.biome_gate {
            for d in [0i64, 25, 60] {
                if *gate > d {
                    for seed in 0..40u64 {
                        assert_ne!(
                            section_biome(&b, seed, 0, d, None, false),
                            biome.as_str(),
                            "{biome} (gated at {gate}) appeared at d={d}"
                        );
                    }
                }
            }
        }
        // …and the gates never starve the generator of a theme to pick.
        for d in [0i64, 100, 300, 600, 5_000] {
            for seed in 0..20u64 {
                let picked = section_biome(&b, seed, 1, d, None, false);
                assert!(BIOMES.contains(&picked), "picked {picked:?} at d={d}");
            }
        }
        // A tutorial run keeps its own hand-tuned bands regardless of the gates.
        assert_eq!(section_biome(&b, 7, 0, 0, None, true), biome_for_distance(0));
    }

    #[test]
    fn no_elite_champions_in_the_first_ring() {
        let b = Balance::load_default().unwrap();
        let min = b.encounters.elite_min_distance;
        assert!(min > 0, "an ungated elite can be a new party's second fight");
        for seed in 0..60u64 {
            let arena = Arena::generate(&b, seed, false);
            for m in arena.monsters.iter() {
                let d = m.position.distance_floor();
                if d < min {
                    assert_eq!(
                        m.encounter_class, "standard",
                        "{} ({}) at d={d} is inside the on-ramp",
                        m.monster_kind, m.encounter_class
                    );
                }
            }
        }
    }

    // ---- FS-4: Elite champions + Gatekeeper bosses ----

    #[test]
    fn only_a_reward_spike_encounter_drops_the_special_tiers() {
        let b = Balance::load_default().unwrap();
        let d = b.world_scaling.red_chest_floor_distance + 500;

        // Ordinary creatures only ever drop STANDARD. Ephemeral is the strongest gear
        // in the game and insured survives wipes; trash handing out either would make
        // the extract-or-die stake evaporate.
        let trash: Vec<_> = (0..400)
            .filter_map(|s| roll_creature_loot(&b, d, 1, 1.0, s).gear)
            .collect();
        assert!(!trash.is_empty(), "a standard creature should still drop gear at all");
        assert!(
            trash.iter().all(|g| g.insurance == Insurance::Standard),
            "trash must never drop ephemeral or insured gear"
        );

        // A gatekeeper's reward spike yields all three, with the special tiers rare.
        let mult = b.encounters.gatekeeper_loot_mult;
        let drops: Vec<_> = (0..600)
            .filter_map(|s| roll_creature_loot(&b, d, 1, mult, s).gear)
            .collect();
        let count = |t: Insurance| drops.iter().filter(|g| g.insurance == t).count();
        assert!(count(Insurance::Ephemeral) > 0, "a champion should sometimes drop ephemeral");
        assert!(count(Insurance::Insured) > 0, "a champion should sometimes drop insured");
        assert!(
            count(Insurance::Standard) > count(Insurance::Ephemeral) + count(Insurance::Insured),
            "the special tiers must stay the exception, not the rule"
        );
    }

    #[test]
    fn the_tiers_are_powered_in_the_order_they_are_risky() {
        // Ephemeral > insured > standard. You cannot keep ephemeral at all, and
        // insured is always dying, so both have to out-hit ordinary kit or the risk
        // buys nothing. Averaged across seeds because each roll carries its own
        // jitter/rarity, which would otherwise drown a per-drop comparison.
        let b = Balance::load_default().unwrap();
        let d = b.world_scaling.red_chest_floor_distance + 500;
        let mult = b.encounters.gatekeeper_loot_mult;
        let mut totals = std::collections::HashMap::new();
        let mut counts = std::collections::HashMap::new();
        for seed in 0..4000u64 {
            if let Some(g) = roll_creature_loot(&b, d, 1, mult, seed).gear {
                let power = g.atk_bonus + g.def_bonus + g.spd_bonus;
                *totals.entry(g.insurance).or_insert(0i64) += power as i64;
                *counts.entry(g.insurance).or_insert(0i64) += 1;
            }
        }
        let mean = |t: Insurance| {
            totals.get(&t).copied().unwrap_or(0) as f64
                / counts.get(&t).copied().unwrap_or(1).max(1) as f64
        };
        let (eph, ins, std_) =
            (mean(Insurance::Ephemeral), mean(Insurance::Insured), mean(Insurance::Standard));
        assert!(eph > ins, "ephemeral should out-roll insured ({eph:.1} vs {ins:.1})");
        assert!(ins > std_, "insured should out-roll standard ({ins:.1} vs {std_:.1})");
    }

    #[test]
    fn forged_gear_is_insured() {
        // You made it at a forge in town, out of materials you carried home. It must
        // not evaporate the first time you die.
        let b = Balance::load_default().unwrap();
        let g = forge_gear(&b, 5, "main_hand", "explorer", "forest", false, 7);
        assert_eq!(g.insurance, Insurance::Insured, "a forged piece is yours to keep");
    }

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

    /// FS-4: the Gatekeeper boss tier escalates with distance, matching the
    /// seam thresholds `push_section` walls (100/300/500/1000/3000) — a
    /// shallow gate reads as a miniboss, a deep one as apocalyptic.
    #[test]
    fn gatekeeper_boss_kind_escalates_with_distance() {
        let miniboss = ["choirmother", "pyrewarden"];
        let dungeon = ["sepulcher", "hollowbishop"];
        let region = ["ironmaw", "weepingcolossus"];
        let biome = ["miredrowned", "ashenleviathan"];
        for seed in 0..20u64 {
            assert!(miniboss.contains(&pick_gatekeeper_boss_kind(100, seed)));
            assert!(dungeon.contains(&pick_gatekeeper_boss_kind(300, seed)));
            assert!(region.contains(&pick_gatekeeper_boss_kind(500, seed)));
            assert!(biome.contains(&pick_gatekeeper_boss_kind(1000, seed)));
            assert!(biome.contains(&pick_gatekeeper_boss_kind(3000, seed)));
        }
    }

    #[test]
    fn elite_boss_kind_is_always_the_elite_tier() {
        for seed in 0..20u64 {
            assert!(["gloamhound", "rustfang"].contains(&pick_elite_boss_kind(seed)));
        }
    }

    #[test]
    fn boss_display_name_covers_every_key() {
        for key in [
            "gloamhound", "rustfang", "choirmother", "pyrewarden", "sepulcher",
            "hollowbishop", "ironmaw", "weepingcolossus", "miredrowned", "ashenleviathan",
        ] {
            assert_ne!(boss_display_name(key), "Unknown Horror", "{key} has a title");
        }
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
            // FS-4: every gatekeeper fights as one of the 8 named Gatekeeper-tier
            // bosses (never the "elite" tier, which is reserved for Elites).
            assert!(!gk.boss_kind.is_empty(), "gatekeeper carries a boss identity");
            assert!(
                !["gloamhound", "rustfang"].contains(&gk.boss_kind.as_str()),
                "gatekeeper doesn't use the elite-tier boss identity: {}", gk.boss_kind
            );
        }
    }

    #[test]
    fn elites_appear_among_mostly_standard_creatures() {
        let b = Balance::load_default().unwrap();
        let mut a = Arena::generate(&b, 3, false);
        a.ensure_frontier(&b, 500.0);
        let elites: Vec<_> = a.monsters.iter().filter(|m| m.encounter_class == "elite").collect();
        let standard = a.monsters.iter().filter(|m| m.encounter_class == "standard").count();
        assert!(!elites.is_empty(), "some creatures are elite champions");
        assert!(standard > elites.len(), "but most creatures are still standard");
        // FS-4: every Elite fights as gloamhound or rustfang (the "elite" tier).
        for elite in &elites {
            assert!(
                ["gloamhound", "rustfang"].contains(&elite.boss_kind.as_str()),
                "elite carries the elite-tier boss identity: {}", elite.boss_kind
            );
        }
    }

    #[test]
    fn the_tutorial_run_has_no_scattered_elites_but_keeps_gate_bosses() {
        let b = Balance::load_default().unwrap();
        let mut a = Arena::generate(&b, 9, true);
        a.ensure_frontier(&b, 400.0);
        // Scattered elite champions stay OFF the tutorial (gentle onboarding)...
        assert!(a.monsters.iter().all(|m| m.encounter_class != "elite"),
            "no scattered elites on a first dive");
        // ...but the milestone GATE bosses spawn on every run, tutorial included, so
        // bosses actually appear in normal play (and the perpetual-tutorial demo build).
        assert!(a.monsters.iter().any(|m| m.encounter_class == "gatekeeper"),
            "a gate-boss guards a crossed biome border even on the tutorial");
    }

    #[test]
    fn champions_roll_a_known_affix_and_standards_have_none() {
        let b = Balance::load_default().unwrap();
        let mut a = Arena::generate(&b, 3, false);
        a.ensure_frontier(&b, 500.0);
        let known = ["Swift", "Brutal", "Armored", "Giant", "Vicious"];
        let mut champions = 0;
        for m in &a.monsters {
            match m.encounter_class.as_str() {
                // Champions are the affix carriers — and an undead rite's boss is a
                // champion tier of its own.
                "elite" | "gatekeeper" | "undead_rite" => {
                    assert!(
                        known.contains(&m.affix.as_str()),
                        "champion affix is known: {:?}",
                        m.affix
                    );
                    champions += 1;
                }
                // Pack roles are encounter COMPOSITION, not a champion tier: a
                // leader is bigger than its minions but carries no champion affix.
                "standard" | "leader" | "minion" => {
                    assert!(m.affix.is_empty(), "{} carries no affix", m.encounter_class)
                }
                other => panic!("unknown encounter class {other:?}"),
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

    /// Creatures per unit of ground, ring by ring. The fan's area grows with radius, so
    /// a per-section creature COUNT is not a density — this is what the player actually
    /// experiences walking around out there.
    fn creatures_per_area_by_ring(b: &Balance, seed: u64) -> Vec<f64> {
        let arena = Arena::generate(b, seed, false);
        let half = arena.radial_half();
        [(40.0, 100.0), (100.0, 180.0), (180.0, 280.0)]
            .iter()
            .map(|&(lo, hi)| {
                let n = arena
                    .monsters
                    .iter()
                    .filter(|m| {
                        let r = m.position.x.hypot(m.position.y);
                        r >= lo && r < hi
                    })
                    .count() as f64;
                n / ((hi * hi - lo * lo) * half)
            })
            .collect()
    }

    /// Obstacles per 1000 u² of world in a radius ring, over a world streamed out well
    /// past the point where `maze_radial_scale_cap` starts to bind. A per-section COUNT is
    /// not a density, and the ring's area grows quadratically — this is what a player
    /// walking out there actually experiences.
    fn obstacle_density_by_ring(b: &Balance, seed: u64, rings: &[(f64, f64)]) -> Vec<f64> {
        // ONE biome for every section. Each biome has its own fill multiplier (forest 7.0
        // against tundra 1.6), and which biome a ring happens to draw is a per-seed
        // accident — so an unpinned ratio measures the biome lottery as much as the
        // compensation, and a deep tundra ring is *correctly* thinner than a shallow
        // forest one. Pinning it is what makes this a measurement of the thing under test.
        let mut a = Arena::generate_with(b, seed, false, Some("forest"));
        let mut reach = 0.0_f64;
        let far = rings.last().map(|r| r.1).unwrap_or(0.0);
        while reach < far {
            reach += 40.0;
            a.ensure_frontier(b, reach);
        }
        let half = a.radial_half();
        rings
            .iter()
            .map(|&(lo, hi)| {
                let n = a
                    .obstacles
                    .iter()
                    .filter(|o| {
                        let r = o.position.x.hypot(o.position.y);
                        r >= lo && r < hi
                    })
                    .count() as f64;
                n / ((hi * hi - lo * lo) * half) * 1000.0
            })
            .collect()
    }

    #[test]
    fn a_dungeon_ring_is_not_a_featureless_plain() {
        // Reported from play: "every biome just kinda looks like a big open field."
        // `dungeon_every = 4` marks every 4th section a procedural dungeon, and the maze
        // fill used to be skipped entirely for one ("rooms-and-corridors INSTEAD of the
        // scattered fill"). That was true when a section was a 20-tile corridor with three
        // rooms in it; after WG-4 a section is an annular band spanning the whole 340° arc,
        // so its two divider walls are a rounding error across it. Measured at seed 424242
        // out to d1700: dungeon sections averaged 0.167 obstacles per 1000 u² against 4.92
        // for ordinary ones, and section 16 (forest, the DENSEST fill multiplier in the
        // table) held 29 props across 900,893 u² — a mean spacing of 88 tiles. A quarter of
        // the world was open ground, and you cross one every fourth section.
        let b = Balance::load_default().unwrap();
        let mut a = Arena::generate(&b, 424242, false);
        let mut reach = 0.0_f64;
        while reach < 1700.0 {
            reach += 40.0;
            a.ensure_frontier(&b, reach);
        }
        let half = a.radial_half();
        let density = |ar: &Area| {
            let (lo, hi) = (ar.start_x, ar.end_x);
            let n = a
                .obstacles
                .iter()
                .filter(|o| {
                    let r = o.position.x.hypot(o.position.y);
                    r >= lo && r < hi
                })
                .count() as f64;
            n / ((hi * hi - lo * lo) * half) * 1000.0
        };
        // Skip the spawn section: it is a 13-unit ring the path tube almost entirely
        // fills, and it is deliberately gentle.
        let (mut dgn, mut normal) = (Vec::new(), Vec::new());
        for ar in a.areas.iter().filter(|ar| ar.index > 0) {
            if ar.dungeon {
                dgn.push(density(ar));
            } else {
                normal.push(density(ar));
            }
        }
        assert!(!dgn.is_empty(), "the sweep has to actually contain a dungeon section");
        let mean = |v: &[f64]| v.iter().sum::<f64>() / v.len() as f64;
        let (d, n) = (mean(&dgn), mean(&normal));
        // A dungeon section may legitimately differ from an ordinary one — it packs
        // creatures denser and holds a guaranteed chest — but it is still a section of its
        // own biome, and it must not be an order of magnitude emptier. The bug sat at 30x.
        assert!(
            d > n * 0.5,
            "a dungeon ring keeps its biome's terrain \
             (dungeon {d:.3} vs ordinary {n:.3} obstacles per 1000 u²)"
        );
    }

    #[test]
    fn the_maze_holds_its_terrain_past_the_compensation_cap() {
        // The density guard that already existed (`the_world_does_not_empty_out_as_it_fans
        // _open`) samples out to r=280 — INSIDE the radius where the arc compensation still
        // holds — so everything past it was unguarded, which is where the whole game
        // actually takes place. Two axes thin the fill and only the arc was ever
        // compensated: `obstacles_per_area` is a count per SECTION, and sections grow from
        // 13 units thick near the hub to 184 by d1560, so the same count also spread over
        // ever more radial extent (`maze_fill_scale` is now the one place both live).
        //
        // Asserted as MEAN PROP SPACING IN TILES rather than as a ratio, for two reasons:
        // spacing is the thing a player actually experiences (a forest with trunks 4 tiles
        // apart is a wood; at 50 it is a plain), and a ratio against the shallow ring
        // measures the deliberate compromise in `maze_radial_scale_cap` rather than the
        // bug. Measured, pinned to forest, mean spacing shallow / mid / deep:
        //
        //   no compensation | 12.3 | 37.8 | 47.2   ← what the arithmetic bug produced
        //   now, cap 24     |  3.0 |  6.5 |  9.4
        //   cap 60          |  3.0 |  4.1 |  6.0   ← not taken; see balance.toml
        //
        // The cap stays at 24 because `ensure_frontier` runs inside the authoritative tick
        // and streaming one deep section is already a 181 ms stall on a 100 ms tick — a
        // bigger cap doubles a stall that is over budget before we touch it. So the deep
        // ring is knowingly still short of the shallow one, and the bar below is a FLOOR
        // that has to hold until `WG-6` spends the fill along the route network instead of
        // across the whole ring. The measured curve lives in balance.toml, once.
        let b = Balance::load_default().unwrap();
        let rings = [(40.0, 100.0), (400.0, 700.0), (900.0, 1200.0)];
        // Mean nearest-neighbour spacing of a Poisson field of density `d` per u².
        let spacing = |per_1k: f64| 0.5 / (per_1k / 1000.0).sqrt();
        // ONE seed, deliberately. Each case streams a world out past the cap's holding
        // radius, which is the most expensive thing in this file — and pinning the biome
        // has already removed the variance a seed sweep would be buying (what a ring
        // draws is the lottery this test exists to hold constant).
        for seed in [424242_u64] {
            for (k, &d) in obstacle_density_by_ring(&b, seed, &rings).iter().enumerate() {
                let sp = spacing(d);
                assert!(
                    sp < 14.0,
                    "seed {seed} ring {k}: a forest has to keep enough trunks to read as \
                     terrain at every depth \
                     (mean prop spacing {sp:.1} tiles, {d:.2} per 1000 u²)"
                );
            }
        }
        // ...and none of that is vacuous: drop the compensation and the deep forest really
        // does open out into a field you can see across, which is the bug as it shipped.
        let mut off = b.clone();
        off.worldgen.maze_radial_scale_cap = 1.0;
        let d = obstacle_density_by_ring(&off, 424242, &rings);
        assert!(
            spacing(d[2]) > 30.0,
            "uncompensated, the deep ring is a plain (mean prop spacing {:.1} tiles)",
            spacing(d[2])
        );
    }

    #[test]
    fn the_world_does_not_empty_out_as_it_fans_open() {
        // Reported from play: "there just aren't enough creatures spawned... I wandered
        // around for a bit and didn't see anything on the screen." Creatures are laid one
        // per `monster_spacing` along the corridor, which is one corridor-WIDTH of content
        // — but WG-4 bends that fixed width into an arc that grows with radius, so the
        // same handful of creatures gets smeared over an ever-larger sector. The count per
        // section was fine; the density collapsed. This pins the density, not the count.
        let b = Balance::load_default().unwrap();
        for seed in [1u64, 7, 42, 9001] {
            let d = creatures_per_area_by_ring(&b, seed);
            assert!(
                d[2] > d[0] * 0.7,
                "seed {seed}: the deep ring keeps its population \
                 (shallow {:.5}/u², deep {:.5}/u²)",
                d[0],
                d[2]
            );
        }
        // ...and the measurement above is not vacuous: turn the compensation off and the
        // density really does fall away, which is the bug as it shipped.
        let mut off = b.clone();
        off.worldgen.creature_radial_lane_cap = 1.0;
        let d = creatures_per_area_by_ring(&off, 42);
        assert!(
            d[2] < d[0] * 0.5,
            "uncompensated, the deep ring thins out (shallow {:.5}/u², deep {:.5}/u²)",
            d[0],
            d[2]
        );
    }

    #[test]
    fn only_a_pack_ever_makes_a_group() {
        // The density fix and the encounter ramp are two halves that have to be joined:
        // `group_around` pulls everything within `[ai] group_radius`, so packing creatures
        // in at the designed density is also a way to silently hand out bigger fights than
        // the ramp promises. Two spawns that are not each other's pack are never within
        // pull range — a fight's size is decided by the ramp table, never by geometry.
        let b = Balance::load_default().unwrap();
        let pull = b.ai.group_radius;
        for seed in [3u64, 11, 42, 777] {
            let mut a = Arena::generate(&b, seed, false);
            a.ensure_frontier(&b, 600.0);
            let solo: Vec<&MonsterSpawn> =
                a.monsters.iter().filter(|m| m.encounter_class == "standard").collect();
            for (n, m) in solo.iter().enumerate() {
                for o in &solo[n + 1..] {
                    let d = m.position.distance_to(&o.position);
                    assert!(
                        d > pull,
                        "seed {seed}: {} and {} are {d:.2} apart and pull each other in",
                        m.monster_kind,
                        o.monster_kind
                    );
                }
            }
        }
    }

    #[test]
    fn spawn_section_keeps_a_creature_free_hub_ring() {
        // Bug fix: procedural area 0 used to place creatures ~2 tiles from the hub, so
        // an aggressive creature closed on the stationary just-spawned player and pulled
        // it into a battle before the player could orient. The spawn section now clears a
        // `hub_safe_radius` ring that MUST exceed aggro_radius. Check across many seeds.
        let b = Balance::load_default().unwrap();
        let safe = b.worldgen.hub_safe_radius;
        assert!(
            safe > b.ai.aggro_radius,
            "hub_safe_radius {safe} must exceed aggro_radius {}",
            b.ai.aggro_radius
        );
        for seed in [1u64, 2, 3, 7, 42, 100, 555, 9001] {
            let mut arena = Arena::generate(&b, seed, false); // non-tutorial → procedural area 0
            arena.add_avatar("p1".into(), 6.0);
            // No creature spawns within aggro range of the hub, so the stationary spawn
            // is never chased or touched on the first tick.
            let nearest = arena
                .monsters
                .iter()
                .map(|m| m.position.x.hypot(m.position.y))
                .fold(f64::MAX, f64::min);
            assert!(
                nearest > b.ai.aggro_radius,
                "seed {seed}: nearest creature {nearest:.1} inside aggro range at spawn"
            );
            assert!(
                arena.check_touch(&std::collections::HashSet::new()).is_none(),
                "seed {seed}: player spawned already in contact with a creature"
            );
        }
    }

    #[test]
    fn city_return_is_the_west_wedge_not_a_straight_line() {
        // Bug fix: `west_return` used a straight `x < border` test, which in the 340°
        // radial fan sliced through explorable western content — walking over to a
        // creature at a west-ish bearing silently extracted the player. Only the empty
        // due-west wedge (beyond the fan's arc, out past the wall ring) returns you now.
        let b = Balance::load_default().unwrap();
        let border = b.worldgen.west_return_border; // -20.0
        let mut arena = Arena::generate(&b, 7, false);
        arena.add_avatar("p1".into(), 6.0);
        let place = |arena: &mut Arena, x: f64, y: f64| {
            arena.avatars[0].position = Position::new(x, y);
        };

        // Fresh spawn at the hub is not returning.
        assert!(!arena.heading_into_city("p1", border));

        // West-ish FAN content: far past `border` in x, but inside the content arc — a
        // legit place to fight. Must NOT extract. (bearing ~150°, radius ~35.)
        let (r, th) = (35.0_f64, 150.0_f64.to_radians());
        place(&mut arena, r * th.cos(), r * th.sin());
        assert!(
            arena.avatars[0].position.x < border,
            "sanity: the test point is genuinely west of the straight border"
        );
        assert!(
            !arena.heading_into_city("p1", border),
            "west-ish fan content must not trigger the city return"
        );

        // Due-west, out past the wall ring → genuinely stepping into the city wedge.
        place(&mut arena, border - 5.0, 0.0);
        assert!(
            arena.heading_into_city("p1", border),
            "walking due-west into the city wedge returns you"
        );

        // Due-west but still inside the wall ring (not out to the gate yet) → not yet.
        place(&mut arena, -(border.abs() * 0.5), 0.0);
        assert!(
            !arena.heading_into_city("p1", border),
            "not out to the gate ring yet — no return"
        );
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

    #[test]
    fn affix_rolls_are_tier_gated_and_deterministic() {
        use meld_proto::affixes::AffixClass;
        let b = Balance::load_default().unwrap();
        let class_of = |a: &meld_proto::affixes::Affix| a.class().unwrap();

        // A shallow legendary rolls only stat affixes: the early game stays a
        // legible ladder (P1-3).
        let mut rng = Rng(42);
        let shallow = roll_affixes(&b, &mut rng, 1, "legendary", false, false, "explorer", "main_hand", "forest");
        assert!(!shallow.is_empty());
        assert!(
            shallow.iter().all(|a| class_of(a) == AffixClass::Stat),
            "shallow roll leaked a non-stat affix: {shallow:?}"
        );

        // Deep rolls reach the build-forming classes.
        let mut seen = std::collections::HashSet::new();
        for seed in 0..400u64 {
            let mut rng = Rng(seed);
            for a in roll_affixes(&b, &mut rng, 12, "legendary", true, false, "explorer", "chest", "ashfall") {
                seen.insert(class_of(&a));
            }
        }
        for want in [AffixClass::Ward, AffixClass::Synergy, AffixClass::Element] {
            assert!(seen.contains(&want), "deep rolls never produced {want:?}");
        }
        // A Keyword affix is class-locked, so it is reachable for its OWN class and for
        // nobody else — ask each one of its owner rather than expecting the Explorer to
        // roll a Psyker's Focus slot.
        for d in meld_proto::affixes::AFFIXES
            .iter()
            .filter(|d| matches!(d.class, AffixClass::Keyword))
        {
            let owner = d.only_class.expect("a keyword affix names its class");
            let key = meld_proto::equipment::class_key(owner);
            let mut found = false;
            for seed in 0..400u64 {
                let mut rng = Rng(seed);
                for slot in ["main_hand", "chest", "accessory"] {
                    if roll_affixes(&b, &mut rng, 12, "legendary", true, false, key, slot, "ashfall")
                        .iter()
                        .any(|a| a.key == d.key)
                    {
                        found = true;
                    }
                }
            }
            assert!(found, "{} never rolled for its own class ({key})", d.key);
        }

        // Common drops stay plain, and the same seed always rolls the same affixes.
        let mut rng = Rng(7);
        assert!(roll_affixes(&b, &mut rng, 12, "common", false, false, "explorer", "main_hand", "forest").is_empty());
        // Same seed, same affixes — loot stays reproducible from the world seed.
        let a = roll_affixes(&b, &mut Rng(5), 12, "legendary", true, false, "explorer", "chest", "ashfall");
        let c = roll_affixes(&b, &mut Rng(5), 12, "legendary", true, false, "explorer", "chest", "ashfall");
        assert_eq!(a, c);
    }

    #[test]
    fn a_keyword_affix_only_rolls_for_its_own_class() {
        let b = Balance::load_default().unwrap();
        for seed in 0..300u64 {
            let mut rng = Rng(seed);
            for a in roll_affixes(&b, &mut rng, 14, "legendary", true, false, "resonant", "main_hand", "tundra") {
                // "of Fury" is a Hunter twist; a Resonant drop must never carry it.
                assert_ne!(a.key, "adrenaline_primed", "seed {seed}");
                assert_ne!(a.key, "focus_slot", "seed {seed}");
            }
        }
    }

    #[test]
    fn a_synergy_affix_always_names_another_class() {
        let b = Balance::load_default().unwrap();
        for seed in 0..300u64 {
            let mut rng = Rng(seed);
            for a in roll_affixes(&b, &mut rng, 14, "legendary", true, false, "shifter", "chest", "mire") {
                if let Some(ally) = &a.ally_class {
                    assert_ne!(ally, "shifter", "a synergy affix asked for its own class");
                    assert!(CLASS_KEYS.contains(&ally.as_str()));
                }
            }
        }
    }

    #[test]
    fn uniques_only_drop_from_a_reward_spike() {
        let b = Balance::load_default().unwrap();
        // Deep, rich, but NOT spiked: thousands of drops and never a unique.
        let mut plain_uniques = 0;
        for seed in 0..1500u64 {
            let loot = roll_creature_loot(&b, 4000, 3, 1.0, seed);
            if let Some(g) = loot.gear {
                if !g.unique_key.is_empty() {
                    plain_uniques += 1;
                }
            }
        }
        assert_eq!(plain_uniques, 0, "a unique dropped without a reward spike");

        // Spiked (elite / Gatekeeper / boss) drops do produce them.
        let mut spiked_uniques = 0;
        for seed in 0..1500u64 {
            let loot = roll_creature_loot(&b, 4000, 3, 3.0, seed);
            if let Some(g) = loot.gear {
                if let Some(u) = meld_proto::uniques::unique(&g.unique_key) {
                    spiked_uniques += 1;
                    // A unique brings its authored identity, not a rolled one.
                    assert_eq!(g.name, u.name);
                    assert_eq!(g.slot, u.slot);
                    assert_eq!(g.affixes, u.rolled());
                    // A class-locked unique never lands on another class's drop.
                    if let Some(only) = u.only_class {
                        assert_eq!(g.class_key, meld_proto::equipment::class_key(only));
                    }
                }
            }
        }
        assert!(spiked_uniques > 0, "spiked drops never produced a unique");
    }

    #[test]
    fn set_pieces_appear_and_always_name_a_real_set() {
        let b = Balance::load_default().unwrap();
        let mut set_pieces = 0;
        for seed in 0..1500u64 {
            let loot = roll_creature_loot(&b, 2000, 3, 1.0, seed);
            if let Some(g) = loot.gear {
                if !g.set_key.is_empty() {
                    set_pieces += 1;
                    assert!(meld_proto::uniques::set(&g.set_key).is_some(), "{}", g.set_key);
                }
            }
        }
        assert!(set_pieces > 0, "no set pieces ever dropped");
        // Shallow drops predate the set floor.
        for seed in 0..400u64 {
            let loot = roll_creature_loot(&b, 320, 3, 1.0, seed);
            if let Some(g) = loot.gear {
                assert!(g.set_key.is_empty(), "a set piece dropped below its tier floor");
            }
        }
    }

    /// Every affix in the registry has to be REACHABLE, or it is a design that exists only
    /// on paper. The two newest — "of the Aegis" (ward) and "of the Furnace" (element power)
    /// — are the reason this is a test: the pool is derived from `AFFIXES`, so adding one
    /// should be enough, and this proves it rather than assuming it.
    /// A slowed party really is slower: `apply_move` normalises only magnitudes ABOVE 1, so a
    /// sub-unit direction is the hook that lets an affliction drag a march. If that ever
    /// changes, being webbed becomes cosmetic and this is what says so.
    #[test]
    fn a_sub_unit_heading_moves_you_less_far() {
        let b = Balance::load_default().unwrap();
        let mut full = Arena::generate(&b, 7, false);
        let mut slow = Arena::generate(&b, 7, false);
        full.add_avatar("p".into(), b.world.avatar_speed_tiles_per_sec);
        slow.add_avatar("p".into(), b.world.avatar_speed_tiles_per_sec);

        for i in 1..=20 {
            full.apply_move("p", 1.0, 0.0, i);
            slow.apply_move("p", 0.55, 0.0, i);
        }
        let far = full.avatar("p").map(|a| a.position.x).unwrap_or(0.0);
        let near = slow.avatar("p").map(|a| a.position.x).unwrap_or(0.0);
        assert!(
            near < far,
            "a 0.55 heading travelled {near}, the same as a full one ({far}) — a slow would be \
             cosmetic"
        );
        assert!(near > 0.0, "it should still move, just less");
    }

    /// GR-2: durability is measured in DEATHS, and the average is the number the design
    /// asks for. Held as a distribution rather than a value, because the point is that
    /// no two pieces are the same and yet the mean is still what was tuned.
    #[test]
    fn a_piece_of_gear_survives_about_three_deaths_and_no_two_are_alike() {
        let b = Balance::load_default().unwrap();
        let loss = b.loot.durability_loss_per_fall;
        assert!(loss > 0, "a flat loss of 0 would make durability infinite");
        let deaths = |points: i32| (points + loss - 1) / loss;

        let mut seen: Vec<i32> = Vec::new();
        for seed in 0..500u64 {
            let mut rng = Rng(seed);
            seen.push(roll_durability(&b, &mut rng, &[]));
        }
        let lives: Vec<i32> = seen.iter().map(|p| deaths(*p)).collect();
        let mean = lives.iter().sum::<i32>() as f64 / lives.len() as f64;
        assert!(
            (2.5..=3.6).contains(&mean),
            "a plain piece should average about three deaths, got {mean:.2}"
        );
        let distinct: std::collections::HashSet<i32> = seen.iter().copied().collect();
        assert!(
            distinct.len() > 20,
            "every piece rolled the same durability ({} distinct) — the jitter is dead",
            distinct.len()
        );
        assert!(
            lives.iter().any(|d| *d < 3) && lives.iter().any(|d| *d > 3),
            "the spread collapsed onto the mean: {:?}..{:?}",
            lives.iter().min(),
            lives.iter().max()
        );

        // Masterwork is worth real deaths, not a rounding difference — otherwise
        // "exceptionally crafted" is a name on a piece that wears out with the rest.
        let fine = aff::Affix {
            key: "masterwork".to_string(),
            magnitude: b.affix.masterwork_durability_pct,
            element: None,
            ally_class: None,
        };
        let mut better = 0;
        for seed in 0..500u64 {
            let plain = deaths(roll_durability(&b, &mut Rng(seed), &[]));
            let made = deaths(roll_durability(&b, &mut Rng(seed), std::slice::from_ref(&fine)));
            assert!(made >= plain, "masterwork made a piece flimsier at seed {seed}");
            if made > plain {
                better += 1;
            }
        }
        assert!(
            better > 400,
            "masterwork bought an extra death only {better}/500 times"
        );
    }

    /// The loot pool may only hold classes somebody can field, and must hold ALL of them.
    /// It carried five classes with no kit and omitted the **Hunter** — so a Hunter could
    /// never find gear, and five drops in twelve were wearable by nobody alive. Asked of
    /// the unlock registry rather than of a second hand-written list, because a second list
    /// is how this happened.
    /// AN EPHEMERAL DROP IS ALWAYS A BUILD. Two independent rolls (rarity, insurance) meant
    /// the strongest tier in the game could come up **common** — and a common carries zero
    /// affixes, so the piece that burns twice over offered one inflated number and nothing
    /// else. Floored at rare, and it rolls `count_ephemeral_bonus` MORE lines than its
    /// rarity alone would give it, which is what makes it defining rather than merely big.
    #[test]
    fn an_ephemeral_drop_is_always_a_build() {
        let b = Balance::load_default().unwrap();
        let mut seen = 0;
        for seed in 0..6000u64 {
            // A reward spike: only an elite / Gatekeeper / rite / chest can yield the tier.
            let Some(g) = roll_creature_loot(&b, 1200, 3, 3.0, seed).gear else { continue };
            if g.insurance != Insurance::Ephemeral {
                continue;
            }
            seen += 1;
            assert_ne!(g.rarity, "common", "an ephemeral piece with no affixes at all");
            assert!(
                !g.affixes.is_empty(),
                "ephemeral {} rolled no affixes - it burns twice and does nothing",
                g.name
            );
        }
        assert!(seen > 0, "no ephemeral gear ever dropped from a spiked kill");
    }

    /// And the bonus is real: same rarity, same everything, MORE lines.
    #[test]
    fn ephemeral_rolls_a_wider_build_than_the_piece_beside_it() {
        let b = Balance::load_default().unwrap();
        let bonus = b.affix.count_ephemeral_bonus;
        assert!(bonus > 0, "the ephemeral tier buys no extra affixes at all");
        for seed in 0..200u64 {
            let plain =
                roll_affixes(&b, &mut Rng(seed), 20, "epic", false, false, "hunter", "chest", "ashfall");
            let ephem =
                roll_affixes(&b, &mut Rng(seed), 20, "epic", false, true, "hunter", "chest", "ashfall");
            // `roll_affixes` skips a duplicate key rather than re-drawing, so the counts are
            // an upper bound each - what must hold is that the ephemeral piece is never the
            // NARROWER of the two, and that it is genuinely wider on average.
            assert!(
                ephem.len() >= plain.len(),
                "seed {seed}: ephemeral rolled {} lines against a plain {}",
                ephem.len(),
                plain.len()
            );
        }
        let width = |eph: bool| -> usize {
            (0..400u64)
                .map(|s| {
                    roll_affixes(&b, &mut Rng(s), 20, "epic", false, eph, "hunter", "chest", "ashfall")
                        .len()
                })
                .sum()
        };
        assert!(
            width(true) > width(false),
            "the ephemeral bonus never widened a single roll"
        );
    }

    /// EVERY FIELDABLE CLASS HAS A KEYWORD AFFIX, and exactly one. The class-mechanic lane
    /// was a two-class feature — the Hunter's Adrenaline and the Psyker's Focus slot — so
    /// six of eight classes drew from a pool with no twist in it, and the most characterful
    /// affix class was one most heroes could never find. Asked of the roster rather than a
    /// hand-written list, because that is how it came to be two in the first place.
    /// A RAID BOSS SAYS SO — in its size, in its name, and (through the tag) on screen.
    /// The three come from one number, so a four-party wall can never be labelled as a
    /// two-party one: that mismatch is the whole bug, in which every gatekeeper was
    /// four-party-sized and none of them mentioned it.
    #[test]
    fn a_raid_sized_boss_carries_its_scale_in_both_hp_and_name() {
        let b = Balance::load_default().unwrap();
        let base = MonsterSpawn::build(&b, "m".into(), "forest_bloom_stalker", Position::new(600.0, 0.0), 7);
        for parties in 1..=meld_proto::warbands::max_parties() {
            let mut m = base.clone();
            m.scale_to_warband(parties);
            assert_eq!(m.expects_parties, parties);
            let want = (base.max_hp as f64 * parties.max(1) as f64).round() as i32;
            assert_eq!(m.max_hp, want, "{parties} parties did not scale the wall");
            assert_eq!(m.hp, m.max_hp, "a scaled boss must start full");
            // XP rides it too: a fight sized for four parties that paid one party's XP
            // would make the raid the WORST use of everybody's time.
            assert!(m.xp_reward >= base.xp_reward);
            let title = meld_proto::warbands::title(parties);
            assert_eq!(title.is_empty(), parties == 1, "{parties} parties: title {title:?}");
        }
        // Attack deliberately does NOT scale: a raid boss is a longer fight for more
        // people, not one that one-shots whoever arrives first.
        let mut four = base.clone();
        four.scale_to_warband(4);
        assert_eq!(four.atk, base.atk, "scaling by parties must not raise its attack");
    }

    /// The roll stays inside the ladder and leaves most doors ordinary — a raid on every
    /// seam is not an event, it is a toll.
    #[test]
    fn the_raid_roll_is_rare_and_never_leaves_the_ladder() {
        let b = Balance::load_default().unwrap();
        let enc = &b.encounters;
        let mut raids = 0;
        for seed in 0..4000u64 {
            let n = roll_warband(enc, seed);
            assert!(
                (1..=meld_proto::warbands::max_parties()).contains(&n),
                "rolled {n} parties, off the ladder"
            );
            if n > 1 {
                raids += 1;
            }
        }
        let rate = raids as f64 / 4000.0;
        assert!(rate > 0.05, "raids never happen ({rate})");
        assert!(rate < 0.5, "a raid on half the doors is a toll, not an event ({rate})");
    }

    #[test]
    fn every_class_has_exactly_one_keyword_affix() {
        for key in CLASS_KEYS {
            let class = eq::class_from_key(key).expect("a fieldable class parses");
            let mine: Vec<&str> = meld_proto::affixes::AFFIXES
                .iter()
                .filter(|d| matches!(d.class, meld_proto::affixes::AffixClass::Keyword))
                .filter(|d| d.only_class == Some(class))
                .map(|d| d.key)
                .collect();
            assert_eq!(
                mine.len(),
                1,
                "{key} owns {mine:?} keyword affixes - every class gets exactly one twist"
            );
        }
        // And no keyword belongs to a class nobody can field.
        for d in meld_proto::affixes::AFFIXES
            .iter()
            .filter(|d| matches!(d.class, meld_proto::affixes::AffixClass::Keyword))
        {
            let owner = d.only_class.expect("a keyword affix names its class");
            assert!(
                CLASS_KEYS.contains(&eq::class_key(owner)),
                "{} belongs to {owner:?}, who cannot be fielded",
                d.key
            );
        }
    }

    /// A PRIZE IS RARER THAN FILLER. A flat pool made `brand` — which decides what damage
    /// type your attacks ARE — exactly as common as `masterwork`, extra durability: measured
    /// at 32-34% each, every key identical, so a wide roll was more random lines rather than
    /// a build worth chasing. The ORDERING is what is asserted, not the rates: the weights
    /// are `[TUNABLE]` and the shape is the rule.
    #[test]
    fn a_prize_affix_is_rarer_than_filler() {
        use meld_proto::affixes::AffixClass;
        let b = Balance::load_default().unwrap();
        let mut hits: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
        let rolls = 4000u64;
        for s in 0..rolls {
            for aff in
                roll_affixes(&b, &mut Rng(s), 20, "legendary", false, false, "hunter", "main_hand", "ashfall")
            {
                let cls = meld_proto::affixes::find(&aff.key).map(|d| d.class);
                *hits.entry(format!("{cls:?}")).or_default() += 1;
            }
        }
        let rate = |c: AffixClass| -> f64 {
            let n = meld_proto::affixes::AFFIXES.iter().filter(|d| d.class == c).count().max(1);
            hits.get(&format!("{:?}", Some(c))).copied().unwrap_or(0) as f64 / n as f64
        };
        // Per-affix rates, so a class with more members is not credited for its size.
        let (stat, quality, ward, element, keyword, synergy) = (
            rate(AffixClass::Stat),
            rate(AffixClass::Quality),
            rate(AffixClass::Ward),
            rate(AffixClass::Element),
            rate(AffixClass::Keyword),
            rate(AffixClass::Synergy),
        );
        assert!(stat > ward, "stat {stat} should be commoner filler than a ward {ward}");
        assert!(quality > ward, "masterwork {quality} should be commoner than a ward {ward}");
        assert!(ward > element, "a ward {ward} should be commoner than an element {element}");
        assert!(
            element > keyword && element > synergy,
            "the build-defining classes must be the rarest: element {element}, keyword \
             {keyword}, synergy {synergy}"
        );
        assert!(keyword > 0.0 && synergy > 0.0, "a prize that never drops is not a prize");
    }

    /// THE COUNT IS THE COUNT. The draw used to `continue` past a duplicate key, silently
    /// eating the line — so a nominal 5 delivered 4.29 and a nominal 6 delivered 4.97, and
    /// the gap grew with the count, which made `count_ephemeral_bonus` mean less the more of
    /// it you asked for. Drawing without replacement is exact.
    #[test]
    fn every_line_a_piece_is_owed_actually_lands() {
        let b = Balance::load_default().unwrap();
        for (rarity, sig, eph) in [
            ("rare", false, false),
            ("legendary", false, false),
            ("legendary", false, true),
            ("legendary", true, true),
        ] {
            let want = b.affix.count_for(rarity, sig, eph);
            for s in 0..500u64 {
                let got =
                    roll_affixes(&b, &mut Rng(s), 20, rarity, sig, eph, "hunter", "main_hand", "ashfall");
                assert_eq!(
                    got.len(),
                    want,
                    "{rarity} sig={sig} eph={eph} seed {s}: owed {want} lines, landed {}",
                    got.len()
                );
                // Still never the same key twice.
                let mut keys: Vec<&str> = got.iter().map(|a| a.key.as_str()).collect();
                keys.sort();
                let before = keys.len();
                keys.dedup();
                assert_eq!(keys.len(), before, "a piece rolled the same affix twice");
            }
        }
        // A pool smaller than the count caps at the pool, rather than looping forever.
        let shallow = roll_affixes(&b, &mut Rng(1), 1, "legendary", true, true, "hunter", "chest", "forest");
        assert!(!shallow.is_empty() && shallow.len() <= 4, "tier-1 pool is 3 keys: {shallow:?}");
    }

    #[test]
    fn every_fieldable_class_can_find_gear() {
        let owned: Vec<String> = meld_proto::unlocks::UNLOCKS.iter().map(|u| u.key.to_string()).collect();
        let fieldable = meld_proto::unlocks::owned_classes(&owned);
        let want: std::collections::HashSet<&str> =
            fieldable.iter().map(|c| meld_proto::equipment::class_key(*c)).collect();
        let have: std::collections::HashSet<&str> = CLASS_KEYS.iter().copied().collect();
        assert_eq!(
            have, want,
            "the gear pool and the fieldable roster disagree - a class in one and not the \
             other is either loot nobody can wear or a hero that can never find any"
        );
        // And every one of them has real nouns, or its drops are all called "Trinket".
        for key in CLASS_KEYS {
            for slot in meld_proto::equipment::SLOTS {
                assert_ne!(
                    class_slot_noun(key, slot),
                    "Trinket",
                    "{key}/{slot} has no authored noun"
                );
            }
        }
    }

    #[test]
    fn every_affix_can_actually_roll_on_something() {
        let b = Balance::load_default().unwrap();
        let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
        for seed in 0..4000u64 {
            let mut rng = Rng(seed);
            for slot in ["main_hand", "chest", "accessory"] {
                for class in CLASS_KEYS {
                    for aff in roll_affixes(&b, &mut rng, 40, "legendary", true, false, class, slot, "ashfall") {
                        seen.insert(aff.key.clone());
                    }
                }
            }
        }
        for d in meld_proto::affixes::AFFIXES {
            assert!(
                seen.contains(d.key),
                "{} ({}) never rolled on anything — it is unreachable loot",
                d.key,
                d.suffix
            );
        }
    }

    /// An element affix names the element it answers, or it is a resistance to nothing.
    #[test]
    fn an_element_affix_always_names_its_element() {
        let b = Balance::load_default().unwrap();
        for seed in 0..500u64 {
            let mut rng = Rng(seed);
            for aff in roll_affixes(&b, &mut rng, 30, "epic", false, false, "psyker", "main_hand", "tundra") {
                let Some(d) = meld_proto::affixes::find(&aff.key) else { continue };
                if matches!(d.class, meld_proto::affixes::AffixClass::Element) {
                    let el = aff.element.as_deref().unwrap_or("");
                    assert!(
                        meld_proto::enums::DamageType::from_wire(el).is_some(),
                        "{} rolled with element {el:?}",
                        aff.key
                    );
                }
            }
        }
    }

    #[test]
    fn only_a_weapon_can_brand_your_attacks() {
        let b = Balance::load_default().unwrap();
        let mut branded_hands = 0;
        for seed in 0..600u64 {
            for slot in ["main_hand", "off_hand"] {
                let mut rng = Rng(seed);
                for a in roll_affixes(&b, &mut rng, 12, "legendary", true, false, "explorer", slot, "ashfall") {
                    if a.key == "brand" {
                        branded_hands += 1;
                        assert!(a.element.is_some(), "a brand with no element");
                    }
                }
            }
            // Armour and accessories never decide what your swing is.
            for slot in ["head", "chest", "legs", "accessory"] {
                let mut rng = Rng(seed);
                for a in roll_affixes(&b, &mut rng, 12, "legendary", true, false, "explorer", slot, "ashfall") {
                    assert_ne!(a.key, "brand", "{slot} rolled a brand");
                }
            }
        }
        assert!(branded_hands > 0, "no weapon ever rolled a brand");
    }

    #[test]
    fn a_touched_leader_drags_its_whole_pack_into_the_fight() {
        let b = Balance::load_default().unwrap();
        let mut a = Arena::generate(&b, 4, false);
        a.ensure_frontier(&b, 900.0);
        let leaders: Vec<usize> = a
            .monsters
            .iter()
            .enumerate()
            .filter(|(_, m)| m.encounter_class == "leader")
            .map(|(i, _)| i)
            .collect();
        assert!(!leaders.is_empty(), "no packs were spawned at all");

        // The whole point: touching the leader pulls the minions in, so a fight is
        // a party-of-four versus a GROUP instead of versus one creature.
        let mut biggest = 0;
        for &li in &leaders {
            let group = a.group_around(li);
            biggest = biggest.max(group.len());
            assert!(group.contains(&li));
        }
        assert!(
            biggest >= 4,
            "the largest pack pulled only {biggest} creatures into a fight"
        );

        // Minions exist, are visibly lesser, and sit near a leader.
        let minions: Vec<&MonsterSpawn> = a
            .monsters
            .iter()
            .filter(|m| m.encounter_class == "minion")
            .collect();
        assert!(!minions.is_empty(), "packs spawned no minions");
        let leader_hp = a.monsters[leaders[0]].max_hp;
        let worst_minion = minions.iter().map(|m| m.max_hp).min().unwrap();
        assert!(
            leader_hp > worst_minion,
            "the big one should outclass the little ones: {leader_hp} vs {worst_minion}"
        );

        // And the fight is genuinely harder, not just longer: a pack brings more
        // total HP *and* more total attack per round than the lone creature it
        // replaced — the second is what makes a player reach for a heal.
        let lone = a
            .monsters
            .iter()
            .find(|m| m.encounter_class == "standard")
            .expect("a lone creature exists");
        let best = leaders
            .iter()
            .map(|&li| {
                let g = a.group_around(li);
                (
                    g.iter().map(|&i| a.monsters[i].max_hp).sum::<i32>(),
                    g.iter().map(|&i| a.monsters[i].atk).sum::<i32>(),
                )
            })
            .max_by_key(|(hp, _)| *hp)
            .unwrap();
        assert!(best.0 > lone.max_hp, "pack hp {} vs lone {}", best.0, lone.max_hp);
        assert!(
            best.1 > lone.atk * 2,
            "a pack should out-damage a lone creature by a wide margin: {} vs {}",
            best.1,
            lone.atk
        );
    }

    #[test]
    fn a_pack_pays_more_xp_than_the_lone_creature_it_replaced() {
        let b = Balance::load_default().unwrap();
        let mut a = Arena::generate(&b, 4, false);
        a.ensure_frontier(&b, 900.0);
        let li = a
            .monsters
            .iter()
            .position(|m| m.encounter_class == "leader")
            .expect("a pack exists");
        // The battle sums every creature's reward, so clearing a pack pays for the
        // whole pack — which is what makes deep fights worth their risk.
        let pack_xp: i64 = a
            .group_around(li)
            .iter()
            .map(|&i| a.monsters[i].xp_reward)
            .sum();
        let lone = a
            .monsters
            .iter()
            .find(|m| m.encounter_class == "standard")
            .map(|m| m.xp_reward)
            .unwrap_or(0);
        assert!(
            pack_xp > lone,
            "a pack ({pack_xp}) should out-pay a lone creature ({lone})"
        );
    }

    #[test]
    fn onboarding_never_meets_a_pack() {
        let b = Balance::load_default().unwrap();
        // The tutorial world is calm everywhere…
        let tut = Arena::generate(&b, 7, true);
        assert!(
            tut.monsters.iter().all(|m| m.encounter_class != "leader"
                && m.encounter_class != "minion"),
            "the tutorial spawned a pack"
        );
        // …and in a normal world the spawn section (i == 0) stays a safe first step.
        let a = Arena::generate(&b, 7, false);
        let section0_end = a.monsters
            .iter()
            .filter(|m| m.area_min_x == 0.0)
            .all(|m| m.encounter_class != "leader" && m.encounter_class != "minion");
        assert!(section0_end, "the spawn section spawned a pack");
    }

    #[test]
    fn the_encounter_ramp_climbs_band_by_band() {
        // The readout Don asked for: average encounter size per distance band, so the
        // ramp is visible rather than asserted. Run with
        // `cargo test -p meld-world the_encounter_ramp -- --nocapture`.
        let b = Balance::load_default().unwrap();
        let bands: [(f64, f64, &str); 5] = [
            (0.0, 150.0, "0-150   duels"),
            (150.0, 250.0, "150-250 duos"),
            (250.0, 350.0, "250-350 triples"),
            (350.0, 500.0, "350-500 quads"),
            (500.0, 1200.0, "500+    deep"),
        ];
        // Pool several seeds per band so one unlucky world does not read as the curve.
        let mut avg_by_band = Vec::new();
        for (lo, hi, label) in bands {
            let (mut creatures, mut fights, mut biggest, mut mixed_fights) = (0usize, 0usize, 0usize, 0usize);
            for seed in 0..6u64 {
                let mut a = Arena::generate(&b, 40 + seed, false);
                a.ensure_frontier(&b, hi + 200.0);
                let mut seen = std::collections::HashSet::new();
                for (i, m) in a.monsters.iter().enumerate() {
                    if seen.contains(&i) || m.defeated {
                        continue;
                    }
                    let d = m.position.x.hypot(m.position.y);
                    let g = a.group_around(i);
                    for &j in &g {
                        seen.insert(j);
                    }
                    if d < lo || d >= hi {
                        continue;
                    }
                    let kinds: std::collections::HashSet<&str> =
                        g.iter().map(|&j| a.monsters[j].monster_kind.as_str()).collect();
                    if kinds.len() > 1 {
                        mixed_fights += 1;
                    }
                    creatures += g.len();
                    fights += 1;
                    biggest = biggest.max(g.len());
                }
            }
            let avg = creatures as f64 / fights.max(1) as f64;
            println!(
                "{label:<16} {fights:>4} fights, avg {avg:.2} creatures, biggest {biggest}, {mixed_fights} mixed"
            );
            avg_by_band.push((label, avg, biggest, mixed_fights, fights));
        }

        // The shallow band stays a duel — a new player learns the ATB one creature at
        // a time (the `[[encounters.group_ramp]]` table starts at 150).
        assert!(
            avg_by_band[0].1 < 1.35,
            "the first 150 tiles should be near-duels: avg {:.2}",
            avg_by_band[0].1
        );
        // And it climbs from there, band by band, without needing every band to be
        // strictly bigger than the last (bands overlap as sections straddle a border).
        assert!(
            avg_by_band[1].1 > avg_by_band[0].1,
            "duos did not exceed duels: {:.2} vs {:.2}",
            avg_by_band[1].1,
            avg_by_band[0].1
        );
        assert!(
            avg_by_band[3].1 > avg_by_band[1].1,
            "quads did not exceed duos: {:.2} vs {:.2}",
            avg_by_band[3].1,
            avg_by_band[1].1
        );
        // Group SIZE ceiling climbs with the table.
        assert!(avg_by_band[1].2 >= 2, "no duo ever formed");
        assert!(avg_by_band[3].2 >= 4, "no quad ever formed");
        // Mixing ramps too. A duo band fight can still READ as mixed when an
        // unrelated neighbour falls inside the grouping radius — that is proximity,
        // not the pack's own `mixed_chance` — so compare shares rather than assert an
        // absolute zero.
        let mixed_share = |i: usize| avg_by_band[i].3 as f64 / avg_by_band[i].4.max(1) as f64;
        assert!(
            mixed_share(1) < 0.10,
            "duos should be near-uniform species: {:.0}% mixed",
            mixed_share(1) * 100.0
        );
        assert!(
            mixed_share(2) > mixed_share(1) * 2.0,
            "species mixing did not ramp at the triple band: {:.0}% vs {:.0}%",
            mixed_share(2) * 100.0,
            mixed_share(1) * 100.0
        );
    }

    #[test]
    fn a_better_smith_forges_deeper_and_more_consistently() {
        let b = Balance::load_default().unwrap();
        let forge_at = |level: i32, seed: u64| {
            forge_gear(&b, level, "main_hand", "explorer", "forest", false, seed)
        };

        // Forging level sets the tier a smith can reach.
        let apprentice = forge_at(1, 7);
        let master = forge_at(10, 7);
        assert!(
            master.tier > apprentice.tier,
            "a master should forge deeper: {} vs {}",
            master.tier,
            apprentice.tier
        );

        // …and how tightly the stat rolls. Compare the SPREAD across many seeds at
        // each level: an apprentice's work is erratic, a master's is dependable.
        let spread = |level: i32| -> f64 {
            let stats: Vec<i32> = (0..200u64).map(|s| forge_at(level, s).atk_bonus).collect();
            let lo = *stats.iter().min().unwrap() as f64;
            let hi = *stats.iter().max().unwrap() as f64;
            let mid = stats.iter().sum::<i32>() as f64 / stats.len() as f64;
            (hi - lo) / mid.max(1.0)
        };
        assert!(
            spread(10) < spread(1),
            "a master smith is not more consistent: {:.3} vs {:.3}",
            spread(10),
            spread(1)
        );

        // A forged piece is a real, wearable piece of its class's kit.
        let piece = forge_at(6, 3);
        assert_eq!(piece.class_key, "explorer");
        assert_eq!(piece.slot, "main_hand");
        assert!(piece.atk_bonus > 0 && piece.max_durability > 0);
        let fam = meld_proto::equipment::ItemFamily::from_wire(&piece.family)
            .expect("a forged weapon has a family");
        assert!(
            meld_proto::equipment::allows_family(meld_proto::enums::CharacterClass::Explorer, fam),
            "forged a family its own class cannot hold: {:?}",
            fam
        );
        // Crafted gear is never a unique or a set piece — those are chased, not made.
        assert!(piece.unique_key.is_empty() && piece.set_key.is_empty());
    }

    #[test]
    fn a_trophy_quenched_into_the_piece_reaches_past_the_smiths_own_level() {
        let b = Balance::load_default().unwrap();
        for level in [1, 4, 10, 20] {
            let plain = forge_gear(&b, level, "main_hand", "explorer", "forest", false, 5);
            let quenched = forge_gear(&b, level, "main_hand", "explorer", "forest", true, 5);
            assert_eq!(
                quenched.tier,
                plain.tier + b.forge.catalyst_tier_bonus,
                "the catalyst bought no reach at level {level}"
            );
            assert_eq!(plain.rarity, "rare");
            assert_eq!(quenched.rarity, "epic", "a catalyzed piece rolls the better pool");
            // The stat scales off `tier.max(1)`, so at level 1 (tier 0 → 1) the
            // catalyst buys affix quality rather than a bigger number. Everywhere
            // above that it buys both.
            if plain.tier >= 1 {
                assert!(quenched.atk_bonus > plain.atk_bonus, "level {level}");
            } else {
                assert!(quenched.atk_bonus >= plain.atk_bonus, "level {level}");
            }
        }
    }

    #[test]
    fn trophy_yield_tracks_the_pack_and_the_depth_you_beat() {
        let b = Balance::load_default().unwrap();
        let qty = |dist: i64, count: i32, mult: f64| {
            roll_creature_loot(&b, dist, count, mult, 7).material_qty
        };
        // A carcass each: beating five things yields more parts than beating one.
        assert!(qty(0, 5, 1.0) > qty(0, 1, 1.0));
        // Depth pays, because the deep bands' parts are what the top recipes want.
        assert!(qty(1500, 1, 1.0) > qty(0, 1, 1.0));
        // An elite is worth cutting up; a lone hub creature still leaves something.
        assert!(qty(300, 2, b.encounters.elite_loot_mult) > qty(300, 2, 1.0));
        assert_eq!(qty(0, 1, 1.0), 1, "the first kill of the game should give one");
        assert!(qty(0, 1, 0.0) >= 1, "no encounter yields zero parts");
        // Deterministic: the same fight always butchers the same, whatever the seed,
        // so a crafter can count on a plan.
        for seed in 0..50u64 {
            assert_eq!(roll_creature_loot(&b, 800, 3, 1.0, seed).material_qty, qty(800, 3, 1.0));
        }
    }

    #[test]
    fn every_combat_drop_is_a_registered_trophy() {
        // The registry is the sink's contract: a drop key that isn't in it is loot
        // no recipe, no Forge and no vendor can accept.
        use meld_proto::materials::{is_class, material, MaterialClass};
        let b = Balance::load_default().unwrap();
        for d in [0i64, 50, 150, 350, 700, 1500, 9000] {
            let key = combat_material_for_biome(d);
            let def = material(key).unwrap_or_else(|| panic!("{key} is not a registered material"));
            assert!(is_class(key, MaterialClass::Trophy), "{key} is not a trophy");
            assert!(
                !meld_proto::consumables::recipes_consuming(key).is_empty(),
                "{key} has no recipe to be spent in"
            );
            assert!(b.material.sale_price(def.tier, def.class.wire(), 1) > 0);
        }
        for node in BIOMES.iter().flat_map(|b| resources_for_biome(b)) {
            assert!(material(node).is_some(), "harvest node {node} is not a registered material");
        }
    }

    #[test]
    fn forging_armour_rolls_a_weight_its_class_can_wear() {
        let b = Balance::load_default().unwrap();
        for (class, slot) in [("phoenix_guard", "chest"), ("psyker", "head"), ("shifter", "legs")] {
            let piece = forge_gear(&b, 8, slot, class, "tundra", false, 11);
            let w = meld_proto::equipment::ArmorWeight::from_wire(&piece.armor_weight)
                .expect("forged armour has a weight");
            let c = meld_proto::equipment::class_from_key(class).unwrap();
            assert!(
                meld_proto::equipment::allows_weight(c, w),
                "{class} cannot wear its own forged {slot}: {w:?}"
            );
            assert!(piece.def_bonus > 0, "armour should defend");
            assert!(piece.family.is_empty(), "armour carries no weapon family");
        }
    }

    #[test]
    fn a_reroll_is_another_draw_not_a_better_piece() {
        let b = Balance::load_default().unwrap();
        // Same tier and slot, different seeds: the affixes differ, so paying for a
        // reroll is paying for a chance.
        let a = reroll_affixes(&b, 12, "explorer", "main_hand", "ashfall", 1);
        let mut differed = false;
        for seed in 2..40u64 {
            let c = reroll_affixes(&b, 12, "explorer", "main_hand", "ashfall", seed);
            if c != a {
                differed = true;
                break;
            }
        }
        assert!(differed, "every reroll produced the same affixes");

        // The same seed reproduces the same draw (so a bug report is reproducible).
        assert_eq!(
            reroll_affixes(&b, 12, "explorer", "main_hand", "ashfall", 99),
            reroll_affixes(&b, 12, "explorer", "main_hand", "ashfall", 99)
        );

        // A reroll respects the same tier gates as loot: shallow gear cannot reroll
        // into a deep affix class.
        use meld_proto::affixes::AffixClass;
        for seed in 0..60u64 {
            for a in reroll_affixes(&b, 1, "explorer", "main_hand", "forest", seed) {
                assert_eq!(
                    a.class(),
                    Some(AffixClass::Stat),
                    "a tier-1 reroll produced {a:?}"
                );
            }
        }
    }

    #[test]
    fn a_boss_is_what_it_is_not_what_it_rode_in_on() {
        // Lineage is the boss's own: the dead are undead wherever they appear, and
        // the made are constructs. A Choirmother riding a forest beast used to fight
        // as a beast.
        for dead in ["choirmother", "hollowbishop", "miredrowned", "sepulcher"] {
            assert_eq!(abilities::boss_faction(dead), Some("undead"), "{dead}");
        }
        for made in ["ironmaw", "rustfang", "gloamhound", "weepingcolossus", "pyrewarden"] {
            assert_eq!(abilities::boss_faction(made), Some("construct"), "{made}");
        }
        assert_eq!(abilities::boss_faction("ashenleviathan"), Some("wyrm"));
        assert_eq!(abilities::boss_faction("not_a_boss"), None);

        // Every named boss has a lineage, or it would silently keep its host's.
        for key in meld_proto::bosses::keys() {
            assert!(
                abilities::boss_faction(key).is_some(),
                "{key} has no lineage"
            );
        }
        assert_eq!(abilities::bosses_of_faction("undead").len(), 4);
        assert_eq!(abilities::bosses_of_faction("construct").len(), 5);

        // The engine's roster and the shared registry are the SAME ten. The registry is
        // what the client draws a name plate from, so a boss listed here and missing
        // there would fight under a name nobody outside the server ever sees.
        let mut engine: Vec<&str> = abilities::all_bosses().to_vec();
        let mut shared: Vec<&str> = meld_proto::bosses::keys().collect();
        engine.sort_unstable();
        shared.sort_unstable();
        assert_eq!(engine, shared, "the boss roster and the shared registry disagree");
    }

    #[test]
    fn a_boss_grows_a_deeper_kit_and_a_darker_palette() {
        let b = Balance::load_default().unwrap();
        // Deep-gated abilities: the same boss, met further out, has more to throw.
        for boss in ["choirmother", "hollowbishop", "ironmaw", "weepingcolossus"] {
            let shallow = abilities::creature_abilities(boss)
                .iter()
                .filter(|a| a.min_level <= 10)
                .count();
            let deep = abilities::creature_abilities(boss)
                .iter()
                .filter(|a| a.min_level <= 60)
                .count();
            assert!(
                deep > shallow,
                "{boss} fights the same at level 60 as at level 10 ({shallow} vs {deep})"
            );
        }

        // Palette bands escalate with the level a boss is met at, and the deepest
        // band lines up with where the deep abilities come online.
        assert_eq!(abilities::boss_palette_band(1), 0);
        assert_eq!(abilities::boss_palette_band(45), 1);
        assert_eq!(abilities::boss_palette_band(90), 2);
        assert_eq!(abilities::boss_palette_band(200), 3);
        let _ = &b;
    }

    #[test]
    fn the_undead_rite_is_a_boss_with_its_own_dead() {
        let b = Balance::load_default().unwrap();
        let mut found = 0;
        for seed in 0..8u64 {
            let mut a = Arena::generate(&b, 300 + seed, false);
            a.ensure_frontier(&b, 1400.0);
            let rites: Vec<usize> = a
                .monsters
                .iter()
                .enumerate()
                .filter(|(_, m)| m.encounter_class == "undead_rite")
                .map(|(i, _)| i)
                .collect();
            for &ri in &rites {
                found += 1;
                let boss = &a.monsters[ri];
                // It IS undead, and it is one of the named dead.
                assert_eq!(boss.faction, "undead", "a rite led by the living");
                assert_eq!(abilities::boss_faction(&boss.boss_kind), Some("undead"));
                // Never shallow: the rite is a set-piece, not an ambush.
                let d = boss.position.x.hypot(boss.position.y);
                assert!(
                    Scaling::new(&b).tier(d as i64) as i32 >= b.encounters.undead_rite_min_tier,
                    "a rite appeared at distance {d}"
                );
                // Its retinue joins the fight, and the retinue is its own dead.
                let group = a.group_around(ri);
                assert!(
                    group.len() >= 3,
                    "a rite pulled only {} into the fight",
                    group.len()
                );
                for &gi in &group {
                    if gi != ri {
                        assert_eq!(a.monsters[gi].faction, "undead", "a living retainer");
                    }
                }
                // Harder than a pack leader, softer than a Gatekeeper.
                assert!(boss.max_hp > 0 && !boss.affix.is_empty(), "a rite boss with no affix");
            }
        }
        assert!(found > 0, "no undead rite ever spawned across eight worlds");

        // And onboarding never meets one.
        let tut = Arena::generate(&b, 5, true);
        assert!(tut.monsters.iter().all(|m| m.encounter_class != "undead_rite"));
    }

    /// Creature health may climb as fast as the design wants, but it may not JUMP:
    /// riding the integer `tier(d)` made it a staircase with a 6.4x riser, so a forest
    /// stalker at d=99 died in 2 swings and the same creature one step later took 10.
    /// A single unit of walking must never change a fight's length by more than a hair.
    #[test]
    fn creature_health_climbs_without_a_cliff() {
        let b = Balance::load_default().unwrap();
        let s = Scaling::new(&b);
        // One unit of walking may add at most one unit of the curve's own slope — a
        // band's worth spread over the band, never a band's worth in a single step.
        let step_cap = b.world_scaling.hp_per_tier / b.world_scaling.tier_divisor;
        let mut prev = s.hp_mult(0);
        for d in 1..=4000i64 {
            let now = s.hp_mult(d);
            assert!(now >= prev - 1e-9, "creature health fell going deeper at d={d}");
            assert!(
                now - prev <= step_cap + 1e-9,
                "creature health jumps {:.2} at d={d} ({prev:.2} -> {now:.2})",
                now - prev
            );
            prev = now;
        }
        // The depths the curve was tuned at keep the multiplier they had: the fix takes
        // the cliff out from between the bands, it does not re-tune the game.
        let per = b.world_scaling.hp_per_tier;
        for band in 1..=6i64 {
            let centre = band * b.world_scaling.tier_divisor as i64 + 50;
            let want = 1.0 + per * band as f64;
            assert!(
                (s.hp_mult(centre) - want).abs() < 1e-6,
                "band {band}'s centre (d={centre}) moved: {} vs {want}",
                s.hp_mult(centre)
            );
        }
        // And the hub ring is still the hub ring — the tutorial band is untouched.
        assert_eq!(s.hp_mult(0), 1.0);
        assert_eq!(s.hp_mult(50), 1.0);
    }

    #[test]
    fn armour_grows_with_distance_but_never_outruns_the_attacks_aimed_at_it() {
        let b = Balance::load_default().unwrap();
        let s = Scaling::new(&b);
        // Armour used to be the ONE creature stat that ignored distance, which made
        // every armour-piercing ability (Backstab, the Psyker's armour-ignoring Foci)
        // a selling point with nothing to sell against out deep.
        assert!(s.def_mult(0) < s.def_mult(1000), "armour does not scale with depth");
        assert!(s.def_mult(1000) < s.def_mult(4000));

        // But it must stay GENTLER than attack and HP: defence subtracts from damage
        // instead of scaling it, so armour growing as fast as HP would floor every
        // physical hit at `min_damage` and turn the deep game into a wall.
        for d in [500i64, 1000, 2000, 4000] {
            assert!(
                s.def_mult(d) < s.stat_mult(d),
                "at d={d} armour ({}) grows at or above HP/attack ({})",
                s.def_mult(d),
                s.stat_mult(d)
            );
        }

        // Concretely: the heaviest-armoured creature in the game, met deep, must still
        // be hurt by a creature-tier attack rather than shrugging it off entirely.
        let heaviest = b
            .creature
            .values()
            .map(|c| c.base_def)
            .max()
            .expect("creatures exist");
        for d in [1000i64, 4000] {
            let def = ((heaviest as f64) * s.def_mult(d)).round() as i32;
            let atk = b
                .creature
                .values()
                .map(|c| ((c.base_atk as f64) * s.stat_mult(d)).round() as i32)
                .max()
                .unwrap();
            assert!(
                atk > def,
                "at d={d} the toughest armour ({def}) beats the hardest hit ({atk})"
            );
        }
    }
}



/// The smithing tempo game (MS-1). Working metal is a rhythm: a marker sweeps a bar of
/// **red** and the smith strikes on the **yellow** — the hot part of the heat. Hitting it
/// is what quality is, and quality is what decides the affix pool a re-draw rolls from,
/// how much a repair gives back, and how sharp a temporary edge comes out.
///
/// Pure and deterministic, like the rest of this crate: the schedule comes from a seed
/// the SERVER picks and the grade is a function of it plus the strikes reported. A client
/// renders the bar, but it never decides what the bar was or whether a blow landed.
pub mod tempo {
    use meld_balance::Balance;

    /// One hot band, as fractions of a sweep (`0.0` = the bar's left edge).
    #[derive(Debug, Clone, Copy, PartialEq)]
    pub struct Band {
        pub lo: f64,
        pub hi: f64,
    }

    impl Band {
        pub fn holds(&self, at: f64) -> bool {
            at >= self.lo && at <= self.hi
        }
    }

    /// A heat: how many blows, how fast the marker sweeps, and where the yellow is on
    /// each blow. One band per strike, so a smith cannot learn one spot and stop looking.
    #[derive(Debug, Clone, PartialEq)]
    pub struct Heat {
        pub strikes: i32,
        pub sweep_ms: i64,
        pub bands: Vec<Band>,
    }

    impl Heat {
        /// How long the whole heat may take before the server grades what it has.
        pub fn window_ms(&self, balance: &Balance) -> i64 {
            self.sweep_ms * self.strikes.max(1) as i64 + balance.tempo.grace_ms.max(0)
        }
    }

    /// Lay out a heat for work of difficulty `tier`, done by someone of `skill_level`
    /// with `extra_hands` others helping. Deeper work is harder; better craftspeople and
    /// bigger crews make it easier — the same number, from both ends. Used by the smith's
    /// anvil (difficulty = the piece's tier) and the Keeper's alembic (= the recipe's
    /// level), because it is the same idea wearing different words.
    pub fn schedule(
        balance: &Balance,
        tier: i32,
        skill_level: i32,
        extra_hands: i32,
        seed: u64,
    ) -> Heat {
        let t = &balance.tempo;
        let strikes = t.strikes(tier);
        let width = t.band_width(tier, skill_level, extra_hands);
        let mut rng = super::Rng(seed);
        let bands = (0..strikes)
            .map(|_| {
                // The band sits anywhere it fits whole: a band clipped by the bar's edge
                // would be a quietly easier (or impossible) blow.
                let lo = rng.unit() * (1.0 - width);
                Band { lo, hi: lo + width }
            })
            .collect();
        Heat {
            strikes,
            sweep_ms: t.sweep_ms(tier, skill_level, extra_hands),
            bands,
        }
    }

    /// Grade the blows a smith actually landed: the fraction that fell on yellow.
    /// Strikes past the last blow are ignored rather than counted against — a client
    /// that spams cannot lower someone's quality, and cannot raise it either.
    pub fn grade(heat: &Heat, strikes: &[f64]) -> f64 {
        if heat.strikes <= 0 {
            return 0.0;
        }
        let hits = heat
            .bands
            .iter()
            .zip(strikes.iter())
            .filter(|(band, at)| band.holds(**at))
            .count();
        hits as f64 / heat.strikes as f64
    }
}

#[cfg(test)]
mod tempo_tests {
    use super::tempo::*;
    use meld_balance::Balance;

    // The bar is the piece MINUS the smiths: a deep item is a sliver moving fast for an
    // apprentice working alone, and a workable band for a master with a crew. That
    // subtraction is the whole reason a second smith is worth a party slot.
    #[test]
    fn depth_makes_it_hard_and_smiths_make_it_easy_again() {
        let b = Balance::load_default().unwrap();
        let apprentice_shallow = schedule(&b, 0, 1, 0, 7);
        let apprentice_deep = schedule(&b, 6, 1, 0, 7);
        assert!(
            apprentice_deep.bands[0].hi - apprentice_deep.bands[0].lo
                < apprentice_shallow.bands[0].hi - apprentice_shallow.bands[0].lo,
            "a deeper piece should be a narrower band"
        );
        assert!(
            apprentice_deep.sweep_ms <= apprentice_shallow.sweep_ms,
            "and a faster sweep"
        );
        assert!(
            apprentice_deep.strikes >= apprentice_shallow.strikes,
            "and take at least as many blows"
        );

        let master_deep = schedule(&b, 6, 20, 0, 7);
        let width = |h: &Heat| h.bands[0].hi - h.bands[0].lo;
        assert!(
            width(&master_deep) > width(&apprentice_deep),
            "a smith's own level should widen the yellow"
        );
        let crew_deep = schedule(&b, 6, 20, 3, 7);
        assert!(width(&crew_deep) > width(&master_deep), "so should a crew");
        assert!(crew_deep.sweep_ms > master_deep.sweep_ms, "a crew buys time too");

        // Past a full party of smiths there is nothing left to hold.
        assert_eq!(schedule(&b, 6, 20, 9, 7), schedule(&b, 6, 20, 3, 7));
    }

    // Every band has to fit the bar WHOLE: one clipped by an edge would be a quietly
    // easier blow (or an impossible one), which is not a difficulty curve, it is a bug.
    #[test]
    fn every_band_fits_inside_the_bar() {
        let b = Balance::load_default().unwrap();
        for tier in 0..8 {
            for seed in 0..64u64 {
                let h = schedule(&b, tier, 1, 0, seed);
                assert_eq!(h.bands.len(), h.strikes as usize, "one band per blow");
                for band in &h.bands {
                    assert!(band.lo >= 0.0 && band.hi <= 1.0, "tier {tier} seed {seed}: {band:?}");
                    assert!(band.hi > band.lo);
                }
            }
        }
    }

    // Same seed, same heat — the server can hand a client the bar and still be the only
    // thing that knows whether a blow landed.
    #[test]
    fn a_heat_is_reproducible_from_its_seed() {
        let b = Balance::load_default().unwrap();
        assert_eq!(schedule(&b, 3, 4, 1, 99), schedule(&b, 3, 4, 1, 99));
        assert_ne!(schedule(&b, 3, 4, 1, 99), schedule(&b, 3, 4, 1, 100));
    }

    #[test]
    fn quality_is_the_blows_that_landed_on_yellow() {
        let b = Balance::load_default().unwrap();
        let h = schedule(&b, 2, 1, 0, 12);
        let n = h.strikes as usize;

        // Dead centre of every band: a flawless heat, and the epic pool.
        let perfect: Vec<f64> = h.bands.iter().map(|x| (x.lo + x.hi) / 2.0).collect();
        assert_eq!(grade(&h, &perfect), 1.0);
        assert_eq!(b.tempo.rarity_for(grade(&h, &perfect)), "epic");

        // Nowhere near it: no quality, the common pool, and a repair still gives back
        // its floor — a missed heat is a bad job, not a robbery.
        let missed: Vec<f64> = h.bands.iter().map(|x| if x.lo > 0.5 { 0.0 } else { 1.0 }).collect();
        assert_eq!(grade(&h, &missed), 0.0);
        assert_eq!(b.tempo.rarity_for(0.0), "common");
        assert!(b.tempo.repair_fraction(0.0) > 0.0);
        assert!(b.tempo.repair_fraction(1.0) > b.tempo.repair_fraction(0.0));

        // Half the blows landed is half the quality.
        let mut half = perfect.clone();
        for i in (0..n).step_by(2) {
            half[i] = if h.bands[i].lo > 0.5 { 0.0 } else { 1.0 };
        }
        let q = grade(&h, &half);
        assert!(q > 0.0 && q < 1.0, "{q}");

        // Spam cannot help: blows past the last one are ignored, not counted.
        let mut spam = perfect.clone();
        spam.extend(std::iter::repeat_n(perfect[0], 40));
        assert_eq!(grade(&h, &spam), 1.0);
        assert_eq!(grade(&h, &[]), 0.0, "a smith who never struck earned nothing");
    }
}

/// A player-built world object — the ONE primitive (CANON D21/§W3). Its `function` tag
/// (from [`meld_proto::structures`]) is the only thing that varies its role; the
/// lifecycle, the persistence and — when `BD-4` lands — the siege are shared by every
/// function, which is the discipline CANON mandates rather than a convenience.
#[derive(Debug, Clone)]
pub struct Structure {
    pub entity_id: Id,
    /// A `meld_proto::structures` key (`anchor`, `wall`).
    pub function: String,
    pub owner_player_id: Id,
    pub position: Position,
    pub elevation: u8,
    pub hp: i32,
    pub max_hp: i32,
    /// World tick it was placed, with [`Structure::build_ticks`] — together they are the
    /// build ramp. A structure goes up weak and strengthens, so planting one in front of
    /// an oncoming Shift is a gamble on whether it finishes.
    pub placed_tick: u64,
    pub build_ticks: u64,
    /// The material it was built from, so packing it up hands back the same stock rather
    /// than something the world had to guess at — the same rule MS-1's benches follow.
    pub ore: String,
    /// What it cost, so the refund is a share of the real price and not of a list price
    /// that a perk or a discount may have moved.
    pub ore_cost: i32,
}

impl Structure {
    /// Is it still going up? A structure under construction is weaker, not inert.
    pub fn building(&self, tick: u64) -> bool {
        tick.saturating_sub(self.placed_tick) < self.build_ticks
    }
    /// Whole-percent HP, for the wire and the bar over its head.
    pub fn hp_pct(&self) -> i32 {
        if self.max_hp <= 0 {
            return 0;
        }
        ((self.hp as f64 / self.max_hp as f64) * 100.0).round().clamp(0.0, 100.0) as i32
    }
    pub fn def(&self) -> Option<&'static meld_proto::structures::StructureDef> {
        meld_proto::structures::structure(&self.function)
    }
    pub fn pins(&self) -> bool {
        self.def().is_some_and(|d| d.pins)
    }
    pub fn blocks(&self) -> bool {
        self.def().is_some_and(|d| d.blocks)
    }
}

/// Why a placement was refused. Each is a sentence the player can act on — "no" with no
/// reason is the thing that makes building feel arbitrary.
#[derive(Debug, Clone, PartialEq)]
pub enum PlaceRefusal {
    UnknownFunction,
    NotInWorld,
    TooClose,
    OnTheTrail,
    Blocked,
    AtYourLimit,
    SomeoneStanding,
}

impl PlaceRefusal {
    pub fn message(&self) -> &'static str {
        match self {
            PlaceRefusal::UnknownFunction => "No such structure.",
            PlaceRefusal::NotInWorld => "You are not out in the world.",
            PlaceRefusal::TooClose => "Too close to something already standing.",
            PlaceRefusal::OnTheTrail => "Not on the trail — the way out has to stay open.",
            PlaceRefusal::Blocked => "There is no room here.",
            PlaceRefusal::AtYourLimit => "You are holding as much ground as you can.",
            PlaceRefusal::SomeoneStanding => "Someone is standing there.",
        }
    }
}

/// One anchor that took a Shift, and what holding it cost.
///
/// A NAMED record rather than a tuple because the caller has to send `hp`/`max_hp` on the
/// wire, and it used to re-derive them by searching `Arena::structures` *after*
/// [`Arena::hold_shift`] had already retained the dead ones out — so every destroyed anchor
/// reported `hp: 0, max_hp: 0`, a max that no structure in the game has. The values are
/// correct exactly once, at the moment the damage lands, so that is where they are recorded.
#[derive(Debug, Clone, PartialEq)]
pub struct HeldAnchor {
    pub entity_id: String,
    /// HP the hold cost it.
    pub damage: i32,
    /// What it has left, after taking the blow.
    pub hp: i32,
    pub max_hp: i32,
    /// It did not survive holding. The ground is shiftable again from here.
    pub destroyed: bool,
}

/// A Shift that was HELD (CANON §W3): what the land did to the anchors that stopped it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ShiftHeld {
    /// Anchors that took the blow.
    pub anchors: Vec<HeldAnchor>,
    pub inner_radius: f64,
    pub outer_radius: f64,
}

impl Arena {
    /// Place a structure where `player_id` stands (CANON §W3, `BD-2`).
    ///
    /// **Never on the clear-path tube.** Generation keeps the route out feasible by
    /// construction and the Shift's re-scatter honours the same tube; a player-built wall
    /// across the trail would be the one thing in the world that could seal the exit, and
    /// it would do it on purpose.
    pub fn place_structure(
        &mut self,
        balance: &Balance,
        player_id: &str,
        function: &str,
        ore: &str,
        tick: u64,
    ) -> Result<&Structure, PlaceRefusal> {
        let b = &balance.building;
        let Some((cost, max_hp, build_ms)) = b.spec(function) else {
            return Err(PlaceRefusal::UnknownFunction);
        };
        if meld_proto::structures::structure(function).is_none() {
            return Err(PlaceRefusal::UnknownFunction);
        }
        let Some(av) = self.avatar(player_id) else {
            return Err(PlaceRefusal::NotInWorld);
        };
        let (position, elevation) = (av.position, av.elevation);
        if self.structures.iter().filter(|s| s.owner_player_id == player_id).count()
            >= b.max_per_player
        {
            return Err(PlaceRefusal::AtYourLimit);
        }
        let corridor = self.corridorize(&position);
        if dist_to_path(&corridor, &self.corridor_path) < self.path_clear_radius
            || dist_to_web(&corridor, &self.corridor_web) < self.web_clear()
        {
            return Err(PlaceRefusal::OnTheTrail);
        }
        if self
            .structures
            .iter()
            .any(|s| s.position.distance_to(&position) < b.min_spacing)
            || self.stations.iter().any(|s| s.position.distance_to(&position) < b.min_spacing)
        {
            return Err(PlaceRefusal::TooClose);
        }
        if self
            .obstacles
            .iter()
            .any(|o| o.position.distance_to(&position) < o.radius + self.player_radius)
        {
            return Err(PlaceRefusal::Blocked);
        }
        // You may not drop something impassable on another player. The spacing invariant
        // already guarantees a ring of walls has a gap, but that is a promise about
        // GEOMETRY — it says nothing about walling somebody in one block at a time while
        // they stand still, and a player who cannot demolish what is around them has no
        // answer to it. Only blockers: an anchor is not a cage.
        let def = meld_proto::structures::structure(function);
        if def.is_some_and(|d| d.blocks) {
            let keep_clear = b.no_build_near_player;
            if self
                .avatars
                .iter()
                .any(|a| a.player_id != player_id && a.position.distance_to(&position) < keep_clear)
            {
                return Err(PlaceRefusal::SomeoneStanding);
            }
        }
        let tick_ms = balance.battle.tick_ms.max(1);
        let hp = ((max_hp as f64) * b.build_start_fraction).round().max(1.0) as i32;
        self.structures.push(Structure {
            entity_id: format!("struct-{}-{}", function, self.next_structure),
            function: function.to_string(),
            owner_player_id: player_id.to_string(),
            position,
            elevation,
            hp: hp.min(max_hp),
            max_hp,
            placed_tick: tick,
            build_ticks: (build_ms / tick_ms).max(1),
            ore: ore.to_string(),
            ore_cost: cost,
        });
        self.next_structure += 1;
        Ok(self.structures.last().expect("just pushed"))
    }

    /// Ramp every structure still going up toward its full HP. Linear in build time, so
    /// the bar over its head and the HP it can actually soak are the same fact.
    ///
    /// Only ever raises HP toward the ramp's floor: a structure that has been damaged
    /// mid-build must not be healed by its own construction, or a besieged wall would
    /// repair itself for free while the siege was still landing.
    pub fn advance_builds(&mut self, tick: u64) {
        for s in self.structures.iter_mut() {
            // `<=`, not `building()`: that predicate is false ON the completion tick (it
            // answers "still going up"), so gating the ramp on it left every structure
            // frozen at the second-to-last step and permanently short of its own max HP.
            if tick.saturating_sub(s.placed_tick) > s.build_ticks {
                continue;
            }
            let elapsed = tick.saturating_sub(s.placed_tick) as f64;
            let t = (elapsed / s.build_ticks.max(1) as f64).clamp(0.0, 1.0);
            let floor = ((s.max_hp as f64) * t).round() as i32;
            s.hp = s.hp.max(floor.min(s.max_hp));
        }
    }

    /// How much room a blocking structure takes up. Exposed so the anti-grief invariant
    /// can be asserted against the real number rather than a copy of it.
    pub fn structure_footprint(&self) -> f64 {
        self.structure_radius
    }

    #[cfg(test)]
    pub(crate) fn blocking_field_for_test(&self) -> BlockField {
        self.blocking_field()
    }

}

/// The blocking field as a spatial hash rather than a flat list.
///
/// **This is a per-tick cost, and the world streams outward without bound.** The check is
/// asked two or three times per creature per tick, so a linear scan is O(creatures x props):
/// measured at d1269 that is 10,650 creatures against 12,600 props — ~268 million distance
/// tests per 100 ms tick, which took **1.8 SECONDS** in a debug build. The game loop is a
/// single task, so it simply never caught up: a deep dive could not even send `run.started`,
/// which read as "the harness cannot boot a level-100 party" and was really this.
///
/// A prop can only block `p` if its own centre lies within `max_radius + radius` of it, so
/// bucketing by cell and sweeping the cells that span that distance returns exactly what the
/// scan returned. `blocks` is a predicate, so bucket order cannot change the answer and the
/// engine stays deterministic (CANON §S).
pub struct BlockField {
    cell: f64,
    max_radius: f64,
    buckets: HashMap<(i64, i64), Vec<(Position, f64)>>,
}

impl BlockField {
    fn new(items: Vec<(Position, f64)>) -> Self {
        let max_radius = items.iter().map(|(_, r)| *r).fold(0.0_f64, f64::max);
        // Wide enough that a query sweeps few cells, narrow enough that a cell holds few
        // props. Props are metres-wide, so a handful of metres is both.
        let cell = (max_radius * 2.0).max(8.0);
        let mut buckets: HashMap<(i64, i64), Vec<(Position, f64)>> = HashMap::new();
        for it in items {
            buckets.entry(Self::key(cell, &it.0)).or_default().push(it);
        }
        Self { cell, max_radius, buckets }
    }

    fn key(cell: f64, p: &Position) -> (i64, i64) {
        ((p.x / cell).floor() as i64, (p.y / cell).floor() as i64)
    }

    /// Does anything blocking overlap a disc of `radius` at `p`?
    pub fn blocks(&self, p: &Position, radius: f64) -> bool {
        let span = ((self.max_radius + radius) / self.cell).ceil() as i64;
        let (cx, cy) = Self::key(self.cell, p);
        for dx in -span..=span {
            for dy in -span..=span {
                if let Some(v) = self.buckets.get(&(cx + dx, cy + dy)) {
                    if v.iter().any(|(c, r)| p.distance_to(c) < r + radius) {
                        return true;
                    }
                }
            }
        }
        false
    }

    /// Everything in the field, for tests that want to reason about the whole set.
    pub fn len(&self) -> usize {
        self.buckets.values().map(|v| v.len()).sum()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl Arena {
    /// Everything nothing may walk through: terrain props, plus every `Structure` whose
    /// function `blocks` (CANON D21/§W3).
    ///
    /// ONE list, built in ONE place, because the alternative was tried and failed inside a
    /// single release: the wall-collision line was added to `step_creatures_with_aggro`
    /// and not to `apply_move`, so walls stopped creatures and players strolled through
    /// them — a `wall` that did not wall. A comment saying "some call site will forget"
    /// does not stop a call site forgetting; a shared function does.
    ///
    /// A structure still going up already blocks: a half-built wall you can walk through
    /// is a wall nobody would bother finishing.
    fn blocking_field(&self) -> BlockField {
        let mut out: Vec<(Position, f64)> =
            self.obstacles.iter().map(|o| (o.position, o.radius)).collect();
        out.extend(
            self.structures
                .iter()
                .filter(|s| s.blocks())
                .map(|s| (s.position, self.structure_radius)),
        );
        BlockField::new(out)
    }

    /// The structure `player_id` is standing at, within `radius` and on their level.
    pub fn structure_at(&self, player_id: &str, entity_id: &str, radius: f64) -> Option<&Structure> {
        let av = self.avatar(player_id)?;
        self.structures.iter().find(|s| {
            s.entity_id == entity_id
                && s.elevation == av.elevation
                && s.position.distance_to(&av.position) <= radius
        })
    }

    /// Spend one unit of ore on a structure. Returns the HP actually restored, so a
    /// nearly-full structure charges for what it took rather than for the whole unit.
    pub fn repair_structure(&mut self, balance: &Balance, entity_id: &str) -> Option<i32> {
        let per = balance.building.repair_hp_per_ore.max(1);
        let s = self.structures.iter_mut().find(|s| s.entity_id == entity_id)?;
        if s.hp >= s.max_hp {
            return None;
        }
        let before = s.hp;
        s.hp = (s.hp + per).min(s.max_hp);
        Some(s.hp - before)
    }

    /// Pack a structure down. Returns `(ore_kind, refunded)` — a share of what it cost,
    /// never all of it, or moving one is free.
    pub fn demolish_structure(
        &mut self,
        balance: &Balance,
        entity_id: &str,
    ) -> Option<(String, i32)> {
        let idx = self.structures.iter().position(|s| s.entity_id == entity_id)?;
        let s = self.structures.remove(idx);
        let back = (((s.ore_cost as f64) * balance.building.demolish_refund_fraction).floor()
            as i32)
            .max(0);
        Some((s.ore, back))
    }

    /// Is section `i` held against the Shift by a standing anchor (CANON §W3)?
    ///
    /// A pin is measured in the BENT world frame, because `pin_radius` is a distance a
    /// player paces out — asking it in corridor coordinates would make an anchor's reach
    /// grow with depth, since corridor y is an angle.
    pub fn section_pinned(&self, balance: &Balance, i: usize) -> bool {
        let Some(area) = self.areas.get(i) else { return false };
        let r = balance.building.anchor_pin_radius;
        self.structures.iter().filter(|s| s.pins() && s.hp > 0).any(|s| {
            let c = self.corridorize(&s.position).x;
            c + r >= area.start_x && c - r < area.end_x
        })
    }

    /// Every anchor holding any part of `[first, last]`, for the blow the land lands on
    /// them when they stop a Shift.
    fn anchors_holding(&self, balance: &Balance, first: usize, last: usize) -> Vec<usize> {
        let (inner, outer) = self.shift_band(first, last);
        let r = balance.building.anchor_pin_radius;
        self.structures
            .iter()
            .enumerate()
            .filter(|(_, s)| s.pins() && s.hp > 0)
            .filter(|(_, s)| {
                let c = self.corridorize(&s.position).x;
                c + r >= inner && c - r < outer
            })
            .map(|(i, _)| i)
            .collect()
    }

    /// The Shift arrives and the anchors stop it (CANON §W3, `BD-3`).
    ///
    /// **Holding costs.** The land pushes back on whatever is pinning it, so every anchor
    /// that held this one takes a share of its own max HP. That is what keeps an anchor
    /// from being permanence you buy once: it is permanence you keep paying for, and an
    /// anchor nobody hauls ore out to eventually falls on its own and hands the ground
    /// back to the Shift. Without this, `BD-3` would be "plant one, never think about this
    /// region again", which is the opposite of the loop it is named for.
    ///
    /// Returns `None` when nothing is holding, which is the caller's signal to land the
    /// Shift normally.
    pub fn hold_shift(
        &mut self,
        balance: &Balance,
        first: usize,
        last: usize,
    ) -> Option<ShiftHeld> {
        let holding = self.anchors_holding(balance, first, last);
        if holding.is_empty() {
            return None;
        }
        let (inner_radius, outer_radius) = self.shift_band(first, last);
        let share = balance.building.shift_hold_damage_fraction;
        let mut anchors = Vec::new();
        for idx in holding {
            let s = &mut self.structures[idx];
            let dmg = (((s.max_hp as f64) * share).round() as i32).max(1);
            let took = dmg.min(s.hp);
            s.hp -= took;
            anchors.push(HeldAnchor {
                entity_id: s.entity_id.clone(),
                damage: took,
                hp: s.hp,
                max_hp: s.max_hp,
                destroyed: s.hp <= 0,
            });
        }
        self.structures.retain(|s| s.hp > 0);
        Some(ShiftHeld { anchors, inner_radius, outer_radius })
    }
}
