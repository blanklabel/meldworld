//! **WHAT AN ABILITY LOOKS LIKE.** The battle's elemental VFX: impact bursts over
//! whoever was struck, a screen wash behind a big one, and the sprite reactions that make
//! a buffed creature look buffed.
//!
//! Reported from play: *"when you're hit with a fireball, the screen shows you're getting
//! hit by a fireball… if a creature buffs, maybe make their sprite bigger; if they rage,
//! turn the sprite redder."* Dragon Quest is the reference, and it is the right one for
//! HD-2D billboards — short, punchy, readable, a colour wash on the big spells and
//! nothing that outlasts the turn it belongs to.
//!
//! ## Three tiers, and only two of them are shaders
//!
//! 1. **Sprite reactions** — rage reddens, a boon swells, an affliction tints. These are
//!    material and transform writes on the billboard that already exists
//!    ([`react_to_conditions`]). No shader: this is the cheapest, most reliable tier and
//!    it is where most of the readability actually comes from.
//! 2. **Impact bursts** — one WGSL material ([`AbilityFx`], `ability_fx.wgsl`) with a
//!    `kind` uniform, on a camera-facing quad over the struck combatant. ONE shader for
//!    every element rather than one per element, for the same reason the ability registry
//!    beats a list of keys: sixteen shaders is sixteen things to keep in step.
//! 3. **Screen wash** — the same idea at full-frame scale ([`ScreenWash`],
//!    `screen_wash.wgsl`), on a quad parented to the battle camera. Only for a blow that
//!    hit the whole party or the whole field, because a wash on every basic attack is a
//!    strobe.
//!
//! ## Everything here is additive, and that is load-bearing
//!
//! `sprite_material`'s note records the trap this repo has hit three times: a full-quad
//! effect painting over the art. Both materials are [`AlphaMode::Add`] with an alpha that
//! falls to zero at their own edges, so the worst a mistuned effect can do is look too
//! bright — never hide the fight. The wash in particular can never go opaque.
//!
//! ## And it all dies with the fight
//!
//! Every entity this module spawns carries [`BattleFxRoot`], despawned on
//! `OnExit(Screen::Battle)` beside `BattleActor`. [`reset_battle_fx`] additionally clears
//! the queue, because a burst queued on the frame the fight ended would otherwise be the
//! first thing the NEXT fight drew. The sprite reactions need no teardown: they write to
//! materials and transforms owned by `BattleActor`, which is despawned with them.

use bevy::prelude::*;
use bevy::render::render_resource::AsBindGroup;
use bevy::shader::ShaderRef;
use meld_proto::enums::DamageType;

use crate::feel::BattleFeel;

/// Everything this module spawns. One marker, one `despawn::<BattleFxRoot>` on battle
/// exit — the same teardown every other battle entity uses.
#[derive(Component)]
pub(crate) struct BattleFxRoot;

/// An impact burst in flight, so [`advance_ability_fx`] can age and retire it.
#[derive(Component)]
pub(crate) struct FxBurst {
    age: f32,
    ttl: f32,
    /// Which combatant it is pinned over, so it tracks a recoiling sprite instead of
    /// detaching from it.
    follow: String,
    /// World-space height above the actor's feet.
    lift: f32,
}

/// The camera-locked wash quad. Spawned once per fight, lazily, and left hidden between
/// bursts — a quad is cheaper to hide than to respawn, and respawning it every blast
/// would churn a material handle ten times a fight.
#[derive(Component)]
pub(crate) struct FxWash {
    age: f32,
    ttl: f32,
}

/// The impact material. `kind` picks the element inside one shader.
#[derive(Asset, AsBindGroup, TypePath, Debug, Clone)]
pub(crate) struct AbilityFx {
    /// `(kind, age 0..1, seed, strength)`.
    #[uniform(100)]
    pub(crate) params: Vec4,
    #[uniform(100)]
    pub(crate) tint: Vec4,
}

impl Material for AbilityFx {
    fn fragment_shader() -> ShaderRef {
        "shaders/ability_fx.wgsl".into()
    }
    /// ADDITIVE — see the module note. An impact adds light to the arena; it never
    /// occludes the art beneath it.
    fn alpha_mode(&self) -> AlphaMode {
        AlphaMode::Add
    }
    /// ⚠️ CULLING OFF. The quad is billboarded to face the camera, and the camera can
    /// orbit past its plane — the same lesson `SkyDome` records, where a custom `Material`
    /// silently defaults to back-face culling and the geometry is thrown away with no
    /// error at all.
    fn specialize(
        _pipeline: &bevy::pbr::MaterialPipeline,
        descriptor: &mut bevy::render::render_resource::RenderPipelineDescriptor,
        _layout: &bevy::mesh::MeshVertexBufferLayoutRef,
        _key: bevy::pbr::MaterialPipelineKey<Self>,
    ) -> Result<(), bevy::render::render_resource::SpecializedMeshPipelineError> {
        descriptor.primitive.cull_mode = None;
        Ok(())
    }
}

/// The full-frame wash. Same uniform shape as [`AbilityFx`] on purpose: one `kind`
/// vocabulary across both shaders, so a new element cannot mean one thing to the impact
/// and another to the screen.
#[derive(Asset, AsBindGroup, TypePath, Debug, Clone)]
pub(crate) struct ScreenWash {
    #[uniform(100)]
    pub(crate) params: Vec4,
    #[uniform(100)]
    pub(crate) tint: Vec4,
}

impl Material for ScreenWash {
    fn fragment_shader() -> ShaderRef {
        "shaders/screen_wash.wgsl".into()
    }
    fn alpha_mode(&self) -> AlphaMode {
        AlphaMode::Add
    }
    fn specialize(
        _pipeline: &bevy::pbr::MaterialPipeline,
        descriptor: &mut bevy::render::render_resource::RenderPipelineDescriptor,
        _layout: &bevy::mesh::MeshVertexBufferLayoutRef,
        _key: bevy::pbr::MaterialPipelineKey<Self>,
    ) -> Result<(), bevy::render::render_resource::SpecializedMeshPipelineError> {
        descriptor.primitive.cull_mode = None;
        Ok(())
    }
}

/// An element's look: which branch of the shader draws it, and in what colour.
///
/// ⚠️ **THE `kind` NUMBERS ARE A CONTRACT.** They are a uniform read by two shaders, so
/// renumbering here silently draws fire where ice should be — held by
/// `the_kind_numbers_match_the_shader`, which reads the WGSL's own constants.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) struct Element {
    pub(crate) kind: f32,
    /// Linear RGB, deliberately over 1.0 on the hot channels: these are additive and the
    /// scene is HDR, so a value past 1 is what makes an impact bloom.
    pub(crate) rgb: Vec3,
}

/// What each damage type looks like. **Every** [`DamageType`] must land somewhere here —
/// a type with no entry is a blow the player sees no reason for.
pub(crate) fn element_of(ty: DamageType) -> Element {
    let e = |kind: f32, r: f32, g: f32, b: f32| Element { kind, rgb: Vec3::new(r, g, b) };
    match ty {
        // Physical (kind 0): the three weapon types share the slash-and-spark shape and
        // differ only in colour temperature, because a hammer and a sword should read as
        // the same CATEGORY of thing — the fight already tells you which weapon it was.
        DamageType::Slash => e(0.0, 1.9, 1.9, 2.0),
        DamageType::Pierce => e(0.0, 1.7, 1.85, 2.1),
        DamageType::Blunt => e(0.0, 2.0, 1.8, 1.4),
        DamageType::Fire => e(1.0, 3.4, 1.1, 0.25),
        DamageType::Infernal => e(1.0, 3.2, 0.5, 0.9),
        DamageType::Ice => e(2.0, 1.1, 2.4, 3.2),
        DamageType::Lightning => e(3.0, 2.4, 2.2, 3.6),
        DamageType::Water => e(4.0, 0.7, 1.8, 3.0),
        DamageType::Wind => e(5.0, 1.6, 2.4, 1.9),
        DamageType::Earth => e(6.0, 1.9, 1.4, 0.8),
        DamageType::Mind => e(7.0, 2.4, 1.0, 2.8),
        DamageType::Ethereal => e(7.0, 1.6, 2.2, 2.6),
        DamageType::Poison => e(8.0, 1.5, 2.6, 0.7),
        DamageType::Celestial => e(9.0, 3.0, 2.9, 2.0),
        DamageType::Shadow => e(10.0, 1.2, 0.5, 1.9),
        // True damage answers to nothing, so it gets the one effect with no element in
        // it: the physical shape, colourless and very bright.
        DamageType::None => e(0.0, 2.6, 2.6, 2.6),
    }
}

/// One queued cast, waiting for the next frame to place it over its targets.
///
/// Queued rather than spawned inline because the wire handler ([`crate::netglue`]) has no
/// access to the arena's transforms — the same split every other battle effect uses, and
/// what lets a burst find an actor that is spawned later in the frame.
pub(crate) struct Cast {
    pub(crate) targets: Vec<String>,
    pub(crate) element: Element,
    /// A blow that landed on three or more combatants — the bar for spending a screen
    /// wash. Everything smaller stays local to its target, because a wash on every basic
    /// attack is a strobe rather than a flourish.
    pub(crate) wide: bool,
    /// 0..1: how hard it hit, as a share of the target's max HP. Scales the burst and the
    /// wash, so a scratch and an apocalypse do not look alike.
    pub(crate) power: f32,
}

/// The queue plus a scratch seed counter. Drained every frame by [`spawn_ability_fx`].
#[derive(Resource, Default)]
pub(crate) struct BattleFx {
    pub(crate) queue: Vec<Cast>,
    seed: u32,
}

impl BattleFx {
    /// A per-cast seed, so two fireballs on the same frame are not the same fireball.
    /// A counter rather than a random source: reproducible under `MELD_BATTLE`, which is
    /// the fixture every screenshot of this is taken through.
    fn next_seed(&mut self) -> f32 {
        self.seed = self.seed.wrapping_add(1);
        (self.seed % 997) as f32 * 0.0631
    }
}

/// Turn a resolved action into a queued cast. Called from the wire handler.
///
/// `hits` is `(target, damage, max_hp)` for every combatant the action actually damaged —
/// a heal or a pure buff queues nothing, because those have their own tell (the sprite
/// swell in [`react_to_conditions`]) and a burst over a healed ally reads as an attack.
pub(crate) fn queue_cast(fx: &mut BattleFx, ty: Option<DamageType>, hits: &[(String, i32, i32)]) {
    if hits.is_empty() {
        return;
    }
    // An untyped resolution that still did damage is a weapon blow: the engine leaves
    // `damage_type` empty only when nothing typed landed, and a hit with no effect at all
    // is worse than a generic one.
    let element = element_of(ty.unwrap_or(DamageType::None));
    // The hardest single hit as a share of that target's own pool. Per-target rather than
    // summed: an all-enemy chip on five bodies is not a big hit, and a summing rule would
    // say it was.
    let power = hits
        .iter()
        .map(|(_, dmg, max)| (*dmg as f32 / (*max).max(1) as f32).clamp(0.0, 1.0))
        .fold(0.0f32, f32::max);
    fx.queue.push(Cast {
        targets: hits.iter().map(|(t, _, _)| t.clone()).collect(),
        element,
        wide: hits.len() >= 3,
        power,
    });
}

/// Place this frame's queued bursts over their targets, and raise a wash behind a wide one.
#[allow(clippy::too_many_arguments)]
pub(crate) fn spawn_ability_fx(
    mut commands: Commands,
    mut fx: ResMut<BattleFx>,
    feel: Res<BattleFeel>,
    mut mats: ResMut<Assets<AbilityFx>>,
    mut washes: ResMut<Assets<ScreenWash>>,
    mut meshes: ResMut<Assets<Mesh>>,
    actors: Query<(&crate::battle::BattleActor, &Transform)>,
    cam: Query<Entity, With<Camera3d>>,
    existing_wash: Query<(Entity, &MeshMaterial3d<ScreenWash>), With<FxWash>>,
) {
    if fx.queue.is_empty() {
        return;
    }
    let casts: Vec<Cast> = std::mem::take(&mut fx.queue);
    for cast in casts {
        let seed = fx.next_seed();
        // A hit's size drives how big the burst draws — a scratch should not look like a
        // capstone. Floored well above zero so a 1-damage poke still reads.
        let scale = feel.fx_size * (0.7 + cast.power * 0.9);
        for target in &cast.targets {
            let Some((_, tf)) = actors.iter().find(|(a, _)| a.id == *target) else {
                continue;
            };
            let mat = mats.add(AbilityFx {
                params: Vec4::new(cast.element.kind, 0.0, seed, 1.0),
                tint: cast.element.rgb.extend(1.0),
            });
            commands.spawn((
                BattleFxRoot,
                FxBurst {
                    age: 0.0,
                    ttl: feel.fx_ttl,
                    follow: target.clone(),
                    lift: feel.fx_height,
                },
                Mesh3d(meshes.add(Rectangle::new(scale, scale))),
                MeshMaterial3d(mat),
                Transform::from_translation(tf.translation + Vec3::Y * feel.fx_height),
                // The same billboarding every sprite in the arena uses, so an impact
                // faces the player under any camera orbit.
                crate::hd2d::Billboard,
            ));
        }
        if !cast.wide {
            continue;
        }
        // THE WASH. Reuse the standing quad if there is one — a second blast during the
        // first simply restarts it, which is what "the screen answers the newest thing"
        // should mean.
        let params = Vec4::new(cast.element.kind, 0.0, seed, (0.45 + cast.power).min(1.0));
        let tint = cast.element.rgb.extend(1.0);
        if let Some((e, handle)) = existing_wash.iter().next() {
            if let Some(mut m) = washes.get_mut(&handle.0) {
                m.params = params;
                m.tint = tint;
            }
            commands.entity(e).insert(FxWash { age: 0.0, ttl: feel.wash_ttl });
            continue;
        }
        let Ok(cam) = cam.single() else { continue };
        let mat = washes.add(ScreenWash { params, tint });
        // Parented to the camera and pushed forward along -Z: the quad rides every orbit
        // and zoom for free, which is the whole reason this is not a post-process node.
        // Sized generously past the frustum at that distance so no edge is ever on screen.
        commands.entity(cam).with_children(|p| {
            p.spawn((
                BattleFxRoot,
                FxWash { age: 0.0, ttl: feel.wash_ttl },
                Mesh3d(meshes.add(Rectangle::new(40.0, 40.0))),
                MeshMaterial3d(mat),
                Transform::from_xyz(0.0, 0.0, -8.0),
            ));
        });
    }
}

/// Age every burst, keep it over its (recoiling) target, and retire it.
pub(crate) fn advance_ability_fx(
    mut commands: Commands,
    time: Res<Time>,
    mut mats: ResMut<Assets<AbilityFx>>,
    actors: Query<(&crate::battle::BattleActor, &Transform), Without<FxBurst>>,
    mut q: Query<(
        Entity,
        &mut FxBurst,
        &mut Transform,
        &MeshMaterial3d<AbilityFx>,
    )>,
) {
    let dt = time.delta_secs();
    for (e, mut burst, mut tf, mat) in &mut q {
        burst.age += dt;
        if burst.age >= burst.ttl {
            commands.entity(e).despawn();
            continue;
        }
        if let Some((_, at)) = actors.iter().find(|(a, _)| a.id == burst.follow) {
            tf.translation = at.translation + Vec3::Y * burst.lift;
        }
        if let Some(mut m) = mats.get_mut(&mat.0) {
            let t = burst.age / burst.ttl.max(1e-3);
            m.params.y = t;
            // The CPU owns the tail fade so every element ends the same way, whatever its
            // own shape does — a burst that lingers a frame longer than its neighbours
            // reads as a stuck effect.
            m.tint.w = (1.0 - t).clamp(0.0, 1.0).powf(0.6);
        }
    }
}

/// Age the wash, and hide it once it has run — hidden rather than despawned, because the
/// next blast wants the same quad.
pub(crate) fn advance_screen_wash(
    time: Res<Time>,
    mut mats: ResMut<Assets<ScreenWash>>,
    mut q: Query<(&mut FxWash, &mut Visibility, &MeshMaterial3d<ScreenWash>)>,
) {
    let dt = time.delta_secs();
    for (mut wash, mut vis, mat) in &mut q {
        wash.age += dt;
        let done = wash.age >= wash.ttl;
        let want = if done { Visibility::Hidden } else { Visibility::Inherited };
        if *vis != want {
            *vis = want;
        }
        if done {
            continue;
        }
        if let Some(mut m) = mats.get_mut(&mat.0) {
            m.params.y = wash.age / wash.ttl.max(1e-3);
        }
    }
}

/// **A CREATURE THAT BUFFS LOOKS BUFFED.** Sprite scale driven off the combatant's own
/// status tokens, for every combatant — heroes and creatures alike.
///
/// This is the tier that needs no shader, and it does most of the readability work: a
/// body carrying boons stands larger and breathes, a frenzied one swells with fury. The
/// tokens are already on the wire (`Combatant.statuses`), so this costs one query.
///
/// ⚠️ **IT WRITES SCALE EVERY FRAME, INCLUDING THE WAY BACK TO 1.0.** A pulse that only
/// writes while a condition is present leaves the sprite stuck at its swollen size the
/// moment the condition lifts, and the swell outlives the buff.
///
/// ⚠️ **AND IT DELIBERATELY DOES NOT TOUCH THE MATERIAL.** Colour on a battle sprite has
/// exactly one owner, `animate_battle_actors` — the night-glow note there records what
/// two systems writing one material costs: whichever the scheduler ran second decided the
/// frame, and the party flickered for the whole fight. Scale is a transform, so it can
/// live here; the rage TINT belongs beside the flash, and that is where it is.
pub(crate) fn react_to_conditions(
    time: Res<Time>,
    battle: Res<crate::BattleData>,
    feel: Res<BattleFeel>,
    mut q: Query<(&crate::battle::SpriteQuad, &mut Transform)>,
) {
    let t = time.elapsed_secs();
    let dt = time.delta_secs();
    for (s, mut tf) in &mut q {
        let want = match battle.view(&s.id) {
            Some(c) => {
                // A BOON SWELLS. Barrier, Regen, Haste, Evasion and a fight-long attack
                // buff all make the body read as larger — the DQ tell for "something was
                // done FOR this one". Deliberately small: a sprite that grows 30% overlaps
                // its neighbours in a five-body rank, and the rank is how a pack stays
                // legible.
                let boons = ["barrier:", "regen:", "hasted", "evasion:", "tempered"]
                    .iter()
                    // `barrier:0` is not a barrier — the same zero-guard `condition_tint`
                    // makes, or a spent boon keeps its swell forever.
                    .filter(|k| {
                        c.statuses.iter().any(|x| x.starts_with(*k) && !x.ends_with(":0"))
                    })
                    .count();
                let swell = (boons as f32).min(3.0) * feel.buff_swell;
                // A slow breath, so a buffed body is alive rather than simply bigger.
                let breath = if swell > 0.0 {
                    (t * feel.buff_breath_hz).sin() * feel.buff_swell * 0.35
                } else {
                    0.0
                };
                let rage = if c.statuses.iter().any(|x| x == "frenzied") {
                    feel.rage_swell
                } else {
                    0.0
                };
                1.0 + swell + breath + rage
            }
            // A sprite with no combatant behind it any more eases home rather than
            // freezing mid-swell.
            None => 1.0,
        };
        // Ease rather than snap, so a boon landing is a beat instead of a pop.
        let now = tf.scale.x;
        let k = (feel.buff_ease * dt).clamp(0.0, 1.0);
        tf.scale = Vec3::splat(now + (want - now) * k);
    }
}

/// Clear the queue when the fight ends.
///
/// The entities go with `despawn::<BattleFxRoot>`; this is the other half. A cast queued
/// on the frame the battle ended would otherwise sit in the resource and be drawn as the
/// first thing the NEXT fight showed — over whichever combatant happened to reuse the id.
pub(crate) fn reset_battle_fx(mut fx: ResMut<BattleFx>) {
    fx.queue.clear();
    fx.seed = 0;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **Every damage type draws something.** `element_of` is a total match, so the
    /// compiler holds that every type has an entry — this pins the half it cannot: that
    /// each entry names a kind the shader implements, and a colour bright enough for an
    /// additive effect to be visible at all.
    #[test]
    fn every_damage_type_draws_something() {
        use DamageType::*;
        for ty in [
            Blunt, Slash, Pierce, Water, Ice, Fire, Lightning, Wind, Earth, Poison, Infernal,
            Celestial, Shadow, Mind, Ethereal, None,
        ] {
            let e = element_of(ty);
            assert!(
                (0.0..=10.0).contains(&e.kind),
                "{ty:?} draws kind {} — the shader only implements 0..=10",
                e.kind
            );
            assert_eq!(e.kind, e.kind.floor(), "{ty:?}'s kind is not a whole number");
            assert!(
                e.rgb.max_element() > 1.0,
                "{ty:?} is dimmer than 1.0 on every channel, so an ADDITIVE effect will \
                 not be visible against a lit arena: {:?}",
                e.rgb
            );
        }
    }

    /// ⚠️ The `kind` numbers are a uniform shared with two WGSL files, so renumbering one
    /// side draws fire where ice should be — silently, since a float is a float. Read the
    /// shader's own constants and hold the Rust table against them.
    #[test]
    fn the_kind_numbers_match_the_shader() {
        let src = include_str!("../assets/shaders/ability_fx.wgsl");
        let want = [
            ("KIND_PHYSICAL", DamageType::Slash),
            ("KIND_FIRE", DamageType::Fire),
            ("KIND_ICE", DamageType::Ice),
            ("KIND_LIGHTNING", DamageType::Lightning),
            ("KIND_WATER", DamageType::Water),
            ("KIND_WIND", DamageType::Wind),
            ("KIND_EARTH", DamageType::Earth),
            ("KIND_MIND", DamageType::Mind),
            ("KIND_POISON", DamageType::Poison),
            ("KIND_HOLY", DamageType::Celestial),
            ("KIND_SHADOW", DamageType::Shadow),
        ];
        for (name, ty) in want {
            let line = src
                .lines()
                .find(|l| l.trim_start().starts_with(&format!("const {name}:")))
                .unwrap_or_else(|| {
                    panic!(
                        "ability_fx.wgsl has no `{name}` — the Rust table names a kind the \
                         shader does not implement"
                    )
                });
            let n: f32 = line
                .split('=')
                .nth(1)
                .and_then(|v| v.trim().trim_end_matches(';').parse().ok())
                .unwrap_or_else(|| panic!("could not read a number out of `{line}`"));
            assert_eq!(
                element_of(ty).kind,
                n,
                "{ty:?} maps to kind {} in Rust and `{name}` is {n} in the shader",
                element_of(ty).kind
            );
        }
    }

    /// A heal or a pure buff must not throw an impact — a burst over a mended ally reads
    /// as the healer attacking them.
    #[test]
    fn a_cast_that_damaged_nobody_draws_nothing() {
        let mut fx = BattleFx::default();
        queue_cast(&mut fx, Some(DamageType::Fire), &[]);
        assert!(fx.queue.is_empty(), "an effect-less action queued a burst");
        queue_cast(&mut fx, Some(DamageType::Fire), &[("a".into(), 40, 100)]);
        assert_eq!(fx.queue.len(), 1);
    }

    /// The wash is for a blow that took the field, not for a sword. Three bodies is the
    /// bar: a two-target hit is a cleave, and a wash on every one of those is a strobe.
    #[test]
    fn only_a_wide_blow_washes_the_screen() {
        let hit = |n: usize| {
            let mut fx = BattleFx::default();
            let hits: Vec<(String, i32, i32)> =
                (0..n).map(|i| (format!("t{i}"), 10, 100)).collect();
            queue_cast(&mut fx, Some(DamageType::Fire), &hits);
            fx.queue[0].wide
        };
        assert!(!hit(1), "a single-target hit washed the screen");
        assert!(!hit(2), "a two-target cleave washed the screen");
        assert!(hit(3), "an all-enemy blast did not wash the screen");
    }

    /// Power is the WORST single hit as a share of that target's own pool, never the sum.
    /// Summed, a chip landing on five bodies would out-rank a hit that nearly killed one,
    /// and the screen would shout loudest at the least dangerous thing in the fight.
    #[test]
    fn a_wide_chip_is_not_a_big_hit() {
        let mut fx = BattleFx::default();
        queue_cast(
            &mut fx,
            Some(DamageType::Fire),
            &[("a".into(), 5, 100), ("b".into(), 5, 100), ("c".into(), 5, 100)],
        );
        let chip = fx.queue[0].power;
        let mut fx2 = BattleFx::default();
        queue_cast(&mut fx2, Some(DamageType::Fire), &[("a".into(), 80, 100)]);
        assert!(
            fx2.queue[0].power > chip,
            "a chip on three bodies ({chip}) out-ranked a hit that took 80% of one"
        );
    }

    /// Two casts on one frame must not be the same cast — an ice burst and a fire burst
    /// sharing a seed are two identical noise fields, which reads as a rendering glitch.
    #[test]
    fn two_casts_in_a_frame_do_not_share_a_seed() {
        let mut fx = BattleFx::default();
        let a = fx.next_seed();
        let b = fx.next_seed();
        assert_ne!(a, b);
    }

    /// The queue must not survive the fight it belongs to: a cast queued on the frame the
    /// battle ended would be drawn over whichever combatant reused that id next.
    #[test]
    fn the_queue_does_not_outlive_the_fight() {
        let mut fx = BattleFx::default();
        queue_cast(&mut fx, Some(DamageType::Fire), &[("a".into(), 40, 100)]);
        let _ = fx.next_seed();
        fx.queue.clear();
        fx.seed = 0;
        assert!(fx.queue.is_empty());
    }
}
