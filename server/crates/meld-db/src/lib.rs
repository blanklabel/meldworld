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
        Ok(())
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
        match &self.backend {
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
                     VALUES ($1, $2, 'Chipped Blade', 'weapon', 'blue', 0, 3, 100, 100, 0)",
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
                        slot: "weapon".into(),
                        insurance: "blue".into(),
                        tier: 0,
                        atk_bonus: 3,
                        def_bonus: 0,
                        spd_bonus: 0,
                        base_max_durability: 100,
                        max_durability: 100,
                        equipped_hero_slot: Some(0),
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
        }
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
                    "SELECT gear_id, name, slot, insurance, tier, atk_bonus, def_bonus, spd_bonus, base_max_durability, max_durability, equipped_hero_slot
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
                        "INSERT INTO gear (gear_id, owner_player_id, name, slot, insurance, tier, atk_bonus, def_bonus, spd_bonus, base_max_durability, max_durability, equipped_hero_slot)
                         VALUES ($1, $2, $3, $4, 'red', $5, $6, $7, $8, $9, $10, NULL)
                         ON CONFLICT (gear_id) DO NOTHING",
                    )
                    .bind(g.gear_id)
                    .bind(player_id)
                    .bind(&g.name)
                    .bind(&g.slot)
                    .bind(g.tier)
                    .bind(g.atk_bonus)
                    .bind(g.def_bonus)
                    .bind(g.spd_bonus)
                    .bind(g.base_max_durability)
                    .bind(g.max_durability)
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
                        insurance: "red".into(),
                        tier: g.tier,
                        atk_bonus: g.atk_bonus,
                        def_bonus: g.def_bonus,
                        spd_bonus: g.spd_bonus,
                        base_max_durability: g.base_max_durability,
                        max_durability: g.max_durability,
                        equipped_hero_slot: None,
                    });
                }
            }
        }
        Ok(())
    }

    /// Per-hero-slot totals from a player's currently-equipped gear, indexed
    /// `0..party_size` (each hero's own weapon/armor/accessory summed).
    pub async fn equipped_gear_bonuses(
        &self,
        player_id: Uuid,
        party_size: i32,
    ) -> Result<Vec<GearBonus>, DbError> {
        let mut bonuses = vec![GearBonus::default(); party_size.max(0) as usize];
        match &self.backend {
            Backend::Pg(pool) => {
                let rows = sqlx::query(
                    "SELECT equipped_hero_slot, atk_bonus, def_bonus, spd_bonus FROM gear
                     WHERE owner_player_id = $1 AND equipped_hero_slot IS NOT NULL",
                )
                .bind(player_id)
                .fetch_all(pool)
                .await?;
                for row in rows {
                    let slot: i32 = row.get("equipped_hero_slot");
                    if let Some(b) = bonuses.get_mut(slot as usize) {
                        b.atk += row.get::<i32, _>("atk_bonus");
                        b.def += row.get::<i32, _>("def_bonus");
                        b.spd += row.get::<i32, _>("spd_bonus");
                    }
                }
            }
            Backend::Mem(m) => {
                let m = m.lock().unwrap();
                for g in m.gear.values().filter(|g| g.owner_player_id == player_id) {
                    if let Some(slot) = g.equipped_hero_slot {
                        if let Some(b) = bonuses.get_mut(slot as usize) {
                            b.atk += g.atk_bonus;
                            b.def += g.def_bonus;
                            b.spd += g.spd_bonus;
                        }
                    }
                }
            }
        }
        Ok(bonuses)
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
                // One item per (hero, slot category): a different item already
                // worn by this hero in the same category conflicts.
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
                if occupied > 0 {
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
                let occupied = m.gear.values().any(|g| {
                    g.owner_player_id == player_id
                        && g.slot == slot
                        && g.equipped_hero_slot == Some(hero_slot)
                        && g.gear_id != gear_id
                });
                if occupied {
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

/// One hero's summed combat bonuses from their equipped gear.
#[derive(Debug, Clone, Copy, Default)]
pub struct GearBonus {
    pub atk: i32,
    pub def: i32,
    pub spd: i32,
}

/// A red-chest gear item to bank into the Vault on extraction.
#[derive(Debug, Clone)]
pub struct LootedGear {
    pub gear_id: Uuid,
    pub name: String,
    pub slot: String,
    pub tier: i32,
    pub atk_bonus: i32,
    pub def_bonus: i32,
    pub spd_bonus: i32,
    pub base_max_durability: i32,
    pub max_durability: i32,
}

/// A gear row (blue-chest only, this slice).
#[derive(Debug, Clone)]
pub struct GearRow {
    pub gear_id: Uuid,
    pub name: String,
    pub slot: String,
    pub insurance: String,
    pub tier: i32,
    pub atk_bonus: i32,
    pub def_bonus: i32,
    pub spd_bonus: i32,
    pub base_max_durability: i32,
    pub max_durability: i32,
    /// Which of the owner's heroes has this equipped, if any.
    pub equipped_hero_slot: Option<i32>,
}

fn row_to_gear(row: &sqlx::postgres::PgRow) -> GearRow {
    GearRow {
        gear_id: row.get("gear_id"),
        name: row.get("name"),
        slot: row.get("slot"),
        insurance: row.get("insurance"),
        tier: row.get("tier"),
        atk_bonus: row.get("atk_bonus"),
        def_bonus: row.get("def_bonus"),
        spd_bonus: row.get("spd_bonus"),
        base_max_durability: row.get("base_max_durability"),
        max_durability: row.get("max_durability"),
        equipped_hero_slot: row.get("equipped_hero_slot"),
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
    /// gear, keyed by gear_id.
    gear: HashMap<Uuid, MemGear>,
    /// meld_skills.xp, keyed by (player_id, skill_kind).
    skills: HashMap<(Uuid, String), i64>,
    /// heroes.name, keyed by (player_id, slot).
    heroes: HashMap<(Uuid, i16), String>,
    /// heroes.back_row, keyed by (player_id, slot); absent = false (front).
    hero_rows: HashMap<(Uuid, i16), bool>,
}

struct MemPlayer {
    player_id: Uuid,
    username: String,
    password_hash: String,
    created_at: DateTime<Utc>,
    active_title: Option<String>,
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
    insurance: String,
    tier: i32,
    atk_bonus: i32,
    def_bonus: i32,
    spd_bonus: i32,
    base_max_durability: i32,
    max_durability: i32,
    equipped_hero_slot: Option<i32>,
}

impl MemGear {
    fn to_row(&self) -> GearRow {
        GearRow {
            gear_id: self.gear_id,
            name: self.name.clone(),
            slot: self.slot.clone(),
            insurance: self.insurance.clone(),
            tier: self.tier,
            atk_bonus: self.atk_bonus,
            def_bonus: self.def_bonus,
            spd_bonus: self.spd_bonus,
            base_max_durability: self.base_max_durability,
            max_durability: self.max_durability,
            equipped_hero_slot: self.equipped_hero_slot,
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

        // Seeded: 4 hero names, 3 skills, a starter weapon equipped to hero 0, empty vault.
        assert_eq!(db.get_hero_names(p.player_id).await.unwrap().len(), 4);
        assert_eq!(db.get_skills(p.player_id).await.unwrap().len(), 3);
        let gear = db.get_gear(p.player_id).await.unwrap();
        assert_eq!(gear.len(), 1);
        assert_eq!(gear[0].equipped_hero_slot, Some(0));
        assert_eq!(db.equipped_gear_bonuses(p.player_id, 4).await.unwrap()[0].atk, 3);
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
    async fn equip_rules_and_death_durability() {
        let db = mem().await;
        let p = db.register("carol", "pw").await.unwrap().player_id;
        let starter = db.get_gear(p).await.unwrap()[0].gear_id;

        // A second weapon; equipping it to the same hero (0) conflicts with the
        // equipped starter (one weapon per hero).
        db.insert_looted_gear(
            p,
            &[LootedGear {
                gear_id: Uuid::now_v7(),
                name: "Looted Sword".into(),
                slot: "weapon".into(),
                tier: 1,
                atk_bonus: 7,
                def_bonus: 0,
                spd_bonus: 0,
                base_max_durability: 80,
                max_durability: 80,
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
        // Per-character equip: the looted sword goes on hero 1 instead, no
        // conflict, and hero 0 keeps the starter — two different heroes with
        // two different weapons is exactly the point of this feature.
        assert_eq!(db.set_equipped(p, looted, Some(1)).await.unwrap(), EquipResult::Ok);
        let bonuses = db.equipped_gear_bonuses(p, 4).await.unwrap();
        assert_eq!(bonuses[0].atk, 3);
        assert_eq!(bonuses[1].atk, 7);
        assert_eq!(db.set_equipped(p, Uuid::now_v7(), Some(0)).await.unwrap(), EquipResult::NotFound);

        // Death sink only touches equipped blue-chest gear (looted sword is red).
        db.apply_death_durability(p).await.unwrap();
        let starter_row = db.get_gear(p).await.unwrap().into_iter().find(|g| g.gear_id == starter).unwrap();
        assert_eq!(starter_row.max_durability, 90); // floor(100 * 0.9)
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
}
