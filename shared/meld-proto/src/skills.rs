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

/// How long a class's ability ladder runs — the Dragon Quest lesson that not every
/// class should keep learning forever.
///
/// A **martial** class gets a short, front-loaded kit and then scales on *gear and
/// stats*: its power curve is the weapon in its hand, so handing it a new button at
/// level 80 would be inventing a caster. A **caster** has almost no gear scaling by
/// comparison, so its ladder is the progression and runs the whole way. A **hybrid**
/// sits between the two.
///
/// This is what stops "more abilities" from meaning "every class gets ten": it says
/// *which* classes should, and why.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Archetype {
    /// Short kit, done early. Scales on gear. (Hunter, Shifter.)
    Martial,
    /// Medium kit. Some gear scaling, some utility. (Explorer, Phoenix Guard.)
    Hybrid,
    /// Long kit, arriving all the way out. (Psyker, Resonant.)
    Caster,
}

/// The deepest level a class's ladder should reach, by archetype. A martial class's
/// last ability lands while the numbers still matter; a caster's arrives at the cap
/// of the authored range.
pub fn archetype(class: &str) -> Archetype {
    match class {
        "hunter" | "shifter" => Archetype::Martial,
        "psyker" | "resonant" => Archetype::Caster,
        _ => Archetype::Hybrid,
    }
}

/// The level band an archetype's last ability is expected to fall in.
pub fn ladder_ceiling(a: Archetype) -> i32 {
    match a {
        Archetype::Martial => 25,
        Archetype::Hybrid => 49,
        Archetype::Caster => 100,
    }
}

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
    /// The ability this one REPLACES when it unlocks (`mug` upgrades `steal`). This
    /// is how a martial class progresses without its menu growing: the row improves
    /// in place instead of a fifth button appearing. Only the best owned version of a
    /// chain is ever offered — see [`skills_for_class_at`].
    pub upgrades: Option<&'static str>,
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
        description: "Damage, and MARKS the target: every ally hits it harder until the mark fades. Costs nothing.",
        upgrades: None,
        rank: "Walker",
    },
    SkillDef {
        key: "field_dressing",
        name: "Field Dressing",
        class: "explorer",
        unlock: 4,
        description: "Heals one ally (the most wounded if you pick nobody) for a share of their max HP. Costs nothing.",
        upgrades: None,
        rank: "Traveler",
    },
    SkillDef {
        key: "misdirection",
        name: "Misdirection",
        class: "explorer",
        unlock: 9,
        description: "Damage, drains the target's ATB gauge, and DISTRACTS it: it swings wide at whoever it attacks, and the party's chance to flee goes up while it holds.",
        upgrades: None,
        rank: "Scout",
    },
    SkillDef {
        key: "stable_ground",
        name: "Stable Ground",
        class: "explorer",
        unlock: 16,
        description: "Barrier for the WHOLE party, sized off each ally's own max HP. Not an Anchor - just enough certainty underfoot to stand on.",
        upgrades: None,
        rank: "Pioneer",
    },
    SkillDef {
        key: "safe_passage",
        name: "Safe Passage",
        class: "explorer",
        unlock: 25,
        description: "Evasion for the WHOLE party: every ally becomes harder to hit until it decays. The Guides' promise, as a stat.",
        upgrades: None,
        rank: "Discoverer",
    },
    SkillDef {
        key: "now",
        name: "Now",
        class: "explorer",
        unlock: 49,
        description: "Every living ally's gauge fills instantly - they all act at once. ONCE per battle.",
        upgrades: None,
        rank: "Globemaster",
    },
    SkillDef {
        key: "a_world_known",
        name: "A World Known",
        class: "explorer",
        unlock: 36,
        description: "HASTE for the whole party: every ally's ATB gauge fills faster for a while.",
        upgrades: None,
        rank: "Globemaster",
    },
    // ---- Hunter: martial. Attacks bank Adrenaline, every skill spends it —
    // "adrenaline junkies", in the guild's own words. A short kit on purpose: the
    // Hunter's late game is the weapon, not a longer menu.
    SkillDef {
        key: "power_strike",
        name: "Power Strike",
        class: "hunter",
        unlock: 1,
        description: "A heavy blow. Spends Adrenaline.",
        upgrades: None,
        rank: "Wisker",
    },
    SkillDef {
        key: "second_wind",
        name: "Second Wind",
        class: "hunter",
        unlock: 4,
        description: "Heals YOURSELF for a share of your max HP. Spends Adrenaline.",
        upgrades: None,
        rank: "Stalker",
    },
    SkillDef {
        key: "snare",
        name: "Snare",
        class: "hunter",
        unlock: 9,
        description: "Damage, and drains the target's ATB gauge so its turn comes later. Spends Adrenaline.",
        upgrades: None,
        rank: "Stalker",
    },
    SkillDef {
        key: "frenzy",
        name: "Frenzy",
        class: "hunter",
        unlock: 16,
        description: "The biggest single hit in the kit, at the biggest Adrenaline cost.",
        upgrades: None,
        rank: "Shikari",
    },
    SkillDef {
        key: "crushing_blow",
        name: "Crushing Blow",
        class: "hunter",
        unlock: 16,
        description: "Power Strike, harder. Spends the same Adrenaline.",
        upgrades: Some("power_strike"),
        rank: "Predator",
    },
    SkillDef {
        key: "pin_the_prey",
        name: "Pin the Prey",
        class: "hunter",
        unlock: 25,
        description: "Snare, with a longer drain. Spends the same Adrenaline.",
        upgrades: Some("snare"),
        rank: "Master Hunter",
    },
    // ---- Psyker: Foci persist and fire every turn ----
    SkillDef {
        key: "gravity_well",
        name: "Gravity Well",
        class: "psyker",
        unlock: 1,
        description: "A Focus: each Psyker turn it damages the target, ignoring armour. Persists until revoked.",
        upgrades: None,
        rank: "Initiate",
    },
    SkillDef {
        key: "kinetic_aegis",
        name: "Kinetic Aegis",
        class: "psyker",
        unlock: 1,
        description: "A Focus: each Psyker turn it grants Barrier. Persists until revoked.",
        upgrades: None,
        rank: "Initiate",
    },
    SkillDef {
        key: "mind_spike",
        name: "Mind Spike",
        class: "psyker",
        unlock: 9,
        description: "A Focus: a stronger armour-ignoring tick each Psyker turn. Persists until revoked.",
        upgrades: None,
        rank: "Tracer",
    },
    SkillDef {
        key: "temporal_anchor",
        name: "Temporal Anchor",
        class: "psyker",
        unlock: 16,
        description: "A Focus: each Psyker turn it drains the target's ATB gauge. Persists until revoked.",
        upgrades: None,
        rank: "Field Marshal",
    },
    SkillDef {
        key: "kinetic_wave",
        name: "Kinetic Wave",
        class: "psyker",
        unlock: 25,
        description: "A Focus that hits EVERY enemy each Psyker turn, ignoring armour.",
        upgrades: None,
        rank: "Field Marshal",
    },
    SkillDef {
        key: "thermal_flux",
        name: "Thermal Flux",
        class: "psyker",
        unlock: 36,
        description: "A Focus: fire damage each Psyker turn, so a target that resists physical still burns.",
        upgrades: None,
        rank: "Field Marshal",
    },
    SkillDef {
        key: "matter_dissolution",
        name: "Matter Dissolution",
        class: "psyker",
        unlock: 49,
        description: "A Focus that strips the target's armour as well as its HP.",
        upgrades: None,
        rank: "Lead Investigator",
    },
    SkillDef {
        key: "phase_shift",
        name: "Phase Shift",
        class: "psyker",
        unlock: 64,
        description: "A Focus that grants the party Evasion each Psyker turn.",
        upgrades: None,
        rank: "Lead Investigator",
    },
    SkillDef {
        key: "dominate_mind",
        name: "Dominate Mind",
        class: "psyker",
        unlock: 81,
        description: "A Focus that drains the target's gauge and damages it - control and pressure at once.",
        upgrades: None,
        rank: "Bureau Chief",
    },
    SkillDef {
        key: "reality_collapse",
        name: "Reality Collapse",
        class: "psyker",
        unlock: 100,
        description: "The heaviest Focus: armour-ignoring damage to every enemy, every Psyker turn.",
        upgrades: None,
        rank: "Director",
    },
    // ---- Resonant: a caster, so its ladder runs the whole way. It has no order and
    // no gear curve to speak of; the kit IS its progression.
    SkillDef {
        key: "transfuse",
        name: "Transfuse",
        class: "resonant",
        unlock: 1,
        description: "Heals an ally for a large share of their max HP, PAID from your own. The healer bleeds so the party does not.",
        upgrades: None,
        rank: "",
    },
    SkillDef {
        key: "regen_boon",
        name: "Regen Boon",
        class: "resonant",
        unlock: 4,
        description: "Grants an ally Regen: HP back at the start of each of their turns.",
        upgrades: None,
        rank: "",
    },
    SkillDef {
        key: "ward",
        name: "Ward",
        class: "resonant",
        unlock: 9,
        description: "Grants an ally Barrier - temporary HP that soaks damage before their own.",
        upgrades: None,
        rank: "",
    },
    SkillDef {
        key: "mend_all",
        name: "Mend All",
        class: "resonant",
        unlock: 16,
        description: "A small heal for EVERY ally at once, paid from your own HP.",
        upgrades: None,
        rank: "",
    },
    SkillDef {
        key: "sanctuary",
        name: "Sanctuary",
        class: "resonant",
        unlock: 25,
        description: "Barrier for the WHOLE party.",
        upgrades: None,
        rank: "",
    },
    SkillDef {
        key: "revitalize",
        name: "Revitalize",
        class: "resonant",
        unlock: 36,
        description: "A large single-target heal with no HP cost to you.",
        upgrades: None,
        rank: "",
    },
    SkillDef {
        key: "lifewell",
        name: "Lifewell",
        class: "resonant",
        unlock: 49,
        description: "Regen for the WHOLE party.",
        upgrades: None,
        rank: "",
    },
    SkillDef {
        key: "bloodbond",
        name: "Bloodbond",
        class: "resonant",
        unlock: 64,
        description: "Heals an ally and grants them Regen in one turn, paid from your own HP.",
        upgrades: None,
        rank: "",
    },
    SkillDef {
        key: "martyr",
        name: "Martyr",
        class: "resonant",
        unlock: 81,
        description: "Spends a large share of your own HP to bring an ally back up to fighting shape.",
        upgrades: None,
        rank: "",
    },
    SkillDef {
        key: "eternal_bloom",
        name: "Eternal Bloom",
        class: "resonant",
        unlock: 100,
        description: "The capstone: the whole party is healed, warded and given Regen at once.",
        upgrades: None,
        rank: "",
    },
    // ---- Shifter: martial. Three tricks, learned early, then it lives on daggers
    // and Dex — a fourth button at level 60 would make it a caster in leather.

    SkillDef {
        key: "backstab",
        name: "Backstab",
        class: "shifter",
        unlock: 1,
        description: "A heavy strike that pierces most of the target's armour.",
        upgrades: None,
        rank: "Flicker Foot",
    },
    SkillDef {
        key: "flicker",
        name: "Flicker",
        class: "shifter",
        unlock: 4,
        description: "Blink: grants YOURSELF a large Evasion bonus that decays each of your turns. The best dodge in the game.",
        upgrades: None,
        rank: "Shift Rat",
    },
    SkillDef {
        key: "ransack",
        name: "Ransack",
        class: "shifter",
        unlock: 9,
        description: "Damage plus a heavy gauge drain.",
        upgrades: None,
        rank: "Shift Rat",
    },
    SkillDef {
        key: "steal",
        name: "Steal",
        class: "shifter",
        unlock: 4,
        description: "Takes the target's tempo - drains its ATB gauge - without hitting it.",
        upgrades: None,
        rank: "Shift Rat",
    },
    SkillDef {
        key: "mug",
        name: "Mug",
        class: "shifter",
        unlock: 25,
        description: "Steal, with a hit on the way past: damage AND a gauge drain.",
        upgrades: Some("steal"),
        rank: "Void-Dancer",
    },
    // ---- Phoenix Guard: the Last City's anti-undead order (docs/lore/factions.md).
    // The ladder IS their rank ladder — Initiate 1, Purifier 2, Exemplar 5,
    // Luminary 9, Redeemer 13, Apotheosis 17 — so every promotion is a new tool.
    SkillDef {
        key: "silvered_strike",
        name: "Silvered Strike",
        class: "phoenix_guard",
        unlock: 1,
        description: "Damage that also drains the target's gauge, and bites far deeper into UNDEAD.",
        upgrades: None,
        rank: "Initiate",
    },
    SkillDef {
        key: "rite_of_rest",
        name: "Rite of Rest",
        class: "phoenix_guard",
        unlock: 4,
        description: "Grants YOURSELF Barrier sized off your own max HP.",
        upgrades: None,
        rank: "Purifier",
    },
    SkillDef {
        key: "holy_censure",
        name: "Holy Censure",
        class: "phoenix_guard",
        unlock: 9,
        description: "Damage that ZEROES the target's ATB gauge - a hard stagger. Extra against undead.",
        upgrades: None,
        rank: "Exemplar",
    },
    SkillDef {
        key: "purging_light",
        name: "Purging Light",
        class: "phoenix_guard",
        unlock: 16,
        description: "Damage to EVERY living enemy. Extra against undead.",
        upgrades: None,
        rank: "Luminary",
    },
    SkillDef {
        key: "unbroken_vigil",
        name: "Unbroken Vigil",
        class: "phoenix_guard",
        unlock: 25,
        description: "Barrier for the WHOLE party, sized off each ally's own max HP.",
        upgrades: None,
        rank: "Redeemer",
    },
    SkillDef {
        key: "eradication",
        name: "Eradication",
        class: "phoenix_guard",
        unlock: 36,
        description: "An execute: the more HP the target is missing, the harder it lands. Extra against undead.",
        upgrades: None,
        rank: "Apotheosis",
    },
    // ---- Smithwright: the Foundry's caste that BUILDS (docs/lore/factions.md). The kit
    // is what a working smith carries into a dangerous place — a hammer, a bulwark, and
    // the heat itself. The ladder is the Foundry's own, so every promotion is a new tool.
    SkillDef {
        key: "hammer_fall",
        name: "Hammer Fall",
        class: "smithwright",
        unlock: 1,
        description: "Damage that also drains the target's gauge - a staggering blow with the tool itself.",
        upgrades: None,
        rank: "Indentured Extractor",
    },
    SkillDef {
        key: "quench",
        name: "Quench",
        class: "smithwright",
        unlock: 4,
        description: "Grants YOURSELF Barrier sized off your own max HP.",
        upgrades: None,
        rank: "Smelter Apprentice",
    },
    SkillDef {
        key: "bulwark",
        name: "Plant the Bulwark",
        class: "smithwright",
        unlock: 12,
        description: "Barrier for the WHOLE party, sized off each ally's own max HP.",
        upgrades: None,
        rank: "Journeyman Smithwright",
    },
    SkillDef {
        key: "tempering_blow",
        name: "Tempering Blow",
        class: "smithwright",
        unlock: 20,
        description: "Raises ONE ally's attack for the rest of the fight. No damage of its own.",
        upgrades: None,
        rank: "Smithwright",
    },
    SkillDef {
        key: "slag_spray",
        name: "Slag Spray",
        class: "smithwright",
        unlock: 28,
        description: "Damage to EVERY enemy, ignoring armour.",
        upgrades: None,
        rank: "Master Smithwright",
    },
    SkillDef {
        key: "one_true_forge",
        name: "The One True Forge",
        class: "smithwright",
        unlock: 36,
        description: "Heals AND shields the whole party at once.",
        upgrades: None,
        rank: "Master of the Foundry",
    },
    // ---- Keeper: the Open Flower in the field. A mender, not a duellist: everything
    // here keeps someone standing, and the ladder is the order's own growth ladder.
    SkillDef {
        key: "thornlash",
        name: "Thornlash",
        class: "keeper",
        unlock: 1,
        description: "Damage (from Mnd, not Str) plus a gauge drain.",
        upgrades: None,
        rank: "Sprout",
    },
    SkillDef {
        key: "poultice",
        name: "Poultice",
        class: "keeper",
        unlock: 4,
        description: "Heals an ally now AND grants them Regen after.",
        upgrades: None,
        rank: "Seedling",
    },
    SkillDef {
        key: "bloomfield",
        name: "Bloomfield",
        class: "keeper",
        unlock: 12,
        description: "Regen for the WHOLE party.",
        upgrades: None,
        rank: "Budling",
    },
    SkillDef {
        key: "root_snare",
        name: "Root Snare",
        class: "keeper",
        unlock: 20,
        description: "Damage and a heavy gauge drain - its turn is a long way off.",
        upgrades: None,
        rank: "Flowerling",
    },
    SkillDef {
        key: "vital_draught",
        name: "Vital Draught",
        class: "keeper",
        unlock: 28,
        description: "Grants an ally Barrier and Regen together.",
        upgrades: None,
        rank: "Cultivator",
    },
    SkillDef {
        key: "terras_gift",
        name: "Terra's Gift",
        class: "keeper",
        unlock: 36,
        description: "The capstone: the whole party is healed, shielded, and pushed up the turn order.",
        upgrades: None,
        rank: "Terra",
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
        "smithwright",
        &[
            ("Indentured Extractor", 1),
            ("Smelter Apprentice", 25),
            ("Journeyman Smithwright", 65),
            ("Smithwright", 115),
            ("Master Smithwright", 165),
            ("Master of the Foundry", 215),
        ],
    ),
    (
        "keeper",
        &[
            ("Sprout", 1),
            ("Seedling", 25),
            ("Budling", 65),
            ("Flowerling", 115),
            ("Cultivator", 165),
            ("Terra", 215),
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
        .rfind(|(_, at)| level >= *at)
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

/// A class's whole kit, including superseded versions. Callers showing a menu want
/// [`skills_for_class_at`] instead.
pub fn skills_for_class(class: &str) -> Vec<&'static SkillDef> {
    SKILLS.iter().filter(|s| s.class == class).collect()
}

/// What a hero of `class` at `level` can actually do: unlocked abilities, with any
/// ability that has been SUPERSEDED dropped. A Shifter with Mug does not also carry
/// Steal — the row improved, it did not multiply.
pub fn skills_for_class_at(class: &str, level: i32) -> Vec<&'static SkillDef> {
    let owned: Vec<&SkillDef> =
        SKILLS.iter().filter(|s| s.class == class && level >= s.unlock).collect();
    let superseded: Vec<&str> = owned.iter().filter_map(|s| s.upgrades).collect();
    owned.into_iter().filter(|s| !superseded.contains(&s.key)).collect()
}

/// The chain an ability belongs to, base first — for a tooltip that wants to say
/// what this grew out of.
pub fn upgrade_chain(key: &str) -> Vec<&'static SkillDef> {
    let mut chain = Vec::new();
    let mut cur = skill(key);
    while let Some(d) = cur {
        chain.push(d);
        cur = d.upgrades.and_then(skill);
    }
    chain.reverse();
    chain
}

/// The level at which `skill` unlocks. Returns 1 for always-available actions
/// (Attack/Defend/Item) and for anything unknown, so a caller that asks about a
/// non-skill gets "usable" rather than "locked forever".
/// Abilities that may be used ONCE per battle. A once-per-fight call is a decision about
/// one moment, so it lives in the registry beside the unlock level rather than as a rule
/// the server and the client each remember separately.
pub fn is_once_per_battle(skill: &str) -> bool {
    matches!(skill, "now")
}

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

    /// A description has to answer "what does this DO". Fluff-only rows shipped for a long
    /// time — Trailblaze's said "a solid strike that costs nothing to make" and never
    /// mentioned that it marks the target, which is the entire reason to press it.
    #[test]
    fn every_ability_description_states_a_mechanic() {
        // Words that describe an EFFECT rather than a mood.
        const MECHANICAL: &[&str] = &[
            "damage", "heal", "barrier", "regen", "evasion", "gauge", "marks", "mark",
            "distracts", "haste", "adrenaline", "focus", "armour", "attack", "hp",
            "execute", "drain", "stagger", "revoke", "shield",
        ];
        for d in SKILLS {
            let lower = d.description.to_lowercase();
            assert!(
                MECHANICAL.iter().any(|w| lower.contains(w)),
                "{} ({}) reads as flavour only: {:?}",
                d.name,
                d.key,
                d.description
            );
            assert!(
                d.description.len() > 20,
                "{}'s description is too short to say anything",
                d.name
            );
        }
    }

    /// Whoever it touches has to be legible too: an ability that hits the party, or only
    /// the caster, says so — "Barrier" alone leaves the player guessing who gets it.
    #[test]
    fn party_and_self_abilities_say_whose_they_are() {
        for key in ["stable_ground", "safe_passage", "bulwark", "unbroken_vigil", "bloomfield"] {
            let d = skill(key).unwrap_or_else(|| panic!("{key} missing"));
            let l = d.description.to_lowercase();
            assert!(
                l.contains("whole party") || l.contains("every ally") || l.contains("party"),
                "{} must say it covers the party: {:?}",
                d.name,
                d.description
            );
        }
        for key in ["second_wind", "quench", "rite_of_rest", "flicker"] {
            let d = skill(key).unwrap_or_else(|| panic!("{key} missing"));
            assert!(
                d.description.to_uppercase().contains("YOURSELF"),
                "{} must say it is self-only: {:?}",
                d.name,
                d.description
            );
        }
    }

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
        for class in [
            "hunter",
            "psyker",
            "resonant",
            "shifter",
            "phoenix_guard",
            "smithwright",
            "keeper",
        ] {
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
    fn an_upgrade_replaces_its_base_rather_than_joining_it() {
        // A Shifter at 4 has Steal. At 25 it has Mug INSTEAD — the martial answer to
        // progression is a better row, not a fifth button.
        let early: Vec<&str> = skills_for_class_at("shifter", 4).iter().map(|s| s.key).collect();
        assert!(early.contains(&"steal"), "{early:?}");
        assert!(!early.contains(&"mug"), "{early:?}");

        let late: Vec<&str> = skills_for_class_at("shifter", 25).iter().map(|s| s.key).collect();
        assert!(late.contains(&"mug"), "{late:?}");
        assert!(!late.contains(&"steal"), "Steal survived its own upgrade: {late:?}");

        // So the menu does not grow: same row count, better rows.
        assert_eq!(
            skills_for_class_at("shifter", 9).len(),
            skills_for_class_at("shifter", 25).len(),
            "the Shifter's menu grew instead of improving"
        );

        // The Hunter upgrades twice and still fields four rows.
        let hunter: Vec<&str> = skills_for_class_at("hunter", 25).iter().map(|s| s.key).collect();
        assert!(hunter.contains(&"crushing_blow") && hunter.contains(&"pin_the_prey"), "{hunter:?}");
        assert!(!hunter.contains(&"power_strike") && !hunter.contains(&"snare"), "{hunter:?}");
        assert_eq!(hunter.len(), 4, "{hunter:?}");
    }

    #[test]
    fn an_upgrade_chain_is_well_formed() {
        for s in SKILLS {
            let Some(base) = s.upgrades else { continue };
            let b = skill(base).unwrap_or_else(|| panic!("{} upgrades unknown {base}", s.key));
            assert_eq!(b.class, s.class, "{} upgrades another class's ability", s.key);
            assert!(
                b.unlock < s.unlock,
                "{} unlocks at or before the {base} it replaces",
                s.key
            );
            // The chain reads base-first, so a tooltip can say where it came from.
            let chain: Vec<&str> = upgrade_chain(s.key).iter().map(|d| d.key).collect();
            assert_eq!(chain.first(), Some(&base));
            assert_eq!(chain.last(), Some(&s.key));
        }
        assert_eq!(upgrade_chain("mug").len(), 2);
        assert_eq!(upgrade_chain("backstab").len(), 1, "an unchained ability is its own chain");
    }

    #[test]
    fn a_martial_kit_is_short_and_a_casters_runs_the_whole_way() {
        // The Dragon Quest rule: a martial class's late game is its weapon, so its
        // ladder ends while the numbers still matter. A caster has no comparable gear
        // curve, so its ladder IS the progression. "More abilities" must not mean
        // "ten each".
        for class in ["hunter", "shifter", "explorer", "psyker", "resonant", "phoenix_guard"] {
            let kit = skills_for_class(class);
            let deepest = kit.iter().map(|s| s.unlock).max().unwrap();
            let ceiling = ladder_ceiling(archetype(class));
            assert!(
                deepest <= ceiling,
                "{class} ({:?}) learns something at {deepest}, past its {ceiling} ceiling",
                archetype(class)
            );
        }
        // And a caster actually reaches for it, rather than stopping early and
        // leaving the archetype a claim nobody honoured.
        let resonant = skills_for_class("resonant");
        assert_eq!(
            resonant.iter().map(|s| s.unlock).max().unwrap(),
            100,
            "the Resonant is a caster and should still be learning at the top"
        );
        assert!(resonant.len() >= 9, "a caster's ladder is thin: {}", resonant.len());
        // A martial class stays lean.
        assert!(
            skills_for_class("shifter").len() <= 5,
            "the Shifter has grown a caster's menu"
        );
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
        // The hybrid kits reach past the martial ceiling, or the archetypes are a
        // distinction with no difference.
        assert!(
            skills_for_class("phoenix_guard").iter().map(|s| s.unlock).max().unwrap()
                > ladder_ceiling(Archetype::Martial),
            "the Phoenix Guard's ladder is no deeper than a martial one"
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
