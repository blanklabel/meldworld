# Biome hazards — what would have to exist before the bestiary is buildable

> **Status: proposal / gap analysis.** No code, no committed design. It exists because
> [`lore/biomes.md`](../lore/biomes.md) specifies 27 biomes almost entirely in terms of
> **hazards**, and the engine has nowhere to put one. This is an honest accounting of
> the distance between those two facts, and the cheapest order in which to close it.
>
> Higher docs win ([`GDD.md`](../GDD.md) → [`CANON.md`](../CANON.md)). Nothing here is
> canon until it is folded into them.

## The gap, stated plainly

A biome today is **terrain plus spawn tables**. `meld-world` gives each biome a set of
impassable obstacle kinds (`obstacles_for_biome`), a signature maze fill
(`fill_kind_for_biome`), harvest nodes (`resources_for_biome`), a ground texture, and a
creature set. Every one of those is *scenery or a fight*.

The registry's biomes are not scenery. Strip the prose from the 27 entries and nearly
every one resolves to the same sentence: **the place itself does something to you, and
it does it while you are simply standing there.** The crust gives way. The geyser blows.
The grass holds you. The chains take you. The cold kills you.

There is exactly **one** thing in the build that does that today, and it is not in the
overworld: `Game::apply_trap_hit` (`meld-server/src/game.rs`) damages a stepping player's
whole party out of battle, scales it by the floor's effective distance, and routes a full
party wipe into `dungeon_death`. It fires from an authored `ObjectKind::Trap` inside
`meld-dungeon-run`.

That function is the seed of the whole system. **The work is not inventing out-of-battle
damage — it is generalising the one instance of it we already have, and giving the
overworld a way to place and telegraph it.**

## What the 27 biomes actually ask for

Sorted by what the engine would need, not by fiction. The count is how many registry
entries lean on it.

| Primitive | What it is | Registry entries that need it (examples) |
|---|---|---|
| **1. Hazard field** | A region that ticks damage or a status while you are inside it | Sintered Caldera crust, Seized Engine arc-pools, Glass Desert Grit, Ashfell ash drifts, Caustic Sea, Chronal Bog quagmires |
| **2. Telegraphed emitter** | A periodic area event with a *readable warning window* | Caldera geysers (on the forges' seismic beat), Tension Snaps, Crimson Wakes, Iron Tundra shearing, Archipelago collisions, the Warden's Gaze |
| **3. Terrain modifier** | Speed / traction changes, sometimes conditional on gear | Slag-Fields magnetic tides (metal armour → half speed), Sinking Mire mud, Petrified Ocean slip, the Penitent Weight |
| **4. Trigger → response** | A *player action* provokes the environment | Clockwork Phantoms (you cast), Flesh-Bound Shadows (you light), Crimson Wakes (you fight), Lithic Leviathans (you walk) |
| **5. Attrition meter** | A slow resource the field drains: warmth, water, air | The entire **Pale Echoes** category — Ashfell, Bitter Tundra, Sun-Bleached Dune, Serene Field |

Two more that are *not* hazards and should not be smuggled in as ones:

- **The Aggregate Warrens** is a negotiation/faction design. It needs dialogue, standing
  and an economy of persuasion — none of which exist. It is a different pillar; it
  should not be built as "a biome."
- **Lotus-Engine** is a moral choice attached to a rest. It is meaningless without
  **FS-1 (camping)**, and it is *the* argument for building FS-1 with a cost attached.

## The four constraints any design must satisfy

These are not preferences; three of them are load-bearing invariants that already have
tests.

1. **The clear path must stay walkable.** `Arena::path` carves a guaranteed hub→portal
   route and rejection-samples obstacles out of its `path_clear_radius` tube, so
   extraction is feasible *by construction*, across seeds, under test. A hazard that can
   land on the clear path silently converts "always escapable" into "sometimes a death
   sentence." **Hazards must be sampled against the same tube as obstacles**, and the
   existing property test extended to cover them. This is the single highest-risk part
   of the whole idea.
2. **`meld-world` stays pure.** No `Instant::now`, no global RNG. A geyser "on a
   schedule" must derive its phase from the instance seed and the tick counter, never
   from wall-clock — otherwise the engine stops being replayable and unit-testable.
3. **Server-authoritative.** The client renders a hazard and its telegraph; it never
   decides that one hit. Damage lands on the game loop like `apply_trap_hit` does.
4. **No gameplay literal in code.** Every radius, tick, damage figure and warning window
   is a `[TUNABLE]` in `balance.toml` behind the `meld-balance` loader.

## The decision that actually matters (and it is not technical)

Today the overworld cannot hurt you. All damage is inside the ATB battle; walking is
safe. Hazards change the shape of the game:

- **HP between fights becomes a resource.** Right now it is only spent in battle. Once
  the field drains it, the Resonant stops being a battle healer and becomes a *travel*
  healer, and "do we push one more area" becomes a genuine question instead of a
  formality.
- **Extraction pressure gets a second axis.** Distance already scales difficulty. Attrition
  scales *time*, which is what makes the extract-or-die decision bite. This is the strongest
  argument for building it.
- **It can trivially become miserable.** A field that chips at you constantly, with no
  counterplay, is a tax rather than a decision. Every hazard needs a legible answer —
  route around it, time it, gear for it, or spend something.

**Recommendation:** hazards should be *avoidable by reading the environment*, never
ambient chip damage. The Caldera is the model to copy — a thicker vein of safe crust
exists, and the geysers are on a beat you can learn. The Glass Desert's biome-wide Grit
is the model to avoid, because it is a flat tax on time spent.

## Cheapest credible order

Each step is shippable and observable on its own.

- **H-0 — Make lava hurt.** Ashfall already places a `lava` obstacle kind; it is currently
  just an impassable rock. Give the obstacle table an optional `contact_damage`, tick it
  through the existing `apply_trap_hit` path, and lava becomes the first honest hazard.
  Near-zero new surface: no new placement, no new wire field, no new art. It proves the
  damage path end-to-end and answers "does the overworld hurting you feel good?" before
  anything expensive is built.
- **H-1 — Hazard fields as placed entities.** Generalise placement: a hazard is sampled
  like an obstacle (outside the path tube), carries a kind + radius + effect, and rides
  the snapshot the way obstacles already do — as an `avatar_state` tag
  (`hazard:<kind>:<radius>:<state>`), so **no proto change is needed**, matching the
  precedent CLAUDE.md sets for `statuses` tokens.
- **H-2 — Telegraphed emitters.** The first mechanic that makes a biome *play*
  differently rather than *look* different. Phase from `(seed, tick)`, a warning state on
  the wire, and a client tell that reads at the HD-2D camera's pulled-back pitch — which
  is a real art problem, not a footnote.
- **H-3 — Attrition meters.** Unlocks the Pale Echoes as designed biomes rather than
  quiet ones. Wants **FS-1 (camping)** and **FS-5 (day/night)** to exist first, since
  warmth and shelter are meaningless without a clock and a rest.
- **H-4 — Trigger → response.** Cheapest *interesting* one, and it needs no new placement
  at all: the hooks (harvest channel, battle start, the Explorer's lamp) already exist.
  Grafting a spawn onto "you channelled here" is mostly wiring.

## The cost nobody should be surprised by

**27 biomes is a content pipeline problem before it is a systems problem.** Each biome in
the build needs a ground tile, an obstacle prop set, harvest nodes, a creature set, and
music. The five current biomes represent most of the 150 MB asset budget. The hazard
*system* is a few weeks; the *bestiary* is a long, asset-bound campaign, and the registry
should be treated as a menu to draw from over seasons — not a backlog to burn down.

The honest read: build the system against **three** biomes that each exercise a different
primitive (Sintered Caldera for emitters, Slag-Fields for terrain modifiers, Graft Lands
for trigger→response), and let the rest arrive as art lands.

## What does not translate

The registry is written in tabletop terms. These have no analogue and should not be
faked:

- **Saving throws, DCs, proficiency, damage dice.** MELDWORLD resolves with ATB stats and
  seeded rolls. "DC 16 Strength save" means *heavy armour should be a liability here* —
  translate the intent, drop the mechanism.
- **Long rests.** Runs are continuous. Every "if the party takes a long rest" hazard is
  blocked on FS-1. The nearest live analogue is **standing still**, which the harvest
  channel already models and already interrupts — the Chitin-Kilns' Rebirth Tax is a
  channel punisher waiting for a channel.
- **Ability-score damage, aging, Exhaustion levels.** We have no ability scores to burn
  and no exhaustion track. Max-HP drain (Hearth-Plains, Graft Lands) is the one that maps
  cleanly, since run HP is already per-hero server state.
- **Casting as a trigger.** Several biomes react to *spellcasting*. There is no casting on
  the overworld — magic happens in battle. These triggers must re-target onto overworld
  verbs that exist: harvesting, lighting (the Explorer's lamp), fighting, moving fast.

## Open questions

- Does a hazard interrupt an ATB battle that starts on top of it, or is battle a safe
  bubble? (Cleanest: battle is a bubble; the hazard resumes when you are spat out.)
- Do hazards persist across a Shift, or does retiling a region reroll them? The Shift is
  the natural authoring moment for "this area is now lethal."
- Does a hazard-killed party count as a death for durability/insurance purposes? The
  dungeon trap path already answers this — it calls `dungeon_death` — so consistency says
  yes.
- Should any hazard be *harvestable*? A geyser that vents a rare reagent on its schedule
  turns a threat into a reason to go there, which is the difference between a hazard and
  an obstacle.

See also: [`lore/biomes.md`](../lore/biomes.md),
[`lore/shifting-lands.md`](../lore/shifting-lands.md),
[`behaviors/world-generation.md`](../behaviors/world-generation.md),
[`ROADMAP.md`](../ROADMAP.md) Epic FS.
