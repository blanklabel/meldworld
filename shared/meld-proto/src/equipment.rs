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

use crate::enums::{CharacterClass, DamageType};

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
    /// Two-handed bow. RANGED: its shot ignores the target's rank.
    Bow,
    /// One-handed sling. Ranged, and light enough to leave a hand free.
    Sling,
    /// A spear built to be thrown rather than held. Ranged, one-handed.
    ThrownSpear,
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
            ItemFamily::Spear | ItemFamily::Staff | ItemFamily::Globe | ItemFamily::Bow => 2,
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
            ItemFamily::Bow => "bow",
            ItemFamily::Sling => "sling",
            ItemFamily::ThrownSpear => "thrown_spear",
        }
    }

    /// **Does this weapon reach past a front rank?**
    ///
    /// A back rank halves an incoming PHYSICAL blow, and until now nothing physical could
    /// answer it — the rear was a caster's problem and a swordsman's wall. A ranged weapon
    /// is the martial answer: it shoots over the front line and lands on the rear at full
    /// force.
    ///
    /// Reach is one of TWO independent axes. This one is "can it get there"; how MANY it
    /// hits is sweep, and bundling them would make every ranged weapon a crowd-clearer and
    /// every crowd-clearer ranged.
    ///
    /// `Spear` is deliberately NOT reaching, despite being the two-handed reach weapon:
    /// giving it reach here would silently buff every Explorer holding one, which is a
    /// balance change wearing a refactor's clothes. If a spear should reach, that is its own
    /// decision.
    pub fn reaches_past_the_front(self) -> bool {
        matches!(self, ItemFamily::Bow | ItemFamily::Sling | ItemFamily::ThrownSpear)
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
            "bow" => ItemFamily::Bow,
            "sling" => ItemFamily::Sling,
            "thrown_spear" => ItemFamily::ThrownSpear,
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
        // A Smithwright fights with the tool of the trade: a hammer in one hand, a
        // shield-sized bulwark in the other.
        CharacterClass::Smithwright => &[ItemFamily::Gauntlet, ItemFamily::Shield],
        // A Keeper's staff is a walking stick, a pestle and a splint — two-handed, so
        // the order that carries the medicine carries nothing else.
        CharacterClass::Keeper => &[ItemFamily::Staff],
        CharacterClass::Shifter => &[ItemFamily::Dagger, ItemFamily::ParryBlade],
        // The disposal-of-dangerous-creatures guild shoots things it would rather not be
        // standing next to. It is also the only class that had no hand of its own — it fell
        // through to the martial default — so the ranged families land here without taking
        // an option away from anybody.
        CharacterClass::Hunter => &[
            ItemFamily::Bow,
            ItemFamily::Sling,
            ItemFamily::ThrownSpear,
            ItemFamily::Sword,
        ],
        // The unbuilt roster classes inherit the martial baseline until each gets
        // its own kit — never an empty set, which would lock a hero out of gear.
        _ => &[ItemFamily::Sword, ItemFamily::Shield, ItemFamily::Spear],
    }
}

/// The armor weights a class may wear.
pub fn armor_weights(class: CharacterClass) -> &'static [ArmorWeight] {
    match class {
        CharacterClass::PhoenixGuard => &[ArmorWeight::Heavy, ArmorWeight::Medium],
        CharacterClass::Smithwright => &[ArmorWeight::Heavy, ArmorWeight::Medium],
        CharacterClass::Keeper => &[ArmorWeight::Light, ArmorWeight::Robe],
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

/// How a piece of armour of this weight answers ONE damage type: a signed number of
/// STEPS, negative resisting and positive taking it worse. The magnitude of a step is a
/// `[TUNABLE]` (`[armor_resist]`), because a coefficient is balance's and `meld-proto` is
/// shared with a client that has no `balance.toml`.
pub type ResistSteps = i8;

/// What a weight of armour is good and bad against — the trade-off that makes weight a
/// CHOICE rather than a wearability gate.
///
/// A plate cuirass turns an edge and does nothing about a war hammer; mail spreads a cut
/// across its rings and lets a spike through one; a padded jerkin soaks impact and opens
/// to a blade. A robe is not armour at all — it answers with wards, so it takes every
/// physical blow worse and shrugs off what is thrown rather than swung. Steel also
/// conducts, which is why the heavy suit fears lightning.
///
/// Every weight has at least one resistance AND at least one weakness. That is the rule
/// (`every_armor_weight_is_a_trade`), not a property of these particular numbers: a weight
/// that is only ever better is a weight everybody wears.
pub fn weight_profile(weight: ArmorWeight) -> &'static [(DamageType, ResistSteps)] {
    use DamageType::*;
    match weight {
        // Plate: an edge skids, a point is worse, a hammer is what it fears. And it is a
        // steel shell wrapped around a person in a lightning storm.
        //
        // These step COUNTS and `[armor_resist] step` are coupled: the step is sized by the
        // early game (a level-1 caster has no margin), and the counts are sized so a hammer
        // is still meaningfully better than a sword against plate at that step
        // (`what_you_wear_decides_what_hurts_you` asserts the ratio). Change one and check
        // the other.
        ArmorWeight::Heavy => &[(Slash, -2), (Pierce, -1), (Blunt, 2), (Lightning, 1)],
        // Mail: rings defeat a cut and a spike goes between them.
        ArmorWeight::Medium => &[(Slash, -1), (Pierce, 2), (Blunt, 1)],
        // Padded leather: made for impact, not for edges. Light enough to keep moving,
        // which is why the elements find less of you.
        ArmorWeight::Light => &[(Blunt, -2), (Slash, 1), (Pierce, 1), (Fire, -1)],
        // Cloth and wards: no answer at all to being hit, and the best answer to being
        // burned, frozen or read.
        // Worse against every physical type than any real armour is against its own worst
        // one — cloth is not a trade between blade and hammer, it is the absence of both.
        ArmorWeight::Robe => &[
            (Slash, 3),
            (Blunt, 3),
            (Pierce, 3),
            (Fire, -1),
            (Ice, -1),
            (Lightning, -1),
            (Mind, -2),
        ],
    }
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
        "smithwright" => CharacterClass::Smithwright,
        "keeper" => CharacterClass::Keeper,
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
        CharacterClass::Smithwright => "smithwright",
        CharacterClass::Keeper => "keeper",
    }
}

/// Every equipment slot, in loadout order. One list, so "equip best" and the client's
/// category columns cannot disagree about what a hero wears.
pub const SLOTS: [&str; 6] = ["main_hand", "off_hand", "head", "chest", "legs", "accessory"];

/// Score a piece for a class, given that class's `[atk, def, spd]` weights (`[equip_best]`).
///
/// Gear carries only those three bonuses, so they are the whole axis. A flat sum would be
/// wrong for most of the roster — a Psyker's damage rides Mnd, so `atk` on its staff is
/// nearly dead weight, while a Phoenix Guard would rather have the armour than anything.
/// Tier breaks ties, so between two pieces that score the same the deeper one wins.
pub fn gear_score(atk: i32, def: i32, spd: i32, tier: i32, w: [f64; 3]) -> f64 {
    atk as f64 * w[0] + def as f64 * w[1] + spd as f64 * w[2] + tier as f64 * 0.001
}

/// Whether `class` may wear this piece at all: right slot, allowed family, allowed weight,
/// and not restricted to somebody else's class. The same four rules `set_equipped` enforces,
/// so a picker can never propose a piece the equip call would refuse.
pub fn can_wear(
    class: CharacterClass,
    slot: &str,
    class_key: &str,
    family: &str,
    armor_weight: &str,
) -> bool {
    if !class_key.is_empty() && class_from_key(class_key) != Some(class) {
        return false;
    }
    if let Some(f) = ItemFamily::from_wire(family) {
        if !f.fits_slot(slot) || !allows_family(class, f) {
            return false;
        }
    }
    if let Some(w) = ArmorWeight::from_wire(armor_weight) {
        if !allows_weight(class, w) {
            return false;
        }
    }
    true
}

#[cfg(test)]
mod tests {
    /// A weight that is only ever better is a weight everybody wears. Every one of them has
    /// to give something up, and none may be so lopsided that its answer is "wear this".
    #[test]
    fn every_armor_weight_is_a_trade() {
        for w in [ArmorWeight::Heavy, ArmorWeight::Medium, ArmorWeight::Light, ArmorWeight::Robe] {
            let p = weight_profile(w);
            assert!(!p.is_empty(), "{w:?} answers for nothing");
            assert!(
                p.iter().any(|(_, s)| *s < 0),
                "{w:?} resists nothing, so nobody would wear it"
            );
            assert!(
                p.iter().any(|(_, s)| *s > 0),
                "{w:?} is weak to nothing, so everybody would wear it"
            );
            // One damage type, one answer — a duplicate entry silently doubles a step.
            let mut seen: Vec<DamageType> = p.iter().map(|(t, _)| *t).collect();
            let before = seen.len();
            seen.sort_by_key(|t| t.to_wire());
            seen.dedup();
            assert_eq!(before, seen.len(), "{w:?} names a damage type twice");
            for (ty, steps) in p {
                assert_ne!(*ty, DamageType::None, "{w:?} tries to resist TRUE damage");
                assert!((-3..=3).contains(steps), "{w:?} uses {steps} steps on {ty:?}");
            }
        }
    }

    /// The physical trio is where weight is SUPPOSED to matter, so every weight has to have
    /// an opinion about all three — a weight silent on blunt is a weight a hammer ignores.
    #[test]
    fn every_weight_answers_for_every_physical_type() {
        for w in [ArmorWeight::Heavy, ArmorWeight::Medium, ArmorWeight::Light, ArmorWeight::Robe] {
            for ty in [DamageType::Slash, DamageType::Blunt, DamageType::Pierce] {
                assert!(
                    weight_profile(w).iter().any(|(t, _)| *t == ty),
                    "{w:?} has nothing to say about {ty:?}"
                );
            }
        }
    }

    /// The rock-paper-scissors the fiction promises: a hammer beats plate, a spear beats
    /// mail, an edge beats leather. Pinned as a RELATION, so a retune has to keep the shape.
    #[test]
    fn the_physical_triangle_holds() {
        let at = |w: ArmorWeight, ty: DamageType| {
            weight_profile(w).iter().find(|(t, _)| *t == ty).map(|(_, s)| *s).unwrap_or(0)
        };
        assert!(at(ArmorWeight::Heavy, DamageType::Blunt) > 0, "plate should fear a hammer");
        assert!(at(ArmorWeight::Heavy, DamageType::Slash) < 0, "plate should turn an edge");
        assert!(at(ArmorWeight::Medium, DamageType::Pierce) > 0, "mail should fear a point");
        assert!(at(ArmorWeight::Light, DamageType::Slash) > 0, "leather should fear an edge");
        assert!(at(ArmorWeight::Light, DamageType::Blunt) < 0, "padding should soak impact");
        // And the robe's bargain: worst at being hit, best at being burned or read.
        for ty in [DamageType::Slash, DamageType::Blunt, DamageType::Pierce] {
            assert!(at(ArmorWeight::Robe, ty) > 0, "a robe should fear {ty:?}");
        }
        assert!(at(ArmorWeight::Robe, DamageType::Mind) < 0, "a robe is wards, and wards read");
        // Cloth must be worse against EVERY physical type than any armour is against the one
        // it likes least — otherwise a robe is a plate cuirass with better elemental rolls,
        // which is what the first draft of these numbers accidentally was.
        let worst_armor = [ArmorWeight::Heavy, ArmorWeight::Medium, ArmorWeight::Light]
            .into_iter()
            .flat_map(|w| [DamageType::Slash, DamageType::Blunt, DamageType::Pierce].map(move |t| at(w, t)))
            .max()
            .unwrap();
        for ty in [DamageType::Slash, DamageType::Blunt, DamageType::Pierce] {
            assert!(
                at(ArmorWeight::Robe, ty) > worst_armor,
                "a robe takes {ty:?} no worse than real armour's weakest point"
            );
        }
    }

    /// Every class can actually reach a weight, or `weight_profile` is decoration for it.
    #[test]
    fn every_class_can_wear_something_with_a_profile() {
        // Off the SKILL registry rather than a list here, so a new class cannot be added
        // and quietly left out of the rule.
        for key in crate::skills::all_classes() {
            let c = class_from_key(&key).unwrap_or_else(|| panic!("{key} is not a class key"));
            let ws = armor_weights(c);
            assert!(!ws.is_empty(), "{c:?} can wear no armour at all");
            assert!(ws.iter().all(|w| !weight_profile(*w).is_empty()));
        }
    }


    /// "Best" has to mean best FOR THAT CLASS. Gear carries only atk/def/spd, so a flat sum
    /// would hand a Psyker the warhammer and a Phoenix Guard the dagger.
    #[test]
    fn a_class_wants_the_gear_its_own_kit_uses() {
        // A heavy hitter vs a light, quick piece.
        let hammer = (9, 1, 0);
        let charm = (0, 2, 8);

        let psyker = [0.3, 0.9, 1.0];
        let hunter = [1.3, 0.7, 0.6];
        let score = |g: (i32, i32, i32), w: [f64; 3]| gear_score(g.0, g.1, g.2, 1, w);

        assert!(
            score(charm, psyker) > score(hammer, psyker),
            "a Psyker's damage rides Mnd - raw attack is nearly dead weight to it"
        );
        assert!(
            score(hammer, hunter) > score(charm, hunter),
            "a Hunter's damage IS the point"
        );
        // Tier only breaks ties, it does not outrank the stats.
        assert!(
            gear_score(9, 1, 0, 1, hunter) > gear_score(0, 0, 0, 40, hunter),
            "a tier-40 blank must not beat a tier-1 piece that actually helps"
        );
    }

    /// The picker must never propose a piece the equip call would refuse, so it asks the
    /// same four questions `set_equipped` does.
    #[test]
    fn can_wear_agrees_with_the_equip_rules() {
        use CharacterClass::*;
        // A Phoenix Guard takes gauntlets and heavy armour, not a spear or a robe.
        assert!(can_wear(PhoenixGuard, "main_hand", "", "gauntlet", ""));
        assert!(!can_wear(PhoenixGuard, "main_hand", "", "spear", ""));
        assert!(can_wear(PhoenixGuard, "chest", "", "", "heavy"));
        assert!(!can_wear(PhoenixGuard, "chest", "", "", "robe"));
        // Class-locked gear stays with its class.
        assert!(!can_wear(PhoenixGuard, "main_hand", "shifter", "", ""));
        assert!(can_wear(Shifter, "main_hand", "shifter", "", ""));
        // And a family in the wrong slot is refused whoever asks.
        assert!(!can_wear(Shifter, "head", "", "sword", ""));
    }

    /// Every slot a hero can wear needs a name "equip best" and the client's columns agree on.
    #[test]
    fn the_slot_list_covers_every_wearable_family() {
        assert_eq!(SLOTS.len(), 6);
        for s in SLOTS {
            assert!(!s.is_empty());
        }
        assert!(SLOTS.contains(&"main_hand") && SLOTS.contains(&"off_hand"));
    }

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
