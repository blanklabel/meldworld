# Dungeons (Hand-Designed Sub-Spaces)

A **dungeon** is a hand-authored, multi-floor sub-space — the opposite pole from the infinite seeded overworld. As you roam the streaming overworld, a biome section can host a **dungeon entrance**; stepping up to it and choosing to descend drops your group into the dungeon's own space, laid out by a designer with **puzzles, stairs, traps, a boss, and treasure**. It is a **committed space**: once inside, you leave only by reaching the authored **end-exit** or by **dying**. Every dungeon is a **per-entry-fresh** copy shared by the group that entered it (up to 4), so co-op parties each get their own run at the content.

**Source:** GDD.md §2.1 (the maze); CANON.md §D25 (dungeons), §D6 (death/durability), §S (server-authoritative); ROADMAP WG-1. Design of record: [`../proposals/dungeons.md`](../proposals/dungeons.md).

**Related:** [world-generation.md](./world-generation.md) (the overworld the entrance sits in, distance→difficulty), [run-lifecycle.md](./run-lifecycle.md) (death ends the run the same way), [combat-atb.md](./combat-atb.md) (the boss fight runs on the ATB engine), [../interfaces/realtime-protocol/run-social.md](../interfaces/realtime-protocol/run-social.md) (`run.enter_dungeon`, `run.open_chest`).

> **Authored, not generated.** Seeded generation decides only *whether* a section hosts an entrance, *which* dungeon from the biome pool, and the *loot rolls*. The layout, puzzles, traps, boss, and treasure placements are fixed by the author and compiled into the binary behind a build-time **solvability gate** — a dungeon that has no route from entrance to exit fails the build, so every shipped dungeon is provably completable.

---

## Entrances

**Source:** CANON.md §D25; world-generation.md (per-section streaming).

- As the overworld streams, each **non-tutorial** section (the initial chain and every streamed section) is rolled once for an entrance: with probability `dungeon_spawn_chance` **[TUNABLE]** it hosts one, drawn from its biome's authored pool. A biome with no authored dungeons never spawns one. The tutorial (first-dive) world is entrance-free.
- The entrance is placed on the section's **guaranteed clear path** (a reachable, walkable spot) and streams to clients as a `entrance:<dungeon>` world entity.
- Rolls are **deterministic** in the section seed: the same world re-rolls the same entrances.

## Entry — deliberate, co-op, per-entry-fresh

**Source:** CANON.md §D25; proposals/dungeons.md §3–§4.

- Descent is a **deliberate action** — `run.enter_dungeon { entity_id }`, sent while standing within interaction range of an entrance. **Walking past an entrance never pulls you in.** Rejected (with an error) if you are already in a dungeon, in battle, or out of range.
- On entry, a **fresh subinstance** of the authored dungeon is created and **stamped** with a difficulty: `effective_distance(floor) = entry_distance + floor × dungeon_depth_level_step` **[TUNABLE]**, where `entry_distance` is the entrance's overworld floored distance. Everything inside (boss, traps, rolled loot) scales off this stamp — **never** the meaningless dungeon-local position; deeper floors are harder *and* richer.
- **Group entry:** every teammate gathered at the entrance (active, in the overworld, within `[ai] join_radius`) descends **together** into that one subinstance — a co-op group of up to 4. A dungeon already in progress is **not** joinable afterward.
- Each entering player's overworld avatar is **frozen at the entry position** while they are inside; it is restored there on exit (you return exactly where you came in).
- Because each entry mints a fresh copy, several groups (or the same group re-entering) each get their **own** dungeon with its own boss and treasure.

## The committed space — exits and Town Portal

**Source:** CANON.md §D25 (committed space), §D6.

- **No Town Portal inside a dungeon.** `run.begin_extraction` is rejected while you are in a dungeon.
- You leave only by:
  - the authored **end-exit** — stepping onto it returns you (and only you) to the overworld at your entry position; or
  - **death** — a wipe ends the run exactly like an overworld death (see below).
- Moving **up/down among floors** (via stairs) is free; only *leaving* is gated. The overworld entrance is effectively sealed behind you until you clear the dungeon.

## Traversal — movement, puzzles, stairs

**Source:** CANON.md §D25; proposals/dungeons.md §"Puzzle vocabulary".

- **Movement** inside a dungeon is scoped to the current floor: you slide along walls, and a **closed door/gate blocks** passage.
- **Puzzles** are emitters wired to barriers by a boolean `when` grammar. Reaching an emitter activates it, and any barrier whose condition then holds opens:
  - **Emitters:** `lever` (latches on), `plate` (pressed), `key` (`has_key`), `boss_dead` (the boss defeated).
  - **Barriers:** `door` / `gate`, opening when `when` holds.
  - **Grammar:** `all[…]`, `any[…]`, `not X`, `seq[…]` (in order), `count(n, […])` (N-of-M), plus atoms and `has_key(id)` / `boss_dead(id)`.
  - Ordered/`all` plate gates enable **co-op puzzles** — e.g. a gate needing bodies on several plates.
- **Stairs** join floors: stepping onto a stair endpoint transitions you to its paired endpoint on the neighbouring floor. Stairs are the only way to change floor.

## Traps

**Source:** CANON.md §D25; proposals/dungeons.md §5.

- Every trap starts **armed**. Stepping onto an armed trap's cell **fires it**, dealing HP damage to the stepping player's party: `dungeon_trap_damage` **[TUNABLE]** base, scaled up by the floor's `effective_distance`. Firing does not disarm it (a persistent hazard); it fires again on re-entering the cell.
- A trap fires only when you **enter** its cell, not while you linger on it.
- **Disarmable** traps (authored flag) can be neutralised via a **Dex check the Shifter is far better at**: `p = dex / dex_divisor (+ shifter_bonus)`, clamped; on success the trap is inert, on **failure it springs**. A non-disarmable trap can't be disarmed — you route around it.
- A trap that reduces the party to 0 HP is a **death** (below).

## Boss combat

**Source:** CANON.md §D25; combat-atb.md.

- Entering the **boss's cell** starts a boss fight (once, until the boss is dead) through the normal ATB battle engine. The boss is an **FS-4 named boss** whose combat stats are scaled to the dungeon's stamped `effective_distance`.
- **Victory** marks the boss dead (`boss_dead(id)` becomes true — which can unlock a gated chest) and returns the survivors to the dungeon.
- **Defeat** (a wipe) ends the run in **death** (below).

## Treasure

**Source:** CANON.md §D25; proposals/dungeons.md §6; economy.md (chest loot S2).

- A dungeon chest is looted with `run.open_chest { entity_id }` while standing by it, **once**, and only if its `when` condition holds — most vaults are gated on `boss_dead`, so **you loot the vault by first killing the boss**.
- Contents are **rolled** (material + chits + gear, scaled to the stamped `effective_distance`, richer than an open-world chest) and/or **authored** (fixed designer contents). The reward banks into the run **backpack** (`run.backpack_update`) — extract it to keep it, lose it on death like any other haul.

## Death & exit

**Source:** CANON.md §D25, §D6; run-lifecycle.md.

- **Death in a dungeon** (a trap wipe or a lost boss fight) ends the run identically to an overworld death: `run.member_result { died }`, the backpack forfeited, the death-durability sink applied. The player is dropped from the dungeon; if the dungeon empties, it is discarded.
- **Clearing the dungeon** (reaching the end-exit) returns you to the overworld at your entry position, carrying whatever you looted — you have **not** extracted, so you still have to make it home or die.

## Wire surface

**Source:** CANON.md §I, §S; interfaces/realtime-protocol.

- **C2S:** `run.enter_dungeon { entity_id }` (deliberate descent), `run.open_chest { entity_id }` (also loots dungeon chests), `movement.move_intent` (scoped to the dungeon while inside).
- **S2C:** an in-dungeon player's `world.snapshot` is scoped to their dungeon floor — the floor is mapped onto existing entity tags (walls/closed doors as obstacles, the exit as a portal, chest/boss as their usual tags).
- **S2C:** `world.dungeon_scene { active, theme, floor, width, height }` — the client's cue to re-skin the whole environment as a **secluded, themed space** rather than the open overworld. Sent on descent and on every floor change (`active = true`, with the floor's biome `theme` + grid bounds), and once on exit/death (`active = false`). It is **purely presentational**: the authoritative playable floor is still the `snapshot` walls. Given the theme + bounds, the client (client-side only) swaps the ground/sky mood and rings the play area with a dense, collision-free biome enclosure — for a `forest` dungeon, a Guardia-Forest-style canopy: low shrubs at the clearing rim (so the hero stays visible) rising to towering trees that fill the frame, so no overworld shows through. It is emitted only on a transition (diffed against the last-sent scene), never every tick.

## Client rendering — the secluded space

**Source:** ROADMAP WG-1/DG-6b; presentation only (no gameplay rides on it).

Driven by `world.dungeon_scene`, the client renders an in-dungeon floor as an enclosed, themed space, not the open overworld:

- **Interior maze walls** (the `dungeon_wall`/`dungeon_door` obstacle cells) render per theme — a `forest` dungeon plants **low foliage** (squat bushes kept below the hero's height, so you always see your character to steer); non-forest themes keep tinted stone/timber masonry (ruins suit desert/ashfall/tundra/mire).
- **Enclosure:** the play area is ringed by a deep, collision-free biome belt whose prop height ramps with distance — a low rim by the clearing rising to a tall backdrop — so the frame reads as forest (or the biome's equivalent) to the horizon and the overworld never shows, even zoomed out.
- **Mood:** the sky/light dim to a themed, enclosed half-light; overworld terraces and biome-edge cliff/treeline framing are hidden while underground and restored on exit.

## Tunables

`[worldgen]`: `dungeon_spawn_chance`, `dungeon_depth_level_step`, `dungeon_trap_damage`. `[ai]`: `join_radius` (group entry). `[world]`: `interaction_radius_tiles` (entrance/chest reach). Boss scaling reuses `[world_scaling]` + `[encounters]` (Gatekeeper multipliers); disarm reuses a Dex-check tunable.

## Edge cases

- **Empty biome pool:** a biome with no authored dungeon never spawns an entrance — no error, just no dungeon there.
- **Latecomers:** a teammate who arrives at the entrance *after* the group descended cannot join that copy (per-entry-fresh, not-joinable-in-progress). They may start their own fresh copy.
- **Unsolvable content is impossible to ship:** the build-time solvability gate rejects any dungeon with no entrance→exit route, so a committed space is never a dead end by construction.
- **Boss already dead:** re-entering the boss cell after victory does not restart the fight; the gated chest stays unlocked.
- **Difficulty is position-independent inside:** two dungeons entered at the same overworld distance scale identically regardless of their internal layout size, because everything reads the stamped `effective_distance`.
