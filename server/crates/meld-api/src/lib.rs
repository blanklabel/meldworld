//! HTTP API (axum) — auth + player surface for the today-slice
//! (interfaces/http-api.md, auth-players.md; CANON.md D17).
//!
//! Also owns the two short-lived credential stores the realtime gateway needs:
//! opaque Bearer **session tokens** (24 h) and single-use **realtime tickets**
//! (60 s). Both live here so the gateway (which depends on this crate) validates
//! against the same state.

pub mod tokens;

use std::sync::Arc;

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use meld_db::{Db, DbError, PlayerRow};
use meld_proto::enums::CharacterClass;
use meld_proto::http::*;
use meld_proto::limits;
use uuid::Uuid;

pub use tokens::{Sessions, Tickets};

/// Shared HTTP state. Cheap to clone (pool handle + Arc stores).
#[derive(Clone)]
pub struct ApiState {
    pub db: Db,
    pub tickets: Tickets,
    pub sessions: Sessions,
    pub session_ttl_secs: i32,
}

pub fn router(state: ApiState) -> Router {
    Router::new()
        .route("/v1/healthz", get(healthz))
        .route("/v1/auth/register", post(register))
        .route("/v1/auth/login", post(login))
        .route("/v1/players/me", get(players_me))
        .route("/v1/vault", get(vault))
        .with_state(state)
}

async fn healthz() -> impl IntoResponse {
    (StatusCode::OK, "ok")
}

async fn register(
    State(st): State<ApiState>,
    Json(req): Json<RegisterRequest>,
) -> Result<Response, ApiReject> {
    if !limits::is_valid_username(&req.username) {
        return Err(ApiReject::validation(
            "Username must be 3–20 chars of [a-zA-Z0-9_].",
        ));
    }
    if !limits::is_valid_password(&req.password) {
        return Err(ApiReject::validation("Password must be 8–128 chars."));
    }
    match st.db.register(&req.username, &req.password).await {
        Ok(row) => Ok((
            StatusCode::CREATED,
            Json(RegisterResponse {
                player: to_player(row),
            }),
        )
            .into_response()),
        Err(DbError::Conflict) => Err(ApiReject::new(
            StatusCode::CONFLICT,
            "conflict",
            format!("Username '{}' is already taken.", req.username),
        )),
        Err(e) => Err(ApiReject::internal(e)),
    }
}

async fn login(
    State(st): State<ApiState>,
    Json(req): Json<LoginRequest>,
) -> Result<Response, ApiReject> {
    if req.username.is_empty() || req.password.is_empty() {
        return Err(ApiReject::validation("Username and password are required."));
    }
    // Identical response for unknown-username and wrong-password (D17, M1.9).
    let row = match st.db.verify_login(&req.username, &req.password).await {
        Ok(Some(row)) => row,
        Ok(None) => return Err(ApiReject::unauthorized_login()),
        Err(e) => return Err(ApiReject::internal(e)),
    };
    let session_token = st.sessions.mint(row.player_id, st.session_ttl_secs as i64);
    let realtime_ticket = st.tickets.mint(row.player_id);
    Ok((
        StatusCode::OK,
        Json(LoginResponse {
            session_token,
            token_type: "Bearer".to_string(),
            expires_in: st.session_ttl_secs,
            realtime_ticket,
            player: to_player(row),
        }),
    )
        .into_response())
}

async fn players_me(State(st): State<ApiState>, headers: HeaderMap) -> Result<Response, ApiReject> {
    let player_id = authenticate(&st, &headers)?;
    match st.db.get_player(player_id).await {
        Ok(Some(row)) => Ok((StatusCode::OK, Json(to_player(row))).into_response()),
        Ok(None) => Err(ApiReject::unauthorized()),
        Err(e) => Err(ApiReject::internal(e)),
    }
}

async fn vault(State(st): State<ApiState>, headers: HeaderMap) -> Result<Response, ApiReject> {
    let player_id = authenticate(&st, &headers)?;
    match st.db.get_vault(player_id).await {
        Ok((chits, items)) => {
            let materials = items
                .into_iter()
                .map(|(item_kind, quantity)| VaultItemStack { item_kind, quantity })
                .collect();
            Ok((StatusCode::OK, Json(VaultSummary { chits, materials })).into_response())
        }
        Err(e) => Err(ApiReject::internal(e)),
    }
}

/// Resolve the Bearer session token to a player id, or 401.
fn authenticate(st: &ApiState, headers: &HeaderMap) -> Result<Uuid, ApiReject> {
    let auth = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .ok_or_else(ApiReject::unauthorized)?;
    let token = auth
        .strip_prefix("Bearer ")
        .ok_or_else(ApiReject::unauthorized)?;
    st.sessions
        .resolve(token)
        .ok_or_else(ApiReject::unauthorized)
}

/// Build the wire `Player` from a DB row. A fresh account has `squire` unlocked
/// and all three meld skills at level 1 (auth-players.md).
fn to_player(row: PlayerRow) -> Player {
    Player {
        player_id: row.player_id.to_string(),
        username: row.username,
        created_at: row.created_at.to_rfc3339(),
        active_title: row.active_title,
        class_unlocks: vec![CharacterClass::Squire],
        meld_skills: ["forging", "mercantile", "alchemy"]
            .iter()
            .map(|k| MeldSkillEntry {
                skill_kind: k.to_string(),
                level: 1,
                xp: 0,
            })
            .collect(),
    }
}

/// An HTTP rejection that renders the canonical error envelope (CANON.md §I).
pub struct ApiReject {
    status: StatusCode,
    code: &'static str,
    message: String,
}

impl ApiReject {
    fn new(status: StatusCode, code: &'static str, message: impl Into<String>) -> Self {
        Self {
            status,
            code,
            message: message.into(),
        }
    }
    fn validation(msg: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, "validation_error", msg)
    }
    fn unauthorized() -> Self {
        Self::new(
            StatusCode::UNAUTHORIZED,
            "unauthorized",
            "Missing or invalid session token.",
        )
    }
    /// The account-enumeration-safe login failure (identical for both causes).
    fn unauthorized_login() -> Self {
        Self::new(
            StatusCode::UNAUTHORIZED,
            "unauthorized",
            "Invalid username or password.",
        )
    }
    fn internal(err: impl std::fmt::Display) -> Self {
        // Log server-side; never leak details to the client.
        tracing::error!("internal error: {err}");
        Self::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal",
            "Internal server error.",
        )
    }
}

impl IntoResponse for ApiReject {
    fn into_response(self) -> Response {
        let body = ApiError {
            error: ApiErrorBody {
                code: self.code.to_string(),
                message: self.message,
                request_id: Uuid::now_v7().to_string(),
            },
        };
        (self.status, Json(body)).into_response()
    }
}

// The stores are Arc-wrapped internally; alias for the server's convenience.
pub type SharedTickets = Arc<Tickets>;
