//! Semantic + solvability checks over a parsed [`DungeonDef`].
//!
//! The headline check is **solvability**: a bounded fixpoint search proving that
//! *some* order of operations a party can perform opens every barrier on a route
//! from the entrance to an exit, across the whole floor stack. Because a dungeon
//! is a committed space (no Town Portal — design §4), an unsolvable dungeon would
//! be a trap with no way out, so this is a hard gate. See `docs/proposals/dungeons.md`.

use std::collections::{BTreeMap, HashSet, VecDeque};

use crate::error::DungeonError;
use crate::model::*;

/// Run every check; empty `Vec` means the dungeon is well-formed and solvable.
pub fn validate(d: &DungeonDef) -> Vec<DungeonError> {
    let mut errs = Vec::new();
    structural(d, &mut errs);
    placements(d, &mut errs);
    references(d, &mut errs);
    stairs(d, &mut errs);
    // Only attempt the solvability search once the graph is well-formed; a
    // dangling reference or unpaired stair would make it meaningless.
    if errs.is_empty() {
        if let Err(e) = solvable(d) {
            errs.push(e);
        }
    }
    errs
}

fn structural(d: &DungeonDef, errs: &mut Vec<DungeonError>) {
    match d.entrances.len() {
        1 => {
            if d.entrances[0].floor != 0 {
                errs.push(DungeonError::EntranceFloor { floor: d.entrances[0].floor });
            }
        }
        n => errs.push(DungeonError::EntranceCount { found: n }),
    }
    if d.exits.is_empty() {
        errs.push(DungeonError::ExitCount { found: 0 });
    }
}

/// Each object placed exactly once (a stair: exactly one Down + one Up), and
/// every declared object actually placed somewhere.
fn placements(d: &DungeonDef, errs: &mut Vec<DungeonError>) {
    for (id, kind) in &d.objects {
        let count = d.placements_of(id).count();
        let is_stair = matches!(kind, ObjectKind::Stair);
        if count == 0 {
            errs.push(DungeonError::Unplaced { id: id.clone() });
        } else if is_stair {
            // stair pairing (count + adjacency) is checked in `stairs`
        } else if count != 1 {
            errs.push(DungeonError::DuplicatePlacement { id: id.clone(), count });
        }
    }
}

/// Every id named by a condition exists and is a sensible type for the predicate.
fn references(d: &DungeonDef, errs: &mut Vec<DungeonError>) {
    for (id, kind) in &d.objects {
        let Some(cond) = kind.condition() else { continue };
        let mut refs = Vec::new();
        cond.referenced(&mut refs);
        for (target, want) in refs {
            let Some(target_kind) = d.objects.get(&target) else {
                errs.push(DungeonError::UnknownRef { id: id.clone(), referenced: target });
                continue;
            };
            let ok = match want {
                RefKind::Activatable => matches!(target_kind, ObjectKind::Lever | ObjectKind::Plate { .. }),
                RefKind::Key => matches!(target_kind, ObjectKind::Key),
                RefKind::Boss => matches!(target_kind, ObjectKind::Boss { .. }),
                RefKind::Any => true,
            };
            if !ok {
                errs.push(DungeonError::TypeMismatch {
                    id: id.clone(),
                    referenced: target,
                    reason: match want {
                        RefKind::Activatable => "a bare atom / seq element must name a lever or plate".into(),
                        RefKind::Key => "has_key(…) must name a key".into(),
                        RefKind::Boss => "boss_dead(…) must name a boss".into(),
                        RefKind::Any => unreachable!(),
                    },
                });
            }
        }
    }
}

/// Each stair id: exactly one `Down` on floor n and one `Up` on floor n+1.
fn stairs(d: &DungeonDef, errs: &mut Vec<DungeonError>) {
    for (id, kind) in &d.objects {
        if !matches!(kind, ObjectKind::Stair) {
            continue;
        }
        let ps: Vec<&Placement> = d.placements_of(id).collect();
        let downs: Vec<&&Placement> = ps.iter().filter(|p| p.dir == Some(StairDir::Down)).collect();
        let ups: Vec<&&Placement> = ps.iter().filter(|p| p.dir == Some(StairDir::Up)).collect();
        if downs.len() != 1 || ups.len() != 1 {
            errs.push(DungeonError::BadStair {
                id: id.clone(),
                reason: format!("needs exactly one 'down' and one 'up' endpoint (got {} down, {} up)", downs.len(), ups.len()),
            });
            continue;
        }
        let (down, up) = (downs[0], ups[0]);
        if up.floor != down.floor + 1 {
            errs.push(DungeonError::BadStair {
                id: id.clone(),
                reason: format!("'down' is on floor {} so 'up' must be on floor {}, but it's on floor {}", down.floor, down.floor + 1, up.floor),
            });
        }
    }
}

// ---------------------------------------------------------------------------
// Solvability — bounded fixpoint reachability across the floor stack
// ---------------------------------------------------------------------------

type Node = (usize, usize, usize); // (floor, x, y)

/// Prove an exit is reachable. Grows two monotone sets to a fixpoint: `active`
/// (emitters reached, hence operable) and `open` (barriers whose condition now
/// holds). Each round re-floods reachability under the currently-open barriers,
/// harvesting any newly-reached emitters; opening barriers can reveal more
/// emitters, which can open more barriers, until nothing changes.
fn solvable(d: &DungeonDef) -> Result<(), DungeonError> {
    let start = (d.entrances[0].floor, d.entrances[0].x, d.entrances[0].y);
    let links = stair_links(d);
    let exit_set: HashSet<Node> = d.exits.iter().map(|e| (e.floor, e.x, e.y)).collect();

    let mut active: HashSet<Id> = HashSet::new();
    let mut open: HashSet<Id> = HashSet::new();

    loop {
        let (reached, reached_emitters) = flood(d, start, &links, &open);
        if reached.iter().any(|n| exit_set.contains(n)) {
            return Ok(());
        }
        let mut changed = false;
        for id in reached_emitters {
            if active.insert(id) {
                changed = true;
            }
        }
        for (id, kind) in &d.objects {
            if kind.is_barrier() && !open.contains(id) {
                if let Some(c) = kind.condition() {
                    if c.eval(&active) {
                        open.insert(id.clone());
                        changed = true;
                    }
                }
            }
        }
        if !changed {
            return Err(DungeonError::Unsolvable { reason: blocked_reason(d, &open) });
        }
    }
}

/// Flood-fill walkable cells from `start` under the open-barrier set, returning
/// the reached cells and the ids of any emitters (lever/plate/key/pedestal/boss)
/// standing on a reached cell.
fn flood(d: &DungeonDef, start: Node, links: &BTreeMap<Node, Node>, open: &HashSet<Id>) -> (HashSet<Node>, HashSet<Id>) {
    let mut seen: HashSet<Node> = HashSet::new();
    let mut emitters: HashSet<Id> = HashSet::new();
    let mut q: VecDeque<Node> = VecDeque::new();
    if walkable(d, start, open) {
        seen.insert(start);
        q.push_back(start);
    }
    while let Some(n @ (f, x, y)) = q.pop_front() {
        if let Some(id) = &d.grids[f].at(x, y).object {
            if d.objects.get(id).is_some_and(|k| k.activates_on_reach()) {
                emitters.insert(id.clone());
            }
        }
        let mut neigh: Vec<Node> = Vec::new();
        let g = &d.grids[f];
        if x > 0 {
            neigh.push((f, x - 1, y));
        }
        if x + 1 < g.width {
            neigh.push((f, x + 1, y));
        }
        if y > 0 {
            neigh.push((f, x, y - 1));
        }
        if y + 1 < g.height {
            neigh.push((f, x, y + 1));
        }
        if let Some(&linked) = links.get(&n) {
            neigh.push(linked);
        }
        for m in neigh {
            if !seen.contains(&m) && walkable(d, m, open) {
                seen.insert(m);
                q.push_back(m);
            }
        }
    }
    (seen, emitters)
}

/// A cell is walkable if it is `Floor` and not blocked by a closed barrier.
fn walkable(d: &DungeonDef, (f, x, y): Node, open: &HashSet<Id>) -> bool {
    let cell = d.grids[f].at(x, y);
    if cell.tile != Tile::Floor {
        return false;
    }
    match &cell.object {
        None => true,
        Some(id) => match d.objects.get(id) {
            Some(k) if k.is_barrier() => open.contains(id),
            _ => true, // levers/plates/keys/traps/bosses/chests/stairs are walkable
        },
    }
}

/// Map each stair endpoint cell to its paired endpoint on the neighbouring floor.
fn stair_links(d: &DungeonDef) -> BTreeMap<Node, Node> {
    let mut by_id: BTreeMap<&str, Vec<&Placement>> = BTreeMap::new();
    for p in &d.placements {
        if p.dir.is_some() {
            by_id.entry(p.id.as_str()).or_default().push(p);
        }
    }
    let mut links = BTreeMap::new();
    for ps in by_id.values() {
        if ps.len() == 2 {
            let a = (ps[0].floor, ps[0].x, ps[0].y);
            let b = (ps[1].floor, ps[1].x, ps[1].y);
            links.insert(a, b);
            links.insert(b, a);
        }
    }
    links
}

/// A human-readable hint at which barriers stayed shut when the search stalled.
fn blocked_reason(d: &DungeonDef, open: &HashSet<Id>) -> String {
    let shut: Vec<&str> = d
        .objects
        .iter()
        .filter(|(id, k)| k.is_barrier() && !open.contains(*id))
        .map(|(id, _)| id.as_str())
        .collect();
    if shut.is_empty() {
        "the exit is walled off from the entrance".into()
    } else {
        format!("these barriers can never open: {}", shut.join(", "))
    }
}
