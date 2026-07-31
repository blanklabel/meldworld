//! Errors from parsing or validating an authored dungeon. In DG-2 these become
//! `build.rs` compile errors — the correctness gate for agent-authored content.

/// A single problem with a dungeon file. Parsing returns the first fatal one;
/// [`crate::validate`] collects all semantic + solvability issues into a `Vec`.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DungeonError {
    #[error("TOML parse error: {0}")]
    Toml(String),

    #[error("floor {floor}: grid is empty")]
    EmptyFloor { floor: usize },

    #[error("unknown glyph {glyph:?} at floor {floor} ({x},{y}) — add it to [legend] or use a structural char (#/./space/>/<)")]
    UnknownGlyph { glyph: char, floor: usize, x: usize, y: usize },

    #[error("legend entry {glyph:?}: {reason}")]
    BadLegend { glyph: char, reason: String },

    #[error("object {id:?}: bad table: {reason}")]
    BadTable { id: String, reason: String },

    #[error("condition for {id:?}: {reason}")]
    BadCondition { id: String, reason: String },

    #[error("exactly one entrance ('>') required, found {found}")]
    EntranceCount { found: usize },

    #[error("the single entrance must be on floor 0, found it on floor {floor}")]
    EntranceFloor { floor: usize },

    #[error("at least one exit ('<') required, found {found}")]
    ExitCount { found: usize },

    #[error("object {id:?} is placed {count} times — each needs exactly one cell")]
    DuplicatePlacement { id: String, count: usize },

    #[error("object {id:?} is declared but never placed in any grid")]
    Unplaced { id: String },

    #[error("{id:?} references unknown object {referenced:?}")]
    UnknownRef { id: String, referenced: String },

    #[error("{id:?} references {referenced:?}, which is the wrong type: {reason}")]
    TypeMismatch { id: String, referenced: String, reason: String },

    #[error("stair {id:?}: {reason}")]
    BadStair { id: String, reason: String },

    #[error("chest {id:?}: {reason}")]
    BadChest { id: String, reason: String },

    #[error("unsolvable: no route from the entrance reaches an exit ({reason})")]
    Unsolvable { reason: String },
}
