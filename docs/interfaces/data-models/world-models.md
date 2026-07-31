# World & Season Models

> Parent: [interfaces/data-models](../data-models.md)

Persistent world structure and endgame competition: `Hub` safe zones keyed by distance, curated `BiomeBand` distance bands, the 13-week `Season` epoch, and `VanguardBoardEntry` leaderboard records. Hubs and biome bands are largely static world definitions; seasons and board entries are live persistent state served over the HTTP API (CANON §S).

## Models

### `Hub`

A persistent safe zone. No combat occurs inside; players trade, craft, organize, and start runs here.

**Source:** GDD.md §2.1, §3 (Persistent Milestones), §4; CANON.md §G (Hub), §D (D15), §B (Hubs & run levels)

| Field | Type | Required | Nullable | Default | Since | Deprecated | Description |
|-------|------|----------|----------|---------|-------|------------|-------------|
| distance | integer (int64, one of 0, 500, 1000, …, 5000) | Yes | No | — | v0.1 | No | The hub's distance from the world origin; also its unique key. Exactly 11 curated hubs exist (structural); no hubs exist beyond 5000. |
| hub_kind | string (enum: center, outer) | Yes | No | — | v0.1 | No | The hub type. `center` is the single hub at distance 0; all others are `outer` hubs unlocked by rebuilding ruined camps after defeating the guarding Gatekeeper. |
| name | string | Yes | No | — | v0.1 | No | The display name. Content-defined. |
| base_run_level | integer (int32, ≥ 1) | Yes | No | — | v0.1 | No | The Run Level granted to parties departing from this hub: `round(1 + distance × 0.078)` [TUNABLE] — Center = 1, D500 = 40, D1000 = 79, D5000 = 391. |
| has_extraction_portal | boolean | Yes | No | `true` | v0.1 | No | Whether an extraction portal spawns here. Deterministically `true` at every hub, including Center (CANON D15). |

**Relationships**

- Has many deployed `Stall` records and posted `Contract` records via `hub_distance`.
- Guarded by the `GatekeeperBoss` at `distance − 1` (for `outer` hubs).

**Notes**

- Invariant: no combat state (`Battle`, hostile `MonsterDefinition` spawns) ever exists inside a hub (GDD §2.1).
- Hub facilities include the Vault, Training Ground (build-template skill allocation), stalls, and the bounty board (GDD §4, §7).
- Outer-hub access requires the guarding Gatekeeper cleared; whether the "rebuild" unlock is per-player or server-global is not resolved by GDD or CANON — implementers must not guess; this requires a canon ruling.
- Hubs are never wiped at season end (CANON §B, Sessions & seasons — structural).

### `BiomeBand`

A curated distance band mapping a range of the radial plane to a biome theme and its content tables.

**Source:** GDD.md §3 (Biomes & Chokepoints); CANON.md §B (Biome bands)

| Field | Type | Required | Nullable | Default | Since | Deprecated | Description |
|-------|------|----------|----------|---------|-------|------------|-------------|
| biome | string (enum: forest, desert, ashfall, tundra, mire) | Yes | No | — | v0.1 | No | The biome theme. Curated launch set; beyond distance 1500, repeating themed bands are defined by content tables per 500 (content-extensible). |
| min_distance | integer (int64, ≥ 0) | Yes | No | — | v0.1 | No | The band's inclusive lower distance bound. |
| max_distance | integer (int64, ≥ 1) | Yes | No | — | v0.1 | No | The band's exclusive upper distance bound. Curated bands (structural order): 0–100 forest, 100–300 desert, 300–500 ashfall, 500–1000 tundra, 1000–1500 mire. |
| content_table_id | string | Yes | No | — | v0.1 | No | The content-defined table governing monster spawns and loot for this band. |

**Relationships**

- Determines `Chunk.biome` for chunks whose distance falls inside the band.
- Each band border at `500k − 1` hosts a `GatekeeperBoss` arena.

**Notes**

- Invariant: bands are contiguous and non-overlapping; every distance ≥ 0 maps to exactly one band.
- Band boundaries are structural; the theme/content assignments are content-extensible per 500 beyond the curated set.

### `Season`

A 13-week leaderboard epoch. Season end archives the Vanguard Board and grants titles; nothing persistent is wiped.

**Source:** GDD.md §8 (Seasonal Wipes); CANON.md §G (Season), §D (D8), §B (Sessions & seasons)

| Field | Type | Required | Nullable | Default | Since | Deprecated | Description |
|-------|------|----------|----------|---------|-------|------------|-------------|
| id | string (uuid) | Yes | No | — | v0.1 | No | The unique season identifier. Server-assigned UUIDv7. |
| season_number | integer (int32, ≥ 1) | Yes | No | — | v0.1 | No | The sequential season index, starting at 1. |
| starts_at | string (date-time) | Yes | No | — | v0.1 | No | The season's opening instant, on a rolling UTC boundary. |
| ends_at | string (date-time) | Yes | No | — | v0.1 | No | The season's closing instant, exactly 13 weeks after `starts_at` (structural). |
| state | string (enum: active, archived) | Yes | No | `active` | v0.1 | No | The season lifecycle state. `archived` seasons are immortalized: their Vanguard Board becomes a read-only archive. |

**Relationships**

- Has many `VanguardBoardEntry` records via `season_id`.
- Referenced by `CosmeticTitle.season_id` for titles granted at season end.

**Notes**

- Invariant: exactly one season is `active` at any time; seasons are contiguous with no gap.
- At season end (CANON §B, structural): the Vanguard Board is immortalized read-only, cosmetic titles are granted to members of the top 100 instances, and the infinite-zone leaderboard resets. Vaults, hubs, meld skills, and class unlocks are NOT wiped.

### `VanguardBoardEntry`

One instance's record on the seasonal Vanguard Board — the global, real-time leaderboard of highest distance reached.

**Source:** GDD.md §8 (The Vanguard Board); CANON.md §G (Vanguard Board), §D (D3), §B (Sessions & seasons). Entry model name is spec-assigned; CANON §G names the board (`VanguardBoard`) but not its row type.

| Field | Type | Required | Nullable | Default | Since | Deprecated | Description |
|-------|------|----------|----------|---------|-------|------------|-------------|
| id | string (uuid) | Yes | No | — | v0.1 | No | The unique entry identifier. Server-assigned UUIDv7. |
| season_id | string (uuid) | Yes | No | — | v0.1 | No | The season the record was set in. |
| instance_id | string (uuid) | Yes | No | — | v0.1 | No | The MazeInstance that set the record. One entry per instance per season. |
| member_player_ids | array of string (uuid) (1–4 items) | Yes | No | — | v0.1 | No | The party members credited with the record. |
| max_distance | integer (int64, ≥ 0) | Yes | No | — | v0.1 | No | The highest floored distance reached by the instance during a single run (CANON D3) — the board's ranking key, descending. |
| achieved_at | string (date-time) | Yes | No | — | v0.1 | No | Timestamp when `max_distance` was reached. Tiebreaker: earlier `achieved_at` ranks higher. |
| rank | integer (int32, ≥ 1) | Yes | No | — | v0.1 | No | The entry's current position on the board. Recomputed in real time during the active season; frozen when the season archives. |

**Relationships**

- Belongs to one `Season` via `season_id`; references one `MazeInstance` via `instance_id`.

**Notes**

- Invariant: (`season_id`, `instance_id`) is unique; an instance's entry only ever increases its `max_distance`.
- Invariant: entries of an `archived` season are read-only, including `rank`.
- The board is global and updates in real time as instances push deeper (GDD §8); members of the top 100 instances at season end receive `CosmeticTitle` grants (CANON §B).

## Persistent World & Structures (target model — CANON §W)

> **Target model (D19–D23), not yet built.** These are the forward contracts for the persistent, player-seeded world the slice evolves toward. The current build uses the ephemeral `MazeInstance` (see [Runs & Maze](../data-models.md)); it is the **precursor**. Field shapes below are intended, **[TUNABLE]**/subject to refinement as §W is implemented.

### `World`

A persistent, player-seeded overworld shard (a "realm") — the evolution of `MazeInstance` (CANON §W1, D19). Holds its players, monster population, and player-built `Structure`s; persists as **seed + event log** (§W5). Capped; a full world queues (no auto-fork).

**Source:** CANON.md §W1/§W5 (D19, D23); [`../../proposals/server-scaling.md`](../../proposals/server-scaling.md)

| Field | Type | Required | Nullable | Default | Since | Deprecated | Description |
|-------|------|----------|----------|---------|-------|------------|-------------|
| id | string (uuid) | Yes | No | — | target | No | Unique world id. Server-assigned UUIDv7. |
| seed | integer (uint64) | Yes | No | — | target | No | The **player-chosen** world seed; all baseline terrain/placement derives from it deterministically (`section_seed(seed, n)`), so the map is regenerable and never stored. |
| name | string | No | Yes | null | target | No | Player-given world name. |
| created_by | string (uuid) | Yes | No | — | target | No | The `Player` who created the world. |
| population_cap | integer (int32, ≥ 1) | Yes | No | — | target | No | Max concurrent players; at cap the world **queues** rather than forking (it holds unique structures). [TUNABLE] |
| shift_generation | integer (int64, ≥ 0) | Yes | No | `0` | target | No | Monotonic count of Shifts applied; with `seed` it makes the natural Shift schedule replayable (§W2). |
| state | string (enum: active, hibernated, archived) | Yes | No | `active` | target | No | Lifecycle. `hibernated` = serialized to DB and evicted from RAM (reloads on first joiner); `archived` = season-closed. |
| season_id | string (uuid) | Yes | No | — | target | No | The `Season` this world belongs to; the season boundary archives it (§W5). |

**Notes**
- Persistence stores only the **event-log delta** from the seed baseline (structures, damage, harvest, anchor-altered Shifts) — not the map (§W5).
- Supersedes the ephemeral `MazeInstance` as the target (D19); `MazeInstance` remains the current precursor.

### `Structure`

**One** primitive for every player-built, HP-bearing, destructible, siege-able world object; a `function` tag varies its role (CANON §W3, D21). **`anchor` is `Structure(function: anchor)` — there is no separate `Anchor`/`Portal` model.**

**Source:** CANON.md §W3 (D21)

| Field | Type | Required | Nullable | Default | Since | Deprecated | Description |
|-------|------|----------|----------|---------|-------|------------|-------------|
| id | string (uuid) | Yes | No | — | target | No | Unique structure id. |
| world_id | string (uuid) | Yes | No | — | target | No | The `World` it stands in. |
| function | string (enum: anchor, portal, wall, stash) | Yes | No | — | target | No | `anchor` pins its region against the Shift while it stands; `portal` = plantable/defendable extraction; `wall` = defense; `stash` = field storage. |
| owner_player_id | string (uuid) | Yes | No | — | target | No | The builder/owner. |
| position | object `{x: number, y: number}` | Yes | No | — | target | No | Overworld position. |
| level | integer (uint8) | Yes | No | `0` | target | No | Terrace level it sits on (verticality, D24). |
| max_hp | integer (int64, ≥ 1) | Yes | No | — | target | No | Durability pool. [TUNABLE] |
| hp | integer (int64, ≥ 0) | Yes | No | — | target | No | Current HP; monsters siege it, and at `0` it is destroyed (an anchor's region becomes shiftable again). |
| pin_radius | number | No | Yes | null | target | No | For `function = anchor`: the radius it holds against the Shift. `null` for other functions. [TUNABLE] |
| built_at | string (date-time) | Yes | No | — | target | No | When it was built. |

**Notes**
- The four functions are one model by decision D21; the siege sim, spatial interest index, and world persistence treat them uniformly.

### `ShiftEvent`

One entry in a `World`'s event log: a Shift that fired (or was suppressed by an anchor). Replaying `(World.seed, ShiftEvent*)` reconstructs the world's current biome layout (§W2/§W5).

**Source:** CANON.md §W2/§W5 (D20, D23)

| Field | Type | Required | Nullable | Default | Since | Deprecated | Description |
|-------|------|----------|----------|---------|-------|------------|-------------|
| id | string (uuid) | Yes | No | — | target | No | Unique event id. |
| world_id | string (uuid) | Yes | No | — | target | No | The `World` this event belongs to. |
| shift_generation | integer (int64, ≥ 0) | Yes | No | — | target | No | Which generation this event is. |
| region | object `{center: {x,y}, size: enum(tiny,small,medium,large,huge,cataclysmic)}` | Yes | No | — | target | No | The swapped region (the lore size table, D20). |
| to_biome | string | Yes | No | — | target | No | The biome the region became (bestiary; content-defined). |
| occurred_at_tick | integer (int64, ≥ 0) | Yes | No | — | target | No | The server tick it fired at (deterministic scheduler, §W2). |
| suppressed_by | string (uuid) | No | Yes | null | target | No | The anchor `Structure` that pinned the region and prevented the swap, or `null` if the Shift fired. |

**Notes**
- The **natural** schedule is a pure function of `(World.seed, shift_generation)` and need not be stored; only anchor-**altered** outcomes (`suppressed_by` set) must persist (§W5).
