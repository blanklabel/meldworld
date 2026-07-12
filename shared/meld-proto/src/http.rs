//! HTTP request/response DTOs (interfaces/http-api.md, auth-players.md).
//! Only the auth + player surface the today-slice needs is modelled.

use serde::{Deserialize, Serialize};

use crate::enums::CharacterClass;
use crate::Id;

/// Standard HTTP error envelope (CANON.md §I).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiError {
    pub error: ApiErrorBody,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiErrorBody {
    pub code: String,
    pub message: String,
    pub request_id: Id,
}

/// One meld-skill entry embedded in `Player` (crafting-meld.md shape).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MeldSkillEntry {
    pub skill_kind: String,
    pub level: i32,
    pub xp: i64,
}

/// The player account representation (auth-players.md Shared object: Player).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Player {
    pub player_id: Id,
    pub username: String,
    pub created_at: String,
    pub active_title: Option<String>,
    pub class_unlocks: Vec<CharacterClass>,
    pub meld_skills: Vec<MeldSkillEntry>,
}

/// `POST /v1/auth/register` request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegisterRequest {
    pub username: String,
    pub password: String,
}

/// `POST /v1/auth/register` response — `201 Created`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegisterResponse {
    pub player: Player,
}

/// `POST /v1/auth/login` request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoginRequest {
    pub username: String,
    pub password: String,
}

/// One banked item stack in the Vault.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VaultItemStack {
    pub item_kind: String,
    pub quantity: i32,
}

/// `GET /v1/vault` response — chits balance + banked item stacks (slice subset
/// of the full vault-gear surface).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VaultSummary {
    pub chits: i64,
    pub materials: Vec<VaultItemStack>,
}

/// `POST /v1/auth/login` response — `200 OK`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoginResponse {
    pub session_token: String,
    pub token_type: String,
    pub expires_in: i32,
    pub realtime_ticket: String,
    pub player: Player,
}
