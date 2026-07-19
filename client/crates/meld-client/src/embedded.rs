//! In-process server for the self-contained QA / demo binary (`embedded-server`
//! feature). Instead of talking to a separate `meld-server` over the network, the
//! native client boots the whole authoritative server on a background thread with
//! an **in-memory** database and the **embedded** balance table — so the single
//! binary needs no Postgres, no server process, and no config files beside it.
//!
//! Everything is ephemeral: accounts, the Vault, hero renames and progression all
//! live in RAM and reset when the binary exits. That's exactly what you want for
//! "hand a tester one file and let them poke at a clean slate."

use std::sync::mpsc;

/// Boot the in-process server and point this client at it. Spawns a background
/// thread that owns a Tokio runtime and serves forever; blocks only until the
/// server has bound its (OS-chosen, ephemeral) port, then sets `MELD_SERVER` so
/// the normal `server_base()` path picks it up. Called once, before the Bevy app
/// reads the base URL.
pub fn boot() {
    // Tell the server to use the dependency-free in-memory store and an ephemeral
    // localhost port (0 → the OS picks a free one, reported back below). Set here,
    // in the parent, so `Config::from_env` on the server thread sees them.
    std::env::set_var("MELD_DATABASE_URL", "memory://qa");
    std::env::set_var("MELD_ADDR", "127.0.0.1:0");

    let (tx, rx) = mpsc::channel();
    std::thread::Builder::new()
        .name("meld-embedded-server".into())
        .spawn(move || {
            let rt = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .expect("build embedded tokio runtime");
            rt.block_on(async move {
                let config = match meld_server::Config::from_env() {
                    Ok(c) => c,
                    Err(e) => {
                        eprintln!("embedded server config error: {e}");
                        return;
                    }
                };
                let report = move |addr: std::net::SocketAddr| {
                    // Receiver may be gone if boot() already timed out; ignore.
                    let _ = tx.send(addr);
                };
                if let Err(e) = meld_server::serve_reporting(config, report).await {
                    eprintln!("embedded server error: {e}");
                }
            });
        })
        .expect("spawn embedded server thread");

    // Wait for the bound address so the client connects to a server that's up.
    let addr = rx.recv().expect("embedded server failed to start");
    let base = format!("http://{addr}");
    eprintln!("▶ MELDWORLD running self-contained at {base} (in-memory, ephemeral)");
    std::env::set_var("MELD_SERVER", base);
}
