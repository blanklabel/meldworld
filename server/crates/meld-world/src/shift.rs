//! The Shift — the Shifting Lands' churn engine (CANON D20, §W2).
//!
//! Regions of the overworld periodically **Shift**: after a brief warning tell they
//! swap to a different bestiary biome, deal **Force** damage to whatever is standing
//! in them, and wipe that region's creatures and collectables. What grows back
//! belongs to the new biome. This is what makes a *persistent* world (§W1/§W5)
//! survivable content rather than a strip mine — a place nobody ever refreshed would
//! be picked clean within an hour of it becoming permanent.
//!
//! **The schedule is a pure function of `(world_seed, generation)`** and is driven by
//! the server's tick counter, never `Instant::now` (CANON §W2, structural). Two
//! integers therefore replay a world's entire Shift history, which is exactly what
//! makes §W5 persistence cheap: the baseline comes from the seed, the schedule comes
//! from the seed, and only what *players* did has to be written down.
//!
//! Everything here is a *roll* — an intent, in world-independent units. Resolving a
//! roll against the sections that actually exist is [`crate::Arena::apply_shift`]'s
//! job, because how many sections there are is a property of the live world and not
//! of the seed.

use crate::Rng;
use meld_balance::Balance;

/// One scheduled Shift, drawn from `(seed, generation)` alone.
///
/// `locate` and `sections` are deliberately world-independent: a roll made before the
/// player had walked past section 3 and the same roll made after they reached section
/// 40 are the same roll, and both are still valid — the arena resolves `locate` against
/// whatever depth exists when the Shift lands.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ShiftRoll {
    pub generation: u64,
    /// Tick at which the tell goes up.
    pub warn_tick: u64,
    /// Tick at which the land actually swaps.
    pub land_tick: u64,
    /// Where, as a fraction of the world's generated depth (0 = the hub ring,
    /// 1 = the frontier). CANON's `1d100 × 1d100` location roll, in one axis: the
    /// overworld is a radial fan, so a section index IS the location.
    pub locate: f64,
    /// How DEEP the region reaches, in contiguous sections — the game's translation of
    /// CANON's 1d6 Tiny…Cataclysmic size table. It sizes the BEARING wedge too
    /// ([`crate::Arena::shift_patch`]), so a Tiny Shift is about one cell on a side and a
    /// Cataclysmic one about three: the size table means the same thing in both axes.
    ///
    /// ⚠️ **This used to be the whole region, and the comment here said a section-granular
    /// Shift "retiles the world for free" because the client keyed its ground off
    /// per-section radius rings.** That was true when it was written and false from `WG-7`
    /// on, where a biome became a property of a CELL derived analytically — so the Shift
    /// swapped `Area.biome`, the banner announced it, and the ground never repainted. A
    /// region is a patch of cells now, and what it repaints rides the wire.
    pub sections: usize,
    /// Draw for the incoming biome. Resolved against the candidate list by the
    /// arena, so a region can never "shift" into the biome it already is.
    pub biome_pick: u64,
    /// Force damage every avatar caught inside takes when it lands, as a fraction of
    /// that hero's OWN max HP. Scales with the region's size, so the Cataclysmic swap
    /// is the one that kills you. A fraction rather than points because a hero runs
    /// 40 max HP at level 1 and ~535 at 100 — the every-magnitude-is-a-fraction rule.
    pub damage_fraction: f64,
    /// Whether this one picks its region uniformly at random rather than by
    /// least-recently-disturbed. A purely LRU Shift is a Shift that always lands
    /// where you are not, which is a weather report rather than a hazard.
    pub uniform_pick: bool,
    /// **Where around the arc**, as a fraction of the fan's width at the region's own
    /// radius (0 = one rim, 1 = the other). The second axis a region needs now that it is
    /// a patch of cells rather than a whole ring — without it every Shift takes a complete
    /// annulus, which is the concentric-ring world `WG-7` and `WG-11` exist to retire.
    pub bearing: f64,
}

/// Stream position for generation `g`: distinct per generation and per purpose, so
/// two rolls never share state and the schedule can be evaluated out of order.
fn stream(seed: u64, generation: u64) -> Rng {
    Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15).rotate_left(17)
        ^ generation.wrapping_mul(0xD1B5_4A32_D192_ED03)
        ^ 0x5348_4946_5400_0001)
}

/// Ticks between generation `g - 1` landing and generation `g` landing. Pure in
/// `(seed, g)`.
pub fn cadence(b: &Balance, seed: u64, generation: u64) -> u64 {
    let s = &b.shift;
    let mut rng = stream(seed, generation);
    let jitter = 1.0 + rng.signed() * s.cadence_jitter.clamp(0.0, 0.95);
    ((s.cadence_ticks.max(1) as f64) * jitter).round().max(1.0) as u64
}

/// The tick at which generation `g` lands — the fold of every cadence draw up to and
/// including `g`, so it is pure in `(seed, g)` with no running state to persist.
///
/// `O(g)`, and `g` advances about once every few minutes, so even a world left up for
/// a year folds ~100k `u64` adds — computed once when a generation is retired, never
/// per tick.
pub fn land_tick(b: &Balance, seed: u64, generation: u64) -> u64 {
    (0..=generation).map(|g| cadence(b, seed, g)).sum()
}

/// Draw generation `g`'s Shift. Pure in `(seed, g)` (CANON §W2).
pub fn roll(b: &Balance, seed: u64, generation: u64) -> ShiftRoll {
    let s = &b.shift;
    let land = land_tick(b, seed, generation);
    // A separate stream from `cadence`'s, or the size draw would be correlated with
    // the jitter that scheduled it (small Shifts always early, and so on).
    let mut rng = stream(seed ^ 0xA5A5_5A5A_C3C3_3C3C, generation);
    let lo = s.min_sections.max(1);
    let hi = s.max_sections.max(lo);
    let span = (hi - lo + 1) as u64;
    let sections = lo + (rng.next_u64() % span) as usize;
    // Damage rides the SIZE, not another draw: "how much of the world went" and "how
    // hard it hit" being the same number is what lets the tell's radius read as a
    // threat level rather than as decoration.
    let t = if hi > lo { (sections - lo) as f64 / (hi - lo) as f64 } else { 0.0 };
    let damage_fraction =
        s.damage_fraction_min + (s.damage_fraction_max - s.damage_fraction_min) * t;
    ShiftRoll {
        generation,
        warn_tick: land.saturating_sub(s.warning_ticks),
        land_tick: land,
        locate: rng.unit(),
        sections,
        biome_pick: rng.next_u64(),
        damage_fraction,
        uniform_pick: rng.unit() < s.random_pick_share,
        // ⚠️ DRAWN LAST, deliberately. The schedule is a pure function of `(seed, g)` and
        // the draws are positional, so a new draw inserted anywhere above would move every
        // value after it and every world's history with it.
        bearing: rng.unit(),
    }
}

/// What a landed Shift did, for the wire and for the log.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ShiftOutcome {
    /// The sections that retiled, low to high.
    pub sections: Vec<usize>,
    /// The biome they are now.
    pub biome: String,
    /// Inner and outer radius of the swapped region, so the client can draw the tell and
    /// the flash without knowing what a section is. With `arc_center`/`arc_half` this is
    /// the region's bounding patch — a region is a set of CELLS now, and these four numbers
    /// are what the tell needs rather than the membership itself.
    pub inner_radius: f64,
    pub outer_radius: f64,
    /// The bearing wedge the region occupies, in radians: its centre and half-width. A
    /// region no longer spans the whole arc, so a tell drawn from the radii alone lights up
    /// a full ring around a patch that changed.
    pub arc_center: f64,
    pub arc_half: f64,
    /// **The cells this Shift repainted, and what they became.** The ground derives a
    /// cell's biome analytically ([`meld_proto::regions`]), so this delta is the only thing
    /// that can move it — without it the land swaps, the props re-scatter and the floor
    /// stays exactly what the seed said.
    pub repaints: Vec<meld_proto::regions::Repaint>,
    /// Entities the Shift removed, so a client holding them can drop them without
    /// waiting for a snapshot to omit them.
    pub wiped: Vec<String>,
    /// `player_id -> fraction of max HP` the Force blast took off everyone caught inside.
    pub caught: Vec<(String, f64)>,
    /// Anyone the new land was strewn on top of, walked back to the region's entry. The
    /// server corrects their client with these rather than letting it argue with the
    /// authoritative position for a second.
    pub moved: Vec<(String, crate::Position)>,
    /// Each retiled section's new mountains, so the retile message carries them. Peaks are
    /// what elevation IS in this world (discrete terraces are retired), so this is the
    /// half of "the land changed shape" that renders.
    pub peaks: Vec<(usize, Vec<[f32; 4]>)>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn balance() -> Balance {
        Balance::load_default().expect("embedded balance")
    }

    #[test]
    fn a_shift_schedule_is_a_pure_function_of_seed_and_generation() {
        let b = balance();
        for g in 0..40 {
            assert_eq!(roll(&b, 77, g), roll(&b, 77, g), "gen {g} is not reproducible");
        }
        assert_ne!(roll(&b, 77, 3), roll(&b, 78, 3), "two worlds shift in lockstep");
    }

    #[test]
    fn shifts_land_in_order_and_never_twice_on_the_same_tick() {
        let b = balance();
        let mut prev = 0;
        for g in 0..200 {
            let r = roll(&b, 12345, g);
            assert!(r.land_tick > prev, "gen {g} lands at {} after {prev}", r.land_tick);
            assert!(r.warn_tick < r.land_tick, "gen {g} has no tell");
            prev = r.land_tick;
        }
    }

    #[test]
    fn the_tell_is_long_enough_to_walk_out_of() {
        // A Shift you cannot escape is a dice roll, not a hazard: the warning has to
        // outlast the time it takes to cross the largest region it can roll.
        let b = balance();
        let warn_secs = (b.shift.warning_ticks * b.battle.tick_ms) as f64 / 1000.0;
        let reach = warn_secs * b.world.avatar_speed_tiles_per_sec;
        let widest = b.worldgen.base_area_length
            + b.worldgen.area_length_growth * (b.shift.max_sections as f64);
        assert!(
            reach > widest * 0.5,
            "the tell ({warn_secs}s = {reach} tiles) cannot cross half a {widest}-tile region"
        );
    }

    #[test]
    fn damage_scales_with_the_size_of_what_went() {
        let b = balance();
        let mut seen: Vec<(usize, f64)> = (0..400)
            .map(|g| {
                let r = roll(&b, 909, g);
                (r.sections, r.damage_fraction)
            })
            .collect();
        seen.sort_by_key(|s| s.0);
        for w in seen.windows(2) {
            assert!(w[0].1 <= w[1].1, "a bigger Shift hit softer: {:?} then {:?}", w[0], w[1]);
        }
        let sizes: std::collections::HashSet<usize> = seen.iter().map(|s| s.0).collect();
        assert_eq!(sizes.len(), b.shift.max_sections - b.shift.min_sections + 1);
    }

    #[test]
    fn force_damage_is_a_fraction_never_points() {
        // A flat blast is a death sentence at level 1 and a rounding error at 100.
        let b = balance();
        assert!(b.shift.damage_fraction_min > 0.0 && b.shift.damage_fraction_max < 1.0);
        assert!(b.shift.damage_fraction_min <= b.shift.damage_fraction_max);
    }

    #[test]
    fn both_pick_modes_actually_occur() {
        let b = balance();
        let uniform = (0..300).filter(|g| roll(&b, 4242, *g).uniform_pick).count();
        assert!(uniform > 30 && uniform < 270, "pick mode is effectively fixed ({uniform}/300)");
    }
}
