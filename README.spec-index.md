# MELDWORLD Spec

Forward-design behavior specifications derived from the source documents [`../GDD.md`](../GDD.md) and [`../CANON.md`](../CANON.md) (CANON wins on conflict). Execution plan: [`../BUILD-PLAN.md`](../BUILD-PLAN.md). Constants marked **[TUNABLE]** are server config, not hardcoded.

## Interfaces

### interfaces/data-models

Canonical data types split into persistent vs. ephemeral classes: player/progression models (Player, Vault, MeldSkill, ClassEmblem, cosmetics), gear & items (GearItem blue/red insurance, Gem, consumables, wards, materials), runs & maze (MazeInstance, Party, Run, Backpack, AvatarState, Chunk), combat (Battle, BattleCombatant, monsters, GatekeeperBoss), economy (Stall, StallListing, Contract, LedgerEntry), and world & seasons (Hub, BiomeBand, Season, VanguardBoardEntry).

→ [`interfaces/data-models.md`](interfaces/data-models.md) (index + 6 detail files)

### interfaces/http-api

Documents the full persistent-state HTTP API (base path `/v1`, opaque Bearer session-token auth per CANON D17, cursor pagination, canonical error envelope) across 6 resource groups: auth & players, vault & gear, crafting & meld skills, economy (stalls & bounty contracts), runs & world, and leaderboards & seasons — 45 endpoints total.

→ [`interfaces/http-api.md`](interfaces/http-api.md) (index + 6 detail files)

### interfaces/realtime-protocol

Documents the realtime WebSocket protocol (ephemeral state only): connection lifecycle and envelope/sequencing, session auth-handshake/heartbeat/resume, movement and chunk streaming, server-authoritative ATB battle sync, and run/social messages — 40 messages across 6 domains.

→ [`interfaces/realtime-protocol.md`](interfaces/realtime-protocol.md) (index + 4 detail files: session, movement-world, battle, run-social)

## Behaviors

### behaviors/world-generation

Deterministic seeded world generation per MazeInstance: radial distance model, tier/mlevel/stat_mult formulas, biome bands, chunk streaming, chokepoints, Gatekeeper arenas, extraction portal placement, loot banding, and infinite scaling past d=5000.

→ [`behaviors/world-generation.md`](behaviors/world-generation.md)

### behaviors/run-lifecycle

The extract-or-die state machine: run states active→extracted|died|abandoned, maze entry, extraction banking, death penalties, abandon semantics, instance close conditions, and persistence invariants.

→ [`behaviors/run-lifecycle.md`](behaviors/run-lifecycle.md)

### behaviors/combat-atb

Server-authoritative ATB battles: creation on touch, 100 ms tick gauge loop, action set and resolution ordering, flee, battle merge caps, external heal injection, outcomes, and XP/run-level-up.

→ [`behaviors/combat-atb.md`](behaviors/combat-atb.md)

### behaviors/disconnect-handling

Disconnect grace window, in-battle forced-flee/auto-defend rules, sleeping avatars and ward items, reconnect/resume, and the 60-minute all-disconnected auto-abandon.

→ [`behaviors/disconnect-handling.md`](behaviors/disconnect-handling.md)

### behaviors/meta-progression

Hub unlock flow (gatekeeper → ruined camp → rebuild → active outer hub), `base_run_level(hub)` formula and hub table, class unlocks via ClassEmblem drops, Training Ground build templates, resource stratification, and the three Meld Skills (forging/mercantile/alchemy 1–99) with their XP sources and level effects.

→ [`behaviors/meta-progression.md`](behaviors/meta-progression.md)

### behaviors/economy

Stall lifecycle (deploy/purchase/close, offline persistence, atomic taxed sales), bounty contracts (escrow, accept, verified fulfillment, 7-day auto-refund), the durability sink loop, and chits conservation invariants (source/sink/transfer tables).

→ [`behaviors/economy.md`](behaviors/economy.md)

### behaviors/async-interaction

Overworld backpack drops (visibility, first-come pickup, despawn), consumable injection into active battles (healing/curative only), ward deployment over sleeping allies, and anti-grief rules.

→ [`behaviors/async-interaction.md`](behaviors/async-interaction.md)

### behaviors/endgame-seasons

The Vanguard Board (max distance per instance per season, real-time, earliest-achievement tie-break), the infinite zone past d=5000 (exponential stat_mult, Prestige aura tiers), and the 13-week season lifecycle (archive, titles to top 100 instances, what is never wiped).

→ [`behaviors/endgame-seasons.md`](behaviors/endgame-seasons.md)

## Edge Cases

### edge-cases/limits

Consolidated table of every numeric limit, cap, timeout, and size — party/merge caps, inventory and stall slots, tax and durability formula bounds, timers (grace, channel, turn timeout, idle close, despawn, wards, contract expiry), world constants (chunk, interest radius, tick rates), season length, text-length limits, and rate limits — each row citing its GDD/CANON source or marked spec-defined.

→ [`edge-cases/limits.md`](edge-cases/limits.md)
