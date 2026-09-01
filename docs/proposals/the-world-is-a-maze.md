# The world is a maze you hold open (Epic WG — `WG-11`)

> **DRAFT for review.** Nothing here is built. It supersedes `WG-7`'s *routes* half and
> absorbs `WG-8` wholesale; `WG-7`'s *regions* half already shipped and is the foundation
> this stands on. It changes CANON §B (a finite world) and the feasibility guarantee, so it
> needs sign-off before code.

## 1. The shape of it

You walk out of the Center Hub into a **forest maze**. Past it the ground rises into
**ashfall**, which mazes with mountain ranges instead of trees — you are looking for a pass.
Then **tundra**, which mazes with both. Then **mire**, trees and water. **Field** and
**desert** are not mazes at all: they are the crossings between mazes, open ground you are
trying to get *over* to reach the next region.

All of it funnels down to **d3200 — the end of the world**, the prison biome.

**At most two** routes actually reach the end. Finding one might mean going north 700
units, east 20, then south 1,200, west 200, south 20, east 100, north again. The rest of
the branches are **dead ends that end in dungeons and other things worth finding**.

And the maze **shifts**. A route you found decays unless you **anchor** it. An anchored
route is a **road** — fast travel, because you should not have to wonder. Later, player
towns on held routes link by **teleport**, and a link requires the route between them to
still be held.

## 2. What exists today, measured

The good news is how much of this is already here in pieces, and how much of what is in the
way is *stale* rather than load-bearing.

**Already built and reusable:**

| Piece | State |
|---|---|
| `meld_proto::regions` | The fan decomposed into **cells with adjacency**, biome per cell. 8 cells around the arc at r=400, 65 at r=3000, cell area roughly constant. Mirrored into the ground shader and held by test. |
| `meld_proto::coast` | One signed field: ocean, straits, lobes, basins, rivers, bridges. `is_land` gates `astar_route`, `apply_move`, prop placement and the shader. Everything already respects it. |
| `meld-dungeon` | Glyph-grid authored spaces with a validator that **proves** entrance→exit connectivity via a bounded fixpoint. Exactly the guarantee a maze needs, already enforced. |
| The Shift | Scheduled from `(seed, generation)`, driven by the tick counter, replayable. Re-scatters props and re-cuts peaks. |
| Anchors (`BD-3`) | Suppress the Shift in range, take damage per Shift turned aside, fall if nobody maintains them. The upkeep model is right; only its *shape* is wrong (below). |
| Structures | Exactly two: `Anchor` and `Wall`. Towns are greenfield. |

**In the way, and mostly stale:**

- **`Area` is a radius band, and it is the unit of everything structural.** Elevation
  (`Area.terrain`), dungeon-ness (`Area.dungeon`), the Shift's region (1–3 contiguous
  sections), streaming (`ensure_frontier` by radius), the terrain wire message, and density
  compensation all quantize to annuli. Cells were layered on top for biome *labels* and prop
  *kinds* only. **This is why the world reads as "rings with paths through".**
- `biome_for_distance` — the retired ring-world biome model — is still live in nine places.
- **Seams are dead.** `radialize` calls `self.seams.clear()`: "straight-wall biome seams
  don't survive the bend". They persist only as Gatekeeper/biome data.
- **The dungeon dividers are not walls.** They step ~2.0 units in corridor `y`, which is an
  ANGLE, so at r=1200 that is ~250 world units between rocks.
- **The capstone is a ring.** `seraphic_oubliette` is `EXCLUSIVE` past `[biome_gate] 3000`,
  so *every* cell past r=3000 is prison — ~15,700 units of arc you can arrive at anywhere
  along. There is no funnel today.
- `maze_radial_scale_cap` is pinned at 24 because streaming one deep section already blows
  the 100 ms tick.

**And one thing that is better than it looks:** the backbone already wanders hard — measured
at seed 424242 it swings from +24° at d200 to −40° at d900, 630 units of arc off the centre
line. Nobody follows it because the world is open enough to ignore it. *Enclosure is what
converts that swing from decoration into the only way through.*

## 3. Architecture

### 3.1 Macro — the cell graph

Cells already have adjacency. Make each shared boundary either a **pass** or a **barrier**,
and the maze is the graph.

- Generate a connected spanning route hub→capstone. Feasibility stays **by construction**,
  the same contract `Arena::path` has today.
- Add extra passes so it is **not a tree** — a tree has exactly one route, which is a worse
  maze and a crueller one.
- Leave branches **terminal**, deliberately. A dead end is *a cell whose only open boundary
  is the one you came through*; a wrong route is a terminal branch several cells deep. Both
  are properties of the graph, so both are testable.

### 3.2 Micro — authored parts inside a pass

A cell is ~260 units across at r=400. That cannot produce the 20-unit corridors the design
calls for, and it should not try. The fine structure lives **inside a pass**, built from
`meld-dungeon`'s authored parts with derived openings — which is `WG-8` exactly, so `WG-8`
becomes this section rather than a separate item.

Two scales, each doing what it is shaped for: **the cell graph decides where you can go, the
parts decide what walking it feels like.**

### 3.3 The teardrop — taper the land, do not pick cells

Do not choose "which portion of d3200 is the prison". Make `arc_half_rad` a function of
radius in `coast::sea_depth`, and the sea closes in as you go out. Then *"past the gate only
the prison draws"* and *"the world funnels to a point"* are the same sentence, and the
capstone is a place instead of a band — with no special case, because `is_land` is already
what movement, routing, placement and the shader all ask.

It also pays for itself: deep rings get **smaller** instead of quadratically larger, so the
density cap stops binding and the deep biomes can hold their designed terrain.

⚠️ **The taper fights the anti-east rule.** A funnel makes the centre line geometrically
shortest, and the pull strengthens toward the point. The taper must stay gentle through the
mid-world and the passes near the end must be deliberately off-axis, or the funnel
re-creates the "just head east" degenerate the maze exists to prevent.

### 3.4 What each biome mazes with

Already the documented intent, and already asserted as orderings by
`each_biome_mazes_with_its_own_primitive` — but today those are *density* knobs producing
scatter. They become **porosity plus material**: how many of a cell's boundaries close, and
what closes them.

| Biome | Closes with | Porosity |
|---|---|---|
| field, desert | almost nothing | the crossings between mazes |
| forest | trees | moderate |
| ashfall | mountain ranges | low — you are hunting a pass |
| tundra | trees **and** ranges | low |
| mire | trees **and** water | low |

### 3.4a Landforms are everywhere, and everywhere is visible

**Ranges, bridges, water and height belong in the whole world — decided** — with the prison
ending the one exception, because it is authored rather than natural.

⚠️ **Four hard gates stand in the way, all at the SHALLOW end**: `ridge_min_section = 3`
(r≈81), `bay_min_section = 4` (r≈122), `water_min_section = 5` (r≈170),
`strait_min_section = 6` (r≈225). Every one is justified by keeping the on-ramp gentle — *"a
new party should not meet a wall before it has a reason to want one"*, *"keeps the on-ramp
coastline-free"*. **That justification is exactly what this design retires**: the on-ramp is
now the forest maze, so meeting a wall is the point, and a first ridge at d81 is late rather
than early. Drop the section gates for the real world and keep the **tutorial** protected —
the flag already exists, and a guided first dive is the one place gentleness is still right.

⚠️ **And nothing gates the DEEP end, so the exception needs building.** The capstone has no
maximum — it inherits whatever the generator does. A natural-landform ceiling at the prison
is new work, not a tunable to flip.

**Per-biome character survives "everywhere", and the two must not be confused.** The biome
multipliers are RATIOS and none of them is zero — the lowest in the table is `seized_engine`
at 0.15, and `seraphic_oubliette` sits at 0.2 ("it weeps mercury, not water"). So a desert at
`biome_water_mult = 0.25` is a desert with an oasis, not a desert with no water, and a desert
that mazes with nothing still has mesas and dry washes. **Presence is global; porosity and
character are per biome.** Written down because the next reader of a 0.25 will otherwise
round it to zero.

**"Landforms should be visible" is a RULE, not a nice-to-have.** It is already the most
expensive lesson in this area — `bridges` generated correctly, rode the wire, reached
`TerrainSectionView`, and had no consumer, so every bridge past section 7 was open water the
server walked parties across. Two obligations follow, and both are load-bearing:

1. Every landform the generator makes has a client consumer, held by
   `every_streamed_landform_is_consumed`.
2. What is uploaded each frame is what is **nearest the player**. The fixed-size shader arrays
   shipped taking the first N in SECTION order while claiming to window — and a landform that
   renders only in the shallowest sixteen sections is invisible exactly where the world is
   most worth looking at.

### 3.5 Dead ends are the content

This is what makes wandering worth it, and it is a better answer to *"why would anyone go
north or south"* than any invariant: **the through-route is for progress, the dead ends are
for reward.** Dungeon entrances, caches, bosses, rich resource cells, bounty marks. Nobody
sprints for the exit if the exit is where the least treasure is — and a maze whose wrong
turns are pure tax is a maze players resent.

### 3.6 The prison is a corridor and a door

**The last stretch is a ~200-unit-wide straight run, and the prison itself is a DUNGEON.**
Split at the gate:

- **The corridor is overworld.** The taper produces it — a 200-unit width at d3200 pins the
  end of the taper curve at a half-angle of `200 / (2 · 3200) ≈ 0.031 rad ≈ 1.8°`, which is
  the boundary condition the curve was missing. It is the staging ground: where parties
  gather, merge and decide, with co-op, watchers and clash markers all still working because
  it is just world.
- **The prison is a descent.** `meld-dungeon` is authored glyph-grid space with a validator
  that *proves* entrance→exit connectivity, and it is a hard gate for precisely the right
  reason: *"a dungeon is a committed space (no Town Portal), so an unsolvable dungeon would
  be a trap with no way out."* The end of the world should be a place you commit to. The
  three `set_piece` bosses are already authored content living in procedural ground; this
  puts them where authored content belongs.
- **And it is the perf answer at the worst radius in the game.** A descent is its own space,
  so the rest of the world unloads exactly where `ensure_frontier` is already over budget.

⚠️ **AND THE RAID QUESTION IS ALREADY ANSWERED — the descent half does not survive it.**
[`endgame-bosses.md`](endgame-bosses.md) has Ometus as *"the single biggest encounter — the
maximum raid the engine allows, a multi-phase fight that should feel like the whole world
showing up."* A DG-3 descent is its own instance and a teammate standing outside cannot see
or join you, which is the exact difference `WG-8` drew between a descent and an overworld
region. **So the prison cannot be an ordinary descent.** Either it stays overworld — the
corridor simply ends in the arena — or a committed space has to learn to MERGE, which is new
engine work, not a dungeon def. The three `set_piece` bosses currently at d3200 are a
placeholder for Ometus and carry no `scale_to_warband`, which is why the code reads
one-party today and should not be taken as the design.

### 3.6b The three are RAIDS, and they are not depth-bound

**Decided: Termina, Nestiph and Slake are raid-scoped, they expect very high level parties
wherever they stand, and the only placement rule is that they generate past d1500.** No
depth binding beyond that floor.

⚠️ **`set_piece` DOES NOT DO THIS TODAY, and the gap is the whole decision.** It fixes
`max_hp`, `atk`, `xp_reward` and `encounter_class` — and nothing else. `def`, `ward`,
`speed_stat` and **level** still come from where the spawn was built:

- defence and ward ride `(1 + d/500)^0.7`, so the same boss carries **2.64** at d1500 and
  **4.06** at d3200 — a 1.5x swing across its own legal spawn range;
- and **deep-gated abilities come online by monster LEVEL**, so a shallow spawn fights with a
  smaller kit than a deep one.

That second one is not hypothetical. It already shipped for the gatekeepers: three bosses in
ten had their ONLY party-wide ability gated at level 45 while the first gate boss stands at
24, so a third of the "Worldbreakers" in the game could physically only hit one hero at a
time — caught by
`every_boss_can_go_wide_at_the_level_a_gatekeeper_is_first_met`. A world boss that can appear
anywhere past 1500 inherits exactly that bug, in a worse place.

**So the work is: `set_piece` must fix the WHOLE fight — `def`, `ward`, `speed` and level —
not just the three numbers it currently touches.** Until it does, "not depth-bound" is not
true no matter what the placement rule says.

Two more that follow:

- **The tier is DECLARED, never rolled.** `roll_warband` picks a warband size for
  gatekeepers; a named world boss must not sometimes be a one-party fight. Ometus is the cap
  (Worldbreaker, 4 — `merge_cap_gatekeeper_instances`); whether the three sit at 3 or also at
  4 is a tuning call, but it is authored either way.
- **`[biome_gate]` changes shape.** `seized_engine = 1200`, `nestiphian_cradle = 1800`,
  `hearth_plains = 2400` become a single floor of **1500** for all three, and the comment
  justifying them — *"gated to where a party has any business meeting one"* — is retired: the
  boss is endgame-hard wherever it stands, and the marker is what warns you.
  ⚠️ **They therefore stop being depth-ORDERED** — you may well meet Slake first. That reads
  as intended (three destinations to find, not a ladder to climb), but it is a change and the
  lore/ladder framing in `endgame-bosses.md` assumes an order.
- **Legibility is already handled**, and must stay that way: the warband count rides the
  snapshot as `parties:<n>` and the plate shows it TOPMOST — the one line a player has to act
  on before engaging — and the boss identity now rides the mob tag as `:boss:<key>`. A
  Worldbreaker standing at d1600 is survivable *as a design* only because it announces itself.

### 3.6a The prison is OPENED, not walked to

The door is not a distance — **it is the three world bosses.** This is already canon and
already half-encoded: `[biome_gate]` names each arena in a comment —
`seized_engine = 1200 # Termina`, `nestiphian_cradle = 1800 # Nestiph`,
`hearth_plains = 2400 # Slake`, `seraphic_oubliette = 3000 # Ometus` — and
[`endgame-bosses.md`](endgame-bosses.md) has Ometus *"unlocked only after all three known
bosses fall."*

What that does to this design:

- **The maze has three named destinations before the end.** Termina, Nestiph and Slake each
  hold their own biome at a fixed depth, so the route network must reach three specific
  regions, not just push outward. That is the strongest possible use of *bearing is
  topology*: you are not walking deep, you are finding a place.
- **The through-route stops being one road.** The supply problem is a road to each arena and
  then to the door — which is exactly what `endgame-bosses.md` already asks for from the
  other side: *"a supply road of anchors/forward towns/portals to get there."* The two
  designs met independently, which is a good sign both are right.
- ⚠️ **The Shift IS the antagonist.** Ometus is *"the true cause of every Shift"*, and
  killing it **quiets the Shifts for the season**. So the whole loop of this proposal — a
  maze that rearranges, held open at cost — is the mechanical expression of the endgame's
  villain, and the endgame's reward is the mechanical resolution of it. Nothing needs to be
  invented to connect them; they should just be written as one thing.
- **The unlock is PER SEED — decided — which is per WORLD.** A world *is* its seed: the
  `worlds` row stores it, `WorldActor` outlives its divers, and §W5's claim is that the
  baseline derives from it. So the prison opens **once, for everyone in that world**, and the
  three kills are server history rather than personal progress. That is what makes anchors,
  roads and the two routes matter to people who did not build them.
  - **It is world state, so it persists with the world.** The `worlds` row already carries
    four integers and a small JSON delta; three kill flags (with when, and by whom) go
    alongside the seed, the generation and the Shift log. The **door itself** is world state
    too — the corridor to the prison exists from the start, and it is shut until three flags
    flip.
  - **Anyone may walk through a door someone else opened**, and that is consistent with roads
    being open to all. It is self-gating anyway: entering is free, surviving needs ~level 250
    and an unbroken expedition to get there.
  - ⚠️ **It implies A SEASON IS A SEED.** `endgame-bosses.md` scopes the same unlock "per
    world, per season"; if the bosses are per-seed and a season is three months, then season
    rollover is a **new seed** — bosses stand back up, the prison shuts, and the world is new.
    Clean, and it matches the seasonal framing. But it also means **everything players built
    into the world goes with it**: roads, anchors, towns and links are world state. Account
    progression (the Vault, gear, Meld skills, hunter rank) is not. That bargain — *your
    stuff persists, your footprint does not* — has to be stated to players up front, because
    discovering it at rollover is how a playerbase leaves. §Open in `endgame-bosses.md` and
    it should be settled there, not here.
  - *Future, explicitly out of scope here:* there is likely room for **persistent,
    non-seasonal worlds** alongside the seasonal ones — a seed that simply never rolls over,
    where a road held for a year is a road held for a year. Nothing in this design forbids
    it: the unlock is per seed either way, and "a season is a seed" is a statement about the
    seasonal mode rather than about worlds. Left as a future item so the seasonal path does
    not accidentally hard-code the rollover.

**It also settles two loose ends.** No town can exist in a 200-unit corridor, so "the final
approach is always on foot" stops needing a rule. And the natural-landform ceiling from §3.4a
becomes trivial: the corridor is the exception, and everything inside the door is authored.

## 4. The loop: Shift → anchor → road → link

1. **Explore.** Slow, wandering, dangerous. You find a route.
2. **The Shift erodes it.** A route is knowledge, and knowledge is perishable.
3. **Anchor it.** Costly up front, costly to keep — an anchor nobody maintains falls.
4. **An anchored route is a road.** Fast travel along held passes, so you never re-solve
   ground you already paid for.
5. **Towns on held routes link by teleport.** **A link requires the route between them to
   still be held** — so the network *is* the anchored graph, and a Shift that cuts a corridor
   genuinely severs it.

Which is why the world persists, why anchors have upkeep, and why **a route is worth more
than anything you can carry home.**

**A road is open to everyone, and it should be VISIBLE — decided.** Retexture the ground
along a held pass so a road reads as a road from a distance. That is not decoration:

- It makes the held route **discoverable in the world** rather than through a UI. You follow
  a road because you can see one, which is how a route stops being a wiki entry.
- It shows **upkeep**. A corridor losing its anchors should visibly degrade back toward
  wilderness, so the Shift and the maintenance economy are legible without a single HUD line.
- **The free-rider problem is already solved by a shipped rule.** Anyone may repair a
  structure; only its owner may demolish it. So a road everyone uses is a road anyone can
  haul ore out to, and it survives its founder logging off. No new mechanic needed — and the
  per-warp chit cost on a *link* is the sink that keeps the network from being free.

⚠️ **Two traps are already waiting for the visible road, both documented in blood.**
`ground_biome.wgsl` mirrors the region decomposition analytically, but ranges and bridges ride
as fixed-size uniform arrays — and *"TRUNCATION IS NOT A WINDOW"*: both shipped taking the
first N in SECTION order (the shallowest in the world, kept forever) while their comment
claimed windowing, so past sixteen you walk into terrain the ground renders as flat. A road
array inherits that exactly, and roads are longest and most numerous precisely where the
player is deep. Second: *"a landform the client never consumes does not exist to the player"*
— `bridges` was plumbed proto-to-view with no consumer, so every bridge past section 7 was
invisible while the server happily walked parties across it.
`every_streamed_landform_is_consumed` exists for this and a road belongs in it.

⚠️ **An anchor pins the wrong shape.** `anchor_pin_radius = 90.0` holds a disc of ground —
another ring-era construct. Under a maze it must hold **passes**: a route of N cells costs N
anchors and N upkeep, which is an economy rather than a radius.

**Travel rules, decided:**

- **Town Portal only ever returns you to the Center Hub.** It stays the way home. The
  dangerous walk out is the loop's teeth and a friendly town two cells away would pull them.
- **Warping to a player town costs chits, every time.** A recurring sink, not a one-off
  unlock — register it in `behaviors/economy.md` alongside the existing sources.
- **You may only warp to a town you have personally reached on foot.** Without this, the
  first crew to solve the maze builds a ladder to d3200 for the entire server. With it, every
  player walks it once and the network is a veteran's shortcut over earned ground.
- **Founding already requires the route** — you must haul the stock there — so no rule is
  needed to stop towns appearing where nobody has walked.
- **The teardrop forbids towns near the end.** Minimum separation is a world distance and the
  world narrows, so past some radius another town cannot fit. The final approach to the prison
  is always on foot, always through maze. *State this as intended*, or a later tuning pass to
  the taper or the spacing quietly reopens it.

## 4a. Season pacing — and the curve is NOT the clock

**Target: a season is ~3 months, it should LAST ~3 months, and Ometus should fall in the
last one.** That is the first hard number this design has to answer to, and it changes which
levers matter.

⚠️ **Modelled, and it says the XP ladder finishes far too early.** Level 251 is what d3200
ground demands. Walking the real curve (`fights_per_level`: base 2, +1 every 5 levels to the
knee at 100, then +1 every 15):

| level | at-level fights | hours | @10 h/wk | @20 h/wk |
|---|---|---|---|---|
| 100 | 1,129 | 19 | 1.9 wks | 0.9 |
| 200 | 3,514 | 59 | 5.9 wks | 2.9 |
| **251** | **4,985** | **83** | **8.3 wks** | **4.2** |

> **Assumption, and it is a single thread to pull.** Hours come from one documented anchor —
> *"a continuous expedition reaches only ~d1150 / level ~43 in four hours"* — which implies
> **~60 at-level fights an hour**, i.e. a fight a minute, back to back. Everything else a dive
> does (walking the maze, gathering, hauling, building, dying) makes the true rate lower and
> the hours higher. **Halve the rate and level 251 is 166 hours**, which is 16 weeks at
> 10 h/week and still only 8 at 20. The conclusion survives the sensitivity: a committed
> group reaches the cap inside a season with room to spare. Re-derive this against a played
> dive (`mcp/`) before tuning anything to it.

**So the ladder is not what makes a season last.** What does is a mechanic already shipped
and easy to overlook: **levels are DIVE-SCOPED.** Every hero starts at level 1 at the Center
Hub, level comes only from XP earned on that expedition, and an expedition survives a
session boundary *only* through an inn — which is a save point that can be destroyed with
you inside it. Those 83 hours are therefore **one unbroken expedition**, and a wipe at hour
70 costs all of it.

That is the clock, and it has three hands:

1. **Expedition fragility.** A long dive is a standing bet, and the Shift, the ground and a
   town that can fall are what price it.
2. **The three-boss search.** Termina, Nestiph and Slake stand at unknown bearings past
   d1500 (§3.6b), so finding them is a server-wide search rather than a walk to a coordinate.
3. **The road.** Four parties at the door needs a supply road built and *held* against the
   Shift the entire time — `endgame-bosses.md` already asks for exactly this from its own
   side.

Which maps onto the target without forcing anything: **months 1–2 are the search and the
road; month 3 is the prison opening and the raid.** The three-boss gate *is* the season's
structure.

⚠️ **Two failure modes, and the band between them is narrow but MEASURABLE:**

- **Too safe** — reliable inns, cheap roads, a forgiving Shift — and the season collapses
  back to the XP curve: someone caps in week 4 and Ometus dies in week 5, leaving two months
  of nothing.
- **Too fragile** and no group ever completes an 83-hour unbroken expedition, so the season
  has no climax at all and the last month is the same as the first.

Neither is a matter of taste. Both are simulable from the curve, the wipe rate and the Shift
cadence, and this is the number to model *before* the maze is tuned.

### 4a.1 Backtracking pays 5%, and that is the brake

A real route doubles back (§6), so a lot of the maze is re-crossing ground BELOW your level.
That is fine and it is already priced: `xp_after_level_gap` pays in full within
`xp_gap_grace = 3` levels above an encounter and falls linearly to `xp_gap_floor_mult = 0.05`
by `xp_gap_zero = 12` levels above. Monster level is `d / 12.5`, so **twelve levels is one
hundred and fifty units of depth** — a 1,200-unit backtrack is ~96 levels down, far past the
floor. Essentially *all* meaningful backtracking pays the 5% floor, with no new mechanic.

Three things follow, and the first corrects a worry stated above:

- **The maze is a much weaker season-length lever than raw path length suggests.** Tortuosity
  adds path, but that path splits into at-depth ground (full pay) and backtrack (5%). The
  supply increase is therefore close to the at-depth share alone, not the whole multiplier —
  so "the maze finishes the season early" is a far smaller effect than it first looks, and
  may be a rounding error. Model it as two streams, never as one.
- **It is a genuine brake on hours.** Time spent at the 5% floor is wall-clock that buys
  almost no level, which pushes the 83-hour figure up rather than down. That is *helpful* for
  a three-month season, and it is why the model above should be re-derived from a played dive
  rather than from the curve alone.
- ⚠️ **The risk is FEEL, not balance.** `regrow` restocks cleared ground, so backtracking is
  not empty — it is populated with things that pay 5%. Respawned *and* worthless is the worst
  combination in the list: an over-levelled party being chased down a corridor by creatures
  worth nothing is a tax with no decision in it. **The road is the answer, and it is the same
  answer as everywhere else** — backtracked ground is by definition ground you have already
  crossed, which is exactly the ground that becomes a road. That makes roads not a
  convenience but the mechanic that pays off the maze's own structural cost.

## 5. Invariants

The contract, all testable:

1. **Global feasibility.** A route hub→capstone exists, by construction, at every generation.
   *Local* feasibility goes away on purpose — that is what makes a wrong route possible.
   **AT MOST TWO routes reach the prison zone** — decided. Two is what makes a route a
   server-level asset worth anchoring and worth contesting; more and the maze is porous, and
   holding one stops meaning anything.
   ⚠️ **At N=2, invariant 2 is carrying the whole design.** One Shift may close one of the
   two; the next must not be able to close the other. Whatever guarantees connectivity has to
   be evaluated against *the routes that remain*, not against the pair, and if the floor is
   ever one route then a single Shift is one step from sealing the world.
2. **Connectivity survives every Shift** — a Shift must never seal the last real path, the
   soft-lock risk of the whole design. **Enforced by RE-ROLLING the region until the graph
   holds**, which is cheap for one reason worth building around: *roll the topology, validate,
   then generate the content.* Whether a set of open boundaries breaks hub→capstone is a
   reachability query over a few thousand cells — microseconds — while regenerating a
   region's terrain and props is the expensive half. Re-roll only the boundary configuration
   and pay for content once, and the retries are free; re-roll the whole regeneration and
   every attempt costs a section rebuild.
   - **Bounded, with a no-op fallback.** N attempts, then the region simply does not shift —
     the same discipline as every other rejection loop here (`fa < extra * 12`) and as
     `meld-dungeon`'s bounded fixpoint. A Shift that finds no legal rearrangement is a Shift
     that did not land, which the anchor path already models.
   - ⚠️ **It costs §W5 a property, and the wording has to change.** Today a Shift's content is
     a pure function of `(seed, generation)`, so any generation is computable on its own. Once
     acceptance depends on the state of the whole graph — which includes previously shifted
     regions *and* player anchors — generation N is only reachable by replaying 1..N in order.
     The log already replays in order so this is a cost rather than a blocker, but "the
     baseline is a pure function of the seed" stops being literally true and becomes "a pure
     function of the seed and the ordered log".
   - ⚠️ **Say whether the floor is ONE route or TWO.** "Does not break the graph" is
     ambiguous at §3.6a's cap of two. A floor of one can never soft-lock, but the world can
     erode to a single route permanently; a floor of two never erodes but constrains the Shift
     much harder. Recommended: **floor of one, and anchoring is how a group protects the
     route it knows** — which puts the second route in players' hands rather than the
     generator's.
3. **Tortuosity floor and ceiling.** The genuine route's length must exceed some multiple of
   the radial distance, and span real bearing — so nobody tunes the maze into a straight
   shot. A ceiling too, or the walk becomes tedious rather than interesting.
4. **Dead ends pay.** Every terminal branch past some depth carries content. Assert it;
   an unrewarded dead end is a bug, not flavour.
5. **No town past the taper threshold**, and **no link without a held route**.
6. **A season lasts a season.** The gap between "the first group can reach Ometus" and "the
   season ends" is held to roughly the last month — modelled, not hoped. See §4a; the lever
   is expedition fragility and the three-boss search, never the XP curve.
7. **The maze is derived, never stored.** Boundary state must be a pure function of
   `(seed, generation, cell)` plus the anchor set, or §W5's "the baseline is a pure function
   of the seed" stops being true and the world can no longer be replayed from four integers.

## 6. What this breaks

- **`route_point_at` assumes the route crosses each ring exactly once** — its own doc says
  so. A doubling-back route crosses many times. Same for `path_y_at`. Both feed the Shift's
  rescue, the deep-start harness and hunts' "where to look".
- **`ensure_frontier(reach)` streams by max radius**, which assumes outward progress. A route
  that goes inward for 1,200 units does not.
- **"Endless" ends.** `wg4_radial_world_streams_endlessly_outward` asserts the opposite of a
  finite teardrop. CANON §B needs the amendment, not a quiet test edit.
- **`Area` stops being the structural unit** — terrain, the Shift, streaming and the terrain
  wire message all move to cells. This is the large piece, and it reaches the client.

## 7. Open

- **Traversal budget — and it is NOT "less content per minute".** Creatures, nodes, chests
  and dungeon entrances sit on *every* path, so a longer route is more content, not more
  walking between content. Two real consequences instead:
  - **More fights per unit of DEPTH changes the level curve.** Difficulty rides distance
    while XP rides fights, so tripling path length at a given radius roughly triples the
    fights you take to reach it — you arrive deeper *over*-levelled relative to today. That
    is plausibly a *fix*: d3200 demands ~level 251, supply is `1 + 0.078d`, and the two only
    cross at d≈3350, which is why the far end is barely reachable. A maze raises supply
    without touching the curve. Wants measuring before it is celebrated.
  - **Backtracking is the only genuinely empty time.** The first traversal of a cell is
    content; walking back through one you already emptied is not. `Arena::regrow` already
    restocks cleared ground on a timer, which is most of the answer — and roads are the rest.
- **Does a dead-end dungeon move when the Shift re-rolls its cell?** **Yes — decided.** A
  dungeon travels with the Shift, which gives an anchor a second job beyond holding the road:
  **hold your farm.** That matters because it gives anchors a purpose for players who are not
  pushing the frontier, and it is a reason to put a town somewhere other than on the through
  route. Implies dungeon entrances are cell content re-rolled per generation, and that an
  anchored cell keeps the one it has.
- **What does the player see?** A route like the one above is unmemorable. The Explorer's map
  perk and anchor visibility go from convenience to core, which is the design telling us the
  Explorers' own fiction — *they map and anchor the unstable world* — is finally mechanical.

## 8. Staging

1. **Barriers on cell boundaries** with passes and a guaranteed spanning route. Additive,
   reuses range/water placement, leaves streaming alone. The world becomes chambers and
   passes — the biggest change in feel per unit of risk.
2. **Dead ends and content placement** on terminal branches.
3. **The taper**, via one term in `coast`. Unlocks the density cap as a side effect.
4. **Anchors hold passes; a held route is a road.**
5. **Cells replace `Area` as the structural unit** — terrain, Shift, streaming, wire, client.
   The big one, and it wants its own epic.
6. **Authored parts inside passes** (the old `WG-8`).
7. **Towns and links** (with `BD-5`).
