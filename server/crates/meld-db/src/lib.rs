//! Postgres persistence (CANON.md D18). The today-slice needs only accounts +
//! credentials; the Vault/gear/meld/economy schema lands with those systems.
//!
//! Passwords are stored **only** as bcrypt hashes (cost from balance, D17) — the
//! plaintext is never persisted or logged (BUILD-PLAN M1.8). Login returns an
//! indistinguishable result for unknown-username vs wrong-password (M1.9).

use bcrypt::{hash, verify};
use chrono::{DateTime, Utc};
use sqlx::postgres::{PgPool, PgPoolOptions};
use sqlx::Row;
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
    pool: PgPool,
    bcrypt_cost: u32,
}

impl Db {
    /// Connect to Postgres and return a pool handle.
    pub async fn connect(database_url: &str, bcrypt_cost: u32) -> Result<Self, DbError> {
        let pool = PgPoolOptions::new()
            .max_connections(8)
            .connect(database_url)
            .await?;
        Ok(Db { pool, bcrypt_cost })
    }

    /// Apply the (idempotent) schema. Safe to call on every boot.
    pub async fn migrate(&self) -> Result<(), DbError> {
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
        .execute(&self.pool)
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
        .execute(&self.pool)
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
        .execute(&self.pool)
        .await?;
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
        let mut tx = self.pool.begin().await?;
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
        Ok(())
    }

    /// Read a player's Vault: chits balance + item stacks (kind, quantity).
    pub async fn get_vault(&self, player_id: Uuid) -> Result<(i64, Vec<(String, i32)>), DbError> {
        let chits: i64 = sqlx::query_scalar("SELECT chits FROM vaults WHERE player_id = $1")
            .bind(player_id)
            .fetch_optional(&self.pool)
            .await?
            .unwrap_or(0);
        let rows = sqlx::query(
            "SELECT item_kind, quantity FROM vault_items WHERE player_id = $1 ORDER BY item_kind",
        )
        .bind(player_id)
        .fetch_all(&self.pool)
        .await?;
        let items = rows
            .iter()
            .map(|r| (r.get::<String, _>("item_kind"), r.get::<i32, _>("quantity")))
            .collect();
        Ok((chits, items))
    }

    /// Create an account. Hashes the password with bcrypt (cost from balance);
    /// the plaintext is dropped here and never stored. `Conflict` on dup username.
    pub async fn register(&self, username: &str, password: &str) -> Result<PlayerRow, DbError> {
        let password_hash = hash(password, self.bcrypt_cost)?;
        let player_id = Uuid::now_v7();
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
        .fetch_one(&self.pool)
        .await
        .map_err(|e| match &e {
            sqlx::Error::Database(db) if db.code().as_deref() == Some("23505") => DbError::Conflict,
            _ => DbError::Sqlx(e),
        })?;
        Ok(row_to_player(&row))
    }

    /// Verify a login. Returns `Some(player)` on a correct password, `None` for
    /// an unknown username OR a wrong password — indistinguishable, with matched
    /// timing (D17, M1.9).
    pub async fn verify_login(
        &self,
        username: &str,
        password: &str,
    ) -> Result<Option<PlayerRow>, DbError> {
        let maybe = sqlx::query(
            r#"
            SELECT player_id, username, password_hash, created_at, active_title
            FROM players WHERE username = $1
            "#,
        )
        .bind(username)
        .fetch_optional(&self.pool)
        .await?;

        match maybe {
            Some(row) => {
                let stored: String = row.get("password_hash");
                if verify(password, &stored).unwrap_or(false) {
                    Ok(Some(row_to_player(&row)))
                } else {
                    Ok(None)
                }
            }
            None => {
                // Burn equivalent time so a missing account isn't faster.
                let _ = verify(password, DUMMY_HASH);
                Ok(None)
            }
        }
    }

    /// Fetch an account by id (for `GET /v1/players/me`).
    pub async fn get_player(&self, player_id: Uuid) -> Result<Option<PlayerRow>, DbError> {
        let row = sqlx::query(
            r#"
            SELECT player_id, username, created_at, active_title
            FROM players WHERE player_id = $1
            "#,
        )
        .bind(player_id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(|r| row_to_player(&r)))
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
