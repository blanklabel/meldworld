# Worldgen (Epic WG) — research spike + what shipped

> Status: **WG-2 + WG-3 SHIPPED** (seeded biome randomization + first-run tutorial
> carve-out); **WG-1 (dungeons) and WG-4 (radial) DESIGNED + DEFERRED** with the
> recommended approach below. Tracked as epic **WG** in [`../ROADMAP.md`](../ROADMAP.md).
> Written against the real generator: `meld-world::Arena` / `section_biome` /
> `push_section`, `meld-server::game.rs` (`form_run`), and the `has_dived` account
> flag in `meld-db`.

## The spike: how other games solved this

Grounded survey of named games/sources, filtered to our hard constraints —
**difficulty = floor(distance)**, per-run seed, a *pure* deterministic Rust
generator (splitmix64, `section_seed(run_seed, n)`), infinite streaming plane.

### Seeded biome ordering that stays fair
- **Fixed difficulty axis + shuffled *theme* (Hades, Risk of Rain 2).** Hades keeps
  its four lands in a fixed order and only shuffles the *chambers inside*; RoR2's
  difficulty is a monotonic function of time/stage-count while the stage *pool* is a
  weighted pick. Difficulty rides the tier, never the theme.
- **Layered DAG, one node per depth (Dead Cells, Slay the Spire).** Great pacing
  control (guaranteed beats), but it's a *bounded graph* — awkward against an
  infinite streaming plane.
- **Rejected: a full seeded *permutation* of the biome set per run.** It breaks
  distance-monotonic difficulty (nothing stops a "hard" biome landing at d=0) unless
  biomes are difficulty-neutral skins.

**Winner for us:** the Hades/RoR2 model. Our biomes *are* difficulty-neutral skins —
creature stats scale from `distance` via `stat_mult` at spawn, so the biome only
picks the *theme* (creature/resource/obstacle tables). So we draw a biome per
section from `section_seed`, keep difficulty on `distance`, and forbid two identical
themes back-to-back.

### Randomized start with a fixed first-run tutorial
- **Pin the seed for run #1 only** (Cogmind on seeds): a seed needn't be random.
  Pin the first dive to a constant → a reproducible, hand-tuned tutorial world that
  reuses 100% of the real generator; every later dive seeds from entropy.

### Radial worlds anchored on one hub
- **Hub-and-spoke, difficulty = distance from hub** (RDR2/AC/cRPGs) — already our model.
- **Key insight: stream in Cartesian, read difficulty in polar.** Keep the square-grid
  section storage; compute `distance = hypot(pos − hub)` (and `angle` only if you want
  angular theme variety). *Do not* store the world in polar/angular chunks — chunk size
  varies with radius and seams get ugly (Minecraft rings features by (radius, angle) but
  still stores Cartesian).

### Dungeons as sub-spaces
- **BSP room-and-corridor** is the best first implementation: recursive split, a room
  per leaf, corridors between siblings — connectivity guaranteed by construction, trivially
  seedable/pure, and room identity (loot/boss rooms) suits an extraction game. CA /
  drunkard's-walk (organic caves, needs a connectivity repair pass) and grammar/graph
  dungeons (Dead Cells' concept graph) are later polish, not a v1.

## What shipped (WG-2 + WG-3)

**`meld-world::section_biome(run_seed, i, distance, prev, tutorial)`** — the biome
*theme* for section `i`:
- **Tutorial run** → the classic distance-ordered bands (`biome_for_distance`): the
  hand-tuned Forest→Desert→… onboarding.
- **Any other run** → a uniform per-section draw from `BIOMES`, excluding the previous
  section's biome (no adjacent repeat), off an independent salted stream so the theme is
  stable regardless of unrelated placement draws. This gives **WG-3** (order varies every
  run) and **WG-2** (the first section is randomized too → you don't always start in Forest).

**Difficulty is untouched** — `tier`/`mlevel`/`stat_mult` remain pure functions of
`distance`; the biome is a skin. Verified by tests (`no_two_adjacent_sections_share_a_biome`,
`biome_order_is_deterministic_per_seed_and_varies_across_seeds`,
`non_tutorial_start_biome_varies_and_is_not_pinned_to_forest`, `tutorial_run_always_starts_in_forest`).

**First-run gate** — a persistent `players.has_dived` flag (`meld-db`, idempotent ALTER;
loaded into the session on connect). `form_run` sets `tutorial = !initiator.has_dived`; every
diver is marked `has_dived` (via the off-loop `DbWrite::Dived` queue) so their *next* run is a
randomized world. Both Postgres and the in-memory backend implement it.

> **Deviation from the spike's "pin a `TUTORIAL_SEED`" advice — deliberate.** That advice
> assumes a *hand-authored* tutorial world worth reproducing byte-for-byte. Ours is
> procedural and the tutorial is a **one-time** first dive, so byte-reproducibility has no
> player-facing payoff (you never replay it). A `tutorial` flag that fixes the biome *order* +
> area-0 onboarding already delivers the gentle, known first dive — with a normal random seed,
> which is simpler and keeps the whole QA suite on the same random-world footing as before.
> (Aside: `two_parties_fight_separate_battles_at_once` is a pre-existing flaky concurrent-ATB
> test — it fails on clean `main` too — not related to this work.)

### Known cosmetic follow-up
Biome-boundary **seams** (chokepoint walls) still fire at the fixed distances
(100/300/500/1000/3000) for pacing and label themselves from the *fixed* bands, so on a
randomized run a "Forest→Desert pass" label may sit inside a section that's actually another
biome. The wall is functionally correct (gap always on the clear path); only the label is
cosmetic. Fix later by labelling the seam from the actual adjacent-section biomes.

## Deferred, with the recommended approach

- **WG-1 — Dungeons.** Implement **BSP room-and-corridor**, seeded
  `section_seed(run_seed, dungeon_id)`, place start+exit leaves first (connectivity by
  construction), difficulty = the entrance's overworld distance via existing
  `mlevel`/`stat_mult`. Entrance is a portal-like entity; a dungeon is an off-corridor
  sub-section. Ship BSP before any CA/grammar variety.
- **WG-4 — Radial 350° + city to the west.** Keep Cartesian section storage; add a
  **radial difficulty read** (`distance = hypot(pos − hub)`) instead of pure-x when you want
  the arc. The wall-as-safe-return is an impassable hub-edge boundary the player can always
  re-cross into Last City, mirroring how the clear-path already guarantees a feasible route.
  **Do not** re-architect into polar chunks.

## Explicitly avoided as over-engineering (for now)
Full biome permutation (breaks monotonic difficulty), polar/angular chunk storage,
bounded DAG biome graphs (clash with the infinite plane), and grammar/CA dungeons before BSP.
