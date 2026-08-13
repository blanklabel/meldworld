//! The Hunt Board: directed combat goals (roadmap `AD-4`,
//! [`docs/proposals/adventure-depth.md`] §E).
//!
//! A hunt is a named thing to go and do, posted on a board in Last City, that pays when
//! you come home. It is **combat-facing**, and so distinct from the economy's *gathering*
//! contracts (`EC-1`), which are player-posted and share the board later.
//!
//! Both sides read this registry: the server credits progress through
//! [`HuntGoal::credits`] and the board draws its rows from the same defs, so the board
//! cannot advertise a condition the server does not check.
//!
//! **A hunt names a quarry; only its goal carries a count.** [`objective`] formats the
//! sentence, so a retuned count cannot leave a stale number in prose.
//!
//! Reward magnitudes are `[TUNABLE]`s in `balance.toml` under `[hunt]`, scaled by
//! [`HuntDef::tier`] and formatted server-side onto the wire. A def names only the
//! **material** a board hands over, which is content.

use serde::{Deserialize, Serialize};

/// What a hunt asks for. Every variant reduces to "N of these events", so progress is
/// one integer and a hunt is complete at [`HuntGoal::target`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum HuntGoal {
    /// Fell `count` creatures of one kind (`meld-world`'s `monster_kind`).
    Fell { creature: &'static str, count: i32 },
    /// Fell `count` creatures of one **encounter class** — `elite` or `gatekeeper`.
    FellClass { class: &'static str, count: i32 },
    /// Stand at `distance` or deeper.
    Depth { distance: i32 },
    /// End a run in a successful extraction, having reached `distance` or deeper.
    ExtractFrom { distance: i32 },
    /// Fell `count` dungeon bosses.
    ClearDungeon { count: i32 },
}

impl HuntGoal {
    /// The progress a hunt is complete at.
    pub fn target(&self) -> i32 {
        match self {
            HuntGoal::Fell { count, .. }
            | HuntGoal::FellClass { count, .. }
            | HuntGoal::ClearDungeon { count } => *count,
            HuntGoal::Depth { .. } | HuntGoal::ExtractFrom { .. } => 1,
        }
    }

    /// How much `ev` credits this goal — `0` for an event it does not care about.
    ///
    /// The only place the matching rule lives, so a hunt kind cannot be half-wired with
    /// the loop reporting an event nothing credits.
    pub fn credits(&self, ev: &HuntEvent) -> i32 {
        match (self, ev) {
            (HuntGoal::Fell { creature, .. }, HuntEvent::Felled { creature: c, .. }) => {
                i32::from(creature == c)
            }
            (HuntGoal::FellClass { class, .. }, HuntEvent::Felled { class: c, .. }) => {
                i32::from(class == c)
            }
            (HuntGoal::Depth { distance }, HuntEvent::Depth { distance: d }) => {
                i32::from(d >= distance)
            }
            (HuntGoal::ExtractFrom { distance }, HuntEvent::Extracted { deepest }) => {
                i32::from(deepest >= distance)
            }
            (HuntGoal::ClearDungeon { .. }, HuntEvent::DungeonCleared) => 1,
            _ => 0,
        }
    }
}

/// Something the game loop saw happen, offered to every hunt.
///
/// Every field is read off server-owned state — the felled creature's own kind, the
/// validated avatar position, the run's own record — so a board cannot be talked into
/// paying (CANON §S).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HuntEvent<'a> {
    /// A creature died in a won fight. `class` is its `encounter_class`.
    Felled { creature: &'a str, class: &'a str },
    /// The player is standing at this floored distance.
    Depth { distance: i32 },
    /// A run ended in a successful extraction, having reached `deepest`.
    Extracted { deepest: i32 },
    /// A dungeon boss was felled.
    DungeonCleared,
}

/// One posted hunt.
#[derive(Debug, Clone, Copy)]
pub struct HuntDef {
    /// Stable key: what persists, and what a claim names.
    pub key: &'static str,
    pub name: &'static str,
    /// The counted noun, plural — "Bloom Stalkers", "Gatekeepers", "dungeon vaults".
    /// Empty for a goal that counts nothing but depth.
    pub quarry: &'static str,
    /// Why the board wants it. Flavour; never a magnitude.
    pub blurb: &'static str,
    pub goal: HuntGoal,
    /// Biome band (forest 0 … mire 4). Scales the reward from `[hunt]` and orders the
    /// board shallow → deep.
    pub tier: i32,
    /// The stack handed over on top of the chits, keyed into [`crate::materials`]. The
    /// quantity is a `[TUNABLE]`.
    pub reward_material: Option<&'static str>,
    /// Whether the board also hands over a **rolled piece of gear**.
    ///
    /// Only the deep hunts do, so the board is a ladder rather than a shortcut — and the
    /// piece is rolled at the hunt's own band from the ordinary pool, never the epic one:
    /// a champion you fought for stays the better source of a great item. The board's
    /// version is *reliable*, not superior.
    pub reward_gear: bool,
}

/// Every hunt on the board, shallow → deep.
///
/// One cull in the shallow, middle and deep bands; both champion classes; the two halves
/// of the core loop (get deep, come home); and the dungeons. Rotation, expiry, accepting
/// a contract and co-op hunts are the full `AD-4` system and are not here.
pub const HUNTS: &[HuntDef] = &[
    HuntDef {
        key: "cull_the_bloom",
        name: "Cull the Bloom",
        quarry: "Bloom Stalkers",
        blurb: "They have learned to stand where the light comes through, which is where \
                everyone walks. The wood is the first thing anyone sees on a first run; \
                the Guides would rather it did not eat them.",
        goal: HuntGoal::Fell { creature: "forest_bloom_stalker", count: 8 },
        tier: 0,
        reward_material: Some("forest_bloom_petal"),
        reward_gear: false,
    },
    HuntDef {
        key: "the_wyrm_contract",
        name: "The Wyrm Contract",
        quarry: "Dune Wyrms",
        blurb: "A caravan road runs through their hunting ground, and the Foundry's ore \
                comes up it. The Den posts this one every season and every season it is \
                taken.",
        goal: HuntGoal::Fell { creature: "dune_wyrm", count: 6 },
        tier: 1,
        reward_material: Some("sun_scarab_husk"),
        reward_gear: false,
    },
    HuntDef {
        key: "the_vaults_below",
        name: "The Vaults Below",
        quarry: "dungeon vaults",
        blurb: "Something built these, and something else is keeping the door. The board \
                does not much care which — it pays on the door being open.",
        goal: HuntGoal::ClearDungeon { count: 1 },
        tier: 1,
        reward_material: Some("dune_ingot"),
        reward_gear: false,
    },
    HuntDef {
        key: "the_far_frontier",
        name: "The Far Frontier",
        quarry: "",
        blurb: "Walk out past where the maps stop agreeing with each other. The Explorers \
                pay for the standing, not the loot — someone has to have been there.",
        goal: HuntGoal::Depth { distance: 300 },
        tier: 2,
        reward_material: None,
        reward_gear: false,
    },
    HuntDef {
        key: "break_the_champions",
        name: "Break the Champions",
        quarry: "Elites",
        blurb: "The ones that came back bigger. Left alone they hold ground the rest of us \
                need, and they teach the others how it is done.",
        goal: HuntGoal::FellClass { class: "elite", count: 5 },
        tier: 2,
        reward_material: Some("ember_cinder"),
        reward_gear: false,
    },
    HuntDef {
        key: "the_long_walk_back",
        name: "The Long Walk Back",
        quarry: "",
        blurb: "Getting deep is the easy half. The hall pays on evidence, not stories, and \
                a backpack that came home is the evidence.",
        goal: HuntGoal::ExtractFrom { distance: 500 },
        tier: 3,
        reward_material: Some("rime_ingot"),
        reward_gear: true,
    },
    HuntDef {
        key: "unseat_the_keeper",
        name: "Unseat the Keeper",
        quarry: "Gatekeepers",
        blurb: "One of them sits at the edge of every band, and nothing behind it moves \
                until it does. Take one down and the whole frontier breathes out.",
        goal: HuntGoal::FellClass { class: "gatekeeper", count: 1 },
        tier: 3,
        reward_material: Some("frost_shard"),
        reward_gear: true,
    },
    HuntDef {
        key: "drain_the_mire",
        name: "Drain the Mire",
        quarry: "Bog Serpents",
        blurb: "They swallowed the deep and kept going. Nobody expects the mire cleared — \
                the board would settle for it being thinned.",
        goal: HuntGoal::Fell { creature: "bog_serpent", count: 6 },
        tier: 4,
        reward_material: Some("bog_ichor"),
        reward_gear: true,
    },
];

pub fn hunt(key: &str) -> Option<&'static HuntDef> {
    HUNTS.iter().find(|h| h.key == key)
}

/// The one-line statement of what a hunt wants, with its number in it.
pub fn objective(def: &HuntDef) -> String {
    match def.goal {
        HuntGoal::Fell { count, .. } | HuntGoal::FellClass { count, .. } => {
            format!("Fell {count} {}", def.quarry)
        }
        HuntGoal::ClearDungeon { count } => format!("Clear {count} {}", def.quarry),
        HuntGoal::Depth { distance } => format!("Reach depth {distance}"),
        HuntGoal::ExtractFrom { distance } => format!("Extract from depth {distance} or deeper"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_hunt_is_uniquely_keyed_and_pays_something_real() {
        let mut seen = std::collections::HashSet::new();
        for h in HUNTS {
            assert!(seen.insert(h.key), "duplicate hunt {}", h.key);
            assert!(!h.name.is_empty() && !h.blurb.is_empty(), "{}", h.key);
            assert!((0..=4).contains(&h.tier), "{} has tier {}", h.key, h.tier);
            assert!(h.goal.target() > 0, "{} can never be completed", h.key);
            assert_eq!(hunt(h.key).map(|d| d.key), Some(h.key));
            if let Some(m) = h.reward_material {
                assert!(
                    crate::materials::is_material(m),
                    "{} pays in {m}, which no registry knows",
                    h.key
                );
            }
        }
    }

    #[test]
    fn a_counted_hunt_names_its_quarry_and_the_count_lives_only_in_the_goal() {
        for h in HUNTS {
            assert!(
                !h.blurb.chars().any(|c| c.is_ascii_digit()),
                "{} writes a number into its blurb",
                h.key
            );
            let counted = matches!(
                h.goal,
                HuntGoal::Fell { .. } | HuntGoal::FellClass { .. } | HuntGoal::ClearDungeon { .. }
            );
            assert_eq!(
                counted,
                !h.quarry.is_empty(),
                "{} must name a quarry if and only if it counts them",
                h.key
            );
            let line = objective(h);
            assert!(line.chars().any(|c| c.is_ascii_digit()), "{line} states no number");
        }
    }

    #[test]
    fn a_goal_credits_its_own_event_and_nothing_else() {
        let fell = HuntGoal::Fell { creature: "dune_wyrm", count: 6 };
        assert_eq!(fell.credits(&HuntEvent::Felled { creature: "dune_wyrm", class: "standard" }), 1);
        assert_eq!(fell.credits(&HuntEvent::Felled { creature: "bog_serpent", class: "elite" }), 0);
        assert_eq!(fell.credits(&HuntEvent::DungeonCleared), 0);

        let champs = HuntGoal::FellClass { class: "elite", count: 5 };
        assert_eq!(champs.credits(&HuntEvent::Felled { creature: "anything", class: "elite" }), 1);
        assert_eq!(champs.credits(&HuntEvent::Felled { creature: "anything", class: "standard" }), 0);
        assert_eq!(champs.credits(&HuntEvent::Felled { creature: "x", class: "gatekeeper" }), 0);

        let deep = HuntGoal::Depth { distance: 300 };
        assert_eq!(deep.credits(&HuntEvent::Depth { distance: 299 }), 0);
        assert_eq!(deep.credits(&HuntEvent::Depth { distance: 300 }), 1);
        assert_eq!(deep.credits(&HuntEvent::Depth { distance: 4000 }), 1);
        assert_eq!(deep.credits(&HuntEvent::Extracted { deepest: 900 }), 0);

        let home = HuntGoal::ExtractFrom { distance: 500 };
        assert_eq!(home.credits(&HuntEvent::Depth { distance: 900 }), 0);
        assert_eq!(home.credits(&HuntEvent::Extracted { deepest: 500 }), 1);
    }

    #[test]
    fn only_the_deep_hunts_pay_gear_and_the_shallow_ones_still_pay() {
        // The board has to read as a ladder: if the first hunt handed over a piece there
        // would be no reason to work the deep ones, and if none did, the board would only
        // ever pay in a currency the Broker already prints.
        for h in HUNTS {
            assert_eq!(
                h.reward_gear,
                h.tier >= 3,
                "{} pays gear at tier {}",
                h.key,
                h.tier
            );
            assert!(
                h.reward_gear || h.reward_material.is_some() || h.tier == 2,
                "{} pays nothing but chits",
                h.key
            );
        }
        assert!(HUNTS.iter().any(|h| h.reward_gear), "no hunt on the board pays a piece");
        assert!(HUNTS.iter().any(|h| !h.reward_gear), "every hunt pays a piece");
    }

    #[test]
    fn the_board_spans_the_difficulty_axis_and_the_things_a_dive_is_made_of() {
        assert!(HUNTS.len() >= 6, "a board needs a handful of hunts");
        for want in ["elite", "gatekeeper"] {
            assert!(
                HUNTS
                    .iter()
                    .any(|h| matches!(h.goal, HuntGoal::FellClass { class, .. } if class == want)),
                "nothing on the board asks for a {want}"
            );
        }
        assert!(HUNTS.iter().any(|h| matches!(h.goal, HuntGoal::Depth { .. })));
        assert!(HUNTS.iter().any(|h| matches!(h.goal, HuntGoal::ExtractFrom { .. })));
        assert!(HUNTS.iter().any(|h| matches!(h.goal, HuntGoal::ClearDungeon { .. })));
        assert!(HUNTS.iter().any(|h| h.tier == 0), "nothing a first dive can take");
        assert!(HUNTS.iter().any(|h| h.tier >= 3), "nothing left for a deep account");

        let tiers: Vec<i32> = HUNTS.iter().map(|h| h.tier).collect();
        let mut sorted = tiers.clone();
        sorted.sort();
        assert_eq!(tiers, sorted, "the board is ordered shallow → deep");
    }
}
