# Combat Models

> Parent: [interfaces/data-models](../data-models.md)

Server-authoritative ATB combat state: the `Battle` encounter, its `BattleCombatant` entries with ATB gauge state, content-defined `MonsterDefinition` archetypes, and `GatekeeperBoss` definitions. All ATB math (timers, damage, status) is computed server-side; clients render and submit intents only (CANON D11). Battle state is ephemeral and flows over the realtime protocol (CANON §S).

## Encounter Classes

Every battle has an encounter class that drives flee rules, disconnect handling, and merge capacity.

**Source:** GDD.md §5 (Disconnect & Sleep Mechanics); CANON.md §D (D5), §B (ATB combat; Disconnect handling)

| encounter_class | Flee | Disconnect behavior | Merge cap |
|-----------------|------|--------------------|-----------|
| `standard` | Base 60% success, −10% per tier the encounter is above the party's level tier, always ≥ 5% [TUNABLE] | Forced flee, always succeeds (structural) | Up to 2 instances (8 players) [TUNABLE] |
| `elite` | Same formula as `standard` [TUNABLE] | Auto-defend until the battle ends or the player reconnects | Up to 2 instances (8 players) [TUNABLE] — CANON D5 specifies caps only for "normal" and Gatekeeper encounters; elite is assumed to use the normal cap |
| `gatekeeper` | Disabled | Auto-defend until the battle ends or the player reconnects | Up to 4 instances (16 players) [TUNABLE] |

## Models

### `Battle`

One active ATB subscreen encounter — a server-side entity anchored to an overworld position.

**Source:** GDD.md §5, §6 (Real-Time Influence); CANON.md §G (Battle), §D (D5, D11), §B (ATB combat)

| Field | Type | Required | Nullable | Default | Since | Deprecated | Description |
|-------|------|----------|----------|---------|-------|------------|-------------|
| id | string (uuid) | Yes | No | — | v0.1 | No | The unique battle identifier. Server-assigned UUIDv7. |
| encounter_class | string (enum: standard, elite, gatekeeper) | Yes | No | — | v0.1 | No | The encounter class, fixed at battle start. Determines flee rules, disconnect behavior, and merge capacity (see Encounter Classes). |
| instance_ids | array of string (uuid) (1–4 items) | Yes | No | — | v0.1 | No | The MazeInstances whose parties are engaged. Grows when another party touches the same enemy (battle merge). Capped at 2 for `standard`/`elite`, 4 for `gatekeeper` [TUNABLE]. |
| position_x | number (double) | Yes | No | — | v0.1 | No | The battle's anchor X coordinate on the overworld, in tile units. Non-combat players can drop items (e.g. a health potion) onto this sprite to affect the battle in real time. |
| position_y | number (double) | Yes | No | — | v0.1 | No | The battle's anchor Y coordinate on the overworld, in tile units. |
| state | string (enum: active, ended) | Yes | No | `active` | v0.1 | No | The battle lifecycle state. |
| started_at | integer (int64) | Yes | No | — | v0.1 | No | Unix-millisecond timestamp of battle start. |
| ended_at | integer (int64) | No | Yes | null | v0.1 | No | Unix-millisecond timestamp of battle end. `null` while active. |

**Relationships**

- Has many `BattleCombatant` entries via `battle_id`.
- Referenced by `AvatarState.battle_id` for every player currently `in_battle`.

**Notes**

- Invariant: `instance_ids` never exceeds the merge cap for `encounter_class` and never shrinks while the battle is active.
- The server advances the battle on a 100 ms tick [TUNABLE]; enemies keep acting even if a player walks away from the screen (GDD §5).
- Battle merge: a joining party's combatants are inserted at ATB gauge 0; enemy stats do not rescale mid-fight (CANON §B).
- An item dropped by a non-combat player onto the battle's overworld sprite is applied server-side to the targeted combatant inside the subscreen instantly (GDD §6).

### `BattleCombatant`

One participant in a battle — a player avatar or a monster — carrying its ATB gauge state.

**Source:** GDD.md §5; CANON.md §D (D11), §B (ATB combat; Disconnect handling)

| Field | Type | Required | Nullable | Default | Since | Deprecated | Description |
|-------|------|----------|----------|---------|-------|------------|-------------|
| id | string (uuid) | Yes | No | — | v0.1 | No | The unique combatant-entry identifier. Server-assigned UUIDv7. |
| battle_id | string (uuid) | Yes | No | — | v0.1 | No | The battle this entry belongs to. |
| side | string (enum: party, enemy) | Yes | No | — | v0.1 | No | Which side of the encounter the combatant fights on. |
| kind | string (enum: player, monster) | Yes | No | — | v0.1 | No | Whether the combatant is a player avatar or a monster. |
| player_id | string (uuid) | No | Yes | null | v0.1 | No | The player, when `kind = player`. `null` for monsters. |
| monster_def_id | string | No | Yes | null | v0.1 | No | The monster archetype, when `kind = monster`. `null` for players. |
| level | integer (int32, ≥ 1) | Yes | No | — | v0.1 | No | The combatant's level: the player's current `run_level`, or `mlevel(d) = max(1, round(d / 12.5))` for a monster spawned at distance d [TUNABLE]. |
| hp | integer (int64, ≥ 0) | Yes | No | — | v0.1 | No | Current hit points. A combatant at 0 HP is defeated. |
| max_hp | integer (int64, ≥ 1) | Yes | No | — | v0.1 | No | Maximum hit points, fixed at battle entry. Enemy stats do not rescale mid-fight. |
| speed_stat | integer (int32, ≥ 1) | Yes | No | — | v0.1 | No | The speed stat driving ATB gauge fill. |
| atb_gauge | number (double, 0.0–1.0) | Yes | No | `0.0` | v0.1 | No | The ATB gauge. Fills by `speed_stat / 400` per 100 ms server tick [TUNABLE]; the combatant may act when it reaches 1.0, and it resets to 0.0 after acting. Combatants joining via battle merge start at 0.0. |
| gauge_full_at | integer (int64) | No | Yes | null | v0.1 | No | Unix-millisecond timestamp when the gauge reached full. A player combatant auto-defends after 15 seconds at full gauge without submitting an action [TUNABLE]. `null` while the gauge is filling. |
| auto_defend | boolean | Yes | No | `false` | v0.1 | No | Whether the combatant is in the auto-defend state, applied to disconnected players in `elite`/`gatekeeper` encounters (and on turn timeout) to prevent wiping a boss attempt. Cleared on reconnect or action. |
| statuses | array of string | Yes | No | `[]` | v0.1 | No | Active status effects. Effect names are content-defined; all status math is server-computed. |

**Relationships**

- Belongs to one `Battle` via `battle_id`; references one `Player` or one `MonsterDefinition`.

**Notes**

- Invariant: exactly one of `player_id` / `monster_def_id` is non-null, matching `kind`.
- Invariant: `0 ≤ hp ≤ max_hp` and `0.0 ≤ atb_gauge ≤ 1.0` at all times.
- On disconnect in a `standard` encounter, the disconnected party's combatant entries are removed via a forced, always-successful flee (structural); in `elite`/`gatekeeper` encounters the entries remain with `auto_defend = true` (CANON §B, Disconnect handling).

### `MonsterDefinition`

A content-defined monster archetype; runtime stats scale with spawn distance.

**Source:** GDD.md §3, §8 (Infinite Scaling); CANON.md §B (Distance → difficulty; Biome bands)

| Field | Type | Required | Nullable | Default | Since | Deprecated | Description |
|-------|------|----------|----------|---------|-------|------------|-------------|
| monster_def_id | string | Yes | No | — | v0.1 | No | The unique archetype key. Content-defined identifier, stable across instances. |
| name | string | Yes | No | — | v0.1 | No | The display name. Content-defined. |
| biomes | array of string (enum: forest, desert, ashfall, tundra, mire) | Yes | No | — | v0.1 | No | The biome bands this monster can spawn in. Content-extensible alongside the biome tables. |
| min_tier | integer (int32, ≥ 0) | Yes | No | `0` | v0.1 | No | The lowest tier band (`floor(distance / 100)`) at which this monster appears. |
| elite_capable | boolean | Yes | No | `false` | v0.1 | No | Whether this archetype can spawn as an `elite` encounter. |
| base_stats | object | Yes | No | — | v0.1 | No | Map of base stat names to values (including `speed_stat` and base HP) before distance scaling. Stat names are content-defined. |
| loot_table_id | string | Yes | No | — | v0.1 | No | The content-defined loot table rolled on defeat. Loot rarity weights shift one band per tier; past distance 5000 tables include Prestige aura drops. |

**Relationships**

- Referenced by `BattleCombatant.monster_def_id`.

**Notes**

- Runtime scaling at spawn distance d [TUNABLE unless noted]: level `mlevel(d) = max(1, round(d / 12.5))`; stats multiplied by `stat_mult(d) = (1 + d/500)^1.25` for d ≤ 5000, and `stat_mult(5000) × 1.5^((d − 5000)/500)` beyond (exponential endgame — structural).
- Roaming monsters can attack a `sleeping` avatar on contact unless it is warded (GDD §5).
- Monster definitions are static content, not per-player state; the definition set is content-team extensible.

### `GatekeeperBoss`

The massive, unavoidable boss guarding each biome border. Clearing it is the only way past its arena.

**Source:** GDD.md §3 (Gatekeeper Bosses), §4 (Gatekeeper Drops); CANON.md §G (Gatekeeper, Emblem), §D (D5), §B (Hubs & run levels; ATB combat)

| Field | Type | Required | Nullable | Default | Since | Deprecated | Description |
|-------|------|----------|----------|---------|-------|------------|-------------|
| distance | integer (int64, one of 499, 999, …, 4999) | Yes | No | — | v0.1 | No | The arena's distance, `500k − 1` for k = 1..10 (structural). Serves as the boss's unique key. |
| name | string | Yes | No | — | v0.1 | No | The display name (e.g. "Distance 500 Desert Gatekeeper"). Content-defined. |
| biome | string (enum: forest, desert, ashfall, tundra, mire) | Yes | No | — | v0.1 | No | The biome band whose border the boss guards. |
| emblem_class | string (enum: dragoon, sage, ranger, alchemist_knight, bard) | Yes | No | — | v0.1 | No | The character class whose `ClassEmblem` this boss drops. |
| base_hp | integer (int64, ≥ 1) | Yes | No | — | v0.1 | No | The HP pool at spawn, sized for 8 players [TUNABLE]. Does not rescale when additional parties merge in. |
| base_stats | object | Yes | No | — | v0.1 | No | Map of base stat names to values before distance scaling. Stat names are content-defined. |

**Relationships**

- Clear state is tracked per instance on `MazeInstance.cleared_gatekeeper_distances`.
- Drops `ClassEmblem` records on first-kill per player.

**Notes**

- Gatekeeper battles always have `encounter_class = gatekeeper`: flee is disabled, disconnects auto-defend, and up to 4 instances (16 players) may merge in [TUNABLE].
- The arena is a full-width chokepoint — no path exists past an uncleared arena (structural, CANON §B).
- Defeating the Gatekeeper at deep distances is what unlocks rebuilding the ruined camp beyond it as an Outer Hub (GDD §3).
- Tension noted from source: CANON D5 allows up to 16 players in a merged Gatekeeper battle while §B sizes Gatekeeper HP pools for 8 at spawn — over-merging is intentionally favorable to players.
