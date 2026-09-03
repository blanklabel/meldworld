//! Battle feel: the timings and magnitudes behind the ATB's juice, in one tunable place.
//!
//! These are *presentation* numbers — the authoritative pacing (`tick_ms`,
//! `gauge_fill_divisor`, `turn_timeout_ms`) stays in `balance.toml` server-side. But the
//! client is a separate workspace sharing only `meld-proto`, so it has no balance loader
//! to hang these off, and they had drifted into bare `const`s in `main.rs` plus magic
//! literals inside the animation systems — which made dialing the feel in a recompile per
//! guess. One struct, defaults that are exactly what shipped, and a runtime override:
//! `MELD_FEEL="lunge_ttl=0.5,number_rise=70"`.

use bevy::prelude::*;

#[derive(Resource, Clone, Debug, PartialEq)]
pub(crate) struct BattleFeel {
    pub hit_ttl: f32,
    pub flash_ttl: f32,
    pub recoil_ttl: f32,
    /// The white impact bloom. A *subset* of the recoil, not a separate beat.
    pub white_ttl: f32,
    pub lunge_ttl: f32,
    pub atb_flash_ttl: f32,
    pub recoil_distance: f32,
    pub lunge_distance: f32,
    pub shake_distance: f32,
    pub shake_hz: f32,
    pub number_rise: f32,
    pub number_size: f32,
    pub weak_scale: f32,
    pub number_shake: f32,
    /// World units above a combatant's feet that its numbers float from — a world-space
    /// offset, not a pixel one, so perspective keeps it over the head at any depth. Sits
    /// just clear of the target diamond (`h + 0.45` over the tallest enemy).
    pub number_height: f32,
    /// Pixels each additional simultaneous number on one target is lifted, so an
    /// all-enemy sweep reads as a stack instead of one illegible overstrike. Wants to
    /// exceed `number_size`, or consecutive lines touch.
    pub stack_step: f32,
}

impl Default for BattleFeel {
    fn default() -> Self {
        Self {
            hit_ttl: 1.0,
            flash_ttl: 0.18,
            recoil_ttl: 0.3,
            white_ttl: 0.12,
            lunge_ttl: 0.34,
            atb_flash_ttl: 0.55,
            recoil_distance: 0.35,
            lunge_distance: 0.6,
            shake_distance: 0.12,
            shake_hz: 90.0,
            number_rise: 46.0,
            number_size: 26.0,
            weak_scale: 1.45,
            number_shake: 4.0,
            number_height: 3.1,
            stack_step: 30.0,
        }
    }
}

impl BattleFeel {
    /// Apply a `key=value,key=value` override. A bad key or number warns and is skipped:
    /// this is a dial you turn with the game in front of you, so a typo should cost one
    /// setting rather than the session.
    pub(crate) fn apply(&mut self, spec: &str) {
        for pair in spec.split(',').map(str::trim).filter(|s| !s.is_empty()) {
            let Some((key, raw)) = pair.split_once('=') else {
                warn!("MELD_FEEL: `{pair}` is not key=value");
                continue;
            };
            let Ok(v) = raw.trim().parse::<f32>() else {
                warn!("MELD_FEEL: `{raw}` is not a number");
                continue;
            };
            let key = key.trim();
            match key {
                "hit_ttl" => self.hit_ttl = v,
                "flash_ttl" => self.flash_ttl = v,
                "recoil_ttl" => self.recoil_ttl = v,
                "white_ttl" => self.white_ttl = v,
                "lunge_ttl" => self.lunge_ttl = v,
                "atb_flash_ttl" => self.atb_flash_ttl = v,
                "recoil_distance" => self.recoil_distance = v,
                "lunge_distance" => self.lunge_distance = v,
                "shake_distance" => self.shake_distance = v,
                "shake_hz" => self.shake_hz = v,
                "number_rise" => self.number_rise = v,
                "number_size" => self.number_size = v,
                "weak_scale" => self.weak_scale = v,
                "number_shake" => self.number_shake = v,
                "number_height" => self.number_height = v,
                "stack_step" => self.stack_step = v,
                _ => warn!("MELD_FEEL: no such knob `{key}`"),
            }
        }
    }

    pub(crate) fn from_flags() -> Self {
        let mut feel = Self::default();
        if let Some(spec) = crate::feel_flag() {
            feel.apply(&spec);
        }
        feel
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `1.0 - age / ttl` is the alpha of every fading number, so a zero TTL is a NaN on
    /// screen rather than an instant fade.
    #[test]
    fn every_ttl_default_is_positive() {
        let f = BattleFeel::default();
        for (name, ttl) in [
            ("hit_ttl", f.hit_ttl),
            ("flash_ttl", f.flash_ttl),
            ("recoil_ttl", f.recoil_ttl),
            ("white_ttl", f.white_ttl),
            ("lunge_ttl", f.lunge_ttl),
            ("atb_flash_ttl", f.atb_flash_ttl),
        ] {
            assert!(ttl > 0.0, "{name} must be positive, got {ttl}");
        }
    }

    /// The sprite blooms on contact and is still travelling back when the bloom stops;
    /// inverting these reads as two separate hits.
    #[test]
    fn the_flash_is_shorter_than_the_recoil() {
        let f = BattleFeel::default();
        assert!(f.white_ttl < f.recoil_ttl);
    }

    /// The whole point of the stack is legibility, and a step under the font size puts
    /// the next line back through the glyphs of the last one.
    #[test]
    fn stacked_numbers_clear_their_own_font() {
        let f = BattleFeel::default();
        assert!(f.stack_step > f.number_size);
    }

    #[test]
    fn overrides_apply_and_leave_the_rest_alone() {
        let mut f = BattleFeel::default();
        f.apply("lunge_ttl=0.5, number_rise=70");
        assert_eq!(f.lunge_ttl, 0.5);
        assert_eq!(f.number_rise, 70.0);
        assert_eq!(f.hit_ttl, BattleFeel::default().hit_ttl);
    }

    #[test]
    fn a_bad_knob_is_skipped_not_fatal() {
        let mut f = BattleFeel::default();
        f.apply("nonsense=1,lunge_ttl=oops,lunge_distance=0.9");
        assert_eq!(f, BattleFeel { lunge_distance: 0.9, ..BattleFeel::default() });
    }
}

/// World feel: how fast the sky turns and how often it rains.
///
/// The same argument as [`BattleFeel`] and the same shape — these are *presentation*
/// pacing, they had drifted into a bare `const` plus four magic literals inside
/// `advance_sky`, and they are exactly the numbers you want to turn with the game in
/// front of you. `MELD_WORLD_FEEL="day_len=900,fair_secs=800"`.
#[derive(Resource, Clone, Debug, PartialEq)]
pub(crate) struct WorldFeel {
    /// Seconds for one full day → night → day cycle.
    pub day_len: f32,
    /// The time of day the world OPENS at, as a fraction of the cycle: `0.0`/`1.0`
    /// midnight, `0.25` sunrise, `0.5` noon, `0.75` sunset. Default is mid-morning.
    ///
    /// It exists so a NIGHT scene can be screenshotted deterministically — the same
    /// argument as `MELD_TALLY` holding an extraction haul on screen. Nightfall is
    /// otherwise minutes into a session and gone again by the time a capture lands,
    /// which is how a bug that only shows in the dark (the battle glow washing every
    /// creature to a white silhouette) reached a release unseen.
    /// `MELD_WORLD_FEEL="sky_t=0.0"` opens at midnight.
    pub sky_t: f32,
    /// The long dry spell between storms. This is the knob that decides how often it
    /// rains: the other three phases are the storm itself and are short by design.
    pub fair_secs: f32,
    /// Wind rises before the rain arrives, so the trees toss before the downpour.
    pub gust_secs: f32,
    /// How long rain actually falls.
    pub storm_secs: f32,
    /// Rain stops, wind dies down.
    pub clearing_secs: f32,
    /// **CAMERA EXPOSURE, AS EV100.** The lens, and the knob that was actually wrong — see
    /// [`meld_client::hd2d::DEFAULT_EV100`] for the diagnosis (the world was lit in real lux
    /// and metered at Bevy's Blender default, 5.3 stops apart) and for why it can move only
    /// so far before the emissive sky crushes to black.
    pub exposure: f32,
    /// **NOON SUN, IN LUX.** Bevy's `FULL_DAYLIGHT`. Physical, and paired with an exposure
    /// that expects a physical value — see `exposure` for why this is NOT the brightness
    /// knob, however much it looks like one.
    pub sun_lux: f32,
    /// **NOON AMBIENT.** Undirected fill, so it lifts the shadowed side of everything
    /// equally and is what flattens contrast rather than what sets overall brightness.
    /// Bevy's own default is 80; this had been left at 260 to help a too-dark scene, so it
    /// is part of the same fixed-bug compensation as the sun and comes back down with it.
    pub ambient: f32,
}

impl Default for WorldFeel {
    fn default() -> Self {
        Self {
            // A full cycle was 3.5 minutes, which made the sun a strobe — you could watch
            // dawn and dusk inside one fight. Ten minutes still shows a player both halves
            // in a normal dive without the sky ever being the thing they notice.
            day_len: 600.0,
            // Mid-morning: the sun is up and climbing, so a fresh player's first
            // frame is a lit world rather than a puzzle about the brightness.
            sky_t: 0.36,
            // Rain was ~8% of all weather and a storm arrived every ~4 minutes, so the
            // overworld read as permanently overcast. The dry spell is what governs the
            // rate, so it is the only one that moved.
            fair_secs: 600.0,
            gust_secs: 16.0,
            storm_secs: 22.0,
            clearing_secs: 14.0,
            // ONE source: the lib owns the value, because the camera is spawned there and
            // this default has to agree with it. A literal in both places is a lens that
            // changes on the second frame.
            exposure: meld_client::hd2d::DEFAULT_EV100,
            sun_lux: 20_000.0,
            ambient: 80.0,
        }
    }
}

impl WorldFeel {
    /// Seconds the phase after `phase` lasts (`0` Fair, `1` Gust, `2` Storm, `3` Clearing).
    pub(crate) fn phase_secs(&self, phase: u8) -> f32 {
        match phase {
            0 => self.fair_secs,
            1 => self.gust_secs,
            2 => self.storm_secs,
            _ => self.clearing_secs,
        }
    }

    pub(crate) fn apply(&mut self, spec: &str) {
        for pair in spec.split(',').map(str::trim).filter(|s| !s.is_empty()) {
            let Some((key, raw)) = pair.split_once('=') else {
                warn!("MELD_WORLD_FEEL: `{pair}` is not key=value");
                continue;
            };
            let Ok(v) = raw.trim().parse::<f32>() else {
                warn!("MELD_WORLD_FEEL: `{raw}` is not a number");
                continue;
            };
            match key.trim() {
                "day_len" => self.day_len = v,
                // Wrapped, not clamped: `sky.t` is a fraction of a cycle and
                // `advance_sky` keeps it that way, so `sky_t=1.25` is quarter past dawn
                // rather than an error. A negative reads back from midnight.
                "sky_t" => self.sky_t = v.rem_euclid(1.0),
                "fair_secs" => self.fair_secs = v,
                "gust_secs" => self.gust_secs = v,
                "storm_secs" => self.storm_secs = v,
                "clearing_secs" => self.clearing_secs = v,
                // No clamp: the whole point of these two is to render a LADDER of values
                // and pick by eye, and a clamp would silently flatten the top of it.
                "exposure" => self.exposure = v,
                "sun_lux" => self.sun_lux = v.max(0.0),
                "ambient" => self.ambient = v.max(0.0),
                other => warn!("MELD_WORLD_FEEL: no such knob `{other}`"),
            }
        }
    }

    pub(crate) fn from_flags() -> Self {
        let mut feel = Self::default();
        if let Some(spec) = crate::flags::world_feel_flag() {
            feel.apply(&spec);
        }
        feel
    }
}

#[cfg(test)]
mod world_feel_tests {
    use super::*;

    /// `sky.t += dt / day_len` — a zero or negative day is a division that stops the sun
    /// or runs it backwards, and every phase timer counts down from its own value.
    #[test]
    fn every_world_duration_is_positive() {
        let f = WorldFeel::default();
        for (name, secs) in [
            ("day_len", f.day_len),
            ("fair_secs", f.fair_secs),
            ("gust_secs", f.gust_secs),
            ("storm_secs", f.storm_secs),
            ("clearing_secs", f.clearing_secs),
        ] {
            assert!(secs > 0.0, "{name} must be positive, got {secs}");
        }
    }

    /// Rain is punctuation, not the setting. The overworld shipped ~8% wet with a storm
    /// every four minutes, which read as permanent overcast.
    #[test]
    fn it_rains_rarely_and_the_dry_spell_is_what_says_so() {
        let f = WorldFeel::default();
        let cycle = f.fair_secs + f.gust_secs + f.storm_secs + f.clearing_secs;
        let wet = f.storm_secs / cycle;
        assert!(wet < 0.06, "raining {:.0}% of the time", wet * 100.0);
        assert!(
            f.fair_secs > f.gust_secs + f.storm_secs + f.clearing_secs,
            "the storm is longer than the calm between storms"
        );
    }

    /// You should be able to walk out a dive and see the sky turn, without the sun
    /// strobing through a whole day inside one fight.
    #[test]
    fn a_day_outlasts_a_storm_but_not_a_session() {
        let f = WorldFeel::default();
        assert!(f.day_len > f.storm_secs * 4.0, "the sun turns faster than the weather");
        assert!(f.day_len <= 1800.0, "a half-hour of night is a player who cannot see");
    }

    #[test]
    fn overrides_apply_and_a_bad_knob_is_skipped() {
        let mut f = WorldFeel::default();
        f.apply("day_len=900, nonsense=3, fair_secs=oops");
        assert_eq!(f, WorldFeel { day_len: 900.0, ..WorldFeel::default() });
    }

    /// The opening time of day is a PHASE of the cycle, so it has to stay inside one —
    /// `advance_sky` only ever `fract()`s what it adds, and `apply_sky` reads
    /// `sin((t - 0.25) * TAU)`, which a t of 12.0 answers with an arbitrary sun.
    #[test]
    fn the_opening_time_of_day_is_a_fraction_of_a_cycle() {
        let f = WorldFeel::default();
        assert!((0.0..1.0).contains(&f.sky_t), "sky_t {} is not a time of day", f.sky_t);
        for (spec, want) in [("sky_t=0.75", 0.75), ("sky_t=1.25", 0.25), ("sky_t=-0.25", 0.75)] {
            let mut f = WorldFeel::default();
            f.apply(spec);
            assert!((f.sky_t - want).abs() < 1e-5, "{spec} → {}, wanted {want}", f.sky_t);
        }
    }
}
