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

use bevy::prelude::*;

use meld_client::glass;

use crate::battle::lighten;
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
/// Embers in a fast fighter's charge trail — the fuzz it is burning off.
///
/// ⚠️ **THIS REPLACED A LIGHTNING BOLT, WHICH DID NOT LAND.** Two cuts of a zigzag were
/// tried: one node per segment (a big jump became a wide tall BLOCK, and it read as a torn
/// paper ribbon) and then a run plus a vertical joint per vertex, which drew a clean stepped
/// arc and still read as a square wave drawn on the panel rather than as something moving.
/// The shape was never the problem — a hard-edged line cannot say "flying". Streaming
/// particles can, so the trail is fuzz now: embers thrown off the body and left behind.
const EMBERS: usize = 26;
/// How far an ember drifts off the line as it falls behind.
const EMBER_SPREAD: f32 = 7.0;
/// Fastest an ember travels backwards, in fractions of the line per second.
const EMBER_DRIFT: f32 = 0.55;

/// Concentric rings in a fast fighter's aura.
///
/// ⚠️ **FOUR, because a UI node is a FLAT disc.** Bevy's UI has no radial gradient, so two
/// rings drew as two hard-edged circles and read as a smudge of dirt under the sprite
/// rather than as something burning. Stacking several with a falling alpha is how a soft
/// bloom is spelled here.
const AURA_RINGS: usize = 7;
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

/// One ember streaming off a fast fighter — the fuzzy charge trail that says it is FLYING
/// rather than sliding (see [`EMBERS`] for the two hard-edged bolts this replaced).
#[derive(Component)]
pub(crate) struct TurnEmber {
    pub(crate) id: String,
    pub(crate) seed: usize,
    /// The colour of the charge line these embers come off — anything else reads as a
    /// separate effect laid over the line rather than as the line itself burning.
    pub(crate) col: Color,
}

/// One travelling spark on a charge line.
#[derive(Component)]
pub(crate) struct TurnSpark {
    pub(crate) id: String,
    /// Where on the line this spark starts, 0..1 — spread so a line reads as flowing
    /// rather than as one dot going round.
    pub(crate) phase: f32,
}

/// One ring of the aura a fast fighter burns with — the energy it is charging with, in its
/// own line's colour. Two of them per fighter: a soft outer bloom and a tight inner core.
#[derive(Component)]
pub(crate) struct TurnAura {
    pub(crate) id: String,
    pub(crate) ring: usize,
    pub(crate) col: Color,
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

/// A stable per-ember random in `0..1`. Deterministic off its own seed, so an ember keeps
/// its size, its drift and its wobble for as long as it exists instead of reshuffling every
/// frame — which is the difference between a trail and a field of static.
pub(crate) fn ember_rand(seed: usize, salt: u32) -> f32 {
    let h = (seed as u32)
        .wrapping_add(1)
        .wrapping_mul(0x9E37_79B9)
        .wrapping_add(salt.wrapping_mul(0x85EB_CA6B));
    let h = h ^ (h >> 15);
    ((h >> 8) & 0xFFFF) as f32 / 65535.0
}

/// How far along its line an ember is, `1.0` at the body and `0.0` at the far tail. Embers
/// are thrown off the fighter and fall BACKWARDS down the line, which is what says it is
/// travelling rather than that the line is decorated.
pub(crate) fn ember_at(t: f32, seed: usize) -> f32 {
    let speed = EMBER_DRIFT * (0.6 + 0.8 * ember_rand(seed, 3));
    1.0 - (ember_rand(seed, 1) + t * speed).rem_euclid(1.0)
}

/// **WHITE-HOT AT BOTH ENDS.** An ember is brightest right at the body, where the energy is
/// coming off, and again at the very tail, where it is dissipating into a white wisp — the
/// line's own colour in between. Both ends whitening is what makes the trail read as heat
/// rather than as a coloured smear.
pub(crate) fn ember_heat(p: f32) -> f32 {
    let front = ((p - 0.62) / 0.38).clamp(0.0, 1.0);
    let tail = ((0.30 - p) / 0.30).clamp(0.0, 1.0);
    front.max(tail * 0.85)
}

/// An ember's opacity: solid where it leaves the body, gone by the end of the tail.
pub(crate) fn ember_alpha(p: f32) -> f32 {
    (p * p * 0.95).clamp(0.0, 1.0)
}

/// How far off the line an ember has wandered. Flames SPREAD as they fall behind, so the
/// wobble grows with distance from the body — a constant one reads as a wavy rope.
pub(crate) fn ember_wobble(t: f32, seed: usize, p: f32) -> f32 {
    let phase = ember_rand(seed, 5) * std::f32::consts::TAU;
    let rate = 3.0 + 4.0 * ember_rand(seed, 7);
    (t * rate + phase).sin() * EMBER_SPREAD * (1.0 - p)
}

/// One layer of the aura, as (height, how far it reaches BACK past the body, base alpha).
///
/// ⚠️ **THE TAPER IS THE STACK, because one box cannot narrow.** A UI node is a rectangle
/// with rounded corners: rounding its leading edge hard and its trailing edge barely gives a
/// pill, and the first cut of this drew exactly that — a solid red lozenge that read as a
/// highlighted button around the sprite. Stacking lozenges that get SHORTER as they reach
/// further back makes the silhouette itself taper, which is what a comet is: tall and hot at
/// the head, thin and long down the tail.
pub(crate) fn aura_ring(ring: usize) -> (f32, f32, f32) {
    let t = ring as f32 / (AURA_RINGS - 1).max(1) as f32;
    let height = ICON * (0.98 - 0.66 * t);
    let back = 6.0 + 54.0 * t;
    // Low on purpose: four of these stack, and the sprite has to read THROUGH them.
    let alpha = 0.15 * (1.0 - t).powf(1.2) + 0.03;
    (height, back, alpha)
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
                            // The aura, FIRST so it sits behind its own icon: a fighter
                            // charging faster than it should be burns with it. Hidden until
                            // wanted, like the arc below.
                            // Widest first, so the tight hot core lands on top of the bloom.
                            for ring in (0..AURA_RINGS).rev() {
                                lane.spawn((
                                    TurnAura { id: id.clone(), ring, col },
                                    Node {
                                        border_radius: BorderRadius::all(Val::Px(ICON * 2.0)),
                                        position_type: PositionType::Absolute,
                                        left: Val::Px(0.0),
                                        top: Val::Px(0.0),
                                        width: Val::Px(0.0),
                                        height: Val::Px(0.0),
                                        display: Display::None,
                                        ..default()
                                    },
                                    BackgroundColor(Color::NONE),
                                ));
                            }
                            // The charge trail's embers, for when this one is charging faster
                            // than it should be. Spawned for everybody and hidden until
                            // wanted: a fighter hops onto the fast rail mid-fight (a guard
                            // goes up, a haste lands, somebody blazes ahead), and building
                            // the nodes at that moment would mean rebuilding the whole bar
                            // on a state change the animator otherwise absorbs.
                            for seed in 0..EMBERS {
                                lane.spawn((
                                    TurnEmber { id: id.clone(), seed, col },
                                    Node {
                                        border_radius: BorderRadius::all(Val::Px(ICON)),
                                        position_type: PositionType::Absolute,
                                        left: Val::Px(0.0),
                                        top: Val::Px(0.0),
                                        width: Val::Px(0.0),
                                        height: Val::Px(0.0),
                                        display: Display::None,
                                        ..default()
                                    },
                                    BackgroundColor(Color::NONE),
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
            Without<TurnEmber>,
            Without<TurnAura>,
        ),
    >,
    mut trails: Query<
        (&TurnTrail, &mut Node, &mut BackgroundColor),
        (Without<TurnSpark>, Without<TurnGlow>, Without<TurnEmber>, Without<TurnAura>),
    >,
    mut sparks: Query<
        (&TurnSpark, &mut Node, &mut BackgroundColor),
        (Without<TurnGlow>, Without<TurnEmber>, Without<TurnAura>),
    >,
    mut glows: Query<
        (&TurnGlow, &mut Node, &mut BackgroundColor),
        (Without<TurnEmber>, Without<TurnAura>),
    >,
    mut embers: Query<(&TurnEmber, &mut Node, &mut BackgroundColor), Without<TurnAura>>,
    mut auras: Query<(&TurnAura, &mut Node, &mut BackgroundColor)>,
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
    // **THE CHARGE TRAIL.** A fighter charging faster than it should be burns embers off
    // itself and leaves them behind — the fuzz that says it is FLYING rather than sliding.
    //
    // ⚠️ This writes every frame on purpose, against the repo's "only write what moved"
    // rule: a trail that holds still is not a trail. The cost is bounded by construction —
    // every ember is `Display::None` for anybody who is not fast, which is the common case
    // for most of a fight.
    for (ember, mut node, mut bg) in &mut embers {
        if lane_of_id(&ember.id) != Lane::Fast {
            if node.display != Display::None {
                node.display = Display::None;
            }
            continue;
        }
        if node.display != Display::Flex {
            node.display = Display::Flex;
        }
        let w = line_w(&ember.id);
        let p = ember_at(t, ember.seed);
        // Fat where it comes off the body, thinning to a wisp as it falls behind.
        let size = (2.0 + 6.0 * p * ember_rand(ember.seed, 11).mul_add(0.7, 0.5)).max(1.5);
        set_px(&mut node.width, size);
        set_px(&mut node.height, size);
        set_px(&mut node.left, w * p - size * 0.5);
        set_px(
            &mut node.top,
            rail_y(&ember.id) + ember_wobble(t, ember.seed, p) - size * 0.5,
        );
        let heat = ember_heat(p);
        bg.0 = lighten(ember.col, 0.2 + 0.75 * heat).with_alpha(ember_alpha(p));
    }
    // **THE AURA.** A fighter on the fast rail burns with its own line's colour — the visible
    // half of "this one is charging harder than it should be", so the state reads off the
    // BODY as well as off which rail it is standing on.
    for (aura, mut node, mut bg) in &mut auras {
        if lane_of_id(&aura.id) != Lane::Fast {
            if node.display != Display::None {
                node.display = Display::None;
            }
            continue;
        }
        if node.display != Display::Flex {
            node.display = Display::Flex;
        }
        let (h, back, base) = aura_ring(aura.ring);
        // The layers breathe out of phase, so the tail licks rather than blinking.
        let phase = t * 4.0 + aura.ring as f32 * 0.9;
        let pulse = 0.5 + 0.5 * phase.sin();
        let back = back * (0.82 + 0.25 * pulse);
        let head = at_x(&aura.id) + ICON * 0.72;
        let lift = if up(&aura.id) { bounce_px(t) } else { 0.0 };
        set_px(&mut node.width, back + h);
        set_px(&mut node.height, h);
        set_px(&mut node.left, head - (back + h));
        set_px(
            &mut node.top,
            set.top(lane_of_id(&aura.id)) + ICON_INSET + ICON * 0.5 - h * 0.5 + lift,
        );
        // A lozenge: fully round at both ends, so the stack's OUTLINE carries the taper
        // instead of any one box pretending to be a teardrop.
        node.border_radius = BorderRadius::all(Val::Px(h * 0.5));
        // **WHITE AT THE HEAD.** The core layer is nearly white — that is where the energy
        // is coming off — and the wider, longer layers behind it carry the line's colour.
        let core = 1.0 - aura.ring as f32 / (AURA_RINGS - 1).max(1) as f32;
        // ⚠️ The whitening falls off STEEPLY (`core` cubed), so only the head is white-hot
        // and the tail keeps the line's own colour. A linear falloff washed the whole comet
        // out to near-white and lost the one thing the colour is there to say — whose it is.
        bg.0 = lighten(aura.col, 0.12 + 0.8 * core.powi(3) * (0.85 + 0.15 * pulse))
            .with_alpha(base * (0.8 + 0.2 * pulse));
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

    /// **THE TRAIL STREAMS BACKWARDS.** An ember is thrown off the body and falls behind it,
    /// which is the whole reason this replaced a lightning bolt: a hard-edged zigzag sits on
    /// the panel, and only motion says "flying".
    #[test]
    fn an_ember_is_thrown_off_the_body_and_falls_behind() {
        for seed in 0..EMBERS {
            let a = ember_at(0.0, seed);
            let b = ember_at(0.05, seed);
            // It moves back down the line, except across the wrap where it is re-emitted.
            assert!(b < a || a < 0.1, "ember {seed} did not stream backwards: {a} -> {b}");
            for k in 0..30 {
                let p = ember_at(k as f32 * 0.13, seed);
                assert!((0.0..=1.0).contains(&p), "ember {seed} left its own line at {p}");
            }
        }
    }

    /// **WHITE-HOT AT BOTH ENDS**, the line's own colour in between: brightest where the
    /// energy comes off the body, and again where it dissipates into a wisp.
    #[test]
    fn the_trail_is_hottest_at_the_body_and_at_the_tip() {
        assert!(ember_heat(1.0) > ember_heat(0.5), "the body end was not the hottest");
        assert!(ember_heat(0.0) > ember_heat(0.5), "the tail did not whiten as it died");
        for i in 0..=10 {
            let p = i as f32 / 10.0;
            assert!((0.0..=1.0).contains(&ember_heat(p)), "heat left its range at {p}");
        }
    }

    /// It fades out as it falls behind — an ember that stayed solid to the end of the line
    /// draws a hard stripe, which is the thing this shape exists to not be.
    #[test]
    fn an_ember_dies_as_it_falls_behind() {
        assert!(ember_alpha(1.0) > ember_alpha(0.5));
        assert!(ember_alpha(0.5) > ember_alpha(0.1));
        assert_eq!(ember_alpha(0.0), 0.0, "the far tail must reach nothing at all");
    }

    /// Flames SPREAD as they fall behind: the wobble is nothing at the body and widest at
    /// the tail. A constant one reads as a wavy rope rather than as something burning off.
    #[test]
    fn the_flames_spread_as_they_trail() {
        let near: f32 = (0..EMBERS)
            .map(|s| ember_wobble(0.37, s, 0.95).abs())
            .fold(0.0, f32::max);
        let far: f32 = (0..EMBERS)
            .map(|s| ember_wobble(0.37, s, 0.05).abs())
            .fold(0.0, f32::max);
        assert!(far > near, "the tail did not spread: near {near}, far {far}");
        assert!(far <= EMBER_SPREAD, "an ember wandered out of its own lane: {far}");
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
