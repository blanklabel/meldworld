# Run & Social Messages

> Parent: [interfaces/realtime-protocol](../realtime-protocol.md)

Run lifecycle (enter maze, extraction channels, death, abandon, instance close, backpack sync) and asynchronous social interaction (ground drops, pickups, dropping consumables onto battling players). A `Run` ends in exactly one of `extracted`, `died`, or `abandoned` per member (CANON.md §G). All persistent consequences — vault banking on extraction, durability loss on death — are server-side mutations observed via the HTTP API, never realtime mutations (CANON.md §S boundary rule).

Shared payload objects (`Position`, `ItemStack`) are defined in the [index](../realtime-protocol.md#common-payload-objects).

### `run.enter_maze` (C2S)

Starts the party's run: the server creates the `MazeInstance` (with its own world seed) and the `Run`, resets combat state, and drops the party into the maze.

**Source:** GDD.md §2.2 (the Reset), §4 (base level scaling); CANON.md D13 (instance created at maze-entry time, not joinable afterward), §B (hubs & run levels).
**Direction:** C2S — sent by the **party leader** (solo players are their own leader) while the whole party stands in the same Hub. Party formation and matchmaking happen beforehand via Hub UIs over HTTP (out of scope).
**Idempotency:** Non-idempotent. A second `run.enter_maze` while a run is active → `invalid_state`.

**Payload** — empty object `{}` (the server already knows the sender's party and current hub; nothing client-supplied is trusted).

**Server validation**

- Sender not in a hub, or not the party leader → `invalid_state` / `forbidden`.
- Any party member already in an active run, disconnected, or in a different hub → `invalid_state`.

**Results in** — `run.started` to every party member; then initial `world.chunk_load` for all chunks in each member's interest radius, `world.entity_spawn`s, and a first `world.snapshot`.

**Example**

```json
{"type": "run.enter_maze", "seq": 12, "ts": 1783728200000, "payload": {}}
```

---

### `run.started` (S2C)

Authoritative run and instance state at maze entry.

**Source:** GDD.md §2.2, §4; CANON.md §B (`base_run_level(hub) = 1 + hub.distance × 0.078`, rounded to nearest int → Center = 1, D500 = 40), §G (`Run`, `MazeInstance`, `Backpack`).
**Direction:** S2C — sent to every party member in response to a valid `run.enter_maze` (carries `client_seq` on the leader's copy only).

**Payload**

| Field | Type | Required | Nullable | Default | Description |
|-------|------|----------|----------|---------|-------------|
| client_seq | integer (int64, u32 range) | Yes | Yes | — | Echo of the leader's `run.enter_maze` seq; `null` on other members' copies. |
| run_id | string (uuid) | Yes | No | — | The new Run. |
| instance_id | string (uuid) | Yes | No | — | The new MazeInstance. |
| departure_hub_distance | integer (int32, one of 0, 500, 1000, …, 5000) | Yes | No | — | The hub the run departs from. |
| base_run_level | integer (int32, ≥ 1) | Yes | No | — | Starting run level for every member: `round(1 + hub.distance × 0.078)`. |
| members | array of object (1–4 items) | Yes | No | — | The party. Fields: `player_id` string (uuid); `username` string; `character_class` string (enum: `hunter`, `dragoon`, `sage`, `ranger`, `alchemist_knight`, `bard`); `spawn_position` Position. |
| backpack | array of ItemStack | Yes | No | — | The recipient's starting backpack contents (empty array on a fresh run). Authoritative baseline for all later `run.backpack_update` deltas. |

**Example**

```json
{"type": "run.started", "seq": 5001, "ts": 1783728200080, "payload": {"client_seq": 12, "run_id": "0197a610-0001-7abc-9def-0123456789ab", "instance_id": "0197a610-0002-7abc-9def-0123456789ab", "departure_hub_distance": 500, "base_run_level": 40, "members": [{"player_id": "0197a2f0-11aa-7bbb-8ccc-0d1e2f3a4b5c", "username": "Marlowe", "character_class": "dragoon", "spawn_position": {"x": 498.0, "y": 12.0}}], "backpack": []}}
```

---

### `run.begin_extraction` (C2S)

Starts an extraction channel — at an extraction portal, or from anywhere with an escape item.

**Source:** GDD.md §2.2 (Extract: portal or escape item); CANON.md D15 (portals at every Hub plus ~1 per 200-distance band per instance seed; escape items extract from anywhere with a 10 s interruptible channel).
**Direction:** C2S — sent by a player in an active run, not in battle, not already channeling.
**Idempotency:** Non-idempotent; `invalid_state` if a channel is already active.

**Payload**

| Field | Type | Required | Nullable | Default | Description |
|-------|------|----------|----------|---------|-------------|
| method | string (enum: `portal`, `escape_item`) | Yes | No | — | Extraction mechanism. Determines which of the two following fields is required. |
| portal_entity_id | string (uuid) | No | Yes | null | The extraction-portal entity to use. Required when `method` is `portal`; must be `null` otherwise. |
| item_id | string (uuid) | No | Yes | null | Backpack escape-item instance to consume (e.g. `item_kind: "ripcord_scroll"`). Required when `method` is `escape_item`; must be `null` otherwise. |

**Server validation**

- Not in an active run, in battle, sleeping, or already channeling → `invalid_state`.
- `method`/field mismatch → `validation_error`.
- `portal_entity_id` unknown in this instance → `not_found`; farther than the 2-tile **[TUNABLE]** interaction range → `out_of_range`.
- `item_id` not in the sender's backpack → `not_found`; not an escape item → `validation_error`.

**Results in** — `run.channel_started` broadcast to the instance. The channel runs 10 s **[TUNABLE]** for both methods (canon fixes 10 s for escape items, D15; portals mirror it). Escape items are consumed when the channel **starts** (`run.backpack_update`) — an interrupted channel does not refund the item **[TUNABLE]**. While channeling, `avatar_state` is `channeling`; movement intents interrupt the channel. On uninterrupted completion the member's run ends: `run.member_result` with `result: "extracted"`.

**Example**

```json
{"type": "run.begin_extraction", "seq": 480, "ts": 1783729000000, "payload": {"method": "escape_item", "portal_entity_id": null, "item_id": "0197a611-4444-7abc-9def-0123456789ab"}}
```

---

### `run.cancel_extraction` (C2S)

Voluntarily cancels the sender's own active extraction channel.

**Source:** CANON.md D15 (channel is interruptible); voluntary cancel is a canon gap resolved by this spec.
**Direction:** C2S — legal only while the sender is channeling.

**Payload** — empty object `{}`.

**Server validation** — no active channel → `invalid_state`.

**Results in** — `run.channel_interrupted` (`reason: "cancelled"`) broadcast to the instance; `avatar_state` returns to `active`. The consumed escape item is not refunded.

**Example**

```json
{"type": "run.cancel_extraction", "seq": 484, "ts": 1783729004000, "payload": {}}
```

---

### `run.channel_started` (S2C)

An extraction channel began; the channeling avatar is visible and vulnerable for the duration.

**Source:** GDD.md §2.2; CANON.md D15 (10 s interruptible channel).
**Direction:** S2C — broadcast to all instance members (carries `client_seq` on the channeler's copy). A `world.presence_update` (`avatar_state: "channeling"`) accompanies it.

**Payload**

| Field | Type | Required | Nullable | Default | Description |
|-------|------|----------|----------|---------|-------------|
| client_seq | integer (int64, u32 range) | Yes | Yes | — | Echo of `run.begin_extraction` seq on the channeler's copy; `null` on others. |
| player_id | string (uuid) | Yes | No | — | Who is channeling. |
| method | string (enum: `portal`, `escape_item`) | Yes | No | — | Extraction mechanism in use. |
| completes_at | integer (int64, u64) | Yes | No | — | Unix millis when the channel completes if uninterrupted (start + 10 000 ms **[TUNABLE]**). |

**Example**

```json
{"type": "run.channel_started", "seq": 5210, "ts": 1783729000040, "payload": {"client_seq": 480, "player_id": "0197a2f0-11aa-7bbb-8ccc-0d1e2f3a4b5c", "method": "escape_item", "completes_at": 1783729010040}}
```

---

### `run.channel_interrupted` (S2C)

An extraction channel broke before completing.

**Source:** CANON.md D15 (interruptible); GDD.md §5 (enemies keep acting).
**Direction:** S2C — broadcast to all instance members. Interruption is server-decided; the channel breaks the moment any interrupting event lands.

**Payload**

| Field | Type | Required | Nullable | Default | Description |
|-------|------|----------|----------|---------|-------------|
| player_id | string (uuid) | Yes | No | — | Whose channel broke. |
| reason | string (enum: `damage_taken`, `battle_started`, `moved`, `cancelled`, `disconnected`) | Yes | No | — | What broke it: any damage; being pulled into a battle (touch); any accepted movement intent; explicit `run.cancel_extraction`; or the channeler's grace window expiring. |

**Example**

```json
{"type": "run.channel_interrupted", "seq": 5214, "ts": 1783729006000, "payload": {"player_id": "0197a2f0-11aa-7bbb-8ccc-0d1e2f3a4b5c", "reason": "battle_started"}}
```

---

### `run.abandon` (C2S)

Explicitly abandons the sender's run (quit to Hub without extracting).

**Source:** CANON.md §G (`Run` ends in `extracted`, `died`, or `abandoned`), §B (auto-abandon semantics: counts as death for backpack, no durability loss — explicit abandon mirrors this, a canon gap resolved by this spec).
**Direction:** C2S — legal in an active run, out of battle. Per-player: the rest of the party keeps playing.
**Idempotency:** Non-idempotent; `invalid_state` once the run has ended.

**Payload** — empty object `{}`.

**Server validation** — not in an active run → `invalid_state`; in battle → `invalid_state` ("Resolve or flee the battle first.").

**Results in** — the sender's backpack and run level are deleted (as death), blue-chest gear returns **without** durability loss; `run.member_result` (`result: "abandoned"`) broadcast; the avatar despawns and the player returns to the departure hub.

**Notes**

- The despawn is broadcast as `world.entity_despawn` with `reason: "extracted"` — visually identical to an extraction departure; the outcome difference is carried by `run.member_result`.

**Example**

```json
{"type": "run.abandon", "seq": 512, "ts": 1783729100000, "payload": {}}
```

---

### `run.member_result` (S2C)

A party member's run reached its terminal state: `extracted`, `died`, or `abandoned`.

**Source:** GDD.md §2.2 (the Choice: extract or die); CANON.md §G (`Run`, `Backpack`), §B (death & durability), D15.
**Direction:** S2C — broadcast to all instance members. The affected member's own copy additionally carries the private `banked` / `lost` summary; other members receive `null` there.

**Payload**

| Field | Type | Required | Nullable | Default | Description |
|-------|------|----------|----------|---------|-------------|
| run_id | string (uuid) | Yes | No | — | The run. |
| player_id | string (uuid) | Yes | No | — | Whose run ended. |
| result | string (enum: `extracted`, `died`, `abandoned`) | Yes | No | — | Terminal outcome. |
| max_distance_reached | integer (int32, ≥ 0) | Yes | No | — | The member's deepest `floor` distance this run — the Vanguard Board input (CANON.md D3; board itself is HTTP-owned). |
| banked | array of ItemStack | Yes | Yes | — | `extracted` only, own copy only: the backpack contents banked into the Vault (red-chest gear becomes owned Vault gear, still `red` tier). `null` otherwise. **Handoff:** the banking itself is a server-side persistent mutation; the Vault's new state is read via the HTTP API. |
| lost | array of ItemStack | Yes | Yes | — | `died`/`abandoned`, own copy only: the deleted backpack contents. `null` otherwise. |
| durability_loss_applied | boolean | Yes | No | — | Whether blue-chest gear lost max durability: `true` on `died` (−10% of current max, round down, floor 0 — CANON.md D6/§B, applied server-side, visible via HTTP), `false` on `extracted` and `abandoned`. |

**Ordering** — for a death this message follows the terminal `battle.ended` (`outcome: "defeat"`); for extraction it follows the channel completing (no separate "channel completed" message — this is it).

**Example — extraction (own copy)**

```json
{"type": "run.member_result", "seq": 5300, "ts": 1783729010060, "payload": {"run_id": "0197a610-0001-7abc-9def-0123456789ab", "player_id": "0197a2f0-11aa-7bbb-8ccc-0d1e2f3a4b5c", "result": "extracted", "max_distance_reached": 742, "banked": [{"item_id": "0197a602-8888-7abc-9def-0123456789ab", "item_kind": "iron_ore", "quantity": 3, "insurance": null}], "lost": null, "durability_loss_applied": false}}
```

---

### `run.instance_closed` (S2C)

The MazeInstance shut down; all remaining ephemeral state in it is gone.

**Source:** CANON.md §B (instance closes when all members extracted/died/abandoned, or after 60 min with all members disconnected → sleeping avatars auto-abandon: counts as death for the backpack, **no** durability loss).
**Direction:** S2C — sent to any instance member still connected (typically none for the timeout case; the message matters for spectating party members whose own runs already ended but whose session persists).

**Payload**

| Field | Type | Required | Nullable | Default | Description |
|-------|------|----------|----------|---------|-------------|
| instance_id | string (uuid) | Yes | No | — | The closed instance. |
| reason | string (enum: `all_members_resolved`, `idle_timeout`) | Yes | No | — | `all_members_resolved`: every member reached a terminal result. `idle_timeout`: 60 min **[TUNABLE]** with all members disconnected; every still-sleeping avatar was auto-abandoned (a `run.member_result` with `result: "abandoned"` per member precedes this message). |

**Example**

```json
{"type": "run.instance_closed", "seq": 5400, "ts": 1783732700000, "payload": {"instance_id": "0197a610-0002-7abc-9def-0123456789ab", "reason": "all_members_resolved"}}
```

---

### `run.backpack_update` (S2C)

Authoritative delta to the recipient's own backpack — the single source of truth for ephemeral inventory.

**Source:** GDD.md §2.2 (the Backpack); CANON.md §G (`Backpack` — per-player ephemeral run inventory), §S (ephemeral state flows over realtime).
**Direction:** S2C — sent to the owning player only, whenever backpack contents change: battle loot, ground pickup, ground drop, item consumed (battle item, ward deployment, escape item, drop-on-player). Clients apply deltas in envelope `seq` order on top of the `run.started` baseline; equal-and-opposite client prediction must be reconciled to this message.

**Payload**

| Field | Type | Required | Nullable | Default | Description |
|-------|------|----------|----------|---------|-------------|
| changes | array of object (min 1 item) | Yes | No | — | Item-level deltas. |
| changes[].item | ItemStack | Yes | No | — | The affected item; `quantity` is the magnitude of the change. |
| changes[].delta | string (enum: `added`, `removed`) | Yes | No | — | Direction of the change. |
| changes[].cause | string (enum: `battle_loot`, `picked_up`, `dropped`, `consumed`, `banked`, `deleted`) | Yes | No | — | Why. `banked` (extraction) and `deleted` (death/abandon) always empty the backpack and coincide with the `run.member_result`. |

**Example**

```json
{"type": "run.backpack_update", "seq": 5120, "ts": 1783728150001, "payload": {"changes": [{"item": {"item_id": "0197a602-8888-7abc-9def-0123456789ab", "item_kind": "iron_ore", "quantity": 3, "insurance": null}, "delta": "added", "cause": "battle_loot"}]}}
```

---

### `social.drop_item` (C2S)

Drops backpack items onto the overworld for anyone to pick up — cooperation, gifting, or paying bodyguards.

**Source:** GDD.md §6 (backpack dropping); CANON.md §S (drops are ephemeral realtime state).
**Direction:** C2S — sent by a player in an active run (hubs are safe zones; ground-dropping in hubs is also allowed, and such drops despawn when the dropper's session ends **[TUNABLE]**).
**Idempotency:** Non-idempotent; `sequence_error` protects against duplicate retries.

**Payload**

| Field | Type | Required | Nullable | Default | Description |
|-------|------|----------|----------|---------|-------------|
| item_id | string (uuid) | Yes | No | — | Backpack item instance to drop. |
| quantity | integer (int32, ≥ 1) | No | No | full stack | How many to drop from the stack. Exceeding the held quantity → `validation_error`. |
| position | Position | No | Yes | dropper's position | Drop point; farther than the 2-tile **[TUNABLE]** interaction range → `out_of_range`. |

**Server validation** — sender in battle, channeling, or sleeping → `invalid_state`; `item_id` not in the sender's backpack → `not_found`; blue-chest (insured) gear cannot be ground-dropped → `forbidden` (vault gear moves only via HTTP trade/stall flows).

**Results in** — atomically: `run.backpack_update` (`removed`/`dropped`) to the dropper and `world.entity_spawn` (`entity_kind: "item_drop"`) to everyone in radius. The success ack **is** the entity spawn. Ground drops persist until picked up or the instance closes.

**Example**

```json
{"type": "social.drop_item", "seq": 520, "ts": 1783728300000, "payload": {"item_id": "0197a5cc-5555-7abc-9def-0123456789ab", "quantity": 2, "position": null}}
```

---

### `social.pickup_item` (C2S)

Picks a ground item drop into the sender's backpack.

**Source:** GDD.md §6; CANON.md §S.
**Direction:** C2S — any active player (any party — drops are not owner-locked; first-come, first-served).
**Idempotency:** Non-idempotent; racing pickups are serialized server-side — the loser gets `not_found`.

**Payload**

| Field | Type | Required | Nullable | Default | Description |
|-------|------|----------|----------|---------|-------------|
| entity_id | string (uuid) | Yes | No | — | The `item_drop` entity to pick up. |

**Server validation** — sender in battle/channeling/sleeping → `invalid_state`; entity unknown or already claimed → `not_found`; entity not an `item_drop` → `validation_error`; farther than the 2-tile **[TUNABLE]** interaction range → `out_of_range`.

**Results in** — atomically: `social.item_picked_up` ack to the picker, `run.backpack_update` (`added`/`picked_up`) to the picker, `world.entity_despawn` (`reason: "picked_up"`) to everyone in radius.

**Example**

```json
{"type": "social.pickup_item", "seq": 531, "ts": 1783728310000, "payload": {"entity_id": "0197a5cc-4444-7abc-9def-0123456789ab"}}
```

---

### `social.item_picked_up` (S2C)

Pickup confirmation carrying exactly what was gained.

**Source:** GDD.md §6; CANON.md §I (ack echoes client seq).
**Direction:** S2C — to the picking player only, as the direct ack of `social.pickup_item`.

**Payload**

| Field | Type | Required | Nullable | Default | Description |
|-------|------|----------|----------|---------|-------------|
| client_seq | integer (int64, u32 range) | Yes | No | — | Echo of the `social.pickup_item` seq. |
| entity_id | string (uuid) | Yes | No | — | The consumed drop entity. |
| items | array of ItemStack (min 1 item) | Yes | No | — | Items added to the backpack (also mirrored in `run.backpack_update`). |

**Example**

```json
{"type": "social.item_picked_up", "seq": 5600, "ts": 1783728310030, "payload": {"client_seq": 531, "entity_id": "0197a5cc-4444-7abc-9def-0123456789ab", "items": [{"item_id": "0197a5cc-5555-7abc-9def-0123456789ab", "item_kind": "health_potion", "quantity": 2, "insurance": null}]}}
```

---

### `social.drop_item_on_player` (C2S)

Drops a consumable from the sender's backpack directly onto another player's overworld sprite while that player is in a battle — the server intercepts it and injects the effect into their ATB subscreen.

**Source:** GDD.md §6 (real-time influence: health potion on Player A's active battle sprite → instant heal inside the subscreen); CANON.md §S.
**Direction:** C2S — sent by a player **on the world map** (not in battle themselves). The target may be in any party/instance sharing the overworld space.
**Idempotency:** Non-idempotent; `sequence_error` protects retries.

**Payload**

| Field | Type | Required | Nullable | Default | Description |
|-------|------|----------|----------|---------|-------------|
| target_player_id | string (uuid) | Yes | No | — | The battling player whose sprite the item is dropped on. |
| item_id | string (uuid) | Yes | No | — | Backpack consumable to use. Must be battle-usable (e.g. `health_potion`); one unit is consumed. |

**Server validation** (in order)

1. Sender in battle, channeling, or sleeping → `invalid_state` (the dropper must be on the world map, GDD.md §6).
2. `item_id` not in the sender's backpack → `not_found`; not a battle-usable consumable → `validation_error`.
3. Target avatar unknown / not in the sender's interest radius → `not_found`.
4. Target's `avatar_state` is not `in_battle` → `invalid_state` ("Target is not in a battle." — to hand an idle player an item, use `social.drop_item` at their feet instead).
5. Target sprite farther than the 2-tile **[TUNABLE]** interaction range → `out_of_range`.

**Results in** — atomically: one unit consumed (`run.backpack_update`, `removed`/`consumed`, to the sender), `social.drop_applied` ack to the sender, and `battle.external_effect` broadcast inside the target's battle ([battle.md](battle.md#battleexternal_effect-s2c)). The effect applies instantly; it does not consume the target's turn or gauge.

**Example**

```json
{"type": "social.drop_item_on_player", "seq": 544, "ts": 1783728118000, "payload": {"target_player_id": "0197a2f0-11aa-7bbb-8ccc-0d1e2f3a4b5c", "item_id": "0197a612-9999-7abc-9def-0123456789ab"}}
```

---

### `social.drop_applied` (S2C)

Confirmation to the overworld dropper that the item's effect landed inside the target's battle.

**Source:** GDD.md §6; CANON.md §I (ack echoes client seq).
**Direction:** S2C — to the dropping player only, as the direct ack of `social.drop_item_on_player`. (The dropper is not a battle participant and never receives `battle.external_effect`.)

**Payload**

| Field | Type | Required | Nullable | Default | Description |
|-------|------|----------|----------|---------|-------------|
| client_seq | integer (int64, u32 range) | Yes | No | — | Echo of the `social.drop_item_on_player` seq. |
| target_player_id | string (uuid) | Yes | No | — | The recipient. |
| item_kind | string | Yes | No | — | The consumed item's content identifier. |
| effect_summary | string (enum: `healed`, `status_applied`, `no_effect`) | Yes | No | — | Coarse outcome for overworld feedback. `no_effect` when the target's battle ended between validation and application (the item is still consumed **[TUNABLE]**). |

**Example**

```json
{"type": "social.drop_applied", "seq": 5700, "ts": 1783728118040, "payload": {"client_seq": 544, "target_player_id": "0197a2f0-11aa-7bbb-8ccc-0d1e2f3a4b5c", "item_kind": "health_potion", "effect_summary": "healed"}}
```
