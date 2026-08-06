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
    /// The rank within the hero's ORGANISATION that this ability comes with (see
    /// `docs/lore/factions.md`). Levelling is promotion, not just bigger numbers —
    /// a Phoenix Guard at level 9 is a Luminary, and the ability she just learned is
    /// what a Luminary is trusted with.
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
        unlock: 2,
        description: "Patch up an ally — or yourself. What a Guide carries instead of a spell.",
        rank: "Traveler",
    },
    SkillDef {
        key: "read_the_ground",
        name: "Read the Ground",
        class: "explorer",
        unlock: 5,
        description: "You know this terrain and it does not: damage plus a steal from the foe's ATB gauge.",
        rank: "Scout",
    },
    SkillDef {
        key: "set_anchor",
        name: "Set Anchor",
        class: "explorer",
        unlock: 9,
        description: "Fix a point of stability in the chaos: Barrier for the WHOLE party.",
        rank: "Pioneer",
    },
    SkillDef {
        key: "safe_passage",
        name: "Safe Passage",
        class: "explorer",
        unlock: 13,
        description: "Nobody walks it alone: Regen for the whole party, turn after turn.",
        rank: "Discoverer",
    },
    SkillDef {
        key: "a_world_known",
        name: "A World Known",
        class: "explorer",
        unlock: 17,
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
        unlock: 2,
        description: "Heal yourself. Spends banked Adrenaline — no potion needed.",
        rank: "Stalker",
    },
    SkillDef {
        key: "snare",
        name: "Snare",
        class: "hunter",
        unlock: 2,
        description: "Damage a foe and drain its ATB gauge, stealing the turn it was about to take. A capture starts here. Primes it for a Shifter's Backstab.",
        rank: "Stalker",
    },
    SkillDef {
        key: "frenzy",
        name: "Frenzy",
        class: "hunter",
        unlock: 5,
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
        unlock: 1,
        description: "A Focus that plates an ally in Barrier — temporary HP that absorbs damage before their own.",
        rank: "Initiate",
    },
    SkillDef {
        key: "mind_spike",
        name: "Mind Spike",
        class: "psyker",
        unlock: 3,
        description: "A stronger damage Focus. Costs a Focus slot, like every manifestation.",
        rank: "Tracer",
    },
    SkillDef {
        key: "temporal_anchor",
        name: "Temporal Anchor",
        class: "psyker",
        unlock: 5,
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
        unlock: 2,
        description: "Grant an ally Regen: HP back at the start of each of their turns.",
        rank: "",
    },
    SkillDef {
        key: "ward",
        name: "Ward",
        class: "resonant",
        unlock: 3,
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
        unlock: 2,
        description: "Blink out of the way: self Evasion, which decays each turn.",
        rank: "Shift Rat",
    },
    SkillDef {
        key: "ransack",
        name: "Ransack",
        class: "shifter",
        unlock: 3,
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
        unlock: 2,
        description: "Set your feet and speak the rite: a Barrier sized off your own max HP. Nobody gets turned behind you.",
        rank: "Purifier",
    },
    SkillDef {
        key: "holy_censure",
        name: "Holy Censure",
        class: "phoenix_guard",
        unlock: 5,
        description: "Advanced anti-undead discipline: a heavy condemnation that zeroes the foe's gauge outright. It loses its turn, not part of it.",
        rank: "Exemplar",
    },
    SkillDef {
        key: "purging_light",
        name: "Purging Light",
        class: "phoenix_guard",
        unlock: 9,
        description: "Light on EVERY enemy at once — the answer to a pack, and to a rite. Undead burn worst.",
        rank: "Luminary",
    },
    SkillDef {
        key: "unbroken_vigil",
        name: "Unbroken Vigil",
        class: "phoenix_guard",
        unlock: 13,
        description: "Barrier for the WHOLE party. No one is left behind to be turned.",
        rank: "Redeemer",
    },
    SkillDef {
        key: "eradication",
        name: "Eradication",
        class: "phoenix_guard",
        unlock: 17,
        description: "All strikes must be completed to the point of eradication: the more hurt the foe, the harder this lands. Undead do not get back up.",
        rank: "Apotheosis",
    },
];

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
    fn the_ladder_reaches_the_orders_senior_ranks() {
        // Every order gates its late ranks on character level 5/9/13/17
        // (docs/lore/factions.md). A class whose kit stops at level 3 stops
        // mattering at level 3, which is the whole reason for the deep ranks.
        let pg = skills_for_class("phoenix_guard");
        let deepest = pg.iter().map(|s| s.unlock).max().unwrap();
        assert_eq!(deepest, 17, "the Phoenix Guard's Apotheosis rank is level 17");
        for want in [1, 2, 5, 9, 13, 17] {
            assert!(
                pg.iter().any(|s| s.unlock == want),
                "no Phoenix Guard ability at rank level {want}: {:?}",
                pg.iter().map(|s| (s.rank, s.unlock)).collect::<Vec<_>>()
            );
        }
        // And its ranks are named in ladder order, so a promotion reads as one.
        let names: Vec<&str> = pg.iter().map(|s| s.rank).collect();
        assert_eq!(
            names,
            vec!["Initiate", "Purifier", "Exemplar", "Luminary", "Redeemer", "Apotheosis"]
        );
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
