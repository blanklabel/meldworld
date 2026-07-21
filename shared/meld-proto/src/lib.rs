//! `meld-proto` — the single source of truth for every wire type shared by the
//! server and (eventually) the Bevy client. Plain Rust + serde derives, no
//! codegen, consumed directly by both sides (BUILD-PLAN T1).
//!
//! Layout mirrors CANON.md §I:
//! - [`envelope`] — the realtime `{type, seq, ts, payload}` frame.
//! - [`enums`] — canonical enums (`CharacterClass`, error codes, …).
//! - [`common`] — shared payload objects (`Position`, `ItemStack`, `Combatant`).
//! - [`realtime`] — C2S/S2C message payloads by domain.
//! - [`http`] — HTTP request/response DTOs.
//! - [`limits`] — field bounds and validators (docs/edge-cases/limits.md).

pub mod common;
pub mod enums;
pub mod envelope;
pub mod factions;
pub mod http;
pub mod limits;
pub mod realtime;
pub mod skills;

pub use enums::*;
pub use envelope::{Envelope, RawEnvelope};

/// UUIDv7 string, server-generated (CANON.md §I). We carry ids as `String` on
/// the wire so the proto crate stays free of a uuid-version opinion; the server
/// mints them with `uuid::Uuid::now_v7()`.
pub type Id = String;

/// Unix milliseconds UTC — the realtime timestamp type (CANON.md §I).
pub type UnixMillis = u64;
