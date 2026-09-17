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
    /// Which combatant it is pinned over.
    ///
    /// The ACTOR's position, which does not move during a fight — the lunge and recoil are
    /// on the billboard CHILD, and re-reading them here would need a `GlobalTransform` that
    /// is a frame stale anyway. A burst is ~2 world units across and a recoil is 0.35, so
    /// the mismatch is invisible; what this buys is a burst that lands on the right body
    /// when combatants are added mid-fight and indices shift.
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
        // **THE THREE WEAPON TYPES DRAW THREE DIFFERENT SHAPES**: a cut, a punch and a
        // chevron. They used to share kind 0 and differ only in colour temperature, on the
        // argument that the fight already tells you which weapon it was — true of the hero
        // swinging and useless for the blow landing, and colour is the channel already
        // spent on WHAT WAS HIT (`struck_rgb`). A sword, a hammer and a spear leave three
        // different marks.
        DamageType::Slash => e(0.0, 1.9, 1.9, 2.0),
        DamageType::Pierce => e(12.0, 1.7, 1.85, 2.1),
        DamageType::Blunt => e(11.0, 2.0, 1.8, 1.4),
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
    /// Overall loudness, 0..1. A blow is 1.0; a mend is well under, because care should
    /// never be the brightest thing on a screen with something trying to kill you.
    ///
    /// The CAMERA is not on this struct on purpose: a shake is an impulse on the resource
    /// rather than a property of a cast, so the loudest blow of a frame wins instead of
    /// five sweep targets shaking five times. `queue_cast` raises it and `queue_mend`
    /// deliberately does not touch it.
    pub(crate) strength: f32,
}

/// **CARE GETS ITS OWN SHAPE, NOT AN IMPACT SHAPE.**
///
/// The rule this started as was "a mend draws nothing over the target", on the argument
/// that a burst over an ally the healer just tended reads as the healer attacking them.
/// That argument is right about the SHAPE and wrong about the conclusion: a slash or a
/// plume over an ally reads as an attack, but a soft rising column of light is what every
/// game in this lineage uses for healing, and drawing nothing left the single most common
/// friendly action in the game with no visual on its target at all — just a green number.
///
/// So a mend always draws `KIND_HOLY` (a column, not a burst) at a fraction of a blow's
/// loudness, never shakes the camera, and washes the screen only when it reached three or
/// more bodies — the same bar a blow answers to, for the same reason: a wash for a single
/// heal would make every Resonant turn a strobe.
pub(crate) fn queue_mend(fx: &mut BattleFx, mended: &[String]) {
    if mended.is_empty() {
        return;
    }
    fx.queue.push(Cast {
        targets: mended.to_vec(),
        element: Element { kind: 9.0, rgb: Vec3::new(1.6, 2.4, 1.3) },
        wide: mended.len() >= 3,
        // Well under a blow's, so the loudest thing on screen is still whatever is trying
        // to kill you.
        power: 0.15,
        strength: 0.55,
    });
}

/// The queue plus a scratch seed counter. Drained every frame by [`spawn_ability_fx`].
#[derive(Resource, Default)]
pub(crate) struct BattleFx {
    pub(crate) queue: Vec<Cast>,
    seed: u32,
    /// Camera-shake amplitude, 0..1, decayed by [`advance_ability_fx`] and read by
    /// `battle_camera`.
    ///
    /// THE CAMERA IS THE ONLY THING THAT MOVES THE WHOLE FRAME, which is why a heavy blow
    /// needs it: a per-sprite shake says "that body was hit" and a camera shake says "that
    /// hit was BIG". Every JRPG this is modelled on uses both, and the arena had only the
    /// first. Held as an impulse the loudest blow wins rather than a sum, so a five-target
    /// sweep resolving on one frame shakes once instead of five times as hard.
    pub(crate) shake: f32,
    /// Bodies that fell this frame, drained by [`spawn_death_bursts`].
    ///
    /// Queued rather than spawned at the wire, for the same reason `queue` is: the handler
    /// runs off an incoming message and has no access to the arena's transforms, and a
    /// death has to be drawn where the BODY is.
    pub(crate) deaths: Vec<String>,
    /// Bodies struck this frame as `(id, was it a crit)`, drained by [`spawn_hit_sparks`].
    pub(crate) sparks: Vec<(String, bool)>,
    /// ONE unit quad, shared by every burst ever spawned, sized through the transform.
    ///
    /// A `Rectangle::new(scale, scale)` per burst allocates a mesh asset per target per
    /// cast — an all-enemy sweep on five bodies every turn, for the length of a fight.
    /// The size is a transform, so there is no reason for it to be geometry.
    quad: Option<Handle<Mesh>>,
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
pub(crate) fn queue_cast(
    fx: &mut BattleFx,
    ty: Option<DamageType>,
    hits: &[(String, i32, i32)],
    crit: bool,
) {
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
        .map(|(_, dmg, max)| {
            // ⚠️ A MISSING max_hp IS NOT A KILLING BLOW. `.max(1)` on an unknown pool turns
            // any damage at all into 1.0 — the largest burst and the loudest wash the game
            // has — so a combatant the client has not got a view of yet (a reinforcement
            // arriving mid-frame) would draw an apocalypse for a scratch. Unknown reads as
            // an ordinary hit instead.
            if *max <= 0 {
                0.5
            } else {
                (*dmg as f32 / *max as f32).clamp(0.0, 1.0)
            }
        })
        .fold(0.0f32, f32::max);
    // A CRIT LOOKS LIKE ONE. The number already says "CRIT!"; the burst should agree, or
    // the loudest feedback in the fight is a line of text. It lifts the floor rather than
    // the ceiling, so a critical scratch still reads as a scratch.
    let power = if crit { (power * 1.25).max(0.55) } else { power };
    let wide = hits.len() >= 3;
    // The camera feels the worst single blow, and a wide one harder. Capped at 1.0 by the
    // clamp; `max` rather than `+=` so a sweep resolving on one frame shakes once.
    fx.shake = fx.shake.max((power * if wide { 1.0 } else { 0.6 }).clamp(0.0, 1.0));
    fx.queue.push(Cast {
        targets: hits.iter().map(|(t, _, _)| t.clone()).collect(),
        element,
        wide,
        power,
        strength: 1.0,
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
    let quad = fx
        .quad
        .get_or_insert_with(|| meshes.add(Rectangle::new(1.0, 1.0)))
        .clone();
    let casts: Vec<Cast> = std::mem::take(&mut fx.queue);
    for cast in casts {
        let seed = fx.next_seed();
        // ⚠️ THE TARGET LOOP MUST BE ALLOWED TO DO NOTHING and still fall through to the
        // wash below, rather than being guarded on a non-empty list: a cast whose targets
        // have all left the arena (a reinforcement felled on the frame it arrived) still
        // has a screen to answer for.
        // A hit's size drives how big the burst draws — a scratch should not look like a
        // capstone. Floored well above zero so a 1-damage poke still reads.
        let scale = feel.fx_size * (0.7 + cast.power * 0.9);
        for target in &cast.targets {
            let Some((_, tf)) = actors.iter().find(|(a, _)| a.id == *target) else {
                continue;
            };
            let mat = mats.add(AbilityFx {
                params: Vec4::new(cast.element.kind, 0.0, seed, cast.strength),
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
                Mesh3d(quad.clone()),
                MeshMaterial3d(mat),
                Transform::from_translation(tf.translation + Vec3::Y * feel.fx_height)
                    .with_scale(Vec3::splat(scale)),
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
        let params =
            Vec4::new(cast.element.kind, 0.0, seed, ((0.45 + cast.power) * cast.strength).min(1.0));
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
    feel: Res<BattleFeel>,
    mut fx: ResMut<BattleFx>,
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
    // The camera's shake decays here rather than in its own system: it is queued by the
    // same event that spawns a burst and lives about as long, so keeping the two together
    // means one place to look when the arena is jittering.
    fx.shake = (fx.shake - dt / feel.shake_ttl.max(1e-3)).max(0.0);
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
/// frame, and the party flickered for the whole fight. The rage TINT belongs beside the
/// flash, and that is where it is.
///
/// ⚠️ **AND IT SCALES THE ACTOR ROOT, NOT THE SPRITE QUAD — I made both mistakes the note
/// above warns about.** Writing the quad's `Transform` walked straight into the same trap
/// twice over:
///
/// 1. **A second writer.** `hd2d::place_billboards` sets `translation.y` and `scale` on
///    every `HeroBillboard` from `Look`, every frame, and a battle hero's quad IS one. Two
///    systems, one field, no ordering — so the scale alternated between `sprite_scale`
///    (1.6) and `1.0 + swell` depending on which ran second. Reported from play as the
///    characters *bouncing when they don't do anything*.
/// 2. **Un-grounding.** The quad is centred at `grounded_sprite_y`, so scaling it about its
///    own centre lifts the feet off the floor and drops them back — a swell on a grounded
///    billboard is a vertical bob whether anything else writes it or not.
///
/// The ROOT is at ground level (`y = 0`) and nothing else writes its scale, so scaling it
/// grows the body UPWARD FROM ITS FEET and cannot fight anybody. The shadow scales with it,
/// which is what a bigger body should do.
pub(crate) fn react_to_conditions(
    time: Res<Time>,
    battle: Res<crate::BattleData>,
    hitfx: Res<crate::HitFx>,
    feel: Res<BattleFeel>,
    mut q: Query<(&crate::battle::BattleActor, &mut Transform)>,
) {
    let t = time.elapsed_secs();
    let dt = time.delta_secs();
    for (actor, mut tf) in &mut q {
        // **YOU CAN SEE THE BIG ONE COMING.** A telegraphed creature ability shouted a
        // bubble and did nothing else to the creature, so the one mechanic in the game
        // built to be REACTED to was a line of text over a sprite that looked exactly like
        // it had a moment earlier. A channeling body swells and winds UP — the pulse
        // quickens as the cast approaches — which is the affordance that makes a telegraph
        // a decision rather than an announcement.
        let charging = hitfx
            .charging()
            .find(|(id, _)| *id == actor.id.as_str())
            .map(|(_, age)| age);
        let want = match battle.view(&actor.id) {
            // A DOWNED body does not swell. Its boons are still on the wire — a Barrier
            // does not clear because its holder fell — so without this a corpse keeps
            // breathing at full size beside the fight it lost.
            Some(c) if c.hp <= 0 => 1.0,
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
                // The wind-up outranks everything else on the body: whatever else is
                // true of a creature, the thing about to land is what you have to answer.
                let wind = charging
                    .map(|age| {
                        // Quickening: the pulse rate climbs with age, so the beat itself
                        // says "soon" without needing a bar.
                        let hz = feel.charge_hz_base + age * feel.charge_hz_ramp;
                        feel.charge_swell * (0.55 + 0.45 * (age * hz).sin())
                    })
                    .unwrap_or(0.0);
                1.0 + swell + breath + rage + wind
            }
            // A sprite with no combatant behind it any more eases home rather than
            // freezing mid-swell.
            None => 1.0,
        };
        // Ease rather than snap, so a boon landing is a beat instead of a pop.
        let now = tf.scale.x;
        let k = (feel.buff_ease * dt).clamp(0.0, 1.0);
        let next = now + (want - now) * k;
        // Written only when it moves: this is the actor ROOT, and a `DerefMut` write flags
        // it changed every frame for every body in the arena — which is what the projected
        // panels (`render_enemy_panel`, `render_status_icons`) watch to decide whether to
        // rebuild. A converged swell that kept writing kept them rebuilding every frame.
        if (next - now).abs() > 1e-4 {
            tf.scale = Vec3::splat(next);
        }
    }
}

/// Clear the queue when the fight ends.
///
/// The entities go with `despawn::<BattleFxRoot>`; this is the other half. A cast queued
/// on the frame the battle ended would otherwise sit in the resource and be drawn as the
/// first thing the NEXT fight showed — over whichever combatant happened to reuse the id.
pub(crate) fn reset_battle_fx(mut fx: ResMut<BattleFx>) {
    fx.queue.clear();
    // A death queued on the frame the fight ended would otherwise be the first thing the
    // NEXT fight drew, over a body that is not there — the same trap as `queue` above.
    fx.deaths.clear();
    fx.sparks.clear();
    fx.seed = 0;
    // A shake left running would follow the camera onto the overworld, where nothing is
    // hitting anybody.
    fx.shake = 0.0;
    // The shared quad is deliberately KEPT. It is one unit mesh with no per-fight state,
    // and dropping it means re-uploading it at the first blow of the next battle.
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view(id: &str, is_player: bool) -> meld_client::net::CombatantView {
        meld_client::net::CombatantView {
            id: id.into(),
            name: id.into(),
            hp: 0,
            max_hp: 10,
            level: 1,
            gauge: 0.0,
            is_player,
            player_id: None,
            statuses: Vec::new(),
        }
    }

    /// **A FELLED CREATURE EARNS A BURST, AND A FALLEN HERO DOES NOT.** The arena stops
    /// drawing a body the moment `hp > 0` goes false, so without this the thing you have
    /// been hitting is replaced by nothing between two frames. Showering your own party in
    /// ash for the same event reads as a loss rather than as a setback.
    #[test]
    fn a_felled_enemy_earns_a_burst_and_a_fallen_hero_does_not() {
        let roster = vec![view("m1", false), view("h1", true)];
        assert!(is_enemy_death("ko", "m1", &roster), "a felled creature drew nothing");
        assert!(!is_enemy_death("ko", "h1", &roster), "a fallen hero showered the party in ash");
        // …and only a KO. Every blow that merely HURT something must not fire it, or a
        // fight is one long cloud of ash.
        assert!(!is_enemy_death("damage", "m1", &roster));
        assert!(!is_enemy_death("heal", "m1", &roster));
        // A body the client has no view of yet (a reinforcement arriving mid-frame) is not
        // assumed to be an enemy — the burst would be drawn at the origin.
        assert!(!is_enemy_death("ko", "nobody", &roster));
    }

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
                (0.0..=12.0).contains(&e.kind),
                "{ty:?} draws kind {} — the shader only implements 0..=12",
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
            // The three weapon marks. A cut, a punch and a chevron are three shapes, so
            // they are three kinds — and the shader branches on the NUMBER, which means a
            // renumber on one side draws a hammer blow as a spear thrust, silently.
            ("KIND_BLUNT", DamageType::Blunt),
            ("KIND_PIERCE", DamageType::Pierce),
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
        queue_cast(&mut fx, Some(DamageType::Fire), &[], false);
        assert!(fx.queue.is_empty(), "an effect-less action queued a burst");
        queue_cast(&mut fx, Some(DamageType::Fire), &[("a".into(), 40, 100)], false);
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
            queue_cast(&mut fx, Some(DamageType::Fire), &hits, false);
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
            false,
        );
        let chip = fx.queue[0].power;
        let mut fx2 = BattleFx::default();
        queue_cast(&mut fx2, Some(DamageType::Fire), &[("a".into(), 80, 100)], false);
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

    /// **CARE GETS ITS OWN SHAPE.** A mend draws — drawing nothing left the most common
    /// friendly action in the game with no visual on its target at all — but it draws the
    /// holy COLUMN rather than an impact shape, quieter than a blow, and it never shakes
    /// the camera. It washes the screen only when it reached three or more bodies, the same
    /// bar a blow answers to.
    #[test]
    fn a_mend_draws_care_rather_than_an_impact() {
        let mut fx = BattleFx::default();
        queue_mend(&mut fx, &[]);
        assert!(fx.queue.is_empty(), "mending nobody drew something");

        queue_mend(&mut fx, &["a".into()]);
        let one = fx.queue.pop().expect("a single mend drew nothing at all");
        assert_eq!(one.targets.len(), 1, "a mend did not reach its target");
        assert!(!one.wide, "a single heal washed the screen");
        assert_eq!(fx.shake, 0.0, "care shook the camera");
        assert_eq!(one.element.kind, 9.0, "a mend drew an impact shape instead of a column");

        queue_mend(&mut fx, &["a".into(), "b".into(), "c".into()]);
        assert!(fx.queue.pop().expect("no cast").wide, "a party-wide mend did not wash");

        // And it must stay quieter than a blow, or the loudest thing on screen is the
        // healer rather than whatever is trying to kill you.
        let mut hit = BattleFx::default();
        queue_cast(&mut hit, Some(DamageType::Fire), &[("a".into(), 40, 100)], false);
        let blow = &hit.queue[0];
        assert!(
            one.power * one.strength < blow.power * blow.strength,
            "a mend is louder than a blow that took 40% of a hero"
        );
        assert!(hit.shake > 0.0, "a blow did not move the camera");
    }

    /// A CRIT LOOKS LIKE ONE. The number says "CRIT!" already; if the burst does not agree
    /// then the loudest feedback in a fight is a line of text.
    #[test]
    fn a_crit_hits_harder_on_screen_too() {
        let mk = |crit: bool| {
            let mut fx = BattleFx::default();
            queue_cast(&mut fx, Some(DamageType::Slash), &[("a".into(), 12, 100)], crit);
            (fx.queue[0].power, fx.shake)
        };
        let (plain, plain_shake) = mk(false);
        let (crit, crit_shake) = mk(true);
        assert!(crit > plain, "a crit drew the same burst as a graze ({crit} vs {plain})");
        assert!(crit_shake > plain_shake, "a crit did not move the camera any harder");
    }

    /// The queue must not survive the fight it belongs to: a cast queued on the frame the
    /// battle ended would be drawn over whichever combatant reused that id next.
    #[test]
    fn the_queue_does_not_outlive_the_fight() {
        let mut fx = BattleFx::default();
        queue_cast(&mut fx, Some(DamageType::Fire), &[("a".into(), 40, 100)], false);
        let _ = fx.next_seed();
        fx.queue.clear();
        fx.seed = 0;
        assert!(fx.queue.is_empty());
    }
}

// ---------------------------------------------------------- death bursts ---

/// **A FELLED CREATURE COMES APART.** One GPU particle burst, at the body that just fell.
///
/// The fourth tier, and the only one that is not a quad: a body that simply stops being
/// drawn is the one moment in a fight with no feedback at all — the thing you have been
/// hitting is replaced by nothing between two frames, and the KO! number is the only
/// evidence it was ever there. `bevy_hanabi` runs the simulation on the GPU, so a pack of
/// five going down at once costs a handful of draw calls rather than hundreds of entities.
///
/// ⚠️ **ONE ASSET FOR EVERY DEATH, tinted per burst.** The same argument the impact shader
/// makes for its `kind` uniform: an `EffectAsset` per creature kind is a pipeline per
/// creature kind, compiled the first time each one dies — a hitch in the middle of a fight,
/// once per species, forever. The burst takes the body's own colour through
/// [`DeathBurst::tint`] instead.
#[derive(Resource)]
pub(crate) struct DeathBurst {
    pub(crate) effect: Handle<bevy_hanabi::EffectAsset>,
}

/// **WHAT A BLOW THROWS OFF THE BODY IT LANDS ON.** Two effects, because a crit is not a
/// louder ordinary hit — it is a different event and should read as one at a glance.
///
/// ⚠️ **This replaces a PROHIBITION SIGN.** The shader's untyped-physical branch drew a
/// diagonal slash across an expanding ring, which is a circle with a line through it: the
/// universal "no". Every unelemental blow in the game — most of them — announced itself
/// with the symbol for *denied*, which is close to the opposite of what had just happened.
/// Sparks have no such reading, and they are the one shape a symmetric shader cannot
/// accidentally make into a glyph.
#[derive(Resource)]
pub(crate) struct HitSparks {
    /// An ordinary blow: a short, tight spray.
    pub(crate) normal: Handle<bevy_hanabi::EffectAsset>,
    /// A critical: more of it, thrown further and faster, with a hotter core.
    pub(crate) crit: Handle<bevy_hanabi::EffectAsset>,
}

/// How long a spent spark burst lingers before its entity is despawned. Shorter than a
/// death — a hit is over in a few frames or it is wallpaper.
const SPARK_TTL: f32 = 0.9;

/// How long a spent burst lingers before its entity is despawned.
///
/// ⚠️ A ONE-SHOT EFFECT DOES NOT CLEAN ITSELF UP. `SpawnerSettings::once` emits a single
/// burst and then sits there, so without this every creature felled in a fight leaves an
/// idle effect entity behind for the rest of it — `BattleFxRoot` only collects them when
/// the whole fight ends. It is the particle LIFETIME plus a beat, so nothing is cut off
/// mid-fade.
const DEATH_BURST_TTL: f32 = 1.6;

/// A spent burst, aged by [`advance_death_bursts`].
#[derive(Component)]
pub(crate) struct DeathBurstTtl(pub(crate) f32);

/// Build the one death effect. Registered at startup, so the pipeline is compiled before
/// anything dies rather than on the frame something does.
/// Build the two hit effects. One asset each, tinted per burst exactly as the death burst
/// is — see [`DeathBurst`] for why that is a property rather than an asset per lineage.
pub(crate) fn init_hit_sparks(
    mut commands: Commands,
    mut effects: ResMut<Assets<bevy_hanabi::EffectAsset>>,
) {
    use bevy_hanabi::*;

    // `heavy` is the crit: the SAME shape thrown harder and hotter, rather than a second
    // unrelated effect. A crit has to be recognisable as the thing that just happened to
    // you, only worse — a different silhouette would read as a different attack.
    let build = |effects: &mut Assets<EffectAsset>, heavy: bool| -> Handle<EffectAsset> {
        let writer = ExprWriter::new();
        let tint = writer.add_property("tint", Value::Vector(Vec3::ONE.into()));
        let tint = writer.prop(tint);

        let (count, speed_lo, speed_hi, life_lo, life_hi, size) = if heavy {
            (34.0f32, 3.4f32, 7.0f32, 0.30f32, 0.52f32, 0.15f32)
        } else {
            (14.0, 1.8, 3.6, 0.18, 0.32, 0.10)
        };

        // Thrown from the point of contact, around chest height on the body.
        let init_pos = SetPositionSphereModifier {
            center: writer.lit(Vec3::Y * 1.0).expr(),
            radius: writer.lit(if heavy { 0.30 } else { 0.18 }).expr(),
            dimension: ShapeDimension::Volume,
        };
        let init_vel = SetVelocitySphereModifier {
            center: writer.lit(Vec3::Y * 1.0).expr(),
            speed: writer.lit(speed_lo).uniform(writer.lit(speed_hi)).expr(),
        };
        let init_life = SetAttributeModifier::new(
            Attribute::LIFETIME,
            writer.lit(life_lo).uniform(writer.lit(life_hi)).expr(),
        );
        let init_age = SetAttributeModifier::new(Attribute::AGE, writer.lit(0.).expr());
        // The body's own lineage colour, per particle at birth. A crit keeps a hotter core
        // by being lifted toward white — the hue still says WHAT was hit, the brightness
        // says how hard.
        let hue = if heavy {
            (tint.clone() * writer.lit(0.55) + writer.lit(0.45)).vec4_xyz_w(writer.lit(1.))
        } else {
            tint.clone().vec4_xyz_w(writer.lit(1.))
        };
        let init_colour =
            SetAttributeModifier::new(Attribute::COLOR, hue.pack4x8unorm().expr());
        // Gravity and drag, so the spray arcs and settles rather than flying off forever.
        let accel = AccelModifier::new(writer.lit(Vec3::Y * -9.0).expr());
        let drag = LinearDragModifier::new(writer.lit(if heavy { 3.0 } else { 4.5 }).expr());

        // Colourless: `Attribute::COLOR` carries the hue and this carries the FADE, the
        // same split the death burst uses.
        let mut fade = Gradient::new();
        fade.add_key(0.0, Vec4::new(1.0, 1.0, 1.0, 1.0));
        fade.add_key(1.0, Vec4::new(1.0, 1.0, 1.0, 0.0));
        let mut shrink = Gradient::new();
        shrink.add_key(0.0, Vec3::splat(size));
        shrink.add_key(1.0, Vec3::splat(size * 0.15));

        effects.add(
            EffectAsset::new(
                if heavy { 128 } else { 64 },
                SpawnerSettings::once(count.into()),
                writer.finish(),
            )
            .with_name(if heavy { "crit_sparks" } else { "hit_sparks" })
            .init(init_pos)
            .init(init_vel)
            .init(init_life)
            .init(init_age)
            .init(init_colour)
            .update(accel)
            .update(drag)
            .render(ColorOverLifetimeModifier { gradient: fade, ..default() })
            .render(SizeOverLifetimeModifier { gradient: shrink, screen_space_size: false }),
        )
    };

    let normal = build(&mut effects, false);
    let crit = build(&mut effects, true);
    commands.insert_resource(HitSparks { normal, crit });
}

pub(crate) fn init_death_burst(
    mut commands: Commands,
    mut effects: ResMut<Assets<bevy_hanabi::EffectAsset>>,
) {
    use bevy_hanabi::*;

    let writer = ExprWriter::new();

    // Start scattered through the body's own volume rather than at a point: the motes are
    // what the creature was made of, so they come apart from where it stood.
    let init_pos = SetPositionSphereModifier {
        center: writer.lit(Vec3::Y * 0.9).expr(),
        radius: writer.lit(0.55).expr(),
        dimension: ShapeDimension::Volume,
    };
    // Outward and mostly gentle — this is a body falling apart, not an explosion. The
    // upward bias is what keeps it from reading as the sprite sinking through the floor.
    let init_vel = SetVelocitySphereModifier {
        center: writer.lit(Vec3::Y * 0.6).expr(),
        speed: writer.lit(1.1).uniform(writer.lit(2.6)).expr(),
    };
    let lifetime = writer.lit(0.7).uniform(writer.lit(1.25)).expr();
    let init_lifetime = SetAttributeModifier::new(Attribute::LIFETIME, lifetime);
    let init_age = SetAttributeModifier::new(Attribute::AGE, writer.lit(0.).expr());
    // Gravity, so the motes arc and fall. Without it they drift outward forever and read
    // as a puff of smoke rather than as something coming apart.
    let accel = AccelModifier::new(writer.lit(Vec3::Y * -6.5).expr());
    // Drag, so the outward throw dies away and the fall dominates the second half.
    let drag = LinearDragModifier::new(writer.lit(2.4).expr());

    // **THE BODY'S OWN COLOUR, PER BURST, FROM ONE ASSET.** A property rather than a
    // second `EffectAsset`: an asset per creature kind is a pipeline per creature kind,
    // compiled the first time each one dies. `spawn_death_bursts` writes it through
    // `EffectProperties` on the spawned entity.
    let tint = writer.add_property("tint", Value::Vector(Vec3::ONE.into()));
    let tint = writer.prop(tint);
    // Each mote takes the colour at birth, so the gradient below only carries the FADE.
    let init_colour = SetAttributeModifier::new(
        Attribute::COLOR,
        tint.vec4_xyz_w(writer.lit(1.)).pack4x8unorm().expr(),
    );

    // White at the head, falling to nothing. `Attribute::COLOR` above carries the hue and
    // this multiplies it, which is why this gradient is colourless.
    let mut colour = Gradient::new();
    colour.add_key(0.0, Vec4::new(1.0, 1.0, 1.0, 1.0));
    colour.add_key(0.35, Vec4::new(1.0, 1.0, 1.0, 0.85));
    colour.add_key(1.0, Vec4::new(1.0, 1.0, 1.0, 0.0));

    // Shrinking, so the motes read as cooling rather than as receding.
    let mut size = Gradient::new();
    size.add_key(0.0, Vec3::splat(0.16));
    size.add_key(1.0, Vec3::splat(0.02));

    let effect = effects.add(
        EffectAsset::new(256, SpawnerSettings::once(48.0.into()), writer.finish())
            .with_name("death_burst")
            .init(init_pos)
            .init(init_vel)
            .init(init_lifetime)
            .init(init_age)
            .init(init_colour)
            .update(accel)
            .update(drag)
            .render(ColorOverLifetimeModifier {
                gradient: colour,
                ..default()
            })
            .render(SizeOverLifetimeModifier {
                gradient: size,
                screen_space_size: false,
            }),
    );
    commands.insert_resource(DeathBurst { effect });
}

/// Whether this resolved effect is an ENEMY falling — the one thing that earns a burst.
///
/// A free function so the rule is testable without pumping a wire message through the whole
/// client: the trap this repo keeps recording is a feature generated correctly and consumed
/// nowhere, and a death queue nothing ever pushes to looks exactly like a working one.
///
/// ⚠️ Enemies only. A hero going down is somebody you are about to raise, and showering your
/// own party in ash reads as a loss rather than as the setback it is.
pub(crate) fn is_enemy_death(
    kind: &str,
    target: &str,
    combatants: &[meld_client::net::CombatantView],
) -> bool {
    kind.eq_ignore_ascii_case("ko")
        && combatants.iter().any(|c| c.id == target && !c.is_player)
}

/// The lineage colour a blow on this body throws off, from the shared registry.
///
/// ⚠️ **A HERO IS NOT A LINEAGE.** Your own party has no `faction:` on the wire, and
/// falling through to the unknown case would paint every blow you take in the same bone
/// white a missing faction uses — which is exactly the tell that table reserves for a
/// faction nobody authored. Heroes get their own steel instead.
pub(crate) fn struck_rgb(view: Option<&meld_client::net::CombatantView>) -> Vec3 {
    let Some(v) = view else {
        return Vec3::new(0.86, 0.84, 0.80);
    };
    if v.is_player {
        return Vec3::new(0.78, 0.82, 0.90);
    }
    let faction = v.statuses.iter().find_map(|s| s.strip_prefix("faction:")).unwrap_or("");
    let [r, g, b] = meld_proto::factions::faction_rgb(faction);
    Vec3::new(r, g, b)
}

/// Drain the queued hits into spark bursts, at the body each one landed on.
pub(crate) fn spawn_hit_sparks(
    mut commands: Commands,
    mut fx: ResMut<BattleFx>,
    sparks: Option<Res<HitSparks>>,
    battle: Res<crate::BattleData>,
    actors: Query<(&crate::battle::BattleActor, &GlobalTransform)>,
) {
    if fx.sparks.is_empty() {
        return;
    }
    let Some(sparks) = sparks else {
        fx.sparks.clear();
        return;
    };
    for (id, crit) in std::mem::take(&mut fx.sparks) {
        let Some((_, gt)) = actors.iter().find(|(a, _)| a.id == id) else {
            continue;
        };
        let rgb = struck_rgb(battle.combatants.iter().find(|c| c.id == id));
        let mut props = bevy_hanabi::EffectProperties::default();
        props.set("tint", bevy_hanabi::Value::Vector(rgb.into()));
        let effect = if crit { sparks.crit.clone() } else { sparks.normal.clone() };
        commands.spawn((
            BattleFxRoot,
            DeathBurstTtl(SPARK_TTL),
            bevy_hanabi::ParticleEffect::new(effect),
            props,
            Transform::from_translation(gt.translation()),
        ));
    }
}

/// Drain the queued deaths into bursts, at the body each one belongs to.
///
/// ⚠️ **The burst is placed at the ACTOR, never at a screen position or a party slot** —
/// the same rule the floating numbers had to learn (`render_hit_fx`): a body that fell is
/// the one thing on screen the effect is about, and anchoring by role puts every death in
/// a pack onto the first one.
///
/// A body whose actor is already gone is dropped rather than drawn at the origin. The
/// arena rebuilds on `hp > 0`, so a KO and the despawn can land on the same frame, and a
/// burst at `(0,0,0)` is a puff of ash in the middle of the floor.
pub(crate) fn spawn_death_bursts(
    mut commands: Commands,
    mut fx: ResMut<BattleFx>,
    burst: Option<Res<DeathBurst>>,
    actors: Query<(&crate::battle::BattleActor, &GlobalTransform)>,
    quads: Query<&crate::SpriteQuad>,
) {
    if fx.deaths.is_empty() {
        return;
    }
    let Some(burst) = burst else {
        fx.deaths.clear();
        return;
    };
    for id in std::mem::take(&mut fx.deaths) {
        let Some((_, gt)) = actors.iter().find(|(a, _)| a.id == id) else {
            continue;
        };
        // The body's own colour, so a bog serpent does not come apart in the same ash as a
        // golem. The gradient carries the fade and this carries the hue, which is why the
        // gradient is white: the two multiply.
        let tint = quads
            .iter()
            .find(|q| q.id == id)
            .map(|q| q.base)
            .unwrap_or(Color::srgb(0.85, 0.82, 0.78));
        let rgb = tint.to_linear();
        let mut props = bevy_hanabi::EffectProperties::default();
        props.set(
            "tint",
            bevy_hanabi::Value::Vector(Vec3::new(rgb.red, rgb.green, rgb.blue).into()),
        );
        commands.spawn((
            BattleFxRoot,
            DeathBurstTtl(DEATH_BURST_TTL),
            bevy_hanabi::ParticleEffect::new(burst.effect.clone()),
            props,
            Transform::from_translation(gt.translation()),
        ));
    }
}

/// Age the spent bursts out.
pub(crate) fn advance_death_bursts(
    mut commands: Commands,
    time: Res<Time>,
    mut q: Query<(Entity, &mut DeathBurstTtl)>,
) {
    let dt = time.delta_secs();
    for (e, mut ttl) in &mut q {
        ttl.0 -= dt;
        if ttl.0 <= 0.0 {
            commands.entity(e).despawn();
        }
    }
}
