//! HTTP auth conformance (BUILD-PLAN M1.1/M1.8/M1.9, CANON.md D17).
//!
//! - register + login issue a session token and realtime ticket;
//! - passwords are stored ONLY as bcrypt (`$2*$12$`) hashes — plaintext appears
//!   nowhere in the row (M1.8);
//! - unknown-username and wrong-password logins return byte-identical bodies
//!   modulo `request_id` — no account-enumeration oracle (M1.9).
//!
//! Requires `MELD_DATABASE_URL` (see qa/scripts/local_pg.sh).

use std::sync::Arc;

use serde_json::Value;
use sqlx::Row;
use tokio::net::TcpListener;

async fn start_server() -> (String, String) {
    let db_url = std::env::var("MELD_DATABASE_URL")
        .expect("set MELD_DATABASE_URL (see qa/scripts/local_pg.sh)");
    let balance = Arc::new(meld_balance::Balance::load_default().unwrap());
    let config = meld_server::Config {
        bind_addr: "127.0.0.1:0".to_string(),
        database_url: db_url.clone(),
        balance,
        client_dist: None,
    };
    let built = meld_server::build(&config).await.expect("server builds");
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, built.router).await.unwrap();
    });
    (format!("{addr}"), db_url)
}

fn strip_request_id(mut v: Value) -> Value {
    if let Some(err) = v.get_mut("error").and_then(|e| e.as_object_mut()) {
        err.remove("request_id");
    }
    v
}

#[tokio::test]
async fn register_login_me_and_enumeration_safety() {
    let (addr, db_url) = start_server().await;
    let client = reqwest::Client::new();
    let base = format!("http://{addr}");
    let username = format!("mazer_{}", &uuid::Uuid::new_v4().simple().to_string()[..8]);
    let password = "correct-horse-battery";
    let body = serde_json::json!({ "username": username, "password": password });

    // Register.
    let reg = client
        .post(format!("{base}/v1/auth/register"))
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(reg.status(), 201);

    // Duplicate register → 409 conflict.
    let dup = client
        .post(format!("{base}/v1/auth/register"))
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(dup.status(), 409);

    // Login → 200 with token + ticket.
    let login = client
        .post(format!("{base}/v1/auth/login"))
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(login.status(), 200);
    let lv: Value = login.json().await.unwrap();
    let token = lv["session_token"].as_str().unwrap().to_string();
    assert_eq!(lv["token_type"], "Bearer");
    assert!(lv["realtime_ticket"].as_str().unwrap().starts_with("rt-"));

    // players/me with the Bearer token.
    let me = client
        .get(format!("{base}/v1/players/me"))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap();
    assert_eq!(me.status(), 200);
    let mv: Value = me.json().await.unwrap();
    assert_eq!(mv["username"], serde_json::json!(username));
    assert_eq!(mv["class_unlocks"][0], "hunter");

    // players/me without a token → 401.
    let anon = client
        .get(format!("{base}/v1/players/me"))
        .send()
        .await
        .unwrap();
    assert_eq!(anon.status(), 401);

    // M1.9: unknown username vs wrong password → identical status + body (sans request_id).
    let unknown = client
        .post(format!("{base}/v1/auth/login"))
        .json(&serde_json::json!({ "username": "definitely_not_a_user_xyz", "password": password }))
        .send()
        .await
        .unwrap();
    let wrong = client
        .post(format!("{base}/v1/auth/login"))
        .json(&serde_json::json!({ "username": username, "password": "wrong-password-here" }))
        .send()
        .await
        .unwrap();
    assert_eq!(unknown.status(), 401);
    assert_eq!(wrong.status(), 401);
    let ub = strip_request_id(unknown.json().await.unwrap());
    let wb = strip_request_id(wrong.json().await.unwrap());
    assert_eq!(
        ub, wb,
        "enumeration oracle: bodies must match modulo request_id"
    );

    // M1.8: the stored password is a bcrypt cost-12 hash, never the plaintext.
    let pool = sqlx::postgres::PgPool::connect(&db_url).await.unwrap();
    let row = sqlx::query("SELECT password_hash FROM players WHERE username = $1")
        .bind(&username)
        .fetch_one(&pool)
        .await
        .unwrap();
    let hash: String = row.get("password_hash");
    assert!(
        hash.starts_with("$2") && hash.contains("$12$"),
        "must be bcrypt cost-12: {hash}"
    );
    assert!(
        !hash.contains(password),
        "plaintext must not appear in the hash"
    );
}
