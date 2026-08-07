//! Every crafting material, declared once (roadmap `MS-1`).
//!
//! Materials carry a **class**, and the class is the design — it is what lets a
//! recipe or a vendor ask for a monster part specifically rather than for "some
//! item kind":
//!
//! - [`MaterialClass::Reagent`] — harvested plant/mineral matter. Alchemy's input.
//! - [`MaterialClass::Ore`] — harvested ore/wood. The Forge's *body*: what a piece
//!   of gear is actually made out of.
//! - [`MaterialClass::Trophy`] — cut from a felled creature. Alchemy's **trophy
//!   line** (potions only monster parts make) and the Forge's **catalyst** (quench
//!   a piece in a trophy and it comes out a tier better).
//!
//! `tier` is the biome band the material comes from (`meld-world`'s
//! `biome_for_distance` order: forest 0 → mire 4), which is what makes a deep
//! material worth more at the Broker and worth carrying home.

use serde::{Deserialize, Serialize};

/// What a material *is*, and therefore what will accept it. Recipes and the Forge
/// gate on this, so "forge a blade out of herbs" is a validation error rather
/// than a silently-accepted absurdity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MaterialClass {
    Reagent,
    Ore,
    Trophy,
}

impl MaterialClass {
    pub fn wire(&self) -> &'static str {
        match self {
            MaterialClass::Reagent => "reagent",
            MaterialClass::Ore => "ore",
            MaterialClass::Trophy => "trophy",
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct MaterialDef {
    /// Item kind on the wire, in the backpack and in the Vault (`bog_ichor`).
    pub key: &'static str,
    pub name: &'static str,
    pub class: MaterialClass,
    /// Biome band this comes from (forest 0 … mire 4). Drives Broker price.
    pub tier: i32,
    pub description: &'static str,
}

pub const MATERIALS: &[MaterialDef] = &[
    // --- Forest (tier 0) ---
    MaterialDef {
        key: "bloom_herb",
        name: "Bloom Herb",
        class: MaterialClass::Reagent,
        tier: 0,
        description: "Grows where the light gets through. The first thing every alchemist learns.",
    },
    MaterialDef {
        key: "heartoak_bark",
        name: "Heartoak Bark",
        class: MaterialClass::Ore,
        tier: 0,
        description: "Cut in strips from a tree that does not mind.",
    },
    MaterialDef {
        key: "forest_bloom_petal",
        name: "Bloom Stalker Petal",
        class: MaterialClass::Trophy,
        tier: 0,
        description: "Taken off the stalker that wore it as a lure. Still faintly sweet.",
    },
    // --- Desert (tier 1) ---
    MaterialDef {
        key: "sun_salts",
        name: "Sun Salts",
        class: MaterialClass::Reagent,
        tier: 1,
        description: "Scraped off stone the heat has been working on for centuries.",
    },
    MaterialDef {
        key: "dune_iron",
        name: "Dune Iron",
        class: MaterialClass::Ore,
        tier: 1,
        description: "Blown clear of the sand, already half-forged by the wind.",
    },
    MaterialDef {
        key: "sun_scarab_husk",
        name: "Sun Scarab Husk",
        class: MaterialClass::Trophy,
        tier: 1,
        description: "A shell that shrugged off a desert's worth of sun. It will shrug off worse.",
    },
    // --- Ashfall (tier 2) ---
    MaterialDef {
        key: "ember_ash",
        name: "Ember Ash",
        class: MaterialClass::Reagent,
        tier: 2,
        description: "Warm in the bag. Do not pack it against anything you like.",
    },
    MaterialDef {
        key: "cinder_ore",
        name: "Cinder Ore",
        class: MaterialClass::Ore,
        tier: 2,
        description: "Comes out of the ground already wanting to be a blade.",
    },
    MaterialDef {
        key: "ember_cinder",
        name: "Ember Cinder",
        class: MaterialClass::Trophy,
        tier: 2,
        description: "The coal an imp burned on. It never quite goes out.",
    },
    // --- Tundra (tier 3) ---
    MaterialDef {
        key: "frost_lichen",
        name: "Frost Lichen",
        class: MaterialClass::Reagent,
        tier: 3,
        description: "Grows a hand's width a century. Worth every year of it.",
    },
    MaterialDef {
        key: "rime_ore",
        name: "Rime Ore",
        class: MaterialClass::Ore,
        tier: 3,
        description: "Cold enough to work unquenched.",
    },
    MaterialDef {
        key: "frost_shard",
        name: "Frost Shard",
        class: MaterialClass::Trophy,
        tier: 3,
        description: "Cut out of a revenant. It is not water, and it will not melt.",
    },
    // --- Mire (tier 4) ---
    MaterialDef {
        key: "bog_myrrh",
        name: "Bog Myrrh",
        class: MaterialClass::Reagent,
        tier: 4,
        description: "Resin from something that drowned a long time ago.",
    },
    MaterialDef {
        key: "peat_iron",
        name: "Peat Iron",
        class: MaterialClass::Ore,
        tier: 4,
        description: "Dug black and stinking out of the water. Rings true anyway.",
    },
    MaterialDef {
        key: "bog_ichor",
        name: "Bog Ichor",
        class: MaterialClass::Trophy,
        tier: 4,
        description: "Drawn from a serpent that had swallowed the deep. Handle it sealed.",
    },
];

pub fn material(key: &str) -> Option<&'static MaterialDef> {
    MATERIALS.iter().find(|m| m.key == key)
}

/// Whether an item kind is a crafting material at all — as opposed to a potion, a
/// Town Portal, or gear. The Vault holds all of them in the same stacks.
pub fn is_material(key: &str) -> bool {
    material(key).is_some()
}

pub fn is_class(key: &str, class: MaterialClass) -> bool {
    material(key).map(|m| m.class == class).unwrap_or(false)
}

/// Every material of one class, in registry (shallow → deep) order.
pub fn materials_of_class(class: MaterialClass) -> Vec<&'static MaterialDef> {
    MATERIALS.iter().filter(|m| m.class == class).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_material_is_uniquely_keyed_and_describes_itself() {
        let mut seen = std::collections::HashSet::new();
        for m in MATERIALS {
            assert!(seen.insert(m.key), "duplicate material {}", m.key);
            assert!(!m.name.is_empty() && !m.description.is_empty(), "{}", m.key);
            assert!((0..=4).contains(&m.tier), "{} has tier {}", m.key, m.tier);
            assert_eq!(material(m.key).map(|d| d.key), Some(m.key));
        }
        assert!(!is_material("bloom_salve"), "a potion is not a material");
        assert!(!is_material("town_portal"));
    }

    #[test]
    fn every_biome_band_supplies_one_of_each_class() {
        // The three classes are the three ways a band pays you: harvest a reagent,
        // harvest an ore, or kill something. A band missing one is a hole in a
        // whole tier of recipes.
        for tier in 0..=4 {
            for class in [MaterialClass::Reagent, MaterialClass::Ore, MaterialClass::Trophy] {
                assert!(
                    MATERIALS.iter().any(|m| m.tier == tier && m.class == class),
                    "tier {tier} has no {}",
                    class.wire()
                );
            }
        }
        assert_eq!(materials_of_class(MaterialClass::Trophy).len(), 5);
    }
}
