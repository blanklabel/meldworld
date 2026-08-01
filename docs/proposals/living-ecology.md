# The Living Ecology — creatures that eat, breed, herd & war

> **Status: PROPOSED (design only).** This graduates the ROADMAP epic **CR**
> (`CR-1`…`CR-5`) into one coherent design, and threads in the systems it depends
> on: the **day/night clock** (`FS-5`), **materials → crafting** (`MS-1`), overworld
> **ground drops** ([`async-interaction.md`](../behaviors/async-interaction.md)), and
> the persistent-world target (**CANON §W**, `WM-*`). It is written against the real
> stack — creatures already roam, belong to **factions**, carry a live `hp/max_hp`
> bar, skirmish with hostile factions, and leash to `home`
> ([`meld-world` `MonsterSpawn` / `Arena::step_creatures`](../../server/crates/meld-world/src/lib.rs));
> the world tick now runs inside the **`WorldActor`** that owns all world logic
> (SC-3, [`game.rs:595`](../../server/crates/meld-server/src/game.rs)); kills already
> bank a per-biome **material** (`combat_material_for_biome`), and `ResourceNode`s
> already scatter and harvest. This doc builds the ecology on top of that spine.
> Tracked as epic **CR** in [`../ROADMAP.md`](../ROADMAP.md).

> **The one hard constraint (the owner's, and correct).** A living ecology is
> *mutable, stateful, time-evolving* world state — the opposite of the current
> "regenerate any chunk freely from the seed" model. It must **never** threaten the
> single-owner authoritative loop or the server (CANON §S; memory: game-loop-perf).
> So **`CR-4` — the sim budget — is designed first (Part A) and everything else obeys
> it.** No feature in Parts B–G is allowed to violate the LOD/cap/determinism envelope.

---

## The vision

The overworld should feel *inhabited*, not decorated. Creatures should hunt, graze,
sleep at night, roam their territory, and **breed** — babies that grow up. Some
species move as **herds** with an **alpha**; a herd that grows too big **splits**.
Rival factions and herds **war over turf**: they damage each other, the wounded
**slowly regenerate**, and a creature that dies leaves its **loot and materials on
the ground** for whoever finds them. Underpinning all of it, the **flora grows** —
trees, grass, edible plants — because the herbivores have to eat something, and the
carnivores have to eat the herbivores. A real, if simple, food web.

And because it *is* a food web, it has **emergent consequences** (§I): raze a region's
flora and the herbivores starve, then the predators starve, then — left alone — it
**heals bottom-up**, slowly, flora → herbivores → predators. The world remembers what
you did to it, but never permanently: every region self-heals from a colonization floor
(§D.5), so there are no forever-dead zones. That durable memory needs the persistent
world (**`SC-3`**), which shapes the build order (§J).

Every one of those behaviors already has a roadmap item; this doc makes them one
system with one budget.

---

## Design principles (inherited, non-negotiable)

1. **Deterministic & seeded, tick-driven, never wall-clock.** The ecology advances
   from a PRNG seeded on `(section_seed, ecology_generation)` and stepped by the
   **server tick counter**, exactly like the Shift (CANON D20). This keeps it
   replayable, unit-testable (`meld-world` stays pure — no `Instant::now`, no global
   RNG, no I/O), and cheap to persist as an event-log delta (CANON §W5).
2. **Ecology is *overlay* state, not base terrain.** Base terrain/spawn-points stay
   deterministic-from-seed and freely regenerable ([`world-generation.md`](../behaviors/world-generation.md)
   §"Chunk Streaming"). The *population* — who's alive, wounded, pregnant, where the
   herd is — is **mutable overlay state** tracked per area, in the same class as
   dropped items and opened chests. In the precursor it dies with the instance; in
   the target it persists as the world's event log (§F).
3. **Server-authoritative, off the battle tick.** The ecology step runs inside the
   **`WorldActor`** world tick ([`game.rs`](../../server/crates/meld-server/src/game.rs)),
   at a **coarse cadence** (`ecology_tick`, ~1 s = 10 combat ticks), **cooperatively
   bounded** so it never blocks the 100 ms ATB tick and never awaits I/O. Clients
   render the snapshot; they never simulate ecology.
4. **Additive wire only.** New per-creature/flora **status tokens** on the existing
   snapshot (the `key:value` convention, CLAUDE.md "extending combatant state without
   a proto change") and reuse of the existing world-drop + battle-merge machinery. No
   renamed messages (AGENTS.md).

---

## Part A — `CR-4`: the simulation budget (designed first)

The guardrail the roadmap insists on. Everything else is written to fit inside it.

### A.1 Level-of-detail: simulate only what's watched

Each **area/section** ([`world-generation.md`](../behaviors/world-generation.md)
§"Per-section streaming") carries an ecology LOD determined by player proximity:

| LOD | When | Cost |
|---|---|---|
| **Hot** | a player is inside the area's interest radius (2 chunks) | full per-individual step every `ecology_tick`: movement, needs, skirmish, breed/growth rolls |
| **Warm** | recently hot / adjacent to a hot area | reduced-cadence step (every `ecology_warm_divisor` ticks); no new spawns rendered |
| **Cold** | no player near it | **frozen** — population serialized, **zero per-tick cost**. On re-entry, a bounded **catch-up** (§A.3) advances it |

> **Net:** per-tick cost scales with *players present*, not with world size. An
> endless world costs nothing where nobody is.

### A.2 Hard caps (all **[TUNABLE]**, the load-bearing numbers)

| Cap | Meaning |
|---|---|
| `area_pop_cap` | max living creatures simulated in one area (breeding cannot exceed it) |
| `max_hot_areas` | max simultaneously-hot areas per world (excess demote to warm) |
| `ecology_step_budget_creatures` | **global ceiling on creatures stepped per `ecology_tick`** — the loop slices work up to this and defers the rest to the next tick; the tick can never overrun |
| `swarm_battle_max_mobs` | max monster combatants a swarm pull can assemble (§E) |
| `ground_entities_cap_per_area` | max carrion/loot drops on the ground before oldest despawns early |

`ecology_step_budget_creatures` is the backstop: even a pathological world can't make
the world tick exceed its slice. If hot demand exceeds the budget, areas round-robin
across ticks (each still deterministic, since order is seed-derived, not arrival-order).

### A.3 Cold-area catch-up (the offline model)

A cold area is not stepped per-individual. On the tick a player re-enters it, the
server advances it by the elapsed `ecology_tick` count using a **closed-form,
seed-deterministic aggregate** — apply births/deaths/growth/regrowth statistically
over `Δticks` (a logistic population update toward the area's carrying capacity, §D),
**not** a per-creature loop over elapsed time. Cost is O(species in area), independent
of how long it slept. This is the "advances on a coarse offline model" path the
server-scaling proposal calls for ([`server-scaling.md`](server-scaling.md)); it makes
returning to a region feel like time passed there without ever paying for the time.

> **The `N = 0` problem — and the colonization term (§D.5).** Pure logistic growth,
> `dN/dt = rN(1 − N/K)`, has **zero as a stable fixed point**: at `N = 0` the growth
> term is `r·0·(…) = 0`, so an area wiped to *nothing* would stay dead forever, and
> migration + breeding alone can't refill it (breeding needs survivors; migration needs
> a stocked neighbour). The update therefore carries a **colonization source term** `c`
> — `dN/dt = c + rN(1 − N/K)` — a slow trickle seeded from the biome's base spawn table
> (§D.5) that lifts population off zero so logistic growth and local breeding can take
> over. This same `+ c` runs in the hot per-individual path too (rare background
> spawns), so hot and cold areas recover by the same rule. Both catch-up and hot step
> reference `c`; without it, the sim's recovery is silently broken (see §I).

### A.4 Determinism & test harness

- Ecology RNG = `splitmix64(section_seed ^ ecology_generation ^ tick)`; **no
  wall-clock, no global RNG** — `meld-world` stays a pure state machine.
- **Invariant (testable):** stepping an area N times, then N more, yields the same
  state as stepping it 2N times in one go **iff** LOD didn't change — and the
  catch-up (§A.3) is asserted to *approximate* the hot path within a tolerance, so a
  region isn't wildly different depending on whether you watched it. A QA load test
  (500-bot swarm, tick-overrun histogram — mirrors BUILD-PLAN T6-11) proves the
  budget holds under load.

---

## Part B — `CR-2`: turf wars, wounds & loot on the ground

Creatures already skirmish and lose `hp`. This completes the consequences.

### B.1 Territory & aggression

Every creature has a **territory** (its `home` + a species `territory_radius`) and
a **faction** (already present). Two behaviors produce turf wars:

- **Faction hostility.** Where the territories of **mutually hostile factions**
  (`creatures_hostile`, already in `meld-proto`) overlap, individuals in range engage
  — the existing skirmish, now with full consequences.
- **Herd/territorial defense.** An intruding creature (or herd) inside another's
  territory triggers defense (§E alphas lead it).

### B.2 Wounds persist and regenerate

- A skirmish deals real `hp` damage that **persists** on the `MonsterSpawn` (not reset
  when the fight pauses) — so `hp/max_hp` is a truthful pre-fight bar (already
  surfaced for the Explorer's HP-intel perk).
- Out of combat, a wounded creature **regenerates** `hp` at `creature_hp_regen_per_sec`
  **[TUNABLE]** while it roams. **A wounded creature is a real, time-bound
  opportunity** — a player who finds one mid-recovery gets an easier kill, and gets it
  only for a window.
- On-map **fighting state** is rendered (`state:fighting` token, §H) so a player can
  read "those two are clashing" from across the field and choose to third-party it.

### B.3 Death drops loot — and it stays on the ground

When a creature reaches 0 `hp` (from a skirmish, **or** a third species, **or** a
player's overworld hit) it **dies on the overworld** and drops:

- its **loot roll** (chits/gear at the stamped distance), and
- its **material(s)** (§G butchery table),

as **ground-drop entities** reusing the existing world-drop overlay
([`async-interaction.md`](../behaviors/async-interaction.md)) — visible to the
instance, first-come pickup, **but with a longer `carrion_despawn`** (**[TUNABLE]**,
default 10 min vs. the 5-min player-drop timer) so a kill you didn't cause is still
worth running to. The corpse is also a **carrion node** (§D.4): scavengers/carnivores
can eat it, and players can harvest it for extra material before it rots.

> This means the world **generates loot without a player swinging** — walk into the
> aftermath of a turf war and the spoils are lying there. That's the "world feels
> alive" payoff, and it costs only the overlay entities already budgeted (A.2).

---

## Part C — `CR-3` core: needs (eat / sleep / roam) & diets

Each species has a **diet class** driving a small **needs** model. Needs are two
floats in `[0,1]` on the `MonsterSpawn`, advanced each `ecology_tick`.

| Diet (`diet`) | Eats | Behavior |
|---|---|---|
| **herbivore** | mature **flora** (§D) | grazes nodes; flees predators |
| **carnivore** | live prey (hunts herbivores/weaker) + **carrion** (§B.3) | hunts; territorial |
| **omnivore** | both | opportunistic |

**Needs & the behavior state machine** (one `state` token, §H):

| Need | Rises when | Drives |
|---|---|---|
| `hunger` | always, per tick | at `hunger_hunt_threshold` → `hunting`/`grazing`; at `hunger_starve_threshold` → hp decay (starvation death if food stays absent) |
| `energy` (rest) | falls at night / when `sleeping` | at low energy **and** the FS-5 clock says night → `sleeping` |

Resolved states, in priority order: `fighting` > `fleeing` > `sleeping` (night, safe) >
`hunting`/`grazing` (hungry) > `breeding` (§D, well-fed) > `roaming` (leash to
territory). **Sleep ties to `FS-5`** — the seeded day/night clock: nocturnal vs. diurnal
species (`activity_phase`) sleep on opposite halves, so the field's danger shifts with
the clock (a sleeping predator is a window; a waking nocturne is a warning). A sleeping
creature is easier to engage — and, like a sleeping *player*, vulnerable to whatever
wanders into it.

Eating **restores hunger** and **depletes the food** (a grazed flora node → regrow
state §D; an eaten carrion → consumed). No food in the territory → hunger climbs →
the creature **emigrates** (leash stretches, §D.3 migration) or starves. This is the
pressure that keeps populations *dynamic* rather than static.

---

## Part D — Flora growth & the food web

The base of the chain: **the flora has to grow, because the herbivores have to eat.**
This extends `ResourceNode` (today static + instant-harvest) into a growing,
regrowing **`Flora`** layer.

### D.1 Growth stages

A `Flora` node has a `kind` (tree, grass, bush, herb, fungus — per biome) and a
**growth stage** advancing on the ecology tick, seeded:

```
seed → sprout → juvenile → mature → (grazed/harvested) → regrow → mature …
```

- **Trees** grow slowly toward `mature`, at which point they also count as **cover /
  soft obstacle** and yield **wood** when harvested (§G). New saplings seed near
  mature trees up to a **local density cap** (a bounded cellular spread — grass and
  fungus spread faster, trees slowest), so a cleared area regreens over time and a
  dense grove stays dense but never explodes.
- **Grass / edible plants** are the herbivore forage: `mature` nodes are grazeable;
  grazing knocks them to `regrow`; they climb back to `mature` over
  `flora_regrow_ticks` **[TUNABLE]**.

### D.2 Carrying capacity closes the loop

An area's **mature-flora biomass** sets its **herbivore carrying capacity**; the
herbivore population sets the **carnivore carrying capacity**. The catch-up (§A.3) and
the hot step both push populations **logistically toward capacity**:

- Lots of forage → herbivores well-fed → they **breed** (Part E) → population rises.
- Too many herbivores → forage depletes faster than it regrows → hunger → **deaths /
  emigration** → forage recovers → the cycle repeats.
- Herbivores up → carnivores well-fed → carnivores breed → predation rises → herbivores
  fall → carnivores starve back. Classic predator/prey oscillation, **bounded** by
  `area_pop_cap` and damped so it never runs away.

### D.3 Migration & territory

When local food collapses, a creature/herd's leash **stretches** and it **migrates**
toward an adjacent area with capacity (a seed-deterministic gradient walk, not
pathfinding-heavy). This spreads population across the world and means a region you
farmed out will slowly be **repopulated from its neighbors** — *fast* when a neighbour
still has the species, but only if one does. Migration is the *accelerator*, not the
guarantee; the guarantee is colonization (§D.5).

### D.4 Carrion

A corpse (§B.3) is a transient **carrion food node**: carnivores/omnivores/scavengers
eat it to restore hunger; players can harvest it for material; it rots on
`carrion_despawn`. Carrion is why a turf-war battlefield draws *more* creatures — the
kill feeds the next fight.

### D.5 Colonization — how a wiped region comes back

Migration (§D.3) and breeding (§E.1) both need a surviving population to draw from; the
logistic catch-up (§A.3) is stuck at zero without a source. So the recovery
**guarantee** is a slow **colonization trickle** (`colonization_rate` **[TUNABLE]**,
per species) seeded from the biome's **base spawn table** — the deterministic list of
"what belongs in this biome" that world-gen already owns
([`world-generation.md`](../behaviors/world-generation.md)). Two properties make this
the right mechanism:

1. **It lifts population off zero.** The `+ c` term (§A.3) means an emptied area climbs
   slowly off nothing even with no surviving neighbours; once there are ≥ 1 breeding
   pair, logistic growth + local breeding take over and it fills in.
2. **A species can never be *permanently* lost from its biome.** Because the source is
   the seeded base table, the seed guarantees the species still belongs there after a
   total wipe. Local extinction is real and lasting-ish; **global** extinction is
   impossible by construction (resolves open decision #6).

**Flora colonizes the same way.** Flora growth already starts at `seed → sprout` and
seeds near mature nodes (§D.1) — but if *all* flora in an area is gone there's nothing
to seed from, so flora carries the same base-table trickle. And because flora biomass
sets herbivore capacity (§D.2), a razed region must **regreen first**, then herbivores
recolonize, then carnivores — the food web rebuilds **bottom-up** (see §I).

**Recovery speed, in order:** *migration* from a surviving neighbour (fast, if one
exists) → *colonization* from the biome base table (slow, always available — the
guarantee) → then *logistic growth + breeding* off whatever seed population those
established. All three rates are `[ecology]` tunables, so "how long does a farmed-out
valley take to come back" is a dial, not a fixed law.

---

## Part E — Breeding, growth stages, herds, alphas & swarms

### E.1 Breeding & babies that grow up

A well-fed adult pair of the same species in the same territory, past a **cooldown**,
**breeds** (seeded roll each ecology tick, gated on `hunger < breed_hunger_max` and
`area_pop < area_pop_cap`). A birth spawns a **juvenile** (`life_stage: juvenile`):

- smaller, lower `hp`/damage (a `juvenile_stat_mult` **[TUNABLE]**), weaker loot,
- follows a parent/herd, flees readily,
- **grows to `adult`** after `growth_ticks` (seeded), at which point it takes full
  species stats and can itself breed.

Breeding **cannot** exceed `area_pop_cap` — the population is self-limiting by design,
which is what keeps the sim inside the CR-4 budget.

### E.2 Herds (some species) with an alpha

A species flagged `social` forms a **`Herd`** (overlay entity):

| `Herd` field | Meaning |
|---|---|
| `id`, `species` | identity |
| `members` | member creature ids |
| `alpha` | the lead creature — buffs herdmates (`alpha_buff`, a small stat/morale aura) and sets the roam target |
| `center`, `territory_radius` | the herd's roaming territory |

- The herd **roams together**, grazes/hunts together, and **sleeps together**; the
  alpha picks the destination (herdmates leash to the *herd center*, not their own
  `home`).
- **Alpha death** (turf war or players) triggers a **succession** roll — the
  strongest remaining adult becomes alpha; briefly leaderless herds scatter/flee.

### E.3 Herds split when too big

When a herd's membership exceeds `herd_split_threshold` **[TUNABLE]**, it **fissions**:
a subordinate adult becomes the **alpha of a breakaway herd** that migrates to adjacent
territory (§D.3). This caps herd size (bounding swarm battles, §E.4), spreads the
species across the map, and produces organic territory turnover — two herds that later
overlap will **turf-war** (Part B).

### E.4 Swarm battles

Touching **one** member of a herd (or a dense hostile cluster) pulls in nearby
herdmates within `swarm_pull_radius` **[TUNABLE]**, assembling a **swarm battle** — a
single `Battle` with **many** monster combatants (up to `swarm_battle_max_mobs`, A.2),
using the **existing battle-merge machinery** (CANON D5 already merges multiple parties/
mobs into one battle) rather than any new combat path. A swarm is the big, dangerous,
loot-rich set-piece the herds produce naturally; the mob cap keeps even a swarm inside
the ATB engine's per-battle budget. Allied players near a swarm can **join** it
(`run.join_battle`) exactly as they join any fight.

---

## Part F — Persistence (precursor vs. target)

- **Precursor (today — ephemeral instance).** Ecology is overlay state that lives and
  dies with the `MazeInstance`: it's simulated while the instance is open and
  **discarded on close** — no cross-run persistence, matching every other overlay
  ([`world-generation.md`](../behaviors/world-generation.md) Invariant 3). A run walks
  into a freshly-seeded ecology each time.
- **Target (CANON §W — persistent player-seeded World).** Ecology persists as part of
  the world's **event log**: a periodic **population snapshot per area** + the
  birth/death/migration deltas since it. A world **hibernates when empty** and, on
  re-entry, the area catch-up (§A.3) replays elapsed `ecology_generation`s from the
  snapshot — cheap, because it's the closed-form aggregate, not a per-tick replay. The
  **Shift** (CANON D20) is the ecology's reset valve: a Shifted region's creatures and
  collectables are **wiped** (Force damage), and the new biome's ecology recolonizes
  from the edges (§D.3, §D.5) — so the world's life visibly turns over.

> **This is the load-bearing dependency — `SC-3`.** The ecology's *durable*
> consequences (an over-farmed valley staying thin across sessions) exist **only** once
> the world persists. That storage is exactly what `SC-3` (world sharding) is building:
> shards "*hibernate to Postgres when empty and store only the seed delta — built /
> damaged / harvested / **population diffs**, not the map*" ([`../ROADMAP.md`](../ROADMAP.md)
> SC-3). **The ecology overlay is the writer of that "population diffs" line.**
> Crucially, **deterministic seed-generation is *not* persistence:** the seed gives
> *reproducibility* (regenerate the same base world for free — this ships today), while
> persistence needs the seed **plus a delta/event log** of what diverged. Today's build
> has the former and not the latter, so the sequencing in §J below is not optional —
> it's causal.

---

## Part G — `MS-1`: creatures & flora drop materials for crafting

Today a kill banks **one** biome material (`combat_material_for_biome`). This deepens
the drop side so gathering feeds crafting (`MS-1` Forging/Alchemy recipes).

- **Butchery tables (per species).** Each creature drops species-specific materials —
  **hide, chitin, fang, gland, ichor, bone**, etc. — in addition to the biome material.
  Quantity/rarity scale with **distance** (`tier(d)`, CR-1) and **encounter class**
  (Elite/Gatekeeper/**alpha** drop more and rarer, feeding CR-1's higher-rarity
  collectables and CR-5's codex). A material can be a **collectable** (CR-5) the first
  time you see it.
- **Flora harvest tables.** `mature` flora yields **wood / fiber / resin / herbs /
  spores** (§D), gated on stage (you can't harvest a sprout) — the field botany that
  feeds Alchemy. Ties `MS-2` (harvesting takes time) directly to the growth model:
  you harvest a *mature* node and it drops to `regrow`.
- **All materials** flow through the existing backpack → extract → Vault → craft path
  ([`economy.md`](../behaviors/economy.md), [`meta-progression.md`](../behaviors/meta-progression.md));
  nothing here bypasses extract-or-die (materials in the backpack are lost on death).

---

## Part H — Wire surface (additive — status tokens on the snapshot)

No new authoritative messages: the ecology is server-side and clients render the
snapshot. Per the `key:value` status-token convention (CLAUDE.md), the creature
snapshot entity gains tokens:

| Token | Meaning |
|---|---|
| `diet:<herbivore\|carnivore\|omnivore>` | diet class (client tinting / codex) |
| `stage:<juvenile\|adult>` | life stage (juveniles render smaller) |
| `state:<roaming\|hunting\|grazing\|sleeping\|fighting\|fleeing\|breeding>` | behavior (drives on-map icons — CR-2 "read the clash") |
| `herd:<id>` + `alpha` | herd membership; alpha gets a marker |
| `hp:<cur>/<max>` | already present — the wound bar |

Flora rides a `flora:<kind>:<stage>` entity tag (stage drives the sprite —
sprout→mature). Ground carrion/loot reuses the world-drop entity (Part B). The
**day/night clock** is `FS-5`'s `world.time` message (one source of truth; every
client agrees). Swarm battles use the existing `battle.started` with many mob
combatants.

---

## Part I — Emergent consequences: trophic cascades & player-driven scarcity

None of the following is scripted. It **falls out** of three local rules — eat, breed
toward capacity (§D.2), colonize slowly from the biome floor (§D.5) — and is the design's
real payoff. It's worth writing down so whoever builds it knows the behaviour is
*intended*, and so the guardrails that keep it healthy are explicit.

### I.1 The cascade

Kill (or harvest) an area's **flora** faster than it regrows → **herbivores starve**
(their capacity is a function of flora biomass) → **carnivores starve** (their capacity
is a function of the herbivores) → the region goes **barren**. A player can, with
sustained effort, collapse a whole local food web. Nobody wrote "if flora → 0 then
predators die"; the capacity coupling produces it.

### I.2 Bottom-up recovery (with a lag at each level)

Left alone, the region heals in **trophic order**, each level gated on the one below:

1. **Flora regreens first** — from the base-table trickle + spread from any survivors
   (§D.5). Slow, because it starts near zero.
2. **Herbivores return next** — but only once biomass supports them. Migration from a
   stocked neighbour jump-starts this; otherwise the colonization trickle does.
3. **Carnivores come back last** — they need a herbivore population to exist first.

So **predators are the last to recover and the first to crash** — which makes them a
readable barometer: *no predators in a region* ⇒ it was hit and hasn't healed. This is
real ecology emerging from the rules, and it gives the world legible history.

### I.3 What the cascade is *not*

- **Not permanent extinction — suppression.** The colonization floor (§D.5) guarantees
  the region eventually heals if left alone; the world can't accumulate permanent dead
  zones. The achievable fantasy is "*I collapsed this valley for a good while,*" not
  "*I exterminated them from existence.*"
- **Not easy.** Flora regrows and spreads and harvest is time-gated (`MS-2`), so razing
  a region faster than it recovers is a *lot* of sustained effort — and if you don't
  also suppress the neighbours, migration refills it out from under you. The collapse
  should be an **achievement**, not an accident.

### I.4 Design tension: grief vs. stewardship (and the guardrail)

The same mechanic is a **griefing vector** (collapse a material-rich region others rely
on) *and* a **stewardship / territory-control mechanic** (a guild manages or
deliberately scorches a rival's hunting grounds — ties straight into
[`parties-and-guilds.md`](parties-and-guilds.md)). The **self-healing floor is the
mitigation**: damage is always time-bounded, never permanent. The **Shift** (CANON D20)
is the second valve — it can wipe a suppressed region and reset it to a *different*
biome, so no scar lasts indefinitely regardless.

### I.5 Legibility is owed

For this to read as *consequence* rather than *bad RNG*, players must be able to see
that a region is depleted and recovering — sparse creatures, no predators, thin flora,
and a map/bestiary cue (`CR-5`). Build the read-out alongside the mechanic, not after.

---

## Part J — Persistence dependency & recommended build order (cross-epic)

The ecology touches four other epics. This is the sequencing that makes each phase pay
off, and it turns on one fact (§F): **seed-generation ships today; persistence
(`SC-3`) does not.**

### J.1 What pays off *before* persistence (build on the precursor now)

These add "the world feels alive" **within a single run/instance** and need no
cross-session persistence — they're worth building on today's ephemeral build:

- **E0** (`CR-4` budget) — foundational; build first regardless of everything else.
- **E1** (turf wars, wounds, ground loot) — pure within-run drama; loot a battlefield.
- **E2** (needs, diets, sleep) — needs **`FS-5`** (day/night clock) as a prerequisite;
  sequence FS-5 with or just before E2.
- **E5** (herds, alphas, swarm battles) — the big set-piece fights; within-run.

Populations resetting each dive is *fine* for these — the payoff is moment-to-moment,
not durable.

### J.2 What only gets its *full* payoff *with* persistence (`SC-3`)

- **E3** (flora growth) and **E4** (breeding / population dynamics) have a within-run
  payoff (populations shift during a long push) — **build the sim on the precursor** —
  but their *durable* consequence (§I: an over-farmed valley staying thin across
  sessions) lands **only** when `SC-3` persists the population-diff event log (§F).
- **The §I cascade as a persistent world-state** is therefore **gated on `SC-3`**. Order
  it: build the ecology *sim* now for feel; when `SC-3` lands, wire `AreaEcology`
  snapshots + deltas into the seed-delta hibernation and the cascade becomes durable —
  **no rework of the sim, just its persistence hook**.

### J.3 Companion epics (sequence alongside)

- **`FS-5`** (day/night) — **prerequisite of E2**. Do it first or together.
- **`MS-1`** (crafting) — pairs with **E6** (materials/butchery): materials only matter
  if there's crafting to consume them. Ship E6 near MS-1, not before.
- **`CR-1`** (per-creature distance mods, rarity, palette) — feeds E6 drop rarity and
  E1's deep-biome danger read; largely independent, slot it in when convenient.
- **`CR-5`** (bestiary) = **E7**; it's **account-persistent** (the existing account
  path, not world persistence), so it does **not** wait on `SC-3`.

### J.4 The guilds track runs in parallel (no ecology/`SC-3` dependency)

[`parties-and-guilds.md`](parties-and-guilds.md) (`SOC-1`/`SOC-2`) rides the
**account-persistence + HTTP** path that already exists (the Vault surface), so it does
**not** block on world persistence and can proceed independently. The one place the two
tracks *meet* is stewardship (§I.4) — guild territory control over ecology regions — and
that's a *later* join, well after both systems exist. Build guilds whenever; they're
off the critical path of the living world.

### J.5 One-paragraph recommendation

**Do `E0` (the budget) first.** Then ship the within-run "alive" feel on the precursor —
`FS-5` → `E1` → `E2` → `E3`/`E4` sim → `E5` — because it's valuable immediately and
needs no persistence. **Land `SC-3` persistence in parallel** (it's already
foundationed by the `Router`/`WorldActor` split); the moment it's ready, wire the
`AreaEcology` deltas into its seed-delta log and the §I cascade turns durable for free.
Fold `MS-1`+`E6` and `CR-1` in as they mature, and `E7`/`CR-5` anytime (account-side).
Run the **guilds** track independently on its own HTTP/DB path.

---

## Data-model additions (additive — CANON §W-style)

Overlay/ephemeral (server memory today; event-log-persisted in the target, §F) unless
noted.

| Model / field | Summary |
|---|---|
| `MonsterSpawn` (extend) | `+ diet`, `+ life_stage` (+ `growth_ticks_left`), `+ hunger`, `+ energy`, `+ state`, `+ herd_id?`, `+ is_alpha`, `+ hp_regen` |
| `Herd` (new overlay) | id, species, members[], alpha, center, territory_radius |
| `Flora` (new overlay; evolves `ResourceNode`) | kind, position, `growth_stage`, `regrow_ticks_left`, harvest table |
| `CarrionDrop` (new overlay) | a corpse: position, species, decay timer, remaining material — a food node + harvestable |
| `AreaEcology` (new overlay/log) | per-area population counts by species + `ecology_generation` — the unit the catch-up (§A.3) and the §F snapshot operate on |
| `SpeciesDef` (content, extend `MonsterDefinition`) | `diet`, `social`, `activity_phase`, `territory_radius`, `breed_cooldown`, `butchery_table`, `juvenile_stat_mult` |

Detail files would live under [`../interfaces/data-models/`](../interfaces/data-models/)
(e.g. `ecology-models.md`) when this graduates.

## Balance tunables (new `[ecology]` block in `balance.toml`)

Every number **[TUNABLE]** behind `meld-balance` (working agreement #2). The budget
numbers (A.2) are the load-bearing ones.

| Constant | Purpose |
|---|---|
| `ecology_tick_ms` (~1000) | ecology cadence (coarse; off the 100 ms combat tick) |
| `area_pop_cap`, `max_hot_areas`, `ecology_step_budget_creatures` | **the CR-4 budget** |
| `ecology_warm_divisor` | warm-area cadence reduction |
| `creature_hp_regen_per_sec` | wound recovery rate (B.2) |
| `carrion_despawn`, `ground_entities_cap_per_area` | corpse/loot persistence (B.3) |
| `hunger_hunt_threshold`, `hunger_starve_threshold`, `breed_hunger_max` | needs thresholds (C, E) |
| `flora_regrow_ticks`, flora per-kind growth rates, local density caps | flora growth (D) |
| `growth_ticks`, `juvenile_stat_mult`, `breed_cooldown` | breeding & maturation (E.1) |
| `herd_split_threshold`, `swarm_pull_radius`, `swarm_battle_max_mobs`, `alpha_buff` | herds & swarms (E) |
| `territory_radius` (per species), migration gradient params | territory & migration (B, D.3) |
| `colonization_rate` (per species; flora too) | the `+ c` recovery-guarantee trickle from the biome base table (D.5, A.3) — how fast a wiped region comes back |

---

## Build plan (phased; the guardrail first)

The per-phase intra-epic sequence. **Read it with §J** — which says *what pays off on
the precursor now* vs. *what needs `SC-3` persistence*, and where `FS-5`/`MS-1`/`CR-1`
slot in. The **◆ precursor / ⬧ needs-SC-3** tags below mark that split.

- **E0 — `CR-4` budget & determinism harness (the floor). ◆** LOD tiers
  (hot/warm/cold), the caps, the `ecology_tick` inside the `WorldActor`, the cold-area
  catch-up **with the `+ c` colonization term (§A.3/§D.5)**, the seeded PRNG, and the
  QA load test. **Nothing observable ships yet** — this is the envelope every later
  phase must fit. *Do this first (the roadmap is explicit).*
- **E1 — Turf-war consequences (`CR-2` remaining). ◆** Persist skirmish damage, hp
  regen, on-map `state:fighting`, and **death → ground loot + carrion** (reusing world
  drops). *Outcome: walk into a battlefield and loot it.*
- **E2 — Needs & day/night (`CR-3` core + `FS-5`). ◆** Diet classes, hunger/energy, the
  behavior state machine, sleep tied to the seeded clock. **Requires `FS-5` first.**
  *Outcome: creatures hunt, graze, and bed down; the field changes with the clock.*
- **E3 — Flora growth (`MS` / `FS`). ◆ sim / ⬧ durable.** `Flora` growth+regrow,
  herbivore grazing, carrying capacity, the colonization trickle (§D.5). *Outcome: the
  food web's base exists; grazed land regreens.* (Within-run now; the durable §I
  cascade needs `SC-3`.)
- **E4 — Breeding & population dynamics. ◆ sim / ⬧ durable.** Juveniles, growth to
  adult, logistic predator/prey oscillation under `area_pop_cap`. *Outcome: populations
  live and breathe.*
- **E5 — Herds, alphas, swarms & splitting. ◆** `Herd` overlay, alpha buffs/succession,
  `herd_split_threshold` fission, swarm battles via merge, migration. *Outcome: the big
  set-piece fights and organic territory turnover.*
- **E6 — Materials & butchery (`MS-1`).** Per-species butchery + flora harvest tables
  feeding crafting; alpha/elite rare-material drops. **Ship near `MS-1`** (materials
  need a crafting sink). *Outcome: the world stocks the Forge/Alembic.*
- **E7 — Bestiary / codex (`CR-5`).** Persistent **account-level** record of creatures &
  collectables discovered; also the §I.5 depletion read-out. **Account-persistent — does
  not wait on `SC-3`.** *Surfaces in the Last City.*
- **P — Persistence wiring (rides `SC-3`). ⬧** When `SC-3` lands, wire `AreaEcology`
  snapshots + birth/death/migration deltas into its seed-delta hibernation log (§F).
  **No sim rework — just the persistence hook** — and the §I cascade becomes durable
  across sessions.

When each hardens, **fold into CANON** (§/D-numbers) and graduate the observable rules
into `behaviors/living-ecology.md` + `interfaces/` (as verticality and dungeons did).

---

## CANON deltas to fold in (when the design hardens)

- **New D — Ecology sim model & budget (`CR-4`).** Ecology is deterministic
  (`(section_seed, ecology_generation)`, tick-driven, never wall-clock), overlay
  state, LOD-gated (hot/warm/cold), hard-capped (`area_pop_cap`,
  `ecology_step_budget_creatures`), stepped in the `WorldActor` off the ATB tick, and
  cold areas advance by closed-form catch-up. **Binds every rule below.**
- **New D — Diets, needs, activity phase.** `diet ∈ {herbivore, carnivore, omnivore}`;
  hunger/energy needs; sleep gated on the FS-5 clock + `activity_phase`.
- **New D — Wounds & carrion.** Skirmish damage persists; hp regenerates while roaming;
  death drops loot+material to the ground on a longer `carrion_despawn`; corpses are
  food + harvestable.
- **New D — Flora growth, carrying capacity & colonization.** Flora grows through
  seeded stages and regrows after grazing/harvest; area biomass sets herbivore
  capacity; populations move logistically with a **colonization source term** (`+ c`
  from the biome base spawn table) so a fully-wiped area recovers (migration
  accelerates; colonization guarantees). **Local extinction is possible; global
  extinction is impossible by construction** (the seed guarantees the species belongs
  to its biome). The **trophic cascade + bottom-up recovery** (§I) is an intended
  emergent consequence; its *durable* form requires the §W population-diff persistence.
- **New D — Breeding, herds, alphas, swarms.** Fed adults breed juveniles that mature;
  `social` species herd under an alpha; herds fission past `herd_split_threshold`;
  contact assembles swarm battles via the existing merge, capped at
  `swarm_battle_max_mobs`.
- **New D — Ecology persistence.** Ephemeral (dies with the instance) in the precursor;
  event-log-persisted (snapshot + deltas, hibernate-when-empty) in the §W target; the
  Shift wipes and recolonizes.
- **Glossary (§G):** `Herd`, `Flora`, `CarrionDrop`, `Diet`, `LifeStage`, `AreaEcology`,
  `alpha`.

---

## Open decisions (yours to call)

1. **Catch-up fidelity (§A.3).** How closely must a cold area's closed-form catch-up
   match the hot per-individual sim? Recommendation: **approximate within a tolerance**
   (cheap, good enough) rather than exact replay — but confirm, since it sets whether a
   region "feels the same" regardless of who watched it.
2. **Precursor persistence (§F).** Confirm ecology is **ephemeral** in the current
   ephemeral-instance build (assumed — matches every other overlay) and only persists in
   the §W target. The alternative (persist even now) needs the §W storage first.
3. **Do player kills feed population pressure?** i.e. does over-farming an area visibly
   thin it (until migration/breeding refill)? Recommendation: **yes** — it's the whole
   "living world" point — but it means a player can locally deplete a species, which is a
   design stance to take deliberately.
4. **Swarm cap (`swarm_battle_max_mobs`).** How big can a swarm get before it's capped —
   the tension between "epic" and "the ATB engine's per-battle budget." Recommendation:
   start conservative (e.g. 8–12) and raise with load data.
5. **Overpopulation crashes / disease?** Optional damping beyond hunger — a periodic
   die-off when a species overshoots capacity. Recommendation: **defer** — logistic
   capacity + starvation already bounds it; add disease only if populations feel too
   static.
6. **Starvation & extinction.** — *Resolved (§D.5).* **Local extinction is allowed and
   lasting-ish; global extinction is impossible by construction.** A wiped area recovers
   via the colonization trickle (`colonization_rate`, `+ c`) seeded from the biome base
   table — migration accelerates it where a neighbour survives, colonization guarantees
   it where none do. The only remaining dial is the *rate* (`colonization_rate`): how
   long a farmed-out region takes to come back. Recommendation: **slow enough that a
   collapse (§I) feels earned and consequential, fast enough that dead zones aren't
   permanent** — tune against playtest.
