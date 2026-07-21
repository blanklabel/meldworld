# Verticality (terraces + ladders/ropes/slopes) — design + server plan

> Status: **IMPLEMENTED** as a spike (M0–M3), not yet folded into CANON. The
> per-section streaming, terraces + connectors, elevation-aware movement, wire
> additions, and client relief all ship and are unit- + integration-tested; see
> "Implementation status" below. Grounds the feature that replaces the flat
> overworld plane with stepped terraces joined by ladders/ropes/slopes. Written
> against the real code: `meld-world::Arena` / `apply_move` / `Arena::path`,
> `meld-proto::Position` / `SnapshotEntity`, and the HD-2D client ground plane.

> ## Implementation status
> - **M0 — per-section streaming.** `section_seed(run_seed, n)` + `Arena::push_section`
>   / `Arena::ensure_frontier`: each section is generated from its own seed and
>   streamed on demand past the initial chain (endless, reproducible). Server
>   streams new sections' terrain + trail each tick.
> - **M1 — terraces + connectors.** `Terrain` (elevation grid) + `Connector`
>   (slope/ladder/rope) per section, kept out of the clear-path tube; elevation-aware
>   `apply_move` / `check_touch` / `harvest` / `at_portal` (cliffs are walls, connectors
>   are the only way up). `SnapshotEntity.level` + `world.terrain_section` on the wire.
> - **M2 — client relief.** Stepped ground+cliff mesh per section, connector props,
>   entities raised to their terrace, camera follows elevation.
> - **M3 — polish.** Bold emissive connector props; balance `[TUNABLE]`s.
> - **Deferred:** a CANON §/D-number + `behaviors/verticality.md`; true 2-D radial
>   streaming (this is 1-D east streaming); multi-level connector chaining UX.

## The problem

The overworld is a flat plane. `Position { x, y }` has no height, movement is
2-D circle-collision + slide, and the client renders one flat ground mesh. It
reads as an open field with no relief to explore or climb.

## Design decision: discrete levels joined by placed connectors (no free climbing)

Model verticality as a **small number of integer elevation levels** (plateaus /
terraces) joined by **explicit connectors** — **ladders, ropes, and slopes** —
rather than a smooth heightmap *or* free-form cliff-climbing. This is the HD-2D
convention (Octopath / Triangle Strategy), keeps collision cheap, and reads
clearly: you *see* the ladder and use it.

- An area is partitioned into **terraces**, each at an integer `level` (0 = base).
- Terraces are separated by **cliffs — always impassable walls.** There is **no
  "true climbable" surface**; you can only change level at a **connector** placed
  on the boundary.
- **Connectors** are discrete entities (like obstacles/portals today), one of:
  - **Slope / ramp** — a walkable incline; you just walk up/down and your height
    interpolates. Continuous, no special state.
  - **Ladder** — vertical; step onto the base and you move up its axis to the top
    level (and down from the top). Movement is constrained to the ladder while on it.
  - **Rope** — same as a ladder, flavoured for descent (e.g. dropping down a cliff).
- Elevation gates **traversal** (you must find the connector) and enables hidden
  loot, shortcuts, vantage, and funnelled encounters — **without touching the
  difficulty curve** (difficulty stays `sqrt(x²+y²)`; see the invariant below).

## Per-section seeds & streamed generation (the roguelite core)

The world is a **stream of sections, each generated from its *own* seed** derived
from the run seed. As you march outward and cross into a new section, it's as if
you dropped into a fresh seed — new layout, terraces, cliffs, climb routes,
creatures — yet the whole run is reproducible, and re-hitting a section seed
reproduces that section exactly. This is the extract-or-die fantasy: it's always
new as you go deeper, forever, unless the same seed comes up again.

```
run_seed: u64                                  // one per MazeInstance
section_seed(n) = splitmix64(run_seed ^ (n.wrapping_mul(0x9E37_79B9_7F4A_7C15)))
Section::generate(balance, section_seed(n), n) // terrain + monsters + resources
                                               //  + obstacles + local path, all
                                               //  from THIS section's seed
```

This builds on today's model — `Arena::generate(seed)` already uses `Rng(seed)`
splitmix64 ("Same seed ⇒ same world, always") — but **shifts from "generate the
whole area-chain up front" to "generate section N on demand from `section_seed(n)`
as the player approaches."** That gives:

- **Endless outward progression** (the long-deferred "chunk streaming"):
  sections are made just-in-time, so a run isn't a fixed length —
  difficulty keeps scaling with distance as far as you push.
- **Perfect reproducibility**: `run_seed` → a deterministic sequence of
  `section_seed(n)` → the same world every time; and any `section_seed` alone
  reproduces that one section (shared seeds, replays, QA, debugging).
- **Clean isolation**: because each section has its own seed, terrain generation
  never perturbs another section's monster/obstacle draws — the substream problem
  solves itself (each section *is* a substream).

### Section seams (the one hard part)

Sections must **stitch together continuously** so the world reads as one place and
extraction stays feasible:

- **Path handshake:** the clear path exits section N at a known
  `(y, level)` on the shared boundary and section N+1 *starts* its path from that
  same point — so the guaranteed route is continuous across the seam.
- **Terrain handshake:** the elevation level at the boundary column must match on
  both sides (generate the seam edge deterministically from
  `section_seed(n)`+`section_seed(n+1)`, or let N+1 read N's boundary level).
- **Streaming lifecycle:** keep the current + next section (and maybe the previous)
  resident; discard sections far behind the player. Server owns which sections
  exist; the run-start / section-crossing messages carry each section's terrain +
  entities to the client.

Distance/difficulty is still `sqrt(x²+y²)` across the whole stream — section index
is just how the world is *chunked*, not a difficulty axis.

## Hard invariants
- **Difficulty is unchanged.** `distance_floor` stays x,y-only. Elevation never
  feeds `tier`/`mlevel`/`stat_mult` (CANON §G). No balance impact.
- **Same seed ⇒ same terrain.** Fully derived from the instance seed; nothing about
  the terrain is stored or client-authored — the client rebuilds identical relief
  from the seeded terrain payload.
- **Extraction is always feasible — even though the path now CLIMBS.** As of the
  climbing-maze work, a procedural section's clear path may rise onto a plateau over
  the *interior* of its segment (`path_climb_chance`) and drop back down, so the
  critical route itself has verticality. Feasibility is preserved by construction:
  the plateau covers only 30–70% of the segment (both section waypoints stay on
  level 0, so seams/portal/streaming are unaffected), the plateau's y-extent covers
  the whole path tube (no cliff cuts the route), and a **Slope ramp** (≥
  `path_clear_radius` reach) sits on the path at each level boundary. A walker that
  follows the waypoints climbs the ramps and reaches the portal grounded — asserted
  across seeds by `the_clear_path_climbs_a_plateau_and_still_reaches_the_portal`
  (and the level-0-endpoints invariant still holds in
  `no_obstacle_or_terrace_intrudes_on_the_clear_path`). Side terraces remain optional
  off-path detours (grind + treasure). Connector type is now weighted toward slopes.
- **Server-authoritative** (CANON §S, D11). The client renders relief + sends the
  same 2-D intents; the server owns all elevation/collision resolution.

## Server model (`meld-world`)

Add a coarse **level field** + connector list to `Arena` (seeded, deterministic):

```
struct Terrain {
    cell: f64,                  // grid resolution (~2 tiles)
    level: Vec<u8>,             // level[gx*h + gy] per cell (0..=max_level)
    connectors: Vec<Connector>, // ladders/ropes/slopes joining adjacent levels
}

enum ConnectorKind { Slope, Ladder, Rope }

struct Connector {
    kind: ConnectorKind,
    position: Position, // 2-D footprint (like an Obstacle)
    lo: u8, hi: u8,     // the two levels it joins
    // Slope carries a footprint span; ladder/rope are ~point footprints.
}
```

- **Generation** (per section, from `section_seed(n)`, behind a `[worldgen]`
  flag): grow a few raised terraces; **place at least one connector** onto each so
  every terrace is reachable; **lay the clear path first, then only raise terraces
  whose connector keeps a route to the section exit** (mirrors how obstacles are
  rejection-sampled out of the path tube today). The section's entry `(y, level)`
  is fixed by the previous section's exit (the seam handshake), so the route is
  continuous end-to-end.
- **`Avatar` + spawns** gain `level: u8` (derived from their cell at spawn).
- **`apply_move`** gains elevation rules on top of the current circle-collision:
  - same level → move + slide as today;
  - a level boundary is a **cliff = solid wall** → block + slide, always, *unless*
    the avatar is on a **connector** joining those two levels. On a connector,
    movement along its axis changes `avatar.level` (slope: interpolate as you walk
    the ramp; ladder/rope: travel up/down the connector to the far level).
  - No free-form climbing exists — the *only* way `avatar.level` changes is being
    on a connector.
- `check_touch` / harvest / join-radius compare **level too** (you don't fight a
  monster one terrace up).

## Wire (`meld-proto`) — additive, backward-compatible

- Keep `Position` 2-D. Add `level: Option<u8>` to `SnapshotEntity` (+ the avatar);
  old clients ignore it (defaults to 0).
- Send the compact `Terrain` (grid + connector list) **once** in the run-start
  payload, alongside the existing `path` — the client needs it to build relief and
  draw the ladders/ropes/slopes.
- **Movement intents are unchanged** (`{dx, dy}`). Walking onto a slope walks you
  up it; walking onto a ladder/rope base mounts and climbs it; walking into a bare
  cliff slides. No new input, no protocol churn on the hot path.

## Client (`meld-client`) rendering

- **Stepped 3-D ground**: extrude the level grid into one mesh — a quad per cell
  raised to `level * step_height`, plus vertical **cliff-face** quads at level
  changes (biome-tinted ground on top, a cliff texture on the walls). One mesh per
  instance, rebuilt on run start → cheap.
- Place every entity/avatar at `y += level * step_height` (on top of its current
  grounding offset).
- Render each **connector** as its own prop: a **ladder** / **rope** billboard on
  the cliff face, or a **slope** as an actual ramp wedge cut into the stepped mesh
  — so the route up is legible (same spirit as the glowing path trail) and the
  cliff otherwise reads as an unbroken wall.
- Feed the player's world-y (incl. level) into `hd2d_follow`'s target so the
  camera rises/falls with the terrain. Optional cosmetic climb tween while mounted.

## Rollout (server-first, one PR per milestone)

0. **M0 — per-section seeded streaming (foundation):** refactor world gen from one
   up-front `Arena::generate(seed)` into `Section::generate(section_seed(n), n)`
   streamed on demand, with the **path seam handshake** (section N+1 starts where N
   ended) and a resident-section window. This is the deferred "chunk streaming" and
   the base everything else sits on. **Unit-test path continuity across seams +
   reproducibility (same run_seed → same sections).** No visible change beyond
   "runs no longer have a fixed length."
1. **M1 — terrain per section (invisible):** `Terrain` gen inside
   `Section::generate` (flagged), level-aware `apply_move` + level-aware section
   path, `level` on avatars/spawns, additive wire (`level` + terrain payload).
   **Unit-test the connector-routed path guarantee across seeds + across seams.**
2. **M2 — client relief:** build the stepped ground + cliff mesh from each section's
   terrain payload, place entities per level, draw the ladder/rope/slope props, feed
   level to the camera, and stitch adjacent sections' meshes at the seam. *Now it
   looks 3-D and you can go up.*
3. **M3 — polish + payoff:** mount/climb animation, biome-specific cliff + connector
   art, placement rules for hidden loot / shortcuts / vantage, tune `step_height` /
   terrace density / connector density as `[TUNABLE]`s in `balance.toml`.

> M0 (streaming) can ship on its own before any verticality — it's independently
> valuable (endless dives) and de-risks the seam logic before terrain piles on.

## Spec updates this needs
- New `behaviors/verticality.md` (observable behavior) + a **CANON §/D-number** for
  the elevation + connector rules (so `apply_move`'s new branches cite a source, as
  the code convention requires).
- `[TUNABLE]`s in `balance.toml`: `terraces_per_area`, `max_level`, `step_height`,
  `connector_density`, `terrace_min_size`.
- Note in `interfaces/realtime-protocol.md` for the additive `level` + terrain
  payload.

## Why this shape
- Server-first proves the **path-feasibility guarantee** before any art spend.
- Discrete levels keep collision and the guarantee simple (grid, not heightmap).
- Additive wire + unchanged intents = no break for older clients / the QA bots.
- Difficulty stays distance-based, so no re-tuning of the whole balance curve.
