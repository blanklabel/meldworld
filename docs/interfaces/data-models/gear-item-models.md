# Gear & Item Models

> Parent: [interfaces/data-models](../data-models.md)

Item-domain models: `GearItem` (insured equipment with durability and gem sockets), `Gem` (permanent socketables), `ConsumableItem` (potions and escape items), `WardItem` (deployable protection for sleeping avatars), and `Material` (raw crafting inputs). Item location is expressed by mutually exclusive `vault_id` / `backpack_id` references: a vault-held item is persistent, a backpack-held item is ephemeral run loot.

## Models

### `GearItem`

A piece of equipment. Insurance tier determines whether it survives death (`blue`) or is run-loot lost on death (`red`).

**Source:** GDD.md §2.1, §2.2, §7 (Durability Sink); CANON.md §G (Blue/Red Chest gear, Gem), §D (D6), §B (Distance → difficulty; Death & durability)

| Field | Type | Required | Nullable | Default | Since | Deprecated | Description |
|-------|------|----------|----------|---------|-------|------------|-------------|
| id | string (uuid) | Yes | No | — | v0.1 | No | The unique gear identifier. Server-assigned UUIDv7. |
| name | string | Yes | No | — | v0.1 | No | The display name. Content-defined. |
| slot | string | Yes | No | — | v0.1 | No | The equipment slot this item occupies. Slot names are content-defined; CANON does not enumerate them. |
| insurance | string (enum: blue, red) | Yes | No | — | v0.1 | No | The insurance tier. `blue`: permanent insured gear that returns to the Hub on death at reduced max durability. `red`: run-found power gear, lost on death; extraction converts it to Vault-owned gear that remains `red`. |
| tier | integer (int32, ≥ 0) | Yes | No | — | v0.1 | No | The loot tier band at generation, `floor(drop_distance / 100)`. Red gear never generates below tier 3 (distance 300) [TUNABLE]. |
| durability | integer (int32, ≥ 0) | Yes | No | — | v0.1 | No | Current durability. Never exceeds `max_durability`. |
| max_durability | integer (int32, ≥ 0) | Yes | No | — | v0.1 | No | Current durability ceiling. Reduced to `max_durability × 0.9` (rounded down) each time the owner dies in the Maze with this gear insured `blue` [TUNABLE]. Gear at 0 is unequippable until repaired. |
| base_max_durability | integer (int32, ≥ 1) | Yes | No | — | v0.1 | No | The as-created durability ceiling. Repairs can restore `max_durability` up to `base_max_durability × (0.5 + forging_level/198)` [TUNABLE]. |
| socket_count | integer (int32, ≥ 0) | Yes | No | `0` | v0.1 | No | The number of gem sockets. Only `blue` gear accepts socketed gems. |
| stats | object | Yes | No | `{}` | v0.1 | No | Map of stat-modifier names to values granted while equipped. Stat names are content-defined; crafted gear rolls stat variance improved by the crafter's Forging level. |
| owner_player_id | string (uuid) | No | Yes | null | v0.1 | No | The owning player. `null` only for `red` gear still inside a run backpack (unowned until extracted). |
| vault_id | string (uuid) | No | Yes | null | v0.1 | No | The vault holding this gear. Always set for `blue` gear; set for `red` gear only after extraction. |
| backpack_id | string (uuid) | No | Yes | null | v0.1 | No | The run backpack holding this gear. Set only for `red` gear found during an active run. |
| equipped | boolean | Yes | No | `false` | v0.1 | No | Whether the gear is currently worn by the owner's avatar. |

**Relationships**

- Belongs to at most one `Vault` via `vault_id` or one `Backpack` via `backpack_id` (mutually exclusive).
- Has up to `socket_count` socketed `Gem` records via `Gem.gear_item_id`.

**Notes**

- Invariant: `0 ≤ durability ≤ max_durability ≤ base_max_durability`.
- Invariant: exactly one of `vault_id` / `backpack_id` is non-null.
- Invariant: gear with `max_durability = 0` cannot be equipped until repaired (CANON D6).
- Invariant: socketed gem count never exceeds `socket_count`; `red` gear never has socketed gems.
- On death: `blue` gear returns to the Hub (it remains vault-owned) with the −10% max-durability penalty; `red` gear rows still in a backpack are deleted with the backpack. On abandonment the backpack is also deleted but no durability penalty applies (CANON §B, Disconnect handling).
- Extracted `red` gear keeps `insurance: red`, so if carried on a later run it is uninsured and lost again on death.

### `Gem`

A permanent socketable ("Materia/Gem") crafted via Alchemy that slots into blue-chest gear.

**Source:** GDD.md §4.1 (Alchemy/Synthesis); CANON.md §G (Gem)

| Field | Type | Required | Nullable | Default | Since | Deprecated | Description |
|-------|------|----------|----------|---------|-------|------------|-------------|
| id | string (uuid) | Yes | No | — | v0.1 | No | The unique gem identifier. Server-assigned UUIDv7. |
| name | string | Yes | No | — | v0.1 | No | The display name. Content-defined. |
| owner_player_id | string (uuid) | Yes | No | — | v0.1 | No | The owning player. Gems are always player-owned from the moment of crafting. |
| stats | object | Yes | No | `{}` | v0.1 | No | Map of stat-modifier names to values granted while the gem is socketed in equipped gear. Stat names are content-defined. |
| gear_item_id | string (uuid) | No | Yes | null | v0.1 | No | The `blue` gear item this gem is socketed into. `null` when unsocketed. |
| vault_id | string (uuid) | No | Yes | null | v0.1 | No | The vault holding this gem while unsocketed. `null` while socketed. |
| crafted_at | string (date-time) | Yes | No | — | v0.1 | No | Timestamp of crafting. Server-assigned. |

**Relationships**

- Belongs to one `Player` via `owner_player_id`.
- Socketed into at most one `GearItem` via `gear_item_id`.

**Notes**

- Invariant: exactly one of `gear_item_id` / `vault_id` is non-null.
- Invariant: `gear_item_id` may only reference gear with `insurance: blue` (CANON §G).
- Gems are permanent: they survive death along with the blue gear that carries them.
- Crafting is gated by the crafter's Alchemy level; recipe/level requirements are content-defined.

### `ConsumableItem`

A stackable consumable usable during a run: healing potions and extraction escape items.

**Source:** GDD.md §2.2, §6 (Real-Time Influence); CANON.md §D (D15)

| Field | Type | Required | Nullable | Default | Since | Deprecated | Description |
|-------|------|----------|----------|---------|-------|------------|-------------|
| id | string (uuid) | Yes | No | — | v0.1 | No | The unique stack identifier. Server-assigned UUIDv7. |
| consumable_kind | string (enum: health_potion, ripcord_scroll) | Yes | No | — | v0.1 | No | The consumable type. `health_potion` restores HP and can be dropped onto an ally's active battle sprite to heal them mid-battle. `ripcord_scroll` extracts the user from anywhere after a 10-second interruptible channel [TUNABLE]. The enum is content-extensible. |
| quantity | integer (int32, ≥ 1) | Yes | No | `1` | v0.1 | No | The stack size. A stack at quantity 0 is deleted. |
| vault_id | string (uuid) | No | Yes | null | v0.1 | No | The vault holding this stack. `null` while carried in a run backpack. |
| backpack_id | string (uuid) | No | Yes | null | v0.1 | No | The run backpack holding this stack. `null` while banked in the vault. |

**Relationships**

- Belongs to at most one `Vault` via `vault_id` or one `Backpack` via `backpack_id` (mutually exclusive).

**Notes**

- Invariant: exactly one of `vault_id` / `backpack_id` is non-null.
- Naming: CANON §G does not name a generic consumable model; `ConsumableItem` is the spec-assigned model name. The kind values `health_potion` and `ripcord_scroll` come from GDD §6 and CANON D15 respectively.
- Backpack-held consumables can be freely dropped onto the overworld map for other players to pick up (GDD §6).

### `WardItem`

A deployable consumable that protects a sleeping avatar by hiding it from monster pathfinding.

**Source:** GDD.md §5 (Protective Items); CANON.md §G (Ward), §B (Disconnect handling)

| Field | Type | Required | Nullable | Default | Since | Deprecated | Description |
|-------|------|----------|----------|---------|-------|------------|-------------|
| id | string (uuid) | Yes | No | — | v0.1 | No | The unique ward identifier. Server-assigned UUIDv7. |
| ward_kind | string (enum: warding_tent, sanctuary_campfire) | Yes | No | — | v0.1 | No | The ward type. `warding_tent`: 30 minutes of invisibility to monster pathfinding [TUNABLE]. `sanctuary_campfire`: 10 minutes of invisibility plus a slow HP-regeneration aura [TUNABLE]. |
| vault_id | string (uuid) | No | Yes | null | v0.1 | No | The vault holding this ward while undeployed. |
| backpack_id | string (uuid) | No | Yes | null | v0.1 | No | The run backpack holding this ward while undeployed. |
| deployed_at | string (date-time) | No | Yes | null | v0.1 | No | Timestamp of deployment on the overworld. `null` while undeployed. |
| expires_at | string (date-time) | No | Yes | null | v0.1 | No | Timestamp when the ward's protection ends (`deployed_at` plus the kind's duration). `null` while undeployed. |
| protected_player_id | string (uuid) | No | Yes | null | v0.1 | No | The sleeping avatar's player covered by this ward. `null` while undeployed. |

**Relationships**

- Belongs to at most one `Vault` or one `Backpack` while undeployed; belongs to a `MazeInstance`'s overworld while deployed.

**Notes**

- Invariant: while undeployed, exactly one of `vault_id` / `backpack_id` is non-null and the deployment fields are all `null`; while deployed, all three deployment fields are non-null and both location references are `null`.
- Wards are consumed by deployment: an expired ward row is deleted, not returned to inventory.
- A warded sleeping avatar is invisible to monster pathfinding until `expires_at` (CANON §B, Disconnect handling).

### `Material`

A raw crafting material with a tier derived from the distance band where it spawns.

**Source:** GDD.md §2.2, §4 (Resource Stratification), §7 (Bounty Board); CANON.md §B (Distance → difficulty)

| Field | Type | Required | Nullable | Default | Since | Deprecated | Description |
|-------|------|----------|----------|---------|-------|------------|-------------|
| id | string (uuid) | Yes | No | — | v0.1 | No | The unique stack identifier. Server-assigned UUIDv7. |
| name | string | Yes | No | — | v0.1 | No | The material name (e.g. "Iron Ore"). Content-defined; also the matching key for bounty contracts. |
| tier | integer (int32, ≥ 0) | Yes | No | — | v0.1 | No | The material tier, equal to the loot tier band `floor(distance / 100)` where the material spawns. |
| quantity | integer (int32, ≥ 1) | Yes | No | `1` | v0.1 | No | The stack size. A stack at quantity 0 is deleted. |
| vault_id | string (uuid) | No | Yes | null | v0.1 | No | The vault holding this stack. `null` while carried in a run backpack. |
| backpack_id | string (uuid) | No | Yes | null | v0.1 | No | The run backpack holding this stack. `null` while banked in the vault. |

**Relationships**

- Belongs to at most one `Vault` via `vault_id` or one `Backpack` via `backpack_id` (mutually exclusive).

**Notes**

- Invariant: exactly one of `vault_id` / `backpack_id` is non-null.
- Resource stratification: low-tier materials (base crafting components) only spawn near the Center Hub, while high-tier hubs drop rare materials — deliberately keeping every distance band economically relevant (GDD §4).
- Materials are the inputs for Forging (crafting/repair) and the currency of bounty `Contract` fulfillment.
- Naming: CANON §G does not list a material model; `Material` is the spec-assigned model name for GDD's "raw crafting materials".
