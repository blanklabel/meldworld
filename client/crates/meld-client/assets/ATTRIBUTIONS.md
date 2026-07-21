# Asset attributions

The overworld follows the **HD-2D** split: 2D pixel sprites for the actors, real 3D
geometry for the world.

## `models/nature/` — 3D world props & harvest nodes  (CC0)

Low-poly 3D models from **Kenney's Nature Kit**, licensed **CC0 (public domain)** —
free for commercial use, attribution not required (`License.txt` in the pack). Used
for every terrain obstacle (trees, rocks, cliffs, cactus, stumps, mushrooms, …) and
harvest node, so the world is built from lit, shadow-casting geometry instead of flat
billboards.

- Source: https://kenney.nl/assets/nature-kit
- License: Creative Commons Zero (CC0)
- Format: glTF (`.glb`); each model's spawn scale is baked from its bounding box.

## `models/fantasy-town/`, `models/graveyard/`, `models/pirate/` — the hub city  (CC0)

Low-poly 3D kits from **Kenney**, all licensed **CC0 (public domain)** — free for
commercial use, attribution not required. They build **The Weld**, the walkable hub
city (`Screen::City`; see `CITY-PROPOSAL.md`): the New Weird "last city" is welded
together from a fantasy-town core, graveyard/crypt uncanny, and pirate salvage.

- **Fantasy Town Kit** — https://kenney.nl/assets/fantasy-town-kit — stalls, cart,
  fountain, lanterns, the Threshold archway, modular walls/roofs.
- **Graveyard Kit** — https://kenney.nl/assets/graveyard-kit — crypts (district
  buildings), gravestones (the Vanguard Wall), fire-basket, iron fences, and the
  keeper/ghost/skeleton dwellers.
- **Pirate Kit** — https://kenney.nl/assets/pirate-kit — the beached ship-wreck +
  dock the city is salvaged from, chests, barrels, crates.
- License: Creative Commons Zero (CC0).
- Format: glTF (`.glb`); each kit's shared `Textures/colormap.png` is kept beside
  its models (the GLBs reference it by relative path). Spawn scales are eyeballed
  per model in `CITY_PROPS` (client `main.rs`).

## `monsters/` — creature billboards  (public domain)

Pixel-art tiles from **Dungeon Crawl Stone Soup** (`rltiles`), which is **public
domain**: the tiles derive from the public-domain **RLTiles** set
(`crawl-ref/source/rltiles/license.txt`), with DCSS's own originals released **CC0**
(`crawl-ref/docs/license/cc0.txt`). Creatures stay 2D sprites (the HD-2D convention).

- Source repo: https://github.com/crawl/crawl (`crawl-ref/source/rltiles/mon/`)
- License: Public Domain (RLTiles) / CC0 (DCSS-original tiles)

## `ground/grass_full.png` — tiled ground texture  (public domain)

DCSS/RLTiles grass floor tile (same license as above), repeated across the ground
plane and tinted per biome.

## `fx/portal_arch.png` — extraction portal  (public domain)

DCSS/RLTiles gateway tile (same license as above), billboarded as the exit portal.

## `characters/`

AI-generated for MELDWORLD (prompts recorded in each character's `metadata.json`).
Not third-party art.

## `landscape/Tree01`, `landscape/Tree02`

Pre-existing rendered tree billboards (provenance predates this work; **no longer
used** — the 3D Nature Kit trees replaced them).
