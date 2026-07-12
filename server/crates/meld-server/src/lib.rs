//! `meld-server` — the WS gateway + authoritative game loop + HTTP mount.
//! [`build`] wires everything into an axum router and the game loop; the binary
//! and the QA integration tests both build on it.

pub mod config;
pub mod game;
pub mod gateway;

use axum::routing::get;
use axum::Router;
use meld_api::{ApiState, Sessions, Tickets};
use meld_db::Db;
use tower_http::cors::CorsLayer;

pub use config::Config;

/// A fully-wired server: the HTTP + realtime router, plus the DB handle (so
/// callers/tests can run migrations or inspect state).
pub struct Built {
    pub router: Router,
    pub db: Db,
}

/// Connect the DB, spawn the game loop, and assemble the combined router
/// (`/v1/*` HTTP API + `/v1/realtime` WebSocket).
pub async fn build(config: &Config) -> Result<Built, String> {
    let balance = config.balance.clone();
    let db = Db::connect(&config.database_url, balance.auth.bcrypt_cost)
        .await
        .map_err(|e| format!("db connect: {e}"))?;
    db.migrate().await.map_err(|e| format!("migrate: {e}"))?;

    let tickets = Tickets::new(balance.session.realtime_ticket_ttl_ms);
    let sessions = Sessions::new();

    // HTTP API (auth, players/me, healthz).
    let api = meld_api::router(ApiState {
        db: db.clone(),
        tickets: tickets.clone(),
        sessions: sessions.clone(),
        session_ttl_secs: balance.auth.session_token_ttl_secs,
    });

    // Realtime gateway.
    let game = game::spawn(balance.clone(), db.clone());
    let gateway_state = gateway::GatewayState {
        db: db.clone(),
        tickets,
        game,
        heartbeat_interval_ms: balance.session.heartbeat_interval_ms,
        grace_window_ms: balance.session.grace_window_ms,
        auth_timeout_ms: balance.session.auth_timeout_ms.max(0) as u64,
    };
    let realtime = Router::new()
        .route("/v1/realtime", get(gateway::realtime_handler))
        .with_state(gateway_state);

    // Permissive CORS on the HTTP API only, so a browser client on another
    // origin can call it. The WS route is left bare — CORS middleware corrupts
    // the 101 upgrade, and cross-origin WebSockets aren't CORS-governed anyway.
    let router = realtime.merge(api.layer(CorsLayer::permissive()));
    Ok(Built { router, db })
}

/// Build and serve until the process is killed.
pub async fn serve(config: Config) -> Result<(), String> {
    let built = build(&config).await?;
    let listener = tokio::net::TcpListener::bind(&config.bind_addr)
        .await
        .map_err(|e| format!("bind {}: {e}", config.bind_addr))?;
    tracing::info!("meldworld server listening on {}", config.bind_addr);
    axum::serve(listener, built.router)
        .await
        .map_err(|e| format!("serve: {e}"))
}
