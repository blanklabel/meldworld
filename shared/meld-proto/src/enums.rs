//! Canonical enums (CANON.md §G glossary). Wire form is snake_case (CANON.md §I).

use serde::{Deserialize, Serialize};

/// Character classes (CANON.md §G `CharacterClass`, D9). `explorer` is the default —
/// the martial baseline that builds Adrenaline with basic attacks and spends it on
/// its skills (see `Battle::resolve_skill`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CharacterClass {
    /// The Explorers: mapping and reclaiming the unstable world, and the only order
    /// that can set Anchors (docs/lore/factions.md). The class a new account starts
    /// with. Currently fights with the martial kit; its own tempo/stability kit is
    /// designed but not yet built.
    Explorer,
    /// The Hunters' guild: disposal of dangerous non-civilian creatures — the game's
    /// core loop, so this is the martial baseline. Basic attacks bank Adrenaline;
    /// every skill (Power Strike, Second Wind, Snare, Frenzy) spends it.
    Hunter,
    Dragoon,
    Sage,
    Ranger,
    AlchemistKnight,
    Bard,
    /// Psychic controller: armour-ignoring psychic strikes + projected wards.
    Psyker,
    /// Healer: spends its own HP to mend allies, grants Regen + Barrier.
    Resonant,
    /// Rogue / fortune-explorer ("Runner"): fast, fragile, evasive. Armour-piercing
    /// Backstab, a Flicker evasion blink, and Ransack (damage + ATB-gauge steal).
    Shifter,
    /// Order of the Phoenix Guard monk: a dense, slow front-line tank. Blunt
    /// kinetic strikes that stagger (drain the enemy's ATB gauge), a Root stance
    /// that grants Barrier, and Toll of the Deep — an all-enemy shockwave. The
    /// order walks out of fires nothing else survives, which is why the class is
    /// earned by surviving the undead rite.
    ///
    /// The `iron_hull` key is deliberately NOT aliased here: the Order of the Iron
    /// Hull is a separate monastic order whose own kit is already authored
    /// (docs/lore/factions.md), and it will claim that key when it lands.
    PhoenixGuard,
    /// **The Foundry's Smithwright.** The order that keeps the world standing — walls,
    /// Anchors, and every blade the rest of it swings. A front-line class built around
    /// what it CARRIES: a heavy hammer, a bulwark it plants, and the field forge it
    /// raises (MS-1). A Smithwright in the party makes the anvil's rhythm easier for
    /// everyone at it, because there are more hands on the piece.
    Smithwright,
    /// **The Order of the Open Flower's Keeper.** Growers and menders. A support class
    /// that keeps a party alive between fights rather than during them: reagents,
    /// tonics, and the still it sets up in the field (MS-1). A Keeper in the party makes
    /// a cook easier the way a Smithwright makes a heat easier.
    Keeper,
}

/// Damage typing (Creature AI/Combat/Gear spec §1). Every damaging effect
/// carries one of these, or [`DamageType::None`] for pure/true damage that no
/// weakness, resistance, immunity, or absorption touches. Wire form is
/// UPPERCASE (the spec's `damage_modifiers` maps are keyed `"FIRE"`, `"ICE"`, …).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum DamageType {
    // Physical.
    Blunt,
    Slash,
    Pierce,
    // Elemental / magical.
    Water,
    Ice,
    Fire,
    Lightning,
    Wind,
    Earth,
    Poison,
    Infernal,
    Celestial,
    Shadow,
    Mind,
    Ethereal,
    /// Pure/true damage — bypasses the modifier map entirely.
    None,
}

impl DamageType {
    /// Whether this is a PHYSICAL type — a blow that has to reach across the rank.
    ///
    /// The front/back row trade is scoped to these: standing back halves what a physical
    /// blow does to you and what your own physical blows do in return. Everything else —
    /// a spell, a Focus, a creature's elemental breath — reaches the back rank at full
    /// force and is unaffected by where its caster stands, which is what makes a caster's
    /// natural home the back row and gives a martial class a real reason to hold the front.
    pub fn is_physical(self) -> bool {
        matches!(self, DamageType::Blunt | DamageType::Slash | DamageType::Pierce)
    }

    /// Parse the UPPERCASE wire key ("FIRE") used in `damage_modifiers` maps.
    pub fn from_wire(key: &str) -> Option<DamageType> {
        Some(match key {
            "BLUNT" => DamageType::Blunt,
            "SLASH" => DamageType::Slash,
            "PIERCE" => DamageType::Pierce,
            "WATER" => DamageType::Water,
            "ICE" => DamageType::Ice,
            "FIRE" => DamageType::Fire,
            "LIGHTNING" => DamageType::Lightning,
            "WIND" => DamageType::Wind,
            "EARTH" => DamageType::Earth,
            "POISON" => DamageType::Poison,
            "INFERNAL" => DamageType::Infernal,
            "CELESTIAL" => DamageType::Celestial,
            "SHADOW" => DamageType::Shadow,
            "MIND" => DamageType::Mind,
            "ETHEREAL" => DamageType::Ethereal,
            "NONE" => DamageType::None,
            _ => return None,
        })
    }

    /// The UPPERCASE wire key for this type — [`from_wire`](DamageType::from_wire)'s
    /// inverse, held against it by `damage_type_wire_keys_round_trip` so a new variant
    /// cannot be added to one direction only.
    pub fn to_wire(self) -> &'static str {
        match self {
            DamageType::Blunt => "BLUNT",
            DamageType::Slash => "SLASH",
            DamageType::Pierce => "PIERCE",
            DamageType::Water => "WATER",
            DamageType::Ice => "ICE",
            DamageType::Fire => "FIRE",
            DamageType::Lightning => "LIGHTNING",
            DamageType::Wind => "WIND",
            DamageType::Earth => "EARTH",
            DamageType::Poison => "POISON",
            DamageType::Infernal => "INFERNAL",
            DamageType::Celestial => "CELESTIAL",
            DamageType::Shadow => "SHADOW",
            DamageType::Mind => "MIND",
            DamageType::Ethereal => "ETHEREAL",
            DamageType::None => "NONE",
        }
    }

    /// Every variant, so a rule about damage types can be held against the whole set
    /// rather than a hand-written list that a new element gets left off.
    pub fn all() -> &'static [DamageType] {
        &[
            DamageType::Blunt,
            DamageType::Slash,
            DamageType::Pierce,
            DamageType::Water,
            DamageType::Ice,
            DamageType::Fire,
            DamageType::Lightning,
            DamageType::Wind,
            DamageType::Earth,
            DamageType::Poison,
            DamageType::Infernal,
            DamageType::Celestial,
            DamageType::Shadow,
            DamageType::Mind,
            DamageType::Ethereal,
            DamageType::None,
        ]
    }
}

/// How the target's `damage_modifiers` bent a resolved damage effect
/// (spec §2 step 5) — appended per effect on `battle.action_resolved` so the
/// client can render WEAK!/RESIST!/IMMUNE!/ABSORB! feedback.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModifierFlag {
    Weak,
    Resist,
    Immune,
    Absorb,
    Normal,
}

/// A combatant's category, deciding friend-vs-foe and disconnect rules.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CombatantKind {
    Player,
    Monster,
    GatekeeperBoss,
}

/// Encounter classification (realtime battle.md). Drives flee + disconnect rules.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EncounterClass {
    Standard,
    Elite,
    Gatekeeper,
}

/// A creature's role in its encounter (CR-6 packs). Drives pack AI: a leader is
/// shielded by its living minions and buffs them, and its death routs them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PackRole {
    /// Not part of a pack (a lone creature, an elite, a gatekeeper, a hero).
    #[default]
    None,
    /// The big one the pack forms around.
    Leader,
    /// One of the littles.
    Minion,
}

impl PackRole {
    pub fn from_encounter_class(class: &str) -> PackRole {
        match class {
            "leader" => PackRole::Leader,
            "minion" => PackRole::Minion,
            _ => PackRole::None,
        }
    }
}

/// Gear insurance tier (CANON.md §G). The Blue-Chest / Red-Chest *fiction* stays
/// in canon; the enum and every player-facing string say what the tier actually
/// does, because "red" is not something a player can decode (GR-6). The `blue` /
/// `red` wire aliases keep older payloads parsing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Insurance {
    /// Blue-Chest: yours forever, but always dying. Survives a wipe and wears down a
    /// little each time until it finally breaks.
    #[serde(alias = "blue")]
    Insured,
    /// Red-Chest: burns the moment you reach the Last City, whether you walked in or
    /// were carried. Power you can only spend inside the run that found it — which is
    /// why it is the strongest gear in the game.
    #[serde(alias = "red")]
    Ephemeral,
    /// The ordinary kind. Never degrades and is yours to keep — until a run ends in a
    /// wipe nobody could reverse, and then all of it is gone at once.
    ///
    /// Normal and Insured trade the SHAPE of loss rather than the amount: normal gear
    /// is untouched right up until one bad night takes the lot, insured gear can never
    /// be taken but is never quite whole.
    #[serde(alias = "normal")]
    Standard,
}

impl Insurance {
    /// The player-facing label.
    pub fn label(self) -> &'static str {
        match self {
            Insurance::Insured => "Insured",
            Insurance::Ephemeral => "Ephemeral",
            Insurance::Standard => "Standard",
        }
    }

    /// The canonical wire word.
    pub fn wire(self) -> &'static str {
        match self {
            Insurance::Insured => "insured",
            Insurance::Ephemeral => "ephemeral",
            Insurance::Standard => "standard",
        }
    }

    /// Parse either the canonical word or the stored chest colour.
    pub fn from_wire(s: &str) -> Option<Self> {
        Some(match s {
            "insured" | "blue" => Insurance::Insured,
            "ephemeral" | "red" => Insurance::Ephemeral,
            "standard" | "normal" => Insurance::Standard,
            _ => return None,
        })
    }

    /// The one sentence a player must be able to read before they risk the item.
    pub fn tooltip(self) -> &'static str {
        match self {
            Insurance::Insured => "Survives a wipe. Wears down each time, until it breaks.",
            Insurance::Ephemeral => "Burns the moment you reach the city - win or lose. Use it now.",
            Insurance::Standard => "Yours to keep. Lost only if a run ends in a wipe.",
        }
    }
}

/// A battle action a player may submit (realtime battle.md).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BattleActionKind {
    Attack,
    Skill,
    Item,
    Defend,
    Flee,
}

/// Terminal result of one combatant's battle (realtime battle.md).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BattleOutcome {
    Victory,
    Defeat,
    Fled,
}

/// The kind of a per-target effect inside `battle.action_resolved`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EffectKind {
    Damage,
    Heal,
    StatusApplied,
    StatusRemoved,
    Ko,
    Revive,
}

/// Terminal state of a `Run` (CANON.md §G).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunResult {
    Extracted,
    Died,
    Abandoned,
}

/// Realtime rejection codes (realtime-protocol.md common rejection table).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    ValidationError,
    Unauthorized,
    Forbidden,
    NotFound,
    InvalidState,
    OutOfRange,
    DuplicateAction,
    SequenceError,
    ResumeFailed,
    RateLimitExceeded,
    Internal,
}

/// Reason a server-initiated `session.terminated` closes a socket.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TerminateReason {
    ReplacedByNewConnection,
    AuthTimeout,
    IdleTimeout,
    ServerShutdown,
    ProtocolViolation,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The Wall Defense Force's Rift Drop-Trooper (docs/lore/factions.md) has an
    /// authored ladder and no kit, so `rift_knight` is a NAME the lore has claimed and
    /// the enum has not. Same reservation as `iron_hull` above, for the same reason: a
    /// key that silently deserialises to some other class is a hero who wakes up as the
    /// wrong thing the day the real one lands.
    #[test]
    fn rift_knight_is_reserved_and_not_yet_fieldable() {
        assert!(serde_json::from_str::<CharacterClass>("\"rift_knight\"").is_err());
        assert_eq!(crate::equipment::class_from_key("rift_knight"), None);
    }

    #[test]
    fn iron_hull_is_reserved_for_its_own_order_not_an_alias() {
        // The Order of the Iron Hull is a separate monastic order with its own
        // authored kit (docs/lore/factions.md). If `iron_hull` still deserialised to
        // the Phoenix Guard, the day that class lands every one of its heroes would
        // silently be the wrong class.
        assert!(serde_json::from_str::<CharacterClass>("\"iron_hull\"").is_err());
        assert_eq!(crate::equipment::class_from_key("iron_hull"), None);
        assert_eq!(
            serde_json::to_string(&CharacterClass::PhoenixGuard).unwrap(),
            "\"phoenix_guard\""
        );
        assert_eq!(crate::equipment::class_key(CharacterClass::PhoenixGuard), "phoenix_guard");
        // And the Hunter, reintroduced, round-trips on its own key.
        assert_eq!(
            serde_json::from_str::<CharacterClass>("\"hunter\"").unwrap(),
            CharacterClass::Hunter
        );
        assert_eq!(crate::equipment::class_key(CharacterClass::Hunter), "hunter");
    }
}

/// How a creature picks who to hit (CR-9). One rule for every creature made every fight
/// read the same: the pack always went for the lowest HP, so a party learned one lesson and
/// it held from the hub to the deep. A profile is set per creature KIND, upgraded by
/// encounter class (an Elite or a Gatekeeper is smarter than the trash around it), and
/// rolled toward the smarter end as creature level climbs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TargetProfile {
    /// Finish the wounded — the original behaviour, and still the most common.
    Weakest,
    /// No pattern at all. Unpredictable rather than stupid: you cannot plan around it.
    Random,
    /// Hunt the back rank on purpose. The counter to hiding your casters behind a wall.
    Backline,
    /// Go for the ROLE that makes the party work — the healer first, then the casters.
    Role,
    /// Converge: the whole pack shares one mark and commits to it, and says so out loud.
    GangUp,
}

impl TargetProfile {
    /// Whether this profile reads the party rather than the HP bars. Used to decide which
    /// creatures are "smart" when the level roll upgrades them.
    pub fn is_tactical(self) -> bool {
        matches!(self, TargetProfile::Backline | TargetProfile::Role | TargetProfile::GangUp)
    }
}
