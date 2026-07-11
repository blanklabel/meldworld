//! The realtime envelope (CANON.md §I): every frame in both directions is
//! `{ "type": string, "seq": u32, "ts": u64, "payload": object }`.

use serde::{Deserialize, Serialize};

use crate::UnixMillis;

/// A fully-typed envelope wrapping a message payload `T`.
///
/// `type` is carried by the payload enum's own `#[serde(tag = "type")]` in the
/// realtime module, so here we keep `type` as an explicit string field to match
/// the exact wire shape and allow round-tripping without knowing `T`'s variant.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Envelope<T> {
    /// `<domain>.<verb_phrase>`, pattern `^[a-z]+\.[a-z_]+$`.
    #[serde(rename = "type")]
    pub msg_type: String,
    /// Per-session monotonic sequence number, ≥ 1.
    pub seq: u32,
    /// Sender wall-clock, Unix millis UTC.
    pub ts: UnixMillis,
    /// Message-specific body.
    pub payload: T,
}

impl<T> Envelope<T> {
    pub fn new(msg_type: impl Into<String>, seq: u32, ts: UnixMillis, payload: T) -> Self {
        Self {
            msg_type: msg_type.into(),
            seq,
            ts,
            payload,
        }
    }
}

/// An envelope whose payload is left as raw JSON, for the gateway to peek at
/// `type`/`seq` before dispatching to the typed payload decoder.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RawEnvelope {
    #[serde(rename = "type")]
    pub msg_type: String,
    pub seq: u32,
    pub ts: UnixMillis,
    pub payload: serde_json::Value,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn envelope_round_trips_byte_stable() {
        // Golden fixture from realtime-protocol.md §Envelope Format.
        let json = r#"{"type":"battle.submit_action","seq":42,"ts":1783728000000,"payload":{}}"#;
        let env: Envelope<serde_json::Value> = serde_json::from_str(json).unwrap();
        assert_eq!(env.msg_type, "battle.submit_action");
        assert_eq!(env.seq, 42);
        assert_eq!(env.ts, 1783728000000);
        let back = serde_json::to_string(&env).unwrap();
        assert_eq!(back, json);
    }

    #[test]
    fn raw_envelope_reads_type_without_payload_schema() {
        let json = r#"{"type":"session.heartbeat","seq":57,"ts":1783728030000,"payload":{}}"#;
        let raw: RawEnvelope = serde_json::from_str(json).unwrap();
        assert_eq!(raw.msg_type, "session.heartbeat");
        assert_eq!(raw.seq, 57);
    }
}
