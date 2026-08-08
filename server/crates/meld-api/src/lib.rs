//! HTTP API (axum) — auth + player surface for the today-slice
//! (docs/interfaces/http-api.md, auth-players.md; CANON.md D17).
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
use meld_proto::materials as mat;
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
    /// Bounds-checks the `hero_slot` an equip request targets.
    pub party_size_per_player: i32,
    /// The tuned balance table, for the Forge's own maths (MS-1).
    pub balance: std::sync::Arc<meld_balance::Balance>,
    /// The Apothecary's shelf: item kind -> chit price. The map IS the stock list,
    /// so a client cannot buy something the vendor does not sell by naming it.
    /// Injected by the server from `[consumable]` balance.
    pub shop_prices: Vec<(String, i64)>,
}

pub fn router(state: ApiState) -> Router {
    Router::new()
        .route("/v1/healthz", get(healthz))
        .route("/v1/auth/register", post(register))
        .route("/v1/auth/login", post(login))
        .route("/v1/players/me", get(players_me))
        .route("/v1/vault", get(vault))
        .route("/v1/vault/materials/:item_kind/withdraw", post(withdraw_material))
        .route("/v1/vault/gear", get(vault_gear))
        .route("/v1/vault/gear/:gear_id/equip", post(equip))
        .route("/v1/vault/gear/:gear_id/unequip", post(unequip))
        .route("/v1/meld-skills", get(meld_skills))
        .route("/v1/heroes", get(heroes))
        .route("/v1/heroes/:slot", axum::routing::put(rename_hero))
        .route("/v1/party/loadouts", get(list_loadouts).post(save_loadout))
        .route("/v1/party/loadouts/:name", axum::routing::delete(delete_loadout))
        .route("/v1/party/loadouts/:name/apply", post(apply_loadout))
        .route("/v1/crafting/craft", post(craft))
        .route("/v1/crafting/recipes", get(recipes))
        .route("/v1/crafting/forge", post(forge))
        .route("/v1/vault/gear/:gear_id/reroll", post(reroll))
        .route("/v1/vault/gear/:gear_id/repair", post(repair))
        .route("/v1/vendors/apothecary", get(vendor_stock))
        .route("/v1/vendors/apothecary/buy", post(vendor_buy))
        .route("/v1/vendors/requisition", get(requisition_stock))
        .route("/v1/vendors/requisition/buy", post(requisition_buy))
        .route("/v1/vendors/broker", get(broker_prices))
        .route("/v1/vendors/broker/sell", post(broker_sell))
        .route("/v1/leaderboards/vanguard", get(vanguard_board))
        .route("/v1/leaderboards/vanguard/me", get(vanguard_me))
        .route("/v1/leaderboards/vanguard/:season", get(vanguard_season))
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

/// How many rows one Vanguard Board read returns. The spec's title grant covers
/// the top 100 instances, so 100 is the meaningful board depth; the paginated
/// envelope lands with AD-6's full board suite.
const VANGUARD_BOARD_LIMIT: i64 = 100;

/// `GET /v1/leaderboards/vanguard` — the live board for the open season
/// (http-api/leaderboards.md; roadmap P1-1's basic cut).
async fn vanguard_board(State(st): State<ApiState>) -> Result<Response, ApiReject> {
    // PUBLIC, unlike every other route here. The login screen shows the season's
    // board — that is the reason to log in — and a login screen is by definition
    // unauthenticated. It exposes exactly what a leaderboard is for: usernames and
    // how deep they got. `/vanguard/me` and the archived seasons stay authenticated.
    vanguard_body(&st, meld_db::current_season()).await
}

/// `GET /v1/leaderboards/vanguard/:season` — an earlier season's archived standings.
async fn vanguard_season(
    State(st): State<ApiState>,
    headers: HeaderMap,
    Path(season): Path<i32>,
) -> Result<Response, ApiReject> {
    authenticate(&st, &headers)?;
    if season < 0 || season > meld_db::current_season() {
        return Err(ApiReject::new(
            StatusCode::NOT_FOUND,
            "not_found",
            "No such season.",
        ));
    }
    vanguard_body(&st, season).await
}

/// `GET /v1/leaderboards/vanguard/me` — the caller's own placement this season.
/// A caller with no ranked run is a `200` with `entry: null` (the season exists;
/// the placement doesn't) — spec http-api/leaderboards.md.
async fn vanguard_me(State(st): State<ApiState>, headers: HeaderMap) -> Result<Response, ApiReject> {
    let player_id = authenticate(&st, &headers)?;
    let season = meld_db::current_season();
    // Ranked against the whole season, NOT by scanning the board's first page: a player
    // outside the top `VANGUARD_BOARD_LIMIT` is exactly who needs to be told where they
    // stand, and searching a limited page reports them as unranked.
    let entry = st
        .db
        .vanguard_placement(season, player_id)
        .await
        .map_err(ApiReject::internal)?
        .map(|(r, rank)| VanguardEntry {
            rank: rank as i32,
            player_id: r.player_id.to_string(),
            username: r.username,
            max_distance: r.max_distance,
            achieved_at: r.achieved_at.timestamp_millis(),
        });
    Ok((
        StatusCode::OK,
        Json(serde_json::json!({ "season": season, "entry": entry })),
    )
        .into_response())
}

async fn vanguard_entries(st: &ApiState, season: i32) -> Result<Vec<VanguardEntry>, ApiReject> {
    let rows = st
        .db
        .vanguard_board(season, VANGUARD_BOARD_LIMIT)
        .await
        .map_err(ApiReject::internal)?;
    Ok(rows
        .into_iter()
        .enumerate()
        .map(|(i, r)| VanguardEntry {
            rank: i as i32 + 1,
            player_id: r.player_id.to_string(),
            username: r.username,
            max_distance: r.max_distance,
            achieved_at: r.achieved_at.timestamp_millis(),
        })
        .collect())
}

async fn vanguard_body(st: &ApiState, season: i32) -> Result<Response, ApiReject> {
    let body = VanguardBoardResponse {
        season,
        archived: season < meld_db::current_season(),
        data: vanguard_entries(st, season).await?,
    };
    Ok((StatusCode::OK, Json(body)).into_response())
}

/// GET the caller's persistent hero names (by slot).
async fn heroes(State(st): State<ApiState>, headers: HeaderMap) -> Result<Response, ApiReject> {
    let player_id = authenticate(&st, &headers)?;
    // Classes ride alongside the names (GR-7) so the inventory UI can grey what a
    // hero may not wear using the same table the server enforces (GR-5), instead of
    // guessing from the party builder and disagreeing with the server.
    let (names, classes) = tokio::try_join!(
        st.db.get_hero_names(player_id),
        st.db.get_hero_classes(player_id),
    )
    .map_err(ApiReject::internal)?;
    Ok((
        StatusCode::OK,
        Json(serde_json::json!({ "names": names, "classes": classes })),
    )
        .into_response())
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

/// The most loadouts one account may keep. A cap so a client cannot turn the table
/// into unbounded per-account storage; well past what anyone builds by hand.
const MAX_LOADOUTS: usize = 12;

/// `GET /v1/party/loadouts` — the caller's saved party compositions (PT-2).
async fn list_loadouts(State(st): State<ApiState>, headers: HeaderMap) -> Result<Response, ApiReject> {
    let player_id = authenticate(&st, &headers)?;
    let rows = st.db.list_loadouts(player_id).await.map_err(ApiReject::internal)?;
    let data: Vec<_> = rows
        .into_iter()
        .map(|l| {
            serde_json::json!({
                "name": l.name,
                "classes": l.classes,
                "gear_count": l.gear.len(),
            })
        })
        .collect();
    Ok((StatusCode::OK, Json(serde_json::json!({ "data": data }))).into_response())
}

#[derive(serde::Deserialize)]
struct SaveLoadout {
    name: String,
    classes: Vec<String>,
}

/// `POST /v1/party/loadouts` — save (or overwrite) a named composition.
///
/// Validated against the account's OWN unlocks, not just the class registry: saving a
/// party you cannot field would store a loadout that silently changes the moment you
/// load it, since `clamp_party_to_unlocks` rewrites it at dive time. Better to refuse
/// than to keep a lie on disk.
async fn save_loadout(
    State(st): State<ApiState>,
    headers: HeaderMap,
    Json(req): Json<SaveLoadout>,
) -> Result<Response, ApiReject> {
    let player_id = authenticate(&st, &headers)?;
    let name: String = req.name.trim().chars().take(24).collect();
    if name.is_empty() {
        return Err(ApiReject::validation("A loadout needs a name."));
    }
    if req.classes.is_empty() {
        return Err(ApiReject::validation("A loadout needs at least one hero."));
    }
    let owned = st.db.get_unlocks(player_id).await.map_err(ApiReject::internal)?;
    let slots = meld_proto::unlocks::party_slots(&owned) as usize;
    if req.classes.len() > slots {
        return Err(ApiReject::validation(format!(
            "That is {} heroes; this account has earned {slots} party slot(s).",
            req.classes.len()
        )));
    }
    let fieldable = meld_proto::unlocks::owned_classes(&owned);
    for c in &req.classes {
        let Some(class) = meld_proto::equipment::class_from_key(c) else {
            return Err(ApiReject::validation(format!("No such class: {c}.")));
        };
        if !fieldable.contains(&class) {
            return Err(ApiReject::validation(format!("This account has not earned the {c}.")));
        }
    }
    let existing = st.db.list_loadouts(player_id).await.map_err(ApiReject::internal)?;
    // Overwriting an existing name is always allowed; only a NEW name can hit the cap.
    if existing.len() >= MAX_LOADOUTS && !existing.iter().any(|l| l.name == name) {
        return Err(ApiReject::validation(format!(
            "That is {MAX_LOADOUTS} loadouts already — delete one first."
        )));
    }
    // The gear snapshot is taken from the DB, NOT from the request. A client that
    // could name the gear in a loadout could name gear it does not own, or gear it
    // owns but cannot legally wear, and get it equipped on the next load. What is
    // currently equipped is a fact the server already holds.
    let gear: Vec<(i32, uuid::Uuid)> = st
        .db
        .get_gear(player_id)
        .await
        .map_err(ApiReject::internal)?
        .into_iter()
        .filter_map(|g| g.equipped_hero_slot.map(|slot| (slot, g.gear_id)))
        .filter(|(slot, _)| (*slot as usize) < req.classes.len())
        .collect();
    st.db
        .save_loadout(player_id, &name, &req.classes, &gear)
        .await
        .map_err(ApiReject::internal)?;
    Ok((
        StatusCode::OK,
        Json(serde_json::json!({ "name": name, "classes": req.classes, "gear_count": gear.len() })),
    )
        .into_response())
}

/// `POST /v1/party/loadouts/:name/apply` — set the party to a saved composition and
/// re-equip what it wore, as far as that is still possible.
///
/// The client sends a NAME and nothing else. Everything applied is read from the
/// server's own tables and re-validated here, because a loadout is a promise made in
/// the past: gear gets broken, sold, lost on death, and classes get re-clamped. Each
/// piece is checked again at load time and skipped if it no longer qualifies, so the
/// worst case is an unequipped slot rather than a wrong one.
async fn apply_loadout(
    State(st): State<ApiState>,
    headers: HeaderMap,
    Path(name): Path<String>,
) -> Result<Response, ApiReject> {
    let player_id = authenticate(&st, &headers)?;
    let all = st.db.list_loadouts(player_id).await.map_err(ApiReject::internal)?;
    let Some(l) = all.into_iter().find(|l| l.name == name) else {
        return Err(ApiReject::new(StatusCode::NOT_FOUND, "not_found", "No such loadout."));
    };
    let owned = st.db.get_unlocks(player_id).await.map_err(ApiReject::internal)?;
    let slots = meld_proto::unlocks::party_slots(&owned) as usize;
    let fieldable = meld_proto::unlocks::owned_classes(&owned);

    // Re-clamp the composition: it was legal when saved, and the account may have
    // changed since.
    let classes: Vec<String> = l
        .classes
        .iter()
        .take(slots)
        .map(|c| match meld_proto::equipment::class_from_key(c) {
            Some(k) if fieldable.contains(&k) => c.clone(),
            _ => "explorer".to_string(),
        })
        .collect();
    for (slot, key) in classes.iter().enumerate() {
        st.db
            .set_hero_class(player_id, slot as i16, key)
            .await
            .map_err(ApiReject::internal)?;
    }

    // Re-equip. `set_equipped` re-checks ownership, brokenness and class legality in
    // one place, so anything that no longer qualifies simply does not go on.
    let (mut restored, mut skipped) = (0, 0);
    for (slot, gear_id) in l.gear {
        if (slot as usize) >= classes.len() {
            skipped += 1;
            continue;
        }
        match st.db.set_equipped(player_id, gear_id, Some(slot)).await {
            Ok(meld_db::EquipResult::Ok) => restored += 1,
            Ok(_) => skipped += 1,
            Err(e) => return Err(ApiReject::internal(e)),
        }
    }
    Ok((
        StatusCode::OK,
        Json(serde_json::json!({
            "name": name,
            "classes": classes,
            "gear_restored": restored,
            "gear_missing": skipped,
        })),
    )
        .into_response())
}

/// `DELETE /v1/party/loadouts/:name` — forget one.
async fn delete_loadout(
    State(st): State<ApiState>,
    headers: HeaderMap,
    Path(name): Path<String>,
) -> Result<Response, ApiReject> {
    let player_id = authenticate(&st, &headers)?;
    st.db.delete_loadout(player_id, &name).await.map_err(ApiReject::internal)?;
    Ok((StatusCode::OK, Json(serde_json::json!({ "deleted": name }))).into_response())
}

#[derive(serde::Deserialize)]
struct CraftReq {
    /// Which recipe to run (`meld_proto::consumables::RECIPES`). Absent keeps the
    /// slice's single recipe, so an older client still crafts a Bloom Salve.
    #[serde(default)]
    recipe: Option<String>,
}

/// `POST /v1/crafting/craft` — run one recipe (MS-1). The recipe decides its own
/// inputs, output and which Meld skill the craft credits: a potion is Alchemy.
async fn craft(
    State(st): State<ApiState>,
    headers: HeaderMap,
    body: Option<Json<CraftReq>>,
) -> Result<Response, ApiReject> {
    let player_id = authenticate(&st, &headers)?;
    let key = body
        .and_then(|Json(b)| b.recipe)
        .unwrap_or_else(|| "bloom_salve".to_string());
    let Some(r) = meld_proto::consumables::recipe(&key) else {
        return Err(ApiReject::new(
            StatusCode::NOT_FOUND,
            "not_found",
            "No such recipe.",
        ));
    };
    // The recipe book opens with the crafter's own permanent level (crafting-meld.md:
    // a level gate is a 403, not a 409 — the materials are not the problem).
    let level = skill_level(&st, player_id, r.skill).await?;
    if level < r.min_level {
        return Err(ApiReject::new(
            StatusCode::FORBIDDEN,
            "forbidden",
            format!(
                "{} level {} is below the required level {} for recipe '{}'.",
                r.skill, level, r.min_level, r.key
            ),
        ));
    }
    let inputs: Vec<(String, i32)> = r
        .inputs
        .iter()
        .map(|(k, q)| ((*k).to_string(), *q))
        .collect();
    match st
        .db
        .craft(
            player_id,
            &inputs,
            (r.output, r.output_qty),
            r.skill,
            st.meld_forging_xp,
        )
        .await
    {
        Ok(true) => Ok((
            StatusCode::OK,
            Json(serde_json::json!({
                "crafted": r.output,
                "name": r.name,
                "quantity": r.output_qty,
                "skill": r.skill,
                "skill_level": level,
                // Itemised inputs, so a caller can say "2 dune iron became 1 ingot"
                // without holding the recipe table itself.
                "spent": inputs
                    .iter()
                    .map(|(kind, qty)| serde_json::json!({
                        "item_kind": kind,
                        "quantity": qty,
                    }))
                    .collect::<Vec<_>>(),
            })),
        )
            .into_response()),
        Ok(false) => {
            let needed = r
                .inputs
                .iter()
                .map(|(k, q)| format!("{q} {k}"))
                .collect::<Vec<_>>()
                .join(", ");
            Err(ApiReject::new(
                StatusCode::CONFLICT,
                "conflict",
                format!("Insufficient materials (need {needed})."),
            ))
        }
        Err(e) => Err(ApiReject::internal(e)),
    }
}

#[derive(serde::Deserialize)]
struct ForgeReq {
    slot: String,
    /// Which class's kit to forge for; defaults to the martial baseline.
    #[serde(default)]
    class_key: Option<String>,
    /// The piece's BODY: a **`refined`**-class material out of the Vault. Raw ore is
    /// volatile — a Smelter stabilises it first (the smelt recipes) — so the anvil
    /// takes stock, not what came out of the ground. A smith uses whichever stock they
    /// have, so the client names it rather than the server guessing.
    material: String,
    /// Optional **catalyst**: a `trophy` (combat drop) quenched into the piece,
    /// buying `catalyst_tier_bonus` tiers past the smith's own reach and the better
    /// affix pool. This is what a monster part is *for*.
    #[serde(default)]
    catalyst: Option<String>,
}

/// `POST /v1/crafting/forge` — forge one piece of gear (MS-1). Forging level sets
/// both the tier a smith can reach and how tightly the stats roll, so levelling the
/// skill is what makes the Forge worth visiting.
async fn forge(
    State(st): State<ApiState>,
    headers: HeaderMap,
    Json(req): Json<ForgeReq>,
) -> Result<Response, ApiReject> {
    let player_id = authenticate(&st, &headers)?;
    if !meld_proto::equipment::SLOT_CATEGORIES.contains(&req.slot.as_str()) {
        return Err(ApiReject::validation("Unknown equipment slot."));
    }
    let class_key = req.class_key.unwrap_or_else(|| "explorer".to_string());
    if meld_proto::equipment::class_from_key(&class_key).is_none() {
        return Err(ApiReject::validation("Unknown class."));
    }
    if !mat::is_class(&req.material, mat::MaterialClass::Refined) {
        // Name the smelt if they brought raw ore: "wrong class" is useless advice when
        // the fix is one craft away.
        let hint = mat::refined_form(&req.material)
            .map(|r| format!(" Smelt it into {r} first."))
            .unwrap_or_default();
        return Err(ApiReject::validation(format!(
            "The forge builds from refined stock, not raw material.{hint}"
        )));
    }
    if let Some(c) = &req.catalyst {
        if !mat::is_class(c, mat::MaterialClass::Trophy) {
            return Err(ApiReject::validation(
                "Only a trophy — a part cut from a creature — can catalyze a forge.",
            ));
        }
    }
    let catalyzed = req.catalyst.is_some();
    let level = forging_level(&st, player_id).await?;
    let drop = meld_world::forge_gear(
        &st.balance,
        level,
        &req.slot,
        &class_key,
        "forest",
        catalyzed,
        seed_now(),
    );
    let piece = crafted_row(&drop);
    let mut materials = vec![(req.material.clone(), st.balance.forge.gear_material_cost)];
    if let Some(c) = &req.catalyst {
        materials.push((c.clone(), st.balance.forge.catalyst_material_cost));
    }
    match st
        .db
        .forge_gear(player_id, &materials, st.balance.forge.gear_chit_cost, &piece)
        .await
    {
        Ok(true) => {
            let _ = st
                .db
                .add_skill_xp(player_id, "forging", st.balance.forge.forge_xp_per_craft)
                .await;
            Ok((
                StatusCode::OK,
                Json(serde_json::json!({
                    "forged": piece.name,
                    "gear_id": piece.gear_id,
                    "slot": piece.slot,
                    "class_key": piece.class_key,
                    "tier": piece.tier,
                    "rarity": drop.rarity,
                    "catalyzed": catalyzed,
                    "forging_level": level,
                    // What you actually MADE. Without these the caller is told a name
                    // and a tier and has to go re-read the Vault to learn whether the
                    // roll was any good — and a player is owed the numbers they just
                    // paid for.
                    "stats": {
                        "atk": piece.atk_bonus,
                        "def": piece.def_bonus,
                        "spd": piece.spd_bonus,
                    },
                    "max_durability": piece.max_durability,
                    "family": piece.family,
                    "armor_weight": piece.armor_weight,
                    "affixes": drop.affixes,
                    // …and what it cost, itemised, so "how much of what" never needs
                    // a second request.
                    "spent": {
                        "materials": materials
                            .iter()
                            .map(|(kind, qty)| serde_json::json!({
                                "item_kind": kind,
                                "quantity": qty,
                            }))
                            .collect::<Vec<_>>(),
                        "chits": st.balance.forge.gear_chit_cost,
                    },
                })),
            )
                .into_response())
        }
        Ok(false) => {
            let catalyst_cost = req
                .catalyst
                .as_ref()
                .map(|c| format!(", {} {c}", st.balance.forge.catalyst_material_cost))
                .unwrap_or_default();
            Err(ApiReject::new(
                StatusCode::CONFLICT,
                "conflict",
                format!(
                    "The forge needs {} {}{} and {} chits.",
                    st.balance.forge.gear_material_cost,
                    req.material,
                    catalyst_cost,
                    st.balance.forge.gear_chit_cost
                ),
            ))
        }
        Err(e) => Err(ApiReject::internal(e)),
    }
}

#[derive(serde::Deserialize)]
struct RerollReq {
    material: String,
}

/// `POST /v1/vault/gear/:gear_id/reroll` — buy another draw on a piece's affixes
/// (MS-1, and the last open thread of AD-1). The stats are untouched: what a smith
/// sells is a fresh roll, not a better item.
async fn reroll(
    State(st): State<ApiState>,
    headers: HeaderMap,
    Path(gear_id): Path<String>,
    Json(req): Json<RerollReq>,
) -> Result<Response, ApiReject> {
    let player_id = authenticate(&st, &headers)?;
    let gid = Uuid::parse_str(&gear_id)
        .map_err(|_| ApiReject::new(StatusCode::NOT_FOUND, "not_found", "Unknown gear."))?;
    let level = forging_level(&st, player_id).await?;
    if level < st.balance.forge.reroll_min_forging_level {
        return Err(ApiReject::new(
            StatusCode::CONFLICT,
            "conflict",
            format!(
                "Rerolling needs Forging level {}.",
                st.balance.forge.reroll_min_forging_level
            ),
        ));
    }
    let Some(row) = st
        .db
        .get_gear_by_id(player_id, gid)
        .await
        .map_err(ApiReject::internal)?
    else {
        return Err(ApiReject::new(
            StatusCode::NOT_FOUND,
            "not_found",
            "Gear not owned by caller.",
        ));
    };
    let class_key = if row.class_key.is_empty() {
        "explorer".to_string()
    } else {
        row.class_key.clone()
    };
    let rolled = meld_world::reroll_affixes(
        &st.balance,
        row.tier,
        &class_key,
        &row.slot,
        "forest",
        seed_now(),
    );
    let json = meld_proto::affixes::to_json(&rolled);
    let materials = [(req.material.clone(), st.balance.forge.reroll_material_cost)];
    match st
        .db
        .reroll_gear_affixes(player_id, gid, &materials, st.balance.forge.reroll_chit_cost, &json)
        .await
    {
        Ok(true) => {
            let _ = st
                .db
                .add_skill_xp(player_id, "forging", st.balance.forge.forge_xp_per_craft)
                .await;
            Ok((
                StatusCode::OK,
                Json(serde_json::json!({
                    "gear_id": gear_id,
                    "affixes": rolled,
                })),
            )
                .into_response())
        }
        Ok(false) => Err(ApiReject::new(
            StatusCode::CONFLICT,
            "conflict",
            format!(
                "A reroll needs {} {} and {} chits.",
                st.balance.forge.reroll_material_cost, req.material, st.balance.forge.reroll_chit_cost
            ),
        )),
        Err(e) => Err(ApiReject::internal(e)),
    }
}

/// `POST /v1/vault/gear/:gear_id/repair` — buy back max durability a death chewed
/// off (MS-1 / GR-2's repair sink). How much one repair restores scales with Forging
/// level, so a smith's own skill is the sink's efficiency.
async fn repair(
    State(st): State<ApiState>,
    headers: HeaderMap,
    Path(gear_id): Path<String>,
) -> Result<Response, ApiReject> {
    let player_id = authenticate(&st, &headers)?;
    let gid = Uuid::parse_str(&gear_id)
        .map_err(|_| ApiReject::new(StatusCode::NOT_FOUND, "not_found", "Unknown gear."))?;
    let level = forging_level(&st, player_id).await?;
    let points = st.balance.forge.repair_points(level);
    match st
        .db
        .repair_gear(player_id, gid, points, st.balance.forge.repair_chit_cost_per_point)
        .await
    {
        Ok(0) => Err(ApiReject::new(
            StatusCode::CONFLICT,
            "conflict",
            "Nothing to repair, or not enough chits.",
        )),
        Ok(restored) => {
            let _ = st
                .db
                .add_skill_xp(player_id, "forging", st.balance.forge.forge_xp_per_craft)
                .await;
            Ok((
                StatusCode::OK,
                Json(serde_json::json!({
                    "gear_id": gear_id,
                    "restored": restored,
                    "spent_chits": restored as i64 * st.balance.forge.repair_chit_cost_per_point,
                })),
            )
                .into_response())
        }
        Err(e) => Err(ApiReject::internal(e)),
    }
}

/// A forged drop as the gear table wants it.
fn crafted_row(d: &meld_world::GearDrop) -> meld_db::LootedGear {
    meld_db::LootedGear {
        insurance: d.insurance,
        gear_id: Uuid::now_v7(),
        name: d.name.clone(),
        slot: d.slot.clone(),
        class_key: d.class_key.clone(),
        tier: d.tier,
        atk_bonus: d.atk_bonus,
        def_bonus: d.def_bonus,
        spd_bonus: d.spd_bonus,
        base_max_durability: d.max_durability,
        max_durability: d.max_durability,
        damage_modifiers: "{}".to_string(),
        family: d.family.clone(),
        armor_weight: d.armor_weight.clone(),
        affixes: meld_proto::affixes::to_json(&d.affixes),
        unique_key: String::new(),
        set_key: String::new(),
    }
}

/// The caller's level in one Meld skill, derived from banked XP. Permanent
/// progression: this is the number every crafting gate reads.
async fn skill_level(st: &ApiState, player_id: Uuid, kind: &str) -> Result<i32, ApiReject> {
    let skills = st.db.get_skills(player_id).await.map_err(ApiReject::internal)?;
    Ok(skill_entries(skills, st.meld_xp_per_level)
        .into_iter()
        .find(|s| s.skill_kind == kind)
        .map(|s| s.level)
        .unwrap_or(1))
}

async fn forging_level(st: &ApiState, player_id: Uuid) -> Result<i32, ApiReject> {
    skill_level(st, player_id, "forging").await
}

/// A seed for a craft. Crafting is not replayed, so wall-clock entropy is fine here
/// — unlike world generation, which must stay reproducible from its instance seed.
fn seed_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0x5EED)
}

/// `GET /v1/crafting/recipes` — every recipe, so the Forge & Alembic UI can list
/// them instead of hard-coding a copy that drifts. Each row carries the level it
/// needs *and* the caller's level in that skill, so the UI can grey a locked row
/// without a second round trip.
async fn recipes(State(st): State<ApiState>, headers: HeaderMap) -> Result<Response, ApiReject> {
    let player_id = authenticate(&st, &headers)?;
    let skills = st.db.get_skills(player_id).await.map_err(ApiReject::internal)?;
    let levels = skill_entries(skills, st.meld_xp_per_level);
    let level_of = |kind: &str| -> i32 {
        levels
            .iter()
            .find(|s| s.skill_kind == kind)
            .map(|s| s.level)
            .unwrap_or(1)
    };
    let mut rows: Vec<&meld_proto::consumables::RecipeDef> =
        meld_proto::consumables::RECIPES.iter().collect();
    rows.sort_by_key(|r| (r.min_level, r.key));
    let data: Vec<serde_json::Value> = rows
        .iter()
        .map(|r| {
            let have = level_of(r.skill);
            serde_json::json!({
                "recipe": r.key,
                "name": r.name,
                "skill": r.skill,
                "required_level": r.min_level,
                "skill_level": have,
                "craftable": have >= r.min_level,
                "output": r.output,
                "output_quantity": r.output_qty,
                "inputs": r.inputs.iter().map(|(k, q)| serde_json::json!({
                    "item_kind": k,
                    "quantity": q,
                    "material_class": meld_proto::materials::material(k).map(|m| m.class.wire()),
                })).collect::<Vec<_>>(),
            })
        })
        .collect();
    Ok((StatusCode::OK, Json(serde_json::json!({ "data": data }))).into_response())
}

/// `GET /v1/vendors/broker` — the Broker's standing offer on every material,
/// priced at the caller's Mercantile level (a better haggler is quoted better).
/// The floor under the whole material economy: nothing you carry home is
/// unspendable, even if you never learn a craft.
async fn broker_prices(
    State(st): State<ApiState>,
    headers: HeaderMap,
) -> Result<Response, ApiReject> {
    let player_id = authenticate(&st, &headers)?;
    let level = skill_level(&st, player_id, "mercantile").await?;
    let data: Vec<serde_json::Value> = meld_proto::materials::MATERIALS
        .iter()
        .map(|m| {
            serde_json::json!({
                "item_kind": m.key,
                "name": m.name,
                "description": m.description,
                "material_class": m.class.wire(),
                "tier": m.tier,
                "price_chits": broker_price(&st, m, level),
            })
        })
        .collect();
    Ok((
        StatusCode::OK,
        Json(serde_json::json!({
            "vendor": "broker",
            "name": "The Broker",
            "mercantile_level": level,
            "data": data,
        })),
    )
        .into_response())
}

fn broker_price(st: &ApiState, m: &mat::MaterialDef, mercantile_level: i32) -> i64 {
    st.balance.material.sale_price(m.tier, m.class.wire(), mercantile_level)
}

/// The class a hero slot takes when the roster has not been told otherwise, and what
/// the Requisition stocks for an account that has not named anything yet.
const DEFAULT_CLASS_KEY: &str = "explorer";

/// `GET /v1/vendors/requisition` — Silas's off-the-books counter at the Foundry: the
/// plainest gear in the game, for chits (EC-2).
///
/// Lore-consistent by construction — the Foundry makes gear for the military and the
/// state, so an outsider buys through a Requisition Officer who filters stock to the
/// highest bidder ([`docs/lore/factions.md`]). Mechanically it is the floor that lets a
/// player who died with nothing walk back out equipped, which is why every piece is
/// tier 0, common, affix-free and deliberately worse than anything forged or found.
async fn requisition_stock(
    State(st): State<ApiState>,
    headers: HeaderMap,
) -> Result<Response, ApiReject> {
    let player_id = authenticate(&st, &headers)?;
    let classes = st.db.get_hero_classes(player_id).await.map_err(ApiReject::internal)?;
    // Stock what the caller's own roster can actually wear: a counter full of gear for
    // classes you do not field is a catalogue, not a shop. A fresh roster reports its
    // slots as empty (they take the class default), and an empty shop is useless to
    // exactly the player this counter exists for — so fall back to the starting class.
    let mut stocked: std::collections::BTreeSet<&str> =
        classes.iter().map(|c| c.as_str()).filter(|c| !c.is_empty()).collect();
    if stocked.is_empty() {
        stocked.insert(DEFAULT_CLASS_KEY);
    }
    let mut data: Vec<serde_json::Value> = Vec::new();
    for class_key in stocked {
        for slot in meld_proto::equipment::SLOT_CATEGORIES {
            let Some(price) = st.balance.requisition.price(slot) else {
                continue;
            };
            let piece = meld_world::shop_gear(&st.balance, slot, class_key);
            if piece.family.is_empty() && !meld_proto::equipment::is_armor_slot(slot)
                && slot != "accessory"
            {
                continue; // this class has no legal weapon for that hand
            }
            data.push(serde_json::json!({
                "slot": slot,
                "class_key": class_key,
                "name": piece.name,
                "price_chits": price,
                "tier": piece.tier,
                "rarity": piece.rarity,
                "insurance": piece.insurance,
                "family": piece.family,
                "armor_weight": piece.armor_weight,
                "stats": {"atk": piece.atk_bonus, "def": piece.def_bonus, "spd": piece.spd_bonus},
            }));
        }
    }
    Ok((
        StatusCode::OK,
        Json(serde_json::json!({
            "vendor": "requisition",
            "name": "The Requisition",
            "data": data,
        })),
    )
        .into_response())
}

#[derive(serde::Deserialize)]
struct RequisitionBuyReq {
    slot: String,
    #[serde(default)]
    class_key: Option<String>,
}

/// `POST /v1/vendors/requisition/buy` — chits for a plain piece of gear, atomically.
async fn requisition_buy(
    State(st): State<ApiState>,
    headers: HeaderMap,
    Json(req): Json<RequisitionBuyReq>,
) -> Result<Response, ApiReject> {
    let player_id = authenticate(&st, &headers)?;
    let Some(price) = st.balance.requisition.price(&req.slot) else {
        return Err(ApiReject::validation("The Requisition does not stock that slot."));
    };
    let class_key = req.class_key.unwrap_or_else(|| DEFAULT_CLASS_KEY.to_string());
    if meld_proto::equipment::class_from_key(&class_key).is_none() {
        return Err(ApiReject::validation("Unknown class."));
    }
    let drop = meld_world::shop_gear(&st.balance, &req.slot, &class_key);
    let piece = crafted_row(&drop);
    // No materials — a counter takes coin. `forge_gear` with an empty material list is
    // exactly "spend chits, insert one row", atomically.
    match st.db.forge_gear(player_id, &[], price, &piece).await {
        Ok(true) => Ok((
            StatusCode::OK,
            Json(serde_json::json!({
                "bought": piece.name,
                "gear_id": piece.gear_id,
                "slot": piece.slot,
                "class_key": piece.class_key,
                "insurance": drop.insurance,
                "stats": {
                    "atk": piece.atk_bonus,
                    "def": piece.def_bonus,
                    "spd": piece.spd_bonus,
                },
                "spent_chits": price,
            })),
        )
            .into_response()),
        Ok(false) => Err(ApiReject::new(
            StatusCode::CONFLICT,
            "conflict",
            format!("The Requisition wants {price} chits for that."),
        )),
        Err(e) => Err(ApiReject::internal(e)),
    }
}

#[derive(serde::Deserialize)]
struct SellReq {
    item_kind: String,
    #[serde(default = "one")]
    quantity: i32,
}

/// `POST /v1/vendors/broker/sell` — materials out, chits in, Mercantile XP earned.
/// Only *materials*: a potion or a Town Portal is refused, because a shop that buys
/// everything makes the Vault a pawn shop and the crafting economy pointless.
async fn broker_sell(
    State(st): State<ApiState>,
    headers: HeaderMap,
    Json(req): Json<SellReq>,
) -> Result<Response, ApiReject> {
    let player_id = authenticate(&st, &headers)?;
    if req.quantity <= 0 || req.quantity > 999 {
        return Err(ApiReject::validation("Quantity must be 1-999."));
    }
    let Some(def) = mat::material(&req.item_kind) else {
        return Err(ApiReject::new(
            StatusCode::BAD_REQUEST,
            "validation_error",
            "The Broker deals in crafting materials only.",
        ));
    };
    let level = skill_level(&st, player_id, "mercantile").await?;
    let unit = broker_price(&st, def, level);
    match st
        .db
        .sell_to_vendor(
            player_id,
            &req.item_kind,
            req.quantity,
            unit,
            "mercantile",
            st.balance.meld.mercantile_xp_per_sale,
        )
        .await
    {
        Ok(Some(paid)) => Ok((
            StatusCode::OK,
            Json(serde_json::json!({
                "sold": req.item_kind,
                "quantity": req.quantity,
                "unit_price": unit,
                "earned_chits": paid,
                "mercantile_level": level,
            })),
        )
            .into_response()),
        Ok(None) => Err(ApiReject::new(
            StatusCode::CONFLICT,
            "conflict",
            format!("The Vault does not hold {} {}.", req.quantity, req.item_kind),
        )),
        Err(e) => Err(ApiReject::internal(e)),
    }
}

/// The one NPC every new player needs: the Apothecary's shelf (EC-2). Lowest-tier
/// basics only — a heal, a Barrier, a Regen, and a way home — so a player who died
/// with nothing can walk back out equipped for chits alone.
fn apothecary_stock(st: &ApiState) -> Vec<serde_json::Value> {
    st.shop_prices
        .iter()
        .map(|(kind, price)| {
            let def = meld_proto::consumables::consumable(kind);
            serde_json::json!({
                "item_kind": kind,
                "name": def.map(|d| d.name).unwrap_or("Town Portal"),
                "description": def.map(|d| d.description).unwrap_or("Opens the way home."),
                "price_chits": price,
            })
        })
        .collect()
}

/// The shelf price of one unit, or `None` when the Apothecary does not stock it.
fn shelf_price(st: &ApiState, item_kind: &str) -> Option<i64> {
    st.shop_prices
        .iter()
        .find(|(k, _)| k == item_kind)
        .map(|(_, p)| *p)
}

/// `GET /v1/vendors/apothecary` — what the Apothecary has on the shelf today.
async fn vendor_stock(State(st): State<ApiState>, headers: HeaderMap) -> Result<Response, ApiReject> {
    authenticate(&st, &headers)?;
    Ok((
        StatusCode::OK,
        Json(serde_json::json!({
            "vendor": "apothecary",
            "name": "The Apothecary",
            "data": apothecary_stock(&st),
        })),
    )
        .into_response())
}

#[derive(serde::Deserialize)]
struct BuyReq {
    item_kind: String,
    #[serde(default = "one")]
    quantity: i32,
}

fn one() -> i32 {
    1
}

/// `POST /v1/vendors/apothecary/buy` — chits for goods, atomically.
async fn vendor_buy(
    State(st): State<ApiState>,
    headers: HeaderMap,
    Json(req): Json<BuyReq>,
) -> Result<Response, ApiReject> {
    let player_id = authenticate(&st, &headers)?;
    if req.quantity <= 0 || req.quantity > 99 {
        return Err(ApiReject::validation("Quantity must be 1-99."));
    }
    // Only what is actually on the shelf: the price table IS the stock list, so a
    // client cannot buy something the vendor does not sell by naming it.
    let Some(unit) = shelf_price(&st, &req.item_kind) else {
        return Err(ApiReject::new(
            StatusCode::NOT_FOUND,
            "not_found",
            "The Apothecary does not stock that.",
        ));
    };
    match st
        .db
        .buy_from_vendor(player_id, &req.item_kind, req.quantity, unit)
        .await
    {
        Ok(true) => Ok((
            StatusCode::OK,
            Json(serde_json::json!({
                "bought": req.item_kind,
                "quantity": req.quantity,
                "spent_chits": unit * req.quantity as i64,
            })),
        )
            .into_response()),
        Ok(false) => Err(ApiReject::new(
            StatusCode::CONFLICT,
            "conflict",
            "Not enough chits.",
        )),
        Err(e) => Err(ApiReject::internal(e)),
    }
}

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
    let (chits, items) = st.db.get_vault(player_id).await.map_err(ApiReject::internal)?;
    let pending = st
        .db
        .get_pending_backpack(player_id)
        .await
        .map_err(ApiReject::internal)?;
    let materials = items
        .into_iter()
        .map(|(item_kind, quantity)| VaultItemStack { item_kind, quantity })
        .collect();
    let pending = pending
        .into_iter()
        .map(|(item_kind, quantity)| VaultItemStack { item_kind, quantity })
        .collect();
    Ok((StatusCode::OK, Json(VaultSummary { chits, materials, pending })).into_response())
}

#[derive(serde::Deserialize)]
struct WithdrawRequest {
    quantity: i32,
}

/// Withdraw a material from the Vault (storage chest) into the caller's
/// pending-backpack queue, staged to seed their next run's Backpack.
async fn withdraw_material(
    State(st): State<ApiState>,
    headers: HeaderMap,
    Path(item_kind): Path<String>,
    Json(req): Json<WithdrawRequest>,
) -> Result<Response, ApiReject> {
    let player_id = authenticate(&st, &headers)?;
    if req.quantity <= 0 {
        return Err(ApiReject::new(StatusCode::BAD_REQUEST, "bad_request", "Invalid quantity."));
    }
    match st.db.withdraw_material(player_id, &item_kind, req.quantity).await {
        Ok(meld_db::WithdrawResult::Ok) => {
            Ok((StatusCode::OK, Json(serde_json::json!({ "withdrawn": req.quantity }))).into_response())
        }
        Ok(meld_db::WithdrawResult::InsufficientStock) => Err(ApiReject::new(
            StatusCode::CONFLICT,
            "conflict",
            "Not enough of that material in the Vault.",
        )),
        Err(e) => Err(ApiReject::internal(e)),
    }
}

async fn vault_gear(State(st): State<ApiState>, headers: HeaderMap) -> Result<Response, ApiReject> {
    let player_id = authenticate(&st, &headers)?;
    // Self-heals accounts created before the starter kit existed (or that
    // otherwise ended up with a gap) — idempotent, so this is cheap once caught up.
    if let Err(e) = st.db.ensure_starter_gear(player_id, st.party_size_per_player).await {
        tracing::warn!("ensure_starter_gear failed for {player_id}: {e}");
    }
    match st.db.get_gear(player_id).await {
        Ok(rows) => {
            let data = rows
                .into_iter()
                .map(|g| GearView {
                    gear_id: g.gear_id.to_string(),
                    name: g.name,
                    slot: g.slot,
                    class_key: g.class_key,
                    // Stored rows keep the chest-colour words; the wire speaks the
                    // player-facing tier so no client has to decode "red" (GR-6).
                    insurance: meld_proto::enums::Insurance::from_wire(&g.insurance)
                        .map(|i| i.wire().to_string())
                        .unwrap_or(g.insurance),
                    tier: g.tier,
                    atk_bonus: g.atk_bonus,
                    def_bonus: g.def_bonus,
                    spd_bonus: g.spd_bonus,
                    base_max_durability: g.base_max_durability,
                    max_durability: g.max_durability,
                    equipped_hero_slot: g.equipped_hero_slot,
                    family: g.family,
                    armor_weight: g.armor_weight,
                    affixes: meld_proto::affixes::from_json(&g.affixes),
                    unique_key: g.unique_key,
                    set_key: g.set_key,
                })
                .collect();
            Ok((StatusCode::OK, Json(GearListResponse { data })).into_response())
        }
        Err(e) => Err(ApiReject::internal(e)),
    }
}

#[derive(serde::Deserialize)]
struct EquipRequest {
    hero_slot: i32,
}

async fn equip(
    State(st): State<ApiState>,
    headers: HeaderMap,
    Path(gear_id): Path<String>,
    Json(req): Json<EquipRequest>,
) -> Result<Response, ApiReject> {
    if !(0..st.party_size_per_player).contains(&req.hero_slot) {
        return Err(ApiReject::new(StatusCode::BAD_REQUEST, "bad_request", "Invalid hero_slot."));
    }
    set_equipped(st, headers, gear_id, Some(req.hero_slot)).await
}

async fn unequip(
    State(st): State<ApiState>,
    headers: HeaderMap,
    Path(gear_id): Path<String>,
) -> Result<Response, ApiReject> {
    set_equipped(st, headers, gear_id, None).await
}

async fn set_equipped(
    st: ApiState,
    headers: HeaderMap,
    gear_id: String,
    target: Option<i32>,
) -> Result<Response, ApiReject> {
    let player_id = authenticate(&st, &headers)?;
    let gid = Uuid::parse_str(&gear_id)
        .map_err(|_| ApiReject::new(StatusCode::NOT_FOUND, "not_found", "Unknown gear."))?;
    match st.db.set_equipped(player_id, gid, target).await {
        Ok(EquipResult::Ok) => Ok((
            StatusCode::OK,
            Json(serde_json::json!({ "equipped_hero_slot": target })),
        )
            .into_response()),
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
        // GR-5: say WHICH rule refused the equip, so the UI can explain it.
        Ok(EquipResult::ClassLocked(rule)) => {
            use meld_proto::equipment::Legality;
            let msg = match rule {
                Legality::ClassFamily => "This hero's class cannot wield that kind of weapon.",
                Legality::ClassWeight => "This hero's class cannot wear armor that heavy.",
                Legality::ClassExclusive => "That piece belongs to another class.",
                Legality::SlotMismatch => "That item does not go in this slot.",
                Legality::Ok => "Cannot equip.",
            };
            Err(ApiReject::new(StatusCode::CONFLICT, "conflict", msg))
        }
        Ok(EquipResult::TwoHandedConflict) => Err(ApiReject::new(
            StatusCode::CONFLICT,
            "conflict",
            "A two-handed weapon needs both hands; unequip the off-hand first.",
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

/// Build the wire `Player` from a DB row + its meld skills. `explorer` is always
/// unlocked (auth-players.md).
fn to_player(row: PlayerRow, meld_skills: Vec<MeldSkillEntry>) -> Player {
    Player {
        player_id: row.player_id.to_string(),
        username: row.username,
        created_at: row.created_at.to_rfc3339(),
        active_title: row.active_title,
        class_unlocks: vec![CharacterClass::Explorer],
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
