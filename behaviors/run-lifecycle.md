# Run Lifecycle (Extract or Die)

A `Run` is one ephemeral maze excursion by a `MazeInstance`. The game is split into two strict states: **persistent** (the `Hub`, the `Vault`, Meld Skills, class unlocks — survives everything) and **ephemeral** (the `Backpack`, `run_level`, instance world state — survives nothing except extraction). Every run ends in exactly one of three terminal states: `extracted`, `died`, or `abandoned`. Persistent state is only ever mutated by the server at the moment a run reaches a terminal state; nothing a player does mid-run touches the `Vault`.

**Source:** GDD.md §2, §2.1, §2.2, §4, §4.1; CANON.md §G (Run, MazeInstance, Backpack, Vault, run_level), §B (Hubs & run levels, Death & durability, Disconnect handling), §D6, §D13, §D15, §S

Related: [world-generation.md](./world-generation.md) (portal placement), [combat-atb.md](./combat-atb.md) (how battles end runs), [disconnect-handling.md](./disconnect-handling.md) (abandon path), [../interfaces/http-api.md](../interfaces/http-api.md) (vault/run-history reads), [../interfaces/realtime-protocol.md](../interfaces/realtime-protocol.md) (`run.*` messages).

---

## Run State Machine

Each `Player` in an instance has their own `Run` record; terminal states are per-player (one member may extract while another later dies in the same instance).

**Source:** GDD.md §2.2; CANON.md §G (Run), §B (Disconnect handling)

| From state | Event (trigger) | To state | Side effects |
|------------|-----------------|----------|--------------|
| — (none) | Player departs a Hub into the maze (instance created at maze-entry, D13) | `active` | `run_level` set to `base_run_level(hub)`; empty `Backpack` created; equipped blue-chest `GearItem`s carried into the run |
| `active` | Extraction channel completes (portal or `ripcord_scroll`) | `extracted` | Backpack banked to `Vault`; red gear becomes owned Vault gear; Meld XP credited; run record finalized with max distance reached |
| `active` | Player's battle ends in defeat ([combat-atb.md](./combat-atb.md)) | `died` | Backpack deleted; `run_level` deleted; blue-chest gear returned to Hub at `max_durability × 0.9` (round down) |
| `active` | Sleeping avatar killed by a monster while disconnected ([disconnect-handling.md](./disconnect-handling.md)) | `died` | Same as death above |
| `active` | Player explicitly abandons the run (client-initiated, confirmed) | `abandoned` | Backpack deleted; `run_level` deleted; blue-chest gear returned **without** durability loss (see Note 1) |
| `active` | Instance auto-close: 60 min with **all** members disconnected | `abandoned` | Counts as death for the Backpack (deleted), but **no durability loss** on blue-chest gear |
| `extracted` / `died` / `abandoned` | any | — | Terminal. A run never leaves a terminal state; re-entering the maze creates a new `Run`. |

> **Note 1:** CANON only defines the *auto*-abandon trigger (60-min all-disconnected, CANON.md §B Disconnect handling) and its semantics (backpack deleted, no durability loss). A player-initiated abandon is implied by the `abandoned` terminal state in §G; this spec resolves it to the same semantics as auto-abandon. Flagged as a canon gap — see Edge Cases.

---

## Flow: Entering the Maze

**Source:** GDD.md §2.2, §4; CANON.md §B (Hubs & run levels), §D13, §G

1. A `Party` of 1–4 players forms in a `Hub` (solo players may opt into the matchmaking pool, filtered by departure hub).
2. At maze-entry time the server creates a `MazeInstance` (own world seed — see [world-generation.md](./world-generation.md)) and one `Run` per member in state `active`. The instance is **not joinable afterward** except via battle merge, which merges battles, not instances (CANON.md §D13).
3. Each player's ephemeral combat level is reset: `run_level = base_run_level(hub) = round(1 + hub.distance × 0.078)` **[TUNABLE]** → Center Hub = 1, D500 = 40, D1000 = 79, D5000 = 391. Any run level from a previous run is irrelevant (it was deleted at that run's terminal transition).
4. An empty `Backpack` is created for each player. The Backpack is the only inventory that loot can enter during the run.
5. Equipped blue-chest gear (`GearItem` with `insurance: blue`) is carried into the maze with **insurance semantics**: whatever happens, the item itself returns to the Hub — it can lose max durability on death but is never deleted by a run outcome.
6. Vault-owned red gear (`insurance: red`, previously extracted) may also be equipped and carried in, but it is **not insured**: it is permanently lost if the run ends in `died` (see Edge Cases).
7. Training Ground allocations (skill point templates, GDD.md §4) are applied to the ephemeral combat build before stepping into the maze; they configure the run and are not persistent mutations.

---

## Flow: Extraction

**Source:** GDD.md §2.2, §4.1; CANON.md §D15, §G (Backpack, Vault, Red Chest gear), §S

1. The player initiates extraction by either:
   - standing at an **extraction portal** (deterministic at every Hub, plus ~1 per 200-distance band per instance seed — see [world-generation.md](./world-generation.md)), or
   - using an **escape item** (`ripcord_scroll`), usable from anywhere.
2. A **10 s interruptible channel** begins **[TUNABLE]**. The channel is cancelled if the player moves, takes any other action, or is pulled into a `Battle` (monster touch). A cancelled channel has no effect; it may be restarted. (CANON.md §D15 defines the channel for escape items; this spec applies the same channel to portal extraction for symmetry — flagged in Edge Cases.)
3. On channel completion the server (and only the server — CANON.md §S boundary rule: persistent mutation at run end is performed by the server itself) atomically performs the terminal transition `active → extracted`:
   - **Backpack → Vault:** all Backpack contents (potions, raw materials, plants, monster parts, chits found) are banked into the player's `Vault`.
   - **Red gear → owned Vault gear:** any `insurance: red` `GearItem` in the Backpack or equipped becomes player-owned Vault gear. It remains `insurance: red` — extraction changes ownership, not insurance tier.
   - **Blue gear returns** to the Hub unchanged (no durability penalty on extraction).
   - **Meld XP credit:** extraction success credits `MeldSkill` XP — notably `alchemy` XP for rare plants and monster parts banked (GDD.md §4.1: Meld Skills level up "in the Hubs or through extraction success"). Exact XP amounts are content-table driven **[TUNABLE]**.
   - The run record is finalized (outcome, max `distance` reached — feeding the `VanguardBoard`).
4. The player's avatar leaves the instance and returns to the Hub. `run_level` is discarded (it never persists, regardless of outcome).
5. Extraction is per-player: remaining party members' runs stay `active` in the same instance.

---

## Flow: Death

**Source:** GDD.md §2.2, §7; CANON.md §B (Death & durability), §D6, §G

1. A player's run ends in `died` when their side of a `Battle` is defeated (see [combat-atb.md](./combat-atb.md)) — including a battle started against their sleeping avatar while disconnected.
2. The server atomically performs the terminal transition `active → died`:
   - The `Backpack` and all its contents are **permanently deleted** (including any unextracted red gear, equipped or carried).
   - The accumulated `run_level` is **deleted**.
   - Blue-chest gear is returned to the Hub with `max_durability × 0.9`, rounded down, floor 0 **[TUNABLE]** (CANON.md §D6: −10% of current max durability per death). Gear at 0 max durability is unequippable until repaired by a Forging crafter.
3. Nothing else persistent changes: chits in the `Vault`, Meld Skills, class unlocks, and cosmetics are untouched by death.
4. Death is per-player; the instance and other members' runs continue.

---

## Flow: Abandon

**Source:** CANON.md §B (Disconnect handling), §G (Run); GDD.md §5

1. **Auto-abandon (canonical trigger):** if **all** members of an instance are disconnected for **60 min [TUNABLE]**, the instance closes and every remaining `active` run transitions to `abandoned`. This *counts as death for the Backpack* (deleted, with run levels) but applies **no durability loss** to blue-chest gear — explicitly different from a real death (see [disconnect-handling.md](./disconnect-handling.md)).
2. **Player-initiated abandon:** a connected player may abandon their run (e.g. give up while lost). Semantics mirror auto-abandon: Backpack and run level deleted, blue gear returned with no durability loss. *(Canon gap resolution — see Note 1 above.)*
3. Abandon never banks anything: the only path that moves Backpack contents into the `Vault` is `extracted`.

---

## Instance Close Conditions

**Source:** CANON.md §B (Disconnect handling), §D12; GDD.md §5

The `MazeInstance` (world state, chunks, sleeping avatars, per-instance Gatekeeper clear flags) closes when either:

| Condition | Detail |
|-----------|--------|
| All members terminal | Every member's run is `extracted`, `died`, or `abandoned` |
| 60-min all-disconnected | All members disconnected for 60 continuous minutes **[TUNABLE]** → remaining `active` runs auto-abandon, then the instance closes |

On close, all ephemeral instance state is discarded (see [world-generation.md](./world-generation.md)). A sleeping avatar persists on the overworld only until instance close.

---

## What Persists vs. What Is Ephemeral

**Source:** GDD.md §2, §4.1; CANON.md §G, §S

| Data | Scope | On `extracted` | On `died` | On `abandoned` |
|------|-------|----------------|-----------|----------------|
| `Vault` (chits, materials, gems, owned gear) | persistent | Backpack merged in | untouched | untouched |
| Blue-chest gear (`insurance: blue`) | persistent | returned unchanged | returned at `max_durability × 0.9` (round down) | returned, **no durability loss** |
| Red gear found this run (`insurance: red`, in Backpack) | ephemeral until extracted | becomes owned Vault gear (still `red`) | deleted | deleted |
| Vault-owned red gear carried into the run | persistent but uninsured | returned to Vault | **deleted** | deleted *(gap: see Edge Cases)* |
| `Backpack` contents | ephemeral | banked to Vault | deleted | deleted |
| `run_level` | ephemeral | discarded | deleted | deleted |
| `MeldSkill` levels/XP | persistent | XP credited | untouched | untouched |
| Class unlocks (`ClassEmblem` applied), cosmetics | persistent | kept | kept | kept |
| Instance world state, clear flags | ephemeral | discarded at instance close | discarded at instance close | discarded at instance close |

---

## Invariants

1. **Terminal-only persistence:** no persistent state (`Vault`, gear durability, Meld XP) is mutated by run activity except at a run's terminal transition, and that mutation is performed atomically by the server (never by client request mid-run).
2. **Vault untouched mid-run:** the `Vault` is read-only with respect to an `active` run; loot flows only into the `Backpack`.
3. **Exactly one terminal state:** every `Run` ends in exactly one of `extracted` | `died` | `abandoned`; terminal states are absorbing.
4. **Insurance:** `insurance: blue` gear is never deleted by any run outcome; `insurance: red` gear never survives `died`/`abandoned` unless it was already Vault-owned *and* the outcome is `extracted`.
5. **Extraction is the only banking path:** `died` and `abandoned` never move anything into the `Vault`.
6. **Run level never persists:** `run_level` is discarded/deleted at every terminal transition; the next run starts at `base_run_level(hub)`.
7. **Per-player terminality, per-instance world:** run outcomes are per-player; world/instance state outlives individual outcomes until the instance close conditions are met.
8. **Boundary rule (CANON.md §S):** run-end persistent mutations are server-internal; the realtime channel only announces them (`run.*` S2C messages), and clients cannot trigger vault writes over the realtime protocol.

---

## Edge Cases

- **Death during extraction channel:** being pulled into a battle interrupts the channel; if the ensuing battle is lost, the outcome is `died` — a partially-completed channel banks nothing.
- **Simultaneous portal + battle touch:** server processes battle creation first (battle pull cancels the channel); extraction requires the player to be out of battle.
- **Extraction at the Center Hub portal immediately after entry:** legal; banks an empty Backpack, credits no meaningful Meld XP, ends the run `extracted`.
- **Vault-owned red gear on death:** CANON.md §G says red gear is "lost on death unless extracted (extraction converts it to owned Vault gear, still red tier)". This spec reads ownership as *not* conferring insurance: carrying owned red gear into a new run risks it permanently. **Canon ambiguity — flagged for design confirmation.**
- **Portal channel interruptibility:** CANON.md §D15 specifies the 10 s interruptible channel for escape items only; portal extraction channel duration is a spec-level resolution (same 10 s) **[TUNABLE]** — flagged for design confirmation.
- **Player-initiated abandon trigger:** not defined in CANON (only the 60-min auto path); resolved here as identical semantics — flagged for design confirmation.
- **Durability floor:** repeated deaths apply `× 0.9` (round down) each time, floor 0; at 0 the item is unequippable but still owned and repairable (`base_max × (0.5 + forging_L/198)` cap — economy spec).
- **Disconnected but not yet abandoned:** a disconnected player's run remains `active` (avatar `sleeping`); it becomes terminal only via monster kill (`died`), the 60-min all-disconnected close (`abandoned`), or reconnect-and-finish. See [disconnect-handling.md](./disconnect-handling.md).
- **Last member extracts while another sleeps:** the instance does *not* close while a sleeping member's run is still `active`; close requires all members terminal or the 60-min all-disconnected timer (a sleeping member is disconnected, so if everyone else is terminal-and-gone the 60-min clock governs).
- **XP-driven run-level-up mid-run:** `run_level` grows with battle XP (`xp_to_next(L) = 80 × L^1.6`, no cap) but is still deleted at any terminal transition — leveling during a run is never persistent.
