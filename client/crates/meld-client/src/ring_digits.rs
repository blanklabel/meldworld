//! **THE HP NUMBER, PRINTED ON THE GLASS TUBE ITSELF.**
//!
//! A combatant's health is a ring of liquid on the ground at its feet
//! ([`crate::battle_rings`]) and the number belongs IN that stroke — one readout, one object,
//! one piece of ground. Getting it there was screen-space text for a long time: the ring was
//! projected to the viewport, a font size was derived from the projected arc, and each glyph
//! was placed and rotated by sampling the projection either side of itself.
//!
//! ⚠️ **THAT VERSION WAS STEERING TEXT TO LOOK AS THOUGH IT WERE ON THE TUBE, AND IT SHOWED.**
//! Every property the number should have had for free was a calculation that could be wrong,
//! and several of them were, in order: the glyphs straddled the rim (`TEXT_RADIUS` is the
//! tube's CENTRE-LINE, where nothing is drawn), so the offset had to become a camera-derived
//! direction rather than a height; the rotation had to be taken from the PROJECTION rather
//! than from the angle, because a ring on the ground is an ellipse on screen and the tangent
//! at an angle is not that angle; and the whole readout had to be rebuilt whenever the camera
//! or any body moved, since a screen-space node is only correct for the frame it was placed
//! in.
//!
//! A `ForwardDecal` is the answer the module note next door has recorded as "better" for a
//! while: the number is PROJECTED onto whatever is under it, so it takes the tube's curvature
//! from the tube instead of being told about it, and it is a world-space object — it cannot
//! drift when the camera moves, because it is not placed relative to the camera at all.
//!
//! ⚠️ **AND THE BLOCKER ON IT WAS STALE.** A forward decal needs a `DepthPrepass`, which this
//! camera deliberately lacked because the ground's displacement had no prepass vertex stage
//! and the prepass rasterised the world flat. `ground_prepass.wgsl` was written later for an
//! unrelated reason — the terrain was shadowing itself with that same undisplaced sheet — and
//! it applies exactly the same `total_height`. The prerequisite was met the day that file
//! landed; the note saying otherwise outlived its own cause. See `hd2d::spawn_camera`.
//!
//! ⚠️ **ONE DECAL PER GLYPH, NOT ONE PER NUMBER.** A single quad carrying the whole label
//! would be half the draw calls and is the obvious shape — and a flat quad over a CURVED arc
//! only touches the tube in the middle, so the ends of the number would hang off into the
//! hole and smear down the ground. Per glyph, each quad sits over its own short stretch of
//! tube, which is nearly flat; following the arc is then what the placement does and wrapping
//! the tube's cross-section is what the projection does, and neither is a calculation that
//! can be subtly wrong.

use bevy::asset::Asset;
use bevy::prelude::*;
use bevy::reflect::TypePath;
use bevy::render::render_resource::AsBindGroup;
use bevy::shader::ShaderRef;

use crate::battle_rings::{self, CombatantRing};
use crate::BattleData;

/// The glyphs, in order. ⚠️ **THIS STRING IS THE ABI.** `client/scripts/make_digit_atlas.py`
/// bakes `assets/fx/digits/<i>.png` from exactly this sequence and a glyph is picked by nothing
/// but its index, so a character inserted on one side and not the other draws its neighbour —
/// silently, and only on numbers that happen to contain it.
pub(crate) const CHARS: &str = "0123456789/";

/// How many glyph quads each body carries. `1042/1042` is a level-100 Phoenix Guard, which is
/// the widest label the game can produce; extras are hidden rather than despawned, because
/// spawning into the world on the frame a number grows a digit is a hitch on a hit.
pub(crate) const MAX_GLYPHS: usize = 9;

/// The world size of one glyph quad. The atlas leaves a wide transparent margin inside each
/// cell, so the ink is about four fifths of this — a glyph roughly 0.21 across a tube 0.27
/// thick, filling the crown without running over either rim.
///
/// ⚠️ **AND THE QUAD MAY NOT BE WIDER THAN THE TUBE, WHICH IS NOT THE SAME AS THE GLYPH
/// FITTING.** At 0.34 the ink fitted comfortably and the numbers still came out with their
/// edges eaten: the parts of the QUAD hanging past the tube find the ground half a unit below
/// instead, and a forward decal shifts its UVs by exactly that gap — so the overhang samples
/// some far-off part of the cell and prints it back over the glyph. The quad has to land on
/// the tube along its whole width; the ink is then made to fill more of the cell rather than
/// the quad being made bigger, which is why the atlas bakes at 92px and not 74.
///
/// ⚠️ **THE MARGIN IS DOING MORE WORK THAN THE DOCS ASK FOR.** Bevy wants padding because a
/// decal seen at a steep angle smears its edge texels. Here it also covers a gap in the
/// pipeline: this camera runs Order-Independent Transparency, and under OIT `pbr.wgsl` hands
/// the fragment to `oit_draw` and DISCARDS it before the decal's own edge-fade alpha is
/// applied — so the fade that normally stops a decal printing on distant geometry never runs
/// at all. What keeps these digits on the tube rather than smeared down the ground beside it
/// is that a fragment which misses the tube lands in transparent margin.
const CELL: f32 = 0.185;
/// World arc length between glyph centres. Narrower than `CELL`, because the cell is mostly
/// margin: spaced by the cell the digits read as a number with gaps punched through it.
const ADVANCE: f32 = 0.105;
/// How far clear of the tube's surface the quads stand.
///
/// ⚠️ **IT USED TO HAVE TO BE TINY, AND FACING THE QUADS AT THE CAMERA BOUGHT THAT BACK.** A
/// forward decal shifts its UVs by the gap between its own fragment and what the depth buffer
/// holds under it, scaled by how far the view leans across the quad — so for a quad lying FLAT
/// the standoff was pure parallax and a glyph lifted clear of the tube slid across it as the
/// camera moved. A quad that faces the camera is very nearly perpendicular to the view, which
/// drives that lean to zero: the standoff is almost free now, and it is worth spending, because
/// a quad grazing the surface it prints on is a quad the surface can eat.
const CLEARANCE: f32 = 0.035;

// **THE GEOMETRY RULES, ASSERTED AT COMPILE TIME** rather than in a test: every term is a
// constant, so there is nothing to run and nothing to remember to run. (`glass.rs` holds its
// column fractions this way, and the resource hoop next door its clearances.)
//
// ⚠️ The quads must hover just OVER the tube's crown. Buried inside it there is no surface
// under them to project onto; lifted clear they print on the ground beyond it instead.
const _: () = assert!(CLEARANCE > 0.0);
// ⚠️ **THE STANDOFF IS NOT FREE, AND MEASURING IT IS THE ONLY WAY TO KNOW.** Facing the quads
// at the camera makes them nearly perpendicular to the view, which ought to drive the decal's
// UV shift to nothing — so the standoff was raised to 0.13 to lift the glyphs clear of the tube
// they print on. Rendered, that LOST digits rather than saving them: the standoff runs in the
// tube's cross-section, which is outward as much as it is toward the viewer, so a large one
// slides the quad off the tube in screen space and leaves it printing on the ground beyond.
// It has to be just enough to clear the surface and no more.
const _: () = assert!(CLEARANCE < battle_rings::RING_MINOR * 0.35);
// ⚠️ The quad must sit INSIDE the tube's own width. Anything hanging past it finds the ground
// far below, and the decal's UV shift is that distance — so an overhanging edge prints a
// fragment of some other part of the glyph back over the number. Measured: at a quad 26% wider
// than the tube, every digit came out with its edges eaten.
// The widest label has to FIT on the arc facing the camera, or a nine-digit number wraps
// around the back of the body standing in front of it.
const _: () =
    assert!((MAX_GLYPHS as f32 - 1.0) * ADVANCE < std::f32::consts::PI * battle_rings::TEXT_RADIUS * 0.75);
// …and the glyphs must not be spaced wider than their own cells, or the number reads as
// separate digits rather than as one figure.
const _: () = assert!(ADVANCE < CELL);

/// One glyph quad, and which body and position in the label it belongs to.
#[derive(Component)]
pub(crate) struct RingDigit {
    pub(crate) id: String,
    pub(crate) slot: usize,
    /// Which cell of the atlas this quad is currently showing, so the material handle is
    /// swapped only when the character actually changes rather than every frame.
    pub(crate) shown: Option<usize>,
}

/// **OUR OWN DECAL MATERIAL, NOT `bevy_pbr`'s.** One glyph, projected orthographically along
/// this quad's normal onto whatever the depth prepass has behind it — see `ring_digit.wgsl`
/// for the line of Bevy's own decal shader that made this necessary, and the four rounds of
/// constant-tuning that did not.
#[derive(Asset, AsBindGroup, TypePath, Clone)]
pub(crate) struct RingDigitDecal {
    #[texture(0)]
    #[sampler(1)]
    pub(crate) glyph: Handle<Image>,
}

impl Material for RingDigitDecal {
    fn fragment_shader() -> ShaderRef {
        "shaders/ring_digit.wgsl".into()
    }
    /// Blended, so the glyph sits on the tube rather than punching a hole in it — and so it
    /// writes no depth, which keeps overlapping glyph quads from occluding one another.
    fn alpha_mode(&self) -> AlphaMode {
        AlphaMode::Blend
    }
    /// ⚠️ **A DECAL HAS TO WIN AGAINST THE THING IT IS PRINTED ON.** These quads stand
    /// `CLEARANCE` off the tube's surface, and 0.035 of a world unit is nothing at the far
    /// rank — so the trailing digits of a distant creature's number were resolved against the
    /// tube behind them and simply never drawn.
    ///
    /// ⚠️ **AND IT WAS FOUND BY MAKING THE SHADER SAY WHAT IT WAS DOING, not by more guessing.**
    /// Painting the out-of-box and no-depth branches in flat colours showed the far creature had
    /// no quad fragments there AT ALL — neither on target nor missing — and a one-shot log then
    /// proved placement was fine (`label="44/60" placed=[0,1,2,3,4]`). Between those two facts
    /// the answer can only be that the fragments were killed before the shader ran. A/B'd in ONE
    /// binary at one window size, because the two captures that first suggested it had been
    /// taken at different resolutions and depth precision is exactly what was in question:
    /// bias 0 draws `44/6`, bias 1000 draws `44/60`.
    fn depth_bias(&self) -> f32 {
        1000.0
    }
}

/// The eleven materials, one per character, built once and shared by every glyph in the arena.
///
/// ⚠️ **A HANDLE SWAP, NOT A MATERIAL WRITE.** The obvious shape is one material per glyph
/// whose texture is changed as the number moves — and `Assets::get_mut` flags the asset
/// modified and the render world rebuilds its bind group, so that is up to sixty-three bind
/// group rebuilds a frame for a readout that changes a few times a second. Eleven immutable
/// materials and a `MeshMaterial3d` swap costs nothing when the digit holds still.
#[derive(Resource)]
pub(crate) struct DigitAtlas {
    pub(crate) chars: Vec<Handle<RingDigitDecal>>,
    /// The quad every glyph is drawn on: a unit rectangle in its own XY plane, so the shader's
    /// `u`/`v` are the transform's first two columns and the normal is the third.
    pub(crate) quad: Handle<Mesh>,
}

/// Build the atlas materials. One per character; the glyph is selected by a `uv_transform`
/// that scales the atlas down to a single cell and slides it along.
pub(crate) fn load_digit_atlas(
    mut commands: Commands,
    assets: Res<AssetServer>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut mats: ResMut<Assets<RingDigitDecal>>,
) {
    // ⚠️ **ONE TEXTURE PER GLYPH, NOT ONE STRIP AND A `uv_transform`.** A strip is the obvious
    // and cheaper shape, and the projection can land a fragment slightly outside its own cell —
    // on a strip that means sampling the NEIGHBOURING glyph, which came back as digits wearing
    // slivers of their neighbours. Separate textures make it unreachable rather than unlikely:
    // Bevy's sampler clamps to edge, so an out-of-range coordinate can only ever find this
    // glyph's own transparent border, because there is nothing else in the image to find.
    let chars = (0..CHARS.chars().count())
        .map(|i| mats.add(RingDigitDecal { glyph: assets.load(format!("fx/digits/{i}.png")) }))
        .collect();
    commands.insert_resource(DigitAtlas {
        chars,
        quad: meshes.add(Rectangle::from_size(Vec2::ONE)),
    });
}

/// Spawn one body's glyph quads, **as children of the RING**.
///
/// ⚠️ **THAT PARENT IS LOAD-BEARING, TWICE OVER.** The ring is the thing the number describes,
/// so hanging the digits off it means they follow the body's lunge by the same route the bar
/// does (`ring_follows_body`) instead of by a second copy of that rule — and it means
/// `drive_rings` hiding the ring on a felled body hides the number with it, through ordinary
/// visibility inheritance. This file's neighbour records what the alternative costs: the
/// health ring's old glass shell was a second entity beside the one that system drove, so
/// every corpse kept an empty bar at its feet for the rest of the fight.
///
/// ⚠️ The ring carries no rotation and no scale (`Transform::from_xyz` in `spawn_ring`), which
/// is what lets [`place_ring_digits`] write a world-space bearing straight into a local
/// transform. If it ever gains one, this is the first thing that silently goes crooked.
pub(crate) fn spawn_ring_digits(
    parent: &mut ChildSpawnerCommands,
    atlas: &DigitAtlas,
    id: &str,
) {
    for slot in 0..MAX_GLYPHS {
        parent.spawn((
            RingDigit { id: id.to_string(), slot, shown: None },
            Mesh3d(atlas.quad.clone()),
            MeshMaterial3d(atlas.chars[0].clone()),
            Transform::from_scale(Vec3::splat(CELL)),
            // Hidden until `place_ring_digits` has a character for it — a body with a
            // four-character label must not flash five zeroes on the frame it spawns.
            Visibility::Hidden,
        ));
    }
}

/// What a combatant's ring is currently SHOWING, as the label the digits spell.
///
/// ⚠️ **THE NUMBER IS WHAT THE BAR IS SHOWING, NOT WHAT THE WIRE SAYS.** The meter rolls
/// (EarthBound's), so reading `hp` here would land the digits on the new value while the
/// liquid behind them was still counting toward it — two readouts of one fact disagreeing for
/// the whole of every hit.
pub(crate) fn ring_label(shown: f32, c: &meld_client::net::CombatantView) -> String {
    format!("{}/{}", battle_rings::shown_hp(shown, c), c.max_hp)
}

/// Lay every body's digits along the front of its own ring.
pub(crate) fn place_ring_digits(
    battle: Res<BattleData>,
    atlas: Option<Res<DigitAtlas>>,
    cam: Query<&GlobalTransform, With<Camera3d>>,
    rings: Query<(&CombatantRing, &GlobalTransform)>,
    mut digits: Query<(
        &mut RingDigit,
        &mut Transform,
        &mut Visibility,
        &mut MeshMaterial3d<RingDigitDecal>,
    )>,
) {
    let Some(atlas) = atlas else { return };
    let Some(eye) = cam.iter().next().map(|t| t.translation()) else { return };
    for (mut digit, mut tf, mut vis, mut mat) in &mut digits {
        let placed = place_one(&battle, &rings, eye, &digit);
        let Some((at, rot, cell)) = placed else {
            if *vis != Visibility::Hidden {
                *vis = Visibility::Hidden;
            }
            continue;
        };
        if *vis != Visibility::Inherited {
            *vis = Visibility::Inherited;
        }
        if digit.shown != Some(cell) {
            digit.shown = Some(cell);
            mat.0 = atlas.chars[cell].clone();
        }
        // ⚠️ Written only when it moved: a `DerefMut` on a `Transform` re-propagates and
        // re-extracts whether or not the value changed, and there are up to nine of these per
        // body standing perfectly still between turns.
        if tf.translation != at || tf.rotation != rot {
            tf.translation = at;
            tf.rotation = rot;
        }
    }
}

/// Where one glyph goes, which way it faces, and which atlas cell it is — or `None` when this
/// slot is past the end of its body's label, or the body is gone.
fn place_one(
    battle: &BattleData,
    rings: &Query<(&CombatantRing, &GlobalTransform)>,
    eye: Vec3,
    digit: &RingDigit,
) -> Option<(Vec3, Quat, usize)> {
    let c = battle.view(&digit.id)?;
    if c.hp <= 0 {
        return None;
    }
    let (ring, ring_tf) = rings.iter().find(|(r, _)| r.id == digit.id)?;
    let label = ring_label(ring.shown, c);
    let ch = label.chars().nth(digit.slot)?;
    let cell = CHARS.chars().position(|k| k == ch)?;

    // The arc nearest the camera, in the ring's own frame. ⚠️ Taken from the RING rather than
    // the actor root: the ring follows the body's lunge and the root does not move at all, so
    // anchoring on the root writes the number where the bar used to be for the whole of every
    // swing.
    let centre = ring_tf.translation();
    let front = (eye - centre).with_y(0.0).try_normalize()?;
    // ⚠️ Everything below is in the RING's own frame, which is this glyph's parent: the ring
    // has no rotation and no scale, so a world bearing is a local bearing and the only thing
    // that has to be converted is the height.
    // Evenly spaced along the arc, centred on the front. The step is an ANGLE because the
    // glyphs sit on a circle — `ADVANCE` is the chord, and at this radius the two agree to
    // well under a pixel.
    let n = label.chars().count();
    let theta =
        (digit.slot as f32 - (n as f32 - 1.0) * 0.5) * (ADVANCE / battle_rings::TEXT_RADIUS);
    let dir = Quat::from_rotation_y(theta) * front;
    // Above the tube's CROWN — the ring's origin is the tube's own centre-line, so the crown
    // is one minor radius up. The quad projects straight DOWN, so this is the only height that
    // matters: everything about where the ink lands on the curve is the projection's problem
    // now, which is the whole reason this is a decal rather than text aimed at a tube.
    // ⚠️ **THE QUAD FACES THE CAMERA; IT DOES NOT LIE FLAT.** A decal stamps along its own
    // NORMAL, so a quad lying flat on the ground projects straight DOWN and can only ever mark
    // the tube's CROWN — the topmost line of it, which faces the sky. The camera looks at the
    // tube's SIDE, so half of every glyph landed on surface turned away from the viewer, where
    // the body standing in the ring hides it. Reported from play as *"you're too high up on the
    // tube, it's rolling over to the other side"*, which is exactly what it was.
    //
    // ⚠️ **AND SLIDING A FLAT QUAD OUTWARD CANNOT FIX IT**, which is worth knowing before
    // somebody tries it: a downward ray only meets the tube while it is within one minor radius
    // of the centre-line, so every unit the glyph moves outward is a unit of room it loses
    // before falling off the outer silhouette onto the ground. At the offset that would
    // actually centre it on the visible band there is room for a glyph a third of the size.
    //
    // Facing the quad AT the camera removes the constraint instead of trading against it: the
    // stamp then runs along the view, so the ink lands wherever the quad covers ON SCREEN and
    // the tube's whole apparent width is available to it. It also arrives UNFORESHORTENED — a
    // number painted flat on ground seen at this pitch is squashed to about two thirds of its
    // height before anything else happens to it. The ink still wraps the tube's curvature,
    // because that is the surface it lands on; what changed is the direction it is thrown from.
    let spine = dir * battle_rings::TEXT_RADIUS;
    let face = (eye - (centre + spine)).try_normalize().unwrap_or(Vec3::Y);
    // ⚠️ **STOOD OFF THE TUBE IN ITS OWN CROSS-SECTION, NOT ALONG THE FULL VIEW DIRECTION.**
    // `face` is a 3-D bearing and for any glyph off the near point it leans along the ARC as
    // well as across the tube — so standing the quad off by one minor radius along it moves the
    // quad partly sideways down the tube and leaves it less than a radius clear of the surface.
    // It then sinks into the tube and the tube's own near wall eats whatever is behind it,
    // which is the leading digits going missing while `/40` stayed perfect. The tube is a
    // circle in the plane spanned by `dir` and up, so the clearance has to be measured there.
    let across = (dir * face.dot(dir) + Vec3::Y * face.y).try_normalize().unwrap_or(Vec3::Y);
    let at = spine + across * (battle_rings::RING_MINOR + CLEARANCE);

    // The quad's own frame: +X along the arc (the glyph's own left-to-right), +Y up (it is the
    // quad's normal, and a decal projects along it), +Z radially outward.
    //
    // Bevy's decal quad is a `Rectangle` in XY turned by `from_rotation_arc(Z, Y)`, so its
    // local +Y lands on -Z; a texture's `v` grows DOWNWARD from the image's top, which puts
    // the glyph's TOP at local -Z. Outward is toward the camera, and for something lying on
    // the ground the direction AWAY from the camera is up the screen — so the glyph's top
    // wants to face inward, which local -Z is. `tangent x up = dir`, so the three columns are
    // a right-handed orthonormal basis by construction rather than by a sign somebody guessed.
    //
    // ⚠️ **AND I "FIXED" THIS ONCE WHEN IT WAS ALREADY RIGHT.** The first capture looked
    // rotated a half turn, so both axes were negated — which IS a real half turn about the
    // ring's up axis, and made it genuinely wrong. What that capture was actually showing was
    // DEPTH OF FIELD, switched on by accident by the very prepass these decals need (see
    // `hd2d::spawn_camera`), blurring a twenty-pixel number past reading. Settled in the end
    // by sweeping the sign in one process with the glyphs temporarily blown up fivefold,
    // where `1 / 4` is unambiguous. **A blurred thumbnail is not evidence**; a sign is settled
    // by rendering something big enough to read, which is the discipline `SPIN` and the
    // command wheel's yaw are both recorded under one file over.
    // ⚠️ The tangent has to be squared up against the facing direction first: the two are only
    // perpendicular for a glyph dead ahead of the camera, and handing a skewed pair to
    // `from_mat3` builds a matrix that is not a rotation — which comes back as a quietly
    // mangled quaternion rather than as an error.
    let tangent = Vec3::Y.cross(dir);
    let u = (tangent - face * tangent.dot(face)).try_normalize()?;
    // The quad is a plain rectangle in its own XY plane, so +X is the glyph's left-to-right,
    // +Y is its up, and +Z is the normal it projects along. `u x (face x u) = face` for a unit
    // `u` perpendicular to `face`, so these three are orthonormal and right-handed by
    // construction rather than by a sign somebody guessed.
    let rot = Quat::from_mat3(&Mat3::from_cols(u, face.cross(u), face));
    Some((at, rot, cell))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **THE ATLAS AND THE BAKER AGREE ABOUT WHAT IS IN IT.** The glyph is picked by index, so
    /// a character added to one side alone draws its neighbour on every number containing it —
    /// and nothing at run time would say so.
    #[test]
    fn the_atlas_is_the_one_the_script_bakes() {
        let script = include_str!("../../../scripts/make_digit_atlas.py");
        let line = script
            .lines()
            .find(|l| l.trim_start().starts_with("CHARS = "))
            .expect("the baker no longer declares CHARS");
        let baked = line.split('"').nth(1).expect("CHARS is not a plain string literal");
        assert_eq!(baked, CHARS, "the atlas is baked from a different set than the ring reads");
    }

    /// Every character a label can contain has to be IN the atlas, or a number silently loses
    /// a digit — `place_one` returns `None` for an unknown one, which hides that glyph.
    #[test]
    fn every_character_a_label_can_hold_is_in_the_atlas() {
        for hp in [0, 1, 7, 42, 999, 1042] {
            for max in [1, 40, 60, 1042] {
                let label = format!("{hp}/{max}");
                for ch in label.chars() {
                    assert!(CHARS.contains(ch), "{label} needs {ch:?}, which the atlas has not");
                }
                assert!(
                    label.chars().count() <= MAX_GLYPHS,
                    "{label} is wider than the {MAX_GLYPHS} quads a body carries"
                );
            }
        }
    }

}
