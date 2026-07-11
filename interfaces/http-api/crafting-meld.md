# Crafting & Meld Skill Endpoints

> Parent: [interfaces/http-api](../http-api.md)

Meld Skills are the persistent non-combat progression path (GDD.md §4.1): `forging` (crafting/repair), `mercantile` (stalls/contracts), and `alchemy` (gem synthesis). Combat stats wipe per run; meld skills never wipe — not even at season end (CANON.md §Sessions & seasons). This file covers reading skill levels/XP, listing recipes, forging items, and synthesizing gems.

## Shared object: Meld Skill

**Source:** GDD.md §4.1; CANON.md §G (`MeldSkill`)

| Field | Type | Nullable | Since | Deprecated | Description |
|-------|------|----------|-------|------------|-------------|
| skill_kind | string (enum: forging, mercantile, alchemy) | No | v0.1 | No | The meld skill (CANON.md §G — exactly these three). |
| level | integer (int32, 1–99) | No | v0.1 | No | Current level. Hard cap 99 (CANON.md §G). |
| xp | integer (int64) | No | v0.1 | No | XP accumulated toward the next level. Resets to the overflow remainder on level-up. |
| xp_to_next | integer (int64) | Yes | v0.1 | No | XP required to reach the next level. `null` at level 99. The meld XP curve is server-configured [TUNABLE] — CANON.md defines an XP curve only for run levels, not meld skills (flagged gap). |

**XP sources** (all applied server-side; never via client HTTP mutation):

- `forging`: successful crafts ([`POST /v1/crafting/craft`](#post-v1craftingcraft)) and repairs ([`POST /v1/vault/gear/{gear_id}/repair`](vault-gear.md#post-v1vaultgeargear_idrepair)).
- `mercantile`: stall sales completing ([buy listing](economy.md#post-v1stallsstall_idlistingslisting_idbuy), XP to seller) and contract fulfillment ([fulfill](economy.md#post-v1contractscontract_idfulfill), XP to fulfiller).
- `alchemy`: extracting rare plants/monster parts (run-end banking, server-applied) and gem synthesis ([`POST /v1/crafting/synthesize`](#post-v1craftingsynthesize)).

---

### GET /v1/meld-skills

Returns the authenticated player's three meld skills with level and XP.

**Source:** GDD.md §4.1; CANON.md §G (`MeldSkill`)
**Added:** v0.1
**Deprecated:** No
**Auth:** Bearer session token.
**Idempotency:** Safe (read-only).
**Side effects:** None.

**Request** — no parameters.

**Response** — `200 OK`

| Field | Type | Nullable | Since | Deprecated | Description |
|-------|------|----------|-------|------------|-------------|
| data | array of Meld Skill | No | v0.1 | No | Always exactly 3 entries, one per `skill_kind`, in the order `forging`, `mercantile`, `alchemy`. |

**Error responses** — no endpoint-specific errors beyond [Common Errors](../http-api.md#common-errors).

**Example — success**

```http
GET /v1/meld-skills
Authorization: Bearer mw-sess-0195c9a4-example-opaque-token
```

```json
HTTP/1.1 200 OK
{"data": [{"skill_kind": "forging", "level": 22, "xp": 4100, "xp_to_next": 5200}, {"skill_kind": "mercantile", "level": 31, "xp": 900, "xp_to_next": 8800}, {"skill_kind": "alchemy", "level": 8, "xp": 55, "xp_to_next": 640}]}
```

---

### GET /v1/recipes

Lists crafting and synthesis recipes from the content catalog, with the caller's eligibility.

**Source:** GDD.md §4.1 (Crafting/Forging, Alchemy/Synthesis); CANON.md §G (`Gem`, `GearItem`)
**Added:** v0.1
**Deprecated:** No
**Auth:** Bearer session token.
**Idempotency:** Safe (read-only).
**Side effects:** None.

**Request** — [pagination parameters](../http-api.md#pagination), plus:

| Field | Type | Required | Nullable | Default | Since | Deprecated | Description |
|-------|------|----------|----------|---------|-------|------------|-------------|
| skill_kind | string (enum: forging, alchemy), query | No | No | — | v0.1 | No | Filters to one crafting discipline. `mercantile` has no recipes; passing it returns 400 `validation_error`. When omitted, both disciplines are returned. |
| craftable | boolean, query | No | No | — | v0.1 | No | When `true`, only recipes the caller currently meets the level requirement for. When omitted or `false`, all recipes are returned. |

**Ordering:** By `required_level` ascending, then `recipe_id` ascending. Stable across pages.

**Response** — `200 OK` — paginated envelope.

| Field | Type | Nullable | Since | Deprecated | Description |
|-------|------|----------|-------|------------|-------------|
| data | array of object | No | v0.1 | No | Recipe entries. |
| data[].recipe_id | string | No | v0.1 | No | Content-catalog recipe key (e.g. `"iron_longsword"`). Stable across versions. |
| data[].name | string | No | v0.1 | No | Display name. |
| data[].skill_kind | string (enum: forging, alchemy) | No | v0.1 | No | Which meld skill crafts this recipe and gains XP from it. |
| data[].required_level | integer (int32, 1–99) | No | v0.1 | No | Minimum meld skill level to craft. Below it, craft attempts return 403 `forbidden`. |
| data[].inputs | array of object | No | v0.1 | No | Materials consumed per craft. |
| data[].inputs[].material_id | string | No | v0.1 | No | Content-catalog material key. |
| data[].inputs[].quantity | integer (int32, ≥ 1) | No | v0.1 | No | Amount consumed per craft. |
| data[].output_kind | string (enum: gear, material, gem) | No | v0.1 | No | What the recipe produces. `forging` recipes produce `gear` or `material` (intermediate components); `alchemy` recipes produce `gem`. |
| data[].output_name | string | No | v0.1 | No | Display name of the product. |
| next_cursor | string | Yes | v0.1 | No | Next page cursor; `null` on the last page. |

**Error responses**

| Status | Error code | Condition |
|--------|------------|-----------|
| 400 | `validation_error` | Unknown `skill_kind` value (including `mercantile`). |

**Example — mid-page response**

```http
GET /v1/recipes?skill_kind=forging&limit=2
Authorization: Bearer mw-sess-0195c9a4-example-opaque-token
```

```json
HTTP/1.1 200 OK
{"data": [{"recipe_id": "iron_ingot", "name": "Iron Ingot", "skill_kind": "forging", "required_level": 1, "inputs": [{"material_id": "iron_ore", "quantity": 5}], "output_kind": "material", "output_name": "Iron Ingot"}, {"recipe_id": "iron_longsword", "name": "Iron Longsword", "skill_kind": "forging", "required_level": 4, "inputs": [{"material_id": "iron_ingot", "quantity": 3}, {"material_id": "oak_haft", "quantity": 1}], "output_kind": "gear", "output_name": "Iron Longsword"}], "next_cursor": "eyJyIjoiaXJvbl9sb25nc3dvcmQifQ=="}
```

**Example — last page**

```json
HTTP/1.1 200 OK
{"data": [{"recipe_id": "ember_materia", "name": "Ember Materia", "skill_kind": "alchemy", "required_level": 35, "inputs": [{"material_id": "cinder_bloom", "quantity": 12}, {"material_id": "wyrm_gland", "quantity": 2}], "output_kind": "gem", "output_name": "Ember Materia"}], "next_cursor": null}
```

**Notes**

- Low-tier materials only spawn near the Center Hub by design (Resource Stratification, GDD.md §4) — high-level recipes intentionally include low-tier inputs.

---

### POST /v1/crafting/craft

Crafts a Forging recipe: atomically consumes the input materials from the caller's Vault, deposits the product (gear item or material stack) into the Vault, and grants Forging XP. Crafted gear rolls stat variance influenced by the crafter's Forging level (GDD.md §4.1); the roll is server-side and returned in the response.

**Source:** GDD.md §4.1 (Crafting/Forging); CANON.md §G (`GearItem`), §D14 (atomic server-side execution)
**Added:** v0.1
**Deprecated:** No
**Auth:** Bearer session token.
**Idempotency:** Non-idempotent — each successful call consumes materials and produces a new item. There is no idempotency key in v0.1; after an ambiguous failure, re-check Vault materials before retrying (a duplicate retry that lacks materials fails safely with 409 `conflict`).
**Concurrency:** Material consumption is transactional; two concurrent crafts contending for the same stack succeed only as far as materials allow — the loser receives 409 `conflict`.
**Side effects:** Debits input materials; creates a gear item (with `insurance: blue`, freshly rolled `stats`, `max_durability = base_max_durability`) or credits a material stack; grants Forging XP (amount server-configured per recipe [TUNABLE]); may raise Forging level.

**Request**

| Field | Type | Required | Nullable | Default | Since | Deprecated | Description |
|-------|------|----------|----------|---------|-------|------------|-------------|
| recipe_id | string | Yes | No | — | v0.1 | No | The Forging recipe to craft. Alchemy recipes return 409 `conflict` (wrong discipline — use [synthesize](#post-v1craftingsynthesize)). |
| count | integer (int32, 1–20) | No | No | `1` | v0.1 | No | Number of crafts to perform in one atomic batch. All-or-nothing: if materials cover only part of the batch, the whole request fails with 409 `conflict`. Only valid for `material`-output recipes; gear recipes require `count = 1`. |

**Response** — `201 Created`

| Field | Type | Nullable | Since | Deprecated | Description |
|-------|------|----------|-------|------------|-------------|
| output_kind | string (enum: gear, material) | No | v0.1 | No | What was produced. |
| gear | GearItem | Yes | v0.1 | No | The crafted gear item (see [vault-gear.md](vault-gear.md#shared-object-gearitem)). `null` when `output_kind` is `material`. |
| material | object | Yes | v0.1 | No | The credited stack: `{material_id, name, tier, quantity}` reflecting the **new total** in the Vault. `null` when `output_kind` is `gear`. |
| forging_xp_gained | integer (int64) | No | v0.1 | No | Forging XP granted by this craft. |
| forging | Meld Skill | No | v0.1 | No | The caller's Forging skill after the craft (shows level-ups). |

**Error responses**

| Status | Error code | Condition |
|--------|------------|-----------|
| 400 | `validation_error` | Unknown `recipe_id`; `count` out of range or > 1 on a gear recipe. |
| 403 | `forbidden` | Caller's Forging level is below the recipe's `required_level`. |
| 409 | `conflict` | Insufficient input materials in the Vault; or `recipe_id` is an Alchemy recipe. |

**Example — success**

```http
POST /v1/crafting/craft
Authorization: Bearer mw-sess-0195c9a4-example-opaque-token
Content-Type: application/json

{"recipe_id": "iron_longsword"}
```

```json
HTTP/1.1 201 Created
{"output_kind": "gear", "gear": {"gear_id": "0195d200-aaaa-7abc-8f01-23456789abcd", "name": "Iron Longsword", "slot": "weapon", "insurance": "blue", "tier": 0, "stats": {"attack": 14, "speed_stat": 6}, "base_max_durability": 80, "max_durability": 80, "socket_count": 1, "sockets": [{"socket_index": 0, "gem_id": null}], "equipped": false, "created_at": "2026-07-11T12:00:00Z"}, "material": null, "forging_xp_gained": 120, "forging": {"skill_kind": "forging", "level": 22, "xp": 4220, "xp_to_next": 5200}}
```

**Example — error**

```json
HTTP/1.1 409 Conflict
{"error": {"code": "conflict", "message": "Insufficient materials: need 3x iron_ingot, have 1.", "request_id": "0195d201-6666-7f3a-9d2c-4e5f6a7b8c9d"}}
```

**Notes**

- Crafting is a Hub activity conceptually (GDD.md §4.1), but v0.1 does not enforce hub presence on this endpoint — the Vault is the only party to the transaction. Flagged as a design question.
- Crafts never fail randomly in v0.1; "successfully combining" (GDD.md §4.1) is interpreted as deterministic success with level-gated access and level-influenced stat variance.

---

### POST /v1/crafting/synthesize

Synthesizes an Alchemy recipe into a permanent Gem: atomically consumes input materials, deposits the gem loose in the Vault, and grants Alchemy XP.

**Source:** GDD.md §4.1 (Alchemy/Synthesis, "permanent 'Materia/Gems'"); CANON.md §G (`Gem`), §D14
**Added:** v0.1
**Deprecated:** No
**Auth:** Bearer session token.
**Idempotency:** Non-idempotent — each successful call consumes materials and creates a new gem. Same retry guidance as craft.
**Concurrency:** Same transactional material consumption as craft.
**Side effects:** Debits input materials; creates a Gem (`socketed_in: null`); grants Alchemy XP (server-configured per recipe [TUNABLE]); may raise Alchemy level.

**Request**

| Field | Type | Required | Nullable | Default | Since | Deprecated | Description |
|-------|------|----------|----------|---------|-------|------------|-------------|
| recipe_id | string | Yes | No | — | v0.1 | No | The Alchemy recipe to synthesize. Forging recipes return 409 `conflict` (wrong discipline — use [craft](#post-v1craftingcraft)). |

**Response** — `201 Created`

| Field | Type | Nullable | Since | Deprecated | Description |
|-------|------|----------|-------|------------|-------------|
| gem | Gem | No | v0.1 | No | The synthesized gem (see [vault-gear.md](vault-gear.md#shared-object-gem)). |
| alchemy_xp_gained | integer (int64) | No | v0.1 | No | Alchemy XP granted. |
| alchemy | Meld Skill | No | v0.1 | No | The caller's Alchemy skill after synthesis. |

**Error responses**

| Status | Error code | Condition |
|--------|------------|-----------|
| 400 | `validation_error` | Unknown `recipe_id`. |
| 403 | `forbidden` | Caller's Alchemy level is below the recipe's `required_level`. |
| 409 | `conflict` | Insufficient input materials; or `recipe_id` is a Forging recipe. |

**Example — success**

```http
POST /v1/crafting/synthesize
Authorization: Bearer mw-sess-0195c9a4-example-opaque-token
Content-Type: application/json

{"recipe_id": "ember_materia"}
```

```json
HTTP/1.1 201 Created
{"gem": {"gem_id": "0195d202-bbbb-7abc-8f01-23456789abcd", "name": "Ember Materia", "effect": {"fire_damage": 8}, "socketed_in": null, "created_at": "2026-07-11T12:05:00Z"}, "alchemy_xp_gained": 300, "alchemy": {"skill_kind": "alchemy", "level": 35, "xp": 300, "xp_to_next": 11400}}
```

**Example — error**

```json
HTTP/1.1 403 Forbidden
{"error": {"code": "forbidden", "message": "Alchemy level 8 is below the required level 35 for recipe 'ember_materia'.", "request_id": "0195d203-7777-7f3a-9d2c-4e5f6a7b8c9d"}}
```
