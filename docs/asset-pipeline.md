# Asset Pipeline — generating art for MELDWORLD's HD-2D renderer

> **Read this before generating tiles/sprites in PixelLab.** It captures what our
> renderer actually consumes, so you generate the *right* asset the first time
> instead of a tileset we can only use one tile from. Written after a long
> art-direction pass on dungeons (walls/floors/bosses).

## The one mental model: **surfaces vs. props**

MELDWORLD is **HD-2D** — 2D pixel-art **sprites** placed in a **real 3D
environment** (perspective camera, depth, dynamic light, tilt-shift/bloom). The
environment is *actual 3D geometry*, textured with pixel art; characters and
discrete objects are *2D billboards*. This one split decides which kind of asset
you generate for anything:

| Category | Examples | Rendered as | Asset you generate |
|---|---|---|---|
| **Surfaces** — large, continuous, they *tile* | floor, walls | **texture on 3D geometry** (a plane; extruded cubes) | **one seamless tile** per surface |
| **Props** — small, discrete, placed at a point | tree, chest, door, lever, torch, **boss**, character | **billboard sprite** (a PNG that faces the camera) | **a sprite** (a single PNG, or an 8-direction frame set if it animates/turns) |

Rule of thumb: **tiles across an area → texture on geometry; a *thing* sitting
somewhere → billboard sprite.** Corners and edges of walls come from the
**geometry** (a cube corner *is* a corner) — **not** from tiles.

## Surfaces: generate SINGLE seamless tiles, not tilesets

Because the 3D geometry + camera supply all the depth and corners, we only need a
**single seamless fill texture** per surface. We do **not** want Wang/Godot
tilesets or an auto-tiler.

- **Floor** → a **top-down**, seamless, tiling tile. ✅ *We already have these:*
  `assets/ground/tile_<biome>.png` (+ `sand.png`), extracted from the biome sets.
  The dungeon floor is just the overworld biome ground showing through, dimmed —
  so a desert dungeon already stands on sand with no extra work.
- **Wall** → a **side-view / elevation**, seamless tile (a brick/stone wall seen
  *from the front*, tiling vertically as it rises). ⚠️ **This is the real gap.**
  Today `dungeon_wall` cubes wear `tile_street.png` (a *top-down* cobblestone)
  tinted per biome as a **stopgap** — a top-down tile on a vertical face reads
  "okay but slightly off." The correct asset is a dedicated side-view wall
  texture per theme.

### PixelLab: pick the generator by what the asset *is*
- **Floor tile (top-down seamless):** `create_topdown_tileset` (what we used), or
  a single-tile generator. Take the seamless "full/center" fill tile.
- **Wall texture (side-view seamless):** `create_sidescroller_tileset` (platformer
  tiles *are* side-view brick/stone) or `create_tiles_pro`. Ask for a **seamless,
  head-on** wall that tiles vertically. **Not** the top-down tileset.
- **Map Workshop / tileset tools bake fake perspective.** Their *View angle*
  (5° side ↔ 90° top-down) and *Thickness* controls draw pseudo-2.5D **into a
  flat image** — that's for **2D-tilemap** games with no real depth. We have real
  3D, so a baked angle would *fight* our camera (double perspective). If you use
  that tool for a floor, set **90° top-down, 0% thickness** (pure flat top-face).
  Prefer a plain seamless single tile.

### Wang vs. Godot (why we ignore both)
Both are the *same* tiles packaged for **auto-tiling** (pick corner/edge/fill by
neighbor). **Wang** = the generic edge/corner convention (Hao Wang; the 47-tile
"blob" set). **Godot** = the same set arranged for Godot's TileMap importer. Both
are **2D-tilemap** machinery. Our 3D geometry makes corners for free, so we need
**neither** — just the seamless fill tile. (A single tile is a subset of a set, so
you can always extract the fill tile from a set you already generated; you can't go
the other way — so if you *might* want the fancier look later, keep the set as the
source.)

## Props: PNG billboards, and the 8-direction question

- **Format:** **PNG is correct** — pixel art is raster (not SVG) and sprites need
  **lossless + alpha** (never JPEG). Tiny, nearest-sampled. An *animated/directional*
  prop isn't one PNG — it's a **PNG spritesheet/atlas with frame metadata** (still
  PNG, just organized as frames). GPU-compressed formats (KTX2/Basis) only matter
  for *large* 3D textures — overkill for our tiny pixel tiles.
- **A door/lever is a prop, not a wall-texture.** Render it as a **billboard
  sprite** dropped into `WorldAssets::prop_sprites` — exactly like the trees —
  *not* a textured cube.
- **8 directions:** we generate 8-side sets for characters/bosses. The renderer
  (`hd2d::animate_chars`) already picks the frame **relative to the camera** —
  billboard (faces camera) **+** directional frame (by the object's world facing).
  Both at once; that's the HD-2D illusion.
  - **Characters / bosses** → use the 8-direction `CharacterFrames`
    (`characters/<class>/`, `bosses/<key>/`). Heroes do this; bosses now do too.
    Roaming **creatures** are currently single-PNG (`monsters/<kind>.png`) — they
    *don't* turn/animate; give them frame sets if we want them to.
  - **Static props** (tree, chest, door): under our **fixed-yaw** camera you only
    ever see one side, so **one view is correct** — 8 directions on a static prop
    only pays off **if we add a rotatable camera** (which the 8-side art is
    clearly built for; likely a future feature). It's a `sprite` question only —
    orthogonal to surfaces.

## The per-dungeon-theme shopping list (small!)
1. **Floor tile** — top-down seamless. ✅ have (`tile_<biome>`).
2. **Wall texture** — *side-view* seamless stone/brick. ⚠️ generate (one per
   theme, or one grey stone we tint). Wire into `WorldAssets::wall_tex` /
   `spawn_obstacle`'s dungeon-wall branch (`overworld.rs`).
3. **Door / lever sprites** — billboards into `prop_sprites`.
4. (Bosses already have their 8-dir sets in `bosses/<key>/`.)

## Where it wires in code
- Tiling textures: `world_render::load_tiled(&assets, path)` (Repeat sampler).
- Ground tiles: `assets/ground/tile_<biome>.png` (biome-blend shader).
- Dungeon wall texture: `WorldAssets::wall_tex` → cubes in `spawn_obstacle`.
- Prop billboards: `WorldAssets::prop_sprites` (keyed `obstacle_<kind>`, …) → `spawn_billboard_entity`.
- Character/boss frames: `hd2d::CharacterFrames` (idle[8]/walk[8]/clips), loaded by
  `hd2d::load_character*`; picked per-frame by `animate_chars` (facing relative to camera).

## TL;DR for the next generation session
- Generate **single seamless tiles**, not Wang/Godot **tilesets**.
- **Floor** = top-down seamless (have it). **Wall** = *side-view* seamless (need it) —
  use `create_sidescroller_tileset`/`create_tiles_pro`, head-on, tiling.
- **Doors/levers/props** = billboard **sprites** (PNG), like trees.
- **PNG** everywhere; animated/8-dir things = PNG **spritesheets**.
- Corners/edges come from **3D geometry**, so **no auto-tiler, no Wang set** needed.
