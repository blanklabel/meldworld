//! Where a dive starts. **Level at distance is RETIRED** (PG-2 -> `BD-5`).
//!
//! There is one departure point — the **Center Hub** — and it starts every hero at level 1.
//! The six authored deep hubs (d500 … d3250) and the `vanguard` "have you been there" gate
//! are gone, and so is the idea behind them: *a departure point does not grant a level.*
//!
//! **The longer you are out there, the stronger you tend to be.** Level comes from XP earned
//! on the expedition (`AD-7` prices punching above your weight), never from where you set
//! off. A `BD-5` forward town is worth building for what it lets you *do* — rest at an inn
//! across sessions, swap party members, resupply, and buy an NPC garrison with chits — not
//! for a number it hands you on departure. An inn is a **save point that can be destroyed**:
//! if the town falls while you are resting in it, you go with it, which is what makes the
//! garrison worth paying for.
//!
//! Retiring the authored hubs was not only a design call — the top rung was self-defeating.
//! Measured: d3200 ground demands ~level 251 to survive four basic hits from a *standard*
//! creature (a level-100 hero survives 1.4), and levels are dive-scoped, so the d3250 hub
//! could only ever be unlocked by a party that had already walked to d3250 at level 1. It
//! required exactly what it was meant to grant.
//!
//! ⚠️ **`start_level` is now a DEV/QA INSTRUMENT, not a game rule.** Nothing in play reads
//! it: it is the inverse of `MELD_START_LEVEL`, which sets a DISTANCE and lets the level
//! follow, because level and depth are the same fact and a party holding one without the
//! other cannot exist. It is how deep content is measured at all, so it must keep agreeing
//! with the server's `base_run_level` — held by a distance SWEEP in `meld-run`, since this
//! repo has already shipped one balance pass taken through a broken instrument.

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

/// The authored departure points: the Center Hub, and nothing else. Everything deeper is a
/// player-built forward town (`BD-5`) supplying its own distance — see the module docs for
/// why the six authored deep hubs were retired.
pub const HUBS: &[HubDef] = &[HubDef {
    key: "center",
    name: "The Center Hub",
    distance: 0,
    blurb: "Where everyone starts. The Last City sits at its western wedge.",
}];

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

#[cfg(test)]
mod tests {
    use super::*;

    /// The Center Hub is unconditional and it is the ONLY authored one. A player whose
    /// forward town has been destroyed — or who has never built one — still has somewhere
    /// to dive from, at level 1.
    #[test]
    fn the_center_hub_is_the_floor_and_the_only_authored_row() {
        assert_eq!(HUBS.len(), 1, "an authored deep hub came back — see the module docs");
        assert_eq!(HUBS[0].distance, 0);
        assert_eq!(start_level(HUBS[0].distance), 1, "the floor must start a hero at 1");
        assert!(hub("center").is_some());
        assert!(hub("not_a_hub").is_none());
    }

    /// The formula survives only as the `MELD_START_LEVEL` instrument, so what the test
    /// holds is that the instrument spans the real range: level 1 at the origin up to the
    /// cap at the structural end. Nothing in PLAY reads it — a town grants services, not
    /// levels — but a measurement harness that cannot reach level 255 cannot measure the
    /// deep game, which is the only place these numbers are in doubt.
    #[test]
    fn the_ladder_reaches_the_cap_exactly_at_the_structural_end() {
        assert_eq!(start_level(0), 1);
        assert!(start_level(3256) >= 255, "the deep end no longer reaches the cap");
        // …and it is monotonic, so deeper is always worth more until the cap.
        let mut prev = 0;
        for d in (0..3300).step_by(50) {
            let l = start_level(d);
            assert!(l >= prev, "d{d} starts a hero lower than d{}", d - 50);
            prev = l;
        }
    }

    /// Past the cap a departure point buys nothing — heroes stop while creatures keep
    /// scaling — which is what makes ~d3250 the structural end of the game rather than an
    /// arbitrary wall, and what bounds how far a forward town is worth hauling stock.
    #[test]
    fn a_town_past_the_cap_buys_nothing() {
        let capped = start_level(3256).min(255);
        assert_eq!(start_level(6000).min(255), capped);
    }

    #[test]
    fn every_authored_hub_says_what_it_is() {
        for h in HUBS {
            assert!(!h.name.is_empty() && !h.blurb.is_empty(), "{} is unlabelled", h.key);
            assert!(h.blurb.len() > 20, "{}'s blurb says nothing", h.key);
        }
    }
}
