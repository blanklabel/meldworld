//! Battle feel: the timings and magnitudes behind the ATB's juice, in one tunable place.
//!
//! These are *presentation* numbers — the authoritative pacing (`tick_ms`,
//! `gauge_fill_divisor`, `turn_timeout_ms`) stays in `balance.toml` server-side. But the
//! client is a separate workspace sharing only `meld-proto`, so it has no balance loader
//! to hang these off, and they had drifted into bare `const`s in `main.rs` plus magic
//! literals inside the animation systems — which made dialing the feel in a recompile per
//! guess. One struct, defaults that are exactly what shipped, and a runtime override:
//! `MELD_FEEL="lunge_ttl=0.5,number_rise=70"` natively, `?feel=…` in the browser.

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
