//! **THE SKY BELONGS TO THE WORLD, NOT TO THE VIEWER** (`FS-5`, and `FS-2`'s first half).
//!
//! Time of day and weather were a per-client animation: `world_render::advance_sky`
//! accumulated wall-clock into a 10-minute day and ran its own four-phase storm machine,
//! seeded off nothing. Two people standing on the same patch of ground could disagree
//! about whether it was night, and about whether it was raining. That is survivable in a
//! single-player look-dev toy and nonsense in a shared world — and it is why nothing can
//! be *gated* on the time of day: there is no such fact to gate on.
//!
//! So the sky is derived here, from `(seed, tick)`, by both sides:
//!
//! - **The clock is the world's tick counter**, which already exists and is already the
//!   thing CANON §W2 insists everything be scheduled against ("never wall-clock"). The
//!   Shift is scheduled that way for the same reason, and it buys the same two properties
//!   here: a world replays identically from `(seed, tick)`, and a world that slept
//!   wakes up at the right hour **by construction** rather than by anybody remembering to
//!   advance a clock.
//! - **Nothing about the sky is sent per frame.** The server tells a client what tick the
//!   world is on, rarely; the client runs its own estimate forward between those and
//!   derives everything locally. A 60 Hz sky over the wire would be absurd, and a sky the
//!   server *interpolates for you* would be a second clock that can disagree with the
//!   first.
//!
//! ⚠️ **The magnitudes ride the wire because the client has no `balance.toml`.** Same
//! reason and same shape as [`crate::regions`]: structure is code here, coefficients are
//! `[weather]` in balance and travel on `run.started` inside [`Sky`]. Do not hard-code a
//! duration in this file.
//!
//! ⚠️ **What this is NOT.** Weather has no mechanical effect yet — no visibility, no
//! movement or harvest modifier, no elemental interaction. That is the rest of `FS-2`.
//! This makes the sky *authoritative and shared*, which is the thing that has to be true
//! before any rule can be hung off it.

use serde::{Deserialize, Serialize};

/// Fair — a breeze, no rain.
pub const FAIR: u8 = 0;
/// Gust — the wind rises. A storm is coming, and the wind LEADS it.
pub const GUST: u8 = 1;
/// Storm — the downpour, where the biome allows one.
pub const STORM: u8 = 2;
/// Clearing — the wind drops away.
pub const CLEARING: u8 = 3;

/// The world's sky constants, resolved from `[weather]` server-side and carried to the
/// client on `run.started`. Every method here is a pure function of these plus
/// `(seed, tick)`, so both sides reach the same sky without exchanging one.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Sky {
    /// **How long a tick is.** Every other duration here is in ticks, so this is what
    /// makes the struct self-describing — and it is why the client does not hard-code it:
    /// `[battle] tick_ms` is authoritative pacing, the client has no `balance.toml`, and a
    /// client guessing the server's cadence would drift by construction.
    pub tick_ms: u32,
    /// Ticks in one full day → night → day cycle.
    pub day_ticks: u32,
    /// The dry spell. Much the longest phase: it is what governs how OFTEN weather
    /// happens, so it is the one to move when the world reads as permanently overcast.
    pub fair_ticks: u32,
    pub gust_ticks: u32,
    pub storm_ticks: u32,
    pub clearing_ticks: u32,
    /// One storm in this many is a **super storm** — it blows harder and soaks the whole
    /// area rather than the patch under the cloud. `0` disables them.
    pub super_storm_in: u32,
    /// Per-biome chance that a given cycle's storm actually **wets that biome**, indexed
    /// by [`crate::regions::BIOMES`].
    ///
    /// ⚠️ **The CADENCE is global and only the precipitation is local**, which is the
    /// whole trick. Giving each biome its own phase durations would mean the sky changes
    /// at a different moment on either side of a biome boundary — so walking across one
    /// would jump the weather, and a party spread over two cells would disagree about the
    /// time again, which is the bug this module exists to fix. Instead every cell of a
    /// world moves through the same cycle at the same moment, and a desert simply spends
    /// most of its storms windy and dry. Deserts should rarely rain (`FS-2`); they should
    /// not be on their own calendar.
    pub rain_chance: Vec<f32>,
}

impl Sky {
    /// One full weather cycle: fair → gust → storm → clearing.
    pub fn cycle_ticks(&self) -> u64 {
        (self.fair_ticks as u64
            + self.gust_ticks as u64
            + self.storm_ticks as u64
            + self.clearing_ticks as u64)
            .max(1)
    }

    /// Time of day as a fraction of the cycle: `0.0`/`1.0` midnight, `0.25` sunrise,
    /// `0.5` noon, `0.75` sunset — the same convention the client's `sky_t` already used,
    /// so the screenshot flag and the shader keep their meaning.
    pub fn time_of_day(&self, tick: u64) -> f32 {
        let day = self.day_ticks.max(1) as u64;
        (tick % day) as f32 / day as f32
    }

    /// `(phase, cycle index)` at this tick. The cycle index is what a per-storm roll is
    /// keyed on, so the same storm is the same storm for everyone who watches it.
    pub fn phase_at(&self, tick: u64) -> (u8, u64) {
        let cycle_len = self.cycle_ticks();
        let cycle = tick / cycle_len;
        let mut at = tick % cycle_len;
        for (phase, len) in [
            (FAIR, self.fair_ticks),
            (GUST, self.gust_ticks),
            (STORM, self.storm_ticks),
            (CLEARING, self.clearing_ticks),
        ] {
            let len = len as u64;
            if at < len {
                return (phase, cycle);
            }
            at -= len;
        }
        (CLEARING, cycle)
    }

    /// Is this cycle's storm a super storm? Rolled off `(seed, cycle)` so it is the
    /// world's fact rather than each viewer's — the client used to roll it from a local
    /// counter, which is exactly how two people watched different storms.
    pub fn super_storm(&self, seed: u64, cycle: u64) -> bool {
        if self.super_storm_in == 0 {
            return false;
        }
        roll(seed ^ 0x5EED_5701, cycle).is_multiple_of(self.super_storm_in as u64)
    }

    /// Does this cycle's storm actually wet `biome` (an index into
    /// [`crate::regions::BIOMES`])? A separate stream from [`Self::super_storm`], so
    /// retuning one biome's dryness cannot change which storms are severe.
    pub fn rains_on(&self, seed: u64, cycle: u64, biome: usize) -> bool {
        let chance = self.rain_chance.get(biome).copied().unwrap_or(1.0);
        unit(roll(seed ^ 0x5EED_5702, cycle.wrapping_mul(31).wrapping_add(biome as u64))) < chance
    }

    /// The two magnitudes a renderer wants, as **targets**: `(wind, rain)` in `0..=1`.
    ///
    /// Targets rather than smoothed values on purpose. The authoritative fact is which
    /// phase the world is in and whether this storm is severe and wet; how fast a
    /// particular client eases the trees into it is a *look*, and belongs on `WorldFeel`
    /// with the rest of the look. Everyone converges on the same sky within a second, and
    /// nobody has to make the wire carry a frame rate.
    pub fn wind_and_rain(&self, seed: u64, tick: u64, biome: usize) -> (f32, f32) {
        let (phase, cycle) = self.phase_at(tick);
        let severe = self.super_storm(seed, cycle);
        let wet = self.rains_on(seed, cycle, biome);
        match phase {
            GUST => (0.7, 0.0),
            // ⚠️ A STORM MUST OUTBLOW ITS OWN PRECURSOR. Gust targets 0.7, so an ordinary
            // storm below that blows SOFTER than the wind that announced it — the weather
            // peaks and then eases exactly as the rain arrives, backwards from the shape
            // the phase machine exists to tell. A DRY storm still blows: that is what a
            // desert's weather is, and it is why the wind does not read `wet`.
            STORM => (if severe { 1.0 } else { 0.82 }, if wet { 1.0 } else { 0.0 }),
            CLEARING => (0.3, 0.0),
            // ⚠️ FAIR IS A BREEZE, NOT DEAD CALM. Sway and the grass lean both scale off
            // wind, and fair is much the longest phase — at 0.0 the world would stand
            // perfectly still for the overwhelming majority of its wall-clock, and the one
            // window where anything moved would be under a minute in every eleven.
            _ => (0.15, 0.0),
        }
    }
}

/// splitmix64 of two words — the same mixer the rest of the world generator uses, so a
/// sky roll behaves like every other seeded roll in the game.
fn roll(seed: u64, n: u64) -> u64 {
    let mut z = seed
        .wrapping_add(n.wrapping_mul(0x9E37_79B9_7F4A_7C15))
        .wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

fn unit(r: u64) -> f32 {
    (r >> 11) as f32 / (1u64 << 53) as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sky() -> Sky {
        Sky {
            tick_ms: 100,
            day_ticks: 6000,
            fair_ticks: 6000,
            gust_ticks: 160,
            storm_ticks: 220,
            clearing_ticks: 140,
            super_storm_in: 5,
            rain_chance: vec![1.0; crate::regions::BIOMES.len()],
        }
    }

    /// The headline: the sky is a pure function of `(seed, tick)`, so two clients that
    /// agree on the tick agree on the sky. There is nothing else to check — no state, no
    /// accumulation, no wall-clock — and that is the property.
    #[test]
    fn the_sky_is_a_function_of_the_world_clock() {
        let s = sky();
        for tick in [0u64, 1, 999, 6_001, 1_234_567, u32::MAX as u64] {
            assert_eq!(s.time_of_day(tick), s.time_of_day(tick));
            assert_eq!(s.phase_at(tick), s.phase_at(tick));
            assert_eq!(s.wind_and_rain(7, tick, 1), s.wind_and_rain(7, tick, 1));
        }
    }

    /// A day is a day: midnight, sunrise, noon and sunset land where the client's shader
    /// and the `sky_t` screenshot flag already believe they do.
    #[test]
    fn the_day_turns_over_at_the_hours_everything_already_assumes() {
        let s = sky();
        assert_eq!(s.time_of_day(0), 0.0);
        assert!((s.time_of_day(1500) - 0.25).abs() < 1e-6);
        assert!((s.time_of_day(3000) - 0.5).abs() < 1e-6);
        assert!((s.time_of_day(4500) - 0.75).abs() < 1e-6);
        assert_eq!(s.time_of_day(6000), 0.0, "the day wraps");
    }

    /// Fair → Gust → Storm → Clearing, in that order, every cycle, with the wind LEADING
    /// the rain. The ordering is the shape the weather exists to tell; the durations are
    /// `[TUNABLE]`, so this walks the phases rather than asserting a tick.
    #[test]
    fn the_weather_keeps_its_shape() {
        let s = sky();
        let seen: Vec<u8> = (0..s.cycle_ticks()).map(|t| s.phase_at(t).0).collect();
        let mut order: Vec<u8> = Vec::new();
        for p in seen {
            if order.last() != Some(&p) {
                order.push(p);
            }
        }
        assert_eq!(order, vec![FAIR, GUST, STORM, CLEARING]);

        // The wind rises before the rain arrives and outblows its own precursor.
        let gust = s.wind_and_rain(1, s.fair_ticks as u64 + 1, 1);
        let storm = s.wind_and_rain(1, (s.fair_ticks + s.gust_ticks) as u64 + 1, 1);
        assert!(gust.0 > 0.0 && gust.1 == 0.0, "the gust is wind without rain");
        assert!(storm.0 > gust.0, "a storm must outblow the wind that announced it");
        assert!(storm.1 > 0.0);
    }

    /// **A desert rarely rains, and it still gets the wind** (`FS-2`). The dry half of a
    /// storm is what makes a biome's weather its own without putting it on its own
    /// calendar — the cadence stays global so the sky never jumps as you cross a boundary.
    #[test]
    fn a_dry_biome_gets_the_storm_without_the_rain() {
        let mut s = sky();
        let desert = crate::regions::BIOMES.iter().position(|b| *b == "desert").unwrap();
        let mire = crate::regions::BIOMES.iter().position(|b| *b == "mire").unwrap();
        s.rain_chance[desert] = 0.0;
        s.rain_chance[mire] = 1.0;

        let storm_at = (s.fair_ticks + s.gust_ticks) as u64 + 1;
        let (dw, dr) = s.wind_and_rain(9, storm_at, desert);
        let (mw, mr) = s.wind_and_rain(9, storm_at, mire);
        assert_eq!(dr, 0.0, "the desert's storm is dry");
        assert!(mr > 0.0, "the mire's is not");
        assert_eq!(dw, mw, "…and both feel the same wind, on the same schedule");
        assert_eq!(
            s.phase_at(storm_at).0,
            STORM,
            "one cadence for the whole world — a biome must not keep its own calendar"
        );
    }

    /// A rain chance is a RATE, not a switch: a biome set halfway must actually land
    /// somewhere near halfway over many storms, or "deserts rarely rain" would mean
    /// "deserts never rain" and the tunable would be a boolean wearing a float's clothes.
    #[test]
    fn a_rain_chance_is_a_rate() {
        let mut s = sky();
        let b = 1;
        s.rain_chance[b] = 0.25;
        let wet = (0..4000u64).filter(|c| s.rains_on(424242, *c, b)).count();
        assert!((900..=1100).contains(&wet), "expected ~1000 wet storms in 4000, got {wet}");
    }

    /// Severity and wetness are separate streams, so retuning one biome's dryness cannot
    /// silently change which storms are severe everywhere else.
    #[test]
    fn severity_and_wetness_do_not_share_a_stream() {
        let s = sky();
        let severe: Vec<bool> = (0..500).map(|c| s.super_storm(5, c)).collect();
        let wet: Vec<bool> = (0..500).map(|c| s.rains_on(5, c, 1)).collect();
        let agree = severe.iter().zip(&wet).filter(|(a, b)| a == b).count();
        assert!(agree < 480, "the two rolls track each other too closely ({agree}/500)");
    }
}
