# Leaderboard & Season Endpoints

> Parent: [interfaces/http-api](../http-api.md)

The Vanguard Board is the seasonal global leaderboard ranking the highest **distance** reached by a MazeInstance during a single run (CANON.md §D3). Seasons are exactly 13 weeks on a rolling UTC boundary (CANON.md §D8). At season end the board is immortalized as a read-only archive, cosmetic titles go to the top 100 instances, and the infinite-zone leaderboard resets; Vaults, hubs, meld skills, and unlocks are never wiped (CANON.md §Sessions & seasons). Rankings are computed server-side from run results; there is no HTTP write surface here.

## Shared object: Vanguard entry

**Source:** GDD.md §8 (The Vanguard Board); CANON.md §D3, §G (`VanguardBoard`, `MazeInstance`)

| Field | Type | Nullable | Since | Deprecated | Description |
|-------|------|----------|-------|------------|-------------|
| rank | integer (int32, ≥ 1) | No | v0.1 | No | Position on the board. 1 is best. Ties broken by earliest `achieved_at`. |
| instance_id | string (uuid) | No | v0.1 | No | The MazeInstance that set the record (one entry per instance per season — matches `VanguardBoardEntry.instance_id` in [data-models](../data-models/world-models.md)). |
| max_distance | integer (int64) | No | v0.1 | No | Highest distance reached by the instance during a single run (floored tile distance, CANON.md §G). |
| players | array of object (1–4 items) | No | v0.1 | No | The instance's party. |
| players[].player_id | string (uuid) | No | v0.1 | No | Party member account ID. |
| players[].username | string | No | v0.1 | No | Party member username. |
| achieved_at | string (date-time) | No | v0.1 | No | When the `max_distance` was reached. |

One entry per instance: an instance's best run of the season is ranked; lesser runs by the same party composition do not occupy additional slots.

## Shared object: Season

**Source:** GDD.md §8 (Seasonal Wipes); CANON.md §D8, §G (`Season`), §Sessions & seasons

| Field | Type | Nullable | Since | Deprecated | Description |
|-------|------|----------|-------|------------|-------------|
| season_id | string (uuid) | No | v0.1 | No | Unique season identifier. |
| number | integer (int32, ≥ 1) | No | v0.1 | No | Sequential season number, starting at 1. |
| starts_at | string (date-time) | No | v0.1 | No | Season start (UTC boundary). |
| ends_at | string (date-time) | No | v0.1 | No | Season end: exactly 13 weeks after `starts_at` (structural, CANON.md §D8). |
| status | string (enum: active, archived) | No | v0.1 | No | Exactly one season is `active` at any time; past seasons are `archived` (read-only, immortalized). |
| title_reward | string | Yes | v0.1 | No | The cosmetic title granted to members of the top 100 instances at season end (e.g. `"Tundra Vanguard"`). `null` until the content team names it. |

---

### GET /v1/leaderboards/vanguard

Returns Vanguard Board entries for the active season, or for an archived season when `season_id` is given.

**Source:** GDD.md §8; CANON.md §D3, §Sessions & seasons
**Added:** v0.1
**Deprecated:** No
**Auth:** Bearer session token.
**Idempotency:** Safe (read-only). Active-season standings update as runs end ("real-time" per GDD §8 means promptly after each run result — there is no intra-run streaming over HTTP; live tickers are a realtime-channel concern, out of scope).
**Side effects:** None.

**Request** — [pagination parameters](../http-api.md#pagination), plus:

| Field | Type | Required | Nullable | Default | Since | Deprecated | Description |
|-------|------|----------|----------|---------|-------|------------|-------------|
| season_id | string (uuid), query | No | No | — | v0.1 | No | The season whose board to read. When omitted, the active season's live board is returned. Archived boards are immutable. |

**Ordering:** By `rank` ascending (i.e. `max_distance` descending, ties by earliest `achieved_at`). Stable across pages for archived seasons; for the active season, ranks may shift between page fetches as new results land — clients should treat cross-page reads of the live board as best-effort.

**Response** — `200 OK` — paginated envelope.

| Field | Type | Nullable | Since | Deprecated | Description |
|-------|------|----------|-------|------------|-------------|
| season | Season | No | v0.1 | No | The season this board belongs to. |
| data | array of Vanguard entry | No | v0.1 | No | The page of ranked entries. |
| next_cursor | string | Yes | v0.1 | No | Next page cursor; `null` on the last page. |

**Error responses**

| Status | Error code | Condition |
|--------|------------|-----------|
| 404 | `not_found` | `season_id` does not match any season. |

**Example — mid-page response**

```http
GET /v1/leaderboards/vanguard?limit=2
Authorization: Bearer mw-sess-0195c9a4-example-opaque-token
```

```json
HTTP/1.1 200 OK
{"season": {"season_id": "0195c001-bbbb-7abc-8f01-23456789abcd", "number": 3, "starts_at": "2026-06-29T00:00:00Z", "ends_at": "2026-09-28T00:00:00Z", "status": "active", "title_reward": "Mire Vanguard"}, "data": [{"rank": 1, "instance_id": "0195f100-aaaa-7abc-8f01-23456789abcd", "max_distance": 6120, "players": [{"player_id": "0195c9b0-9f00-7abc-8f01-23456789abcd", "username": "ForgeQueen"}, {"player_id": "0195c9c5-0101-7abc-8f01-23456789abcd", "username": "OathboundBard"}], "achieved_at": "2026-07-08T22:10:00Z"}, {"rank": 2, "instance_id": "0195f101-bbbb-7abc-8f01-23456789abcd", "max_distance": 5905, "players": [{"player_id": "0195c9a2-7b1e-7f3a-9d2c-4e5f6a7b8c9d", "username": "MazeRunner_88"}], "achieved_at": "2026-07-09T01:44:00Z"}], "next_cursor": "eyJyYW5rIjoyfQ=="}
```

**Example — last page**

```json
HTTP/1.1 200 OK
{"season": {"season_id": "0195c001-bbbb-7abc-8f01-23456789abcd", "number": 3, "starts_at": "2026-06-29T00:00:00Z", "ends_at": "2026-09-28T00:00:00Z", "status": "active", "title_reward": "Mire Vanguard"}, "data": [{"rank": 847, "instance_id": "0195f102-cccc-7abc-8f01-23456789abcd", "max_distance": 512, "players": [{"player_id": "0195c9d0-2222-7abc-8f01-23456789abcd", "username": "PotionPacifist"}], "achieved_at": "2026-07-02T11:00:00Z"}], "next_cursor": null}
```

---

### GET /v1/leaderboards/vanguard/me

Returns the caller's best Vanguard placement in a season (the highest-ranked entry whose party includes the caller).

**Source:** GDD.md §8; CANON.md §D3
**Added:** v0.1
**Deprecated:** No
**Auth:** Bearer session token.
**Idempotency:** Safe (read-only).
**Side effects:** None.

**Request**

| Field | Type | Required | Nullable | Default | Since | Deprecated | Description |
|-------|------|----------|----------|---------|-------|------------|-------------|
| season_id | string (uuid), query | No | No | — | v0.1 | No | The season to check. When omitted, the active season. |

**Response** — `200 OK`

| Field | Type | Nullable | Since | Deprecated | Description |
|-------|------|----------|-------|------------|-------------|
| season | Season | No | v0.1 | No | The season checked. |
| entry | Vanguard entry | Yes | v0.1 | No | The caller's best entry, or `null` when the caller has no ranked run this season (this is a 200, not a 404 — the season exists; the placement doesn't). |

**Error responses**

| Status | Error code | Condition |
|--------|------------|-----------|
| 404 | `not_found` | `season_id` does not match any season. |

**Example — success**

```http
GET /v1/leaderboards/vanguard/me
Authorization: Bearer mw-sess-0195c9a4-example-opaque-token
```

```json
HTTP/1.1 200 OK
{"season": {"season_id": "0195c001-bbbb-7abc-8f01-23456789abcd", "number": 3, "starts_at": "2026-06-29T00:00:00Z", "ends_at": "2026-09-28T00:00:00Z", "status": "active", "title_reward": "Mire Vanguard"}, "entry": {"rank": 2, "instance_id": "0195f101-bbbb-7abc-8f01-23456789abcd", "max_distance": 5905, "players": [{"player_id": "0195c9a2-7b1e-7f3a-9d2c-4e5f6a7b8c9d", "username": "MazeRunner_88"}], "achieved_at": "2026-07-09T01:44:00Z"}}
```

---

### GET /v1/seasons

Lists all seasons, newest first: the active season plus every archived season.

**Source:** GDD.md §8 (Seasonal Wipes); CANON.md §D8, §Sessions & seasons
**Added:** v0.1
**Deprecated:** No
**Auth:** Bearer session token.
**Idempotency:** Safe (read-only).
**Side effects:** None.

**Request** — [pagination parameters](../http-api.md#pagination) only.

**Ordering:** By `number` descending (active season first). Stable across pages.

**Response** — `200 OK` — paginated envelope of [Season](#shared-object-season) objects in `data`, plus `next_cursor` (`null` on last page).

**Example — mid-page response**

```json
HTTP/1.1 200 OK
{"data": [{"season_id": "0195c001-bbbb-7abc-8f01-23456789abcd", "number": 3, "starts_at": "2026-06-29T00:00:00Z", "ends_at": "2026-09-28T00:00:00Z", "status": "active", "title_reward": "Mire Vanguard"}, {"season_id": "0195c000-aaaa-7abc-8f01-23456789abcd", "number": 2, "starts_at": "2026-03-30T00:00:00Z", "ends_at": "2026-06-29T00:00:00Z", "status": "archived", "title_reward": "Tundra Vanguard"}], "next_cursor": "eyJuIjoyfQ=="}
```

**Example — last page**

```json
HTTP/1.1 200 OK
{"data": [{"season_id": "0195bfff-9999-7abc-8f01-23456789abcd", "number": 1, "starts_at": "2025-12-29T00:00:00Z", "ends_at": "2026-03-30T00:00:00Z", "status": "archived", "title_reward": "Ashfall Vanguard"}], "next_cursor": null}
```

---

### GET /v1/seasons/{season_id}

Returns one season's info.

**Source:** CANON.md §D8, §G (`Season`)
**Added:** v0.1
**Deprecated:** No
**Auth:** Bearer session token.
**Idempotency:** Safe (read-only).
**Side effects:** None.

**Request** — path param `season_id` (string (uuid), required).

**Response** — `200 OK` — a [Season](#shared-object-season) object.

**Error responses**

| Status | Error code | Condition |
|--------|------------|-----------|
| 404 | `not_found` | Season does not exist. |

**Example — success**

```http
GET /v1/seasons/0195c000-aaaa-7abc-8f01-23456789abcd
Authorization: Bearer mw-sess-0195c9a4-example-opaque-token
```

```json
HTTP/1.1 200 OK
{"season_id": "0195c000-aaaa-7abc-8f01-23456789abcd", "number": 2, "starts_at": "2026-03-30T00:00:00Z", "ends_at": "2026-06-29T00:00:00Z", "status": "archived", "title_reward": "Tundra Vanguard"}
```

**Notes**

- Season rollover side effects (board archival, title grants to top-100 instance members, infinite-zone leaderboard reset) are server-scheduled at `ends_at`; they are observable via [`GET /v1/players/me/cosmetics`](auth-players.md#get-v1playersmecosmetics) (new title) and the archived board. No HTTP endpoint triggers them.
