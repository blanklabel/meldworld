# Building & Sieges — harvest → structures → towns → the anchor-defense loop

> **Status: PROPOSED (design only).** This is the design doc for the new ROADMAP epic
> **BD** — player building. It **graduates the CANON foundation that already exists**:
> the one `Structure` primitive and its `function` tag (CANON **D21 / §W3**), the
> `Structure` data model (`owner_player_id`, `function`, `hp/max_hp`, `pin_radius` —
> [`world-models.md`](../interfaces/data-models/world-models.md)), and the world model
> that persists it (§W1–§W5). It threads into the systems it depends on and touches:
> the **persistent world + siege sim** (`SC-3`, [`server-scaling.md`](server-scaling.md)),
> the **ecology sim budget** (`CR-4`, [`living-ecology.md`](living-ecology.md)) — which
> the siege sim *shares* — the **Shift** (D20), **extraction portals** (D15), **Run
> Level / forward towns** (§W4), **guilds** ([`parties-and-guilds.md`](parties-and-guilds.md)),
> and **crafting** (`MS-1`). Tracked as epic **BD** in [`../ROADMAP.md`](../ROADMAP.md).

> **The discipline CANON already mandates (§W3):** there is **one** `Structure`
> primitive — HP-bearing, destructible, siege-able — and a `function` tag varies its
> role. *"The siege sim, the spatial interest index, and world persistence handle every
> function uniformly — do not build towns, anchors, portals, and camps as separate
> systems."* This doc obeys that: a "town" is not a new entity, it's **a cluster of
> `Structure`s around an anchor**; a "camp" is a small one; a "wall" and a "workshop"
> are the same primitive with different `function`s.

---

## The vision

You should be able to **carve a foothold out of a hostile, self-rearranging world.**
Fell trees for **wood**, quarry outcrops for **stone**, and spend them to raise
**structures** — walls, a stash, a workshop, an extraction portal — clustered into a
**town**. Plant an **anchor** and it **pins the ground around it against the Shift**
(D20) *for as long as you defend it* — because in the Shifting Lands nothing stays put
unless someone holds it. Creatures **siege** what you build: they path to your walls
and beat on them, and if the anchor falls, the region starts shifting again and your
foothold can be swept away. Build deep and the loot is richer but the siege is
constant; build near the center and you're safe but poor. **Hope is hard work; nothing
is free** (§W3).

You build it hands-on: drop into a **builder mode** (Part I) — a ghost that snaps to the
grid, rotate, pick a level, confirm — and you can **build *upward*** (Part J), stacking
floors on pillars into towers and ramparts, with buildable stairs the only way up
(verticality, extending D24) — height that's also your best defense, since creatures
can't climb and must breach the base. And because nobody can stand guard 24/7, you
**hire NPC defenders** (Part K) to garrison the walls and fight the siege while you're
logged off.

This is the **sim / world-builder / desperate-roguelite** pillar CANON §W names — and
it's the concrete "anchor and defend" loop. Almost none of it has a home in the current
roadmap; this epic gives it one.

---

## Design principles (mostly inherited from CANON §W)

1. **One primitive, many functions (D21/§W3).** Everything built is a `Structure` with
   a `function`. The siege sim, the interest index, and persistence treat them
   uniformly. New roles are **new `function` values, not new models.**
2. **Server-authoritative, deterministic, event-sourced.** Placement, build progress,
   HP, siege damage, and destruction are all server-side (CANON §S). The world persists
   as **seed + an event log** of player-caused changes (§W5) — structures built /
   damaged / destroyed, anchors placed / lost (and the Shifts they suppressed), harvest
   state. Replaying `seed + log` reconstructs exact state.
3. **The siege sim *is* the ecology sim budget.** Monsters pathing to and attacking a
   wall is the same "always-running-even-when-unwatched" spatial workload as the
   ecology (`CR-4`). Structures are just entities the interest index buckets; the siege
   step runs inside the **`WorldActor` world tick** under the **same LOD (hot/warm/cold)
   and the same per-tick budget** as the ecology ([`living-ecology.md`](living-ecology.md)
   Part A). A besieged town with no players present **freezes or advances on the coarse
   offline model** — it never costs a watching-nobody tick.
4. **Additive wire only.** New `world.structure_*` messages and `build`/`harvest`/
   `repair` intents parallel the existing `run.*`; the `Structure` rides the snapshot as
   an additive entity. No renames (AGENTS.md).
5. **This epic is gated on persistence (`SC-3`) — causally, not optionally.** A town
   that vanishes at instance-close is pointless. Unlike the ecology (which has
   within-run payoff), building's payoff is **almost entirely durable** — see §L. Build
   the primitives against the WorldActor now; the feature *means something* only once
   the world persists.

---

## Part A — Harvesting raw materials: wood & stone

Building needs **bulk structural materials**, distinct from the fine crafting materials
creatures drop (`MS-1` / [`living-ecology.md`](living-ecology.md) §G). Two sources:

- **Wood — from the ecology's flora.** Mature **trees** are already `Flora`
  ([`living-ecology.md`](living-ecology.md) §D) that yield **wood** when harvested (§G
  there). Building consumes that wood. Felling a tree drops it to the `regrow` stage, so
  a cleared stand regrows (and over-logging thins a region exactly like over-farming —
  the §I cascade). **No new harvest system for wood** — it's the flora layer.
- **Stone / ore — new mineral nodes.** This epic adds **`MineralNode`s** (rock
  outcrops, quarries, ore veins) scattered by biome — the inorganic counterpart to
  flora. They yield **stone, ore, clay** and, deeper, rarer mineral materials (rarity
  scales with `distance`/`tier`, CR-1). Unlike flora they **don't regrow** on the
  ecology tick (rock is not alive) but a node has a finite **yield pool** that depletes
  with harvest and **slowly replenishes** (or is one-shot and re-seeded elsewhere — an
  open decision, §Open). Harvest is the **timed `MS-2`** interaction (a quarry takes
  real seconds), gated on tool/level where content dictates.

| Material class | Source | Feeds |
|---|---|---|
| **Wood** | mature `Flora` trees (ecology §D/§G) | walls, workshops, most structures |
| **Stone / clay** | `MineralNode` (this epic) | durable walls, towers, anchors (high HP) |
| **Ore / metal** | deep `MineralNode` | reinforced/upgraded structures; also gear crafting (`MS-1`) |
| **Creature materials** | butchery (ecology §G) | gear crafting — *not* structural (kept separate) |

All materials flow through the existing **backpack → extract → Vault** path; building
consumes from the **backpack in the field** (you carry what you build with) or from a
**`stash`** structure on site (§F). Materials in the backpack are lost on death —
building inherits extract-or-die.

---

## Part B — The `Structure` primitive: place, build, defend, repair

One entity ([`world-models.md`](../interfaces/data-models/world-models.md) `Structure`):
`id`, `function`, `owner_player_id`, `position` (+ `level`, verticality D24), `max_hp`,
`hp`, `pin_radius?`. This epic specifies its **lifecycle**.

### B.1 Functions

The canon set (D21/§W3) plus a **minimal additive extension** for town-building (marked
🆕 — extends the D21 enum; fold into CANON on hardening, keeping the one-primitive rule):

| `function` | Role |
|---|---|
| `anchor` | pins its region against the Shift while HP > 0 (Part C) — canon |
| `portal` | plantable, defendable extraction (evolves D15) — canon |
| `wall` | blocks movement + soaks siege — canon |
| `stash` | field storage (§F) — canon |
| `gate` 🆕 | a wall segment the owner's group/guild can pass, monsters must break |
| `tower` 🆕 | ranged auto-defense: attacks besieging creatures in radius (a static "garrison") |
| `workshop` 🆕 | field crafting station — Forge/Alembic in the field (`MS-1`, §F) |
| `hearth` 🆕 | forward-town rally/**respawn** point + a small safe aura (generalizes FS-1 camping) |
| `banner` 🆕 | flies the owner's **guild heraldry** (SOC) over the town — identity, no combat role |
| `floor` / `platform` 🆕 | a walkable raised surface at a higher `level` — a **built terrace** (verticality, Part J) |
| `pillar` / `stilt` 🆕 | vertical **support** that holds up floors/walls above (Part J) |
| `stair` / `ladder` / `ramp` 🆕 | **buildable connectors** — the player-placed D24 connectors; the only way up your own build (Part J) |
| `barracks` 🆕 | houses & hires **garrison** defenders that hold the town while you're offline (Part K) |

> **How you actually place these — builder mode — is Part I; building *upward* is
> Part J; hiring things to defend them is Part K.**

### B.2 Place → build → HP

1. **Place.** The owner chooses a `function`, a valid position, and (for anchors) an
   orientation. **Placement rules** (all server-validated): on traversable terrain, at a
   given `level` (verticality-aware, D24), **not** overlapping obstacles/other
   structures/the guaranteed clear path, within a **spacing** rule, and — for most
   functions — **within an anchor's `pin_radius`** (you build *inside* your held ground;
   walls/anchors themselves can seed new ground).
2. **Build.** Placement debits the recipe's **material cost** (wood/stone/ore) from
   backpack/stash and creates the `Structure` at **low HP / "under construction,"**
   which **ramps to `max_hp`** over a build time (or via repair hits — §B.4). Build
   progress is server state on the entity.
3. **HP & destruction.** Monsters siege it (Part D); at **`hp = 0` it is destroyed**
   (removed from the world; an anchor's region becomes shiftable again). Destruction is
   an event-log entry (§G).

### B.3 Upgrade tiers

A structure can be **upgraded** (wood → reinforced → stone → metal) for more `max_hp`
and (walls) more soak, at rising material cost — the material sink that keeps deep
harvesting relevant. Tiers are content/`[building]` tunables.

### B.4 Repair & demolish

- **Repair.** Spend materials to restore `hp` toward `max_hp` (the counter to siege
  attrition). Repairing during a siege is the core defensive verb.
- **Demolish.** The owner (permission-gated, §E) removes a structure, refunding a
  **fraction** of its materials — anti-grief and anti-clutter.

---

## Part C — Anchors & the Shift-pin loop (the "anchor and defend")

The heart of the epic, straight from §W3.

1. **Plant an anchor.** An `anchor` `Structure` **pins every region cell within its
   `pin_radius`** against the Shift (D20): while the anchor stands, the natural Shift
   schedule **skips** those cells (recorded as a `ShiftEvent` with `suppressed_by` = the
   anchor, §W5). This is how players **manufacture permanence** in a self-rearranging
   world.
2. **Defend it.** The anchor holds **only while its HP > 0.** Creatures siege it (Part
   D); a Shift-warning tell near a pinned region means "hold, or lose it."
3. **Lose it.** Reduce the anchor to 0 HP (siege, world boss, or a rival group) and its
   region **becomes shiftable again** — the next scheduled Shift can wipe what you
   built there. The anchor is the **single point of failure by design**: defending it is
   the loop.
4. **Race.** Anchors chain outward — each pinned region is a stepping-stone toward the
   far end-world boss (§W). A **forward town** (anchor + hearth + portal) lets a group
   push deep **without resetting Run Level** (§W4) — the anchor network *is* the
   expedition's supply line.

---

## Part D — Siege & defense

### D.1 Creatures siege structures (extends `CR-2`)

The ecology's aggression (turf wars, `CR-2`) **extends to target structures**: a
`Structure` is, to the siege sim, an entity with HP and a faction (the owner's). Hostile
creatures within range **path to the nearest structure** (walls first — they block —
then what's behind) and **attack it each siege tick**, applying `structure_damage`
**[TUNABLE]**. This is the same spatial pathing the ecology already does, pointed at a
new target class — **no new combat path.** Density/aggression scales with `distance`
(deep towns are besieged harder — the siting trade-off, §E).

### D.2 Defense

- **Walls / gates** block and soak; layering them buys time.
- **Towers** 🆕 auto-attack besiegers in radius — a static garrison that works while
  you're away (bounded per-tower, budget-safe).
- **Players garrison** by being present and fighting the siege as normal overworld
  combat; **allied players / guildmates** join via the existing `run.join_battle`.
- **AX-3 agents** (later) can **garrison towns/anchors while owners are offline**
  ([`../ROADMAP.md`](../ROADMAP.md) AX-3) — the answer to "who defends at 3am."
- **Repair** (§B.4) races the attrition.

### D.3 The always-running problem (shared with `CR-4`)

A siege **must progress (or freeze) with zero players watching** — that's the whole
point of persistence. This is the *same* workload as the ecology, so it uses the *same*
solution ([`living-ecology.md`](living-ecology.md) Part A): hot areas step the siege per
structure each `ecology_tick`; **cold areas (no players) freeze**, and on re-entry a
**closed-form catch-up** applies elapsed siege damage / structure loss in O(structures),
not a per-tick replay. A town under siege while you're logged off resolves on the coarse
offline model — you may **log in to a breach**, never to a melted server.

### D.4 World bosses & mega-sieges (endgame)

A world's whole population converging on one besieged town, or a **world boss** sieging
it, is bounded by the **realm population cap** (`SC-3`): the realm cap doubles as the
siege cap, so the worst case is O(realm), owned by one `WorldActor` task — no
cross-shard coordination (server-scaling §"mega-siege"). Endgame content.

---

## Part E — Towns: composition, ownership & siting

### E.1 A town is a cluster, not a new entity

A **town** = an **anchor** (holds the ground) + **walls/gates** (soak the siege) +
**stash** (store materials) + **portal** (get home) + **workshop** (craft on site) +
**hearth** (respawn/rally) + **banner** (identity). It's an emergent composition of the
one primitive — no `Town` model. A **camp** is just a small, cheap one (generalizes
FS-1 / MON-2 camps).

### E.2 Ownership: personal & guild

- `owner_player_id` is the builder (canon field).
- **Guild-owned towns** (SOC): a structure can be owned by a **guild** instead of a
  player; the `banner` 🆕 flies the guild's **heraldry flag**
  ([`parties-and-guilds.md`](parties-and-guilds.md) Part D). Guild **ranks/permissions**
  gate who can **build / upgrade / demolish / repair / access the stash** — the same
  permission bitset (invite/kick/vault…) gains `build`, `demolish`, `stash_access`.
  This is where **stewardship** (living-ecology §I.4) becomes concrete: a guild holds
  and defends a region's anchors, and the shared **guild vault** (SOC B.5) can bankroll
  the materials, with the **audit log** (SOC B.6) recording who spent them.

### E.3 Siting is a real decision (distance-as-difficulty)

Because monster level + loot + siege pressure all scale with `distance`, **where you
build matters** (server-scaling §"free composition"): a deep town is lucrative but under
heavy constant siege and expensive to hold; a central town is safe but poor. No new
rule — it falls out of the existing difficulty axis.

---

## Part F — Field crafting & storage

- **`stash`** — field storage: deposit/withdraw materials + items on site, so you don't
  haul everything home mid-build. (Not the guild vault — a physical, **siege-able**
  world object; if the stash is destroyed, its contents drop as ground loot. Risk lives
  in the field.)
- **`workshop`** 🆕 — a field **Forge/Alembic** (`MS-1`): craft gear/repair from
  materials *at the town* instead of only in the Last City. Extends crafting's reach to
  the frontier so a deep expedition is self-sufficient.
- **`hearth`** 🆕 — forward-town **respawn/rally** + a small safe aura (the
  Sanctuary-Campfire family, GDD §5), the thing that makes a forward town a *base*.

---

## Part G — Persistence & seasons (the `SC-3` dependency)

Structures are **the** canonical content of the §W5 event log. Persisted state is the
**log of player-caused events**: structures built / damaged / destroyed, anchors placed
/ lost (and the Shifts they suppressed, `ShiftEvent.suppressed_by`), harvest/depletion
state. The baseline terrain is regenerated from the seed and never stored; replaying
`seed + log` reconstructs the exact town. Empty worlds **hibernate** to Postgres and
reload on first joiner. **Seasons are the GC** (§W5): at a season boundary worlds are
archived/reset, bounding the log — account-tier state (Vault, gear, skills) is **not**
wiped.

> **This is why BD is gated on `SC-3` (§L).** The event-log persistence, the
> hibernate/reload, and the per-world single-writer siege authority are exactly what
> `SC-3` builds. Building is the **strongest argument for `SC-3`** — and mostly can't
> ship meaningfully without it.

---

## Part H — Wire surface (additive)

**Realtime (world tick — authoritative world content):**
- Snapshot: the `Structure` rides as an additive entity — `structure:<function>` +
  `hp:<cur>/<max>` + `owner`/`guild` + `build:<pct>` tokens (the `key:value` convention).
- New messages: `world.structure_placed` / `world.structure_damaged` /
  `world.structure_destroyed` / `world.structure_repaired` (server → clients);
  `run.build { function, position, level }`, `run.repair { structure_id }`,
  `run.demolish { structure_id }`, `run.harvest_mineral { node_id }` (client intents,
  paralleling the existing `run.harvest`). Siege damage surfaces via
  `world.structure_damaged`.
- Anchor-pinned regions + Shift suppression ride the existing Shift/`world.terrain_*`
  surface (D20/D24).

**HTTP (persistent — reads/management):** `GET /v1/worlds/:id/structures` (a world's
town/structure list), `GET /v1/structures/:id`, guild-town management under the guild
surface (SOC). Mutations that happen *in the field* stay on the realtime world tick
(they're world content, like combat), persisted via the WorldActor's DB writer — not a
separate HTTP write path.

---

## Part I — Builder mode & the building system (how you actually build)

Building needs a real construction UX. **Builder mode** is a **client sub-mode over the
overworld** — not a new `Screen` (you stay in the world, standing where you're
building), just a mode the client enters. It is pure UX: it **creates nothing**; it
previews and sends intents the server validates (§H). This is the same
client-sends-intents / server-authoritative rule as combat (CANON §S).

### I.1 Enter / exit
Toggle builder mode (client key, e.g. `B`) when you're in a **buildable context** —
inside your (or your guild's) anchor `pin_radius`, or anywhere for a field camp. Your
avatar can still move; the world keeps ticking around you (a siege doesn't pause for
your menu).

### I.2 The build palette
A Bevy UI palette (reusing the existing menu patterns) lists the buildable
`function` × `tier` entries with their **material cost**, and **greys what you can't
afford** — exactly like the class skill menu greys an unaffordable/locked skill.
Select one → it becomes the **ghost**.

### I.3 Placement gestures (ghost → snap → confirm)
A translucent **ghost** of the selection tracks a build cursor and **snaps to the tile
grid** (the 64×64 chunk grid — keeps builds aligned to the grid the siege pathing
queries) and to **adjacent structures** (walls chain) and to **levels** (D24 integer
levels, Part J):

- **rotate** (`R`),
- **raise / lower the target `level`** (`Q`/`E` or scroll — Part J),
- **confirm** → sends `run.build { function, tier, position, level, rotation }`.

The server validates (terrain / level / **support** / spacing / within-anchor /
not-on-the-clear-path / materials) and either applies it (debit materials, spawn the
under-construction `Structure`, §B.2) or **rejects with a reason**. The ghost shows
**green/red predictively** as a client hint, but the server is the authority — a race
that invalidates a spot just returns a rejection, no client-side truth.

### I.4 Edit & demolish sub-mode
Target an existing structure to **upgrade** (§B.3), **repair** (§B.4), or **demolish**
(§B.4, partial refund). Guild builds are **permission-gated** (§E.2): the palette and
the edit actions respect the caller's guild rank (`build` / `demolish` / `stash_access`
bits), enforced server-side.

### I.5 Blueprints (later)
Save a laid-out cluster as a **blueprint** to re-lay elsewhere or share as a **guild
blueprint** — deferred, but reserve it; it's the natural "rebuild my town after a
season reset" affordance.

---

## Part J — Building up: verticality in construction (extends D24)

The request: *people can build up.* Today verticality (**CANON D24**) is
**terrain-derived** — discrete integer `level`s per section from the seed, **cliffs are
impassable walls**, and a **connector (slope/ladder/rope) is the only way to change
level — no free climbing.** Building **extends D24 to player-placed elevation**, reusing
the exact same integer-level axis and the exact same no-free-climbing rule.

### J.1 The vertical pieces
- **`floor` / `platform`** — a walkable raised surface at `level = n`: a **built
  terrace**, the player-made analogue of a terrain terrace. Stack them for towers,
  ramparts, multi-storey halls.
- **`pillar` / `stilt`** (and `wall`) — vertical **support**. A floor at level `n` is
  valid **only if support reaches it from the surface below** — no floating castles.
- **`stair` / `ladder` / `ramp`** — **buildable connectors**: the player-placed version
  of D24's slope/ladder/rope. **No-free-climbing is preserved** — to reach your own
  upper floor you place and use a connector, exactly as terrain levels demand one.
  D24's "generation places ≥1 connector per raised terrace" becomes the **builder's**
  responsibility: a floor with no connector is **unreachable**, and the client warns
  (a stranded-floor lint, mirroring the terrain solvability guarantee).

### J.2 Support & height cap (bounded + legible)
Server-validated **support rule** (a floor must rest on a pillar/wall chain to the
ground) forbids floating structures; **height is capped** at `max_build_level`
**[TUNABLE]** (extends D24's `max_level`), bounding the vertical sim + render.

### J.3 Verticality is a defensive advantage (emergent — ties the siege)
Because siege pathing is **level-aware** (D24: "touch / battle-join compare level as
well as position") and **creatures can't free-climb**, they must **breach the ground
floor and the supports** to reach anything above. So building up is genuinely
**defensive**: put your **towers** (auto-defense, §D.2) and hired archers (Part K)
**up high**, and force the siege to chew through walls and pillars at the base.
**Destroying a support collapses what it holds** — a real structural stake and a prime
siege objective. (Collapse fidelity — full cascade vs. "an unsupported floor is simply
destroyed" — is an open decision, §Open.)

### J.4 Wire
No new elevation primitive: a built `level` is the **same integer axis** as terrain, so
structures ride their `level` on the existing `SnapshotEntity.level` (D24) and the
level-aware sim already handles touch/siege/movement. Purely additive.

---

## Part K — Hiring defenders: the NPC garrison

Towers (§D.2) are static; a real town needs **mobile defenders when the owners are
offline** — the "who defends at 3am" answer. You **hire NPC garrison units** bound to a
town/anchor.

### K.1 Hiring
At a **`barracks`** 🆕 structure (or a **town vendor** — this is the *"hiring at a town
vendor"* the roadmap already names under **EC-2**, related to **CL-1** class hires),
spend **chits + materials** to hire a **`GarrisonUnit`** of a chosen **tier**
(militia → guard → veteran …), optionally **class-flavored** (a hired Phoenix Guard holds a
wall; a hired Ranger mans a tower). **Guild towns pay from the guild vault** (SOC B.5),
recorded in the **guild audit log** (SOC B.6) — so a garrison is an accountable guild
expense, not a silent drain.

### K.2 Behavior (defends while you're gone)
A garrison unit **patrols** within the town/anchor radius and **engages the siege**
(Part D): when hostile creatures attack, garrison units fight them as overworld
combatants — **including while every owner is offline**, which is the entire point. They
run on the **shared `CR-4`/siege budget** (they're just combatants the `WorldActor`
steps and the interest index buckets), so a **`garrison_cap` per town** **[TUNABLE]**
bounds them like everything else. AX-3's smart agents (below) can also **sortie**; a
plain garrison only defends.

### K.3 Upkeep & loss (stakes + an economy sink)
Garrison units cost **upkeep** (chits/day from the town/guild treasury) — a **new chits
sink** extending [`economy.md`](../behaviors/economy.md) (add a `K`-row); if upkeep goes
unpaid the units **disband**. And a unit can **die** in a siege — **permanent, re-hire**
— so a garrison is not set-and-forget: it's an ongoing cost you weigh against the loot a
deep town earns. This keeps NPC defense from trivializing the extract-or-die stakes.

### K.4 AX-3 is the smart evolution
A hired `GarrisonUnit` is the **mechanical** defender: scripted patrol + fight.
**`AX-3` agent inhabitants** ([`../ROADMAP.md`](../ROADMAP.md)) are the **intelligent**
evolution — real agent-driven NPCs that garrison, patrol, sortie, and make decisions
(PvE-only). **BD ships the garrison mechanism; AX-3 upgrades the brains** behind the
same `GarrisonUnit` entity — a smarter controller, not a new system.

---

## Data-model additions (additive)

| Model / field | Summary |
|---|---|
| `Structure` (extend) | `+ function` values `gate`/`tower`/`workshop`/`hearth`/`banner`/`floor`/`pillar`/`stair`/`ladder`/`ramp`/`barracks` 🆕; `+ guild_owner_id?` (guild-owned); `+ build_progress`; `+ tier`; `+ level` (already on the entity via D24 — now player-set, Part J); `+ rotation` |
| `MineralNode` (new) | inorganic resource node (stone/ore/clay): kind, position, yield pool, replenish rule, harvest table — the inorganic sibling of `Flora` |
| `BuildRecipe` (content) | `function` + `tier` → material cost, build time, `max_hp`, defense values, `support_required` (Part J) |
| `GarrisonUnit` (new) | a hired NPC defender bound to a town/anchor: id, home structure/anchor, owner (player/guild), tier + optional class, hp, upkeep, state (patrol/fighting) — a combatant on the shared `CR-4` budget (Part K); AX-3 swaps in a smarter controller |
| `ShiftEvent` (reuse) | `suppressed_by` already models an anchor pinning a region (§W5) — no change |

Detail would live under [`../interfaces/data-models/`](../interfaces/data-models/) when
this graduates.

## Balance tunables (new `[building]` / `[siege]` blocks)

| Constant | Purpose |
|---|---|
| per-`function`/`tier` `material_cost`, `build_time`, `max_hp` | recipes (B) |
| `anchor_pin_radius` | how much ground an anchor holds (C) |
| `structure_damage` (per creature class), `siege_aggro_radius` | siege pressure (D) — scales with `distance` |
| `tower_damage`, `tower_range`, `tower_cooldown` | auto-defense (D.2) |
| `repair_rate`, `demolish_refund_pct` | repair/demolish (B.4) |
| `mineral_yield`, `mineral_replenish` | stone/ore nodes (A) |
| `structure_spacing`, `build_within_anchor`, `build_grid` | placement rules + snap grid (B.2, I.3) |
| `hearth_safe_radius` | forward-town respawn aura (F) |
| `max_build_level`, `support_required` | vertical build cap + no-floating-floors rule (J.2) |
| `garrison_cap` (per town), per-tier `hire_cost`, `garrison_upkeep`, `upkeep_interval`, garrison stats | NPC defenders (K) |

The siege step **and** garrison units reuse the **`CR-4`/ecology budget** tunables (LOD,
per-tick ceiling) — they do **not** get their own budget; that's the point of §D.3.

---

## Build plan (phased) — and the hard `SC-3` gate

Tags: **◆** ships/tastes on the precursor · **⬧** needs `SC-3` persistence to matter.

- **BD-0 — Siege/build sim inside the `CR-4` budget. ◆** Prove structures are entities
  the ecology LOD/interest-index/freeze model covers, and the siege step fits the
  existing per-tick ceiling with **no new budget**. Guardrail first (like `CR-4`/`E0`).
- **BD-1 — Harvest wood & stone. ◆** Wood from ecology flora (already there); add
  `MineralNode`s (stone/ore/clay) + timed `MS-2` harvest + structural-material tables.
  *Ships as gathering on the precursor.*
- **BD-2 — The `Structure` primitive: place → build → HP → repair → demolish. ◆ taste /
  ⬧ durable.** One entity, function tag, placement rules, cost, build progress, repair.
  *A within-run "camp" (FS-1) is the precursor taste; real towns need `SC-3`.*
- **BD-3 — Anchors & the Shift-pin loop. ⬧** Anchor pins region within `pin_radius`;
  defend/lose; ties D20 Shift + §W5 suppression. **The headline loop; needs persistence
  + the Shift.**
- **BD-4 — Walls, gates, towers & the siege. ◆ sim / ⬧ durable.** Creatures path to and
  attack structures (extends `CR-2`); walls soak; towers auto-defend; repair races
  attrition. *Sim on the precursor; the point of it is durable.*
- **BD-5 — Towns: composition, guild ownership, permissions, forward-town stops. ⬧**
  Anchor+walls+stash+portal+workshop+hearth+banner; personal/guild ownership (SOC
  permissions); forward towns sustain a deep push (§W4); portal = plantable extraction
  (D15).
- **BD-6 — Field crafting & storage. ◆ taste / ⬧ durable.** `stash`, `workshop`
  (`MS-1` in the field), `hearth` respawn. *Materials → structures + gear at the town.*
- **BD-7 — Persistence wiring (rides `SC-3`). ⬧** Structures / anchor-altered Shifts /
  harvest state into the §W5 event log; hibernate/reload; **season GC**. **No sim
  rework — the persistence hook.**
- **BD-8 — Sieges at scale & world bosses. ⬧** Mega-siege bounded by the realm cap;
  world-boss town sieges; AX-3 agent garrisons. Endgame.
- **BD-9 — Builder mode (the construction UX). ◆** The client build sub-mode: palette
  (affordability-greyed), ghost + grid/adjacency snap, rotate, level select, confirm →
  `run.build` intent, server validation w/ reasons, edit/upgrade/repair/demolish
  sub-mode (permission-gated for guild builds). **Companion to `BD-2`** — you can't
  build without it; build them together. Client UX + the intent surface; ships on the
  precursor (against camps) and carries straight into persistent towns. (Part I.)
- **BD-10 — Building up: verticality in construction. ◆ sim / ⬧ durable.** `floor`/
  `platform`, `pillar`/`stilt` support, buildable `stair`/`ladder`/`ramp` connectors;
  the **support rule** (no floating floors) + `max_build_level`; collapse-on-support-loss;
  verticality as a **defensive advantage** in the siege. **Extends CANON D24** (same
  integer-level axis, no-free-climbing preserved). Follows `BD-2`/`BD-4`. (Part J.)
- **BD-11 — NPC garrison hire (defend while offline). ◆ sim / ⬧ durable.** `barracks` +
  a hire vendor (**EC-2**/**CL-1**); `GarrisonUnit` tiers, patrol + fight-the-siege on
  the shared `CR-4` budget, `garrison_cap`; **upkeep** (new economy sink) + permanent
  loss. Guild towns pay from the guild vault (**SOC**). **`AX-3`** is the smart-agent
  evolution of the same unit. Follows `BD-4` (needs the siege). (Part K.)

When each hardens, **fold into CANON** (extend D21's function enum + new §W-numbers) and
graduate the observable rules into `behaviors/building-and-sieges.md` + `interfaces/`.

---

## Cross-epic build order & the `SC-3` dependency (§L)

**The honest headline: BD is the most `SC-3`-dependent epic in the game.** Unlike the
ecology (rich within-run payoff, §J there), a built town that evaporates at
instance-close is *pointless* — the entire fantasy is durable ground you hold across
sessions. So:

- **What tastes on the precursor now (◆):** the *gathering* (BD-1), the *primitive* and
  a within-run **camp** (BD-2 as an FS-1-style temporary rest), and the *siege sim*
  itself (BD-4) as a within-run spectacle. Useful for building/validating the mechanics
  against the WorldActor — but not the real feature.
- **What needs `SC-3` to mean anything (⬧):** anchors/the Shift-pin loop (BD-3), real
  towns (BD-5), and everything durable. **These should not ship "for real" before
  `SC-3` persistence lands** — build them against the WorldActor, wire persistence when
  it's ready (BD-7), *no sim rework*.
- **Recommended sequence:** `BD-0` (budget) → `BD-1` (harvest, ships now) →
  **`BD-2` + `BD-9` together** (the primitive *and* builder mode — one is useless
  without the other) → `BD-4` siege sim + `BD-10` verticality + `BD-11` garrison against
  the WorldActor → **land `SC-3` in parallel** (already foundationed by the
  Router/WorldActor split) → then `BD-3`/`BD-5`/`BD-6` become durable and `BD-7` wires
  the log → `BD-8` endgame.
- **The three additions slot in naturally:** **builder mode (`BD-9`)** is inseparable
  from `BD-2` (it *is* how you place the primitive) — build them as one. **Verticality
  (`BD-10`)** and **garrison (`BD-11`)** both ride the same ◆-sim / ⬧-durable split as
  the siege: the mechanics validate on the precursor, the payoff lands with `SC-3`.
- **Companion dependencies:** the **Shift (`D20`)** is a prerequisite of `BD-3`;
  **ecology flora** supplies wood (`BD-1`); **`MS-1`** is the workshop's reason to exist
  (`BD-6`); **guilds (`SOC`)** own towns and bankroll them (`BD-5`); **`AX-3`** garrisons
  them (`BD-8`).
- **Ordering vs. the living world:** BD and the ecology (`CR`) **share the `CR-4`
  budget** and both write the world event log — build `CR-4`/`E0` and `BD-0` as **one
  budget effort**, then let the two epics proceed against it. The ecology can ship its
  within-run life first (it has precursor payoff); BD's real content rides `SC-3`
  alongside it.

---

## CANON deltas to fold in (when the design hardens)

- **Extend D21 — `Structure.function` enum.** Add `gate`, `tower`, `workshop`,
  `hearth`, `banner`, and the vertical/garrison set `floor`/`platform`, `pillar`/`stilt`,
  `stair`/`ladder`/`ramp`, `barracks` to the canon `{anchor, portal, wall, stash}`,
  **keeping the one-primitive rule** (still one model; still uniform siege/index/
  persistence).
- **New D — Structural materials & mineral nodes.** Wood from flora; a new `MineralNode`
  layer (stone/ore/clay) with depletion/replenish; structural materials are distinct
  from creature/crafting materials.
- **New D — Build lifecycle, placement & builder mode.** Place → build (cost, progress)
  → HP → repair → demolish (partial refund); placement is server-validated (terrain /
  level / **support** / spacing / within-anchor); upgrade tiers. **Builder mode is
  client UX** — it emits `run.build`/`run.repair`/`run.demolish` intents the WorldActor
  validates + persists; it holds no authority (CANON §S).
- **Extend D24 — buildable verticality.** Player-placed `level`s and connectors reuse
  D24's discrete-integer-level axis and its **no-free-climbing** rule: a `floor` needs
  **support** below (no floating structures), a built `stair`/`ladder`/`ramp` is the
  only way up, height caps at `max_build_level`, and **destroying a support collapses
  what it holds**. Verticality is level-aware in the siege (creatures must breach the
  base to reach upper floors).
- **New §W — NPC garrison.** Hireable `GarrisonUnit` PvE defenders bound to a town/
  anchor, hired at a `barracks`/vendor (EC-2/CL-1), defending **while owners are
  offline** on the **shared `CR-4` budget** (`garrison_cap`); **upkeep is a new chits
  sink** (extends economy.md, a `K`-row) and units are **permanently lost** on death;
  guild towns pay from the guild vault (SOC). **`AX-3` is the smart-agent controller**
  for the same unit.
- **New §W — Siege & defense.** Creatures siege structures (extends CR-2) on the shared
  `CR-4` budget; walls/gates/towers defend; always-running-when-unwatched freeze +
  catch-up; mega-siege bounded by the realm cap.
- **New §W — Town composition & ownership.** A town is a cluster of the primitive; guild
  ownership + permissions (SOC); portal = plantable extraction (evolves D15); forward
  towns sustain Run Level across a push (§W4).
- **Glossary (§G):** `MineralNode`, `BuildRecipe`, `GarrisonUnit`; the new `function`s
  (`gate`/`tower`/`workshop`/`hearth`/`banner`/`floor`/`pillar`/`stair`/`ladder`/`ramp`/
  `barracks`); "town" (a cluster of structures, not a model); "builder mode" (client UX).

---

## Open decisions (yours to call)

1. **Mineral nodes: deplete-and-replenish vs. one-shot-and-reseed.** Do quarries slowly
   refill (like flora regrow) or exhaust and get replaced by new nodes elsewhere on the
   ecology tick? Recommendation: **finite pool + slow replenish** — parallels flora,
   keeps a known quarry valuable, avoids barren permanence.
2. **Function-enum extension.** Ship only the canon 4 first, or the 🆕 town set
   (gate/tower/workshop/hearth/banner) too? Recommendation: **anchor/wall/stash/portal
   first** (the loop), then the town set — but reserve the enum values now so the wire
   doesn't churn.
3. **Griefing: can non-owners damage/demolish your structures?** Recommendation:
   **creatures always can (that's the siege); players cannot demolish others', but a
   rival guild *can* siege an anchor** (PvE-mediated conflict via the Shift, not direct
   PvP) — keeps it PvE-only (aligns AX-3 "PvE-only") while allowing stewardship
   conflict. Confirm.
4. **Offline siege fidelity (§D.3).** Same question as the ecology catch-up — how
   closely must the coarse offline siege match the hot sim? Recommendation: **bounded
   worst-case (you can lose a structure but the loss is deterministic + logged)**, not
   exact replay.
5. **Does building work at all on the precursor, or wait for `SC-3`?** Recommendation:
   **BD-1 (harvest) + a within-run camp (BD-2) ship now; anchors/towns wait for
   `SC-3`** — don't ship a "town" that dies at instance-close and teaches players it's
   worthless.
6. **Portal extraction vs. deep fixed portal.** Does a plantable `portal` structure
   replace or supplement the deterministic hub/deep portals (D15)? Recommendation:
   **supplement** — plantable portals are a built convenience; the structural D15 portals
   remain the guarantee.
7. **Collapse fidelity (Part J).** When a support is destroyed, does the structure above
   **cascade physically** (chain-collapse, heavier sim) or is any **unsupported floor
   simply destroyed** (cheap, deterministic)? Recommendation: **simple destruction** —
   an unsupported floor is removed and drops its contents as loot; skip a physics
   cascade unless playtest demands the drama.
8. **Builder-mode gating (Part I).** Can you enter builder mode **anywhere** (build a
   camp mid-field) or **only inside an anchor's `pin_radius`** (real construction)?
   Recommendation: **both, tiered** — a limited camp set anywhere (FS-1), the full set
   only within held ground; keeps random-spot clutter down while allowing a field rest.
9. **Garrison permanence & fidelity (Part K).** On an offline siege, can a `GarrisonUnit`
   be **permanently killed** while you're logged off (recommendation: **yes** — that's
   the stake), and how faithful is the offline resolution (recommendation: same bounded-
   worst-case as #4)? Also: does unpaid **upkeep disband** units immediately or after a
   grace period? Recommendation: **grace period**, so one missed day isn't catastrophic.
10. **Vertical height cap (`max_build_level`, Part J).** How tall can players build —
    the trade-off between impressive fortresses and render/sim cost + siege legibility.
    Recommendation: **start low (e.g. 3–4 levels)** and raise with load/art data, same
    as the swarm cap.
