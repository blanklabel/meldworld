//! Potions and the recipes that make them (roadmap `GR-4` + `MS-1`).
//!
//! Two registries, both content:
//!
//! - [`CONSUMABLES`] — what a potion DOES when a hero drinks it in battle. Every
//!   effect reuses a state the ATB engine already models (heal, Barrier, Regen,
//!   Evasion, banked Adrenaline), so a potion is authored content rather than new
//!   engine machinery.
//! - [`RECIPES`] — how a potion is made: inputs, output, and the Meld skill the
//!   craft credits. A potion credits **Alchemy**; only metalwork credits Forging.
//!
//! Magnitudes live in `[consumable]` `[TUNABLE]`s — the numbers are balance's, the
//! shape is here.

use serde::{Deserialize, Serialize};

/// What drinking a potion does. Deliberately drawn from states the engine already
/// has: a potion that needed new combat machinery would be a mechanic wearing a
/// potion's clothes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConsumableEffect {
    /// Restore a fraction of max HP (the fraction is a balance tunable).
    Heal,
    /// Restore all of it.
    FullHeal,
    /// Grant Barrier — temporary HP that decays.
    Barrier,
    /// Grant Regen — HP back at the start of each turn.
    Regen,
    /// Grant Evasion — a temporary dodge bonus.
    Evasion,
    /// Bank Adrenaline (Explorer only; inert on anyone else, like the affix).
    Adrenaline,
    /// Bring a FALLEN hero back. The only way back up — a level-up no longer
    /// revives anyone, so a wipe has to be answered with an item.
    Revive,
    /// Grant XP to the hero who drinks it. The world's XP is mostly earned; this is
    /// the part of it you can carry home and choose who to spend on.
    Experience,
}

/// One potion.
#[derive(Debug, Clone, Copy)]
pub struct ConsumableDef {
    /// Item kind on the wire and in the Vault (`bloom_salve`).
    pub key: &'static str,
    pub name: &'static str,
    pub effect: ConsumableEffect,
    /// Which tier of the shop/recipe ladder this sits on; 0 is the basics an
    /// Apothecary stocks for a new player.
    pub tier: i32,
    pub description: &'static str,
}

pub const CONSUMABLES: &[ConsumableDef] = &[
    ConsumableDef {
        key: "waking_salt",
        name: "Waking Salt",
        effect: ConsumableEffect::Revive,
        tier: 1,
        description: "Held under the nose of the fallen. Not pleasant. Effective.",
    },
    ConsumableDef {
        key: "insight_mote",
        name: "Insight Mote",
        effect: ConsumableEffect::Experience,
        tier: 1,
        description: "Someone else's hard-won lesson, bottled. Drink and know it.",
    },
    ConsumableDef {
        key: "bloom_salve",
        name: "Bloom Salve",
        effect: ConsumableEffect::Heal,
        tier: 0,
        description: "Field medicine. Closes what is open.",
    },
    ConsumableDef {
        key: "elixir",
        name: "Elixir",
        effect: ConsumableEffect::FullHeal,
        tier: 2,
        description: "Whole again, once.",
    },
    ConsumableDef {
        key: "bulwark_tonic",
        name: "Bulwark Tonic",
        effect: ConsumableEffect::Barrier,
        tier: 0,
        description: "Drink before the blow, not after.",
    },
    ConsumableDef {
        key: "mending_draught",
        name: "Mending Draught",
        effect: ConsumableEffect::Regen,
        tier: 0,
        description: "Slow, steady, and cheaper than a Resonant's blood.",
    },
    ConsumableDef {
        key: "ghostdust",
        name: "Ghostdust",
        effect: ConsumableEffect::Evasion,
        tier: 1,
        description: "Be somewhere else for a while.",
    },
    ConsumableDef {
        key: "fury_philtre",
        name: "Fury Philtre",
        effect: ConsumableEffect::Adrenaline,
        tier: 1,
        description: "Rage on credit. Explorers only.",
    },
];

pub fn consumable(key: &str) -> Option<&'static ConsumableDef> {
    CONSUMABLES.iter().find(|c| c.key == key)
}

/// Whether an item kind is a drinkable potion (as opposed to a material, a Town
/// Portal, or anything else that lives in the same stacks).
pub fn is_consumable(key: &str) -> bool {
    consumable(key).is_some()
}

/// A crafting recipe: `inputs` (item kind, quantity) become `output`, crediting
/// `skill` XP.
#[derive(Debug, Clone, Copy)]
pub struct RecipeDef {
    pub key: &'static str,
    pub name: &'static str,
    pub inputs: &'static [(&'static str, i32)],
    pub output: &'static str,
    pub output_qty: i32,
    /// The Meld skill the craft credits (`alchemy` / `forging` / `mercantile`).
    pub skill: &'static str,
}

pub const RECIPES: &[RecipeDef] = &[
    RecipeDef {
        key: "bloom_salve",
        name: "Bloom Salve",
        inputs: &[("bloom_herb", 2)],
        output: "bloom_salve",
        output_qty: 1,
        skill: "alchemy",
    },
    RecipeDef {
        key: "bulwark_tonic",
        name: "Bulwark Tonic",
        inputs: &[("heartoak_bark", 2), ("bloom_herb", 1)],
        output: "bulwark_tonic",
        output_qty: 1,
        skill: "alchemy",
    },
    RecipeDef {
        key: "mending_draught",
        name: "Mending Draught",
        inputs: &[("bloom_herb", 1), ("sun_salts", 1)],
        output: "mending_draught",
        output_qty: 1,
        skill: "alchemy",
    },
    RecipeDef {
        key: "ghostdust",
        name: "Ghostdust",
        inputs: &[("frost_lichen", 2), ("bog_myrrh", 1)],
        output: "ghostdust",
        output_qty: 1,
        skill: "alchemy",
    },
    RecipeDef {
        key: "fury_philtre",
        name: "Fury Philtre",
        inputs: &[("ember_ash", 2), ("bog_myrrh", 1)],
        output: "fury_philtre",
        output_qty: 1,
        skill: "alchemy",
    },
    RecipeDef {
        key: "waking_salt",
        name: "Waking Salt",
        inputs: &[("rime_ore", 1), ("bog_myrrh", 2)],
        output: "waking_salt",
        output_qty: 1,
        skill: "alchemy",
    },
    RecipeDef {
        key: "elixir",
        name: "Elixir",
        inputs: &[("bloom_salve", 2), ("sun_salts", 2), ("rime_ore", 1)],
        output: "elixir",
        output_qty: 1,
        skill: "alchemy",
    },
    RecipeDef {
        key: "town_portal",
        name: "Town Portal",
        inputs: &[("dune_iron", 1), ("sun_salts", 1)],
        output: "town_portal",
        output_qty: 1,
        skill: "forging",
    },
];

pub fn recipe(key: &str) -> Option<&'static RecipeDef> {
    RECIPES.iter().find(|r| r.key == key)
}

/// Recipes in a stable display order for the crafting UI.
pub fn recipes_for_skill(skill: &str) -> Vec<&'static RecipeDef> {
    RECIPES.iter().filter(|r| r.skill == skill).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_recipe_makes_something_real_from_something_real() {
        for r in RECIPES {
            assert!(!r.inputs.is_empty(), "{} needs no materials", r.key);
            assert!(r.output_qty > 0);
            assert!(
                matches!(r.skill, "alchemy" | "forging" | "mercantile"),
                "{} credits unknown skill {}",
                r.key,
                r.skill
            );
            // An output nobody can use is a dead recipe: it must be a potion, or a
            // known utility item (the Town Portal).
            assert!(
                is_consumable(r.output) || r.output == "town_portal",
                "{} makes {} which nothing can use",
                r.key,
                r.output
            );
            // A recipe may not eat its own output as an input, or crafting it once
            // makes it craftable forever from nothing.
            assert!(
                !r.inputs.iter().any(|(k, _)| *k == r.output),
                "{} consumes its own output",
                r.key
            );
            assert_eq!(recipe(r.key).map(|d| d.key), Some(r.key));
        }
    }

    #[test]
    fn potions_credit_alchemy_not_forging() {
        for r in RECIPES {
            if is_consumable(r.output) {
                assert_eq!(r.skill, "alchemy", "{} is a potion", r.key);
            }
        }
        assert!(recipes_for_skill("alchemy").len() >= 5);
        assert_eq!(recipes_for_skill("forging").len(), 1);
    }

    #[test]
    fn the_shop_tier_zero_basics_are_the_ones_a_new_player_needs() {
        let basics: Vec<&str> = CONSUMABLES
            .iter()
            .filter(|c| c.tier == 0)
            .map(|c| c.key)
            .collect();
        // Heal, Barrier and Regen: survive a fight, blunt a fight, outlast a fight.
        assert!(basics.contains(&"bloom_salve"), "{basics:?}");
        assert!(basics.contains(&"bulwark_tonic"), "{basics:?}");
        assert!(basics.contains(&"mending_draught"), "{basics:?}");
        // The full heal is NOT a starter item — it would flatten the early risk.
        assert_eq!(consumable("elixir").unwrap().tier, 2);
    }

    #[test]
    fn every_potion_describes_itself_and_is_uniquely_keyed() {
        let mut seen = std::collections::HashSet::new();
        for c in CONSUMABLES {
            assert!(seen.insert(c.key), "duplicate consumable {}", c.key);
            assert!(!c.name.is_empty() && !c.description.is_empty(), "{}", c.key);
            assert!(is_consumable(c.key));
        }
        assert!(!is_consumable("bloom_herb"), "a material is not a potion");
        assert!(!is_consumable("town_portal"), "the portal is not drunk");
    }
}
