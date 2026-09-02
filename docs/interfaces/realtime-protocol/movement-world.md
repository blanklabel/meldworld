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
| entities[].avatar_state | string (tag) | Yes | Yes | — | What this entity IS, as a colon-delimited tag. For a **player avatar**: `active`, `in_battle`, `channeling` or `sleeping` (`in_battle` avatars stand still on the overworld and are valid targets for `social.drop_item_on_player`). For everything else the first part is the kind — `mob:…`, `portal`, `stair`, `trap:<kind>`, `chest:<tier>:<opened>`, `resource:<kind>`, `loot:<kind>`, `obstacle:<kind>:<radius>`, `station:<kind>:<jobs_left>`, `structure:<function>:<hp_pct>:<building>`, `entrance:<dungeon>:<bodies_required>`. See the mob tag below. |
| ↳ mob tag | `mob:<kind>:<faction>[:token…]` | — | — | — | `kind` is the creature content id and `faction` its lineage. The trailing tokens are a **SET** — read every one, never just the first — and each is either a bare flag or a `key:value` pair: `boss:<key>` (FS-4: which of the ten named bosses this is — a boss overlays a host creature, so `kind` stays the wildlife it rode in on), `held` (pinned by a Psyker, CL-2), `clash` (trading blows with another creature right now, CR-2), and the **per-viewer** `quarry` (the quarry of a hunt this recipient is working, AD-4 — the same creature is not a quarry to the teammate beside them). |

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
| character_class | string (enum: `explorer`, `dragoon`, `sage`, `ranger`, `alchemist_knight`, `bard`) | Yes | No | — | Class of the avatar (CANON.md §G `CharacterClass`). |
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

---

### `world.shift_warning` (S2C)

**Source:** CANON.md D20 / §W2 (the Shift); tunables under `[shift]`.
**Direction:** S2C — broadcast to **every** player in the world, not only to those
standing in the doomed ring. A Shift is weather: knowing that the desert three rings
out is about to become tundra is how a party decides where to walk next.

The scheduler is a pure function of `(world_seed, shift_generation)` driven by the
server tick counter and never by wall-clock (§W2, structural), so a client that
reconnects mid-window receives the next warning on schedule with no catch-up state.

**Payload**

| Field | Type | Required | Nullable | Default | Description |
|-------|------|----------|----------|---------|-------------|
| generation | integer (int64, u64) | Yes | No | — | Which Shift this is. Monotonic per world; pairs the warning with its `world.shift`. |
| inner_radius | number (f64) | Yes | No | — | Inner edge of the doomed region's radius band, in world units from the hub. |
| outer_radius | number (f64) | Yes | No | — | Outer edge of the band. |
| biome | string | Yes | No | — | What the ring is about to become (`forest`/`desert`/`ashfall`/`tundra`/`mire`/`field`). Never the biome it already is, and never one the `[biome_gate]` holds deeper than this radius. |
| lands_in_ms | integer (int64, u64) | Yes | No | — | Milliseconds until it lands. `[shift] warning_ticks` is held by test to be long enough to actually walk out of the widest region the size table can roll — a Shift you cannot escape is a dice roll, not a hazard. |
| caught | boolean | No | No | `false` | Whether *this* player is inside the region right now. The server owns the fact; the client owns how loud to be about it. |
| arc_center | number (f64) | No | No | `0` | Centre of the doomed **bearing wedge**, in radians. A region is a PATCH of cells, not a whole annulus — without this the tell lights a full ring around the part that actually goes, sending everyone at that depth running from weather that was never coming. |
| arc_half | number (f64) | No | No | `0` | Half-width of that wedge, in radians. `0` means "no wedge given" and the whole ring is treated as doomed, which is what an older server's tell looks like. |

**Example**

```json
{"type": "world.shift_warning", "seq": 4100, "ts": 1783728100000, "payload": {"generation": 7, "inner_radius": 240.0, "outer_radius": 318.0, "biome": "ashfall", "lands_in_ms": 10000, "caught": true}}
```

---

### `world.shift` (S2C)

**Source:** CANON.md D20 / §W2.
**Direction:** S2C — broadcast to every player in the world.

Sent the tick the region swaps.

> ⚠️ **This section used to say the retiled `world.terrain_section` messages "are what
> actually repaints the ground", because the client keyed its biome ground off per-section
> radius rings.** That was true when it was written and false from `WG-7` on, which made a
> cell's biome **analytic** — derived from the region grid, the world seed and the
> `[biome_gate]`, so a world can stream outward with no lookup table. From then until
> `WG-11` a Shift swapped the region's biome, re-scattered its props, dealt Force damage and
> announced *"Mire became Desert"* — and the ground stayed mire for the life of the world,
> because nothing on the wire could move a derivation that only reads the seed.

The retiled `world.terrain_section` messages carry **geometry** (peaks and the rest of a
section's landforms). The biome comes from **`repaints`** below: the cells that changed and
what they became, which the client folds into its own copy of the decomposition so its ground
shader, grass, minimap and HUD label all agree with the server. This message is also the words
and the damage.

**The region's props are re-scattered and its mountains re-cut**, not reskinned: the
incoming biome strews its own count at its own density in its own places, and raises
or flattens peaks at its own weighting. The retiled sections' `world.terrain_section`
messages carry the new peaks, and a client keys peaks by section — re-sending a
section *replaces* its mountains rather than adding to them.

Placement rejects the clear-path tube exactly as world generation does, so the route
out stays feasible by construction, and **the clear path itself is never changed**.
But the new land can land on a player standing off-trail, and the server then walks
them to the region's entry and sends a `movement.position_correction`. Bounty marks,
chests and player-raised stations survive; the region's other creatures and its
resource nodes do not, and what grows back belongs to the new biome.

**Payload**

| Field | Type | Required | Nullable | Default | Description |
|-------|------|----------|----------|---------|-------------|
| generation | integer (int64, u64) | Yes | No | — | Matches the `world.shift_warning` that announced it. |
| inner_radius | number (f64) | Yes | No | — | Inner edge of the swapped region's radius band. |
| outer_radius | number (f64) | Yes | No | — | Outer edge of that band. |
| arc_center | number (f64) | No | No | `0` | Centre of the swapped **bearing wedge**, in radians — the region is a patch, so the band alone describes a ring around it. |
| arc_half | number (f64) | No | No | `0` | Half-width of that wedge, in radians. |
| repaints | array of object | No | No | `[]` | **The cells that changed, and what they became** — `{cell, biome}`, where `cell` is a packed `regions::Cell::key` and `biome` is an index into `regions::BIOMES`. ⚠️ **This is the only thing that moves the ground.** A cell's biome is derived identically on both sides of the wire, so a client that ignores this paints the world exactly as the seed left it while the server spawns the new biome's creatures on top of it. Fold each entry into the decomposition received on `run.started`; the resolution order there is capstone, then repaint, then the seed's own roll. |
| biome | string | Yes | No | — | What the region is now. |
| from_biome | string | No | No | `""` | What it stopped being, for the line the client prints. |
| wiped | array of string (id) | No | No | `[]` | Entity ids the Shift removed, so a client drops them on the same frame the ground changes rather than one snapshot later. |
| damage | array of integer (int32) | No | No | `[]` | HP each of **this** player's heroes lost to the Force blast, parallel to the party; empty for anyone who was outside the ring. The magnitude is a fraction of each hero's *own* max HP scaled by the region's size (`[shift] damage_fraction_min`/`max`) — a flat blast would be a death sentence at level 1 and a rounding error at 100. A party wiped by one ends its run `died`, exactly as a sprung trap does. |

**Example**

```json
{"type": "world.shift", "seq": 4200, "ts": 1783728110000, "payload": {"generation": 7, "inner_radius": 240.0, "outer_radius": 318.0, "biome": "ashfall", "from_biome": "forest", "wiped": ["mob-118", "mob-119"], "damage": [96, 96, 51, 51]}}
```

---

### `run.build_structure` / `run.repair_structure` / `run.demolish_structure` (C2S)

**Source:** CANON.md D21 / §W3; tunables under `[building]`; functions in
[`meld_proto::structures`](../../../shared/meld-proto/src/structures.rs).
**Direction:** C2S — intents. All placement, cost and permission checks are server-side.

**One intent per verb, not per function.** There is one `Structure` primitive; the
`function` key varies its role. A new buildable is a row in the registry, never a new
message.

| Message | Payload | Notes |
|---|---|---|
| `run.build_structure` | `function` (string) | Raises it where the avatar stands. Debits the function's ore cost from the run backpack, deepest stack first. Refused — **before** the stock is spent — if the spot is on the clear-path tube, too close to another structure or bench, inside an obstacle, past the per-player cap, or (for a **blocking** function only) within `no_build_near_player` of another player: you may not pen somebody in one block at a time. An `anchor` is exempt, since it does not block. It goes up at `build_start_fraction` of its HP and ramps to full over the function's build time. |
| `run.repair_structure` | `entity_id` (id) | Spends one unit of ore for `repair_hp_per_ore`. **Anyone** in reach may repair — hauling ore out to a teammate's anchor is the point. Refused if it is already sound. |
| `run.demolish_structure` | `entity_id` (id) | **Owner only.** Returns `demolish_refund_fraction` of what it cost, in the material it was built from. Never a full refund, or moving one is free. |

Each replies with a `run.backpack_update` carrying the material movement; the structure
itself appears in `world.snapshot` as `structure:<function>:<hp_pct>:<building>`.

---

### `world.shift_held` (S2C)

**Source:** CANON.md §W3 (anchors); `BD-3`.
**Direction:** S2C — broadcast to every player in the world.

The Shift arrived at its scheduled tick and **an anchor stopped it**. The region did not
retile, took no Force damage and lost nothing; the land took it out of whatever was
holding it instead.

An anchor does **not** alter the natural schedule — that stays a pure function of the seed
(§W2/§W5) — it alters the outcome, and the suppression is the event. A held Shift is
therefore absent from the replay log: nothing changed about the world except anchor HP,
which rides the persistence delta already.

**Payload**

| Field | Type | Required | Nullable | Default | Description |
|-------|------|----------|----------|---------|-------------|
| generation | integer (int64, u64) | Yes | No | — | Which Shift was held. Matches the `world.shift_warning` that announced it. |
| inner_radius | number (f64) | Yes | No | — | Inner edge of the ring that would have gone. |
| outer_radius | number (f64) | Yes | No | — | Outer edge. |
| anchors | array of object | Yes | No | — | Every anchor that held, and what holding cost it: `entity_id`, `damage`, `hp`, `max_hp`, `destroyed`. `damage` is `[building] shift_hold_damage_fraction` of that anchor's own max HP — an anchor is permanence you keep *paying for*, and one nobody maintains falls on its own and hands the ground back to the Shift. |

**Example**

```json
{"type": "world.shift_held", "seq": 4300, "ts": 1783728120000, "payload": {"generation": 9, "inner_radius": 240.0, "outer_radius": 318.0, "anchors": [{"entity_id": "struct-anchor-0", "damage": 225, "hp": 450, "max_hp": 900, "destroyed": false}]}}
```

---

### `movement.position_correction` and being stuck

A player who ends up standing inside something impassable is **walked to the nearest
point on the guaranteed clear path** and sent a `movement.position_correction`. The
client's local avatar chases the snapshot exponentially for responsiveness, so an
uncorrected teleport renders as a second-long slide across the map with the camera
following; the correction makes it a snap.

Two things trigger it, and they share one predicate:

- **A Shift** re-scattered props or raised ground where a player was standing — they are
  walked to the *region's entry* (see `world.shift`), because the Shift knows which region
  moved.
- **The general sweep** (`[building] stuck_check_ticks`) catches every other way a player
  can end up inside geometry, with no event behind it, and walks them to the nearest open
  ground. Active avatars only — nobody is pulled out of a battle or a channel by a safety
  net.
