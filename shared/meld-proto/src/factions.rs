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
///
/// ⚠️ It absorbed the old `shade` lineage, which was one creature (`sand_shade`) and a
/// word nothing in the fiction ever defined. A shade IS one of the risen, so the fold is
/// the fiction catching up with itself — but it has a MECHANICAL consequence worth
/// knowing: the Phoenix Guard's `undead_bane` and its whole silvered kit now bite the
/// desert's shades, because this string is what that bonus tests.
pub const UNDEAD: &str = "undead";

/// The old wild things — a briar court that predates the Last City and does not
/// recognise it. Named for the same reason [`UNDEAD`] is: it is the one lineage that
/// appears ONLY behind a dungeon door, never wandering the overworld, so anything
/// reasoning about what a player can meet in the open has to be able to say so.
pub const FAE: &str = "fae";

/// Dragon lineage — wyverns, serpents, the leviathan. Renamed from `wyrm`, which read as
/// one body plan rather than a bloodline and left no room for anything winged.
pub const DRACONIC: &str = "draconic";

/// The ooze. Its appetite is the point: a slime is at war with everything that LIVES and
/// with nothing else, so it is the one lineage that makes a three-way fight likely
/// wherever it stands. See [`HOSTILE_PAIRS`].
pub const SLIME: &str = "slime";

/// Every creature lineage. One list, so a new faction cannot be added to the hostility
/// table and then forgotten by everything that enumerates lineages.
pub const FACTIONS: &[&str] =
    &["beast", "construct", DRACONIC, FAE, "fiend", "fungal", SLIME, UNDEAD];

/// Unordered creature-faction pairs that don't get along. Tuned so **every** biome
/// roster (`creatures_for_biome`) pairs two mutually-hostile factions, so overworld
/// skirmishes are visible everywhere — not just tundra/mire.
const HOSTILE_PAIRS: &[(&str, &str)] = &[
    ("beast", "fiend"),
    ("beast", "undead"),    // tundra: frost_lurker vs ice_revenant
    ("beast", "fungal"),    // forest: thornback_boar vs forest_bloom_stalker
    ("construct", "fungal"),
    ("draconic", "fungal"), // mire: bog_serpent vs myconid
    ("draconic", "undead"), // desert: dune_wyrm vs sand_shade
    ("fiend", "construct"), // ashfall: cinder_imp vs magma_golem
    ("fiend", "fungal"),    // mire: bog_stinger vs myconid
    // THE BRIAR COURT hates what is made of appetite, what is made of rot, and what
    // refuses to stay buried. It does NOT hate beasts: a fae court and the animals of
    // its wood are the same side, which is what lets it hold a barrow without its own
    // ground turning on it.
    ("fae", "fiend"),
    ("fae", "fungal"),
    ("fae", "undead"),
    // THE OOZE EATS WHAT IS ALIVE, AND ONLY THAT. Its two exemptions are `construct` and
    // `undead` — worked iron and dry bone are equally not food — and they are the whole
    // character of the lineage rather than an oversight: a slime among golems or among
    // the risen is the only place it stands quietly, so those are the pairings that
    // build a den which does not eat itself.
    ("slime", "beast"),
    ("slime", "draconic"),
    ("slime", "fae"),
    ("slime", "fiend"),
    ("slime", "fungal"),
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

    /// EVERY LINEAGE HAS AT LEAST ONE ENEMY, and every pair names a real lineage.
    ///
    /// A faction with nobody to fight never appears in a turf war (`CR-2`), which is
    /// invisible rather than loud — it looks like the world simply being quiet. And a
    /// pair naming a faction that no longer exists is a rule that silently does nothing:
    /// exactly what the retired `shade` entries would have become when that lineage
    /// folded into `undead`.
    #[test]
    fn every_lineage_has_an_enemy_and_every_pair_names_a_real_one() {
        for (a, b) in HOSTILE_PAIRS {
            assert!(FACTIONS.contains(a), "{a} is in a hostility pair but is not a lineage");
            assert!(FACTIONS.contains(b), "{b} is in a hostility pair but is not a lineage");
            assert_ne!(a, b, "a lineage cannot be hostile to itself - like does not fight like");
        }
        for f in FACTIONS {
            assert!(
                FACTIONS.iter().any(|o| creatures_hostile(f, o)),
                "{f} is hostile to nothing, so it can never appear in a turf war"
            );
        }
        for (i, (a, b)) in HOSTILE_PAIRS.iter().enumerate() {
            for (c, d) in &HOSTILE_PAIRS[i + 1..] {
                assert!(!((a == c && b == d) || (a == d && b == c)), "{a}/{b} is listed twice");
            }
        }
    }

    /// THE OOZE EATS WHAT IS ALIVE, AND ONLY THAT. Its two exemptions are the lineage's
    /// whole character — a slime among golems or among the risen is the one place it
    /// stands quietly — so they are asserted rather than left to be read off the table.
    #[test]
    fn a_slime_eats_the_living_and_leaves_iron_and_bone_alone() {
        const NOT_FOOD: [&str; 2] = ["construct", UNDEAD];
        for f in NOT_FOOD {
            assert!(!creatures_hostile(SLIME, f), "{f} is not food for an ooze");
        }
        for f in FACTIONS {
            if *f == SLIME || NOT_FOOD.contains(f) {
                continue;
            }
            assert!(creatures_hostile(SLIME, f), "a slime should be at war with {f}");
        }
    }

    /// The `shade` lineage is GONE, not merely unused, and `wyrm` is renamed. A stale
    /// name in the table is a rule that quietly stops applying to anything.
    #[test]
    fn the_shade_lineage_is_folded_into_the_risen() {
        assert!(!FACTIONS.contains(&"shade"));
        assert!(!FACTIONS.contains(&"wyrm"), "wyrm was renamed to draconic");
        assert!(FACTIONS.contains(&DRACONIC));
        for f in FACTIONS {
            assert!(!creatures_hostile("shade", f));
            assert!(!creatures_hostile("wyrm", f));
        }
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
