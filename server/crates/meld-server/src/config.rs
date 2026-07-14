//! Server configuration, resolved from env + balance.toml at boot.

use meld_balance::Balance;

#[derive(Clone)]
pub struct Config {
    pub bind_addr: String,
    pub database_url: String,
    pub balance: std::sync::Arc<Balance>,
    /// Optional path to the built wasm client (`dist/`). When set, the server
    /// serves it at `/` so the whole game is same-origin — no proxy, no CORS,
    /// and browser WebSockets connect straight to `/v1/realtime`.
    pub client_dist: Option<String>,
}

impl Config {
    /// Resolve from the environment:
    /// - `MELD_ADDR` (else `0.0.0.0:$PORT`, else `0.0.0.0:8080`)
    /// - `MELD_DATABASE_URL` (required)
    /// - balance via `MELD_BALANCE` or the checked-in default.
    pub fn from_env() -> Result<Self, String> {
        let bind_addr = std::env::var("MELD_ADDR").unwrap_or_else(|_| {
            let port = std::env::var("PORT").unwrap_or_else(|_| "8080".to_string());
            format!("0.0.0.0:{port}")
        });
        let database_url = std::env::var("MELD_DATABASE_URL").map_err(|_| {
            "MELD_DATABASE_URL must be set (Postgres connection string)".to_string()
        })?;
        let balance = Balance::load_default().map_err(|e| e.to_string())?;
        let client_dist = std::env::var("MELD_CLIENT_DIST").ok().filter(|s| !s.is_empty());
        Ok(Config {
            bind_addr,
            database_url,
            balance: std::sync::Arc::new(balance),
            client_dist,
        })
    }
}
