# MELDWORLD Canon

Authoritative resolutions of every gap, ambiguity, and naming decision in `GDD.md`. All spec files and all implementing agents MUST use the names, enums, formulas, and constants defined here. On conflict with GDD.md, CANON.md wins. Constants marked **[TUNABLE]** are design defaults intended for balance iteration; they must be implemented as server config, not hardcoded.

## §D. Decisions & Assumptions (gaps resolved)

| # | GDD gap | Resolution |
|---|---------|------------|
| D1 | §3 radial scaling formula missing from source doc | Defined in §Balance below. |
| D2 | GDD mentions "Flame canvas/map" in §5–6 but the stack (§1) says Bevy | The overworld is **Bevy**. All "Flame" references read as "overworld". |
| D3 | §8 "highest ___ achieved" (blank) | The Vanguard Board ranks the highest **distance** reached by an instance during a single run. |
| D4 | Run Level for hubs other than Center (RL1) and D500 (RL40) unspecified | `base_run_level(hub)` formula in §Balance. |
| D5 | Party size vs. raid merge cap unspecified | Instance = up to 4 players. A merged battle holds up to 2 instances (8 combatants) for normal encounters; up to 4 instances (16) for Gatekeepers. **[TUNABLE]** |
| D6 | Durability loss on death unspecified | −10% of current max durability per death, floor 0. Gear at 0 max durability is unequippable until repaired. **[TUNABLE]** |
| D7 | Hub tax unspecified | 10% sales tax on stall sales and contract payouts, reduced by Mercantile level (§Balance). **[TUNABLE]** |
| D8 | Season length "e.g. 3 months" | 13 weeks exactly, rolling UTC boundary. |
| D9 | Character classes | Launch set: Explorer (default), Dragoon, Sage, Ranger, Alchemist-Knight, Bard. Classes beyond Explorer are Gatekeeper drops. Placeholder — content team may extend. |
| D10 | Currency granularity | Currency is **Chits** (`chits`, symbol `c`), a 64-bit integer; no fractional chits. Every GDD "Gold"/"G" reference reads as Chits. |
| D11 | Combat is server-authoritative | All ATB math (timers, damage, status) computed server-side; clients render and submit intents only. |
| D12 | Persistence store | Single logical relational DB for persistent state; ephemeral run/instance state lives in server memory with periodic snapshots for crash recovery. Specs describe observable behavior only. |
| D13 | Matchmaking | Party of 1–4 formed in a Hub; solo players may opt into matchmaking pool filtered by departure hub. An instance is created at maze-entry time and is not joinable afterward except via battle merge (which merges battles, not instances). |
| D14 | Offline stall fulfillment | Stalls and bounty contracts execute server-side atomically; escrow model (§Economy semantics in behaviors specs). |
| D15 | Extraction portal spawning | Extraction portals spawn deterministically at every Hub (including Center) plus procedurally at ~1 per 200-distance band per instance seed. Escape items ("Ripcord Scroll") extract from anywhere with a 10 s interruptible channel. **[TUNABLE]** |
| D16 | UI framework pivot | **No Flutter.** All UI — ATB battle screens, hub UIs (Vault, Training Ground, Stall shop, Bounty Board, leaderboards), menus — is built in Bevy (bevy_ui/ecosystem UI crates). Any GDD/spec reference to "Flutter UI" reads as "Bevy UI layer". Art direction: indie-style HD-2D (pixel sprites/tiles with 3D lighting, DoF, particles). |
| D17 | Auth mechanism | Registration/login is username + password only. Usernames unique, 3–20 chars `^[a-zA-Z0-9_]+$`. Passwords 8–128 chars, stored as bcrypt hashes (cost 12 **[TUNABLE]**) in Postgres; plaintext never persisted or logged. Successful login issues a short-lived session token (Bearer, 24 h expiry **[TUNABLE]**) for HTTP plus a single-use realtime session ticket. No OAuth/email/2FA at v0.1. |
| D18 | Persistence engine | Persistent state lives in **Postgres** (explicit implementation mandate; specs still describe observable behavior, with storage noted only where the mandate requires it, e.g. bcrypt credential storage). |
| D19 | World model — persistent, player-seeded shard | The target overworld is a **persistent, player-seeded World** (à la Minecraft): worldgen is deterministic from the seed; many **capped** worlds exist and a full one queues (no auto-fork, since worlds hold unique player structures). This **supersedes "`MazeInstance` is ephemeral, discarded on close"** (§G, D13) *as the target*; the current single-shared-instance build is the precursor. Detail: §W. Server plan: [`proposals/server-scaling.md`](proposals/server-scaling.md). |
| D20 | The Shift | Regions of the overworld periodically **Shift** — swap biome, dealing Force damage to and wiping the entities inside. The scheduler is **deterministic** from `(world_seed, shift_generation)`, driven by the server tick counter, **never wall-clock** (preserves the deterministic-engine invariant and makes persistence a cheap replay). Graduates [`lore/shifting-lands.md`](lore/shifting-lands.md) → canon. Cadence/size/damage **[TUNABLE]**. Detail: §W2. |
| D21 | Structures & anchors — one primitive | Player-built world objects are **one `Structure` primitive** (HP-bearing, destructible, siege-able) distinguished by a `function` tag: `anchor` (pins its region against the Shift while defended), `portal` (extraction; the plantable/defendable evolution of D15), `wall` (defense), `stash` (field storage). Costs/HP/radii **[TUNABLE]**. Detail: §W3. |
| D22 | Run Level reset on return | **Full extraction to the Last City — or death — resets Run Level to 1.** Run Level is built by pushing outward and **persists across forward-town stops within a single push**; it resets only on return to the Last City or death. Refines D4 (`base_run_level`). Detail: §W4. |
| D23 | Ephemerality tiers | Three lifetimes: **Run** (a dive; ephemeral — Run Level + red-chest items lost on any exit), **World** (a shard; persistent as seed + event log), **Account** (always persistent: Vault, permanent gear, Meld skills, heroes, unlocks). Refines D12. Detail: §W4–§W5. |
| D24 | Verticality — terraces, cliffs-as-walls, connectors-only | Overworld elevation is a small number of **discrete integer levels** (terraces/plateaus, bounded by `max_level` **[TUNABLE]**) per section, derived deterministically from the section seed — **not** a continuous heightmap. A boundary between cells of different level is a **cliff: an impassable wall** (movement blocks and slides). An avatar's level changes **if and only if** it is on a placed **connector** — a **slope** (walkable ramp), **ladder**, or **rope** — joining those levels; there is **no free climbing**, and generation places ≥1 connector per raised terrace so none is stranded. Touch/harvest/battle-join compare **level as well as position**. Elevation never feeds difficulty (`tier`/`mlevel`/`stat_mult` stay functions of `distance` only, §G). **Path feasibility holds by construction:** every level change on the hub→portal clear path is served by a Slope connector and both endpoints stay on level 0, so a grounded route home always exists. Server-authoritative (§S); wire surface is additive (2-D `Position` + `SnapshotEntity.level: Option<u8>` + `world.terrain_section`). Behavior: [`behaviors/verticality.md`](behaviors/verticality.md). |
| D25 | Dungeons — hand-authored committed sub-spaces (WG-1) | Dungeons are **hand-authored** (not procgen) multi-floor sub-spaces reached through a **chanced entrance** placed in the streaming overworld (drawn from a per-biome pool; `dungeon_spawn_chance` **[TUNABLE]**). Entry (`run.enter_dungeon`) is **deliberate** (never automatic on walking past) and pulls in teammates within `[ai] join_radius` — a **per-entry-fresh subinstance** shared by a group of ≤4 (a dungeon in progress is not joinable later). A dungeon is a **committed space**: no Town Portal inside; you leave only by the authored **end-exit** (→ the overworld position you entered from) or by **death** (backpack lost, like any death — §D6/run-lifecycle). Inside: **puzzles** (levers/plates/keys open doors/gates via a boolean `when` grammar — `all`/`any`/`not`/`seq`/`count`/`has_key`/`boss_dead`), **stairs** between floors, **disarmable traps** (armed→disarmed; a Dex check the **Shifter** excels at, a fumble springs it; a hit's severity scales with the floor's stamped distance), a **boss** (an FS-4 named boss), and **treasure** (typically boss-gated; loot **rolled** at the stamped distance and/or **authored**). **Difficulty + loot ride a stamped `effective_distance = entry_distance + floor × dungeon_depth_level_step` [TUNABLE]**, never the dungeon-local position. Authored content is compiled with a build-time **solvability gate** (every dungeon is provably completable). Server-authoritative (§S); crude client render reuses existing entity tags pending the dedicated dungeon render. Detail: [`behaviors/dungeons.md`](behaviors/dungeons.md). |

## §G. Glossary & Canonical Names

Use these exact terms (snake_case in wire/DB contexts, PascalCase for models).

| Term | Model name | Definition |
|------|-----------|------------|
| Player / account | `Player` | The persistent account (username + bcrypt-hashed password, D17). Owns Vault, Meld Skills, class unlocks, cosmetics. |
| Chits | `chits` | The currency (D10). 64-bit integer, symbol `c`. Replaces every "Gold"/"G" reference. |
| Character class | `CharacterClass` | Enum: `explorer`, `dragoon`, `sage`, `ranger`, `alchemist_knight`, `bard`. Spike additions (implemented kits): `psyker`, `resonant`, `shifter`, `phoenix_guard`. |
| Run | `Run` | One ephemeral maze excursion by an instance. Ends in `extracted`, `died`, or `abandoned`. |
| Instance | `MazeInstance` | The 1–4 player shared maze world for a run set. Has its own world seed. **Precursor**: D19/§W1 evolve this into the persistent, player-seeded `World`. |
| Party | `Party` | The 1–4 players inside one MazeInstance. |
| Hub | `Hub` | Persistent safe zone. `hub_kind`: `center` or `outer`. Keyed by `distance` (0, 500, 1000, …). |
| The Last City | `Hub` (`center`) | Canonical name of the Center Hub city — the post-auth home and extraction return target. Supersedes the "The Weld" working name. |
| Vault | `Vault` | Per-player persistent storage: chits, materials, blue-chest gear, gems. |
| Backpack | `Backpack` | Per-player ephemeral run inventory. Deleted on death, banked on extraction. |
| Blue Chest gear | `GearItem` with `insurance: blue` | Permanent insured equipment. Survives death; loses max durability. |
| Red Chest gear | `GearItem` with `insurance: red` | Run-found power gear. Lost on death unless extracted (extraction converts it to owned Vault gear, still `red` tier). |
| Run Level | `run_level` | Ephemeral combat level (D4 `base_run_level(hub)`). **D22 refines:** resets to 1 on return to the Last City / death; persists across forward-town stops within a push (§W4). |
| Meld Skill | `MeldSkill` | Persistent non-combat skill. `skill_kind`: `forging`, `mercantile`, `alchemy`. Levels 1–99. |
| Gem | `Gem` | Permanent socketable (GDD "Materia/Gems"), crafted via Alchemy, slots into blue-chest gear. |
| Gatekeeper | `GatekeeperBoss` | Boss at each biome border (distance ≡ 500·k − 1). Drops class emblems. |
| Emblem | `ClassEmblem` | Account-level class unlock item, e.g. "Emblem of the Dragoon". |
| Stall | `Stall` | Player shop deployed in a hub; persists while owner offline. |
| Contract | `Contract` | Bounty-board gathering order: item, quantity, reward, expiry. |
| Battle | `Battle` | One active ATB subscreen encounter, server-side entity. |
| Sleeping | `sleeping` avatar state | Disconnected avatar left on overworld; attackable. |
| Ward | `WardItem` | Consumable protecting a sleeping avatar: `warding_tent`, `sanctuary_campfire`. |
| Vanguard Board | `VanguardBoard` | Seasonal leaderboard of max distance per instance. |
| Season | `Season` | 13-week leaderboard epoch. |
| Chunk | `Chunk` | Server-streamed square region of overworld, 64×64 tiles. **[TUNABLE]** |
| Distance | `distance` | Euclidean distance from world origin (Center Hub) in tile units, `floor`ed to integer for all threshold checks. |
| World / Realm | `World` | A **persistent, player-seeded** overworld shard — the target evolution of `MazeInstance` (D19, §W1). Holds its players, monster population, and player-built structures; persists as **seed + event log** (§W5). Capped; a full world queues (no auto-fork). |
| Shift | `Shift` | A dimensional swap: a region of the overworld retiles to a different biome, dealing Force damage to and wiping the creatures/collectables of everything inside (D20, §W2). |
| Structure | `Structure` | Any player-built, HP-bearing, destructible, siege-able world object. One primitive distinguished by a `function` tag (D21, §W3). |
| Anchor | `Structure` (`function: anchor`) | A defendable Structure that **pins its region against the Shift while it stands**; reduced to 0 HP, its region becomes shiftable again (§W3). |

## §I. Identifier & Wire Conventions

- All entity IDs: UUIDv7 strings (`string (uuid)`), server-generated.
- Timestamps: ISO 8601 UTC, `string (date-time)` on HTTP; `u64` unix millis on the realtime protocol.
- All wire field names: `snake_case`.
- Realtime protocol messages are named `<domain>.<verb_phrase>`, prefixed by direction: client→server messages documented under **C2S**, server→client under **S2C**. Domains: `session`, `world`, `movement`, `battle`, `social`, `run`.
- Realtime envelope: `{ "type": string, "seq": u32, "ts": u64, "payload": object }`. `seq` is per-connection monotonic; server echoes client `seq` in acks.
- HTTP API: REST, base path `/v1`, Bearer session-token auth issued by `/v1/auth/login` against username + bcrypt-verified password (D17). Standard error envelope: `{ "error": { "code": string, "message": string, "request_id": string } }`.
- Canonical HTTP error codes: `validation_error` (400), `unauthorized` (401), `forbidden` (403), `not_found` (404), `conflict` (409), `insufficient_funds` (409), `rate_limit_exceeded` (429), `internal` (500).

## §S. System Boundaries

| System | Owns | Never does |
|--------|------|-----------|
| Rust server | All authority: world gen, movement validation, ATB math, loot rolls, economy transactions, disconnect handling, leaderboards | Trust client-computed outcomes |
| Bevy client (single app) | Overworld rendering (HD-2D), input, prediction/interpolation, collision presentation, AND all UI: ATB battle screens, hub UIs (Vault, Training Ground, Stall shop, Bounty Board, leaderboards), menus (D16) | Persist state; decide combat results; talk to DB directly — all data via server APIs |
| Realtime channel (WebSocket) | Ephemeral state sync: movement, chunks, battles, drops, presence | Carry economy/persistent mutations (those are HTTP) |
| HTTP API | Persistent state: auth, vault, gear, meld skills, stalls, contracts, leaderboards, seasons, run history | Real-time sync |

Boundary rule: anything that survives logout is mutated through the HTTP API (or by the server itself at run end); anything ephemeral flows over the realtime protocol.

## §B. Balance Formulas & Constants

All constants **[TUNABLE]** unless noted structural.

### Distance → difficulty
- `tier(d) = floor(d / 100)` — loot/monster tier band.
- Monster level: `mlevel(d) = max(1, round(d / 12.5))` (so d=500 → L40, matching hub base levels).
- Monster stat scale: `stat_mult(d) = (1 + d/500)^1.25` for `d ≤ 5000`; past the final curated hub, `stat_mult(d) = stat_mult(5000) × 1.5^((d − 5000)/500)` (exponential endgame, structural).
- Loot rarity weights shift one band per tier. Red-chest gear reaches its **full** drop rate at `d = 300` and **ramps in** below it: nothing below `d = 40`, then linear from `gear_ramp_start_mult` of the rate up to all of it at 300. `d = 300` is where the gear *game* lives, not a hard cutoff — a cutoff there is unreachable in practice, since the pre-generated area chain's deep portal sits at only `d ≈ 342–384`, so it would leave most of every dive with the chase switched off. `[TUNABLE]` `[loot] gear_ramp_start_distance`, `gear_ramp_start_mult`.
- A felled encounter may also drop a **potion**, at `[loot] potion_drop_chance`, drawn from the consumables whose own tier is at or below `tier(d)`. Excludes the Revive and Experience consumables, which have their own dedicated world-drop rates (`[consumable] world_revive_item_chance` / `world_xp_item_chance`).

### Hubs & run levels
- Hubs at `d = 0, 500, 1000, 1500, …, 5000` (11 curated hubs, structural). Beyond 5000: no hubs, infinite scaling.
- `base_run_level(hub) = 1 + hub.distance × 0.078` rounded to nearest int → Center = 1, D500 = 40, D1000 = 79, D5000 = 391.
- Run Level cap: none (grows with XP during run); XP formula `xp_to_next(L) = 80 × L^1.6`.
- **Encounter XP falls off once a hero has out-levelled the ground.** An encounter pays
  a hero in full while the hero is within `xp_gap_grace` levels of it, then linearly down
  to `xp_gap_floor_mult` at `xp_gap_zero` levels above; a hero at or *below* the
  encounter's level is never penalised, so a lagging hero catches up. Each hero weighs
  the encounter against its own level (`meld_run::xp_after_level_gap`).
  This is what makes "distance is the difficulty axis" true of **reward** and not only of
  danger: creature power rides distance, but the level curve is priced at the level's own
  matched depth (`d = 12.5 × L`), so without the falloff a party that levels *without
  travelling* is paid hub rates against a hub-rate curve. Measured, two heroes ground
  `d = 0` to level 16 while taking 0–1 damage a fight, then died in one encounter the
  moment they walked out.
- Gatekeeper arenas at `d = 500k − 1` for k = 1..10 (structural); arena is a full-width chokepoint — no path past it without clearing (per-instance clear flag).

### Biome bands (curated tutorial order; theme is randomized per run)
`0–100` Forest, `100–300` Desert, `300–500` Ashfall, `500–1000` Tundra, `1000–1500` Mire, then repeating themed bands defined by content tables per 500. This fixed order is the **tutorial** order (an account's first dive) and the difficulty-band reference. The biome is a **difficulty-neutral skin** (difficulty rides `distance`; creatures scale via `stat_mult`), so on every non-tutorial run the biome *theme* is drawn per section from the run seed with no adjacent repeat — the start and order both vary (roadmap WG-2/WG-3; [`behaviors/world-generation.md`](behaviors/world-generation.md)). The **Shift** (§W2) preserves this: a Shift re-skins a region's biome; its Force damage + entity wipe is a discrete **hazard event**, not the biome carrying steady-state difficulty.

### ATB combat
- ATB tick: 100 ms server tick. Gauge fill per tick: `speed_stat / 400` (gauge full at 1.0).
- Turn timeout: an actor with a full gauge auto-defends after 15 s without an action.
- Flee: base 60% success, −10% per tier the encounter is above party level tier, always ≥ 5%. Gatekeepers: flee disabled.
- Battle merge: joining party is inserted at gauge 0; enemy stats do not rescale mid-fight, but Gatekeeper HP pools are sized for 8 at spawn.

### Death & durability
- On death: backpack deleted, run level deleted, blue-chest gear returned with `max_durability × 0.9` (round down).
- Repair: a Forging-L crafter can restore max durability up to `base_max × (0.5 + L/198)` (L99 → 100%).

### Economy
- Hub tax: `10% − mercantile_level × 0.05%`, min 5%. Applied to stall sales and contract rewards, paid by seller/poster.
- Stall slots: `4 + floor(mercantile_level / 10) × 2`, max 24. Stalls in hub `d ≥ 1000` require Mercantile ≥ 30; `d ≥ 3000` require ≥ 60.
- Contract escrow: reward chits locked at posting; expiry 7 days, auto-refund.

### Disconnect handling
- Grace window: 10 s silent reconnection before disconnect rules fire.
- Standard encounter: forced flee (always succeeds, structural). Elite/Gatekeeper: auto-defend until battle ends or player reconnects.
- Sleeping avatar: persists on overworld until instance closes. Instance closes when all members extracted/died/abandoned, or after 60 min with all members disconnected → sleeping avatars auto-abandon (counts as death for backpack, no durability loss).
- Ward items: `warding_tent` 30 min invisibility to monster pathfinding, `sanctuary_campfire` 10 min invisibility + slow HP regen aura.

### Sessions & seasons
- Season length: 13 weeks (structural). At season end: Vanguard Board immortalized (read-only archive), titles granted to top 100 instances, infinite-zone leaderboard resets. Vault, hubs, meld skills, unlocks are NOT wiped (structural).

### Networking targets (non-binding perf goals)
- Overworld sim 20 Hz, snapshot broadcast 10 Hz, interest radius 2 chunks.
- Battle updates event-driven + 1 Hz keepalive.

## §W. World Model — the Shifting Lands

> **Status: target model (evolves the slice).** The current build implements the
> precursor — one shared, ephemeral `MazeInstance` (§G, D13). This section is the
> authoritative *target*: a persistent, player-seeded world that actively rearranges
> itself, in which players build and defend structures to hold ground and race a
> seasonal push to a far end-world boss. The genre it defines is a **sim /
> world-builder / desperate roguelite**. Fiction + biome bestiary:
> [`lore/shifting-lands.md`](lore/shifting-lands.md); server/scaling plan:
> [`proposals/server-scaling.md`](proposals/server-scaling.md). Higher docs still
> win on conflict; fold rules here into behaviors/interfaces as each system is built.

### §W1. A world is a persistent, player-seeded shard (D19)

A **World** (a "realm") is one persistent overworld + the players in it + its monster
population + the structures players have built on it. Its identity is a
**player-chosen seed**; because worldgen is deterministic from the seed
(`section_seed(run_seed, n)`), the baseline map is regenerable for free and never
stored (§W5). Worlds are **capped**; horizontal scale is **many worlds**, and a world
at cap **queues** rather than auto-forking (it holds unique player structures that
can't be cloned). A player's town/anchors live in exactly one world. This supersedes
"the instance is discarded on close" *as the target*: the world persists; only a
player's **Run** is ephemeral (§W4).

> **Authored spaces are content within a world, not shards.** Alongside the
> procedural persistent overworld, the game has *authored* spaces on the same
> "authored-space substrate": **dungeons** (ephemeral, per-entry-fresh subinstances,
> ≤4 players, **discarded on exit — never persisted**, the opposite lifecycle to §W5)
> and the **City** (a persistent authored hub). Both are **content living on a world's
> single owning task — a "map of live spaces" — not their own shard** (consistent with
> D19 and the "towns are content, not shards" rule). Design (proposal): [`proposals/dungeons.md`](proposals/dungeons.md).

### §W2. The Shift (D20)

The overworld is the **Shifting Lands**: regions periodically **Shift** — swap to a
different bestiary biome mid-run, after a brief warning tell, dealing **Force** damage
to entities in the swapped cells and **wiping that region's creatures + collectables**.
This is the roguelite-freshness engine: the map is not a fixed route, it *rearranges*.

- **Tabletop reference** (from [`lore/shifting-lands.md`](lore/shifting-lands.md)):
  cadence = roll 1d10 → count of natural 1s on ongoing checks before the next Shift;
  location 1d100 × 1d100; size 1d6 (Tiny → Cataclysmic); Force damage scales with size
  (Tiny 1d6 … Cataclysmic 10d10). **Biomes are variable-sized** (the size table).
- **Game translation** (ATB/overworld build): cadence, region size, and damage are
  server config **[TUNABLE]**; the retile picks a biome from the bestiary.
- **Determinism (structural):** the Shift scheduler MUST be seeded from
  `(world_seed, shift_generation)` and driven by the **server tick counter — never
  `Instant::now`/wall-clock**. This keeps the engine deterministic (as `meld-world`
  already is) and makes world persistence a cheap replay (§W5).

### §W3. Structures & anchors — one primitive (D21)

A **Structure** is a player-built, HP-bearing, destructible world object that monsters
path to and attack (siege). There is **one** primitive; a `function` tag varies its
role:

- `anchor` — **pins its region against the Shift while it stands.** It holds only while
  **defended**: reduce it to 0 HP and its region becomes shiftable again. Anchors are
  how players manufacture permanence in a self-rearranging world ("Hope is hard work;
  nothing is free").
- `portal` — a **plantable, defendable extraction** point (the evolution of D15).
- `wall` / defense — blocks and soaks siege.
- `stash` — field storage.

The siege sim, the spatial interest index, and world persistence handle every function
uniformly — do not build towns, anchors, portals, and camps as separate systems.
Costs, HP, defense values, and pin radius are **[TUNABLE]**.

### §W4. Ephemerality — three lifetime tiers (D22, D23)

| Tier | Lifetime | Contents |
|------|----------|----------|
| **Run** (one dive) | reset on **any** exit — death *or* full extraction to the Last City | Run Level (→ 1), ephemeral items (red-chest class) |
| **World** (a shard) | persistent — seed + event log (§W5) | terrain-via-seed, structures, monster population, dropped loot |
| **Account** | always persistent | Vault, permanent gear, Meld skills, heroes, unlocks |

**Run Level** is built by pushing outward and **persists across forward-town stops
within a single push**; it resets to 1 only on **return to the Last City** (full
extraction) or **death** (refines D4). Forward towns exist precisely to sustain a deep
push toward the end-world boss without triggering that reset. Consequently the reward
gap between **extracting and dying is not** the level or ephemeral items (lost either
way) — it is the **backpack**: extraction banks it to the Vault, death drops it.

### §W5. World persistence — seed + event log (D23, refines D12)

A world stores only its **delta from the seed baseline**, event-sourced:

- The **baseline** (terrain, biome layout, initial monster/resource placement) is
  regenerated from the seed and **never stored**.
- The **natural Shift schedule** is a pure function of `(seed, shift_generation)`
  (§W2) — replayable, not stored.
- **Persisted state = the log of player-caused events**: structures built / damaged /
  destroyed, anchors placed / lost (and the Shifts they suppressed or re-enabled),
  harvested / looted state. Replaying seed + log reconstructs exact world state.
- Empty worlds **hibernate** to Postgres and reload on first joiner.
- **Seasons are the GC:** at a season boundary worlds are archived / reset, bounding
  the event log. Account-tier state is **not** wiped (consistent with §B "Sessions &
  seasons").


> **Amended (creature scaling).** `stat_mult(d)` no longer drives creature
> **health**. Each creature stat is scaled against the hero stat that OPPOSES it,
> and those do not share a curve:
>
> - **HP** is opposed by party *damage*, which is dominated by gear — and gear power
>   is linear in `tier(d)` (`gear_atk_per_tier` x 7 slots). So HP grows at the same
>   rate, `max(1, 1 + hp_per_tier x (d/tier_divisor - 0.5))`, and the rounds-per-fight
>   ratio holds at every depth by construction rather than by tuning.
>
>   It is linear in **`d`, not in the integer `tier(d)`**. Riding the floored tier made
>   it a staircase with a `hp_per_tier`-sized riser (6.4x at shipped values): a creature
>   at d=99 died in two swings and the same creature at d=100 took ten, so one unit of
>   walking turned an 8-second fight into a 40-second one and nothing threatened the
>   party on either side of the line. The `- 0.5` runs the line through each band's
>   centre, so every depth the curve was tuned at keeps the multiplier it had.
> - **Attack** is opposed by hero HP and defence, which grow with *level*. That stays
>   `stat_mult(d) = (1 + d/500)^stat_mult_exp`.
> - **Armour** is opposed by hero attack, gently: `def_mult(d) = (1 + d/500)^0.7`.
>
> Before the split, HP rode `stat_mult` while gear rode `tier` — different shapes, so
> no exponent could make them track. A geared hero one-shot ordinary creatures at
> every distance while an ungeared one was fine.
