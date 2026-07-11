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
