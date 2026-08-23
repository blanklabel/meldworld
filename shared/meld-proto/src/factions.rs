//! Creature factions and their relationships (content; structural). Shared by
//! the world (overworld grouping/movement) and the battle engine (targeting) so
//! both agree on who gets along with whom.
//!
//! Rules of thumb the rest of the code relies on:
//! - The **player** faction (`"player"`) is hostile to every creature, and every
//!   creature is hostile to the player — you're always the intruder.
//! - Two creatures of the **same** faction never fight each other (they gang up).
//! - Two creatures of **different** factions fight only if the pair is in the
//!   hostility table below.

/// The player's battle faction.
pub const PLAYER: &str = "player";

/// The risen. Named because it is not just another roster entry: the Phoenix Guard
/// exists to eradicate it, so the engine checks this exact string when applying
/// their standing bonus (docs/lore/factions.md).
pub const UNDEAD: &str = "undead";

/// Unordered creature-faction pairs that don't get along. Tuned so **every**
/// biome roster (`creatures_for_biome`) pairs two mutually-hostile factions, so
/// overworld skirmishes are visible everywhere — not just tundra/mire.
const HOSTILE_PAIRS: &[(&str, &str)] = &[
    ("beast", "fiend"),
    ("beast", "undead"),  // tundra: frost_lurker vs ice_revenant
    ("beast", "fungal"),  // forest: thornback_boar vs forest_bloom_stalker
    ("construct", "fungal"),
    ("wyrm", "fungal"),   // mire: bog_serpent vs myconid_brute
    ("wyrm", "shade"),    // desert: dune_wyrm vs sand_shade
    ("fiend", "construct"), // ashfall: cinder_imp vs magma_golem
    ("shade", "beast"),
];

/// Do two creature FACTIONS dislike each other?
///
/// Faction-level only. Prefer [`creatures_at_odds`], which also knows that like does not
/// fight like — this is the half of the rule that cannot see species.
pub fn creatures_hostile(a: &str, b: &str) -> bool {
    if a == b {
        return false;
    }
    HOSTILE_PAIRS
        .iter()
        .any(|(x, y)| (*x == a && *y == b) || (*x == b && *y == a))
}

/// **LIKE DOES NOT FIGHT LIKE.** Do these two creatures actually go for each other?
///
/// The faction table alone is not enough, because a creature's faction is not a property
/// of its species — it is handed down by whatever pack promoted it. `become_boss` gives a
/// rite's retinue the boss's own lineage ("the dead are undead whatever host they rode in
/// on"), and `join_pack` hands a leader's faction to its minions. Measured across three
/// seeded worlds: **every one of the 15 species holds more than one faction, and every one
/// of them can hold two factions that are hostile to each other.** So a `thornback_boar`
/// conscripted into a construct-led pack would hunt an ordinary `thornback_boar` standing
/// next to it — two of the same animal, tearing at each other for no reason a player could
/// ever read.
///
/// Species is checked FIRST and unconditionally. An empty kind (a hero, a fighter with no
/// species) never matches, so this cannot accidentally pacify anything.
///
/// ⚠️ Note the one arguable exception, deliberately NOT carved out: a risen boar and a
/// living boar are the same species and so will not fight, even though the setting might
/// say the risen one is no longer the same animal. Adding `UNDEAD` as an exception is a
/// one-line change here if that reads wrong in play — but it belongs in ONE place, and
/// this is the place.
pub fn creatures_at_odds(a_faction: &str, a_kind: &str, b_faction: &str, b_kind: &str) -> bool {
    if !a_kind.is_empty() && a_kind == b_kind {
        return false;
    }
    creatures_hostile(a_faction, b_faction)
}

/// Do two **battle** factions target each other? The player fights all creatures
/// (and vice-versa); otherwise fall back to creature hostility.
///
/// Faction-level only — prefer [`battle_at_odds`], which also knows about species.
pub fn battle_hostile(a: &str, b: &str) -> bool {
    if a == PLAYER || b == PLAYER {
        return a != b; // player vs any creature (but not player vs player)
    }
    creatures_hostile(a, b)
}

/// The battle-side [`creatures_at_odds`]: who a fighter will swing at.
///
/// The same rule as the overworld, through the same function, because a pack that walks
/// past each other outside and then tears itself apart the moment a fight starts is the
/// worst of both — and "one rule, two call sites" is the drift this repo has been bitten
/// by more than once.
pub fn battle_at_odds(a_faction: &str, a_kind: &str, b_faction: &str, b_kind: &str) -> bool {
    if a_faction == PLAYER || b_faction == PLAYER {
        return a_faction != b_faction;
    }
    creatures_at_odds(a_faction, a_kind, b_faction, b_kind)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn player_fights_creatures_not_itself() {
        assert!(battle_hostile("player", "beast"));
        assert!(battle_hostile("fungal", "player"));
        assert!(!battle_hostile("player", "player"));
    }

    /// LIKE DOES NOT FIGHT LIKE, whatever pack promoted it.
    ///
    /// A creature's faction is not a property of its species — `become_boss` gives a rite's
    /// retinue the boss's lineage and `join_pack` conscripts a hostile minion — so the same
    /// animal can stand in the world under two factions that the table says are enemies.
    /// Measured across three seeded worlds before this rule: **all 15 species held more than
    /// one faction, and every one of them held two that were hostile to each other**, so two
    /// `thornback_boar` could hunt each other for no reason a player could read.
    #[test]
    fn a_creature_never_fights_its_own_species() {
        // The exact case that was live: a boar raised into a rite, and a boar that was not.
        assert!(creatures_hostile("beast", "undead"), "the premise: those factions ARE enemies");
        assert!(
            !creatures_at_odds("beast", "thornback_boar", "undead", "thornback_boar"),
            "two of the same animal went for each other because a pack relabelled one"
        );
        // …and it holds in the fight, through the same rule.
        assert!(!battle_at_odds("beast", "thornback_boar", "undead", "thornback_boar"));

        // It must not pacify anything else: DIFFERENT species of hostile factions still fight,
        // or `CR-2`'s turf war quietly stops existing.
        assert!(creatures_at_odds("beast", "thornback_boar", "undead", "ice_revenant"));
        assert!(battle_at_odds("beast", "thornback_boar", "fungal", "sporeling"));

        // An empty kind never matches an empty kind — a hero and a creature are not "the
        // same species" just because neither carries one.
        assert!(battle_at_odds(PLAYER, "", "beast", "thornback_boar"));
        assert!(creatures_at_odds("beast", "", "undead", ""));
    }

    /// NON-HOSTILE FACTIONS CAN SHARE A PACK AND KEEP THEIR OWN NAMES.
    ///
    /// The faction table is an allow-list of who dislikes whom, not "different means
    /// enemies" — `beast` and `construct` are simply not a pair. So a mixed pack of the two
    /// is a real thing the world can field, and neither has to be relabelled to make it work.
    #[test]
    fn factions_that_are_not_enemies_can_run_together() {
        assert!(!creatures_hostile("beast", "construct"));
        assert!(!creatures_at_odds("beast", "thornback_boar", "construct", "magma_golem"));
        assert!(!battle_at_odds("beast", "thornback_boar", "construct", "magma_golem"));
    }

    #[test]
    fn same_faction_is_friendly_hostile_pairs_are_not() {
        assert!(!creatures_hostile("beast", "beast"));
        assert!(creatures_hostile("beast", "fiend"));
        assert!(creatures_hostile("fiend", "beast")); // symmetric
        assert!(!creatures_hostile("beast", "construct")); // not a listed pair
    }
}
