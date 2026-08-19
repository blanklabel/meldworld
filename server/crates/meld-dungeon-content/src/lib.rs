//! Compiled authored-dungeon registry (DG-2).
//!
//! The authored `content/**/*.dungeon.toml` files are parsed, validated (incl. the
//! solvability gate), and embedded **at build time** by [`build.rs`] — see there for
//! the compile-as-gate. This crate exposes the resulting registry: a `&'static`
//! slice of validated [`DungeonDef`]s, plus per-biome / by-name lookups the runtime
//! (DG-3) uses to place a dungeon behind an entrance.
//!
//! Types come from `meld-dungeon` and are re-exported, so downstream crates depend
//! only on this one.
//!
//! ```
//! // Every embedded dungeon is valid by construction (the build enforced it).
//! for d in meld_dungeon_content::all() {
//!     assert!(!d.grids.is_empty());
//! }
//! ```

use std::sync::OnceLock;

pub use meld_dungeon::*;

/// The validated registry, serialized at build time (see `build.rs`). Deserialized
/// once, lazily, on first access.
const REGISTRY_JSON: &str = include_str!(concat!(env!("OUT_DIR"), "/dungeons.json"));

fn registry() -> &'static [DungeonDef] {
    static REGISTRY: OnceLock<Vec<DungeonDef>> = OnceLock::new();
    REGISTRY
        .get_or_init(|| {
            serde_json::from_str(REGISTRY_JSON).expect("embedded dungeon registry is valid JSON")
        })
        .as_slice()
}

/// Every authored dungeon, ordered by name. Each is guaranteed well-formed and
/// solvable — the build refused to compile otherwise.
pub fn all() -> &'static [DungeonDef] {
    registry()
}

/// The authored dungeons whose `biome` matches `biome` (the per-biome pool the
/// runtime draws from when an entrance spawns in that biome).
pub fn for_biome(biome: &str) -> impl Iterator<Item = &'static DungeonDef> {
    // Own the key so the returned iterator borrows nothing but the 'static registry.
    let biome = biome.to_string();
    registry().iter().filter(move |d| d.biome == biome)
}

/// Look a dungeon up by its unique `name`.
pub fn by_name(name: &str) -> Option<&'static DungeonDef> {
    registry().iter().find(|d| d.name == name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_loads_and_is_nonempty() {
        assert!(!all().is_empty(), "DG-2 ships at least one authored dungeon");
    }

    #[test]
    fn a_co_op_gate_is_visible_from_the_definition() {
        // `verdant_barrow`'s G1 is `all[P1,P2,P3]` over momentary plates: three bodies.
        // The runtime currently latches plates so one player clears it, but the
        // AUTHORED requirement is what a party is warned about at the entrance — and
        // what binds again the moment `momentary` is honoured.
        let d = by_name("verdant_barrow").unwrap();
        assert_eq!(d.bodies_required(), 3, "its gate wants three plates held at once");

        // Every shipped dungeon reports something sane, and a soloable one says 1.
        for d in all() {
            let n = d.bodies_required();
            assert!((1..=4).contains(&n), "{} wants {n} bodies", d.name);
        }
        assert_eq!(by_name("guardia_forest").unwrap().bodies_required(), 1);
    }

    /// `qa/tests/dungeon_trap_death.rs` marches a bot EAST from the entry and expects to
    /// die, and it pins `verdant_barrow` to get a trap on that line. That is a dependency on
    /// this floor's SHAPE, and nothing here was holding it: the test used to pin a seed
    /// instead, the roll drifted to `guardia_forest` — whose entry row is deliberately clear,
    /// because it is the tutorial's dungeon — and the bot marched into the far wall and sat
    /// there until a 90-second deadline. A content edit could do the same again.
    ///
    /// Asserted here so the break is a one-line failure in a unit test rather than a timeout
    /// in a Postgres-backed bot run.
    #[test]
    fn the_barrow_still_opens_onto_a_trap_the_trap_test_can_walk_into() {
        let d = by_name("verdant_barrow").expect("verdant_barrow is shipped content");
        let entry = d
            .entrances
            .iter()
            .find(|p| p.floor == 0)
            .expect("floor 0 has the overworld entry");
        // A trap on the same row, east of the entry, with clear floor the whole way to it —
        // which is exactly what "march east and die" needs.
        let trap = d
            .placements
            .iter()
            .filter(|p| matches!(d.objects.get(&p.id), Some(crate::ObjectKind::Trap { .. })))
            .filter(|p| p.floor == 0 && p.y == entry.y && p.x > entry.x)
            .min_by_key(|p| p.x)
            .unwrap_or_else(|| {
                panic!(
                    "no trap east of the entry on floor 0 — `dungeon_trap_death` marches east \
                     and would time out rather than fail"
                )
            });
        for x in (entry.x + 1)..trap.x {
            assert_eq!(
                d.grids[0].at(x, entry.y).tile,
                meld_dungeon::Tile::Floor,
                "something blocks the walk east at x={x}; the bot would stop short of the trap"
            );
        }
    }

    #[test]
    fn every_embedded_dungeon_revalidates_at_runtime() {
        // Belt-and-suspenders: the build already validated these, but prove the
        // embedded form round-trips back to a still-valid def.
        for d in all() {
            assert!(validate(d).is_empty(), "embedded dungeon {:?} must stay valid", d.name);
            assert!(!d.entrances.is_empty() && !d.exits.is_empty());
        }
    }

    #[test]
    fn names_are_unique_and_lookups_work() {
        for d in all() {
            assert!(by_name(&d.name).is_some(), "by_name finds {:?}", d.name);
        }
        let names: Vec<&str> = all().iter().map(|d| d.name.as_str()).collect();
        let mut deduped = names.clone();
        deduped.sort_unstable();
        deduped.dedup();
        assert_eq!(names.len(), deduped.len(), "dungeon names are unique");
    }

    #[test]
    fn for_biome_filters() {
        for d in all() {
            assert!(
                for_biome(&d.biome).any(|x| x.name == d.name),
                "{:?} appears in its biome pool {:?}",
                d.name,
                d.biome
            );
        }
        assert_eq!(for_biome("no_such_biome").count(), 0);
    }
}
