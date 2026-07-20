# Player & Progression Models

> Parent: [interfaces/data-models](../data-models.md)

Persistent account-scoped state: the `Player` account, its `Vault`, the three `MeldSkill` tracks, `ClassEmblem` class unlocks, and the two cosmetic reward types (`CosmeticTitle`, `PrestigeAura`). Everything in this file survives death, logout, and seasonal wipes (CANON §B, Sessions & seasons).

## Models

### `Player`

The persistent account. Owns exactly one Vault, three Meld Skills, its class unlocks, and its cosmetics.

**Source:** GDD.md §2.1, §4; CANON.md §G (Player), §D (D9, D17, D18)

| Field | Type | Required | Nullable | Default | Since | Deprecated | Description |
|-------|------|----------|----------|---------|-------|------------|-------------|
| id | string (uuid) | Yes | No | — | v0.1 | No | The unique account identifier. Server-assigned UUIDv7 on account creation. |
| username | string (3–20 chars, pattern: `^[a-zA-Z0-9_]+$`) | Yes | No | — | v0.1 | No | The unique account name (CANON D17): the login identifier and the player-visible name shown in hubs, battles, and leaderboards. |
| password_hash | string | Yes | No | — | v0.1 | No | bcrypt hash (cost 12 **[TUNABLE]**) of the account password, stored in Postgres (CANON D17, D18). Internal storage only — never serialized into any API response, realtime message, or log. |
| created_at | string (date-time) | Yes | No | — | v0.1 | No | Timestamp of account creation. Server-assigned. |
| unlocked_classes | array of string (enum: hunter, dragoon, sage, ranger, alchemist_knight, bard) | Yes | No | `["hunter"]` | v0.1 | No | Character classes available to the account. `hunter` is always present; every other class is added by acquiring the matching `ClassEmblem`. The launch set is a content placeholder and may be extended (CANON D9). |
| active_title_id | string (uuid) | No | Yes | null | v0.1 | No | The `CosmeticTitle` currently displayed in the Hub. `null` when no title is equipped. |
| active_aura_id | string (uuid) | No | Yes | null | v0.1 | No | The `PrestigeAura` currently displayed on the avatar. `null` when no aura is equipped. |

**Relationships**

- Has exactly one `Vault` via `Vault.player_id`.
- Has exactly three `MeldSkill` records via `MeldSkill.player_id` (one per `skill_kind`).
- Has many `ClassEmblem`, `CosmeticTitle`, and `PrestigeAura` records via `player_id`.
- Has at most one active `Run` at a time via `Run.player_id`.

**Notes**

- Invariant: `username` is unique across all accounts and immutable at v0.1.
- Invariant: `password_hash` is never returned by any endpoint or message; the plaintext password is never persisted or logged (CANON D17). There is no email, OAuth, or 2FA at v0.1.
- Invariant: `unlocked_classes` always contains `hunter` and contains no duplicates.
- Invariant: `active_title_id` / `active_aura_id`, when non-null, must reference a cosmetic owned by this player.
- Combat stats are deliberately absent: combat level (`run_level`) is ephemeral per-run state, never account state (GDD §2.2, §4.1).

### `Vault`

Per-player permanent storage for chits, extracted materials, blue-chest gear, gems, and consumables.

**Source:** GDD.md §2.1; CANON.md §G (Vault), §D (D10), §I (error codes)

| Field | Type | Required | Nullable | Default | Since | Deprecated | Description |
|-------|------|----------|----------|---------|-------|------------|-------------|
| id | string (uuid) | Yes | No | — | v0.1 | No | The unique vault identifier. Server-assigned UUIDv7. |
| player_id | string (uuid) | Yes | No | — | v0.1 | No | The owning player. Exactly one vault exists per player. |
| chits | integer (int64, ≥ 0) | Yes | No | `0` | v0.1 | No | The permanent chits balance in whole units; fractional chits does not exist. |

**Relationships**

- Belongs to one `Player` via `player_id`.
- Holds many `GearItem`, `Gem`, `ConsumableItem`, `WardItem`, and `Material` records via their `vault_id`.

**Notes**

- Invariant: `chits` is never negative. Any operation that would overdraw the balance is rejected atomically with the `insufficient_funds` error (HTTP 409) and no partial state change.
- Vault contents are never wiped at season end (CANON §B, Sessions & seasons — structural).
- Item contents are modeled as rows on the item models (via `vault_id`), not as arrays on the vault.

### `MeldSkill`

A persistent non-combat skill track that levels up exclusively in hubs or through extraction success.

**Source:** GDD.md §4.1; CANON.md §G (Meld Skill), §B (Death & durability — repair; Economy)

| Field | Type | Required | Nullable | Default | Since | Deprecated | Description |
|-------|------|----------|----------|---------|-------|------------|-------------|
| id | string (uuid) | Yes | No | — | v0.1 | No | The unique skill-record identifier. Server-assigned UUIDv7. |
| player_id | string (uuid) | Yes | No | — | v0.1 | No | The owning player. |
| skill_kind | string (enum: forging, mercantile, alchemy) | Yes | No | — | v0.1 | No | The Meld Skill track. `forging` levels by combining extracted raw materials; `mercantile` by completing contracts and stall sales; `alchemy` by extracting rare plants and monster parts. |
| level | integer (int32, 1–99) | Yes | No | `1` | v0.1 | No | The current skill level. Levels range 1–99. |
| xp | integer (int64, ≥ 0) | Yes | No | `0` | v0.1 | No | Accumulated experience toward the next level. The per-level XP curve is not defined in CANON and is a server-config balance value [TUNABLE]. |

**Relationships**

- Belongs to one `Player` via `player_id`.

**Notes**

- Invariant: (`player_id`, `skill_kind`) is unique — exactly one record per track per player; all three are created with the account at level 1.
- Level effects (all [TUNABLE] unless noted, defined in CANON §B):
  - `forging` L: repair can restore `max_durability` up to `base_max_durability × (0.5 + L/198)` (L99 → 100%).
  - `mercantile` L: hub tax `10% − L × 0.05%`, floor 5%; stall slots `4 + floor(L/10) × 2`, max 24; stall placement in hubs `distance ≥ 1000` requires L ≥ 30, `distance ≥ 3000` requires L ≥ 60.
  - `alchemy` L: gates crafting of permanent `Gem` items.
- Meld Skills are never reduced by death and never wiped at season end.

### `ClassEmblem`

An account-level class-unlock item dropped by Gatekeeper Bosses (e.g. "Emblem of the Dragoon").

**Source:** GDD.md §4 (Gatekeeper Drops); CANON.md §G (Emblem, Gatekeeper), §D (D9)

| Field | Type | Required | Nullable | Default | Since | Deprecated | Description |
|-------|------|----------|----------|---------|-------|------------|-------------|
| id | string (uuid) | Yes | No | — | v0.1 | No | The unique emblem identifier. Server-assigned UUIDv7. |
| player_id | string (uuid) | Yes | No | — | v0.1 | No | The player whose account the unlock belongs to. |
| character_class | string (enum: dragoon, sage, ranger, alchemist_knight, bard) | Yes | No | — | v0.1 | No | The character class this emblem unlocks. `hunter` never appears — it is the default class, not a drop. |
| source_distance | integer (int64, ≥ 499) | Yes | No | — | v0.1 | No | The distance of the Gatekeeper arena that dropped the emblem. Always of the form `500k − 1`. |
| acquired_at | string (date-time) | Yes | No | — | v0.1 | No | Timestamp when the emblem was granted. Server-assigned. |

**Relationships**

- Belongs to one `Player` via `player_id`.

**Notes**

- Acquiring an emblem permanently adds `character_class` to `Player.unlocked_classes`. The unlock survives death and season end.
- Invariant: (`player_id`, `character_class`) is unique — a duplicate drop does not create a second record.

### `CosmeticTitle`

A cosmetic Hub title granted to members of top-ranked Vanguard Board instances when a season ends.

**Source:** GDD.md §8 (Seasonal Wipes); CANON.md §B (Sessions & seasons)

| Field | Type | Required | Nullable | Default | Since | Deprecated | Description |
|-------|------|----------|----------|---------|-------|------------|-------------|
| id | string (uuid) | Yes | No | — | v0.1 | No | The unique title identifier. Server-assigned UUIDv7. |
| player_id | string (uuid) | Yes | No | — | v0.1 | No | The player who earned the title. |
| label | string | Yes | No | — | v0.1 | No | The display text of the title. Content-defined per season. |
| season_id | string (uuid) | Yes | No | — | v0.1 | No | The season in which the title was earned. |
| granted_at | string (date-time) | Yes | No | — | v0.1 | No | Timestamp of the season-end grant. Server-assigned. |

**Relationships**

- Belongs to one `Player` via `player_id`; references one `Season` via `season_id`.

**Notes**

- Granted at season end to members of the top 100 instances on the Vanguard Board (CANON §B, Sessions & seasons).
- Purely cosmetic; no gameplay effect.

### `PrestigeAura`

A cosmetic aura item dropped by monsters in the infinite zone (distance > 5000) proving how far a player has pushed.

**Source:** GDD.md §8 (Infinite Scaling); CANON.md §B (Distance → difficulty)

| Field | Type | Required | Nullable | Default | Since | Deprecated | Description |
|-------|------|----------|----------|---------|-------|------------|-------------|
| id | string (uuid) | Yes | No | — | v0.1 | No | The unique aura identifier. Server-assigned UUIDv7. |
| player_id | string (uuid) | Yes | No | — | v0.1 | No | The player who owns the aura. |
| aura_kind | string | Yes | No | — | v0.1 | No | The visual aura variant. Content-defined; CANON does not enumerate variants. |
| drop_distance | integer (int64, > 5000) | Yes | No | — | v0.1 | No | The distance at which the aura dropped. Always beyond the final curated hub. |
| acquired_at | string (date-time) | Yes | No | — | v0.1 | No | Timestamp when the aura entered the account. Requires successful extraction of the run that found it. |

**Relationships**

- Belongs to one `Player` via `player_id`.

**Notes**

- Auras only drop past the final curated Outer Hub at distance 5000, where monster stats scale exponentially (CANON §B, structural).
- Purely cosmetic; no gameplay effect. Not wiped at season end.
