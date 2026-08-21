//! How many PARTIES an encounter is sized for, and what it is called (`FS-4`).
//!
//! A gatekeeper's HP multiplier has always been commented "sized to the party over a merge"
//! — `merge_cap_gatekeeper_instances` is 4, so four parties on a door boss has been the
//! intent since it was written. What it never did was **say so**: a solo party walked into a
//! wall sized for four, ground it for 464 measured turns, and had nothing on screen to
//! suggest the fight was not meant for them.
//!
//! So the scale becomes a declared property with a NAME, in the spirit of the champion
//! affixes that already prefix an elite's title: a **Colossus** Ironmaw is one sized for two
//! parties, and it says so before you are close enough to touch it.
//!
//! One registry, read by the server that scales and names the spawn and the client that draws
//! the plate — a title copied to the far side of the wire is a title that goes stale.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Warband {
    pub parties: u8,
    /// Empty at one party: an ordinary fight wearing a label teaches players to ignore
    /// labels, which is what would make the raid ones useless.
    pub title: &'static str,
    /// The title alone is flavour; this is the half that tells a player what it will cost.
    pub warning: &'static str,
}

/// Capped at four because `[ai] merge_cap_gatekeeper_instances` is four — a boss sized for
/// more parties than may legally merge onto it is one nobody can bring enough people to.
pub const WARBANDS: &[Warband] = &[
    Warband { parties: 1, title: "", warning: "" },
    Warband {
        parties: 2,
        title: "Colossus",
        warning: "Sized for TWO parties. Bring help.",
    },
    Warband {
        parties: 3,
        title: "Leviathan",
        warning: "Sized for THREE parties. You cannot out-last this alone.",
    },
    Warband {
        parties: 4,
        title: "Worldbreaker",
        warning: "Sized for FOUR parties - the most that may stand together.",
    },
];

/// Never `None`: an out-of-range count reads as the nearest real tier rather than silently
/// losing its label, and the label is the whole point of this module.
pub fn warband(parties: u8) -> &'static Warband {
    let want = parties.clamp(1, max_parties());
    WARBANDS.iter().find(|w| w.parties == want).unwrap_or(&WARBANDS[0])
}

pub fn max_parties() -> u8 {
    WARBANDS.iter().map(|w| w.parties).max().unwrap_or(1)
}

pub fn title(parties: u8) -> &'static str {
    warband(parties).title
}

pub fn is_raid(parties: u8) -> bool {
    parties > 1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_raid_tier_names_itself_and_says_what_it_costs() {
        for w in WARBANDS {
            if w.parties == 1 {
                assert!(w.title.is_empty() && w.warning.is_empty(), "an ordinary fight is unlabelled");
                continue;
            }
            assert!(!w.title.is_empty(), "{} parties has no title", w.parties);
            assert!(!w.warning.is_empty(), "{} has no warning line", w.title);
            // Both halves have to fit a plate floating over a creature.
            assert!(w.title.len() <= 14, "{} is too long for a nameplate", w.title);
            assert!(w.warning.len() <= 70, "{}'s warning will not fit the plate", w.title);
            assert!(is_raid(w.parties));
        }
    }

    /// Contiguous from 1, so a tier can never silently vanish into a gap.
    #[test]
    fn the_ladder_is_contiguous_and_capped_at_the_merge_limit() {
        for (i, w) in WARBANDS.iter().enumerate() {
            assert_eq!(w.parties as usize, i + 1, "the ladder skips a party count");
        }
        assert_eq!(max_parties(), 4, "the ladder must not outgrow the merge cap");
    }

    #[test]
    fn an_out_of_range_count_still_reads_as_a_tier() {
        assert_eq!(warband(0).parties, 1);
        assert_eq!(warband(9).parties, 4);
        assert_eq!(title(0), "");
        assert_eq!(title(2), "Colossus");
        assert!(!is_raid(1));
    }
}
