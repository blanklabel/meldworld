//! Battle: the ATB command panel (d-pad), party HUD, 3D arena actors + camera,
//! targeting, order queue, hit FX, and per-class kits.
//! Extracted from `main.rs` during the module reorg.

use std::collections::{HashMap, HashSet};

use bevy::input::mouse::MouseWheel;

use meld_client::glass;
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
    // `MELD_BATTLE=skills` wants the Skill page, which is the one with tooltips on it.
    // The mock sets it at startup and this reset would undo it.
    if std::env::var("MELD_BATTLE").as_deref() == Ok("skills") {
        menu.level = MenuLevel::Skills;
        menu.dirty = true;
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
    // The battle stages on FLAT ground (the ground shader's `terrain_amp` is 0 outside the
    // Overworld), so actors sit at their designed Y — NOT lifted by `terrain_height`, which
    // (seeded per run) would otherwise bury or float them off the flat stage.
    let class = c
        .statuses
        .iter()
        .find_map(|s| s.strip_prefix("class:"))
        .unwrap_or("explorer");
    let frames = wa.class_frames(class);
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
            // The Explorer's is the big "Predator's Eye" beam that lights the enemy row
            // across the arena; every other class carries only a soft, short-range
            // glow — bright enough to stay readable, small enough not to flicker as
            // the renderer's light clusters fight over a pile of equal lights.
            let is_explorer = class == "explorer";
            let (strength, range, radius) = if is_explorer {
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
                    shadow_maps_enabled: false,
                    ..default()
                },
                Transform::from_xyz(0.0, 1.6, 0.0),
            ));
            // A bust has no legs to ground, so it skips the contact shadow.
            if !bust {
                p.spawn((
                    Mesh3d(wa.shadow_mesh.clone()),
                    MeshMaterial3d(wa.shadow_mat.clone()),
                    hd2d::ContactShadow,
                    Transform::from_xyz(0.0, 0.02, 0.0)
                        .with_rotation(Quat::from_rotation_x(-std::f32::consts::FRAC_PI_2))
                        .with_scale(Vec3::new(1.0, 0.55, 1.0)),
                ));
            }
        });
}

/// Spawn one enemy billboard at `root`, `h` world-units tall (shrunk when the party
/// surrounds them), with its hidden target reticle.
/// How much bigger or smaller a pack member draws than a lone creature of its species.
/// The leader is the "one big spider", its minions the "little ones" — the shape the
/// `[encounters]` stat multipliers already assume but nothing drew.
pub(crate) fn pack_scale(statuses: &[String]) -> f32 {
    if statuses.iter().any(|s| s == "pack:leader") {
        1.3
    } else if statuses.iter().any(|s| s == "pack:minion") {
        0.75
    } else {
        1.0
    }
}

/// A pack member's name says which one it is, so two rows of the same species with very
/// different health read as a hierarchy instead of a glitch.
pub(crate) fn pack_label(name: &str, statuses: &[String]) -> String {
    if statuses.iter().any(|s| s == "pack:leader") {
        format!("{name} (leader)")
    } else if statuses.iter().any(|s| s == "pack:minion") {
        format!("{name} (runt)")
    } else {
        name.to_string()
    }
}

pub(crate) fn spawn_enemy_actor(
    commands: &mut Commands,
    wa: &WorldAssets,
    mats: &mut Assets<StandardMaterial>,
    c: &CombatantView,
    root: Vec3,
    h: f32,
) {
    // Flat battle stage, matching the heroes (see spawn_hero_actor) — no terrain lift.
    // FS-4 unique boss mechanics: an Elite/Gatekeeper carrying a named boss
    // identity (`boss:<key>` wire status, mirroring hero `class:`) renders as
    // its actual animated sprite set instead of the generic creature
    // billboard, and a bit larger so it visibly reads as a boss — every enemy
    // otherwise renders at the same size in the battle view.
    let boss_key = c.statuses.iter().find_map(|s| s.strip_prefix("boss:"));
    let boss_frames = boss_key.and_then(|k| wa.boss_frames(k));
    let h = if boss_frames.is_some() { h * 1.5 } else { h };
    // An ordinary creature with an installed sprite set is animated here too, and a
    // pack's LEADER draws from its own art rather than being a scaled-up copy of the
    // rank and file standing beside it. Only
    // reached when this is not a named boss: a boss overlays a host creature, and its own
    // set has to win over the host species'.
    let creature_frames = if boss_frames.is_some() {
        None
    } else {
        let kind = crate::overworld::creature_kind(&c.name);
        let leader = c.statuses.iter().any(|s| s == "pack:leader");
        wa.creature_frames(&kind, leader).cloned()
    };
    // A pack's leader and its minions are the SAME species at 1.7x and 0.45x HP, so
    // drawing them identically made a 3.8x health gap look broken. Size is the read the
    // balance table already assumes ("one big spider with four little ones").
    let h = h * pack_scale(&c.statuses);
    // PG-2: the same boss met deeper wears a darker palette (`boss_band:<n>`,
    // server-assigned from the level it is met at). Only a named boss has a band,
    // so an ordinary creature keeps the neutral tint.
    let base_tint = crate::world_render::boss_band_tint(
        c.statuses
            .iter()
            .find_map(|s| s.strip_prefix("boss_band:"))
            .and_then(|n| n.parse::<u8>().ok())
            .unwrap_or(0),
    );
    // The diamond marker hovers just above the sprite's head (its tip reaches down
    // toward the head, so keep a small gap above `h`).
    let marker_y = h + 0.45;
    // Bespoke HD-2D selection diamond (PixelLab) instead of the old 3D faceted mesh.
    let marker_mat = mats.add(hd2d::sprite_material(
        Color::WHITE,
        wa.prop_sprites
            .get("marker_target_marker")
            .cloned()
            .unwrap_or_default(),
    ));
    let mut root_cmds = commands.spawn((
        BattleActor { id: c.id.clone() },
        Transform::from_translation(root),
        Visibility::default(),
    ));
    if let Some(frames) = boss_frames.cloned().or(creature_frames).as_ref() {
        // Animated boss/creature actor: same CharSprite pattern spawn_hero_actor uses,
        // driven by the same generic `hd2d::animate_chars` system.
        let mat = mats.add(hd2d::sprite_material(base_tint, frames.idle[0].clone()));
        root_cmds.insert(CharSprite::new(frames.clone(), mat.clone(), root));
        root_cmds.with_children(|p| {
            p.spawn((
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
                hd2d::ContactShadow,
                Transform::from_xyz(0.0, 0.02, 0.0)
                    .with_rotation(Quat::from_rotation_x(-std::f32::consts::FRAC_PI_2))
                    .with_scale(Vec3::new(h * 0.42, h * 0.23, 1.0)),
            ));
            p.spawn((
                TargetDiamond { id: c.id.clone(), base_y: marker_y },
                Mesh3d(wa.sprite_quad.clone()),
                MeshMaterial3d(marker_mat),
                Transform::from_xyz(0.0, marker_y, 0.0).with_scale(Vec3::splat(0.8 / 2.2)),
                hd2d::Billboard,
                Visibility::Hidden,
            ));
        });
        return;
    }
    // Plain creature: exactly the billboard the overworld uses, resolved by the
    // normalized kind (strips any champion affix like "Swift ") so a mob keeps
    // its sprite crossing from the overworld into the fight.
    let tex = creature_sprite(wa, &c.name);
    let mat = mats.add(hd2d::sprite_material(base_tint, tex));
    root_cmds.with_children(|p| {
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
            hd2d::ContactShadow,
            Transform::from_xyz(0.0, 0.02, 0.0)
                .with_rotation(Quat::from_rotation_x(-std::f32::consts::FRAC_PI_2))
                .with_scale(Vec3::new(h * 0.42, h * 0.23, 1.0)),
        ));
        // Target marker — a camera-facing selection diamond sprite, hidden until
        // this enemy is the picked target (bobbed by `highlight_target`).
        p.spawn((
            TargetDiamond { id: c.id.clone(), base_y: marker_y },
            Mesh3d(wa.sprite_quad.clone()),
            MeshMaterial3d(marker_mat),
            Transform::from_xyz(0.0, marker_y, 0.0).with_scale(Vec3::splat(0.8 / 2.2)),
            hd2d::Billboard,
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
    // TWO RANKS, from the server's own `row:back` — the same flag the party strip reads
    // for heroes. Laid out as ranks rather than one line for the reason every game that
    // fields packs this size does it: five abreast already spans the screen, and ten in a
    // row would put half the encounter off both edges at a size the world now spawns.
    let rear: Vec<bool> =
        enemies.iter().map(|c| c.statuses.iter().any(|s| s == "row:back")).collect();
    let (front_n, back_n) =
        (rear.iter().filter(|b| !**b).count(), rear.iter().filter(|b| **b).count());
    let (mut fi, mut bi) = (0usize, 0usize);
    for (i, c) in enemies.iter().enumerate() {
        if seen.contains(&c.id) {
            continue;
        }
        // The back rank sits deeper and is inset half a gap, so it reads as *behind* the
        // front rather than as a second unrelated line.
        let (n, idx, z_off, inset) = if rear[i] {
            bi += 1;
            (back_n, bi - 1, -gap * 0.9, 0.5)
        } else {
            fi += 1;
            (front_n, fi - 1, 0.0, 0.0)
        };
        let x = (idx as f32 - (n.max(1) as f32 - 1.0) * 0.5 + inset) * gap;
        spawn_enemy_actor(&mut commands, &wa, &mut mats, c, Vec3::new(x, 0.0, cz + z_off), h);
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

/// Hand any just-resolved action clip (queued on `HitFx::act_clip` from
/// `battle.action_resolved`) to the striking hero's `CharSprite`, which plays it once
/// over walk/idle (see `hd2d::animate_chars`). Consumes the queue so each clip fires a
/// single time. The clip swaps the sprite's `base_color_texture`; the lunge/flash in
/// [`animate_battle_actors`] touches `base_color`/`emissive`, so the two compose.
/// The clip every class has: its basic battle attack.
const GENERIC_STRIKE: &str = "attack";

/// Is this ability a blow aimed at the enemy — i.e. would a generic attack animation read
/// as the truth? A heal, a ward or a self-buff would not, and gets no stand-in.
fn swings_at_a_foe(skill: &str) -> bool {
    matches!(
        meld_proto::skills::target_of(skill),
        meld_proto::skills::Target::Enemy | meld_proto::skills::Target::AllEnemies
    )
}

pub(crate) fn drive_battle_action_clips(
    mut hitfx: ResMut<HitFx>,
    mut q: Query<(&BattleActor, &mut hd2d::CharSprite)>,
) {
    if hitfx.act_clip.is_empty() {
        return;
    }
    for (ba, mut cs) in &mut q {
        if let Some(clip) = hitfx.act_clip.get(&ba.id) {
            if cs.frames.clips.contains_key(clip) {
                cs.action = Some((clip.clone(), 0.0));
            } else if swings_at_a_foe(clip) && cs.frames.clips.contains_key(GENERIC_STRIKE) {
                // ⚠️ MOST ABILITIES HAVE NO ART, AND A MISSING CLIP USED TO MEAN THE HERO
                // JUST STOOD THERE. Measured against the registry: 74 of 92 abilities have
                // no clip — every class's L35-and-up rungs (the art predates that ladder)
                // and EVERY ability of the five classes whose sets were regenerated with
                // only walk+attack, the Explorer among them. So the default class played
                // nothing at all, for anything, which reads as the animations being stale.
                //
                // A generic swing is the honest stand-in for a blow and a LIE for anything
                // else — a hero must not slash the air to hand out a Barrier — so the
                // registry decides. Falling back on `target` rather than on a list means an
                // ability added tomorrow is covered the day it lands.
                cs.action = Some((GENERIC_STRIKE.to_string(), 0.0));
            }
        }
    }
    hitfx.act_clip.clear();
}

/// Turn the hero currently AWAITING your command to face the camera (look at you);
/// every other combatant keeps its fighting stance facing the foes. "Awaiting a
/// command" = its ATB gauge is full (`ready`) AND it's the hero the command window is
/// addressing (`active`) — so the moment you issue an order it turns back and acts.
/// Only heroes carry a `CharSprite`, so enemies are unaffected. Facing is untouched,
/// so the lunge-toward-target motion still works.
pub(crate) fn drive_battle_facing(
    battle: Res<BattleData>,
    mut q: Query<(&BattleActor, &mut hd2d::CharSprite)>,
) {
    for (ba, mut cs) in &mut q {
        let awaiting =
            battle.active.as_deref() == Some(ba.id.as_str()) && battle.ready.contains(&ba.id);
        if cs.face_cam != awaiting {
            cs.face_cam = awaiting;
        }
    }
}

/// Give combat weight: struck sprites flash white + recoil (with a quick shake),
/// attackers lunge in and back, and a downed combatant grays out. Drives each sprite
/// *child* (leaving the actor root — and thus the walk-cycle logic + shadow — alone).
pub(crate) fn animate_battle_actors(
    battle: Res<BattleData>,
    hitfx: Res<HitFx>,
    feel: Res<BattleFeel>,
    sky: Res<Sky>,
    mut mats: ResMut<Assets<StandardMaterial>>,
    mut q: Query<(&mut Transform, &SpriteQuad, Has<PlayerGlowSprite>)>,
) {
    // The night glow is folded in HERE rather than left to `illuminate_players`, which
    // owns it everywhere else. Both systems wrote `emissive` on the same battle-hero
    // material with no ordering between them, so whichever the scheduler ran second that
    // frame decided whether the hero was lit — and the party flickered for the whole
    // fight. One field, one owner: `illuminate_players` now skips a `SpriteQuad`.
    //
    // ⚠️ AND IT IS THE PLAYER CHARACTERS' GLOW, NOT THE ARENA'S. A hero self-illuminates
    // because it CARRIES a lamp and a co-located point light cannot light the billboard it
    // sits inside; a creature carries nothing, so what lights it is the party's lamps —
    // chiefly the Explorer's, the one with the reach to cross the arena. Handing the same
    // glow to every `SpriteQuad` made every creature emit its own light instead: emissive
    // is added flat across a TEXTURED billboard, so at full dark the enemy row rendered as
    // solid white silhouettes and the Explorer's lantern read as a bug that erased the art.
    // Gated on [`PlayerGlowSprite`], which is exactly "is this a player character".
    let night = (1.0 - sky.day).clamp(0.0, 1.0);
    for (mut tf, s, player) in &mut q {
        let ef = if player { night * 1.15 } else { 0.0 };
        let glow = LinearRgba::rgb(ef, ef * 0.9, ef * 0.7);
        // KO: gray the sprite, drop any hit motion — reads as "downed".
        if battle.view(&s.id).map(|c| c.hp <= 0).unwrap_or(false) {
            if let Some(mut m) = mats.get_mut(&s.mat) {
                let c = s.base.to_srgba();
                let lum = 0.3 * c.red + 0.5 * c.green + 0.2 * c.blue;
                m.base_color = Color::srgb(lum * 0.45, lum * 0.45, lum * 0.5);
                // Half the glow: a downed hero still has to be FINDABLE at night, and
                // reading dimmer than the ones still standing is the point.
                m.emissive = LinearRgba::rgb(glow.red * 0.5, glow.green * 0.5, glow.blue * 0.5);
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
        let recoil = if hit_age < feel.recoil_ttl {
            (1.0 - hit_age / feel.recoil_ttl) * feel.recoil_distance
        } else {
            0.0
        };
        // Lunge: step toward the foe and back (half-sine, peaks mid-window).
        let lunge = if lunge_age < feel.lunge_ttl {
            (std::f32::consts::PI * lunge_age / feel.lunge_ttl).sin() * feel.lunge_distance
        } else {
            0.0
        };
        // A brief lateral shake right at the moment of impact.
        let shake = if hit_age < feel.white_ttl {
            (hit_age * feel.shake_hz).sin() * (1.0 - hit_age / feel.white_ttl) * feel.shake_distance
        } else {
            0.0
        };
        let perp = Vec3::new(s.forward.z, 0.0, -s.forward.x);
        let off = s.forward * (lunge - recoil) + perp * shake;
        tf.translation.x = off.x;
        tf.translation.z = off.z;

        // Explorer "rage": as banked Adrenaline climbs toward max, redden the sprite
        // and add a faint hot glow so a Explorer *looks* angrier the more it's built.
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
        if let Some(mut m) = mats.get_mut(&s.mat) {
            if hit_age < feel.white_ttl {
                m.base_color =
                    lerp_color(s.base, Color::srgb(2.6, 2.6, 2.6), 1.0 - hit_age / feel.white_ttl);
                m.emissive = glow;
            } else {
                m.base_color = lerp_color(s.base, Color::srgb(1.9, 0.5, 0.35), rage * 0.55);
                m.emissive = LinearRgba::rgb(
                    glow.red + 0.5 * rage,
                    glow.green + 0.04 * rage,
                    glow.blue,
                );
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
    backpack: Res<RunBackpack>,
    roster: Res<crate::PartyRoster>,
    mut menu: ResMut<BattleMenu>,
    mut battle: ResMut<BattleData>,
    mut target: ResMut<BattleTarget>,
    mut press: Local<Option<Vec2>>,
    mut tutorial_run: ResMut<TutorialRun>,
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
    // The tutorial's paced command-menu explainer swallows this path too — it
    // calls `queue_order` directly, bypassing `begin_order`'s own guard.
    if tutorial_run.battle_intro.is_some() {
        return;
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
            let held = held_potions(&backpack, battle.active_slot());
            select_entry(idx, &mut menu, &mut battle, &class, &held, &roster, &mut tutorial_run);
        }
        return;
    }
    // Otherwise mark it (start it shimmering) and, for a martial hero, attack it.
    target.selected = Some(eid.clone());
    if battle.active_class() != "psyker" {
        if let Some(active) = battle.active.clone() {
            queue_order(&mut battle, &active, QueuedKind::Attack, Some(eid), &mut menu, &mut tutorial_run);
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
    mut wheel: MessageReader<MouseWheel>,
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
            Option<&mut bevy::post_process::bloom::Bloom>,
            Option<&mut bevy::post_process::dof::DepthOfField>,
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
        // Flat battle stage (actors are no longer terrain-lifted), so the framing is at a
        // fixed Y — no `terrain_height` offset, which (seeded per run) would frame empty
        // sky or ground.
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
    tutorial_run: &mut TutorialRun,
) {
    battle.queued.insert(hero.to_string(), Order { kind, target });
    battle.active = pick_active(battle).or_else(|| Some(hero.to_string()));
    reset_menu(menu);
    // The guided [T]-dive walkthrough's "what to click" step: the first order
    // submitted, of any kind, is proof the player found the command menu.
    if tutorial_run.step == Some(TutorialStep::Fight) {
        tutorial_run.step = Some(TutorialStep::Dungeon);
    }
}

/// Begin an order for `hero`: self-cast orders queue immediately; aimed orders open the
/// Target picker (auto-picking when only one valid target exists).
pub(crate) fn begin_order(
    battle: &mut BattleData,
    menu: &mut BattleMenu,
    hero: &str,
    kind: QueuedKind,
    tutorial_run: &mut TutorialRun,
) {
    // The tutorial's paced command-menu explainer swallows every order-
    // submission path: this is the funnel every root-menu action dispatches
    // through (`select_entry`'s Attack/Defend/Skill/Item/Hold/Flee arms all
    // call it), so guarding here alone also covers the Target/Revoke pages,
    // which only ever open from inside this function.
    if tutorial_run.battle_intro.is_some() {
        return;
    }
    match order_side(kind) {
        None => queue_order(battle, hero, kind, None, menu, tutorial_run),
        Some(side) => {
            let targets = valid_targets(battle, side);
            match targets.len() {
                0 => reset_menu(menu), // nothing valid to hit — abandon the choice
                1 => queue_order(battle, hero, kind, Some(targets[0].1.clone()), menu, tutorial_run),
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
            let name = if c.is_player {
                battle.hero_label(&c.id)
            } else {
                pack_label(&c.name, &c.statuses)
            };
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
        // Remember the exact skill so the sprite layer can play its clip when the
        // server echoes back the (coarse) resolution.
        if let QueuedKind::Skill(sk) = order.kind {
            battle.last_skill.insert(hero.clone(), sk.to_string());
        }
        fire_order(&net.0, &battle_id, &hero, order.kind, target.as_deref());
        battle.ready.remove(&hero);
        battle.queued.remove(&hero);
    }
}

/// The `&'static str` manifestation kind matching a dynamic `kind` string (from a
/// combatant's parsed foci), or `None` if it isn't a known manifestation.
pub(crate) fn manifest_static(kind: &str) -> Option<&'static str> {
    manifests().into_iter().find(|d| d.key == kind).map(|d| d.key)
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
pub(crate) fn select_entry(
    index: usize,
    menu: &mut BattleMenu,
    battle: &mut BattleData,
    class: &str,
    held: &[(String, i32)],
    roster: &crate::PartyRoster,
    tutorial_run: &mut TutorialRun,
) {
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
                Some((actor, kind)) => queue_order(battle, &actor, kind, Some(value), menu, tutorial_run),
                None => reset_menu(menu),
            },
            MenuLevel::Revoke => match manifest_static(&value) {
                Some(kind) => {
                    queue_order(battle, &active, QueuedKind::Focus("revoke", kind), None, menu, tutorial_run)
                }
                None => reset_menu(menu),
            },
            _ => unreachable!(),
        }
        return;
    }

    let hero_level = battle.view(&active).map(|c| c.level).unwrap_or(1);
    let spent = spent_tokens(battle, &active);
    // The held Foci are what make an aspect row appear (see `menu_entries`).
    let foci = held_foci(battle, &active);
    let adrenaline =
        battle.view(&active).map(|v| status_num(&v.statuses, "adrenaline:")).unwrap_or(0);
    let entries = menu_entries(menu.level, class, hero_level, held, &spent, &foci, roster, adrenaline);
    let Some(entry) = entries.get(index) else {
        return;
    };
    // Greyed out: picking this would only get the hero refused (out of Adrenaline,
    // or a once-per-battle call already spent) — and the refusal never resolves the
    // hero's turn server-side, which is what stalled it for the rest of the fight.
    if !entry.enabled {
        return;
    }
    match entry.action {
        EntryAction::Attack => begin_order(battle, menu, &active, QueuedKind::Attack, tutorial_run),
        EntryAction::Defend => begin_order(battle, menu, &active, QueuedKind::Defend, tutorial_run),
        EntryAction::OpenSkills => open_page(menu, MenuLevel::Skills),
        EntryAction::OpenItems => open_page(menu, MenuLevel::Items),
        EntryAction::Skill(kind) => {
            begin_order(battle, menu, &active, QueuedKind::Skill(kind), tutorial_run)
        }
        EntryAction::Item(id) => begin_order(battle, menu, &active, QueuedKind::Item(id), tutorial_run),
        // Psyker: Focus opens the manifestation list; Revoke lists the live Foci.
        EntryAction::OpenManifest => open_page(menu, MenuLevel::Manifest),
        EntryAction::OpenRevoke => open_revoke_page(menu, battle, &active),
        // Cast, or reinforce if already active; begin_order aims offensive ones.
        EntryAction::Manifest(kind) => {
            let verb = manifest_verb(battle, &active, kind);
            begin_order(battle, menu, &active, QueuedKind::Focus(verb, kind), tutorial_run);
        }
        EntryAction::Hold => begin_order(battle, menu, &active, QueuedKind::Hold, tutorial_run),
        EntryAction::Flee => begin_order(battle, menu, &active, QueuedKind::Flee, tutorial_run),
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
            manifests()
                .into_iter()
                .find(|d| d.key == kind.as_str())
                .map(|d| (format!("{}  x{stacks}", d.name), d.key.to_string()))
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
pub(crate) fn page_len(
    menu: &BattleMenu,
    class: &str,
    hero_level: i32,
    held: &[(String, i32)],
    spent: &[String],
    foci: &[String],
) -> usize {
    match menu.level {
        MenuLevel::Target | MenuLevel::Revoke => menu.rows.len() + 1,
        // Row COUNT doesn't depend on affordability (a disabled row is still
        // counted, just inert), so an empty roster/zero adrenaline is fine here.
        level => menu_entries(level, class, hero_level, held, spent, foci, &PartyRoster::default(), 0)
            .len(),
    }
}

/// The Focus kinds a hero is holding right now, off its `focus:<kind>:<stacks>` tokens.
/// An ASPECT is only offered under a parent that is actually held, so both the row list
/// and the row COUNT (cursor wrapping) have to ask the same question.
pub(crate) fn held_foci(battle: &BattleData, hero: &str) -> Vec<String> {
    battle
        .view(hero)
        .map(|v| parse_foci(&v.statuses).1.into_iter().map(|(k, _)| k).collect())
        .unwrap_or_default()
}

/// The potions HERO `slot` is carrying — what the battle Items page may offer.
///
/// Reads that hero's pouch, never the Party Inventory: in a fight a hero can only
/// reach its own kit, so offering the party's stock here would show rows the server
/// then refuses. Who carries the heals is decided on the overworld.
pub(crate) fn held_potions(backpack: &RunBackpack, slot: usize) -> Vec<(String, i32)> {
    backpack
        .pouch(slot)
        .iter()
        .filter(|(kind, qty)| *qty > 0 && meld_proto::consumables::is_consumable(kind))
        .cloned()
        .collect()
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
    tactics: Res<Tactics>,
    backpack: Res<RunBackpack>,
    report: Res<LootReport>,
    roster: Res<crate::PartyRoster>,
    mut menu: ResMut<BattleMenu>,
    mut battle: ResMut<BattleData>,
    mut tutorial_run: ResMut<TutorialRun>,
) {
    // The fight is already won/lost/fled — the victory/loot tally is up, so no
    // keyboard shortcut should be able to queue another action behind it (see
    // the matching `show` gate in `rebuild_command_menu`).
    if report.active {
        return;
    }
    // The Items page offers only what the party is carrying (GR-4).
    let held = held_potions(&backpack, battle.active_slot());
    // The command menu keys off the *active hero's* class — a mixed party is
    // commanded hero by hero.
    let class = battle.active_class();
    // Tactics (spec §6): available while an Phoenix Guard anchors the battle line;
    // when toggled on it drives the same per-class defaults as `?autoplay`,
    // submitting intents with no human reaction delay.
    if autoplay.0 || (tactics.0 && battle_has_phoenix_guard(&battle)) {
        let idle: Vec<String> = battle
            .your_ids
            .iter()
            .filter(|h| battle.alive(h) && !battle.queued.contains_key(*h))
            .cloned()
            .collect();
        for h in idle {
            // Each hero autoplays by its own class: Psyker channels Foci, Resonant
            // mends the party, everyone else swings — each at a sensible default target.
            let hc = battle.view(&h).map(hero_class).unwrap_or_else(|| "explorer".into());
            let kind = match hc.as_str() {
                "psyker" => battle.view(&h).map(psyker_autoplay_op).unwrap_or(QueuedKind::Hold),
                "resonant" => resonant_autoplay_op(&battle),
                "shifter" => battle.view(&h).map(shifter_autoplay_op).unwrap_or(QueuedKind::Attack),
                "explorer" => battle.view(&h).map(explorer_autoplay_op).unwrap_or(QueuedKind::Attack),
                "phoenix_guard" => battle.view(&h).map(phoenix_guard_autoplay_op).unwrap_or(QueuedKind::Attack),
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
            select_entry(i, &mut menu, &mut battle, &class, &held, &roster, &mut tutorial_run);
        }
        return;
    }

    // Sub-page (or a Psyker's list root): ↑/↓ move the highlight, digits jump to a
    // row, ENTER/SPACE selects.
    let hero_level = battle.active_level();
    let spent = battle.active.clone().map(|a| spent_tokens(&battle, &a)).unwrap_or_default();
    let foci = battle.active.clone().map(|a| held_foci(&battle, &a)).unwrap_or_default();
    let n = page_len(&menu, &class, hero_level, &held, &spent, &foci).max(1);
    if keys.just_pressed(KeyCode::ArrowDown) {
        menu.cursor = (menu.cursor + 1) % n;
    }
    if keys.just_pressed(KeyCode::ArrowUp) {
        menu.cursor = (menu.cursor + n - 1) % n;
    }
    for (i, key) in digits.iter().enumerate() {
        if i < n && keys.just_pressed(*key) {
            menu.cursor = i;
            select_entry(i, &mut menu, &mut battle, &class, &held, &roster, &mut tutorial_run);
            return;
        }
    }
    if keys.just_pressed(KeyCode::Enter) || keys.just_pressed(KeyCode::Space) {
        select_entry(menu.cursor, &mut menu, &mut battle, &class, &held, &roster, &mut tutorial_run);
    }
}

/// Mouse/touch: pressing a command row queues it for the active hero.
pub(crate) fn menu_click(
    backpack: Res<RunBackpack>,
    roster: Res<crate::PartyRoster>,
    mut menu: ResMut<BattleMenu>,
    mut battle: ResMut<BattleData>,
    rows: Query<(&Interaction, &MenuRow), Changed<Interaction>>,
    mut tutorial_run: ResMut<TutorialRun>,
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
        let held = held_potions(&backpack, battle.active_slot());
        select_entry(index, &mut menu, &mut battle, &class, &held, &roster, &mut tutorial_run);
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
                border_radius: BorderRadius::all(Val::Px(8.0)),
                width: Val::Px(w),
                height: Val::Px(46.0),
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                border: UiRect::all(Val::Px(1.0)),
                ..default()
            },
            BorderColor::all(border),
            BackgroundColor(glass::GLASS_THIN),
        ))
        .with_children(|t| {
            t.spawn((
                Text::new(label.to_string()),
                TextFont {
                    font_size: FontSize::Px(15.0),
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
    tactics: Res<Tactics>,
    backpack: Res<RunBackpack>,
    report: Res<LootReport>,
    mut menu: ResMut<BattleMenu>,
    roster: Res<crate::PartyRoster>,
    existing: Query<Entity, With<CommandWindow>>,
    tutorial_run: Res<TutorialRun>,
) {
    // The fight is over the moment the loot report is up (victory/chest tally) —
    // hidden here rather than left to decay naturally, since `battle.active` isn't
    // cleared until the NEXT battle starts and would otherwise keep Attack/Flee
    // live and clickable on top of the summary.
    let show = battle.active.is_some() && !report.active;
    let level = menu.level;
    let active_id = battle.active.clone().unwrap_or_default();
    // Include the dynamic row count so re-opening a Target page (same level) rebuilds,
    // and the Tactics state so the tap toggle's label refreshes when it flips.
    let sig = format!(
        "{show}|{active_id}|{level:?}|{}|{}|{:?}|{:?}",
        menu.rows.len(),
        tactics.0,
        tutorial_run.step,
        tutorial_run.battle_intro
    );
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
    let spent = spent_tokens(&battle, &active_id);
    let foci = held_foci(&battle, &active_id);
    let commanding = battle.hero_label(&active_id);
    let can_switch = next_commandable(&battle).is_some();
    // Hunter only: current banked Adrenaline, checked against `roster`'s per-skill
    // costs so a row the hero can't currently afford greys out (see `menu_entries`).
    let active_adrenaline =
        battle.view(&active_id).map(|v| status_num(&v.statuses, "adrenaline:")).unwrap_or(0);

    // Palette for the d-pad tiles: neutral for Item/Defend/Skill, gold for the
    // primary Attack, red for Flee — so the two "big" choices read at a glance.
    let neutral_edge = glass::EDGE_SOFT;
    let neutral_text = Color::srgb(0.92, 0.94, 1.0);
    let gold = Color::srgb(1.0, 0.85, 0.45);
    let red = Color::srgb(1.0, 0.55, 0.5);
    // The guided [T]-dive walkthrough's paced command-menu explainer: brighten
    // whichever tile is currently being explained, in place, rather than an
    // overlay box — same idiom as everywhere else in this feature.
    let intro_edge = |step: BattleIntroStep, base: Color| {
        if tutorial_run.battle_intro == Some(step) { glass::ACTIVE_EDGE } else { base }
    };
    let attack_edge = intro_edge(BattleIntroStep::Attack, gold);
    let defend_edge = intro_edge(BattleIntroStep::Defend, neutral_edge);
    let skill_edge = intro_edge(BattleIntroStep::Skill, neutral_edge);
    let flee_edge = intro_edge(BattleIntroStep::Flee, red);

    // Row label + enabled state + Adrenaline cost (if any) for the list renderer:
    // the dynamic Target/Revoke pages draw from `menu.rows` (+ a Back row, always
    // enabled, never costed); every other page comes from `menu_entries`, whose
    // `enabled` says whether picking this row would only get the hero refused (see
    // `select_entry`'s matching guard) and whose `adrenaline_cost` — when > 0 — is
    // shown as a right-aligned "N AP" badge so the cost to build toward reads at a
    // glance instead of only living in the tooltip below.
    let rows: Vec<(String, bool, Option<i32>)> = match level {
        MenuLevel::Target | MenuLevel::Revoke => menu
            .rows
            .iter()
            .map(|(l, _)| l.clone())
            .chain(std::iter::once("Back".to_string()))
            .map(|l| (l, true, None))
            .collect(),
        _ => menu_entries(
            level,
            &class,
            hero_level,
            &held_potions(&backpack, battle.active_slot()),
            &spent,
            &foci,
            &roster,
            active_adrenaline,
        )
            .into_iter()
            .map(|e| (e.label, e.enabled, e.adrenaline_cost.filter(|c| *c > 0)))
            .collect(),
    };
    // The selected row's tooltip. An ability nobody can read is an ability nobody
    // presses, so the description rides under the list — from the shared registry,
    // which is also what the server gates on.
    // The registry's prose, then the numbers the server resolved from balance. "Spends
    // Adrenaline" is not a decision; "40 of 100 Adrenaline (25 per Attack)" is.
    let (tooltip, magnitudes): (String, String) = match level {
        MenuLevel::Target | MenuLevel::Revoke => (String::new(), String::new()),
        _ => menu_entries(
            level,
            &class,
            hero_level,
            &held_potions(&backpack, battle.active_slot()),
            &spent,
            &foci,
            &roster,
            active_adrenaline,
        )
            .get(menu.cursor)
            .map(|e| {
                let key = e.action.skill_key().unwrap_or_default();
                (e.tooltip.clone(), roster.effect(key).to_string())
            })
            .unwrap_or_default(),
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
                    border_radius: BorderRadius::all(Val::Px(12.0)),
                    flex_direction: FlexDirection::Column,
                    align_items: AlignItems::Center,
                    row_gap: Val::Px(6.0),
                    padding: UiRect::axes(Val::Px(14.0), Val::Px(10.0)),
                    border: UiRect::all(Val::Px(1.0)),
                    ..default()
                },
                BorderColor::all(glass::EDGE_SOFT),
                BackgroundColor(glass::GLASS_THIN),
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
                            TextFont { font_size: FontSize::Px(15.0), ..default() },
                            TextColor(gold),
                        ));
                        if can_switch {
                            h.spawn((
                                Text::new("[Tab] switch".to_string()),
                                TextFont { font_size: FontSize::Px(12.0), ..default() },
                                TextColor(Color::srgb(0.6, 0.66, 0.8)),
                            ));
                        }
                    });

                // A Psyker's held Foci, spelled out. The party HUD shows them as
                // two-letter tags in slot order, which is fine once you know the
                // class and unreadable before then — this is the hero you are
                // actually commanding, so it gets full names, stack counts, and what
                // each one is doing every turn.
                if is_psyker {
                    let held: Vec<(String, u8)> = battle
                        .view(&active_id)
                        .map(|v| parse_foci(&v.statuses).1)
                        .unwrap_or_default();
                    let line = if held.is_empty() {
                        "Holding nothing — a Focus fires every turn you keep it".to_string()
                    } else {
                        held.iter()
                            .map(|(k, st)| {
                                let name = meld_proto::skills::pretty_skill(k);
                                if *st > 1 {
                                    format!("{name} x{st}")
                                } else {
                                    name
                                }
                            })
                            .collect::<Vec<_>>()
                            .join(" · ")
                    };
                    panel.spawn((
                        Text::new(line),
                        TextFont { font_size: FontSize::Px(12.0), ..default() },
                        TextColor(Color::srgb(0.78, 0.62, 1.0)),
                    ));
                    if !held.is_empty() {
                        panel.spawn((
                            Text::new("each fires again every turn it is held".to_string()),
                            TextFont { font_size: FontSize::Px(10.0), ..default() },
                            TextColor(glass::DIM),
                        ));
                    }
                }
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
                                .with_children(|r| cmd_tile(r, 3, "\u{f0068} Skill", 92.0, skill_edge, neutral_text));
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
                                    cmd_tile(r, 0, "\u{f04e5} Attack", 92.0, attack_edge, gold);
                                    cmd_tile(r, 1, "\u{f132} Defend", 92.0, defend_edge, neutral_text);
                                });
                            cross
                                .spawn(Node {
                                    flex_direction: FlexDirection::Row,
                                    ..default()
                                })
                                .with_children(|r| cmd_tile(r, 4, "\u{f070e} Flee", 92.0, flee_edge, red));
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
                                TextFont { font_size: FontSize::Px(13.0), ..default() },
                                TextColor(Color::srgb(0.95, 0.85, 0.5)),
                                Node {
                                    margin: UiRect::bottom(Val::Px(4.0)),
                                    ..default()
                                },
                            ));
                            for (i, (label, row_enabled, cost)) in rows.iter().enumerate() {
                                // Greyed out AND non-interactive: a skill the hero can't
                                // currently afford (or a spent once-per-battle call) has
                                // no `Button`, so it never receives `Interaction` at
                                // all — clicking it is structurally impossible, and
                                // `select_entry` refuses it too (keyboard Enter/digits).
                                let row_enabled = *row_enabled;
                                let text_color =
                                    if row_enabled { Color::srgb(0.9, 0.93, 1.0) } else { glass::DIM };
                                let mut row = list.spawn((
                                    MenuRow { index: i },
                                    Node {
                                        border_radius: BorderRadius::all(Val::Px(3.0)),
                                        width: Val::Percent(100.0),
                                        flex_direction: FlexDirection::Row,
                                        align_items: AlignItems::Center,
                                        justify_content: JustifyContent::SpaceBetween,
                                        padding: UiRect::axes(Val::Px(8.0), Val::Px(4.0)),
                                        ..default()
                                    },
                                    BackgroundColor(Color::NONE),
                                ));
                                if row_enabled {
                                    row.insert(Button);
                                }
                                row.with_children(|r| {
                                    r.spawn((
                                        Text::new(label.clone()),
                                        TextFont { font_size: FontSize::Px(18.0), ..default() },
                                        TextColor(text_color),
                                    ));
                                    // The Adrenaline cost, right-aligned by `SpaceBetween`
                                    // (the row's only other child) — "N AP" so the amount
                                    // to build toward reads without opening the tooltip.
                                    if let Some(cost) = cost {
                                        r.spawn((
                                            Text::new(format!("{cost} AP")),
                                            TextFont { font_size: FontSize::Px(14.0), ..default() },
                                            TextColor(if row_enabled { glass::DIM } else { text_color }),
                                        ));
                                    }
                                });
                            }
                            if !tooltip.is_empty() {
                                list.spawn((
                                    Text::new(tooltip.clone()),
                                    TextFont { font_size: FontSize::Px(12.0), ..default() },
                                    TextColor(glass::DIM),
                                    Node {
                                        margin: UiRect::top(Val::Px(6.0)),
                                        max_width: Val::Px(230.0),
                                        ..default()
                                    },
                                ));
                            }
                            // The magnitudes get their own gold line: run into the prose
                            // at the same weight they read as one long sentence, and the
                            // number — the part that decides between two rows — is what
                            // gets skipped.
                            if !magnitudes.is_empty() {
                                list.spawn((
                                    Text::new(magnitudes.clone()),
                                    TextFont { font_size: FontSize::Px(12.0), ..default() },
                                    TextColor(glass::TITLE),
                                    Node {
                                        margin: UiRect::top(Val::Px(3.0)),
                                        max_width: Val::Px(230.0),
                                        ..default()
                                    },
                                ));
                            }
                        });
                }
                // Phoenix Guard stance toggle — the last keyboard-only battle control ([T]),
                // now also a tap button so battle is fully click/tap driven. Shown only
                // when an Phoenix Guard anchors the line (mirrors `tactics_toggle`).
                if battle_has_phoenix_guard(&battle) {
                    let (label, col) = if tactics.0 {
                        ("\u{f132} TACTICS: ON  [T]", Color::srgb(0.55, 0.95, 0.65))
                    } else {
                        ("\u{f132} TACTICS: OFF  [T]", Color::srgb(0.75, 0.8, 0.95))
                    };
                    panel
                        .spawn((
                            Button,
                            TacticsButton,
                            Node {
                                border_radius: BorderRadius::all(Val::Px(6.0)),
                                margin: UiRect::top(Val::Px(6.0)),
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
            });
        });
}

/// Marks the tutorial's paced command-menu explainer card (Attack → Defend →
/// Skill → Flee, one at a time, on the tutorial's first fight).
#[derive(Component)]
pub(crate) struct BattleIntroRoot;
#[derive(Component)]
pub(crate) struct BattleIntroNextBtn;
#[derive(Component)]
pub(crate) struct BattleIntroSkipBtn;

const BATTLE_INTRO_SEQUENCE: [BattleIntroStep; 4] = [
    BattleIntroStep::Attack,
    BattleIntroStep::Defend,
    BattleIntroStep::Skill,
    BattleIntroStep::Flee,
];

fn battle_intro_text(step: BattleIntroStep) -> &'static str {
    match step {
        BattleIntroStep::Attack => "Attack — always available, no cost. Your basic hit.",
        BattleIntroStep::Defend => "Defend — braces for the next hit, cutting the damage you take.",
        BattleIntroStep::Skill => "Skill — spends your class's own resource for a stronger move.",
        BattleIntroStep::Flee => "Flee — pulls your whole party out of the fight together.",
    }
}

/// The first tutorial battle's paced, one-at-a-time walkthrough of the command
/// menu. A small card only — no scrim, so the tile `rebuild_command_menu`
/// highlights (per `TutorialRun.battle_intro`) stays visible underneath.
pub(crate) fn battle_intro_card(
    mut commands: Commands,
    tutorial_run: Res<TutorialRun>,
    root_q: Query<Entity, With<BattleIntroRoot>>,
) {
    let Some(step) = tutorial_run.battle_intro else {
        for e in &root_q {
            commands.entity(e).despawn();
        }
        return;
    };
    for e in &root_q {
        commands.entity(e).despawn();
    }
    let is_last = step == BattleIntroStep::Flee;
    commands
        .spawn((
            BattleIntroRoot,
            GlobalZIndex(900),
            Node {
                position_type: PositionType::Absolute,
                top: Val::Px(12.0),
                left: Val::Px(0.0),
                right: Val::Px(0.0),
                align_items: AlignItems::Center,
                justify_content: JustifyContent::Center,
                ..default()
            },
        ))
        .with_children(|root| {
            root.spawn(glass::panel_capped(Val::Percent(70.0), Val::Px(420.0)))
                .with_children(|p| {
                    p.spawn(glass::text("Guided Dive", 12.0, glass::DIM));
                    p.spawn(glass::text(battle_intro_text(step), 15.0, glass::TEXT));
                    p.spawn(Node {
                        flex_direction: FlexDirection::Row,
                        column_gap: Val::Px(12.0),
                        justify_content: JustifyContent::FlexEnd,
                        margin: UiRect::top(Val::Px(6.0)),
                        ..default()
                    })
                    .with_children(|row| {
                        row.spawn((Button, BattleIntroSkipBtn, glass::chip(false)))
                            .with_children(|b| {
                                b.spawn(glass::text("Skip", 14.0, glass::TEXT));
                            });
                        row.spawn((Button, BattleIntroNextBtn, glass::chip(true)))
                            .with_children(|b| {
                                b.spawn(glass::text(
                                    if is_last { "Got it" } else { "Next" },
                                    14.0,
                                    glass::TITLE,
                                ));
                            });
                    });
                });
        });
}

fn battle_intro_advance(tutorial_run: &mut TutorialRun) {
    let Some(step) = tutorial_run.battle_intro else { return };
    let next = BATTLE_INTRO_SEQUENCE.iter().position(|s| *s == step).map(|i| i + 1);
    tutorial_run.battle_intro = next.and_then(|i| BATTLE_INTRO_SEQUENCE.get(i).copied());
}

/// Next/Skip clicks.
pub(crate) fn battle_intro_buttons(
    mut tutorial_run: ResMut<TutorialRun>,
    next_q: Query<&Interaction, (With<BattleIntroNextBtn>, Changed<Interaction>)>,
    skip_q: Query<&Interaction, (With<BattleIntroSkipBtn>, Changed<Interaction>)>,
) {
    if skip_q.iter().any(|i| *i == Interaction::Pressed) {
        tutorial_run.battle_intro = None;
        return;
    }
    if next_q.iter().any(|i| *i == Interaction::Pressed) {
        battle_intro_advance(&mut tutorial_run);
    }
}

/// Keyboard twin: Enter/Space advances (or finishes, on the last step),
/// Escape skips straight to normal battle input.
pub(crate) fn battle_intro_keyboard(
    keys: Res<ButtonInput<KeyCode>>,
    mut tutorial_run: ResMut<TutorialRun>,
) {
    if tutorial_run.battle_intro.is_none() {
        return;
    }
    if keys.just_pressed(KeyCode::Enter) || keys.just_pressed(KeyCode::Space) {
        battle_intro_advance(&mut tutorial_run);
    } else if keys.just_pressed(KeyCode::Escape) {
        tutorial_run.battle_intro = None;
    }
}

/// Toggle the Phoenix Guard Tactics stance from its tap button (the keyboard [T] path
/// is `tactics_toggle`). Marks the command menu dirty so the label rebuilds.
pub(crate) fn tactics_click(
    q: Query<&Interaction, (With<TacticsButton>, Changed<Interaction>)>,
    mut tactics: ResMut<Tactics>,
    mut menu: ResMut<BattleMenu>,
) {
    for interaction in &q {
        if *interaction == Interaction::Pressed {
            tactics.0 = !tactics.0;
            menu.dirty = true;
        }
    }
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
        glass::GLASS_THIN
    } else {
        Color::NONE
    };
    for (row, interaction, mut bg) in &mut rows {
        let selected = row.index == menu.cursor;
        *bg = BackgroundColor(if *interaction == Interaction::Pressed || selected {
            glass::ACTIVE // translucent gold selection
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
                border_radius: BorderRadius::all(Val::Px(3.0)),
                width: Val::Percent(100.0),
                height: Val::Px(height),
                border: UiRect::all(Val::Px(1.0)),
                overflow: Overflow::clip(), // keep the rounded fill inside the track
                ..default()
            },
            BorderColor::all(Color::srgb(0.35, 0.4, 0.55)),
            BackgroundColor(Color::srgb(0.07, 0.08, 0.12)),
        ))
        .with_children(|t| {
            t.spawn((
                Node {
                    border_radius: BorderRadius::all(Val::Px(2.0)),
                    width: Val::Percent((frac * 100.0).clamp(0.0, 100.0)),
                    height: Val::Percent(100.0),
                    flex_direction: FlexDirection::Column,
                    ..default()
                },
                BackgroundColor(fill),
            ))
            .with_children(|f| {
                // Top sheen: a lighter band across the upper half → a rounded highlight.
                f.spawn((
                    Node {
                        border_radius: BorderRadius::all(Val::Px(2.0)),
                        width: Val::Percent(100.0),
                        height: Val::Percent(45.0),
                        ..default()
                    },
                    BackgroundColor(lighten(fill, 1.45)),
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
                border_radius: BorderRadius::all(Val::Px(3.0)),
                width: Val::Percent(100.0),
                height: Val::Px(height),
                border: UiRect::all(Val::Px(1.0)),
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                overflow: Overflow::clip(),
                ..default()
            },
            BorderColor::all(Color::srgb(0.35, 0.4, 0.55)),
            BackgroundColor(Color::srgb(0.07, 0.08, 0.12)),
        ))
        .with_children(|t| {
            // Fill: absolute so the centred label stays centred over the whole track.
            t.spawn((
                Node {
                    border_radius: BorderRadius::all(Val::Px(2.0)),
                    position_type: PositionType::Absolute,
                    left: Val::Px(0.0),
                    top: Val::Px(0.0),
                    bottom: Val::Px(0.0),
                    width: Val::Percent((frac * 100.0).clamp(0.0, 100.0)),
                    ..default()
                },
                BackgroundColor(fill),
            ));
            // Label on top (later sibling renders above the fill).
            t.spawn((
                Text::new(label),
                TextFont { font_size: FontSize::Px((height - 3.0).max(10.0)), ..default() },
                TextColor(Color::srgb(0.97, 0.99, 1.0)),
            ));
        });
}

/// True if a combatant was hit within the last [`BattleFeel::flash_ttl`] seconds.
pub(crate) fn flashing(hitfx: &HitFx, feel: &BattleFeel, id: &str) -> bool {
    hitfx.items.iter().any(|h| h.target == id && h.age < feel.flash_ttl)
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
/// Marker for the spectator banner's root.
#[derive(Component)]
pub(crate) struct WatchBanner;

/// Look away from a watched fight (`SOC-3`). [V] toggles (the same key that opened it) and
/// [Esc] backs out, because backing out of a screen is [Esc] everywhere else in this
/// client.
///
/// The client never leaves optimistically: it asks, and the server's `battle.watch_ended`
/// is what takes the screen down. A stop the server refused would otherwise strand a
/// half-left battle screen with a live feed still pouring into it.
pub(crate) fn watch_keyboard(
    keys: Res<ButtonInput<KeyCode>>,
    battle: Res<BattleData>,
    net: NonSend<crate::NetRes>,
) {
    if !battle.spectating {
        return;
    }
    if keys.just_pressed(KeyCode::KeyV) || keys.just_pressed(KeyCode::Escape) {
        net.0.send(crate::net::ClientCmd::StopWatching);
    }
}

/// The banner over a WATCHED fight (`SOC-3`), and the key that leaves it.
///
/// Everything else on the battle screen already degrades correctly for a watcher — the
/// command menu, the party strip and the hero keys all key off `your_ids`, which is empty
/// — so what is missing is not a suppression but a STATEMENT: without it a spectator sees
/// a fight, no menu, and no way out, which reads as the game having hung. Immediate-mode:
/// a pure display with nothing to preserve across frames.
pub(crate) fn render_watch_banner(
    mut commands: Commands,
    battle: Res<BattleData>,
    existing: Query<Entity, With<WatchBanner>>,
) {
    for e in &existing {
        commands.entity(e).despawn();
    }
    if !battle.spectating {
        return;
    }
    commands
        .spawn((
            WatchBanner,
            Node {
                position_type: PositionType::Absolute,
                width: Val::Percent(100.0),
                top: Val::Px(16.0),
                align_items: AlignItems::Center,
                justify_content: JustifyContent::Center,
                flex_direction: FlexDirection::Column,
                row_gap: Val::Px(3.0),
                ..default()
            },
        ))
        .with_children(|root| {
            root.spawn((
                Node {
                    border_radius: BorderRadius::all(Val::Px(8.0)),
                    flex_direction: FlexDirection::Column,
                    align_items: AlignItems::Center,
                    row_gap: Val::Px(2.0),
                    padding: UiRect::axes(Val::Px(18.0), Val::Px(7.0)),
                    border: UiRect::all(Val::Px(1.0)),
                    ..default()
                },
                BackgroundColor(glass::GLASS_THIN),
                BorderColor::all(glass::EDGE_SOFT),
            ))
            .with_children(|p| {
                p.spawn(glass::text("WATCHING".to_string(), 20.0, glass::DIM));
                p.spawn(glass::text(
                    "not your fight - [V] or [Esc] to look away".to_string(),
                    14.0,
                    glass::DIM,
                ));
            });
        });
}

pub(crate) fn render_enemy_panel(
    mut commands: Commands,
    battle: Res<BattleData>,
    hitfx: Res<HitFx>,
    feel: Res<BattleFeel>,
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
                let hurt = flashing(&hitfx, &feel, &c.id);
                let is_target = focus.as_deref() == Some(c.id.as_str());
                let faction = c.statuses.iter().find_map(|s| s.strip_prefix("faction:"));
                let hp_fill = if hurt {
                    Color::srgb(1.0, 0.95, 0.95)
                } else if let Some((tint, _)) = condition_tint(&c.statuses) {
                    // A creature under a condition says so on its own bar — the mustard on a
                    // blazed target is the party's cue that this is the one to hit.
                    tint
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
                        Text::new(format!(
                            "{}  {}/{}",
                            pack_label(&c.name, &c.statuses),
                            c.hp,
                            c.max_hp
                        )),
                        TextFont { font_size: FontSize::Px(14.0), ..default() },
                        TextColor(name_color),
                    ));
                    meter(e, frac, 10.0, hp_fill);
                    // Hunter "Predator's Eye" top tier (`hunter_intel_atb_at`): reveal the
                    // enemy's ATB gauge — otherwise you read foe HP and guess at its turn.
                    // The perk moved to the Hunter with CL-2; the label said Explorer.
                    if perks.0.hunter_intel >= 3 {
                        meter(e, c.gauge as f32, 5.0, Color::srgb(0.5, 0.72, 1.0));
                    }
                });
            }
        });
}

/// Full-screen, non-interactive layer holding the floating status-effect icons that
/// hover over each afflicted combatant. Rebuilt every frame like [`render_enemy_panel`].
#[derive(Component)]
pub(crate) struct StatusIconLayer;

/// The `spent:<skill>` tokens a combatant carries — which once-per-battle calls it has
/// already made this fight.
pub(crate) fn spent_tokens(battle: &BattleData, id: &str) -> Vec<String> {
    battle
        .view(id)
        .map(|c| {
            c.statuses.iter().filter_map(|s| s.strip_prefix("spent:").map(String::from)).collect()
        })
        .unwrap_or_default()
}

/// The visible status effects on a combatant, in a stable display order, each as
/// (nerdfont glyph, colour, label). Class resources (adrenaline / focus) and the
/// row/faction/class/attribute tokens are intentionally excluded — those show in the
/// HUD cells, not as an aura over the sprite.
/// What a condition looks like. The palette is deliberately NAMED after things rather than
/// built out of saturated primaries — mustard, blue sage, rosemary, steel — so several tints
/// can sit on one screen without any of them shouting.
///
/// The pattern is the split: **afflictions** are warm-to-sour (poison purple, marked mustard,
/// rage red) with slow the one cool exception, and **boons** are the cool herb/metal end
/// (barrier steel blue, regen rosemary). So a glance at a hero's cell says "something is being
/// done TO them" or "something is helping them" before you read a single word.
pub(crate) fn condition_tint(statuses: &[String]) -> Option<(Color, &'static str)> {
    let has = |n: &str| statuses.iter().any(|s| s == n);
    let num = |p: &str| {
        statuses.iter().find_map(|s| s.strip_prefix(p).and_then(|n| n.parse::<i32>().ok()))
    };
    // Afflictions first: what is being done to you outranks what is helping you.
    if has("poison") || has("burn") {
        return Some((Color::srgb(0.42, 0.22, 0.55), "poison")); // purple
    }
    if has("marked") {
        return Some((Color::srgb(0.72, 0.58, 0.16), "marked")); // mustard
    }
    if is_slowed(statuses) {
        return Some((Color::srgb(0.36, 0.48, 0.52), "slow")); // blue sage
    }
    if has("rage") || has("frenzied") {
        return Some((Color::srgb(0.62, 0.18, 0.16), "rage")); // red
    }
    // Then the boons.
    if num("barrier:").is_some_and(|n| n > 0) {
        return Some((Color::srgb(0.27, 0.42, 0.58), "barrier")); // steel blue
    }
    if num("regen:").is_some_and(|n| n > 0) {
        return Some((Color::srgb(0.35, 0.52, 0.35), "regen")); // rosemary green
    }
    None
}

/// A rim of the condition's colour around a combatant's own sprite, keyed by which
/// condition put it there so a change of state rebuilds it.
#[derive(Component)]
pub(crate) struct ConditionRim(&'static str);

/// Paint the condition onto the FIGHTER, not just its readout.
///
/// The cell and the nameplate already take the tint, and in play that was missed entirely
/// — the eye is on the arena, not on the party strip. So the sprite wears its own
/// condition: a copy of it, a little larger and a hair behind, in the condition's colour.
/// Same trick as the overworld's reach rim, and note the same trap — emissive on a
/// TEXTURED billboard floods the whole quad, so this is an unlit alpha-blended overlay
/// with depth-write off rather than anything glowing.
pub(crate) fn update_condition_rims(
    mut commands: Commands,
    time: Res<Time>,
    battle: Res<BattleData>,
    wa: Option<Res<WorldAssets>>,
    mut mats: ResMut<Assets<StandardMaterial>>,
    quads: Query<(Entity, &SpriteQuad, Option<&Children>)>,
    rims: Query<(&ConditionRim, &MeshMaterial3d<StandardMaterial>)>,
) {
    let Some(wa) = wa else { return };
    // Slower and gentler than the reach rim, and it never drops out: a condition is a
    // state you are IN, so it should sit there breathing rather than blink for attention.
    let phase = (time.elapsed_secs() * std::f32::consts::TAU / 2.6).sin();
    let alpha = 0.46 + 0.22 * phase;

    for (quad, sq, kids) in &quads {
        let want = battle
            .combatants
            .iter()
            .find(|c| c.id == sq.id)
            .filter(|c| c.hp > 0)
            .and_then(|c| condition_tint(&c.statuses));
        let mine: Vec<Entity> = kids
            .map(|k| k.iter().filter(|e| rims.get(*e).is_ok()).collect())
            .unwrap_or_default();
        match want {
            Some((colour, key)) => {
                let held = mine.iter().find(|e| {
                    rims.get(**e).is_ok_and(|(r, _)| r.0 == key)
                });
                if let Some(held) = held.copied() {
                    for e in mine.into_iter().filter(|e| *e != held) {
                        commands.entity(e).despawn();
                    }
                    if let Ok((_, mm)) = rims.get(held) {
                        if let Some(mut m) = mats.get_mut(&mm.0) {
                            m.base_color = colour.with_alpha(alpha);
                        }
                    }
                    continue;
                }
                for e in mine {
                    commands.entity(e).despawn();
                }
                let Some(tex) = mats.get(&sq.mat).and_then(|m| m.base_color_texture.clone()) else {
                    continue;
                };
                let rim = mats.add(StandardMaterial {
                    base_color: colour.with_alpha(alpha),
                    base_color_texture: Some(tex),
                    unlit: true,
                    alpha_mode: AlphaMode::Blend,
                    depth_bias: -1.0,
                    double_sided: true,
                    cull_mode: None,
                    ..default()
                });
                commands.entity(quad).with_children(|p| {
                    p.spawn((
                        ConditionRim(key),
                        Mesh3d(wa.sprite_quad.clone()),
                        MeshMaterial3d(rim),
                        // Local to the sprite quad, so it inherits the billboard's facing
                        // and size and only has to say "a bit bigger, a bit behind".
                        Transform::from_xyz(0.0, 0.0, -0.03).with_scale(Vec3::splat(1.10)),
                    ));
                });
            }
            None => {
                for e in mine {
                    commands.entity(e).despawn();
                }
            }
        }
    }
}

/// The statuses that actually drag the ATB gauge — the server's own list (`web`/`chill`/
/// `bind`), so the snail and the tint agree with what the engine is doing.
pub(crate) fn is_slowed(statuses: &[String]) -> bool {
    statuses.iter().any(|s| matches!(s.as_str(), "web" | "chill" | "bind"))
}

/// The ink width of each badge glyph, in px at the badge's 18px font size.
///
/// Needed because flex centring centres a glyph's ADVANCE box, and these are Nerd Font
/// icons patched into a monospace face: every one advances the same 10.80px cell while its
/// ink starts at x=0 and runs *past* the cell — the crosshair is 16.52px of ink in a 10.80px
/// advance. Centring the advance therefore leaves the ink sitting `(ink - advance)/2` to the
/// right, which is what reads as "the skull is off centre".
///
/// So the glyph is positioned absolutely from the badge's left edge instead of being flex
/// centred with a corrective margin: `left = badge/2 - ink/2` puts the INK's centre on the
/// badge's centre exactly, with no fractional margin for the layout to round away. Values
/// are read out of the font's `glyf` table.
fn status_icon_ink(glyph: &str) -> f32 {
    match glyph {
        "\u{f01a4}" => 16.52, // crosshairs
        "\u{f1677}" => 14.98, // snail
        "\u{f0208}" => 16.52, // eye
        "\u{f046e}" => 14.98, // run-fast
        "\u{f068c}" => 13.50, // skull
        "\u{f0498}" => 13.50, // shield
        "\u{f060c}" => 11.25, // lightning-bolt
        "\u{f0238}" => 10.85, // fire
        "\u{f05f5}" => 10.67, // heart-pulse
        _ => 10.80,            // the monospace advance: assume ink fills its cell
    }
}

/// The badge is this wide and tall, in logical px, with this much border. Both matter:
/// `position_type: Absolute` is resolved against the PADDING box, so `left` is measured
/// from inside the border — placing the ink by the outer width put every glyph a border's
/// width too far right, which a screenshot measurement caught after the maths said it
/// should be centred.
const STATUS_BADGE: f32 = 30.0;
const STATUS_BADGE_BORDER: f32 = 1.5;

/// Where a glyph's text node goes so its INK sits centred on the badge.
fn status_icon_left(glyph: &str) -> f32 {
    STATUS_BADGE / 2.0 - STATUS_BADGE_BORDER - status_icon_ink(glyph) / 2.0
}

fn status_effects(statuses: &[String]) -> Vec<(&'static str, Color, &'static str)> {
    let has = |name: &str| statuses.iter().any(|s| s == name);
    let num = |p: &str| {
        statuses
            .iter()
            .find_map(|s| s.strip_prefix(p).and_then(|n| n.parse::<i32>().ok()))
            .unwrap_or(0)
    };
    let mut v = Vec::new();
    // Debuffs first (the ones you most need to notice), then buffs.
    // A blazed target takes more from EVERY ally, so it has to be visible on the creature
    // — the whole value of the Explorer's opener is the party knowing where to swing.
    if has("marked") {
        v.push(("\u{f01a4}", Color::srgb(1.0, 0.82, 0.35), "Blazed")); // crosshairs
    }
    // A distracted creature is missing on purpose. If that is invisible the player reads a
    // string of misses as luck rather than as the Explorer's doing.
    if has("distracted") {
        v.push(("\u{f0208}", Color::srgb(0.85, 0.8, 1.0), "Distracted")); // eye
    }
    if has("hasted") {
        v.push(("\u{f060c}", Color::srgb(1.0, 0.9, 0.55), "Haste")); // lightning-bolt
    }
    // A creature that webbed or chilled you SLOWED you, and nothing said so — the gauge
    // just crawled. A snail is the one icon everybody already reads as "slowed".
    if is_slowed(statuses) {
        v.push(("\u{f1677}", Color::srgb(0.62, 0.76, 0.8), "Slowed")); // snail
    }
    if has("poison") {
        v.push(("\u{f068c}", Color::srgb(0.58, 0.9, 0.4), "Poison")); // skull
    }
    if has("burn") {
        v.push(("\u{f0238}", Color::srgb(1.0, 0.55, 0.25), "Burn")); // fire
    }
    if num("barrier:") > 0 {
        v.push(("\u{f0498}", Color::srgb(0.45, 0.78, 1.0), "Barrier")); // shield
    }
    if num("regen:") > 0 {
        v.push(("\u{f05f5}", Color::srgb(0.5, 0.95, 0.6), "Regen")); // heart-pulse
    }
    if num("evasion:") > 0 {
        v.push(("\u{f046e}", Color::srgb(0.78, 0.9, 1.0), "Evasion")); // run-fast
    }
    v
}

/// A small nerdfont status icon floating over each combatant that carries an active
/// effect (poison / burn debuffs, barrier / regen / evasion buffs). When a combatant
/// carries several, the glyph cycles one-at-a-time on a 1.5 s timer so a single icon
/// always reads cleanly. Projected from the arena each frame like [`render_enemy_panel`],
/// so the effect reads on the creature itself.
pub(crate) fn render_status_icons(
    mut commands: Commands,
    battle: Res<BattleData>,
    time: Res<Time>,
    cam_q: Query<(&Camera, &GlobalTransform), With<Camera3d>>,
    actors: Query<(&BattleActor, &GlobalTransform)>,
    existing: Query<Entity, With<StatusIconLayer>>,
) {
    for e in &existing {
        commands.entity(e).despawn();
    }
    let Some((cam, cam_tf)) = cam_q.iter().next() else { return };
    // Which effect shows this instant when a combatant carries several (1.5 s each).
    let phase = (time.elapsed_secs() / 1.5) as usize;
    commands
        .spawn((
            StatusIconLayer,
            Node {
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                ..default()
            },
        ))
        .with_children(|p| {
            for c in battle.combatants.iter().filter(|c| c.hp > 0) {
                let effects = status_effects(&c.statuses);
                if effects.is_empty() {
                    continue;
                }
                let Some((_, gt)) = actors.iter().find(|(a, _)| a.id == c.id) else { continue };
                // Hover just over the sprite's head (~2.55 world units above its root).
                let Ok(head) = cam.world_to_viewport(cam_tf, gt.translation() + Vec3::Y * 2.55)
                else {
                    continue;
                };
                let (glyph, color, _label) = effects[phase % effects.len()];
                p.spawn((
                    Node {
                        border_radius: BorderRadius::all(Val::Px(15.0)),
                        position_type: PositionType::Absolute,
                        left: Val::Px(head.x - 15.0),
                        top: Val::Px(head.y - 30.0),
                        width: Val::Px(30.0),
                        height: Val::Px(30.0),
                        align_items: AlignItems::Center,
                        justify_content: JustifyContent::Center,
                        border: UiRect::all(Val::Px(1.5)),
                        ..default()
                    },
                    BorderColor::all(color),
                    BackgroundColor(glass::GLASS),
                ))
                .with_children(|b| {
                    b.spawn((
                        Text::new(glyph),
                        TextFont { font_size: FontSize::Px(18.0), ..default() },
                        TextColor(color),
                        // Placed, not centred. `left` puts the INK's centre on the badge's
                        // centre (see `status_icon_ink`); vertical needs nothing, because
                        // measured against the font every one of these glyphs already has
                        // its ink centred on the line box — the 2px of "optical centring"
                        // that once lived here was itself the thing pushing them off.
                        Node {
                            position_type: PositionType::Absolute,
                            left: Val::Px(status_icon_left(glyph)),
                            ..default()
                        },
                    ));
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
    feel: Res<BattleFeel>,
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
                border_radius: BorderRadius {
                                top_left: Val::Px(0.0),
                                top_right: Val::Px(0.0),
                                bottom_left: Val::Px(12.0),
                                bottom_right: Val::Px(12.0),
                            },
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
            BorderColor::all(glass::EDGE_SOFT),
            BackgroundColor(glass::GLASS_THIN),
            // Flat top edge (against the screen), rounded bottom.
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
                    TextFont { font_size: FontSize::Px(13.0), ..default() },
                    TextColor(Color::srgb(0.72, 0.85, 1.0)),
                ));
                hdr.spawn((
                    Button,
                    AllyCollapseBtn,
                    Node {
                        border_radius: BorderRadius::all(Val::Px(5.0)),
                        padding: UiRect::axes(Val::Px(7.0), Val::Px(1.0)),
                        border: UiRect::all(Val::Px(1.0)),
                        ..default()
                    },
                    BorderColor::all(glass::EDGE_SOFT),
                    BackgroundColor(glass::GLASS_THIN),
                ))
                .with_children(|b| {
                    b.spawn((
                        Text::new(if collapsed { "[+]" } else { "[-]" }),
                        TextFont { font_size: FontSize::Px(13.0), ..default() },
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
                            TextFont { font_size: FontSize::Px(12.0), ..default() },
                            TextColor(Color::srgb(0.7, 0.82, 1.0)),
                        ));
                        col.spawn(Node {
                            flex_direction: FlexDirection::Row,
                            column_gap: Val::Px(6.0),
                            ..default()
                        })
                        .with_children(|cells| {
                            for c in heroes.iter() {
                                ally_cell(cells, &hitfx, &feel, c);
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
pub(crate) fn ally_cell(
    parent: &mut ChildSpawnerCommands,
    hitfx: &HitFx,
    feel: &BattleFeel,
    c: &CombatantView,
) {
    let hp_frac = c.hp as f32 / c.max_hp.max(1) as f32;
    let gauge = c.gauge.clamp(0.0, 1.0) as f32;
    let hurt = flashing(hitfx, feel, &c.id);
    let name = if !c.name.is_empty() && c.name != "Hero" {
        c.name.clone()
    } else {
        "Hero".to_string()
    };
    parent
        .spawn((
            Node {
                border_radius: BorderRadius::all(Val::Px(7.0)),
                width: Val::Px(112.0),
                flex_direction: FlexDirection::Column,
                row_gap: Val::Px(2.0),
                padding: UiRect::all(Val::Px(4.0)),
                border: UiRect::all(Val::Px(1.0)),
                ..default()
            },
            BorderColor::all(glass::EDGE_SOFT),
            BackgroundColor(if hurt {
                Color::srgba(0.4, 0.12, 0.14, 0.5)
            } else {
                Color::srgba(0.09, 0.12, 0.22, 0.4)
            }),
        ))
        .with_children(|cell| {
            cell.spawn((
                Text::new(name),
                TextFont { font_size: FontSize::Px(13.0), ..default() },
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
    feel: &BattleFeel,
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
    let hurt = flashing(hitfx, feel, id);
    // "Turn's up" pop: 1.0 the instant the gauge fills, fading to 0 over the TTL.
    let atb_pop = flash
        .age
        .get(id)
        .map(|a| (1.0 - a / feel.atb_flash_ttl).clamp(0.0, 1.0))
        .unwrap_or(0.0);
    // Frosted glass: hairline edge normally, a brighter gold edge for the active /
    // target hero so it still stands out without a heavy border.
    let base_border = if is_target_cursor || active {
        glass::ACTIVE_EDGE
    } else {
        glass::EDGE_SOFT
    };
    parent
        .spawn((
            // The whole cell is a button: tap it to command that hero (see
            // `party_select_click`).
            Button,
            PartyCellButton { id: id.to_string() },
            Node {
                border_radius: BorderRadius::all(Val::Px(10.0)),
                flex_grow: 1.0,
                flex_basis: Val::Px(0.0),
                flex_direction: FlexDirection::Row,
                column_gap: Val::Px(8.0),
                padding: UiRect::all(Val::Px(7.0)),
                border: UiRect::all(Val::Px(1.0)),
                ..default()
            },
            // Flash the edge toward bright gold-white as the turn comes up.
            BorderColor::all(if atb_pop > 0.0 {
                lerp_color(base_border, Color::srgb(1.0, 0.98, 0.7), atb_pop)
            } else {
                base_border
            }),
            // A condition repaints the cell, so the HP/ATB readout itself carries the news.
            // Being hit still wins (it is the most urgent thing on screen), and so does the
            // active/target highlight — you must always be able to see whose turn it is.
            BackgroundColor(if hurt {
                Color::srgba(0.4, 0.12, 0.14, 0.55)
            } else if is_target_cursor {
                Color::srgba(0.28, 0.26, 0.1, 0.5)
            } else if active {
                glass::ACTIVE
            } else if let Some((tint, _)) = condition_tint(&c.statuses) {
                tint.with_alpha(0.5)
            } else {
                glass::GLASS_THIN
            }),
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
                        TextFont { font_size: FontSize::Px(16.0), ..default() },
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
                                TextFont { font_size: FontSize::Px(13.0), ..default() },
                                TextColor(tag_color),
                            ));
                        }
                        right.spawn((
                            Text::new(format!("Lv{}", c.level)),
                            TextFont { font_size: FontSize::Px(12.0), ..default() },
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
                                TextFont { font_size: FontSize::Px(12.0), ..default() },
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
    feel: Res<BattleFeel>,
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
                    Some(id) => party_cell(row, &battle, &hitfx, &feel, &menu, &flash, id, i),
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
pub(crate) fn advance_atb_flash(
    time: Res<Time>,
    battle: Res<BattleData>,
    feel: Res<BattleFeel>,
    mut flash: ResMut<AtbFlash>,
) {
    if battle_mockup_flag() {
        return;
    }
    let dt = time.delta_secs();
    // Age existing flashes and drop the expired.
    flash.age.retain(|_, a| {
        *a += dt;
        *a < feel.atb_flash_ttl
    });
    // Newly-ready heroes (weren't ready last frame) get a fresh flash.
    for id in battle.ready.iter() {
        if !flash.prev.contains(id) {
            flash.age.insert(id.clone(), 0.0);
        }
    }
    flash.prev = battle.ready.iter().cloned().collect();
}

/// Whether any allied hero in this battle is an Phoenix Guard (their wire statuses
/// carry `class:phoenix_guard`) — the gate for the Tactics auto-battle toggle.
pub(crate) fn battle_has_phoenix_guard(battle: &BattleData) -> bool {
    battle
        .combatants
        .iter()
        .any(|c| c.is_player && c.statuses.iter().any(|s| s == "class:phoenix_guard"))
}

/// Toggle Tactics with T on the battle screen (only while an Phoenix Guard is in
/// the battle — without one the toggle is inert and the hint hidden).
pub(crate) fn tactics_toggle(
    keys: Res<ButtonInput<KeyCode>>,
    battle: Res<BattleData>,
    mut tactics: ResMut<Tactics>,
) {
    if keys.just_pressed(KeyCode::KeyT) && battle_has_phoenix_guard(&battle) {
        tactics.0 = !tactics.0;
    }
}

/// Age floating hit numbers; drop the expired. Frozen in the static mockup so
/// the seeded feedback stays on screen.
pub(crate) fn advance_hit_fx(time: Res<Time>, feel: Res<BattleFeel>, mut hitfx: ResMut<HitFx>) {
    if battle_mockup_flag() {
        return;
    }
    let dt = time.delta_secs();
    for h in &mut hitfx.items {
        h.age += dt;
    }
    hitfx.items.retain(|h| h.age < feel.hit_ttl);
    for c in &mut hitfx.callouts {
        c.age += dt;
    }
    hitfx.callouts.retain(|c| c.age < c.ttl);
    hitfx.acts.retain(|_, a| {
        *a += dt;
        *a < feel.lunge_ttl
    });
}

/// Immediate-mode overlay: draw each floating number, rising and fading, anchored over
/// the combatant it landed on — its own sprite in the arena, projected the way
/// [`render_enemy_panel`] hangs the HP bars.
///
/// It used to anchor by *identity*: `monster_combatant` (which is only ever
/// `enemies.first()`) drew top-centre and everything else fell through a
/// `your_ids.position(…).unwrap_or(0)`. So every enemy past the first — and every joined
/// ally — printed its damage over hero slot 0's cell. Packs are standard now and an
/// all-enemy ability resolves four or five at once, so a Purging Light sprayed the whole
/// sweep onto the first hero. Anchoring to the actor removes the class of bug rather
/// than the instance.
pub(crate) fn render_hit_fx(
    mut commands: Commands,
    hitfx: Res<HitFx>,
    battle: Res<BattleData>,
    feel: Res<BattleFeel>,
    tactics: Res<Tactics>,
    windows: Query<&Window>,
    cam_q: Query<(&Camera, &GlobalTransform), With<Camera3d>>,
    actors: Query<(&BattleActor, &GlobalTransform)>,
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
            let cam = cam_q.iter().next();
            for hit in &hitfx.items {
                let over_actor = cam.and_then(|(cam, cam_tf)| {
                    let (_, gt) = actors.iter().find(|(a, _)| a.id == hit.target)?;
                    let head = gt.translation() + Vec3::Y * feel.number_height;
                    cam.world_to_viewport(cam_tf, head).ok()
                });
                // No actor yet (or it is behind the camera) — fall back to the hero's own
                // cell, and only for a hero we actually field. Printing someone else's
                // number on slot 0 is what this system is being fixed for.
                let Some((x, y0)) = over_actor.map(|p| (p.x - 16.0, p.y)).or_else(|| {
                    let idx = battle.your_ids.iter().position(|id| id == &hit.target)?;
                    Some(((idx as f32 + 0.5) / 4.0 * w, h - 150.0))
                }) else {
                    continue;
                };
                let rise = hit.age * feel.number_rise;
                let alpha = (1.0 - hit.age / feel.hit_ttl).clamp(0.0, 1.0);
                // WEAK! hits pop bigger and judder side-to-side (the spec's
                // screen-shaking flourish, scoped to the number itself).
                let shake = if hit.scale > 1.0 {
                    (hit.age * 60.0).sin() * feel.number_shake * alpha
                } else {
                    0.0
                };
                // Simultaneous hits on one target share an anchor exactly, so stack them
                // and alternate the side — an all-enemy sweep otherwise overstrikes into
                // an unreadable smear at a single point.
                let stack = hit.stack as f32 * feel.stack_step;
                let sway = if hit.stack % 2 == 1 { feel.stack_step } else { 0.0 };
                p.spawn((
                    Node {
                        position_type: PositionType::Absolute,
                        left: Val::Px(x + shake + sway),
                        top: Val::Px(y0 - rise - stack),
                        ..default()
                    },
                    Text::new(hit.text.clone()),
                    TextFont {
                        font_size: FontSize::Px(feel.number_size * hit.scale),
                        ..default()
                    },
                    TextColor(hit.color.with_alpha(alpha)),
                ));
            }

            // Monster ability shout bubbles (spec §3/§6): a channeling
            // telegraph flashes for its whole window; an instant callout fades.
            for (i, c) in hitfx.callouts.iter().enumerate() {
                let alpha = if c.flashing {
                    // Flash between bright and dim while channeling.
                    0.55 + 0.45 * (c.age * 9.0).sin().abs()
                } else {
                    (1.0 - c.age / c.ttl).clamp(0.0, 1.0)
                };
                // One row per speaker. Every bubble used to be pinned to the same spot,
                // so two monsters shouting at once drew on top of each other and neither
                // was readable — which is also why `combatant_id` was being recorded and
                // then ignored.
                p.spawn((
                    Node {
                        border_radius: BorderRadius::all(Val::Px(6.0)),
                        position_type: PositionType::Absolute,
                        left: Val::Px(w * 0.5 - 90.0),
                        top: Val::Px(h * 0.12 + i as f32 * 34.0),
                        padding: UiRect::axes(Val::Px(10.0), Val::Px(4.0)),
                        border: UiRect::all(Val::Px(2.0)),
                        ..default()
                    },
                    // The cue fades out, so the shared glass carries the fade.
                    BackgroundColor(glass::GLASS.with_alpha(glass::GLASS.alpha() * alpha)),
                    BorderColor::all(Color::srgba(1.0, 0.9, 0.4, alpha)),
                ))
                .with_children(|b| {
                    b.spawn((
                        Text::new(c.text.clone()),
                        TextFont { font_size: FontSize::Px(22.0), ..default() },
                        TextColor(Color::srgba(1.0, 0.92, 0.55, alpha)),
                    ));
                });
            }

            // Tactics status (spec §6): a passive top-right readout while an Phoenix Guard
            // anchors the line. Suppressed while a hero is being commanded, since the
            // command window then shows the interactive TACTICS toggle button instead.
            if battle_has_phoenix_guard(&battle) && battle.active.is_none() {
                let (label, col) = if tactics.0 {
                    ("TACTICS: ON  [T]", Color::srgb(0.55, 0.95, 0.65))
                } else {
                    ("TACTICS: OFF  [T]", Color::srgb(0.6, 0.65, 0.8))
                };
                p.spawn((
                    Node {
                        position_type: PositionType::Absolute,
                        right: Val::Px(14.0),
                        top: Val::Px(64.0),
                        ..default()
                    },
                    Text::new(label),
                    TextFont { font_size: FontSize::Px(14.0), ..default() },
                    TextColor(col),
                ));
            }
        });
}

/// Turn a resolved effect into a floating number (skips zero/no-op effects).
/// `show_elements` is the Hunter's threat-sense gate (spec §6): only a party whose
/// Hunter has unlocked it reads WEAK!/RESIST!/IMMUNE!/ABSORB! — everyone else sees
/// plain numbers (an immune hit still shows its 0). Reading what a creature is made of
/// is the same trade as reading its level and its gauge.
pub(crate) fn push_hit_fx(hitfx: &mut HitFx, e: &HitEffect, show_elements: bool) {
    let modifier = if show_elements { e.modifier.as_deref() } else { None };
    let mut scale = 1.0;
    let (text, color) = match e.kind.to_lowercase().as_str() {
        "damage" => {
            let n = e.amount.unwrap_or(0);
            match modifier {
                // Immunity shows the word instead of a number (spec §6).
                Some("immune") => ("IMMUNE!".to_string(), Color::srgb(0.75, 0.75, 0.8)),
                Some("weak") => {
                    scale = 1.45; // big, screen-shaking hit text
                    (format!("-{n}  WEAK!"), Color::srgb(1.0, 0.55, 0.15))
                }
                Some("resist") => (format!("-{n}  RESIST!"), Color::srgb(0.62, 0.62, 0.68)),
                _ => {
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
            }
        }
        "heal" => {
            let n = e.amount.unwrap_or(0);
            if n == 0 {
                return;
            }
            match modifier {
                // Absorption heals pop green with the flourish (spec §6).
                Some("absorb") => (format!("+{n}  ABSORB!"), Color::srgb(0.4, 1.0, 0.55)),
                _ => (format!("+{n}"), Color::srgb(0.5, 1.0, 0.6)),
            }
        }
        "ko" => ("KO!".to_string(), Color::srgb(1.0, 0.35, 0.35)),
        _ => return,
    };
    let stack = hitfx.items.iter().filter(|h| h.target == e.target).count().min(255) as u8;
    hitfx.items.push(Hit {
        target: e.target.clone(),
        text,
        color,
        age: 0.0,
        scale,
        stack,
    });
}

#[cfg(test)]
mod pack_tests {
    use super::*;

    /// The condition palette: afflictions read warm-to-sour, boons read cool herb/metal, and
    /// an affliction outranks a boon — "something is being done to me" is the more urgent news.
    #[test]
    fn conditions_repaint_the_cell_and_afflictions_win() {
        let t = |toks: &[&str]| {
            condition_tint(&toks.iter().map(|s| s.to_string()).collect::<Vec<_>>()).map(|(_, n)| n)
        };
        assert_eq!(t(&[]), None, "a clean hero keeps the neutral glass");
        assert_eq!(t(&["poison"]), Some("poison"));
        assert_eq!(t(&["marked"]), Some("marked"));
        assert_eq!(t(&["web"]), Some("slow"), "web/chill/bind all read as slow");
        assert_eq!(t(&["chill"]), Some("slow"));
        assert_eq!(t(&["bind"]), Some("slow"));
        assert_eq!(t(&["barrier:8"]), Some("barrier"));
        assert_eq!(t(&["regen:3"]), Some("regen"));
        // A poisoned hero who also has a Barrier still reads as poisoned.
        assert_eq!(t(&["barrier:8", "poison"]), Some("poison"));
        // And a zero-valued boon is not a boon.
        assert_eq!(t(&["barrier:0"]), None);
    }

    /// Anything the engine slows gets the snail, because the gauge crawling with no icon was
    /// indistinguishable from the gauge being slow.
    #[test]
    fn a_slowed_creature_wears_a_snail() {
        for tok in ["web", "chill", "bind"] {
            let icons = status_effects(&[tok.to_string()]);
            assert!(
                icons.iter().any(|(_, _, label)| *label == "Slowed"),
                "{tok} should show the snail: {icons:?}"
            );
        }
        assert!(
            !status_effects(&["poison".to_string()])
                .iter()
                .any(|(_, _, l)| *l == "Slowed"),
            "poison is not a slow"
        );
    }

    /// Every status icon we can show must carry a measured horizontal offset. These are
    /// Nerd Font icons in a monospace face: the ink is wider than the cell it advances, so
    /// flex centring alone leaves the glyph pushed right — the skull sat 1.35px off and the
    /// eye 2.86px. A new icon added without an entry here would silently do the same.
    #[test]
    fn every_status_icon_has_a_measured_ink_width() {
        // One combatant carrying everything the badge can draw.
        let all = vec![
            "web".to_string(),
            "marked".to_string(),
            "distracted".to_string(),
            "hasted".to_string(),
            "poison".to_string(),
            "burn".to_string(),
            "barrier:5".to_string(),
            "regen:3".to_string(),
            "evasion:20".to_string(),
        ];
        let icons = status_effects(&all);
        assert_eq!(icons.len(), 9, "every badge should be offered: {icons:?}");
        for (glyph, _, label) in icons {
            let ink = status_icon_ink(glyph);
            assert!(
                (10.0..=18.0).contains(&ink),
                "{label}'s ink width {ink} is not something an 18px glyph can be - read it \
                 out of the font's glyf table like the others"
            );
            // The placement it implies must put the ink's centre on the badge's centre,
            // measured from the OUTER edge (absolute `left` starts inside the border).
            let ink_centre = STATUS_BADGE_BORDER + status_icon_left(glyph) + ink / 2.0;
            assert!(
                (ink_centre - STATUS_BADGE / 2.0).abs() < 0.01,
                "{label}'s ink would centre at {ink_centre}, not {}",
                STATUS_BADGE / 2.0
            );
        }
    }

    /// A leader and its minions are the same species at 1.7x and 0.45x HP, so the only
    /// thing telling them apart on screen is size and the name. Both come from the
    /// `pack:` status the server now sends.
    #[test]
    fn a_pack_leader_draws_bigger_than_its_runts_and_says_which_it_is() {
        let leader = vec!["faction:fungal".to_string(), "pack:leader".to_string()];
        let minion = vec!["faction:fungal".to_string(), "pack:minion".to_string()];
        let lone: Vec<String> = vec!["faction:fungal".to_string()];

        assert!(pack_scale(&leader) > pack_scale(&lone));
        assert!(pack_scale(&minion) < pack_scale(&lone));
        assert_eq!(pack_scale(&lone), 1.0, "a lone creature is the reference size");

        assert_eq!(pack_label("myconid brute", &leader), "myconid brute (leader)");
        assert_eq!(pack_label("myconid brute", &minion), "myconid brute (runt)");
        assert_eq!(
            pack_label("myconid brute", &lone),
            "myconid brute",
            "a creature not in a pack gets no mark"
        );
    }
}

#[cfg(test)]
mod watch_banner_tests {
    /// ⚠️ A CLASS WITH NO ABILITY ART MUST STILL READ AS DOING SOMETHING, AND THE
    /// STAND-IN MUST NOT LIE. Measured when this landed: 74 of the registry's 92
    /// abilities had no clip, so the fallback is the common case rather than the edge —
    /// but a swing may only stand in for a blow. This reads the registry rather than a
    /// list, so an ability added later is classified the day it lands.
    #[test]
    fn a_swing_stands_in_for_a_blow_and_never_for_a_kindness() {
        use meld_proto::skills;
        for def in skills::SKILLS {
            let is_blow = matches!(def.target, skills::Target::Enemy | skills::Target::AllEnemies);
            assert_eq!(
                swings_at_a_foe(def.key),
                is_blow,
                "{} targets {:?}",
                def.key,
                def.target
            );
        }
        // The two that matter most, spelled out: a Hunter's unanimated capstone gets the
        // swing, and the Resonant's heal is left alone rather than miming an attack.
        assert!(swings_at_a_foe("pin_the_prey"));
        assert!(!swings_at_a_foe("transfuse"));
    }

    use super::*;

    /// The banner is the only thing on the battle screen that says a watcher is watching
    /// (`SOC-3`). Everything else already degrades right — the command menu, the party
    /// strip and the hero keys all key off `your_ids`, which is empty for a spectator — so
    /// without it they see a fight, no menu, and no way out, which reads as a hang.
    ///
    /// A real-system test rather than a screenshot: spectating is a transient state that
    /// autoplay cannot be steered into, and the memory of chasing transient UI with
    /// screenshots is a long one.
    #[test]
    fn the_banner_says_watching_only_while_watching() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.init_resource::<BattleData>();
        app.add_systems(Update, render_watch_banner);

        // In a fight of our own: nothing. The banner must not appear over a real battle.
        app.update();
        let mut q = app.world_mut().query_filtered::<Entity, With<WatchBanner>>();
        assert_eq!(q.iter(app.world()).count(), 0, "a fighter was told they were watching");

        app.world_mut().resource_mut::<BattleData>().spectating = true;
        app.update();
        let mut q = app.world_mut().query_filtered::<Entity, With<WatchBanner>>();
        assert_eq!(q.iter(app.world()).count(), 1, "a watcher was told nothing");

        // And it comes down the instant the feed closes, rather than lingering over the
        // overworld or over the next fight.
        app.world_mut().resource_mut::<BattleData>().spectating = false;
        app.update();
        let mut q = app.world_mut().query_filtered::<Entity, With<WatchBanner>>();
        assert_eq!(q.iter(app.world()).count(), 0, "the banner outlived the feed");
    }

    /// A watcher owns no combatant, so the whole command surface has to fall silent on its
    /// own — this pins the property the banner depends on rather than a second suppression
    /// path that could drift from it.
    #[test]
    fn a_watcher_has_no_hero_to_command() {
        let battle = BattleData { spectating: true, your_ids: Vec::new(), ..Default::default() };
        assert!(battle.active.is_none(), "a watcher was handed an active hero");
        assert!(battle.your_ids.is_empty());
    }
}
