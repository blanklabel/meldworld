//! Every crafting material, declared once (roadmap `MS-1`).
//!
//! Materials carry a **class**, and the class is the design — it is what lets a
//! recipe or a vendor ask for a monster part specifically rather than for "some
//! item kind":
//!
//! - [`MaterialClass::Reagent`] — harvested plant/mineral matter. Alchemy's input.
//! - [`MaterialClass::Ore`] — harvested ore/wood. The Forge's *body*: what a piece
//!   of gear is actually made out of.
//! - [`MaterialClass::Refined`] — raw ore with the volatility boiled out of it. What
//!   the Forge actually builds with: the Foundry's Smelter caste stands between the
//!   ground and the anvil, and so does this class.
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
    Refined,
    Trophy,
    /// Timber. **Only exists where trees do** — the forest bands and the mire's bog roots —
    /// which is what makes a biome an economy rather than a palette. A builder in the
    /// desert has to haul wood in or build in stone.
    Wood,
    /// Masonry. Every biome has some, because every biome has ground.
    Stone,
}

impl MaterialClass {
    /// Can you BUILD with it? (BD-1.) Wood and stone are what structures are made of;
    /// reagents, ore, refined stock and trophies are the crafting economy.
    ///
    /// This is the question `[building]` costs and the repair path ask, rather than each
    /// site naming the two classes — a third structural material (clay is next) then
    /// becomes one row here instead of a hunt through every call site.
    pub fn is_structural(&self) -> bool {
        matches!(self, MaterialClass::Wood | MaterialClass::Stone)
    }

    pub fn wire(&self) -> &'static str {
        match self {
            MaterialClass::Reagent => "reagent",
            MaterialClass::Ore => "ore",
            MaterialClass::Refined => "refined",
            MaterialClass::Trophy => "trophy",
            MaterialClass::Wood => "wood",
            MaterialClass::Stone => "stone",
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
    // --- Refined stock (`MS-1`): what a Smelter hands the anvil. One per ore, same
    // band as the ore it came out of — smelting stabilises material, it does not move
    // it up the world.
    MaterialDef {
        key: "heartoak_stave",
        name: "Heartoak Stave",
        class: MaterialClass::Refined,
        tier: 0,
        description: "Seasoned until it stopped arguing. Takes a haft or a bow-back.",
    },
    MaterialDef {
        key: "dune_ingot",
        name: "Dune Ingot",
        class: MaterialClass::Refined,
        tier: 1,
        description: "The sand cooked out of it. What is left rings when struck.",
    },
    MaterialDef {
        key: "cinder_ingot",
        name: "Cinder Ingot",
        class: MaterialClass::Refined,
        tier: 2,
        description: "It came out of the furnace hotter than it went in. Nobody asks why.",
    },
    MaterialDef {
        key: "rime_ingot",
        name: "Rime Ingot",
        class: MaterialClass::Refined,
        tier: 3,
        description: "Smelted cold, which should not work. The Foundry stopped filing reports.",
    },
    MaterialDef {
        key: "peat_ingot",
        name: "Peat Ingot",
        class: MaterialClass::Refined,
        tier: 4,
        description: "Black iron with the drowning boiled off. Holds an edge like a grudge.",
    },
    // --- BD-1: STRUCTURAL MATERIALS -----------------------------------------------------
    //
    // What a town is made of. Deliberately NOT one generic "wood" and one "stone": a
    // structural material carries its band like every other material does, so hauling
    // deep stone home means something and the Broker prices it accordingly.
    //
    // ⚠️ WOOD IS NOT IN EVERY BIOME, AND THAT IS THE POINT. There is no timber in the
    // desert, the ashfall or the tundra, because there are no trees there — those bands
    // grow `cactus`, `cinder_rock` and `ice_spire`. So the material tables and the
    // obstacle tables tell the same story, and a builder out on the ash either carries
    // timber with them or builds in stone. It is the first thing in the game that makes
    // one biome's ground worth more to you than another's.
    //
    // Wood comes from deadfall nodes rather than from felling the standing trees you can
    // see. Standing timber wants the ecology's `Flora` (CR), which is unbuilt — and
    // inventing a parallel harvestable-obstacle system beside it is how you end up with
    // two answers to "what is a tree". When CR lands, wood should move onto the trees.
    MaterialDef {
        key: "heartoak_log",
        name: "Heartoak Log",
        class: MaterialClass::Wood,
        tier: 0,
        description: "Deadfall from the old wood. Heavy, straight, and it does not rot in a season.",
    },
    MaterialDef {
        key: "bog_root_timber",
        name: "Bog-Root Timber",
        class: MaterialClass::Wood,
        tier: 4,
        description: "Hauled black and dripping from the peat. The water has been in it so long \
                      that nothing else will get in.",
    },
    MaterialDef {
        key: "river_granite",
        name: "River Granite",
        class: MaterialClass::Stone,
        tier: 0,
        description: "Rounded by water long before anyone came to pick it up.",
    },
    MaterialDef {
        key: "sun_sandstone",
        name: "Sun Sandstone",
        class: MaterialClass::Stone,
        tier: 1,
        description: "Cuts like cheese and sets like iron. Every wall in the dune country is this.",
    },
    MaterialDef {
        key: "basalt_slab",
        name: "Basalt Slab",
        class: MaterialClass::Stone,
        tier: 2,
        description: "Cooled where it stopped. Still the shape the flow left it.",
    },
    MaterialDef {
        key: "rime_stone",
        name: "Rime Stone",
        class: MaterialClass::Stone,
        tier: 3,
        description: "Frost-split off the spires. It comes away in courses, already squared.",
    },
    MaterialDef {
        key: "peat_shale",
        name: "Peat Shale",
        class: MaterialClass::Stone,
        tier: 4,
        description: "Layered flat under the bog. Lift one sheet and the next is waiting.",
    },

];

/// The **refined** form of a raw ore, or `None` for anything that isn't smeltable.
/// Structural: the Smelter's whole job as a lookup.
pub fn refined_form(ore: &str) -> Option<&'static str> {
    Some(match ore {
        "heartoak_bark" => "heartoak_stave",
        "dune_iron" => "dune_ingot",
        "cinder_ore" => "cinder_ingot",
        "rime_ore" => "rime_ingot",
        "peat_iron" => "peat_ingot",
        _ => return None,
    })
}

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

    #[test]
    fn every_ore_has_exactly_one_refined_form_in_its_own_band() {
        // The Forge builds from refined stock, so an ore with no refined form is an
        // ore nothing can be made of — and a refined form in the wrong band would let
        // smelting launder shallow material into deep gear.
        let ores = materials_of_class(MaterialClass::Ore);
        let refined = materials_of_class(MaterialClass::Refined);
        assert_eq!(ores.len(), refined.len(), "one refined form per ore");
        for ore in &ores {
            let out = refined_form(ore.key)
                .unwrap_or_else(|| panic!("{} cannot be smelted into anything", ore.key));
            let def = material(out).unwrap_or_else(|| panic!("{out} is not registered"));
            assert_eq!(def.class, MaterialClass::Refined, "{out}");
            assert_eq!(def.tier, ore.tier, "{out} left {}'s band", ore.key);
        }
        // Nothing else claims to be smeltable.
        for m in MATERIALS {
            if m.class != MaterialClass::Ore {
                assert!(refined_form(m.key).is_none(), "{} is not an ore", m.key);
            }
        }
    }
}
