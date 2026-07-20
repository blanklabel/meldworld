# Vault & Gear Endpoints

> Parent: [interfaces/http-api](../http-api.md)

The Vault is the per-player persistent store: chits, extracted crafting materials, gear (blue-chest insured and extracted red-chest), and gems. This file also covers the blue-chest loadout (equip/unequip), gem socketing, crafter-mediated repair, and Training Ground build templates.

## Shared object: GearItem

**Source:** CANON.md §G (`GearItem`, `Gem`), §D6, §B (Death & durability); GDD.md §2.1, §4.1

| Field | Type | Nullable | Since | Deprecated | Description |
|-------|------|----------|-------|------------|-------------|
| gear_id | string (uuid) | No | v0.1 | No | Unique identifier of this gear item. |
| name | string | No | v0.1 | No | Display name (e.g. `"Ashfall Greatblade"`). |
| slot | string | No | v0.1 | No | Equipment slot key (e.g. `"weapon"`, `"armor"`, `"accessory"`). Slot taxonomy is content-defined, not canonical — treat as an opaque string. |
| insurance | string (enum: blue, red) | No | v0.1 | No | Insurance tier. `blue` gear survives death (losing max durability); `red` gear is extraction-converted run loot (CANON.md §G). |
| tier | integer (int32) | No | v0.1 | No | Loot tier band of the item (`tier(d) = floor(d / 100)`, CANON.md §B). |
| stats | object (map of string → integer int32) | No | v0.1 | No | Content-defined stat key → value map (e.g. `{"attack": 42, "speed_stat": 11}`). Stat variance is rolled at craft time (GDD.md §4.1). |
| base_max_durability | integer (int32) | No | v0.1 | No | The as-crafted maximum durability. Upper bound for any repair. |
| max_durability | integer (int32) | No | v0.1 | No | Current maximum durability. Reduced by 10% (rounded down) per death while equipped [TUNABLE] (CANON.md §D6, §B). At `0` the item is unequippable until repaired. |
| socket_count | integer (int32, 0–3) | No | v0.1 | No | Number of gem sockets on this item (content-defined per item). |
| sockets | array of object | No | v0.1 | No | One entry per socket, index-ordered. |
| sockets[].socket_index | integer (int32, 0-based) | No | v0.1 | No | Position of the socket. |
| sockets[].gem_id | string (uuid) | Yes | v0.1 | No | The socketed gem, or `null` when the socket is empty. |
| equipped | boolean | No | v0.1 | No | Whether the item is currently in the blue-chest loadout. |
| created_at | string (date-time) | No | v0.1 | No | When the item entered the Vault (craft time or extraction bank time). |

## Shared object: Gem

**Source:** CANON.md §G (`Gem`); GDD.md §4.1 (Alchemy)

| Field | Type | Nullable | Since | Deprecated | Description |
|-------|------|----------|-------|------------|-------------|
| gem_id | string (uuid) | No | v0.1 | No | Unique identifier of this gem. |
| name | string | No | v0.1 | No | Display name (e.g. `"Ember Materia"`). |
| effect | object (map of string → integer int32) | No | v0.1 | No | Content-defined effect key → magnitude map applied while socketed. |
| socketed_in | string (uuid) | Yes | v0.1 | No | The `gear_id` currently holding this gem, or `null` when loose in the Vault. |
| created_at | string (date-time) | No | v0.1 | No | Synthesis timestamp. |

---

### GET /v1/vault

Returns a summary snapshot of the authenticated player's Vault: chits balance and item counts.

**Source:** GDD.md §2.1 (The Vault); CANON.md §G (`Vault`), §D10
**Added:** v0.1
**Deprecated:** No
**Auth:** Bearer session token.
**Idempotency:** Safe (read-only). Reads are consistent with immediately preceding writes (single authoritative store, CANON.md §D12).
**Side effects:** None.

**Request** — no parameters.

**Response** — `200 OK`

| Field | Type | Nullable | Since | Deprecated | Description |
|-------|------|----------|-------|------------|-------------|
| chits | integer (int64) | No | v0.1 | No | Current chits balance. Never negative; no fractional chits (CANON.md §D10). |
| material_kinds | integer (int32) | No | v0.1 | No | Number of distinct material kinds held. |
| gear_count | integer (int32) | No | v0.1 | No | Number of gear items in the Vault (equipped items included). |
| gem_count | integer (int32) | No | v0.1 | No | Number of gems owned (socketed gems included). |

**Error responses** — no endpoint-specific errors beyond [Common Errors](../http-api.md#common-errors).

**Example — success**

```http
GET /v1/vault
Authorization: Bearer mw-sess-0195c9a4-example-opaque-token
```

```json
HTTP/1.1 200 OK
{"chits": 15250, "material_kinds": 14, "gear_count": 9, "gem_count": 3}
```

---

### GET /v1/vault/materials

Lists the crafting materials in the Vault with quantities.

**Source:** GDD.md §2.1, §4 (Resource Stratification); CANON.md §G (`Vault`)
**Added:** v0.1
**Deprecated:** No
**Auth:** Bearer session token.
**Idempotency:** Safe (read-only).
**Side effects:** None.

**Request** — [pagination parameters](../http-api.md#pagination), plus:

| Field | Type | Required | Nullable | Default | Since | Deprecated | Description |
|-------|------|----------|----------|---------|-------|------------|-------------|
| tier | integer (int32, ≥ 0), query | No | No | — | v0.1 | No | Filters to materials of the given loot tier band. When omitted, all tiers are returned. |

**Ordering:** By `material_id` ascending. Stable across pages.

**Response** — `200 OK` — paginated envelope.

| Field | Type | Nullable | Since | Deprecated | Description |
|-------|------|----------|-------|------------|-------------|
| data | array of object | No | v0.1 | No | Material stacks. |
| data[].material_id | string | No | v0.1 | No | Content-catalog material key (e.g. `"iron_ore"`). Stable identifier used by recipes and contracts. |
| data[].name | string | No | v0.1 | No | Display name (e.g. `"Iron Ore"`). |
| data[].tier | integer (int32) | No | v0.1 | No | Loot tier band the material belongs to. |
| data[].quantity | integer (int64) | No | v0.1 | No | Amount held. |
| next_cursor | string | Yes | v0.1 | No | Next page cursor; `null` on the last page. |

**Example — mid-page response**

```json
HTTP/1.1 200 OK
{"data": [{"material_id": "iron_ore", "name": "Iron Ore", "tier": 0, "quantity": 320}, {"material_id": "sunbaked_hide", "name": "Sunbaked Hide", "tier": 2, "quantity": 41}], "next_cursor": "eyJtIjoic3VuYmFrZWRfaGlkZSJ9"}
```

**Example — last page**

```json
HTTP/1.1 200 OK
{"data": [{"material_id": "tundra_bloom", "name": "Tundra Bloom", "tier": 5, "quantity": 3}], "next_cursor": null}
```

---

### GET /v1/vault/gear

Lists gear items in the Vault.

**Source:** GDD.md §2.1 (Blue Chest Gear); CANON.md §G (`GearItem`)
**Added:** v0.1
**Deprecated:** No
**Auth:** Bearer session token.
**Idempotency:** Safe (read-only).
**Side effects:** None.

**Request** — [pagination parameters](../http-api.md#pagination), plus:

| Field | Type | Required | Nullable | Default | Since | Deprecated | Description |
|-------|------|----------|----------|---------|-------|------------|-------------|
| insurance | string (enum: blue, red), query | No | No | — | v0.1 | No | Filters by insurance tier. When omitted, both tiers are returned. |
| equipped | boolean, query | No | No | — | v0.1 | No | When `true`, returns only the current loadout; when `false`, only unequipped gear. When omitted, no equip filter. |

**Ordering:** By `created_at` descending. Stable across pages.

**Response** — `200 OK` — paginated envelope of [GearItem](#shared-object-gearitem) objects in `data`, plus `next_cursor` (`null` on last page).

**Example — mid-page response**

```json
HTTP/1.1 200 OK
{"data": [{"gear_id": "0195d001-aaaa-7abc-8f01-23456789abcd", "name": "Ashfall Greatblade", "slot": "weapon", "insurance": "blue", "tier": 4, "stats": {"attack": 42, "speed_stat": 11}, "base_max_durability": 100, "max_durability": 81, "socket_count": 2, "sockets": [{"socket_index": 0, "gem_id": "0195d002-bbbb-7abc-8f01-23456789abcd"}, {"socket_index": 1, "gem_id": null}], "equipped": true, "created_at": "2026-06-30T12:00:00Z"}], "next_cursor": "eyJnIjoiMDE5NWQwMDEifQ=="}
```

**Example — last page**

```json
HTTP/1.1 200 OK
{"data": [{"gear_id": "0195d003-cccc-7abc-8f01-23456789abcd", "name": "Duneglass Charm", "slot": "accessory", "insurance": "red", "tier": 3, "stats": {"speed_stat": 20}, "base_max_durability": 60, "max_durability": 60, "socket_count": 0, "sockets": [], "equipped": false, "created_at": "2026-06-21T09:30:00Z"}], "next_cursor": null}
```

---

### GET /v1/vault/gear/{gear_id}

Returns one gear item with full detail.

**Source:** CANON.md §G (`GearItem`)
**Added:** v0.1
**Deprecated:** No
**Auth:** Bearer session token. Only the owner can read a gear item.
**Idempotency:** Safe (read-only).
**Side effects:** None.

**Request**

| Field | Type | Required | Nullable | Default | Since | Deprecated | Description |
|-------|------|----------|----------|---------|-------|------------|-------------|
| gear_id | string (uuid), path | Yes | No | — | v0.1 | No | The gear item to fetch. |

**Response** — `200 OK` — a [GearItem](#shared-object-gearitem) object.

**Error responses**

| Status | Error code | Condition |
|--------|------------|-----------|
| 404 | `not_found` | Gear does not exist or belongs to another player (existence is not leaked). |

**Example — success**

```http
GET /v1/vault/gear/0195d001-aaaa-7abc-8f01-23456789abcd
Authorization: Bearer mw-sess-0195c9a4-example-opaque-token
```

```json
HTTP/1.1 200 OK
{"gear_id": "0195d001-aaaa-7abc-8f01-23456789abcd", "name": "Ashfall Greatblade", "slot": "weapon", "insurance": "blue", "tier": 4, "stats": {"attack": 42, "speed_stat": 11}, "base_max_durability": 100, "max_durability": 81, "socket_count": 2, "sockets": [{"socket_index": 0, "gem_id": "0195d002-bbbb-7abc-8f01-23456789abcd"}, {"socket_index": 1, "gem_id": null}], "equipped": true, "created_at": "2026-06-30T12:00:00Z"}
```

---

### POST /v1/vault/gear/{gear_id}/equip

Equips a blue-chest gear item into the loadout slot named by its `slot` field. Equipped gear accompanies the player into runs; on death it is returned to the Hub with reduced max durability (GDD.md §2.1).

**Source:** GDD.md §2.1 (Blue Chest Gear); CANON.md §D6, §G (`GearItem` with `insurance: blue`)
**Added:** v0.1
**Deprecated:** No
**Auth:** Bearer session token. Owner only.
**Idempotency:** Idempotent for the already-equipped item — re-equipping the same item returns `200` with unchanged state.
**Side effects:** Sets `equipped = true` on the item. Loadout changes take effect at the next run start; they do not affect a run already in progress.
**Concurrency:** Loadout mutations are rejected with 409 `conflict` while the player has a run in progress (the loadout is locked for the duration of the run).

**Request**

| Field | Type | Required | Nullable | Default | Since | Deprecated | Description |
|-------|------|----------|----------|---------|-------|------------|-------------|
| gear_id | string (uuid), path | Yes | No | — | v0.1 | No | The gear item to equip. |

**Response** — `200 OK` — the updated [GearItem](#shared-object-gearitem).

**Error responses**

| Status | Error code | Condition |
|--------|------------|-----------|
| 404 | `not_found` | Gear does not exist or is not owned by the caller. |
| 409 | `conflict` | Another item already occupies this loadout slot (unequip it first); or `insurance` is `red` (only blue-chest gear is equippable — see Notes); or `max_durability` is 0 (CANON.md §D6); or the caller has a run in progress. |

**Example — success**

```http
POST /v1/vault/gear/0195d001-aaaa-7abc-8f01-23456789abcd/equip
Authorization: Bearer mw-sess-0195c9a4-example-opaque-token
```

```json
HTTP/1.1 200 OK
{"gear_id": "0195d001-aaaa-7abc-8f01-23456789abcd", "name": "Ashfall Greatblade", "slot": "weapon", "insurance": "blue", "tier": 4, "stats": {"attack": 42, "speed_stat": 11}, "base_max_durability": 100, "max_durability": 81, "socket_count": 2, "sockets": [{"socket_index": 0, "gem_id": "0195d002-bbbb-7abc-8f01-23456789abcd"}, {"socket_index": 1, "gem_id": null}], "equipped": true, "created_at": "2026-06-30T12:00:00Z"}
```

**Example — error**

```json
HTTP/1.1 409 Conflict
{"error": {"code": "conflict", "message": "Gear at 0 max durability cannot be equipped until repaired.", "request_id": "0195d0a0-3333-7f3a-9d2c-4e5f6a7b8c9d"}}
```

**Notes**

- **Design decision (canon gap):** CANON.md §G says extracted red-chest gear becomes owned Vault gear "still `red` tier" but does not say whether it can join the insured loadout. This spec restricts the loadout to `insurance: blue` items; equipping red gear returns 409 `conflict`. If design later allows carrying red gear into runs uninsured, this is the endpoint to relax.

---

### POST /v1/vault/gear/{gear_id}/unequip

Removes a gear item from the loadout.

**Source:** GDD.md §2.1
**Added:** v0.1
**Deprecated:** No
**Auth:** Bearer session token. Owner only.
**Idempotency:** Idempotent — unequipping an already-unequipped item returns `200` with unchanged state.
**Side effects:** Sets `equipped = false`. Same run-in-progress lock as equip.

**Request**

| Field | Type | Required | Nullable | Default | Since | Deprecated | Description |
|-------|------|----------|----------|---------|-------|------------|-------------|
| gear_id | string (uuid), path | Yes | No | — | v0.1 | No | The gear item to unequip. |

**Response** — `200 OK` — the updated [GearItem](#shared-object-gearitem).

**Error responses**

| Status | Error code | Condition |
|--------|------------|-----------|
| 404 | `not_found` | Gear does not exist or is not owned by the caller. |
| 409 | `conflict` | The caller has a run in progress (loadout locked). |

**Example — success**

```http
POST /v1/vault/gear/0195d001-aaaa-7abc-8f01-23456789abcd/unequip
Authorization: Bearer mw-sess-0195c9a4-example-opaque-token
```

```json
HTTP/1.1 200 OK
{"gear_id": "0195d001-aaaa-7abc-8f01-23456789abcd", "name": "Ashfall Greatblade", "slot": "weapon", "insurance": "blue", "tier": 4, "stats": {"attack": 42, "speed_stat": 11}, "base_max_durability": 100, "max_durability": 81, "socket_count": 2, "sockets": [{"socket_index": 0, "gem_id": "0195d002-bbbb-7abc-8f01-23456789abcd"}, {"socket_index": 1, "gem_id": null}], "equipped": false, "created_at": "2026-06-30T12:00:00Z"}
```

---

### GET /v1/vault/gems

Lists the player's gems, both loose and socketed.

**Source:** GDD.md §4.1 (Alchemy); CANON.md §G (`Gem`)
**Added:** v0.1
**Deprecated:** No
**Auth:** Bearer session token.
**Idempotency:** Safe (read-only).
**Side effects:** None.

**Request** — [pagination parameters](../http-api.md#pagination), plus:

| Field | Type | Required | Nullable | Default | Since | Deprecated | Description |
|-------|------|----------|----------|---------|-------|------------|-------------|
| socketed | boolean, query | No | No | — | v0.1 | No | When `true`, only gems currently in gear; when `false`, only loose gems. When omitted, all gems. |

**Ordering:** By `created_at` descending. Stable across pages.

**Response** — `200 OK` — paginated envelope of [Gem](#shared-object-gem) objects in `data`, plus `next_cursor` (`null` on last page).

**Example — success (last page)**

```json
HTTP/1.1 200 OK
{"data": [{"gem_id": "0195d002-bbbb-7abc-8f01-23456789abcd", "name": "Ember Materia", "effect": {"fire_damage": 8}, "socketed_in": "0195d001-aaaa-7abc-8f01-23456789abcd", "created_at": "2026-06-25T14:00:00Z"}], "next_cursor": null}
```

---

### POST /v1/vault/gear/{gear_id}/sockets

Sockets a loose gem into an empty socket on a blue-chest gear item.

**Source:** GDD.md §4.1 ("permanent 'Materia/Gems' that slot into Blue Chest gear"); CANON.md §G (`Gem`)
**Added:** v0.1
**Deprecated:** No
**Auth:** Bearer session token. Owner of both gear and gem.
**Idempotency:** Non-idempotent, but retry-safe: retrying after success returns 409 `conflict` (socket occupied / gem already socketed); no state is duplicated.
**Side effects:** Sets `sockets[socket_index].gem_id` on the gear and `socketed_in` on the gem. Same run-in-progress lock as equip.

**Request**

| Field | Type | Required | Nullable | Default | Since | Deprecated | Description |
|-------|------|----------|----------|---------|-------|------------|-------------|
| gear_id | string (uuid), path | Yes | No | — | v0.1 | No | The gear item receiving the gem. |
| gem_id | string (uuid) | Yes | No | — | v0.1 | No | The loose gem to socket. Must not currently be socketed anywhere. |
| socket_index | integer (int32, 0-based) | Yes | No | — | v0.1 | No | Target socket. Must be `< socket_count` and empty. |

**Response** — `200 OK` — the updated [GearItem](#shared-object-gearitem).

**Error responses**

| Status | Error code | Condition |
|--------|------------|-----------|
| 400 | `validation_error` | `socket_index` negative or ≥ `socket_count`. |
| 404 | `not_found` | Gear or gem does not exist or is not owned by the caller. |
| 409 | `conflict` | Socket already occupied; gem already socketed in other gear; gear `insurance` is `red` (gems slot only into blue-chest gear); or run in progress. |

**Example — success**

```http
POST /v1/vault/gear/0195d001-aaaa-7abc-8f01-23456789abcd/sockets
Authorization: Bearer mw-sess-0195c9a4-example-opaque-token
Content-Type: application/json

{"gem_id": "0195d004-dddd-7abc-8f01-23456789abcd", "socket_index": 1}
```

```json
HTTP/1.1 200 OK
{"gear_id": "0195d001-aaaa-7abc-8f01-23456789abcd", "name": "Ashfall Greatblade", "slot": "weapon", "insurance": "blue", "tier": 4, "stats": {"attack": 42, "speed_stat": 11}, "base_max_durability": 100, "max_durability": 81, "socket_count": 2, "sockets": [{"socket_index": 0, "gem_id": "0195d002-bbbb-7abc-8f01-23456789abcd"}, {"socket_index": 1, "gem_id": "0195d004-dddd-7abc-8f01-23456789abcd"}], "equipped": true, "created_at": "2026-06-30T12:00:00Z"}
```

**Example — error**

```json
HTTP/1.1 409 Conflict
{"error": {"code": "conflict", "message": "Socket 1 is already occupied.", "request_id": "0195d0a1-4444-7f3a-9d2c-4e5f6a7b8c9d"}}
```

---

### DELETE /v1/vault/gear/{gear_id}/sockets/{socket_index}

Removes the gem from a socket, returning it loose to the Vault. Gems are permanent (GDD.md §4.1); unsocketing never destroys the gem in v0.1.

**Source:** GDD.md §4.1; CANON.md §G (`Gem`)
**Added:** v0.1
**Deprecated:** No
**Auth:** Bearer session token. Owner only.
**Idempotency:** Non-idempotent, but retry-safe: retrying after success returns 409 `conflict` (socket already empty).
**Side effects:** Clears `sockets[socket_index].gem_id`; sets the gem's `socketed_in` to `null`. Same run-in-progress lock as equip.

**Request**

| Field | Type | Required | Nullable | Default | Since | Deprecated | Description |
|-------|------|----------|----------|---------|-------|------------|-------------|
| gear_id | string (uuid), path | Yes | No | — | v0.1 | No | The gear item to unsocket from. |
| socket_index | integer (int32, 0-based), path | Yes | No | — | v0.1 | No | The socket to empty. |

**Response** — `200 OK`

| Field | Type | Nullable | Since | Deprecated | Description |
|-------|------|----------|-------|------------|-------------|
| gear | GearItem | No | v0.1 | No | The updated gear item. |
| gem | Gem | No | v0.1 | No | The now-loose gem (`socketed_in` is `null`). |

**Error responses**

| Status | Error code | Condition |
|--------|------------|-----------|
| 400 | `validation_error` | `socket_index` out of range for this item. |
| 404 | `not_found` | Gear does not exist or is not owned by the caller. |
| 409 | `conflict` | Socket is already empty, or run in progress. |

**Example — success**

```http
DELETE /v1/vault/gear/0195d001-aaaa-7abc-8f01-23456789abcd/sockets/1
Authorization: Bearer mw-sess-0195c9a4-example-opaque-token
```

```json
HTTP/1.1 200 OK
{"gear": {"gear_id": "0195d001-aaaa-7abc-8f01-23456789abcd", "name": "Ashfall Greatblade", "slot": "weapon", "insurance": "blue", "tier": 4, "stats": {"attack": 42, "speed_stat": 11}, "base_max_durability": 100, "max_durability": 81, "socket_count": 2, "sockets": [{"socket_index": 0, "gem_id": "0195d002-bbbb-7abc-8f01-23456789abcd"}, {"socket_index": 1, "gem_id": null}], "equipped": true, "created_at": "2026-06-30T12:00:00Z"}, "gem": {"gem_id": "0195d004-dddd-7abc-8f01-23456789abcd", "name": "Frost Materia", "effect": {"ice_damage": 6}, "socketed_in": null, "created_at": "2026-07-01T08:00:00Z"}}
```

---

### POST /v1/vault/gear/{gear_id}/repair

Restores a gear item's max durability via a crafter, transferring an agreed chits fee from the gear owner to the crafter. The repair ceiling depends on the crafter's Forging level: `floor(base_max_durability × (0.5 + forging_level / 198))`, so a level-99 crafter restores to 100% of `base_max_durability` (CANON.md §B, Death & durability). The repair sets `max_durability` to that ceiling in a single operation.

**Source:** GDD.md §4.1, §7 (The Durability Sink); CANON.md §B (Death & durability), §D6, §D14
**Added:** v0.1
**Deprecated:** No
**Auth:** Bearer session token. Caller must be the gear owner. Self-repair is allowed only when `crafter_player_id` equals the caller's own ID (the caller's own Forging level then sets the ceiling).
**Idempotency:** Non-idempotent, but retry-safe: the transaction is atomic (fee transfer + durability restore commit together, CANON.md §D14); retrying after success returns 409 `conflict` because `max_durability` already equals the crafter's ceiling.
**Side effects:** Debits `fee_chits` from the caller's Vault; credits it to the crafter's Vault (repair fees are **not** hub-taxed — CANON.md §B applies tax only to stall sales and contract payouts); raises the item's `max_durability`; grants Forging XP to the crafter (amount server-configured [TUNABLE] — see Notes).

**Request**

| Field | Type | Required | Nullable | Default | Since | Deprecated | Description |
|-------|------|----------|----------|---------|-------|------------|-------------|
| gear_id | string (uuid), path | Yes | No | — | v0.1 | No | The gear item to repair. Must have `insurance: blue`. |
| crafter_player_id | string (uuid) | Yes | No | — | v0.1 | No | The crafter performing the repair. Their Forging level determines the restore ceiling. |
| fee_chits | integer (int64, ≥ 0) | Yes | No | — | v0.1 | No | The agreed repair fee, paid by the caller to the crafter. `0` is allowed (free repair, e.g. self-repair or a favor). |

**Response** — `200 OK`

| Field | Type | Nullable | Since | Deprecated | Description |
|-------|------|----------|-------|------------|-------------|
| gear | GearItem | No | v0.1 | No | The repaired item with updated `max_durability`. |
| restored_to | integer (int32) | No | v0.1 | No | The new `max_durability` value (the crafter's ceiling for this item). |
| fee_chits | integer (int64) | No | v0.1 | No | The fee transferred. |

**Error responses**

| Status | Error code | Condition |
|--------|------------|-----------|
| 400 | `validation_error` | `fee_chits` negative or fields missing. |
| 404 | `not_found` | Gear not owned by caller, or crafter player does not exist. |
| 409 | `insufficient_funds` | Caller's Vault chits is below `fee_chits`. |
| 409 | `conflict` | `max_durability` already at or above the crafter's ceiling (nothing to restore); gear `insurance` is `red`; or the gear owner has a run in progress with this item equipped. |

**Example — success**

```http
POST /v1/vault/gear/0195d001-aaaa-7abc-8f01-23456789abcd/repair
Authorization: Bearer mw-sess-0195c9a4-example-opaque-token
Content-Type: application/json

{"crafter_player_id": "0195c9b0-9f00-7abc-8f01-23456789abcd", "fee_chits": 450}
```

```json
HTTP/1.1 200 OK
{"gear": {"gear_id": "0195d001-aaaa-7abc-8f01-23456789abcd", "name": "Ashfall Greatblade", "slot": "weapon", "insurance": "blue", "tier": 4, "stats": {"attack": 42, "speed_stat": 11}, "base_max_durability": 100, "max_durability": 92, "socket_count": 2, "sockets": [{"socket_index": 0, "gem_id": "0195d002-bbbb-7abc-8f01-23456789abcd"}, {"socket_index": 1, "gem_id": null}], "equipped": true, "created_at": "2026-06-30T12:00:00Z"}, "restored_to": 92, "fee_chits": 450}
```

**Example — error**

```json
HTTP/1.1 409 Conflict
{"error": {"code": "insufficient_funds", "message": "Vault chits (120) is less than the required amount (450).", "request_id": "0195d0a2-5555-7f3a-9d2c-4e5f6a7b8c9d"}}
```

**Notes**

- **Design gap (flagged):** neither GDD nor CANON defines a crafter consent/offer flow. v0.1 executes the repair immediately with the fee as stated by the gear owner; fee negotiation and crafter consent are assumed to happen out-of-band (chat, stall advertisement). A future version may add an offer/accept handshake. Because the crafter only ever *gains* chits and XP, immediate execution is not exploitable against the crafter.
- Whether repairing grants Forging XP is not explicit in the GDD (it names "successfully combining extracted raw materials"); this spec grants repair XP to keep the durability sink economically attractive. Amount is server config [TUNABLE].
- Repair restores **max durability** only. In-run current durability is ephemeral state (realtime concern, out of scope).

---

### GET /v1/build-templates

Lists the caller's Training Ground build templates. Returns full objects (there is no separate single-GET endpoint).

**Source:** GDD.md §4 (The Training Ground, "Build Templates")
**Added:** v0.1
**Deprecated:** No
**Auth:** Bearer session token.
**Idempotency:** Safe (read-only).
**Side effects:** None.

**Request** — [pagination parameters](../http-api.md#pagination) only.

**Ordering:** By `created_at` ascending. Stable across pages.

**Response** — `200 OK` — paginated envelope.

| Field | Type | Nullable | Since | Deprecated | Description |
|-------|------|----------|-------|------------|-------------|
| data | array of object | No | v0.1 | No | The caller's templates. |
| data[].template_id | string (uuid) | No | v0.1 | No | Unique template identifier. |
| data[].name | string (1–48 chars) | No | v0.1 | No | Player-chosen template name. |
| data[].class | string (enum: hunter, dragoon, sage, ranger, alchemist_knight, bard) | No | v0.1 | No | The character class this template targets. |
| data[].allocations | object (map of string → integer int32, ≥ 0) | No | v0.1 | No | Content-defined combat stat key → skill point weight. Applied proportionally when the template is used at run start (points available depend on `base_run_level`). |
| data[].created_at | string (date-time) | No | v0.1 | No | Creation timestamp. |
| data[].updated_at | string (date-time) | No | v0.1 | No | Last replacement timestamp. |
| next_cursor | string | Yes | v0.1 | No | Next page cursor; `null` on the last page. |

**Example — success (last page)**

```json
HTTP/1.1 200 OK
{"data": [{"template_id": "0195d100-aaaa-7abc-8f01-23456789abcd", "name": "Dragoon Speed Rush", "class": "dragoon", "allocations": {"speed_stat": 60, "attack": 30, "vitality": 10}, "created_at": "2026-07-01T10:00:00Z", "updated_at": "2026-07-05T18:20:00Z"}], "next_cursor": null}
```

---

### POST /v1/build-templates

Creates a build template. A player may hold at most **20** templates (spec default [TUNABLE]).

**Source:** GDD.md §4 (The Training Ground)
**Added:** v0.1
**Deprecated:** No
**Auth:** Bearer session token.
**Idempotency:** Non-idempotent — each call creates a new template (duplicate names are allowed).
**Side effects:** Creates a template owned by the caller.

**Request**

| Field | Type | Required | Nullable | Default | Since | Deprecated | Description |
|-------|------|----------|----------|---------|-------|------------|-------------|
| name | string (1–48 chars) | Yes | No | — | v0.1 | No | Template display name. |
| class | string (enum: hunter, dragoon, sage, ranger, alchemist_knight, bard) | Yes | No | — | v0.1 | No | Target character class. The class need not be unlocked to save a template, but it must be unlocked to *use* it at run start. |
| allocations | object (map of string → integer int32, ≥ 0) | Yes | No | — | v0.1 | No | Stat key → point weight map. Unknown stat keys return 400 `validation_error`. |

**Response** — `201 Created` — the created template object (same shape as list entries).

**Error responses**

| Status | Error code | Condition |
|--------|------------|-----------|
| 400 | `validation_error` | Name length, unknown class, unknown stat key, or negative weight. |
| 409 | `conflict` | Template limit (20) reached. |

**Example — success**

```http
POST /v1/build-templates
Authorization: Bearer mw-sess-0195c9a4-example-opaque-token
Content-Type: application/json

{"name": "Sage Nuker", "class": "sage", "allocations": {"magic": 70, "speed_stat": 20, "vitality": 10}}
```

```json
HTTP/1.1 201 Created
{"template_id": "0195d101-bbbb-7abc-8f01-23456789abcd", "name": "Sage Nuker", "class": "sage", "allocations": {"magic": 70, "speed_stat": 20, "vitality": 10}, "created_at": "2026-07-11T11:00:00Z", "updated_at": "2026-07-11T11:00:00Z"}
```

---

### PUT /v1/build-templates/{template_id}

Fully replaces a build template. All fields must be supplied (this is a replace, not a merge patch); omitted fields return 400 `validation_error`.

**Update semantics:** Full replacement. There is no PATCH in v0.1.

**Source:** GDD.md §4 (The Training Ground)
**Added:** v0.1
**Deprecated:** No
**Auth:** Bearer session token. Owner only.
**Idempotency:** Idempotent — repeating the same request yields the same state (last-write-wins; no optimistic locking in v0.1).
**Side effects:** Replaces the template's `name`, `class`, and `allocations`; updates `updated_at`.

**Request** — path param `template_id` (string (uuid), required) plus the same body fields as `POST /v1/build-templates` (all required).

**Response** — `200 OK` — the updated template object.

**Error responses**

| Status | Error code | Condition |
|--------|------------|-----------|
| 400 | `validation_error` | Missing field, unknown class/stat key, or bad lengths. |
| 404 | `not_found` | Template does not exist or is not owned by the caller. |

**Example — success**

```http
PUT /v1/build-templates/0195d101-bbbb-7abc-8f01-23456789abcd
Authorization: Bearer mw-sess-0195c9a4-example-opaque-token
Content-Type: application/json

{"name": "Sage Nuker v2", "class": "sage", "allocations": {"magic": 80, "speed_stat": 15, "vitality": 5}}
```

```json
HTTP/1.1 200 OK
{"template_id": "0195d101-bbbb-7abc-8f01-23456789abcd", "name": "Sage Nuker v2", "class": "sage", "allocations": {"magic": 80, "speed_stat": 15, "vitality": 5}, "created_at": "2026-07-11T11:00:00Z", "updated_at": "2026-07-11T11:15:00Z"}
```

---

### DELETE /v1/build-templates/{template_id}

Deletes a build template.

**Source:** GDD.md §4 (The Training Ground)
**Added:** v0.1
**Deprecated:** No
**Auth:** Bearer session token. Owner only.
**Idempotency:** Non-idempotent, but retry-safe: a retry after success returns 404 `not_found`.
**Side effects:** Permanently deletes the template. Runs already prepared with this template are unaffected (the allocation was applied at preparation time).

**Request** — path param `template_id` (string (uuid), required).

**Response** — `204 No Content` (empty body).

**Error responses**

| Status | Error code | Condition |
|--------|------------|-----------|
| 404 | `not_found` | Template does not exist or is not owned by the caller. |

**Example — success**

```http
DELETE /v1/build-templates/0195d101-bbbb-7abc-8f01-23456789abcd
Authorization: Bearer mw-sess-0195c9a4-example-opaque-token
```

```json
HTTP/1.1 204 No Content
```
