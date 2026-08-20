# Run & Social Messages

> Parent: [interfaces/realtime-protocol](../realtime-protocol.md)

Run lifecycle (enter maze, extraction channels, death, abandon, instance close, backpack sync) and asynchronous social interaction (ground drops, pickups, dropping consumables onto battling players). A `Run` ends in exactly one of `extracted`, `died`, or `abandoned` per member (CANON.md §G). All persistent consequences — vault banking on extraction, durability loss on death — are server-side mutations observed via the HTTP API, never realtime mutations (CANON.md §S boundary rule).

Shared payload objects (`Position`, `ItemStack`) are defined in the [index](../realtime-protocol.md#common-payload-objects).

### `run.enter_maze` (C2S)

Starts the party's run: the server creates the `MazeInstance` (with its own world seed) and the `Run`, resets combat state, and drops the party into the maze.

**Source:** GDD.md §2.2 (the Reset), §4 (base level scaling); CANON.md D13 (instance created at maze-entry time, not joinable afterward), §B (hubs & run levels).
**Direction:** C2S — sent by the **party leader** (solo players are their own leader) while the whole party stands in the same Hub. Party formation and matchmaking happen beforehand via Hub UIs over HTTP (out of scope).
**Idempotency:** Non-idempotent. A second `run.enter_maze` while a run is active → `invalid_state`.

**Payload** — empty object `{}` (the server already knows the sender's party and current hub; nothing client-supplied is trusted).

**Server validation**

- Sender not in a hub, or not the party leader → `invalid_state` / `forbidden`.
- Any party member already in an active run, disconnected, or in a different hub → `invalid_state`.

**Results in** — `run.started` to every party member; then initial `world.chunk_load` for all chunks in each member's interest radius, `world.entity_spawn`s, and a first `world.snapshot`.

**Example**

```json
{"type": "run.enter_maze", "seq": 12, "ts": 1783728200000, "payload": {}}
```

---

### `run.started` (S2C)

Authoritative run and instance state at maze entry.

**Source:** GDD.md §2.2, §4; CANON.md §B (`base_run_level(hub) = 1 + hub.distance × 0.078`, rounded to nearest int → Center = 1, D500 = 40), §G (`Run`, `MazeInstance`, `Backpack`).
**Direction:** S2C — sent to every party member in response to a valid `run.enter_maze` (carries `client_seq` on the leader's copy only).

**Payload**

| Field | Type | Required | Nullable | Default | Description |
|-------|------|----------|----------|---------|-------------|
| client_seq | integer (int64, u32 range) | Yes | Yes | — | Echo of the leader's `run.enter_maze` seq; `null` on other members' copies. |
| run_id | string (uuid) | Yes | No | — | The new Run. |
| instance_id | string (uuid) | Yes | No | — | The new MazeInstance. |
| departure_hub_distance | integer (int32, one of 0, 500, 1000, …, 5000) | Yes | No | — | The hub the run departs from. |
| base_run_level | integer (int32, ≥ 1) | Yes | No | — | Starting run level for every member: `round(1 + hub.distance × 0.078)`. |
| members | array of object (1–4 items) | Yes | No | — | The party. Fields: `player_id` string (uuid); `username` string; `character_class` string (enum: `explorer`, `dragoon`, `sage`, `ranger`, `alchemist_knight`, `bard`); `spawn_position` Position. |
| backpack | array of ItemStack | Yes | No | — | The recipient's starting backpack contents (empty array on a fresh run). Authoritative baseline for all later `run.backpack_update` deltas. |

**Example**

```json
{"type": "run.started", "seq": 5001, "ts": 1783728200080, "payload": {"client_seq": 12, "run_id": "0197a610-0001-7abc-9def-0123456789ab", "instance_id": "0197a610-0002-7abc-9def-0123456789ab", "departure_hub_distance": 500, "base_run_level": 40, "members": [{"player_id": "0197a2f0-11aa-7bbb-8ccc-0d1e2f3a4b5c", "username": "Marlowe", "character_class": "dragoon", "spawn_position": {"x": 498.0, "y": 12.0}}], "backpack": []}}
```

---

### `run.begin_extraction` (C2S)

Starts an extraction channel — at an extraction portal, or from anywhere with an escape item.

**Source:** GDD.md §2.2 (Extract: portal or escape item); CANON.md D15 (portals at every Hub plus ~1 per 200-distance band per instance seed; escape items extract from anywhere with a 10 s interruptible channel).
**Direction:** C2S — sent by a player in an active run, not in battle, not already channeling.
**Idempotency:** Non-idempotent; `invalid_state` if a channel is already active.

**Payload**

| Field | Type | Required | Nullable | Default | Description |
|-------|------|----------|----------|---------|-------------|
| method | string (enum: `portal`, `escape_item`) | Yes | No | — | Extraction mechanism. Determines which of the two following fields is required. |
| portal_entity_id | string (uuid) | No | Yes | null | The extraction-portal entity to use. Required when `method` is `portal`; must be `null` otherwise. |
| item_id | string (uuid) | No | Yes | null | Backpack escape-item instance to consume (e.g. `item_kind: "ripcord_scroll"`). Required when `method` is `escape_item`; must be `null` otherwise. |

**Server validation**

- Not in an active run, in battle, sleeping, or already channeling → `invalid_state`.
- `method`/field mismatch → `validation_error`.
- `portal_entity_id` unknown in this instance → `not_found`; farther than the 2-tile **[TUNABLE]** interaction range → `out_of_range`.
- `item_id` not in the sender's backpack → `not_found`; not an escape item → `validation_error`.

**Results in** — `run.channel_started` broadcast to the instance. The channel runs 10 s **[TUNABLE]** for both methods (canon fixes 10 s for escape items, D15; portals mirror it). Escape items are consumed when the channel **starts** (`run.backpack_update`) — an interrupted channel does not refund the item **[TUNABLE]**. While channeling, `avatar_state` is `channeling`; movement intents interrupt the channel. On uninterrupted completion the member's run ends: `run.member_result` with `result: "extracted"`.

**Example**

```json
{"type": "run.begin_extraction", "seq": 480, "ts": 1783729000000, "payload": {"method": "escape_item", "portal_entity_id": null, "item_id": "0197a611-4444-7abc-9def-0123456789ab"}}
```

---

### `run.enter_dungeon` (C2S)

Descend into a hand-designed dungeon whose entrance the avatar is standing next to (WG-1/DG-3). A **committed space**: you leave only by the end-exit or by dying — Town Portal is rejected inside. Entry is **deliberate** (a keypress), never automatic on walking past an entrance.

**Source:** ROADMAP WG-1 / DG-3; [`proposals/dungeons.md`](../../proposals/dungeons.md).
**Direction:** C2S — sent by a player in an active run, in the overworld, not in battle.
**Idempotency:** Non-idempotent; `invalid_state` if already in a dungeon.

**Payload**

| Field | Type | Required | Nullable | Default | Description |
|-------|------|----------|----------|---------|-------------|
| entity_id | string | Yes | No | — | The `entrance:<dungeon>` snapshot entity to descend through. |

**Server validation**

- Already in a dungeon, or in battle → `invalid_state`.
- `entity_id` not a live entrance in this world → `not_found`; farther than the interaction range → `out_of_range`.

**Results in** — the sender's subsequent `world.snapshot`s are scoped to the dungeon floor (its geometry + occupants) instead of the overworld; the overworld avatar is parked at the entry position and restored there when the player reaches the end-exit. (In-dungeon rendering is a temporary tag mapping pending the dedicated dungeon render, DG-6b.)

**Example**

```json
{"type": "run.enter_dungeon", "seq": 512, "ts": 1783729100000, "payload": {"entity_id": "dungeon-entrance-3"}}
```

---

### `run.cancel_extraction` (C2S)

Voluntarily cancels the sender's own active extraction channel.

**Source:** CANON.md D15 (channel is interruptible); voluntary cancel is a canon gap resolved by this spec.
**Direction:** C2S — legal only while the sender is channeling.

**Payload** — empty object `{}`.

**Server validation** — no active channel → `invalid_state`.

**Results in** — `run.channel_interrupted` (`reason: "cancelled"`) broadcast to the instance; `avatar_state` returns to `active`. The consumed escape item is not refunded.

**Example**

```json
{"type": "run.cancel_extraction", "seq": 484, "ts": 1783729004000, "payload": {}}
```

---

### `run.harvest` (C2S)

Begins working a resource node the sender is standing beside — a **channel**, not a
pickup (`MS-2`).

**Source:** GDD.md §4.1; roadmap `MS-2`; [behaviors/run-lifecycle.md](../../behaviors/run-lifecycle.md) "Flow: Harvesting".
**Direction:** C2S.

**Payload**

| Field | Type | Required | Nullable | Default | Description |
|-------|------|----------|----------|---------|-------------|
| entity_id | string | Yes | No | — | The `resource:<kind>` node from `world.snapshot`. |

**Server validation** — in a battle → `invalid_state`; already channeling (extraction *or*
harvest) → `invalid_state`; node unknown, empty, on another elevation, or beyond
`interaction_radius_tiles` → `out_of_range`.

**Results in** — `run.channel_started` (`method: "harvest:<node_kind>"`) broadcast to the
instance, then one `run.backpack_update` per unit (`cause: "harvest:<node_kind>"`) every
`fill_ms` until the node is empty or the channel breaks. Each unit also credits the node's
Meld skill.

**Example**

```json
{"type": "run.harvest", "seq": 512, "ts": 1783729000000, "payload": {"entity_id": "res-7"}}
```

---

### `run.use_item` (C2S)

Drinks a potion on the overworld, out of combat — the same registry and the same
backpack as the battle Item command. Without it a wounded party had to find a fight
before it could heal, so the walk to the next monster was where it died.

**Source:** GDD.md §4.1; [consumables registry](../../../shared/meld-proto/src/consumables.rs).
**Direction:** C2S.

**Payload**

| Field | Type | Required | Nullable | Default | Description |
|-------|------|----------|----------|---------|-------------|
| item_kind | string | Yes | No | — | A `CONSUMABLES` key held in the **Party Inventory or in `hero_slot`'s own pouch** (`bloom_salve`, `elixir`, `waking_salt`, `insight_mote`). In the field either container is in reach; only a battle restricts a hero to its own pouch. |
| hero_slot | int | Yes | No | — | Which of the sender's heroes drinks it (0-based party slot). |

**Server validation** — the sender's party is in a battle → `validation_error` (use the
battle Item command, which costs a turn); unknown `item_kind` → `validation_error`;
an effect that only exists inside a fight (`Barrier`/`Regen`/`Evasion`/`Adrenaline`) →
`validation_error`, checked **before** the stock check because it is a property of the
potion rather than of the pack; slot out of range → `validation_error`; none held →
`validation_error`. An item that would change nothing — a heal on a hero at full HP, a
revive on a hero still standing — is **refused and not consumed**.

The request names only *what* and *on whom*: the dose, the HP cap, and whether the
effect applies at all are computed server-side from the registry and `balance.toml`, so
an edited client can ask for the impossible but cannot receive it.

**Results in** — a refreshed `run.party` carrying the new HP, plus whichever container
paid: `run.pouches` when the drinker's own pouch was spent, else
`run.backpack_update` (`cause: "field_item"`). An Insight Mote may also produce
`run.level_up`.

**Example**

```json
{"type": "run.use_item", "seq": 530, "ts": 1783729010000, "payload": {"item_kind": "bloom_salve", "hero_slot": 2}}
```

---

### `run.move_item` (C2S)

Moves an item between the **Party Inventory** (shared, unbounded, unreachable in a
fight) and one hero's **pouch** (capped, and the only thing that hero can reach mid-
battle). See [run-lifecycle.md](../../behaviors/run-lifecycle.md) for the two-container
model this serves.

**Source:** roadmap `GR-9`; design of record.
**Direction:** C2S.

**Payload**

| Field | Type | Required | Nullable | Default | Description |
|-------|------|----------|----------|---------|-------------|
| item_kind | string | Yes | No | — | The item kind to move. |
| hero_slot | int | Yes | No | — | Which hero's pouch is the other end of the move (0-based party slot). |
| to_pouch | bool | Yes | No | — | `true` = Party Inventory → pouch; `false` = pouch → Party Inventory. |
| quantity | int | No | No | 1 | How many to move. Clamped to what is held. |

**Server validation** — the sender is in a battle → `invalid_state` (a fight is fought
with what the heroes were already carrying); not in a run → `invalid_state`; `hero_slot`
out of range → `validation_error`; nothing of that kind in the source, or the
destination has no room for a new kind → `validation_error`, and **nothing moves**.

Partial moves succeed: asking for 5 when 3 are held moves 3. Only the pouch can refuse
for want of space — returning something to the Party Inventory is always allowed,
because that container has no limit.

**Results in** — `run.backpack_update` (`cause: "pouch_transfer"`, `added`/`removed`
depending on direction) **and** `run.pouches` with the pouches whole.

**Example**

```json
{"type": "run.move_item", "seq": 531, "ts": 1783729011000, "payload": {"item_kind": "bulwark_tonic", "hero_slot": 0, "to_pouch": true, "quantity": 2}}
```

---

### `run.pouches` (S2C)

The caller's per-hero pouches, whole. Sent at run start and after any change — a
transfer, a potion drunk in battle, or a potion drunk in the field from a pouch. A
snapshot rather than a delta: a pouch is `hero_pouch_slots` deep at most, so re-sending
it costs less than the desync a dropped delta would cause.

**Direction:** S2C (to the owning player only — a pouch is not party-visible state).

**Payload**

| Field | Type | Required | Nullable | Default | Description |
|-------|------|----------|----------|---------|-------------|
| pouches | array of PouchView | Yes | No | — | One per hero, in party-slot order. |

**PouchView**

| Field | Type | Required | Nullable | Default | Description |
|-------|------|----------|----------|---------|-------------|
| hero_slot | int | Yes | No | — | 0-based party slot this pouch belongs to. |
| items | array of ItemStack | Yes | No | — | What that hero is carrying. |
| capacity | int | Yes | No | — | Slots this pouch holds (`[runs] hero_pouch_slots`), so the client can render `3/10` without reading balance. |

**Example**

```json
{"type": "run.pouches", "seq": 88, "ts": 1783729011000, "payload": {"pouches": [{"hero_slot": 0, "items": [{"item_id": "…", "item_kind": "bloom_salve", "quantity": 2}], "capacity": 10}, {"hero_slot": 1, "items": [], "capacity": 10}]}}
```

---

### `run.cancel_harvest` (C2S)

Puts the tool down on purpose, keeping every unit already banked.

**Source:** roadmap `MS-2`.
**Direction:** C2S — legal only while the sender is harvesting.

**Payload** — empty object `{}`.

**Server validation** — no active harvest channel → the request is a no-op (no error), so a
client may send it defensively.

**Results in** — `run.channel_interrupted` (`reason: "cancelled"`); `avatar_state` returns
to `active`. Units already handed over are **not** clawed back.

**Example**

```json
{"type": "run.cancel_harvest", "seq": 519, "ts": 1783729003000, "payload": {}}
```

---

### `run.watch_battle` (C2S)

WATCH the nearest fight in reach without entering it (roadmap `SOC-3`).

Joining is a commitment — `run.join_battle` puts the caller's whole party in the ATB queue,
splits the encounter XP, and can get their heroes killed. So the only way to learn whether
the party over there was winning used to be walking into it. Watching costs nothing, which
is why its radius is the wider of the two.

**Source:** roadmap `SOC-3`; `[ai] watch_radius`.
**Direction:** C2S — legal only while the sender is on the overworld and not in a fight.

**Payload** — empty object `{}`.

**Target selection** — the **nearest** of two kinds, compared in one pass so a player brawl
standing beside a creature brawl does not resolve by which check ran first:

- another player's battle within `watch_radius` (excluding one the caller's own party is in,
  and excluding dungeon battles), or
- a creature-vs-creature **clash** (`CR-2`) within `watch_radius`.

**Server validation**

| Condition | Result |
|-----------|--------|
| Sender is inside a dungeon | `INVALID_STATE` — a dungeon is a committed space with its own screen. |
| Sender has no active run | `NOT_FOUND` |
| Sender's party is already in a battle | `INVALID_STATE` — you cannot watch and swing. |
| Nothing within `watch_radius` | `OUT_OF_RANGE` |
| Already watching this same feed | No-op (no message), so the client may fire it off a key without rebuilding its battle screen every press. |

**Results in** — `battle.started` with `spectating: true`, an empty `your_combatant_ids`,
and (for a clash) a `clash:<creature_id>` battle id. From that point the watcher is on the
battle's own audience funnel and receives every message a participant does — turn-ready,
gauge updates, telegraphs, resolutions — but **not** `battle.ended`, which carries somebody
else's XP and haul. The feed closes with `battle.watch_ended`.

**Example**

```json
{"type": "run.watch_battle", "seq": 622, "ts": 1783729400000, "payload": {}}
```

---

### `run.stop_watching` (C2S)

Stop watching whatever fight this session was watching.

**Source:** roadmap `SOC-3`.
**Direction:** C2S.

**Payload** — empty object `{}`.

**Server validation** — a caller watching nothing is a **no-op, not an error**: the client
toggles this off the same key that opened the feed.

**Results in** — `battle.watch_ended` (`reason: "stopped"`).

**Example**

```json
{"type": "run.stop_watching", "seq": 640, "ts": 1783729480000, "payload": {}}
```

---

### `run.build_station` (C2S)

Raises a **field workstation** where the avatar stands (roadmap `MS-1`) — a smith's forge
out in the maze, so a profession is a role *during* a dive rather than something you do
between them.

**Source:** roadmap `MS-1`; [`proposals/crafting-and-professions.md`](../../proposals/crafting-and-professions.md).
**Direction:** C2S — legal only while in a run and not in a battle.

**Payload**

| Field | Type | Required | Description |
|---|---|---|---|
| kind | string (enum: `smith`, `alembic`) | Yes | Which bench to raise: a smith's forge (built from **ore**, gated on Forging) or a Keeper's still (built from **reagents**, gated on Alchemy). Anything else is a validation error. |

**Server validation** — the builder's persistent level in the bench's skill must be at
least `[forge] station_min_forging_level` / `station_min_alchemy_level`; they must be
carrying at least `[forge] station_ore_cost` of a single material of the bench's class
(**ore** for a forge, **reagent** for a still — the deepest such stack is spent first); and
there must be no live station already within `[forge] station_radius` on the same
elevation. Each refusal says which of those it was.

**Results in** — the stock leaving the backpack (`run.backpack_update`,
`cause: "station"`) and a **channel** (`run.channel_started`, `method: "build:<kind>"`,
`[forge] station_setup_ms`). The bench appears in `world.snapshot` as
`station:<kind>:<jobs_left>` for **everyone** in the instance only when that channel
completes; movement, a battle or `run.cancel_harvest` interrupt it and the stock stays
spent, because the materials went into the ground. A station with no jobs left is simply
absent from the snapshot.

### `run.teardown_station` (C2S)

Packs up a bench you raised — its own channel (`[forge] station_teardown_ms`), and a bench
with work still in it hands back `station_teardown_refund` of **the same stock it was built
from**. Anyone may *work* at a station; only its owner may take it down (`409`-equivalent
refusal otherwise).

| Field | Type | Required | Description |
|---|---|---|---|
| entity_id | string | Yes | The bench to pack up. |

**Results in** — `run.channel_started` (`method: "pack:<kind>"`), then the bench leaving
the snapshot and a `run.backpack_update` for the salvage.

**Example**

```json
{"type": "run.build_station", "seq": 540, "ts": 1783729020000, "payload": {"kind": "smith"}}
```

---

### `run.smith_request` (C2S)

Asks the smith whose station this is to work a piece of the **requester's own** gear.
Anyone standing at a station may ask — the station is the permission, and its **owner's**
Forging level is the skill the job is done at (they also take the Forging XP).

**Source:** roadmap `MS-1`.
**Direction:** C2S — legal only while in a run, not in a battle, and standing within
`[forge] station_radius` of a live station on the same elevation.

**Payload**

| Field | Type | Required | Description |
|---|---|---|---|
| entity_id | string | Yes | The station being worked at. |
| gear_id | string (uuid) | For the smith's services | Gear the **sender** owns. A piece owned by anyone else answers as though it does not exist. Ignored by a brew. |
| recipe | string | For `brew` | The recipe to cook, at a Keeper's alembic. |
| service | string (enum: `reroll`, `repair`, `enhance`, `brew`, `tonic`) | Yes | Which service. A **forge** does `reroll` / `repair` / `enhance`; a **still** does `brew` / `tonic` (the still's answer to an edge: +atk/+def/+regen across the whole party for this dive). `enhance` puts a temporary edge on a piece a hero is **wearing** that lasts the rest of the dive — never a Vault write, so it cannot carry power home; it is refused in town, where there is no dive to spend it on. |
| material | string | For `reroll` | Material to spend on the re-draw; ignored by a repair. |

**Server validation** — the bench decides what may be asked of it: a **forge** does
`reroll` / `repair` / `enhance`, a **still** does `brew`, and anything else is refused. The
smith's services keep the same tier rules as the HTTP anvil (`repair` needs an **insured**
piece; `reroll` refuses **ephemeral**) and the same costs (`reroll` eats
`reroll_material_cost + reroll_material_per_tier × tier`). A `brew` is gated on the
**station owner's** Alchemy level against the recipe's own `min_level`, and spends the
requester's reagents. The station must have a job left. **Ownership never moves**: every
Vault call is scoped to the sender's own player id, so a station cannot reach into another
player's gear or stock.

**Results in** — `run.tempo_started`: the work is a **heat**, not an instant. The smith
strikes with `run.strike`, and `run.smith_result` arrives once the last blow lands (or the
heat's window runs out). A station job is only spent when work actually happened.

### `run.tempo_started` (S2C)

The heat is open. The bar is **red** and a marker sweeps it once per `sweep_ms`; each blow
has one **yellow** band to strike on. The server laid the bands out from a seed it picked
and it is the only thing that grades a blow.

| Field | Type | Description |
|---|---|---|
| job_id | string | Identifies this heat, echoed on every `run.strike`. |
| service | string | `reroll`, `repair` or `enhance`. |
| strikes | integer (int32) | How many blows the piece takes. |
| sweep_ms | integer (int64) | One full pass of the marker. |
| bands | array of `{lo, hi}` | The yellow, one band per blow, as fractions of the bar (`0.0`–`1.0`). |

Difficulty is the **piece minus the smiths**: `[tempo]` narrows the band and speeds the
sweep with the piece's tier, and widens/slows it again with the smith's Forging level and
every other smith in the party (`extra_smiths_max` caps the help).

### `run.strike` (C2S)

A blow, at the marker's position when the player struck.

| Field | Type | Description |
|---|---|---|
| job_id | string | The open heat. |
| at | number (double) | Where the marker was, `0.0`–`1.0`. Clamped server-side. |

Only the heat's owner may strike it. Blows past the last one are **ignored** — spam can
neither raise nor lower a heat's quality — and a heat left unstruck is graded on what
arrived once `sweep_ms × strikes + grace_ms` has passed.

**Example**

```json
{"type": "run.smith_request", "seq": 541, "ts": 1783729021000, "payload": {"entity_id": "station-smith-0", "gear_id": "0195d001-aaaa-7abc-8f01-23456789abcd", "service": "reroll", "material": "dune_ingot"}}
```

---

### `run.smith_result` (S2C)

What the smith did, or why they would not — one line, already written for the player.

**Direction:** S2C — to the requester only.

**Payload**

| Field | Type | Description |
|---|---|---|
| player_id | string | The requester. |
| entity_id | string | The station. |
| gear_id | string (uuid) | The piece asked about. |
| service | string | `reroll` or `repair`. |
| ok | boolean | Whether work happened. |
| message | string | Player-facing sentence (what changed and what it cost, or the refusal). |
| uses_left | integer (int32) | Jobs the station has left — `0` means it is spent and gone from the snapshot (and is always `0` for the city anvil, which has none). |
| quality | number (double) | The heat's quality, `0.0`–`1.0`: the blows that landed on yellow. What it bought depends on the service — the affix pool a re-draw rolled from, the points a repair gave back, the size of an edge. |

**Example**

```json
{"type": "run.smith_result", "seq": 88, "ts": 1783729021500, "payload": {"player_id": "0195c9a2-1111-7c1a-9b3e-5f6a7b8c9d01", "entity_id": "station-smith-0", "gear_id": "0195d001-aaaa-7abc-8f01-23456789abcd", "service": "reroll", "ok": true, "message": "re-drew Novice Blade for 3 dune_ingot and 90c", "uses_left": 3}}
```

---

### `run.channel_started` (S2C)

A channel began; the channeling avatar is visible and vulnerable for the duration. Covers
**extraction** and **harvesting** (`MS-2`) — `method` distinguishes them.

**Source:** GDD.md §2.2; CANON.md D15 (10 s interruptible channel); roadmap `MS-2` for the
harvest channel ([behaviors/run-lifecycle.md](../../behaviors/run-lifecycle.md) "Flow: Harvesting").
**Direction:** S2C — broadcast to all instance members (carries `client_seq` on the channeler's copy). A `world.presence_update` (`avatar_state: "channeling"`) accompanies it.

**Payload**

| Field | Type | Required | Nullable | Default | Description |
|-------|------|----------|----------|---------|-------------|
| client_seq | integer (int64, u32 range) | Yes | Yes | — | Echo of the starting request's seq on the channeler's copy; `null` on others. |
| player_id | string (uuid) | Yes | No | — | Who is channeling. |
| method | string | Yes | No | — | `portal` / `town_portal` / `escape_item` for extraction, or **`harvest:<node_kind>`** for a gather. |
| completes_at | integer (int64, u64) | Yes | No | — | Unix millis when the channel completes if uninterrupted. Extraction: start + the channel duration **[TUNABLE]**. Harvest: when the node would run *dry* — a horizon, not a promise, since a gather pays out along the way. |
| fill_ms | integer (int64, u64) | No | No | `0` | Milliseconds per **payout** — how long one fill of the client's progress bar takes. Extraction fills once and completes; a harvest refills per unit banked (`[harvest] *_tick_ms`). `0` = unknown, draw no bar. |

**Example — extraction**

```json
{"type": "run.channel_started", "seq": 5210, "ts": 1783729000040, "payload": {"client_seq": 480, "player_id": "0197a2f0-11aa-7bbb-8ccc-0d1e2f3a4b5c", "method": "escape_item", "completes_at": 1783729010040, "fill_ms": 10000}}
```

**Example — harvest** (a 3-unit reagent patch at 900 ms a unit)

```json
{"type": "run.channel_started", "seq": 5211, "ts": 1783729000040, "payload": {"client_seq": 481, "player_id": "0197a2f0-11aa-7bbb-8ccc-0d1e2f3a4b5c", "method": "harvest:bloom_herb", "completes_at": 1783729002740, "fill_ms": 900}}
```

---

### `run.channel_interrupted` (S2C)

An extraction channel broke before completing.

**Source:** CANON.md D15 (interruptible); GDD.md §5 (enemies keep acting).
**Direction:** S2C — broadcast to all instance members. Interruption is server-decided; the channel breaks the moment any interrupting event lands.

**Payload**

| Field | Type | Required | Nullable | Default | Description |
|-------|------|----------|----------|---------|-------------|
| player_id | string (uuid) | Yes | No | — | Whose channel broke. |
| reason | string (enum: `damage_taken`, `battle_started`, `moved`, `cancelled`, `disconnected`, `exhausted`) | Yes | No | — | What ended it: any damage; being pulled into **or opting into** a battle; any accepted movement intent; explicit `run.cancel_extraction` / `run.cancel_harvest`; the channeler's grace window expiring; or — harvest only — the node **running out of stock** (`exhausted`, a natural end rather than a break). |

**Harvest note.** A broken *extraction* banks nothing. A broken *harvest* keeps every unit
already handed over and loses only the tick in flight — extraction is one atomic event,
harvesting is many small ones. See [behaviors/run-lifecycle.md](../../behaviors/run-lifecycle.md)
"Flow: Harvesting".

**Example**

```json
{"type": "run.channel_interrupted", "seq": 5214, "ts": 1783729006000, "payload": {"player_id": "0197a2f0-11aa-7bbb-8ccc-0d1e2f3a4b5c", "reason": "battle_started"}}
```

---

### `run.abandon` (C2S)

Explicitly abandons the sender's run (quit to Hub without extracting).

**Source:** CANON.md §G (`Run` ends in `extracted`, `died`, or `abandoned`), §B (auto-abandon semantics: counts as death for backpack, no durability loss — explicit abandon mirrors this, a canon gap resolved by this spec).
**Direction:** C2S — legal in an active run, out of battle. Per-player: the rest of the party keeps playing.
**Idempotency:** Non-idempotent; `invalid_state` once the run has ended.

**Payload** — empty object `{}`.

**Server validation** — not in an active run → `invalid_state`; in battle → `invalid_state` ("Resolve or flee the battle first.").

**Results in** — the sender's backpack and run level are deleted (as death), blue-chest gear returns **without** durability loss; `run.member_result` (`result: "abandoned"`) broadcast; the avatar despawns and the player returns to the departure hub.

**Notes**

- The despawn is broadcast as `world.entity_despawn` with `reason: "extracted"` — visually identical to an extraction departure; the outcome difference is carried by `run.member_result`.

**Example**

```json
{"type": "run.abandon", "seq": 512, "ts": 1783729100000, "payload": {}}
```

---

### `run.member_result` (S2C)

A party member's run reached its terminal state: `extracted`, `died`, or `abandoned`.

**Source:** GDD.md §2.2 (the Choice: extract or die); CANON.md §G (`Run`, `Backpack`), §B (death & durability), D15.
**Direction:** S2C — broadcast to all instance members. The affected member's own copy additionally carries the private `banked` / `lost` summary; other members receive `null` there.

**Payload**

| Field | Type | Required | Nullable | Default | Description |
|-------|------|----------|----------|---------|-------------|
| run_id | string (uuid) | Yes | No | — | The run. |
| player_id | string (uuid) | Yes | No | — | Whose run ended. |
| result | string (enum: `extracted`, `died`, `abandoned`) | Yes | No | — | Terminal outcome. |
| max_distance_reached | integer (int32, ≥ 0) | Yes | No | — | The member's deepest `floor` distance this run — the Vanguard Board input (CANON.md D3; board itself is HTTP-owned). |
| banked | array of ItemStack | Yes | Yes | — | `extracted` only, own copy only: the backpack contents banked into the Vault (red-chest gear becomes owned Vault gear, still `red` tier). `null` otherwise. **Handoff:** the banking itself is a server-side persistent mutation; the Vault's new state is read via the HTTP API. |
| lost | array of ItemStack | Yes | Yes | — | `died`/`abandoned`, own copy only: the deleted backpack contents. `null` otherwise. |
| durability_loss_applied | boolean | Yes | No | — | Whether blue-chest gear lost max durability: `true` on `died` (−10% of current max, round down, floor 0 — CANON.md D6/§B, applied server-side, visible via HTTP), `false` on `extracted` and `abandoned`. |

**Ordering** — for a death this message follows the terminal `battle.ended` (`outcome: "defeat"`); for extraction it follows the channel completing (no separate "channel completed" message — this is it).

**Example — extraction (own copy)**

```json
{"type": "run.member_result", "seq": 5300, "ts": 1783729010060, "payload": {"run_id": "0197a610-0001-7abc-9def-0123456789ab", "player_id": "0197a2f0-11aa-7bbb-8ccc-0d1e2f3a4b5c", "result": "extracted", "max_distance_reached": 742, "banked": [{"item_id": "0197a602-8888-7abc-9def-0123456789ab", "item_kind": "iron_ore", "quantity": 3, "insurance": null}], "lost": null, "durability_loss_applied": false}}
```

---

### `run.instance_closed` (S2C)

The MazeInstance shut down; all remaining ephemeral state in it is gone.

**Source:** CANON.md §B (instance closes when all members extracted/died/abandoned, or after 60 min with all members disconnected → sleeping avatars auto-abandon: counts as death for the backpack, **no** durability loss).
**Direction:** S2C — sent to any instance member still connected (typically none for the timeout case; the message matters for spectating party members whose own runs already ended but whose session persists).

**Payload**

| Field | Type | Required | Nullable | Default | Description |
|-------|------|----------|----------|---------|-------------|
| instance_id | string (uuid) | Yes | No | — | The closed instance. |
| reason | string (enum: `all_members_resolved`, `idle_timeout`) | Yes | No | — | `all_members_resolved`: every member reached a terminal result. `idle_timeout`: 60 min **[TUNABLE]** with all members disconnected; every still-sleeping avatar was auto-abandoned (a `run.member_result` with `result: "abandoned"` per member precedes this message). |

**Example**

```json
{"type": "run.instance_closed", "seq": 5400, "ts": 1783732700000, "payload": {"instance_id": "0197a610-0002-7abc-9def-0123456789ab", "reason": "all_members_resolved"}}
```

---

### `run.backpack_update` (S2C)

Authoritative delta to the recipient's own backpack — the single source of truth for ephemeral inventory.

**Source:** GDD.md §2.2 (the Backpack); CANON.md §G (`Backpack` — per-player ephemeral run inventory), §S (ephemeral state flows over realtime).
**Direction:** S2C — sent to the owning player only, whenever backpack contents change: battle loot, ground pickup, ground drop, item consumed (battle item, ward deployment, escape item, drop-on-player). Clients apply deltas in envelope `seq` order on top of the `run.started` baseline; equal-and-opposite client prediction must be reconciled to this message.

**Payload**

| Field | Type | Required | Nullable | Default | Description |
|-------|------|----------|----------|---------|-------------|
| changes | array of object (min 1 item) | Yes | No | — | Item-level deltas. |
| changes[].item | ItemStack | Yes | No | — | The affected item; `quantity` is the magnitude of the change. |
| changes[].delta | string (enum: `added`, `removed`) | Yes | No | — | Direction of the change. |
| changes[].cause | string (enum: `battle_loot`, `picked_up`, `dropped`, `consumed`, `banked`, `deleted`) | Yes | No | — | Why. `banked` (extraction) and `deleted` (death/abandon) always empty the backpack and coincide with the `run.member_result`. |

**Example**

```json
{"type": "run.backpack_update", "seq": 5120, "ts": 1783728150001, "payload": {"changes": [{"item": {"item_id": "0197a602-8888-7abc-9def-0123456789ab", "item_kind": "iron_ore", "quantity": 3, "insurance": null}, "delta": "added", "cause": "battle_loot"}]}}
```

---

### `run.hunt_progress` (S2C)

A posted hunt moved (roadmap `AD-4`; behavior: [behaviors/hunt-board.md](../../behaviors/hunt-board.md)).

**Source:** [proposals/adventure-depth.md](../../proposals/adventure-depth.md) §E; CANON.md §S (progress is server-owned — the client is told, never asked).
**Direction:** S2C — sent to the player whose progress changed, as it changes, so a hunt is something you watch fill rather than something you find finished on your next walk past the board. Several credits in one tick send one message each; `complete` is true on exactly one of them, ever.

**Payload**

| Field | Type | Required | Nullable | Default | Description |
|-------|------|----------|----------|---------|-------------|
| key | string | Yes | No | — | The hunt's stable id (`meld_proto::hunts`). |
| name | string | Yes | No | — | Display name, so a client that does not know the key can still speak. |
| progress | integer (int32, ≥ 1) | Yes | No | — | Progress after this credit, capped at `target`. |
| target | integer (int32, ≥ 1) | Yes | No | — | What the hunt completes at. |
| complete | boolean | Yes | No | — | This credit finished it; the reward is waiting at the Bounty Board (`POST /v1/hunts/:key/claim`). |

**Example**

```json
{"type": "run.hunt_progress", "seq": 812, "ts": 1783728150001, "payload": {"key": "cull_the_bloom", "name": "Cull the Bloom", "progress": 3, "target": 8, "complete": false}}
```

---

### `social.drop_item` (C2S)

Drops backpack items onto the overworld for anyone to pick up — cooperation, gifting, or paying bodyguards.

**Source:** GDD.md §6 (backpack dropping); CANON.md §S (drops are ephemeral realtime state).
**Direction:** C2S — sent by a player in an active run (hubs are safe zones; ground-dropping in hubs is also allowed, and such drops despawn when the dropper's session ends **[TUNABLE]**).
**Idempotency:** Non-idempotent; `sequence_error` protects against duplicate retries.

**Payload**

| Field | Type | Required | Nullable | Default | Description |
|-------|------|----------|----------|---------|-------------|
| item_id | string (uuid) | Yes | No | — | Backpack item instance to drop. |
| quantity | integer (int32, ≥ 1) | No | No | full stack | How many to drop from the stack. Exceeding the held quantity → `validation_error`. |
| position | Position | No | Yes | dropper's position | Drop point; farther than the 2-tile **[TUNABLE]** interaction range → `out_of_range`. |

**Server validation** — sender in battle, channeling, or sleeping → `invalid_state`; `item_id` not in the sender's backpack → `not_found`; blue-chest (insured) gear cannot be ground-dropped → `forbidden` (vault gear moves only via HTTP trade/stall flows).

**Results in** — atomically: `run.backpack_update` (`removed`/`dropped`) to the dropper and `world.entity_spawn` (`entity_kind: "item_drop"`) to everyone in radius. The success ack **is** the entity spawn. Ground drops persist until picked up or the instance closes.

**Example**

```json
{"type": "social.drop_item", "seq": 520, "ts": 1783728300000, "payload": {"item_id": "0197a5cc-5555-7abc-9def-0123456789ab", "quantity": 2, "position": null}}
```

---

### `social.pickup_item` (C2S)

Picks a ground item drop into the sender's backpack.

**Source:** GDD.md §6; CANON.md §S.
**Direction:** C2S — any active player (any party — drops are not owner-locked; first-come, first-served).
**Idempotency:** Non-idempotent; racing pickups are serialized server-side — the loser gets `not_found`.

**Payload**

| Field | Type | Required | Nullable | Default | Description |
|-------|------|----------|----------|---------|-------------|
| entity_id | string (uuid) | Yes | No | — | The `item_drop` entity to pick up. |

**Server validation** — sender in battle/channeling/sleeping → `invalid_state`; entity unknown or already claimed → `not_found`; entity not an `item_drop` → `validation_error`; farther than the 2-tile **[TUNABLE]** interaction range → `out_of_range`.

**Results in** — atomically: `social.item_picked_up` ack to the picker, `run.backpack_update` (`added`/`picked_up`) to the picker, `world.entity_despawn` (`reason: "picked_up"`) to everyone in radius.

**Example**

```json
{"type": "social.pickup_item", "seq": 531, "ts": 1783728310000, "payload": {"entity_id": "0197a5cc-4444-7abc-9def-0123456789ab"}}
```

---

### `social.item_picked_up` (S2C)

Pickup confirmation carrying exactly what was gained.

**Source:** GDD.md §6; CANON.md §I (ack echoes client seq).
**Direction:** S2C — to the picking player only, as the direct ack of `social.pickup_item`.

**Payload**

| Field | Type | Required | Nullable | Default | Description |
|-------|------|----------|----------|---------|-------------|
| client_seq | integer (int64, u32 range) | Yes | No | — | Echo of the `social.pickup_item` seq. |
| entity_id | string (uuid) | Yes | No | — | The consumed drop entity. |
| items | array of ItemStack (min 1 item) | Yes | No | — | Items added to the backpack (also mirrored in `run.backpack_update`). |

**Example**

```json
{"type": "social.item_picked_up", "seq": 5600, "ts": 1783728310030, "payload": {"client_seq": 531, "entity_id": "0197a5cc-4444-7abc-9def-0123456789ab", "items": [{"item_id": "0197a5cc-5555-7abc-9def-0123456789ab", "item_kind": "health_potion", "quantity": 2, "insurance": null}]}}
```

---

### `social.drop_item_on_player` (C2S)

Drops a consumable from the sender's backpack directly onto another player's overworld sprite while that player is in a battle — the server intercepts it and injects the effect into their ATB subscreen.

**Source:** GDD.md §6 (real-time influence: health potion on Player A's active battle sprite → instant heal inside the subscreen); CANON.md §S.
**Direction:** C2S — sent by a player **on the world map** (not in battle themselves). The target may be in any party/instance sharing the overworld space.
**Idempotency:** Non-idempotent; `sequence_error` protects retries.

**Payload**

| Field | Type | Required | Nullable | Default | Description |
|-------|------|----------|----------|---------|-------------|
| target_player_id | string (uuid) | Yes | No | — | The battling player whose sprite the item is dropped on. |
| item_id | string (uuid) | Yes | No | — | Backpack consumable to use. Must be battle-usable (e.g. `health_potion`); one unit is consumed. |

**Server validation** (in order)

1. Sender in battle, channeling, or sleeping → `invalid_state` (the dropper must be on the world map, GDD.md §6).
2. `item_id` not in the sender's backpack → `not_found`; not a battle-usable consumable → `validation_error`.
3. Target avatar unknown / not in the sender's interest radius → `not_found`.
4. Target's `avatar_state` is not `in_battle` → `invalid_state` ("Target is not in a battle." — to hand an idle player an item, use `social.drop_item` at their feet instead).
5. Target sprite farther than the 2-tile **[TUNABLE]** interaction range → `out_of_range`.

**Results in** — atomically: one unit consumed (`run.backpack_update`, `removed`/`consumed`, to the sender), `social.drop_applied` ack to the sender, and `battle.external_effect` broadcast inside the target's battle ([battle.md](battle.md#battleexternal_effect-s2c)). The effect applies instantly; it does not consume the target's turn or gauge.

**Example**

```json
{"type": "social.drop_item_on_player", "seq": 544, "ts": 1783728118000, "payload": {"target_player_id": "0197a2f0-11aa-7bbb-8ccc-0d1e2f3a4b5c", "item_id": "0197a612-9999-7abc-9def-0123456789ab"}}
```

---

### `social.drop_applied` (S2C)

Confirmation to the overworld dropper that the item's effect landed inside the target's battle.

**Source:** GDD.md §6; CANON.md §I (ack echoes client seq).
**Direction:** S2C — to the dropping player only, as the direct ack of `social.drop_item_on_player`. (The dropper is not a battle participant and never receives `battle.external_effect`.)

**Payload**

| Field | Type | Required | Nullable | Default | Description |
|-------|------|----------|----------|---------|-------------|
| client_seq | integer (int64, u32 range) | Yes | No | — | Echo of the `social.drop_item_on_player` seq. |
| target_player_id | string (uuid) | Yes | No | — | The recipient. |
| item_kind | string | Yes | No | — | The consumed item's content identifier. |
| effect_summary | string (enum: `healed`, `status_applied`, `no_effect`) | Yes | No | — | Coarse outcome for overworld feedback. `no_effect` when the target's battle ended between validation and application (the item is still consumed **[TUNABLE]**). |

**Example**

```json
{"type": "social.drop_applied", "seq": 5700, "ts": 1783728118040, "payload": {"client_seq": 544, "target_player_id": "0197a2f0-11aa-7bbb-8ccc-0d1e2f3a4b5c", "item_kind": "health_potion", "effect_summary": "healed"}}
```
