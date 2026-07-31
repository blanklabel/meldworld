//! The parsed, validated shape of an authored dungeon.
//!
//! A dungeon is a stack of **floors** (each a fixed-width glyph grid) plus a set
//! of **objects** (levers, plates, doors, gates, traps, keys, a boss, chests,
//! stairs) placed on those grids and wired together by [`Condition`]s. This module
//! is pure data + the `Condition` semantics; parsing lives in [`crate::parse`] and
//! checking in [`crate::validate`]. See `docs/proposals/dungeons.md` (DG-1).

use std::collections::HashSet;

use serde::{Deserialize, Serialize};

/// An author-assigned object id, unique within a dungeon (e.g. `"L1"`, `"vault"`).
pub type Id = String;

/// A single grid cell's base terrain. Interactive objects sit *on* a `Floor` cell
/// (the cell's `object` names them); a `Door`/`Gate` object makes its floor cell
/// passable only while open.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Tile {
    /// Impassable wall.
    Wall,
    /// Walkable ground.
    Floor,
    /// Outside the dungeon (padding / negative space) — impassable, not rendered.
    Void,
}

/// Which way a stair endpoint leads. A stair id has exactly one `Down` endpoint on
/// floor `n` and one `Up` endpoint on floor `n+1`; stepping either transitions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum StairDir {
    Down,
    Up,
}

/// One grid cell: base terrain plus the id of any object occupying it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Cell {
    pub tile: Tile,
    pub object: Option<Id>,
}

/// One floor: a `width × height` row-major grid of [`Cell`]s.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Grid {
    pub width: usize,
    pub height: usize,
    pub cells: Vec<Cell>,
}

impl Grid {
    pub fn at(&self, x: usize, y: usize) -> &Cell {
        &self.cells[y * self.width + x]
    }
}

/// Where an object sits. Most objects have exactly one placement; a stair has two
/// (a `Down` on floor `n`, an `Up` on floor `n+1`), sharing one `id`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Placement {
    pub id: Id,
    pub floor: usize,
    pub x: usize,
    pub y: usize,
    /// `Some` only for stair endpoints.
    pub dir: Option<StairDir>,
}

/// One item a chest yields when it holds *authored* (designer-defined) contents.
/// Exactly one of `item`/`gear` is set (checked in validation).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChestItem {
    pub item: Option<String>,
    pub gear: Option<String>,
    pub quantity: u32,
}

/// How a chest is filled (design decision §6: "generated *and* defined").
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ChestLoot {
    /// Rolled at open time from the dungeon's stamped effective distance.
    Rolled,
    /// Fixed designer-authored contents; no roll.
    Authored(Vec<ChestItem>),
    /// Guaranteed authored contents **plus** a rolled bonus.
    Hybrid(Vec<ChestItem>),
}

/// The type + parameters of an object. Placement (which floor/cell) is tracked
/// separately in [`Placement`]s so a stair can occupy two cells under one id.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ObjectKind {
    /// A lever: an emitter that latches on when flipped (active once reached).
    Lever,
    /// A pressure plate: emitter active while stood on (`momentary`) or latching.
    Plate { momentary: bool },
    /// A carried key: an emitter satisfying `has_key(id)` once picked up.
    Key,
    /// An item sink ("place the idol here"); emitter active once used.
    Pedestal,
    /// A hazard. Fires on contact while armed; `disarmable` ones can be neutralised
    /// via a Dex/Shifter check (design decision §5). Not a movement barrier.
    Trap { kind: String, disarmable: bool },
    /// A barrier that opens when `when` holds.
    Door { when: Condition },
    /// A barrier that opens when `when` holds (a `Gate` is a `Door` by another
    /// name — kept distinct for authoring clarity / bigger presentation).
    Gate { when: Condition },
    /// The dungeon boss. `on_enter_spawn` reveals it when its room is entered.
    Boss { sprite: String, on_enter_spawn: bool },
    /// Treasure. Openable when `when` holds (or always, if `None`).
    Chest { when: Option<Condition>, loot: ChestLoot },
    /// A stair linking two floors. Its two endpoints (a `Down` on floor `n`, an
    /// `Up` on floor `n+1`) share this id; the direction lives on each
    /// [`Placement`], so both endpoints register the same `Stair` kind.
    Stair,
}

impl ObjectKind {
    /// A movement barrier while closed (needs its condition satisfied to pass).
    pub fn is_barrier(&self) -> bool {
        matches!(self, ObjectKind::Door { .. } | ObjectKind::Gate { .. })
    }

    /// The `when` condition of a barrier / conditional chest, if any.
    pub fn condition(&self) -> Option<&Condition> {
        match self {
            ObjectKind::Door { when } | ObjectKind::Gate { when } => Some(when),
            ObjectKind::Chest { when, .. } => when.as_ref(),
            _ => None,
        }
    }

    /// An emitter that becomes active simply by being reached (lever/plate/key/
    /// pedestal/boss). Used by the solvability search's fixpoint.
    pub fn activates_on_reach(&self) -> bool {
        matches!(
            self,
            ObjectKind::Lever
                | ObjectKind::Plate { .. }
                | ObjectKind::Key
                | ObjectKind::Pedestal
                | ObjectKind::Boss { .. }
        )
    }
}

/// A boolean over emitter states — the puzzle-wiring grammar (design §"Puzzle
/// vocabulary"). Parsed from a `when = "…"` string by [`crate::parse`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Condition {
    /// Bare atom `id` — the object (a lever/plate) is active.
    Ref(Id),
    /// `has_key(id)` — the key has been picked up.
    HasKey(Id),
    /// `boss_dead(id)` — the boss has been defeated.
    BossDead(Id),
    /// `room_clear(id)` — a tagged room/group has been cleared.
    RoomClear(Id),
    /// `not c`.
    Not(Box<Condition>),
    /// `all[c, c, …]` — every sub-condition holds.
    All(Vec<Condition>),
    /// `any[c, c, …]` — at least one holds.
    Any(Vec<Condition>),
    /// `seq[id, id, …]` — the emitters were triggered in this order. (Solvability
    /// treats this as "all present"; ordering is a runtime concern — see DG-1.)
    Seq(Vec<Id>),
    /// `count(n, [c, c, …])` — at least `n` of the sub-conditions hold.
    Count(usize, Vec<Condition>),
}

impl Condition {
    /// Evaluate against the set of currently-active emitter ids. `has_key` /
    /// `boss_dead` / `room_clear` share the same id-membership space as `Ref`
    /// (a key is "active" once held, a boss once dead) — see the solvability search.
    pub fn eval(&self, active: &HashSet<Id>) -> bool {
        match self {
            Condition::Ref(id)
            | Condition::HasKey(id)
            | Condition::BossDead(id)
            | Condition::RoomClear(id) => active.contains(id),
            Condition::Not(c) => !c.eval(active),
            Condition::All(cs) => cs.iter().all(|c| c.eval(active)),
            Condition::Any(cs) => cs.iter().any(|c| c.eval(active)),
            Condition::Seq(ids) => ids.iter().all(|id| active.contains(id)),
            Condition::Count(n, cs) => cs.iter().filter(|c| c.eval(active)).count() >= *n,
        }
    }

    /// Every object id this condition names — with the predicate that constrains
    /// the referent's type (for validation: `has_key` must name a `Key`, etc.).
    pub fn referenced(&self, out: &mut Vec<(Id, RefKind)>) {
        match self {
            Condition::Ref(id) => out.push((id.clone(), RefKind::Activatable)),
            Condition::HasKey(id) => out.push((id.clone(), RefKind::Key)),
            Condition::BossDead(id) => out.push((id.clone(), RefKind::Boss)),
            Condition::RoomClear(id) => out.push((id.clone(), RefKind::Any)),
            Condition::Not(c) => c.referenced(out),
            Condition::All(cs) | Condition::Any(cs) => cs.iter().for_each(|c| c.referenced(out)),
            Condition::Seq(ids) => out.extend(ids.iter().map(|i| (i.clone(), RefKind::Activatable))),
            Condition::Count(_, cs) => cs.iter().for_each(|c| c.referenced(out)),
        }
    }
}

/// What type a condition expects the id it names to be — used by validation to
/// reject `has_key(L1)` when `L1` is a lever, etc.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefKind {
    /// A lever or plate (something a bare `Ref` or `seq[…]` element can activate).
    Activatable,
    Key,
    Boss,
    /// Any object (used by `room_clear`, which is intentionally permissive).
    Any,
}

/// A fully parsed dungeon, ready to validate (and, later, to place at runtime).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DungeonDef {
    pub name: String,
    pub biome: String,
    /// One grid per floor, index 0 = the entrance level.
    pub grids: Vec<Grid>,
    /// Object id → its type/params.
    pub objects: std::collections::BTreeMap<Id, ObjectKind>,
    /// Every interactive-object placement (a stair appears twice).
    pub placements: Vec<Placement>,
    /// Overworld-entry cells (validation requires exactly one, on floor 0).
    pub entrances: Vec<Placement>,
    /// End-exit cells back to the overworld (validation requires ≥ 1).
    pub exits: Vec<Placement>,
}

impl DungeonDef {
    /// All placements of `id` (one for most objects, two for a stair).
    pub fn placements_of<'a>(&'a self, id: &'a str) -> impl Iterator<Item = &'a Placement> + 'a {
        self.placements.iter().filter(move |p| p.id == id)
    }
}
