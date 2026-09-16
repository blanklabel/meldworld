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

use bevy::prelude::*;

use meld_client::glass;

use crate::world_render::WorldAssets;
use crate::{hero_class, BattleData};
use meld_client::net::CombatantView;

/// The bar's track, in pixels. Fixed rather than a percentage: an icon's position along it
/// is a pixel offset written every frame, and a track that re-measured with the window
/// would need every one of them recomputed on resize anyway.
const TRACK_W: f32 = 660.0;
/// One little sprite, square.
const ICON: f32 = 34.0;
/// A lane's height — one row of icons plus the room a bounce needs above it.
const LANE_H: f32 = 40.0;
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
const BOUNCE_PX: f32 = 7.0;
/// Exponential easing rate for an icon chasing its authoritative gauge. The server sends a
/// gauge ten times a second; this is what turns that staircase into a slide.
const GLIDE: f32 = 12.0;

/// The bar's root node.
#[derive(Component)]
pub(crate) struct TurnOrderBar;

/// One combatant's little sprite on the track.
#[derive(Component)]
pub(crate) struct TurnIcon {
    pub(crate) id: String,
    /// Which lane it rides: allies below the centre line, foes above it.
    pub(crate) ally: bool,
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
/// of them looks like. Deliberately NOT the gauges: those move constantly and are what the
/// per-frame animator writes, so folding them in here would rebuild the whole bar ten times
/// a second, which is the exact cost the change-driven rule exists to avoid.
pub(crate) fn cast_key(battle: &BattleData) -> u64 {
    let rows: Vec<(&str, bool, &str, Vec<&str>)> = bar_cast(battle)
        .iter()
        .map(|c| {
            (
                c.id.as_str(),
                c.is_player,
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

/// The charge line's width for a gauge of `g`: from the start of the track to the middle of
/// the icon, so the line runs INTO the sprite rather than stopping short of it.
pub(crate) fn trail_w(gauge: f32) -> f32 {
    (icon_x(gauge) + ICON * 0.5).max(1.0)
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

    commands
        .spawn((
            TurnOrderBar,
            // A root of its own, so it needs its own depth: Bevy orders separate UI roots
            // arbitrarily, and the cards that interrupt a fight (opening 90, level-up 95,
            // tally 100) must all sit over it.
            GlobalZIndex(60),
            Node {
                position_type: PositionType::Absolute,
                top: Val::Px(8.0),
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
                    height: Val::Px(LANE_H * 2.0 + 14.0),
                    padding: UiRect::axes(Val::Px(12.0), Val::Px(7.0)),
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
                        height: Val::Px(LANE_H * 2.0),
                        ..default()
                    })
                    .with_children(|lane| {
                        // The rail: one hairline across the middle, the two lanes' shared
                        // floor. Foes charge above it and the party below it.
                        lane.spawn((
                            Node {
                                position_type: PositionType::Absolute,
                                left: Val::Px(0.0),
                                top: Val::Px(LANE_H - 1.0),
                                width: Val::Px(TRACK_W),
                                height: Val::Px(2.0),
                                ..default()
                            },
                            BackgroundColor(Color::srgba(0.6, 0.72, 1.0, 0.22)),
                        ));
                        // The GO end. A bar rather than a word: it is read a hundred times a
                        // fight and never actually needs to be re-read.
                        lane.spawn((
                            Node {
                                border_radius: BorderRadius::all(Val::Px(2.0)),
                                position_type: PositionType::Absolute,
                                left: Val::Px(TRACK_W - 3.0),
                                top: Val::Px(2.0),
                                width: Val::Px(3.0),
                                height: Val::Px(LANE_H * 2.0 - 4.0),
                                ..default()
                            },
                            BackgroundColor(glass::EDGE.with_alpha(0.75)),
                        ));
                        for (id, ally, img, crop) in &rows {
                            let lane_top = if *ally { LANE_H } else { 0.0 };
                            let col = lane_color(*ally);
                            // The charge line, and the sparks running up it.
                            lane.spawn((
                                TurnTrail { id: id.clone() },
                                Node {
                                    border_radius: BorderRadius::all(Val::Px(2.0)),
                                    position_type: PositionType::Absolute,
                                    left: Val::Px(0.0),
                                    top: Val::Px(lane_top + LANE_H * 0.5 - 1.5),
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
                                        top: Val::Px(lane_top + LANE_H * 0.5 - 2.0),
                                        width: Val::Px(4.0),
                                        height: Val::Px(4.0),
                                        ..default()
                                    },
                                    BackgroundColor(col.with_alpha(0.0)),
                                ));
                            }
                            // The halo, BEHIND the icon (spawned first) so a glowing turn
                            // reads as light coming off the sprite rather than a disc over it.
                            lane.spawn((
                                TurnGlow { id: id.clone() },
                                Node {
                                    border_radius: BorderRadius::all(Val::Px(ICON)),
                                    position_type: PositionType::Absolute,
                                    left: Val::Px(0.0),
                                    top: Val::Px(lane_top),
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
                                TurnIcon { id: id.clone(), ally: *ally },
                                Node {
                                    position_type: PositionType::Absolute,
                                    left: Val::Px(0.0),
                                    top: Val::Px(lane_top + 3.0),
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
    mut icons: Query<(&TurnIcon, &mut Node), (Without<TurnTrail>, Without<TurnSpark>, Without<TurnGlow>)>,
    mut trails: Query<(&TurnTrail, &mut Node), (Without<TurnSpark>, Without<TurnGlow>)>,
    mut sparks: Query<(&TurnSpark, &mut Node, &mut BackgroundColor), Without<TurnGlow>>,
    mut glows: Query<(&TurnGlow, &mut Node, &mut BackgroundColor)>,
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
    // Owning a turn is the SERVER's fact (a full gauge), not the client's `ready` set: the
    // bar draws foes too, and nothing tells this client when a creature's turn comes up.
    let up = |id: &str| {
        battle
            .view(id)
            .map(|c| c.gauge >= 1.0 && c.hp > 0)
            .unwrap_or(false)
    };

    for (icon, mut node) in &mut icons {
        let g = at(&icon.id);
        let lane_top = if icon.ally { LANE_H } else { 0.0 } + 3.0;
        let lift = if up(&icon.id) { bounce_px(t) } else { 0.0 };
        set_px(&mut node.left, icon_x(g));
        set_px(&mut node.top, lane_top + lift);
    }
    for (trail, mut node) in &mut trails {
        set_px(&mut node.width, trail_w(at(&trail.id)));
    }
    for (spark, mut node, mut bg) in &mut sparks {
        let w = trail_w(at(&spark.id));
        set_px(&mut node.left, spark_x(t, spark.phase, w));
        // A spark on a fighter that is already up has nowhere left to run.
        let a = if up(&spark.id) { 0.0 } else { spark_alpha(t, spark.phase) };
        let want = bg.0.with_alpha(a);
        if bg.0.alpha() != a {
            bg.0 = want;
        }
    }
    for (glow, mut node, mut bg) in &mut glows {
        let g = at(&glow.id);
        let ally = battle.view(&glow.id).map(|c| c.is_player).unwrap_or(true);
        let lane_top = if ally { LANE_H } else { 0.0 };
        let lift = if up(&glow.id) { bounce_px(t) } else { 0.0 };
        set_px(&mut node.left, icon_x(g) - 5.0);
        set_px(&mut node.top, lane_top - 2.0 + lift);
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
                (trail_w(g) - want).abs() < 0.01,
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

    /// A spark runs start → icon and wraps, brightening as it arrives: that direction is
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
