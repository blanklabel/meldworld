# The Shifting Lands — world canon, Shift mechanic & biome bestiary

> **Status: captured vision, partially implemented.** This is the north star for what
> the overworld *becomes*: the chaotic, ever-shifting hostile expanse outside Last
> City's stabilizing field. The build's five biomes (forest/desert/ashfall/tundra/mire)
> are **the Pale Echoes** — a deliberate category of the registry in
> [`biomes.md`](biomes.md), not stand-ins to be replaced — and the bestiary grows
> around them; the "web of trails" overworld and the Shift mechanic are the structure
> that makes the field feel like the Shifting Lands. Higher docs still win on conflict
> ([`GDD.md`](../GDD.md) → [`CANON.md`](../CANON.md)); fold rules from here into those
> as each system is built.

## Origin (why the world is like this)

The universe expanded on dark matter until it hit its limit and began to **contract**
back toward its origin. The fabric between dimensions tore: worlds collided, planes
scabbed together, microscopic organisms seeped through dimensional fissures. On Earth,
creatures out of myth and folklore poured in; every dimension thought it was under
attack, and the Merge Wars erupted. Cities fell, nature reclaimed and cross-pollinated
into new terrifying/beautiful ecologies, and the planet's very landscape kept
**shifting** as chunks of other worlds pushed through the veil.

As the planet's **core** began to collapse, the surviving races put aside the wars and,
by merging ancient tech with primal magic, **stabilized the smallest possible patch of
land** — a permanent stable space held by great machines. From that grew a single
harmonious city. This is **Meldworld**.

It is now **537 YM** (Years since Meld). **Last City** survives inside a stabilizing
field walled off to the North, South, and East by great ivory walls; to the West are
cliffs over the **Ever Shifting Ocean**. Beyond the walls lie the **Shifting Lands** —
hostile, uninhabited, and still sliding: the landscape changes with limited notice.
Four **Stabilizer** machines in the crystal Tower's courtyard, fed by **Harvesting
Pods**, are all that hold the bubble. Keeping them running demands constant sacrifice.

## The four Pillars (art direction)

Style: **Clinical Macabre Archival Tintype** — 19th-century penny-dreadful photography
meets New Weird bio-magitech. Harsh, grainy, high-contrast sepia/black-and-white with
oppressive pitch-black shadows; heavy, rusted, brutalist environments. **The only
vibrant color is unnatural** — neon magic, bioluminescence, magitech liquids. Grimy,
lived-in, terrifying realism. No clean fantasy.

1. **Infrastructure Realism** — if it could be broken, citizens already broke it. No
   fragile brass filigree/exposed cogs. Thick concrete, riveted iron, tamper-proof
   utilitarian tech; power looks like reinforced industrial vats, not crystals.
2. **Bio-Industrial Noir** — the city is dark; the *anomalies* provide the light.
   Swirling magitech-liquid streetlamps, sickly bioluminescent fungal neon.
3. **Utilitarian Grit** — no superheroes. Specialized laborers, tactical responders,
   exhausted investigators. Heavy leather/canvas, patched asymmetric rigging, scars
   that read as workplace injuries not heroic wounds.
4. **Biological Magitech** — magic isn't a spell, it's an invasive species. Fleshy,
   fungal, parasitic. Psychic power *hurts* to use (white eyes, bleeding noses,
   bruised skin). Magic items look like modified translucent deep-sea biology.

## The four Truths (design/theme)

1. **Hope is hard work.** Survival is manufactured and maintained by grueling labor,
   not granted by divine right.
2. **Nothing is free.** Every power and comfort is *paid for* — with life force, scars,
   privacy, biological runoff.
3. **Trades are never even.** The house always wins; magic takes slightly more than it
   gives. The imbalance is the engine of all conflict.
4. **Everything has a reason.** Anomalies aren't "because magic." A fungal plague, a
   feral psyker, a spawned monster — someone made a **trade** that caused it. Parties
   are supernatural detectives auditing the timeline for *why*.

## The Shift mechanic (dimensional swaps)

The Shifting Lands live up to the name: patches of the map are periodically **swapped**
for chunks of other planes, with little warning, damaging anyone caught inside.

- **Cadence:** roll **1d10** → the number of natural **1s** that must be rolled (on the
  ongoing checks) before the **next Shift** triggers. (A countdown of rare events, so a
  Shift is a real, tension-building surprise.)
- **Location:** **1d100 × 1d100** on the travel map (the swapped region's map cell).
- **Size:** **1d6** —
  | # | Size | Scale examples |
  |---|------|----------------|
  | 1 | Tiny | a porta-potty, a maple tree, a couple of gravestones |
  | 2 | Small | a large cottage, 4–5 large trees, a family graveyard |
  | 3 | Medium | a temple, a grotto, a small-town graveyard |
  | 4 | Large | ~4 city blocks, a small forest, a city graveyard |
  | 5 | Huge | a district, a medium forest, Calvary Cemetery |
  | 6 | Cataclysmic | a city, an untouched Alaskan forest — you don't want to know |
- **Damage** (Force) to anyone caught in the Shift, by size:
  Tiny **1d6** · Small **2d6** · Medium **2d10** · Large **3d10** · Huge **8d10** ·
  Cataclysmic **10d10**.

**Game translation (for our ATB/overworld build):** a Shift retiles a region of the
overworld to a *different biome* (from the bestiary) mid-run, with a brief warning
tell, dealing force damage to entities standing in the swapped cells. This is the
mechanical heart of "ditch the corridor" — the map is not a fixed route, it *rearranges*.

## Biome bestiary (the Shifting Lands)

The full registry — **27 biomes in five categories**, in the author's own words — is
[`biomes.md`](biomes.md). It is the source of record; this section used to restate a
subset of it and no longer does, so the two cannot drift apart.

What that registry adds beyond a list of places:

- **Five categories**, which are really five *kinds of threat*: Industrial & Mechanical
  Wastes, Biological & Fleshscapes, Arcane & Conceptual Anomalies, Divine & Infernal
  Remnants, and **The Pale Echoes**.
- **The Pale Echoes settle what our five current biomes are.** Forest / desert / ashfall
  / tundra / mire are not placeholders waiting to be replaced by something weirder —
  they are a deliberate category (Deep Timber, Sun-Bleached Dune, Ashfell, Bitter
  Tundra, Sinking Mire), and their whole point is contrast: after clockwork gods and
  psychic prisons, a biome that kills you with nothing but exposure and a bear reads as
  *more* frightening, not less. They stay, and the bestiary grows around them.
- **Hazards are the substance of a biome here**, not decoration. Almost every entry is
  defined by one signature environmental mechanic, and the engine currently has nowhere
  to put one — the overworld knows terrain, obstacles, and spawn tables, and nothing
  that damages or impedes a player for *standing somewhere*. What would have to exist
  first is worked through in
  [`proposals/biome-hazards.md`](../proposals/biome-hazards.md).

## How this maps to the current build (the bridge)

- **Overworld = the Shifting Lands.** The "web of trails" replaces the single carved
  corridor so the field reads as chaotic interconnected terrain, not a lane. Next,
  biomes evolve from the five placeholders toward the bestiary above (art + per-biome
  hazard mechanics), and the **Shift** retiles regions mid-run.
- **Extraction = plantable, defendable portals** (see the separate proposal): you
  construct a portal in the field and hold it while creatures try to demolish it —
  "hope is hard work," "nothing is free."
- **Every anomaly has a cause** (Truth 4) should eventually gate spawns/hazards behind
  a *trade* somewhere, not pure RNG — the long-term hook that makes runs feel authored.

See also: [`GDD.md`](../GDD.md), [`CANON.md`](../CANON.md),
[`ROADMAP.md`](../ROADMAP.md), [`proposals/`](../proposals/).
