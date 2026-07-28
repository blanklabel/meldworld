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
use meld_proto::enums::DamageType;

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
    Blunt, Celestial, Earth, Fire, Ice, Lightning, Mind, Pierce, Poison, Shadow, Slash, Water,
    Wind,
};
use ScalingBase::{Attack, Magic, MaxHp};

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
        // Unknown kinds fight with basic attacks only (still a full combatant).
        _ => vec![],
    }
}

/// A creature kind's elemental profile — `damage_modifiers` in the spec.
/// `> 1.0` weakness, `< 1.0` resistance, `0.0` immunity, `< 0.0` absorption
/// (heals for the absolute value). Types not listed default to `1.0`.
pub fn creature_damage_modifiers(kind: &str) -> Vec<(DamageType, f64)> {
    match kind {
        "forest_bloom_stalker" => vec![(Fire, 2.0), (Water, 0.5), (Earth, 0.5)],
        "thornback_boar" => vec![(Pierce, 0.75), (Mind, 1.5)],
        "sporeling" => vec![(Fire, 2.0), (Poison, 0.0)],
        "dune_wyrm" => vec![(Water, 2.0), (Earth, 0.5), (Fire, 0.75)],
        "sand_shade" => vec![(Celestial, 2.0), (Shadow, -0.5), (Blunt, 0.75)],
        "dune_colossus" => vec![(Water, 1.5), (Lightning, 1.5), (Blunt, 0.5), (Pierce, 0.5)],
        "cinder_imp" => vec![(Water, 2.0), (Ice, 1.5), (Fire, -1.0)],
        "magma_golem" => vec![(Water, 2.0), (Ice, 2.0), (Fire, 0.0), (Poison, 0.0), (Blunt, 0.75)],
        "ember_wisp" => vec![(Water, 2.0), (Wind, 1.5), (Fire, -1.0), (Earth, 0.75)],
        "frost_lurker" => vec![(Fire, 2.0), (Ice, 0.5)],
        "ice_revenant" => vec![(Fire, 1.5), (Celestial, 2.0), (Ice, -0.5), (Poison, 0.0), (Shadow, 0.5)],
        "glacier_maw" => vec![(Fire, 2.0), (Ice, 0.0), (Blunt, 0.75)],
        "bog_serpent" => vec![(Ice, 1.5), (Poison, -0.5), (Earth, 0.75)],
        "myconid_brute" => vec![(Fire, 2.0), (Poison, 0.0), (Slash, 1.25)],
        "bog_stinger" => vec![(Fire, 1.5), (Wind, 1.5), (Poison, 0.0)],
        _ => vec![],
    }
}

/// The [`DamageType`] a creature kind's *basic* attack carries (the fallback
/// swing the AI mixes into every pool). Physical for most; a few exotics burn.
pub fn creature_basic_attack_type(kind: &str) -> DamageType {
    match kind {
        "thornback_boar" | "dune_colossus" | "magma_golem" | "glacier_maw" | "myconid_brute" => {
            Blunt
        }
        "forest_bloom_stalker" | "sand_shade" | "cinder_imp" | "ice_revenant" => Slash,
        "ember_wisp" => Fire,
        // Wyrms, lurkers, stingers, serpents, sporelings: piercing fangs/stings.
        _ => Pierce,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_creature_kind_has_a_pool_and_all_pools_are_well_formed() {
        let kinds = [
            "forest_bloom_stalker", "thornback_boar", "sporeling",
            "dune_wyrm", "sand_shade", "dune_colossus",
            "cinder_imp", "magma_golem", "ember_wisp",
            "frost_lurker", "ice_revenant", "glacier_maw",
            "bog_serpent", "myconid_brute", "bog_stinger",
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
}
