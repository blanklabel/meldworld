//! HTTP API (axum) — auth + player surface for the today-slice
//! (interfaces/http-api.md, auth-players.md; CANON.md D17).
//!
//! Also owns the two short-lived credential stores the realtime gateway needs:
//! opaque Bearer **session tokens** (24 h) and single-use **realtime tickets**
//! (60 s). Both live here so the gateway (which depends on this crate) validates
//! against the same state.

pub mod tokens;

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use meld_db::{Db, DbError, EquipResult, PlayerRow};
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
    pub meld_xp_per_level: i64,
    pub meld_forging_xp: i64,
}

pub fn router(state: ApiState) -> Router {
    Router::new()
        .route("/v1/healthz", get(healthz))
        .route("/v1/auth/register", post(register))
        .route("/v1/auth/login", post(login))
        .route("/v1/players/me", get(players_me))
        .route("/v1/vault", get(vault))
        .route("/v1/vault/gear", get(vault_gear))
        .route("/v1/vault/gear/:gear_id/equip", post(equip))
        .route("/v1/vault/gear/:gear_id/unequip", post(unequip))
        .route("/v1/meld-skills", get(meld_skills))
        .route("/v1/heroes", get(heroes))
        .route("/v1/heroes/:slot", axum::routing::put(rename_hero))
        .route("/v1/crafting/craft", post(craft))
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
                player: to_player(row, default_skills()),
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
            player: to_player(row, default_skills()),
        }),
    )
        .into_response())
}

async fn players_me(State(st): State<ApiState>, headers: HeaderMap) -> Result<Response, ApiReject> {
    let player_id = authenticate(&st, &headers)?;
    // The two lookups are independent — run them concurrently (one RTT, not two).
    let (row_opt, skills) = tokio::try_join!(
        st.db.get_player(player_id),
        st.db.get_skills(player_id),
    )
    .map_err(ApiReject::internal)?;
    let row = match row_opt {
        Some(row) => row,
        None => return Err(ApiReject::unauthorized()),
    };
    let entries = skill_entries(skills, st.meld_xp_per_level);
    Ok((StatusCode::OK, Json(to_player(row, entries))).into_response())
}

async fn meld_skills(State(st): State<ApiState>, headers: HeaderMap) -> Result<Response, ApiReject> {
    let player_id = authenticate(&st, &headers)?;
    match st.db.get_skills(player_id).await {
        Ok(skills) => {
            let data = skill_entries(skills, st.meld_xp_per_level);
            Ok((StatusCode::OK, Json(serde_json::json!({ "data": data }))).into_response())
        }
        Err(e) => Err(ApiReject::internal(e)),
    }
}

/// GET the caller's persistent hero names (by slot).
async fn heroes(State(st): State<ApiState>, headers: HeaderMap) -> Result<Response, ApiReject> {
    let player_id = authenticate(&st, &headers)?;
    match st.db.get_hero_names(player_id).await {
        Ok(names) => Ok((StatusCode::OK, Json(serde_json::json!({ "names": names }))).into_response()),
        Err(e) => Err(ApiReject::internal(e)),
    }
}

#[derive(serde::Deserialize)]
struct RenameHero {
    name: String,
}

/// PUT a hero slot's name (persistent, per-account).
async fn rename_hero(
    State(st): State<ApiState>,
    headers: HeaderMap,
    Path(slot): Path<i16>,
    Json(req): Json<RenameHero>,
) -> Result<Response, ApiReject> {
    let player_id = authenticate(&st, &headers)?;
    if !(0..4).contains(&slot) {
        return Err(ApiReject::new(StatusCode::BAD_REQUEST, "bad_request", "Invalid hero slot."));
    }
    let name: String = req.name.trim().chars().take(24).collect();
    if name.is_empty() {
        return Err(ApiReject::new(StatusCode::BAD_REQUEST, "bad_request", "Name cannot be empty."));
    }
    match st.db.set_hero_name(player_id, slot, &name).await {
        Ok(()) => Ok((StatusCode::OK, Json(serde_json::json!({ "slot": slot, "name": name }))).into_response()),
        Err(e) => Err(ApiReject::internal(e)),
    }
}

async fn craft(State(st): State<ApiState>, headers: HeaderMap) -> Result<Response, ApiReject> {
    let player_id = authenticate(&st, &headers)?;
    // v0.1 recipe set: 1 forest_bloom_petal -> 1 bloom_salve (Forging).
    let inputs = [("forest_bloom_petal".to_string(), 1)];
    match st
        .db
        .craft(player_id, &inputs, ("bloom_salve", 1), st.meld_forging_xp)
        .await
    {
        Ok(true) => Ok((
            StatusCode::OK,
            Json(serde_json::json!({ "crafted": "bloom_salve", "quantity": 1 })),
        )
            .into_response()),
        Ok(false) => Err(ApiReject::new(
            StatusCode::CONFLICT,
            "conflict",
            "Insufficient materials (need 1 forest_bloom_petal).",
        )),
        Err(e) => Err(ApiReject::internal(e)),
    }
}

/// Derive a skill level from total xp: `1 + xp/xp_per_level`, capped at 99.
fn skill_entries(skills: Vec<(String, i64)>, xp_per_level: i64) -> Vec<MeldSkillEntry> {
    let per = xp_per_level.max(1);
    skills
        .into_iter()
        .map(|(skill_kind, xp)| MeldSkillEntry {
            level: (1 + xp / per).clamp(1, 99) as i32,
            xp,
            skill_kind,
        })
        .collect()
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

async fn vault_gear(State(st): State<ApiState>, headers: HeaderMap) -> Result<Response, ApiReject> {
    let player_id = authenticate(&st, &headers)?;
    match st.db.get_gear(player_id).await {
        Ok(rows) => {
            let data = rows
                .into_iter()
                .map(|g| GearView {
                    gear_id: g.gear_id.to_string(),
                    name: g.name,
                    slot: g.slot,
                    insurance: g.insurance,
                    tier: g.tier,
                    atk_bonus: g.atk_bonus,
                    base_max_durability: g.base_max_durability,
                    max_durability: g.max_durability,
                    equipped: g.equipped,
                })
                .collect();
            Ok((StatusCode::OK, Json(GearListResponse { data })).into_response())
        }
        Err(e) => Err(ApiReject::internal(e)),
    }
}

async fn equip(
    State(st): State<ApiState>,
    headers: HeaderMap,
    Path(gear_id): Path<String>,
) -> Result<Response, ApiReject> {
    set_equipped(st, headers, gear_id, true).await
}

async fn unequip(
    State(st): State<ApiState>,
    headers: HeaderMap,
    Path(gear_id): Path<String>,
) -> Result<Response, ApiReject> {
    set_equipped(st, headers, gear_id, false).await
}

async fn set_equipped(
    st: ApiState,
    headers: HeaderMap,
    gear_id: String,
    equipped: bool,
) -> Result<Response, ApiReject> {
    let player_id = authenticate(&st, &headers)?;
    let gid = Uuid::parse_str(&gear_id)
        .map_err(|_| ApiReject::new(StatusCode::NOT_FOUND, "not_found", "Unknown gear."))?;
    match st.db.set_equipped(player_id, gid, equipped).await {
        Ok(EquipResult::Ok) => {
            Ok((StatusCode::OK, Json(serde_json::json!({ "equipped": equipped }))).into_response())
        }
        Ok(EquipResult::NotFound) => Err(ApiReject::new(
            StatusCode::NOT_FOUND,
            "not_found",
            "Gear not owned by caller.",
        )),
        Ok(EquipResult::Broken) => Err(ApiReject::new(
            StatusCode::CONFLICT,
            "conflict",
            "Gear at 0 max durability cannot be equipped until repaired.",
        )),
        Ok(EquipResult::SlotOccupied) => Err(ApiReject::new(
            StatusCode::CONFLICT,
            "conflict",
            "Another item already occupies this slot; unequip it first.",
        )),
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

/// Build the wire `Player` from a DB row + its meld skills. `squire` is always
/// unlocked (auth-players.md).
fn to_player(row: PlayerRow, meld_skills: Vec<MeldSkillEntry>) -> Player {
    Player {
        player_id: row.player_id.to_string(),
        username: row.username,
        created_at: row.created_at.to_rfc3339(),
        active_title: row.active_title,
        class_unlocks: vec![CharacterClass::Squire],
        meld_skills,
    }
}

/// The three skills at level 1 / 0 xp — for a just-registered/just-logged-in
/// account (fresh) without a DB round-trip.
fn default_skills() -> Vec<MeldSkillEntry> {
    ["forging", "mercantile", "alchemy"]
        .iter()
        .map(|k| MeldSkillEntry {
            skill_kind: k.to_string(),
            level: 1,
            xp: 0,
        })
        .collect()
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
