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
