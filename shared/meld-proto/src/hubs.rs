//! Departure hubs (PG-2): where a dive starts, and therefore what level it starts at.
//!
//! `base_run_level(distance)` has always existed — `1 + 0.078 × D`, so a hub at d500 starts
//! every hero at 40. What did not exist was any way to depart from anywhere but the Center
//! Hub, so every hero started every dive at level 1 and the whole ladder above roughly
//! level 16 was authored ahead of what anyone could reach.
//!
//! **A hub is somewhere you have BEEN.** Not a purchase, not a trigger — the gate is the
//! player's own deepest recorded distance, which the `vanguard` table already keeps off
//! *validated movement* and which a client cannot submit. Read all-time, never the live
//! season: a season rollover must not revoke a hub you demonstrably reached.
//!
//! **This is a LOOKUP, not an entity.** The run reads one integer. These are the rows a
//! server-owned world offers; when `BD-5`'s forward towns land, a player's own town becomes
//! another row and nothing here is rewritten. A hub deliberately has no placement,
//! ownership or lifecycle of its own — that is the `Structure` primitive (`BD-2`), and
//! duplicating it is the rework this indirection exists to avoid.

use serde::{Deserialize, Serialize};

/// One place a dive may start from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct HubDef {
    /// Stable key — the wire value `run.enter_maze` names.
    pub key: &'static str,
    pub name: &'static str,
    /// Distance from the origin. Feeds `base_run_level`, and is also the bar a player must
    /// have reached to depart from here.
    pub distance: i32,
    /// One line for the chooser: what this hub is, in the fiction.
    pub blurb: &'static str,
}

/// Every departure hub, shallowest first.
///
/// Spaced so each is a visible jump in starting level rather than a trickle — 1 / 40 / 79 /
/// 118 / 157 / 196 / 255 — with the last landing exactly on `max_hero_level`. Past d3256 a
/// hub buys nothing (heroes are capped while creatures keep scaling), which makes that
/// distance the structural end of the game rather than an arbitrary wall.
pub const HUBS: &[HubDef] = &[
    HubDef {
        key: "center",
        name: "The Center Hub",
        distance: 0,
        blurb: "Where everyone starts. The Last City sits at its western wedge.",
    },
    HubDef {
        key: "first_reach",
        name: "First Reach",
        distance: 500,
        blurb: "The furthest post the Explorers kept after the second expansion.",
    },
    HubDef {
        key: "the_span",
        name: "The Span",
        distance: 1000,
        blurb: "A pass held open by an anchor nobody living remembers setting.",
    },
    HubDef {
        key: "cinderwatch",
        name: "Cinderwatch",
        distance: 1500,
        blurb: "Built downwind of the ashfall, and still standing.",
    },
    HubDef {
        key: "the_lastward",
        name: "The Lastward",
        distance: 2000,
        blurb: "The deepest ground the Vanguard has ever held for a full season.",
    },
    HubDef {
        key: "hollow_march",
        name: "Hollow March",
        distance: 2500,
        blurb: "Not a settlement. A staging line, and a name for what is past it.",
    },
    HubDef {
        key: "the_threshold_deep",
        name: "The Threshold Deep",
        distance: 3250,
        blurb: "As far out as a hero can still be made ready for. Beyond is the end-world.",
    },
];

/// The level every hero starts at when departing from `distance` — `base_run_level`'s
/// formula, duplicated here ONLY so the chooser can say "heroes start at 40" without the
/// client needing `balance.toml` (which it has no access to). Held against the server's
/// own copy by a test in `meld-run`, so the two cannot drift.
pub fn start_level(distance: i32) -> i32 {
    (1.0 + 0.078 * distance as f64).round() as i32
}

/// The hub `key` names, if it is one.
pub fn hub(key: &str) -> Option<&'static HubDef> {
    HUBS.iter().find(|h| h.key == key)
}

/// Every hub a player who has reached `deepest` may depart from, shallowest first. The
/// Center Hub is always in the list — `distance: 0` clears any record, including none.
pub fn hubs_reached(deepest: i32) -> Vec<&'static HubDef> {
    HUBS.iter().filter(|h| h.distance <= deepest).collect()
}

/// The deepest hub a player who has reached `deepest` may depart from. What the chooser
/// defaults to, and what a client that names nothing gets.
pub fn deepest_hub(deepest: i32) -> &'static HubDef {
    hubs_reached(deepest).last().copied().unwrap_or(&HUBS[0])
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The Center Hub is unconditional. A brand-new account has no vanguard row at all, so
    /// the read is 0 (or absent) — and it still has somewhere to dive from.
    #[test]
    fn everyone_can_always_leave_from_the_center() {
        assert_eq!(deepest_hub(0).key, "center");
        assert_eq!(deepest_hub(-1).key, "center", "a missing record is not a locked game");
        assert_eq!(hubs_reached(0).len(), 1);
    }

    /// You may only leave from ground you have stood on. The whole point of reading the
    /// vanguard record rather than selling hubs is that this cannot be short-circuited.
    #[test]
    fn a_hub_is_somewhere_you_have_been() {
        for h in HUBS {
            assert_eq!(
                deepest_hub(h.distance).key,
                h.key,
                "reaching exactly {} did not unlock {}",
                h.distance,
                h.key
            );
            if h.distance > 0 {
                assert_ne!(
                    deepest_hub(h.distance - 1).key,
                    h.key,
                    "{} opened one unit short of itself",
                    h.key
                );
            }
        }
    }

    /// Shallowest first, no duplicate distances, and each a real jump in starting level —
    /// a hub that started you within a level or two of the last one is a row nobody picks.
    #[test]
    fn the_hubs_are_ordered_and_meaningfully_spaced() {
        let mut prev = -1;
        for h in HUBS {
            assert!(h.distance > prev, "{} is out of order or duplicated", h.key);
            prev = h.distance;
        }
        for pair in HUBS.windows(2) {
            let step = pair[1].distance - pair[0].distance;
            assert!(step >= 400, "{} is only {step} past {}", pair[1].key, pair[0].key);
        }
    }

    /// The deepest hub lands on the hero level cap, which is what makes it the end of the
    /// ladder rather than a number someone liked. `base_run_level` is
    /// `1 + 0.078 × D`, so 255 arrives at d3256 — and a hub past it would start a hero no
    /// higher while the creatures there keep scaling.
    #[test]
    fn the_deepest_hub_lands_on_the_level_cap() {
        let deepest = HUBS.last().unwrap();
        let start = (1.0 + 0.078 * deepest.distance as f64).round() as i32;
        assert!(
            (250..=255).contains(&start),
            "the deepest hub starts a hero at {start}, not at the 255 cap"
        );
    }

    #[test]
    fn every_hub_says_what_it_is() {
        for h in HUBS {
            assert!(!h.name.is_empty() && !h.blurb.is_empty(), "{} is unlabelled", h.key);
            assert!(h.blurb.len() > 20, "{}'s blurb says nothing", h.key);
            assert!(hub(h.key).is_some());
        }
        assert!(hub("not_a_hub").is_none());
    }
}
