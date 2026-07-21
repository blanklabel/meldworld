# Runs & World Endpoints

> Parent: [interfaces/http-api](../http-api.md)

Persistent-side run management (history and preparation) and hub meta-progression (unlock state, rebuilding outer hubs). Everything inside the maze — movement, chunks, battles, extraction channels — is realtime and out of scope; this file marks the handoff points.

## Shared object: Run

**Source:** GDD.md §2.2 (The Maze); CANON.md §G (`Run`, `MazeInstance`), §D13, §B (Hubs & run levels)

| Field | Type | Nullable | Since | Deprecated | Description |
|-------|------|----------|-------|------------|-------------|
| run_id | string (uuid) | No | v0.1 | No | Unique run identifier for the caller's participation. |
| departure_hub_distance | integer (int32) | No | v0.1 | No | The hub the run departs from (0, 500, …, 5000). |
| base_run_level | integer (int32) | No | v0.1 | No | Starting combat level: `round(1 + distance × 0.078)` — Center = 1, D500 = 40, D1000 = 79, D5000 = 391 (CANON.md §B). |
| party_player_ids | array of string (uuid) (1–4 items) | No | v0.1 | No | Members of the MazeInstance's party (CANON.md §D13: 1–4, fixed at maze entry; battle merges never change instance membership). |
| status | string (enum: preparing, active, completed) | No | v0.1 | No | `preparing`: staged/in matchmaking, not yet entered. `active`: in the maze. `completed`: ended — see `outcome`. (Lifecycle states within a run are a spec-level decision; CANON.md names only the end states.) |
| outcome | string (enum: extracted, died, abandoned) | Yes | v0.1 | No | Terminal result (CANON.md §G). `null` until `status` is `completed`. `abandoned` includes the 60-minute all-disconnected auto-abandon (CANON.md §Disconnect handling — counts as death for the backpack, but no durability loss). |
| max_distance_reached | integer (int32) | Yes | v0.1 | No | Highest distance reached during the run (the Vanguard Board metric, CANON.md §D3). `null` while `preparing`. |
| final_run_level | integer (int32) | Yes | v0.1 | No | Run level at run end. `null` until `completed`. |
| started_at | string (date-time) | Yes | v0.1 | No | Maze-entry time. `null` while `preparing`. |
| ended_at | string (date-time) | Yes | v0.1 | No | Run-end time. `null` until `completed`. |

Run records are written by the server at run end (extraction banking, death cleanup — CANON.md §S); clients never mutate them over HTTP.

---

### GET /v1/runs

Lists the caller's run history, most recent first.

**Source:** GDD.md §2.2; CANON.md §S (run history is HTTP-owned)
**Added:** v0.1
**Deprecated:** No
**Auth:** Bearer session token. Returns only runs the caller was a party member of.
**Idempotency:** Safe (read-only).
**Side effects:** None.

**Request** — [pagination parameters](../http-api.md#pagination), plus:

| Field | Type | Required | Nullable | Default | Since | Deprecated | Description |
|-------|------|----------|----------|---------|-------|------------|-------------|
| outcome | string (enum: extracted, died, abandoned), query | No | No | — | v0.1 | No | Filters completed runs by result. When omitted, all runs (including the in-progress one) are returned. |

**Ordering:** By run creation time descending (the `preparing`/`active` run, if any, appears first). Stable across pages.

**Response** — `200 OK` — paginated envelope of [Run](#shared-object-run) objects in `data`, plus `next_cursor` (`null` on last page).

**Example — mid-page response**

```json
HTTP/1.1 200 OK
{"data": [{"run_id": "0195f001-aaaa-7abc-8f01-23456789abcd", "departure_hub_distance": 500, "base_run_level": 40, "party_player_ids": ["0195c9a2-7b1e-7f3a-9d2c-4e5f6a7b8c9d", "0195c9b0-9f00-7abc-8f01-23456789abcd"], "status": "completed", "outcome": "extracted", "max_distance_reached": 812, "final_run_level": 55, "started_at": "2026-07-10T19:00:00Z", "ended_at": "2026-07-10T20:12:00Z"}], "next_cursor": "eyJyIjoiMDE5NWYwMDEifQ=="}
```

**Example — last page**

```json
HTTP/1.1 200 OK
{"data": [{"run_id": "0195f000-0000-7abc-8f01-23456789abcd", "departure_hub_distance": 0, "base_run_level": 1, "party_player_ids": ["0195c9a2-7b1e-7f3a-9d2c-4e5f6a7b8c9d"], "status": "completed", "outcome": "died", "max_distance_reached": 143, "final_run_level": 9, "started_at": "2026-07-01T18:00:00Z", "ended_at": "2026-07-01T18:40:00Z"}], "next_cursor": null}
```

---

### GET /v1/runs/{run_id}

Returns one run the caller participated in.

**Source:** CANON.md §G (`Run`)
**Added:** v0.1
**Deprecated:** No
**Auth:** Bearer session token. Party members only; other players receive 404 (no existence leak).
**Idempotency:** Safe (read-only).
**Side effects:** None.

**Request** — path param `run_id` (string (uuid), required).

**Response** — `200 OK` — a [Run](#shared-object-run) object.

**Error responses**

| Status | Error code | Condition |
|--------|------------|-----------|
| 404 | `not_found` | Run does not exist or the caller was not a party member. |

**Example — success**

```http
GET /v1/runs/0195f001-aaaa-7abc-8f01-23456789abcd
Authorization: Bearer mw-sess-0195c9a4-example-opaque-token
```

```json
HTTP/1.1 200 OK
{"run_id": "0195f001-aaaa-7abc-8f01-23456789abcd", "departure_hub_distance": 500, "base_run_level": 40, "party_player_ids": ["0195c9a2-7b1e-7f3a-9d2c-4e5f6a7b8c9d", "0195c9b0-9f00-7abc-8f01-23456789abcd"], "status": "completed", "outcome": "extracted", "max_distance_reached": 812, "final_run_level": 55, "started_at": "2026-07-10T19:00:00Z", "ended_at": "2026-07-10T20:12:00Z"}
```

---

### POST /v1/runs/prepare

Stages a run: validates the departure hub, party, and build template, then either creates a `preparing` run for a pre-formed party or enqueues the caller into the matchmaking pool for that hub (CANON.md §D13). **Handoff point:** everything after this call — party readiness, MazeInstance creation, maze entry, and the run itself — happens over the realtime channel. Matchmaking pool progress and match notification are realtime concerns and are not observable via HTTP beyond this run's `status`.

**Source:** GDD.md §4 (Base Level Scaling, The Training Ground), §5 (The 4-Player Instance); CANON.md §D13, §B (Hubs & run levels)
**Added:** v0.1
**Deprecated:** No
**Auth:** Bearer session token.
**Idempotency:** Non-idempotent, but retry-safe: a player can have at most one non-`completed` run, so a retry after success returns 409 `conflict`.
**Side effects:** Creates a `Run` in status `preparing` (or registers the caller in the matchmaking pool, in which case the run is created when a party is formed); records the chosen build template's allocations against the run (the template itself may be edited or deleted afterward without affecting this run); locks the caller's loadout (see [equip](vault-gear.md#post-v1vaultgeargear_idequip)).

**Request**

| Field | Type | Required | Nullable | Default | Since | Deprecated | Description |
|-------|------|----------|----------|---------|-------|------------|-------------|
| departure_hub_distance | integer (int32) | Yes | No | — | v0.1 | No | The hub to depart from. Must be unlocked by every party member. Determines `base_run_level = round(1 + distance × 0.078)`. |
| mode | string (enum: party, matchmaking) | Yes | No | — | v0.1 | No | `party`: enter with the listed members. `matchmaking`: solo-join the pool for this hub (CANON.md §D13); pool matches are filtered by departure hub. |
| party_member_ids | array of string (uuid) (1–4 items) | When mode=party | No | — | v0.1 | No | Full party including the caller. All members must have consented via realtime party formation (out of scope); must not exceed 4 (CANON.md §D5, §D13). Must be omitted when `mode` is `matchmaking` (400 if present). |
| class | string (enum: hunter, dragoon, sage, ranger, alchemist_knight, bard) | Yes | No | — | v0.1 | No | The class the caller plays this run. Must be unlocked on the caller's account. |
| build_template_id | string (uuid) | No | Yes | `null` | v0.1 | No | Training Ground template used to auto-allocate skill points at run start (GDD.md §4). `null` or omitted: points are allocated manually in-run (realtime concern). Template's `class` must match `class`. |

**Response** — `202 Accepted`

| Field | Type | Nullable | Since | Deprecated | Description |
|-------|------|----------|-------|------------|-------------|
| run_id | string (uuid) | Yes | v0.1 | No | The staged run. `null` when `mode` is `matchmaking` (the run is created when the pool forms a party; the match arrives via realtime notification). |
| status | string (enum: preparing, queued) | No | v0.1 | No | `preparing` for `party` mode; `queued` for `matchmaking` mode. |
| departure_hub_distance | integer (int32) | No | v0.1 | No | Echo of the validated hub. |
| base_run_level | integer (int32) | No | v0.1 | No | The computed starting run level. |

**Error responses**

| Status | Error code | Condition |
|--------|------------|-----------|
| 400 | `validation_error` | Unknown hub distance; party size 0 or > 4; `party_member_ids` present in matchmaking mode or absent in party mode; caller missing from `party_member_ids`. |
| 403 | `forbidden` | `class` not unlocked on the caller's account. |
| 404 | `not_found` | `build_template_id` does not exist or is not owned by the caller; a `party_member_ids` entry does not exist. |
| 409 | `conflict` | Departure hub not unlocked by the caller or a party member; a member already has a non-`completed` run or is already queued; template `class` mismatch; a member's equipped gear is at 0 max durability. |

**Example — success**

```http
POST /v1/runs/prepare
Authorization: Bearer mw-sess-0195c9a4-example-opaque-token
Content-Type: application/json

{"departure_hub_distance": 500, "mode": "party", "party_member_ids": ["0195c9a2-7b1e-7f3a-9d2c-4e5f6a7b8c9d", "0195c9b0-9f00-7abc-8f01-23456789abcd"], "class": "dragoon", "build_template_id": "0195d100-aaaa-7abc-8f01-23456789abcd"}
```

```json
HTTP/1.1 202 Accepted
{"run_id": "0195f002-bbbb-7abc-8f01-23456789abcd", "status": "preparing", "departure_hub_distance": 500, "base_run_level": 40}
```

**Example — error**

```json
HTTP/1.1 409 Conflict
{"error": {"code": "conflict", "message": "Hub 500 is not unlocked for player ForgeQueen.", "request_id": "0195f003-aaaa-7f3a-9d2c-4e5f6a7b8c9d"}}
```

**Notes**

- There is no HTTP cancel for a `preparing`/`queued` run; abandoning preparation is a realtime action. A run never entered resolves as `abandoned`. Flagged as a design decision.
- Party consent (invites/acceptance) is realtime; this endpoint trusts the roster only after the server verifies realtime-side consent. It returns 409 `conflict` if consent is missing.

---

### GET /v1/hubs

Lists all 11 curated hubs (d = 0, 500, …, 5000 — structural, CANON.md §B) with the caller's per-player unlock state. No hubs exist beyond 5000 (infinite scaling zone).

**Source:** GDD.md §3 (Persistent Milestones), §4; CANON.md §G (`Hub`), §B (Hubs & run levels)
**Added:** v0.1
**Deprecated:** No
**Auth:** Bearer session token.
**Idempotency:** Safe (read-only).
**Side effects:** None.

**Request** — no parameters (the list is always exactly 11 entries; no pagination).

**Response** — `200 OK`

| Field | Type | Nullable | Since | Deprecated | Description |
|-------|------|----------|-------|------------|-------------|
| data | array of object (11 items) | No | v0.1 | No | Hubs ordered by `distance` ascending. |
| data[].distance | integer (int32) | No | v0.1 | No | Hub key: 0, 500, 1000, …, 5000. |
| data[].hub_kind | string (enum: center, outer) | No | v0.1 | No | `center` only for distance 0 (CANON.md §G). |
| data[].base_run_level | integer (int32) | No | v0.1 | No | Starting run level when departing here: `round(1 + distance × 0.078)`. |
| data[].gatekeeper_cleared | boolean | No | v0.1 | No | Whether the caller has cleared the Gatekeeper guarding this hub (the boss at `distance − 1`, CANON.md §B). Always `true` for the Center Hub. |
| data[].unlocked | boolean | No | v0.1 | No | Whether the caller can depart from and place stalls in this hub. Center is always `true`; outer hubs require clearing the Gatekeeper **and** rebuilding (GDD.md §3). |
| data[].rebuild_cost | object | Yes | v0.1 | No | Cost to rebuild, shown only when `gatekeeper_cleared` is `true` and `unlocked` is `false`; otherwise `null`. Shape: `{chits: integer (int64), materials: array of {material_id, quantity}}`. Costs are server-configured per hub [TUNABLE] — no canonical numbers exist (flagged gap). |
| data[].unlocked_at | string (date-time) | Yes | v0.1 | No | When the caller unlocked this hub; `null` if not unlocked. For Center, equals account creation. |

**Error responses** — no endpoint-specific errors beyond [Common Errors](../http-api.md#common-errors).

**Example — success**

```http
GET /v1/hubs
Authorization: Bearer mw-sess-0195c9a4-example-opaque-token
```

```json
HTTP/1.1 200 OK
{"data": [{"distance": 0, "hub_kind": "center", "base_run_level": 1, "gatekeeper_cleared": true, "unlocked": true, "rebuild_cost": null, "unlocked_at": "2026-07-11T10:30:00Z"}, {"distance": 500, "hub_kind": "outer", "base_run_level": 40, "gatekeeper_cleared": true, "unlocked": true, "rebuild_cost": null, "unlocked_at": "2026-07-14T21:30:00Z"}, {"distance": 1000, "hub_kind": "outer", "base_run_level": 79, "gatekeeper_cleared": true, "unlocked": false, "rebuild_cost": {"chits": 25000, "materials": [{"material_id": "tundra_bloom", "quantity": 40}, {"material_id": "iron_ingot", "quantity": 200}]}, "unlocked_at": null}, {"distance": 1500, "hub_kind": "outer", "base_run_level": 118, "gatekeeper_cleared": false, "unlocked": false, "rebuild_cost": null, "unlocked_at": null}]}
```

*(Example truncated to 4 of 11 entries for brevity; the real response always contains all 11.)*

**Notes**

- **Design decision (flagged):** hub unlock state is **per-player** (account-level), matching class unlocks. The GDD's "players can access and rebuild ruined camps" (§3) is ambiguous between per-player and server-global; per-player was chosen so meta-progression is individual.
- Gatekeeper clears are recorded server-side at battle resolution (realtime concern); HTTP only reads them.

---

### POST /v1/hubs/{distance}/rebuild

Rebuilds a ruined camp into an unlocked Outer Hub for the caller, atomically consuming the rebuild cost (chits and materials) from their Vault.

**Source:** GDD.md §3 (Persistent Milestones: "rebuild ruined camps to unlock them as Outer Hubs"); CANON.md §D14, §B (Hubs)
**Added:** v0.1
**Deprecated:** No
**Auth:** Bearer session token.
**Idempotency:** Non-idempotent, but retry-safe: the payment commit also flips `unlocked` to `true`, so a retry returns 409 `conflict` (already unlocked) and cannot double-charge.
**Side effects:** Debits `rebuild_cost.chits` and each `rebuild_cost.materials` entry from the Vault; marks the hub unlocked for the caller (permanent — never wiped by seasons, CANON.md §Sessions & seasons); the hub becomes a valid departure hub and stall location for the caller.

**Request**

| Field | Type | Required | Nullable | Default | Since | Deprecated | Description |
|-------|------|----------|----------|---------|-------|------------|-------------|
| distance | integer (int32), path | Yes | No | — | v0.1 | No | The outer hub to rebuild (500, 1000, …, 5000). |

No body.

**Response** — `200 OK` — the updated hub entry (same shape as a `GET /v1/hubs` item), with `unlocked: true`, `rebuild_cost: null`, and `unlocked_at` set.

**Error responses**

| Status | Error code | Condition |
|--------|------------|-----------|
| 404 | `not_found` | `distance` is not a curated hub (not a multiple of 500 in 500–5000). |
| 409 | `conflict` | Hub is the Center (nothing to rebuild); Gatekeeper not yet cleared by the caller; hub already unlocked; or insufficient materials. |
| 409 | `insufficient_funds` | Vault chits below `rebuild_cost.chits`. |

**Example — success**

```http
POST /v1/hubs/1000/rebuild
Authorization: Bearer mw-sess-0195c9a4-example-opaque-token
```

```json
HTTP/1.1 200 OK
{"distance": 1000, "hub_kind": "outer", "base_run_level": 79, "gatekeeper_cleared": true, "unlocked": true, "rebuild_cost": null, "unlocked_at": "2026-07-11T15:00:00Z"}
```

**Example — error**

```json
HTTP/1.1 409 Conflict
{"error": {"code": "conflict", "message": "The Gatekeeper at distance 999 has not been cleared by this account.", "request_id": "0195f004-bbbb-7f3a-9d2c-4e5f6a7b8c9d"}}
```
