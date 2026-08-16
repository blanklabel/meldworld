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
//! **The NUMBERS do not live here** — they are `[TUNABLE]`s, and this crate is shared
//! with a client that has no `balance.toml`. So a description says what KIND of thing
//! an ability is, and `meld_run::ability_effects` formats the magnitudes from balance
//! and ships them beside it on `run.party`. Write prose here that stays true whatever
//! the numbers are retuned to; if a row can only be understood by knowing a
//! coefficient, that half belongs in the effect line, not in a literal here.
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

/// How WIDE a class's menu gets — not how deep its ladder runs.
///
/// **Every class learns something at 49 and again at 100**, so levelling pays for
/// everyone all the way out. The Dragon Quest lesson this encodes — a martial class's
/// late game is its weapon, not a longer menu — now lives in *how* it gets there: a
/// martial class reaches 100 through [`SkillDef::upgrades`], the row it already has
/// getting better, while a caster reaches it by learning a genuinely new button.
/// Frenzy becoming Apex Predator is a deeper ladder with the same four rows.
///
/// So the archetype answers "how many things does this class do at once", and the
/// answer stops "more abilities" from meaning "every class ends up with ten".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Archetype {
    /// A lean menu. Its repeatable rows stop improving at 50 and gear carries it from
    /// there; what it learns after is a single dramatic call per fight, not more DPS.
    Martial,
    /// A working spread of tools. (Explorer, Phoenix Guard, Smithwright, Keeper.)
    Hybrid,
    /// A button for every situation; the kit IS the progression. (Psyker, Resonant.)
    Caster,
}

pub fn archetype(class: &str) -> Archetype {
    match class {
        "hunter" | "shifter" => Archetype::Martial,
        "psyker" | "resonant" => Archetype::Caster,
        _ => Archetype::Hybrid,
    }
}

/// The most rows an archetype's menu may hold once everything is unlocked.
pub fn menu_width(a: Archetype) -> usize {
    match a {
        Archetype::Martial => 6,
        Archetype::Hybrid => 8,
        Archetype::Caster => 11,
    }
}

/// The rungs every ladder is cut on. Round numbers, not squares: `49` was the only thing
/// standing between the deep rung and a legible 50, and a player counting to the next
/// ability should be counting in tens.
pub const RUNGS: &[i32] = &[1, 5, 10, 20, 35, 50, 75, 100, 150, 200, 255];

/// The level a class's ladder is expected to REACH, by archetype. A caster's kit is its
/// progression, so it runs to the cap; a martial class is done at 50 and scales on gear
/// from there, with its last two rungs being once-a-fight calls rather than damage.
pub fn ladder_top(a: Archetype) -> i32 {
    match a {
        Archetype::Caster => 255,
        _ => 100,
    }
}

/// Who an ability is aimed at — and, just as importantly, whether the player is asked
/// to aim it at all.
///
/// This lives in the registry because the client used to keep its own list of "which
/// skills need a target pick", and a list is a list a new ability falls off. It had
/// already gone stale silently: it still named the Iron Hull's `root` and
/// `toll_of_the_deep`, so the Phoenix Guard's self-cast Rite of Rest and its all-enemy
/// Purging Light both fell through to "pick an enemy" and asked the player to aim a
/// stance at a creature.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    /// Pick one enemy.
    Enemy,
    /// Pick one ally (resolvers default to the most wounded if nobody is picked).
    Ally,
    /// The caster only — nothing to pick.
    Caster,
    /// Every living enemy — nothing to pick.
    AllEnemies,
    /// The whole party — nothing to pick.
    Party,
}

impl Target {
    /// Whether the player must choose a combatant before this can be submitted.
    pub fn needs_pick(self) -> bool {
        matches!(self, Target::Enemy | Target::Ally)
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
    /// Who it lands on, and whether the player is asked to aim it.
    pub target: Target,
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
    /// The Focus this one is an ASPECT of — it may only be cast onto a target that is
    /// already holding its parent, and it falls the moment the parent does. This is how
    /// the Psyker controls: Pressure crushes, Gravity slows what is being crushed, Anchor
    /// pins what is already slowed. An aspect is never a top-level menu row (see
    /// [`skills_for_class_at`]); it is reached from the manifestation it deepens, which
    /// is what keeps a caster's menu at its width while its DEPTH grows.
    pub requires: Option<&'static str>,
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
        target: Target::Enemy,
        description: "Damage, and MARKS the target: every ally hits it harder until the mark fades. Costs nothing.",
        upgrades: None,
        requires: None,
        rank: "Walker",
    },
    SkillDef {
        key: "field_dressing",
        name: "Field Dressing",
        class: "explorer",
        unlock: 5,
        target: Target::Ally,
        description: "Heals one ally (the most wounded if you pick nobody) for a share of their max HP. Costs nothing.",
        upgrades: None,
        requires: None,
        rank: "Traveler",
    },
    SkillDef {
        key: "misdirection",
        name: "Misdirection",
        class: "explorer",
        unlock: 10,
        target: Target::Enemy,
        description: "Damage, drains the target's ATB gauge, and DISTRACTS it: it swings wide at whoever it attacks, and the party's chance to flee goes up while it holds.",
        upgrades: None,
        requires: None,
        rank: "Scout",
    },
    SkillDef {
        key: "stable_ground",
        name: "Stable Ground",
        class: "explorer",
        unlock: 20,
        target: Target::Party,
        description: "Barrier for the WHOLE party, sized off each ally's own max HP. Not an Anchor - just enough certainty underfoot to stand on.",
        upgrades: None,
        requires: None,
        rank: "Pioneer",
    },
    SkillDef {
        key: "safe_passage",
        name: "Safe Passage",
        class: "explorer",
        unlock: 35,
        target: Target::Party,
        description: "Evasion for the WHOLE party: every ally becomes harder to hit until it decays. The Guides' promise, as a stat.",
        upgrades: None,
        requires: None,
        rank: "Discoverer",
    },
    SkillDef {
        key: "now",
        name: "Now",
        class: "explorer",
        unlock: 75,
        target: Target::Party,
        description: "Every living ally's gauge fills instantly - they all act at once. ONCE per battle.",
        upgrades: None,
        requires: None,
        rank: "Globemaster",
    },
    SkillDef {
        key: "a_world_known",
        name: "A World Known",
        class: "explorer",
        unlock: 50,
        target: Target::Party,
        description: "HASTE for the whole party: every ally's ATB gauge fills faster for a while.",
        upgrades: None,
        requires: None,
        rank: "Globemaster",
    },
    SkillDef {
        key: "the_world_entire",
        name: "The World Entire",
        class: "explorer",
        unlock: 100,
        target: Target::Party,
        description: "The whole field, read at once: MARKS every enemy so every ally hits all of them harder, and HASTES the whole party at the same time. ONCE per battle.",
        upgrades: None,
        requires: None,
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
        target: Target::Enemy,
        description: "A heavy blow. Spends Adrenaline.",
        upgrades: None,
        requires: None,
        rank: "Wisker",
    },
    SkillDef {
        key: "second_wind",
        name: "Second Wind",
        class: "hunter",
        unlock: 5,
        target: Target::Caster,
        description: "Heals YOURSELF for a share of your max HP. Spends Adrenaline.",
        upgrades: None,
        requires: None,
        rank: "Stalker",
    },
    SkillDef {
        key: "snare",
        name: "Snare",
        class: "hunter",
        unlock: 10,
        target: Target::Enemy,
        description: "Damage, and drains the target's ATB gauge so its turn comes later. Spends Adrenaline.",
        upgrades: None,
        requires: None,
        rank: "Stalker",
    },
    SkillDef {
        key: "frenzy",
        name: "Frenzy",
        class: "hunter",
        unlock: 20,
        target: Target::Enemy,
        description: "The biggest single hit in the kit, at the biggest Adrenaline cost.",
        upgrades: None,
        requires: None,
        rank: "Shikari",
    },
    SkillDef {
        key: "crushing_blow",
        name: "Crushing Blow",
        class: "hunter",
        unlock: 35,
        target: Target::Enemy,
        description: "Power Strike, harder. Spends the same Adrenaline.",
        upgrades: Some("power_strike"),
        requires: None,
        rank: "Predator",
    },
    SkillDef {
        key: "iron_lung",
        name: "Iron Lung",
        class: "hunter",
        unlock: 75,
        target: Target::Caster,
        description: "The deep breath before the last push: heals YOURSELF far more than Second Wind and leaves Regen behind. ONCE per battle, and it spends Adrenaline.",
        upgrades: None,
        requires: None,
        rank: "Master Hunter",
    },
    SkillDef {
        key: "apex_predator",
        name: "Apex Predator",
        class: "hunter",
        unlock: 50,
        target: Target::AllEnemies,
        description: "Frenzy, turned on the whole pack: the biggest hit in the kit, against EVERY enemy at once. Spends the same Adrenaline.",
        upgrades: Some("frenzy"),
        requires: None,
        rank: "Apex",
    },
    SkillDef {
        key: "pin_the_prey",
        name: "Pin the Prey",
        class: "hunter",
        unlock: 100,
        target: Target::AllEnemies,
        description: "The whole pack pinned at once: damage and a heavy gauge drain on EVERY enemy. ONCE per battle, and it spends Adrenaline.",
        upgrades: None,
        requires: None,
        rank: "Master Hunter",
    },
    // ---- Psyker: Foci persist and fire every turn ----
    SkillDef {
        key: "gravity_well",
        name: "Gravity Well",
        class: "psyker",
        unlock: 1,
        target: Target::Enemy,
        description: "A Focus: each Psyker turn it damages the target, ignoring armour. Persists until revoked.",
        upgrades: None,
        requires: None,
        rank: "Initiate",
    },
    // Gravity Well's own aspects. The doc's chain — Pressure crushes, Gravity slows what
    // is being crushed, Anchor pins what is already slowed — is what makes this class a
    // CONTROLLER from its first level rather than a damage tick until Temporal Anchor at
    // 20. Each occupies its own Focus slot, so an early Psyker holding both halves of the
    // chain is holding nothing else: depth costs width.
    SkillDef {
        key: "gravity",
        name: "Gravity",
        class: "psyker",
        unlock: 5,
        target: Target::Enemy,
        description: "An aspect of Gravity Well: the crushed target's gauge fills slower for as long as both are held.",
        upgrades: None,
        requires: Some("gravity_well"),
        rank: "Initiate",
    },
    SkillDef {
        key: "anchor",
        name: "Anchor",
        class: "psyker",
        unlock: 20,
        target: Target::Enemy,
        description: "An aspect of Gravity: what is already slowed is pinned, its gauge crawling, while all three are held.",
        upgrades: None,
        requires: Some("gravity"),
        rank: "Tracer",
    },
    SkillDef {
        key: "shield",
        name: "Shield",
        class: "psyker",
        unlock: 10,
        target: Target::Party,
        description: "An aspect of Kinetic Aegis: the Barrier covers the WHOLE party each Psyker turn, not just you.",
        upgrades: None,
        requires: Some("kinetic_aegis"),
        rank: "Initiate",
    },
    SkillDef {
        key: "acceleration",
        name: "Acceleration",
        class: "psyker",
        unlock: 35,
        target: Target::Ally,
        description: "An aspect of Temporal Anchor: time runs fast for one ALLY, filling its ATB gauge each Psyker turn.",
        upgrades: None,
        requires: Some("temporal_anchor"),
        rank: "Field Marshal",
    },
    SkillDef {
        key: "freeze",
        name: "Freeze",
        class: "psyker",
        unlock: 75,
        target: Target::Enemy,
        description: "An aspect of Thermal Flux: the burning target's gauge fills slower - and anything already slowed is pinned outright.",
        upgrades: None,
        requires: Some("thermal_flux"),
        rank: "Lead Investigator",
    },
    SkillDef {
        key: "brittle",
        name: "Brittle",
        class: "psyker",
        unlock: 100,
        target: Target::Enemy,
        description: "An aspect of Matter Dissolution: the corroded target loses its elemental resistances for good, so every damage type lands in full.",
        upgrades: None,
        requires: Some("matter_dissolution"),
        rank: "Lead Investigator",
    },
    SkillDef {
        key: "blackout",
        name: "Blackout",
        class: "psyker",
        unlock: 200,
        target: Target::Enemy,
        description: "An aspect of Dominate Mind: its senses are cut, so its dodge and evasion are gone and every attack lands.",
        upgrades: None,
        requires: Some("dominate_mind"),
        rank: "Director",
    },
    SkillDef {
        key: "kinetic_aegis",
        name: "Kinetic Aegis",
        class: "psyker",
        unlock: 5,
        target: Target::Caster,
        description: "A Focus: each Psyker turn it grants YOURSELF Barrier. Persists until revoked.",
        upgrades: None,
        requires: None,
        rank: "Initiate",
    },
    SkillDef {
        key: "mind_spike",
        name: "Mind Spike",
        class: "psyker",
        unlock: 10,
        target: Target::Enemy,
        description: "A Focus: a stronger armour-ignoring tick each Psyker turn. Persists until revoked.",
        upgrades: None,
        requires: None,
        rank: "Tracer",
    },
    SkillDef {
        key: "temporal_anchor",
        name: "Temporal Anchor",
        class: "psyker",
        unlock: 20,
        target: Target::Enemy,
        description: "A Focus: each Psyker turn it drains the target's ATB gauge. Persists until revoked.",
        upgrades: None,
        requires: None,
        rank: "Field Marshal",
    },
    SkillDef {
        key: "kinetic_wave",
        name: "Kinetic Wave",
        class: "psyker",
        unlock: 35,
        target: Target::AllEnemies,
        description: "A Focus that hits EVERY enemy each Psyker turn, ignoring armour.",
        upgrades: None,
        requires: None,
        rank: "Field Marshal",
    },
    SkillDef {
        key: "thermal_flux",
        name: "Thermal Flux",
        class: "psyker",
        unlock: 50,
        target: Target::Enemy,
        description: "A Focus: fire damage each Psyker turn, so a target that resists physical still burns.",
        upgrades: None,
        requires: None,
        rank: "Field Marshal",
    },
    SkillDef {
        key: "matter_dissolution",
        name: "Matter Dissolution",
        class: "psyker",
        unlock: 75,
        target: Target::Enemy,
        description: "A Focus that strips the target's armour as well as its HP.",
        upgrades: None,
        requires: None,
        rank: "Lead Investigator",
    },
    SkillDef {
        key: "phase_shift",
        name: "Phase Shift",
        class: "psyker",
        unlock: 100,
        target: Target::Caster,
        description: "A Focus: each Psyker turn it grants YOURSELF Evasion, so you are harder to hit for as long as it is held.",
        upgrades: None,
        requires: None,
        rank: "Lead Investigator",
    },
    SkillDef {
        key: "dominate_mind",
        name: "Dominate Mind",
        class: "psyker",
        unlock: 150,
        target: Target::Enemy,
        description: "A Focus that drains the target's gauge and damages it - control and pressure at once.",
        upgrades: None,
        requires: None,
        rank: "Bureau Chief",
    },
    SkillDef {
        key: "reality_collapse",
        name: "Reality Collapse",
        class: "psyker",
        unlock: 200,
        target: Target::AllEnemies,
        description: "The heaviest Focus: armour-ignoring damage to every enemy, every Psyker turn.",
        upgrades: None,
        requires: None,
        rank: "Director",
    },
    SkillDef {
        key: "gravity_vortex",
        name: "Gravity Vortex",
        class: "psyker",
        unlock: 255,
        target: Target::AllEnemies,
        description: "The last Focus: a sphere of warped spacetime over the whole line - every enemy's gauge fills at half speed while it is held, and all of them are ground down each Psyker turn.",
        upgrades: None,
        requires: None,
        rank: "Director",
    },
    // ---- Resonant: a caster, so its ladder runs the whole way. It has no order and
    // no gear curve to speak of; the kit IS its progression.
    SkillDef {
        key: "transfuse",
        name: "Transfuse",
        class: "resonant",
        unlock: 1,
        target: Target::Ally,
        description: "Heals an ally for a large share of their max HP, PAID from your own. The healer bleeds so the party does not.",
        upgrades: None,
        requires: None,
        rank: "",
    },
    SkillDef {
        key: "regen_boon",
        name: "Regen Boon",
        class: "resonant",
        unlock: 5,
        target: Target::Ally,
        description: "Grants an ally Regen: HP back at the start of each of their turns.",
        upgrades: None,
        requires: None,
        rank: "",
    },
    SkillDef {
        key: "ward",
        name: "Ward",
        class: "resonant",
        unlock: 10,
        target: Target::Ally,
        description: "Grants an ally Barrier - temporary HP that soaks damage before their own.",
        upgrades: None,
        requires: None,
        rank: "",
    },
    SkillDef {
        key: "mend_all",
        name: "Mend All",
        class: "resonant",
        unlock: 20,
        target: Target::Party,
        description: "A small heal for EVERY ally at once, paid from your own HP.",
        upgrades: None,
        requires: None,
        rank: "",
    },
    SkillDef {
        key: "sanctuary",
        name: "Sanctuary",
        class: "resonant",
        unlock: 35,
        target: Target::Party,
        description: "Regen for the WHOLE party, and it costs you nothing.",
        upgrades: None,
        requires: None,
        rank: "",
    },
    SkillDef {
        key: "revitalize",
        name: "Revitalize",
        class: "resonant",
        unlock: 50,
        target: Target::Ally,
        description: "A large heal for ONE ally, paid from your own HP.",
        upgrades: None,
        requires: None,
        rank: "",
    },
    SkillDef {
        key: "lifewell",
        name: "Lifewell",
        class: "resonant",
        unlock: 75,
        target: Target::Party,
        description: "Heals the WHOLE party and grants them Regen, paid from your own HP.",
        upgrades: None,
        requires: None,
        rank: "",
    },
    SkillDef {
        key: "bloodbond",
        name: "Bloodbond",
        class: "resonant",
        unlock: 100,
        target: Target::Ally,
        description: "Heals ONE ally, Wards them and grants Regen in one turn — the heaviest single-target boon, and the heaviest HP cost to you.",
        upgrades: None,
        requires: None,
        rank: "",
    },
    SkillDef {
        key: "martyr",
        name: "Martyr",
        class: "resonant",
        unlock: 150,
        target: Target::Party,
        description: "Heals the WHOLE party for most of their max HP, and spends most of your own to do it.",
        upgrades: None,
        requires: None,
        rank: "",
    },
    SkillDef {
        key: "eternal_bloom",
        name: "Eternal Bloom",
        class: "resonant",
        unlock: 200,
        target: Target::Party,
        description: "The capstone: the whole party is healed and Warded at once, paid from your own HP. ONCE per battle.",
        upgrades: None,
        requires: None,
        rank: "",
    },
    // ---- Shifter: martial. Three tricks, learned early, then it lives on daggers
    // and Dex — a fourth button at level 60 would make it a caster in leather.

    SkillDef {
        key: "backstab",
        name: "Backstab",
        class: "shifter",
        unlock: 1,
        target: Target::Enemy,
        description: "A heavy strike that pierces most of the target's armour.",
        upgrades: None,
        requires: None,
        rank: "Flicker Foot",
    },
    SkillDef {
        key: "flicker",
        name: "Flicker",
        class: "shifter",
        unlock: 5,
        target: Target::Caster,
        description: "Blink: grants YOURSELF a large Evasion bonus that decays each of your turns. The best dodge in the game.",
        upgrades: None,
        requires: None,
        rank: "Shift Rat",
    },
    SkillDef {
        key: "ransack",
        name: "Ransack",
        class: "shifter",
        unlock: 20,
        target: Target::Enemy,
        description: "Damage plus a heavy gauge drain.",
        upgrades: None,
        requires: None,
        rank: "Shift Rat",
    },
    SkillDef {
        key: "steal",
        name: "Steal",
        class: "shifter",
        unlock: 10,
        target: Target::Enemy,
        description: "Takes the target's tempo - drains its ATB gauge - without hitting it.",
        upgrades: None,
        requires: None,
        rank: "Shift Rat",
    },
    SkillDef {
        key: "assassinate",
        name: "Assassinate",
        class: "shifter",
        unlock: 50,
        target: Target::Enemy,
        description: "Backstab, placed properly: a heavier strike that ignores the target's armour ENTIRELY rather than most of it.",
        upgrades: Some("backstab"),
        requires: None,
        rank: "Void-Dancer",
    },
    SkillDef {
        key: "grand_larceny",
        name: "Grand Larceny",
        class: "shifter",
        unlock: 100,
        target: Target::AllEnemies,
        description: "The whole room worked at once: a Mug against EVERY enemy — damage, a heavy gauge drain, and you pick all of their pockets. ONCE per battle.",
        upgrades: None,
        requires: None,
        rank: "The Named",
    },
    SkillDef {
        key: "mug",
        name: "Mug",
        class: "shifter",
        unlock: 35,
        target: Target::Enemy,
        description: "Steal, with a hit on the way past: damage AND a gauge drain.",
        upgrades: Some("steal"),
        requires: None,
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
        target: Target::Enemy,
        description: "Damage that also drains the target's gauge, and bites far deeper into UNDEAD.",
        upgrades: None,
        requires: None,
        rank: "Initiate",
    },
    SkillDef {
        key: "rite_of_rest",
        name: "Rite of Rest",
        class: "phoenix_guard",
        unlock: 5,
        target: Target::Caster,
        description: "Grants YOURSELF Barrier sized off your own max HP.",
        upgrades: None,
        requires: None,
        rank: "Purifier",
    },
    SkillDef {
        key: "holy_censure",
        name: "Holy Censure",
        class: "phoenix_guard",
        unlock: 10,
        target: Target::Enemy,
        description: "Damage that ZEROES the target's ATB gauge - a hard stagger. Extra against undead.",
        upgrades: None,
        requires: None,
        rank: "Exemplar",
    },
    SkillDef {
        key: "purging_light",
        name: "Purging Light",
        class: "phoenix_guard",
        unlock: 20,
        target: Target::AllEnemies,
        description: "Damage to EVERY living enemy. Extra against undead.",
        upgrades: None,
        requires: None,
        rank: "Luminary",
    },
    SkillDef {
        key: "unbroken_vigil",
        name: "Unbroken Vigil",
        class: "phoenix_guard",
        unlock: 35,
        target: Target::Party,
        description: "Barrier for the WHOLE party, sized off each ally's own max HP.",
        upgrades: None,
        requires: None,
        rank: "Redeemer",
    },
    SkillDef {
        key: "hallowed_ground",
        name: "Hallowed Ground",
        class: "phoenix_guard",
        unlock: 75,
        target: Target::AllEnemies,
        description: "Consecrates the field: damage to EVERY living enemy that also ZEROES each of their ATB gauges. Extra against undead. ONCE per battle.",
        upgrades: None,
        requires: None,
        rank: "Apotheosis",
    },
    SkillDef {
        key: "phoenix_ascendant",
        name: "Phoenix Ascendant",
        class: "phoenix_guard",
        unlock: 100,
        target: Target::AllEnemies,
        description: "The order's own fire: heavy damage to EVERY enemy, far heavier against undead, and Barrier for the WHOLE party out of the same flame. ONCE per battle.",
        upgrades: None,
        requires: None,
        rank: "Apotheosis",
    },
    SkillDef {
        key: "eradication",
        name: "Eradication",
        class: "phoenix_guard",
        unlock: 50,
        target: Target::Enemy,
        description: "An execute: the more HP the target is missing, the harder it lands. Extra against undead.",
        upgrades: None,
        requires: None,
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
        target: Target::Enemy,
        description: "Damage that also drains the target's gauge - a staggering blow with the tool itself.",
        upgrades: None,
        requires: None,
        rank: "Indentured Extractor",
    },
    SkillDef {
        key: "quench",
        name: "Quench",
        class: "smithwright",
        unlock: 5,
        target: Target::Caster,
        description: "Grants YOURSELF Barrier sized off your own max HP.",
        upgrades: None,
        requires: None,
        rank: "Smelter Apprentice",
    },
    SkillDef {
        key: "bulwark",
        name: "Plant the Bulwark",
        class: "smithwright",
        unlock: 10,
        target: Target::Party,
        description: "Barrier for the WHOLE party, sized off each ally's own max HP.",
        upgrades: None,
        requires: None,
        rank: "Journeyman Smithwright",
    },
    SkillDef {
        key: "tempering_blow",
        name: "Tempering Blow",
        class: "smithwright",
        unlock: 20,
        target: Target::Ally,
        description: "Raises ONE ally's attack for the rest of the fight. No damage of its own.",
        upgrades: None,
        requires: None,
        rank: "Smithwright",
    },
    SkillDef {
        key: "slag_spray",
        name: "Slag Spray",
        class: "smithwright",
        unlock: 35,
        target: Target::AllEnemies,
        description: "Damage to EVERY enemy, ignoring armour.",
        upgrades: None,
        requires: None,
        rank: "Master Smithwright",
    },
    SkillDef {
        key: "anvil_chorus",
        name: "Anvil Chorus",
        class: "smithwright",
        unlock: 75,
        target: Target::Party,
        description: "Tempering Blow for the WHOLE party: every ally's attack goes up for the rest of the fight. No damage of its own. ONCE per battle.",
        upgrades: None,
        requires: None,
        rank: "Master of the Foundry",
    },
    SkillDef {
        key: "great_work",
        name: "The Great Work",
        class: "smithwright",
        unlock: 100,
        target: Target::Party,
        description: "Everything the trade knows, at once: the whole party is healed, given Barrier, AND has its attack raised for the rest of the fight. ONCE per battle.",
        upgrades: None,
        requires: None,
        rank: "Master of the Foundry",
    },
    SkillDef {
        key: "one_true_forge",
        name: "The One True Forge",
        class: "smithwright",
        unlock: 50,
        target: Target::Party,
        description: "Heals AND shields the whole party at once.",
        upgrades: None,
        requires: None,
        rank: "Master of the Foundry",
    },
    // ---- Keeper: the Open Flower in the field. A mender, not a duellist: everything
    // here keeps someone standing, and the ladder is the order's own growth ladder.
    SkillDef {
        key: "thornlash",
        name: "Thornlash",
        class: "keeper",
        unlock: 1,
        target: Target::Enemy,
        description: "Damage (from Mnd, not Str) plus a gauge drain.",
        upgrades: None,
        requires: None,
        rank: "Sprout",
    },
    SkillDef {
        key: "poultice",
        name: "Poultice",
        class: "keeper",
        unlock: 5,
        target: Target::Ally,
        description: "Heals an ally now AND grants them Regen after.",
        upgrades: None,
        requires: None,
        rank: "Seedling",
    },
    SkillDef {
        key: "bloomfield",
        name: "Bloomfield",
        class: "keeper",
        unlock: 10,
        target: Target::Party,
        description: "Regen for the WHOLE party.",
        upgrades: None,
        requires: None,
        rank: "Budling",
    },
    SkillDef {
        key: "root_snare",
        name: "Root Snare",
        class: "keeper",
        unlock: 20,
        target: Target::Enemy,
        description: "Damage and a heavy gauge drain - its turn is a long way off.",
        upgrades: None,
        requires: None,
        rank: "Flowerling",
    },
    SkillDef {
        key: "vital_draught",
        name: "Vital Draught",
        class: "keeper",
        unlock: 35,
        target: Target::Ally,
        description: "Grants an ally Barrier and Regen together.",
        upgrades: None,
        requires: None,
        rank: "Cultivator",
    },
    SkillDef {
        key: "thorn_grove",
        name: "Thorn Grove",
        class: "keeper",
        unlock: 75,
        target: Target::AllEnemies,
        description: "The ground itself closes in: damage (from Mnd, not Str) AND a gauge drain on EVERY enemy at once.",
        upgrades: None,
        requires: None,
        rank: "Terra",
    },
    SkillDef {
        key: "world_tree",
        name: "World Tree",
        class: "keeper",
        unlock: 100,
        target: Target::Party,
        description: "The capstone: the WHOLE party is healed, given Barrier, and given Regen — everything the order knows about keeping people alive, in one turn. ONCE per battle.",
        upgrades: None,
        requires: None,
        rank: "Terra",
    },
    SkillDef {
        key: "terras_gift",
        name: "Terra's Gift",
        class: "keeper",
        unlock: 50,
        target: Target::Party,
        description: "The capstone: the whole party is healed, shielded, and pushed up the turn order.",
        upgrades: None,
        requires: None,
        rank: "Terra",
    },
    SkillDef {
        key: "second_life",
        name: "Second Life",
        class: "resonant",
        unlock: 255,
        target: Target::Party,
        description: "What the order is for: a fallen ally stands back up at part of their max HP, and the whole party is healed in the same breath. ONCE per battle, paid heavily out of your own HP.",
        upgrades: None,
        requires: None,
        rank: "",
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

/// Every class that owns a kit, in registry order. Read off `SKILLS` rather than
/// listed, so a class cannot be added and then quietly left out of a rule.
pub fn all_classes() -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for s in SKILLS {
        if !out.iter().any(|c| c == s.class) {
            out.push(s.class.to_string());
        }
    }
    out
}

/// A class's whole kit, including superseded versions, **in ladder order**. Callers
/// showing a menu want [`skills_for_class_at`] instead.
///
/// Sorted by unlock, not registry order: the Explorer's `Now` (49) is written above
/// `A World Known` (36) in the table, and the abilities panel listed them that way —
/// a ladder shown out of order reads as no ladder.
pub fn skills_for_class(class: &str) -> Vec<&'static SkillDef> {
    let mut kit: Vec<&'static SkillDef> = SKILLS.iter().filter(|s| s.class == class).collect();
    kit.sort_by_key(|s| s.unlock);
    kit
}

/// What a hero of `class` at `level` can actually do: unlocked abilities, with any
/// ability that has been SUPERSEDED dropped. A Shifter with Mug does not also carry
/// Steal — the row improved, it did not multiply.
pub fn skills_for_class_at(class: &str, level: i32) -> Vec<&'static SkillDef> {
    let owned: Vec<&SkillDef> =
        skills_for_class(class).into_iter().filter(|s| level >= s.unlock).collect();
    let superseded: Vec<&str> = owned.iter().filter_map(|s| s.upgrades).collect();
    owned
        .into_iter()
        // An ASPECT is not a row of its own: it is reached from the manifestation it
        // deepens ([`aspects_of`]), so the menu stays as wide as the class has *ideas*
        // rather than as wide as it has buttons.
        .filter(|s| s.requires.is_none() && !superseded.contains(&s.key))
        .collect()
}

/// The aspects that deepen `key`, in unlock order — the rows a manifestation opens onto.
pub fn aspects_of(key: &str) -> Vec<&'static SkillDef> {
    let mut v: Vec<&SkillDef> = SKILLS.iter().filter(|s| s.requires == Some(key)).collect();
    v.sort_by_key(|s| s.unlock);
    v
}

/// Every aspect a hero owns at `level`, parent first — the whole chain, flattened, for
/// the surfaces that need to know what is castable rather than what is on the menu.
pub fn aspect_chain_at(class: &str, level: i32) -> Vec<&'static SkillDef> {
    skills_for_class(class)
        .into_iter()
        .filter(|s| level >= s.unlock && s.requires.is_some())
        .collect()
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
    matches!(
        skill,
        // A once-a-fight call is one of two things: a decision about a single MOMENT
        // (every ally acts NOW; a fallen hero stands up), or an effect that would be
        // degenerate on repeat. Hallowed Ground is the second kind — it zeroes EVERY
        // enemy gauge, and creature speed is a fixed constant while a hero's climbs with
        // Dex, so a deep Phoenix Guard casting it on repeat means nothing on the other
        // side ever acts again.
        "now"
            | "the_world_entire"
            | "iron_lung"
            | "pin_the_prey"
            | "grand_larceny"
            | "hallowed_ground"
            | "phoenix_ascendant"
            | "anvil_chorus"
            | "great_work"
            | "world_tree"
            | "eternal_bloom"
            | "second_life"
    )
}

pub fn unlock_level(skill: &str) -> i32 {
    self::skill(skill).map(|s| s.unlock).unwrap_or(1)
}

/// What `skill` does, for a tooltip. Empty for actions that need no explanation.
/// Who `skill` is aimed at. An action outside the registry (Attack) is an enemy pick,
/// which is what every caller wanted as its fallback anyway.
pub fn target_of(skill: &str) -> Target {
    self::skill(skill).map(|s| s.target).unwrap_or(Target::Enemy)
}

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
        // A Shifter at 10 has Steal. At 35 it has Mug INSTEAD — the martial answer to
        // progression is a better row, not a fifth button.
        let early: Vec<&str> = skills_for_class_at("shifter", 10).iter().map(|s| s.key).collect();
        assert!(early.contains(&"steal"), "{early:?}");
        assert!(!early.contains(&"mug"), "{early:?}");

        let late: Vec<&str> = skills_for_class_at("shifter", 35).iter().map(|s| s.key).collect();
        assert!(late.contains(&"mug"), "{late:?}");
        assert!(!late.contains(&"steal"), "Steal survived its own upgrade: {late:?}");

        // So the repeatable menu does not grow: same row count, better rows.
        assert_eq!(
            skills_for_class_at("shifter", 20).len(),
            skills_for_class_at("shifter", 50).len(),
            "the Shifter's menu grew instead of improving"
        );

        // The Hunter upgrades twice and still fields four repeatable rows at 50.
        let hunter: Vec<&str> = skills_for_class_at("hunter", 50).iter().map(|s| s.key).collect();
        assert!(hunter.contains(&"crushing_blow") && hunter.contains(&"apex_predator"), "{hunter:?}");
        assert!(!hunter.contains(&"power_strike") && !hunter.contains(&"frenzy"), "{hunter:?}");
        assert_eq!(hunter.len(), 4, "{hunter:?}");

        // Past 50 it gains only once-a-fight calls, which is the whole martial bargain.
        let capped = skills_for_class_at("hunter", 255);
        assert_eq!(capped.len(), 6, "{capped:?}");
        assert_eq!(capped.iter().filter(|d| is_once_per_battle(d.key)).count(), 2);
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

    /// **A capstone that lands on the WHOLE party or EVERY enemy is a once-a-fight call.**
    /// That is the rule the gated list encodes, and it is checked rather than trusted: a
    /// class's deepest rung, if it covers everyone, must be gated. Repeatable all-enemy
    /// DAMAGE is fine — that is a rotation — which is why the exceptions below are named
    /// individually rather than the rule being weakened to fit them.
    #[test]
    fn a_party_wide_capstone_is_a_once_a_fight_call() {
        for class in all_classes() {
            let class = class.as_str();
            let kit = skills_for_class(class);
            let deepest = kit.iter().map(|s| s.unlock).max().unwrap();
            for d in kit.iter().filter(|d| d.unlock == deepest) {
                // A Psyker's capstone is a FOCUS: it is seated and held, and the limit on
                // it is the slot it occupies out of five, not a use count. Gating one to
                // once a fight would mean revoking it ended the ability for the battle,
                // which is not what any of the other ten Foci do.
                if class == "psyker" {
                    continue;
                }
                if matches!(d.target, Target::Party | Target::AllEnemies) {
                    assert!(
                        is_once_per_battle(d.key),
                        "{} is {}'s capstone and covers everyone, but is not gated",
                        d.name,
                        class
                    );
                }
            }
        }
        // And the gate is spelled out in the prose, or the player only finds out by
        // being refused mid-fight.
        for d in SKILLS.iter().filter(|d| is_once_per_battle(d.key)) {
            assert!(
                d.description.contains("ONCE per battle"),
                "{} is gated but never says so: {:?}",
                d.name,
                d.description
            );
        }
    }

    /// EVERY class learns something at 49 and again at 100 — read off the registry, so
    /// a class added later cannot quietly stop paying for levels the way the Hunter,
    /// Shifter, Phoenix Guard, Smithwright and Keeper all did at 25 or 36.
    /// A row that says "the WHOLE party" and then asks the player to pick one enemy is
    /// the bug this field exists to stop, so the two halves are held against each other:
    /// the prose and the targeting have to tell the same story.
    #[test]
    fn the_prose_and_the_targeting_agree() {
        for d in SKILLS {
            let l = d.description.to_lowercase();
            let says_party =
                l.contains("whole party") || l.contains("every ally") || l.contains("the party");
            let says_all = l.contains("every enemy") || l.contains("every living enemy");
            let says_self = d.description.contains("YOURSELF");
            match d.target {
                Target::Party => assert!(
                    says_party || d.key == "now" || d.key == "the_world_entire",
                    "{} lands on the party but never says so: {:?}",
                    d.name,
                    d.description
                ),
                Target::AllEnemies => assert!(
                    says_all,
                    "{} hits every enemy but never says so: {:?}",
                    d.name,
                    d.description
                ),
                Target::Caster => assert!(
                    says_self,
                    "{} is self-only but never says so: {:?}",
                    d.name,
                    d.description
                ),
                Target::Enemy | Target::Ally => {
                    // "every ally hits it harder" is who BENEFITS, not who it lands on
                    // — Trailblaze is still one pick — so only the explicit
                    // whole-party/every-enemy phrasings contradict a single target.
                    assert!(
                        !l.contains("whole party") && !says_all,
                        "{} is a single pick but its prose covers everyone: {:?}",
                        d.name,
                        d.description
                    );
                }
            }
            // And only a single pick may ask the player to aim.
            assert_eq!(
                d.target.needs_pick(),
                matches!(d.target, Target::Enemy | Target::Ally),
                "{}",
                d.name
            );
        }
        assert_eq!(target_of("attack"), Target::Enemy, "an unknown action aims at a foe");
    }

    #[test]
    fn every_class_is_still_learning_at_fifty_and_at_a_hundred() {
        for class in all_classes() {
            let class = class.as_str();
            let levels: Vec<i32> = skills_for_class(class).iter().map(|s| s.unlock).collect();
            assert!(levels.contains(&50), "{class} learns nothing at 50: {levels:?}");
            assert!(levels.contains(&100), "{class} learns nothing at 100: {levels:?}");
            let top = ladder_top(archetype(class));
            assert_eq!(
                levels.iter().max().copied(),
                Some(top),
                "{class} ({:?}) should finish at {top}: {levels:?}",
                archetype(class)
            );
        }
        // A caster genuinely runs deeper than everyone else, or the archetypes are a
        // distinction with no difference.
        assert_eq!(ladder_top(Archetype::Caster), 255);
        assert!(ladder_top(Archetype::Caster) > ladder_top(Archetype::Martial));
    }

    /// The ladder got deeper for everyone, so the thing that still separates a martial
    /// class from a caster is menu WIDTH: a martial class's deep rungs must upgrade a
    /// row it already has, not add a fifth button.
    #[test]
    fn a_martial_kit_stays_lean_and_a_casters_runs_wide() {
        for class in all_classes() {
            let class = class.as_str();
            let rows = skills_for_class_at(class, 255).len();
            let width = menu_width(archetype(class));
            assert!(
                rows <= width,
                "{class} ({:?}) fields {rows} rows at the cap, past its {width}",
                archetype(class)
            );
        }
        // A martial class's REPEATABLE menu does not grow past 50 — everything it learns
        // after that is a once-a-fight call, which is what "it scales on gear" means here.
        for class in ["hunter", "shifter"] {
            let repeatable = |lv| {
                skills_for_class_at(class, lv)
                    .into_iter()
                    .filter(|d| !is_once_per_battle(d.key))
                    .count()
            };
            assert_eq!(
                repeatable(50),
                repeatable(255),
                "{class} grew a repeatable row after 50"
            );
        }
        // A caster earns its width, or the archetypes are a distinction with no
        // difference.
        assert!(
            skills_for_class_at("resonant", 255).len() > menu_width(Archetype::Martial),
            "a caster's menu is no wider than a martial one"
        );
    }

    #[test]
    fn abilities_are_spaced_out_to_about_a_hundred_not_bunched_under_ten() {
        // The point of the ladder is that levelling keeps paying. A kit whose last
        // ability lands at level 5 stops mattering at level 5.
        //
        // EVERY class, read off the registry — a hand-written list is a list that a
        // new class is simply left off, and the Smithwright and the Keeper were: both
        // shipped on 1/4/12/20/28/36 while the rule below says squares, and nothing
        // failed because neither was named here.
        for class in all_classes() {
            let class = class.as_str();
            let kit = skills_for_class(class);
            let mut levels: Vec<i32> = kit.iter().map(|s| s.unlock).collect();
            levels.sort();
            assert_eq!(levels[0], 1, "{class} can do nothing at level 1");
            // Every unlock sits on a shared rung, so a player counting to their next
            // ability counts in tens rather than in squares.
            for lv in &levels {
                assert!(RUNGS.contains(lv), "{class}: level {lv} is not a rung ({RUNGS:?})");
            }
            assert!(*levels.last().unwrap() <= 255, "{class} reaches past the level cap");
            // And the kit is HANDED OUT in ladder order, so the abilities panel and
            // the battle menu read as a ladder rather than as table order.
            let shown: Vec<i32> = kit.iter().map(|s| s.unlock).collect();
            assert_eq!(shown, levels, "{class}'s kit is listed out of ladder order");
        }
        // Every class in the registry is covered, not a hand-picked few.
        assert_eq!(all_classes().len(), 8, "{:?}", all_classes());
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
