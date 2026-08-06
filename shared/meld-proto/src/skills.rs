//! The ability registry: every hero skill the engine resolves, what it does in the
//! player's words, and the level it unlocks at.
//!
//! Shared by the server (which rejects a locked skill, authoritatively) and the
//! client (which greys the menu row and shows the tooltip), so the two can never
//! disagree about what an ability is called, what it does, or when you get it.
//!
//! **Descriptions live here, next to the unlock level.** A description kept in the
//! client is a description that drifts from the code that resolves it; keeping both
//! in one row means the battle menu, the abilities view and the server's gate all
//! read the same line.
//!
//! **Unlock levels are square numbers** — 1, 4, 9, 16, 25, 36, … out to about 100.
//! The XP curve costs `L + 1` fights per level, so cumulative effort grows with the
//! square of the level; spacing unlocks on squares therefore makes each new ability
//! cost a *step up* in commitment rather than an ever-flatter trickle, while still
//! putting several in reach of a new player's first hours.
//!
//! **Ranks** are the other half of the ladder ([`docs/proposals/progression-and-unlocks.md`]).
//! An ability a hero already owns gets stronger at intervals all the way to the
//! level cap, so level 200 still means something when the last *new* button arrived
//! long before. The rank levels and the per-rank gain are `[progression]`
//! `[TUNABLE]`s derived from the ability's unlock level, so the whole ladder retunes
//! from balance rather than from a hand-authored table per ability — see
//! `meld_run::ability_rank`.

/// One ability.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SkillDef {
    /// The C2S `skill_kind` (or Psyker manifestation kind) the engine resolves.
    pub key: &'static str,
    pub name: &'static str,
    /// Owning class, as a `class_key`.
    pub class: &'static str,
    /// The level a hero must reach to use it.
    pub unlock: i32,
    /// What it does, for the battle menu tooltip and the abilities view. Written for
    /// the player: the effect, and the cost or catch.
    pub description: &'static str,
    /// The rank the order associates with this ability, kept as flavour. The rank a
    /// hero actually *holds* comes from their level via [`rank_title`] and rides next
    /// to the class name — abilities now run well past the top rank, so this is a
    /// note about the ability's station, not the hero's.
    pub rank: &'static str,
}

/// Every hero ability in the game.
pub const SKILLS: &[SkillDef] = &[
    // ---- Explorer: the mapping order (docs/lore/factions.md). Tempo and stability,
    // not burst — it keeps the party moving and standing. Ranks are the Explorers'
    // ladder: Walker, Traveler, Scout, Pioneer, Discoverer, Globemaster.
    SkillDef {
        key: "trailblaze",
        name: "Trailblaze",
        class: "explorer",
        unlock: 1,
        description: "Cut a line through the fight: a solid strike that costs nothing to make.",
        rank: "Walker",
    },
    SkillDef {
        key: "field_dressing",
        name: "Field Dressing",
        class: "explorer",
        unlock: 4,
        description: "Patch up an ally — or yourself. What a Guide carries instead of a spell.",
        rank: "Traveler",
    },
    SkillDef {
        key: "read_the_ground",
        name: "Read the Ground",
        class: "explorer",
        unlock: 9,
        description: "You know this terrain and it does not: damage plus a steal from the foe's ATB gauge.",
        rank: "Scout",
    },
    SkillDef {
        key: "set_anchor",
        name: "Set Anchor",
        class: "explorer",
        unlock: 16,
        description: "Fix a point of stability in the chaos: Barrier for the WHOLE party.",
        rank: "Pioneer",
    },
    SkillDef {
        key: "safe_passage",
        name: "Safe Passage",
        class: "explorer",
        unlock: 25,
        description: "Nobody walks it alone: Regen for the whole party, turn after turn.",
        rank: "Discoverer",
    },
    SkillDef {
        key: "a_world_known",
        name: "A World Known",
        class: "explorer",
        unlock: 36,
        description: "The order's whole vision, for one moment: every ally's ATB gauge surges. The party goes first.",
        rank: "Globemaster",
    },
    // ---- Hunter: the martial baseline. Attacks bank Adrenaline, every skill
    // spends it — "adrenaline junkies", in the guild's own words. Ranks are the
    // Hunters' armband ladder (docs/lore/factions.md).
    SkillDef {
        key: "power_strike",
        name: "Power Strike",
        class: "hunter",
        unlock: 1,
        description: "A heavy blow to one foe. Spends banked Adrenaline.",
        rank: "Wisker",
    },
    SkillDef {
        key: "second_wind",
        name: "Second Wind",
        class: "hunter",
        unlock: 4,
        description: "Heal yourself. Spends banked Adrenaline — no potion needed.",
        rank: "Stalker",
    },
    SkillDef {
        key: "snare",
        name: "Snare",
        class: "hunter",
        unlock: 9,
        description: "Damage a foe and drain its ATB gauge, stealing the turn it was about to take. A capture starts here. Primes it for a Shifter's Backstab.",
        rank: "Stalker",
    },
    SkillDef {
        key: "frenzy",
        name: "Frenzy",
        class: "hunter",
        unlock: 16,
        description: "Your biggest hit, for your biggest Adrenaline cost. It's a victory when the hunt is over.",
        rank: "Shikari",
    },
    // ---- Psyker: Foci persist and fire every turn ----
    SkillDef {
        key: "gravity_well",
        name: "Gravity Well",
        class: "psyker",
        unlock: 1,
        description: "A Focus that crushes one foe every turn, ignoring armour. Holds it in place for a Phoenix Guard's Kinetic Shock.",
        rank: "Initiate",
    },
    SkillDef {
        key: "kinetic_aegis",
        name: "Kinetic Aegis",
        class: "psyker",
        unlock: 4,
        description: "A Focus that plates an ally in Barrier — temporary HP that absorbs damage before their own.",
        rank: "Initiate",
    },
    SkillDef {
        key: "mind_spike",
        name: "Mind Spike",
        class: "psyker",
        unlock: 9,
        description: "A stronger damage Focus. Costs a Focus slot, like every manifestation.",
        rank: "Tracer",
    },
    SkillDef {
        key: "temporal_anchor",
        name: "Temporal Anchor",
        class: "psyker",
        unlock: 16,
        description: "A Focus that drains a foe's ATB gauge every turn. It acts, and acts, and never gets there.",
        rank: "Field Marshal",
    },
    // ---- Resonant: pays in its own blood ----
    SkillDef {
        key: "transfuse",
        name: "Transfuse",
        class: "resonant",
        unlock: 1,
        description: "Heal an ally with your own HP. The only heal that costs you something.",
        rank: "",
    },
    SkillDef {
        key: "regen_boon",
        name: "Regen Boon",
        class: "resonant",
        unlock: 4,
        description: "Grant an ally Regen: HP back at the start of each of their turns.",
        rank: "",
    },
    SkillDef {
        key: "ward",
        name: "Ward",
        class: "resonant",
        unlock: 9,
        description: "Grant an ally Barrier. Cheaper than healing the damage afterwards.",
        rank: "",
    },
    // ---- Shifter: fast, fragile, evasive ----
    SkillDef {
        key: "backstab",
        name: "Backstab",
        class: "shifter",
        unlock: 1,
        description: "A strike that pierces most armour. Devastating on a Snared foe.",
        rank: "Flicker Foot",
    },
    SkillDef {
        key: "flicker",
        name: "Flicker",
        class: "shifter",
        unlock: 4,
        description: "Blink out of the way: self Evasion, which decays each turn.",
        rank: "Shift Rat",
    },
    SkillDef {
        key: "ransack",
        name: "Ransack",
        class: "shifter",
        unlock: 9,
        description: "Damage a foe and rob it of its tempo (ATB gauge). Sets up a Power Strike.",
        rank: "Shift Rat",
    },
    // ---- Phoenix Guard: the Last City's anti-undead order (docs/lore/factions.md).
    // The ladder IS their rank ladder — Initiate 1, Purifier 2, Exemplar 5,
    // Luminary 9, Redeemer 13, Apotheosis 17 — so every promotion is a new tool.
    SkillDef {
        key: "silvered_strike",
        name: "Silvered Strike",
        class: "phoenix_guard",
        unlock: 1,
        description: "Standard-issue silvered steel, swung to stagger. Bites far deeper into undead. Primes a foe for a Frenzy.",
        rank: "Initiate",
    },
    SkillDef {
        key: "rite_of_rest",
        name: "Rite of Rest",
        class: "phoenix_guard",
        unlock: 4,
        description: "Set your feet and speak the rite: a Barrier sized off your own max HP. Nobody gets turned behind you.",
        rank: "Purifier",
    },
    SkillDef {
        key: "holy_censure",
        name: "Holy Censure",
        class: "phoenix_guard",
        unlock: 9,
        description: "Advanced anti-undead discipline: a heavy condemnation that zeroes the foe's gauge outright. It loses its turn, not part of it.",
        rank: "Exemplar",
    },
    SkillDef {
        key: "purging_light",
        name: "Purging Light",
        class: "phoenix_guard",
        unlock: 16,
        description: "Light on EVERY enemy at once — the answer to a pack, and to a rite. Undead burn worst.",
        rank: "Luminary",
    },
    SkillDef {
        key: "unbroken_vigil",
        name: "Unbroken Vigil",
        class: "phoenix_guard",
        unlock: 25,
        description: "Barrier for the WHOLE party. No one is left behind to be turned.",
        rank: "Redeemer",
    },
    SkillDef {
        key: "eradication",
        name: "Eradication",
        class: "phoenix_guard",
        unlock: 36,
        description: "All strikes must be completed to the point of eradication: the more hurt the foe, the harder this lands. Undead do not get back up.",
        rank: "Apotheosis",
    },
];

/// Each order's six-rank ladder, and the character level each rank is gated on.
///
/// The lore (docs/lore/factions.md) comes from a **D&D campaign capped at level
/// 20**, where the senior ranks sit at 5/9/13/17. MELDWORLD caps at 255, so those
/// are scaled by the same ratio (≈ ×12.75) and rounded to legible numbers —
/// **1 / 25 / 65 / 115 / 165 / 215**. Unscaled, every rank would be earned in the
/// first afternoon and the remaining 238 levels would carry no standing at all.
///
/// Rank 1 stays at level 1: you hold it the moment the order accepts you.
///
/// A rank is **standing, not power** — it gates nothing, and rides next to the
/// class name because it is fun.
pub const RANK_LADDERS: &[(&str, &[(&str, i32)])] = &[
    (
        "hunter",
        &[
            ("Wisker", 1),
            ("Stalker", 25),
            ("Shikari", 65),
            ("Predator", 115),
            ("Master Hunter", 165),
            ("Apex", 215),
        ],
    ),
    (
        "explorer",
        &[
            ("Walker", 1),
            ("Traveler", 25),
            ("Scout", 65),
            ("Pioneer", 115),
            ("Discoverer", 165),
            ("Globemaster", 215),
        ],
    ),
    (
        "phoenix_guard",
        &[
            ("Initiate", 1),
            ("Purifier", 25),
            ("Exemplar", 65),
            ("Luminary", 115),
            ("Redeemer", 165),
            ("Apotheosis", 215),
        ],
    ),
    (
        "shifter",
        &[
            ("Flicker Foot", 1),
            ("Shift Rat", 25),
            ("Runner", 65),
            ("Shifter", 115),
            ("Void-Dancer", 165),
            ("The Named", 215),
        ],
    ),
    (
        "psyker",
        &[
            ("Initiate", 1),
            ("Tracer", 25),
            ("Field Marshal", 65),
            ("Lead Investigator", 115),
            ("Bureau Chief", 165),
            ("Director", 215),
        ],
    ),
];

/// The org rank a hero of `class` holds at `level` — the highest rank whose level
/// they have reached. `None` for a class with no order yet (the Resonant).
pub fn rank_title(class: &str, level: i32) -> Option<&'static str> {
    RANK_LADDERS
        .iter()
        .find(|(c, _)| *c == class)?
        .1
        .iter()
        .filter(|(_, at)| level >= *at)
        .next_back()
        .map(|(title, _)| *title)
}

pub fn skill(key: &str) -> Option<&'static SkillDef> {
    SKILLS.iter().find(|s| s.key == key)
}

/// Whether `skill` is a hero skill the engine knows how to resolve.
pub fn is_hero_skill(skill: &str) -> bool {
    self::skill(skill).is_some()
}

/// The class whose kit `skill` belongs to, as a `class_key`.
pub fn skill_owner(skill: &str) -> Option<&'static str> {
    self::skill(skill).map(|s| s.class)
}

/// A class's kit, in registry (level) order.
pub fn skills_for_class(class: &str) -> Vec<&'static SkillDef> {
    SKILLS.iter().filter(|s| s.class == class).collect()
}

/// The level at which `skill` unlocks. Returns 1 for always-available actions
/// (Attack/Defend/Item) and for anything unknown, so a caller that asks about a
/// non-skill gets "usable" rather than "locked forever".
pub fn unlock_level(skill: &str) -> i32 {
    self::skill(skill).map(|s| s.unlock).unwrap_or(1)
}

/// What `skill` does, for a tooltip. Empty for actions that need no explanation.
pub fn describe(skill: &str) -> &'static str {
    self::skill(skill).map(|s| s.description).unwrap_or("")
}

/// The player-facing name of a skill key (`swell_strike` → `Swell Strike`). Falls
/// back to title-casing the key, so an action outside the registry (Attack, Defend)
/// still reads properly.
pub fn pretty_skill(key: &str) -> String {
    if let Some(s) = self::skill(key) {
        return s.name.to_string();
    }
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

/// Whether a hero at `level` may use `skill`.
pub fn is_unlocked(skill: &str, level: i32) -> bool {
    level >= unlock_level(skill)
}

/// A rank as a Roman numeral suffix (`2` → `"II"`); empty at rank 1, since an
/// ability at its base rank is just its name.
pub fn rank_suffix(rank: i32) -> &'static str {
    match rank {
        ..=1 => "",
        2 => "II",
        3 => "III",
        4 => "IV",
        5 => "V",
        6 => "VI",
        7 => "VII",
        8 => "VIII",
        9 => "IX",
        _ => "X",
    }
}

/// The ability's name at a rank: `Power Strike III`.
pub fn name_at_rank(key: &str, rank: i32) -> String {
    let name = pretty_skill(key);
    match rank_suffix(rank) {
        "" => name,
        s => format!("{name} {s}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_ability_is_owned_named_and_explained() {
        let mut seen = std::collections::HashSet::new();
        for s in SKILLS {
            assert!(seen.insert(s.key), "duplicate ability {}", s.key);
            assert!(!s.name.is_empty(), "{} has no name", s.key);
            // The tooltip is the whole point of the registry carrying text: an
            // ability the player cannot read is an ability they will never press.
            assert!(
                s.description.len() > 20,
                "{}'s description does not say what it does",
                s.key
            );
            assert!(s.unlock >= 1, "{} unlocks below level 1", s.key);
            // A rank is the org title the ability arrives with. Empty is allowed
            // only for the Resonant, the one class with no order yet
            // (docs/lore/factions.md) — anyone else with a blank rank is an
            // ability nobody was promoted into.
            if s.class != "resonant" {
                assert!(!s.rank.is_empty(), "{} arrives with no rank", s.key);
            }
            assert!(
                crate::equipment::class_from_key(s.class).is_some(),
                "{} belongs to unknown class {}",
                s.key,
                s.class
            );
            assert_eq!(unlock_level(s.key), s.unlock);
            assert_eq!(describe(s.key), s.description);
            assert_eq!(pretty_skill(s.key), s.name);
        }
        // Every class has a kit, and every kit starts with something usable at 1.
        for class in ["hunter", "psyker", "resonant", "shifter", "phoenix_guard"] {
            let kit = skills_for_class(class);
            assert!(kit.len() >= 3, "{class} has a thin kit: {}", kit.len());
            assert!(
                kit.iter().any(|s| s.unlock == 1),
                "{class} can do nothing at level 1"
            );
        }
    }

    #[test]
    fn a_non_skill_is_usable_rather_than_a_mystery() {
        assert!(!is_hero_skill("attack"));
        assert!(!is_hero_skill("nonsense"));
        // Attack/Defend/Item are not registry entries, and must not read as locked.
        assert_eq!(unlock_level("attack"), 1);
        assert!(is_unlocked("attack", 1));
        assert_eq!(describe("attack"), "");
        assert_eq!(pretty_skill("purging_light"), "Purging Light");
        // Toll of the Deep is the Iron Hull's Grandmaster perk, reserved for that
        // order — it must NOT resolve as a Phoenix Guard ability.
        assert!(!is_hero_skill("toll_of_the_deep"));
        assert_eq!(skill_owner("backstab"), Some("shifter"));
        assert_eq!(skill_owner("nope"), None);
    }

    #[test]
    fn abilities_are_spaced_out_to_about_a_hundred_not_bunched_under_ten() {
        // The point of the ladder is that levelling keeps paying. A kit whose last
        // ability lands at level 5 stops mattering at level 5.
        for class in ["hunter", "explorer", "psyker", "resonant", "shifter", "phoenix_guard"] {
            let kit = skills_for_class(class);
            let mut levels: Vec<i32> = kit.iter().map(|s| s.unlock).collect();
            levels.sort();
            assert_eq!(levels[0], 1, "{class} can do nothing at level 1");
            // Every unlock is a square number, so each new ability costs a step up in
            // commitment on the `L + 1` fights-per-level curve.
            for lv in &levels {
                let r = (*lv as f64).sqrt().round() as i32;
                assert_eq!(r * r, *lv, "{class}: level {lv} is not a square");
            }
            assert!(*levels.last().unwrap() <= 100, "{class} reaches past 100");
        }
        // The two biggest kits reach deep; nothing is bunched at the bottom.
        assert!(
            skills_for_class("phoenix_guard").iter().map(|s| s.unlock).max().unwrap() >= 36,
            "the Phoenix Guard's ladder is too shallow"
        );
    }

    #[test]
    fn a_rank_is_standing_scaled_to_our_level_cap_not_dnds() {
        // The lore's ranks are D&D levels (cap 20); ours cap at 255, so they are
        // scaled. Unscaled, every rank would be held by the first afternoon.
        assert_eq!(rank_title("phoenix_guard", 1), Some("Initiate"));
        assert_eq!(rank_title("phoenix_guard", 24), Some("Initiate"));
        assert_eq!(rank_title("phoenix_guard", 25), Some("Purifier"));
        assert_eq!(rank_title("phoenix_guard", 214), Some("Redeemer"));
        assert_eq!(rank_title("phoenix_guard", 255), Some("Apotheosis"));
        assert_eq!(rank_title("hunter", 65), Some("Shikari"));
        assert_eq!(rank_title("explorer", 115), Some("Pioneer"));
        // The Resonant has no order, so it holds no rank — and that must not panic.
        assert_eq!(rank_title("resonant", 200), None);
        assert_eq!(rank_title("nonsense", 9), None);
        // Every ladder is six ranks, in ascending level order.
        for (class, ladder) in RANK_LADDERS {
            assert_eq!(ladder.len(), 6, "{class} has {} ranks", ladder.len());
            assert_eq!(ladder[0].1, 1, "{class}'s first rank is not held on joining");
            for w in ladder.windows(2) {
                assert!(w[0].1 < w[1].1, "{class}'s ladder is out of order");
            }
            assert!(ladder[5].1 <= 255, "{class}'s top rank is unreachable");
        }
    }

    #[test]
    fn a_rank_reads_as_a_numeral_and_rank_one_is_just_the_name() {
        assert_eq!(name_at_rank("power_strike", 1), "Power Strike");
        assert_eq!(name_at_rank("power_strike", 3), "Power Strike III");
        assert_eq!(name_at_rank("power_strike", 12), "Power Strike X");
        // A rank below 1 is a bug upstream, not a crash here.
        assert_eq!(name_at_rank("power_strike", 0), "Power Strike");
    }
}
