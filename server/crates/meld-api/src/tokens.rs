//! In-memory credential stores: opaque session tokens (24 h) and single-use
//! realtime tickets (60 s). Opaque + server-side (D17) — clients never parse
//! them. Shared (Arc) so the realtime gateway validates against the same state.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use uuid::Uuid;

fn random_token(prefix: &str) -> String {
    // 128 bits of randomness, hex-encoded. Opaque to clients.
    let a: u64 = rand::random();
    let b: u64 = rand::random();
    format!("{prefix}{a:016x}{b:016x}")
}

/// Single-use realtime tickets consumed by `session.authenticate`.
#[derive(Clone)]
pub struct Tickets {
    inner: Arc<Mutex<HashMap<String, (Uuid, Instant)>>>,
    ttl: Duration,
}

impl Tickets {
    pub fn new(ttl_ms: i64) -> Self {
        Self {
            inner: Arc::new(Mutex::new(HashMap::new())),
            ttl: Duration::from_millis(ttl_ms.max(0) as u64),
        }
    }

    /// Mint a fresh ticket for `player_id`.
    pub fn mint(&self, player_id: Uuid) -> String {
        let ticket = random_token("rt-");
        let mut m = self.inner.lock().unwrap();
        m.retain(|_, (_, exp)| *exp > Instant::now());
        m.insert(ticket.clone(), (player_id, Instant::now() + self.ttl));
        ticket
    }

    /// Consume a ticket exactly once. `None` if unknown, expired, or reused.
    pub fn consume(&self, ticket: &str) -> Option<Uuid> {
        let mut m = self.inner.lock().unwrap();
        match m.remove(ticket) {
            Some((pid, exp)) if exp > Instant::now() => Some(pid),
            _ => None,
        }
    }
}

/// Opaque Bearer session tokens for the HTTP API.
///
/// `RwLock` (not `Mutex`) because `resolve` — hit on *every* authenticated HTTP
/// request — is a non-consuming read, so many requests can validate concurrently;
/// only the infrequent `mint` takes the write lock.
#[derive(Clone, Default)]
pub struct Sessions {
    inner: Arc<RwLock<HashMap<String, (Uuid, Instant)>>>,
}

impl Sessions {
    pub fn new() -> Self {
        Self::default()
    }

    /// Mint a session token valid for `ttl_secs`.
    pub fn mint(&self, player_id: Uuid, ttl_secs: i64) -> String {
        let token = random_token("mw-sess-");
        let mut m = self.inner.write().unwrap();
        m.retain(|_, (_, exp)| *exp > Instant::now());
        m.insert(
            token.clone(),
            (
                player_id,
                Instant::now() + Duration::from_secs(ttl_secs.max(0) as u64),
            ),
        );
        token
    }

    /// Resolve a token to its player id, if valid and unexpired. Non-consuming.
    pub fn resolve(&self, token: &str) -> Option<Uuid> {
        let m = self.inner.read().unwrap();
        match m.get(token) {
            Some((pid, exp)) if *exp > Instant::now() => Some(*pid),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ticket_is_single_use() {
        let t = Tickets::new(60_000);
        let pid = Uuid::now_v7();
        let ticket = t.mint(pid);
        assert_eq!(t.consume(&ticket), Some(pid));
        assert_eq!(t.consume(&ticket), None, "second use must fail");
    }

    #[test]
    fn session_resolves_until_expiry() {
        let s = Sessions::new();
        let pid = Uuid::now_v7();
        let tok = s.mint(pid, 86_400);
        assert_eq!(s.resolve(&tok), Some(pid));
        assert_eq!(s.resolve("mw-sess-nope"), None);
    }
}
