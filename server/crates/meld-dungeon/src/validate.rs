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
    sequences(d, &mut errs);
    negations(d, &mut errs);
    timers(d, &mut errs);
    stairs(d, &mut errs);
    teleporters(d, &mut errs);
    // Only attempt the solvability search once the graph is well-formed; a
    // dangling reference or unpaired stair would make it meaningless.
    if errs.is_empty() {
        if let Err(e) = solvable(d) {
            errs.push(e);
        } else if let Err(e) = no_closing_can_strand(d) {
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
        // A TIMER is not a thing in a room — it is a clock. It has no glyph and no cell,
        // so it is the one declared object that is not required to be placed.
        if matches!(kind, ObjectKind::Timer { .. }) {
            continue;
        }
        let count = d.placements_of(id).count();
        let is_stair =
            matches!(kind, ObjectKind::Stair | ObjectKind::Teleporter { .. });
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
    // A pedestal's `wants` is a reference too, and the only one that is not in a
    // condition — an unresolvable one would otherwise surface as "the barrier behind this
    // pedestal can never open", which is true and unhelpful.
    for (id, kind) in &d.objects {
        if let ObjectKind::Pedestal { wants } = kind {
            match d.objects.get(wants) {
                Some(ObjectKind::Key) => {}
                Some(_) => errs.push(DungeonError::TypeMismatch {
                    id: id.clone(),
                    referenced: wants.clone(),
                    reason: "a pedestal's `wants` must name a key".into(),
                }),
                None => errs.push(DungeonError::UnknownRef {
                    id: id.clone(),
                    referenced: wants.clone(),
                }),
            }
        }
    }
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
                // A PEDESTAL is an emitter like any other once it has been fed, so it can
                // be a bare atom. Excluding it made the whole object inert: nothing could
                // name it, so setting the idol down opened nothing.
                RefKind::Activatable => matches!(
                    target_kind,
                    ObjectKind::Lever
                        | ObjectKind::Plate { .. }
                        | ObjectKind::Pedestal { .. }
                        // A timer is named as a bare atom too — `timers()` above is what
                        // restricts WHO may name one.
                        | ObjectKind::Timer { .. }
                ),
                RefKind::Key => matches!(target_kind, ObjectKind::Key),
                RefKind::Boss => matches!(target_kind, ObjectKind::Boss { .. }),
                RefKind::Clearable => {
                    matches!(target_kind, ObjectKind::Boss { .. } | ObjectKind::Spawn { .. })
                }
                RefKind::Any => true,
            };
            if !ok {
                errs.push(DungeonError::TypeMismatch {
                    id: id.clone(),
                    referenced: target,
                    reason: match want {
                        RefKind::Activatable => {
                            "a bare atom / seq element must name a lever, plate, pedestal or timer".into()
                        }
                        RefKind::Key => "has_key(…) must name a key".into(),
                        RefKind::Boss => "boss_dead(…) must name a boss".into(),
                        RefKind::Clearable => {
                            "room_clear(…) must name a spawn or a boss — something a \
                             party can actually fight"
                                .into()
                        }
                        RefKind::Any => unreachable!(),
                    },
                });
            }
        }
    }
}

/// A timer's `started_by` must be something that can actually fire, only a MOVER may name
/// a timer, and no `seq[…]` may contain one.
///
/// A timer is the one emitter that goes INACTIVE again, so a latching door told to open
/// "while the clock runs" opens once and never shuts — the same silent lie `not` tells. And
/// the activation log is a record of PRESSES; a timer is not something anybody pressed.
fn timers(d: &DungeonDef, errs: &mut Vec<DungeonError>) {
    fn seq_ids(c: &Condition, out: &mut Vec<Id>) {
        match c {
            Condition::Seq(ids) => out.extend(ids.iter().cloned()),
            Condition::Not(x) => seq_ids(x, out),
            Condition::All(cs) | Condition::Any(cs) | Condition::Count(_, cs) => {
                cs.iter().for_each(|c| seq_ids(c, out))
            }
            _ => {}
        }
    }
    let is_timer = |id: &Id| matches!(d.objects.get(id), Some(ObjectKind::Timer { .. }));
    for (id, kind) in &d.objects {
        if let ObjectKind::Timer { started_by, .. } = kind {
            match d.objects.get(started_by) {
                Some(k) if k.activates_on_reach() => {}
                Some(_) => errs.push(DungeonError::TypeMismatch {
                    id: id.clone(),
                    referenced: started_by.clone(),
                    reason: "a timer's `started_by` must name something that fires — a lever,                              plate, pedestal, key, spawn or boss"
                        .into(),
                }),
                None => errs.push(DungeonError::UnknownRef {
                    id: id.clone(),
                    referenced: started_by.clone(),
                }),
            }
        }
        let Some(c) = kind.condition() else { continue };
        let mut named = Vec::new();
        c.referenced(&mut named);
        if !kind.can_close() {
            if let Some((t, _)) = named.iter().find(|(n, _)| is_timer(n)) {
                errs.push(DungeonError::BadCondition {
                    id: id.clone(),
                    reason: format!(
                        "only a [mover.<id>] may name the timer {t:?}; a door, gate or chest                          LATCHES open and would never shut when the clock ran out"
                    ),
                });
            }
        }
        let mut in_seq = Vec::new();
        seq_ids(c, &mut in_seq);
        if let Some(t) = in_seq.iter().find(|n| is_timer(n)) {
            errs.push(DungeonError::BadCondition {
                id: id.clone(),
                reason: format!("seq[…] records PRESSES; the timer {t:?} is not one"),
            });
        }
    }
}

/// `not` may only appear on a MOVER, because only a mover can act on it.
///
/// A `Door`/`Gate` LATCHES — `open` never shrinks, in the runtime or in this search — so
/// `door.when = "not L1"` reads as "shut once the lever is pulled" and does the opposite:
/// it opens at the start and stays open forever. Nothing said so, and no content had
/// tried it, which is the only reason the lie went unnoticed. Say it at build time.
fn negations(d: &DungeonDef, errs: &mut Vec<DungeonError>) {
    fn has_not(c: &Condition) -> bool {
        match c {
            Condition::Not(_) => true,
            Condition::All(cs) | Condition::Any(cs) | Condition::Count(_, cs) => {
                cs.iter().any(has_not)
            }
            _ => false,
        }
    }
    for (id, kind) in &d.objects {
        if kind.can_close() {
            continue;
        }
        if let Some(c) = kind.condition() {
            if has_not(c) {
                errs.push(DungeonError::BadCondition {
                    id: id.clone(),
                    reason: "`not` only means something on a [mover.<id>], which re-closes; a                              door, gate or chest LATCHES open and would never shut again"
                        .into(),
                });
            }
        }
    }
}

/// A `seq[…]` must never ask for the same press TWICE IN A ROW.
///
/// This is the one hole in "ordering can never make a dungeon unsolvable". The runtime
/// dedupes consecutive presses of one emitter — it has to, or standing on a plate would
/// flood the activation log — so `seq[P1,P1]` needs some OTHER emitter pressed in between
/// to separate them. Where none is reachable the lock can never open, and the solvability
/// search cannot see it: that search evaluates `seq` order-agnostically (correctly — see
/// `Condition::eval_ordered`), so it would call the dungeon solvable.
///
/// Refusing the shape is far better than teaching the search about it. A dungeon takes no
/// Town Portal, so a barrier that can never open is a party sealed in.
fn sequences(d: &DungeonDef, errs: &mut Vec<DungeonError>) {
    fn walk(id: &Id, c: &Condition, errs: &mut Vec<DungeonError>) {
        match c {
            Condition::Seq(ids) => {
                for w in ids.windows(2) {
                    if w[0] == w[1] {
                        errs.push(DungeonError::BadCondition {
                            id: id.clone(),
                            reason: format!(
                                "seq[...] names {:?} twice in a row; consecutive presses of one                                  emitter count once, so that step can never be satisfied",
                                w[0]
                            ),
                        });
                    }
                }
            }
            Condition::Not(inner) => walk(id, inner, errs),
            Condition::All(cs) | Condition::Any(cs) | Condition::Count(_, cs) => {
                cs.iter().for_each(|c| walk(id, c, errs))
            }
            _ => {}
        }
    }
    for (id, kind) in &d.objects {
        if let Some(c) = kind.condition() {
            walk(id, c, errs);
        }
    }
}

/// Each teleporter: exactly one `from` pad and one `to` pad. Unlike a stair there is NO
/// floor constraint — a pad may land you on the same floor, which is the whole point.
fn teleporters(d: &DungeonDef, errs: &mut Vec<DungeonError>) {
    for (id, kind) in &d.objects {
        if !matches!(kind, ObjectKind::Teleporter { .. }) {
            continue;
        }
        let ps: Vec<&Placement> = d.placements_of(id).collect();
        let from = ps.iter().filter(|p| p.dir == Some(StairDir::Down)).count();
        let to = ps.iter().filter(|p| p.dir == Some(StairDir::Up)).count();
        if from != 1 || to != 1 {
            errs.push(DungeonError::BadStair {
                id: id.clone(),
                reason: format!(
                    "a teleporter needs exactly one 'from' pad and one 'to' pad (got {from} from, {to} to)"
                ),
            });
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
            // A PEDESTAL is the one emitter reaching is not enough for: you have to be
            // carrying what it wants. Reached-but-unfed simply does not activate yet —
            // the fixpoint runs again once the key is in the active set, so the order
            // "find the idol, then bring it here" falls out rather than being sequenced
            // by hand.
            if let Some(ObjectKind::Pedestal { wants }) = d.objects.get(&id) {
                if !active.contains(wants) {
                    continue;
                }
            }
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

/// A CLOSING BARRIER MUST NEVER BE ABLE TO SEAL A PARTY IN.
///
/// A [`ObjectKind::Mover`] can shut again, and a dungeon takes no Town Portal, so a wall
/// dropping behind you is the one thing in this model that could end a run with no way out
/// but dying. Proving "no *reachable sequence* of closings strands anyone" would mean
/// searching orderings; this proves something strictly stronger and costs one flood:
///
/// **from every cell a party can reach, an exit is still reachable with EVERY mover
/// shut.** If that holds, no combination of closings can matter — the worst case is
/// already survivable. It rejects some safe designs (a mover that shuts behind you onto a
/// route only another mover opens), and that is the right trade: the failure it prevents
/// is a run that cannot end.
///
/// Cheap because "can reach an exit" is one REVERSE flood from the exits, not one flood
/// per cell — passability here is symmetric, so a route out is a route in. The one
/// asymmetry is a ONE-WAY teleport pad, and the reverse flood walks those backwards
/// deliberately: a pad that carried you in is a pad you cannot come back through, so its
/// landing side must be able to reach an exit on its own.
fn no_closing_can_strand(d: &DungeonDef) -> Result<(), DungeonError> {
    if !d.objects.values().any(|k| k.can_close()) {
        return Ok(());
    }
    let links = stair_links(d);
    // Everywhere a party can get, with movers allowed to open (the generous set).
    let all_open: HashSet<Id> =
        d.objects.iter().filter(|(_, k)| k.is_barrier()).map(|(id, _)| id.clone()).collect();
    let start = (d.entrances[0].floor, d.entrances[0].x, d.entrances[0].y);
    let (reachable, _) = flood(d, start, &links, &all_open);

    // Everywhere that can still get OUT with every mover shut. Reverse links, so a
    // one-way pad is walked the way a stranded party would have to walk it.
    let shut: HashSet<Id> = all_open
        .iter()
        .filter(|id| !d.objects.get(*id).is_some_and(|k| k.can_close()))
        .cloned()
        .collect();
    let mut back: BTreeMap<Node, Node> = BTreeMap::new();
    for (from, to) in &links {
        back.insert(*to, *from);
    }
    let mut safe: HashSet<Node> = HashSet::new();
    for e in &d.exits {
        let (r, _) = flood(d, (e.floor, e.x, e.y), &back, &shut);
        safe.extend(r);
    }
    // A party STANDING on a mover when it shuts is not sealed in — they step off it. So a
    // mover's own cell counts as safe if any neighbour is, which is the one place this
    // otherwise-strict check would have cried wolf (and did, on its first fixture).
    let mover_cells: Vec<Node> = d
        .placements
        .iter()
        .filter(|p| d.objects.get(&p.id).is_some_and(|k| k.can_close()))
        .map(|p| (p.floor, p.x, p.y))
        .collect();
    for n @ (f, x, y) in mover_cells {
        let g = &d.grids[f];
        let neigh = [
            (x > 0).then(|| (f, x - 1, y)),
            (x + 1 < g.width).then(|| (f, x + 1, y)),
            (y > 0).then(|| (f, x, y - 1)),
            (y + 1 < g.height).then(|| (f, x, y + 1)),
        ];
        if neigh.into_iter().flatten().any(|m| safe.contains(&m)) {
            safe.insert(n);
        }
    }
    if let Some(stranded) = reachable.iter().find(|n| !safe.contains(n)) {
        let (f, x, y) = *stranded;
        return Err(DungeonError::Unsolvable {
            reason: format!(
                "a mover shutting could strand a party: floor {f} cell ({x},{y}) is reachable,                  but with every mover closed no exit can be reached from it"
            ),
        });
    }
    Ok(())
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
            // A BLOCK is a wall to the search. Proving what a push can reach is Sokoban;
            // guessing generously would certify a route nobody can walk. So the gate
            // proves the dungeon solvable with nothing moved, and a block can only add
            // options on top of that. See `ObjectKind::Block`.
            Some(ObjectKind::Block) => false,
            _ => true, // levers/plates/keys/traps/bosses/chests/stairs are walkable
        },
    }
}

/// Map each linked endpoint cell to where standing on it takes you.
///
/// DIRECTED, because a one-way teleporter is directed: the search must not walk back
/// through a pad a player cannot walk back through, or it would prove an exit reachable
/// by a route nobody can take — in a space with no Town Portal.
fn stair_links(d: &DungeonDef) -> BTreeMap<Node, Node> {
    let mut by_id: BTreeMap<&str, Vec<&Placement>> = BTreeMap::new();
    for p in &d.placements {
        if p.dir.is_some() {
            by_id.entry(p.id.as_str()).or_default().push(p);
        }
    }
    let mut links = BTreeMap::new();
    for (id, ps) in &by_id {
        if ps.len() != 2 {
            continue;
        }
        let one_way = matches!(d.objects.get(*id), Some(ObjectKind::Teleporter { one_way: true }));
        // `Down` is a stair's lower end and a teleporter's ENTRY pad.
        let (from, to) = if ps[0].dir == Some(StairDir::Down) { (ps[0], ps[1]) } else { (ps[1], ps[0]) };
        let a = (from.floor, from.x, from.y);
        let b = (to.floor, to.x, to.y);
        links.insert(a, b);
        if !one_way {
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
