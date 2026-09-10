//! Per-phase timing for the authoritative loop, on when `MELD_TICK_STATS=1`.
//!
//! The loop had no clock of its own: every "the tick costs X" in this repo came from a
//! one-off test or a stopwatch someone remembered to hold. This is the standing instrument.
//! Off, each phase costs one branch. On, it sums and maxes each phase and reports every
//! `REPORT_EVERY` through `tracing::info!` so a tick that spikes shows up as a `max`.

use std::time::{Duration, Instant};

const REPORT_EVERY: Duration = Duration::from_secs(5);

#[derive(Default, Clone, Copy)]
struct Phase {
    name: &'static str,
    sum: f64,
    max: f64,
    n: u32,
}

pub struct Prof {
    enabled: bool,
    phases: Vec<Phase>,
    last_report: Option<Instant>,
    last_tick: Option<Instant>,
    period: Phase,
}

impl Prof {
    pub fn from_env() -> Self {
        let enabled = std::env::var("MELD_TICK_STATS").is_ok_and(|v| v != "0" && !v.is_empty());
        Self::new(enabled)
    }

    pub fn new(enabled: bool) -> Self {
        Self {
            enabled,
            phases: Vec::new(),
            last_report: None,
            last_tick: None,
            period: Phase { name: "period", ..Default::default() },
        }
    }

    #[inline]
    pub fn enabled(&self) -> bool {
        self.enabled
    }

    #[inline]
    pub fn time<T>(&mut self, name: &'static str, f: impl FnOnce() -> T) -> T {
        if !self.enabled {
            return f();
        }
        let t = Instant::now();
        let out = f();
        self.record(name, t.elapsed().as_secs_f64() * 1000.0);
        out
    }

    /// Record the time since `mark` under `name` and move `mark` to now — a sequence of
    /// laps is how a straight-line function is phased without closures.
    #[inline]
    pub fn lap(&mut self, mark: &mut Instant, name: &'static str) {
        if !self.enabled {
            return;
        }
        let now = Instant::now();
        self.record(name, (now - *mark).as_secs_f64() * 1000.0);
        *mark = now;
    }

    pub fn record(&mut self, name: &'static str, ms: f64) {
        if !self.enabled {
            return;
        }
        let p = match self.phases.iter_mut().find(|p| p.name == name) {
            Some(p) => p,
            None => {
                self.phases.push(Phase { name, ..Default::default() });
                self.phases.last_mut().unwrap()
            }
        };
        p.sum += ms;
        p.max = p.max.max(ms);
        p.n += 1;
    }

    /// Call once per tick, at its start: measures the real inter-tick period (the
    /// scheduler's jitter, which is what a player feels as a hitch) and emits the report
    /// when it is due.
    pub fn tick_start(&mut self) {
        if !self.enabled {
            return;
        }
        let now = Instant::now();
        if let Some(prev) = self.last_tick {
            let ms = (now - prev).as_secs_f64() * 1000.0;
            self.period.sum += ms;
            self.period.max = self.period.max.max(ms);
            self.period.n += 1;
        }
        self.last_tick = Some(now);
        let due = self.last_report.is_none_or(|t| now - t >= REPORT_EVERY);
        if due {
            self.report();
            self.last_report = Some(now);
        }
    }

    /// The report as a line, and reset — what `tick_start` logs on its cadence, exposed so
    /// a benchmark can print it.
    pub fn take_report(&mut self) -> String {
        let mut line = String::new();
        let mean = |p: &Phase| if p.n == 0 { 0.0 } else { p.sum / p.n as f64 };
        line.push_str(&format!(
            "period {:.1}/{:.1}ms",
            mean(&self.period),
            self.period.max
        ));
        // Sorted by total so the first entry is where the tick goes.
        self.phases.sort_by(|a, b| b.sum.partial_cmp(&a.sum).unwrap_or(std::cmp::Ordering::Equal));
        for p in &self.phases {
            if p.n == 0 {
                continue;
            }
            line.push_str(&format!(" | {} {:.2}/{:.2}", p.name, mean(p), p.max));
        }
        for p in &mut self.phases {
            *p = Phase { name: p.name, ..Default::default() };
        }
        self.period = Phase { name: "period", ..Default::default() };
        line
    }

    fn report(&mut self) {
        if self.period.n == 0 {
            return;
        }
        let line = self.take_report();
        tracing::info!(target: "meld_tick", "{line}");
    }
}
