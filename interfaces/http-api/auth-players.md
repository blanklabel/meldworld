# Auth & Player Endpoints

> Parent: [interfaces/http-api](../http-api.md)

Account registration, login (opaque Bearer session token + realtime session ticket issuance, CANON.md D17), and player-facing account data: profile, class unlocks, cosmetics, and titles.

Auth is username + password only — no email, no OAuth, no 2FA at v0.1 (CANON.md D17). Passwords are stored as bcrypt hashes (cost 12 **[TUNABLE]**) in Postgres (CANON.md D17, D18); the plaintext password and the `password_hash` are **never returned by any endpoint** and never logged.

## Shared object: Player

The player account representation returned by `GET /v1/players/me` and embedded in login responses.

**Source:** CANON.md §G (`Player`, `CharacterClass`), GDD.md §2.1, §8

| Field | Type | Nullable | Since | Deprecated | Description |
|-------|------|----------|-------|------------|-------------|
| player_id | string (uuid) | No | v0.1 | No | Unique account identifier. |
| username | string (3–20 chars, pattern: `^[a-zA-Z0-9_]+$`) | No | v0.1 | No | Unique account name (CANON.md D17); the login identifier and public display name. |
| created_at | string (date-time) | No | v0.1 | No | Account creation timestamp. |
| active_title | string | Yes | v0.1 | No | Currently displayed cosmetic title. `null` when no title is set. |
| class_unlocks | array of string (enum: squire, dragoon, sage, ranger, alchemist_knight, bard) | No | v0.1 | No | Character classes unlocked on this account. Always contains at least `squire`. |
| meld_skills | array of object | No | v0.1 | No | The three persistent meld skills. See [crafting-meld.md](crafting-meld.md#shared-object-meld-skill) for the entry shape. |

The **public profile** (`GET /v1/players/{player_id}`) is a subset: `player_id`, `username`, `created_at`, `active_title`, and `class_unlocks`. Meld skill levels are not exposed publicly. The `password_hash` is internal storage only — it appears in no response, public or private.

---

### POST /v1/auth/register

Creates a new player account. The new account starts with `squire` unlocked (the default class), an empty Vault with 0 chits, all three meld skills at level 1 with 0 XP, and only the Center Hub unlocked.

**Source:** GDD.md §2.1, §4.1; CANON.md §D9, §D17, §D18, §G (`Player`)
**Added:** v0.1
**Deprecated:** No
**Auth:** None (unauthenticated).
**Idempotency:** Non-idempotent. Retrying with the same `username` returns 409 `conflict`; no duplicate account is created.
**Side effects:** Creates a `Player` account (password stored as a bcrypt hash, cost 12 **[TUNABLE]**, in Postgres — CANON.md D17/D18; plaintext never persisted or logged), its Vault, and its meld skill records.

**Request**

| Field | Type | Required | Nullable | Default | Since | Deprecated | Description |
|-------|------|----------|----------|---------|-------|------------|-------------|
| username | string (3–20 chars, pattern: `^[a-zA-Z0-9_]+$`) | Yes | No | — | v0.1 | No | Unique account name (CANON.md D17); the login identifier and public display name. |
| password | string (8–128 chars) | Yes | No | — | v0.1 | No | Account password. Never returned by any endpoint. |

**Response** — `201 Created`

| Field | Type | Nullable | Since | Deprecated | Description |
|-------|------|----------|-------|------------|-------------|
| player | Player | No | v0.1 | No | The created account. See [Shared object: Player](#shared-object-player). |

**Error responses**

| Status | Error code | Condition |
|--------|------------|-----------|
| 400 | `validation_error` | Username or password missing, or failing length/pattern constraints (username 3–20 chars `^[a-zA-Z0-9_]+$`; password 8–128 chars). |
| 409 | `conflict` | Username already taken. |

**Example — success**

```http
POST /v1/auth/register
Content-Type: application/json

{"username": "MazeRunner_88", "password": "correct-horse-battery"}
```

```json
HTTP/1.1 201 Created
{"player": {"player_id": "0195c9a2-7b1e-7f3a-9d2c-4e5f6a7b8c9d", "username": "MazeRunner_88", "created_at": "2026-07-11T10:30:00Z", "active_title": null, "class_unlocks": ["squire"], "meld_skills": [{"skill_kind": "forging", "level": 1, "xp": 0}, {"skill_kind": "mercantile", "level": 1, "xp": 0}, {"skill_kind": "alchemy", "level": 1, "xp": 0}]}}
```

**Example — error**

```json
HTTP/1.1 409 Conflict
{"error": {"code": "conflict", "message": "Username 'MazeRunner_88' is already taken.", "request_id": "0195c9a3-1111-7f3a-9d2c-4e5f6a7b8c9d"}}
```

---

### POST /v1/auth/login

Authenticates a player (bcrypt verification of the submitted password against the stored hash, CANON.md D17) and issues an opaque Bearer **session token** for the HTTP API plus a single-use **realtime session ticket** for the WebSocket handshake.

**Source:** CANON.md §D17, §I (session token issued by `/v1/auth/login`), CANON.md §S (realtime channel)
**Added:** v0.1
**Deprecated:** No
**Auth:** None (unauthenticated).
**Idempotency:** Safe to retry. Each call issues a fresh token and ticket; previously issued session tokens remain valid until expiry.
**Side effects:** Issues a session token and a realtime session ticket. No persistent state change beyond session bookkeeping.

**Request**

| Field | Type | Required | Nullable | Default | Since | Deprecated | Description |
|-------|------|----------|----------|---------|-------|------------|-------------|
| username | string | Yes | No | — | v0.1 | No | Registered account name. |
| password | string | Yes | No | — | v0.1 | No | Account password. Verified server-side against the stored bcrypt hash. |

**Response** — `200 OK`

| Field | Type | Nullable | Since | Deprecated | Description |
|-------|------|----------|-------|------------|-------------|
| session_token | string | No | v0.1 | No | Opaque session token for the `Authorization: Bearer` header on all authenticated endpoints. Not a JWT — carries no client-readable claims; clients must not parse it. |
| token_type | string (enum: Bearer) | No | v0.1 | No | Always `Bearer`. |
| expires_in | integer (int32) | No | v0.1 | No | Session token lifetime in seconds. `86400` (24 h, CANON.md D17; server-configured [TUNABLE]). |
| realtime_ticket | string | No | v0.1 | No | Single-use ticket for the realtime WebSocket `session` handshake. Expires 60 seconds after issuance (spec default [TUNABLE]). Consumed on first use. Realtime protocol itself is out of scope — see [Realtime Handoff](../http-api.md#realtime-handoff-out-of-scope). |
| player | Player | No | v0.1 | No | The authenticated account. See [Shared object: Player](#shared-object-player). |

**Error responses**

| Status | Error code | Condition |
|--------|------------|-----------|
| 400 | `validation_error` | Username or password missing. |
| 401 | `unauthorized` | Username not registered **or** password incorrect — the identical status, code, and message for both, to prevent account enumeration. |

**Example — success**

```http
POST /v1/auth/login
Content-Type: application/json

{"username": "MazeRunner_88", "password": "correct-horse-battery"}
```

```json
HTTP/1.1 200 OK
{"session_token": "mw-sess-0195c9a4-example-opaque-token", "token_type": "Bearer", "expires_in": 86400, "realtime_ticket": "rt-0195c9a4-5d2e-7abc-8f01-23456789abcd", "player": {"player_id": "0195c9a2-7b1e-7f3a-9d2c-4e5f6a7b8c9d", "username": "MazeRunner_88", "created_at": "2026-07-11T10:30:00Z", "active_title": null, "class_unlocks": ["squire"], "meld_skills": [{"skill_kind": "forging", "level": 1, "xp": 0}, {"skill_kind": "mercantile", "level": 1, "xp": 0}, {"skill_kind": "alchemy", "level": 1, "xp": 0}]}}
```

**Example — error (identical for unknown username and wrong password)**

```json
HTTP/1.1 401 Unauthorized
{"error": {"code": "unauthorized", "message": "Invalid username or password.", "request_id": "0195c9a5-2222-7f3a-9d2c-4e5f6a7b8c9d"}}
```

---

### GET /v1/players/me

Returns the authenticated player's full account object.

**Source:** CANON.md §G (`Player`)
**Added:** v0.1
**Deprecated:** No
**Auth:** Bearer session token.
**Idempotency:** Safe (read-only).
**Side effects:** None.

**Request** — no parameters.

**Response** — `200 OK` — a [Player](#shared-object-player) object.

**Error responses** — no endpoint-specific errors beyond [Common Errors](../http-api.md#common-errors).

**Example — success**

```http
GET /v1/players/me
Authorization: Bearer mw-sess-0195c9a4-example-opaque-token
```

```json
HTTP/1.1 200 OK
{"player_id": "0195c9a2-7b1e-7f3a-9d2c-4e5f6a7b8c9d", "username": "MazeRunner_88", "created_at": "2026-07-11T10:30:00Z", "active_title": "Tundra Vanguard", "class_unlocks": ["squire", "dragoon"], "meld_skills": [{"skill_kind": "forging", "level": 22, "xp": 4100}, {"skill_kind": "mercantile", "level": 31, "xp": 900}, {"skill_kind": "alchemy", "level": 8, "xp": 55}]}
```

---

### GET /v1/players/{player_id}

Returns another player's public profile (used by stall UIs, contract listings, and leaderboards).

**Source:** CANON.md §G (`Player`); GDD.md §7, §8
**Added:** v0.1
**Deprecated:** No
**Auth:** Bearer session token.
**Idempotency:** Safe (read-only).
**Side effects:** None.

**Request**

| Field | Type | Required | Nullable | Default | Since | Deprecated | Description |
|-------|------|----------|----------|---------|-------|------------|-------------|
| player_id | string (uuid), path | Yes | No | — | v0.1 | No | The player to look up. |

**Response** — `200 OK`

| Field | Type | Nullable | Since | Deprecated | Description |
|-------|------|----------|-------|------------|-------------|
| player_id | string (uuid) | No | v0.1 | No | Unique account identifier. |
| username | string | No | v0.1 | No | Unique account name; the public display name. |
| created_at | string (date-time) | No | v0.1 | No | Account creation timestamp. |
| active_title | string | Yes | v0.1 | No | Currently displayed cosmetic title, or `null`. |
| class_unlocks | array of string (enum: squire, dragoon, sage, ranger, alchemist_knight, bard) | No | v0.1 | No | Character classes unlocked on the account. |

**Error responses**

| Status | Error code | Condition |
|--------|------------|-----------|
| 404 | `not_found` | No player with the given ID. |

**Example — success**

```http
GET /v1/players/0195c9b0-9f00-7abc-8f01-23456789abcd
Authorization: Bearer mw-sess-0195c9a4-example-opaque-token
```

```json
HTTP/1.1 200 OK
{"player_id": "0195c9b0-9f00-7abc-8f01-23456789abcd", "username": "ForgeQueen", "created_at": "2026-05-02T08:15:00Z", "active_title": null, "class_unlocks": ["squire", "sage", "bard"]}
```

---

### GET /v1/players/me/class-unlocks

Returns per-class unlock state for the authenticated player, including which Gatekeeper drop granted each unlock.

**Source:** GDD.md §4 (Gatekeeper Drops); CANON.md §D9, §G (`ClassEmblem`, `GatekeeperBoss`)
**Added:** v0.1
**Deprecated:** No
**Auth:** Bearer session token.
**Idempotency:** Safe (read-only).
**Side effects:** None.

**Request** — no parameters.

**Response** — `200 OK`

| Field | Type | Nullable | Since | Deprecated | Description |
|-------|------|----------|-------|------------|-------------|
| data | array of object | No | v0.1 | No | One entry per launch-set class (6 entries; content team may extend, CANON.md §D9). |
| data[].class | string (enum: squire, dragoon, sage, ranger, alchemist_knight, bard) | No | v0.1 | No | The character class. |
| data[].unlocked | boolean | No | v0.1 | No | Whether the class is unlocked on this account. `squire` is always `true`. |
| data[].emblem_name | string | Yes | v0.1 | No | The class emblem that grants the unlock (e.g. `"Emblem of the Dragoon"`). `null` for `squire`, which has no emblem. |
| data[].unlocked_at | string (date-time) | Yes | v0.1 | No | When the emblem was claimed. `null` if not yet unlocked; for `squire`, equals account creation time. |

Class unlocks are **account-level and permanent**; they are never wiped by death or season end (GDD.md §4; CANON.md §Sessions & seasons). Emblems are granted server-side when a Gatekeeper drop is extracted (realtime/run-end concern — not mutable via HTTP).

**Error responses** — no endpoint-specific errors beyond [Common Errors](../http-api.md#common-errors).

**Example — success**

```http
GET /v1/players/me/class-unlocks
Authorization: Bearer mw-sess-0195c9a4-example-opaque-token
```

```json
HTTP/1.1 200 OK
{"data": [{"class": "squire", "unlocked": true, "emblem_name": null, "unlocked_at": "2026-07-11T10:30:00Z"}, {"class": "dragoon", "unlocked": true, "emblem_name": "Emblem of the Dragoon", "unlocked_at": "2026-07-14T21:02:11Z"}, {"class": "sage", "unlocked": false, "emblem_name": "Emblem of the Sage", "unlocked_at": null}, {"class": "ranger", "unlocked": false, "emblem_name": "Emblem of the Ranger", "unlocked_at": null}, {"class": "alchemist_knight", "unlocked": false, "emblem_name": "Emblem of the Alchemist-Knight", "unlocked_at": null}, {"class": "bard", "unlocked": false, "emblem_name": "Emblem of the Bard", "unlocked_at": null}]}
```

---

### GET /v1/players/me/cosmetics

Lists the authenticated player's owned cosmetics: seasonal titles (granted to top-100 Vanguard instances at season end) and Prestige aura items (deep-distance drops).

**Source:** GDD.md §8 (Prestige cosmetics, seasonal titles); CANON.md §Sessions & seasons
**Added:** v0.1
**Deprecated:** No
**Auth:** Bearer session token.
**Idempotency:** Safe (read-only).
**Side effects:** None.

**Request** — [pagination parameters](../http-api.md#pagination) only.

**Response** — `200 OK` — paginated envelope.

| Field | Type | Nullable | Since | Deprecated | Description |
|-------|------|----------|-------|------------|-------------|
| data | array of object | No | v0.1 | No | Owned cosmetics, ordered by `granted_at` descending; stable across pages. |
| data[].cosmetic_id | string (uuid) | No | v0.1 | No | Unique identifier of the owned cosmetic. |
| data[].kind | string (enum: title, prestige_aura) | No | v0.1 | No | Cosmetic category. `title` entries can be set via [`PUT /v1/players/me/title`](#put-v1playersmetitle). |
| data[].name | string | No | v0.1 | No | Display name (e.g. `"Tundra Vanguard"`, `"Aura of the Sixth Milestone"`). |
| data[].season_id | string (uuid) | Yes | v0.1 | No | The season this cosmetic was earned in. `null` for non-seasonal cosmetics. |
| data[].granted_at | string (date-time) | No | v0.1 | No | When the cosmetic was granted. |
| next_cursor | string | Yes | v0.1 | No | Next page cursor; `null` on the last page. |

**Error responses** — no endpoint-specific errors beyond [Common Errors](../http-api.md#common-errors).

**Example — success (last page)**

```http
GET /v1/players/me/cosmetics?limit=25
Authorization: Bearer mw-sess-0195c9a4-example-opaque-token
```

```json
HTTP/1.1 200 OK
{"data": [{"cosmetic_id": "0195c9c1-0001-7abc-8f01-23456789abcd", "kind": "title", "name": "Tundra Vanguard", "season_id": "0195c000-aaaa-7abc-8f01-23456789abcd", "granted_at": "2026-06-29T00:00:00Z"}, {"cosmetic_id": "0195c9c1-0002-7abc-8f01-23456789abcd", "kind": "prestige_aura", "name": "Aura of the Sixth Milestone", "season_id": null, "granted_at": "2026-06-20T18:44:09Z"}], "next_cursor": null}
```

---

### PUT /v1/players/me/title

Sets or clears the authenticated player's active display title.

**Source:** GDD.md §8 (cosmetic titles in the Hub)
**Added:** v0.1
**Deprecated:** No
**Auth:** Bearer session token.
**Idempotency:** Idempotent — repeating the same request yields the same state.
**Side effects:** Updates `active_title` on the account; visible in the public profile and hub presence.

**Request**

| Field | Type | Required | Nullable | Default | Since | Deprecated | Description |
|-------|------|----------|----------|---------|-------|------------|-------------|
| cosmetic_id | string (uuid) | Yes | Yes | — | v0.1 | No | The owned `title`-kind cosmetic to display. Pass `null` explicitly to clear the active title. |

**Response** — `200 OK`

| Field | Type | Nullable | Since | Deprecated | Description |
|-------|------|----------|-------|------------|-------------|
| active_title | string | Yes | v0.1 | No | The now-active title name, or `null` when cleared. |

**Error responses**

| Status | Error code | Condition |
|--------|------------|-----------|
| 400 | `validation_error` | `cosmetic_id` field omitted entirely (must be present, possibly `null`). |
| 404 | `not_found` | Cosmetic not owned by the caller. |
| 409 | `conflict` | Cosmetic exists but its `kind` is not `title` (e.g. a prestige aura). |

**Example — success**

```http
PUT /v1/players/me/title
Authorization: Bearer mw-sess-0195c9a4-example-opaque-token
Content-Type: application/json

{"cosmetic_id": "0195c9c1-0001-7abc-8f01-23456789abcd"}
```

```json
HTTP/1.1 200 OK
{"active_title": "Tundra Vanguard"}
```

**Notes**

- Prestige auras are equip-visualized on the avatar and handled by the realtime/rendering layer; there is no HTTP "equip aura" operation in v0.1.
