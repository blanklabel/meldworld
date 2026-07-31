# Server scaling — interest indexing, sim/IO split, instance sharding

> Status: **PROPOSAL** (nothing here is built). A staged plan for lifting the
> authoritative server's concurrency ceiling *without* abandoning the property
> that makes it correct — **one task owns each world's ephemeral state, so there
> are no locks** (CANON §S). Written against the real code:
> `meld-server::game.rs` (the game loop, `snapshot_msgs`, `ActiveInstance`),
> `meld-world::Arena` (`step_creatures_with_aggro`), and the `meld-proto` wire
> types. Companion to the [CR-4](../ROADMAP.md) sim-budget guardrail and the
> [Last City](last-city.md) ward-sharding plan; this doc is the server-side
> half those depend on.

## TL;DR

- Today: **no admission cap**; one Tokio task drives **one global `MazeInstance`**
  at 10 Hz. Realistic ceiling ≈ a few dozen comfortable, degrading in the low
  hundreds, falling behind its own tick in the mid-hundreds-to-~1000.
- The binding cost is **not** the world sim (that's O(creatures), once per tick,
  already spatial-hashed). It's the **per-client overworld snapshot**, which is
  **O(sessions × entities)** with a clone + per-recipient serialize every tick.
- Four levers, in cost order. Do the first regardless; it's free money.

| Lever | Scales | Rough cost | When |
|---|---|---|---|
| **A. Interest index** (chunk grid) | one world's fan-out, algorithmically | days | now, always |
| **B. Sim/IO split** (in-process) | one world across CPU cores | ~1 week | when one core's tick is full |
| **C. Instance sharding** | number of independent worlds | ~2 weeks | when you want many mazes/towns |
| **D. Cross-process sim + gateways** | one world across machines | large | only when one box can't hold it |

## The problem (confirmed in code)

The authoritative loop is one `tokio::select!` over an inbound `mpsc(1024)` and a
`tokio::time::interval` at `battle.tick_ms` = 100 ms
([`game.rs`](../../server/crates/meld-server/src/game.rs) `GameState::run`). It's
well-built for a single owner: DB writes are off-loop (unbounded `db_writes` →
`run_db_writer`), broadcasts serialize **once** into an `Arc<RawValue>` and hand
each session a cheap clone, and a slow client is `try_send`-dropped so a stalled
socket never blocks the loop.

Two facts cap it:

1. **One global instance.** State is `instance: Option<ActiveInstance>` — a single
   world for the whole server — and it's torn down the moment it empties
   (`if inst.run.runs.is_empty() { self.instance = None; }`). Everyone, including
   `solo` divers, shares it. This is a documented slice simplification, not a
   design intent.

2. **The snapshot is the hot loop.** `snapshot_msgs` builds the full entity list
   once (O(E)), then **for every roaming player** re-scans that whole list,
   distance-filters to the interest radius, **clones** the survivors, and
   serializes a per-recipient message:

   ```rust
   // meld-server/src/game.rs — per player, every 100 ms
   Some(p) => entities.iter()
       .filter(|e| /* d2 <= radius2 */)
       .cloned()
       .collect(),
   ```

   That's the **O(sessions × E)** term, ten times a second. Interest radius =
   `interest_radius_chunks × chunk_size` = 2 × 64 = **128 tiles**, so what each
   player *receives* is bounded — but the *scan* is over the whole world and the
   *outer loop* is linear in connected players.

Everything below attacks these two facts in increasing order of cost, and each
stage is independently shippable and independently valuable.

---

## Lever A — Interest index (chunk grid): free money, do it first

The snapshot cull is a 2-D range query ("entities within 128 tiles of me") done as
a full linear scan. Replace it with a **uniform chunk grid / spatial hash**:
bucket entities by `(x / chunk_size, y / chunk_size)`, and each player's query
visits only the ~(2r+1)² ≈ 25 cells around them instead of all E.

**We already own this pattern.** `Arena::step_creatures_with_aggro` builds exactly
this to find skirmish targets:

```rust
// meld-world/src/lib.rs — the idiom already exists one layer down
let mut grid: HashMap<(i32, i32), Vec<usize>> = HashMap::new();
// …each creature scans only its 3×3 cell neighbourhood…
```

The snapshot is simply the one hot loop that never got the treatment. Turning the
per-player `.filter` over all entities into a lookup of ~25 cells converts
**O(sessions × E)** → **O(sessions × visible)**.

**Bonus — per-chunk serialize caching.** Once bucketed, the *bytes* for a chunk's
entities are identical for every player who sees that chunk. Serialize each visible
chunk once per tick and reuse the `Arc` across every viewer — this reclaims the
"serialize-once" win that `snapshot_msgs` currently forfeits (it falls back to
per-recipient `out_msg` precisely because each player's set differs). Grid +
per-chunk byte cache ≈ most of the fan-out cost gone, on one core, no new
processes.

> A literal B-tree is the wrong shape here (it's 1-D ordered; this is a 2-D range
> query). The tree-flavoured version is a `BTreeMap` keyed by a Morton/Z-order
> code — but the flat grid is simpler and strictly better, and the cell size is
> already chosen for us by `interest_radius_chunks`.

**Estimated headroom:** likely 5–10× on its own, which may defer every heavier
stage. This is the highest leverage-to-effort item in the whole doc.

---

## Lever B — Sim/IO split (in-process): one world across cores

Today one task both **simulates** and **fans out**. Separate them:

1. The instance task runs **only** the sim (apply intents, `step_creatures`,
   battle ticks) and publishes an **immutable `Arc<WorldSnapshot>`** each tick.
2. A pool of **serialization workers** takes that read-only snapshot and does the
   cull + serialize + send for their slice of players, in parallel across cores.

Because a tick's published state is immutable, the fan-out parallelizes trivially
— no locks, no contention, the single-owner invariant is untouched (one task still
*mutates* the world; the workers only *read* a frozen copy). This gets N-core
scaling on **one machine** with none of the distributed-systems tax, and it
decouples **sim cadence** from **snapshot cadence** (useful for Lever-A-adjacent
work and for projectiles — see Forward-compatibility).

---

## Lever C — Instance sharding: many independent worlds

The domain is already partitioned: a `MazeInstance` is a self-contained seeded
world (`Arena`, `runs`, `battles`). Players who share a maze *interact* (they see
each other, `run.join_battle` pulls a nearby ally, battles merge), so the shard
boundary is the **instance**, never the player.

Refactor `GameState` into two roles:

```
gateway sockets ──▶  Router task  ──▶  WorldActor (world/realm A)  ─┐
                     (routing +        WorldActor (world/realm B)   ├─▶ shared DbWriter ─▶ Postgres
                      matchmaking)      WorldActor (The Last City)  ─┘
```

- **`WorldActor`** — ~95% of today's `GameState`, scoped to one world (its
  overworld + players + monsters + towns), with its own 100 ms tick. The **"one
  task, no locks" invariant survives verbatim** — it just becomes *one task per
  world*. (A persistent world hibernates to Postgres when empty rather than tearing
  down; see the town analysis below.)
- **`Router`** — owns the session registry (`player_id → (out_tx, instance)`) and
  the instance table; forwards each `ServerEvent::Client` with an O(1) lookup. It
  is **not on any tick**, so it costs O(1) per message and won't bottleneck for a
  long time. It also handles instance spawn/teardown and matchmaking.
- **Handoff** — a player moving between mazes (extract → new dive) is a Router
  transaction: sit in a hub between dives, then hand the `Session` (a cheap struct;
  `out_tx` is a clonable `mpsc::Sender`) to the target actor and re-point the
  routing entry. No socket churn; the gateway writer never notices.
- **Shared state is already minimal** — Postgres is the only cross-instance state,
  and the `db_writes` path is already off-loop. Keep **one** shared `DbWriter`.
- **Bonus: failure isolation** — a panic in one instance takes down that maze, not
  the server; the Router can rebuild it.

Load after sharding, with a per-instance cap M and K = N/M instances:
**O(N × M)** instead of **O(N²)** — linear in total players for fixed M.

---

## Lever D — Cross-process sim + gateways: one world across machines

Only when a single box can't hold the population. Keep the sim **central and
authoritative**; push the **per-client fan-out** out to **gateway** processes next
to the sockets. Gateways hold sockets, forward intents up, receive per-tick world
deltas, cull + serialize locally, and scale horizontally.

This game is unusually suited to it:

- **Determinism makes handoff cheap.** `meld-world` / `meld-battle` are pure,
  seeded, and do zero I/O — an instance's entire state is a plain serializable
  struct with no sockets/DB handles baked in. So a **live instance can migrate
  between machines** by shipping state + seed (or replaying its ordered input log)
  — the nightmare of most stateful game servers is, here, a `serde` call.
- **The clock is forgiving.** 100 ms tick + 15 s ATB window (turn-based) means an
  extra network hop client→gateway→sim→gateway is imperceptible. An FPS couldn't;
  this can.
- **The wire types mostly exist** — `SnapshotEntity` + the S2C messages are the
  raw material for the internal sim↔gateway link.

Cost is real (service discovery, an internal binary protocol, sim-server failure
handling), so it's explicitly last.

---

## Forward-compatibility with planned gameplay

Two features on the horizon stress the overworld sim. Both were checked against
this plan; the summary is **the two cheapest levers (A + B) are exactly what they
want**, with one caveat each.

### Traps + NPC archers (active overworld hazards affecting creatures *and* players)

Real-time projectiles, traps, and NPC shooters that deal damage in the overworld
(not just inside turn-based battles). **The plan helps, it doesn't hurt:**

- **Lever A helps directly.** Projectile-vs-target and trap-trigger tests are
  broadphase collision queries — "what's near this point?" — i.e. the *same chunk
  grid*. Without it: O(projectiles × targets). With it: O(projectiles × local).
  Projectiles reuse the index we're already adding.
- **Lever B helps directly.** This is *more always-running authoritative sim* —
  exactly what the sim task isolates and (Lever D) can put on dedicated capacity.
  Projectiles are just more entities the sim publishes; gateways cull + serialize
  them unchanged.
- **Decoupled cadence (from Lever B) helps.** If projectiles need finer than
  100 ms resolution to feel fair, **sub-step the sim** (integrate motion N times
  per network tick) without raising the *network* tick. The sim/IO split makes
  this clean because sim cadence and snapshot cadence are already separate.
- **The one rule:** keep projectile motion, trap RNG, and archer cadence in the
  **pure seeded `meld-world` sim** (no `Instant::now`, no global RNG) so it stays
  replayable and unit-testable — the same discipline as everything else. This is a
  help disguised as a constraint: build it in `meld-world` and it's testable for
  free, and it lives under the [CR-4](../ROADMAP.md) budget umbrella.
- **Orthogonal model change:** overworld HP damage means a hero's run HP mutates
  *outside* `Battle`. That's a `meld-world` / `meld-run` data question (where run
  HP lives, how field damage applies), independent of scaling and not blocked by
  anything here.
- **Reinforcement:** fast, numerous projectiles churn positions every tick, which
  *raises* snapshot volume — making Lever A's grid + per-chunk serialize cache
  *more* valuable, not less. (Choppy fast projectiles at 10 Hz are a client
  interpolation concern, not an architecture one.)

**Verdict: helps.** Nothing in the plan is at odds with it; the cheap items are
tailor-made for it.

### Player-made towns that monsters siege and can destroy

This is the one that pushes hardest, because it introduces **persistent, shared,
destructible world state** — which today's "ephemeral instance, torn down when
empty" model deliberately does *not* have.

**The design intent (clarified): a town is not its own shard — it is persistent
content built *on* the overworld of a world shard.** A **world** = one shard = one
persistent overworld + everyone in it + that world's monster population + the towns
players have built on it. The town is authoritative world content, exactly like
terrain, resource nodes, and monsters. So the `InstanceActor` is really a
**`WorldActor`**: one task owns one whole persistent world, ticks it (movement,
ecology, sieges), and persists it. This *collapses* the would-be "town shard" into
the ordinary world shard: **the world persists; only each player's run/backpack
stays ephemeral** (that's the extract-or-die reconciliation — you dive out from
your town into the dangerous frontier; dying drops what you carried, it doesn't
reset the world).

**Worlds are player-seeded (à la Minecraft), and sharding is "many worlds."** A
world's identity is its **player-chosen seed**. Because worldgen is fully
deterministic from the seed (`section_seed(run_seed, n)`), horizontal scale comes
from **many distinct player-created worlds, each a capped shard** — *not* from
auto-cloning one realm. A named world that hits its population cap is simply *full*
(queue for a slot); you can't fork it, because it holds unique player-built towns.
This is the Minecraft-Realms / game-server-hosting model: lots of small/medium
world shards spread across machines, idle ones hibernated. (This promotes
[MON-2](../ROADMAP.md)'s "pinned seeds" from a premium perk to the *core* world
model, and reframes WG-2/WG-3 per-run biome reshuffling as a property of the old
ephemeral instances — a seeded persistent world is stable; variety comes from many
seeds.)

**It helps:**

- **The world shard is the single writer for its towns — for free.** One
  `WorldActor` owns the whole world including every town on it, so a destructible
  shared structure has exactly one authority and no two tasks ever race on it
  (CANON §S, applied per world). No cross-shard town coordination exists to get
  wrong.
- **Siege sim is all spatial.** Monster pathfinding toward a town, targeting the
  nearest wall segment, applying structure damage — every step is a grid query.
  **Lever A helps again** (towns are just entities the interest index buckets).
- **Persistence is a `serde` call.** Because world state is I/O-free and
  deterministic, terrain + town layout + per-structure HP + stored goods
  save/restore cleanly, and [MON-2](../ROADMAP.md) already contemplates "a
  persistent instance keeps mutable world state across sessions." **Lever D's**
  live migration applies too.
- **"Always-running even when unwatched"** siege/ecology is precisely the workload
  Lever D's dedicated sim tier exists for — gateways come and go with players; the
  world persists.

**Two commitments the plan must add (and this doc hereby adds):**

1. **World shards are persistent by default — they hibernate, they don't tear
   down — and they persist only the *delta from the seed*.** The current
   `runs.is_empty() ⇒ instance = None` teardown is simply *wrong* for any buildable
   world: the siege must progress (or freeze) with zero players watching. This is no
   longer a special "town mode" — it's how every world shard works. An empty world
   **serializes to Postgres and is evicted from RAM, reloading on first joiner**;
   while offline its siege/ecology either freezes or advances on a coarse offline
   tick (the CR-4 budget, applied at world granularity, so an idle world doesn't
   burn a core). **Crucially, because the world regenerates from its seed for free,
   the `WorldActor` stores only the *mutations* — built structures, structure
   damage, harvested/looted state, monster-population drift — not the map itself**
   (exactly like Minecraft saving only modified chunks). Determinism turns
   persistence into a cheap diff.
2. **A plan for the un-splittable mega-siege — but it's now bounded by the realm
   cap.** A world's whole population converging on one besieged town is a single
   **hot shard that cannot be split** — everyone is interacting in one place —
   O(participants × entities) on one core. The reframe *bounds* this: it can never
   exceed **one world's population cap M**, because that cap *is* the shard
   boundary. The realm cap doubles as the siege cap; the worst case is O(M), not
   O(global N). To survive it:
   - **Lever B is most valuable here:** one task owns the authoritative siege; the
     expensive per-defender fan-out parallelizes across cores.
   - **Cheaper design mitigations first** (align with CR-4): hard caps on
     simultaneous attackers, LOD sim for backline/distant attackers on a coarser
     tick, and wave pacing that bounds concurrent combatants.
   - **The far-future escape hatch** if even the *sim* of one world exceeds one
     core: **intra-world spatial partitioning** — sub-dividing a single world's
     simulation by region across tasks with boundary handoff. This is the hardest
     thing in the plan and the real ceiling; flagged so it's a conscious future
     investment, not a surprise.

**A free composition: distance-as-difficulty makes town siting a real decision.**
A town built deep (high `distance`) sits in a high-level monster band under heavier
constant siege; a town near the center is safer but less lucrative. Town placement
becomes risk/reward on the *existing* difficulty axis — no new system, it just
composes.

**Verdict: helps more than it hurts** — a town *is* persistent content on a world
shard, and the world shard is the persistence + single-writer boundary that a
destructible town needs. Aligns with existing roadmap thinking (CR-4 budget, MON-2
persistent camps, LC ward-sharding).

### The Shift mechanic + anchors (the world rearranges; you fight to pin it)

The overworld is [the Shifting Lands](../lore/shifting-lands.md): regions
periodically **Shift** — swap to a different biome mid-run, deal force damage to
anyone caught inside, and wipe that region's creatures + collectables. Players buy
**anchors** to *pin* a region (it stops shifting), but an anchor must be **defended
with buildings** or monsters destroy it and the region shifts again. *(Now canon:
the Shift is [CANON §W2](../CANON.md) (D20), and anchors + the unified structure
primitive are [§W3](../CANON.md) (D21); this proposal is the server-side plan for
them.)* This is the roguelite-freshness engine, and it reshapes persistence rather
than fighting it:

- **Persistence becomes event-sourcing.** Make the **Shift scheduler deterministic
  from `(seed, shift-generation)`**, driven by the tick counter — *never* wall-clock
  (the engine's no-`Instant::now` invariant already demands this). Then the world's
  *natural* shift evolution is a pure function of the seed, fast-forwardable at zero
  storage. The **only** thing that breaks purity is player action: an anchor
  suppresses a shift; a destroyed anchor re-enables it. So the persisted delta is
  **seed + a log of player events** (anchor placed/destroyed, structure
  built/damaged) — replay to reconstruct exact state. **Seasons are the natural GC:**
  a season boundary archives/wipes worlds, bounding the log.
- **Anchors, extraction portals, town walls, camps are ONE primitive.** Don't build
  four systems — build a **player-built structure with HP + a function tag**
  (`anchor` = suppress local Shift · `portal` = extract · `wall` = defend · `stash`
  = store) that monsters path to and attack. The siege sim, the interest grid, and
  delta-persistence handle them uniformly; only the function tag differs.
- **SC-1 powers Shift.** On a swap: re-bucket just the affected chunks; the grid
  tells you *exactly which players* have them in-interest (push new terrain via the
  existing `world.terrain_section` message to only them); "who's caught in the swap"
  for force damage is a grid range-query. Without the index, every Shift is
  O(everyone × everything).
- **SC-2 carries Shift for free.** A Shift is an authoritative sim event on the
  `WorldActor` tick — the retile + damage land in the immutable snapshot and the
  fan-out workers publish it. No new machinery.
- **Biomes are different sizes** (lore size table 1d6) — variable-extent regions
  are just a property of the shifted patch; the chunk grid is size-agnostic.

**This is what gives towns their mechanical purpose.** Returning to Last City
resets the run to level 1, so the incentive is to push *deeper* toward the
juicy-unlock end-world boss without going back — **forward towns let you sustain
that push** (stage/resupply/respawn out in the field), and **anchors keep those
hard-won regions from shifting away.** The world tries to erase your progress
(Shift); you fight to hold ground (anchors + defense); the payoff gates a true
end-battle sequence, raced on **seasons + leaderboards**
([endgame-seasons.md](../behaviors/endgame-seasons.md)). The architecture and the
game's point are the same shape.

### What's ephemeral vs. persistent (the three tiers)

The run/world/account lifetime split, the "reset to level 1 on any exit," and
seed-delta persistence are now **canon** — see [CANON §W4–§W5](../CANON.md)
(D22, D23). In short for the server plan: a **World** persists as **seed + event
log** (it hibernates when empty); only a player's **Run** is ephemeral; freshness
comes from the world *rearranging itself* (the Shift), and permanence is something
players *manufacture and defend* (anchors). The `WorldActor` is the single owner of
that persistent state.

### The shard taxonomy this implies

The clarified model leaves **two kinds of shard**, and naming them lets every
future feature slot in cleanly:

| Shard kind | Lifetime | Sim load | Contents |
|---|---|---|---|
| **Persistent hub shard** | long-lived, social | low | The Last City |
| **Persistent world shard** (a seeded "realm") | long-lived, DB-backed (seed delta), **hibernates when empty** | **high** (ecology + sieges) | one overworld + its players + monsters + **player towns** + world bosses. **Player-created & seeded**; capped — a full world queues, it does *not* auto-fork (unique towns). Scale = many worlds. |

Towns, camps, world bosses, and roaming ecology are all **content on a world
shard**, not shard kinds of their own. The only thing that stays ephemeral is a
player's **run/backpack**. The `WorldActor` is one task per world; only the hub
differs (lower sim, different lifecycle).

---

## Invariants to preserve (non-negotiable)

- **One task owns each world's state; no locks** (CANON §S). Sharding *partitions*
  this invariant, it never violates it. The Router owns routing state and touches
  it only from its own task.
- **Pure, seeded, I/O-free sim** (`meld-world` / `meld-battle`). Every new
  mechanic — projectiles, traps, sieges — lands here so it stays deterministic and
  unit-testable. No `Instant::now`, no global RNG.
- **DB writes stay off the loop** via the `db_writes` channel; loads that feed
  state back `await` Postgres only on connect/extraction/shard-spawn, never per
  tick.
- **Additive wire changes** — new `SnapshotEntity` variants / message types, not
  reshaped envelopes (CANON §I).

## Suggested sequencing

1. **Lever A** — chunk-grid the snapshot cull + per-chunk serialize cache. Reuses
   the `HashMap<(i32,i32), Vec<_>>` pattern already in `step_creatures`. Ship a QA
   load test (bot-ramp until tick time crosses ~100 ms) as the acceptance gate.
2. **Lever B** — publish `Arc<WorldSnapshot>`; parallelize fan-out across a worker
   pool. Unlocks sub-stepped projectiles.
3. **Lever C** — `GameState` → `Router` + `InstanceActor` (pure refactor to one
   actor first, then multi-instance + matchmaking + hub handoff). Add the
   persistent/player-independent lifecycle mode that towns need.
4. **Lever D** — only when a single box can't hold the population; keep the sim
   central, push fan-out to gateways.

`meld-battle` / `meld-world` never change structurally through any of this —
they're pure and seeded, so determinism and every existing unit test carry over.
The whole effort lives in `meld-server`.
