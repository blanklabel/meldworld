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
use tower_http::services::ServeDir;

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
        meld_xp_per_level: balance.meld.xp_per_level,
        meld_forging_xp: balance.meld.forging_xp_per_craft,
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
    let mut router = realtime.merge(api.layer(CorsLayer::permissive()));

    // Optionally serve the built wasm client at `/` so the whole game is
    // same-origin: the browser loads the page, calls `/v1/*`, and opens the
    // `/v1/realtime` WebSocket all against this one server — no dev-proxy in
    // the loop (trunk's WS proxy is unreliable) and no cross-origin at all.
    if let Some(dist) = &config.client_dist {
        router = router.fallback_service(ServeDir::new(dist));
        tracing::info!("serving wasm client from {dist} at /");
    }

    Ok(Built { router, db })
}

/// Build and serve until the process is killed.
pub async fn serve(config: Config) -> Result<(), String> {
    serve_reporting(config, |addr| {
        tracing::info!("meldworld server listening on {addr}");
    })
    .await
}

/// Like [`serve`], but reports the **actually-bound** socket address through
/// `on_bound` just before it starts serving. Bind to a `…:0` port and this hands
/// back the OS-chosen port — which is how the self-contained, in-process client
/// build (the `embedded-server` feature) learns where to point itself without a
/// fixed port that could collide.
pub async fn serve_reporting(
    config: Config,
    on_bound: impl FnOnce(std::net::SocketAddr),
) -> Result<(), String> {
    let built = build(&config).await?;
    let listener = tokio::net::TcpListener::bind(&config.bind_addr)
        .await
        .map_err(|e| format!("bind {}: {e}", config.bind_addr))?;
    let addr = listener
        .local_addr()
        .map_err(|e| format!("local_addr: {e}"))?;
    on_bound(addr);
    axum::serve(listener, built.router)
        .await
        .map_err(|e| format!("serve: {e}"))
}
