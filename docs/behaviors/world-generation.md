# World Generation

Meldworld's overworld is an infinite radial plane expanding outward from the Center Hub. Every `MazeInstance` generates its own copy of the world from a per-instance seed: terrain, monster placement, chest placement, chokepoints, Gatekeeper arenas, and extraction portals are all deterministic functions of `(instance seed, chunk coordinates)`. Difficulty, monster level, and loot rarity are pure functions of `distance` — the Euclidean distance from the world origin (Center Hub) in tile units, **floored to an integer for all threshold checks**. This file specifies generation determinism, the scaling formulas, biome banding, chunk streaming, chokepoint and Gatekeeper guarantees, extraction portal placement, loot banding, and infinite scaling past the final curated hub.

**Source:** GDD.md §3, §8; CANON.md §B (Distance → difficulty, Hubs & run levels, Biome bands), §D15, §G (MazeInstance, Chunk, Distance), §S

Related: [run-lifecycle.md](./run-lifecycle.md) (what happens at portals and Gatekeepers), [combat-atb.md](./combat-atb.md) (how `mlevel`/`stat_mult` feed battles), [../interfaces/realtime-protocol.md](../interfaces/realtime-protocol.md) (`world.*` chunk sync messages).

---

## Flow: Deterministic Seeded Generation

**Source:** GDD.md §3; CANON.md §G (MazeInstance, Chunk), §D12, §S (Rust server owns world gen)

1. At maze-entry time, the server creates a `MazeInstance` and assigns it a world seed (server-generated; the client never supplies or computes seeds).
2. All world content for that instance — tile layout, biome detailing, monster spawns, chest spawns, chokepoint geometry, Gatekeeper arenas, procedural extraction portals — is derived deterministically from `(instance_seed, chunk_coords)`. Regenerating the same chunk for the same instance always yields identical content.
3. Two different `MazeInstance`s (even for the same party, back to back) have different seeds and therefore different world layouts, except for the **structural guarantees** below (hub positions, Gatekeeper arena distances, biome band boundaries), which hold in every instance regardless of seed.
4. World generation is entirely server-side. Clients receive generated chunk data over the realtime channel and render it; they never generate or validate world content themselves.
5. Instance world state is ephemeral: it lives in server memory (with periodic snapshots for crash recovery only) and is discarded when the instance closes. Nothing about a generated world persists across instances.

### Structural guarantees (seed-independent)

| Guarantee | Value | Tunability |
|-----------|-------|------------|
| Curated Hubs exist at | `distance = 0, 500, 1000, 1500, …, 5000` (11 hubs) | structural |
| Gatekeeper arenas exist at | `distance = 500·k − 1` for `k = 1..10` (i.e. 499, 999, …, 4999) | structural |
| Biome band order | see Biome Bands table below | structural (content-extensible) |
| No hubs beyond `d = 5000` | infinite scaling zone only | structural |
| Endgame scaling formula shape | exponential (see Infinite Scaling) | structural |

**Source:** CANON.md §B (Hubs & run levels, Biome bands, Distance → difficulty)

---

## Distance → Difficulty Formulas

All formulas operate on integer `distance` (`d`), the floored Euclidean distance from world origin in tile units. All constants are **[TUNABLE]** server config unless marked structural.

**Source:** CANON.md §B (Distance → difficulty), §G (Distance); GDD.md §3

| Quantity | Formula | Examples | Tunability |
|----------|---------|----------|------------|
| Loot/monster tier band | `tier(d) = floor(d / 100)` | d=0→0, d=99→0, d=100→1, d=499→4 | [TUNABLE] |
| Monster level | `mlevel(d) = max(1, round(d / 12.5))` | d=0→1, d=500→40, d=1000→80, d=5000→400 | [TUNABLE] |
| Monster stat scale (d ≤ 5000) | `stat_mult(d) = (1 + d/500)^1.25` | d=0→1.0, d=500→2^1.25 ≈ 2.378, d=5000→11^1.25 ≈ 20.11 | [TUNABLE] |
| Monster stat scale (d > 5000) | `stat_mult(d) = stat_mult(5000) × 1.5^((d − 5000)/500)` | d=5500 → stat_mult(5000)×1.5 | exponential shape structural; constants [TUNABLE] |

Notes:

- `mlevel(500) = 40` deliberately matches `base_run_level` of the D500 Hub (see [run-lifecycle.md](./run-lifecycle.md)), so monsters at a hub's distance are level-matched to a fresh run started from that hub.
- The two `stat_mult` branches are continuous at `d = 5000` (the exponential factor is `1.5^0 = 1` there).

---

## Biome Bands

Difficulty is a pure function of `distance` (below); the **biome is a difficulty-neutral *skin*** — it picks the section's theme (creature/resource/obstacle tables) but never its difficulty, since creature stats scale from `distance` via `stat_mult` at spawn. So biome *ordering* is randomized per run without affecting fairness (roadmap WG-2/WG-3; design in [`../proposals/worldgen-wg.md`](../proposals/worldgen-wg.md)):

- **Tutorial run** (an account's first dive, gated on the persistent `has_dived` flag): biomes walk the **fixed distance bands** in the table below — the gentle Forest→Desert→… onboarding (plus the centred, single-creature area 0). The seed is still server-random; the tutorial shapes the biome *order* and area-0 structure, not a fixed world.
- **Every other run:** each section draws a biome per `section_seed`, excluding the previous section's biome (no adjacent repeat). The *start* biome is randomized too (WG-2), and the *order* varies per run (WG-3). The fixed table below is the tutorial order and the difficulty-band reference; it is no longer the biome order for non-tutorial runs.

**Source:** CANON.md §B (Biome bands); GDD.md §3

| Distance band | Biome |
|---------------|-------|
| 0–100 | Forest |
| 100–300 | Desert |
| 300–500 | Ashfall |
| 500–1000 | Tundra |
| 1000–1500 | Mire |
| 1500+ | Repeating themed bands defined by content tables, one per 500 distance |

Band boundaries use integer `distance` thresholds (a tile at floored distance exactly 100 is in the Desert band, consistent with `tier(100) = 1` starting a new band).

---

## Flow: Chunk Streaming

**Source:** GDD.md §3; CANON.md §G (Chunk), §B (Networking targets), §D12, §S

1. The overworld is partitioned into square `Chunk`s of **64×64 tiles [TUNABLE]**.
2. Chunks are generated **on demand**: the server generates a chunk the first time any party member's position (or interest radius) requires it. Nothing is pre-generated for the whole (infinite) world.
3. The server streams chunk content to each client over the realtime channel for chunks within the client's interest radius (**2 chunks [TUNABLE]**, a non-binding performance target). See [../interfaces/realtime-protocol.md](../interfaces/realtime-protocol.md), `world.*` messages.
4. Generated chunks are cached in server memory for the lifetime of the `MazeInstance`. Because generation is deterministic, the server may evict and regenerate distant chunks freely with no observable difference — except for **mutable overlay state** (dropped items, opened chests, cleared flags, deployed `WardItem`s, sleeping avatars), which is instance state tracked independently of the deterministic base terrain.
5. When the instance closes (see [run-lifecycle.md](./run-lifecycle.md) for close conditions), all chunks and overlay state for that instance are **discarded**. The world is never persisted.

---

## Chokepoint Generation Guarantees

**Source:** GDD.md §3; CANON.md §B (Hubs & run levels)

1. Procedural generation forces geographic chokepoints (bridges, canyon passes, etc.) at intervals so that outward travel funnels players through shared narrow routes. Chokepoint frequency and geometry are content-table driven **[TUNABLE]**.
2. Guarantee: the maze is never a fully open plane — every path from a hub to the next tier band crosses at least one generated chokepoint.
3. Guarantee (reachability): generation never produces an unreachable ring; for every `distance` there exists at least one traversable path from the Center Hub, subject only to Gatekeeper blockers (below). The traversable path may change **elevation** (it can climb a plateau and descend again), but every such level change on the guaranteed path is served by a ramp connector, so the route is always completable — verified across seeds by the `meld-world` clear-path tests. See [`VERTICALITY-PROPOSAL.md`](../proposals/verticality.md).

---

## Flow: Gatekeeper Arena Placement

**Source:** GDD.md §3, §4; CANON.md §B (Hubs & run levels: Gatekeeper arenas), §G (GatekeeperBoss)

1. At `distance = 500·k − 1` for `k = 1..10` (structural), generation places a `GatekeeperBoss` arena.
2. The arena is a **full-width chokepoint blocker**: it spans the entire traversable width of its distance ring. There is no path from `d < 500·k − 1` to `d ≥ 500·k` that avoids the arena.
3. Each instance tracks a **per-instance clear flag** per Gatekeeper arena. While the flag is unset, the arena blocks passage; touching the `GatekeeperBoss` starts (or merges into) its Battle (see [combat-atb.md](./combat-atb.md)).
4. When the Gatekeeper is defeated, the instance's clear flag for that arena is set, the blocker opens, and all members of the instance may pass for the remainder of the instance. The flag is instance-scoped ephemeral state — a new run faces the Gatekeeper again.
5. Persistent consequences of a Gatekeeper kill (e.g. a `ClassEmblem` drop such as "Emblem of the Dragoon", Outer Hub unlock eligibility) are separate from the clear flag: the emblem drops into the killer's loot as run loot, and hub unlocks are account-level progression (GDD.md §4). Only those persistent effects survive the instance.

---

## Extraction Portal Placement

**Source:** CANON.md §D15; GDD.md §2.2

| Rule | Value | Tunability |
|------|-------|------------|
| Deterministic portals | One extraction portal at **every Hub**, including the Center Hub | [TUNABLE] (part of D15) |
| Procedural portals | ~1 portal per **200-distance band** per instance, placed deterministically from the instance seed | [TUNABLE] |
| Escape item | `ripcord_scroll` extracts from anywhere (no portal needed) via a 10 s interruptible channel — see [run-lifecycle.md](./run-lifecycle.md) | [TUNABLE] |

Procedural portal positions are a function of the instance seed: the same instance always has portals in the same places; different instances differ. Portals are usable by any member of the instance's `Party` and are never consumed by use.

---

## Loot Rarity Banding

**Source:** CANON.md §B (Distance → difficulty: loot rarity); GDD.md §3, §2.2

1. Loot rarity weights shift **one band per tier**: each increment of `tier(d)` shifts the rarity weight table one band toward rarer outcomes **[TUNABLE]**.
2. **Red-chest floor:** `GearItem`s with `insurance: red` cannot spawn below `d = 300` **[TUNABLE]**. Chests and monster drops at `d < 300` never yield red gear, regardless of rarity roll.
3. Resource stratification (GDD.md §4): low-tier raw materials required for base crafting spawn only near the Center Hub; high-tier hubs drop rare materials. Exact material tables are content-defined.
4. All loot rolls are server-side (CANON.md §S); the client is informed of results only.

---

## Infinite Scaling (d > 5000)

**Source:** GDD.md §8; CANON.md §B (Distance → difficulty, Hubs & run levels), §D3

1. Past the final curated Outer Hub at `d = 5000`, generation continues indefinitely: no further hubs, no further Gatekeeper arenas (structural).
2. Monster stats follow the exponential branch: `stat_mult(d) = stat_mult(5000) × 1.5^((d − 5000)/500)`.
3. Monsters in the infinite zone drop "Prestige" cosmetic aura items (content-defined tables) **[TUNABLE]**.
4. The maximum integer `distance` reached by the instance during a run feeds the `VanguardBoard` (highest distance per instance per season); leaderboard mechanics are out of scope here.

---

## Invariants

1. **Determinism:** For a fixed instance seed, chunk content (base terrain, spawns, portals, arenas) is identical no matter when, in what order, or how many times chunks are generated.
2. **Server authority:** No world content is ever generated, decided, or validated client-side.
3. **Ephemerality:** No generated world data survives instance close; the only cross-instance world facts are the structural guarantees (hub/arena distances, biome bands).
4. **Monotone difficulty:** `tier`, `mlevel`, and `stat_mult` are non-decreasing in `d`.
5. **Gatekeeper impassability:** while an arena's per-instance clear flag is unset, no movement path crosses its distance ring (movement validation is server-side and enforces this).
6. **Red-chest floor:** no `insurance: red` gear ever spawns at `d < 300`.
7. **Integer thresholds:** every distance comparison (tiers, biomes, red-chest floor, arena placement, hub placement) uses floored integer `distance`.

---

## Edge Cases

- **d exactly at a band boundary:** boundaries are half-open low-inclusive by convention of `tier(d) = floor(d/100)` — e.g. `d = 100` is tier 1/Desert, `d = 300` is the first distance where red gear may spawn (`d ≥ 300`).
- **`mlevel` rounding:** `round(d / 12.5)` uses round-half-to-nearest; `d ≤ 6` yields `mlevel = 1` only via the `max(1, …)` clamp (`round(0/12.5) = 0`).
- **Chunk eviction vs. mutable state:** if the server regenerates an evicted chunk, opened-chest/cleared/dropped-item overlay state must still be reflected — deterministic regeneration alone is not sufficient for chunks that were mutated.
- **Party members far apart:** each member streams their own interest radius; the same instance may have many disjoint generated regions concurrently.
- **Gatekeeper arena chunk spans:** an arena at `d = 499` may span multiple chunks; generation must place the full-width blocker coherently across chunk borders regardless of which chunk is generated first.
- **Portal in an unvisited band:** a procedural portal "exists" deterministically even if its chunk was never generated; it becomes observable only when the chunk streams in.
- **d > 5000 numeric growth:** `stat_mult` grows exponentially without bound; implementations must use saturating/floating-point-safe math and define behavior at extreme distances (overflow must not wrap).
- **Gap (not resolved by CANON):** exact chokepoint frequency, arena interior layout, and Prestige drop tables are content-team deliverables; specs constrain only the guarantees above.
