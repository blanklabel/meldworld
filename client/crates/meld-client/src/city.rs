//! The Last City — the persistent hub city: walkable HD-2D plaza, districts, HUD.
//! Extracted from `main.rs` during the module reorg.


use bevy::prelude::*;
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
        action: CityAction::Notice("The Market Tiers are still being raised - player stalls open in M1."),
    },
    District {
        label: "The Forge & Alembic",
        x: -10.0,
        z: 9.0,
        radius: 5.0,
        action: CityAction::Notice("The Forge & Alembic are cold - crafting arrives in M2."),
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
        action: CityAction::Notice("The Drill Yard is closed - build templates arrive in M3."),
    },
    District {
        label: "The Vanguard Wall",
        x: -15.0,
        z: -14.0,
        radius: 5.0,
        action: CityAction::Notice("The Vanguard Wall is unlit - leaderboards arrive in M3."),
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
            p.spawn(Node {
                flex_direction: FlexDirection::Column,
                align_items: AlignItems::Center,
                row_gap: Val::Px(4.0),
                ..default()
            })
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
            p.spawn((
                CityStatusText,
                Text::new(""),
                TextFont { font_size: 18.0, ..default() },
                TextColor(Color::srgb(0.95, 0.88, 0.62)),
            ));
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
            BorderColor(Color::srgb(0.5, 0.42, 0.2)),
            BorderRadius::all(Val::Px(8.0)),
            BackgroundColor(Color::srgba(0.16, 0.12, 0.05, 0.9)),
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
    mut next: ResMut<NextState<Screen>>,
) {
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
    // E interacts with whichever other district the avatar is standing in.
    if keys.just_pressed(KeyCode::KeyE) {
        if let Some(i) = city.near {
            match CITY_DISTRICTS[i].action {
                CityAction::Dive | CityAction::Vault => {} // handled above
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
    mut q: Query<&mut Transform, With<CityPlayer>>,
) {
    let Ok(mut tf) = q.single_mut() else { return };
    if session.entered {
        return; // stepping through The Threshold — stop walking
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
    city.near = best.map(|(i, _)| i);
}

pub(crate) fn render_city(
    inv: Res<InventoryData>,
    session: Res<Session>,
    city: Res<CityUi>,
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
                CityAction::Notice(_) => format!("{}    [E] inspect", d.label),
            }
        } else {
            "WASD move    [E] enter a district    [ENTER] run    [T] tutorial    [C] co-op    [V] storage chest"
                .to_string()
        };
        **t = if city.notice.is_empty() {
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
