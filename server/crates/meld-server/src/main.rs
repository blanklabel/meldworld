//! MELDWORLD server binary.
//!
//! Boot: load balance + config from env, connect Postgres, run migrations,
//! spawn the game loop, and serve the HTTP API + realtime WebSocket.

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,meld_server=debug".into()),
        )
        .init();

    let config = match meld_server::Config::from_env() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("configuration error: {e}");
            std::process::exit(2);
        }
    };

    if let Err(e) = meld_server::serve(config).await {
        eprintln!("server error: {e}");
        std::process::exit(1);
    }
}
