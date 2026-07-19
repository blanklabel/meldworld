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
            // Sized above the expected concurrent-agent count (~20) so a connect
            // burst (everyone hitting vault/gear/me at once) doesn't queue behind
            // a small pool. Queries are short, so idle connections are cheap.
            .max_connections(32)
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
        .execute(&self.pool)
        .await?;
        // Forward-compat: add `tier` to any gear table created before this column
        // existed (CREATE TABLE IF NOT EXISTS won't alter an existing table).
        sqlx::query("ALTER TABLE gear ADD COLUMN IF NOT EXISTS tier INTEGER NOT NULL DEFAULT 0")
            .execute(&self.pool)
            .await?;
        // Every hot gear query filters by `owner_player_id` (get_gear,
        // equipped_atk_bonus on connect, death durability, equip checks), but a FK
        // is NOT auto-indexed in Postgres — so each was a full-table Seq Scan, and
        // `gear` is insert-only (never pruned), so it degraded linearly forever.
        sqlx::query("CREATE INDEX IF NOT EXISTS idx_gear_owner ON gear(owner_player_id)")
            .execute(&self.pool)
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
        .execute(&self.pool)
        .await?;
        // Persistent per-account hero names, one row per party slot. The class is
        // still chosen per dive in the party builder; only the name persists.
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS heroes (
                player_id UUID NOT NULL REFERENCES players(player_id),
                slot      SMALLINT NOT NULL,
                name      TEXT NOT NULL,
                PRIMARY KEY (player_id, slot)
            )
            "#,
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// The player's hero names by slot (0-based), ordered. Empty if never set.
    pub async fn get_hero_names(&self, player_id: Uuid) -> Result<Vec<String>, DbError> {
        let rows = sqlx::query("SELECT name FROM heroes WHERE player_id = $1 ORDER BY slot")
            .bind(player_id)
            .fetch_all(&self.pool)
            .await?;
        Ok(rows.iter().map(|r| r.get::<String, _>("name")).collect())
    }

    /// Rename a hero slot (upsert). Names are trimmed/capped by the caller.
    pub async fn set_hero_name(&self, player_id: Uuid, slot: i16, name: &str) -> Result<(), DbError> {
        sqlx::query(
            "INSERT INTO heroes (player_id, slot, name) VALUES ($1, $2, $3)
             ON CONFLICT (player_id, slot) DO UPDATE SET name = $3",
        )
        .bind(player_id)
        .bind(slot)
        .bind(name)
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
        let mut tx = self.pool.begin().await?;
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
            sqlx::Error::Database(db) if db.code().as_deref() == Some("23505") => DbError::Conflict,
            _ => DbError::Sqlx(e),
        })?;
        sqlx::query("INSERT INTO vaults (player_id, chits) VALUES ($1, 0)")
            .bind(player_id)
            .execute(&mut *tx)
            .await?;
        // A humble starting weapon (blue-chest, equipped, tier 0).
        sqlx::query(
            "INSERT INTO gear (gear_id, owner_player_id, name, slot, insurance, tier, atk_bonus, base_max_durability, max_durability, equipped)
             VALUES ($1, $2, 'Chipped Blade', 'weapon', 'blue', 0, 3, 100, 100, TRUE)",
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

    /// Credit Meld-skill XP (upsert; caps handled by the level curve on read).
    pub async fn add_skill_xp(&self, player_id: Uuid, kind: &str, xp: i64) -> Result<(), DbError> {
        sqlx::query(
            "INSERT INTO meld_skills (player_id, skill_kind, xp) VALUES ($1, $2, $3)
             ON CONFLICT (player_id, skill_kind) DO UPDATE SET xp = meld_skills.xp + $3",
        )
        .bind(player_id)
        .bind(kind)
        .bind(xp)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Read a player's Meld skills as (kind, total_xp).
    pub async fn get_skills(&self, player_id: Uuid) -> Result<Vec<(String, i64)>, DbError> {
        let rows =
            sqlx::query("SELECT skill_kind, xp FROM meld_skills WHERE player_id = $1 ORDER BY skill_kind")
                .bind(player_id)
                .fetch_all(&self.pool)
                .await?;
        Ok(rows
            .iter()
            .map(|r| (r.get::<String, _>("skill_kind"), r.get::<i64, _>("xp")))
            .collect())
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
        let mut tx = self.pool.begin().await?;
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

        // bcrypt verify is CPU-heavy — run it on the blocking pool (see `register`).
        match maybe {
            Some(row) => {
                let stored: String = row.get("password_hash");
                let pw = password.to_string();
                let ok = tokio::task::spawn_blocking(move || verify(pw, &stored).unwrap_or(false))
                    .await
                    .unwrap_or(false);
                if ok {
                    Ok(Some(row_to_player(&row)))
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

    /// List a player's gear.
    pub async fn get_gear(&self, player_id: Uuid) -> Result<Vec<GearRow>, DbError> {
        let rows = sqlx::query(
            "SELECT gear_id, name, slot, insurance, tier, atk_bonus, base_max_durability, max_durability, equipped
             FROM gear WHERE owner_player_id = $1 ORDER BY equipped DESC, name",
        )
        .bind(player_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.iter().map(row_to_gear).collect())
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
        let mut tx = self.pool.begin().await?;
        for g in gear {
            sqlx::query(
                "INSERT INTO gear (gear_id, owner_player_id, name, slot, insurance, tier, atk_bonus, base_max_durability, max_durability, equipped)
                 VALUES ($1, $2, $3, $4, 'red', $5, $6, $7, $8, FALSE)
                 ON CONFLICT (gear_id) DO NOTHING",
            )
            .bind(g.gear_id)
            .bind(player_id)
            .bind(&g.name)
            .bind(&g.slot)
            .bind(g.tier)
            .bind(g.atk_bonus)
            .bind(g.base_max_durability)
            .bind(g.max_durability)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    /// Total attack bonus from a player's currently-equipped gear.
    pub async fn equipped_atk_bonus(&self, player_id: Uuid) -> Result<i32, DbError> {
        let bonus: Option<i64> = sqlx::query_scalar(
            "SELECT COALESCE(SUM(atk_bonus), 0) FROM gear WHERE owner_player_id = $1 AND equipped = TRUE",
        )
        .bind(player_id)
        .fetch_one(&self.pool)
        .await?;
        Ok(bonus.unwrap_or(0) as i32)
    }

    /// Apply the death durability sink to equipped blue-chest gear:
    /// `max_durability ← floor(max_durability × 0.9)` (CANON.md D6).
    pub async fn apply_death_durability(&self, player_id: Uuid) -> Result<(), DbError> {
        sqlx::query(
            "UPDATE gear SET max_durability = (max_durability * 9) / 10
             WHERE owner_player_id = $1 AND insurance = 'blue' AND equipped = TRUE",
        )
        .bind(player_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Equip or unequip a gear item, enforcing the loadout rules (vault-gear.md
    /// equip endpoint). Equipping is idempotent, rejects broken gear (max
    /// durability 0, CANON.md D6), and enforces one item per `slot` (a different
    /// item already in the slot is a conflict — the caller unequips it first).
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
        equipped: bool,
    ) -> Result<EquipResult, DbError> {
        let mut tx = self.pool.begin().await?;
        // Load the target (owner-scoped so existence isn't leaked cross-account).
        let row = sqlx::query(
            "SELECT slot, max_durability, equipped FROM gear
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
        let already: bool = row.get("equipped");

        if !equipped {
            // Unequip is idempotent; just clear the flag.
            sqlx::query("UPDATE gear SET equipped = FALSE WHERE gear_id = $1")
                .bind(gear_id)
                .execute(&mut *tx)
                .await?;
            tx.commit().await?;
            return Ok(EquipResult::Ok);
        }

        // Equip: idempotent no-op if already worn.
        if already {
            tx.rollback().await?;
            return Ok(EquipResult::Ok);
        }
        // Broken gear cannot be equipped until repaired (CANON.md D6).
        if max_durability == 0 {
            tx.rollback().await?;
            return Ok(EquipResult::Broken);
        }
        // One item per slot: a different equipped item in the same slot conflicts.
        let occupied: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM gear
             WHERE owner_player_id = $1 AND slot = $2 AND equipped = TRUE AND gear_id <> $3",
        )
        .bind(player_id)
        .bind(&slot)
        .bind(gear_id)
        .fetch_one(&mut *tx)
        .await?;
        if occupied > 0 {
            tx.rollback().await?;
            return Ok(EquipResult::SlotOccupied);
        }
        sqlx::query("UPDATE gear SET equipped = TRUE WHERE gear_id = $1")
            .bind(gear_id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(EquipResult::Ok)
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

/// A red-chest gear item to bank into the Vault on extraction.
#[derive(Debug, Clone)]
pub struct LootedGear {
    pub gear_id: Uuid,
    pub name: String,
    pub slot: String,
    pub tier: i32,
    pub atk_bonus: i32,
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
    pub base_max_durability: i32,
    pub max_durability: i32,
    pub equipped: bool,
}

fn row_to_gear(row: &sqlx::postgres::PgRow) -> GearRow {
    GearRow {
        gear_id: row.get("gear_id"),
        name: row.get("name"),
        slot: row.get("slot"),
        insurance: row.get("insurance"),
        tier: row.get("tier"),
        atk_bonus: row.get("atk_bonus"),
        base_max_durability: row.get("base_max_durability"),
        max_durability: row.get("max_durability"),
        equipped: row.get("equipped"),
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
