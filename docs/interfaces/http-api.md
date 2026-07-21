# MELDWORLD HTTP API

The MELDWORLD HTTP API is the sole interface for all **persistent** game state: accounts, the Vault (chits, materials, gear, gems), meld skills, crafting, stalls, bounty contracts, run history, hub unlocks, and leaderboards. It is consumed by the Bevy UI layer (hub screens, Vault, Training Ground, stall shop, Bounty Board, leaderboards — CANON.md D16). Ephemeral state — movement, chunks, battles, drops, presence — is **out of scope** for this API and flows over the realtime WebSocket protocol; this spec notes handoff points where an HTTP operation transitions into a realtime flow (e.g. run preparation → maze entry).

**Source:** CANON.md §S (System Boundaries), CANON.md §I (Identifier & Wire Conventions), GDD.md §1

## Overview

**Base URL:** `https://api.meldworld.example` (environment-specific host)
**Version:** v0.1 (`/v1/` path prefix on all endpoints)
**Versioning policy:** Pre-release forward-design spec. The `/v1` path prefix is stable; additive fields may appear without a version bump. All fields in this spec are Since `v0.1`.
**Breaking change definition:** Removing a field, changing a field type, changing an error code or status for a documented condition, or changing documented default values.
**Auth model:** Opaque Bearer session token in the `Authorization` header (`Authorization: Bearer <session_token>`), issued by [`POST /v1/auth/login`](http-api/auth-players.md#post-v1authlogin) against username + bcrypt-verified password (CANON.md D17). Tokens expire after 24 h **[TUNABLE]**; the token is opaque — clients must not parse it (no JWT, no OAuth, no refresh tokens in v0.1). All endpoints require auth except `POST /v1/auth/register` and `POST /v1/auth/login`.

**Wire conventions** (CANON.md §I):

- All entity IDs are UUIDv7 strings (`string (uuid)`), server-generated.
- All timestamps are ISO 8601 UTC (`string (date-time)`).
- All field names are `snake_case`.
- Chits is a 64-bit integer (`integer (int64)`); no fractional chits (CANON.md §D10). Note: int64 values can exceed JavaScript's safe integer range; clients must handle accordingly.
- The server is authoritative for every mutation; clients never submit computed outcomes (CANON.md §D11, §S).

## Common Errors

These errors can occur on any endpoint unless noted otherwise. The canonical code set is fixed by CANON.md §I.

| Status | Error code | Condition |
|--------|------------|-----------|
| 400 | `validation_error` | Request body or parameters are malformed, missing required fields, or fail static validation. |
| 401 | `unauthorized` | Bearer token is missing, expired, or malformed. |
| 403 | `forbidden` | Authenticated caller lacks permission or a required capability (e.g. a meld-skill level gate). |
| 404 | `not_found` | Resource with the given ID does not exist, or is not visible to the caller. |
| 409 | `conflict` | The request conflicts with current resource state (e.g. listing already sold, slot occupied, hub not cleared). |
| 409 | `insufficient_funds` | The caller's Vault chits cannot cover the required amount. |
| 429 | `rate_limit_exceeded` | Request rate exceeds the allowed limit. |
| 500 | `internal` | Unexpected server error. Safe to retry with backoff. |

**Status-code conventions used throughout this spec:**

- **400 `validation_error`** — statically invalid input (bad types, out-of-range values, missing required fields).
- **403 `forbidden`** — the caller is authenticated but gated by ownership or a persistent capability requirement (e.g. Mercantile ≥ 30 to place a stall in a hub at distance ≥ 1000).
- **409 `conflict`** — a state precondition failed (already sold, already accepted, already equipped, insufficient materials, gatekeeper not cleared). Retrying without changing state returns the same error.
- **409 `insufficient_funds`** — reserved exclusively for chits shortfalls.

## Error Response Shape

All error responses follow this envelope (CANON.md §I):

```json
{
  "error": {
    "code": "insufficient_funds",
    "message": "Vault chits (120) is less than the required amount (500).",
    "request_id": "0195c9a2-7b1e-7f3a-9d2c-4e5f6a7b8c9d"
  }
}
```

| Field | Type | Nullable | Description |
|-------|------|----------|-------------|
| error.code | string | No | Machine-readable error code (e.g. `validation_error`). Stable — safe to match on. One of the codes in the Common Errors table. |
| error.message | string | No | Human-readable description. May change without notice — do not parse or match on. |
| error.request_id | string (uuid) | No | Unique identifier for this request, for support and tracing. |

## Pagination

List endpoints use **cursor-based pagination** with a shared contract:

**Request parameters (query string):**

| Param | Type | Required | Default | Description |
|-------|------|----------|---------|-------------|
| `cursor` | string | No | — | Opaque cursor from a previous response's `next_cursor`. When omitted, returns the first page. |
| `limit` | integer (int32, 1–100) | No | `25` | Maximum number of items to return per page. Values outside 1–100 return 400 `validation_error`. |

**Response envelope:**

| Field | Type | Nullable | Description |
|-------|------|----------|-------------|
| `data` | array | No | The page of results. May be empty. |
| `next_cursor` | string | Yes | Cursor for the next page. `null` on the last page — this is the sole termination condition. |

Cursors are opaque; clients must not construct or parse them. Each list endpoint documents its sort order; order is stable across pages. Passing a cursor from a different endpoint returns 400 `validation_error`.

## Idempotency

There is no `Idempotency-Key` header mechanism in v0.1. Instead:

- All `GET` endpoints are safe and side-effect free.
- Mutation endpoints document their individual retry semantics. Money-moving endpoints (buy listing, post contract, repair, rebuild) are designed so that a **blind retry after an ambiguous failure cannot double-spend**: the state transition that moves chits also consumes the precondition (e.g. the listing becomes `sold`), so the retry deterministically returns 409 `conflict`.
- All economy transactions execute atomically server-side (CANON.md §D14): either every debit, credit, item transfer, and status change commits, or none do.

## Realtime Handoff (out of scope)

Ephemeral concerns are documented in the realtime protocol spec, not here. Handoff points from this API:

- `POST /v1/auth/login` returns a single-use **realtime session ticket** used to authenticate the WebSocket `session` handshake.
- `POST /v1/runs/prepare` validates and stages a run; instance creation, party readiness, maze entry, and everything inside the maze proceed over the realtime channel. Run completion (extraction banking, death cleanup) is applied to persistent state by the server itself at run end (CANON.md §S), not by client HTTP calls.

## Endpoints

| Method | Path | Summary | Detail |
|--------|------|---------|--------|
| POST | `/v1/auth/register` | Create a player account | [auth-players.md](http-api/auth-players.md) |
| POST | `/v1/auth/login` | Issue session token + realtime session ticket | [auth-players.md](http-api/auth-players.md) |
| GET | `/v1/players/me` | Get own account | [auth-players.md](http-api/auth-players.md) |
| GET | `/v1/players/{player_id}` | Get public player profile | [auth-players.md](http-api/auth-players.md) |
| GET | `/v1/players/me/class-unlocks` | List class unlock state | [auth-players.md](http-api/auth-players.md) |
| GET | `/v1/players/me/cosmetics` | List owned cosmetics and titles | [auth-players.md](http-api/auth-players.md) |
| PUT | `/v1/players/me/title` | Set or clear the active title | [auth-players.md](http-api/auth-players.md) |
| GET | `/v1/vault` | Vault summary (chits + counts) | [vault-gear.md](http-api/vault-gear.md) |
| GET | `/v1/vault/materials` | List Vault materials | [vault-gear.md](http-api/vault-gear.md) |
| GET | `/v1/vault/gear` | List Vault gear | [vault-gear.md](http-api/vault-gear.md) |
| GET | `/v1/vault/gear/{gear_id}` | Gear detail | [vault-gear.md](http-api/vault-gear.md) |
| POST | `/v1/vault/gear/{gear_id}/equip` | Equip blue-chest gear into loadout | [vault-gear.md](http-api/vault-gear.md) |
| POST | `/v1/vault/gear/{gear_id}/unequip` | Unequip gear from loadout | [vault-gear.md](http-api/vault-gear.md) |
| GET | `/v1/vault/gems` | List Vault gems | [vault-gear.md](http-api/vault-gear.md) |
| POST | `/v1/vault/gear/{gear_id}/sockets` | Socket a gem into gear | [vault-gear.md](http-api/vault-gear.md) |
| DELETE | `/v1/vault/gear/{gear_id}/sockets/{socket_index}` | Unsocket a gem | [vault-gear.md](http-api/vault-gear.md) |
| POST | `/v1/vault/gear/{gear_id}/repair` | Repair max durability via a crafter | [vault-gear.md](http-api/vault-gear.md) |
| GET | `/v1/build-templates` | List build templates | [vault-gear.md](http-api/vault-gear.md) |
| POST | `/v1/build-templates` | Create a build template | [vault-gear.md](http-api/vault-gear.md) |
| PUT | `/v1/build-templates/{template_id}` | Replace a build template | [vault-gear.md](http-api/vault-gear.md) |
| DELETE | `/v1/build-templates/{template_id}` | Delete a build template | [vault-gear.md](http-api/vault-gear.md) |
| GET | `/v1/meld-skills` | Read meld skill levels/XP | [crafting-meld.md](http-api/crafting-meld.md) |
| GET | `/v1/recipes` | List crafting/synthesis recipes | [crafting-meld.md](http-api/crafting-meld.md) |
| POST | `/v1/crafting/craft` | Craft an item (Forging) | [crafting-meld.md](http-api/crafting-meld.md) |
| POST | `/v1/crafting/synthesize` | Synthesize a gem (Alchemy) | [crafting-meld.md](http-api/crafting-meld.md) |
| POST | `/v1/stalls` | Deploy a stall | [economy.md](http-api/economy.md) |
| GET | `/v1/stalls` | List stalls in a hub | [economy.md](http-api/economy.md) |
| GET | `/v1/stalls/{stall_id}` | Get a stall with listings | [economy.md](http-api/economy.md) |
| POST | `/v1/stalls/{stall_id}/close` | Close own stall | [economy.md](http-api/economy.md) |
| POST | `/v1/stalls/{stall_id}/listings/{listing_id}/buy` | Buy a listing (atomic, taxed) | [economy.md](http-api/economy.md) |
| POST | `/v1/contracts` | Post a bounty contract (escrow) | [economy.md](http-api/economy.md) |
| GET | `/v1/contracts` | List contracts | [economy.md](http-api/economy.md) |
| GET | `/v1/contracts/{contract_id}` | Get a contract | [economy.md](http-api/economy.md) |
| POST | `/v1/contracts/{contract_id}/accept` | Accept an open contract | [economy.md](http-api/economy.md) |
| POST | `/v1/contracts/{contract_id}/fulfill` | Fulfill an accepted contract | [economy.md](http-api/economy.md) |
| POST | `/v1/contracts/{contract_id}/cancel` | Cancel own open contract (refund) | [economy.md](http-api/economy.md) |
| GET | `/v1/runs` | Run history | [runs-world.md](http-api/runs-world.md) |
| GET | `/v1/runs/{run_id}` | Run detail | [runs-world.md](http-api/runs-world.md) |
| POST | `/v1/runs/prepare` | Prepare a run / enqueue matchmaking | [runs-world.md](http-api/runs-world.md) |
| GET | `/v1/hubs` | List hubs with unlock state | [runs-world.md](http-api/runs-world.md) |
| POST | `/v1/hubs/{distance}/rebuild` | Rebuild an outer hub | [runs-world.md](http-api/runs-world.md) |
| GET | `/v1/leaderboards/vanguard` | Vanguard Board (current or archived season) | [leaderboards.md](http-api/leaderboards.md) |
| GET | `/v1/leaderboards/vanguard/me` | My best Vanguard ranking | [leaderboards.md](http-api/leaderboards.md) |
| GET | `/v1/seasons` | List seasons | [leaderboards.md](http-api/leaderboards.md) |
| GET | `/v1/seasons/{season_id}` | Season info | [leaderboards.md](http-api/leaderboards.md) |
