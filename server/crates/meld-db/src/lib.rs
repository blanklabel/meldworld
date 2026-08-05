//! Persistence (CANON.md D18). The today-slice needs only accounts +
//! credentials; the Vault/gear/meld/economy schema lands with those systems.
//!
//! Passwords are stored **only** as bcrypt hashes (cost from balance, D17) — the
//! plaintext is never persisted or logged (BUILD-PLAN M1.8). Login returns an
//! indistinguishable result for unknown-username vs wrong-password (M1.9).
//!
//! Two interchangeable backends sit behind the one [`Db`] handle, chosen by the
//! connection string (all callers are backend-agnostic):
//!   - **Postgres** (`postgres://…`) — the real, persistent store.
//!   - **In-memory** (`memory:` / `memory://…`) — an ephemeral, dependency-free
//!     store for the self-contained QA/demo binary (no Postgres to install). It
//!     mirrors the Postgres semantics table-for-table but lives only in RAM, so
//!     everything resets on restart. See the `embedded-server` client build.

use bcrypt::{hash, verify};
use chrono::{DateTime, Utc};
use sqlx::postgres::{PgPool, PgPoolOptions};
use sqlx::Row;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use uuid::Uuid;

#[derive(Debug, thiserror::Error)]
pub enum DbError {
    #[error("database error: {0}")]
    Sqlx(#[from] sqlx::Error),
    #[error("username already taken")]
    Conflict,
    #[error("password hashing error: {0}")]
    Bcrypt(#[from] bcrypt::BcryptError),
}

/// A persisted player account row (no `password_hash` — never leaves the DB).
#[derive(Debug, Clone)]
pub struct PlayerRow {
    pub player_id: Uuid,
    pub username: String,
    pub created_at: DateTime<Utc>,
    pub active_title: Option<String>,
}

/// A dummy bcrypt hash used to equalize login timing when the username is
/// unknown, so we do the same work whether or not the account exists.
const DUMMY_HASH: &str = "$2b$12$C6UzMDM.H6dfI/f/IKcEeO7Y3l0Q1qk3s9m2p1o0n9m8l7k6j5i4a";

#[derive(Clone)]
pub struct Db {
    backend: Backend,
    bcrypt_cost: u32,
}

/// The concrete store behind a [`Db`]. Postgres for the real server; an
/// in-memory map for the self-contained QA binary (no external Postgres).
#[derive(Clone)]
enum Backend {
    Pg(PgPool),
    Mem(Arc<Mutex<Mem>>),
}

impl Db {
    /// Connect to a store. A `memory:`/`memory://…` URL selects the ephemeral
    /// in-memory backend (no Postgres needed — for the QA/demo binary); anything
    /// else is treated as a Postgres connection string.
    pub async fn connect(database_url: &str, bcrypt_cost: u32) -> Result<Self, DbError> {
        if database_url == "memory:"
            || database_url.starts_with("memory://")
            || database_url.starts_with("memory:")
        {
            return Ok(Db {
                backend: Backend::Mem(Arc::new(Mutex::new(Mem::default()))),
                bcrypt_cost,
            });
        }
        let pool = PgPoolOptions::new()
            // Sized above the expected concurrent-agent count (~20) so a connect
            // burst (everyone hitting vault/gear/me at once) doesn't queue behind
            // a small pool. Queries are short, so idle connections are cheap.
            .max_connections(32)
            .connect(database_url)
            .await?;
        Ok(Db {
            backend: Backend::Pg(pool),
            bcrypt_cost,
        })
    }

    /// Apply the (idempotent) schema. Safe to call on every boot. A no-op for the
    /// in-memory backend (its tables are just empty maps).
    pub async fn migrate(&self) -> Result<(), DbError> {
        let Backend::Pg(pool) = &self.backend else {
            return Ok(());
        };
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS players (
                player_id     UUID PRIMARY KEY,
                username      TEXT NOT NULL UNIQUE,
                password_hash TEXT NOT NULL,
                created_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
                active_title  TEXT
            );
            "#,
        )
        .execute(pool)
        .await?;
        // Tutorial gate (roadmap WG-2): true once the account has taken its first
        // dive. Added via idempotent ALTER so existing player rows pick it up.
        sqlx::query("ALTER TABLE players ADD COLUMN IF NOT EXISTS has_dived BOOLEAN NOT NULL DEFAULT false")
            .execute(pool)
            .await?;
        // The Vault: per-player persistent chits balance + banked item stacks.
        // (Gear/gems/durability land with the gear slice; materials/consumables
        // are stacked by kind here.) One statement per query() — sqlx uses
        // prepared statements, which reject multiple commands in one string.
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS vaults (
                player_id UUID PRIMARY KEY REFERENCES players(player_id),
                chits     BIGINT NOT NULL DEFAULT 0
            )
            "#,
        )
        .execute(pool)
        .await?;
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS vault_items (
                player_id UUID NOT NULL REFERENCES players(player_id),
                item_kind TEXT NOT NULL,
                quantity  INTEGER NOT NULL,
                PRIMARY KEY (player_id, item_kind)
            )
            "#,
        )
        .execute(pool)
        .await?;
        // Materials withdrawn from the Vault (storage chest), staged to seed the
        // player's *next* run's Backpack (`form_run` drains + clears this at dive
        // time). Same shape as `vault_items` — it's the mirror-image queue.
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS pending_backpack (
                player_id UUID NOT NULL REFERENCES players(player_id),
                item_kind TEXT NOT NULL,
                quantity  INTEGER NOT NULL,
                PRIMARY KEY (player_id, item_kind)
            )
            "#,
        )
        .execute(pool)
        .await?;
        // Gear with a durability sink (CANON.md D6). Both blue-chest (insured) and
        // extracted red-chest (run loot, gear-item-models.md) live here; `tier` is
        // the loot band at generation (`floor(d/100)`). Gems/sockets: later slice.
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS gear (
                gear_id              UUID PRIMARY KEY,
                owner_player_id      UUID NOT NULL REFERENCES players(player_id),
                name                 TEXT NOT NULL,
                slot                 TEXT NOT NULL,
                insurance            TEXT NOT NULL,
                tier                 INTEGER NOT NULL DEFAULT 0,
                atk_bonus            INTEGER NOT NULL DEFAULT 0,
                base_max_durability  INTEGER NOT NULL,
                max_durability       INTEGER NOT NULL,
                equipped             BOOLEAN NOT NULL DEFAULT FALSE
            )
            "#,
        )
        .execute(pool)
        .await?;
        // Forward-compat: add `tier` to any gear table created before this column
        // existed (CREATE TABLE IF NOT EXISTS won't alter an existing table).
        sqlx::query("ALTER TABLE gear ADD COLUMN IF NOT EXISTS tier INTEGER NOT NULL DEFAULT 0")
            .execute(pool)
            .await?;
        // Per-hero equip slots + the def/spd stats that came with them. Additive:
        // `equipped_hero_slot` (NULL = unequipped, else which of the player's
        // heroes is wearing it) supersedes the old `equipped` boolean, which stays
        // in the table (unused by new code) rather than being dropped.
        sqlx::query("ALTER TABLE gear ADD COLUMN IF NOT EXISTS def_bonus INTEGER NOT NULL DEFAULT 0")
            .execute(pool)
            .await?;
        sqlx::query("ALTER TABLE gear ADD COLUMN IF NOT EXISTS spd_bonus INTEGER NOT NULL DEFAULT 0")
            .execute(pool)
            .await?;
        sqlx::query("ALTER TABLE gear ADD COLUMN IF NOT EXISTS equipped_hero_slot INTEGER")
            .execute(pool)
            .await?;
        // Class-specific gear: which class this item is for (`meld_world::
        // CLASS_KEYS`), empty = unrestricted (the starter weapon). Only that
        // class's heroes gain the equipped bonus — enforced in
        // `equipped_gear_bonuses` below, not by rejecting the equip itself
        // (a hero's class for the *next* dive isn't known at equip time).
        sqlx::query("ALTER TABLE gear ADD COLUMN IF NOT EXISTS class_key TEXT NOT NULL DEFAULT ''")
            .execute(pool)
            .await?;
        // Elemental profile a piece grants its wearer (Epic GR spec §5): a JSON
        // object of DamageType wire key → multiplier (e.g. {"FIRE":0.75}).
        sqlx::query(
            "ALTER TABLE gear ADD COLUMN IF NOT EXISTS damage_modifiers TEXT NOT NULL DEFAULT '{}'",
        )
        .execute(pool)
        .await?;
        // 7-slot loadout migration (Epic GR spec §5): the old 3-category model
        // maps onto the new six categories — weapon→main_hand, armor→chest;
        // accessory keeps its name (two equip slots share the one category).
        // Idempotent: once renamed, the WHERE matches nothing.
        sqlx::query("UPDATE gear SET slot = 'main_hand' WHERE slot = 'weapon'")
            .execute(pool)
            .await?;
        sqlx::query("UPDATE gear SET slot = 'chest' WHERE slot = 'armor'")
            .execute(pool)
            .await?;
        // Every hot gear query filters by `owner_player_id` (get_gear,
        // equipped_gear_bonuses on connect, death durability, equip checks), but a FK
        // is NOT auto-indexed in Postgres — so each was a full-table Seq Scan, and
        // `gear` is insert-only (never pruned), so it degraded linearly forever.
        sqlx::query("CREATE INDEX IF NOT EXISTS idx_gear_owner ON gear(owner_player_id)")
            .execute(pool)
            .await?;
        // Persistent Meld skills (forging / mercantile / alchemy). Level is a
        // pure function of xp (derived on read); we persist total xp only.
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS meld_skills (
                player_id  UUID NOT NULL REFERENCES players(player_id),
                skill_kind TEXT NOT NULL,
                xp         BIGINT NOT NULL DEFAULT 0,
                PRIMARY KEY (player_id, skill_kind)
            )
            "#,
        )
        .execute(pool)
        .await?;
        // Persistent per-account hero names, one row per party slot. The class is
        // still chosen per dive in the party builder; only the name persists.
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS heroes (
                player_id UUID NOT NULL REFERENCES players(player_id),
                slot      SMALLINT NOT NULL,
                name      TEXT NOT NULL,
                back_row  BOOLEAN NOT NULL DEFAULT false,
                PRIMARY KEY (player_id, slot)
            )
            "#,
        )
        .execute(pool)
        .await?;
        // Additive migration: `back_row` was added after the table shipped, and
        // CREATE TABLE IF NOT EXISTS won't alter an existing table.
        sqlx::query("ALTER TABLE heroes ADD COLUMN IF NOT EXISTS back_row BOOLEAN NOT NULL DEFAULT false")
            .execute(pool)
            .await?;
        // The Vanguard Board (roadmap P1-1, behaviors/endgame-seasons.md): the
        // per-season deepest-distance leaderboard. One row per (season, player):
        // that player's deepest run in the season. `achieved_at` is the tie-break.
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS vanguard (
                season       INTEGER NOT NULL,
                player_id    UUID NOT NULL REFERENCES players(player_id),
                max_distance INTEGER NOT NULL,
                achieved_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
                PRIMARY KEY (season, player_id)
            )
            "#,
        )
        .execute(pool)
        .await?;
        // Board reads are `ORDER BY max_distance DESC, achieved_at ASC` within one
        // season — index that exact shape so the live board stays a cheap query.
        sqlx::query(
            "CREATE INDEX IF NOT EXISTS idx_vanguard_rank ON vanguard(season, max_distance DESC, achieved_at ASC)",
        )
        .execute(pool)
        .await?;
        Ok(())
    }

    /// Post a run's deepest distance to the Vanguard Board for `season`.
    ///
    /// Monotonic: the stored record only ever grows, and `achieved_at` is stamped
    /// only when the record improves, so the earliest-to-the-frontier tie-break
    /// holds (spec "Ranking rules"). Returns `true` on a new personal best.
    pub async fn record_vanguard_distance(
        &self,
        player_id: Uuid,
        season: i32,
        distance: i32,
    ) -> Result<bool, DbError> {
        if distance <= 0 {
            return Ok(false);
        }
        match &self.backend {
            Backend::Pg(pool) => {
                // The `WHERE` makes a shallower post a true no-op: neither the
                // distance nor the timestamp moves.
                let res = sqlx::query(
                    "INSERT INTO vanguard (season, player_id, max_distance) VALUES ($1, $2, $3)
                     ON CONFLICT (season, player_id) DO UPDATE
                       SET max_distance = $3, achieved_at = now()
                       WHERE vanguard.max_distance < $3",
                )
                .bind(season)
                .bind(player_id)
                .bind(distance)
                .execute(pool)
                .await?;
                Ok(res.rows_affected() > 0)
            }
            Backend::Mem(m) => {
                let mut m = m.lock().unwrap();
                let e = m.vanguard.entry((season, player_id)).or_insert((0, Utc::now()));
                if e.0 < distance {
                    *e = (distance, Utc::now());
                    Ok(true)
                } else {
                    Ok(false)
                }
            }
        }
    }

    /// The Vanguard Board for one season, best-first, capped at `limit` rows.
    ///
    /// Ranking: `max_distance` DESC, then earliest `achieved_at` (first to the
    /// frontier wins the tie), then `player_id` as the final deterministic key.
    pub async fn vanguard_board(
        &self,
        season: i32,
        limit: i64,
    ) -> Result<Vec<VanguardRow>, DbError> {
        match &self.backend {
            Backend::Pg(pool) => {
                let rows = sqlx::query(
                    "SELECT v.player_id, p.username, v.max_distance, v.achieved_at
                       FROM vanguard v JOIN players p USING (player_id)
                      WHERE v.season = $1
                      ORDER BY v.max_distance DESC, v.achieved_at ASC, v.player_id ASC
                      LIMIT $2",
                )
                .bind(season)
                .bind(limit)
                .fetch_all(pool)
                .await?;
                Ok(rows
                    .iter()
                    .map(|r| VanguardRow {
                        player_id: r.get("player_id"),
                        username: r.get("username"),
                        max_distance: r.get("max_distance"),
                        achieved_at: r.get::<DateTime<Utc>, _>("achieved_at"),
                    })
                    .collect())
            }
            Backend::Mem(m) => {
                let m = m.lock().unwrap();
                let mut rows: Vec<VanguardRow> = m
                    .vanguard
                    .iter()
                    .filter(|((s, _), _)| *s == season)
                    .filter_map(|((_, pid), (dist, at))| {
                        m.players.get(pid).map(|p| VanguardRow {
                            player_id: *pid,
                            username: p.username.clone(),
                            max_distance: *dist,
                            achieved_at: *at,
                        })
                    })
                    .collect();
                rows.sort_by(|a, b| {
                    b.max_distance
                        .cmp(&a.max_distance)
                        .then(a.achieved_at.cmp(&b.achieved_at))
                        .then(a.player_id.cmp(&b.player_id))
                });
                rows.truncate(limit.max(0) as usize);
                Ok(rows)
            }
        }
    }

    /// The player's hero names by slot (0-based), ordered. Empty if never set.
    pub async fn get_hero_names(&self, player_id: Uuid) -> Result<Vec<String>, DbError> {
        match &self.backend {
            Backend::Pg(pool) => {
                let rows = sqlx::query("SELECT name FROM heroes WHERE player_id = $1 ORDER BY slot")
                    .bind(player_id)
                    .fetch_all(pool)
                    .await?;
                Ok(rows.iter().map(|r| r.get::<String, _>("name")).collect())
            }
            Backend::Mem(m) => {
                let m = m.lock().unwrap();
                let mut rows: Vec<(i16, String)> = m
                    .heroes
                    .iter()
                    .filter(|((p, _), _)| *p == player_id)
                    .map(|((_, slot), name)| (*slot, name.clone()))
                    .collect();
                rows.sort_by_key(|(slot, _)| *slot);
                Ok(rows.into_iter().map(|(_, name)| name).collect())
            }
        }
    }

    /// Rename a hero slot (upsert). Names are trimmed/capped by the caller.
    pub async fn set_hero_name(&self, player_id: Uuid, slot: i16, name: &str) -> Result<(), DbError> {
        match &self.backend {
            Backend::Pg(pool) => {
                sqlx::query(
                    "INSERT INTO heroes (player_id, slot, name) VALUES ($1, $2, $3)
                     ON CONFLICT (player_id, slot) DO UPDATE SET name = $3",
                )
                .bind(player_id)
                .bind(slot)
                .bind(name)
                .execute(pool)
                .await?;
            }
            Backend::Mem(m) => {
                m.lock()
                    .unwrap()
                    .heroes
                    .insert((player_id, slot), name.to_string());
            }
        }
        Ok(())
    }

    /// The player's hero formation flags by slot (0-based), ordered — `true` = back
    /// row. Aligned with [`Self::get_hero_names`]; unset slots default to `false`.
    pub async fn get_hero_rows(&self, player_id: Uuid) -> Result<Vec<bool>, DbError> {
        match &self.backend {
            Backend::Pg(pool) => {
                let rows =
                    sqlx::query("SELECT back_row FROM heroes WHERE player_id = $1 ORDER BY slot")
                        .bind(player_id)
                        .fetch_all(pool)
                        .await?;
                Ok(rows.iter().map(|r| r.get::<bool, _>("back_row")).collect())
            }
            Backend::Mem(m) => {
                let m = m.lock().unwrap();
                // Same slots as the names (seeded 0..N), each with its back_row flag.
                let mut slots: Vec<i16> = m
                    .heroes
                    .keys()
                    .filter(|(p, _)| *p == player_id)
                    .map(|(_, slot)| *slot)
                    .collect();
                slots.sort_unstable();
                Ok(slots
                    .into_iter()
                    .map(|slot| m.hero_rows.get(&(player_id, slot)).copied().unwrap_or(false))
                    .collect())
            }
        }
    }

    /// Set a hero slot's formation rank (`true` = back row). Upsert; the row already
    /// exists from account seeding, so the INSERT branch is just a safety net.
    pub async fn set_hero_row(&self, player_id: Uuid, slot: i16, back_row: bool) -> Result<(), DbError> {
        match &self.backend {
            Backend::Pg(pool) => {
                sqlx::query(
                    "INSERT INTO heroes (player_id, slot, name, back_row) VALUES ($1, $2, 'Hero', $3)
                     ON CONFLICT (player_id, slot) DO UPDATE SET back_row = $3",
                )
                .bind(player_id)
                .bind(slot)
                .bind(back_row)
                .execute(pool)
                .await?;
            }
            Backend::Mem(m) => {
                m.lock().unwrap().hero_rows.insert((player_id, slot), back_row);
            }
        }
        Ok(())
    }

    /// Has this account ever dived? Drives the WG-2 tutorial gate (false = the next
    /// dive is the deterministic Forest onboarding world).
    pub async fn get_has_dived(&self, player_id: Uuid) -> Result<bool, DbError> {
        match &self.backend {
            Backend::Pg(pool) => {
                let row = sqlx::query("SELECT has_dived FROM players WHERE player_id = $1")
                    .bind(player_id)
                    .fetch_optional(pool)
                    .await?;
                Ok(row.map(|r| r.get::<bool, _>("has_dived")).unwrap_or(false))
            }
            Backend::Mem(m) => Ok(m
                .lock()
                .unwrap()
                .players
                .get(&player_id)
                .map(|p| p.has_dived)
                .unwrap_or(false)),
        }
    }

    /// Mark that this account has dived (ends its tutorial state). Idempotent.
    pub async fn set_has_dived(&self, player_id: Uuid) -> Result<(), DbError> {
        match &self.backend {
            Backend::Pg(pool) => {
                sqlx::query("UPDATE players SET has_dived = true WHERE player_id = $1")
                    .bind(player_id)
                    .execute(pool)
                    .await?;
            }
            Backend::Mem(m) => {
                if let Some(p) = m.lock().unwrap().players.get_mut(&player_id) {
                    p.has_dived = true;
                }
            }
        }
        Ok(())
    }

    /// Bank a run's backpack into the player's Vault atomically (extraction).
    /// Upserts each item stack and adds `chits`; creates the vault row if absent.
    pub async fn bank_extraction(
        &self,
        player_id: Uuid,
        items: &[(String, i32)],
        chits: i64,
    ) -> Result<(), DbError> {
        match &self.backend {
            Backend::Pg(pool) => {
                let mut tx = pool.begin().await?;
                sqlx::query(
                    "INSERT INTO vaults (player_id, chits) VALUES ($1, $2)
                     ON CONFLICT (player_id) DO UPDATE SET chits = vaults.chits + $2",
                )
                .bind(player_id)
                .bind(chits)
                .execute(&mut *tx)
                .await?;
                for (kind, qty) in items {
                    sqlx::query(
                        "INSERT INTO vault_items (player_id, item_kind, quantity) VALUES ($1, $2, $3)
                         ON CONFLICT (player_id, item_kind)
                         DO UPDATE SET quantity = vault_items.quantity + $3",
                    )
                    .bind(player_id)
                    .bind(kind)
                    .bind(qty)
                    .execute(&mut *tx)
                    .await?;
                }
                tx.commit().await?;
            }
            Backend::Mem(m) => {
                let mut m = m.lock().unwrap();
                *m.chits.entry(player_id).or_insert(0) += chits;
                for (kind, qty) in items {
                    *m.vault_items.entry((player_id, kind.clone())).or_insert(0) += *qty;
                }
            }
        }
        Ok(())
    }

    /// Read a player's Vault: chits balance + item stacks (kind, quantity).
    pub async fn get_vault(&self, player_id: Uuid) -> Result<(i64, Vec<(String, i32)>), DbError> {
        match &self.backend {
            Backend::Pg(pool) => {
                let chits: i64 = sqlx::query_scalar("SELECT chits FROM vaults WHERE player_id = $1")
                    .bind(player_id)
                    .fetch_optional(pool)
                    .await?
                    .unwrap_or(0);
                let rows = sqlx::query(
                    "SELECT item_kind, quantity FROM vault_items WHERE player_id = $1 ORDER BY item_kind",
                )
                .bind(player_id)
                .fetch_all(pool)
                .await?;
                let items = rows
                    .iter()
                    .map(|r| (r.get::<String, _>("item_kind"), r.get::<i32, _>("quantity")))
                    .collect();
                Ok((chits, items))
            }
            Backend::Mem(m) => {
                let m = m.lock().unwrap();
                let chits = m.chits.get(&player_id).copied().unwrap_or(0);
                let mut items: Vec<(String, i32)> = m
                    .vault_items
                    .iter()
                    .filter(|((p, _), _)| *p == player_id)
                    .map(|((_, kind), qty)| (kind.clone(), *qty))
                    .collect();
                items.sort_by(|a, b| a.0.cmp(&b.0));
                Ok((chits, items))
            }
        }
    }

    /// Create an account (+ empty Vault + a starting blue-chest weapon). Hashes
    /// the password with bcrypt; the plaintext is dropped here and never stored.
    /// `Conflict` on dup username. All rows commit together.
    pub async fn register(&self, username: &str, password: &str) -> Result<PlayerRow, DbError> {
        // bcrypt is ~hundreds of ms of pure CPU — run it on the blocking pool so it
        // never pins an async worker thread (a login burst would otherwise stall the
        // HTTP + WS handling that shares those threads).
        let password_hash = {
            let (pw, cost) = (password.to_string(), self.bcrypt_cost);
            tokio::task::spawn_blocking(move || hash(pw, cost))
                .await
                .expect("bcrypt hash task panicked")?
        };
        let player_id = Uuid::now_v7();
        let player_row: Result<PlayerRow, DbError> = match &self.backend {
            Backend::Pg(pool) => {
                let mut tx = pool.begin().await?;
                let row = sqlx::query(
                    r#"
                    INSERT INTO players (player_id, username, password_hash)
                    VALUES ($1, $2, $3)
                    RETURNING player_id, username, created_at, active_title
                    "#,
                )
                .bind(player_id)
                .bind(username)
                .bind(&password_hash)
                .fetch_one(&mut *tx)
                .await
                .map_err(|e| match &e {
                    sqlx::Error::Database(db) if db.code().as_deref() == Some("23505") => {
                        DbError::Conflict
                    }
                    _ => DbError::Sqlx(e),
                })?;
                sqlx::query("INSERT INTO vaults (player_id, chits) VALUES ($1, 0)")
                    .bind(player_id)
                    .execute(&mut *tx)
                    .await?;
                // A humble starting weapon (blue-chest, equipped to hero 0, tier 0).
                sqlx::query(
                    "INSERT INTO gear (gear_id, owner_player_id, name, slot, insurance, tier, atk_bonus, base_max_durability, max_durability, equipped_hero_slot)
                     VALUES ($1, $2, 'Chipped Blade', 'main_hand', 'blue', 0, 3, 100, 100, 0)",
                )
                .bind(Uuid::now_v7())
                .bind(player_id)
                .execute(&mut *tx)
                .await?;
                // Seed the three Meld skills at 0 xp.
                sqlx::query(
                    "INSERT INTO meld_skills (player_id, skill_kind, xp) VALUES ($1,'forging',0),($1,'mercantile',0),($1,'alchemy',0)",
                )
                .bind(player_id)
                .execute(&mut *tx)
                .await?;
                // Seed default hero names (renameable in the party builder).
                sqlx::query(
                    "INSERT INTO heroes (player_id, slot, name) VALUES ($1,0,'Hero 1'),($1,1,'Hero 2'),($1,2,'Hero 3'),($1,3,'Hero 4')",
                )
                .bind(player_id)
                .execute(&mut *tx)
                .await?;
                tx.commit().await?;
                Ok(row_to_player(&row))
            }
            Backend::Mem(m) => {
                let mut m = m.lock().unwrap();
                if m.players.values().any(|p| p.username == username) {
                    return Err(DbError::Conflict);
                }
                let created_at = DateTime::<Utc>::from_timestamp(0, 0).unwrap();
                m.players.insert(
                    player_id,
                    MemPlayer {
                        player_id,
                        username: username.to_string(),
                        password_hash,
                        created_at,
                        active_title: None,
                        has_dived: false,
                    },
                );
                m.chits.insert(player_id, 0);
                // A humble starting weapon (blue-chest, equipped to hero 0, tier 0).
                let gear_id = Uuid::now_v7();
                m.gear.insert(
                    gear_id,
                    MemGear {
                        gear_id,
                        owner_player_id: player_id,
                        name: "Chipped Blade".into(),
                        slot: "main_hand".into(),
                        class_key: String::new(),
                        insurance: "blue".into(),
                        tier: 0,
                        atk_bonus: 3,
                        def_bonus: 0,
                        spd_bonus: 0,
                        base_max_durability: 100,
                        max_durability: 100,
                        equipped_hero_slot: Some(0),
                        damage_modifiers: "{}".into(),
                    },
                );
                for kind in ["forging", "mercantile", "alchemy"] {
                    m.skills.insert((player_id, kind.to_string()), 0);
                }
                for (slot, name) in [(0, "Hero 1"), (1, "Hero 2"), (2, "Hero 3"), (3, "Hero 4")] {
                    m.heroes.insert((player_id, slot), name.to_string());
                }
                Ok(PlayerRow {
                    player_id,
                    username: username.to_string(),
                    created_at,
                    active_title: None,
                })
            }
        };
        let player_row = player_row?;
        // Backfill the rest of the starter kit (every hero slot × category not
        // already covered by the weapon just seeded above) — same permanent,
        // class-unrestricted +1 gear a pre-existing account gets caught up on
        // via `ensure_starter_gear`'s other call sites.
        self.ensure_starter_gear(player_id, 4).await?;
        Ok(player_row)
    }

    /// Credit Meld-skill XP (upsert; caps handled by the level curve on read).
    pub async fn add_skill_xp(&self, player_id: Uuid, kind: &str, xp: i64) -> Result<(), DbError> {
        match &self.backend {
            Backend::Pg(pool) => {
                sqlx::query(
                    "INSERT INTO meld_skills (player_id, skill_kind, xp) VALUES ($1, $2, $3)
                     ON CONFLICT (player_id, skill_kind) DO UPDATE SET xp = meld_skills.xp + $3",
                )
                .bind(player_id)
                .bind(kind)
                .bind(xp)
                .execute(pool)
                .await?;
            }
            Backend::Mem(m) => {
                *m.lock().unwrap().skills.entry((player_id, kind.to_string())).or_insert(0) += xp;
            }
        }
        Ok(())
    }

    /// Read a player's Meld skills as (kind, total_xp).
    pub async fn get_skills(&self, player_id: Uuid) -> Result<Vec<(String, i64)>, DbError> {
        match &self.backend {
            Backend::Pg(pool) => {
                let rows = sqlx::query(
                    "SELECT skill_kind, xp FROM meld_skills WHERE player_id = $1 ORDER BY skill_kind",
                )
                .bind(player_id)
                .fetch_all(pool)
                .await?;
                Ok(rows
                    .iter()
                    .map(|r| (r.get::<String, _>("skill_kind"), r.get::<i64, _>("xp")))
                    .collect())
            }
            Backend::Mem(m) => {
                let m = m.lock().unwrap();
                let mut rows: Vec<(String, i64)> = m
                    .skills
                    .iter()
                    .filter(|((p, _), _)| *p == player_id)
                    .map(|((_, kind), xp)| (kind.clone(), *xp))
                    .collect();
                rows.sort_by(|a, b| a.0.cmp(&b.0));
                Ok(rows)
            }
        }
    }

    /// Craft: atomically consume `inputs` from the Vault, add `output`, and
    /// credit Forging XP. Returns `false` if materials are insufficient.
    pub async fn craft(
        &self,
        player_id: Uuid,
        inputs: &[(String, i32)],
        output: (&str, i32),
        forging_xp: i64,
    ) -> Result<bool, DbError> {
        match &self.backend {
            Backend::Pg(pool) => {
                let mut tx = pool.begin().await?;
                for (kind, need) in inputs {
                    let res = sqlx::query(
                        "UPDATE vault_items SET quantity = quantity - $3
                         WHERE player_id = $1 AND item_kind = $2 AND quantity >= $3",
                    )
                    .bind(player_id)
                    .bind(kind)
                    .bind(need)
                    .execute(&mut *tx)
                    .await?;
                    if res.rows_affected() == 0 {
                        tx.rollback().await?;
                        return Ok(false);
                    }
                }
                sqlx::query("DELETE FROM vault_items WHERE player_id = $1 AND quantity <= 0")
                    .bind(player_id)
                    .execute(&mut *tx)
                    .await?;
                sqlx::query(
                    "INSERT INTO vault_items (player_id, item_kind, quantity) VALUES ($1, $2, $3)
                     ON CONFLICT (player_id, item_kind) DO UPDATE SET quantity = vault_items.quantity + $3",
                )
                .bind(player_id)
                .bind(output.0)
                .bind(output.1)
                .execute(&mut *tx)
                .await?;
                sqlx::query(
                    "INSERT INTO meld_skills (player_id, skill_kind, xp) VALUES ($1, 'forging', $2)
                     ON CONFLICT (player_id, skill_kind) DO UPDATE SET xp = meld_skills.xp + $2",
                )
                .bind(player_id)
                .bind(forging_xp)
                .execute(&mut *tx)
                .await?;
                tx.commit().await?;
                Ok(true)
            }
            Backend::Mem(m) => {
                let mut m = m.lock().unwrap();
                // Pre-check availability so we don't partially consume on failure.
                for (kind, need) in inputs {
                    let have = m
                        .vault_items
                        .get(&(player_id, kind.clone()))
                        .copied()
                        .unwrap_or(0);
                    if have < *need {
                        return Ok(false);
                    }
                }
                for (kind, need) in inputs {
                    let key = (player_id, kind.clone());
                    let q = m.vault_items.get_mut(&key).unwrap();
                    *q -= *need;
                    if *q <= 0 {
                        m.vault_items.remove(&key);
                    }
                }
                *m.vault_items
                    .entry((player_id, output.0.to_string()))
                    .or_insert(0) += output.1;
                *m.skills
                    .entry((player_id, "forging".to_string()))
                    .or_insert(0) += forging_xp;
                Ok(true)
            }
        }
    }

    /// Withdraw `qty` of `item_kind` from the Vault (storage chest) into the
    /// player's pending-backpack queue — staged to seed their *next* run's
    /// Backpack (`form_run` drains + clears it at dive time). Atomic: fails with
    /// `InsufficientStock` (no-op) if the Vault doesn't have enough.
    pub async fn withdraw_material(
        &self,
        player_id: Uuid,
        item_kind: &str,
        qty: i32,
    ) -> Result<WithdrawResult, DbError> {
        match &self.backend {
            Backend::Pg(pool) => {
                let mut tx = pool.begin().await?;
                let res = sqlx::query(
                    "UPDATE vault_items SET quantity = quantity - $3
                     WHERE player_id = $1 AND item_kind = $2 AND quantity >= $3",
                )
                .bind(player_id)
                .bind(item_kind)
                .bind(qty)
                .execute(&mut *tx)
                .await?;
                if res.rows_affected() == 0 {
                    tx.rollback().await?;
                    return Ok(WithdrawResult::InsufficientStock);
                }
                sqlx::query("DELETE FROM vault_items WHERE player_id = $1 AND quantity <= 0")
                    .bind(player_id)
                    .execute(&mut *tx)
                    .await?;
                sqlx::query(
                    "INSERT INTO pending_backpack (player_id, item_kind, quantity) VALUES ($1, $2, $3)
                     ON CONFLICT (player_id, item_kind) DO UPDATE SET quantity = pending_backpack.quantity + $3",
                )
                .bind(player_id)
                .bind(item_kind)
                .bind(qty)
                .execute(&mut *tx)
                .await?;
                tx.commit().await?;
                Ok(WithdrawResult::Ok)
            }
            Backend::Mem(m) => {
                let mut m = m.lock().unwrap();
                let key = (player_id, item_kind.to_string());
                let have = m.vault_items.get(&key).copied().unwrap_or(0);
                if have < qty {
                    return Ok(WithdrawResult::InsufficientStock);
                }
                let q = m.vault_items.get_mut(&key).unwrap();
                *q -= qty;
                if *q <= 0 {
                    m.vault_items.remove(&key);
                }
                *m.pending_backpack.entry(key).or_insert(0) += qty;
                Ok(WithdrawResult::Ok)
            }
        }
    }

    /// Read a player's pending-backpack queue (materials withdrawn from the
    /// Vault, staged for their next dive).
    pub async fn get_pending_backpack(&self, player_id: Uuid) -> Result<Vec<(String, i32)>, DbError> {
        match &self.backend {
            Backend::Pg(pool) => {
                let rows = sqlx::query(
                    "SELECT item_kind, quantity FROM pending_backpack WHERE player_id = $1 ORDER BY item_kind",
                )
                .bind(player_id)
                .fetch_all(pool)
                .await?;
                Ok(rows
                    .iter()
                    .map(|r| (r.get::<String, _>("item_kind"), r.get::<i32, _>("quantity")))
                    .collect())
            }
            Backend::Mem(m) => {
                let m = m.lock().unwrap();
                let mut items: Vec<(String, i32)> = m
                    .pending_backpack
                    .iter()
                    .filter(|((p, _), _)| *p == player_id)
                    .map(|((_, kind), qty)| (kind.clone(), *qty))
                    .collect();
                items.sort_by(|a, b| a.0.cmp(&b.0));
                Ok(items)
            }
        }
    }

    /// Clear a player's pending-backpack queue — called once its contents have
    /// been folded into a freshly-formed run's live Backpack.
    pub async fn clear_pending_backpack(&self, player_id: Uuid) -> Result<(), DbError> {
        match &self.backend {
            Backend::Pg(pool) => {
                sqlx::query("DELETE FROM pending_backpack WHERE player_id = $1")
                    .bind(player_id)
                    .execute(pool)
                    .await?;
            }
            Backend::Mem(m) => {
                let mut m = m.lock().unwrap();
                m.pending_backpack.retain(|(p, _), _| *p != player_id);
            }
        }
        Ok(())
    }

    /// Verify a login. Returns `Some(player)` on a correct password, `None` for
    /// an unknown username OR a wrong password — indistinguishable, with matched
    /// timing (D17, M1.9).
    pub async fn verify_login(
        &self,
        username: &str,
        password: &str,
    ) -> Result<Option<PlayerRow>, DbError> {
        // (stored password hash, PlayerRow) for the account, if it exists.
        let account: Option<(String, PlayerRow)> = match &self.backend {
            Backend::Pg(pool) => {
                let maybe = sqlx::query(
                    r#"
                    SELECT player_id, username, password_hash, created_at, active_title
                    FROM players WHERE username = $1
                    "#,
                )
                .bind(username)
                .fetch_optional(pool)
                .await?;
                maybe.map(|row| (row.get::<String, _>("password_hash"), row_to_player(&row)))
            }
            Backend::Mem(m) => {
                let m = m.lock().unwrap();
                m.players
                    .values()
                    .find(|p| p.username == username)
                    .map(|p| (p.password_hash.clone(), p.to_row()))
            }
        };

        // bcrypt verify is CPU-heavy — run it on the blocking pool (see `register`).
        match account {
            Some((stored, player)) => {
                let pw = password.to_string();
                let ok = tokio::task::spawn_blocking(move || verify(pw, &stored).unwrap_or(false))
                    .await
                    .unwrap_or(false);
                if ok {
                    Ok(Some(player))
                } else {
                    Ok(None)
                }
            }
            None => {
                // Burn equivalent time so a missing account isn't faster.
                let pw = password.to_string();
                let _ = tokio::task::spawn_blocking(move || verify(pw, DUMMY_HASH)).await;
                Ok(None)
            }
        }
    }

    /// Fetch an account by id (for `GET /v1/players/me`).
    pub async fn get_player(&self, player_id: Uuid) -> Result<Option<PlayerRow>, DbError> {
        match &self.backend {
            Backend::Pg(pool) => {
                let row = sqlx::query(
                    r#"
                    SELECT player_id, username, created_at, active_title
                    FROM players WHERE player_id = $1
                    "#,
                )
                .bind(player_id)
                .fetch_optional(pool)
                .await?;
                Ok(row.map(|r| row_to_player(&r)))
            }
            Backend::Mem(m) => Ok(m.lock().unwrap().players.get(&player_id).map(|p| p.to_row())),
        }
    }

    /// List a player's gear.
    pub async fn get_gear(&self, player_id: Uuid) -> Result<Vec<GearRow>, DbError> {
        match &self.backend {
            Backend::Pg(pool) => {
                let rows = sqlx::query(
                    "SELECT gear_id, name, slot, class_key, insurance, tier, atk_bonus, def_bonus, spd_bonus, base_max_durability, max_durability, equipped_hero_slot, damage_modifiers
                     FROM gear WHERE owner_player_id = $1 ORDER BY equipped_hero_slot IS NOT NULL DESC, name",
                )
                .bind(player_id)
                .fetch_all(pool)
                .await?;
                Ok(rows.iter().map(row_to_gear).collect())
            }
            Backend::Mem(m) => {
                let m = m.lock().unwrap();
                let mut rows: Vec<GearRow> = m
                    .gear
                    .values()
                    .filter(|g| g.owner_player_id == player_id)
                    .map(|g| g.to_row())
                    .collect();
                // ORDER BY equipped_hero_slot IS NOT NULL DESC, name.
                rows.sort_by(|a, b| {
                    b.equipped_hero_slot.is_some().cmp(&a.equipped_hero_slot.is_some()).then(a.name.cmp(&b.name))
                });
                Ok(rows)
            }
        }
    }

    /// Backfill any of a player's hero slots (`0..party_size`) missing a
    /// piece of gear in some category (the six of the 7-slot loadout —
    /// accessory counts once; the second accessory equip slot starts empty)
    /// with a permanent, class-unrestricted +1 starter piece — so nobody is
    /// ever looking at a genuinely empty slot, in town or mid-run. Idempotent
    /// (checks what's already equipped first, in that category, regardless
    /// of source); safe to call on every Vault touch. `blue`-chest like the
    /// starter weapon it complements, so it survives death like the rest of
    /// an account's permanent kit.
    pub async fn ensure_starter_gear(&self, player_id: Uuid, party_size: i32) -> Result<(), DbError> {
        let existing = self.get_gear(player_id).await?;
        let mut have: std::collections::HashSet<(i32, String)> = std::collections::HashSet::new();
        for g in &existing {
            if let Some(slot) = g.equipped_hero_slot {
                have.insert((slot, g.slot.clone()));
            }
        }
        // The kit's TOTAL stays +1 atk / +1 def / +1 spd (same budget as the
        // old 3-piece kit): the extra pieces are slot-fillers, so a fresh
        // account's defense doesn't quietly double with the 7-slot loadout.
        let kit = [
            ("main_hand", "Novice Blade", 1, 0, 0),
            ("off_hand", "Novice Buckler", 0, 0, 0),
            ("head", "Novice Cap", 0, 0, 0),
            ("chest", "Novice Vest", 0, 1, 0),
            ("legs", "Novice Greaves", 0, 0, 0),
            ("accessory", "Novice Charm", 0, 0, 1),
        ];
        let mut to_insert = Vec::new();
        for slot in 0..party_size {
            for (category, name, atk, def, spd) in kit {
                if !have.contains(&(slot, category.to_string())) {
                    to_insert.push((slot, category, name, atk, def, spd));
                }
            }
        }
        if to_insert.is_empty() {
            return Ok(());
        }
        match &self.backend {
            Backend::Pg(pool) => {
                let mut tx = pool.begin().await?;
                for (slot, category, name, atk, def, spd) in &to_insert {
                    sqlx::query(
                        "INSERT INTO gear (gear_id, owner_player_id, name, slot, class_key, insurance, tier, atk_bonus, def_bonus, spd_bonus, base_max_durability, max_durability, equipped_hero_slot, damage_modifiers)
                         VALUES ($1, $2, $3, $4, '', 'blue', 0, $5, $6, $7, 100, 100, $8, '{}')",
                    )
                    .bind(Uuid::now_v7())
                    .bind(player_id)
                    .bind(*name)
                    .bind(*category)
                    .bind(*atk)
                    .bind(*def)
                    .bind(*spd)
                    .bind(*slot)
                    .execute(&mut *tx)
                    .await?;
                }
                tx.commit().await?;
            }
            Backend::Mem(m) => {
                let mut m = m.lock().unwrap();
                for (slot, category, name, atk, def, spd) in &to_insert {
                    let gear_id = Uuid::now_v7();
                    m.gear.insert(
                        gear_id,
                        MemGear {
                            gear_id,
                            owner_player_id: player_id,
                            name: (*name).to_string(),
                            slot: (*category).to_string(),
                            class_key: String::new(),
                            insurance: "blue".to_string(),
                            tier: 0,
                            atk_bonus: *atk,
                            def_bonus: *def,
                            spd_bonus: *spd,
                            base_max_durability: 100,
                            max_durability: 100,
                            equipped_hero_slot: Some(*slot),
                            damage_modifiers: "{}".into(),
                        },
                    );
                }
            }
        }
        Ok(())
    }

    /// Bank a run's looted red-chest gear into the Vault as owned gear
    /// (gear-item-models.md: extraction converts run loot to owned gear that stays
    /// `red`). Inserted unequipped; the gear_id is the one already assigned at
    /// drop time. Part of the extraction transaction's spirit; called alongside
    /// [`Self::bank_extraction`].
    pub async fn insert_looted_gear(
        &self,
        player_id: Uuid,
        gear: &[LootedGear],
    ) -> Result<(), DbError> {
        if gear.is_empty() {
            return Ok(());
        }
        match &self.backend {
            Backend::Pg(pool) => {
                let mut tx = pool.begin().await?;
                for g in gear {
                    sqlx::query(
                        "INSERT INTO gear (gear_id, owner_player_id, name, slot, class_key, insurance, tier, atk_bonus, def_bonus, spd_bonus, base_max_durability, max_durability, equipped_hero_slot, damage_modifiers)
                         VALUES ($1, $2, $3, $4, $5, 'red', $6, $7, $8, $9, $10, $11, NULL, $12)
                         ON CONFLICT (gear_id) DO NOTHING",
                    )
                    .bind(g.gear_id)
                    .bind(player_id)
                    .bind(&g.name)
                    .bind(&g.slot)
                    .bind(&g.class_key)
                    .bind(g.tier)
                    .bind(g.atk_bonus)
                    .bind(g.def_bonus)
                    .bind(g.spd_bonus)
                    .bind(g.base_max_durability)
                    .bind(g.max_durability)
                    .bind(&g.damage_modifiers)
                    .execute(&mut *tx)
                    .await?;
                }
                tx.commit().await?;
            }
            Backend::Mem(m) => {
                let mut m = m.lock().unwrap();
                for g in gear {
                    // ON CONFLICT (gear_id) DO NOTHING.
                    m.gear.entry(g.gear_id).or_insert_with(|| MemGear {
                        gear_id: g.gear_id,
                        owner_player_id: player_id,
                        name: g.name.clone(),
                        slot: g.slot.clone(),
                        class_key: g.class_key.clone(),
                        insurance: "red".into(),
                        tier: g.tier,
                        atk_bonus: g.atk_bonus,
                        def_bonus: g.def_bonus,
                        spd_bonus: g.spd_bonus,
                        base_max_durability: g.base_max_durability,
                        max_durability: g.max_durability,
                        equipped_hero_slot: None,
                        damage_modifiers: g.damage_modifiers.clone(),
                    });
                }
            }
        }
        Ok(())
    }

    /// Per-hero-slot totals from a player's currently-equipped gear, indexed
    /// `0..party_size` (each hero's own weapon/armor/accessory summed).
    /// `hero_classes[slot]` is that slot's class *for this dive* (content key,
    /// e.g. `"explorer"`; out-of-range/unknown slots contribute nothing from
    /// class-restricted gear) — a class-specific item's bonus only counts
    /// when it matches, silently excluded otherwise. This is the enforcement
    /// point for class-restricted gear: equipping it (HTTP, outside any run)
    /// is never blocked, since a hero's class for the *next* dive isn't known
    /// yet at equip time, but a mismatched item just contributes 0 here.
    pub async fn equipped_gear_bonuses(
        &self,
        player_id: Uuid,
        party_size: i32,
        hero_classes: &[String],
    ) -> Result<Vec<GearBonus>, DbError> {
        let mut bonuses = vec![GearBonus::default(); party_size.max(0) as usize];
        let class_ok = |slot: usize, class_key: &str| -> bool {
            class_key.is_empty() || hero_classes.get(slot).map(|c| c.as_str()) == Some(class_key)
        };
        match &self.backend {
            Backend::Pg(pool) => {
                let rows = sqlx::query(
                    "SELECT equipped_hero_slot, atk_bonus, def_bonus, spd_bonus, class_key, max_durability, damage_modifiers FROM gear
                     WHERE owner_player_id = $1 AND equipped_hero_slot IS NOT NULL",
                )
                .bind(player_id)
                .fetch_all(pool)
                .await?;
                for row in rows {
                    let slot: i32 = row.get("equipped_hero_slot");
                    let class_key: String = row.get("class_key");
                    // Crucial guardrail (spec §5): broken gear (max durability
                    // 0) contributes NOTHING until repaired.
                    if row.get::<i32, _>("max_durability") == 0 {
                        continue;
                    }
                    if class_ok(slot as usize, &class_key) {
                        if let Some(b) = bonuses.get_mut(slot as usize) {
                            b.atk += row.get::<i32, _>("atk_bonus");
                            b.def += row.get::<i32, _>("def_bonus");
                            b.spd += row.get::<i32, _>("spd_bonus");
                            append_modifier_entries(
                                &mut b.modifiers,
                                &row.get::<String, _>("damage_modifiers"),
                            );
                        }
                    }
                }
            }
            Backend::Mem(m) => {
                let m = m.lock().unwrap();
                for g in m.gear.values().filter(|g| g.owner_player_id == player_id) {
                    if let Some(slot) = g.equipped_hero_slot {
                        // Broken gear contributes nothing (spec §5 guardrail).
                        if g.max_durability == 0 {
                            continue;
                        }
                        if class_ok(slot as usize, &g.class_key) {
                            if let Some(b) = bonuses.get_mut(slot as usize) {
                                b.atk += g.atk_bonus;
                                b.def += g.def_bonus;
                                b.spd += g.spd_bonus;
                                append_modifier_entries(&mut b.modifiers, &g.damage_modifiers);
                            }
                        }
                    }
                }
            }
        }
        Ok(bonuses)
    }

    /// Delete every Vault-owned `red` gear item this player has EQUIPPED —
    /// the spec §5 canon-gap resolution: red gear brought back into a run is
    /// at absolute risk, permanently deleted when the run ends `died` OR
    /// `abandoned`. (Blue gear only decays; unequipped red gear sat safe in
    /// the Vault and is untouched.)
    pub async fn delete_equipped_red_gear(&self, player_id: Uuid) -> Result<(), DbError> {
        match &self.backend {
            Backend::Pg(pool) => {
                sqlx::query(
                    "DELETE FROM gear
                     WHERE owner_player_id = $1 AND insurance = 'red' AND equipped_hero_slot IS NOT NULL",
                )
                .bind(player_id)
                .execute(pool)
                .await?;
            }
            Backend::Mem(m) => {
                let mut m = m.lock().unwrap();
                m.gear.retain(|_, g| {
                    !(g.owner_player_id == player_id
                        && g.insurance == "red"
                        && g.equipped_hero_slot.is_some())
                });
            }
        }
        Ok(())
    }

    /// Apply the death durability sink to equipped blue-chest gear:
    /// `max_durability ← floor(max_durability × 0.9)` (CANON.md D6).
    pub async fn apply_death_durability(&self, player_id: Uuid) -> Result<(), DbError> {
        match &self.backend {
            Backend::Pg(pool) => {
                sqlx::query(
                    "UPDATE gear SET max_durability = (max_durability * 9) / 10
                     WHERE owner_player_id = $1 AND insurance = 'blue' AND equipped_hero_slot IS NOT NULL",
                )
                .bind(player_id)
                .execute(pool)
                .await?;
            }
            Backend::Mem(m) => {
                let mut m = m.lock().unwrap();
                for g in m.gear.values_mut() {
                    if g.owner_player_id == player_id && g.insurance == "blue" && g.equipped_hero_slot.is_some() {
                        g.max_durability = (g.max_durability * 9) / 10;
                    }
                }
            }
        }
        Ok(())
    }

    /// Equip a gear item to hero slot `Some(hero_slot)`, or unequip it with
    /// `None`, enforcing the loadout rules (vault-gear.md equip endpoint).
    /// Equipping is idempotent (already worn by that same hero → no-op),
    /// rejects broken gear (max durability 0, CANON.md D6), and enforces one
    /// item per `(hero, slot category)` — a different item already worn by that
    /// hero in the same category conflicts, the caller unequips it first.
    /// Equipping an item already worn by a *different* hero simply moves it.
    /// Unequipping is idempotent. Returns [`EquipResult`] so the API can map to
    /// the right HTTP status.
    ///
    /// Spike divergence (documented): the spec also locks the loadout while a run
    /// is in progress and restricts equip to `insurance: blue`. This slice omits
    /// the run-lock (the HTTP API has no view of in-memory run state) and — per
    /// vault-gear.md's own "this is the endpoint to relax" note — allows equipping
    /// extracted `red` loot, since red drops are the loop's main gear source.
    pub async fn set_equipped(
        &self,
        player_id: Uuid,
        gear_id: Uuid,
        target: Option<i32>,
    ) -> Result<EquipResult, DbError> {
        match &self.backend {
            Backend::Pg(pool) => {
                let mut tx = pool.begin().await?;
                // Load the target (owner-scoped so existence isn't leaked cross-account).
                let row = sqlx::query(
                    "SELECT slot, max_durability, equipped_hero_slot FROM gear
                     WHERE gear_id = $1 AND owner_player_id = $2",
                )
                .bind(gear_id)
                .bind(player_id)
                .fetch_optional(&mut *tx)
                .await?;
                let Some(row) = row else {
                    tx.rollback().await?;
                    return Ok(EquipResult::NotFound);
                };
                let slot: String = row.get("slot");
                let max_durability: i32 = row.get("max_durability");
                let already: Option<i32> = row.get("equipped_hero_slot");

                let Some(hero_slot) = target else {
                    // Unequip is idempotent; just clear it.
                    sqlx::query("UPDATE gear SET equipped_hero_slot = NULL WHERE gear_id = $1")
                        .bind(gear_id)
                        .execute(&mut *tx)
                        .await?;
                    tx.commit().await?;
                    return Ok(EquipResult::Ok);
                };

                // Equip: idempotent no-op if already worn by this same hero.
                if already == Some(hero_slot) {
                    tx.rollback().await?;
                    return Ok(EquipResult::Ok);
                }
                // Broken gear cannot be equipped until repaired (CANON.md D6).
                if max_durability == 0 {
                    tx.rollback().await?;
                    return Ok(EquipResult::Broken);
                }
                // Per-(hero, category) capacity: one item everywhere except
                // accessories, which get TWO equip slots (ACCESSORY_1/2 of the
                // 7-slot loadout, spec §5).
                let occupied: i64 = sqlx::query_scalar(
                    "SELECT COUNT(*) FROM gear
                     WHERE owner_player_id = $1 AND slot = $2 AND equipped_hero_slot = $3 AND gear_id <> $4",
                )
                .bind(player_id)
                .bind(&slot)
                .bind(hero_slot)
                .bind(gear_id)
                .fetch_one(&mut *tx)
                .await?;
                if occupied >= category_capacity(&slot) {
                    tx.rollback().await?;
                    return Ok(EquipResult::SlotOccupied);
                }
                sqlx::query("UPDATE gear SET equipped_hero_slot = $2 WHERE gear_id = $1")
                    .bind(gear_id)
                    .bind(hero_slot)
                    .execute(&mut *tx)
                    .await?;
                tx.commit().await?;
                Ok(EquipResult::Ok)
            }
            Backend::Mem(m) => {
                let mut m = m.lock().unwrap();
                // Load the target (owner-scoped so existence isn't leaked cross-account).
                let Some((slot, max_durability, already)) = m
                    .gear
                    .get(&gear_id)
                    .filter(|g| g.owner_player_id == player_id)
                    .map(|g| (g.slot.clone(), g.max_durability, g.equipped_hero_slot))
                else {
                    return Ok(EquipResult::NotFound);
                };

                let Some(hero_slot) = target else {
                    m.gear.get_mut(&gear_id).unwrap().equipped_hero_slot = None;
                    return Ok(EquipResult::Ok);
                };
                if already == Some(hero_slot) {
                    return Ok(EquipResult::Ok);
                }
                if max_durability == 0 {
                    return Ok(EquipResult::Broken);
                }
                let occupied = m
                    .gear
                    .values()
                    .filter(|g| {
                        g.owner_player_id == player_id
                            && g.slot == slot
                            && g.equipped_hero_slot == Some(hero_slot)
                            && g.gear_id != gear_id
                    })
                    .count() as i64;
                if occupied >= category_capacity(&slot) {
                    return Ok(EquipResult::SlotOccupied);
                }
                m.gear.get_mut(&gear_id).unwrap().equipped_hero_slot = Some(hero_slot);
                Ok(EquipResult::Ok)
            }
        }
    }
}

/// Outcome of [`Db::set_equipped`], mapped to HTTP status by the API layer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EquipResult {
    /// Applied (or already in the requested state — idempotent).
    Ok,
    /// Gear does not exist or is not owned by the caller → 404.
    NotFound,
    /// Gear at 0 max durability → 409 conflict.
    Broken,
    /// Another item already occupies this slot → 409 conflict.
    SlotOccupied,
}

/// Outcome of [`Db::withdraw_material`], mapped to HTTP status by the API layer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WithdrawResult {
    /// Applied — the Vault had enough of that material.
    Ok,
    /// Fewer than the requested quantity in the Vault → 409 conflict.
    InsufficientStock,
}

/// One hero's summed combat bonuses from their equipped gear.
#[derive(Debug, Clone, Default)]
pub struct GearBonus {
    pub atk: i32,
    pub def: i32,
    pub spd: i32,
    /// Raw per-item elemental entries (DamageType wire key → multiplier) from
    /// every equipped piece — folded (`1 + Σ(mᵢ−1)`) and clamped to 0.0–2.0 at
    /// battle assembly (spec §5 stat aggregation).
    pub modifiers: Vec<(String, f64)>,
}

/// A red-chest gear item to bank into the Vault on extraction.
#[derive(Debug, Clone)]
pub struct LootedGear {
    pub gear_id: Uuid,
    pub name: String,
    pub slot: String,
    /// Which class this item is for (empty = unrestricted).
    pub class_key: String,
    pub tier: i32,
    pub atk_bonus: i32,
    pub def_bonus: i32,
    pub spd_bonus: i32,
    pub base_max_durability: i32,
    pub max_durability: i32,
    /// JSON elemental profile ({"FIRE":0.75}); "{}"/empty for none.
    pub damage_modifiers: String,
}

/// A gear row (blue-chest only, this slice).
#[derive(Debug, Clone)]
pub struct GearRow {
    pub gear_id: Uuid,
    pub name: String,
    pub slot: String,
    /// Which class this item is for (empty = unrestricted).
    pub class_key: String,
    pub insurance: String,
    pub tier: i32,
    pub atk_bonus: i32,
    pub def_bonus: i32,
    pub spd_bonus: i32,
    pub base_max_durability: i32,
    pub max_durability: i32,
    /// Which of the owner's heroes has this equipped, if any.
    pub equipped_hero_slot: Option<i32>,
    /// JSON elemental profile ({"FIRE":0.75}); "{}" for none.
    pub damage_modifiers: String,
}

/// How many items of one category a single hero can wear at once: two
/// accessories (ACCESSORY_1/2 of the 7-slot loadout, spec §5), one everywhere
/// else.
fn category_capacity(category: &str) -> i64 {
    if category == "accessory" {
        2
    } else {
        1
    }
}

/// Parse a gear row's `damage_modifiers` JSON object ({"FIRE":0.75}) and
/// append its entries to a hero's raw modifier list. Malformed/empty JSON
/// contributes nothing (defensive: the column is content-written).
fn append_modifier_entries(out: &mut Vec<(String, f64)>, json: &str) {
    if json.is_empty() || json == "{}" {
        return;
    }
    if let Ok(serde_json::Value::Object(map)) = serde_json::from_str::<serde_json::Value>(json) {
        for (k, v) in map {
            if let Some(m) = v.as_f64() {
                out.push((k, m));
            }
        }
    }
}

fn row_to_gear(row: &sqlx::postgres::PgRow) -> GearRow {
    GearRow {
        gear_id: row.get("gear_id"),
        name: row.get("name"),
        slot: row.get("slot"),
        class_key: row.get("class_key"),
        insurance: row.get("insurance"),
        tier: row.get("tier"),
        atk_bonus: row.get("atk_bonus"),
        def_bonus: row.get("def_bonus"),
        spd_bonus: row.get("spd_bonus"),
        base_max_durability: row.get("base_max_durability"),
        max_durability: row.get("max_durability"),
        equipped_hero_slot: row.get("equipped_hero_slot"),
        damage_modifiers: row.get("damage_modifiers"),
    }
}

fn row_to_player(row: &sqlx::postgres::PgRow) -> PlayerRow {
    PlayerRow {
        player_id: row.get("player_id"),
        username: row.get("username"),
        created_at: row.get("created_at"),
        active_title: row.get("active_title"),
    }
}

// --------------------------------------------------------- in-memory store ---

/// The ephemeral in-memory backend (used by the self-contained QA/demo binary).
/// One flat map per Postgres table; keys mirror each table's primary key. Lives
/// only for the process lifetime — no persistence, resets on restart.
#[derive(Default)]
struct Mem {
    /// players, keyed by player_id.
    players: HashMap<Uuid, MemPlayer>,
    /// vaults.chits, keyed by player_id.
    chits: HashMap<Uuid, i64>,
    /// vault_items.quantity, keyed by (player_id, item_kind).
    vault_items: HashMap<(Uuid, String), i32>,
    /// pending_backpack.quantity, keyed by (player_id, item_kind).
    pending_backpack: HashMap<(Uuid, String), i32>,
    /// gear, keyed by gear_id.
    gear: HashMap<Uuid, MemGear>,
    /// meld_skills.xp, keyed by (player_id, skill_kind).
    skills: HashMap<(Uuid, String), i64>,
    /// heroes.name, keyed by (player_id, slot).
    heroes: HashMap<(Uuid, i16), String>,
    /// heroes.back_row, keyed by (player_id, slot); absent = false (front).
    hero_rows: HashMap<(Uuid, i16), bool>,
    /// vanguard (max_distance, achieved_at), keyed by (season, player_id).
    vanguard: HashMap<(i32, Uuid), (i32, DateTime<Utc>)>,
}

struct MemPlayer {
    player_id: Uuid,
    username: String,
    password_hash: String,
    created_at: DateTime<Utc>,
    active_title: Option<String>,
    has_dived: bool,
}

impl MemPlayer {
    fn to_row(&self) -> PlayerRow {
        PlayerRow {
            player_id: self.player_id,
            username: self.username.clone(),
            created_at: self.created_at,
            active_title: self.active_title.clone(),
        }
    }
}

struct MemGear {
    gear_id: Uuid,
    owner_player_id: Uuid,
    name: String,
    slot: String,
    class_key: String,
    insurance: String,
    tier: i32,
    atk_bonus: i32,
    def_bonus: i32,
    spd_bonus: i32,
    base_max_durability: i32,
    max_durability: i32,
    equipped_hero_slot: Option<i32>,
    /// JSON elemental profile ({"FIRE":0.75}); "{}" for none.
    damage_modifiers: String,
}

impl MemGear {
    fn to_row(&self) -> GearRow {
        GearRow {
            gear_id: self.gear_id,
            name: self.name.clone(),
            slot: self.slot.clone(),
            class_key: self.class_key.clone(),
            insurance: self.insurance.clone(),
            tier: self.tier,
            atk_bonus: self.atk_bonus,
            def_bonus: self.def_bonus,
            spd_bonus: self.spd_bonus,
            base_max_durability: self.base_max_durability,
            max_durability: self.max_durability,
            equipped_hero_slot: self.equipped_hero_slot,
            damage_modifiers: self.damage_modifiers.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // A cheap bcrypt cost keeps the in-memory tests fast (they still exercise the
    // real hash/verify path).
    async fn mem() -> Db {
        Db::connect("memory://test", 4).await.unwrap()
    }

    #[tokio::test]
    async fn register_seeds_account_and_login_roundtrips() {
        let db = mem().await;
        let p = db.register("alice", "pw").await.unwrap();
        assert_eq!(p.username, "alice");

        // Dup username → Conflict.
        assert!(matches!(
            db.register("alice", "other").await,
            Err(DbError::Conflict)
        ));

        // Correct password logs in; wrong password / unknown user do not.
        assert_eq!(db.verify_login("alice", "pw").await.unwrap().unwrap().player_id, p.player_id);
        assert!(db.verify_login("alice", "nope").await.unwrap().is_none());
        assert!(db.verify_login("ghost", "pw").await.unwrap().is_none());

        // Seeded: 4 hero names, 3 skills, a starter weapon equipped to hero 0
        // plus the rest of the 6-category starter kit backfilled for all 4
        // heroes (24 pieces total: the one named "Chipped Blade" + 23
        // "Novice ..."), empty vault.
        assert_eq!(db.get_hero_names(p.player_id).await.unwrap().len(), 4);
        assert_eq!(db.get_skills(p.player_id).await.unwrap().len(), 3);
        let gear = db.get_gear(p.player_id).await.unwrap();
        assert_eq!(gear.len(), 24);
        assert!(gear.iter().all(|g| g.equipped_hero_slot.is_some()));
        assert_eq!(gear[0].name, "Chipped Blade");
        assert_eq!(gear[0].equipped_hero_slot, Some(0));
        assert_eq!(db.equipped_gear_bonuses(p.player_id, 4, &[]).await.unwrap()[0].atk, 3);
        assert_eq!(db.get_vault(p.player_id).await.unwrap(), (0, vec![]));
    }

    #[tokio::test]
    async fn vault_banking_and_crafting() {
        let db = mem().await;
        let p = db.register("bob", "pw").await.unwrap().player_id;

        db.bank_extraction(p, &[("iron".into(), 3), ("wood".into(), 2)], 50)
            .await
            .unwrap();
        db.bank_extraction(p, &[("iron".into(), 1)], 10).await.unwrap();
        let (chits, items) = db.get_vault(p).await.unwrap();
        assert_eq!(chits, 60);
        assert_eq!(items, vec![("iron".to_string(), 4), ("wood".to_string(), 2)]);

        // Craft consumes inputs, adds output, credits forging xp.
        assert!(db.craft(p, &[("iron".into(), 4)], ("blade", 1), 5).await.unwrap());
        let (_, items) = db.get_vault(p).await.unwrap();
        assert_eq!(items, vec![("blade".to_string(), 1), ("wood".to_string(), 2)]);
        let forging = db
            .get_skills(p)
            .await
            .unwrap()
            .into_iter()
            .find(|(k, _)| k == "forging")
            .unwrap()
            .1;
        assert_eq!(forging, 5);

        // Insufficient materials → false, and nothing is consumed.
        assert!(!db.craft(p, &[("wood".into(), 99)], ("plank", 1), 5).await.unwrap());
        let (_, items) = db.get_vault(p).await.unwrap();
        assert_eq!(items, vec![("blade".to_string(), 1), ("wood".to_string(), 2)]);
    }

    #[tokio::test]
    async fn withdraw_materials_stages_a_pending_backpack() {
        let db = mem().await;
        let test_password = Uuid::new_v4().to_string();
        let p = db
            .register("dana", &test_password)
            .await
            .unwrap()
            .player_id;
        db.bank_extraction(p, &[("iron".into(), 5)], 0).await.unwrap();

        // Partial withdraw: decrements the Vault, stages the pending backpack.
        assert_eq!(
            db.withdraw_material(p, "iron", 2).await.unwrap(),
            WithdrawResult::Ok
        );
        let (_, items) = db.get_vault(p).await.unwrap();
        assert_eq!(items, vec![("iron".to_string(), 3)]);
        assert_eq!(
            db.get_pending_backpack(p).await.unwrap(),
            vec![("iron".to_string(), 2)]
        );

        // A second withdraw accumulates in the same pending row.
        assert_eq!(
            db.withdraw_material(p, "iron", 1).await.unwrap(),
            WithdrawResult::Ok
        );
        assert_eq!(
            db.get_pending_backpack(p).await.unwrap(),
            vec![("iron".to_string(), 3)]
        );

        // Over-withdrawing what's left in the Vault is rejected, no-op.
        assert_eq!(
            db.withdraw_material(p, "iron", 99).await.unwrap(),
            WithdrawResult::InsufficientStock
        );
        let (_, items) = db.get_vault(p).await.unwrap();
        assert_eq!(items, vec![("iron".to_string(), 2)]);

        // Clearing empties the pending queue (simulates a dive consuming it).
        db.clear_pending_backpack(p).await.unwrap();
        assert!(db.get_pending_backpack(p).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn equip_rules_and_death_durability() {
        let db = mem().await;
        let p = db.register("carol", "pw").await.unwrap().player_id;
        let starter = db.get_gear(p).await.unwrap()[0].gear_id;

        // A second main-hand; equipping it to the same hero (0) conflicts with
        // the equipped starter (one main-hand per hero).
        db.insert_looted_gear(
            p,
            &[LootedGear {
                gear_id: Uuid::now_v7(),
                name: "Looted Sword".into(),
                slot: "main_hand".into(),
                class_key: String::new(),
                tier: 1,
                atk_bonus: 7,
                def_bonus: 0,
                spd_bonus: 0,
                base_max_durability: 80,
                max_durability: 80,
                damage_modifiers: "{}".into(),
            }],
        )
        .await
        .unwrap();
        let looted = db
            .get_gear(p)
            .await
            .unwrap()
            .into_iter()
            .find(|g| g.name == "Looted Sword")
            .unwrap()
            .gear_id;

        assert_eq!(db.set_equipped(p, looted, Some(0)).await.unwrap(), EquipResult::SlotOccupied);
        // Hero 1 also starts with its own starter weapon (the backfilled
        // starter kit covers every hero) — unequip it first, same as a real
        // player swapping in better gear.
        let hero1_starter_weapon = db
            .get_gear(p)
            .await
            .unwrap()
            .into_iter()
            .find(|g| g.equipped_hero_slot == Some(1) && g.slot == "main_hand")
            .unwrap()
            .gear_id;
        assert_eq!(db.set_equipped(p, hero1_starter_weapon, None).await.unwrap(), EquipResult::Ok);
        // Per-character equip: the looted sword goes on hero 1 instead, no
        // conflict, and hero 0 keeps the starter — two different heroes with
        // two different weapons is exactly the point of this feature.
        assert_eq!(db.set_equipped(p, looted, Some(1)).await.unwrap(), EquipResult::Ok);
        let bonuses = db.equipped_gear_bonuses(p, 4, &[]).await.unwrap();
        assert_eq!(bonuses[0].atk, 3);
        assert_eq!(bonuses[1].atk, 7);
        assert_eq!(db.set_equipped(p, Uuid::now_v7(), Some(0)).await.unwrap(), EquipResult::NotFound);

        // Death sink only touches equipped blue-chest gear (looted sword is red).
        db.apply_death_durability(p).await.unwrap();
        let starter_row = db.get_gear(p).await.unwrap().into_iter().find(|g| g.gear_id == starter).unwrap();
        assert_eq!(starter_row.max_durability, 90); // floor(100 * 0.9)

        // Spec §5 red-gear canon gap: equipped red gear is DELETED on a run
        // that ends died/abandoned (the looted sword is equipped on hero 1),
        // while blue gear survives (decayed above, never deleted).
        db.delete_equipped_red_gear(p).await.unwrap();
        let after = db.get_gear(p).await.unwrap();
        assert!(after.iter().all(|g| g.name != "Looted Sword"), "equipped red gear burned");
        assert!(after.iter().any(|g| g.gear_id == starter), "blue starter survives");
    }

    #[tokio::test]
    async fn two_accessories_but_only_one_of_everything_else() {
        let db = mem().await;
        let p = db.register("erin", "pw").await.unwrap().player_id;
        let ring = |name: &str| LootedGear {
            gear_id: Uuid::now_v7(),
            name: name.into(),
            slot: "accessory".into(),
            class_key: String::new(),
            tier: 1,
            atk_bonus: 0,
            def_bonus: 0,
            spd_bonus: 2,
            base_max_durability: 80,
            max_durability: 80,
            damage_modifiers: "{\"FIRE\":0.75}".into(),
        };
        db.insert_looted_gear(p, &[ring("Ring A"), ring("Ring B")]).await.unwrap();
        let ids: Vec<Uuid> = db
            .get_gear(p)
            .await
            .unwrap()
            .into_iter()
            .filter(|g| g.name.starts_with("Ring"))
            .map(|g| g.gear_id)
            .collect();
        // Hero 0 already wears the Novice Charm (starter kit) — the loadout
        // has TWO accessory equip slots, so one more ring fits, the third
        // accessory conflicts.
        assert_eq!(db.set_equipped(p, ids[0], Some(0)).await.unwrap(), EquipResult::Ok);
        assert_eq!(db.set_equipped(p, ids[1], Some(0)).await.unwrap(), EquipResult::SlotOccupied);
        // The hero's aggregated bonuses carry the ring's elemental profile.
        let bonuses = db.equipped_gear_bonuses(p, 4, &[]).await.unwrap();
        assert!(bonuses[0].modifiers.iter().any(|(k, m)| k == "FIRE" && *m == 0.75));
    }

    #[tokio::test]
    async fn hero_rename_and_skill_xp() {
        let db = mem().await;
        let p = db.register("dave", "pw").await.unwrap().player_id;
        db.set_hero_name(p, 1, "Gandalf").await.unwrap();
        assert_eq!(db.get_hero_names(p).await.unwrap()[1], "Gandalf");
        db.add_skill_xp(p, "alchemy", 12).await.unwrap();
        db.add_skill_xp(p, "alchemy", 3).await.unwrap();
        let alchemy = db.get_skills(p).await.unwrap().into_iter().find(|(k, _)| k == "alchemy").unwrap().1;
        assert_eq!(alchemy, 15);
    }

    #[tokio::test]
    async fn hero_formation_persists() {
        let db = mem().await;
        let p = db.register("nell", "pw").await.unwrap().player_id;
        // Seeded slots default to the front row (all false), aligned with the names.
        assert_eq!(db.get_hero_rows(p).await.unwrap(), vec![false, false, false, false]);
        db.set_hero_row(p, 2, true).await.unwrap();
        assert_eq!(db.get_hero_rows(p).await.unwrap(), vec![false, false, true, false]);
        // Toggling back to the front is remembered too.
        db.set_hero_row(p, 2, false).await.unwrap();
        assert_eq!(db.get_hero_rows(p).await.unwrap()[2], false);
    }

    #[tokio::test]
    async fn vanguard_board_ranks_deepest_first_and_records_only_personal_bests() {
        let db = mem().await;
        let deep_password = Uuid::new_v4().to_string();
        let shallow_password = Uuid::new_v4().to_string();
        let never_password = Uuid::new_v4().to_string();
        let deep = db.register("vg_deep", &deep_password).await.unwrap();
        let shallow = db.register("vg_shallow", &shallow_password).await.unwrap();
        let season = current_season();

        assert!(db.record_vanguard_distance(deep.player_id, season, 400).await.unwrap());
        assert!(db.record_vanguard_distance(shallow.player_id, season, 120).await.unwrap());
        // A shallower run never replaces a deeper record, and reports no new best.
        assert!(!db.record_vanguard_distance(deep.player_id, season, 200).await.unwrap());
        assert!(db.record_vanguard_distance(deep.player_id, season, 900).await.unwrap());
        // Distance 0 (never left the hub) does not put you on the board at all.
        let never = db.register("vg_never", &never_password).await.unwrap();
        assert!(!db.record_vanguard_distance(never.player_id, season, 0).await.unwrap());

        let board = db.vanguard_board(season, 100).await.unwrap();
        assert_eq!(board.len(), 2);
        assert_eq!(board[0].username, "vg_deep");
        assert_eq!(board[0].max_distance, 900);
        assert_eq!(board[1].username, "vg_shallow");
        // Seasons are separate boards: last season's rows never leak into this one.
        assert!(db.vanguard_board(season - 1, 100).await.unwrap().is_empty());
    }

    #[test]
    fn seasons_are_back_to_back_13_week_windows() {
        assert_eq!(season_at(SEASON_EPOCH_UNIX), 0);
        assert_eq!(season_at(SEASON_EPOCH_UNIX + SEASON_LEN_SECS - 1), 0);
        assert_eq!(season_at(SEASON_EPOCH_UNIX + SEASON_LEN_SECS), 1);
        assert_eq!(season_at(SEASON_EPOCH_UNIX + 2 * SEASON_LEN_SECS), 2);
        // A clock skewed before the epoch clamps instead of minting season -1.
        assert_eq!(season_at(0), 0);
    }
}

// ------------------------------------------------------- the Vanguard Board ---

/// One stored Vanguard Board record (roadmap P1-1). Unranked — rank is assigned
/// by the reader from the query's order, so a slice of the board still ranks 1..n.
#[derive(Debug, Clone)]
pub struct VanguardRow {
    pub player_id: Uuid,
    pub username: String,
    pub max_distance: i32,
    pub achieved_at: DateTime<Utc>,
}

/// Seasons are back-to-back 13-week UTC epochs with no off-season gap
/// (behaviors/endgame-seasons.md, CANON §B — structural). Season 0 opens at
/// `SEASON_EPOCH_UNIX`; every board query resolves its season through here so
/// the server never stores a wall-clock season id it would later disagree with.
pub const SEASON_EPOCH_UNIX: i64 = 1_735_689_600;

/// 13 weeks in seconds — the season length (structural, not a balance tunable).
pub const SEASON_LEN_SECS: i64 = 13 * 7 * 24 * 60 * 60;

/// Which season a unix-seconds instant falls in. Instants before the epoch clamp
/// to season 0, so a clock skewed backwards can't mint a negative board.
pub fn season_at(unix_secs: i64) -> i32 {
    if unix_secs <= SEASON_EPOCH_UNIX {
        return 0;
    }
    ((unix_secs - SEASON_EPOCH_UNIX) / SEASON_LEN_SECS) as i32
}

/// The season currently open.
pub fn current_season() -> i32 {
    season_at(Utc::now().timestamp())
}
