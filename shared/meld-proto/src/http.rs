//! HTTP request/response DTOs (docs/interfaces/http-api.md, auth-players.md).
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
    /// Materials withdrawn from the Vault (storage chest), staged to seed the
    /// player's next run's Backpack.
    #[serde(default)]
    pub pending: Vec<VaultItemStack>,
}

/// A gear item (vault-gear.md subset — blue-chest, durability, per-slot stat).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GearView {
    pub gear_id: Id,
    pub name: String,
    pub slot: String,
    /// Which class this item is for (`meld_world::CLASS_KEYS`); empty means
    /// unrestricted (e.g. the starter weapon).
    #[serde(default)]
    pub class_key: String,
    pub insurance: String,
    /// Loot tier band at generation (`floor(d/100)`); 0 for the starter weapon.
    #[serde(default)]
    pub tier: i32,
    pub atk_bonus: i32,
    #[serde(default)]
    pub def_bonus: i32,
    #[serde(default)]
    pub spd_bonus: i32,
    pub base_max_durability: i32,
    pub max_durability: i32,
    /// Which of the owner's heroes has this equipped, if any.
    pub equipped_hero_slot: Option<i32>,
    /// GR-5 weapon family wire word (`sword`, `staff`, …); empty = unrestricted.
    #[serde(default)]
    pub family: String,
    /// GR-5 armor weight wire word (`heavy`, `robe`, …); empty = unrestricted.
    #[serde(default)]
    pub armor_weight: String,
    /// AD-1 rolled affixes — what makes this piece worth chasing.
    #[serde(default)]
    pub affixes: Vec<crate::affixes::Affix>,
    #[serde(default)]
    pub unique_key: String,
    #[serde(default)]
    pub set_key: String,
    /// Materials one affix reroll on this piece would eat. The cost climbs with the
    /// piece's tier, and the formula is the server's ([`meld_balance`] `[forge]`), so
    /// it rides the row rather than being re-derived by every client.
    #[serde(default)]
    pub reroll_cost: i32,
}

/// `GET /v1/vault/gear` response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GearListResponse {
    pub data: Vec<GearView>,
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

/// One ranked row on the Vanguard Board (`GET /v1/vanguard[/:season]`) —
/// the seasonal deepest-distance leaderboard (behaviors/endgame-seasons.md,
/// roadmap P1-1). Rank is 1-based and assigned by the server.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VanguardEntry {
    pub rank: i32,
    pub player_id: String,
    pub username: String,
    /// Deepest integer distance reached in a single run this season.
    pub max_distance: i32,
    /// Server time (unix millis) the record was first reached — the tie-break.
    pub achieved_at: i64,
}

/// `GET /v1/vanguard` / `GET /v1/vanguard/:season` response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VanguardBoardResponse {
    /// Which season this board covers (0-based, 13-week windows).
    pub season: i32,
    /// True once the season has closed — archived boards never change again.
    pub archived: bool,
    pub data: Vec<VanguardEntry>,
}

/// One row on the Hunt Board (`GET /v1/hunts`) — roadmap AD-4.
///
/// Progress and the reward are both the server's answer: the client draws the row it
/// is handed rather than re-deriving either, so a retuned `[hunt]` retunes the board.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HuntView {
    pub key: String,
    pub name: String,
    /// What the hunt wants, with its number in it ("Fell 8 Bloom Stalkers").
    pub objective: String,
    pub blurb: String,
    /// Biome band, shallow 0 … deep 4.
    pub tier: i32,
    pub progress: i32,
    pub target: i32,
    /// Earned, and the reward is still on the board.
    pub claimable: bool,
    pub claimed: bool,
    pub reward_chits: i64,
    /// Item kind of the stack paid alongside the chits; empty for chits alone.
    #[serde(default)]
    pub reward_material: String,
    #[serde(default)]
    pub reward_material_qty: i32,
    /// Whether finishing this one also hands over a rolled piece of gear.
    #[serde(default)]
    pub reward_gear: bool,
    /// Where to go to work it, derived server-side from the world's own placement
    /// tables. Empty when the objective already says it (a depth).
    #[serde(default)]
    pub where_to_look: String,
}

/// `GET /v1/hunts` response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HuntBoardResponse {
    pub data: Vec<HuntView>,
}

/// One bounty contract on the Quests panel (`GET /v1/bounties`) — roadmap AD-4.
///
/// Everything a player reads about a mark, resolved server-side: what it is called, where
/// it was sighted, how hard it is and what it pays. The client renders the row it is
/// handed and never re-derives a number.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BountyView {
    pub bounty_id: String,
    /// `active` · `completed` · `claimed` · `expired`.
    pub state: String,
    /// The mark's full name — "Ironmaw the Unburied".
    pub mark_name: String,
    /// FS-4 boss key, for the client's boss portrait.
    pub boss_kind: String,
    pub creature: String,
    pub biome: String,
    pub distance: i32,
    /// `overworld` or `dungeon`.
    pub venue: String,
    /// Where to go, in a sentence.
    pub where_to_look: String,
    /// How much harder than a standard creature at that depth the mark is.
    pub power: f64,
    /// Seconds until the contract is withdrawn; `0` once it is no longer standing.
    pub expires_in_secs: i64,
    pub reward_chits: i64,
    #[serde(default)]
    pub reward_material: String,
    #[serde(default)]
    pub reward_material_qty: i32,
    #[serde(default)]
    pub reward_gear: bool,
    pub reward_rank_xp: i64,
}

/// `GET /v1/bounties` response: the Den's standing offers, your history, and your rank.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BountyBoardResponse {
    /// Hunter rank — raised only by finished board work, never by levelling a party.
    pub rank: i32,
    pub rank_title: String,
    pub rank_xp: i64,
    /// XP still owed for the next rank.
    pub rank_xp_to_next: i64,
    /// Standing and finished-but-unpaid contracts, newest first.
    pub active: Vec<BountyView>,
    /// Everything that is over: paid, or withdrawn unfought.
    pub history: Vec<BountyView>,
}

/// `POST /v1/bounties/:id/claim` response — `200 OK`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BountyClaimResponse {
    pub bounty_id: String,
    pub mark_name: String,
    pub reward_chits: i64,
    #[serde(default)]
    pub reward_material: String,
    #[serde(default)]
    pub reward_material_qty: i32,
    #[serde(default)]
    pub reward_gear: String,
    /// The Vault's chit balance after the Den paid.
    pub chits: i64,
    /// Hunter rank after banking this contract's XP, and whether it just went up.
    pub rank: i32,
    pub rank_title: String,
    pub ranked_up: bool,
}

/// `POST /v1/hunts/:key/claim` response — `200 OK`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HuntClaimResponse {
    pub key: String,
    pub reward_chits: i64,
    #[serde(default)]
    pub reward_material: String,
    #[serde(default)]
    pub reward_material_qty: i32,
    /// Name of the piece the board handed over; empty when the hunt pays no gear.
    #[serde(default)]
    pub reward_gear: String,
    /// The Vault's chit balance after the board paid out.
    pub chits: i64,
}
