# Verticality (terraces + ladders/ropes/slopes)

> ## ⚠️ RETIRED — this describes code that no longer exists
>
> Terraced verticality was **deleted** in `WG-11` stage 5: the per-section elevation grid,
> the connectors (slope/ladder/rope), cliffs-as-walls, their wire fields
> (`TerrainSection.levels`/`connectors`/`cell`/`cols`/`rows`/`y_min` and `ConnectorDto`),
> the client's stepped ground+cliff mesh and connector props, and seven `[worldgen]`
> tunables.
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


**Status: SHIPPED — graduated.** Terraced verticality (discrete elevation levels, cliffs
as impassable walls, connectors as the only way to change level, and the clear-path
feasibility guarantee) is live and unit-/integration-tested.

The behavior it specified now lives in:

- **[`../behaviors/verticality.md`](../behaviors/verticality.md)** — the observable spec (level model, cliffs, connectors, feasibility invariant, wire surface).
- **CANON D24 (verticality)** — the canonical rules the spec cites.
- Per-section streaming and the radial world are specified in [`../behaviors/world-generation.md`](../behaviors/world-generation.md).

This stub is kept so existing links don't 404.
