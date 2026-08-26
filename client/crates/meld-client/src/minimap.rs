//! The **ground** under the menu's Map column — the terrain the blips stand on.
//!
//! # Why this is a tilemap and not more dots
//!
//! Both map surfaces were absolutely-positioned UI nodes over flat glass: the corner
//! panel spawned one node per entity **every frame**, and the Map column spawns one per
//! WALKED CELL — thousands of them on a long dive, every one laid out through taffy. So
//! they were radars, not maps: they showed where things were and nothing whatever about
//! where *you* were. Biome, coastline, water, high ground — none of it was on the one
//! surface whose entire job is answering "where am I".
//!
//! Ground is a GRID, and a grid drawn as UI nodes is the wrong shape twice over — Bevy
//! lays out every node through taffy, and a repaint means despawning the lot. So the
//! ground is a [`bevy_ecs_tilemap`] tilemap: the tile entities are allocated **once** at
//! startup and thereafter only their [`TileTextureIndex`] and [`TileColor`] are written,
//! which is what a map wants. It renders through its own 2D camera onto its own
//! [`RenderLayers`] into a texture, and the Map column shows that texture — so it stays a
//! UI element and never touches the 3D scene or the OIT sprite pipeline.
//!
//! The blips stay UI nodes on top: there are dozens of them, they are not on the grid, and
//! the column only rebuilds when you open it.
//!
//! # Why the corner panel is gone
//!
//! It was 140px across. At any tile count fine enough to keep a tile square in WORLD units
//! as the Explorer's radius grows, a 64px tile lands on about one and a half pixels and the
//! art reads as static — which is what it did. The Map column gives the same picture
//! 460x260 to breathe in, and it is where the dive's *remembered* ground already lived, so
//! the two readings of one world are now one surface instead of two that disagreed.
//!
//! # The tiles are the world's own tiles
//!
//! Ground reads with `tile_<biome>.png` and water with `water_<clear|bog|ice>.png` — the
//! **same art the ground shader samples**, not a palette of flat colours approximating it.
//! That is deliberate and has been asked for by name: a minimap whose sea is a blue
//! rectangle is a second, disagreeing description of a coastline this repo went to some
//! trouble to keep in [`meld_proto::coast`], once.

use bevy::asset::RenderAssetUsages;
use bevy::camera::visibility::RenderLayers;
use bevy::camera::{ClearColorConfig, ImageRenderTarget, Projection, RenderTarget, ScalingMode};
use bevy::image::ImageSampler;
use bevy::prelude::*;
use bevy::render::render_resource::{
    Extent3d, TextureDimension, TextureFormat, TextureUsages,
};
use bevy_ecs_tilemap::prelude::*;

/// The minimap's own render layer. Nothing else in the client uses a layer at all
/// (everything sits on the default 0), so the tilemap is invisible to the game cameras
/// and the game is invisible to the minimap camera — including the UI, which would
/// otherwise be drawn a second time into this texture.
const LAYER: usize = 7;

/// Grid resolution, in the Map column's proportions. `460/96` and `260/54` are within
/// half a percent of each other, so a tile stays SQUARE on screen — a grid that stretched
/// to fill the panel would report a straight march as a diagonal, which is the same reason
/// [`crate::overworld::map_to_px`] fits both axes on one scale.
const TILES_X: u32 = 96;
const TILES_Y: u32 = 54;
/// One tile's size. With `TilemapTexture::Vector` this is **not** a free layout unit —
/// the crate asserts every source image is exactly this size, so it is the tiles' pixel
/// size (64x64) and nothing else. The camera's fixed projection is derived from it, so
/// the value never has to agree with anything on screen.
const TILE: f32 = 64.0;
/// The render target's size in pixels — above the panel's 460x260 so the map stays crisp
/// on a hidpi display, and in the same proportion as the grid.
const TARGET_W: u32 = 512;
const TARGET_H: u32 = 288;

// Tile indices into the `TilemapTexture::Vector` built by `setup`. Order is the only
// contract between the two, so they are named rather than written as bare numbers.
const T_FOREST: u32 = 0;
const T_DESERT: u32 = 1;
const T_ASHFALL: u32 = 2;
const T_TUNDRA: u32 = 3;
const T_MIRE: u32 = 4;
const T_WATER: u32 = 5;
const T_WATER_BOG: u32 = 6;
const T_WATER_ICE: u32 = 7;

/// The tile files, in index order. `TilemapTexture::Vector` requires every image to be
/// exactly [`TILE`] — these are all 64x64. A mismatch is a startup panic naming both
/// sizes rather than something subtle, which is the failure mode we want (and the one
/// that caught `TILE` being set to a made-up layout unit).
const TILE_FILES: [&str; 8] = [
    "ground/tile_forest.png",
    "ground/tile_desert.png",
    "ground/tile_ashfall.png",
    "ground/tile_tundra.png",
    "ground/tile_mire.png",
    "ground/water_clear.png",
    "ground/water_bog.png",
    "ground/water_ice.png",
];

/// The ground tile a biome reads with. Unknown biomes fall to forest rather than to a
/// blank, because a new biome showing as grass is a wrong map and a new biome showing as
/// a hole is a *broken* one.
fn ground_tile(biome: &str) -> u32 {
    match biome {
        "desert" => T_DESERT,
        "ashfall" => T_ASHFALL,
        "tundra" => T_TUNDRA,
        "mire" => T_MIRE,
        _ => T_FOREST,
    }
}

/// The water tile a biome's water reads with — the mire's water is bog, the tundra's is
/// ice, everything else is clear. Matches the prop spawner's choice, so a pond on the map
/// is the colour of the pond you are standing next to.
fn water_tile(biome: &str) -> u32 {
    match biome {
        "mire" => T_WATER_BOG,
        "tundra" => T_WATER_ICE,
        _ => T_WATER,
    }
}

/// The minimap's tilemap: the render-target texture the panel displays, and the tile
/// entities, allocated once and repainted in place.
#[derive(Resource)]
pub(crate) struct MinimapTiles {
    /// The texture the UI panel shows. Handed to the `ImageNode` on `MinimapRoot`.
    pub image: Handle<Image>,
    /// Row-major `TILES_X * TILES_Y`, indexed `x + y * TILES_X`.
    tiles: Vec<Entity>,
    /// Where the last repaint was centred, and at what scale — a repaint touches ~5k
    /// tiles, so it happens when the view actually moved, not every frame.
    last: Option<(Vec2, f32)>,
}

/// Build the render target, its camera, and the tile grid. Runs once at startup.
pub(crate) fn setup(
    mut commands: Commands,
    assets: Res<AssetServer>,
    mut images: ResMut<Assets<Image>>,
    array_texture_loader: Res<ArrayTextureLoader>,
) {
    // The texture the minimap camera draws into and the UI panel samples.
    let mut target = Image::new_fill(
        Extent3d { width: TARGET_W, height: TARGET_H, depth_or_array_layers: 1 },
        TextureDimension::D2,
        &[0, 0, 0, 0],
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::default(),
    );
    target.texture_descriptor.usage = TextureUsages::TEXTURE_BINDING
        | TextureUsages::COPY_DST
        | TextureUsages::RENDER_ATTACHMENT;
    // Pixel art: point sampling, or a 64px tile scaled into a 140px panel turns to soup.
    target.sampler = ImageSampler::nearest();
    let image = images.add(target);

    let (span_x, span_y) = (TILES_X as f32 * TILE, TILES_Y as f32 * TILE);
    commands.spawn((
        Camera2d,
        Camera {
            // Transparent, so the panel's own glass shows through wherever the world has
            // not streamed in yet — unknown ground reads as unknown rather than as sea.
            clear_color: ClearColorConfig::Custom(Color::NONE),
            // Before the main pass: this camera feeds a texture the UI then samples.
            order: -1,
            ..default()
        },
        // Bevy 0.19 split the render target OFF `Camera` into its own component.
        RenderTarget::Image(ImageRenderTarget::from(image.clone())),
        Projection::Orthographic(OrthographicProjection {
            scaling_mode: ScalingMode::Fixed { width: span_x, height: span_y },
            ..OrthographicProjection::default_2d()
        }),
        Msaa::Off,
        RenderLayers::layer(LAYER),
    ));

    let size = TilemapSize { x: TILES_X, y: TILES_Y };
    let map = commands.spawn_empty().id();
    let mut storage = TileStorage::empty(size);
    let mut tiles = Vec::with_capacity((TILES_X * TILES_Y) as usize);
    for y in 0..TILES_Y {
        for x in 0..TILES_X {
            let pos = TilePos { x, y };
            let e = commands
                .spawn(TileBundle {
                    position: pos,
                    tilemap_id: TilemapId(map),
                    // Nothing is known before the first snapshot; an invisible tile is how
                    // the map says so.
                    visible: TileVisible(false),
                    ..default()
                })
                .id();
            storage.set(&pos, e);
            tiles.push(e);
        }
    }
    // `tiles` is indexed `x + y * TILES_X`; the loops above fill it in exactly that order.
    debug_assert_eq!(tiles.len(), (TILES_X * TILES_Y) as usize);

    let tile_size = TilemapTileSize { x: TILE, y: TILE };
    // ⚠️ Through `load_tiled`, NOT `assets.load` — these are the SAME image assets the
    // ground shader samples, and an asset is keyed by its path. `load_tiled` asks for a
    // REPEAT sampler because the ground tiles its texture across a whole section; loading
    // the same path here with the default (clamp) sampler is two loaders fighting over one
    // asset, and whichever lands second decides. Clamp winning is a section-wide stretch of
    // one texel row instead of tiled ground — the whole world's terrain, broken by a
    // 140px panel asking for the same file. Repeat costs the tilemap nothing: its UVs
    // never leave 0..1.
    let texture =
        TilemapTexture::Vector(TILE_FILES.iter().map(|f| crate::world_render::load_tiled(&assets, f)).collect());
    commands.entity(map).insert((
        TilemapBundle {
            grid_size: tile_size.into(),
            map_type: TilemapType::Square,
            size,
            storage,
            texture: texture.clone(),
            tile_size,
            anchor: TilemapAnchor::Center,
            ..default()
        },
        RenderLayers::layer(LAYER),
    ));
    // Pre-build the texture array the non-`atlas` renderer wants, so the first frame that
    // needs it is not the frame that builds it.
    array_texture_loader.add(TilemapArrayTexture { texture, tile_size, ..default() });

    commands.insert_resource(MinimapTiles { image, tiles, last: None });
}

/// What one minimap cell shows: which tile, and how bright.
///
/// Brightness carries ELEVATION, which is the one thing a flat tile cannot say and the
/// one thing verticality made matter — a terrace you cannot climb without finding a
/// connector reads as a wall on the ground and read as nothing at all on the map.
fn sample(
    terrain: &crate::Terrain,
    frame: &crate::WorldFrame,
    wx: f32,
    wz: f32,
) -> Option<(u32, f32)> {
    let arc_half = frame.radial_arc_degrees.to_radians() * 0.5;
    // Corridor x. WG-4 bends a corridor into a fan by mapping corridor x to RADIUS, so a
    // world point's section is decided by how far OUT it is and not by its x coordinate —
    // the mistake that has bitten every piece of code in this repo that forgot the fan.
    //
    // ⚠️ But only when there IS a fan. `radial_bend` is the identity at `half <= 0`, so a
    // flat corridor's corridor-x is plain world x, and taking the radius there is the
    // same mistake mirrored: it reads `hypot(x, z)`, which drifts further from x the
    // further off the centre line you stand. This inverse must branch exactly where
    // `radial_bend` branches or the two disagree off-axis.
    let cx = if arc_half > 0.0 { (wx * wx + wz * wz).sqrt() } else { wx };
    let sec = terrain
        .sections
        .values()
        .find(|s| cx >= s.start_x as f32 && cx < s.end_x as f32)?;

    // Sea next: the coastline is analytic and owns the answer everywhere, including over
    // ground a section nominally covers.
    //
    // ⚠️ INCLUDING THE STRAITS (WG-7 continents), and the map is where that matters most.
    // A strait's whole point is a lateral decision — cross at the isthmus you can see, or
    // follow the shore to one you cannot — and a coastline you cannot see on the map makes
    // the second option undiscoverable, which collapses the decision back to the wall the
    // retired `Seam` was.
    let (straits, lobes) = crate::world_render::shore_snapshot();
    if (meld_proto::coast::Shore { arc_half, straits: &straits, lobes: &lobes })
        .is_ocean(wx, wz)
    {
        return Some((water_tile(&sec.biome), 1.0));
    }

    let (half, lat) = crate::overworld::radial_params(sec);
    // Un-bend: corridor z is an ANGLE scaled by the corridor's half-width.
    let cz = if half > 0.0 && lat > 0.0 {
        (wz.atan2(wx) / half) * lat
    } else {
        wz
    };
    let cell = sec.cell as f32;
    let col = ((cx - sec.start_x as f32) / cell).floor();
    let row = ((cz - sec.y_min as f32) / cell).floor();
    if col < 0.0 || row < 0.0 || col >= sec.cols as f32 || row >= sec.rows as f32 {
        return None;
    }
    let level = sec
        .levels
        .get(row as usize * sec.cols as usize + col as usize)
        .copied()
        .unwrap_or(0);
    // Each terrace steps the tile brighter. Clamped well under 2.0: a tint is a reading of
    // height, not a spotlight.
    let bright = (0.72 + level as f32 * 0.14).min(1.35);
    Some((ground_tile(&sec.biome), bright))
}

/// What world rectangle the map is currently showing, in the Map column's own framing.
///
/// The column fits the walked rectangle inside the panel on ONE scale for both axes
/// ([`crate::overworld::map_to_px`]); the ground has to cover **the whole panel** at that
/// same scale, or the tiles and the dots drawn over them describe different worlds. So
/// this carries what the panel corresponds to rather than what the walk does: the walked
/// rectangle's centre, and the world size of one tile.
#[derive(Resource, Default)]
pub(crate) struct MapView {
    /// The Map column is open. Repainting ~5k tiles for a panel nobody is looking at is
    /// work the overworld does not need to do while you walk.
    pub open: bool,
    pub centre: Vec2,
    /// World units one tile covers. Square by construction — see [`TILES_X`].
    pub units: f32,
}

impl MapView {
    /// The panel's framing, from the walked bounds. `w`/`h` are the panel's pixels.
    pub(crate) fn frame_on(bounds: (f32, f32, f32, f32), w: f32, h: f32) -> (Vec2, f32) {
        let (sx, sy) = ((bounds.2 - bounds.0).max(1.0), (bounds.3 - bounds.1).max(1.0));
        let scale = (w / sx).min(h / sy);
        let centre = Vec2::new((bounds.0 + bounds.2) * 0.5, (bounds.1 + bounds.3) * 0.5);
        // World units the panel spans at that scale, divided across the grid.
        (centre, (w / scale) / TILES_X as f32)
    }
}

/// Repaint the ground. Skips entirely when the map is not unlocked, the Map column is
/// shut, or the view has not moved far enough to change what a tile covers.
pub(crate) fn repaint(
    perks: Res<crate::PerksRes>,
    view: Res<MapView>,
    world: Res<crate::Overworld>,
    terrain: Res<crate::Terrain>,
    frame: Res<crate::WorldFrame>,
    map: Option<ResMut<MinimapTiles>>,
    mut tiles: Query<(&mut TileTextureIndex, &mut TileColor, &mut TileVisible)>,
) {
    let Some(mut map) = map else { return };
    if perks.0.explorer_map == 0 || !frame.have || !view.open {
        return;
    }
    let (centre, units) = (view.centre, view.units.max(0.01));
    // A repaint writes ~5k components; do it when the picture would actually differ —
    // half a tile of movement, or a change of scale as the walked rectangle grows.
    if let Some((last_c, last_u)) = map.last {
        if (last_u - units).abs() < 0.001 && centre.distance(last_c) < units * 0.5 {
            return;
        }
    }
    map.last = Some((centre, units));

    // Water bodies (ponds, bog pools, frozen ponds) are PROPS rather than terrain, so they
    // are stamped from the snapshot. Collected once instead of per tile: this is ~9k tiles
    // against a handful of pools, and the naive nesting is the quadratic scan that cost the
    // creature step 1.7 seconds a tick.
    let pools: Vec<(Vec2, f32, u32)> = world
        .entities
        .values()
        .filter(|e| {
            e.kind == meld_client::net::EntityKind::Obstacle
                && e.name.as_deref().is_some_and(meld_proto::coast::is_water_kind)
        })
        .map(|e| {
            let biome = e.name.as_deref().unwrap_or("");
            let t = match biome {
                "bog_pool" => T_WATER_BOG,
                "frozen_pond" => T_WATER_ICE,
                _ => T_WATER,
            };
            (Vec2::new(e.x, e.y), e.radius, t)
        })
        .collect();

    for ty in 0..TILES_Y {
        for tx in 0..TILES_X {
            // Tile centre in world units. Tilemap +y is up and the overworld's `y` maps to
            // world Z, which increases the same way here — north stays up.
            let dx = (tx as f32 - TILES_X as f32 * 0.5 + 0.5) * units;
            let dz = (ty as f32 - TILES_Y as f32 * 0.5 + 0.5) * units;
            let (wx, wz) = (centre.x + dx, centre.y + dz);

            let mut got = sample(&terrain, &frame, wx, wz);
            // A pool overrides the ground it sits on.
            let p = Vec2::new(wx, wz);
            if let Some((c, r, t)) = pools.iter().find(|(c, r, _)| c.distance(p) <= *r) {
                let _ = (c, r);
                got = Some((*t, got.map_or(1.0, |(_, b)| b)));
            }

            let Ok((mut idx, mut col, mut vis)) = tiles.get_mut(map.tiles[(tx + ty * TILES_X) as usize])
            else {
                continue;
            };
            match got {
                Some((t, b)) => {
                    if idx.0 != t {
                        idx.0 = t;
                    }
                    let want = Color::srgb(b, b, b);
                    if col.0 != want {
                        col.0 = want;
                    }
                    if !vis.0 {
                        vis.0 = true;
                    }
                }
                // Unstreamed ground stays blank — the map does not guess.
                None => {
                    if vis.0 {
                        vis.0 = false;
                    }
                }
            }
        }
    }
}

/// Arm the ground for the Map column, and frame it exactly as the column frames the walk.
///
/// This is its own system rather than a line inside the menu's render because the two run
/// on different clocks: `render_main_menu` rebuilds the panel only when something it reads
/// changes, while the ground has to keep up with the party walking under an open map.
/// Half the grid's width in tiles — what a corner panel showing the whole texture spans.
pub(crate) fn corner_tiles_half() -> f32 {
    TILES_X as f32 * 0.5
}

pub(crate) fn track_map_view(
    menu: Res<crate::MainMenu>,
    explored: Res<crate::overworld::ExploredMap>,
    perks: Res<crate::PerksRes>,
    world: Res<crate::Overworld>,
    session: Res<crate::Session>,
    mut view: ResMut<MapView>,
) {
    // TWO SURFACES, ONE TEXTURE. The Map column wants the whole walked rectangle; the corner
    // panel wants a tight ring around the player. Rather than render twice, the framing
    // follows whichever is being looked at — the menu when it is open, the player otherwise.
    let open = menu.section == Some(crate::MenuSection::Map) && explored.walked;
    if !open {
        // Corner mode: centred on the party, spanning the Explorer's own map radius.
        if perks.0.explorer_map == 0 {
            view.open = false;
            return;
        }
        let Some(me) = world.entities.get(&session.player_id) else {
            view.open = false;
            return;
        };
        view.open = true;
        view.centre = Vec2::new(me.x, me.y);
        view.units = (perks.0.explorer_map_radius.max(1.0) * 2.0) / TILES_X as f32;
        return;
    }
    // The panel's pixel size, from `explored_map`. One place would be better than two;
    // they are held together by `the_ground_is_framed_like_the_panel_it_sits_in`.
    const W: f32 = 460.0;
    const H: f32 = 260.0;
    let (centre, units) = MapView::frame_on(crate::overworld::map_bounds(&explored), W, H);
    view.open = true;
    view.centre = centre;
    view.units = units;
}

#[cfg(test)]
mod tests {
    use super::*;

    // Every biome the world can theme a section with must have a ground tile that is
    // actually one of the files we load — a biome added to the server and not here would
    // otherwise render as forest, silently, which is a map that lies.
    #[test]
    fn every_biome_has_a_ground_and_a_water_tile() {
        for b in ["forest", "field", "desert", "ashfall", "tundra", "mire"] {
            assert!((ground_tile(b) as usize) < TILE_FILES.len(), "{b} ground");
            assert!((water_tile(b) as usize) < TILE_FILES.len(), "{b} water");
        }
    }

    // Field and Forest are the same ground with different tree counts (the ratio IS the
    // content), so they must not read as different biomes on the map.
    #[test]
    fn field_and_forest_share_their_ground() {
        assert_eq!(ground_tile("field"), ground_tile("forest"));
    }

    // The mire's water is bog and the tundra's is ice: water is not one colour, and the
    // map's water has to match the pool you are standing next to.
    #[test]
    fn water_reads_by_biome() {
        assert_eq!(water_tile("mire"), T_WATER_BOG);
        assert_eq!(water_tile("tundra"), T_WATER_ICE);
        assert_eq!(water_tile("desert"), T_WATER);
        assert_ne!(water_tile("mire"), water_tile("tundra"));
    }

    /// A section spanning `start_x..end_x` of a flat corridor, all ground at level 0
    /// except one raised cell.
    fn section(biome: &str, start: f64, end: f64, half: f64) -> meld_client::net::TerrainSectionView {
        let (cols, rows) = (((end - start) / 2.0) as u32, 20u32);
        let mut levels = vec![0u8; (cols * rows) as usize];
        levels[0] = 3; // the corner cell is three terraces up
        meld_client::net::TerrainSectionView {
            index: 0,
            start_x: start,
            end_x: end,
            y_min: -20.0,
            cell: 2.0,
            cols,
            rows,
            levels,
            connectors: vec![],
            path: vec![],
            biome: biome.to_string(),
            radial_half: half,
            corridor_lateral: 20.0,
            peaks: vec![],
            // No inland seas in the fixture: these tests are about the BIOME/terrace
            // shading, and a strait here would put water over the cells they assert on.
            // The straits' own map behaviour is covered by `coast`'s geometry tests.
            straits: vec![],
            lobes: vec![],
        }
    }

    fn terrain(sec: meld_client::net::TerrainSectionView) -> crate::Terrain {
        let mut t = crate::Terrain::default();
        t.sections.insert(0, sec);
        t
    }

    fn frame(arc_degrees: f32) -> crate::WorldFrame {
        crate::WorldFrame { have: true, radial_arc_degrees: arc_degrees, ..default() }
    }

    // A point beyond every streamed section has no answer. The map must say "unknown"
    // rather than pick the nearest biome, or the frontier reads as solid ground the
    // party has not actually seen.
    #[test]
    fn ground_the_world_has_not_streamed_is_unknown() {
        let t = terrain(section("mire", 0.0, 40.0, 0.0));
        assert!(sample(&t, &frame(0.0), 400.0, 0.0).is_none());
    }

    // The section a world point belongs to is decided by its RADIUS, because WG-4 maps
    // corridor x to radius. Reading world x instead puts everything off the centre line
    // in the wrong section — the mistake this repo has made in every subsystem that
    // forgot the fan.
    #[test]
    fn a_section_is_found_by_radius_and_not_by_x() {
        // A real fan: half-arc 1.2 rad, so the world genuinely bends.
        let t = terrain(section("desert", 20.0, 60.0, 1.2));
        let f = frame(1.2f32.to_degrees() * 2.0);
        // Straight out along +x at r=40: inside the section either way.
        assert!(sample(&t, &f, 40.0, 0.0).is_some());
        // The same RADIUS, swung 1.1 rad round the fan — still well inside the corridor's
        // lateral extent, but its world x is now 18, BELOW the section's start_x of 20.
        // A lookup by world x drops it; a lookup by radius keeps it.
        let (wx, wz) = (40.0 * 1.1f32.cos(), 40.0 * 1.1f32.sin());
        assert!(wx < 20.0, "the fixture must actually put world x outside the span");
        assert!(
            sample(&t, &f, wx, wz).is_some(),
            "a point at the same radius must land in the same section however far round the fan it is"
        );
    }

    // The inverse has to branch exactly where `radial_bend` does. In a FLAT corridor
    // corridor-x is world x, and taking the radius instead reads `hypot(x, z)` — which is
    // correct on the centre line and drifts further off it the further out you stand, so
    // the bug hides precisely where a corridor is most walked.
    #[test]
    fn a_flat_corridor_reads_world_x_not_radius() {
        let t = terrain(section("forest", 0.0, 40.0, 0.0));
        let f = frame(0.0);
        // World (10, 18): x is inside 0..40, but hypot is 20.6 — both inside here, so
        // assert on the CELL instead, which only agrees if corridor-x is world x.
        let on_axis = sample(&t, &f, 10.0, 0.0);
        let off_axis = sample(&t, &f, 10.0, 18.0);
        assert!(on_axis.is_some() && off_axis.is_some());
        // A point past the section's end in x must be unknown even though its radius is
        // inside — the reading a radius-based lookup would get wrong.
        assert!(
            sample(&t, &f, 45.0, 0.0).is_none(),
            "world x past end_x is outside the section in a flat corridor"
        );
    }

    // Elevation is the one thing a flat tile cannot say, so it rides the tint. Higher
    // ground must read brighter, or verticality is invisible on the map.
    #[test]
    fn higher_ground_reads_brighter() {
        let t = terrain(section("forest", 0.0, 40.0, 0.0));
        let f = frame(0.0);
        // Flat corridor, so corridor coords ARE world coords: the raised cell is grid
        // (col 0, row 0) => x in [0,2), z in [-20,-18).
        let (_, high) = sample(&t, &f, 1.0, -19.0).expect("raised cell");
        let (_, low) = sample(&t, &f, 20.0, 0.0).expect("flat ground");
        assert!(high > low, "level 3 ({high}) should out-read level 0 ({low})");
    }

    // The ground covers the whole panel at the SAME scale `map_to_px` fits the walk to, so
    // a tile and the dot drawn over it name the same place. Two framings that drift apart
    // is a map whose terrain slides out from under its own landmarks — and the two live in
    // different files, so nothing but this holds them together.
    #[test]
    fn the_ground_is_framed_like_the_panel_it_sits_in() {
        const W: f32 = 460.0;
        const H: f32 = 260.0;
        // A deliberately non-square walked rectangle: the aspect fit is the whole point.
        let bounds = (-120.0f32, 40.0f32, 380.0f32, 190.0f32);
        let (centre, units) = MapView::frame_on(bounds, W, H);
        let span = TILES_X as f32 * units; // world units the panel's width covers

        for x in [-120.0f32, 0.0, 130.0, 380.0] {
            // Where the ground puts it: fraction across the grid, times the panel.
            let from_ground = ((x - (centre.x - span * 0.5)) / span) * W;
            // Where the dots put it.
            let (from_dots, _) = crate::overworld::map_to_px(x, 100.0, bounds, W, H);
            assert!(
                (from_ground - from_dots).abs() < 0.01,
                "x={x}: ground says {from_ground}px, dots say {from_dots}px"
            );
        }
    }

    // A tile has to be SQUARE on screen or a straight march reads as a diagonal — the same
    // rule `map_to_px` follows by fitting both axes on one scale. That holds only while the
    // grid keeps the panel's proportions, so the two are checked against each other rather
    // than trusted.
    #[test]
    fn a_tile_is_square_on_screen() {
        let (per_x, per_y) = (460.0 / TILES_X as f32, 260.0 / TILES_Y as f32);
        assert!(
            (per_x - per_y).abs() / per_x < 0.01,
            "a tile is {per_x}px wide and {per_y}px tall — the grid has drifted off the panel's aspect"
        );
    }

    // The index constants and the file list are one contract; a file inserted without
    // renumbering the constants would repaint the whole world as the wrong ground.
    #[test]
    fn tile_indices_match_their_files() {
        assert_eq!(TILE_FILES[T_FOREST as usize], "ground/tile_forest.png");
        assert_eq!(TILE_FILES[T_DESERT as usize], "ground/tile_desert.png");
        assert_eq!(TILE_FILES[T_ASHFALL as usize], "ground/tile_ashfall.png");
        assert_eq!(TILE_FILES[T_TUNDRA as usize], "ground/tile_tundra.png");
        assert_eq!(TILE_FILES[T_MIRE as usize], "ground/tile_mire.png");
        assert_eq!(TILE_FILES[T_WATER as usize], "ground/water_clear.png");
        assert_eq!(TILE_FILES[T_WATER_BOG as usize], "ground/water_bog.png");
        assert_eq!(TILE_FILES[T_WATER_ICE as usize], "ground/water_ice.png");
    }
}
