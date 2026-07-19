//! meld-client library surface. The Bevy app (`main.rs`) and the headless
//! smoke binary (`bin/smoke.rs`) both build on the shared network layer.

pub mod hd2d;
pub mod net;

// The self-contained QA/demo build boots the server in-process; see the module.
#[cfg(feature = "embedded-server")]
pub mod embedded;
