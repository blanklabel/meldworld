//! meld-client library surface. The Bevy app (`main.rs`) and the headless
//! smoke binary (`bin/smoke.rs`) both build on the shared network layer.

// Two clippy lints fire on essentially every Bevy system in this crate and neither is
// telling us anything: a system's parameters ARE its dependency list (`too_many_arguments`
// counts them), and a `Query` with filters is a type by construction (`type_complexity`).
// Allowed crate-wide and named here rather than sprinkled over ~30 individual items, so
// the rest of clippy's output stays worth reading.
#![allow(clippy::too_many_arguments)]
#![allow(clippy::type_complexity)]

pub mod glass;
pub mod hd2d;
pub mod net;

// The self-contained QA/demo build boots the server in-process; see the module.
#[cfg(feature = "embedded-server")]
pub mod embedded;
