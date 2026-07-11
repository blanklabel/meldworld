# Run & Maze Models

> Parent: [interfaces/data-models](../data-models.md)

Ephemeral excursion state: the shared `MazeInstance` world, the `Party` inside it, each member's `Run` and `Backpack`, realtime `AvatarState`, and the streamed `Chunk` world descriptor. All models here live in server memory with periodic snapshots for crash recovery (CANON D12); only run outcomes (banked loot, durability penalties, Vanguard records) touch the persistent store, applied by the server at run end.

## Models

### `MazeInstance`

The 1–4 player shared maze world for one run set, with its own world seed.

**Source:** GDD.md §3, §5 (4-Player Instance); CANON.md §G (Instance), §D (D13, D15), §B (Hubs & run levels; Disconnect handling)

| Field | Type | Required | Nullable | Default | Since | Deprecated | Description |
|-------|------|----------|----------|---------|-------|------------|-------------|
| id | string (uuid) | Yes | No | — | v0.1 | No | The unique instance identifier. Server-assigned UUIDv7. |
| world_seed | integer (int64) | Yes | No | — | v0.1 | No | The seed driving all procedural generation for this instance: chunk layout, chokepoints, loot rolls, and extraction-portal placement. |
| departure_hub_distance | integer (int64, one of 0, 500, …, 5000) | Yes | No | — | v0.1 | No | The distance of the hub the party departed from. Determines the party's base run level. |
| state | string (enum: active, closed) | Yes | No | `active` | v0.1 | No | The instance lifecycle state. Closes when every member's run is terminal, or after 60 minutes with all members disconnected [TUNABLE]. |
| created_at | string (date-time) | Yes | No | — | v0.1 | No | Timestamp of instance creation at maze entry. Server-assigned. |
| closed_at | string (date-time) | No | Yes | null | v0.1 | No | Timestamp when the instance closed. `null` while active. |
| max_distance_reached | integer (int64, ≥ 0) | Yes | No | `departure_hub_distance` | v0.1 | No | The highest floored distance any member has reached during this instance. Feeds the Vanguard Board. |
| cleared_gatekeeper_distances | array of integer (int64) | Yes | No | `[]` | v0.1 | No | Distances of Gatekeeper arenas this instance has cleared (each of the form `500k − 1`). Arenas are full-width chokepoints; there is no path past an uncleared arena. |

**Relationships**

- Has exactly one `Party` via `Party.instance_id`.
- Has one `Run` per party member via `Run.instance_id`.
- Has many `AvatarState` and deployed `WardItem` entries while active.

**Notes**

- Invariant: an instance is created at maze-entry time and is never joinable afterward; battle merges merge battles, not instances (CANON D13).
- Invariant: `state = closed` implies every associated run is in a terminal state.
- When the 60-minute all-disconnected timeout fires, remaining sleeping avatars auto-abandon: backpacks are deleted (as on death) but no gear durability loss is applied (CANON §B, Disconnect handling).
- Extraction portals spawn deterministically at every hub plus roughly one per 200-distance band, derived from `world_seed` [TUNABLE] (CANON D15).

### `Party`

The 1–4 players inside one MazeInstance.

**Source:** GDD.md §5 (4-Player Instance); CANON.md §G (Party), §D (D13)

| Field | Type | Required | Nullable | Default | Since | Deprecated | Description |
|-------|------|----------|----------|---------|-------|------------|-------------|
| id | string (uuid) | Yes | No | — | v0.1 | No | The unique party identifier. Server-assigned UUIDv7. |
| instance_id | string (uuid) | Yes | No | — | v0.1 | No | The MazeInstance this party inhabits. One party per instance. |
| member_player_ids | array of string (uuid) (1–4 items) | Yes | No | — | v0.1 | No | The players in the party, fixed at maze entry. Parties form in a hub; solo players may opt into a matchmaking pool filtered by departure hub. |
| formed_at | string (date-time) | Yes | No | — | v0.1 | No | Timestamp when the party was formed. Server-assigned. |

**Relationships**

- Belongs to one `MazeInstance` via `instance_id`.

**Notes**

- Invariant: 1–4 unique members; membership never changes after instance creation (CANON D13).
- Every member shares the same `base_run_level`, set by the departure hub (GDD §4).

### `Run`

One player's ephemeral maze excursion. Ends in exactly one of `extracted`, `died`, or `abandoned`.

**Source:** GDD.md §2.2; CANON.md §G (Run, Run Level), §B (Hubs & run levels; Death & durability; Disconnect handling)

| Field | Type | Required | Nullable | Default | Since | Deprecated | Description |
|-------|------|----------|----------|---------|-------|------------|-------------|
| id | string (uuid) | Yes | No | — | v0.1 | No | The unique run identifier. Server-assigned UUIDv7. |
| player_id | string (uuid) | Yes | No | — | v0.1 | No | The player making the excursion. A player has at most one active run at a time. |
| instance_id | string (uuid) | Yes | No | — | v0.1 | No | The MazeInstance the run takes place in. |
| state | string (enum: active, extracted, died, abandoned) | Yes | No | `active` | v0.1 | No | The run lifecycle state. `extracted`: backpack banked to the vault. `died`: backpack and run levels deleted; blue gear returned with durability penalty. `abandoned`: sleeping avatar timed out with the instance — backpack deleted as on death, but no durability penalty. |
| base_run_level | integer (int32, ≥ 1) | Yes | No | — | v0.1 | No | The starting combat level, `round(1 + departure_hub_distance × 0.078)` [TUNABLE] — Center = 1, D500 = 40, D1000 = 79, D5000 = 391. |
| run_level | integer (int32, ≥ 1) | Yes | No | `base_run_level` | v0.1 | No | The current ephemeral combat level. Uncapped; grows with combat XP during the run and is discarded when the run ends, regardless of outcome. |
| run_xp | integer (int64, ≥ 0) | Yes | No | `0` | v0.1 | No | Accumulated XP toward the next run level. XP to advance from level L is `80 × L^1.6` [TUNABLE]. |
| started_at | string (date-time) | Yes | No | — | v0.1 | No | Timestamp of maze entry. Server-assigned. |
| ended_at | string (date-time) | No | Yes | null | v0.1 | No | Timestamp when the run reached a terminal state. `null` while active. |
| max_distance_reached | integer (int64, ≥ 0) | Yes | No | `departure_hub_distance` | v0.1 | No | The highest floored distance this player reached during the run. |

**Relationships**

- Belongs to one `Player` via `player_id` and one `MazeInstance` via `instance_id`.
- Has exactly one `Backpack` via `Backpack.run_id` while active.

**Notes**

- Invariant: the only state transitions are `active → extracted`, `active → died`, and `active → abandoned`; terminal states never change.
- Invariant: the backpack and all its item rows are deleted when `state` becomes `died` or `abandoned`; they are banked into the vault atomically when `state` becomes `extracted`.
- Invariant: `run_level ≥ base_run_level` at all times; `run_level` and `run_xp` never persist past `ended_at`.
- The blue-gear max-durability penalty (×0.9, rounded down) applies only on `died`, never on `abandoned` or `extracted` (CANON §B).
- Extraction happens via a portal or a `ripcord_scroll` escape channel (CANON D15).

### `Backpack`

Per-player ephemeral run inventory. Deleted on death, banked on extraction.

**Source:** GDD.md §2.2 (The Backpack); CANON.md §G (Backpack), §B (Death & durability)

| Field | Type | Required | Nullable | Default | Since | Deprecated | Description |
|-------|------|----------|----------|---------|-------|------------|-------------|
| id | string (uuid) | Yes | No | — | v0.1 | No | The unique backpack identifier. Server-assigned UUIDv7. |
| run_id | string (uuid) | Yes | No | — | v0.1 | No | The run this backpack belongs to. Exactly one backpack per run. |
| created_at | string (date-time) | Yes | No | — | v0.1 | No | Timestamp of creation at maze entry. Server-assigned. |

**Relationships**

- Belongs to one `Run` via `run_id`.
- Holds many `GearItem` (insurance `red`), `ConsumableItem`, `WardItem`, and `Material` rows via their `backpack_id`.

**Notes**

- Invariant: backpack rows and all contained item rows are deleted when `run.state = died` (or `abandoned`); contents move to the owner's `Vault` atomically when `run.state = extracted`.
- Backpack items can be freely dropped onto the overworld map for other players to pick up (GDD §6).
- Capacity is not defined in GDD or CANON; if a limit is introduced it must be server config [TUNABLE].

### `AvatarState`

The realtime overworld state of one party member's avatar, including the attackable `sleeping` state for disconnected players.

**Source:** GDD.md §5 (Disconnect & Sleep Mechanics); CANON.md §G (Sleeping, Ward, Distance), §B (Disconnect handling), §I (realtime conventions)

| Field | Type | Required | Nullable | Default | Since | Deprecated | Description |
|-------|------|----------|----------|---------|-------|------------|-------------|
| player_id | string (uuid) | Yes | No | — | v0.1 | No | The player this avatar represents. |
| instance_id | string (uuid) | Yes | No | — | v0.1 | No | The MazeInstance the avatar occupies. |
| position_x | number (double) | Yes | No | — | v0.1 | No | The avatar's X coordinate in tile units from the world origin. |
| position_y | number (double) | Yes | No | — | v0.1 | No | The avatar's Y coordinate in tile units from the world origin. |
| distance | integer (int64, ≥ 0) | Yes | No | — | v0.1 | No | The floored Euclidean distance from the world origin, recomputed from position; the value used for every threshold check. |
| state | string (enum: active, in_battle, channeling, sleeping) | Yes | No | `active` | v0.1 | No | The avatar's overworld state. `in_battle`: engaged in an ATB subscreen. `channeling`: running the 10 s interruptible extraction channel — visible and vulnerable. `sleeping`: disconnected and left on the map — not safe; a roaming monster that touches a sleeping avatar can attack it. |
| battle_id | string (uuid) | No | Yes | null | v0.1 | No | The battle the avatar is engaged in. Non-null exactly when `state = in_battle`. |
| disconnected_at | integer (int64) | No | Yes | null | v0.1 | No | Unix-millisecond timestamp of connection loss. Disconnect rules fire only after a 10-second silent reconnection grace window [TUNABLE]. `null` while connected. |
| warded_until | integer (int64) | No | Yes | null | v0.1 | No | Unix-millisecond timestamp until which the avatar is invisible to monster pathfinding, set by a deployed `WardItem`. `null` when unwarded. |

**Relationships**

- Belongs to one `MazeInstance` via `instance_id`; references at most one `Battle` via `battle_id`.

**Notes**

- Invariant: `battle_id` is non-null if and only if `state = in_battle`.
- Invariant: a `sleeping` avatar is always disconnected (`disconnected_at` non-null) and never in a battle.
- A sleeping avatar persists on the overworld until its instance closes; if the instance hits the 60-minute all-disconnected timeout, sleeping avatars auto-abandon (CANON §B, Disconnect handling).
- On disconnect during battle: standard encounters force a successful flee (structural), then the avatar becomes `sleeping`; elite and Gatekeeper encounters put the combatant into auto-defend until the battle ends or the player reconnects (CANON §B).
- Ephemeral realtime state only — synced over the realtime protocol, never via the HTTP API (CANON §S).

### `Chunk`

A server-streamed square region of the overworld, 64×64 tiles, generated deterministically from the instance seed.

**Source:** GDD.md §3; CANON.md §G (Chunk), §B (Distance → difficulty; Biome bands), §D (D15)

| Field | Type | Required | Nullable | Default | Since | Deprecated | Description |
|-------|------|----------|----------|---------|-------|------------|-------------|
| chunk_x | integer (int32) | Yes | No | — | v0.1 | No | The chunk's X index in chunk units (each chunk spans 64 tiles [TUNABLE]). |
| chunk_y | integer (int32) | Yes | No | — | v0.1 | No | The chunk's Y index in chunk units. |
| instance_id | string (uuid) | Yes | No | — | v0.1 | No | The MazeInstance whose seed generated this chunk. |
| biome | string (enum: forest, desert, ashfall, tundra, mire) | Yes | No | — | v0.1 | No | The biome theme, determined by the chunk's distance band. Curated bands: 0–100 forest, 100–300 desert, 300–500 ashfall, 500–1000 tundra, 1000–1500 mire; beyond 1500, repeating themed bands defined by content tables per 500 (content-extensible). |
| tier | integer (int32, ≥ 0) | Yes | No | — | v0.1 | No | The loot/monster tier band, `floor(distance / 100)` at the chunk's location. |
| has_gatekeeper_arena | boolean | Yes | No | `false` | v0.1 | No | Whether this chunk contains part of a Gatekeeper Boss arena (arenas sit at distances of the form `500k − 1` and span the full width of the traversable path). |
| has_extraction_portal | boolean | Yes | No | `false` | v0.1 | No | Whether this chunk contains an extraction portal. Portals appear deterministically at every hub plus roughly one per 200-distance band per instance seed [TUNABLE]. |
| tiles | array of integer (int32) (4096 items) | Yes | No | — | v0.1 | No | Row-major tile data for the 64×64 grid. Tile-type codes are content-defined. |

**Relationships**

- Belongs to one `MazeInstance` via `instance_id`.

**Notes**

- Invariant: chunk content is a pure function of (`instance_id` world seed, `chunk_x`, `chunk_y`) — re-streaming a chunk always yields identical data.
- Chunks are streamed over the realtime channel as avatars move; clients hold an interest radius of 2 chunks (CANON §B, Networking targets — non-binding).
- Procedural generation forces geographic chokepoints (e.g. bridges) to push players together (GDD §3).
