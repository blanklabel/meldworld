//! Potions and the recipes that make them (roadmap `GR-4` + `MS-1`).
//!
//! Two registries, both content:
//!
//! - [`CONSUMABLES`] — what a potion DOES when a hero drinks it in battle. Every
//!   effect reuses a state the ATB engine already models (heal, Barrier, Regen,
//!   Evasion, banked Adrenaline), so a potion is authored content rather than new
//!   engine machinery.
//! - [`RECIPES`] — how a potion is made: inputs, output, the Meld skill the craft
//!   credits, and the **level that skill must have reached**. A potion credits
//!   **Alchemy**; only metalwork credits Forging.
//!
//! Magnitudes live in `[consumable]` `[TUNABLE]`s — the numbers are balance's, the
//! shape is here. A potion's [`ConsumableDef::potency`] is how many steps up its
//! own effect it sits, so the **trophy line** (potions made from monster parts,
//! [`crate::materials::MaterialClass::Trophy`]) is the same eight effects at a
//! bigger dose rather than eight new mechanics.

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
    /// Steps up this effect's own ladder: 0 is the standard dose, each step
    /// multiplies the magnitude by `[consumable] potency_per_step`. Lets the
    /// trophy line be *stronger* without being *different*.
    pub potency: i32,
    pub description: &'static str,
}

pub const CONSUMABLES: &[ConsumableDef] = &[
    ConsumableDef {
        key: "waking_salt",
        name: "Waking Salt",
        effect: ConsumableEffect::Revive,
        tier: 1,
        potency: 0,
        description: "Held under the nose of the fallen. Not pleasant. Effective.",
    },
    ConsumableDef {
        key: "insight_mote",
        name: "Insight Mote",
        effect: ConsumableEffect::Experience,
        tier: 1,
        potency: 0,
        description: "Someone else's hard-won lesson, bottled. Drink and know it.",
    },
    ConsumableDef {
        key: "bloom_salve",
        name: "Bloom Salve",
        effect: ConsumableEffect::Heal,
        tier: 0,
        potency: 0,
        description: "Field medicine. Closes what is open.",
    },
    ConsumableDef {
        key: "elixir",
        name: "Elixir",
        effect: ConsumableEffect::FullHeal,
        tier: 2,
        potency: 0,
        description: "Whole again, once.",
    },
    ConsumableDef {
        key: "bulwark_tonic",
        name: "Bulwark Tonic",
        effect: ConsumableEffect::Barrier,
        tier: 0,
        potency: 0,
        description: "Drink before the blow, not after.",
    },
    ConsumableDef {
        key: "mending_draught",
        name: "Mending Draught",
        effect: ConsumableEffect::Regen,
        tier: 0,
        potency: 0,
        description: "Slow, steady, and cheaper than a Resonant's blood.",
    },
    ConsumableDef {
        key: "ghostdust",
        name: "Ghostdust",
        effect: ConsumableEffect::Evasion,
        tier: 1,
        potency: 0,
        description: "Be somewhere else for a while.",
    },
    ConsumableDef {
        key: "fury_philtre",
        name: "Fury Philtre",
        effect: ConsumableEffect::Adrenaline,
        tier: 1,
        potency: 0,
        description: "Rage on credit. Explorers only.",
    },
    // --- The trophy line: the same effects, rendered out of monster parts. Each
    // is one step stronger than its reagent-line counterpart and gated behind a
    // real Alchemy level, so a felled creature is worth cutting up.
    ConsumableDef {
        key: "verdant_draught",
        name: "Verdant Draught",
        effect: ConsumableEffect::Regen,
        tier: 1,
        potency: 1,
        description: "The lure a stalker grew, boiled down. It keeps growing in you.",
    },
    ConsumableDef {
        key: "scarab_ward",
        name: "Scarab Ward",
        effect: ConsumableEffect::Barrier,
        tier: 1,
        potency: 1,
        description: "Ground husk, drunk thick. For a while you are wearing the desert's answer.",
    },
    ConsumableDef {
        key: "cinderblood_philtre",
        name: "Cinderblood Philtre",
        effect: ConsumableEffect::Adrenaline,
        tier: 2,
        potency: 1,
        description: "An imp's coal, still burning, in your blood instead of its own.",
    },
    ConsumableDef {
        key: "rimeglass_vial",
        name: "Rimeglass Vial",
        effect: ConsumableEffect::Evasion,
        tier: 2,
        potency: 1,
        description: "Whatever a revenant does instead of standing still, bottled.",
    },
    ConsumableDef {
        key: "ichor_salve",
        name: "Ichor Salve",
        effect: ConsumableEffect::Heal,
        tier: 3,
        potency: 2,
        description: "It closes wounds the way the mire closes over things. Do not watch.",
    },
    ConsumableDef {
        key: "quintessence",
        name: "Quintessence",
        effect: ConsumableEffect::Revive,
        tier: 4,
        potency: 2,
        description: "One part of every biome that tried to kill you. The fallen stand up nearly whole.",
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
/// `skill` XP — once `skill` has reached `min_level`.
#[derive(Debug, Clone, Copy)]
pub struct RecipeDef {
    pub key: &'static str,
    pub name: &'static str,
    pub inputs: &'static [(&'static str, i32)],
    pub output: &'static str,
    pub output_qty: i32,
    /// The Meld skill the craft credits (`alchemy` / `forging` / `mercantile`).
    pub skill: &'static str,
    /// Level of `skill` the crafter must have reached. Permanent progression:
    /// Meld levels never wipe, not even at season end, so the recipe book opening
    /// up is the crafter's own ladder — the same role `unlock_level` plays for
    /// abilities in [`crate::skills`].
    pub min_level: i32,
}

pub const RECIPES: &[RecipeDef] = &[
    RecipeDef {
        key: "bloom_salve",
        name: "Bloom Salve",
        inputs: &[("bloom_herb", 2)],
        output: "bloom_salve",
        output_qty: 1,
        skill: "alchemy",
        min_level: 1,
    },
    RecipeDef {
        key: "bulwark_tonic",
        name: "Bulwark Tonic",
        inputs: &[("heartoak_bark", 2), ("bloom_herb", 1)],
        output: "bulwark_tonic",
        output_qty: 1,
        skill: "alchemy",
        min_level: 1,
    },
    RecipeDef {
        key: "mending_draught",
        name: "Mending Draught",
        inputs: &[("bloom_herb", 1), ("sun_salts", 1)],
        output: "mending_draught",
        output_qty: 1,
        skill: "alchemy",
        min_level: 1,
    },
    RecipeDef {
        key: "ghostdust",
        name: "Ghostdust",
        inputs: &[("frost_lichen", 2), ("bog_myrrh", 1)],
        output: "ghostdust",
        output_qty: 1,
        skill: "alchemy",
        min_level: 3,
    },
    RecipeDef {
        key: "fury_philtre",
        name: "Fury Philtre",
        inputs: &[("ember_ash", 2), ("bog_myrrh", 1)],
        output: "fury_philtre",
        output_qty: 1,
        skill: "alchemy",
        min_level: 3,
    },
    RecipeDef {
        key: "waking_salt",
        name: "Waking Salt",
        inputs: &[("rime_ore", 1), ("bog_myrrh", 2)],
        output: "waking_salt",
        output_qty: 1,
        skill: "alchemy",
        min_level: 5,
    },
    RecipeDef {
        key: "elixir",
        name: "Elixir",
        inputs: &[("bloom_salve", 2), ("sun_salts", 2), ("rime_ore", 1)],
        output: "elixir",
        output_qty: 1,
        skill: "alchemy",
        min_level: 7,
    },
    RecipeDef {
        key: "town_portal",
        name: "Town Portal",
        inputs: &[("dune_iron", 1), ("sun_salts", 1)],
        output: "town_portal",
        output_qty: 1,
        skill: "forging",
        min_level: 1,
    },
    // --- The trophy line. Every one of these is keyed on a monster part, which is
    // the point: a creature felled anywhere in the world now has something a
    // crafter wants, and the deep bands' parts open the strongest doses.
    RecipeDef {
        key: "verdant_draught",
        name: "Verdant Draught",
        inputs: &[("forest_bloom_petal", 2), ("bloom_herb", 1)],
        output: "verdant_draught",
        output_qty: 1,
        skill: "alchemy",
        min_level: 2,
    },
    RecipeDef {
        key: "scarab_ward",
        name: "Scarab Ward",
        inputs: &[("sun_scarab_husk", 2), ("sun_salts", 1)],
        output: "scarab_ward",
        output_qty: 1,
        skill: "alchemy",
        min_level: 2,
    },
    RecipeDef {
        key: "cinderblood_philtre",
        name: "Cinderblood Philtre",
        inputs: &[("ember_cinder", 2), ("ember_ash", 1)],
        output: "cinderblood_philtre",
        output_qty: 1,
        skill: "alchemy",
        min_level: 4,
    },
    RecipeDef {
        key: "rimeglass_vial",
        name: "Rimeglass Vial",
        inputs: &[("frost_shard", 2), ("frost_lichen", 1)],
        output: "rimeglass_vial",
        output_qty: 1,
        skill: "alchemy",
        min_level: 4,
    },
    RecipeDef {
        key: "ichor_salve",
        name: "Ichor Salve",
        inputs: &[("bog_ichor", 2), ("bog_myrrh", 1)],
        output: "ichor_salve",
        output_qty: 1,
        skill: "alchemy",
        min_level: 6,
    },
    RecipeDef {
        key: "quintessence",
        name: "Quintessence",
        inputs: &[
            ("forest_bloom_petal", 1),
            ("sun_scarab_husk", 1),
            ("ember_cinder", 1),
            ("frost_shard", 1),
            ("bog_ichor", 1),
        ],
        output: "quintessence",
        output_qty: 1,
        skill: "alchemy",
        min_level: 9,
    },
];

pub fn recipe(key: &str) -> Option<&'static RecipeDef> {
    RECIPES.iter().find(|r| r.key == key)
}

/// Recipes in a stable display order for the crafting UI.
pub fn recipes_for_skill(skill: &str) -> Vec<&'static RecipeDef> {
    RECIPES.iter().filter(|r| r.skill == skill).collect()
}

/// Every recipe that consumes `item_kind` — the "what is this good for?" lookup a
/// player asks of a stack in their Vault, and the check that keeps a material from
/// becoming a dead end.
pub fn recipes_consuming(item_kind: &str) -> Vec<&'static RecipeDef> {
    RECIPES
        .iter()
        .filter(|r| r.inputs.iter().any(|(k, _)| *k == item_kind))
        .collect()
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
            assert!(r.min_level >= 1, "{} unlocks below level 1", r.key);
            assert_eq!(recipe(r.key).map(|d| d.key), Some(r.key));
        }
    }

    #[test]
    fn every_recipe_input_is_a_real_material_or_something_craftable() {
        for r in RECIPES {
            for (kind, qty) in r.inputs {
                assert!(qty > &0, "{} asks for {qty} {kind}", r.key);
                assert!(
                    crate::materials::is_material(kind) || recipe(kind).is_some(),
                    "{} needs {kind}, which nothing in the world produces",
                    r.key
                );
            }
        }
    }

    #[test]
    fn every_material_the_world_drops_has_somewhere_to_go() {
        // The reason this test exists: a material with no recipe and no shelf is
        // loot the player can never spend, and that is invisible until someone
        // audits the tables by hand. Trophies (combat drops) are the ones that
        // went unspent, so they get the strictest form of the check.
        use crate::materials::{MaterialClass, MATERIALS};
        for m in MATERIALS {
            // An ore's sink is the Forge, which takes any ore by class rather than
            // by name — so a recipe is not the bar for those.
            if m.class == MaterialClass::Ore {
                continue;
            }
            assert!(
                !recipes_consuming(m.key).is_empty(),
                "{} is dead loot: no recipe consumes it",
                m.key
            );
        }
        for t in crate::materials::materials_of_class(MaterialClass::Trophy) {
            let line: Vec<&str> = recipes_consuming(t.key).iter().map(|r| r.key).collect();
            assert!(
                line.iter().any(|k| *k != "quintessence"),
                "{} is only good for the capstone: {line:?}",
                t.key
            );
        }
    }

    #[test]
    fn the_trophy_line_is_stronger_than_the_reagent_line_it_shadows() {
        // A monster part costs a fight; a herb costs a walk. If the trophy line
        // were not the bigger dose there would be no reason to prefer it.
        for (reagent_potion, trophy_potion) in [
            ("mending_draught", "verdant_draught"),
            ("bulwark_tonic", "scarab_ward"),
            ("fury_philtre", "cinderblood_philtre"),
            ("ghostdust", "rimeglass_vial"),
            ("bloom_salve", "ichor_salve"),
            ("waking_salt", "quintessence"),
        ] {
            let base = consumable(reagent_potion).unwrap();
            let trophy = consumable(trophy_potion).unwrap();
            assert_eq!(base.effect, trophy.effect, "{trophy_potion} shadows {reagent_potion}");
            assert!(
                trophy.potency > base.potency,
                "{trophy_potion} is no stronger than {reagent_potion}"
            );
            let r = recipe(trophy_potion).expect("trophy potion is craftable");
            assert!(
                r.inputs
                    .iter()
                    .any(|(k, _)| crate::materials::is_class(k, crate::materials::MaterialClass::Trophy)),
                "{trophy_potion} is not made of monster parts"
            );
            assert!(
                r.min_level > recipe(reagent_potion).map(|b| b.min_level).unwrap_or(1)
                    || r.min_level >= 2,
                "{trophy_potion} is not gated above the basics"
            );
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
