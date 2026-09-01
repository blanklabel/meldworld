# World shape & exploration (Epic WG — `WG-6`, `WG-7`)

**Status: PROPOSAL.** §3 and §4 are not built. It exists because two pieces of play
feedback — *"creature movement in the overworld makes no sense whatsoever"* and
*"every biome just kinda looks like… a big open field, there isn't any sense of
exploration"* — turned out to have three separate causes at three different levels,
and only two of them were bugs. The bugs are fixed (`CR-10`, and the arithmetic half
of `WG-6`). The third is a decision about what the world *is*, and §4 now carries the
owner's direction for it: **keep the world radial, give the regions shapes, and put
rivers and mountain ranges in the way that you have to walk AROUND.**

The single most useful thing in here: **every source of impassable large-scale terrain in
the overworld is set to zero** (§4.0), the A\* machinery to route around barriers is
already built and correct, and the mesas were disabled for a *rendering* reason rather
than a design one. The barrier half of `WG-7` is much closer to hand than it looks.

Read [`worldgen-wg.md`](worldgen-wg.md) and
[`../behaviors/world-generation.md`](../behaviors/world-generation.md) first — WG-4's
radial fan is the geometry everything below argues with.

---

## 1. What was measured

Every number here comes from `Arena::generate(.., tutorial: false)` — the real
procedural world, not the onboarding corridor — at the shipped balance, and every
one is reproducible from a seed.

### 1a. The maze was thinning with depth, along two axes, and only one was compensated

`obstacles_per_area` is a count **per section**. Two independent things spread that
count ever thinner as you walk out:

- **The arc.** WG-4 bends a fixed-width corridor into an arc that grows with radius.
  At r=1200 a section spans ~7,000 units of arc. This was compensated (capped).
- **The thickness.** `area_length_growth` grows sections from 13 units thick near the
  hub to 184 by d1560, so the same count also spreads over 5.7× the radial extent.
  **This was not compensated at all.**

Their product is exactly the section's world area over the base area's, which is now
the one function `maze_fill_scale`. Measured before the fix, seed 424242, streamed to
d1700:

| ring | obstacles / 1000 u² | mean prop spacing |
|---|---|---|
| 40–100 | 7.38 | 5.9 tiles |
| 200–300 | 4.78 | 7.2 |
| 500–800 | 3.23 | 8.8 |
| 800–1200 | **1.49** | **13.0** |

(Mixed biomes, as generated. Pinned to forest — which is where the claim "a THICK wood"
actually has to hold — the same collapse runs 3.5 → 7.1 → 12.5 tiles of spacing.)

**The measurement to actually steer by is props in the camera's view, standing on the
world's own route.** Density per 1000 u² is right but abstract; this is the thing the
player looks at. Pinned to forest, obstacles within 40 world units of
`route_point_at(d)` — before these fixes, and after:

| standing at | before | after | reads as |
|---|---|---|---|
| d60 | 69 | **136** | a wood |
| d200 | 42 | **65** | a wood |
| d550 | 11 | 11 | parkland |
| d900 | 3 | 3 | a field with three trees in it |
| d1200 | **0** | **0** | a plain |

⚠️ **Past roughly d550 the arithmetic fixes change nothing, because the cap binds either
way.** That is the honest scope of what shipped: the shallow rings roughly double, every
fourth ring stops being a plain, and **the deep world — where the game is actually
played — is exactly as empty as it was.** Zero trees in view, in a forest, at d1200. This
table is the acceptance criterion for §3: props-in-view along the route must not collapse
with depth.

The existing regression test (`the_world_does_not_empty_out_as_it_fans_open`) samples
out to r=280 — *inside* the radius where the cap still holds — so the collapse past it
was unguarded, and past it is where the whole game takes place.

### 1b. Every fourth ring of the world had no terrain in it at all

`dungeon_every = 4` marks every 4th section a procedural dungeon, and the maze fill
was skipped entirely for one: *"rooms-and-corridors instead of the scattered fill."*
That was true when a section was a 20-tile corridor with three rooms in it. After WG-4
a section is an annular band spanning the full 340°, so two divider walls are a
rounding error across it. Measured, same sweep:

- procedural-dungeon sections: **0.167** obstacles per 1000 u²
- ordinary sections: **4.92** — a **30× gap**
- section 16 (forest, `forest_obstacle_mult = 7.0`, *"a THICK wood"*): **29 obstacles
  across 900,893 u²**, mean prop spacing **88 tiles**

A quarter of the world was open ground, and you cross one every fourth section.

**And the walls themselves are no longer walls.** A divider steps in corridor y at
`dungeon_wall_radius * 1.8` ≈ 2.0 units. Corridor y is an **angle**, so at r=1200 that
step is ~250 world units: the "wall" is a line of rocks a quarter-kilometre apart with
a door gap nobody needs. This is the same bent-frame mistake documented against the
maze fill (*"the forest asked for 392 trees and placed 90"*) — the count was
compensated, the **spacing** never was.

It is deliberately **not** fixed by making the wall real: at r=1200 a genuine ring wall
is ~3,600 props, twice per section. A ring-scale "room" is not a room, and that is the
finding, not the bug.

### 1c. Wandering creatures were vibrating in place

The wander destination was re-rolled inside the movement pass **every tick** — a fresh
angle ten times a second at the 100 ms authoritative tick — so a creature was chasing a
point that teleported around its leash faster than it could walk. Measured over 30 s,
400 creatures, no players in the arena:

| | before | after (`CR-10`) |
|---|---|---|
| path walked | 47.8 tiles (full speed throughout) | 28.8 (it now pauses) |
| net displacement | **0.87 tiles** | 4.63 |
| furthest from start | **1.93 tiles** | 7.30 (leash is 9.0) |
| straightness (net/path) | 0.018 | 0.186 |

98% of the motion cancelled. And because the client picks its 8-way facing off
frame-to-frame movement (`hd2d::animate_chars`), the sprites spun on the spot as well.
Fixed and guarded, including a vacuity check that puts the per-tick re-roll back and
proves the new bar fails.

---

## 2. Why raising the density cap is the wrong answer to 1a

After the two fixes the shallow and mid rings read as a wood and the deep ring is still
about 3× short. The obvious next move is to raise `maze_radial_scale_cap`. Measured, seed
424242, biome pinned to forest, streamed to d1300 — density as obstacles per 1000 u² with
mean prop spacing in tiles beside it, `ms/tick` = `step_creatures`, `stream ms` = one deep
section entering through `ensure_frontier`. Best-of-N in release, because this box is
shared and single-shot timings on it are noise:

| cap | props | d 40–100 | d 400–700 | d 900–1200 | ms/tick | stream ms |
|---|---|---|---|---|---|---|
| 1 (no compensation) | 995 | 1.65 (12 t) | 0.17 (38 t) | 0.11 (47 t) | 12.3 | 50 |
| **24 (held)** | 27,451 | 27.4 (3 t) | 5.89 (7 t) | 2.83 (9 t) | 14.8 | **181** |
| 60 | 66,081 | 27.4 (3 t) | 14.7 (4 t) | 7.05 (6 t) | 17.5 | **372** |
| 120 | 122,652 | 27.4 (3 t) | 29.3 (3 t) | 14.1 (4 t) | 22.1 | **652** |

**The binding constraint is the last column, not the density argument.**
`ensure_frontier` runs *inside* the authoritative tick (`game.rs`), so streaming one deep
section is already a **181 ms stall on a 100 ms tick** — over budget by 1.8× whenever a
player walks into new ground, before any of this was touched. Raising the cap doubles a
stall that is already broken. World *generation* is not the problem (best-of-5 is
unchanged by these fixes, inside the noise; an early "4× slower" reading was contention
from my own concurrent test runs — worth recording because it nearly got the correct fix
reverted). Bucketing the props is only 7–21% of the tick, so caching the blocking field
does not recover the per-tick half either: a denser world genuinely makes creature
movement more expensive. On a single-owner loop (CANON §S) this is `SC-2`/`CR-4` work, not
a tuning decision.

**And the deeper objection is that the budget is being spent where nobody can go.** The fill
is scattered uniformly across the section's whole lateral extent, which the fan bends
into ~7,000 units of arc at r=1200. A player occupies a neighbourhood tens of units
across and reaches it along the clear path and the trail web. The overwhelming majority
of every deep ring is content that no player will ever stand near. Constant *ring*
density is not the goal and never was — it is a proxy that gets more expensive the
further it drifts from what it is proxying for.

---

## 3. `WG-6` — spend the fill where the player can actually be

> ⚠️ **PARTLY OVERTAKEN, BUT NOT RETIRED — and a measurement that said it WAS retired was
> wrong.** Two corrections, in order.
>
> **1. The near-field symptom had a different cause, now fixed.** `path_clear_radius` was
> asked in the CORRIDOR frame, where `y` is an angle, so the guaranteed route's slit —
> authored at 1.9, deliberately narrow — fanned into a cleared swath 438 world units wide at
> d1200, centred on the one line every player walks. Props per 1000 u² within 50 units of the
> trail measured **0.0 at d200, d550 AND d1200**, against ring densities of 13.1 / 5.2 / 2.5.
> Fixing the frame (`clear_of_routes`) took props-in-view on the route at d1200 from **0 to
> 13** at identical prop count and no measurable perf cost. **Note §3 could not have worked
> before that fix**: a 55-unit band placed inside a 438-unit exclusion produces a world with
> no fill at all.
>
> **2. And the coverage number that appeared to kill §3 was measuring a DIFFERENT bug.** A
> naive measurement said a 55-unit band around the route network already covers 100% of a ring
> to d550 and 62-90% deep — i.e. the band *is* the ring, so banding buys nothing. That is an
> artifact of the web: its trail nodes are offset by `wrng.range(6.0, lat)` in **corridor**
> units, so at depth a single fork sweeps thousands of world units across the fan. Web length
> summed **18,000-24,000 units per ring against the backbone's 222-2,430**. Measured on the
> **backbone alone**, coverage at 55 units is:
>
> | d | 200 | 550 | 900 | 1200 |
> |---|---|---|---|---|
> | coverage | 26% | 15% | 2% | 11% |
>
> So the ring's interior really is mostly unreachable, §3's premise holds, and the section
> below stands — with the band sized by **visibility** rather than budget and following the
> route **network**, per the owner's constraint recorded in `WG-6` ("you shouldn't be able to
> tell you're off trail, otherwise it loses being a maze"). **Convert the web offsets through
> the arc stretch first**, or any coverage number is measuring the fan rather than the trails.

**Proposal (as written, unbuilt): place the maze fill relative to the ROUTE NETWORK, not
uniformly across the ring.** The design intent already says so — *"only the winding clear path (plus the
branch detours) stays open"*. The fill is the **walls of the maze**. A maze's walls
belong beside its corridors; scattered evenly over a ring that has no corridors in it,
they simply vanish.

Sketch:

- Sample each fill attempt near the route network (`Arena::path` windows + `corridor_web`
  edges intersecting the section) rather than uniformly over `±lateral`: pick a segment,
  pick `t` along it, offset by a band width.
- The offset must be converted **into corridor y through the arc stretch**, because
  corridor y is an angle — the same bent-frame discipline `BlockGrid` already applies to
  spacing. Getting this wrong is how the fill vanished in the first place.
- A new `[worldgen] maze_fill_band` (world units) sets how far from a trail the terrain
  reaches. Past it: open ground, honestly and deliberately, instead of open ground by
  accident.
- Every existing rejection stays: the clear tube, the web clear, terrain level, the
  `BlockGrid` spacing, the connector prune. Feasibility remains by construction.

Why this is strictly better than raising the cap: the count needed becomes proportional
to **route length × band**, which grows roughly linearly with depth rather than
quadratically. High density where the player walks, at a per-tick cost that does not run
away — and the ring's unreachable interior stops being paid for at all.

Open questions worth settling before building it:

1. **Does off-trail-is-open read as worse?** Trails become corridors through dense
   terrain and the space between them becomes visibly empty. Today it is *all* empty, so
   this is an improvement either way — but it changes the character of going off-piste,
   and off-piste is where the Shift can rearrange the ground under you
   (`rescue_stranded`). Sparse landmarks scattered into the interior (§4) are the likely
   answer.
2. **What happens to the whole-section procedural dungeon?** §1b argues a ring-scale room
   is a category error. Options: retire `dungeon_every` (set it to 0 and let DG-3's
   authored entrances be what a dungeon means), or make it a **local** enclosure placed
   on the route like an entrance is. The second keeps the feature; the first admits it
   was superseded. Either is fine; carrying it forward as-is is not.
3. **Does `creature_radial_lane_cap` want the same treatment?** Creature count already
   scales with thickness (each lane walks the section's whole length), so creatures have
   only the arc axis and it is compensated — but they thin past the cap too (2.65 → 1.12
   per 1000 u²), and creatures are the dominant per-tick cost. Route-relative placement
   would help them for the same reason. This is `CR-4` territory.

---

## 4. `WG-7` — the world is radial; the regions and the routes should not be

This is the part that is not a bug. It is the answer to *"there isn't any sense of
exploration"*, and the direction below is the owner's: **keep the world radial, give the
regions shapes, and put things in the way that you have to walk AROUND to reach the end
of the world.**

### 4.0 The finding that reframes all of it

**Every source of impassable large-scale terrain in the overworld is set to zero.**

| switch | value | what it turned off |
|---|---|---|
| `terrain::CLIFF_HEIGHT` | `0.0` | the impassable cliff-mesas in the height field |
| `[worldgen] terraces_per_area` | `0.0` | discrete raised terraces |
| `[worldgen] max_level` | `0` | discrete elevation levels / cliff walls |

Nothing in the world can stop you or make you turn. The authored peaks that *do* exist are
deliberately **walkable domes** (`PEAK_MAX_ASPECT` keeps a summit climbable from any side),
so a mountain today is something you walk *over*, never *around*. Together with §1's
density collapse, that is the whole of "every biome looks like a big open field."

And the machinery to undo it is **already built and already correct**:

- `terrain::height` is a pure function of world position with a per-run offset — the
  terrain is *already* 2D and not radially banded. Only the biome skin and the props are.
- `terrain::routable` / `walkable` are slope thresholds over that field.
- `Arena::astar_route` already routes the guaranteed backbone **around** unroutable ground,
  and it does so *honestly*: it costs each edge by sampling the edge's **bent** arc at
  ~1-world-unit intervals, so it cannot leap a ~2u cliff ring at large radius where one
  corridor cell spans hundreds of world units tangentially.
- Feasibility-by-construction therefore already survives detours. Nothing about "walk
  around it" needs new routing.

### 4.0a Measured: re-enabling the old mesas would NOT give you barriers

Before designing on top of that machinery, I put the switch back and measured what it
produces. Coverage is the share of a 1200x1200-unit patch around the hub that
`terrain::routable` refuses (seed 424242's offset, 6-unit sampling):

| cliff amplitude & mask | unroutable ground | what it is |
|---|---|---|
| `CLIFF_HEIGHT = 0.0` (as shipped) | **0.00%** | nothing in the world can block you |
| `11.0`, original band `smoothstep(1.15, 1.30)` | **1.05%** | sparse isolated buttes |
| `11.0`, band widened to `smoothstep(0.50, 0.65)` | **6.73%** | scattered blobs |

Two conclusions, and the second is the important one:

- **0.00% is the decisive number for §4.0.** It is not "the world has gentle terrain"; it
  is that *no point in the overworld is unroutable*, so no barrier of any kind exists.
- **The old mesa settings were mechanically pointless as well as ugly.** At 1.05% coverage
  you essentially never have to walk around anything — which is why they read as blocky
  props rather than as terrain. And widening the mask does not fix that: at 6.73% the
  repo's own feasibility test still passes (so coverage is not inherently dangerous), but
  what you get is *more scattered blobs*, not ranges. An isotropic threshold over a sum of
  sines cannot produce a long connected ridge with a pass in it, at any amplitude.

**So a barrier has to be STRUCTURED, not a threshold on noise** — a range with a spine, a
river with a channel — and, like the existing `Seam`, it must carry a **guaranteed pass**.
That is not merely tidier: a structured barrier with a designed pass can cover as much
ground as the design wants while feasibility stays true *by construction*, whereas an
isotropic mask buys coverage by rolling dice against the route and leans on
`generate_with`'s twelve terrain-offset re-rolls to notice when it has sealed one.

The mesas were switched off for a **rendering** reason, not a design one — quoting the
source: *"even sparse, they rendered as stair-stepped blocky WALLS (the coarse ground grid
can't smooth an 11u vertical face) that the player kept reading as a corridor… The cliff
mask + slope-collision + A\* routing all still work if we bring dramatic terrain back
later."* So the blocker to `WG-7`'s barrier half is **how a vertical face is drawn**, and
that is worth stating plainly because it is a much smaller problem than "the world needs
new geometry."

### 4.1 Water first — and LAKES before rivers

⚠️ **This reverses the ordering an earlier draft of this section gave.** Rivers were put
first because they dodge the cliff-face rendering bug. Lakes are cheaper still, and the
reason is worth stating precisely.

**Ponds already exist.** `pond` / `bog_pool` / `frozen_pond` are real obstacle kinds that
the world already places: the field/forest scatter list is `["tree", "boulder", "pond"]`,
the tundra's is `frozen_pond`, and the **mire's entire maze fill is `bog_pool`** at
`mire_obstacle_mult = 7.5`. So impassable water as *terrain* is shipped, played, and it
already reads well — the flooded mire is the existing proof that this works.

**But every body of water in the game is at most 5.6 units across.** Both the sparse
scatter and the dense fill draw `radius = obstacle_min_radius + u·(obstacle_max_radius −
obstacle_min_radius)` = **1.1 … 2.8**. So "add ponds" is really *let a pond be
pond-sized*, and a lake is a pond whose radius was allowed to grow.

**And that costs almost nothing to draw.** A water obstacle is already an *organic blob*
mesh — `hd2d::blob_mesh(28)`, commented "organic pool outline, not a circle" — scaled by
`Vec3::splat(r * 2.0)` and painted with the animated per-biome water material. A lake is
that same primitive with a bigger `r`: **no new mesh, no new material, no new shader, no
new collision path.** A river is *not* free in the same way — the blob is scaled
uniformly, so an elongated channel needs its own geometry. Hence: lakes, then rivers.

**A lake is also the best-behaved barrier for the detour budget (§4.3).** It is convex, so
routing around one costs the semicircle against the chord — `πr / 2r` = **π/2 ≈ 1.57×** the
straight line, *bounded*. A ridge or a river can force an arbitrarily long detour. So a
lake is simultaneously the cheapest barrier to build, the cheapest to draw, and the one
whose cost to the player is provably small.

#### ⚠️ You cannot just raise the radius — measured

`BlockField::new` sets its spatial-hash cell from the **largest radius in the world**:
`cell = (max_radius * 2).max(8.0)`, and `blocks()` sweeps `±ceil((max_radius + radius) /
cell)` cells. That maximum is **global**, so one large body of water coarsens the grid for
every prop everywhere and pushes the per-query scan back toward the linear one whose
removal is documented three lines above it (the O(creatures × props) pass that cost
**1.7 s a tick**).

Measured on a world streamed to d1300 — 23,069 props, 11,836 creatures — adding **one**
water body parked at (9000, 9000), where it blocks nothing and nobody can ever reach it:

| | per-tick `step_creatures` |
|---|---|
| baseline (largest prop radius 2.80) | **15.8 ms** |
| + one r=30 body | 15.5 ms (free) |
| + one r=80 body | 18.0 ms (+14%) |
| + one r=150 body | **23.6 ms (+50%)** |

So **water up to ~60 units across is free today**, which is already a real lake at shallow
depth. Anything larger needs `BlockField` to bucket by **radius tier** first — small props
in a fine grid, large bodies in a coarse one — which is a contained change and the obvious
prerequisite. And it *is* required for the deep world: at r=1200 the ring's arc is ~7,000
units, so a lake that actually gates a route out there is hundreds of units across. That is
the same shape of bug as §1a — **a fixed size in a world whose scale keeps growing** — and
it will bite lakes exactly as it bit the maze fill.

#### And a lake has to be placed BEFORE the path

Today obstacles are *rejected* from the clear-path tube, so a lake placed by the ordinary
scatter would never cross your route and would never make you turn. Barriers must be placed
first and the route drawn around them (the inversion §3 and §4.0 both call for). Otherwise
a lake is scenery you walk past, which is what a pond already is.

#### Water as the region boundary

Worth considering while both are on the table: a connected **lake + river network
partitions the ring**, which is `WG-7`'s "regions with shapes" (§4.2) achieved through
*topology* rather than through a separate biome field — one system doing two jobs, with the
boundary being a thing you can see and stand on rather than a texture cross-fade.

That is also the feasibility hazard, and it is sharper here than for mountains:
connectedness is what water *is*, and a connected impassable network is precisely what
disconnects a graph. The measured mesa experiment (§4.0a) already showed an isotropic mask
starting to seal routes at ~7% coverage; water wants to be connected on purpose. So fords
and isthmuses have to be **generator-level guarantees**, not repairs after the fact — the
`Seam` contract, applied to a shape that genuinely tries to cut the world in half.

### 4.1a Rivers, and why they still dodge the art bug

A **river** should land after lakes (§4.1) but before a mountain range, for three
reasons:

1. **The art already ships, and it is a SURFACE rather than a billboard.** A river is a
   depression, a water plane and a crossing — it never asks the coarse ground grid to
   smooth an 11-unit wall, which is the exact failure that turned the mesas off. And there
   is nothing to draw: water is already rendered as **animated textured ground geometry**
   (`WorldAssets::water_mats` → `MeshMaterial3d`, swept by `animate_water`), in **three
   biome variants keyed by kind**:

   | kind | tile | reads as |
   |---|---|---|
   | `pond` | `ground/water_clear.png` | field / forest water |
   | `bog_pool` | `ground/water_bog.png` | mire water |
   | `frozen_pond` | `ground/water_ice.png` | tundra ice |

   That mapping is already biome-shaped, so a river takes its region's own water tile for
   free — a channel through the tundra is ice, through the mire it is bog. `water_mat`
   already falls back to `pond` for an unknown kind, so a new `river` kind renders on day
   one and gets its own tile only if it wants one. There is even a
   `models/nature/cliff_waterfall_rock.glb` for where a channel crosses elevation. The
   mire's entire fill is already impassable water, so "a flooded region you route around"
   is a shipped, played thing — it is simply scattered into pools instead of drawn into a
   channel.

   This is what makes rivers the cheap half of `WG-7`: the barrier is new, the *rendering*
   is not.
2. **A line barrier beats an area barrier for routing pressure.** A mountain gives you a
   detour *around* it. A river gives you *"follow it until you find a ford"* — a lateral
   decision along the barrier, which is precisely what a world with no angular structure
   has none of. It also composes with co-op: a known ford is a place people meet.
3. **It is the cheapest thing that makes a bearing matter.** A river running roughly
   radially divides the world laterally — two regions that are genuinely separate at the
   same depth. One running tangentially gates progress outward.

What it needs that does not exist: rivers cannot come from the isotropic sine field
(`height` is a sum of sines; it has no long connected channels). They want their own
construct — a seeded polyline or ridged-noise channel with a signed distance — folded into
`routable`/`walkable`. **Once it is in there, A\* handles the rest for free**, and the
fords are simply the gaps left in it. That is the same contract the existing `Seam`
already honours (a wall with a gap the route is guaranteed to pass through), generalized
from "a ring wall with one door" to "an arbitrary barrier with a pass."

Mountains then return as `CLIFF_HEIGHT` raised **and widened into ranges** rather than
isolated buttes — a range is what you walk around; a butte is what you walk past — gated
on the face rendering being solved (a cliff needs its own geometry, not a displaced ground
plane). `WG-5` is the same feature seen from the content side.

### 4.2 Regions with shapes: biome as a 2D field

Today `section_biome(seed, i, distance, prev)` returns **one biome per section**, and a
section is a radius band spanning the whole 340° arc. So a biome is a 20-180 unit stripe
wrapped around you: every angle is the same content, two players at the same radius on
different bearings see statistically identical worlds, and you cross forest → mire → field
→ forest in a couple of minutes without ever being *in* one. That also quietly defeats the
per-biome density contrast §1 exists to protect — the difference between open grassland and
a wood you cannot see across only lands if you are in one long enough to notice.

**Biome should be a pure function of world position, exactly like `height` is.** This is a
pattern the codebase already runs on: one function, mirrored between Rust and the WGSL
ground shader, seeded by a per-run offset, with `terrain.rs`'s module doc as the standing
instruction to keep the two in lock-step.

The client is closer to this than it looks. `ground_biome.wgsl` **already picks the biome
per fragment from the fragment's own world position** and cross-fades across boundaries —
it just resolves that position through a 32-entry `rings: array<vec4<f32>>` uniform of
`(outer_radius, biome)`. Replace the ring lookup with `biome_at(x, z)` and the ring table
goes away. Voronoi is the natural generator: it gives a **guaranteed minimum region size**
(the "regions with real extent" requirement) and the cross-fade for free via
distance-to-second-nearest.

Difficulty does not move. `[biome_gate]` stays a **radial gate** — a harsh theme may not be
drawn inside its gate radius, wherever the 2D field would otherwise have put it — so the
on-ramp is unaffected and CANON §B (distance *is* difficulty) is untouched. What changes is
only *which* skin sits at a given bearing.

Two costs to see before committing:

- **The Shift is radius-banded, in the renderer as well as the model.** `apply_shift`
  retiles a *section span* and re-sends `world.terrain_section`; the shader comment is
  explicit that *"a region is a radius ring in the WG-4 fan and this ground is already
  painted in rings, so the doomed region draws as an annulus in the same frame as
  everything."* 2D regions mean CANON §W2's Shift granularity **and** its rendering both
  change. The Shift's replay log (`§W5`) stores spans, so the persistence format moves too.
- **Props and creatures are placed per section from that section's biome.** A section
  becomes a patchwork rather than one theme, so `creatures_for_biome` /
  `resources_for_biome` / `fill_kind_for_biome` get asked per *placement* rather than per
  section. That is mechanical, but it is every placement site.

### 4.2a The ocean: give the fan a coastline

Measured first, seed 424242 streamed to d600. The arc is **340°**, so the wedge behind
Last City spans **20°** — and:

- content outside the fan: **0 obstacles, 0 creatures, 0 nodes** (of 11,704 / 3,745 / 36)
- the movement clamp is a **SQUARE** (`x_min`/`x_max`/`lateral` = ±rmax), **not the arc**

So a player can walk off the side of the world into entirely empty walkable ground and keep
going until an invisible rectangle stops them. That is the same class of problem as a token
nothing renders: **a boundary the player cannot see does not exist to them.**

**The version worth building is the fan's COASTLINE, not a backdrop behind town.** The fan
has two edges and they exist at *every* radius; "behind the city" only anchors you near
home, and the orientation problem is worst deep. One body of water runs out along both
edges and closes behind the city where they meet — Last City at the head of a bay.

Why it is close to free:

- **An ocean is pure boundary.** You never need its interior, only its shore — the
  strongest possible case for the edge representation measured in §4.1 (2,110 rim colliders
  **+3%**; one filled disc **+63%**).
- The rendering already ships, animated, in three biome-keyed variants, so a tundra coast is
  `water_ice` and a mire coast is `water_bog` for nothing.
- Coastline length grows linearly with reach and streams like everything else.

It also retires a magic number: `west_return_border = -20.0` is an invisible line in an
empty field today. **Make the shore the return** — walk to the coast and the city is there.

⚠️ **Two honest limits.**

1. **It does not solve `WG-7`.** It is a *frame*, not a destination: it tells you which way
   is out, but there is still nothing to walk *toward*. Wedges and landmarks remain the work.
2. **The coastline may never be seen.** All content is inside the fan and the clear path runs
   radially, so nothing draws a player sideways; a coast at ±170° would be visited by almost
   nobody. So: **behind the city it pays immediately** (you go there every run), while
   **along the arc edges it only pays once `WG-7` gives a reason to travel laterally.** Ocean
   and angular structure are complements. Building the ocean alone means building the part
   behind the city. (Narrowing the arc with depth would make the coast unavoidable instead —
   but that is a real difficulty-curve change, not a free one.)

### 4.2b Make Last City a PENINSULA

This is the form the ocean should take, and it is strictly better than "water behind the
city" because it answers §4.2a's own objection. The problem with a coastline along the arc
edges is that **nobody would ever see it** — all content is inside the fan, the clear path
runs radially, and nothing draws a player sideways. A peninsula **puts the coast where the
player stands every single run.**

**The geometry already almost is one.** The fan is 340° centred east and the gap is 20°
centred west, so the city already sits in a *notch with world on both sides*. Fill that
wedge — and everything beyond the fan's radius — with water, and the city becomes a spit of
land reaching west into open sea, with the fan's two edges (±170°) as its coastline.

**And the neck is the hub itself**, which is already where every run begins. The Threshold
stops being a UI affordance and becomes a geographic fact: the single land route out.

**The fiction pays for the geometry.** A peninsula is *defensible* — water on three sides,
one landward approach — which is a reason for this to be the *Last* city rather than a
coincidence. It also pre-loads `BD-4`/`BD-8`: when creatures siege the city, the neck is
the front and the only axis an assault can come from. And it gives **both** spaces an edge
they currently lack — the arena ends at an invisible square with zero content outside the
fan (§4.2a), and the city scene has no bound at all.

⚠️ **The structural caveat: the city is a SEPARATE SCENE, not continuous world.**
`Screen::City` has its own `city_scene` / `city_move` / `city_camera`, and `EnterMaze` is a
screen transition. So the peninsula is authored **twice** — once as the city's ground and
coast, once as the arena's western water — and the two must agree or the illusion breaks
the instant you dive. Cheap (a transition, not a seam), but exactly the kind of duplicated
fact that drifts silently: the same shape as the `terrain.rs` ↔ WGSL mirror the repo
already hand-maintains. **Put the coastline behind one shared constant** rather than two
hand-placed shorelines.

Two calls to make:

- **Is the neck a walk you make every run?** Today `Enter` dives from anywhere in town.
  Proposed: keep that as the fast path and let the neck be the *diegetic* route — exactly
  how the Threshold district already works (`E` there, or `Enter` anywhere). A mandatory
  walk taxes every single run.
- **A city on a peninsula is a PORT.** Boats become an obvious affordance and someone will
  ask. Out of scope — but say no on purpose rather than by omission, because the geometry
  invites it.

### 4.3 The constraint I would insist on: a detour budget

If barriers lengthen the walk to depth *d* without bound, "walk around the mountain" stops
being a decision and becomes a tax — and this game already has a walking-time problem
(the roadmap's own figure is a continuous expedition reaching only ~d1150 in four hours).

**Hold route length to depth `d` under a bounded multiple of `d`, by test, across seeds**,
and treat that ratio as the acceptance criterion for the whole feature. It is also the
knob that decides how *aggressive* barriers may be: every range and every river is placed
against the same budget, so the world can be made to feel obstructed without the walk out
growing. A barrier that cannot be afforded is not placed.

The companion criterion is §1's: **props in view along the route must not collapse with
depth.** Between them they pin the two halves of "it feels like a place" — the ground has
things on it, and the ground makes you turn.

### 4.4 What is deliberately still open

- **Should a river be crossable at a cost rather than only at a ford?** (Swim and lose the
  channel? Wade slowly and be ambushed?) A binary barrier is simpler and reads better;
  a cost turns it into a risk decision. No position yet.
- **Do regions get *names*, and does anything remember them?** CANON §W says a world is a
  place and `BD-3` anchors already make a region worth holding, but neither is legible
  while every region is interchangeable. Named, shaped regions are what would make "the
  mire north-east of the hub" a true sentence — and a persistent one.
- **Whether the whole-section procedural dungeon survives** (§1b). A ring-scale "room" is
  a category error; retire `dungeon_every`, or make a procedural dungeon a *local*
  enclosure placed on the route the way a DG-3 entrance is.

## 5. `WG-8` — overworld dungeons: maze regions assembled from authored parts

A maze does not have to be a global property of a biome. §4.1's pushback was that making
dense fill *impassable* turns the whole overworld into corridors — but that objection only
holds if density is global. **Bounded regions with derived openings** answer it: a biome
can feel like a maze *in some spots*, and be open everywhere else.

The shape: a region of the overworld assembled from **authored design parts**, laid out so
it can only be entered and left through a **few openings derived when it is put together**
— Diablo's trick, where preset pieces give a place authored legibility while derived
connections keep it from being the same twice. Crucially it is **not a sublayer**: you do
not descend into it, and the rest of the world does not unload.

### 5.1 The substrate already exists, and it already makes the right guarantee

`server/crates/meld-dungeon/` is a glyph-grid + legend format with a parser
(`parse.rs`), semantic checks (`validate.rs`), and a **build-time-compiled content pool**
(`meld-dungeon-content`, with a `build.rs`). Its headline check is a bounded fixpoint that
grows two monotone sets — `active` (emitters reached, hence operable) and `open` (barriers
whose condition now holds) — re-flooding reachability until nothing changes, proving that
*some* order of operations a party can perform opens a route from the entrance to an exit.

Its stated reason is exactly this design's reason:

> *"a dungeon is a committed space (no Town Portal — design §4), so an unsolvable dungeon
> would be a trap with no way out, so this is a hard gate."*

"Only a couple of defined ways in and out, guaranteed to connect" is therefore **already
built and already enforced**. `WG-8` reuses it rather than inventing it.

### 5.2 What changes for an overworld piece

1. **Openings replace stairs.** The structural glyphs today are `#` / `.` / space / `>` /
   `<`, and the validator requires exactly one `Down` on floor *n* and one `Up` on floor
   *n+1*. An overworld region has no floor stack — it has **boundary openings**, and per the
   design they are *derived at assembly* rather than authored into each part.
2. **The guarantee gets STRONGER.** A descent dungeon is **directed** (enter → clear →
   exit). An overworld maze region is **permeable**: a player may enter from the north
   meaning to leave west, or cut through it as a shortcut. So the fixpoint's query becomes
   **all-pairs reachability between openings**, not entrance→exit. Same machinery, harder
   question — and it must be a hard gate for the same reason, because a region that can be
   entered and not left is a trap in the middle of the overworld.
3. ⚠️ **Prefabs are rectangular and this world bends.** A glyph grid is an array in some
   frame; corridor `y` is an **ANGLE**. A piece laid out naively at r=1200 is smeared into
   an arc. This is the same mistake the repo has now made three times — the tree spacing
   that asked for 392 and placed 90, the creature grouping, and (§1b) the dungeon divider
   walls that are currently a line of rocks ~250 world units apart. **The assembler must lay
   parts out in WORLD space, or bend per cell.** Write it into the design before the code.

### 5.3 Two properties that are easy to miss

- **Co-op works inside it, with no instance transition.** Because a maze region is just
  world, it is in the snapshot, the interest cull and `check_touch` — so `run.join_battle`,
  `run.watch_battle`, clash markers and everything else already apply. A `DG-3` descent
  dungeon is its own space: a teammate standing outside cannot see or join you. This is a
  gameplay advantage, not only a loading one.
- **"In some spots" is load-bearing for PERFORMANCE, not just for feel.** A maze region is
  far denser in blocking props than open ground, and §2 measured that prop density is what
  makes `blocks()` expensive. Bounded regions are affordable; carpeting the world in them
  would not be. The design instinct and the perf envelope agree, which is a good sign.

### 5.4 It replaces something broken rather than adding a system

`dungeon_every = 4` makes every **fourth ring** of the world a procedural "dungeon" whose
divider walls are a rounding error across 340° of arc — §1b measured those rings at 30x
emptier than ordinary ones, and §1b concluded that a ring-scale "room" is a category error
whose fix is *retire it, or make it a LOCAL enclosure placed on the route*. `WG-8` is that
second option, done properly. So this is not a new system beside the old one: **retire the
ring-dungeon and put prefab maze regions in its place.**

### 5.5 Open

- **On the clear path, or off it?** On-path makes a region a **gate**: mandatory, so it must
  be tuned for every party that passes. Off-path makes it **optional** content that can be
  harder and pay better, consistent with how side terraces and treasure already work.
  Default proposed: *off-path and optional*, with the option of a mandatory one at a biome
  seam later, where a pass already exists and a Gatekeeper already stands.
- **How big, and does size ride depth?** The same trap as §1a and §4.1: a fixed-size region
  in a world whose scale grows becomes negligible at depth. At r=1200 the ring's arc is
  ~7,000 units.
- **Do parts carry their own encounters, or are creatures placed by the normal pass?**
  Authored encounters make a place memorable; procedural placement keeps difficulty riding
  distance, which CANON §B requires.

## 6. What shipped alongside this

- **`CR-10`** — the wander fix (§1c), with `[ai] wander_leg_seconds` /
  `wander_arrive_radius` / `wander_pause_chance` / `wander_pause_seconds`.
- **`WG-6` (the clear-tube frame fix ships; §3 still stands — see the box there)** — the
  dungeon-ring fill skip removed (§1b) and the thickness axis
  folded into the one `maze_fill_scale` (§1a), which together take the shallow ring from
  7.38 to 27.4 obstacles per 1000 u² and a quarter of the world from 0.167 to parity. The
  cap is documented with its measured cost curve but **held at 24** (§2). The density guard
  now runs past the cap's holding radius, pins the biome so it measures the compensation
  rather than the seed's biome draw, states its bar in mean prop spacing rather than as a
  ratio against a deliberately-capped baseline, and carries a vacuity check.
- **A dev-harness bug that hid all of this — and that two of us hit independently.**
  `MELD_AUTOPLAY` / `?autoplay` could never leave town: `city_input` returns early while a
  town-tour step is open (so Enter cannot both advance the tour and fire a dive), and the
  tour opens on any account that has not seen it — which is *every* account in the
  in-memory embedded build. So `client/scripts/view_biome.sh` and every `?autoplay`
  screenshot had been capturing the hub rather than the maze, which is why the density
  collapse in §1 went unseen for so long. `render_town_tour`'s fix landed on `main`
  concurrently with this work and `main`'s version is the one kept (it marks the tour
  *seen* rather than merely returning, so nothing re-opens it). What this branch adds is
  the same rule for the **"Before You Dive"** card (`first_run_popup`), which nothing had
  covered and which sat over a third of every captured frame.

  Worth a look while someone is in there: the two now use `crate::flags::autoplay_flag()`,
  while the `Autoplay` resource is `(autoplay || city_idle) && !demo`. So `?city` — a
  screenshot flag — does *not* suppress onboarding, and `?demo` does. Neither is obviously
  wrong; they are just two different definitions of "this is an instrument, not a person".
