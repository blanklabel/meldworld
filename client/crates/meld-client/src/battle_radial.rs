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
use bevy::light::NotShadowCaster;
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
pub(crate) const W_IN: f32 = 0.56;
pub(crate) const W_OUT: f32 = 0.97;
/// `WorldAssets::shadow_mesh` is a `Circle::new(0.7)`; the wheel borrows it exactly as the
/// health ring does rather than carrying a third disc.
const MESH_RADIUS: f32 = 0.7;
/// How big the wheel is in world units.
///
/// ⚠️ **ITS INNER WALL MUST CLEAR THE HEALTH RING'S OUTER ONE.** `W_IN × 0.7 × scale` has to
/// exceed the feet ring's outer wall (`RING_MAJOR + RING_MINOR` = 0.915) or the wheel is drawn
/// straight over the bar, which at 2.05 left the health ring as a thin green thread under a
/// dark plate.
///
/// ⚠️ **A THICKER BAND WAS ASKED FOR AND IT IS NOT AVAILABLE AT THIS SIZE — I TRIED.** The
/// outer wall cannot help (`W_OUT` is in mesh units and 0.97 is already the disc's own edge),
/// so the only way to thicken is to shrink the hole and grow the wheel to keep the inner wall
/// off the health ring. At `W_IN` 0.46 and scale 2.95 the band really is ~28% thicker — and the
/// outer wall lands 2.00 world units out, against heroes standing 2.7 apart. Reported at once
/// as *"the circle is TOO huge now"*, and it also made the wedges so large that an icon at a
/// sector's true centre read as lost in it rather than centred on it.
///
/// **The band is squeezed between two fixed things**: the health ring's outer wall at 0.915 and
/// the next hero at 2.7. ⚠️ **And shrinking the wheel spends the band, because only the OUTER
/// wall can move** — the hole is pinned where it is by the health ring under it, so every unit
/// the wheel comes in is a unit off the button's depth. At 2.40 the inner wall is 0.941 (clear
/// by 0.026, the tightest number here) and the band is 0.689: a smaller, calmer wheel that sits
/// well clear of the hero either side, bought with ~16% of the button's thickness. That is the
/// whole trade, in one place, for whoever turns this dial next.
///
/// ⚠️ **IT STANDS OUTSIDE THE HEALTH RING, WHICH THE REFERENCE DOES NOT.** In the art the
/// health arcs are the wheel's own outer rim. Nesting it that way is not merely tight here, it
/// is impossible: the health band's inner wall is 0.567 world units out, so a wheel inside it
/// caps at about a third of this scale — roughly 73px of screen radius for five words. The two
/// rings are concentric instead, and neither is a caption on the other.
const WHEEL_SCALE: f32 = 2.40;
/// Where the wheel starts before it opens: its outer wall exactly on the health ring's, so the
/// tiles are seen to PUSH the circle out rather than to appear beside it.
///
/// ⚠️ **THE GROWTH IS THE TELL.** `0.97 × 0.7 × 1.348 = 0.915`, which is the health band's own
/// outer radius (`RING_MAJOR + RING_MINOR`) — pick this number by arithmetic from that one, not
/// by eye, or the wheel opens out of thin air a few pixels off the bar and the whole gesture
/// reads as two rings blinking rather than as one becoming the other.
const WHEEL_SHUT_SCALE: f32 = 1.348;
/// How long the push takes. Long enough to be a movement the eye catches on its own, short
/// enough that it is never between you and an order you already knew you wanted.
const WHEEL_OPEN_SECS: f32 = 0.22;
/// **THE CENTRE OF A SECTOR AS THE PLAYER SEES IT**: the area centroid of its PROJECTED
/// outline, by the shoelace formula over points sampled round its own boundary.
///
/// ⚠️ **THE PROJECTION OF THE CENTROID IS NOT THE CENTROID OF THE PROJECTION, and that is the
/// whole bug.** The wheel lies on the ground under a perspective camera, so a sector's near
/// half is magnified and its far half compressed — the shape on screen is not a scaled copy of
/// the shape on the ground, and no point computed in the GROUND plane lands where the eye puts
/// the middle. Three tries died on that: the mid-radius, a weighting toward the outer wall, and
/// the exact planar area centroid `(2/3)·(b³−a³)/(b²−a²)·sin(α)/α`, which is right about a
/// shape nobody is looking at. The corner average failed for a second reason on top — a sector
/// is not a quadrilateral, its arcs bulge.
///
/// So the outline is walked in the ground plane, each point is projected, and the centroid is
/// taken in SCREEN space where the answer is wanted. `ARC_STEPS` per arc is enough that the
/// bulge is represented; the cost is ~34 projections per wedge on a panel that already projects
/// five points per wedge, and it is exact for any camera, any band, any wedge count.
fn sector_centre(
    feet: Vec3,
    front: Vec3,
    turns: f32,
    scale: f32,
    project: impl Fn(Vec3) -> Option<Vec2>,
) -> Option<Vec2> {
    const ARC_STEPS: usize = 16;
    let edge = half_wedge() * TILE_INSET;
    let (r_in, r_out) = band_radii(scale);
    let mut poly: Vec<Vec2> = Vec::with_capacity(ARC_STEPS * 2 + 2);
    // Out along the near wall, back along the far one: one closed ring, wound consistently.
    for i in 0..=ARC_STEPS {
        let t = turns - edge + (2.0 * edge) * (i as f32 / ARC_STEPS as f32);
        poly.push(project(wheel_point(feet, front, t, r_out, scale))?);
    }
    for i in 0..=ARC_STEPS {
        let t = turns + edge - (2.0 * edge) * (i as f32 / ARC_STEPS as f32);
        poly.push(project(wheel_point(feet, front, t, r_in, scale))?);
    }
    // Shoelace. ⚠️ A degenerate outline — the wheel seen exactly edge-on, or a sector entirely
    // behind the camera — has zero area, and dividing by it would fling the label to infinity.
    let mut area = 0.0f32;
    let mut c = Vec2::ZERO;
    for i in 0..poly.len() {
        let (p, q) = (poly[i], poly[(i + 1) % poly.len()]);
        let cross = p.x * q.y - q.x * p.y;
        area += cross;
        c += (p + q) * cross;
    }
    (area.abs() > 1.0).then(|| c / (3.0 * area))
}

/// The line the tiles sit on: **the AREA CENTROID of the sector**, not a weighting of its walls.
///
/// ⚠️ **A SECTOR'S MIDDLE IS NOT ITS MIDDLE RADIUS, AND NO AMOUNT OF TUNING MAKES IT ONE.** An
/// annular sector has more of itself near its outer wall — the arc out there is longer — so a
/// label at `(W_IN + W_OUT)/2` sits visibly inside the shape it is naming. That is what *"all
/// the action items are off centre"* was. Two guesses were tried and both were guesses: a
/// weighting toward the outer wall, then the average of the sector's four projected corners,
/// which is the centroid of a QUADRILATERAL and the sector is not one — the arcs bulge.
///
/// The closed form is standard and exact:
/// `r_c = (2/3) · (b³ − a³)/(b² − a²) · sin(α)/α` for an annular sector of radii `a..b` and
/// half-angle `α`. Every term is already a constant of this wheel, so the centre is derived
/// rather than tuned, and it stays correct if the band is made thicker or a verb is added.
pub(crate) fn label_radius(scale: f32) -> f32 {
    let r = MESH_RADIUS * scale;
    let (a, b) = (W_IN * r, W_OUT * r);
    // Half the sector, in radians. `half_wedge` is in turns.
    let alpha = half_wedge() * std::f32::consts::TAU;
    (2.0 / 3.0) * (b.powi(3) - a.powi(3)) / (b.powi(2) - a.powi(2)) * (alpha.sin() / alpha)
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
/// ⚠️ **ZERO, AND IT HAS TO BE.** Setting the wheel back keeps its near wedge on screen and
/// costs the only thing the shape is for: the hero stops being INSIDE its own menu and ends up
/// standing at the front edge of a ring drawn behind it. The room it was buying is bought by
/// the CAMERA instead, which lifts the party off the bottom edge while somebody is being asked
/// — the fix that does not deform the thing being drawn.
const WHEEL_BACK: f32 = 0.0;

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
const TILE_MIN_H: f32 = 62.0;
const TILE_MAX_H: f32 = 132.0;
/// How much of its own wedge a tile takes, leaving the wheel's gaps and walls showing round it.
const TILE_INSET: f32 = 0.88;
/// How far up-screen a label sits from its wedge's own centre.
///
/// ⚠️ **ZERO: A LABEL SITS ON ITS BAND.** Any lift at all reads as the word floating off the
/// tile it names, and at this radius a uniform one put every verb near the outer wall of its
/// own wedge — reported from play as the action items being off centre. The near wedge's
/// crowding against the HP digits is answered by the digits' own radius, not by shoving the
/// menu around.
/// **THE ICON IS A FIXED SIZE, AND THAT IS THE POINT.**
///
/// ⚠️ **IT USED TO BE FITTED TO ITS SECTOR EVERY FRAME AND THAT WAS THE BUG.** Reported from
/// play: *"when I scale out or scale in with the camera, the text moves."* The first answer was
/// to make the glyph track the projection exactly — which does stop it drifting relative to its
/// wedge, and leaves it growing and shrinking under the player's hand as the camera breathes.
/// Neither is what a BUTTON does. A control holds still: same size, same place, whatever the
/// camera is doing. So the size is a constant and the position is the sector's own centre, and
/// the only thing the zoom moves is the wheel underneath it.
///
/// ⚠️ The trade is real and bounded: zoom far enough out and fixed-size icons crowd a shrinking
/// wheel. The battle camera auto-fits to the party rather than being free, so that range is
/// small — if a future camera opens it up, this is the constant that has to become a clamp.
const GLYPH_PX: f32 = 33.0;

const LABEL_RISE: f32 = 0.0;
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
    /// What this verb IS, as a colour. ⚠️ **ONE LANGUAGE FOR THE WHOLE FIGHT:** the wedge you
    /// are about to press and the body you are about to press it on wear the same colour, so
    /// "red" means a blow wherever it appears rather than meaning one thing on the menu and
    /// another on the arena. Red strikes, blue is a skill, green mends, steel guards, amber
    /// leaves.
    pub(crate) hue: Color,
    /// The mdi glyph, set ABOVE the word — the reference art stacks them, and a wedge is
    /// taller than it is wide at this radius, so the shape wants the icon on its own line.
    pub(crate) glyph: &'static str,
    /// Upper case, as the art has it: these are five short commands rather than prose, and
    /// caps is what makes them read as the labels ON a control rather than as text near one.
    pub(crate) word: &'static str,
    /// ⚠️ Read only by the test that holds it to being the word's own INITIAL — which is what
    /// lets the caption print `ATTACK` rather than `ATTACK [A]`. A verb whose key is not its
    /// initial makes that caption a lie, and the test is what says so.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) key: &'static str,
    /// The font's OWN name for `glyph`. ⚠️ **A codepoint with nothing checking it is how this
    /// repo drew a keyboard where a chest plate should have been** (`icons.rs`): this face's
    /// Material Design block is shifted from the upstream table, so a hand-copied codepoint
    /// lands on a neighbour and renders perfectly as the wrong picture. These five were exactly
    /// that — five hand-copied codepoints with no test on them — while every icon in `icons.rs`
    /// had been held to its name since the keyboard.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) icon: &'static str,
}

/// The five verbs around the wheel, **laid out on the battlefield rather than on a list**.
///
/// The wheel lies on the ground the fight is happening on, so its bearings mean something: the
/// enemies are north of every hero and the way out is south. **FLEE takes the south wedge**,
/// pointing at the retreat; **ATTACK and SKILL take the two wedges facing the enemy line**,
/// because both are things you do TO something over there; **ITEM and DEFEND take the sides**,
/// since both are things you do to yourself and neither has a direction. A player who has
/// understood the arena has already understood the menu.
pub(crate) const SLOTS: [Slot; 5] = [
    // mdi glyphs (see UiFont): auto-fix=Skill, flask=Item, run-fast=Flee, shield=Defend,
    // sword=Attack. Array order IS the order round the ring, clockwise from the GAP — and the
    // gap is on the FAR arc, so slot 0 is the sector just clockwise of the enemy-facing hole
    // and slot 2 lands dead south. Read it as a clock with the enemies at twelve: Skill and
    // Attack either side of twelve, Item and Defend at nine and three, Flee at six.
    Slot { index: 3, glyph: "\u{f0068}", icon: "md-auto_fix", word: "SKILL", key: "S", hue: INTENT_SKILL },
    Slot { index: 2, glyph: "\u{f0093}", icon: "md-flask", word: "ITEM", key: "I", hue: INTENT_MEND },
    // ⚠️ U+F070E is `md-run`, NOT `md-run_fast` — this drew a plain walker while the comment
    // above claimed the sprinter, and nothing checked it until the test below existed. The
    // verified codepoint is the one `icons.rs::nf::RUN_FAST` already carries.
    Slot { index: 4, glyph: "\u{f046e}", icon: "md-run_fast", word: "FLEE", key: "F", hue: INTENT_FLEE },
    // ⚠️ A HALVED shield, asked for by name — `fa-shield_halved`, not `fa-shield`.
    Slot { index: 1, glyph: "\u{ED25}", icon: "fa-shield_halved", word: "DEFEND", key: "D", hue: INTENT_GUARD },
    Slot { index: 0, glyph: "\u{f04e5}", icon: "md-sword", word: "ATTACK", key: "A", hue: INTENT_STRIKE },
];

/// **THE COLOUR OF AN INTENT.** Shared by the wheel's wedges and — once it lands — the glow on
/// whichever body a verb is aimed at, because a player who has learned that red means a blow
/// has learned it everywhere rather than twice.
pub(crate) const INTENT_STRIKE: Color = Color::srgb(1.0, 0.34, 0.28);
pub(crate) const INTENT_SKILL: Color = Color::srgb(0.42, 0.62, 1.0);
pub(crate) const INTENT_MEND: Color = Color::srgb(0.38, 0.95, 0.52);
pub(crate) const INTENT_GUARD: Color = Color::srgb(0.72, 0.82, 0.95);
pub(crate) const INTENT_FLEE: Color = Color::srgb(1.0, 0.72, 0.30);

/// Which way round the wheel a rising wedge number goes, in world space.
///
/// ⚠️ **THE SIGN IS SETTLED BY RENDERING IT, NOT BY READING THE TRANSFORM.** The disc is laid
/// flat by a -90° turn about X, and which way that sends the mesh's own +x is exactly the kind
/// of thing this file has already been wrong about once (the health pool drained across the
/// front for a whole build). Attack sits on the near arc and Defend must land to its RIGHT on
/// screen; flip this if it does not.
const SPIN: f32 = 1.0;

/// **THE RING HAS ONE MORE SECTOR THAN IT HAS VERBS, AND THE SPARE ONE IS THE FRONT.**
///
/// The near arc is left EMPTY. Three things wanted that exact patch of ground — the HP digits
/// written into the health ring below, the fighter's name, and whatever verb happened to sit
/// due south — and on screen they stacked into one unreadable pile; the reference art solves it
/// by simply not putting a button there. It also gives the wheel an axis, so the two halves
/// read as a pair of banks rather than as a wreath.
///
/// ⚠️ **AND THE HOLE FACES THE ENEMY, NOT THE CAMERA.** The first cut put it on the near arc —
/// the front of the wheel, nearest the player — which is the wrong half twice over: it is the
/// half you are looking THROUGH to read the fight, so a solid bank of buttons there is exactly
/// what you do not want, and the near arc is where the wheel is widest on screen and so has the
/// most room for buttons. Opening the FAR side means nothing stands between the hero and the
/// creatures it is being pointed at.
///
/// ⚠️ **AND IT DISSOLVES A CONTRADICTION RATHER THAN CREATING ONE.** With the gap at the front,
/// FLEE could not be due south and the two rules genuinely fought. With the gap at the BACK,
/// the sector opposite it IS due south — so Flee is due south again, Attack and Skill take the
/// two sectors flanking the hole (the ones facing the enemy line), and Item and Defend take
/// the flanks. Every bearing rule this menu has ever had holds at once, which is the tell that
/// this is the right way round.
pub(crate) const WEDGES: usize = SLOTS.len() + 1;

/// Where verb `n`'s sector sits, in turns clockwise from the arc nearest the camera.
///
/// ⚠️ **TWO ORIGINS MEET HERE AND THEY ARE HALF A TURN APART.** Sectors are numbered from the
/// GAP, which is on the far arc; `wheel_point` measures turns from the NEAR arc, because that
/// is the direction the rest of the file reasons in (the camera's). So verb `n` is sector
/// `n + 1` counted from the back, which is `0.5 + (n + 1)/WEDGES` counted from the front.
/// Forgetting the half turn put FLEE exactly in the hole — the one sector nothing is drawn in.
pub(crate) fn slot_turns(n: usize) -> f32 {
    (0.5 + (n + 1) as f32 / WEDGES as f32).fract()
}

/// The bearing handed to the shader so sector 0 — the GAP — is centred on the FAR arc, the one
/// pointing at the enemy line. A gap centred on an arc is a space the eye reads as deliberate;
/// a gap starting there is a seam.
pub(crate) fn wheel_bearing() -> f32 {
    0.5 - 0.5 / WEDGES as f32
}

/// Every wedge's own colour, packed for the shader in wedge order.
pub(crate) fn wedge_hues() -> [Vec4; WEDGES] {
    // Sector 0 is the GAP and is discarded in the shader, so its entry is never read — it is
    // present only so a hue can be indexed by sector number rather than by verb number, which
    // is the kind of off-by-one that shows up as the whole wheel wearing the wrong colours.
    let mut out = [Vec4::ONE; WEDGES];
    for (i, slot) in SLOTS.iter().enumerate() {
        let c = slot.hue.to_linear();
        out[i + 1] = Vec4::new(c.red, c.green, c.blue, 1.0);
    }
    out
}

/// Where wedge `n`'s label stands in the world, given the body's feet and the direction from
/// the body toward the camera (flattened onto the ground the wheel lies on).
/// ⚠️ **TEST-ONLY.** The renderer centres a tile with `sector_centre`, in SCREEN space, because
/// the projection of a ground-plane centroid is not the centroid of the projection. This stays
/// because the BEARING rules — Flee due south, Attack and Skill facing the enemy line, the gap
/// on the far arc — are about where a wedge points, which is a fact about the ground and not
/// about the camera looking at it.
#[cfg(test)]
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
    0.5 / WEDGES as f32
}

/// Where the wheel's own middle sits: behind the body it belongs to, by [`WHEEL_BACK`].
pub(crate) fn wheel_centre(feet: Vec3, front: Vec3, scale: f32) -> Vec3 {
    feet - front * (label_radius(scale) * WHEEL_BACK)
}

/// What a chip is drawn on. Solid enough to read over grass and a sprite at once, which the
/// panel's own glass is not — that sits over a scrim and this sits over the fight.
pub(crate) const CHIP_BASE: Color = Color::srgba(0.04, 0.05, 0.09, 0.84);

/// Report which wedge the pointer is over to the WHEEL, and leave the labels plain.
///
/// ⚠️ **THE HIGHLIGHT IS THE WEDGE, NOT A BOX OVER IT.** Lighting a rectangle on top of a
/// sector is what made the tiles read as plates parked on the ring rather than as its faces,
/// so both the cursor and the hover are drawn by `command_wheel.wgsl`. This only tells it
/// which wedge the pointer found.
pub(crate) fn style_radial(
    chips: Query<(&WheelLabel, &Interaction), With<RadialChip>>,
    mut mats: ResMut<Assets<CommandWheel>>,
    disc: Query<&MeshMaterial3d<CommandWheel>, With<CommandWheelDisc>>,
) {
    let over = chips
        .iter()
        .find(|(_, i)| matches!(**i, Interaction::Hovered | Interaction::Pressed))
        // +1 for the same reason the cursor is shifted: the shader counts SECTORS, and
        // sector 0 is the empty front.
        .map(|(l, _)| (l.wedge + 1) as f32)
        .unwrap_or(-1.0);
    let Ok(mat) = disc.single() else { return };
    if let Some(mut m) = mats.get_mut(&mat.0) {
        if m.state.x != over {
            m.state.x = over;
        }
    }
}


/// Move the cursor one wedge around the wheel. `dir` is +1 clockwise, -1 anticlockwise, and it
/// WRAPS — a wheel has no ends, and a cursor that stopped at one would be inventing a seam the
/// player cannot see.
pub(crate) fn step_cursor(cursor: usize, dir: i32) -> usize {
    let n = SLOTS.len() as i32;
    let at = SLOTS.iter().position(|s| s.index == cursor).unwrap_or(2) as i32;
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
                // ⚠️ **THE ICON CARRIES THE COLOUR; THE WORD STAYS LEGIBLE.** Colouring the
                // word too put red text on a red wedge the moment that wedge lit — the same
                // gold-on-gold mistake as the old selected chip, where the one label the player
                // is about to press is the hardest to read. The wedge is the colour; the glyph
                // repeats it at a size where contrast does not matter; the word is white on
                // both a dark wedge and a lit one.
                let text = Color::srgb(0.96, 0.97, 1.0);
                let step = match slot.index {
                    0 => Some(BattleIntroStep::Attack),
                    1 => Some(BattleIntroStep::Defend),
                    3 => Some(BattleIntroStep::Skill),
                    4 => Some(BattleIntroStep::Flee),
                    _ => None,
                };
                let edge = glass::EDGE_SOFT;
                // The guided dive's paced explainer brightens whichever verb it is describing.
                // It lit the chip's BORDER before; with the wedge carrying the face, the only
                // thing left that belongs to one verb is its own word.
                // The guided dive's explainer brightens whichever verb it is describing — and
                // the icon is the only thing on a wedge that belongs to one verb now, so the
                // highlight lands there rather than on a word that no longer exists.
                let icon_col = step
                    .filter(|s| tutorial_run.battle_intro == Some(*s))
                    .map(|_| glass::ACTIVE_EDGE)
                    .unwrap_or(slot.hue);
                let _ = (edge, text);
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
                        ..default()
                    },
                    // ⚠️ **NO PLATE AND NO BORDER.** The wedge beneath IS this button's face.
                    // A rectangle drawn on top of a sector is what made the tiles read as five
                    // plates parked near a hero rather than as the ring's own faces — which is
                    // the whole thing the wheel exists to be. What is left here is an icon, a
                    // word, and a hit box.
                    BackgroundColor(Color::NONE),
                    UiTransform::IDENTITY,
                ))
                .with_children(|chip| {
                    // ⚠️ **THE ICON IS THE WHOLE BUTTON: no word under it and no key letter.**
                    // A sector seen in perspective is a different shape at every bearing and
                    // every camera distance, and a six-character word is the thing that will
                    // not fit the narrow ones — DEFEND ran over the hero standing beside the
                    // wheel, and each attempt to make it fit was a smaller word in a shape
                    // that was still wrong. One glyph fits any sector by construction. It is
                    // also the only way it lands CENTRED: a stack of icon-over-letter centres
                    // the STACK, which puts the icon itself above the middle of its wedge.
                    // The verb's name and its key are said in the caption, once, where there
                    // is room for them.
                    chip.spawn((
                        Text::new(slot.glyph),
                        TextFont { font_size: FontSize::Px(GLYPH_PX), ..default() },
                        // **THE ICON WEARS ITS VERB'S COLOUR** — the same `INTENT_*` the wedge
                        // lights in and the targeting gem burns in, so the three say one thing.
                        // It went white for one build while the words were being removed and
                        // was asked for back immediately, which is the answer to whether the
                        // colour was carrying anything: it was.
                        TextColor(icon_col),
                        // The face beneath is dark, but it is dark GROUND in perspective with
                        // grass, a sprite and a health ring showing through the gaps.
                        TextShadow {
                            offset: Vec2::splat(2.0),
                            color: Color::srgba(0.0, 0.0, 0.0, 0.85),
                        },
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
                // ⚠️ **THE CAPTION NAMES THE VERB, BECAUSE THE WEDGES NO LONGER DO.** With the
                // words off the wheel the icons carry the choice and this carries the word —
                // once, in full, in its own intent colour, where there is room for it. Said on
                // every wedge it was five cramped words in five shapes none of them fit; said
                // here it is one, and it is already rebuilt whenever the cursor moves.
                // ⚠️ **AND IT DOES NOT PRINT THE KEY, because the key is the word.** Every
                // verb's letter is its own initial — A for ATTACK, F for FLEE, all five — so
                // `ATTACK [A]` spends a bracket saying what the first character already said.
                // `every_verbs_key_is_its_own_initial` is what keeps that true: the day a verb
                // is added whose key is not its initial, this has to start printing it again.
                if let Some(slot) = SLOTS.iter().find(|s| s.index == menu.cursor) {
                    cap.spawn((
                        Text::new(slot.word),
                        TextFont { font_size: FontSize::Px(17.0), ..default() },
                        TextColor(slot.hue),
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
    /// `(hovered wedge, openness, _, _)` — see `command_wheel.wgsl`.
    #[uniform(100)]
    pub(crate) state: Vec4,
    /// One colour per SECTOR, sector 0 being the empty front. `wedge_hues` says why it is
    /// indexed by sector rather than by verb.
    #[uniform(100)]
    pub(crate) hues: [Vec4; WEDGES],
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
                    params: Vec4::new(WEDGES as f32, -1.0, 0.0, wheel_bearing()),
                    tint: Vec4::new(0.40, 0.82, 1.0, 1.0),
                    state: Vec4::new(-1.0, 0.0, 0.0, 0.0),
                    hues: wedge_hues(),
                });
                commands.spawn((
                    CommandWheelDisc { open: 0.0 },
                    Mesh3d(wa.shadow_mesh.clone()),
                    MeshMaterial3d(mat),
                    // ⚠️ **AND IT MUST NOT CAST A SHADOW.** The mesh is a full DISC and the
                    // wedges are cut out of it in the fragment stage — but a shadow is
                    // rasterised from the geometry, not from what the shader decided to keep,
                    // so the sun threw a solid circle onto the ground underneath. Everything
                    // this material carefully discards — the hub the hero stands in, the seams
                    // between wedges, and now the gap facing the enemy — came back as a dark
                    // ellipse in exactly those places. Reported as the missing spot "not being
                    // transparent", which is precisely what it was: transparent, over its own
                    // shadow.
                    NotShadowCaster,
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
    // ⚠️ **THE DISC HAS TO TURN TO FACE THE CAMERA, AND FOR A LONG TIME IT DID NOT.**
    // `command_wheel.wgsl` measures its sectors with `atan2` in the MESH's own space, so the
    // bearing it is handed is relative to a fixed world axis — while every label here is placed
    // relative to `front`, the direction from THIS body toward the camera. Those are the same
    // line only for a hero standing on the arena's centre line. For anybody else the two frames
    // are rotated apart by that hero's own bearing, so the wedges were drawn in one place and
    // their icons stood in another — worst for the outermost hero, which is exactly where it
    // was reported ("those icons are WAY off from center"), and invisible on the middle of the
    // line, which is where a four-hero mock puts the eye first.
    //
    // Yawing the disc is the fix rather than folding the angle into the bearing uniform,
    // because it makes the shader's frame BE the frame the rest of this file reasons in
    // instead of leaving two conventions that have to be kept in step by hand.
    // ⚠️ **AND THE SIGN IS SETTLED BY RENDERING IT, exactly as `SPIN`'s note says.** The
    // shader's bearing zero lies along the mesh's local −Y, so the disc is yawed to put local
    // +Y on the direction AWAY from the camera. Reasoning it out from the flatten gives the
    // opposite answer, and the opposite answer also *looks* like a fix, because every wedge
    // moves — it just moves half a turn, and the cursor's red wedge ends up opposite the icon
    // it is lighting. That is the check: the lit wedge must appear under the icon it names.
    let yaw = Quat::from_rotation_y(front.x.atan2(front.z))
        * Quat::from_rotation_x(-std::f32::consts::FRAC_PI_2);
    if tf.rotation != yaw {
        tf.rotation = yaw;
    }
    // +1: the shader counts sectors and sector 0 is the gap.
    let cursor = SLOTS.iter().position(|s| s.index == menu.cursor).map(|n| (n + 1) as f32);
    if let Some(mut m) = mats.get_mut(&mat.0) {
        m.params = Vec4::new(
            WEDGES as f32,
            cursor.unwrap_or(-1.0),
            time.elapsed_secs(),
            wheel_bearing(),
        );
        m.state.y = state.open;
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
    mut labels: Query<
        (&WheelLabel, &mut Node, &mut UiTransform, &mut Visibility),
        Without<WheelCaption>,
    >,
    mut caption: Query<&mut Node, With<WheelCaption>>,
) {
    let Some(active) = battle.active.as_deref() else { return };
    let Some((cam, cam_tf)) = cam_q.iter().next() else { return };
    let Some(feet) = rings.iter().find(|(r, _)| r.id == active).map(|(_, t)| t.translation())
    else {
        for (_, _, _, mut vis) in &mut labels {
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

    for (label, mut node, mut tf, mut vis) in &mut labels {
        let turns = slot_turns(label.wedge);
        // The wedge's OWN corners: its two arc ends at the label radius, and its inner and
        // outer wall at its centre bearing. A tile cut to those sits in the ring rather than
        // on it — which is the whole difference between the reference art's wheel and five
        // plates parked near a hero.
        let edge = half_wedge() * TILE_INSET;
        // ⚠️ **THE CENTRE IS THE AVERAGE OF THE SECTOR'S OWN FOUR CORNERS, NOT A RADIUS.**
        // A wheel drawn flat on the ground is a sector in perspective, and no single radius is
        // its middle: the half nearer the camera's line of sight through the hub projects wider
        // than the half beyond it, so the true radial midpoint lands in the inner third of the
        // shape. Guessing a weighting got closer and was still a guess. Projecting the corners
        // and averaging them is the centre of the quad the player is actually looking at, at
        // any camera, for any wedge count.
        let (Some(p), Some(l), Some(r), Some(i), Some(o)) = (
            // Where the sector's own outline says its middle is, ON SCREEN.
            sector_centre(feet, front, turns, scale, project),
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
        // ⚠️ **THE HIT BOX AND THE WORD ARE SIZED BY DIFFERENT RULES, ON PURPOSE.** The box is
        // clamped UP to a minimum because a target too small to click is the one failure this
        // menu may not have; the word is fitted DOWN to the wedge's true width because a word
        // wider than its own sector runs onto the next one — and clamping both the same way is
        // what put DEFEND across the hero standing beside it.
        let span = l.distance(r) * TILE_INSET;
        let w = span.clamp(TILE_MIN_W, TILE_MAX_W);
        let h = (i.distance(o) * TILE_INSET).clamp(TILE_MIN_H, TILE_MAX_H);
        // Kept inside the window: a wedge at the edge of a wide formation would otherwise hang
        // its label off the side, and an order you cannot click is the one failure this menu
        // is not allowed to have.
        let x = (p.x - w * 0.5).clamp(2.0, (sw - w - 2.0).max(2.0));
        let y = (p.y - h * 0.5 - LABEL_RISE).clamp(2.0, (sh - h - 2.0).max(2.0));
        if std::env::var("MELD_WHEEL_TRACE").is_ok() {
            let edge = half_wedge() * TILE_INSET;
            let (r_in2, r_out2) = band_radii(scale);
            let corner = |t: f32, rr: f32| project(wheel_point(feet, front, t, rr, scale));
            bevy::log::info!(
                "wedge {} {:>7} turns={:.4} centre=({:.0},{:.0})                  near_l={:?} near_r={:?} far_l={:?} far_r={:?}",
                label.wedge,
                SLOTS[label.wedge].word,
                turns,
                p.x,
                p.y,
                corner(turns - edge, r_in2).map(|v| (v.x as i32, v.y as i32)),
                corner(turns + edge, r_in2).map(|v| (v.x as i32, v.y as i32)),
                corner(turns - edge, r_out2).map(|v| (v.x as i32, v.y as i32)),
                corner(turns + edge, r_out2).map(|v| (v.x as i32, v.y as i32)),
            );
        }
        if node.left != Val::Px(x) || node.top != Val::Px(y) {
            node.left = Val::Px(x);
            node.top = Val::Px(y);
        }
        // **EVERY WORD READS LEFT TO RIGHT.** The labels used to take a share of the arc's
        // tangent so they would "belong to the curve", damped and capped because following it
        // fully stands the side wedges' words on their ends. It was wrong at any strength, and
        // the reference art says so plainly: on a wheel you do not read round, you read the
        // wedge you are pointing at. A tilted word is also the thing that leaves a sector
        // through its corners, so the whole class of overhang goes with it.
        if tf.rotation != Rot2::IDENTITY {
            tf.rotation = Rot2::IDENTITY;
        }
        if node.width != Val::Px(w) || node.height != Val::Px(h) {
            node.width = Val::Px(w);
            node.height = Val::Px(h);
        }
    }

    // The caption rides above the hero it is speaking for, clamped into the window so it can
    // never be pushed off the edge by a body standing at the end of a wide formation.
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

    /// **THE DISC'S OWN FRAME IS THE ONE THE LABELS ARE PLACED IN.**
    ///
    /// ⚠️ `command_wheel.wgsl` measures its sectors with `atan2` in the MESH's space, so the
    /// wedge at bearing zero is wherever the mesh points. Every label is placed relative to
    /// `front` — the direction from THIS body toward the camera — and the disc carried **no yaw
    /// at all**, so the two frames were the same one only for a hero standing on the arena's
    /// centre line. For anybody else the wedges were drawn rotated away from their own icons:
    /// worst at the ends of the line, and invisible in the middle, which is where a four-hero
    /// mock puts the eye first and why it survived so long.
    ///
    /// ⚠️ **The SIGN is settled by rendering, like `SPIN` above it.** Deriving it from the
    /// flatten gives the opposite answer, which also looks like a fix because every wedge
    /// moves — half a turn, leaving the cursor's lit wedge opposite the icon it is lighting.
    /// What this test holds is the part that is pure arithmetic: the wheel turns with the body
    /// and stays flat on the ground.
    #[test]
    fn the_disc_turns_to_face_the_camera() {
        let yaw_for = |front: Vec3| {
            Quat::from_rotation_y(front.x.atan2(front.z))
                * Quat::from_rotation_x(-std::f32::consts::FRAC_PI_2)
        };
        for front in [
            Vec3::Z,
            Vec3::new(0.6, 0.0, 0.8).normalize(),
            Vec3::new(-0.9, 0.0, 0.44).normalize(),
            Vec3::X,
            Vec3::new(-0.2, 0.0, -0.98).normalize(),
        ] {
            let yaw = yaw_for(front);
            // FLAT. The disc's own normal comes out straight up whatever the body's bearing —
            // a wheel standing on its edge is the failure a yaw applied in the wrong order gives.
            let up = yaw * Vec3::Z;
            assert!(up.y > 0.999, "the wheel is not lying on the ground for {front:?}: {up:?}");
            // AND IT TURNS WITH THE BODY: the mesh's own axis stays on the body's own bearing
            // line, so a hero at the end of the line gets its wedges rotated exactly as much as
            // its labels are. (Which END of that line is the rendered question above.)
            let axis = yaw * Vec3::Y;
            assert!(
                axis.y.abs() < 1e-4,
                "the wheel's bearing axis left the ground plane: {axis:?}",
            );
            assert!(
                axis.cross(front).length() < 1e-3,
                "the wheel's bearing axis {axis:?} is off the body's own line {front:?}",
            );
        }
    }

    /// **EVERY VERB'S KEY IS ITS OWN INITIAL**, which is why the caption prints `ATTACK` and
    /// not `ATTACK [A]` — the bracket would spend itself saying what the first letter already
    /// said. It is a real constraint rather than a coincidence: the day a verb arrives whose
    /// key is not its initial, the caption has to start printing keys again, and this is what
    /// will say so.
    #[test]
    fn every_verbs_key_is_its_own_initial() {
        for slot in SLOTS {
            let first = slot.word.chars().next().expect("a verb has a name");
            let key = slot.key.chars().next().expect("a verb has a key");
            assert_eq!(
                first.to_ascii_uppercase(),
                key.to_ascii_uppercase(),
                "{}'s key is {} — the caption stops being able to leave it out",
                slot.word,
                slot.key,
            );
        }
        // And no two verbs share one, or the letter stops identifying anything.
        for (i, a) in SLOTS.iter().enumerate() {
            for b in SLOTS.iter().skip(i + 1) {
                assert_ne!(a.key, b.key, "{} and {} share a key", a.word, b.word);
            }
        }
    }

    /// **EVERY WEDGE'S ICON IS THE GLYPH IT CLAIMS TO BE.**
    ///
    /// ⚠️ The wheel's five codepoints were hand-copied and nothing checked them, which is the
    /// exact gap `icons.rs` closed for every other icon in the game after `md-tshirt_crew` drew
    /// a KEYBOARD — present in the font, rendered happily, and the wrong picture. This face's
    /// Material Design block is shifted from the upstream table, so "is it in the font" is not
    /// the question; the face knows its own glyph names, so ask it.
    #[test]
    fn every_wedge_icon_is_the_glyph_it_claims_to_be() {
        let face = ttf_parser::Face::parse(crate::netglue::UI_FONT_BYTES, 0)
            .expect("the bundled UI font parses");
        for slot in SLOTS {
            let ch = slot.glyph.chars().next().expect("a glyph is at least one char");
            let gid = face.glyph_index(ch).unwrap_or_else(|| {
                panic!("{} (U+{:X}) is not in the font at all", slot.icon, ch as u32)
            });
            assert_eq!(
                face.glyph_name(gid),
                Some(slot.icon),
                "{}'s icon U+{:X} is {:?}, not {} — the codepoint is off",
                slot.word,
                ch as u32,
                face.glyph_name(gid),
                slot.icon,
            );
        }
    }

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

    /// **THE GAP FACES THE ENEMY LINE, AND NO VERB STANDS THERE.** The ring carries one more
    /// sector than it has verbs and the spare one is centred on the FAR arc, so nothing stands
    /// between the hero and the creatures it is being pointed at.
    ///
    /// ⚠️ **THE FIRST CUT PUT IT ON THE NEAR ARC AND THAT WAS BACKWARDS.** The near arc is the
    /// half you look THROUGH to read the fight, and it is where the wheel is widest on screen —
    /// so it is simultaneously the worst place to spend on nothing and the best place to put
    /// buttons. Half a sector out in either direction and the gap stops being a space and
    /// becomes a seam between two buttons, which reads as a crack.
    #[test]
    fn the_gap_faces_the_enemy_line() {
        let half = 0.5 / WEDGES as f32;
        assert!(
            (wheel_bearing() - (0.5 - half)).abs() < 1e-6,
            "sector 0 is not centred on the far arc",
        );
        // No verb may stand on the far arc: that half-turn bearing is the gap's alone.
        for (n, slot) in SLOTS.iter().enumerate() {
            let from_back = (slot_turns(n) - 0.5).abs();
            assert!(from_back > half - 1e-6, "{} stands in the gap", slot.word);
        }
        // And the gap is exactly one sector wide — not a wider hole that eats a neighbour, nor
        // a hairline that reads as a join.
        assert!((half_wedge() * 2.0 - 1.0 / WEDGES as f32).abs() < 1e-6);
    }

    /// **THE MENU IS LAID OUT ON THE BATTLEFIELD.** The enemies are north of every hero, so
    /// Attack and Skill face the enemy line and the two verbs with no direction — Item and
    /// Defend — take the sides. Asserted by BEARING rather than by array index, because the
    /// rule is about where a wedge points.
    ///
    /// ⚠️ **AND THE HOLE IS WHAT LETS ALL THREE RULES HOLD AT ONCE.** With the gap on the near
    /// arc, Flee could not also be due south and the rules genuinely fought. With it on the FAR
    /// arc the sector opposite is due south, so Flee is back where it belongs and the two
    /// sectors flanking the hole are the two facing the enemies. A layout in which every stated
    /// rule is satisfiable is the tell that the hole is on the right side.
    #[test]
    fn the_wheel_is_laid_out_on_the_arena() {
        let feet = Vec3::ZERO;
        // The camera is along +z, so +z is south (toward the player) and -z is the enemy line.
        let front = Vec3::Z;
        let at = |verb: usize| {
            let n = SLOTS.iter().position(|s| s.index == verb).expect("verb has no wedge");
            slot_world(feet, front, n, WHEEL_SCALE)
        };
        let (attack, skill, item, defend, flee) = (at(0), at(3), at(2), at(1), at(4));
        // Flee is the southernmost thing on the wheel — the way out is behind you.
        for (name, p) in [("attack", attack), ("skill", skill), ("item", item), ("defend", defend)]
        {
            assert!(flee.z > p.z, "flee is not south of {name}");
        }
        // Attack and Skill are the two facing the enemies, and they are the two FLANKING THE
        // GAP — so the pair of buttons either side of the hole are the pair that reach across
        // it, which is the whole reason the hole is on that side.
        for (name, p) in [("item", item), ("defend", defend), ("flee", flee)] {
            assert!(attack.z < p.z, "attack does not face the enemies against {name}");
            assert!(skill.z < p.z, "skill does not face the enemies against {name}");
        }
        // …and they are on opposite sides of the centre line, so neither is straight ahead —
        // straight ahead is the gap.
        assert!(attack.x * skill.x < 0.0, "attack and skill are on the same flank");
        // Item and Defend take the flanks, one each.
        assert!(item.x * defend.x < 0.0, "item and defend are on the same side");
        assert!(defend.x > 0.0, "defend is not on the right");
    }

    /// The labels stand on the wheel, evenly spaced, one per wedge.
    #[test]
    fn every_wedge_gets_its_own_place_on_the_wheel() {
        let feet = Vec3::ZERO;
        let front = Vec3::Z;
        let places: Vec<Vec3> =
            (0..SLOTS.len()).map(|n| slot_world(feet, front, n, WHEEL_SCALE)).collect();
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
        // ⚠️ The wheel is CENTRED on the body now, so its hub is the feet: a set-back wheel
        // leaves the hero standing at the front edge of a ring drawn behind it.
        assert!(hub.distance(feet) < 1e-5, "the wheel is not centred on the hero");
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

    /// **THE WHEEL DOES NOT COVER THE BAR IT GREW OUT OF.** Its inner wall has to clear the
    /// health ring's outer one, or the wheel is drawn straight over the readout it is supposed
    /// to have pushed outward — which at the first scale left the ring as a green thread under
    /// a dark plate.
    #[test]
    fn the_wheel_clears_the_health_ring() {
        let inner = W_IN * MESH_RADIUS * WHEEL_SCALE;
        let health_outer = 0.7 * 1.35 * 0.96;
        assert!(inner > health_outer, "the wheel covers the bar: {inner} against {health_outer}");
    }

    /// **THE CURSOR WRAPS.** A wheel has no ends, so a cursor that stopped at one would be
    /// inventing a seam the player cannot see — and it must reach every wedge either way
    /// round, or a verb is keyboard-unreachable.
    #[test]
    fn the_cursor_goes_round_the_wheel() {
        // Flee · Defend · Attack · Skill · Item, clockwise from the near arc.
        assert_eq!(step_cursor(0, 1), 3, "clockwise of Attack is Skill");
        assert_eq!(step_cursor(0, -1), 1, "anticlockwise of Attack is Defend");
        assert_eq!(step_cursor(4, -1), 2, "anticlockwise of Flee wraps to Item");
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
