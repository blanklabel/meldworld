//! Creature ability pools + elemental profiles (Creature AI spec §1).
//!
//! Content-side of the monster AI: every creature kind owns a permanent pool
//! of [`MonsterAbility`] options that *expands with level* (`min_level` gates —
//! a young creature knows its opener; the deep-world veteran of the same kind
//! layers on channelled devastation), plus a `damage_modifiers` elemental
//! profile (weakness / resistance / immunity / absorption) and a typed basic
//! attack. Execution lives in `meld-battle`; this module is pure data.

use meld_proto::abilities::{
    AbilityEffect, AbilityEffectKind, AbilityTarget, MonsterAbility, ScalingBase, StealTargetKind,
};
use meld_proto::enums::{DamageType, TargetProfile};

/// Shorthand constructors — the tables below stay readable.
fn dmg(base: ScalingBase, coeff: f64, ty: DamageType, target: AbilityTarget) -> AbilityEffect {
    AbilityEffect {
        effect_kind: AbilityEffectKind::Damage,
        scaling_base: Some(base),
        coefficient: Some(coeff),
        damage_type: Some(ty),
        target,
        status_name: None,
        duration_ticks: None,
        steal_target_kind: None,
    }
}
fn heal(base: ScalingBase, coeff: f64, target: AbilityTarget) -> AbilityEffect {
    AbilityEffect {
        effect_kind: AbilityEffectKind::Heal,
        scaling_base: Some(base),
        coefficient: Some(coeff),
        damage_type: None,
        target,
        status_name: None,
        duration_ticks: None,
        steal_target_kind: None,
    }
}
fn status(name: &str, ticks: i32, target: AbilityTarget) -> AbilityEffect {
    AbilityEffect {
        effect_kind: AbilityEffectKind::Status,
        scaling_base: None,
        coefficient: None,
        damage_type: None,
        target,
        status_name: Some(name.to_string()),
        duration_ticks: Some(ticks),
        steal_target_kind: None,
    }
}
/// `coeff` is the gauge fraction added (positive) or drained (negative).
fn atb(coeff: f64, target: AbilityTarget) -> AbilityEffect {
    AbilityEffect {
        effect_kind: AbilityEffectKind::AtbManipulation,
        scaling_base: None,
        coefficient: Some(coeff),
        damage_type: None,
        target,
        status_name: None,
        duration_ticks: None,
        steal_target_kind: None,
    }
}
fn steal(kind: StealTargetKind, target: AbilityTarget) -> AbilityEffect {
    AbilityEffect {
        effect_kind: AbilityEffectKind::Steal,
        scaling_base: None,
        coefficient: None,
        damage_type: None,
        target,
        status_name: None,
        duration_ticks: None,
        steal_target_kind: Some(kind),
    }
}
#[allow(clippy::too_many_arguments)]
fn ability(
    kind: &str,
    callout: &str,
    weight: i32,
    cooldown_ticks: i32,
    telegraph_ticks: i32,
    min_level: i32,
    hp_threshold_pct: Option<f64>,
    effects: Vec<AbilityEffect>,
) -> MonsterAbility {
    MonsterAbility {
        ability_kind: kind.to_string(),
        callout_text: callout.to_string(),
        weight,
        cooldown_ticks,
        telegraph_ticks,
        hp_threshold_pct,
        min_level,
        effects,
    }
}

use AbilityTarget::{AllEnemies, MonsterGroup, SelfCast, SingleEnemy};
// No glob import: `DamageType::None` would shadow `Option::None`.
use DamageType::{
    Blunt, Celestial, Earth, Ethereal, Fire, Ice, Infernal, Lightning, Mind, Pierce, Poison,
    Shadow, Slash, Water, Wind,
};
use ScalingBase::{Attack, Level, Magic, MaxHp};

/// The full (level-unfiltered) ability pool for a creature kind. The battle
/// engine gates entries by the spawn's level (`min_level`), cooldown, and
/// `hp_threshold_pct` at selection time — the pool itself is permanent per
/// kind across all runs.
pub fn creature_abilities(kind: &str) -> Vec<MonsterAbility> {
    match kind {
        // ---------------------------------------------------------- forest --
        "forest_bloom_stalker" => vec![
            // min_level 4: the tutorial-band stalker (L1–3) just claws — its
            // webs come out once the world starts scaling (keeps the very
            // first fight legible AND lethal to a passive party).
            ability("web_bind", "Web Bind!", 3, 60, 0, 4, None,
                vec![status("web", 60, SingleEnemy)]),
            ability("toxic_spores", "Toxic Spores!", 2, 100, 10, 8, None,
                vec![dmg(Magic, 0.9, Poison, AllEnemies), status("poison", 60, AllEnemies)]),
            ability("verdant_mend", "Verdant Mend!", 2, 150, 0, 15, Some(0.5),
                vec![heal(MaxHp, 0.25, SelfCast)]),
            ability("bloom_frenzy", "Bloom Frenzy!", 2, 50, 0, 20, None,
                vec![dmg(Attack, 1.4, Slash, SingleEnemy)]),
        ],
        "thornback_boar" => vec![
            ability("gore", "Gore!", 3, 30, 0, 1, None,
                vec![dmg(Attack, 1.2, Pierce, SingleEnemy)]),
            ability("trample", "Trample!", 2, 120, 20, 12, None,
                vec![dmg(Attack, 0.8, Blunt, AllEnemies)]),
            ability("squeal", "Rallying Squeal!", 1, 200, 10, 30, None,
                vec![atb(0.3, MonsterGroup)]),
        ],
        "sporeling" => vec![
            ability("spore_burst", "Spore Burst!", 3, 40, 0, 1, None,
                vec![dmg(Magic, 0.7, Poison, SingleEnemy)]),
            ability("replicate_haste", "Replicate!", 2, 150, 0, 10, None,
                vec![atb(0.25, MonsterGroup)]),
            ability("mycotoxin", "Mycotoxin!", 2, 180, 15, 25, None,
                vec![status("poison", 80, AllEnemies)]),
        ],
        // ---------------------------------------------------------- desert --
        "dune_wyrm" => vec![
            ability("sand_jet", "Sand Jet!", 3, 40, 0, 1, None,
                vec![dmg(Magic, 0.9, Earth, SingleEnemy)]),
            ability("burrow_strike", "Burrow Strike!", 2, 100, 15, 14, None,
                vec![dmg(Attack, 1.8, Pierce, SingleEnemy)]),
            ability("sandstorm", "Sandstorm!", 1, 200, 25, 28, None,
                vec![dmg(Magic, 0.7, Wind, AllEnemies), status("sand_veil", 60, AllEnemies)]),
        ],
        "sand_shade" => vec![
            ability("shadow_rake", "Shadow Rake!", 3, 40, 0, 1, None,
                vec![dmg(Magic, 1.0, Shadow, SingleEnemy)]),
            ability("mirage", "Mirage!", 2, 150, 0, 12, Some(0.7),
                vec![heal(MaxHp, 0.15, SelfCast)]),
            ability("chit_snatch", "Chit Snatch!", 1, 120, 0, 18, None,
                vec![steal(StealTargetKind::Chits, SingleEnemy)]),
            ability("umbral_howl", "Umbral Howl!", 2, 180, 20, 35, None,
                vec![dmg(Magic, 0.8, Shadow, AllEnemies)]),
        ],
        "dune_colossus" => vec![
            ability("boulder_fist", "Boulder Fist!", 3, 50, 0, 1, None,
                vec![dmg(Attack, 1.3, Blunt, SingleEnemy)]),
            ability("quake", "Quake!", 2, 200, 30, 20, None,
                vec![dmg(Attack, 0.9, Earth, AllEnemies)]),
            ability("stone_skin", "Stone Skin!", 1, 250, 10, 30, Some(0.5),
                vec![heal(MaxHp, 0.3, SelfCast)]),
        ],
        // --------------------------------------------------------- ashfall --
        "cinder_imp" => vec![
            ability("ember_flick", "Ember Flick!", 3, 30, 0, 1, None,
                vec![dmg(Magic, 0.9, Fire, SingleEnemy)]),
            ability("cackle", "Cackle!", 2, 90, 0, 10, None,
                vec![atb(-0.3, SingleEnemy)]),
            ability("pilfer", "Pilfer!", 1, 140, 0, 16, None,
                vec![steal(StealTargetKind::Consumable, SingleEnemy)]),
            ability("fire_dance", "Fire Dance!", 2, 150, 15, 22, None,
                vec![dmg(Magic, 0.6, Fire, AllEnemies), status("burn", 50, AllEnemies)]),
        ],
        "magma_golem" => vec![
            ability("molten_slam", "Molten Slam!", 3, 40, 0, 1, None,
                vec![dmg(Attack, 1.2, Blunt, SingleEnemy)]),
            ability("lava_surge", "Lava Surge!", 2, 180, 25, 15, None,
                vec![dmg(Magic, 1.0, Fire, AllEnemies), status("burn", 60, AllEnemies)]),
            ability("core_vent", "CORE VENT!", 2, 250, 30, 30, Some(0.4),
                vec![dmg(Magic, 1.5, Fire, AllEnemies)]),
        ],
        "ember_wisp" => vec![
            ability("flicker_burn", "Flicker Burn!", 3, 50, 0, 1, None,
                vec![dmg(Magic, 0.8, Fire, SingleEnemy), status("burn", 40, SingleEnemy)]),
            ability("wisp_haste", "Wisp Haste!", 2, 100, 0, 12, None,
                vec![atb(0.4, SelfCast)]),
            ability("flare_pop", "Flare Pop!", 2, 300, 10, 26, Some(0.35),
                vec![dmg(Magic, 1.8, Fire, SingleEnemy)]),
        ],
        // ---------------------------------------------------------- tundra --
        "frost_lurker" => vec![
            ability("ice_shard", "Ice Shard!", 3, 40, 0, 1, None,
                vec![dmg(Magic, 0.9, Ice, SingleEnemy)]),
            ability("chilling_grasp", "Chilling Grasp!", 2, 90, 0, 10, None,
                vec![status("chill", 60, SingleEnemy)]),
            ability("shatter_pounce", "Shatter Pounce!", 2, 120, 15, 24, None,
                vec![dmg(Attack, 1.7, Pierce, SingleEnemy)]),
        ],
        "ice_revenant" => vec![
            ability("grave_frost", "Grave Frost!", 3, 40, 0, 1, None,
                vec![dmg(Magic, 1.0, Ice, SingleEnemy)]),
            ability("soul_siphon", "Soul Siphon!", 2, 110, 0, 15, None,
                vec![dmg(Magic, 0.7, Shadow, SingleEnemy), heal(MaxHp, 0.1, SelfCast)]),
            ability("blizzard", "BLIZZARD!", 1, 220, 30, 32, None,
                vec![dmg(Magic, 0.8, Ice, AllEnemies), status("chill", 60, AllEnemies)]),
        ],
        "glacier_maw" => vec![
            ability("crushing_jaws", "Crushing Jaws!", 3, 50, 0, 1, None,
                vec![dmg(Attack, 1.3, Blunt, SingleEnemy)]),
            ability("glacial_bellow", "Glacial Bellow!", 2, 180, 20, 18, None,
                vec![atb(-0.4, AllEnemies)]),
            ability("avalanche", "AVALANCHE!", 2, 240, 30, 36, None,
                vec![dmg(Attack, 1.0, Ice, AllEnemies)]),
        ],
        // ------------------------------------------------------------ mire --
        "bog_serpent" => vec![
            ability("venom_fang", "Venom Fang!", 3, 50, 0, 1, None,
                vec![dmg(Attack, 1.0, Poison, SingleEnemy), status("poison", 60, SingleEnemy)]),
            ability("constrict", "Constrict!", 2, 100, 0, 12, None,
                vec![dmg(Attack, 0.6, Blunt, SingleEnemy), status("bind", 50, SingleEnemy)]),
            ability("miasma", "Miasma!", 1, 200, 20, 30, None,
                vec![status("poison", 80, AllEnemies)]),
        ],
        "myconid_brute" => vec![
            ability("fungal_smash", "Fungal Smash!", 3, 40, 0, 1, None,
                vec![dmg(Attack, 1.25, Blunt, SingleEnemy)]),
            ability("spore_cloud", "Spore Cloud!", 2, 160, 15, 16, None,
                vec![dmg(Magic, 0.6, Poison, AllEnemies), status("poison", 60, AllEnemies)]),
            ability("regrowth", "Regrowth!", 2, 200, 0, 22, Some(0.5),
                vec![heal(MaxHp, 0.25, SelfCast)]),
        ],
        "bog_stinger" => vec![
            ability("sting", "Sting!", 3, 30, 0, 1, None,
                vec![dmg(Attack, 1.0, Pierce, SingleEnemy), status("poison", 40, SingleEnemy)]),
            ability("swarm_frenzy", "Swarm Frenzy!", 2, 140, 0, 14, None,
                vec![atb(0.3, MonsterGroup)]),
            ability("neurotoxin", "Neurotoxin!", 2, 150, 10, 28, None,
                vec![dmg(Magic, 1.2, Poison, SingleEnemy), status("numb", 50, SingleEnemy)]),
        ],
        // -------------------------------------------------- bosses (FS-4) --
        // The 10 named bosses (`client::world_render::BOSS_KEYS`): "elite" tier
        // (gloamhound/rustfang) fights as an Elite champion; the other 8 (two
        // per miniboss/dungeon/region/biome tier) fight as Gatekeepers, picked
        // by `pick_elite_boss_kind`/`pick_gatekeeper_boss_kind`. Each gets a
        // bespoke kit — a real signature move, not just bigger stats.
        "gloamhound" => vec![
            ability("gloom_bite", "Gloom Bite!", 3, 40, 0, 1, None,
                vec![dmg(Attack, 1.1, Shadow, SingleEnemy)]),
            ability("dusk_howl", "Dusk Howl!", 2, 130, 0, 8, None,
                vec![status("chill", 50, SingleEnemy), atb(-0.2, SingleEnemy)]),
            ability("umbral_pounce", "Umbral Pounce!", 2, 180, 12, 16, None,
                vec![dmg(Magic, 1.6, Shadow, SingleEnemy)]),
            ability("gloom_bay", "GLOOM BAY!", 2, 240, 24, 12, None,
                vec![dmg(Magic, 1.0, Shadow, AllEnemies), status("chill", 60, AllEnemies)]),
            ability("umbral_pack", "UMBRAL PACK!", 2, 300, 30, 45, None,
                vec![dmg(Attack, 1.45, Shadow, AllEnemies), status("dread", 80, AllEnemies)]),
        ],
        "rustfang" => vec![
            ability("rust_gnash", "Rust Gnash!", 3, 40, 0, 1, None,
                vec![dmg(Attack, 1.1, Blunt, SingleEnemy)]),
            ability("spark_coil", "Spark Coil!", 2, 140, 0, 8, None,
                vec![dmg(Magic, 0.8, Lightning, SingleEnemy), atb(0.2, SelfCast)]),
            ability("overdrive_maul", "Overdrive Maul!", 2, 200, 14, 16, Some(0.5),
                vec![dmg(Attack, 1.7, Lightning, SingleEnemy)]),
            ability("scrapstorm", "SCRAPSTORM!", 2, 250, 24, 12, None,
                vec![dmg(Attack, 1.0, Pierce, AllEnemies), status("corrode", 60, AllEnemies)]),
            ability("corrosion_bloom", "CORROSION BLOOM!", 2, 300, 30, 45, None,
                vec![dmg(Magic, 1.35, Water, AllEnemies), status("corrode", 90, AllEnemies)]),
        ],
        "choirmother" => vec![
            ability("discordant_note", "Discordant Note!", 3, 40, 0, 1, None,
                vec![dmg(Magic, 0.9, Mind, SingleEnemy)]),
            ability("mournful_hymn", "Mournful Hymn!", 2, 160, 0, 6, None,
                vec![heal(MaxHp, 0.2, SelfCast), status("resolve", 80, SelfCast)]),
            ability("choir_of_the_lost", "CHOIR OF THE LOST!", 2, 260, 24, 12, None,
                vec![dmg(Magic, 1.1, Mind, AllEnemies), status("dread", 60, AllEnemies)]),
            ability("requiem_unending", "REQUIEM UNENDING!", 2, 300, 30, 45, None,
                vec![dmg(Magic, 1.4, Mind, AllEnemies), status("dread", 90, AllEnemies), atb(-0.2, AllEnemies)]),
        ],
        "pyrewarden" => vec![
            ability("cinder_lash", "Cinder Lash!", 3, 40, 0, 1, None,
                vec![dmg(Attack, 1.1, Fire, SingleEnemy)]),
            ability("kindled_shell", "Kindled Shell!", 2, 180, 0, 6, Some(0.6),
                vec![heal(MaxHp, 0.15, SelfCast), status("burn_ward", 80, SelfCast)]),
            ability("pyre_eruption", "PYRE ERUPTION!", 2, 260, 24, 12, None,
                vec![dmg(Magic, 1.2, Fire, AllEnemies), status("burn", 60, AllEnemies)]),
            ability("second_kindling", "SECOND KINDLING!", 2, 300, 30, 45, Some(0.5),
                vec![heal(MaxHp, 0.25, SelfCast), dmg(Magic, 1.3, Fire, AllEnemies)]),
        ],
        "sepulcher" => vec![
            ability("tomb_claw", "Tomb Claw!", 3, 40, 0, 1, None,
                vec![dmg(Attack, 1.1, Shadow, SingleEnemy)]),
            ability("grave_siphon", "Grave Siphon!", 2, 150, 0, 6, None,
                vec![dmg(Magic, 0.9, Shadow, SingleEnemy), heal(MaxHp, 0.12, SelfCast)]),
            ability("epitaph_of_ruin", "EPITAPH OF RUIN!", 1, 300, 28, 14, Some(0.45),
                vec![dmg(Magic, 1.5, Ethereal, SingleEnemy), status("dread", 70, SingleEnemy)]),
            // Its mid-tier WIDE row. Every other boss has one around this rung; these three
            // (sepulcher, rustfang, gloamhound) had their only party-wide ability gated at
            // level 45, and a gatekeeper stands at `gatekeeper_min_distance` = level 24 — so
            // for three bosses in ten a Worldbreaker label sat on a creature that could only
            // ever hit one hero at a time, which at sixteen heroes is a sixteenth of a fight.
            ability("grave_pall", "GRAVE PALL!", 2, 250, 24, 12, None,
                vec![dmg(Magic, 1.0, Ethereal, AllEnemies), status("dread", 60, AllEnemies)]),
            ability("mausoleum_collapse", "MAUSOLEUM COLLAPSE!", 2, 330, 34, 45, None,
                vec![dmg(Attack, 1.6, Earth, AllEnemies), status("dread", 90, AllEnemies)]),
        ],
        "hollowbishop" => vec![
            ability("hollow_gaze", "Hollow Gaze!", 3, 40, 0, 1, None,
                vec![dmg(Magic, 0.9, Mind, SingleEnemy)]),
            ability("last_rites", "Last Rites!", 2, 170, 0, 6, None,
                vec![status("curse", 90, SingleEnemy), atb(-0.25, SingleEnemy)]),
            ability("sermon_of_silence", "SERMON OF SILENCE!", 1, 280, 26, 14, None,
                vec![dmg(Magic, 1.0, Mind, AllEnemies), status("numb", 60, AllEnemies)]),
            ability("excommunication", "EXCOMMUNICATION!", 2, 320, 32, 45, None,
                vec![dmg(Magic, 1.5, Ethereal, AllEnemies), status("curse", 120, AllEnemies)]),
        ],
        "ironmaw" => vec![
            ability("crush_bite", "Crush Bite!", 3, 40, 0, 1, None,
                vec![dmg(Attack, 1.15, Blunt, SingleEnemy)]),
            ability("chain_lash", "Chain Lash!", 2, 160, 10, 8, None,
                vec![dmg(Attack, 1.3, Lightning, SingleEnemy), status("bind", 50, SingleEnemy)]),
            ability("magnetic_surge", "Magnetic Surge!", 2, 220, 0, 14, None,
                vec![atb(-0.35, AllEnemies)]),
            ability("ironmaw_rampage", "IRONMAW RAMPAGE!", 1, 320, 30, 20, Some(0.4),
                vec![dmg(Attack, 1.6, Lightning, AllEnemies)]),
            ability("scrap_avalanche", "SCRAP AVALANCHE!", 2, 320, 32, 45, None,
                vec![dmg(Attack, 1.6, Blunt, AllEnemies), status("stagger", 80, AllEnemies)]),
        ],
        "weepingcolossus" => vec![
            ability("tremor_step", "Tremor Step!", 3, 50, 0, 1, None,
                vec![dmg(Attack, 1.1, Earth, SingleEnemy)]),
            ability("sorrow_wail", "Sorrow Wail!", 2, 170, 14, 8, None,
                vec![dmg(Magic, 0.8, Ethereal, AllEnemies), status("dread", 50, AllEnemies)]),
            ability("crushing_grief", "Crushing Grief!", 2, 210, 0, 14, None,
                vec![atb(-0.3, SingleEnemy), dmg(Attack, 0.7, Earth, SingleEnemy)]),
            ability("collapsing_sorrow", "COLLAPSING SORROW!", 1, 340, 32, 22, Some(0.4),
                vec![dmg(Attack, 1.7, Earth, AllEnemies)]),
            ability("flood_of_years", "FLOOD OF YEARS!", 2, 340, 34, 45, None,
                vec![dmg(Magic, 1.5, Water, AllEnemies), atb(-0.3, AllEnemies)]),
        ],
        "miredrowned" => vec![
            ability("silt_claw", "Silt Claw!", 3, 40, 0, 1, None,
                vec![dmg(Attack, 1.0, Poison, SingleEnemy), status("poison", 60, SingleEnemy)]),
            ability("drowning_grip", "Drowning Grip!", 2, 170, 10, 8, None,
                vec![dmg(Attack, 0.7, Water, SingleEnemy), status("bind", 60, SingleEnemy)]),
            ability("bog_miasma", "Bog Miasma!", 2, 220, 0, 14, None,
                vec![status("poison", 100, AllEnemies)]),
            ability("depths_reclaim", "THE DEPTHS RECLAIM!", 1, 340, 30, 24, Some(0.4),
                vec![dmg(Magic, 1.4, Poison, AllEnemies), status("poison", 80, AllEnemies)]),
            ability("drowning_procession", "DROWNING PROCESSION!", 2, 300, 30, 45, None,
                vec![dmg(Magic, 1.4, Water, AllEnemies), status("numb", 90, AllEnemies)]),
        ],
        "ashenleviathan" => vec![
            ability("ash_maw", "Ash Maw!", 3, 40, 0, 1, None,
                vec![dmg(Attack, 1.15, Fire, SingleEnemy)]),
            ability("cinder_wave", "Cinder Wave!", 2, 180, 14, 8, None,
                vec![dmg(Magic, 0.9, Fire, AllEnemies), status("burn", 60, AllEnemies)]),
            ability("infernal_maw", "Infernal Maw!", 2, 240, 0, 16, Some(0.5),
                vec![dmg(Magic, 1.3, Infernal, SingleEnemy)]),
            ability("ashfall_apocalypse", "ASHFALL APOCALYPSE!", 1, 360, 34, 26, Some(0.35),
                vec![dmg(Level, 0.35, Infernal, AllEnemies), status("burn", 100, AllEnemies)]),
        ],
        // Unknown kinds fight with basic attacks only (still a full combatant).
        _ => vec![],
    }
}

/// What a named boss actually IS (FS-4). A boss overlays a host creature, and used
/// to inherit that host's faction — so a Choirmother riding a forest beast fought as
/// a beast. Lineage is the boss's own: it drives overworld skirmishing, battle
/// targeting, and (with `PG-2`) which unlock an encounter can grant.
pub fn boss_faction(boss_kind: &str) -> Option<&'static str> {
    Some(match boss_kind {
        // The dead: a choir, a bishop, a drowned congregation, a tomb.
        "choirmother" | "hollowbishop" | "miredrowned" | "sepulcher" => "undead",
        // The made: jaws, teeth, hounds, colossi and wardens of metal and fire.
        "ironmaw" | "rustfang" | "gloamhound" | "weepingcolossus" | "pyrewarden" => "construct",
        // The leviathan is neither built nor buried.
        "ashenleviathan" => "wyrm",
        _ => return None,
    })
}

/// Every boss whose lineage is `faction`.
/// Every named boss (FS-4), whatever its lineage — what the END FIGHT draws its three
/// peers from, since that encounter is the world itself resisting rather than one faction.
pub const ALL_BOSSES: &[&str] = &[
    "choirmother",
    "hollowbishop",
    "miredrowned",
    "sepulcher",
    "ironmaw",
    "rustfang",
    "gloamhound",
    "weepingcolossus",
    "pyrewarden",
    "ashenleviathan",
];

pub fn all_bosses() -> &'static [&'static str] {
    ALL_BOSSES
}

pub fn bosses_of_faction(faction: &str) -> Vec<&'static str> {
    [
        "choirmother",
        "hollowbishop",
        "miredrowned",
        "sepulcher",
        "ironmaw",
        "rustfang",
        "gloamhound",
        "weepingcolossus",
        "pyrewarden",
        "ashenleviathan",
    ]
    .into_iter()
    .filter(|k| boss_faction(k) == Some(faction))
    .collect()
}

/// Size a raid boss's KIT to the crowd it is sized for (`FS-4`).
///
/// A raid boss's HP and XP ride its declared party count and its ATTACK deliberately does
/// not, on the grounds that scaling a blow which lands on ONE hero would one-shot whoever
/// arrived before the merge filled. That argument is entirely about single targets — and
/// followed through, it demands the opposite conclusion for a WIDE ability, which was the
/// half nobody drew.
///
/// **A single-target blow is divided by the crowd; a wide one is not.** So the shipped raid
/// boss got *less* threatening per hero the more people brought, not more. Measured over five
/// world seeds, **12.5%** of an unlabelled gatekeeper's turns go wide, which puts a
/// Worldbreaker at sixteen heroes on **52%** of the per-hero pressure that same boss applies
/// to a lone party — while carrying 20x the health. Four times the damage went in and each
/// hero felt half the answer back: "sized for four parties" meant a longer fight, and an
/// *easier* one.
///
/// The fix is CADENCE, never magnitude: the abilities that reach the whole party come round
/// sooner and are picked more often, and every number a hero actually takes is the one an
/// ordinary gatekeeper would have dealt. That is what keeps this safe where scaling attack
/// is not — nothing here can turn a hit into a one-shot, so the party that touches it first
/// is threatened over time rather than deleted on arrival. It also reuses each boss's OWN
/// authored signature (a Cinder Wave, a SCRAP AVALANCHE) rather than bolting a generic raid
/// nuke onto ten different kits.
///
/// Single-target rows are left exactly alone. Raising them would be the attack-scaling
/// mistake wearing a cooldown's clothes.
pub fn widen_for_warband(
    pool: Vec<MonsterAbility>,
    parties: u8,
    weight_per_party: f64,
    cooldown_per_party: f64,
) -> Vec<MonsterAbility> {
    if !meld_proto::warbands::is_raid(parties) {
        return pool;
    }
    // Extra parties past the first, which is what the escalation is priced in.
    let extra = f64::from(parties.saturating_sub(1));
    let weight_mult = 1.0 + weight_per_party.max(0.0) * extra;
    let cooldown_div = 1.0 + cooldown_per_party.max(0.0) * extra;
    pool.into_iter()
        .map(|mut a| {
            if !a.reaches_the_whole_party() {
                return a;
            }
            a.weight = ((f64::from(a.weight.max(1)) * weight_mult).round() as i32).max(1);
            // A telegraph is the fight's readability and is NOT shortened — a raid blow
            // still announces itself for as long, it simply comes back sooner. Floored at
            // the telegraph so an ability can never be ready again before the last cast has
            // even landed.
            let floor = a.telegraph_ticks.max(1);
            a.cooldown_ticks =
                ((f64::from(a.cooldown_ticks) / cooldown_div).round() as i32).max(floor);
            a
        })
        .collect()
}

/// A boss's PALETTE band, from the monster level it is met at. A boss encountered
/// deep is the same boss wearing a worse mood: the client tints it by band, and the
/// deep-gated abilities in its pool come online around the same thresholds, so the
/// look and the fight escalate together.
pub fn boss_palette_band(monster_level: i32) -> u8 {
    match monster_level {
        l if l >= 120 => 3,
        l if l >= 80 => 2,
        l if l >= 40 => 1,
        _ => 0,
    }
}

/// What a creature is MADE OF, which is what decides how it answers a blade, a point and a
/// hammer. The creature-side mirror of a hero's `ArmorWeight`: same question, same answer
/// shape, so "plate turns an edge and fears a hammer" is one rule the whole game obeys
/// rather than a thing heroes happen to do.
///
/// This exists because the per-kind table had been authored as "a tough thing resists the
/// physical types", which is backwards for most materials — every colossus, golem and ice
/// maw in the game RESISTED blunt, when a hammer is exactly what shatters stone and ice.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Body {
    /// Stone, ice, crystal, fired clay. Blades skid off it; it CRACKS.
    Rigid,
    /// Iron and steel constructs, heavy carapace. Turns an edge, dents and rings under
    /// impact — a hero's plate, worn on the inside.
    Plated,
    /// Thick hide, scale, fur over muscle. Spines and depth defeat a point; impact carries
    /// straight through it.
    Hide,
    /// Flesh, fungal mass, fat. Opens to an edge and absorbs a blow.
    Soft,
    /// Sand, smoke, shadow, flame-held-in-a-shape. There is very little there to hit at all,
    /// which is the whole reason the Phoenix Guard exists.
    Amorphous,
}

/// How a body answers each physical type — ABSOLUTE multipliers, not steps, because a
/// creature is one body wearing its own hide and there is nothing to stack (a hero's armour
/// is per-piece and folds; see `[armor_resist]`).
///
/// Every body resists something and fears something, exactly as every armour weight does
/// (`a_body_is_a_trade`).
pub fn body_profile(body: Body) -> &'static [(DamageType, f64)] {
    match body {
        // A hammer is what stone is for.
        Body::Rigid => &[(Blunt, 1.4), (Slash, 0.6), (Pierce, 0.7)],
        // An edge skids, a spike finds a seam, a hammer rings it like a bell.
        Body::Plated => &[(Blunt, 1.3), (Slash, 0.5), (Pierce, 0.85)],
        // Depth and spines beat a point; impact does not care how thick you are.
        Body::Hide => &[(Blunt, 1.15), (Pierce, 0.7), (Slash, 0.9)],
        // Cuts open, soaks a blow.
        Body::Soft => &[(Slash, 1.25), (Pierce, 1.1), (Blunt, 0.8)],
        // Nothing solid to swing at. Its answer to being hit is that it mostly is not.
        Body::Amorphous => &[(Slash, 0.5), (Blunt, 0.5), (Pierce, 0.75)],
    }
}

/// What each creature kind is made of. Unlisted kinds are [`Body::Hide`] — the ordinary
/// case of a living thing with skin, and the safest default for a new creature.
pub fn creature_body(kind: &str) -> Body {
    match kind {
        "dune_colossus" | "glacier_maw" | "weepingcolossus" => Body::Rigid,
        "magma_golem" | "ironmaw" | "rustfang" => Body::Plated,
        "sand_shade" | "ember_wisp" | "gloamhound" => Body::Amorphous,
        "sporeling" | "myconid_brute" | "forest_bloom_stalker" | "bog_stinger"
        | "choirmother" | "hollowbishop" | "miredrowned" | "sepulcher" => Body::Soft,
        _ => Body::Hide,
    }
}

/// A creature kind's elemental profile — `damage_modifiers` in the spec.
/// `> 1.0` weakness, `< 1.0` resistance, `0.0` immunity, `< 0.0` absorption
/// (heals for the absolute value). Types not listed default to `1.0`.
fn creature_elemental_modifiers(kind: &str) -> Vec<(DamageType, f64)> {
    match kind {
        "forest_bloom_stalker" => vec![(Fire, 2.0), (Water, 0.5), (Earth, 0.5)],
        "thornback_boar" => vec![(Mind, 1.5)],
        "sporeling" => vec![(Fire, 2.0), (Poison, 0.0)],
        "dune_wyrm" => vec![(Water, 2.0), (Earth, 0.5), (Fire, 0.75)],
        "sand_shade" => vec![(Celestial, 2.0), (Shadow, -0.5)],
        "dune_colossus" => vec![(Water, 1.5), (Lightning, 1.5)],
        "cinder_imp" => vec![(Water, 2.0), (Ice, 1.5), (Fire, -1.0)],
        "magma_golem" => vec![(Water, 2.0), (Ice, 2.0), (Fire, 0.0), (Poison, 0.0)],
        "ember_wisp" => vec![(Water, 2.0), (Wind, 1.5), (Fire, -1.0), (Earth, 0.75)],
        "frost_lurker" => vec![(Fire, 2.0), (Ice, 0.5)],
        "ice_revenant" => vec![(Fire, 1.5), (Celestial, 2.0), (Ice, -0.5), (Poison, 0.0), (Shadow, 0.5)],
        "glacier_maw" => vec![(Fire, 2.0), (Ice, 0.0)],
        "bog_serpent" => vec![(Ice, 1.5), (Poison, -0.5), (Earth, 0.75)],
        "myconid_brute" => vec![(Fire, 2.0), (Poison, 0.0)],
        "bog_stinger" => vec![(Fire, 1.5), (Wind, 1.5), (Poison, 0.0)],
        // -------------------------------------------------- bosses (FS-4) --
        "gloamhound" => vec![(Celestial, 1.5), (Shadow, -0.25), (Fire, 0.75)],
        "rustfang" => vec![(Water, 1.5), (Lightning, -0.25), (Earth, 0.75)],
        "choirmother" => vec![(Shadow, 1.5), (Mind, 0.0), (Celestial, 0.5)],
        "pyrewarden" => vec![(Water, 1.5), (Ice, 1.25), (Fire, 0.0)],
        "sepulcher" => vec![(Celestial, 2.0), (Shadow, -0.25), (Mind, 0.5)],
        "hollowbishop" => vec![(Celestial, 1.5), (Mind, -0.25), (Shadow, 0.5)],
        "ironmaw" => vec![(Water, 1.5), (Earth, 0.5), (Lightning, 0.0)],
        "weepingcolossus" => vec![(Celestial, 1.25), (Ethereal, 0.5)],
        "miredrowned" => vec![(Fire, 1.5), (Ice, 1.25), (Poison, 0.0), (Water, -0.25)],
        "ashenleviathan" => vec![(Water, 2.0), (Ice, 1.5), (Fire, 0.0), (Infernal, 0.0)],
        _ => vec![],
    }
}
/// A creature kind's full damage profile: what its BODY does about blades, points and
/// hammers, plus what its nature does about the elements.
///
/// The physical half comes from [`creature_body`] and is no longer authored per kind — it
/// had been, and it was wrong in the same direction almost everywhere (every colossus and
/// golem resisted Blunt). A per-kind ELEMENTAL entry still wins over the body, so an
/// authored exception is possible; a per-kind physical entry is not, because that is the
/// mistake this split exists to prevent.
pub fn creature_damage_modifiers(kind: &str) -> Vec<(DamageType, f64)> {
    let mut out: Vec<(DamageType, f64)> = body_profile(creature_body(kind)).to_vec();
    for (ty, m) in creature_elemental_modifiers(kind) {
        match out.iter_mut().find(|(t, _)| *t == ty) {
            Some(slot) => slot.1 = m,
            None => out.push((ty, m)),
        }
    }
    out
}


/// The [`DamageType`] a creature kind's *basic* attack carries (the fallback
/// swing the AI mixes into every pool). Physical for most; a few exotics burn.
/// The targeting profile a creature of `kind` fights with at `level`, in an encounter of
/// `encounter_class` (CR-9).
///
/// Three inputs, in order of authority:
///
/// 1. **The kind's own nature.** An ambusher goes for the back rank; a pack animal
///    converges; a mindless thing swings at whatever is nearest. This is the creature's
///    character and it never changes.
/// 2. **The encounter class.** An Elite, a Gatekeeper or a boss is *smarter than the trash
///    around it* — it is promoted to a tactical profile even when its kind is not.
/// 3. **Level.** Deeper creatures are smarter ON AVERAGE: past `[ai] smart_level_floor` a
///    share of ordinary spawns rolls into a tactical profile, and the share climbs with
///    level to a ceiling. Rolled off the creature's own id + level so it is reproducible
///    rather than wall-clock — the same creature in the same fight always thinks the same
///    way.
pub fn creature_target_profile(
    kind: &str,
    encounter_class: &str,
    level: i32,
    seed: u64,
    b: &meld_balance::Ai,
) -> TargetProfile {
    // 1. The kind's own nature.
    let innate = match kind {
        // Ambushers and skirmishers slip past the front line by trade.
        "sand_shade" | "gloamhound" | "forest_bloom_stalker" => TargetProfile::Backline,
        // Pack animals converge on one mark.
        "thornback_boar" | "rustfang" | "ironmaw" => TargetProfile::GangUp,
        // Things that read minds go for the mind that matters.
        "choirmother" | "hollowbishop" | "sepulcher" => TargetProfile::Role,
        // Big mindless bodies swing at whatever is in front of them.
        "dune_colossus" | "magma_golem" | "weepingcolossus" | "myconid_brute" => {
            TargetProfile::Random
        }
        _ => TargetProfile::Weakest,
    };
    if innate.is_tactical() {
        return innate;
    }
    // 2. A champion is smarter than its escort, whatever it is.
    match encounter_class {
        // THE END FIGHT. Three peers, each hunting the role that makes the party work —
        // and `cap_role_hunters` then leaves exactly ONE of them doing it, which is the
        // whole reason that cap exists: three bosses independently deciding to kill the
        // healer ends the fight in about one round.
        "world_end" => return TargetProfile::Role,
        "gatekeeper" => return TargetProfile::Role,
        "elite" | "undead_rite" => return TargetProfile::GangUp,
        _ => {}
    }
    // 3. …and depth makes ordinary creatures smarter on average.
    if level < b.smart_level_floor {
        return innate;
    }
    let over = (level - b.smart_level_floor) as f64;
    let chance = (b.smart_chance_base + b.smart_chance_per_level * over).min(b.smart_chance_cap);
    // splitmix64 on the creature's own identity: reproducible, and independent of the
    // battle RNG so promoting a creature cannot shift any other roll in the fight.
    let mut h = seed ^ (level as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15);
    h = (h ^ (h >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    h = (h ^ (h >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    h ^= h >> 31;
    if ((h >> 11) as f64 / (1u64 << 53) as f64) < chance {
        // Which kind of smart it becomes is also its own: half hunt the line, half converge.
        if h & 1 == 0 { TargetProfile::Backline } else { TargetProfile::GangUp }
    } else {
        innate
    }
}

pub fn creature_basic_attack_type(kind: &str) -> DamageType {
    match kind {
        "thornback_boar" | "dune_colossus" | "magma_golem" | "glacier_maw" | "myconid_brute" => {
            Blunt
        }
        "forest_bloom_stalker" | "sand_shade" | "cinder_imp" | "ice_revenant" => Slash,
        "ember_wisp" => Fire,
        // Bosses (FS-4): a fang/claw/maw basic, flavored per identity.
        "gloamhound" | "sepulcher" | "hollowbishop" => Shadow,
        "rustfang" | "ironmaw" | "weepingcolossus" => Blunt,
        "pyrewarden" | "ashenleviathan" => Fire,
        "choirmother" => Mind,
        "miredrowned" => Poison,
        // Wyrms, lurkers, stingers, serpents, sporelings: piercing fangs/stings.
        _ => Pierce,
    }
}

#[cfg(test)]
mod tests {
    /// A body that is only ever better is a body nothing can fight. Same rule the hero's
    /// armour weights obey (`every_armor_weight_is_a_trade`) — this is the creature half of
    /// the symmetry.
    #[test]
    fn a_body_is_a_trade() {
        for b in [Body::Rigid, Body::Plated, Body::Hide, Body::Soft, Body::Amorphous] {
            let p = body_profile(b);
            assert!(p.iter().any(|(_, m)| *m < 1.0), "{b:?} resists nothing");
            assert!(
                p.iter().any(|(_, m)| *m > 1.0) || b == Body::Amorphous,
                "{b:?} fears nothing physical"
            );
            for (ty, _) in p {
                assert!(ty.is_physical(), "{b:?} claims {ty:?}, which is not a physical type");
            }
        }
    }

    /// **A hammer is what stone is for.** This is the bug the audit found: the per-kind table
    /// had every colossus, golem and ice maw RESISTING Blunt, because it was authored as
    /// "tough things resist physical" instead of by material. Nothing rigid or plated may
    /// resist a hammer again.
    #[test]
    fn nothing_made_of_stone_or_steel_shrugs_off_a_hammer() {
        for kind in ["dune_colossus", "glacier_maw", "weepingcolossus", "magma_golem", "ironmaw"] {
            let m: Vec<(DamageType, f64)> = creature_damage_modifiers(kind);
            let blunt = m.iter().find(|(t, _)| *t == Blunt).map(|(_, v)| *v).expect(kind);
            let slash = m.iter().find(|(t, _)| *t == Slash).map(|(_, v)| *v).expect(kind);
            assert!(blunt > 1.0, "{kind} resists Blunt at {blunt} — a hammer breaks stone");
            assert!(slash < 1.0, "{kind} should turn an edge, not take it at {slash}");
        }
    }

    /// And the other direction: soft things open to a blade and soak a blow.
    #[test]
    fn soft_things_fear_an_edge() {
        for kind in ["sporeling", "myconid_brute", "forest_bloom_stalker"] {
            let m = creature_damage_modifiers(kind);
            let slash = m.iter().find(|(t, _)| *t == Slash).map(|(_, v)| *v).expect(kind);
            let blunt = m.iter().find(|(t, _)| *t == Blunt).map(|(_, v)| *v).expect(kind);
            assert!(slash > 1.0, "{kind} shrugs off a blade at {slash}");
            assert!(blunt < 1.0, "{kind} should soak impact, not take {blunt}");
        }
    }

    /// Every creature answers for all three physical types, so no weapon choice is ever
    /// simply irrelevant against something.
    #[test]
    fn every_creature_has_a_physical_stance() {
        for kind in crate::all_creature_kinds().into_iter().chain(ALL_BOSSES.iter().copied()) {
            let m = creature_damage_modifiers(kind);
            for ty in [Blunt, Slash, Pierce] {
                assert!(
                    m.iter().any(|(t, _)| *t == ty),
                    "{kind} has nothing to say about {ty:?}"
                );
            }
        }
    }

    /// An ELEMENTAL entry may override the body; a PHYSICAL one may not exist, because that
    /// is exactly how the backwards values got in.
    #[test]
    fn the_per_kind_table_no_longer_authors_physical_types() {
        for kind in crate::all_creature_kinds().into_iter().chain(ALL_BOSSES.iter().copied()) {
            for (ty, _) in creature_elemental_modifiers(kind) {
                assert!(
                    !ty.is_physical(),
                    "{kind} authors {ty:?} per-kind — physical answers come from its Body"
                );
            }
        }
    }

    use super::*;

    /// CR-9: a champion is smarter than its escort, and depth makes ordinary creatures
    /// smarter ON AVERAGE — the two rules that stop every fight in the game reading the
    /// same way.
    #[test]
    fn creatures_get_smarter_with_rank_and_with_depth() {
        let b = meld_balance::Balance::load_default().unwrap();
        let ai = &b.ai;
        // A champion is promoted whatever its kind, at any level.
        assert!(creature_target_profile("dune_wyrm", "elite", 1, 7, ai).is_tactical());
        assert!(creature_target_profile("dune_wyrm", "gatekeeper", 1, 7, ai).is_tactical());

        // A kind with its own nature keeps it regardless of rank — that IS its character.
        assert_eq!(
            creature_target_profile("sand_shade", "standard", 1, 7, ai),
            TargetProfile::Backline
        );

        // …and ordinary spawns get smarter with depth. Measured over many creatures rather
        // than asserted on one, because the roll is per creature.
        let share = |level: i32| {
            let n = 400;
            let smart = (0..n)
                .filter(|i| {
                    creature_target_profile("dune_wyrm", "standard", level, *i as u64, ai)
                        .is_tactical()
                })
                .count();
            smart as f64 / n as f64
        };
        assert_eq!(share(ai.smart_level_floor - 1), 0.0, "upgraded below the floor");
        let shallow = share(ai.smart_level_floor);
        let deep = share(200);
        assert!(deep > shallow, "depth bought no cunning: {shallow} -> {deep}");
        assert!(deep <= ai.smart_chance_cap + 0.1, "the cap is not a cap: {deep}");
    }

    /// The roll is off the creature's own identity, so the same creature in the same fight
    /// always thinks the same way — a profile that flickered between ticks would be a
    /// creature that changes its mind for no reason a player can see.
    #[test]
    fn a_creatures_profile_is_reproducible() {
        let b = meld_balance::Balance::load_default().unwrap();
        for seed in [1u64, 99, 12345] {
            let a = creature_target_profile("dune_wyrm", "standard", 60, seed, &b.ai);
            let again = creature_target_profile("dune_wyrm", "standard", 60, seed, &b.ai);
            assert_eq!(a, again);
        }
    }

    #[test]
    fn every_creature_kind_has_a_pool_and_all_pools_are_well_formed() {
        let kinds = [
            "forest_bloom_stalker", "thornback_boar", "sporeling",
            "dune_wyrm", "sand_shade", "dune_colossus",
            "cinder_imp", "magma_golem", "ember_wisp",
            "frost_lurker", "ice_revenant", "glacier_maw",
            "bog_serpent", "myconid_brute", "bog_stinger",
            "gloamhound", "rustfang", "choirmother", "pyrewarden", "sepulcher",
            "hollowbishop", "ironmaw", "weepingcolossus", "miredrowned", "ashenleviathan",
        ];
        for kind in kinds {
            let pool = creature_abilities(kind);
            assert!(!pool.is_empty(), "{kind} has abilities");
            // Every pool opens early (the implicit basic attack covers L1;
            // a kind's first authored ability lands within the first band).
            assert!(
                pool.iter().any(|a| a.min_level <= 4),
                "{kind} has an early opener"
            );
            for a in &pool {
                assert!(a.weight > 0, "{kind}/{} weight", a.ability_kind);
                assert!(!a.callout_text.is_empty(), "{kind}/{} callout", a.ability_kind);
                assert!(!a.effects.is_empty(), "{kind}/{} effects", a.ability_kind);
                for e in &a.effects {
                    match e.effect_kind {
                        AbilityEffectKind::Damage => {
                            assert!(e.damage_type.is_some(), "{kind}/{} damage typed", a.ability_kind);
                            assert!(e.scaling_base.is_some() && e.coefficient.is_some());
                        }
                        AbilityEffectKind::Heal => {
                            assert!(e.scaling_base.is_some() && e.coefficient.is_some());
                        }
                        AbilityEffectKind::Status => {
                            assert!(e.status_name.is_some() && e.duration_ticks.is_some());
                        }
                        AbilityEffectKind::AtbManipulation => {
                            assert!(e.coefficient.is_some());
                        }
                        AbilityEffectKind::Steal => {
                            assert!(e.steal_target_kind.is_some());
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn pools_expand_with_level() {
        // The spec's spider example, transposed: a L1 stalker knows only its
        // opener tier; the L99 veteran's pool includes everything.
        let pool = creature_abilities("forest_bloom_stalker");
        let at = |lvl: i32| pool.iter().filter(|a| a.min_level <= lvl).count();
        assert!(at(1) < at(20), "higher level unlocks more abilities");
        assert_eq!(at(99), pool.len());
    }

    /// A REBUKE IS NOT ALWAYS A BLOW. The rebuke is a creature's rarest ability
    /// (`Battle::signature_ability`), so what a staggered boss answers with comes out of the
    /// kits already authored — and across the roster that has to include boss that MEND,
    /// SHIELD or hurry themselves as well as ones that swing. A roster whose every scarcest
    /// entry is damage makes the mechanic one note.
    #[test]
    fn the_rarest_thing_in_a_boss_book_is_not_always_damage() {
        use meld_proto::abilities::AbilityEffectKind as K;
        let mut kinds: std::collections::HashSet<&'static str> = Default::default();
        let mut bosses = 0;
        for key in meld_proto::bosses::keys() {
            let pool = creature_abilities(key);
            if pool.is_empty() {
                continue;
            }
            bosses += 1;
            // The same pick `signature_ability` makes: the rarest it could ever use.
            let rarest = pool.iter().min_by_key(|a| a.weight).expect("a non-empty kit");
            for e in &rarest.effects {
                kinds.insert(match e.effect_kind {
                    K::Damage => "damage",
                    K::Heal => "heal",
                    K::AtbManipulation => "atb",
                    K::Status => "status",
                    K::Steal => "steal",
                });
            }
        }
        assert!(bosses > 0, "no boss kits to check");
        assert!(
            kinds.len() > 1,
            "every boss's rarest ability does the same one thing ({kinds:?}) - the rebuke has \
             no variety in it, so staggering anything always means the same answer"
        );
    }

    /// FS-4: every named boss carries its own kit, distinct from any of the 15
    /// biome creatures, plus a signature telegraphed attack (not just a plain
    /// instant like the base creature kits mostly are).
    #[test]
    fn every_boss_has_a_distinct_kit_with_a_signature_telegraph() {
        let boss_keys = [
            "gloamhound", "rustfang", "choirmother", "pyrewarden", "sepulcher",
            "hollowbishop", "ironmaw", "weepingcolossus", "miredrowned", "ashenleviathan",
        ];
        for key in boss_keys {
            let pool = creature_abilities(key);
            assert!(!pool.is_empty(), "{key} has a kit");
            assert!(
                pool.iter().any(|a| a.telegraph_ticks > 0),
                "{key} has a telegraphed signature move"
            );
            assert_ne!(
                creature_basic_attack_type(key),
                DamageType::None,
                "{key} has a typed basic attack"
            );
        }
    }

    #[test]
    fn modifiers_cover_the_full_semantic_range() {
        // sand_shade absorbs shadow (< 0), magma_golem is immune to fire (0.0),
        // sporeling is weak to fire (> 1).
        let shade: std::collections::HashMap<_, _> =
            creature_damage_modifiers("sand_shade").into_iter().collect();
        assert!(shade[&Shadow] < 0.0);
        let golem: std::collections::HashMap<_, _> =
            creature_damage_modifiers("magma_golem").into_iter().collect();
        assert_eq!(golem[&Fire], 0.0);
        let spore: std::collections::HashMap<_, _> =
            creature_damage_modifiers("sporeling").into_iter().collect();
        assert!(spore[&Fire] > 1.0);
    }

    /// The wide half escalates and the single-target half does not — which is the whole
    /// claim. Raising a single-target row would be the attack-scaling mistake this module
    /// exists to avoid, wearing a cooldown's clothes.
    #[test]
    fn a_raid_tier_widens_the_kit_and_leaves_the_single_target_half_alone() {
        let base = creature_abilities("ironmaw");
        assert!(base.iter().any(|a| a.reaches_the_whole_party()), "the fixture has no wide row");
        assert!(base.iter().any(|a| !a.reaches_the_whole_party()), "the fixture has no single row");
        let raid = widen_for_warband(base.clone(), 4, 0.6, 0.4);
        assert_eq!(raid.len(), base.len(), "a raid tier must not add or drop rows");
        for (b, r) in base.iter().zip(raid.iter()) {
            assert_eq!(b.ability_kind, r.ability_kind, "the pool was reordered");
            // MAGNITUDES ARE UNTOUCHED. This is the property that makes the lever safe: no
            // number a hero takes changes, so no hit can become a one-shot for the party
            // that arrives before the merge fills.
            assert_eq!(b.effects, r.effects, "{} changed what it does, not how often", b.ability_kind);
            assert_eq!(b.telegraph_ticks, r.telegraph_ticks, "{} stopped announcing itself", b.ability_kind);
            assert_eq!(b.min_level, r.min_level);
            assert_eq!(b.hp_threshold_pct, r.hp_threshold_pct);
            if b.reaches_the_whole_party() {
                assert!(r.weight > b.weight, "{} is no likelier at four parties", b.ability_kind);
                assert!(r.cooldown_ticks < b.cooldown_ticks, "{} is no sooner", b.ability_kind);
            } else {
                assert_eq!(b.weight, r.weight, "{} was widened", b.ability_kind);
                assert_eq!(b.cooldown_ticks, r.cooldown_ticks, "{} was widened", b.ability_kind);
            }
        }
    }

    /// A bigger raid is a wider fight at every rung, and an ordinary encounter is untouched
    /// — a boss nobody labelled must fight exactly as it always has.
    #[test]
    fn the_escalation_is_monotonic_and_one_party_is_left_exactly_alone() {
        let base = creature_abilities("ashenleviathan");
        assert_eq!(widen_for_warband(base.clone(), 1, 0.6, 0.4), base, "an ordinary boss changed");
        let wide = |parties: u8| {
            widen_for_warband(base.clone(), parties, 0.6, 0.4)
                .into_iter()
                .filter(|a| a.reaches_the_whole_party())
                .map(|a| (a.weight, a.cooldown_ticks))
                .collect::<Vec<_>>()
        };
        for parties in 2..=meld_proto::warbands::max_parties() {
            for ((w, cd), (pw, pcd)) in wide(parties).into_iter().zip(wide(parties - 1)) {
                assert!(w >= pw, "{parties} parties is not likelier to go wide than {}", parties - 1);
                assert!(cd <= pcd, "{parties} parties does not come round sooner");
            }
        }
    }

    /// A shortened cooldown may never dip below the telegraph, or a raid blow becomes ready
    /// again before the last one has even landed — the shout would stop meaning anything.
    #[test]
    fn a_widened_cooldown_never_undercuts_its_own_telegraph() {
        for kind in all_bosses() {
            // Far past anything tunable, to prove the floor rather than the current numbers.
            for a in widen_for_warband(creature_abilities(kind), 4, 50.0, 500.0) {
                if a.reaches_the_whole_party() {
                    assert!(
                        a.cooldown_ticks >= a.telegraph_ticks.max(1),
                        "{kind}/{} is ready before it lands",
                        a.ability_kind
                    );
                }
            }
        }
    }

    /// Every named boss must be able to go WIDE at the shallowest level a gatekeeper is ever
    /// met at — not merely somewhere in its pool.
    ///
    /// A raid tier is expressed entirely through the wide half, so a boss that cannot reach
    /// any of it is labelled a Worldbreaker and fights exactly like an ordinary gatekeeper:
    /// one hero at a time, which at sixteen heroes is a sixteenth of a fight. That is the
    /// FS-4 bug over again, one layer down, and existence alone does not catch it — three
    /// bosses in ten (sepulcher, rustfang, gloamhound) HAD a party-wide ability and had it
    /// gated at level 45, while `gatekeeper_min_distance` puts the first gate boss at 24.
    ///
    /// The threshold is derived from balance rather than written down, so retuning where
    /// gatekeepers start retunes what a boss must be able to do when it gets there.
    #[test]
    fn every_boss_can_go_wide_at_the_level_a_gatekeeper_is_first_met() {
        let b = meld_balance::Balance::load_default().unwrap();
        let first_gate =
            crate::Scaling::new(&b).mlevel(b.encounters.gatekeeper_min_distance);
        for kind in all_bosses() {
            let pool = creature_abilities(kind);
            assert!(
                pool.iter().any(|a| a.reaches_the_whole_party()),
                "{kind} has no party-wide ability, so a raid tier cannot reach it"
            );
            // An hp_threshold row does not count: a boss that can only go wide once it is
            // nearly dead spends the whole fight unable to answer a crowd.
            assert!(
                pool.iter().any(|a| a.reaches_the_whole_party()
                    && a.min_level <= first_gate
                    && a.hp_threshold_pct.is_none()),
                "{kind} cannot go wide at level {first_gate}, where the first gatekeeper \
                 stands - a raid tier has nothing to escalate"
            );
        }
    }
}
