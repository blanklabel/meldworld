//! Counters for the creature step, so its cost can be read as WORK rather than as wall time
//! on a shared box. Always compiled, always cheap (relaxed atomics), read by the server's
//! `tick_budget_at_depth` benchmark and by nothing in play.

use std::sync::atomic::{AtomicU64, Ordering::Relaxed};

pub const STEPPED: usize = 0;
pub const FREE_CALLS: usize = 1;
pub const SETUP_NS: usize = 2;
pub const GRID_NS: usize = 3;
pub const DECIDE_NS: usize = 4;
pub const APPLY_NS: usize = 5;
pub const DAMAGE_NS: usize = 6;
pub const TAIL_NS: usize = 7;
const N: usize = 8;
const NAMES: [&str; N] =
    ["stepped", "free_calls", "setup_ns", "grid_ns", "decide_ns", "apply_ns", "damage_ns", "tail_ns"];

static COUNTERS: [AtomicU64; N] = [const { AtomicU64::new(0) }; N];

#[inline]
pub fn add(which: usize, n: u64) {
    COUNTERS[which].fetch_add(n, Relaxed);
}

/// Every counter as `(name, value)`, then zeroed.
pub fn take() -> Vec<(&'static str, u64)> {
    NAMES.iter().enumerate().map(|(i, n)| (*n, COUNTERS[i].swap(0, Relaxed))).collect()
}
