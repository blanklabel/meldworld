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
| D9 | Character classes | Launch set: Hunter (default), Dragoon, Sage, Ranger, Alchemist-Knight, Bard. Classes beyond Hunter are Gatekeeper drops. Placeholder — content team may extend. |
| D10 | Currency granularity | Currency is **Chits** (`chits`, symbol `c`), a 64-bit integer; no fractional chits. Every GDD "Gold"/"G" reference reads as Chits. |
| D11 | Combat is server-authoritative | All ATB math (timers, damage, status) computed server-side; clients render and submit intents only. |
| D12 | Persistence store | Single logical relational DB for persistent state; ephemeral run/instance state lives in server memory with periodic snapshots for crash recovery. Specs describe observable behavior only. |
| D13 | Matchmaking | Party of 1–4 formed in a Hub; solo players may opt into matchmaking pool filtered by departure hub. An instance is created at maze-entry time and is not joinable afterward except via battle merge (which merges battles, not instances). |
| D14 | Offline stall fulfillment | Stalls and bounty contracts execute server-side atomically; escrow model (§Economy semantics in behaviors specs). |
| D15 | Extraction portal spawning | Extraction portals spawn deterministically at every Hub (including Center) plus procedurally at ~1 per 200-distance band per instance seed. Escape items ("Ripcord Scroll") extract from anywhere with a 10 s interruptible channel. **[TUNABLE]** |
| D16 | UI framework pivot | **No Flutter.** All UI — ATB battle screens, hub UIs (Vault, Training Ground, Stall shop, Bounty Board, leaderboards), menus — is built in Bevy (bevy_ui/ecosystem UI crates). Any GDD/spec reference to "Flutter UI" reads as "Bevy UI layer". Art direction: indie-style HD-2D (pixel sprites/tiles with 3D lighting, DoF, particles). |
| D17 | Auth mechanism | Registration/login is username + password only. Usernames unique, 3–20 chars `^[a-zA-Z0-9_]+$`. Passwords 8–128 chars, stored as bcrypt hashes (cost 12 **[TUNABLE]**) in Postgres; plaintext never persisted or logged. Successful login issues a short-lived session token (Bearer, 24 h expiry **[TUNABLE]**) for HTTP plus a single-use realtime session ticket. No OAuth/email/2FA at v0.1. |
| D18 | Persistence engine | Persistent state lives in **Postgres** (explicit implementation mandate; specs still describe observable behavior, with storage noted only where the mandate requires it, e.g. bcrypt credential storage). |

## §G. Glossary & Canonical Names

Use these exact terms (snake_case in wire/DB contexts, PascalCase for models).

| Term | Model name | Definition |
|------|-----------|------------|
| Player / account | `Player` | The persistent account (username + bcrypt-hashed password, D17). Owns Vault, Meld Skills, class unlocks, cosmetics. |
| Chits | `chits` | The currency (D10). 64-bit integer, symbol `c`. Replaces every "Gold"/"G" reference. |
| Character class | `CharacterClass` | Enum: `hunter`, `dragoon`, `sage`, `ranger`, `alchemist_knight`, `bard`. Spike additions (implemented kits): `psyker`, `resonant`, `shifter`, `iron_hull`. |
| Run | `Run` | One ephemeral maze excursion by an instance. Ends in `extracted`, `died`, or `abandoned`. |
| Instance | `MazeInstance` | The 1–4 player shared maze world for a run set. Has its own world seed. |
| Party | `Party` | The 1–4 players inside one MazeInstance. |
| Hub | `Hub` | Persistent safe zone. `hub_kind`: `center` or `outer`. Keyed by `distance` (0, 500, 1000, …). |
| Vault | `Vault` | Per-player persistent storage: chits, materials, blue-chest gear, gems. |
| Backpack | `Backpack` | Per-player ephemeral run inventory. Deleted on death, banked on extraction. |
| Blue Chest gear | `GearItem` with `insurance: blue` | Permanent insured equipment. Survives death; loses max durability. |
| Red Chest gear | `GearItem` with `insurance: red` | Run-found power gear. Lost on death unless extracted (extraction converts it to owned Vault gear, still `red` tier). |
| Run Level | `run_level` | Ephemeral combat level, resets per run to `base_run_level(hub)`. |
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
- Loot rarity weights shift one band per tier; red-chest gear cannot spawn below `d = 300`.

### Hubs & run levels
- Hubs at `d = 0, 500, 1000, 1500, …, 5000` (11 curated hubs, structural). Beyond 5000: no hubs, infinite scaling.
- `base_run_level(hub) = 1 + hub.distance × 0.078` rounded to nearest int → Center = 1, D500 = 40, D1000 = 79, D5000 = 391.
- Run Level cap: none (grows with XP during run); XP formula `xp_to_next(L) = 80 × L^1.6`.
- Gatekeeper arenas at `d = 500k − 1` for k = 1..10 (structural); arena is a full-width chokepoint — no path past it without clearing (per-instance clear flag).

### Biome bands (curated tutorial order; theme is randomized per run)
`0–100` Forest, `100–300` Desert, `300–500` Ashfall, `500–1000` Tundra, `1000–1500` Mire, then repeating themed bands defined by content tables per 500. This fixed order is the **tutorial** order (an account's first dive) and the difficulty-band reference. The biome is a **difficulty-neutral skin** (difficulty rides `distance`; creatures scale via `stat_mult`), so on every non-tutorial run the biome *theme* is drawn per section from the run seed with no adjacent repeat — the start and order both vary (roadmap WG-2/WG-3; [`behaviors/world-generation.md`](behaviors/world-generation.md)).

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
