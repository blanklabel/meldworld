# Worldgen research survey (non-normative)

> **Non-normative design rationale**, preserved from the former `proposals/worldgen-wg.md`
> proposal after Epic WG shipped. This is the research spike — a survey of how other games
> solved procedural worldgen, and the reasoning behind the choices we made — kept for
> context. It is **not** a spec: the shipped, authoritative behavior lives in
> [`../behaviors/world-generation.md`](../behaviors/world-generation.md) (and
> [`../behaviors/verticality.md`](../behaviors/verticality.md)). Where this note and the
> behavior spec disagree, the behavior spec (and CANON above it) wins.

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

## Deviations from the spike (deliberate)

> **Deviation from the spike's "pin a `TUTORIAL_SEED`" advice — deliberate.** That advice
> assumes a *hand-authored* tutorial world worth reproducing byte-for-byte. Ours is
> procedural and the tutorial is a **one-time** first dive, so byte-reproducibility has no
> player-facing payoff (you never replay it). A `tutorial` flag that fixes the biome *order* +
> area-0 onboarding already delivers the gentle, known first dive — with a normal random seed,
> which is simpler and keeps the whole QA suite on the same random-world footing as before.

## Explicitly avoided as over-engineering (for now)

Full biome permutation (breaks monotonic difficulty), polar/angular chunk storage,
bounded DAG biome graphs (clash with the infinite plane), and grammar/CA dungeons before BSP.
