//! The Last City — the persistent hub city: walkable HD-2D plaza, districts, HUD.
//! Extracted from `main.rs` during the module reorg.


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
    /// The Bounty Board: the posted hunts and their rewards (AD-4).
    Hunts,
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
    /// Close the game window outright — the only place that quit lives besides
    /// the Ended screen's Esc, since the hub has no "walk to the exit" district.
    Quit,
}

/// Marks a tappable on-screen city action button.
#[derive(Component)]
pub(crate) struct CityActionButton(pub(crate) CityAct);

/// The tap-action bar's own container — carries an invisible border by default,
/// brightened by `highlight_tap_action_bar` while the town tour is pointing at
/// it (`tutorial::TAP_ACTION_BAR_STEP`), so the highlight always bounds
/// exactly the real, currently-rendered bar rather than a guessed box.
#[derive(Component)]
pub(crate) struct TapActionBar;

/// Root UI node that holds the per-district nameplates (small, always-on labels
/// floating above each district — distinct from the interactive travel column:
/// this is passive world signage, not a menu).
#[derive(Component)]
pub(crate) struct DistrictNameplateRoot;
/// One district nameplate (rebuilt each frame).
#[derive(Component)]
pub(crate) struct DistrictNameplate;

/// A walkable district: an anchor on the plaza the avatar can stand in and act on.
pub(crate) struct District {
    label: &'static str,
    /// What you actually do here, in plain words. The names are the city's fiction
    /// (CANON §G) and a fiction does not tell a new player where to sell a rock, so the
    /// two always travel together: the nav chip, the walk-up prompt and the counter's
    /// own header all carry this.
    purpose: &'static str,
    x: f32,
    z: f32,
    /// Radius the avatar must be within to interact.
    radius: f32,
    action: CityAction,
}

/// The city's interactable districts (positions are plaza-local world x/z; the
/// avatar spawns near +z/south and the camera looks north/-z).
pub(crate) const CITY_DISTRICTS: &[District] = &[
    District {
        label: "The Threshold",
        purpose: "leave town: start a run",
        x: 0.0,
        z: -19.0,
        radius: 5.5,
        action: CityAction::Dive,
    },
    District {
        label: "The Vault-Deep",
        purpose: "your storage: chits, materials, gear",
        x: -13.0,
        z: -5.0,
        radius: 5.0,
        action: CityAction::Vault,
    },
    District {
        label: "The Market Tiers",
        purpose: "buy supplies, sell what you hauled home",
        x: 13.0,
        z: 0.0,
        radius: 6.0,
        action: CityAction::Shop,
    },
    District {
        label: "The Forge & Alembic",
        purpose: "craft, repair and re-roll gear",
        x: -10.0,
        z: 9.0,
        radius: 5.0,
        action: CityAction::Craft,
    },
    District {
        label: "The Bounty Board",
        purpose: "hunts: what to go and do, and what it pays",
        x: 8.0,
        z: -12.0,
        radius: 4.5,
        action: CityAction::Hunts,
    },
    District {
        label: "The Drill Yard",
        purpose: "pick the party you take down",
        x: 15.0,
        z: -13.0,
        radius: 5.0,
        action: CityAction::Party,
    },
    District {
        label: "The Vanguard Wall",
        purpose: "the season's deepest-run rankings",
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
    mut heat: ResMut<crate::overworld::HeatUi>,
    mut pick: ResMut<CounterPick>,
) {
    inv.loaded = false;
    net.0.fetch_inventory();
    pick.clear();
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
    city.hunts_open = crate::flags::hunts_preview_flag();
    // Screenshot-only: land with a row already picked, so the detail column's description,
    // amount and commit buttons are on screen without a click to make them appear.
    if let Some(row) = crate::flags::pick_preview_flag() {
        pick.pick(row);
    }
    if city.hunts_open {
        net.0.fetch_hunts();
    }
    if crate::flags::heat_preview_flag() {
        // A plausible heat, laid out the way the server would for a mid-tier piece:
        // three blows, bands in different places, so the bar can be read at a glance.
        *heat = crate::overworld::HeatUi {
            job_id: Some("preview".into()),
            service: "reroll".into(),
            strikes: 3,
            sweep_ms: 1600,
            bands: vec![(0.18, 0.40), (0.55, 0.72), (0.30, 0.48)],
            struck: 0,
            opened_at: 0.0,
        };
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
                // The anvil's heat is struck here too, so the bar lives in the same
                // panel the bench reads out of.
                crate::overworld::spawn_heat_bar(panel);
            });
            // Always-available tap actions (bottom-right). Mirror the keyboard: Dive
            // (Enter), Vault (V), Co-op (C) — so the hub is fully click/tap driven
            // without having to walk to each district first.
            p.spawn((
                TapActionBar,
                Node {
                    position_type: PositionType::Absolute,
                    right: Val::Px(16.0),
                    bottom: Val::Px(16.0),
                    flex_direction: FlexDirection::Column,
                    row_gap: Val::Px(8.0),
                    align_items: AlignItems::FlexEnd,
                    padding: UiRect::all(Val::Px(6.0)),
                    border: UiRect::all(Val::Px(2.0)),
                    ..default()
                },
                // Invisible until the tour highlights it (see `TapActionBar`).
                BorderColor(Color::NONE),
                BorderRadius::all(Val::Px(10.0)),
            ))
            .with_children(|bar| {
                for (act, label) in [
                    (CityAct::Party, "Party"),
                    (CityAct::Dive, "Run"),
                    (CityAct::Vault, "Vault"),
                    (CityAct::Coop, "Co-op"),
                    (CityAct::Quit, "Exit Game"),
                ] {
                    city_button(bar, act, label);
                }
            });
            // Full-screen overlay that holds per-district nameplates, positioned in
            // screen space by `render_district_nameplates` (mirrors the overworld's
            // `NameplateRoot`). A sibling of the HUD panels, not nested in them, so
            // it can be absolutely positioned edge-to-edge.
            p.spawn((
                DistrictNameplateRoot,
                Node {
                    position_type: PositionType::Absolute,
                    width: Val::Percent(100.0),
                    height: Val::Percent(100.0),
                    ..default()
                },
            ));
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

/// Brightens the tap-action bar's own border while the town tour is pointing at
/// it, and clears it otherwise — mutated in place every frame (like
/// `battle::style_command_menu`) rather than despawned/respawned, since the bar
/// itself is spawned once in `city_hud`, not rebuilt per frame.
pub(crate) fn highlight_tap_action_bar(
    tutorial: Res<Tutorial>,
    mut bar: Query<&mut BorderColor, With<TapActionBar>>,
) {
    let Ok(mut border) = bar.single_mut() else { return };
    border.0 = if tutorial.town_step == Some(crate::tutorial::TAP_ACTION_BAR_STEP) {
        glass::ACTIVE_EDGE
    } else {
        Color::NONE
    };
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
    mut exit: EventWriter<AppExit>,
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
                    net.0.send(ClientCmd::EnterMaze {
                        party: session.party.clone(),
                        tutorial: false,
                        hub: session.hub.clone(),
                    });
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
                    net.0.fetch_bounties();
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
            CityAct::Quit => {
                exit.write(AppExit::Success);
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
    // The two boards travel as one param: this system is at Bevy's 16-param ceiling, and
    // the Bounty Board's own two sides are the natural pair to group.
    mut boards: (ResMut<HuntBoardData>, Res<BountyData>),
    mut shop_selling: ResMut<ShopSelling>,
    // Grouped for the same reason as `boards` above: this system is already at
    // Bevy's 16-param ceiling.
    // No `UnlocksRes` here any more: its only reader was the unreachable Drill-Yard branch
    // this system returns above (see the note further down). `pick` rides this tuple because
    // the system is at Bevy's 16-param ceiling.
    (tutorial, mut pick): (Res<Tutorial>, ResMut<CounterPick>),
    mut next: ResMut<NextState<Screen>>,
) {
    let (hunts, bounties) = (&mut boards.0, &boards.1);
    // The welcome tour has its own keyboard handler (`tutorial::tour_keyboard`)
    // reading the same Enter/Space keys to advance its steps — without this
    // guard, pressing Enter to step the tour would ALSO fire a dive here.
    if tutorial.town_step.is_some() {
        return;
    }
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
        .is_some_and(|i| matches!(CITY_DISTRICTS[i].action, CityAction::Dive));
    // PG-2's hub chooser is deliberately absent: the registry and the "have you been
    // there" gate exist, but nothing spawns a dive AT a hub's distance yet, so choosing
    // one would only change your starting level. See `form_run`.
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
            hub: session.hub.clone(),
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
            && city.near.is_some_and(|i| matches!(CITY_DISTRICTS[i].action, CityAction::Vault)))
    {
        if overlay.kind == Some(OverlayKind::Inventory) {
            overlay.kind = None;
        } else {
            overlay.kind = Some(OverlayKind::Inventory);
            net.0.fetch_bounties();
            *tab = OverlayTab::Items;
            inv.loaded = false;
            net.0.fetch_inventory();
            net.0.fetch_hero_names();
        }
        return;
    }
    // NOTE: no Drill-Yard branch here. `city_input` returns above the moment the yard is
    // open, so the slot/class/[E] handling that used to sit at this point was unreachable
    // for as long as that early-out has existed. The yard's own systems
    // (`party_panel_buttons`, `yard_rename_input`, `loadout_name_input`) drive it.
    // Esc closes whatever town screen is open — the counter, the bench or the board —
    // the same way walking away already does (`city_interact`), so there's always a
    // key that gets you out without having to find the district's own toggle again.
    // A pick is the newest thing on screen, so it's dropped first rather than falling
    // through to close the counter underneath it.
    //
    // Then ONE call, not a chain of `else if`s over each flag. The chain is how the claims
    // board ended up with no way out at all: it was the one panel nobody added to it, and
    // travel lands you inside a district so there was nothing to walk out of either. The
    // Leave chip already routed through `close_counters`; this is the other exit joining it,
    // so a new panel cannot be closable by mouse and stuck by key.
    if keys.just_pressed(KeyCode::Escape) {
        if pick.row.is_some() {
            pick.clear();
        } else {
            city.close_counters();
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
        // A number PICKS its row, exactly as a tap does — the amount and the commit live in
        // the detail column for both. Left/right nudge the amount, ENTER commits, ESC drops
        // the pick (handled with the other Escapes above).
        for (i, key) in KEYS.iter().enumerate() {
            if !keys.just_pressed(*key) {
                continue;
            }
            pick.pick(i);
            return;
        }
        if pick.row.is_some() {
            let dir = i32::from(keys.just_pressed(KeyCode::ArrowRight))
                - i32::from(keys.just_pressed(KeyCode::ArrowLeft));
            if dir != 0 {
                let max = counter_pick_max(&city, &shop, &inv, &craft, &shop_selling, &pick);
                pick.nudge(dir, max);
                return;
            }
            if keys.just_pressed(KeyCode::Enter) {
                commit_counter_pick(
                    &net,
                    &mut city,
                    &shop,
                    &inv,
                    &mut craft,
                    hunts,
                    &shop_selling,
                    &mut pick,
                );
                return;
            }
        }
    }
    // The Bounty Board: ↑/↓ walk the hunts, [1]-[8] (or ENTER on the row) claim one, and
    // [B] turns the board around to the Den's own contracts.
    if city.hunts_open {
        if keys.just_pressed(KeyCode::KeyB) {
            city.bounty_tab = !city.bounty_tab;
            if city.bounty_tab {
                net.0.fetch_bounties();
            }
            return;
        }
        if city.bounty_tab {
            const BOUNTY_KEYS: [KeyCode; 8] = [
                KeyCode::Digit1,
                KeyCode::Digit2,
                KeyCode::Digit3,
                KeyCode::Digit4,
                KeyCode::Digit5,
                KeyCode::Digit6,
                KeyCode::Digit7,
                KeyCode::Digit8,
            ];
            for (i, key) in BOUNTY_KEYS.iter().enumerate() {
                if keys.just_pressed(*key) && i < bounties.active.len() {
                    claim_bounty_row(&net, &mut city, bounties, i);
                    return;
                }
            }
            return;
        }
        let n = hunts.hunts.len();
        if n > 0 && keys.just_pressed(KeyCode::ArrowDown) {
            hunts.cursor = (hunts.cursor + 1) % n;
            return;
        }
        if n > 0 && keys.just_pressed(KeyCode::ArrowUp) {
            hunts.cursor = (hunts.cursor + n - 1) % n;
            return;
        }
        if keys.just_pressed(KeyCode::Enter) {
            claim_hunt_row(&net, &mut city, hunts, hunts.cursor);
            return;
        }
        const HUNT_KEYS: [KeyCode; 8] = [
            KeyCode::Digit1,
            KeyCode::Digit2,
            KeyCode::Digit3,
            KeyCode::Digit4,
            KeyCode::Digit5,
            KeyCode::Digit6,
            KeyCode::Digit7,
            KeyCode::Digit8,
        ];
        for (i, key) in HUNT_KEYS.iter().enumerate() {
            if keys.just_pressed(*key) && i < n {
                hunts.cursor = i;
                claim_hunt_row(&net, &mut city, hunts, i);
                return;
            }
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
                        recipe: String::new(),
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
                        recipe: String::new(),
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
            toggle_district(i, &mut city, &net, &mut craft, &mut pick);
        }
    }
}

/// Walk the avatar around the plaza with WASD/arrows (camera-relative), softly
/// colliding out of building anchors and clamped to the plaza. Client-local — the
/// city has no server-side simulation (see docs/proposals/last-city.md).
/// The camera-relative planar basis for town walking: `(forward, right)` in world xz,
/// for a camera yaw in DEGREES (at yaw 0 the camera looks toward -z).
///
/// `right` is `fwd` rotated +90° in the xz plane — at yaw 0 that is `(1, 0)` = +x = east,
/// which is where the screen's right edge actually is. It used to be `(fwd.y, -fwd.x)`,
/// the opposite, so A/D **and** the walk-facing derived from the motion vector both came
/// out mirrored in town (`LC-2`). The overworld's own mover has always used this handedness;
/// the two screens agreeing is the whole point of naming it once.
pub(crate) fn planar_basis(yaw_deg: f32) -> (Vec2, Vec2) {
    let yaw = yaw_deg.to_radians();
    let fwd = Vec2::new(-yaw.sin(), -yaw.cos()); // W = into the screen
    (fwd, Vec2::new(-fwd.y, fwd.x))
}

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
    let (fwd, right) = planar_basis(look.cam_yaw);
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
        if dist <= d.radius && best.is_none_or(|(_, b)| dist < b) {
            best = Some((i, dist));
        }
    }
    let near = best.map(|(i, _)| i);
    if city.shop_open
        && !crate::flags::shop_preview_flag()
        && !near.is_some_and(|i| matches!(CITY_DISTRICTS[i].action, CityAction::Shop))
    {
        city.shop_open = false;
    }
    if city.board_open
        && !crate::flags::wall_preview_flag()
        && !near.is_some_and(|i| matches!(CITY_DISTRICTS[i].action, CityAction::Vanguard))
    {
        city.board_open = false;
    }
    if city.hunts_open
        && !crate::flags::hunts_preview_flag()
        && !near.is_some_and(|i| matches!(CITY_DISTRICTS[i].action, CityAction::Hunts))
    {
        city.hunts_open = false;
        city.bounty_tab = false;
    }
    city.near = near;
}

pub(crate) fn render_city(
    inv: Res<InventoryData>,
    session: Res<Session>,
    city: Res<CityUi>,
    heat: Res<crate::overworld::HeatUi>,
    notice: Res<Notice>,
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
            district_prompt(d)
        } else {
            "WASD move    [E] enter a district    [ENTER] run    [T] tutorial    [C] co-op    [V] storage chest"
                .to_string()
        };
        // The counters live in the centred three-column panel now — a shop is a menu, and
        // a menu belongs where the eye already is — so this strip is back to the one thing
        // a strip is good for: the walking-around prompt. The anvil's HEAT stays, because
        // it is a timing bar, and a bar that jumps around under the rows you are reading is
        // worse than one that holds still at the foot of the screen.
        // The server's own words win over the client's guess at them: a counter reply
        // ("the board pays 200c", or why it will not) arrives on `Notice`, and until it
        // reached this strip it was spoken to nobody — town has no other line.
        let spoken = notice
            .live(time.elapsed_secs_f64())
            .map(str::to_string)
            .or_else(|| (!city.notice.is_empty()).then(|| city.notice.clone()));
        **t = match crate::overworld::heat_line(&heat, time.elapsed_secs_f64()) {
            Some(bar) => format!("{bar}\n{prompt}"),
            None => match spoken {
                Some(line) => format!("{line}\n{prompt}"),
                None => prompt,
            },
        };
    }
}

/// The walk-up line for the district you are standing in: its name, what it is for, and
/// the one key that does it.
///
/// The name alone is scenery to anyone who has not been told what a Drill Yard is.
pub(crate) fn district_prompt(d: &District) -> String {
    let key = match d.action {
        CityAction::Dive => "[E]/[ENTER] run",
        CityAction::Vault => "[E] open",
        CityAction::Shop => "[E] browse",
        CityAction::Craft => "[E] work",
        CityAction::Vanguard | CityAction::Hunts => "[E] read",
        CityAction::Party => "[E] muster",
    };
    format!("{} - {}    {key}", d.label, d.purpose)
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

#[cfg(test)]
mod tests {
    use super::planar_basis;
    use bevy::math::Vec2;

    /// A sign error in this basis is invisible in a still frame — it only shows as motion —
    /// so the guard is a test. W walks into the screen and D walks screen-right, which at
    /// yaw 0 is EAST. Getting `right` backwards is `LC-2`: the town walked you the wrong way.
    #[test]
    fn town_walking_is_not_mirrored() {
        let (fwd, right) = planar_basis(0.0);
        assert!((fwd - Vec2::new(0.0, -1.0)).length() < 1e-5, "W at yaw 0 must go -z: {fwd:?}");
        assert!((right - Vec2::new(1.0, 0.0)).length() < 1e-5, "D at yaw 0 must go +x: {right:?}");

        // And at every yaw: orthonormal, and `right` is `fwd` turned the SAME way round
        // (a cross product with a consistent sign), not its mirror.
        for deg in (0..360).step_by(15) {
            let (f, r) = planar_basis(deg as f32);
            assert!((f.length() - 1.0).abs() < 1e-5 && (r.length() - 1.0).abs() < 1e-5);
            assert!(f.dot(r).abs() < 1e-5, "{deg}deg: basis is not orthogonal");
            // xz cross product f x r, same sign at every yaw.
            let cross = f.x * r.y - f.y * r.x;
            assert!((cross - 1.0).abs() < 1e-5, "{deg}deg: handedness flipped ({cross})");
        }
    }


    /// Travel has to leave you where [E] works, or the button is a worse version of walking.
    #[test]
    fn travelling_lands_you_inside_the_district_it_names() {
        for (i, d) in CITY_DISTRICTS.iter().enumerate() {
            let mut city = CityUi::default();
            let mut tf = Transform::from_xyz(0.0, 0.0, 0.0);
            travel_to(i, &mut city, &mut tf);
            assert_eq!(city.near, Some(i), "{} should read as the district you are in", d.label);
            let dist = ((tf.translation.x - d.x).powi(2) + (tf.translation.z - d.z).powi(2)).sqrt();
            assert!(
                dist < d.radius,
                "{} lands {dist:.1} away but its radius is {} - [E] would not fire",
                d.label,
                d.radius
            );
        }
    }

    /// And an out-of-range index is a no-op rather than a teleport to the origin.
    #[test]
    fn travelling_nowhere_moves_nothing() {
        let mut city = CityUi::default();
        let mut tf = Transform::from_xyz(3.0, 0.0, 4.0);
        travel_to(99, &mut city, &mut tf);
        assert_eq!(tf.translation.x, 3.0);
        assert_eq!(tf.translation.z, 4.0);
        assert_eq!(city.near, None);
    }

    /// A district's NAME is the city's fiction; a player still has to be told what the
    /// room is for. Both halves are required, so a new district cannot ship as scenery.
    #[test]
    fn every_district_says_what_it_is_for() {
        for d in CITY_DISTRICTS {
            assert!(!d.label.is_empty(), "a nameless district");
            assert!(!d.purpose.is_empty(), "{} does not say what it is for", d.label);
            // The nav column is one sixth of the window: a paragraph does not fit, and a
            // purpose that wraps to three lines is one nobody reads.
            assert!(
                d.purpose.len() <= 48,
                "{}'s purpose is too long for the column: {:?}",
                d.label,
                d.purpose
            );
            assert!(
                d.purpose.chars().next().is_some_and(|c| c.is_lowercase()),
                "{}'s purpose reads as a phrase under the name, not a title: {:?}",
                d.label,
                d.purpose
            );
        }
    }

    #[test]
    fn a_walk_up_prompt_names_the_room_what_it_does_and_one_key() {
        for d in CITY_DISTRICTS {
            let line = district_prompt(d);
            assert!(line.contains(d.label), "{line} does not name the district");
            assert!(line.contains(d.purpose), "{line} does not say what it is for");
            assert!(line.contains("[E]"), "{line} offers no key");
        }
    }

    /// Every town panel has to be closable, by BOTH exits.
    ///
    /// The claims board shipped with no way out at all: `Escape` ran a hand-written chain of
    /// `else if`s over the panel flags and `hunts_open` was simply not in it, while travel
    /// lands you inside a district so walking away could not close it either. Both exits go
    /// through `close_counters` now, and this holds that helper against `any_counter_open`
    /// so the two cannot disagree about what "a counter is open" means.
    ///
    /// The flag list below is still written by hand — Rust cannot enumerate struct fields —
    /// so this does not automatically catch a NEW panel. What it does catch is the thing that
    /// actually went wrong: one of the two helpers knowing about a flag the other does not.
    #[test]
    fn every_town_panel_can_be_closed() {
        let panels: [(&str, fn(&mut CityUi)); 4] = [
            ("shop", |c| c.shop_open = true),
            ("craft", |c| c.craft_open = true),
            ("board", |c| c.board_open = true),
            ("hunts", |c| c.hunts_open = true),
        ];
        for (name, open) in panels {
            let mut city = CityUi::default();
            open(&mut city);
            assert!(city.any_counter_open(), "{name} is open but nothing reports it");
            city.close_counters();
            assert!(!city.any_counter_open(), "{name} survived close_counters");
        }
        // …and all at once, since nothing stops two being open together.
        let mut city = CityUi::default();
        for (_, open) in panels {
            open(&mut city);
        }
        city.bounty_tab = true;
        city.close_counters();
        assert!(!city.any_counter_open(), "a counter survived a blanket close");
        assert!(!city.bounty_tab, "the board's side should reset with it");
    }

    /// The board's row order is also its CLAIM order — the digits and the row chips index
    /// straight into the list — so the sort has to happen where the list is built, and
    /// finished work has to come first or a player scrolls past eight rows to find the one
    /// they came home for.
    #[test]
    fn finished_hunts_sit_at_the_top_of_the_board() {
        let hunt = |name: &str, claimable: bool, claimed: bool| crate::net::HuntLine {
            name: name.into(),
            claimable,
            claimed,
            ..Default::default()
        };
        let mut rows = [
            hunt("paid", false, true),
            hunt("working", false, false),
            hunt("done", true, false),
            hunt("working two", false, false),
            hunt("done two", true, false),
        ];
        rows.sort_by_key(|h| h.board_order());
        let order: Vec<&str> = rows.iter().map(|h| h.name.as_str()).collect();
        assert_eq!(
            order,
            vec!["done", "done two", "working", "working two", "paid"],
            "claimable first, then in hand, then paid - stable inside each group"
        );
    }

    /// Every district needs a number a player can press. This caught the first version
    /// advertising [1]-[6] against seven districts — the Vanguard Wall had no key.
    #[test]
    fn every_district_has_a_travel_key() {
        assert!(
            TRAVEL_KEYS.len() >= CITY_DISTRICTS.len(),
            "{} districts but only {} travel keys",
            CITY_DISTRICTS.len(),
            TRAVEL_KEYS.len()
        );
    }


    /// The Drill Yard is the one screen in town with TWO text fields and a save button
    /// sharing one keyboard, so the thing that breaks is the wiring, not the arithmetic.
    /// These run the real systems.
    fn yard_app() -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .insert_non_send_resource(NetRes(crate::net::start("http://127.0.0.1:1".into())))
            .insert_resource(ButtonInput::<KeyCode>::default())
            .insert_resource(CityUi { party_open: true, ..Default::default() })
            .insert_resource(HeroRename::default())
            .insert_resource(AccountHeroNames {
                names: vec!["Ash".into(), "Bex".into()],
                classes: vec!["explorer".into(), "hunter".into()],
                ..Default::default()
            })
            .insert_resource(Session { party: vec!["explorer".into()], ..Default::default() })
            .insert_resource(UnlocksRes::default())
            .insert_resource(LoadoutData::default())
            .add_systems(Update, (party_panel_buttons, yard_rename_input, loadout_name_input));
        app
    }

    fn press(app: &mut App, key: KeyCode) {
        app.world_mut().resource_mut::<ButtonInput<KeyCode>>().press(key);
        app.update();
        app.world_mut().resource_mut::<ButtonInput<KeyCode>>().clear();
    }

    /// Open the rename the way the yard actually offers it: the button.
    fn open_rename(app: &mut App) {
        app.world_mut().resource_mut::<HeroRename>().slot = Some(0);
        app.world_mut().resource_mut::<HeroRename>().buffer = "Ash".into();
    }

    #[test]
    fn letters_land_in_the_hero_name_while_it_is_open() {
        let mut app = yard_app();
        open_rename(&mut app);
        for k in [KeyCode::KeyZ, KeyCode::KeyO, KeyCode::KeyE] {
            press(&mut app, k);
        }
        assert_eq!(
            app.world().resource::<HeroRename>().buffer,
            "Ashzoe",
            "each keystroke should land ONCE - two capture systems gave 'NNOoOo'"
        );
    }

    #[test]
    fn a_renamed_hero_keeps_the_name_locally_and_the_field_closes() {
        let mut app = yard_app();
        open_rename(&mut app);
        press(&mut app, KeyCode::KeyX);
        press(&mut app, KeyCode::Enter);
        let names = app.world().resource::<AccountHeroNames>();
        assert_eq!(names.names[0], "Ashx", "the name should stick without a run behind it");
        assert!(app.world().resource::<HeroRename>().slot.is_none(), "field should close");
    }

    /// Every letter has to reach the party-name field, including the "r" that used to
    /// open a rename dialog instead — a party called "Reapers" was untypeable.
    #[test]
    fn letters_name_the_party_when_no_hero_is_being_renamed() {
        let mut app = yard_app();
        for k in [KeyCode::KeyR, KeyCode::KeyE, KeyCode::KeyD] {
            press(&mut app, k);
        }
        assert_eq!(app.world().resource::<CityUi>().loadout_name, "red");
        assert!(
            app.world().resource::<HeroRename>().slot.is_none(),
            "typing a name must not open a rename dialog"
        );
    }

    /// The two fields must not both eat the same keystroke: typing a hero's name should
    /// never also be typing the party's, or one of them is always wrong.
    #[test]
    fn the_two_text_fields_never_share_a_keystroke() {
        let mut app = yard_app();
        open_rename(&mut app);
        for k in [KeyCode::KeyQ, KeyCode::KeyW] {
            press(&mut app, k);
        }
        assert_eq!(app.world().resource::<HeroRename>().buffer, "Ashqw");
        assert_eq!(
            app.world().resource::<CityUi>().loadout_name,
            "",
            "the loadout name must stay empty while a hero is being renamed"
        );
    }

    use super::*;
    use meld_client::net::VanguardLine;

    fn line(rank: i32, name: &str, d: i32) -> VanguardLine {
        VanguardLine { rank, username: name.to_string(), max_distance: d, ..Default::default() }
    }

    #[test]
    fn wall_text_covers_flickering_empty_and_ranked() {
        let mut board = VanguardBoardData::default();
        assert!(wall_view(&board).flat().contains("flickers awake"));

        board.loaded = true;
        board.season = 2;
        let empty = wall_view(&board).flat();
        assert!(empty.contains("Season 2"), "{empty}");
        assert!(empty.contains("No name carved"), "{empty}");

        board.entries = (1..=8).map(|i| line(i, &format!("digger{i}"), 900 - i * 10)).collect();
        board.you = Some(4);
        let lit = wall_view(&board).flat();
        assert!(lit.contains("[1] digger1 — d890"), "{lit}");
        // The wall has a column to itself now, so it shows the whole top ten rather than
        // the five that fitted on the city's one status line.
        assert!(lit.contains("[8] digger8"), "{lit}");
        assert!(lit.contains("You are #4"), "{lit}");

        board.you = None;
        assert!(wall_view(&board).flat().contains("uncarved"));
    }
}

/// The Apothecary's shelf as the city's one status line can carry it: name, price,
/// and what the player can currently afford (EC-2). Buying is `[1]`-`[4]`.
/// One actionable line on a counter: the key that runs it and the line itself.
///
/// `enabled` is advisory — the SERVER prices and refuses. A greyed row still says what it
/// would cost, because "you cannot afford this" is a decision and "nothing happened when I
/// pressed 5" is a bug report.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct CounterRow {
    pub(crate) key: String,
    pub(crate) label: String,
    pub(crate) enabled: bool,
    pub(crate) current: bool,
    /// The item kind this row is about, so the panel can put its icon in front of the
    /// words — its own sprite where we drew one, else a glyph for its type. `None` for a
    /// row that is a switch rather than a thing (the anvil's quench, a leaderboard entry).
    pub(crate) icon: Option<String>,
    /// What this row IS, for the detail column once it is picked: what the thing does, what
    /// a recipe needs, what the Broker is paying for.
    ///
    /// Every counter used to act on the press — a tap bought, sold or forged immediately —
    /// so the only way to find out what something did was to own it. A price with no
    /// description is not a decision.
    pub(crate) describe: Vec<String>,
    /// Chits per one of it. `0` for a row that is not priced (a recipe, a switch).
    pub(crate) unit_price: i64,
    /// Whether more than one can be had at once, and so whether the detail column offers
    /// an amount. A recipe makes one batch; a potion is bought by the handful.
    pub(crate) countable: bool,
    /// The most of it that can be had right now — chits for a buy, stock for a sell.
    pub(crate) max_qty: i32,
    /// The word on the button that commits it: `"Buy"`, `"Sell"`, `"Forge"`.
    pub(crate) verb: String,
}

impl CounterRow {
    fn new(key: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            label: label.into(),
            enabled: true,
            current: false,
            icon: None,
            describe: Vec::new(),
            unit_price: 0,
            countable: false,
            max_qty: 1,
            verb: "Confirm".into(),
        }
    }
    fn of(mut self, kind: impl Into<String>) -> Self {
        self.icon = Some(kind.into());
        self
    }
    fn dim(mut self) -> Self {
        self.enabled = false;
        self
    }
    fn cursor(mut self, on: bool) -> Self {
        self.current = on;
        self
    }
    /// What it does, in the player's words. One line per sentence.
    fn saying(mut self, lines: Vec<String>) -> Self {
        self.describe = lines;
        self
    }
    /// Priced, and how many of it are within reach.
    fn priced(mut self, unit: i64, max_qty: i32) -> Self {
        self.unit_price = unit;
        self.countable = max_qty > 1;
        self.max_qty = max_qty.max(1);
        self
    }
    /// The verb on its commit button.
    fn committed_by(mut self, verb: &str) -> Self {
        self.verb = verb.into();
        self
    }
}

/// The row the player has PICKED at the open counter, and how many of it they want.
///
/// A counter used to act on the press: a tap or a number key bought, sold or forged on the
/// spot. So nothing could describe what a thing did before you owned it, an amount was
/// always exactly one, and selling had no confirmation at all. Picking and committing are
/// two steps now, and the detail column is where the second one lives.
#[derive(Resource, Default)]
pub(crate) struct CounterPick {
    /// Index into the open [`CounterView`]'s rows.
    pub(crate) row: Option<usize>,
    pub(crate) qty: i32,
}

impl CounterPick {
    pub(crate) fn clear(&mut self) {
        self.row = None;
        self.qty = 1;
    }

    /// Pick a row, starting at one of it.
    pub(crate) fn pick(&mut self, row: usize) {
        self.row = Some(row);
        self.qty = 1;
    }

    /// Nudge the amount, held inside `1..=max`.
    pub(crate) fn nudge(&mut self, by: i32, max: i32) {
        self.qty = (self.qty + by).clamp(1, max.max(1));
    }
}

/// A counter as the three-column convention sees it: **nav | rows | detail**.
///
/// A view rather than a string, because a string can only be poured into one text node —
/// which is what made the counters read as scenery along the bottom of the screen instead
/// of as the menus they are. Rows as data is what lets each one be its own tappable chip
/// and gives the detail column something to say about the one under the cursor.
#[derive(Debug, Clone, PartialEq, Default)]
pub(crate) struct CounterView {
    pub(crate) title: String,
    /// What this counter is for, in plain words, under its name. A player who walked in
    /// on the strength of a chip should not have to press a row to find out what the
    /// room does.
    pub(crate) subtitle: String,
    pub(crate) nav: Vec<(String, bool)>,
    pub(crate) rows: Vec<CounterRow>,
    pub(crate) detail: Vec<String>,
    pub(crate) footer: Vec<String>,
}

impl CounterView {
    /// Everything the panel would draw, as one string. For tests — the panel itself walks
    /// the fields.
    #[cfg(test)]
    pub(crate) fn flat(&self) -> String {
        let mut out = self.title.clone();
        if !self.subtitle.is_empty() {
            out.push_str(&format!("\n{}", self.subtitle));
        }
        for (n, on) in &self.nav {
            out.push_str(&format!("\n{}{n}", if *on { "> " } else { "  " }));
        }
        for r in &self.rows {
            out.push_str(&format!(
                "\n{}{}{}{}",
                if r.current { "> " } else { "  " },
                if r.key.is_empty() { String::new() } else { format!("[{}] ", r.key) },
                r.label,
                if r.enabled { "" } else { " (locked)" }
            ));
        }
        for l in self.detail.iter().chain(self.footer.iter()) {
            out.push_str(&format!("\n{l}"));
        }
        out
    }
}

/// The Apothecary's counter: the shelf and the Requisition's plain gear on the buy side,
/// the Broker's quotes on the sell side.
pub(crate) fn shop_view(shop: &ShopData, inv: &InventoryData, selling: bool) -> CounterView {
    let mut v = CounterView {
        title: if selling { "The Broker".into() } else { shop.vendor.clone() },
        subtitle: if selling {
            "sell what you will never use".into()
        } else {
            "supplies and plain gear, for chits".into()
        },
        nav: vec![("Buy".into(), !selling), ("Sell".into(), selling)],
        footer: vec![format!(
            "{}   [E]/[ESC] leave",
            if selling { "[B] buy instead" } else { "[B] sell" }
        )],
        ..default()
    };
    if !shop.loaded {
        v.detail = vec!["The Apothecary is unpacking crates...".into()];
        return v;
    }
    v.detail.push(format!("{} chits", inv.chits));
    let held = |kind: &str| -> i32 {
        inv.materials.iter().find(|(k, _)| k == kind).map_or(0, |(_, q)| *q)
    };
    if selling {
        let rows = sellable(shop, inv);
        if rows.is_empty() {
            v.detail.push("There is nothing in the Vault it wants.".into());
            return v;
        }
        v.detail.push(
            "The Broker pays a floor, not a living — this is the answer to \"I will never \
             use this\"."
                .into(),
        );
        v.rows = rows
            .iter()
            .take(ITEM_ROWS + GEAR_ROWS)
            .enumerate()
            .map(|(i, (kind, price))| {
                let have = held(kind);
                CounterRow::new(
                    (i + 1).to_string(),
                    format!(
                        "{} x{have} @{price}c",
                        crate::icons::display_name(kind)
                    ),
                )
                .of(kind)
                .saying(vec![
                    material_blurb(kind),
                    format!("You hold {have}. The Broker pays {price}c each."),
                ])
                // Selling used to fire on the press, one unit, with no confirmation: a
                // mis-tap sold something and the only tell was the notice line.
                .priced(*price, have.min(SELL_QTY_CAP))
                .committed_by("Sell")
            })
            .collect();
        return v;
    }
    if shop.items.is_empty() {
        v.detail.push("The Apothecary has nothing on the shelf.".into());
        return v;
    }
    // Mark what the player cannot afford, so a price is a decision rather than a
    // rejection they discover by pressing a key.
    let afford = |price: i64| if inv.chits >= price { "" } else { " (short)" };
    for (i, s) in shop.items.iter().take(ITEM_ROWS).enumerate() {
        // The server has always SENT the description; the counter simply never showed it, so
        // every shelf read as a price list of names.
        let affordable = if s.price_chits > 0 {
            ((inv.chits / s.price_chits) as i32).min(BUY_QTY_CAP)
        } else {
            1
        };
        let row = CounterRow::new(
            (i + 1).to_string(),
            format!("{} {}c{}", s.name, s.price_chits, afford(s.price_chits)),
        )
        .of(s.item_kind.clone())
        .saying(vec![s.description.clone()])
        .priced(s.price_chits, affordable)
        .committed_by("Buy");
        v.rows.push(if inv.chits >= s.price_chits { row } else { row.dim() });
    }
    // The Requisition's plain gear shares the counter, on the keys after the items:
    // "spend chits so the next dive is easier" is one errand, not two.
    for (i, g) in shop.gear.iter().take(GEAR_ROWS).enumerate() {
        let stat = [("atk", g.atk), ("def", g.def), ("spd", g.spd)]
            .into_iter()
            .find(|(_, v)| *v > 0)
            .map(|(n, v)| format!(" +{v} {n}"))
            .unwrap_or_default();
        let stats = [("atk", g.atk), ("def", g.def), ("spd", g.spd)]
            .into_iter()
            .filter(|(_, v)| *v > 0)
            .map(|(n, v)| format!("+{v} {n}"))
            .collect::<Vec<_>>()
            .join("  ");
        let row = CounterRow::new(
            (ITEM_ROWS + i + 1).to_string(),
            format!("{}{} {}c{}", g.name, stat, g.price_chits, afford(g.price_chits)),
        )
        .of(g.slot.clone())
        .saying(vec![
            format!(
                "Plain {} for a {}. No affixes.",
                g.slot.replace('_', " "),
                g.class_key.replace('_', " ")
            ),
            if stats.is_empty() { "No bonuses.".into() } else { stats },
        ])
        // One piece at a time: a second copy of the same plain item is not a decision.
        .priced(g.price_chits, 1)
        .committed_by("Buy");
        v.rows.push(if inv.chits >= g.price_chits { row } else { row.dim() });
    }
    if !shop.gear.is_empty() {
        v.detail.push(format!(
            "Rows {}-{} are the Requisition: plain gear, no affixes, so the next run starts \
             dressed.",
            ITEM_ROWS + 1,
            ITEM_ROWS + shop.gear.len().min(GEAR_ROWS)
        ));
    }
    v
}

/// The Vanguard Wall as a counter with nothing to press: the season's deepest dives, best
/// first, with the reader's own placement called out (P1-1 — behaviors/endgame-seasons.md).
/// A clear time as minutes and seconds. The board stores milliseconds; nobody reads a fight
/// length in milliseconds.
fn clear_time(ms: i64) -> String {
    let secs = (ms / 1000).max(0);
    match (secs / 60, secs % 60) {
        (0, s) => format!("{s}s"),
        (m, s) => format!("{m}m {s:02}s"),
    }
}

pub(crate) fn wall_view(board: &VanguardBoardData) -> CounterView {
    let mut v = CounterView {
        title: "The Vanguard Wall".into(),
        subtitle: "who got deepest this season".into(),
        footer: vec!["[E]/[ESC] leave".into()],
        ..default()
    };
    if !board.loaded {
        v.detail = vec!["The Vanguard Wall flickers awake...".into()];
        return v;
    }
    v.nav = vec![(format!("Season {}", board.season), true)];
    if board.entries.is_empty() {
        v.detail = vec![
            "No name carved yet — the first to walk out and come back deep takes it.".into(),
        ];
        return v;
    }
    v.rows = board
        .entries
        .iter()
        .take(10)
        .map(|e| {
            // Click a name and the detail column reads out the RUN, not just the distance.
            // The endpoint has always sent every one of these; the Wall showed a name and a
            // number, so there was nothing to look at and no reason to press a row.
            let mut describe = vec![
                format!("Reached d{} at run level {}", e.max_distance, e.at_level.max(1)),
                format!("{} fights taken, {} fled", e.fights, e.flees),
            ];
            // Going quietly is a real way to travel (the Pacifist unlock), so a run that
            // touched nothing is worth saying out loud rather than reading as a blank.
            if e.fights == 0 {
                describe.push("Walked it without a single fight.".into());
            }
            if let Some(star) = &e.star {
                describe.push(match e.clear_ms {
                    Some(ms) if ms > 0 => {
                        format!("Felled the end fight ({star}) in {}", clear_time(ms))
                    }
                    _ => format!("Felled the end fight ({star})"),
                });
            }
            CounterRow::new(e.rank.to_string(), format!("{} — d{}", e.username, e.max_distance))
                .saying(describe)
                // Reading only: there is nothing to do to somebody else's posting.
                .committed_by("Close")
        })
        .collect();
    v.detail = vec![
        match board.you {
            Some(rank) => format!("You are #{rank}."),
            None => "You are uncarved.".into(),
        },
        "Press a name to read how that run went.".into(),
    ];
    v
}

/// The Bounty Board as a counter: every posted hunt, what it wants, how far along you
/// are, and what it pays (AD-4).
///
/// A finished hunt is a row you can press; everything else states its progress. The
/// numbers are all the server's — the panel never computes a reward or a completion.
pub(crate) fn hunts_view(board: &HuntBoardData) -> CounterView {
    let mut v = CounterView {
        title: "The Bounty Board".into(),
        subtitle: "go and do these; claim the reward here".into(),
        footer: vec!["[1]-[8] claim   [E]/[ESC] leave".into()],
        ..default()
    };
    if !board.loaded {
        v.detail = vec!["Someone is still pinning the contracts up...".into()];
        return v;
    }
    if board.hunts.is_empty() {
        v.detail = vec!["The board is bare.".into()];
        return v;
    }
    let claimable = board.hunts.iter().filter(|h| h.claimable).count();
    v.nav = vec![
        ("Hunts".into(), true),
        (
            if claimable > 0 {
                format!("{claimable} to claim")
            } else {
                "nothing to claim".into()
            },
            claimable > 0,
        ),
    ];
    if board.hunts.iter().any(|h| h.reward_gear) {
        v.footer.insert(0, "* also pays a piece of gear".into());
    }
    v.rows = board
        .hunts
        .iter()
        .enumerate()
        .map(|(i, h)| {
            let state = if h.claimed {
                " - paid".to_string()
            } else if h.claimable {
                " - CLAIM".to_string()
            } else if !h.accepted {
                " - not taken".to_string()
            } else {
                format!(" {}/{}", h.progress, h.target)
            };
            let mark = if h.reward_gear { " *" } else { "" };
            let mut describe = vec![h.objective.clone()];
            if !h.where_to_look.is_empty() {
                describe.push(h.where_to_look.clone());
            }
            let mut reward = format!("Pays {}c", h.reward_chits);
            if h.reward_material_qty > 0 && !h.reward_material.is_empty() {
                reward.push_str(&format!(
                    ", {} {}",
                    h.reward_material_qty,
                    crate::icons::display_name(&h.reward_material)
                ));
            }
            if h.reward_gear {
                reward.push_str(" and a piece of gear");
            }
            describe.push(reward);
            describe.push(if h.claimed {
                "Already paid.".to_string()
            } else if !h.accepted {
                "You have not taken this one - nothing you do counts toward it yet."
                    .to_string()
            } else {
                format!("Taken. {}/{}", h.progress, h.target)
            });
            describe.push(h.blurb.clone());
            let row = CounterRow::new((i + 1).to_string(), format!("{}{mark}{state}", h.name))
                .cursor(i == board.cursor)
                .saying(describe)
                .committed_by(if h.claimed {
                    "Paid"
                } else if h.claimable {
                    "Claim"
                } else if !h.accepted {
                    "Accept"
                } else {
                    "Close"
                });
            if h.claimed {
                row.dim()
            } else {
                row
            }
        })
        .collect();
    if let Some(h) = board.hunts.get(board.cursor) {
        let mut reward = format!("Pays {}c", h.reward_chits);
        if h.reward_material_qty > 0 && !h.reward_material.is_empty() {
            reward.push_str(&format!(
                ", {} {}",
                h.reward_material_qty,
                crate::icons::display_name(&h.reward_material)
            ));
        }
        // The piece is the reason to work a deep hunt, so it is on the row's own line
        // rather than buried at the end of the price.
        if h.reward_gear {
            reward.push_str(" and a piece of gear");
        }
        v.detail = vec![h.objective.clone(), reward];
        if !h.where_to_look.is_empty() {
            v.detail.push(h.where_to_look.clone());
        }
        v.detail.push(h.blurb.clone());
    }
    v
}

/// The Den's side of the Bounty Board: the contracts with your name on them (AD-4).
///
/// A felled mark is a row you can press; a standing one states where it is and how long is
/// left. This is the one place a bounty is paid, which is why the menu's Quests column is
/// reading-only.
pub(crate) fn bounty_view(board: &BountyData) -> CounterView {
    let mut v = CounterView {
        title: "The Bounty Board".into(),
        subtitle: "the Den's contracts, with your name on them".into(),
        footer: vec!["[1]-[8] claim   [B] posted hunts   [E]/[ESC] leave".into()],
        ..default()
    };
    if !board.loaded {
        v.detail = vec!["The Den is checking its ledger...".into()];
        return v;
    }
    let ready = board.active.iter().filter(|b| b.state == "completed").count();
    v.nav = vec![
        ("Hunts".into(), false),
        ("Bounties".into(), true),
        (
            if ready > 0 {
                format!("{ready} to claim")
            } else {
                format!("rank {} - {}", board.rank, board.rank_title)
            },
            ready > 0,
        ),
    ];
    if board.active.is_empty() {
        v.detail = vec!["The Den has nothing posted for you.".into()];
        return v;
    }
    v.rows = board
        .active
        .iter()
        .enumerate()
        .map(|(i, b)| {
            let done = b.state == "completed";
            let row = CounterRow::new(
                (i + 1).to_string(),
                format!("{}{}", b.mark_name, if done { " - CLAIM" } else { "" }),
            );
            if done {
                row
            } else {
                row.dim()
            }
        })
        .collect();
    let head = board.active.first();
    if let Some(b) = head {
        v.detail = vec![
            b.where_to_look.clone(),
            format!("Pays {}c and {} rank XP", b.reward_chits, b.reward_rank_xp),
        ];
    }
    v.detail.push(format!(
        "Hunter rank {} - {}. {} XP to the next.",
        board.rank, board.rank_title, board.rank_xp_to_next
    ));
    v
}

/// Ask the Den to pay for the contract on `row`, or say why it will not.
fn claim_bounty_row(net: &NetRes, city: &mut CityUi, board: &BountyData, row: usize) {
    let Some(b) = board.active.get(row) else { return };
    if b.state != "completed" {
        city.notice = format!("{} is still standing. {}", b.mark_name, b.where_to_look);
        return;
    }
    net.0.claim_bounty(b.bounty_id.clone());
    city.notice = format!("collecting on {}...", b.mark_name);
}

/// Ask the board to pay for the hunt on `row`, or say why it will not.
///
/// One path for the key and the tap: a row that claims by thumb but not by number is
/// the kind of split nobody notices until it is the reward that went missing.
fn claim_hunt_row(net: &NetRes, city: &mut CityUi, board: &HuntBoardData, row: usize) {
    let Some(h) = board.hunts.get(row) else { return };
    if h.claimed {
        city.notice = format!("{} is already paid.", h.name);
        return;
    }
    // A hunt you have not TAKEN is taken by this press. Every posted hunt used to credit
    // itself from the moment the account existed, so the board was eight jobs somebody had
    // signed you up for and "accept" meant nothing.
    if !h.accepted {
        net.0.accept_hunt(h.key.clone());
        city.notice = format!("took {}.", h.name);
        return;
    }
    if !h.claimable {
        city.notice = format!("{} - {}/{}. {}.", h.name, h.progress, h.target, h.objective);
        return;
    }
    net.0.claim_hunt(h.key.clone());
    city.notice = format!("claiming {}...", h.name);
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

/// The Forge & Alembic: the recipe book with the cursor on one row, then the anvil and the
/// bench as rows of their own. The server owns every gate, so a locked row says the level it
/// wants and an unaffordable one says what it is missing — before a keypress is spent on it.
///
/// The whole book fits in `main` now rather than a five-row window, because a column has
/// height where a status line had none.
pub(crate) fn craft_view(craft: &CraftData, inv: &InventoryData) -> CounterView {
    let mut v = CounterView {
        title: "The Forge & Alembic".into(),
        subtitle: "brew, smelt, forge, mend".into(),
        nav: vec![
            ("Recipes  up/down".into(), true),
            ("Anvil  S C F".into(), false),
            ("Bench  left/right R P".into(), false),
        ],
        footer: vec!["ENTER craft   [E]/[ESC] leave".into()],
        ..default()
    };
    if !craft.loaded {
        v.detail = vec!["The Forge & Alembic are warming up...".into()];
        return v;
    }
    let held = |kind: &str| -> i32 {
        inv.materials.iter().find(|(k, _)| k == kind).map_or(0, |(_, q)| *q)
    };
    if craft.recipes.is_empty() {
        v.detail = vec!["No recipes known.".into()];
    }
    for (i, r) in craft.recipes.iter().enumerate() {
        let short = r.inputs.iter().any(|(kind, need)| held(kind) < *need);
        let gate = if !r.craftable {
            format!("  (needs {} {})", r.skill, r.required_level)
        } else if short {
            "  (short)".to_string()
        } else {
            String::new()
        };
        let output = meld_proto::consumables::recipe(&r.recipe)
            .map_or_else(|| r.recipe.clone(), |d| d.output.to_string());
        // What the thing you are about to MAKE does. A recipe row named its inputs and its
        // skill and never once said what came out of it, so the book read as a list of costs.
        let mut describe = vec![format!("Makes {} x{}.", crate::icons::display_name(&output), r.output_quantity)];
        if let Some(def) = meld_proto::consumables::consumable(&output) {
            describe.push(def.description.to_string());
        }
        describe.push(format!("{} line.", r.skill));
        for (kind, need) in &r.inputs {
            describe.push(format!(
                "  {}/{need} {}",
                held(kind),
                crate::icons::display_name(kind)
            ));
        }
        if !r.craftable {
            describe.push(format!("Locked until {} {}.", r.skill, r.required_level));
        } else if short {
            describe.push("You are short of a material.".into());
        }
        let row = CounterRow::new(
            String::new(),
            format!("{} x{}{gate}", r.name, r.output_quantity),
        )
        .cursor(i == craft.cursor)
        .of(output)
        .saying(describe)
        // One batch at a time, and a confirm in front of it: forging used to fire on the
        // press and spend the materials before the player had read the row.
        .committed_by(if r.skill == "forging" { "Forge" } else { "Brew" });
        v.rows.push(if r.craftable && !short { row } else { row.dim() });
    }
    let stock = best_stock(inv, meld_proto::materials::MaterialClass::Refined);
    let anvil = stock.as_deref().unwrap_or("nothing refined");
    let quench = if craft.catalyze { "on" } else { "off" };
    v.rows.push(CounterRow::new("S", format!("slot: {}", FORGE_SLOTS[craft.slot])));
    v.rows.push(CounterRow::new("C", format!("quench: {quench}")));
    v.rows.push(CounterRow::new("F", format!("forge from {anvil}")).of(anvil));
    v.rows.push(CounterRow::new("left/right", bench_line(craft, inv).trim().to_string()));
    // The detail column belongs to whatever the cursor is on: which materials, how many of
    // each are already in the Vault, and what comes out. "1/2 dune_iron" is the whole
    // answer to "why is this row greyed out", and it needs room a status line never had.
    match craft.recipes.get(craft.cursor) {
        Some(r) => {
            v.detail.push(r.name.clone());
            v.detail.push(format!("Makes {} — {} line", r.output_quantity, r.skill));
            for (kind, need) in &r.inputs {
                v.detail.push(format!(
                    "  {}/{need} {}",
                    held(kind),
                    crate::icons::display_name(kind)
                ));
            }
            if !r.craftable {
                v.detail.push(format!("Locked until {} {}.", r.skill, r.required_level));
            }
        }
        None => v.detail.push(format!("{} chits", inv.chits)),
    }
    if !craft.last.is_empty() {
        v.detail.push(craft.last.clone());
    }
    v
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

/// Shelf rows the counter shows: items on `[1]`-`[4]`, plain gear on the keys after.
pub(crate) const ITEM_ROWS: usize = 4;
pub(crate) const GEAR_ROWS: usize = 4;

/// The most the amount stepper will offer, per side. These mirror the server's own limits
/// (`/v1/vendors/apothecary/buy` refuses past 99, `/v1/vendors/broker/sell` past 999), so the
/// counter cannot offer an amount the endpoint would reject.
pub(crate) const BUY_QTY_CAP: i32 = 99;
pub(crate) const SELL_QTY_CAP: i32 = 999;

/// What a material IS, for the detail column on the sell side. The Broker deals only in
/// materials, and the registry knows their class and tier — enough to say whether the thing
/// in your hand is ore, a reagent, refined stock or a trophy, which is what decides whether
/// you will ever want it back.
pub(crate) fn material_blurb(kind: &str) -> String {
    use meld_proto::materials::MaterialClass;
    match meld_proto::materials::material(kind).map(|m| m.class) {
        Some(MaterialClass::Ore) => "Raw ore. Smelts into refined stock for the Forge.".into(),
        Some(MaterialClass::Reagent) => "A reagent. Brews into potions at the alembic.".into(),
        Some(MaterialClass::Refined) => "Refined stock. What the Forge builds gear out of.".into(),
        Some(MaterialClass::Trophy) => {
            "A trophy off something you felled. Catalyses a forge, or brews a stronger dose."
                .into()
        }
        None => "The Broker will take it.".into(),
    }
}

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

    /// The Wall showed a name and a distance and nothing else, though the endpoint has always
    /// sent how the run got there. Pressing a name has to be worth doing.
    #[test]
    fn a_wall_row_reads_out_how_that_run_went() {
        let board = VanguardBoardData {
            loaded: true,
            season: 1,
            you: Some(4),
            entries: vec![meld_client::net::VanguardLine {
                rank: 1,
                username: "Ash".into(),
                max_distance: 1200,
                at_level: 94,
                fights: 0,
                flees: 2,
                star: Some("Unburied".into()),
                clear_ms: Some(185_000),
            }],
        };
        let v = wall_view(&board);
        let d = v.rows[0].describe.join("\n");
        assert!(d.contains("d1200"), "no distance: {d}");
        assert!(d.contains("run level 94"), "no level: {d}");
        assert!(d.contains("0 fights taken, 2 fled"), "no route: {d}");
        assert!(d.contains("without a single fight"), "a pacifist run should say so: {d}");
        assert!(d.contains("Unburied"), "no end-fight mark: {d}");
        assert!(d.contains("3m 05s"), "a clear time should read as minutes: {d}");
    }

    #[test]
    fn a_clear_time_reads_in_minutes_and_seconds() {
        assert_eq!(clear_time(0), "0s");
        assert_eq!(clear_time(45_000), "45s");
        assert_eq!(clear_time(185_000), "3m 05s");
        assert_eq!(clear_time(-5), "0s");
    }

    /// The server has always sent a description for every shelf line. The counter dropped
    /// it, so a shop was a list of names and prices and the only way to learn what something
    /// did was to buy it.
    #[test]
    fn a_shelf_row_carries_what_the_thing_does() {
        let mut shop = ShopData { loaded: true, ..Default::default() };
        shop.items.push(meld_client::net::ShopLine {
            item_kind: "bloom_salve".into(),
            name: "Bloom Salve".into(),
            description: "Restores a chunk of a hero's health.".into(),
            price_chits: 12,
        });
        let inv = InventoryData { chits: 100, loaded: true, ..Default::default() };
        let v = shop_view(&shop, &inv, false);
        let row = &v.rows[0];
        assert!(
            row.describe.iter().any(|l| l.contains("Restores a chunk")),
            "the row does not say what it does: {:?}",
            row.describe
        );
        assert_eq!(row.unit_price, 12);
        assert_eq!(row.verb, "Buy");
    }

    /// An amount is offered up to what the player can actually afford, and never past the
    /// endpoint's own ceiling — the counter must not offer a purchase the server refuses.
    #[test]
    fn the_amount_offered_is_what_you_can_afford() {
        let mut shop = ShopData { loaded: true, ..Default::default() };
        shop.items.push(line("bloom_salve", "Bloom Salve", 10));
        let v = shop_view(&shop, &InventoryData { chits: 35, loaded: true, ..Default::default() }, false);
        assert_eq!(v.rows[0].max_qty, 3, "35 chits buys three at 10c");
        assert!(v.rows[0].countable);

        // Rich enough to hit the server's cap.
        let v = shop_view(
            &shop,
            &InventoryData { chits: 100_000, loaded: true, ..Default::default() },
            false,
        );
        assert_eq!(v.rows[0].max_qty, BUY_QTY_CAP, "must not offer past what buy accepts");
    }

    /// Selling used to fire on the press, one unit, with no confirmation at all.
    #[test]
    fn a_sell_row_is_countable_up_to_what_you_hold_and_says_sell() {
        let mut shop = ShopData { loaded: true, ..Default::default() };
        shop.quotes.push(meld_client::net::BrokerQuote {
            item_kind: "bog_myrrh".into(),
            name: "Bog Myrrh".into(),
            price_chits: 4,
        });
        let inv = InventoryData {
            loaded: true,
            materials: vec![("bog_myrrh".to_string(), 7)],
            ..Default::default()
        };
        let v = shop_view(&shop, &inv, true);
        let row = &v.rows[0];
        assert_eq!(row.verb, "Sell");
        assert_eq!(row.max_qty, 7, "you can sell what you hold");
        assert_eq!(row.unit_price, 4);
        assert!(
            row.describe.iter().any(|l| l.contains("You hold 7")),
            "a sell row should say what you have: {:?}",
            row.describe
        );
    }

    /// The amount is held inside 1..=max however hard the stepper is pressed — a zero or a
    /// negative would be a request the server rejects, and past the max is chits you do not
    /// have.
    #[test]
    fn the_amount_stays_inside_one_and_the_maximum() {
        let mut pick = CounterPick::default();
        pick.pick(2);
        assert_eq!((pick.row, pick.qty), (Some(2), 1), "a fresh pick starts at one");
        pick.nudge(-5, 9);
        assert_eq!(pick.qty, 1, "never below one");
        pick.nudge(100, 9);
        assert_eq!(pick.qty, 9, "never past the maximum");
        pick.clear();
        assert!(pick.row.is_none());
    }

    /// The three-column convention only reads as three columns if all three have something
    /// in them. A counter that leaves nav or detail blank collapses back into "a list of
    /// rows", which is the shape this replaced.
    #[test]
    fn every_counter_fills_all_three_columns() {
        let shop = ShopData {
            loaded: true,
            vendor: "The Apothecary".into(),
            items: vec![line("bloom_salve", "Bloom Salve", 25)],
            ..Default::default()
        };
        let inv = InventoryData { chits: 40, ..Default::default() };
        let craft = CraftData {
            loaded: true,
            recipes: vec![recipe("Bloom Salve", 1, true, &[("bloom_herb", 2)])],
            ..Default::default()
        };
        let board = VanguardBoardData { loaded: true, season: 1, ..Default::default() };
        for (name, v) in [
            ("shop/buy", shop_view(&shop, &inv, false)),
            ("shop/sell", shop_view(&shop, &inv, true)),
            ("forge", craft_view(&craft, &inv)),
            ("wall", wall_view(&board)),
        ] {
            assert!(!v.title.is_empty(), "{name} has no title");
            assert!(!v.nav.is_empty(), "{name} has an empty nav column");
            assert!(!v.detail.is_empty(), "{name} has an empty detail column");
            // Exactly one nav chip reads as selected, or the column stops saying where
            // you are — which is the other half of its job.
            assert_eq!(
                v.nav.iter().filter(|(_, on)| *on).count(),
                1,
                "{name} nav does not mark exactly one side as current: {:?}",
                v.nav
            );
        }
    }

    #[test]
    fn the_shelf_prices_every_row_and_flags_what_you_cannot_afford() {
        let mut shop = ShopData::default();
        let mut inv = InventoryData::default();
        assert!(shop_view(&shop, &inv, false).flat().contains("unpacking"));

        shop.loaded = true;
        assert!(shop_view(&shop, &inv, false).flat().contains("nothing on the shelf"));

        shop.vendor = "The Apothecary".into();
        shop.items = vec![line("bloom_salve", "Bloom Salve", 25), line("town_portal", "Town Portal", 60)];
        inv.chits = 30;
        let text = shop_view(&shop, &inv, false).flat();
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
        assert!(craft_view(&craft, &inv).flat().contains("warming up"));

        craft.loaded = true;
        assert!(craft_view(&craft, &inv).flat().contains("No recipes known"));

        craft.recipes = vec![
            recipe("Bloom Salve", 1, true, &[("bloom_herb", 2)]),
            recipe("Quintessence", 9, false, &[("bog_ichor", 1)]),
        ];
        inv.materials = vec![("bloom_herb".to_string(), 1)];
        let text = craft_view(&craft, &inv).flat();
        // Have/need per input is the whole answer to "what am I missing".
        assert!(text.contains("1/2 Bloom Herb"), "{text}");
        assert!(text.contains("(short)"), "{text}");
        // A locked row names the level rather than just refusing later.
        assert!(text.contains("needs alchemy 9"), "{text}");
        // The cursor is visible, and moves.
        assert!(text.contains("> Bloom Salve"), "{text}");
        craft.cursor = 1;
        assert!(craft_view(&craft, &inv).flat().contains("> Quintessence"), "{text}");

        // Enough material and the row stops complaining — and stops being greyed out,
        // which is the part the player actually acts on.
        craft.cursor = 0;
        inv.materials = vec![("bloom_herb".to_string(), 5)];
        let view = craft_view(&craft, &inv);
        assert!(view.flat().contains("5/2 Bloom Herb"), "{}", view.flat());
        let salve = view.rows.iter().find(|r| r.label.starts_with("Bloom Salve")).unwrap();
        assert!(salve.enabled, "a stocked, unlocked recipe is not greyed: {salve:?}");
        assert!(!salve.label.contains("(short)"), "{salve:?}");
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

        let craft = CraftData {
            loaded: true,
            recipes: vec![recipe("Bloom Salve", 1, true, &[("bloom_herb", 2)])],
            ..Default::default()
        };
        let text = craft_view(&craft, &inv).flat();
        assert!(text.contains("nothing refined"), "the anvil should say it is empty: {text}");
        assert!(text.contains("[F] forge from"), "{text}");
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
        let empty = shop_view(&shop, &inv, true).flat();
        assert!(empty.contains("nothing in the Vault it wants"), "{empty}");
        assert!(shop_view(&shop, &inv, true).rows.is_empty(), "no rows to press");

        inv.chits = 12;
        inv.materials = vec![
            ("bloom_herb".to_string(), 4),
            ("bog_ichor".to_string(), 2),
            ("mystery_rock".to_string(), 9), // not a material the Broker quotes
        ];
        let text = shop_view(&shop, &inv, true).flat();
        assert!(text.contains("The Broker"), "{text}");
        // Richest first, with the stack you hold and the price each.
        assert!(text.contains("[1] Bog Ichor x2 @66c"), "{text}");
        assert!(text.contains("[2] Bloom Herb x4 @5c"), "{text}");
        // A price for something you do not carry is noise.
        assert!(!text.to_lowercase().contains("mystery"), "{text}");
        // And the way back is on the row.
        assert!(text.contains("[B] buy instead"), "{text}");
        assert!(sellable(&shop, &inv).len() == 2);

        // The buy side advertises the other half too.
        shop.items = vec![line("bloom_salve", "Bloom Salve", 25)];
        assert!(shop_view(&shop, &inv, false).flat().contains("[B] sell"));
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
        assert!(craft_view(&craft, &inv).flat().contains("nothing in the Vault"));
        assert!(bench_gear(&craft, &inv).is_none());

        inv.gear = vec![
            bench_piece("g1", "Worn Warblade", 6, 10),
            bench_piece("g2", "Issued Cuirass", 10, 10),
        ];
        let text = craft_view(&craft, &inv).flat();
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
        let mut inv = InventoryData {
            gear: vec![bench_piece_of("insured", 2, "g", "Wearing Blade", 8, 12)],
            ..Default::default()
        };
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
        let text = shop_view(&shop, &inv, false).flat();
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
        assert!(shop_view(&shop, &inv, false).flat().contains("220c (short)"));
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
    // Whatever the saved composition turned out to be, the party may never be WIDER than
    // the slots this account has earned. The newcomer default is four classes (a spread,
    // so the builder shows what the game has in it), and nothing trimmed it when no saved
    // composition came back — so a player holding one slot was shown four heroes
    // everywhere the client draws `session.party`: the Vault-Deep's party strip and the
    // overworld entourage both listed people who do not exist. The server clamps the same
    // way on `enter_maze`, so this is the client agreeing with it rather than a new rule.
    if session.party.len() > slots {
        session.party.truncate(slots);
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
        city.notice = "Muster a party before your run.".to_string();
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
    // NO bare [R] shortcut here, deliberately. The yard is the one screen in town with
    // a text field in it, and a letter key that also opens a dialog means a party named
    // "Reapers" can never be typed — the R opened a rename instead, and typed an "r"
    // into the name field on the way. The visible Rename button is the way in; the
    // in-dive party screen keeps its [R], where there is no field to type into.
    let _ = &keys;
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

/// The number keys that travel, one per district in order. A module const rather than a
/// local so a test can hold it against `CITY_DISTRICTS` — the column advertises these keys,
/// and a district past the end of the list would silently have none. (It already did: this
/// started as `[1]-[6]` against seven districts.)
pub(crate) const TRAVEL_KEYS: [KeyCode; 7] = [
    KeyCode::Digit1,
    KeyCode::Digit2,
    KeyCode::Digit3,
    KeyCode::Digit4,
    KeyCode::Digit5,
    KeyCode::Digit6,
    KeyCode::Digit7,
];

/// A district chip in the town's travel column: click it to go there and open it.
#[derive(Component, Clone, Copy)]
pub(crate) struct TravelButton(pub usize);

/// Marker for the travel column, rebuilt each frame.
#[derive(Component)]
pub(crate) struct TravelColumn;

/// The town's nav: one chip per district, in the same 1/6 column the menu's nav uses.
///
/// Town used to be walk-only — every counter meant crossing the plaza to stand in a radius
/// and press [E], and nothing on screen said where the shops were. This is the "button to
/// quickly go to them", and it inherits the three-column convention rather than inventing
/// another panel shape.
pub(crate) fn render_travel_column(
    mut commands: Commands,
    city: Res<CityUi>,
    session: Res<Session>,
    tutorial: Res<Tutorial>,
    old: Query<Entity, With<TravelColumn>>,
    root_q: Query<Entity, With<CityRoot>>,
) {
    for e in &old {
        commands.entity(e).despawn();
    }
    // The Drill Yard is modal (it swallows letters for the name fields), a dive is
    // underway, or a counter is open and holding the same left third for its own nav — and
    // `travel_keys` already stands down for all three, so the column has to as well or it
    // advertises numbers that do nothing.
    if city.party_open
        || city.any_counter_open()
        || session.entered
    {
        return;
    }
    // The town tour highlights this column by brightening/thickening ITS OWN
    // border rather than drawing an approximated overlay box on top — since
    // this whole node is rebuilt fresh every frame anyway, the highlight is
    // guaranteed to bound exactly what's actually rendered, however many
    // districts the list holds.
    let highlighted = tutorial.town_step == Some(crate::tutorial::TRAVEL_COLUMN_STEP);
    let Ok(root) = root_q.single() else { return };
    commands.entity(root).with_children(|p| {
        p.spawn((
            TravelColumn,
            Node {
                position_type: PositionType::Absolute,
                left: Val::Px(18.0),
                top: Val::Px(90.0),
                width: Val::Percent(glass::COL_NAV),
                min_width: Val::Px(172.0),
                flex_direction: FlexDirection::Column,
                row_gap: Val::Px(6.0),
                padding: UiRect::all(Val::Px(14.0)),
                border: UiRect::all(Val::Px(if highlighted { 3.0 } else { 1.0 })),
                ..default()
            },
            BackgroundColor(glass::GLASS),
            BorderColor(if highlighted { glass::ACTIVE_EDGE } else { glass::EDGE }),
            BorderRadius::all(Val::Px(10.0)),
        ))
        .with_children(|col| {
            col.spawn(glass::text("THE LAST CITY", 19.0, glass::TITLE));
            col.spawn(glass::divider());
            for (i, d) in CITY_DISTRICTS.iter().enumerate() {
                // The one you are standing in reads as selected, so the column doubles as
                // "where am I" — the same job the menu's nav does.
                let here = city.near == Some(i);
                col.spawn((Button, TravelButton(i), glass::row_chip(here)))
                    .with_children(|b| {
                        b.spawn(Node {
                            flex_direction: FlexDirection::Column,
                            row_gap: Val::Px(2.0),
                            ..default()
                        })
                        .with_children(|lines| {
                            lines.spawn(glass::text(
                                format!("{}  {}", i + 1, d.label),
                                16.0,
                                if here { glass::TITLE } else { glass::TEXT },
                            ));
                            lines.spawn(glass::text(d.purpose, 13.0, glass::DIM));
                        });
                    });
            }
            col.spawn(glass::text(
                format!("[1]-[{}] go   [E] use", CITY_DISTRICTS.len()),
                13.0,
                glass::DIM,
            ));
        });
    });
}

/// A small, quiet nameplate floating above each district's world position — passive
/// signage, not a menu: unlike [`render_travel_column`] this never hides (it isn't
/// interactive and doesn't compete for the same screen space as a counter), so
/// walking past always tells you what you're looking at. Rebuilt every frame from
/// each district's fixed anchor, projected to screen space the same way
/// `overworld::update_mob_nameplates` floats a plate over a mob's head.
pub(crate) fn render_district_nameplates(
    mut commands: Commands,
    cam_q: Query<(&Camera, &GlobalTransform), With<Camera3d>>,
    root_q: Query<Entity, With<DistrictNameplateRoot>>,
    old: Query<Entity, With<DistrictNameplate>>,
) {
    for e in &old {
        commands.entity(e).despawn();
    }
    let Ok((cam, cam_tf)) = cam_q.single() else { return };
    let Ok(root) = root_q.single() else { return };
    commands.entity(root).with_children(|p| {
        for d in CITY_DISTRICTS {
            // A fixed height above the district's ground anchor — a little higher
            // than a mob's head-height plate since a district is a structure, not
            // a creature.
            let anchor = Vec3::new(d.x, 3.0, d.z);
            let Ok(s) = cam.world_to_viewport(cam_tf, anchor) else {
                continue; // behind the camera or otherwise unprojectable
            };
            p.spawn((
                DistrictNameplate,
                Node {
                    position_type: PositionType::Absolute,
                    left: Val::Px(s.x),
                    top: Val::Px(s.y),
                    padding: UiRect::axes(Val::Px(8.0), Val::Px(3.0)),
                    border: UiRect::all(Val::Px(1.0)),
                    ..default()
                },
                BackgroundColor(glass::GLASS_THIN),
                BorderColor(glass::EDGE_SOFT),
                BorderRadius::all(Val::Px(6.0)),
            ))
            .with_children(|b| {
                // Name over purpose, the same pairing the nav chip and the walk-up prompt
                // use: a plate that reads "The Drill Yard" and stops has told a new player
                // the one thing they already knew — that it is called something.
                b.spawn(Node {
                    flex_direction: FlexDirection::Column,
                    align_items: AlignItems::Center,
                    row_gap: Val::Px(1.0),
                    ..default()
                })
                .with_children(|lines| {
                    lines.spawn(glass::text(d.label, 12.0, glass::TEXT));
                    lines.spawn(glass::text(d.purpose, 10.0, glass::DIM));
                });
            });
        }
    });
}

/// Walk the avatar to a district and open it. Clicking the chip and pressing its number are
/// the same path, so they can never disagree.
/// Open (or close) whatever the district at `i` offers — the shelf, the recipe book, the
/// Wall, the board, the yard.
///
/// ONE place, because there are now two ways in: pressing [E] while standing in a district,
/// and clicking its travel chip a second time. A player who arrived by clicking a chip has
/// no reason to know about [E] at all, and the district they are standing in is the one the
/// chip already put them in.
pub(crate) fn toggle_district(
    i: usize,
    city: &mut CityUi,
    net: &NetRes,
    craft: &mut CraftData,
    pick: &mut CounterPick,
) {
    // Opening or closing ANY counter drops whatever was picked at the last one: the pick is a
    // row INDEX, and row 3 of the shelf is not row 3 of the recipe book.
    pick.clear();
    match CITY_DISTRICTS[i].action {
        // Diving and the Vault are handled by their own callers (a dive changes screen; the
        // Vault opens the shared inventory overlay).
        CityAction::Dive | CityAction::Vault => {}
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
                // Every half of the counter: the Apothecary's basics, the Requisition's
                // plain gear, and what the Broker pays for what you carried home.
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
        CityAction::Hunts => {
            city.hunts_open = !city.hunts_open;
            if city.hunts_open {
                city.notice.clear();
                net.0.fetch_hunts();
                net.0.fetch_bounties();
            }
        }
    }
}

pub(crate) fn travel_to(
    i: usize,
    city: &mut CityUi,
    tf: &mut Transform,
) {
    let Some(d) = CITY_DISTRICTS.get(i) else { return };
    // Stand just inside the district's radius, facing in — close enough that the existing
    // proximity check lights it up and [E] works exactly as if you had walked.
    tf.translation.x = d.x;
    tf.translation.z = d.z + (d.radius * 0.5);
    city.near = Some(i);
}

/// Clicks on the travel column.
pub(crate) fn travel_click(
    q: Query<(&Interaction, &TravelButton), Changed<Interaction>>,
    net: NonSend<NetRes>,
    mut city: ResMut<CityUi>,
    mut craft: ResMut<CraftData>,
    mut pick: ResMut<CounterPick>,
    mut player: Query<&mut Transform, With<CityPlayer>>,
) {
    let Ok(mut tf) = player.single_mut() else { return };
    for (interaction, btn) in &q {
        if *interaction != Interaction::Pressed {
            continue;
        }
        // Click a district you are ALREADY standing in and it opens — so a second click on
        // the same chip is what a double-click means, with no timer to guess at. Travel used
        // to land you inside the radius and then wait for an [E] the player had never been
        // told about; a chip that takes you to a shop and does not open it is a door you
        // have to knock on twice for no reason.
        if city.near == Some(btn.0) {
            toggle_district(btn.0, &mut city, &net, &mut craft, &mut pick);
            continue;
        }
        travel_to(btn.0, &mut city, &mut tf);
    }
}

/// [1]-[6] travel to a district — the keyboard twin of the column.
pub(crate) fn travel_keys(
    keys: Res<ButtonInput<KeyCode>>,
    mut city: ResMut<CityUi>,
    session: Res<Session>,
    mut player: Query<&mut Transform, With<CityPlayer>>,
) {
    // While a counter is open the number keys buy things, and the yard types names.
    if city.party_open
        || city.any_counter_open()
        || session.entered
    {
        return;
    }
    let Ok(mut tf) = player.single_mut() else { return };
    for (i, k) in TRAVEL_KEYS.iter().enumerate() {
        if keys.just_pressed(*k) {
            travel_to(i, &mut city, &mut tf);
            return;
        }
    }
}

/// Marker for the counter panel, rebuilt each frame.
#[derive(Component)]
pub(crate) struct CounterPanel;

/// A row chip on the counter: pressing it does whatever its key does.
#[derive(Component, Clone, Copy)]
pub(crate) struct CounterRowButton(pub usize);

/// A nav chip on the counter. The shop's Buy/Sell sides are the only two, and they are
/// each other's only alternative, so the chip that was pressed does not matter yet.
#[derive(Component, Clone, Copy)]
pub(crate) struct CounterNavButton;

/// Nudge the amount on the picked row: `-1` or `+1`.
#[derive(Component, Clone, Copy)]
pub(crate) struct CounterQtyButton(pub i32);

/// Commit the picked row — buy, sell or forge it, for the amount chosen.
#[derive(Component, Clone, Copy)]
pub(crate) struct CounterCommitButton;

/// Drop the pick without doing anything.
#[derive(Component, Clone, Copy)]
pub(crate) struct CounterCancelButton;

/// The counter's way OUT, at the foot of its nav column.
///
/// Spawned by [`render_counter_panel`] for every counter rather than by each `*_view`
/// builder, because a per-builder list is a list the next counter gets left off — which is
/// exactly how the claims screen shipped with no exit at all. A player who arrived by
/// clicking a travel chip has no reason to know that [Esc] is the way back, and travel
/// lands you INSIDE the district radius, so walking away does not close it either.
#[derive(Component, Clone, Copy)]
pub(crate) struct CounterCloseButton;

/// Draw whichever counter is open as **nav | main | detail**, centred.
///
/// A shop is a menu, so it gets the menu treatment: the same three columns at the same
/// 1/6, 1/2, 1/3 as everything else, in the middle of the screen where the player is
/// already looking. Poured into the city's bottom status strip it was a wall of text
/// running off both edges, and it read as scenery rather than as something to use.
pub(crate) fn render_counter_panel(
    mut commands: Commands,
    city: Res<CityUi>,
    session: Res<Session>,
    inv: Res<InventoryData>,
    shop: Res<ShopData>,
    shop_selling: Res<ShopSelling>,
    craft: Res<CraftData>,
    board: Res<VanguardBoardData>,
    hunts: Res<HuntBoardData>,
    bounties: Res<BountyData>,
    pick: Res<CounterPick>,
    wa: Option<Res<WorldAssets>>,
    old: Query<Entity, With<CounterPanel>>,
    root_q: Query<Entity, With<CityRoot>>,
) {
    for e in &old {
        commands.entity(e).despawn();
    }
    if session.entered || city.party_open {
        return;
    }
    let view = if city.craft_open {
        craft_view(&craft, &inv)
    } else if city.shop_open {
        shop_view(&shop, &inv, shop_selling.0)
    } else if city.board_open {
        wall_view(&board)
    } else if city.hunts_open {
        if city.bounty_tab {
            bounty_view(&bounties)
        } else {
            hunts_view(&hunts)
        }
    } else {
        return;
    };
    let Ok(root) = root_q.single() else { return };
    commands.entity(root).with_children(|p| {
        p.spawn((
            CounterPanel,
            Node {
                position_type: PositionType::Absolute,
                top: Val::Px(90.0),
                left: Val::Px(0.0),
                width: Val::Vw(100.0),
                ..default()
            },
        ))
        .with_children(|scrim| {
            scrim.spawn(glass::columns()).with_children(|cols| {
                cols.spawn(glass::column(glass::COL_NAV)).with_children(|nav| {
                    nav.spawn(glass::text(view.title.clone(), 19.0, glass::TITLE));
                    if !view.subtitle.is_empty() {
                        nav.spawn(glass::text(view.subtitle.clone(), 12.0, glass::DIM));
                    }
                    nav.spawn(glass::divider());
                    for (label, on) in view.nav.iter() {
                        nav.spawn((Button, CounterNavButton, glass::row_chip(*on))).with_children(
                            |b| {
                                b.spawn(glass::text(
                                    label.clone(),
                                    15.0,
                                    if *on { glass::TITLE } else { glass::TEXT },
                                ));
                            },
                        );
                    }
                    nav.spawn(glass::divider());
                    nav.spawn((Button, CounterCloseButton, glass::row_chip(false)))
                        .with_children(|b| {
                            b.spawn(glass::text("Leave  [Esc]".to_string(), 15.0, glass::TEXT));
                        });
                });
                cols.spawn(glass::column(glass::COL_MAIN)).with_children(|main| {
                    for (i, r) in view.rows.iter().enumerate() {
                        // Every row is its own chip, so the counter is as usable by thumb as
                        // by number key — the same rule the over-head prompts follow.
                        main.spawn((Button, CounterRowButton(i), glass::row_chip(r.current)))
                            .with_children(|b| {
                                if let Some(kind) = &r.icon {
                                    crate::icons::spawn_icon(b, wa.as_deref(), kind, 22.0);
                                }
                                let label = if r.key.is_empty() {
                                    r.label.clone()
                                } else {
                                    format!("[{}]  {}", r.key, r.label)
                                };
                                b.spawn(glass::text(
                                    label,
                                    16.0,
                                    if !r.enabled {
                                        glass::DIM
                                    } else if r.current {
                                        glass::TITLE
                                    } else {
                                        glass::TEXT
                                    },
                                ));
                            });
                    }
                    for line in &view.footer {
                        main.spawn(glass::text(line.clone(), 13.0, glass::DIM));
                    }
                });
                if view.detail.is_empty() {
                    cols.spawn(glass::column_empty(glass::COL_DETAIL));
                } else {
                    cols.spawn(glass::column(glass::COL_DETAIL)).with_children(|d| {
                        // A PICKED row owns the detail column: what it is, how many, and the
                        // two buttons that commit or back out. Nothing at a counter acts on
                        // the press any more, so this column is the whole second step.
                        // A row you cannot afford is still a row you are allowed to READ.
                        // Filtering the pick by `enabled` meant a broke player could not find
                        // out what anything did — which is the whole complaint this column
                        // exists to answer. Only the COMMIT stands down.
                        let picked = pick.row.and_then(|i| view.rows.get(i));
                        if let Some(row) = picked {
                            d.spawn(glass::text(row.label.clone(), 17.0, glass::TITLE));
                            for line in &row.describe {
                                d.spawn(glass::text(line.clone(), 14.0, glass::TEXT));
                            }
                            d.spawn(glass::divider());
                            if row.countable && row.enabled {
                                d.spawn(Node {
                                    flex_direction: FlexDirection::Row,
                                    align_items: AlignItems::Center,
                                    column_gap: Val::Px(8.0),
                                    ..default()
                                })
                                .with_children(|qty| {
                                    qty.spawn((Button, CounterQtyButton(-1), glass::chip(false)))
                                        .with_children(|b| {
                                            b.spawn(glass::text("-".to_string(), 17.0, glass::TEXT));
                                        });
                                    qty.spawn(glass::text(
                                        format!("{}", pick.qty),
                                        17.0,
                                        glass::TITLE,
                                    ));
                                    qty.spawn((Button, CounterQtyButton(1), glass::chip(false)))
                                        .with_children(|b| {
                                            b.spawn(glass::text("+".to_string(), 17.0, glass::TEXT));
                                        });
                                    qty.spawn(glass::text(
                                        format!("of {}", row.max_qty),
                                        13.0,
                                        glass::DIM,
                                    ));
                                });
                            }
                            if row.unit_price > 0 {
                                d.spawn(glass::text(
                                    format!(
                                        "{}c each - {}c for {}",
                                        row.unit_price,
                                        row.unit_price * pick.qty as i64,
                                        pick.qty
                                    ),
                                    14.0,
                                    glass::TEXT,
                                ));
                            }
                            d.spawn(Node {
                                flex_direction: FlexDirection::Row,
                                column_gap: Val::Px(8.0),
                                ..default()
                            })
                            .with_children(|act| {
                                if row.enabled {
                                    act.spawn((Button, CounterCommitButton, glass::chip(true)))
                                        .with_children(|b| {
                                            b.spawn(glass::text(
                                                row.verb.clone(),
                                                16.0,
                                                glass::TITLE,
                                            ));
                                        });
                                } else {
                                    // No button at all rather than a dead one: a chip in this
                                    // UI is a promise that pressing it does something.
                                    act.spawn(glass::text(
                                        "Out of reach for now.".to_string(),
                                        14.0,
                                        glass::DIM,
                                    ));
                                }
                                act.spawn((Button, CounterCancelButton, glass::chip(false)))
                                    .with_children(|b| {
                                        b.spawn(glass::text(
                                            if row.enabled { "Cancel" } else { "Back" }.to_string(),
                                            16.0,
                                            glass::TEXT,
                                        ));
                                    });
                            });
                            return;
                        }
                        for (i, line) in view.detail.iter().enumerate() {
                            let (size, colour) =
                                if i == 0 { (17.0, glass::TITLE) } else { (14.0, glass::TEXT) };
                            d.spawn(glass::text(line.clone(), size, colour));
                        }
                    });
                }
            });
        });
    });
}

/// A tap on a counter row, or on a nav chip — the mouse twin of the counter's keys.
///
/// Deliberately the same dispatch the keys use rather than a second path: a row that buys
/// something different by thumb than by key is the sort of thing nobody notices until a
/// player buys the wrong gear.
#[allow(clippy::too_many_arguments)]
pub(crate) fn counter_click(
    net: NonSend<NetRes>,
    mut city: ResMut<CityUi>,
    inv: Res<InventoryData>,
    shop: Res<ShopData>,
    mut shop_selling: ResMut<ShopSelling>,
    mut craft: ResMut<CraftData>,
    mut hunts: ResMut<HuntBoardData>,
    rows: Query<(&Interaction, &CounterRowButton), Changed<Interaction>>,
    navs: Query<(&Interaction, &CounterNavButton), Changed<Interaction>>,
    closes: Query<(&Interaction, &CounterCloseButton), Changed<Interaction>>,
    qtys: Query<(&Interaction, &CounterQtyButton), Changed<Interaction>>,
    commits: Query<(&Interaction, &CounterCommitButton), Changed<Interaction>>,
    cancels: Query<(&Interaction, &CounterCancelButton), Changed<Interaction>>,
    mut pick: ResMut<CounterPick>,
) {
    for (interaction, _) in &closes {
        if *interaction == Interaction::Pressed {
            pick.clear();
            city.close_counters();
            return;
        }
    }
    for (interaction, _) in &navs {
        if *interaction == Interaction::Pressed && city.shop_open {
            shop_selling.0 = !shop_selling.0;
            // The two sides hold different things on the same row numbers, so a pick made on
            // one side must not survive the flip.
            pick.clear();
            return;
        }
    }
    for (interaction, _) in &cancels {
        if *interaction == Interaction::Pressed {
            pick.clear();
            return;
        }
    }
    for (interaction, btn) in &qtys {
        if *interaction == Interaction::Pressed {
            let max = counter_pick_max(&city, &shop, &inv, &craft, &shop_selling, &pick);
            pick.nudge(btn.0, max);
            return;
        }
    }
    for (interaction, _) in &commits {
        if *interaction == Interaction::Pressed {
            commit_counter_pick(
                &net,
                &mut city,
                &shop,
                &inv,
                &mut craft,
                &hunts,
                &shop_selling,
                &mut pick,
            );
            return;
        }
    }
    for (interaction, btn) in &rows {
        if *interaction != Interaction::Pressed {
            continue;
        }
        // A row PICKS. Every counter used to act on the press — buying, selling and forging
        // all fired on the tap — so nothing could tell you what a thing did before you owned
        // it, and a mis-tap spent chits or materials with no way back. The detail column
        // holds the second step.
        pick.pick(btn.0);
        // Keep each counter's own cursor on the picked row, since the detail those panels
        // build for themselves (a hunt's objective, a recipe's inputs) reads from it.
        if city.craft_open && btn.0 < craft.recipes.len() {
            craft.cursor = btn.0;
        }
        if city.hunts_open && btn.0 < hunts.hunts.len() {
            hunts.cursor = btn.0;
        }
        return;
    }
}

/// The most of the picked row that can be had right now — asked of the view, so the stepper
/// and the rows can never disagree about the ceiling.
fn counter_pick_max(
    city: &CityUi,
    shop: &ShopData,
    inv: &InventoryData,
    craft: &CraftData,
    selling: &ShopSelling,
    pick: &CounterPick,
) -> i32 {
    let view = if city.craft_open {
        craft_view(craft, inv)
    } else if city.shop_open {
        shop_view(shop, inv, selling.0)
    } else {
        return 1;
    };
    pick.row.and_then(|i| view.rows.get(i)).map_or(1, |r| r.max_qty)
}

/// Do the thing the picked row says, for the amount chosen, then drop the pick.
///
/// ONE place, so the mouse and the keyboard commit identically — a row that buys something
/// different by thumb than by key is the sort of thing nobody notices until a player buys the
/// wrong gear.
#[allow(clippy::too_many_arguments)]
fn commit_counter_pick(
    net: &NetRes,
    city: &mut CityUi,
    shop: &ShopData,
    inv: &InventoryData,
    craft: &mut CraftData,
    hunts: &HuntBoardData,
    selling: &ShopSelling,
    pick: &mut CounterPick,
) {
    let Some(idx) = pick.row else { return };
    let qty = pick.qty.max(1);
    if city.craft_open {
        if let Some(r) = craft.recipes.get(idx) {
            if r.craftable {
                net.0.craft(r.recipe.clone());
                craft.last = format!("working {}...", r.name);
            }
        }
        pick.clear();
        return;
    }
    if city.hunts_open {
        claim_hunt_row(net, city, hunts, idx);
        pick.clear();
        return;
    }
    if city.shop_open {
        if selling.0 {
            if let Some((kind, price)) = sellable(shop, inv).get(idx) {
                net.0.sell_material(kind.clone(), qty);
                city.notice = format!("sold {qty} {kind} for {}c", price * qty as i64);
            }
        } else if idx < ITEM_ROWS {
            if let Some(line) = shop.items.get(idx) {
                net.0.buy_item(line.item_kind.clone(), qty);
                city.notice = format!("bought {qty} x {}", line.name);
            }
        } else if let Some(g) = shop.gear.get(idx - ITEM_ROWS) {
            net.0.buy_gear(g.slot.clone(), g.class_key.clone());
            city.notice = format!("bought {}", g.name);
        }
    }
    pick.clear();
}

