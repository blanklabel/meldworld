# Movement & World Messages

> Parent: [interfaces/realtime-protocol](../realtime-protocol.md)

Overworld state sync: movement intents and corrections, chunk streaming, entity spawn/despawn, party presence (including sleeping-avatar placement), and ward deployment. The overworld simulation runs at 20 Hz server-side with a 10 Hz snapshot broadcast and an interest radius of 2 chunks (CANON.md §B, networking targets — non-binding perf goals except where stated). A chunk is a 64×64-tile square region **[TUNABLE]** (CANON.md §G).

Shared payload objects (`Position`, `ItemStack`) are defined in the [index](../realtime-protocol.md#common-payload-objects).

### `movement.move_intent` (C2S)

A movement input sample; the client's desired movement direction, never its self-computed position of record.

**Source:** GDD.md §1 (Bevy overworld movement), §5; CANON.md §S (server validates movement), §B (overworld sim 20 Hz), D11.
**Direction:** C2S — sent while the player is in a Hub or MazeInstance and not in a battle subscreen.
**Idempotency:** Non-idempotent; superseded by the next sample. Out-of-order/duplicate protection via envelope `seq` (`sequence_error`).
**Rate limit:** At most 20 messages/s (one per server sim tick). Excess → `rate_limit_exceeded`, sample dropped.

**Payload**

| Field | Type | Required | Nullable | Default | Description |
|-------|------|----------|----------|---------|-------------|
| input_seq | integer (int64, u32 range, ≥ 1) | Yes | No | — | Client-side input counter, monotonic per session, used by the client to reconcile corrections. Distinct from the envelope `seq`. |
| move_dir | object | Yes | No | — | Desired movement direction: fields `x`, `y`, each number (double, −1.0–1.0). Magnitude is clamped to ≤ 1.0 by the server; `{0,0}` means "stop". |
| client_pos | Position | Yes | No | — | Where the client believes its avatar is, for divergence measurement. Advisory only — never trusted as authoritative. |

**Server validation**

- Not authenticated / not in an overworld context (in battle, channeling with movement locked, sleeping, no run and not in a hub) → `invalid_state`.
- Malformed vector / NaN / out-of-range → `validation_error`.
- The server integrates the intent at 20 Hz against authoritative position, collision, and the avatar's max movement speed. Illegal client positions are never adopted; they trigger corrections instead. Walking into a monster's touch range triggers a battle ([battle.md — `battle.started`](battle.md#battlestarted-s2c)); walking during an extraction channel interrupts it ([run-social.md — `run.channel_interrupted`](run-social.md#runchannel_interrupted-s2c)).

**Results in** — no per-message ack. Authoritative position flows back via `world.snapshot` (10 Hz) and, on divergence, `movement.position_correction`.

**Example**

```json
{"type": "movement.move_intent", "seq": 210, "ts": 1783728060050, "payload": {"input_seq": 188, "move_dir": {"x": 0.7071, "y": -0.7071}, "client_pos": {"x": 412.5, "y": -87.25}}}
```

---

### `movement.position_correction` (S2C)

Authoritative position override; the client must snap or smoothly reconcile to it and replay unacknowledged inputs.

**Source:** CANON.md §S (Bevy layer does prediction/interpolation; server owns movement validation), D11.
**Direction:** S2C — sent to the affected player only, whenever the client's reported position diverges from the server position by more than 0.5 tiles **[TUNABLE]**, or after any teleport-like event (battle exit placement, resume, extraction portal use).

**Payload**

| Field | Type | Required | Nullable | Default | Description |
|-------|------|----------|----------|---------|-------------|
| position | Position | Yes | No | — | The authoritative avatar position. |
| last_input_seq | integer (int64, u32 range, ≥ 0) | Yes | No | — | Highest `input_seq` integrated into this position. The client replays later inputs on top. |

**Example**

```json
{"type": "movement.position_correction", "seq": 3120, "ts": 1783728060110, "payload": {"position": {"x": 412.0, "y": -87.0}, "last_input_seq": 188}}
```

---

### `world.snapshot` (S2C)

Periodic authoritative state of all dynamic entities within the player's interest radius.

**Source:** CANON.md §B (snapshot broadcast 10 Hz, interest radius 2 chunks); GDD.md §3.
**Direction:** S2C — broadcast at 10 Hz to each connected player, scoped to that player's interest radius (all chunks within Chebyshev distance 2 of the avatar's chunk).

**Payload**

| Field | Type | Required | Nullable | Default | Description |
|-------|------|----------|----------|---------|-------------|
| server_tick | integer (int64) | Yes | No | — | Monotonic 20 Hz simulation tick number this snapshot was taken at. |
| entities | array of object | Yes | No | — | One entry per dynamic entity (players, monsters, gatekeeper bosses) currently in interest radius. Static drops/portals/wards/stalls appear via spawn/despawn and chunk data, not in every snapshot. |
| entities[].entity_id | string (uuid) | Yes | No | — | Entity id, stable across snapshots. |
| entities[].position | Position | Yes | No | — | Authoritative position at `server_tick`. |
| entities[].velocity | object | Yes | No | — | Current velocity in tiles/s: fields `x`, `y` (number, double). For client-side interpolation/extrapolation. |
| entities[].avatar_state | string (enum: `active`, `in_battle`, `channeling`, `sleeping`) | Yes | Yes | — | Player avatars only; `null` for non-players. `in_battle` avatars stand still on the overworld and are valid targets for `social.drop_item_on_player`. |

**Ordering:** snapshots are self-contained; a client may drop any snapshot older than the newest received (compare `server_tick`).

**Example**

```json
{"type": "world.snapshot", "seq": 3126, "ts": 1783728060200, "payload": {"server_tick": 884210, "entities": [{"entity_id": "0197a5aa-1111-7abc-9def-0123456789ab", "position": {"x": 412.0, "y": -87.0}, "velocity": {"x": 3.5, "y": -3.5}, "avatar_state": "active"}, {"entity_id": "0197a5aa-2222-7abc-9def-0123456789ab", "position": {"x": 420.5, "y": -90.0}, "velocity": {"x": 0.0, "y": 0.0}, "avatar_state": null}]}}
```

---

### `world.chunk_load` (S2C)

Streams one chunk's terrain and resident entities as it enters the player's interest radius.

**Source:** GDD.md §3 (server dynamically loads chunks by distance); CANON.md §G (Chunk = 64×64 tiles), §B (interest radius 2 chunks; biome bands).
**Direction:** S2C — sent when a chunk enters interest radius (avatar chunk change, run start, teleport, or resume reconciliation). Chunks within radius 2 are loaded; a chunk is unloaded when it falls outside radius 2 (no hysteresis specified in canon).

**Payload**

| Field | Type | Required | Nullable | Default | Description |
|-------|------|----------|----------|---------|-------------|
| cx | integer (int32) | Yes | No | — | Chunk column index. Chunk (0,0) contains the world origin (Center Hub). |
| cy | integer (int32) | Yes | No | — | Chunk row index. |
| biome | string (enum: `forest`, `desert`, `ashfall`, `tundra`, `mire`, plus content-table bands per 500 distance) | Yes | No | — | Biome band for this chunk per CANON.md §B (0–100 Forest, 100–300 Desert, 300–500 Ashfall, 500–1000 Tundra, 1000–1500 Mire, then content-defined). |
| tiles | string (base64) | Yes | No | — | 64×64 tile grid, row-major, one tile-kind byte per tile from the content tile table, base64-encoded (4096 bytes decoded). |
| entities | array of object | Yes | No | — | All entities resident in the chunk at load time, each in `world.entity_spawn` payload shape (see below). Includes item drops, portals, wards, stalls, monsters, sleeping avatars — so late/reconnecting clients need no separate backfill. |

**Server behavior** — chunks are generated deterministically from the MazeInstance world seed (Hub chunks from the persistent hub layout); Gatekeeper arenas occupy the full chokepoint width at `d = 500k − 1` and appear in chunk terrain. Loot/monster content scales by `tier(d) = floor(d / 100)` (CANON.md §B).

**Example**

```json
{"type": "world.chunk_load", "seq": 3300, "ts": 1783728061000, "payload": {"cx": 6, "cy": -2, "biome": "desert", "tiles": "AAECAwQF...base64...", "entities": [{"entity_id": "0197a5bb-3333-7abc-9def-0123456789ab", "entity_kind": "extraction_portal", "position": {"x": 400.0, "y": -100.0}, "detail": {}}]}}
```

---

### `world.chunk_unload` (S2C)

Instructs the client to evict a chunk that left the interest radius.

**Source:** GDD.md §3; CANON.md §B (interest radius 2 chunks).
**Direction:** S2C — sent when a loaded chunk falls outside radius 2 of the avatar's chunk.

**Payload**

| Field | Type | Required | Nullable | Default | Description |
|-------|------|----------|----------|---------|-------------|
| cx | integer (int32) | Yes | No | — | Chunk column index to evict. |
| cy | integer (int32) | Yes | No | — | Chunk row index to evict. |

**Example**

```json
{"type": "world.chunk_unload", "seq": 3301, "ts": 1783728061005, "payload": {"cx": 2, "cy": -2}}
```

---

### `world.entity_spawn` (S2C)

An entity appeared inside the player's loaded chunks.

**Source:** GDD.md §3, §5–§6; CANON.md §G (glossary entity kinds), D15 (portals), §B (wards).
**Direction:** S2C — broadcast to every player whose interest radius covers the position, when an entity is created or moves into radius: monster spawns, item drops, ward deployment, portal reveal, another player approaching, an avatar going to sleep.

**Payload**

| Field | Type | Required | Nullable | Default | Description |
|-------|------|----------|----------|---------|-------------|
| entity_id | string (uuid) | Yes | No | — | Server-generated entity id (UUIDv7). |
| entity_kind | string (enum: `player`, `monster`, `gatekeeper_boss`, `item_drop`, `extraction_portal`, `ward`, `stall`) | Yes | No | — | Entity category. Determines the shape of `detail` — see variants below. |
| position | Position | Yes | No | — | Spawn position. |
| detail | object | Yes | No | — | Kind-specific data; variants below. `{}` for `extraction_portal`. |

**`detail` variants**

#### entity_kind = `player`

| Field | Type | Required | Nullable | Default | Description |
|-------|------|----------|----------|---------|-------------|
| player_id | string (uuid) | Yes | No | — | The owning player account. |
| username | string | Yes | No | — | Player's account name (CANON.md D17). |
| character_class | string (enum: `squire`, `dragoon`, `sage`, `ranger`, `alchemist_knight`, `bard`) | Yes | No | — | Class of the avatar (CANON.md §G `CharacterClass`). |
| avatar_state | string (enum: `active`, `in_battle`, `channeling`, `sleeping`) | Yes | No | — | Current avatar state. Sleeping avatars are attackable by roaming monsters (GDD.md §5). |

#### entity_kind = `monster` / `gatekeeper_boss`

| Field | Type | Required | Nullable | Default | Description |
|-------|------|----------|----------|---------|-------------|
| monster_kind | string | Yes | No | — | Content-table monster identifier. |
| level | integer (int32, ≥ 1) | Yes | No | — | `mlevel(d) = max(1, round(d / 12.5))` at spawn distance (CANON.md §B). |
| encounter_class | string (enum: `standard`, `elite`, `gatekeeper`) | Yes | No | — | Drives disconnect rules and flee availability in battle (CANON.md §B). Always `gatekeeper` for `gatekeeper_boss`. |

#### entity_kind = `item_drop`

| Field | Type | Required | Nullable | Default | Description |
|-------|------|----------|----------|---------|-------------|
| items | array of ItemStack (min 1 item) | Yes | No | — | The dropped stack(s), pickupable via `social.pickup_item`. |
| dropped_by | string (uuid) | Yes | Yes | — | Player who dropped it; `null` for world-generated drops. |

#### entity_kind = `ward`

| Field | Type | Required | Nullable | Default | Description |
|-------|------|----------|----------|---------|-------------|
| ward_kind | string (enum: `warding_tent`, `sanctuary_campfire`) | Yes | No | — | Ward type (CANON.md §G `WardItem`). |
| expires_at | integer (int64, u64) | Yes | No | — | Unix millis when the ward effect ends. |
| deployed_by | string (uuid) | Yes | No | — | Player who deployed it. |

#### entity_kind = `stall`

| Field | Type | Required | Nullable | Default | Description |
|-------|------|----------|----------|---------|-------------|
| stall_id | string (uuid) | Yes | No | — | Persistent stall id — used with the HTTP stall-shop endpoints (browsing/purchase is HTTP, out of scope). |
| owner_player_id | string (uuid) | Yes | No | — | Stall owner; the stall persists while the owner is offline (GDD.md §7). |
| stall_name | string | Yes | No | — | Shop display name. |

**Example**

```json
{"type": "world.entity_spawn", "seq": 3410, "ts": 1783728065000, "payload": {"entity_id": "0197a5cc-4444-7abc-9def-0123456789ab", "entity_kind": "item_drop", "position": {"x": 413.0, "y": -86.0}, "detail": {"items": [{"item_id": "0197a5cc-5555-7abc-9def-0123456789ab", "item_kind": "health_potion", "quantity": 2, "insurance": null}], "dropped_by": "0197a2f0-11aa-7bbb-8ccc-0d1e2f3a4b5c"}}}
```

---

### `world.entity_despawn` (S2C)

An entity left the player's view: picked up, expired, killed, moved out of radius, or its owner woke/extracted.

**Source:** GDD.md §3, §6; CANON.md §B.
**Direction:** S2C — broadcast to every player whose interest radius covered the entity.

**Payload**

| Field | Type | Required | Nullable | Default | Description |
|-------|------|----------|----------|---------|-------------|
| entity_id | string (uuid) | Yes | No | — | The despawned entity. |
| reason | string (enum: `picked_up`, `expired`, `defeated`, `out_of_range`, `extracted`, `woke`, `instance_closed`) | Yes | No | — | Why it despawned. `out_of_range` means the entity still exists server-side but left this client's interest radius. |

**Example**

```json
{"type": "world.entity_despawn", "seq": 3502, "ts": 1783728070000, "payload": {"entity_id": "0197a5cc-4444-7abc-9def-0123456789ab", "reason": "picked_up"}}
```

---

### `world.presence_update` (S2C)

Connection and avatar-state change for a party member — including the sleeping-avatar placement that follows a disconnect.

**Source:** GDD.md §5 (the "Sleeping" state); CANON.md §B (disconnect handling; sleeping avatar persists until instance closes).
**Direction:** S2C — broadcast to all members of the player's Party/MazeInstance (and, for `avatar_state` changes, to anyone with the avatar in interest radius) whenever any field below changes.

**Payload**

| Field | Type | Required | Nullable | Default | Description |
|-------|------|----------|----------|---------|-------------|
| player_id | string (uuid) | Yes | No | — | The party member whose state changed. |
| connected | boolean | Yes | No | — | Whether the member has a live realtime session. Flips to `false` only after the 10 s grace window expires — mid-grace drops are invisible. |
| avatar_state | string (enum: `active`, `in_battle`, `channeling`, `sleeping`) | Yes | Yes | — | Current avatar state; `null` when the member has no avatar in the world (e.g. run ended). |
| position | Position | Yes | Yes | — | Avatar position at the state change; for `sleeping`, the exact overworld placement of the sleeping body (its last authoritative position, or its pre-battle overworld position after a forced flee / boss-battle end while disconnected). `null` when `avatar_state` is `null`. |
| warded_until | integer (int64, u64) | Yes | Yes | — | When a ward currently covers this avatar: Unix millis until which monster pathfinding ignores it. `null` when unwarded. Sleeping + `null` = attackable. |

**Sleeping-avatar rules** (server behavior, observable via this message and `world.snapshot`):

- Grace expiry out of battle → `connected: false`, `avatar_state: "sleeping"`, placed at the last authoritative position.
- Grace expiry in a standard battle → forced flee resolves first ([battle.md](battle.md#disconnect-handling-in-battle)), then the avatar sleeps at its pre-battle overworld position.
- In elite/Gatekeeper battles the avatar auto-defends instead; it sleeps only when that battle ends without reconnection.
- A roaming monster touching a sleeping avatar starts a battle against it (`battle.started` with the sleeper as sole party member, auto-defending throughout).
- Reconnection wakes the avatar: `connected: true`, `avatar_state: "active"`.
- Sleeping avatars persist until the instance closes; 60 min with **all** members disconnected closes the instance and auto-abandons them (CANON.md §B; see [run-social.md](run-social.md#runinstance_closed-s2c)).

**Example**

```json
{"type": "world.presence_update", "seq": 3600, "ts": 1783728080000, "payload": {"player_id": "0197a2f0-22bb-7ccc-9ddd-0e1f2a3b4c5d", "connected": false, "avatar_state": "sleeping", "position": {"x": 415.0, "y": -84.5}, "warded_until": null}}
```

---

### `world.deploy_ward` (C2S)

Consumes a ward item from the backpack to protect a sleeping ally on the map.

**Source:** GDD.md §5 (protective items over sleeping allies); CANON.md §G (`WardItem`), §B (ward durations).
**Direction:** C2S — sent by an active (not battling, not channeling) player inside a MazeInstance.
**Idempotency:** Non-idempotent (consumes an item); duplicate-retry protection via envelope `seq`.

**Payload**

| Field | Type | Required | Nullable | Default | Description |
|-------|------|----------|----------|---------|-------------|
| item_id | string (uuid) | Yes | No | — | Backpack item instance to consume. Must have `item_kind` `warding_tent` or `sanctuary_campfire`. |
| position | Position | No | Yes | deployer's position | Deployment point. When omitted or `null`, the ward is placed at the deployer's authoritative position. |

**Server validation**

- Sender not in a run / in battle / channeling / sleeping → `invalid_state`.
- `item_id` not in sender's backpack → `not_found`; in the backpack but not a ward kind → `validation_error`.
- `position` farther than the 2-tile **[TUNABLE]** interaction range from the deployer → `out_of_range`.
- Placement is allowed pre-emptively (no sleeping avatar required at the spot); the ward protects any sleeping avatar within its 2-tile **[TUNABLE]** effect radius while active.

**Results in** — on success (atomically): the item is consumed (`run.backpack_update` with a negative delta to the deployer), `world.ward_deployed` broadcast to the instance, `world.entity_spawn` (`entity_kind: "ward"`) to players in radius, and `world.presence_update` with `warded_until` for any sleeping avatar now covered.

**Example**

```json
{"type": "world.deploy_ward", "seq": 240, "ts": 1783728090000, "payload": {"item_id": "0197a5dd-6666-7abc-9def-0123456789ab", "position": {"x": 415.0, "y": -84.5}}}
```

---

### `world.ward_deployed` (S2C)

A ward is now active in the instance.

**Source:** GDD.md §5; CANON.md §B (ward items: `warding_tent` 30 min invisibility to monster pathfinding; `sanctuary_campfire` 10 min invisibility + slow HP regen aura).
**Direction:** S2C — broadcast to all MazeInstance members (party members outside interest radius still learn their sleeping ally is safe). Carries `client_seq` on the deployer's copy.

**Payload**

| Field | Type | Required | Nullable | Default | Description |
|-------|------|----------|----------|---------|-------------|
| client_seq | integer (int64, u32 range) | Yes | Yes | — | Echo of the `world.deploy_ward` seq on the deployer's copy; `null` on other members' copies. |
| entity_id | string (uuid) | Yes | No | — | The ward entity (matches the `world.entity_spawn`). |
| ward_kind | string (enum: `warding_tent`, `sanctuary_campfire`) | Yes | No | — | Ward type. `warding_tent`: 30 min invisibility to monster pathfinding. `sanctuary_campfire`: 10 min invisibility plus a slow HP-regen aura on covered sleeping avatars. Durations **[TUNABLE]**. |
| position | Position | Yes | No | — | Where the ward stands. |
| expires_at | integer (int64, u64) | Yes | No | — | Unix millis when the ward expires; a `world.entity_despawn` (`expired`) follows at that time, and covered avatars' `warded_until` reverts to `null` via `world.presence_update`. |
| deployed_by | string (uuid) | Yes | No | — | Deploying player. |

**Example**

```json
{"type": "world.ward_deployed", "seq": 3700, "ts": 1783728090040, "payload": {"client_seq": 240, "entity_id": "0197a5ee-7777-7abc-9def-0123456789ab", "ward_kind": "warding_tent", "position": {"x": 415.0, "y": -84.5}, "expires_at": 1783729890040, "deployed_by": "0197a2f0-11aa-7bbb-8ccc-0d1e2f3a4b5c"}}
```
