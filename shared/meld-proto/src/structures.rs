//! Player-built world objects — **one primitive, many functions** (CANON D21 / §W3).
//!
//! A `Structure` is HP-bearing, destructible and siege-able, and a `function` tag varies
//! its role. CANON is explicit that this is a discipline rather than a convenience:
//! *"the siege sim, the spatial interest index, and world persistence handle every
//! function uniformly — do not build towns, anchors, portals, and camps as separate
//! systems."* A new kind of thing players can build is a row in this table, never a new
//! entity type with its own lifecycle.
//!
//! This registry is what both sides read: the server gates placement and cost on it, the
//! client builds its menu rows and its render from it, so a function is defined once. The
//! *magnitudes* — cost, HP, build time, pin radius — are `[TUNABLE]`s in
//! `balance/balance.toml` and deliberately not here, because `meld-proto` is shared with a
//! client that has no balance loader.
//!
//! **`workshop` is missing on purpose.** MS-1's field stations (the forge and the alembic)
//! predate this primitive and still run their own lifecycle; folding them in is `BD-6`,
//! which is the roadmap item that owns field crafting. Adding a paper `workshop` row here
//! that nothing honours would be worse than the honest gap.

/// What a structure is *for*. The role is the only thing that varies — everything else
/// about the lifecycle is shared.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StructureFunction {
    /// Pins its region against the Shift while it stands (D20/§W2, §W3). The way players
    /// manufacture permanence in a self-rearranging world — and the single point of
    /// failure by design, because holding ground you cannot defend is not holding it.
    Anchor,
    /// Blocks movement and soaks siege. The cheap one, and the reason this is a *registry*
    /// rather than an anchor with extra steps: two functions on one lifecycle is the claim
    /// D21 makes, and one function would leave it untested.
    Wall,
}

pub struct StructureDef {
    pub key: &'static str,
    pub name: &'static str,
    /// What it does, in a sentence a player reads on the build menu. No magnitudes — those
    /// live in balance and are formatted server-side, the way ability effects are.
    pub description: &'static str,
    pub function: StructureFunction,
    /// Does it hold ground against the Shift?
    pub pins: bool,
    /// Does it stop things walking through it?
    pub blocks: bool,
}

pub const STRUCTURES: &[StructureDef] = &[
    StructureDef {
        key: "anchor",
        name: "Anchor",
        description: "Holds the ground around it against the Shift, for as long as it stands.",
        function: StructureFunction::Anchor,
        pins: true,
        blocks: false,
    },
    StructureDef {
        key: "wall",
        name: "Wall",
        description: "A barrier nothing walks through. Cheap, and it does not hold ground.",
        function: StructureFunction::Wall,
        pins: false,
        blocks: true,
    },
];

pub fn structure(key: &str) -> Option<&'static StructureDef> {
    STRUCTURES.iter().find(|s| s.key == key)
}

/// Every structure key, for a client that wants to list what can be built.
pub fn keys() -> impl Iterator<Item = &'static str> {
    STRUCTURES.iter().map(|s| s.key)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_are_unique_and_named() {
        let mut seen = std::collections::HashSet::new();
        for s in STRUCTURES {
            assert!(seen.insert(s.key), "duplicate structure key `{}`", s.key);
            assert!(!s.name.is_empty() && !s.description.is_empty(), "{} is unlabelled", s.key);
        }
    }

    /// The pin is the whole point of `BD-3`, and the roadmap's headline loop hangs off
    /// exactly one function having it. A registry with nothing that pins would let the
    /// Shift-suppression code path exist with nothing able to reach it.
    #[test]
    fn something_holds_ground_and_something_does_not() {
        assert!(STRUCTURES.iter().any(|s| s.pins), "nothing can hold ground");
        assert!(
            STRUCTURES.iter().any(|s| !s.pins),
            "every structure pins, so `pins` is not a distinction"
        );
    }

    /// A description that repeats the name teaches nothing, and this text is the only
    /// thing standing between a player and a menu of nouns.
    #[test]
    fn a_description_says_more_than_the_name() {
        for s in STRUCTURES {
            assert!(
                s.description.len() > s.name.len() + 12,
                "`{}` describes itself as `{}`",
                s.key,
                s.description
            );
        }
    }
}
