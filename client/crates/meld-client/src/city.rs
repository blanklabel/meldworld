//! The Last City — the persistent hub city: walkable HD-2D plaza, districts, HUD.
//! Extracted from `main.rs` during the module reorg.


use bevy::prelude::*;

use meld_client::glass;
use bevy::gltf::GltfAssetLabel;

use meld_client::hd2d::{self, CharSprite};
use meld_client::net::ClientCmd;

use super::*;

// ----------------------------------------------------------------- city ----
// The Last City — the persistent hub city: a walkable HD-2D plaza (CANON D16). You walk
// your avatar between districts built from Kenney CC0 kits (fantasy-town / graveyard
// / pirate — see assets/ATTRIBUTIONS.md) and interact with the one you're standing
// in. M0 wires The Threshold (dive) + The Vault-Deep (live `GET /v1/vault`); the
// rest are placed but not yet functional. This closes the extract-or-die loop —
// you always come home here. See docs/proposals/last-city.md.

/// What interacting with a district does.
#[derive(Clone, Copy)]
pub(crate) enum CityAction {
    /// The Threshold: step onto the plane (solo dive).
    Dive,
    /// The Vault-Deep: toggle the banked chits/materials/gear panel.
    Vault,
    /// The Vanguard Wall: light it with the live seasonal leaderboard (P1-1).
    Vanguard,
    /// The Apothecary: the one NPC who sells the lowest-tier basics for chits.
    Shop,
    /// The Forge & Alembic: the recipe book, and the anvil (MS-1).
    Craft,
    /// The Drill Yard: pick the party you take down.
    Party,
    /// A not-yet-raised district: post its milestone notice.
    Notice(&'static str),
}

/// A city action reachable by an on-screen (touch) button — always available, so a
/// player can dive / open the Vault / go co-op with a tap instead of walking to the
/// matching district and pressing a key. The keyboard paths (`city_input`) still work.
#[derive(Clone, Copy, PartialEq)]
pub(crate) enum CityAct {
    Dive,
    Vault,
    Coop,
    /// The Drill Yard: open party management.
    Party,
}

/// Marks a tappable on-screen city action button.
#[derive(Component)]
pub(crate) struct CityActionButton(pub(crate) CityAct);

/// A walkable district: an anchor on the plaza the avatar can stand in and act on.
pub(crate) struct District {
    label: &'static str,
    x: f32,
    z: f32,
    /// Radius the avatar must be within to interact.
    radius: f32,
    action: CityAction,
}

/// The city's interactable districts (positions are plaza-local world x/z; the
/// avatar spawns near +z/south and the camera looks north/-z).
pub(crate) const CITY_DISTRICTS: &[District] = &[
    District { label: "The Threshold", x: 0.0, z: -19.0, radius: 5.5, action: CityAction::Dive },
    District { label: "The Vault-Deep", x: -13.0, z: -5.0, radius: 5.0, action: CityAction::Vault },
    District {
        label: "The Market Tiers",
        x: 13.0,
        z: 0.0,
        radius: 6.0,
        action: CityAction::Shop,
    },
    District {
        label: "The Forge & Alembic",
        x: -10.0,
        z: 9.0,
        radius: 5.0,
        action: CityAction::Craft,
    },
    District {
        label: "The Bounty Board",
        x: 8.0,
        z: -12.0,
        radius: 4.5,
        action: CityAction::Notice("The Bounty Board is bare - gathering contracts arrive in M2."),
    },
    District {
        label: "The Drill Yard",
        x: 15.0,
        z: -13.0,
        radius: 5.0,
        action: CityAction::Party,
    },
    District {
        label: "The Vanguard Wall",
        x: -15.0,
        z: -14.0,
        radius: 5.0,
        action: CityAction::Vanguard,
    },
];

/// Static city props: `(model path under assets/models, x, z, yaw°, scale)`. The
/// GLBs are Kenney CC0 kits; scales are eyeballed to a ~1-unit grid.
pub(crate) const CITY_PROPS: &[(&str, f32, f32, f32, f32)] = &[
    // The Threshold — a great archway onto the plane (far north).
    ("fantasy-town/wall-arch-top", 0.0, -19.0, 0.0, 5.0),
    // The Vault-Deep — a large crypt strongroom + a hoard chest (west).
    ("graveyard/crypt-large", -13.0, -5.0, 90.0, 2.6),
    ("graveyard/crypt-large-roof", -13.0, -5.0, 90.0, 2.6),
    ("pirate/chest", -13.0, -1.0, 0.0, 1.4),
    // The Market Tiers — a row of stalls + a cart (east).
    ("fantasy-town/stall-red", 13.0, -2.0, 180.0, 1.7),
    ("fantasy-town/stall-green", 13.0, 1.5, 180.0, 1.7),
    ("fantasy-town/stall", 15.5, -0.2, 180.0, 1.7),
    ("fantasy-town/cart", 10.5, 2.0, 90.0, 1.5),
    // The Forge & Alembic — a crypt workshop + a fire-basket (southwest).
    ("graveyard/crypt-b", -10.0, 10.0, 0.0, 2.4),
    ("graveyard/fire-basket", -10.0, 7.0, 0.0, 1.6),
    ("pirate/barrel", -8.0, 8.5, 0.0, 1.2),
    // The Bounty Board — a small crypt + a banner (northeast).
    ("graveyard/crypt-a", 8.0, -13.0, 180.0, 2.2),
    ("fantasy-town/banner-red", 8.0, -10.0, 0.0, 1.8),
    // The Drill Yard — blades + crates behind a fence (far east-north).
    ("fantasy-town/blade", 15.0, -12.0, 0.0, 1.4),
    ("pirate/crate", 16.5, -14.0, 0.0, 1.2),
    ("graveyard/iron-fence", 13.0, -13.0, 90.0, 1.5),
    // The Vanguard Wall — carved gravestones + candles (northwest).
    ("graveyard/gravestone-cross-large", -15.0, -14.0, 180.0, 2.0),
    ("graveyard/gravestone-wide", -16.8, -13.0, 180.0, 1.6),
    ("graveyard/gravestone-round", -13.3, -15.0, 180.0, 1.6),
    ("graveyard/candle-multiple", -15.0, -12.0, 0.0, 1.6),
    // The Commons — central fountain ringed by lanterns.
    ("fantasy-town/fountain-round", 0.0, 0.0, 0.0, 2.2),
    ("fantasy-town/lantern", 4.5, 3.5, 0.0, 1.6),
    ("fantasy-town/lantern", -4.5, 3.5, 0.0, 1.6),
    ("fantasy-town/lantern", 4.5, -3.5, 0.0, 1.6),
    ("fantasy-town/lantern", -4.5, -3.5, 0.0, 1.6),
    // Salvage the last city is welded from — a beached wreck + a dock (far corner).
    ("pirate/ship-wreck", 21.0, -19.0, 210.0, 2.2),
    ("pirate/structure-platform-dock", 19.0, -10.0, 0.0, 1.6),
    // First dwellers (a hint of the crowd to come in M1).
    ("graveyard/character-keeper", 2.5, 4.0, 180.0, 1.4),
    ("graveyard/character-ghost", -3.0, -1.0, 150.0, 1.4),
    ("graveyard/character-skeleton", 6.0, 2.0, 200.0, 1.4),
];

/// The city HUD (2D overlay over the walkable scene): identity + live Vault line
/// at the top, a contextual interact prompt at the bottom. Also (re)fetches the
/// Vault and re-arms the dive on every arrival — extract → walk in → see it grow.
pub(crate) fn city_hud(
    mut commands: Commands,
    net: NonSend<NetRes>,
    mut inv: ResMut<InventoryData>,
    mut session: ResMut<Session>,
    mut city: ResMut<CityUi>,
) {
    inv.loaded = false;
    net.0.fetch_inventory();
    city.notice.clear();
    city.near = None;
    // `MELD_WALL`/`?wall` lights the board on arrival for screenshot frames; a
    // real player toggles it with [E] at the wall.
    city.board_open = crate::flags::wall_preview_flag();
    if city.board_open {
        net.0.fetch_vanguard();
    }
    city.craft_open = crate::flags::forge_preview_flag();
    if city.craft_open {
        net.0.fetch_recipes();
    }
    city.shop_open = crate::flags::shop_preview_flag();
    if city.shop_open {
        // All three halves, as [E] does — the preview flag exists to frame the WHOLE
        // counter, and fetching only the shelf left the Requisition and the Broker
        // missing from every screenshot taken with it.
        net.0.fetch_shop();
        net.0.fetch_gear_shop();
        net.0.fetch_broker();
    }
    session.entered = false;
    session.status.clear();

    let amber = Color::srgb(0.96, 0.78, 0.45);
    let teal = Color::srgb(0.55, 0.85, 0.9);

    commands
        .spawn((
            CityRoot,
            Node {
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                flex_direction: FlexDirection::Column,
                align_items: AlignItems::Center,
                justify_content: JustifyContent::SpaceBetween,
                padding: UiRect::all(Val::Px(18.0)),
                ..default()
            },
        ))
        .with_children(|p| {
            p.spawn(glass::hud(Val::Auto))
            .with_children(|t| {
                t.spawn((
                    Text::new("THE LAST CITY"),
                    TextFont { font_size: 34.0, ..default() },
                    TextColor(amber),
                ));
                t.spawn((
                    CityVaultText,
                    Text::new("The Vault-Deep is being tallied..."),
                    TextFont { font_size: 16.0, ..default() },
                    TextColor(teal),
                ));
            });
            // The status line doubles as the Apothecary's shelf and the Vanguard
            // board, so it is a menu: it gets the shared glass rather than bare
            // text washing out against the plaza.
            p.spawn(glass::hud(Val::Auto)).with_children(|panel| {
                panel.spawn((
                    CityStatusText,
                    Text::new(""),
                    TextFont { font_size: 18.0, ..default() },
                    TextColor(glass::TITLE),
                ));
            });
            // Always-available tap actions (bottom-right). Mirror the keyboard: Dive
            // (Enter), Vault (V), Co-op (C) — so the hub is fully click/tap driven
            // without having to walk to each district first.
            p.spawn((
                Node {
                    position_type: PositionType::Absolute,
                    right: Val::Px(16.0),
                    bottom: Val::Px(16.0),
                    flex_direction: FlexDirection::Column,
                    row_gap: Val::Px(8.0),
                    align_items: AlignItems::FlexEnd,
                    ..default()
                },
            ))
            .with_children(|bar| {
                for (act, label) in [
                    (CityAct::Party, "Party"),
                    (CityAct::Dive, "Run"),
                    (CityAct::Vault, "Vault"),
                    (CityAct::Coop, "Co-op"),
                ] {
                    city_button(bar, act, label);
                }
            });
        });
}

/// Spawn one always-available city action button into the tap bar.
fn city_button(parent: &mut ChildSpawnerCommands, act: CityAct, label: &str) {
    parent
        .spawn((
            Button,
            CityActionButton(act),
            Node {
                width: Val::Px(150.0),
                padding: UiRect::axes(Val::Px(14.0), Val::Px(11.0)),
                justify_content: JustifyContent::Center,
                border: UiRect::all(Val::Px(1.5)),
                ..default()
            },
            BorderColor(glass::EDGE),
            BorderRadius::all(Val::Px(8.0)),
            BackgroundColor(glass::ACTIVE),
        ))
        .with_children(|b| {
            b.spawn((
                Text::new(label.to_string()),
                TextFont { font_size: 16.0, ..default() },
                TextColor(Color::srgb(0.98, 0.9, 0.68)),
            ));
        });
}

/// Handle taps on the city action buttons — the same effects as the `city_input`
/// keyboard shortcuts, so touch and keyboard stay interchangeable.
#[allow(clippy::too_many_arguments)]
pub(crate) fn city_action_buttons(
    q: Query<(&Interaction, &CityActionButton), Changed<Interaction>>,
    net: NonSend<NetRes>,
    mut session: ResMut<Session>,
    mut city: ResMut<CityUi>,
    mut overlay: ResMut<Overlay>,
    mut tab: ResMut<OverlayTab>,
    mut inv: ResMut<InventoryData>,
    mut next: ResMut<NextState<Screen>>,
) {
    for (interaction, btn) in &q {
        if *interaction != Interaction::Pressed {
            continue;
        }
        match btn.0 {
            CityAct::Dive => {
                if !session.entered {
                    session.entered = true;
                    session.coop = false;
                    session.status = "stepping through The Threshold...".to_string();
                    net.0.send(ClientCmd::EnterMaze { party: session.party.clone(), tutorial: false });
                }
            }
            CityAct::Party => {
                city.party_open = !city.party_open;
                if city.party_open {
                    city.notice.clear();
                    net.0.fetch_hero_names();
                    net.0.fetch_loadouts();
                }
            }
            CityAct::Vault => {
                if overlay.kind == Some(OverlayKind::Inventory) {
                    overlay.kind = None;
                } else {
                    overlay.kind = Some(OverlayKind::Inventory);
                    *tab = OverlayTab::Items;
                    inv.loaded = false;
                    net.0.fetch_inventory();
                    net.0.fetch_hero_names();
                }
            }
            CityAct::Coop => {
                session.coop = true;
                next.set(Screen::Lobby);
            }
        }
    }
}

/// Spawn the walkable 3D city: a plaza floor, the Kenney-kit buildings/props from
/// [`CITY_PROPS`], and the player avatar (reusing the overworld HD-2D avatar).
#[allow(clippy::too_many_arguments)]
/// A flat road quad (XZ plane, centred at origin) of `len`×`width`, UV-tiled so the
/// cobblestone texture repeats (~2.5 world units per tile) instead of stretching.
fn road_mesh(len: f32, width: f32) -> Mesh {
    use bevy::render::mesh::{Indices, PrimitiveTopology};
    use bevy::render::render_asset::RenderAssetUsages;
    let (hl, hw) = (len * 0.5, width * 0.5);
    let tile = 2.5;
    let (u, v) = (len / tile, width / tile);
    let mut m = Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::default());
    m.insert_attribute(
        Mesh::ATTRIBUTE_POSITION,
        vec![[-hl, 0.0, -hw], [hl, 0.0, -hw], [hl, 0.0, hw], [-hl, 0.0, hw]],
    );
    m.insert_attribute(Mesh::ATTRIBUTE_NORMAL, vec![[0.0, 1.0, 0.0]; 4]);
    m.insert_attribute(
        Mesh::ATTRIBUTE_UV_0,
        vec![[0.0, 0.0], [u, 0.0], [u, v], [0.0, v]],
    );
    // Wind so the +Y (up) face is front-facing — the reverse order would put the
    // visible face downward and get it back-face-culled from the overhead camera.
    m.insert_indices(Indices::U32(vec![0, 2, 1, 0, 3, 2]));
    m
}

/// A magitech street light: its point light breathes over time (see [`pulse_magitech`]).
#[derive(Component)]
pub(crate) struct MagitechLight {
    phase: f32,
    base: f32,
}

/// Gently pulse the magitech lamps so the hub feels alive (a slow energy breathing).
pub(crate) fn pulse_magitech(time: Res<Time>, mut q: Query<(&MagitechLight, &mut PointLight)>) {
    let t = time.elapsed_secs();
    for (m, mut light) in &mut q {
        light.intensity = m.base * (0.82 + 0.18 * (t * 2.0 + m.phase).sin());
    }
}

pub(crate) fn city_scene(
    mut commands: Commands,
    assets: Res<AssetServer>,
    wa: Option<Res<WorldAssets>>,
    session: Res<Session>,
    look: Res<hd2d::Look>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut mats: ResMut<Assets<StandardMaterial>>,
) {
    let Some(wa) = wa else { return };

    // --- STREETS: a cobblestone plaza around the fountain + radial spokes out to
    // each district, laid flat on the grass (y just above 0 to avoid z-fighting). ---
    let street_mat = mats.add(StandardMaterial {
        base_color: Color::srgb(0.8, 0.8, 0.84),
        base_color_texture: Some(crate::world_render::load_tiled(&assets, "ground/tile_street.png")),
        perceptual_roughness: 0.95,
        ..default()
    });
    // Central plaza (a paved square around the fountain).
    commands.spawn((
        CityScene,
        Mesh3d(meshes.add(road_mesh(13.0, 13.0))),
        MeshMaterial3d(street_mat.clone()),
        Transform::from_xyz(0.0, 0.02, 0.0),
    ));
    // A spoke from the plaza edge out to each district anchor.
    for d in CITY_DISTRICTS {
        let dir = Vec2::new(d.x, d.z);
        let len = dir.length();
        if len < 6.0 {
            continue;
        }
        let n = dir / len;
        let start = 4.5; // leave the plaza; stop a bit short of the building
        let seg_len = (len - start - 2.0).max(1.0);
        let mid = n * (start + seg_len * 0.5);
        let angle = f32::atan2(-n.y, n.x); // align the quad's local +X with the spoke
        commands.spawn((
            CityScene,
            Mesh3d(meshes.add(road_mesh(seg_len, 3.4))),
            MeshMaterial3d(street_mat.clone()),
            Transform::from_xyz(mid.x, 0.02, mid.y).with_rotation(Quat::from_rotation_y(angle)),
        ));
    }

    // Buildings + district props (Kenney CC0 kits). The old fountain-ring lanterns are
    // skipped here — replaced by the glowing magitech lamps spawned below.
    for (path, x, z, yaw, scale) in CITY_PROPS {
        if *path == "fantasy-town/lantern" {
            continue;
        }
        commands.spawn((
            CityScene,
            SceneRoot(
                assets.load(GltfAssetLabel::Scene(0).from_asset(format!("models/{path}.glb"))),
            ),
            Transform::from_xyz(*x, 0.0, *z)
                .with_rotation(Quat::from_rotation_y(yaw.to_radians()))
                .with_scale(Vec3::splat(*scale)),
        ));
    }

    // --- MAGITECH LIGHTS: glowing cyan energy lamps (a bespoke sprite that blooms via
    // an HDR emissive), each with a real point light so they illuminate day or night.
    // Four ring the fountain (where the old lanterns were) + a pair down each spoke. ---
    let lamp_tex = assets.load("props/decor_magitech_pylon.png");
    let mut lamp_spots: Vec<Vec2> = vec![
        Vec2::new(4.5, 3.5),
        Vec2::new(-4.5, 3.5),
        Vec2::new(4.5, -3.5),
        Vec2::new(-4.5, -3.5),
    ];
    for d in CITY_DISTRICTS {
        let dir = Vec2::new(d.x, d.z);
        let len = dir.length();
        // Only line the longest streets, so the plaza doesn't fill with lamps.
        if len < 15.0 {
            continue;
        }
        let n = dir / len;
        // A lamp partway along the spoke, offset to the side so it lines the street.
        let side = Vec2::new(-n.y, n.x) * 2.2;
        lamp_spots.push(n * (len * 0.55) + side);
    }
    for (i, s) in lamp_spots.iter().enumerate() {
        let h = 2.4;
        let lmat = mats.add(StandardMaterial {
            base_color: Color::srgb(0.7, 0.72, 0.78),
            base_color_texture: Some(lamp_tex.clone()),
            // Emissive is TEXTURED by the same sprite, so only its bright cyan crystal
            // core glows/blooms — the dark metal body stays dark and keeps its detail
            // (a flat emissive washed the whole pylon into a featureless cyan blob).
            emissive_texture: Some(lamp_tex.clone()),
            emissive: LinearRgba::rgb(0.8, 1.2, 1.5),
            perceptual_roughness: 0.9,
            double_sided: true,
            cull_mode: None,
            alpha_mode: AlphaMode::Mask(0.5),
            ..default()
        });
        commands
            .spawn((
                CityScene,
                Transform::from_xyz(s.x, 0.0, s.y),
                Visibility::default(),
            ))
            .with_children(|p| {
                p.spawn((
                    Mesh3d(wa.sprite_quad.clone()),
                    MeshMaterial3d(lmat),
                    Transform::from_xyz(0.0, h * 0.5, 0.0).with_scale(Vec3::splat(h / 2.2)),
                    hd2d::Billboard,
                ));
                p.spawn((
                    MagitechLight { phase: i as f32 * 1.7, base: 32_000.0 },
                    PointLight {
                        color: Color::srgb(0.35, 0.85, 1.15),
                        intensity: 32_000.0,
                        range: 15.0,
                        radius: 0.5,
                        shadows_enabled: false,
                        ..default()
                    },
                    Transform::from_xyz(0.0, h * 0.78, 0.0),
                ));
            });
    }

    // The walkable avatar: the lead hero's sprite, ground-anchored + walk-animated
    // (the same `CharSprite` the overworld uses — `animate_chars` drives it here too).
    let frames = wa.class_frames(session.party.first().map(String::as_str).unwrap_or("explorer"));
    let mat = mats.add(hd2d::sprite_material(
        Color::srgb(1.25, 1.22, 1.12),
        frames.idle[0].clone(),
    ));
    let start = Vec3::new(0.0, 0.0, 11.0);
    commands
        .spawn((
            CityScene,
            CityPlayer,
            Transform::from_translation(start),
            Visibility::default(),
            CharSprite::new(frames.clone(), mat.clone(), start),
        ))
        .with_children(|p| {
            p.spawn((
                Mesh3d(wa.sprite_quad.clone()),
                MeshMaterial3d(mat),
                Transform::from_xyz(0.0, look.sprite_y, 0.0),
                hd2d::Billboard,
                hd2d::HeroBillboard,
            ));
            p.spawn((
                Mesh3d(wa.shadow_mesh.clone()),
                MeshMaterial3d(wa.shadow_mat.clone()),
                Transform::from_xyz(0.0, 0.02, 0.0)
                    .with_rotation(Quat::from_rotation_x(-std::f32::consts::FRAC_PI_2))
                    .with_scale(Vec3::new(1.0, 0.55, 1.0)),
            ));
        });

    // Boss preview (`MELD_BOSS=<key>` / `?boss=<key>`): a towering, ground-anchored
    // boss billboard in the plaza for eyeballing the encounter art. Static south
    // facing; grounded like the pylons (scale = h/2.2, centre at h/2).
    if let Some(key) = crate::flags::boss_preview() {
        if let Some(bf) = wa.boss_frames(&key) {
            let h = 5.0;
            let bmat = mats.add(hd2d::sprite_material(
                Color::srgb(1.1, 1.05, 1.0),
                bf.idle[0].clone(),
            ));
            commands
                .spawn((
                    CityScene,
                    Transform::from_xyz(0.0, 0.0, 3.5),
                    Visibility::default(),
                ))
                .with_children(|p| {
                    p.spawn((
                        Mesh3d(wa.sprite_quad.clone()),
                        MeshMaterial3d(bmat),
                        Transform::from_xyz(0.0, h * 0.5, 0.0).with_scale(Vec3::splat(h / 2.2)),
                        hd2d::Billboard,
                    ));
                    p.spawn((
                        Mesh3d(wa.shadow_mesh.clone()),
                        MeshMaterial3d(wa.shadow_mat.clone()),
                        Transform::from_xyz(0.0, 0.02, 0.0)
                            .with_rotation(Quat::from_rotation_x(-std::f32::consts::FRAC_PI_2))
                            .with_scale(Vec3::splat(h * 0.45)),
                    ));
                });
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn city_input(
    keys: Res<ButtonInput<KeyCode>>,
    net: NonSend<NetRes>,
    autoplay: Res<Autoplay>,
    city_idle: Res<CityIdle>,
    mut session: ResMut<Session>,
    mut city: ResMut<CityUi>,
    mut overlay: ResMut<Overlay>,
    mut tab: ResMut<OverlayTab>,
    mut inv: ResMut<InventoryData>,
    shop: Res<ShopData>,
    mut craft: ResMut<CraftData>,
    mut shop_selling: ResMut<ShopSelling>,
    unlocks: Res<UnlocksRes>,
    mut next: ResMut<NextState<Screen>>,
) {
    // The Drill Yard is modal and full of text fields, so town hotkeys are off while
    // it is open: `T` is a tutorial dive and `Enter` is a dive, and both sit in the
    // middle of the alphabet you type a hero's name out of. Autoplay never opens the
    // yard (`prompt_party_if_unset` skips it), so its dive path is untouched.
    if city.party_open {
        return;
    }
    // Dive: ENTER anywhere, E while standing at The Threshold, or autoplay (which
    // ?city / CityIdle suppresses so the hub can be inspected).
    let at_threshold = city
        .near
        .map_or(false, |i| matches!(CITY_DISTRICTS[i].action, CityAction::Dive));
    let dive = keys.just_pressed(KeyCode::Enter)
        || (keys.just_pressed(KeyCode::KeyE) && at_threshold)
        || (autoplay.0 && !city_idle.0);
    // T = the guided TUTORIAL dive (offered, never forced). A normal dive (ENTER/E/
    // autoplay) is a randomized run, so you don't reappear in the onboarding corridor.
    let tutorial_dive = keys.just_pressed(KeyCode::KeyT);
    if (dive || tutorial_dive) && !session.entered {
        session.entered = true;
        session.coop = false;
        session.status = if tutorial_dive {
            "beginning the guided run...".to_string()
        } else {
            "stepping through The Threshold...".to_string()
        };
        net.0.send(ClientCmd::EnterMaze {
            party: session.party.clone(),
            tutorial: tutorial_dive,
        });
        return;
    }
    if keys.just_pressed(KeyCode::KeyC) {
        // Co-op: open the lobby to create/join a party by code.
        session.coop = true;
        next.set(Screen::Lobby);
        return;
    }
    // The storage chest: open the same tabbed Items/Equip/Status overlay the
    // Overworld uses. Toggling closed just clears `overlay.kind`; opening always
    // lands on Items and kicks off a fresh fetch (vault + persistent hero names,
    // since there's no active run's `PartyRoster` to source names from here).
    if keys.just_pressed(KeyCode::KeyV)
        || (keys.just_pressed(KeyCode::KeyE)
            && city.near.map_or(false, |i| matches!(CITY_DISTRICTS[i].action, CityAction::Vault)))
    {
        if overlay.kind == Some(OverlayKind::Inventory) {
            overlay.kind = None;
        } else {
            overlay.kind = Some(OverlayKind::Inventory);
            *tab = OverlayTab::Items;
            inv.loaded = false;
            net.0.fetch_inventory();
            net.0.fetch_hero_names();
        }
        return;
    }
    // While the Drill Yard is open, [1]-[4] pick a slot and the arrows change its
    // class — only ever among the classes the account actually owns.
    if city.party_open {
        let pool = fieldable_classes(&unlocks);
        let slots = (unlocks.party_slots.max(1) as usize).min(4);
        if session.party.len() < slots {
            session.party.resize(slots, "explorer".to_string());
        }
        session.party.truncate(slots.max(1));
        for (i, key) in [KeyCode::Digit1, KeyCode::Digit2, KeyCode::Digit3, KeyCode::Digit4]
            .iter()
            .enumerate()
        {
            if keys.just_pressed(*key) && i < slots {
                session.party_cursor = i;
            }
        }
        let dir = i32::from(keys.just_pressed(KeyCode::ArrowRight))
            - i32::from(keys.just_pressed(KeyCode::ArrowLeft));
        if dir != 0 && !pool.is_empty() {
            let slot = session.party_cursor.min(session.party.len().saturating_sub(1));
            let cur = session.party.get(slot).cloned().unwrap_or_default();
            let n = pool.len() as i32;
            let at = pool.iter().position(|c| *c == cur).unwrap_or(0) as i32;
            session.party[slot] = pool[(((at + dir) % n + n) % n) as usize].to_string();
            session.party_chosen = true;
        }
        if keys.just_pressed(KeyCode::KeyE) || keys.just_pressed(KeyCode::Escape) {
            city.party_open = false;
            session.party_chosen = true;
            city.notice = "Party set.".to_string();
        }
        return;
    }
    // While the counter is open, [1]-[4] buy an item and [5]-[8] buy a piece of plain
    // gear. The server prices and refuses; the client only names the row.
    if city.shop_open {
        const KEYS: [KeyCode; 8] = [
            KeyCode::Digit1,
            KeyCode::Digit2,
            KeyCode::Digit3,
            KeyCode::Digit4,
            KeyCode::Digit5,
            KeyCode::Digit6,
            KeyCode::Digit7,
            KeyCode::Digit8,
        ];
        // [B] turns the counter around. Buying and selling are the same errand from
        // opposite sides, so they share one button and one set of number keys rather
        // than doubling the key space.
        if keys.just_pressed(KeyCode::KeyB) {
            shop_selling.0 = !shop_selling.0;
            return;
        }
        for (i, key) in KEYS.iter().enumerate() {
            if !keys.just_pressed(*key) {
                continue;
            }
            if shop_selling.0 {
                if let Some((kind, price)) = sellable(&shop, &inv).get(i) {
                    net.0.sell_material(kind.clone(), 1);
                    city.notice = format!("sold 1 {kind} for {price}c");
                }
            } else if i < ITEM_ROWS {
                if let Some(line) = shop.items.get(i) {
                    net.0.buy_item(line.item_kind.clone(), 1);
                    city.notice = format!("bought {}", line.name);
                }
            } else if let Some(g) = shop.gear.get(i - ITEM_ROWS) {
                net.0.buy_gear(g.slot.clone(), g.class_key.clone());
                city.notice = format!("requisitioned {}", g.name);
            }
            return;
        }
    }
    // The Forge & Alembic: ↑/↓ walk the recipe book, ENTER runs the highlighted recipe,
    // [S] cycles which slot the anvil would make, [C] arms a trophy quench, [F] forges.
    // Every refusal comes back from the server in its own words.
    if city.craft_open {
        let n = craft.recipes.len();
        if n > 0 && keys.just_pressed(KeyCode::ArrowDown) {
            craft.cursor = (craft.cursor + 1) % n;
            return;
        }
        if n > 0 && keys.just_pressed(KeyCode::ArrowUp) {
            craft.cursor = (craft.cursor + n - 1) % n;
            return;
        }
        if keys.just_pressed(KeyCode::Enter) {
            if let Some(r) = craft.recipes.get(craft.cursor) {
                net.0.craft(r.recipe.clone());
                craft.last = format!("working {}...", r.name);
            }
            return;
        }
        if keys.just_pressed(KeyCode::KeyS) {
            craft.slot = (craft.slot + 1) % FORGE_SLOTS.len();
            return;
        }
        if keys.just_pressed(KeyCode::KeyC) {
            craft.catalyze = !craft.catalyze;
            return;
        }
        // Left/right walk the bench; [R] and [P] are the smith's two services on
        // whatever is on it. Both go over HTTP against the Vault, so the reply comes
        // back through the same `craft.last` line every other refusal uses.
        let bench_n = inv.gear.len();
        if bench_n > 0 && keys.just_pressed(KeyCode::ArrowRight) {
            craft.bench = (craft.bench + 1) % bench_n;
            return;
        }
        if bench_n > 0 && keys.just_pressed(KeyCode::ArrowLeft) {
            craft.bench = (craft.bench + bench_n - 1) % bench_n;
            return;
        }
        // Both services go over the REALTIME channel rather than straight to HTTP,
        // because smithing is a heat now: the server answers with a bar to strike and
        // grades the blows. The HTTP endpoints stay for API callers.
        if keys.just_pressed(KeyCode::KeyP) {
            match bench_gear(&craft, &inv) {
                Some(g) => {
                    craft.last = format!("heating {}...", g.name);
                    net.0.send(ClientCmd::SmithRequest {
                        entity_id: String::new(),
                        gear_id: g.gear_id.clone(),
                        service: "repair".into(),
                        material: String::new(),
                    });
                }
                None => craft.last = "nothing on the bench".to_string(),
            }
            return;
        }
        if keys.just_pressed(KeyCode::KeyR) {
            let piece = bench_gear(&craft, &inv).map(|g| (g.gear_id.clone(), g.name.clone()));
            match (piece, best_stock(&inv, meld_proto::materials::MaterialClass::Refined)) {
                (Some((gear_id, name)), Some(material)) => {
                    craft.last = format!("heating {name}...");
                    net.0.send(ClientCmd::SmithRequest {
                        entity_id: String::new(),
                        gear_id,
                        service: "reroll".into(),
                        material,
                    });
                }
                (None, _) => craft.last = "nothing on the bench".to_string(),
                (_, None) => {
                    craft.last = "a reroll needs refined stock - smelt an ore first".to_string();
                }
            }
            return;
        }
        if keys.just_pressed(KeyCode::KeyF) {
            // The anvil takes REFINED stock, so pick the best the Vault holds rather
            // than making the player name it; same for the trophy if a quench is armed.
            match best_stock(&inv, meld_proto::materials::MaterialClass::Refined) {
                Some(material) => {
                    let catalyst = craft
                        .catalyze
                        .then(|| best_stock(&inv, meld_proto::materials::MaterialClass::Trophy))
                        .flatten();
                    if craft.catalyze && catalyst.is_none() {
                        craft.last = "no trophy in the Vault to quench it in".to_string();
                    } else {
                        let slot = FORGE_SLOTS[craft.slot];
                        craft.last = format!("forging a {slot}...");
                        net.0.forge(slot.to_string(), material, catalyst);
                    }
                }
                None => {
                    craft.last =
                        "the anvil needs refined stock - smelt an ore first".to_string();
                }
            }
            return;
        }
    }
    // E interacts with whichever other district the avatar is standing in.
    if keys.just_pressed(KeyCode::KeyE) {
        if let Some(i) = city.near {
            match CITY_DISTRICTS[i].action {
                CityAction::Dive | CityAction::Vault => {} // handled above
                CityAction::Party => {
                    city.party_open = true;
                    city.notice.clear();
                    net.0.fetch_hero_names();
                    net.0.fetch_loadouts();
                }
                CityAction::Shop => {
                    city.shop_open = !city.shop_open;
                    if city.shop_open {
                        city.notice.clear();
                        net.0.fetch_shop();
                        // Every half of the counter: the Apothecary's basics, the
                        // Requisition's plain gear, and what the Broker pays for what
                        // you carried home.
                        net.0.fetch_gear_shop();
                        net.0.fetch_broker();
                    }
                }
                CityAction::Craft => {
                    city.craft_open = !city.craft_open;
                    if city.craft_open {
                        city.notice.clear();
                        craft.last.clear();
                        net.0.fetch_recipes();
                    }
                }
                CityAction::Vanguard => {
                    city.board_open = !city.board_open;
                    if city.board_open {
                        city.notice.clear();
                        net.0.fetch_vanguard();
                    }
                }
                CityAction::Notice(s) => city.notice = s.to_string(),
            }
        }
    }
}

/// Walk the avatar around the plaza with WASD/arrows (camera-relative), softly
/// colliding out of building anchors and clamped to the plaza. Client-local — the
/// city has no server-side simulation (see docs/proposals/last-city.md).
pub(crate) fn city_move(
    time: Res<Time>,
    keys: Res<ButtonInput<KeyCode>>,
    look: Res<hd2d::Look>,
    session: Res<Session>,
    city: Res<CityUi>,
    mut q: Query<&mut Transform, With<CityPlayer>>,
) {
    let Ok(mut tf) = q.single_mut() else { return };
    if session.entered {
        return; // stepping through The Threshold — stop walking
    }
    // WASD is also four letters of a hero's name, so the yard holds you still.
    if city.party_open {
        return;
    }
    // Camera-relative planar basis (at yaw 0 the camera looks toward -z).
    let yaw = look.cam_yaw.to_radians();
    let fwd = Vec2::new(-yaw.sin(), -yaw.cos()); // W = into the screen
    // D = screen-right. The camera's right is `fwd` rotated +90° in the xz plane
    // (at yaw 0: fwd=(0,-1) → right=(1,0)=+x=east). The previous `(fwd.y,-fwd.x)`
    // gave the opposite (-x), so A/D — and the walk-facing derived from motion —
    // came out mirrored in town.
    let right = Vec2::new(-fwd.y, fwd.x);
    let mut m = Vec2::ZERO;
    if keys.pressed(KeyCode::KeyW) || keys.pressed(KeyCode::ArrowUp) {
        m += fwd;
    }
    if keys.pressed(KeyCode::KeyS) || keys.pressed(KeyCode::ArrowDown) {
        m -= fwd;
    }
    if keys.pressed(KeyCode::KeyD) || keys.pressed(KeyCode::ArrowRight) {
        m += right;
    }
    if keys.pressed(KeyCode::KeyA) || keys.pressed(KeyCode::ArrowLeft) {
        m -= right;
    }
    if m.length_squared() <= 1e-6 {
        return;
    }
    let step = m.normalize() * 9.0 * time.delta_secs();
    let mut pos = Vec2::new(tf.translation.x + step.x, tf.translation.z + step.y);
    // Soft-collide out of each building anchor.
    for d in CITY_DISTRICTS {
        let c = Vec2::new(d.x, d.z);
        let off = pos - c;
        let block = 2.4;
        if off.length() < block && off.length() > 1e-4 {
            pos = c + off.normalize() * block;
        }
    }
    // Keep within the plaza bounds.
    let center = Vec2::new(0.0, -6.0);
    let rel = pos - center;
    if rel.length() > 25.0 {
        pos = center + rel.normalize() * 25.0;
    }
    tf.translation.x = pos.x;
    tf.translation.z = pos.y;
}

/// Orbit-follow the avatar with the HD-2D camera (mirrors `hd2d_follow`).
#[allow(clippy::type_complexity)]
pub(crate) fn city_camera(
    look: Res<hd2d::Look>,
    time: Res<Time>,
    players: Query<&Transform, (With<CityPlayer>, Without<Camera3d>)>,
    mut cam_q: Query<
        (
            &mut Transform,
            &mut Projection,
            Option<&mut bevy::core_pipeline::bloom::Bloom>,
            Option<&mut bevy::core_pipeline::dof::DepthOfField>,
            Option<&mut bevy::pbr::DistanceFog>,
        ),
        With<Camera3d>,
    >,
) {
    let Ok(p) = players.single() else { return };
    let target = Vec3::new(p.translation.x, 1.0, p.translation.z);
    if let Ok((mut t, mut proj, bloom, dof, fog)) = cam_q.single_mut() {
        *t = hd2d::camera_transform(&look, target, time.elapsed_secs());
        hd2d::apply_post(
            &look,
            &mut proj,
            bloom.map(|b| b.into_inner()),
            dof.map(|d| d.into_inner()),
            fog.map(|f| f.into_inner()),
        );
    }
}

/// Track which district the avatar is standing in (nearest whose radius contains
/// it), for the contextual interact prompt.
pub(crate) fn city_interact(players: Query<&Transform, With<CityPlayer>>, mut city: ResMut<CityUi>) {
    let Ok(p) = players.single() else {
        city.near = None;
        return;
    };
    let pos = Vec2::new(p.translation.x, p.translation.z);
    let mut best: Option<(usize, f32)> = None;
    for (i, d) in CITY_DISTRICTS.iter().enumerate() {
        let dist = pos.distance(Vec2::new(d.x, d.z));
        if dist <= d.radius && best.map_or(true, |(_, b)| dist < b) {
            best = Some((i, dist));
        }
    }
    let near = best.map(|(i, _)| i);
    if city.shop_open
        && !crate::flags::shop_preview_flag()
        && !near.map_or(false, |i| matches!(CITY_DISTRICTS[i].action, CityAction::Shop))
    {
        city.shop_open = false;
    }
    if city.board_open
        && !crate::flags::wall_preview_flag()
        && !near.map_or(false, |i| matches!(CITY_DISTRICTS[i].action, CityAction::Vanguard))
    {
        city.board_open = false;
    }
    city.near = near;
}

pub(crate) fn render_city(
    inv: Res<InventoryData>,
    session: Res<Session>,
    city: Res<CityUi>,
    board: Res<VanguardBoardData>,
    shop_selling: Res<ShopSelling>,
    craft: Res<CraftData>,
    shop: Res<ShopData>,
    unlocks: Res<UnlocksRes>,
    hero_names: Res<AccountHeroNames>,
    heat: Res<crate::overworld::HeatUi>,
    time: Res<Time>,
    mut q_vault: Query<&mut Text, (With<CityVaultText>, Without<CityStatusText>)>,
    mut q_status: Query<&mut Text, With<CityStatusText>>,
) {
    if let Ok(mut t) = q_vault.single_mut() {
        **t = city_vault_text(&inv);
    }
    if let Ok(mut t) = q_status.single_mut() {
        let prompt = if !session.status.is_empty() {
            session.status.clone()
        } else if let Some(i) = city.near {
            let d = &CITY_DISTRICTS[i];
            match d.action {
                CityAction::Dive => format!("{}    [E]/[ENTER] step onto the plane", d.label),
                CityAction::Vault => format!("{}    [E] open your storage chest", d.label),
                CityAction::Shop => format!("{}    [E] browse the Apothecary", d.label),
                CityAction::Craft => format!("{}    [E] work the recipes and the anvil", d.label),
                CityAction::Vanguard => format!("{}    [E] read the season's board", d.label),
                CityAction::Party => format!("{}    [E] muster your party", d.label),
                CityAction::Notice(_) => format!("{}    [E] inspect", d.label),
            }
        } else {
            "WASD move    [E] enter a district    [ENTER] run    [T] tutorial    [C] co-op    [V] storage chest"
                .to_string()
        };
        // A heat in progress owns the panel: the player is mid-blow at the anvil.
        **t = if let Some(bar) =
            crate::overworld::heat_line(&heat, time.elapsed_secs_f64())
        {
            format!("{}\n{}", craft_text(&craft, &inv), bar)
        } else if city.craft_open {
            format!("{}\n{prompt}", craft_text(&craft, &inv))
        } else if city.shop_open {
            format!("{}\n{prompt}", shop_text(&shop, &inv, shop_selling.0))
        } else if city.board_open {
            format!("{}\n{prompt}", vanguard_wall_text(&board))
        } else if city.notice.is_empty() {
            prompt
        } else {
            format!("{}\n{prompt}", city.notice)
        };
    }
}

/// Compose the Vault-Deep's ambient one-line summary from the live `GET
/// /v1/vault` data (the full material/gear/withdraw view lives in the tabbed
/// inventory overlay now — `V`/`E` opens that instead of expanding this text).
pub(crate) fn city_vault_text(inv: &InventoryData) -> String {
    if !inv.loaded {
        return "The Vault-Deep is being tallied...".to_string();
    }
    let mat_count: i32 = inv.materials.iter().map(|(_, n)| *n).sum();
    format!(
        "The Vault-Deep:  {} chits    {} materials    {} gear     [V] open",
        inv.chits,
        mat_count,
        inv.gear.len()
    )
}

/// The lit Vanguard Wall: the season's deepest dives, best first, with the
/// reader's own placement called out (P1-1 — behaviors/endgame-seasons.md).
/// Trimmed to the top few rows because the wall shares the city's one status
/// line; the full 100 belongs to `AD-6`'s board screen.
pub(crate) fn vanguard_wall_text(board: &VanguardBoardData) -> String {
    if !board.loaded {
        return "The Vanguard Wall flickers awake...".to_string();
    }
    if board.entries.is_empty() {
        return format!(
            "The Vanguard Wall, season {}:  no name carved yet - the first to walk out and come back deep takes it.",
            board.season
        );
    }
    let rows: Vec<String> = board
        .entries
        .iter()
        .take(5)
        .map(|e| format!("{}. {} - d{}", e.rank, e.username, e.max_distance))
        .collect();
    let you = match board.you {
        Some(rank) => format!("    (you: #{rank})"),
        None => "    (you: uncarved)".to_string(),
    };
    format!(
        "The Vanguard Wall, season {}:  {}{you}",
        board.season,
        rows.join("    ")
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use meld_client::net::VanguardLine;

    fn line(rank: i32, name: &str, d: i32) -> VanguardLine {
        VanguardLine { rank, username: name.to_string(), max_distance: d }
    }

    #[test]
    fn wall_text_covers_flickering_empty_and_ranked() {
        let mut board = VanguardBoardData::default();
        assert!(vanguard_wall_text(&board).contains("flickers awake"));

        board.loaded = true;
        board.season = 2;
        let empty = vanguard_wall_text(&board);
        assert!(empty.contains("season 2"), "{empty}");
        assert!(empty.contains("no name carved"), "{empty}");

        board.entries = (1..=8).map(|i| line(i, &format!("digger{i}"), 900 - i * 10)).collect();
        board.you = Some(4);
        let lit = vanguard_wall_text(&board);
        assert!(lit.contains("1. digger1 - d890"), "{lit}");
        // Only the top five share the city's one status line.
        assert!(lit.contains("5. digger5"), "{lit}");
        assert!(!lit.contains("6. digger6"), "{lit}");
        assert!(lit.contains("(you: #4)"), "{lit}");

        board.you = None;
        assert!(vanguard_wall_text(&board).contains("uncarved"));
    }
}

/// The Apothecary's shelf as the city's one status line can carry it: name, price,
/// and what the player can currently afford (EC-2). Buying is `[1]`-`[4]`.
pub(crate) fn shop_text(shop: &ShopData, inv: &InventoryData, selling: bool) -> String {
    if !shop.loaded {
        return "The Apothecary is unpacking crates...".to_string();
    }
    if selling {
        // The SELL side: what the Broker pays for what you carried home. Priced as a
        // floor, so this is the answer to "I will never use this" rather than a living.
        let rows = sellable(shop, inv);
        if rows.is_empty() {
            return format!(
                "The Broker - {} chits    nothing in the Vault it wants    [B] buy instead",
                inv.chits
            );
        }
        let held = |kind: &str| -> i32 {
            inv.materials.iter().find(|(k, _)| k == kind).map_or(0, |(_, q)| *q)
        };
        let listed: Vec<String> = rows
            .iter()
            .take(ITEM_ROWS + GEAR_ROWS)
            .enumerate()
            .map(|(i, (kind, price))| {
                format!("[{}] {kind} x{} @{price}c", i + 1, held(kind))
            })
            .collect();
        return format!(
            "The Broker - {} chits    {}    [B] buy instead",
            inv.chits,
            listed.join("   ")
        );
    }
    if shop.items.is_empty() {
        return "The Apothecary has nothing on the shelf.".to_string();
    }
    // Mark what the player cannot afford, so a price is a decision rather than a
    // rejection they discover by pressing a key.
    let afford = |price: i64| if inv.chits >= price { "" } else { " (short)" };
    let rows: Vec<String> = shop
        .items
        .iter()
        .take(4)
        .enumerate()
        .map(|(i, s)| format!("[{}] {} {}c{}", i + 1, s.name, s.price_chits, afford(s.price_chits)))
        .collect();
    // The Requisition's plain gear shares the counter, on the keys after the items:
    // "spend chits so the next dive is easier" is one errand, not two.
    let gear: Vec<String> = shop
        .gear
        .iter()
        .take(GEAR_ROWS)
        .enumerate()
        .map(|(i, g)| {
            let stat = [("atk", g.atk), ("def", g.def), ("spd", g.spd)]
                .into_iter()
                .find(|(_, v)| *v > 0)
                .map(|(n, v)| format!(" +{v} {n}"))
                .unwrap_or_default();
            format!(
                "[{}] {}{} {}c{}",
                ITEM_ROWS + i + 1,
                g.name,
                stat,
                g.price_chits,
                afford(g.price_chits)
            )
        })
        .collect();
    let mut line = format!("{} - {} chits    {}", shop.vendor.clone(), inv.chits, rows.join("   "));
    if !gear.is_empty() {
        line.push_str("    |  Requisition: ");
        line.push_str(&gear.join("   "));
    }
    line.push_str("    [B] sell");
    line
}

/// The materials the counter will buy that the player actually holds, richest first.
///
/// Intersecting the Broker's quotes with the Vault is the whole list: a price for
/// something you do not carry is noise, and the most valuable stack is the one you came
/// to sell.
pub(crate) fn sellable(shop: &ShopData, inv: &InventoryData) -> Vec<(String, i64)> {
    let mut rows: Vec<(String, i64)> = inv
        .materials
        .iter()
        .filter(|(_, qty)| *qty > 0)
        .filter_map(|(kind, _)| {
            shop.quotes
                .iter()
                .find(|q| &q.item_kind == kind)
                .map(|q| (kind.clone(), q.price_chits))
        })
        .collect();
    rows.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    rows
}

/// The deepest-band material of `class` the Vault holds, or `None`.
///
/// The Forge asks for a material by name, and making the player type one would be
/// absurd — so the panel spends the best thing available, which is also what a smith
/// would reach for.
pub(crate) fn best_stock(
    inv: &InventoryData,
    class: meld_proto::materials::MaterialClass,
) -> Option<String> {
    inv.materials
        .iter()
        .filter(|(kind, qty)| *qty > 0 && meld_proto::materials::is_class(kind, class))
        .filter_map(|(kind, _)| {
            meld_proto::materials::material(kind).map(|m| (m.tier, kind.clone()))
        })
        .max_by_key(|(tier, _)| *tier)
        .map(|(_, kind)| kind)
}

/// The Forge & Alembic as the city's status block: the recipe book with the cursor on
/// one row, then the anvil's own line. The server owns every gate, so a locked row says
/// the level it wants and an unaffordable one says what it is missing — before a
/// keypress is spent on it.
pub(crate) fn craft_text(craft: &CraftData, inv: &InventoryData) -> String {
    if !craft.loaded {
        return "The Forge & Alembic are warming up...".to_string();
    }
    if craft.recipes.is_empty() {
        return "No recipes known.".to_string();
    }
    let held = |kind: &str| -> i32 {
        inv.materials.iter().find(|(k, _)| k == kind).map_or(0, |(_, q)| *q)
    };
    let mut out = String::new();
    // A window around the cursor, so a long book still fits the city's status block.
    let n = craft.recipes.len();
    let start = craft.cursor.saturating_sub(1).min(n.saturating_sub(CRAFT_ROWS));
    for (i, r) in craft.recipes.iter().enumerate().skip(start).take(CRAFT_ROWS) {
        let inputs: Vec<String> = r
            .inputs
            .iter()
            .map(|(kind, need)| {
                let have = held(kind);
                // Show have/need per input: "1/2 dune_iron" is the whole answer to
                // "what am I missing", and the reason a craft is greyed out.
                format!("{have}/{need} {kind}")
            })
            .collect();
        let gate = if !r.craftable {
            format!("  (needs {} {})", r.skill, r.required_level)
        } else if r.inputs.iter().any(|(kind, need)| held(kind) < *need) {
            "  (short)".to_string()
        } else {
            String::new()
        };
        let cursor = if i == craft.cursor { ">" } else { " " };
        out.push_str(&format!(
            "{cursor} {} x{}  <- {}{gate}
",
            r.name,
            r.output_quantity,
            inputs.join(" + ")
        ));
    }
    let stock = best_stock(inv, meld_proto::materials::MaterialClass::Refined);
    let anvil = match &stock {
        Some(m) => m.as_str(),
        None => "nothing refined",
    };
    let quench = if craft.catalyze { "on" } else { "off" };
    out.push_str(&format!(
        "  ANVIL  [S] slot: {}   [C] quench: {quench}   [F] forge from {anvil}
",
        FORGE_SLOTS[craft.slot]
    ));
    out.push_str(&bench_line(craft, inv));
    out.push_str("  up/down choose   ENTER craft");
    if !craft.last.is_empty() {
        out.push_str(&format!("
  {}", craft.last));
    }
    out
}

/// The smith's other half: the two things they do to a piece you already own —
/// another draw on its affixes, and durability bought back. Both need a CHOSEN piece,
/// so the anvil keeps one on the bench and left/right walk the Vault.
pub(crate) fn bench_line(craft: &CraftData, inv: &InventoryData) -> String {
    let Some(g) = bench_gear(craft, inv) else {
        return "  BENCH  nothing in the Vault to work on\n".to_string();
    };
    // Only advertise the service the piece can actually take: repair buys back the
    // max durability a death chewed off, which only INSURED gear ever loses, and a
    // reroll on ephemeral gear would burn with it on the walk home. Offering a key
    // that is certain to be refused is worse than not offering it.
    let ins = meld_proto::enums::Insurance::from_wire(&g.insurance);
    let mut keys = Vec::new();
    if ins != Some(meld_proto::enums::Insurance::Ephemeral) {
        keys.push(format!("[R] reroll ({} stock)", g.reroll_cost));
    }
    if ins == Some(meld_proto::enums::Insurance::Insured) {
        keys.push("[P] repair".to_string());
    }
    let offer = if keys.is_empty() {
        "nothing a smith can do with this".to_string()
    } else {
        keys.join("   ")
    };
    format!(
        "  BENCH  <-/-> {} T{} {}  ({}/{} dur, {} affix)   {offer}\n",
        g.name,
        g.tier,
        ins.map(|i| i.label()).unwrap_or("?"),
        g.max_durability,
        g.base_max_durability,
        g.affixes.len()
    )
}

/// The piece the bench cursor sits on, or None when the Vault is empty.
pub(crate) fn bench_gear<'a>(craft: &CraftData, inv: &'a InventoryData) -> Option<&'a GearLine> {
    if inv.gear.is_empty() {
        return None;
    }
    inv.gear.get(craft.bench % inv.gear.len())
}

/// Recipe rows the book shows at once, so the book fits the city's status block.
pub(crate) const CRAFT_ROWS: usize = 5;

/// Shelf rows the counter shows: items on `[1]`-`[4]`, plain gear on the keys after.
pub(crate) const ITEM_ROWS: usize = 4;
pub(crate) const GEAR_ROWS: usize = 4;

#[cfg(test)]
mod shop_tests {
    use super::*;

    fn line(kind: &str, name: &str, price: i64) -> meld_client::net::ShopLine {
        meld_client::net::ShopLine {
            item_kind: kind.into(),
            name: name.into(),
            description: "…".into(),
            price_chits: price,
        }
    }

    #[test]
    fn the_shelf_prices_every_row_and_flags_what_you_cannot_afford() {
        let mut shop = ShopData::default();
        let mut inv = InventoryData::default();
        assert!(shop_text(&shop, &inv, false).contains("unpacking"));

        shop.loaded = true;
        assert!(shop_text(&shop, &inv, false).contains("nothing on the shelf"));

        shop.vendor = "The Apothecary".into();
        shop.items = vec![line("bloom_salve", "Bloom Salve", 25), line("town_portal", "Town Portal", 60)];
        inv.chits = 30;
        let text = shop_text(&shop, &inv, false);
        assert!(text.contains("The Apothecary"), "{text}");
        assert!(text.contains("[1] Bloom Salve 25c"), "{text}");
        // 30 chits buys the salve but not the portal, and the row says so BEFORE the
        // player spends a keypress on it.
        assert!(!text.contains("Bloom Salve 25c (short)"), "{text}");
        assert!(text.contains("Town Portal 60c (short)"), "{text}");
        assert!(text.contains("30 chits"), "{text}");
    }

    fn recipe(name: &str, level: i32, have: bool, inputs: &[(&str, i32)]) -> meld_client::net::RecipeLine {
        meld_client::net::RecipeLine {
            recipe: name.to_lowercase().replace(' ', "_"),
            name: name.into(),
            skill: "alchemy".into(),
            required_level: level,
            skill_level: if have { level } else { 1 },
            craftable: have,
            output_quantity: 1,
            inputs: inputs.iter().map(|(k, q)| ((*k).to_string(), *q)).collect(),
        }
    }

    // The book has to answer "why can't I make this" on the row itself — a level it
    // wants, or the exact material it is short of — because the alternative is a player
    // pressing ENTER to find out.
    #[test]
    fn the_recipe_book_says_what_each_row_needs() {
        let mut craft = CraftData::default();
        let mut inv = InventoryData::default();
        assert!(craft_text(&craft, &inv).contains("warming up"));

        craft.loaded = true;
        assert!(craft_text(&craft, &inv).contains("No recipes known"));

        craft.recipes = vec![
            recipe("Bloom Salve", 1, true, &[("bloom_herb", 2)]),
            recipe("Quintessence", 9, false, &[("bog_ichor", 1)]),
        ];
        inv.materials = vec![("bloom_herb".to_string(), 1)];
        let text = craft_text(&craft, &inv);
        // Have/need per input is the whole answer to "what am I missing".
        assert!(text.contains("1/2 bloom_herb"), "{text}");
        assert!(text.contains("(short)"), "{text}");
        // A locked row names the level rather than just refusing later.
        assert!(text.contains("needs alchemy 9"), "{text}");
        // The cursor is visible, and moves.
        assert!(text.starts_with("> Bloom Salve"), "{text}");
        craft.cursor = 1;
        assert!(craft_text(&craft, &inv).contains("> Quintessence"), "{text}");

        // Enough material and the row stops complaining.
        inv.materials = vec![("bloom_herb".to_string(), 5)];
        let text = craft_text(&craft, &inv);
        assert!(text.contains("5/2 bloom_herb"), "{text}");
        assert!(!text.contains("Bloom Salve x1  <- 5/2 bloom_herb  (short)"), "{text}");
    }

    // The anvil spends the best refined stock in the Vault rather than making anyone
    // type a material name — and says so, including when there is none.
    #[test]
    fn the_anvil_reaches_for_the_deepest_stock_it_has() {
        use meld_proto::materials::MaterialClass;
        let mut inv = InventoryData::default();
        assert_eq!(best_stock(&inv, MaterialClass::Refined), None);
        inv.materials = vec![
            ("dune_ingot".to_string(), 4),   // tier 1
            ("peat_ingot".to_string(), 2),   // tier 4 — the best
            ("bloom_herb".to_string(), 9),   // not refined at all
            ("cinder_ore".to_string(), 9),   // raw ore, not stock
        ];
        assert_eq!(best_stock(&inv, MaterialClass::Refined).as_deref(), Some("peat_ingot"));
        // A stack of zero is not stock.
        inv.materials = vec![("peat_ingot".to_string(), 0)];
        assert_eq!(best_stock(&inv, MaterialClass::Refined), None);

        let mut craft = CraftData::default();
        craft.loaded = true;
        craft.recipes = vec![recipe("Bloom Salve", 1, true, &[("bloom_herb", 2)])];
        let text = craft_text(&craft, &inv);
        assert!(text.contains("nothing refined"), "the anvil should say it is empty: {text}");
        assert!(text.contains("[S] slot: main_hand"), "{text}");
        assert!(text.contains("[C] quench: off"), "{text}");
    }

    // Selling is the same counter turned around: only what you HOLD and it WANTS, the
    // richest stack first (that is the one you came to sell), and it says plainly when
    // there is nothing it will take.
    #[test]
    fn the_counter_turns_around_and_buys_what_you_carried_home() {
        let mut shop = ShopData::default();
        let mut inv = InventoryData::default();
        shop.loaded = true;
        shop.vendor = "The Apothecary".into();
        shop.quotes = vec![
            meld_client::net::BrokerQuote {
                item_kind: "bloom_herb".into(),
                name: "Bloom Herb".into(),
                price_chits: 5,
            },
            meld_client::net::BrokerQuote {
                item_kind: "bog_ichor".into(),
                name: "Bog Ichor".into(),
                price_chits: 66,
            },
        ];

        // Nothing in the Vault → it says so rather than showing an empty list.
        let empty = shop_text(&shop, &inv, true);
        assert!(empty.contains("nothing in the Vault it wants"), "{empty}");

        inv.chits = 12;
        inv.materials = vec![
            ("bloom_herb".to_string(), 4),
            ("bog_ichor".to_string(), 2),
            ("mystery_rock".to_string(), 9), // not a material the Broker quotes
        ];
        let text = shop_text(&shop, &inv, true);
        assert!(text.contains("The Broker"), "{text}");
        // Richest first, with the stack you hold and the price each.
        assert!(text.contains("[1] bog_ichor x2 @66c"), "{text}");
        assert!(text.contains("[2] bloom_herb x4 @5c"), "{text}");
        // A price for something you do not carry is noise.
        assert!(!text.contains("mystery_rock"), "{text}");
        // And the way back is on the row.
        assert!(text.contains("[B] buy instead"), "{text}");
        assert!(sellable(&shop, &inv).len() == 2);

        // The buy side advertises the other half too.
        shop.items = vec![line("bloom_salve", "Bloom Salve", 25)];
        assert!(shop_text(&shop, &inv, false).contains("[B] sell"));
    }

    fn bench_piece(id: &str, name: &str, dur: i32, base: i32) -> GearLine {
        bench_piece_of("insured", 1, id, name, dur, base)
    }

    fn bench_piece_of(
        insurance: &str,
        tier: i32,
        id: &str,
        name: &str,
        dur: i32,
        base: i32,
    ) -> GearLine {
        GearLine {
            gear_id: id.into(),
            name: name.into(),
            slot: "main_hand".into(),
            class_key: String::new(),
            insurance: insurance.into(),
            family: String::new(),
            armor_weight: String::new(),
            affixes: Vec::new(),
            unique_key: String::new(),
            set_key: String::new(),
            tier,
            equipped_hero_slot: None,
            max_durability: dur,
            base_max_durability: base,
            atk_bonus: 4,
            def_bonus: 0,
            spd_bonus: 0,
            reroll_cost: 3 + 2 * tier,
        }
    }

    #[test]
    fn the_bench_names_the_piece_it_would_work_on_and_wraps_around_the_vault() {
        let mut craft = CraftData { loaded: true, ..Default::default() };
        craft.recipes = vec![recipe("Bloom Salve", 1, true, &[("bloom_herb", 2)])];
        let mut inv = InventoryData::default();

        // An empty Vault says so, rather than offering services on nothing.
        assert!(craft_text(&craft, &inv).contains("nothing in the Vault"));
        assert!(bench_gear(&craft, &inv).is_none());

        inv.gear = vec![
            bench_piece("g1", "Worn Warblade", 6, 10),
            bench_piece("g2", "Issued Cuirass", 10, 10),
        ];
        let text = craft_text(&craft, &inv);
        // Both services are advertised, and the piece's state is the reason to use them.
        assert!(text.contains("Worn Warblade"), "{text}");
        assert!(text.contains("6/10 dur"), "{text}");
        assert!(text.contains("[R] reroll"), "{text}");
        assert!(text.contains("[P] repair"), "{text}");
        // The reroll names the stock it eats, which is the server's number for THIS
        // piece's tier — a deep item costs more to re-draw than a starter blade.
        assert!(text.contains("[R] reroll (5 stock)"), "{text}");

        // The cursor is taken modulo the Vault, so a stale index left by a smaller
        // Vault (a piece sold, or lost on a death) can never index out of range.
        craft.bench = 1;
        assert_eq!(bench_gear(&craft, &inv).unwrap().name, "Issued Cuirass");
        craft.bench = 6;
        assert_eq!(bench_gear(&craft, &inv).unwrap().name, "Worn Warblade");
    }

    // A smith's two services do not apply to every tier, and a key that is certain to
    // be refused is worse than no key at all. Repair buys back max durability, which
    // only INSURED gear ever loses; a reroll on ephemeral gear would burn with it on
    // the walk home.
    #[test]
    fn the_bench_offers_only_the_service_the_tier_can_take() {
        let craft = CraftData { loaded: true, recipes: vec![], ..Default::default() };
        let mut inv = InventoryData::default();

        inv.gear = vec![bench_piece_of("insured", 2, "g", "Wearing Blade", 8, 12)];
        let insured = bench_line(&craft, &inv);
        assert!(insured.contains("Insured"), "{insured}");
        assert!(insured.contains("[R] reroll (7 stock)"), "{insured}");
        assert!(insured.contains("[P] repair"), "{insured}");

        // Standard never degrades, so there is nothing to mend — but it is yours, so
        // it is worth re-drawing.
        inv.gear = vec![bench_piece_of("standard", 0, "g", "Issued Blade", 20, 20)];
        let standard = bench_line(&craft, &inv);
        assert!(standard.contains("[R] reroll (3 stock)"), "{standard}");
        assert!(!standard.contains("[P] repair"), "{standard}");

        // Ephemeral burns on the walk home: neither service is worth a chit.
        inv.gear = vec![bench_piece_of("ephemeral", 4, "g", "Cinderglass Edge", 30, 30)];
        let ephemeral = bench_line(&craft, &inv);
        assert!(!ephemeral.contains("[R] reroll"), "{ephemeral}");
        assert!(!ephemeral.contains("[P] repair"), "{ephemeral}");
        assert!(ephemeral.contains("nothing a smith can do"), "{ephemeral}");
    }

    #[test]
    fn the_counter_stocks_plain_gear_on_the_keys_after_the_items() {
        let mut shop = ShopData::default();
        let mut inv = InventoryData::default();
        shop.loaded = true;
        shop.vendor = "The Apothecary".into();
        shop.items = vec![line("bloom_salve", "Bloom Salve", 25)];
        shop.gear = vec![meld_client::net::GearShopLine {
            slot: "main_hand".into(),
            class_key: "explorer".into(),
            name: "Issued Warblade".into(),
            price_chits: 220,
            atk: 3,
            def: 0,
            spd: 0,
        }];
        inv.chits = 300;
        let text = shop_text(&shop, &inv, false);
        // Items keep [1]-[4]; gear starts on the key after them, so a row's number
        // never moves when the shelf is short.
        assert!(text.contains("[1] Bloom Salve"), "{text}");
        assert!(text.contains("[5] Issued Warblade"), "{text}");
        // What the piece DOES is on the row — a price with no stat is not a decision.
        assert!(text.contains("+3 atk"), "{text}");
        assert!(text.contains("Requisition"), "{text}");
        assert!(!text.contains("Warblade +3 atk 220c (short)"), "300 chits covers it: {text}");

        // …and when it does not, the row says so before a keypress is spent.
        inv.chits = 10;
        assert!(shop_text(&shop, &inv, false).contains("220c (short)"));
    }
}

/// Seed the party from what the account last took down, filtered to what it owns.
///
/// The composition is already persisted per hero slot (`heroes.class_key`, GR-7) and
/// already arrives on `/v1/heroes` — it was simply never read back, so every session
/// rebuilt the party from scratch. Reusing it is what lets town skip the picker for a
/// returning player and only prompt someone who has never chosen.
///
/// Filtered because a class can be persisted and later be un-fieldable: a slot that
/// dived as a Hunter on an account that has since been reset to one party slot must
/// not silently re-enter as one — the server would clamp it and the two would disagree.
pub(crate) fn seed_party_from_account(
    hero_names: Res<AccountHeroNames>,
    unlocks: Res<UnlocksRes>,
    mut session: ResMut<Session>,
    mut done: Local<bool>,
) {
    // Runs every frame in town until the async `/v1/heroes` fetch lands, then once.
    if *done || session.party_from_flags {
        return;
    }
    if !hero_names.loaded {
        return;
    }
    *done = true;
    let owned: Vec<String> = if unlocks.owned.is_empty() {
        vec!["explorer".to_string()]
    } else {
        unlocks
            .owned
            .iter()
            .filter_map(|k| k.strip_prefix("class_"))
            .map(str::to_string)
            .collect()
    };
    let slots = (unlocks.party_slots.max(1) as usize).min(4);
    let saved: Vec<String> = hero_names
        .classes
        .iter()
        .take(slots)
        .map(|c| {
            if owned.iter().any(|o| o == c) {
                c.clone()
            } else {
                "explorer".to_string()
            }
        })
        .collect();
    if !saved.is_empty() && saved.iter().any(|c| !c.is_empty()) {
        session.party = saved;
        session.party_chosen = true;
    }
}

/// Open the Drill Yard by itself for anyone who has never picked a party.
///
/// A returning player is NOT re-asked — their composition is already persisted and
/// seeded by [`seed_party_from_account`]. This is only the first trip to town, so the
/// first dive is a team someone chose rather than the newcomer default.
pub(crate) fn prompt_party_if_unset(
    hero_names: Res<AccountHeroNames>,
    autoplay: Res<Autoplay>,
    session: Res<Session>,
    mut city: ResMut<CityUi>,
    mut asked: Local<bool>,
) {
    // Wait for the roster fetch, or a brand-new account looks unset and gets asked
    // before the answer has even arrived.
    if *asked || !hero_names.loaded || autoplay.0 || session.party_from_flags {
        return;
    }
    *asked = true;
    if !session.party_chosen {
        city.party_open = true;
        city.notice = "Muster a party before you dive.".to_string();
    }
}

/// The classes this account may actually field, in a stable order.
///
/// Derived from the CL-1 unlock set rather than the full class list, which is the
/// whole reason party selection belongs in town: the Join screen runs BEFORE login,
/// so it cannot know what the account owns and can only offer all six and let the
/// server clamp the answer afterwards.
pub(crate) fn fieldable_classes(unlocks: &UnlocksRes) -> Vec<&'static str> {
    let owned: Vec<&str> =
        unlocks.owned.iter().filter_map(|k| k.strip_prefix("class_")).collect();
    let mut out: Vec<&'static str> =
        PARTY_CLASSES.iter().copied().filter(|c| owned.contains(c)).collect();
    if out.is_empty() {
        out.push("explorer"); // every account owns the Explorer from its first login
    }
    out
}


/// Marks the Drill Yard panel root, so it can be despawned when the yard closes.
#[derive(Component)]
pub(crate) struct PartyPanelRoot;

/// A clickable party slot (index) in the Drill Yard.
#[derive(Component)]
pub(crate) struct PartySlotButton(pub usize);

/// A clickable class chip in the Drill Yard.
#[derive(Component)]
pub(crate) struct PartyClassButton(pub &'static str);

/// Closes the yard.
#[derive(Component)]
pub(crate) struct PartyDoneButton;

/// Load a saved composition into the live party.
#[derive(Component)]
pub(crate) struct LoadoutLoadButton(pub String);

/// Forget a saved composition.
#[derive(Component)]
pub(crate) struct LoadoutDeleteButton(pub String);

/// Save the current party under the typed name.
#[derive(Component)]
pub(crate) struct LoadoutSaveButton;

/// The editable name field for the next save.
#[derive(Component)]
pub(crate) struct LoadoutNameText;

/// The label inside a slot button (kept in sync by [`party_panel_refresh`]).
#[derive(Component)]
pub(crate) struct PartySlotLabel(pub usize);

/// The portrait on a party slot card.
#[derive(Component)]
pub(crate) struct PartySlotSprite(pub usize);

/// A slot card's hero name — the editable one, not the class.
#[derive(Component)]
pub(crate) struct PartySlotHeroName(pub usize);

/// The portrait on a palette class card.
#[derive(Component)]
pub(crate) struct PartyClassSprite(pub &'static str);

/// The detail panel's parts, filled from [`CityUi::yard_focus`].
#[derive(Component)]
pub(crate) struct YardDetailSprite;
#[derive(Component)]
pub(crate) struct YardDetailName;
#[derive(Component)]
pub(crate) struct YardDetailRole;
#[derive(Component)]
pub(crate) struct YardDetailKit;

/// One segment of one 0..5 stat bar in the detail panel.
#[derive(Component)]
pub(crate) struct YardStatFill {
    pub stat: u8,
    pub seg: u8,
}

/// Click to start renaming the focused hero.
#[derive(Component)]
pub(crate) struct YardRenameButton;

/// The rename line's editable text.
#[derive(Component)]
pub(crate) struct YardRenameText;

/// One framed card: a portrait over a label, optionally with a second line under it.
/// Shared by the four party slots and the class palette, so a hero you have and a
/// class you could field are literally the same object at two sizes.
#[allow(clippy::too_many_arguments)]
fn yard_card(
    parent: &mut ChildSpawnerCommands,
    sprite: Handle<Image>,
    label: &str,
    sub: &str,
    w: f32,
    tags: impl Bundle,
    sprite_tag: impl Bundle,
    label_tag: impl Bundle,
    sub_tag: impl Bundle,
) {
    parent
        .spawn((
            Button,
            tags,
            Node {
                width: Val::Px(w),
                flex_direction: FlexDirection::Column,
                align_items: AlignItems::Center,
                row_gap: Val::Px(3.0),
                padding: UiRect::all(Val::Px(8.0)),
                border: UiRect::all(Val::Px(2.0)),
                ..default()
            },
            BorderColor(glass::EDGE),
            BackgroundColor(glass::GLASS_DEEP),
            BorderRadius::all(Val::Px(10.0)),
        ))
        .with_children(|c| {
            c.spawn((
                ImageNode::new(sprite),
                sprite_tag,
                Node { width: Val::Px(w * 0.86), height: Val::Px(w * 0.86), ..default() },
            ));
            c.spawn((
                Text::new(label.to_string()),
                label_tag,
                TextFont { font_size: 17.0, ..default() },
                TextColor(Color::srgb(0.92, 0.94, 1.0)),
            ));
            c.spawn((
                Text::new(sub.to_string()),
                sub_tag,
                TextFont { font_size: 13.0, ..default() },
                TextColor(Color::srgb(0.6, 0.65, 0.8)),
            ));
        });
}

/// Build (or tear down) the Drill Yard's party panel as `city.party_open` flips.
///
/// Spawned rather than drawn into the shared HUD line so the slots and classes are
/// real buttons — mustering a party is pointing at heroes, not memorising [1]-[4].
///
/// This is the login screen's old party builder, brought into town and given the
/// thing it never had: the heroes are *yours* here, with names you can change, so
/// the panel shows portraits, a class's role and kit, and its stat shape — the
/// reading a player actually wants before committing four slots to a dive.
#[allow(clippy::too_many_arguments)]
pub(crate) fn party_panel(
    mut commands: Commands,
    city: Res<CityUi>,
    unlocks: Res<UnlocksRes>,
    session: Res<Session>,
    hero_names: Res<AccountHeroNames>,
    wa: Option<Res<WorldAssets>>,
    loadouts: Res<LoadoutData>,
    existing: Query<Entity, With<PartyPanelRoot>>,
    mut was_open: Local<bool>,
    mut shown: Local<(usize, usize, i64)>,
) {
    // Rebuild when the yard opens/closes OR when the saved list changes, so a save or
    // delete is reflected without closing and reopening the panel.
    //
    // The unlock set is in that signature too, because the panel can OPEN before it
    // has arrived: `prompt_party_if_unset` fires as soon as the hero roster loads,
    // and a palette built a moment earlier would offer the Explorer alone and keep
    // offering it for as long as the panel stayed up — a player looking straight at
    // a class they own and cannot pick.
    let sig = (loadouts.list.len(), unlocks.owned.len(), unlocks.party_slots as i64);
    if city.party_open == *was_open && (!city.party_open || sig == *shown) {
        return;
    }
    *was_open = city.party_open;
    *shown = sig;
    for e in &existing {
        commands.entity(e).despawn();
    }
    if !city.party_open {
        return;
    }
    let pool = fieldable_classes(&unlocks);
    let slots = (unlocks.party_slots.max(1) as usize).min(4);
    let sprite = |key: &str| -> Handle<Image> {
        wa.as_ref().map(|w| w.class_frames(key).idle[0].clone()).unwrap_or_default()
    };
    let focus = class_info(if city.yard_focus.is_empty() {
        session.party.first().map(|s| s.as_str()).unwrap_or("explorer")
    } else {
        city.yard_focus.as_str()
    });
    commands
        .spawn((
            PartyPanelRoot,
            Node {
                position_type: PositionType::Absolute,
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                flex_direction: FlexDirection::Column,
                align_items: AlignItems::Center,
                justify_content: JustifyContent::Center,
                row_gap: Val::Px(8.0),
                ..default()
            },
            BackgroundColor(glass::SCRIM),
        ))
        .with_children(|p| {
            p.spawn((
                Text::new("THE DRILL YARD"),
                TextFont { font_size: 34.0, ..default() },
                TextColor(Color::srgb(0.98, 0.9, 0.68)),
            ));
            p.spawn((
                Text::new(format!(
                    "{slots} of 4 slots earned \u{2014} click a hero, then a class. [R] renames."
                )),
                TextFont { font_size: 16.0, ..default() },
                TextColor(Color::srgb(0.6, 0.65, 0.8)),
            ));

            // The party itself: one card per slot, portrait + class + the hero's own
            // name. A locked slot is drawn rather than omitted, so the roster you are
            // working toward is legible instead of a list that stops short.
            p.spawn(Node {
                flex_direction: FlexDirection::Row,
                column_gap: Val::Px(10.0),
                margin: UiRect::top(Val::Px(4.0)),
                ..default()
            })
            .with_children(|row| {
                for i in 0..4 {
                    if i < slots {
                        let cls =
                            session.party.get(i).cloned().unwrap_or_else(|| "explorer".into());
                        let name = hero_names
                            .names
                            .get(i)
                            .cloned()
                            .filter(|n| !n.is_empty())
                            .unwrap_or_else(|| format!("Hero {}", i + 1));
                        yard_card(
                            row,
                            sprite(&cls),
                            class_info(&cls).name,
                            &name,
                            118.0,
                            PartySlotButton(i),
                            PartySlotSprite(i),
                            PartySlotLabel(i),
                            PartySlotHeroName(i),
                        );
                    } else {
                        row.spawn((
                            Node {
                                width: Val::Px(118.0),
                                flex_direction: FlexDirection::Column,
                                align_items: AlignItems::Center,
                                justify_content: JustifyContent::Center,
                                padding: UiRect::all(Val::Px(8.0)),
                                border: UiRect::all(Val::Px(2.0)),
                                ..default()
                            },
                            BorderColor(glass::EDGE_SOFT),
                            BackgroundColor(glass::CHIP_OFF),
                            BorderRadius::all(Val::Px(10.0)),
                        ))
                        .with_children(|c| {
                            c.spawn((
                                Text::new(format!("{}\nlocked", i + 1)),
                                TextFont { font_size: 14.0, ..default() },
                                TextColor(Color::srgb(0.45, 0.48, 0.58)),
                            ));
                        });
                    }
                }
            });

            // Renaming lives beside the heroes it renames, rather than behind a menu
            // in the middle of a dive — this is the screen where they are people.
            p.spawn((
                Button,
                YardRenameButton,
                Node {
                    padding: UiRect::axes(Val::Px(12.0), Val::Px(6.0)),
                    border: UiRect::all(Val::Px(1.0)),
                    ..default()
                },
                BorderColor(glass::EDGE_SOFT),
                BorderRadius::all(Val::Px(6.0)),
                BackgroundColor(glass::CHIP_OFF),
            ))
            .with_children(|b| {
                b.spawn((
                    Text::new("Rename this hero"),
                    YardRenameText,
                    TextFont { font_size: 14.0, ..default() },
                    TextColor(Color::srgb(0.92, 0.94, 1.0)),
                ));
            });

            // Only what the account owns — the whole reason this lives in town rather
            // than on the pre-authentication login screen.
            p.spawn(Node {
                flex_direction: FlexDirection::Row,
                column_gap: Val::Px(8.0),
                flex_wrap: FlexWrap::Wrap,
                row_gap: Val::Px(8.0),
                justify_content: JustifyContent::Center,
                margin: UiRect::top(Val::Px(2.0)),
                ..default()
            })
            .with_children(|row| {
                for key in &pool {
                    let ci = class_info(key);
                    yard_card(
                        row,
                        sprite(key),
                        ci.name,
                        "",
                        104.0,
                        PartyClassButton(key),
                        PartyClassSprite(key),
                        (),
                        (),
                    );
                }
            });

            // The detail panel: what a class actually is, at the moment you are
            // deciding whether to field it.
            p.spawn((
                Node {
                    width: Val::Px(780.0),
                    flex_direction: FlexDirection::Row,
                    column_gap: Val::Px(16.0),
                    padding: UiRect::all(Val::Px(12.0)),
                    border: UiRect::all(Val::Px(1.0)),
                    ..default()
                },
                BorderColor(glass::EDGE),
                BackgroundColor(glass::GLASS_DEEP),
                BorderRadius::all(Val::Px(12.0)),
            ))
            .with_children(|d| {
                d.spawn((
                    ImageNode::new(sprite(focus.key)),
                    YardDetailSprite,
                    Node { width: Val::Px(120.0), height: Val::Px(120.0), ..default() },
                ));
                d.spawn(Node {
                    flex_direction: FlexDirection::Column,
                    row_gap: Val::Px(5.0),
                    flex_grow: 1.0,
                    ..default()
                })
                .with_children(|col| {
                    col.spawn((
                        Text::new(focus.name.to_string()),
                        YardDetailName,
                        TextFont { font_size: 26.0, ..default() },
                        TextColor(Color::srgb(1.0, 0.85, 0.45)),
                    ));
                    col.spawn((
                        Text::new(focus.role.to_string()),
                        YardDetailRole,
                        TextFont { font_size: 15.0, ..default() },
                        TextColor(Color::srgb(0.78, 0.82, 0.95)),
                    ));
                    col.spawn(Node {
                        flex_direction: FlexDirection::Row,
                        column_gap: Val::Px(24.0),
                        margin: UiRect::top(Val::Px(3.0)),
                        ..default()
                    })
                    .with_children(|body| {
                        body.spawn(Node {
                            flex_direction: FlexDirection::Column,
                            row_gap: Val::Px(4.0),
                            ..default()
                        })
                        .with_children(|stats| {
                            for (si, name) in
                                ["HP", "ATK", "SPD", "MAG", "DEF"].iter().enumerate()
                            {
                                stats
                                    .spawn(Node {
                                        flex_direction: FlexDirection::Row,
                                        align_items: AlignItems::Center,
                                        column_gap: Val::Px(6.0),
                                        ..default()
                                    })
                                    .with_children(|r| {
                                        r.spawn((
                                            Text::new(name.to_string()),
                                            TextFont { font_size: 12.0, ..default() },
                                            TextColor(Color::srgb(0.6, 0.65, 0.8)),
                                            Node { width: Val::Px(34.0), ..default() },
                                        ));
                                        for seg in 0..5u8 {
                                            r.spawn((
                                                YardStatFill { stat: si as u8, seg },
                                                Node {
                                                    width: Val::Px(20.0),
                                                    height: Val::Px(9.0),
                                                    ..default()
                                                },
                                                BackgroundColor(glass::CHIP_OFF),
                                                BorderRadius::all(Val::Px(2.0)),
                                            ));
                                        }
                                    });
                            }
                        });
                        body.spawn((
                            Text::new(crate::screens::kit_text(focus)),
                            YardDetailKit,
                            TextFont { font_size: 13.0, ..default() },
                            TextColor(Color::srgb(0.7, 0.85, 0.7)),
                        ));
                    });
                });
            });
            // PT-2: the saved compositions. Named rather than numbered slots because
            // the point is recognising a team at a glance ("Delvers", "Boss squad").
            p.spawn((
                Text::new("Saved parties"),
                TextFont { font_size: 13.0, ..default() },
                TextColor(Color::srgb(0.6, 0.65, 0.8)),
                Node { margin: UiRect::top(Val::Px(6.0)), ..default() },
            ));
            if loadouts.list.is_empty() {
                p.spawn((
                    Text::new("none yet"),
                    TextFont { font_size: 13.0, ..default() },
                    TextColor(Color::srgb(0.45, 0.48, 0.58)),
                ));
            }
            for l in &loadouts.list {
                p.spawn(Node {
                    flex_direction: FlexDirection::Row,
                    column_gap: Val::Px(6.0),
                    align_items: AlignItems::Center,
                    ..default()
                })
                .with_children(|row| {
                    row.spawn((
                        Button,
                        LoadoutLoadButton(l.name.clone()),
                        Node {
                            flex_grow: 1.0,
                            padding: UiRect::axes(Val::Px(9.0), Val::Px(6.0)),
                            border: UiRect::all(Val::Px(1.0)),
                            ..default()
                        },
                        BorderColor(glass::EDGE_SOFT),
                        BorderRadius::all(Val::Px(6.0)),
                        BackgroundColor(glass::CHIP_OFF),
                    ))
                    .with_children(|b| {
                        let comp = l
                            .classes
                            .iter()
                            .map(|c| class_info(c).name)
                            .collect::<Vec<_>>()
                            .join(" / ");
                        b.spawn((
                            Text::new(format!("{}  —  {comp}", l.name)),
                            TextFont { font_size: 13.0, ..default() },
                            TextColor(Color::srgb(0.92, 0.94, 1.0)),
                        ));
                    });
                    row.spawn((
                        Button,
                        LoadoutDeleteButton(l.name.clone()),
                        Node {
                            padding: UiRect::axes(Val::Px(8.0), Val::Px(6.0)),
                            border: UiRect::all(Val::Px(1.0)),
                            ..default()
                        },
                        BorderColor(glass::EDGE_SOFT),
                        BorderRadius::all(Val::Px(6.0)),
                        BackgroundColor(glass::CHIP_OFF),
                    ))
                    .with_children(|b| {
                        b.spawn((
                            Text::new("x"),
                            TextFont { font_size: 13.0, ..default() },
                            TextColor(Color::srgb(0.9, 0.6, 0.6)),
                        ));
                    });
                });
            }
            p.spawn((
                Node {
                    flex_direction: FlexDirection::Row,
                    column_gap: Val::Px(6.0),
                    align_items: AlignItems::Center,
                    margin: UiRect::top(Val::Px(4.0)),
                    ..default()
                },
            ))
            .with_children(|row| {
                row.spawn((
                    Text::new("Name:"),
                    TextFont { font_size: 13.0, ..default() },
                    TextColor(Color::srgb(0.6, 0.65, 0.8)),
                ));
                row.spawn((
                    Node {
                        flex_grow: 1.0,
                        padding: UiRect::axes(Val::Px(8.0), Val::Px(5.0)),
                        border: UiRect::all(Val::Px(1.0)),
                        ..default()
                    },
                    BorderColor(glass::EDGE_SOFT),
                    BorderRadius::all(Val::Px(5.0)),
                    BackgroundColor(glass::CHIP_OFF),
                ))
                .with_children(|f| {
                    f.spawn((
                        Text::new(String::new()),
                        LoadoutNameText,
                        TextFont { font_size: 13.0, ..default() },
                        TextColor(Color::srgb(0.92, 0.94, 1.0)),
                    ));
                });
            });
            p.spawn((
                Button,
                LoadoutSaveButton,
                Node {
                    padding: UiRect::axes(Val::Px(10.0), Val::Px(6.0)),
                    margin: UiRect::top(Val::Px(2.0)),
                    justify_content: JustifyContent::Center,
                    border: UiRect::all(Val::Px(1.0)),
                    ..default()
                },
                BorderColor(glass::EDGE_SOFT),
                BorderRadius::all(Val::Px(6.0)),
                BackgroundColor(glass::CHIP_OFF),
            ))
            .with_children(|b| {
                b.spawn((
                    Text::new("Save this party"),
                    TextFont { font_size: 13.0, ..default() },
                    TextColor(Color::srgb(0.92, 0.94, 1.0)),
                ));
            });
            p.spawn((
                Button,
                PartyDoneButton,
                Node {
                    padding: UiRect::axes(Val::Px(12.0), Val::Px(8.0)),
                    margin: UiRect::top(Val::Px(4.0)),
                    justify_content: JustifyContent::Center,
                    border: UiRect::all(Val::Px(1.5)),
                    ..default()
                },
                BorderColor(glass::EDGE),
                BorderRadius::all(Val::Px(8.0)),
                BackgroundColor(glass::ACTIVE),
            ))
            .with_children(|b| {
                b.spawn((
                    Text::new("Done"),
                    TextFont { font_size: 15.0, ..default() },
                    TextColor(Color::srgb(0.98, 0.9, 0.68)),
                ));
            });
        });
}

/// Clicks inside the Drill Yard: pick a slot, assign a class, or close.
#[allow(clippy::too_many_arguments)]
pub(crate) fn party_panel_buttons(
    slots_q: Query<(&Interaction, &PartySlotButton), Changed<Interaction>>,
    class_q: Query<(&Interaction, &PartyClassButton), Changed<Interaction>>,
    done_q: Query<&Interaction, (Changed<Interaction>, With<PartyDoneButton>)>,
    rename_q: Query<&Interaction, (Changed<Interaction>, With<YardRenameButton>)>,
    keys: Res<ButtonInput<KeyCode>>,
    unlocks: Res<UnlocksRes>,
    hero_names: Res<AccountHeroNames>,
    mut rename: ResMut<HeroRename>,
    mut session: ResMut<Session>,
    mut city: ResMut<CityUi>,
) {
    // Hovering reads, clicking commits. Pointing at a class you are only curious
    // about should never quietly change who you are taking down.
    for (i, b) in &slots_q {
        match *i {
            Interaction::Pressed => {
                session.party_cursor = b.0;
                if let Some(k) = session.party.get(b.0) {
                    city.yard_focus = k.clone();
                }
            }
            Interaction::Hovered => {
                if let Some(k) = session.party.get(b.0) {
                    city.yard_focus = k.clone();
                }
            }
            Interaction::None => {}
        }
    }
    for (i, b) in &class_q {
        if *i == Interaction::Hovered {
            city.yard_focus = b.0.to_string();
        }
    }
    let start_rename = |rename: &mut HeroRename, slot: usize| {
        rename.slot = Some(slot);
        rename.buffer = hero_names.names.get(slot).cloned().unwrap_or_default();
    };
    for i in &rename_q {
        if *i == Interaction::Pressed && rename.slot.is_none() {
            start_rename(&mut rename, session.party_cursor);
        }
    }
    if city.party_open && rename.slot.is_none() && keys.just_pressed(KeyCode::KeyR) {
        start_rename(&mut rename, session.party_cursor);
    }
    let slots = (unlocks.party_slots.max(1) as usize).min(4);
    for (i, b) in &class_q {
        if *i != Interaction::Pressed {
            continue;
        }
        if session.party.len() < slots {
            session.party.resize(slots, "explorer".to_string());
        }
        session.party.truncate(slots.max(1));
        let slot = session.party_cursor.min(session.party.len().saturating_sub(1));
        session.party[slot] = b.0.to_string();
        session.party_chosen = true;
    }
    for i in &done_q {
        if *i == Interaction::Pressed {
            city.party_open = false;
            session.party_chosen = true;
            city.notice = "Party set.".to_string();
        }
    }
}

/// Type the name for the next save while the Drill Yard is open.
///
/// Only while the yard is open, so the town's WASD/E/C shortcuts are untouched the
/// rest of the time — the panel is the one place in town that swallows letter keys.
pub(crate) fn loadout_name_input(
    keys: Res<ButtonInput<KeyCode>>,
    rename: Res<HeroRename>,
    mut city: ResMut<CityUi>,
    mut q: Query<&mut Text, With<LoadoutNameText>>,
) {
    // Two text fields share one keyboard: while a hero is being renamed the letters
    // belong to it, or naming a hero would also name the loadout.
    if !city.party_open || rename.slot.is_some() {
        return;
    }
    let shift = keys.pressed(KeyCode::ShiftLeft) || keys.pressed(KeyCode::ShiftRight);
    let mut changed = false;
    if keys.just_pressed(KeyCode::Backspace) {
        city.loadout_name.pop();
        changed = true;
    }
    for key in keys.get_just_pressed() {
        if let Some(c) = crate::screens::typed_char(*key, shift) {
            if city.loadout_name.chars().count() < 24 {
                city.loadout_name.push(c);
                changed = true;
            }
        }
    }
    if changed {
        if let Ok(mut t) = q.single_mut() {
            **t = city.loadout_name.clone();
        }
    }
}

/// Clicks on the saved-loadout rows: load one into the live party, delete one, or
/// save the current party under the typed name.
pub(crate) fn loadout_buttons(
    load_q: Query<(&Interaction, &LoadoutLoadButton), Changed<Interaction>>,
    del_q: Query<(&Interaction, &LoadoutDeleteButton), Changed<Interaction>>,
    save_q: Query<&Interaction, (Changed<Interaction>, With<LoadoutSaveButton>)>,
    net: NonSend<NetRes>,
    loadouts: Res<LoadoutData>,
    unlocks: Res<UnlocksRes>,
    mut session: ResMut<Session>,
    mut city: ResMut<CityUi>,
) {
    for (i, b) in &load_q {
        if *i != Interaction::Pressed {
            continue;
        }
        if let Some(l) = loadouts.list.iter().find(|l| l.name == b.0) {
            // The SERVER applies it — it re-clamps the classes and re-equips the gear
            // it captured, skipping anything since broken, sold or lost. This local
            // copy is only so the panel reads right before the refresh lands; the
            // authoritative answer is whatever the server did.
            net.0.apply_loadout(l.name.clone());
            let owned = fieldable_classes(&unlocks);
            let slots = (unlocks.party_slots.max(1) as usize).min(4);
            let party: Vec<String> = l
                .classes
                .iter()
                .take(slots)
                .map(|c| {
                    if owned.iter().any(|o| o == c) {
                        c.clone()
                    } else {
                        "explorer".to_string()
                    }
                })
                .collect();
            if !party.is_empty() {
                session.party = party;
                session.party_chosen = true;
                session.party_cursor = 0;
                city.notice = format!("Loaded \"{}\".", l.name);
            }
        }
    }
    for (i, b) in &del_q {
        if *i == Interaction::Pressed {
            net.0.delete_loadout(b.0.clone());
            city.notice = format!("Deleted \"{}\".", b.0);
        }
    }
    for i in &save_q {
        if *i != Interaction::Pressed {
            continue;
        }
        // The typed name if there is one, else the next free "Party N" — an empty
        // field should still save something rather than refuse.
        let typed = city.loadout_name.trim().to_string();
        let name = if typed.is_empty() {
            let mut n = 1;
            while loadouts.list.iter().any(|l| l.name == format!("Party {n}")) {
                n += 1;
            }
            format!("Party {n}")
        } else {
            typed
        };
        city.loadout_name.clear();
        net.0.save_loadout(name.clone(), session.party.clone());
        city.notice = format!("Saved as \"{name}\".");
    }
}

/// Keep the slot labels and the selected-slot highlight honest as clicks land.
#[allow(clippy::type_complexity, clippy::too_many_arguments)]
pub(crate) fn party_panel_refresh(
    session: Res<Session>,
    city: Res<CityUi>,
    rename: Res<HeroRename>,
    hero_names: Res<AccountHeroNames>,
    wa: Option<Res<WorldAssets>>,
    mut labels: Query<(&PartySlotLabel, &mut Text), (Without<PartySlotHeroName>, Without<YardDetailName>, Without<YardDetailRole>, Without<YardDetailKit>, Without<YardRenameText>)>,
    mut hero_name_q: Query<(&PartySlotHeroName, &mut Text), (Without<PartySlotLabel>, Without<YardDetailName>, Without<YardDetailRole>, Without<YardDetailKit>, Without<YardRenameText>)>,
    mut slot_sprites: Query<(&PartySlotSprite, &mut ImageNode), (Without<PartyClassSprite>, Without<YardDetailSprite>)>,
    mut class_sprites: Query<(&PartyClassSprite, &mut ImageNode), (Without<PartySlotSprite>, Without<YardDetailSprite>)>,
    mut slot_borders: Query<(&PartySlotButton, &mut BorderColor)>,
    mut det_sprite: Query<&mut ImageNode, (With<YardDetailSprite>, Without<PartySlotSprite>, Without<PartyClassSprite>)>,
    mut det_name: Query<&mut Text, (With<YardDetailName>, Without<PartySlotLabel>, Without<PartySlotHeroName>, Without<YardDetailRole>, Without<YardDetailKit>, Without<YardRenameText>)>,
    mut det_role: Query<&mut Text, (With<YardDetailRole>, Without<PartySlotLabel>, Without<PartySlotHeroName>, Without<YardDetailName>, Without<YardDetailKit>, Without<YardRenameText>)>,
    mut det_kit: Query<&mut Text, (With<YardDetailKit>, Without<PartySlotLabel>, Without<PartySlotHeroName>, Without<YardDetailName>, Without<YardDetailRole>, Without<YardRenameText>)>,
    mut rename_text: Query<&mut Text, (With<YardRenameText>, Without<PartySlotLabel>, Without<PartySlotHeroName>, Without<YardDetailName>, Without<YardDetailRole>, Without<YardDetailKit>)>,
    mut stat_fills: Query<(&YardStatFill, &mut BackgroundColor), Without<PartySlotButton>>,
) {
    for (tag, mut t) in &mut labels {
        let cls = session.party.get(tag.0).cloned().unwrap_or_else(|| "explorer".into());
        let want = class_info(&cls).name.to_string();
        if **t != want {
            **t = want;
        }
    }
    // While typing, the card shows the buffer with a caret — you are editing the hero
    // in front of you, not filling in a form somewhere else on screen.
    for (tag, mut t) in &mut hero_name_q {
        let want = match rename.slot {
            Some(s) if s == tag.0 => format!("{}_", rename.buffer),
            _ => hero_names
                .names
                .get(tag.0)
                .cloned()
                .filter(|n| !n.is_empty())
                .unwrap_or_else(|| format!("Hero {}", tag.0 + 1)),
        };
        if **t != want {
            **t = want;
        }
    }
    for (tag, mut bc) in &mut slot_borders {
        let want = if tag.0 == session.party_cursor {
            Color::srgb(1.0, 0.85, 0.45)
        } else {
            glass::EDGE
        };
        if bc.0 != want {
            *bc = BorderColor(want);
        }
    }
    if let Ok(mut t) = rename_text.single_mut() {
        let want = if rename.slot.is_some() {
            "typing\u{2026}  Enter to keep, Esc to drop".to_string()
        } else {
            "Rename this hero".to_string()
        };
        if **t != want {
            **t = want;
        }
    }

    // Sprites re-assign every frame rather than once at spawn: the panel can open
    // before the class art has finished loading, and a card that stayed blank until
    // it was rebuilt is the bug that reads as "the portraits don't work".
    let Some(wa) = wa else { return };
    let img = |key: &str| wa.class_frames(key).idle[0].clone();
    for (tag, mut node) in &mut slot_sprites {
        if let Some(k) = session.party.get(tag.0) {
            node.image = img(k);
        }
    }
    for (tag, mut node) in &mut class_sprites {
        node.image = img(tag.0);
    }

    let focus = class_info(if city.yard_focus.is_empty() {
        session.party.first().map(|s| s.as_str()).unwrap_or("explorer")
    } else {
        city.yard_focus.as_str()
    });
    if let Ok(mut n) = det_sprite.single_mut() {
        n.image = img(focus.key);
    }
    if let Ok(mut t) = det_name.single_mut() {
        if **t != focus.name {
            **t = focus.name.to_string();
        }
    }
    if let Ok(mut t) = det_role.single_mut() {
        if **t != focus.role {
            **t = focus.role.to_string();
        }
    }
    if let Ok(mut t) = det_kit.single_mut() {
        let want = crate::screens::kit_text(focus);
        if **t != want {
            **t = want;
        }
    }
    let vals = [focus.hp, focus.atk, focus.spd, focus.mag, focus.def];
    let cols = [
        Color::srgb(0.4, 0.75, 0.45),
        Color::srgb(0.9, 0.5, 0.4),
        Color::srgb(0.5, 0.8, 0.9),
        Color::srgb(0.7, 0.55, 1.0),
        Color::srgb(0.6, 0.65, 0.85),
    ];
    for (f, mut bg) in &mut stat_fills {
        let on = f.seg < vals[f.stat as usize];
        let want =
            if on { cols[f.stat as usize] } else { Color::srgb(0.2, 0.22, 0.3) };
        if bg.0 != want {
            bg.0 = want;
        }
    }
}

/// Type a hero's name in the Drill Yard. Reuses the same [`HeroRename`] buffer and
/// the same `run.rename_hero` message the in-dive party screen uses, so a name set
/// here and a name set there are one thing.
pub(crate) fn yard_rename_input(
    keys: Res<ButtonInput<KeyCode>>,
    net: NonSend<NetRes>,
    city: Res<CityUi>,
    mut hero_names: ResMut<AccountHeroNames>,
    mut rename: ResMut<HeroRename>,
) {
    if !city.party_open {
        return;
    }
    let Some(slot) = rename.slot else { return };
    if keys.just_pressed(KeyCode::Escape) {
        rename.slot = None;
        rename.buffer.clear();
        return;
    }
    if keys.just_pressed(KeyCode::Enter) {
        let name = rename.buffer.trim().to_string();
        if !name.is_empty() {
            // Write the local copy too. Renaming from town has no run behind it, so
            // the server persists the name and answers with an EMPTY roster — there
            // is no party to describe yet — and the card would snap back to the old
            // name the moment the edit buffer cleared. The server applies the same
            // trim and the same 24-character cap this buffer does, so the optimistic
            // copy and the stored one agree.
            if hero_names.names.len() <= slot {
                hero_names.names.resize(slot + 1, String::new());
            }
            hero_names.names[slot] = name.clone();
            net.0.send(ClientCmd::RenameHero { slot: slot as i32, name });
        }
        rename.slot = None;
        rename.buffer.clear();
        return;
    }
    if keys.just_pressed(KeyCode::Backspace) {
        rename.buffer.pop();
        return;
    }
    let shift = keys.pressed(KeyCode::ShiftLeft) || keys.pressed(KeyCode::ShiftRight);
    if keys.just_pressed(KeyCode::Space) && rename.buffer.chars().count() < 24 {
        rename.buffer.push(' ');
    }
    for key in keys.get_just_pressed() {
        if let Some(c) = crate::screens::typed_char(*key, shift) {
            if rename.buffer.chars().count() < 24 {
                rename.buffer.push(c);
            }
        }
    }
}
