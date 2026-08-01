//! Dungeon runtime engine (DG-3a) — the live subinstance, as a pure state machine.
//!
//! This is the runtime half of the dungeon feature: a [`DungeonInstance`] is one
//! **live** instantiation of an authored [`DungeonDef`] (from the compiled pool),
//! shared by the group that entered it, with the barrier/emitter **puzzle state**,
//! **stairs** between floors, the **end-exit**, and the **committed-space** rules
//! (no Town Portal; you leave by the exit or by dying). Plus seeded **entrance
//! placement** — as the overworld streams, a section rolls whether it hosts a
//! dungeon entrance and which dungeon from its biome's pool.
//!
//! Like `meld-battle` / `meld-world`, it is **pure and deterministic** — no I/O, no
//! wall-clock, seeded RNG only — so it is exhaustively unit-testable. The game loop
//! (**DG-3b**, once **SC-3**'s `WorldActor` lands) owns [`DungeonInstance`]s inside
//! a world's single task and drives them; the client render is **DG-6**. See
//! `docs/proposals/dungeons.md`.
//!
//! ```
//! # use meld_dungeon_run::*;
//! // A section with a forest dungeon in its pool, spawn chance 1.0, always rolls one.
//! let roll = roll_entrance(42, "forest", 1.0).unwrap();
//! let def = meld_dungeon_content::by_name(roll.dungeon).unwrap();
//! let mut d = DungeonInstance::new(1, def, /*level*/ 250, /*depth_step*/ 20);
//! let spawn = d.enter("p1");
//! assert_eq!(d.occupant("p1").unwrap().floor, 0);
//! let _ = spawn;
//! ```

use std::collections::{HashMap, HashSet};

use meld_balance::Balance;
use meld_dungeon_content::{ChestItem, ChestLoot, DungeonDef, Id, ObjectKind, StairDir, Tile};
use meld_proto::common::Position;
use meld_world::{roll_creature_loot, CreatureLoot};

/// A unique id for a live dungeon subinstance. Minted by the driver (DG-3b) — the
/// engine never generates one, so many fresh copies of the same dungeon coexist
/// (per-entry-fresh — design §3).
pub type DungeonKey = u64;

/// Where a player is. The overworld, or inside a specific live dungeon on a
/// specific floor. The driver keeps one of these per session and scopes movement /
/// touch / snapshot to it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Location {
    Overworld,
    InDungeon { key: DungeonKey, floor: usize },
}

impl Location {
    pub fn in_dungeon(&self) -> Option<(DungeonKey, usize)> {
        match self {
            Location::InDungeon { key, floor } => Some((*key, *floor)),
            Location::Overworld => None,
        }
    }
}

/// One player's presence inside a dungeon: which floor, and where on it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Occupant {
    pub floor: usize,
    pub pos: Position,
}

/// The centre of grid cell `(x, y)` in continuous dungeon-local coordinates
/// (1 cell = 1.0 unit), matching how the overworld places entities on a plane.
pub fn cell_center(x: usize, y: usize) -> Position {
    Position { x: x as f64 + 0.5, y: y as f64 + 0.5 }
}

/// The grid cell a position falls in, or `None` if it is off-grid (negative).
fn cell_of(p: Position) -> Option<(usize, usize)> {
    if p.x < 0.0 || p.y < 0.0 {
        return None;
    }
    Some((p.x as usize, p.y as usize))
}

/// A live dungeon subinstance.
///
/// Holds the shared, mutable **puzzle state** (`active` emitters, `open` barriers),
/// the `occupants`, and the stamped difficulty (`level` + per-floor `depth_step`,
/// design §6). Traps' armed/disarm state and damage are **DG-4**; here traps are
/// non-blocking floor. Movement collision / interaction *timing* are the driver's
/// (DG-3b); this exposes the queries + transitions it needs.
pub struct DungeonInstance<'a> {
    pub key: DungeonKey,
    def: &'a DungeonDef,
    /// The stamped effective distance for floor 0 (the entrance's overworld
    /// distance-from-origin), the difficulty/loot axis. See [`Self::effective_distance`].
    level: i64,
    /// How much each descended floor adds to the effective distance.
    depth_step: i64,
    /// Emitter ids that are active (lever flipped / plate held / key held / boss
    /// dead). Monotone — grows as the group progresses.
    active: HashSet<Id>,
    /// Barrier ids (doors/gates) currently open. Monotone.
    open: HashSet<Id>,
    occupants: HashMap<String, Occupant>,
    /// Cell → its stair partner cell on the neighbouring floor (both directions).
    stair_links: HashMap<(usize, usize, usize), (usize, usize, usize)>,
    /// Trap id → armed/disarmed (DG-4). Every trap starts armed; disarming (or a
    /// future `trap.disarm` signal) makes it inert.
    traps: HashMap<Id, TrapState>,
    /// Chest ids already looted (DG-5/C) — a chest opens once.
    opened_chests: HashSet<Id>,
}

impl<'a> DungeonInstance<'a> {
    /// Instantiate `def` as a fresh live subinstance. `level` is the stamped
    /// effective distance for floor 0 (compute it from the entrance's overworld
    /// `distance_floor()` at entry); `depth_step` is the per-floor difficulty bump.
    pub fn new(key: DungeonKey, def: &'a DungeonDef, level: i64, depth_step: i64) -> Self {
        DungeonInstance {
            key,
            def,
            level,
            depth_step,
            active: HashSet::new(),
            open: HashSet::new(),
            occupants: HashMap::new(),
            stair_links: build_stair_links(def),
            traps: def
                .objects
                .iter()
                .filter(|(_, k)| matches!(k, ObjectKind::Trap { .. }))
                .map(|(id, _)| (id.clone(), TrapState::Armed))
                .collect(),
            opened_chests: HashSet::new(),
        }
    }

    /// Whether chest `id` can be looted right now: it is a chest, its `when`
    /// condition (if any) is satisfied by the active set (e.g. `boss_dead(...)`
    /// after the boss falls), and it hasn't been opened yet (DG-5/C).
    pub fn chest_openable(&self, id: &str) -> bool {
        if self.opened_chests.contains(id) {
            return false;
        }
        match self.def.objects.get(id) {
            Some(ObjectKind::Chest { when, .. }) => when.as_ref().is_none_or(|c| c.eval(&self.active)),
            _ => false,
        }
    }

    /// Mark chest `id` looted; returns `false` if it wasn't openable (sealed /
    /// already looted / not a chest).
    pub fn open_chest(&mut self, id: &str) -> bool {
        if !self.chest_openable(id) {
            return false;
        }
        self.opened_chests.insert(id.to_string());
        true
    }

    pub fn def(&self) -> &DungeonDef {
        self.def
    }

    pub fn name(&self) -> &str {
        &self.def.name
    }

    /// **Committed space** (design §4): there is no Town Portal inside a dungeon.
    /// The driver rejects `begin_extraction` while a player is `InDungeon`.
    pub fn town_portal_allowed(&self) -> bool {
        false
    }

    // --- occupancy -------------------------------------------------------

    /// Place a player at the entrance (floor 0). Returns their spawn position.
    /// The driver calls this for each member of the entering group.
    pub fn enter(&mut self, player_id: &str) -> Position {
        let e = &self.def.entrances[0];
        let pos = cell_center(e.x, e.y);
        self.occupants.insert(player_id.to_string(), Occupant { floor: e.floor, pos });
        pos
    }

    pub fn remove(&mut self, player_id: &str) -> Option<Occupant> {
        self.occupants.remove(player_id)
    }

    pub fn occupant(&self, player_id: &str) -> Option<&Occupant> {
        self.occupants.get(player_id)
    }

    pub fn occupants(&self) -> impl Iterator<Item = (&String, &Occupant)> {
        self.occupants.iter()
    }

    /// No one left inside — the driver despawns the instance (per-entry-fresh, so
    /// nothing persists; contrast SC-3's hibernating overworld shards).
    pub fn is_empty(&self) -> bool {
        self.occupants.is_empty()
    }

    /// Update a player's floor/position (after a validated move). No-op if absent.
    pub fn set_pos(&mut self, player_id: &str, floor: usize, pos: Position) {
        if let Some(o) = self.occupants.get_mut(player_id) {
            o.floor = floor;
            o.pos = pos;
        }
    }

    /// Move the occupant by direction `(dx, dy)` up to `step` tiles on their floor,
    /// **sliding along walls** (axis-separated) so a corridor turn doesn't stick.
    /// Cell-granular collision via [`Self::walkable`] (a closed door blocks; an open
    /// one passes). Updates + returns the new position; `None` if not an occupant.
    /// The driver computes `step = speed × sim_dt` to match overworld feel.
    pub fn try_move(&mut self, player_id: &str, dx: f64, dy: f64, step: f64) -> Option<Position> {
        let occ = *self.occupants.get(player_id)?;
        let (floor, cur) = (occ.floor, occ.pos);
        let mag = (dx * dx + dy * dy).sqrt();
        let (nx, ny) = if mag > 1.0 { (dx / mag, dy / mag) } else { (dx, dy) };
        let full = Position { x: cur.x + nx * step, y: cur.y + ny * step };
        let dest = if self.walkable(floor, full) {
            full
        } else {
            // Slide: keep whichever single-axis component clears a wall.
            let sx = Position { x: full.x, y: cur.y };
            let sy = Position { x: cur.x, y: full.y };
            if self.walkable(floor, sx) {
                sx
            } else if self.walkable(floor, sy) {
                sy
            } else {
                cur
            }
        };
        self.occupants.insert(player_id.to_string(), Occupant { floor, pos: dest });
        Some(dest)
    }

    // --- difficulty ------------------------------------------------------

    /// The effective distance to scale a `floor`'s mobs / traps / rolled loot by:
    /// `level + floor × depth_step` (design §6). Deeper = harder *and* richer.
    pub fn effective_distance(&self, floor: usize) -> i64 {
        self.level + floor as i64 * self.depth_step
    }

    // --- spatial queries -------------------------------------------------

    /// Is `pos` on `floor` walkable? `Floor` cells are, unless occupied by a
    /// *closed* door/gate. Walls, void, and off-grid are not. Traps are walkable
    /// (a hazard, not a barrier).
    pub fn walkable(&self, floor: usize, pos: Position) -> bool {
        let Some(grid) = self.def.grids.get(floor) else { return false };
        let Some((x, y)) = cell_of(pos) else { return false };
        if x >= grid.width || y >= grid.height {
            return false;
        }
        let cell = grid.at(x, y);
        if cell.tile != Tile::Floor {
            return false;
        }
        match &cell.object {
            None => true,
            Some(id) => match self.def.objects.get(id) {
                Some(k) if k.is_barrier() => self.open.contains(id),
                _ => true,
            },
        }
    }

    /// The object id on a cell, if any (an emitter to activate, a stair to take…).
    pub fn object_at(&self, floor: usize, pos: Position) -> Option<&Id> {
        let grid = self.def.grids.get(floor)?;
        let (x, y) = cell_of(pos)?;
        if x >= grid.width || y >= grid.height {
            return None;
        }
        grid.at(x, y).object.as_ref()
    }

    /// Is the player standing on an end-exit cell (→ back to the overworld entry)?
    pub fn at_exit(&self, floor: usize, pos: Position) -> bool {
        let Some((x, y)) = cell_of(pos) else { return false };
        self.def.exits.iter().any(|e| e.floor == floor && e.x == x && e.y == y)
    }

    /// If `pos` on `floor` is a stair endpoint, the paired endpoint's
    /// `(floor, centre)` — the destination of "go up/down stairs". Transition is on
    /// contact (like the overworld portal); the driver moves the avatar there.
    pub fn stair_dest(&self, floor: usize, pos: Position) -> Option<(usize, Position)> {
        let (x, y) = cell_of(pos)?;
        let (df, dx, dy) = self.stair_links.get(&(floor, x, y))?;
        Some((*df, cell_center(*dx, *dy)))
    }

    // --- puzzle state ----------------------------------------------------

    /// Mark an emitter active (a reached lever/plate/key/pedestal, or a defeated
    /// boss) and re-open any barrier whose condition now holds. Returns the barrier
    /// ids that newly opened (for the driver to broadcast). No-op for non-emitters
    /// or an already-active id.
    pub fn activate(&mut self, id: &str) -> Vec<Id> {
        let is_emitter = self.def.objects.get(id).is_some_and(|k| k.activates_on_reach());
        if !is_emitter || self.active.contains(id) {
            return Vec::new();
        }
        self.active.insert(id.to_string());
        self.reeval()
    }

    /// Convenience: if a cell holds an activatable emitter, activate it. Returns the
    /// newly-opened barriers. (The driver decides *when* — on reach or on interact.)
    pub fn activate_at(&mut self, floor: usize, pos: Position) -> Vec<Id> {
        match self.object_at(floor, pos).cloned() {
            Some(id) => self.activate(&id),
            None => Vec::new(),
        }
    }

    pub fn is_active(&self, id: &str) -> bool {
        self.active.contains(id)
    }

    pub fn is_open(&self, id: &str) -> bool {
        self.open.contains(id)
    }

    /// Re-evaluate every closed barrier against the current `active` set, opening
    /// those now satisfiable. Monotone, so a single pass per activation suffices
    /// (a newly-opened barrier reveals cells, but activating *those* emitters
    /// re-runs this).
    fn reeval(&mut self) -> Vec<Id> {
        let def = self.def; // copy the &ref so the loop doesn't borrow `self`
        let mut opened = Vec::new();
        for (id, kind) in &def.objects {
            if kind.is_barrier() && !self.open.contains(id) {
                if let Some(c) = kind.condition() {
                    if c.eval(&self.active) {
                        self.open.insert(id.clone());
                        opened.push(id.clone());
                    }
                }
            }
        }
        opened
    }
}

// ---------------------------------------------------------------------------
// DG-4 — traps: armed/disarmed state + the Dex/Shifter disarm check
// ---------------------------------------------------------------------------

/// A trap's live state. Every trap starts [`Armed`](TrapState::Armed); a successful
/// disarm makes it [`Disarmed`](TrapState::Disarmed) (inert). Firing does *not*
/// change state — an armed trap is a persistent hazard until disarmed or routed
/// around (design §5).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrapState {
    Armed,
    Disarmed,
}

/// What a sprung trap does to the party. The driver maps `kind` (the authored
/// `"thorns"` / `"dart"` / `"fire"` …) to a damage type and scales damage by
/// `severity` (the floor's effective distance, so deeper traps bite harder).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrapHit {
    pub kind: String,
    pub severity: i64,
}

/// The result of an attempted disarm.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DisarmOutcome {
    /// The check passed — the trap is now inert.
    Disarmed,
    /// The check failed — the trap fired.
    Sprung(TrapHit),
    /// This trap is authored `disarmable = false`; you must route around it.
    NotDisarmable,
    /// Already inert.
    AlreadyDisarmed,
    /// No trap on that cell.
    NotATrap,
}

impl DungeonInstance<'_> {
    /// A trap's current state, if `id` is a trap.
    pub fn trap_state(&self, id: &str) -> Option<TrapState> {
        self.traps.get(id).copied()
    }

    /// Fire the trap on `(floor, pos)` if one is there and armed — the movement
    /// path (the driver calls this when a player steps onto the cell). Returns the
    /// [`TrapHit`]; `None` if the cell has no armed trap. Does not change state
    /// (armed traps are persistent hazards).
    pub fn spring_trap(&self, floor: usize, pos: Position) -> Option<TrapHit> {
        let id = self.object_at(floor, pos)?;
        if self.traps.get(id) != Some(&TrapState::Armed) {
            return None;
        }
        match self.def.objects.get(id) {
            Some(ObjectKind::Trap { kind, .. }) => {
                Some(TrapHit { kind: kind.clone(), severity: self.effective_distance(floor) })
            }
            _ => None,
        }
    }

    /// Attempt to disarm the trap on `(floor, pos)` — the `run.interact` path. A Dex
    /// check the **Shifter** is far better at (design §5): success probability
    /// `p = dex / dex_divisor (+ shifter_bonus if a Shifter)`, clamped to
    /// `[0.05, 0.95]`. On success the trap is neutralised; **on failure it springs**.
    /// A non-disarmable trap returns [`DisarmOutcome::NotDisarmable`] (no roll).
    /// `dex_divisor` / `shifter_bonus` are driver-supplied tunables.
    #[allow(clippy::too_many_arguments)]
    pub fn attempt_disarm(
        &mut self,
        floor: usize,
        pos: Position,
        dex: i32,
        is_shifter: bool,
        dex_divisor: f64,
        shifter_bonus: f64,
        seed: u64,
    ) -> DisarmOutcome {
        let def = self.def;
        let Some(id) = self.object_at(floor, pos).cloned() else {
            return DisarmOutcome::NotATrap;
        };
        let (kind, disarmable) = match def.objects.get(&id) {
            Some(ObjectKind::Trap { kind, disarmable }) => (kind.clone(), *disarmable),
            _ => return DisarmOutcome::NotATrap,
        };
        match self.traps.get(&id) {
            None => return DisarmOutcome::NotATrap,
            Some(TrapState::Disarmed) => return DisarmOutcome::AlreadyDisarmed,
            Some(TrapState::Armed) => {}
        }
        if !disarmable {
            return DisarmOutcome::NotDisarmable;
        }
        let bonus = if is_shifter { shifter_bonus } else { 0.0 };
        let p = (dex as f64 / dex_divisor + bonus).clamp(0.05, 0.95);
        let mut s = seed;
        if unit(splitmix64(&mut s)) < p {
            self.traps.insert(id, TrapState::Disarmed);
            DisarmOutcome::Disarmed
        } else {
            DisarmOutcome::Sprung(TrapHit { kind, severity: self.effective_distance(floor) })
        }
    }
}

// ---------------------------------------------------------------------------
// DG-5 — chest loot resolution
// ---------------------------------------------------------------------------

/// What a dungeon chest yields when opened. The driver banks `rolled` (material +
/// chits + gear, exactly like an overworld chest) and grants each `authored` item.
#[derive(Debug, Clone, PartialEq)]
pub struct ChestReward {
    /// The effective distance the roll was scaled to (`level + floor × depth_step`).
    pub effective_distance: i64,
    /// The distance-scaled roll (`Rolled` / `Hybrid` chests), else `None`.
    pub rolled: Option<CreatureLoot>,
    /// Designer-authored fixed contents (`Authored` / `Hybrid` chests), else empty.
    pub authored: Vec<ChestItem>,
}

impl DungeonInstance<'_> {
    /// Resolve `chest_id`'s loot (design §6). `Rolled`/`Hybrid` chests roll off the
    /// chest's floor's [`Self::effective_distance`] — so **deeper = richer** and the
    /// whole thing rides the *dungeon's stamped distance*, not the meaningless
    /// dungeon-local position. `richness` (≈`dungeon_chest_richness`) and
    /// `loot_mult` (≈`dungeon_loot_rarity_bonus`) are driver-supplied tunables.
    /// `None` if `chest_id` is not a chest.
    pub fn resolve_chest(
        &self,
        chest_id: &str,
        balance: &Balance,
        richness: i32,
        loot_mult: f64,
        seed: u64,
    ) -> Option<ChestReward> {
        let ObjectKind::Chest { loot, .. } = self.def.objects.get(chest_id)? else {
            return None;
        };
        let floor = self.def.placements.iter().find(|p| p.id == chest_id)?.floor;
        let effective_distance = self.effective_distance(floor);
        let roll = || roll_creature_loot(balance, effective_distance, richness, loot_mult, seed);
        let (rolled, authored) = match loot {
            ChestLoot::Rolled => (Some(roll()), Vec::new()),
            ChestLoot::Authored(items) => (None, items.clone()),
            ChestLoot::Hybrid(items) => (Some(roll()), items.clone()),
        };
        Some(ChestReward { effective_distance, rolled, authored })
    }
}

/// Map each stair endpoint cell to its partner on the neighbouring floor (both
/// ways). Relies on validation having paired every stair (one `Down` on floor n,
/// one `Up` on floor n+1).
fn build_stair_links(def: &DungeonDef) -> HashMap<(usize, usize, usize), (usize, usize, usize)> {
    let mut by_id: HashMap<&str, Vec<&meld_dungeon_content::Placement>> = HashMap::new();
    for p in &def.placements {
        if p.dir.is_some() && matches!(def.objects.get(&p.id), Some(ObjectKind::Stair)) {
            by_id.entry(p.id.as_str()).or_default().push(p);
        }
    }
    let mut links = HashMap::new();
    for ps in by_id.values() {
        if let [a, b] = ps[..] {
            // Order-independent: link whichever is Down to whichever is Up.
            let (down, up) = if a.dir == Some(StairDir::Down) { (a, b) } else { (b, a) };
            links.insert((down.floor, down.x, down.y), (up.floor, up.x, up.y));
            links.insert((up.floor, up.x, up.y), (down.floor, down.x, down.y));
        }
    }
    links
}

// ---------------------------------------------------------------------------
// Entrance placement — seeded, from the biome pool
// ---------------------------------------------------------------------------

/// The outcome of rolling a streamed section for a dungeon entrance.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EntranceRoll {
    /// The chosen dungeon's `name` — feed to `meld_dungeon_content::by_name`.
    pub dungeon: &'static str,
    /// A deterministic `[0,1)` lateral fraction; the driver maps it to a concrete
    /// entrance position within the section (kept off the clear path).
    pub lateral_frac: f64,
}

/// Roll whether a streamed section hosts a dungeon entrance, and which authored
/// dungeon from its `biome` pool. Deterministic in `section_seed`. `None` if the
/// biome has no authored dungeons or the `spawn_chance` roll fails.
///
/// `spawn_chance` is a probability in `[0,1]`: `0.0` never spawns, `1.0` always
/// spawns (when the biome pool is non-empty). The driver reads it from
/// `[worldgen] dungeon_spawn_chance` (DG-3b).
pub fn roll_entrance(section_seed: u64, biome: &str, spawn_chance: f64) -> Option<EntranceRoll> {
    if spawn_chance <= 0.0 {
        return None;
    }
    let pool: Vec<&'static DungeonDef> = meld_dungeon_content::for_biome(biome).collect();
    if pool.is_empty() {
        return None;
    }
    // Salt so this draw is independent of unrelated per-section rolls.
    let mut s = section_seed ^ 0x6D75_6E67_656F_6E00; // "…ngeon\0"
    if unit(splitmix64(&mut s)) >= spawn_chance.min(1.0) {
        return None;
    }
    let pick = (splitmix64(&mut s) % pool.len() as u64) as usize;
    let lateral_frac = unit(splitmix64(&mut s));
    Some(EntranceRoll { dungeon: pool[pick].name.as_str(), lateral_frac })
}

/// A placed dungeon entrance: which dungeon, and a concrete overworld position.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EntrancePlacement {
    pub dungeon: &'static str,
    pub position: Position,
}

/// Roll **and place** a dungeon entrance for a streamed section, given the
/// section's clear-path segment `p0 → p1` (the driver passes two consecutive path
/// waypoints). The entrance sits on the segment near mid-section — a guaranteed
/// walkable, reachable spot — at a deterministic fraction in `[0.35, 0.65]` from
/// `lateral_frac`, so entrances don't all land dead-centre. `None` when
/// [`roll_entrance`] declines. Pure + deterministic in `section_seed`.
pub fn place_entrance(
    section_seed: u64,
    biome: &str,
    spawn_chance: f64,
    p0: Position,
    p1: Position,
) -> Option<EntrancePlacement> {
    let roll = roll_entrance(section_seed, biome, spawn_chance)?;
    let t = 0.35 + roll.lateral_frac * 0.30;
    let position = Position { x: p0.x + (p1.x - p0.x) * t, y: p0.y + (p1.y - p0.y) * t };
    Some(EntrancePlacement { dungeon: roll.dungeon, position })
}

/// A `[0,1)` double from a 64-bit draw (top 53 bits → mantissa).
fn unit(x: u64) -> f64 {
    (x >> 11) as f64 / (1u64 << 53) as f64
}

/// splitmix64 — the same seeded-PRNG family the world generator uses, inlined to
/// keep this crate a pure leaf.
fn splitmix64(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn forest() -> &'static DungeonDef {
        meld_dungeon_content::by_name("verdant_barrow").expect("DG-2 forest dungeon")
    }

    fn desert() -> &'static DungeonDef {
        meld_dungeon_content::by_name("sunken_vault").expect("DG-2 desert dungeon")
    }

    fn balance() -> Balance {
        Balance::from_toml_str(Balance::EMBEDDED_DEFAULT).unwrap()
    }

    /// Find the (floor, centre) of the single placement of object `id`.
    fn cell(def: &DungeonDef, id: &str) -> (usize, Position) {
        let p = def.placements.iter().find(|p| p.id == id).unwrap();
        (p.floor, cell_center(p.x, p.y))
    }

    // --- entrance placement ---

    #[test]
    fn spawn_chance_bounds() {
        assert!(roll_entrance(1, "forest", 0.0).is_none(), "chance 0 never spawns");
        assert!(roll_entrance(1, "forest", 1.0).is_some(), "chance 1 always spawns (pool non-empty)");
    }

    #[test]
    fn unknown_biome_has_no_dungeons() {
        assert!(roll_entrance(1, "no_such_biome", 1.0).is_none());
    }

    #[test]
    fn roll_is_deterministic_and_picks_from_the_biome() {
        let a = roll_entrance(12345, "forest", 1.0);
        let b = roll_entrance(12345, "forest", 1.0);
        assert_eq!(a, b, "same seed → same roll");
        let r = a.unwrap();
        assert!(
            meld_dungeon_content::for_biome("forest").any(|d| d.name == r.dungeon),
            "picked dungeon belongs to the forest pool"
        );
        assert!((0.0..1.0).contains(&r.lateral_frac));
    }

    #[test]
    fn roll_varies_across_seeds() {
        // Over many seeds at a middling chance, we see both spawns and misses.
        let spawns = (0..200u64).filter(|s| roll_entrance(*s, "forest", 0.5).is_some()).count();
        assert!(spawns > 20 && spawns < 180, "≈50% of sections spawn one, got {spawns}/200");
    }

    #[test]
    fn place_entrance_sits_on_the_section_path_segment() {
        let p0 = Position { x: 10.0, y: 4.0 };
        let p1 = Position { x: 20.0, y: 4.0 };
        let pl = place_entrance(1, "forest", 1.0, p0, p1).expect("forest spawns at chance 1");
        assert!(meld_dungeon_content::by_name(pl.dungeon).is_some(), "names a real dungeon");
        // On the segment, between the 0.35 and 0.65 marks.
        assert!((13.5..=16.5).contains(&pl.position.x), "x in mid-section, got {}", pl.position.x);
        assert_eq!(pl.position.y, 4.0, "on the straight segment");
    }

    #[test]
    fn place_entrance_declines_when_the_roll_declines() {
        let p = Position { x: 0.0, y: 0.0 };
        assert!(place_entrance(1, "forest", 0.0, p, p).is_none(), "chance 0 → no entrance");
        assert!(place_entrance(1, "no_such_biome", 1.0, p, p).is_none(), "no pool → no entrance");
    }

    // --- instance: occupancy + committed space ---

    #[test]
    fn enter_places_at_the_entrance_on_floor_0() {
        let def = forest();
        let mut d = DungeonInstance::new(7, def, 250, 20);
        let spawn = d.enter("p1");
        let o = d.occupant("p1").unwrap();
        assert_eq!(o.floor, 0);
        assert_eq!(o.pos, spawn);
        let (ef, ep) = {
            let e = &def.entrances[0];
            (e.floor, cell_center(e.x, e.y))
        };
        assert_eq!((o.floor, o.pos), (ef, ep));
    }

    #[test]
    fn occupancy_lifecycle() {
        let def = forest();
        let mut d = DungeonInstance::new(7, def, 100, 10);
        assert!(d.is_empty());
        d.enter("a");
        d.enter("b");
        assert!(!d.is_empty());
        d.remove("a");
        assert!(!d.is_empty());
        d.remove("b");
        assert!(d.is_empty(), "empties when the last member leaves");
    }

    #[test]
    fn no_town_portal_in_a_dungeon() {
        let d = DungeonInstance::new(1, forest(), 0, 0);
        assert!(!d.town_portal_allowed());
    }

    #[test]
    fn try_move_slides_on_walls_and_respects_doors() {
        let def = forest(); // floor 0: `#>.t.a.X.k..........s#`, door D1 = X at cell 7, lever L1 at 5
        let mut d = DungeonInstance::new(1, def, 0, 0);
        let start = d.enter("p"); // entrance cell (1,1) → centre (1.5,1.5)
        // Into the wall row (downward) is blocked — position unchanged.
        assert_eq!(d.try_move("p", 0.0, 1.0, 0.5).unwrap(), start, "a wall blocks the move");
        // Along the open corridor (rightward) moves freely on the same row.
        let r = d.try_move("p", 1.0, 0.0, 0.5).unwrap();
        assert!(r.x > start.x && (r.y - start.y).abs() < 1e-9, "corridor is walkable");
        // A closed door blocks; opening its lever lets you through.
        d.set_pos("p", 0, Position { x: 6.5, y: 1.5 }); // just before door D1 (cell 7)
        assert_eq!(d.try_move("p", 1.0, 0.0, 1.0).unwrap(), Position { x: 6.5, y: 1.5 }, "closed door blocks");
        d.activate("L1");
        assert!(d.try_move("p", 1.0, 0.0, 1.0).unwrap().x > 6.5, "the opened door passes");
    }

    // --- instance: barriers open only when their puzzle is solved ---

    #[test]
    fn a_lever_opens_its_door() {
        let def = forest(); // door D1 opens when lever L1 is active
        let mut d = DungeonInstance::new(1, def, 0, 0);
        let (df, dpos) = cell(def, "D1");
        assert!(!d.walkable(df, dpos), "closed door blocks movement");
        let opened = d.activate("L1");
        assert!(opened.contains(&"D1".to_string()));
        assert!(d.walkable(df, dpos), "door is walkable once the lever is flipped");
    }

    #[test]
    fn a_co_op_gate_needs_all_three_plates() {
        let def = forest(); // gate G1 = all[P1,P2,P3]
        let mut d = DungeonInstance::new(1, def, 0, 0);
        assert!(d.activate("P1").is_empty(), "one plate is not enough");
        assert!(d.activate("P2").is_empty(), "two plates is not enough");
        let opened = d.activate("P3");
        assert!(opened.contains(&"G1".to_string()), "all three opens the co-op gate");
    }

    #[test]
    fn a_keyed_door_opens_once_the_key_is_held() {
        let def = forest(); // door D2 = has_key(K1)
        let mut d = DungeonInstance::new(1, def, 0, 0);
        let (df, dpos) = cell(def, "D2");
        assert!(!d.walkable(df, dpos));
        d.activate("K1");
        assert!(d.walkable(df, dpos));
    }

    #[test]
    fn activating_a_non_emitter_does_nothing() {
        let mut d = DungeonInstance::new(1, forest(), 0, 0);
        assert!(d.activate("D1").is_empty(), "you can't 'activate' a door");
        assert!(!d.is_open("D1"));
    }

    // --- instance: stairs + exit ---

    #[test]
    fn stairs_link_the_two_floors_both_ways() {
        let def = forest(); // stair S1: down on floor 0, up on floor 1
        let d = DungeonInstance::new(1, def, 0, 0);
        let down = def.placements.iter().find(|p| p.id == "S1" && p.dir == Some(StairDir::Down)).unwrap();
        let up = def.placements.iter().find(|p| p.id == "S1" && p.dir == Some(StairDir::Up)).unwrap();
        let dest = d.stair_dest(down.floor, cell_center(down.x, down.y)).unwrap();
        assert_eq!(dest, (up.floor, cell_center(up.x, up.y)), "down → up endpoint");
        let back = d.stair_dest(up.floor, cell_center(up.x, up.y)).unwrap();
        assert_eq!(back, (down.floor, cell_center(down.x, down.y)), "and back");
        assert!(d.stair_dest(0, cell_center(0, 0)).is_none(), "a wall corner is not a stair");
    }

    #[test]
    fn exit_is_detected_only_on_the_exit_cell() {
        let def = forest();
        let d = DungeonInstance::new(1, def, 0, 0);
        let e = &def.exits[0];
        assert!(d.at_exit(e.floor, cell_center(e.x, e.y)));
        assert!(!d.at_exit(0, cell_center(0, 0)));
    }

    // --- difficulty stamp ---

    #[test]
    fn effective_distance_grows_with_depth() {
        let d = DungeonInstance::new(1, forest(), 250, 20);
        assert_eq!(d.effective_distance(0), 250, "floor 0 = the stamped entry distance");
        assert_eq!(d.effective_distance(1), 270, "floor 1 adds one depth step");
        assert_eq!(d.effective_distance(3), 310);
    }

    // --- DG-5: chest loot resolution ---

    #[test]
    fn a_rolled_chest_rolls_at_its_floors_effective_distance() {
        // verdant_barrow's `vault` is a Rolled chest on floor 1.
        let def = forest();
        let d = DungeonInstance::new(1, def, 400, 25);
        let floor = def.placements.iter().find(|p| p.id == "vault").unwrap().floor;
        let r = d.resolve_chest("vault", &balance(), 4, 1.0, 99).unwrap();
        assert_eq!(r.effective_distance, d.effective_distance(floor));
        assert_eq!(r.effective_distance, 400 + floor as i64 * 25, "rides the stamp, not local position");
        assert!(r.rolled.is_some(), "a Rolled chest produces a roll");
        assert!(r.authored.is_empty());
    }

    #[test]
    fn a_hybrid_chest_grants_the_authored_relic_and_a_roll() {
        // sunken_vault's `sun_relic` is Hybrid: guaranteed relic + a roll.
        let d = DungeonInstance::new(1, desert(), 600, 30);
        let r = d.resolve_chest("sun_relic", &balance(), 4, 1.0, 7).unwrap();
        assert!(r.rolled.is_some(), "hybrid rolls too");
        assert_eq!(r.authored.len(), 1);
        assert_eq!(r.authored[0].gear.as_deref(), Some("sunspine_relic"));
    }

    #[test]
    fn chest_loot_is_deterministic_in_the_seed() {
        let d = DungeonInstance::new(1, forest(), 400, 25);
        let a = d.resolve_chest("vault", &balance(), 4, 1.0, 1234);
        let b = d.resolve_chest("vault", &balance(), 4, 1.0, 1234);
        assert_eq!(a, b, "same seed → same loot");
    }

    #[test]
    fn deeper_dungeons_out_scale_shallower_ones_on_average() {
        // Aggregate chit yield rises with the stamped distance (depth axis, design §6).
        let bal = balance();
        let sum = |level: i64| -> i64 {
            (0..64u64)
                .filter_map(|s| {
                    DungeonInstance::new(1, forest(), level, 25)
                        .resolve_chest("vault", &bal, 4, 1.0, s)
                        .and_then(|r| r.rolled)
                        .map(|l| l.chits)
                })
                .sum()
        };
        assert!(sum(1500) > sum(100), "a deep barrow out-rewards a shallow one");
    }

    #[test]
    fn a_boss_gated_chest_opens_only_after_the_boss_dies() {
        let def = forest(); // vault chest is `when = boss_dead(B1)`
        let mut d = DungeonInstance::new(1, def, 0, 0);
        assert!(!d.chest_openable("vault"), "sealed until the boss falls");
        d.activate("B1"); // boss defeated → boss_dead(B1)
        assert!(d.chest_openable("vault"), "now lootable");
        assert!(d.open_chest("vault"), "opens once");
        assert!(!d.chest_openable("vault"), "and only once");
        assert!(!d.open_chest("vault"));
    }

    #[test]
    fn resolving_a_non_chest_is_none() {
        let d = DungeonInstance::new(1, forest(), 100, 10);
        assert!(d.resolve_chest("L1", &balance(), 4, 1.0, 0).is_none(), "a lever is not a chest");
        assert!(d.resolve_chest("nope", &balance(), 4, 1.0, 0).is_none());
    }

    // --- DG-4: traps + disarm ---

    #[test]
    fn traps_start_armed() {
        assert_eq!(DungeonInstance::new(1, forest(), 0, 0).trap_state("T1"), Some(TrapState::Armed));
        assert_eq!(DungeonInstance::new(1, desert(), 0, 0).trap_state("T2"), Some(TrapState::Armed));
        assert_eq!(DungeonInstance::new(1, forest(), 0, 0).trap_state("L1"), None, "a lever is not a trap");
    }

    #[test]
    fn an_armed_trap_springs_on_its_cell_and_stays_armed() {
        let def = forest();
        let d = DungeonInstance::new(1, def, 300, 20);
        let (tf, tp) = cell(def, "T1"); // thorns, floor 0
        let hit = d.spring_trap(tf, tp).expect("armed trap fires");
        assert_eq!(hit.kind, "thorns");
        assert_eq!(hit.severity, d.effective_distance(tf), "severity rides the stamped distance");
        assert!(d.spring_trap(tf, tp).is_some(), "still armed after firing (persistent hazard)");
        // Not on an empty cell.
        let (_, entrance) = { let e = &def.entrances[0]; (e.floor, cell_center(e.x, e.y)) };
        assert!(d.spring_trap(0, entrance).is_none());
    }

    /// The smallest `seed` for which a disarm at the given odds resolves to `want`.
    fn seed_for(def: &'static DungeonDef, dex: i32, is_shifter: bool, div: f64, bonus: f64, want_disarm: bool) -> u64 {
        let (tf, tp) = cell(def, "T1");
        (0u64..10_000)
            .find(|s| {
                let mut d = DungeonInstance::new(1, def, 0, 0);
                let got = d.attempt_disarm(tf, tp, dex, is_shifter, div, bonus, *s);
                matches!(got, DisarmOutcome::Disarmed) == want_disarm
            })
            .unwrap()
    }

    #[test]
    fn a_successful_disarm_neutralises_the_trap() {
        let def = forest();
        let (tf, tp) = cell(def, "T1");
        let seed = seed_for(def, 100_000, false, 100.0, 0.0, true);
        let mut d = DungeonInstance::new(1, def, 0, 0);
        assert_eq!(d.attempt_disarm(tf, tp, 100_000, false, 100.0, 0.0, seed), DisarmOutcome::Disarmed);
        assert_eq!(d.trap_state("T1"), Some(TrapState::Disarmed));
        assert!(d.spring_trap(tf, tp).is_none(), "a disarmed trap no longer fires");
        assert_eq!(
            d.attempt_disarm(tf, tp, 100_000, false, 100.0, 0.0, seed),
            DisarmOutcome::AlreadyDisarmed
        );
    }

    #[test]
    fn a_failed_disarm_springs_the_trap_and_leaves_it_armed() {
        let def = forest();
        let (tf, tp) = cell(def, "T1");
        let seed = seed_for(def, 0, false, 100.0, 0.0, false); // p clamps to 0.05
        let mut d = DungeonInstance::new(1, def, 500, 0);
        match d.attempt_disarm(tf, tp, 0, false, 100.0, 0.0, seed) {
            DisarmOutcome::Sprung(hit) => assert_eq!(hit.kind, "thorns"),
            other => panic!("expected Sprung, got {other:?}"),
        }
        assert_eq!(d.trap_state("T1"), Some(TrapState::Armed), "a fumbled disarm leaves it armed");
    }

    #[test]
    fn the_shifter_is_far_better_at_disarming() {
        let def = forest();
        let (tf, tp) = cell(def, "T1");
        // A middling Dex: base p ≈ 0.10, Shifter p ≈ 0.60.
        let count = |shifter: bool| {
            (0..300u64)
                .filter(|s| {
                    let mut d = DungeonInstance::new(1, def, 0, 0);
                    matches!(
                        d.attempt_disarm(tf, tp, 10, shifter, 100.0, 0.5, *s),
                        DisarmOutcome::Disarmed
                    )
                })
                .count()
        };
        let (plain, shifter) = (count(false), count(true));
        assert!(shifter > plain * 3, "shifter {shifter} ≫ plain {plain}");
    }

    #[test]
    fn a_non_disarmable_trap_cannot_be_disarmed() {
        let def = desert(); // T2 is the dart, disarmable = false
        let (tf, tp) = cell(def, "T2");
        let mut d = DungeonInstance::new(1, def, 0, 0);
        assert_eq!(
            d.attempt_disarm(tf, tp, 100_000, true, 100.0, 0.5, 1),
            DisarmOutcome::NotDisarmable
        );
        assert_eq!(d.trap_state("T2"), Some(TrapState::Armed), "still a live hazard");
    }

    #[test]
    fn disarming_a_non_trap_cell_is_not_a_trap() {
        let def = forest();
        let (lf, lp) = cell(def, "L1");
        let mut d = DungeonInstance::new(1, def, 0, 0);
        assert_eq!(d.attempt_disarm(lf, lp, 50, false, 100.0, 0.0, 0), DisarmOutcome::NotATrap);
    }
}
