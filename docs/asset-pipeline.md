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

## ⚠️ Character sprites: the canvas FILL is what sets on-screen size

A character billboard maps the **whole PNG** onto a fixed-size quad
(`hd2d::cyl_billboard_mesh`), so what decides how big a hero draws in the world is
**not** the art's pixel height — it is **the fraction of the canvas the art fills**.

The shipped class sets are **~90px of art centred on a 184px canvas — a 48% fill**
(measured: psyker content 85x89, 47px of transparent margin above and 48px below).
They read that way because PixelLab used to inflate its canvas ~40% past the
requested `size` "to make room for animations". **It no longer does**: a `size: 96`
request now returns a 96px canvas with the character running edge to edge, ~91%
fill. Generate a class that way and it walks into the party at **roughly twice the
size of every hero beside it** — and it does not show up in any preview thumbnail,
because a thumbnail is normalised.

So the recipe for a new class is:

1. `create_character` at **`size: 96`, `mode: "v3"`** — that is the pixel density the
   existing set is drawn at, so the new class's pixels are the same size as its
   party's. (Do **not** ask for 184 to "match": you get 184px of art filling a 184px
   canvas, i.e. the same too-big result at double the detail, which then clashes with
   the chunkier pixels next to it.)
2. Animate in **v3 custom**, never template — see below.
3. `client/scripts/install_class_sprite.sh <character-id> <key> [asset-dir]` —
   downloads the zip, lifts it out of its state folder into `characters/<key>/` (or
   `creatures/<key>/`), and runs `client/scripts/pad_sprites.py`, which pads every
   frame out to **184x184 centred**. Padding, never scaling: pixel art must not be
   resampled, so the art is copied through untouched and only transparent margin is
   added.

Verify with the content-height check the script prints — a new class should land at
**~90px content, ~47px top pad, ~48% fill**, the same as psyker/shifter/hunter.
Bosses run bigger on purpose (95-121px of content) and answer the same rule.

### The bestiary: `client/scripts/gen_creature_sprites.py`

Creatures get the same treatment (a species was one static 32px png), driven from
`client/scripts/creature_sprites.json` — ordered by **how close to the hub a creature
lives**, so an interrupted run leaves the shallow end, which is what almost every
player actually sees, finished first. The driver records every step and is safe to
kill and re-run.

**Each species is TWO characters**, `<kind>` and `<kind>_minion`. A pack's leader and
its minions are the same species at 1.7x and 0.45x HP, and scaling one sprite only
ever made a bigger or smaller copy of the same animal. A runt is a different animal.

**Walk is drawn for all eight facings; attack is south-only.** Walking happens on the
overworld, where a creature crosses the view in every direction and a body that slides
sideways while facing you is what reads as broken. An attack is only ever seen in the
arena, which faces the party, so its other seven directions would be art for a camera
angle that never occurs. `hd2d::load_creature_clips` takes that per-clip and reuses the
south attack for every facing.

### Prompting notes (each of these cost a re-roll)

- **Say "full body, head to toe, boots visible."** A character described mostly from
  the waist up gets framed from the waist up, and the legs run off the canvas.
- **Keep held gear DOWN at the sides.** "Hammer resting on the shoulder" put the hammer
  head above the skull and clipped it off the top of the frame.
- **A weapon in the description can duplicate.** An explorer asked for a spear came back
  with extra spear tips branching off the shaft.
- **Template animations DROP hand-held weapons** mid-swing (skeleton retarget), so clips
  are **v3 custom** (`mode: "v3"`, `action_description`, `frame_count: 8`,
  `keep_first_frame: false`). v3 also defaults to **south only** — always pass
  `directions` explicitly.
- 10 concurrent job slots. A 1-direction clip is one job, so a batch of characters runs
  at once; an 8-direction clip takes the whole cap and serializes everything behind it.

## The per-dungeon-theme shopping list (small!)
1. **Floor tile** — top-down seamless. ✅ have (`tile_<biome>`).
2. **Wall texture** — *side-view* seamless stone/brick. ⚠️ generate (one per
   theme, or one grey stone we tint). Wire into `WorldAssets::wall_tex` /
   `spawn_obstacle`'s dungeon-wall branch (`overworld.rs`).
3. **Door / lever sprites** — billboards into `prop_sprites`.
4. (Bosses already have their 8-dir sets in `bosses/<key>/`.)

## Login backdrops: a video, baked to frames

The login screen plays a looping clip behind the panel. Bevy plays no video, so a clip
is **baked into a WebP frame sequence** and stepped by `LoginBg`
(`client/crates/meld-client/src/screens.rs`):

```sh
client/scripts/bake_login_bg.sh client/crates/meld-client/assets/loginscreens/<clip>.mp4
```

That decodes via AVFoundation (no ffmpeg) and writes
`assets/loginscreens/<clip>/frame###.webp`. Keep the source `.mp4` beside the folder —
it is the master, and nothing loads it at runtime.

- **WebP, not JPEG** — every `zune-jpeg` 0.5.x (what bevy's `jpeg` feature pulls in)
  fails to build on the current rustc. Bevy's `webp` feature uses an unrelated decoder.
- **Every frame is a live GPU texture** while the login screen is up, so width is a
  memory decision, not a quality one: 120 frames at 640×360 is ~110 MB of VRAM (and
  4.3 MB on disk). The handles are dropped on log-in.
- **Playback is ping-pong** (forwards, then backwards). A push-in clip does not join
  its own first frame, so a plain loop would jump.
- After a re-bake, set `LOGIN_BG_FRAMES` in `screens.rs` to the count the script prints.

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
