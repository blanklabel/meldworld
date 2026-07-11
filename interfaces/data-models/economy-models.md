# Economy Models

> Parent: [interfaces/data-models](../data-models.md)

Player-driven economy state: `Stall` shops, their `StallListing` offers, bounty-board `Contract` orders with chits escrow, and the append-only `LedgerEntry` chits record. All economy transactions are persistent-state mutations: they execute server-side, atomically, through the HTTP API — never over the realtime channel (CANON D14, §S). All amounts are whole chits (int64, CANON D10).

## Models

### `Stall`

A player shop deployed in a hub. The owner's avatar becomes a shop sprite, and the stall keeps selling while the owner is offline.

**Source:** GDD.md §7 (Player Stalls); CANON.md §G (Stall), §D (D14), §B (Economy)

| Field | Type | Required | Nullable | Default | Since | Deprecated | Description |
|-------|------|----------|----------|---------|-------|------------|-------------|
| id | string (uuid) | Yes | No | — | v0.1 | No | The unique stall identifier. Server-assigned UUIDv7. |
| owner_player_id | string (uuid) | Yes | No | — | v0.1 | No | The player operating the stall. A player has at most one deployed stall at a time. |
| hub_distance | integer (int64, one of 0, 500, …, 5000) | Yes | No | — | v0.1 | No | The hub the stall is deployed in. Deployment in hubs at distance ≥ 1000 requires Mercantile ≥ 30; distance ≥ 3000 requires Mercantile ≥ 60 [TUNABLE]. |
| slot_capacity | integer (int32, 4–24) | Yes | No | `4` | v0.1 | No | The maximum concurrent listings, `4 + floor(mercantile_level / 10) × 2`, capped at 24 [TUNABLE]. Snapshotted from the owner's Mercantile level at deployment. |
| deployed_at | string (date-time) | Yes | No | — | v0.1 | No | Timestamp of deployment. Server-assigned. |
| packed_at | string (date-time) | No | Yes | null | v0.1 | No | Timestamp when the owner packed the stall up. `null` while deployed. Sales stop at pack-up; unsold listings return to the owner's vault. |

**Relationships**

- Belongs to one `Player` via `owner_player_id`.
- Has many `StallListing` records via `stall_id`.

**Notes**

- Invariant: active (unsold, unremoved) listings never exceed `slot_capacity`.
- Invariant: stalls exist only in hubs — never in the maze (hubs are the only no-combat safe zones, GDD §2.1).
- The stall remains active and can complete sales while the owner is logged off (GDD §7); fulfillment is server-side and atomic (CANON D14).

### `StallListing`

One item (or stack) offered for sale at a stall.

**Source:** GDD.md §7 (Player Stalls); CANON.md §D (D14), §B (Economy), §I (error codes)

| Field | Type | Required | Nullable | Default | Since | Deprecated | Description |
|-------|------|----------|----------|---------|-------|------------|-------------|
| id | string (uuid) | Yes | No | — | v0.1 | No | The unique listing identifier. Server-assigned UUIDv7. |
| stall_id | string (uuid) | Yes | No | — | v0.1 | No | The stall offering the item. |
| item_kind | string (enum: gear_item, gem, consumable, ward_item, material) | Yes | No | — | v0.1 | No | The model type of the listed item. |
| item_id | string (uuid) | Yes | No | — | v0.1 | No | The listed item record. The item leaves the seller's vault while listed and cannot be equipped, consumed, or listed elsewhere. |
| quantity | integer (int32, ≥ 1) | Yes | No | `1` | v0.1 | No | The stack size offered. Always 1 for `gear_item` and `gem`. |
| price_chits | integer (int64, ≥ 1) | Yes | No | — | v0.1 | No | The asking price in whole chits for the entire listing. |
| listed_at | string (date-time) | Yes | No | — | v0.1 | No | Timestamp of listing. Server-assigned. |
| sold_at | string (date-time) | No | Yes | null | v0.1 | No | Timestamp of sale. `null` while unsold. |
| buyer_player_id | string (uuid) | No | Yes | null | v0.1 | No | The purchasing player. `null` while unsold. |

**Relationships**

- Belongs to one `Stall` via `stall_id`; references exactly one item record via (`item_kind`, `item_id`).

**Notes**

- Invariant: `sold_at` and `buyer_player_id` are set together, exactly once; a sold listing is immutable.
- Purchase is a single atomic transaction: the buyer's vault is debited `price_chits` (rejected with `insufficient_funds` if short), the item moves to the buyer's vault, and the seller's vault is credited `price_chits` minus hub tax. Tax is `10% − seller_mercantile_level × 0.05%`, floor 5%, paid by the seller [TUNABLE].
- Each completed sale credits the seller's Mercantile XP (GDD §4.1).

### `Contract`

A bounty-board gathering order: an item, a quantity, an escrowed chits reward, and an expiry.

**Source:** GDD.md §7 (Bounty Board); CANON.md §G (Contract), §D (D14), §B (Economy)

| Field | Type | Required | Nullable | Default | Since | Deprecated | Description |
|-------|------|----------|----------|---------|-------|------------|-------------|
| id | string (uuid) | Yes | No | — | v0.1 | No | The unique contract identifier. Server-assigned UUIDv7. |
| poster_player_id | string (uuid) | Yes | No | — | v0.1 | No | The player who posted the order. |
| hub_distance | integer (int64, one of 0, 500, …, 5000) | Yes | No | — | v0.1 | No | The hub whose bounty board carries the contract. |
| material_name | string | Yes | No | — | v0.1 | No | The requested material, matched against `Material.name` (e.g. "Iron Ore"). |
| quantity | integer (int32, ≥ 1) | Yes | No | — | v0.1 | No | The number of units required to fulfill the contract. |
| reward_chits | integer (int64, ≥ 1) | Yes | No | — | v0.1 | No | The chits reward, locked in escrow from the poster's vault at posting time. |
| state | string (enum: open, fulfilled, expired) | Yes | No | `open` | v0.1 | No | The contract lifecycle state. `fulfilled`: materials delivered, escrow paid out. `expired`: 7-day expiry reached, escrow auto-refunded in full [TUNABLE]. |
| posted_at | string (date-time) | Yes | No | — | v0.1 | No | Timestamp of posting. Server-assigned. Posting fails with `insufficient_funds` if the poster's vault cannot cover `reward_chits`. |
| expires_at | string (date-time) | Yes | No | — | v0.1 | No | The expiry deadline, `posted_at` plus 7 days [TUNABLE]. |
| fulfilled_by_player_id | string (uuid) | No | Yes | null | v0.1 | No | The player who delivered the materials. `null` until fulfilled. |
| fulfilled_at | string (date-time) | No | Yes | null | v0.1 | No | Timestamp of fulfillment. `null` until fulfilled. |

**Relationships**

- Belongs to one `Player` via `poster_player_id`; fulfilled by at most one `Player` via `fulfilled_by_player_id`.

**Notes**

- Invariant: the only state transitions are `open → fulfilled` and `open → expired`; terminal states are immutable.
- Invariant: `reward_chits` is debited from the poster's vault at posting and held in escrow; exactly one of payout or refund ever occurs.
- Fulfillment is a single atomic transaction: `quantity` units of the material move from the fulfiller's vault to the poster's vault, and the fulfiller's vault is credited `reward_chits` minus hub tax (`10% − poster_mercantile_level × 0.05%`, floor 5%, paid by the poster from escrow) [TUNABLE].
- Each completed contract credits Mercantile XP (GDD §4.1). Contracts are designed to give direction to short 15-minute sessions (GDD §7).
- CANON defines no poster-cancellation path; only fulfillment or expiry releases escrow. If cancellation is added later it must be specified as a new state.

### `LedgerEntry`

An immutable record of one chits movement. Every mutation of any vault's chits produces exactly one entry.

**Source:** GDD.md §7; CANON.md §D (D10, D14), §B (Economy). Model name is spec-assigned; CANON §G defines no ledger entity.

| Field | Type | Required | Nullable | Default | Since | Deprecated | Description |
|-------|------|----------|----------|---------|-------|------------|-------------|
| id | string (uuid) | Yes | No | — | v0.1 | No | The unique ledger-entry identifier. Server-assigned UUIDv7. |
| entry_kind | string (enum: stall_sale, contract_escrow_lock, contract_payout, contract_refund, hub_tax) | Yes | No | — | v0.1 | No | The transaction type. `hub_tax` entries record the tax portion withheld from a sale or payout; taxed chits leaves the economy (a sink). |
| amount_chits | integer (int64, ≥ 1) | Yes | No | — | v0.1 | No | The chits moved, in whole units. Always positive; direction is carried by the debit/credit fields. |
| debit_player_id | string (uuid) | No | Yes | null | v0.1 | No | The player whose vault was debited. `null` when the source is escrow or the system. |
| credit_player_id | string (uuid) | No | Yes | null | v0.1 | No | The player whose vault was credited. `null` when the destination is escrow or the tax sink. |
| reference_kind | string (enum: stall_listing, contract) | Yes | No | — | v0.1 | No | The model type of the originating record. |
| reference_id | string (uuid) | Yes | No | — | v0.1 | No | The originating `StallListing` or `Contract`. |
| hub_distance | integer (int64) | Yes | No | — | v0.1 | No | The hub where the transaction occurred. |
| ts | string (date-time) | Yes | No | — | v0.1 | No | Timestamp of the transaction. Server-assigned. |

**Relationships**

- References one `StallListing` or one `Contract` via (`reference_kind`, `reference_id`).

**Notes**

- Invariant: entries are append-only and immutable; no update or delete ever occurs.
- Invariant: at least one of `debit_player_id` / `credit_player_id` is non-null.
- Invariant: no transaction may drive any vault's chits below zero — the whole transaction is rejected atomically with `insufficient_funds` and no ledger entry is written.
- A single sale or fulfillment writes multiple entries (e.g. a stall sale writes one `stall_sale` for the net proceeds and one `hub_tax` for the withheld tax), all within one atomic transaction.
