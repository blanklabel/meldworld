//! Departure points (PG-2, superseded in part by `BD-5`): where a dive starts, and
//! therefore what level it starts at.
//!
//! `start_level(distance)` is the load-bearing thing here — `1 + 0.078 × D`, so departing
//! from d500 starts every hero at 40 and d3256 at the 255 cap. That formula is what makes
//! pushing outward worth anything, and it is unchanged.
//!
//! **THE AUTHORED DEEP HUBS ARE DEPRECATED.** There used to be six of them (d500 … d3250)
//! gated on the player's own deepest recorded distance from the `vanguard` table. They are
//! gone, and the gate with them, because `BD-5`'s **player-built forward towns** do the same
//! job strictly better:
//!
//! - **The gate becomes physical.** "Have you been here" needed a server-owned all-time
//!   distance record precisely because a hub was scenery that appeared once you qualified.
//!   A town is something you walked to, hauled stock to, and built — you cannot raise one
//!   where you cannot stand, so the proof *is* the structure. A whole bookkeeping mechanism
//!   turns into a consequence.
//! - **The ladder becomes a loop instead of a list.** An authored hub, once unlocked, was
//!   permanent and free. A town is HP-bearing, Shift-exposed and siege-able (`BD-2`/`BD-3`),
//!   so a deep departure point is permanence you keep paying for — which is the loop those
//!   epics exist to create, and the reason an anchor beside it means something.
//! - **It was never reachable anyway.** Measured: the ground at d3200 demands ~level 251 to
//!   survive four basic hits from a *standard* creature, and levels are dive-scoped, so the
//!   d3250 hub could only ever be unlocked by a party that had already walked to d3250 at
//!   level 1. The authored ladder's own top rung required the thing it was meant to grant.
//!
//! **The Center Hub stays, and stays unconditional.** It is the one departure point nothing
//! can take away — a player whose forward town is destroyed still has somewhere to dive
//! from, at level 1. Everything deeper is built, held, and losable.
//!
//! **This is still a LOOKUP, not an entity.** The run reads one integer: a distance. A town
//! supplies that integer through the `Structure` primitive's own placement and ownership
//! (`BD-2`) — `do not build towns, anchors, portals and camps as separate systems`. Nothing
//! here grows a placement or lifecycle model of its own; that was the point of the
//! indirection and it is the point still.

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

    /// **The formula is what survived the deprecation, so it is what the test holds.** A
    /// forward town at distance D starts every hero at `start_level(D)`, which is the entire
    /// reason to push one outward. The old `the_deepest_hub_lands_on_the_level_cap` asserted
    /// this through an authored row; the claim was never about the row.
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
