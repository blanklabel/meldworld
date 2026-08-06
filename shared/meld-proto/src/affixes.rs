//! Gear affixes — the loot chase (roadmap AD-1, docs/proposals/gear-identity.md §3).
//!
//! Past a tier floor a drop stops being a bigger number and starts being a
//! *quality*. Five affix classes, in ascending order of how much they change a
//! build:
//!
//! - **Stat** — `+N atk/def/spd`. The floor of the system; always available.
//! - **Element** — an elemental profile, riding the `damage_modifiers` plumbing.
//! - **Ward** — the hero *starts each battle* with Barrier / Regen / Evasion.
//!   These reuse states the ATB engine already models, so a ward affix is a real
//!   build lever rather than new machinery.
//! - **Keyword** — twists a class mechanic (banked Adrenaline, an extra Focus
//!   slot). Class-locked by construction: the twist only means something to the
//!   class whose mechanic it is.
//! - **Synergy** — conditional on an *ally* ("+N atk while a Resonant is in your
//!   party"). This is what turns one drop into a **party** build decision.
//!
//! The registry (which affixes exist, what they twist, what they're called) is
//! content and lives here; the numbers — how many affixes a rarity rolls, the
//! tier floors, the magnitude scale — are `[TUNABLE]`s in `balance.toml`
//! (working agreement #2).

use serde::{Deserialize, Serialize};

use crate::enums::CharacterClass;

/// Which of the five classes an affix belongs to. Drives the tier floor it is
/// gated behind and how the client groups it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AffixClass {
    Stat,
    Element,
    Ward,
    Keyword,
    Synergy,
}

/// One rolled affix on one piece of gear. `magnitude` is already resolved (the
/// roll happened server-side at generation); `element` and `ally_class` carry the
/// extra key an Element / Synergy affix needs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Affix {
    /// Registry key ([`AFFIXES`]).
    pub key: String,
    pub magnitude: i32,
    /// DamageType wire key, for an Element affix.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub element: Option<String>,
    /// The ally class this affix keys off, for a Synergy affix.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ally_class: Option<String>,
}

/// A registry entry: what an affix is, who can roll it, and how it reads.
#[derive(Debug, Clone, Copy)]
pub struct AffixDef {
    pub key: &'static str,
    pub class: AffixClass,
    /// Relative magnitude weight — multiplied by the balance-side magnitude scale
    /// so one affix can be inherently punchier than another without a tunable per
    /// affix.
    pub scale: f64,
    /// `Some` when only one class's heroes can benefit (Keyword affixes).
    pub only_class: Option<CharacterClass>,
    /// The name suffix this affix lends an item ("… of the Bulwark").
    pub suffix: &'static str,
}

const fn def(
    key: &'static str,
    class: AffixClass,
    scale: f64,
    only_class: Option<CharacterClass>,
    suffix: &'static str,
) -> AffixDef {
    AffixDef { key, class, scale, only_class, suffix }
}

/// Every affix in the game.
pub const AFFIXES: &[AffixDef] = &[
    def("atk", AffixClass::Stat, 1.0, None, "of Edges"),
    def("def", AffixClass::Stat, 1.0, None, "of Plating"),
    def("spd", AffixClass::Stat, 0.6, None, "of Quickness"),
    def("resist", AffixClass::Element, 1.0, None, "of Warding"),
    def("brand", AffixClass::Element, 1.0, None, "of the Kiln"),
    def("barrier", AffixClass::Ward, 2.0, None, "of the Bulwark"),
    def("regen", AffixClass::Ward, 0.5, None, "of Mending"),
    def("evasion", AffixClass::Ward, 0.5, None, "of the Ghost"),
    def(
        "adrenaline_primed",
        AffixClass::Keyword,
        1.5,
        Some(CharacterClass::Explorer),
        "of Fury",
    ),
    def(
        "focus_slot",
        AffixClass::Keyword,
        0.2,
        Some(CharacterClass::Psyker),
        "of the Open Mind",
    ),
    def("ally_atk", AffixClass::Synergy, 1.2, None, "of Fellowship"),
    def("ally_def", AffixClass::Synergy, 1.2, None, "of the Shield Wall"),
];

pub fn find(key: &str) -> Option<&'static AffixDef> {
    AFFIXES.iter().find(|d| d.key == key)
}

impl Affix {
    pub fn def(&self) -> Option<&'static AffixDef> {
        find(&self.key)
    }

    pub fn class(&self) -> Option<AffixClass> {
        self.def().map(|d| d.class)
    }

    /// Whether this affix does anything for a hero of `class`. A Keyword affix is
    /// inert on the wrong class — it still rolls (loot is not party-aware), which
    /// is what makes finding the *right* one feel like a find.
    pub fn applies_to(&self, class: CharacterClass) -> bool {
        match self.def().and_then(|d| d.only_class) {
            Some(only) => only == class,
            None => true,
        }
    }

    /// The line the player reads, e.g. `+12 Barrier at battle start` or
    /// `+3 atk while a Resonant is in your party`.
    pub fn describe(&self) -> String {
        let m = self.magnitude;
        match self.key.as_str() {
            "atk" => format!("+{m} atk"),
            "def" => format!("+{m} def"),
            "spd" => format!("+{m} speed"),
            "resist" => {
                let el = self.element.clone().unwrap_or_else(|| "all".into());
                format!("resists {}% {}", m.clamp(0, 100), el.to_lowercase())
            }
            "brand" => {
                let el = self.element.clone().unwrap_or_else(|| "none".into());
                format!("attacks deal {} damage", el.to_lowercase())
            }
            "barrier" => format!("+{m} Barrier at battle start"),
            "regen" => format!("+{m} Regen"),
            "evasion" => format!("+{m}% Evasion at battle start"),
            "adrenaline_primed" => format!("start battle with {m} Adrenaline"),
            "focus_slot" => format!("+{m} Focus slot"),
            "ally_atk" | "ally_def" => {
                let stat = if self.key == "ally_atk" { "atk" } else { "def" };
                match &self.ally_class {
                    Some(c) => format!("+{m} {stat} while a {} is in your party", pretty_class(c)),
                    None => format!("+{m} {stat} with an ally"),
                }
            }
            other => format!("{other} +{m}"),
        }
    }
}

/// Display form of a class key (`phoenix_guard` → `Phoenix Guard`).
pub fn pretty_class(key: &str) -> String {
    key.split('_')
        .map(|w| {
            let mut c = w.chars();
            match c.next() {
                Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// The name suffix a set of affixes lends an item: the strongest affix's suffix,
/// so a piece reads as the thing that makes it interesting rather than
/// accumulating a sentence of them.
pub fn name_suffix(affixes: &[Affix]) -> Option<&'static str> {
    affixes
        .iter()
        .filter_map(|a| a.def().map(|d| (d, a.magnitude)))
        .max_by(|(da, ma), (db, mb)| {
            (da.scale * *ma as f64)
                .partial_cmp(&(db.scale * *mb as f64))
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .map(|(d, _)| d.suffix)
}

/// Serialize affixes for the `gear.affixes` column / the wire. `[]` when empty.
pub fn to_json(affixes: &[Affix]) -> String {
    serde_json::to_string(affixes).unwrap_or_else(|_| "[]".into())
}

/// Parse the `gear.affixes` column. Unreadable or empty content is *no* affixes
/// rather than an error: a malformed row costs the player a bonus, never access
/// to the item.
pub fn from_json(raw: &str) -> Vec<Affix> {
    if raw.is_empty() || raw == "[]" {
        return Vec::new();
    }
    serde_json::from_str(raw).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn affix(key: &str, magnitude: i32) -> Affix {
        Affix { key: key.into(), magnitude, element: None, ally_class: None }
    }

    #[test]
    fn every_registry_entry_is_findable_and_describes_itself() {
        for d in AFFIXES {
            assert!(find(d.key).is_some(), "{} not findable", d.key);
            let line = affix(d.key, 7).describe();
            assert!(!line.is_empty());
            // A brand has no magnitude — it changes WHAT your attacks are, not how
            // much. Everything else must show its number so drops can be compared.
            if d.key != "brand" {
                assert!(line.contains('7'), "{}: {line}", d.key);
            }
            assert!(!d.suffix.is_empty());
        }
    }

    #[test]
    fn keyword_affixes_are_inert_on_the_wrong_class() {
        let fury = affix("adrenaline_primed", 4);
        assert!(fury.applies_to(CharacterClass::Explorer));
        assert!(!fury.applies_to(CharacterClass::Psyker));
        let mind = affix("focus_slot", 1);
        assert!(mind.applies_to(CharacterClass::Psyker));
        assert!(!mind.applies_to(CharacterClass::Resonant));
        for key in ["atk", "def", "spd", "barrier", "regen", "evasion", "ally_atk"] {
            assert!(affix(key, 1).applies_to(CharacterClass::Shifter), "{key}");
        }
    }

    #[test]
    fn a_brand_names_the_element_it_deals() {
        let a = Affix {
            key: "brand".into(),
            magnitude: 1,
            element: Some("FIRE".into()),
            ally_class: None,
        };
        assert_eq!(a.describe(), "attacks deal fire damage");
        assert_eq!(a.class(), Some(AffixClass::Element));
    }

    #[test]
    fn synergy_lines_name_the_ally_class() {
        let a = Affix {
            key: "ally_atk".into(),
            magnitude: 3,
            element: None,
            ally_class: Some("phoenix_guard".into()),
        };
        assert_eq!(a.describe(), "+3 atk while a Phoenix Guard is in your party");
        assert_eq!(pretty_class("phoenix_guard"), "Phoenix Guard");
    }

    #[test]
    fn the_name_suffix_comes_from_the_strongest_affix() {
        let picked = name_suffix(&[affix("atk", 5), affix("barrier", 4)]);
        assert_eq!(picked, Some("of the Bulwark"));
        let picked = name_suffix(&[affix("atk", 20), affix("barrier", 4)]);
        assert_eq!(picked, Some("of Edges"));
        assert_eq!(name_suffix(&[]), None);
    }

    #[test]
    fn json_round_trips_and_bad_content_costs_only_the_bonus() {
        let rolled = vec![
            affix("barrier", 12),
            Affix {
                key: "resist".into(),
                magnitude: 25,
                element: Some("FIRE".into()),
                ally_class: None,
            },
        ];
        let json = to_json(&rolled);
        assert_eq!(from_json(&json), rolled);
        assert!(from_json("").is_empty());
        assert!(from_json("[]").is_empty());
        assert!(from_json("{not json").is_empty());
    }
}
