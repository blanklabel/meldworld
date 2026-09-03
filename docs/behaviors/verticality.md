# Verticality (Terraces & Connectors)

> ## ⚠️ RETIRED — this describes code that no longer exists
>
> Terraced verticality was **deleted** in `WG-11` stage 5: the per-section elevation grid,
> the connectors (slope/ladder/rope), cliffs-as-walls, their wire fields
> (`TerrainSection.levels`/`connectors`/`cell`/`cols`/`rows`/`y_min` and `ConnectorDto`),
> the client's stepped ground+cliff mesh and connector props, the `Terrain` / `Connector` /
> `ConnectorKind` types, and seven `[worldgen]` tunables.
>
> It had been **provably unreachable** for a long time before that. `raise_terrace` was the
> only writer of a level and it ran `terraces_per_area × biome_terrace_mult = 0` times, so
> the grid was all zeros and the client's mesh sat behind an `if any(level > 0)` that could
> never fire. The one thing still exercising it was a test fixture (`corridor_balance()` set
> `terraces_per_area = 3.0`).
>
> **What replaced it:** relief is the continuous **heightmap** plus **PEAKS** — walkable
> domes summed into the height field, crowned with a gate boss or a guaranteed chest — which
> is world-space and cell-agnostic, and so is what "cells replace `Area` as the structural
> unit" asks for. `area_level_at` remains as one function returning 0 and
> `SnapshotEntity.level` remains an always-zero entity property, because a level is still a
> real concept in a dungeon.
>
> Kept for the history of the decision, and because the *feasibility* discipline it
> established — the clear path stays on one level, barriers yield to the drawn route — is
> the discipline the cell-graph maze inherited. Do not implement against it.


The overworld is not a flat plane: each section carries **elevation**. Height is modelled as a small number of **discrete integer levels** (terraces / plateaus), not a smooth heightmap. Terraces are separated by **cliffs**, which are impassable walls, and the *only* way to change level is by stepping onto a placed **connector** (a slope, ladder, or rope) — there is no free-form climbing. Elevation gates traversal and enables hidden loot, shortcuts, and vantage without touching the difficulty curve, which stays a pure function of `distance` (see [world-generation.md](./world-generation.md)). This file specifies the observable rules of terraced verticality: the level model, cliff impassability, connector-only level changes, the elevation-aware interactions, the clear-path feasibility invariant, and the wire surface.

**Source:** GDD.md §3; CANON D24 (verticality — elevation levels, cliffs-as-walls, connectors-only, path feasibility); CANON.md §S (server-authoritative), §G (distance-only difficulty). *CANON D24 is the pending canonical home for these rules.*

Related: [world-generation.md](./world-generation.md) (per-section streaming, the guaranteed clear path this feasibility rule rides on), [../interfaces/realtime-protocol.md](../interfaces/realtime-protocol.md) (the `world.terrain_section` message and `SnapshotEntity.level`).

---

## The level model

1. Every world position has an integer elevation **`level: u8`** (0 = ground). Elevation is quantised into a small number of discrete levels (bounded by a `max_level` **[TUNABLE]**), stored as a coarse per-cell grid per section — **not** a continuous heightmap.
2. Terraces are raised rectangular plateaus at a single `level`, generated deterministically from the section's seed (see [world-generation.md](./world-generation.md), per-section streaming). Same seed ⇒ same terraces, cliffs, and connectors; nothing about the terrain is client-authored.
3. Every avatar, creature, chest, and resource node carries the `level` of the cell it stands on. It is set at spawn and, for a moving avatar, changes **only** by traversing a connector.

---

## Cliffs are impassable walls

1. A boundary between two adjacent cells of different `level` is a **cliff**. A cliff is a solid wall: movement into it is blocked and the mover **slides** along it, exactly as with an obstacle.
2. There is no "climbable" cliff surface. An avatar can never gain or lose elevation by walking into a cliff — the only outcome is block-and-slide.
3. `check_touch`, `harvest`, and battle join-radius all compare **level as well as position**: you cannot touch, harvest, or join across a cliff (a creature one terrace up is out of reach until you climb to its level).

---

## Connectors are the only way to change level

1. A **connector** is a placed entity (like an obstacle or portal) that joins two levels `lo` and `hi`. It has one of three kinds:
   - **Slope / ramp** — a walkable incline with a footprint span; you walk up or down it and your `level` changes as you cross it. Continuous, no special state.
   - **Ladder** — a near-point footprint; mount its base and travel along its axis to the far level (and back down from the top).
   - **Rope** — a ladder-equivalent flavoured for descent down a cliff.
2. An avatar's `level` changes **if and only if** it is on a connector joining its current level to the target level. Off a connector, a level change is impossible. This is enforced server-side in `apply_move`.
3. Generation guarantees every terrace is reachable: at least one connector is placed to join each raised terrace to an adjacent level, so no terrace is stranded.

---

## Clear-path feasibility invariant

1. The guaranteed hub→portal **clear path** (see [world-generation.md](./world-generation.md), Chokepoint guarantees) is always completable on foot, even though it may itself climb. A section's clear path may rise onto a plateau over the interior of its segment and drop back down.
2. Feasibility is preserved **by construction**: wherever the clear path crosses a level boundary, a **Slope connector** (with reach ≥ the path's clear radius) sits on the path at that boundary, so a walker that follows the waypoints climbs the ramp and stays grounded the whole way.
3. Both endpoints of every section's clear path stay on **level 0**, so section seams, the extraction portal, and streaming are unaffected by interior climbs. Side terraces off the path are optional detours (grind + treasure), never required to reach the exit.
4. These guarantees are verified across seeds by the `meld-world` clear-path tests (e.g. the plateau-climb path reaches the portal grounded; no terrace intrudes on the clear-path tube).

---

## Wire surface (additive, backward-compatible)

1. `Position` stays 2-D `{x, y}`; elevation rides alongside it. `SnapshotEntity.level: Option<u8>` reports each dynamic entity's terrace — absent means ground level 0, and old clients ignore it. The client raises the entity's render height by `level × step_height`.
2. Static terrain streams on the **`world.terrain_section`** message (`TerrainSection`): the section's elevation grid (`levels`, row-major), its `connectors` (kind, position, `lo`/`hi`, radius), and its clear-path contribution. It is sent once per initial-chain section at run start and again for each new section the server streams in as the player advances.
3. **Movement intents are unchanged** — the client sends 2-D `{dx, dy}` and the server owns all elevation and collision resolution (CANON.md §S). Walking onto a slope walks you up it; walking onto a ladder/rope base mounts and climbs it; walking into a bare cliff slides.

---

## Invariants

1. **Discrete levels only.** Elevation is an integer `level` per cell, bounded by `max_level`; there is no continuous height.
2. **Cliffs are walls.** A level boundary that is not served by a connector is impassable; movement into it always block-and-slides.
3. **Connector-only level change.** An avatar's `level` changes only while on a connector joining those two levels; no free climbing exists anywhere.
4. **Difficulty is elevation-independent.** `tier`, `mlevel`, and `stat_mult` are functions of `distance = floor(hypot(x, y))` only; elevation never feeds them (CANON.md §G).
5. **Extraction always feasible.** Every level change on the guaranteed clear path is served by a ramp connector, and both section-path endpoints stay on level 0 — so a route home always exists by construction.
6. **Server-authoritative & deterministic.** All terrain, connectors, and elevation resolution are server-owned and derived from the section seed; the client rebuilds identical relief from the streamed terrain payload.
