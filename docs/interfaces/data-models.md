# Data Models

Canonical data types for MELDWORLD, derived forward from the design documents (`GDD.md`, `CANON.md`; CANON wins on conflict). No code exists yet — these models are the contract implementing agents build to. Models split into two persistence classes (CANON §D, D12): **persistent** state (accounts, vaults, gear, economy, leaderboards) lives in a single logical relational DB and is mutated only through the HTTP API or by the server at run end; **ephemeral** state (instances, runs, backpacks, battles, avatar state) lives in server memory with periodic snapshots and flows over the realtime protocol. Anything that survives logout is persistent.

**Source:** GDD.md §1, §2; CANON.md §D (D12), §G, §I, §S

## Conventions

- All entity IDs are UUIDv7 strings (`string (uuid)`), server-generated (CANON §I).
- Timestamps are ISO 8601 UTC (`string (date-time)`) on the HTTP surface; `u64` unix milliseconds on the realtime protocol (CANON §I).
- Wire/DB field names are `snake_case`; model names are PascalCase (CANON §G).
- Chits is a 64-bit integer everywhere; there is no fractional chits (CANON §D, D10).
- `distance` is Euclidean distance from the world origin (Center Hub) in tile units, floored to an integer for every threshold check (CANON §G).
- Constants marked **[TUNABLE]** in descriptions are design defaults that must live in server config, not be hardcoded (CANON preamble).

**Source:** CANON.md §I, §G, §D (D10)

## Models

### Player & Progression

Persistent account identity, currency vault, non-combat Meld Skills, class unlocks, and cosmetics.

| Model | Summary | Detail |
|-------|---------|--------|
| `Player` | Persistent account; owns Vault, Meld Skills, class unlocks, cosmetics | [player-models.md](data-models/player-models.md) |
| `Vault` | Per-player permanent storage: chits, materials, blue-chest gear, gems | [player-models.md](data-models/player-models.md) |
| `MeldSkill` | Persistent non-combat skill (`forging`, `mercantile`, `alchemy`), levels 1–99 | [player-models.md](data-models/player-models.md) |
| `ClassEmblem` | Account-level character-class unlock dropped by Gatekeeper Bosses | [player-models.md](data-models/player-models.md) |
| `CosmeticTitle` | Seasonal cosmetic title granted to top Vanguard Board finishers | [player-models.md](data-models/player-models.md) |
| `PrestigeAura` | Cosmetic aura item dropped in the infinite zone (distance > 5000) | [player-models.md](data-models/player-models.md) |

### Gear & Items

Equipment, socketables, consumables, and raw crafting materials.

| Model | Summary | Detail |
|-------|---------|--------|
| `GearItem` | Equipment with `blue`/`red` insurance, durability, and gem sockets | [gear-item-models.md](data-models/gear-item-models.md) |
| `Gem` | Permanent socketable crafted via Alchemy; slots into blue-chest gear | [gear-item-models.md](data-models/gear-item-models.md) |
| `ConsumableItem` | Stackable consumables: potions and escape items | [gear-item-models.md](data-models/gear-item-models.md) |
| `WardItem` | Deployable ward protecting a sleeping avatar (`warding_tent`, `sanctuary_campfire`) | [gear-item-models.md](data-models/gear-item-models.md) |
| `Material` | Raw crafting material with a distance-derived tier | [gear-item-models.md](data-models/gear-item-models.md) |

### Runs & Maze

Ephemeral excursion state: the shared maze world, the party inside it, per-player runs and backpacks, avatar presence, and streamed world chunks.

| Model | Summary | Detail |
|-------|---------|--------|
| `MazeInstance` | The 1–4 player shared maze world with its own seed | [run-models.md](data-models/run-models.md) |
| `Party` | The 1–4 players inside one MazeInstance | [run-models.md](data-models/run-models.md) |
| `Run` | One player's maze excursion; ends `extracted`, `died`, or `abandoned` | [run-models.md](data-models/run-models.md) |
| `Backpack` | Per-player ephemeral run inventory; deleted on death, banked on extraction | [run-models.md](data-models/run-models.md) |
| `AvatarState` | Realtime overworld avatar state, including `sleeping` | [run-models.md](data-models/run-models.md) |
| `Chunk` | Server-streamed 64×64-tile overworld region descriptor | [run-models.md](data-models/run-models.md) |

### Combat

Server-authoritative ATB battle state and monster definitions.

| Model | Summary | Detail |
|-------|---------|--------|
| `Battle` | One active ATB subscreen encounter (`standard`/`elite`/`gatekeeper`) | [combat-models.md](data-models/combat-models.md) |
| `BattleCombatant` | Per-combatant battle entry with ATB gauge state | [combat-models.md](data-models/combat-models.md) |
| `MonsterDefinition` | Content-defined monster archetype scaled by distance at spawn | [combat-models.md](data-models/combat-models.md) |
| `GatekeeperBoss` | Biome-border boss definition; drops class emblems | [combat-models.md](data-models/combat-models.md) |

### Economy

Player-driven market: stalls, listings, bounty contracts with escrow, and the chits ledger.

| Model | Summary | Detail |
|-------|---------|--------|
| `Stall` | Player shop deployed in a hub; persists while owner offline | [economy-models.md](data-models/economy-models.md) |
| `StallListing` | One item offered for sale at a stall | [economy-models.md](data-models/economy-models.md) |
| `Contract` | Bounty-board gathering order with chits escrow and 7-day expiry | [economy-models.md](data-models/economy-models.md) |
| `LedgerEntry` | Immutable record of every chits movement | [economy-models.md](data-models/economy-models.md) |

### World & Seasons

Persistent world structure and the seasonal leaderboard.

| Model | Summary | Detail |
|-------|---------|--------|
| `Hub` | Persistent safe zone keyed by distance (`center` or `outer`) | [world-models.md](data-models/world-models.md) |
| `BiomeBand` | Curated distance band mapping to a biome theme | [world-models.md](data-models/world-models.md) |
| `Season` | 13-week leaderboard epoch | [world-models.md](data-models/world-models.md) |
| `VanguardBoardEntry` | One instance's best-distance record on the seasonal Vanguard Board | [world-models.md](data-models/world-models.md) |
