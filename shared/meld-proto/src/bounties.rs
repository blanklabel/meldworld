//! Bounties: generated contracts from the Den, each one a named boss with your name on
//! it (roadmap `AD-4`; behaviour: [`docs/behaviors/hunt-board.md`]).
//!
//! Where a **hunt** is posted for everyone and stands forever
//! ([`crate::hunts`]), a bounty is **yours**: rolled for your **hunter rank**, sighted at
//! a depth that rank has earned, standing in the world for **you alone**, and gone after
//! a while whether or not you went. Finishing one raises the rank, which rolls the next
//! one harder — the ladder the fixed board deliberately does not have.
//!
//! **A bounty always ends in a boss fight.** The mark is one of the named bosses
//! (`meld-world`'s `boss_display_name`) wearing a rolled **epithet**, promoted past a
//! Gatekeeper by rank. There is no "kill eight of these" bounty; that is what hunts are
//! for.
//!
//! This module owns the *shape* and the words. The **roll** lives in `meld-world`, which
//! is where the creature and biome tables are, and every magnitude in it comes from
//! `[bounty]` in `balance.toml`.

use serde::{Deserialize, Serialize};

/// Where the mark will be found.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Venue {
    /// Standing in the open at its sighted depth.
    Overworld,
    /// Waiting at the bottom of a descent, in place of what usually keeps the door.
    Dungeon,
}

impl Venue {
    pub fn wire(&self) -> &'static str {
        match self {
            Venue::Overworld => "overworld",
            Venue::Dungeon => "dungeon",
        }
    }

    /// How the board words where to go.
    pub fn phrasing(&self) -> &'static str {
        match self {
            Venue::Overworld => "in the open",
            Venue::Dungeon => "at the bottom of a descent",
        }
    }
}

/// One rolled contract. Every number here was **drawn at generation time** against
/// `[bounty]` and then stored, so a retune changes the next bounty rather than silently
/// rewriting one a player is already working.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BountySpec {
    /// Which named boss the mark fights as (`meld_world::boss_display_name`).
    pub boss_kind: String,
    /// What makes this one *this* one — "the Thrice-Broken".
    pub epithet: String,
    /// The species it wears, so the sprite and the biome agree.
    pub creature: String,
    pub biome: String,
    /// Where it has been sighted (floored distance).
    pub distance: i32,
    pub venue: Venue,
    /// The hunter rank this was rolled for.
    pub rank: i32,
    /// Multiplier on the mark's HP and attack, over a standard creature at that depth.
    pub power: f64,
    pub reward_chits: i64,
    #[serde(default)]
    pub reward_material: String,
    #[serde(default)]
    pub reward_material_qty: i32,
    #[serde(default)]
    pub reward_gear: bool,
    /// Hunter XP finishing it banks.
    pub reward_rank_xp: i64,
}

impl BountySpec {
    /// The mark's full name, as everything from the board to the battle header shows it.
    pub fn mark_name(&self, boss_display: &str) -> String {
        if self.epithet.is_empty() {
            boss_display.to_string()
        } else {
            format!("{boss_display} {}", self.epithet)
        }
    }
}

/// What a bounty is doing right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BountyState {
    /// Standing: the mark is out there and has not been put down.
    Active,
    /// Felled, reward not yet taken.
    Completed,
    /// Felled and paid.
    Claimed,
    /// Its window closed with the mark still standing.
    Expired,
}

impl BountyState {
    pub fn wire(&self) -> &'static str {
        match self {
            BountyState::Active => "active",
            BountyState::Completed => "completed",
            BountyState::Claimed => "claimed",
            BountyState::Expired => "expired",
        }
    }

    pub fn from_wire(s: &str) -> Option<Self> {
        Some(match s {
            "active" => BountyState::Active,
            "completed" => BountyState::Completed,
            "claimed" => BountyState::Claimed,
            "expired" => BountyState::Expired,
            _ => return None,
        })
    }

    /// Whether the mark should be standing in the world for its owner. A felled mark must
    /// not reappear between the kill and the walk home.
    pub fn is_standing(&self) -> bool {
        matches!(self, BountyState::Active)
    }
}

/// Epithets a mark can be rolled with. Content: they say only that this creature has a
/// history, which is the whole job — a mark called "Ironmaw" twice is two encounters with
/// the same monster, and "Ironmaw the Thrice-Broken" is a story.
pub const EPITHETS: &[&str] = &[
    "the Thrice-Broken",
    "of the Long Silence",
    "the Unburied",
    "Who Waits",
    "the Hollow Crown",
    "of Nine Wounds",
    "the Sundered",
    "That Came Back",
    "of the Drowned Choir",
    "the Last Warden",
];

/// The word a rank is worn as. Purely a title — it gates nothing, exactly like the orders'
/// rank ladders.
pub fn rank_title(rank: i32) -> &'static str {
    match rank.max(0) {
        0..=1 => "Unblooded",
        2..=4 => "Tracker",
        5..=9 => "Marksworn",
        10..=19 => "Houndmaster",
        20..=39 => "Reaver",
        _ => "Apex",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_mark_wears_its_epithet_and_survives_not_having_one() {
        let mut spec = BountySpec {
            boss_kind: "ironmaw".into(),
            epithet: "the Unburied".into(),
            creature: "dune_wyrm".into(),
            biome: "desert".into(),
            distance: 420,
            venue: Venue::Overworld,
            rank: 3,
            power: 2.5,
            reward_chits: 900,
            reward_material: "frost_shard".into(),
            reward_material_qty: 4,
            reward_gear: true,
            reward_rank_xp: 100,
        };
        assert_eq!(spec.mark_name("Ironmaw"), "Ironmaw the Unburied");
        spec.epithet.clear();
        assert_eq!(spec.mark_name("Ironmaw"), "Ironmaw");
    }

    #[test]
    fn every_state_round_trips_through_the_wire_word() {
        for state in [
            BountyState::Active,
            BountyState::Completed,
            BountyState::Claimed,
            BountyState::Expired,
        ] {
            assert_eq!(BountyState::from_wire(state.wire()), Some(state));
        }
        assert_eq!(BountyState::from_wire("nonsense"), None);
        assert!(BountyState::Active.is_standing());
        for done in [BountyState::Completed, BountyState::Claimed, BountyState::Expired] {
            assert!(!done.is_standing(), "{} should not stand", done.wire());
        }
    }

    #[test]
    fn the_rank_ladder_names_every_rank_and_climbs() {
        let titles: Vec<&str> = (0..60).map(rank_title).collect();
        assert!(titles.iter().all(|t| !t.is_empty()));
        assert_eq!(rank_title(0), "Unblooded");
        assert_ne!(rank_title(1), rank_title(2), "the first bounty changes the title");
        assert_eq!(rank_title(999), "Apex");
        let mut seen = Vec::new();
        for r in 0..60 {
            let t = rank_title(r);
            if seen.last() != Some(&t) {
                assert!(!seen.contains(&t), "{t} is handed out twice at rank {r}");
                seen.push(t);
            }
        }
    }

    #[test]
    fn there_are_enough_epithets_that_two_marks_read_as_two_creatures() {
        assert!(EPITHETS.len() >= 8);
        let mut seen = std::collections::HashSet::new();
        for e in EPITHETS {
            assert!(seen.insert(e), "duplicate epithet {e}");
            assert!(!e.is_empty());
            assert!(!e.ends_with('.'), "{e} is a clause, not a sentence");
        }
    }
}
