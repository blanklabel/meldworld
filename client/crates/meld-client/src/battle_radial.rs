//! **THE ORDER IS ASKED WHERE THE HERO IS STANDING.**
//!
//! A martial hero's root menu is five verbs — Attack, Skill, Defend, Item, Flee — and they
//! belong on the body they are about to be done with. A panel floating in the middle of the
//! arena answers "what can I do" a long way from the hero whose turn it is: the body glows at
//! one end of the screen and the buttons commanding it sit at the other, so every order costs
//! a look away and a look back, and the panel covers the fight while you take it.
//!
//! They are a WHEEL of wedges lying on the ground at the acting hero's feet, with the body
//! standing in the middle of it — the same object its health ring is, so the question and the
//! fighter being asked are one shape.
//!
//! ⚠️ **WEDGES, NOT CHIPS.** Five pills floating near a hero are five separate things that
//! happen to be nearby; a divided ring is ONE thing that belongs to it, and it is what makes
//! the hero read as standing inside its own menu. A `Node` is an alpha-blended rounded
//! rectangle and cannot be a wedge, so the ring is a ground material (`command_wheel.wgsl`)
//! and each label is UI text projected onto its own wedge — the same split the health ring's
//! curved digits use.
//!
//! ⚠️ **THE LABELS ARE UPRIGHT, THE WHEEL IS IN PERSPECTIVE.** Text laid flat on the ground at
//! this camera pitch is unreadable; text that ignores the ground is not on the wheel. Each
//! label sits at its wedge's own projected centre and stays upright, which is what the
//! reference art does too.
//!
//! ⚠️ **AND THE ARROWS ARE NOT A CROSS.** A d-pad maps `ArrowDown` to FLEE and `ArrowRight`
//! to DEFEND, which is the hazard `swallow_the_key_you_walked_in_on` exists to catch: walk
//! into a creature holding south and the fight ends before you have looked at it. An arc has
//! ONE axis, so ←/→ run the cursor along it and Enter picks — and Flee is off the arrows
//! altogether, which retires the worst reading of that hazard rather than guarding it twice.
//! Every chip carries its own letter key, so a direct press is never hidden.

use bevy::asset::Asset;
use bevy::reflect::TypePath;
use bevy::render::render_resource::AsBindGroup;
use bevy::shader::ShaderRef;

use meld_client::glass;

use super::*;

/// The root node of the radial: an empty anchor at the acting hero's feet that every chip is
/// positioned around. One parent, so [`follow_radial_menu`] moves the whole cluster by writing
/// a single `Node` instead of re-laying-out six.
#[derive(Component)]
pub(crate) struct RadialMenu;


/// One verb's label, and which wedge of the wheel it stands on.
#[derive(Component)]
pub(crate) struct WheelLabel {
    pub(crate) wedge: usize,
}

/// The block naming who is being commanded, which rides above the wheel.
#[derive(Component)]
pub(crate) struct WheelCaption;

/// The ground disc the wedges are cut from. One for the whole battle, moved onto whichever
/// body is being asked for an order.
#[derive(Component)]
pub(crate) struct CommandWheelDisc {
    /// How far through the push this wheel is, in seconds of [`WHEEL_OPEN_SECS`].
    pub(crate) open: f32,
}

/// One chip on the arc. Its own marker because the arc styles itself: the list panel's rows go
/// transparent between states and these have to stay legible over grass, a sprite and a health
/// ring all at once.
#[derive(Component)]
pub(crate) struct RadialChip;

/// The fixed chip holding the auto-battle toggle. Its own corner, because auto-battle is a
/// decision about the whole FIGHT rather than about this hero's turn, and a menu attached to a
/// hero is gone between turns — which is exactly when a player reaches for it.
#[derive(Component)]
pub(crate) struct AutoChipRoot;

/// The band the wedges occupy, as a fraction of the wheel mesh's radius — mirrored from
/// `command_wheel.wgsl`, which cuts them, and held against it by test. The Rust side needs
/// them because every label is placed along the middle of that band.
pub(crate) const W_IN: f32 = 0.52;
pub(crate) const W_OUT: f32 = 0.97;
/// `WorldAssets::shadow_mesh` is a `Circle::new(0.7)`; the wheel borrows it exactly as the
/// health ring does rather than carrying a third disc.
const MESH_RADIUS: f32 = 0.7;
/// How big the wheel is in world units.
///
/// ⚠️ **IT STANDS OUTSIDE THE HEALTH RING, WHICH THE REFERENCE DOES NOT.** In the art the
/// health arcs are the wheel's own outer rim. Nesting it that way is not merely tight here, it
/// is impossible: the health band's inner wall is 0.567 world units out, so a wheel inside it
/// caps at about a third of this scale — roughly 73px of screen radius for five words. The two
/// rings are concentric instead, and neither is a caption on the other.
const WHEEL_SCALE: f32 = 2.05;
/// Where the wheel starts before it opens: its outer wall exactly on the health ring's, so the
/// tiles are seen to PUSH the circle out rather than to appear beside it.
///
/// ⚠️ **THE GROWTH IS THE TELL.** `0.97 × 0.7 × 1.336 = 0.907`, which is the health band's own
/// outer radius — pick this number by arithmetic from that one, not by eye, or the wheel opens
/// out of thin air a few pixels off the bar and the whole gesture reads as two rings blinking
/// rather than as one becoming the other.
const WHEEL_SHUT_SCALE: f32 = 1.336;
/// How long the push takes. Long enough to be a movement the eye catches on its own, short
/// enough that it is never between you and an order you already knew you wanted.
const WHEEL_OPEN_SECS: f32 = 0.22;
/// World-space radius of the middle of the band at a given scale: the line the tiles sit on.
pub(crate) fn label_radius(scale: f32) -> f32 {
    MESH_RADIUS * scale * (W_IN + W_OUT) * 0.5
}

/// How far open the wheel is, 0..1, eased so it arrives rather than stops.
pub(crate) fn open_ease(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    1.0 - (1.0 - t) * (1.0 - t) * (1.0 - t)
}

/// The wheel's scale at a given openness — from sitting on the health ring to fully pushed out.
pub(crate) fn open_scale(open: f32) -> f32 {
    WHEEL_SHUT_SCALE + (WHEEL_SCALE - WHEEL_SHUT_SCALE) * open_ease(open)
}
/// How far the wheel is pushed AWAY from the camera, as a fraction of its own label radius.
///
/// ⚠️ **THE PARTY STANDS AT THE BOTTOM EDGE OF THE FRAME, AND A GROUND WHEEL HAS A FRONT.**
/// Centred on the feet, the near wedge lands ~115px BELOW them and runs off the screen — the
/// same constraint that put the old arc in the upper half-circle, met again by a shape that
/// has no upper half to move to. Sitting the wheel behind the body spends the room that IS
/// there (up-screen, over open ground) to buy back the room that is not — and it has to be
/// set back by HALF its radius, not a token amount: the near wedge otherwise lands on the
/// health ring's own HP digits, which want the same front-bottom arc of the same body.
const WHEEL_BACK: f32 = 0.52;

/// How far above the ground the wheel is drawn — under the health ring's own lift, so where
/// the two overlap the bar wins. The bar is the one that has to be read at a glance.
const WHEEL_LIFT: f32 = 0.022;
/// A tile IS its wedge: it is measured from the wedge's own projected corners every frame, so
/// it sits INSIDE the ring the way the reference art's do rather than floating over it. These
/// are the floor and ceiling that keeps a far wedge from collapsing to nothing and a near one
/// from swallowing the arena.
///
/// ⚠️ **THE TILE IS THE CLICK TARGET, WHICH IS WHY IT WANTS THE WHOLE WEDGE.** A small plate
/// on a big sector leaves most of the control dead to the pointer, and an order you have to
/// aim at costs a beat of the fight. [`TILE_INSET`] is what stops it overhanging: a tile that
/// spills past its wedge steals the press next door, and the wrong order still resolves.
const TILE_MIN_W: f32 = 84.0;
const TILE_MAX_W: f32 = 210.0;
const TILE_MIN_H: f32 = 46.0;
const TILE_MAX_H: f32 = 132.0;
/// How much of its own wedge a tile takes, leaving the wheel's gaps and walls showing round it.
const TILE_INSET: f32 = 0.88;
/// How far up-screen a label sits from its wedge's own centre. The near wedge would otherwise
/// touch the health ring's HP digits, which sit on the band directly below it — and lifting
/// every label uniformly keeps the wheel reading as one control, where lifting the one that
/// collides would be a special case nothing else on the wheel obeys.
const LABEL_RISE: f32 = 14.0;
/// The caption block (who is being commanded, and what the cursor's wedge does) sits above the
/// wheel — the one direction with room at every hero's position, since the party stands at the
/// bottom edge of the frame.
const CAPTION_W: f32 = 300.0;
const CAPTION_LIFT: f32 = 312.0;

/// One wedge: the `menu_entries` Root index it fires, its label, and the key that presses it
/// directly. Its POSITION is its place in this array — wedge `n` of `SLOTS.len()`, measured
/// clockwise from the arc nearest the camera — so the shader and the labels cannot disagree
/// about which sector a word belongs to.
///
/// ⚠️ **THE INDEX IS THE CONTRACT.** These are positions in `menu_entries`' Root list
/// (0 Attack · 1 Defend · 2 Item · 3 Skill · 4 Flee) and `menu_click` fires whatever index the
/// wedge carries — so a wedge pointing at the wrong one silently gives a different order.
pub(crate) struct Slot {
    pub(crate) index: usize,
    /// The mdi glyph, set ABOVE the word — the reference art stacks them, and a wedge is
    /// taller than it is wide at this radius, so the shape wants the icon on its own line.
    pub(crate) glyph: &'static str,
    /// Upper case, as the art has it: these are five short commands rather than prose, and
    /// caps is what makes them read as the labels ON a control rather than as text near one.
    pub(crate) word: &'static str,
    pub(crate) key: &'static str,
}

/// The five verbs around the wheel, ATTACK on the arc nearest the camera: it is the
/// most-pressed row in the game and the cursor's home, so it takes the one wedge that is never
/// behind the body and never has to be looked for.
pub(crate) const SLOTS: [Slot; 5] = [
    // mdi glyphs (see UiFont): sword=Attack, shield=Defend, auto-fix=Skill, flask=Item,
    // run-fast=Flee.
    Slot { index: 0, glyph: "\u{f04e5}", word: "ATTACK", key: "A" },
    Slot { index: 1, glyph: "\u{f132}", word: "DEFEND", key: "D" },
    Slot { index: 3, glyph: "\u{f0068}", word: "SKILL", key: "S" },
    Slot { index: 2, glyph: "\u{f0093}", word: "ITEM", key: "I" },
    Slot { index: 4, glyph: "\u{f070e}", word: "FLEE", key: "F" },
];

/// Which way round the wheel a rising wedge number goes, in world space.
///
/// ⚠️ **THE SIGN IS SETTLED BY RENDERING IT, NOT BY READING THE TRANSFORM.** The disc is laid
/// flat by a -90° turn about X, and which way that sends the mesh's own +x is exactly the kind
/// of thing this file has already been wrong about once (the health pool drained across the
/// front for a whole build). Attack sits on the near arc and Defend must land to its RIGHT on
/// screen; flip this if it does not.
const SPIN: f32 = 1.0;

/// Where wedge `n`'s centre sits, in turns clockwise from the arc nearest the camera.
pub(crate) fn slot_turns(n: usize) -> f32 {
    n as f32 / SLOTS.len() as f32
}

/// The bearing handed to the shader so wedge 0 is CENTRED on the near arc rather than starting
/// there — a sector whose edge is the thing the eye lands on reads as a seam.
pub(crate) fn wheel_bearing() -> f32 {
    -0.5 / SLOTS.len() as f32
}

/// Where wedge `n`'s label stands in the world, given the body's feet and the direction from
/// the body toward the camera (flattened onto the ground the wheel lies on).
pub(crate) fn slot_world(feet: Vec3, front: Vec3, n: usize, scale: f32) -> Vec3 {
    wheel_point(feet, front, slot_turns(n), label_radius(scale), scale)
}

/// Any point on the wheel: `turns` clockwise from the near arc, `radius` out from its hub.
///
/// ⚠️ **THE TILE IS MEASURED WITH THIS, NOT GUESSED.** A wedge is a sector of a circle drawn
/// in perspective, so how wide and how deep it is ON SCREEN differs at every bearing and with
/// every camera. Projecting its own corners is the only thing that puts a tile INSIDE its
/// wedge rather than over it.
pub(crate) fn wheel_point(feet: Vec3, front: Vec3, turns: f32, radius: f32, scale: f32) -> Vec3 {
    let a = SPIN * turns * std::f32::consts::TAU;
    wheel_centre(feet, front, scale) + Quat::from_rotation_y(a) * front * radius
}

/// The wheel's inner and outer wall in world units — where the band the tiles sit in begins
/// and ends.
pub(crate) fn band_radii(scale: f32) -> (f32, f32) {
    let r = MESH_RADIUS * scale;
    (W_IN * r, W_OUT * r)
}

/// Half a wedge, in turns.
pub(crate) fn half_wedge() -> f32 {
    0.5 / SLOTS.len() as f32
}

/// Where the wheel's own middle sits: behind the body it belongs to, by [`WHEEL_BACK`].
pub(crate) fn wheel_centre(feet: Vec3, front: Vec3, scale: f32) -> Vec3 {
    feet - front * (label_radius(scale) * WHEEL_BACK)
}

/// What a chip is drawn on. Solid enough to read over grass and a sprite at once, which the
/// panel's own glass is not — that sits over a scrim and this sits over the fight.
pub(crate) const CHIP_BASE: Color = Color::srgba(0.04, 0.05, 0.09, 0.84);
/// The highlight under the wedge the cursor is on, and under the one the pointer is over.
///
/// ⚠️ **DEEP, NOT BRIGHT.** A pale gold plate under gold text is gold on gold: the selected
/// wedge — the one the player is about to press — was the least readable label on the wheel.
/// The lift belongs on the BORDER, which is already lit, and the fill stays dark enough for
/// the word to sit on it.
pub(crate) const CHIP_ON: Color = Color::srgba(0.46, 0.31, 0.04, 0.92);
pub(crate) const CHIP_HOVER: Color = Color::srgba(0.30, 0.40, 0.70, 0.85);

/// Light the chip the cursor is on, and whichever one the pointer is over. Its own system
/// rather than `style_command_menu`'s: a row in a list may fall back to transparent, and a chip
/// standing on the battlefield may never.
pub(crate) fn style_radial(
    menu: Res<BattleMenu>,
    mut chips: Query<(&MenuRow, &Interaction, &mut BackgroundColor), With<RadialChip>>,
) {
    for (row, interaction, mut bg) in &mut chips {
        let want = if *interaction == Interaction::Pressed || row.index == menu.cursor {
            CHIP_ON
        } else if *interaction == Interaction::Hovered {
            CHIP_HOVER
        } else {
            CHIP_BASE
        };
        if bg.0 != want {
            *bg = BackgroundColor(want);
        }
    }
}


/// Move the cursor one wedge around the wheel. `dir` is +1 clockwise, -1 anticlockwise, and it
/// WRAPS — a wheel has no ends, and a cursor that stopped at one would be inventing a seam the
/// player cannot see.
pub(crate) fn step_cursor(cursor: usize, dir: i32) -> usize {
    let n = SLOTS.len() as i32;
    let at = SLOTS.iter().position(|s| s.index == cursor).unwrap_or(0) as i32;
    SLOTS[(at + dir).rem_euclid(n) as usize].index
}

/// Is the radial what is being shown right now? The panel and the radial answer the SAME page
/// for the same hero, so exactly one of them may draw it — `rebuild_command_menu` asks this
/// and stands down.
///
/// A Psyker's root is deliberately not radial: it is `Focus / Revoke / Hold / Flee`, which
/// OPENS pages rather than naming five things to do, and its held manifestations need prose
/// the arc has nowhere to put.
pub(crate) fn radial_root(battle: &BattleData, menu: &BattleMenu) -> bool {
    menu.level == MenuLevel::Root && battle.active_class() != "psyker"
}

/// Build the arc around whichever hero is being asked for an order.
///
/// Rebuilt on its SIGNATURE: the chips' contents depend on which hero and which page, not on
/// where that hero is standing, so following a moving body is [`follow_radial_menu`]'s job and
/// costs one `Node` write instead of six spawns.
#[allow(clippy::too_many_arguments)]
pub(crate) fn rebuild_radial_menu(
    mut commands: Commands,
    battle: Res<BattleData>,
    menu: Res<BattleMenu>,
    report: Res<LootReport>,
    levelup: Res<LevelUpQueue>,
    roster: Res<PartyRoster>,
    backpack: Res<RunBackpack>,
    tutorial_run: Res<TutorialRun>,
    existing: Query<Entity, With<RadialMenu>>,
    mut last_sig: Local<String>,
) {
    // Same gate as the panel: a results card is up means the fight is over, and Attack must
    // not be clickable on top of the summary.
    let show = battle.active.is_some()
        && !crate::battle::results_showing(&report, &levelup)
        && radial_root(&battle, &menu);
    let active_id = battle.active.clone().unwrap_or_default();
    // The cursor rides the signature so the caption's prose follows the highlighted chip.
    let sig = format!("{show}|{active_id}|{}|{:?}", menu.cursor, tutorial_run.battle_intro);
    if sig == *last_sig && (existing.is_empty() == !show) {
        return;
    }
    *last_sig = sig;
    for e in &existing {
        commands.entity(e).despawn();
    }
    if !show {
        return;
    }

    let commanding = battle.hero_label(&active_id);
    let can_switch = crate::battle::next_commandable(&battle).is_some();
    // The cursor row's own prose. Defend in particular is a play rather than a last resort —
    // it braces the gauge as well as the blow — and nothing on a four-word chip can say so.
    let hero_level = battle.active_level();
    let spent = crate::battle::spent_tokens(&battle, &active_id);
    let foci = crate::battle::held_foci(&battle, &active_id);
    let adrenaline =
        battle.view(&active_id).map(|v| status_num(&v.statuses, "adrenaline:")).unwrap_or(0);
    let tooltip = menu_entries(
        MenuLevel::Root,
        &battle.active_class(),
        hero_level,
        &crate::battle::held_potions(&backpack, battle.active_slot()),
        &spent,
        &foci,
        &roster,
        adrenaline,
    )
    .get(menu.cursor)
    .map(|e| e.tooltip.clone())
    .unwrap_or_default();

    let gold = Color::srgb(1.0, 0.85, 0.45);
    let red = Color::srgb(1.0, 0.55, 0.5);
    let neutral = Color::srgb(0.92, 0.94, 1.0);
    // The guided [T]-dive's paced explainer brightens whichever chip it is currently
    // describing, in place — the same idiom every other step of it uses.
    let intro = |step: BattleIntroStep, base: Color| {
        if tutorial_run.battle_intro == Some(step) { glass::ACTIVE_EDGE } else { base }
    };

    commands
        .spawn((
            RadialMenu,
            // A full-screen, non-interactive layer: every label inside it is positioned in
            // SCREEN space from its own wedge, so the root is a canvas rather than an anchor.
            Node {
                position_type: PositionType::Absolute,
                left: Val::Px(0.0),
                top: Val::Px(0.0),
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                ..default()
            },
        ))
        .with_children(|root| {
            for (n, slot) in SLOTS.iter().enumerate() {
                let (edge, text, step) = match slot.index {
                    0 => (gold, gold, Some(BattleIntroStep::Attack)),
                    1 => (glass::EDGE_SOFT, neutral, Some(BattleIntroStep::Defend)),
                    3 => (glass::EDGE_SOFT, neutral, Some(BattleIntroStep::Skill)),
                    4 => (red, red, Some(BattleIntroStep::Flee)),
                    _ => (glass::EDGE_SOFT, neutral, None),
                };
                let edge = step.map(|s| intro(s, edge)).unwrap_or(edge);
                root.spawn((
                    Button,
                    RadialChip,
                    WheelLabel { wedge: n },
                    MenuRow { index: slot.index },
                    Node {
                        border_radius: BorderRadius::all(Val::Px(7.0)),
                        position_type: PositionType::Absolute,
                        // Parked; `follow_radial_menu` puts it on its own wedge once the body
                        // has been projected.
                        left: Val::Px(-4000.0),
                        top: Val::Px(-4000.0),
                        width: Val::Px(TILE_MIN_W),
                        height: Val::Px(TILE_MIN_H),
                        flex_direction: FlexDirection::Column,
                        align_items: AlignItems::Center,
                        justify_content: JustifyContent::Center,
                        border: UiRect::all(Val::Px(1.0)),
                        ..default()
                    },
                    BorderColor::all(edge),
                    BackgroundColor(CHIP_BASE),
                ))
                .with_children(|chip| {
                    chip.spawn((
                        Text::new(slot.glyph),
                        TextFont { font_size: FontSize::Px(16.0), ..default() },
                        TextColor(text),
                    ));
                    chip.spawn((
                        Text::new(slot.word),
                        TextFont { font_size: FontSize::Px(13.0), ..default() },
                        TextColor(text),
                    ));
                    // The key on the wedge, not in a legend. A wheel has no reading order, so
                    // "press F to flee" has nowhere else to be said.
                    chip.spawn((
                        Text::new(slot.key),
                        TextFont { font_size: FontSize::Px(9.0), ..default() },
                        TextColor(glass::DIM),
                    ));
                });
            }

            // The caption: who is being commanded, whether another hero is also waiting, and
            // what the highlighted chip actually does. Above the apex, which is the one
            // direction with room whichever row the hero stands in.
            root.spawn((
                WheelCaption,
                Node {
                    border_radius: BorderRadius::all(Val::Px(8.0)),
                    position_type: PositionType::Absolute,
                    left: Val::Px(-CAPTION_W * 0.5),
                    top: Val::Px(-CAPTION_LIFT),
                    width: Val::Px(CAPTION_W),
                    flex_direction: FlexDirection::Column,
                    align_items: AlignItems::Center,
                    row_gap: Val::Px(2.0),
                    padding: UiRect::axes(Val::Px(10.0), Val::Px(5.0)),
                    ..default()
                },
                // ON ITS OWN PLATE. The caption hangs over whatever the arena put behind the
                // hero — grass, a sprite, another body's ring — and bare text on that is not
                // text, it is texture.
                BackgroundColor(CHIP_BASE),
            ))
            .with_children(|cap| {
                cap.spawn((
                    // Nerd Font (mdi) sword glyph, not a bare unicode symbol — the default
                    // face would tofu it (see UiFont).
                    Text::new(format!("\u{f04e5} {commanding}")),
                    TextFont { font_size: FontSize::Px(15.0), ..default() },
                    TextColor(gold),
                ));
                if can_switch {
                    cap.spawn((
                        Text::new("[Tab] switch".to_string()),
                        TextFont { font_size: FontSize::Px(12.0), ..default() },
                        TextColor(Color::srgb(0.6, 0.66, 0.8)),
                    ));
                }
                if !tooltip.is_empty() {
                    cap.spawn((
                        Text::new(tooltip),
                        TextFont { font_size: FontSize::Px(12.0), ..default() },
                        TextColor(glass::DIM),
                        Node { max_width: Val::Px(CAPTION_W), ..default() },
                    ));
                }
            });
        });
}

/// The wheel's own material.
#[derive(Asset, AsBindGroup, TypePath, Debug, Clone)]
pub(crate) struct CommandWheel {
    /// `(wedge count, cursor wedge, seconds, bearing)` — see `command_wheel.wgsl`.
    #[uniform(100)]
    pub(crate) params: Vec4,
    /// The acting side's colour, with the wheel's overall opacity in `a`.
    #[uniform(100)]
    pub(crate) tint: Vec4,
}

impl Material for CommandWheel {
    fn fragment_shader() -> ShaderRef {
        "shaders/command_wheel.wgsl".into()
    }
    /// Paint on the ground, like the health ring it sits inside.
    fn alpha_mode(&self) -> AlphaMode {
        AlphaMode::Blend
    }
}

/// Put the wheel under whichever hero is being asked, and light the wedge the cursor is on.
///
/// ⚠️ **ONE DISC FOR THE WHOLE FIGHT.** Spawning it per hero would rebuild a mesh binding every
/// time the turn moved, and the wheel is only ever under one body at a time by definition.
#[allow(clippy::too_many_arguments)]
pub(crate) fn drive_command_wheel(
    mut commands: Commands,
    time: Res<Time>,
    battle: Res<BattleData>,
    menu: Res<BattleMenu>,
    report: Res<LootReport>,
    levelup: Res<LevelUpQueue>,
    assets: Option<Res<crate::world_render::WorldAssets>>,
    mut mats: ResMut<Assets<CommandWheel>>,
    rings: Query<(&crate::battle_rings::CombatantRing, &GlobalTransform)>,
    cam: Query<&GlobalTransform, With<Camera3d>>,
    mut disc: Query<(
        &mut Transform,
        &mut Visibility,
        &MeshMaterial3d<CommandWheel>,
        &mut CommandWheelDisc,
    )>,
) {
    let show = battle.active.is_some()
        && !crate::battle::results_showing(&report, &levelup)
        && radial_root(&battle, &menu);
    let Ok((mut tf, mut vis, mat, mut state)) = disc.single_mut() else {
        // Not built yet. It costs one mesh and one material for the whole battle, so it is
        // raised the first time a hero is actually asked for something.
        if show {
            if let Some(wa) = assets {
                let mat = mats.add(CommandWheel {
                    params: Vec4::new(SLOTS.len() as f32, -1.0, 0.0, wheel_bearing()),
                    tint: Vec4::new(0.40, 0.82, 1.0, 1.0),
                });
                commands.spawn((
                    CommandWheelDisc { open: 0.0 },
                    Mesh3d(wa.shadow_mesh.clone()),
                    MeshMaterial3d(mat),
                    Visibility::Hidden,
                    Transform::from_xyz(0.0, WHEEL_LIFT, 0.0)
                        .with_rotation(Quat::from_rotation_x(-std::f32::consts::FRAC_PI_2))
                        .with_scale(Vec3::splat(WHEEL_SCALE)),
                ));
            }
        }
        return;
    };
    let at = battle
        .active
        .as_deref()
        .and_then(|id| rings.iter().find(|(r, _)| r.id == id).map(|(_, t)| t.translation()));
    let want = if show && at.is_some() { Visibility::Inherited } else { Visibility::Hidden };
    if *vis != want {
        *vis = want;
    }
    let Some(at) = at.filter(|_| show) else {
        // Shut again the instant the turn is spent: the push is the TELL, and a wheel caught
        // half-open on a hero who is no longer being asked says the opposite of what it means.
        state.open = 0.0;
        return;
    };
    state.open = (state.open + time.delta_secs() / WHEEL_OPEN_SECS).min(1.0);
    let scale = open_scale(state.open);
    if tf.scale.x != scale {
        tf.scale = Vec3::splat(scale);
    }
    // The wheel rides the RING's position, so it follows the body's lunge for the same reason
    // and by the same route the bar does — set back from it by the same amount the labels are,
    // or the words stand off the wedges they name.
    let front = cam
        .iter()
        .next()
        .and_then(|c| (c.translation() - at).with_y(0.0).try_normalize())
        .unwrap_or(Vec3::Z);
    let c = wheel_centre(at, front, scale);
    if tf.translation.x != c.x || tf.translation.z != c.z {
        tf.translation.x = c.x;
        tf.translation.z = c.z;
        tf.translation.y = c.y + WHEEL_LIFT;
    }
    let cursor = SLOTS.iter().position(|s| s.index == menu.cursor).map(|n| n as f32);
    if let Some(mut m) = mats.get_mut(&mat.0) {
        m.params = Vec4::new(
            SLOTS.len() as f32,
            cursor.unwrap_or(-1.0),
            time.elapsed_secs(),
            wheel_bearing(),
        );
    }
}

/// Stand every label on its own wedge, and the caption above the wheel.
///
/// ⚠️ **PROJECTED EVERY FRAME, NOT LAID OUT ONCE.** The wheel lies on the ground under a body
/// that lunges, and the camera moves; a label placed at spawn is a label that is on the right
/// wedge only until something happens. It is a `Node` write per label and only when the point
/// actually moved.
pub(crate) fn follow_radial_menu(
    battle: Res<BattleData>,
    window: Query<&Window>,
    cam_q: Query<(&Camera, &GlobalTransform), With<Camera3d>>,
    rings: Query<(&crate::battle_rings::CombatantRing, &GlobalTransform)>,
    // The wheel's own openness, so a tile rides the push out instead of snapping to where the
    // wheel will end up — the growth IS the tell, and half of it is the labels moving with it.
    wheel: Query<&CommandWheelDisc>,
    mut labels: Query<(&WheelLabel, &mut Node, &mut Visibility), Without<WheelCaption>>,
    mut caption: Query<&mut Node, With<WheelCaption>>,
) {
    let Some(active) = battle.active.as_deref() else { return };
    let Some((cam, cam_tf)) = cam_q.iter().next() else { return };
    let Some(feet) = rings.iter().find(|(r, _)| r.id == active).map(|(_, t)| t.translation())
    else {
        for (_, _, mut vis) in &mut labels {
            if *vis != Visibility::Hidden {
                *vis = Visibility::Hidden;
            }
        }
        return;
    };
    let Some(front) = (cam_tf.translation() - feet).with_y(0.0).try_normalize() else { return };
    let (sw, sh) = window.single().map(|w| (w.width(), w.height())).unwrap_or((1920.0, 1080.0));

    let project = |p: Vec3| cam.world_to_viewport(cam_tf, p).ok().map(|v| Vec2::new(v.x, v.y));
    let scale = wheel.single().map(|w| open_scale(w.open)).unwrap_or(WHEEL_SCALE);
    let (r_in, r_out) = band_radii(scale);
    let lr = label_radius(scale);

    for (label, mut node, mut vis) in &mut labels {
        let turns = slot_turns(label.wedge);
        // The wedge's OWN corners: its two arc ends at the label radius, and its inner and
        // outer wall at its centre bearing. A tile cut to those sits in the ring rather than
        // on it — which is the whole difference between the reference art's wheel and five
        // plates parked near a hero.
        let edge = half_wedge() * TILE_INSET;
        let (Some(p), Some(l), Some(r), Some(i), Some(o)) = (
            project(slot_world(feet, front, label.wedge, scale)),
            project(wheel_point(feet, front, turns - edge, lr, scale)),
            project(wheel_point(feet, front, turns + edge, lr, scale)),
            project(wheel_point(feet, front, turns, r_in, scale)),
            project(wheel_point(feet, front, turns, r_out, scale)),
        ) else {
            if *vis != Visibility::Hidden {
                *vis = Visibility::Hidden;
            }
            continue;
        };
        if *vis != Visibility::Inherited {
            *vis = Visibility::Inherited;
        }
        let w = (l.distance(r) * TILE_INSET).clamp(TILE_MIN_W, TILE_MAX_W);
        let h = (i.distance(o) * TILE_INSET).clamp(TILE_MIN_H, TILE_MAX_H);
        // Kept inside the window: a wedge at the edge of a wide formation would otherwise hang
        // its label off the side, and an order you cannot click is the one failure this menu
        // is not allowed to have.
        let x = (p.x - w * 0.5).clamp(2.0, (sw - w - 2.0).max(2.0));
        let y = (p.y - h * 0.5 - LABEL_RISE).clamp(2.0, (sh - h - 2.0).max(2.0));
        if node.left != Val::Px(x) || node.top != Val::Px(y) {
            node.left = Val::Px(x);
            node.top = Val::Px(y);
        }
        if node.width != Val::Px(w) || node.height != Val::Px(h) {
            node.width = Val::Px(w);
            node.height = Val::Px(h);
        }
    }

    if let Ok(mut node) = caption.single_mut() {
        if let Ok(p) = cam.world_to_viewport(cam_tf, feet) {
            let x = (p.x - CAPTION_W * 0.5).clamp(2.0, (sw - CAPTION_W - 2.0).max(2.0));
            let y = (p.y - CAPTION_LIFT).max(2.0);
            if node.left != Val::Px(x) || node.top != Val::Px(y) {
                node.left = Val::Px(x);
                node.top = Val::Px(y);
            }
        }
    }
}

/// The auto-battle toggle, in its own corner for the whole fight — a decision about the FIGHT
/// does not belong to one hero's turn, and it has to stay reachable in the beats where every
/// hero has already committed and no menu is up at all.
pub(crate) fn rebuild_auto_chip(
    mut commands: Commands,
    auto: Res<AutoBattle>,
    report: Res<LootReport>,
    levelup: Res<LevelUpQueue>,
    existing: Query<Entity, With<AutoChipRoot>>,
    mut last: Local<Option<bool>>,
) {
    // Down while a results card is up, with the rest of the fight's controls.
    let want = (!crate::battle::results_showing(&report, &levelup)).then_some(auto.0);
    if want == *last && want.is_some() == !existing.is_empty() {
        return;
    }
    *last = want;
    for e in &existing {
        commands.entity(e).despawn();
    }
    let Some(on) = want else { return };
    let (label, col) = if on {
        ("\u{f132} AUTO-BATTLE: ON  [T]", Color::srgb(0.55, 0.95, 0.65))
    } else {
        ("\u{f132} AUTO-BATTLE: OFF  [T]", Color::srgb(0.75, 0.8, 0.95))
    };
    commands
        .spawn((
            AutoChipRoot,
            Button,
            AutoBattleButton,
            Node {
                border_radius: BorderRadius::all(Val::Px(6.0)),
                position_type: PositionType::Absolute,
                right: Val::Px(14.0),
                bottom: Val::Px(14.0),
                padding: UiRect::axes(Val::Px(10.0), Val::Px(4.0)),
                border: UiRect::all(Val::Px(1.0)),
                ..default()
            },
            BorderColor::all(glass::EDGE_SOFT),
            BackgroundColor(glass::GLASS_THIN),
        ))
        .with_children(|b| {
            b.spawn((
                Text::new(label),
                TextFont { font_size: FontSize::Px(13.0), ..default() },
                TextColor(col),
            ));
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **A CHIP FIRES THE INDEX IT CARRIES.** `menu_click` looks up `menu_entries`' Root list
    /// by index, so a slot pointing at the wrong one hands the server a different order than
    /// the word the player read. All five are present, exactly once.
    #[test]
    fn every_root_verb_has_exactly_one_chip() {
        let mut seen: Vec<usize> = SLOTS.iter().map(|s| s.index).collect();
        seen.sort_unstable();
        assert_eq!(seen, vec![0, 1, 2, 3, 4], "the chips do not cover the root menu exactly");
        assert!(SLOTS.iter().any(|s| s.index == 0 && s.word == "ATTACK"));
        assert!(SLOTS.iter().any(|s| s.index == 4 && s.word == "FLEE"));
        // Upper case is the art's own convention and the thing that makes five short commands
        // read as labels ON a control rather than as words near one.
        for s in &SLOTS {
            assert_eq!(s.word, s.word.to_uppercase(), "{} is not a label", s.word);
            assert!(!s.glyph.is_empty(), "{} lost its icon", s.word);
        }
    }

    /// **THE WEDGES AND THE SHADER AGREE ABOUT WHERE THE BAND IS.** The ring is cut in WGSL
    /// and every label is placed along the middle of it from Rust, so the two copies of where
    /// that band sits have to match — and `make check` never builds a pipeline, so a drift
    /// here ships green and stands the words off their own wedges.
    #[test]
    fn the_band_is_where_the_shader_cuts_it() {
        let src = include_str!("../assets/shaders/command_wheel.wgsl");
        let read = |name: &str| -> f32 {
            let line = src
                .lines()
                .find(|l| l.trim_start().starts_with(&format!("const {name}: f32")))
                .unwrap_or_else(|| panic!("{name} is gone from the shader"));
            line.split('=').nth(1).unwrap().trim().trim_end_matches(';').parse().unwrap()
        };
        assert_eq!(read("W_IN"), W_IN, "the inner wall moved in the shader only");
        assert_eq!(read("W_OUT"), W_OUT, "the outer wall moved in the shader only");
        let mid = label_radius(WHEEL_SCALE) / (MESH_RADIUS * WHEEL_SCALE);
        assert!(mid > W_IN && mid < W_OUT, "the labels stand off the band: {mid}");
    }

    /// **A WEDGE IS CENTRED ON THE NEAR ARC, NOT SEAMED ACROSS IT.** The bearing handed to the
    /// shader has to put wedge 0's MIDDLE at the point the eye lands on; half a wedge out and
    /// the first thing the player sees is the gap between two choices.
    #[test]
    fn the_first_wedge_faces_the_camera() {
        let half = 0.5 / SLOTS.len() as f32;
        assert!((wheel_bearing() + half).abs() < 1e-6, "wedge 0 is not centred on the front");
        assert_eq!(slot_turns(0), 0.0, "wedge 0 is not on the near arc");
        assert_eq!(SLOTS[0].index, 0, "the near wedge is not Attack");
    }

    /// The labels stand on the wheel, evenly spaced, one per wedge — and Attack is the one
    /// nearest the camera, which is the whole reason it gets that wedge.
    #[test]
    fn every_wedge_gets_its_own_place_on_the_wheel() {
        let feet = Vec3::ZERO;
        // The camera is somewhere along +z, so "front" is +z.
        let front = Vec3::Z;
        let places: Vec<Vec3> =
            (0..SLOTS.len()).map(|n| slot_world(feet, front, n, WHEEL_SCALE)).collect();
        // Measured from the WHEEL's own middle, which sits behind the body (`WHEEL_BACK`) so
        // its near wedge stays on screen — the labels ride the band, not a circle round the
        // feet.
        let hub = wheel_centre(feet, front, WHEEL_SCALE);
        for (i, a) in places.iter().enumerate() {
            assert!(
                (a.distance(hub) - label_radius(WHEEL_SCALE)).abs() < 1e-3,
                "wedge {i} is off the band at {a:?}"
            );
            for (j, b) in places.iter().enumerate().skip(i + 1) {
                assert!(
                    a.distance(*b) > label_radius(WHEEL_SCALE) * 0.5,
                    "wedges {i} and {j} sit together"
                );
            }
        }
        // Attack is nearest the camera, and still in FRONT of the body despite the set-back —
        // a near wedge that ended up behind the hero is a wheel the hero is not standing in.
        assert!(places[0].z > places[1].z, "Attack is not on the near arc");
        assert!(places[0].z > feet.z, "the set-back pushed the near wedge behind the body");
        // …and Defend is to its right, which is the only thing that settles `SPIN`.
        assert!(places[1].x > 0.0, "Defend landed to the LEFT — flip SPIN");
    }

    /// **THE TILES PUSH THE CIRCLE OUT, AND THAT IS THE TELL.** The wheel starts with its
    /// outer wall exactly on the health ring's and grows from there, so the player sees one
    /// ring become another rather than a second ring blink into being beside the first.
    #[test]
    fn the_wheel_grows_out_of_the_health_ring() {
        // Shut, the wheel's outer wall is the health band's outer wall — the number this
        // constant is derived from rather than eyeballed against.
        let shut_outer = W_OUT * MESH_RADIUS * WHEEL_SHUT_SCALE;
        let health_outer = 0.7 * 1.35 * 0.96;
        assert!(
            (shut_outer - health_outer).abs() < 0.01,
            "the wheel does not start on the bar: {shut_outer} against {health_outer}"
        );
        // …and it only ever grows.
        assert_eq!(open_scale(0.0), WHEEL_SHUT_SCALE);
        assert!((open_scale(1.0) - WHEEL_SCALE).abs() < 1e-5);
        let mut last = open_scale(0.0);
        for i in 1..=20 {
            let s = open_scale(i as f32 / 20.0);
            assert!(s >= last, "the push went backwards at {i}");
            last = s;
        }
        // It eases OUT — most of the travel is spent early, so the movement is caught.
        assert!(open_ease(0.5) > 0.7, "the push is not front-loaded: {}", open_ease(0.5));
    }

    /// **THE CURSOR WRAPS.** A wheel has no ends, so a cursor that stopped at one would be
    /// inventing a seam the player cannot see — and it must reach every wedge either way
    /// round, or a verb is keyboard-unreachable.
    #[test]
    fn the_cursor_goes_round_the_wheel() {
        // Attack · Defend · Skill · Item · Flee, clockwise from the near arc.
        assert_eq!(step_cursor(0, 1), 1, "clockwise of Attack is Defend");
        assert_eq!(step_cursor(0, -1), 4, "anticlockwise of Attack is Flee");
        let mut seen = vec![0];
        let mut c = 0;
        for _ in 1..SLOTS.len() {
            c = step_cursor(c, 1);
            seen.push(c);
        }
        seen.sort_unstable();
        assert_eq!(seen, vec![0, 1, 2, 3, 4], "the cursor cannot reach every wedge");
        assert_eq!(step_cursor(c, 1), 0, "the wheel did not close");
    }
}
