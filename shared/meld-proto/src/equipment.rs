//! Equipment legality: which item families and armor weights each class may
//! wear, and how many hands a weapon takes (roadmap GR-5,
//! docs/proposals/gear-identity.md §1).
//!
//! This is the **single source of truth** both sides read: the server enforces it
//! at equip / derivation / loot generation, and the client greys illegal rows with
//! the same table instead of a second, drifting copy. Legality is a *rule*, not a
//! coefficient, so it lives in code rather than `balance.toml` (working
//! agreement #2 — structure in code, numbers in config).

use serde::{Deserialize, Serialize};

use crate::enums::CharacterClass;

/// What kind of thing an equippable item is. Weapons and shields are families;
/// armor carries an [`ArmorWeight`] instead; accessories are unrestricted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ItemFamily {
    Sword,
    Shield,
    /// Two-handed reach weapon.
    Spear,
    /// Two-handed caster weapon (Resonant).
    Staff,
    /// Two-handed psychic focus (Psyker).
    Globe,
    Gauntlet,
    Dagger,
    /// Defensive off-hand blade (Shifter).
    ParryBlade,
}

/// Armor weight band. A class allows a *set* of these, so most drops fit more
/// than one hero — the reason weights exist instead of per-class armor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArmorWeight {
    Heavy,
    Medium,
    Light,
    Robe,
}

impl ItemFamily {
    /// Hands the family occupies. A 2-hand weapon fills `main_hand` and reserves
    /// `off_hand` (see [`reserves_off_hand`](ItemFamily::reserves_off_hand)).
    pub fn hands(self) -> u8 {
        match self {
            ItemFamily::Spear | ItemFamily::Staff | ItemFamily::Globe => 2,
            _ => 1,
        }
    }

    pub fn reserves_off_hand(self) -> bool {
        self.hands() == 2
    }

    /// Whether the family can sit in the given slot category. A dagger is the one
    /// family legal in either hand (the Shifter's dual-wield).
    pub fn fits_slot(self, slot: &str) -> bool {
        match self {
            ItemFamily::Shield | ItemFamily::ParryBlade => slot == "off_hand",
            ItemFamily::Dagger => slot == "main_hand" || slot == "off_hand",
            _ => slot == "main_hand",
        }
    }

    pub fn wire(self) -> &'static str {
        match self {
            ItemFamily::Sword => "sword",
            ItemFamily::Shield => "shield",
            ItemFamily::Spear => "spear",
            ItemFamily::Staff => "staff",
            ItemFamily::Globe => "globe",
            ItemFamily::Gauntlet => "gauntlet",
            ItemFamily::Dagger => "dagger",
            ItemFamily::ParryBlade => "parry_blade",
        }
    }

    pub fn from_wire(s: &str) -> Option<Self> {
        Some(match s {
            "sword" => ItemFamily::Sword,
            "shield" => ItemFamily::Shield,
            "spear" => ItemFamily::Spear,
            "staff" => ItemFamily::Staff,
            "globe" => ItemFamily::Globe,
            "gauntlet" => ItemFamily::Gauntlet,
            "dagger" => ItemFamily::Dagger,
            "parry_blade" => ItemFamily::ParryBlade,
            _ => return None,
        })
    }
}

impl ArmorWeight {
    pub fn wire(self) -> &'static str {
        match self {
            ArmorWeight::Heavy => "heavy",
            ArmorWeight::Medium => "medium",
            ArmorWeight::Light => "light",
            ArmorWeight::Robe => "robe",
        }
    }

    pub fn from_wire(s: &str) -> Option<Self> {
        Some(match s {
            "heavy" => ArmorWeight::Heavy,
            "medium" => ArmorWeight::Medium,
            "light" => ArmorWeight::Light,
            "robe" => ArmorWeight::Robe,
            _ => return None,
        })
    }
}

/// The weapon families a class may wear. Each class gets a recognizable hand:
/// Explorer chooses between sword+shield and a two-handed spear, Resonant and
/// Psyker have both hands full (staff / globe), Phoenix Guard cannot reach past its
/// own arms (gauntlet+shield), and the Shifter's off-hand is a build decision —
/// a second dagger (aggressive) or a parrying blade (defensive).
pub fn weapon_families(class: CharacterClass) -> &'static [ItemFamily] {
    match class {
        CharacterClass::Explorer => &[ItemFamily::Sword, ItemFamily::Shield, ItemFamily::Spear],
        CharacterClass::Resonant => &[ItemFamily::Staff],
        CharacterClass::Psyker => &[ItemFamily::Globe],
        CharacterClass::PhoenixGuard => &[ItemFamily::Gauntlet, ItemFamily::Shield],
        CharacterClass::Shifter => &[ItemFamily::Dagger, ItemFamily::ParryBlade],
        // The unbuilt roster classes inherit the martial baseline until each gets
        // its own kit — never an empty set, which would lock a hero out of gear.
        _ => &[ItemFamily::Sword, ItemFamily::Shield, ItemFamily::Spear],
    }
}

/// The armor weights a class may wear.
pub fn armor_weights(class: CharacterClass) -> &'static [ArmorWeight] {
    match class {
        CharacterClass::PhoenixGuard => &[ArmorWeight::Heavy, ArmorWeight::Medium],
        CharacterClass::Explorer => &[ArmorWeight::Medium, ArmorWeight::Light],
        CharacterClass::Shifter => &[ArmorWeight::Light],
        CharacterClass::Resonant => &[ArmorWeight::Robe, ArmorWeight::Light],
        CharacterClass::Psyker => &[ArmorWeight::Robe],
        _ => &[ArmorWeight::Medium, ArmorWeight::Light],
    }
}

pub fn allows_family(class: CharacterClass, family: ItemFamily) -> bool {
    weapon_families(class).contains(&family)
}

pub fn allows_weight(class: CharacterClass, weight: ArmorWeight) -> bool {
    armor_weights(class).contains(&weight)
}

/// The class a `gear.class_key` / `[player.<key>]` string names.
pub fn class_from_key(key: &str) -> Option<CharacterClass> {
    Some(match key {
        "explorer" => CharacterClass::Explorer,
        "hunter" => CharacterClass::Hunter,
        "dragoon" => CharacterClass::Dragoon,
        "sage" => CharacterClass::Sage,
        "ranger" => CharacterClass::Ranger,
        "alchemist_knight" => CharacterClass::AlchemistKnight,
        "bard" => CharacterClass::Bard,
        "psyker" => CharacterClass::Psyker,
        "resonant" => CharacterClass::Resonant,
        "shifter" => CharacterClass::Shifter,
        "phoenix_guard" => CharacterClass::PhoenixGuard,
        _ => return None,
    })
}

/// The families a class can put in `slot`, in declaration order. Empty for a
/// class that cannot fill that slot at all — a two-handed class has **no**
/// off-hand, which is what makes "both hands full" a real cost.
pub fn families_for_slot(class: CharacterClass, slot: &str) -> Vec<ItemFamily> {
    weapon_families(class)
        .iter()
        .copied()
        .filter(|f| f.fits_slot(slot))
        .collect()
}

/// Whether this class has an off-hand to fill at all.
pub fn has_off_hand(class: CharacterClass) -> bool {
    !families_for_slot(class, "off_hand").is_empty()
        && !weapon_families(class).iter().all(|f| f.reserves_off_hand())
}

/// The weight a drop for this class rolls: the heaviest the class allows, so a
/// class's armor reads as its own (an Phoenix Guard drop is plate, not the medium it
/// merely tolerates).
pub fn drop_weight(class: CharacterClass) -> ArmorWeight {
    armor_weights(class).first().copied().unwrap_or(ArmorWeight::Medium)
}

/// The six equipment categories of the 7-slot loadout, in display order (two
/// accessory equip slots share the one `accessory` category).
pub const SLOT_CATEGORIES: [&str; 6] =
    ["main_hand", "off_hand", "head", "chest", "legs", "accessory"];

/// The slots that carry an [`ArmorWeight`]. Hands carry a family instead, and
/// accessories are unrestricted so every loot table has a never-dead family.
pub fn is_armor_slot(slot: &str) -> bool {
    matches!(slot, "head" | "chest" | "legs")
}

/// Why an equip was refused. `Ok` is the only legal outcome; everything else maps
/// to a `409` with a code naming the rule that failed, so the client can say
/// *which* rule rather than "cannot equip".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Legality {
    Ok,
    /// The item's family is not in this class's list (a Resonant holding a spear).
    ClassFamily,
    /// The item's armor weight is not in this class's set (a Psyker in plate).
    ClassWeight,
    /// A signature piece named for another class.
    ClassExclusive,
    /// Right class, wrong hole (a shield in the main hand).
    SlotMismatch,
}

/// Can `class` (a hero also identified by `class_key` for signature pieces) put
/// this item in `slot`?
///
/// `family` / `weight` are the item's parsed descriptors; `None` for either means
/// the item carries no such descriptor and is unrestricted on that axis, so a
/// plain stat stick remains wearable by anyone. An empty `class_key` means the
/// item is not a signature piece.
pub fn check_equip(
    class: CharacterClass,
    class_key: &str,
    slot: &str,
    family: Option<ItemFamily>,
    weight: Option<ArmorWeight>,
) -> Legality {
    if !class_key.is_empty() && class_key != crate::equipment::class_key(class) {
        return Legality::ClassExclusive;
    }
    if let Some(f) = family {
        if !f.fits_slot(slot) {
            return Legality::SlotMismatch;
        }
        if !allows_family(class, f) {
            return Legality::ClassFamily;
        }
    }
    // A weight only means anything on armor: a stray descriptor on a weapon or an
    // accessory must never lock a class out of a slot that has no weight rule.
    if is_armor_slot(slot) {
        if let Some(w) = weight {
            if !allows_weight(class, w) {
                return Legality::ClassWeight;
            }
        }
    }
    Legality::Ok
}

/// The `[player.<key>]` / `gear.class_key` string for a class. Mirrors
/// `meld_run::class_key`, which cannot be depended on from the proto crate.
pub fn class_key(class: CharacterClass) -> &'static str {
    match class {
        CharacterClass::Explorer => "explorer",
        CharacterClass::Hunter => "hunter",
        CharacterClass::Dragoon => "dragoon",
        CharacterClass::Sage => "sage",
        CharacterClass::Ranger => "ranger",
        CharacterClass::AlchemistKnight => "alchemist_knight",
        CharacterClass::Bard => "bard",
        CharacterClass::Psyker => "psyker",
        CharacterClass::Resonant => "resonant",
        CharacterClass::Shifter => "shifter",
        CharacterClass::PhoenixGuard => "phoenix_guard",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_built_class_has_a_recognizable_hand() {
        // Explorer is the only class with a weapon *choice*: defensive or reach.
        assert!(allows_family(CharacterClass::Explorer, ItemFamily::Sword));
        assert!(allows_family(CharacterClass::Explorer, ItemFamily::Shield));
        assert!(allows_family(CharacterClass::Explorer, ItemFamily::Spear));
        // Casters and healers have both hands full.
        assert_eq!(weapon_families(CharacterClass::Resonant), &[ItemFamily::Staff]);
        assert_eq!(weapon_families(CharacterClass::Psyker), &[ItemFamily::Globe]);
        // Phoenix Guard cannot reach past its own arms.
        assert!(allows_family(CharacterClass::PhoenixGuard, ItemFamily::Gauntlet));
        assert!(!allows_family(CharacterClass::PhoenixGuard, ItemFamily::Spear));
        // The Shifter's off-hand is the build decision: dagger or parry blade.
        assert!(allows_family(CharacterClass::Shifter, ItemFamily::Dagger));
        assert!(allows_family(CharacterClass::Shifter, ItemFamily::ParryBlade));
        assert!(!allows_family(CharacterClass::Shifter, ItemFamily::Sword));
        // No class may hold nothing — an empty set would lock a hero out of gear.
        for c in [
            CharacterClass::Explorer,
            CharacterClass::Psyker,
            CharacterClass::Resonant,
            CharacterClass::Shifter,
            CharacterClass::PhoenixGuard,
            CharacterClass::Dragoon,
            CharacterClass::Bard,
        ] {
            assert!(!weapon_families(c).is_empty(), "{c:?} has no weapon");
            assert!(!armor_weights(c).is_empty(), "{c:?} has no armor");
        }
    }

    #[test]
    fn two_handed_weapons_reserve_the_off_hand() {
        for f in [ItemFamily::Spear, ItemFamily::Staff, ItemFamily::Globe] {
            assert_eq!(f.hands(), 2, "{f:?}");
            assert!(f.reserves_off_hand());
        }
        for f in [
            ItemFamily::Sword,
            ItemFamily::Shield,
            ItemFamily::Gauntlet,
            ItemFamily::Dagger,
            ItemFamily::ParryBlade,
        ] {
            assert_eq!(f.hands(), 1, "{f:?}");
            assert!(!f.reserves_off_hand());
        }
    }

    #[test]
    fn families_only_fit_their_own_hand() {
        assert!(ItemFamily::Shield.fits_slot("off_hand"));
        assert!(!ItemFamily::Shield.fits_slot("main_hand"));
        assert!(ItemFamily::ParryBlade.fits_slot("off_hand"));
        assert!(!ItemFamily::ParryBlade.fits_slot("main_hand"));
        // The dual-wield exception.
        assert!(ItemFamily::Dagger.fits_slot("main_hand"));
        assert!(ItemFamily::Dagger.fits_slot("off_hand"));
        assert!(!ItemFamily::Sword.fits_slot("off_hand"));
    }

    #[test]
    fn armor_weights_are_shared_where_they_overlap() {
        // Medium fits Explorer AND Phoenix Guard; light fits three classes. That
        // overlap is the whole point of weights over per-class armor.
        assert!(allows_weight(CharacterClass::Explorer, ArmorWeight::Medium));
        assert!(allows_weight(CharacterClass::PhoenixGuard, ArmorWeight::Medium));
        for c in [
            CharacterClass::Explorer,
            CharacterClass::Shifter,
            CharacterClass::Resonant,
        ] {
            assert!(allows_weight(c, ArmorWeight::Light), "{c:?}");
        }
        // …but plate on a Psyker never happens.
        assert!(!allows_weight(CharacterClass::Psyker, ArmorWeight::Heavy));
        assert!(!allows_weight(CharacterClass::Psyker, ArmorWeight::Medium));
    }

    #[test]
    fn two_handed_classes_have_no_off_hand_to_fill() {
        // A staff/globe class cannot fill an off-hand at all; loot must not roll
        // one for them (an unwearable drop is a dead drop).
        assert!(!has_off_hand(CharacterClass::Psyker));
        assert!(!has_off_hand(CharacterClass::Resonant));
        assert!(families_for_slot(CharacterClass::Psyker, "off_hand").is_empty());
        // Everyone else does.
        assert!(has_off_hand(CharacterClass::Explorer));
        assert!(has_off_hand(CharacterClass::PhoenixGuard));
        assert!(has_off_hand(CharacterClass::Shifter));
        assert_eq!(
            families_for_slot(CharacterClass::Shifter, "off_hand"),
            vec![ItemFamily::Dagger, ItemFamily::ParryBlade]
        );
        // The Explorer's main hand is a choice between reach and sword+shield.
        assert_eq!(
            families_for_slot(CharacterClass::Explorer, "main_hand"),
            vec![ItemFamily::Sword, ItemFamily::Spear]
        );
    }

    #[test]
    fn drops_roll_a_class_defining_weight() {
        assert_eq!(drop_weight(CharacterClass::PhoenixGuard), ArmorWeight::Heavy);
        assert_eq!(drop_weight(CharacterClass::Psyker), ArmorWeight::Robe);
        assert_eq!(drop_weight(CharacterClass::Shifter), ArmorWeight::Light);
        // And a class always allows what it drops.
        for c in [CharacterClass::PhoenixGuard, CharacterClass::Psyker, CharacterClass::Shifter] {
            assert!(allows_weight(c, drop_weight(c)), "{c:?}");
        }
    }

    #[test]
    fn a_stray_weight_outside_armor_never_blocks() {
        use CharacterClass::*;
        // An accessory (or weapon) that happens to carry a weight string is still
        // wearable — accessories are unrestricted by design.
        assert_eq!(
            check_equip(Explorer, "", "accessory", None, Some(ArmorWeight::Robe)),
            Legality::Ok
        );
        assert_eq!(
            check_equip(PhoenixGuard, "", "main_hand", Some(ItemFamily::Gauntlet), Some(ArmorWeight::Robe)),
            Legality::Ok
        );
        // On real armor it bites.
        assert_eq!(
            check_equip(Explorer, "", "legs", None, Some(ArmorWeight::Heavy)),
            Legality::ClassWeight
        );
    }

    #[test]
    fn check_equip_names_the_rule_that_failed() {
        use CharacterClass::*;
        assert_eq!(
            check_equip(Resonant, "", "main_hand", Some(ItemFamily::Spear), None),
            Legality::ClassFamily
        );
        assert_eq!(
            check_equip(Psyker, "", "chest", None, Some(ArmorWeight::Heavy)),
            Legality::ClassWeight
        );
        assert_eq!(
            check_equip(Explorer, "", "main_hand", Some(ItemFamily::Shield), None),
            Legality::SlotMismatch
        );
        // A signature piece named for another class.
        assert_eq!(
            check_equip(Explorer, "psyker", "head", None, Some(ArmorWeight::Robe)),
            Legality::ClassExclusive
        );
        // Its owner wears it, weight table or not.
        assert_eq!(
            check_equip(Psyker, "psyker", "head", None, Some(ArmorWeight::Robe)),
            Legality::Ok
        );
        // Legal kit passes.
        assert_eq!(
            check_equip(Shifter, "", "off_hand", Some(ItemFamily::ParryBlade), None),
            Legality::Ok
        );
        // An item carrying no descriptors is unrestricted on both axes.
        assert_eq!(check_equip(Resonant, "", "main_hand", None, None), Legality::Ok);
    }
}
