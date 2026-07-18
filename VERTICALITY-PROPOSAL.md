# Climbable terrain (verticality) — design + server plan

> Status: **proposal**, not yet CANON. Grounds a multi-PR feature to replace the
> flat overworld plane with climbable ledges/plateaus. Written against the real
> code: `meld-world::Arena` / `apply_move` / `Arena::path`, `meld-proto::Position`
> / `SnapshotEntity`, and the HD-2D client ground plane.

## The problem

The overworld is a flat plane. `Position { x, y }` has no height, movement is
2-D circle-collision + slide, and the client renders one flat ground mesh. It
reads as an open field with no relief to explore or climb.

## Design decision: discrete elevation levels, not a continuous heightmap

Model verticality as a **small number of integer elevation levels** (plateaus /
terraces) joined by **climb edges**, rather than a smooth heightmap. This is the
HD-2D convention (Octopath / Triangle Strategy) and it keeps collision cheap and
the "path is always feasible" guarantee tractable.

- An area is partitioned into **terraces**, each at an integer `level` (0 = base).
- Terraces are separated by **cliffs** (impassable walls) *except* at **climb
  edges** — ledges / ramps / ladders where you can move between adjacent levels.
- Elevation gates **traversal** (you must find the climb point) and enables
  hidden loot, shortcuts, vantage, and funnelled encounters — **without touching
  the difficulty curve** (difficulty stays `sqrt(x²+y²)`; see the invariant below).

### Hard invariants
- **Difficulty is unchanged.** `distance_floor` stays x,y-only. Elevation never
  feeds `tier`/`mlevel`/`stat_mult` (CANON §G). No balance impact.
- **Extraction is always feasible.** The guaranteed clear path (`Arena::path`)
  must remain reachable — now routed *through* climb edges. This is the riskiest
  part and gets the same rejection-sampling + unit-test-across-seeds treatment the
  current path already has.
- **Server-authoritative** (CANON §S, D11). The client renders relief + sends the
  same 2-D intents; the server owns all elevation/collision resolution.

## Server model (`meld-world`)

Add a coarse **level field** + climb data to `Arena` (seeded, deterministic):

```
struct Terrain {
    cell: f64,                 // grid resolution (~2 tiles)
    level: Vec<u8>,            // level[gx*h + gy] per cell (0..=max_level)
    climb: HashSet<(u32,u32)>, // cell-boundary edges flagged climbable
}
```

- **Generation** (`Arena::generate`, behind a `[worldgen]` flag): grow a few
  raised terraces per area with splitmix from the instance seed; carve at least
  one climb edge onto each raised terrace; **carve the clear path first, then only
  raise terraces that leave a climbable route to the portal** (mirrors how
  obstacles are rejection-sampled out of the path tube today).
- **`Avatar` + spawns** gain `level: u8` (derived from their cell at spawn).
- **`apply_move`** gains elevation rules on top of the current circle-collision:
  - same level → move + slide as today;
  - crossing a level boundary → allowed only across a **climb edge** (then update
    `avatar.level`); a non-climb boundary blocks + slides like an obstacle wall.
- `check_touch` / harvest / join-radius compare **level too** (you don't fight a
  monster one terrace up).

## Wire (`meld-proto`) — additive, backward-compatible

- Keep `Position` 2-D. Add `level: Option<u8>` to `SnapshotEntity` (+ the avatar);
  old clients ignore it (defaults to 0).
- Send the compact `Terrain` (grid + climb edges) **once** in the run-start
  payload, alongside the existing `path` — the client needs it to build relief.
- **Movement intents are unchanged** (`{dx, dy}`). Walking into a climb edge
  auto-climbs; walking into a cliff slides. No new input, no protocol churn on the
  hot path.

## Client (`meld-client`) rendering

- **Stepped 3-D ground**: extrude the level grid into one mesh — a quad per cell
  raised to `level * step_height`, plus vertical **cliff-face** quads at level
  changes (biome-tinted ground on top, a cliff texture on the walls). One mesh per
  instance, rebuilt on run start → cheap.
- Place every entity/avatar at `y += level * step_height` (on top of its current
  grounding offset).
- Draw **climb-edge affordances** (ladder / vine / ramp sprite) so the route up is
  legible — same spirit as the glowing path trail.
- Feed the player's world-y (incl. level) into `hd2d_follow`'s target so the
  camera rises/falls with the terrain. Optional cosmetic climb tween.

## Rollout (server-first, one PR per milestone)

1. **M1 — server terrain + gen (invisible):** `Terrain` gen in `Arena::generate`
   (flagged), level-aware `apply_move` + level-aware `Arena::path`, `level` on
   avatars/spawns, additive wire fields. **Unit-test the path guarantee across
   seeds** (the existing `Arena::path` tests are the template). No visible change.
2. **M2 — client relief:** build the stepped ground + cliff mesh from the terrain
   payload, place entities per level, draw climb affordances, feed level to the
   camera. *Now it looks 3-D and you can climb.*
3. **M3 — polish + payoff:** climb animation, biome-specific cliff art, placement
   rules for hidden loot / shortcuts / vantage, tune `step_height` / terrace
   density / climb-edge density as `[TUNABLE]`s in `balance.toml`.

## Spec updates this needs
- New `behaviors/verticality.md` (observable behavior) + a **CANON §/D-number** for
  the elevation + climb rules (so `apply_move`'s new branches cite a source, as the
  code convention requires).
- `[TUNABLE]`s in `balance.toml`: `terraces_per_area`, `max_level`, `step_height`,
  `climb_edge_density`, `terrace_min_size`.
- Note in `interfaces/realtime-protocol.md` for the additive `level` + terrain
  payload.

## Why this shape
- Server-first proves the **path-feasibility guarantee** before any art spend.
- Discrete levels keep collision and the guarantee simple (grid, not heightmap).
- Additive wire + unchanged intents = no break for older clients / the QA bots.
- Difficulty stays distance-based, so no re-tuning of the whole balance curve.
