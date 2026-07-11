# Economy Endpoints (Stalls & Bounty Board)

> Parent: [interfaces/http-api](../http-api.md)

The player-driven economy (GDD.md §7): player Stalls (offline-persistent shops in hubs) and the Bounty Board (escrow-backed gathering Contracts). All operations here are atomic server-side transactions (CANON.md §D14): every debit, credit, item transfer, and status change commits together or not at all.

## Hub tax

**Source:** CANON.md §D7, §B (Economy); GDD.md §4.1 (Mercantile)

`tax_rate(mercantile_level) = max(5%, 10% − mercantile_level × 0.05%)` **[TUNABLE]**. Examples: level 1 → 9.95%, level 50 → 7.5%, level 99 → 5.05%. Tax amounts in chits are `floor(amount × tax_rate)` (rounding down is a spec decision — chits is integral, CANON.md §D10). Tax applies to **stall sales** (paid by the seller: the seller receives `price − tax`) and **contract payouts** (paid by the poster: escrow locks `reward_chits + tax` at posting; the fulfiller receives the full `reward_chits`). The tax rate used is the payer's Mercantile level **at transaction time** (sale time for stalls, posting time for contracts). Tax chits is destroyed (chits sink), not redistributed.

## Shared object: Stall

**Source:** GDD.md §7 (Player Stalls); CANON.md §G (`Stall`), §B (Economy)

| Field | Type | Nullable | Since | Deprecated | Description |
|-------|------|----------|-------|------------|-------------|
| stall_id | string (uuid) | No | v0.1 | No | Unique stall identifier. |
| owner_player_id | string (uuid) | No | v0.1 | No | The deploying player. Remains active while the owner is offline (GDD.md §7). |
| owner_username | string | No | v0.1 | No | Owner's username (denormalized for shop UI). |
| hub_distance | integer (int32) | No | v0.1 | No | The hub the stall is deployed in (0, 500, 1000, …). |
| status | string (enum: open, closed) | No | v0.1 | No | `open` stalls are visible and purchasable; `closed` stalls are terminal (historical record). |
| slot_capacity | integer (int32) | No | v0.1 | No | Listing capacity at deploy time: `4 + floor(mercantile_level / 10) × 2`, max 24 [TUNABLE] (CANON.md §B). |
| listings | array of Listing | No | v0.1 | No | The stall's listings, including sold ones (`status: sold`). |
| created_at | string (date-time) | No | v0.1 | No | Deploy timestamp. |

## Shared object: Listing

A listing offers exactly one thing at a fixed chits price. The `kind` field discriminates the item payload.

| Field | Type | Nullable | Since | Deprecated | Description |
|-------|------|----------|-------|------------|-------------|
| listing_id | string (uuid) | No | v0.1 | No | Unique listing identifier. |
| kind | string (enum: gear, material, gem) | No | v0.1 | No | What is being sold. Determines which item field is non-null. |
| gear | GearItem | Yes | v0.1 | No | The gear item for sale (see [vault-gear.md](vault-gear.md#shared-object-gearitem)). Non-null only when `kind` is `gear`. |
| material | object | Yes | v0.1 | No | `{material_id, name, tier, quantity}` — the material stack for sale. Non-null only when `kind` is `material`. |
| gem | Gem | Yes | v0.1 | No | The gem for sale (must be loose, not socketed). Non-null only when `kind` is `gem`. |
| price_chits | integer (int64, ≥ 1) | No | v0.1 | No | Buyout price paid by the buyer. |
| status | string (enum: available, sold) | No | v0.1 | No | `sold` is terminal. Listings cannot be edited or removed individually in v0.1 — close the stall to reclaim unsold inventory. |
| sold_at | string (date-time) | Yes | v0.1 | No | Sale timestamp; `null` while `available`. |

## Shared object: Contract

**Source:** GDD.md §7 (Bounty Board); CANON.md §G (`Contract`), §B (Contract escrow), §D14

| Field | Type | Nullable | Since | Deprecated | Description |
|-------|------|----------|-------|------------|-------------|
| contract_id | string (uuid) | No | v0.1 | No | Unique contract identifier. |
| poster_player_id | string (uuid) | No | v0.1 | No | The player who posted (and escrowed) the contract. |
| poster_username | string | No | v0.1 | No | Poster's username. |
| material_id | string | No | v0.1 | No | The requested material (content-catalog key, e.g. `"iron_ore"`). Contracts request materials only in v0.1 (GDD.md §7 describes gathering contracts). |
| quantity | integer (int32, ≥ 1) | No | v0.1 | No | Amount of the material required for fulfillment (all-at-once; no partial fulfillment in v0.1). |
| reward_chits | integer (int64, ≥ 1) | No | v0.1 | No | Chits paid to the fulfiller, in full (tax is paid by the poster on top — see [Hub tax](#hub-tax)). |
| escrow_chits | integer (int64) | No | v0.1 | No | Total locked at posting: `reward_chits + floor(reward_chits × tax_rate(poster mercantile at posting))`. |
| status | string (enum: open, accepted, fulfilled, cancelled, expired) | No | v0.1 | No | Lifecycle state. `fulfilled`, `cancelled`, and `expired` are terminal. |
| accepted_by_player_id | string (uuid) | Yes | v0.1 | No | The player who accepted the contract; `null` while `open` and after cancel/expiry-from-open. |
| expires_at | string (date-time) | No | v0.1 | No | Posting time + 7 days [TUNABLE] (CANON.md §B). On expiry the server auto-refunds the full escrow and sets status `expired`. |
| created_at | string (date-time) | No | v0.1 | No | Posting timestamp. |

---

### POST /v1/stalls

Deploys a stall in a hub with an initial set of listings, moving the listed items out of the caller's Vault into the stall. The caller's avatar becomes a shop sprite in that hub (realtime presentation — out of scope); the stall keeps selling after the owner logs off (GDD.md §7).

**Source:** GDD.md §7 (Player Stalls); GDD.md §4.1 (Mercantile stall gates); CANON.md §B (Economy: slots, hub gates)
**Added:** v0.1
**Deprecated:** No
**Auth:** Bearer session token.
**Idempotency:** Non-idempotent, but retry-safe: a player may have at most **one** `open` stall, so retrying after success returns 409 `conflict` and cannot duplicate listings or strand items.
**Side effects:** Creates the stall; removes listed gear/gems and debits listed material quantities from the caller's Vault (items are held by the stall until sold or the stall closes). Equipped gear cannot be listed.

**Request**

| Field | Type | Required | Nullable | Default | Since | Deprecated | Description |
|-------|------|----------|----------|---------|-------|------------|-------------|
| hub_distance | integer (int32) | Yes | No | — | v0.1 | No | Target hub (0, 500, …, 5000). Must be unlocked by the caller. Hubs at distance ≥ 1000 require Mercantile ≥ 30; ≥ 3000 require Mercantile ≥ 60 [TUNABLE] (CANON.md §B). |
| listings | array of object (1 – slot_capacity items) | Yes | No | — | v0.1 | No | Initial listings. Count must not exceed `4 + floor(mercantile_level / 10) × 2` (max 24) [TUNABLE]. |
| listings[].kind | string (enum: gear, material, gem) | Yes | No | — | v0.1 | No | What this listing sells. Determines which reference field is required. |
| listings[].gear_id | string (uuid) | When kind=gear | No | — | v0.1 | No | Vault gear to list. Must be owned, unequipped. Socketed gems travel with the gear. |
| listings[].material_id | string | When kind=material | No | — | v0.1 | No | Material to list. |
| listings[].quantity | integer (int32, ≥ 1) | When kind=material | No | — | v0.1 | No | Stack size to list; debited from the Vault at deploy time. |
| listings[].gem_id | string (uuid) | When kind=gem | No | — | v0.1 | No | Loose gem to list (socketed gems return 409 `conflict`). |
| listings[].price_chits | integer (int64, ≥ 1) | Yes | No | — | v0.1 | No | Buyout price for this listing. |

**Response** — `201 Created` — the created [Stall](#shared-object-stall).

**Error responses**

| Status | Error code | Condition |
|--------|------------|-----------|
| 400 | `validation_error` | Unknown hub distance; empty listings; price < 1; mismatched kind/reference fields. |
| 403 | `forbidden` | Mercantile level below the hub's stall gate (30 for d ≥ 1000, 60 for d ≥ 3000). |
| 404 | `not_found` | Referenced gear/gem does not exist or is not owned by the caller. |
| 409 | `conflict` | Caller already has an `open` stall; listing count exceeds `slot_capacity`; gear equipped; gem socketed; insufficient material quantity; hub not unlocked by the caller. |

**Example — success**

```http
POST /v1/stalls
Authorization: Bearer mw-sess-0195c9a4-example-opaque-token
Content-Type: application/json

{"hub_distance": 500, "listings": [{"kind": "material", "material_id": "iron_ore", "quantity": 50, "price_chits": 500}, {"kind": "gear", "gear_id": "0195d003-cccc-7abc-8f01-23456789abcd", "price_chits": 2200}]}
```

```json
HTTP/1.1 201 Created
{"stall_id": "0195e001-aaaa-7abc-8f01-23456789abcd", "owner_player_id": "0195c9a2-7b1e-7f3a-9d2c-4e5f6a7b8c9d", "owner_username": "MazeRunner_88", "hub_distance": 500, "status": "open", "slot_capacity": 10, "listings": [{"listing_id": "0195e002-bbbb-7abc-8f01-23456789abcd", "kind": "material", "material": {"material_id": "iron_ore", "name": "Iron Ore", "tier": 0, "quantity": 50}, "gear": null, "gem": null, "price_chits": 500, "status": "available", "sold_at": null}, {"listing_id": "0195e003-cccc-7abc-8f01-23456789abcd", "kind": "gear", "gear": {"gear_id": "0195d003-cccc-7abc-8f01-23456789abcd", "name": "Duneglass Charm", "slot": "accessory", "insurance": "red", "tier": 3, "stats": {"speed_stat": 20}, "base_max_durability": 60, "max_durability": 60, "socket_count": 0, "sockets": [], "equipped": false, "created_at": "2026-06-21T09:30:00Z"}, "material": null, "gem": null, "price_chits": 2200, "status": "available", "sold_at": null}], "created_at": "2026-07-11T13:00:00Z"}
```

**Example — error**

```json
HTTP/1.1 403 Forbidden
{"error": {"code": "forbidden", "message": "Placing a stall in hub 1000 requires Mercantile level 30 (you are 22).", "request_id": "0195e004-8888-7f3a-9d2c-4e5f6a7b8c9d"}}
```

**Notes**

- **Design decision:** whether deploying requires the avatar to be physically present in the target hub is a realtime-presence question; v0.1 does **not** enforce presence over HTTP (the hub-unlock check stands in for reachability). Flagged for design review.

---

### GET /v1/stalls

Lists `open` stalls in a hub (the shop directory the tap-to-open UI is built on).

**Source:** GDD.md §7 (Player Stalls)
**Added:** v0.1
**Deprecated:** No
**Auth:** Bearer session token.
**Idempotency:** Safe (read-only).
**Side effects:** None.

**Request** — [pagination parameters](../http-api.md#pagination), plus:

| Field | Type | Required | Nullable | Default | Since | Deprecated | Description |
|-------|------|----------|----------|---------|-------|------------|-------------|
| hub_distance | integer (int32), query | Yes | No | — | v0.1 | No | The hub whose stalls to list. |

**Ordering:** By `created_at` descending. Stable across pages.

**Response** — `200 OK` — paginated envelope of [Stall](#shared-object-stall) summaries in `data` (with `listings` omitted and a `listing_count` integer instead, to keep pages small), plus `next_cursor` (`null` on last page). Fetch a single stall for full listings.

**Example — mid-page response**

```json
HTTP/1.1 200 OK
{"data": [{"stall_id": "0195e001-aaaa-7abc-8f01-23456789abcd", "owner_player_id": "0195c9a2-7b1e-7f3a-9d2c-4e5f6a7b8c9d", "owner_username": "MazeRunner_88", "hub_distance": 500, "status": "open", "slot_capacity": 10, "listing_count": 2, "created_at": "2026-07-11T13:00:00Z"}], "next_cursor": "eyJzIjoiMDE5NWUwMDEifQ=="}
```

**Example — last page**

```json
HTTP/1.1 200 OK
{"data": [{"stall_id": "0195e010-dddd-7abc-8f01-23456789abcd", "owner_player_id": "0195c9b0-9f00-7abc-8f01-23456789abcd", "owner_username": "ForgeQueen", "hub_distance": 500, "status": "open", "slot_capacity": 22, "listing_count": 17, "created_at": "2026-07-10T20:00:00Z"}], "next_cursor": null}
```

---

### GET /v1/stalls/{stall_id}

Returns one stall with its full listings (available and sold).

**Source:** GDD.md §7
**Added:** v0.1
**Deprecated:** No
**Auth:** Bearer session token.
**Idempotency:** Safe (read-only).
**Side effects:** None.

**Request** — path param `stall_id` (string (uuid), required).

**Response** — `200 OK` — a [Stall](#shared-object-stall) object. `closed` stalls remain readable by their owner only; non-owners receive 404.

**Error responses**

| Status | Error code | Condition |
|--------|------------|-----------|
| 404 | `not_found` | Stall does not exist, or is `closed` and the caller is not the owner. |

**Example — success**

```http
GET /v1/stalls/0195e001-aaaa-7abc-8f01-23456789abcd
Authorization: Bearer mw-sess-0195c9a4-example-opaque-token
```

```json
HTTP/1.1 200 OK
{"stall_id": "0195e001-aaaa-7abc-8f01-23456789abcd", "owner_player_id": "0195c9a2-7b1e-7f3a-9d2c-4e5f6a7b8c9d", "owner_username": "MazeRunner_88", "hub_distance": 500, "status": "open", "slot_capacity": 10, "listings": [{"listing_id": "0195e002-bbbb-7abc-8f01-23456789abcd", "kind": "material", "material": {"material_id": "iron_ore", "name": "Iron Ore", "tier": 0, "quantity": 50}, "gear": null, "gem": null, "price_chits": 500, "status": "available", "sold_at": null}], "created_at": "2026-07-11T13:00:00Z"}
```

---

### POST /v1/stalls/{stall_id}/close

Closes the caller's stall: all `available` listings are returned to the caller's Vault (gear and gems as items, materials re-credited to stacks), and the stall becomes `closed` (terminal). Chits already earned from sales was credited at each sale — closing transfers no chits.

**Source:** GDD.md §7; CANON.md §D14
**Added:** v0.1
**Deprecated:** No
**Auth:** Bearer session token. Owner only.
**Idempotency:** Non-idempotent, but retry-safe: retrying after success returns 409 `conflict` (already closed); inventory cannot be returned twice.
**Concurrency:** Close and buy race atomically — a buy that commits first is honored (its item is not returned); a close that commits first causes the in-flight buy to fail with 409 `conflict`.
**Side effects:** Returns unsold inventory to the Vault; sets stall status `closed`; removes the shop sprite from the hub (realtime concern, out of scope).

**Request** — path param `stall_id` (string (uuid), required). No body.

**Response** — `200 OK` — the closed [Stall](#shared-object-stall) (final state, sold listings included for the receipt view).

**Error responses**

| Status | Error code | Condition |
|--------|------------|-----------|
| 403 | `forbidden` | Caller is not the stall owner. |
| 404 | `not_found` | Stall does not exist. |
| 409 | `conflict` | Stall is already `closed`. |

**Example — success**

```http
POST /v1/stalls/0195e001-aaaa-7abc-8f01-23456789abcd/close
Authorization: Bearer mw-sess-0195c9a4-example-opaque-token
```

```json
HTTP/1.1 200 OK
{"stall_id": "0195e001-aaaa-7abc-8f01-23456789abcd", "owner_player_id": "0195c9a2-7b1e-7f3a-9d2c-4e5f6a7b8c9d", "owner_username": "MazeRunner_88", "hub_distance": 500, "status": "closed", "slot_capacity": 10, "listings": [{"listing_id": "0195e002-bbbb-7abc-8f01-23456789abcd", "kind": "material", "material": {"material_id": "iron_ore", "name": "Iron Ore", "tier": 0, "quantity": 50}, "gear": null, "gem": null, "price_chits": 500, "status": "available", "sold_at": null}], "created_at": "2026-07-11T13:00:00Z"}
```

---

### POST /v1/stalls/{stall_id}/listings/{listing_id}/buy

Buys one listing. This is the canonical atomic economy transaction: in a single commit, the buyer's Vault is debited `price_chits`, the seller's Vault is credited `price_chits − floor(price_chits × tax_rate(seller's mercantile level at sale time))`, the item transfers to the buyer's Vault, the listing becomes `sold`, and the seller gains Mercantile XP. Works while the seller is offline (GDD.md §7).

**Source:** GDD.md §7 (Player Stalls); CANON.md §D7, §D14, §B (Economy)
**Added:** v0.1
**Deprecated:** No
**Auth:** Bearer session token. Buyer must not be the stall owner (self-purchase returns 409 `conflict`).
**Idempotency:** Non-idempotent, but **double-spend-proof by construction**: the same commit that moves chits also transitions the listing `available → sold`. A retry after a successful-but-unacknowledged attempt deterministically returns 409 `conflict` ("already sold") and moves no chits. Clients seeing 409 after a network failure should refresh the Vault to learn whether their earlier attempt won.
**Concurrency:** Exactly one concurrent buyer can win a listing. All losers receive 409 `conflict`. There is no reservation or cart mechanism.
**Side effects:** Buyer chits −`price_chits`; seller chits +`price_chits − tax` (tax destroyed — see [Hub tax](#hub-tax)); item added to buyer's Vault; listing `sold`; Mercantile XP granted to the seller (server-configured [TUNABLE]).

**Request**

| Field | Type | Required | Nullable | Default | Since | Deprecated | Description |
|-------|------|----------|----------|---------|-------|------------|-------------|
| stall_id | string (uuid), path | Yes | No | — | v0.1 | No | The stall being bought from. |
| listing_id | string (uuid), path | Yes | No | — | v0.1 | No | The listing to buy. |
| expected_price_chits | integer (int64, ≥ 1) | No | Yes | `null` | v0.1 | No | Price guard. When non-null and it differs from the listing's current `price_chits`, the purchase fails with 409 `conflict` instead of charging an unexpected amount. When omitted or `null`, no guard is applied. (Prices are immutable in v0.1, so this guards against stale UI only.) |

**Response** — `200 OK`

| Field | Type | Nullable | Since | Deprecated | Description |
|-------|------|----------|-------|------------|-------------|
| listing | Listing | No | v0.1 | No | The purchased listing with `status: sold` and `sold_at` set. |
| paid_chits | integer (int64) | No | v0.1 | No | Amount debited from the buyer (equals `price_chits`). |
| buyer_chits_after | integer (int64) | No | v0.1 | No | Buyer's Vault chits balance after the purchase. |

**Error responses**

| Status | Error code | Condition |
|--------|------------|-----------|
| 404 | `not_found` | Stall or listing does not exist, or the stall is `closed`. |
| 409 | `conflict` | Listing already `sold` (including a lost race or a retry of your own success); stall closed mid-flight; buyer is the stall owner; `expected_price_chits` mismatch. |
| 409 | `insufficient_funds` | Buyer's Vault chits is below `price_chits`. |

**Example — success**

```http
POST /v1/stalls/0195e001-aaaa-7abc-8f01-23456789abcd/listings/0195e002-bbbb-7abc-8f01-23456789abcd/buy
Authorization: Bearer mw-sess-0195c9a4-example-opaque-token
Content-Type: application/json

{"expected_price_chits": 500}
```

```json
HTTP/1.1 200 OK
{"listing": {"listing_id": "0195e002-bbbb-7abc-8f01-23456789abcd", "kind": "material", "material": {"material_id": "iron_ore", "name": "Iron Ore", "tier": 0, "quantity": 50}, "gear": null, "gem": null, "price_chits": 500, "status": "sold", "sold_at": "2026-07-11T13:30:00Z"}, "paid_chits": 500, "buyer_chits_after": 14750}
```

**Example — error (lost the race)**

```json
HTTP/1.1 409 Conflict
{"error": {"code": "conflict", "message": "Listing 0195e002-bbbb-7abc-8f01-23456789abcd has already been sold.", "request_id": "0195e005-9999-7f3a-9d2c-4e5f6a7b8c9d"}}
```

**Notes**

- Seller tax example: `price_chits = 500`, seller Mercantile 31 → rate 8.45% → tax `floor(42.25) = 42` → seller receives 458.
- The seller's payout lands directly in their Vault even while offline; there is no claim step.

---

### POST /v1/contracts

Posts a bounty contract, locking the escrow (`reward_chits + tax`) from the caller's Vault in the same transaction. Contracts expire 7 days after posting [TUNABLE] with an automatic full-escrow refund (CANON.md §B).

**Source:** GDD.md §7 (Bounty Board); CANON.md §D14, §B (Contract escrow), §D7
**Added:** v0.1
**Deprecated:** No
**Auth:** Bearer session token.
**Idempotency:** Non-idempotent — each call posts a new contract and locks new escrow. No idempotency key in v0.1; after an ambiguous failure, list your open contracts before retrying to avoid double-posting.
**Side effects:** Debits `escrow_chits` from the caller's Vault into escrow; creates the contract in status `open`; schedules the expiry refund.

**Request**

| Field | Type | Required | Nullable | Default | Since | Deprecated | Description |
|-------|------|----------|----------|---------|-------|------------|-------------|
| material_id | string | Yes | No | — | v0.1 | No | The requested material (content-catalog key). Unknown keys return 400 `validation_error`. |
| quantity | integer (int32, 1–10000) | Yes | No | — | v0.1 | No | Amount required for fulfillment (delivered all at once). |
| reward_chits | integer (int64, ≥ 1) | Yes | No | — | v0.1 | No | Chits the fulfiller will receive in full. The caller is additionally charged `floor(reward_chits × tax_rate(caller's mercantile level))` into escrow. |

**Response** — `201 Created` — the created [Contract](#shared-object-contract).

**Error responses**

| Status | Error code | Condition |
|--------|------------|-----------|
| 400 | `validation_error` | Unknown material; quantity or reward out of range. |
| 409 | `insufficient_funds` | Vault chits below `reward_chits + tax`. |

**Example — success**

```http
POST /v1/contracts
Authorization: Bearer mw-sess-0195c9a4-example-opaque-token
Content-Type: application/json

{"material_id": "iron_ore", "quantity": 50, "reward_chits": 500}
```

```json
HTTP/1.1 201 Created
{"contract_id": "0195e100-aaaa-7abc-8f01-23456789abcd", "poster_player_id": "0195c9b0-9f00-7abc-8f01-23456789abcd", "poster_username": "ForgeQueen", "material_id": "iron_ore", "quantity": 50, "reward_chits": 500, "escrow_chits": 542, "status": "open", "accepted_by_player_id": null, "expires_at": "2026-07-18T14:00:00Z", "created_at": "2026-07-11T14:00:00Z"}
```

**Example — error**

```json
HTTP/1.1 409 Conflict
{"error": {"code": "insufficient_funds", "message": "Posting requires 542 chits (500 reward + 42 tax); Vault holds 300.", "request_id": "0195e101-aaaa-7f3a-9d2c-4e5f6a7b8c9d"}}
```

---

### GET /v1/contracts

Lists bounty contracts. The Bounty Board is global across hubs in v0.1 (contracts are not hub-scoped — flagged design decision; GDD.md §7 does not scope them).

**Source:** GDD.md §7 (Bounty Board)
**Added:** v0.1
**Deprecated:** No
**Auth:** Bearer session token.
**Idempotency:** Safe (read-only).
**Side effects:** None.

**Request** — [pagination parameters](../http-api.md#pagination), plus:

| Field | Type | Required | Nullable | Default | Since | Deprecated | Description |
|-------|------|----------|----------|---------|-------|------------|-------------|
| status | string (enum: open, accepted, fulfilled, cancelled, expired), query | No | No | `open` | v0.1 | No | Lifecycle filter. Defaults to `open` (the board view). |
| material_id | string, query | No | No | — | v0.1 | No | Filters to contracts requesting this material. When omitted, all materials. |
| mine | boolean, query | No | No | `false` | v0.1 | No | When `true`, returns only contracts the caller posted or accepted (any status; overrides the `status` default to "all"). |

**Ordering:** By `created_at` descending. Stable across pages.

**Response** — `200 OK` — paginated envelope of [Contract](#shared-object-contract) objects in `data`, plus `next_cursor` (`null` on last page).

**Example — mid-page response**

```json
HTTP/1.1 200 OK
{"data": [{"contract_id": "0195e100-aaaa-7abc-8f01-23456789abcd", "poster_player_id": "0195c9b0-9f00-7abc-8f01-23456789abcd", "poster_username": "ForgeQueen", "material_id": "iron_ore", "quantity": 50, "reward_chits": 500, "escrow_chits": 542, "status": "open", "accepted_by_player_id": null, "expires_at": "2026-07-18T14:00:00Z", "created_at": "2026-07-11T14:00:00Z"}], "next_cursor": "eyJjIjoiMDE5NWUxMDAifQ=="}
```

**Example — last page**

```json
HTTP/1.1 200 OK
{"data": [{"contract_id": "0195e102-bbbb-7abc-8f01-23456789abcd", "poster_player_id": "0195c9a2-7b1e-7f3a-9d2c-4e5f6a7b8c9d", "poster_username": "MazeRunner_88", "material_id": "tundra_bloom", "quantity": 10, "reward_chits": 1200, "escrow_chits": 1301, "status": "open", "accepted_by_player_id": null, "expires_at": "2026-07-17T09:00:00Z", "created_at": "2026-07-10T09:00:00Z"}], "next_cursor": null}
```

---

### GET /v1/contracts/{contract_id}

Returns one contract in any lifecycle state.

**Source:** GDD.md §7; CANON.md §G (`Contract`)
**Added:** v0.1
**Deprecated:** No
**Auth:** Bearer session token.
**Idempotency:** Safe (read-only).
**Side effects:** None.

**Request** — path param `contract_id` (string (uuid), required).

**Response** — `200 OK` — a [Contract](#shared-object-contract).

**Error responses**

| Status | Error code | Condition |
|--------|------------|-----------|
| 404 | `not_found` | Contract does not exist. |

**Example — success**

```http
GET /v1/contracts/0195e100-aaaa-7abc-8f01-23456789abcd
Authorization: Bearer mw-sess-0195c9a4-example-opaque-token
```

```json
HTTP/1.1 200 OK
{"contract_id": "0195e100-aaaa-7abc-8f01-23456789abcd", "poster_player_id": "0195c9b0-9f00-7abc-8f01-23456789abcd", "poster_username": "ForgeQueen", "material_id": "iron_ore", "quantity": 50, "reward_chits": 500, "escrow_chits": 542, "status": "open", "accepted_by_player_id": null, "expires_at": "2026-07-18T14:00:00Z", "created_at": "2026-07-11T14:00:00Z"}
```

---

### POST /v1/contracts/{contract_id}/accept

Accepts an open contract, claiming it exclusively for the caller. Only the acceptor may fulfill it. Acceptance does not pause the 7-day expiry clock.

**Source:** GDD.md §7 ("Casual players can grab gathering contracts")
**Added:** v0.1
**Deprecated:** No
**Auth:** Bearer session token. The poster cannot accept their own contract.
**Idempotency:** Non-idempotent, but retry-safe: retrying your own successful accept returns 409 `conflict` (already accepted — check `accepted_by_player_id` to see it is you).
**Concurrency:** Exactly one concurrent acceptor wins (`open → accepted` transition is atomic); losers receive 409 `conflict`.
**Side effects:** Sets status `accepted` and `accepted_by_player_id`. No chits moves.

**Request** — path param `contract_id` (string (uuid), required). No body.

**Response** — `200 OK` — the updated [Contract](#shared-object-contract).

**Error responses**

| Status | Error code | Condition |
|--------|------------|-----------|
| 404 | `not_found` | Contract does not exist. |
| 409 | `conflict` | Contract is not `open` (already accepted, fulfilled, cancelled, or expired); or the caller is the poster. |

**Example — success**

```http
POST /v1/contracts/0195e100-aaaa-7abc-8f01-23456789abcd/accept
Authorization: Bearer mw-sess-0195c9a4-example-opaque-token
```

```json
HTTP/1.1 200 OK
{"contract_id": "0195e100-aaaa-7abc-8f01-23456789abcd", "poster_player_id": "0195c9b0-9f00-7abc-8f01-23456789abcd", "poster_username": "ForgeQueen", "material_id": "iron_ore", "quantity": 50, "reward_chits": 500, "escrow_chits": 542, "status": "accepted", "accepted_by_player_id": "0195c9a2-7b1e-7f3a-9d2c-4e5f6a7b8c9d", "expires_at": "2026-07-18T14:00:00Z", "created_at": "2026-07-11T14:00:00Z"}
```

**Notes**

- **Design gap (flagged):** there is no un-accept/abandon operation and no acceptance timeout in the canon. An acceptor who never fulfills blocks the contract until the 7-day expiry refunds the poster. Flagged for design review.

---

### POST /v1/contracts/{contract_id}/fulfill

Fulfills an accepted contract: in one atomic commit, the required material quantity moves from the fulfiller's Vault to the poster's Vault, the full `reward_chits` moves from escrow to the fulfiller's Vault (the tax portion of escrow is destroyed — see [Hub tax](#hub-tax)), status becomes `fulfilled`, and the fulfiller gains Mercantile XP.

**Source:** GDD.md §7, §4.1 (Mercantile "levels up by successfully completing player contracts"); CANON.md §D14, §B
**Added:** v0.1
**Deprecated:** No
**Auth:** Bearer session token. Caller must be `accepted_by_player_id`.
**Idempotency:** Non-idempotent, but double-payout-proof: the payout commit also transitions `accepted → fulfilled`, so a retry returns 409 `conflict` and no second payout occurs.
**Side effects:** Materials fulfiller → poster; `reward_chits` escrow → fulfiller; tax destroyed; status `fulfilled`; Mercantile XP to the fulfiller (server-configured [TUNABLE]).

**Request** — path param `contract_id` (string (uuid), required). No body (the material and quantity are fixed by the contract; delivery is all-or-nothing from the fulfiller's Vault).

**Response** — `200 OK`

| Field | Type | Nullable | Since | Deprecated | Description |
|-------|------|----------|-------|------------|-------------|
| contract | Contract | No | v0.1 | No | The contract with `status: fulfilled`. |
| reward_chits | integer (int64) | No | v0.1 | No | Chits credited to the fulfiller. |
| fulfiller_chits_after | integer (int64) | No | v0.1 | No | Fulfiller's Vault chits after payout. |
| mercantile | Meld Skill | No | v0.1 | No | Fulfiller's Mercantile skill after the XP grant (see [crafting-meld.md](crafting-meld.md#shared-object-meld-skill)). |

**Error responses**

| Status | Error code | Condition |
|--------|------------|-----------|
| 403 | `forbidden` | Caller is not the acceptor of this contract. |
| 404 | `not_found` | Contract does not exist. |
| 409 | `conflict` | Contract not in `accepted` status (open, already fulfilled, cancelled, or expired mid-flight); or fulfiller's Vault holds less than `quantity` of `material_id`. |

**Example — success**

```http
POST /v1/contracts/0195e100-aaaa-7abc-8f01-23456789abcd/fulfill
Authorization: Bearer mw-sess-0195c9a4-example-opaque-token
```

```json
HTTP/1.1 200 OK
{"contract": {"contract_id": "0195e100-aaaa-7abc-8f01-23456789abcd", "poster_player_id": "0195c9b0-9f00-7abc-8f01-23456789abcd", "poster_username": "ForgeQueen", "material_id": "iron_ore", "quantity": 50, "reward_chits": 500, "escrow_chits": 542, "status": "fulfilled", "accepted_by_player_id": "0195c9a2-7b1e-7f3a-9d2c-4e5f6a7b8c9d", "expires_at": "2026-07-18T14:00:00Z", "created_at": "2026-07-11T14:00:00Z"}, "reward_chits": 500, "fulfiller_chits_after": 15250, "mercantile": {"skill_kind": "mercantile", "level": 31, "xp": 1150, "xp_to_next": 8800}}
```

**Example — error**

```json
HTTP/1.1 409 Conflict
{"error": {"code": "conflict", "message": "Vault holds 32x iron_ore; contract requires 50.", "request_id": "0195e103-cccc-7f3a-9d2c-4e5f6a7b8c9d"}}
```

---

### POST /v1/contracts/{contract_id}/cancel

Cancels the caller's own **open** contract, refunding the full escrow (`reward_chits + tax`) to their Vault. Accepted contracts cannot be cancelled in v0.1 (see accept Notes); they resolve by fulfillment or expiry.

**Source:** GDD.md §7; CANON.md §B (Contract escrow, auto-refund)
**Added:** v0.1
**Deprecated:** No
**Auth:** Bearer session token. Poster only.
**Idempotency:** Non-idempotent, but retry-safe: retrying after success returns 409 `conflict` (already cancelled); escrow cannot refund twice.
**Concurrency:** Cancel races atomically with accept: whichever transition commits first wins; the loser gets 409 `conflict`.
**Side effects:** Credits `escrow_chits` back to the poster's Vault; sets status `cancelled` (terminal).

**Request** — path param `contract_id` (string (uuid), required). No body.

**Response** — `200 OK` — the [Contract](#shared-object-contract) with `status: cancelled`.

**Error responses**

| Status | Error code | Condition |
|--------|------------|-----------|
| 403 | `forbidden` | Caller is not the poster. |
| 404 | `not_found` | Contract does not exist. |
| 409 | `conflict` | Contract is not `open` (accepted, fulfilled, already cancelled, or expired). |

**Example — success**

```http
POST /v1/contracts/0195e102-bbbb-7abc-8f01-23456789abcd/cancel
Authorization: Bearer mw-sess-0195c9a4-example-opaque-token
```

```json
HTTP/1.1 200 OK
{"contract_id": "0195e102-bbbb-7abc-8f01-23456789abcd", "poster_player_id": "0195c9a2-7b1e-7f3a-9d2c-4e5f6a7b8c9d", "poster_username": "MazeRunner_88", "material_id": "tundra_bloom", "quantity": 10, "reward_chits": 1200, "escrow_chits": 1301, "status": "cancelled", "accepted_by_player_id": null, "expires_at": "2026-07-17T09:00:00Z", "created_at": "2026-07-10T09:00:00Z"}
```

**Notes**

- **Expiry (server-side behavior, no endpoint):** at `expires_at`, a contract in `open` or `accepted` status transitions to `expired` and the full escrow is refunded to the poster automatically (CANON.md §B). An in-flight fulfill that commits before the expiry sweep wins; after expiry, fulfill returns 409 `conflict`.
