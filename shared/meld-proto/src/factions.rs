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

/// Do two creature factions dislike each other (overworld skirmishing)?
pub fn creatures_hostile(a: &str, b: &str) -> bool {
    if a == b {
        return false;
    }
    HOSTILE_PAIRS
        .iter()
        .any(|(x, y)| (*x == a && *y == b) || (*x == b && *y == a))
}

/// Do two **battle** factions target each other? The player fights all creatures
/// (and vice-versa); otherwise fall back to creature hostility.
pub fn battle_hostile(a: &str, b: &str) -> bool {
    if a == PLAYER || b == PLAYER {
        return a != b; // player vs any creature (but not player vs player)
    }
    creatures_hostile(a, b)
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

    #[test]
    fn same_faction_is_friendly_hostile_pairs_are_not() {
        assert!(!creatures_hostile("beast", "beast"));
        assert!(creatures_hostile("beast", "fiend"));
        assert!(creatures_hostile("fiend", "beast")); // symmetric
        assert!(!creatures_hostile("beast", "construct")); // not a listed pair
    }
}
