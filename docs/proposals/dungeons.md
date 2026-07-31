# Designed Dungeons (Epic WG-1, full version) — design + build plan

> Status: **DESIGN — build in progress.** This is the full, separately-instanced
> dungeon that the WG-1 *dungeon sections* slice deferred (see
> [`worldgen-wg.md`](worldgen-wg.md) §"WG-1 — dungeon sections (shipped)"). Written
> against the real code: `meld-world::Arena` (`open_chest`, `check_touch`,
> `ensure_frontier`, `Chest`/`Obstacle`/`MonsterSpawn`), `meld-server::game.rs`
> (the single-task loop, `ActiveInstance`, `open_chest` loot roll at ~line 2698),
> `meld-run::InstanceRun`, and `meld-proto` `SnapshotEntity`. The pure authoring +
> validation layer (`meld-dungeon`) is built first; runtime + client follow.
> **Not yet folded into CANON** — a `D`-number + `behaviors/dungeons.md` land with
> the runtime phase.

## The vision

Each biome has a **small pool of hand-designed dungeons** — *authored*, not
procedurally generated. As the overworld streams, a biome section has a **chance**
to also host a **dungeon entrance**. Walk through it and you drop into the
dungeon's own space: a stack of **floors** joined by **stairs**, laid out by a
designer (human or agent) with **traps, puzzles, a boss, and treasure**. It is a
**subinstance** — shared live among the party who entered — but **per-entry
fresh**, so every group gets its own copy.

This is the opposite pole from the rest of the world (infinite seeded procgen):
dungeons are the **authored, set-piece** content that seeded generation can't
deliver — real puzzles, real traps, a boss arena, a guaranteed reward.

## Broader frame: one authored-space substrate, three profiles

A dungeon is not the only hand-designed space the game wants. **Last City is one
too** — an authored, multi-room space you walk around in, just stocked with shops
and NPCs instead of traps and a boss. So dungeons are best built as **one profile
of a shared "authored-space" substrate**, not a bespoke system:

| | Overworld | **Dungeon** | City / Hub |
|---|---|---|---|
| Layout | procgen (seeded) | **authored** | **authored** |
| Sharing | instance-shared | per-entry-fresh, ≤4 | persistent, many |
| Danger | combat | traps/puzzles/boss, **committed** | none (safe) |
| Objects | mobs / nodes / chests | levers / gates / traps / boss | shops / NPCs / services |
| Gate | clear-path | **solvability** | none |

The **shared core** is the glyph-grid + manifest authoring format, the placed-object
+ `run.interact` model, and the runtime *space* abstraction (map-of-spaces + avatar
location + scoped tick/snapshot — DG-3). Everything hostile — solvability,
committed-space/death, distance-scaled loot, per-entry-fresh — is the **dungeon
profile only**. The **city profile** keeps just the grid + placement + interaction
core and adds the friendly layer (services, NPCs).

**Caution — the sharing models are opposite.** A dungeon is per-entry-fresh and
≤4; a city is *one persistent space shared by hundreds* (the LC-1 presence-at-scale
problem). The dungeon subinstance runtime is cheap *because* groups are tiny — it is
a **foundation** for the city, **not a solution** to its scale. Don't conflate "the
city is a space" with "the city is a dungeon."

**Implication for the code:** the `meld-dungeon` crate keeps the DG-1 foundation as
is, but when DG-3's space runtime lands, factor the shared authored-space core out
from under it (grid / legend / placements / conditions / the space trait), with
`dungeon` (traps / solvability / committed) and `city` (services / presence) as
layers on top — so we don't bake in a dungeon-only abstraction. Tracked as **LC-5**.

## Design decisions (locked)

Every one of these was decided deliberately; they drive the build.

### 1. Designed, not generated
Dungeon interiors are **authored data**, compiled into the binary — not procgen.
Seeded generation only decides *whether* a section hosts an entrance, *where* the
entrance sits, *which* dungeon from the biome's pool spawns, and the *loot rolls*.
The layout, traps, puzzles, boss, and treasure placements are fixed by the author.

### 2. Subinstance = a stack of floors (the runtime model)
Today the loop owns exactly one space (`GameState::instance: Option<ActiveInstance>`,
game.rs:563). Generalize to **a map of live spaces**: the overworld arena plus
`HashMap<DungeonKey, DungeonSpace>`, and give each avatar a **location**
(`Overworld` or `InDungeon(DungeonKey, floor)`). The tick loops over spaces;
movement / touch / interact / snapshot all scope to the player's current space.
**Exactly one task still touches all state, so there are still no locks** (CANON
§S survives intact).

> **Aligns with Epic SC (server scaling).** [SC-3](../ROADMAP.md) shards the server
> into a `Router` + **one `WorldActor` per world**, and rules that *"towns are
> content, not their own shard."* A dungeon subinstance is the same: it is **content
> inside a world-actor**, living on that world's single owning task — **not its own
> shard or task**. So "map of live spaces" means *spaces within one world-actor*,
> and the no-locks invariant is the world-actor's, unchanged. Two more SC ties:
> per-space snapshots must reuse **SC-1**'s chunk-grid interest index rather than
> re-scan (the same broadphase SC-1 earmarks for overworld traps/hazards, which
> dungeon traps also ride); and — unlike SC-3's *persistent* worlds (hibernated to
> Postgres as a seed-delta) — a dungeon is **ephemeral, per-entry-fresh, discarded
> on exit** (decision §3). Totally different lifecycle: never persist a dungeon.

- A `DungeonSpace` is an **ordered stack of floors**, each an authored grid.
- **Stairs** are an *inter-floor connector* — glyph pairs matched by id across two
  floor grids (`v` down on floor N ↔ `^` up on floor N+1), transition **on
  contact** (like the overworld portal). This is a **coarser axis than
  verticality**: verticality's `level: u8` is elevation *within one grid*; the
  floor index is *whole grids stacked*. Distinct field — do **not** overload
  `level`. A floor may still use verticality internally.

### 3. Per-entry-fresh, shared among a group of up to 4
Each entry spawns a **fresh `DungeonSpace` copy**; several can be live at once. A
group forms with the existing **`join_battle` pattern**: the player who touches the
entrance plus any teammate within `[ai] join_radius` who opts in enters together,
bound to that one space. A dungeon already in progress is **not joinable** (matches
"no joining a fight already underway"). Fresh-per-entry dissolves the extract-or-die
loot tension — a fresh boss + fresh treasure per group, no "second group finds it
cleared."

### 4. Committed space — entry / exit / death
The dungeon is a **committed** space:
- **No Town Portal inside a dungeon.** `begin_extraction` is rejected while
  `InDungeon`.
- On descent, the **overworld entrance seals behind you.** You may move **up/down
  among floors freely** (puzzles need backtracking), but the only ways *out* are
  the authored **end-exit** or **death**.
- The **end-exit** returns you to the overworld **exactly where you entered** (not
  an extraction). Clearing a dungeon is not a shortcut home — you walk back out
  hauling a fatter, more valuable backpack and **still** have to extract or die.
- **Death inside a dungeon = death.** Same backpack loss + durability sink as the
  overworld (`DbWrite::Death`), then back to town.

### 5. Traps, including disarmable ones
Traps are stateful entities in the `DungeonSpace` with a small state machine:
**armed → (triggered | disarmed)**.
- **Trigger** on movement — the `check_touch` pattern: step on an armed trap and it
  fires (damage / gauge-drain / spawn / seal).
- **Disarm** via a `run.interact` intent — the `harvest`/`open_chest` pattern.
  Disarm is a **Dex-check** any hero may attempt, and the **Shifter** (rogue /
  "Runner") is **far better** at it — a **fail springs the trap**. This gives the
  Shifter a real out-of-combat identity and a reason to bring one along. The
  manifest flags each trap `disarmable = true|false` (some are pure hazards to route
  around).

### 6. Dungeon level & loot — stamped from distance-to-Last-City
**The position problem:** inside a subinstance a chest's position is *dungeon-local*
coordinates, so today's loot path — `open_chest → (tier, distance) →
roll_creature_loot(balance, distance, CHEST_RICHNESS, …)` where `distance =
chest.position.distance_floor()` (world.rs:2528, game.rs:2698) — would scale off a
meaningless number.

**The fix:** at entry, **stamp a `dungeon_level` (an effective distance)** on the
`DungeonSpace` = the entrance's floored **distance-from-origin** (the exact measure
the whole game already uses for difficulty). Everything inside — chest loot, mob
`mlevel`, trap severity — reads the stamp, not local position. **Floors add depth:**

```
effective_distance = dungeon_base_distance + floor_index × dungeon_depth_level_step
```

So floor 0 rolls at the entrance's tier; each descent is harder **and** more
rewarding, all off the existing `distance → tier` pipeline — no new scaling math,
just a different `distance` fed in.

**Chests support two modes** (the "generated *and* defined" ask):
- **`loot = "rolled"`** — reuses `roll_creature_loot` verbatim, fed
  `effective_distance` and a richer `dungeon_chest_richness` (dungeons out-reward
  open-world chests at equal distance — the incentive to risk the committed space).
- **`contents = [...]`** — authored fixed items/gear (named artifacts, puzzle keys,
  guaranteed class rewards); bypasses the roll. Ids validated at compile time.
- **Hybrid** — a guaranteed authored item **plus** a rolled bonus.

## Authoring format — glyph grid + manifest, compiled

Authored as pure text so an **agent** (or a human, in any editor) edits it
confidently. Agents can't drive a visual tile editor, but they are excellent at
**glyph grids as text** — the roguelike-vault convention (NetHack/Brogue/Caves of
Qud) precisely because it's the most *diffable* spatial format there is.

A dungeon is a **single file** (`content/dungeons/<name>.dungeon.toml`) holding, per
floor, a **glyph grid** (fixed-width geometry) plus a dungeon-wide **`[legend]`**
(each glyph char → an object type + id) and typed **tables** (each object's params
+ wiring). Keeping the grid to **single-char** glyphs is deliberate — multi-char
ids inside a grid break column alignment, and a fixed-width grid is what makes it
diffable and coordinate-addressable. This is the built format (`meld-dungeon`); the
reference file is [`forest_barrow.dungeon.toml`](../../server/crates/meld-dungeon/content/forest_barrow.dungeon.toml):

```toml
name  = "forest_barrow"
biome = "forest"

[legend]                    # glyph → "<type> <id> [extra]"
t = "trap T1"
a = "lever L1"
X = "door D1"
"1" = "plate P1"; "2" = "plate P2"; "3" = "plate P3"
Y = "gate G1"
s = "stair S1 down"; w = "stair S1 up"   # same id, paired across floors
k = "key K1"; Z = "door D2"; B = "boss B1"; C = "chest vault"

[[floor]]                   # floor 0 — entrance level
grid = """
####################
#>.t.a.X.1.2.3.Y.s.#
####################
"""

[[floor]]                   # floor 1 — deeper (reached via the stair)
grid = """
##################
#w.k.Z.B...C...<.#
##################
"""

# --- wiring (typed tables carry params; param-less objects need none) ---
[trap.T1]  kind = "fire"  disarmable = true
[door.D1]  when = "L1"
[gate.G1]  when = "seq[P1,P2,P3]"          # ordered; use all[P1,P2,P3] for co-op
[door.D2]  when = "has_key(K1)"
[boss.B1]  sprite = "sepulcher"  on_enter_spawn = true
[chest.vault]  when = "boss_dead(B1)"  loot = "rolled"
```

Structural glyphs are implicit — `#` wall, `.` floor, ` ` void, `>` entrance,
`<` end-exit; every other glyph must appear in `[legend]`. Object ids are unique
dungeon-wide; a stair id appears twice in the legend (`down` on floor n, `up` on
floor n+1). Param-less objects (lever/plate/key/pedestal/stair) need no table;
trap/door/gate/boss/chest pull their params from a `[type.id]` table (a missing one
is a parse error).

### Puzzle vocabulary — emitters, receivers, conditions
One small composable set:

- **Emitters** (produce state/event): `lever` (latching toggle), `plate`
  (momentary or latching), `key`/`lock`, `boss_dead(id)` / `room_clear(id)`, a
  `trap`'s own state, `pedestal` (item sink), `timer` (started by a signal).
- **Receivers** (react): `door`/`gate` (open on condition), `trap.arm`/`trap.disarm`
  (a signal arms/neutralizes traps), `spawn` (a signal drops mobs — ambush),
  `mover` (raise a bridge / drop a wall — **reuses verticality's `Terrain`/
  `Connector`/`level`**).
- **Wiring** — every receiver has a `when` condition from a tiny grammar:
  `all[…]`, `any[…]`, `not X`, `seq[…]` (ordered), `count(n, […])` (N-of-M), plus
  atoms (`L1`, `P2`, `boss_dead(B)`, `has_key(K)`). `seq` and `count` are
  first-class because "step the plates in order" and "N of M" are common and
  miserable in raw booleans.

**Co-op payoff (free):** momentary `plate`s + `all[…]` = a gate needing *four
bodies on four plates at once* — content only a full human group can solve. The
"up to 4 humans" decision is what makes that class of puzzle possible.

### Compiled, with a solvability gate
A `build.rs` codegen step reads the authored files and emits validated Rust
`static DUNGEONS: &[DungeonDef]`. This fits the project's DNA (assets are already
*embedded* for the single-file dist binary; determinism is sacred) and — crucially
for **agent-authored** content — **makes the compile the correctness gate.** Build
errors fire when:
- a grid glyph has no manifest entry (or vice-versa),
- a wire references an id that doesn't exist (`L1 → door` where `L1` is undefined),
- a stair `to` points at a missing up-point, or floors don't chain,
- an authored chest `contents` id isn't in the item/gear registry,
- **the dungeon isn't solvable** — no order of operations a party can perform
  reaches `<` from `>` across the whole floor stack.

**Solvability** is a bounded fixpoint search over *puzzle state*: BFS the reachable
cells across floors (stairs are edges); an emitter becomes *operable* once reached
(lever flipped, key held, boss defeated if its room is reachable); a door/gate
opens once its `when` becomes satisfiable from operable emitters; iterate to
fixpoint; success iff `<` is reached. **Abstraction (v1):** simultaneous co-op
plates (`all[…]` on momentary plates) are treated as satisfiable when all plates
are *reachable* — a 4-body party can co-occupy them; documented, revisited if we
add solo dungeons.

## Wire / proto additions
Kept minimal, following the "extend state on `statuses`/`avatar_state` strings
rather than new fields" convention (CLAUDE.md):
- `SnapshotEntity.avatar_state` gains dungeon entity tags: `entrance:<dungeon_id>`,
  `stair:down`/`stair:up`, `trap:<kind>:<armed|disarmed>`, `lever:<on|off>`,
  `door:<open|closed>`, `plate:<on|off>`, `boss:<sprite>`.
- A `location`/space id on the player so the client knows which space it's rendering
  (overworld vs. `dungeon:<id>:<floor>`), and a `run.interact { entity_id }` intent
  (levers, disarm, stairs where explicit) mirroring `run.harvest`.
- `run.enter_dungeon` / `run.exit_dungeon` transition messages (or fold into the
  touch/`at_portal` machinery the entrance/exit reuse).

## Balance tunables (`[worldgen]`, added in the runtime phase)
`dungeon_spawn_chance` (per streamed non-tutorial section), `dungeon_depth_level_step`
(per-floor effective-distance bump), `dungeon_chest_richness`,
`dungeon_loot_rarity_bonus`, `dungeon_disarm_dex_divisor` + `dungeon_disarm_shifter_bonus`
(the Dex-check), `dungeon_trap_severity_mult`.

## Build plan (phase IDs)
- **DG-1 — authoring + validation foundation** ✅ *(shipped, #118)*. Pure crate
  `meld-dungeon`: the `DungeonDef` data model, the glyph-grid + manifest parser, the
  full emitter/receiver/condition vocabulary, and the **validator incl. the
  solvability search**. `forest_barrow` sample + 16 unit tests. No game-loop or
  client changes — isolated and testable, like `meld-world`.
- **DG-2 — `build.rs` codegen + content** ✅ *(shipped)*. New `meld-dungeon-content`
  crate: its `build.rs` runs the real parser+validator (incl. the solvability gate)
  over every `content/**/*.dungeon.toml` — **a malformed or unsolvable dungeon is a
  compile error** — and embeds the validated defs (serialized to `$OUT_DIR`) as a
  `&'static` registry (`all()` / `for_biome()` / `by_name()`). First pool:
  `verdant_barrow` (forest), `sunken_vault` (desert). *(The embedded form is a
  build-validated JSON blob deserialized once at startup — the compile-time
  guarantee is the validation, not Rust-literal construction.)*
- **DG-3 — runtime subinstance**, split so the loop-invasive half waits on SC-3:
  - **DG-3a — the pure engine** ✅ *(shipped)*. `meld-dungeon-run`: the `Location`
    model, a live `DungeonInstance` (barrier/emitter puzzle state — reaching a
    lever/plate/key/boss opens the doors/gates whose condition now holds; stairs
    between floors; end-exit detection; the committed-space rule
    `town_portal_allowed() == false`; and the per-floor `effective_distance`
    difficulty stamp), and **seeded entrance placement** from the biome pool
    (`roll_entrance`). Pure + deterministic (splitmix64), 14 unit tests + doctest,
    no `game.rs` changes — the same isolation as DG-1/DG-2.
  - **DG-3b — the `game.rs` wiring** *(pending)*. Own a map of spaces + avatar
    `Location` **inside the world-actor** (built on / after **SC-3**'s `Router` +
    `WorldActor` refactor — dungeons are content in a world, not their own shard);
    entrance placement in `ensure_frontier` (`dungeon_spawn_chance`); the
    enter/seal/exit/death flow; per-space snapshots via **SC-1**'s interest index;
    `[worldgen]` tunables (`dungeon_spawn_chance`, `dungeon_depth_level_step`).
    Deferred rather than churning today's single global instance twice.
- **DG-4 — traps + puzzles live**, engine-first like DG-3:
  - **DG-4a — the engine** ✅ *(shipped)*. The puzzle emitter/barrier runtime
    already lives in `meld-dungeon-run` (DG-3a: levers/plates/keys/boss-clear open
    doors/gates via the `Condition` grammar). DG-4a adds the **trap state machine**
    (`TrapState` armed→disarmed) with `spring_trap` (fires on contact; `severity`
    rides the floor's effective distance) and `attempt_disarm` (the **Dex check the
    Shifter is far better at**, design §5 — failure springs it; non-disarmable traps
    return `NotDisarmable`). Pure, 7 tests.
  - **DG-4b — the loop** *(pending, with DG-3b)*. The `spawn` / `mover` / `timer`
    receivers (need `meld-dungeon` model additions), the `run.interact` wire, and
    applying trap hits + interact-dispatch server-side.
- **DG-5 — loot** ✅ *(shipped)*. `DungeonInstance::resolve_chest` (in
  `meld-dungeon-run`) turns a chest's `ChestLoot` into a `ChestReward` scaled by the
  floor's `effective_distance` — **rolled** (reuses `meld_world::roll_creature_loot`),
  **authored** (fixed contents), **hybrid** (both). Rides the *stamped* distance, not
  the meaningless dungeon-local position; richness/rarity are driver params. 5 tests.
  Banking the reward into the run backpack is DG-3b.
- **DG-6 — client**, split like DG-3:
  - **DG-6a — visualizer** ✅ *(shipped)*. `meld-dungeon-viz`: `to_svg(&DungeonDef)`
    renders a top-down map of every floor (walls, entrance/exit, stairs, traps,
    levers/plates, doors/gates, keys, boss, treasure, legend) so an author can *see*
    a dungeon without the game; `dungeon-preview` bin dumps the pool. The reference
    the in-game render matches.
  - **DG-6b — in-game** *(pending)*. The live Bevy render of a dungeon space + the
    space transitions + the "you're committed" framing — needs DG-3b's wire surface
    (pending SC-3).
- **DG-7 — CANON + spec**. `D`-number, `behaviors/dungeons.md`, `interfaces/`
  updates; tick WG-1.

## Explicitly deferred
Hidden traps + a perception/detect layer; grammar/graph procedural dungeons (these
are *authored*); solo (1-player) dungeons; cross-dungeon persistent state; dungeon
matchmaking across instances.
