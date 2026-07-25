//! Battle: the ATB command panel (d-pad), party HUD, 3D arena actors + camera,
//! targeting, order queue, hit FX, and per-class kits.
//! Extracted from `main.rs` during the module reorg.

use std::collections::{HashMap, HashSet};

use bevy::prelude::*;
use bevy::input::mouse::MouseWheel;

use meld_client::hd2d::{self, CharSprite};
use meld_client::net::{ClientCmd, CombatantView, HitEffect, Net};

use super::*;

// ---------------------------------------------------------------- battle ---

/// Reset the command window to its root page, clearing any pending target choice.
pub(crate) fn reset_menu(menu: &mut BattleMenu) {
    menu.level = MenuLevel::Root;
    menu.cursor = 0;
    menu.dirty = true;
    menu.pending = None;
    menu.rows.clear();
}

/// On entering a battle, open the command window on the root page and clear any
/// enemy target left over from a previous fight. (The static `MELD_BATTLE` mock
/// preseeds a target to show the reticle in a screenshot, so leave that one alone.)
pub(crate) fn enter_battle(
    mut menu: ResMut<BattleMenu>,
    mut target: ResMut<BattleTarget>,
    mut bcam: ResMut<BattleCam>,
) {
    reset_menu(&mut menu);
    bcam.zoom = 1.0; // each fight starts at the automatic fit
    if !battle_mockup_flag() {
        target.selected = None;
    }
}

/// A 3D combatant in the HD-2D battle arena, keyed by its combatant id.
#[derive(Component)]
pub(crate) struct BattleActor {
    id: String,
}

/// The floating diamond marker over an enemy, carrying the enemy id it belongs to
/// and the resting height above that enemy's head. Hidden until that enemy is the
/// current [`BattleTarget`]; bounces + spins while shown (see [`highlight_target`]).
#[derive(Component)]
pub(crate) struct TargetDiamond {
    id: String,
    base_y: f32,
}

/// A battle sprite billboard (hero or enemy), tagged with its combatant id, its
/// material + base tint (for hit-flash / KO gray), and the world direction it
/// attacks toward (for lunge / recoil). [`animate_battle_actors`] slides *this
/// child* — not the actor root — so the flinch doesn't disturb the walk-cycle logic
/// or the grounded shadow.
#[derive(Component)]
pub(crate) struct SpriteQuad {
    id: String,
    mat: Handle<StandardMaterial>,
    base: Color,
    forward: Vec3,
}

/// The enemy the player has singled out — a diamond marker floats over its head in
/// the 3D arena. Set by tapping an enemy sprite ([`battle_click_target`]); while the
/// Target picker is open the picker's cursor drives the marker instead.
#[derive(Resource, Default)]
pub(crate) struct BattleTarget {
    pub(crate) selected: Option<String>,
}

/// A joined ally player's arena edge. Your own party owns the south (placed
/// directly in `sync_battle_actors`); each *other* player's party takes one of these
/// so co-op lineups ring the field instead of stacking.
#[derive(Clone, Copy)]
pub(crate) enum PartyEdge {
    North,
    West,
    East,
}

impl PartyEdge {
    /// World position + world-space facing (toward the centre) for hero `i` of `n`.
    fn slot(self, i: usize, n: usize) -> (Vec3, Vec2) {
        // Fan `n` heroes evenly around the edge's midpoint.
        let s = (i as f32 - (n.max(1) as f32 - 1.0) * 0.5) * 2.4;
        // Enemies knot around CZ; the allied parties ring them from each side.
        const CZ: f32 = -1.4;
        match self {
            PartyEdge::North => (Vec3::new(s, 0.0, -5.8), Vec2::new(0.0, 1.0)),
            PartyEdge::West => (Vec3::new(-4.8, 0.0, CZ + s), Vec2::new(1.0, 0.0)),
            PartyEdge::East => (Vec3::new(4.8, 0.0, CZ + s), Vec2::new(-1.0, 0.0)),
        }
    }
}

/// Spawn one hero billboard at `root` facing `facing` (idle sprite chosen from that
/// heading, camera-relative). `bust` renders only head→torso (back-row heroes, so
/// they stack tight behind the front) and drops the ground shadow.
pub(crate) fn spawn_hero_actor(
    commands: &mut Commands,
    wa: &WorldAssets,
    mats: &mut Assets<StandardMaterial>,
    c: &CombatantView,
    root: Vec3,
    facing: Vec2,
    bust: bool,
) {
    let class = c
        .statuses
        .iter()
        .find_map(|s| s.strip_prefix("class:"))
        .unwrap_or("hunter");
    let frames = match class {
        "psyker" => &wa.psyker,
        _ => &wa.hunter,
    };
    let base_tint = Color::srgb(1.2, 1.18, 1.08);
    let mat = mats.add(hd2d::sprite_material(base_tint, frames.idle[0].clone()));
    let mut cs = CharSprite::new(frames.clone(), mat.clone(), root);
    cs.facing = facing;
    cs.locked = Some(facing); // a battle hero always faces the monsters
    let forward = Vec3::new(facing.x, 0.0, facing.y); // toward the foes
    let quad = if bust { wa.bust_quad.clone() } else { wa.sprite_quad.clone() };
    commands
        .spawn((
            BattleActor { id: c.id.clone() },
            Transform::from_translation(root),
            Visibility::default(),
            cs,
        ))
        .with_children(|p| {
            p.spawn((
                SpriteQuad { id: c.id.clone(), mat: mat.clone(), base: base_tint, forward },
                Mesh3d(quad),
                MeshMaterial3d(mat),
                Transform::from_xyz(0.0, 0.72, 0.0),
                hd2d::Billboard,
                hd2d::HeroBillboard,
                // Player-character sprite: self-illuminates at night (battle stays
                // readable in the dark).
                PlayerGlowSprite,
            ));
            // The hero carries a warm lamp at night (driven by `illuminate_players`).
            // The Hunter's is the big "Predator's Eye" beam that lights the enemy row
            // across the arena; every other class carries only a soft, short-range
            // glow — bright enough to stay readable, small enough not to flicker as
            // the renderer's light clusters fight over a pile of equal lights.
            let is_hunter = class == "hunter";
            let (strength, range, radius) = if is_hunter {
                (140_000.0, 34.0, 0.6) // full, big — reaches the foes
            } else {
                (16_000.0, 8.5, 0.3) // soft, close
            };
            p.spawn((
                BattlePartyLamp { strength },
                PointLight {
                    color: Color::srgb(1.0, 0.88, 0.62),
                    intensity: 0.0,
                    range,
                    radius,
                    shadows_enabled: false,
                    ..default()
                },
                Transform::from_xyz(0.0, 1.6, 0.0),
            ));
            // A bust has no legs to ground, so it skips the contact shadow.
            if !bust {
                p.spawn((
                    Mesh3d(wa.shadow_mesh.clone()),
                    MeshMaterial3d(wa.shadow_mat.clone()),
                    Transform::from_xyz(0.0, 0.02, 0.0)
                        .with_rotation(Quat::from_rotation_x(-std::f32::consts::FRAC_PI_2))
                        .with_scale(Vec3::new(1.0, 0.55, 1.0)),
                ));
            }
        });
}

/// Spawn one enemy billboard at `root`, `h` world-units tall (shrunk when the party
/// surrounds them), with its hidden target reticle.
pub(crate) fn spawn_enemy_actor(
    commands: &mut Commands,
    wa: &WorldAssets,
    mats: &mut Assets<StandardMaterial>,
    c: &CombatantView,
    root: Vec3,
    h: f32,
) {
    // Exactly the billboard the overworld uses for this creature: resolve by the
    // normalized kind (strips any champion affix like "Swift ") so a mob keeps its
    // sprite crossing from the overworld into the fight.
    let tex = creature_sprite(wa, &c.name);
    let base_tint = Color::srgb(1.2, 1.15, 1.1);
    let mat = mats.add(hd2d::sprite_material(base_tint, tex));
    // The diamond marker hovers just above the sprite's head (its tip reaches down
    // toward the head, so keep a small gap above `h`).
    let marker_y = h + 0.45;
    commands
        .spawn((
            BattleActor { id: c.id.clone() },
            Transform::from_translation(root),
            Visibility::default(),
        ))
        .with_children(|p| {
            p.spawn((
                // Enemies strike toward the players (south, +z).
                SpriteQuad {
                    id: c.id.clone(),
                    mat: mat.clone(),
                    base: base_tint,
                    forward: Vec3::new(0.0, 0.0, 1.0),
                },
                Mesh3d(wa.sprite_quad.clone()),
                MeshMaterial3d(mat),
                Transform::from_xyz(0.0, h * 0.5, 0.0).with_scale(Vec3::splat(h / 2.2)),
                hd2d::Billboard,
            ));
            p.spawn((
                Mesh3d(wa.shadow_mesh.clone()),
                MeshMaterial3d(wa.shadow_mat.clone()),
                Transform::from_xyz(0.0, 0.02, 0.0)
                    .with_rotation(Quat::from_rotation_x(-std::f32::consts::FRAC_PI_2))
                    .with_scale(Vec3::new(h * 0.42, h * 0.23, 1.0)),
            ));
            // Target diamond — hidden until this enemy is the picked target.
            p.spawn((
                TargetDiamond { id: c.id.clone(), base_y: marker_y },
                Mesh3d(wa.target_diamond_mesh.clone()),
                MeshMaterial3d(wa.target_diamond_mat.clone()),
                Transform::from_xyz(0.0, marker_y, 0.0),
                Visibility::Hidden,
            ));
        });
}

/// Reconcile the 3D battle arena with `BattleData`. Your party lines up on the near
/// (south) edge facing in, Octopath-style backs; each **other player's** joined
/// party takes its own edge (north/west/east) so co-op lineups ring the field
/// instead of stacking. Enemies knot in the centre and **shrink when surrounded**,
/// reading as outnumbered. The HP/ATB/command UI frames it.
pub(crate) fn sync_battle_actors(
    mut commands: Commands,
    battle: Res<BattleData>,
    wa: Option<Res<WorldAssets>>,
    mut mats: ResMut<Assets<StandardMaterial>>,
    q: Query<(Entity, &BattleActor)>,
) {
    let Some(wa) = wa else { return };
    let mut seen = HashSet::new();
    for (ent, a) in &q {
        if battle.combatants.iter().any(|c| c.id == a.id) {
            seen.insert(a.id.clone());
        } else {
            commands.entity(ent).despawn();
        }
    }
    // Split combatants: my heroes, each ally player's heroes (grouped by owner,
    // first-seen order), and the enemies.
    let mut mine: Vec<&CombatantView> = Vec::new();
    let mut ally_order: Vec<String> = Vec::new();
    let mut allies: HashMap<String, Vec<&CombatantView>> = HashMap::new();
    let mut enemies: Vec<&CombatantView> = Vec::new();
    for c in battle.combatants.iter() {
        if !c.is_player {
            enemies.push(c);
        } else if battle.your_ids.contains(&c.id) {
            mine.push(c);
        } else {
            let owner = c.player_id.clone().unwrap_or_else(|| c.id.clone());
            if !allies.contains_key(&owner) {
                ally_order.push(owner.clone());
            }
            allies.entry(owner).or_default().push(c);
        }
    }
    let surrounded = !ally_order.is_empty();

    // My party fans across the foreground so ALL FOUR read clearly (not two hidden
    // behind two): laid out in party order along a shallow arc facing north, with
    // row:back casters set a little deeper + rendered as head→torso BUSTS so they
    // sit visually behind the front line without their legs cluttering. Shrinking
    // the enemies (below) frees the centre this needs.
    let is_back = |c: &&CombatantView| c.statuses.iter().any(|s| s == "row:back");
    let n = mine.len().max(1) as f32;
    for (i, c) in mine.iter().enumerate() {
        if seen.contains(&c.id) {
            continue;
        }
        let back = is_back(c);
        // Even spread across x; a small depth offset (and bust crop) sets the rows.
        // Enemies are to the north (−z); the camera is to the south (+z). So the
        // FRONT line belongs at the smaller z (nearer the foe, up-screen) and the
        // protected BACK row at the larger z (nearer the camera, in the foreground)
        // — matching the combat semantics (back row is targeted less / takes less).
        // The up-screen row is the one cropped to a bust so it tucks behind cleanly.
        let x = (i as f32 - (n - 1.0) * 0.5) * 2.7;
        let z = if back { 3.7 } else { 2.7 };
        // Full sprites for everyone: the head→torso "bust" crop dropped the legs +
        // shadow, so cropped heroes read as floating torsos ("hovering"). Render
        // the whole body grounded instead.
        spawn_hero_actor(&mut commands, &wa, &mut mats, c, Vec3::new(x, 0.0, z), Vec2::new(0.0, -1.0), false);
    }
    // Allies fill the remaining edges; a rare 4th+ party reuses the north edge.
    let edges = [PartyEdge::North, PartyEdge::West, PartyEdge::East];
    for (gi, owner) in ally_order.iter().enumerate() {
        let edge = edges[gi.min(edges.len() - 1)];
        let heroes = &allies[owner];
        for (i, c) in heroes.iter().enumerate() {
            if seen.contains(&c.id) {
                continue;
            }
            let (root, facing) = edge.slot(i, heroes.len());
            spawn_hero_actor(&mut commands, &wa, &mut mats, c, root, facing, false);
        }
    }
    // Enemies cluster in the centre; a solo fight keeps the classic far-line framing,
    // a surrounded one pulls them in tight and shrinks them. Kept modest so they read
    // as foes without dwarfing the party (they used to tower over the heroes).
    let (h, gap, cz) = if surrounded {
        (1.9, 2.0, -1.4)
    } else {
        (2.5, 3.0, -4.5)
    };
    for (i, c) in enemies.iter().enumerate() {
        if seen.contains(&c.id) {
            continue;
        }
        let x = (i as f32 - (enemies.len().max(1) as f32 - 1.0) * 0.5) * gap;
        spawn_enemy_actor(&mut commands, &wa, &mut mats, c, Vec3::new(x, 0.0, cz), h);
    }
}

/// Which enemy carries the target marker this frame: the Target picker's cursor
/// while aiming an action (so keyboard target-scrolling moves the diamond too),
/// otherwise the sticky tap-selected enemy. `None` if that enemy is gone or dead.
pub(crate) fn highlight_focus(battle: &BattleData, menu: &BattleMenu, target: &BattleTarget) -> Option<String> {
    let living = |id: &str| {
        battle
            .combatants
            .iter()
            .any(|c| c.id == id && !c.is_player && c.hp > 0)
    };
    let id = if menu.level == MenuLevel::Target {
        menu.rows.get(menu.cursor).map(|(_, v)| v.clone())
    } else {
        target.selected.clone()
    };
    id.filter(|id| living(id))
}

/// Float the target marker over the picked enemy: show its diamond, slowly bounce
/// it above the head, and spin it for a gem glint. Every other diamond is hidden, so
/// exactly one enemy is marked as "this is who your order hits."
pub(crate) fn highlight_target(
    time: Res<Time>,
    battle: Res<BattleData>,
    menu: Res<BattleMenu>,
    mut target: ResMut<BattleTarget>,
    mut diamonds: Query<(&TargetDiamond, &mut Transform, &mut Visibility)>,
) {
    // Drop a stale sticky pick (its enemy died / the battle moved on).
    if let Some(sel) = target.selected.clone() {
        if !battle
            .combatants
            .iter()
            .any(|c| c.id == sel && !c.is_player && c.hp > 0)
        {
            target.selected = None;
        }
    }
    let focus = highlight_focus(&battle, &menu, &target);
    let t = time.elapsed_secs();
    // A slow, gentle bob (≈0.4 Hz) and an unhurried spin.
    let bob = 0.22 * (t * 2.4).sin();
    let spin = Quat::from_rotation_y(t * 1.1);

    for (d, mut tf, mut vis) in &mut diamonds {
        let on = focus.as_deref() == Some(d.id.as_str());
        *vis = if on { Visibility::Visible } else { Visibility::Hidden };
        if on {
            tf.translation.y = d.base_y + bob;
            tf.rotation = spin;
        }
    }
}

/// Give combat weight: struck sprites flash white + recoil (with a quick shake),
/// attackers lunge in and back, and a downed combatant grays out. Drives each sprite
/// *child* (leaving the actor root — and thus the walk-cycle logic + shadow — alone).
pub(crate) fn animate_battle_actors(
    battle: Res<BattleData>,
    hitfx: Res<HitFx>,
    mut mats: ResMut<Assets<StandardMaterial>>,
    mut q: Query<(&mut Transform, &SpriteQuad)>,
) {
    for (mut tf, s) in &mut q {
        // KO: gray the sprite, drop any hit motion — reads as "downed".
        if battle.view(&s.id).map(|c| c.hp <= 0).unwrap_or(false) {
            if let Some(m) = mats.get_mut(&s.mat) {
                let c = s.base.to_srgba();
                let lum = 0.3 * c.red + 0.5 * c.green + 0.2 * c.blue;
                m.base_color = Color::srgb(lum * 0.45, lum * 0.45, lum * 0.5);
                m.emissive = LinearRgba::BLACK;
            }
            tf.translation.x = 0.0;
            tf.translation.z = 0.0;
            continue;
        }
        // Freshest damage hit on this sprite, and its own lunge timer.
        let hit_age = hitfx
            .items
            .iter()
            .filter(|h| h.target == s.id && h.text.starts_with('-'))
            .map(|h| h.age)
            .fold(f32::INFINITY, f32::min);
        let lunge_age = hitfx.acts.get(&s.id).copied().unwrap_or(f32::INFINITY);

        // Recoil: a knockback that eases home over the TTL.
        let recoil = if hit_age < HIT_RECOIL_TTL {
            (1.0 - hit_age / HIT_RECOIL_TTL) * 0.35
        } else {
            0.0
        };
        // Lunge: step toward the foe and back (half-sine, peaks mid-window).
        let lunge = if lunge_age < ATTACK_LUNGE_TTL {
            (std::f32::consts::PI * lunge_age / ATTACK_LUNGE_TTL).sin() * 0.6
        } else {
            0.0
        };
        // A brief lateral shake right at the moment of impact.
        let shake = if hit_age < HIT_WHITE_TTL {
            (hit_age * 90.0).sin() * (1.0 - hit_age / HIT_WHITE_TTL) * 0.12
        } else {
            0.0
        };
        let perp = Vec3::new(s.forward.z, 0.0, -s.forward.x);
        let off = s.forward * (lunge - recoil) + perp * shake;
        tf.translation.x = off.x;
        tf.translation.z = off.z;

        // Hunter "rage": as banked Adrenaline climbs toward max, redden the sprite
        // and add a faint hot glow so a Hunter *looks* angrier the more it's built.
        // Only Hunters carry adrenaline_max > 0, so every other class stays neutral.
        let rage = battle
            .view(&s.id)
            .map(|c| {
                let max = status_num(&c.statuses, "adrenaline_max:");
                if max > 0 {
                    status_num(&c.statuses, "adrenaline:") as f32 / max as f32
                } else {
                    0.0
                }
            })
            .unwrap_or(0.0)
            .clamp(0.0, 1.0);
        // White impact flash on the instant of a hit (brighter than base → blooms);
        // otherwise the base tint, warmed toward angry red by the rage fraction.
        if let Some(m) = mats.get_mut(&s.mat) {
            if hit_age < HIT_WHITE_TTL {
                m.base_color =
                    lerp_color(s.base, Color::srgb(2.6, 2.6, 2.6), 1.0 - hit_age / HIT_WHITE_TTL);
                m.emissive = LinearRgba::BLACK;
            } else {
                m.base_color = lerp_color(s.base, Color::srgb(1.9, 0.5, 0.35), rage * 0.55);
                m.emissive = LinearRgba::rgb(0.5 * rage, 0.04 * rage, 0.0);
            }
        }
    }
}

/// Tap/click an enemy sprite in the arena to target it — the JRPG "point at the
/// monster" instead of scrolling a text list. While the Target picker is open the
/// click fulfils the pending action's target; otherwise it marks the enemy (which
/// starts it shimmering) and, for a hero with a basic Attack, swings at it directly.
/// The hit-test projects each enemy sprite's feet→head to the screen (like
/// [`overworld_click_menu`]), so it matches the billboard you see under any camera.
#[allow(clippy::too_many_arguments)]
pub(crate) fn battle_click_target(
    mouse: Res<ButtonInput<MouseButton>>,
    touches: Res<Touches>,
    windows: Query<&Window>,
    cam_q: Query<(&Camera, &GlobalTransform), With<Camera3d>>,
    ui_hit: Query<&Interaction, With<Button>>,
    actors: Query<(&BattleActor, &GlobalTransform)>,
    mut menu: ResMut<BattleMenu>,
    mut battle: ResMut<BattleData>,
    mut target: ResMut<BattleTarget>,
    mut press: Local<Option<Vec2>>,
) {
    // Gather a click point: a no-drag mouse click (drags orbit the camera) or a tap.
    let mut point = None;
    if let Some(w) = windows.iter().next() {
        if mouse.just_pressed(MouseButton::Left) {
            *press = w.cursor_position();
        }
        if mouse.just_released(MouseButton::Left) {
            if let (Some(p0), Some(p1)) = (*press, w.cursor_position()) {
                if p0.distance(p1) < 6.0 {
                    point = Some(p1);
                }
            }
            *press = None;
        }
    }
    for touch in touches.iter_just_pressed() {
        point = Some(touch.position());
    }
    let Some(p) = point else { return };
    if ui_hit.iter().any(|i| *i != Interaction::None) {
        return; // a UI button (the command window) — not the arena
    }
    let Some((cam, cam_tf)) = cam_q.iter().next() else { return };

    // Nearest living enemy sprite under the click.
    let mut best: Option<(f32, String)> = None;
    for (a, gt) in &actors {
        match battle.view(&a.id) {
            Some(c) if !c.is_player && c.hp > 0 => {}
            _ => continue,
        }
        let feet = gt.translation();
        let head = feet + Vec3::Y * 3.4; // enemy sprite height
        if let (Ok(fs), Ok(hs)) = (
            cam.world_to_viewport(cam_tf, feet),
            cam.world_to_viewport(cam_tf, head),
        ) {
            let radius = ((hs - fs).length() * 0.5).max(44.0);
            let d = seg_point_dist(p, fs, hs);
            if d < radius && best.as_ref().is_none_or(|(bd, _)| d < *bd) {
                best = Some((d, a.id.clone()));
            }
        }
    }
    let Some((_, eid)) = best else { return };

    // Aiming a pending action → this tap is the target choice.
    if menu.level == MenuLevel::Target {
        if let Some(idx) = menu.rows.iter().position(|(_, v)| *v == eid) {
            let class = battle.active_class();
            select_entry(idx, &mut menu, &mut battle, &class);
        }
        return;
    }
    // Otherwise mark it (start it shimmering) and, for a martial hero, attack it.
    target.selected = Some(eid.clone());
    if battle.active_class() != "psyker" {
        if let Some(active) = battle.active.clone() {
            queue_order(&mut battle, &active, QueuedKind::Attack, Some(eid), &mut menu);
        }
    }
}

/// Frame the HD-2D battle arena: a fixed 3/4 view of the two rows, with the live
/// `Look` post stack. (Overworld `hd2d_follow` doesn't run here.)
#[allow(clippy::type_complexity)]
/// Battle-camera zoom the player can nudge on top of the automatic co-op fit — 1.0
/// is the auto-framed distance; higher pulls back. Reset each fight (`enter_battle`).
#[derive(Resource)]
pub(crate) struct BattleCam {
    zoom: f32,
}
impl Default for BattleCam {
    fn default() -> Self {
        BattleCam { zoom: 1.0 }
    }
}

/// How far back the battle camera sits to frame everyone: 1.0 for a solo fight, more
/// as extra co-op parties join (each lines up on its own edge, so the arena only fits
/// from farther out). Multiplied by the player's manual `BattleCam::zoom`.
pub(crate) fn battle_fit(battle: &BattleData) -> f32 {
    let mut owners: Vec<&str> = battle
        .combatants
        .iter()
        .filter(|c| c.is_player)
        .filter_map(|c| c.player_id.as_deref())
        .collect();
    owners.sort_unstable();
    owners.dedup();
    let parties = owners.len().max(1) as f32;
    // +32% reach per extra party, capped so a full raid still frames tightly enough
    // to read. A lone party keeps the original framing (1.0).
    (1.0 + 0.32 * (parties - 1.0)).min(2.3)
}

/// Mouse-wheel / pinch zoom for the battle camera. It nudges `BattleCam::zoom` on top
/// of the automatic co-op fit, so the framing is auto but adjustable.
pub(crate) fn battle_zoom_input(
    mut bcam: ResMut<BattleCam>,
    mut wheel: EventReader<MouseWheel>,
    touches: Res<Touches>,
    mut pinch: Local<Option<f32>>,
) {
    for e in wheel.read() {
        bcam.zoom = (bcam.zoom - e.y * 0.06).clamp(0.6, 2.6);
    }
    // Two-finger pinch mirrors the overworld's touch zoom.
    let ts: Vec<_> = touches.iter().collect();
    if ts.len() == 2 {
        let d = ts[0].position().distance(ts[1].position());
        if let Some(prev) = *pinch {
            bcam.zoom = (bcam.zoom - (d - prev) * 0.003).clamp(0.6, 2.6);
        }
        *pinch = Some(d);
    } else {
        *pinch = None;
    }
}

pub(crate) fn battle_camera(
    look: Res<hd2d::Look>,
    battle: Res<BattleData>,
    bcam: Res<BattleCam>,
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
    // Auto-fit for co-op, times the player's manual nudge.
    let dist = battle_fit(&battle) * bcam.zoom;
    if let Ok((mut t, mut proj, bloom, dof, fog)) = cam_q.single_mut() {
        // Scale the base offset outward to pull back (and up) proportionally, so more
        // of the surround layout fits without changing the viewing angle. The solo
        // base sits a touch closer than before so a lone party fills the frame
        // (there was a lot of empty arena); the auto-fit reclaims the distance for
        // co-op.
        *t = Transform::from_translation(Vec3::new(0.0, 8.6, 11.2) * dist)
            .looking_at(Vec3::new(0.0, 0.9, -1.6), Vec3::Y);
        // Battle gets its own mood: a punchier bloom so hits/markers glow, and a
        // tighter fog that closes the arena in (the walkable field beyond hazes off)
        // — without disturbing the shared overworld look. The fog distances scale
        // with the zoom so a pulled-back raid camera doesn't fog off its own party.
        let mut blook = look.clone();
        blook.bloom = look.bloom + 0.12;
        blook.fog_on = true;
        blook.fog_start = 22.0 * dist;
        blook.fog_end = 58.0 * dist;
        hd2d::apply_post(
            &blook,
            &mut proj,
            bloom.map(|b| b.into_inner()),
            dof.map(|d| d.into_inner()),
            fog.map(|f| f.into_inner()),
        );
    }
    // Sun owned by `apply_sky` (day/night cycle).
}

/// Queue an order (with its chosen target) for `hero`, then hand focus to the next
/// hero that still needs one — preferring a hero whose ATB is already full
/// ([`pick_active`]). The order fires the instant that hero is ready
/// ([`auto_fire_queued`]).
pub(crate) fn queue_order(
    battle: &mut BattleData,
    hero: &str,
    kind: QueuedKind,
    target: Option<String>,
    menu: &mut BattleMenu,
) {
    battle.queued.insert(hero.to_string(), Order { kind, target });
    battle.active = pick_active(battle).or_else(|| Some(hero.to_string()));
    reset_menu(menu);
}

/// Begin an order for `hero`: self-cast orders queue immediately; aimed orders open the
/// Target picker (auto-picking when only one valid target exists).
pub(crate) fn begin_order(battle: &mut BattleData, menu: &mut BattleMenu, hero: &str, kind: QueuedKind) {
    match order_side(kind) {
        None => queue_order(battle, hero, kind, None, menu),
        Some(side) => {
            let targets = valid_targets(battle, side);
            match targets.len() {
                0 => reset_menu(menu), // nothing valid to hit — abandon the choice
                1 => queue_order(battle, hero, kind, Some(targets[0].1.clone()), menu),
                _ => {
                    menu.pending = Some((hero.to_string(), kind));
                    menu.rows = targets;
                    open_page(menu, MenuLevel::Target);
                }
            }
        }
    }
}

/// Living combatants on `side`, as `(label, id)` rows for the Target picker. Allies are
/// every living player combatant — including co-op heroes who joined the battle (they
/// live in `combatants`, not `your_ids`).
pub(crate) fn valid_targets(battle: &BattleData, side: Side) -> Vec<(String, String)> {
    battle
        .combatants
        .iter()
        .filter(|c| c.hp > 0 && (side == Side::Ally) == c.is_player)
        .map(|c| {
            let name = if c.is_player { battle.hero_label(&c.id) } else { c.name.clone() };
            (format!("{}  {}/{}", name, c.hp, c.max_hp), c.id.clone())
        })
        .collect()
}

/// A default target for an order lacking an explicit pick (autoplay, or a queued target
/// that died before firing): first living enemy for offensive orders, most-wounded
/// living ally for supportive ones. `None` when nothing valid remains.
pub(crate) fn default_target(battle: &BattleData, kind: QueuedKind) -> Option<String> {
    match order_side(kind) {
        Some(Side::Enemy) => battle
            .combatants
            .iter()
            .find(|c| !c.is_player && c.hp > 0)
            .map(|c| c.id.clone()),
        Some(Side::Ally) => battle
            .combatants
            .iter()
            .filter(|c| c.is_player && c.hp > 0)
            .min_by(|a, b| {
                let fa = a.hp as f32 / a.max_hp.max(1) as f32;
                let fb = b.hp as f32 / b.max_hp.max(1) as f32;
                fa.total_cmp(&fb)
            })
            .map(|c| c.id.clone()),
        None => None,
    }
}

/// The hero the command panel should focus: prefer one that's ready and still
/// un-ordered, else any un-ordered live hero. Returns `None` when every living
/// hero already has a locked order — that's the signal for the command panel to
/// hide (nothing left to command until someone acts).
pub(crate) fn pick_active(battle: &BattleData) -> Option<String> {
    let alive: Vec<&String> = battle.your_ids.iter().filter(|h| battle.alive(h)).collect();
    alive
        .iter()
        .find(|h| battle.ready.contains(**h) && !battle.queued.contains_key(**h))
        .or_else(|| alive.iter().find(|h| !battle.queued.contains_key(**h)))
        .map(|h| (*h).clone())
}

/// The next living, un-ordered hero after the current `active` (wrapping) — the
/// target of TAB / clicking another hero's box, so you can pick WHICH ready hero to
/// command. `None` when no other hero can be commanded. A hero that already locked
/// an order is skipped (its action can't be changed until it fires).
pub(crate) fn next_commandable(battle: &BattleData) -> Option<String> {
    let ids = &battle.your_ids;
    let n = ids.len();
    if n == 0 {
        return None;
    }
    let start = battle
        .active
        .as_ref()
        .and_then(|a| ids.iter().position(|h| h == a))
        .unwrap_or(0);
    (1..=n)
        .map(|step| &ids[(start + step) % n])
        .find(|h| battle.alive(h) && !battle.queued.contains_key(*h))
        .cloned()
}

/// Send a hero's order to the server, aimed at `target` (the combatant the player
/// chose; already validated/retargeted by [`auto_fire_queued`]).
pub(crate) fn fire_order(net: &Net, battle_id: &str, actor: &str, kind: QueuedKind, target: Option<&str>) {
    let cmd = match kind {
        QueuedKind::Attack => target.map(|t| ClientCmd::Attack {
            battle_id: battle_id.to_string(),
            actor: actor.to_string(),
            target: t.to_string(),
        }),
        QueuedKind::Skill(sk) => target.map(|t| ClientCmd::Skill {
            battle_id: battle_id.to_string(),
            actor: actor.to_string(),
            target: t.to_string(),
            skill_kind: sk.to_string(),
        }),
        QueuedKind::Defend => Some(ClientCmd::Defend {
            battle_id: battle_id.to_string(),
            actor: actor.to_string(),
        }),
        // Items heal the chosen ally (server falls back to the actor for an empty id).
        QueuedKind::Item(it) => Some(ClientCmd::Item {
            battle_id: battle_id.to_string(),
            actor: actor.to_string(),
            item_id: it.to_string(),
            target: target.unwrap_or(actor).to_string(),
        }),
        // Psyker Focus ops ride the Skill action with a `verb:kind` skill_kind; the
        // aimed enemy (for offensive Foci) travels as the target.
        QueuedKind::Focus(verb, kind) => Some(ClientCmd::Skill {
            battle_id: battle_id.to_string(),
            actor: actor.to_string(),
            target: target.unwrap_or("").to_string(),
            skill_kind: format!("{verb}:{kind}"),
        }),
        QueuedKind::Hold => Some(ClientCmd::Skill {
            battle_id: battle_id.to_string(),
            actor: actor.to_string(),
            target: target.unwrap_or("").to_string(),
            skill_kind: "hold".to_string(),
        }),
        QueuedKind::Flee => Some(ClientCmd::Flee {
            battle_id: battle_id.to_string(),
            actor: actor.to_string(),
        }),
    };
    if let Some(cmd) = cmd {
        net.send(cmd);
    }
}

/// Keep `active` on a live, controllable hero and auto-focus the ready one: re-pick
/// whenever the active hero is gone or already has a queued order, so focus follows
/// the ATB. Frozen while the Target picker is open (the pending actor owns the turn).
pub(crate) fn validate_active(mut battle: ResMut<BattleData>, mut menu: ResMut<BattleMenu>) {
    if menu.level == MenuLevel::Target {
        return;
    }
    let prev = battle.active.clone();
    let needs_repick = match &battle.active {
        Some(a) => {
            !(battle.your_ids.contains(a) && battle.alive(a)) || battle.queued.contains_key(a)
        }
        None => true,
    };
    if needs_repick {
        battle.active = pick_active(&battle);
    }
    // If focus jumped to a different hero (or vanished), drop any half-open sub-page
    // so the panel shows the new hero's root instead of a stale one.
    if battle.active != prev && menu.level != MenuLevel::Root {
        reset_menu(&mut menu);
    }
}

/// Fire every hero whose gauge is full and who has a queued order, at its chosen
/// target — retargeting to a sensible default if that target died while the gauge filled.
pub(crate) fn auto_fire_queued(net: NonSend<NetRes>, mut battle: ResMut<BattleData>) {
    let battle_id = battle.battle_id.clone();
    let ready_orders: Vec<(String, Order)> = battle
        .your_ids
        .iter()
        .filter(|h| battle.ready.contains(*h))
        .filter_map(|h| battle.queued.get(h).map(|o| (h.clone(), o.clone())))
        .collect();
    for (hero, order) in ready_orders {
        let target = order
            .target
            .filter(|t| battle.alive(t))
            .or_else(|| default_target(&battle, order.kind));
        fire_order(&net.0, &battle_id, &hero, order.kind, target.as_deref());
        battle.ready.remove(&hero);
        battle.queued.remove(&hero);
    }
}

/// The `&'static str` manifestation kind matching a dynamic `kind` string (from a
/// combatant's parsed foci), or `None` if it isn't a known manifestation.
pub(crate) fn manifest_static(kind: &str) -> Option<&'static str> {
    MANIFESTS.iter().find(|(k, _, _)| *k == kind).map(|(k, _, _)| *k)
}

/// Cast vs reinforce for a Psyker picking `kind`: reinforce if that manifestation is
/// already active on the hero, else cast. Mirrors the server's slot logic so the
/// unified menu "just reinforces" a live Focus.
pub(crate) fn manifest_verb(battle: &BattleData, hero: &str, kind: &str) -> &'static str {
    let active = battle
        .view(hero)
        .map(|v| parse_foci(&v.statuses).1)
        .unwrap_or_default();
    if active.iter().any(|(k, _)| k == kind) {
        "reinforce"
    } else {
        "cast"
    }
}

/// Act on the command row at `index`. Root/list rows come from [`menu_entries`]; the
/// dynamic Target/Revoke pages index into [`BattleMenu::rows`] (with a trailing Back).
/// Order-producing rows route through [`begin_order`], which opens the Target picker
/// when the action needs aiming.
pub(crate) fn select_entry(index: usize, menu: &mut BattleMenu, battle: &mut BattleData, class: &str) {
    let active = match battle.active.clone() {
        Some(a) => a,
        None => return,
    };

    // Dynamic pages: `menu.rows` then a trailing Back row.
    if matches!(menu.level, MenuLevel::Target | MenuLevel::Revoke) {
        let Some((_, value)) = menu.rows.get(index).cloned() else {
            reset_menu(menu); // the Back row (or out of range)
            return;
        };
        match menu.level {
            MenuLevel::Target => match menu.pending.clone() {
                Some((actor, kind)) => queue_order(battle, &actor, kind, Some(value), menu),
                None => reset_menu(menu),
            },
            MenuLevel::Revoke => match manifest_static(&value) {
                Some(kind) => queue_order(battle, &active, QueuedKind::Focus("revoke", kind), None, menu),
                None => reset_menu(menu),
            },
            _ => unreachable!(),
        }
        return;
    }

    let hero_level = battle.view(&active).map(|c| c.level).unwrap_or(1);
    let entries = menu_entries(menu.level, class, hero_level);
    let Some(entry) = entries.get(index) else {
        return;
    };
    match entry.action {
        EntryAction::Attack => begin_order(battle, menu, &active, QueuedKind::Attack),
        EntryAction::Defend => begin_order(battle, menu, &active, QueuedKind::Defend),
        EntryAction::OpenSkills => open_page(menu, MenuLevel::Skills),
        EntryAction::OpenItems => open_page(menu, MenuLevel::Items),
        EntryAction::Skill(kind) => begin_order(battle, menu, &active, QueuedKind::Skill(kind)),
        EntryAction::Item(id) => begin_order(battle, menu, &active, QueuedKind::Item(id)),
        // Psyker: Focus opens the manifestation list; Revoke lists the live Foci.
        EntryAction::OpenManifest => open_page(menu, MenuLevel::Manifest),
        EntryAction::OpenRevoke => open_revoke_page(menu, battle, &active),
        // Cast, or reinforce if already active; begin_order aims offensive ones.
        EntryAction::Manifest(kind) => {
            let verb = manifest_verb(battle, &active, kind);
            begin_order(battle, menu, &active, QueuedKind::Focus(verb, kind));
        }
        EntryAction::Hold => begin_order(battle, menu, &active, QueuedKind::Hold),
        EntryAction::Flee => begin_order(battle, menu, &active, QueuedKind::Flee),
        EntryAction::Back => reset_menu(menu),
    }
}

/// Build the Revoke page rows from the hero's live Foci and open it (staying at root
/// if there is nothing to revoke).
pub(crate) fn open_revoke_page(menu: &mut BattleMenu, battle: &BattleData, hero: &str) {
    let foci = battle
        .view(hero)
        .map(|v| parse_foci(&v.statuses).1)
        .unwrap_or_default();
    menu.rows = foci
        .iter()
        .filter_map(|(kind, stacks)| {
            MANIFESTS
                .iter()
                .find(|(k, _, _)| *k == kind.as_str())
                .map(|(k, name, _)| (format!("{name}  x{stacks}"), (*k).to_string()))
        })
        .collect();
    if menu.rows.is_empty() {
        reset_menu(menu);
    } else {
        open_page(menu, MenuLevel::Revoke);
    }
}

/// Switch the command window to a sub-page.
pub(crate) fn open_page(menu: &mut BattleMenu, level: MenuLevel) {
    menu.level = level;
    menu.cursor = 0;
    menu.dirty = true;
}

/// Number of selectable rows on the current page. Static pages come from
/// [`menu_entries`]; the dynamic Target/Revoke pages are `rows` plus a Back row.
pub(crate) fn page_len(menu: &BattleMenu, class: &str, hero_level: i32) -> usize {
    match menu.level {
        MenuLevel::Target | MenuLevel::Revoke => menu.rows.len() + 1,
        level => menu_entries(level, class, hero_level).len(),
    }
}

/// Keyboard control for the command panel. Orders are *queued* for the active hero
/// and fire when its ATB fills. At the martial root the ARROWS are the d-pad —
/// ↑ Skill · ← Item · → Defend · ↓ Flee — and ENTER/SPACE/A = Attack (the centre).
/// TAB (or clicking another ready hero's box) switches which hero you're commanding;
/// 1-4 jump straight to a hero. In a sub-page ↑/↓ move the highlight, ENTER selects,
/// ESC backs out. A Psyker's root is a short list, navigated like a sub-page.
/// Autoplay queues each hero's class default.
pub(crate) fn menu_keyboard(
    keys: Res<ButtonInput<KeyCode>>,
    autoplay: Res<Autoplay>,
    mut menu: ResMut<BattleMenu>,
    mut battle: ResMut<BattleData>,
) {
    // The command menu keys off the *active hero's* class — a mixed party is
    // commanded hero by hero.
    let class = battle.active_class();
    if autoplay.0 {
        let idle: Vec<String> = battle
            .your_ids
            .iter()
            .filter(|h| battle.alive(h) && !battle.queued.contains_key(*h))
            .cloned()
            .collect();
        for h in idle {
            // Each hero autoplays by its own class: Psyker channels Foci, Resonant
            // mends the party, everyone else swings — each at a sensible default target.
            let hc = battle.view(&h).map(hero_class).unwrap_or_else(|| "hunter".into());
            let kind = match hc.as_str() {
                "psyker" => battle.view(&h).map(psyker_autoplay_op).unwrap_or(QueuedKind::Hold),
                "resonant" => resonant_autoplay_op(&battle),
                "shifter" => battle.view(&h).map(shifter_autoplay_op).unwrap_or(QueuedKind::Attack),
                "hunter" => battle.view(&h).map(hunter_autoplay_op).unwrap_or(QueuedKind::Attack),
                "iron_hull" => battle.view(&h).map(ironhull_autoplay_op).unwrap_or(QueuedKind::Attack),
                _ => QueuedKind::Attack,
            };
            let target = default_target(&battle, kind);
            battle.queued.insert(h, Order { kind, target });
        }
        return;
    }

    // TAB switches which un-ordered, living hero the panel commands (skipping any
    // that already locked an order — those can't be changed until they act). Works
    // from anywhere and drops back to the root page.
    if keys.just_pressed(KeyCode::Tab) {
        if let Some(next) = next_commandable(&battle) {
            battle.active = Some(next);
            reset_menu(&mut menu);
        }
        return;
    }

    // Nothing to command right now (every hero committed, or none controllable):
    // the panel is hidden, so swallow the keys.
    if battle.active.is_none() {
        return;
    }

    // ESC / Backspace backs out of a sub-page.
    if (keys.just_pressed(KeyCode::Escape) || keys.just_pressed(KeyCode::Backspace))
        && menu.level != MenuLevel::Root
    {
        reset_menu(&mut menu);
        return;
    }

    let digits = [KeyCode::Digit1, KeyCode::Digit2, KeyCode::Digit3, KeyCode::Digit4];

    // Root of a MARTIAL hero: the arrows ARE the actions (the d-pad), not a cursor.
    // (Indices match `menu_entries`' Root order: 0 Attack, 1 Defend, 2 Item, 3 Skill,
    // 4 Flee.)
    if menu.level == MenuLevel::Root && class != "psyker" {
        // Jump straight to a hero by number (only a commandable one).
        for (i, key) in digits.iter().enumerate() {
            if i < battle.your_ids.len() && keys.just_pressed(*key) {
                let h = battle.your_ids[i].clone();
                if battle.alive(&h) && !battle.queued.contains_key(&h) {
                    battle.active = Some(h);
                    reset_menu(&mut menu);
                }
                return;
            }
        }
        let pick = if keys.just_pressed(KeyCode::ArrowUp) || keys.just_pressed(KeyCode::KeyS) {
            Some(3) // Skill
        } else if keys.just_pressed(KeyCode::ArrowLeft) || keys.just_pressed(KeyCode::KeyI) {
            Some(2) // Item
        } else if keys.just_pressed(KeyCode::ArrowRight) || keys.just_pressed(KeyCode::KeyD) {
            Some(1) // Defend
        } else if keys.just_pressed(KeyCode::ArrowDown) || keys.just_pressed(KeyCode::KeyF) {
            Some(4) // Flee
        } else if keys.just_pressed(KeyCode::Enter)
            || keys.just_pressed(KeyCode::Space)
            || keys.just_pressed(KeyCode::KeyA)
        {
            Some(0) // Attack (the centre / default)
        } else {
            None
        };
        if let Some(i) = pick {
            menu.cursor = i;
            select_entry(i, &mut menu, &mut battle, &class);
        }
        return;
    }

    // Sub-page (or a Psyker's list root): ↑/↓ move the highlight, digits jump to a
    // row, ENTER/SPACE selects.
    let hero_level = battle.active_level();
    let n = page_len(&menu, &class, hero_level).max(1);
    if keys.just_pressed(KeyCode::ArrowDown) {
        menu.cursor = (menu.cursor + 1) % n;
    }
    if keys.just_pressed(KeyCode::ArrowUp) {
        menu.cursor = (menu.cursor + n - 1) % n;
    }
    for (i, key) in digits.iter().enumerate() {
        if i < n && keys.just_pressed(*key) {
            menu.cursor = i;
            select_entry(i, &mut menu, &mut battle, &class);
            return;
        }
    }
    if keys.just_pressed(KeyCode::Enter) || keys.just_pressed(KeyCode::Space) {
        select_entry(menu.cursor, &mut menu, &mut battle, &class);
    }
}

/// Mouse/touch: pressing a command row queues it for the active hero.
pub(crate) fn menu_click(
    mut menu: ResMut<BattleMenu>,
    mut battle: ResMut<BattleData>,
    rows: Query<(&Interaction, &MenuRow), Changed<Interaction>>,
) {
    let mut pressed = None;
    for (interaction, row) in &rows {
        if *interaction == Interaction::Pressed {
            pressed = Some(row.index);
        }
    }
    if let Some(index) = pressed {
        menu.cursor = index;
        let class = battle.active_class();
        select_entry(index, &mut menu, &mut battle, &class);
    }
}

/// Mouse/touch: tapping a party HUD cell makes that hero the one being commanded —
/// the touch-friendly counterpart to TAB. Only a controllable, un-ordered hero can
/// be picked (you can't re-open a hero that already locked its action), and only
/// from the root page (so it never hijacks a target pick). Switching drops back to
/// the root page for the newly focused hero.
pub(crate) fn party_select_click(
    mut menu: ResMut<BattleMenu>,
    mut battle: ResMut<BattleData>,
    cells: Query<(&Interaction, &PartyCellButton), Changed<Interaction>>,
) {
    if menu.level != MenuLevel::Root {
        return;
    }
    let mut pick = None;
    for (interaction, cell) in &cells {
        if *interaction == Interaction::Pressed {
            pick = Some(cell.id.clone());
        }
    }
    if let Some(id) = pick {
        if battle.your_ids.contains(&id) && battle.alive(&id) && !battle.queued.contains_key(&id) {
            battle.active = Some(id);
            reset_menu(&mut menu);
        }
    }
}

/// One command tile in the cross, tagged with its menu-entry index.
// Frosted-glass battle-HUD palette (Dragon Quest HD-2D remake vibe): translucent
// fills + hairline light edges so panels read as glass floating over the 3D arena
// instead of opaque bordered boxes. Bevy UI has no true backdrop blur, so the low
// alpha over the busy scene carries the effect.
/// Default glass panel fill.
pub(crate) fn glass_fill() -> Color {
    Color::srgba(0.06, 0.09, 0.17, 0.5)
}
/// Hairline light edge for a glass panel.
pub(crate) fn glass_edge() -> Color {
    Color::srgba(0.78, 0.86, 1.0, 0.32)
}
/// Translucent gold wash for the active/selected element.
pub(crate) fn glass_active() -> Color {
    Color::srgba(0.85, 0.68, 0.28, 0.5)
}
/// Bright edge for the active/selected element.
pub(crate) fn glass_active_edge() -> Color {
    Color::srgba(1.0, 0.9, 0.5, 0.8)
}

pub(crate) fn cmd_tile(
    parent: &mut ChildSpawnerCommands,
    index: usize,
    label: &str,
    w: f32,
    border: Color,
    text: Color,
) {
    parent
        .spawn((
            Button,
            MenuRow { index },
            Node {
                width: Val::Px(w),
                height: Val::Px(46.0),
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                border: UiRect::all(Val::Px(1.0)),
                ..default()
            },
            BorderColor(border),
            BackgroundColor(glass_fill()),
            BorderRadius::all(Val::Px(8.0)),
        ))
        .with_children(|t| {
            t.spawn((
                Text::new(label.to_string()),
                TextFont {
                    font_size: 15.0,
                    ..default()
                },
                TextColor(text),
            ));
        });
}

/// Rebuild the floating command panel. It's shown ONLY while a hero needs orders
/// (`battle.active` is set) — once every ready hero has locked an action the panel
/// vanishes, so it never just sits on screen or covers the party boxes below. A
/// martial hero's root is a d-pad cross (Attack centre · Skill ↑ · Item ← · Defend →
/// · Flee ↓); a Psyker's root and every Skill/Item/Target sub-page is a compact list.
/// The panel is rebuilt only when its signature (shown? · which hero · which page)
/// changes, so button `Interaction` survives across frames within one state.
pub(crate) fn rebuild_command_menu(
    mut commands: Commands,
    battle: Res<BattleData>,
    mut menu: ResMut<BattleMenu>,
    existing: Query<Entity, With<CommandWindow>>,
) {
    let show = battle.active.is_some();
    let level = menu.level;
    let active_id = battle.active.clone().unwrap_or_default();
    // Include the dynamic row count so re-opening a Target page (same level) rebuilds.
    let sig = format!("{show}|{active_id}|{level:?}|{}", menu.rows.len());
    if !menu.dirty && sig == menu.sig {
        return;
    }
    menu.dirty = false;
    menu.sig = sig;
    for e in &existing {
        commands.entity(e).despawn();
    }
    // Hidden: nothing to command right now. Leave the screen clear.
    if !show {
        return;
    }
    let class = battle.active_class();
    let is_psyker = class == "psyker";
    let hero_level = battle.active_level();
    let commanding = battle.hero_label(&active_id);
    let can_switch = next_commandable(&battle).is_some();

    // Palette for the d-pad tiles: neutral for Item/Defend/Skill, gold for the
    // primary Attack, red for Flee — so the two "big" choices read at a glance.
    let neutral_edge = glass_edge();
    let neutral_text = Color::srgb(0.92, 0.94, 1.0);
    let gold = Color::srgb(1.0, 0.85, 0.45);
    let red = Color::srgb(1.0, 0.55, 0.5);

    // Row labels for the list renderer: the dynamic Target/Revoke pages draw from
    // `menu.rows` (+ a Back row); every other page comes from `menu_entries`.
    let labels: Vec<String> = match level {
        MenuLevel::Target | MenuLevel::Revoke => menu
            .rows
            .iter()
            .map(|(l, _)| l.clone())
            .chain(std::iter::once("Back".to_string()))
            .collect(),
        _ => menu_entries(level, &class, hero_level)
            .into_iter()
            .map(|e| e.label.to_string())
            .collect(),
    };

    // A single glass panel (header + body) centred above the party HUD row (which
    // sits at bottom:10, height 92 → top ≈ 102). Anchoring the panel at bottom:112
    // keeps a clear gap so the controls never overlap the hero boxes.
    commands
        .spawn((
            CommandWindow,
            Node {
                position_type: PositionType::Absolute,
                left: Val::Px(0.0),
                right: Val::Px(0.0),
                bottom: Val::Px(112.0),
                flex_direction: FlexDirection::Row,
                justify_content: JustifyContent::Center,
                ..default()
            },
        ))
        .with_children(|w| {
            w.spawn((
                Node {
                    flex_direction: FlexDirection::Column,
                    align_items: AlignItems::Center,
                    row_gap: Val::Px(6.0),
                    padding: UiRect::axes(Val::Px(14.0), Val::Px(10.0)),
                    border: UiRect::all(Val::Px(1.0)),
                    ..default()
                },
                BorderColor(glass_edge()),
                BackgroundColor(glass_fill()),
                BorderRadius::all(Val::Px(12.0)),
            ))
            .with_children(|panel| {
                // Header: who you're commanding (+ a Tab hint when another ready hero
                // is waiting). Keeps the multi-ready case legible.
                panel
                    .spawn(Node {
                        flex_direction: FlexDirection::Row,
                        align_items: AlignItems::Center,
                        column_gap: Val::Px(8.0),
                        margin: UiRect::bottom(Val::Px(2.0)),
                        ..default()
                    })
                    .with_children(|h| {
                        h.spawn((
                            // Nerd Font (mdi) sword glyph, not a bare unicode symbol
                            // (the default face would tofu that) — see UiFont.
                            Text::new(format!("\u{f04e5} {commanding}")), // sword
                            TextFont { font_size: 15.0, ..default() },
                            TextColor(gold),
                        ));
                        if can_switch {
                            h.spawn((
                                Text::new("[Tab] switch".to_string()),
                                TextFont { font_size: 12.0, ..default() },
                                TextColor(Color::srgb(0.6, 0.66, 0.8)),
                            ));
                        }
                    });

                if level == MenuLevel::Root && !is_psyker {
                    // The d-pad cross. Indices match `menu_entries` Root order.
                    panel
                        .spawn(Node {
                            flex_direction: FlexDirection::Column,
                            align_items: AlignItems::Center,
                            row_gap: Val::Px(6.0),
                            ..default()
                        })
                        .with_children(|cross| {
                            cross
                                .spawn(Node {
                                    flex_direction: FlexDirection::Row,
                                    ..default()
                                })
                                .with_children(|r| cmd_tile(r, 3, "\u{f0068} Skill", 92.0, neutral_edge, neutral_text));
                            cross
                                .spawn(Node {
                                    flex_direction: FlexDirection::Row,
                                    column_gap: Val::Px(6.0),
                                    ..default()
                                })
                                .with_children(|r| {
                                    // mdi glyphs (see UiFont): flask=Item, sword=Attack,
                                    // shield=Defend, run-fast=Flee, auto-fix=Skill.
                                    cmd_tile(r, 2, "\u{f0093} Item", 92.0, neutral_edge, neutral_text);
                                    cmd_tile(r, 0, "\u{f04e5} Attack", 92.0, gold, gold);
                                    cmd_tile(r, 1, "\u{f132} Defend", 92.0, neutral_edge, neutral_text);
                                });
                            cross
                                .spawn(Node {
                                    flex_direction: FlexDirection::Row,
                                    ..default()
                                })
                                .with_children(|r| cmd_tile(r, 4, "\u{f070e} Flee", 92.0, red, red));
                        });
                } else {
                    let header: &str = match level {
                        MenuLevel::Root => "ACTIONS", // Psyker root list
                        MenuLevel::Skills => "SKILL",
                        MenuLevel::Items => "ITEM",
                        MenuLevel::Manifest => "FOCUS",
                        MenuLevel::Revoke => "REVOKE",
                        MenuLevel::Target => "TARGET",
                    };
                    panel
                        .spawn(Node {
                            width: Val::Px(230.0),
                            flex_direction: FlexDirection::Column,
                            row_gap: Val::Px(2.0),
                            ..default()
                        })
                        .with_children(|list| {
                            list.spawn((
                                Text::new(header),
                                TextFont { font_size: 13.0, ..default() },
                                TextColor(Color::srgb(0.95, 0.85, 0.5)),
                                Node {
                                    margin: UiRect::bottom(Val::Px(4.0)),
                                    ..default()
                                },
                            ));
                            for (i, label) in labels.iter().enumerate() {
                                list.spawn((
                                    Button,
                                    MenuRow { index: i },
                                    Node {
                                        width: Val::Percent(100.0),
                                        padding: UiRect::axes(Val::Px(8.0), Val::Px(4.0)),
                                        ..default()
                                    },
                                    BackgroundColor(Color::NONE),
                                    BorderRadius::all(Val::Px(3.0)),
                                ))
                                .with_children(|r| {
                                    r.spawn((
                                        Text::new(label.clone()),
                                        TextFont { font_size: 18.0, ..default() },
                                        TextColor(Color::srgb(0.9, 0.93, 1.0)),
                                    ));
                                });
                            }
                        });
                }
            });
        });
}

/// Highlight the cursor tile/row (and hover). Cross tiles keep a dark base so
/// they read as buttons; list rows fall back to transparent.
pub(crate) fn style_command_menu(
    menu: Res<BattleMenu>,
    mut rows: Query<(&MenuRow, &Interaction, &mut BackgroundColor)>,
) {
    // Cross tiles keep a faint glass base so they read as buttons; list rows are
    // transparent until hovered/selected.
    let base = if menu.level == MenuLevel::Root {
        glass_fill()
    } else {
        Color::NONE
    };
    for (row, interaction, mut bg) in &mut rows {
        let selected = row.index == menu.cursor;
        *bg = BackgroundColor(if *interaction == Interaction::Pressed || selected {
            glass_active() // translucent gold selection
        } else if *interaction == Interaction::Hovered {
            Color::srgba(0.5, 0.6, 0.9, 0.25)
        } else {
            base
        });
    }
}

/// A labelled meter (HP or gauge): a bordered track with a proportional fill.
/// Lighten a colour toward white by `f` (>1 brightens), clamped for the sRGB UI.
pub(crate) fn lighten(c: Color, f: f32) -> Color {
    let s = c.to_srgba();
    Color::srgb((s.red * f).min(1.0), (s.green * f).min(1.0), (s.blue * f).min(1.0))
}

/// Linear blend from `a` to `b` by `t` (0→a, 1→b), in sRGB — for quick UI fades.
pub(crate) fn lerp_color(a: Color, b: Color, t: f32) -> Color {
    let (a, b, t) = (a.to_srgba(), b.to_srgba(), t.clamp(0.0, 1.0));
    Color::srgb(
        a.red + (b.red - a.red) * t,
        a.green + (b.green - a.green) * t,
        a.blue + (b.blue - a.blue) * t,
    )
}

/// A stat bar with a bit of depth: a rounded, inset dark track holding a rounded
/// fill that carries a lighter top sheen (reads glossy/3D rather than a flat block).
pub(crate) fn meter(parent: &mut ChildSpawnerCommands, frac: f32, height: f32, fill: Color) {
    parent
        .spawn((
            Node {
                width: Val::Percent(100.0),
                height: Val::Px(height),
                border: UiRect::all(Val::Px(1.0)),
                overflow: Overflow::clip(), // keep the rounded fill inside the track
                ..default()
            },
            BorderColor(Color::srgb(0.35, 0.4, 0.55)),
            BackgroundColor(Color::srgb(0.07, 0.08, 0.12)),
            BorderRadius::all(Val::Px(3.0)),
        ))
        .with_children(|t| {
            t.spawn((
                Node {
                    width: Val::Percent((frac * 100.0).clamp(0.0, 100.0)),
                    height: Val::Percent(100.0),
                    flex_direction: FlexDirection::Column,
                    ..default()
                },
                BackgroundColor(fill),
                BorderRadius::all(Val::Px(2.0)),
            ))
            .with_children(|f| {
                // Top sheen: a lighter band across the upper half → a rounded highlight.
                f.spawn((
                    Node {
                        width: Val::Percent(100.0),
                        height: Val::Percent(45.0),
                        ..default()
                    },
                    BackgroundColor(lighten(fill, 1.45)),
                    BorderRadius::all(Val::Px(2.0)),
                ));
            });
        });
}

/// A [`meter`] with a `label` (e.g. "32/40") centred *inside* the bar — saves a
/// line vs a separate HP text row. The fill is absolutely positioned so it doesn't
/// shove the label off-centre, and the label draws on top.
pub(crate) fn meter_labeled(parent: &mut ChildSpawnerCommands, frac: f32, height: f32, fill: Color, label: String) {
    parent
        .spawn((
            Node {
                width: Val::Percent(100.0),
                height: Val::Px(height),
                border: UiRect::all(Val::Px(1.0)),
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                overflow: Overflow::clip(),
                ..default()
            },
            BorderColor(Color::srgb(0.35, 0.4, 0.55)),
            BackgroundColor(Color::srgb(0.07, 0.08, 0.12)),
            BorderRadius::all(Val::Px(3.0)),
        ))
        .with_children(|t| {
            // Fill: absolute so the centred label stays centred over the whole track.
            t.spawn((
                Node {
                    position_type: PositionType::Absolute,
                    left: Val::Px(0.0),
                    top: Val::Px(0.0),
                    bottom: Val::Px(0.0),
                    width: Val::Percent((frac * 100.0).clamp(0.0, 100.0)),
                    ..default()
                },
                BackgroundColor(fill),
                BorderRadius::all(Val::Px(2.0)),
            ));
            // Label on top (later sibling renders above the fill).
            t.spawn((
                Text::new(label),
                TextFont { font_size: (height - 3.0).max(10.0), ..default() },
                TextColor(Color::srgb(0.97, 0.99, 1.0)),
            ));
        });
}

/// True if a combatant was hit within the last [`FLASH_TTL`] seconds.
pub(crate) fn flashing(hitfx: &HitFx, id: &str) -> bool {
    hitfx.items.iter().any(|h| h.target == id && h.age < FLASH_TTL)
}

/// During the Target picker, classify a combatant: `(is a candidate, is the
/// highlighted pick)`. Off the Target page both are false, so panels render normally.
pub(crate) fn target_state(menu: &BattleMenu, id: &str) -> (bool, bool) {
    if menu.level != MenuLevel::Target {
        return (false, false);
    }
    let candidate = menu.rows.iter().any(|(_, v)| v == id);
    let cursor = menu.rows.get(menu.cursor).map(|(_, v)| v.as_str()) == Some(id);
    (candidate, cursor)
}

/// Immediate-mode enemy HUD: a compact name + HP bar floated in screen space
/// **under each enemy's 3D sprite** (projected from the arena each frame), so the
/// health reads on the creature itself instead of a detached row of chips. The bar
/// flashes white when the enemy is struck and its name goes gold while it's the
/// current target (matching the sprite's shimmer).
#[allow(clippy::too_many_arguments)]
pub(crate) fn render_enemy_panel(
    mut commands: Commands,
    battle: Res<BattleData>,
    hitfx: Res<HitFx>,
    menu: Res<BattleMenu>,
    target: Res<BattleTarget>,
    perks: Res<PerksRes>,
    cam_q: Query<(&Camera, &GlobalTransform), With<Camera3d>>,
    actors: Query<(&BattleActor, &GlobalTransform)>,
    existing: Query<Entity, With<BattleScene>>,
) {
    for e in &existing {
        commands.entity(e).despawn();
    }
    let Some((cam, cam_tf)) = cam_q.iter().next() else { return };
    let focus = highlight_focus(&battle, &menu, &target);
    // A full-screen, non-interactive layer; each enemy's bar is absolutely placed.
    commands
        .spawn((
            BattleScene,
            Node {
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                ..default()
            },
        ))
        .with_children(|p| {
            for c in battle.combatants.iter().filter(|c| !c.is_player && c.hp > 0) {
                // Project the enemy sprite's feet to the screen and hang the bar just
                // below them. Skip if it has no arena actor yet or is behind the camera.
                let Some((_, gt)) = actors.iter().find(|(a, _)| a.id == c.id) else { continue };
                let Ok(feet) = cam.world_to_viewport(cam_tf, gt.translation()) else { continue };
                let frac = c.hp as f32 / c.max_hp.max(1) as f32;
                let hurt = flashing(&hitfx, &c.id);
                let is_target = focus.as_deref() == Some(c.id.as_str());
                let faction = c.statuses.iter().find_map(|s| s.strip_prefix("faction:"));
                let hp_fill = if hurt {
                    Color::srgb(1.0, 0.95, 0.95)
                } else {
                    faction.map(faction_color).unwrap_or(Color::srgb(0.85, 0.3, 0.3))
                };
                let name_color = if hurt {
                    Color::srgb(1.0, 1.0, 1.0)
                } else if is_target {
                    Color::srgb(1.0, 0.9, 0.45)
                } else {
                    Color::srgb(0.95, 0.72, 0.72)
                };
                const W: f32 = 132.0;
                p.spawn(Node {
                    position_type: PositionType::Absolute,
                    left: Val::Px(feet.x - W * 0.5),
                    top: Val::Px(feet.y + 8.0),
                    width: Val::Px(W),
                    flex_direction: FlexDirection::Column,
                    align_items: AlignItems::Center,
                    row_gap: Val::Px(3.0),
                    ..default()
                })
                .with_children(|e| {
                    e.spawn((
                        Text::new(format!("{}  {}/{}", c.name, c.hp, c.max_hp)),
                        TextFont { font_size: 14.0, ..default() },
                        TextColor(name_color),
                    ));
                    meter(e, frac, 10.0, hp_fill);
                    // Hunter "Predator's Eye" top tier: reveal the enemy's ATB gauge
                    // (otherwise hidden — you only see foe HP). ATB shows in battle only.
                    if perks.0.hunter_intel >= 3 {
                        meter(e, c.gauge as f32, 5.0, Color::srgb(0.5, 0.72, 1.0));
                    }
                });
            }
        });
}

/// Collapse state for the single ally box (toggled by its header button).
#[derive(Resource, Default)]
pub(crate) struct AllyPanel {
    collapsed: bool,
}

/// Marker on the ally box's collapse/expand header button.
#[derive(Component)]
pub(crate) struct AllyCollapseBtn;

/// Flip [`AllyPanel::collapsed`] when the ally box header is clicked.
pub(crate) fn ally_collapse_click(
    mut panel: ResMut<AllyPanel>,
    btn: Query<&Interaction, (Changed<Interaction>, With<AllyCollapseBtn>)>,
) {
    if btn.iter().any(|i| *i == Interaction::Pressed) {
        panel.collapsed = !panel.collapsed;
    }
}

/// Immediate-mode ally box: EVERY other player's party lives in ONE glass panel
/// flush to the top of the screen (co-op status without eating the edges). Grouped
/// by owner; each party is a labelled column of slim cells. Collapsible via its
/// header so it can be tucked away to a thin bar.
pub(crate) fn render_ally_parties(
    mut commands: Commands,
    battle: Res<BattleData>,
    hitfx: Res<HitFx>,
    panel: Res<AllyPanel>,
    existing: Query<Entity, With<AllyPartyStrips>>,
) {
    for e in &existing {
        commands.entity(e).despawn();
    }
    // Group joined heroes by owner, preserving first-seen order for a stable layout.
    let mut order: Vec<String> = Vec::new();
    let mut parties: HashMap<String, Vec<&CombatantView>> = HashMap::new();
    for c in battle.combatants.iter() {
        if !c.is_player || battle.your_ids.contains(&c.id) {
            continue;
        }
        let owner = c.player_id.clone().unwrap_or_else(|| c.id.clone());
        if !parties.contains_key(&owner) {
            order.push(owner.clone());
        }
        parties.entry(owner).or_default().push(c);
    }
    if order.is_empty() {
        return;
    }
    let collapsed = panel.collapsed;
    commands
        .spawn((
            AllyPartyStrips,
            Node {
                position_type: PositionType::Absolute,
                top: Val::Px(0.0), // flush to the top — no buffer
                left: Val::Px(0.0),
                right: Val::Px(0.0),
                flex_direction: FlexDirection::Column,
                align_items: AlignItems::Center,
                row_gap: Val::Px(4.0),
                padding: UiRect::axes(Val::Px(10.0), Val::Px(5.0)),
                border: UiRect::all(Val::Px(1.0)),
                ..default()
            },
            BorderColor(glass_edge()),
            BackgroundColor(glass_fill()),
            // Flat top edge (against the screen), rounded bottom.
            BorderRadius {
                top_left: Val::Px(0.0),
                top_right: Val::Px(0.0),
                bottom_left: Val::Px(12.0),
                bottom_right: Val::Px(12.0),
            },
        ))
        .with_children(|box_| {
            // Header: "Allies (N)" + a clickable collapse/expand toggle.
            box_.spawn(Node {
                flex_direction: FlexDirection::Row,
                align_items: AlignItems::Center,
                column_gap: Val::Px(10.0),
                ..default()
            })
            .with_children(|hdr| {
                hdr.spawn((
                    Text::new(format!("Allies ({})", order.len())),
                    TextFont { font_size: 13.0, ..default() },
                    TextColor(Color::srgb(0.72, 0.85, 1.0)),
                ));
                hdr.spawn((
                    Button,
                    AllyCollapseBtn,
                    Node {
                        padding: UiRect::axes(Val::Px(7.0), Val::Px(1.0)),
                        border: UiRect::all(Val::Px(1.0)),
                        ..default()
                    },
                    BorderColor(glass_edge()),
                    BackgroundColor(glass_fill()),
                    BorderRadius::all(Val::Px(5.0)),
                ))
                .with_children(|b| {
                    b.spawn((
                        Text::new(if collapsed { "[+]" } else { "[-]" }),
                        TextFont { font_size: 13.0, ..default() },
                        TextColor(Color::srgb(0.85, 0.9, 1.0)),
                    ));
                });
            });
            if collapsed {
                return;
            }
            // Body: each party a column (name + its heroes in a row), side by side.
            box_.spawn(Node {
                flex_direction: FlexDirection::Row,
                column_gap: Val::Px(14.0),
                ..default()
            })
            .with_children(|row| {
                for owner in &order {
                    let heroes = &parties[owner];
                    let label = heroes
                        .iter()
                        .find_map(|c| (!c.name.is_empty() && c.name != "Hero").then(|| c.name.clone()))
                        .map(|n| format!("{n}'s party"))
                        .unwrap_or_else(|| "Allied party".to_string());
                    row.spawn(Node {
                        flex_direction: FlexDirection::Column,
                        align_items: AlignItems::Center,
                        row_gap: Val::Px(3.0),
                        ..default()
                    })
                    .with_children(|col| {
                        col.spawn((
                            Text::new(label),
                            TextFont { font_size: 12.0, ..default() },
                            TextColor(Color::srgb(0.7, 0.82, 1.0)),
                        ));
                        col.spawn(Node {
                            flex_direction: FlexDirection::Row,
                            column_gap: Val::Px(6.0),
                            ..default()
                        })
                        .with_children(|cells| {
                            for c in heroes.iter() {
                                ally_cell(cells, &hitfx, c);
                            }
                        });
                    });
                }
            });
        });
}

/// A compact read-only status cell for one joined ally hero: name (+ HP number
/// inside its bar) and a slim ATB gauge. Kept narrow so a full co-op board fits in
/// the single top ally box.
pub(crate) fn ally_cell(parent: &mut ChildSpawnerCommands, hitfx: &HitFx, c: &CombatantView) {
    let hp_frac = c.hp as f32 / c.max_hp.max(1) as f32;
    let gauge = c.gauge.clamp(0.0, 1.0) as f32;
    let hurt = flashing(hitfx, &c.id);
    let name = if !c.name.is_empty() && c.name != "Hero" {
        c.name.clone()
    } else {
        "Hero".to_string()
    };
    parent
        .spawn((
            Node {
                width: Val::Px(112.0),
                flex_direction: FlexDirection::Column,
                row_gap: Val::Px(2.0),
                padding: UiRect::all(Val::Px(4.0)),
                border: UiRect::all(Val::Px(1.0)),
                ..default()
            },
            BorderColor(glass_edge()),
            BackgroundColor(if hurt {
                Color::srgba(0.4, 0.12, 0.14, 0.5)
            } else {
                Color::srgba(0.09, 0.12, 0.22, 0.4)
            }),
            BorderRadius::all(Val::Px(7.0)),
        ))
        .with_children(|cell| {
            cell.spawn((
                Text::new(name),
                TextFont { font_size: 13.0, ..default() },
                TextColor(if c.hp == 0 {
                    Color::srgb(0.55, 0.55, 0.6)
                } else {
                    Color::srgb(0.8, 0.88, 1.0)
                }),
            ));
            meter_labeled(cell, hp_frac, 13.0, Color::srgb(0.35, 0.6, 0.95), format!("{}/{}", c.hp, c.max_hp));
            meter(cell, gauge, 5.0, Color::srgb(0.4, 0.85, 0.5));
        });
}

/// Immediate-mode party window (bottom-left): one row per hero with HP bar, ATB
/// gauge, the active-hero highlight, a ready flag, and the queued-order icon.
/// One Lufia-style party window (name + Lv, HP + ATB bars, portrait, order icon).
pub(crate) fn party_cell(
    parent: &mut ChildSpawnerCommands,
    battle: &BattleData,
    hitfx: &HitFx,
    menu: &BattleMenu,
    flash: &AtbFlash,
    id: &str,
    _idx: usize,
) {
    let Some(c) = battle.view(id) else { return };
    let active = battle.active.as_deref() == Some(id);
    let ready = battle.ready.contains(id);
    let queued = battle.queued.get(id).map(|o| o.kind);
    // While aiming an ally-targeted action, this cell is a candidate; the cursor one
    // gets the bright ring (reusing the active-hero highlight colour).
    let (_is_cand, is_target_cursor) = target_state(menu, id);
    let hp_frac = c.hp as f32 / c.max_hp.max(1) as f32;
    let gauge = c.gauge.clamp(0.0, 1.0) as f32;
    let name = battle.hero_label(id);
    let hurt = flashing(hitfx, id);
    // "Turn's up" pop: 1.0 the instant the gauge fills, fading to 0 over the TTL.
    let atb_pop = flash
        .age
        .get(id)
        .map(|a| (1.0 - a / ATB_FLASH_TTL).clamp(0.0, 1.0))
        .unwrap_or(0.0);
    // Frosted glass: hairline edge normally, a brighter gold edge for the active /
    // target hero so it still stands out without a heavy border.
    let base_border = if is_target_cursor || active {
        glass_active_edge()
    } else {
        glass_edge()
    };
    parent
        .spawn((
            // The whole cell is a button: tap it to command that hero (see
            // `party_select_click`).
            Button,
            PartyCellButton { id: id.to_string() },
            Node {
                flex_grow: 1.0,
                flex_basis: Val::Px(0.0),
                flex_direction: FlexDirection::Row,
                column_gap: Val::Px(8.0),
                padding: UiRect::all(Val::Px(7.0)),
                border: UiRect::all(Val::Px(1.0)),
                ..default()
            },
            // Flash the edge toward bright gold-white as the turn comes up.
            BorderColor(if atb_pop > 0.0 {
                lerp_color(base_border, Color::srgb(1.0, 0.98, 0.7), atb_pop)
            } else {
                base_border
            }),
            BackgroundColor(if hurt {
                Color::srgba(0.4, 0.12, 0.14, 0.55)
            } else if is_target_cursor {
                Color::srgba(0.28, 0.26, 0.1, 0.5)
            } else if active {
                glass_active()
            } else {
                glass_fill()
            }),
            BorderRadius::all(Val::Px(10.0)),
        ))
        .with_children(|cell| {
            // Compact 3-line readout: name + Lv/tag, HP bar (number inside), ATB bar.
            cell.spawn(Node {
                flex_grow: 1.0,
                flex_direction: FlexDirection::Column,
                row_gap: Val::Px(2.0),
                ..default()
            })
            .with_children(|col| {
                // Line 1: name (left); Lv + action tag (right).
                col.spawn(Node {
                    flex_direction: FlexDirection::Row,
                    justify_content: JustifyContent::SpaceBetween,
                    align_items: AlignItems::Center,
                    column_gap: Val::Px(6.0),
                    ..default()
                })
                .with_children(|line| {
                    line.spawn((
                        Text::new(name),
                        TextFont { font_size: 16.0, ..default() },
                        TextColor(if c.hp == 0 {
                            Color::srgb(0.55, 0.55, 0.6)
                        } else {
                            Color::srgb(0.85, 0.92, 1.0)
                        }),
                    ));
                    line.spawn(Node {
                        flex_direction: FlexDirection::Row,
                        align_items: AlignItems::Center,
                        column_gap: Val::Px(6.0),
                        ..default()
                    })
                    .with_children(|right| {
                        let (tag, tag_color) = match queued {
                            Some(k) => (k.tag().to_string(), k.color()),
                            None if ready => ("!".to_string(), Color::srgb(0.98, 0.8, 0.3)),
                            None => (String::new(), Color::NONE),
                        };
                        if !tag.is_empty() {
                            right.spawn((
                                Text::new(tag),
                                TextFont { font_size: 13.0, ..default() },
                                TextColor(tag_color),
                            ));
                        }
                        right.spawn((
                            Text::new(format!("Lv{}", c.level)),
                            TextFont { font_size: 12.0, ..default() },
                            TextColor(Color::srgb(0.95, 0.85, 0.4)),
                        ));
                    });
                });
                // Line 2: HP bar with the number inside, plus status suffixes when
                // present — Barrier ◆, Regen /t, Evasion ~%, Adrenaline ⚡ (the
                // action tag already rides Line 1, so it isn't repeated here).
                let barrier = status_num(&c.statuses, "barrier:");
                let regen = status_num(&c.statuses, "regen:");
                let evasion = status_num(&c.statuses, "evasion:");
                let mut hp_label = format!("{}/{}", c.hp, c.max_hp);
                // Status suffixes use Nerd Font icons (see UiFont): shield =
                // Barrier, heart-pulse = Regen, runner = Evasion, bolt = Adrenaline.
                if barrier > 0 {
                    hp_label.push_str(&format!("  \u{f132}{barrier}")); // shield = Barrier
                }
                if regen > 0 {
                    hp_label.push_str(&format!("  \u{f05f7}{regen}/t")); // heart-pulse = Regen/turn
                }
                if evasion > 0 {
                    hp_label.push_str(&format!("  \u{f070e}{evasion}%")); // runner = Evasion (dodge)
                }
                let adrenaline_max = status_num(&c.statuses, "adrenaline_max:");
                if adrenaline_max > 0 {
                    let adr = status_num(&c.statuses, "adrenaline:");
                    hp_label.push_str(&format!("  \u{f0e7}{adr}/{adrenaline_max}")); // bolt = Adrenaline
                }
                meter_labeled(col, hp_frac, 15.0, Color::srgb(0.35, 0.6, 0.95), hp_label);
                // Line 3: ATB bar — flares gold-white the instant the gauge fills.
                let atb_fill = lerp_color(
                    Color::srgb(0.4, 0.85, 0.5),
                    Color::srgb(1.0, 0.98, 0.7),
                    atb_pop,
                );
                meter(col, gauge, 6.0, atb_fill);
                // Psyker: a compact row of Focus slots (filled = manifestation abbrev).
                let (fmax, foci) = parse_foci(&c.statuses);
                if fmax > 0 {
                    col.spawn(Node {
                        flex_direction: FlexDirection::Row,
                        column_gap: Val::Px(5.0),
                        margin: UiRect::top(Val::Px(1.0)),
                        ..default()
                    })
                    .with_children(|row| {
                        for slot in 0..fmax {
                            let (label, filled) = match foci.get(slot) {
                                Some((k, s)) => {
                                    let tag = if *s > 1 {
                                        format!("{}{}", manifest_abbrev(k), s)
                                    } else {
                                        manifest_abbrev(k)
                                    };
                                    (tag, true)
                                }
                                None => ("-".to_string(), false),
                            };
                            row.spawn((
                                Text::new(label),
                                TextFont { font_size: 12.0, ..default() },
                                TextColor(if filled {
                                    Color::srgb(0.8, 0.6, 1.0)
                                } else {
                                    Color::srgb(0.4, 0.45, 0.6)
                                }),
                            ));
                        }
                    });
                }
            });
        });
}

/// Immediate-mode party grid: a 2×2 of Lufia-style windows across the bottom,
/// with the command cross floating in the centre gap.
pub(crate) fn render_party_window(
    mut commands: Commands,
    battle: Res<BattleData>,
    hitfx: Res<HitFx>,
    menu: Res<BattleMenu>,
    flash: Res<AtbFlash>,
    existing: Query<Entity, With<PartyWindow>>,
) {
    for e in &existing {
        commands.entity(e).despawn();
    }
    let ids = battle.your_ids.clone();
    // Compact HD-2D HUD: a single row of slim hero status cells across the very
    // bottom, leaving the arena above open for the 3D combatant sprites.
    commands
        .spawn((
            PartyWindow,
            Node {
                position_type: PositionType::Absolute,
                left: Val::Px(10.0),
                right: Val::Px(10.0),
                bottom: Val::Px(10.0),
                // Tall enough for 4 lines: name + HP + ATB + the Psyker's Focus-slot
                // row. At 74px the Focus row overflowed and clipped the ATB bar, so
                // a Psyker's gauge looked like it never filled.
                height: Val::Px(92.0),
                flex_direction: FlexDirection::Row,
                column_gap: Val::Px(8.0),
                ..default()
            },
        ))
        .with_children(|row| {
            for i in 0..4 {
                match ids.get(i) {
                    Some(id) => party_cell(row, &battle, &hitfx, &menu, &flash, id, i),
                    None => {
                        row.spawn(Node {
                            flex_grow: 1.0,
                            flex_basis: Val::Px(0.0),
                            ..default()
                        });
                    }
                }
            }
        });
}

/// Start a fading flash for any hero whose ATB just filled (rising edge into
/// `ready`) and age out the running ones — the "your turn is up" pop that
/// [`party_cell`] renders on the ATB bar. Frozen in the static mockup.
pub(crate) fn advance_atb_flash(time: Res<Time>, battle: Res<BattleData>, mut flash: ResMut<AtbFlash>) {
    if battle_mockup_flag() {
        return;
    }
    let dt = time.delta_secs();
    // Age existing flashes and drop the expired.
    flash.age.retain(|_, a| {
        *a += dt;
        *a < ATB_FLASH_TTL
    });
    // Newly-ready heroes (weren't ready last frame) get a fresh flash.
    for id in battle.ready.iter() {
        if !flash.prev.contains(id) {
            flash.age.insert(id.clone(), 0.0);
        }
    }
    flash.prev = battle.ready.iter().cloned().collect();
}

/// Age floating hit numbers; drop the expired. Frozen in the static mockup so
/// the seeded feedback stays on screen.
pub(crate) fn advance_hit_fx(time: Res<Time>, mut hitfx: ResMut<HitFx>) {
    if battle_mockup_flag() {
        return;
    }
    let dt = time.delta_secs();
    for h in &mut hitfx.items {
        h.age += dt;
    }
    hitfx.items.retain(|h| h.age < HIT_TTL);
    hitfx.acts.retain(|_, a| {
        *a += dt;
        *a < ATTACK_LUNGE_TTL
    });
}

/// Immediate-mode overlay: draw each floating number, rising and fading, anchored
/// over the monster (top-centre) or the striking hero's slot (bottom-left).
pub(crate) fn render_hit_fx(
    mut commands: Commands,
    hitfx: Res<HitFx>,
    battle: Res<BattleData>,
    windows: Query<&Window>,
    existing: Query<Entity, With<HitFxRoot>>,
) {
    for e in &existing {
        commands.entity(e).despawn();
    }
    let Some(win) = windows.iter().next() else {
        return;
    };
    let (w, h) = (win.width(), win.height());
    commands
        .spawn((
            HitFxRoot,
            Node {
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                position_type: PositionType::Absolute,
                ..default()
            },
        ))
        .with_children(|p| {
            for hit in &hitfx.items {
                let (x, y0) = if Some(hit.target.as_str()) == battle.monster_combatant.as_deref() {
                    (w * 0.5 - 16.0, h * 0.22)
                } else {
                    // Heroes sit in a single compact row across the bottom; float the
                    // number over that hero's cell.
                    let idx = battle
                        .your_ids
                        .iter()
                        .position(|id| id == &hit.target)
                        .unwrap_or(0);
                    ((idx as f32 + 0.5) / 4.0 * w, h - 150.0)
                };
                let rise = hit.age * 46.0;
                let alpha = (1.0 - hit.age / HIT_TTL).clamp(0.0, 1.0);
                p.spawn((
                    Node {
                        position_type: PositionType::Absolute,
                        left: Val::Px(x),
                        top: Val::Px(y0 - rise),
                        ..default()
                    },
                    Text::new(hit.text.clone()),
                    TextFont {
                        font_size: 26.0,
                        ..default()
                    },
                    TextColor(hit.color.with_alpha(alpha)),
                ));
            }
        });
}

/// Turn a resolved effect into a floating number (skips zero/no-op effects).
pub(crate) fn push_hit_fx(hitfx: &mut HitFx, e: &HitEffect) {
    let (text, color) = match e.kind.to_lowercase().as_str() {
        "damage" => {
            let n = e.amount.unwrap_or(0);
            if n == 0 {
                return;
            }
            if e.crit {
                // Crits pop in gold with a "CRIT!" flourish.
                (format!("-{n}  CRIT!"), Color::srgb(1.0, 0.85, 0.3))
            } else {
                (format!("-{n}"), Color::srgb(1.0, 0.5, 0.4))
            }
        }
        "heal" => {
            let n = e.amount.unwrap_or(0);
            if n == 0 {
                return;
            }
            (format!("+{n}"), Color::srgb(0.5, 1.0, 0.6))
        }
        "ko" => ("KO!".to_string(), Color::srgb(1.0, 0.35, 0.35)),
        _ => return,
    };
    hitfx.items.push(Hit {
        target: e.target.clone(),
        text,
        color,
        age: 0.0,
    });
}
