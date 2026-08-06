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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Insurance {
    /// Blue-Chest: comes home with you, degrading on death.
    #[serde(alias = "blue")]
    Insured,
    /// Red-Chest: vanishes when the run ends, win or lose.
    #[serde(alias = "red")]
    Ephemeral,
}

impl Insurance {
    /// The player-facing label.
    pub fn label(self) -> &'static str {
        match self {
            Insurance::Insured => "Insured",
            Insurance::Ephemeral => "Ephemeral",
        }
    }

    /// The canonical wire word.
    pub fn wire(self) -> &'static str {
        match self {
            Insurance::Insured => "insured",
            Insurance::Ephemeral => "ephemeral",
        }
    }

    /// Parse either the canonical word or the stored chest colour.
    pub fn from_wire(s: &str) -> Option<Self> {
        Some(match s {
            "insured" | "blue" => Insurance::Insured,
            "ephemeral" | "red" => Insurance::Ephemeral,
            _ => return None,
        })
    }

    /// The one sentence a player must be able to read before they risk the item.
    pub fn tooltip(self) -> &'static str {
        match self {
            Insurance::Insured => "Comes home with you. Degrades on death.",
            Insurance::Ephemeral => "Vanishes when the run ends - win or lose. Use it now.",
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
