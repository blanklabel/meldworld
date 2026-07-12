# MELDWORLD — Spec vs. Spike Gap Analysis

What the spec (`GDD.md` → `CANON.md` → `behaviors/` + `interfaces/` + `edge-cases/`)
describes, versus what the architecture spike on this branch actually implements.
Written after reading the full spec tree. Blunt on purpose.

**One-line verdict:** the spike is a single server-side vertical thread
(auth → join instance → walk into monster → ATB-kill it). The **entire
player-facing client/UX is absent**, and most server systems (economy, meta,
endgame, persistence beyond accounts, disconnect/resume, world streaming) are
0%. Ballpark coverage of the spec surface: **single digits**.

---

## 0. UX: the core loop now exists; everything else is unbuilt

The spec mandates an all-Bevy client (CANON D16; BUILD-PLAN T4 + T5) that is the
*entire* player experience. The **core gameplay loop is now playable** in a Bevy
client (`client/`): Join → Overworld → ATB Battle → Ended, driven by the real
wire protocol, verified headlessly to a victory. The **meta / hub / economy /
endgame UIs are still absent.** Status per surface:

| Surface | Spec source | Status |
|---------|-------------|--------|
| Bevy app shell + state routing (Join ↔ Overworld ↔ Battle ↔ Ended) | T4/T5, `client/app-states.md` | ✅ core loop |
| Overworld render (avatars + monster from `world.snapshot`, WASD movement) | T4, movement-world | 🟡 minimal (flat sprites; no HD-2D, no chunks/prediction) |
| **ATB battle screen** (gauges, HP bars, attack on your turn) | T5, combat-atb | 🟡 minimal (attack only; no skills/items/targeting/merge UI) |
| Auth / join screen | T5, auth-players | 🟡 guest join (Enter); no register/login form yet |
| HD-2D pipeline (tiles, biomes, lighting/DoF/particles) | D16, T4 | ❌ none |
| Overworld entities: drops, wards, sleeping avatars, portals, gatekeeper arenas, stall sprites | T4, async-interaction, disconnect | ❌ none |
| Battle screen multi-party expansion (4/8/16), flee/auto-defend states | T5, combat-atb merge | ❌ none |
| **Vault UI** (chits, materials, gear + durability, equip/unequip, red/blue) | T5, vault-gear | ❌ none |
| **Training Ground** (build templates, bulk skill allocation) | T5, meta-progression | ❌ none |
| Crafting / repair / alchemy / gem-socketing screens | T5, crafting-meld | ❌ none |
| **Stall UIs** (owner deploy/manage + buyer shop) | T5, economy | ❌ none |
| **Bounty Board** (browse/post/accept/fulfill) | T5, economy | ❌ none |
| **Leaderboards** (Vanguard live + archived seasons, titles) | T5, endgame-seasons | ❌ none |
| Run-flow UI (party formation, matchmaking, backpack, extract/death summary) | T5, run-lifecycle | ❌ none |
| Disconnect UX (reconnect banner, sleeping-ally indicators, ward deploy flow) | T5, disconnect | ❌ none |

Practically: **a human can now play the core loop** (join → fight Grendel →
win/lose) via `client/scripts/serve.sh`. Everything past that one fight is still
server-or-spec-only.

---

## 1. What the spike DOES implement (for reference)

- **Auth**: `register` / `login` / `players/me` over Postgres + bcrypt; enumeration-safe login. (BUILD-PLAN M1.1/M1.8/M1.9)
- **Realtime session**: ticket handshake → `session.authenticated`, heartbeat, per-session seq.
- **One instance, one fight**: `run.enter_maze` forms a party from connected players, spawns one Forest monster, movement + touch triggers a battle.
- **ATB engine**: 100 ms tick, gauge fill `speed/400`, attack/defend/flee, 15 s auto-defend, victory/defeat. Deterministic + unit-tested.
- **Distance→difficulty formulas** (`tier`/`mlevel`/`stat_mult`) — implemented even though the world around them isn't.
- **Proof**: 4 headless bots kill the monster end-to-end over real HTTP+WS.

---

## 2. Server systems — spec vs. spike

### 2.1 HTTP API — **4 of ~46 endpoints**

Implemented: `POST /v1/auth/register`, `POST /v1/auth/login`, `GET /v1/players/me`, `GET /v1/vault` (+ non-spec `/v1/healthz`).

Missing whole groups (interfaces/http-api.md):
- **players** (4): `players/{id}`, `players/me/class-unlocks`, `players/me/cosmetics`, `PUT players/me/title`
- **vault-gear** (~15): vault summary, materials, gear list/detail, equip/unequip, gems, socket/unsocket, repair, build-templates CRUD
- **crafting-meld** (4): meld-skills, recipes, craft, synthesize
- **economy** (11): stalls (deploy/list/get/close/buy), contracts (post/list/get/accept/fulfill/cancel)
- **runs-world** (5): runs history/detail, `runs/prepare` (matchmaking), hubs, hub rebuild
- **leaderboards** (4): vanguard (+ me), seasons (+ detail)

### 2.2 Persistence (`meld-db`) — **~3 of ~20 models**

Have: `players` (id, username, password_hash, …) and the **Vault** (`vaults` chits
+ `vault_items` stacks). Extraction banks a run's backpack into the Vault
atomically (Postgres), read back via `GET /v1/vault`.

Missing most persistent models (data-models/*): `Vault`, `GearItem`,
`Gem`, `ConsumableItem`, `WardItem`, `Material`, `MeldSkill`, `ClassEmblem`,
`CosmeticTitle`, `PrestigeAura`, `Stall`, `StallListing`, `Contract`,
`LedgerEntry`, `Hub`, `BiomeBand`, `Season`, `VanguardBoardEntry`, `Run` history,
build templates. No migrations for any of them; no run-end persistence (extraction
banking, death durability) touches the DB at all.

### 2.3 Realtime protocol — most message types not wired

Handled today (in/out): `session.authenticate/authenticated/heartbeat(_ack)/error`,
`run.enter_maze/started/member_result/backpack_update`, `movement.move_intent`,
`world.snapshot`, `battle.started/turn_ready/gauge_update/submit_action/action_resolved/ended`.

Defined-but-unused or **not modelled at all**:
- **world**: `chunk_load`, `chunk_unload`, `entity_spawn`, `entity_despawn`, `presence_update`, `deploy_ward`, `ward_deployed` — none.
- **battle**: `party_joined` (raid merge), `external_effect` (heal injection), `participant_left` (defined, not driven).
- **run**: `begin_extraction`, `cancel_extraction`, `channel_started`, `channel_interrupted`, `abandon`, `instance_closed` — none.
- **social**: `drop_item`, `pickup_item`, `item_picked_up`, `drop_item_on_player`, `drop_applied` — none.
- **session**: `terminated` defined; resume/seq-replay path unimplemented.
- No realtime rate limiting; `movement.position_correction` not emitted.

### 2.4 World generation — bounded arena, not the world

Missing (world-generation.md): chunk streaming (64×64, interest radius), seeded
deterministic generation, biomes past Forest, chokepoints, **Gatekeeper arenas**,
extraction portals, the infinite zone past d=5000, loot banding, red-chest floor.
Have: a single fixed-size in-memory arena with one monster; the scaling formulas.

### 2.5 Movement — placeholder

Missing: real 20 Hz sim loop, 2-chunk interest management, collision, speed
enforcement against real terrain, position corrections, 10 Hz snapshot cadence.
Have: per-intent integration + proximity touch, a naive snapshot.

### 2.6 Combat — core loop only

Missing (combat-atb.md): **battle merge / raid** (`party_joined`, 8/16 caps),
skills, items (in-battle consumables), status effects, **external heal injection**,
encounter-class behaviors for `elite`/`gatekeeper`, resolution-ordering edge cases,
gatekeeper HP-sizing. Have: attack/defend/flee(basic), victory/defeat, auto-defend.

### 2.7 Disconnect / resume — 0%

Missing entirely (disconnect-handling.md): 10 s grace window, session **resume +
seq replay**, forced-flee vs auto-defend on disconnect, **sleeping avatars**, ward
protection, roaming-monster-attacks-sleeper, 60-min all-disconnected auto-abandon.
`session_id` is carried but resume is a stub.

### 2.8 Run lifecycle — entry + battle + **extraction**

Have: enter_maze, victory loot into the backpack, **portal extraction** (an
interruptible channel that banks the backpack into the persistent Vault),
defeat→`died`. Missing: death **durability penalty** (needs the gear model),
`ripcord_scroll` escape item, explicit abandon, instance close, matchmaking/party
formation (auto-forms from whoever's connected), backpack capacity, ground
**drops/pickups**, drop-on-battling-player.

### 2.9 Economy — 0%

Nothing from economy.md: stalls, listings, purchase (atomic, taxed), bounty
contracts (escrow, accept, fulfill, expiry/refund), the durability sink, the chits
ledger, and all the conservation invariants (I1–I5).

### 2.10 Meta-progression — 0%

Nothing from meta-progression.md: hub unlock flow (gatekeeper→camp→rebuild),
class-emblem drops + account unlocks, Training Ground build templates, resource
stratification, the three **Meld Skills** (forging/mercantile/alchemy) with their
XP sources and level effects (repair cap, tax, stall slots/gates, gem gating).

### 2.11 Endgame / seasons — 0%

Nothing from endgame-seasons.md: the **Vanguard Board** (real-time max-distance
leaderboard), the infinite zone (exponential scaling, Prestige auras), the 13-week
season lifecycle (archive, top-100 titles, resets).

### 2.12 Content — one hardcoded creature

Missing: monster definitions (only `forest_bloom_stalker`), class stat blocks (only
`squire`), biome content tables, loot tables, crafting/gem recipes, item catalog.
`balance.toml` holds only the subset of `[TUNABLE]`s the slice reads — most CANON §B
constants aren't there yet.

---

## 3. Suggested next slices (dependency order)

1. **Playable Bevy client for the existing loop** (T4 shell + T5 auth/overworld/ATB) — turns the proven server thread into something a human can actually play. This is the highest-value next step given "no UX."
2. **Persistence + extraction** — the DB schema (Vault/gear/materials), `runs/prepare`, extraction channel + banking, death durability. Makes runs *mean* something.
3. **World streaming + real movement** — chunks, interest management, biomes; unlocks distance, portals, and the Vanguard metric.
4. **Economy + meld skills** — stalls, contracts, crafting/repair, the durability sink loop.
5. **Multiplayer depth** — battle merge, disconnect/resume, sleeping/wards, drops.
6. **Endgame** — gatekeepers + class unlocks, leaderboards, seasons, infinite zone.

Each is a milestone-sized effort in its own right (cf. BUILD-PLAN M1–M6). The spike
is M0-plus-a-thread; M1–M6 are still ahead.
