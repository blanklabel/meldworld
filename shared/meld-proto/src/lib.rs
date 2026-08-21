//! `meld-proto` — the single source of truth for every wire type shared by the
//! server and (eventually) the Bevy client. Plain Rust + serde derives, no
//! codegen, consumed directly by both sides (BUILD-PLAN T1).
//!
//! Layout mirrors CANON.md §I:
//! - [`envelope`] — the realtime `{type, seq, ts, payload}` frame.
//! - [`enums`] — canonical enums (`CharacterClass`, error codes, …).
//! - [`equipment`] — which gear each class may wear (GR-5).
//! - [`affixes`] — the rolled qualities that make a drop a build (AD-1).
//! - [`uniques`] — named uniques (with a drawback) and party-wide sets (AD-1).
//! - [`synergies`] — class-pair synergies and sequenced ability combos (AD-2).
//! - [`consumables`] — potions, what they do, and the recipes that make them (GR-4/MS-1).
//! - [`materials`] — every crafting material and its class (reagent/ore/trophy) (MS-1).
//! - [`common`] — shared payload objects (`Position`, `ItemStack`, `Combatant`).
//! - [`realtime`] — C2S/S2C message payloads by domain.
//! - [`http`] — HTTP request/response DTOs.
//! - [`limits`] — field bounds and validators (docs/edge-cases/limits.md).

pub mod abilities;
pub mod affixes;
pub mod bosses;
pub mod bounties;
pub mod common;
pub mod consumables;
pub mod enums;
pub mod envelope;
pub mod equipment;
pub mod factions;
pub mod http;
pub mod hubs;
pub mod hunts;
pub mod limits;
pub mod materials;
pub mod names;
pub mod realtime;
pub mod skills;
pub mod statuses;
pub mod structures;
pub mod synergies;
pub mod coast;
pub mod terrain;
pub mod unlocks;
pub mod warbands;
pub mod uniques;

pub use enums::*;
pub use envelope::{Envelope, RawEnvelope};

/// UUIDv7 string, server-generated (CANON.md §I). We carry ids as `String` on
/// the wire so the proto crate stays free of a uuid-version opinion; the server
/// mints them with `uuid::Uuid::now_v7()`.
pub type Id = String;

/// Unix milliseconds UTC — the realtime timestamp type (CANON.md §I).
pub type UnixMillis = u64;
