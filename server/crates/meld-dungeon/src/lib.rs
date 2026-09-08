//! Authored dungeons — the DG-1 foundation (see `docs/proposals/dungeons.md`).
//!
//! Each biome has a small pool of **hand-designed** dungeons (traps, puzzles, a
//! boss, treasure) laid out as a glyph grid + a manifest. This crate is the pure,
//! deterministic core: the [`DungeonDef`] data model, the [`parse`] layer
//! (condition grammar + TOML/grid → `DungeonDef`), and the [`validate`] layer
//! (structural + reference checks + the entrance→exit **solvability** search).
//!
//! No I/O, no RNG, no wall-clock — like `meld-battle`/`meld-world`, so it is
//! exhaustively unit-testable. Runtime placement, the trap/puzzle engine, loot,
//! and the client render land in DG-2…DG-6.
//!
//! ```
//! let d = meld_dungeon::parse_and_validate(meld_dungeon::FOREST_BARROW).unwrap();
//! assert_eq!(d.biome, "forest");
//! ```

mod error;
mod model;
mod parse;
mod validate;

pub use error::DungeonError;
pub use model::{
    Cell, ChestItem, ChestLoot, Condition, DungeonDef, Grid, Id, ObjectKind, Placement, RefKind,
    StairDir, Tile,
};
pub use parse::{parse_condition, parse_str};
pub use validate::validate;

/// The reference dungeon (also a build-time fixture). Embedded so tests — and
/// later the `build.rs` codegen — never depend on a filesystem path.
pub const FOREST_BARROW: &str = include_str!("../content/forest_barrow.dungeon.toml");

/// Parse **and** validate in one step, returning the def only if it is
/// well-formed and solvable. This is the entry point DG-2's `build.rs` will call
/// per authored file (a non-empty error list becomes a compile error).
pub fn parse_and_validate(src: &str) -> Result<DungeonDef, Vec<DungeonError>> {
    let def = parse_str(src).map_err(|e| vec![e])?;
    let errs = validate(&def);
    if errs.is_empty() {
        Ok(def)
    } else {
        Err(errs)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- the reference dungeon parses, validates, and is solvable ---

    #[test]
    fn forest_barrow_parses_validates_and_is_solvable() {
        let d = parse_and_validate(FOREST_BARROW).expect("reference dungeon must be valid");
        assert_eq!(d.name, "forest_barrow");
        assert_eq!(d.biome, "forest");
        assert_eq!(d.grids.len(), 2, "two floors");
        assert_eq!(d.entrances.len(), 1);
        assert_eq!(d.exits.len(), 1);
    }

    #[test]
    fn forest_barrow_has_the_expected_objects() {
        let d = parse_str(FOREST_BARROW).unwrap();
        assert!(matches!(d.objects.get("T1"), Some(ObjectKind::Trap { disarmable: true, .. })));
        assert!(matches!(d.objects.get("L1"), Some(ObjectKind::Lever)));
        assert!(matches!(d.objects.get("G1"), Some(ObjectKind::Gate { .. })));
        assert!(matches!(d.objects.get("S1"), Some(ObjectKind::Stair)));
        assert!(matches!(d.objects.get("B1"), Some(ObjectKind::Boss { on_enter_spawn: true, .. })));
        // The chest rolls its loot from the stamped distance.
        assert!(matches!(d.objects.get("vault"), Some(ObjectKind::Chest { loot: ChestLoot::Rolled, .. })));
    }

    #[test]
    fn stair_is_placed_on_two_floors_and_linked() {
        let d = parse_str(FOREST_BARROW).unwrap();
        let ps: Vec<&Placement> = d.placements_of("S1").collect();
        assert_eq!(ps.len(), 2, "one down + one up");
        assert!(ps.iter().any(|p| p.dir == Some(StairDir::Down) && p.floor == 0));
        assert!(ps.iter().any(|p| p.dir == Some(StairDir::Up) && p.floor == 1));
    }

    // --- the condition grammar ---

    #[test]
    fn condition_grammar_parses_every_form() {
        assert_eq!(parse_condition("L1").unwrap(), Condition::Ref("L1".into()));
        assert_eq!(parse_condition("has_key(K1)").unwrap(), Condition::HasKey("K1".into()));
        assert_eq!(parse_condition("boss_dead(B1)").unwrap(), Condition::BossDead("B1".into()));
        assert_eq!(parse_condition("seq[P1,P2,P3]").unwrap(), Condition::Seq(vec!["P1".into(), "P2".into(), "P3".into()]));
        assert!(matches!(parse_condition("all[L1, not L2]").unwrap(), Condition::All(_)));
        assert!(matches!(parse_condition("count(2,[a,b,c])").unwrap(), Condition::Count(2, _)));
        // whitespace tolerance + nesting
        assert!(parse_condition(" any[ all[L1,L2] , not has_key(K1) ] ").is_ok());
    }

    #[test]
    fn condition_grammar_rejects_junk() {
        assert!(parse_condition("all[L1").is_err(), "unclosed bracket");
        assert!(parse_condition("count(5,[a,b])").is_err(), "n exceeds options");
        assert!(parse_condition("L1 L2").is_err(), "trailing input");
        assert!(parse_condition("").is_err(), "empty");
    }

    #[test]
    fn condition_eval_matches_the_active_set() {
        use std::collections::HashSet;
        let c = parse_condition("all[L1, count(2,[P1,P2,P3]), not L2]").unwrap();
        let active: HashSet<Id> = ["L1", "P1", "P2"].iter().map(|s| s.to_string()).collect();
        assert!(c.eval(&active));
        let with_l2: HashSet<Id> = ["L1", "P1", "P2", "L2"].iter().map(|s| s.to_string()).collect();
        assert!(!c.eval(&with_l2), "not L2 fails once L2 is active");
    }

    // --- parse-time failures ---

    #[test]
    fn unknown_glyph_is_rejected() {
        let src = r#"
name = "x"
biome = "forest"
[[floor]]
grid = """
#####
#>Q<#
#####
"""
"#;
        assert!(matches!(parse_str(src), Err(DungeonError::UnknownGlyph { glyph: 'Q', .. })));
    }

    #[test]
    fn param_object_without_its_table_is_rejected() {
        // A trap glyph with no [trap.T1] table.
        let src = r#"
name = "x"
biome = "forest"
[legend]
t = "trap T1"
[[floor]]
grid = """
#####
#>t<#
#####
"""
"#;
        assert!(matches!(parse_str(src), Err(DungeonError::BadTable { .. })));
    }

    // --- semantic failures caught by validate() ---

    #[test]
    fn missing_entrance_is_reported() {
        let src = r#"
name = "x"
biome = "forest"
[[floor]]
grid = """
#####
#..<#
#####
"""
"#;
        let errs = validate(&parse_str(src).unwrap());
        assert!(errs.iter().any(|e| matches!(e, DungeonError::EntranceCount { found: 0 })));
    }

    #[test]
    fn dangling_condition_reference_is_reported() {
        // Gate references a lever that doesn't exist.
        let src = r#"
name = "x"
biome = "forest"
[legend]
Y = "gate G1"
[gate.G1]
when = "L9"
[[floor]]
grid = """
######
#>.Y<#
######
"""
"#;
        let errs = validate(&parse_str(src).unwrap());
        assert!(errs.iter().any(|e| matches!(e, DungeonError::UnknownRef { referenced, .. } if referenced == "L9")));
    }

    #[test]
    fn type_mismatched_reference_is_reported() {
        // has_key(L1) but L1 is a lever, not a key.
        let src = r#"
name = "x"
biome = "forest"
[legend]
a = "lever L1"
D = "door D1"
[door.D1]
when = "has_key(L1)"
[[floor]]
grid = """
########
#>.a.D<#
########
"""
"#;
        let errs = validate(&parse_str(src).unwrap());
        assert!(errs.iter().any(|e| matches!(e, DungeonError::TypeMismatch { .. })));
    }

    // --- the solvability gate ---

    #[test]
    fn a_door_gated_by_a_lever_behind_it_is_unsolvable() {
        // The only path to the exit runs through door D1, but the lever that opens
        // D1 sits *past* D1 — a deadlock the fixpoint must catch.
        let src = r#"
name = "deadlock"
biome = "forest"
[legend]
D = "door D1"
a = "lever L1"
[door.D1]
when = "L1"
[[floor]]
grid = """
##########
#>.D.a..<#
##########
"""
"#;
        let errs = validate(&parse_str(src).unwrap());
        assert!(errs.iter().any(|e| matches!(e, DungeonError::Unsolvable { .. })), "got: {errs:?}");
    }

    #[test]
    fn a_lever_before_its_door_is_solvable() {
        // Same shape, lever BEFORE the door → solvable.
        let src = r#"
name = "ok"
biome = "forest"
[legend]
D = "door D1"
a = "lever L1"
[door.D1]
when = "L1"
[[floor]]
grid = """
##########
#>.a.D..<#
##########
"""
"#;
        assert!(validate(&parse_str(src).unwrap()).is_empty());
    }

    #[test]
    fn exit_walled_off_is_unsolvable() {
        let src = r#"
name = "walled"
biome = "forest"
[[floor]]
grid = """
#######
#>.#.<#
#######
"""
"#;
        let errs = validate(&parse_str(src).unwrap());
        assert!(errs.iter().any(|e| matches!(e, DungeonError::Unsolvable { .. })));
    }

    #[test]
    fn solvability_crosses_floors_via_stairs() {
        // Entrance on floor 0, exit only reachable by taking the stairs down.
        let src = r#"
name = "twofloor"
biome = "forest"
[legend]
s = "stair S1 down"
w = "stair S1 up"
[[floor]]
grid = """
#######
#>...s#
#######
"""
[[floor]]
grid = """
#######
#w...<#
#######
"""
"#;
        assert!(validate(&parse_str(src).unwrap()).is_empty());
    }

    #[test]
    fn a_backwards_stair_pairing_is_rejected() {
        // 'up' must be one floor deeper than 'down'.
        let src = r#"
name = "badstair"
biome = "forest"
[legend]
w = "stair S1 up"
s = "stair S1 down"
[[floor]]
grid = """
#######
#>..w<#
#######
"""
[[floor]]
grid = """
#######
#..s..#
#######
"""
"#;
        let errs = validate(&parse_str(src).unwrap());
        assert!(errs.iter().any(|e| matches!(e, DungeonError::BadStair { .. })));
    }

    /// `seq[P1,P1]` is refused: consecutive presses of one emitter count ONCE, so that
    /// step could never be satisfied — and the solvability search, which reads `seq`
    /// order-agnostically, would have called the dungeon solvable anyway. It is the only
    /// way ordering can seal a party in, so the shape is rejected rather than searched.
    #[test]
    fn a_sequence_cannot_ask_for_the_same_press_twice_running() {
        let src = r#"
name = "doublepress"
biome = "forest"
[legend]
"1" = "plate P1 latching"
"2" = "plate P2 latching"
Y = "gate G1"
[gate.G1]
when = "seq[P1,P1,P2]"
[[floor]]
grid = """
###########
#>.1.2.Y.<#
###########
"""
"#;
        let errs = validate(&parse_str(src).unwrap());
        assert!(
            errs.iter().any(|e| matches!(e, DungeonError::BadCondition { .. })),
            "expected the doubled press to be refused, got {errs:?}"
        );
    }

    /// `not` is refused on a LATCHING barrier. `open` never shrinks for a door or a gate,
    /// so `when = "not L1"` reads as "shut once the lever is pulled" and does the opposite
    /// — opens at the start, stays open forever. Nothing said so, and no content had tried
    /// it, which is the only reason the lie went unnoticed.
    #[test]
    fn not_is_refused_on_a_door_and_allowed_on_a_mover() {
        let door = r#"
name = "latchlie"
biome = "forest"
[legend]
a = "lever L1"
X = "door D1"
[door.D1]
when = "not L1"
[[floor]]
grid = """
############
#>.a.X....<#
############
"""
"#;
        let errs = validate(&parse_str(door).unwrap());
        assert!(errs.iter().any(|e| matches!(e, DungeonError::BadCondition { .. })), "{errs:?}");

        // The same wiring on a MOVER is exactly what a mover is for — and here the exit is
        // reachable without ever passing the mover, so nothing can be sealed in.
        let mover = r#"
name = "dropwall"
biome = "forest"
[legend]
a = "lever L1"
M = "mover MV1"
[mover.MV1]
when = "not L1"
[[floor]]
grid = """
############
#>.a.M.....#
#..........#
#<.........#
############
"""
"#;
        assert!(validate(&parse_str(mover).unwrap()).is_empty(), "{:?}", validate(&parse_str(mover).unwrap()));
    }

    /// A MOVER MUST NOT BE ABLE TO SEAL A PARTY IN. A dungeon takes no Town Portal, so a
    /// wall dropping behind you is the one thing here that could end a run with no way out
    /// but dying — and the gate proves the worst case survivable rather than trusting the
    /// author.
    #[test]
    fn a_mover_that_could_seal_the_exit_away_is_refused() {
        // The ONLY route to the exit runs through the mover, and pulling the lever shuts
        // it. Stand past it, pull nothing, and you are fine — but the gate refuses the
        // shape rather than relying on that.
        let src = r#"
name = "sealed"
biome = "forest"
[legend]
a = "lever L1"
M = "mover MV1"
[mover.MV1]
when = "not L1"
[[floor]]
grid = """
##############
#>.a....M...<#
##############
"""
"#;
        let errs = validate(&parse_str(src).unwrap());
        assert!(
            errs.iter().any(|e| matches!(e, DungeonError::Unsolvable { .. })),
            "a closing mover on the only route out must be refused, got {errs:?}"
        );
    }

    /// A PEDESTAL is a remote effect: fetch the idol, carry it here, and something opens
    /// SOMEWHERE ELSE. Standing on it empty-handed does nothing, and the search knows it.
    #[test]
    fn a_pedestal_wants_its_idol_before_it_opens_anything() {
        let src = r#"
name = "idol"
biome = "forest"
[legend]
k = "key K1"
o = "pedestal PD1"
X = "door D1"
[pedestal.PD1]
wants = "K1"
[door.D1]
when = "PD1"
[[floor]]
grid = """
###############
#>..o..X....<##
#.............#
#....k........#
###############
"""
"#;
        let d = parse_str(src).unwrap();
        assert!(validate(&d).is_empty(), "{:?}", validate(&d));

        // The same tomb with the idol walled away: the pedestal is reachable, the door is
        // not openable, and the gate says so rather than shipping a party into a room they
        // cannot leave.
        let sealed = src.replace("#.............#\n#....k........#", "###############\n#####k#########");
        let errs = validate(&parse_str(&sealed).unwrap());
        assert!(
            errs.iter().any(|e| matches!(e, DungeonError::Unsolvable { .. })),
            "an unreachable idol makes its pedestal unsatisfiable, got {errs:?}"
        );
    }

    /// `wants` has to name a KEY. Pointing it at a lever would otherwise surface as
    /// "the barrier behind this pedestal can never open" — true, and unhelpful.
    #[test]
    fn a_pedestal_must_want_a_key() {
        let src = r#"
name = "badidol"
biome = "forest"
[legend]
a = "lever L1"
o = "pedestal PD1"
[pedestal.PD1]
wants = "L1"
[[floor]]
grid = """
############
#>.a.o....<#
############
"""
"#;
        let errs = validate(&parse_str(src).unwrap());
        assert!(errs.iter().any(|e| matches!(e, DungeonError::TypeMismatch { .. })), "{errs:?}");
    }

    /// A teleporter may land you on the SAME floor — the constraint a stair has and this
    /// deliberately drops. That is what makes it a maze tool (Wizardry, Etrian, Grimrock,
    /// Silph Co.) rather than a way between levels.
    #[test]
    fn a_teleporter_may_link_two_cells_of_one_floor() {
        let src = r#"
name = "padmaze"
biome = "forest"
[legend]
a = "teleporter TP1 from"
b = "teleporter TP1 to"
[teleporter.TP1]
[[floor]]
grid = """
############
#>.a####b.<#
############
"""
"#;
        let d = parse_str(src).unwrap();
        assert!(validate(&d).is_empty(), "{:?}", validate(&d));
    }

    /// A ONE-WAY pad is honoured by the solvability search, which is the only reason it is
    /// safe to author one: the search's link map is directed, so it cannot certify an exit
    /// reachable only by walking backwards through a pad a player cannot walk back through.
    #[test]
    fn a_one_way_pad_is_a_one_way_route_to_the_search() {
        // The exit sits behind the pad's landing side, walled off from the entrance —
        // reachable only by taking the pad. Fine.
        let ok = r#"
name = "oneway_ok"
biome = "forest"
[legend]
a = "teleporter TP1 from"
b = "teleporter TP1 to"
[teleporter.TP1]
one_way = true
[[floor]]
grid = """
############
#>.a##b..<##
############
"""
"#;
        assert!(validate(&parse_str(ok).unwrap()).is_empty());

        // Reversed: the ENTRANCE is on the landing side and the exit behind the entry pad,
        // so the only route runs backwards through a one-way pad. Must be refused.
        let bad = r#"
name = "oneway_bad"
biome = "forest"
[legend]
a = "teleporter TP1 from"
b = "teleporter TP1 to"
[teleporter.TP1]
one_way = true
[[floor]]
grid = """
############
#.a<##b..>##
############
"""
"#;
        let errs = validate(&parse_str(bad).unwrap());
        assert!(
            errs.iter().any(|e| matches!(e, DungeonError::Unsolvable { .. })),
            "walking a one-way pad backwards is not a route, got {errs:?}"
        );
    }

    /// `room_clear` gates a door on a fight, and the search proves the door can open —
    /// the same "reachable means beatable" argument a boss already gets.
    #[test]
    fn a_room_clear_door_opens_behind_a_spawn() {
        let src = r#"
name = "guardroom"
biome = "forest"
[legend]
g = "spawn S1"
X = "door D1"
[spawn.S1]
count = 3
[door.D1]
when = "room_clear(S1)"
[[floor]]
grid = """
##########
#>.g.X..<#
##########
"""
"#;
        let d = parse_str(src).unwrap();
        assert_eq!(d.objects.get("S1"), Some(&ObjectKind::Spawn { count: 3 }));
        assert!(validate(&d).is_empty(), "{:?}", validate(&d));
        // A spawn costs no BODIES — it is a fight, not a plate someone has to stand on.
        assert_eq!(d.bodies_required(), 1);
    }

    /// `room_clear(…)` must name something a party can FIGHT. Pointing it at a chest used
    /// to parse and then fail the solvability search with "these barriers can never open"
    /// — a true error naming the wrong cause.
    #[test]
    fn room_clear_must_name_something_clearable() {
        let src = r#"
name = "badclear"
biome = "forest"
[legend]
C = "chest loot"
X = "door D1"
[chest.loot]
loot = "rolled"
[door.D1]
when = "room_clear(loot)"
[[floor]]
grid = """
##########
#>.C.X..<#
##########
"""
"#;
        let errs = validate(&parse_str(src).unwrap());
        assert!(
            errs.iter().any(|e| matches!(e, DungeonError::TypeMismatch { .. })),
            "expected a type mismatch naming the cause, got {errs:?}"
        );
    }

    /// A spawn of nobody is a room that clears itself.
    #[test]
    fn a_spawn_of_zero_is_refused() {
        let src = r#"
name = "empty"
biome = "forest"
[legend]
g = "spawn S1"
[spawn.S1]
count = 0
[[floor]]
grid = """
#########
#>.g...<#
#########
"""
"#;
        assert!(parse_str(src).is_err());
    }

    /// …and the same ids NOT adjacent are fine, because an unrelated press separates them.
    #[test]
    fn a_sequence_may_revisit_an_emitter_with_something_in_between() {
        let src = r#"
name = "revisit"
biome = "forest"
[legend]
"1" = "plate P1 latching"
"2" = "plate P2 latching"
Y = "gate G1"
[gate.G1]
when = "seq[P1,P2,P1]"
[[floor]]
grid = """
###########
#>.1.2.Y.<#
###########
"""
"#;
        assert!(validate(&parse_str(src).unwrap()).is_empty());
    }
}
