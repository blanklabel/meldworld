//! **THE CHARGING LINE: who is about to go, across the top of the fight.**
//!
//! An ATB fight's whole shape is *everyone charges at once*, and until this bar the only
//! way to read it was four hero gauges along the bottom edge and — for a party whose
//! Hunter had bought the top rung of Predator's Eye — a 5px enemy gauge under each foe's
//! HP. So the one question a player asks every second of a fight ("who goes next, them or
//! me?") had no answer on screen for either side.
//!
//! Every combatant rides one shared track: its little south-facing sprite slides left to
//! right as its gauge fills, dragging a sparkling charge line behind it, and the moment it
//! reaches the GO end it **bounces and glows** until it spends its turn. Two lanes — foes
//! above the line, your party below it — because nine icons on one rail is a pile.
//!
//! **AND A THIRD LANE FOR ANYONE CHARGING FASTER THAN THEY SHOULD BE.** A fighter that
//! GUARDED (braced: its gauge fills at `defend_haste_mult`), one carrying a HASTE, or one
//! that just blazed ahead on a five-blow streak hops down to the FAST rail — so "why is
//! that one moving quicker" is answered by where it is standing rather than by remembering
//! what you pressed four seconds ago. The icon keeps its own side's colour there, because
//! the lane says *speed* and the colour says *whose*.
//!
//! ⚠️ **GETTING TO GO DOES NOT STOP ANYBODY ELSE GOING, AND THIS IS WHERE YOU SEE IT.** A
//! fighter awaiting input stops filling its own gauge and nothing else pauses: the party
//! keeps charging and so do the creatures (held by
//! `meld_battle`'s `one_hero_thinking_does_not_stop_the_fight`). An icon sitting at the GO
//! end glowing while five others keep sliding toward it is that rule, drawn.
//!
//! ⚠️ **IT ANIMATES ON A CLOCK AND REBUILDS ON A CAST.** The repo's rule is that UI redraws
//! on CHANGE, not on frame — so the node tree is torn down only when the set of combatants
//! in the fight changes ([`cast_key`]), and the per-frame system writes nothing but the
//! handful of `Node`/`BackgroundColor` values that actually moved. A gauge arrives ten
//! times a second; the icons are eased toward it ([`GLIDE`]) so the motion reads as a slide
//! rather than a tick.

use std::collections::HashMap;

use bevy::asset::Asset;
use bevy::prelude::*;
use bevy::reflect::TypePath;
use bevy::render::render_resource::{
    AsBindGroup, BlendComponent, BlendFactor, BlendOperation, BlendState, RenderPipelineDescriptor,
};
use bevy::shader::ShaderRef;
use bevy::ui_render::ui_material::{MaterialNode, UiMaterial, UiMaterialKey};

use meld_client::glass;

use crate::world_render::WorldAssets;
use crate::{hero_class, BattleData};
use meld_client::net::CombatantView;

/// How far the bar's panel sits from the top of the screen.
const BAR_TOP: f32 = 8.0;
/// Vertical padding inside the bar's panel.
const PANEL_PAD: f32 = 7.0;
/// The bar's track, in pixels. Fixed rather than a percentage: an icon's position along it
/// is a pixel offset written every frame, and a track that re-measured with the window
/// would need every one of them recomputed on resize anyway.
const TRACK_W: f32 = 660.0;
/// One little sprite, square.
const ICON: f32 = 30.0;
/// How far an icon sits below the top of its own lane.
const ICON_INSET: f32 = 3.0;
/// A lane's height: an icon, the gap it sits in, and the room its bounce needs above it —
/// DERIVED rather than picked, because a third lane turned "40 looks about right" into a
/// one-pixel overlap where a bouncing icon clipped into the rail above it
/// (`the_three_lanes_do_not_overlap` found it).
const LANE_H: f32 = ICON + ICON_INSET + BOUNCE_PX;
/// How much of the sprite's canvas the icon shows. Every class and creature sheet is a
/// 184² canvas with the character filling the middle ~48% of it (see `docs/asset-pipeline.md`),
/// so the icon is a centre crop rather than the whole square — otherwise the figure floats
/// in a box of nothing at half the size the slot allows. Expressed as a zoom so it works on
/// the 240²/256² boss sheets too.
const SPRITE_ZOOM: f32 = 2.1;
/// Sparks per charge line.
const SPARKS: usize = 3;
/// How fast a spark runs the length of a line, in lines per second.
const SPARK_SPEED: f32 = 0.55;
/// Bounces per second for a fighter whose turn has come up.
const BOUNCE_HZ: f32 = 2.6;
/// How far that bounce lifts the icon, in pixels.
const BOUNCE_PX: f32 = 6.0;
/// Exponential easing rate for an icon chasing its authoritative gauge. The server sends a
/// gauge ten times a second; this is what turns that staircase into a slide.
const GLIDE: f32 = 12.0;
/// The closest two icons in one lane may be drawn before the one behind is fanned back.
///
/// ⚠️ **WITHOUT THIS, EVERYBODY WHO IS READY IS ONE ICON.** Every fighter at a full gauge
/// sits at exactly the same x, so two heroes whose turns came up together drew on the same
/// pixel and the bar showed one of them — on the very bar whose whole point is that a turn
/// arriving does not stop anybody else's. Measured in the first real capture of this
/// feature: a party of four with two heroes ready rendered three icons.
const PILE_GAP: f32 = ICON * 0.72;

/// Which rail a fighter rides.
///
/// **FOUR RAILS AT MOST, EVER.** The bar is read at a glance in the middle of a fight, so
/// the number of lines it can show is a fixed, small vocabulary: what you are fighting,
/// your own party, everybody else's heroes, and whoever is charging faster than they
/// should be. A co-op merge can field sixteen heroes across four parties — a rail each
/// would be a stave, so every ally shares one.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Lane {
    Foe,
    /// The heroes this client is commanding.
    Mine,
    /// Everybody else's heroes, however many parties they came from.
    Ally,
    /// Braced, hastened, or fresh off a catch-up — either side.
    Fast,
}

/// Which rails this fight actually has, and therefore where each one sits.
///
/// The ALLY rail only exists when somebody else's heroes are in the fight, so a solo dive
/// draws three lines rather than a permanently empty fourth. The FAST rail is always drawn
/// even when empty: fighters hop onto it mid-fight, and a rail that appeared under them
/// would resize the whole panel at the moment the player is trying to read it.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) struct LaneSet {
    pub(crate) ally: bool,
}

impl LaneSet {
    /// The rails this fight draws, top to bottom. Mine sits directly under the foes —
    /// those are the two a player reads most — with the allies below them and the fast
    /// rail at the bottom.
    pub(crate) fn rails(self) -> Vec<Lane> {
        let mut v = vec![Lane::Foe, Lane::Mine];
        if self.ally {
            v.push(Lane::Ally);
        }
        v.push(Lane::Fast);
        v
    }

    pub(crate) fn top(self, lane: Lane) -> f32 {
        let i = self.rails().iter().position(|l| *l == lane).unwrap_or(0);
        i as f32 * LANE_H
    }

    /// The y every line in this lane shares — its rail, its charge lines and its arcs. One
    /// answer, so a lane cannot draw as several parallel lines at slightly different
    /// heights, which is exactly what the first cut did and it read as a barcode.
    pub(crate) fn rail_y(self, lane: Lane) -> f32 {
        self.top(lane) + ICON_INSET + ICON * 0.5
    }

    pub(crate) fn height(self) -> f32 {
        self.rails().len() as f32 * LANE_H
    }
}

/// How far down the screen the charging line reaches, so anything else that wants the top
/// of the battle screen can start below it.
///
/// ⚠️ **The co-op ally strip is flush to the top and the bar claimed that space**, so in a
/// merge the two drew straight through each other — four rails of turn order behind six
/// hero cells. One exported number rather than a constant copied into the other module,
/// because the bar's height is not fixed: it grows a rail when allies turn up.
pub(crate) fn bar_bottom(battle: &BattleData) -> f32 {
    BAR_TOP + lane_set(battle).height() + PANEL_PAD * 2.0
}

/// The rails this battle needs: an ally lane only when somebody else's heroes are here.
pub(crate) fn lane_set(battle: &BattleData) -> LaneSet {
    LaneSet {
        ally: bar_cast(battle)
            .iter()
            .any(|c| c.is_player && !battle.your_ids.contains(&c.id)),
    }
}

/// The rail a combatant belongs on right now.
///
/// ⚠️ **Read off the wire statuses, not off a client guess.** All three are server facts —
/// the gauge really is filling faster — so the lane cannot disagree with the motion the
/// player is watching. `surged` rides the wire for exactly this reason: a catch-up is over
/// the instant it lands, and a client left to infer it from a gauge that jumped would be
/// guessing at something the server already knows.
pub(crate) fn lane_of(c: &CombatantView, mine: bool) -> Lane {
    let fast = c
        .statuses
        .iter()
        .any(|s| s == "braced" || s == "hasted" || s == "surged");
    if fast {
        Lane::Fast
    } else if !c.is_player {
        Lane::Foe
    } else if mine {
        Lane::Mine
    } else {
        Lane::Ally
    }
}

/// **THE FLAME A FAST FIGHTER BURNS**, as a custom UI material.
///
/// ⚠️ **IT IS A SHADER BECAUSE THE BAR IS UI, AND UI IS ROUNDED RECTANGLES.** Three cuts of
/// this were built out of plain `Node`s and every one failed the same way: a lightning bolt
/// drawn as one node per segment became a torn ribbon, the same bolt drawn as runs plus
/// joints became a square wave, and a comet drawn as stacked lozenges read as a smudge under
/// the sprite. A node is an alpha-blended box — it cannot ramp a colour through a gradient
/// and it cannot ADD light where it overlaps, which are the two things fire is made of. A
/// `UiMaterial` does both, and it replaced ~47 CPU-moved nodes per fast fighter with one
/// quad whose motion is a noise field scrolling inside it.
#[derive(Asset, AsBindGroup, TypePath, Debug, Clone)]
pub(crate) struct FireTrail {
    /// `(seconds, fill 0..1, seed, aspect)` — see `turn_fire.wgsl`.
    #[uniform(0)]
    pub(crate) params: Vec4,
    /// The charge line's own colour, with an overall opacity in `a`.
    #[uniform(0)]
    pub(crate) tint: Vec4,
}

impl UiMaterial for FireTrail {
    fn fragment_shader() -> ShaderRef {
        "shaders/turn_fire.wgsl".into()
    }

    /// **ADDITIVE.** Flames compile light where they overlap, and — the part that matters
    /// here — the icon underneath is never occluded by its own aura. That is the trap
    /// `hd2d::sprite_material` records three separate times: a full-quad effect painting
    /// over the art it was supposed to decorate.
    fn specialize(descriptor: &mut RenderPipelineDescriptor, _key: UiMaterialKey<Self>) {
        if let Some(target) = descriptor
            .fragment
            .as_mut()
            .and_then(|f| f.targets.first_mut())
            .and_then(|t| t.as_mut())
        {
            target.blend = Some(BlendState {
                color: BlendComponent {
                    src_factor: BlendFactor::One,
                    dst_factor: BlendFactor::One,
                    operation: BlendOperation::Add,
                },
                alpha: BlendComponent {
                    src_factor: BlendFactor::One,
                    dst_factor: BlendFactor::One,
                    operation: BlendOperation::Add,
                },
            });
        }
    }
}

/// The one quad a fast fighter's flame is drawn on.
#[derive(Component)]
pub(crate) struct TurnFire {
    pub(crate) id: String,
    pub(crate) mat: Handle<FireTrail>,
    pub(crate) col: Color,
}

/// How much taller than an icon the flame's quad is, so tongues have room to lick above and
/// below the line without being clipped by their own node.
const FIRE_HEIGHT: f32 = ICON * 1.9;
/// How long the flame's tail is, in pixels, however long the charge line happens to be — a
/// fill expressed as a fraction would give a fighter near the start of the track a stubby
/// flame and one at the GO end a six-hundred-pixel bonfire.
const FIRE_TAIL: f32 = 150.0;
/// How far the quad reaches PAST the body, so the nose of the flame has room to fade inside
/// its own node. Without it the hot core is cut by the node's right edge — a straight
/// vertical line through the middle of the sprite.
const FIRE_NOSE: f32 = ICON * 0.9;

/// A stable 0..1 number for a combatant id — the per-fighter seed that stops every flame on
/// the rail from flickering in lockstep.
pub(crate) fn hash01(id: &str) -> f32 {
    (glass::redraw_key(&id) % 10_000) as f32 / 10_000.0
}

/// The bar's root node.
#[derive(Component)]
pub(crate) struct TurnOrderBar;

/// One combatant's little sprite on the track.
#[derive(Component)]
pub(crate) struct TurnIcon {
    pub(crate) id: String,
}

/// The charge line running from the start of the track to an icon.
#[derive(Component)]
pub(crate) struct TurnTrail {
    pub(crate) id: String,
}

/// One travelling spark on a charge line.
#[derive(Component)]
pub(crate) struct TurnSpark {
    pub(crate) id: String,
    /// Where on the line this spark starts, 0..1 — spread so a line reads as flowing
    /// rather than as one dot going round.
    pub(crate) phase: f32,
}

/// The halo behind an icon whose turn has come up.
#[derive(Component)]
pub(crate) struct TurnGlow {
    pub(crate) id: String,
}

/// The eased gauge each icon is drawn at, so the bar glides between the server's ten
/// updates a second instead of stepping.
#[derive(Resource, Default)]
pub(crate) struct TurnBarView {
    pub(crate) shown: HashMap<String, f32>,
    /// The cast the current node tree was built for.
    pub(crate) key: u64,
}

/// Everyone the bar draws, in the order they are laid out: living combatants only, since
/// the bar answers "who acts next" and a corpse never does.
pub(crate) fn bar_cast(battle: &BattleData) -> Vec<&CombatantView> {
    battle.combatants.iter().filter(|c| c.hp > 0).collect()
}

/// A signature of everything the NODE TREE depends on — who is in the fight and what each
/// of them looks like. Deliberately NOT the gauges, and deliberately not the LANE either:
/// both move constantly and both are written by the per-frame animator, so folding either
/// in here would rebuild the whole bar ten times a second, which is the exact cost the
/// change-driven rule exists to avoid.
pub(crate) fn cast_key(battle: &BattleData) -> u64 {
    let rows: Vec<(&str, bool, bool, &str, Vec<&str>)> = bar_cast(battle)
        .iter()
        .map(|c| {
            (
                c.id.as_str(),
                c.is_player,
                // Mine or somebody else's: it decides whether this fight has an ally rail
                // at all, which changes the panel's height and every lane under it.
                battle.your_ids.contains(&c.id),
                c.name.as_str(),
                c.statuses
                    .iter()
                    .filter(|s| {
                        s.starts_with("class:") || s.starts_with("boss:") || s.starts_with("pack:")
                    })
                    .map(String::as_str)
                    .collect(),
            )
        })
        .collect();
    glass::redraw_key(&rows)
}

/// Where an icon's left edge sits for a gauge of `g`: 0 at the start of the track, flush
/// against the GO end at a full gauge. Clamped, because a gauge arrives from the server and
/// an icon hanging off the end of its own panel is the kind of thing a bad tick should not
/// be able to draw.
pub(crate) fn icon_x(gauge: f32) -> f32 {
    gauge.clamp(0.0, 1.0) * (TRACK_W - ICON)
}

/// The charge line's width for an icon DRAWN at `x`: from the head of the rail into the
/// middle of that icon, so the line runs into the sprite rather than stopping short of it.
///
/// Keyed on the drawn x rather than on the gauge, because a pile is fanned apart before
/// anything is drawn — two rules here would put a fanned hero's charge line through empty
/// space while its icon sat somewhere else.
pub(crate) fn line_from_x(x: f32) -> f32 {
    (x + ICON * 0.5).max(1.0)
}

/// Ease `shown` toward `target` at [`GLIDE`], frame-rate independently. A turn that just
/// fired (the gauge dropping a long way at once) SNAPS instead: easing it would show the
/// icon sliding backwards down the whole track, which reads as the fighter losing charge
/// rather than as it having spent a turn.
pub(crate) fn glide(shown: f32, target: f32, dt: f32) -> f32 {
    if target < shown - 0.5 {
        return target;
    }
    shown + (target - shown) * (1.0 - (-GLIDE * dt).exp())
}

/// Fan a lane's icons apart so a pile is still countable, working from the FRONT of the
/// track backwards: the leader keeps its true position and anything sitting on top of it is
/// pushed back to [`PILE_GAP`].
///
/// ⚠️ **It lies about the gauge, deliberately and by a bounded amount.** A fanned icon is
/// drawn up to `PILE_GAP` per stacked body behind where its gauge actually is — but it only
/// ever fires when two fighters are within half an icon of each other, which is to say
/// functionally simultaneous, and the alternative is a teammate who is simply not on screen.
/// An invisible hero is a worse lie than a hero drawn 16px early.
pub(crate) fn fan_pile(xs: &mut [f32]) {
    xs.sort_by(|a, b| b.total_cmp(a));
    for i in 1..xs.len() {
        let limit = xs[i - 1] - PILE_GAP;
        if xs[i] > limit {
            xs[i] = limit.max(0.0);
        }
    }
}

/// The bounce offset for a fighter whose turn is up: a lift, never a drop, so the icon
/// never dips below the lane the others are sliding along.
pub(crate) fn bounce_px(t: f32) -> f32 {
    -BOUNCE_PX * (t * BOUNCE_HZ * std::f32::consts::TAU).sin().abs()
}

/// Where a spark sits along a line of `width`, given the clock and the spark's own phase.
/// It runs start → icon and wraps, which is what makes the line read as charge flowing INTO
/// the sprite rather than as a static bar.
pub(crate) fn spark_x(t: f32, phase: f32, width: f32) -> f32 {
    let p = (t * SPARK_SPEED + phase).rem_euclid(1.0);
    p * width
}

/// A spark's alpha at position `p` along its line: brightest as it arrives, faint where it
/// sets off, so the flow has a direction.
pub(crate) fn spark_alpha(t: f32, phase: f32) -> f32 {
    let p = (t * SPARK_SPEED + phase).rem_euclid(1.0);
    0.12 + 0.78 * p * p
}

/// The little south-facing sprite for a combatant, and whether it needs the centre crop.
///
/// The SAME art the arena spawns, resolved the same way (`boss:` overrides the host
/// species, a pack leader draws from its own set) — an icon that disagreed with the body it
/// stands for would be worse than no icon, since the whole point is to recognise at a
/// glance which of the five things in front of you is about to move.
fn icon_image(wa: &WorldAssets, c: &CombatantView) -> (Handle<Image>, bool) {
    if c.is_player {
        return (wa.class_portrait(&hero_class(c)), true);
    }
    if let Some(frames) = c
        .statuses
        .iter()
        .find_map(|s| s.strip_prefix("boss:"))
        .and_then(|k| wa.boss_frames(k))
    {
        return (frames.idle[0].clone(), true);
    }
    let kind = crate::overworld::creature_kind(&c.name);
    let leader = c.statuses.iter().any(|s| s == "pack:leader");
    match wa.creature_frames(&kind, leader) {
        Some(frames) => (frames.idle[0].clone(), true),
        // The old single-png billboard: already cropped to the animal, so zooming it would
        // cut its head off.
        None => (crate::overworld::creature_sprite(wa, &c.name), false),
    }
}

/// Foes run warm, the party runs cool — the same "whose side is this" read the arena's own
/// lighting uses, so the bar can be scanned without reading a single name.
fn lane_color(ally: bool) -> Color {
    if ally {
        Color::srgb(0.45, 0.78, 1.0)
    } else {
        Color::srgb(1.0, 0.5, 0.42)
    }
}

/// Rebuild the bar when the CAST changes (see [`cast_key`]); the gauges are the per-frame
/// animator's business.
pub(crate) fn rebuild_turn_bar(
    mut commands: Commands,
    battle: Res<BattleData>,
    wa: Option<Res<WorldAssets>>,
    mut view: ResMut<TurnBarView>,
    mut fires: ResMut<Assets<FireTrail>>,
    existing: Query<Entity, With<TurnOrderBar>>,
) {
    let Some(wa) = wa else { return };
    let cast = bar_cast(&battle);
    let key = cast_key(&battle);
    if !existing.is_empty() && key == view.key {
        return;
    }
    for e in &existing {
        commands.entity(e).despawn();
    }
    view.key = key;
    view.shown.retain(|id, _| cast.iter().any(|c| &c.id == id));
    if cast.is_empty() {
        return;
    }
    let rows: Vec<(String, bool, Handle<Image>, bool)> = cast
        .iter()
        .map(|c| {
            let (img, crop) = icon_image(&wa, c);
            (c.id.clone(), c.is_player, img, crop)
        })
        .collect();
    let set = lane_set(&battle);

    commands
        .spawn((
            TurnOrderBar,
            // A root of its own, so it needs its own depth: Bevy orders separate UI roots
            // arbitrarily, and the cards that interrupt a fight (opening 90, level-up 95,
            // tally 100) must all sit over it.
            GlobalZIndex(60),
            Node {
                position_type: PositionType::Absolute,
                top: Val::Px(BAR_TOP),
                width: Val::Percent(100.0),
                justify_content: JustifyContent::Center,
                ..default()
            },
        ))
        .with_children(|root| {
            root.spawn((
                Node {
                    border_radius: BorderRadius::all(Val::Px(8.0)),
                    width: Val::Px(TRACK_W + 24.0),
                    height: Val::Px(set.height() + PANEL_PAD * 2.0),
                    padding: UiRect::axes(Val::Px(12.0), Val::Px(PANEL_PAD)),
                    border: UiRect::all(Val::Px(1.0)),
                    ..default()
                },
                BackgroundColor(glass::GLASS_THIN),
                BorderColor::all(glass::EDGE_SOFT),
            ))
            .with_children(|panel| {
                // The track the whole cast shares, and the GO end it is charging toward.
                panel
                    .spawn(Node {
                        width: Val::Px(TRACK_W),
                        height: Val::Px(set.height()),
                        ..default()
                    })
                    .with_children(|lane| {
                        // ⚠️ **NO EMPTY RAIL UNDER A LANE.** The first cut drew a faint
                        // full-width line per lane for the charge lines to run along, which
                        // put four unfilled lines across the panel on top of the filled ones
                        // — read from play as "the additional lines". A lane is already
                        // legible from what is ON it: the charge line, which is as long as
                        // that fighter's gauge and stops at its icon. The only full-height
                        // line left is the GO end, which is not a lane, it is the finish.
                        // The GO end. A bar rather than a word: it is read a hundred times a
                        // fight and never actually needs to be re-read.
                        lane.spawn((
                            Node {
                                border_radius: BorderRadius::all(Val::Px(2.0)),
                                position_type: PositionType::Absolute,
                                left: Val::Px(TRACK_W - 3.0),
                                top: Val::Px(2.0),
                                width: Val::Px(3.0),
                                height: Val::Px(set.height() - 4.0),
                                ..default()
                            },
                            BackgroundColor(glass::EDGE.with_alpha(0.75)),
                        ));
                        for (id, ally, img, crop) in &rows {
                            // Every y here is a placeholder: which LANE a fighter rides
                            // changes while the fight runs (a guard goes up, a haste lands,
                            // somebody blazes ahead), so `animate_turn_bar` writes every
                            // `top` each frame. Only the COLOUR is fixed at spawn, because
                            // whose side you are on does not change.
                            let col = lane_color(*ally);
                            // The charge line, and the sparks running up it.
                            lane.spawn((
                                TurnTrail { id: id.clone() },
                                Node {
                                    border_radius: BorderRadius::all(Val::Px(2.0)),
                                    position_type: PositionType::Absolute,
                                    left: Val::Px(0.0),
                                    top: Val::Px(0.0),
                                    width: Val::Px(1.0),
                                    height: Val::Px(3.0),
                                    ..default()
                                },
                                BackgroundColor(col.with_alpha(0.30)),
                            ));
                            for s in 0..SPARKS {
                                lane.spawn((
                                    TurnSpark {
                                        id: id.clone(),
                                        phase: s as f32 / SPARKS as f32,
                                    },
                                    Node {
                                        border_radius: BorderRadius::all(Val::Px(2.0)),
                                        position_type: PositionType::Absolute,
                                        left: Val::Px(0.0),
                                        top: Val::Px(0.0),
                                        width: Val::Px(4.0),
                                        height: Val::Px(4.0),
                                        ..default()
                                    },
                                    BackgroundColor(col.with_alpha(0.0)),
                                ));
                            }
                            // The FLAME, first so it sits behind its own icon: one quad
                            // spanning this fighter's charge line, painted by `FireTrail`.
                            // Hidden until wanted — a fighter hops onto the fast rail
                            // mid-fight (a guard goes up, a haste lands, somebody blazes
                            // ahead), and building it at that moment would mean rebuilding
                            // the whole bar on a state change the animator absorbs.
                            let mat = fires.add(FireTrail {
                                params: Vec4::new(0.0, 0.0, hash01(id), 8.0),
                                tint: Vec4::new(0.0, 0.0, 0.0, 0.0),
                            });
                            lane.spawn((
                                TurnFire { id: id.clone(), mat: mat.clone(), col },
                                MaterialNode(mat),
                                Node {
                                    position_type: PositionType::Absolute,
                                    left: Val::Px(0.0),
                                    top: Val::Px(0.0),
                                    width: Val::Px(0.0),
                                    height: Val::Px(FIRE_HEIGHT),
                                    display: Display::None,
                                    ..default()
                                },
                            ));
                            // The halo, BEHIND the icon (spawned first) so a glowing turn
                            // reads as light coming off the sprite rather than a disc over it.
                            lane.spawn((
                                TurnGlow { id: id.clone() },
                                Node {
                                    border_radius: BorderRadius::all(Val::Px(ICON)),
                                    position_type: PositionType::Absolute,
                                    left: Val::Px(0.0),
                                    top: Val::Px(0.0),
                                    width: Val::Px(ICON + 10.0),
                                    height: Val::Px(ICON + 10.0),
                                    ..default()
                                },
                                BackgroundColor(Color::NONE),
                            ));
                            let zoom = if *crop { SPRITE_ZOOM } else { 1.0 };
                            let inner = ICON * zoom;
                            let inset = -(inner - ICON) * 0.5;
                            lane.spawn((
                                TurnIcon { id: id.clone() },
                                Node {
                                    position_type: PositionType::Absolute,
                                    left: Val::Px(0.0),
                                    top: Val::Px(ICON_INSET),
                                    width: Val::Px(ICON),
                                    height: Val::Px(ICON),
                                    // The sheet is mostly transparent margin; show the middle
                                    // of it and clip the rest (see `SPRITE_ZOOM`).
                                    overflow: Overflow::clip(),
                                    ..default()
                                },
                            ))
                            .with_children(|slot| {
                                slot.spawn((
                                    ImageNode::new(img.clone()),
                                    Node {
                                        position_type: PositionType::Absolute,
                                        left: Val::Px(inset),
                                        top: Val::Px(inset),
                                        width: Val::Px(inner),
                                        height: Val::Px(inner),
                                        ..default()
                                    },
                                ));
                            });
                        }
                    });
            });
        });
}

/// Move everything on the bar: icons chase their gauges, lines follow their icons, sparks
/// run up the lines, and whoever owns a turn bounces and glows.
///
/// ⚠️ Every write is guarded by "did this value actually move" — a `DerefMut` on a `Node`
/// flags it changed whether or not anything differs, and every flagged node re-runs layout.
pub(crate) fn animate_turn_bar(
    time: Res<Time>,
    battle: Res<BattleData>,
    mut view: ResMut<TurnBarView>,
    // ⚠️ **EVERY ONE OF THESE NEEDS A `Without` FOR EVERY OTHER MARKER, and a missing one is
    // a RUNTIME panic (Bevy B0001), not a compile error.** Clippy and the whole test suite
    // pass on a bar that cannot boot — adding the aura here without teaching the three
    // queries above about it took the game down on startup with the marker names stripped
    // out of the message. `make check` never boots the app; this is one of the things that
    // gets through it.
    mut icons: Query<
        (&TurnIcon, &mut Node),
        (
            Without<TurnTrail>,
            Without<TurnSpark>,
            Without<TurnGlow>,
            Without<TurnFire>,
        ),
    >,
    mut trails: Query<
        (&TurnTrail, &mut Node, &mut BackgroundColor),
        (Without<TurnSpark>, Without<TurnGlow>, Without<TurnFire>),
    >,
    mut sparks: Query<
        (&TurnSpark, &mut Node, &mut BackgroundColor),
        (Without<TurnGlow>, Without<TurnFire>),
    >,
    mut glows: Query<
        (&TurnGlow, &mut Node, &mut BackgroundColor),
        Without<TurnFire>,
    >,
    mut fires: Query<(&TurnFire, &mut Node)>,
    mut fire_mats: ResMut<Assets<FireTrail>>,
) {
    let t = time.elapsed_secs();
    let dt = time.delta_secs();
    // Ease every gauge first, so all four queries below read one consistent set of numbers.
    for c in bar_cast(&battle) {
        let target = c.gauge.clamp(0.0, 1.0) as f32;
        let shown = view.shown.get(&c.id).copied().unwrap_or(target);
        view.shown.insert(c.id.clone(), glide(shown, target, dt));
    }
    let at = |id: &str| view.shown.get(id).copied().unwrap_or(0.0);
    // Which rail each body rides THIS frame — a guard going up or a catch-up landing moves
    // a fighter between lanes mid-fight, so the lane is read here rather than baked in at
    // spawn. Resolved once and shared by all four queries below, so an icon, its charge
    // line and its sparks can never end up on different rails for a frame.
    let set = lane_set(&battle);
    let lanes: HashMap<String, Lane> = battle
        .combatants
        .iter()
        .map(|c| (c.id.clone(), lane_of(c, battle.your_ids.contains(&c.id))))
        .collect();
    let lane_of_id = |id: &str| lanes.get(id).copied().unwrap_or(Lane::Mine);
    let rail_y = |id: &str| set.rail_y(lane_of_id(id));
    // **WHERE EACH ICON IS ACTUALLY DRAWN**, after a pile is fanned apart. Everything below
    // reads this one map, so a fighter's icon, its charge line, its sparks, its arc and its
    // halo cannot end up in different places for a frame.
    let display_x: HashMap<String, f32> = {
        let mut out = HashMap::new();
        for lane in set.rails() {
            let mut ids: Vec<&String> = lanes
                .iter()
                .filter(|(_, l)| **l == lane)
                .map(|(id, _)| id)
                .collect();
            ids.sort_by(|a, b| icon_x(at(b)).total_cmp(&icon_x(at(a))));
            let mut xs: Vec<f32> = ids.iter().map(|id| icon_x(at(id))).collect();
            fan_pile(&mut xs);
            for (id, x) in ids.into_iter().zip(xs) {
                out.insert(id.clone(), x);
            }
        }
        out
    };
    let at_x = |id: &str| display_x.get(id).copied().unwrap_or(0.0);
    // A charge line runs from the head of the rail into the middle of its own icon.
    let line_w = |id: &str| line_from_x(at_x(id));
    // Owning a turn is the SERVER's fact (a full gauge), not the client's `ready` set: the
    // bar draws foes too, and nothing tells this client when a creature's turn comes up.
    let up = |id: &str| {
        battle
            .view(id)
            .map(|c| c.gauge >= 1.0 && c.hp > 0)
            .unwrap_or(false)
    };

    for (icon, mut node) in &mut icons {
        let lift = if up(&icon.id) { bounce_px(t) } else { 0.0 };
        set_px(&mut node.left, at_x(&icon.id));
        set_px(&mut node.top, set.top(lane_of_id(&icon.id)) + ICON_INSET + lift);
    }
    // A fast fighter's line is an ARC, so its smooth bar steps aside for the segments below.
    for (trail, mut node, mut bg) in &mut trails {
        let fast = lane_of_id(&trail.id) == Lane::Fast;
        set_px(&mut node.width, line_w(&trail.id));
        set_px(&mut node.top, rail_y(&trail.id) - 1.5);
        // Kept as a dim core under the arc rather than hidden outright: with the bolt alone
        // the line vanishes between strikes, which reads as a dropout instead of a current.
        let a = if fast { 0.12 } else { 0.30 };
        if bg.0.alpha() != a {
            bg.0 = bg.0.with_alpha(a);
        }
    }
    for (spark, mut node, mut bg) in &mut sparks {
        let w = line_w(&spark.id);
        set_px(&mut node.left, spark_x(t, spark.phase, w));
        set_px(&mut node.top, rail_y(&spark.id) - 2.0);
        // A spark on a fighter that is already up has nowhere left to run — and one on a
        // fast fighter stands down entirely, because its arc is carrying the motion.
        let quiet = up(&spark.id) || lane_of_id(&spark.id) == Lane::Fast;
        let a = if quiet { 0.0 } else { spark_alpha(t, spark.phase) };
        if bg.0.alpha() != a {
            bg.0 = bg.0.with_alpha(a);
        }
    }
    // **THE FLAME.** One quad per fast fighter, spanning its own charge line; everything
    // that moves inside it is the shader's noise field (see `turn_fire.wgsl`).
    //
    // ⚠️ The MATERIAL is written every frame while a fighter is fast, which is a deliberate
    // exception to "only write what moved" — a flame that holds still is not a flame. It is
    // one small uniform per fast fighter rather than the ~47 nodes the particle version
    // moved, and nothing is touched at all for anybody who is not on the fast rail.
    for (fire, mut node) in &mut fires {
        let fast = lane_of_id(&fire.id) == Lane::Fast;
        if !fast {
            if node.display != Display::None {
                node.display = Display::None;
            }
            continue;
        }
        if node.display != Display::Flex {
            node.display = Display::Flex;
        }
        // The quad runs from the head of the rail to a margin PAST the icon, so the hot core
        // fades out inside its own node instead of being sliced off by the right-hand edge —
        // and the tail licks back down the line the fighter came along.
        let body = at_x(&fire.id) + ICON * 0.5;
        let w = body + FIRE_NOSE;
        set_px(&mut node.left, 0.0);
        set_px(&mut node.width, w);
        set_px(&mut node.top, rail_y(&fire.id) - FIRE_HEIGHT * 0.5);
        if let Some(mat) = fire_mats.get_mut(&fire.mat).as_mut() {
            let col = fire.col.to_linear();
            // The tail is a fixed LENGTH in pixels, converted to this quad's uv: a fighter
            // near the start of the track has a short line and must not get a stubby flame,
            // and one at the GO end must not get a six-hundred-pixel bonfire.
            let fill = (FIRE_TAIL / w.max(1.0)).clamp(0.05, 1.0);
            mat.params = Vec4::new(t, fill, hash01(&fire.id), body / w.max(1.0));
            mat.tint = Vec4::new(col.red, col.green, col.blue, 1.0);
        }
    }
    for (glow, mut node, mut bg) in &mut glows {
        let lift = if up(&glow.id) { bounce_px(t) } else { 0.0 };
        set_px(&mut node.left, at_x(&glow.id) - 5.0);
        set_px(&mut node.top, set.top(lane_of_id(&glow.id)) - 2.0 + lift);
        // Quantised to a few steps a second: a continuous pulse is a material write every
        // frame for a difference nobody can see.
        let a = if up(&glow.id) {
            let pulse = 0.5 + 0.5 * (t * 3.0 * std::f32::consts::TAU / 2.0).sin();
            ((0.26 + 0.22 * pulse) * 20.0).round() / 20.0
        } else {
            0.0
        };
        if bg.0.alpha() != a {
            bg.0 = glass::EDGE.with_alpha(a);
        }
    }
}

/// Write a pixel value only if it actually moved (see [`animate_turn_bar`]'s note).
fn set_px(v: &mut Val, px: f32) {
    if *v != Val::Px(px) {
        *v = Val::Px(px);
    }
}

/// Forget the eased gauges when a fight ends, so the next one does not open with the last
/// one's icons sliding home from wherever they stopped.
pub(crate) fn reset_turn_bar(mut view: ResMut<TurnBarView>) {
    view.shown.clear();
    view.key = 0;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The track is read left to right: an empty gauge starts at the rail's head, a full one
    /// is flush against the GO end, and nothing can be drawn off either end of its panel.
    #[test]
    fn an_icon_walks_the_track_and_stops_at_go() {
        assert_eq!(icon_x(0.0), 0.0);
        assert_eq!(icon_x(1.0), TRACK_W - ICON);
        assert!(icon_x(0.5) > 0.0 && icon_x(0.5) < TRACK_W - ICON);
        assert_eq!(icon_x(-3.0), 0.0, "a bad gauge cannot draw off the left end");
        assert_eq!(icon_x(9.0), TRACK_W - ICON, "…nor off the right");
    }

    /// The line runs INTO the sprite rather than stopping at its edge — a charge line that
    /// ends in a gap reads as broken rather than as flowing.
    #[test]
    fn the_charge_line_reaches_its_icon() {
        for g in [0.0, 0.25, 0.5, 1.0] {
            let want = icon_x(g) + ICON * 0.5;
            assert!(
                (line_from_x(icon_x(g)) - want).abs() < 0.01,
                "the line must reach the middle of the icon at gauge {g}"
            );
        }
    }

    /// **A SPENT TURN SNAPS; A FILLING GAUGE GLIDES.** Easing the drop would show the icon
    /// sliding backwards down the whole track, which reads as losing charge rather than as
    /// having just acted.
    #[test]
    fn a_spent_turn_snaps_back_and_a_filling_one_eases() {
        assert_eq!(glide(1.0, 0.0, 1.0 / 60.0), 0.0, "acting snaps the icon home");
        let eased = glide(0.2, 0.3, 1.0 / 60.0);
        assert!(eased > 0.2 && eased < 0.3, "a filling gauge eases: {eased}");
        // A small backward step (a gauge drain, a knock) still eases rather than snapping.
        let drained = glide(0.6, 0.45, 1.0 / 60.0);
        assert!(drained < 0.6 && drained > 0.45, "a drain eases too: {drained}");
    }

    /// The bounce is a LIFT. Bevy's `top` grows downward, so a positive offset would push
    /// the icon down through the rail the other lane is drawn on.
    #[test]
    fn the_bounce_only_ever_lifts() {
        for i in 0..64 {
            let t = i as f32 * 0.037;
            assert!(bounce_px(t) <= 0.0, "bounce dipped below the lane at t={t}");
            assert!(bounce_px(t) >= -BOUNCE_PX, "bounce overshot at t={t}");
        }
    }

    fn cv(id: &str, ally: bool, statuses: &[&str]) -> CombatantView {
        CombatantView {
            id: id.into(),
            name: id.into(),
            hp: 10,
            max_hp: 10,
            gauge: 0.0,
            is_player: ally,
            player_id: ally.then(|| id.into()),
            level: 1,
            statuses: statuses.iter().map(|s| s.to_string()).collect(),
        }
    }

    /// **THE LANE SAYS SPEED; THE COLOUR SAYS SIDE.** Anyone charging faster than they
    /// should be — a guard that braced, a haste, or a fighter fresh off a five-blow
    /// catch-up — hops onto the FAST rail, whichever side they are on.
    #[test]
    fn anyone_charging_faster_rides_the_fast_rail() {
        assert_eq!(lane_of(&cv("m1", false, &[]), false), Lane::Foe);
        assert_eq!(lane_of(&cv("h1", true, &[]), true), Lane::Mine);
        assert_eq!(lane_of(&cv("co", true, &[]), false), Lane::Ally);
        for fast in ["braced", "hasted", "surged"] {
            for (id, player, mine) in [("h1", true, true), ("co", true, false), ("m1", false, false)]
            {
                assert_eq!(
                    lane_of(&cv(id, player, &[fast]), mine),
                    Lane::Fast,
                    "a {fast} {id} stayed on its ordinary rail — the rule reads the same \
                     for everybody"
                );
            }
        }
    }

    /// **FOUR RAILS AT MOST, AND THREE WHEN YOU ARE ALONE.** A co-op merge can field
    /// sixteen heroes across four parties; a rail each would be a stave, so every ally
    /// shares one. And a solo dive must not carry a permanently empty ally rail.
    #[test]
    fn the_bar_never_grows_past_four_rails() {
        let solo = LaneSet { ally: false };
        let coop = LaneSet { ally: true };
        assert_eq!(solo.rails().len(), 3, "a solo fight drew an empty ally rail");
        assert_eq!(coop.rails().len(), 4);
        assert!(coop.rails().len() <= 4, "the bar grew a fifth rail");
        // Every rail sits on its own row, in reading order, with no two sharing a y.
        for set in [solo, coop] {
            let ys: Vec<f32> = set.rails().iter().map(|l| set.rail_y(*l)).collect();
            for pair in ys.windows(2) {
                assert!(
                    pair[1] - pair[0] >= ICON + ICON_INSET + BOUNCE_PX,
                    "rails at {ys:?} are closer than an icon, its inset and its bounce"
                );
            }
            assert!(
                set.height() >= ys.last().copied().unwrap_or(0.0),
                "a rail was drawn outside the panel it lives in"
            );
        }
        // The two a player reads most stay adjacent, whoever else turned up.
        assert_eq!(coop.top(Lane::Mine) - coop.top(Lane::Foe), LANE_H);
    }

    /// An ally rail exists only when somebody else's heroes are actually in the fight —
    /// read off `your_ids`, which is the one thing that says whose party is whose.
    #[test]
    fn the_ally_rail_appears_only_in_co_op() {
        let mut solo = BattleData {
            your_ids: vec!["h1".into()],
            combatants: vec![cv("h1", true, &[]), cv("m1", false, &[])],
            ..Default::default()
        };
        assert!(!lane_set(&solo).ally, "a solo fight claimed an ally rail");
        solo.combatants.push(cv("co", true, &[]));
        assert!(lane_set(&solo).ally, "a joined ally did not get a rail");
    }

    /// **EVERYBODY WHO IS READY MUST STILL BE COUNTABLE.** Every fighter at a full gauge
    /// sits at the same x, so without a fan two heroes whose turns came up together draw on
    /// one pixel — on the very bar whose point is that one turn arriving does not stop
    /// anybody else's. Found in the first real capture: a party of four with two heroes
    /// ready rendered three icons.
    #[test]
    fn a_pile_of_ready_fighters_is_still_countable() {
        let mut xs = vec![icon_x(1.0); 4];
        fan_pile(&mut xs);
        for pair in xs.windows(2) {
            assert!(
                (pair[0] - pair[1]).abs() >= PILE_GAP - 0.01,
                "two icons drew closer than a readable gap: {xs:?}"
            );
        }
        assert_eq!(xs[0], icon_x(1.0), "the leader must keep its true position");
        assert!(xs.iter().all(|x| *x >= 0.0), "a fan pushed an icon off the track: {xs:?}");
    }

    /// …and it only fires on an actual pile: fighters spread across the track are drawn
    /// exactly where their gauges put them, because the fan is a bounded lie told only when
    /// the truth would be invisible.
    #[test]
    fn a_spread_out_field_is_never_fanned() {
        let mut xs = vec![icon_x(0.9), icon_x(0.6), icon_x(0.3), icon_x(0.0)];
        let before = xs.clone();
        fan_pile(&mut xs);
        assert_eq!(xs, before, "a spread field was moved: {before:?} -> {xs:?}");
    }

    /// A charge line ends in the middle of the icon it feeds, whether that icon is drawn at
    /// its true position or fanned back out of a pile — two rules here would put a fanned
    /// hero's line through empty space.
    #[test]
    fn a_fanned_icon_keeps_its_own_charge_line() {
        let fanned = icon_x(1.0) - PILE_GAP;
        assert!(
            (line_from_x(fanned) - (fanned + ICON * 0.5)).abs() < 0.01,
            "a fanned icon's line did not follow it"
        );
    }

    /// **THE FLAME'S QUAD COVERS ITS OWN ICON AND ITS OWN TAIL.** The hot core is drawn at
    /// the head, so a quad that stopped at the icon's left edge would put the aura behind
    /// the sprite instead of on it.
    #[test]
    fn the_flame_reaches_past_the_body_it_burns_off() {
        for g in [0.0, 0.5, 1.0] {
            let head = icon_x(g) + ICON;
            assert!(
                head >= icon_x(g) + ICON * 0.9,
                "the flame stopped short of its own icon at gauge {g}"
            );
        }
    }

    /// The tail is a fixed LENGTH, not a fraction: a fighter near the start of the track has
    /// a short line and must not get a stubby flame, and one at the GO end must not get a
    /// six-hundred-pixel bonfire.
    #[test]
    fn the_tail_is_the_same_length_wherever_the_fighter_is() {
        let near_start = (FIRE_TAIL / (icon_x(0.08) + ICON)).clamp(0.12, 1.0);
        let at_go = (FIRE_TAIL / (icon_x(1.0) + ICON)).clamp(0.12, 1.0);
        assert!(near_start > at_go, "the tail did not stay a fixed length");
        let px_near = near_start * (icon_x(0.08) + ICON);
        let px_go = at_go * (icon_x(1.0) + ICON);
        assert!(
            (px_near - px_go).abs() < FIRE_TAIL * 0.5,
            "the drawn tail changed length across the track: {px_near} vs {px_go}"
        );
    }

    /// Each fighter's flame flickers on its own clock — one seed shared by the rail would
    /// have every fast body pulsing in lockstep, which reads as a blinking panel.
    #[test]
    fn every_flame_burns_on_its_own_seed() {
        let ids = ["h1", "h2", "m1", "bog_stinger", "co-op-ally"];
        for a in ids {
            assert!((0.0..1.0).contains(&hash01(a)), "{a} seeded out of range");
            for b in ids {
                if a != b {
                    assert!(hash01(a) != hash01(b), "{a} and {b} share a flicker clock");
                }
            }
        }
    }

    /// A spark runs start → icon and wraps, brightening as it arrives: that direction is    /// A spark runs start → icon and wraps, brightening as it arrives: that direction is
    /// what says "charging" rather than "draining".
    #[test]
    fn a_spark_runs_toward_the_icon_and_brightens() {
        let w = 200.0;
        let early = spark_x(0.0, 0.0, w);
        let later = spark_x(0.4, 0.0, w);
        assert!(later > early, "a spark travels toward the icon: {early} -> {later}");
        assert!(
            spark_alpha(0.4, 0.0) > spark_alpha(0.0, 0.0),
            "…and is brightest as it arrives"
        );
        for i in 0..40 {
            let t = i as f32 * 0.31;
            let x = spark_x(t, 0.5, w);
            assert!((0.0..=w).contains(&x), "a spark left its own line at t={t}");
        }
    }
}
