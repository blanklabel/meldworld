//! Parse an authored `*.dungeon.toml` string into a [`DungeonDef`].
//!
//! Two layers: the `when = "…"` **condition** mini-grammar (a recursive-descent
//! parser), and the **file** itself (serde over TOML for the tables + a
//! hand-written pass over each floor's glyph grid). Fatal shape errors surface
//! here; semantic + solvability checks live in [`crate::validate`].

use std::collections::BTreeMap;

use serde::Deserialize;

use crate::error::DungeonError;
use crate::model::*;

// ---------------------------------------------------------------------------
// Condition grammar
// ---------------------------------------------------------------------------

/// Parse a `when` string like `all[L1, not L2]`, `seq[P1,P2,P3]`,
/// `count(2,[a,b,c])`, `has_key(K1)`, `boss_dead(B1)`, or a bare `L1`.
pub fn parse_condition(s: &str) -> Result<Condition, String> {
    let mut p = CondParser { b: s.as_bytes(), i: 0 };
    p.ws();
    let c = p.cond()?;
    p.ws();
    if p.i != p.b.len() {
        return Err(format!("unexpected trailing input at byte {}", p.i));
    }
    Ok(c)
}

struct CondParser<'a> {
    b: &'a [u8],
    i: usize,
}

impl CondParser<'_> {
    fn ws(&mut self) {
        while self.i < self.b.len() && self.b[self.i].is_ascii_whitespace() {
            self.i += 1;
        }
    }

    fn peek(&self) -> Option<u8> {
        self.b.get(self.i).copied()
    }

    fn expect(&mut self, c: u8) -> Result<(), String> {
        self.ws();
        if self.peek() == Some(c) {
            self.i += 1;
            Ok(())
        } else {
            Err(format!("expected {:?} at byte {}", c as char, self.i))
        }
    }

    /// An identifier: `[A-Za-z0-9_]+`.
    fn ident(&mut self) -> Result<String, String> {
        self.ws();
        let start = self.i;
        while let Some(c) = self.peek() {
            if c.is_ascii_alphanumeric() || c == b'_' {
                self.i += 1;
            } else {
                break;
            }
        }
        if self.i == start {
            return Err(format!("expected identifier at byte {}", self.i));
        }
        Ok(String::from_utf8_lossy(&self.b[start..self.i]).into_owned())
    }

    fn number(&mut self) -> Result<usize, String> {
        self.ws();
        let start = self.i;
        while matches!(self.peek(), Some(c) if c.is_ascii_digit()) {
            self.i += 1;
        }
        if self.i == start {
            return Err(format!("expected number at byte {}", self.i));
        }
        String::from_utf8_lossy(&self.b[start..self.i])
            .parse()
            .map_err(|_| "bad number".to_string())
    }

    fn cond(&mut self) -> Result<Condition, String> {
        let word = self.ident()?;
        match word.as_str() {
            "all" => {
                self.expect(b'[')?;
                let cs = self.cond_list()?;
                self.expect(b']')?;
                Ok(Condition::All(cs))
            }
            "any" => {
                self.expect(b'[')?;
                let cs = self.cond_list()?;
                self.expect(b']')?;
                Ok(Condition::Any(cs))
            }
            "seq" => {
                self.expect(b'[')?;
                let ids = self.id_list()?;
                self.expect(b']')?;
                if ids.is_empty() {
                    return Err("seq[] must list at least one id".into());
                }
                Ok(Condition::Seq(ids))
            }
            "count" => {
                self.expect(b'(')?;
                let n = self.number()?;
                self.expect(b',')?;
                self.expect(b'[')?;
                let cs = self.cond_list()?;
                self.expect(b']')?;
                self.expect(b')')?;
                if n > cs.len() {
                    return Err(format!("count({n}, …) needs {n} options but only {} given", cs.len()));
                }
                Ok(Condition::Count(n, cs))
            }
            "not" => Ok(Condition::Not(Box::new(self.cond()?))),
            "has_key" => {
                self.expect(b'(')?;
                let id = self.ident()?;
                self.expect(b')')?;
                Ok(Condition::HasKey(id))
            }
            "boss_dead" => {
                self.expect(b'(')?;
                let id = self.ident()?;
                self.expect(b')')?;
                Ok(Condition::BossDead(id))
            }
            "room_clear" => {
                self.expect(b'(')?;
                let id = self.ident()?;
                self.expect(b')')?;
                Ok(Condition::RoomClear(id))
            }
            other => Ok(Condition::Ref(other.to_string())),
        }
    }

    fn cond_list(&mut self) -> Result<Vec<Condition>, String> {
        let mut out = vec![self.cond()?];
        loop {
            self.ws();
            if self.peek() == Some(b',') {
                self.i += 1;
                out.push(self.cond()?);
            } else {
                break;
            }
        }
        Ok(out)
    }

    fn id_list(&mut self) -> Result<Vec<String>, String> {
        let mut out = vec![self.ident()?];
        loop {
            self.ws();
            if self.peek() == Some(b',') {
                self.i += 1;
                out.push(self.ident()?);
            } else {
                break;
            }
        }
        Ok(out)
    }
}

// ---------------------------------------------------------------------------
// File format (serde) → DungeonDef
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct RawDungeon {
    name: String,
    biome: String,
    #[serde(default)]
    legend: BTreeMap<String, String>,
    #[serde(default, rename = "floor")]
    floors: Vec<RawFloor>,
    #[serde(default)]
    trap: BTreeMap<String, RawTrap>,
    #[serde(default)]
    door: BTreeMap<String, RawWhen>,
    #[serde(default)]
    gate: BTreeMap<String, RawWhen>,
    #[serde(default)]
    boss: BTreeMap<String, RawBoss>,
    #[serde(default)]
    chest: BTreeMap<String, RawChest>,
}

#[derive(Deserialize)]
struct RawFloor {
    grid: String,
}

#[derive(Deserialize)]
struct RawTrap {
    kind: String,
    #[serde(default)]
    disarmable: bool,
}

#[derive(Deserialize)]
struct RawWhen {
    when: String,
}

#[derive(Deserialize)]
struct RawBoss {
    sprite: String,
    #[serde(default)]
    on_enter_spawn: bool,
}

#[derive(Deserialize)]
struct RawChest {
    #[serde(default)]
    when: Option<String>,
    #[serde(default)]
    loot: Option<String>,
    #[serde(default)]
    contents: Vec<RawChestItem>,
}

#[derive(Deserialize)]
struct RawChestItem {
    #[serde(default)]
    item: Option<String>,
    #[serde(default)]
    gear: Option<String>,
    #[serde(default = "one")]
    quantity: u32,
}

fn one() -> u32 {
    1
}

/// One legend entry, resolved from its `"<type> <id> [extra]"` string.
struct LegendEntry {
    kind_tag: KindTag,
    id: Id,
    dir: Option<StairDir>,
    plate_momentary: bool,
}

enum KindTag {
    Lever,
    Plate,
    Key,
    Pedestal,
    Trap,
    Door,
    Gate,
    Boss,
    Chest,
    Stair,
}

/// Parse a `*.dungeon.toml` string into a [`DungeonDef`] (no semantic validation).
pub fn parse_str(src: &str) -> Result<DungeonDef, DungeonError> {
    let raw: RawDungeon = toml::from_str(src).map_err(|e| DungeonError::Toml(e.to_string()))?;

    // 1. Build the object table (type + params) from the typed `[trap.*]` etc.
    //    Legend cross-checks these; param-less objects (lever/key/…) are built
    //    straight from the legend.
    let mut objects: BTreeMap<Id, ObjectKind> = BTreeMap::new();
    for (id, t) in &raw.trap {
        objects.insert(id.clone(), ObjectKind::Trap { kind: t.kind.clone(), disarmable: t.disarmable });
    }
    for (id, d) in &raw.door {
        objects.insert(id.clone(), ObjectKind::Door { when: cond(id, &d.when)? });
    }
    for (id, g) in &raw.gate {
        objects.insert(id.clone(), ObjectKind::Gate { when: cond(id, &g.when)? });
    }
    for (id, b) in &raw.boss {
        objects.insert(id.clone(), ObjectKind::Boss { sprite: b.sprite.clone(), on_enter_spawn: b.on_enter_spawn });
    }
    for (id, c) in &raw.chest {
        let when = match &c.when {
            Some(w) => Some(cond(id, w)?),
            None => None,
        };
        let items: Vec<ChestItem> = c
            .contents
            .iter()
            .map(|i| ChestItem { item: i.item.clone(), gear: i.gear.clone(), quantity: i.quantity })
            .collect();
        let rolled = c.loot.as_deref() == Some("rolled");
        let loot = match (rolled, items.is_empty()) {
            (true, true) => ChestLoot::Rolled,
            (false, false) => ChestLoot::Authored(items),
            (true, false) => ChestLoot::Hybrid(items),
            (false, true) => {
                return Err(DungeonError::BadTable {
                    id: id.clone(),
                    reason: "chest needs loot = \"rolled\" and/or non-empty contents".into(),
                })
            }
        };
        objects.insert(id.clone(), ObjectKind::Chest { when, loot });
    }
    if let Some(bad) = c_loot_check(&raw) {
        return Err(bad);
    }

    // 2. Resolve the legend: char → (type, id).
    let mut legend: BTreeMap<char, LegendEntry> = BTreeMap::new();
    for (glyph, spec) in &raw.legend {
        let ch = single_char(glyph)?;
        legend.insert(ch, parse_legend(ch, spec)?);
    }

    // 3. Parse each floor grid into cells + placements, registering param-less
    //    objects and cross-checking param objects against their tables.
    let mut grids: Vec<Grid> = Vec::new();
    let mut placements: Vec<Placement> = Vec::new();
    let mut entrances: Vec<Placement> = Vec::new();
    let mut exits: Vec<Placement> = Vec::new();

    for (fidx, f) in raw.floors.iter().enumerate() {
        let lines = grid_lines(&f.grid, fidx)?;
        let width = lines.iter().map(|l| l.chars().count()).max().unwrap_or(0);
        let height = lines.len();
        let mut cells = Vec::with_capacity(width * height);
        for (y, line) in lines.iter().enumerate() {
            let chars: Vec<char> = line.chars().collect();
            for x in 0..width {
                let ch = chars.get(x).copied().unwrap_or(' ');
                let cell = match ch {
                    '#' => Cell { tile: Tile::Wall, object: None },
                    '.' => Cell { tile: Tile::Floor, object: None },
                    ' ' => Cell { tile: Tile::Void, object: None },
                    '>' => {
                        entrances.push(Placement { id: "@entrance".into(), floor: fidx, x, y, dir: None });
                        Cell { tile: Tile::Floor, object: None }
                    }
                    '<' => {
                        exits.push(Placement { id: "@exit".into(), floor: fidx, x, y, dir: None });
                        Cell { tile: Tile::Floor, object: None }
                    }
                    other => {
                        let e = legend.get(&other).ok_or(DungeonError::UnknownGlyph {
                            glyph: other,
                            floor: fidx,
                            x,
                            y,
                        })?;
                        register_object(&mut objects, e)?;
                        placements.push(Placement { id: e.id.clone(), floor: fidx, x, y, dir: e.dir });
                        Cell { tile: Tile::Floor, object: Some(e.id.clone()) }
                    }
                };
                cells.push(cell);
            }
        }
        grids.push(Grid { width, height, cells });
    }

    Ok(DungeonDef { name: raw.name, biome: raw.biome, grids, objects, placements, entrances, exits })
}

fn cond(id: &str, s: &str) -> Result<Condition, DungeonError> {
    parse_condition(s).map_err(|e| DungeonError::BadCondition { id: id.to_string(), reason: e })
}

/// A chest table with neither `loot` nor `contents` is already rejected inline;
/// this catches a stray `contents` item that sets neither `item` nor `gear`.
fn c_loot_check(raw: &RawDungeon) -> Option<DungeonError> {
    for (id, c) in &raw.chest {
        for it in &c.contents {
            if it.item.is_none() && it.gear.is_none() {
                return Some(DungeonError::BadTable {
                    id: id.clone(),
                    reason: "each chest content item must set `item` or `gear`".into(),
                });
            }
        }
    }
    None
}

fn single_char(s: &str) -> Result<char, DungeonError> {
    let mut it = s.chars();
    match (it.next(), it.next()) {
        (Some(c), None) => Ok(c),
        _ => Err(DungeonError::BadLegend {
            glyph: s.chars().next().unwrap_or('?'),
            reason: format!("legend key {s:?} must be exactly one character"),
        }),
    }
}

fn parse_legend(ch: char, spec: &str) -> Result<LegendEntry, DungeonError> {
    let toks: Vec<&str> = spec.split_whitespace().collect();
    let bad = |reason: &str| DungeonError::BadLegend { glyph: ch, reason: reason.to_string() };
    let ty = toks.first().ok_or_else(|| bad("empty legend entry"))?;
    let need_id = || toks.get(1).map(|s| s.to_string()).ok_or_else(|| bad("missing object id"));
    let (kind_tag, id, dir, plate_momentary) = match *ty {
        "lever" => (KindTag::Lever, need_id()?, None, true),
        "key" => (KindTag::Key, need_id()?, None, true),
        "pedestal" => (KindTag::Pedestal, need_id()?, None, true),
        "trap" => (KindTag::Trap, need_id()?, None, true),
        "door" => (KindTag::Door, need_id()?, None, true),
        "gate" => (KindTag::Gate, need_id()?, None, true),
        "boss" => (KindTag::Boss, need_id()?, None, true),
        "chest" => (KindTag::Chest, need_id()?, None, true),
        "plate" => {
            let momentary = match toks.get(2).copied() {
                None | Some("momentary") => true,
                Some("latching") => false,
                Some(o) => return Err(bad(&format!("plate flag must be momentary|latching, got {o:?}"))),
            };
            (KindTag::Plate, need_id()?, None, momentary)
        }
        "stair" => {
            let dir = match toks.get(2).copied() {
                Some("down") => StairDir::Down,
                Some("up") => StairDir::Up,
                _ => return Err(bad("stair needs a direction: down|up")),
            };
            (KindTag::Stair, need_id()?, Some(dir), true)
        }
        o => return Err(bad(&format!("unknown object type {o:?}"))),
    };
    Ok(LegendEntry { kind_tag, id, dir, plate_momentary })
}

/// Register (or cross-check) an object from a legend entry. Param-less kinds are
/// built here; param kinds (trap/door/gate/boss/chest) must already exist from a
/// table — a missing table is an error.
fn register_object(objects: &mut BTreeMap<Id, ObjectKind>, e: &LegendEntry) -> Result<(), DungeonError> {
    let needs_table = |what: &str| DungeonError::BadTable {
        id: e.id.clone(),
        reason: format!("legend declares {what} {:?} but there is no [{what}.{}] table", e.id, e.id),
    };
    match e.kind_tag {
        KindTag::Lever => insert_once(objects, &e.id, ObjectKind::Lever),
        KindTag::Plate => insert_once(objects, &e.id, ObjectKind::Plate { momentary: e.plate_momentary }),
        KindTag::Key => insert_once(objects, &e.id, ObjectKind::Key),
        KindTag::Pedestal => insert_once(objects, &e.id, ObjectKind::Pedestal),
        KindTag::Stair => insert_once(objects, &e.id, ObjectKind::Stair),
        KindTag::Trap => require(objects, &e.id, |k| matches!(k, ObjectKind::Trap { .. }), needs_table("trap")),
        KindTag::Door => require(objects, &e.id, |k| matches!(k, ObjectKind::Door { .. }), needs_table("door")),
        KindTag::Gate => require(objects, &e.id, |k| matches!(k, ObjectKind::Gate { .. }), needs_table("gate")),
        KindTag::Boss => require(objects, &e.id, |k| matches!(k, ObjectKind::Boss { .. }), needs_table("boss")),
        KindTag::Chest => require(objects, &e.id, |k| matches!(k, ObjectKind::Chest { .. }), needs_table("chest")),
    }
}

/// Insert a param-less object, tolerating the stair's second endpoint (same id,
/// already inserted by the first) but rejecting a genuine id collision.
fn insert_once(objects: &mut BTreeMap<Id, ObjectKind>, id: &str, kind: ObjectKind) -> Result<(), DungeonError> {
    match objects.get(id) {
        None => {
            objects.insert(id.to_string(), kind);
            Ok(())
        }
        Some(existing) if *existing == kind => Ok(()), // stair up/down share one id+kind
        Some(_) => Err(DungeonError::BadTable {
            id: id.to_string(),
            reason: format!("id {id:?} is used by two different objects"),
        }),
    }
}

fn require(
    objects: &BTreeMap<Id, ObjectKind>,
    id: &str,
    ok: impl Fn(&ObjectKind) -> bool,
    err: DungeonError,
) -> Result<(), DungeonError> {
    match objects.get(id) {
        Some(k) if ok(k) => Ok(()),
        _ => Err(err),
    }
}

/// Trim leading/trailing blank lines from a `"""…"""` grid block.
fn grid_lines(grid: &str, floor: usize) -> Result<Vec<&str>, DungeonError> {
    let raw: Vec<&str> = grid.lines().collect();
    let start = raw.iter().position(|l| !l.trim().is_empty());
    let (Some(start), Some(end)) = (start, raw.iter().rposition(|l| !l.trim().is_empty())) else {
        return Err(DungeonError::EmptyFloor { floor });
    };
    Ok(raw[start..=end].to_vec())
}
