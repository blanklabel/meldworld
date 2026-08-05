//! Uniques and sets — the two chase tiers above affixes (roadmap AD-1,
//! docs/proposals/gear-identity.md §3).
//!
//! An affix makes a drop *interesting*. These two make it **build-defining**:
//!
//! - A **unique** is one named item with fixed affixes and a **drawback**. It is
//!   never a strict upgrade, so equipping one is a decision rather than an
//!   inventory chore — that is the whole design. Uniques drop only from a reward
//!   spike (elites, Gatekeepers, dungeon bosses), because a chase item you can
//!   farm from trash is not a chase.
//! - A **set** spans several pieces and pays the whole **party** once enough of
//!   it is worn. Sets are the only bonus in the game that reaches other players'
//!   heroes, which is what makes assembling one a group project.
//!
//! Both are content and live here; their scalars are `[affix]` `[TUNABLE]`s.

use crate::affixes::Affix;
use crate::enums::CharacterClass;

/// A stat a unique's drawback can bite into. Deliberately small: a drawback has
/// to be legible at a glance ("−15 def") or a player cannot weigh the trade.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Drawback {
    Atk(i32),
    Def(i32),
    Spd(i32),
    MaxHp(i32),
}

impl Drawback {
    pub fn describe(self) -> String {
        match self {
            Drawback::Atk(n) => format!("-{n} atk"),
            Drawback::Def(n) => format!("-{n} def"),
            Drawback::Spd(n) => format!("-{n} speed"),
            Drawback::MaxHp(n) => format!("-{n} max HP"),
        }
    }
}

/// One named unique item.
#[derive(Debug, Clone, Copy)]
pub struct UniqueDef {
    pub key: &'static str,
    pub name: &'static str,
    /// The slot category it occupies.
    pub slot: &'static str,
    /// `Some` when only one class can use it at all.
    pub only_class: Option<CharacterClass>,
    /// Fixed affixes: `(affix key, magnitude)`. Fixed, not rolled — a unique is a
    /// known quantity you go looking for.
    pub affixes: &'static [(&'static str, i32)],
    pub drawback: Drawback,
    /// The one line of flavour the tooltip shows.
    pub flavour: &'static str,
}

pub const UNIQUES: &[UniqueDef] = &[
    UniqueDef {
        key: "reaver_edge",
        name: "Reaver's Edge",
        slot: "main_hand",
        only_class: None,
        affixes: &[("atk", 22)],
        drawback: Drawback::Def(12),
        flavour: "It parries nothing. It was never meant to.",
    },
    UniqueDef {
        key: "deadweight_aegis",
        name: "Deadweight Aegis",
        slot: "off_hand",
        only_class: None,
        affixes: &[("barrier", 30), ("def", 10)],
        drawback: Drawback::Spd(6),
        flavour: "Nothing gets through. Nothing gets past, either.",
    },
    UniqueDef {
        key: "furys_yoke",
        name: "Fury's Yoke",
        slot: "chest",
        only_class: Some(CharacterClass::Explorer),
        affixes: &[("adrenaline_primed", 12), ("atk", 8)],
        drawback: Drawback::MaxHp(25),
        flavour: "The rage arrives before the fight does.",
    },
    UniqueDef {
        key: "hollow_crown",
        name: "Hollow Crown",
        slot: "head",
        only_class: Some(CharacterClass::Psyker),
        affixes: &[("focus_slot", 2)],
        drawback: Drawback::MaxHp(30),
        flavour: "Room for two more thoughts. Less room for you.",
    },
    UniqueDef {
        key: "gutterstep",
        name: "Gutterstep",
        slot: "legs",
        only_class: Some(CharacterClass::Shifter),
        affixes: &[("evasion", 18), ("spd", 6)],
        drawback: Drawback::Def(10),
        flavour: "Being elsewhere is its own armour.",
    },
];

pub fn unique(key: &str) -> Option<&'static UniqueDef> {
    UNIQUES.iter().find(|u| u.key == key)
}

impl UniqueDef {
    /// The unique's fixed affixes, as rolled affixes.
    pub fn rolled(&self) -> Vec<Affix> {
        self.affixes
            .iter()
            .map(|(key, m)| Affix {
                key: (*key).to_string(),
                magnitude: *m,
                element: None,
                ally_class: None,
            })
            .collect()
    }
}

/// A gear set: several pieces, a threshold, and a bonus that reaches the whole
/// party once the threshold is met.
#[derive(Debug, Clone, Copy)]
pub struct SetDef {
    pub key: &'static str,
    pub name: &'static str,
    /// How many distinct pieces one hero must wear for the set to fire.
    pub pieces_required: usize,
    /// Party-wide bonus once complete: `(atk, def, spd)` for **every** hero,
    /// including other players' heroes in a merged raid.
    pub party_atk: i32,
    pub party_def: i32,
    pub party_spd: i32,
    pub flavour: &'static str,
}

pub const SETS: &[SetDef] = &[
    SetDef {
        key: "wardens_march",
        name: "Warden's March",
        pieces_required: 3,
        party_atk: 0,
        party_def: 6,
        party_spd: 2,
        flavour: "Wardens moved in threes, and nothing moved through them.",
    },
    SetDef {
        key: "kiln_chorus",
        name: "Kiln Chorus",
        pieces_required: 2,
        party_atk: 5,
        party_def: 0,
        party_spd: 0,
        flavour: "Struck together, they ring the same note.",
    },
];

pub fn set(key: &str) -> Option<&'static SetDef> {
    SETS.iter().find(|s| s.key == key)
}

/// Which sets a hero's worn pieces complete: for each `(set key, worn count)`,
/// the sets whose threshold is met. Counting happens per hero; the *payout* is
/// party-wide, so battle assembly applies it to everyone.
pub fn completed_sets(worn: &[(String, usize)]) -> Vec<&'static SetDef> {
    worn.iter()
        .filter_map(|(key, count)| set(key).filter(|d| *count >= d.pieces_required))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_unique_has_a_real_cost() {
        for u in UNIQUES {
            assert!(!u.affixes.is_empty(), "{} grants nothing", u.key);
            let cost = match u.drawback {
                Drawback::Atk(n) | Drawback::Def(n) | Drawback::Spd(n) | Drawback::MaxHp(n) => n,
            };
            assert!(cost > 0, "{} has no drawback", u.key);
            assert!(!u.drawback.describe().is_empty());
            assert!(!u.flavour.is_empty(), "{} has no flavour", u.key);
            for (key, m) in u.affixes {
                assert!(crate::affixes::find(key).is_some(), "{}: unknown affix {key}", u.key);
                assert!(*m > 0);
            }
            assert!(unique(u.key).is_some());
        }
    }

    #[test]
    fn a_class_locked_unique_only_carries_its_own_class_keywords() {
        let yoke = unique("furys_yoke").unwrap();
        assert_eq!(yoke.only_class, Some(CharacterClass::Explorer));
        for a in yoke.rolled() {
            assert!(a.applies_to(CharacterClass::Explorer));
        }
        let crown = unique("hollow_crown").unwrap();
        assert_eq!(crown.only_class, Some(CharacterClass::Psyker));
        for a in crown.rolled() {
            assert!(a.applies_to(CharacterClass::Psyker));
        }
    }

    #[test]
    fn sets_fire_only_at_their_threshold() {
        let two = vec![("kiln_chorus".to_string(), 2)];
        let one = vec![("kiln_chorus".to_string(), 1)];
        assert_eq!(completed_sets(&two).len(), 1);
        assert!(completed_sets(&one).is_empty());
        let partial = vec![("wardens_march".to_string(), 2)];
        assert!(completed_sets(&partial).is_empty());
        let full = vec![("wardens_march".to_string(), 3)];
        assert_eq!(completed_sets(&full)[0].name, "Warden's March");
        assert!(completed_sets(&[("nonsense".to_string(), 9)]).is_empty());
    }

    #[test]
    fn every_set_actually_pays_the_party_something() {
        for s in SETS {
            assert!(s.pieces_required >= 2, "{} is not a set", s.key);
            assert!(
                s.party_atk + s.party_def + s.party_spd > 0,
                "{} pays nothing",
                s.key
            );
            assert!(!s.flavour.is_empty());
            assert!(set(s.key).is_some());
        }
    }
}
