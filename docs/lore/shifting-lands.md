# The Shifting Lands — world canon, Shift mechanic & biome bestiary

> **Status: captured vision, partially implemented.** This is the north star for what
> the overworld *becomes*: the chaotic, ever-shifting hostile expanse outside Last
> City's stabilizing field. The current build's five placeholder biomes
> (forest/desert/ashfall/tundra/mire) are stand-ins for the bestiary below; the
> "web of trails" overworld and the Shift mechanic are the structure that makes the
> field feel like the Shifting Lands. Higher docs still win on conflict
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

Each entry is a *planar fragment* that shifts in. **Look** = art brief, **Hazard** =
its signature environmental mechanic, plus damage types where given. These supersede
the placeholder biomes as they're built.

- **Ribscape** — the continent-sized ribcage of a dead planar entity. *Look:* arching
  bone-white calcium pillars, porous marrow ground that crunches wetly. *Hazard:*
  Marrow-gas vents (hallucinogenic/toxic) + Shatter-like resonance through the ribs
  (deafen / shatter brittle gear).
- **Slag-Fields** — cooled runoff from the world's planar forges. *Look:* an ocean of
  jagged metallic slag, rivers of superheated heavy metal, rusted iron-glass plains.
  *Hazard:* Magnetic Tides — heavy metal armor halves movement, metal projectiles drop.
- **The Glass Desert (Scoured Wastes)** — razor silica around the ancient forges.
  *Look:* blinding jagged crystalline dunes, prismatic glare, shrieking glass wind.
  *Hazard:* "The Grit" (abrasive wind, internal slashing without a filter) + Flash-Zones
  (dunes align into roving heat beams that ignite packs / melt plating).
- **Lodestone Peaks (Magnetic Mountains)** — shattered planar stabilizer, floating
  iron islands. *Look:* black iron archipelagos shuddering aloft, undersides trailing
  broken chains, metallic iron-fog in geometric patterns. *Hazard:* Polarity Inversions
  (fall *upward* into a peak) + conductive Iron Fog (lightning arcs unpredictably;
  filings whipped into flesh-shredding tornados). Verticality-heavy.
- **Graft Lands** — necrotic biomatter scabbed over a planar collision. *Look:*
  claustrophobic swamp of light-eating black sludge, banyan-like trees of petrified
  purple muscle and pulsing veins, shed-skin foliage, permanent suffocating twilight.
  *Hazard:* The Hungry Dark (torches die in minutes, light strains the caster) +
  **Flesh-Bound Shadows** — cast a strong light and the trees peel your *shadow* off
  the ground and graft it into a flayed 3D mimic that siphons your **max HP** into the
  roots. Resting → roots try to graft into your veins.
- **Razor Forest (Arterial Thicket)** — the exposed frozen circulatory system of a
  mechanical god (a red-iron re-skin of a factory hazard). *Look:* from afar a crimson
  autumn forest; up close, braided hematite/oxidized-steel trunks, branches of
  razor-sharp fractal blood-red iron, a foot-deep rust litter like dried scabs.
  *Hazard:* Acoustic Laceration (resonating fractals fray leather / slice skin, rust
  dust infects) + **Crimson Wakes** (moving/casting loud triggers an Arterial Fall — a
  persistent rain of red-hot iron razors leaving scars healing magic can't cleanly close).
- **Sintered Caldera (Prismatic Vents)** — the world's exhaust system. *Look:* blinding
  white porous sinter plains, tiered pools of boiling clear liquid ringed by neon
  bacterial mats, iridescent steam geysers raining scalding ash. *Hazard:* the Fragile
  Crust (thin shelf over boiling alchemical acid — misstep or AoE = plunge) + Geyseric
  Venting on the forge's seismic schedule (maximized-Fireball eruptions that fling you up).
- **The Seized Engine (Brass Corpse)** — the surface of a dead clockwork plane, its
  curvature the horizon. *Look:* walking on a dead mechanical god — city-sized rusted
  gears, snapped shafts, frozen mainspring tornados, circular basins of black hydraulic
  fluid, dead silence, smell of grease and shaved brass. *Damage:* Bludgeoning &
  Lightning. *Hazard:* Tension Snaps (trapped kinetic energy releases upward shockwaves)
  + Arc-Bleed (black battery-fluid pools discharge into any conductive metal) +
  Clockwork Phantoms (repair-drones swarm anyone who *casts*, reading magic as an anomaly).
- **Seraphic Oubliette (Shattered Panopticon)** — a broken divine supermax prison still
  trying to enforce containment. *Look:* inverted crater walled with millions of
  petrified weeping eyes, a shattered gyroscope of golden rings holding raining
  isolation cubes, shadowless unnatural light, autonomous iron chains. *Damage:*
  Psychic & Radiant. *Hazard:* the Warden's Gaze (interrogation beams rip out your
  guilt, then burn it) + Failsafe Tethers (chains grapple and winch you into a cube
  hundreds of feet up) + the Penitent Weight (recent violence makes your gear
  crushingly heavy — localized guilt-gravity).
- **Hearth-Plains (Velvet Steppe)** — weaponized paradise around the buried heart of
  Arch-Devil **Ometus**. *Look:* endless rose-gold grass swaying without wind, perpetual
  golden-hour, exact body-temperature air, nostalgic smells — but the grass is
  capillaries pumping warm red fluid and the ground pulses like a sleeping chest.
  *Damage:* Psychic & Radiant + **max-HP drain**. *Hazard:* Sympathetic Resonance (harm
  the environment and it reflects the pain back as unavoidable psychic guilt) + the
  Lover's Grasp (rest and it lovingly restrains you — a **Charisma** save to reject
  safety, not Strength) + the Euphoric Siphon (cures all ailments while siphoning max
  HP; hit zero here and you *dissolve* into the landscape, you don't die).
- **Chitin-Kilns (Nestiphian Cradle)** — a biological recycling plant; rebirth as a
  violent industrial process. *Look:* valley of porous-bone hives, open fermentation
  vats of glowing amber that dissolve corpses to genetic slurry, spider-drones spinning
  pulsating chrysalises, smell of honey/ozone/warm calcium. *Hazard:* the Rebirth Tax
  (rest unsecured → drones spin a chrysalis over you, healing you while mutating a limb
  to chitin) + the Newly-Hatched (enemies are freshly-spun confused chimeras of whatever
  was last thrown in the vats).
- **The Aggregate Warrens** — a subterranean society that mathematically eradicated the
  *individual*. *Look:* perfectly smooth precise tunnels, grid-planted bioluminescent
  fungi, a pale mutated race in flawless silent synchronicity; no art, property, or
  poverty. *Hazard:* no concept of theft — idle gear is "optimized" (melted/redistributed)
  + the Empathy Void — only utilitarian logic persuades; "inefficient" parties are
  scheduled for dismantling into fungal-farm biomass. (No malice — pure efficiency.)
- **Lotus-Engine (Apathy Resort)** — a trickster paradise whose trick is that the rest
  isn't free. *Look:* a genuine sun-drenched restorative oasis, perfect temperature,
  soothing springs, no threats. *Sacrifice:* at its center a bound entity (or a prior
  legendary party) is drained to power the paradise. *Hazard:* rest here for real buffs
  (clear exhaustion, temp HP, max hit dice) — but every later use of a buff makes you
  hear the sacrifice's scream (psychic damage). The real trap is moral: leaving means
  *choosing* to walk out of perfect comfort back into the meat-grinder.
- **Wavering Waters (Divine Autopsy)** — great lakes of a dead god's drained humors.
  *Basins:* the Silver Deep (conductive cerebrospinal fluid, misfiring thought-storms
  below), the Iron Swell (copper-reeking blood, floating coagulated-blood scab islands),
  the Caustic Sea (bile — dissolves normal hulls, sickly radioactive glow). *Hazard:*
  you can't sail it in wood (need treated iron or the god's own bone) + Psychic Weather
  (a "storm" is a localized psychic scream forcing INT saves; an Iron-Swell wave rusts
  hulls and pulls oxygen from the air).
- **The Fossilized Sprawl** — a shattered magitek mega-city run as a **Gnoll turf war**
  (Warriors-style urban chaos, not a quiet ruin). Urban gnolls den in elevator shafts,
  zipline on mag-lev cables, mark turf with leaking magic batteries. Gangs:
  - **Neon-Maws** (entertainment district) — smear leaking optical-cable light as
    warpaint, wield shattered neon tubes (radiant/lightning), blind prey with strobes.
  - **Drop-Chutes** (skyscraper canopy) — canvas-tarp hang-gliders, death-from-above
    dive-bombs, grapple-winch back into the fog.
  - **Sump-Cacklers** (flooded subways) — blind, pale, massive, echolocating; subway-car
    armor; ambush by wading up out of the oily dark.

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
