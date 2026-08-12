//! Account-permanent unlocks: party slots and classes a player EARNS
//! (roadmap `CL-1`, [`docs/proposals/progression-and-unlocks.md`]).
//!
//! A new account starts with **one party slot and the Explorer**. Everything else
//! is earned by playing, and once earned is never lost — a bad dive can't cost you
//! a class.
//!
//! This module is the single source of truth both sides read: the server decides
//! what a milestone grants, the client greys the rows it doesn't own yet and names
//! the trigger. Keeping the trigger text *here* rather than in the client is what
//! stops the UI promising a condition the server doesn't actually check.
//!
//! Triggers are deliberately things a player was going to do anyway, so an unlock
//! reads as recognition rather than a chore.

use serde::{Deserialize, Serialize};

use crate::enums::CharacterClass;

/// What an unlock gives you.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind", content = "value")]
pub enum UnlockKind {
    /// The Nth party slot becomes usable (slot 1 needs no unlock).
    PartySlot(i32),
    /// A class becomes selectable in the party builder.
    Class(CharacterClass),
}

/// The condition that grants an unlock. Evaluated server-side against a
/// [`Milestone`] the game loop reports; nothing here is client-checkable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Trigger {
    /// Owned from the first login.
    Start,
    /// `heroes` heroes are **simultaneously** at `level` or above during a dive.
    /// Simultaneous rather than ever-reached, so the bar is a party achievement —
    /// and one that gets easier as gear improves, which is the intended pull
    /// toward the loot chase.
    HeroesAtLevel { heroes: i32, level: i32 },
    /// An elite creature is felled.
    EliteFelled,
    /// A dungeon is entered for the first time.
    DungeonEntered,
    /// The party survives an undead rite (an undead boss and its minions).
    SurvivedUndeadRite,
    /// A dive is extracted successfully — you came back, and you brought the proof.
    Extracted,
    /// A full party wipe with at least `heroes` heroes in the party. The
    /// consolation prize for the worst night.
    PartyWipe { heroes: i32 },
    /// A piece of gear is FORGED — the first time you made something instead of
    /// finding it. The Foundry notices people who make things.
    GearForged,
    /// A resource node is worked dry. The Open Flower notices people who tend.
    NodeExhausted,
}

/// One earnable thing.
#[derive(Debug, Clone, Copy)]
pub struct UnlockDef {
    /// Stable key; what persists and what rides the wire.
    pub key: &'static str,
    pub name: &'static str,
    pub kind: UnlockKind,
    pub trigger: Trigger,
    /// Shown on the locked row in the party builder: how to earn it.
    pub trigger_text: &'static str,
    /// The line under the banner when it lands: what it means for you.
    pub banner: &'static str,
    /// Another unlock that must already be owned. A class you can't field is not
    /// a reward, so every class past the first waits on the slot that seats it.
    pub requires: Option<&'static str>,
}

/// Every unlock in the game, in the order a player is expected to earn them.
pub const UNLOCKS: &[UnlockDef] = &[
    UnlockDef {
        key: "class_explorer",
        name: "Explorer",
        kind: UnlockKind::Class(CharacterClass::Explorer),
        trigger: Trigger::Start,
        trigger_text: "Yours from the first dive.",
        banner: "The mapping order takes anyone, because nobody gets through the Meld alone. \
                 An Explorer sets the pace instead of the damage: its opener MARKS a creature \
                 so every ally hits it harder, and its later calls Barrier the party, make it \
                 hard to hit, and hurry everyone's turn along.",
        requires: None,
    },
    UnlockDef {
        key: "party_slot_2",
        name: "Second party slot",
        kind: UnlockKind::PartySlot(2),
        trigger: Trigger::HeroesAtLevel { heroes: 1, level: 10 },
        trigger_text: "Bring any hero to level 10.",
        banner: "You have carried enough alone. Someone else can carry the rest.",
        requires: None,
    },
    UnlockDef {
        key: "class_hunter",
        name: "Hunter",
        kind: UnlockKind::Class(CharacterClass::Hunter),
        trigger: Trigger::Extracted,
        trigger_text: "Extract from a dive — the hall pays on evidence, not stories.",
        banner: "The Den has a board, a pit, and a marker with your callsign on it. A Hunter \
                 fights on ADRENALINE: every basic attack banks it and every skill spends it, \
                 so the longer a fight runs the harder you hit. Its intel also names what you \
                 are walking into before you commit to it.",
        requires: Some("party_slot_2"),
    },
    UnlockDef {
        key: "class_resonant",
        name: "Resonant",
        kind: UnlockKind::Class(CharacterClass::Resonant),
        // The wipe IS the lesson, so it pays out on the first one — which for a new
        // account is a solo death. Gating this behind the second slot would push the
        // healer past the exact fight that explains why you want a healer.
        trigger: Trigger::PartyWipe { heroes: 1 },
        trigger_text: "Lose a run to a wipe. You'll learn what you were missing.",
        banner: "That fight wanted a healer. A Resonant keeps a party standing: it heals an \
                 ally from its own HP, grants Regen so wounds close every turn, and Wards \
                 damage before it lands. Wounds carry between fights — this is how you stop \
                 starting the next one already hurt.",
        requires: None,
    },
    UnlockDef {
        key: "class_shifter",
        name: "Shifter",
        kind: UnlockKind::Class(CharacterClass::Shifter),
        trigger: Trigger::DungeonEntered,
        trigger_text: "Enter a dungeon.",
        banner: "A Runner found the door before you did. A Shifter is fast and fragile — the \
                 only class that dodges by default. It robs creatures mid-fight, and it senses \
                 the dungeon doors and the traps you would have walked into.",
        requires: Some("party_slot_2"),
    },
    UnlockDef {
        key: "party_slot_3",
        name: "Third party slot",
        kind: UnlockKind::PartySlot(3),
        trigger: Trigger::HeroesAtLevel { heroes: 2, level: 20 },
        trigger_text: "Have two heroes at level 20 at the same time.",
        banner: "Two veterans make a formation. A third makes it hold.",
        requires: Some("party_slot_2"),
    },
    UnlockDef {
        key: "class_phoenix_guard",
        name: "Phoenix Guard",
        kind: UnlockKind::Class(CharacterClass::PhoenixGuard),
        trigger: Trigger::SurvivedUndeadRite,
        trigger_text: "Survive an undead rite — a risen boss and its dead.",
        banner: "The Order walks out of fires nothing else survives, and one of them saw you \
                 do it. A Phoenix Guard is the wall: the most HP and armour in the game, it \
                 soaks what was aimed at everyone else and burns undead ordinary steel barely \
                 scratches.",
        requires: Some("party_slot_3"),
    },
    UnlockDef {
        key: "party_slot_4",
        name: "Fourth party slot",
        kind: UnlockKind::PartySlot(4),
        trigger: Trigger::HeroesAtLevel { heroes: 3, level: 30 },
        trigger_text: "Have three heroes at level 30 at the same time.",
        banner: "A full march. Nothing out there is built for four of you.",
        requires: Some("party_slot_3"),
    },
    UnlockDef {
        key: "class_smithwright",
        name: "Smithwright",
        kind: UnlockKind::Class(CharacterClass::Smithwright),
        trigger: Trigger::GearForged,
        trigger_text: "Forge a piece of gear at the anvil, rather than finding one.",
        banner: "You made something. The Foundry only ever recruits people who have. A \
                 Smithwright fights with the trade's own tools — a hammer that staggers, a \
                 bulwark planted in front of the line — and raises the field forge, so the \
                 party stops walking home to mend.",
        requires: Some("party_slot_3"),
    },
    UnlockDef {
        key: "class_keeper",
        name: "Keeper",
        kind: UnlockKind::Class(CharacterClass::Keeper),
        trigger: Trigger::NodeExhausted,
        trigger_text: "Work a resource node dry — take everything it had.",
        banner: "You took the whole vein and left nothing rotting. The Open Flower has been \
                 waiting for someone patient. A Keeper mends between fights instead of \
                 during them, and its still turns a patch of ground into somewhere the party \
                 can actually rest.",
        requires: Some("party_slot_3"),
    },
    UnlockDef {
        key: "class_psyker",
        name: "Psyker",
        kind: UnlockKind::Class(CharacterClass::Psyker),
        trigger: Trigger::PartyWipe { heroes: 3 },
        trigger_text: "Lose a full party of three or more. Someone will explain why.",
        banner: "Nothing you swung mattered. The Psyker does not swing — it seats \
                 Manifestations that fire on their own every turn: armour-ignoring damage, \
                 Barriers for the party, and a drag on the enemy's ATB gauge.",
        requires: Some("party_slot_4"),
    },
];

pub fn unlock(key: &str) -> Option<&'static UnlockDef> {
    UNLOCKS.iter().find(|u| u.key == key)
}

/// What a brand-new account owns: one slot, one class.
pub fn starting_unlocks() -> Vec<&'static str> {
    UNLOCKS
        .iter()
        .filter(|u| u.trigger == Trigger::Start)
        .map(|u| u.key)
        .collect()
}

/// How many party slots a set of owned unlocks is worth. Always at least 1 — slot
/// 1 needs no unlock, so a player with nothing still has a party.
pub fn party_slots(owned: &[String]) -> i32 {
    UNLOCKS
        .iter()
        .filter(|u| owned.iter().any(|o| o == u.key))
        .filter_map(|u| match u.kind {
            UnlockKind::PartySlot(n) => Some(n),
            _ => None,
        })
        .max()
        .unwrap_or(1)
        .max(1)
}

/// Which classes a set of owned unlocks can field. Always includes the Explorer:
/// an account with no unlocks at all still has to be able to build a party.
pub fn owned_classes(owned: &[String]) -> Vec<CharacterClass> {
    let mut out = vec![CharacterClass::Explorer];
    for u in UNLOCKS {
        if let UnlockKind::Class(c) = u.kind {
            if c != CharacterClass::Explorer && owned.iter().any(|o| o == u.key) {
                out.push(c);
            }
        }
    }
    out
}

/// The unlock that grants a class, if the class is earnable.
pub fn unlock_for_class(class: CharacterClass) -> Option<&'static UnlockDef> {
    UNLOCKS.iter().find(|u| u.kind == UnlockKind::Class(class))
}

/// The wire view of an unlock, for the banner and the locked party-builder row.
pub fn view(def: &UnlockDef) -> crate::realtime::run::UnlockView {
    let (kind, class_key, slot) = match def.kind {
        UnlockKind::PartySlot(n) => ("party_slot", None, Some(n)),
        UnlockKind::Class(c) => (
            "class",
            Some(crate::equipment::class_key(c).to_string()),
            None,
        ),
    };
    crate::realtime::run::UnlockView {
        key: def.key.to_string(),
        name: def.name.to_string(),
        kind: kind.to_string(),
        class_key,
        slot,
        trigger_text: def.trigger_text.to_string(),
        banner: def.banner.to_string(),
    }
}

/// A thing that just happened in a dive, which an unlock might be waiting on.
/// The game loop reports these; [`granted_by`] turns one into unlocks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Milestone {
    /// The most heroes simultaneously at-or-above a level. Carries the level and
    /// the count so one milestone can satisfy every `HeroesAtLevel` bar at once.
    HeroesReached { heroes: i32, level: i32 },
    EliteFelled,
    DungeonEntered,
    SurvivedUndeadRite,
    Extracted,
    PartyWiped { heroes: i32 },
    GearForged,
    NodeExhausted,
}

/// Which unlocks a milestone grants, given what's already owned. Returns only
/// *new* unlocks whose prerequisite is met — so a class whose seat doesn't exist
/// yet stays locked, and re-reporting a milestone grants nothing twice.
pub fn granted_by(milestone: Milestone, owned: &[String]) -> Vec<&'static UnlockDef> {
    UNLOCKS
        .iter()
        .filter(|u| !owned.iter().any(|o| o == u.key))
        .filter(|u| u.requires.is_none_or(|r| owned.iter().any(|o| o == r)))
        .filter(|u| match (u.trigger, milestone) {
            (
                Trigger::HeroesAtLevel { heroes, level },
                Milestone::HeroesReached { heroes: got, level: at },
            ) => got >= heroes && at >= level,
            (Trigger::EliteFelled, Milestone::EliteFelled) => true,
            (Trigger::DungeonEntered, Milestone::DungeonEntered) => true,
            (Trigger::SurvivedUndeadRite, Milestone::SurvivedUndeadRite) => true,
            (Trigger::Extracted, Milestone::Extracted) => true,
            (Trigger::PartyWipe { heroes }, Milestone::PartyWiped { heroes: got }) => got >= heroes,
            (Trigger::GearForged, Milestone::GearForged) => true,
            (Trigger::NodeExhausted, Milestone::NodeExhausted) => true,
            _ => false,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys(v: Vec<&'static UnlockDef>) -> Vec<&'static str> {
        v.into_iter().map(|u| u.key).collect()
    }

    #[test]
    fn a_fresh_account_has_one_slot_and_one_class() {
        let owned: Vec<String> = starting_unlocks().iter().map(|s| s.to_string()).collect();
        assert_eq!(owned, vec!["class_explorer"]);
        assert_eq!(party_slots(&owned), 1);
        assert_eq!(owned_classes(&owned), vec![CharacterClass::Explorer]);
        // Even a corrupt/empty unlock set must leave a playable account.
        assert_eq!(party_slots(&[]), 1);
        assert_eq!(owned_classes(&[]), vec![CharacterClass::Explorer]);
    }

    #[test]
    fn every_unlock_is_reachable_and_says_how() {
        let mut seen = std::collections::HashSet::new();
        for u in UNLOCKS {
            assert!(seen.insert(u.key), "duplicate unlock {}", u.key);
            assert!(!u.trigger_text.is_empty() && !u.banner.is_empty(), "{}", u.key);
            assert_eq!(unlock(u.key).map(|d| d.key), Some(u.key));
            // A prerequisite must be a real unlock, and must come EARLIER in the
            // list — a cycle or a forward reference is a permanently locked class.
            if let Some(req) = u.requires {
                let ri = UNLOCKS.iter().position(|o| o.key == req);
                let ui = UNLOCKS.iter().position(|o| o.key == u.key);
                assert!(ri.is_some(), "{} requires unknown {}", u.key, req);
                assert!(ri < ui, "{} requires {} which comes later", u.key, req);
            }
        }
        // Four slots, five classes, and the Explorer is the only free one.
        let slots: Vec<i32> = UNLOCKS
            .iter()
            .filter_map(|u| match u.kind {
                UnlockKind::PartySlot(n) => Some(n),
                _ => None,
            })
            .collect();
        assert_eq!(slots, vec![2, 3, 4], "slot 1 needs no unlock");
        assert_eq!(
            UNLOCKS.iter().filter(|u| matches!(u.kind, UnlockKind::Class(_))).count(),
            8
        );
    }

    #[test]
    fn a_class_waits_for_a_seat_to_sit_in() {
        // A dungeon found on the very first dive: no second slot, no Shifter. A class
        // you cannot field is not a reward.
        let owned = vec!["class_explorer".to_string()];
        assert!(keys(granted_by(Milestone::DungeonEntered, &owned)).is_empty());

        // Earn the slot, walk into another dungeon, and now it lands.
        let owned = vec!["class_explorer".to_string(), "party_slot_2".to_string()];
        assert_eq!(keys(granted_by(Milestone::DungeonEntered, &owned)), vec!["class_shifter"]);
        // And coming home with the proof is what the Hunters' hall recruits on.
        assert_eq!(keys(granted_by(Milestone::Extracted, &owned)), vec!["class_hunter"]);
    }

    #[test]
    fn the_first_wipe_hands_over_the_healer_even_solo() {
        // The Resonant is the ONE class with no seat requirement, because the fight
        // that teaches you why you want a healer is the one a lone hero loses — and
        // that happens long before the second slot opens at level 10.
        let owned = vec!["class_explorer".to_string()];
        assert_eq!(
            keys(granted_by(Milestone::PartyWiped { heroes: 1 }, &owned)),
            vec!["class_resonant"]
        );
        // Once earned it is never re-granted, so a second bad night is quiet.
        let owned = vec!["class_explorer".to_string(), "class_resonant".to_string()];
        assert!(keys(granted_by(Milestone::PartyWiped { heroes: 1 }, &owned)).is_empty());
    }

    #[test]
    fn a_level_milestone_grants_only_the_bars_it_clears() {
        let mut owned = vec!["class_explorer".to_string()];
        // One hero at 9 clears nothing; at 10 it opens the second slot.
        assert!(keys(granted_by(Milestone::HeroesReached { heroes: 1, level: 9 }, &owned)).is_empty());
        assert_eq!(
            keys(granted_by(Milestone::HeroesReached { heroes: 1, level: 10 }, &owned)),
            vec!["party_slot_2"]
        );
        owned.push("party_slot_2".to_string());

        // ONE hero at 20 is not TWO heroes at 20.
        assert!(keys(granted_by(Milestone::HeroesReached { heroes: 1, level: 25 }, &owned)).is_empty());
        assert_eq!(
            keys(granted_by(Milestone::HeroesReached { heroes: 2, level: 20 }, &owned)),
            vec!["party_slot_3"]
        );
        owned.push("party_slot_3".to_string());
        assert_eq!(
            keys(granted_by(Milestone::HeroesReached { heroes: 3, level: 30 }, &owned)),
            vec!["party_slot_4"]
        );

        // Re-reporting a cleared bar grants nothing: unlocks are permanent, not
        // repeatable.
        owned.push("party_slot_4".to_string());
        assert!(keys(granted_by(Milestone::HeroesReached { heroes: 3, level: 99 }, &owned)).is_empty());
    }

    #[test]
    fn the_hard_earned_classes_need_their_own_night() {
        let owned = vec![
            "class_explorer".to_string(),
            "party_slot_2".to_string(),
            "party_slot_3".to_string(),
        ];
        assert_eq!(
            keys(granted_by(Milestone::SurvivedUndeadRite, &owned)),
            vec!["class_phoenix_guard"]
        );
        assert_eq!(keys(granted_by(Milestone::DungeonEntered, &owned)), vec!["class_shifter"]);
        // The Psyker needs the fourth slot AND a three-hero wipe — a solo death buys
        // the healer, not the mind. `class_resonant` is pre-owned here so the wipes
        // below isolate the Psyker's own bar.
        let owned = [owned, vec!["class_resonant".to_string()]].concat();
        assert!(keys(granted_by(Milestone::PartyWiped { heroes: 3 }, &owned)).is_empty());
        let owned = [owned, vec!["party_slot_4".to_string()]].concat();
        assert!(keys(granted_by(Milestone::PartyWiped { heroes: 2 }, &owned)).is_empty());
        assert_eq!(keys(granted_by(Milestone::PartyWiped { heroes: 3 }, &owned)), vec!["class_psyker"]);
    }

    #[test]
    fn owning_everything_fields_everything() {
        let all: Vec<String> = UNLOCKS.iter().map(|u| u.key.to_string()).collect();
        assert_eq!(party_slots(&all), 4);
        let classes = owned_classes(&all);
        assert_eq!(classes.len(), 8, "{classes:?}");
        for c in [
            CharacterClass::Explorer,
            CharacterClass::Hunter,
            CharacterClass::Resonant,
            CharacterClass::Shifter,
            CharacterClass::PhoenixGuard,
            CharacterClass::Psyker,
            CharacterClass::Smithwright,
            CharacterClass::Keeper,
        ] {
            assert!(classes.contains(&c), "{c:?} not fieldable with everything owned");
            assert!(unlock_for_class(c).is_some(), "{c:?} has no unlock");
        }
        // A class nobody can earn yet has no unlock row, rather than an empty one.
        assert!(unlock_for_class(CharacterClass::Bard).is_none());
    }
}
